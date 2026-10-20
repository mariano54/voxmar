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

/// What the cap sees of the world this frame.
struct Seen<B: FnOnce() -> bool> {
    joined: bool,
    /// Columns received.
    columns: usize,
    /// `ClientWorld::caught_up`.
    caught_up: bool,
    /// The distant land in.
    distant: bool,
    /// The columns within `NEAR` of the player's come.
    near: bool,
    /// `ClientWorld::built_in_view`, asked only once the rest holds.
    built: B,
}

#[derive(Debug, PartialEq, Eq)]
enum Change {
    Cap,
    Lift,
}

impl Cap {
    /// Whether to cap streaming now, or lift the cap: capped once the world has loaded
    /// (joined, the land around the player and the distant land in, everything come meshed,
    /// no more arriving for `LOADED_FOR`, and the land in view all built, or no more arriving
    /// for `SHORTER_VIEW_FOR` from a server that sends less), lifted while the land around
    /// the player is missing. A region build that holds up the next rings for a while leaves
    /// columns in view missing, so the cap waits for them.
    fn judge(&mut self, now: f32, seen: Seen<impl FnOnce() -> bool>) -> Option<Change> {
        if self.capped {
            return (!seen.near).then_some(Change::Lift);
        }
        if !(seen.joined && seen.columns >= 9 && seen.caught_up && seen.distant && seen.near) {
            self.quiet = None;
            return None;
        }
        let since = match self.quiet {
            Some((columns, since)) if columns == seen.columns => since,
            _ => {
                self.quiet = Some((seen.columns, now));
                return None;
            }
        };
        let quiet = now - since;
        (quiet >= LOADED_FOR && (quiet >= SHORTER_VIEW_FOR || (seen.built)())).then_some(Change::Cap)
    }
}

fn cap_once_loaded(
    mut cap: ResMut<Cap>,
    time: Res<Time<Real>>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    lod: Option<Res<crate::render::lod::Lod>>,
) {
    let center = ColumnPos::of_block(player.position.floor().as_ivec3());
    let seen = Seen {
        joined: player.joined,
        columns: world.voxels.len(),
        caught_up: world.caught_up(),
        distant: lod.is_none_or(|lod| {
            let (tiles, wanted) = lod.progress();
            tiles >= wanted
        }),
        near: world.all_in(center, NEAR),
        built: || world.built_in_view(center),
    };
    match cap.judge(time.elapsed_secs(), seen) {
        Some(Change::Cap) => {
            cap.capped = true;
            voxmar_engine::jobs::set_limit(cap.playing);
            voxmar_worldgen::set_helper_limit(cap.playing);
            info!("streaming on {} of {} workers while playing, from {} columns in", cap.playing, cap.all, world.voxels.len());
        }
        Some(Change::Lift) => {
            uncap(&mut cap);
            info!("streaming on all {} workers until the land around the player has come", cap.all);
        }
        None => {}
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(columns: usize, near: bool, built: bool) -> Seen<impl FnOnce() -> bool> {
        Seen { joined: true, columns, caught_up: true, distant: true, near, built: move || built }
    }

    fn cap() -> Cap {
        Cap { all: 16, playing: 6, capped: false, quiet: None }
    }

    /// Capped once the land in view is built and nothing has come for a while; lifted when
    /// the land around the player goes missing; capped again once it has loaded again.
    #[test]
    fn caps_once_loaded_lifts_when_the_land_around_goes_and_caps_again() {
        let mut cap = cap();
        assert_eq!(cap.judge(0.0, seen(100, true, true)), None, "just come");
        assert_eq!(cap.judge(1.0, seen(100, true, true)), None);
        assert_eq!(cap.judge(1.5, seen(120, true, true)), None, "more came: quiet again from here");
        assert_eq!(cap.judge(3.0, seen(120, true, true)), None);
        assert_eq!(cap.judge(3.6, seen(120, true, true)), Some(Change::Cap));
        cap.capped = true;
        assert_eq!(cap.judge(4.0, seen(130, true, false)), None, "streaming on while flying");
        assert_eq!(cap.judge(5.0, seen(130, false, false)), Some(Change::Lift), "a teleport beyond the view");
        // As `uncap` leaves it, without the process's limits, which other tests share.
        (cap.capped, cap.quiet) = (false, None);
        assert_eq!(cap.judge(6.0, seen(140, true, true)), None);
        assert_eq!(cap.judge(8.5, seen(140, true, true)), Some(Change::Cap), "loaded again");
    }

    /// A server that sends less than the view needs no column ever in view: capped after
    /// `SHORTER_VIEW_FOR`, and only with the land around the player in.
    #[test]
    fn a_shorter_view_caps_late_and_only_with_the_land_around_in() {
        let mut cap = cap();
        assert_eq!(cap.judge(0.0, seen(100, true, false)), None);
        assert_eq!(cap.judge(LOADED_FOR + 1.0, seen(100, true, false)), None, "the view not built");
        assert_eq!(cap.judge(SHORTER_VIEW_FOR + 1.0, seen(100, false, false)), None, "a stall with the land around missing");
        assert_eq!(cap.judge(SHORTER_VIEW_FOR + 2.0, seen(100, true, false)), None, "quiet again from here");
        assert_eq!(cap.judge(2.0 * SHORTER_VIEW_FOR + 3.0, seen(100, true, false)), Some(Change::Cap));
    }

    #[test]
    fn nothing_capped_before_joining_or_while_meshing() {
        let mut cap = cap();
        for t in 0..30 {
            let now = t as f32;
            assert_eq!(cap.judge(now, Seen { joined: false, ..seen(100, true, true) }), None);
        }
        for t in 30..60 {
            assert_eq!(cap.judge(t as f32, Seen { caught_up: false, ..seen(100, true, true) }), None);
        }
        for t in 60..90 {
            assert_eq!(cap.judge(t as f32, Seen { distant: false, ..seen(100, true, true) }), None);
        }
    }
}
