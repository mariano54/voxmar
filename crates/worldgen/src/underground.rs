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
//! Picks are made on a lattice every 8 blocks, and each block takes the pick of one of the
//! eight lattice points around it, chosen by trilinear weight, so one cave biome gives way
//! to the next in a dithered gradient and a column costs a few hundred picks.
//!
//! A cave biome then
//! - replaces the rock in its volume with its own stones or strata, when it has them;
//! - lines cave floors with its surface's `ground` (`underwater` under water) and ceilings
//!   with its `cliff`, unless those are the stones beneath;
//! - grows its `cave_decoration` from floors and ceilings: plants and mushrooms,
//!   stalagmites and stalactites, icicles, roots and vines, crystals, some of them glowing;
//! - freezes the surface of its pools, when it is tagged `frozen`.
//!
//! Wherever daylight falls straight down a shaft to a cave floor (a sinkhole, a ravine, an
//! entrance), the floor grows a small garden of turf, grass, ferns and flowers, or snow
//! where it is cold. Buried in the rock lie two kinds of finds, seeded per region cell:
//! geodes (hollows lined with crystal in calcite in basalt, carved by
//! `crate::caves` so the water tables can flood them) and fossil skeletons (a spine, ribs
//! and a skull of pale stone), seen only where a cave cuts through them or by digging.
//!
//! Decorations grow straight up or down within their block column and only into open air,
//! so they never cross a column border, never displace water or lava and never leave a
//! fluid beside air.

use std::sync::LazyLock;

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry, Support};
use voxmar_engine::coords::CHUNK_SIZE;
use voxmar_engine::glam::{IVec3, Vec3};

use crate::biome::{BiomeId, Biomes, Point};
use crate::caves::{Cave, GEODE_SHELL, Geode, LAVA_LEVEL};
use crate::content::{Layer, Material, Side, WorldContent};
use crate::geology::{GeoGrid, noise};
use crate::rng::{Rng, hash3};
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

/// Blocks the underground uses whatever the biome.
struct Ids {
    ice: BlockId,
    snow: BlockId,
    turf: BlockId,
    soil: BlockId,
    /// Plants of a sunlit floor, by weight out of 1 (the rest stays bare).
    garden: Vec<(BlockId, f32)>,
    crystal: BlockId,
    /// Crystals growing on a geode's floor, which light it.
    cluster: BlockId,
    calcite: BlockId,
    dark_rock: BlockId,
    bone: BlockId,
}

/// A fossil skeleton: line segments of bone and the ball of its skull.
pub(crate) struct Fossil {
    bones: Vec<(Vec3, Vec3)>,
    skull: (Vec3, f32),
    /// Its bounds, inclusive.
    pub min: IVec3,
    pub max: IVec3,
}

impl Fossil {
    pub fn contains(&self, p: IVec3) -> bool {
        let c = p.as_vec3() + 0.5;
        if c.distance_squared(self.skull.0) < self.skull.1 * self.skull.1 {
            return true;
        }
        self.bones.iter().any(|&(a, b)| {
            let ab = b - a;
            let t = ((c - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            c.distance_squared(a + ab * t) < 0.36
        })
    }
}

/// Fossils are seeded per cell of this size.
const FOSSIL_CELL: i32 = 160;
const FOSSILS: u64 = 0xf055;

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
    ids: Ids,
    /// Each block's support and whether it is solid, by block id, so decorations only hang
    /// under (and stand on) something that holds them.
    supports: Vec<(Support, bool)>,
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
            ids: {
                let id = |name: &str| registry.expect(&format!("base:{name}"));
                Ids {
                    ice: registry.expect(&content.climate.frost.ice),
                    snow: registry.expect(&content.climate.frost.snow),
                    turf: id("grass_block"),
                    soil: id("dirt"),
                    garden: vec![
                        (id("short_grass"), 0.45),
                        (id("fern"), 0.12),
                        (id("dandelion"), 0.03),
                        (id("poppy"), 0.03),
                        (id("azure_bluet"), 0.04),
                        (id("lily_of_the_valley"), 0.02),
                    ],
                    crystal: id("crystal_block"),
                    cluster: id("crystal_cluster"),
                    calcite: id("calcite"),
                    dark_rock: id("basalt"),
                    // A stand-in for bone.
                    bone: id("chalk"),
                }
            },
            supports: (0..registry.len()).map(|i| (registry.info(BlockId(i as u16)).support, registry.info(BlockId(i as u16)).solid)).collect(),
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
        let geodes = world.caves.geodes_near(geology, origin);
        let fossils = self.fossils_near(world, origin);
        UndergroundColumn { underground: self, world, origin, layers, picks, rocky, geodes, fossils }
    }

    /// Fossil skeletons reaching into the column at `origin`.
    pub(crate) fn fossils_near(&self, world: &Overworld, origin: IVec3) -> Vec<Fossil> {
        const REACH: i32 = 16;
        let cell = |v: i32| v.div_euclid(FOSSIL_CELL);
        let mut found = Vec::new();
        for fz in cell(origin.z - REACH)..=cell(origin.z + CHUNK_SIZE + REACH) {
            for fx in cell(origin.x - REACH)..=cell(origin.x + CHUNK_SIZE + REACH) {
                let mut rng = Rng::new(self.seed, FOSSILS, fx, fz);
                if !rng.chance(0.3) {
                    continue;
                }
                let span = (FOSSIL_CELL - 2 * REACH) as u32;
                let (x, z) = (fx * FOSSIL_CELL + REACH + rng.below(span), fz * FOSSIL_CELL + REACH + rng.below(span));
                let y = rng.range(LAVA_LEVEL + 14, SEA_LEVEL - 16);
                let length = rng.range(10, 22) as f32;
                let along = if rng.chance(0.5) { Vec3::X } else { Vec3::Z };
                let side = Vec3::Y.cross(along);
                let rib = 2.5 + 1.5 * rng.unit();
                let centre = Vec3::new(x as f32, y as f32, z as f32) + 0.5;
                let (tail, head) = (centre - along * length / 2.0, centre + along * length / 2.0);
                let mut bones = vec![(tail, head)];
                // Ribs every two blocks along the front two thirds, curving down.
                let mut t = length * 0.3;
                while t < length - 2.0 {
                    let at = tail + along * t;
                    let reach = rib * (1.0 - 0.5 * ((t / length - 0.65) * 2.4).powi(2)).max(0.4);
                    for s in [side, -side] {
                        let elbow = at + s * reach * 0.8 - Vec3::Y * 0.4;
                        bones.push((at, elbow));
                        bones.push((elbow, at + s * reach - Vec3::Y * (reach + 0.6)));
                    }
                    t += 2.0;
                }
                let skull = (head + along * 1.6, 1.7);
                let extent = Vec3::splat(rib + 3.0) + along * length / 2.0;
                let (min, max) = ((centre - extent).floor().as_ivec3(), (centre + extent).ceil().as_ivec3());
                let touches = min.x < origin.x + CHUNK_SIZE && max.x >= origin.x && min.z < origin.z + CHUNK_SIZE && max.z >= origin.z;
                // Buried well inside the ground.
                if touches && world.height(x, z) - y >= 16 {
                    found.push(Fossil { bones, skull, min, max });
                }
            }
        }
        found
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
    geodes: Vec<Geode>,
    fossils: Vec<Fossil>,
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

    /// Whether any cave biome in the column has rock of its own.
    pub(crate) fn any_rocky(&self) -> bool {
        self.rocky.iter().any(|&r| r)
    }

    /// Whether any cave biome near the block column at (x, z) has rock of its own.
    pub(crate) fn rocky(&self, x: i32, z: i32) -> bool {
        let (lx, lz) = ((x / STEP) as usize, (z / STEP) as usize);
        self.rocky[lz * SIDE + lx] || self.rocky[lz * SIDE + lx + 1] || self.rocky[(lz + 1) * SIDE + lx] || self.rocky[(lz + 1) * SIDE + lx + 1]
    }

    /// The rock of the cave biome at (x, y, z), if it has its own.
    pub(crate) fn rock(&self, x: i32, y: i32, z: i32) -> Option<Rock<'_>> {
        self.rock_of(self.biome(x, y, z))
    }

    /// The rock of a cave biome, if it has its own.
    pub(crate) fn rock_of(&self, biome: Option<BiomeId>) -> Option<Rock<'_>> {
        let biome = self.world.biomes.get(biome?);
        if !biome.strata.is_empty() {
            Some(Rock::Strata(&biome.strata))
        } else {
            biome.stones.as_ref().map(Rock::Stones)
        }
    }

    /// For the block column at (x, z), by lattice layer: the cave biome of the blocks
    /// from that layer up to the next, where the eight lattice points around them agree
    /// on it (as they do away from borders), so that `biome` needs no hash to choose;
    /// `None` where they differ.
    pub(crate) fn settled(&self, x: i32, z: i32, out: &mut Vec<Option<Option<BiomeId>>>) {
        let (lx, lz) = ((x / STEP) as usize, (z / STEP) as usize);
        let column = |dx: usize, dz: usize| &self.picks[((lz + dz) * SIDE + lx + dx) * self.layers..][..self.layers];
        let around = [column(0, 0), column(1, 0), column(0, 1), column(1, 1)];
        out.clear();
        out.extend((0..self.layers).map(|layer| {
            let (pick, up) = (around[0][layer], (layer + 1).min(self.layers - 1));
            around.iter().all(|c| c[layer] == pick && c[up] == pick).then_some(pick)
        }));
    }

    /// Lines the floors and ceilings of the caves in the block column at (x, z) (relative
    /// to the column origin), whose open blocks `cave` gives, and grows their decorations.
    pub(crate) fn decorate(&self, x: i32, z: i32, site: &Site, cave: &[Cave], blocks: &mut Blocks) {
        // Most block columns have no cave at all.
        if cave.iter().all(|&c| c == Cave::Rock) {
            return;
        }
        let world = self.world;
        let open = |y: i32| y >= 0 && (y as usize) < cave.len() && cave[y as usize] != Cave::Rock;
        let (wx, wz) = (self.origin.x + x, self.origin.z + z);
        // What the column offers the rules; only the height changes from floor to floor.
        let (dither, chance) = (world.terrain.dither(wx, wz), world.terrain.chance(wx, wz));
        let mix = std::cell::OnceCell::new();
        let mix = || *mix.get_or_init(|| world.terrain.mix(wx, wz));
        let column_stones = world.stones(site);
        // Daylight falls straight down to every open block from here up.
        let height = cave.len() as i32 - 1;
        let sunlit = (0..=height).rev().take_while(|&y| cave[y as usize] == Cave::Air).last().unwrap_or(i32::MAX);
        let geodes: Vec<(&Geode, std::ops::RangeInclusive<i32>)> = self.geodes.iter().filter_map(|g| Some((g, g.span(wx, wz)?))).collect();
        let in_geode = |y: i32| geodes.iter().any(|(_, span)| span.contains(&y));
        for y in 1..cave.len() as i32 - 1 {
            if cave[y as usize] == Cave::Water && cave[y as usize + 1] == Cave::Air {
                // The surface of a pool, which freezes in frozen caves.
                if self.biome(x, y, z).is_some_and(|id| self.underground.work[id.0 as usize].freezes) {
                    blocks.set(IVec3::new(wx, y, wz), self.underground.ids.ice);
                }
                continue;
            }
            if cave[y as usize] != Cave::Rock {
                continue;
            }
            let (floor, ceiling) = (open(y + 1), open(y - 1));
            if !floor && !ceiling || in_geode(y) {
                continue;
            }
            if floor && y + 1 >= sunlit && y < height {
                self.garden(IVec3::new(wx, y, wz), site, cave, blocks);
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
            let spot = Spot { site: &at, x: wx, z: wz, dither, mix: &mix, chance, sampled: Default::default() };
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

    /// Finishes the block column at (x, z): lines the geodes and lays the fossils in it.
    /// Both replace only rock the caves left whole.
    pub(crate) fn bury(&self, x: i32, z: i32, cave: &[Cave], blocks: &mut Blocks) {
        let ids = &self.underground.ids;
        let (wx, wz) = (self.origin.x + x, self.origin.z + z);
        let rock = |y: i32| y >= 0 && cave.get(y as usize) == Some(&Cave::Rock);
        for geode in &self.geodes {
            let Some(span) = geode.span(wx, wz) else { continue };
            for y in span.filter(|&y| rock(y)) {
                let pos = IVec3::new(wx, y, wz);
                let depth = geode.distance(pos) - geode.radius;
                let h = hash3(geode.seed, pos.x, pos.y, pos.z);
                let block = match depth {
                    d if d < 0.0 || d >= GEODE_SHELL => continue,
                    d if d < 1.2 => {
                        // Clusters grow on the floor.
                        if h.is_multiple_of(3) && cave.get(y as usize + 1) == Some(&Cave::Air) && geode.distance(pos + IVec3::Y) < geode.radius {
                            blocks.put_if_air(pos + IVec3::Y, ids.cluster);
                        }
                        ids.crystal
                    }
                    d if d < 2.2 => ids.calcite,
                    _ => ids.dark_rock,
                };
                blocks.set(pos, block);
            }
        }
        for fossil in &self.fossils {
            if !(fossil.min.x..=fossil.max.x).contains(&wx) || !(fossil.min.z..=fossil.max.z).contains(&wz) {
                continue;
            }
            for y in (fossil.min.y..=fossil.max.y).filter(|&y| rock(y)) {
                let pos = IVec3::new(wx, y, wz);
                if fossil.contains(pos) {
                    blocks.set(pos, ids.bone);
                }
            }
        }
    }

    /// Turns a sunlit cave floor at `pos` into a small garden: turf and plants, or snow
    /// where it is cold.
    fn garden(&self, pos: IVec3, site: &Site, cave: &[Cave], blocks: &mut Blocks) {
        let ids = &self.underground.ids;
        if site.temperature < 0.0 {
            blocks.replace_stone(pos, ids.snow);
            return;
        }
        blocks.replace_stone(pos, ids.turf);
        if cave.get(pos.y as usize - 1) == Some(&Cave::Rock) {
            blocks.replace_stone(pos - IVec3::Y, ids.soil);
        }
        if blocks.get(pos) != ids.turf {
            return;
        }
        let mut roll = (hash3(self.underground.seed ^ 0x6a4d, pos.x, pos.y, pos.z) & 0xffff) as f32 / 65536.0;
        for &(plant, share) in &ids.garden {
            if roll < share {
                blocks.put_if_air(pos + IVec3::Y, plant);
                return;
            }
            roll -= share;
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
                }
                .pick(u, stones, strata, at.y);
                // Hanging things hang under something solid or more of their column, and
                // plants stand on something solid or more of their stalk.
                let supports = &self.underground.supports;
                let holds = |p: IVec3, need: Support| {
                    let (support, solid) = supports[blocks.get(p).0 as usize];
                    solid || support == need
                };
                let held = match supports[block.0 as usize].0 {
                    Support::Above => holds(at + IVec3::Y, Support::Above),
                    Support::Below => holds(at - IVec3::Y, Support::Below),
                    Support::None => true,
                };
                if !held {
                    break;
                }
                blocks.put_if_air(at, block);
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
