//! Stage 2: temperature and precipitation, and the biome they select.

use fastnoise_lite::{FastNoiseLite, FractalType};

use crate::SEA_LEVEL;
use crate::geology::{GeoSample, noise};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    Ocean,
    FrozenOcean,
    Beach,
    Plains,
    Forest,
    BirchForest,
    Taiga,
    SnowyTundra,
    Desert,
    Savanna,
    Jungle,
    Badlands,
    Mountains,
    SnowyPeaks,
}

pub struct ClimateSample {
    /// 0 (frozen) ..= 1 (hot), after altitude cooling.
    pub temperature: f32,
    /// 0 (arid) ..= 1 (wet).
    pub precipitation: f32,
    pub biome: Biome,
}

pub struct Climate {
    temperature: FastNoiseLite,
    precipitation: FastNoiseLite,
}

impl Climate {
    pub fn new(seed: u64) -> Self {
        Self {
            temperature: noise(seed, 10, 0.0009, FractalType::FBm, 3),
            precipitation: noise(seed, 11, 0.0011, FractalType::FBm, 3),
        }
    }

    pub fn sample(&self, x: i32, z: i32, geo: &GeoSample) -> ClimateSample {
        let altitude = (geo.height - SEA_LEVEL - 20).max(0) as f32;
        let temperature = ((self.temperature.get_noise_2d(x as f32, z as f32) * 0.7 + 0.5) - altitude * 0.006).clamp(0.0, 1.0);
        let precipitation = (self.precipitation.get_noise_2d(x as f32, z as f32) * 0.7 + 0.5).clamp(0.0, 1.0);
        let biome = pick(temperature, precipitation, geo);
        ClimateSample { temperature, precipitation, biome }
    }
}

fn pick(t: f32, p: f32, geo: &GeoSample) -> Biome {
    let h = geo.height;
    if h < SEA_LEVEL - 3 {
        return if t < 0.2 { Biome::FrozenOcean } else { Biome::Ocean };
    }
    if h <= SEA_LEVEL + 2 && t > 0.25 && geo.mountain < 0.3 {
        return Biome::Beach;
    }
    if h > SEA_LEVEL + 90 {
        return if t < 0.45 { Biome::SnowyPeaks } else { Biome::Mountains };
    }
    if h > SEA_LEVEL + 55 && geo.mountain > 0.4 {
        return Biome::Mountains;
    }
    match (t, p) {
        (t, p) if t < 0.22 => if p > 0.5 { Biome::Taiga } else { Biome::SnowyTundra },
        (t, p) if t < 0.38 => if p > 0.35 { Biome::Taiga } else { Biome::Plains },
        (t, p) if t > 0.72 => match p {
            p if p < 0.3 => Biome::Desert,
            p if p < 0.42 => Biome::Badlands,
            p if p < 0.62 => Biome::Savanna,
            _ => Biome::Jungle,
        },
        (_, p) if p < 0.38 => Biome::Plains,
        (_, p) if p < 0.62 => Biome::Forest,
        _ => Biome::BirchForest,
    }
}
