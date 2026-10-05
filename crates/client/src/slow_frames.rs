//! A record of slow frames on disk, for finding out later what made the game hitch while
//! someone played: `slow-frames.jsonl` in the configuration directory (see
//! `settings::config_dir`), one JSON object a line for every frame over
//! max(`SLOW_MS`, `SLOW_FACTOR` × the median of the last `RECENT` frames).
//!
//! Each line has when (`t`, UTC), how long the frame and its parts took (`frame_ms`,
//! `median_ms`; `stages_ms` for the main world's First, PreUpdate, Update (with state
//! transitions and fixed steps), PostUpdate and Last, and `outside_ms` for the rest:
//! waiting for the render thread, extraction, the window's events), what the client was
//! busy with (meshing jobs, columns waiting, quads handed to the renderer, distant tiles),
//! where the player was and what they had set, the weather, what happened that frame
//! (blocks broken), and `render`: what the renderer adds through [`SlowFrameExtra`].
//! Frames while the world loads, or just after a teleport, are marked `loading`.
//!
//! While frames are fine this costs six `Instant`s and a ring of frame times; nothing is
//! written until a frame is slow. The file is rotated to `slow-frames.old.jsonl` at
//! `MAX_BYTES`. Captures and gate runs keep no record.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use serde_json::{Value, json};

use crate::breaking::BlockBroken;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::lod::Lod;
use crate::settings::Settings;
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

/// A frame slower than this is always recorded.
const SLOW_MS: f32 = 25.0;
/// A frame this many times slower than the recent median is recorded too.
const SLOW_FACTOR: f32 = 3.0;
/// Frames the median is taken over.
const RECENT: usize = 120;
/// Seconds after joining or a teleport during which slow frames are marked `loading`.
const SETTLING: f32 = 2.0;
/// Size at which the record starts afresh, the last one kept as `.old`.
const MAX_BYTES: u64 = 5 << 20;

pub struct SlowFramesPlugin;

/// What the renderer adds to a slow frame's record: set it every frame (or only when
/// there is something to say); it is written with the frame if the frame is slow, and
/// cleared after every frame either way. Always present, recording or not (main.rs).
#[derive(Resource, Default)]
pub struct SlowFrameExtra {
    pub render: Option<Value>,
}

impl Plugin for SlowFramesPlugin {
    fn build(&self, app: &mut App) {
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        order.insert_before(First, FrameStart);
        order.insert_after(First, AfterFirst);
        order.insert_after(PreUpdate, AfterPreUpdate);
        order.insert_after(Update, AfterUpdate);
        order.insert_after(PostUpdate, AfterPostUpdate);
        order.insert_after(Last, FrameEnd);
        app.insert_resource(SlowFrames::default())
            .add_systems(FrameStart, |mut frames: ResMut<SlowFrames>| frames.mark(0))
            .add_systems(AfterFirst, |mut frames: ResMut<SlowFrames>| frames.mark(1))
            .add_systems(AfterPreUpdate, |mut frames: ResMut<SlowFrames>| frames.mark(2))
            .add_systems(AfterUpdate, |mut frames: ResMut<SlowFrames>| frames.mark(3))
            .add_systems(AfterPostUpdate, |mut frames: ResMut<SlowFrames>| frames.mark(4))
            .add_systems(FrameEnd, record);
    }
}

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct FrameStart;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterFirst;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterPreUpdate;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterUpdate;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterPostUpdate;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct FrameEnd;

#[derive(Resource, Default)]
struct SlowFrames {
    /// When each stage of this frame began, and when the last frame ended.
    marks: [Option<Instant>; 5],
    last_end: Option<Instant>,
    recent: VecDeque<f32>,
    /// Until when (seconds since startup) slow frames count as loading.
    settling_until: f32,
    last_position: Option<Vec3>,
    file: Option<File>,
    /// Set once the file could not be opened, so it is not tried every slow frame.
    failed: bool,
}

impl SlowFrames {
    fn mark(&mut self, stage: usize) {
        self.marks[stage] = Some(Instant::now());
    }

    /// The median of recent frames, in milliseconds.
    fn median(&self) -> f32 {
        let mut sorted: Vec<f32> = self.recent.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
    }

    fn write(&mut self, line: &Value) {
        if self.failed {
            return;
        }
        let Some(path) = path() else { return };
        if self.file.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() > MAX_BYTES) {
            self.file = None;
            let _ = std::fs::rename(&path, path.with_file_name("slow-frames.old.jsonl"));
        }
        if self.file.is_none() {
            let _ = path.parent().map(std::fs::create_dir_all);
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => {
                    info!("recording slow frames in {}", path.display());
                    self.file = Some(file);
                }
                Err(e) => {
                    warn!("cannot record slow frames in {}: {e}", path.display());
                    self.failed = true;
                    return;
                }
            }
        }
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(crate::settings::config_dir()?.join("slow-frames.jsonl"))
}

/// Milliseconds between two marks, if both were taken.
fn between(a: Option<Instant>, b: Option<Instant>) -> Option<f32> {
    Some(b?.saturating_duration_since(a?).as_secs_f32() * 1000.0)
}

#[allow(clippy::too_many_arguments)]
fn record(
    time: Res<Time<Real>>,
    mut frames: ResMut<SlowFrames>,
    mut extra: ResMut<SlowFrameExtra>,
    mut broken: MessageReader<BlockBroken>,
    screen: Res<State<Screen>>,
    player: Res<LocalPlayer>,
    (world, lod, settings, weather): (Res<ClientWorld>, Option<Res<Lod>>, Res<Settings>, Res<ClientWeather>),
) {
    let end = Instant::now();
    let frames = &mut *frames;
    let render = extra.render.take();
    let broken = broken.read().count();
    let Some(last_end) = frames.last_end.replace(end) else { return };
    let frame_ms = end.saturating_duration_since(last_end).as_secs_f32() * 1000.0;
    let now = time.elapsed_secs();
    // Joining, and a jump of more than a few blocks (a teleport, a respawn), load a new
    // area of the world; frames until it has settled are marked as loading.
    let jumped = frames.last_position.is_some_and(|p| p.distance(player.position) > 8.0);
    frames.last_position = Some(player.position);
    if !player.joined || jumped {
        frames.settling_until = now + SETTLING;
    }

    let median = frames.median();
    let slow = frames.recent.len() >= RECENT / 4 && frame_ms > SLOW_MS.max(SLOW_FACTOR * median);
    if frames.recent.len() == RECENT {
        frames.recent.pop_front();
    }
    frames.recent.push_back(frame_ms);
    if !slow {
        return;
    }

    let [start, first, pre_update, update, post_update] = frames.marks;
    let stages = [between(start, first), between(first, pre_update), between(pre_update, update), between(update, post_update), between(post_update, Some(end))];
    let main_ms = between(start, Some(end)).unwrap_or(0.0);
    let (tiles, wanted) = lod.map_or((0, 0), |lod| lod.progress());
    let stats = world.stats;
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let line = json!({
        "t": seconds,
        "frame_ms": frame_ms,
        "median_ms": median,
        "loading": now < frames.settling_until,
        "screen": format!("{:?}", screen.get()),
        "stages_ms": {
            "first": stages[0], "pre_update": stages[1], "update": stages[2], "post_update": stages[3], "last": stages[4],
        },
        "outside_ms": (frame_ms - main_ms).max(0.0),
        "world": {
            "columns": world.voxels.len(),
            "jobs_in_flight": stats.jobs_in_flight,
            "pending_columns": stats.pending_columns,
            "chunk_meshes": stats.chunk_entities,
            "visible_chunks": stats.visible_chunks,
            "quads": stats.quads,
            "lod_tiles": tiles,
            "lod_wanted": wanted,
        },
        "player": {
            "pos": [player.position.x, player.position.y, player.position.z],
            "yaw": player.yaw.to_degrees(),
            "pitch": player.pitch.to_degrees(),
            "flying": player.flying,
            "in_water": player.in_water,
        },
        "settings": {
            "view_distance": settings.view_distance,
            "fly_speed": settings.fly_speed,
            "resolution": format!("{:?}", settings.resolution),
            "pack": settings.shader_pack,
            "vsync": settings.vsync,
        },
        "weather": weather.current.summary(),
        "events": { "blocks_broken": broken },
        "render": render,
    });
    frames.write(&line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_is_of_recent_frames() {
        let mut frames = SlowFrames::default();
        frames.recent.extend([5.0, 50.0, 6.0, 7.0, 4.0]);
        assert_eq!(frames.median(), 6.0);
    }

    #[test]
    fn stages_are_timed_between_marks() {
        let a = Instant::now();
        let b = a + std::time::Duration::from_millis(3);
        assert_eq!(between(Some(a), Some(b)), Some(3.0));
        assert_eq!(between(None, Some(b)), None);
    }
}
