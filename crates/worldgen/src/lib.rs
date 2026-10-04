//! Procedural world generation.
//!
//! The overworld generator runs in stages, each a pure function of the world seed and
//! position: world dials (global parameters such as mean temperature, ocean fraction and
//! wind), geology (continents, mountains, rivers and lakes), climate (temperature and
//! precipitation as continuous fields), ecology (what grows where), resources (ore veins)
//! and terrain (blocks, caves, trees, plants). Biomes are data (`biomes.ron` in content
//! packages) placed over the continuous climate: they decide surface materials and what
//! grows, while temperature, moisture and the rest stay continuous underneath, and
//! `Overworld::site` exposes all of it for gameplay. Purity means any column can be
//! generated alone, in any order, on any thread, and features that cross column borders
//! (trees, veins) come out identical on both sides.

mod biome;
mod climate;
mod content;
mod dials;
mod ecology;
mod features;
mod geology;
mod rarity;
mod resources;
mod rng;
mod rocks;
mod surface;
mod terrain;
mod trees;

use std::sync::Arc;

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

pub use biome::{Biome, BiomeId, Biomes};
pub use climate::LAPSE_RATE;
pub use content::{ContentError, WorldContent};
pub use dials::WorldDials;
pub use rarity::{Featured, Rarity};
pub use rocks::ProvinceId;

use fastnoise_lite::{FastNoiseLite, FractalType};

use crate::biome::{GRADIENT_STEP, Pick, Point};
use crate::climate::{Climate, ClimateGrid, Regional};
use crate::ecology::Ecology;
use crate::geology::{GeoSample, Geology, smoothstep};
use crate::rocks::Rocks;
use crate::surface::SurfaceRules;

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
    /// The rock province beneath.
    pub province: ProvinceId,
    /// The biome whose rules make this block: near a border, the home biome or its
    /// neighbour, dithered; in a river or lake, the river biome of the climate.
    pub biome: BiomeId,
    /// The biome this position belongs to.
    pub home: BiomeId,
    /// The nearest other biome, across the nearest border; `home` when none is near.
    pub neighbour: BiomeId,
    /// Distance to that border, in blocks, up to 128.
    pub edge: f32,
    /// 0 at the centre of the home biome's climate ..= 1 at its edge.
    pub from_centre: f32,
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
    /// Grass, foliage and water colours.
    pub colors: BiomeColors,
}

/// The colours of grass, foliage and water at a position, sRGB. They follow a
/// continuous climate colour map, broken up by palette noise, except where a biome sets
/// its own; near a border the two biomes' colours blend, so there are no seams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeColors {
    pub grass: [u8; 3],
    pub foliage: [u8; 3],
    pub water: [u8; 3],
}

/// The default overworld.
pub struct Overworld {
    dials: WorldDials,
    geology: Geology,
    climate: Climate,
    ecology: Ecology,
    biomes: Biomes,
    rocks: Rocks,
    rules: SurfaceRules,
    terrain: terrain::Terrain,
    /// Clumps for dithering between biomes near their borders.
    blend: FastNoiseLite,
    /// Patches of lighter and darker grass and leaves.
    tint: FastNoiseLite,
    seed: u64,
}

impl Overworld {
    pub const ID: &'static str = "base:overworld";
    /// Bump when generation output changes.
    pub const VERSION: u32 = 5;

    /// A generator with the base package's biomes, rock provinces and climate bands
    /// alone, for tools and distant terrain; worlds are made `with_content`.
    pub fn new(seed: u64, registry: Arc<BlockRegistry>) -> Self {
        let content = WorldContent::base(&registry).expect("the base package's world content is valid");
        Self::with_content(seed, registry, &content)
    }

    /// A generator with the world content of loaded packages ([`WorldContent::load`]).
    pub fn with_content(seed: u64, registry: Arc<BlockRegistry>, content: &WorldContent) -> Self {
        let stream = rng::stream_seed(seed, Self::ID, Self::VERSION);
        let dials = WorldDials::sample(stream);
        let rocks = Rocks::new(stream, content, &registry, &dials.featured);
        let mut rules = SurfaceRules::new(stream, content, &registry);
        let biomes = Biomes::new(content, &registry, &mut rules, &rocks, &dials.featured);
        Self {
            dials,
            geology: Geology::new(stream, &dials),
            climate: Climate::new(stream, &dials),
            ecology: Ecology::new(stream, &dials, &registry),
            terrain: terrain::Terrain::new(stream, &registry, content, &rocks, &biomes),
            biomes,
            rocks,
            rules,
            blend: geology::noise(stream, 40, 1.0 / 12.0, FractalType::FBm, 2),
            tint: geology::noise(stream, 41, 1.0 / 45.0, FractalType::FBm, 2),
            seed: stream,
        }
    }

    /// The biomes of this world.
    pub fn biomes(&self) -> &Biomes {
        &self.biomes
    }

    /// The biome whose rules make the ground at (x, z).
    pub fn biome_at(&self, x: i32, z: i32) -> &Biome {
        self.biomes.get(self.site(x, z).biome)
    }

    /// The nearest place within `radius` blocks of `from` that belongs to the biome
    /// named `name` (`base:taiga`, or `taiga` for the base package's), searched every 64
    /// blocks outwards. `None` if there is no such biome or none is that close.
    pub fn locate_biome(&self, name: &str, from: IVec2, radius: i32) -> Option<IVec2> {
        let id = self.biomes.id(name).or_else(|| self.biomes.id(&format!("base:{name}")))?;
        self.biomes.locate(id, from, radius, climate::CELL, |at| self.point(&self.climate.regional_at(&self.geology, at)))
    }

    /// The biome picker's fields from the regional climate.
    fn point(&self, regional: &Regional) -> Point {
        let temperature = regional.sea_temperature - LAPSE_RATE * (regional.elevation - SEA_LEVEL as f32).max(0.0);
        let moisture = climate::moisture(regional.rainfall, temperature);
        self.biomes.remap(Point([
            temperature,
            moisture,
            regional.continentalness,
            regional.erosion,
            regional.weirdness,
            regional.aether,
            regional.variant,
        ]))
    }

    /// Clumpy dither in 0..1 for choosing between two biomes near their border.
    fn blend_dither(&self, x: i32, z: i32) -> f32 {
        let speckle = (rng::hash3(self.seed ^ 0xb1e4, x, 0, z) & 0xffff) as f32 / 65536.0;
        let patch = self.blend.get_noise_2d(x as f32, z as f32) * 0.6 + 0.5;
        (0.6 * patch + 0.4 * speckle).clamp(0.0, 1.0)
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
        Sampler { world: self, grid: self.grid(min, max), picks: None }
    }

    /// A sampler that picks the biomes of every cell in its area up front, for sampling
    /// it block by block.
    pub(crate) fn dense_sampler(&self, min: IVec2, max: IVec2) -> Sampler<'_> {
        let mut sampler = Sampler { world: self, grid: self.grid(min, max), picks: None };
        let (first, last) = (min.div_euclid(IVec2::splat(biome::CELL)), max.div_euclid(IVec2::splat(biome::CELL)));
        let size = last - first + 1;
        let picks = (0..size.x * size.y).map(|i| sampler.pick_cell(first + IVec2::new(i % size.x, i / size.x))).collect();
        sampler.picks = Some(Picks { first, width: size.x, picks });
        sampler
    }

    /// The regional climate over an area, with room for the biome picker's probes.
    fn grid(&self, min: IVec2, max: IVec2) -> ClimateGrid {
        let margin = biome::CELL + GRADIENT_STEP;
        ClimateGrid::new(&self.climate, &self.geology, min - margin, max + margin)
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
    picks: Option<Picks>,
}

/// Biome picks for every cell of an area.
struct Picks {
    /// The first cell.
    first: IVec2,
    width: i32,
    picks: Vec<Pick>,
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
        let (ground, water) = self.world.surface_blocks(&site, x, z);
        let canopy = if water.is_some() { None } else { self.world.ecology.canopy(&site, self.world.terrain.ground(ground), x, z) }
            .map(|(tree, cover)| (self.world.terrain.leaves(tree), cover));
        // Distant terrain takes the colours of the cell alone.
        let pick = self.pick(x, z);
        let byte = |c: [f32; 3]| c.map(|v| v.round().clamp(0.0, 255.0) as u8);
        let [grass, foliage, water_color] = self.blend_colors(&site, x, z, &pick, Biomes::blend(pick.edge, site.fertility)).map(byte);
        let colors = BiomeColors { grass, foliage, water: water_color };
        SurfaceSample { height, ground, water, canopy, climate: site.tint(), colors }
    }

    /// The grass, foliage and water colours at a position inside the sampler's area,
    /// changing smoothly from block to block.
    pub fn colors(&self, x: i32, z: i32) -> BiomeColors {
        let site = self.site(x, z);
        // Bilinear between the four nearest cell centres, so identity colours fade in
        // block by block rather than in steps of a cell.
        let p = IVec2::new(x, z) - biome::CELL / 2;
        let cell = p.div_euclid(IVec2::splat(biome::CELL));
        let f = (p - cell * biome::CELL).as_vec2() / biome::CELL as f32;
        let mut sum = [[0.0f32; 3]; 3];
        for (dx, dz, w) in [(0, 0, (1.0 - f.x) * (1.0 - f.y)), (1, 0, f.x * (1.0 - f.y)), (0, 1, (1.0 - f.x) * f.y), (1, 1, f.x * f.y)] {
            let at = (cell + IVec2::new(dx, dz)) * biome::CELL;
            let pick = self.pick(at.x, at.y);
            let colors = self.blend_colors(&site, x, z, &pick, Biomes::blend(pick.edge, site.fertility));
            for (total, c) in sum.iter_mut().zip(colors) {
                for k in 0..3 {
                    total[k] += w * c[k];
                }
            }
        }
        let byte = |c: [f32; 3]| c.map(|v| v.round().clamp(0.0, 255.0) as u8);
        BiomeColors { grass: byte(sum[0]), foliage: byte(sum[1]), water: byte(sum[2]) }
    }

    /// Colours from the climate at `site` and the biomes of `pick`, the neighbour's taking
    /// `share`: grass, foliage and water.
    fn blend_colors(&self, site: &Site, x: i32, z: i32, pick: &Pick, share: f32) -> [[f32; 3]; 3] {
        let [t, h] = site.tint().map(|c| c as f32 / 255.0);
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
        // Blue-green when cold, olive when hot and dry, deep green when hot and wet.
        let grass = mix([128.0, 167.0, 140.0], mix([191.0, 183.0, 85.0], [71.0, 176.0, 26.0], h), t);
        let foliage = mix([97.0, 140.0, 110.0], mix([174.0, 164.0, 42.0], [48.0, 150.0, 18.0], h), t);
        let water = mix([61.0, 87.0, 214.0], [67.0, 213.0, 238.0], t * (0.4 + 0.6 * h));
        // Patches a little lighter or darker, warmer or cooler.
        let n = self.world.tint.get_noise_2d(x as f32, z as f32);
        let palette = |c: [f32; 3]| [c[0] * (1.0 + 0.06 * n), c[1] * (1.0 + 0.03 * n), c[2] * (1.0 - 0.04 * n)];
        let climate = [palette(grass), palette(foliage), water];

        let own = |biome: BiomeId| {
            let colors = &self.world.biomes.get(biome).colors;
            let mut own = climate;
            for (c, set) in own.iter_mut().zip([colors.grass, colors.foliage, colors.water]) {
                if let Some((r, g, b)) = set {
                    *c = palette([r as f32, g as f32, b as f32]);
                }
            }
            own
        };
        let (home, neighbour) = (own(pick.home), own(pick.neighbour));
        std::array::from_fn(|i| mix(home[i], neighbour[i], share))
    }

    /// The biome pick of the cell holding (x, z).
    fn pick(&self, x: i32, z: i32) -> Pick {
        let cell = IVec2::new(x, z).div_euclid(IVec2::splat(biome::CELL));
        if let Some(picks) = &self.picks {
            let at = cell - picks.first;
            if at.x >= 0 && at.y >= 0 && at.x < picks.width && ((at.y * picks.width + at.x) as usize) < picks.picks.len() {
                return picks.picks[(at.y * picks.width + at.x) as usize];
            }
        }
        self.pick_cell(cell)
    }

    /// Picks the biome of a cell from the fields at its centre.
    fn pick_cell(&self, cell: IVec2) -> Pick {
        let centre = cell * biome::CELL + biome::CELL / 2;
        let point = |p: IVec2| self.world.point(&self.grid.at(p.x, p.y));
        let around = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y].map(|d| point(centre + d * GRADIENT_STEP));
        self.world.biomes.pick(&point(centre), &around)
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
            province: self.world.rocks.at(x, z, regional.warp),
            biome: BiomeId(0),
            home: BiomeId(0),
            neighbour: BiomeId(0),
            edge: 0.0,
            from_centre: 0.0,
        };
        if geo.height >= SEA_LEVEL {
            let water = site.soil_moisture();
            site.vegetation = ecology::ground_cover(temperature, water);
            site.forest = self.world.ecology.forest(x, z, temperature, water);
        }

        let pick = self.pick(x, z);
        let share = Biomes::blend(pick.edge, regional.fertility);
        let dither = if share > 0.0 || geo.river > 0.0 { self.world.blend_dither(x, z) } else { 1.0 };
        site.biome = if dither < share { pick.neighbour } else { pick.home };
        // Rivers and lakes and their banks, with ragged edges.
        if let Some(river) = pick.river
            && geo.river > 0.4 + 0.35 * smoothstep(0.0, 1.0, dither)
        {
            site.biome = river;
        }
        (site.home, site.neighbour, site.edge, site.from_centre) = (pick.home, pick.neighbour, pick.edge, pick.from_centre);
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
                if pleasant && open && self.terrain.is_grass(self.surface_layers(&site, x, z).top) {
                    return IVec3::new(x, height + 1, z);
                }
            }
        }
        IVec3::new(0, voxmar_engine::coords::WORLD_HEIGHT - 2, 0)
    }
}

/// The `step`-th position on the square ring at Chebyshev distance `ring`.
pub(crate) fn spiral(ring: i32, step: i32) -> (i32, i32) {
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

    /// `cargo test --release -p voxmar_worldgen map_preview -- --ignored --nocapture`
    /// writes a shaded top-down map (north up, 8 blocks per pixel) around the origin to
    /// `target/map_preview.ppm`, for judging the climate layout at a glance, and prints
    /// each biome's share of the map and how many biomes lie within 5 km of spawn.
    ///
    /// - `SEED=n`, or `SEED=1,2,3` for several maps (`map_preview_<seed>.ppm`);
    /// - `BIOMES=1` colours land by biome (a colour per biome, from its name);
    /// - `FIELD=erosion` (or `continentalness`, `weirdness`, `aether`, `fertility`,
    ///   `variant`) shows that selection field on land, dark to light.
    #[test]
    #[ignore]
    fn map_preview() {
        let seeds: Vec<u64> = std::env::var("SEED").ok().map_or(vec![0x5eed], |s| s.split(',').filter_map(|s| s.trim().parse().ok()).collect());
        for &seed in &seeds {
            let path = if seeds.len() == 1 {
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/map_preview.ppm").to_owned()
            } else {
                format!("{}/../../target/map_preview_{seed}.ppm", env!("CARGO_MANIFEST_DIR"))
            };
            map(seed, &path);
        }
    }

    /// A colour per biome, from its name.
    fn biome_color(name: &str) -> [f32; 3] {
        let h = rarity::name_hash(name);
        let hue = (h % 360) as f32 / 60.0;
        let (s, v) = (0.45 + (h >> 16 & 0xff) as f32 / 255.0 * 0.4, 0.55 + (h >> 24 & 0xff) as f32 / 255.0 * 0.4);
        let c = v * s;
        let x = c * (1.0 - (hue % 2.0 - 1.0).abs());
        let (r, g, b) = match hue as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        [r + v - c, g + v - c, b + v - c]
    }

    fn map(seed: u64, path: &str) {
        type Field = fn(&Site) -> f32;
        let field: Option<Field> = match std::env::var("FIELD").as_deref() {
            Ok("erosion") => Some(|s| s.erosion),
            Ok("continentalness") => Some(|s| s.continentalness * 0.5 + 0.5),
            Ok("weirdness") => Some(|s| s.weirdness),
            Ok("aether") => Some(|s| s.aether),
            Ok("fertility") => Some(|s| s.fertility),
            Ok("variant") => Some(|s| s.variant),
            Ok("moisture") => Some(|s| s.moisture / 2.0),
            Ok("temperature") => Some(|s| (s.temperature + 10.0) / 40.0),
            _ => None,
        };
        let by_biome = std::env::var("BIOMES").is_ok_and(|v| v != "0");
        const SIZE: i32 = 1024;
        const SCALE: i32 = 8;
        let registry = registry();
        let world = Overworld::new(seed, registry.clone());
        let half = SIZE * SCALE / 2;
        let sampler = world.sampler(IVec2::splat(-half - SCALE), IVec2::splat(half + SCALE));
        let at = |i: i32| i * SCALE - half;
        let heights: Vec<i32> = (0..SIZE * SIZE).map(|i| world.height(at(i % SIZE), at(i / SIZE))).collect();
        let height = |x: i32, z: i32| heights[(z.clamp(0, SIZE - 1) * SIZE + x.clamp(0, SIZE - 1)) as usize];
        let color = |block: BlockId| registry.def(block).color.map(|c| c as f32 / 255.0);
        let spawn = world.spawn_point();
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
        let mut wet = 0;
        let mut area = vec![0usize; world.biomes.len()];
        let mut near_spawn = vec![false; world.biomes.len()];
        for z in 0..SIZE {
            for x in 0..SIZE {
                let h = height(x, z);
                let slope = (height(x + 1, z) - h).abs().max((height(x, z + 1) - h).abs()) / SCALE;
                let surface = sampler.surface(at(x), at(z), h, slope);
                let site = sampler.site(at(x), at(z));
                area[site.biome.0 as usize] += 1;
                if (at(x) - spawn.x).pow(2) + (at(z) - spawn.z).pow(2) < 5000 * 5000 {
                    near_spawn[site.home.0 as usize] = true;
                }
                let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
                let mut rgb = color(surface.ground);
                if world.terrain.is_grass(surface.ground) {
                    rgb = surface.colors.grass.map(|c| c as f32 / 255.0);
                }
                if let Some((leaves, cover)) = surface.canopy {
                    let name = &registry.def(leaves).name;
                    let crown = [("spruce", [0.15, 0.3, 0.2]), ("birch", [0.45, 0.6, 0.3]), ("jungle", [0.2, 0.55, 0.1]), ("acacia", [0.45, 0.5, 0.15])]
                        .into_iter()
                        .find(|(tree, _)| name.contains(tree))
                        .map_or([0.25, 0.45, 0.15], |(_, crown)| crown);
                    rgb = mix(rgb, crown, cover);
                }
                if by_biome {
                    rgb = biome_color(&world.biomes.get(site.biome).name);
                }
                if let Some(field) = field {
                    let v = field(&site);
                    rgb = [0.1 + 0.9 * v, 0.1 + 0.8 * v * v, 0.3 + 0.4 * v];
                }
                if let Some(water) = surface.water {
                    wet += 1;
                    let depth = ((SEA_LEVEL - h) as f32 / 30.0).min(1.0);
                    let sea = if water == registry.expect("base:ice") { [0.75, 0.85, 0.95] } else { [0.15, 0.35 - 0.15 * depth, 0.7 - 0.25 * depth] };
                    rgb = if by_biome { mix(rgb, sea, 0.6) } else { sea };
                } else {
                    // Light from the north-west.
                    let shade = 1.0 + ((height(x - 1, z - 1) - h) as f32 * 0.04).clamp(-0.4, 0.4);
                    rgb = rgb.map(|c| c * shade);
                }
                pixels.extend(rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8));
            }
        }
        let mut file = format!("P6 {SIZE} {SIZE} 255\n").into_bytes();
        file.extend(pixels);
        std::fs::write(path, file).unwrap();
        let water = wet as f32 / (SIZE * SIZE) as f32;
        println!("seed {seed}: {:?}\nwater {water:.2}, spawn {spawn}: {:?}", world.dials(), world.site(spawn.x, spawn.z));
        let mut shares: Vec<(usize, &str)> = area.iter().enumerate().map(|(i, &a)| (a, world.biomes.get(BiomeId(i as u16)).name.as_str())).collect();
        shares.sort_by_key(|s| std::cmp::Reverse(s.0));
        for (a, name) in shares {
            let [r, g, b] = biome_color(name).map(|c| (c * 255.0) as u8);
            println!("  {:6.2}%  {name}  #{r:02x}{g:02x}{b:02x}", a as f32 * 100.0 / (SIZE * SIZE) as f32);
        }
        let near: Vec<&str> = (0..near_spawn.len()).filter(|&i| near_spawn[i]).map(|i| world.biomes.get(BiomeId(i as u16)).name.as_str()).collect();
        println!("{} biomes within 5 km of spawn: {}\nwrote {path}", near.len(), near.join(", "));
    }

    /// `SEED=1,2 cargo test --release -p voxmar_worldgen biomes_near_spawn -- --ignored
    /// --nocapture` lists the biomes within view of spawn, for in-game captures: each
    /// one's nearest point as an offset from the spawn point.
    #[test]
    #[ignore]
    fn biomes_near_spawn() {
        let seeds: Vec<u64> = std::env::var("SEED").ok().map_or(vec![7], |s| s.split(',').filter_map(|s| s.trim().parse().ok()).collect());
        for seed in seeds {
            let world = Overworld::new(seed, registry());
            let spawn = world.spawn_point();
            let sampler = world.sampler(IVec2::new(spawn.x, spawn.z) - 450, IVec2::new(spawn.x, spawn.z) + 450);
            let mut nearest: Vec<Option<(i32, IVec2, i32)>> = vec![None; world.biomes.len()];
            for dz in (-448..=448).step_by(8) {
                for dx in (-448..=448).step_by(8) {
                    let site = sampler.site(spawn.x + dx, spawn.z + dz);
                    let d = dx * dx + dz * dz;
                    let slot = &mut nearest[site.biome.0 as usize];
                    if slot.is_none_or(|(best, _, _)| d < best) {
                        *slot = Some((d, IVec2::new(dx, dz), site.height - spawn.y));
                    }
                }
            }
            println!("seed {seed}, spawn {spawn}:");
            for (i, n) in nearest.iter().enumerate() {
                if let Some((_, at, dy)) = n {
                    println!("  {:24} at {:4},{:4}, {dy:+} blocks", world.biomes.get(BiomeId(i as u16)).name, at.x, at.y);
                }
            }
        }
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
                    let top = world.surface_layers(&site, x, z).top;
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
        type Field = fn(&Site) -> f32;
        let fields: [(&str, Field); 3] = [("weirdness", |s| s.weirdness), ("fertility", |s| s.fertility), ("variant", |s| s.variant)];
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
    fn biomes_are_deterministic_in_any_sampler() {
        let blocks = registry();
        let (a, b) = (Overworld::new(9, blocks.clone()), Overworld::new(9, blocks.clone()));
        let (min, max) = (IVec2::new(-700, 300), IVec2::new(-500, 520));
        let (sparse, dense) = (a.sampler(min, max), b.dense_sampler(min, max));
        for i in 0..2000 {
            let (x, z) = (min.x + i * 37 % 201, min.y + i * 53 % 221);
            let site = sparse.site(x, z);
            assert_eq!(site, dense.site(x, z));
            assert_eq!(site, a.site(x, z));
            assert_eq!(sparse.colors(x, z), dense.colors(x, z));
        }
        // Worlds with other seeds pick other biomes.
        let c = Overworld::new(10, blocks);
        assert!((0..50).any(|i| a.site(i * 160, 0).home != c.site(i * 160, 0).home));
    }

    #[test]
    fn borders_blend_smoothly() {
        let world = Overworld::new(3, registry());
        let sampler = world.sampler(IVec2::new(-4000, -4000), IVec2::new(4000, 4000));
        let (mut borders, mut crossings, mut steps, mut jumps) = (0, 0, 0, 0);
        for x in [-2500, -300, 1700] {
            let line: Vec<Site> = (-4000..4000).map(|z| sampler.site(x, z)).collect();
            let colors: Vec<BiomeColors> = (-4000..4000).map(|z| sampler.colors(x, z)).collect();
            for i in 1..line.len() {
                let (a, b) = (&line[i - 1], &line[i]);
                // Colours never jump: no seams, even across borders. An identity colour may
                // differ from its neighbour's by a lot, so it fades in over the blend width.
                // (Banks are greener than the land around them; that is the climate's tint.)
                for (ca, cb) in [(colors[i - 1].grass, colors[i].grass), (colors[i - 1].foliage, colors[i].foliage), (colors[i - 1].water, colors[i].water)] {
                    let step = (0..3).map(|c| (ca[c] as i32 - cb[c] as i32).abs()).max().unwrap();
                    assert!(step <= 24 || a.river > 0.0 || b.river > 0.0, "colour jumps by {step} at {x}, {}", i as i32 - 4000);
                }
                if a.home != b.home {
                    crossings += 1;
                    // Each side knows the other is next door and that the border is near.
                    let near = a.neighbour == b.home && b.neighbour == a.home && a.edge < 12.0 && b.edge < 12.0;
                    borders += near as i32;
                } else if a.neighbour == b.neighbour && a.edge < 48.0 && b.edge < 48.0 {
                    // Within blending range, the distance to the border changes about a block
                    // per block (in steps of a 4-block cell), with rare jumps where the
                    // estimate switches between borders.
                    steps += 1;
                    jumps += ((a.edge - b.edge).abs() >= 8.0) as i32;
                }
            }
        }
        println!("{crossings} borders, {jumps} jumps in {steps} steps");
        assert!(crossings > 10, "only {crossings} borders crossed");
        assert!(jumps * 50 < steps, "the distance to the border jumps {jumps} times in {steps} steps");
        // Where three biomes meet, the nearest border may lead to a third one.
        assert!(borders * 10 >= crossings * 7, "{borders} of {crossings} borders agree on both sides");
    }

    #[test]
    fn biomes_can_be_located() {
        let world = Overworld::new(3, registry());
        for name in ["taiga", "base:plains", "ocean"] {
            let at = world.locate_biome(name, IVec2::ZERO, 6000).unwrap_or_else(|| panic!("no {name} within 6 km"));
            let id = world.biomes().id(&format!("base:{}", name.trim_start_matches("base:"))).unwrap();
            assert_eq!(world.site(at.x, at.y).home, id, "{name} at {at}");
        }
        assert_eq!(world.locate_biome("base:no_such_biome", IVec2::ZERO, 1000), None);
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
