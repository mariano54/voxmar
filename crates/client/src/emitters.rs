//! Particles that blocks give off near the player: a campfire's smoke, embers and crackle of
//! sparks, a torch's flickering flame. What a block gives off is its data (`particles` in
//! blocks.ron, [`Emission`]), so a mod's braziers and candles can smoke and spark as well.
//!
//! Twice a second, and whenever the camera has moved a few blocks, the chunks around the
//! camera are searched for the nearest [`MAX_EMITTERS`] emitting blocks within [`RANGE`]. A
//! chunk whose palette holds no emitting block is passed over without reading its blocks,
//! so away from fires the search costs next to nothing. Every frame those emitters spawn
//! particles into the shared ring (`render::particles`), which moves, grows, cools and
//! fades them on the GPU, so the CPU only writes each particle once. Nothing here allocates
//! once its two lists have grown to their size.

use std::sync::Arc;

use bevy::prelude::*;
use voxmar_engine::block::{BlockId, BlockRegistry, Emission, ParticleStyle};
use voxmar_engine::coords::{CHUNK_BITS, CHUNK_SIZE, ColumnPos, WORLD_HEIGHT, local_index};

use crate::breaking::sky_light;
use crate::menu::Screen;
use crate::render::particles::{Motion, Particle, Particles};
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

/// How far from the camera blocks give off particles, in blocks, and how many do at once
/// (the nearest).
pub const RANGE: i32 = 24;
pub const MAX_EMITTERS: usize = 32;
/// Seconds between searches, and how far the camera may move before one comes early.
const RESEARCH: f32 = 0.5;
const MOVED: i32 = 4;
/// How far up smoke looks for a ceiling, which it fades out under rather than rising into.
const HEADROOM: i32 = 8;
/// The fastest wind smoke and embers drift with, in blocks per second.
const MAX_DRIFT: f32 = 6.0;
/// Emitters nearer than this give off all their particles; beyond, fewer, down to
/// `FAR_SHARE` at `RANGE`.
const NEAR: f32 = 10.0;
const FAR_SHARE: f32 = 0.3;

pub struct EmittersPlugin;

impl Plugin for EmittersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Emitters>()
            .add_systems(Update, emit.run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut emitters: ResMut<Emitters>| emitters.near.clear());
    }
}

/// A block near the camera giving off particles, and what it needs to light and bound them.
#[derive(Clone, Copy, Debug)]
struct Emitter {
    pos: IVec3,
    block: BlockId,
    /// Sky light just above it.
    sky: u8,
    /// Open air above it, in blocks, up to `HEADROOM`.
    headroom: f32,
    /// Its share of its particles, by how far it is from the camera: fewer far off, where
    /// they are small.
    share: f32,
}

#[derive(Resource, Default)]
pub(crate) struct Emitters {
    /// The registry `emits` was made from.
    registry: Option<Arc<BlockRegistry>>,
    /// Per block id, whether it gives off particles.
    emits: Vec<bool>,
    /// The nearest emitters, nearest first.
    near: Vec<Emitter>,
    /// The last search's finds by squared distance, kept for its capacity.
    found: Vec<(i32, IVec3, BlockId)>,
    /// Seconds since the last search, and the camera's cell then.
    since: f32,
    searched_at: Option<IVec3>,
    dice: u64,
    /// A block whose particles are counted into `watched` (the gate's campfire check):
    /// a count is the same at any frame rate, where a picture of a few faint puffs is not.
    pub(crate) watch: Option<IVec3>,
    pub(crate) watched: u32,
}

impl Emitters {
    /// Finds the nearest emitting blocks around `eye`.
    fn search(&mut self, world: &ClientWorld, eye: IVec3) {
        self.found.clear();
        let low = (eye - RANGE).max(IVec3::new(i32::MIN, 0, i32::MIN));
        let high = (eye + RANGE).min(IVec3::new(i32::MAX, WORLD_HEIGHT - 1, i32::MAX));
        if low.y > high.y {
            self.near.clear();
            return;
        }
        let chunks = (low >> CHUNK_BITS, high >> CHUNK_BITS);
        for cz in chunks.0.z..=chunks.1.z {
            for cx in chunks.0.x..=chunks.1.x {
                let Some(column) = world.voxels.column(ColumnPos::new(cx, cz)) else { continue };
                for cy in chunks.0.y..=chunks.1.y {
                    let chunk = &column.chunks[cy as usize];
                    if !chunk.palette().iter().any(|b| self.emits.get(b.0 as usize).copied().unwrap_or(false)) {
                        continue;
                    }
                    // The part of the search box inside this chunk, in the chunk's order.
                    let origin = IVec3::new(cx, cy, cz) << CHUNK_BITS;
                    let (from, to) = (low.max(origin) - origin, high.min(origin + (CHUNK_SIZE - 1)) - origin);
                    for y in from.y..=to.y {
                        for z in from.z..=to.z {
                            for x in from.x..=to.x {
                                let local = IVec3::new(x, y, z);
                                let block = chunk.get(local_index(local));
                                if !self.emits[block.0 as usize] {
                                    continue;
                                }
                                let pos = origin + local;
                                let distance = (pos - eye).length_squared();
                                if distance <= RANGE * RANGE {
                                    self.found.push((distance, pos, block));
                                }
                            }
                        }
                    }
                }
            }
        }
        self.found.sort_unstable_by_key(|&(distance, ..)| distance);
        self.found.truncate(MAX_EMITTERS);
        self.near.clear();
        for &(distance, pos, block) in &self.found {
            let far = ((distance as f32).sqrt() - NEAR) / (RANGE as f32 - NEAR);
            let share = 1.0 - (1.0 - FAR_SHARE) * far.clamp(0.0, 1.0);
            let above = pos + IVec3::Y;
            let headroom = (1..=HEADROOM)
                .find(|&dy| world.block(pos + IVec3::Y * dy).is_none_or(|b| world.registry.info(b).solid))
                .unwrap_or(HEADROOM + 1)
                - 1;
            self.near.push(Emitter { pos, block, sky: sky_light(world, above), headroom: headroom as f32 + 0.5, share });
        }
    }

    fn roll(&mut self) -> f32 {
        self.dice = crate::sfx::mix(self.dice.wrapping_add(1));
        (self.dice >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn emit(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    world: Res<ClientWorld>,
    weather: Res<ClientWeather>,
    mut particles: ResMut<Particles>,
    mut emitters: ResMut<Emitters>,
) {
    let Ok(camera) = camera.single() else { return };
    if !emitters.registry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &world.registry)) {
        emitters.emits = (0..world.registry.len()).map(|id| !world.registry.def(BlockId(id as u16)).particles.is_empty()).collect();
        emitters.registry = Some(world.registry.clone());
        emitters.searched_at = None;
    }
    let eye = camera.translation().floor().as_ivec3();
    let dt = time.delta_secs().min(0.1);
    emitters.since += dt;
    let moved = emitters.searched_at.is_none_or(|at| (at - eye).abs().max_element() >= MOVED);
    if moved || emitters.since >= RESEARCH {
        emitters.search(&world, eye);
        emitters.since = 0.0;
        emitters.searched_at = Some(eye);
    }
    if particles.amount <= 0.0 {
        return;
    }
    let wind = weather.current.wind;
    let wind = Vec3::new(wind.x, 0.0, wind.y).clamp_length_max(MAX_DRIFT);
    let amount = particles.amount;
    for i in 0..emitters.near.len() {
        let emitter = emitters.near[i];
        // Gone since the last search: broken, or put out.
        if world.block(emitter.pos) != Some(emitter.block) {
            continue;
        }
        let def = world.registry.def(emitter.block);
        let light = world.registry.info(emitter.block).emission;
        for emission in &def.particles {
            // On average `rate` a second, at random moments.
            let owed = emission.rate.max(0.0) * dt * amount * emitter.share + emitters.roll();
            for _ in 0..owed as u32 {
                for _ in 0..emission.count.max(1) {
                    let particle = particle(emission, &emitter, light, wind, &mut emitters);
                    particles.spawn(&particle);
                    if emitters.watch == Some(emitter.pos) {
                        emitters.watched += 1;
                    }
                }
            }
        }
    }
}

/// One particle of `emission` from `emitter`, as its style makes it.
fn particle(emission: &Emission, emitter: &Emitter, light: u8, wind: Vec3, dice: &mut Emitters) -> Particle {
    let angle = dice.roll() * std::f32::consts::TAU;
    let across = Vec2::from_angle(angle) * dice.roll().sqrt() * emission.spread;
    let position = emitter.pos.as_vec3() + Vec3::from(emission.at) + Vec3::new(across.x, 0.0, across.y);
    let jitter = Vec3::new(dice.roll() - 0.5, 0.0, dice.roll() - 0.5) * 2.0;
    let scale = emission.size.max(0.0);
    let color = |default: [u8; 3], alpha: u8| {
        let [r, g, b] = emission.color.unwrap_or(default);
        [r, g, b, alpha]
    };
    let (r0, r1, r2) = (dice.roll(), dice.roll(), dice.roll());
    let mut particle = match emission.style {
        // Soft grey puffs that rise, slow, swell and drift apart on the wind, thick as they
        // leave the fire and thinning as they go. Lit warm by the fire beneath.
        ParticleStyle::Smoke => Particle {
            position,
            velocity: Vec3::Y * (0.7 + 0.35 * r0) + jitter * 0.06 + wind * 0.2,
            motion: Motion::Rise,
            drag: 0.15,
            size: (0.18 + 0.1 * r1) * scale,
            grow: 3.0,
            wander: 0.3,
            life: 3.5 + 1.5 * r2,
            color: color([150, 144, 138], 140),
            light: [emitter.sky, light.saturating_sub(3)],
            ..default()
        },
        // Glowing motes that float up, flitting, and cool to nothing.
        ParticleStyle::Ember => Particle {
            position,
            velocity: Vec3::Y * (0.9 + 0.9 * r0) + jitter * 0.25 + wind * 0.15,
            motion: Motion::Rise,
            drag: 0.9,
            size: (0.045 + 0.03 * r1) * scale,
            grow: 0.6,
            wander: 0.2,
            life: 1.0 + 1.2 * r2,
            color: color([255, 150, 50], 255),
            glow: true,
            cools: true,
            ..default()
        },
        // Bright pinpricks the fire spits out, which arc over and die on the ground.
        ParticleStyle::Spark => Particle {
            position,
            velocity: Vec3::Y * (3.0 + 1.5 * r0) + jitter * 1.1,
            motion: Motion::Ballistic,
            gravity: 10.0,
            drag: 0.8,
            floor: emitter.pos.y as f32 + 0.04,
            size: (0.03 + 0.015 * r1) * scale,
            grow: 0.7,
            life: 0.45 + 0.4 * r2,
            color: color([255, 205, 110], 255),
            glow: true,
            cools: true,
            ..default()
        },
        // Tongues of flame that lick up, flicker and shrink away, white-hot at first,
        // cooling through orange to red.
        ParticleStyle::Flame => Particle {
            position,
            velocity: Vec3::Y * (0.3 + 0.25 * r0) + jitter * 0.03 + wind * 0.04,
            motion: Motion::Rise,
            size: (0.09 + 0.05 * r1) * scale,
            grow: 0.35,
            wander: 0.02,
            life: 0.18 + 0.18 * r2,
            color: color([255, 170, 50], 255),
            glow: true,
            cools: true,
            ..default()
        },
    };
    // Rising further: faster and for longer, both by the square root, and with drag
    // eased to match, so the distance grows with the height asked for.
    if let Some(height) = emission.height {
        let stretch = (height.max(0.0) / natural_height(emission.style)).sqrt();
        particle.velocity *= stretch;
        particle.life *= stretch;
        particle.wander = (particle.wander * stretch).min(0.63);
        if stretch > 0.0 {
            particle.drag /= stretch;
        }
    }
    // Smoke fades out before any ceiling rather than rising into it.
    if emission.style == ParticleStyle::Smoke {
        particle.life = particle.life.min(emitter.headroom / particle.velocity.y.max(0.1) * 1.4);
    }
    particle
}

/// About how high a style's particles rise on their own, in blocks.
fn natural_height(style: ParticleStyle) -> f32 {
    match style {
        ParticleStyle::Smoke => 2.5,
        ParticleStyle::Ember => 1.4,
        ParticleStyle::Spark => 0.75,
        ParticleStyle::Flame => 0.12,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emission(style: ParticleStyle) -> Emission {
        Emission { style, ..default() }
    }

    #[test]
    fn smoke_fades_under_a_ceiling() {
        let mut dice = Emitters::default();
        let emitter = Emitter { pos: IVec3::new(3, 60, -2), block: BlockId(1), sky: 15, headroom: 1.5, share: 1.0 };
        for _ in 0..50 {
            let smoke = particle(&emission(ParticleStyle::Smoke), &emitter, 13, Vec3::ZERO, &mut dice);
            // It rises at most about its speed times its life.
            assert!(smoke.velocity.y * smoke.life <= 1.5 * 1.4 + 1e-4, "{smoke:?}");
            assert_eq!(smoke.light, [15, 10]);
        }
    }

    #[test]
    fn particles_start_where_the_emission_says() {
        let mut dice = Emitters::default();
        let emitter = Emitter { pos: IVec3::new(10, 70, 10), block: BlockId(1), sky: 0, headroom: 9.0, share: 1.0 };
        let spark = Emission { style: ParticleStyle::Spark, at: [0.5, 0.25, 0.5], spread: 0.2, color: Some([1, 2, 3]), ..default() };
        for _ in 0..50 {
            let p = particle(&spark, &emitter, 13, Vec3::ZERO, &mut dice);
            let offset = p.position - Vec3::new(10.5, 70.25, 10.5);
            assert!(offset.y.abs() < 1e-4 && offset.xz().length() <= 0.2 + 1e-4, "{offset}");
            assert_eq!(p.color, [1, 2, 3, 255]);
            assert!(p.floor <= p.position.y);
        }
    }

    #[test]
    fn taller_flames_rise_further() {
        let mut dice = Emitters::default();
        let emitter = Emitter { pos: IVec3::ZERO, block: BlockId(1), sky: 0, headroom: 9.0, share: 1.0 };
        let reach = |emission: &Emission, dice: &mut Emitters| {
            (0..200).map(|_| particle(emission, &emitter, 13, Vec3::ZERO, dice)).map(|p| p.velocity.y * p.life).sum::<f32>() / 200.0
        };
        let small = reach(&emission(ParticleStyle::Flame), &mut dice);
        let tall = reach(&Emission { height: Some(1.2), ..emission(ParticleStyle::Flame) }, &mut dice);
        assert!((0.08..0.2).contains(&small), "{small}");
        assert!((0.8..1.6).contains(&tall), "{tall}");
    }
}
