//! Stage 1: continents, erosion and mountain ranges, reduced to a surface height.

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType, NoiseType};

use crate::SEA_LEVEL;

pub struct GeoSample {
    pub height: i32,
    /// 0 on flat land, towards 1 on mountain ranges.
    pub mountain: f32,
}

pub struct Geology {
    warp: FastNoiseLite,
    continental: FastNoiseLite,
    erosion: FastNoiseLite,
    ridges: FastNoiseLite,
    hills: FastNoiseLite,
}

pub fn noise(seed: u64, salt: u32, frequency: f32, fractal: FractalType, octaves: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed((crate::rng::mix(seed ^ salt as u64) >> 32) as i32);
    n.set_noise_type(Some(NoiseType::OpenSimplex2));
    n.set_frequency(Some(frequency));
    n.set_fractal_type(Some(fractal));
    n.set_fractal_octaves(Some(octaves));
    n
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Piecewise-linear curve through `(x, y)` points sorted by x.
fn spline(points: &[(f32, f32)], x: f32) -> f32 {
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

impl Geology {
    pub fn new(seed: u64) -> Self {
        let mut warp = noise(seed, 1, 0.0012, FractalType::DomainWarpProgressive, 3);
        warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
        warp.set_domain_warp_amp(Some(90.0));
        Self {
            warp,
            continental: noise(seed, 2, 0.0007, FractalType::FBm, 5),
            erosion: noise(seed, 3, 0.0011, FractalType::FBm, 3),
            ridges: noise(seed, 4, 0.0022, FractalType::Ridged, 5),
            hills: noise(seed, 5, 0.008, FractalType::FBm, 4),
        }
    }

    pub fn sample(&self, x: i32, z: i32) -> GeoSample {
        let (wx, wz) = self.warp.domain_warp_2d(x as f32, z as f32);
        let continental = self.continental.get_noise_2d(wx, wz);
        let erosion = self.erosion.get_noise_2d(wx, wz);
        let ridge = (self.ridges.get_noise_2d(wx, wz) + 1.0) * 0.5;
        let hills = self.hills.get_noise_2d(x as f32, z as f32);

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
        let peaks = mountain * ridge.powf(2.2) * 150.0;
        let inland = smoothstep(-0.06, 0.1, continental);
        let height = base + peaks + hills * (3.0 + 14.0 * roughness) * inland;

        GeoSample { height: (height as i32).clamp(4, voxmar_engine::coords::WORLD_HEIGHT - 24), mountain }
    }
}
