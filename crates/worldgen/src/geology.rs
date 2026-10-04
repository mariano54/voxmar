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
use rustc_hash::FxHashMap;
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
    /// Eighths of its top water block the water fills, 1..=8, where water covers the
    /// column: its surface between whole blocks.
    pub top: u8,
    /// The kind of water `water` belongs to.
    pub body: Body,
    /// How river water over the column flows.
    pub current: Option<Current>,
    /// A cave a river runs through under the ground.
    pub cavity: Option<Cavity>,
    /// What the terrain shapes made of the column.
    pub form: Form,
    /// What a trail does here.
    pub trail: Option<crate::trails::TrailMark>,
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

    /// The water in the column, open or in a river's tunnel: the height of the ground or
    /// tunnel floor under it, and of its top block.
    pub fn wet(&self) -> Option<(i32, i32)> {
        match self.cavity {
            Some(cavity) => (self.water > cavity.floor).then_some((cavity.floor, self.water)),
            None => self.submerged().then_some((self.height, self.water)),
        }
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
#[derive(Clone, Copy, Debug, Default)]
pub struct Landform {
    /// Height of the ground, in blocks (the top ground block is its whole part).
    pub height: f32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    pub form: Form,
}

/// Continents, ranges, plateaus and valleys, without hills, shapes, rivers or lakes.
#[derive(Clone, Copy, Debug, Default)]
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
    debug_assert!(points.windows(2).all(|w| w[0].0 < w[1].0), "spline points out of order");
    // The first point past x. The points are sorted, so it is the number of points at or
    // before x, which counts without a branch to mispredict. NaN lies past every point.
    let i = if x.is_nan() { points.len() } else { points.iter().filter(|&&(px, _)| px <= x).count() };
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
    (0.02, 4.0),
    (0.12, 7.5),
    (0.4, 16.0),
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

    /// Continents, ranges, plateaus and valleys at (x, z). Generation asks through
    /// `reliefs` and `land`; this is for tests and benchmarks.
    #[cfg(test)]
    pub fn relief(&self, x: f32, z: f32) -> Relief {
        self.relief_lanes(&[(x, z)])[0]
    }

    /// The relief at each position: the same as `relief` at each, sooner.
    pub fn reliefs(&self, points: &[(f32, f32)]) -> Vec<Relief> {
        let mut out = Vec::with_capacity(points.len());
        for four in points.chunks(4) {
            out.extend_from_slice(&self.relief_lanes(four)[..four.len()]);
        }
        out
    }

    /// The relief at one to four positions side by side, one in each lane of the noises
    /// (`crate::noise`), exactly as at each alone. A noise some lanes do not need is still
    /// sampled in all of them when any does, and left unused where it is not needed; lanes
    /// past the positions repeat the last, and their results mean nothing.
    fn relief_lanes(&self, points: &[(f32, f32)]) -> [Relief; 4] {
        let n = points.len();
        debug_assert!((1..=4).contains(&n));
        let x: [f32; 4] = std::array::from_fn(|k| points[k.min(n - 1)].0);
        let z: [f32; 4] = std::array::from_fn(|k| points[k.min(n - 1)].1);
        let sample = |noise: &FastNoiseLite, x: [f32; 4], z: [f32; 4]| crate::noise::get_2d_lanes(noise, x, z, n);
        let any = |need: &dyn Fn(usize) -> bool| (0..n).any(need);
        let (wx, wz) = crate::noise::warp_2d_lanes(&self.warp, x, z, n);
        let sea = SEA_LEVEL as f32;

        // Land where the folded continental noise is large; the sea along its zero lines,
        // deepest in their middle.
        let folded = sample(&self.continental, wx, wz);
        let continental: [f32; 4] = std::array::from_fn(|k| {
            let fold = folded[k].abs() - self.coast_fold;
            COAST + if fold > 0.0 { 1.4 * fold } else { 0.75 * fold / self.coast_fold }
        });
        // Lanes past the positions are left at zero wherever work is per lane.
        let mut above: [f32; 4] = std::array::from_fn(|k| if k < n { spline(&BASE, continental[k]) } else { 0.0 });
        if any(&|k| continental[k] < -0.2) {
            let seabed = sample(&self.seabed, x, z);
            for k in (0..n).filter(|&k| continental[k] < -0.2) {
                let ridge = 1.0 - 6.0 * seabed[k].abs();
                above[k] += 7.0 * ridge.max(0.0) * smoothstep(-0.2, -0.45, continental[k]);
            }
        }
        // Ranges and plateaus rise over a few hundred blocks inland of the coast.
        let inland = continental.map(|c| smoothstep(-0.03, 0.3, c));

        // Ranges along the folded range noise, rising and falling along their length. Out at
        // sea and far from a range's line, nothing rises, so the noises are left unsampled.
        let (mut mountain, mut e) = ([0.0f32; 4], [0.0f32; 4]);
        if any(&|k| inland[k] > 0.0) {
            let ranges = sample(&self.ranges, wx, wz);
            let near: [bool; 4] = std::array::from_fn(|k| inland[k] > 0.0 && ranges[k].abs() < RANGE_HALF_WIDTH * 1.3 * ACROSS[ACROSS.len() - 1].0);
            for k in (0..n).filter(|&k| inland[k] > 0.0) {
                e[k] = ranges[k];
            }
            if any(&|k| near[k]) {
                let strength = sample(&self.range_strength, x, z);
                for k in (0..n).filter(|&k| near[k]) {
                    let strength = smoothstep(self.range_threshold - 0.12, self.range_threshold + 0.12, strength[k]);
                    let half = RANGE_HALF_WIDTH * (0.7 + 0.6 * strength);
                    mountain[k] = spline(&ACROSS, e[k].abs() / half) * strength * inland[k];
                }
            }
        }
        let (mut rise, mut peak) = ([0.0f32; 4], [0.0f32; 4]);
        if any(&|k| mountain[k] > 0.005) {
            let ridges = sample(&self.ridges, wx, wz);
            // Crags on the upper slopes, stretched down the fall line (across the range,
            // along the range noise's gradient) into gullies and arêtes: a blend of crags
            // stretched along x and along z, by how the fall line runs.
            let spine = mountain.map(|m| smoothstep(0.35, 0.8, m) * self.jaggedness);
            let crags = if any(&|k| mountain[k] > 0.005 && spine[k] > 0.0) {
                let step = 8.0;
                let east = sample(&self.ranges, wx.map(|wx| wx + step), wz);
                let south = sample(&self.ranges, wx, wz.map(|wz| wz + step));
                let stretched = |sx: f32, sz: f32| sample(&self.crags, wx.map(|wx| wx * sx), wz.map(|wz| wz * sz)).map(|c| c * 0.5 + 0.5);
                let (along_x_stretched, along_z_stretched) = (stretched(0.3, 1.0), stretched(1.0, 0.3));
                std::array::from_fn(|k| {
                    let (gx, gz) = (east[k] - e[k], south[k] - e[k]);
                    let along_x = gx * gx / (gx * gx + gz * gz).max(1e-12);
                    if spine[k] > 0.0 { spine[k] * (along_x * along_x_stretched[k] + (1.0 - along_x) * along_z_stretched[k]) } else { 0.0 }
                })
            } else {
                [0.0f32; 4]
            };
            for k in (0..n).filter(|&k| mountain[k] > 0.005) {
                let ridge = ridges[k] * 0.5 + 0.5;
                let massif = mountain[k] * (0.35 + 0.65 * ridge);
                peak[k] = mountain[k] * ridge * ridge;
                rise[k] = self.peak_height * (1.15 * massif + 0.16 * crags[k]);
            }
        }

        // Plateaus where their noise is high, off the ranges, in two tiers. Each edge is an
        // escarpment: a talus slope under a cliff band, where rivers fall.
        let hilliness = sample(&self.hilliness, x, z);
        let off_ranges: [f32; 4] = std::array::from_fn(|k| inland[k] * (1.0 - smoothstep(0.1, 0.4, mountain[k])));
        let mut shelf = [0.0f32; 4];
        if any(&|k| off_ranges[k] > 0.0) {
            let plateaus = sample(&self.plateaus, wx, wz);
            for k in (0..n).filter(|&k| off_ranges[k] > 0.0) {
                let p = plateaus[k] + 0.04 * hilliness[k];
                let escarpment = |edge: f32| 0.3 * smoothstep(edge - 0.07, edge, p) + 0.7 * smoothstep(edge - 0.012, edge + 0.004, p);
                shelf[k] = off_ranges[k] * (26.0 * escarpment(0.26) + 24.0 * escarpment(0.46));
            }
        }

        // Broad valleys, and passes where they cross the ranges.
        let valleys = sample(&self.valleys, wx, wz);
        std::array::from_fn(|k| {
            if k >= n {
                return Relief::default();
            }
            let valley = 1.0 - smoothstep(VALLEY_FLOOR, VALLEY_SIDES, valleys[k].abs());
            let land = above[k].max(0.0) + shelf[k] + rise[k];
            // Valleys cut into what rises above the lowest land, so they never sink low
            // plains into the sea.
            let depth = VALLEY_DEPTH + (PASS_DEPTH - VALLEY_DEPTH) * mountain[k];
            let above = above[k].min(0.0) + land - depth * valley * smoothstep(0.0, 0.1, continental[k]) * (land - 5.0).max(0.0);
            let hilly = 1.0 - smoothstep(-0.4, 0.5, hilliness[k]);
            Relief {
                elevation: sea + lift(above),
                continental: continental[k],
                mountain: mountain[k],
                peak: peak[k],
                roughness: hilly.max(mountain[k] * 1.2).min(1.0),
                valley,
            }
        })
    }

    /// The land at (x, z), before rivers and lakes.
    pub fn land(&self, x: i32, z: i32) -> Landform {
        let mut out = [Landform::default()];
        self.land_lanes(&[(x, z)], &mut out);
        out[0]
    }

    /// The land at each position: the same as `land` at each, sooner.
    pub fn lands(&self, points: &[(i32, i32)]) -> Vec<Landform> {
        let mut out = vec![Landform::default(); points.len()];
        for (four, out) in points.chunks(4).zip(out.chunks_mut(4)) {
            self.land_lanes(four, out);
        }
        out
    }

    /// The land at one to four positions, their relief and hills side by side.
    fn land_lanes(&self, points: &[(i32, i32)], out: &mut [Landform]) {
        let n = points.len();
        let at: [(f32, f32); 4] = std::array::from_fn(|k| (points[k.min(n - 1)].0 as f32, points[k.min(n - 1)].1 as f32));
        let reliefs = self.relief_lanes(&at[..n]);
        let noise = crate::noise::get_2d_lanes(&self.hills, at.map(|p| p.0), at.map(|p| p.1), n);
        let hills: [f32; 4] = std::array::from_fn(|k| {
            let inland = smoothstep(-0.06, 0.1, reliefs[k].continental);
            noise[k] * (3.0 + 14.0 * reliefs[k].roughness) * self.hill_scale * inland
        });
        let anywhere = |x: i32, z: i32| self.regional(x, z);
        let grounds: [Ground; 4] = std::array::from_fn(|k| Ground { regional: &anywhere, height: reliefs[k].elevation + hills[k], hills: hills[k], relief: &reliefs[k] });
        let regionals: [Regional; 4] = std::array::from_fn(|k| if k < n { self.regional(points[k].0, points[k].1) } else { Regional::default() });
        let shaped = self.shapes.shape_lanes(points, &grounds[..n], &regionals[..n]);
        let sea = SEA_LEVEL as f32;
        for (k, out) in out.iter_mut().enumerate() {
            let (ground, shaped) = (grounds[k].height, &shaped[k]);
            let base = sea + ease(ground + shaped.terrain - sea);
            let height = sea + ease(ground + shaped.terrain + shaped.features - sea);
            *out = Landform { height, mountain: reliefs[k].mountain, form: Form { shape: shaped.shape, strength: shaped.strength, base } };
        }
    }

    /// The regional climate at a lattice point (in lattice cells).
    pub fn regional_point(&self, point: IVec2) -> Regional {
        let key = point.div_euclid(IVec2::splat(TILE));
        let local = point - key * TILE;
        self.with_tile(key, |tile| tile[(local.y * (TILE + 1) + local.x) as usize])
    }

    /// The regional climate at (x, z), bilinear between the lattice points around it: the
    /// same fields every sampler of the climate sees there.
    pub fn regional(&self, x: i32, z: i32) -> Regional {
        let cell = IVec2::new(x, z).div_euclid(IVec2::splat(climate::CELL));
        let key = cell.div_euclid(IVec2::splat(TILE));
        let local = cell - key * TILE;
        let f = |v: i32| v.rem_euclid(climate::CELL) as f32 / climate::CELL as f32;
        self.with_tile(key, |tile| {
            let at = |i: i32, j: i32| tile[((local.y + j) * (TILE + 1) + local.x + i) as usize];
            at(0, 0).lerp(at(1, 0), f(x)).lerp(at(0, 1).lerp(at(1, 1), f(x)), f(z))
        })
    }

    /// Calls `f` with a tile of the regional climate lattice. The tiles this thread used
    /// last are kept at hand, which spares the shared cache's lock and the shared count of
    /// the tile's owners on most calls.
    fn with_tile<R>(&self, key: IVec2, f: impl FnOnce(&[Regional]) -> R) -> R {
        thread_local! {
            static RECENT: RefCell<Vec<(u64, IVec2, Arc<[Regional]>)>> = const { RefCell::new(Vec::new()) };
        }
        RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            match recent.iter().position(|(id, k, _)| *id == self.id && *k == key) {
                Some(0) => {}
                Some(i) => {
                    let entry = recent.remove(i);
                    recent.insert(0, entry);
                }
                None => {
                    let tile = self.lattice.get(key, || {
                        let side = TILE + 1;
                        let points: Vec<(f32, f32)> = (0..side * side)
                            .map(|i| {
                                let p = (key * TILE + IVec2::new(i % side, i / side)) * climate::CELL;
                                (p.x as f32, p.y as f32)
                            })
                            .collect();
                        self.climate.regionals(self, &points).into()
                    });
                    recent.insert(0, (self.id, key, tile));
                    recent.truncate(6);
                }
            }
            f(&recent[0].2)
        })
    }
}

/// Geology already sampled over a square of positions, which falls back to sampling
/// anywhere else (and keeps those samples, since decorations reaching out of the column
/// ask for the same few positions over and over).
pub struct GeoGrid<'a> {
    world: &'a Overworld,
    min: (i32, i32),
    size: i32,
    samples: Vec<GeoSample>,
    beyond: RefCell<FxHashMap<(i32, i32), GeoSample>>,
}

impl<'a> GeoGrid<'a> {
    /// Samples the `size`×`size` positions from `min`.
    pub fn new(world: &'a Overworld, min: (i32, i32), size: i32) -> Self {
        let samples = world.geo_area(min.0, min.1, size);
        Self { world, min, size, samples, beyond: RefCell::default() }
    }

    pub fn samples(&self) -> &[GeoSample] {
        &self.samples
    }

    pub fn get(&self, x: i32, z: i32) -> GeoSample {
        let (i, j) = (x - self.min.0, z - self.min.1);
        if (0..self.size).contains(&i) && (0..self.size).contains(&j) {
            self.samples[(j * self.size + i) as usize]
        } else {
            *self.beyond.borrow_mut().entry((x, z)).or_insert_with(|| self.world.geo(x, z))
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

    /// Land worked out four positions at a time is exactly the land at each alone, over
    /// sea, coast, plains and ranges, in any grouping.
    #[test]
    fn lands_match_land_alone() {
        let registry = std::sync::Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap());
        for seed in [7, 42] {
            let world = crate::Overworld::new(seed, registry.clone());
            let geology = &world.geology;
            let mut rng = crate::rng::Rng::new(seed, 1, 2, 3);
            // Rows of a grid, so that neighbouring lanes are alike, and scattered positions,
            // so that they are not; seven at a time leaves groups of four and of three.
            let mut points: Vec<(i32, i32)> = (0..4000).map(|i| (i % 80 * 37 - 1500, i / 80 * 41 - 1000)).collect();
            points.extend((0..4000).map(|_| (rng.range(-20_000, 20_000), rng.range(-20_000, 20_000))));
            for group in points.chunks(7) {
                let lands = geology.lands(group);
                let floats: Vec<(f32, f32)> = group.iter().map(|&(x, z)| (x as f32 + 0.5, z as f32 - 0.25)).collect();
                let reliefs = geology.reliefs(&floats);
                for (k, &(x, z)) in group.iter().enumerate() {
                    assert_eq!(format!("{:?}", lands[k]), format!("{:?}", geology.land(x, z)), "land at ({x}, {z})");
                    assert_eq!(format!("{:?}", reliefs[k]), format!("{:?}", geology.relief(floats[k].0, floats[k].1)), "relief at {:?}", floats[k]);
                }
            }
        }
    }

    /// The regional climate worked out four positions at a time is exactly the climate at
    /// each alone.
    #[test]
    fn regionals_match_regional_alone() {
        let registry = std::sync::Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap());
        let world = crate::Overworld::new(3, registry);
        let mut rng = crate::rng::Rng::new(3, 4, 5, 6);
        let points: Vec<(f32, f32)> = (0..2000).map(|i| if i % 2 == 0 { ((i % 40 * 64) as f32, (i / 40 * 64) as f32) } else { (rng.range(-30_000, 30_000) as f32, rng.range(-30_000, 30_000) as f32) }).collect();
        for group in points.chunks(7) {
            let batch = world.geology.climate.regionals(&world.geology, group);
            for (k, &(x, z)) in group.iter().enumerate() {
                assert_eq!(format!("{:?}", batch[k]), format!("{:?}", world.geology.climate.regional(&world.geology, x, z)), "at ({x}, {z})");
            }
        }
    }

    /// `cargo test --release -p voxmar_worldgen land_timing -- --ignored --nocapture` prints
    /// the time the relief, the land with its shapes, and the regional climate take per
    /// position, on one thread.
    #[test]
    #[ignore]
    fn land_timing() {
        let registry = voxmar_engine::block::BlockRegistry::from_ron("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
        let world = crate::Overworld::new(7, std::sync::Arc::new(registry));
        let geology = &world.geology;
        let points: Vec<(i32, i32)> = (0..200_000).map(|i| (i % 400 * 3 - 600, i / 400 * 3 - 750)).collect();
        // Warm the climate lattice first.
        for &(x, z) in points.iter().step_by(97) {
            std::hint::black_box(geology.land(x, z));
        }
        let time = |name: &str, f: &dyn Fn(i32, i32) -> f32| {
            let start = std::time::Instant::now();
            let mut sum = 0.0;
            for &(x, z) in &points {
                sum += f(x, z);
            }
            std::hint::black_box(sum);
            println!("{name:10} {:.0} ns", start.elapsed().as_nanos() as f64 / points.len() as f64);
        };
        time("relief", &|x, z| geology.relief(x as f32, z as f32).elevation);
        time("regional", &|x, z| geology.regional(x, z).rainfall);
        time("shapes", &|x, z| {
            let relief = geology.relief(x as f32, z as f32);
            let anywhere = |x: i32, z: i32| geology.regional(x, z);
            let ground = Ground { regional: &anywhere, height: relief.elevation, hills: 0.0, relief: &relief };
            geology.shapes.shape(x, z, &ground, &geology.regional(x, z)).terrain
        });
        time("land", &|x, z| geology.land(x, z).height);
        let start = std::time::Instant::now();
        for &(x, z) in points.iter().step_by(10) {
            std::hint::black_box(geology.climate.regional(geology, x as f32, z as f32));
        }
        println!("climate    {:.0} ns per regional point", start.elapsed().as_nanos() as f64 / (points.len() / 10) as f64);
        let start = std::time::Instant::now();
        let mut pieces = 0;
        for i in 0..400 {
            let corner = voxmar_engine::glam::IVec2::new(i % 20 * 32 - 320, i / 20 * 32 - 320);
            pieces += world.pieces(corner).len();
        }
        println!("pieces     {:.1} µs per column ({pieces} pieces)", start.elapsed().as_secs_f64() * 1e6 / 400.0);
    }

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
