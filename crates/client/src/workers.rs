//! How many of the streaming workers (the global rayon pool, see `main`) stream: all of them
//! while a world loads, before the player is in it and frames matter, and at most `PLAYING`
//! once its land has come in. On the 18-core Mac, busy with other work as it usually is, a
//! 40 s flight's p99 frame was 24 ms with 16 workers (and with 12), 10-15 ms with 8 and 6 ms
//! with 4, at the same frame median; but the gate's world took 49 s to load with 16, and
//! loading is what more workers are for.
//!
//! The pool keeps its size; what is capped is how many streaming jobs (generating, meshing,
//! distant land) run at once (`voxmar_engine::jobs`), and the helper threads generation's
//! region builds borrow. No worker is ever held: a job with no room is not started yet. The
//! cap is lifted on leaving the world, so the next one loads on every worker, and whenever
//! the land around the player is missing (a teleport beyond the view, or streaming fallen
//! that far behind), which is loading again, until it has all come.
//!
//! `VOXMAR_WORKERS=n` sets the workers outright, loading and playing alike (for timing how
//! many a busy machine's frames can take, tools/perf/play_frames).

use bevy::prelude::*;
use voxmar_engine::coords::ColumnPos;

use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::world::ClientWorld;

/// Most streaming jobs at once while playing.
pub const PLAYING: usize = 6;

/// Columns on each side of the player's that must have come while capped.
const NEAR: i32 = 2;

/// Seconds the land in view must have all come, and been meshed, with no more arriving, for
/// the world to count as loaded: one quiet moment is not enough while a busy machine
/// generates it in bursts.
const LOADED_FOR: f32 = 2.0;
/// Seconds with no land arriving, all of it meshed, after which the world counts as loaded
/// even with columns in view missing: a server whose view distance is shorter than the
/// player's never sends them.
const SHORTER_VIEW_FOR: f32 = 10.0;

/// The pool's workers, and how many of them stream while playing.
pub struct WorkersPlugin {
    pub all: usize,
    pub playing: usize,
}

impl Plugin for WorkersPlugin {
    fn build(&self, app: &mut App) {
        if self.playing < self.all {
            app.insert_resource(Cap { all: self.all, playing: self.playing, capped: false, quiet: None })
                .add_systems(Update, cap_once_loaded.run_if(in_state(Screen::InGame)))
                .add_systems(OnExit(Screen::InGame), lift);
        }
    }
}

#[derive(Resource)]
struct Cap {
    all: usize,
    playing: usize,
    capped: bool,
    /// Columns received, and since when no more have come with everything meshed.
    quiet: Option<(usize, f32)>,
}

/// Caps streaming once the player's world has loaded: joined, the land in view all built
/// (`ClientWorld::built_in_view`) and the distant land too, and no more arriving for
/// `LOADED_FOR` (or for `SHORTER_VIEW_FOR`, everything that came meshed, with columns in
/// view still to come). A region build that holds up the next rings for a while leaves
/// columns in view missing, so the cap waits for them. Lifts it while the land around the
/// player is missing.
fn cap_once_loaded(
    mut cap: ResMut<Cap>,
    time: Res<Time<Real>>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    lod: Option<Res<crate::render::lod::Lod>>,
) {
    let center = ColumnPos::of_block(player.position.floor().as_ivec3());
    if cap.capped {
        if !world.all_in(center, NEAR) {
            uncap(&mut cap);
            info!("streaming on all {} workers until the land around the player has come", cap.all);
        }
        return;
    }
    let now = time.elapsed_secs();
    let columns = world.voxels.len();
    let distant = lod.is_none_or(|lod| {
        let (tiles, wanted) = lod.progress();
        tiles >= wanted
    });
    if !(player.joined && columns >= 9 && world.caught_up() && distant) {
        cap.quiet = None;
        return;
    }
    let since = match cap.quiet {
        Some((seen, since)) if seen == columns => since,
        _ => {
            cap.quiet = Some((columns, now));
            return;
        }
    };
    let quiet = now - since;
    if quiet < LOADED_FOR || (quiet < SHORTER_VIEW_FOR && !world.built_in_view(center)) {
        return;
    }
    cap.capped = true;
    voxmar_engine::jobs::set_limit(cap.playing);
    voxmar_worldgen::set_helper_limit(cap.playing);
    info!("streaming on {} of {} workers while playing, from {columns} columns in", cap.playing, cap.all);
}

/// Streams on every worker again on leaving the world, so the next one loads on all of them.
fn lift(mut cap: ResMut<Cap>) {
    uncap(&mut cap);
}

fn uncap(cap: &mut Cap) {
    cap.quiet = None;
    if !cap.capped {
        return;
    }
    cap.capped = false;
    voxmar_engine::jobs::set_limit(0);
    voxmar_worldgen::set_helper_limit(cap.all);
}
