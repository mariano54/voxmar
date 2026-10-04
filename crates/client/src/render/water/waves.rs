//! The waves on water: a tiling texture of gentle swells, made here once, which the water
//! shader lays over the surface several times at different sizes, turns and speeds
//! (`water_surface.wgsl`). Summed, the layers never line up, so the surface shows no
//! pattern repeating, as a sum of a few sine waves does; and each layer is one texture
//! sample, where the sines needed a dozen octaves.
//!
//! How high the waves are is the weather's and the water's: only open water in a wind
//! carries waves (the mesher counts how much open water is around each vertex), so a
//! puddle or a sheltered pool lies almost still.

use bevy::math::{DVec2, Vec2, Vec4};

/// Texels along a side of the wave texture.
pub const TEXELS: u32 = 256;

/// One layer of waves: the texture, `size` blocks across, turned by `turn`, drifting with the
/// wind at `drift` times its speed (plus a little in calm air), its slopes weighted by
/// `weight`.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub size: f64,
    pub turn: f64,
    pub drift: f64,
    pub weight: f32,
}

/// The layers on still water, longest first. Sizes and turns share no ratio, so the layers'
/// repeats never coincide.
pub const LAYERS: [Layer; 4] = [
    Layer { size: 43.0, turn: 0.0, drift: 0.30, weight: 1.0 },
    Layer { size: 17.3, turn: 2.13, drift: 0.21, weight: 0.6 },
    Layer { size: 6.9, turn: 4.01, drift: 0.14, weight: 0.36 },
    Layer { size: 2.7, turn: 5.29, drift: 0.09, weight: 0.2 },
];

/// Blocks per second calm water still drifts, so a still surface is not frozen.
const CALM_DRIFT: f64 = 0.35;

/// Where each layer has drifted to, integrated frame by frame so that a change of wind
/// turns the drift rather than jumping the pattern.
#[derive(Clone, Copy, Default, Debug)]
pub struct Drift {
    offsets: [DVec2; LAYERS.len()],
}

impl Drift {
    /// Moves every layer on by `seconds` of a `wind` (blocks per second along x and z).
    pub fn advance(&mut self, wind: Vec2, seconds: f64) {
        let wind = wind.as_dvec2();
        for (i, (offset, layer)) in self.offsets.iter_mut().zip(LAYERS).enumerate() {
            // In calm air each layer drifts its own slow way, so they slide over each other.
            let calm = DVec2::from_angle(1.3 + i as f64 * 2.4) * CALM_DRIFT;
            *offset += (wind * layer.drift + calm) * seconds;
            // The pattern repeats every `size` blocks along its turned axes, but not along
            // x and z; keep the offset small by wrapping a large period instead.
            *offset = offset.rem_euclid(DVec2::splat(layer.size * 4096.0));
        }
    }

    /// Each layer for the shader: (cos turn, sin turn, 1 / size, weight), and the texture
    /// coordinates of `origin` in it (wrapped to the unit square), worked out in double
    /// precision so the shader only adds what is near the camera.
    pub fn uniform(&self, origin: DVec2) -> ([Vec4; LAYERS.len()], [Vec4; LAYERS.len()]) {
        let mut turns = [Vec4::ZERO; LAYERS.len()];
        let mut phases = [Vec4::ZERO; LAYERS.len()];
        for (i, layer) in LAYERS.iter().enumerate() {
            let (sin, cos) = layer.turn.sin_cos();
            turns[i] = Vec4::new(cos as f32, sin as f32, (1.0 / layer.size) as f32, layer.weight);
            let uv = rotate(origin - self.offsets[i], cos, sin) / layer.size;
            phases[i] = Vec4::new(uv.x.rem_euclid(1.0) as f32, uv.y.rem_euclid(1.0) as f32, 0.0, 0.0);
        }
        (turns, phases)
    }
}

fn rotate(p: DVec2, cos: f64, sin: f64) -> DVec2 {
    DVec2::new(cos * p.x - sin * p.y, sin * p.x + cos * p.y)
}

/// The wave texture with every mip level, as Rgba8Snorm texels: the swell's slope along u
/// and v (scaled so the steepest is ±1) and its height (−1..1).
pub fn texture() -> (Vec<u8>, u32) {
    let n = TEXELS as usize;
    let mut height = vec![0.0f32; n * n];
    // Octaves of tiling gradient noise, from 4 to 32 lattice cells across.
    for (cells, weight) in [(4, 1.0), (8, 0.5), (16, 0.25), (32, 0.125)] {
        for y in 0..n {
            for x in 0..n {
                let p = Vec2::new(x as f32, y as f32) * cells as f32 / n as f32;
                height[y * n + x] += weight * gradient_noise(p, cells);
            }
        }
    }
    let at = |x: usize, y: usize| height[(y % n) * n + x % n];
    let mut texels: Vec<[f32; 3]> = (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let du = (at(x + 1, y) - at(x + n - 1, y)) * 0.5;
            let dv = (at(x, y + 1) - at(x, y + n - 1)) * 0.5;
            [du, dv, height[i]]
        })
        .collect();
    let steepest = texels.iter().map(|t| t[0].abs().max(t[1].abs())).fold(1e-6, f32::max);
    let tallest = texels.iter().map(|t| t[2].abs()).fold(1e-6, f32::max);
    for t in &mut texels {
        *t = [t[0] / steepest, t[1] / steepest, t[2] / tallest];
    }
    let mut bytes = Vec::new();
    let mut levels = 0;
    let mut size = n;
    loop {
        for t in &texels {
            bytes.extend(t.map(|c| (c.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8));
            bytes.push(0);
        }
        levels += 1;
        if size == 1 {
            break;
        }
        let half = size / 2;
        texels = (0..half * half)
            .map(|i| {
                let (x, y) = (i % half * 2, i / half * 2);
                let corners = [texels[y * size + x], texels[y * size + x + 1], texels[(y + 1) * size + x], texels[(y + 1) * size + x + 1]];
                std::array::from_fn(|c| corners.iter().map(|t| t[c]).sum::<f32>() / 4.0)
            })
            .collect();
        size = half;
    }
    (bytes, levels)
}

/// Gradient noise at `p` (in lattice cells) repeating every `period` cells, about −1..1.
fn gradient_noise(p: Vec2, period: u32) -> f32 {
    let cell = p.floor();
    let f = p - cell;
    let fade = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let corner = |dx: u32, dy: u32| {
        let (cx, cy) = ((cell.x as u32 + dx) % period, (cell.y as u32 + dy) % period);
        let angle = hash(cx, cy, period) * std::f32::consts::TAU;
        Vec2::from_angle(angle).dot(f - Vec2::new(dx as f32, dy as f32))
    };
    let (a, b, c, d) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
    let x0 = a + (b - a) * fade.x;
    let x1 = c + (d - c) * fade.x;
    (x0 + (x1 - x0) * fade.y) * 1.4
}

fn hash(x: u32, y: u32, salt: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ salt.wrapping_mul(0xcb1a_b31f);
    h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h >> 8) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_tiles_and_has_every_level() {
        let (bytes, levels) = texture();
        assert_eq!(levels, TEXELS.trailing_zeros() + 1);
        let texels: usize = (0..levels).map(|l| ((TEXELS >> l) * (TEXELS >> l)) as usize).sum();
        assert_eq!(bytes.len(), texels * 4);
        // Heights across the seam differ no more than between neighbours inside.
        let n = TEXELS as usize;
        let h = |x: usize, y: usize| bytes[(y * n + x) * 4 + 2] as i8 as i32;
        let inside = (0..n).map(|y| (h(1, y) - h(0, y)).abs()).max().unwrap();
        let seam = (0..n).map(|y| (h(0, y) - h(n - 1, y)).abs()).max().unwrap();
        assert!(seam <= inside + 2, "seam {seam}, inside {inside}");
    }

    #[test]
    fn phases_are_continuous_across_origins() {
        // A point's texture coordinates are the same measured from either of two origins.
        let mut drift = Drift::default();
        drift.advance(Vec2::new(3.0, -1.0), 1234.5);
        let point = DVec2::new(1_000_123.25, -7_654_321.5);
        let coords = |origin: DVec2| {
            let (turns, phases) = drift.uniform(origin);
            (0..LAYERS.len())
                .map(|i| {
                    let t = turns[i];
                    let d = (point - origin).as_vec2();
                    let uv = Vec2::new(t.x * d.x - t.y * d.y, t.y * d.x + t.x * d.y) * t.z + Vec2::new(phases[i].x, phases[i].y);
                    uv - uv.floor()
                })
                .collect::<Vec<_>>()
        };
        let (a, b) = (coords(DVec2::new(1_000_000.0, -7_654_400.0)), coords(DVec2::new(1_000_256.0, -7_654_144.0)));
        for (a, b) in a.iter().zip(&b) {
            let d = (*a - *b).abs();
            let d = Vec2::new(d.x.min(1.0 - d.x), d.y.min(1.0 - d.y));
            assert!(d.max_element() < 1e-3, "{a} vs {b}");
        }
    }
}
