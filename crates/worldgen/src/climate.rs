//! Stage 2: climate as continuous fields instead of biomes.
//!
//! Temperature falls from the equator towards the poles, plus slow regional anomalies
//! kilometres across. Latitude repeats along z with colder land to the north, so climate
//! zones are large and ordered: hot, temperate and cold land always lie in that order,
//! with wide transitions between them.
//!
//! Precipitation follows the moisture the prevailing wind carries in from the sea, worked
//! out on a coarse base lattice over the continents and ranges alone (`Base`): coasts are
//! wet, interiors drier, and the lee of a range lies in a rain shadow, as regions
//! kilometres wide. The land's regions read this base climate (`crate::router`), and it
//! never reads the land's local relief, so the climate the biomes see does not follow every
//! contour and slope: that is what kept biomes small (`docs/terrain-foundation.md`).
//!
//! The regional lattice carries the fields biomes are chosen by: continentalness and a
//! landform class read from the land, and seeded fields for weirdness, aether, fertility
//! and sub-biome variants. These follow landforms or vary over hundreds of blocks, never
//! block by block, so biome borders are smooth curves rather than speckle.
//!
//! These regional fields vary over hundreds of blocks, so they are computed on a lattice
//! every `CELL` blocks and interpolated; only altitude cooling is applied per block.

use std::f32::consts::PI;
use std::sync::LazyLock;

use fastnoise_lite::{FastNoiseLite, FractalType};
use rustc_hash::FxHashMap;
use voxmar_engine::glam::IVec2;

use crate::SEA_LEVEL;
use crate::content::{ClimateDef, HumidityDef};
use crate::dials::WorldDials;
use crate::geology::{Geology, Relief, noise, smoothstep, spline};
use crate::levels::LevelsDef;

/// Lattice spacing of the regional climate, in blocks.
pub const CELL: i32 = 64;
/// Cooling with height above sea level, °C per block.
pub const LAPSE_RATE: f32 = 0.11;
/// Distances upwind, far to near, at which the air crossing a point of the base lattice
/// is followed, ending at the point.
const UPWIND: [f32; 8] = [4800.0, 3600.0, 2600.0, 1800.0, 1200.0, 700.0, 300.0, 0.0];
/// Distance over land in which the air loses all but a third of its moisture.
const RAIN_OUT: f32 = 3000.0;
/// How far around a position the land's height and landform are averaged for the climate
/// biomes see, in blocks: wide, so a whole range cools as one rather than banding with its
/// contours. A whole number of lattice cells, so on the lattice the positions around a
/// point are other points, whose relief is worked out once for both.
const UPLAND: f32 = (6 * CELL) as f32;
/// The positions whose relief makes up the regional climate at a position: the position,
/// then around it.
const AROUND: [(f32, f32); 5] = [(0.0, 0.0), (1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)];
/// Rise of a range, in blocks, from the bottom to the top of the router's mountain field,
/// and the rise over which lifted air loses all but a third of its moisture.
const RANGE_RISE: f32 = 110.0;
const LIFT_OUT: f32 = 60.0;

/// The climate the land's regions read, on a lattice `geology::BASE_CELL` blocks apart:
/// pure functions of the seed and the large-scale land (continents and ranges), so the land
/// can read them without reading itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Base {
    /// Air temperature at sea level, °C.
    pub sea_temperature: f32,
    /// Annual precipitation, mm: wet coasts and windward ranges, dry interiors and rain
    /// shadows, at the scale of whole ranges and coasts.
    pub precipitation: f32,
    /// The humidity biomes are placed by, in moisture's units (see `HumidityDef`).
    pub humidity: f32,
}

impl Base {
    pub(crate) fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            sea_temperature: mix(self.sea_temperature, other.sea_temperature),
            precipitation: mix(self.precipitation, other.precipitation),
            humidity: mix(self.humidity, other.humidity),
        }
    }
}

impl crate::water::cache::Weigh for Base {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

/// The slowly varying part of the climate at a position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Regional {
    /// Air temperature at sea level, °C.
    pub sea_temperature: f32,
    /// Annual precipitation, mm.
    pub precipitation: f32,
    /// Precipitation without the extra rain of windward slopes, which changes from one
    /// slope to the next.
    pub rainfall: f32,
    /// The humidity biomes and plants go by, in moisture's units (see `HumidityDef`).
    pub humidity: f32,
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
    /// Evenly spread over 0 ..= 1, varying over kilometres: picks sub-biomes.
    pub variant: f32,
    /// Offset that warps rock province borders, in blocks.
    pub warp: [f32; 2],
    /// How strongly distance from spawn favours higher-level biomes here (P15): 0 at the
    /// origin, growing outwards and scaled by a slow wildness field, up to 2 far out where
    /// the land is wildest (see `crate::levels`).
    pub lean: f32,
}

impl Regional {
    /// Air temperature at the regional elevation, °C: what biomes and terrain shapes are
    /// placed by, without the hills, valleys and features of each spot.
    pub fn temperature(&self) -> f32 {
        self.sea_temperature - LAPSE_RATE * (self.elevation - SEA_LEVEL as f32).max(0.0)
    }

    /// The regional humidity, in moisture's units: what biomes and terrain shapes are
    /// placed by.
    pub fn moisture(&self) -> f32 {
        self.humidity
    }

    pub(crate) fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            sea_temperature: mix(self.sea_temperature, other.sea_temperature),
            precipitation: mix(self.precipitation, other.precipitation),
            rainfall: mix(self.rainfall, other.rainfall),
            humidity: mix(self.humidity, other.humidity),
            continentalness: mix(self.continentalness, other.continentalness),
            latitude: mix(self.latitude, other.latitude),
            elevation: mix(self.elevation, other.elevation),
            erosion: mix(self.erosion, other.erosion),
            weirdness: mix(self.weirdness, other.weirdness),
            aether: mix(self.aether, other.aether),
            fertility: mix(self.fertility, other.fertility),
            variant: mix(self.variant, other.variant),
            warp: [mix(self.warp[0], other.warp[0]), mix(self.warp[1], other.warp[1])],
            lean: mix(self.lean, other.lean),
        }
    }
}

/// How rugged the landform is, in steps: 0 on flat land, 1/3 in hill country, 2/3 on the
/// slopes of a range and 1 along its peaks. Each terrain class gets one value with short
/// ramps between them (Tectonic's terrain classification), so biome borders follow the
/// landforms instead of the noise that shapes them.
pub fn landform(relief: &Relief) -> f32 {
    let inland = smoothstep(crate::geology::COAST - 0.02, crate::geology::COAST + 0.14, relief.continental);
    let hills = smoothstep(0.42, 0.58, relief.roughness * inland) / 3.0;
    hills.max(range(relief)).max(smoothstep(0.22, 0.34, relief.peak))
}

/// The slopes of a range: 2/3 on them, 0 off them.
fn range(relief: &Relief) -> f32 {
    smoothstep(0.3, 0.42, relief.mountain) * 2.0 / 3.0
}

/// −1 over the deep ocean, 0 at the coast ..= 1 deep inland, from the router's
/// continentalness. It changes fastest at the coast, so coastal biome borders are sharp.
pub(crate) fn continentalness(continental: f32) -> f32 {
    use crate::geology::COAST;
    let ease = |t: f32| t * (2.0 - t);
    if continental >= COAST {
        ease(((continental - COAST) / 0.64).min(1.0))
    } else {
        -ease(((COAST - continental) / 0.46).min(1.0))
    }
}

/// Inverse of the standard normal distribution function, by a logistic fit that is
/// accurate to a few percent over 0.05..0.95.
fn probit(p: f32) -> f32 {
    let p = p.clamp(0.01, 0.99);
    (p / (1.0 - p)).ln() / 1.702
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
    /// Where distance leans hardest towards higher-level biomes (see `Regional::lean`).
    wildness: FastNoiseLite,
    levels: LevelsDef,
    humidity: HumidityDef,
}

/// How far rock province borders are pushed around, in blocks.
const PROVINCE_WARP: f32 = 450.0;

impl Climate {
    pub fn new(seed: u64, dials: &WorldDials, def: &ClimateDef) -> Self {
        let scales = &def.scales;
        Self {
            dials: *dials,
            latitude_warp: noise(seed, 10, 0.8 / dials.pole_distance, FractalType::FBm, 2),
            // Anomalies and wetness vary over many kilometres, so climate zones are large;
            // weirdness slowly, so that biomes sharing a climate split it into regions
            // kilometres across, and variants too.
            anomaly: noise(seed, 11, 1.0 / scales.anomaly, FractalType::FBm, 2),
            wetness: noise(seed, 12, 1.0 / scales.wetness, FractalType::FBm, 2),
            weirdness: noise(seed, 13, 1.0 / scales.weirdness, FractalType::FBm, 2),
            aether: noise(seed, 14, 1.0 / scales.aether, FractalType::FBm, 2),
            aether_exponent: dials.fantasy.clamp(0.01, 0.99).ln() / 0.5f32.ln(),
            fertility: noise(seed, 15, 1.0 / scales.fertility, FractalType::FBm, 2),
            variant: noise(seed, 16, 1.0 / scales.variant, FractalType::FBm, 2),
            warp: [noise(seed, 17, 1.0 / 1100.0, FractalType::FBm, 2), noise(seed, 18, 1.0 / 1100.0, FractalType::FBm, 2)],
            wildness: noise(seed, 19, 1.0 / def.levels.wildness, FractalType::FBm, 2),
            levels: def.levels.clone(),
            humidity: def.humidity.clone(),
        }
    }

    /// 0 at the equator, 1 at a pole; 0.5 at z = 0. Latitude changes evenly between them
    /// (a triangle wave with rounded tips), so every climate zone gets its share of land,
    /// and zones wander north and south (by `wander`, the latitude warp noise at the
    /// position) so their borders are not straight lines.
    fn latitude(&self, z: f32, wander: f32) -> f32 {
        const ROUNDING: f32 = 0.98;
        let pole = self.dials.pole_distance;
        let z = z + wander * 0.3 * pole;
        0.5 - 0.5 * (ROUNDING * (z / pole * PI).sin()).asin() / ROUNDING.asin()
    }

    /// The regional climate at a lattice-free position, for sparse searches.
    pub fn regional_at(&self, geology: &Geology, at: IVec2) -> Regional {
        self.regional(geology, at.x as f32, at.y as f32)
    }

    /// The regional climate at one position.
    pub(crate) fn regional(&self, geology: &Geology, x: f32, z: f32) -> Regional {
        self.regionals(geology, &[(x, z)])[0]
    }

    /// The regional climate at each position: the same as `regional` at each, sooner. The
    /// relief at and around every position is asked for at once, so positions that lie
    /// around others, as on the lattice, are worked out once; offset by offset, so
    /// neighbours share their lanes.
    pub(crate) fn regionals(&self, geology: &Geology, points: &[(f32, f32)]) -> Vec<Regional> {
        self.regionals_with(geology, points, |unique| geology.reliefs(unique))
    }

    /// The regional climate at each position like `regionals`, the relief at and around
    /// them from `reliefs`, which gives it at each position asked for.
    pub(crate) fn regionals_with(&self, geology: &Geology, points: &[(f32, f32)], reliefs: impl FnOnce(&[(f32, f32)]) -> Vec<Relief>) -> Vec<Regional> {
        let n = points.len();
        let mut index: FxHashMap<(u32, u32), u32> = FxHashMap::default();
        let mut unique = Vec::new();
        let samples: Vec<u32> = AROUND
            .iter()
            .flat_map(|&(dx, dz)| points.iter().map(move |&(x, z)| (x + dx * UPLAND, z + dz * UPLAND)))
            .map(|(x, z)| {
                *index.entry((x.to_bits(), z.to_bits())).or_insert_with(|| {
                    unique.push((x, z));
                    (unique.len() - 1) as u32
                })
            })
            .collect();
        let reliefs = reliefs(&unique);
        let mut out = vec![Regional::default(); n];
        for (c, (four, out)) in points.chunks(4).zip(out.chunks_mut(4)).enumerate() {
            let around: [[Relief; AROUND.len()]; 4] =
                std::array::from_fn(|k| std::array::from_fn(|o| samples.get(o * n + c * 4 + k).map_or_else(Relief::default, |&i| reliefs[i as usize])));
            self.regional_lanes(geology, four, &around, out);
        }
        out
    }

    /// The base climate at each position: sea-level temperature, and precipitation from
    /// the moisture the prevailing wind carries. Walking from far upwind to the position,
    /// the air picks up water over open sea, slowly rains it out over land and loses most of
    /// it where it is lifted over a range, so coasts are wet, interiors drier and the lee of
    /// a range lies in a rain shadow. The walk reads only continents and ranges, in steps of
    /// hundreds of blocks, so these are regions kilometres wide, never stripes along each
    /// slope. Earth-like circulation adds wet tropics, dry subtropics and wet mid-latitudes.
    pub(crate) fn bases(&self, geology: &Geology, points: &[(f32, f32)]) -> Vec<Base> {
        let [wind_x, wind_z] = self.dials.wind;
        let samples: Vec<(f32, f32)> = points.iter().flat_map(|&(x, z)| UPWIND.map(|d| (x - wind_x * d, z - wind_z * d))).collect();
        let shadows = geology.shadows(&samples);
        points
            .iter()
            .zip(shadows.chunks(UPWIND.len()))
            .map(|(&(x, z), walk)| {
                // Air arriving from beyond the walk carries an average amount of water.
                let mut moisture = 0.5f32;
                let mut last: Option<(f32, f32)> = None;
                for (&distance, &(continental, mountain)) in UPWIND.iter().zip(walk) {
                    let land = continental > crate::geology::COAST;
                    if let Some((last_distance, last_mountain)) = last {
                        if land {
                            moisture *= (-(last_distance - distance) / RAIN_OUT).exp();
                        }
                        // Lifted air rains out on the way up.
                        moisture *= (-(mountain - last_mountain).max(0.0) * RANGE_RISE / LIFT_OUT).exp();
                    }
                    if !land {
                        moisture += (1.0 - moisture) * 0.45;
                    }
                    last = Some((distance, mountain));
                }
                let wander = self.latitude_warp.get_noise_2d(x, z);
                let latitude = self.latitude(z, wander);
                let wet = self.wetness.get_noise_2d(x, z);
                let wetness = (1.0 + 0.35 * wet).max(0.1);
                let humid = self.dials.humidity;
                // The humidity weighs the same three in standard units: the wetness field
                // (evenly spread, then normal), the wind's water and the belts.
                let h = &self.humidity;
                let parts = [
                    (h.wetness, probit(uniform(wet))),
                    (h.wind, ((moisture - 0.5) / 0.25).clamp(-2.5, 2.5)),
                    (h.belts, (circulation(latitude) - 0.6) / 0.25),
                ];
                let norm = parts.iter().map(|(w, _)| w * w).sum::<f32>().sqrt().max(1e-6);
                let index = parts.iter().map(|(w, v)| w * v).sum::<f32>() / norm;
                Base {
                    sea_temperature: self.sea_temperature(latitude, self.anomaly.get_noise_2d(x, z)),
                    precipitation: 2600.0 * (0.55 + 0.9 * humid) * circulation(latitude) * (0.2 + 0.8 * moisture) * wetness,
                    humidity: h.median * (0.55 + 0.9 * humid) * (h.spread * index).exp(),
                }
            })
            .collect()
    }

    /// Air temperature at sea level at `latitude`, with the anomaly noise's value there.
    fn sea_temperature(&self, latitude: f32, anomaly: f32) -> f32 {
        self.dials.mean_temperature + self.dials.latitude_gradient * (0.5 - latitude) + 5.0 * anomaly
    }

    /// The regional climate at one to four positions, their noises side by side: the base
    /// climate, with the land under it, given the relief at `AROUND` each.
    fn regional_lanes(&self, geology: &Geology, points: &[(f32, f32)], around: &[[Relief; AROUND.len()]; 4], out: &mut [Regional]) {
        let n = points.len();
        let x: [f32; 4] = std::array::from_fn(|k| points[k.min(n - 1)].0);
        let z: [f32; 4] = std::array::from_fn(|k| points[k.min(n - 1)].1);
        let sample = |noise: &FastNoiseLite| crate::noise::get_2d_lanes(noise, x, z, n);
        let sea = SEA_LEVEL as f32;
        let (wander, anomaly) = (sample(&self.latitude_warp), sample(&self.anomaly));
        let (weirdness, aether, fertility, variant) = (sample(&self.weirdness), sample(&self.aether), sample(&self.fertility), sample(&self.variant));
        let warp = self.warp.each_ref().map(sample);
        let wildness = sample(&self.wildness);
        for (k, out) in out.iter_mut().enumerate() {
            let reliefs = &around[k];
            let relief = reliefs[0];
            let latitude = self.latitude(z[k], wander[k]);
            let base = geology.base(x[k], z[k]);
            let precipitation = base.precipitation;
            *out = Regional {
                sea_temperature: self.sea_temperature(latitude, anomaly[k]),
                precipitation,
                rainfall: precipitation,
                humidity: base.humidity,
                continentalness: continentalness(relief.continental),
                latitude,
                // Averaged over several hundred blocks, so a range's flanks share one
                // climate rather than banding with every few dozen blocks of height, and the
                // landform follows hill country, plains and broad summits rather than every
                // hill and peak; a range's slopes stay slopes, so lowland biomes never climb
                // them.
                elevation: 0.4 * relief.elevation.max(sea) + 0.15 * reliefs[1..].iter().map(|r| r.elevation.max(sea)).sum::<f32>(),
                erosion: (0.4 * landform(&relief) + 0.15 * reliefs[1..].iter().map(landform).sum::<f32>()).max(range(&relief)),
                weirdness: uniform(weirdness[k]),
                aether: uniform(aether[k]).powf(self.aether_exponent),
                fertility: uniform(fertility[k]),
                variant: uniform(variant[k]),
                warp: [warp[0][k] * PROVINCE_WARP, warp[1][k] * PROVINCE_WARP],
                lean: self.levels.lean_at(x[k].hypot(z[k]), uniform(wildness[k])),
            };
        }
    }
}

/// How much rain the circulation brings at a latitude: rising air near the equator,
/// sinking air in the subtropics, storms along the polar front, and little water in the
/// cold polar air.
fn circulation(latitude: f32) -> f32 {
    spline(&[(0.0, 1.0), (0.12, 0.9), (0.3, 0.26), (0.48, 0.62), (0.66, 0.8), (0.85, 0.5), (1.0, 0.3)], latitude)
}

/// The regional climate over an area, from the lattice points the land caches, bilinearly
/// interpolated, so every caller sees exactly the same smooth fields.
pub struct ClimateGrid {
    /// Lattice coordinates of the first point.
    origin: IVec2,
    width: i32,
    points: Vec<Regional>,
}

impl ClimateGrid {
    /// Covers the blocks from `min` to `max` inclusive.
    pub fn new(geology: &Geology, min: IVec2, max: IVec2) -> Self {
        let origin = min.div_euclid(IVec2::splat(CELL));
        let size = max.div_euclid(IVec2::splat(CELL)) + 2 - origin;
        let points = (0..size.y).flat_map(|j| (0..size.x).map(move |i| origin + IVec2::new(i, j))).map(|p| geology.regional_point(p)).collect();
        Self { origin, width: size.x, points }
    }

    /// As [`Self::new`], with each lattice point worked out alone rather than through the
    /// cached tiles: the same values (`regionals_match_regional_alone`), for a few points of
    /// country a coarse decision asks about once, where working out a whole tile of 289 points
    /// for the four it reads cost twenty times as much.
    pub fn alone(geology: &Geology, min: IVec2, max: IVec2) -> Self {
        let origin = min.div_euclid(IVec2::splat(CELL));
        let size = max.div_euclid(IVec2::splat(CELL)) + 2 - origin;
        let at: Vec<(f32, f32)> = (0..size.y).flat_map(|j| (0..size.x).map(move |i| ((origin.x + i) * CELL, (origin.y + j) * CELL))).map(|(x, z)| (x as f32, z as f32)).collect();
        Self { origin, width: size.x, points: geology.climate.regionals(geology, &at) }
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
