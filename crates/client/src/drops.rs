//! Dropped items on the client: what the server says lies about, shown as Minecraft shows
//! it (a tiny block, or a plant's sprite, turning and bobbing, a few copies for a big
//! stack), carried on smoothly between the server's updates, and flown into whoever picks
//! it up. Q throws one item from the hand, Ctrl+Q the whole stack.
//!
//! The server owns every item (see `server::items`); this only draws them, through
//! `render::items`.

use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_engine::action::ActionTarget;
use voxmar_engine::block::{BlockId, RenderKind, Sound, Tint};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::dropped::SIZE;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::world::DEFAULT_COLORS;
use voxmar_protocol::{ItemGone, ItemView, PlayerId};

use crate::Actions;
use crate::breaking::sky_light;
use crate::menu::Screen;
use crate::net::{Net, RemotePlayers};
use crate::player::{LocalPlayer, UiState};
use crate::render::items::{CAPACITY, ItemModel, ItemModels, Shape};
use crate::render::particles::TexturePieces;
use crate::sfx::{Cue, SoundCue};
use crate::world::ClientWorld;

/// Seconds an item takes to fly into whoever picked it up (Minecraft's three ticks).
const PICKUP_FLIGHT: f32 = 0.15;
/// The furthest the shown position runs ahead of the server's last word, in seconds of
/// motion, before it waits for the next.
const AHEAD: f32 = 0.1;
/// How quickly the shown position catches up with where the item should be, per second.
const FOLLOW: f32 = 25.0;
/// How high an item floats above where it lies, at the middle of its bob (Minecraft's
/// tenth of a block; the renderer bobs and turns it).
const BOB: f32 = 0.1;
/// How wide a sprite is drawn, against a block's [`SIZE`] (Minecraft draws flat items at
/// half a block).
const SPRITE_SIZE: f32 = 0.5;
/// Items further than this are not drawn.
const DRAWN_WITHIN: f32 = 48.0;
/// Block light reaches this many blocks from a source when lighting an item.
const BLOCK_LIGHT_REACH: i32 = 7;

pub struct DropsPlugin;

impl Plugin for DropsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Drops>()
            .add_systems(Update, (throw, show).chain().run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut drops: ResMut<Drops>, mut models: ResMut<ItemModels>| {
                *drops = Drops::default();
                models.0.clear();
            });
    }
}

/// An item as the client shows it.
struct Shown {
    stack: ItemStack,
    /// Where the server last said it was, how fast it was going, and when that was said.
    told: Vec3,
    velocity: Vec3,
    told_at: f32,
    /// Where it is drawn (bottom centre).
    position: Vec3,
    /// Seconds into its spin and bob when it appeared, so neighbours don't turn in step.
    phase: f32,
    /// Who picked it up and when, while it flies to them.
    collected: Option<(Option<PlayerId>, f32, Vec3)>,
    /// Whether its pickup has popped.
    popped: bool,
    /// Light where it lies, and the cell that was measured at.
    light: [u8; 2],
    lit_cell: Option<IVec3>,
}

/// The dropped items the server has told us about.
#[derive(Resource, Default)]
pub struct Drops {
    items: FxHashMap<u32, Shown>,
}

impl Drops {
    /// The items lying about, not being picked up: id, stack and where the server last
    /// put them (bottom centre).
    pub fn lying(&self) -> impl Iterator<Item = (u32, ItemStack, Vec3)> + '_ {
        self.items.iter().filter(|(_, item)| item.collected.is_none()).map(|(&id, item)| (id, item.stack, item.told))
    }

    /// The items flying to whoever picked them up: id, stack and where they were taken from.
    pub fn picked_up(&self) -> impl Iterator<Item = (u32, ItemStack, Vec3)> + '_ {
        self.items.iter().filter_map(|(&id, item)| item.collected.map(|(_, _, from)| (id, item.stack, from)))
    }

    /// The item lying nearest along the look ray within `within` blocks of the eye, and how
    /// far, met by a box `width` across (an item is small, and hard to aim at). The box is
    /// where the item is drawn, which runs a little behind the server's word while it moves.
    pub fn looked_at(&self, eye: Vec3, look: Vec3, within: f32, width: f32) -> Option<(ItemStack, f32)> {
        let half = Vec3::new(width, width, width) / 2.0;
        self.items
            .values()
            .filter(|item| item.collected.is_none())
            .filter_map(|item| {
                let middle = item.position + Vec3::Y * (SIZE / 2.0);
                let hit = crate::creatures::ray_box(eye, look, &voxmar_engine::physics::Aabb { min: middle - half, max: middle + half })?;
                Some((item.stack, hit))
            })
            .filter(|&(_, distance)| distance <= within)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Applies the server's news, at `now` seconds.
    pub fn receive(&mut self, changed: Vec<ItemView>, gone: Vec<ItemGone>, now: f32) {
        for view in changed {
            let item = self.items.entry(view.id).or_insert_with(|| Shown {
                stack: view.stack,
                told: view.position,
                velocity: view.velocity,
                told_at: now,
                position: view.position,
                phase: (view.id.wrapping_mul(0x9e37_79b9) >> 8) as f32 / (1u32 << 24) as f32 * 10.0,
                collected: None,
                popped: false,
                light: [15, 0],
                lit_cell: None,
            });
            item.stack = view.stack;
            item.told = view.position;
            item.velocity = view.velocity;
            item.told_at = now;
        }
        for gone in gone {
            match gone.collector {
                Some(collector) => {
                    if let Some(item) = self.items.get_mut(&gone.id) {
                        item.collected = Some((Some(collector), now, item.position));
                    }
                }
                None => {
                    self.items.remove(&gone.id);
                }
            }
        }
    }
}

/// Q throws one item from the selected slot, Ctrl+Q the whole stack, as in Minecraft:
/// predicted here, done by the server's `core:drop`.
fn throw(keys: Res<ButtonInput<KeyCode>>, ui: Res<UiState>, net: Res<Net>, actions: Res<Actions>, mut player: ResMut<LocalPlayer>) {
    if !ui.cursor_locked || player.dead || !keys.just_pressed(KeyCode::KeyQ) {
        return;
    }
    let whole = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let slot = player.selected;
    let Some(stack) = player.inventory.get(slot) else { return };
    let count = if whole { stack.count } else { 1 };
    let Some(drop) = actions.named("core:drop") else { return };
    if player.inventory.take(slot, count).is_some() {
        net.propose_from(&player, drop, ActionTarget::Stack { slot: slot as u8, count });
    }
}

/// Moves every item on towards where it should be, and hands the renderer this frame's
/// models.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    time: Res<Time>,
    world: Res<ClientWorld>,
    player: Res<LocalPlayer>,
    remote: Res<RemotePlayers>,
    pieces: Option<Res<TexturePieces>>,
    mut drops: ResMut<Drops>,
    mut models: ResMut<ItemModels>,
    mut cues: MessageWriter<SoundCue>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let eye = player.eye();
    // Where an item flies to: the middle of whoever picked it up.
    let collector_at = |id: Option<PlayerId>| {
        id.and_then(|id| remote.position(id)).map_or(eye - Vec3::Y * 0.6, |feet| feet + Vec3::Y * 1.0)
    };
    drops.items.retain(|_, item| item.collected.is_none_or(|(_, at, _)| now - at < PICKUP_FLIGHT));
    models.0.clear();
    for item in drops.items.values_mut() {
        let position = match item.collected {
            Some((collector, at, from)) => {
                if !item.popped {
                    item.popped = true;
                    cues.write(SoundCue::new(Sound::Stone, Cue::Pickup).at(from));
                }
                let t = ((now - at) / PICKUP_FLIGHT).clamp(0.0, 1.0);
                from.lerp(collector_at(collector), t * t)
            }
            None => {
                let ahead = (now - item.told_at).min(AHEAD);
                let goal = item.told + item.velocity * ahead;
                // Settled once close, so an item lying still shows the very same model
                // every frame and costs the renderer no upload.
                if item.position.distance_squared(goal) < 1e-6 { goal } else { item.position.lerp(goal, 1.0 - (-FOLLOW * dt).exp()) }
            }
        };
        item.position = position;
        if position.distance(eye) > DRAWN_WITHIN {
            continue;
        }
        let cell = (position + Vec3::Y * SIZE / 2.0).floor().as_ivec3();
        if item.lit_cell != Some(cell) {
            item.lit_cell = Some(cell);
            item.light = [sky_light(&world, cell), block_light(&world, cell)];
        }
        push_models(&mut models.0, &world, pieces.as_deref(), item);
    }
    // Too many to draw: the nearest go first.
    if models.0.len() > CAPACITY {
        models.0.sort_by(|a, b| a.center.distance_squared(eye).total_cmp(&b.center.distance_squared(eye)));
        models.0.truncate(CAPACITY);
    }
}

/// How many copies of an item a stack shows, as Minecraft's: one, then more for bigger
/// stacks, up to five.
fn copies(count: u8) -> usize {
    match count {
        0..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    }
}

/// How an item is drawn: its shape, texture layers, which faces take a biome colour and
/// that colour where it is (`at`, a block), and the width of its model.
pub(crate) struct Look {
    pub shape: Shape,
    pub size: f32,
    pub layers: [u16; 6],
    pub tinted: u8,
    pub tint: [u8; 3],
}

/// The look of `item` where it is at `at`: a tiny block for a block, and a plant's sprite
/// (or any item's, drawn as a plant) flat.
pub(crate) fn look_of(world: &ClientWorld, pieces: Option<&TexturePieces>, item: BlockId, at: IVec3) -> Look {
    let info = world.registry.info(item);
    let sprite = info.render == RenderKind::Plant;
    let (shape, size) = if sprite { (Shape::Sprite, SPRITE_SIZE) } else { (Shape::Cube, SIZE) };
    let layers = if sprite { [info.layers[0]; 6] } else { info.layers };
    // Faces that take a biome colour, on their texture or on an overlay (a grass block's
    // side), and the colour where the item lies.
    let mut tinted = 0u8;
    let mut kind = Tint::None;
    for (face, (&layer, &tint)) in layers.iter().zip(&info.tints).enumerate() {
        let tint = match tint {
            Tint::None => pieces.map_or(Tint::None, |p| p.overlay_tint(layer)),
            tint => tint,
        };
        if tint != Tint::None {
            tinted |= 1 << face;
            kind = tint;
        }
    }
    let colors = world
        .voxels
        .column(ColumnPos::of_block(at))
        .map_or(DEFAULT_COLORS, |column| column.colors_at(at.x & (CHUNK_SIZE - 1), at.z & (CHUNK_SIZE - 1)));
    let tint = match kind {
        Tint::None => [255; 3],
        Tint::Grass => colors[0],
        Tint::Foliage => colors[1],
        Tint::Water => colors[2],
    };
    Look { shape, size, layers, tinted, tint }
}

/// The models showing `item`: its copies, turning and bobbing together.
fn push_models(models: &mut Vec<ItemModel>, world: &ClientWorld, pieces: Option<&TexturePieces>, item: &Shown) {
    let Look { shape, size, layers, tinted, tint } = look_of(world, pieces, item.stack.item, item.position.floor().as_ivec3());
    let sprite = shape == Shape::Sprite;
    // Lifted off the ground by the bob, as Minecraft's are.
    let center = item.position + Vec3::Y * (BOB + size / 2.0);
    for copy in 0..copies(item.stack.count) {
        // Copies sit a little apart (a sprite's stack behind one another), in the item's
        // own turning frame.
        let spread = if copy == 0 { Vec3::ZERO } else { scatter(copy) * size * 0.35 };
        let offset = if sprite { Vec3::new(spread.x * 0.3, spread.y * 0.3, -(copy as f32) * size / 16.0) } else { spread };
        models.push(ItemModel { center, phase: item.phase, offset, size, shape, layers, tinted, tint, light: item.light, held: None, posed: None });
    }
}

/// A fixed spread for the `copy`th copy of a stack, each axis -1..1.
fn scatter(copy: usize) -> Vec3 {
    let h = (copy as u32 + 1).wrapping_mul(0x2c1b_3c6d) ^ 0x297a_2d39;
    let unit = |shift: u32| ((h >> shift) & 255) as f32 / 127.5 - 1.0;
    Vec3::new(unit(0), unit(8) * 0.5, unit(16))
}

/// Block light at `cell`, 0..=15: the brightest light source near it, less a level per
/// block of distance, as light spreads through open space.
pub(crate) fn block_light(world: &ClientWorld, cell: IVec3) -> u8 {
    // Only the diamond within reach, a fifth of the cube around it.
    let r = BLOCK_LIGHT_REACH;
    let mut best = 0u8;
    for dy in -r..=r {
        let rz = r - dy.abs();
        for dz in -rz..=rz {
            let rx = rz - dz.abs();
            for dx in -rx..=rx {
                let distance = (dx.abs() + dy.abs() + dz.abs()) as u8;
                let Some(block) = world.block(cell + IVec3::new(dx, dy, dz)) else { continue };
                best = best.max(world.registry.info(block).emission.saturating_sub(distance));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigger_stacks_show_more_copies() {
        assert_eq!([1, 2, 16, 17, 33, 64].map(copies), [1, 2, 2, 3, 4, 5]);
        for copy in 1..5 {
            let s = scatter(copy);
            assert!(s.abs().max_element() <= 1.0 && s != Vec3::ZERO);
        }
    }

    #[test]
    fn picked_up_items_fly_off_then_go() {
        let mut drops = Drops::default();
        let view = ItemView { id: 3, stack: ItemStack::new(voxmar_engine::block::BlockId(1), 2), position: Vec3::ONE, velocity: Vec3::ZERO };
        drops.receive(vec![view], vec![], 0.0);
        drops.receive(vec![], vec![ItemGone { id: 3, collector: Some(7) }], 1.0);
        let item = &drops.items[&3];
        assert_eq!(item.collected.map(|(who, at, _)| (who, at)), Some((Some(7), 1.0)));
        // Gone without a collector (merged, despawned, out of sight): at once.
        drops.receive(vec![ItemView { id: 4, ..view }], vec![ItemGone { id: 4, collector: None }], 1.0);
        assert!(!drops.items.contains_key(&4));
    }

    #[test]
    fn a_lying_item_is_looked_at_within_range_and_aim() {
        let mut drops = Drops::default();
        let stack = ItemStack::new(voxmar_engine::block::BlockId(1), 5);
        drops.receive(vec![ItemView { id: 1, stack, position: Vec3::new(0.0, 0.0, -4.0), velocity: Vec3::ZERO }], vec![], 0.0);
        let eye = Vec3::new(0.0, SIZE / 2.0, 0.0);
        let (seen, distance) = drops.looked_at(eye, Vec3::NEG_Z, 6.0, 0.6).unwrap();
        assert_eq!(seen, stack);
        assert!((distance - 3.7).abs() < 1e-4, "met at the box's near face, not its middle: {distance}");
        // An item still flying is met where it is drawn, not where the server last put it.
        drops.items.get_mut(&1).unwrap().position = Vec3::new(2.0, 0.0, -4.0);
        assert_eq!(drops.looked_at(eye, Vec3::NEG_Z, 6.0, 0.6), None);
        assert!(drops.looked_at(eye + Vec3::X * 2.0, Vec3::NEG_Z, 6.0, 0.6).is_some());
        drops.items.get_mut(&1).unwrap().position = Vec3::new(0.0, 0.0, -4.0);
        // Not at all out of range, or aimed a hand's breadth beside it, or when it is picked up.
        assert_eq!(drops.looked_at(eye, Vec3::NEG_Z, 3.0, 0.6), None);
        assert_eq!(drops.looked_at(eye + Vec3::X, Vec3::NEG_Z, 6.0, 0.6), None);
        drops.receive(vec![], vec![ItemGone { id: 1, collector: Some(7) }], 1.0);
        assert_eq!(drops.looked_at(eye, Vec3::NEG_Z, 6.0, 0.6), None);
    }
}
