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
use crate::rarity::{Tier, name_hash};
use crate::rocks::{ProvinceId, Rocks};
use crate::{SEA_LEVEL, Site};

/// How far the threshold of a ground patch rises when its colour is wholly unlike the
/// ground's, in noise units: such patches become rare and small.
const PATCH_CONTRAST: f32 = 0.3;

/// How well a block's colour must sit with the main block's to count as its family.
const FAMILY: f32 = 0.3;

/// Blocks chosen by weight from a uniform value.
#[derive(Clone, Debug)]
pub struct Palette {
    blocks: Vec<BlockId>,
    /// Cumulative weights, ending at 1.
    ends: Vec<f32>,
}

impl Palette {
    /// From selectors already checked at load. A tag selects every block carrying it,
    /// each at the selector's weight times its rarity, so a stone a mod adds with the
    /// right tag joins every palette that asks for the tag.
    pub fn new(selects: &[Select], registry: &BlockRegistry) -> Self {
        Self::from_weights(Self::selected(selects, registry))
    }

    /// Like `new`, with each block weighed by how well its colour sits with the heaviest
    /// one's (`crate::hue`), grass-tinted blocks in `grass`: a mix of similar colours
    /// keeps its variety, while a clashing block is a rare speck.
    pub fn harmonious(selects: &[Select], registry: &BlockRegistry, grass: [f32; 3]) -> Self {
        let mut named = Self::selected(selects, registry);
        let shown = |block: BlockId| crate::hue::shown(registry.def(block), grass, grass);
        if let Some(&(main, _)) = named.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            let anchor = shown(main);
            for (block, weight) in &mut named {
                *weight *= crate::hue::harmony(shown(*block), anchor, crate::hue::GROUND_SPREAD);
            }
        }
        Self::from_weights(named)
    }

    /// The family of the heaviest block's colour among `selects`: those whose colour sits
    /// with it (`crate::hue`, lightness counting little), by weight, so they differ in shade
    /// rather than hue. Clashing blocks are left out altogether, and so are stones that
    /// do not belong at the surface: deep stone (tagged `deep` and not `cliff`, such as
    /// deepslate and tuff) and cobbled or crafted blocks.
    pub(crate) fn family(selects: &[Select], registry: &BlockRegistry) -> Vec<(BlockId, f32)> {
        let surface = |block: BlockId| {
            let def = registry.def(block);
            !(def.has_tag("deep") && !def.has_tag("cliff")) && !["cobbled", "building", "polished", "bricks"].iter().any(|t| def.has_tag(t))
        };
        let named: Vec<(BlockId, f32)> = Self::selected(selects, registry).into_iter().filter(|&(block, _)| surface(block)).collect();
        let color = |block: BlockId| registry.def(block).color.map(f32::from);
        let Some(&(main, _)) = named.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else { return named };
        named.into_iter().filter(|&(block, _)| crate::hue::harmony(color(block), color(main), crate::hue::GROUND_SPREAD) >= FAMILY).collect()
    }

    /// From blocks and their weights.
    pub(crate) fn weighted(named: Vec<(BlockId, f32)>) -> Self {
        Self::from_weights(named)
    }

    fn selected(selects: &[Select], registry: &BlockRegistry) -> Vec<(BlockId, f32)> {
        let mut named: Vec<(BlockId, f32)> = Vec::new();
        for select in selects {
            match select {
                Select::Name(name, weight) => named.push((registry.expect(name), *weight)),
                Select::Tag(tag, weight) => {
                    named.extend(registry.tagged(tag).map(|block| (block, weight * registry.def(block).rarity.multiplier())))
                }
            }
        }
        named
    }

    fn from_weights(named: Vec<(BlockId, f32)>) -> Self {
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

    /// The block it holds most of.
    pub fn main(&self) -> Option<BlockId> {
        let mut last = 0.0;
        let mut best = None;
        for (&block, &end) in self.blocks.iter().zip(&self.ends) {
            if best.is_none_or(|(_, share)| end - last > share) {
                best = Some((block, end - last));
            }
            last = end;
        }
        best.map(|(block, _)| block)
    }

    pub fn one(block: BlockId) -> Self {
        Self { blocks: vec![block], ends: vec![1.0] }
    }

    /// The block for a uniform value `u` in 0..1.
    pub fn pick(&self, u: f32) -> BlockId {
        // The ends rise, so the ends at or below `u` are the ones before its block. Palettes
        // are short: counting them all beats a branching search.
        let i = self.ends.iter().map(|&end| (end <= u) as usize).sum::<usize>().min(self.blocks.len() - 1);
        self.blocks[i]
    }

    /// The block for a value only computed if there is more than one to choose from.
    fn pick_with(&self, u: impl Fn() -> f32) -> BlockId {
        if self.blocks.len() == 1 { self.blocks[0] } else { self.pick(u()) }
    }

    pub fn blocks(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.blocks.iter().copied()
    }
}

/// A material compiled from data.
#[derive(Clone, Debug)]
pub(crate) enum Mat {
    Mix(Palette),
    Stones,
    Strata,
}

impl Mat {
    /// With `grass` (the biome's grass colour), mixes keep to the colour of their main block.
    pub(crate) fn new(material: &Material, registry: &BlockRegistry, grass: Option<[f32; 3]>) -> Self {
        match material {
            Material::Block(name) => Mat::Mix(Palette::one(registry.expect(name))),
            Material::Mix(selects) => Mat::Mix(match grass {
                Some(grass) => Palette::harmonious(selects, registry, grass),
                None => Palette::new(selects, registry),
            }),
            Material::Stones => Mat::Stones,
            Material::Strata => Mat::Strata,
        }
    }

    /// The block for a uniform value `u` in 0..1, at height `y` for strata.
    pub(crate) fn pick(&self, u: f32, stones: &Palette, strata: &[BlockId], y: i32) -> BlockId {
        match self {
            Mat::Mix(p) => p.pick(u),
            Mat::Stones => stones.pick(u),
            Mat::Strata => strata[y.rem_euclid(strata.len() as i32) as usize],
        }
    }
}

/// Conditions compiled from data, for other rules that test columns the way surfaces do.
#[derive(Clone, Debug, Default)]
pub(crate) struct Conditions(Vec<Cond>);

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
    Water(Vec<crate::Body>),
    Slope(f32, f32),
    Altitude(f32, f32),
    Colder(f32, f32),
    Warmer(f32, f32),
    Wetter(f32, f32),
    Drier(f32, f32),
    Forest(f32, f32),
    River(f32, f32),
    Clearing(f32, f32),
    Stand(f32, f32),
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
    /// The rock province's bands, by height: bare rock.
    Rock,
}

/// What a column offers the rules.
pub struct Spot<'a> {
    pub site: &'a Site,
    pub x: i32,
    pub z: i32,
    /// Clumpy dither for thresholds, 0..1.
    pub dither: f32,
    /// A second, independent one for choosing among mixed blocks, computed only when a
    /// mix needs it.
    pub mix: &'a dyn Fn() -> f32,
    /// Uniform per-column randomness.
    pub chance: f32,
    /// The rock province's beds of bare rock here (see `crate::rocks::Province::bands`):
    /// what rules placing the stones beneath lay, bed by bed; empty where the biome has
    /// stones of its own.
    pub bands: &'a [BlockId],
    /// Noise values already sampled here: rules often test one noise twice (a trail and
    /// its fringe, a patch and its rim).
    pub sampled: Sampled,
}

/// A few noise values sampled at one spot, by noise index.
#[derive(Default)]
pub struct Sampled {
    values: std::cell::Cell<[(u32, f32); 6]>,
    len: std::cell::Cell<usize>,
}

impl Sampled {
    fn get(&self, noise: usize, sample: impl FnOnce() -> f32) -> f32 {
        let mut values = self.values.get();
        let len = self.len.get();
        if let Some(&(_, v)) = values[..len].iter().find(|(i, _)| *i == noise as u32) {
            return v;
        }
        let v = sample();
        if len < values.len() {
            values[len] = (noise as u32, v);
            self.values.set(values);
            self.len.set(len + 1);
        }
        v
    }
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

    /// The index of a named noise field, made on first use.
    pub(crate) fn noise(&mut self, def: &NoiseDef) -> usize {
        let key = format!("{}/{}/{}", def.name, def.scale, def.octaves);
        if let Some(&i) = self.names.get(&key) {
            return i;
        }
        let fractal = if def.octaves > 1 { FractalType::FBm } else { FractalType::None };
        self.noises.push(noise(self.seed ^ name_hash(&def.name), 0x5ef, 1.0 / def.scale, fractal, def.octaves));
        self.names.insert(key, self.noises.len() - 1);
        self.noises.len() - 1
    }

    /// With `grass` (the biome's grass colour), mixed materials keep to their main block's
    /// colour (see `Mat::new`).
    pub fn compile(&mut self, def: &SurfaceDef, registry: &BlockRegistry, rocks: &Rocks, grass: Option<[f32; 3]>) -> Surface {
        let material = |m: &Material| Mat::new(m, registry, grass);
        let layers = |l: &LayersDef| LayersRule {
            top: material(&l.top),
            filler: l.filler.as_ref().map_or_else(|| material(&l.top), material),
            depth: l.depth.unwrap_or(3),
        };
        // The colour a material mostly shows; none for rock, which follows the province.
        let colour = |m: &Material, grass: [f32; 3]| -> Option<[f32; 3]> {
            let block = match m {
                Material::Block(name) => registry.expect(name),
                Material::Mix(selects) => Palette::harmonious(selects, registry, grass).main()?,
                Material::Stones | Material::Strata => return None,
            };
            Some(crate::hue::shown(registry.def(block), grass, grass))
        };
        let rules = def
            .rules
            .iter()
            .map(|r| {
                let mut when: Vec<Cond> = r.when.iter().map(|c| self.condition(c, rocks)).collect();
                // Patches on dry land of a colour unlike the ground's are fewer and smaller:
                // their threshold rises as the colours part (no change where they agree).
                let underwater = r.when.contains(&Condition::Underwater);
                if let Some(grass) = grass.filter(|_| !underwater)
                    && let (Some(ground), Some(top)) = (colour(&def.ground.top, grass), colour(&r.place.top, grass))
                {
                    let apart = 1.0 - crate::hue::harmony(top, ground, crate::hue::GROUND_SPREAD);
                    for c in &mut when {
                        if let Cond::Patch(_, threshold) = c {
                            *threshold += PATCH_CONTRAST * apart;
                        }
                    }
                }
                Rule { underwater, when, place: layers(&r.place) }
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

    pub(crate) fn conditions(&mut self, when: &[Condition], rocks: &Rocks) -> Conditions {
        Conditions(when.iter().map(|c| self.condition(c, rocks)).collect())
    }

    /// Whether all of `conditions` hold at a spot.
    pub(crate) fn hold(&self, conditions: &Conditions, spot: &Spot) -> bool {
        conditions.0.iter().all(|c| self.holds(c, spot))
    }

    fn condition(&mut self, c: &Condition, rocks: &Rocks) -> Cond {
        match c {
            Condition::Underwater => Cond::Underwater,
            Condition::Deeper(a, b) => Cond::Deeper(*a, *b),
            Condition::Water(bodies) => Cond::Water(bodies.clone()),
            Condition::Slope(a, b) => Cond::Slope(*a, *b),
            Condition::Altitude(a, b) => Cond::Altitude(*a, *b),
            Condition::Colder(a, b) => Cond::Colder(*a, *b),
            Condition::Warmer(a, b) => Cond::Warmer(*a, *b),
            Condition::Wetter(a, b) => Cond::Wetter(*a, *b),
            Condition::Drier(a, b) => Cond::Drier(*a, *b),
            Condition::Forest(a, b) => Cond::Forest(*a, *b),
            Condition::River(a, b) => Cond::River(*a, *b),
            Condition::Clearing(a, b) => Cond::Clearing(*a, *b),
            Condition::Stand(a, b) => Cond::Stand(*a, *b),
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

    /// A noise field's value at a position, −1 ..= 1.
    pub(crate) fn sample(&self, noise: usize, x: i32, z: i32) -> f32 {
        self.noises[noise].get_noise_2d(x as f32, z as f32)
    }

    fn holds(&self, c: &Cond, spot: &Spot) -> bool {
        let site = spot.site;
        let at = |a: f32, b: f32| a + (b - a) * spot.dither;
        let noise = |i: usize| spot.sampled.get(i, || self.noises[i].get_noise_2d(spot.x as f32, spot.z as f32));
        let above = (site.height - SEA_LEVEL) as f32;
        match c {
            Cond::Underwater => true,
            Cond::Deeper(a, b) => (site.water - site.height) as f32 > at(*a, *b),
            Cond::Water(bodies) => bodies.contains(&site.body),
            Cond::Slope(a, b) => site.slope as f32 > at(*a, *b),
            Cond::Altitude(a, b) => above > at(*a, *b),
            Cond::Colder(a, b) => site.temperature < at(*a, *b),
            Cond::Warmer(a, b) => site.temperature > at(*a, *b),
            Cond::Wetter(a, b) => site.soil_moisture() > at(*a, *b),
            Cond::Drier(a, b) => site.soil_moisture() < at(*a, *b),
            Cond::Forest(a, b) => site.forest > at(*a, *b),
            Cond::River(a, b) => site.river > at(*a, *b),
            Cond::Clearing(a, b) => site.clearing > at(*a, *b),
            Cond::Stand(a, b) => site.stand > at(*a, *b),
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

    /// The lining of a cave floor (`ground`, or `underwater` when `underwater`) for an
    /// underground biome's surface; `None` where it is the stones beneath, which stay as
    /// they are.
    pub(crate) fn cave_floor(&self, surface: &Surface, spot: &Spot, underwater: bool, stones: &Palette, strata: &[BlockId]) -> Option<Layers> {
        let rule = self.first(surface, spot, underwater, if underwater { &surface.underwater } else { &surface.ground });
        (!matches!(rule.top, Mat::Stones)).then(|| resolve(rule, spot, stones, strata))
    }

    /// The lining of a cave ceiling (an underground biome's `cliff`), if it is not the
    /// stones beneath.
    pub(crate) fn cave_ceiling(&self, surface: &Surface, spot: &Spot, stones: &Palette, strata: &[BlockId]) -> Option<BlockId> {
        (!matches!(surface.cliff, Mat::Stones)).then(|| surface.cliff.pick((spot.mix)(), stones, strata, spot.site.height))
    }

    /// Bare rock of steep slopes and exposed heights. Banded cliffs show their bands all
    /// the way down a riser, not just along its rim; where the cliff is the stones beneath
    /// and the rock province lays them in beds, the beds (`Fill::Rock`, whose top the
    /// caller sets by height).
    pub(crate) fn cliff(&self, surface: &Surface, spot: &Spot, stones: &Palette, strata: &[BlockId]) -> Layers {
        if matches!(surface.cliff, Mat::Stones) && !spot.bands.is_empty() {
            return Layers { top: spot.bands[0], filler: Fill::Rock, depth: CLIFF_DEPTH };
        }
        let (filler, depth) = if matches!(surface.cliff, Mat::Strata) { (Mat::Strata, CLIFF_DEPTH) } else { (Mat::Stones, 0) };
        let rock = LayersRule { top: surface.cliff.clone(), filler, depth };
        resolve(&rock, spot, stones, strata)
    }
}

/// How far down a cliff's column its bands go: past the foot of most risers.
const CLIFF_DEPTH: i32 = 24;
/// How far down the beds of rock on a slope go: past the steps beside it.
const ROCK_DEPTH: i32 = 4;

fn resolve(rule: &LayersRule, spot: &Spot, stones: &Palette, strata: &[BlockId]) -> Layers {
    // Bare rock is the province's beds, a few blocks down, whose top the caller sets by
    // height (see `Fill::Rock`).
    if matches!(rule.top, Mat::Stones) && !spot.bands.is_empty() {
        return Layers { top: spot.bands[0], filler: Fill::Rock, depth: ROCK_DEPTH };
    }
    let band = |y: i32| strata[y.rem_euclid(strata.len() as i32) as usize];
    let top = match &rule.top {
        Mat::Mix(p) => p.pick_with(spot.mix),
        Mat::Stones => stones.pick_with(spot.mix),
        Mat::Strata => band(spot.site.height),
    };
    let filler = match &rule.filler {
        Mat::Mix(p) => Fill::Block(p.pick_with(spot.mix)),
        Mat::Stones => Fill::Stones,
        Mat::Strata => Fill::Strata,
    };
    Layers { top, filler, depth: rule.depth }
}
