//! `--capture PATH`: once the world around the player (or behind a menu screen) has loaded
//! and settled, logs frame statistics, saves a screenshot and exits. Used for visual checks
//! and benchmarking.
//! Runs without a window and renders into an offscreen image, so frame times measure the
//! engine rather than the display, and it works with the display asleep.
//!
//! `--flicker` measures shimmer instead: with the clock stopped, it takes the shot, steps
//! the camera 4 cm sideways, takes another, and reports the share of pixels that changed
//! by more than 60 levels (of 255) in any channel. Animation is frozen, so only aliasing
//! (geometry, texture and shading detail smaller than a pixel) moves pixels that much.
//! Changed pixels are marked in red in `OUT-flicker.png`. `--flicker-still` measures the
//! other kind: the camera stands still while the game runs on by exactly one 60 fps frame
//! between the shots (the sun moving, the wind blowing), which is how shadows and leaves
//! flicker as time passes.
//!
//! `--clip SECONDS` records motion instead: every frame of that much game time at 30 fps,
//! as `OUT-000.png` on, for checking how things move (wind, water, weather).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::time::TimeUpdateStrategy;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};

use voxmar_protocol::TICKS_PER_DAY;

use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::render::DayCycle;
use crate::render::lod::Lod;
use crate::world::ClientWorld;

/// Height `--flight` climbs to before flying forward, above all but the highest peaks.
const FLIGHT_HEIGHT: f32 = voxmar_worldgen::SEA_LEVEL as f32 + 106.0;

pub struct CapturePlugin {
    pub path: PathBuf,
    /// Optional camera override: offset from the player's eye, then yaw and pitch (degrees).
    pub pose: Option<[f32; 5]>,
    /// Seconds to fly forward after loading, measuring frame pacing while streaming.
    pub flight: Option<f32>,
    /// Extra seconds to let the world run before capturing, for weather to build.
    pub wait: f32,
    /// Fixed time of day, as a fraction of the day from sunrise.
    pub time: Option<f32>,
    /// Mine the block in the middle of the view this far (0..1); 1 breaks it just before
    /// the screenshot.
    pub mine: Option<f32>,
    /// Show the settings menu.
    pub menu: bool,
    /// Show the inventory screen.
    pub inventory: bool,
    /// Measure flicker for a small camera step or as time passes (see the module docs).
    pub flicker: Option<FlickerKind>,
    /// Seconds of game time to record frame by frame (see the module docs).
    pub clip: Option<f32>,
    /// Width and height of the shot, in pixels.
    pub size: UVec2,
}

/// What changes between the two shots of a flicker measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlickerKind {
    /// The camera steps 4 cm sideways with the clock stopped (`--flicker`).
    Step,
    /// The game runs on one 60 fps frame with the camera still (`--flicker-still`).
    Still,
}

/// How far `--flicker` steps the camera sideways, in blocks.
const FLICKER_STEP: f32 = 0.04;
/// A pixel flickers when a channel changes by more than this many levels.
const FLICKER_LEVELS: u8 = 60;
/// Frames to let a change of pose or clock reach the screen before a flicker shot.
const FLICKER_FRAMES: u32 = 3;
/// Game time between the shots of `--flicker-still`: one frame at 60 fps.
const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Frames per second of game time in a `--clip`.
const CLIP_RATE: u32 = 30;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(UiState { menu_open: self.menu, inventory_open: self.inventory, ..default() }).insert_resource(Capture {
            path: self.path.clone(),
            pose: self.pose,
            flight: self.flight,
            wait: self.wait,
            time: self.time,
            mine: self.mine,
            broken_at: None,
            target: Handle::default(),
            settled_at: None,
            flight_started: None,
            taken_at: None,
            saved: false,
            frame_times: Vec::new(),
            flicker: self.flicker.map(|kind| Flicker { kind, ..default() }),
            clip: self.clip.map(|seconds| Clip { frames: (seconds * CLIP_RATE as f32).ceil() as u32, ..default() }),
            size: self.size,
        })
        .add_systems(PostStartup, render_offscreen)
        .add_systems(Update, (target_ui, capture, measure_flicker.after(capture), record_clip.after(capture)))
        .add_systems(
            Update,
            mine.after(crate::interact::InteractSystems).before(crate::breaking::EffectSystems).run_if(in_state(Screen::InGame)),
        )
        .add_systems(
            PostUpdate,
            (apply_pose.run_if(in_state(Screen::InGame)), hold_time).before(TransformSystems::Propagate),
        );
    }
}

#[derive(Resource)]
struct Capture {
    path: PathBuf,
    pose: Option<[f32; 5]>,
    flight: Option<f32>,
    wait: f32,
    time: Option<f32>,
    mine: Option<f32>,
    /// When `--mine 1` broke its block, which the screenshot follows by a moment.
    broken_at: Option<f32>,
    target: Handle<Image>,
    settled_at: Option<f32>,
    flight_started: Option<f32>,
    taken_at: Option<f32>,
    /// Whether the screenshot has been written, which can take a while on a busy machine.
    saved: bool,
    frame_times: Vec<f32>,
    flicker: Option<Flicker>,
    clip: Option<Clip>,
    size: UVec2,
}

/// Progress of a `--clip`, which starts once the world has settled.
#[derive(Default)]
struct Clip {
    frames: u32,
    started: bool,
    /// Shots asked for, and shots written.
    taken: u32,
    saved: Arc<AtomicU32>,
}

/// Progress of a `--flicker` measurement, which starts once the world has settled.
struct Flicker {
    kind: FlickerKind,
    started: bool,
    /// For `--flicker-still`, when the game clock started stepping (seconds).
    since: Option<f64>,
    /// Frames since the clock stopped.
    frames: u32,
    /// The camera's sideways step, once the first shot is taken.
    step: Vec3,
    /// The two shots, as they arrive.
    shots: Arc<Mutex<Vec<Image>>>,
}

impl Default for Flicker {
    fn default() -> Self {
        Self { kind: FlickerKind::Step, started: false, since: None, frames: 0, step: Vec3::ZERO, shots: default() }
    }
}

fn render_offscreen(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    camera: Query<Entity, With<Camera3d>>,
) {
    let mut image = Image::new_target_texture(state.size.x, state.size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    state.target = images.add(image);
    for entity in &camera {
        commands.entity(entity).insert(RenderTarget::Image(state.target.clone().into()));
    }
}

type UntargetedUiRoot = (With<Node>, Without<ChildOf>, Without<UiTargetCamera>);

/// UI follows the window by default; point it at the offscreen camera instead.
fn target_ui(
    mut commands: Commands,
    camera: Query<Entity, With<Camera3d>>,
    roots: Query<Entity, UntargetedUiRoot>,
) {
    let Ok(camera) = camera.single() else { return };
    for root in &roots {
        commands.entity(root).insert(UiTargetCamera(camera));
    }
}

fn apply_pose(state: Res<Capture>, player: Res<LocalPlayer>, mut camera: Query<&mut Transform, With<Camera3d>>) {
    let (Some([x, y, z, yaw, pitch]), Ok(mut transform)) = (state.pose, camera.single_mut()) else { return };
    let step = state.flicker.as_ref().map_or(Vec3::ZERO, |f| f.step);
    transform.translation = player.eye() + Vec3::new(x, y, z) + step;
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), 0.0);
}

/// Seconds between a `--mine 1` break and the screenshot.
const BROKEN_FOR: f32 = 0.3;

/// Mines the solid block the camera looks at (through plants), in place of a held mouse
/// button, once the world has settled; a full break happens a moment before the screenshot
/// so its pieces are in the air, and takes a plant on top with it.
fn mine(
    time: Res<Time>,
    mut state: ResMut<Capture>,
    mut world: ResMut<ClientWorld>,
    mut target: ResMut<crate::interact::Target>,
    mut broken: MessageWriter<crate::breaking::BlockBroken>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let (Some(progress), Some(settled), Ok(camera)) = (state.mine, state.settled_at, camera.single()) else { return };
    let since = time.elapsed_secs() - settled;
    let reach = voxmar_engine::physics::player::REACH * 2.0;
    let hit = voxmar_engine::physics::raycast(camera.translation(), camera.forward().as_vec3(), reach, |p| {
        world.block(p).is_some_and(|b| world.registry.info(b).solid)
    });
    target.hit = hit;
    let Some(hit) = hit.filter(|_| state.broken_at.is_none() && since > 1.0) else {
        target.breaking = None;
        return;
    };
    let full = 4.0 + state.wait - BROKEN_FOR;
    if progress < 1.0 {
        target.breaking = Some((hit.block, progress));
    } else if since > full {
        state.broken_at = Some(time.elapsed_secs());
        target.breaking = None;
        for pos in [hit.block, hit.block + IVec3::Y] {
            let Some(block) = world.block(pos) else { continue };
            if pos == hit.block || world.registry.info(block).support == voxmar_engine::block::Support::Below {
                broken.write(crate::breaking::BlockBroken { pos, block });
                world.set_block(pos, voxmar_engine::block::BlockId::AIR);
            }
        }
    } else {
        target.breaking = Some((hit.block, (since / full).min(0.99)));
    }
}

/// Holds the time of day, or during `--flicker-still` runs it on from there.
fn hold_time(state: Res<Capture>, time: Res<Time>, mut cycle: ResMut<DayCycle>) {
    if let Some(day) = state.time {
        let since = state.flicker.as_ref().and_then(|f| f.since).map_or(0.0, |start| time.elapsed_secs_f64() - start);
        cycle.ticks = day as f64 * TICKS_PER_DAY as f64 + since * voxmar_protocol::TICKS_PER_SECOND as f64;
    }
}

/// Climbs straight up above the terrain, then flies forward at sprint-flight speed. Flying
/// through the ground would have the server send the player back.
fn fly(time: &Time, player: &mut LocalPlayer) {
    player.flying = true;
    let forward = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
    let direction = if player.position.y < FLIGHT_HEIGHT { Vec3::Y } else { forward };
    let speed = voxmar_engine::physics::player::FLY_SPRINT_SPEED * 0.9;
    player.position += direction * speed * time.delta_secs();
}

#[allow(clippy::too_many_arguments)]
fn capture(
    mut commands: Commands,
    time: Res<Time>,
    real: Res<Time<Real>>,
    mut state: ResMut<Capture>,
    world: Res<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    diagnostics: Res<DiagnosticsStore>,
    mut exit: MessageWriter<AppExit>,
    lod: Res<Lod>,
    screen: Res<State<Screen>>,
    (weather, air): (Res<crate::weather::ClientWeather>, Res<crate::weather::atmosphere::ClientAtmosphere>),
) {
    // Real time while `--flicker` stops the game's clock.
    let now = if state.flicker.is_some() { real.elapsed_secs() } else { time.elapsed_secs() };
    if let Some(taken) = state.taken_at {
        if state.saved || state.flicker.is_some() || state.clip.is_some() || now - taken > 30.0 {
            exit.write(AppExit::Success);
        }
        return;
    }
    if state.flicker.as_ref().is_some_and(|f| f.started) || state.clip.as_ref().is_some_and(|c| c.started) {
        return;
    }
    let stats = world.stats;
    let (lod_tiles, lod_wanted) = lod.progress();
    // Menus wait for their backdrop world, games for the player's.
    let joining = match screen.get() {
        Screen::Boot => true,
        Screen::InGame => !player.joined,
        _ => false,
    };
    let busy = joining
        || stats.jobs_in_flight > 0
        || stats.pending_columns > 0
        || world.voxels.len() < 9
        || lod_tiles < lod_wanted;
    let flying = state.flight_started.is_some();
    // A mined block's pieces are caught in the air, while its column remeshes.
    let broken = state.broken_at.map(|at| now - at);
    let patience = if state.flicker.is_some() { 300.0 } else { 60.0 };
    if busy && !flying && broken.is_none() && now < patience {
        state.settled_at = None;
        return;
    }
    // Measure a few seconds of steady-state rendering before capturing.
    let settled = *state.settled_at.get_or_insert(now);
    if broken.map_or(now - settled < 4.0 + state.wait, |since| since < BROKEN_FOR) {
        return;
    }
    if let Some(duration) = state.flight
        && now - *state.flight_started.get_or_insert(now) < duration
    {
        fly(&time, &mut player);
        state.frame_times.push(time.delta_secs() * 1000.0);
        return;
    }
    if !state.frame_times.is_empty() {
        let mut times = std::mem::take(&mut state.frame_times);
        times.sort_by(f32::total_cmp);
        let at = |q: f32| times[((times.len() - 1) as f32 * q) as usize];
        let median = at(0.5);
        let hitches = times.iter().filter(|&&t| t > median * 2.0).count();
        info!(
            "flight: {} frames, median {median:.2} ms, p99 {:.2} ms, max {:.2} ms, {hitches} frames over twice the median",
            times.len(),
            at(0.99),
            at(1.0),
        );
    }
    let fps = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.average()).unwrap_or(0.0);
    let frame = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.average()).unwrap_or(0.0);
    info!(
        "capture: {fps:.0} fps ({frame:.2} ms), {} columns, {} chunk meshes, {} quads ({} after cave culling), {} visible, {} jobs, {} pending, {lod_tiles}/{lod_wanted} distant tiles, loaded in {settled:.1}s",
        world.voxels.len(),
        stats.chunk_entities,
        stats.quads,
        stats.visible_quads,
        stats.visible_chunks,
        stats.jobs_in_flight,
        stats.pending_columns,
    );
    let shown = &weather.current;
    info!("weather: {}: {}; effects: {}", shown.describe(), shown.summary(), weather.effects.join(", "));
    let particles: Vec<String> = air.particles.iter().map(|p| format!("{} {:.0}%", p.name, p.density * 100.0)).collect();
    info!("air: haze {:.2} ×{:.2}, warmth {:.2}, saturation {:.2}; particles: {}", air.haze, air.haze_density, air.warmth, air.saturation, particles.join(", "));
    if let Some(flicker) = &mut state.flicker {
        // `measure_flicker` takes over.
        flicker.started = true;
        return;
    }
    if let Some(clip) = &mut state.clip {
        // `record_clip` takes over, and every frame from now on advances the game alike.
        clip.started = true;
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1) / CLIP_RATE));
        return;
    }
    commands
        .spawn(Screenshot::image(state.target.clone()))
        .observe(save_to_disk(state.path.clone()))
        .observe(|_: On<ScreenshotCaptured>, mut state: ResMut<Capture>| state.saved = true);
    state.taken_at = Some(now);
}

/// Runs a `--flicker` measurement once `capture` has started it: stops the clock, takes
/// a shot, steps the camera sideways, takes another and compares them.
fn measure_flicker(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    real: Res<Time<Real>>,
    mut virtual_time: ResMut<Time<Virtual>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let (target, path) = (state.target.clone(), state.path.clone());
    let Some(flicker) = state.flicker.as_mut().filter(|f| f.started) else { return };
    let kind = flicker.kind;
    if flicker.kind == FlickerKind::Step {
        virtual_time.pause();
    } else if flicker.since.is_none() {
        // From the next frame on, every frame advances the game by exactly one 60 fps step.
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
        flicker.since = Some(virtual_time.elapsed_secs_f64());
    }
    flicker.frames += 1;
    let shot = |name: PathBuf, commands: &mut Commands, shots: &Arc<Mutex<Vec<Image>>>| {
        let shots = shots.clone();
        commands
            .spawn(Screenshot::image(target.clone()))
            .observe(save_to_disk(name))
            .observe(move |captured: On<ScreenshotCaptured>| shots.lock().expect("flicker shots").push(captured.image.clone()));
    };
    // A step is taken with the clock stopped, a few frames after the first shot; time
    // passing is the very next frame.
    let second = match flicker.kind {
        FlickerKind::Step => 2 * FLICKER_FRAMES + 1,
        FlickerKind::Still => FLICKER_FRAMES + 1,
    };
    if flicker.frames == FLICKER_FRAMES {
        shot(path.clone(), &mut commands, &flicker.shots);
    } else if flicker.frames == FLICKER_FRAMES + 1 && flicker.kind == FlickerKind::Step {
        let Ok(camera) = camera.single() else { return };
        flicker.step = camera.right() * FLICKER_STEP;
    }
    if flicker.frames == second {
        shot(sibling(&path, "moved"), &mut commands, &flicker.shots);
    }
    let shots = flicker.shots.clone();
    let shots = shots.lock().expect("flicker shots");
    if shots.len() < 2 || state.taken_at.is_some() {
        return;
    }
    let (Some(a), Some(b)) = (&shots[0].data, &shots[1].data) else { return };
    let (changed, map) = flicker_map(a, b);
    let mut marked = shots[0].clone();
    marked.data = Some(map);
    let pixels = (a.len() / 4).max(1);
    let between = match kind {
        FlickerKind::Step => format!("a {:.0} cm step", FLICKER_STEP * 100.0),
        FlickerKind::Still => "one frame of time with the camera still".into(),
    };
    info!(
        "flicker: {changed} of {pixels} pixels ({:.3}%) changed by more than {FLICKER_LEVELS} levels for {between}",
        changed as f64 * 100.0 / pixels as f64,
    );
    let marked_path = sibling(&path, "flicker");
    match marked.try_into_dynamic() {
        Ok(image) => match image.to_rgb8().save(&marked_path) {
            Ok(()) => info!("flicker map saved to {}", marked_path.display()),
            Err(e) => error!("cannot save the flicker map: {e}"),
        },
        Err(e) => error!("cannot convert the flicker map: {e}"),
    }
    drop(shots);
    state.taken_at = Some(real.elapsed_secs());
}

/// Records a `--clip` once `capture` has started it: a shot every frame, each one
/// `CLIP_RATE`th of a second of game time after the last.
fn record_clip(mut commands: Commands, mut state: ResMut<Capture>, real: Res<Time<Real>>) {
    let state = &mut *state;
    let Some(clip) = state.clip.as_mut().filter(|c| c.started) else { return };
    if clip.taken < clip.frames {
        let saved = clip.saved.clone();
        commands
            .spawn(Screenshot::image(state.target.clone()))
            .observe(save_to_disk(sibling(&state.path, &format!("{:03}", clip.taken))))
            .observe(move |_: On<ScreenshotCaptured>| {
                saved.fetch_add(1, Ordering::Relaxed);
            });
        clip.taken += 1;
    } else if clip.saved.load(Ordering::Relaxed) >= clip.frames && state.taken_at.is_none() {
        info!("clip: {} frames saved", clip.frames);
        state.taken_at = Some(real.elapsed_secs());
    }
}

/// Compares two RGBA8 images: how many pixels changed by more than `FLICKER_LEVELS` in any
/// channel, and the first image dimmed with those pixels in red.
fn flicker_map(a: &[u8], b: &[u8]) -> (usize, Vec<u8>) {
    let mut changed = 0;
    let mut map = Vec::with_capacity(a.len());
    for (a, b) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let flickers = (0..3).any(|c| a[c].abs_diff(b[c]) > FLICKER_LEVELS);
        changed += flickers as usize;
        let pixel = if flickers { [255, 0, 0, 255] } else { [a[0] / 3, a[1] / 3, a[2] / 3, 255] };
        map.extend_from_slice(&pixel);
    }
    (changed, map)
}

/// `OUT.png` → `OUT-suffix.png`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().map_or_else(Default::default, |s| s.to_string_lossy().into_owned());
    let extension = path.extension().map_or_else(|| "png".into(), |e| e.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem}-{suffix}.{extension}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicker_counts_pixels_that_change_by_more_than_the_threshold() {
        let a = [10, 10, 10, 255, 100, 100, 100, 255, 0, 0, 0, 255];
        let b = [10, 10, 70, 255, 100, 161, 100, 255, 0, 0, 0, 0];
        let (changed, map) = flicker_map(&a, &b);
        assert_eq!(changed, 1);
        assert_eq!(map, [3, 3, 3, 255, 255, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn sibling_adds_a_suffix_before_the_extension() {
        assert_eq!(sibling(Path::new("shots/far.png"), "moved"), PathBuf::from("shots/far-moved.png"));
    }
}
