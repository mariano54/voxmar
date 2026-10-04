//! Stage 4: turns sites and resources into blocks: surface materials, caves (see
//! [`crate::caves`]), water and ice, then trees and the small plants on the ground.
//!
//! Surface materials follow the site's parameters through thresholds (cold enough for
//! snow, steep enough for bare rock, dry and hot enough for sand). Every threshold is a
//! band rather than a line: within it, the two materials interleave with a probability
//! that ramps across the band, in patches and speckles, and intermediate materials sit
//! between distant ones (grass, snowy grass, snow; grass, coarse dirt, sand). So the
//! ground changes gradually, the way the climate does.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::chunk::Chunk;
use voxmar_engine::coords::{CHUNK_SIZE, CHUNK_VOLUME, COLUMN_CHUNKS, ColumnPos, WORLD_HEIGHT, local_index};
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

use crate::caves::{Cave, CaveFace};
use crate::ecology::{Ground, Plant};
use crate::features::{self, Features};
use crate::geology::{GeoGrid, noise};
use crate::lattice::{self, Lattice, Profile};
use crate::resources::Resources;
use crate::rng::hash3;
use crate::trees::Tree;
use crate::{Overworld, SEA_LEVEL, Site};

/// Stone turns to deepslate below this height, 48 blocks under sea level.
const DEEPSLATE: i32 = SEA_LEVEL - 48;

struct Ids {
    bedrock: BlockId,
    stone: BlockId,
    deepslate: BlockId,
    granite: BlockId,
    diorite: BlockId,
    andesite: BlockId,
    tuff: BlockId,
    calcite: BlockId,
    dirt: BlockId,
    coarse_dirt: BlockId,
    rooted_dirt: BlockId,
    mud: BlockId,
    moss: BlockId,
    podzol: BlockId,
    grass: BlockId,
    snowy_grass: BlockId,
    snow: BlockId,
    powder_snow: BlockId,
    ice: BlockId,
    packed_ice: BlockId,
    blue_ice: BlockId,
    sand: BlockId,
    sandstone: BlockId,
    red_sand: BlockId,
    red_sandstone: BlockId,
    gravel: BlockId,
    clay: BlockId,
    water: BlockId,
    lava: BlockId,
    /// Badlands strata, bottom to top of one repeat.
    bands: [BlockId; 16],
}

/// The surface of a column, from the top down.
pub struct Layers {
    pub top: BlockId,
    under: BlockId,
    /// Blocks of `under` beneath the top.
    depth: i32,
    /// Blocks of banded terracotta beneath those.
    strata: i32,
}

pub struct Terrain {
    ids: Ids,
    rock: FastNoiseLite,
    /// Patches that dither the borders between surface materials.
    patches: FastNoiseLite,
    /// Whether wood may replace a block: air, water and leaves, by block id.
    soft: Vec<bool>,
    seed: u64,
    resources: Resources,
    features: Features,
}

impl Terrain {
    pub fn new(seed: u64, registry: &BlockRegistry) -> Self {
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        let bands = [
            "orange", "", "yellow", "orange", "white", "", "brown", "orange", "red", "light_gray", "", "orange", "yellow", "red", "white",
            "brown",
        ]
        .map(|colour| if colour.is_empty() { id("terracotta") } else { id(&format!("{colour}_terracotta")) });
        let water = id("water");
        let soft = (0..registry.len())
            .map(|i| {
                let block = BlockId(i as u16);
                block.is_air() || block == water || registry.def(block).name.ends_with("_leaves")
            })
            .collect();
        Self {
            ids: Ids {
                bedrock: id("bedrock"),
                stone: id("stone"),
                deepslate: id("deepslate"),
                granite: id("granite"),
                diorite: id("diorite"),
                andesite: id("andesite"),
                tuff: id("tuff"),
                calcite: id("calcite"),
                dirt: id("dirt"),
                coarse_dirt: id("coarse_dirt"),
                rooted_dirt: id("rooted_dirt"),
                mud: id("mud"),
                moss: id("moss_block"),
                podzol: id("podzol"),
                grass: id("grass_block"),
                snowy_grass: id("snowy_grass_block"),
                snow: id("snow_block"),
                powder_snow: id("powder_snow"),
                ice: id("ice"),
                packed_ice: id("packed_ice"),
                blue_ice: id("blue_ice"),
                sand: id("sand"),
                sandstone: id("sandstone"),
                red_sand: id("red_sand"),
                red_sandstone: id("red_sandstone"),
                gravel: id("gravel"),
                clay: id("clay"),
                water,
                lava: id("lava"),
                bands,
            },
            rock: noise(seed, 23, 0.02, FractalType::None, 1),
            patches: noise(seed, 24, 0.04, FractalType::FBm, 2),
            soft,
            seed,
            resources: Resources::new(registry),
            features: Features::new(registry),
        }
    }
}

/// Dense, mutable blocks of the column being generated, addressed in world coordinates.
pub struct Blocks<'a> {
    origin: IVec3,
    data: Vec<BlockId>,
    stone_like: [BlockId; 6],
    soft: &'a [bool],
}

impl Blocks<'_> {
    fn index(&self, world: IVec3) -> Option<usize> {
        let p = world - self.origin;
        let inside = (0..CHUNK_SIZE).contains(&p.x) && (0..CHUNK_SIZE).contains(&p.z) && (0..WORLD_HEIGHT).contains(&p.y);
        inside.then(|| Self::local(p.x, p.y, p.z))
    }

    fn local(x: i32, y: i32, z: i32) -> usize {
        (y / CHUNK_SIZE) as usize * CHUNK_VOLUME + local_index(IVec3::new(x, y % CHUNK_SIZE, z))
    }

    fn get_local(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.data[Self::local(x, y, z)]
    }

    fn set_local(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        self.data[Self::local(x, y, z)] = block;
    }

    /// The part of the box from `min` to `max` (inclusive) inside the column, if any.
    pub fn clip(&self, min: IVec3, max: IVec3) -> Option<(IVec3, IVec3)> {
        let low = min.max(self.origin);
        let high = max.min(self.origin + IVec3::new(CHUNK_SIZE - 1, WORLD_HEIGHT - 1, CHUNK_SIZE - 1));
        low.cmple(high).all().then_some((low, high))
    }

    pub fn put_if_air(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos)
            && self.data[i].is_air()
        {
            self.data[i] = block;
        }
    }

    /// Wood grows through air, water and leaves.
    pub fn put_log(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos)
            && self.soft[self.data[i].0 as usize]
        {
            self.data[i] = block;
        }
    }

    pub fn replace_stone(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos)
            && self.stone_like.contains(&self.data[i])
        {
            self.data[i] = block;
        }
    }
}

pub fn generate(world: &Overworld, pos: ColumnPos) -> Column {
    let terrain = &world.terrain;
    let ids = &terrain.ids;
    let origin = pos.origin();
    let corner = IVec2::new(origin.x, origin.z);
    // Trees reach in from candidates around the column, so sample the climate there too.
    let area = world.sampler(corner - features::REACH, corner + CHUNK_SIZE - 1 + features::REACH);

    // Geology for the column plus a one-block border (for slopes).
    let geology = GeoGrid::new(&world.geology, (origin.x - 1, origin.z - 1), CHUNK_SIZE + 2);
    let top = geology.samples().iter().map(|g| g.height).max().unwrap_or(0).max(SEA_LEVEL);

    let rock = Lattice::noise(&terrain.rock, origin, 0, top, lattice::STEP, 1.0);
    let mut rock_profile = Profile::default();
    let mut caves = world.caves.column(&geology, origin, top);
    let mut cave = Vec::with_capacity(WORLD_HEIGHT as usize);

    let mut blocks = Blocks {
        origin,
        data: vec![BlockId::AIR; CHUNK_VOLUME * COLUMN_CHUNKS as usize],
        stone_like: [ids.stone, ids.deepslate, ids.granite, ids.diorite, ids.andesite, ids.tuff],
        soft: &terrain.soft,
    };
    let mut column = Column::default();
    let mut surfaces = Vec::with_capacity((CHUNK_SIZE * CHUNK_SIZE) as usize);

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (wx, wz) = (origin.x + x, origin.z + z);
            let geo = geology.get(wx, wz);
            let height = geo.height;
            let site = area.site_on(&geo, wx, wz, geology.slope(wx, wz));
            column.climate[(z * CHUNK_SIZE + x) as usize] = site.tint();
            let layers = terrain.surface_layers(&site, wx, wz);
            caves.carve(x, z, &mut cave);
            rock.profile(x, z, &mut rock_profile);

            for y in 0..=height {
                let below_surface = height - y;
                match cave[y as usize] {
                    Cave::Rock => {}
                    Cave::Air => continue,
                    Cave::Water => {
                        blocks.set_local(x, y, z, ids.water);
                        continue;
                    }
                    Cave::Lava => {
                        blocks.set_local(x, y, z, ids.lava);
                        continue;
                    }
                }
                let open = |y: i32| cave.get(y as usize).is_some_and(|&c| c != Cave::Rock);
                let lining = if open(y + 1) {
                    world.caves.decoration(IVec3::new(wx, y, wz), below_surface, CaveFace::Floor)
                } else if open(y - 1) {
                    world.caves.decoration(IVec3::new(wx, y, wz), below_surface, CaveFace::Ceiling)
                } else {
                    None
                };
                let block = if let Some(lining) = lining {
                    lining
                } else if y == 0 || (y < 4 && hash3(world.seed, wx, y, wz) % 4 >= y as u64) {
                    ids.bedrock
                } else if below_surface == 0 {
                    layers.top
                } else if below_surface <= layers.depth {
                    layers.under
                } else if below_surface <= layers.depth + layers.strata {
                    terrain.band(y, wx, wz)
                } else {
                    stone_at(ids, rock_profile.at(y), world.seed, y, wx, wz)
                };
                blocks.set_local(x, y, z, block);
            }
            let ice = terrain.ice(&site, wx, wz);
            for y in height + 1..=SEA_LEVEL {
                blocks.set_local(x, y, z, if y == SEA_LEVEL { ice.unwrap_or(ids.water) } else { ids.water });
            }
            surfaces.push((site, layers.top));
        }
    }

    terrain.resources.place(world.seed, pos, &mut blocks);
    terrain.features.place(world, &area, &geology, &caves, pos, &mut blocks);

    // Ground plants last, wherever trees, caves and water left the surface open.
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (site, surface) = &surfaces[(z * CHUNK_SIZE + x) as usize];
            let h = site.height;
            let open = |y: i32| y < WORLD_HEIGHT && blocks.get_local(x, y, z).is_air();
            if h < SEA_LEVEL || blocks.get_local(x, h, z) != *surface || !open(h + 1) {
                continue;
            }
            match world.ecology.plant(site, terrain.ground(*surface), origin.x + x, origin.z + z) {
                Some(Plant::One(plant)) => blocks.set_local(x, h + 1, z, plant),
                Some(Plant::Tall(bottom, top)) if open(h + 2) => {
                    blocks.set_local(x, h + 1, z, bottom);
                    blocks.set_local(x, h + 2, z, top);
                }
                Some(Plant::Stack(plant, n)) => {
                    let height = (h + 1..=h + n).take_while(|&y| open(y)).count() as i32;
                    for y in h + 1..=h + height {
                        blocks.set_local(x, y, z, plant);
                    }
                }
                _ => {}
            }
        }
    }

    for (cy, chunk) in column.chunks.iter_mut().enumerate() {
        *chunk = Chunk::from_dense(&blocks.data[cy * CHUNK_VOLUME..(cy + 1) * CHUNK_VOLUME]);
    }
    column
}

impl Terrain {
    /// A uniform-ish 0..1 value for dithering thresholds: smooth patches plus per-block
    /// speckle, so materials interleave in clumps and scattered blocks across a band.
    fn dither(&self, x: i32, z: i32) -> f32 {
        let speckle = (hash3(self.seed ^ 0x5fac, x, 0, z) & 0xffff) as f32 / 65536.0;
        let patch = self.patches.get_noise_2d(x as f32, z as f32) * 0.6 + 0.5;
        (0.55 * patch + 0.45 * speckle).clamp(0.0, 1.0)
    }

    /// Badlands strata: bands of coloured terracotta that wobble gently.
    fn band(&self, y: i32, x: i32, z: i32) -> BlockId {
        let wobble = (self.patches.get_noise_2d(x as f32 * 0.25, z as f32 * 0.25) * 2.5).round() as i32;
        self.ids.bands[(y + wobble).rem_euclid(self.ids.bands.len() as i32) as usize]
    }

    /// The frozen surface of sea, lake or river water at this site, if it freezes: ice
    /// that breaks up into floes at its edge, thick packed ice where it is very cold.
    fn ice(&self, site: &Site, x: i32, z: i32) -> Option<BlockId> {
        let t = site.temperature + 1.5 * self.patches.get_noise_2d(x as f32, z as f32);
        let d = self.dither(x, z);
        match t {
            t if t >= -1.0 => None,
            t if -t > 7.0 + 4.0 * d => Some(if d > 0.97 { self.ids.blue_ice } else { self.ids.packed_ice }),
            _ => Some(self.ids.ice),
        }
    }

    /// The surface at a site, from the top down.
    pub fn surface_layers(&self, site: &Site, x: i32, z: i32) -> Layers {
        let ids = &self.ids;
        let d = self.dither(x, z);
        let patch = self.patches.get_noise_2d(x as f32, z as f32);
        // Whether `v` lies past a threshold that is dithered across `lo..hi`.
        let past = |v: f32, lo: f32, hi: f32| v > lo + (hi - lo) * d;
        let layers = |top, under, depth| Layers { top, under, depth, strata: 0 };
        let (t, w) = (site.temperature, site.soil_moisture());
        let above = (site.height - SEA_LEVEL) as f32;

        if above < 0.0 {
            // Muddy warm rivers, sandy beds and shallows, clay patches, gravel in deep or
            // cold water.
            let block = if site.river > 0.3 && t > 18.0 && w > 1.0 && d > 0.45 {
                ids.mud
            } else if patch > 0.5 && above > -10.0 {
                ids.clay
            } else if site.river > 0.5 || (above >= -8.0 && past(t, 0.0, 4.0)) {
                ids.sand
            } else {
                ids.gravel
            };
            return layers(block, block, 3);
        }

        // Sand deserts are hot; cooler dry land is steppe with coarse, stony soil.
        let hot = past(t, 14.0, 18.0);
        let warm = past(t, 8.0, 12.0);
        let arid = !past(w, 0.2, 0.34);
        let dry = !past(w, 0.3, 0.44);
        // Red rock weathers out of hot, dry uplands.
        let red = hot && dry && past((above - 12.0) / 16.0 + (t - 21.0) / 5.0, 0.0, 1.0);

        // Bare rock on cliffs, and on high ground exposed to the weather where no snow
        // lies; banded terracotta in badlands.
        let rock = |above: f32| {
            if red {
                return Layers { top: self.band(site.height, x, z), under: ids.stone, depth: 0, strata: 14 };
            }
            let top = match patch {
                p if above > 90.0 && p > 0.45 => ids.calcite,
                p if above > 70.0 && p < -0.45 => ids.tuff,
                p if (-0.08..0.04).contains(&p) => ids.andesite,
                _ => ids.stone,
            };
            layers(top, ids.stone, 0)
        };
        if past(site.slope as f32, 3.0, 5.5) {
            return rock(above);
        }
        // Cold: grass, then grass under snow, then deep snow (powdery where it is coldest).
        if past(-t, 1.5, 6.0) {
            let top = if past(-t, 8.0, 12.0) && patch > 0.15 { ids.powder_snow } else { ids.snow };
            return layers(top, ids.snow, 2);
        }
        if past(-t, -2.5, 1.5) {
            return layers(ids.snowy_grass, ids.dirt, 3);
        }
        let exposure = site.slope as f32 + ((above - 95.0) / 12.0).max(0.0);
        if past(exposure, 3.0, 5.5) {
            return rock(above);
        }
        // Scree below cold peaks.
        if above > 70.0 && t < 8.0 && past(exposure, 1.8, 3.2) {
            return layers(ids.gravel, ids.stone, 2);
        }
        // Beaches along the sea, sandy (or in warm, wet places muddy) banks along rivers.
        let coast = site.continentality < 0.45 || site.river > 0.3;
        if coast && !past(above, 0.5, 2.5) {
            let top = match () {
                _ if t < 3.0 => ids.gravel,
                _ if t > 18.0 && w > 1.2 && site.river > 0.3 && d > 0.5 => ids.mud,
                _ => ids.sand,
            };
            return layers(top, top, 3);
        }
        // Dry land: grass gives way to coarse dirt, then to sand or red sand.
        if hot && arid {
            return if red {
                Layers { top: ids.red_sand, under: ids.red_sandstone, depth: 2, strata: 12 }
            } else {
                layers(ids.sand, ids.sandstone, 6)
            };
        }
        if warm && (dry || arid) {
            return layers(ids.coarse_dirt, ids.dirt, 3);
        }
        // Damp ground: moss on wet, warm forest floors, mud in hot swampy lowlands, needle
        // litter under cool, dense forest.
        if w > 1.25 && t > 15.0 && past(site.forest, 0.35, 0.7) && patch > 0.0 {
            return layers(ids.moss, ids.rooted_dirt, 3);
        }
        if w > 1.35 && t > 14.0 && !past(above, 0.0, 3.0) {
            return layers(ids.mud, ids.mud, 2);
        }
        if t < 10.0 && past(site.forest, 0.45, 0.8) {
            return layers(ids.podzol, ids.dirt, 3);
        }
        // Roots run deep under hot, wet forests.
        let under = if t > 18.0 && w > 1.2 { ids.rooted_dirt } else { ids.dirt };
        layers(ids.grass, under, 3)
    }

    /// Top block of dry or submerged ground, and the block (water or ice) above it when
    /// it lies below sea level.
    pub fn surface_blocks(&self, site: &Site, x: i32, z: i32) -> (BlockId, Option<BlockId>) {
        let top = self.surface_layers(site, x, z).top;
        let water = (site.height < SEA_LEVEL).then(|| self.ice(site, x, z).unwrap_or(self.ids.water));
        (top, water)
    }

    pub fn leaves(&self, tree: Tree) -> BlockId {
        self.features.trees().leaves(tree)
    }

    pub fn is_grass(&self, block: BlockId) -> bool {
        block == self.ids.grass
    }

    /// What a surface block offers plants.
    pub fn ground(&self, block: BlockId) -> Ground {
        let ids = &self.ids;
        if [ids.grass, ids.dirt, ids.coarse_dirt, ids.rooted_dirt, ids.podzol, ids.moss, ids.mud].contains(&block) {
            Ground::Soil
        } else if [ids.snowy_grass, ids.snow, ids.powder_snow].contains(&block) {
            Ground::Snow
        } else if block == ids.sand || block == ids.red_sand {
            Ground::Sand
        } else {
            Ground::Bare
        }
    }
}

fn stone_at(ids: &Ids, rock: f32, seed: u64, y: i32, wx: i32, wz: i32) -> BlockId {
    // Deepslate below `DEEPSLATE`, dithered over 8 blocks.
    if y < DEEPSLATE + (hash3(seed ^ 0xd5, wx, y, wz) % 8) as i32 {
        return ids.deepslate;
    }
    match rock {
        n if n > 0.6 => ids.granite,
        n if n < -0.6 => ids.diorite,
        n if (0.28..0.34).contains(&n) => ids.andesite,
        n if y < DEEPSLATE + 24 && (-0.36..-0.28).contains(&n) => ids.tuff,
        _ => ids.stone,
    }
}
