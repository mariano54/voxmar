//! Local player: mouse look, zoom, predicted movement with voxel collision, and the camera.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::inventory::Inventory;
use voxmar_engine::skill::Skills;
use voxmar_engine::vitals::Vitals;
use voxmar_engine::movement::{self, Body, Controls, Surroundings};
use voxmar_engine::physics::{Aabb, player as dims};

use crate::menu::Screen;
use crate::render::DayCycle;
use crate::session::Launch;
use crate::settings::Settings;
use crate::world::ClientWorld;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayer>()
            .init_resource::<UiState>()
            .init_resource::<Zoom>()
            .init_resource::<SprintFov>()
            .add_systems(
                Update,
                ((grab_cursor, zoom, look, movement, follow_camera).chain().run_if(in_state(Screen::InGame)), set_fov.after(zoom)),
            )
            .add_systems(
                OnExit(Screen::InGame),
                (release_cursor, |mut zoom: ResMut<Zoom>, mut kick: ResMut<SprintFov>| {
                    *zoom = Zoom::default();
                    *kick = SprintFov::default();
                }),
            );
    }
}

#[derive(Resource)]
pub struct LocalPlayer {
    /// Feet position.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub in_water: bool,
    /// Whether the last step was stopped by a wall across (see `movement::Body`).
    pub against_wall: bool,
    pub flying: bool,
    pub sprinting: bool,
    /// Seconds until a held jump may jump again (see `movement::Body`).
    pub jump_delay: f32,
    pub allow_flight: bool,
    pub admin: bool,
    /// Flight speed multiplier, as last agreed with the server.
    pub fly_speed: f32,
    pub health: u8,
    /// The most health the player may have: twice their Hitpoints level, and what effects add.
    pub max_health: u8,
    pub inventory: Inventory,
    /// Experience in every skill, as the server last said, and whether it has said yet.
    pub skills: Skills,
    pub skills_known: bool,
    /// Hunger, warmth and their effects, as the server last said.
    pub vitals: Vitals,
    /// When the vitals last came, in seconds since startup: timed effects count down from it.
    pub vitals_at: f32,
    pub selected: usize,
    /// The crafting table the player is working at (its three-by-three grid shows in the
    /// inventory), until the inventory closes.
    pub station: Option<IVec3>,
    pub dead: bool,
    /// Set once the server has welcomed us.
    pub joined: bool,
    /// The world's seed, as the server told us.
    pub seed: u64,
    /// What the player has discovered, as the server tells it (the journal screen's data).
    pub journal: voxmar_engine::journal::Journal,
    /// Every quest's state, as the server tells it (the quest book's data).
    pub quests: Vec<voxmar_engine::quest::QuestState>,
}

impl Default for LocalPlayer {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.5, 100.0, 0.5),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            in_water: false,
            against_wall: false,
            flying: false,
            sprinting: false,
            jump_delay: 0.0,
            allow_flight: false,
            admin: false,
            fly_speed: 1.0,
            health: 20,
            station: None,
            max_health: 20,
            inventory: Inventory::default(),
            skills: Skills::default(),
            skills_known: false,
            vitals: Vitals::default(),
            vitals_at: 0.0,
            selected: 0,
            dead: false,
            joined: false,
            seed: 0,
            journal: Default::default(),
            quests: Vec::new(),
        }
    }
}

impl LocalPlayer {
    pub fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * dims::EYE_HEIGHT
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn look_direction(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
}

#[derive(Resource, Default)]
pub struct UiState {
    pub inventory_open: bool,
    /// The character screen: skills, health, hunger and effects.
    pub character_open: bool,
    /// The journal of what the player has found.
    pub journal_open: bool,
    /// The settings menu.
    pub menu_open: bool,
    /// The chat and command line.
    pub console_open: bool,
    /// The recipe viewer, over another screen (`hud::recipes`).
    pub recipes_open: bool,
    pub cursor_locked: bool,
}

/// Esc closes the console, the inventory or the character screen, or toggles the menu. The mouse is free while a
/// screen is open, grabbed again when it closes, and grabbed by clicking into the window.
/// Without a window (`--gate`) all of it works the same, but for grabbing the pointer.
fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    player: Res<LocalPlayer>,
    mut screen_was_open: Local<bool>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        if ui.console_open {
            ui.console_open = false;
        } else if ui.recipes_open {
            ui.recipes_open = false;
        } else if ui.inventory_open || ui.character_open || ui.journal_open {
            ui.inventory_open = false;
            ui.character_open = false;
            ui.journal_open = false;
        } else {
            ui.menu_open = !ui.menu_open;
        }
    }
    let screen_open = ui.inventory_open || ui.character_open || ui.journal_open || ui.menu_open || ui.console_open || ui.recipes_open || player.dead;
    let want_locked = !screen_open && (ui.cursor_locked || *screen_was_open || mouse.just_pressed(MouseButton::Left));
    *screen_was_open = screen_open;
    let changed = want_locked != ui.cursor_locked;
    ui.cursor_locked = want_locked;
    let Ok(mut cursor) = cursor.single_mut() else { return };
    if changed || (want_locked && cursor.grab_mode == CursorGrabMode::None) {
        cursor.grab_mode = if want_locked { CursorGrabMode::Locked } else { CursorGrabMode::None };
        cursor.visible = !want_locked;
    }
}

/// The menus need the mouse.
fn release_cursor(mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Zooming in, like a spyglass: while the zoom key is held the view narrows by the chosen
/// magnification, which the wheel changes, easing in and out. Mouse look slows to match,
/// so the view sweeps across the screen as fast as unzoomed, and with cinematic zoom it
/// follows the mouse smoothly, for steady shots.
#[derive(Resource)]
pub struct Zoom {
    /// Whether the zoom key is held where zooming is allowed.
    pub held: bool,
    /// Magnification while held: the setting's, changed by the wheel since the key went down.
    target: f32,
    /// Magnification shown now, easing towards `target` while held and back to 1 after.
    current: f32,
    /// Where cinematic zoom is taking the view (yaw, pitch), while it is on.
    aim: Option<Vec2>,
}

impl Default for Zoom {
    fn default() -> Self {
        Self { held: false, target: 1.0, current: 1.0, aim: None }
    }
}

impl Zoom {
    /// Magnifications the wheel stays between. The smallest is also the least the setting
    /// allows.
    pub const MIN: f32 = 1.5;
    pub const MAX: f32 = 50.0;
    /// Change in magnification per notch of the wheel.
    const STEP: f32 = 1.15;
    /// Pixels of trackpad scrolling that count as one notch.
    const PIXELS_PER_NOTCH: f32 = 20.0;
    /// How quickly the view zooms in and out, per second.
    const RATE: f32 = 12.0;
    /// How quickly the cinematic view catches up with the mouse, per second.
    const FOLLOW: f32 = 5.0;

    /// The vertical field of view, in radians, for `degrees` unzoomed: narrowed so that
    /// everything at the centre looks magnified, rather than the angle simply divided.
    pub fn fov(&self, degrees: f32) -> f32 {
        2.0 * ((degrees.to_radians() / 2.0).tan() / self.current).atan()
    }

    /// How far cinematic smoothing applies, from 0 unzoomed to 1 from the least
    /// magnification on, so it comes and goes with the zoom without a jump.
    fn smoothing(&self) -> f32 {
        (self.current.ln() / Self::MIN.ln()).clamp(0.0, 1.0)
    }

    /// Holds or releases the zoom, turns it by `notches` of the wheel while held, and eases
    /// the magnification over `dt` seconds. `magnification` is the setting.
    fn update(&mut self, held: bool, magnification: f32, notches: f32, dt: f32) {
        if held && !self.held {
            self.target = magnification;
        }
        self.held = held;
        if held {
            self.target = (self.target * Self::STEP.powf(notches)).clamp(Self::MIN, Self::MAX);
        }
        // Eased in proportion, so every doubling takes as long.
        let goal = if held { self.target } else { 1.0 };
        let eased = goal * (self.current / goal).powf((-Self::RATE * dt).exp());
        self.current = if (eased / goal - 1.0).abs() < 1e-3 { goal } else { eased };
    }

    /// Turns the view by `turn` (yaw, pitch) of mouse movement, already scaled to radians,
    /// over `dt` seconds. With `cinematic`, the view eases towards where the mouse points.
    fn look(&mut self, player: &mut LocalPlayer, turn: Vec2, cinematic: bool, dt: f32) {
        let clamp_pitch = |pitch: f32| pitch.clamp(-1.55, 1.55);
        let smoothing = if cinematic { self.smoothing() } else { 0.0 };
        if smoothing == 0.0 {
            self.aim = None;
            player.yaw -= turn.x;
            player.pitch = clamp_pitch(player.pitch - turn.y);
            return;
        }
        let aim = self.aim.get_or_insert(Vec2::new(player.yaw, player.pitch));
        *aim = Vec2::new(aim.x - turn.x, clamp_pitch(aim.y - turn.y));
        let follow = 1.0 - smoothing * (-Self::FOLLOW * dt).exp();
        player.yaw += (aim.x - player.yaw) * follow;
        player.pitch += (aim.y - player.pitch) * follow;
    }
}

/// Zooms while the zoom key is held, in creative (where flight is allowed) or anywhere if
/// the settings allow it. The wheel changes the magnification while zoomed.
pub fn zoom(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiState>,
    settings: Res<Settings>,
    player: Res<LocalPlayer>,
    mut zoom: ResMut<Zoom>,
) {
    let allowed = ui.cursor_locked && !player.dead && (player.allow_flight || settings.zoom_in_survival);
    let held = allowed && keys.pressed(settings.zoom_key.0);
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / Zoom::PIXELS_PER_NOTCH,
    };
    zoom.update(held, settings.zoom as f32 / 100.0, notches, time.delta_secs());
}

fn look(
    time: Res<Time>,
    motion: Res<AccumulatedMouseMotion>,
    ui: Res<UiState>,
    settings: Res<Settings>,
    mut zoom: ResMut<Zoom>,
    mut player: ResMut<LocalPlayer>,
) {
    const SENSITIVITY: f32 = 0.0022;
    let delta = if ui.cursor_locked { motion.delta } else { Vec2::ZERO };
    if delta == Vec2::ZERO && zoom.aim.is_none() {
        return;
    }
    let sensitivity = SENSITIVITY * settings.sensitivity as f32 / 100.0 / zoom.current;
    zoom.look(&mut player, delta * sensitivity, settings.cinematic_zoom, time.delta_secs());
}

/// The view widening a little while sprinting, as Minecraft's does, so speed is felt.
#[derive(Resource, Default)]
pub struct SprintFov {
    /// How far into the kick the view is, 0 to 1.
    amount: f32,
}

impl SprintFov {
    /// How much wider the view is at full kick (Minecraft's is 1.15).
    const WIDEST: f32 = 1.1;
    /// How quickly it widens and narrows back, per second.
    const RATE: f32 = 10.0;

    fn update(&mut self, sprinting: bool, dt: f32) {
        let goal = if sprinting { 1.0 } else { 0.0 };
        self.amount += (goal - self.amount) * (1.0 - (-Self::RATE * dt).exp());
    }

    fn factor(&self) -> f32 {
        1.0 + (Self::WIDEST - 1.0) * self.amount
    }
}

/// The camera's field of view: the setting's, widened while sprinting and narrowed while
/// zoomed.
fn set_fov(
    time: Res<Time>,
    settings: Res<Settings>,
    zoom: Res<Zoom>,
    player: Res<LocalPlayer>,
    mut kick: ResMut<SprintFov>,
    mut camera: Query<&mut Projection, With<Camera3d>>,
) {
    let Ok(mut projection) = camera.single_mut() else { return };
    kick.update(settings.fov_effects && player.sprinting && player.velocity.with_y(0.0).length() > 1.0, time.delta_secs());
    let fov = zoom.fov(settings.fov as f32 * kick.factor());
    if let Projection::Perspective(perspective) = projection.bypass_change_detection()
        && perspective.fov != fov
    {
        perspective.fov = fov;
        projection.set_changed();
    }
}

/// Seconds within which a second press of forward starts a sprint (Minecraft's seven
/// ticks).
const DOUBLE_TAP: f32 = 0.35;

#[allow(clippy::too_many_arguments)]
fn movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiState>,
    settings: Res<Settings>,
    launch: Res<Launch>,
    world: Res<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut last_forward: Local<Option<f32>>,
) {
    let player = &mut *player;
    if !player.joined || player.dead {
        return;
    }
    // Hold still until the ground under us has arrived.
    if !world.voxels.contains(ColumnPos::of_block(player.position.floor().as_ivec3())) {
        player.velocity = Vec3::ZERO;
        return;
    }

    let pressed = |key| ui.cursor_locked && keys.pressed(key);
    if ui.cursor_locked && keys.just_pressed(KeyCode::KeyF) && player.allow_flight {
        player.flying = !player.flying;
        player.velocity = Vec3::ZERO;
    }

    let forward = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
    let right = Vec3::new(player.yaw.cos(), 0.0, -player.yaw.sin());
    let mut wish = Vec3::ZERO;
    for (key, dir) in [(KeyCode::KeyW, forward), (KeyCode::KeyS, -forward), (KeyCode::KeyD, right), (KeyCode::KeyA, -right)] {
        if pressed(key) {
            wish += dir;
        }
    }
    // A second tap of forward soon after the first starts a sprint.
    let now = time.elapsed_secs();
    let mut double_tap = false;
    if ui.cursor_locked && keys.just_pressed(KeyCode::KeyW) {
        double_tap = settings.double_tap_sprint && last_forward.is_some_and(|t| now - t < DOUBLE_TAP);
        *last_forward = (!double_tap).then_some(now);
    }
    let controls = Controls {
        wish: wish.normalize_or_zero(),
        forward: pressed(KeyCode::KeyW) && !pressed(KeyCode::KeyS),
        sprint: pressed(settings.sprint_key.0) || double_tap,
        jump: pressed(KeyCode::Space),
        descend: pressed(KeyCode::ShiftLeft),
    };
    let mut body = Body {
        position: player.position,
        velocity: player.velocity,
        on_ground: player.on_ground,
        in_water: player.in_water,
        flying: player.flying,
        fly_speed: player.fly_speed,
        against_wall: player.against_wall,
        sprinting: player.sprinting,
        jump_delay: player.jump_delay,
    };
    let water = world.registry.id("base:water").unwrap_or(BlockId::AIR);
    // Quicker or slower on foot as the player's effects say (fruit, a gale).
    let speed = launch.content.effects.modifiers(&player.vitals.effects).speed;
    let tuning = launch.content.movement.on_foot(speed);
    movement::advance(&mut body, &Around { world: &world, water }, &tuning, controls, time.delta_secs().min(0.25));
    player.position = body.position;
    player.velocity = body.velocity;
    player.on_ground = body.on_ground;
    player.in_water = body.in_water;
    player.flying = body.flying;
    player.against_wall = body.against_wall;
    player.sprinting = body.sprinting;
    player.jump_delay = body.jump_delay;
}

/// The client's world as movement sees it.
struct Around<'a> {
    world: &'a ClientWorld,
    /// The water block, the one fluid that is swum in.
    water: BlockId,
}

impl Surroundings for Around<'_> {
    fn solid(&self, cell: IVec3) -> bool {
        self.world.is_solid(cell)
    }

    fn water(&self, aabb: &Aabb) -> (f32, Vec2) {
        movement::water_around(&self.world.registry, self.water, |p| self.world.block(p), aabb)
    }
}

fn follow_camera(
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    mut cycle: ResMut<DayCycle>,
    mut camera: Query<&mut Transform, With<Camera3d>>,
) {
    if let Ok(mut transform) = camera.single_mut() {
        transform.translation = player.eye();
        transform.rotation = player.rotation();
    }
    // Under water when the eye is below the surface of the water around it.
    let eye = player.eye();
    let registry = &world.registry;
    let water = registry.id("base:water").and_then(|b| registry.fluid(b)).map(|f| f.fluid);
    let cell = eye.floor().as_ivec3();
    let in_water = |p: IVec3| world.block(p).and_then(|b| registry.fluid(b)).filter(|s| Some(s.fluid) == water);
    cycle.underwater = in_water(cell).is_some_and(|s| in_water(cell + IVec3::Y).is_some() || eye.y - cell.y as f32 <= s.height());
    if cycle.underwater {
        // The surface above, which the light comes down through (no deeper than this is
        // worth looking up: little light is left by then).
        let mut top = cell;
        while top.y - cell.y < 64 && in_water(top + IVec3::Y).is_some() {
            top += IVec3::Y;
        }
        cycle.water_surface = top.y as f32 + in_water(top).map_or(1.0, |s| s.height());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_eases_in_and_out_and_the_wheel_changes_it() {
        let mut zoom = Zoom::default();
        assert_eq!(zoom.fov(70.0), 70f32.to_radians());
        // Held for a second, the view reaches the setting's magnification.
        for _ in 0..60 {
            zoom.update(true, 4.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(zoom.current, 4.0);
        let half = (35f32.to_radians().tan() / 4.0).atan();
        assert!((zoom.fov(70.0) - 2.0 * half).abs() < 1e-6);
        // Scrolling up zooms further, within limits.
        zoom.update(true, 4.0, 2.0, 0.0);
        assert!((zoom.target - 4.0 * Zoom::STEP * Zoom::STEP).abs() < 1e-4);
        zoom.update(true, 4.0, 1000.0, 0.0);
        assert_eq!(zoom.target, Zoom::MAX);
        // Released, it eases back, and the next hold starts from the setting again.
        for _ in 0..60 {
            zoom.update(false, 4.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(zoom.current, 1.0);
        zoom.update(true, 4.0, 0.0, 0.0);
        assert_eq!(zoom.target, 4.0);
    }

    #[test]
    fn cinematic_zoom_follows_the_mouse_smoothly_and_only_while_zoomed() {
        let mut player = LocalPlayer::default();
        let mut zoom = Zoom::default();
        // Unzoomed, looking is direct even with cinematic zoom on.
        zoom.look(&mut player, Vec2::new(0.1, 0.0), true, 1.0 / 60.0);
        assert_eq!(player.yaw, -0.1);
        zoom.current = 4.0;
        zoom.look(&mut player, Vec2::new(0.1, 0.0), true, 1.0 / 60.0);
        assert!(player.yaw > -0.2 && player.yaw < -0.1, "eases towards the aim");
        for _ in 0..300 {
            zoom.look(&mut player, Vec2::ZERO, true, 1.0 / 60.0);
        }
        assert!((player.yaw + 0.2).abs() < 1e-4, "catches up");
        // Without cinematic zoom the view turns at once.
        zoom.look(&mut player, Vec2::new(0.1, 0.0), false, 1.0 / 60.0);
        assert!((player.yaw + 0.3).abs() < 1e-4);
    }
}
