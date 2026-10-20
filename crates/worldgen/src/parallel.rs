//! Building the regions generation shares (rivers, trails) on every core.
//!
//! A column that needs a region nobody has built yet waits for it, and so does every
//! other worker that needs it, so the build is on the critical path of loading: the first
//! columns of a new world, or of new land while flying. Its parts (the ground samples of
//! many tiles, the grids of many trail regions) are independent and cached, so warming
//! them on scoped threads first and then running the build as before gives exactly the
//! same result, sooner.
//!
//! The helpers are fresh OS threads, not the caller's thread pool: a pool worker waiting
//! for its helpers could otherwise pick up another job that waits on the very region it
//! is building. Helpers only ever wait on regions that do not depend on the one being
//! built (water runs downhill, tiles only read the trunk tier), so they cannot deadlock
//! either. Builds nest (a trail grid needs a river region, which needs tiles), so the
//! helpers alive at once are capped, past which a build runs on its caller alone.
//!
//! Helpers take their caller's scheduling class: on macOS a new thread starts at the default
//! class whatever its creator's, so the game's background workers (utility) would otherwise
//! start up to a worker count of helpers above themselves, level with the frame threads.
//! On Linux a thread inherits its creator's nice value already.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Whether builds use helper threads. Benchmarks that measure CPU time on one thread turn
/// it off, so the work they measure stays on that thread.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Helper threads alive now, across every world.
static HELPERS: AtomicUsize = AtomicUsize::new(0);

/// Most helper threads alive at once, and so taken by one build; 0 for one per core.
static LIMIT: AtomicUsize = AtomicUsize::new(0);

/// Caps the helper threads alive at once at `threads` (0 for one per core). The game client
/// gives it its worker count: on a machine with two hardware threads a core, a build taking
/// one helper per hardware thread while every worker was busy left the main and render
/// threads waiting for a core, the worst frames of a fast flight.
pub fn set_helper_limit(threads: usize) {
    LIMIT.store(threads, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Calls `f` on every item, on the calling thread and on helpers (up to one per other
/// core, as the cap allows), each taking the next item in order.
pub(crate) fn for_each<T: Sync>(items: &[T], f: impl Fn(&T) + Sync) {
    // Given a share of the cores (`RAYON_NUM_THREADS`, as the gate gives its test binaries,
    // the server's and the gameplay tests' as well as these), that share and no more helpers
    // alive than it: eight tests each taking a helper a core ran an 18-core machine three
    // times over, and the server binaries' worlds once had 36 helpers out at once.
    let (cores, cap) = match given_threads() {
        Some(given) => (given, given),
        None => {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            (cores, 2 * cores)
        }
    };
    let limit = LIMIT.load(Ordering::Relaxed);
    let wanted = cores.min(items.len()).saturating_sub(1);
    let cap = if limit == 0 { cap } else { limit };
    let helpers = if ENABLED.load(Ordering::Relaxed) { reserve(wanted.min(cap), cap) } else { 0 };
    if helpers == 0 {
        items.iter().for_each(f);
        return;
    }
    let next = AtomicUsize::new(0);
    let class = SchedulingClass::current();
    let work = || {
        while let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
            f(item);
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..helpers {
            scope.spawn(|| {
                class.apply();
                work();
            });
        }
        work();
    });
    HELPERS.fetch_sub(helpers, Ordering::Relaxed);
}

/// A thread's scheduling class (its QoS class on macOS; elsewhere nothing to carry).
#[derive(Clone, Copy)]
struct SchedulingClass(#[cfg(target_os = "macos")] Option<libc::qos_class_t>);

impl SchedulingClass {
    fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut relative = 0;
            // SAFETY: writes the calling thread's class and relative priority into the two
            // locals it is given.
            let found = unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative) } == 0;
            Self((found && !matches!(class, libc::qos_class_t::QOS_CLASS_UNSPECIFIED)).then_some(class))
        }
        #[cfg(not(target_os = "macos"))]
        Self()
    }

    fn apply(self) {
        #[cfg(target_os = "macos")]
        if let Some(class) = self.0 {
            // SAFETY: only changes the scheduling class of the current thread.
            unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
        }
    }
}

/// The threads the process was given with `RAYON_NUM_THREADS`, if any.
fn given_threads() -> Option<usize> {
    static GIVEN: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *GIVEN.get_or_init(|| std::env::var("RAYON_NUM_THREADS").ok().and_then(|n| n.parse().ok()).filter(|&n| n > 0))
}

/// Takes up to `wanted` helpers while fewer than `cap` are alive; how many it took.
fn reserve(wanted: usize, cap: usize) -> usize {
    let mut taken = 0;
    let _ = HELPERS.try_update(Ordering::Relaxed, Ordering::Relaxed, |alive| {
        taken = wanted.min(cap.saturating_sub(alive));
        Some(alive + taken)
    });
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_once_per_item() {
        let items: Vec<usize> = (0..1000).collect();
        let calls: Vec<AtomicUsize> = (0..1000).map(|_| AtomicUsize::new(0)).collect();
        for_each(&items, |&i| {
            // Nested, as builds are.
            for_each(&[i], |&j| {
                calls[j].fetch_add(1, Ordering::Relaxed);
            });
        });
        assert!(calls.iter().all(|c| c.load(Ordering::Relaxed) == 1));
    }

    /// A build on one of the game's background workers keeps its helpers in the background.
    #[cfg(target_os = "macos")]
    #[test]
    fn helpers_keep_their_callers_class() {
        let classes = std::thread::spawn(|| {
            // SAFETY: only changes the scheduling class of the current thread.
            unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
            let items: Vec<usize> = (0..64).collect();
            let classes = std::sync::Mutex::new(Vec::new());
            for_each(&items, |_| {
                std::thread::sleep(std::time::Duration::from_millis(1));
                classes.lock().unwrap().push(SchedulingClass::current().0);
            });
            classes.into_inner().unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(classes.len(), 64);
        assert!(classes.iter().all(|c| matches!(c, Some(libc::qos_class_t::QOS_CLASS_UTILITY))));
    }
}
