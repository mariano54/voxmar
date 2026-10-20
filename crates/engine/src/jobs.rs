//! How many streaming jobs (generating columns, meshing them, building distant land) run on
//! the shared workers at once. Unlimited unless a game client sets a limit
//! (`client::workers`): one that streams on fewer workers than the pool has, so the rest of
//! the machine's cores stay with its frames. Jobs are never held back once started; a job
//! that finds no room is not started, and its dispatcher tries again on a later frame or tick.
//!
//! Under a limit each kind has its share, since the client asks every frame and the server
//! only every tick: generation always has a third of the room (at least one job) that the
//! others may not take, and distant land, the least urgent, at most that much again.

use std::sync::Mutex;

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

struct Counts {
    /// Most streaming jobs at once; 0 for no limit.
    limit: usize,
    running: [usize; 3],
}

static COUNTS: Mutex<Counts> = Mutex::new(Counts { limit: 0, running: [0; 3] });

fn counts() -> std::sync::MutexGuard<'static, Counts> {
    COUNTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets the most streaming jobs at once (0 for no limit). Jobs already running go on.
pub fn set_limit(jobs: usize) {
    counts().limit = jobs;
}

/// Room for one more streaming job of `kind`, if there is any: held by the job while it runs.
#[must_use]
pub fn start(kind: Kind) -> Option<Slot> {
    let mut counts = counts();
    let [columns, meshes, distant] = counts.running;
    let limit = counts.limit;
    let reserved = limit.div_ceil(3);
    let room = limit == 0
        || match kind {
            Kind::Columns => columns + meshes + distant < limit,
            // Leaving generation its share, however little of it it uses.
            Kind::Meshes => meshes + distant + reserved.max(columns) < limit,
            Kind::DistantLand => distant < reserved && meshes + distant + reserved.max(columns) < limit,
        };
    room.then(|| {
        counts.running[kind as usize] += 1;
        Slot(kind)
    })
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

    /// One test for the limit, which is the whole process's.
    #[test]
    fn each_kind_has_its_share_of_the_limit() {
        set_limit(6);
        // Distant land takes no more than two, and meshing leaves generation two.
        let distant: Vec<_> = std::iter::from_fn(|| start(Kind::DistantLand)).collect();
        assert_eq!(distant.len(), 2);
        let meshes: Vec<_> = std::iter::from_fn(|| start(Kind::Meshes)).collect();
        assert_eq!(meshes.len(), 2, "the rest but generation's two");
        let columns: Vec<_> = std::iter::from_fn(|| start(Kind::Columns)).collect();
        assert_eq!(columns.len(), 2, "generation's own");
        drop(distant);
        // Generation may take more than its share when the others leave room.
        let more: Vec<_> = std::iter::from_fn(|| start(Kind::Columns)).collect();
        assert_eq!(more.len(), 2);
        assert!(start(Kind::Meshes).is_none(), "full");
        drop((meshes, columns, more));
        set_limit(0);
        let unlimited: Vec<_> = (0..20).map_while(|_| start(Kind::DistantLand)).collect();
        assert_eq!(unlimited.len(), 20, "no limit");
    }
}
