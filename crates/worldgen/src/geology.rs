//! Stage 1: continents and mountain ranges (the relief the climate sees), and hills on
//! top, reduced to a surface height. Rivers and lakes cut into it later (`water`).

use std::cell::RefCell;

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType, NoiseType};
use rustc_hash::FxHashMap;

use crate::dials::WorldDials;
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
    /// What a trail does here.
    pub trail: Option<crate::trails::TrailMark>,
}

impl GeoSample {
    /// Whether water covers the ground.
    pub fn submerged(&self) -> bool {
        self.water > self.height
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

/// The land before rivers and lakes cut into it.
#[derive(Clone, Copy, Debug)]
pub struct Landform {
    /// Height of the ground, in blocks (the top ground block is its whole part).
    pub height: f32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
}

/// Continents and mountain ranges, without hills, rivers or lakes.
#[derive(Clone, Copy, Debug)]
pub struct Relief {
    /// Large-scale surface height.
    pub elevation: f32,
    /// Continental noise: below about −0.08 is sea, rising inland.
    pub continental: f32,
    pub mountain: f32,
    /// How much of the range's full height rises here, 0 ..= 1: high along ridgelines.
    pub peak: f32,
    /// 0 on smooth lowland ..= 1 where hills are steep and mountains can rise.
    pub roughness: f32,
}

pub struct Geology {
    warp: FastNoiseLite,
    continental: FastNoiseLite,
    erosion: FastNoiseLite,
    ridges: FastNoiseLite,
    hills: FastNoiseLite,
    /// Raises or lowers the continents to give the dialled ocean fraction.
    ocean_bias: f32,
    peak_height: f32,
    hill_scale: f32,
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

/// Heights above this are compressed so the highest peaks approach their ceiling (`PEAK_ROOM`
/// higher) smoothly instead of being cut flat.
const PEAK_KNEE: f32 = (SEA_LEVEL + 132) as f32;
const PEAK_ROOM: f32 = 36.0;

impl Geology {
    pub fn new(seed: u64, dials: &WorldDials) -> Self {
        // The continental noise is roughly normal with this spread around zero, and land
        // (with its hills) ends near −0.02; shift it so about `ocean_fraction` of it is
        // under water. Both values are measured.
        const SPREAD: f32 = 0.415;
        let ocean_bias = -0.02 - SPREAD * probit(dials.ocean_fraction);
        Self {
            warp: warp_noise(seed, 1, 0.0012, 3, 90.0),
            continental: noise(seed, 2, 1.0 / dials.continent_size, FractalType::FBm, 5),
            erosion: noise(seed, 3, 0.0011, FractalType::FBm, 3),
            ridges: noise(seed, 4, 0.0022, FractalType::Ridged, 5),
            hills: noise(seed, 5, 0.008, FractalType::FBm, 4),
            ocean_bias,
            peak_height: dials.mountain_height,
            hill_scale: 1.25 - 0.5 * dials.erosion_age,
        }
    }

    /// Continents and mountain ranges at (x, z).
    pub fn relief(&self, x: f32, z: f32) -> Relief {
        let (wx, wz) = self.warp.domain_warp_2d(x, z);
        let continental = self.continental.get_noise_2d(wx, wz) + self.ocean_bias;
        let erosion = self.erosion.get_noise_2d(wx, wz);
        let ridge = (self.ridges.get_noise_2d(wx, wz) + 1.0) * 0.5;

        let sea = SEA_LEVEL as f32;
        let base = spline(
            &[
                (-0.6, sea - 34.0),
                (-0.3, sea - 14.0),
                (-0.12, sea - 3.0),
                (-0.04, sea + 1.0),
                (0.1, sea + 5.0),
                (0.4, sea + 16.0),
                (1.0, sea + 30.0),
            ],
            continental,
        );
        let roughness = 1.0 - smoothstep(-0.4, 0.5, erosion);
        let mountain = smoothstep(0.02, 0.35, continental) * roughness;
        let peak = mountain * ridge.powf(2.2);
        let elevation = base + peak * self.peak_height;
        Relief { elevation, continental, mountain, peak, roughness }
    }

    /// The land at (x, z), before rivers and lakes.
    pub fn land(&self, x: i32, z: i32) -> Landform {
        let relief = self.relief(x as f32, z as f32);
        let hills = self.hills.get_noise_2d(x as f32, z as f32);
        let inland = smoothstep(-0.06, 0.1, relief.continental);
        let land = relief.elevation + hills * (3.0 + 14.0 * relief.roughness) * self.hill_scale * inland;
        let height = if land > PEAK_KNEE { PEAK_KNEE + PEAK_ROOM * ((land - PEAK_KNEE) / PEAK_ROOM).tanh() } else { land };
        Landform { height, mountain: relief.mountain }
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
