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

mod climate;
mod dials;
mod ecology;
mod features;
mod geology;
mod resources;
mod rng;
mod terrain;

use std::sync::Arc;

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

pub use climate::LAPSE_RATE;
pub use dials::WorldDials;

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

pub const SEA_LEVEL: i32 = 64;

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
    /// 0 at the coast ..= 1 deep inland.
    pub continentality: f32,
    /// 0 at the equator ..= 1 at a pole.
    pub latitude: f32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
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
    terrain: terrain::Terrain,
    seed: u64,
}

impl Overworld {
    pub const ID: &'static str = "base:overworld";
    /// Bump when generation output changes.
    pub const VERSION: u32 = 2;

    pub fn new(seed: u64, registry: Arc<BlockRegistry>) -> Self {
        let stream = rng::stream_seed(seed, Self::ID, Self::VERSION);
        let dials = WorldDials::sample(stream);
        Self {
            dials,
            geology: Geology::new(stream, &dials),
            climate: Climate::new(stream, &dials),
            ecology: Ecology::new(stream, &dials, &registry),
            terrain: terrain::Terrain::new(stream, &registry),
            seed: stream,
        }
    }

    /// The world's global parameters.
    pub fn dials(&self) -> &WorldDials {
        &self.dials
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
        let canopy = (water.is_none() && site.forest > 0.0).then(|| {
            let tree = self.world.ecology.dominant_tree(&site, x, z);
            let cover = 1.0 - (-2.0 * site.forest * ecology::TREE_CHANCE).exp();
            (self.world.terrain.leaves(tree), cover)
        });
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
            continentality: regional.continentality,
            latitude: regional.latitude,
            mountain: geo.mountain,
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
                if pleasant && open && self.terrain.is_grass(self.terrain.surface_blocks(&site, x, z).0) {
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
        let quads: usize = (0..8).map(|cy| mesh_chunk(&registry, &region, &columns[4], cy).opaque.vertices.len() / 4).sum();
        println!("9 columns {generated:?}, light {lit:?}, mesh {:?}, {quads} quads, spawn {spawn}", start.elapsed());
        println!("{:#?}", world.dials());
        assert!(quads > 0);
    }

    /// `cargo test --release -p voxmar_worldgen map_preview -- --ignored` writes a shaded
    /// top-down map (north up, 8 blocks per pixel) around the origin to
    /// `target/map_preview.ppm`, for judging the climate layout at a glance.
    #[test]
    #[ignore]
    fn map_preview() {
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

    #[test]
    fn sampler_matches_single_sites() {
        let world = Overworld::new(3, registry());
        let sampler = world.sampler(IVec2::new(-300, 200), IVec2::new(100, 700));
        for (x, z) in [(-300, 200), (-1, 450), (37, 699), (100, 700)] {
            assert_eq!(sampler.site(x, z), world.site(x, z));
        }
    }
}
