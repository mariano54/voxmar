//! Local player: mouse look, predicted movement with voxel collision, and the camera.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::inventory::Inventory;
use voxmar_engine::physics::{Aabb, move_and_slide, player as dims};

use crate::world::ClientWorld;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayer>()
            .init_resource::<UiState>()
            .add_systems(Update, (grab_cursor, look, movement, follow_camera).chain());
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
    pub health: u8,
    pub inventory: Inventory,
    pub selected: usize,
    pub dead: bool,
    /// Set once the server has welcomed us.
    pub joined: bool,
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
            health: 20,
            inventory: Inventory::default(),
            selected: 0,
            dead: false,
            joined: false,
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
    pub cursor_locked: bool,
    pub debug: bool,
}

fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    player: Res<LocalPlayer>,
) {
    let Ok(mut cursor) = cursor.single_mut() else { return };
    let want_locked = if keys.just_pressed(KeyCode::Escape) {
        ui.inventory_open = false;
        false
    } else if ui.inventory_open || player.dead {
        false
    } else if mouse.just_pressed(MouseButton::Left) {
        true
    } else {
        ui.cursor_locked
    };
    if want_locked != ui.cursor_locked || (want_locked && cursor.grab_mode == CursorGrabMode::None) {
        ui.cursor_locked = want_locked;
        cursor.grab_mode = if want_locked { CursorGrabMode::Locked } else { CursorGrabMode::None };
        cursor.visible = !want_locked;
    }
}

fn look(motion: Res<AccumulatedMouseMotion>, ui: Res<UiState>, mut player: ResMut<LocalPlayer>) {
    const SENSITIVITY: f32 = 0.0022;
    if !ui.cursor_locked || motion.delta == Vec2::ZERO {
        return;
    }
    player.yaw -= motion.delta.x * SENSITIVITY;
    player.pitch = (player.pitch - motion.delta.y * SENSITIVITY).clamp(-1.55, 1.55);
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

fn step(player: &mut LocalPlayer, world: &ClientWorld, wish: Vec3, sprint: bool, jump: bool, sneak: bool, dt: f32) {
    let water = world.registry.id("base:water");
    let aabb = Aabb::standing(player.position, dims::WIDTH, dims::HEIGHT);
    player.in_water = aabb.intersects(|p| world.block(p).is_some() && world.block(p) == water);

    if player.flying {
        let vertical = (jump as i32 - sneak as i32) as f32;
        let speed = if sprint { dims::FLY_SPRINT_SPEED } else { dims::FLY_SPEED };
        let target = (wish + Vec3::Y * vertical) * speed;
        player.velocity = player.velocity.lerp(target, (12.0 * dt).min(1.0));
    } else {
        let speed = if sprint { dims::SPRINT_SPEED } else { dims::WALK_SPEED } * if player.in_water { 0.5 } else { 1.0 };
        let control = if player.on_ground { 16.0 } else { 3.0 };
        let horizontal = player.velocity.with_y(0.0).lerp(wish * speed, (control * dt).min(1.0));
        player.velocity = horizontal.with_y(player.velocity.y);

        if player.in_water {
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

fn follow_camera(player: Res<LocalPlayer>, mut camera: Query<&mut Transform, With<Camera3d>>) {
    if let Ok(mut transform) = camera.single_mut() {
        transform.translation = player.eye();
        transform.rotation = player.rotation();
    }
}
