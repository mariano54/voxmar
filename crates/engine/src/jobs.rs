//! How many streaming jobs (generating columns, meshing them, building distant land) run on
//! the shared workers at once. Unlimited unless a game client sets a limit
//! (`client::workers`): one that streams on fewer workers than the pool has, so the rest of
//! the machine's cores stay with its frames. Jobs are never held back once started; a job
//! that finds no room is not started, and its dispatcher tries again on a later frame or tick.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Most streaming jobs at once; 0 for no limit.
static LIMIT: AtomicUsize = AtomicUsize::new(0);
/// Streaming jobs started and not yet finished.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Sets the most streaming jobs at once (0 for no limit). Jobs already running go on.
pub fn set_limit(jobs: usize) {
    LIMIT.store(jobs, Ordering::Relaxed);
}

/// Room for one more streaming job, if there is any: held by the job while it runs.
#[must_use]
pub fn start() -> Option<Slot> {
    RUNNING
        .try_update(Ordering::AcqRel, Ordering::Acquire, |running| {
            let limit = LIMIT.load(Ordering::Relaxed);
            (limit == 0 || running < limit).then_some(running + 1)
        })
        .ok()
        .map(|_| Slot(()))
}

/// A streaming job's room, given back when the job ends (dropped).
pub struct Slot(());

impl Drop for Slot {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_more_than_the_limit_at_once() {
        set_limit(2);
        let (a, b) = (start(), start());
        assert!(a.is_some() && b.is_some());
        assert!(start().is_none(), "a third waits");
        drop(a);
        let c = start();
        assert!(c.is_some(), "room again once one ends");
        set_limit(0);
        assert!(start().is_some(), "no limit");
        drop((b, c));
    }
}
