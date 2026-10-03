//! Block targeting, breaking and placing, and hotbar selection.
//!
//! Edits are predicted locally for responsiveness and sent to the server as actions; if
//! the server disagrees it replies with the authoritative block, which overwrites ours.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use voxmar_engine::block::{BlockId, RenderKind};
use voxmar_engine::inventory::HOTBAR_SLOTS;
use voxmar_engine::physics::{Aabb, RayHit, player as dims, raycast};
use voxmar_protocol::Action;

use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::world::ClientWorld;

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Target>()
            .add_systems(Update, (select_slot, find_target, break_and_place, draw_target).chain());
    }
}

#[derive(Resource, Default)]
pub struct Target {
    pub hit: Option<RayHit>,
    /// Block being broken and progress in 0..1.
    pub breaking: Option<(IVec3, f32)>,
    place_cooldown: f32,
}

fn select_slot(keys: Res<ButtonInput<KeyCode>>, scroll: Res<AccumulatedMouseScroll>, ui: Res<UiState>, mut player: ResMut<LocalPlayer>) {
    if !ui.cursor_locked {
        return;
    }
    const DIGITS: [KeyCode; HOTBAR_SLOTS] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    if let Some(slot) = DIGITS.iter().position(|k| keys.just_pressed(*k)) {
        player.selected = slot;
    }
    if scroll.delta.y.abs() > 0.0 {
        let step = if scroll.delta.y > 0.0 { HOTBAR_SLOTS - 1 } else { 1 };
        player.selected = (player.selected + step) % HOTBAR_SLOTS;
    }
}

fn find_target(world: Res<ClientWorld>, player: Res<LocalPlayer>, mut target: ResMut<Target>) {
    target.hit = raycast(player.eye(), player.look_direction(), dims::REACH, |p| {
        world.block(p).is_some_and(|b| {
            let render = world.registry.info(b).render;
            render != RenderKind::Invisible && render != RenderKind::Translucent
        })
    });
}

fn break_and_place(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    net: Res<Net>,
    mut world: ResMut<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut target: ResMut<Target>,
) {
    target.place_cooldown -= time.delta_secs();
    if !ui.cursor_locked || player.dead {
        target.breaking = None;
        return;
    }
    let Some(hit) = target.hit else {
        target.breaking = None;
        return;
    };

    if mouse.pressed(MouseButton::Left) {
        let block = world.block(hit.block).unwrap_or(BlockId::AIR);
        let hardness = world.registry.info(block).hardness;
        let progress = match target.breaking {
            Some((pos, progress)) if pos == hit.block => progress,
            _ => {
                net.act(Action::StartBreaking { block: hit.block });
                0.0
            }
        };
        if hardness >= 0.0 {
            let progress = progress + time.delta_secs() / hardness.max(0.05);
            if progress >= 1.0 {
                net.act(Action::FinishBreaking { block: hit.block });
                world.set_block(hit.block, BlockId::AIR);
                target.breaking = None;
            } else {
                target.breaking = Some((hit.block, progress));
            }
        }
    } else {
        target.breaking = None;
    }

    if mouse.pressed(MouseButton::Right) && target.place_cooldown <= 0.0 {
        target.place_cooldown = 0.2;
        let pos = hit.block + hit.face.normal();
        let selected = player.selected;
        let Some(item) = player.inventory.get(selected).map(|s| s.item) else { return };
        let solid = world.registry.info(item).solid;
        let overlaps_player = Aabb::standing(player.position, dims::WIDTH, dims::HEIGHT).intersects(|c| c == pos);
        let replaceable = world.block(pos).is_some_and(|b| !world.registry.info(b).solid);
        if replaceable && !(solid && overlaps_player) {
            net.act(Action::Place { block: hit.block, face: hit.face });
            world.set_block(pos, item);
            player.inventory.take_one(selected);
        }
    }
}

fn draw_target(target: Res<Target>, mut gizmos: Gizmos) {
    if let Some(hit) = target.hit {
        let center = hit.block.as_vec3() + 0.5;
        gizmos.cube(Transform::from_translation(center).with_scale(Vec3::splat(1.004)), Color::srgba(0.0, 0.0, 0.0, 0.8));
    }
}
