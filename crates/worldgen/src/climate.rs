//! Stage 2: climate as continuous fields instead of biomes.
//!
//! Temperature falls from the equator towards the poles and with altitude, plus slow
//! regional anomalies. Latitude repeats along z with colder land to the north, so climate
//! zones are large and ordered: hot, temperate and cold land always lie in that order,
//! with wide transitions between them.
//!
//! Precipitation follows the moisture the prevailing wind carries in from the sea. Walking
//! from far upwind to each point, the air picks up water over open sea, slowly rains it out
//! over land and loses most of it where it is lifted over mountains, so coasts are wet,
//! interiors drier and the lee of a range lies in a rain shadow. Earth-like circulation
//! adds wet tropics, dry subtropics and wet mid-latitudes on top.
//!
//! The same lattice carries the fields biomes are chosen by: continentalness and a
//! landform class read from the continents and ranges, and seeded fields for weirdness,
//! aether, fertility and sub-biome variants. These follow landforms or vary over hundreds
//! of blocks, never block by block, so biome borders are smooth curves rather than speckle.
//!
//! These regional fields vary over hundreds of blocks, so they are computed on a lattice
//! every `CELL` blocks and interpolated; only altitude cooling is applied per block.

use std::f32::consts::PI;
use std::sync::LazyLock;

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::glam::IVec2;

use crate::SEA_LEVEL;
use crate::dials::WorldDials;
use crate::geology::{Geology, Relief, noise, smoothstep, spline};

/// Lattice spacing of the regional climate, in blocks.
pub const CELL: i32 = 64;
/// Cooling with height above sea level, °C per block.
pub const LAPSE_RATE: f32 = 0.11;
/// Distances upwind, far to near, at which the air crossing a point is followed.
const UPWIND: [f32; 9] = [2600.0, 1800.0, 1250.0, 850.0, 560.0, 360.0, 220.0, 120.0, 50.0];
/// Distance over land in which the air loses all but a third of its moisture.
const RAIN_OUT: f32 = 3000.0;
/// How far around a position its elevation is averaged for the climate biomes see, in
/// blocks.
const UPLAND: f32 = 160.0;
/// Rise in blocks over which lifted air loses all but a third of its moisture.
const LIFT_OUT: f32 = 60.0;

/// The slowly varying part of the climate at a position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Regional {
    /// Air temperature at sea level, °C.
    pub sea_temperature: f32,
    /// Annual precipitation, mm.
    pub precipitation: f32,
    /// Precipitation without the extra rain of windward slopes, which changes from one
    /// slope to the next: the regional wetness biomes are chosen by.
    pub rainfall: f32,
    /// −1 over the deep ocean, 0 at the coast ..= 1 deep inland.
    pub continentalness: f32,
    /// 0 at the equator ..= 1 at a pole.
    pub latitude: f32,
    /// Height of the continents and ranges the climate sees, without hills, valleys or
    /// terrain features: the altitude that cools biomes.
    pub elevation: f32,
    /// The landform class, see [`landform`].
    pub erosion: f32,
    /// Evenly spread over 0 ..= 1, slowly varying: picks rare biomes in its extremes.
    pub weirdness: f32,
    /// How fantastical the land is, 0 ..= 1, with the world's fantasy dial as its median.
    pub aether: f32,
    /// Evenly spread over 0 ..= 1: how rich the soil is.
    pub fertility: f32,
    /// Evenly spread over 0 ..= 1, varying over a few hundred blocks: picks sub-biomes.
    pub variant: f32,
    /// Offset that warps rock province borders, in blocks.
    pub warp: [f32; 2],
}

impl Regional {
    pub(crate) fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            sea_temperature: mix(self.sea_temperature, other.sea_temperature),
            precipitation: mix(self.precipitation, other.precipitation),
            rainfall: mix(self.rainfall, other.rainfall),
            continentalness: mix(self.continentalness, other.continentalness),
            latitude: mix(self.latitude, other.latitude),
            elevation: mix(self.elevation, other.elevation),
            erosion: mix(self.erosion, other.erosion),
            weirdness: mix(self.weirdness, other.weirdness),
            aether: mix(self.aether, other.aether),
            fertility: mix(self.fertility, other.fertility),
            variant: mix(self.variant, other.variant),
            warp: [mix(self.warp[0], other.warp[0]), mix(self.warp[1], other.warp[1])],
        }
    }
}

/// How rugged the landform is, in steps: 0 on flat land, 1/3 in hill country, 2/3 on the
/// slopes of a range and 1 along its peaks. Each terrain class gets one value with short
/// ramps between them (Tectonic's terrain classification), so biome borders follow the
/// landforms instead of the noise that shapes them.
pub fn landform(relief: &Relief) -> f32 {
    let inland = smoothstep(-0.06, 0.1, relief.continental);
    let hills = smoothstep(0.42, 0.58, relief.roughness * inland) / 3.0;
    let slopes = smoothstep(0.3, 0.42, relief.mountain) * 2.0 / 3.0;
    let peaks = smoothstep(0.22, 0.34, relief.peak);
    hills.max(slopes).max(peaks)
}

/// −1 over the deep ocean, 0 at the coast ..= 1 deep inland, from the continental noise.
/// It changes fastest at the coast, so coastal biome borders are sharp.
fn continentalness(continental: f32) -> f32 {
    const COAST: f32 = -0.04;
    let ease = |t: f32| t * (2.0 - t);
    if continental >= COAST {
        ease(((continental - COAST) / 0.64).min(1.0))
    } else {
        -ease(((COAST - continental) / 0.46).min(1.0))
    }
}

/// Spreads a two-octave noise value evenly over 0 ..= 1 through its measured distribution,
/// so a share of the range is the same share of the land. A table lookup, cheap enough to
/// call per block.
pub fn uniform(n: f32) -> f32 {
    /// Quantiles of the noise at every 1/QUANTILES, measured once from many samples.
    const QUANTILES: usize = 256;
    /// The distribution at evenly spaced noise values between the lowest and highest
    /// quantile, for finding a value's place without a search.
    const BINS: usize = 2048;
    struct Table {
        low: f32,
        scale: f32,
        cdf: Vec<f32>,
    }
    static TABLE: LazyLock<Table> = LazyLock::new(|| {
        let n = noise(0x0d15_7a1b, 0, 1.0, FractalType::FBm, 2);
        // Samples far enough apart to be nearly independent.
        let mut samples: Vec<f32> = (0..1 << 16).map(|i| n.get_noise_2d((i & 255) as f32 * 2.3, (i >> 8) as f32 * 2.3)).collect();
        samples.sort_by(f32::total_cmp);
        let quantiles: Vec<f32> = (0..=QUANTILES).map(|q| samples[(q * (samples.len() - 1)) / QUANTILES]).collect();
        let at = |n: f32| {
            let i = quantiles.partition_point(|&q| q < n).clamp(1, QUANTILES);
            let (lo, hi) = (quantiles[i - 1], quantiles[i]);
            ((i - 1) as f32 + ((n - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)) / QUANTILES as f32
        };
        let (low, high) = (quantiles[0], quantiles[QUANTILES]);
        let cdf = (0..=BINS).map(|b| at(low + (high - low) * b as f32 / BINS as f32)).collect();
        Table { low, scale: BINS as f32 / (high - low), cdf }
    });
    let table = &*TABLE;
    let t = ((n - table.low) * table.scale).clamp(0.0, BINS as f32 - 1e-3);
    let i = t as usize;
    table.cdf[i] + (table.cdf[i + 1] - table.cdf[i]) * (t - i as f32)
}

/// Water available to plants: precipitation over potential evaporation, which grows with
/// warmth. Around 0.1 in deserts, 1 where they balance, 2 in rainforests.
pub fn moisture(precipitation: f32, temperature: f32) -> f32 {
    precipitation / (250.0 + 50.0 * temperature.max(0.0))
}

pub struct Climate {
    dials: WorldDials,
    latitude_warp: FastNoiseLite,
    anomaly: FastNoiseLite,
    wetness: FastNoiseLite,
    weirdness: FastNoiseLite,
    aether: FastNoiseLite,
    /// Turns the evenly spread aether noise into aether with the fantasy dial as median.
    aether_exponent: f32,
    fertility: FastNoiseLite,
    variant: FastNoiseLite,
    warp: [FastNoiseLite; 2],
}

/// How far rock province borders are pushed around, in blocks.
const PROVINCE_WARP: f32 = 450.0;

impl Climate {
    pub fn new(seed: u64, dials: &WorldDials) -> Self {
        Self {
            dials: *dials,
            latitude_warp: noise(seed, 10, 0.8 / dials.pole_distance, FractalType::FBm, 2),
            anomaly: noise(seed, 11, 1.0 / 1600.0, FractalType::FBm, 2),
            wetness: noise(seed, 12, 1.0 / 1200.0, FractalType::FBm, 2),
            // Slow, so that biomes sharing a climate split it into regions kilometres across.
            weirdness: noise(seed, 13, 1.0 / 7000.0, FractalType::FBm, 2),
            aether: noise(seed, 14, 1.0 / 2200.0, FractalType::FBm, 2),
            aether_exponent: dials.fantasy.clamp(0.01, 0.99).ln() / 0.5f32.ln(),
            fertility: noise(seed, 15, 1.0 / 700.0, FractalType::FBm, 2),
            variant: noise(seed, 16, 1.0 / 450.0, FractalType::FBm, 2),
            warp: [noise(seed, 17, 1.0 / 1100.0, FractalType::FBm, 2), noise(seed, 18, 1.0 / 1100.0, FractalType::FBm, 2)],
        }
    }

    /// 0 at the equator, 1 at a pole; 0.5 at z = 0. Latitude changes evenly between them
    /// (a triangle wave with rounded tips), so every climate zone gets its share of land,
    /// and zones wander north and south so their borders are not straight lines.
    fn latitude(&self, x: f32, z: f32) -> f32 {
        const ROUNDING: f32 = 0.98;
        let pole = self.dials.pole_distance;
        let z = z + self.latitude_warp.get_noise_2d(x, z) * 0.3 * pole;
        0.5 - 0.5 * (ROUNDING * (z / pole * PI).sin()).asin() / ROUNDING.asin()
    }

    /// The regional climate at a lattice-free position, for sparse searches.
    pub fn regional_at(&self, geology: &Geology, at: IVec2) -> Regional {
        self.regional(geology, at.x as f32, at.y as f32)
    }

    /// The regional climate at one position. Expensive: it follows the wind upwind.
    pub(crate) fn regional(&self, geology: &Geology, x: f32, z: f32) -> Regional {
        let sea = SEA_LEVEL as f32;
        let [wind_x, wind_z] = self.dials.wind;
        // Air arriving from beyond the walk carries an average amount of water.
        let mut moisture = 0.5f32;
        let mut last: Option<(f32, f32)> = None;
        let mut lift = 0.0;
        let mut here = None;
        for distance in UPWIND.into_iter().chain([0.0]) {
            let relief = geology.relief(x - wind_x * distance, z - wind_z * distance);
            let ground = relief.elevation.max(sea);
            if let Some((last_distance, last_ground)) = last {
                if relief.elevation >= sea {
                    moisture *= (-(last_distance - distance) / RAIN_OUT).exp();
                }
                lift = (ground - last_ground).max(0.0);
                // Lifted air rains out on the way up; the last rise is the slope here.
                if distance > 0.0 {
                    moisture *= (-lift / LIFT_OUT).exp();
                }
            }
            if relief.elevation < sea {
                moisture += (1.0 - moisture) * 0.45;
            }
            last = Some((distance, ground));
            here = Some(relief);
        }
        let relief = here.expect("the walk ends here");
        // Air forced up a windward slope rains on it.
        let orographic = 1.0 + (lift / 30.0).min(1.0);

        let latitude = self.latitude(x, z);
        // Rising air near the equator, sinking air in the subtropics, storms along the
        // polar front, and little water in the cold polar air.
        let circulation =
            spline(&[(0.0, 1.0), (0.12, 0.9), (0.3, 0.26), (0.48, 0.62), (0.66, 0.8), (0.85, 0.5), (1.0, 0.3)], latitude);
        let wetness = (1.0 + 0.35 * self.wetness.get_noise_2d(x, z)).max(0.1);
        let precipitation =
            2600.0 * (0.55 + 0.9 * self.dials.humidity) * circulation * (0.2 + 0.8 * moisture) * orographic * wetness;

        let sea_temperature = self.dials.mean_temperature
            + self.dials.latitude_gradient * (0.5 - latitude)
            + 3.5 * self.anomaly.get_noise_2d(x, z);
        let even = |n: &FastNoiseLite| uniform(n.get_noise_2d(x, z));
        Regional {
            sea_temperature,
            precipitation,
            rainfall: precipitation / orographic,
            continentalness: continentalness(relief.continental),
            latitude,
            elevation: {
                // Averaged over a few hundred blocks, so a range's flanks share one
                // climate rather than banding with every few dozen blocks of height.
                let around: f32 = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)]
                    .iter()
                    .map(|(dx, dz)| geology.relief(x + dx * UPLAND, z + dz * UPLAND).elevation.max(sea))
                    .sum();
                0.4 * relief.elevation.max(sea) + 0.15 * around
            },
            erosion: landform(&relief),
            weirdness: even(&self.weirdness),
            aether: even(&self.aether).powf(self.aether_exponent),
            fertility: even(&self.fertility),
            variant: even(&self.variant),
            warp: self.warp.each_ref().map(|n| n.get_noise_2d(x, z) * PROVINCE_WARP),
        }
    }
}

/// The regional climate over an area, computed once per lattice point and bilinearly
/// interpolated, so every caller sees exactly the same smooth fields.
pub struct ClimateGrid {
    /// Lattice coordinates of the first point.
    origin: IVec2,
    width: i32,
    points: Vec<Regional>,
}

impl ClimateGrid {
    /// Covers the blocks from `min` to `max` inclusive.
    pub fn new(climate: &Climate, geology: &Geology, min: IVec2, max: IVec2) -> Self {
        let origin = min.div_euclid(IVec2::splat(CELL));
        let size = max.div_euclid(IVec2::splat(CELL)) + 2 - origin;
        let points = (0..size.y)
            .flat_map(|j| (0..size.x).map(move |i| (origin + IVec2::new(i, j)) * CELL))
            .map(|p| climate.regional(geology, p.x as f32, p.y as f32))
            .collect();
        Self { origin, width: size.x, points }
    }

    /// The regional climate at a position inside the grid's area.
    pub fn at(&self, x: i32, z: i32) -> Regional {
        let cell = IVec2::new(x, z).div_euclid(IVec2::splat(CELL)) - self.origin;
        let fraction = |v: i32| v.rem_euclid(CELL) as f32 / CELL as f32;
        let point = |dx: i32, dz: i32| self.points[((cell.y + dz) * self.width + cell.x + dx) as usize];
        let near = point(0, 0).lerp(point(1, 0), fraction(x));
        let far = point(0, 1).lerp(point(1, 1), fraction(x));
        near.lerp(far, fraction(z))
    }
}
