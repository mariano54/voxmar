//! How many of the streaming workers (the global rayon pool, see `main`) take work: all of
//! them while a world loads, before the player is in it and frames matter, and at most
//! `PLAYING` once its land has first come in. On the 18-core Mac, busy with other work as it
//! usually is, a 40 s flight's p99 frame was 24 ms with 16 workers (and with 12), 10-15 ms
//! with 8 and 6 ms with 4, at the same frame median; but the gate's world took 49 s to load
//! with 16, and loading is what more workers are for.
//!
//! A pool's size is fixed once built, so the workers beyond the cap are parked: each takes a
//! job that sleeps until the cap is lifted (leaving the world). Work already queued runs
//! first, so the cap takes hold as the loading's last jobs finish. Meshing dispatches for
//! the workers that take work (`active`), and generation's region builds borrow no more
//! helper threads than that.
//!
//! `VOXMAR_WORKERS=n` sets the workers outright, loading and playing alike (for timing how
//! many a busy machine's frames can take, tools/perf/play_frames).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use bevy::prelude::*;

use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::world::ClientWorld;

/// Most workers that take work while playing.
pub const PLAYING: usize = 6;

/// Workers taking work now; 0 for every one in the pool (before `WorkersPlugin`, and in tests).
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Workers that take work now.
pub fn active() -> usize {
    match ACTIVE.load(Ordering::Relaxed) {
        0 => rayon::current_num_threads(),
        n => n,
    }
}

/// The parked workers' wake-up: a count of lifts, which each parked job waits to change.
static LIFTS: Mutex<u64> = Mutex::new(0);
static LIFTED: Condvar = Condvar::new();

/// The pool's workers, and how many of them take work while playing.
pub struct WorkersPlugin {
    pub all: usize,
    pub playing: usize,
}

impl Plugin for WorkersPlugin {
    fn build(&self, app: &mut App) {
        ACTIVE.store(self.all, Ordering::Relaxed);
        if self.playing < self.all {
            app.insert_resource(Cap { all: self.all, playing: self.playing, capped: false })
                .add_systems(Update, cap_once_loaded)
                .add_systems(OnExit(Screen::InGame), lift);
        }
    }
}

#[derive(Resource)]
struct Cap {
    all: usize,
    playing: usize,
    capped: bool,
}

/// Parks the workers beyond the cap once the player's world has first loaded: joined, every
/// column in view meshed and nothing left meshing (as the gate's `load` waits for).
fn cap_once_loaded(mut cap: ResMut<Cap>, screen: Res<State<Screen>>, player: Res<LocalPlayer>, world: Res<ClientWorld>) {
    let loaded = *screen.get() == Screen::InGame
        && player.joined
        && world.voxels.len() >= 9
        && world.stats.pending_columns == 0
        && world.stats.jobs_in_flight == 0;
    if cap.capped || !loaded {
        return;
    }
    cap.capped = true;
    let lifts = *LIFTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for _ in cap.playing..cap.all {
        rayon::spawn(move || {
            let mut now = LIFTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            while *now == lifts {
                now = LIFTED.wait(now).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        });
    }
    ACTIVE.store(cap.playing, Ordering::Relaxed);
    voxmar_worldgen::set_helper_limit(cap.playing);
    info!("streaming on {} of {} workers while playing", cap.playing, cap.all);
}

/// Wakes the parked workers on leaving the world, so the next one loads on all of them.
fn lift(mut cap: ResMut<Cap>) {
    if !cap.capped {
        return;
    }
    cap.capped = false;
    *LIFTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
    LIFTED.notify_all();
    ACTIVE.store(cap.all, Ordering::Relaxed);
    voxmar_worldgen::set_helper_limit(cap.all);
}
