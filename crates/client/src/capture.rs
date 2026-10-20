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
//! as `OUT-000.png` on, for checking how things move (wind, water, weather). It starts once
//! the world has settled, or with `--clip-join` the moment the player joins (to see what
//! joining shows); `--clip-command LINE` runs a console command as it starts (`/weather
//! clear`, to watch the weather change).
//!
//! Still shots wait for the weather to be ready (`weather::WeatherReady`) as well as the
//! world, so they show the place's own weather and air.
//!
//! `--strike CREATURE` summons one beside the player and strikes it once, a moment before
//! the shot, to show the target readout with its health; `--level-up SKILL` notes that skill's
//! next level as reached just before the shot, to show how a new level is announced.
//! `--grid A,B,C,D` lays one of each named item (or nothing, for an empty name) into the
//! inventory's crafting grid once the world settles, to show what it makes.
//! `--tend CREATURE` summons one as soon as the world settles and keeps feeding it from the
//! hotbar until the shot, which tames it and then readies it to breed (give the food with
//! `--command`, and time with `--wait`).
//!
//! `--press left|right` holds that mouse button through the first second of a clip,
//! aimed where `--pose` looks, as a player would: a real break or place, through the server,
//! to check edits show.

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

/// Height `--flight` is teleported to before flying forward, above all but the highest peaks.
const FLIGHT_HEIGHT: f32 = voxmar_worldgen::SEA_LEVEL as f32 + 106.0;
/// How long the teleport up before a flight may go unanswered, in the server's seconds.
const LIFT_WAIT: f32 = 3.0;

pub struct CapturePlugin {
    pub path: PathBuf,
    /// Optional camera override: offset from the player's eye, then yaw and pitch (degrees).
    pub pose: Option<[f32; 5]>,
    /// Seconds to fly forward after loading, measuring frame pacing while streaming.
    pub flight: Option<f32>,
    /// Seconds to dig straight down after loading, holding the button as a player would,
    /// measuring frame pacing while blocks break, burst, drop and remesh.
    pub dig: Option<f32>,
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
    /// Show the character screen.
    pub character: bool,
    /// Measure flicker for a small camera step or as time passes (see the module docs).
    pub flicker: Option<FlickerKind>,
    /// Seconds of game time to record frame by frame (see the module docs).
    pub clip: Option<f32>,
    /// Start the clip on joining, and run these console lines as it starts.
    pub clip_join: bool,
    pub clip_commands: Vec<String>,
    /// A mouse button to hold at the start of the clip (see the module docs).
    pub press: Option<MouseButton>,
    /// An item whose hotbar slot to select once the inventory has it (`--hold`).
    pub hold: Option<String>,
    /// Armour to put on once the inventory has it (`--wear`, again for each piece).
    pub wear: Vec<String>,
    /// Click `press` over and over through the clip (`--repeat`).
    pub repeat: bool,
    /// Keep looking at the nearest creature (`--face-creature`).
    pub face_creature: bool,
    /// The creature to summon and strike before the shot (see the module docs).
    pub strike: Option<String>,
    /// The skill whose next level to note before the shot (see the module docs).
    pub level_up: Option<String>,
    /// The creature to summon and keep feeding until the shot (see the module docs).
    pub tend: Option<String>,
    /// Items to lay into the crafting grid (see the module docs).
    pub grid: Option<Vec<String>>,
    /// Width and height of the shot, in pixels.
    pub size: UVec2,
    /// Screen pixels per interface pixel, as a window on a Retina display has 2.
    pub scale_factor: f32,
    /// Render to a real window and shoot that, rather than an offscreen image: slower to
    /// set up and in the way on screen, but it exercises what only a window has (the
    /// render scale, presenting frames).
    pub windowed: bool,
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
        // Drawing offscreen, with no swapchain to hold the CPU back (see `in_flight`).
        if !self.windowed {
            app.add_plugins(crate::render::in_flight::InFlightPlugin);
        }
        app.insert_resource(UiState { menu_open: self.menu, inventory_open: self.inventory, character_open: self.character, ..default() }).insert_resource(Capture {
            path: self.path.clone(),
            pose: self.pose,
            flight: self.flight,
            lift_asked: None,
            dig: self.dig,
            dig_started: None,
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
            steady_times: Vec::new(),
            flicker: self.flicker.map(|kind| Flicker { kind, ..default() }),
            clip: self.clip.map(|seconds| Clip {
                frames: (seconds * CLIP_RATE as f32).ceil() as u32,
                join: self.clip_join,
                commands: self.clip_commands.clone(),
                ..default()
            }),
            press: self.press,
            hold: self.hold.clone(),
            wear: self.wear.clone(),
            dressed: self.wear.is_empty(),
            repeat: self.repeat,
            face_creature: self.face_creature,
            strike: self.strike.clone(),
            tend: self.tend.clone(),
            grid: self.grid.clone(),
            fit_listed: false,
            fed_at: 0.0,
            summoned: false,
            struck: false,
            level_up: self.level_up.clone(),
            size: self.size,
            scale_factor: self.scale_factor,
            windowed: self.windowed,
        })
        .add_systems(PostStartup, render_offscreen.run_if(|state: Res<Capture>| !state.windowed))
        .add_systems(
            Update,
            (target_ui.run_if(|state: Res<Capture>| !state.windowed), capture, measure_flicker.after(capture), record_clip.after(capture), list_spills.after(capture)),
        )
        .add_systems(
            Update,
            mine.after(crate::interact::InteractSystems).before(crate::breaking::EffectSystems).run_if(in_state(Screen::InGame)),
        )
        .add_systems(Update, (hold, wear, press, dig, showcase, fill_grid).before(crate::interact::InteractSystems).run_if(in_state(Screen::InGame)))
        .add_systems(
            PostUpdate,
            (apply_pose.run_if(in_state(Screen::InGame)), hold_time)
                .before(crate::render::update_environment)
                .before(TransformSystems::Propagate),
        );
    }
}

#[derive(Resource)]
struct Capture {
    path: PathBuf,
    pose: Option<[f32; 5]>,
    flight: Option<f32>,
    /// When (by the server's clock) the flight asked to be teleported to its height.
    lift_asked: Option<f32>,
    dig: Option<f32>,
    dig_started: Option<f32>,
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
    /// Frame times while a still shot waits, after its first second.
    steady_times: Vec<f32>,
    flicker: Option<Flicker>,
    clip: Option<Clip>,
    press: Option<MouseButton>,
    hold: Option<String>,
    wear: Vec<String>,
    /// Every piece of `wear` is on (the shot waits for it).
    dressed: bool,
    repeat: bool,
    face_creature: bool,
    strike: Option<String>,
    tend: Option<String>,
    grid: Option<Vec<String>>,
    /// Whether the texts that do not fit were listed for the shot.
    fit_listed: bool,
    /// When `--tend` last fed its creature.
    fed_at: f32,
    summoned: bool,
    struck: bool,
    level_up: Option<String>,
    size: UVec2,
    scale_factor: f32,
    windowed: bool,
}

impl Capture {
    /// A shot of what is drawn: the offscreen image, or the window. The image is named with
    /// its scale factor, as the camera names it, or Bevy takes it for another target.
    fn screenshot(&self) -> Screenshot {
        if self.windowed {
            Screenshot::primary_window()
        } else {
            Screenshot(RenderTarget::Image(bevy::camera::ImageRenderTarget { handle: self.target.clone(), scale_factor: self.scale_factor }))
        }
    }
}

/// Progress of a `--clip`, which starts once the world has settled.
#[derive(Default)]
struct Clip {
    frames: u32,
    /// Starts on joining rather than once settled, and runs these lines as it starts.
    join: bool,
    commands: Vec<String>,
    started: bool,
    /// Shots asked for, and shots written.
    taken: u32,
    saved: Arc<AtomicU32>,
}

impl Clip {
    /// `record_clip` takes over, and every frame from now on advances the game alike.
    fn start(&mut self, commands: &mut Commands, console: &mut crate::hud::console::Console) {
        self.started = true;
        console.run_when_joined(std::mem::take(&mut self.commands));
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1) / CLIP_RATE));
    }
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
    state.target = offscreen(&mut commands, &mut images, &camera, state.size, state.scale_factor);
}

/// Points the world's camera at a new offscreen image `size` pixels across, which shots
/// are then taken of, and returns the image. `scale_factor` is screen pixels per
/// interface pixel, as on a window.
pub(crate) fn offscreen(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    camera: &Query<Entity, With<Camera3d>>,
    size: UVec2,
    scale_factor: f32,
) -> Handle<Image> {
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = images.add(image);
    commands.insert_resource(crate::screenshot::Offscreen(target.clone()));
    for entity in camera {
        commands.entity(entity).insert(RenderTarget::Image(bevy::camera::ImageRenderTarget { handle: target.clone(), scale_factor }));
    }
    target
}

type UntargetedUiRoot = (With<Node>, Without<ChildOf>, Without<UiTargetCamera>);

/// UI follows the window by default; point it at the offscreen camera instead.
pub(crate) fn target_ui(
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
    // `--face-creature` turns the player toward the creature; the camera looks as they do.
    let (yaw, pitch) = if state.face_creature { (player.yaw, player.pitch) } else { (yaw.to_radians(), pitch.to_radians()) };
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
}

/// Selects the hotbar slot holding `--hold`'s item, as a player picks it with a number key.
fn hold(state: Res<Capture>, world: Res<crate::world::ClientWorld>, mut player: ResMut<LocalPlayer>) {
    let Some(item) = state.hold.as_deref().and_then(|name| world.registry.id(name).or_else(|| world.registry.id(&format!("base:{name}")))) else { return };
    if let Some(slot) = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| player.inventory.get(s).is_some_and(|stack| stack.item == item)) {
        player.selected = slot;
    }
}

/// Puts on each piece of `--wear` once the inventory has it, moving it into its worn slot
/// as a click would. The shot waits until every piece is on; a piece the server has not
/// put on `WEAR_TICKS` after it was asked fails the capture, naming it.
fn wear(
    mut state: ResMut<Capture>,
    world: Res<crate::world::ClientWorld>,
    player: Res<LocalPlayer>,
    net: Res<crate::net::Net>,
    actions: Res<crate::Actions>,
    mut asked: Local<Vec<(String, usize, u64)>>,
    mut exit: MessageWriter<AppExit>,
) {
    use voxmar_engine::inventory::{WORN, worn_slot};
    let ticks = net.server().map_or(0, |server| server.ticks());
    let id = |name: &str| world.registry.id(name).or_else(|| world.registry.id(&format!("base:{name}")));
    let new: Vec<String> = state.wear.iter().filter(|name| !asked.iter().any(|(n, ..)| n == *name)).cloned().collect();
    for name in &new {
        let Some(item) = id(name) else { continue };
        let (Some(from), Some(at)) = ((0..WORN).find(|&s| player.inventory.get(s).is_some_and(|stack| stack.item == item)), worn_slot(item)) else { continue };
        let target = voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: (WORN + at) as u8, count: 1 };
        net.propose(actions.0.expect("core:transfer"), target);
        asked.push((name.clone(), WORN + at, ticks));
    }
    let worn = |(name, slot, _): &(String, usize, u64)| player.inventory.get(*slot).is_some_and(|stack| Some(stack.item) == id(name));
    if let Some((name, ..)) = asked.iter().find(|piece| !worn(piece) && ticks.saturating_sub(piece.2) > WEAR_TICKS) {
        error!("capture: {name} was not put on {WEAR_TICKS} server ticks after asking");
        exit.write(AppExit::error());
    }
    let dressed = asked.len() == state.wear.len() && asked.iter().all(worn);
    if state.dressed != dressed {
        state.dressed = dressed;
    }
}

/// Server ticks a piece `--wear` asks for has to come on.
const WEAR_TICKS: u64 = 40;

/// Clip frames `--press` holds its button for: a second, long enough to dig soil.
const PRESS_FRAMES: u32 = CLIP_RATE;

/// Holds `--press`'s button at the start of the clip, with the player looking where the
/// pose does, so the interaction systems break or place exactly as for a player.
fn press(
    state: Res<Capture>,
    creatures: Res<crate::creatures::Creatures>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut ui: ResMut<UiState>,
    mut player: ResMut<LocalPlayer>,
) {
    // `--face-creature`: the eye follows the nearest creature, as a player tracks one.
    if state.face_creature {
        let eye = player.eye();
        if let Some(view) = creatures.views().min_by(|a, b| a.position.distance(eye).total_cmp(&b.position.distance(eye))) {
            let to = view.position + Vec3::Y * 0.25 - eye;
            player.yaw = (-to.x).atan2(-to.z);
            player.pitch = to.y.atan2(to.xz().length());
        }
    }
    let Some(button) = state.press else { return };
    if let Some([_, _, _, yaw, pitch]) = state.pose
        && !state.face_creature
    {
        player.yaw = yaw.to_radians();
        player.pitch = pitch.to_radians();
    }
    // Held through the first second, or with `--repeat` clicked every 0.4 s of the clip.
    let holding = state.clip.as_ref().is_some_and(|c| c.started && if state.repeat { c.taken % 12 < 2 } else { c.taken < PRESS_FRAMES });
    if holding {
        ui.cursor_locked = true;
        mouse.press(button);
    } else if mouse.pressed(button) {
        mouse.release(button);
    }
}

/// Holds the left button looking straight down while `--dig` runs, so the interaction
/// systems break block after block exactly as for a player digging a shaft.
fn dig(state: Res<Capture>, mut mouse: ResMut<ButtonInput<MouseButton>>, mut ui: ResMut<UiState>, mut player: ResMut<LocalPlayer>) {
    if state.dig.is_none() {
        return;
    }
    if state.dig_started.is_some() && state.taken_at.is_none() {
        ui.cursor_locked = true;
        player.pitch = -89f32.to_radians();
        mouse.press(MouseButton::Left);
    } else if mouse.pressed(MouseButton::Left) {
        mouse.release(MouseButton::Left);
    }
}

/// Seconds before a still shot that `--strike` summons its creature, that it strikes it,
/// and that `--level-up` notes its level: long enough for the server's answers, short
/// enough that nothing has wandered off or faded.
const SUMMON_LEAD: f32 = 1.2;
const SHOWCASE_LEAD: f32 = 0.6;
/// Seconds between `--tend`'s feeds, as a player clicks.
const TEND_EVERY: f32 = 0.5;

/// Summons and strikes a creature for `--strike`, as a player clicking on it, and notes a
/// new level for `--level-up`, just before the shot.
#[allow(clippy::too_many_arguments)]
fn showcase(
    time: Res<Time>,
    launch: Res<crate::session::Launch>,
    creatures: Res<crate::creatures::Creatures>,
    mut state: ResMut<Capture>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut ui: ResMut<UiState>,
    mut player: ResMut<LocalPlayer>,
    mut toasts: ResMut<crate::hud::skills::SkillToasts>,
    mut console: ResMut<crate::hud::console::Console>,
) {
    let Some(settled) = state.settled_at else { return };
    let now = time.elapsed_secs();
    if state.strike.is_some() && mouse.pressed(MouseButton::Left) {
        mouse.release(MouseButton::Left);
    }
    let until_shot = 4.0 + state.wait - (now - settled);
    if state.tend.is_some() && mouse.pressed(MouseButton::Right) {
        mouse.release(MouseButton::Right);
    }
    if let Some(name) = state.tend.clone()
        && state.taken_at.is_none()
    {
        if !state.summoned {
            console.run_when_joined(vec![format!("/summon {name} 1 ~ ~ ~-2")]);
            state.summoned = true;
        } else if now - state.fed_at > TEND_EVERY
            && let Some(species) = launch.content.creatures.find(&name).or_else(|| launch.content.creatures.find(&format!("base:{name}")))
            && let Some(view) = creatures.views().filter(|v| v.species == species).min_by_key(|v| v.id)
        {
            // Turn to it, and feed it whatever in the hotbar it takes now: taming food until
            // it is ours, then breeding food.
            // Ready to breed, it has had all it wants: just keep looking at it.
            let creature = launch.content.creatures.get(species);
            let food = if view.kept { &creature.breed_food } else { &creature.tame_food };
            let height = creature.def.body.size.1 * view.scale;
            let to = view.position + Vec3::Y * height * 0.6 - player.eye();
            player.yaw = (-to.x).atan2(-to.z);
            player.pitch = to.y.atan2(to.xz().length());
            if !view.ready
                && let Some(slot) = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| player.inventory.get(s).is_some_and(|stack| food.contains(&stack.item)))
            {
                player.selected = slot;
                ui.cursor_locked = true;
                mouse.press(MouseButton::Right);
            }
            state.fed_at = now;
        }
    }
    if state.taken_at.is_some() || until_shot > SUMMON_LEAD {
        return;
    }
    if let Some(name) = &state.strike
        && !state.summoned
    {
        console.run_when_joined(vec![format!("/summon {name} 1 ~ ~ ~-2")]);
        state.summoned = true;
    }
    if until_shot > SHOWCASE_LEAD {
        return;
    }
    if let Some(name) = state.level_up.take() {
        let skills = &launch.content.skills;
        match skills.named("core", &name) {
            Some(id) => toasts.level_up(id, (skills.level(player.skills.xp(id)) + 1).min(skills.max_level())),
            None => warn!("--level-up: no skill called `{name}`"),
        }
    }
    if state.strike.is_some() && !state.struck {
        state.struck = true;
        let eye = player.eye();
        let Some(nearest) = creatures.views().min_by(|a, b| a.position.distance_squared(eye).total_cmp(&b.position.distance_squared(eye))) else {
            warn!("--strike: no creature in sight");
            return;
        };
        info!("--strike: {} at {:.1} blocks", launch.content.creatures.get(nearest.species).def.name, nearest.position.distance(eye));
        // Its upper body, above the grass.
        let height = launch.content.creatures.get(nearest.species).def.body.size.1 * nearest.scale;
        let to = nearest.position + Vec3::Y * height * 0.75 - eye;
        player.yaw = (-to.x).atan2(-to.z);
        player.pitch = to.y.atan2(to.xz().length());
        ui.cursor_locked = true;
        mouse.press(MouseButton::Left);
    }
}

/// Lays `--grid`'s items into the crafting grid as the inventory's clicks would, once
/// the world has settled; the server does the moving. Up to four go into the inventory's
/// two by two; more show a crafting table's three by three, row by row.
fn fill_grid(
    mut state: ResMut<Capture>,
    mut player: ResMut<LocalPlayer>,
    world: Res<ClientWorld>,
    actions: Res<crate::Actions>,
    net: Res<crate::net::Net>,
) {
    if state.settled_at.is_none() {
        return;
    }
    let Some(names) = state.grid.take() else { return };
    let table = names.len() > voxmar_engine::craft::SMALL.len();
    if table {
        player.station = Some(player.position.floor().as_ivec3());
    }
    let cell = |i: usize| if table { i } else { voxmar_engine::craft::SMALL[i] };
    let mut taken = vec![0u8; voxmar_engine::inventory::SLOTS];
    for (i, name) in names.iter().enumerate().filter(|(_, name)| !name.is_empty()) {
        let item = world.registry.id(name).or_else(|| world.registry.id(&format!("base:{name}")));
        let from = (0..voxmar_engine::inventory::SLOTS).find(|&s| player.inventory.get(s).is_some_and(|stack| Some(stack.item) == item && stack.count > taken[s]));
        let Some(from) = from else {
            warn!("--grid: no {name} in the inventory");
            continue;
        };
        taken[from] += 1;
        let target = voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: (voxmar_engine::inventory::CRAFT + cell(i)) as u8, count: 1 };
        net.propose(actions.0.expect("core:transfer"), target);
    }
}

/// Lists every text in the shot that does not fit its element (see `ui::fit`).
fn list_spills(world: &mut World) {
    let state = world.resource::<Capture>();
    if state.taken_at.is_none() || state.fit_listed {
        return;
    }
    world.resource_mut::<Capture>().fit_listed = true;
    let spills = crate::ui::fit::find(world);
    if spills.is_empty() {
        info!("fit: every text fits");
    }
    for spill in spills {
        warn!("fit: {spill}");
    }
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

/// Holds the time of day for still shots. Clips (once the world has settled) and
/// `--flicker-still` runs let it run on from there, so the sky moves in them as in the game
/// (clouds forming, stars falling). Runs after the server's clock has moved the day and
/// before the sky reads it.
fn hold_time(state: Res<Capture>, time: Res<Time>, mut cycle: ResMut<DayCycle>) {
    if let Some(day) = state.time {
        let clip_start = state.clip.as_ref().and(state.settled_at).map(f64::from);
        let start = state.flicker.as_ref().and_then(|f| f.since).or(clip_start);
        let since = start.map_or(0.0, |start| time.elapsed_secs_f64() - start);
        cycle.ticks = day as f64 * TICKS_PER_DAY as f64 + since * voxmar_protocol::TICKS_PER_SECOND as f64;
    }
}

/// Flies forward at sprint-flight speed (above the terrain: `capture` has teleported the
/// player up first).
fn fly(time: &Time, player: &mut LocalPlayer, sprint_speed: f32) {
    player.flying = true;
    let forward = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
    // As fast as the player's fly speed setting makes it (an admin's, up to several times).
    let speed = sprint_speed * 0.9 * player.fly_speed;
    player.position += forward * speed * time.delta_secs();
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
    (weather, air, launch, creatures): (
        Res<crate::weather::ClientWeather>, Res<crate::weather::atmosphere::ClientAtmosphere>, Res<crate::session::Launch>, Res<crate::creatures::Creatures>,
    ),
    (ready, mut console, net): (Res<crate::weather::WeatherReady>, ResMut<crate::hud::console::Console>, Res<crate::net::Net>),
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
    if let Some(clip) = &mut state.clip
        && clip.join
        && *screen.get() == Screen::InGame
        && player.joined
    {
        clip.start(&mut commands, &mut console);
        return;
    }
    let busy = joining
        || (*screen.get() == Screen::InGame && !ready.0)
        || (*screen.get() == Screen::InGame && !state.dressed)
        || stats.jobs_in_flight > 0
        || stats.pending_columns > 0
        || world.voxels.len() < 9
        || lod_tiles < lod_wanted;
    // Flying and digging keep the world busy streaming and remeshing; once started they
    // run their course. While a flight's teleport up is unanswered, the world settling
    // must not hold back the check on its deadline; once the player is up, the land there
    // settles before the flight is timed, so its frame times are the flight's own.
    let lift_pending = state.lift_asked.is_some() && state.flight_started.is_none() && player.position.y < FLIGHT_HEIGHT - 1.0;
    let flying = state.flight_started.is_some() || state.dig_started.is_some() || lift_pending;
    // A mined block's pieces are caught in the air, while its column remeshes.
    let broken = state.broken_at.map(|at| now - at);
    let patience = if state.flicker.is_some() { 300.0 } else { 60.0 };
    if busy && !flying && broken.is_none() && now < patience {
        state.settled_at = None;
        state.steady_times.clear();
        return;
    }
    // Measure a few seconds of steady-state rendering before capturing.
    let settled = *state.settled_at.get_or_insert(now);
    if broken.map_or(now - settled < 4.0 + state.wait, |since| since < BROKEN_FOR) {
        // The steady state before a still shot, measured as a flight is.
        if state.flight.is_none() && state.dig.is_none() && now - settled > 1.0 {
            state.steady_times.push(real.delta_secs() * 1000.0);
        }
        return;
    }
    // Frame times by the wall clock: the game's own clock caps a frame at 250 ms.
    let frame_ms = real.delta_secs() * 1000.0;
    // A flight first goes above the terrain by teleport (a climb straight up is refused
    // under a canopy or an overhang), and is timed only from there, so its frame times are
    // the flight's and not the teleport's. A teleport the server never answers fails it.
    if state.flight.is_some() && player.position.y < FLIGHT_HEIGHT - 1.0 {
        player.flying = true;
        // Once flying, a drop below the height (a correction from the server) fails the
        // flight rather than being taken for a teleport not yet answered.
        if state.flight_started.is_some() {
            error!("flight: fell to y {:.1}, below its height of {FLIGHT_HEIGHT}", player.position.y);
            exit.write(AppExit::error());
            return;
        }
        // The server's clock when the server is this process's own; for a remote join, the
        // client's (whose frames count for at most 250 ms each).
        let server_now = net.server().map_or(now, |server| server.ticks() as f32 / voxmar_protocol::TICKS_PER_SECOND as f32);
        match state.lift_asked {
            None => {
                // `VOXMAR_CAPTURE_DROP_LIFT` asks for nothing, as if the teleport were lost: to
                // show that an unanswered one fails the capture.
                if std::env::var_os("VOXMAR_CAPTURE_DROP_LIFT").is_none() {
                    net.command(&format!("tp {} {FLIGHT_HEIGHT} {}", player.position.x, player.position.z));
                }
                state.lift_asked = Some(server_now);
            }
            Some(asked) if server_now - asked > LIFT_WAIT => {
                error!("flight: the teleport up was not answered in {LIFT_WAIT} s of the server's clock");
                exit.write(AppExit::error());
            }
            Some(_) => {}
        }
        return;
    }
    if let Some(duration) = state.flight
        && now - *state.flight_started.get_or_insert(now) < duration
    {
        // Marks the flight's frames in a Chrome trace (tools/perf/flight_spikes.py), which
        // cannot tell them by wall time: the flight is timed in game time, which falls behind
        // when a frame is capped, and the trace goes on through the shot and the exit.
        let _flight = info_span!("capture_flight").entered();
        fly(&time, &mut player, launch.content.movement.fly_sprint_speed);
        state.frame_times.push(frame_ms);
        return;
    }
    if let Some(duration) = state.dig
        && now - *state.dig_started.get_or_insert(now) < duration
    {
        state.frame_times.push(frame_ms);
        return;
    }
    if !state.frame_times.is_empty() {
        let label = if state.dig.is_some() { "dig" } else { "flight" };
        info!("{label}: {}", frame_stats(std::mem::take(&mut state.frame_times)));
    } else if !state.steady_times.is_empty() {
        info!("steady: {}", frame_stats(std::mem::take(&mut state.steady_times)));
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
    // Count after taking the frame sample, so settlement comparisons record their
    // actual replicated population without adding census work to the timed frames.
    let mut actors = [0usize; 5];
    for view in creatures.views() {
        let i = match launch.content.creatures.get(view.species).def.kind {
            voxmar_engine::creature::Kind::Villager => 0,
            voxmar_engine::creature::Kind::Animal => 1,
            voxmar_engine::creature::Kind::Monster => 2,
            voxmar_engine::creature::Kind::Fish => 3,
            voxmar_engine::creature::Kind::Spirit => 4,
        };
        actors[i] += 1;
    }
    info!("actors: {} villagers, {} animals, {} monsters, {} fish, {} spirits replicated alive", actors[0], actors[1], actors[2], actors[3], actors[4]);
    // Per-pass GPU and CPU times with VOXMAR_GPU_TIMES (`RenderDiagnosticsPlugin`).
    let mut passes: Vec<(String, f64)> = diagnostics
        .iter()
        .filter(|d| d.path().as_str().starts_with("render/") && d.path().as_str().ends_with("/elapsed_gpu"))
        .filter_map(|d| Some((d.path().as_str().to_owned(), d.average()?)))
        .collect();
    if !passes.is_empty() {
        passes.sort_by(|a, b| b.1.total_cmp(&a.1));
        let total: f64 = passes.iter().filter(|(p, _)| p.matches('/').count() == 2).map(|(_, t)| t).sum();
        info!("gpu passes: {total:.3} ms at the top level");
        for (path, ms) in passes {
            info!("gpu {ms:7.3} ms {path}");
        }
    }
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
        clip.start(&mut commands, &mut console);
        return;
    }
    commands
        .spawn(state.screenshot())
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
    let (target, path) = (RenderTarget::Image(bevy::camera::ImageRenderTarget { handle: state.target.clone(), scale_factor: state.scale_factor }), state.path.clone());
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
            .spawn(Screenshot(target.clone()))
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
    let screenshot = state.screenshot();
    let Some(clip) = state.clip.as_mut().filter(|c| c.started) else { return };
    if clip.taken < clip.frames {
        let saved = clip.saved.clone();
        commands
            .spawn(screenshot)
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

/// A run's frame times summed up, as `tools/bench` reads them: count, median, p99 and the
/// worst (ms), frames over twice the median, the mean and the 1% low (the mean of the
/// slowest 1%) as frames per second.
fn frame_stats(mut times: Vec<f32>) -> String {
    times.sort_by(f32::total_cmp);
    let at = |q: f32| times[((times.len() - 1) as f32 * q) as usize];
    let median = at(0.5);
    let hitches = times.iter().filter(|&&t| t > median * 2.0).count();
    let mean = times.iter().sum::<f32>() / times.len() as f32;
    let slowest = &times[times.len() - (times.len() / 100).max(1)..];
    let low = slowest.iter().sum::<f32>() / slowest.len() as f32;
    format!(
        "{} frames, median {median:.2} ms, p99 {:.2} ms, max {:.2} ms, {hitches} frames over twice the median, mean {mean:.2} ms ({:.0} fps), 1% low {:.0} fps",
        times.len(),
        at(0.99),
        at(1.0),
        1000.0 / mean,
        1000.0 / low,
    )
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
    fn frame_stats_give_the_mean_and_the_slowest_hundredth() {
        let mut times = vec![10.0; 199];
        times.push(50.0);
        let stats = frame_stats(times);
        assert!(stats.starts_with("200 frames, median 10.00 ms, p99 10.00 ms, max 50.00 ms, 1 frames over twice the median"), "{stats}");
        // The slowest 1% of 200 frames is the two slowest: 50 and 10 ms.
        assert!(stats.ends_with("mean 10.20 ms (98 fps), 1% low 33 fps"), "{stats}");
    }

    #[test]
    fn sibling_adds_a_suffix_before_the_extension() {
        assert_eq!(sibling(Path::new("shots/far.png"), "moved"), PathBuf::from("shots/far-moved.png"));
    }
}
