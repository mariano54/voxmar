//! Cave biomes: which underground biome each block of rock and cave belongs to, and how
//! that biome lines and decorates its caves.
//!
//! Underground biomes are ordinary biome definitions (`layer: Underground`, kept in
//! `cave_biomes.ron`) placed by the surface's picker: the climate above (temperature,
//! moisture, continentalness, landform and aether) selects a cell of the band table, a
//! claim may also ask for a range of depth below the surface, of height and a set of rock
//! provinces, and the claimants of a cell split it along an evenly spread weirdness field,
//! rare ones in its extremes. Underground that field and the variant field are 3D, so cave
//! biomes change with depth as well as across the land: a lush layer above a dripstone
//! one, a frozen grotto that gives way to plain rock. The fields vary over about 600
//! blocks across and 150 up and down, so cave biomes are wide regions stacked in layers.
//!
//! Picks are made on a lattice every 8 blocks, and each block takes the pick of a nearby
//! lattice point, jittered per block, so the borders between cave biomes are ragged clumps
//! rather than planes and a column costs a few hundred picks.
//!
//! A cave biome then
//! - replaces the rock in its volume with its own stones or strata, when it has them;
//! - lines cave floors with its surface's `ground` (`underwater` under water) and ceilings
//!   with its `cliff`, unless those are the stones beneath;
//! - grows its `cave_decoration` from floors and ceilings: plants and mushrooms,
//!   stalagmites and stalactites, icicles, roots and vines, crystals, some of them glowing;
//! - freezes the surface of its pools, when it is tagged `frozen`.
//!
//! Decorations grow straight up or down within their block column and only into open air,
//! so they never cross a column border, never displace water or lava and never leave a
//! fluid beside air.

use std::sync::LazyLock;

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::CHUNK_SIZE;
use voxmar_engine::glam::IVec3;

use crate::biome::{BiomeId, Biomes, Point};
use crate::caves::Cave;
use crate::content::{Layer, Material, Side, WorldContent};
use crate::geology::{GeoGrid, noise};
use crate::rng::hash3;
use crate::rocks::{ProvinceId, Rocks};
use crate::surface::{Conditions, Fill, Mat, Palette, Spot, SurfaceRules};
use crate::terrain::Blocks;
use crate::{Overworld, SEA_LEVEL, Sampler, Site};

/// Spacing of the lattice cave biomes are picked on, in blocks.
const STEP: i32 = 8;
/// Lattice points along each side of a column, including the far edge.
const SIDE: usize = (CHUNK_SIZE / STEP + 1) as usize;
/// Typical size of a cave biome's regions across, in blocks, and how much faster they
/// change with depth: regions spread wide and stack in layers.
const SCALE: f32 = 600.0;
const SQUASH: f32 = 4.0;

const PURPOSE: u64 = 0xcafe;

struct Decoration {
    side: Side,
    chance: f32,
    when: Conditions,
    /// The floor or ceiling blocks it grows on, by block id; any block when `None`.
    on: Option<Vec<bool>>,
    block: Mat,
    length: (i32, i32),
    tip: Option<(Mat, f32)>,
}

/// What a cave biome does to its caves: whether it lines floors and ceilings and grows
/// decorations, so the rest can be skipped.
#[derive(Clone, Copy, Default)]
struct Work {
    floor: bool,
    ceiling: bool,
    decorations: bool,
    /// Pools freeze over (cave biomes tagged `frozen`).
    freezes: bool,
}

/// The cave biomes of one world.
pub struct Underground {
    weirdness: FastNoiseLite,
    variant: FastNoiseLite,
    /// Each biome's decorations, by biome id; none for surface biomes.
    decorations: Vec<Vec<Decoration>>,
    /// Whether each biome has rock of its own.
    rocky: Vec<bool>,
    /// What each biome does to its caves, by biome id.
    work: Vec<Work>,
    /// What still water freezes into.
    ice: BlockId,
    seed: u64,
}

/// A cave biome's own rock.
pub(crate) enum Rock<'a> {
    Stones(&'a Palette),
    Strata(&'a [BlockId]),
}

impl Underground {
    pub fn new(seed: u64, content: &WorldContent, registry: &BlockRegistry, biomes: &Biomes, rules: &mut SurfaceRules, rocks: &Rocks) -> Self {
        let decorations = content
            .biomes
            .iter()
            .map(|def| {
                def.cave_decoration
                    .iter()
                    .map(|d| Decoration {
                        side: d.on,
                        chance: d.chance,
                        when: rules.conditions(&d.when, rocks),
                        on: (!d.on_tags.is_empty()).then(|| {
                            (0..registry.len()).map(|i| d.on_tags.iter().any(|t| registry.def(BlockId(i as u16)).has_tag(t))).collect()
                        }),
                        block: Mat::new(&d.block, registry),
                        length: d.length,
                        tip: d.tip.as_ref().map(|(m, chance)| (Mat::new(m, registry), *chance)),
                    })
                    .collect()
            })
            .collect();
        let rocky = biomes.iter().map(|(_, b)| b.layer == Layer::Underground && (b.stones.is_some() || !b.strata.is_empty())).collect();
        let work = content
            .biomes
            .iter()
            .map(|def| {
                let s = &def.surface;
                let lined = |l: &crate::content::LayersDef| l.top != Material::Stones;
                Work {
                    floor: def.layer == Layer::Underground && (lined(&s.ground) || lined(&s.underwater) || !s.rules.is_empty()),
                    ceiling: def.layer == Layer::Underground && s.cliff.as_ref().is_some_and(|c| *c != Material::Stones),
                    decorations: !def.cave_decoration.is_empty(),
                    freezes: def.layer == Layer::Underground && def.tags.iter().any(|t| t == "frozen"),
                }
            })
            .collect();
        Self {
            weirdness: noise(seed, 60, 1.0 / SCALE, FractalType::FBm, 2),
            variant: noise(seed, 61, 1.0 / (0.6 * SCALE), FractalType::FBm, 2),
            decorations,
            rocky,
            work,
            ice: registry.expect(&content.climate.frost.ice),
            seed: seed ^ PURPOSE,
        }
    }

    /// The picker's point for a cave at `p` over the surface point `surface`.
    fn point(&self, surface: &Point, p: IVec3) -> Point {
        let (x, y, z) = (p.x as f32, p.y as f32, p.z as f32);
        let mut point = *surface;
        point.0[4] = uniform3(self.weirdness.get_noise_3d(x, y * SQUASH, z));
        point.0[6] = uniform3(self.variant.get_noise_3d(x, y * SQUASH, z));
        point
    }

    /// Picks the cave biomes of the column at `origin`, whose ground (with a one-block
    /// border) `geology` holds, up to height `top`; `area` must cover the column.
    pub(crate) fn column<'a>(&'a self, world: &'a Overworld, area: &Sampler, geology: &GeoGrid, origin: IVec3, top: i32) -> UndergroundColumn<'a> {
        let _span = tracing::info_span!("cave_biomes").entered();
        let layers = (top / STEP + 2) as usize;
        let mut picks = vec![None; SIDE * SIDE * layers];
        let mut rocky = [false; SIDE * SIDE];
        for lz in 0..SIDE {
            for lx in 0..SIDE {
                let (x, z) = (origin.x + lx as i32 * STEP, origin.z + lz as i32 * STEP);
                let (surface, province) = area.underground_point(x, z);
                let height = geology.get(x, z).height;
                for layer in 0..layers {
                    let y = layer as i32 * STEP;
                    if y > height {
                        break;
                    }
                    let point = self.point(&surface, IVec3::new(x, y, z));
                    let pick = world.biomes.underground(&point, (height - y) as f32, (y - SEA_LEVEL) as f32, province);
                    rocky[lz * SIDE + lx] |= pick.is_some_and(|b| self.rocky[b.0 as usize]);
                    picks[(lz * SIDE + lx) * layers + layer] = pick;
                }
            }
        }
        UndergroundColumn { underground: self, world, origin, layers, picks, rocky }
    }

    /// The lattice point nearest to `pos`, whose pick its block most likely takes.
    pub(crate) fn nearest(pos: IVec3) -> IVec3 {
        (pos.as_vec3() / STEP as f32).round().as_ivec3() * STEP
    }

    /// The cave biome picked at the lattice point `at`, given the ground's height, the
    /// surface's picker point and the rock province there.
    pub(crate) fn pick(&self, biomes: &Biomes, at: IVec3, height: i32, surface: &Point, province: ProvinceId) -> Option<BiomeId> {
        if at.y > height {
            return None;
        }
        biomes.underground(&self.point(surface, at), (height - at.y) as f32, (at.y - SEA_LEVEL) as f32, province)
    }
}

/// The cave biomes of one column, picked on a lattice.
pub(crate) struct UndergroundColumn<'a> {
    underground: &'a Underground,
    world: &'a Overworld,
    origin: IVec3,
    layers: usize,
    /// Picks by lattice column (z-major), then layer.
    picks: Vec<Option<BiomeId>>,
    /// Whether any pick of each lattice column has rock of its own.
    rocky: [bool; SIDE * SIDE],
}

impl UndergroundColumn<'_> {
    /// The cave biome of the block at (x, y, z) relative to the column origin: the pick of
    /// one of the eight lattice points around it, each with its trilinear weight, so one
    /// biome gives way to the next in a dithered gradient across a lattice cell.
    pub(crate) fn biome(&self, x: i32, y: i32, z: i32) -> Option<BiomeId> {
        let h = hash3(self.underground.seed, self.origin.x + x, y, self.origin.z + z);
        // Eight bits of the hash per axis against the position within the cell.
        let corner = |v: i32, shift: u32, most: usize| {
            let (cell, within) = (v.div_euclid(STEP), v.rem_euclid(STEP));
            let up = ((h >> shift) & 0xff) as i32 * STEP < within * 256;
            ((cell + up as i32) as usize).min(most - 1)
        };
        let (lx, ly, lz) = (corner(x, 0, SIDE), corner(y, 8, self.layers), corner(z, 16, SIDE));
        self.picks[(lz * SIDE + lx) * self.layers + ly]
    }

    /// Whether any cave biome near the block column at (x, z) has rock of its own.
    pub(crate) fn rocky(&self, x: i32, z: i32) -> bool {
        let (lx, lz) = ((x / STEP) as usize, (z / STEP) as usize);
        self.rocky[lz * SIDE + lx] || self.rocky[lz * SIDE + lx + 1] || self.rocky[(lz + 1) * SIDE + lx] || self.rocky[(lz + 1) * SIDE + lx + 1]
    }

    /// The rock of the cave biome at (x, y, z), if it has its own.
    pub(crate) fn rock(&self, x: i32, y: i32, z: i32) -> Option<Rock<'_>> {
        let biome = self.world.biomes.get(self.biome(x, y, z)?);
        if !biome.strata.is_empty() {
            Some(Rock::Strata(&biome.strata))
        } else {
            biome.stones.as_ref().map(Rock::Stones)
        }
    }

    /// Lines the floors and ceilings of the caves in the block column at (x, z) (relative
    /// to the column origin), whose open blocks `cave` gives, and grows their decorations.
    pub(crate) fn decorate(&self, x: i32, z: i32, site: &Site, cave: &[Cave], blocks: &mut Blocks) {
        let world = self.world;
        let open = |y: i32| y >= 0 && (y as usize) < cave.len() && cave[y as usize] != Cave::Rock;
        let (wx, wz) = (self.origin.x + x, self.origin.z + z);
        // What the column offers the rules; only the height changes from floor to floor.
        let (dither, chance) = (world.terrain.dither(wx, wz), world.terrain.chance(wx, wz));
        let mix = std::cell::OnceCell::new();
        let mix = || *mix.get_or_init(|| world.terrain.mix(wx, wz));
        let column_stones = world.stones(site);
        for y in 1..cave.len() as i32 - 1 {
            if cave[y as usize] == Cave::Water && cave[y as usize + 1] == Cave::Air {
                // The surface of a pool, which freezes in frozen caves.
                if self.biome(x, y, z).is_some_and(|id| self.underground.work[id.0 as usize].freezes) {
                    blocks.set(IVec3::new(wx, y, wz), self.underground.ice);
                }
                continue;
            }
            if cave[y as usize] != Cave::Rock {
                continue;
            }
            let (floor, ceiling) = (open(y + 1), open(y - 1));
            if !floor && !ceiling {
                continue;
            }
            let Some(id) = self.biome(x, y, z) else { continue };
            let biome = world.biomes.get(id);
            let work = &self.underground.work[id.0 as usize];
            if !(floor && work.floor || ceiling && work.ceiling || work.decorations) {
                continue;
            }
            // The stones beneath: the cave biome's own, or those of the column.
            let stones = biome.stones.as_ref().unwrap_or(column_stones);
            let at = Site { height: y, slope: 0, ..*site };
            let spot = Spot { site: &at, x: wx, z: wz, dither, mix: &mix, chance };
            let pos = IVec3::new(wx, y, wz);
            if floor && work.floor {
                let underwater = cave[y as usize + 1] == Cave::Water;
                if let Some(layers) = world.rules.cave_floor(&biome.surface, &spot, underwater, stones, &biome.strata) {
                    blocks.replace_stone(pos, layers.top);
                    for k in 1..=layers.depth {
                        if open(y - k) {
                            break;
                        }
                        let below = pos - IVec3::Y * k;
                        match layers.filler {
                            Fill::Block(block) => blocks.replace_stone(below, block),
                            Fill::Strata => blocks.replace_stone(below, biome.strata[(y - k).rem_euclid(biome.strata.len() as i32) as usize]),
                            Fill::Stones => {}
                        }
                    }
                }
            }
            if ceiling && work.ceiling && let Some(block) = world.rules.cave_ceiling(&biome.surface, &spot, stones, &biome.strata) {
                blocks.replace_stone(pos, block);
            }
            for (side, grows) in [(Side::Floor, floor && cave[y as usize + 1] == Cave::Air), (Side::Ceiling, ceiling && cave[y as usize - 1] == Cave::Air)] {
                if grows {
                    self.grow(id, side, &spot, stones, &biome.strata, pos, cave, blocks);
                }
            }
        }
    }

    /// Grows the first decoration of `biome` on `side` that the block at `pos` rolls.
    #[allow(clippy::too_many_arguments)]
    fn grow(&self, biome: BiomeId, side: Side, spot: &Spot, stones: &Palette, strata: &[BlockId], pos: IVec3, cave: &[Cave], blocks: &mut Blocks) {
        let decorations = &self.underground.decorations[biome.0 as usize];
        if decorations.is_empty() {
            return;
        }
        let salt = if side == Side::Floor { 0x51 } else { 0xce };
        let h = hash3(self.underground.seed ^ salt ^ (biome.0 as u64) << 20, pos.x, pos.y, pos.z);
        let unit = |bits: u64| (bits & 0xff_ffff) as f32 / (1u32 << 24) as f32;
        let mut roll = unit(h);
        let rooted_on = blocks.get(pos);
        for d in decorations.iter().filter(|d| d.side == side) {
            if roll >= d.chance {
                roll -= d.chance;
                continue;
            }
            if d.on.as_ref().is_some_and(|on| !on[rooted_on.0 as usize]) || !self.world.rules.hold(&d.when, spot) {
                return;
            }
            let step = if side == Side::Floor { IVec3::Y } else { IVec3::NEG_Y };
            let (lo, hi) = d.length;
            let length = lo + ((h >> 24) % (hi - lo + 1) as u64) as i32;
            let tip = d.tip.as_ref().filter(|(_, chance)| unit(h >> 40) < *chance);
            let u = unit(h >> 32);
            for k in 1..=length {
                let at = pos + step * k;
                if cave.get(at.y as usize) != Some(&Cave::Air) || !blocks.get(at).is_air() {
                    break;
                }
                let block = match tip {
                    Some((tip, _)) if k == length => tip,
                    _ => &d.block,
                };
                blocks.put_if_air(at, block.pick(u, stones, strata, at.y));
            }
            return;
        }
    }
}

/// Spreads a two-octave 3D noise value evenly over 0 ..= 1 through its measured
/// distribution, so a share of the range is the same share of the underground.
fn uniform3(n: f32) -> f32 {
    const QUANTILES: usize = 256;
    static TABLE: LazyLock<Vec<f32>> = LazyLock::new(|| {
        let n = noise(0x3d_c4fe, 0, 1.0, FractalType::FBm, 2);
        let mut samples: Vec<f32> =
            (0..1 << 16).map(|i| n.get_noise_3d((i & 63) as f32 * 2.3, ((i >> 6) & 31) as f32 * 2.3, (i >> 11) as f32 * 2.3)).collect();
        samples.sort_by(f32::total_cmp);
        (0..=QUANTILES).map(|q| samples[(q * (samples.len() - 1)) / QUANTILES]).collect()
    });
    let i = TABLE.partition_point(|&q| q < n).clamp(1, QUANTILES);
    let (lo, hi) = (TABLE[i - 1], TABLE[i]);
    ((i - 1) as f32 + ((n - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)) / QUANTILES as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform3_spreads_evenly() {
        let n = noise(77, 1, 1.0 / SCALE, FractalType::FBm, 2);
        let mut buckets = [0; 10];
        for i in 0..20_000 {
            let (x, y, z) = ((i % 37) as f32 * 61.0, (i / 37 % 23) as f32 * 53.0, (i / 851) as f32 * 59.0);
            buckets[((uniform3(n.get_noise_3d(x, y, z)) * 10.0) as usize).min(9)] += 1;
        }
        assert!(buckets.iter().all(|&b| (1500..2500).contains(&b)), "{buckets:?}");
    }

    /// Every cave biome grows only where its claims allow: by climate, depth, height,
    /// rock province and aether; plain caves fill the rest, and every base cave biome
    /// turns up somewhere.
    #[test]
    fn cave_biomes_keep_to_their_claims() {
        use crate::rarity::Featured;
        use crate::rng::Rng;
        let registry = BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
        let content = WorldContent::base(&registry).unwrap();
        let featured = Featured::new(3, 0.5);
        let rocks = Rocks::new(3, &content, &registry, &featured);
        let biomes = Biomes::new(&content, &registry, &mut SurfaceRules::new(3, &content, &registry), &rocks, &featured);
        let name = |id: BiomeId| biomes.get(id).name.as_str();
        let mut rng = Rng::new(9, 9, 9, 9);
        let mut counts: rustc_hash::FxHashMap<&str, usize> = Default::default();
        for _ in 0..200_000 {
            let mut unit = || rng.unit();
            let (t, m, a) = (-25.0 + 60.0 * unit(), 2.0 * unit(), unit());
            let p = Point([t, m, -1.0 + 2.0 * unit(), unit(), unit(), a, unit()]);
            let (depth, height) = (200.0 * unit(), -124.0 + 250.0 * unit());
            let province = ProvinceId((unit() * rocks.len() as f32) as u8);
            let biome = biomes.underground(&p, depth, height, province).expect("plain caves claim everywhere");
            assert_eq!(biomes.get(biome).layer, Layer::Underground);
            let province_is = |tag: &str| rocks.matches(province, tag);
            let ok = match name(biome) {
                "base:frozen_caves" => t < 4.0 && depth < 70.0,
                "base:molten_caves" => height < -96.0 || height < -60.0 && province_is("volcanic"),
                "base:deep_dark" => depth >= 80.0 && height < -40.0,
                "base:painted_caves" => (t >= 17.0 && m < 0.6 || province_is("sedimentary")) && depth >= 6.0,
                "base:crystal_caves" => (province_is("igneous") || province_is("metamorphic")) && depth >= 24.0,
                "base:glowroot_hollows" => a >= 0.6,
                "base:starlit_grotto" => a >= 0.7,
                _ => true,
            };
            assert!(ok, "{} at t {t}, m {m}, aether {a}, depth {depth}, height {height}, {}", name(biome), rocks.get(province).name);
            *counts.entry(name(biome)).or_default() += 1;
        }
        let underground = biomes.iter().filter(|(_, b)| b.layer == Layer::Underground).count();
        assert_eq!(counts.len(), underground, "{counts:?}");
        let most = counts.iter().max_by_key(|c| c.1).unwrap();
        assert_eq!(*most.0, "base:cave", "{counts:?}");
    }
}
