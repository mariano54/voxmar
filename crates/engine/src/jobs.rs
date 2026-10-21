//! How many streaming jobs (generating columns, meshing them, building distant land) run on
//! the shared workers at once. Unlimited unless a game client sets a limit
//! (`client::workers`): one that streams on fewer workers than the pool has, so the rest of
//! the machine's cores stay with its frames. Jobs are never held back once started; a job
//! that finds no room is not started, and its dispatcher tries again on a later frame or tick.
//!
//! Under a limit each kind has its share, since the client asks every frame and the server
//! only every tick: generation always has a third of the room, rounded down (at least one
//! job: a fifth at a limit of 5), that the others may not take, distant land, the least
//! urgent, at most that much, and meshing at least one. While a player's land around them
//! is missing (any server says so, each tick: `LandMissing`), as after a teleport beyond
//! the view, there is no limit: that is loading again.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What a streaming job does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Generating (or loading) a column near a player, on the server.
    Columns,
    /// Lighting and meshing a column that has come, on the client.
    Meshes,
    /// Building a tile of the distant land, on the client.
    DistantLand,
}

/// The least limit there is, short of none: room for one job of each kind.
pub const LEAST: usize = 3;

struct Counts {
    /// Most streaming jobs at once; 0 for no limit.
    limit: usize,
    running: [usize; 3],
}

static COUNTS: Mutex<Counts> = Mutex::new(Counts { limit: 0, running: [0; 3] });

/// Servers saying some player's land around them is missing (`LandMissing`).
static MISSING: AtomicUsize = AtomicUsize::new(0);

fn counts() -> std::sync::MutexGuard<'static, Counts> {
    COUNTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets the most streaming jobs at once: 0 for no limit, else at least `LEAST`. Jobs already
/// running go on.
pub fn set_limit(jobs: usize) {
    counts().limit = if jobs == 0 { 0 } else { jobs.max(LEAST) };
}

/// One server's say in whether some player's land around them is missing: no limit while
/// any server's says so. Taken back when dropped, with its server, so a server stopped
/// while land was missing leaves no other world in the process streaming unlimited, and
/// servers side by side (as tests run them) never clear each other's.
#[derive(Default)]
pub struct LandMissing(bool);

impl LandMissing {
    pub fn set(&mut self, missing: bool) {
        if missing != self.0 {
            self.0 = missing;
            if missing {
                MISSING.fetch_add(1, Ordering::Relaxed);
            } else {
                MISSING.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for LandMissing {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// The most streaming jobs at once now: 0 for no limit, as while land around a player is
/// missing. Generation's region builds take no more helper threads than this
/// (`voxmar_worldgen::parallel`), so they lift with it.
pub fn limit() -> usize {
    limit_of(&counts())
}

fn limit_of(counts: &Counts) -> usize {
    if MISSING.load(Ordering::Relaxed) > 0 { 0 } else { counts.limit }
}

/// Room for one more streaming job of `kind`, if there is any: held by the job while it runs.
#[must_use]
pub fn start(kind: Kind) -> Option<Slot> {
    let mut counts = counts();
    let limit = limit_of(&counts);
    (limit == 0 || room(limit, counts.running, kind)).then(|| {
        counts.running[kind as usize] += 1;
        Slot(kind)
    })
}

/// Whether `running` (columns, meshes, distant land) leaves room for one more of `kind`
/// under `limit` (at least `LEAST`).
fn room(limit: usize, [columns, meshes, distant]: [usize; 3], kind: Kind) -> bool {
    // A third, rounded down, and at least one.
    let third = (limit / 3).max(1);
    // Generation's share, however little of it it uses.
    let others = meshes + distant + third.max(columns);
    match kind {
        Kind::Columns => columns + meshes + distant < limit,
        Kind::Meshes => others < limit,
        // Leaving meshing one besides.
        Kind::DistantLand => distant < third.min(limit - third - 1) && others < limit,
    }
}

/// A streaming job's room, given back when the job ends (dropped).
pub struct Slot(Kind);

impl Drop for Slot {
    fn drop(&mut self) {
        counts().running[self.0 as usize] -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes jobs of `kind` while there is room, on top of `running`.
    fn fill(limit: usize, running: &mut [usize; 3], kind: Kind) -> usize {
        let mut taken = 0;
        while room(limit, *running, kind) {
            running[kind as usize] += 1;
            taken += 1;
        }
        taken
    }

    /// At every limit, each kind has room when the others are full: generation its third
    /// (rounded down), meshing one at least, distant land a third at most; and the limit
    /// holds.
    #[test]
    fn each_kind_has_its_share_at_every_limit() {
        for limit in LEAST..=12 {
            let third = (limit / 3).max(1);
            // Distant land first, then meshing, then generation.
            let mut running = [0; 3];
            let distant = fill(limit, &mut running, Kind::DistantLand);
            let meshes = fill(limit, &mut running, Kind::Meshes);
            let columns = fill(limit, &mut running, Kind::Columns);
            assert!((1..=third).contains(&distant), "limit {limit}: {distant} distant");
            assert!(meshes >= 1, "limit {limit}: meshing starved");
            assert!(columns >= third, "limit {limit}: generation's share");
            assert_eq!(distant + meshes + columns, limit, "limit {limit}: all of it, no more");
            // Meshing first: it leaves generation its share, and distant land none.
            let mut running = [0; 3];
            let meshes = fill(limit, &mut running, Kind::Meshes);
            assert_eq!(meshes, limit - third, "limit {limit}");
            assert_eq!(fill(limit, &mut running, Kind::DistantLand), 0, "limit {limit}");
            assert_eq!(fill(limit, &mut running, Kind::Columns), third, "limit {limit}");
            // Generation may take everything.
            assert_eq!(fill(limit, &mut [0; 3], Kind::Columns), limit, "limit {limit}");
        }
    }

    /// One test for the process's own limit: at least `LEAST`, none at 0, and none while
    /// any server says land around a player is missing, until each has taken it back.
    #[test]
    fn the_limit_is_at_least_the_least_and_lapses_while_land_is_missing() {
        for asked in 1..=5 {
            set_limit(asked);
            let slots: Vec<_> = std::iter::from_fn(|| start(Kind::Columns)).take(50).collect();
            assert_eq!(slots.len(), asked.max(LEAST), "asked {asked}");
            assert!(start(Kind::Meshes).is_none(), "asked {asked}: full");
            assert_eq!(limit(), asked.max(LEAST), "asked {asked}");
            let (mut one, mut other) = (LandMissing::default(), LandMissing::default());
            one.set(true);
            other.set(true);
            other.set(true);
            assert_eq!(limit(), 0, "asked {asked}: loading again");
            assert!(start(Kind::Meshes).is_some(), "asked {asked}: loading again");
            one.set(false);
            assert_eq!(limit(), 0, "asked {asked}: the other's land still missing");
            drop(other);
            assert_eq!(limit(), asked.max(LEAST), "asked {asked}: taken back with its server");
        }
        set_limit(0);
        assert_eq!(limit(), 0);
        let unlimited: Vec<_> = (0..20).map_while(|_| start(Kind::DistantLand)).collect();
        assert_eq!(unlimited.len(), 20, "no limit");
    }
}
