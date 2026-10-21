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
//! Above the default class, though, they take the default: a build the server's tick
//! (user-interactive) starts would otherwise put up to a worker count of helpers level with
//! a game's frame threads. On Linux a thread inherits its creator's nice value already. `VOXMAR_THREAD_PRIORITY=off`
//! (the game's timing switch, see the client's `priority.rs`) leaves helpers as they start.
//! Helpers of an urgent thread (`urgent`) are urgent too, whatever their class: they do its
//! work, which must not wait on a background build either.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

std::thread_local! {
    /// Whether this thread said it must never wait on a build at a lower priority
    /// (`mark_urgent`), or helps one that did.
    static URGENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the calling thread as one that must never wait on a region another thread, at a
/// lower priority, is building: it makes its own copy instead (water/cache.rs). The
/// server's tick calls it, on every platform: on a busy machine a build at a background
/// priority is served last, on Linux (a higher nice value) as on macOS (a lower class).
pub fn mark_urgent() {
    URGENT.with(|urgent| urgent.set(true));
}

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
/// threads waiting for a core, the worst frames of a fast flight. A limit on streaming
/// (`voxmar_engine::jobs::limit`) caps them further, and lifts with it.
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
    let limit = helper_limit(LIMIT.load(Ordering::Relaxed), voxmar_engine::jobs::limit());
    let wanted = cores.min(items.len()).saturating_sub(1);
    let cap = if limit == 0 { cap } else { limit };
    let helpers = if ENABLED.load(Ordering::Relaxed) { reserve(wanted.min(cap), cap) } else { 0 };
    if helpers == 0 {
        items.iter().for_each(f);
        return;
    }
    with_helpers(items, f, helpers);
    HELPERS.fetch_sub(helpers, Ordering::Relaxed);
}

/// The most helpers alive at once (0 for one per core): `set`'s, and no more than streaming
/// jobs may run at once (0 for no limit).
fn helper_limit(set: usize, streaming: usize) -> usize {
    match (set, streaming) {
        (0, limit) | (limit, 0) => limit,
        (set, streaming) => set.min(streaming),
    }
}

/// Calls `f` on every item, on the calling thread and `helpers` fresh threads of the
/// caller's scheduling class, each taking the next item in order.
fn with_helpers<T: Sync>(items: &[T], f: impl Fn(&T) + Sync, helpers: usize) {
    let next = AtomicUsize::new(0);
    let class = SchedulingClass::current().capped();
    let urgent = urgent();
    let work = || {
        while let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
            f(item);
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..helpers {
            scope.spawn(|| {
                class.apply();
                if urgent {
                    mark_urgent();
                }
                work();
            });
        }
        work();
    });
}

/// A thread's scheduling class (its QoS class on macOS; elsewhere nothing to carry).
#[derive(Clone, Copy)]
struct SchedulingClass(#[cfg(target_os = "macos")] Option<libc::qos_class_t>);

impl SchedulingClass {
    fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            static CARRIED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if !*CARRIED.get_or_init(|| std::env::var("VOXMAR_THREAD_PRIORITY").map_or(true, |v| v != "off")) {
                return Self(None);
            }
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

    /// This class, or the default if it is above it.
    fn capped(self) -> Self {
        #[cfg(target_os = "macos")]
        {
            use libc::qos_class_t::{QOS_CLASS_DEFAULT, QOS_CLASS_USER_INITIATED, QOS_CLASS_USER_INTERACTIVE};
            Self(self.0.map(|class| if matches!(class, QOS_CLASS_USER_INTERACTIVE | QOS_CLASS_USER_INITIATED) { QOS_CLASS_DEFAULT } else { class }))
        }
        #[cfg(not(target_os = "macos"))]
        self
    }

    /// Whether this is a class above the default, which a game's frame threads and the
    /// server's tick ask for.
    fn urgent(self) -> bool {
        #[cfg(target_os = "macos")]
        {
            use libc::qos_class_t::{QOS_CLASS_USER_INITIATED, QOS_CLASS_USER_INTERACTIVE};
            matches!(self.0, Some(QOS_CLASS_USER_INTERACTIVE | QOS_CLASS_USER_INITIATED))
        }
        #[cfg(not(target_os = "macos"))]
        false
    }

    fn apply(self) {
        #[cfg(target_os = "macos")]
        if let Some(class) = self.0 {
            // SAFETY: only changes the scheduling class of the current thread.
            unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
        }
    }
}

/// Whether the calling thread must never wait on a build at a lower priority: marked so
/// (`mark_urgent`), helping a thread that is, or on macOS of a class above the default (the
/// server's tick, a game's frame threads). The scheduler serves the builder after everything
/// above it, the waiter's priority notwithstanding (water/cache.rs).
pub(crate) fn urgent() -> bool {
    URGENT.with(|urgent| urgent.get()) || SchedulingClass::current().urgent()
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

    /// Capped by streaming's limit while there is one, so lifted while land is missing.
    #[test]
    fn helpers_take_no_more_than_streaming_may() {
        assert_eq!(helper_limit(0, 0), 0, "one per core");
        assert_eq!(helper_limit(16, 0), 16, "no streaming limit");
        assert_eq!(helper_limit(16, 4), 4);
        assert_eq!(helper_limit(0, 4), 4);
        assert_eq!(helper_limit(3, 4), 3);
    }

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
    /// Three items, two helpers and a barrier each item waits at: every thread takes one.
    #[cfg(target_os = "macos")]
    #[test]
    fn helpers_keep_their_callers_class() {
        let classes = std::thread::spawn(|| {
            // SAFETY: only changes the scheduling class of the current thread.
            unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
            let barrier = std::sync::Barrier::new(3);
            let seen = std::sync::Mutex::new(Vec::new());
            with_helpers(&[0, 1, 2], |_| {
                seen.lock().unwrap().push((std::thread::current().id(), SchedulingClass::current().0));
                barrier.wait();
            }, 2);
            (std::thread::current().id(), seen.into_inner().unwrap())
        })
        .join()
        .unwrap();
        let (caller, seen) = classes;
        assert_eq!(seen.iter().filter(|(thread, _)| *thread != caller).count(), 2, "two helpers took an item each");
        assert!(seen.iter().all(|(_, class)| matches!(class, Some(libc::qos_class_t::QOS_CLASS_UTILITY))));
    }

    /// The server's tick (user-interactive) keeps its helpers at the default class, below the
    /// frame threads; a background class stays as it is.
    #[cfg(target_os = "macos")]
    #[test]
    fn helpers_are_never_above_the_default_class() {
        use libc::qos_class_t::*;
        let capped = |class| SchedulingClass(Some(class)).capped().0;
        assert!(matches!(capped(QOS_CLASS_USER_INTERACTIVE), Some(QOS_CLASS_DEFAULT)));
        assert!(matches!(capped(QOS_CLASS_USER_INITIATED), Some(QOS_CLASS_DEFAULT)));
        assert!(matches!(capped(QOS_CLASS_UTILITY), Some(QOS_CLASS_UTILITY)));
        assert!(SchedulingClass(None).capped().0.is_none());
    }

    /// Helpers of an urgent thread are urgent, and those of any other are not.
    #[test]
    fn helpers_of_an_urgent_thread_are_urgent() {
        let helped = |marked: bool| {
            std::thread::spawn(move || {
                if marked {
                    mark_urgent();
                }
                let barrier = std::sync::Barrier::new(2);
                let seen = std::sync::Mutex::new(Vec::new());
                with_helpers(&[0, 1], |_| {
                    seen.lock().unwrap().push(urgent());
                    barrier.wait();
                }, 1);
                seen.into_inner().unwrap()
            })
            .join()
            .unwrap()
        };
        assert_eq!(helped(true), [true, true]);
        assert_eq!(helped(false), [false, false]);
    }
}
