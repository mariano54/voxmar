//! Procedural world generation.
//!
//! The overworld generator runs in stages, each a pure function of the world seed and
//! position: world dials (global parameters such as mean temperature, ocean fraction and
//! wind), geology (continents, mountains, rivers and lakes), climate (temperature and
//! precipitation as continuous fields), ecology (what grows where), resources (ore veins)
//! and terrain (blocks, caves, trees, plants). There are no biomes: every surface
//! decision follows smoothly from the parameters at that spot, which `Overworld::site`
//! exposes for gameplay. Purity means any column can be generated alone, in any order,
//! on any thread, and features that cross column borders (trees, veins) come out
//! identical on both sides.

mod caves;
mod climate;
mod dials;
mod ecology;
mod features;
mod geology;
mod lattice;
mod rarity;
mod resources;
mod rng;
mod terrain;
mod trees;

use std::sync::Arc;

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

pub use caves::{CaveStyle, LAVA_LEVEL};
pub use climate::LAPSE_RATE;
pub use dials::WorldDials;
pub use rarity::{Featured, Rarity};

use crate::climate::{Climate, ClimateGrid};
use crate::ecology::Ecology;
use crate::geology::{GeoSample, Geology};

/// Something that produces world columns. Generators are identified by a stable id that
/// is mixed into their random streams.
pub trait Generator: Send + Sync {
    fn id(&self) -> &'static str;
    fn generate(&self, pos: ColumnPos) -> Column;
    /// A safe place to stand, near the world origin.
    fn spawn_point(&self) -> IVec3;
}

/// Height of the sea surface: 128 blocks of ground below it, 256 of air and mountains above.
pub const SEA_LEVEL: i32 = 128;

/// Everything the generator knows about the surface at one position. These are the
/// continuous parameters every surface decision is made from, kept physical so that
/// gameplay (crops, comfort, settlements) can reason with them too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    /// Height of the top ground block.
    pub height: i32,
    /// Steepest height step to a neighbouring position, in blocks.
    pub slope: i32,
    /// Mean annual air temperature at the surface, °C, after cooling with altitude.
    pub temperature: f32,
    /// Annual precipitation, mm.
    pub precipitation: f32,
    /// Precipitation over potential evaporation: around 0.1 in deserts, 1 where they
    /// balance, 2 in rainforests.
    pub moisture: f32,
    /// −1 over the deep ocean, 0 at the coast ..= 1 deep inland.
    pub continentalness: f32,
    /// 0 at the equator ..= 1 at a pole.
    pub latitude: f32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    /// The landform, in steps: 0 on flat land, 1/3 in hill country, 2/3 on the slopes of a
    /// range and 1 along its peaks, with short ramps between. Like Minecraft's erosion
    /// inverted: it grows with relief.
    pub erosion: f32,
    /// Evenly spread over 0 ..= 1 and varying over kilometres; rare biomes live in its
    /// extremes.
    pub weirdness: f32,
    /// How fantastical the land is, 0 ..= 1. The world's fantasy dial is its median; where
    /// it is high the strange is common.
    pub aether: f32,
    /// How rich the soil is, evenly spread over 0 ..= 1.
    pub fertility: f32,
    /// Evenly spread over 0 ..= 1, varying over a few hundred blocks: which sub-biome of a
    /// biome grows here.
    pub variant: f32,
    /// 1 in a river or lake, falling to 0 at the edge of its valley or shore.
    pub river: f32,
    /// Ground cover the climate supports, 0 (barren) ..= 1 (lush).
    pub vegetation: f32,
    /// Tree cover, 0 (open) ..= 1 (dense forest).
    pub forest: f32,
}

impl Site {
    /// Water available to plants: the climate's, or ground water near rivers and lakes.
    pub fn soil_moisture(&self) -> f32 {
        self.moisture.max(self.river * 1.3)
    }

    /// Temperature and humidity bytes as stored in `Column::climate`: coordinates in
    /// Minecraft-style grass and foliage colour maps.
    pub fn tint(&self) -> [u8; 2] {
        let temperature = ((self.temperature + 5.0) / 25.0).clamp(0.0, 1.0);
        let humidity = ((self.soil_moisture() - 0.1) / 1.1).clamp(0.0, 1.0);
        [(temperature * 255.0) as u8, (humidity * 255.0) as u8]
    }
}

/// A coarse description of the surface at one position, for distant terrain.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceSample {
    /// Height of the top ground block.
    pub height: i32,
    pub ground: BlockId,
    /// Water or ice at sea level when the ground is submerged.
    pub water: Option<BlockId>,
    /// Dominant leaves and the fraction of ground under trees.
    pub canopy: Option<(BlockId, f32)>,
    /// Temperature and humidity, as stored in `Column::climate`.
    pub climate: [u8; 2],
}

/// The default overworld.
pub struct Overworld {
    dials: WorldDials,
    geology: Geology,
    climate: Climate,
    ecology: Ecology,
    caves: caves::Caves,
    terrain: terrain::Terrain,
    seed: u64,
}

impl Overworld {
    pub const ID: &'static str = "base:overworld";
    /// Bump when generation output changes.
    pub const VERSION: u32 = 6;

    pub fn new(seed: u64, registry: Arc<BlockRegistry>) -> Self {
        let stream = rng::stream_seed(seed, Self::ID, Self::VERSION);
        let dials = WorldDials::sample(stream);
        Self {
            dials,
            geology: Geology::new(stream, &dials),
            climate: Climate::new(stream, &dials),
            ecology: Ecology::new(stream, &dials, &registry),
            caves: caves::Caves::new(stream, &dials),
            terrain: terrain::Terrain::new(stream, &registry),
            seed: stream,
        }
    }

    /// The world's global parameters.
    pub fn dials(&self) -> &WorldDials {
        &self.dials
    }

    /// How much of each kind of cave the world has.
    pub fn cave_style(&self) -> &CaveStyle {
        self.caves.style()
    }

    /// Surface height only; cheap enough to sample on coarse grids.
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.geology.sample(x, z).height
    }

    /// Steepest height step from (x, z), whose height is `height`, to its four neighbours.
    fn slope(&self, x: i32, z: i32, height: i32) -> i32 {
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().map(|(dx, dz)| (self.height(x + dx, z + dz) - height).abs()).max().unwrap_or(0)
    }

    /// Samples many positions within the blocks from `min` to `max` (inclusive) far
    /// faster than one at a time, by sharing the regional climate.
    pub fn sampler(&self, min: IVec2, max: IVec2) -> Sampler<'_> {
        Sampler { world: self, grid: ClimateGrid::new(&self.climate, &self.geology, min, max) }
    }

    /// The parameters of the surface at one position.
    pub fn site(&self, x: i32, z: i32) -> Site {
        self.sampler(IVec2::new(x, z), IVec2::new(x, z)).site(x, z)
    }

    /// What the surface looks like from far away, given its local steepness in blocks.
    /// Matches full generation except for caves and individual trees and plants.
    pub fn surface_sample(&self, x: i32, z: i32, height: i32, slope: i32) -> SurfaceSample {
        self.sampler(IVec2::new(x, z), IVec2::new(x, z)).surface(x, z, height, slope)
    }
}

/// Samples sites over an area, computing the regional climate once per lattice point.
pub struct Sampler<'a> {
    world: &'a Overworld,
    grid: ClimateGrid,
}

impl Sampler<'_> {
    /// The parameters of the surface at a position inside the sampler's area.
    pub fn site(&self, x: i32, z: i32) -> Site {
        let geo = self.world.geology.sample(x, z);
        self.site_on(&geo, x, z, self.world.slope(x, z, geo.height))
    }

    /// What the surface looks like from far away; see [`Overworld::surface_sample`].
    pub fn surface(&self, x: i32, z: i32, height: i32, slope: i32) -> SurfaceSample {
        let geo = GeoSample { height, ..self.world.geology.sample(x, z) };
        let site = self.site_on(&geo, x, z, slope);
        let (ground, water) = self.world.terrain.surface_blocks(&site, x, z);
        let canopy = if water.is_some() { None } else { self.world.ecology.canopy(&site, self.world.terrain.ground(ground), x, z) }
            .map(|(tree, cover)| (self.world.terrain.leaves(tree), cover));
        SurfaceSample { height, ground, water, canopy, climate: site.tint() }
    }

    /// The site on already sampled geology.
    fn site_on(&self, geo: &GeoSample, x: i32, z: i32, slope: i32) -> Site {
        let regional = self.grid.at(x, z);
        let temperature = regional.sea_temperature - LAPSE_RATE * (geo.height - SEA_LEVEL).max(0) as f32;
        let mut site = Site {
            height: geo.height,
            slope,
            temperature,
            precipitation: regional.precipitation,
            moisture: climate::moisture(regional.precipitation, temperature),
            continentalness: regional.continentalness,
            latitude: regional.latitude,
            mountain: geo.mountain,
            erosion: regional.erosion,
            weirdness: regional.weirdness,
            aether: regional.aether,
            fertility: regional.fertility,
            variant: regional.variant,
            river: geo.river,
            vegetation: 0.0,
            forest: 0.0,
        };
        if geo.height >= SEA_LEVEL {
            let water = site.soil_moisture();
            site.vegetation = ecology::ground_cover(temperature, water);
            site.forest = self.world.ecology.forest(x, z, temperature, water);
        }
        site
    }
}

impl Generator for Overworld {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn generate(&self, pos: ColumnPos) -> Column {
        let _span = tracing::info_span!("generate_column").entered();
        terrain::generate(self, pos)
    }

    fn spawn_point(&self) -> IVec3 {
        // Walk outwards on a spiral until we find open, temperate grassland on dry land.
        for ring in 0..256 {
            for step in 0..(ring * 8).max(1) {
                let (x, z) = spiral(ring, step);
                let (x, z) = (x * 16, z * 16);
                let height = self.height(x, z);
                if !(SEA_LEVEL + 3..=SEA_LEVEL + 30).contains(&height) {
                    continue;
                }
                let site = self.site(x, z);
                let pleasant = (8.0..20.0).contains(&site.temperature) && (0.6..1.6).contains(&site.soil_moisture());
                let open = site.slope <= 1 && site.forest < 0.35 && site.river < 0.1;
                if pleasant && open && self.terrain.is_grass(self.terrain.surface_layers(&site, x, z).top) {
                    return IVec3::new(x, height + 1, z);
                }
            }
        }
        IVec3::new(0, voxmar_engine::coords::WORLD_HEIGHT - 2, 0)
    }
}

/// The `step`-th position on the square ring at Chebyshev distance `ring`.
fn spiral(ring: i32, step: i32) -> (i32, i32) {
    if ring == 0 {
        return (0, 0);
    }
    let side = step / (ring * 2);
    let t = step % (ring * 2) - ring;
    match side {
        0 => (t, -ring),
        1 => (ring, t),
        2 => (-t, ring),
        _ => (-ring, -t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::coords::COLUMN_CHUNKS;
    use voxmar_engine::light::LitRegion;
    use voxmar_engine::mesh::mesh_chunk;

    fn registry() -> Arc<BlockRegistry> {
        let source = include_str!("../../../assets/content/base/blocks.ron");
        Arc::new(BlockRegistry::from_ron("base", source).unwrap())
    }

    #[test]
    fn generation_is_deterministic_and_meshable() {
        let registry = registry();
        let world = Overworld::new(42, registry.clone());
        let spawn = world.spawn_point();
        let center = ColumnPos::of_block(spawn);
        assert_eq!(world.generate(center), world.generate(center));

        let start = std::time::Instant::now();
        let columns: [Arc<Column>; 9] =
            std::array::from_fn(|i| Arc::new(world.generate(center.offset(i as i32 % 3 - 1, i as i32 / 3 - 1))));
        let generated = start.elapsed();
        let start = std::time::Instant::now();
        let region = LitRegion::compute(&registry, &columns);
        let lit = start.elapsed();
        let start = std::time::Instant::now();
        let quads: usize = (0..COLUMN_CHUNKS).map(|cy| mesh_chunk(&registry, &region, &columns[4], cy).opaque.vertices.len() / 4).sum();
        println!("9 columns {generated:?}, light {lit:?}, mesh {:?}, {quads} quads, spawn {spawn}", start.elapsed());
        println!("{:#?}", world.dials());
        assert!(quads > 0);
    }

    /// `cargo test --release -p voxmar_worldgen column_timing -- --ignored --nocapture`
    /// reports the mean time to generate one column on one thread, over a fixed spread of
    /// places around spawn on a few seeds, for comparing generation before and after a
    /// change.
    #[test]
    #[ignore]
    fn column_timing() {
        let registry = registry();
        let mut total = std::time::Duration::ZERO;
        let mut count = 0;
        for seed in [1, 42, 0x5eed] {
            let world = Overworld::new(seed, registry.clone());
            let spawn = ColumnPos::of_block(world.spawn_point());
            let positions: Vec<ColumnPos> = (0..100).map(|i| spawn.offset(i % 10 * 7 - 35, i / 10 * 7 - 35)).collect();
            world.generate(spawn);
            let start = std::time::Instant::now();
            for &pos in &positions {
                std::hint::black_box(world.generate(pos));
            }
            let elapsed = start.elapsed();
            println!("seed {seed}: {:.2} ms per column", elapsed.as_secs_f64() * 1000.0 / positions.len() as f64);
            total += elapsed;
            count += positions.len();
        }
        println!("mean {:.2} ms per column", total.as_secs_f64() * 1000.0 / count as f64);
    }

    /// Generates the `count`×`count` columns from `first` on all cores.
    fn generate_area(world: &Overworld, first: ColumnPos, count: i32) -> Vec<Column> {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let mut columns: Vec<Option<Column>> = (0..count * count).map(|_| None).collect();
        let chunk = columns.len().div_ceil(threads);
        std::thread::scope(|scope| {
            for (part, slots) in columns.chunks_mut(chunk).enumerate() {
                scope.spawn(move || {
                    for (i, slot) in slots.iter_mut().enumerate() {
                        let i = (part * chunk + i) as i32;
                        *slot = Some(world.generate(first.offset(i % count, i / count)));
                    }
                });
            }
        });
        columns.into_iter().map(Option::unwrap).collect()
    }

    /// Generated water and lava stay put: no fluid touches air beside or below it, water
    /// never touches lava, and lava lies only at the bottom of the world. Checked across
    /// column borders, so it also shows the columns agree with each other.
    #[test]
    fn fluids_are_contained() {
        let registry = registry();
        let (water, lava) = (registry.expect("base:water"), registry.expect("base:lava"));
        let (mut fluids, mut caves) = (0, 0);
        for (seed, place) in [(1, (0, 0)), (42, (3000, -2000)), (0x5eed, (-4100, 900)), (7, (1500, 5200))] {
            let world = Overworld::new(seed, registry.clone());
            // Some coast too, where caves meet the sea.
            let coast = (0..4000).map(|i| (place.0 + i % 64 * 48, place.1 + i / 64 * 48)).find(|&(x, z)| (world.height(x, z) - SEA_LEVEL).abs() <= 2).unwrap();
            for (x, z) in [place, (place.0 + 700, place.1 + 300), (place.0 - 900, place.1 - 600), coast] {
                let first = ColumnPos::of_block(IVec3::new(x, 0, z)).offset(-2, -2);
                let columns = generate_area(&world, first, 5);
                let origin = first.origin();
                let block = |p: IVec3| {
                    let (cx, cz) = ((p.x - origin.x) >> 5, (p.z - origin.z) >> 5);
                    (0..voxmar_engine::coords::WORLD_HEIGHT).contains(&p.y).then(|| columns[(cz * 5 + cx) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31)))
                };
                // The middle 3×3 columns, whose neighbours are all generated.
                for z in origin.z + 32..origin.z + 128 {
                    for x in origin.x + 32..origin.x + 128 {
                        for y in 0..voxmar_engine::coords::WORLD_HEIGHT {
                            let p = IVec3::new(x, y, z);
                            let here = block(p).unwrap();
                            if here.is_air() && y < world.height(x, z) {
                                caves += 1;
                            }
                            if here != water && here != lava {
                                continue;
                            }
                            fluids += 1;
                            assert!(here == water || y < LAVA_LEVEL, "seed {seed}: lava at {p}");
                            for d in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::NEG_Y] {
                                let next = block(p + d);
                                let other = if here == water { lava } else { water };
                                assert!(next.is_none_or(|b| !b.is_air() && b != other), "seed {seed}: {} at {p} beside {} at {}", registry.def(here).name, registry.def(next.unwrap()).name, p + d);
                            }
                        }
                    }
                }
            }
        }
        assert!(fluids > 0 && caves > 0);
    }

    /// `cargo test --release -p voxmar_worldgen cave_preview -- --ignored --nocapture`
    /// (optionally with `SEED=n` and a centre `X=x Z=z`, by default spawn) writes views of
    /// the caves over 1.5 km to `target/`: `caves_xray.ppm`, a top-down view where brighter
    /// means more open cave under the ground (blue for water, red for lava, white where it
    /// opens to the sky), `caves_section.ppm`, west-east and north-south cuts through the
    /// centre with the sky above the ground, and `caves_levels.ppm`, level cuts at heights
    /// `Y1` and `Y2` (by default 50 and 100).
    #[test]
    #[ignore]
    fn cave_preview() {
        const COLUMNS: i32 = 48;
        const SIZE: i32 = COLUMNS * 32;
        let registry = registry();
        let env = |name: &str| std::env::var(name).ok().and_then(|s| s.parse::<i64>().ok());
        let seed = env("SEED").unwrap_or(0x5eed) as u64;
        let world = Overworld::new(seed, registry.clone());
        let spawn = world.spawn_point();
        let centre = IVec3::new(env("X").map_or(spawn.x, |x| x as i32), 0, env("Z").map_or(spawn.z, |z| z as i32));
        let first = ColumnPos::of_block(centre).offset(-COLUMNS / 2, -COLUMNS / 2);
        let start = std::time::Instant::now();
        let columns = generate_area(&world, first, COLUMNS);
        println!("generated {} columns in {:?}", columns.len(), start.elapsed());
        let (water, lava) = (registry.expect("base:water"), registry.expect("base:lava"));
        let origin = first.origin();
        let block = |x: i32, y: i32, z: i32| {
            let (cx, cz) = ((x - origin.x) >> 5, (z - origin.z) >> 5);
            columns[(cz * COLUMNS + cx) as usize].block(IVec3::new(x & 31, y, z & 31))
        };
        let write = |name: &str, width: i32, height: i32, pixels: &[[f32; 3]]| {
            let path = format!("{}/../../target/{name}", env!("CARGO_MANIFEST_DIR"));
            let mut file = format!("P6 {width} {height} 255\n").into_bytes();
            file.extend(pixels.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8)));
            std::fs::write(&path, file).unwrap();
            println!("wrote {path}");
        };

        let mut xray = Vec::with_capacity((SIZE * SIZE) as usize);
        let (mut carved, mut ground) = (0u64, 0u64);
        for z in origin.z..origin.z + SIZE {
            for x in origin.x..origin.x + SIZE {
                let height = world.height(x, z);
                let (mut air, mut wet, mut hot) = (0, 0, 0);
                for y in 4..height.min(WORLD_HEIGHT_FOR_TESTS) {
                    match block(x, y, z) {
                        b if b.is_air() => air += 1,
                        b if b == water => wet += 1,
                        b if b == lava => hot += 1,
                        _ => {}
                    }
                }
                carved += (air + wet + hot) as u64;
                ground += (height - 4).max(0) as u64;
                let open = block(x, height, z).is_air() && height > SEA_LEVEL;
                let f = |n: i32| (n as f32 / 24.0).min(1.0);
                xray.push(if open { [1.0, 1.0, 1.0] } else { [0.12 + 0.75 * f(air) + 0.6 * f(hot), 0.12 + 0.75 * f(air) + 0.1 * f(hot), 0.14 + 0.75 * f(air) + 0.8 * f(wet)] });
            }
        }
        println!("{:.1}% of the ground is open cave", 100.0 * carved as f64 / ground as f64);
        write("caves_xray.ppm", SIZE, SIZE, &xray);

        let colour = |x: i32, y: i32, z: i32| -> [f32; 3] {
            let b = block(x, y, z);
            let above = y > world.height(x, z);
            match b {
                _ if b == water => if above { [0.2, 0.35, 0.7] } else { [0.15, 0.3, 0.95] },
                _ if b == lava => [1.0, 0.45, 0.1],
                _ if b.is_air() => if above { [0.65, 0.8, 0.95] } else { [0.02, 0.02, 0.03] },
                _ => {
                    let c = registry.def(b).color.map(|c| c as f32 / 255.0);
                    c.map(|c| 0.25 + 0.5 * c)
                }
            }
        };
        let mut section = Vec::new();
        for y in (0..WORLD_HEIGHT_FOR_TESTS).rev() {
            for x in origin.x..origin.x + SIZE {
                section.push(colour(x, y, centre.z));
            }
        }
        for y in (0..WORLD_HEIGHT_FOR_TESTS).rev() {
            for z in origin.z..origin.z + SIZE {
                section.push(colour(centre.x, y, z));
            }
        }
        write("caves_section.ppm", SIZE, 2 * WORLD_HEIGHT_FOR_TESTS, &section);
        // Level cuts at two heights, side by side.
        let levels = [env("Y1").unwrap_or(50) as i32, env("Y2").unwrap_or(100) as i32];
        let mut cuts = Vec::with_capacity((2 * SIZE * SIZE) as usize);
        for z in origin.z..origin.z + SIZE {
            for y in levels {
                for x in origin.x..origin.x + SIZE {
                    cuts.push(colour(x, y, z));
                }
            }
        }
        write("caves_levels.ppm", 2 * SIZE, SIZE, &cuts);
        println!("{:#?}\ncentre {centre}, spawn {spawn}", world.cave_style());
    }

    /// `cargo test --release -p voxmar_worldgen cave_sights -- --ignored --nocapture`
    /// (optionally `SEED=n`, or several as `SEEDS=1,2,3`) prints `--pose` arguments for
    /// in-game captures of caves near spawn: inside the largest open space under each sight.
    #[test]
    #[ignore]
    fn cave_sights() {
        let registry = registry();
        let seeds = std::env::var("SEEDS").or_else(|_| std::env::var("SEED")).unwrap_or("24301".into());
        let (water, lava) = (registry.expect("base:water"), registry.expect("base:lava"));
        for seed in seeds.split(',').map(|s| s.parse::<u64>().unwrap()) {
            let world = Overworld::new(seed, registry.clone());
            let spawn = world.spawn_point();
            let eye = spawn.as_vec3() + voxmar_engine::glam::Vec3::new(0.5, 1.62, 0.5);
            println!("seed {seed}: spawn {spawn}, {:?}", world.cave_style());
            for (kind, x, z) in world.caves.sights((spawn.x, spawn.z), 420) {
                if kind == "ravine" || kind == "sinkhole" {
                    // From above and to the east, looking west and down into it.
                    let ground = (-20..=20).map(|d| world.height(x + 18, z + d)).max().unwrap();
                    let d = voxmar_engine::glam::Vec3::new(x as f32 + 18.0, ground as f32 + 14.0, z as f32) - eye;
                    println!("  {kind} from above: --pose {:.1},{:.1},{:.1},90,-40", d.x, d.y, d.z);
                }
                // The tallest open run of cave in the 5×5 columns around the sight.
                let mut best = (0, 0, 0, 0, 0);
                for (dx, dz) in (-2..=2).flat_map(|a| (-2..=2).map(move |b| (a * 6, b * 6))) {
                    let (cx, cz) = (x + dx, z + dz);
                    let pos = ColumnPos::of_block(IVec3::new(cx, 0, cz));
                    let column = world.generate(pos);
                    let height = world.height(cx, cz);
                    let local = |y: i32| column.block(IVec3::new(cx & 31, y, cz & 31));
                    let mut run = 0;
                    for y in (5..=height + 1).rev() {
                        let b = local(y);
                        if b.is_air() && y <= height || kind == "ravine" && b.is_air() {
                            run += 1;
                        } else {
                            if run > best.0 {
                                best = (run, cx, y + 1, cz, if b == water { 1 } else if b == lava { 2 } else { 0 });
                            }
                            run = 0;
                        }
                    }
                }
                let (run, cx, floor, cz, under) = best;
                if run == 0 {
                    continue;
                }
                let camera = voxmar_engine::glam::Vec3::new(cx as f32 + 0.5, floor as f32 + (run as f32 * 0.5).min(6.0), cz as f32 + 0.5);
                let d = camera - eye;
                let fluid = ["", " over water", " over lava"][under];
                println!("  {kind}: {run} tall at {cx} {floor} {cz}{fluid}: --pose {:.1},{:.1},{:.1},0,0", d.x, d.y, d.z);
            }
            // Openings in the ground within 300 blocks: the largest few, with a pose a little
            // way off looking down into them.
            let first = ColumnPos::of_block(spawn).offset(-9, -9);
            let columns = generate_area(&world, first, 19);
            let mut openings = Vec::new();
            for (i, column) in columns.iter().enumerate() {
                let origin = first.offset(i as i32 % 19, i as i32 / 19).origin();
                let mut count = 0;
                let mut deepest = (0, IVec3::ZERO);
                for z in 0..32 {
                    for x in 0..32 {
                        let h = world.height(origin.x + x, origin.z + z);
                        if h <= SEA_LEVEL || !column.block(IVec3::new(x, h, z)).is_air() {
                            continue;
                        }
                        count += 1;
                        let depth = (0..h).rev().take_while(|&y| column.block(IVec3::new(x, y, z)).is_air()).count() as i32;
                        if depth > deepest.0 {
                            deepest = (depth, origin + IVec3::new(x, h, z));
                        }
                    }
                }
                if count > 0 {
                    openings.push((count, deepest));
                }
            }
            openings.sort_by_key(|o| -o.0);
            println!("  {} columns with openings", openings.len());
            for (count, (depth, at)) in openings.into_iter().take(5) {
                let camera = at.as_vec3() + voxmar_engine::glam::Vec3::new(10.0, 9.0, 10.0);
                let d = camera - eye;
                println!("  opening: {count} blocks, {depth} deep at {at}: --pose {:.1},{:.1},{:.1},45,-35", d.x, d.y, d.z);
            }
        }
    }

    const WORLD_HEIGHT_FOR_TESTS: i32 = voxmar_engine::coords::WORLD_HEIGHT;

    /// `cargo test --release -p voxmar_worldgen map_preview -- --ignored` writes a shaded
    /// top-down map (north up, 8 blocks per pixel) around the origin to
    /// `target/map_preview.ppm`, for judging the climate layout at a glance. With
    /// `FIELD=erosion` (or `continentalness`, `weirdness`, `aether`, `fertility`,
    /// `variant`) it shows that selection field on land instead, dark to light.
    #[test]
    #[ignore]
    fn map_preview() {
        let field: Option<fn(&Site) -> f32> = match std::env::var("FIELD").as_deref() {
            Ok("erosion") => Some(|s| s.erosion),
            Ok("continentalness") => Some(|s| s.continentalness * 0.5 + 0.5),
            Ok("weirdness") => Some(|s| s.weirdness),
            Ok("aether") => Some(|s| s.aether),
            Ok("fertility") => Some(|s| s.fertility),
            Ok("variant") => Some(|s| s.variant),
            _ => None,
        };
        const SIZE: i32 = 1024;
        const SCALE: i32 = 8;
        let registry = registry();
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5eed);
        let world = Overworld::new(seed, registry.clone());
        let half = SIZE * SCALE / 2;
        let sampler = world.sampler(IVec2::splat(-half - SCALE), IVec2::splat(half + SCALE));
        let at = |i: i32| i * SCALE - half;
        let heights: Vec<i32> = (0..SIZE * SIZE).map(|i| world.height(at(i % SIZE), at(i / SIZE))).collect();
        let height = |x: i32, z: i32| heights[(z.clamp(0, SIZE - 1) * SIZE + x.clamp(0, SIZE - 1)) as usize];
        let color = |block: BlockId| registry.def(block).color.map(|c| c as f32 / 255.0);
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
        let mut wet = 0;
        for z in 0..SIZE {
            for x in 0..SIZE {
                let h = height(x, z);
                let slope = (height(x + 1, z) - h).abs().max((height(x, z + 1) - h).abs()) / SCALE;
                let surface = sampler.surface(at(x), at(z), h, slope);
                let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
                let mut rgb = color(surface.ground);
                if world.terrain.is_grass(surface.ground) {
                    // Roughly the grass colour map: blue-green when cold, yellow when hot and
                    // dry, deep green when hot and wet.
                    let [t, m] = surface.climate.map(|c| c as f32 / 255.0);
                    rgb = mix([0.5, 0.65, 0.55], mix([0.75, 0.72, 0.36], [0.28, 0.69, 0.1], m), t);
                }
                if let Some((leaves, cover)) = surface.canopy {
                    let name = &registry.def(leaves).name;
                    let crown = [("spruce", [0.15, 0.3, 0.2]), ("birch", [0.45, 0.6, 0.3]), ("jungle", [0.2, 0.55, 0.1]), ("acacia", [0.45, 0.5, 0.15])]
                        .into_iter()
                        .find(|(tree, _)| name.contains(tree))
                        .map_or([0.25, 0.45, 0.15], |(_, crown)| crown);
                    rgb = mix(rgb, crown, cover);
                }
                if let Some(field) = field {
                    let v = field(&sampler.site(at(x), at(z)));
                    rgb = [0.1 + 0.9 * v, 0.1 + 0.8 * v * v, 0.3 + 0.4 * v];
                }
                if let Some(water) = surface.water {
                    wet += 1;
                    let depth = ((SEA_LEVEL - h) as f32 / 30.0).min(1.0);
                    rgb = if water == registry.expect("base:ice") { [0.75, 0.85, 0.95] } else { [0.15, 0.35 - 0.15 * depth, 0.7 - 0.25 * depth] };
                } else {
                    // Light from the north-west.
                    let shade = 1.0 + ((height(x - 1, z - 1) - h) as f32 * 0.04).clamp(-0.4, 0.4);
                    rgb = rgb.map(|c| c * shade);
                }
                pixels.extend(rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8));
            }
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/map_preview.ppm");
        let mut file = format!("P6 {SIZE} {SIZE} 255\n").into_bytes();
        file.extend(pixels);
        std::fs::write(path, file).unwrap();
        let water = wet as f32 / (SIZE * SIZE) as f32;
        let spawn = world.spawn_point();
        println!("{:#?}\nwater {water:.2}, spawn {spawn}: {:#?}\nwrote {path}", world.dials(), world.site(spawn.x, spawn.z));
    }

    /// CPU time this thread has used: unlike wall-clock time, it is not inflated when other
    /// processes compete for the cores.
    fn thread_time() -> std::time::Duration {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a valid timespec for the call to fill.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        std::time::Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
    }

    /// `cargo test --release -p voxmar_worldgen generation_speed -- --ignored --nocapture`
    /// measures, in CPU time on one thread, full column generation over areas in several
    /// climates and seeds, and the distant-terrain surface samples of the same areas.
    #[test]
    #[ignore]
    fn generation_speed() {
        use std::time::Duration;
        let registry = registry();
        let (mut columns, mut generating) = (0, Duration::ZERO);
        let (mut samples, mut sampling) = (0, Duration::ZERO);
        for seed in [1, 3, 0x5eed] {
            let world = Overworld::new(seed, registry.clone());
            for (cx, cz) in [(0, 0), (60, -90), (-120, 40), (150, 150), (-40, -200)] {
                let start = thread_time();
                for i in 0..64 {
                    std::hint::black_box(world.generate(ColumnPos::new(cx + i % 8, cz + i / 8)));
                }
                generating += thread_time() - start;
                columns += 64;

                // A distant-terrain tile: 33×33 samples, 8 blocks apart.
                let origin = IVec2::new(cx, cz) * voxmar_engine::coords::CHUNK_SIZE;
                let start = thread_time();
                let sampler = world.sampler(origin - 8, origin + 33 * 8);
                for i in 0..33 * 33 {
                    let (x, z) = (origin.x + i % 33 * 8, origin.y + i / 33 * 8);
                    std::hint::black_box(sampler.surface(x, z, world.height(x, z), 0));
                }
                sampling += thread_time() - start;
                samples += 33 * 33;
            }
        }
        println!(
            "{:.3} ms per column ({columns} columns), {:.2} µs per surface sample ({samples} samples)",
            generating.as_secs_f64() * 1e3 / columns as f64,
            sampling.as_secs_f64() * 1e6 / samples as f64,
        );
    }

    #[test]
    fn temperature_changes_smoothly_and_in_order() {
        let world = Overworld::new(11, registry());
        for x in [-2000, 0, 3000] {
            let sampler = world.sampler(IVec2::new(x, -4000), IVec2::new(x, 4000));
            // Sea-level temperature: what the site would have without cooling with height.
            let sea_level = |z: i32| {
                let site = sampler.site(x, z);
                site.temperature + LAPSE_RATE * (site.height - SEA_LEVEL).max(0) as f32
            };
            let temperatures: Vec<f32> = (-4000..=4000).step_by(8).map(sea_level).collect();
            let steepest = temperatures.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
            assert!(steepest < 0.25, "temperature jumps {steepest} °C over 8 blocks at x {x}");
            // Climate zones are kilometres wide: from cold to hot over this line.
            let (min, max) = temperatures.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &t| (lo.min(t), hi.max(t)));
            assert!(max - min > 15.0, "only {min}..{max} °C along x {x}");
        }
    }

    #[test]
    fn deserts_never_border_snow() {
        const STEP: i32 = 32;
        const SIZE: i32 = 8192 / STEP;
        // Closest a hot desert may come to snow, in samples.
        const GAP: i32 = 128 / STEP;
        let registry = registry();
        let (sand, red_sand) = (registry.expect("base:sand"), registry.expect("base:red_sand"));
        let snowy = ["base:snow_block", "base:powder_snow", "base:snowy_grass_block"].map(|n| registry.expect(n));
        for seed in [1, 3, 0x5eed] {
            let world = Overworld::new(seed, registry.clone());
            let half = SIZE * STEP / 2;
            let sampler = world.sampler(IVec2::splat(-half - 1), IVec2::splat(half + 1));
            let mut snow = vec![false; (SIZE * SIZE) as usize];
            let mut deserts = Vec::new();
            for i in 0..SIZE {
                for j in 0..SIZE {
                    let (x, z) = (i * STEP - half, j * STEP - half);
                    let site = sampler.site(x, z);
                    let top = world.terrain.surface_layers(&site, x, z).top;
                    snow[(j * SIZE + i) as usize] = snowy.contains(&top);
                    // Sand away from beaches and banks is desert.
                    if (top == sand || top == red_sand) && site.height > SEA_LEVEL + 3 && site.river < 0.3 {
                        deserts.push((i, j));
                    }
                }
            }
            assert!(!deserts.is_empty() || seed != 3, "the dry world has no desert");
            for (i, j) in deserts {
                for dj in -GAP..=GAP {
                    for di in -GAP..=GAP {
                        let (si, sj) = (i + di, j + dj);
                        let inside = (0..SIZE).contains(&si) && (0..SIZE).contains(&sj);
                        if inside && di * di + dj * dj <= GAP * GAP && snow[(sj * SIZE + si) as usize] {
                            let (desert, snow) = ((i * STEP - half, j * STEP - half), (si * STEP - half, sj * STEP - half));
                            panic!("seed {seed}: desert at {desert:?} within {} blocks of snow at {snow:?}", GAP * STEP);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn selection_fields_are_smooth_and_evenly_spread() {
        let world = Overworld::new(5, registry());
        let sampler = world.sampler(IVec2::splat(-6000), IVec2::splat(6000));
        let sites: Vec<Site> = (0..150 * 150).map(|i| sampler.site(i % 150 * 80 - 6000, i / 150 * 80 - 6000)).collect();
        // Evenly spread fields put a tenth of the land in each tenth of their range.
        let fields: [(&str, fn(&Site) -> f32); 3] = [("weirdness", |s| s.weirdness), ("fertility", |s| s.fertility), ("variant", |s| s.variant)];
        for (name, field) in fields {
            let values: Vec<f32> = sites.iter().map(field).collect();
            assert!(values.iter().all(|v| (0.0..=1.0).contains(v)), "{name} out of range");
            for decile in 0..10 {
                let share = values.iter().filter(|&&v| (v * 10.0) as i32 == decile).count() as f32 / values.len() as f32;
                assert!((0.04..0.16).contains(&share), "{name}: {share} of the land in decile {decile}");
            }
        }
        let median = |mut v: Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v[v.len() / 2]
        };
        let aether = median(sites.iter().map(|s| s.aether).collect());
        assert!((aether - world.dials().fantasy).abs() < 0.1, "median aether {aether}, fantasy {}", world.dials().fantasy);
        assert!(sites.iter().all(|s| (-1.0..=1.0).contains(&s.continentalness) && (0.0..=1.0).contains(&s.erosion)));
        assert!(sites.iter().any(|s| s.continentalness < -0.5) && sites.iter().any(|s| s.continentalness > 0.5));
        assert!(sites.iter().any(|s| s.erosion > 0.9) && sites.iter().any(|s| s.erosion < 0.05));

        // Block to block, nothing a biome is picked by jumps.
        for x in [-1500, 400] {
            let line: Vec<Site> = (-3000..3000).map(|z| sampler.site(x, z)).collect();
            for pair in line.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                for (name, step) in [
                    ("continentalness", (a.continentalness - b.continentalness).abs()),
                    ("erosion", (a.erosion - b.erosion).abs()),
                    ("weirdness", (a.weirdness - b.weirdness).abs()),
                    ("aether", (a.aether - b.aether).abs()),
                    ("variant", (a.variant - b.variant).abs()),
                ] {
                    assert!(step < 0.03, "{name} jumps by {step} at x {x}");
                }
            }
        }
    }

    #[test]
    fn sampler_matches_single_sites() {
        let world = Overworld::new(3, registry());
        let sampler = world.sampler(IVec2::new(-300, 200), IVec2::new(100, 700));
        for (x, z) in [(-300, 200), (-1, 450), (37, 699), (100, 700)] {
            assert_eq!(sampler.site(x, z), world.site(x, z));
        }
    }
}
