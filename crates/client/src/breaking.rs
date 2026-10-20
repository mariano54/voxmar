//! What mining and breaking a block looks and sounds like, as in Minecraft: cracks
//! spreading over the block being mined, chips falling from the face being hit, and when it
//! breaks, its material's break sound and a burst of small pieces of its texture that fly
//! out, fall, hop on the ground and fade.
//!
//! All of it shows what the client already knows and the server decides: the local mining
//! progress (which runs over the same time the server takes), the break the client predicts
//! at the moment the server commits it, and blocks the server reports broken that the
//! client did not predict (by others, or by the player when the server's word came first).
//! Every broken block is one [`BlockBroken`], whichever came first, so each sounds and
//! bursts exactly once.
//! The motion is Minecraft's terrain particles' (gravity 0.04 blocks per tick², 2% drag per
//! tick), worked out on the GPU (see `render::particles`).

use bevy::prelude::*;
use voxmar_engine::block::{BlockId, BlockInfo, RenderKind, Tint};
use voxmar_engine::world::DEFAULT_COLORS;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, Face};

use crate::interact::{InteractSystems, Target};
use crate::menu::Screen;
use crate::render::cracks::{Crack, STAGES};
use crate::render::pack::ActivePack;
use crate::render::particles::{Look, Motion, Particle, Particles, TexturePieces};
use crate::settings::Settings;
use crate::sfx::{Cue, SoundCue};
use crate::world::ClientWorld;

/// Blocks per second squared, and the share of speed lost per second.
const GRAVITY: f32 = 16.0;
const DRAG: f32 = 0.4;
/// Pieces along each side of a full block: 64 in all.
const PIECES: u32 = 4;
/// Chips per second from the face being mined.
const CHIPS: f32 = 12.0;
/// Bursts and break sounds at most per frame, and how far away others' breaks still show.
const MAX_BURSTS: usize = 16;
const MAX_SOUNDS: usize = 4;
pub const SHOWN_WITHIN: f32 = 32.0;
/// How far up an open sky is looked for, and how far down a floor.
const SKY_SCAN: i32 = 48;
const FLOOR_SCAN: i32 = 8;
/// How many blocks sideways sky light is looked for.
const SKY_SPREAD: u8 = 3;

pub struct BreakingPlugin;

impl Plugin for BreakingPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<BlockBroken>()
            .add_systems(
                Update,
                (cracks, chips, bursts, sounds).in_set(EffectSystems).after(InteractSystems).run_if(in_state(Screen::InGame)),
            )
            .add_systems(Update, amount.run_if(resource_changed::<Settings>.or_eager(resource_changed::<ActivePack>)))
            .add_systems(OnExit(Screen::InGame), |mut crack: ResMut<Crack>| crack.at = None);
    }
}

/// The effects, which follow targeting and mining.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EffectSystems;

/// A block that broke: by the player (as predicted) or, by the server's word, someone else.
#[derive(Message, Clone, Copy, Debug)]
pub struct BlockBroken {
    pub pos: IVec3,
    pub block: BlockId,
}

/// Whether breaking a block is something to see: not air, and not water.
pub fn shows_breaking(info: &BlockInfo) -> bool {
    info.render != RenderKind::Invisible && (info.solid || info.render != RenderKind::Translucent)
}

/// The player's particle setting times the shader pack's amounts.
fn amount(settings: Res<Settings>, pack: Res<ActivePack>, mut particles: ResMut<Particles>, mut crack: ResMut<Crack>) {
    particles.amount = settings.particles.share() * pack.particles.max(0.0);
    crack.strength = pack.cracks.clamp(0.0, 1.0);
}

/// Cheap random numbers; effects need no determinism.
#[derive(Default)]
pub(crate) struct Dice(u64);

impl Dice {
    pub(crate) fn roll(&mut self) -> f32 {
        self.0 = crate::sfx::mix(self.0.wrapping_add(1));
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// -1..1 on each axis.
    pub(crate) fn spread(&mut self) -> Vec3 {
        Vec3::new(self.roll(), self.roll(), self.roll()) * 2.0 - 1.0
    }
}

fn cracks(target: Res<Target>, world: Res<ClientWorld>, mut crack: ResMut<Crack>, mut lit: Local<Option<(IVec3, [u8; 2])>>) {
    let at = target.breaking.filter(|&(pos, progress)| {
        progress > 0.0 && world.block(pos).is_some_and(|b| !matches!(world.registry.info(b).render, RenderKind::Plant | RenderKind::Invisible))
    });
    let at = at.map(|(pos, progress)| (pos, ((progress * STAGES as f32) as u32).min(STAGES - 1)));
    if crack.at != at {
        crack.at = at;
    }
    // The cracks are lit by what lights the face being mined: the air in front of it, worked
    // out once per face rather than every frame, and again at the next stint (a torch may
    // have been set meanwhile).
    let Some((block, _)) = at else {
        *lit = None;
        return;
    };
    let outside = match target.hit {
        Some(hit) if hit.block == block => block + hit.face.normal(),
        _ => block,
    };
    let light = match *lit {
        Some((cell, light)) if cell == outside => light,
        _ => {
            let light = light_at(&world, outside);
            *lit = Some((outside, light));
            light
        }
    };
    if crack.light != light {
        crack.light = light;
    }
}

/// What `chips` keeps between frames: the chips owed so far and the light outside the face
/// being mined, worked out once per face.
#[derive(Default)]
struct Chipping {
    owed: f32,
    dice: Dice,
    light: Option<(IVec3, [u8; 2])>,
}

/// Chips falling from the face being mined, a dozen a second.
fn chips(
    time: Res<Time>,
    target: Res<Target>,
    world: Res<ClientWorld>,
    pieces: Option<Res<TexturePieces>>,
    mut particles: ResMut<Particles>,
    mut state: Local<Chipping>,
) {
    let Chipping { owed, dice, light: lit } = &mut *state;
    let (Some(pieces), Some(hit), Some((pos, _))) = (pieces, target.hit, target.breaking) else {
        *owed = 0.0;
        *lit = None;
        return;
    };
    let Some(block) = world.block(pos).filter(|_| hit.block == pos) else { return };
    let info = world.registry.info(block);
    *owed += time.delta_secs() * CHIPS * particles.amount;
    let outside = pos + hit.face.normal();
    let light = match *lit {
        Some((at, light)) if at == outside => light,
        _ => {
            let light = light_at(&world, outside);
            *lit = Some((outside, light));
            light
        }
    };
    while *owed >= 1.0 {
        *owed -= 1.0;
        let normal = hit.face.normal().as_vec3();
        // Somewhere on the face, a tenth of a block out from it, popping up a little and
        // spreading a fifth as fast as a burst's pieces, as Minecraft's do.
        let across = (dice.spread() * 0.4) * (Vec3::ONE - normal.abs());
        let position = pos.as_vec3() + 0.5 + normal * 0.6 + across;
        let speed = (dice.roll() + dice.roll() + 1.0) * 0.15 * 0.4 * 20.0;
        let velocity = dice.spread().normalize_or(Vec3::Y) * speed * 0.2 + Vec3::Y * 2.0;
        let size = 0.06 + 0.06 * dice.roll();
        let mut particle = piece(&world, &pieces, dice, pos, info, hit.face, light, position, velocity, size);
        particle.life = 0.3 + 0.5 * dice.roll();
        particles.spawn(&particle);
    }
}

/// Break sounds, from where each block was: a few a frame at most, so a command that
/// clears a room does not roar.
fn sounds(mut broken: MessageReader<BlockBroken>, world: Res<ClientWorld>, mut cues: MessageWriter<SoundCue>) {
    for &BlockBroken { pos, block } in broken.read().take(MAX_SOUNDS) {
        cues.write(SoundCue::for_block(&world, block, Cue::Break).at(pos.as_vec3() + 0.5));
    }
}

/// A burst of pieces for every block that broke near the camera.
fn bursts(
    mut broken: MessageReader<BlockBroken>,
    world: Res<ClientWorld>,
    pieces: Option<Res<TexturePieces>>,
    mut particles: ResMut<Particles>,
    mut dice: Local<Dice>,
) {
    let Some(pieces) = pieces else { return };
    for &BlockBroken { pos, block } in broken.read().take(MAX_BURSTS) {
        let info = world.registry.info(block);
        if !shows_breaking(info) || particles.amount <= 0.0 {
            continue;
        }
        let light = light_at(&world, pos);
        // A plant is a narrower, lower shape, and breaks into fewer pieces.
        let (low, high, counts) = if info.render == RenderKind::Plant {
            (Vec3::new(0.2, 0.0, 0.2), Vec3::new(0.8, 0.8, 0.8), UVec3::new(3, 3, 3))
        } else {
            (Vec3::ZERO, Vec3::ONE, UVec3::splat(PIECES))
        };
        for i in 0..counts.element_product() {
            if dice.roll() >= particles.amount {
                continue;
            }
            let cell = UVec3::new(i % counts.x, i / counts.x % counts.y, i / (counts.x * counts.y));
            let offset = low + (cell.as_vec3() + 0.5) / counts.as_vec3() * (high - low);
            // Minecraft's: outwards from the middle with some randomness, a speed of 0.06
            // to 0.18 blocks a tick, and a lift of 0.1.
            let speed = (dice.roll() + dice.roll() + 1.0) * 0.15 * 0.4 * 20.0;
            let direction = (offset - 0.5 + dice.spread() * 0.4).normalize_or(Vec3::Y);
            let velocity = direction * speed + Vec3::Y * 2.0;
            let size = 0.1 + 0.1 * dice.roll();
            let mut particle = piece(&world, &pieces, &mut dice, pos, info, Face::PosZ, light, pos.as_vec3() + offset, velocity, size);
            particle.life = 0.5 + 1.0 * dice.roll();
            particles.spawn(&particle);
        }
    }
}

/// A falling piece of `block`'s texture on `face`, with its floor found below where it
/// will land.
#[allow(clippy::too_many_arguments)]
pub(crate) fn piece(
    world: &ClientWorld,
    pieces: &TexturePieces,
    dice: &mut Dice,
    pos: IVec3,
    info: &BlockInfo,
    face: Face,
    light: [u8; 2],
    position: Vec3,
    velocity: Vec3,
    size: f32,
) -> Particle {
    let layer = info.layers[face as usize];
    // Tinted faces take the block's biome colour, as the terrain does.
    let colors = world
        .voxels
        .column(ColumnPos::of_block(pos))
        .map_or(DEFAULT_COLORS, |column| column.colors_at(pos.x & (CHUNK_SIZE - 1), pos.z & (CHUNK_SIZE - 1)));
    // An untinted face with a tinted overlay (a grass block's side) gives its colour to the
    // overlay, which the particle shader draws over the piece.
    let tint = match info.tints[face as usize] {
        Tint::None => pieces.overlay_tint(layer),
        tint => tint,
    };
    let [r, g, b] = match tint {
        Tint::None => [255; 3],
        Tint::Grass => colors[0],
        Tint::Foliage => colors[1],
        Tint::Water => colors[2],
    };
    Particle {
        position,
        velocity,
        motion: Motion::Ballistic,
        gravity: GRAVITY,
        drag: DRAG,
        // Resting mostly above the ground, as a flat piece would.
        floor: landing_floor(world, position, velocity) + size * 0.35,
        size,
        look: Look::Texture { layer, corner: pieces.corner(layer, dice.roll()), span: 0.25 },
        color: [r, g, b, 255],
        // Lit as the terrain there is: by the sky, and by torches and glowing blocks near, or
        // by its own glow if that is more.
        light: [light[0], light[1].max(info.emission)],
        ..default()
    }
}

/// The height a particle thrown from `position` lands on: the ground below where it comes
/// down, or below where it starts if that is a wall it would fly into.
fn landing_floor(world: &ClientWorld, position: Vec3, velocity: Vec3) -> f32 {
    landing_floor_with(world, position, velocity, GRAVITY, DRAG)
}

/// [`landing_floor`] for a particle falling with its own `gravity` and `drag`.
pub(crate) fn landing_floor_with(world: &ClientWorld, position: Vec3, velocity: Vec3, gravity: f32, drag: f32) -> f32 {
    let below_start = ground_below(world, position);
    let drop = (position.y - below_start).max(0.0);
    let gravity = gravity.max(0.01);
    let land = (velocity.y + (velocity.y * velocity.y + 2.0 * gravity * drop).sqrt()) / gravity;
    let travel = if drag > 0.0 { velocity.xz() * (1.0 - (-drag * land).exp()) / drag } else { velocity.xz() * land };
    let landing = position + Vec3::new(travel.x, 0.0, travel.y);
    if landing.xz().floor() == position.xz().floor() {
        return below_start;
    }
    let below_landing = ground_below(world, landing);
    if below_landing <= position.y { below_landing } else { below_start }
}

/// The top of the first solid block at or below `position`, or a long way down if none is
/// near.
fn ground_below(world: &ClientWorld, position: Vec3) -> f32 {
    let cell = position.floor().as_ivec3();
    (0..FLOOR_SCAN)
        .map(|depth| cell - IVec3::Y * depth)
        .find(|&below| world.block(below).is_some_and(|b| world.registry.info(b).solid))
        .map_or(position.y - FLOOR_SCAN as f32 * 2.0, |below| below.y as f32 + 1.0)
}

/// Sky and block light at a cell, as the terrain around it is lit: pieces breaking from a
/// block in a cave by a torch are lit by that torch, in a dark cave dark, and at night by
/// the moon.
pub(crate) fn light_at(world: &ClientWorld, cell: IVec3) -> [u8; 2] {
    [sky_light(world, cell), crate::drops::block_light(world, cell)]
}

/// Sky light at a cell, 0..=15, judged from the columns around it as the engine lights
/// them: sky light falls straight down, losing what water and leaves take and stopping at
/// solid blocks, then spreads sideways one level a block, through what lets light by. A
/// column behind a wall gives none, so a cave a block or two inside a cliff stays dark.
/// Close enough to light a few particles and cracks like the terrain around them.
pub(crate) fn sky_light(world: &ClientWorld, cell: IVec3) -> u8 {
    let column = |x: i32, z: i32| {
        let mut level = 15u8;
        for dy in 0..SKY_SCAN {
            // Unloaded space above is open sky.
            let Some(block) = world.block(IVec3::new(x, cell.y + dy, z)) else { break };
            let attenuation = world.registry.info(block).attenuation;
            if attenuation >= 15 {
                return 0;
            }
            level = level.saturating_sub(attenuation - 1);
        }
        level
    };
    // Outwards from the cell over the cells at its height that light passes, nearest first.
    let passes = |at: IVec3| world.block(at).is_none_or(|block| world.registry.info(block).attenuation < 15);
    let mut reached = vec![IVec3::ZERO];
    let mut ring = vec![IVec3::ZERO];
    let mut best = column(cell.x, cell.z);
    for reach in 1..=SKY_SPREAD {
        // Columns further away give at most one level less.
        if best >= 15 - reach {
            break;
        }
        let mut next = Vec::new();
        for from in ring {
            for step in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
                let at = from + step;
                if reached.contains(&at) || !passes(cell + at) {
                    continue;
                }
                reached.push(at);
                next.push(at);
                best = best.max(column(cell.x + at.x, cell.z + at.z).saturating_sub(reach));
            }
        }
        ring = next;
    }
    best
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::world::Column;

    use super::*;
    use crate::render::terrain::Terrain;

    #[test]
    fn sky_light_spreads_sideways_but_not_through_a_wall() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let stone = content.blocks.expect("base:stone");
        let mut world = ClientWorld::new(content.blocks.clone(), 2, Terrain::detached());
        world.insert_column(ColumnPos::of_block(IVec3::ZERO), Arc::new(Column::default()));
        // A roof over the cell and all around, but for one column two blocks away.
        for x in -2..12 {
            for z in -2..12 {
                if (x, z) != (3, 5) {
                    world.set_block(IVec3::new(x, 101, z), stone);
                }
            }
        }
        let cell = IVec3::new(5, 100, 5);
        assert_eq!(sky_light(&world, IVec3::new(3, 100, 5)), 15);
        assert_eq!(sky_light(&world, cell), 13);
        // A wall between them: the open column is only reached the long way round, four
        // blocks, out of reach.
        world.set_block(IVec3::new(4, 100, 5), stone);
        assert_eq!(sky_light(&world, cell), 0);
    }
}
