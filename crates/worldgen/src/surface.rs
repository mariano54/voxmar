//! Surface rules: how a biome's ground is made, compiled from data.
//!
//! A biome's surface is an ordered list of rules, each a few conditions on the column
//! (slope, altitude, temperature, moisture, tree cover, nearness to water, rock province,
//! named noise fields) and the layers to place when they all hold. Thresholds come in
//! pairs and are dithered between them in clumps, so the borders they draw are ragged.
//! Noise conditions draw patches (`n > t`), lines along a zero contour (`|n| < w`, for
//! trails and creeks) and lines whose width follows a second noise (tapering rivulets),
//! as in Terralith's surface rules. Everything is decided per column from pure fields.
//!
//! Noise fields are seeded by name, so two rules naming the same field share it (a trail
//! crossing several biomes stays one trail) and adding a field never reseeds another.

use fastnoise_lite::{FastNoiseLite, FractalType};
use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, BlockRegistry};

use crate::content::{Condition, LayersDef, Material, NoiseDef, Select, SurfaceDef, WorldContent};
use crate::geology::noise;
use crate::rarity::name_hash;
use crate::rocks::{ProvinceId, Rocks};
use crate::{SEA_LEVEL, Site};

/// Blocks chosen by weight from a uniform value.
#[derive(Clone, Debug)]
pub struct Palette {
    blocks: Vec<BlockId>,
    /// Cumulative weights, ending at 1.
    ends: Vec<f32>,
}

impl Palette {
    /// From selectors already checked at load. Tags select no blocks yet.
    pub fn new(selects: &[Select], registry: &BlockRegistry) -> Self {
        let named: Vec<(BlockId, f32)> = selects
            .iter()
            .filter_map(|s| match s {
                Select::Name(name, weight) => Some((registry.expect(name), *weight)),
                Select::Tag(..) => None,
            })
            .collect();
        let total: f32 = named.iter().map(|n| n.1).sum();
        let mut sum = 0.0;
        let ends = named
            .iter()
            .map(|(_, w)| {
                sum += w / total;
                sum
            })
            .collect();
        Self { blocks: named.into_iter().map(|n| n.0).collect(), ends }
    }

    pub fn one(block: BlockId) -> Self {
        Self { blocks: vec![block], ends: vec![1.0] }
    }

    /// The block for a uniform value `u` in 0..1.
    pub fn pick(&self, u: f32) -> BlockId {
        let i = self.ends.partition_point(|&end| end <= u).min(self.blocks.len() - 1);
        self.blocks[i]
    }

    pub fn blocks(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.blocks.iter().copied()
    }
}

#[derive(Clone, Debug)]
enum Mat {
    Mix(Palette),
    Stones,
    Strata,
}

#[derive(Clone, Debug)]
struct LayersRule {
    top: Mat,
    filler: Mat,
    depth: i32,
}

#[derive(Clone, Debug)]
enum Cond {
    Underwater,
    Deeper(f32, f32),
    Slope(f32, f32),
    Altitude(f32, f32),
    Colder(f32, f32),
    Warmer(f32, f32),
    Wetter(f32, f32),
    Drier(f32, f32),
    Forest(f32, f32),
    River(f32, f32),
    /// A bit per matching province.
    Province(u64),
    Band(usize, f32),
    Tapered(usize, usize, Vec<(f32, f32)>),
    Patch(usize, f32),
    Chance(f32),
    Not(Box<Cond>),
    Any(Vec<Cond>),
}

#[derive(Clone, Debug)]
struct Rule {
    underwater: bool,
    when: Vec<Cond>,
    place: LayersRule,
}

/// A biome's compiled surface.
#[derive(Clone, Debug)]
pub struct Surface {
    rules: Vec<Rule>,
    ground: LayersRule,
    underwater: LayersRule,
    shore: Option<LayersRule>,
    cliff: Mat,
}

/// The column's surface, resolved to blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layers {
    pub top: BlockId,
    pub filler: Fill,
    /// Blocks of filler beneath the top.
    pub depth: i32,
}

/// What lies beneath the top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    Block(BlockId),
    /// The stones beneath, block by block.
    Stones,
    /// The biome's strata, by height.
    Strata,
}

/// What a column offers the rules.
pub struct Spot<'a> {
    pub site: &'a Site,
    pub x: i32,
    pub z: i32,
    /// Clumpy dither for thresholds, 0..1.
    pub dither: f32,
    /// A second, independent one for choosing among mixed blocks.
    pub mix: f32,
    /// Uniform per-column randomness.
    pub chance: f32,
}

/// The named noise fields of every biome's rules.
pub struct SurfaceRules {
    noises: Vec<FastNoiseLite>,
    names: FxHashMap<String, usize>,
    seed: u64,
}

impl SurfaceRules {
    pub fn new(seed: u64, _content: &WorldContent, _registry: &BlockRegistry) -> Self {
        Self { noises: Vec::new(), names: FxHashMap::default(), seed }
    }

    fn noise(&mut self, def: &NoiseDef) -> usize {
        let key = format!("{}/{}/{}", def.name, def.scale, def.octaves);
        if let Some(&i) = self.names.get(&key) {
            return i;
        }
        let fractal = if def.octaves > 1 { FractalType::FBm } else { FractalType::None };
        self.noises.push(noise(self.seed ^ name_hash(&def.name), 0x5ef, 1.0 / def.scale, fractal, def.octaves));
        self.names.insert(key, self.noises.len() - 1);
        self.noises.len() - 1
    }

    pub fn compile(&mut self, def: &SurfaceDef, registry: &BlockRegistry, rocks: &Rocks) -> Surface {
        let material = |m: &Material| match m {
            Material::Block(name) => Mat::Mix(Palette::one(registry.expect(name))),
            Material::Mix(selects) => Mat::Mix(Palette::new(selects, registry)),
            Material::Stones => Mat::Stones,
            Material::Strata => Mat::Strata,
        };
        let layers = |l: &LayersDef| LayersRule {
            top: material(&l.top),
            filler: l.filler.as_ref().map_or_else(|| material(&l.top), material),
            depth: l.depth.unwrap_or(3),
        };
        let rules = def
            .rules
            .iter()
            .map(|r| Rule {
                underwater: r.when.contains(&Condition::Underwater),
                when: r.when.iter().map(|c| self.condition(c, rocks)).collect(),
                place: layers(&r.place),
            })
            .collect();
        Surface {
            rules,
            ground: layers(&def.ground),
            underwater: layers(&def.underwater),
            shore: def.shore.as_ref().map(layers),
            cliff: def.cliff.as_ref().map_or(Mat::Stones, material),
        }
    }

    fn condition(&mut self, c: &Condition, rocks: &Rocks) -> Cond {
        match c {
            Condition::Underwater => Cond::Underwater,
            Condition::Deeper(a, b) => Cond::Deeper(*a, *b),
            Condition::Slope(a, b) => Cond::Slope(*a, *b),
            Condition::Altitude(a, b) => Cond::Altitude(*a, *b),
            Condition::Colder(a, b) => Cond::Colder(*a, *b),
            Condition::Warmer(a, b) => Cond::Warmer(*a, *b),
            Condition::Wetter(a, b) => Cond::Wetter(*a, *b),
            Condition::Drier(a, b) => Cond::Drier(*a, *b),
            Condition::Forest(a, b) => Cond::Forest(*a, *b),
            Condition::River(a, b) => Cond::River(*a, *b),
            Condition::Province(name) => {
                Cond::Province((0..rocks.len().min(64)).filter(|&i| rocks.matches(ProvinceId(i as u8), name)).fold(0, |m, i| m | 1 << i))
            }
            Condition::Band(n, w) => Cond::Band(self.noise(n), *w),
            Condition::Tapered(n, by, widths) => Cond::Tapered(self.noise(n), self.noise(by), widths.clone()),
            Condition::Patch(n, t) => Cond::Patch(self.noise(n), *t),
            Condition::Chance(p) => Cond::Chance(*p),
            Condition::Not(inner) => Cond::Not(Box::new(self.condition(inner, rocks))),
            Condition::Any(list) => Cond::Any(list.iter().map(|c| self.condition(c, rocks)).collect()),
        }
    }

    fn holds(&self, c: &Cond, spot: &Spot) -> bool {
        let site = spot.site;
        let at = |a: f32, b: f32| a + (b - a) * spot.dither;
        let noise = |i: usize| self.noises[i].get_noise_2d(spot.x as f32, spot.z as f32);
        let above = (site.height - SEA_LEVEL) as f32;
        match c {
            Cond::Underwater => true,
            Cond::Deeper(a, b) => -above > at(*a, *b),
            Cond::Slope(a, b) => site.slope as f32 > at(*a, *b),
            Cond::Altitude(a, b) => above > at(*a, *b),
            Cond::Colder(a, b) => site.temperature < at(*a, *b),
            Cond::Warmer(a, b) => site.temperature > at(*a, *b),
            Cond::Wetter(a, b) => site.soil_moisture() > at(*a, *b),
            Cond::Drier(a, b) => site.soil_moisture() < at(*a, *b),
            Cond::Forest(a, b) => site.forest > at(*a, *b),
            Cond::River(a, b) => site.river > at(*a, *b),
            Cond::Province(mask) => mask & (1 << site.province.0) != 0,
            Cond::Band(n, w) => noise(*n).abs() < *w,
            Cond::Tapered(n, by, widths) => {
                let by = noise(*by);
                widths.iter().find(|(threshold, _)| by >= *threshold).is_some_and(|(_, w)| noise(*n).abs() < *w)
            }
            Cond::Patch(n, t) => noise(*n) > *t,
            Cond::Chance(p) => spot.chance < *p,
            Cond::Not(inner) => !self.holds(inner, spot),
            Cond::Any(list) => list.iter().any(|c| self.holds(c, spot)),
        }
    }

    /// The first rule of `surface` for this column that holds, or `fallback`.
    fn first<'a>(&self, surface: &'a Surface, spot: &Spot, underwater: bool, fallback: &'a LayersRule) -> &'a LayersRule {
        surface
            .rules
            .iter()
            .find(|r| r.underwater == underwater && r.when.iter().all(|c| self.holds(c, spot)))
            .map_or(fallback, |r| &r.place)
    }

    /// The layers of a submerged column.
    pub(crate) fn underwater(&self, surface: &Surface, spot: &Spot, stones: &Palette, strata: &[BlockId]) -> Layers {
        resolve(self.first(surface, spot, true, &surface.underwater), spot, stones, strata)
    }

    /// The layers of a dry column, before cliffs and frost.
    pub(crate) fn ground(&self, surface: &Surface, spot: &Spot, shore: bool, stones: &Palette, strata: &[BlockId]) -> Layers {
        let fallback = match (&surface.shore, shore) {
            (Some(s), true) => s,
            _ => &surface.ground,
        };
        resolve(self.first(surface, spot, false, fallback), spot, stones, strata)
    }

    /// Bare rock of steep slopes and exposed heights.
    pub(crate) fn cliff(&self, surface: &Surface, spot: &Spot, stones: &Palette, strata: &[BlockId]) -> Layers {
        let rock = LayersRule { top: surface.cliff.clone(), filler: Mat::Stones, depth: 0 };
        resolve(&rock, spot, stones, strata)
    }
}

fn resolve(rule: &LayersRule, spot: &Spot, stones: &Palette, strata: &[BlockId]) -> Layers {
    let band = |y: i32| strata[y.rem_euclid(strata.len() as i32) as usize];
    let top = match &rule.top {
        Mat::Mix(p) => p.pick(spot.mix),
        Mat::Stones => stones.pick(spot.mix),
        Mat::Strata => band(spot.site.height),
    };
    let filler = match &rule.filler {
        Mat::Mix(p) => Fill::Block(p.pick(spot.mix)),
        Mat::Stones => Fill::Stones,
        Mat::Strata => Fill::Strata,
    };
    Layers { top, filler, depth: rule.depth }
}
