//! FastNoiseLite's 3D OpenSimplex2, four points at a time.
//!
//! Generation samples 3D noise on lattices of thousands of points per column (caves, rock),
//! one call at a time. Each call branches on which corners of the simplex grids contribute,
//! which no branch predictor can guess, and works on one point while the core could work on
//! four. Here the same arithmetic runs on four points in step, in NEON vectors, with the
//! branches turned into selects and the gradient lookups into byte table lookups (every 3D
//! gradient component is -1, 0 or 1).
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

    use super::GRADIENTS_3D;

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
        println!("one at a time {:.1} ns per point, four at a time {:.1} ns", ns(one), ns(many));
    }
}

