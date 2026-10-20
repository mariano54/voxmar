//! Particles that blocks give off near the player: a campfire's smoke, embers and crackle of
//! sparks, a torch's flickering flame. What a block gives off is its data (`particles` in
//! blocks.ron, [`Emission`]), so a mod's braziers and candles can smoke and spark as well.
//!
//! Twice a second, and whenever the camera has moved a few blocks, the chunks around the
//! camera are searched for the nearest [`MAX_EMITTERS`] emitting blocks within [`RANGE`], or
//! an emission's own `range` (a chimney's smoke is seen from afar), nearness counted as a
//! share of that range. A chunk whose palette holds no block seen from that far is passed
//! over without reading its blocks, so away from fires the search costs next to nothing.
//! An emission `over` a tag is given off only with such a block below and open air above
//! (a chimney smokes over a hearth, from its top), and over a structure's core only while
//! its fire burns (`ClientWorld::fires`): a cold hearth's chimney does not smoke. Chimney
//! pieces (`chimney_piece`: a flue, a hood, a pot) stacked straight over an emitting block
//! carry what it gives off up to their top, so a chimney carried up through a roof smokes
//! from its pot, not under the roof. Thatch
//! smouldering by a flame (`ClientWorld::smoulders`) gives off fire.ron's smoke. Every frame those emitters spawn
//! particles into the shared ring (`render::particles`), which moves, grows, cools and
//! fades them on the GPU, so the CPU only writes each particle once. Nothing here allocates
//! once its two lists have grown to their size.

use std::sync::Arc;

use bevy::prelude::*;
use voxmar_engine::block::{BlockId, BlockRegistry, CHIMNEY_PIECE_TAG, Emission, ParticleBlend, ParticleStyle};
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
/// The opacity, so the share of its light, an added flame gives (of 255).
const FLAME_LIGHT: u8 = 150;
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
    /// Where its particles come out: the block's cell, or the top of the flue over it.
    pos: IVec3,
    block: BlockId,
    /// The emitting block's own cell.
    from: IVec3,
    /// Sky light just above it.
    sky: u8,
    /// Open air above it, in blocks, up to `HEADROOM`.
    headroom: f32,
    /// Its share of its particles, by how far it is from the camera: fewer far off, where
    /// they are small.
    share: f32,
    /// Which of its emissions it gives off now, a bit each (the first eight): those seen
    /// from this far, and with what they need below them.
    giving: u8,
}

impl Emitter {
    /// Whether its block still stands where it was found (not broken or put out since the
    /// last search), judged at the block, not at the top of a flue over it.
    fn standing(&self, world: &ClientWorld) -> bool {
        world.block(self.from) == Some(self.block)
    }
}

#[derive(Resource, Default)]
pub(crate) struct Emitters {
    /// The registry `emits` was made from.
    registry: Option<Arc<BlockRegistry>>,
    /// Per block id, how far off its particles are seen, in blocks (0: it gives off none),
    /// and the farthest of them.
    emits: Vec<i32>,
    far: i32,
    /// Per block id, whether it is a structure's core (a hearthstone), which an emission
    /// is `over` only while it burns.
    cores: Vec<bool>,
    /// The nearest emitters, nearest first.
    near: Vec<Emitter>,
    /// The last search's finds by squared distance as a share of their range's (scaled to
    /// `RANGE`), with their squared distance, kept for its capacity.
    found: Vec<(i64, IVec3, BlockId, i32)>,
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
        let far = self.far.max(RANGE);
        let low = (eye - far).max(IVec3::new(i32::MIN, 0, i32::MIN));
        let high = (eye + far).min(IVec3::new(i32::MAX, WORLD_HEIGHT - 1, i32::MAX));
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
                    let origin = IVec3::new(cx, cy, cz) << CHUNK_BITS;
                    let nearest = (eye.clamp(origin, origin + (CHUNK_SIZE - 1)) - eye).length_squared();
                    if !chunk.palette().iter().any(|b| self.emits.get(b.0 as usize).is_some_and(|&r| r > 0 && r * r >= nearest)) {
                        continue;
                    }
                    // The part of the search box inside this chunk, in the chunk's order.
                    let (from, to) = (low.max(origin) - origin, high.min(origin + (CHUNK_SIZE - 1)) - origin);
                    for y in from.y..=to.y {
                        for z in from.z..=to.z {
                            for x in from.x..=to.x {
                                let local = IVec3::new(x, y, z);
                                let block = chunk.get(local_index(local));
                                let range = self.emits[block.0 as usize];
                                if range == 0 {
                                    continue;
                                }
                                let pos = origin + local;
                                let distance = (pos - eye).length_squared();
                                if distance <= range * range {
                                    let share = distance as i64 * (RANGE * RANGE) as i64 / (range * range) as i64;
                                    self.found.push((share, pos, block, distance));
                                }
                            }
                        }
                    }
                }
            }
        }
        self.found.sort_unstable_by_key(|&(share, ..)| share);
        self.near.clear();
        for &(scaled, pos, block, distance) in &self.found {
            if self.near.len() == MAX_EMITTERS {
                break;
            }
            let far = ((scaled as f32).sqrt() - NEAR) / (RANGE as f32 - NEAR);
            let share = 1.0 - (1.0 - FAR_SHARE) * far.clamp(0.0, 1.0);
            // A block's smoke comes out of the top of the flue over it, its flames, embers and
            // sparks where it stands (what it is `over` is judged from there either way).
            let top = flue_top(world, pos);
            let smoke = world.registry.def(block).particles.iter().take(8).enumerate().filter(|(_, e)| e.style == ParticleStyle::Smoke).fold(0u8, |bits, (i, _)| bits | 1 << i);
            let places = if top == pos { [(pos, u8::MAX), (pos, 0)] } else { [(pos, !smoke), (top, smoke)] };
            for (at, emissions) in places {
                if emissions == 0 || self.near.len() == MAX_EMITTERS {
                    continue;
                }
                let headroom = (1..=HEADROOM).find(|&dy| world.block(at + IVec3::Y * dy).is_none_or(|b| world.registry.info(b).solid)).unwrap_or(HEADROOM + 1) - 1;
                let giving = giving(world, &self.cores, pos, block, distance, headroom) & emissions;
                if giving != 0 {
                    self.near.push(Emitter { pos: at, block, from: pos, sky: sky_light(world, at + IVec3::Y), headroom: headroom as f32 + 0.5, share, giving });
                }
            }
        }
    }

    /// Starts counting `pos`'s particles into `watched`, finding emitters again at once so
    /// a block placed moments ago is among them.
    pub(crate) fn watch(&mut self, pos: IVec3) {
        self.watch = Some(pos);
        self.watched = 0;
        self.searched_at = None;
    }

    /// How many particles `pos` gives off a second on average, as found now (its giving
    /// emissions' rates, at `amount` and its share of the budget): what the gate waits on.
    pub(crate) fn rate_of(&self, pos: IVec3, world: &ClientWorld, amount: f32) -> f32 {
        self.near.iter().filter(|e| e.pos == pos).map(|e| {
            let def = world.registry.def(e.block);
            def.particles.iter().enumerate().filter(|&(i, _)| i < 8 && e.giving & (1 << i) != 0).map(|(_, emission)| emission.rate.max(0.0) * emission.count.max(1) as f32).sum::<f32>() * amount * e.share
        }).sum()
    }

    /// Why a watched block might give off nothing, for the gate's message.
    pub(crate) fn why_quiet(&self, pos: IVec3, amount: f32) -> String {
        let found = self.near.iter().any(|e| e.pos == pos);
        format!("found among {} nearby emitters: {found}; particle amount {amount:.2}", self.near.len())
    }

    fn roll(&mut self) -> f32 {
        self.dice = crate::sfx::mix(self.dice.wrapping_add(1));
        (self.dice >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn emit(
    time: Res<Time>,
    launch: Res<crate::session::Launch>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    world: Res<ClientWorld>,
    weather: Res<ClientWeather>,
    mut particles: ResMut<Particles>,
    mut emitters: ResMut<Emitters>,
) {
    let Ok(camera) = camera.single() else { return };
    if !emitters.registry.as_ref().is_some_and(|r| Arc::ptr_eq(r, &world.registry)) {
        emitters.emits = (0..world.registry.len()).map(|id| world.registry.def(BlockId(id as u16)).particles.iter().map(reach).max().unwrap_or(0)).collect();
        emitters.far = emitters.emits.iter().copied().max().unwrap_or(0);
        emitters.cores = cores(&world.registry, &launch.content.multiblocks);
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
        if !emitter.standing(&world) {
            continue;
        }
        let def = world.registry.def(emitter.block);
        let light = world.registry.info(emitter.block).emission;
        for (i, emission) in def.particles.iter().enumerate() {
            if i >= 8 || emitter.giving & (1 << i) == 0 {
                continue;
            }
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
    // Thatch and wattle smouldering by a flame (`ClientWorld::smoulders`, from the server)
    // give off the fire's smoke (fire.ron), whatever block they are: few, and only near.
    let smoke = &launch.content.fire.rules.smoke;
    if smoke.is_empty() {
        return;
    }
    for &pos in &world.smoulders {
        let distance = (pos - eye).length_squared();
        if distance > RANGE * RANGE {
            continue;
        }
        let Some(block) = world.block(pos) else { continue };
        let far = ((distance as f32).sqrt() - NEAR) / (RANGE as f32 - NEAR);
        let share = 1.0 - (1.0 - FAR_SHARE) * far.clamp(0.0, 1.0);
        let headroom = (1..=HEADROOM)
            .find(|&dy| world.block(pos + IVec3::Y * dy).is_none_or(|b| world.registry.info(b).solid))
            .unwrap_or(HEADROOM + 1)
            - 1;
        let emitter = Emitter { pos, block, from: pos, sky: sky_light(&world, pos + IVec3::Y), headroom: headroom as f32 + 0.5, share, giving: u8::MAX };
        for emission in smoke {
            let owed = emission.rate.max(0.0) * dt * amount * share + emitters.roll();
            for _ in 0..owed as u32 {
                for _ in 0..emission.count.max(1) {
                    let particle = particle(emission, &emitter, 0, wind, &mut emitters);
                    particles.spawn(&particle);
                    if emitters.watch == Some(pos) {
                        emitters.watched += 1;
                    }
                }
            }
        }
    }
}

/// The top of the chimney pieces stacked straight over `pos`, however tall (an unloaded
/// cell ends it), or `pos` itself with none over it: where its smoke comes out.
fn flue_top(world: &ClientWorld, pos: IVec3) -> IVec3 {
    let piece = |at: IVec3| world.block(at).is_some_and(|b| world.registry.def(b).has_tag(CHIMNEY_PIECE_TAG));
    (1..).take_while(|&dy| piece(pos + IVec3::Y * dy)).last().map_or(pos, |dy| pos + IVec3::Y * dy)
}

/// Per block id, whether some structure has it as its core.
fn cores(registry: &BlockRegistry, multiblocks: &voxmar_engine::multiblock::MultiblockRegistry) -> Vec<bool> {
    (0..registry.len()).map(|id| multiblocks.with_core(BlockId(id as u16)).next().is_some()).collect()
}

/// How far off an emission is seen, in whole blocks.
fn reach(emission: &Emission) -> i32 {
    emission.range.map_or(RANGE, |range| range.round() as i32)
}

/// Which of `block`'s emissions at `pos`, `distance` squared from the camera with
/// `headroom` blocks of air above, it gives off: those seen from this far, and with what
/// they are `over` below them and open air above.
fn giving(world: &ClientWorld, cores: &[bool], pos: IVec3, block: BlockId, distance: i32, headroom: i32) -> u8 {
    let mut giving = 0;
    for (i, emission) in world.registry.def(block).particles.iter().enumerate().take(8) {
        let seen = reach(emission).pow(2) >= distance;
        let over = emission.over.as_ref().is_none_or(|(tag, reach)| {
            headroom > 0
                && (1..=*reach as i32).any(|dy| {
                    (-1..=1).any(|dx| {
                        (-1..=1).any(|dz| {
                            let at = pos + IVec3::new(dx, -dy, dz);
                            world.block(at).is_some_and(|b| {
                                world.registry.def(b).has_tag(tag) && (!cores.get(b.0 as usize).copied().unwrap_or(false) || world.fires.contains(&at))
                            })
                        })
                    })
                })
        });
        if seen && over {
            giving |= 1 << i;
        }
    }
    giving
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
            // Soft greys, each puff a little lighter or darker than the last.
            color: {
                let grey = (138.0 + 24.0 * (dice.roll() - 0.5)) as u8;
                color([grey, grey - 4, grey - 8], 140)
            },
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
    particle.blend = emission.blend.unwrap_or(emission.style.blend());
    particle.hides = emission.hides.unwrap_or(emission.style.hides());
    // Added light builds up where flames overlap: each gives a share, so a crowd of
    // tongues glows as one fire instead of burning out to white.
    if emission.style == ParticleStyle::Flame && particle.blend == ParticleBlend::Add {
        particle.color[3] = FLAME_LIGHT;
    }
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

    /// A lit hearth's chimney smokes from its top, seen from far beyond a torch's flame; a
    /// cold one does not smoke, nor one whose hearthstone has gone.
    #[test]
    fn a_chimney_smokes_over_a_hearth_from_afar() {
        use voxmar_engine::world::Column;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let block = |name: &str| content.blocks.expect(name);
        let mut world = ClientWorld::new(content.blocks.clone(), 2, crate::render::terrain::Terrain::detached());
        world.insert_column(ColumnPos::of_block(IVec3::ZERO), Arc::new(Column::default()));
        let (hearth, low, top) = (IVec3::new(5, 100, 5), IVec3::new(4, 101, 4), IVec3::new(4, 102, 4));
        world.set_block(hearth, block("base:hearthstone"));
        world.set_block(low, block("base:chimney"));
        world.set_block(top, block("base:chimney"));
        world.set_block(IVec3::new(7, 100, 7), block("base:torch"));
        let mut emitters = Emitters::default();
        emitters.emits = (0..world.registry.len()).map(|id| world.registry.def(BlockId(id as u16)).particles.iter().map(reach).max().unwrap_or(0)).collect();
        emitters.far = emitters.emits.iter().copied().max().unwrap_or(0);
        emitters.cores = cores(&world.registry, &content.multiblocks);
        let found = |emitters: &mut Emitters, world: &ClientWorld| {
            emitters.search(world, IVec3::new(80, 102, 4));
            emitters.near.iter().map(|e| e.pos).collect::<Vec<_>>()
        };
        assert!(found(&mut emitters, &world).is_empty(), "a cold hearth smokes");
        world.fires.insert(hearth);
        assert_eq!(found(&mut emitters, &world), vec![top], "the top smokes, 76 blocks off; the torch is too far");
        world.set_block(hearth, BlockId::AIR);
        assert!(found(&mut emitters, &world).is_empty(), "no hearth, no smoke");
    }

    /// A brick flue and a pot carried up over a lit hearth's chimney take its smoke to the
    /// pot's top, and only there; with the pot gone it comes out of the flue, and with a
    /// gap between chimney and pieces, out of the chimney under them. Cold, none smokes.
    #[test]
    fn a_flue_over_a_chimney_carries_its_smoke_up_to_the_pot() {
        use voxmar_engine::world::Column;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let block = |name: &str| content.blocks.expect(name);
        let mut world = ClientWorld::new(content.blocks.clone(), 2, crate::render::terrain::Terrain::detached());
        world.insert_column(ColumnPos::of_block(IVec3::ZERO), Arc::new(Column::default()));
        let hearth = IVec3::new(5, 100, 5);
        let (top, flue, pot) = (IVec3::new(4, 102, 4), IVec3::new(4, 103, 4), IVec3::new(4, 104, 4));
        world.set_block(hearth, block("base:hearthstone"));
        world.set_block(top - IVec3::Y, block("base:chimney"));
        world.set_block(top, block("base:chimney"));
        world.set_block(flue, block("base:chimney_stack"));
        world.set_block(pot, block("base:chimney_pot"));
        let mut emitters = Emitters::default();
        emitters.emits = (0..world.registry.len()).map(|id| world.registry.def(BlockId(id as u16)).particles.iter().map(reach).max().unwrap_or(0)).collect();
        emitters.far = emitters.emits.iter().copied().max().unwrap_or(0);
        emitters.cores = cores(&world.registry, &content.multiblocks);
        // Where each found emitter's particles come out, once it is seen to stand (as the
        // frame's emitting asks).
        let found = |emitters: &mut Emitters, world: &ClientWorld| {
            emitters.search(world, IVec3::new(40, 102, 4));
            emitters.near.iter().filter(|e| e.standing(world)).map(|e| e.pos).collect::<Vec<_>>()
        };
        assert!(found(&mut emitters, &world).is_empty(), "a cold hearth's flue smokes");
        world.fires.insert(hearth);
        assert_eq!(found(&mut emitters, &world), vec![pot], "the smoke does not come out of the pot");
        world.set_block(pot, BlockId::AIR);
        assert_eq!(found(&mut emitters, &world), vec![flue], "a flue without its pot smokes from its top");
        world.set_block(flue, BlockId::AIR);
        world.set_block(pot, block("base:chimney_pot"));
        assert_eq!(found(&mut emitters, &world), vec![top], "a pot a block over the chimney carried its smoke");
        // Nine flues over the chimney: the smoke comes out of the ninth, the real top.
        world.set_block(pot, BlockId::AIR);
        for dy in 1..=9 {
            world.set_block(top + IVec3::Y * dy, block("base:chimney_stack"));
        }
        assert_eq!(found(&mut emitters, &world), vec![top + IVec3::Y * 9], "a nine-piece flue gave no smoke at its top");
    }

    /// Under a hood only a campfire's smoke goes up the flue: its flames, embers and sparks
    /// stay at the fire.
    #[test]
    fn only_smoke_goes_up_a_flue() {
        use voxmar_engine::world::Column;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let block = |name: &str| content.blocks.expect(name);
        let mut world = ClientWorld::new(content.blocks.clone(), 2, crate::render::terrain::Terrain::detached());
        world.insert_column(ColumnPos::of_block(IVec3::ZERO), Arc::new(Column::default()));
        let (fire, hood) = (IVec3::new(5, 100, 5), IVec3::new(5, 101, 5));
        world.set_block(fire, block("base:campfire"));
        world.set_block(hood, block("base:chimney_hood"));
        let mut emitters = Emitters::default();
        emitters.emits = (0..world.registry.len()).map(|id| world.registry.def(BlockId(id as u16)).particles.iter().map(reach).max().unwrap_or(0)).collect();
        emitters.far = emitters.emits.iter().copied().max().unwrap_or(0);
        emitters.cores = cores(&world.registry, &content.multiblocks);
        emitters.search(&world, IVec3::new(8, 100, 8));
        let styles = |at: IVec3| -> Vec<ParticleStyle> {
            let emitter = emitters.near.iter().find(|e| e.pos == at && e.standing(&world));
            let def = world.registry.def(block("base:campfire"));
            emitter.map_or(Vec::new(), |e| def.particles.iter().take(8).enumerate().filter(|&(i, _)| e.giving & 1 << i != 0).map(|(_, p)| p.style).collect())
        };
        let (at_fire, at_hood) = (styles(fire), styles(hood));
        assert!(!at_fire.is_empty() && at_fire.iter().all(|&s| s != ParticleStyle::Smoke), "the fire gives off {at_fire:?}");
        assert!(!at_hood.is_empty() && at_hood.iter().all(|&s| s == ParticleStyle::Smoke), "the hood gives off {at_hood:?}");
    }

    #[test]
    fn smoke_fades_under_a_ceiling() {
        let mut dice = Emitters::default();
        let emitter = Emitter { pos: IVec3::new(3, 60, -2), block: BlockId(1), from: IVec3::new(3, 60, -2), sky: 15, headroom: 1.5, share: 1.0, giving: 1 };
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
        let emitter = Emitter { pos: IVec3::new(10, 70, 10), block: BlockId(1), from: IVec3::new(10, 70, 10), sky: 0, headroom: 9.0, share: 1.0, giving: 1 };
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
        let emitter = Emitter { pos: IVec3::ZERO, block: BlockId(1), from: IVec3::ZERO, sky: 0, headroom: 9.0, share: 1.0, giving: 1 };
        let reach = |emission: &Emission, dice: &mut Emitters| {
            (0..200).map(|_| particle(emission, &emitter, 13, Vec3::ZERO, dice)).map(|p| p.velocity.y * p.life).sum::<f32>() / 200.0
        };
        let small = reach(&emission(ParticleStyle::Flame), &mut dice);
        let tall = reach(&Emission { height: Some(1.2), ..emission(ParticleStyle::Flame) }, &mut dice);
        assert!((0.08..0.2).contains(&small), "{small}");
        assert!((0.8..1.6).contains(&tall), "{tall}");
    }
}
