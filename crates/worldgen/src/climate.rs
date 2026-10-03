//! Stage 2: temperature and precipitation, and the biome they select.
//!
//! Biomes are chosen per region, not per block: the land is divided into irregular
//! regions about a kilometre across (a domain-warped jittered Voronoi diagram), and each
//! region takes the biome its centre's climate selects. That gives large, coherent biomes
//! without speckles of other biomes inside them. Height still decides oceans, beaches and
//! mountains locally, and tints use the smooth per-position climate.

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType};

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
    /// Picks between biomes that share a climate (desert or badlands, forest kinds).
    variant: FastNoiseLite,
    /// Bends region borders so they are not straight lines.
    border_warp: FastNoiseLite,
    seed: u64,
}

/// Average biome region size in blocks.
const REGION: i32 = 1024;

impl Climate {
    pub fn new(seed: u64) -> Self {
        Self {
            // Low frequencies and few octaves give biomes a few kilometres across with
            // smooth borders instead of thin bands.
            temperature: noise(seed, 10, 0.00035, FractalType::FBm, 2),
            precipitation: noise(seed, 11, 0.0004, FractalType::FBm, 2),
            variant: noise(seed, 12, 0.0006, FractalType::FBm, 2),
            border_warp: {
                let mut warp = noise(seed, 13, 0.002, FractalType::DomainWarpProgressive, 3);
                warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
                warp.set_domain_warp_amp(Some(160.0));
                warp
            },
            seed,
        }
    }

    /// Centre of the biome region containing (x, z).
    fn region_center(&self, x: i32, z: i32) -> (f32, f32) {
        let (wx, wz) = self.border_warp.domain_warp_2d(x as f32, z as f32);
        let cell = (wx.div_euclid(REGION as f32) as i32, wz.div_euclid(REGION as f32) as i32);
        let mut best = (f32::MAX, 0.0, 0.0);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (cx, cz) = (cell.0 + dx, cell.1 + dz);
                let h = crate::rng::hash3(self.seed ^ 0xb10e, cx, 0, cz);
                let jitter = |bits: u64| 0.15 + 0.7 * ((bits & 0xffff) as f32 / 65535.0);
                let px = (cx as f32 + jitter(h)) * REGION as f32;
                let pz = (cz as f32 + jitter(h >> 16)) * REGION as f32;
                let distance = (px - wx).powi(2) + (pz - wz).powi(2);
                if distance < best.0 {
                    best = (distance, px, pz);
                }
            }
        }
        (best.1, best.2)
    }

    pub fn sample(&self, x: i32, z: i32, geo: &GeoSample) -> ClimateSample {
        let altitude = (geo.height - SEA_LEVEL - 20).max(0) as f32;
        let temperature = ((self.temperature.get_noise_2d(x as f32, z as f32) * 0.7 + 0.5) - altitude * 0.006).clamp(0.0, 1.0);
        let precipitation = (self.precipitation.get_noise_2d(x as f32, z as f32) * 0.7 + 0.5).clamp(0.0, 1.0);
        // The region's biome comes from the climate at its centre.
        let (cx, cz) = self.region_center(x, z);
        let region_temperature = (self.temperature.get_noise_2d(cx, cz) * 0.7 + 0.5 - altitude * 0.006).clamp(0.0, 1.0);
        let region_precipitation = (self.precipitation.get_noise_2d(cx, cz) * 0.7 + 0.5).clamp(0.0, 1.0);
        let variant = self.variant.get_noise_2d(cx, cz);
        let biome = pick(region_temperature, region_precipitation, variant, geo);
        ClimateSample { temperature, precipitation, biome }
    }
}

fn pick(t: f32, p: f32, variant: f32, geo: &GeoSample) -> Biome {
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
        (t, p) if t > 0.7 => match p {
            p if p < 0.35 && variant > 0.35 => Biome::Badlands,
            p if p < 0.35 => Biome::Desert,
            p if p < 0.6 => Biome::Savanna,
            _ => Biome::Jungle,
        },
        (_, p) if p < 0.38 => Biome::Plains,
        _ if variant > 0.3 => Biome::BirchForest,
        _ => Biome::Forest,
    }
}
