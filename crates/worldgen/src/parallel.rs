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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Whether builds use helper threads. Benchmarks that measure CPU time on one thread turn
/// it off, so the work they measure stays on that thread.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Helper threads alive now, across every world.
static HELPERS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Calls `f` on every item, on the calling thread and on helpers (up to one per other
/// core, as the cap allows), each taking the next item in order.
pub(crate) fn for_each<T: Sync>(items: &[T], f: impl Fn(&T) + Sync) {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let wanted = cores.min(items.len()).saturating_sub(1);
    let helpers = if ENABLED.load(Ordering::Relaxed) { reserve(wanted, 2 * cores) } else { 0 };
    if helpers == 0 {
        items.iter().for_each(f);
        return;
    }
    let next = AtomicUsize::new(0);
    let work = || {
        while let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
            f(item);
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..helpers {
            scope.spawn(work);
        }
        work();
    });
    HELPERS.fetch_sub(helpers, Ordering::Relaxed);
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
}
