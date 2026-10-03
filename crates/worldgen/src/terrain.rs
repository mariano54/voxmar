//! Stage 4: turns heights, biomes and resources into blocks, and carves caves.

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::chunk::Chunk;
use voxmar_engine::coords::{CHUNK_SIZE, CHUNK_VOLUME, COLUMN_CHUNKS, ColumnPos, WORLD_HEIGHT, local_index};
use voxmar_engine::glam::IVec3;
use voxmar_engine::world::{COLUMN_AREA, Column};

use crate::climate::Biome;
use crate::features::Features;
use crate::geology::noise;
use crate::resources::Resources;
use crate::rng::hash3;
use crate::{Overworld, SEA_LEVEL};

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
    leaves: [BlockId; 5],
}

pub struct Terrain {
    ids: Ids,
    cheese: FastNoiseLite,
    tunnel_a: FastNoiseLite,
    tunnel_b: FastNoiseLite,
    rock: FastNoiseLite,
    floor: FastNoiseLite,
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
                leaves: ["oak", "birch", "spruce", "jungle", "acacia"].map(|w| id(&format!("{w}_leaves"))),
            },
            cheese: noise(seed, 20, 0.012, FractalType::FBm, 2),
            tunnel_a: noise(seed, 21, 0.014, FractalType::FBm, 2),
            tunnel_b: noise(seed, 22, 0.014, FractalType::FBm, 2),
            rock: noise(seed, 23, 0.02, FractalType::None, 1),
            floor: noise(seed, 24, 0.04, FractalType::None, 1),
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

    fn set_local(&mut self, x: i32, y: i32, z: i32, block: BlockId) {
        self.data[(y / CHUNK_SIZE) as usize * CHUNK_VOLUME + local_index(IVec3::new(x, y % CHUNK_SIZE, z))] = block;
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

    // Geology and climate for the column plus a one-block border (for slopes).
    const PW: i32 = CHUNK_SIZE + 2;
    let mut heights = vec![0i32; (PW * PW) as usize];
    let mut climates = Vec::with_capacity(COLUMN_AREA);
    for z in -1..=CHUNK_SIZE {
        for x in -1..=CHUNK_SIZE {
            let geo = world.geology.sample(origin.x + x, origin.z + z);
            heights[((z + 1) * PW + x + 1) as usize] = geo.height;
            if (0..CHUNK_SIZE).contains(&x) && (0..CHUNK_SIZE).contains(&z) {
                climates.push(world.climate.sample(origin.x + x, origin.z + z, &geo));
            }
        }
    }
    let height_at = |x: i32, z: i32| heights[((z + 1) * PW + x + 1) as usize];
    let top = heights.iter().copied().max().unwrap_or(0).max(SEA_LEVEL);

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

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let climate = &climates[(z * CHUNK_SIZE + x) as usize];
            column.climate[(z * CHUNK_SIZE + x) as usize] =
                [(climate.temperature * 255.0) as u8, (climate.precipitation * 255.0) as u8];
            let height = height_at(x, z);
            let slope = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .map(|(dx, dz)| (height_at(x + dx, z + dz) - height).abs())
                .max()
                .unwrap_or(0);
            let (wx, wz) = (origin.x + x, origin.z + z);
            let (surface, subsurface, depth) = surface_layers(ids, climate.biome, height, slope, || {
                terrain.floor.get_noise_2d(wx as f32, wz as f32)
            });
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

                let carvable = y > 4 && block != ids.bedrock && (!underwater || below_surface > 6);
                if carvable && is_cave(&cheese, &tunnel_a, &tunnel_b, x, y, z, below_surface) {
                    continue;
                }
                blocks.set_local(x, y, z, block);
            }
            for y in height + 1..=SEA_LEVEL {
                let frozen = y == SEA_LEVEL && climate.temperature < 0.18;
                blocks.set_local(x, y, z, if frozen { ids.ice } else { ids.water });
            }
        }
    }

    terrain.resources.place(world.seed, pos, &mut blocks);
    terrain.features.place(world, world.seed, pos, &mut blocks);

    for (cy, chunk) in column.chunks.iter_mut().enumerate() {
        *chunk = Chunk::from_dense(&blocks.data[cy * CHUNK_VOLUME..(cy + 1) * CHUNK_VOLUME]);
    }
    column
}

impl Terrain {
    /// Top block of dry or submerged ground, and the block (water or ice) above it when
    /// it lies below sea level.
    pub fn surface_blocks(&self, x: i32, z: i32, height: i32, biome: Biome, temperature: f32, slope: i32) -> (BlockId, Option<BlockId>) {
        let (top, _, _) = surface_layers(&self.ids, biome, height, slope, || self.floor.get_noise_2d(x as f32, z as f32));
        let water = (height < SEA_LEVEL).then_some(if temperature < 0.18 { self.ids.ice } else { self.ids.water });
        (top, water)
    }

    pub fn canopy(&self, biome: Biome) -> Option<(BlockId, f32)> {
        self.features.canopy(biome)
    }
}

/// Top block, the block beneath it and how deep that second layer goes.
fn surface_layers(ids: &Ids, biome: Biome, height: i32, slope: i32, floor: impl Fn() -> f32) -> (BlockId, BlockId, i32) {
    if height < SEA_LEVEL {
        let n = floor();
        let block = if n > 0.55 {
            ids.clay
        } else if height < SEA_LEVEL - 12 || n < -0.5 {
            ids.gravel
        } else {
            ids.sand
        };
        return (block, block, 3);
    }
    let steep = slope >= 3;
    match biome {
        Biome::Beach | Biome::Ocean | Biome::FrozenOcean => (ids.sand, ids.sand, 4),
        Biome::Desert => (ids.sand, ids.sandstone, 6),
        Biome::Badlands => (ids.red_sand, ids.terracotta, 14),
        Biome::SnowyPeaks if steep => (ids.stone, ids.stone, 0),
        Biome::SnowyPeaks => (ids.snow, ids.snow, 2),
        Biome::Mountains if steep || height > SEA_LEVEL + 100 => (ids.stone, ids.stone, 0),
        Biome::SnowyTundra => (ids.snowy_grass, ids.dirt, 3),
        _ if steep && slope >= 5 => (ids.stone, ids.stone, 0),
        _ => (ids.grass, ids.dirt, 3),
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
