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
//! (blocks broken), the machine's load average over the last minute (`load_average`: well
//! above the core count, everything waits for a core, the game too), how the render thread
//! spent its last few frames (`render_thread_ms`, see `render::frame_timing`), and
//! `render`: what the renderer adds through [`SlowFrameExtra`].
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
use bevy::diagnostic::FrameCount;
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

/// The system's load average over the last minute: how many threads wanted a core. Well
/// above the core count, every thread waits its turn, the game's too, and frames slow down
/// with nothing in the game to blame (other work on the machine, say a build).
pub(crate) fn load_average() -> Option<f64> {
    let mut loads = [0.0f64; 3];
    // SAFETY: getloadavg writes at most `nelem` doubles into the buffer it is given.
    let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    (n >= 1).then_some(loads[0])
}

/// How long this process's threads, together, have waited for a core since it started:
/// ready to run, with every core busy with other work. On a calm machine it barely moves; on
/// one crowded past its cores it grows by up to as many seconds a second as our threads
/// wanted to run, the time something else ran instead of us. `None` where the system does not
/// say (and on macOS until our CPU time is enough to tell the unit its counters use).
#[cfg(target_os = "linux")]
pub(crate) fn waited_for_cores() -> Option<std::time::Duration> {
    // Each thread's `schedstat`: nanoseconds run, nanoseconds waited on a run queue, slices.
    let mut waited = 0u64;
    for task in std::fs::read_dir("/proc/self/task").ok()? {
        let Ok(task) = task else { continue };
        // A thread that ended since the listing is passed over: the sum then drops, and a
        // drop counts as no waiting.
        let Ok(stat) = std::fs::read_to_string(task.path().join("schedstat")) else { continue };
        waited += stat.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    }
    Some(std::time::Duration::from_nanos(waited))
}

/// macOS keeps it per process (`ri_runnable_time`, macOS 12 and later), in the unit of the
/// task's other counters: mach ticks on Apple silicon, nanoseconds on Intel. That counter is
/// the threads' time runnable, running or queued (XNU starts each thread's runnable timer when
/// it is made runnable and stops it only when it blocks), so the waiting is what is left of it
/// once the CPU time (`ri_user_time` and `ri_system_time`) is taken off (`queued`). Which unit is
/// told once, by setting the same counters' CPU time beside `getrusage`'s, in microseconds:
/// the unit (nanoseconds or ticks) that makes them agree within a half; neither, and nothing
/// is said.
#[cfg(target_os = "macos")]
// libc marks its mach calls deprecated in favour of the mach2 crate; the one used here
// (`mach_timebase_info`) is stable on every macOS, and no crate is added for it.
#[allow(deprecated)]
pub(crate) fn waited_for_cores() -> Option<std::time::Duration> {
    use std::sync::OnceLock;
    static NANOS_PER_UNIT: OnceLock<Option<f64>> = OnceLock::new();
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: proc_pid_rusage fills at most a `rusage_info_v4` for this flavour, into the
    // buffer it is given; it is read only when the call says it succeeded.
    let info = unsafe {
        if libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) != 0 {
            return None;
        }
        info.assume_init()
    };
    let scale = match NANOS_PER_UNIT.get() {
        Some(scale) => *scale,
        None => {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
            // SAFETY: getrusage fills the `rusage` it is given, read only on success.
            let usage = unsafe {
                if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
                    return None;
                }
                usage.assume_init()
            };
            let micros = |t: libc::timeval| t.tv_sec as f64 * 1e6 + t.tv_usec as f64;
            let cpu_ns = (micros(usage.ru_utime) + micros(usage.ru_stime)) * 1e3;
            let units = (info.ri_user_time + info.ri_system_time) as f64;
            // Too little CPU yet to tell nanoseconds from ticks by: asked again later.
            if cpu_ns < 1e9 || units == 0.0 {
                return None;
            }
            let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
            // SAFETY: mach_timebase_info fills the struct it is given.
            let tick = if unsafe { libc::mach_timebase_info(&mut timebase) } == 0 && timebase.denom > 0 { timebase.numer as f64 / timebase.denom as f64 } else { 1.0 };
            let measured = cpu_ns / units;
            let scale = [1.0, tick].into_iter().find(|unit| (measured / unit - 1.0).abs() < 0.5);
            *NANOS_PER_UNIT.get_or_init(|| scale)
        }
    };
    let waited = queued(info.ri_runnable_time, info.ri_user_time, info.ri_system_time);
    Some(std::time::Duration::from_nanos((waited as f64 * scale?) as u64))
}

/// Of the time a process's threads were runnable, what they spent queued for a core rather
/// than running: their CPU time taken off, in the same unit. A busy thread nothing held back
/// waited nothing; counters read a moment apart can put the CPU time a little past it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn queued(runnable: u64, user: u64, system: u64) -> u64 {
    runnable.saturating_sub(user.saturating_add(system))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn waited_for_cores() -> Option<std::time::Duration> {
    None
}

/// How much core time the whole machine has spent busy (every core's user, nice and system
/// time, and on Linux what the host took for itself), and how much CPU time this process has
/// had, since the machine and the process started. Their difference over a frame is the core
/// time other work on the machine ran (`waited_for_others`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CoreTimes {
    pub machine_busy: std::time::Duration,
    pub ours: std::time::Duration,
}

pub(crate) fn core_times() -> Option<CoreTimes> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the `rusage` it is given, read only on success.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return None;
        }
        usage.assume_init()
    };
    let micros = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
    let ours = std::time::Duration::from_micros(micros(usage.ru_utime) + micros(usage.ru_stime));
    // SAFETY: sysconf only reads a configuration value.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return None;
    }
    let machine_busy = std::time::Duration::from_secs_f64(machine_busy_ticks()? as f64 / hz as f64);
    Some(CoreTimes { machine_busy, ours })
}

/// The machine's busy core time in clock ticks: `/proc/stat`'s first line, every field but
/// idle and waiting on the disk (guest time is counted in user time already).
#[cfg(target_os = "linux")]
fn machine_busy_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let fields: Vec<u64> = stat.lines().next()?.split_whitespace().skip(1).take(8).map(|f| f.parse().ok()).collect::<Option<_>>()?;
    let [user, nice, system, _idle, _iowait, irq, softirq, steal] = fields[..] else { return None };
    Some(user + nice + system + irq + softirq + steal)
}

/// The machine's busy core time in clock ticks, summed over its cores: the host's CPU load
/// (`HOST_CPU_LOAD_INFO`), every state but idle.
#[cfg(target_os = "macos")]
// libc marks its mach calls deprecated in favour of the mach2 crate; these two
// (`mach_host_self`, `host_statistics`) are stable on every macOS, and no crate is added.
#[allow(deprecated)]
fn machine_busy_ticks() -> Option<u64> {
    let mut info = libc::host_cpu_load_info { cpu_ticks: [0; libc::CPU_STATE_MAX as usize] };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: host_statistics writes at most `count` integers, the size of `info`, into it.
    let got = unsafe { libc::host_statistics(libc::mach_host_self(), libc::HOST_CPU_LOAD_INFO, (&raw mut info).cast(), &mut count) };
    if got != libc::KERN_SUCCESS {
        return None;
    }
    let ticks = |state: libc::c_int| u64::from(info.cpu_ticks[state as usize]);
    Some(ticks(libc::CPU_STATE_USER) + ticks(libc::CPU_STATE_SYSTEM) + ticks(libc::CPU_STATE_NICE))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn machine_busy_ticks() -> Option<u64> {
    None
}

/// Of the `waited` seconds our threads spent queued for a core in a frame of `wall` seconds,
/// the share other work on the machine caused, given the CPU seconds we had (`ours`) and the
/// core seconds the machine was busy (`busy`) on `cores` cores in it. With the machine to
/// ourselves we would have waited only for as much as we asked past every core; the rest of
/// the waiting is set down to others, and never more than they ran. Our own pools crowding the
/// cores (generation, meshing, the server) are no excuse: on a machine with nothing else to
/// run, other work ran nothing and nothing is set down to it.
pub(crate) fn waited_for_others(wall: f32, waited: f32, ours: f32, busy: f32, cores: f32) -> f32 {
    let others = (busy - ours).max(0.0);
    let ourselves = (ours + waited - cores * wall).max(0.0);
    (waited - ourselves).min(others).clamp(0.0, waited.max(0.0))
}

/// Milliseconds between two marks, if both were taken.
fn between(a: Option<Instant>, b: Option<Instant>) -> Option<f32> {
    Some(b?.saturating_duration_since(a?).as_secs_f32() * 1000.0)
}

#[allow(clippy::too_many_arguments)]
fn record(
    count: Res<FrameCount>,
    time: Res<Time<Real>>,
    mut frames: ResMut<SlowFrames>,
    mut extra: ResMut<SlowFrameExtra>,
    mut broken: MessageReader<BlockBroken>,
    screen: Res<State<Screen>>,
    player: Res<LocalPlayer>,
    (world, lod, settings, weather): (Res<ClientWorld>, Option<Res<Lod>>, Res<Settings>, Res<ClientWeather>),
    spans: Option<Res<crate::render::frame_timing::PostUpdateSpans>>,
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
    let mut line = json!({
        "t": seconds,
        "main_frame": count.0,
        "frame_ms": frame_ms,
        "median_ms": median,
        "loading": now < frames.settling_until,
        "screen": format!("{:?}", screen.get()),
        "stages_ms": {
            "first": stages[0], "pre_update": stages[1], "update": stages[2], "post_update": stages[3], "last": stages[4],
        },
        "outside_ms": (frame_ms - main_ms).max(0.0),
        "render_thread_ms": crate::render::frame_timing::recent(),
        "post_update_ms": spans.map_or(Value::Null, |spans| spans.json()),
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
        "load_average": load_average(),
    });
    if let Some(end) = crate::render::work::timestamp(end) {
        line["main_interval_monotonic"] = json!({
            "previous_end": crate::render::work::timestamp(last_end),
            "begun": start.and_then(crate::render::work::timestamp), "ended": end,
        });
    }
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn waiting_for_cores_is_told() {
        // macOS tells its unit after a second of our CPU, which the other tests may have
        // spent already: spun for until then, two seconds at most.
        let start = Instant::now();
        let mut spin = 0u64;
        while waited_for_cores().is_none() && start.elapsed().as_secs_f32() < 2.0 {
            for _ in 0..100_000 {
                spin = std::hint::black_box(spin.wrapping_add(1));
            }
        }
        assert!(waited_for_cores().is_some(), "the system tells how long we waited for a core");
    }

    /// macOS's runnable time counts running as well as queueing: a frame its threads were
    /// busy for all along, held back by nothing, waited for no core, and only what was queued
    /// beyond the running counts.
    #[test]
    fn running_is_not_waiting() {
        assert_eq!(queued(1_000, 700, 300), 0);
        assert_eq!(queued(3_000, 700, 300), 2_000);
        assert_eq!(queued(990, 700, 300), 0);
    }

    #[test]
    fn stages_are_timed_between_marks() {
        let a = Instant::now();
        let b = a + std::time::Duration::from_millis(3);
        assert_eq!(between(Some(a), Some(b)), Some(3.0));
        assert_eq!(between(None, Some(b)), None);
    }
}
