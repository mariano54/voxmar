//! Procedural world generation.
//!
//! The overworld generator runs in stages, each a pure function of the world seed and
//! position: world dials (global parameters such as mean temperature, ocean fraction and
//! wind), geology (continents and mountains), climate (temperature and precipitation as
//! continuous fields), water (rivers and lakes), ecology (what grows where), resources
//! (ore veins) and terrain (blocks, caves, trees, plants). There are no biomes: every surface
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
mod trees;
mod water;

use std::sync::Arc;

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::Column;

pub use climate::LAPSE_RATE;
pub use dials::WorldDials;
pub use water::{Body, Cavity, Current};

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
}

/// The default overworld.
pub struct Overworld {
    dials: WorldDials,
    geology: Geology,
    climate: Climate,
    rivers: water::Rivers,
    ecology: Ecology,
    terrain: terrain::Terrain,
    seed: u64,
}

impl Overworld {
    pub const ID: &'static str = "base:overworld";
    /// Bump when generation output changes.
    pub const VERSION: u32 = 4;

    pub fn new(seed: u64, registry: Arc<BlockRegistry>) -> Self {
        let stream = rng::stream_seed(seed, Self::ID, Self::VERSION);
        let dials = WorldDials::sample(stream);
        Self {
            dials,
            geology: Geology::new(stream, &dials),
            climate: Climate::new(stream, &dials),
            rivers: water::Rivers::new(stream, &dials),
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
        self.geo(x, z).height
    }

    fn stages(&self) -> water::Stages<'_> {
        water::Stages { geology: &self.geology, climate: &self.climate }
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
        let geo = self.world.geo(x, z);
        self.site_on(&geo, x, z, self.world.slope(x, z, geo.height))
    }

    /// What the surface looks like from far away; see [`Overworld::surface_sample`].
    pub fn surface(&self, x: i32, z: i32, height: i32, slope: i32) -> SurfaceSample {
        let geo = GeoSample { height, ..self.world.geo(x, z) };
        let site = self.site_on(&geo, x, z, slope);
        let (ground, water) = self.world.terrain.surface_blocks(&site, x, z);
        let canopy = if water.is_some() { None } else { self.world.ecology.canopy(&site, self.world.terrain.ground(ground), x, z) }
            .map(|(tree, cover)| (self.world.terrain.leaves(tree), cover));
        SurfaceSample { height, ground, water, water_level: site.water, canopy, climate: site.tint() }
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
            water: geo.water,
            body: geo.body,
            vegetation: 0.0,
            forest: 0.0,
        };
        if !geo.submerged() {
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
    use voxmar_engine::coords::CHUNK_SIZE;
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

    /// `cargo test --release -p voxmar_worldgen river_sights -- --ignored --nocapture`
    /// finds a highland lake, a waterfall, a confluence and a river mouth near spawn and
    /// prints `--pose` arguments that look at each, for in-game captures.
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
        let mut best: [(f32, i32, i32, i32); 4] = [(0.0, 0, 0, 0); 4];
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
                if !g.submerged() {
                    continue;
                }
                if g.body == Body::Lake && g.water > SEA_LEVEL + 20 {
                    consider(0, (g.water - SEA_LEVEL) as f32);
                }
                let drop = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|&(di, dj)| at(i + di, j + dj)).filter(|o| o.submerged()).map(|o| g.water - o.water).max().unwrap_or(0);
                if drop >= 3 && g.body != Body::Lake {
                    consider(1, drop as f32);
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
        for (name, (score, x, z, y)) in ["highland lake", "waterfall", "confluence", "river mouth"].into_iter().zip(best) {
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

    /// `cargo test --release -p voxmar_worldgen map_preview -- --ignored` writes a shaded
    /// top-down map (north up, 8 blocks per pixel) around the origin to
    /// `target/map_preview.ppm`, for judging the climate layout at a glance. The sea is
    /// deep blue, lakes above sea level turquoise, rivers bright blue and creeks (too
    /// narrow to show at this scale, so found by sampling inside each pixel) light blue;
    /// dry salt flats are white. `X` and `Z` move the centre, `SCALE` the blocks per pixel.
    #[test]
    #[ignore]
    fn map_preview() {
        const SIZE: i32 = 1024;
        let env = |name: &str, default: i64| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
        let scale = env("SCALE", 8) as i32;
        let centre = IVec2::new(env("X", 0) as i32, env("Z", 0) as i32);
        let registry = registry();
        let seed = env("SEED", 0x5eed) as u64;
        let world = Overworld::new(seed, registry.clone());
        let half = SIZE * scale / 2;
        let sampler = world.sampler(centre - half - scale, centre + half + scale);
        let at = |i: i32| i * scale - half;
        let geos: Vec<GeoSample> = (0..SIZE * SIZE).map(|i| world.geo(centre.x + at(i % SIZE), centre.y + at(i / SIZE))).collect();
        let geo = |x: i32, z: i32| geos[(z.clamp(0, SIZE - 1) * SIZE + x.clamp(0, SIZE - 1)) as usize];
        let height = |x: i32, z: i32| geo(x, z).height;
        let color = |block: BlockId| registry.def(block).color.map(|c| c as f32 / 255.0);
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
        let (mut wet, mut creek, mut river, mut lake) = (0, 0, 0, 0);
        for z in 0..SIZE {
            for x in 0..SIZE {
                let h = height(x, z);
                let slope = (height(x + 1, z) - h).abs().max((height(x, z + 1) - h).abs()) / scale;
                let (wx, wz) = (centre.x + at(x), centre.y + at(z));
                let surface = sampler.surface(wx, wz, h, slope);
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
                // Narrow water between the samples: look inside the pixel.
                let mut body = geo(x, z).submerged().then(|| geo(x, z).body);
                if body.is_none() && geo(x, z).river > 0.2 {
                    let step = (scale / 3).max(1);
                    body = (-1..=1)
                        .flat_map(|j| (-1..=1).map(move |i| (i, j)))
                        .map(|(i, j)| world.geo(wx + i * step, wz + j * step))
                        .find(|g| g.submerged())
                        .map(|g| g.body);
                }
                if let Some(body) = body {
                    wet += 1;
                    let depth = ((surface.water_level - h) as f32 / 30.0).clamp(0.0, 1.0);
                    let ice = surface.water == Some(registry.expect("base:ice"));
                    rgb = match body {
                        _ if ice => [0.75, 0.85, 0.95],
                        Body::Creek => [0.45, 0.75, 1.0],
                        Body::River => [0.2, 0.55, 1.0],
                        Body::Lake if surface.water_level > SEA_LEVEL => [0.1, 0.6 - 0.2 * depth, 0.75 - 0.2 * depth],
                        _ => [0.15, 0.35 - 0.15 * depth, 0.7 - 0.25 * depth],
                    };
                    match body {
                        Body::Creek => creek += 1,
                        Body::River => river += 1,
                        Body::Lake if surface.water_level > SEA_LEVEL => lake += 1,
                        _ => {}
                    }
                } else if geo(x, z).body == Body::Salt {
                    rgb = [0.95, 0.93, 0.88];
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
        println!("pixels with creeks {creek}, rivers {river}, lakes above the sea {lake}");
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

    /// Generated water stands still: every water block lies on ground or water, and
    /// beside it is ground, water, or the air over lower water that a waterfall drops into.
    #[test]
    fn generated_water_is_sealed() {
        let registry = registry();
        let water = registry.expect("base:water");
        for (seed, centre) in [(42, ColumnPos::new(-10, -3)), (1, ColumnPos::new(-1, 5)), (1, ColumnPos::new(0, 2))] {
            let world = Overworld::new(seed, registry.clone());
            let columns: Vec<Column> = (0..9).map(|i| world.generate(centre.offset(i % 3 - 1, i / 3 - 1))).collect();
            let origin = centre.origin() - IVec3::new(CHUNK_SIZE, 0, CHUNK_SIZE);
            let block = |p: IVec3| {
                let local = p - origin;
                let (cx, cz) = (local.x.div_euclid(CHUNK_SIZE), local.z.div_euclid(CHUNK_SIZE));
                columns[(cz * 3 + cx) as usize].block(IVec3::new(local.x.rem_euclid(CHUNK_SIZE), p.y, local.z.rem_euclid(CHUNK_SIZE)))
            };
            let mut cells = 0;
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    for y in 1..voxmar_engine::coords::WORLD_HEIGHT - 1 {
                        let p = centre.origin() + IVec3::new(x, y, z);
                        if block(p) != water {
                            continue;
                        }
                        cells += 1;
                        assert!(!block(p - IVec3::Y).is_air(), "seed {seed}: water at {p} over air");
                        for d in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
                            let side = p + d;
                            if !block(side).is_air() {
                                continue;
                            }
                            // Air beside water only over lower water: the face of a fall.
                            let below = (0..y).rev().map(|y| block(IVec3::new(side.x, y, side.z))).find(|b| !b.is_air());
                            assert!(below == Some(water), "seed {seed}: water at {p} leaks into the air at {side}");
                        }
                    }
                }
            }
            assert!(cells > 0, "seed {seed}: no water around {centre:?}");
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
