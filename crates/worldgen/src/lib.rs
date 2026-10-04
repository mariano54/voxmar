//! Procedural world generation.
//!
//! The overworld generator runs in stages, each a pure function of the world seed and
//! position: world dials (global parameters such as mean temperature, ocean fraction and
//! wind), geology (continents, ranges, plateaus and valleys), climate (temperature and
//! precipitation as continuous fields), terrain shapes that rise with the climate (dunes,
//! mesas, karst, volcanoes), rivers and lakes, ecology (what grows where), resources (ore
//! veins) and terrain (blocks, caves, trees, plants). Biomes are data (`biomes.ron` in content
//! packages) placed over the continuous climate: they decide surface materials and what
//! grows, while temperature, moisture and the rest stay continuous underneath, and
//! `Overworld::site` exposes all of it for gameplay. Purity means any column can be
//! generated alone, in any order, on any thread, and features that cross column borders
//! (trees, veins) come out identical on both sides.

mod atmosphere;
mod biome;
mod caves;
mod climate;
mod content;
mod dials;
mod ecology;
mod features;
mod geology;
mod lattice;
mod rarity;
mod resources;
mod rng;
mod rocks;
mod shapes;
mod surface;
mod terrain;
mod trees;
mod underground;
mod water;

use std::sync::Arc;

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

pub use atmosphere::{Atmosphere, ParticleId};
pub use biome::{Biome, BiomeId, Biomes};
pub use content::{DayTime, Motion, ParticleDef, WeatherWhen};
pub use caves::{CaveStyle, LAVA_LEVEL};
pub use climate::LAPSE_RATE;
pub use content::{ContentError, WorldContent};
pub use dials::WorldDials;
pub use ecology::PlantProperties;
pub use water::{Body, Cavity, Current};
pub use rarity::{Featured, Rarity, Tier};
pub use geology::Form;
pub use rocks::ProvinceId;
pub use shapes::ShapeId;

use fastnoise_lite::{FastNoiseLite, FractalType};

use crate::biome::{GRADIENT_STEP, Pick, Point};
use crate::climate::{ClimateGrid, Regional};
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
    /// Height of the top water block of the sea, lake or river over this position, or of
    /// the river or lake nearest it: the position is under water where this is above
    /// `height`. Sea level away from rivers and lakes.
    pub water: i32,
    /// What kind of water that is.
    pub body: Body,
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
    /// The terrain shape (dunes, a volcano, karst towers) that made the ground here, if any,
    /// and how strongly.
    pub form: Form,
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
    /// Water or ice over the ground when it is submerged, by the sea, a lake or a river.
    pub water: Option<BlockId>,
    /// Height of the top water block when `water` is some: sea level, or a lake's or
    /// river's own level, which may be far above it.
    pub water_level: i32,
    /// Dominant leaves and the fraction of ground under trees.
    pub canopy: Option<(BlockId, f32)>,
    /// Temperature and humidity, as stored in `Column::climate`.
    pub climate: [u8; 2],
    /// Grass, foliage and water colours.
    pub colors: BiomeColors,
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Patches a little lighter or darker, warmer or cooler, by the palette noise `n`.
fn palette(c: [f32; 3], n: f32) -> [f32; 3] {
    [c[0] * (1.0 + 0.06 * n), c[1] * (1.0 + 0.03 * n), c[2] * (1.0 - 0.04 * n)]
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

/// A terrain shape's own ground, and its banded rock (the biome's when empty).
struct ShapeSurface {
    surface: surface::Surface,
    strata: Vec<BlockId>,
}

/// The default overworld.
pub struct Overworld {
    dials: WorldDials,
    /// The land, and the climate over it.
    geology: Geology,
    rivers: water::Rivers,
    ecology: Ecology,
    biomes: Biomes,
    atmospheres: atmosphere::Atmospheres,
    rocks: Rocks,
    rules: SurfaceRules,
    /// The ground of the terrain shapes that make their own, by shape.
    shape_surfaces: Vec<Option<ShapeSurface>>,
    caves: caves::Caves,
    underground: underground::Underground,
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
    pub const VERSION: u32 = 10;

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
        let underground = underground::Underground::new(stream, content, &registry, &biomes, &mut rules, &rocks);
        let shape_surfaces = content
            .shapes
            .iter()
            .map(|shape| {
                let surface = rules.compile(shape.surface.as_ref()?, &registry, &rocks);
                Some(ShapeSurface { surface, strata: shape.strata.iter().map(|name| registry.expect(name)).collect() })
            })
            .collect();
        Self {
            dials,
            geology: Geology::new(stream, &dials, content, &rocks),
            rivers: water::Rivers::new(stream, &dials),
            ecology: Ecology::new(stream, &dials, content, &registry),
            caves: caves::Caves::new(stream, &dials),
            underground,
            terrain: terrain::Terrain::new(stream, &registry, content, &rocks, &biomes),
            atmospheres: atmosphere::Atmospheres::new(content, &dials.featured),
            biomes,
            rocks,
            rules,
            shape_surfaces,
            blend: geology::noise(stream, 40, 1.0 / 12.0, FractalType::FBm, 2),
            tint: geology::noise(stream, 41, 1.0 / 45.0, FractalType::FBm, 2),
            seed: stream,
        }
    }

    /// The biomes of this world.
    pub fn biomes(&self) -> &Biomes {
        &self.biomes
    }

    /// What the plant with this name (`base:dandelion`) is good for: nutrition,
    /// alchemical properties, dye.
    pub fn plant_properties(&self, name: &str) -> Option<&PlantProperties> {
        self.ecology.plant_properties(name)
    }

    /// The air at (x, z): haze, sky and colour-grading nudges, and ambient particles,
    /// blended across biome borders. Cheap enough to call around the player every so often.
    pub fn atmosphere_at(&self, x: i32, z: i32) -> Atmosphere {
        let site = self.site(x, z);
        self.atmospheres.at(&site, Biomes::blend(site.edge, site.fertility))
    }

    /// The ambient particle sets [`Atmosphere::particles`] refers to, with their looks and
    /// the time of day and weather they show in.
    pub fn particle_sets(&self) -> &[ParticleDef] {
        self.atmospheres.sets()
    }

    /// The name of a terrain shape (`base:volcanoes`).
    pub fn shape_name(&self, id: ShapeId) -> &str {
        self.geology.shapes().name(id)
    }

    /// The biome whose rules make the ground at (x, z).
    pub fn biome_at(&self, x: i32, z: i32) -> &Biome {
        self.biomes.get(self.site(x, z).biome)
    }

    /// The cave biome at `pos`, if one claims it and `pos` is underground. Taken from the
    /// nearest point of the lattice generation picks on; near a border, generation dithers
    /// between it and its neighbours.
    pub fn cave_biome_at(&self, pos: IVec3) -> Option<&Biome> {
        if pos.y > self.height(pos.x, pos.z) {
            return None;
        }
        let at = underground::Underground::nearest(pos);
        let sampler = self.sampler(IVec2::new(at.x, at.z), IVec2::new(at.x, at.z));
        let (surface, province) = sampler.underground_point(at.x, at.z);
        let id = self.underground.pick(&self.biomes, at, self.height(at.x, at.z), &surface, province)?;
        Some(self.biomes.get(id))
    }

    /// The nearest place within `radius` blocks of `from` that belongs to the biome
    /// named `name` (`base:taiga`, or `taiga` for the base package's), searched every 64
    /// blocks outwards. `None` if there is no such biome or none is that close.
    pub fn locate_biome(&self, name: &str, from: IVec2, radius: i32) -> Option<IVec2> {
        let id = self.biomes.id(name).or_else(|| self.biomes.id(&format!("base:{name}")))?;
        self.biomes.locate(
            id,
            from,
            radius,
            climate::CELL,
            |at| self.point(&self.geology.climate().regional_at(&self.geology, at)),
            |at| self.sampler(at, at).pick(at.x, at.y).home == id,
        )
    }

    /// The biome picker's fields from the regional climate.
    fn point(&self, regional: &Regional) -> Point {
        self.biomes.remap(Point([
            regional.temperature(),
            regional.moisture(),
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

    /// How much of each kind of cave the world has.
    pub fn cave_style(&self) -> &CaveStyle {
        self.caves.style()
    }

    /// Surface height only; cheap enough to sample on coarse grids.
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.geo(x, z).height
    }

    fn stages(&self) -> water::Stages<'_> {
        water::Stages { geology: &self.geology, climate: self.geology.climate() }
    }

    /// The ground at one position, with its rivers and lakes.
    pub(crate) fn geo(&self, x: i32, z: i32) -> GeoSample {
        self.rivers.sample(self.stages(), x, z)
    }

    /// The ground over the `size`×`size` positions from (x, z), row by row.
    pub(crate) fn geo_area(&self, x: i32, z: i32, size: i32) -> Vec<GeoSample> {
        self.rivers.area(self.stages(), IVec2::new(x, z), size)
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
        ClimateGrid::new(&self.geology, min - margin, max + margin)
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
        let geo = self.world.geo(x, z);
        self.site_on(&geo, x, z, self.world.slope(x, z, geo.height))
    }

    /// What the surface looks like from far away; see [`Overworld::surface_sample`].
    pub fn surface(&self, x: i32, z: i32, height: i32, slope: i32) -> SurfaceSample {
        let geo = GeoSample { height, ..self.world.geo(x, z) };
        let site = self.site_on(&geo, x, z, slope);
        let (ground, water) = self.world.surface_blocks(&site, x, z);
        let canopy = if water.is_some() { None } else { self.world.ecology.canopy(&site, self.world.terrain.ground(ground), x, z) }
            .map(|(tree, cover)| (self.world.terrain.leaves(tree), cover));
        // Distant terrain takes the colours of the cell alone.
        let pick = self.pick(x, z);
        let byte = |c: [f32; 3]| c.map(|v| v.round().clamp(0.0, 255.0) as u8);
        let (climate, n) = self.climate_colors(&site, x, z);
        let [grass, foliage, water_color] = self.biome_colors(climate, n, &pick, Biomes::blend(pick.edge, site.fertility)).map(byte);
        let colors = BiomeColors { grass, foliage, water: water_color };
        SurfaceSample { height, ground, water, water_level: site.water, canopy, climate: site.tint(), colors }
    }

    /// The grass, foliage and water colours at a position inside the sampler's area,
    /// changing smoothly from block to block.
    pub fn colors(&self, x: i32, z: i32) -> BiomeColors {
        self.colors_of(&self.site(x, z), x, z)
    }

    /// The colours at a site already sampled at (x, z).
    pub(crate) fn colors_of(&self, site: &Site, x: i32, z: i32) -> BiomeColors {
        let (climate, n) = self.climate_colors(site, x, z);
        // Bilinear between the four nearest cell centres, so identity colours fade in
        // block by block rather than in steps of a cell.
        let p = IVec2::new(x, z) - biome::CELL / 2;
        let cell = p.div_euclid(IVec2::splat(biome::CELL));
        let f = (p - cell * biome::CELL).as_vec2() / biome::CELL as f32;
        let mut sum = [[0.0f32; 3]; 3];
        for (dx, dz, w) in [(0, 0, (1.0 - f.x) * (1.0 - f.y)), (1, 0, f.x * (1.0 - f.y)), (0, 1, (1.0 - f.x) * f.y), (1, 1, f.x * f.y)] {
            let at = (cell + IVec2::new(dx, dz)) * biome::CELL;
            let pick = self.pick(at.x, at.y);
            let colors = self.biome_colors(climate, n, &pick, Biomes::blend(pick.edge, site.fertility));
            for (total, c) in sum.iter_mut().zip(colors) {
                for k in 0..3 {
                    total[k] += w * c[k];
                }
            }
        }
        let byte = |c: [f32; 3]| c.map(|v| v.round().clamp(0.0, 255.0) as u8);
        BiomeColors { grass: byte(sum[0]), foliage: byte(sum[1]), water: byte(sum[2]) }
    }

    /// The climate's colours at a site (grass, foliage, water), and the palette noise there.
    fn climate_colors(&self, site: &Site, x: i32, z: i32) -> ([[f32; 3]; 3], f32) {
        let [t, h] = site.tint().map(|c| c as f32 / 255.0);
        // Blue-green when cold, olive when hot and dry, deep green when hot and wet.
        let grass = mix3([128.0, 167.0, 140.0], mix3([191.0, 183.0, 85.0], [71.0, 176.0, 26.0], h), t);
        let foliage = mix3([97.0, 140.0, 110.0], mix3([174.0, 164.0, 42.0], [48.0, 150.0, 18.0], h), t);
        let water = mix3([61.0, 87.0, 214.0], [67.0, 213.0, 238.0], t * (0.4 + 0.6 * h));
        let n = self.world.tint.get_noise_2d(x as f32, z as f32);
        ([palette(grass, n), palette(foliage, n), water], n)
    }

    /// Colours from the climate's and the biomes of `pick`, the neighbour's taking `share`:
    /// grass, foliage and water.
    fn biome_colors(&self, climate: [[f32; 3]; 3], n: f32, pick: &Pick, share: f32) -> [[f32; 3]; 3] {
        let own = |biome: BiomeId| {
            let colors = &self.world.biomes.get(biome).colors;
            let mut own = climate;
            for (c, set) in own.iter_mut().zip([colors.grass, colors.foliage, colors.water]) {
                if let Some((r, g, b)) = set {
                    *c = palette([r as f32, g as f32, b as f32], n);
                }
            }
            own
        };
        let (home, neighbour) = (own(pick.home), own(pick.neighbour));
        std::array::from_fn(|i| mix3(home[i], neighbour[i], share))
    }

    /// The biome picker's fields and the rock province at (x, z), for cave biomes.
    pub(crate) fn underground_point(&self, x: i32, z: i32) -> (Point, ProvinceId) {
        let regional = self.grid.at(x, z);
        (self.world.point(&regional), self.world.rocks.at(x, z, regional.warp))
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
        // Cooled by the height of the land, not of the towers and spires standing on it.
        let temperature = regional.sea_temperature - LAPSE_RATE * (geo.climate_height() - SEA_LEVEL).max(0) as f32;
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
            water: geo.water,
            body: geo.body,
            vegetation: 0.0,
            forest: 0.0,
            province: self.world.rocks.at(x, z, regional.warp),
            biome: BiomeId(0),
            home: BiomeId(0),
            neighbour: BiomeId(0),
            edge: 0.0,
            from_centre: 0.0,
            form: geo.form,
        };
        let pick = self.pick(x, z);
        let share = Biomes::blend(pick.edge, regional.fertility);
        if !geo.submerged() {
            site.vegetation = ecology::ground_cover(temperature, site.soil_moisture());
            site.forest = self.world.ecology.forest(x, z, pick.home, pick.neighbour, share, regional.fertility);
        }
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

    /// Rivers are traced region by region and cached; nothing may depend on which column,
    /// region or thread asks first, or on what the caches hold.
    #[test]
    fn rivers_never_depend_on_generation_order() {
        let registry = registry();
        // Around a creek with waterfalls and a confluence on seed 42, across a region border.
        let columns: Vec<ColumnPos> = [(-10, -3), (-9, -2), (-10, -2), (-1, 0), (0, 0), (-33, -2)].map(|(x, z)| ColumnPos::new(x, z)).to_vec();
        let reference: Vec<Column> = {
            let world = Overworld::new(42, registry.clone());
            columns.iter().map(|&pos| world.generate(pos)).collect()
        };
        // Backwards, after warming the caches far away, on several threads at once.
        let world = Overworld::new(42, registry.clone());
        world.generate(ColumnPos::new(150, -90));
        let results: Vec<(usize, Column)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..3)
                .map(|t| {
                    let (world, columns) = (&world, &columns);
                    scope.spawn(move || columns.iter().enumerate().rev().filter(|(i, _)| i % 3 == t).map(|(i, &pos)| (i, world.generate(pos))).collect::<Vec<_>>())
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        for (i, column) in results {
            assert!(column == reference[i], "column {:?} differs with another generation order", columns[i]);
        }
    }

    /// `cargo test --release -p voxmar_worldgen shape_sights -- --ignored --nocapture`
    /// (optionally `SEED=n` and `RANGE=blocks`, 3000 by default) finds the best example of
    /// each terrain shape near spawn, and the highest peak, and prints capture arguments
    /// that look at each from a little way off: a `/tp` to stand there and a `--pose`.
    #[test]
    #[ignore]
    fn shape_sights() {
        let env = |name: &str, default: i64| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
        let seed = env("SEED", 1) as u64;
        let range = env("RANGE", 3000) as i32;
        let world = Overworld::new(seed, registry());
        let spawn = world.spawn_point();
        let shapes = world.geology.shapes();
        // (score, x, z, height) per shape, and the highest peak.
        let mut best: Vec<(f32, i32, i32, f32)> = vec![(0.0, 0, 0, 0.0); 256];
        let mut peak = (0.0f32, 0, 0);
        for dz in (-range..=range).step_by(12) {
            for dx in (-range..=range).step_by(12) {
                let (x, z) = (spawn.x + dx, spawn.z + dz);
                let land = world.geology.land(x, z);
                let near = 1.0 / (1.0 + ((dx * dx + dz * dz) as f32).sqrt() / 1500.0);
                if land.height > peak.0 {
                    peak = (land.height, x, z);
                }
                if let Some(id) = land.form.shape {
                    let score = land.form.strength.min(1.0) * near;
                    let slot = &mut best[id.0 as usize];
                    if score > slot.0 {
                        *slot = (score, x, z, land.height);
                    }
                }
            }
        }
        println!("seed {seed}, spawn {spawn}, volcanism {:.2}", world.dials().volcanism);
        // A camera `back` blocks from (x, z) towards `direction` (radians from +x), `raise`
        // above the top or clear of the ground and any water there, looking at the top.
        let view = |name: &str, x: i32, z: i32, top: f32, back: f32, direction: f32, raise: f32| {
            let target = voxmar_engine::glam::Vec3::new(x as f32, top, z as f32);
            let (cx, cz) = ((x as f32 + back * direction.cos()) as i32, (z as f32 + back * direction.sin()) as i32);
            let geo = world.geo(cx, cz);
            let eye = geo.height as f32 + 1.0 + 1.62;
            let camera_y = (top + raise).max(geo.height.max(geo.water) as f32 + 6.0);
            let d = target - voxmar_engine::glam::Vec3::new(cx as f32, camera_y, cz as f32);
            let yaw = (-d.x).atan2(-d.z).to_degrees();
            let pitch = d.y.atan2(d.x.hypot(d.z)).to_degrees();
            println!("{name} at {x},{top:.0},{z}: --command \"/tp {cx} {cz}\" --pose 0,{:.0},0,{yaw:.0},{pitch:.0}", camera_y - eye);
        };
        // The range from afar: from the dry lowland, about a kilometre off, that sees most
        // of it.
        let lowland = (0..16)
            .map(|k| k as f32 * std::f32::consts::TAU / 16.0)
            .filter_map(|a| {
                let (cx, cz) = ((peak.1 as f32 + 900.0 * a.cos()) as i32, (peak.2 as f32 + 900.0 * a.sin()) as i32);
                let geo = world.geo(cx, cz);
                (!geo.submerged() && geo.height > SEA_LEVEL).then_some((geo.height, a))
            })
            .min_by_key(|&(h, _)| h);
        if let Some((ground, a)) = lowland {
            view("range from afar", peak.1, peak.2, peak.0, 900.0, a, ground as f32 + 40.0 - peak.0);
        }
        view("highest peak", peak.1, peak.2, peak.0, 600.0, std::f32::consts::FRAC_PI_4, 150.0);
        for (i, &(score, x, z, height)) in best.iter().enumerate().filter(|(_, b)| b.0 > 0.0) {
            let name = shapes.name(ShapeId(i as u8));
            let back = if name.contains("volcano") { 420.0 } else if name.contains("mesa") || name.contains("canyon") { 160.0 } else { 70.0 };
            print!("score {score:.2}: ");
            view(name, x, z, height, back, std::f32::consts::FRAC_PI_4, back * 0.25);
        }
    }

    /// `cargo test --release -p voxmar_worldgen river_sights -- --ignored --nocapture`
    /// finds a highland lake, the tallest waterfall, a confluence, a river mouth, a river
    /// tunnel, an island in a river and a hot spring near spawn and prints `--pose` arguments that look at each, for in-game
    /// captures.
    #[test]
    #[ignore]
    fn river_sights() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        let range: i32 = std::env::var("RANGE").ok().and_then(|s| s.parse().ok()).unwrap_or(420);
        let world = Overworld::new(seed, registry());
        let spawn = world.spawn_point();
        let eye = spawn.as_vec3() + IVec3::Y.as_vec3() * 1.62;
        let step = 2;
        let n = 2 * range / step + 1;
        let geos: Vec<GeoSample> = (0..n * n).map(|i| world.geo(spawn.x - range + i % n * step, spawn.z - range + i / n * step)).collect();
        let at = |i: i32, j: i32| geos[(j.clamp(0, n - 1) * n + i.clamp(0, n - 1)) as usize];
        // (score, x, z, y)
        let mut best: [(f32, i32, i32, i32); 7] = [(0.0, 0, 0, 0); 7];
        for j in 1..n - 1 {
            for i in 1..n - 1 {
                let g = at(i, j);
                let (x, z) = (spawn.x - range + i * step, spawn.z - range + j * step);
                let near = 1.0 / (1.0 + (((x - spawn.x).pow(2) + (z - spawn.z).pow(2)) as f32).sqrt() / 200.0);
                let mut consider = |k: usize, score: f32| {
                    if score * near > best[k].0 {
                        best[k] = (score * near, x, z, g.water);
                    }
                };
                if g.cavity.is_some() {
                    consider(4, 1.0);
                }
                let river = |o: GeoSample| o.submerged() && o.body == Body::River;
                if g.submerged() && g.body == Body::Spring {
                    consider(6, 1.0);
                }
                if !g.submerged() && g.body == Body::River && (river(at(i - 2, j)) && river(at(i + 2, j)) || river(at(i, j - 2)) && river(at(i, j + 2))) {
                    consider(5, 1.0);
                }
                if !g.submerged() {
                    continue;
                }
                if g.body == Body::Lake && g.water > SEA_LEVEL + 20 {
                    consider(0, (g.water - SEA_LEVEL) as f32);
                }
                // Steps of two samples, so a fall is found from its top.
                let drop = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|&(di, dj)| at(i + di, j + dj)).filter(|o| o.submerged()).map(|o| g.water - o.water).max().unwrap_or(0);
                if drop >= 3 && g.body != Body::Lake {
                    consider(1, (drop * drop) as f32);
                }
                let channel = |o: GeoSample| o.submerged() && matches!(o.body, Body::Creek | Body::River);
                if channel(g) {
                    // Arms of water crossing a ring around the point: three or more meet here.
                    let ring: Vec<bool> = (0..32)
                        .map(|k| {
                            let a = k as f32 / 32.0 * std::f32::consts::TAU;
                            channel(at(i + (a.cos() * 5.0).round() as i32, j + (a.sin() * 5.0).round() as i32))
                        })
                        .collect();
                    let arms = (0..32).filter(|&k| ring[k] && !ring[(k + 31) % 32]).count();
                    if arms >= 3 && ring.iter().filter(|&&w| w).count() < 16 {
                        consider(2, arms as f32 + if g.body == Body::River { 2.0 } else { 0.0 });
                    }
                    let sea = (-6..=6).flat_map(|dj| (-6..=6).map(move |di| (di, dj))).filter(|&(di, dj)| at(i + di, j + dj).body == Body::Sea && at(i + di, j + dj).submerged()).count();
                    if g.water == SEA_LEVEL && sea > 0 {
                        consider(3, sea as f32 + if g.body == Body::River { 20.0 } else { 0.0 });
                    }
                }
            }
        }
        println!("seed {seed}, spawn {spawn}");
        for (name, (score, x, z, y)) in ["highland lake", "waterfall", "confluence", "river mouth", "river tunnel", "river island", "hot spring"].into_iter().zip(best) {
            if score == 0.0 {
                println!("{name}: none within {range}");
                continue;
            }
            // Stand back and above, looking at it from the south-east, clear of the ground.
            let back: f32 = std::env::var("BACK").ok().and_then(|s| s.parse().ok()).unwrap_or(16.0);
            let target = IVec3::new(x, y, z).as_vec3();
            let mut camera = target + voxmar_engine::glam::Vec3::new(back, back * 0.6, back);
            camera.y = camera.y.max(world.height(camera.x as i32, camera.z as i32) as f32 + 5.0);
            let d = target - camera;
            let yaw = (-d.x).atan2(-d.z).to_degrees();
            let pitch = d.y.atan2(d.x.hypot(d.z)).to_degrees();
            let o = camera - eye;
            println!("{name} at {x},{y},{z}: --pose {:.0},{:.0},{:.0},{yaw:.0},{pitch:.0}", o.x, o.y, o.z);
        }
    }

    /// Generated water and lava stay put: no fluid touches air below it, nor beside it
    /// except the air over lower water that a waterfall drops into; water never touches
    /// lava, and lava lies only at the bottom of the world. Checked across column borders,
    /// so it also shows the columns agree with each other, and around rivers.
    #[test]
    fn fluids_are_contained() {
        let registry = registry();
        let (water, lava) = (registry.expect("base:water"), registry.expect("base:lava"));
        let (mut fluids, mut caves) = (0, 0);
        for (seed, place) in [(1, (0, 0)), (42, (3000, -2000)), (0x5eed, (-4100, 900)), (7, (1500, 5200))] {
            let world = Overworld::new(seed, registry.clone());
            // Some coast too, where caves meet the sea.
            let coast = (0..4000).map(|i| (place.0 + i % 64 * 48, place.1 + i / 64 * 48)).find(|&(x, z)| (world.height(x, z) - SEA_LEVEL).abs() <= 2).unwrap();
            // Creeks with waterfalls and confluences, and a highland lake.
            let rivers: &[(i32, i32)] = match seed {
                1 => &[(10, 88), (-20, 160)],
                42 => &[(-306, -70)],
                _ => &[],
            };
            for (x, z) in [place, (place.0 + 700, place.1 + 300), (place.0 - 900, place.1 - 600), coast].into_iter().chain(rivers.iter().copied()) {
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
                                // The face of a waterfall: air beside water, over lower water
                                // (which may be frozen over).
                                let fall = here == water
                                    && d.y == 0
                                    && next.is_some_and(|b| b.is_air())
                                    && (0..y)
                                        .rev()
                                        .filter_map(|y| block((p + d).with_y(y)))
                                        .find(|b| !b.is_air())
                                        .is_some_and(|b| b == water || registry.def(b).name.ends_with("ice"));
                                assert!(fall || next.is_none_or(|b| !b.is_air() && b != other), "seed {seed}: {} at {p} beside {} at {}", registry.def(here).name, registry.def(next.unwrap()).name, p + d);
                            }
                        }
                    }
                }
            }
        }
        assert!(fluids > 0 && caves > 0);
    }

    /// Cave biomes line and decorate their caves: their blocks turn up underground, and no
    /// decoration floats with air on every side. (Linings may: the caves leave the odd
    /// lone block of rock.)
    #[test]
    fn caves_are_decorated() {
        let registry = registry();
        let decorations: Vec<BlockId> =
            ["glowstone", "sea_lantern", "mangrove_roots", "azalea_leaves", "brown_mushroom", "moss_block", "blue_ice"].map(|n| registry.expect(&format!("base:{n}"))).to_vec();
        // The last two also line caves or make up their rock.
        let decoration_only = decorations.len() - 2;
        let mut found = vec![0; decorations.len()];
        for (seed, place) in [(1, (0, 0)), (7, (200, 150)), (42, (-300, -100))] {
            let world = Overworld::new(seed, registry.clone());
            let first = ColumnPos::of_block(IVec3::new(place.0, 0, place.1)).offset(-4, -4);
            let columns = generate_area(&world, first, 8);
            let origin = first.origin();
            let block = |p: IVec3| {
                let (cx, cz) = ((p.x - origin.x) >> 5, (p.z - origin.z) >> 5);
                columns[(cz * 8 + cx) as usize].block(IVec3::new(p.x & 31, p.y, p.z & 31))
            };
            // Inside the area, leaving a block's margin for the neighbours read.
            for z in origin.z + 1..origin.z + 255 {
                for x in origin.x + 1..origin.x + 255 {
                    let height = world.height(x, z);
                    for y in 6..height - 3 {
                        let p = IVec3::new(x, y, z);
                        let Some(i) = decorations.iter().position(|&d| d == block(p)) else { continue };
                        found[i] += 1;
                        if i >= decoration_only {
                            continue;
                        }
                        let sides = [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z];
                        assert!(sides.iter().any(|&d| !block(p + d).is_air()), "seed {seed}: {} floats at {p}", registry.def(block(p)).name);
                    }
                }
            }
        }
        assert!(found.iter().filter(|&&n| n > 0).count() >= 5, "{found:?}");
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

        // The cave biomes of the same cuts, a pixel every 8 blocks, a colour per biome.
        const STEP: i32 = 8;
        const PALETTE: [[f32; 3]; 14] = [
            [0.45, 0.45, 0.45], [0.85, 0.55, 0.25], [0.35, 0.8, 0.25], [0.6, 0.4, 0.2], [0.75, 0.9, 1.0], [0.95, 0.25, 0.1], [0.95, 0.4, 0.7],
            [0.3, 0.5, 1.0], [0.1, 0.75, 0.75], [0.1, 0.1, 0.3], [0.9, 0.9, 0.2], [1.0, 1.0, 1.0], [0.6, 0.2, 0.8], [0.2, 0.4, 0.2],
        ];
        let underground: Vec<&str> = world.biomes().iter().filter(|(_, b)| b.layer == content::Layer::Underground).map(|(_, b)| b.name.as_str()).collect();
        let colour = |name: &str| PALETTE[underground.iter().position(|n| *n == name).unwrap_or(0) % PALETTE.len()];
        let mut seen: Vec<String> = Vec::new();
        let mut map = Vec::new();
        for z in (origin.z..origin.z + SIZE).step_by(STEP as usize) {
            for y in levels {
                for x in (origin.x..origin.x + SIZE).step_by(STEP as usize) {
                    map.push(match world.cave_biome_at(IVec3::new(x, y, z)) {
                        Some(biome) => {
                            if !seen.contains(&biome.name) {
                                seen.push(biome.name.clone());
                            }
                            colour(&biome.name)
                        }
                        None => [0.0, 0.0, 0.0],
                    });
                }
            }
        }
        write("caves_biomes.ppm", 2 * SIZE / STEP, SIZE / STEP, &map);
        for name in seen {
            println!("{name}: {:?}", colour(&name).map(|c| (c * 255.0) as u8));
        }
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

            // Cave biomes: the share of open cave each takes, and the best view of each:
            // the most open space around, with light near it, every 4 blocks across the area.
            let origin = first.origin();
            let block = |p: IVec3| {
                let (cx, cz) = ((p.x - origin.x) >> 5, (p.z - origin.z) >> 5);
                let inside = (0..19).contains(&cx) && (0..19).contains(&cz) && (0..voxmar_engine::coords::WORLD_HEIGHT).contains(&p.y);
                inside.then(|| columns[(cz * 19 + cx) as usize].block(IVec3::new(p.x & 31, p.y, p.z & 31)))
            };
            let lights: Vec<BlockId> = ["base:glowstone", "base:sea_lantern", "base:lava"].iter().filter_map(|n| registry.id(n)).collect();
            let air = |p: IVec3| block(p).is_some_and(|b| b.is_air());
            let mut biomes: Vec<(String, u64, (i32, IVec3, f32))> = Vec::new();
            let mut total = 0;
            for (i, column) in columns.iter().enumerate() {
                let corner = first.offset(i as i32 % 19, i as i32 / 19).origin();
                for (x, z) in (0..8).flat_map(|a| (0..8).map(move |b| (a * 4 + 1, b * 4 + 2))) {
                    let height = world.height(corner.x + x, corner.z + z);
                    let mut run = 0;
                    for y in (4..height).rev() {
                        if column.block(IVec3::new(x, y, z)).is_air() {
                            run += 1;
                            continue;
                        }
                        if run >= 4 {
                            let camera = corner + IVec3::new(x, y + 3, z);
                            let name = world.cave_biome_at(camera).map_or("none".to_owned(), |b| b.name.clone());
                            total += run as u64;
                            let i = biomes.iter().position(|b| b.0 == name).unwrap_or_else(|| {
                                biomes.push((name, 0, (i32::MIN, IVec3::ZERO, 0.0)));
                                biomes.len() - 1
                            });
                            biomes[i].1 += run as u64;
                            // The longest view along eight directions, and light within it.
                            let (mut view, mut yaw) = (0, 0.0f32);
                            for d in 0..8 {
                                let a = d as f32 * std::f32::consts::FRAC_PI_4;
                                let dir = voxmar_engine::glam::Vec2::new(a.cos(), a.sin());
                                let length = (1..32).take_while(|&k| {
                                    let step = (dir * k as f32).round().as_ivec2();
                                    air(camera + IVec3::new(step.x, 0, step.y))
                                }).count() as i32;
                                if length > view {
                                    (view, yaw) = (length, (-dir.x).atan2(-dir.y).to_degrees());
                                }
                            }
                            let lit = (-8..=8)
                                .flat_map(|dz| (-6..=6).flat_map(move |dy| (-8..=8).map(move |dx| IVec3::new(dx, dy, dz))))
                                .filter(|&d| block(camera + d).is_some_and(|b| lights.contains(&b)))
                                .count() as i32;
                            let score = view + run.min(20) + 12 * lit.min(4);
                            if score > biomes[i].2.0 {
                                biomes[i].2 = (score, camera, yaw);
                            }
                        }
                        run = 0;
                    }
                }
            }
            biomes.sort_by_key(|b| std::cmp::Reverse(b.1));
            for (name, count, (score, camera, yaw)) in biomes {
                let d = camera.as_vec3() + 0.5 - eye;
                let share = 100.0 * count as f64 / total.max(1) as f64;
                println!("  {name}: {share:.1}% of caves; view {score} at {camera}: --pose {:.1},{:.1},{:.1},{yaw:.0},-12", d.x, d.y, d.z);
            }
        }
    }

    const WORLD_HEIGHT_FOR_TESTS: i32 = voxmar_engine::coords::WORLD_HEIGHT;

    /// `cargo test --release -p voxmar_worldgen map_preview -- --ignored --nocapture`
    /// writes a shaded top-down map (north up, 8 blocks per pixel) around the origin to
    /// `target/map_preview.ppm`, for judging the climate layout at a glance, and prints
    /// each biome's share of the map and how many biomes lie within 5 km of spawn.
    ///
    /// - `SEED=n`, or `SEED=1,2,3` for several maps (`map_preview_<seed>.ppm`);
    /// - `BIOMES=1` colours land by biome (a colour per biome, from its name);
    /// - `RELIEF=1` colours land by height, green lowland to white peaks, and `SHAPES=1`
    ///   tints it by the terrain shape that made it (a colour per shape, from its name);
    /// - `FIELD=erosion` (or `continentalness`, `weirdness`, `aether`, `fertility`,
    ///   `variant`) shows that selection field on land, dark to light;
    /// - `X=x Z=z` centre the map elsewhere and `SCALE=s` sets the blocks per pixel.
    ///
    /// The sea is deep blue, lakes above the sea turquoise, rivers bright blue and creeks
    /// light blue (found by sampling inside each pixel, since they are narrower than one),
    /// hot springs orange and dry salt flats white.
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

    /// A colour for land `h` high: green lowland, tan hills, brown and grey ranges, white
    /// peaks.
    fn hypsometric(h: i32) -> [f32; 3] {
        const STOPS: [(f32, [f32; 3]); 7] = [
            (0.0, [0.33, 0.55, 0.28]),
            (25.0, [0.55, 0.68, 0.36]),
            (60.0, [0.8, 0.75, 0.47]),
            (100.0, [0.66, 0.52, 0.35]),
            (150.0, [0.52, 0.44, 0.4]),
            (190.0, [0.74, 0.72, 0.71]),
            (225.0, [0.98, 0.98, 0.99]),
        ];
        let a = (h - SEA_LEVEL) as f32;
        let i = STOPS.iter().position(|s| s.0 > a).unwrap_or(STOPS.len()).clamp(1, STOPS.len() - 1);
        let ((a0, c0), (a1, c1)) = (STOPS[i - 1], STOPS[i]);
        let t = ((a - a0) / (a1 - a0)).clamp(0.0, 1.0);
        std::array::from_fn(|k| c0[k] + (c1[k] - c0[k]) * t)
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
            Ok("mountain") => Some(|s| s.mountain),
            _ => None,
        };
        let flag = |name: &str| std::env::var(name).is_ok_and(|v| v != "0");
        let (by_biome, by_height, by_shape) = (flag("BIOMES"), flag("RELIEF"), flag("SHAPES"));
        const SIZE: i32 = 1024;
        let env = |name: &str, default: i32| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
        let scale = env("SCALE", 8);
        let centre = IVec2::new(env("X", 0), env("Z", 0));
        let registry = registry();
        let world = Overworld::new(seed, registry.clone());
        let half = SIZE * scale / 2;
        let sampler = world.sampler(centre - half - scale, centre + half + scale);
        let (ax, az) = (|i: i32| centre.x + i * scale - half, |i: i32| centre.y + i * scale - half);
        let heights: Vec<i32> = (0..SIZE * SIZE).map(|i| world.height(ax(i % SIZE), az(i / SIZE))).collect();
        let height = |x: i32, z: i32| heights[(z.clamp(0, SIZE - 1) * SIZE + x.clamp(0, SIZE - 1)) as usize];
        let color = |block: BlockId| registry.def(block).color.map(|c| c as f32 / 255.0);
        let spawn = world.spawn_point();
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
        let (mut wet, mut creeks, mut rivers, mut lakes) = (0, 0, 0, 0);
        let mut area = vec![0usize; world.biomes.len()];
        let mut shaped = vec![0usize; 256];
        let mut land_heights = Vec::new();
        let mut near_spawn = vec![false; world.biomes.len()];
        for z in 0..SIZE {
            for x in 0..SIZE {
                let h = height(x, z);
                let slope = (height(x + 1, z) - h).abs().max((height(x, z + 1) - h).abs()) / scale;
                let surface = sampler.surface(ax(x), az(z), h, slope);
                let site = sampler.site(ax(x), az(z));
                area[site.biome.0 as usize] += 1;
                if (ax(x) - spawn.x).pow(2) + (az(z) - spawn.z).pow(2) < 5000 * 5000 {
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
                if by_height {
                    rgb = hypsometric(h);
                }
                if let Some(shape) = site.form.shape {
                    shaped[shape.0 as usize] += 1;
                    if by_shape {
                        rgb = mix(rgb, biome_color(world.shape_name(shape)), 0.35 + 0.5 * site.form.strength.min(1.0));
                    }
                }
                if h >= SEA_LEVEL {
                    land_heights.push(h);
                }
                if let Some(field) = field {
                    let v = field(&site);
                    rgb = [0.1 + 0.9 * v, 0.1 + 0.8 * v * v, 0.3 + 0.4 * v];
                }
                // Narrow water between the samples: look inside the pixel.
                let geo = world.geo(ax(x), az(z));
                let mut body = geo.submerged().then_some(geo.body);
                if body.is_none() && geo.river > 0.2 {
                    let step = (scale / 3).max(1);
                    body = (-1..=1)
                        .flat_map(|j| (-1..=1).map(move |i| (i, j)))
                        .map(|(i, j)| world.geo(ax(x) + i * step, az(z) + j * step))
                        .find(|g| g.submerged())
                        .map(|g| g.body);
                }
                if let Some(body) = body {
                    wet += 1;
                    let depth = ((surface.water_level - h) as f32 / 30.0).clamp(0.0, 1.0);
                    let ice = surface.water == Some(registry.expect("base:ice"));
                    let color = match body {
                        _ if ice => [0.75, 0.85, 0.95],
                        Body::Creek => [0.45, 0.75, 1.0],
                        Body::River => [0.2, 0.55, 1.0],
                        Body::Spring => [0.95, 0.6, 0.2],
                        Body::Lake if surface.water_level > SEA_LEVEL => [0.1, 0.6 - 0.2 * depth, 0.75 - 0.2 * depth],
                        _ => [0.15, 0.35 - 0.15 * depth, 0.7 - 0.25 * depth],
                    };
                    match body {
                        Body::Creek => creeks += 1,
                        Body::River => rivers += 1,
                        Body::Lake if surface.water_level > SEA_LEVEL => lakes += 1,
                        _ => {}
                    }
                    rgb = if by_biome { mix(rgb, color, 0.6) } else { color };
                } else if geo.body == Body::Salt {
                    rgb = [0.95, 0.93, 0.88];
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
        println!("pixels with creeks {creeks}, rivers {rivers}, lakes above the sea {lakes}");
        let mut shares: Vec<(usize, &str)> = area.iter().enumerate().map(|(i, &a)| (a, world.biomes.get(BiomeId(i as u16)).name.as_str())).collect();
        shares.sort_by_key(|s| std::cmp::Reverse(s.0));
        for (a, name) in shares {
            let [r, g, b] = biome_color(name).map(|c| (c * 255.0) as u8);
            println!("  {:6.2}%  {name}  #{r:02x}{g:02x}{b:02x}", a as f32 * 100.0 / (SIZE * SIZE) as f32);
        }
        land_heights.sort_unstable();
        let pct = |p: f32| land_heights.get(((land_heights.len() as f32 - 1.0) * p) as usize).map_or(0, |h| h - SEA_LEVEL);
        println!(
            "land above the sea: median {}, 90% {}, 99% {}, 99.9% {}, highest {}",
            pct(0.5),
            pct(0.9),
            pct(0.99),
            pct(0.999),
            pct(1.0)
        );
        for (i, &count) in shaped.iter().enumerate().filter(|(_, c)| **c > 0) {
            println!("  {:6.2}%  shaped by {}", count as f32 * 100.0 / (SIZE * SIZE) as f32, world.shape_name(ShapeId(i as u8)));
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

    /// `cargo test --release -p voxmar_worldgen river_speed -- --ignored --nocapture`
    /// separates the two costs rivers add, in CPU time on one thread: building a region's
    /// rivers (once, for its 32×32 columns) and generating columns once it is built.
    #[test]
    #[ignore]
    fn river_speed() {
        let registry = registry();
        let (mut first, mut warm, mut columns) = (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        for seed in [1, 3, 0x5eed] {
            let world = Overworld::new(seed, registry.clone());
            let start = ColumnPos::of_block(world.spawn_point());
            let t = thread_time();
            std::hint::black_box(world.generate(start));
            first += thread_time() - t;
            let t = thread_time();
            for i in 0..100 {
                std::hint::black_box(world.generate(start.offset(i % 10, i / 10)));
            }
            warm += thread_time() - t;
            columns += 100;
        }
        println!("first column (cold caches) {:.0} ms, then {:.3} ms per column", first.as_secs_f64() * 1e3 / 3.0, warm.as_secs_f64() * 1e3 / columns as f64);
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
                site.temperature + LAPSE_RATE * (site.height.min(site.form.base.floor() as i32) - SEA_LEVEL).max(0) as f32
            };
            let temperatures: Vec<f32> = (-4000..=4000).step_by(8).map(sea_level).collect();
            let steepest = temperatures.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
            assert!(steepest < 0.3, "temperature jumps {steepest} °C over 8 blocks at x {x}");
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
    fn the_air_follows_climate_and_biome() {
        // Deep in a dry, ordinary desert, low and away from water (high deserts are cooler
        // and fantastical ones violet), in the first of these worlds with one near the origin.
        let (world, desert) = [3, 7, 42]
            .into_iter()
            .find_map(|seed| {
                let world = Overworld::new(seed, registry());
                let id = world.biomes().id("base:desert").unwrap();
                let sampler = world.sampler(IVec2::splat(-4000), IVec2::splat(4000));
                let desert = (0..200 * 200).map(|i| IVec2::new(i % 200 * 40 - 4000, i / 200 * 40 - 4000)).find(|p| {
                    let site = sampler.site(p.x, p.y);
                    site.home == id && site.edge > 40.0 && site.soil_moisture() < 0.3 && site.height < SEA_LEVEL + 40 && site.aether < 0.6
                })?;
                Some((world, desert))
            })
            .expect("a desert");
        let set = |name: &str| world.particle_sets().iter().position(|p| p.name == name).map(|i| ParticleId(i as u16)).unwrap();
        let has = |air: &Atmosphere, name: &str| air.particles.iter().any(|&(p, d)| p == set(name) && d > 0.0);
        let air = world.atmosphere_at(desert.x, desert.y);
        assert_eq!(air, world.atmosphere_at(desert.x, desert.y));
        assert!(has(&air, "base:dust") && !has(&air, "base:drifting_snow"), "{air:?}");
        assert!(air.haze[0] > air.haze[2], "desert haze should be dusty: {air:?}");
        let tundra = world.locate_biome("snowy_tundra", IVec2::ZERO, 8000).unwrap();
        assert!(has(&world.atmosphere_at(tundra.x, tundra.y), "base:drifting_snow"));
        // Smooth along a line: no colour or density jumps between neighbouring blocks.
        let line: Vec<Atmosphere> = (0..600).map(|i| world.atmosphere_at(-1500 + i, 900)).collect();
        for pair in line.windows(2) {
            let step = (0..3).map(|c| (pair[0].haze[c] - pair[1].haze[c]).abs()).fold(0.0, f32::max);
            assert!(step < 0.05 && (pair[0].haze_density - pair[1].haze_density).abs() < 0.1, "{pair:?}");
        }
    }

    /// Shapes rise only inside their climate boxes, read off the same bands as the biome
    /// table; features never cool the climate; nothing rises past the ceiling.
    #[test]
    fn shapes_follow_the_climate_and_not_the_other_way() {
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let axes = [content::Axis::Temperature, content::Axis::Moisture, content::Axis::Continentalness, content::Axis::Erosion];
        let (mut shaped, mut features, mut highest) = (vec![0; content.shapes.len()], 0, 0.0f32);
        for seed in [3, 11] {
            let world = Overworld::new(seed, registry.clone());
            let shapes = world.geology.shapes();
            for i in 0..160 * 160 {
                let (x, z) = (i % 160 * 50 - 4000, i / 160 * 50 - 4000);
                let land = world.geology.land(x, z);
                highest = highest.max(land.height);
                let Some(id) = land.form.shape else { continue };
                shaped[id.0 as usize] += 1;
                let def = content.shapes.iter().find(|s| s.name == shapes.name(id)).unwrap();
                // Those that stand alone weigh the climate at their centre, and their flanks
                // may reach past it.
                if matches!(def.kind, content::ShapeKind::Volcanoes { .. } | content::ShapeKind::Mountain { .. }) {
                    continue;
                }
                let regional = world.geology.regional(x, z);
                let fields = [regional.temperature(), regional.moisture(), regional.continentalness, regional.erosion];
                let inside = def.climate.is_empty()
                    || def.climate.iter().any(|b| {
                        axes.iter().zip(fields).all(|(&axis, v)| {
                            let (lo, hi) = content.span(axis, b.bands(axis));
                            (lo..=hi).contains(&v)
                        })
                    });
                assert!(inside, "seed {seed}: {} at {x}, {z} outside its climate: {fields:?}", def.name);
                // A tower or spire leaves the climate at the height of the land beneath it.
                if land.height > land.form.base + 15.0 {
                    features += 1;
                    let site = world.site(x, z);
                    if site.height > site.form.base as i32 + 15 && !site.form.base.is_nan() {
                        let cooled = regional.sea_temperature - LAPSE_RATE * (site.form.base.floor() - SEA_LEVEL as f32).max(0.0);
                        assert!((site.temperature - cooled).abs() < 1e-3, "seed {seed}: {} at {x}, {z} cooled by its own height", def.name);
                    }
                }
            }
        }
        println!("{shaped:?}, {features} features, highest {highest}");
        assert!(shaped.iter().filter(|&&n| n > 0).count() >= 5, "few shapes rose: {shaped:?}");
        assert!(features > 0, "no towers or spires");
        assert!(highest < (voxmar_engine::coords::WORLD_HEIGHT - 16) as f32, "land reaches {highest}");
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
