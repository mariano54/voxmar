//! Local player: mouse look, zoom, predicted movement with voxel collision, and the camera.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::fluid;
use voxmar_engine::inventory::Inventory;
use voxmar_engine::physics::{Aabb, move_and_slide, player as dims};

use crate::menu::Screen;
use crate::render::DayCycle;
use crate::settings::Settings;
use crate::world::ClientWorld;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayer>()
            .init_resource::<UiState>()
            .init_resource::<Zoom>()
            .add_systems(
                Update,
                ((grab_cursor, zoom, look, movement, follow_camera).chain().run_if(in_state(Screen::InGame)), set_fov.after(zoom)),
            )
            .add_systems(OnExit(Screen::InGame), (release_cursor, |mut zoom: ResMut<Zoom>| *zoom = Zoom::default()));
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
    pub flying: bool,
    pub allow_flight: bool,
    pub admin: bool,
    /// Flight speed multiplier, as last agreed with the server.
    pub fly_speed: f32,
    pub health: u8,
    pub inventory: Inventory,
    pub selected: usize,
    pub dead: bool,
    /// Set once the server has welcomed us.
    pub joined: bool,
    /// The world's seed, as the server told us.
    pub seed: u64,
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
            flying: false,
            allow_flight: false,
            admin: false,
            fly_speed: 1.0,
            health: 20,
            inventory: Inventory::default(),
            selected: 0,
            dead: false,
            joined: false,
            seed: 0,
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
    /// The settings menu.
    pub menu_open: bool,
    /// The chat and command line.
    pub console_open: bool,
    pub cursor_locked: bool,
}

/// Esc closes the console or the inventory, or toggles the menu. The mouse is free while a
/// screen is open, grabbed again when it closes, and grabbed by clicking into the window.
fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    player: Res<LocalPlayer>,
    mut screen_was_open: Local<bool>,
) {
    let Ok(mut cursor) = cursor.single_mut() else { return };
    if keys.just_pressed(KeyCode::Escape) {
        if ui.console_open {
            ui.console_open = false;
        } else if ui.inventory_open {
            ui.inventory_open = false;
        } else {
            ui.menu_open = !ui.menu_open;
        }
    }
    let screen_open = ui.inventory_open || ui.menu_open || ui.console_open || player.dead;
    let want_locked = !screen_open && (ui.cursor_locked || *screen_was_open || mouse.just_pressed(MouseButton::Left));
    *screen_was_open = screen_open;
    if want_locked != ui.cursor_locked || (want_locked && cursor.grab_mode == CursorGrabMode::None) {
        ui.cursor_locked = want_locked;
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

/// The camera's field of view: the setting's, narrowed while zoomed.
fn set_fov(settings: Res<Settings>, zoom: Res<Zoom>, mut camera: Query<&mut Projection, With<Camera3d>>) {
    let Ok(mut projection) = camera.single_mut() else { return };
    let fov = zoom.fov(settings.fov as f32);
    if let Projection::Perspective(perspective) = projection.bypass_change_detection()
        && perspective.fov != fov
    {
        perspective.fov = fov;
        projection.set_changed();
    }
}

fn movement(time: Res<Time>, keys: Res<ButtonInput<KeyCode>>, ui: Res<UiState>, world: Res<ClientWorld>, mut player: ResMut<LocalPlayer>) {
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
    let wish = wish.normalize_or_zero();
    let sprint = pressed(KeyCode::ControlLeft);
    let jump = pressed(KeyCode::Space);
    let sneak = pressed(KeyCode::ShiftLeft);

    // Fixed substeps keep collision stable at low frame rates.
    let mut remaining = time.delta_secs().min(0.25);
    while remaining > 0.0 {
        let dt = remaining.min(1.0 / 120.0);
        remaining -= dt;
        step(player, &world, wish, sprint, jump, sneak, dt);
    }
}

/// Water shallower than this over the feet is waded through: walked and jumped in, though
/// slowly and carried by its flow, rather than swum in.
const WADE_DEPTH: f32 = 0.4;

/// How deep the water a body stands in is over its feet (0 out of water), and the mean
/// flow of that water (blocks per second along x, z): flowing water and river currents
/// carry swimmers along.
fn water_around(world: &ClientWorld, aabb: &Aabb) -> (f32, Vec2) {
    let registry = &world.registry;
    let water = registry.id("base:water").and_then(|b| registry.fluid(b)).map(|f| f.fluid);
    let (lo, hi) = (aabb.min.floor().as_ivec3(), (aabb.max - 1e-4).floor().as_ivec3());
    let (mut cells, mut depth, mut flow) = (0, 0.0f32, Vec2::ZERO);
    for y in lo.y..=hi.y {
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                let cell = IVec3::new(x, y, z);
                let Some(state) = world.block(cell).and_then(|b| registry.fluid(b)).filter(|s| Some(s.fluid) == water) else { continue };
                // Shallow flowing water only counts up to its surface.
                let covered = world.block(cell + IVec3::Y).and_then(|b| registry.fluid(b)).is_some_and(|s| s.fluid == state.fluid);
                let surface = y as f32 + if covered { 1.0 } else { state.height() };
                if aabb.min.y < surface {
                    cells += 1;
                    depth = depth.max(surface - aabb.min.y);
                    flow += fluid::flow(registry, |p| world.block(p), cell);
                }
            }
        }
    }
    (depth, if cells > 0 { flow / cells as f32 } else { Vec2::ZERO })
}

fn step(player: &mut LocalPlayer, world: &ClientWorld, wish: Vec3, sprint: bool, jump: bool, sneak: bool, dt: f32) {
    let aabb = Aabb::standing(player.position, dims::WIDTH, dims::HEIGHT);
    let (depth, current) = water_around(world, &aabb);
    player.in_water = depth > 0.0;
    let swimming = player.in_water && !(player.on_ground && depth < WADE_DEPTH);

    if player.flying {
        let vertical = (jump as i32 - sneak as i32) as f32;
        let speed = if sprint { dims::FLY_SPRINT_SPEED } else { dims::FLY_SPEED } * player.fly_speed;
        let target = (wish + Vec3::Y * vertical) * speed;
        player.velocity = player.velocity.lerp(target, (12.0 * dt).min(1.0));
    } else {
        let speed = if sprint { dims::SPRINT_SPEED } else { dims::WALK_SPEED } * if player.in_water { 0.5 } else { 1.0 };
        let control = if player.on_ground { 16.0 } else { 3.0 };
        // In water you move relative to the water, so a current carries you along.
        let target = wish * speed + Vec3::new(current.x, 0.0, current.y);
        let horizontal = player.velocity.with_y(0.0).lerp(target, (control * dt).min(1.0));
        player.velocity = horizontal.with_y(player.velocity.y);

        if swimming {
            player.velocity.y -= dims::GRAVITY * 0.15 * dt;
            player.velocity.y *= 1.0 - (2.5 * dt).min(1.0);
            if jump {
                player.velocity.y = (player.velocity.y + 25.0 * dt).min(3.5);
            }
        } else {
            if jump && player.on_ground {
                player.velocity.y = dims::JUMP_VELOCITY;
            }
            player.velocity.y = (player.velocity.y - dims::GRAVITY * dt).max(-dims::TERMINAL_VELOCITY);
        }
    }

    let mut aabb = Aabb::standing(player.position, dims::WIDTH, dims::HEIGHT);
    let blocked = move_and_slide(&mut aabb, player.velocity * dt, |p| world.is_solid(p));
    player.on_ground = blocked[1] && player.velocity.y <= 0.0;
    for (axis, blocked) in blocked.into_iter().enumerate() {
        if blocked {
            player.velocity[axis] = 0.0;
        }
    }
    if player.on_ground && player.flying && !jump {
        player.flying = false;
    }
    player.position = Vec3::new((aabb.min.x + aabb.max.x) / 2.0, aabb.min.y, (aabb.min.z + aabb.max.z) / 2.0);
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
