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
//! cap is lifted on leaving the world, so the next one loads on every worker.
//!
//! `VOXMAR_WORKERS=n` sets the workers outright, loading and playing alike (for timing how
//! many a busy machine's frames can take, tools/perf/play_frames).

use bevy::prelude::*;

use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::world::ClientWorld;

/// Most streaming jobs at once while playing.
pub const PLAYING: usize = 6;

/// Seconds the land must have stopped arriving, all of it meshed, for the world to count
/// as loaded: one quiet moment is not enough while a busy machine generates it in bursts.
const LOADED_FOR: f32 = 2.0;

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

/// Caps streaming once the player's world has loaded: joined, no column arriving for
/// `LOADED_FOR`, and every column received that can be meshed meshed.
fn cap_once_loaded(mut cap: ResMut<Cap>, time: Res<Time<Real>>, player: Res<LocalPlayer>, world: Res<ClientWorld>) {
    if cap.capped {
        return;
    }
    let now = time.elapsed_secs();
    let columns = world.voxels.len();
    if !(player.joined && columns >= 9 && world.caught_up()) {
        cap.quiet = None;
        return;
    }
    match cap.quiet {
        Some((seen, since)) if seen == columns => {
            if now - since < LOADED_FOR {
                return;
            }
        }
        _ => {
            cap.quiet = Some((columns, now));
            return;
        }
    }
    cap.capped = true;
    voxmar_engine::jobs::set_limit(cap.playing);
    voxmar_worldgen::set_helper_limit(cap.playing);
    info!("streaming on {} of {} workers while playing, from {columns} columns in", cap.playing, cap.all);
}

/// Streams on every worker again on leaving the world, so the next one loads on all of them.
fn lift(mut cap: ResMut<Cap>) {
    cap.quiet = None;
    if !cap.capped {
        return;
    }
    cap.capped = false;
    voxmar_engine::jobs::set_limit(0);
    voxmar_worldgen::set_helper_limit(cap.all);
}
