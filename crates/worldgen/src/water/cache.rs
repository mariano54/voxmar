//! A bounded, thread-safe memo of values computed once per key.
//!
//! Each key gets a cell that one thread fills while others asking for the same key wait
//! for it, so nothing is computed twice at once, and different keys are computed in
//! parallel. Every value has a size in bytes; once the values together pass the cache's
//! budget, the least recently used are dropped. A dropped value is computed again,
//! identically, when next needed, because everything cached here is a pure function of
//! its key.
//!
//! The exception to computing once, in a cache made with `Cache::copying` (and in a
//! `Shared` value inside a cached one): an urgent thread (the server's tick, see
//! `parallel::urgent`) asking for a value another thread is making makes its own copy rather
//! than wait, and the first of the two made is the one kept, so the tick asking again (as
//! it does, cell by cell) has it at once. The maker is usually a streaming worker at a
//! background priority, which on a busy machine the scheduler serves last, so waiting on it
//! held the tick (and the world's creatures) for as long as the build took at that priority.
//! Only caches whose `make` does nothing but compute take it: one whose `make` also takes
//! something out of shared state (settlement plans take their early spawns) would see that
//! done twice.

use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

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

impl<T> Weigh for [T] {
    fn bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }
}

pub struct Cache<K, V> {
    budget: usize,
    /// Whether an urgent thread makes its own value rather than wait on another's.
    copying: bool,
    inner: Mutex<Inner<K, V>>,
}

struct Slot<V> {
    cell: Arc<Shared<V>>,
    used: u64,
    /// Bytes of the value, once made.
    bytes: usize,
}

struct Inner<K, V> {
    clock: u64,
    bytes: usize,
    slots: FxHashMap<K, Slot<V>>,
}

/// A value made once, by the first thread to ask, which others asking meanwhile wait for;
/// but an urgent thread asking while it is made makes its own copy, and whichever is done
/// first is kept (see the module's doc). For values that `make` only computes.
pub struct Shared<V> {
    /// Set once, by the first to finish making it; never made inside, so setting it never
    /// waits on a make.
    value: OnceLock<V>,
    /// Whether a thread is making it (not counting urgent copies): looked at and claimed
    /// under one lock, so no urgent thread finds it being made and still waits.
    making: Mutex<bool>,
    /// Told when it is made, or its maker gave up (panicked).
    made: Condvar,
}

impl<V> Default for Shared<V> {
    fn default() -> Self {
        Self { value: OnceLock::new(), making: Mutex::new(false), made: Condvar::new() }
    }
}

impl<V> Shared<V> {
    pub fn get(&self) -> Option<&V> {
        self.value.get()
    }

    /// The value, made by `make` unless it is made, or being made by another thread (then
    /// waited for, or by an urgent thread made too, see the module's doc).
    pub fn get_or_make(&self, make: impl FnOnce() -> V) -> &V {
        self.make_or_wait(make, true, Hold::Inner).0
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        self.making.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The value, and whether this call's make is the one kept. With `copying`, an urgent
    /// thread finding it being made makes its own rather than wait.
    fn make_or_wait(&self, make: impl FnOnce() -> V, copying: bool, _kind: Hold) -> (&V, bool) {
        loop {
            if let Some(value) = self.value.get() {
                return (value, false);
            }
            let mut making = self.lock();
            if self.value.get().is_some() {
                continue;
            }
            let maker = !*making;
            if !maker && !(copying && crate::parallel::urgent()) {
                // Made by then, or its maker gave up and this thread may take it on.
                drop(self.made.wait(making));
                continue;
            }
            *making = true;
            drop(making);
            // A maker that panics leaves nothing made and unmarks it, for a waiter to make.
            struct GiveUp<'a, V>(&'a Shared<V>);
            impl<V> Drop for GiveUp<'_, V> {
                fn drop(&mut self) {
                    *self.0.lock() = false;
                    self.0.made.notify_all();
                }
            }
            let give_up = maker.then(|| GiveUp(self));
            let value = make();
            #[cfg(test)]
            if maker {
                hold(_kind);
            }
            let kept = self.value.set(value).is_ok();
            // Unmarks it (a maker's), and tells the waiters.
            drop(give_up);
            self.made.notify_all();
            return (self.value.get().expect("set above"), kept);
        }
    }
}

/// Which makes a test holds (`hold_on`): those of a cache's values, or of `Shared` values
/// inside them.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Hold {
    Cached,
    Inner,
}

#[cfg(test)]
std::thread_local! {
    static HOLDING: std::cell::RefCell<Option<(Hold, std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
}

/// Has the calling thread's next make of a `kind` of value, once made and before anyone
/// else sees it, say so on `held` and wait for `release` (or a while): the value, and every
/// value whose making waits on it, is then being made for as long as a test wants.
#[cfg(test)]
pub(crate) fn hold_on(kind: Hold, held: std::sync::mpsc::Sender<()>, release: std::sync::mpsc::Receiver<()>) {
    HOLDING.with(|holding| *holding.borrow_mut() = Some((kind, held, release)));
}

#[cfg(test)]
fn hold(kind: Hold) {
    let Some((_, held, release)) = HOLDING.with(|holding| {
        let mut holding = holding.borrow_mut();
        if holding.as_ref().is_some_and(|(k, _, _)| *k == kind) { holding.take() } else { None }
    }) else {
        return;
    };
    let _ = held.send(());
    let _ = release.recv_timeout(std::time::Duration::from_secs(600));
}

/// Bookkeeping per entry besides the value itself.
const OVERHEAD: usize = 64;

impl<K: Copy + Eq + Hash, V: Clone + Weigh> Cache<K, V> {
    /// A cache holding up to about `budget` bytes.
    pub fn new(budget: usize) -> Self {
        Self { budget, copying: false, inner: Mutex::new(Inner { clock: 0, bytes: 0, slots: FxHashMap::default() }) }
    }

    /// A cache holding up to about `budget` bytes, of which an urgent thread makes its own
    /// copy of a value being made rather than wait (see the module's doc): for values whose
    /// `make` only computes.
    pub fn copying(budget: usize) -> Self {
        Self { copying: true, ..Self::new(budget) }
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
            let slot = inner.slots.entry(key).or_insert_with(|| Slot { cell: Arc::default(), used: clock, bytes: 0 });
            slot.used = clock;
            slot.cell.clone()
        };
        if let Some(value) = cell.value.get() {
            return value.clone();
        }
        let (value, kept) = cell.make_or_wait(make, self.copying, Hold::Cached);
        let value = value.clone();
        if kept {
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

    /// The server's tick (an urgent thread), asking a copying cache for a value a background
    /// worker is still making, makes its own, which, done first, is the one kept: here the
    /// maker holds off until the urgent thread has its answer.
    #[test]
    fn an_urgent_thread_makes_its_own_rather_than_wait() {
        let cache: Cache<u32, Arc<u8>> = Cache::copying(1 << 20);
        let (started_tx, started) = std::sync::mpsc::channel();
        let (answered_tx, answered) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            let cache = &cache;
            let maker = scope.spawn(move || {
                cache.get(1, || {
                    started_tx.send(()).unwrap();
                    // Released once the urgent thread has its own value, or after a while if
                    // it waited after all (and then the assertion below fails).
                    let _ = answered.recv_timeout(std::time::Duration::from_secs(5));
                    Arc::new(1)
                })
            });
            started.recv().unwrap();
            let urgent = scope.spawn(move || {
                crate::parallel::mark_urgent();
                let value = cache.get(1, || Arc::new(2));
                answered_tx.send(()).unwrap();
                value
            });
            assert_eq!(*urgent.join().unwrap(), 2, "made its own");
            assert_eq!(*maker.join().unwrap(), 2, "the first made is everyone's");
        });
        assert_eq!(*cache.get(1, || Arc::new(3)), 2, "and the one kept");
        assert_eq!(cache.inner.lock().unwrap().bytes, 1 + OVERHEAD, "counted once");
    }

    /// A maker that panics leaves the value unmade and unmarked: the next thread to ask makes
    /// it, and an urgent one asking meanwhile waits for that one.
    #[test]
    fn a_maker_that_panics_leaves_it_unmarked() {
        let shared: Shared<u8> = Shared::default();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = shared.get_or_make(|| panic!("no value"));
        }));
        assert!(panicked.is_err());
        assert!(shared.get().is_none());
        assert!(!*shared.lock());
        assert_eq!(*shared.get_or_make(|| 7), 7);
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
