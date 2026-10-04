//! Stage 1: continents and mountain ranges (the relief the climate sees), hills on top,
//! then the rivers and lakes that cut into the land, reduced to a surface height.

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType, NoiseType};

use crate::SEA_LEVEL;
use crate::dials::WorldDials;

#[derive(Clone, Copy, Debug)]
pub struct GeoSample {
    /// Height of the top ground block, rivers and lakes carved in.
    pub height: i32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
    /// 1 in a river channel or lake, falling to 0 at the edge of its valley or shore.
    pub river: f32,
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
    river_warp: FastNoiseLite,
    rivers: FastNoiseLite,
    lakes: FastNoiseLite,
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
            river_warp: warp_noise(seed, 6, 0.004, 2, 45.0),
            rivers: noise(seed, 7, 0.0005 + 0.0007 * dials.river_density, FractalType::None, 1),
            lakes: noise(seed, 8, 0.0022, FractalType::FBm, 2),
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

    pub fn sample(&self, x: i32, z: i32) -> GeoSample {
        let relief = self.relief(x as f32, z as f32);
        let hills = self.hills.get_noise_2d(x as f32, z as f32);
        let inland = smoothstep(-0.06, 0.1, relief.continental);
        let land = relief.elevation + hills * (3.0 + 14.0 * relief.roughness) * self.hill_scale * inland;
        let (carved, river) = self.water(x, z, land, relief.mountain);
        let height = if carved > PEAK_KNEE { PEAK_KNEE + PEAK_ROOM * ((carved - PEAK_KNEE) / PEAK_ROOM).tanh() } else { carved };
        GeoSample { height: (height as i32).max(4), mountain: relief.mountain, river }
    }

    /// Lowers `land` into river valleys and lake basins; returns the new height and how
    /// close fresh water is (see [`GeoSample::river`]).
    ///
    /// Rivers run where a warped noise crosses zero. The distance to that line in blocks
    /// is the noise value over its gradient, so channels keep their width whether the
    /// noise is steep or flat there. Their surface is sea level, so they join the sea and
    /// any lake they cross; in high country they shrink into dry valleys and end.
    fn water(&self, x: i32, z: i32, land: f32, mountain: f32) -> (f32, f32) {
        let sea = SEA_LEVEL as f32;
        let above = land - sea;
        if above < -3.0 {
            return (land, 0.0);
        }
        let mut height = land;
        let mut wet = 0.0f32;

        let strength = (1.0 - smoothstep(0.2, 0.5, mountain)) * (1.0 - smoothstep(28.0, 56.0, above));
        if strength > 0.0 {
            let (wx, wz) = self.river_warp.domain_warp_2d(x as f32, z as f32);
            let n = self.rivers.get_noise_2d(wx, wz);
            let gradient = (self.rivers.get_noise_2d(wx + 1.0, wz) - n).hypot(self.rivers.get_noise_2d(wx, wz + 1.0) - n);
            let distance = n.abs() / gradient.max(1e-5);
            let channel = 1.5 + 4.0 * strength;
            // Valley sides about as steep as the hills around them.
            let valley = channel + 3.0 + above.max(0.0) * 1.6;
            if distance < valley {
                // A fading river's surface rises from just under sea level to the land.
                let level = sea - 0.5 + (1.0 - strength) * (above + 0.5).max(0.0);
                let carved = if distance < channel {
                    level - (1.0 + 2.5 * strength) * (1.0 - (distance / channel).powi(2))
                } else {
                    level + 1.0 + (land - level - 1.0) * smoothstep(channel, valley, distance)
                };
                height = height.min(carved);
                wet = strength * (1.0 - smoothstep(channel, valley, distance));
            }
        }

        // Lakes fill hollows in low, flat country.
        let lake = self.lakes.get_noise_2d(x as f32, z as f32);
        let basin = smoothstep(0.45, 0.7, lake) * (1.0 - smoothstep(5.0, 15.0, above)) * (1.0 - smoothstep(0.1, 0.3, mountain));
        if basin > 0.0 {
            height = height.min(land - basin * (above + 6.0));
            wet = wet.max(smoothstep(0.0, 0.5, basin));
        }
        (height, wet)
    }
}

/// Geology already sampled over a square of positions, which falls back to sampling
/// anywhere else.
pub struct GeoGrid<'a> {
    geology: &'a Geology,
    min: (i32, i32),
    size: i32,
    samples: Vec<GeoSample>,
}

impl<'a> GeoGrid<'a> {
    /// Samples the `size`×`size` positions from `min`.
    pub fn new(geology: &'a Geology, min: (i32, i32), size: i32) -> Self {
        let samples = (0..size * size).map(|i| geology.sample(min.0 + i % size, min.1 + i / size)).collect();
        Self { geology, min, size, samples }
    }

    pub fn samples(&self) -> &[GeoSample] {
        &self.samples
    }

    pub fn get(&self, x: i32, z: i32) -> GeoSample {
        let (i, j) = (x - self.min.0, z - self.min.1);
        if (0..self.size).contains(&i) && (0..self.size).contains(&j) {
            self.samples[(j * self.size + i) as usize]
        } else {
            self.geology.sample(x, z)
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
