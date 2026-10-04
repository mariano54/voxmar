//! What mining and breaking a block looks like, as in Minecraft: cracks spreading over the
//! block being mined, chips falling from the face being hit, and when it breaks, a burst of
//! small pieces of its texture that fly out, fall, hop on the ground and fade.
//!
//! All of it shows what the client already knows and the server decides: the local mining
//! progress (which runs over the same time the server takes), the break the client predicts
//! at the moment the server commits it, and blocks the server reports broken by others.
//! The motion is Minecraft's terrain particles' (gravity 0.04 blocks per tick², 2% drag per
//! tick), worked out on the GPU (see `render::particles`).

use bevy::prelude::*;
use voxmar_engine::block::{BlockId, BlockInfo, RenderKind};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, Face};

use crate::interact::{InteractSystems, Target};
use crate::menu::Screen;
use crate::render::cracks::{Crack, STAGES};
use crate::render::pack::ActivePack;
use crate::render::particles::{Look, Motion, Particle, Particles, TexturePieces};
use crate::settings::Settings;
use crate::world::ClientWorld;

/// Blocks per second squared, and the share of speed lost per second.
const GRAVITY: f32 = 16.0;
const DRAG: f32 = 0.4;
/// Pieces along each side of a full block: 64 in all.
const PIECES: u32 = 4;
/// Chips per second from the face being mined.
const CHIPS: f32 = 12.0;
/// Bursts at most per frame, and how far away others' breaks still show.
const MAX_BURSTS: usize = 16;
pub const SHOWN_WITHIN: f32 = 32.0;
/// How far up an open sky is looked for, and how far down a floor.
const SKY_SCAN: i32 = 48;
const FLOOR_SCAN: i32 = 8;

pub struct BreakingPlugin;

impl Plugin for BreakingPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<BlockBroken>()
            .add_systems(
                Update,
                (cracks, chips, bursts).in_set(EffectSystems).after(InteractSystems).run_if(in_state(Screen::InGame)),
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
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> f32 {
        self.0 = crate::sfx::mix(self.0.wrapping_add(1));
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// -1..1 on each axis.
    fn spread(&mut self) -> Vec3 {
        Vec3::new(self.roll(), self.roll(), self.roll()) * 2.0 - 1.0
    }
}

fn cracks(target: Res<Target>, world: Res<ClientWorld>, mut crack: ResMut<Crack>) {
    let at = target.breaking.filter(|&(pos, progress)| {
        progress > 0.0 && world.block(pos).is_some_and(|b| !matches!(world.registry.info(b).render, RenderKind::Plant | RenderKind::Invisible))
    });
    let at = at.map(|(pos, progress)| (pos, ((progress * STAGES as f32) as u32).min(STAGES - 1)));
    if crack.at != at {
        crack.at = at;
    }
}

/// What `chips` keeps between frames: the chips owed so far and the light outside the face
/// being mined, worked out once per face.
#[derive(Default)]
struct Chipping {
    owed: f32,
    dice: Dice,
    light: Option<(IVec3, u8)>,
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
        return;
    };
    let Some(block) = world.block(pos).filter(|_| hit.block == pos) else { return };
    let info = world.registry.info(block);
    *owed += time.delta_secs() * CHIPS * particles.amount;
    let outside = pos + hit.face.normal();
    let light = match *lit {
        Some((at, light)) if at == outside => light,
        _ => {
            let light = sky_light(&world, outside);
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
        let light = sky_light(&world, pos);
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
fn piece(
    world: &ClientWorld,
    pieces: &TexturePieces,
    dice: &mut Dice,
    pos: IVec3,
    info: &BlockInfo,
    face: Face,
    sky: u8,
    position: Vec3,
    velocity: Vec3,
    size: f32,
) -> Particle {
    let layer = info.layers[face as usize];
    let tint = info.tints[face as usize];
    let climate = world
        .voxels
        .column(ColumnPos::of_block(pos))
        .map_or([128, 128], |column| column.climate_at(pos.x & (CHUNK_SIZE - 1), pos.z & (CHUNK_SIZE - 1)));
    Particle {
        position,
        velocity,
        motion: Motion::Ballistic,
        gravity: GRAVITY,
        drag: DRAG,
        // Resting mostly above the ground, as a flat piece would.
        floor: landing_floor(world, position, velocity) + size * 0.35,
        size,
        look: Look::Texture { layer, corner: pieces.corner(layer, dice.roll()), span: 0.25, tint, climate },
        light: [sky, info.emission],
        ..default()
    }
}

/// The height a particle thrown from `position` lands on: the ground below where it comes
/// down, or below where it starts if that is a wall it would fly into.
fn landing_floor(world: &ClientWorld, position: Vec3, velocity: Vec3) -> f32 {
    let below_start = ground_below(world, position);
    let drop = (position.y - below_start).max(0.0);
    let land = (velocity.y + (velocity.y * velocity.y + 2.0 * GRAVITY * drop).sqrt()) / GRAVITY;
    let travel = velocity.xz() * (1.0 - (-DRAG * land).exp()) / DRAG;
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

/// Sky light at a cell, 0..=15, judged from the columns around it: full where nothing
/// blocks the sky straight above, one less for each block sideways to the nearest open
/// column, and none further in. Close enough to light a few particles like the terrain
/// around them.
fn sky_light(world: &ClientWorld, cell: IVec3) -> u8 {
    let open = |x: i32, z: i32| {
        (0..SKY_SCAN).all(|dy| !world.block(IVec3::new(x, cell.y + dy, z)).is_some_and(|b| world.registry.info(b).occludes))
    };
    for reach in 0..=3i32 {
        for dx in -reach..=reach {
            let dz = reach - dx.abs();
            if open(cell.x + dx, cell.z + dz) || (dz != 0 && open(cell.x + dx, cell.z - dz)) {
                return 15 - reach as u8;
            }
        }
    }
    0
}
