//! Stage 4: turns sites and resources into blocks: surface materials, caves, water, then
//! trees and the small plants on the ground.
//!
//! Surface materials follow the site's parameters through thresholds (cold enough for
//! snow, steep enough for bare rock, dry and hot enough for sand), each dithered by the
//! same patchy noise, so materials meet along ragged, natural borders.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::chunk::Chunk;
use voxmar_engine::coords::{CHUNK_SIZE, CHUNK_VOLUME, COLUMN_CHUNKS, ColumnPos, WORLD_HEIGHT, local_index};
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

use crate::ecology::{Ground, Tree};
use crate::features::{self, Features};
use crate::geology::noise;
use crate::resources::Resources;
use crate::rng::hash3;
use crate::{Overworld, SEA_LEVEL, Site};

/// Spacing of the lattice on which 3D noise is evaluated and then interpolated.
const LATTICE: i32 = 4;
const LX: usize = (CHUNK_SIZE / LATTICE + 1) as usize;

struct Ids {
    bedrock: BlockId,
    stone: BlockId,
    deepslate: BlockId,
    granite: BlockId,
    diorite: BlockId,
    andesite: BlockId,
    dirt: BlockId,
    coarse_dirt: BlockId,
    podzol: BlockId,
    grass: BlockId,
    snowy_grass: BlockId,
    snow: BlockId,
    ice: BlockId,
    sand: BlockId,
    sandstone: BlockId,
    red_sand: BlockId,
    terracotta: BlockId,
    gravel: BlockId,
    clay: BlockId,
    water: BlockId,
    /// Per `Tree`.
    leaves: [BlockId; 5],
}

pub struct Terrain {
    ids: Ids,
    cheese: FastNoiseLite,
    tunnel_a: FastNoiseLite,
    tunnel_b: FastNoiseLite,
    rock: FastNoiseLite,
    /// Dithers the borders between surface materials.
    patches: FastNoiseLite,
    seed: u64,
    resources: Resources,
    features: Features,
}

impl Terrain {
    pub fn new(seed: u64, registry: &BlockRegistry) -> Self {
        let id = |name: &str| registry.expect(&format!("base:{name}"));
        Self {
            ids: Ids {
                bedrock: id("bedrock"),
                stone: id("stone"),
                deepslate: id("deepslate"),
                granite: id("granite"),
                diorite: id("diorite"),
                andesite: id("andesite"),
                dirt: id("dirt"),
                coarse_dirt: id("coarse_dirt"),
                podzol: id("podzol"),
                grass: id("grass_block"),
                snowy_grass: id("snowy_grass_block"),
                snow: id("snow_block"),
                ice: id("ice"),
                sand: id("sand"),
                sandstone: id("sandstone"),
                red_sand: id("red_sand"),
                terracotta: id("terracotta"),
                gravel: id("gravel"),
                clay: id("clay"),
                water: id("water"),
                leaves: Tree::ALL.map(|tree| id(&format!("{}_leaves", tree.name()))),
            },
            cheese: noise(seed, 20, 0.012, FractalType::FBm, 2),
            tunnel_a: noise(seed, 21, 0.014, FractalType::FBm, 2),
            tunnel_b: noise(seed, 22, 0.014, FractalType::FBm, 2),
            rock: noise(seed, 23, 0.02, FractalType::None, 1),
            patches: noise(seed, 24, 0.04, FractalType::FBm, 2),
            seed,
            resources: Resources::new(registry),
            features: Features::new(registry),
        }
    }
}

/// Dense, mutable blocks of the column being generated, addressed in world coordinates.
pub struct Blocks {
    origin: IVec3,
    data: Vec<BlockId>,
    stone_like: [BlockId; 5],
    leaves: [BlockId; 5],
}

impl Blocks {
    fn index(&self, world: IVec3) -> Option<usize> {
        let p = world - self.origin;
        let inside = (0..CHUNK_SIZE).contains(&p.x) && (0..CHUNK_SIZE).contains(&p.z) && (0..WORLD_HEIGHT).contains(&p.y);
        inside.then(|| (p.y / CHUNK_SIZE) as usize * CHUNK_VOLUME + local_index(IVec3::new(p.x, p.y % CHUNK_SIZE, p.z)))
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

    pub fn put_if_air(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos)
            && self.data[i].is_air()
        {
            self.data[i] = block;
        }
    }

    pub fn put_log(&mut self, pos: IVec3, block: BlockId) {
        if let Some(i) = self.index(pos)
            && (self.data[i].is_air() || self.leaves.contains(&self.data[i]))
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

/// Trilinearly interpolated 3D noise over the column, sampled every `LATTICE` blocks.
struct Lattice {
    values: Vec<f32>,
    layers: usize,
}

impl Lattice {
    fn new(noise: &FastNoiseLite, origin: IVec3, top: i32, scale_y: f32) -> Self {
        let layers = (top / LATTICE + 2) as usize;
        let mut values = Vec::with_capacity(LX * LX * layers);
        for ly in 0..layers {
            for lz in 0..LX {
                for lx in 0..LX {
                    let p = origin + IVec3::new(lx as i32, ly as i32, lz as i32) * LATTICE;
                    values.push(noise.get_noise_3d(p.x as f32, p.y as f32 * scale_y, p.z as f32));
                }
            }
        }
        Self { values, layers }
    }

    fn get(&self, x: i32, y: i32, z: i32) -> f32 {
        let (gx, gy, gz) = ((x / LATTICE) as usize, (y / LATTICE) as usize, (z / LATTICE) as usize);
        if gy + 1 >= self.layers {
            return 0.0;
        }
        let f = |v: i32| (v % LATTICE) as f32 / LATTICE as f32;
        let (fx, fy, fz) = (f(x), f(y), f(z));
        let at = |dx: usize, dy: usize, dz: usize| self.values[((gy + dy) * LX + gz + dz) * LX + gx + dx];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let x00 = lerp(at(0, 0, 0), at(1, 0, 0), fx);
        let x10 = lerp(at(0, 1, 0), at(1, 1, 0), fx);
        let x01 = lerp(at(0, 0, 1), at(1, 0, 1), fx);
        let x11 = lerp(at(0, 1, 1), at(1, 1, 1), fx);
        lerp(lerp(x00, x10, fy), lerp(x01, x11, fy), fz)
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
    const PW: i32 = CHUNK_SIZE + 2;
    let geology: Vec<_> =
        (0..PW * PW).map(|i| world.geology.sample(origin.x + i % PW - 1, origin.z + i / PW - 1)).collect();
    let geo_at = |x: i32, z: i32| &geology[((z + 1) * PW + x + 1) as usize];
    let top = geology.iter().map(|g| g.height).max().unwrap_or(0).max(SEA_LEVEL);

    let cheese = Lattice::new(&terrain.cheese, origin, top, 1.6);
    let tunnel_a = Lattice::new(&terrain.tunnel_a, origin, top, 1.0);
    let tunnel_b = Lattice::new(&terrain.tunnel_b, origin, top, 1.0);
    let rock = Lattice::new(&terrain.rock, origin, top, 1.0);

    let mut blocks = Blocks {
        origin,
        data: vec![BlockId::AIR; CHUNK_VOLUME * COLUMN_CHUNKS as usize],
        stone_like: [ids.stone, ids.deepslate, ids.granite, ids.diorite, ids.andesite],
        leaves: ids.leaves,
    };
    let mut column = Column::default();
    let mut surfaces = Vec::with_capacity((CHUNK_SIZE * CHUNK_SIZE) as usize);

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let geo = geo_at(x, z);
            let height = geo.height;
            let slope = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .map(|(dx, dz)| (geo_at(x + dx, z + dz).height - height).abs())
                .max()
                .unwrap_or(0);
            let (wx, wz) = (origin.x + x, origin.z + z);
            let site = area.site_on(geo, wx, wz, slope);
            column.climate[(z * CHUNK_SIZE + x) as usize] = site.tint();
            let (surface, subsurface, depth) = terrain.surface_layers(&site, wx, wz);
            let underwater = height < SEA_LEVEL;

            for y in 0..=height {
                let below_surface = height - y;
                let block = if y == 0 || (y < 4 && hash3(world.seed, wx, y, wz) % 4 >= y as u64) {
                    ids.bedrock
                } else if below_surface == 0 {
                    surface
                } else if below_surface <= depth {
                    subsurface
                } else {
                    stone_at(ids, &rock, world.seed, x, y, z, wx, wz)
                };

                // Caves stay dry: they never open into the floor of a sea, lake or river.
                let carvable = y > 4 && block != ids.bedrock && (!underwater || below_surface > 6);
                if carvable && is_cave(&cheese, &tunnel_a, &tunnel_b, x, y, z, below_surface) {
                    continue;
                }
                blocks.set_local(x, y, z, block);
            }
            let frozen = terrain.frozen(&site, wx, wz);
            for y in height + 1..=SEA_LEVEL {
                blocks.set_local(x, y, z, if y == SEA_LEVEL && frozen { ids.ice } else { ids.water });
            }
            surfaces.push((site, surface));
        }
    }

    terrain.resources.place(world.seed, pos, &mut blocks);
    terrain.features.place(world, &area, world.seed, pos, &mut blocks);

    // Ground plants last, wherever trees, caves and water left the surface open.
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (site, surface) = &surfaces[(z * CHUNK_SIZE + x) as usize];
            let h = site.height;
            let open = h >= SEA_LEVEL
                && h + 1 < WORLD_HEIGHT
                && blocks.get_local(x, h, z) == *surface
                && blocks.get_local(x, h + 1, z).is_air();
            if let Some(plant) = open.then(|| world.ecology.plant(site, terrain.ground(*surface), origin.x + x, origin.z + z)).flatten() {
                blocks.set_local(x, h + 1, z, plant);
            }
        }
    }

    for (cy, chunk) in column.chunks.iter_mut().enumerate() {
        *chunk = Chunk::from_dense(&blocks.data[cy * CHUNK_VOLUME..(cy + 1) * CHUNK_VOLUME]);
    }
    column
}

impl Terrain {
    /// Dithers thresholds: smooth patches plus a little per-block speckle, in −1..1.
    fn jitter(&self, x: i32, z: i32) -> f32 {
        let speckle = (hash3(self.seed ^ 0x5fac, x, 0, z) & 0xffff) as f32 / 32768.0 - 1.0;
        0.85 * self.patches.get_noise_2d(x as f32, z as f32) + 0.15 * speckle
    }

    /// Whether sea, lake or river water freezes over at this site; the edge of the ice
    /// breaks up into floes.
    fn frozen(&self, site: &Site, x: i32, z: i32) -> bool {
        site.temperature + 1.5 * self.patches.get_noise_2d(x as f32, z as f32) < -1.0
    }

    /// Top block, the block beneath it and how deep that second layer goes.
    pub fn surface_layers(&self, site: &Site, x: i32, z: i32) -> (BlockId, BlockId, i32) {
        let ids = &self.ids;
        let jitter = self.jitter(x, z);
        let h = site.height;
        let t = site.temperature + 1.5 * jitter;
        if h < SEA_LEVEL {
            // Sandy river beds and shallows, clay patches, gravel in deep or cold water.
            let block = if jitter > 0.55 && h > SEA_LEVEL - 10 {
                ids.clay
            } else if site.river > 0.5 || (h >= SEA_LEVEL - 8 && t > 2.0 && jitter > -0.6) {
                ids.sand
            } else {
                ids.gravel
            };
            return (block, block, 3);
        }
        // Bare rock on cliffs and on high, exposed ridges.
        let slope = site.slope as f32 + 1.2 * jitter;
        let alpine = h as f32 + 12.0 * jitter > (SEA_LEVEL + 110) as f32;
        if slope >= 4.5 || (alpine && slope >= 2.0) {
            return (ids.stone, ids.stone, 0);
        }
        if t < -3.0 {
            return (ids.snow, ids.snow, 2);
        }
        if t < 0.5 {
            return (ids.snowy_grass, ids.dirt, 3);
        }
        // Beaches along the sea, sandy banks along rivers and lakes.
        let shore = h as f32 + 1.5 * jitter <= (SEA_LEVEL + 1) as f32 && (site.continentality < 0.45 || site.river > 0.3);
        if shore {
            return if t < 4.0 { (ids.gravel, ids.gravel, 3) } else { (ids.sand, ids.sand, 4) };
        }
        let water = site.soil_moisture() + 0.08 * jitter;
        if water < 0.3 && t > 14.0 {
            // Hot deserts; dry uplands weather into red sand over banded terracotta.
            if t > 20.0 && h as f32 + 6.0 * jitter > (SEA_LEVEL + 16) as f32 {
                return (ids.red_sand, ids.terracotta, 12);
            }
            return (ids.sand, ids.sandstone, 6);
        }
        // Bare patches in dry grassland, needle litter under dense cool forest.
        if water < 0.5 && t > 10.0 && jitter > 0.35 {
            return (ids.coarse_dirt, ids.dirt, 3);
        }
        if site.forest > 0.55 && t < 9.0 && jitter > 0.1 {
            return (ids.podzol, ids.dirt, 3);
        }
        (ids.grass, ids.dirt, 3)
    }

    /// Top block of dry or submerged ground, and the block (water or ice) above it when
    /// it lies below sea level.
    pub fn surface_blocks(&self, site: &Site, x: i32, z: i32) -> (BlockId, Option<BlockId>) {
        let (top, _, _) = self.surface_layers(site, x, z);
        let water = (site.height < SEA_LEVEL).then_some(if self.frozen(site, x, z) { self.ids.ice } else { self.ids.water });
        (top, water)
    }

    pub fn leaves(&self, tree: Tree) -> BlockId {
        self.ids.leaves[tree as usize]
    }

    pub fn is_grass(&self, block: BlockId) -> bool {
        block == self.ids.grass
    }

    /// What a surface block offers plants.
    pub fn ground(&self, block: BlockId) -> Ground {
        let ids = &self.ids;
        if [ids.grass, ids.dirt, ids.coarse_dirt, ids.podzol, ids.snowy_grass].contains(&block) {
            Ground::Soil
        } else if block == ids.sand || block == ids.red_sand {
            Ground::Sand
        } else {
            Ground::Bare
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stone_at(ids: &Ids, rock: &Lattice, seed: u64, x: i32, y: i32, z: i32, wx: i32, wz: i32) -> BlockId {
    // Deepslate below y 16, dithered over 8 blocks.
    if y < 16 + (hash3(seed ^ 0xd5, wx, y, wz) % 8) as i32 {
        return ids.deepslate;
    }
    match rock.get(x, y, z) {
        n if n > 0.6 => ids.granite,
        n if n < -0.6 => ids.diorite,
        n if (0.28..0.34).contains(&n) => ids.andesite,
        _ => ids.stone,
    }
}

fn is_cave(cheese: &Lattice, a: &Lattice, b: &Lattice, x: i32, y: i32, z: i32, depth: i32) -> bool {
    // Large caverns well below the surface.
    if depth > 12 && cheese.get(x, y, z) > 0.62 {
        return true;
    }
    // Winding tunnels where two noise fields are both near zero; they may break the
    // surface, which forms cave entrances.
    let (a, b) = (a.get(x, y, z), b.get(x, y, z));
    a * a + b * b < 0.0045
}
