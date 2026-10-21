//! Procedural world generation.
//!
//! The overworld generator runs in stages, each a pure function of the world seed and
//! position: world dials (global parameters such as mean temperature, ocean fraction and
//! wind), geology (continents, ranges, plateaus and valleys, from the land's router and the
//! splines of `land.ron`), climate (temperature and precipitation as continuous fields),
//! terrain shapes that rise with the climate (dunes, mesas, karst, volcanoes), rivers and
//! lakes, ecology (what grows where), ore veins and terrain (blocks, caves,
//! trees, plants). Biomes are data (`biomes.ron` in content packages) placed over the
//! continuous climate: they decide surface materials and what
//! grows, while temperature, moisture and the rest stay continuous underneath, and
//! `Overworld::site` exposes all of it for gameplay. Purity means any column can be
//! generated alone, in any order, on any thread, and features that cross column borders
//! (trees, veins) come out identical on both sides.

mod atmosphere;
mod biome;
mod caves;
mod climate;
mod content;
mod decor;
mod density;
mod dials;
mod ecology;
mod features;
#[cfg(test)]
mod generated_water;
mod geology;
mod growth;
mod hue;
mod lattice;
mod levels;
mod noise;
mod parallel;
pub use parallel::set_helper_limit;
#[cfg(test)]
mod perf;
mod rarity;
mod hoards;
mod ores;
mod rng;
mod rocks;
mod router;
mod lairs;
mod settlements;
mod shapes;
mod stage;
mod surface;
mod terrain;
mod trails;
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
pub use content::{DayTime, Motion, ParticleDef, WeatherWhen, WingPattern, WingsDef, WINGS_MOST};
pub use caves::{CaveStyle, LAVA_LEVEL};
pub use climate::LAPSE_RATE;
pub use content::{ContentError, Select, WorldContent};
pub use dials::WorldDials;
pub use ecology::PlantProperties;
pub use water::{Body, Cavity, Current, WaterSight};
pub use levels::{LevelsDef, MAX_LEVEL};
pub use rarity::{Featured, Rarity, Tier};
pub use geology::Form;
pub use rocks::ProvinceId;
pub use shapes::ShapeId;
pub use trails::{Kind as PlaceKind, Poi, TrailMark};
pub use lairs::{LairDef, LairSite};
pub use settlements::{Building as SettlementBuilding, Facing as SettlementFacing, Kind as BuildingKind, Levers as SettlementLevers, people};

use fastnoise_lite::{FastNoiseLite, FractalType};

use crate::biome::{GRADIENT_STEP, Pick, Point};
use crate::climate::{ClimateGrid, Regional};
use crate::ecology::Ecology;
use crate::geology::{GeoSample, Geology, smoothstep};
use crate::rocks::Rocks;
use crate::stage::Stage;
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
    /// How steep the ground is around the position, in blocks of rise per block, from the
    /// heights two blocks away on either side: smooth from one block to the next, so rules
    /// switching on it make coherent patches rather than a checkerboard on stepped slopes.
    pub steepness: f32,
    /// Mean annual air temperature at the surface, °C, after cooling with altitude.
    pub temperature: f32,
    /// Annual precipitation, mm.
    pub precipitation: f32,
    /// The regional humidity, in the units of precipitation over potential evaporation:
    /// around 0.1 in deserts, 1 where they balance, 2 in rainforests (see
    /// `content::HumidityDef`).
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
    /// Evenly spread over 0 ..= 1, varying over kilometres: which sub-biome of a
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
    /// The forest's structure, evenly spread over 0 ..= 1: gaps and scrubby edges low,
    /// ordinary woodland in the middle, old-growth cores (dense, tall, the giants) high.
    pub stand: f32,
    /// How deep into a clearing the position lies: 0 outside, rising across its soft edge
    /// to 1. Clearings open in every biome; trees stay out of them.
    pub clearing: f32,
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
    /// How much of a trail's tread lies here, 0 (none) ..= 1 (the middle of the path).
    pub trail: f32,
}

impl Site {
    /// Water available to plants: the climate's, or ground water near rivers and lakes.
    pub fn soil_moisture(&self) -> f32 {
        self.moisture.max(self.river * 1.3)
    }

    /// Temperature and humidity as bytes (0..=255): coordinates in
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
    /// Temperature and humidity bytes, as [`Site::tint`] gives them.
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
    trails: trails::Trails,
    settlements: settlements::Settlements,
    lairs: lairs::Lairs,
    ecology: Ecology,
    decor: decor::Decor,
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
    /// The biome picker's fields on the lattice `locate_biome` searches, a tile of
    /// `FIELD_TILE` × `FIELD_TILE` points at a time, by the lattice's offset and the tile:
    /// searches from the same lattice (a test's many biomes from the same places) work
    /// each out once.
    fields: water::cache::Cache<(IVec2, IVec2), Arc<[Point]>>,
    /// `spawn_search` without and with trails and settlements, each worked out once: a
    /// shared generator gives every server on its world the spawn found by the first.
    spawns: [std::sync::OnceLock<IVec3>; 2],
    seed: u64,
}

/// Points a side of a tile of `Overworld::fields`, and the bytes those tiles may hold.
const FIELD_TILE: i32 = 16;
const FIELD_BUDGET: usize = 16 << 20;

/// How far around the first place found `locate_biome` looks for the biome's interior.
const LOCATE_INSIDE: i32 = 256;
/// How far from its border a located biome's patch must reach somewhere, in blocks.
const LOCATE_DEPTH: f32 = 40.0;

impl Overworld {
    pub const ID: &'static str = "base:overworld";
    /// Bump when generation output changes.
    pub const VERSION: u32 = 23;

    /// A generator with the base package's biomes, rock provinces and climate bands
    /// alone, for tools and distant terrain; worlds are made `with_content`.
    pub fn new(seed: u64, registry: Arc<BlockRegistry>) -> Self {
        let content = WorldContent::base(&registry).expect("the base package's world content is valid");
        Self::with_content(seed, registry, &content)
    }

    /// A generator with the world content of loaded packages ([`WorldContent::load`]).
    pub fn with_content(seed: u64, registry: Arc<BlockRegistry>, content: &WorldContent) -> Self {
        Self::with_content_at(seed, registry, content, Self::VERSION)
    }

    /// As [`with_content`](Self::with_content), as generation `version` would seed it: tests
    /// use it to check a world before `VERSION` is bumped to reseed every world.
    fn with_content_at(seed: u64, registry: Arc<BlockRegistry>, content: &WorldContent, version: u32) -> Self {
        let stream = rng::stream_seed(seed, Self::ID, version);
        let mut dials = WorldDials::sample(stream);
        dials.pole_distance *= content.climate.zones / WorldDials::POLE_DISTANCE;
        let rocks = Rocks::new(stream, content, &registry, &dials.featured);
        let mut rules = SurfaceRules::new(stream, content, &registry);
        let biomes = Biomes::new(content, &registry, &mut rules, &rocks, &dials.featured);
        let ecology = Ecology::new(stream, &dials, content, &registry);
        let decor = decor::Decor::new(content, &registry, &mut rules, &ecology, &dials.featured, features::CELL, stream);
        let underground = underground::Underground::new(stream, content, &registry, &biomes, &mut rules, &rocks);
        let shape_surfaces = content
            .shapes
            .iter()
            .map(|shape| {
                let surface = rules.compile(shape.surface.as_ref()?, &registry, &rocks, None);
                Some(ShapeSurface { surface, strata: shape.strata.iter().map(|name| registry.expect(name)).collect() })
            })
            .collect();
        Self {
            dials,
            geology: Geology::new(stream, &dials, content, &rocks),
            rivers: water::Rivers::new(stream, &dials, content),
            ecology,
            decor,
            trails: trails::Trails::new(stream, content.trails.laid),
            settlements: settlements::Settlements::new(stream, &content.settlements, &registry, trees::tallest(content)),
            lairs: lairs::Lairs::new(stream, &content.lairs, &registry),
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
            fields: water::cache::Cache::new(FIELD_BUDGET),
            spawns: Default::default(),
            seed: stream,
        }
    }

    /// The generator of the world with this seed made from `content` (`world` being its
    /// world content), shared with whatever else in the process asks for the same while
    /// any of them holds it. Generation is a pure function of the seed and the content,
    /// so a shared generator makes exactly what a new one would; what sharing saves is
    /// planning the same rivers, trails and settlements again. A server started on a world
    /// another holds open (each gameplay test starts its own on one seed) finds the land
    /// round its spawn already planned rather than keeping its first player waiting.
    pub fn shared(seed: u64, content: &voxmar_engine::content::Content, world: &WorldContent) -> Arc<Self> {
        let key = (seed, content.fingerprint());
        let mut shared = Self::held().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(found) = Self::find(&mut shared, key) {
            return found;
        }
        let made = Arc::new(Self::with_content(seed, content.blocks.clone(), world));
        shared.push((key, Arc::downgrade(&made)));
        made
    }

    /// As `shared`, reading the world content from `content` (outside the lock) only when
    /// no generator for it is held: a server started beside another on its world reads none.
    pub fn shared_loading(seed: u64, content: &voxmar_engine::content::Content) -> Result<Arc<Self>, ContentError> {
        let key = (seed, content.fingerprint());
        if let Some(found) = Self::find(&mut Self::held().lock().unwrap_or_else(|poisoned| poisoned.into_inner()), key) {
            return Ok(found);
        }
        Ok(Self::shared(seed, content, &WorldContent::load(content)?))
    }

    /// The generators `shared` hands out, by seed and content fingerprint.
    fn held() -> &'static std::sync::Mutex<Vec<((u64, u64), std::sync::Weak<Overworld>)>> {
        static SHARED: std::sync::Mutex<Vec<((u64, u64), std::sync::Weak<Overworld>)>> = std::sync::Mutex::new(Vec::new());
        &SHARED
    }

    fn find(shared: &mut Vec<((u64, u64), std::sync::Weak<Overworld>)>, key: (u64, u64)) -> Option<Arc<Self>> {
        shared.retain(|(_, held)| held.strong_count() > 0);
        shared.iter().find(|(k, _)| *k == key).and_then(|(_, held)| held.upgrade())
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

    /// The level of the land at (x, z), 1 ..= 10 (`levels`): its home biome's, the biome
    /// whose land it is, not a river's or the neighbour's dithered in near a border. A
    /// cave's is `cave_biome_at(pos)`'s `level`.
    pub fn level_at(&self, x: i32, z: i32) -> u8 {
        self.biomes.get(self.site(x, z).home).level
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
        let found = self.biomes.locate(
            id,
            from,
            radius,
            climate::CELL,
            |ring| self.fields_at(ring),
            // Only a patch with ground at least `LOCATE_DEPTH` from its border near the
            // candidate: a sliver of the biome is passed over and the search goes on.
            |at| {
                if self.sampler(at, at).pick(at.x, at.y).home != id {
                    return false;
                }
                let near = self.sampler(at - 64, at + 64);
                (0..9 * 9).any(|i| {
                    let p = at + IVec2::new(i % 9 * 16 - 64, i / 9 * 16 - 64);
                    let site = near.site(p.x, p.y);
                    site.home == id && site.biome == id && site.edge >= LOCATE_DEPTH
                })
            },
        )?;
        // Then the point deepest inside it around there (not a sliver at a border, nor a
        // variant within it), on dry land where there is some.
        let area = self.sampler(found - LOCATE_INSIDE, found + LOCATE_INSIDE);
        let steps = 2 * LOCATE_INSIDE / 16 + 1;
        let best = (0..steps * steps)
            .map(|i| found + IVec2::new(i % steps * 16 - LOCATE_INSIDE, i / steps * 16 - LOCATE_INSIDE))
            .filter_map(|p| {
                let site = area.site(p.x, p.y);
                (site.home == id && site.biome == id).then(|| (p, site.edge + if site.height > site.water { 64.0 } else { 0.0 }))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        Some(best.map_or(found, |b| b.0))
    }

    /// The biome picker's fields at each of `at`, all on one climate lattice (a multiple
    /// of `climate::CELL` apart), from the tiles of them cached in `fields`.
    fn fields_at(&self, at: &[IVec2]) -> Vec<Point> {
        let mut last: Option<((IVec2, IVec2), Arc<[Point]>)> = None;
        at.iter()
            .map(|&p| {
                let offset = p.rem_euclid(IVec2::splat(climate::CELL));
                let cell = (p - offset) / climate::CELL;
                let key = (offset, cell.div_euclid(IVec2::splat(FIELD_TILE)));
                let tile = match &last {
                    Some((k, tile)) if *k == key => tile.clone(),
                    _ => {
                        let tile = self.fields.get(key, || {
                            let first = offset + key.1 * FIELD_TILE * climate::CELL;
                            let at: Vec<(f32, f32)> = (0..FIELD_TILE * FIELD_TILE)
                                .map(|i| first + IVec2::new(i % FIELD_TILE, i / FIELD_TILE) * climate::CELL)
                                .map(|q| (q.x as f32, q.y as f32))
                                .collect();
                            self.geology.climate().regionals(&self.geology, &at).iter().map(|r| self.point(r)).collect()
                        });
                        last = Some((key, tile.clone()));
                        tile
                    }
                };
                let within = cell.rem_euclid(IVec2::splat(FIELD_TILE));
                tile[(within.y * FIELD_TILE + within.x) as usize]
            })
            .collect()
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
            regional.lean,
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

    /// Surface height only; cheap enough to sample on coarse grids. Trails, which change
    /// it by a few blocks along a narrow line, are left out (see [`Overworld::site`]).
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.ground(x, z).height
    }

    /// The ground at one position with its rivers and lakes, its crags to their tops.
    fn ground(&self, x: i32, z: i32) -> GeoSample {
        self.rivers.sample(self.stages(), x, z, true)
    }

    fn stages(&self) -> water::Stages<'_> {
        water::Stages { geology: &self.geology, climate: self.geology.climate() }
    }

    /// The ground at one position, with its rivers, lakes and trails.
    pub(crate) fn geo(&self, x: i32, z: i32) -> GeoSample {
        let sample = {
            let _stage = stage::enter(Stage::Rivers);
            self.ground(x, z)
        };
        let _stage = stage::enter(Stage::Trails);
        let sample = self.trails.apply(&self.land(), sample, x, z);
        self.settlements.apply(self, sample, x, z)
    }

    /// The ground at the `size`×`size` positions `step` blocks apart from `origin`, row by
    /// row: the same as [`Overworld::geo`] at each, finding the rivers near them once a
    /// tile of positions (a few regions at most, which `Rivers::grid` shares), the tiles
    /// side by side.
    pub(crate) fn geo_grid(&self, origin: IVec2, step: i32, size: i32) -> Vec<GeoSample> {
        // Tiles of about 64 positions a side, as even as the size allows.
        let count = (size + 63) / 64;
        let tile = (size + count - 1) / count.max(1);
        let one = |t: i32| {
            let from = origin + IVec2::new(t % count, t / count) * tile * step;
            let land = self.land();
            let mut samples = {
                let _stage = stage::enter(Stage::Rivers);
                self.rivers.grid_with_crags(self.stages(), from, step, tile)
            };
            let _stage = stage::enter(Stage::Trails);
            for (i, sample) in samples.iter_mut().enumerate() {
                let (x, z) = (from.x + i as i32 % tile * step, from.y + i as i32 / tile * step);
                *sample = self.settlements.apply(self, self.trails.apply(&land, *sample, x, z), x, z);
            }
            samples
        };
        // Side by side in tests (rayon is only a test dependency here); the same tiles either way.
        #[cfg(test)]
        let tiles: Vec<Vec<GeoSample>> = {
            use rayon::prelude::*;
            (0..count * count).into_par_iter().map(one).collect()
        };
        #[cfg(not(test))]
        let tiles: Vec<Vec<GeoSample>> = (0..count * count).map(one).collect();
        // Cropped to the grid where the last tiles run past it.
        (0..size * size)
            .map(|i| {
                let (i, j) = (i % size, i / size);
                tiles[((j / tile) * count + i / tile) as usize][((j % tile) * tile + i % tile) as usize]
            })
            .collect()
    }

    /// The ground over the `size`×`size` positions from (x, z), row by row.
    pub(crate) fn geo_area(&self, x: i32, z: i32, size: i32) -> Vec<GeoSample> {
        let land = self.land();
        let mut area = {
            let _stage = stage::enter(Stage::Rivers);
            self.rivers.area(self.stages(), IVec2::new(x, z), size)
        };
        let _stage = stage::enter(Stage::Trails);
        for (i, sample) in area.iter_mut().enumerate() {
            let (sx, sz) = (x + i as i32 % size, z + i as i32 / size);
            *sample = self.settlements.apply(self, self.trails.apply(&land, *sample, sx, sz), sx, sz);
        }
        area
    }

    /// The land trails are planned over: the ground with its rivers and lakes.
    fn land(&self) -> RiverLand<'_> {
        RiverLand { rivers: &self.rivers, stages: self.stages(), world: self }
    }

    /// The middle of the vein a sample of ore (`block`) lying at `pos` marks, telling one
    /// vein from another: none for a sample no vein laid there (one a player set down).
    pub fn vein_of_sample(&self, pos: IVec3, block: BlockId) -> Option<IVec3> {
        self.terrain.ores.vein_of_sample(self, pos.x, pos.z, block)
    }

    /// The lairs within `radius` blocks of `centre`, nearest first (`lairs.ron`).
    pub fn lairs_near(&self, centre: IVec2, radius: i32) -> Vec<LairSite> {
        self.lairs.near(self, centre, radius)
    }

    /// The kinds of lair, by `LairSite::lair`.
    pub fn lair_defs(&self) -> &[LairDef] {
        self.lairs.defs()
    }

    /// Whether a settlement's plan covers (x, z).
    pub(crate) fn settlement_covers(&self, x: i32, z: i32) -> bool {
        self.settlements.covers(self, x, z)
    }

    /// The settlements whose squares lie within `radius` blocks of `centre`: their names
    /// (English and Japanese), sizes, middles and buildings.
    pub fn settlements_near(&self, centre: IVec2, radius: i32) -> Vec<SettlementInfo> {
        self.settlements.near(self, centre, radius).iter().filter_map(|slot| slot.0.as_ref()).map(|p| self.settlement_info(p)).collect()
    }

    /// The settlement nearest `from` by its middle, within `radius`, that `keep` takes:
    /// what `settlements_near` would find nearest, planning only the squares nearer than
    /// it (`/locate settlement`).
    pub fn nearest_settlement(&self, from: IVec2, radius: i32, keep: impl Fn(&SettlementInfo) -> bool) -> Option<SettlementInfo> {
        let slot = self.settlements.nearest(self, from, radius, |p| keep(&self.settlement_info(p)))?;
        slot.0.as_ref().map(|p| self.settlement_info(p))
    }

    /// The settlement whose square holds (x, z), if that square is settled: the only one a
    /// block there can be part of, planning that square alone.
    pub fn settlement_holding(&self, at: IVec2) -> Option<SettlementInfo> {
        let slot = self.settlements.holding(self, at.x, at.y)?;
        slot.0.as_ref().map(|p| self.settlement_info(p))
    }

    fn settlement_info(&self, p: &settlements::Plan) -> SettlementInfo {
        SettlementInfo {
            id: p.id,
            name: p.name.clone(),
            size: p.size.clone(),
            style: p.style.clone(),
            layout: p.layout.clone(),
            centre: IVec3::new(p.centre.x, p.height, p.centre.y),
            buildings: p.buildings.clone(),
            gates: p.gates.clone(),
            lanes: p.lanes.clone(),
            levers: p.levers,
            culture: self.settlements.culture_info(p),
            people: p.people.clone(),
            walls: p.walls.clone(),
            wall_block: p.wall_block,
        }
    }

    /// The shaped roof pieces the settlement whose square's middle is at `centre` lays,
    /// each where it goes (none where no settlement stands there).
    pub fn settlement_roofs(&self, centre: IVec2) -> Vec<(IVec3, voxmar_engine::block::BlockId)> {
        self.settlements.roofs_of(self, centre).unwrap_or_default()
    }

    /// The blocks `keep` picks that the settlement whose square's middle is at `centre`
    /// sets, each where it goes.
    pub fn settlement_planned(&self, centre: IVec2, keep: &dyn Fn(voxmar_engine::block::BlockId) -> bool) -> Vec<(IVec3, voxmar_engine::block::BlockId)> {
        self.settlements.planned(self, centre, keep).unwrap_or_default()
    }

    /// Whether `block` is one of settlements' own flames (`rules.fire.flames`).
    pub fn settlement_flame(&self, block: voxmar_engine::block::BlockId) -> bool {
        self.settlements.fire().is_some_and(|f| f.flame(block))
    }

    /// How many steps settlements' fire reaches from a flame (`rules.fire.reach`).
    pub fn settlement_fire_reach(&self) -> i32 {
        self.settlements.fire().map_or(0, |f| f.reach)
    }

    /// Every block that catches fire (thatch, wattle) a flame among `blocks` reaches, as
    /// settlements keep their own flames from it (`rules.fire`): each flame and what it
    /// reaches.
    pub fn flames_reaching(&self, blocks: &rustc_hash::FxHashMap<IVec3, voxmar_engine::block::BlockId>) -> Vec<(IVec3, IVec3)> {
        self.settlements.fire().map_or_else(Vec::new, |f| f.exposed(blocks))
    }

    /// The names of the sizes settlements come in, as written.
    pub fn settlement_sizes(&self) -> Vec<String> {
        self.settlements.sizes()
    }

    /// The routines and lists settlements' people live by.
    pub fn settlement_people(&self) -> &people::PeopleDef {
        self.settlements.people()
    }

    /// What is built beside trails within the blocks from `min` to `max`.
    pub(crate) fn trail_markers(&self, min: IVec2, max: IVec2) -> Vec<(IVec2, trails::Marker)> {
        self.trails.markers(&self.land(), min, max)
    }

    /// The trails through the blocks from `min` to `max`, as centrelines (each point with
    /// 0 on the ground, 1 on a ford and 2 on a bridge), and the places they join.
    pub fn trails_in(&self, min: IVec2, max: IVec2) -> (Vec<Vec<(voxmar_engine::glam::Vec2, u8)>>, Vec<Poi>) {
        self.trails.sketch(&self.land(), min, max)
    }


    /// Every set of bands (strata) the surface lays by height, of biomes, of terrain shapes
    /// and of the rock provinces' bare rock: what distant terrain blends to show a banded
    /// slope as it looks from afar.
    pub fn strata(&self) -> impl Iterator<Item = &[BlockId]> {
        let shapes = self.shape_surfaces.iter().flatten().map(|shape| shape.strata.as_slice());
        let rock = (0..self.rocks.len()).map(|i| self.rocks.get(ProvinceId(i as u8)).bands.as_slice());
        self.biomes.strata().chain(shapes).chain(rock).filter(|strata| !strata.is_empty())
    }

    /// Samples many positions within the blocks from `min` to `max` (inclusive) far
    /// faster than one at a time, by sharing the regional climate.
    pub fn sampler(&self, min: IVec2, max: IVec2) -> Sampler<'_> {
        Sampler { world: self, grid: self.grid(min, max), picks: None }
    }

    /// A sampler that picks the biomes of every cell in its area up front, for sampling
    /// it block by block.
    pub(crate) fn dense_sampler(&self, min: IVec2, max: IVec2) -> Sampler<'_> {
        let grid = {
            let _stage = stage::enter(Stage::Climate);
            self.grid(min, max)
        };
        let _stage = stage::enter(Stage::Biomes);
        let mut sampler = Sampler { world: self, grid, picks: None };
        let (first, last) = (min.div_euclid(IVec2::splat(biome::CELL)), max.div_euclid(IVec2::splat(biome::CELL)));
        let size = last - first + 1;
        let cells: Vec<IVec2> = (0..size.x * size.y).map(|i| first + IVec2::new(i % size.x, i / size.x)).collect();
        let shores = sampler.shores(&cells);
        let picks = cells.iter().zip(shores).map(|(&cell, shore)| sampler.pick_on(cell, shore)).collect();
        sampler.picks = Some(Picks { first, width: size.x, picks });
        sampler
    }

    /// The regional climate over an area, with room for the biome picker's probes.
    fn grid(&self, min: IVec2, max: IVec2) -> ClimateGrid {
        let margin = biome::CELL + GRADIENT_STEP;
        ClimateGrid::new(&self.geology, min - margin, max + margin)
    }

    /// The parameters of the surface at one position. Like the other sampling for maps,
    /// distant terrain and coarse decisions, it leaves trails out: planning them costs far
    /// more than one position is worth, and they are only a few blocks wide. Generated
    /// columns have them.
    pub fn site(&self, x: i32, z: i32) -> Site {
        self.sampler(IVec2::new(x, z), IVec2::new(x, z)).site(x, z)
    }

    /// The home biome and regional temperature for decisions spanning kilometres, such
    /// as culture regions. Reading their smooth fields must not plan distant rivers or
    /// sample local slopes and vegetation, which only a settlement's own site needs.
    pub(crate) fn regional_biome(&self, x: i32, z: i32) -> (&Biome, f32) {
        // Probes kilometres apart: no tile of the climate lattice is read twice.
        let margin = biome::CELL + GRADIENT_STEP;
        let at = IVec2::new(x, z);
        let sampler = Sampler { world: self, grid: ClimateGrid::alone(&self.geology, at - margin, at + margin), picks: None };
        let home = sampler.pick(x, z).home;
        (self.biomes.get(home), sampler.grid.at(x, z).temperature())
    }

    /// What the surface looks like from far away, given its local steepness in blocks.
    /// Matches full generation except for caves and individual trees and plants.
    pub fn surface_sample(&self, x: i32, z: i32, height: i32, slope: i32) -> SurfaceSample {
        self.sampler(IVec2::new(x, z), IVec2::new(x, z)).surface(x, z, height, slope)
    }
}

/// How steep the ground is at a position whose heights around it `height` gives by offset:
/// the gradient of the heights two blocks away on either side, averaged over the three rows
/// and columns through the position, so the single steps of gentle ground read as an even
/// slope (see `Site::steepness`).
pub(crate) fn steepness(height: impl Fn(i32, i32) -> i32) -> f32 {
    let (mut dx, mut dz) = (0, 0);
    for k in -1..=1 {
        dx += height(2, k) - height(-2, k);
        dz += height(k, 2) - height(k, -2);
    }
    (dx as f32 / 12.0).hypot(dz as f32 / 12.0)
}

/// The ground with rivers and lakes, as trails see it: crags are left at their preliminary
/// surface, which is near enough for planning routes that keep off them anyway (and a
/// trail's own ground is whole, see `Trails::apply`).
struct RiverLand<'a> {
    rivers: &'a water::Rivers,
    stages: water::Stages<'a>,
    world: &'a Overworld,
}

impl trails::Land for RiverLand<'_> {
    fn sample(&self, x: i32, z: i32) -> GeoSample {
        self.rivers.sample(self.stages, x, z, false)
    }

    fn sample_cached(&self, x: i32, z: i32, scratch: &mut water::Samples) -> GeoSample {
        self.rivers.sample_cached(self.stages, x, z, scratch)
    }

    fn samples_cached(&self, points: &[(i32, i32)], scratch: &mut water::Samples) -> Vec<GeoSample> {
        self.rivers.samples_cached(self.stages, points, scratch)
    }

    fn freshwater_absent_cached(&self, at: IVec2, scratch: &mut water::Samples) -> bool {
        self.rivers.freshwater_absent_cached(self.stages, at, scratch)
    }

    fn freshwater_near(&self, centre: IVec2, radius: i32, step: i32, cache: &mut water::Freshwater) -> bool {
        self.rivers.freshwater_near(self.stages, centre, radius, step, cache)
    }

    fn remember_freshwater_grid(&self, origin: IVec2, step: i32, size: i32, samples: &[GeoSample], cache: &mut water::Freshwater) {
        self.rivers.remember_freshwater_grid(origin, step, size, samples, cache);
    }

    fn grid(&self, origin: IVec2, step: i32, size: i32) -> Vec<GeoSample> {
        self.rivers.grid(self.stages, origin, step, size)
    }

    fn settled(&self, min: IVec2, max: IVec2, reach: i32) -> bool {
        self.world.settlements.any_near(self.world, min, max, reach)
    }

    fn built(&self, min: IVec2, max: IVec2) -> Vec<(IVec2, IVec2)> {
        let centre = (min + max) / 2;
        let radius = (max - min).max_element() / 2 + 1;
        self.world
            .settlements
            .near(self.world, centre, radius)
            .iter()
            .filter_map(|slot| slot.0.as_ref())
            // A walled place's wall too, column by column, so trails come in at its gates.
            .flat_map(|p| p.buildings.iter().map(|b| (b.min, b.max)).chain(p.walls.iter().map(|w| (w.0, w.0))))
            .filter(|(lo, hi)| lo.cmple(max).all() && hi.cmpge(min).all())
            .collect()
    }

    fn gates(&self, min: IVec2, max: IVec2) -> Vec<IVec2> {
        let centre = (min + max) / 2;
        let radius = (max - min).max_element() / 2 + 1;
        self.world
            .settlements
            .near(self.world, centre, radius)
            .iter()
            .filter_map(|slot| slot.0.as_ref())
            .flat_map(|p| p.gates.iter().copied())
            .filter(|g| g.cmpge(min).all() && g.cmple(max).all())
            .collect()
    }
}

/// A settlement, as gameplay and tools see it.
#[derive(Clone, Debug, PartialEq)]
pub struct SettlementInfo {
    pub id: u64,
    /// In English and in Japanese.
    pub name: (String, String),
    /// `hamlet`, `village` or `town` (the size's name in settlements.ron), and its style and
    /// layout.
    pub size: String,
    pub style: String,
    pub layout: String,
    /// The middle of its square or green, on the ground.
    pub centre: IVec3,
    pub buildings: Vec<SettlementBuilding>,
    /// Where its lanes leading out end.
    pub gates: Vec<IVec2>,
    pub lanes: Vec<Vec<voxmar_engine::glam::Vec2>>,
    /// The five settlement levers and eight culture axes, each 0 ..= 1.
    pub levers: settlements::Levers,
    pub culture: CultureInfo,
    /// Who lives there, each with their home and work as indices into `buildings`.
    pub people: Vec<people::Resident>,
    /// Its wall, if it is walled: each column and how many blocks over the ground its top
    /// stands, and the block it is built of.
    pub walls: Vec<(IVec2, i32)>,
    pub wall_block: Option<voxmar_engine::block::BlockId>,
}

pub use settlements::CultureInfo;

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
        // The ground five blocks across around the position, sampled together, for its
        // slope and steepness.
        let area = self.world.rivers.area(self.world.stages(), IVec2::new(x - 2, z - 2), 5);
        let height = |dx: i32, dz: i32| area[((dz + 2) * 5 + dx + 2) as usize].height;
        let geo = area[12];
        let slope = [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().map(|(dx, dz)| (height(dx, dz) - geo.height).abs()).max().unwrap_or(0);
        self.site_on(&geo, x, z, slope, steepness(height))
    }

    /// What the surface looks like from far away; see [`Overworld::surface_sample`].
    pub fn surface(&self, x: i32, z: i32, height: i32, slope: i32) -> SurfaceSample {
        let geo = GeoSample { height, ..self.world.ground(x, z) };
        let site = self.site_on(&geo, x, z, slope, slope as f32);
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
        let _stage = stage::enter(Stage::Climate);
        let (climate, n) = self.climate_colors(site, x, z);
        // Bilinear between the four nearest cell centres, so identity colours fade in
        // block by block rather than in steps of a cell.
        let p = IVec2::new(x, z) - biome::CELL / 2;
        let cell = p.div_euclid(IVec2::splat(biome::CELL));
        let f = (p - cell * biome::CELL).as_vec2() / biome::CELL as f32;
        let mut sum = [[0.0f32; 3]; 3];
        // Corners of the same biomes at the same share have the same colours (all four,
        // inside a biome), worked out once.
        let mut last: Option<((BiomeId, BiomeId, u32), [[f32; 3]; 3])> = None;
        for (dx, dz, w) in [(0, 0, (1.0 - f.x) * (1.0 - f.y)), (1, 0, f.x * (1.0 - f.y)), (0, 1, (1.0 - f.x) * f.y), (1, 1, f.x * f.y)] {
            let at = (cell + IVec2::new(dx, dz)) * biome::CELL;
            let pick = self.pick(at.x, at.y);
            let share = Biomes::blend(pick.edge, site.fertility);
            let key = (pick.home, pick.neighbour, share.to_bits());
            let colors = match last {
                Some((k, colors)) if k == key => colors,
                _ => self.biome_colors(climate, n, &pick, share),
            };
            last = Some((key, colors));
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
        let [grass, foliage] = hue::climate_tints(t, h);
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
        let mut colors: [[f32; 3]; 3] = std::array::from_fn(|i| mix3(home[i], neighbour[i], share));
        colors[0] = hue::soften_grass(colors[0]);
        colors
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
        self.pick_on(cell, self.shores(&[cell])[0])
    }

    /// The land's own continentalness at the centre of each cell (the router's, exactly
    /// there rather than the regional climate's), and whether its corner lies above the sea:
    /// what keeps a coast's biomes to their side of the water line. All at once, as the
    /// router works four positions at a time.
    fn shores(&self, cells: &[IVec2]) -> Vec<(f32, bool)> {
        let geology = &self.world.geology;
        let at = |p: IVec2| (p.x as f32, p.y as f32);
        let centres: Vec<(f32, f32)> = cells.iter().map(|&c| at(c * biome::CELL + biome::CELL / 2)).collect();
        let corners: Vec<(f32, f32)> = cells.iter().map(|&c| at(c * biome::CELL)).collect();
        let continental = geology.continentals(&centres);
        let reliefs = geology.reliefs(&corners);
        continental.iter().zip(reliefs).map(|(&c, relief)| (climate::continentalness(c), relief.elevation > SEA_LEVEL as f32)).collect()
    }

    /// Picks the biome of a cell from the fields at its centre, with continentalness its
    /// shore's (see `shores`): the regional field moved to agree with it at the centre, so
    /// its gradient still finds the borders, and on the dry side of zero where the cell is
    /// land, the wet side where it is sea.
    fn pick_on(&self, cell: IVec2, (continentalness, dry): (f32, bool)) -> Pick {
        let centre = cell * biome::CELL + biome::CELL / 2;
        let at = [IVec2::ZERO, IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y].map(|d| centre + d * GRADIENT_STEP);
        let regional = at.map(|p| self.grid.at(p.x, p.y));
        let shift = continentalness - regional[0].continentalness;
        let point = |k: usize| {
            let c = if k > 0 {
                regional[k].continentalness + shift
            } else if dry {
                continentalness.max(0.0)
            } else {
                continentalness.min(-0.001)
            };
            self.world.point(&Regional { continentalness: c, ..regional[k] })
        };
        self.world.biomes.pick(&point(0), &[point(1), point(2), point(3), point(4)])
    }

    /// The site on already sampled geology.
    fn site_on(&self, geo: &GeoSample, x: i32, z: i32, slope: i32, steepness: f32) -> Site {
        let _stage = stage::enter(Stage::Climate);
        let regional = self.grid.at(x, z);
        // Cooled by the height of the land, not of the towers and spires standing on it.
        let temperature = regional.sea_temperature - LAPSE_RATE * (geo.climate_height() - SEA_LEVEL).max(0) as f32;
        let mut site = Site {
            height: geo.height,
            slope,
            steepness,
            temperature,
            precipitation: regional.precipitation,
            moisture: regional.moisture(),
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
            stand: self.world.ecology.stand(x, z),
            clearing: self.world.ecology.clearing(x, z),
            province: self.world.rocks.at(x, z, regional.warp),
            biome: BiomeId(0),
            home: BiomeId(0),
            neighbour: BiomeId(0),
            edge: 0.0,
            from_centre: 0.0,
            form: geo.form,
            trail: geo.trail.map_or(0.0, |t| t.tread),
        };
        let pick = self.pick(x, z);
        let share = self.world.biomes.blend_between(pick.home, pick.neighbour, pick.edge, regional.fertility);
        if !geo.submerged() {
            site.vegetation = ecology::ground_cover(temperature, site.soil_moisture());
            site.forest = self.world.ecology.forest(site.stand, site.clearing, pick.home, pick.neighbour, share, regional.fertility, temperature, site.soil_moisture());
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
        self.spawn_search(true)
    }
}

impl Overworld {
    /// Near where a new player spawns, from the land alone, before trails and
    /// settlements: the spawn point unless it would fall on a trail or in a settlement,
    /// and then a step or two along the same search. Settlements plan by it, so the
    /// square a player spawns in can always be settled.
    pub(crate) fn spawn_estimate(&self) -> IVec2 {
        let at = self.spawn_search(false);
        IVec2::new(at.x, at.z)
    }

    /// Walks outwards on a spiral until it finds open, temperate grassland on dry land;
    /// `built`: also off trails and outside settlements, which plan by the search without.
    /// Each is worked out once (`spawns`), so a search must never ask for its own kind's
    /// spawn: it would wait on itself.
    fn spawn_search(&self, built: bool) -> IVec3 {
        *self.spawns[built as usize].get_or_init(|| self.search_spawn(built))
    }

    fn search_spawn(&self, built: bool) -> IVec3 {
        for ring in 0..256 {
            for step in 0..(ring * 8).max(1) {
                let (x, z) = spiral(ring, step);
                let (x, z) = (x * 16, z * 16);
                let height = self.height(x, z);
                if !(SEA_LEVEL + 3..=SEA_LEVEL + 30).contains(&height) {
                    continue;
                }
                // Level ground first, from the neighbours' heights alone: cheaper than a site.
                if [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| self.height(x + dx, z + dz) != height) {
                    continue;
                }
                let site = self.site(x, z);
                let pleasant = (8.0..20.0).contains(&site.temperature) && (0.6..1.6).contains(&site.soil_moisture());
                let open = site.slope == 0 && site.forest < 0.35 && site.river < 0.1;
                // Not on a trail, whose tread is no grass (sites leave trails out), nor in
                // a settlement: a new player walks into it.
                if pleasant && open && self.terrain.is_grass(self.surface_layers(&site, x, z).top) && (!built || self.geo(x, z).trail.is_none() && !self.settlements.covers(self, x, z)) {
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

/// Every package's content, as a server loads it, loaded once for the run and read by the
/// tests that only read it.
#[cfg(test)]
pub(crate) fn full_content() -> &'static (voxmar_engine::content::Content, WorldContent) {
    static FULL: std::sync::OnceLock<(voxmar_engine::content::Content, WorldContent)> = std::sync::OnceLock::new();
    FULL.get_or_init(|| {
        let content = voxmar_engine::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap();
        let world = WorldContent::load(&content).unwrap();
        (content, world)
    })
}

/// The seeds more than one test reads on every package's content (`shared_full_world`):
/// the water tests (`generated_water`), and the village thatch and jettied storeys tests.
#[cfg(test)]
const HELD_FULL_SEEDS: [u64; 5] = [1, 2, 3, 7, 42];

/// The world of `seed` on every package's content (`full_content`), for tests that only read
/// what it generates. A seed of `HELD_FULL_SEEDS` is held for the rest of the run: what a
/// world generates depends on neither which test asks first nor on what its caches hold,
/// and the next test on the seed finds its rivers and settlements already planned rather
/// than planning them again. Any other seed gets a world of its own, let go with its test,
/// as a held world keeps all it planned. Kept apart from `Overworld::shared`, whose letting
/// go is tested. Tests that count what a world plans, or compare a fresh world with another,
/// make their own.
#[cfg(test)]
pub(crate) fn shared_full_world(seed: u64) -> Arc<Overworld> {
    let (content, world) = full_content();
    let make = || Arc::new(Overworld::with_content(seed, content.blocks.clone(), world));
    if !HELD_FULL_SEEDS.contains(&seed) {
        return make();
    }
    static WORLDS: std::sync::Mutex<Vec<(u64, Arc<std::sync::OnceLock<Arc<Overworld>>>)>> = std::sync::Mutex::new(Vec::new());
    let slot = {
        let mut worlds = WORLDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match worlds.iter().find(|(s, _)| *s == seed) {
            Some((_, slot)) => slot.clone(),
            None => {
                let slot = Arc::new(std::sync::OnceLock::new());
                worlds.push((seed, slot.clone()));
                slot
            }
        }
    };
    slot.get_or_init(make).clone()
}

/// All base block files, in the same order as the game loader. Partial registries
/// omit furnishings and tag members, which can change generation and census results.
#[cfg(test)]
pub(crate) fn test_registry() -> BlockRegistry {
    let sources = [
        include_str!("../../../assets/content/base/blocks.ron"),
        include_str!("../../../assets/content/base/blocks_armour.ron"),
        include_str!("../../../assets/content/base/blocks_bees.ron"),
        include_str!("../../../assets/content/base/blocks_creatures.ron"),
        include_str!("../../../assets/content/base/blocks_farm.ron"),
        include_str!("../../../assets/content/base/blocks_food.ron"),
        include_str!("../../../assets/content/base/blocks_furnishings.ron"),
        include_str!("../../../assets/content/base/blocks_herbs.ron"),
        include_str!("../../../assets/content/base/blocks_machines.ron"),
        include_str!("../../../assets/content/base/blocks_tools.ron"),
        include_str!("../../../assets/content/base/blocks_treasure.ron"),
        include_str!("../../../assets/content/base/blocks_weapons.ron"),
    ];
    let packages = sources.iter().map(|s| ("base".to_owned(), BlockRegistry::read("base", s).unwrap())).collect();
    BlockRegistry::new(BlockRegistry::expand(packages).unwrap()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::thread_time;
    use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS};
    use voxmar_engine::light::LitRegion;
    use voxmar_engine::mesh::mesh_chunk;

    fn registry() -> Arc<BlockRegistry> {
        Arc::new(crate::test_registry())
    }

    /// The world of `seed` on the test registry, one for every test that only reads what it
    /// generates, here and in the settlements' tests: each new world would trace its rivers,
    /// trails and settlements from nothing again. What a world generates depends on neither
    /// which column nor which test asks first, nor on what its caches hold
    /// (`rivers_never_depend_on_generation_order`, `decorations_never_depend_on_generation_order`),
    /// and those tests, the timing ones and the golden hashes still make worlds of their
    /// own. Made once however many tests ask for it at once.
    pub(crate) fn shared_world(seed: u64) -> Arc<Overworld> {
        static WORLDS: std::sync::Mutex<Vec<(u64, Arc<std::sync::OnceLock<Arc<Overworld>>>)>> = std::sync::Mutex::new(Vec::new());
        let slot = {
            let mut worlds = WORLDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match worlds.iter().find(|(s, _)| *s == seed) {
                Some((_, slot)) => slot.clone(),
                None => {
                    let slot = Arc::new(std::sync::OnceLock::new());
                    worlds.push((seed, slot.clone()));
                    slot
                }
            }
        };
        slot.get_or_init(|| Arc::new(Overworld::new(seed, registry()))).clone()
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
        let quads: usize = (0..COLUMN_CHUNKS).map(|cy| mesh_chunk(&registry, &region, &columns[4], cy, 1).opaque.vertices.len() / 4).sum();
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
        // CPU time on this thread measures all the work only if it all stays here.
        crate::parallel::set_enabled(false);
        crate::perf::prefer_fast_cores();
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

    /// The columns at `positions`, generated on the test threads' pool (`RAYON_NUM_THREADS`,
    /// which the gate sets to the cores it leaves the tests), in their order.
    pub(crate) fn generate_columns(world: &Overworld, positions: &[ColumnPos]) -> Vec<Column> {
        use rayon::prelude::*;
        positions.par_iter().map(|&pos| world.generate(pos)).collect()
    }

    /// Generates the `count`×`count` columns from `first`, as [`generate_columns`] does.
    pub(crate) fn generate_area(world: &Overworld, first: ColumnPos, count: i32) -> Vec<Column> {
        let positions: Vec<ColumnPos> = (0..count * count).map(|i| first.offset(i % count, i / count)).collect();
        generate_columns(world, &positions)
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
        // From the dry lowland `distance` off that sees most of a mountain, low over it.
        let afar = |name: &str, x: i32, z: i32, top: f32, distance: f32| {
            let lowland = (0..16)
                .map(|k| k as f32 * std::f32::consts::TAU / 16.0)
                .filter_map(|a| {
                    let (cx, cz) = ((x as f32 + distance * a.cos()) as i32, (z as f32 + distance * a.sin()) as i32);
                    let geo = world.geo(cx, cz);
                    (!geo.submerged() && geo.height > SEA_LEVEL).then_some((geo.height, a))
                })
                .min_by_key(|&(h, _)| h);
            if let Some((ground, a)) = lowland {
                view(name, x, z, top, distance, a, ground as f32 + 40.0 - top);
            }
        };
        afar("range from afar", peak.1, peak.2, peak.0, 900.0);
        view("highest peak", peak.1, peak.2, peak.0, 600.0, std::f32::consts::FRAC_PI_4, 150.0);
        for (i, &(score, x, z, height)) in best.iter().enumerate().filter(|(_, b)| b.0 > 0.0) {
            let name = shapes.name(ShapeId(i as u8));
            print!("score {score:.2}: ");
            if name.contains("volcano") || name.contains("lone") || name.contains("cliff") || name.contains("fjord") {
                let distance = if name.contains("lone") { 1300.0 } else if name.contains("volcano") { 700.0 } else { 150.0 };
                afar(name, x, z, height, distance);
                continue;
            }
            let back = if name.contains("mesa") || name.contains("canyon") { 160.0 } else { 70.0 };
            view(name, x, z, height, back, std::f32::consts::FRAC_PI_4, back * 0.25);
        }
    }

    /// `cargo test --release -p voxmar_worldgen river_sights -- --ignored --nocapture`
    /// (with `SEED`, `RANGE` and `BACK`, the camera's distance) prints `--pose` arguments
    /// that look at each of the places water makes near spawn ([`Overworld::water_sights`]),
    /// for in-game captures.
    #[test]
    #[ignore]
    fn river_sights() {
        let env = |name: &str, default: i64| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
        let (seed, range, back) = (env("SEED", 1) as u64, env("RANGE", 420) as i32, env("BACK", 16) as f32);
        let world = Overworld::new(seed, registry());
        let spawn = world.spawn_point();
        let eye = spawn.as_vec3() + IVec3::Y.as_vec3() * 1.62;
        println!("seed {seed}, spawn {spawn}");
        let sights = world.water_sights(IVec2::new(spawn.x, spawn.z), range);
        for sight in WaterSight::ALL {
            let Some(&(_, at)) = sights.iter().find(|(s, _)| *s == sight) else {
                println!("{}: none within {range}", sight.name());
                continue;
            };
            // Stand back and above, looking at it from the south-east, clear of the ground.
            let target = at.as_vec3();
            let mut camera = target + voxmar_engine::glam::Vec3::new(back, back * 0.6, back);
            camera.y = camera.y.max(world.height(camera.x as i32, camera.z as i32) as f32 + 5.0);
            let d = target - camera;
            let yaw = (-d.x).atan2(-d.z).to_degrees();
            let pitch = d.y.atan2(d.x.hypot(d.z)).to_degrees();
            let o = camera - eye;
            println!("{} at {at}: --pose {:.0},{:.0},{:.0},{yaw:.0},{pitch:.0}", sight.name(), o.x, o.y, o.z);
        }
    }

    /// Logs, stumps, rocks and trees rest on the ground: every solid block above the ground
    /// joins it through other solid blocks (leaves aside, which hang from branches), and
    /// every small plant stands on something solid.
    /// Wherever a forest biome has tree cover, some tree suits the climate: no forest is
    /// left bare because the climate there lies outside every species' comfort.
    #[test]
    fn forests_have_trees() {
        for seed in [42, 7] {
            let world = shared_world(seed);
            let sampler = world.sampler(IVec2::splat(-2048), IVec2::splat(2048));
            let (mut forested, mut bare) = (0, Vec::new());
            let mut chances = Vec::new();
            for i in 0..64 * 64 {
                let (x, z) = (i % 64 * 64 - 2048, i / 64 * 64 - 2048);
                let site = sampler.site(x, z);
                // Banks of creeks and rivers grow the river biome's plants, not the forest's.
                if !world.biomes.get(site.home).has_tag("forest") || site.forest < 0.3 || site.height <= site.water || site.biome != site.home && site.river > 0.0 {
                    continue;
                }
                forested += 1;
                world.ecology.tree_chances(&site, None, x, z, &mut chances);
                if chances.is_empty() {
                    bare.push((x, z, site.temperature, site.soil_moisture()));
                }
            }
            assert!(forested > 50, "seed {seed}: only {forested} forest samples");
            assert!(bare.len() * 50 <= forested, "seed {seed}: {} of {forested} forest samples grow no tree: {:?}", bare.len(), &bare[..bare.len().min(5)]);
        }
    }

    /// Mariano asked for less grass and less tall grass: in the ordinary grassy biomes a
    /// plant stands on well under all of the grass, and tall grass is a minority of them
    /// (the lush meadow aside). One column well inside each biome, found by locating it, so
    /// it does not move when spawn does.
    #[test]
    fn grass_is_kept_in_check() {
        let registry = registry();
        let grass = registry.expect("base:grass_block");
        let world = shared_world(42);
        for biome in ["plains", "forest", "savanna"] {
            let id = world.biomes.id(&format!("base:{biome}")).unwrap();
            let at = world.locate_biome(biome, IVec2::ZERO, 8000).unwrap_or_else(|| panic!("no {biome}"));
            let at = (0..60 * 60)
                .map(|i| spiral(i / 60, i % 60))
                .map(|(dx, dz)| at + IVec2::new(dx * 8, dz * 8))
                .find(|p| {
                    let site = world.site(p.x, p.y);
                    world.biomes.parent_of(site.home) == id && site.height > site.water + 2
                })
                .unwrap_or_else(|| panic!("no dry {biome}"));
            let pos = ColumnPos::of_block(IVec3::new(at.x, 0, at.y));
            let column = world.generate(pos);
            let origin = pos.origin();
            let (mut on_grass, mut planted, mut tall) = (0, 0, 0);
            for z in 0..32 {
                for x in 0..32 {
                    let h = world.height(origin.x + x, origin.z + z);
                    if column.block(IVec3::new(x, h, z)) != grass {
                        continue;
                    }
                    on_grass += 1;
                    let above = column.block(IVec3::new(x, h + 1, z));
                    if registry.info(above).render == voxmar_engine::block::RenderKind::Plant {
                        planted += 1;
                        tall += usize::from(registry.def(above).name.contains("tall_grass"));
                    }
                }
            }
            if on_grass < 200 {
                continue;
            }
            let (cover, share) = (planted as f32 / on_grass as f32, tall as f32 / planted.max(1) as f32);
            println!("{biome} at {at}: cover {cover:.2} of {on_grass}, tall {share:.2}");
            assert!((0.1..0.9).contains(&cover), "{biome} at {at}: a plant on {:.0}% of {on_grass} grass", 100.0 * cover);
            assert!(share < 0.3, "{biome} at {at}: tall grass {:.0}% of {planted} plants", 100.0 * share);
        }
    }

    /// `/locate biome` lands well inside the named biome, not on a sliver at its border or
    /// in a variant within it (on seed 24301 dark forest once sent players to cherry trees).
    #[test]
    fn locate_lands_inside_the_biome() {
        let world = shared_world(24301);
        let spawn = world.spawn_point();
        for name in ["dark_forest", "forest", "taiga"] {
            let Some(at) = world.locate_biome(name, IVec2::new(spawn.x, spawn.z), 4000) else { continue };
            let site = world.site(at.x, at.y);
            let id = world.biomes.id(&format!("base:{name}")).unwrap();
            assert!(site.home == id && site.biome == id, "{name} at {at}: {}", world.biomes.get(site.biome).name);
            assert!(site.edge >= 32.0, "{name} at {at}: only {:.0} blocks from its border", site.edge);
        }
    }

    #[test]
    fn decorations_rest_on_the_ground() {
        let registry = registry();
        // Each biome on the first of its seeds whose world has it near the origin (a new
        // generator version reseeds every world, and rarer biomes come and go).
        let places: [(&[u64], &str); 9] = [
            (&[42, 1], "forest"),
            (&[1, 7, 3, 42], "birch_taiga"),
            (&[3, 1, 42], "dark_forest"),
            (&[7, 3], "forest"),
            (&[11, 7, 1], "taiga"),
            (&[5, 1, 3], "maple_forest"),
            (&[1, 7, 9], "old_growth_taiga"),
            (&[2, 3, 42], "savanna"),
            (&[9, 1], "mixed_forest"),
        ];
        // The places side by side, each on its own worlds.
        use rayon::prelude::*;
        places.into_par_iter().for_each(|(seeds, biome)| {
            // Dry, wooded land well inside the biome, not on a sliver of it at a border, in one
            // of its clearings, or under snow, where few plants grow. A site's home and edge
            // are its sampler's pick, which needs no ground: only a place that passes them
            // has its ground found.
            let found = seeds.iter().find_map(|&seed| {
                let world = shared_world(seed);
                let id = world.biomes.id(&format!("base:{biome}")).unwrap();
                let at = (0..6000 / 64)
                    .flat_map(|ring| (0..(ring * 8).max(1)).map(move |step| spiral(ring, step)))
                    .map(|(dx, dz)| IVec2::new(dx, dz) * 64)
                    .find(|&p| {
                        let sampler = world.sampler(p, p);
                        let pick = sampler.pick(p.x, p.y);
                        pick.home == id && pick.edge >= 48.0 && {
                            let site = sampler.site(p.x, p.y);
                            site.height > SEA_LEVEL + 2 && site.forest > 0.15 && site.temperature > 0.0
                        }
                    })?;
                Some((seed, world, id, at))
            });
            let (seed, world, id, at) = found.unwrap_or_else(|| panic!("no {biome} on seeds {seeds:?}"));
            let at = (0..60 * 60)
                .map(|i| spiral(i / 60, i % 60))
                .map(|(dx, dz)| at + IVec2::new(dx * 8, dz * 8))
                .find(|&p| {
                    let sampler = world.sampler(p, p);
                    sampler.pick(p.x, p.y).home == id && {
                        let site = sampler.site(p.x, p.y);
                        site.height > site.water
                    }
                })
                .unwrap_or_else(|| panic!("seed {seed}: no dry {biome}"));
            let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
            let columns = generate_area(&world, first, 3);
            let origin = first.origin();
            let size = 3 * voxmar_engine::coords::CHUNK_SIZE;
            let inside = |p: IVec3| (0..size).contains(&(p.x - origin.x)) && (0..size).contains(&(p.z - origin.z));
            // The ground's height over the area, sampled once: it is asked for at every block.
            let heights: Vec<i32> = (0..size * size).map(|i| world.height(origin.x + i % size, origin.z + i / size)).collect();
            let ground = |x: i32, z: i32| heights[((z - origin.z) * size + (x - origin.x)) as usize];
            let block = |p: IVec3| {
                let (cx, cz) = ((p.x - origin.x) >> 5, (p.z - origin.z) >> 5);
                columns[(cz * 3 + cx) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31))
            };
            let info = |b: BlockId| registry.info(b);
            let leafy = |b: BlockId| registry.def(b).name.ends_with("leaves");
            // Solid blocks above the ground that are not leaves, nor ice, which floats on the
            // water it froze from.
            let lifted = |p: IVec3| inside(p) && p.y > ground(p.x, p.z) && p.y < WORLD_HEIGHT_FOR_TESTS && {
                let b = block(p);
                info(b).solid && !leafy(b) && b != registry.expect("base:water") && !registry.def(b).name.ends_with("ice")
            };
            let mut seen = rustc_hash::FxHashSet::default();
            let (mut pieces, mut plants) = (0, 0);
            for z in origin.z..origin.z + size {
                for x in origin.x..origin.x + size {
                    let h = ground(x, z);
                    for y in h + 1..(h + 48).min(WORLD_HEIGHT_FOR_TESTS) {
                        let p = IVec3::new(x, y, z);
                        let b = block(p);
                        if info(b).render == voxmar_engine::block::RenderKind::Plant {
                            plants += 1;
                            let below = block(p - IVec3::Y);
                            assert!(info(below).solid || info(below).render == voxmar_engine::block::RenderKind::Plant, "seed {seed}: {} at {p} on {}", registry.def(b).name, registry.def(below).name);
                        }
                        if !lifted(p) || seen.contains(&p) {
                            continue;
                        }
                        // The piece this block belongs to must touch the ground or leave the area.
                        pieces += 1;
                        let (mut stack, mut grounded) = (vec![p], false);
                        seen.insert(p);
                        while let Some(q) = stack.pop() {
                            for d in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                                let n = q + d;
                                if !inside(n) || (n.y <= ground(n.x, n.z) && info(block(n)).solid) {
                                    grounded = true;
                                } else if lifted(n) && seen.insert(n) {
                                    stack.push(n);
                                }
                            }
                        }
                        assert!(grounded, "seed {seed}: {} at {p} floats", registry.def(b).name);
                    }
                }
            }
            assert!(pieces >= 10 && plants > 200, "seed {seed}: only {pieces} pieces and {plants} plants in a {biome} at {at}: {:?}", world.site(at.x, at.y));
        });
    }

    /// What rests on nothing in the inner columns of `count`×`count` columns from `first`:
    /// pieces of wood (trunks, roots, branches, stumps, fallen logs, cacti, mushroom stems)
    /// none of whose blocks, joined face to face through other wood, has firm ground beneath
    /// it (any solid block but wood and leaves), and clumps of leaves that join no wood and
    /// neither rest on firm ground nor touch it from the side. Pieces reaching the edge of the generated area, and the log
    /// decks of trail bridges, are left out. Each comes as its lowest block and its size.
    fn floating_wood(world: &Overworld, registry: &BlockRegistry, first: ColumnPos, count: i32) -> Vec<(IVec3, String, usize)> {
        use voxmar_engine::coords::{CHUNK_SIZE, WORLD_HEIGHT};
        let columns = generate_area(world, first, count);
        let origin = first.origin();
        let size = count * CHUNK_SIZE;
        let block = |p: IVec3| {
            let (cx, cz) = ((p.x - origin.x) >> 5, (p.z - origin.z) >> 5);
            columns[(cz * count + cx) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31))
        };
        let names: Vec<&str> = (0..registry.len()).map(|i| registry.def(BlockId(i as u16)).name.as_str()).collect();
        let wood = |b: BlockId| {
            let name = names[b.0 as usize];
            // (Hanging roots hang from the rock of cave ceilings.)
            (["_log", "_wood", "_roots", "_stem"].iter().any(|s| name.ends_with(s)) && !name.contains("hanging")) || name.ends_with("cactus") || name.contains("glowcap_cap")
        };
        let leafy = |b: BlockId| names[b.0 as usize].ends_with("leaves");
        let firm = |b: BlockId| registry.info(b).solid && !wood(b) && !leafy(b);
        let edge = |p: IVec3| p.x - origin.x < 1 || p.z - origin.z < 1 || p.x - origin.x >= size - 1 || p.z - origin.z >= size - 1;
        let deck = |p: IVec3| world.geo(p.x, p.z).trail.is_some_and(|t| t.deck.is_some());
        let mut floating = Vec::new();
        // Wood first, then leaves through wood: a clump of leaves is fine wherever it holds
        // wood, which was checked on its own.
        for leaves in [false, true] {
            let joins = |b: BlockId| wood(b) || (leaves && leafy(b));
            let mut seen = rustc_hash::FxHashSet::default();
            // Inner columns only start pieces; the fill may wander anywhere in the area.
            for z in origin.z + CHUNK_SIZE..origin.z + size - CHUNK_SIZE {
                for x in origin.x + CHUNK_SIZE..origin.x + size - CHUNK_SIZE {
                    for y in 1..WORLD_HEIGHT {
                        let p = IVec3::new(x, y, z);
                        let b = block(p);
                        if seen.contains(&p) || !(if leaves { leafy(b) } else { wood(b) }) {
                            continue;
                        }
                        // (Leaves on cave floors are the cave biomes', underground.)
                        let (mut stack, mut piece, mut rests, mut skip) = (vec![p], Vec::new(), false, leaves && p.y <= world.height(p.x, p.z));
                        seen.insert(p);
                        while let Some(q) = stack.pop() {
                            piece.push(q);
                            skip |= edge(q) || (wood(block(q)) && (leaves || deck(q)));
                            // Wood rests on what lies beneath it; leaves may also grow
                            // against a rock face beside them.
                            rests |= firm(block(q - IVec3::Y)) || (leaves && !edge(q) && [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].iter().any(|&d| firm(block(q + d))));
                            for d in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                                let n = q + d;
                                if !edge(q) && (1..WORLD_HEIGHT).contains(&n.y) && joins(block(n)) && seen.insert(n) {
                                    stack.push(n);
                                }
                            }
                        }
                        if !rests && !skip {
                            let low = *piece.iter().min_by_key(|q| (q.y, q.x, q.z)).unwrap();
                            floating.push((low, names[block(low).0 as usize].to_string(), piece.len()));
                        }
                    }
                }
            }
        }
        floating
    }

    /// `cargo test --profile iter -p voxmar_worldgen forest_preview -- --ignored --nocapture`
    /// (with `SEED`, and `BIOMES=forest,taiga` or `X`, `Z`; `SIZE` columns square, 2 by
    /// default; `SCALE` pixels a block, 5) generates the columns around the nearest place well inside each biome (or
    /// around X, Z) and draws them in an isometric view from the south-east, coloured by
    /// block, with grass and leaves in the colours their columns carry, to
    /// `target/forest_NAME.ppm`: for judging stands of trees, their heights and colours,
    /// without the game.
    #[test]
    #[ignore]
    fn forest_preview() {
        use voxmar_engine::block::{RenderKind, Tint};
        use voxmar_engine::coords::{CHUNK_SIZE, WORLD_HEIGHT};
        use voxmar_engine::glam::{Vec2, Vec3};
        let env = |name: &str| std::env::var(name).ok();
        let seed: u64 = env("SEED").and_then(|s| s.parse().ok()).unwrap_or(42);
        let size: i32 = env("SIZE").and_then(|s| s.parse().ok()).unwrap_or(2);
        let registry = registry();
        let world = Overworld::new(seed, registry.clone());
        let spawn = world.spawn_point();
        let mut places: Vec<(String, IVec2)> = Vec::new();
        if let Some(names) = env("BIOMES") {
            for name in names.split(',') {
                let Some(id) = world.biomes.id(&format!("base:{name}")) else { continue };
                let found = world.locate_biome(name, IVec2::new(spawn.x, spawn.z), 6000).and_then(|at| {
                    (0..400).map(|i| spiral(i / 16, i % 16)).map(|(dx, dz)| at + IVec2::new(dx * 16, dz * 16)).find(|p| {
                        let site = world.site(p.x, p.y);
                        site.home == id && site.edge > 24.0 && site.height > site.water && site.slope <= 2
                    })
                });
                match found {
                    Some(at) => places.push((name.to_string(), at)),
                    None => println!("{name}: not found"),
                }
            }
        } else {
            let x = env("X").and_then(|s| s.parse().ok()).unwrap_or(spawn.x);
            let z = env("Z").and_then(|s| s.parse().ok()).unwrap_or(spawn.z);
            places.push((format!("{x}_{z}"), IVec2::new(x, z)));
        }
        // Faces seen from +x, +y, +z, as in `trees::tests::tree_preview`, `SCALE` pixels
        // to a block.
        let a: f32 = env("SCALE").and_then(|s| s.parse().ok()).unwrap_or(5.0);
        let project = |p: Vec3| Vec2::new((p.x - p.z) * a, (p.x + p.z) * a * 0.5 - p.y * a);
        let quads = [
            [Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
            [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 0.0, 1.0)],
            [Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
        ];
        let inside = |q: &[Vec2; 4], p: Vec2| {
            let sides: Vec<f32> = (0..4).map(|i| (q[(i + 1) % 4] - q[i]).perp_dot(p - q[i])).collect();
            sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
        };
        let mut sprite: Vec<(i32, i32, usize)> = Vec::new();
        for py in -16..16 {
            for px in -16..16 {
                let p = Vec2::new(px as f32 + 0.5, py as f32 + 0.5);
                if let Some(face) = (0..3).find(|&f| inside(&quads[f].map(project), p)) {
                    sprite.push((px, py, face));
                }
            }
        }
        for (name, at) in places {
            let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-size / 2, -size / 2);
            let columns = generate_area(&world, first, size);
            let origin = first.origin();
            let span = size * CHUNK_SIZE;
            let column = |p: IVec3| &columns[(((p.z - origin.z) >> 5) * size + ((p.x - origin.x) >> 5)) as usize];
            let block = |p: IVec3| {
                let inside = (0..span).contains(&(p.x - origin.x)) && (0..span).contains(&(p.z - origin.z)) && (0..WORLD_HEIGHT).contains(&p.y);
                if inside { column(p).block(p.with_x(p.x & 31).with_z(p.z & 31)) } else { BlockId::AIR }
            };
            let low = (0..span * span).map(|i| world.height(origin.x + i % span, origin.z + i / span)).min().unwrap() - 2;
            let high = (low + 90).min(WORLD_HEIGHT - 1);
            let corner = |p: Vec3| project(p - Vec3::new(origin.x as f32, low as f32, origin.z as f32));
            let (w, h) = ((2.0 * span as f32 * a) as i32 + 16, (span as f32 * a + (high - low) as f32 * a) as i32 + 16);
            let shift = Vec2::new(span as f32 * a + 8.0, (high - low) as f32 * a + 8.0);
            let mut image = vec![[200u8, 214, 230]; (w * h) as usize];
            let open = |b: BlockId| b.is_air() || registry.def(b).render == RenderKind::Plant || registry.def(b).render == RenderKind::Leaves;
            for sum in 0..(2 * span + (high - low)) {
                for y in low..=high {
                    for dz in 0..span {
                        let dx = sum - (y - low) - dz;
                        if !(0..span).contains(&dx) {
                            continue;
                        }
                        let p = IVec3::new(origin.x + dx, y, origin.z + dz);
                        let b = block(p);
                        if b.is_air() || ![IVec3::X, IVec3::Y, IVec3::Z].iter().any(|&d| open(block(p + d))) {
                            continue;
                        }
                        let def = registry.def(b);
                        let colors = column(p).colors_at(p.x & 31, p.z & 31);
                        let tint = |c: [u8; 3]| Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32);
                        let base = match def.tint.drawn() {
                            Tint::Foliage => tint(colors[1]) * 0.75,
                            Tint::Grass => tint(colors[0]) * 0.8,
                            Tint::Water => tint(colors[2]),
                            Tint::None => tint(def.color),
                        };
                        let speckle = 0.9 + 0.2 * ((crate::rng::hash3(3, p.x, p.y, p.z) & 0xff) as f32 / 255.0);
                        let at = corner(p.as_vec3()) + shift;
                        for &(px, py, face) in &sprite {
                            let (x, y) = (at.x as i32 + px, at.y as i32 + py);
                            if x < 0 || y < 0 || x >= w || y >= h {
                                continue;
                            }
                            let c = (base * [1.0, 0.8, 0.62][face] * speckle).min(Vec3::splat(255.0));
                            image[(y * w + x) as usize] = [c.x as u8, c.y as u8, c.z as u8];
                        }
                    }
                }
            }
            let mut out = format!("P6 {w} {h} 255\n").into_bytes();
            out.extend(image.iter().flatten());
            let path = format!("{}/../../target/forest_{name}.ppm", env!("CARGO_MANIFEST_DIR"));
            std::fs::write(&path, out).unwrap();
            println!("{name} at {}, {}: {path}", at.x, at.y);
        }
    }

    /// No tree reaches over a hole the caves carved beside its trunk: roots and thick-trunk
    /// feet reach the columns next to the candidate, and a pit there can be deeper than a
    /// foot sinks. Seed 22's sinkhole near (319, 2061) had a dark oak whose flare root, two
    /// columns out, hung over its rim.
    #[test]
    fn trees_keep_off_the_rims_of_cave_mouths() {
        let registry = registry();
        let world = shared_world(22);
        let first = ColumnPos::of_block(IVec3::new(319, 0, 2061)).offset(-2, -2);
        let floating = floating_wood(&world, &registry, first, 5);
        assert!(floating.is_empty(), "wood floats at the sinkhole's rim: {floating:?}");
    }

    /// Bare-capable herbs still need actual earth. Both plant paths take the same
    /// admitted surface: rock, gravel, ice and snow work; removed ground, water and
    /// wooden obstacles do not become a Bare surface to grow on.
    #[test]
    fn ground_plants_require_actual_earth() {
        use crate::ecology::Plant;
        let registry = registry();
        let world = shared_world(1);
        let meadow = world.biomes.id("base:alpine_meadow").unwrap();
        let site = Site { height: 201, water: 200, body: Body::Lake, home: meadow, biome: meadow,
            forest: 0.0, trail: 0.0, ..world.site(0, 0) };
        let looks = plant_looks(&["edelweiss", "windroot"]);
        for (name, supported) in [("air", false), ("water", false), ("oak_wood", false),
            ("oak_leaves", false), ("short_grass", false), ("stone", true),
            ("gravel", true), ("ice", true), ("snowy_grass_block", true), ("snow_block", true)]
        {
            let floor = if name == "air" { BlockId::AIR } else { registry.expect(&format!("base:{name}")) };
            let ground = world.terrain.plant_ground(&site, floor, BlockId::AIR);
            let (mut ordinary, mut bank) = (0, 0);
            for cell in 0..256 {
                let first = IVec2::new(cell % 16 * 32, cell / 16 * 32);
                bank += world.ecology.edge_patches(first, |_| ground.map(|g| (&site, g))).len();
                for i in 0..32 * 32 {
                    let Some(ground) = ground else { continue };
                    match world.ecology.plant(&site, ground, first.x + i % 32, first.y + i / 32) {
                        Some(Plant::One(b) | Plant::Tall(b, _) | Plant::Stack(b, _)) if looks.contains_key(&b) => ordinary += 1,
                        _ => {}
                    }
                }
            }
            assert_eq!((ordinary > 0, bank > 0), (supported, supported), "{name}: {ordinary} ordinary patch plants, {bank} bank patch plants");
            assert_eq!(ground.is_some(), supported, "{name} admitted as ground");
        }
        let stone = registry.expect("base:stone");
        assert!(world.terrain.plant_ground(&site, stone, stone).is_none(), "an occupied surface");
        assert!(world.terrain.plant_ground(&Site { height: 199, ..site }, stone, BlockId::AIR).is_none(), "under water");
        assert!(world.terrain.plant_ground(&Site { trail: 1.0, ..site }, stone, BlockId::AIR).is_none(), "on a trail");
    }

    /// Actual carving removes ground at the seed-22 mouth; analytic height alone must
    /// not admit a plant there. Also retain admitted earth nearby and check the plants
    /// that generation really placed, so the mouth fixture cannot become vacuous.
    #[test]
    fn ground_plants_keep_off_carved_mouths() {
        let registry = registry();
        let world = shared_world(22);
        let first = ColumnPos::of_block(IVec3::new(319, 0, 2061)).offset(-1, -1);
        let area = generate_area(&world, first, 3);
        let origin = first.origin();
        let (mut holes, mut earth, mut plants) = (0, 0, 0);
        for z in 0..96 {
            for x in 0..96 {
                let site = world.site(origin.x + x, origin.z + z);
                if site.height + 1 >= WORLD_HEIGHT_FOR_TESTS {
                    continue;
                }
                let column = &area[((z >> 5) * 3 + (x >> 5)) as usize];
                let p = IVec3::new(x & 31, site.height, z & 31);
                let (below, above) = (column.block(p), column.block(p + IVec3::Y));
                if site.height >= site.water && below.is_air() && above.is_air() && site.trail <= 0.2 {
                    holes += 1;
                    assert!(world.terrain.plant_ground(&site, below, above).is_none(), "a carved mouth at {} {}", origin.x + x, origin.z + z);
                }
                earth += usize::from(world.terrain.plant_ground(&site, below, above).is_some());
                if registry.info(above).render == voxmar_engine::block::RenderKind::Plant {
                    plants += 1;
                    assert!(crate::terrain::earth(&registry, below), "{} above {} at {} {}", registry.def(above).name, registry.def(below).name, origin.x + x, origin.z + z);
                }
            }
        }
        assert!(holes > 0 && earth > 100 && plants > 0, "{holes} holes, {earth} admitted surfaces, {plants} plants");
    }

    /// Every trunk, root, stump, fallen log, cactus and mushroom stem rests on the ground,
    /// branches through their trunks: on several seeds, in forests, at cave mouths, ravines
    /// and sinkholes, by rivers and in the biomes of snags and cacti, where carving and
    /// uneven ground could leave wood over a hole.
    #[test]
    fn wood_never_floats() {
        use rayon::prelude::*;
        let registry = registry();
        // The seeds, their places and the biomes looked for side by side; the failures in
        // the same order as one after another.
        let seeds: Vec<(Vec<String>, usize)> = [1u64, 7, 22, 42, 0x5eed]
            .into_par_iter()
            .map(|seed| {
                let world = shared_world(seed);
                let spawn = world.spawn_point();
                let mut at: Vec<(String, IVec2)> = vec![("spawn".into(), IVec2::new(spawn.x, spawn.z))];
                for (name, x, z) in world.caves.sights((spawn.x, spawn.z), 1500) {
                    if matches!(name, "entrances" | "ravine" | "sinkhole") {
                        at.push((name.into(), IVec2::new(x, z)));
                    }
                }
                let biomes = ["forest", "old_growth_taiga", "wooded_badlands", "bayou", "mangrove_swamp", "jungle", "baobab_savanna", "snowy_taiga", "ghost_forest", "steppe", "meadow", "redwood_forest", "dark_forest", "glowcap_forest", "desert"];
                let found: Vec<_> = biomes.into_par_iter().map(|biome| world.locate_biome(biome, IVec2::new(spawn.x, spawn.z), 4000).map(|p| (biome.to_owned(), p))).collect();
                at.extend(found.into_iter().flatten());
                let failures: Vec<String> = at
                    .par_iter()
                    .flat_map_iter(|(name, p)| {
                        let first = ColumnPos::of_block(IVec3::new(p.x, 0, p.y)).offset(-2, -2);
                        floating_wood(&world, &registry, first, 5).into_iter().map(move |(low, block, size)| format!("seed {seed} {name}: {size} {block} from {low}"))
                    })
                    .collect();
                (failures, at.len())
            })
            .collect();
        let places: usize = seeds.iter().map(|(_, n)| n).sum();
        let failures: Vec<String> = seeds.into_iter().flat_map(|(f, _)| f).collect();
        assert!(failures.is_empty(), "wood floats in {} of {places} places:\n{}", failures.len(), failures.join("\n"));
    }

    /// Water crossing region borders never runs round in a circle that generation has to
    /// break wherever it happened to start. On these seeds' land, regions that resolved a
    /// lake on their shared border differently once passed water round in circles: river
    /// sizes then depended on which column was made first, and with regions built on several
    /// threads, two threads could each wait on the other's half of a circle forever (the
    /// geode test hung). The area is the geode test's, built on helper threads as in play.
    #[test]
    fn rivers_never_loop_across_regions() {
        let registry = registry();
        for seed in [5, 1, 42, 7, 3, 11] {
            let world = Overworld::new(seed, registry.clone());
            for i in 0..400 {
                let origin = ColumnPos::new(i % 20 - 10, i / 20 - 10).origin();
                geology::GeoGrid::new(&world, (origin.x - 1, origin.z - 1), voxmar_engine::coords::CHUNK_SIZE + 2);
            }
        }
        assert_eq!(water::drainage_loops_ended(), 0, "water ran round in a circle across region borders");
    }

    /// A new player finds flint lying loose near where they start, whatever the seed
    /// (Mariano, Oct 15: bare hands take only loose things; gravel, the other flint, lay
    /// hundreds of blocks off on some seeds): within 64 blocks of spawn, at least the four
    /// nodules a flint axe and pickaxe take. Prints each seed's count.
    #[test]
    fn flint_lies_near_every_spawn() {
        use rayon::prelude::*;
        let registry = registry();
        let flint = registry.expect("base:loose_flint");
        let seeds = [1u64, 2, 3, 4, 8, 12, 16, 42, 100, 777, 0x5eed];
        let found: Vec<(u64, u32)> = seeds
            .par_iter()
            .map(|&seed| {
                let world = shared_world(seed);
                let spawn = world.spawn_point();
                let first = ColumnPos::of_block(spawn).offset(-2, -2);
                let mut near = 0;
                for (i, column) in generate_area(&world, first, 5).iter().enumerate() {
                    let origin = first.offset(i as i32 % 5, i as i32 / 5).origin();
                    for (x, z) in (0..CHUNK_SIZE).flat_map(|x| (0..CHUNK_SIZE).map(move |z| (x, z))) {
                        let (dx, dz) = (origin.x + x - spawn.x, origin.z + z - spawn.z);
                        if dx * dx + dz * dz > 64 * 64 {
                            continue;
                        }
                        near += (1..WORLD_HEIGHT_FOR_TESTS).filter(|&y| column.block(IVec3::new(x, y, z)) == flint).count() as u32;
                    }
                }
                (seed, near)
            })
            .collect();
        println!("loose flint within 64 blocks of spawn: {found:?}");
        let short: Vec<&(u64, u32)> = found.iter().filter(|&&(_, n)| n < 4).collect();
        assert!(short.is_empty(), "seeds with fewer than four loose flint within 64 blocks of spawn: {short:?} (all: {found:?})");
    }

    /// Features reaching across column borders (trees, logs, rocks) come out the same
    /// whichever column is generated first, on whichever thread.
    #[test]
    fn decorations_never_depend_on_generation_order() {
        let world = Overworld::new(42, registry());
        let at = world.locate_biome("forest", IVec2::ZERO, 6000).unwrap();
        let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
        let together = generate_area(&world, first, 3);
        for i in (0..9).rev() {
            assert!(world.generate(first.offset(i % 3, i / 3)) == together[i as usize], "column {i} differs");
        }
    }

    /// Flowers in 9 columns (a 3×3 area) well inside each biome near the origin, averaged over
    /// a few seeds: `BIOMES=plains,meadow cargo test --profile iter -p voxmar_worldgen
    /// flower_counts -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn flower_counts() {
        let registry = registry();
        let names = std::env::var("BIOMES").unwrap_or_else(|_| "plains,meadow,flower_meadow,forest,taiga,jungle,savanna,swamp".into());
        let seeds = [1u64, 7, 42];
        let worlds: Vec<Overworld> = seeds.iter().map(|&s| Overworld::new(s, registry.clone())).collect();
        for name in names.split(',') {
            let (mut flowers, mut plants, mut areas) = (0, 0, 0);
            let mut kinds = std::collections::BTreeMap::<String, i32>::new();
            for world in &worlds {
                let Some(at) = world.locate_biome(name, IVec2::ZERO, 8000) else { continue };
                let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
                for column in generate_area(world, first, 3) {
                    for z in 0..32 {
                        for x in 0..32 {
                            for y in (1..WORLD_HEIGHT_FOR_TESTS).rev() {
                                let b = column.block(IVec3::new(x, y, z));
                                if b.is_air() {
                                    continue;
                                }
                                if registry.info(b).render == voxmar_engine::block::RenderKind::Plant {
                                    plants += 1;
                                }
                                if registry.def(b).has_tag("flower") && registry.info(b).render == voxmar_engine::block::RenderKind::Plant {
                                    flowers += 1;
                                    *kinds.entry(registry.def(b).name.clone()).or_default() += 1;
                                }
                                if registry.info(b).render != voxmar_engine::block::RenderKind::Plant {
                                    break;
                                }
                            }
                        }
                    }
                }
                areas += 1;
            }
            let mut top: Vec<_> = kinds.into_iter().collect();
            top.sort_by_key(|k| -k.1);
            top.truncate(4);
            let per = |n: i32| n as f32 / areas.max(1) as f32;
            println!("{name}: {:.0} plants, {:.0} flowers per 9 columns ({areas} seeds); most flowers: {top:?}", per(plants), per(flowers));
        }
    }

    /// The surface of each biome, block by block, for judging whether it reads as one
    /// palette: `BIOMES=plains,forest SEEDS=7,1 cargo test --profile iter -p
    /// voxmar_worldgen surface_census -- --ignored --nocapture`. In 9 columns (3×3 chunks)
    /// well inside the biome it counts the ground's top blocks, the solid blocks standing on
    /// the ground that are not trees (boulders, rocks, stones), and of those the lone cubes:
    /// one block with nothing solid beside or on it.
    #[test]
    #[ignore]
    fn surface_census() {
        use voxmar_engine::block::RenderKind;
        let registry = registry();
        let names = std::env::var("BIOMES").unwrap_or_else(|_| "plains,forest,meadow,taiga,savanna,desert,moonlit_meadow".into());
        let seeds: Vec<u64> = std::env::var("SEEDS").unwrap_or_else(|_| "7,1".into()).split(',').filter_map(|s| s.parse().ok()).collect();
        let worlds: Vec<Overworld> = seeds.iter().map(|&s| Overworld::new(s, registry.clone())).collect();
        let tree = |b: BlockId| {
            let d = registry.def(b);
            ["_log", "_wood", "_stem", "_leaves", "_cap", "cactus", "_roots"].iter().any(|s| d.name.ends_with(s)) || registry.info(b).render == RenderKind::Leaves
        };
        let solid = |b: BlockId| !b.is_air() && registry.info(b).solid && registry.info(b).render != RenderKind::Plant && registry.fluid(b).is_none();
        let top = |m: &std::collections::BTreeMap<String, i32>, n: usize| {
            let mut v: Vec<_> = m.iter().map(|(k, c)| (k.trim_start_matches("base:").to_owned(), *c)).collect();
            v.sort_by_key(|k| -k.1);
            v.truncate(n);
            v
        };
        for name in names.split(',') {
            let mut ground = std::collections::BTreeMap::<String, i32>::new();
            let mut above = std::collections::BTreeMap::<String, i32>::new();
            let mut lone = std::collections::BTreeMap::<String, i32>::new();
            let mut areas = 0;
            for world in &worlds {
                let Some(id) = world.biomes.id(&format!("base:{name}")) else { continue };
                let Some(at) = world.locate_biome(name, IVec2::ZERO, 8000) else { continue };
                let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
                let columns = generate_area(world, first, 3);
                let origin = first.origin();
                let span = 3 * voxmar_engine::coords::CHUNK_SIZE;
                let block = |p: IVec3| {
                    if !(0..span).contains(&(p.x - origin.x)) || !(0..span).contains(&(p.z - origin.z)) || !(0..WORLD_HEIGHT_FOR_TESTS).contains(&p.y) {
                        return BlockId::AIR;
                    }
                    columns[(((p.z - origin.z) >> 5) * 3 + ((p.x - origin.x) >> 5)) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31))
                };
                for z in origin.z..origin.z + span {
                    for x in origin.x..origin.x + span {
                        let site = world.site(x, z);
                        if site.home != id {
                            continue;
                        }
                        let h = site.height;
                        *ground.entry(registry.def(block(IVec3::new(x, h, z))).name.clone()).or_default() += 1;
                        for y in h + 1..h + 6 {
                            let p = IVec3::new(x, y, z);
                            let b = block(p);
                            if !solid(b) || tree(b) {
                                continue;
                            }
                            *above.entry(registry.def(b).name.clone()).or_default() += 1;
                            let beside = [IVec3::X, -IVec3::X, IVec3::Z, -IVec3::Z, IVec3::Y].iter().any(|&d| { let n = block(p + d); solid(n) && !tree(n) });
                            if !beside && y == h + 1 {
                                // Where: by a stream, on a slope or on open ground.
                                let place = if site.river >= 0.5 { "stream" } else if site.slope >= 2 { "slope" } else { "open" };
                                *lone.entry(format!("{}@{place}", registry.def(b).name)).or_default() += 1;
                            }
                        }
                    }
                }
                areas += 1;
            }
            let per = |m: &std::collections::BTreeMap<String, i32>| m.values().sum::<i32>() as f32 / areas.max(1) as f32;
            println!("{name} ({areas} areas): ground {:?}", top(&ground, 6));
            println!("  on the ground {:.0}/area: {:?}", per(&above), top(&above, 8));
            println!("  lone cubes {:.0}/area: {:?}", per(&lone), top(&lone, 8));
        }
    }

    /// Every look of each plant's block (pairs, clumps, its snowy look), as the plant's index
    /// in `plants`: the blocks that count as it.
    fn plant_looks(plants: &[&str]) -> std::collections::HashMap<BlockId, usize> {
        let registry = registry();
        let content = crate::content::WorldContent::base(&registry).unwrap();
        let mut looks = std::collections::HashMap::new();
        for (i, name) in plants.iter().enumerate() {
            let def = content.plants.iter().find(|p| p.name.trim_start_matches("base:") == *name).unwrap_or_else(|| panic!("no plant {name}"));
            let block = registry.expect(&def.block);
            for id in registry.looks(block).iter().map(|l| l.block).chain([block]).chain(def.snowy.as_deref().map(|n| registry.expect(n))) {
                looks.insert(id, i);
            }
        }
        looks
    }

    /// One area of a biome as generated: `chunks` columns of chunks square around the
    /// nearest place of `biome` to `from` in `world`, with its columns (dry land of that home biome)
    /// and where each of `count` plants (by `looks`) stands on them, after trees, trails,
    /// boulders and water; none where the world has no such biome near.
    fn biome_area(world: &Overworld, biome: &str, from: IVec2, chunks: i32, looks: &std::collections::HashMap<BlockId, usize>, count: usize) -> Option<(u64, u64, Vec<Vec<(i32, i32)>>, Vec<usize>)> {
        let id = world.biomes.id(&format!("base:{biome}"))?;
        let at = world.locate_biome(biome, from, 60000)?;
        let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-chunks / 2, -chunks / 2);
        // One sampler a column of chunks, caching its home-biome picks.
        let sampler = |pos: ColumnPos| {
            let corner = pos.origin();
            world.dense_sampler(IVec2::new(corner.x, corner.z), IVec2::new(corner.x + 31, corner.z + 31))
        };
        // Only the columns of chunks the biome reaches into (seen every four blocks) are
        // generated: a biome in scattered pockets costs no more than its own land.
        use rayon::prelude::*;
        let positions: Vec<ColumnPos> = (0..chunks * chunks)
            .into_par_iter()
            .map(|i| first.offset(i % chunks, i / chunks))
            .filter(|&pos| {
                let (sampler, corner) = (sampler(pos), pos.origin());
                (0..64).any(|k| sampler.pick(corner.x + k % 8 * 4 + 2, corner.z + k / 8 * 4 + 2).home == id)
            })
            .collect();
        let area = generate_columns(world, &positions);
        // Each column of chunks counted on its own core, summed in order.
        let counted: Vec<(u64, u64, Vec<Vec<(i32, i32)>>, Vec<usize>)> = positions.par_iter().zip(&area).map(|(&pos, column)| {
            let (mut columns, mut banks, mut placed, mut off) = (0u64, 0u64, vec![Vec::new(); count], vec![0; count]);
            let corner = pos.origin();
            let sampler = sampler(pos);
            // A census needs home, height and water only. Sample the full river area once,
            // including crag tops, rather than 25 geology positions for every land column.
            // Like Sampler::site, this excludes later trail/settlement height adjustments.
            let geology = world.rivers.area(world.stages(), IVec2::new(corner.x, corner.z), 32);
            for z in 0..32 {
                for x in 0..32 {
                    let geo = &geology[(z * 32 + x) as usize];
                    if sampler.pick(corner.x + x, corner.z + z).home != id || geo.height < geo.water || geo.height + 1 >= WORLD_HEIGHT_FOR_TESTS {
                        continue;
                    }
                    columns += 1;
                    // A lake's bank (`Habitat::LakeShore`), for the herbs that grow along it,
                    // and each herb's plants off it.
                    let bank = geo.body == Body::Lake && geo.height - geo.water <= 2;
                    banks += u64::from(bank);
                    if let Some(&k) = looks.get(&column.block(IVec3::new(x, geo.height + 1, z))) {
                        placed[k].push((corner.x + x, corner.z + z));
                        off[k] += usize::from(!bank);
                    }
                }
            }
            (columns, banks, placed, off)
        }).collect();
        let (mut columns, mut banks, mut placed, mut off) = (0u64, 0u64, vec![Vec::new(); count], vec![0; count]);
        for (n, bank, found, away) in counted {
            columns += n;
            banks += bank;
            off.iter_mut().zip(away).for_each(|(all, away)| *all += away);
            for (all, found) in placed.iter_mut().zip(found) {
                all.extend(found);
            }
        }
        Some((columns, banks, placed, off))
    }

    /// Census batching must retain exactly the old site's home, height and water,
    /// including wet banks and crag tops; these fields choose the denominator and block.
    #[test]
    fn biome_census_keeps_site_fields() {
        let (mut wet, mut crags) = (0, 0);
        for (seed, at) in [(2, IVec2::new(820, 124)), (2, IVec2::new(-2660, -564)), (42, IVec2::new(-306, -70)), (22, IVec2::new(319, 2061))] {
            let world = shared_world(seed);
            let crag = (0..160 * 160).map(|i| IVec2::new(i % 160 * 50 - 4000, i / 160 * 50 - 4000)).find(|p| world.ground(p.x, p.y).crag.amplitude > 6.0);
            for at in std::iter::once(at).chain(crag) {
                let corner = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).origin();
                let sampler = world.dense_sampler(IVec2::new(corner.x, corner.z), IVec2::new(corner.x + 31, corner.z + 31));
                let area = world.rivers.area(world.stages(), IVec2::new(corner.x, corner.z), 32);
                for z in 0..32 {
                    for x in 0..32 {
                        let geo = &area[(z * 32 + x) as usize];
                        let (wx, wz) = (corner.x + x, corner.z + z);
                        let site = sampler.site(wx, wz);
                        assert_eq!((sampler.pick(wx, wz).home, geo.height, geo.water), (site.home, site.height, site.water), "census fields differ on seed {seed} at ({wx}, {wz})");
                        wet += (geo.height < geo.water) as usize;
                        crags += (geo.crag.amplitude > 6.0) as usize;
                    }
                }
            }
        }
        assert!(wet > 0 && crags > 0, "comparison missed wet or crag land: wet={wet}, crags={crags}");
    }

    /// Visible plant clusters: connected components with neighbouring plants within
    /// four blocks. Merge groups a new plant bridges, so visit order cannot split one.
    fn patches(placed: &[(i32, i32)]) -> Vec<Vec<(i32, i32)>> {
        let mut patches: Vec<Vec<(i32, i32)>> = Vec::new();
        for &p in placed {
            let mut group = vec![p];
            let mut i = 0;
            while i < patches.len() {
                if patches[i].iter().any(|q| (q.0 - p.0).abs() <= 4 && (q.1 - p.1).abs() <= 4) {
                    group.extend(patches.remove(i));
                } else {
                    i += 1;
                }
            }
            patches.push(group);
        }
        patches
    }

    #[test]
    fn plant_clusters_merge_bridges_in_any_order() {
        let plants = [(0, 0), (0, 8), (4, 4)];
        for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
            let placed: Vec<_> = order.into_iter().map(|i| plants[i]).collect();
            assert_eq!(patches(&placed).len(), 1, "split bridge in {placed:?}");
        }
        assert_eq!(patches(&[(0, 0), (0, 9), (4, 4)]).len(), 2);
    }

    /// Where in a world plant counts look for a biome: around the nearest place of it to each
    /// of these, far apart (`PLACES=n` takes the first n for the census; one by default).
    const PLACES: [IVec2; 3] = [IVec2::ZERO, IVec2::new(24000, 8000), IVec2::new(-8000, 24000)];

    fn places() -> usize {
        std::env::var("PLACES").ok().and_then(|s| s.parse().ok()).unwrap_or(1)
    }

    /// Plants as they are actually placed (`biome_area`), around the nearest places of each
    /// biome given in each seed's world, per million columns of that biome, and the size of
    /// their patches. One world at a time, its land generated once for all the plants of a
    /// biome. Returns (plant, biome, rate, plants a patch, plants, columns).
    fn plant_counts(seeds: &[u64], chunks: i32, plants: &[&str], biomes: &[&str]) -> Vec<(String, String, f64, f64, usize, u64)> {
        let looks = plant_looks(plants);
        // Per biome: columns, and per plant its plants, patches and biggest patch (with its seed).
        let mut tally: Vec<(u64, Vec<(usize, usize, Option<(usize, u64, (i32, i32))>)>)> = vec![(0, vec![(0, 0, None); plants.len()]); biomes.len()];
        for &seed in seeds {
            let world = shared_world(seed);
            for (b, biome) in biomes.iter().enumerate() {
                for &from in PLACES.iter().take(places()) {
                    let Some((columns, _, placed, _)) = biome_area(&world, biome, from, chunks, &looks, plants.len()) else { continue };
                    tally[b].0 += columns;
                    for (k, placed) in placed.iter().enumerate() {
                        let groups = patches(placed);
                        let t = &mut tally[b].1[k];
                        t.0 += placed.len();
                        t.1 += groups.len();
                        if let Some(g) = groups.iter().max_by_key(|g| g.len())
                            && t.2.is_none_or(|(n, _, _)| g.len() > n)
                        {
                            t.2 = Some((g.len(), seed, g[0]));
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (b, biome) in biomes.iter().enumerate() {
            let columns = tally[b].0;
            for (k, &(n, groups, biggest)) in tally[b].1.iter().enumerate() {
                // AT=1 shows where the biggest patch is, for shots.
                if std::env::var("AT").is_ok()
                    && let Some((size, seed, at)) = biggest
                {
                    println!("  {} in {biome}: a patch of {size} at {at:?} (seed {seed})", plants[k]);
                }
                let rate = n as f64 * 1e6 / columns.max(1) as f64;
                let size = if groups == 0 { 0.0 } else { n as f64 / groups as f64 };
                out.push((plants[k].to_owned(), (*biome).to_owned(), rate, size, n, columns));
            }
        }
        out
    }

    /// The rare herbs (`named_only`) and the biomes naming each, as plant_counts takes them,
    /// with each pair it should report.
    fn rare_herbs() -> (Vec<String>, Vec<String>, Vec<(String, String)>) {
        let registry = registry();
        let content = crate::content::WorldContent::base(&registry).unwrap();
        let (mut herbs, mut biomes, mut pairs) = (Vec::new(), Vec::new(), Vec::new());
        for herb in content.plants.iter().filter(|p| p.named_only) {
            let named = |list: &[crate::content::Select]| list.iter().any(|s| matches!(s, crate::content::Select::Name(n, _) if *n == herb.name));
            let name = herb.name.trim_start_matches("base:").to_owned();
            for biome in content.biomes.iter().filter(|b| named(&b.flora.plants) || b.flora.wonders.as_deref().is_some_and(named)) {
                let short = biome.name.trim_start_matches("base:").to_owned();
                if !biomes.contains(&short) {
                    biomes.push(short.clone());
                }
                pairs.push((name.clone(), short));
            }
            herbs.push(name);
        }
        (herbs, biomes, pairs)
    }

    fn rare_herb_counts(seeds: &[u64], chunks: i32) -> Vec<(String, String, f64, f64, usize, u64)> {
        let (herbs, biomes, pairs) = rare_herbs();
        let herbs: Vec<&str> = herbs.iter().map(String::as_str).collect();
        let biomes: Vec<&str> = biomes.iter().map(String::as_str).collect();
        plant_counts(seeds, chunks, &herbs, &biomes).into_iter().filter(|r| pairs.contains(&(r.0.clone(), r.1.clone()))).collect()
    }

    /// Actual lotus on the selected wet component's shore, assigned to the nearest wet
    /// column at its level (ties by x, z). Other same-level pools compete for banks but
    /// cannot be crossed through dry shore. The bank window contains the complete small
    /// tarn's painted cells; use a wider water context so an unrelated bank near that
    /// window's edge is attributed to its own water rather than rejecting this pool.
    /// Skip if the pool's water or the nearest-water proof reaches its sampling edge.
    fn tarn_lotus(world: &Overworld, pit: IVec2, water: i32, reach: i32) -> Option<(usize, usize, usize, IVec2)> {
        let water_reach = reach * 2 + 32;
        let sampler = world.dense_sampler(pit - water_reach, pit + water_reach);
        let mut wet = Vec::new();
        let mut banks = Vec::new();
        for z in pit.y - water_reach..=pit.y + water_reach {
            for x in pit.x - water_reach..=pit.x + water_reach {
                let site = sampler.site(x, z);
                if site.water != water {
                    continue;
                }
                let p = IVec2::new(x, z);
                if site.height < water {
                    wet.push(p);
                } else if (p - pit).abs().max_element() <= reach && site.body == Body::Lake && site.height <= water + 2 && site.height + 1 < WORLD_HEIGHT_FOR_TESTS {
                    banks.push((p, site.height));
                }
            }
        }
        let wet_set: std::collections::HashSet<_> = wet.iter().copied().collect();
        let pool = water::lake_component(pit, reach, |p| wet_set.contains(&p));
        if pool.len() < 20 || pool.iter().any(|p| (*p - pit).abs().max_element() >= reach) {
            if std::env::var("AT").is_ok() {
                println!("  lotus pool at {pit} not measured: {} water columns or sampling-edge water (reach {reach})", pool.len());
            }
            return None;
        }
        let anchor = pool[0];
        let component: std::collections::HashSet<_> = pool.iter().copied().collect();
        let mut shore = Vec::new();
        for (p, height) in banks {
            let nearest = wet.iter().min_by_key(|q| ((**q - p).length_squared(), q.x, q.y)).unwrap();
            if component.contains(nearest) {
                let margin = water_reach + 1 - (p - pit).abs().max_element();
                if (*nearest - p).length_squared() >= margin * margin {
                    if std::env::var("AT").is_ok() {
                        println!("  lotus pool at {pit} not measured: bank {p} attribution reaches water sampling edge (bank reach {reach}, water reach {water_reach})");
                    }
                    return None;
                }
                shore.push((p, height));
            }
        }
        let mut positions: Vec<ColumnPos> = shore.iter().map(|&(p, _)| ColumnPos::of_block(IVec3::new(p.x, 0, p.y))).collect();
        positions.sort_unstable_by_key(|p| (p.0.x, p.0.y));
        positions.dedup();
        let area = generate_columns(world, &positions);
        let looks = plant_looks(&["sky_lotus"]);
        let audit = std::env::var("AT").is_ok();
        let registry = registry();
        let content = crate::content::WorldContent::base(&registry).unwrap();
        let lotus = content.plants.iter().find(|p| p.name == "base:sky_lotus").unwrap();
        let (_, _, pairs) = rare_herbs();
        let named: std::collections::HashSet<_> = pairs.iter().filter(|p| p.0 == "sky_lotus")
            .filter_map(|p| world.biomes.id(&format!("base:{}", p.1))).collect();
        // Per home biome: banks, eligible banks, obstructions, absent earth and trails. Existing ground
        // plants were placed after the edge patches and do not obstruct their candidates.
        let mut banks_by_home = std::collections::BTreeMap::<String, [usize; 5]>::new();
        let mut placed = Vec::new();
        for (p, height) in shore {
            let at = IVec3::new(p.x, height + 1, p.y);
            let pos = ColumnPos::of_block(at);
            let i = positions.binary_search_by_key(&(pos.0.x, pos.0.y), |p| (p.0.x, p.0.y)).unwrap();
            let above = area[i].block(at - pos.origin());
            if looks.contains_key(&above) {
                placed.push((p.x, p.y));
                if audit {
                    println!("    lotus sight {at}");
                }
            }
            if audit {
                let mut site = sampler.site(p.x, p.y);
                // The climate sampler leaves paths out; generation's full geology does not.
                site.trail = world.geo(p.x, p.y).trail.map_or(0.0, |t| t.tread);
                let below = area[i].block(at - pos.origin() - IVec3::Y);
                let blocked = !above.is_air() && registry.info(above).render != voxmar_engine::block::RenderKind::Plant;
                let ground = world.terrain.plant_ground(&site, below, if blocked { above } else { BlockId::AIR });
                let rooted = ground.is_some_and(|ground| lotus.ground.contains(&ground) || (ground == crate::content::Ground::Snow
                    && lotus.snowy.is_some() && lotus.ground.contains(&crate::content::Ground::Soil)));
                let trail = site.trail > 0.2;
                let counts = banks_by_home.entry(world.biomes.get(site.home).name.clone()).or_default();
                counts[0] += 1;
                counts[1] += usize::from(named.contains(&site.home) && rooted);
                counts[2] += usize::from(blocked);
                counts[3] += usize::from(!crate::terrain::earth(&registry, below));
                counts[4] += usize::from(trail);
            }
        }
        if audit {
            println!("    lotus shore at {pit}: banks/eligible/blocked/absent-earth/trail by home {banks_by_home:?}");
        }
        placed.sort_unstable();
        Some((pool.len(), placed.len(), patches(&placed).len(), anchor))
    }

    /// Tarns and lake banks in mountains and their alpine meadows (and snowy peaks, whose
    /// own lakes the sky lotus shares): `cargo test --profile iter -p voxmar_worldgen
    /// tarn_census -- --ignored --nocapture` (SEEDS=1,2,3,4, REGIONS=3, AT=1 for places to
    /// look). Over `REGIONS` × `REGIONS` regions of rivers (a kilometre each) around the
    /// nearest place of each of those biomes to the origin of each seed's world: each biome's land in and out of
    /// tarn country, the still lakes there per square kilometre (by their pit) and their size,
    /// and the share of its land that is lake bank (a block or two over a lake's water,
    /// where the sky lotus may grow).
    #[test]
    #[ignore]
    fn tarn_census() {
        let registry = registry();
        let seeds: Vec<u64> = std::env::var("SEEDS").unwrap_or_else(|_| "1,2,3,4".into()).split(',').filter_map(|s| s.parse().ok()).collect();
        let regions: i32 = std::env::var("REGIONS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
        let names = ["mountains", "alpine_meadow", "snowy_peaks"];
        // Per biome, in country and out: land columns (sampled every 8 blocks, so 64 each),
        // bank columns, lakes, lake cells.
        let mut tally = [[[0u64; 4]; 2]; 3];
        // At the same sample positions: high geographic area, then its dry columns.
        // The draft describes high ground but not an exact area equation; use the fixed
        // tarn floor, without narrowing it by any of the other admission conditions.
        let mut highland = [[[0u64; 2]; 2]; 3];
        // Each tarn's generated water in blocks (columns), inside tarn country and out.
        let mut sizes: [[Vec<usize>; 2]; 3] = Default::default();
        // LOTUS=n also measures n visible pools a seed, after actual chunk generation.
        let lotus: usize = std::env::var("LOTUS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut lotus_counts = Vec::new();
        let mut clipped = 0;
        const STEP: i32 = 8;
        // Stream-tier cells in a lake under which a pit is small, not big (drainage `min_area`).
        const TARN_CELLS: usize = 48;
        let floor = crate::content::WorldContent::base(&registry).unwrap().tarns.as_ref().map_or(0.0, |t| t.floor);
        let high_floor = SEA_LEVEL as f32 + floor;
        for &seed in &seeds {
            let world = Overworld::new(seed, registry.clone());
            let ids: Vec<BiomeId> = names.iter().map(|n| world.biomes.id(&format!("base:{n}")).unwrap()).collect();
            let size = crate::water::REGION;
            // The regions around the nearest place of each biome, each once.
            let mut around: Vec<IVec2> = Vec::new();
            for name in names {
                let Some(at) = world.locate_biome(name, IVec2::ZERO, 60000) else { continue };
                let centre = at.div_euclid(IVec2::splat(size));
                for r in 0..regions * regions {
                    let region = centre + IVec2::new(r % regions - regions / 2, r / regions - regions / 2);
                    if !around.contains(&region) {
                        around.push(region);
                    }
                }
            }
            let country = |x: i32, z: i32| usize::from(world.rivers.tarn_country(x, z) >= 0.5);
            let mut dry: Option<((i32, i32), i32)> = None;
            let mut banks = std::collections::HashSet::new();
            let mut waters = std::collections::HashSet::new();
            let mut lotus_sampled = 0;
            let mut lotus_water = std::collections::HashSet::new();
            for region in around {
                let first = region * size;
                // The land, every `STEP` blocks, a sampler a 256-block tile.
                for t in 0..16 {
                    let tile = first + IVec2::new(t % 4, t / 4) * 256;
                    let sampler = world.dense_sampler(tile, tile + 255);
                    for k in 0..(256 / STEP) * (256 / STEP) {
                        let (x, z) = (tile.x + k % (256 / STEP) * STEP, tile.y + k / (256 / STEP) * STEP);
                        let site = sampler.site(x, z);
                        let Some(b) = ids.iter().position(|&id| id == site.home) else { continue };
                        let c = country(x, z);
                        if site.height as f32 >= high_floor {
                            highland[b][c][0] += (STEP * STEP) as u64;
                            if site.height >= site.water {
                                highland[b][c][1] += (STEP * STEP) as u64;
                            }
                        }
                        if site.height >= site.water {
                            tally[b][c][0] += (STEP * STEP) as u64;
                            if b < 2 && world.rivers.tarn_country(x, z) == 0.0 && dry.is_none_or(|(_, h)| site.height > h) {
                                dry = Some(((x, z), site.height));
                            }
                        }
                    }
                }
                // The lakes by their pits, and the banks around each, column by column.
                for (pit, water, cells, is_tarn) in world.rivers.stream_lakes(world.stages(), region) {
                    let site = world.site(pit.x, pit.y);
                    let Some(b) = ids.iter().position(|&id| id == site.home) else { continue };
                    let c = country(pit.x, pit.y);
                    // Include existing small high lakes in the bank count as well: snowy
                    // peaks' own lakes also carry lotus. Only marked tarns enter `sizes`.
                    if cells >= TARN_CELLS || (water as f32) < SEA_LEVEL as f32 + floor {
                        continue;
                    }
                    tally[b][c][2] += 1;
                    tally[b][c][3] += cells as u64;
                    if std::env::var("AT").is_ok() && b < 2 {
                        println!("  seed {seed}: a lake of {cells} cells, its water at {water}, at {}, {} ({}{})", pit.x, pit.y, names[b], if c == 1 { ", tarn country" } else { "" });
                    }
                    let reach = (8 + 4 * (cells as f32).sqrt() as i32 * 2).max(32);
                    let sampler = world.dense_sampler(pit - reach, pit + reach);
                    // The tarn's generated water: its still-lake columns at its own level, which
                    // no other tarn has claimed.
                    let mut water_columns = std::collections::HashSet::new();
                    for z in pit.y - reach..=pit.y + reach {
                        for x in pit.x - reach..=pit.x + reach {
                            let site = sampler.site(x, z);
                            if site.height < site.water && site.water == water {
                                water_columns.insert(IVec2::new(x, z));
                            }
                        }
                    }
                    let pool = water::lake_component(pit, reach, |p| water_columns.contains(&p));
                    let wet = if is_tarn { pool.iter().filter(|p| waters.insert((p.x, p.y))).count() } else { pool.len() };
                    if is_tarn && pool.iter().any(|p| (*p - pit).abs().max_element() >= reach) {
                        clipped += 1;
                    }
                    if is_tarn {
                        assert!(!pool.is_empty(), "seed {seed}: no physical water at {pit}");
                        assert!(pool.iter().all(|p| (*p - pit).abs().max_element() < reach), "seed {seed}: clipped physical water at {pit}");
                        let low = pool.iter().copied().fold(pool[0], IVec2::min);
                        let high = pool.iter().copied().fold(pool[0], IVec2::max);
                        let span = high - low + 1;
                        assert!((8..=25).contains(&span.max_element()), "seed {seed}: connected physical water spans {span} at {pit}");
                        println!("  physical tarn seed {seed} at {pit}: {} water columns, span {span}", pool.len());
                        assert_eq!(c, 1, "seed {seed}: a tarn outside its country at {pit}");
                        sizes[b][c].push(wet);
                        if b < 2 && wet >= 20 && lotus_sampled < lotus
                            && let Some((water_columns, plants, groups, anchor)) = tarn_lotus(&world, pit, water, reach + 32)
                            && lotus_water.insert((water, anchor))
                        {
                            lotus_sampled += 1;
                            lotus_counts.push((plants, groups));
                            println!("  lotus seed {seed} pool {}, {}: {water_columns} connected water columns, {plants} actual plants in {groups} visible shore clusters; canonical {anchor}, water {water}", pit.x, pit.y);
                        }
                    }
                    if std::env::var("AT").is_ok() && b < 2 {
                        println!("  seed {seed}: a {} of {wet} water blocks across, at {}, {} ({}{})", if is_tarn { "tarn" } else { "high lake" }, pit.x, pit.y, names[b], if c == 1 { ", tarn country" } else { "" });
                    }
                    for z in pit.y - reach..=pit.y + reach {
                        for x in pit.x - reach..=pit.x + reach {
                            let site = sampler.site(x, z);
                            if let Some(b) = ids.iter().position(|&id| id == site.home)
                                && site.body == Body::Lake
                                && (0..=2).contains(&(site.height - site.water))
                                && banks.insert((x, z))
                            {
                                tally[b][country(x, z)][1] += 1;
                            }
                        }
                    }
                }
            }
            if std::env::var("AT").is_ok()
                && let Some((at, height)) = dry
            {
                println!("  seed {seed}: high ground outside tarn country at {at:?}, height {height}");
            }
        }
        if !lotus_counts.is_empty() {
            let plants: usize = lotus_counts.iter().map(|p| p.0).sum();
            let groups: usize = lotus_counts.iter().map(|p| p.1).sum();
            let empty = lotus_counts.iter().filter(|p| p.0 == 0).count();
            let distribution: Vec<usize> = (0..=4).map(|n| lotus_counts.iter().filter(|p| p.1.min(4) == n).count()).collect();
            println!("lotus visible clusters per pool (0, 1, 2, 3, 4+): {distribution:?}");
            println!("lotus occupancy: {} visible pools, {:.2} clusters and {:.2} plants per pool, {empty} empty; {:.2} plants per cluster", lotus_counts.len(), groups as f64 / lotus_counts.len() as f64, plants as f64 / lotus_counts.len() as f64, plants as f64 / groups.max(1) as f64);
        }
        for (b, name) in names.iter().enumerate() {
            let land: u64 = tally[b].iter().map(|t| t[0]).sum();
            for (country, label) in [(1, "in tarn country"), (0, "outside it")] {
                let [columns, banks, lakes, cells] = tally[b][country];
                let km2 = columns as f64 / 1e6;
                let across = if lakes > 0 { 2.0 * (cells as f64 * 64.0 / lakes as f64 / std::f64::consts::PI).sqrt() } else { 0.0 };
                println!(
                    "{name:<14} {label:<16} {:>5.1}% of its land, {km2:>6.2} km2: {:>5.1} lakes a km2 (about {across:.0} blocks across), bank {:.2}% of the land",
                    columns as f64 * 100.0 / land.max(1) as f64,
                    lakes as f64 / km2.max(1e-9),
                    banks as f64 * 100.0 / columns.max(1) as f64
                );
            }
        }
        println!("tarn wet footprints clipped at sampling edge: {clipped} (those sizes are lower bounds)");
        println!("tarn density mask: named home biome, ground height >= {high_floor}; the same {STEP}-block sample positions, 64 columns each; area includes submerged high ground, dry uses height >= water");
        // Keep the historical all-elevation named dry-land rate below. These parallel
        // denominators all use the same unique visible physical pools (20+ columns).
        // Zero sizes are repeated marked pits of an already counted physical pool.
        for (name, biomes) in [("mountains", 0..1), ("alpine_meadow", 1..2), ("snowy_peaks", 2..3), ("mountains+alpine_meadow", 0..2)] {
            let total_area: u64 = biomes.clone().flat_map(|b| highland[b].iter()).map(|h| h[0]).sum();
            let total_dry: u64 = biomes.clone().flat_map(|b| highland[b].iter()).map(|h| h[1]).sum();
            for (country, label) in [(1, "inside"), (0, "outside"), (2, "overall")] {
                let countries = if country == 2 { 0..2 } else { country..country + 1 };
                let mut tagged = 0;
                let mut visible = 0;
                let mut duplicate = 0;
                let mut named_dry = 0;
                let mut high_area = 0;
                let mut high_dry = 0;
                for b in biomes.clone() {
                    for c in countries.clone() {
                        tagged += sizes[b][c].len();
                        visible += sizes[b][c].iter().filter(|&&n| n >= 20).count();
                        duplicate += sizes[b][c].iter().filter(|&&n| n == 0).count();
                        named_dry += tally[b][c][0];
                        high_area += highland[b][c][0];
                        high_dry += highland[b][c][1];
                    }
                }
                let density = |columns: u64| visible as f64 * 1e6 / columns.max(1) as f64;
                println!("tarn density {name} {label}: {tagged} tagged pits, {visible} unique visible pools, {} sub-20 pits ({duplicate} repeated); {named_dry} all-named dry columns, {high_area} high-area columns, {high_dry} high-dry columns, {} submerged high columns; {:.3} all-named dry/km2, {:.3} high-area/km2, {:.3} high-dry/km2; {:.3}% of high area, {:.3}% of high dry",
                    tagged - visible, high_area - high_dry, density(named_dry), density(high_area), density(high_dry),
                    high_area as f64 * 100.0 / total_area.max(1) as f64, high_dry as f64 * 100.0 / total_dry.max(1) as f64);
            }
        }
        // The tarns themselves, by the water generated for each (the pit counts above are
        // pits; a tarn is only seen if its water is): how many hold at least 20 blocks, and
        // the spread of sizes, with the visible ones a square kilometre.
        for (country, label) in [(1, "in tarn country"), (0, "outside it")] {
            // The draft 96 target is the mountains and alpine meadows. Snowy peaks' own
            // lakes are counted separately above, and cannot dilute the target's denominator.
            let mut sizes: Vec<usize> = (0..2).flat_map(|b| std::mem::take(&mut sizes[b][country])).collect();
            sizes.sort_unstable();
            let km2: f64 = (0..2).map(|b| tally[b][country][0]).sum::<u64>() as f64 / 1e6;
            let visible = sizes.iter().filter(|&&n| n >= 20).count();
            let at = |q: f64| sizes.get(((sizes.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0);
            println!(
                "tarns {label:<16}: {} of them, {:.0}% hold 20+ water blocks, {:.1} visible a km2 ({:.1} pits); water blocks p10 {} median {} p90 {}",
                sizes.len(),
                visible as f64 * 100.0 / sizes.len().max(1) as f64,
                visible as f64 / km2.max(1e-9),
                sizes.len() as f64 / km2.max(1e-9),
                at(0.1),
                at(0.5),
                at(0.9)
            );
        }
    }

    /// Where the rare herbs grow and how many: `cargo test --profile iter -p voxmar_worldgen
    /// rare_herb_census -- --ignored --nocapture` (SEEDS=7,1,42, CHUNKS=12, PLACES=1).
    #[test]
    #[ignore]
    fn rare_herb_census() {
        let seeds: Vec<u64> = std::env::var("SEEDS").unwrap_or_else(|_| "7,1,42".into()).split(',').filter_map(|s| s.parse().ok()).collect();
        let chunks = std::env::var("CHUNKS").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
        let other = std::env::var("WILD").is_ok() || std::env::var("WILDTAG").is_ok();
        for (herb, biome, rate, size, n, columns) in if other { Vec::new() } else { rare_herb_counts(&seeds, chunks) } {
            println!("{herb:>13} {biome:<18} {rate:>6.0} per million columns, {n:>4} plants in {columns:>6} columns, {size:.1} per visible cluster");
        }
        // WILD=plant,plant (or WILDTAG=tag, every plant carrying it) instead: those plants over
        // a spread of climates (or WILDBIOMES=a,b), every biome they appear in, as placed plants
        // per million columns (a square kilometre) and one in how many columns.
        let registry = registry();
        let content = crate::content::WorldContent::base(&registry).unwrap();
        let tagged: Vec<String> = std::env::var("WILDTAG")
            .map(|tag| content.plants.iter().filter(|p| content.plant_tags(p, &registry).any(|t| t == tag)).map(|p| p.name.trim_start_matches("base:").to_owned()).collect())
            .unwrap_or_default();
        let wild = std::env::var("WILD").unwrap_or_default();
        let plants: Vec<&str> = wild.split(',').filter(|p| !p.is_empty()).chain(tagged.iter().map(String::as_str)).collect();
        if !plants.is_empty() {
            let listed = std::env::var("WILDBIOMES").unwrap_or_default();
            let mut biomes: Vec<&str> = listed.split(',').filter(|b| !b.is_empty()).collect();
            if biomes.is_empty() {
                biomes = vec!["plains", "meadow", "forest", "taiga", "steppe", "sagebrush_steppe", "savanna", "baobab_savanna", "outback", "desert", "badlands", "mediterranean", "jungle", "swamp", "bayou", "mangrove_swamp"];
            }
            for (plant, biome, rate, size, n, columns) in plant_counts(&seeds, chunks, &plants, &biomes) {
                if n > 0 {
                    println!("{plant:>16} {biome:<18} {rate:>6.0} per million columns, 1 in {:>6.0}, {n:>4} plants in {columns:>7} columns, {size:.1} per visible cluster", 1e6 / rate);
                }
            }
        }
    }

    /// The worlds `rare_herbs_grow_at_their_rates` counts, in order: the first four hold
    /// every herb biome near each of `PLACES` and big enough that they usually cover all of
    /// them (silverwood glades and moonlit meadows the scarcest), so few worlds are made;
    /// the rest are the next best, should a count fall short.
    const HERB_SEEDS: [u64; 16] = [26, 14, 37, 6, 22, 33, 31, 13, 25, 4, 28, 19, 21, 3, 12, 32];

    /// Design's rates for the rare herbs (draft 89): in every biome naming it, each herb
    /// grows within 0.5 to 1.6 times its own `aim` (plants.ron), in patches of 3 to 6
    /// (2.5 to 6.5 measured), and the classes keep their order: Legendary herbs rarer than
    /// VeryRare ones, VeryRare than Rare, on the same samples. Each biome is counted in more
    /// places (three a seed) until every herb in it has 25 patches (a count's noise under
    /// about ±20%) over at least twelve places, up to 16 seeds (`HERB_SEEDS`); a biome too small to get
    /// there is printed and left to the census. A herb of a lake's bank (the sky lotus) is
    /// held to its rate per million columns of bank instead, its data's `rate` times the
    /// biome's weight, with the same band: it grows only where the land has lakes, and
    /// how much of a biome is lake bank differs eightfold from world to world, so its rate
    /// over the whole biome follows the world's tarns (it still counts towards the classes'
    /// order). Its count stops at the bank that makes 25 patches at that rate, and is left to
    /// the census when the other herbs are done first; it must never grow off a bank, and
    /// must show wherever a quarter of that bank was counted.
    /// Generated for real, so slow: the full gate runs it.
    #[test]
    fn rare_herbs_grow_at_their_rates() {
        let registry = registry();
        let content = crate::content::WorldContent::base(&registry).unwrap();
        let (herbs, biomes, pairs) = rare_herbs();
        let herb_names: Vec<&str> = herbs.iter().map(String::as_str).collect();
        let looks = plant_looks(&herb_names);
        let def = |herb: &str| content.plants.iter().find(|p| p.name.trim_start_matches("base:") == herb).unwrap();
        let herb_index = |herb: &str| herbs.iter().position(|h| h == herb).unwrap();
        // Per biome: places counted, columns, and per herb its plants and patches. Each
        // world gives three places of each biome, far apart, as herbs along water gather where
        // the water does (tarns in tarn country), so a count needs many places as well as
        // many patches.
        let mut tally: Vec<(usize, u64, Vec<(usize, usize)>)> = vec![(0, 0, vec![(0, 0); herbs.len()]); biomes.len()];
        // Per biome its columns of lake bank, and per herb its plants off them.
        let mut banks = vec![0u64; biomes.len()];
        let mut off = vec![vec![0usize; herbs.len()]; biomes.len()];
        // A herb of a lake's bank, by pair: its rate per million columns of bank (its data's
        // rate times the biome's weight), and the columns of bank that make 25 patches at
        // that rate, which its count stops at instead of at 25 patches: lake-free places add
        // no bank, so they cannot hold a count open.
        let along_bank = |herb: &str, biome: &str| {
            let plant = def(herb);
            plant.habitat.edge().then(|| {
                assert_eq!(plant.habitat, crate::content::Habitat::LakeShore, "{herb}: only a lake's bank is counted");
                let named = content.biomes.iter().find(|d| d.name == format!("base:{biome}")).unwrap();
                let weight = named.flora.plants.iter().chain(named.flora.wonders.iter().flatten()).find_map(|s| matches!(s, crate::content::Select::Name(n, _) if *n == plant.name).then(|| s.weight())).unwrap();
                let patches = plant.patches.as_ref().unwrap();
                let aim = (patches.rate * weight) as f64;
                let mean = (patches.size.0 + patches.size.1) as f64 / 2.0;
                (aim, (25.0 * mean * 1e6 / aim) as u64)
            })
        };
        // Larger acceptance censuses can require the same seed prefix in every biome;
        // the normal gate retains twelve places and the existing sixteen-seed ceiling.
        let min_places = std::env::var("HERB_MIN_PLACES").map(|s| s.parse::<usize>().expect("HERB_MIN_PLACES must be an integer")).unwrap_or(12);
        assert!((12..=48).contains(&min_places), "HERB_MIN_PLACES must be in 12..=48");
        println!("rare-herb coverage: at least {min_places} places and 25 clusters per pair (enough bank for 25 along a bank), up to 16 seeds");
        // A biome is done when it has its places and every herb its patches or its bank; the
        // run goes on while any biome lacks places or patches, never for bank alone: a world
        // of few tarns would otherwise make many worlds for one herb, which is then left to
        // the census.
        let counted = |tally: &[(usize, u64, Vec<(usize, usize)>)], b: usize| {
            tally[b].0 >= min_places && pairs.iter().filter(|p| p.1 == biomes[b] && along_bank(&p.0, &p.1).is_none()).all(|p| tally[b].2[herb_index(&p.0)].1 >= 25)
        };
        let done = |tally: &[(usize, u64, Vec<(usize, usize)>)], banks: &[u64], b: usize| {
            counted(tally, b) && pairs.iter().filter(|p| p.1 == biomes[b]).all(|p| along_bank(&p.0, &p.1).is_none_or(|(_, wanted)| banks[b] >= wanted))
        };
        // HERB_SEEDS=a,b,... counts other worlds instead, to check the rates hold beyond these.
        let order: Vec<u64> = std::env::var("HERB_SEEDS").map(|s| s.split(',').map(|n| n.trim().parse().expect("HERB_SEEDS must be seeds")).collect()).unwrap_or_else(|_| HERB_SEEDS.to_vec());
        // The census on a pool of its own, as many threads as the tests' own: its generation
        // would otherwise fill that pool for minutes, every other test's work queued behind
        // it (`waterfalls_pour`, 7 s alone, took 198 s beside it). What it counts does not
        // depend on the threads: the places are counted in order. Only where the tests'
        // pool is held to part of the machine (`RAYON_NUM_THREADS`, half the cores in the
        // gate); on a pool of every core (no `RAYON_NUM_THREADS`, or 0), a second would
        // double the threads.
        let capped = std::env::var("RAYON_NUM_THREADS").ok().and_then(|n| n.parse::<usize>().ok()).is_some_and(|n| n > 0);
        let pool = capped.then(|| rayon::ThreadPoolBuilder::new().num_threads(rayon::current_num_threads()).build().unwrap());
        fn on_pool<R: Send>(pool: &Option<rayon::ThreadPool>, f: impl FnOnce() -> R + Send) -> R {
            match pool {
                Some(pool) => pool.install(f),
                None => f(),
            }
        }
        let mut seeds = 0;
        while seeds < order.len().min(16) && (0..biomes.len()).any(|b| !counted(&tally, b)) {
            let seed = order[seeds];
            seeds += 1;
            let world = shared_world(seed);
            let open: Vec<usize> = (0..biomes.len()).filter(|&b| !done(&tally, &banks, b)).collect();
            println!("seed {seed}: counting {}", open.iter().map(|&b| biomes[b].as_str()).collect::<Vec<_>>().join(", "));
            // Every place of every open biome side by side, counted in order.
            use rayon::prelude::*;
            let wanted: Vec<(usize, IVec2)> = open.iter().flat_map(|&b| PLACES.map(|from| (b, from))).collect();
            let areas: Vec<_> = on_pool(&pool, || wanted.par_iter().map(|&(b, from)| (b, biome_area(&world, &biomes[b], from, 16, &looks, herbs.len()))).collect());
            for (b, area) in areas {
                let Some((columns, bank, placed, away)) = area else { continue };
                tally[b].0 += 1;
                tally[b].1 += columns;
                banks[b] += bank;
                off[b].iter_mut().zip(away).for_each(|(all, away)| *all += away);
                for (k, placed) in placed.iter().enumerate() {
                    tally[b].2[k].0 += placed.len();
                    tally[b].2[k].1 += patches(placed).len();
                }
            }
        }
        let mut wrong = Vec::new();
        let mut measured: Vec<(String, f64)> = Vec::new();
        for (herb, biome) in &pairs {
            let b = biomes.iter().position(|x| x == biome).unwrap();
            let columns = tally[b].1;
            let (n, groups) = tally[b].2[herb_index(herb)];
            let rate = n as f64 * 1e6 / columns.max(1) as f64;
            let bank = along_bank(herb, biome);
            // Along a lake's bank, per million columns of bank; elsewhere, of the biome.
            let (judged, aim, over) = match bank {
                Some((aim, _)) => (n as f64 * 1e6 / banks[b].max(1) as f64, aim, format!("{} columns of bank", banks[b])),
                None => (rate, def(herb).patches.as_ref().unwrap().aim as f64, format!("{columns} columns")),
            };
            let size = n as f64 / groups.max(1) as f64;
            let line = format!("{herb} in {biome}: {judged:.1} per million{} (aim {aim:.0}, {:.2}x), {rate:.1} per million of the biome, {:.0} visible clusters a million, {size:.1} per cluster, {n} plants in {groups} clusters in {over} of {} places", if bank.is_some() { " of bank" } else { "" }, judged / aim, groups as f64 * 1e6 / columns.max(1) as f64, tally[b].0);
            if let Some((_, wanted)) = bank {
                // Never off its bank, and found wherever there is bank enough for a few patches.
                if off[b][herb_index(herb)] > 0 {
                    wrong.push(format!("{herb} in {biome}: {} plants off a lake's bank", off[b][herb_index(herb)]));
                }
                if banks[b] >= wanted / 4 && n == 0 {
                    wrong.push(format!("{herb} in {biome}: none along {} columns of bank", banks[b]));
                }
            }
            let enough = match bank {
                Some((_, wanted)) => banks[b] >= wanted,
                None => groups >= 25,
            };
            if !enough {
                // The gate's own seeds must find bank enough for every herb of the bank, or a
                // change to tarns or weights would leave one unjudged unseen; other seed sets
                // (HERB_SEEDS) leave it to the census.
                if bank.is_some() && std::env::var("HERB_SEEDS").is_err() {
                    wrong.push(format!("{line}: too little bank on the gate's seeds to judge it"));
                    continue;
                }
                println!("{line}: too few to judge, left to the census");
                continue;
            }
            println!("{line}");
            // The classes' order compares rates over the biome; a herb of the bank takes its
            // ratio to its aim there, as its rate over the biome follows the world's tarns.
            measured.push((herb.clone(), if bank.is_some() { judged / aim * def(herb).patches.as_ref().unwrap().aim as f64 } else { rate }));
            if !(0.5 * aim..=1.6 * aim).contains(&judged) || !(2.5..=6.5).contains(&size) {
                wrong.push(line);
            }
        }
        println!("over {seeds} seeds");
        // The classes in order, by each class's mean rate over its herbs' biomes.
        let class = |herb: &str| registry.def(registry.expect(&def(herb).block)).rarity;
        let mean = |r: voxmar_engine::block::Rarity| {
            let rates: Vec<f64> = measured.iter().filter(|(h, _)| class(h) == r).map(|(_, x)| *x).collect();
            (!rates.is_empty()).then(|| rates.iter().sum::<f64>() / rates.len() as f64)
        };
        use voxmar_engine::block::Rarity::{Legendary, Rare, VeryRare};
        let order: Vec<(voxmar_engine::block::Rarity, f64)> = [Legendary, VeryRare, Rare].into_iter().filter_map(|r| mean(r).map(|m| (r, m))).collect();
        for pair in order.windows(2) {
            if pair[0].1 >= pair[1].1 {
                wrong.push(format!("{:?} herbs ({:.0} per million) are not rarer than {:?} ones ({:.0})", pair[0].0, pair[0].1, pair[1].0, pair[1].1));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("; "));
    }

    /// Plains are well carpeted (about three blocks of open grass in ten carry a plant, since
    /// plants grow at half the biomes' own density), and few of the plants are flowers.
    /// Measured on the open grass of `base:plains` near two seeds' origins.
    #[test]
    fn plains_are_carpeted_and_flowers_stay_sparse() {
        let registry = registry();
        let (grass, air) = (registry.expect("base:grass_block"), BlockId::AIR);
        let (mut open, mut planted, mut flowers, mut implied) = (0, 0, 0, 0);
        for seed in [1, 42] {
            let world = shared_world(seed);
            let at = world.locate_biome("plains", IVec2::ZERO, 6000).unwrap();
            let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
            for (i, column) in generate_area(&world, first, 3).iter().enumerate() {
                let origin = first.offset(i as i32 % 3, i as i32 / 3).origin();
                for z in 0..32 {
                    for x in 0..32 {
                        let (wx, wz) = (origin.x + x, origin.z + z);
                        let site = world.site(wx, wz);
                        let h = site.height;
                        if column.block(IVec3::new(x, h, z)) != grass || world.biomes.get(site.biome).name != "base:plains" {
                            continue;
                        }
                        let above = column.block(IVec3::new(x, h + 1, z));
                        if above != air && registry.info(above).render != voxmar_engine::block::RenderKind::Plant {
                            continue;
                        }
                        open += 1;
                        if above != air {
                            planted += 1;
                            flowers += registry.def(above).has_tag("flower") as i32;
                        }
                        // The flower the biome's data grows here, before anything else
                        // claims the spot.
                        if let Some(ecology::Plant::One(b) | ecology::Plant::Tall(b, _) | ecology::Plant::Stack(b, _)) = world.ecology.plant(&site, crate::content::Ground::Soil, wx, wz) {
                            implied += registry.def(b).has_tag("flower") as i32;
                        }
                    }
                }
            }
        }
        let (planted, implied) = (planted as f32 / open as f32, implied as f32 / open as f32);
        let flowers = flowers as f32 / open as f32;
        println!("{open} open grass blocks, {planted:.2} planted, {flowers:.4} flowers ({implied:.4} by the biome's data)");
        // Two-sided: well carpeted but not crowded; flowers scarce, yet at least half as
        // many as the biome's data grows (nothing else crowds them out).
        assert!(open > 1000 && (0.24..=0.36).contains(&planted), "{open} open, {planted} planted");
        assert!(implied > 0.0 && flowers >= 0.5 * implied && flowers <= 0.05, "{flowers} flowers, {implied} by the data");
    }

    /// Small plants in 3×3 chunks of `biome` near the origin of `world`: how many in all,
    /// and how many of them the plant density keeps (gatherables).
    fn plants_in(world: &Overworld, content: &WorldContent, biome: &str, registry: &BlockRegistry) -> (u32, u32) {
        let kept: rustc_hash::FxHashSet<BlockId> =
            content.plants.iter().filter(|p| content.plant_kept(p, registry)).filter_map(|p| registry.id(&p.block)).collect();
        let at = world.locate_biome(biome, IVec2::ZERO, 6000).unwrap();
        let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
        let (mut all, mut gathered) = (0, 0);
        for column in generate_area(world, first, 3) {
            for z in 0..32 {
                for x in 0..32 {
                    for y in (1..WORLD_HEIGHT_FOR_TESTS).rev() {
                        let b = column.block(IVec3::new(x, y, z));
                        if b.is_air() {
                            continue;
                        }
                        if registry.info(b).render != voxmar_engine::block::RenderKind::Plant {
                            break;
                        }
                        all += 1;
                        gathered += kept.contains(&b) as u32;
                    }
                }
            }
        }
        (all, gathered)
    }

    /// The plant density thins the look of the land, not its harvest: at density 0.5 the
    /// same ground grows 40-60% of the plants it grows at 1.0, while the plants gathered
    /// for food and crafting (wild crops, rare herbs, forage, berries, fungi, sugar cane)
    /// keep their count within sampling noise.
    #[test]
    fn plant_density_thins_the_look_not_the_harvest() {
        let registry = registry();
        let full_density = "(density: 1.0, plants: [])";
        let half = WorldContent::base(&registry).unwrap();
        assert_eq!(half.plant_density, 0.5, "the test measures the base package's 0.5");
        let files: Vec<voxmar_engine::content::ContentFile> = crate::content::BASE_FILES
            .iter()
            .map(|(name, source)| voxmar_engine::content::ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() })
            .chain([voxmar_engine::content::ContentFile { package: "mod".into(), name: "plants.ron".into(), source: full_density.into() }])
            .collect();
        let full = WorldContent::from_files(files.iter(), &registry).unwrap();
        assert_eq!(full.plant_density, 1.0);
        use rayon::prelude::*;
        // Each seed's world at each density, and its biomes, side by side.
        let counts: Vec<(usize, u32, u32)> = [1, 7]
            .into_par_iter()
            .flat_map(|seed| [(seed, 0usize), (seed, 1)])
            .flat_map(|(seed, k)| {
                let content = [&full, &half][k];
                let world = Overworld::with_content(seed, registry.clone(), content);
                ["forest", "plains", "taiga"].into_par_iter().map(|biome| plants_in(&world, content, biome, &registry)).map(|(a, g)| (k, a, g)).collect::<Vec<_>>()
            })
            .collect();
        let (mut all, mut gathered) = ([0u32; 2], [0u32; 2]);
        for (k, a, g) in counts {
            all[k] += a;
            gathered[k] += g;
        }
        let (ratio, kept) = (all[1] as f32 / all[0] as f32, gathered[1] as f32 / gathered[0] as f32);
        println!("plants {} → {} ({ratio:.2}); gathered {} → {} ({kept:.2})", all[0], all[1], gathered[0], gathered[1]);
        assert!((0.4..=0.6).contains(&ratio), "plants at half density: {ratio}");
        assert!(gathered[0] >= 100 && (0.85..=1.15).contains(&kept), "gathered plants {gathered:?}: {kept}");
    }

    /// `cargo test --release -p voxmar_worldgen decor_sights -- --ignored --nocapture`
    /// (optionally `SEED=n`, `RANGE=blocks`) prints `--pose` arguments for in-game captures
    /// of ground decoration near spawn: deep in an old-growth forest, at a scrubby forest
    /// edge, in a clearing, in a meadow, on a slope, in a cold forest and in a dry
    /// grassland, each standing on the ground and from a little above.
    #[test]
    #[ignore]
    fn decor_sights() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(42);
        let range: i32 = std::env::var("RANGE").ok().and_then(|s| s.parse().ok()).unwrap_or(700);
        let world = Overworld::new(seed, registry());
        let spawn = world.spawn_point();
        let eye = spawn.as_vec3() + voxmar_engine::glam::Vec3::new(0.5, 1.62, 0.5);
        let sampler = world.sampler(IVec2::new(spawn.x, spawn.z) - range, IVec2::new(spawn.x, spawn.z) + range);
        type Test = fn(&Site, &Biome) -> bool;
        let dry = |s: &Site| s.height > s.water && s.river < 0.2 && s.height < SEA_LEVEL + 70;
        let sights: [(&str, Test); 8] = [
            ("old-growth forest", |s, b| b.has_tag("forest") && s.stand > 0.75 && s.forest > 0.55 && s.clearing == 0.0 && s.slope <= 1),
            ("forest edge", |s, b| b.has_tag("forest") && s.stand < 0.2 && s.clearing == 0.0 && s.slope <= 1),
            ("clearing", |s, b| b.has_tag("forest") && s.clearing > 0.95),
            ("meadow", |s, b| b.has_tag("grassland") && !b.has_tag("dry") && s.edge > 40.0 && s.slope <= 1),
            ("slope", |s, b| !b.has_tag("mountain") && !b.has_tag("desert") && s.slope >= 2 && s.slope <= 3 && s.temperature > 4.0),
            ("cold forest", |s, b| b.has_tag("forest") && b.has_tag("cold") && s.forest > 0.4),
            ("dry grassland", |s, b| b.has_tag("grassland") && b.has_tag("dry") && s.edge > 30.0),
            ("snowy", |s, _| s.temperature < -3.0 && s.forest > 0.2),
        ];
        println!("seed {seed}, spawn {spawn}");
        // Named biomes (`BIOMES=maple_forest,bog`): the nearest place well inside each.
        if let Ok(names) = std::env::var("BIOMES") {
            for name in names.split(',') {
                let Some(id) = world.biomes.id(&format!("base:{name}")) else { continue };
                // Peaks are steep by nature.
                let steep = if world.biomes.get(id).has_tag("peaks") || world.biomes.get(id).has_tag("mountain") { 6 } else { 2 };
                // The sea: a view out over it from the nearest dry shore, looking at its
                // nearest open water of this biome.
                if world.biomes.get(id).has_tag("ocean") {
                    let centre = IVec2::new(spawn.x, spawn.z);
                    let near = world.sampler(centre - 3000, centre + 3000);
                    let grid: Vec<(IVec2, Site)> = (0..188 * 188).map(|i| centre + IVec2::new(i % 188 * 32 - 3000, i / 188 * 32 - 3000)).map(|p| (p, near.site(p.x, p.y))).collect();
                    let sea = grid.iter().filter(|(_, s)| s.home == id && s.height < s.water && s.edge > 16.0).min_by_key(|(p, _)| (*p - centre).length_squared()).map(|(p, _)| *p);
                    let shore = sea.and_then(|sea| grid.iter().filter(|(_, s)| s.height >= s.water && s.height < s.water + 12 && s.river < 0.2).min_by_key(|(p, _)| (*p - sea).length_squared()).map(|(p, _)| (*p, sea)));
                    match shore {
                        Some((at, sea)) => {
                            let dir = (sea - at).as_vec2().normalize_or_zero();
                            let yaw = (-dir.x).atan2(-dir.y).to_degrees();
                            println!("{name} at {},{}: --command \"/tp {} {}\" --pose 0,0.3,0,{yaw:.0},-8  above: --pose 0,14,0,{yaw:.0},-14", at.x, at.y, at.x, at.y);
                        }
                        None => println!("{name}: not found"),
                    }
                    continue;
                }
                // The point deepest inside the biome within a few kilometres of spawn, else
                // the deepest around the first place found further out.
                let centre = IVec2::new(spawn.x, spawn.z);
                let near = world.sampler(centre - 3000, centre + 3000);
                let found = (0..188 * 188)
                    .map(|i| centre + IVec2::new(i % 188 * 32 - 3000, i / 188 * 32 - 3000))
                    .map(|p| (p, near.site(p.x, p.y)))
                    .filter(|(_, site)| site.home == id && site.slope <= steep && (dry(site) || steep > 2 && site.height > site.water && site.river < 0.2))
                    .max_by(|a, b| a.1.edge.total_cmp(&b.1.edge))
                    .map(|(p, _)| p);
                let found = found.or_else(|| world.locate_biome(name, centre, 12000)).and_then(|at| {
                    let sampler = world.sampler(at - 256, at + 256);
                    (0..64 * 64)
                        .map(|i| at + IVec2::new(i % 64 * 8 - 256, i / 64 * 8 - 256))
                        .map(|p| (p, sampler.site(p.x, p.y)))
                        .filter(|(_, site)| site.home == id && site.slope <= steep && (dry(site) || steep > 2 && site.height > site.water && site.river < 0.2))
                        .max_by(|a, b| a.1.edge.total_cmp(&b.1.edge))
                        .map(|(p, _)| p)
                });
                let Some(at) = found else {
                    println!("{name}: not found");
                    continue;
                };
                // Poses are relative to the eye of the player the command takes there.
                let forest = world.site(at.x, at.y).forest > 0.3;
                let (back, up) = if forest { (20.0f32, 26.0f32) } else { (14.0, 11.0) };
                // Above every hill between the player and the camera, and a little beyond.
                let here = world.height(at.x, at.y);
                let reach = back as i32 + 6;
                let highest = (0..=reach).step_by(2).flat_map(|a| (0..=reach).step_by(2).map(move |b| (a, b))).map(|(a, b)| world.height(at.x + a, at.y + b)).max().unwrap_or(here);
                let up = up.max((highest - here) as f32 + if forest { 24.0 } else { 9.0 });
                println!("{name} at {},{}: --command \"/tp {} {}\" --pose 0,0.3,0,45,-8  above: --pose {back},{up:.0},{back},45,-36", at.x, at.y, at.x, at.y);
            }
            return;
        }
        for (name, test) in sights {
            let mut best: Option<(i32, Site, i32, i32)> = None;
            for dz in (-range..=range).step_by(6) {
                for dx in (-range..=range).step_by(6) {
                    let d = dx * dx + dz * dz;
                    if best.as_ref().is_some_and(|b| d >= b.0) {
                        continue;
                    }
                    let (x, z) = (spawn.x + dx, spawn.z + dz);
                    let site = sampler.site(x, z);
                    if dry(&site) && test(&site, world.biomes.get(site.biome)) {
                        best = Some((d, site, x, z));
                    }
                }
            }
            let Some((_, site, x, z)) = best else {
                println!("{name}: none within {range}");
                continue;
            };
            let biome = &world.biomes.get(site.biome).name;
            let ground = voxmar_engine::glam::Vec3::new(x as f32 + 0.5, site.height as f32 + 1.0, z as f32 + 0.5);
            let stand = ground + voxmar_engine::glam::Vec3::Y * 1.7 - eye;
            let above = ground + voxmar_engine::glam::Vec3::new(12.0, 10.0, 12.0) - eye;
            println!(
                "{name} ({biome}, stand {:.2}, forest {:.2}, slope {}) at {x},{z}: --pose {:.1},{:.1},{:.1},45,-12  above: --pose {:.1},{:.1},{:.1},45,-35",
                site.stand, site.forest, site.slope, stand.x, stand.y, stand.z, above.x, above.y, above.z
            );
        }
    }

    /// `cargo test --profile iter -p voxmar_worldgen tree_sights -- --ignored --nocapture`
    /// (with `SEED` and `BIOMES=forest,taiga`) prints, for the biggest tree near the middle
    /// of each biome, the `/tp` command and `--pose` arguments of three views of it: from
    /// under its crown looking up, from a dozen blocks away and from above, so the same
    /// trees can be compared however they are grown.
    #[test]
    #[ignore]
    fn tree_sights() {
        use voxmar_engine::block::RenderKind;
        use voxmar_engine::coords::{CHUNK_SIZE, WORLD_HEIGHT};
        use voxmar_engine::glam::Vec3;
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(42);
        let registry = registry();
        let world = Overworld::new(seed, registry.clone());
        let spawn = world.spawn_point();
        let names = std::env::var("BIOMES").unwrap_or_else(|_| "forest".into());
        for name in names.split(',') {
            let Some(id) = world.biomes.id(&format!("base:{name}")) else { continue };
            let centre = IVec2::new(spawn.x, spawn.z);
            let dry = |s: &Site| s.height > s.water && s.river < 0.2;
            let Some(at) = world.locate_biome(name, centre, 12000).and_then(|at| {
                let sampler = world.sampler(at - 256, at + 256);
                (0..64 * 64)
                    .map(|i| at + IVec2::new(i % 64 * 8 - 256, i / 64 * 8 - 256))
                    .map(|p| (p, sampler.site(p.x, p.y)))
                    .filter(|(_, site)| site.home == id && dry(site) && site.slope <= 1)
                    .max_by(|a, b| (a.1.edge.min(60.0) + 40.0 * a.1.forest).total_cmp(&(b.1.edge.min(60.0) + 40.0 * b.1.forest)))
                    .map(|(p, _)| p)
            }) else {
                println!("{name}: not found");
                continue;
            };
            let size = 3;
            let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
            let columns = generate_area(&world, first, size);
            let origin = first.origin();
            let span = size * CHUNK_SIZE;
            let block = |p: IVec3| {
                let inside = (0..span).contains(&(p.x - origin.x)) && (0..span).contains(&(p.z - origin.z)) && (0..WORLD_HEIGHT).contains(&p.y);
                if inside {
                    columns[(((p.z - origin.z) >> 5) * size + ((p.x - origin.x) >> 5)) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31))
                } else {
                    BlockId::AIR
                }
            };
            let log = |b: BlockId| registry.def(b).name.ends_with("_log");
            let leaf = |b: BlockId| registry.info(b).render == RenderKind::Leaves;
            let open = |b: BlockId| b.is_air() || registry.info(b).render == RenderKind::Plant;
            // The top of the ground (not wood or leaves) at x, z.
            let ground = |x: i32, z: i32| (1..WORLD_HEIGHT).rev().find(|&y| {
                let b = block(IVec3::new(x, y, z));
                !open(b) && !log(b) && !leaf(b)
            });
            // Trunks standing on the ground in the middle column, by how many leaves their
            // crown holds within five blocks.
            let mut best: Option<(IVec3, usize)> = None;
            for z in origin.z + 32..origin.z + 64 {
                for x in origin.x + 32..origin.x + 64 {
                    let Some(g) = ground(x, z) else { continue };
                    let base = IVec3::new(x, g + 1, z);
                    if !log(block(base)) {
                        continue;
                    }
                    let mut leaves = 0;
                    for dy in 2..30 {
                        for dz in -5..=5 {
                            for dx in -5..=5 {
                                leaves += usize::from(leaf(block(base + IVec3::new(dx, dy, dz))));
                            }
                        }
                    }
                    if best.is_none_or(|b| leaves > b.1) {
                        best = Some((base, leaves));
                    }
                }
            }
            let Some((base, _)) = best else {
                println!("{name}: no tree near {at}");
                continue;
            };
            // Where the crown is: the middle of its leaves above the trunk.
            let mut sum = Vec3::ZERO;
            let mut n = 0.0;
            for dy in 2..30 {
                for dz in -5..=5 {
                    for dx in -5..=5 {
                        let p = base + IVec3::new(dx, dy, dz);
                        if leaf(block(p)) {
                            sum += p.as_vec3() + 0.5;
                            n += 1.0;
                        }
                    }
                }
            }
            let crown = if n > 0.0 { sum / n } else { base.as_vec3() + Vec3::new(0.5, 6.0, 0.5) };
            let look = |from: Vec3, to: Vec3| {
                let d = to - from;
                let yaw = (-d.x).atan2(-d.z).to_degrees();
                let pitch = d.y.atan2(Vec3::new(d.x, 0.0, d.z).length()).to_degrees();
                (yaw, pitch)
            };
            // A standing eye at x, z, with what the player's eye is at after `/tp x z`.
            let eye = |x: i32, z: i32| ground(x, z).map(|g| Vec3::new(x as f32 + 0.5, g as f32 + 2.62, z as f32 + 0.5));
            // Teleported onto the ground at x, z (`/tp` with a height, as `/tp x z` would
            // land on the crown above), so poses are relative to that eye.
            let pose = |tp: IVec3, from: Vec3, to: Vec3| {
                let player = eye(tp.x, tp.z).unwrap();
                let (yaw, pitch) = look(from, to);
                let o = from - player;
                let feet = player - Vec3::Y * 1.62;
                format!("\"{} {} {}\" {:.1},{:.1},{:.1},{:.0},{:.0}", feet.x, feet.y, feet.z, o.x, o.y, o.z, yaw, pitch)
            };
            // How clear the line from an eye to the crown is: blocks in the way.
            let blocked = |from: Vec3, to: Vec3| (1..40).filter(|&i| !open(block((from + (to - from) * (i as f32 / 40.0)).floor().as_ivec3()))).count();
            let headings: Vec<IVec2> = (0..16).map(|i| {
                let a = i as f32 / 16.0 * std::f32::consts::TAU;
                IVec2::new((a.cos() * 18.0).round() as i32, (a.sin() * 18.0).round() as i32)
            }).collect();
            // Under the crown: two blocks out from the trunk, looking up into it.
            let under = [IVec2::new(2, 1), IVec2::new(-2, 1), IVec2::new(1, -2), IVec2::new(-1, 2)]
                .into_iter()
                .map(|d| IVec3::new(base.x + d.x, 0, base.z + d.y))
                .find(|p| eye(p.x, p.z).is_some_and(|e| open(block(e.floor().as_ivec3())) && open(block(e.floor().as_ivec3() - IVec3::Y))))
                .unwrap_or(base);
            let under_eye = eye(under.x, under.z).unwrap();
            println!("{name} tree at {base}, crown {crown:.0}");
            println!("  under: {}", pose(under, under_eye, crown + Vec3::Y * 2.0));
            // From eighteen blocks away and a little up, as from a rise, where the view of the
            // crown is clearest.
            let (mid, mid_eye) = headings
                .iter()
                .filter_map(|d| {
                    let p = IVec3::new(base.x + d.x, 0, base.z + d.y);
                    eye(p.x, p.z).map(|e| (p, e + Vec3::Y * 4.0))
                })
                .min_by_key(|(_, e)| blocked(*e, crown))
                .unwrap();
            println!("  mid:   {}", pose(mid, mid_eye, crown - Vec3::Y * 1.5));
            // From above and further back along the same line.
            let back = (mid_eye - crown).with_y(0.0).normalize();
            let high = crown + back * 30.0 + Vec3::Y * 22.0;
            println!("  hill:  {}", pose(mid, high, crown - Vec3::Y * 4.0));
            let far = crown + back * 70.0 + Vec3::Y * 45.0;
            println!("  far:   {}", pose(mid, far, crown + back * 10.0 - Vec3::Y * 10.0));
        }
    }

    /// The ground sampled a grid at a time (`geo_grid`, which `water_sights` reads) is the
    /// ground at each of its positions, across tiles cropped at the grid's edge: around the
    /// water and crags near spawn on two seeds.
    #[test]
    fn a_grid_of_ground_is_the_ground_at_each_position() {
        for seed in [1, 5] {
            let world = shared_world(seed);
            let spawn = world.spawn_point();
            let sights = world.water_sights(IVec2::new(spawn.x, spawn.z), 300);
            let mut origins = vec![IVec2::new(spawn.x, spawn.z) - 150];
            origins.extend(sights.iter().map(|&(_, at)| IVec2::new(at.x, at.z) - 100));
            for origin in origins {
                let (step, size) = (2, 101);
                let grid = world.geo_grid(origin, step, size);
                for (i, sample) in grid.iter().enumerate() {
                    let (x, z) = (origin.x + i as i32 % size * step, origin.y + i as i32 / size * step);
                    assert_eq!(*sample, world.geo(x, z), "seed {seed}: the grid from {origin} at {x} {z}");
                }
            }
        }
    }

    /// Every waterfall pours: wherever water stands three or more blocks above the water
    /// beside it, the upper water's top is water and the face below it holds the curtain
    /// it pours, around the falls nearest spawn on a few seeds.
    #[test]
    fn waterfalls_pour() {
        let registry = registry();
        let mut faces = 0;
        for seed in [1, 3, 7, 42] {
            let world = shared_world(seed);
            let spawn = world.spawn_point();
            let sights = world.water_sights(IVec2::new(spawn.x, spawn.z), 600);
            let Some(&(_, fall)) = sights.iter().find(|(s, _)| *s == WaterSight::Waterfall) else { continue };
            let first = ColumnPos::of_block(fall).offset(-2, -2);
            let columns = generate_area(&world, first, 5);
            let origin = first.origin();
            let block = |p: IVec3| {
                let (cx, cz) = ((p.x - origin.x).div_euclid(CHUNK_SIZE), (p.z - origin.z).div_euclid(CHUNK_SIZE));
                columns[(cz * 5 + cx) as usize].block(IVec3::new(p.x.rem_euclid(CHUNK_SIZE), p.y, p.z.rem_euclid(CHUNK_SIZE)))
            };
            let geos = world.geo_area(origin.x + 31, origin.z + 31, 98);
            let geo = |x: i32, z: i32| geos[((z - origin.z - 31) * 98 + x - origin.x - 31) as usize];
            for z in origin.z + 32..origin.z + 128 {
                for x in origin.x + 32..origin.x + 128 {
                    let Some((_, top)) = geo(x, z).wet() else { continue };
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let Some((_, above)) = geo(x + dx, z + dz).wet() else { continue };
                        if above < top + 3 {
                            continue;
                        }
                        faces += 1;
                        let lip = block(IVec3::new(x + dx, above, z + dz));
                        let curtain = (top + 1..above).filter(|&y| registry.fluid(block(IVec3::new(x, y, z))).is_some()).count();
                        assert!(
                            registry.fluid(lip).is_some() && curtain > 0,
                            "seed {seed}: the fall from {},{} (water at {above}) to {x},{z} (water at {top}) is dry: lip {}, {curtain} curtain cells",
                            x + dx,
                            z + dz,
                            registry.def(lip).name
                        );
                    }
                }
            }
        }
        assert!(faces >= 4, "only {faces} fall faces near spawn on the test seeds");
    }

    /// Generated water and lava stay put: no fluid touches air below it, nor beside it
    /// except the air over lower water that a waterfall drops into; water never touches
    /// lava, and lava lies only at the bottom of the world. Checked across column borders,
    /// so it also shows the columns agree with each other, and around rivers.
    /// On the spines of ranges the density breaks the rock into crags: there are overhangs
    /// (air under rock within a few blocks of a column's top), and every column's top is the
    /// same however it is asked for, alone or with its area, and holds its top block.
    #[test]
    fn crags_overhang_and_keep_their_tops() {
        let mut overhangs = 0;
        for seed in [7, 3, 42] {
            let world = shared_world(seed);
            let Some(at) = (0..160 * 160).map(|i| IVec2::new(i % 160 * 50 - 4000, i / 160 * 50 - 4000)).find(|p| world.geo(p.x, p.y).crag.amplitude > 6.0) else {
                continue;
            };
            let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-1, -1);
            let columns = generate_area(&world, first, 3);
            let middle = &columns[4];
            let origin = first.offset(1, 1).origin();
            let area = world.geo_area(origin.x, origin.z, 32);
            // Columns a cave mouth, sinkhole or ravine opens to the sky are not counted.
            let geology = crate::geology::GeoGrid::new(&world, (origin.x - 2, origin.z - 2), 36);
            let caves = world.caves.column(&geology, origin, voxmar_engine::coords::WORLD_HEIGHT - 1);
            let (mut solid_tops, mut counted) = (0, 0);
            for z in 0..32 {
                for x in 0..32 {
                    let (wx, wz) = (origin.x + x, origin.z + z);
                    let top = area[(z * 32 + x) as usize].height;
                    let alone = world.geo(wx, wz);
                    assert_eq!(top, alone.height, "seed {seed}: the top at ({wx}, {wz}): {:?} alone, {:?} in the area", alone.crag, area[(z * 32 + x) as usize].crag);
                    if !caves.surface_open(wx, wz, &alone) {
                        counted += 1;
                        solid_tops += !middle.block(IVec3::new(x, top, z)).is_air() as usize;
                    }
                    let solid = |y: i32| !middle.block(IVec3::new(x, y, z)).is_air();
                    overhangs += (top - 8..top).any(|y| !solid(y) && solid(y + 1)) as usize;
                }
            }
            assert!(solid_tops > counted * 95 / 100, "seed {seed}: {solid_tops} of {counted} columns with their top block in place");
        }
        assert!(overhangs > 0, "no overhangs on any range");
    }

    #[test]
    fn fluids_are_contained() {
        let registry = registry();
        let (water, lava) = (registry.expect("base:water"), registry.expect("base:lava"));
        use rayon::prelude::*;
        // The seeds and their places side by side.
        let counted: Vec<(usize, usize)> = [(1, (0, 0)), (42, (3000, -2000)), (0x5eed, (-4100, 900)), (7, (1500, 5200))].into_par_iter().map(|(seed, place)| {
            let world = shared_world(seed);
            // Some coast too, where caves meet the sea.
            let coast = (0..4000).map(|i| (place.0 + i % 64 * 48, place.1 + i / 64 * 48)).find(|&(x, z)| (world.height(x, z) - SEA_LEVEL).abs() <= 2).unwrap();
            // Creeks with waterfalls and confluences, and a highland lake.
            let rivers: &[(i32, i32)] = match seed {
                1 => &[(10, 88), (-20, 160)],
                42 => &[(-306, -70)],
                _ => &[],
            };
            let places: Vec<(i32, i32)> = [place, (place.0 + 700, place.1 + 300), (place.0 - 900, place.1 - 600), coast].into_iter().chain(rivers.iter().copied()).collect();
            places.into_par_iter().map(|(x, z)| {
                let (mut fluids, mut caves) = (0, 0);
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
                        let ground = world.height(x, z);
                        for y in 0..voxmar_engine::coords::WORLD_HEIGHT {
                            let p = IVec3::new(x, y, z);
                            let here = block(p).unwrap();
                            if here.is_air() && y < ground {
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
                (fluids, caves)
            }).reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
        }).collect();
        let (fluids, caves) = counted.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert!(fluids > 0 && caves > 0);
    }

    /// Cave biomes line and decorate their caves: their blocks turn up underground, and no
    /// decoration floats with air on every side. (Linings may: the caves leave the odd
    /// lone block of rock.)
    #[test]
    fn caves_are_decorated() {
        let registry = registry();
        let decorations: Vec<BlockId> =
            ["glow_berries", "hanging_roots", "hanging_dripstone", "pointed_dripstone", "crystal_cluster", "moss_carpet", "brown_mushroom", "moss_block", "dripstone_block"]
                .map(|n| registry.expect(&format!("base:{n}")))
                .to_vec();
        // The last two also line caves or make up their rock.
        let decoration_only = decorations.len() - 2;
        let mut found = vec![0; decorations.len()];
        for (seed, place) in [(1, (0, 0)), (7, (200, 150)), (42, (-300, -100))] {
            let world = shared_world(seed);
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

    /// Everything hanging (glow berries, roots, stalactite tips) hangs under solid rock or
    /// under more of its own column, as the game requires of blocks with `support: Above`.
    #[test]
    fn hanging_blocks_hang_from_rock() {
        use voxmar_engine::block::Support;
        let registry = registry();
        let mut hanging = 0;
        // Lush caves, glowroot hollows and dripstone caves near spawn (see `cave_sights`).
        for (seed, place) in [(1, (-271, 38)), (7, (261, 150)), (1, (-47, -126)), (42, (-43, -354))] {
            let world = shared_world(seed);
            let first = ColumnPos::of_block(IVec3::new(place.0, 0, place.1)).offset(-3, -3);
            let columns = generate_area(&world, first, 6);
            let origin = first.origin();
            let block = |p: IVec3| {
                let (cx, cz) = ((p.x - origin.x) >> 5, (p.z - origin.z) >> 5);
                columns[(cz * 6 + cx) as usize].block(IVec3::new(p.x & 31, p.y, p.z & 31))
            };
            for z in origin.z..origin.z + 192 {
                for x in origin.x..origin.x + 192 {
                    for y in 1..voxmar_engine::coords::WORLD_HEIGHT - 1 {
                        let p = IVec3::new(x, y, z);
                        if registry.info(block(p)).support != Support::Above {
                            continue;
                        }
                        hanging += 1;
                        let mut up = p + IVec3::Y;
                        while registry.info(block(up)).support == Support::Above {
                            up += IVec3::Y;
                        }
                        assert!(registry.info(block(up)).solid, "seed {seed}: {} at {p} hangs from {} at {up}", registry.def(block(p)).name, registry.def(block(up)).name);
                    }
                }
            }
        }
        assert!(hanging > 100, "only {hanging} hanging blocks");
    }

    /// Geodes are hollow and lined with crystal, in calcite and dark rock.
    #[test]
    fn geodes_are_hollow_and_lined() {
        let registry = registry();
        let crystal = [registry.expect("base:crystal_block")];
        let mut checked = 0;
        for seed in [5, 1, 42, 7, 3, 11] {
            let world = shared_world(seed);
            // Around spawn, which is on land, as geodes are.
            let spawn = ColumnPos::of_block(world.spawn_point());
            for i in 0..32 * 32 {
                let pos = spawn.offset(i % 32 - 16, i / 32 - 16);
                let origin = pos.origin();
                let geology = geology::GeoGrid::new(&world, (origin.x - 1, origin.z - 1), voxmar_engine::coords::CHUNK_SIZE + 2);
                let geodes: Vec<_> = world.caves.geodes_near(&geology, origin).into_iter().filter(|g| ColumnPos::of_block(g.centre.floor().as_ivec3()) == pos).collect();
                if geodes.is_empty() {
                    continue;
                }
                let column = world.generate(pos);
                for geode in geodes {
                    let centre = geode.centre.floor().as_ivec3();
                    let local = |p: IVec3| IVec3::new(p.x & 31, p.y, p.z & 31);
                    let middle = column.block(local(centre));
                    assert!(!registry.info(middle).solid, "seed {seed}: a geode at {centre} is not hollow: {}", registry.def(middle).name);
                    // Straight up and down from the centre, the first solid block is the
                    // crystal lining, unless a cave cut through it.
                    for dir in [IVec3::Y, IVec3::NEG_Y] {
                        let reach = (geode.radius + caves::GEODE_SHELL).ceil() as i32;
                        let first = (1..=reach).map(|i| centre + dir * i).find(|&p| registry.info(column.block(local(p))).solid);
                        if let Some(p) = first {
                            let block = column.block(local(p));
                            // Where a cave cut away the lining, the open block before it lies
                            // past the hollow.
                            let cut = geode.distance(p - dir) > geode.radius + 0.5;
                            assert!(crystal.contains(&block) || cut, "seed {seed}: {} at {p} lines the geode at {centre}", registry.def(block).name);
                        }
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked >= 6, "only {checked} geodes");
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

    /// The stone pieces standing on the ground around `AT=x,z` (seed `SEED`): each piece's
    /// lowest block, its size, height above the ground and blocks, to find what made one.
    #[test]
    #[ignore]
    fn stone_pieces() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
        let at: Vec<i32> = std::env::var("AT").unwrap_or_else(|_| "0,0".into()).split(',').filter_map(|s| s.parse().ok()).collect();
        let registry = registry();
        let world = Overworld::new(seed, registry.clone());
        let first = ColumnPos::of_block(IVec3::new(at[0], 0, at[1])).offset(-1, -1);
        let columns = generate_area(&world, first, 3);
        let origin = first.origin();
        let span = 3 * voxmar_engine::coords::CHUNK_SIZE;
        let block = |p: IVec3| {
            if !(0..span).contains(&(p.x - origin.x)) || !(0..span).contains(&(p.z - origin.z)) || !(1..WORLD_HEIGHT_FOR_TESTS).contains(&p.y) {
                return BlockId::AIR;
            }
            columns[(((p.z - origin.z) >> 5) * 3 + ((p.x - origin.x) >> 5)) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31))
        };
        let stone = |b: BlockId| registry.def(b).has_tag("stone") || registry.def(b).name.ends_with("moss_block");
        let mut seen = rustc_hash::FxHashSet::default();
        for z in origin.z..origin.z + span {
            for x in origin.x..origin.x + span {
                let h = world.height(x, z);
                for y in h + 1..h + 5 {
                    let p = IVec3::new(x, y, z);
                    if !stone(block(p)) || !seen.insert(p) {
                        continue;
                    }
                    let mut stack = vec![p];
                    let mut cells = Vec::new();
                    while let Some(q) = stack.pop() {
                        cells.push(q);
                        for d in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                            let n = q + d;
                            if n.y > world.height(n.x, n.z) && stone(block(n)) && seen.insert(n) {
                                stack.push(n);
                            }
                        }
                    }
                    let high = cells.iter().map(|c| c.y - world.height(c.x, c.z)).max().unwrap();
                    let mut kinds = std::collections::BTreeMap::<String, i32>::new();
                    for c in &cells {
                        *kinds.entry(registry.def(block(*c)).name.trim_start_matches("base:").to_owned()).or_default() += 1;
                    }
                    let d = (p.x - at[0]).abs().max((p.z - at[1]).abs());
                    println!("{p} (d {d}): {} blocks, {high} high, {kinds:?}", cells.len());
                }
            }
        }
    }

    /// The level of the land the picker gives a point, with the lean distance gives it
    /// there or (`plain`) without, and whether that land is dry (not sea).
    fn picked_level(world: &Overworld, x: i32, z: i32, plain: bool) -> (u8, bool) {
        let mut regional = world.geology.climate().regional_at(&world.geology, IVec2::new(x, z));
        if plain {
            regional.lean = 0.0;
        }
        let biome = world.biomes.get(world.biomes.resolve(&world.point(&regional)));
        (biome.level, !biome.has_tag("ocean"))
    }

    /// Far from spawn higher-level biomes are likelier, never certain (P15): on the same
    /// land 10 to 20 km out, the lean raises the mean level and moves a share of the land
    /// to a higher-level biome, but most keeps its biome, some far land stays level 1 and
    /// next to none is pushed down; near the origin next to nothing changes.
    #[test]
    fn distance_makes_higher_levels_likelier() {
        let (mut far, mut raised, mut lowered, mut sum, mut plain_sum, mut gentle, mut near_moved) = (0, 0, 0, 0u32, 0u32, 0, 0);
        for seed in 1..=3 {
            let world = shared_world(seed);
            let mut rng = rng::Rng::new(seed, 0x1e7e1, 0, 0);
            for _ in 0..1500 {
                let (angle, distance) = (rng.unit() * std::f32::consts::TAU, 10_000.0 + 10_000.0 * rng.unit());
                let (x, z) = ((angle.cos() * distance) as i32, (angle.sin() * distance) as i32);
                let ((leaned, dry), (plain, _)) = (picked_level(&world, x, z, false), picked_level(&world, x, z, true));
                if !dry {
                    continue;
                }
                far += 1;
                sum += u32::from(leaned);
                plain_sum += u32::from(plain);
                raised += i32::from(leaned > plain);
                lowered += i32::from(leaned < plain);
                gentle += i32::from(leaned == 1);
            }
            for _ in 0..500 {
                let (x, z) = (rng.range(-350, 350), rng.range(-350, 350));
                near_moved += i32::from(picked_level(&world, x, z, false) != picked_level(&world, x, z, true));
            }
        }
        let (mean, plain_mean) = (sum as f32 / far as f32, plain_sum as f32 / far as f32);
        println!("{far} far samples: mean level {mean:.2} (plain {plain_mean:.2}), {raised} raised, {lowered} lowered, {gentle} at level 1; {near_moved} of 1500 near spawn moved");
        assert!(near_moved <= 15, "{near_moved} of 1500 points near spawn changed level");
        assert!(far > 2000 && mean > plain_mean + 0.05, "mean level {mean} against {plain_mean}");
        // A share of the land, not most of it: the climate still decides first.
        let share = raised as f32 / far as f32;
        assert!((0.03..0.4).contains(&share) && gentle > 0, "{share} of far land raised, {gentle} at level 1");
        assert!(lowered < raised / 4, "{lowered} lowered against {raised} raised");
    }

    /// The world page's level tables (P15): `cargo test -p voxmar_worldgen level_census --
    /// --ignored --nocapture` prints each level's multiplier for finds of each rarity, the
    /// share of each rarity among finds a level-1 pool holds in equal parts by rarity
    /// class, and over `SEEDS` (default 4) the share of dry land at each level by distance
    /// from the origin, with the lean and (in brackets) without it.
    #[test]
    #[ignore]
    fn level_census() {
        use crate::rarity::Tier;
        let content = WorldContent::base(&registry()).unwrap();
        let table = &content.climate.levels;
        let classes = [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::VeryRare, Rarity::Legendary];
        println!("level | multiplier: common, uncommon, rare, very rare, legendary | mix of a pool with one of each");
        for level in 1..=levels::MAX_LEVEL {
            let m: Vec<f32> = classes.iter().map(|&c| table.finds(level, c)).collect();
            let rates: Vec<f32> = classes.iter().zip(&m).map(|(&c, m)| levels::capped(c, c.multiplier() * m)).collect();
            let total: f32 = rates.iter().sum();
            let mix: Vec<String> = rates.iter().map(|r| format!("{:.2}%", 100.0 * r / total)).collect();
            println!("{level} | {} | {}", m.iter().map(|m| format!("{m:.2}")).collect::<Vec<_>>().join(", "), mix.join(", "));
        }
        let seeds: u64 = std::env::var("SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
        let rings = [(0.0, 2000.0), (2000.0, 5000.0), (5000.0, 10_000.0), (10_000.0, 15_000.0), (15_000.0, 25_000.0)];
        let registry = registry();
        let mut counts = vec![[[0u32; 11]; 2]; rings.len()];
        for seed in 1..=seeds {
            let world = Overworld::new(seed, registry.clone());
            let mut rng = rng::Rng::new(seed, 0xce45, 0, 0);
            for (r, &(inner, outer)) in rings.iter().enumerate() {
                for _ in 0..4000 {
                    let angle = rng.unit() * std::f32::consts::TAU;
                    let distance = (inner * inner + (outer * outer - inner * inner) * rng.unit()).sqrt();
                    let (x, z) = ((angle.cos() * distance) as i32, (angle.sin() * distance) as i32);
                    for plain in [false, true] {
                        let (level, dry) = picked_level(&world, x, z, plain);
                        if dry {
                            counts[r][plain as usize][level as usize] += 1;
                        }
                    }
                }
            }
        }
        println!("km | share of dry land at levels 1 ..= 10, with the lean (without) | mean");
        for (r, &(inner, outer)) in rings.iter().enumerate() {
            let [leaned, plain] = &counts[r];
            let (n, m) = (leaned.iter().sum::<u32>() as f32, plain.iter().sum::<u32>() as f32);
            let mean = |c: &[u32; 11], n: f32| c.iter().enumerate().map(|(l, &k)| l as f32 * k as f32).sum::<f32>() / n;
            let cells: Vec<String> = (1..=10).map(|l| format!("{:.1} ({:.1})", 100.0 * leaned[l] as f32 / n, 100.0 * plain[l] as f32 / m)).collect();
            println!("{}-{} | {} | {:.2} ({:.2})", inner / 1000.0, outer / 1000.0, cells.join(", "), mean(leaned, n), mean(plain, m));
        }
    }

    /// The world page's measured finds by level (P15): `cargo test -p voxmar_worldgen
    /// level_finds -- --ignored --nocapture` sets every biome to each of levels 1, 3, 5, 7
    /// and 10 in turn (the biome map stays the same, as the lean weighs equal levels alike),
    /// and counts the small plants the ecology grows on dry land within 256 blocks of
    /// spawn on seeds 1 and 2, every other block, by rarity class: per 10,000 columns and
    /// against level 1; and the trees' chances there, by class, as shares of the forest.
    #[test]
    #[ignore]
    fn level_finds() {
        let registry = registry();
        let base = WorldContent::base(&registry).unwrap();
        let mut class_of = rustc_hash::FxHashMap::default();
        for plant in &base.plants {
            let block = registry.expect(&plant.block);
            let class = plant.rarity.unwrap_or_else(|| registry.def(block).rarity);
            for look in registry.looks(block) {
                class_of.entry(look.block).or_insert(class);
            }
            class_of.entry(block).or_insert(class);
        }
        let names = ["common", "uncommon", "rare", "very rare", "legendary"];
        let mut first = [0f32; 5];
        println!("level | per 10,000 dry columns: {} | against level 1", names.join(", "));
        for level in [1u8, 3, 5, 7, 10] {
            let mut content = base.clone();
            for biome in &mut content.biomes {
                biome.level = level;
            }
            let (mut counts, mut dry, mut trees) = ([0u32; 5], 0u32, [0f64; 5]);
            let mut chances = Vec::new();
            for seed in [1, 2] {
                let world = Overworld::with_content(seed, registry.clone(), &content);
                let spawn = world.spawn_estimate();
                for z in (spawn.y - 256..spawn.y + 256).step_by(2) {
                    for x in (spawn.x - 256..spawn.x + 256).step_by(2) {
                        let site = world.site(x, z);
                        if site.height < site.water {
                            continue;
                        }
                        dry += 1;
                        if let Some(ecology::Plant::One(b) | ecology::Plant::Tall(b, _) | ecology::Plant::Stack(b, _)) = world.ecology.plant(&site, crate::content::Ground::Soil, x, z) {
                            counts[class_of.get(&b).map_or(0, |c| *c as usize)] += 1;
                        }
                        if (x | z) & 3 == 0 {
                            world.ecology.tree_chances(&site, None, x, z, &mut chances);
                            for &(tree, chance) in &chances {
                                trees[content.trees[tree.0 as usize].rarity as usize] += chance as f64;
                            }
                        }
                    }
                }
            }
            let per: Vec<f32> = counts.iter().map(|&c| 10_000.0 * c as f32 / dry as f32).collect();
            if level == 1 {
                first.copy_from_slice(&per);
            }
            let against: Vec<String> = per.iter().zip(first).map(|(p, f)| if f > 0.0 { format!("×{:.2}", p / f) } else { "-".into() }).collect();
            let forest: f64 = trees.iter().sum();
            let shares: Vec<String> = trees.iter().map(|t| format!("{:.1}%", 100.0 * t / forest)).collect();
            println!("{level} | {} | {} | trees {}", per.iter().map(|p| format!("{p:.1}")).collect::<Vec<_>>().join(", "), against.join(", "), shares.join(", "));
        }
    }

    /// Close-ups of kinds of tree in place, for captures: `TREES=olive:mediterranean,
    /// larch:taiga SEED=n` finds, in each biome near spawn, the places the tree is likeliest,
    /// then in the first of them that has one the trunk whose crown holds most of its own
    /// leaves (its fruit's, where it bears fruit), and prints `/tp` and a pose from a dozen
    /// blocks off (as `tree_sights`).
    #[test]
    #[ignore]
    fn gathering_sights() {
        use voxmar_engine::block::RenderKind;
        use voxmar_engine::coords::{CHUNK_SIZE, WORLD_HEIGHT};
        use voxmar_engine::glam::Vec3;
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(42);
        let registry = registry();
        let content = WorldContent::base(&registry).unwrap();
        let world = Overworld::new(seed, registry.clone());
        let spawn = world.spawn_point();
        let centre = IVec2::new(spawn.x, spawn.z);
        let mut chances = Vec::new();
        for pair in std::env::var("TREES").unwrap_or_default().split(',') {
            let Some((tree, biome)) = pair.split_once(':') else { continue };
            let Some(index) = content.trees.iter().position(|t| t.name == format!("base:{tree}")) else {
                println!("{tree}: no such tree");
                continue;
            };
            let def = &content.trees[index];
            // Its fruit's leaves at every stage of ripening, and its own leaves where no other
            // tree shares them.
            let fruit = def.fruit.as_ref().map(|f| f.leaves.clone());
            let leaves = def.leaves.clone().filter(|l| content.trees.iter().filter(|t| t.leaves.as_ref() == Some(l)).count() == 1);
            let marks: Vec<BlockId> = fruit
                .iter()
                .flat_map(|f| [f.clone(), format!("{f}_0"), format!("{f}_1"), format!("{f}_2")])
                .chain(leaves)
                .filter_map(|name| registry.id(&name))
                .collect();
            let Some(id) = world.biomes.id(&format!("base:{biome}")) else { continue };
            // The 8-block cells where the tree is likeliest, its chance summed over the cells
            // around, within 400 blocks of a few places inside the biome.
            let mut cells: Vec<(f32, IVec2)> = Vec::new();
            let mut from = centre;
            for _ in 0..3 {
                let Some(at) = world.locate_biome(biome, from, 12000) else { break };
                let sampler = world.sampler(at - 400, at + 400);
                let n = 100;
                let mut grid = vec![0.0f32; n * n];
                for (i, cell) in grid.iter_mut().enumerate() {
                    let p = at + IVec2::new((i % n) as i32 * 8 - 400, (i / n) as i32 * 8 - 400);
                    let site = sampler.site(p.x, p.y);
                    if site.home != id || site.height <= site.water {
                        continue;
                    }
                    let (ground, water) = world.surface_blocks(&site, p.x, p.y);
                    if water.is_none() {
                        world.ecology.tree_chances(&site, Some(world.terrain.ground(ground)), p.x, p.y, &mut chances);
                        *cell = chances.iter().filter(|c| c.0 .0 as usize == index).map(|c| c.1).sum();
                    }
                }
                for i in 0..n * n {
                    let (x, z) = ((i % n) as i32, (i / n) as i32);
                    let sum: f32 = (-2..=2).flat_map(|dz| (-2..=2).map(move |dx| (x + dx, z + dz))).filter(|&(u, v)| (0..n as i32).contains(&u) && (0..n as i32).contains(&v)).map(|(u, v)| grid[(v * n as i32 + u) as usize]).sum();
                    if sum > 0.0 {
                        cells.push((sum, at + IVec2::new(x * 8 - 400, z * 8 - 400)));
                    }
                }
                from = at + IVec2::new(1500, 900);
            }
            cells.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut places: Vec<IVec2> = Vec::new();
            for (_, p) in cells {
                if places.len() < 8 && places.iter().all(|q| (*q - p).abs().max_element() > 160) {
                    places.push(p);
                }
            }
            let found = places.iter().find_map(|&at| {
                let size = 5;
                let first = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(-2, -2);
                let columns = generate_area(&world, first, size);
                let origin = first.origin();
                let span = size * CHUNK_SIZE;
                let block = |p: IVec3| {
                    let inside = (0..span).contains(&(p.x - origin.x)) && (0..span).contains(&(p.z - origin.z)) && (0..WORLD_HEIGHT).contains(&p.y);
                    if inside { columns[(((p.z - origin.z) >> 5) * size + ((p.x - origin.x) >> 5)) as usize].block(p.with_x(p.x & 31).with_z(p.z & 31)) } else { BlockId::AIR }
                };
                let log = |b: BlockId| registry.def(b).name.ends_with("_log");
                let leaf = |b: BlockId| registry.info(b).render == RenderKind::Leaves;
                let open = |b: BlockId| b.is_air() || registry.info(b).render == RenderKind::Plant;
                let ground = |x: i32, z: i32| (1..WORLD_HEIGHT).rev().find(|&y| { let b = block(IVec3::new(x, y, z)); !open(b) && !log(b) && !leaf(b) });
                // Trunks by how many of the tree's own leaves lie within four blocks of them.
                let mut trunk: Option<(IVec3, usize)> = None;
                for z in origin.z + 16..origin.z + span - 16 {
                    for x in origin.x + 16..origin.x + span - 16 {
                        let Some(g) = ground(x, z) else { continue };
                        let base = IVec3::new(x, g + 1, z);
                        if !log(block(base)) {
                            continue;
                        }
                        let own = (1..20).flat_map(|dy| (-4..=4).flat_map(move |dz| (-4..=4).map(move |dx| IVec3::new(dx, dy, dz)))).filter(|&d| marks.contains(&block(base + d))).count();
                        if own > 0 && trunk.is_none_or(|t| own > t.1) {
                            trunk = Some((base, own));
                        }
                    }
                }
                let (base, own) = trunk?;
                let crown = base.as_vec3() + Vec3::new(0.5, (def.height.0 + def.height.1) as f32 * 0.35, 0.5);
                let eye = |x: i32, z: i32| ground(x, z).map(|g| Vec3::new(x as f32 + 0.5, g as f32 + 2.62, z as f32 + 0.5));
                let blocked = |from: Vec3, to: Vec3| (1..40).filter(|&i| !open(block((from + (to - from) * (i as f32 / 40.0)).floor().as_ivec3()))).count();
                // From twelve blocks off and a little up, on the clearest line to the crown.
                let (spot, from) = (0..16)
                    .filter_map(|i| {
                        let a = i as f32 / 16.0 * std::f32::consts::TAU;
                        let p = IVec3::new(base.x + (a.cos() * 12.0).round() as i32, 0, base.z + (a.sin() * 12.0).round() as i32);
                        eye(p.x, p.z).map(|e| (p, e + Vec3::Y * 2.0))
                    })
                    .min_by_key(|(_, e)| blocked(*e, crown))?;
                let player = eye(spot.x, spot.z)?;
                let d = crown - from;
                let (yaw, pitch) = ((-d.x).atan2(-d.z).to_degrees(), d.y.atan2(Vec3::new(d.x, 0.0, d.z).length()).to_degrees());
                let (o, feet) = (from - player, player - Vec3::Y * 1.62);
                Some(format!("trunk {base} ({own} own leaves): --command \"/tp {} {} {}\" --pose {:.1},{:.1},{:.1},{:.0},{:.0}", feet.x, feet.y, feet.z, o.x, o.y, o.z, yaw, pitch))
            });
            match found {
                Some(line) => println!("{tree} in {biome}, {line}"),
                None => println!("{tree} in {biome}: none in the {} likeliest places", places.len()),
            }
        }
    }

    /// `cargo test --release -p voxmar_worldgen cave_sights -- --ignored --nocapture`
    /// (optionally `SEED=n`, or several as `SEEDS=1,2,3`, and a place `X`, `Z` other than
    /// spawn, or `NEAR=biome` for the nearest of that biome) prints `--pose` arguments for
    /// in-game captures of caves near spawn: inside the largest open space under each sight.
    #[test]
    #[ignore]
    fn cave_sights() {
        let registry = registry();
        let seeds = std::env::var("SEEDS").or_else(|_| std::env::var("SEED")).unwrap_or("24301".into());
        let (water, lava) = (registry.expect("base:water"), registry.expect("base:lava"));
        for seed in seeds.split(',').map(|s| s.parse::<u64>().unwrap()) {
            let world = Overworld::new(seed, registry.clone());
            // Around spawn, or around `X`, `Z`, which a capture reaches with
            // `--command "/tp X Z"` (the poses are from the eye of a player standing there).
            // `NEAR=biome` finds the nearest of that surface biome to spawn instead.
            let near = std::env::var("NEAR").ok().and_then(|name| {
                let spawn = world.spawn_point();
                world.locate_biome(&name, IVec2::new(spawn.x, spawn.z), 8000).map(|p| (p.x, p.y))
            });
            let place = near.or_else(|| std::env::var("X").ok().zip(std::env::var("Z").ok()).and_then(|(x, z)| Some((x.parse::<i32>().ok()?, z.parse::<i32>().ok()?))));
            let spawn = match place {
                Some((x, z)) => {
                    println!("  capture with --command \"/tp {x} {z}\"");
                    IVec3::new(x, world.height(x, z) + 1, z)
                }
                None => world.spawn_point(),
            };
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
            // Glowing blocks light a view; lava does too, but a view over a lava sea shows
            // the lava rather than the cave biome, so views near it are left out.
            let lights: Vec<BlockId> = (0..registry.len() as u16).map(BlockId).filter(|&b| registry.def(b).light > 0 && b != lava).collect();
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
                            let around = || (-8..=8).flat_map(|dz| (-6..=6).flat_map(move |dy| (-8..=8).map(move |dx| IVec3::new(dx, dy, dz))));
                            let lit = around().filter(|&d| block(camera + d).is_some_and(|b| lights.contains(&b))).count() as i32;
                            let molten = !biomes[i].0.contains("molten") && (camera.y < caves::LAVA_LEVEL + 8 || around().any(|d| block(camera + d) == Some(lava)));
                            let score = if molten { i32::MIN + 1 } else { view + run.min(20) + 12 * lit.min(4) };
                            if score > biomes[i].2.0 {
                                biomes[i].2 = (score, camera, yaw);
                            }
                        }
                        run = 0;
                    }
                }
            }
            // Geodes, from inside, and fossils where a cave lays them bare.
            let air_around = |p: IVec3| [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::Y, IVec3::NEG_Y].into_iter().find(|&d| air(p + d));
            let mut fossils: Vec<IVec3> = Vec::new();
            for i in 0..19 * 19 {
                let corner = first.offset(i % 19, i / 19).origin();
                let geology = geology::GeoGrid::new(&world, (corner.x - 1, corner.z - 1), voxmar_engine::coords::CHUNK_SIZE + 2);
                for geode in world.caves.geodes_near(&geology, corner) {
                    let c = geode.centre.floor().as_ivec3();
                    if ColumnPos::of_block(c) == ColumnPos::of_block(corner) && air(c) {
                        let d = c.as_vec3() + 0.5 - eye;
                        println!("  geode, {:.0} across at {c}: --pose {:.1},{:.1},{:.1},0,-30", 2.0 * geode.radius, d.x, d.y, d.z);
                    }
                }
                for fossil in world.underground.fossils_near(&world, corner) {
                    if fossils.contains(&fossil.min) {
                        continue;
                    }
                    fossils.push(fossil.min);
                    let bare: Vec<(IVec3, IVec3)> = (fossil.min.y..=fossil.max.y)
                        .flat_map(|y| (fossil.min.z..=fossil.max.z).flat_map(move |z| (fossil.min.x..=fossil.max.x).map(move |x| IVec3::new(x, y, z))))
                        .filter(|&p| fossil.contains(p) && block(p).is_some_and(|b| !b.is_air()))
                        .filter_map(|p| air_around(p).map(|d| (p, d)))
                        .collect();
                    // From as far back as the cave allows, at the barest part.
                    let view = |&(p, d): &(IVec3, IVec3)| (1..9).map(|k| p + d * k).take_while(|&c| air(c)).count();
                    if let Some(&(p, d)) = bare.iter().max_by_key(|b| (view(b), bare.iter().filter(|o| o.0.distance_squared(b.0) < 16).count())) {
                        println!("  ({} bones bare)", bare.len());
                        let camera = p + d * view(&(p, d)) as i32;
                        let look = (p - camera).as_vec3();
                        let yaw = (-look.x).atan2(-look.z).to_degrees();
                        let e = camera.as_vec3() + 0.5 - eye;
                        println!("  fossil laid bare at {p}: --pose {:.1},{:.1},{:.1},{yaw:.0},-20", e.x, e.y, e.z);
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

    /// `SEED=1,7,42 cargo test -p voxmar_worldgen biome_spread -- --ignored --nocapture`
    /// prints the biome statistics of `map_preview` (region sizes, and the biomes within
    /// 5 km of the middle, the sea's included) for its 8 km map around the origin, from the
    /// land and the picker alone, without rivers or a picture: quick, for tuning biomes.
    #[test]
    #[ignore]
    fn biome_spread() {
        let seeds: Vec<u64> = std::env::var("SEED").ok().map_or(vec![7], |s| s.split(',').filter_map(|s| s.trim().parse().ok()).collect());
        const SIZE: i32 = 1024;
        // `SCALE=16` for a 16 km map, whose edges cut fewer regions short.
        let scale_env: i32 = std::env::var("SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
        let half = SIZE * scale_env / 2;
        let mut sums = [0.0f32; 3];
        let mut shares: rustc_hash::FxHashMap<String, usize> = Default::default();
        for &seed in &seeds {
            let world = Overworld::new(seed, registry());
            let sampler = world.sampler(IVec2::splat(-half - scale_env), IVec2::splat(half + scale_env));
            let mut homes = vec![u16::MAX; (SIZE * SIZE) as usize];
            let mut bands = vec![[0u8; 4]; (SIZE * SIZE) as usize];
            let mut near = vec![false; world.biomes.len()];
            for i in 0..SIZE * SIZE {
                let (x, z) = (i % SIZE * scale_env - half, i / SIZE * scale_env - half);
                let home = sampler.pick(x, z).home;
                bands[i as usize] = world.biomes.bands(&world.point(&sampler.grid.at(x, z)));
                if x * x + z * z <= 5000 * 5000 {
                    near[home.0 as usize] = true;
                }
                if world.geology.land(x, z).height > SEA_LEVEL as f32 {
                    homes[i as usize] = world.biomes.parent_of(home).0;
                    *shares.entry(world.biomes.get(home).name.trim_start_matches("base:").to_owned()).or_default() += 1;
                }
            }
            let names: Vec<&str> = (0..near.len()).filter(|&i| near[i]).map(|i| world.biomes.get(BiomeId(i as u16)).name.trim_start_matches("base:")).collect();
            let parents: std::collections::BTreeSet<u16> = (0..near.len()).filter(|&i| near[i]).map(|i| world.biomes.parent_of(BiomeId(i as u16)).0).collect();
            println!("seed {seed}: {} biomes within 5 km of the middle ({} without variants and ecotones): {}", names.len(), parents.len(), names.join(", "));
            let (median, big) = biome_sizes(&world, &homes, SIZE, scale_env);
            borders(&world, &homes, &bands, SIZE);
            if std::env::var("SMALL").is_ok() {
                // A picture of the land in regions under a kilometre across (red, darker the
                // smaller), the rest grey by biome, sea black.
                let size = small_regions(&homes, SIZE, scale_env);
                let mut file = format!("P6 {SIZE} {SIZE} 255\n").into_bytes();
                for i in 0..homes.len() {
                    file.extend(match homes[i] {
                        u16::MAX => [0, 0, 40],
                        _ if size[i] < 1000.0 => [(120.0 + 135.0 * size[i] / 1000.0) as u8, 30, 30],
                        home => [(90 + home as u32 * 37 % 120) as u8; 3],
                    });
                }
                std::fs::write(format!("{}/../../target/small_{seed}.ppm", env!("CARGO_MANIFEST_DIR")), file).unwrap();
            }
            if std::env::var("AXES").is_ok() {
                // The regions of each axis' bands alone, and of the table's cells, on land.
                for (name, key) in [("temperature", 0usize), ("moisture", 1), ("continentalness", 2), ("landform", 3), ("cell", 4)] {
                    let ids: Vec<u16> = (0..homes.len())
                        .map(|i| match homes[i] {
                            u16::MAX => u16::MAX,
                            _ if key == 4 => bands[i].iter().fold(0u16, |id, &b| id * 8 + b as u16),
                            _ => bands[i][key] as u16,
                        })
                        .collect();
                    print!("{name} bands alone: ");
                    region_sizes(&ids, SIZE, scale_env, None);
                }
            }
            sums = [sums[0] + median, sums[1] + big, sums[2] + names.len() as f32];
        }
        let total: usize = shares.values().sum::<usize>().max(1);
        let mut shares: Vec<(String, usize)> = shares.into_iter().collect();
        shares.sort_by_key(|s| std::cmp::Reverse(s.1));
        println!("land by biome: {}", shares.iter().map(|(name, n)| format!("{name} {:.1}%", 100.0 * *n as f32 / total as f32)).collect::<Vec<_>>().join(", "));
        let n = seeds.len().max(1) as f32;
        println!("mean: median region {:.0} m, {:.0}% of land in regions ≥1 km, {:.1} biomes within 5 km", sums[0] / n, 100.0 * sums[1] / n, sums[2] / n);
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
        let world = shared_world(seed);
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
        let mut homes = vec![u16::MAX; (SIZE * SIZE) as usize];
        let mut bands = vec![[0u8; 4]; (SIZE * SIZE) as usize];
        let mut near_spawn = vec![false; world.biomes.len()];
        for z in 0..SIZE {
            for x in 0..SIZE {
                let h = height(x, z);
                let slope = (height(x + 1, z) - h).abs().max((height(x, z + 1) - h).abs()) / scale;
                let surface = sampler.surface(ax(x), az(z), h, slope);
                let site = sampler.site(ax(x), az(z));
                area[site.biome.0 as usize] += 1;
                // Land, leaving out the shore the sea's biome reaches onto.
                let shore = world.biomes.get(site.home).has_tag("ocean");
                if (!site.body.eq(&Body::Sea) || site.height >= site.water) && !shore {
                    homes[(z * SIZE + x) as usize] = world.biomes.parent_of(site.home).0;
                    bands[(z * SIZE + x) as usize] = world.biomes.bands(&world.point(&sampler.grid.at(ax(x), az(z))));
                }
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
        // Trails over everything: brown on the ground, cyan on fords, red on bridges, and
        // the places they join (summits white, lake shores turquoise, waterfalls blue,
        // viewpoints yellow, trailheads green). `TRAILS=0` leaves them out.
        if std::env::var("TRAILS").map_or(true, |v| v != "0") {
            let started = std::time::Instant::now();
            let (lines, places) = world.trails_in(centre - half, centre + half);
            let mut paint = |at: voxmar_engine::glam::Vec2, rgb: [u8; 3], r: i32| {
                let (px, pz) = (((at.x - (centre.x - half) as f32) / scale as f32) as i32, ((at.y - (centre.y - half) as f32) / scale as f32) as i32);
                for (dx, dz) in (-r..=r).flat_map(|j| (-r..=r).map(move |i| (i, j))) {
                    let (x, z) = (px + dx, pz + dz);
                    if (0..SIZE).contains(&x) && (0..SIZE).contains(&z) {
                        let i = ((z * SIZE + x) * 3) as usize;
                        pixels[i..i + 3].copy_from_slice(&rgb);
                    }
                }
            };
            let length: usize = lines.iter().map(|l| l.len()).sum();
            for line in &lines {
                for &(at, kind) in line {
                    paint(at, [[110, 60, 20], [40, 230, 230], [230, 30, 30]][kind as usize], if scale <= 2 { 1 } else { 0 });
                }
            }
            for place in &places {
                let rgb = match place.kind {
                    PlaceKind::Summit => [255, 255, 255],
                    PlaceKind::Lake => [40, 220, 200],
                    PlaceKind::Waterfall => [30, 90, 255],
                    PlaceKind::Viewpoint => [255, 230, 40],
                    PlaceKind::Trailhead => [60, 220, 60],
                    PlaceKind::Settlement => [255, 90, 200],
                };
                paint(place.at().as_vec2(), rgb, 2);
            }
            println!("{} trails, {} km of them, {} places, in {:.1?}", lines.len(), length / 1000, places.len(), started.elapsed());
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
        biome_sizes(&world, &homes, SIZE, scale);
        borders(&world, &homes, &bands, SIZE);
    }

    /// Which axis of the biome table changes across the borders between biomes on a map of
    /// `homes` (a biome per pixel, `u16::MAX` for sea) whose table bands are `bands`, and the
    /// commonest borders of each kind.
    fn borders(world: &Overworld, homes: &[u16], bands: &[[u8; 4]], size: i32) {
        // Which axis changes across each border between biomes (a border can count twice).
        let mut causes = [0usize; 5];
        let mut pairs: rustc_hash::FxHashMap<(u16, u16), usize> = Default::default();
        let mut crossing: [rustc_hash::FxHashMap<(u16, u16), usize>; 4] = Default::default();
        for z in 0..size - 1 {
            for x in 0..size - 1 {
                let i = (z * size + x) as usize;
                for j in [i + 1, i + size as usize] {
                    if homes[i] == u16::MAX || homes[j] == u16::MAX || homes[i] == homes[j] {
                        continue;
                    }
                    let differ: Vec<usize> = (0..4).filter(|&a| bands[i][a] != bands[j][a]).collect();
                    if differ.is_empty() {
                        causes[4] += 1;
                        *pairs.entry((homes[i].min(homes[j]), homes[i].max(homes[j]))).or_default() += 1;
                    }
                    for a in differ {
                        causes[a] += 1;
                        *crossing[a].entry((homes[i].min(homes[j]), homes[i].max(homes[j]))).or_default() += 1;
                    }
                }
            }
        }
        let total: usize = causes.iter().sum::<usize>().max(1);
        println!(
            "borders by axis: temperature {:.0}%, moisture {:.0}%, continentalness {:.0}%, landform {:.0}%, within a cell (weirdness, aether, ecotones) {:.0}%",
            100.0 * causes[0] as f32 / total as f32,
            100.0 * causes[1] as f32 / total as f32,
            100.0 * causes[2] as f32 / total as f32,
            100.0 * causes[3] as f32 / total as f32,
            100.0 * causes[4] as f32 / total as f32
        );
        let mut pairs: Vec<((u16, u16), usize)> = pairs.into_iter().collect();
        pairs.sort_by_key(|p| std::cmp::Reverse(p.1));
        let name = |b: u16| world.biomes.get(BiomeId(b)).name.trim_start_matches("base:").to_owned();
        println!("within-cell borders: {}", pairs.iter().take(12).map(|((a, b), n)| format!("{}|{} {}", name(*a), name(*b), n)).collect::<Vec<_>>().join(", "));
        for (axis, crossing) in ["temperature", "moisture", "continentalness", "landform"].iter().zip(crossing) {
            let mut crossing: Vec<((u16, u16), usize)> = crossing.into_iter().collect();
            crossing.sort_by_key(|p| std::cmp::Reverse(p.1));
            println!("{axis} borders: {}", crossing.iter().take(10).map(|((a, b), n)| format!("{}|{} {}", name(*a), name(*b), n)).collect::<Vec<_>>().join(", "));
        }
        }

    /// How large the land biomes' regions are on a map of `homes` (a biome per pixel,
    /// `u16::MAX` for sea), `scale` blocks per pixel: each region's size as the diameter of a
    /// circle of its area, and the share of land in regions at least a kilometre across.
    fn biome_sizes(world: &Overworld, homes: &[u16], size: i32, scale: i32) -> (f32, f32) {
        let (median, big) = region_sizes(homes, size, scale, Some(world));
        (median, big)
    }

    /// The diameter of the region each pixel of `homes` lies in, in blocks.
    fn small_regions(homes: &[u16], size: i32, scale: i32) -> Vec<f32> {
        let mut out = vec![f32::MAX; homes.len()];
        let mut seen = vec![false; homes.len()];
        for start in 0..homes.len() {
            if seen[start] || homes[start] == u16::MAX {
                continue;
            }
            let (mut stack, mut members) = (vec![start], Vec::new());
            seen[start] = true;
            while let Some(i) = stack.pop() {
                members.push(i);
                let (x, z) = (i as i32 % size, i as i32 / size);
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, nz) = (x + dx, z + dz);
                    if (0..size).contains(&nx) && (0..size).contains(&nz) {
                        let j = (nz * size + nx) as usize;
                        if !seen[j] && homes[j] == homes[start] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
            let diameter = 2.0 * (members.len() as f32 * (scale * scale) as f32 / std::f32::consts::PI).sqrt();
            for i in members {
                out[i] = diameter;
            }
        }
        out
    }

    /// Like `biome_sizes`, for any map of ids (`u16::MAX` for sea); `world` names biomes in
    /// the breakdowns, which are left out without it.
    fn region_sizes(homes: &[u16], size: i32, scale: i32, world: Option<&Overworld>) -> (f32, f32) {
        let mut seen = vec![false; homes.len()];
        let mut regions: Vec<(u16, usize)> = Vec::new();
        for start in 0..homes.len() {
            if seen[start] || homes[start] == u16::MAX {
                continue;
            }
            let id = homes[start];
            let (mut stack, mut count) = (vec![start], 0);
            seen[start] = true;
            while let Some(i) = stack.pop() {
                count += 1;
                let (x, z) = (i as i32 % size, i as i32 / size);
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, nz) = (x + dx, z + dz);
                    if (0..size).contains(&nx) && (0..size).contains(&nz) {
                        let j = (nz * size + nx) as usize;
                        if !seen[j] && homes[j] == id {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
            regions.push((id, count));
        }
        let diameter = |pixels: usize| 2.0 * (pixels as f32 * (scale * scale) as f32 / std::f32::consts::PI).sqrt();
        let land: usize = regions.iter().map(|r| r.1).sum();
        let big: usize = regions.iter().filter(|r| diameter(r.1) >= 1000.0).map(|r| r.1).sum();
        let mid: usize = regions.iter().filter(|r| (400.0..1000.0).contains(&diameter(r.1))).map(|r| r.1).sum();
        // The diameter of the region the median block of land lies in.
        let mut by_size: Vec<(f32, usize)> = regions.iter().map(|r| (diameter(r.1), r.1)).collect();
        by_size.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut acc = 0;
        let median = by_size.iter().find(|r| {
            acc += r.1;
            acc * 2 >= land
        });
        println!(
            "biome regions: land in regions ≥1 km across {:.0}%, 0.4-1 km {:.0}%; the median block of land lies in a region {:.0} m across",
            100.0 * big as f32 / land.max(1) as f32,
            100.0 * mid as f32 / land.max(1) as f32,
            median.map_or(0.0, |r| r.0)
        );
        let Some(world) = world else { return (median.map_or(0.0, |r| r.0), big as f32 / land.max(1) as f32) };
        // Which biomes hold the land in regions under 600 m across.
        let mut small: rustc_hash::FxHashMap<u16, usize> = Default::default();
        for r in regions.iter().filter(|r| diameter(r.1) < 600.0) {
            *small.entry(r.0).or_default() += r.1;
        }
        let mut small: Vec<(u16, usize)> = small.into_iter().collect();
        small.sort_by_key(|s| std::cmp::Reverse(s.1));
        println!(
            "land in regions under 600 m: {}",
            small.iter().take(12).map(|(b, n)| format!("{} {:.1}%", world.biomes.get(BiomeId(*b)).name.trim_start_matches("base:"), 100.0 * *n as f32 / land.max(1) as f32)).collect::<Vec<_>>().join(", ")
        );
        // The largest region of each base biome (not variants, ecotones or water).
        let mut largest: Vec<(f32, &str)> = (0..world.biomes.len())
            .filter_map(|b| {
                let biome = world.biomes.get(BiomeId(b as u16));
                let best = regions.iter().filter(|r| r.0 as usize == b).map(|r| r.1).max()?;
                (!biome.has_tag("ocean") && !biome.has_tag("water")).then(|| (diameter(best), biome.name.as_str()))
            })
            .collect();
        largest.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("largest region per biome: {}", largest.iter().map(|(d, n)| format!("{} {:.1} km", n.trim_start_matches("base:"), d / 1000.0)).collect::<Vec<_>>().join(", "));
        (median.map_or(0.0, |r| r.0), big as f32 / land.max(1) as f32)
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

    /// `cargo test --release -p voxmar_worldgen river_speed -- --ignored --nocapture`
    /// separates the two costs rivers add, in CPU time on one thread: building a region's
    /// rivers (once, for its 32×32 columns) and generating columns once it is built.
    #[test]
    #[ignore]
    fn river_speed() {
        // CPU time on this thread measures all the work only if it all stays here.
        crate::parallel::set_enabled(false);
        crate::perf::prefer_fast_cores();
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
        // CPU time on this thread measures all the work only if it all stays here.
        crate::parallel::set_enabled(false);
        crate::perf::prefer_fast_cores();
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
        let world = shared_world(11);
        for x in [-2000, 0, 3000] {
            let sampler = world.sampler(IVec2::new(x, -8000), IVec2::new(x, 8000));
            // Sea-level temperature: what the site would have without cooling with height.
            let sea_level = |z: i32| {
                let site = sampler.site(x, z);
                site.temperature + LAPSE_RATE * (site.height.min(site.form.base.floor() as i32) - SEA_LEVEL).max(0) as f32
            };
            let temperatures: Vec<f32> = (-8000..=8000).step_by(8).map(sea_level).collect();
            let steepest = temperatures.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
            assert!(steepest < 0.3, "temperature jumps {steepest} °C over 8 blocks at x {x}");
            // Climate zones are kilometres wide: from cold to hot over this line, longer than
            // the way from a pole to the equator.
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
        // Some world must have a desert, or the test would pass for want of one.
        let mut any_desert = false;
        use rayon::prelude::*;
        let half = SIZE * STEP / 2;
        // Each world on its own core, and each of its columns of samples.
        let worlds: Vec<(u64, Vec<bool>, Vec<i32>, Vec<(i32, i32, i32)>)> = [1u64, 3, 0x5eed].into_par_iter().map(|seed| {
            let world = shared_world(seed);
            let sampler = world.sampler(IVec2::splat(-half - 1), IVec2::splat(half + 1));
            let rows: Vec<Vec<(bool, i32, bool)>> = (0..SIZE).into_par_iter().map(|i| {
                (0..SIZE).map(|j| {
                    let (x, z) = (i * STEP - half, j * STEP - half);
                    let site = sampler.site(x, z);
                    let top = world.surface_layers(&site, x, z).top;
                    // Warm sand away from beaches, banks and the sea's islands is desert (cold
                    // steppes have sandy patches too).
                    let shore = world.biomes.get(site.home).has_tag("ocean");
                    let desert = (top == sand || top == red_sand) && site.height > SEA_LEVEL + 3 && site.river < 0.3 && !shore && site.temperature >= 17.0;
                    (snowy.contains(&top), site.height, desert)
                }).collect()
            }).collect();
            let mut snow = vec![false; (SIZE * SIZE) as usize];
            let mut heights = vec![0; (SIZE * SIZE) as usize];
            let mut deserts = Vec::new();
            for (i, row) in rows.into_iter().enumerate() {
                for (j, (snowy, height, desert)) in row.into_iter().enumerate() {
                    let (i, j) = (i as i32, j as i32);
                    snow[(j * SIZE + i) as usize] = snowy;
                    heights[(j * SIZE + i) as usize] = height;
                    if desert {
                        deserts.push((i, j, height));
                    }
                }
            }
            (seed, snow, heights, deserts)
        }).collect();
        for (seed, snow, heights, deserts) in worlds {
            any_desert |= !deserts.is_empty();
            for (i, j, height) in deserts {
                for dj in -GAP..=GAP {
                    for di in -GAP..=GAP {
                        let (si, sj) = (i + di, j + dj);
                        let inside = (0..SIZE).contains(&si) && (0..SIZE).contains(&sj);
                        // Snow on a peak far above the desert is the mountain's, not the
                        // desert's neighbour.
                        if inside && di * di + dj * dj <= GAP * GAP && snow[(sj * SIZE + si) as usize] && heights[(sj * SIZE + si) as usize] < height + 100 {
                            let (desert, snow) = ((i * STEP - half, j * STEP - half), (si * STEP - half, sj * STEP - half));
                            panic!("seed {seed}: desert at {desert:?} within {} blocks of snow at {snow:?}", GAP * STEP);
                        }
                    }
                }
            }
        }
        assert!(any_desert, "no desert in any of the three worlds");
    }

    #[test]
    fn selection_fields_are_smooth_and_evenly_spread() {
        let world = shared_world(5);
        let sampler = world.sampler(IVec2::splat(-6000), IVec2::splat(6000));
        let sites: Vec<Site> = (0..150 * 150).map(|i| sampler.site(i % 150 * 80 - 6000, i / 150 * 80 - 6000)).collect();
        // Evenly spread fields put a tenth of the land in each tenth of their range. Those
        // varying over many kilometres are sampled over a wider area.
        let wide: Vec<Regional> = (0..100 * 100).map(|i| world.geology.climate().regional_at(&world.geology, IVec2::new(i % 100 * 600 - 30000, i / 100 * 600 - 30000))).collect();
        let weirdness: Vec<f32> = wide.iter().map(|r| r.weirdness).collect();
        let variant: Vec<f32> = wide.iter().map(|r| r.variant).collect();
        let fertility: Vec<f32> = sites.iter().map(|s| s.fertility).collect();
        for (name, values) in [("weirdness", weirdness), ("variant", variant), ("fertility", fertility)] {
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
        let aether = median(wide.iter().map(|r| r.aether).collect());
        assert!((aether - world.dials().fantasy).abs() < 0.1, "median aether {aether}, fantasy {}", world.dials().fantasy);
        assert!(sites.iter().all(|s| (-1.0..=1.0).contains(&s.continentalness) && (0.0..=1.0).contains(&s.erosion)));
        assert!(sites.iter().any(|s| s.continentalness < -0.5) && sites.iter().any(|s| s.continentalness > 0.5));
        assert!(wide.iter().any(|r| r.erosion > 0.9) && wide.iter().any(|r| r.erosion < 0.05));

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
        let world = shared_world(3);
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
        let world = shared_world(3);
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
        let (world, desert) = [3, 7, 42, 1, 2, 5, 9, 11, 13]
            .into_iter()
            .find_map(|seed| {
                let world = shared_world(seed);
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
            let world = shared_world(seed);
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
                if matches!(def.kind, content::ShapeKind::Volcanoes { .. }) {
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
        let world = shared_world(3);
        let sampler = world.sampler(IVec2::new(-300, 200), IVec2::new(100, 700));
        for (x, z) in [(-300, 200), (-1, 450), (37, 699), (100, 700)] {
            assert_eq!(sampler.site(x, z), world.site(x, z));
        }
    }

    /// `SEED=7 X=3600 Z=680 SIZE=4 cargo test -p voxmar_worldgen surface_patches -- --ignored --nocapture`
    ///
    /// How patchy the ground is around a position, by steepness: the share of its top
    /// blocks (under trees and plants) of each kind, how often neighbours differ, and how
    /// many blocks stand alone, differing from three or four of their neighbours, the
    /// speckle that makes ground look busy. `MAP=1` draws the top blocks, one letter per
    /// `STEP` blocks.
    #[test]
    #[ignore]
    fn surface_patches() {
        use voxmar_engine::block::RenderKind;
        use voxmar_engine::coords::CHUNK_SIZE;
        let env = |name: &str| std::env::var(name).ok();
        let seed: u64 = env("SEED").and_then(|s| s.parse().ok()).unwrap_or(7);
        let size: i32 = env("SIZE").and_then(|s| s.parse().ok()).unwrap_or(4);
        let x: i32 = env("X").and_then(|s| s.parse().ok()).unwrap_or(3600);
        let z: i32 = env("Z").and_then(|s| s.parse().ok()).unwrap_or(680);
        let step: i32 = env("STEP").and_then(|s| s.parse().ok()).unwrap_or(2);
        let registry = registry();
        let world = Overworld::new(seed, registry.clone());
        let first = ColumnPos::of_block(IVec3::new(x, 0, z)).offset(-size / 2, -size / 2);
        let columns = generate_area(&world, first, size);
        let origin = first.origin();
        let span = size * CHUNK_SIZE;
        let top = |dx: i32, dz: i32| {
            let c = &columns[((dz >> 5) * size + (dx >> 5)) as usize];
            let mut y = world.height(origin.x + dx, origin.z + dz) + 2;
            loop {
                let b = c.block(IVec3::new(dx & 31, y, dz & 31));
                let def = registry.def(b);
                if !b.is_air() && def.solid && def.render != RenderKind::Leaves && !def.name.contains("log") && !def.name.contains("wood") {
                    return b;
                }
                y -= 1;
            }
        };
        let tops: Vec<BlockId> = (0..span * span).map(|i| top(i % span, i / span)).collect();
        let at = |dx: i32, dz: i32| tops[(dz * span + dx) as usize];
        let steep = |dx: i32, dz: i32| steepness(|a, b| world.height(origin.x + dx + a, origin.z + dz + b));
        let mut bins: Vec<std::collections::BTreeMap<String, u32>> = vec![Default::default(); 4];
        let (mut pairs, mut changes, mut lonely, mut inner) = ([0u32; 4], [0u32; 4], [0u32; 4], [0u32; 4]);
        let mut legend: Vec<BlockId> = Vec::new();
        // Lone blocks by what they are and what surrounds them.
        let mut kinds: std::collections::BTreeMap<String, u32> = Default::default();
        let mut rows = Vec::new();
        for dz in 1..span - 1 {
            let mut row = String::new();
            for dx in 1..span - 1 {
                let s = steep(dx, dz);
                let bin = if s < 0.7 { 0 } else if s < 1.3 { 1 } else if s < 2.0 { 2 } else { 3 };
                let b = at(dx, dz);
                *bins[bin].entry(registry.def(b).name.clone()).or_default() += 1;
                let n = [at(dx + 1, dz), at(dx - 1, dz), at(dx, dz + 1), at(dx, dz - 1)];
                pairs[bin] += 2;
                changes[bin] += (n[0] != b) as u32 + (n[2] != b) as u32;
                inner[bin] += 1;
                if n.iter().filter(|&&o| o != b).count() >= 3 {
                    lonely[bin] += 1;
                    let other = n.iter().find(|&&o| o != b).copied().unwrap();
                    *kinds.entry(format!("{} in {}", registry.def(b).name, registry.def(other).name)).or_insert(0u32) += 1;
                }
                let k = legend.iter().position(|&l| l == b).unwrap_or_else(|| {
                    legend.push(b);
                    legend.len() - 1
                });
                if dx % step == 1 % step && dz % step == 1 % step {
                    row.push((b'a' + (k % 26) as u8) as char);
                }
            }
            if dz % step == 1 % step {
                rows.push(row);
            }
        }
        for (i, name) in ["gentle <0.7", "0.7..1.3", "1.3..2", "steep >2"].iter().enumerate() {
            let total: u32 = bins[i].values().sum();
            let mut list: Vec<_> = bins[i].iter().collect();
            list.sort_by(|a, b| b.1.cmp(a.1));
            let shares: Vec<String> = list.iter().take(6).map(|(n, c)| format!("{n} {:.0}%", 100.0 * **c as f32 / total.max(1) as f32)).collect();
            println!(
                "{name}: {total} cols, changes {:.1}%, lonely {:.1}%: {}",
                100.0 * changes[i] as f32 / pairs[i].max(1) as f32,
                100.0 * lonely[i] as f32 / inner[i].max(1) as f32,
                shares.join(", ")
            );
        }
        let mut kinds: Vec<_> = kinds.into_iter().collect();
        kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
        println!("lonely: {:?}", &kinds[..kinds.len().min(12)]);
        if env("MAP").is_some() {
            for (k, b) in legend.iter().enumerate() {
                println!("{} = {}", (b'a' + (k % 26) as u8) as char, registry.def(*b).name);
            }
            for row in rows {
                println!("{row}");
            }
        }
    }
}

#[cfg(test)]
mod land_spread {
    use super::*;

    /// `SEED=1,7 cargo test -p voxmar_worldgen climate_bands -- --ignored --nocapture`
    /// prints how temperature, moisture and height are spread over the land within 4 km of
    /// the origin, for tuning the climate against the bands of `climate.ron`.
    #[test]
    #[ignore]
    fn climate_bands() {
        let registry = Arc::new(crate::test_registry());
        let seeds: Vec<u64> = std::env::var("SEED").ok().map_or(vec![7], |s| s.split(',').filter_map(|s| s.trim().parse().ok()).collect());
        for seed in seeds {
            let world = Overworld::new(seed, registry.clone());
            let sampler = world.sampler(IVec2::splat(-4096), IVec2::splat(4096));
            let (mut t, mut m, mut h, mut low, mut ranges) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0);
            for i in 0..128 * 128 {
                let (x, z) = (i % 128 * 64 - 4096, i / 128 * 64 - 4096);
                let site = sampler.site(x, z);
                if site.height > site.water {
                    t.push(site.temperature);
                    m.push(site.moisture);
                    h.push((site.height - SEA_LEVEL) as f32);
                    if site.mountain < 0.05 {
                        low.push((site.height - SEA_LEVEL) as f32);
                    } else {
                        ranges += 1;
                    }
                }
            }
            let q = |v: &mut Vec<f32>| {
                v.sort_by(f32::total_cmp);
                [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99].map(|p| format!("{:.2}", v[((v.len() - 1) as f32 * p) as usize])).join(" ")
            };
            println!("seed {seed}: land {:.0}%", 100.0 * t.len() as f32 / (128.0 * 128.0));
            println!("  temperature q01..q99: {}", q(&mut t));
            println!("  moisture    q01..q99: {}", q(&mut m));
            println!("  height      q01..q99: {}", q(&mut h));
            println!("  ranges {:.0}% of the land; height off them q01..q99: {}", 100.0 * ranges as f32 / h.len() as f32, q(&mut low));
        }
    }

    /// `SEED=1,2,3 cargo test -p voxmar_worldgen land_heights -- --ignored --nocapture`
    /// prints how high the land is over 20 km squares around the origin, each seed's and
    /// all together: its share of the world against the ocean dial's, quantiles of its
    /// height above the sea and the share above each of a few heights, for tuning how
    /// mountainous `land.ron` makes the world.
    #[test]
    #[ignore]
    fn land_heights() {
        let registry = Arc::new(crate::test_registry());
        let seeds: Vec<u64> = std::env::var("SEED").ok().map_or(vec![7], |s| s.split(',').filter_map(|s| s.trim().parse().ok()).collect());
        let (n, step) = (160, 128);
        let summary = |h: &mut Vec<f32>, cells: usize| {
            h.sort_by(f32::total_cmp);
            let q = |p: f32| h[((h.len() - 1) as f32 * p) as usize];
            let above = |t: f32| 100.0 * h.iter().filter(|&&v| v > t).count() as f32 / h.len() as f32;
            format!(
                "land {:.0}% | median {:.0}, 90% {:.0}, 99% {:.0}, highest {:.0} | above 40: {:.1}%, 70: {:.1}%, 100: {:.1}%, 130: {:.1}%, 160: {:.1}%",
                100.0 * h.len() as f32 / cells as f32,
                q(0.5),
                q(0.9),
                q(0.99),
                q(1.0),
                above(40.0),
                above(70.0),
                above(100.0),
                above(130.0),
                above(160.0)
            )
        };
        let mut all = Vec::new();
        for &seed in &seeds {
            let world = Overworld::new(seed, registry.clone());
            let mut h: Vec<f32> = (0..n * n)
                .map(|i| world.height(i % n * step - n * step / 2, i / n * step - n * step / 2) - SEA_LEVEL)
                .filter(|&height| height > 0)
                .map(|height| height as f32)
                .collect();
            all.extend_from_slice(&h);
            println!("seed {seed} (dial {:.0}% land): {}", 100.0 * (1.0 - world.dials.ocean_fraction), summary(&mut h, (n * n) as usize));
        }
        println!("all: {}", summary(&mut all, (n * n) as usize * seeds.len()));
    }
}
