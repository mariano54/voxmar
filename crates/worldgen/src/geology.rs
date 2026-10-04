//! Stage 1: the land. Continents, mountain ranges, plateaus and broad valleys make the
//! relief the climate sees (stage 2, `climate`); hills and the terrain shapes that rise with
//! the climate (`shapes`) stand on it, and rivers and lakes cut into the result (`water`).
//!
//! The relief follows Tectonic: each large feature lies along the zero line of a noise
//! folded with `abs()`, so it forms a line rather than a blob. Coasts follow the folded
//! continental noise, so the sea runs in a connected web of channels and straits around
//! separate landmasses. Mountain ranges follow a second folded noise, so they run in long
//! chains, and a third noise raises them in some stretches and lets them die away in
//! others; on their spines ridged noise makes peaks and saddles, and crags stretched down
//! the fall line make gullies and arêtes where the land is young. Broad valleys follow a
//! fourth folded noise, a network that cuts passes through the ranges and that trunk rivers
//! tend to find. Plateaus rise where a fifth noise is high.
//!
//! Heights above the sea are then stretched by their square, so lowlands stay low while
//! ranges rise high (peaks of the most active worlds reach about 230 blocks over the sea),
//! and eased under a ceiling well below the top of the world, so nothing is cut flat.
//!
//! Shapes read the regional climate, which reads the relief, so the climate never sees
//! them: a karst tower does not cool its own summit. The regional climate is computed on a
//! lattice every `climate::CELL` blocks and cached here, shared by the shapes and by every
//! sampler of the climate, so both see exactly the same fields.

use std::cell::RefCell;
use std::sync::Arc;

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType, NoiseType};
use voxmar_engine::glam::IVec2;

use crate::climate::{self, Climate, Regional};
use crate::content::WorldContent;
use crate::dials::WorldDials;
use crate::rocks::Rocks;
use crate::shapes::{Ground, ShapeId, Shapes};
use crate::water::cache::Cache;
use crate::water::{Body, Cavity, Current};
use crate::{Overworld, SEA_LEVEL};

/// The ground at one position, with its rivers and lakes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoSample {
    /// Height of the top ground block, rivers and lakes carved in.
    pub height: i32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    /// 1 in a river channel or lake, falling to 0 at the edge of its valley or shore.
    pub river: f32,
    /// Height of the top water block over the column where it is above `height`;
    /// otherwise of the river or lake nearest the column, or sea level away from them.
    pub water: i32,
    /// The kind of water `water` belongs to.
    pub body: Body,
    /// How river water over the column flows.
    pub current: Option<Current>,
    /// A cave a river runs through under the ground.
    pub cavity: Option<Cavity>,
    /// What the terrain shapes made of the column.
    pub form: Form,
}

impl GeoSample {
    /// Whether water covers the ground.
    pub fn submerged(&self) -> bool {
        self.water > self.height
    }

    /// Height of the ground the climate cools with: without the towers and spires that
    /// stand on it, and with what rivers cut away.
    pub fn climate_height(&self) -> i32 {
        self.height.min(self.form.base.floor() as i32)
    }
}

/// What the terrain shapes made of a column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Form {
    /// The shape that changes the column most, if any does.
    pub shape: Option<ShapeId>,
    /// How much of that shape's full form rises here, 0 ..= 1.
    pub strength: f32,
    /// Height of the land without the features that stand on it (towers, spires): what the
    /// climate cools with. Volcanoes, mesas, dunes and canyons are part of it.
    pub base: f32,
}

/// The land before rivers and lakes cut into it.
#[derive(Clone, Copy, Debug)]
pub struct Landform {
    /// Height of the ground, in blocks (the top ground block is its whole part).
    pub height: f32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    pub form: Form,
}

/// Continents, ranges, plateaus and valleys, without hills, shapes, rivers or lakes.
#[derive(Clone, Copy, Debug)]
pub struct Relief {
    /// Large-scale surface height.
    pub elevation: f32,
    /// Continental noise, folded: below about −0.04 is sea, rising inland.
    pub continental: f32,
    /// 0 off the ranges ..= 1 on their spines.
    pub mountain: f32,
    /// How much of the range's full height rises here, 0 ..= 1: high along ridgelines.
    pub peak: f32,
    /// 0 on smooth lowland ..= 1 where hills are steep and mountains can rise.
    pub roughness: f32,
    /// 1 on the floor of a broad valley, falling to 0 beyond its sides.
    pub valley: f32,
}

/// Where the continental noise reaches the coast.
pub const COAST: f32 = -0.04;
/// The continental noise is folded at a smaller frequency than the dial's continent size,
/// since folding splits each landmass of the raw noise in two.
const FOLDED_CONTINENTS: f32 = 3.0;
/// Spread of the continental noise around zero, measured: it sets how much of the folded
/// noise lies under the sea for a given ocean fraction.
const CONTINENTAL_SPREAD: f32 = 0.40;
/// Half the width of a range, in units of the range noise, at full strength: about 200
/// blocks from the spine to where the foothills begin.
const RANGE_HALF_WIDTH: f32 = 0.14;
/// Half the width of a broad valley's floor and of the whole valley, in units of its noise:
/// about 25 and 200 blocks.
const VALLEY_FLOOR: f32 = 0.03;
const VALLEY_SIDES: f32 = 0.24;
/// How much of the height above the sea a broad valley takes away, off the ranges and on
/// them.
const VALLEY_DEPTH: f32 = 0.5;
const PASS_DEPTH: f32 = 0.25;
/// The stretch of heights above the sea by their square, per 128 blocks squared.
const BOOST: f32 = 0.35;
/// Heights above this many blocks over the sea are eased towards `KNEE + ROOM` instead of
/// rising further, so peaks round off below the top of the world rather than being cut.
const KNEE: f32 = 190.0;
const ROOM: f32 = 48.0;
/// Regional climate tiles, in lattice cells a side.
const TILE: i32 = 8;

pub struct Geology {
    warp: FastNoiseLite,
    continental: FastNoiseLite,
    /// Folded, the lines mountain ranges follow.
    ranges: FastNoiseLite,
    /// Where along their lines ranges rise.
    range_strength: FastNoiseLite,
    ridges: FastNoiseLite,
    /// Gullies and arêtes on the spines, stretched down the fall line.
    crags: FastNoiseLite,
    plateaus: FastNoiseLite,
    /// Folded, the network of broad valleys.
    valleys: FastNoiseLite,
    /// How hilly the land is.
    hilliness: FastNoiseLite,
    hills: FastNoiseLite,
    /// Folded, ridges on the floor of the deep sea.
    seabed: FastNoiseLite,
    /// Where the folded continental noise meets the sea, for the dialled ocean fraction.
    coast_fold: f32,
    peak_height: f32,
    hill_scale: f32,
    /// Range noise values below which a range rises, from the tectonic dial.
    range_threshold: f32,
    /// How craggy the spines are, from the land's age.
    jaggedness: f32,
    climate: Climate,
    shapes: Shapes,
    /// The regional climate at the lattice points of each tile, plus the next row and
    /// column, so any position interpolates within one tile.
    lattice: Cache<IVec2, Arc<[Regional]>>,
    /// Tells this world's tiles apart in the per-thread memo.
    id: u64,
}

pub fn noise(seed: u64, salt: u32, frequency: f32, fractal: FractalType, octaves: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed((crate::rng::mix(seed ^ salt as u64) >> 32) as i32);
    n.set_noise_type(Some(NoiseType::OpenSimplex2));
    n.set_frequency(Some(frequency));
    n.set_fractal_type(Some(fractal));
    n.set_fractal_octaves(Some(octaves));
    n
}

fn warp_noise(seed: u64, salt: u32, frequency: f32, octaves: i32, amplitude: f32) -> FastNoiseLite {
    let mut warp = noise(seed, salt, frequency, FractalType::DomainWarpProgressive, octaves);
    warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
    warp.set_domain_warp_amp(Some(amplitude));
    warp
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Like `smoothstep`, with a flat first and second derivative at both ends (quintic), so
/// shapes faded by it meet the land around them without a crease.
pub fn smootherstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

/// Piecewise-linear curve through `(x, y)` points sorted by x.
pub fn spline(points: &[(f32, f32)], x: f32) -> f32 {
    let i = points.iter().position(|&(px, _)| px > x).unwrap_or(points.len());
    match i {
        0 => points[0].1,
        i if i == points.len() => points[i - 1].1,
        i => {
            let (x0, y0) = points[i - 1];
            let (x1, y1) = points[i];
            y0 + (y1 - y0) * (x - x0) / (x1 - x0)
        }
    }
}

/// Height above the sea, stretched by its square and eased under the ceiling.
fn lift(above: f32) -> f32 {
    if above <= 0.0 {
        return above;
    }
    ease(above * (1.0 + BOOST * (above / 128.0).powi(2)))
}

/// Eases heights above the sea that pass `KNEE` towards `KNEE + ROOM`.
fn ease(above: f32) -> f32 {
    if above > KNEE { KNEE + ROOM * ((above - KNEE) / ROOM).tanh() } else { above }
}

/// The sea floor and lowland by continental noise: shelves near the coast, deep basins
/// along the middle of the seas.
const BASE: [(f32, f32); 11] = [
    (-1.0, -100.0),
    (-0.75, -80.0),
    (-0.45, -52.0),
    (-0.25, -28.0),
    (-0.12, -10.0),
    (-0.07, -4.0),
    (COAST, 0.5),
    (0.02, 3.0),
    (0.12, 6.0),
    (0.4, 15.0),
    (1.0, 30.0),
];

/// A range's height across it, by distance from its spine in half-widths: spine, slopes,
/// foothills.
const ACROSS: [(f32, f32); 6] = [(0.0, 1.0), (0.35, 0.9), (0.7, 0.62), (1.0, 0.34), (1.4, 0.1), (1.9, 0.0)];

impl Geology {
    pub fn new(seed: u64, dials: &WorldDials, content: &WorldContent, rocks: &Rocks) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        // The continental noise is roughly normal around zero, so its folded value is
        // half-normal: the sea takes `ocean_fraction` of the land where it is below this.
        let coast_fold = CONTINENTAL_SPREAD * probit(0.5 + 0.5 * dials.ocean_fraction);
        let tectonic = dials.tectonic_activity;
        Self {
            warp: warp_noise(seed, 1, 0.0009, 3, 120.0),
            continental: noise(seed, 2, 1.0 / (dials.continent_size * FOLDED_CONTINENTS), FractalType::FBm, 5),
            ranges: noise(seed, 60, 1.0 / 7000.0, FractalType::FBm, 3),
            range_strength: noise(seed, 61, 1.0 / 5000.0, FractalType::FBm, 2),
            ridges: noise(seed, 4, 1.0 / 520.0, FractalType::Ridged, 5),
            crags: noise(seed, 62, 1.0 / 70.0, FractalType::Ridged, 3),
            plateaus: noise(seed, 63, 1.0 / 1700.0, FractalType::FBm, 2),
            valleys: noise(seed, 64, 1.0 / 2600.0, FractalType::FBm, 2),
            hilliness: noise(seed, 3, 0.0011, FractalType::FBm, 3),
            hills: noise(seed, 5, 0.008, FractalType::FBm, 4),
            seabed: noise(seed, 65, 1.0 / 700.0, FractalType::FBm, 2),
            coast_fold,
            peak_height: dials.mountain_height,
            hill_scale: 1.25 - 0.5 * dials.erosion_age,
            range_threshold: -0.25 + 0.4 * tectonic,
            jaggedness: 1.0 - 0.7 * dials.erosion_age,
            climate: Climate::new(seed, dials),
            shapes: Shapes::new(seed, dials, content, rocks),
            lattice: Cache::new(16 << 20),
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// The climate over this land.
    pub fn climate(&self) -> &Climate {
        &self.climate
    }

    /// The terrain shapes of this land.
    pub fn shapes(&self) -> &Shapes {
        &self.shapes
    }

    /// Continents, ranges, plateaus and valleys at (x, z).
    pub fn relief(&self, x: f32, z: f32) -> Relief {
        let (wx, wz) = self.warp.domain_warp_2d(x, z);
        let sea = SEA_LEVEL as f32;

        // Land where the folded continental noise is large; the sea along its zero lines,
        // deepest in their middle.
        let fold = self.continental.get_noise_2d(wx, wz).abs() - self.coast_fold;
        let continental = COAST + if fold > 0.0 { 1.4 * fold } else { 0.75 * fold / self.coast_fold };
        let mut above = spline(&BASE, continental);
        if continental < -0.2 {
            let ridge = 1.0 - 6.0 * self.seabed.get_noise_2d(x, z).abs();
            above += 7.0 * ridge.max(0.0) * smoothstep(-0.2, -0.45, continental);
        }
        let inland = smoothstep(-0.02, 0.14, continental);

        // Ranges along the folded range noise, rising and falling along their length.
        let strength = smoothstep(self.range_threshold - 0.12, self.range_threshold + 0.12, self.range_strength.get_noise_2d(x, z));
        let half = RANGE_HALF_WIDTH * (0.7 + 0.6 * strength);
        let e = self.ranges.get_noise_2d(wx, wz);
        let across = spline(&ACROSS, e.abs() / half);
        let mountain = across * strength * inland;
        let mut rise = 0.0;
        let mut peak = 0.0;
        if mountain > 0.0 {
            let ridge = self.ridges.get_noise_2d(wx, wz) * 0.5 + 0.5;
            let massif = mountain * (0.35 + 0.65 * ridge);
            peak = mountain * ridge * ridge;
            // Crags on the upper slopes, stretched down the fall line (across the range,
            // along the range noise's gradient) into gullies and arêtes: a blend of crags
            // stretched along x and along z, by how the fall line runs.
            let spine = smoothstep(0.35, 0.8, mountain) * self.jaggedness;
            let crags = if spine > 0.0 {
                let step = 8.0;
                let gx = self.ranges.get_noise_2d(wx + step, wz) - e;
                let gz = self.ranges.get_noise_2d(wx, wz + step) - e;
                let along_x = gx * gx / (gx * gx + gz * gz).max(1e-12);
                let stretched = |sx: f32, sz: f32| self.crags.get_noise_2d(wx * sx, wz * sz) * 0.5 + 0.5;
                spine * (along_x * stretched(0.3, 1.0) + (1.0 - along_x) * stretched(1.0, 0.3))
            } else {
                0.0
            };
            rise = self.peak_height * (1.15 * massif + 0.16 * crags);
        }

        // Plateaus where their noise is high, off the ranges, in two tiers. Each edge is an
        // escarpment: a talus slope under a cliff band, where rivers fall.
        let hilliness = self.hilliness.get_noise_2d(x, z);
        let p = self.plateaus.get_noise_2d(wx, wz) + 0.04 * hilliness;
        let escarpment = |edge: f32| 0.3 * smoothstep(edge - 0.07, edge, p) + 0.7 * smoothstep(edge - 0.012, edge + 0.004, p);
        let off_ranges = inland * (1.0 - smoothstep(0.1, 0.4, mountain));
        let shelf = off_ranges * (26.0 * escarpment(0.26) + 24.0 * escarpment(0.46));

        // Broad valleys, and passes where they cross the ranges.
        let valley = 1.0 - smoothstep(VALLEY_FLOOR, VALLEY_SIDES, self.valleys.get_noise_2d(wx, wz).abs());
        let land = above.max(0.0) + shelf + rise;
        let depth = VALLEY_DEPTH + (PASS_DEPTH - VALLEY_DEPTH) * mountain;
        let above = above.min(0.0) + land * (1.0 - depth * valley * smoothstep(0.0, 0.1, continental));

        let hilly = 1.0 - smoothstep(-0.4, 0.5, hilliness);
        Relief {
            elevation: sea + lift(above),
            continental,
            mountain,
            peak,
            roughness: hilly.max(mountain * 1.2).min(1.0),
            valley,
        }
    }

    /// The land at (x, z), before rivers and lakes.
    pub fn land(&self, x: i32, z: i32) -> Landform {
        let relief = self.relief(x as f32, z as f32);
        let hills = self.hills.get_noise_2d(x as f32, z as f32);
        let inland = smoothstep(-0.06, 0.1, relief.continental);
        let hills = hills * (3.0 + 14.0 * relief.roughness) * self.hill_scale * inland;
        let ground = relief.elevation + hills;
        let regional = self.regional(x, z);
        let anywhere = |x: i32, z: i32| self.regional(x, z);
        let shaped = self.shapes.shape(x, z, &Ground { regional: &anywhere, height: ground, hills, relief: &relief }, &regional);
        let sea = SEA_LEVEL as f32;
        let base = sea + ease(ground + shaped.terrain - sea);
        let height = sea + ease(ground + shaped.terrain + shaped.features - sea);
        Landform { height, mountain: relief.mountain, form: Form { shape: shaped.shape, strength: shaped.strength, base } }
    }

    /// The regional climate at a lattice point (in lattice cells).
    pub fn regional_point(&self, point: IVec2) -> Regional {
        let key = point.div_euclid(IVec2::splat(TILE));
        let local = point - key * TILE;
        self.tile(key)[(local.y * (TILE + 1) + local.x) as usize]
    }

    /// The regional climate at (x, z), bilinear between the lattice points around it: the
    /// same fields every sampler of the climate sees there.
    pub fn regional(&self, x: i32, z: i32) -> Regional {
        let cell = IVec2::new(x, z).div_euclid(IVec2::splat(climate::CELL));
        let key = cell.div_euclid(IVec2::splat(TILE));
        let local = cell - key * TILE;
        let tile = self.tile(key);
        let at = |i: i32, j: i32| tile[((local.y + j) * (TILE + 1) + local.x + i) as usize];
        let f = |v: i32| v.rem_euclid(climate::CELL) as f32 / climate::CELL as f32;
        at(0, 0).lerp(at(1, 0), f(x)).lerp(at(0, 1).lerp(at(1, 1), f(x)), f(z))
    }

    fn tile(&self, key: IVec2) -> Arc<[Regional]> {
        thread_local! {
            /// The tiles this thread used last, which spares the shared cache's lock.
            static RECENT: RefCell<Vec<(u64, IVec2, Arc<[Regional]>)>> = const { RefCell::new(Vec::new()) };
        }
        let found = RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            let i = recent.iter().position(|(id, k, _)| *id == self.id && *k == key)?;
            let entry = recent.remove(i);
            let tile = entry.2.clone();
            recent.insert(0, entry);
            Some(tile)
        });
        if let Some(tile) = found {
            return tile;
        }
        let tile = self.lattice.get(key, || {
            let side = TILE + 1;
            (0..side * side)
                .map(|i| {
                    let p = (key * TILE + IVec2::new(i % side, i / side)) * climate::CELL;
                    self.climate.regional(self, p.x as f32, p.y as f32)
                })
                .collect()
        });
        RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            recent.insert(0, (self.id, key, tile.clone()));
            recent.truncate(6);
        });
        tile
    }
}

/// Geology already sampled over a square of positions, which falls back to sampling
/// anywhere else.
pub struct GeoGrid<'a> {
    world: &'a Overworld,
    min: (i32, i32),
    size: i32,
    samples: Vec<GeoSample>,
}

impl<'a> GeoGrid<'a> {
    /// Samples the `size`×`size` positions from `min`.
    pub fn new(world: &'a Overworld, min: (i32, i32), size: i32) -> Self {
        let samples = world.geo_area(min.0, min.1, size);
        Self { world, min, size, samples }
    }

    pub fn samples(&self) -> &[GeoSample] {
        &self.samples
    }

    pub fn get(&self, x: i32, z: i32) -> GeoSample {
        let (i, j) = (x - self.min.0, z - self.min.1);
        if (0..self.size).contains(&i) && (0..self.size).contains(&j) {
            self.samples[(j * self.size + i) as usize]
        } else {
            self.world.geo(x, z)
        }
    }

    /// Steepest height step from (x, z) to its four neighbours, in blocks.
    pub fn slope(&self, x: i32, z: i32) -> i32 {
        let height = self.get(x, z).height;
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().map(|(dx, dz)| (self.get(x + dx, z + dz).height - height).abs()).max().unwrap_or(0)
    }
}

/// Inverse of the standard normal distribution function, by a logistic fit that is
/// accurate to a few percent over 0.05..0.95.
fn probit(p: f32) -> f32 {
    let p = p.clamp(0.01, 0.99);
    (p / (1.0 - p)).ln() / 1.702
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p voxmar_worldgen noise_spreads -- --ignored --nocapture` prints how the
    /// relief noises are spread, for setting the constants above.
    #[test]
    #[ignore]
    fn noise_spreads() {
        let seed = 7;
        let fields: [(&str, FastNoiseLite); 6] = [
            ("continental", noise(seed, 2, 1.0 / 4000.0, FractalType::FBm, 5)),
            ("ranges", noise(seed, 60, 1.0 / 3400.0, FractalType::FBm, 3)),
            ("range strength", noise(seed, 61, 1.0 / 2400.0, FractalType::FBm, 2)),
            ("ridges", noise(seed, 4, 1.0 / 520.0, FractalType::Ridged, 5)),
            ("plateaus", noise(seed, 63, 1.0 / 1700.0, FractalType::FBm, 2)),
            ("valleys", noise(seed, 64, 1.0 / 1300.0, FractalType::FBm, 2)),
        ];
        for (name, n) in fields {
            let mut values: Vec<f32> = (0..200 * 200).map(|i| n.get_noise_2d((i % 200) as f32 * 97.0, (i / 200) as f32 * 97.0)).collect();
            values.sort_by(f32::total_cmp);
            let mean = values.iter().sum::<f32>() / values.len() as f32;
            let sd = (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt();
            let q = |p: f32| values[((values.len() - 1) as f32 * p) as usize];
            let folded = |w: f32| values.iter().filter(|v| v.abs() < w).count() as f32 / values.len() as f32;
            // Slope near the zero line, in noise units per block.
            let mut slopes: Vec<f32> = (0..20000)
                .filter_map(|i| {
                    let (x, z) = ((i % 141) as f32 * 131.0, (i / 141) as f32 * 131.0);
                    let v = n.get_noise_2d(x, z);
                    (v.abs() < 0.03).then(|| (n.get_noise_2d(x + 1.0, z) - v).hypot(n.get_noise_2d(x, z + 1.0) - v))
                })
                .collect();
            slopes.sort_by(f32::total_cmp);
            let slope = slopes.get(slopes.len() / 2).copied().unwrap_or(0.0);
            println!(
                "{name:15} mean {mean:6.3} sd {sd:5.3}  q01 {:6.3} q50 {:6.3} q99 {:6.3}  |v|<0.1 {:4.2} |v|<0.2 {:4.2}  slope at 0 {slope:.5}/block",
                q(0.01),
                q(0.5),
                q(0.99),
                folded(0.1),
                folded(0.2)
            );
        }
    }
}
