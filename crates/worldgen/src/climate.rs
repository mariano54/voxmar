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
//! These regional fields vary over hundreds of blocks, so they are computed on a lattice
//! every `CELL` blocks and interpolated; only altitude cooling is applied per block.

use std::f32::consts::PI;

use fastnoise_lite::{FastNoiseLite, FractalType};
use voxmar_engine::glam::IVec2;

use crate::SEA_LEVEL;
use crate::dials::WorldDials;
use crate::geology::{Geology, noise, smoothstep, spline};

/// Lattice spacing of the regional climate, in blocks.
pub const CELL: i32 = 64;
/// Cooling with height above sea level, °C per block.
pub const LAPSE_RATE: f32 = 0.11;
/// Distances upwind, far to near, at which the air crossing a point is followed.
const UPWIND: [f32; 9] = [2600.0, 1800.0, 1250.0, 850.0, 560.0, 360.0, 220.0, 120.0, 50.0];
/// Distance over land in which the air loses all but a third of its moisture.
const RAIN_OUT: f32 = 3000.0;
/// Rise in blocks over which lifted air loses all but a third of its moisture.
const LIFT_OUT: f32 = 60.0;

/// The slowly varying part of the climate at a position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Regional {
    /// Air temperature at sea level, °C.
    pub sea_temperature: f32,
    /// Annual precipitation, mm.
    pub precipitation: f32,
    /// 0 at the coast ..= 1 deep inland.
    pub continentality: f32,
    /// 0 at the equator ..= 1 at a pole.
    pub latitude: f32,
}

impl Regional {
    fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self {
            sea_temperature: mix(self.sea_temperature, other.sea_temperature),
            precipitation: mix(self.precipitation, other.precipitation),
            continentality: mix(self.continentality, other.continentality),
            latitude: mix(self.latitude, other.latitude),
        }
    }
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
}

impl Climate {
    pub fn new(seed: u64, dials: &WorldDials) -> Self {
        Self {
            dials: *dials,
            latitude_warp: noise(seed, 10, 0.8 / dials.pole_distance, FractalType::FBm, 2),
            anomaly: noise(seed, 11, 1.0 / 1600.0, FractalType::FBm, 2),
            wetness: noise(seed, 12, 1.0 / 1200.0, FractalType::FBm, 2),
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

    /// The regional climate at one position. Expensive: it follows the wind upwind.
    fn regional(&self, geology: &Geology, x: f32, z: f32) -> Regional {
        let sea = SEA_LEVEL as f32;
        let [wind_x, wind_z] = self.dials.wind;
        // Air arriving from beyond the walk carries an average amount of water.
        let mut moisture = 0.5f32;
        let mut last: Option<(f32, f32)> = None;
        let (mut lift, mut continental) = (0.0, 0.0);
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
            continental = relief.continental;
        }
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
        Regional {
            sea_temperature,
            precipitation,
            continentality: smoothstep(-0.05, 0.6, continental),
            latitude,
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
