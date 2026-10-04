//! Block targeting, breaking and placing, and hotbar selection.
//!
//! Edits are proposed to the server as actions and predicted locally for responsiveness;
//! if the server disagrees it replies with the authoritative state, which overwrites ours.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use voxmar_engine::action::{ActionContext, ActionTarget, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo};
use voxmar_engine::inventory::{HOTBAR_SLOTS, SLOTS};
use voxmar_engine::physics::{Aabb, RayHit, player as dims, raycast};

use crate::Actions;
use crate::breaking::{BlockBroken, shows_breaking};
use crate::net::Net;
use crate::player::{self, LocalPlayer, UiState, Zoom};
use crate::sfx::{Cue, SoundCue};
use crate::world::ClientWorld;

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Target>()
            .add_systems(
                Update,
                (select_slot.after(player::zoom), find_target, pick_block, break_and_place, draw_target)
                    .chain()
                    .in_set(InteractSystems)
                    .run_if(in_state(crate::menu::Screen::InGame)),
            );
    }
}

/// Targeting, mining and placing, which effects of them run after.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InteractSystems;

#[derive(Resource, Default)]
pub struct Target {
    pub hit: Option<RayHit>,
    /// Block being broken and progress in 0..1.
    pub breaking: Option<(IVec3, f32)>,
    mining_seconds: f32,
    place_cooldown: f32,
}

/// The number keys pick a hotbar slot, and so does the wheel unless it is zooming.
fn select_slot(
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiState>,
    zoom: Res<Zoom>,
    mut player: ResMut<LocalPlayer>,
) {
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
    if scroll.delta.y.abs() > 0.0 && !zoom.held {
        let step = if scroll.delta.y > 0.0 { HOTBAR_SLOTS - 1 } else { 1 };
        player.selected = (player.selected + step) % HOTBAR_SLOTS;
    }
}

/// The middle button picks the block looked at: it selects the hotbar slot holding it, or
/// swaps it into the selected slot from the rest of the inventory, as in Minecraft.
fn pick_block(
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    target: Res<Target>,
    world: Res<ClientWorld>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
) {
    if !ui.cursor_locked || !mouse.just_pressed(MouseButton::Middle) {
        return;
    }
    let Some(block) = target.hit.and_then(|hit| world.block(hit.block)) else { return };
    let item = world.registry.info(block).drops;
    let item = if item.is_air() { block } else { item };
    let holds = |slot: usize| player.inventory.get(slot).is_some_and(|s| s.item == item);
    if let Some(slot) = (0..HOTBAR_SLOTS).find(|&slot| holds(slot)) {
        player.selected = slot;
    } else if let Some(slot) = (HOTBAR_SLOTS..SLOTS).find(|&slot| holds(slot)) {
        let selected = player.selected;
        let count = player.inventory.get(slot).map_or(0, |s| s.count);
        if player.inventory.move_items(slot, selected, count) {
            let target = ActionTarget::Slots { from: slot as u8, to: selected as u8, count };
            net.propose(actions.0.expect("core:transfer"), target);
        }
    }
}

fn find_target(world: Res<ClientWorld>, player: Res<LocalPlayer>, mut target: ResMut<Target>) {
    // Water is looked through; ice and other solid see-through blocks can be mined.
    target.hit = raycast(player.eye(), player.look_direction(), dims::REACH, |p| {
        world.block(p).is_some_and(|b| shows_breaking(world.registry.info(b)))
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
    mut broken: MessageWriter<BlockBroken>,
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
                // A tap sound four times a second while mining; the last one is the break.
                let taps = (target.mining_seconds * 4.0).max(1.0);
                if progress + step < 1.0 && ((progress + step) * taps).floor() > (progress * taps).floor() {
                    cues.write(SoundCue::for_block(&world, block, Cue::Hit));
                }
                let progress = progress + step;
                if progress >= 1.0 {
                    // The server commits at the same moment; predict it, including a plant
                    // that loses its ground.
                    cues.write(SoundCue::for_block(&world, block, Cue::Break));
                    let plants = supported_by(&ClientContext { world: &world, player: &player }, hit.block);
                    for pos in std::iter::once(hit.block).chain(plants) {
                        if let Some(block) = world.block(pos) {
                            broken.write(BlockBroken { pos, block });
                        }
                        world.set_block(pos, BlockId::AIR);
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
