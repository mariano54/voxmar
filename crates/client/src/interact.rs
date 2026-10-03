//! Block targeting, breaking and placing, and hotbar selection.
//!
//! Edits are proposed to the server as actions and predicted locally for responsiveness;
//! if the server disagrees it replies with the authoritative state, which overwrites ours.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use voxmar_engine::action::{ActionContext, ActionTarget, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo, RenderKind};
use voxmar_engine::inventory::HOTBAR_SLOTS;
use voxmar_engine::physics::{Aabb, RayHit, player as dims, raycast};

use crate::Actions;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::sfx::{Cue, SoundCue};
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
    mining_seconds: f32,
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

/// The client's view for checking actions before proposing them, using the same rules
/// the server applies.
struct ClientContext<'a> {
    world: &'a ClientWorld,
    player: &'a LocalPlayer,
}

impl ActionContext for ClientContext<'_> {
    fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.world.block(pos)
    }

    fn info(&self, block: BlockId) -> &BlockInfo {
        self.world.registry.info(block)
    }

    fn actor_eye(&self) -> Vec3 {
        self.player.eye()
    }

    fn actor_alive(&self) -> bool {
        !self.player.dead
    }

    fn actor_held(&self) -> Option<BlockId> {
        self.player.inventory.get(self.player.selected).map(|s| s.item)
    }

    fn occupied(&self, cell: IVec3) -> bool {
        Aabb::standing(self.player.position, dims::WIDTH, dims::HEIGHT).intersects(|c| c == cell)
    }
}

/// Mining proposes `core:mine` and shows progress over the same duration the server
/// uses; releasing the button or looking away cancels it. Placing proposes `core:place`.
/// Both are predicted locally and corrected by the server if it disagrees.
#[allow(clippy::too_many_arguments)]
fn break_and_place(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut world: ResMut<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut target: ResMut<Target>,
    mut cues: MessageWriter<SoundCue>,
) {
    target.place_cooldown -= time.delta_secs();
    let hit = target.hit.filter(|_| ui.cursor_locked && !player.dead);
    let mining = hit.filter(|_| mouse.pressed(MouseButton::Left));

    // Stop mining when the button is released or the target changes.
    if let Some((pos, _)) = target.breaking
        && mining.is_none_or(|hit| hit.block != pos)
    {
        target.breaking = None;
        net.cancel();
    }

    if let Some(hit) = mining {
        let mine = actions.0.expect("core:mine");
        let def = actions.0.get(mine).expect("core:mine is defined");
        let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
        let context = ClientContext { world: &world, player: &player };
        match target.breaking {
            Some((pos, progress)) if pos == hit.block => {
                let step = time.delta_secs() / target.mining_seconds.max(0.05);
                let block = world.block(pos).unwrap_or(BlockId::AIR);
                // A tap sound four times a second while mining.
                let taps = (target.mining_seconds * 4.0).max(1.0);
                if ((progress + step) * taps).floor() > (progress * taps).floor() {
                    cues.write(SoundCue::for_block(&world, block, Cue::Hit));
                }
                let progress = progress + step;
                if progress >= 1.0 {
                    // The server commits at the same moment; predict it, including a plant
                    // that loses its ground.
                    cues.write(SoundCue::for_block(&world, block, Cue::Break));
                    let plant = supported_by(&ClientContext { world: &world, player: &player }, hit.block);
                    world.set_block(hit.block, BlockId::AIR);
                    if let Some(plant) = plant {
                        world.set_block(plant, BlockId::AIR);
                    }
                    target.breaking = None;
                } else {
                    target.breaking = Some((pos, progress));
                }
            }
            _ if def.validate(&context, action_target).is_ok() => {
                target.mining_seconds = def.seconds(&context, action_target);
                target.breaking = Some((hit.block, 0.0));
                net.propose(mine, action_target);
            }
            _ => {}
        }
    }

    let Some(hit) = hit else { return };
    if mouse.pressed(MouseButton::Right) && target.place_cooldown <= 0.0 {
        target.place_cooldown = 0.2;
        let place = actions.0.expect("core:place");
        let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
        let context = ClientContext { world: &world, player: &player };
        let allowed = actions.0.get(place).is_some_and(|def| def.validate(&context, action_target).is_ok());
        if let Some(item) = context.actor_held().filter(|_| allowed) {
            let cell = placement_cell(&context, hit.block, hit.face);
            net.propose(place, action_target);
            cues.write(SoundCue::for_block(&world, item, Cue::Place));
            world.set_block(cell, item);
            let selected = player.selected;
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
