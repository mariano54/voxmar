//! Ore veins, and the samples of their ore lying on the ground above them.
//!
//! Ore lies in large veins of a few ores that belong together, never as single blocks
//! scattered through the rock: finding ore means finding a vein, and a vein, once found,
//! pays for the search. This follows GregTech's large veins (GregTech CE and GregTech
//! Modern, LGPL) as the StaTech Industry modpack (GPL) brings them to its world with
//! Gregic Oregen, the ideas alone (see INSPIRATIONS.md): veins sited on a coarse grid, each
//! kind drawn by weight within its band of heights, its ores layered from top to bottom,
//! and small samples of its ore on the surface over it.
//!
//! The world is divided into squares `spacing` blocks across. Each holds up to `slots`
//! veins, each drawn from `ores.ron` by its weight times its rarity's and times its liking
//! for the rock province it lies in, at a height in its band, never overlapping another
//! vein of its square in height. A vein's place, size and turn depend only on the seed and
//! its square, so every column works out the veins that reach into it for itself and
//! veins cross column borders seamlessly.
//!
//! A layered vein is a lens, an ellipse across with a wavy rim, thickest at its middle and
//! tapering to its rim, dipping a little one way as beds of rock do. Its ores keep to
//! bands of its thickness (`within`: 0 its foot, 1 its top), so a copper and tin vein shows
//! copper above and tin below. A pipe is a carrot-shaped column, widening upwards and
//! swaying as it rises: a kimberlite pipe's diamonds. Ore thins towards a vein's edges and
//! replaces only stone, so caves cut through veins and show their ore in the walls. Where
//! a vein runs through deepslate its ore takes its deepslate form (`deepslate_{name}`) when
//! it has one.
//!
//! Over each vein lie a few samples of its main ore: small rocks with ore in them, on open,
//! natural ground (never in water, on leaves or on a path) wherever the vein lies below the
//! ground, so a sample always points down to its vein. Breaking one gives a piece of the
//! ore (the sample's block data says what).

use std::collections::BTreeMap;
use std::f32::consts::TAU;

use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, WORLD_HEIGHT};
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3Swizzles};

use crate::content::{ContentError, ProvinceDef, qualify};
use crate::rarity::{Rarity, Tier};
use crate::rng::{Rng, hash3};
use crate::rocks::Rocks;
use crate::terrain::Blocks;
use crate::{Overworld, SEA_LEVEL};

const PURPOSE: u64 = 0x04e6;
const SAMPLE_PURPOSE: u64 = 0x5a3e;
/// How far a layered vein's rim reaches past its axes at most, as a share of them.
const RIM: f32 = 1.32;
/// How far a pipe sways from its axis, in blocks.
const SWAY: f32 = 2.0;

// ---------------------------------------------------------------------------------------
// ores.ron

/// The veins of every content package (`ores.ron`).
#[derive(Clone, Debug, PartialEq)]
pub struct OresDef {
    pub grid: GridDef,
    pub veins: Vec<VeinDef>,
}

impl Default for OresDef {
    fn default() -> Self {
        Self { grid: GridDef { spacing: 112, slots: 2, chance: 0.7 }, veins: Vec::new() }
    }
}

/// Where veins may lie; a later package's replaces an earlier one's.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GridDef {
    /// The side of the squares veins are sited in, in blocks.
    pub spacing: i32,
    /// How many veins a square may hold, at different heights.
    pub slots: u32,
    /// The chance each of them is there.
    pub chance: f32,
}

/// A kind of vein.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VeinDef {
    pub name: String,
    /// How often it is drawn, relative to other kinds.
    #[serde(default = "one")]
    pub weight: f32,
    #[serde(default)]
    pub rarity: Rarity,
    /// The heights its middle may lie at, above sea level (negative below it).
    pub height: (i32, i32),
    #[serde(default)]
    pub shape: VeinShape,
    /// A layered vein's half-length along its longer axis (its shorter is 0.6 to 1 of
    /// it); a pipe's radius at its top. Blocks, least and most.
    pub radius: (f32, f32),
    /// A layered vein's thickness at its middle; a pipe's height. Blocks, least and most.
    pub thickness: (f32, f32),
    /// The share of the rock turned to ore at its heart; less towards its edges.
    #[serde(default = "half")]
    pub density: f32,
    pub ores: Vec<OreShare>,
    /// Its weight is multiplied by these where the rock province has the name or tag
    /// (all that apply): more in some rocks, less in others.
    #[serde(default)]
    pub provinces: BTreeMap<String, f32>,
    /// What lies on the ground above it.
    pub sample: Option<String>,
    /// How many samples, least and most (fewer show where the ground is not open).
    #[serde(default = "samples")]
    pub samples: (u32, u32),
}

/// One of a vein's ores: its block, its share where it occurs, and the band of the
/// vein's thickness (or a pipe's height) it occurs in, from 0 at the foot to 1 at the top.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OreShare {
    pub block: String,
    #[serde(default = "one")]
    pub share: f32,
    #[serde(default = "whole")]
    pub within: (f32, f32),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum VeinShape {
    /// A lens lying across, thickest at its middle.
    #[default]
    Layered,
    /// An upright column, widening upwards.
    Pipe,
}

fn one() -> f32 {
    1.0
}

fn half() -> f32 {
    0.5
}

fn whole() -> (f32, f32) {
    (0.0, 1.0)
}

fn samples() -> (u32, u32) {
    (2, 5)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OreFile {
    grid: Option<GridDef>,
    #[serde(default)]
    veins: Vec<VeinDef>,
}

impl OresDef {
    /// Adds a package's `ores.ron`, its names qualified by its package.
    pub fn extend(&mut self, at: &str, package: &str, source: &str) -> Result<(), ContentError> {
        let file: OreFile = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(source)
            .map_err(|e| ContentError(format!("{at}: {e}")))?;
        if let Some(grid) = file.grid {
            self.grid = grid;
        }
        let q = |name: &str| qualify(package, name);
        self.veins.extend(file.veins.into_iter().map(|v| VeinDef {
            name: q(&v.name),
            ores: v.ores.into_iter().map(|o| OreShare { block: q(&o.block), ..o }).collect(),
            sample: v.sample.as_deref().map(q),
            ..v
        }));
        Ok(())
    }

    /// Checks every vein's numbers and the blocks and provinces it names.
    pub fn check(&self, registry: &BlockRegistry, provinces: &[ProvinceDef]) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("ores.ron: {what}")));
        let grid = &self.grid;
        if grid.spacing < 32 || grid.slots == 0 || !(grid.chance > 0.0 && grid.chance <= 1.0) {
            return fail(format!("the grid needs a spacing of 32 blocks or more, a slot and a chance in (0, 1]: {grid:?}"));
        }
        for (i, vein) in self.veins.iter().enumerate() {
            let name = &vein.name;
            if self.veins[..i].iter().any(|v| v.name == *name) {
                return fail(format!("vein `{name}` is defined twice"));
            }
            let range = |(lo, hi): (f32, f32)| lo > 0.0 && lo <= hi && hi.is_finite();
            if !(vein.weight > 0.0 && vein.weight.is_finite()) || !(vein.density > 0.0 && vein.density <= 1.0) {
                return fail(format!("vein `{name}`: its weight must be positive and its density in (0, 1]"));
            }
            if !range(vein.radius) || !range(vein.thickness) || vein.radius.1 > 64.0 || vein.thickness.1 > 96.0 {
                return fail(format!("vein `{name}`: radius and thickness are positive ranges, least first, up to 64 and 96 blocks"));
            }
            let (lo, hi) = vein.height;
            if lo > hi || lo + SEA_LEVEL < 0 || hi + SEA_LEVEL >= WORLD_HEIGHT {
                return fail(format!("vein `{name}`: its heights ({lo}, {hi}) are not a range within the world"));
            }
            if vein.ores.is_empty() {
                return fail(format!("vein `{name}` has no ores"));
            }
            for ore in &vein.ores {
                if registry.id(&ore.block).is_none() {
                    return fail(format!("vein `{name}`: no block `{}`", ore.block));
                }
                let (a, b) = ore.within;
                if !(ore.share > 0.0) || !(0.0..=1.0).contains(&a) || !(0.0..=1.0).contains(&b) || a >= b {
                    return fail(format!("vein `{name}`: ore `{}` needs a positive share and a band within 0..1", ore.block));
                }
            }
            if let Some(sample) = &vein.sample
                && registry.id(sample).is_none()
            {
                return fail(format!("vein `{name}`: no sample block `{sample}`"));
            }
            if vein.samples.0 > vein.samples.1 || vein.samples.1 > 16 {
                return fail(format!("vein `{name}`: samples are a range, least first, up to 16"));
            }
            for (province, factor) in &vein.provinces {
                if !provinces.iter().any(|p| p.matches(province)) {
                    return fail(format!("vein `{name}`: no rock province is named or tagged `{province}`"));
                }
                if !(*factor >= 0.0 && factor.is_finite()) {
                    return fail(format!("vein `{name}`: the factor for `{province}` must not be negative"));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Placement

/// A kind of vein, ready to place.
struct Kind {
    name: String,
    /// Weight times rarity, by rock province.
    weights: Vec<f32>,
    /// Heights of its middle, in world coordinates.
    height: (i32, i32),
    shape: VeinShape,
    radius: (f32, f32),
    thickness: (f32, f32),
    density: f32,
    ores: Vec<Ore>,
    sample: Option<BlockId>,
    samples: (u32, u32),
}

struct Ore {
    name: String,
    block: BlockId,
    /// Its form in deepslate, if it has one.
    deep: Option<BlockId>,
    share: f32,
    within: (f32, f32),
}

/// One vein, where it lies.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vein {
    kind: usize,
    centre: IVec3,
    /// Half-lengths of its axes across (a pipe's radius at its top, twice).
    axes: Vec2,
    /// The direction of its longer axis.
    turn: Vec2,
    /// Thickness at its middle (a pipe's height).
    thickness: f32,
    /// How far its middle rises per block across, as beds of rock dip.
    dip: Vec2,
    /// Phases of its rim's waves (a pipe's sway).
    phases: [f32; 3],
    seed: u64,
}

impl Vein {
    /// A layered vein's span at (x, z): its foot, its top, and how far out towards its
    /// rim the position lies (0 at its middle, 1 at its rim).
    fn span(&self, x: i32, z: i32) -> Option<(f32, f32, f32)> {
        let d = Vec2::new((x - self.centre.x) as f32, (z - self.centre.z) as f32);
        let (u, v) = (d.dot(self.turn) / self.axes.x, d.perp_dot(self.turn) / self.axes.y);
        let r2 = u * u + v * v;
        if r2 >= RIM * RIM {
            return None;
        }
        let angle = v.atan2(u);
        let [a, b, c] = self.phases;
        let rim = 1.0 + 0.16 * (2.0 * angle + a).sin() + 0.1 * (3.0 * angle + b).sin() + 0.06 * (5.0 * angle + c).sin();
        let out = r2 / (rim * rim);
        if out >= 1.0 {
            return None;
        }
        let half = 0.5 * self.thickness * (1.0 - out).sqrt();
        let middle = self.centre.y as f32 + self.dip.dot(d);
        (half >= 0.5).then_some((middle - half, middle + half, out))
    }

    /// A pipe's foot and top.
    fn pipe_ends(&self) -> (i32, i32) {
        let foot = self.centre.y - (self.thickness * 0.5) as i32;
        (foot, foot + self.thickness as i32)
    }

    /// A pipe's axis and radius at height `y`, and how far up the pipe that is (0..1).
    fn disc(&self, y: i32) -> Option<(Vec2, f32, f32)> {
        let (foot, top) = self.pipe_ends();
        if y < foot || y > top {
            return None;
        }
        let t = (y - foot) as f32 / (top - foot).max(1) as f32;
        let [a, b, _] = self.phases;
        let sway = Vec2::new((y as f32 * 0.11 + a).sin(), (y as f32 * 0.09 + b).cos()) * SWAY;
        // A carrot: narrow at the foot, widest near the top, rounded off there.
        let radius = self.axes.x * (0.45 + 0.55 * t) * ((1.0 - t) * 6.0).min(1.0).sqrt();
        Some((self.centre.xz().as_vec2() + sway, radius, t))
    }
}

/// A sample on the ground above a vein: its block, where it lies across, and the lowest
/// the vein lies, so it is placed only on ground above that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sample {
    pub x: i32,
    pub z: i32,
    pub block: BlockId,
    pub floor: i32,
}

pub struct Ores {
    kinds: Vec<Kind>,
    spacing: i32,
    slots: u32,
    chance: f32,
    /// How far any vein or sample reaches from its middle across, in blocks.
    reach: i32,
    deepslate: BlockId,
}

impl Ores {
    pub fn new(content: &crate::content::WorldContent, registry: &BlockRegistry, rocks: &Rocks) -> Self {
        let def = &content.ores;
        let kinds = def
            .veins
            .iter()
            .map(|vein| {
                let weights = (0..rocks.len())
                    .map(|p| {
                        let province = crate::rocks::ProvinceId(p as u8);
                        let liking: f32 = vein.provinces.iter().filter(|(name, _)| rocks.matches(province, name)).map(|(_, f)| f).product();
                        vein.weight * vein.rarity.multiplier() * liking
                    })
                    .collect();
                let ores = vein
                    .ores
                    .iter()
                    .map(|o| {
                        let block = registry.expect(&o.block);
                        let deep = o.block.split_once(':').and_then(|(package, local)| registry.id(&format!("{package}:deepslate_{local}")));
                        Ore { name: o.block.clone(), block, deep, share: o.share, within: o.within }
                    })
                    .collect();
                Kind {
                    name: vein.name.clone(),
                    weights,
                    height: (vein.height.0 + SEA_LEVEL, vein.height.1 + SEA_LEVEL),
                    shape: vein.shape,
                    radius: vein.radius,
                    thickness: vein.thickness,
                    density: vein.density,
                    ores,
                    sample: vein.sample.as_deref().map(|s| registry.expect(s)),
                    samples: vein.samples,
                }
            })
            .collect::<Vec<Kind>>();
        let reach = kinds
            .iter()
            .map(|k| match k.shape {
                VeinShape::Layered => k.radius.1 * RIM,
                VeinShape::Pipe => k.radius.1 + SWAY,
            })
            .fold(0.0f32, f32::max)
            .ceil() as i32
            + 1;
        Self { kinds, spacing: def.grid.spacing, slots: def.grid.slots, chance: def.grid.chance, reach, deepslate: registry.expect("base:deepslate") }
    }

    /// The veins of a square of the grid, by its seed alone.
    fn square(&self, world: &Overworld, square: IVec2, out: &mut Vec<Vein>) {
        let first = out.len();
        for slot in 0..self.slots {
            let mut rng = Rng::new(world.seed, PURPOSE + slot as u64, square.x, square.y);
            if !rng.chance(self.chance) || self.kinds.is_empty() {
                continue;
            }
            let (x, z) = (square.x * self.spacing + rng.below(self.spacing as u32), square.y * self.spacing + rng.below(self.spacing as u32));
            let province = world.rocks.at(x, z, world.geology.regional(x, z).warp).0 as usize;
            let total: f32 = self.kinds.iter().map(|k| k.weights[province]).sum();
            let mut pick = rng.unit() * total;
            let Some(kind) = self.kinds.iter().position(|k| {
                pick -= k.weights[province];
                pick < 0.0
            }) else {
                continue;
            };
            let k = &self.kinds[kind];
            let between = |rng: &mut Rng, (lo, hi): (f32, f32)| lo + (hi - lo) * rng.unit();
            let y = rng.range(k.height.0, k.height.1 + 1);
            let long = between(&mut rng, k.radius);
            let thickness = between(&mut rng, k.thickness);
            let angle = rng.unit() * TAU;
            let tilt = rng.unit() * TAU;
            let steep = rng.unit() * 0.22;
            let phases = [rng.unit() * TAU, rng.unit() * TAU, rng.unit() * TAU];
            let seed = rng.next_u64();
            let vein = match k.shape {
                VeinShape::Layered => Vein {
                    kind,
                    centre: IVec3::new(x, y, z),
                    axes: Vec2::new(long, long * (0.6 + 0.4 * rng.unit())),
                    turn: Vec2::from_angle(angle),
                    thickness,
                    dip: Vec2::from_angle(tilt) * steep,
                    phases,
                    seed,
                },
                VeinShape::Pipe => Vein { kind, centre: IVec3::new(x, y, z), axes: Vec2::splat(long), turn: Vec2::X, thickness, dip: Vec2::ZERO, phases, seed },
            };
            // Veins of a square never share heights.
            let (low, high) = self.heights(&vein);
            if out[first..].iter().any(|other| {
                let (a, b) = self.heights(other);
                low <= b && a <= high
            }) {
                continue;
            }
            out.push(vein);
        }
    }

    /// The lowest and highest a vein reaches.
    fn heights(&self, vein: &Vein) -> (i32, i32) {
        match self.kinds[vein.kind].shape {
            VeinShape::Pipe => vein.pipe_ends(),
            VeinShape::Layered => {
                let rise = vein.dip.length() * vein.axes.x * RIM;
                let half = 0.5 * vein.thickness + rise;
                ((vein.centre.y as f32 - half).floor() as i32, (vein.centre.y as f32 + half).ceil() as i32)
            }
        }
    }

    /// Every vein whose middle lies within `reach` of the box from `min` to `max` across.
    pub(crate) fn veins_near(&self, world: &Overworld, min: IVec2, max: IVec2) -> Vec<Vein> {
        let (low, high) = ((min - self.reach).div_euclid(IVec2::splat(self.spacing)), (max + self.reach).div_euclid(IVec2::splat(self.spacing)));
        let mut veins = Vec::new();
        for sz in low.y..=high.y {
            for sx in low.x..=high.x {
                self.square(world, IVec2::new(sx, sz), &mut veins);
            }
        }
        veins.retain(|v| {
            let c = v.centre.xz();
            c.cmpge(min - self.reach).all() && c.cmple(max + self.reach).all()
        });
        veins
    }

    /// Places the ore of every vein reaching into the column.
    pub fn place(&self, world: &Overworld, pos: ColumnPos, blocks: &mut Blocks) {
        let origin = pos.origin();
        let (min, max) = (origin.xz(), origin.xz() + CHUNK_SIZE - 1);
        for vein in self.veins_near(world, min, max) {
            self.ores_in(&vein, min, max, |p, ore| {
                let block = match ore.deep {
                    Some(deep) if blocks.get(p) == self.deepslate => deep,
                    _ => ore.block,
                };
                blocks.replace_stone(p, block);
            });
        }
    }

    /// Calls `put` with every block of ore a vein holds between `min` and `max` across
    /// (columns), and the ore there.
    fn ores_in(&self, vein: &Vein, min: IVec2, max: IVec2, mut put: impl FnMut(IVec3, &Ore)) {
        let kind = &self.kinds[vein.kind];
        let mut put = |p: IVec3, t: f32, density: f32| {
            let h = hash3(vein.seed, p.x, p.y, p.z);
            if ((h & 0xffff) as f32) >= density * 65536.0 {
                return;
            }
            if let Some(ore) = kind.pick(t, (h >> 16 & 0xffff) as f32 / 65536.0) {
                put(p, ore);
            }
        };
        let reach = match kind.shape {
            VeinShape::Layered => (vein.axes.x * RIM).ceil() as i32,
            VeinShape::Pipe => (vein.axes.x + SWAY).ceil() as i32,
        };
        let (low, high) = ((vein.centre.xz() - reach).max(min), (vein.centre.xz() + reach).min(max));
        if low.cmpgt(high).any() {
            return;
        }
        match kind.shape {
            VeinShape::Layered => {
                for z in low.y..=high.y {
                    for x in low.x..=high.x {
                        let Some((foot, top, out)) = vein.span(x, z) else { continue };
                        // Ore thins towards the rim.
                        let density = kind.density * (1.0 - 0.55 * out);
                        for y in (foot.ceil() as i32).max(1)..=(top.floor() as i32).min(WORLD_HEIGHT - 1) {
                            put(IVec3::new(x, y, z), (y as f32 - foot) / (top - foot), density);
                        }
                    }
                }
            }
            VeinShape::Pipe => {
                let (foot, top) = vein.pipe_ends();
                for y in foot.max(1)..=top.min(WORLD_HEIGHT - 1) {
                    let Some((axis, radius, t)) = vein.disc(y) else { continue };
                    for z in low.y..=high.y {
                        for x in low.x..=high.x {
                            let out = Vec2::new(x as f32, z as f32).distance_squared(axis) / (radius * radius).max(0.25);
                            if out < 1.0 {
                                put(IVec3::new(x, y, z), t, kind.density * (1.0 - 0.5 * out));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Whether a block lies within a vein's body (ore or not).
    #[cfg(test)]
    fn contains(&self, vein: &Vein, p: IVec3) -> bool {
        match self.kinds[vein.kind].shape {
            VeinShape::Layered => vein.span(p.x, p.z).is_some_and(|(foot, top, _)| foot.ceil() as i32 <= p.y && p.y <= top.floor() as i32),
            VeinShape::Pipe => vein.disc(p.y).is_some_and(|(axis, radius, _)| Vec2::new(p.x as f32, p.z as f32).distance_squared(axis) < (radius * radius).max(0.25)),
        }
    }

    /// A vein's samples, wherever they lie.
    fn samples_of(&self, vein: &Vein) -> Vec<Sample> {
        let kind = &self.kinds[vein.kind];
        let mut rng = Rng::new(vein.seed, SAMPLE_PURPOSE, 0, 0);
        let count = match kind.sample {
            Some(_) => rng.range(kind.samples.0 as i32, kind.samples.1 as i32 + 1),
            None => 0,
        };
        let (block, floor) = (kind.sample.unwrap_or(BlockId::AIR), self.heights(vein).0);
        (0..count)
            .map(|_| {
                // Scattered over the middle of the vein, well inside its rim, or about the
                // mouth of a pipe.
                let (r, a) = (rng.unit().sqrt(), rng.unit() * TAU);
                let local = Vec2::from_angle(a) * r;
                let at = match kind.shape {
                    VeinShape::Layered => vein.centre.xz().as_vec2() + vein.turn.rotate(local * vein.axes * 0.6),
                    VeinShape::Pipe => vein.centre.xz().as_vec2() + local * (vein.axes.x + 1.0),
                };
                Sample { x: at.x.floor() as i32, z: at.y.floor() as i32, block, floor }
            })
            .collect()
    }

    /// The middle of the vein a sample of `block` at (x, z) lies over: what tells one vein
    /// from another for prospecting. None where no vein's sample of it lies there.
    pub(crate) fn vein_of_sample(&self, world: &Overworld, x: i32, z: i32, block: BlockId) -> Option<IVec3> {
        let at = IVec2::new(x, z);
        self.veins_near(world, at, at).into_iter().find(|vein| self.samples_of(vein).iter().any(|s| s.x == x && s.z == z && s.block == block)).map(|vein| vein.centre)
    }

    /// The samples lying in a column, if the ground there allows.
    pub(crate) fn samples(&self, world: &Overworld, pos: ColumnPos) -> Vec<Sample> {
        let origin = pos.origin();
        let (min, max) = (origin.xz(), origin.xz() + CHUNK_SIZE - 1);
        self.veins_near(world, min, max)
            .iter()
            .flat_map(|vein| self.samples_of(vein))
            .filter(|s| (min.x..=max.x).contains(&s.x) && (min.y..=max.y).contains(&s.z))
            .collect()
    }
}

impl Kind {
    /// The ore at `t` up the vein, by `pick` (0..1) among those whose band holds it.
    fn pick(&self, t: f32, pick: f32) -> Option<&Ore> {
        let holds = |o: &&Ore| o.within.0 <= t && t <= o.within.1;
        let total: f32 = self.ores.iter().filter(holds).map(|o| o.share).sum();
        let mut left = pick * total;
        self.ores.iter().filter(holds).find(|o| {
            left -= o.share;
            left < 0.0
        })
    }
}

impl Overworld {
    /// The nearest sample of ore on the ground within `radius` blocks across of `from`, of a
    /// vein named `name` or holding an ore named `name` (`iron`, `iron_ore`, `base:iron_ore`),
    /// or of any vein: where it lies, and its block.
    pub fn locate_ore(&self, name: Option<&str>, from: IVec2, radius: i32) -> Option<(IVec3, BlockId)> {
        let ores = &self.terrain.ores;
        let local = |full: &str| full.split_once(':').map_or(full, |(_, l)| l).to_owned();
        let named = |kind: &Kind| match name {
            None => true,
            Some(name) => {
                let wanted = local(name);
                local(&kind.name) == wanted
                    || kind.ores.iter().any(|o| {
                        let ore = local(&o.name);
                        ore == wanted || ore == format!("{wanted}_ore")
                    })
            }
        };
        let mut found: Vec<Sample> = ores
            .veins_near(self, from - radius, from + radius)
            .iter()
            .filter(|v| named(&ores.kinds[v.kind]))
            .flat_map(|v| ores.samples_of(v))
            .filter(|s| IVec2::new(s.x, s.z).distance_squared(from) <= radius * radius)
            .collect();
        found.sort_by_key(|s| (IVec2::new(s.x, s.z).distance_squared(from), s.x, s.z));
        // A sample is only where the ground let it lie: look in the generated column.
        found.into_iter().take(24).find_map(|s| {
            use crate::Generator;
            let column = self.generate(ColumnPos::of_block(IVec3::new(s.x, 0, s.z)));
            let (x, z) = (s.x.rem_euclid(CHUNK_SIZE), s.z.rem_euclid(CHUNK_SIZE));
            (0..WORLD_HEIGHT).rev().find(|&y| column.block(IVec3::new(x, y, z)) == s.block).map(|y| (IVec3::new(s.x, y, s.z), s.block))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::Generator;
    use crate::tests::generate_area;

    fn registry() -> Arc<BlockRegistry> {
        Arc::new(crate::test_registry())
    }

    /// `cargo test --profile iter -p voxmar_worldgen ore_preview -- --ignored --nocapture`
    /// (optionally `SEED=n`, a centre `X=x Z=z`, by default spawn, and `COLUMNS=n`, 16 by
    /// default) writes two views of the ore over that square to `target/`: `ores_xray.ppm`,
    /// from above, each position coloured by the ore most common under it and brighter for
    /// more of it, over the land's height in grey, with surface samples as white dots; and
    /// `ores_section.ppm`, a west-east cut through the richest row, an x-ray of a slab eight
    /// blocks thick (ore anywhere in it shows) over the blocks of the cut itself. It prints
    /// how much of each ore there is per column and how much of it lies alone.
    #[test]
    #[ignore]
    fn ore_preview() {
        let env = |name: &str| std::env::var(name).ok().and_then(|s| s.parse::<i64>().ok());
        let registry = registry();
        let seed = env("SEED").unwrap_or(0x5eed) as u64;
        let count = env("COLUMNS").unwrap_or(16) as i32;
        let size = count * 32;
        let world = Overworld::new(seed, registry.clone());
        let spawn = world.spawn_point();
        let centre = IVec3::new(env("X").map_or(spawn.x, |x| x as i32), 0, env("Z").map_or(spawn.z, |z| z as i32));
        let first = ColumnPos::of_block(centre).offset(-count / 2, -count / 2);
        let start = std::time::Instant::now();
        let columns = generate_area(&world, first, count);
        println!("generated {} columns in {:?}", columns.len(), start.elapsed());
        let origin = first.origin();
        let block = |x: i32, y: i32, z: i32| {
            let (cx, cz) = ((x - origin.x) >> 5, (z - origin.z) >> 5);
            if !(0..count).contains(&cx) || !(0..count).contains(&cz) || !(0..WORLD_HEIGHT).contains(&y) {
                return BlockId::AIR;
            }
            columns[(cz * count + cx) as usize].block(IVec3::new(x & 31, y, z & 31))
        };
        let ore = |b: BlockId| registry.def(b).has_tag("ore") && !registry.def(b).has_tag("ore_sample");
        let sample = |b: BlockId| registry.def(b).has_tag("ore_sample");
        let colour = |b: BlockId| registry.def(b).color.map(|c| c as f32 / 255.0);
        // Ore at full strength, so a vein stands out: coal black, the rest their colour at
        // its brightest.
        let vivid = |b: BlockId| {
            let c = colour(b);
            let top = c.into_iter().fold(0.0f32, f32::max);
            if registry.def(b).has_tag("fuel") { [0.02; 3] } else { c.map(|v| v / top.max(0.01)) }
        };
        let write = |name: &str, width: i32, height: i32, pixels: &[[f32; 3]]| {
            let path = format!("{}/../../target/{name}", env!("CARGO_MANIFEST_DIR"));
            let mut file = format!("P6 {width} {height} 255\n").into_bytes();
            file.extend(pixels.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8)));
            std::fs::write(&path, file).unwrap();
            println!("wrote {path}");
        };

        // From above, and the census.
        let mut xray = vec![[0.0f32; 3]; (size * size) as usize];
        let mut totals: Vec<(String, u64, u64)> = Vec::new();
        let mut rows = vec![0u64; size as usize];
        let mut samples = 0;
        for z in 0..size {
            for x in 0..size {
                let (wx, wz) = (origin.x + x, origin.z + z);
                let mut kinds: Vec<(BlockId, u32)> = Vec::new();
                let mut ground = 0;
                for y in 0..WORLD_HEIGHT {
                    let b = block(wx, y, wz);
                    if registry.def(b).solid {
                        ground = y;
                    }
                    if sample(b) {
                        samples += 1;
                        for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)] {
                            let (px, pz) = (x + dx, z + dz);
                            if (0..size).contains(&px) && (0..size).contains(&pz) {
                                xray[(pz * size + px) as usize] = [1.0; 3];
                            }
                        }
                    }
                    if !ore(b) {
                        continue;
                    }
                    rows[z as usize] += 1;
                    match kinds.iter_mut().find(|(k, _)| *k == b) {
                        Some((_, n)) => *n += 1,
                        None => kinds.push((b, 1)),
                    }
                    let alone = (-1..=1)
                        .flat_map(|dy| (-1..=1).flat_map(move |dz| (-1..=1).map(move |dx| (dx, dy, dz))))
                        .filter(|&d| d != (0, 0, 0))
                        .all(|(dx, dy, dz)| !ore(block(wx + dx, y + dy, wz + dz)));
                    let name = &registry.def(b).name;
                    let entry = match totals.iter().position(|(n, _, _)| n == name) {
                        Some(i) => &mut totals[i],
                        None => {
                            totals.push((name.clone(), 0, 0));
                            totals.last_mut().unwrap()
                        }
                    };
                    entry.1 += 1;
                    entry.2 += alone as u64;
                }
                let pixel = &mut xray[(z * size + x) as usize];
                if *pixel == [1.0; 3] {
                    continue;
                }
                *pixel = match kinds.iter().max_by_key(|(_, n)| *n) {
                    Some(&(b, _)) => {
                        let n: u32 = kinds.iter().map(|(_, n)| n).sum();
                        let strength = 0.55 + 0.45 * (n as f32 / 12.0).min(1.0);
                        let ground = 0.2 + 0.35 * ground as f32 / WORLD_HEIGHT as f32;
                        vivid(b).map(|c| ground + (c - ground) * strength)
                    }
                    None => [0.2 + 0.35 * ground as f32 / WORLD_HEIGHT as f32; 3],
                };
            }
        }
        write("ores_xray.ppm", size, size, &xray);
        totals.sort();
        let columns = (count * count) as f64;
        println!("{samples} surface samples ({:.2} per column)", samples as f64 / columns);
        for (name, n, alone) in &totals {
            println!("{name:<28} {:>8.1} per column, {:>5.1}% alone", *n as f64 / columns, 100.0 * *alone as f64 / *n as f64);
        }

        // The veins around, and how much of each kind's ore shows.
        let veins = world.terrain.ores.veins_near(&world, origin.xz(), origin.xz() + size - 1);
        let mut kinds: Vec<(&str, u32)> = Vec::new();
        for vein in &veins {
            let name = world.terrain.ores.kinds[vein.kind].name.as_str();
            match kinds.iter_mut().find(|(n, _)| *n == name) {
                Some((_, n)) => *n += 1,
                None => kinds.push((name, 1)),
            }
        }
        kinds.sort();
        println!("{} veins within reach: {kinds:?}", veins.len());

        // A west-east cut through the richest row, 160 blocks wide around its richest part,
        // three pixels a block: an x-ray of the slab eight blocks thick around it (ore
        // anywhere in it shows) over the blocks of the cut, sky at the top.
        let (water, deepslate) = (registry.expect("base:water"), registry.expect("base:deepslate"));
        let row = (0..size).max_by_key(|&z| (z - 4..z + 4).filter(|z| (0..size).contains(z)).map(|z| rows[z as usize]).sum::<u64>()).unwrap();
        let wz = origin.z + row;
        let slab = |x: i32, y: i32| (wz - 4..wz + 4).map(|z| block(x, y, z)).find(|&b| ore(b) || sample(b));
        const WIDE: i32 = 160;
        const SCALE: i32 = 3;
        let rich = |x: i32| (0..WORLD_HEIGHT).filter(|&y| slab(x, y).is_some()).count();
        let middle = (WIDE / 2..size - WIDE / 2).max_by_key(|&x| (x - 20..x + 20).map(|x| rich(origin.x + x)).sum::<usize>()).unwrap_or(size / 2);
        let first_x = origin.x + middle - WIDE / 2;
        let mut section = Vec::with_capacity((WIDE * SCALE * WORLD_HEIGHT * SCALE) as usize);
        for y in (0..WORLD_HEIGHT).rev() {
            let line: Vec<[f32; 3]> = (first_x..first_x + WIDE)
                .map(|wx| {
                    let b = block(wx, y, wz);
                    match slab(wx, y) {
                        Some(o) if sample(o) => [1.0, 0.25, 0.9],
                        Some(o) => vivid(o),
                        None if b == water => [0.12, 0.25, 0.6],
                        None if b.is_air() => [0.6, 0.75, 0.92],
                        None if b == deepslate => [0.3, 0.3, 0.34],
                        None => colour(b).map(|c| 0.25 + c * 0.3),
                    }
                })
                .collect();
            for _ in 0..SCALE {
                section.extend(line.iter().flat_map(|&p| std::iter::repeat_n(p, SCALE as usize)));
            }
        }
        println!("section at z = {wz}, x from {first_x} to {}", first_x + WIDE - 1);
        write("ores_section.ppm", WIDE * SCALE, WORLD_HEIGHT * SCALE, &section);
    }

    /// The ore of the generated columns of a square `count` columns across from `first`,
    /// and its samples: where each lies and its block.
    fn census(world: &Overworld, registry: &BlockRegistry, first: ColumnPos, count: i32) -> (Vec<(IVec3, BlockId)>, Vec<(IVec3, BlockId)>) {
        let columns = generate_area(world, first, count);
        let (mut ores, mut samples) = (Vec::new(), Vec::new());
        for (i, column) in columns.iter().enumerate() {
            let origin = first.offset(i as i32 % count, i as i32 / count).origin();
            for y in 0..WORLD_HEIGHT {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let b = column.block(IVec3::new(x, y, z));
                        let def = registry.def(b);
                        if def.has_tag("ore_sample") {
                            samples.push((origin + IVec3::new(x, y, z), b));
                        } else if def.has_tag("ore") {
                            ores.push((origin + IVec3::new(x, y, z), b));
                        }
                    }
                }
            }
        }
        (ores, samples)
    }

    /// What ore a block is (its deepslate form is the same ore): what it drops.
    fn ore_of(registry: &BlockRegistry, block: BlockId) -> String {
        let def = registry.def(block);
        def.drops.clone().unwrap_or_else(|| def.name.clone())
    }

    /// Every block of ore lies in the body of a vein: no ore is scattered alone through the
    /// rock, and veins hold their ores. Samples lie on solid ground with their vein's ore
    /// below them.
    #[test]
    fn ore_lies_in_veins_under_its_samples() {
        let registry = registry();
        let world = crate::tests::shared_world(42);
        let first = ColumnPos::of_block(world.spawn_point()).offset(-4, -4);
        let (ores, samples) = census(&world, &registry, first, 8);
        let (min, max) = (first.origin().xz(), first.origin().xz() + 8 * CHUNK_SIZE - 1);
        let ore_veins = &world.terrain.ores;
        let veins = ore_veins.veins_near(&world, min, max);
        assert!(ores.len() > 1000, "only {} ore blocks over 8x8 columns", ores.len());
        for &(p, b) in &ores {
            let vein = veins.iter().find(|v| ore_veins.contains(v, p));
            let Some(vein) = vein else { panic!("{} at {p} lies in no vein", registry.def(b).name) };
            let kind = &ore_veins.kinds[vein.kind];
            assert!(kind.ores.iter().any(|o| o.block == b || o.deep == Some(b)), "{} at {p} in a {} vein", registry.def(b).name, kind.name);
        }
        assert!(samples.len() >= 4, "only {} samples of ore over 8x8 columns", samples.len());
        for &(p, b) in &samples {
            let ground = world.generate(ColumnPos::of_block(p)).block(IVec3::new(p.x.rem_euclid(32), p.y - 1, p.z.rem_euclid(32)));
            assert!(registry.def(ground).solid, "the {} at {p} lies on {}", registry.def(b).name, registry.def(ground).name);
            let wanted = ore_of(&registry, b);
            let below = ores.iter().filter(|(q, o)| q.y < p.y && (q.xz() - p.xz()).abs().max_element() <= 7 && ore_of(&registry, *o) == wanted).count();
            // Near the edge of the area its vein may lie outside it.
            let inside = (p.xz() - min).min_element() >= 7 && (max - p.xz()).min_element() >= 7;
            assert!(below > 0 || !inside, "no {wanted} below the {} at {p}", registry.def(b).name);
        }
    }

    /// A sample tells which vein it lies over: every sample of a vein names the same one,
    /// samples of different veins different ones, and a cell with no sample none.
    #[test]
    fn a_sample_names_its_vein() {
        let world = crate::tests::shared_world(42);
        let first = ColumnPos::of_block(world.spawn_point()).offset(-4, -4);
        let (min, max) = (first.origin().xz(), first.origin().xz() + 8 * CHUNK_SIZE - 1);
        let ores = &world.terrain.ores;
        let veins: Vec<Vein> = ores.veins_near(&world, min, max).into_iter().filter(|v| ores.kinds[v.kind].sample.is_some()).collect();
        assert!(veins.len() >= 2, "only {} veins with samples near spawn", veins.len());
        let mut told = 0;
        for vein in &veins {
            for sample in ores.samples_of(vein) {
                let at = IVec3::new(sample.x, 0, sample.z);
                let named = world.vein_of_sample(at, sample.block).expect("a vein for its own sample");
                // Two veins' samples may share a cell; the first vein found answers for both.
                let shared = veins.iter().any(|other| other.centre != vein.centre && ores.samples_of(other).iter().any(|s| s.x == sample.x && s.z == sample.z && s.block == sample.block));
                assert!(named == vein.centre || shared, "the sample at {at} names the vein at {named}, not its own at {}", vein.centre);
                told += 1;
            }
        }
        assert!(told >= 4, "only {told} samples");
        let sample = ores.samples_of(&veins[0])[0];
        assert_eq!(world.vein_of_sample(IVec3::new(sample.x, 0, sample.z), BlockId::AIR), None, "a vein for a sample of nothing");
    }

    /// `/locate ore` finds a sample lying where it says, of the ore asked for.
    #[test]
    fn locate_finds_samples() {
        let registry = registry();
        let world = crate::tests::shared_world(42);
        let spawn = world.spawn_point();
        let (at, block) = world.locate_ore(None, spawn.xz(), 800).expect("a sample of ore within 800 blocks of spawn");
        let column = world.generate(ColumnPos::of_block(at));
        assert_eq!(column.block(IVec3::new(at.x.rem_euclid(32), at.y, at.z.rem_euclid(32))), block);
        assert!(registry.def(block).has_tag("ore_sample"));
        // A vein holding iron: its sample is of its main ore, iron or what lies with it.
        if let Some((_, sample)) = world.locate_ore(Some("iron"), spawn.xz(), 1500) {
            let name = &registry.def(sample).name;
            assert!(["base:iron_ore_sample", "base:copper_ore_sample", "base:lodestone_sample"].contains(&name.as_str()), "/locate ore iron found a {name}");
        }
        if let Some((_, sample)) = world.locate_ore(Some("coal_seam"), spawn.xz(), 1500) {
            assert_eq!(registry.def(sample).name, "base:coal_ore_sample");
        }
    }

    /// `cargo test --profile iter -p voxmar_worldgen ore_sights -- --ignored --nocapture`
    /// (optionally `SEED=n`, 1 by default, and `BACK`, the camera's distance, 3 by default)
    /// prints `--pose` arguments that look at the nearest sample of each ore near spawn, for
    /// in-game captures.
    #[test]
    #[ignore]
    fn ore_sights() {
        let env = |name: &str, default: f64| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
        let (seed, back) = (env("SEED", 1.0) as u64, env("BACK", 3.0) as f32);
        let world = Overworld::new(seed, registry());
        let spawn = world.spawn_point();
        let eye = spawn.as_vec3() + voxmar_engine::glam::Vec3::Y * 1.62;
        println!("seed {seed}, spawn {spawn}");
        for ore in ["coal", "copper", "iron", "tin", "silver", "gold", "cobalt", "diamond", "aetherite"] {
            let Some((at, _)) = world.locate_ore(Some(ore), spawn.xz(), 1500) else {
                println!("{ore}: none within 1500");
                continue;
            };
            // From the south-east, a little above, looking down at it.
            let target = at.as_vec3() + voxmar_engine::glam::Vec3::new(0.5, 0.2, 0.5);
            let mut camera = target + voxmar_engine::glam::Vec3::new(back, back * 0.9, back);
            camera.y = camera.y.max(world.height(camera.x as i32, camera.z as i32) as f32 + 2.6);
            let d = target - camera;
            let yaw = (-d.x).atan2(-d.z).to_degrees();
            let pitch = d.y.atan2(d.x.hypot(d.z)).to_degrees();
            let o = camera - eye;
            println!("{ore} at {at}: --pose {:.1},{:.1},{:.1},{yaw:.0},{pitch:.0}", o.x, o.y, o.z);
        }
    }

    /// The share of veins of each rarity class (common, uncommon, rare, very rare), in
    /// percent, over `squares` squares of the grid on each side around the origin, on
    /// seeds 1 to `seeds`, and how many veins that was.
    fn vein_mix(seeds: u64, squares: i32) -> ([f32; 4], usize) {
        let registry = registry();
        let defs = crate::content::WorldContent::base(&registry).unwrap().ores.veins;
        let class = |name: &str| defs.iter().find(|d| d.name == name).map_or(0, |d| (d.rarity as usize).min(3));
        let mut counts = [0usize; 4];
        for seed in 1..=seeds {
            let world = Overworld::new(seed, registry.clone());
            let ores = &world.terrain.ores;
            let mut veins = Vec::new();
            for sz in -squares / 2..squares / 2 {
                for sx in -squares / 2..squares / 2 {
                    ores.square(&world, IVec2::new(sx, sz), &mut veins);
                }
            }
            for vein in &veins {
                counts[class(&ores.kinds[vein.kind].name)] += 1;
            }
        }
        let total: usize = counts.iter().sum();
        (counts.map(|c| 100.0 * c as f32 / total as f32), total)
    }

    /// Veins come in the mix of rarity classes the design asks for (draft 85): about 68%
    /// common, 17% uncommon, 14% rare and 1% very rare, counted over the veins that
    /// generate (the weights make up for the rock provinces' bias), within ±1.5, ±1.5, ±0.8
    /// and ±0.3 points.
    #[test]
    fn veins_come_in_the_designed_mix_of_rarities() {
        let wanted = [68.0, 17.0, 14.0, 1.0];
        let (mix, veins) = vein_mix(4, 60);
        println!("vein mix over {veins} veins: common {:.1}%, uncommon {:.1}%, rare {:.1}%, very rare {:.1}%", mix[0], mix[1], mix[2], mix[3]);
        let tolerance = [1.5, 1.5, 0.8, 0.3];
        for ((got, want), tolerance) in mix.iter().zip(wanted).zip(tolerance) {
            assert!((got - want).abs() <= tolerance, "vein mix {mix:?}, wanted {wanted:?} within {tolerance:?}");
        }
    }

    /// Every very rare ore is rarer per square than every rare ore (the wiki's rule), in
    /// blocks of ore over seeds 1 to 3 and 2,500 squares each (the vein bodies' ore, before
    /// caves cut into it).
    #[test]
    fn very_rare_ores_are_rarer_than_rare_ones() {
        use crate::rarity::Rarity;
        let registry = registry();
        let mut counts: rustc_hash::FxHashMap<BlockId, u64> = Default::default();
        let (seeds, side) = (3u64, 50);
        for seed in 1..=seeds {
            let world = Overworld::new(seed, registry.clone());
            let ores = &world.terrain.ores;
            let mut veins = Vec::new();
            for sz in -side / 2..side / 2 {
                for sx in -side / 2..side / 2 {
                    ores.square(&world, IVec2::new(sx, sz), &mut veins);
                }
            }
            for vein in &veins {
                ores.ores_in(vein, IVec2::splat(i32::MIN / 2), IVec2::splat(i32::MAX / 2), |_, ore| *counts.entry(ore.block).or_default() += 1);
            }
        }
        let squares = (seeds * (side * side) as u64) as f32;
        let per_square = |rarity: Rarity| -> Vec<(String, f32)> {
            let mut list: Vec<(String, f32)> =
                counts.iter().filter(|(b, _)| registry.def(**b).rarity == rarity).map(|(b, c)| (registry.def(*b).name.clone(), *c as f32 / squares)).collect();
            list.sort_by(|a, b| a.1.total_cmp(&b.1));
            list
        };
        let (rare, very_rare) = (per_square(Rarity::Rare), per_square(Rarity::VeryRare));
        println!("rare ores per square: {rare:?}\nvery rare ores per square: {very_rare:?}");
        assert!(!rare.is_empty() && !very_rare.is_empty());
        let (rarest_rare, commonest_very_rare) = (&rare[0], &very_rare[very_rare.len() - 1]);
        assert!(commonest_very_rare.1 < rarest_rare.1, "{commonest_very_rare:?} is no rarer than {rarest_rare:?}");
    }

    /// The base package's veins load, and every kind can be drawn somewhere.
    #[test]
    fn veins_load() {
        let registry = registry();
        let world = Overworld::new(1, registry);
        let ores = &world.terrain.ores;
        assert!(ores.kinds.len() >= 9);
        for kind in &ores.kinds {
            assert!(kind.weights.iter().any(|&w| w > 0.0), "vein {} is never drawn", kind.name);
        }
    }
}
