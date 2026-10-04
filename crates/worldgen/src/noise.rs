//! FastNoiseLite's OpenSimplex2, four at a time: 3D noise at four points, and the octaves
//! of 2D fractal noise at one.
//!
//! Generation samples 3D noise on lattices of thousands of points per column (caves, rock),
//! and fractal 2D noise of up to five octaves everywhere (the land's continents, ranges and
//! hills, the climate), one call and one octave at a time. Each octave branches on which
//! corners of the simplex grids contribute, which no branch predictor can guess, and works
//! on one point while the core could work on four. Here the same arithmetic runs four wide,
//! in NEON vectors, with the branches turned into selects: across four points of a lattice
//! in 3D, and across the octaves of one point in 2D, which are independent (each octave
//! scales the point by the lacunarity and adds its value times an amplitude that, without
//! FastNoiseLite's weighting, does not depend on the octaves before it). 3D gradient
//! components are all -1, 0 or 1, so their lookups are byte table lookups.
//!
//! The results are exactly FastNoiseLite's, bit for bit: every value comes from the same
//! operations in the same order, never fused (a contribution that a branch would skip adds
//! +0.0 instead, which changes nothing, as the sum is never -0.0), as the tests check
//! against the crate itself over many points and settings. A port of FastNoiseLite 1.1.1
//! (MIT, Jordan Peck and contributors; see THIRD_PARTY_NOTICES.md), for OpenSimplex2 with
//! no fractal or with fractal Brownian motion, which is what generation's lattices use;
//! anything else, and other processors, go to the crate one point at a time.

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType, RotationType3D};

/// Whether this module evaluates `noise` itself rather than leaving it to FastNoiseLite.
pub fn supported_3d(noise: &FastNoiseLite) -> bool {
    cfg!(target_arch = "aarch64")
        && noise.noise_type == NoiseType::OpenSimplex2
        && noise.rotation_type_3d == RotationType3D::None
        && matches!(noise.fractal_type, FractalType::None | FractalType::FBm)
        // Weighting would give each point its own amplitude.
        && noise.weighted_strength == 0.
}

/// Whether this module evaluates the 2D `noise` itself: fractal OpenSimplex2 of a few
/// octaves (a single octave gains nothing by it).
pub fn supported_2d(noise: &FastNoiseLite) -> bool {
    cfg!(target_arch = "aarch64")
        && noise.noise_type == NoiseType::OpenSimplex2
        && matches!(noise.fractal_type, FractalType::FBm | FractalType::Ridged)
        && noise.octaves > 1
        && noise.weighted_strength == 0.
}

/// `noise.get_noise_2d(x, y)`, with its octaves worked out side by side when it is
/// `supported_2d`.
#[inline]
pub fn get_2d(noise: &FastNoiseLite, x: f32, y: f32) -> f32 {
    #[cfg(target_arch = "aarch64")]
    if supported_2d(noise) {
        return fractal_2d(noise, x, y);
    }
    noise.get_noise_2d(x, y)
}

/// Fractal 2D OpenSimplex2 as FastNoiseLite sums it, the octaves four at a time.
#[cfg(target_arch = "aarch64")]
#[allow(clippy::excessive_precision)]
fn fractal_2d(noise: &FastNoiseLite, x: f32, y: f32) -> f32 {
    // The frequency, then OpenSimplex2's skew.
    let (mut x, mut y) = (x * noise.frequency, y * noise.frequency);
    let sqrt3: f32 = 1.7320508075688772935274463415059;
    let f2 = 0.5 * (sqrt3 - 1.);
    let t = (x + y) * f2;
    x += t;
    y += t;
    let ridged = noise.fractal_type == FractalType::Ridged;
    let (mut seed, mut amp, mut sum) = (noise.seed, fractal_bounding(noise), 0.0f32);
    let mut octave = 0;
    while octave < noise.octaves {
        let n = (noise.octaves - octave).min(4) as usize;
        // Each octave's seed and point, as FastNoiseLite steps them.
        let (mut seeds, mut xs, mut ys) = ([0i32; 4], [0.0f32; 4], [0.0f32; 4]);
        for k in 0..n {
            (seeds[k], xs[k], ys[k]) = (seed, x, y);
            seed = seed.wrapping_add(1);
            x *= noise.lacunarity;
            y *= noise.lacunarity;
        }
        // SAFETY: NEON is part of every aarch64 processor.
        let values = unsafe { neon::simplex_2d(seeds, xs, ys) };
        for &value in &values[..n] {
            // FastNoiseLite also multiplies the amplitude by its weighting, which at
            // strength 0 is exactly 1.
            if ridged {
                sum += (value.abs() * -2. + 1.) * amp;
            } else {
                sum += value * amp;
            }
            amp *= noise.gain;
        }
        octave += n as i32;
    }
    sum
}

/// `noise.get_noise_3d` at every point, into `out`.
pub fn noise_3d(noise: &FastNoiseLite, points: &[[f32; 3]], out: &mut [f32]) {
    assert_eq!(points.len(), out.len());
    #[cfg(target_arch = "aarch64")]
    if supported_3d(noise) {
        let (octaves, bounding) = match noise.fractal_type {
            FractalType::FBm => (noise.octaves, fractal_bounding(noise)),
            _ => (1, 1.0),
        };
        for (points, out) in points.chunks(4).zip(out.chunks_mut(4)) {
            let mut lanes = [[0.0f32; 3]; 4];
            lanes[..points.len()].copy_from_slice(points);
            // SAFETY: NEON is part of every aarch64 processor.
            let values = unsafe { neon::fractal_3d(noise, &lanes, octaves, bounding) };
            out.copy_from_slice(&values[..out.len()]);
        }
        return;
    }
    for (o, p) in out.iter_mut().zip(points) {
        *o = noise.get_noise_3d(p[0], p[1], p[2]);
    }
}

/// FastNoiseLite's private normalisation of fractal sums, worked out the same way.
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
fn fractal_bounding(noise: &FastNoiseLite) -> f32 {
    let gain = noise.gain.abs();
    let mut amp = gain;
    let mut amp_fractal = 1.;
    for _ in 1..noise.octaves {
        amp_fractal += amp;
        amp *= gain;
    }
    1. / amp_fractal
}

/// FastNoiseLite's 2D gradients, as pairs.
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
#[rustfmt::skip]
#[allow(clippy::excessive_precision, clippy::approx_constant)]
const GRADIENTS_2D: [f32; 256] = [
    0.130526192220052, 0.99144486137381, 0.38268343236509, 0.923879532511287, 0.608761429008721, 0.793353340291235, 0.793353340291235, 0.608761429008721,
    0.923879532511287, 0.38268343236509, 0.99144486137381, 0.130526192220051, 0.99144486137381, -0.130526192220051, 0.923879532511287, -0.38268343236509,
    0.793353340291235, -0.60876142900872, 0.608761429008721, -0.793353340291235, 0.38268343236509, -0.923879532511287, 0.130526192220052, -0.99144486137381,
    -0.130526192220052, -0.99144486137381, -0.38268343236509, -0.923879532511287, -0.608761429008721, -0.793353340291235, -0.793353340291235, -0.608761429008721,
    -0.923879532511287, -0.38268343236509, -0.99144486137381, -0.130526192220052, -0.99144486137381, 0.130526192220051, -0.923879532511287, 0.38268343236509,
    -0.793353340291235, 0.608761429008721, -0.608761429008721, 0.793353340291235, -0.38268343236509, 0.923879532511287, -0.130526192220052, 0.99144486137381,
    0.130526192220052, 0.99144486137381, 0.38268343236509, 0.923879532511287, 0.608761429008721, 0.793353340291235, 0.793353340291235, 0.608761429008721,
    0.923879532511287, 0.38268343236509, 0.99144486137381, 0.130526192220051, 0.99144486137381, -0.130526192220051, 0.923879532511287, -0.38268343236509,
    0.793353340291235, -0.60876142900872, 0.608761429008721, -0.793353340291235, 0.38268343236509, -0.923879532511287, 0.130526192220052, -0.99144486137381,
    -0.130526192220052, -0.99144486137381, -0.38268343236509, -0.923879532511287, -0.608761429008721, -0.793353340291235, -0.793353340291235, -0.608761429008721,
    -0.923879532511287, -0.38268343236509, -0.99144486137381, -0.130526192220052, -0.99144486137381, 0.130526192220051, -0.923879532511287, 0.38268343236509,
    -0.793353340291235, 0.608761429008721, -0.608761429008721, 0.793353340291235, -0.38268343236509, 0.923879532511287, -0.130526192220052, 0.99144486137381,
    0.130526192220052, 0.99144486137381, 0.38268343236509, 0.923879532511287, 0.608761429008721, 0.793353340291235, 0.793353340291235, 0.608761429008721,
    0.923879532511287, 0.38268343236509, 0.99144486137381, 0.130526192220051, 0.99144486137381, -0.130526192220051, 0.923879532511287, -0.38268343236509,
    0.793353340291235, -0.60876142900872, 0.608761429008721, -0.793353340291235, 0.38268343236509, -0.923879532511287, 0.130526192220052, -0.99144486137381,
    -0.130526192220052, -0.99144486137381, -0.38268343236509, -0.923879532511287, -0.608761429008721, -0.793353340291235, -0.793353340291235, -0.608761429008721,
    -0.923879532511287, -0.38268343236509, -0.99144486137381, -0.130526192220052, -0.99144486137381, 0.130526192220051, -0.923879532511287, 0.38268343236509,
    -0.793353340291235, 0.608761429008721, -0.608761429008721, 0.793353340291235, -0.38268343236509, 0.923879532511287, -0.130526192220052, 0.99144486137381,
    0.130526192220052, 0.99144486137381, 0.38268343236509, 0.923879532511287, 0.608761429008721, 0.793353340291235, 0.793353340291235, 0.608761429008721,
    0.923879532511287, 0.38268343236509, 0.99144486137381, 0.130526192220051, 0.99144486137381, -0.130526192220051, 0.923879532511287, -0.38268343236509,
    0.793353340291235, -0.60876142900872, 0.608761429008721, -0.793353340291235, 0.38268343236509, -0.923879532511287, 0.130526192220052, -0.99144486137381,
    -0.130526192220052, -0.99144486137381, -0.38268343236509, -0.923879532511287, -0.608761429008721, -0.793353340291235, -0.793353340291235, -0.608761429008721,
    -0.923879532511287, -0.38268343236509, -0.99144486137381, -0.130526192220052, -0.99144486137381, 0.130526192220051, -0.923879532511287, 0.38268343236509,
    -0.793353340291235, 0.608761429008721, -0.608761429008721, 0.793353340291235, -0.38268343236509, 0.923879532511287, -0.130526192220052, 0.99144486137381,
    0.130526192220052, 0.99144486137381, 0.38268343236509, 0.923879532511287, 0.608761429008721, 0.793353340291235, 0.793353340291235, 0.608761429008721,
    0.923879532511287, 0.38268343236509, 0.99144486137381, 0.130526192220051, 0.99144486137381, -0.130526192220051, 0.923879532511287, -0.38268343236509,
    0.793353340291235, -0.60876142900872, 0.608761429008721, -0.793353340291235, 0.38268343236509, -0.923879532511287, 0.130526192220052, -0.99144486137381,
    -0.130526192220052, -0.99144486137381, -0.38268343236509, -0.923879532511287, -0.608761429008721, -0.793353340291235, -0.793353340291235, -0.608761429008721,
    -0.923879532511287, -0.38268343236509, -0.99144486137381, -0.130526192220052, -0.99144486137381, 0.130526192220051, -0.923879532511287, 0.38268343236509,
    -0.793353340291235, 0.608761429008721, -0.608761429008721, 0.793353340291235, -0.38268343236509, 0.923879532511287, -0.130526192220052, 0.99144486137381,
    0.38268343236509, 0.923879532511287, 0.923879532511287, 0.38268343236509, 0.923879532511287, -0.38268343236509, 0.38268343236509, -0.923879532511287,
    -0.38268343236509, -0.923879532511287, -0.923879532511287, -0.38268343236509, -0.923879532511287, 0.38268343236509, -0.38268343236509, 0.923879532511287,
];

/// FastNoiseLite's 3D gradients, by axis, in the order of its table (whose offsets are four
/// times these indices).
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
const GRADIENTS_3D: [[i8; 64]; 3] = {
    #[rustfmt::skip]
    const TABLE: [[i8; 3]; 64] = [
        [0, 1, 1], [0,-1, 1], [0, 1,-1], [0,-1,-1], [1, 0, 1], [-1, 0, 1], [1, 0,-1], [-1, 0,-1],
        [1, 1, 0], [-1, 1, 0], [1,-1, 0], [-1,-1, 0], [0, 1, 1], [0,-1, 1], [0, 1,-1], [0,-1,-1],
        [1, 0, 1], [-1, 0, 1], [1, 0,-1], [-1, 0,-1], [1, 1, 0], [-1, 1, 0], [1,-1, 0], [-1,-1, 0],
        [0, 1, 1], [0,-1, 1], [0, 1,-1], [0,-1,-1], [1, 0, 1], [-1, 0, 1], [1, 0,-1], [-1, 0,-1],
        [1, 1, 0], [-1, 1, 0], [1,-1, 0], [-1,-1, 0], [0, 1, 1], [0,-1, 1], [0, 1,-1], [0,-1,-1],
        [1, 0, 1], [-1, 0, 1], [1, 0,-1], [-1, 0,-1], [1, 1, 0], [-1, 1, 0], [1,-1, 0], [-1,-1, 0],
        [0, 1, 1], [0,-1, 1], [0, 1,-1], [0,-1,-1], [1, 0, 1], [-1, 0, 1], [1, 0,-1], [-1, 0,-1],
        [1, 1, 0], [-1, 1, 0], [1,-1, 0], [-1,-1, 0], [1, 1, 0], [0,-1, 1], [-1, 1, 0], [0,-1,-1],
    ];
    let mut by_axis = [[0i8; 64]; 3];
    let mut g = 0;
    while g < 64 {
        by_axis[0][g] = TABLE[g][0];
        by_axis[1][g] = TABLE[g][1];
        by_axis[2][g] = TABLE[g][2];
        g += 1;
    }
    by_axis
};

#[cfg(target_arch = "aarch64")]
mod neon {
    use std::arch::aarch64::*;

    use fastnoise_lite::{FastNoiseLite, FractalType};

    use super::{GRADIENTS_2D, GRADIENTS_3D};

    const PRIME_X: i32 = 501125321;
    const PRIME_Y: i32 = 1136930381;
    const PRIME_Z: i32 = 1720413743;

    type Vec3 = (float32x4_t, float32x4_t, float32x4_t);

    /// The noise at four points: their transform, then each octave in turn.
    #[target_feature(enable = "neon")]
    pub(super) fn fractal_3d(noise: &FastNoiseLite, points: &[[f32; 3]; 4], octaves: i32, bounding: f32) -> [f32; 4] {
        let load = |a: usize| {
            let lanes = points.map(|p| p[a]);
            // SAFETY: `lanes` holds four floats.
            unsafe { vld1q_f32(lanes.as_ptr()) }
        };
        let frequency = vdupq_n_f32(noise.frequency);
        let (px, py, pz) = (vmulq_f32(load(0), frequency), vmulq_f32(load(1), frequency), vmulq_f32(load(2), frequency));
        // OpenSimplex2's default rotation: r = (x + y + z) * 2/3, then r - x and so on.
        let r = vmulq_f32(vaddq_f32(vaddq_f32(px, py), pz), vdupq_n_f32(2. / 3.));
        let (mut x, mut y, mut z) = (vsubq_f32(r, px), vsubq_f32(r, py), vsubq_f32(r, pz));
        let tables = GRADIENTS_3D.map(|t| {
            // SAFETY: each table is 64 bytes, loaded as four rows of 16.
            unsafe { vld1q_s8_x4(t.as_ptr()) }
        });
        let sum = if noise.fractal_type == FractalType::FBm {
            let (mut seed, mut amp, mut sum) = (noise.seed, bounding, vdupq_n_f32(0.));
            let lacunarity = vdupq_n_f32(noise.lacunarity);
            for _ in 0..octaves {
                let value = simplex_3d(seed, (x, y, z), &tables);
                seed = seed.wrapping_add(1);
                sum = vaddq_f32(sum, vmulq_f32(value, vdupq_n_f32(amp)));
                // FastNoiseLite also multiplies by its weighting, which at strength 0 is
                // exactly 1.
                amp *= noise.gain;
                (x, y, z) = (vmulq_f32(x, lacunarity), vmulq_f32(y, lacunarity), vmulq_f32(z, lacunarity));
            }
            sum
        } else {
            simplex_3d(noise.seed, (x, y, z), &tables)
        };
        let mut out = [0.0f32; 4];
        // SAFETY: `out` holds four floats.
        unsafe { vst1q_f32(out.as_mut_ptr(), sum) };
        out
    }

    /// One octave of 2D OpenSimplex2 (the same algorithm as ordinary simplex noise) at
    /// skewed, scaled points, each with its own seed.
    #[target_feature(enable = "neon")]
    #[allow(clippy::excessive_precision)]
    pub(super) fn simplex_2d(seeds: [i32; 4], x: [f32; 4], y: [f32; 4]) -> [f32; 4] {
        let sqrt3: f32 = 1.7320508075688772935274463415059;
        let g2 = (3. - sqrt3) / 6.;
        let load = |v: [f32; 4]| {
            // SAFETY: four floats.
            unsafe { vld1q_f32(v.as_ptr()) }
        };
        let (x, y, seeds) = (load(x), load(y), {
            // SAFETY: four integers.
            unsafe { vld1q_s32(seeds.as_ptr()) }
        });
        let zero = vdupq_n_f32(0.);
        // FastNoiseLite's `fast_floor`: truncated, less one below zero (even at whole
        // numbers).
        let floor = |v: float32x4_t| vsubq_s32(vcvtq_s32_f32(v), vreinterpretq_s32_u32(vandq_u32(vcltq_f32(v, zero), vdupq_n_u32(1))));
        let (i, j) = (floor(x), floor(y));
        let (xi, yi) = (vsubq_f32(x, vcvtq_f32_s32(i)), vsubq_f32(y, vcvtq_f32_s32(j)));
        let t = vmulq_f32(vaddq_f32(xi, yi), vdupq_n_f32(g2));
        let (x0, y0) = (vsubq_f32(xi, t), vsubq_f32(yi, t));
        let (i, j) = (vmulq_s32(i, vdupq_n_s32(PRIME_X)), vmulq_s32(j, vdupq_n_s32(PRIME_Y)));
        let falloff = |w: float32x4_t, g: float32x4_t| {
            let w2 = vmulq_f32(w, w);
            vmulq_f32(vmulq_f32(w2, w2), g)
        };
        let a = vsubq_f32(vsubq_f32(vdupq_n_f32(0.5), vmulq_f32(x0, x0)), vmulq_f32(y0, y0));
        let n0 = vbslq_f32(vcleq_f32(a, zero), zero, falloff(a, gradient_2d(seeds, i, j, x0, y0)));
        let c = vaddq_f32(
            vmulq_f32(vdupq_n_f32(2. * (1. - 2. * g2) * (1. / g2 - 2.)), t),
            vaddq_f32(vdupq_n_f32(-2. * (1. - 2. * g2) * (1. - 2. * g2)), a),
        );
        let (x2, y2) = (vaddq_f32(x0, vdupq_n_f32(2. * g2 - 1.)), vaddq_f32(y0, vdupq_n_f32(2. * g2 - 1.)));
        let (ip, jp) = (vaddq_s32(i, vdupq_n_s32(PRIME_X)), vaddq_s32(j, vdupq_n_s32(PRIME_Y)));
        let n2 = vbslq_f32(vcleq_f32(c, zero), zero, falloff(c, gradient_2d(seeds, ip, jp, x2, y2)));
        // The middle corner: towards y where the point lies above the diagonal.
        let upper = vcgtq_f32(y0, x0);
        let x1 = vaddq_f32(x0, vbslq_f32(upper, vdupq_n_f32(g2), vdupq_n_f32(g2 - 1.)));
        let y1 = vaddq_f32(y0, vbslq_f32(upper, vdupq_n_f32(g2 - 1.), vdupq_n_f32(g2)));
        let b = vsubq_f32(vsubq_f32(vdupq_n_f32(0.5), vmulq_f32(x1, x1)), vmulq_f32(y1, y1));
        let (i1, j1) = (vbslq_s32(upper, i, ip), vbslq_s32(upper, jp, j));
        let n1 = vbslq_f32(vcleq_f32(b, zero), zero, falloff(b, gradient_2d(seeds, i1, j1, x1, y1)));
        let value = vmulq_f32(vaddq_f32(vaddq_f32(n0, n1), n2), vdupq_n_f32(99.83685446303647));
        let mut out = [0.0f32; 4];
        // SAFETY: `out` holds four floats.
        unsafe { vst1q_f32(out.as_mut_ptr(), value) };
        out
    }

    /// The dot product of offsets with the 2D gradient of each lane's lattice corner.
    #[target_feature(enable = "neon")]
    fn gradient_2d(seed: int32x4_t, i: int32x4_t, j: int32x4_t, dx: float32x4_t, dy: float32x4_t) -> float32x4_t {
        let hash = vmulq_s32(veorq_s32(veorq_s32(seed, i), j), vdupq_n_s32(0x27d4eb2d));
        let hash = vandq_s32(veorq_s32(hash, vshrq_n_s32::<15>(hash)), vdupq_n_s32(127 << 1));
        let mut offsets = [0i32; 4];
        // SAFETY: `offsets` holds four integers.
        unsafe { vst1q_s32(offsets.as_mut_ptr(), hash) };
        let (gx, gy) = (offsets.map(|o| GRADIENTS_2D[o as usize]), offsets.map(|o| GRADIENTS_2D[o as usize | 1]));
        // SAFETY: four floats each.
        let (gx, gy) = unsafe { (vld1q_f32(gx.as_ptr()), vld1q_f32(gy.as_ptr())) };
        vaddq_f32(vmulq_f32(dx, gx), vmulq_f32(dy, gy))
    }

    /// FastNoiseLite's `fast_round`: half away from zero, then truncated.
    #[target_feature(enable = "neon")]
    fn fast_round(f: float32x4_t) -> int32x4_t {
        let half = vbslq_f32(vcgeq_f32(f, vdupq_n_f32(0.)), vdupq_n_f32(0.5), vdupq_n_f32(-0.5));
        vcvtq_s32_f32(vaddq_f32(f, half))
    }

    /// The dot product of offsets `d` with the gradient of each lane's lattice corner.
    #[target_feature(enable = "neon")]
    fn gradient(seed: int32x4_t, (i, j, k): (int32x4_t, int32x4_t, int32x4_t), d: Vec3, tables: &[int8x16x4_t; 3]) -> float32x4_t {
        let hash = vmulq_s32(veorq_s32(veorq_s32(veorq_s32(seed, i), j), k), vdupq_n_s32(0x27d4eb2d));
        let hash = veorq_s32(hash, vshrq_n_s32::<15>(hash));
        // The table offset is `hash & 252`, the gradient a quarter of it: in each lane's low
        // byte, with the other bytes out of the tables' range, so they look up zero.
        let index = vreinterpretq_u8_s32(vorrq_s32(vshrq_n_s32::<2>(vandq_s32(hash, vdupq_n_s32(252))), vdupq_n_s32(0x4040_4000)));
        let component = |table: int8x16x4_t| {
            let bytes = vreinterpretq_s32_s8(vqtbl4q_s8(table, index));
            // The low byte of each lane, sign extended, as a float.
            vcvtq_f32_s32(vshrq_n_s32::<24>(vshlq_n_s32::<24>(bytes)))
        };
        let (gx, gy, gz) = (component(tables[0]), component(tables[1]), component(tables[2]));
        vaddq_f32(vaddq_f32(vmulq_f32(d.0, gx), vmulq_f32(d.1, gy)), vmulq_f32(d.2, gz))
    }

    /// `t⁴ g`, grouped as FastNoiseLite groups it.
    #[target_feature(enable = "neon")]
    fn falloff(t: float32x4_t, g: float32x4_t) -> float32x4_t {
        let t2 = vmulq_f32(t, t);
        vmulq_f32(vmulq_f32(t2, t2), g)
    }

    /// One octave of 3D OpenSimplex2 at rotated, scaled coordinates: two offset cubic
    /// grids, each with its nearest corner and the one across the strongest axis.
    #[target_feature(enable = "neon")]
    #[allow(clippy::excessive_precision)]
    fn simplex_3d(seed: i32, (x, y, z): Vec3, tables: &[int8x16x4_t; 3]) -> float32x4_t {
        let zero = vdupq_n_f32(0.);
        let (ri, rj, rk) = (fast_round(x), fast_round(y), fast_round(z));
        let (mut x0, mut y0, mut z0) = (vsubq_f32(x, vcvtq_f32_s32(ri)), vsubq_f32(y, vcvtq_f32_s32(rj)), vsubq_f32(z, vcvtq_f32_s32(rk)));
        let sign = |d: float32x4_t| vorrq_s32(vcvtq_s32_f32(vsubq_f32(vdupq_n_f32(-1.), d)), vdupq_n_s32(1));
        let (mut xs, mut ys, mut zs) = (sign(x0), sign(y0), sign(z0));
        let away = |s: int32x4_t, d: float32x4_t| vmulq_f32(vcvtq_f32_s32(s), vnegq_f32(d));
        let (mut ax, mut ay, mut az) = (away(xs, x0), away(ys, y0), away(zs, z0));
        let primes = (vdupq_n_s32(PRIME_X), vdupq_n_s32(PRIME_Y), vdupq_n_s32(PRIME_Z));
        let (mut i, mut j, mut k) = (vmulq_s32(ri, primes.0), vmulq_s32(rj, primes.1), vmulq_s32(rk, primes.2));
        let square = |v: float32x4_t| vmulq_f32(v, v);
        let mut a = vsubq_f32(vsubq_f32(vdupq_n_f32(0.6), square(x0)), vaddq_f32(square(y0), square(z0)));
        let mut value = zero;
        let mut seed = seed;
        for step in 0..2 {
            let seeds = vdupq_n_s32(seed);
            // The corner itself.
            let t = falloff(a, gradient(seeds, (i, j, k), (x0, y0, z0), tables));
            value = vaddq_f32(value, vbslq_f32(vcgtq_f32(a, zero), t, zero));
            // The corner across the axis the point lies furthest along.
            let along_x = vandq_u32(vcgeq_f32(ax, ay), vcgeq_f32(ax, az));
            let along_y = vbicq_u32(vandq_u32(vcgtq_f32(ay, ax), vcgeq_f32(ay, az)), along_x);
            let along_z = vmvnq_u32(vorrq_u32(along_x, along_y));
            let strongest = vbslq_f32(along_x, ax, vbslq_f32(along_y, ay, az));
            let b = vaddq_f32(vaddq_f32(a, strongest), strongest);
            let across = |along: uint32x4_t, c: int32x4_t, s: int32x4_t, prime: int32x4_t| vbslq_s32(along, vsubq_s32(c, vmulq_s32(s, prime)), c);
            let corner = (across(along_x, i, xs, primes.0), across(along_y, j, ys, primes.1), across(along_z, k, zs, primes.2));
            let offset = |along: uint32x4_t, d: float32x4_t, s: int32x4_t| vbslq_f32(along, vaddq_f32(d, vcvtq_f32_s32(s)), d);
            let offsets = (offset(along_x, x0, xs), offset(along_y, y0, ys), offset(along_z, z0, zs));
            let t = falloff(vsubq_f32(b, vdupq_n_f32(1.)), gradient(seeds, corner, offsets, tables));
            value = vaddq_f32(value, vbslq_f32(vcgtq_f32(b, vdupq_n_f32(1.)), t, zero));
            if step == 1 {
                break;
            }
            // On to the second grid, offset by half a cell.
            let half = vdupq_n_f32(0.5);
            (ax, ay, az) = (vsubq_f32(half, ax), vsubq_f32(half, ay), vsubq_f32(half, az));
            (x0, y0, z0) = (vmulq_f32(vcvtq_f32_s32(xs), ax), vmulq_f32(vcvtq_f32_s32(ys), ay), vmulq_f32(vcvtq_f32_s32(zs), az));
            a = vsubq_f32(vaddq_f32(a, vsubq_f32(vdupq_n_f32(0.75), ax)), vaddq_f32(ay, az));
            let step_in = |c: int32x4_t, s: int32x4_t, prime: int32x4_t| vaddq_s32(c, vandq_s32(vshrq_n_s32::<1>(s), prime));
            (i, j, k) = (step_in(i, xs, primes.0), step_in(j, ys, primes.1), step_in(k, zs, primes.2));
            (xs, ys, zs) = (vnegq_s32(xs), vnegq_s32(ys), vnegq_s32(zs));
            seed = !seed;
        }
        vmulq_f32(value, vdupq_n_f32(32.69428253173828125))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geology::noise;
    use crate::rng::Rng;

    /// Bit for bit what FastNoiseLite gives, over many points, at every setting generation
    /// uses and some it does not.
    #[test]
    fn matches_fastnoise_lite_exactly() {
        let mut rng = Rng::new(7, 7, 7, 7);
        for (salt, frequency, fractal, octaves) in
            [(37, 1.0 / 125.0, FractalType::FBm, 2), (23, 0.02, FractalType::None, 1), (40, 1.0 / 140.0, FractalType::FBm, 2), (42, 1.0 / 40.0, FractalType::None, 1), (9, 0.3, FractalType::FBm, 5)]
        {
            let n = noise(0x5eed + salt, salt as u32, frequency, fractal, octaves);
            assert!(supported_3d(&n));
            let mut points: Vec<[f32; 3]> = (0..20_001).map(|_| [rng.range(-40_000, 40_000) as f32, rng.range(0, 384) as f32 * 1.3, rng.range(-40_000, 40_000) as f32]).collect();
            // Fractions and the far reaches too.
            points.extend((0..2000).map(|_| [rng.unit() * 1e6 - 5e5, rng.unit() * 500.0, rng.unit() * 77.7 - 40.0]));
            let mut out = vec![0.0; points.len()];
            noise_3d(&n, &points, &mut out);
            for (p, v) in points.iter().zip(&out) {
                assert_eq!(v.to_bits(), n.get_noise_3d(p[0], p[1], p[2]).to_bits(), "{p:?} at {frequency} {fractal:?} {octaves}");
            }
        }
    }

    /// The 2D octaves too, for the land's and the climate's settings and others.
    #[test]
    fn fractal_2d_matches_fastnoise_lite_exactly() {
        let mut rng = Rng::new(8, 8, 8, 8);
        let mut checked = 0;
        for (salt, frequency, fractal, octaves) in [
            (2, 1.0 / 2800.0, FractalType::FBm, 5),
            (3, 0.0011, FractalType::FBm, 3),
            (4, 0.0022, FractalType::Ridged, 5),
            (5, 0.008, FractalType::FBm, 4),
            (30, 1.0 / 220.0, FractalType::FBm, 2),
            (41, 1.0 / 45.0, FractalType::FBm, 2),
            (9, 0.37, FractalType::Ridged, 3),
            (11, 1.3, FractalType::FBm, 8),
        ] {
            let n = noise(0x5eed ^ salt, salt as u32, frequency, fractal, octaves);
            assert_eq!(supported_2d(&n), cfg!(target_arch = "aarch64"));
            for i in 0..40_000 {
                let (x, y) = if i % 4 == 0 {
                    (rng.unit() * 2e6 - 1e6, rng.unit() * 2e6 - 1e6)
                } else {
                    (rng.range(-60_000, 60_000) as f32, rng.range(-60_000, 60_000) as f32 + 0.5 * (i % 3) as f32)
                };
                assert_eq!(get_2d(&n, x, y).to_bits(), n.get_noise_2d(x, y).to_bits(), "({x}, {y}) at {frequency} {fractal:?} {octaves}");
                checked += 1;
            }
        }
        assert_eq!(checked, 320_000);
    }

    /// `cargo test --release -p voxmar_worldgen noise_speed -- --ignored --nocapture`
    /// compares the time per point of the tunnels' noise, one point at a time and here.
    #[test]
    #[ignore]
    fn noise_speed() {
        crate::perf::prefer_fast_cores();
        let n = noise(1, 37, 1.0 / 125.0, FractalType::FBm, 2);
        let points: Vec<[f32; 3]> = (0..1 << 20).map(|i| [(i % 97) as f32 * 4.0, (i / 97 % 40) as f32 * 8.0 * 1.3, (i / 3880) as f32 * 4.0]).collect();
        let mut out = vec![0.0; points.len()];
        let start = crate::perf::thread_time();
        for (o, p) in out.iter_mut().zip(&points) {
            *o = n.get_noise_3d(p[0], p[1], p[2]);
        }
        let one = crate::perf::thread_time() - start;
        let check = out.iter().sum::<f32>();
        let start = crate::perf::thread_time();
        noise_3d(&n, &points, &mut out);
        let many = crate::perf::thread_time() - start;
        assert_eq!(check, out.iter().sum::<f32>());
        let ns = |d: std::time::Duration| d.as_secs_f64() * 1e9 / points.len() as f64;
        println!("3D: one at a time {:.1} ns per point, four at a time {:.1} ns", ns(one), ns(many));
        // The land's continents: five octaves of 2D noise.
        let n = noise(1, 2, 1.0 / 2800.0, FractalType::FBm, 5);
        let start = crate::perf::thread_time();
        let one: f32 = points.iter().map(|p| n.get_noise_2d(p[0], p[2])).sum();
        let scalar = crate::perf::thread_time() - start;
        let start = crate::perf::thread_time();
        let many: f32 = points.iter().map(|p| get_2d(&n, p[0], p[2])).sum();
        let side_by_side = crate::perf::thread_time() - start;
        assert_eq!(one, many);
        println!("2D, 5 octaves: one octave at a time {:.1} ns per point, four at a time {:.1} ns", ns(scalar), ns(side_by_side));
    }
}

