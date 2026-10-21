//! A bounded, thread-safe memo of values computed once per key.
//!
//! Each key gets a cell that one thread fills while others asking for the same key wait
//! for it, so nothing is computed twice at once, and different keys are computed in
//! parallel. Every value has a size in bytes; once the values together pass the cache's
//! budget, the least recently used are dropped. A dropped value is computed again,
//! identically, when next needed, because everything cached here is a pure function of
//! its key.

use std::hash::Hash;
use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;

/// Values that know roughly how much memory they hold.
pub trait Weigh {
    /// Bytes held, counting what the value owns on the heap.
    fn bytes(&self) -> usize;
}

impl<T: Weigh + ?Sized> Weigh for Arc<T> {
    fn bytes(&self) -> usize {
        (**self).bytes()
    }
}

/// A generated column holds about this much (106 KiB measured over 576 columns round a
/// spawn), counted for each as its chunks are not walked: a budget of 128 MiB keeps about
/// 1170 columns.
impl Weigh for voxmar_engine::world::Column {
    fn bytes(&self) -> usize {
        112 << 10
    }
}

impl<T> Weigh for [T] {
    fn bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }
}

pub struct Cache<K, V> {
    budget: usize,
    inner: Mutex<Inner<K, V>>,
}

struct Slot<V> {
    cell: Arc<OnceLock<V>>,
    used: u64,
    /// Bytes of the value, once made.
    bytes: usize,
}

struct Inner<K, V> {
    clock: u64,
    bytes: usize,
    slots: FxHashMap<K, Slot<V>>,
}

/// Bookkeeping per entry besides the value itself.
const OVERHEAD: usize = 64;

impl<K: Copy + Eq + Hash, V: Clone + Weigh> Cache<K, V> {
    /// A cache holding up to about `budget` bytes.
    pub fn new(budget: usize) -> Self {
        Self { budget, inner: Mutex::new(Inner { clock: 0, bytes: 0, slots: FxHashMap::default() }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<K, V>> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many values are cached or being made.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().slots.len()
    }

    /// Whether the value for `key` is cached or being made.
    pub fn has(&self, key: K) -> bool {
        self.lock().slots.contains_key(&key)
    }

    /// The value for `key`, made by `make` unless it is cached or being made.
    ///
    /// `make` must not ask this cache for the same key, which would wait on itself.
    pub fn get(&self, key: K, make: impl FnOnce() -> V) -> V {
        let cell = {
            let mut inner = self.lock();
            inner.clock += 1;
            let clock = inner.clock;
            let slot = inner.slots.entry(key).or_insert_with(|| Slot { cell: Arc::new(OnceLock::new()), used: clock, bytes: 0 });
            slot.used = clock;
            slot.cell.clone()
        };
        let mut made = false;
        let value = cell
            .get_or_init(|| {
                made = true;
                make()
            })
            .clone();
        if made {
            let mut inner = self.lock();
            let bytes = value.bytes() + OVERHEAD;
            if let Some(slot) = inner.slots.get_mut(&key)
                && Arc::ptr_eq(&slot.cell, &cell)
            {
                slot.bytes = bytes;
                inner.bytes += bytes;
                if inner.bytes > self.budget {
                    inner.evict(self.budget - self.budget / 4);
                }
            }
        }
        value
    }
}

impl<K: Eq + Hash, V> Inner<K, V> {
    /// Drops the least recently used entries until the rest hold at most `keep` bytes.
    /// Entries still being made weigh nothing yet and stay.
    fn evict(&mut self, keep: usize) {
        let mut ages: Vec<(u64, usize)> = self.slots.values().filter(|s| s.bytes > 0).map(|s| (s.used, s.bytes)).collect();
        ages.sort_unstable_by_key(|&(used, _)| std::cmp::Reverse(used));
        let mut kept = 0;
        let mut oldest_kept = u64::MAX;
        for (used, bytes) in ages {
            if kept + bytes > keep {
                break;
            }
            kept += bytes;
            oldest_kept = used;
        }
        self.slots.retain(|_, s| s.bytes == 0 || s.used >= oldest_kept);
        self.bytes = self.slots.values().map(|s| s.bytes).sum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Weigh for i32 {
        fn bytes(&self) -> usize {
            100 - OVERHEAD
        }
    }

    impl Weigh for u8 {
        fn bytes(&self) -> usize {
            1
        }
    }

    #[test]
    fn computes_once_and_evicts_the_least_recently_used() {
        // Room for eight values of 100 bytes.
        let cache = Cache::new(800);
        let mut made = 0;
        for round in 0..3 {
            for key in 0..6 {
                assert_eq!(cache.get(key, || { made += 1; key * 10 }), key * 10, "round {round}");
            }
        }
        assert_eq!(made, 6);
        // Filling past the capacity drops the oldest keys, which are then made again.
        for key in 6..20 {
            cache.get(key, || key * 10);
        }
        assert!(cache.inner.lock().unwrap().slots.len() <= 8);
        assert!(cache.inner.lock().unwrap().bytes <= 800);
        assert_eq!(cache.get(0, || { made += 1; 0 }), 0);
        assert_eq!(made, 7);
    }

    #[test]
    fn threads_share_one_computation() {
        let cache = Cache::new(1 << 20);
        let made = std::sync::atomic::AtomicU32::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    let value = cache.get(1, || {
                        made.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        Arc::new(42u8)
                    });
                    assert_eq!(*value, 42);
                });
            }
        });
        assert_eq!(made.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
