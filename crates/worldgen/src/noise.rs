//! FastNoiseLite's OpenSimplex2, four at a time: 3D noise at four points, 2D fractal noise
//! and domain warp at four points, and the octaves of 2D fractal noise at one.
//!
//! Generation samples 3D noise on lattices of thousands of points per column (caves, rock),
//! and fractal 2D noise of up to five octaves everywhere (the land's continents, ranges and
//! hills, the climate), one call and one octave at a time. Each octave branches on which
//! corners of the simplex grids contribute, which no branch predictor can guess, and works
//! on one point while the core could work on four. Here the same arithmetic runs four wide,
//! in NEON vectors, with the branches turned into selects: across four points of a lattice
//! in 3D; across four points in 2D, where many are asked for together (the land under a
//! tile of river cells, say), each lane summing its own octaves; and across the octaves of
//! one point in 2D, which are independent (each octave scales the point by the lacunarity
//! and adds its value times an amplitude that, without FastNoiseLite's weighting, does not
//! depend on the octaves before it). The land's domain warp is progressive (each octave
//! warps what the octave before left), so it only runs four points wide. 3D gradient
//! components are all -1, 0 or 1, so their lookups are byte table lookups.
//!
//! The results are exactly FastNoiseLite's, bit for bit: every value comes from the same
//! operations in the same order, never fused (a contribution that a branch would skip adds
//! +0.0 instead, which changes nothing, as the sum is never -0.0), as the tests check
//! against the crate itself over many points and settings. A port of FastNoiseLite 1.1.1
//! (MIT, Jordan Peck and contributors; see THIRD_PARTY_NOTICES.md), for OpenSimplex2 with
//! no fractal, fractal Brownian motion or ridged fractal, and its gradient domain warp,
//! which is what generation uses; anything else, and other processors, go to the crate
//! one point at a time.

use fastnoise_lite::{DomainWarpType, FastNoiseLite, FractalType, NoiseType, RotationType3D};

/// Whether this module evaluates `noise` itself rather than leaving it to FastNoiseLite.
pub fn supported_3d(noise: &FastNoiseLite) -> bool {
    cfg!(target_arch = "aarch64")
        && noise.noise_type == NoiseType::OpenSimplex2
        && noise.rotation_type_3d == RotationType3D::None
        && matches!(noise.fractal_type, FractalType::None | FractalType::FBm)
        // Weighting would give each point its own amplitude.
        && noise.weighted_strength == 0.
}

/// Whether this module evaluates the 2D `noise` itself: fractal OpenSimplex2 of three
/// octaves or more (four side by side cost about as much as two one after the other, so
/// fewer gain nothing).
pub fn supported_2d(noise: &FastNoiseLite) -> bool {
    cfg!(target_arch = "aarch64")
        && noise.noise_type == NoiseType::OpenSimplex2
        && matches!(noise.fractal_type, FractalType::FBm | FractalType::Ridged)
        && noise.octaves >= 3
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

/// Whether this module evaluates the 2D `noise` itself four points at a time: OpenSimplex2,
/// plain or fractal (each lane sums its own octaves, so any number of octaves pays).
pub fn supported_2d_x4(noise: &FastNoiseLite) -> bool {
    cfg!(target_arch = "aarch64")
        && noise.noise_type == NoiseType::OpenSimplex2
        && matches!(noise.fractal_type, FractalType::None | FractalType::FBm | FractalType::Ridged)
        && noise.weighted_strength == 0.
}

/// `noise.get_noise_2d` at four points, side by side when it is `supported_2d_x4`.
#[inline]
pub fn get_2d_x4(noise: &FastNoiseLite, x: [f32; 4], y: [f32; 4]) -> [f32; 4] {
    #[cfg(target_arch = "aarch64")]
    if supported_2d_x4(noise) {
        // SAFETY: NEON is part of every aarch64 processor.
        return unsafe { neon::fractal_2d_x4(noise, x, y, fractal_bounding(noise)) };
    }
    std::array::from_fn(|k| noise.get_noise_2d(x[k], y[k]))
}

/// `get_2d` at the first `n` (1 ..= 4) of four points: one alone as `get_2d` works it out,
/// its octaves side by side, more a point per lane (`get_2d_x4`). Lanes past `n` hold
/// whatever.
#[inline]
pub fn get_2d_lanes(noise: &FastNoiseLite, x: [f32; 4], y: [f32; 4], n: usize) -> [f32; 4] {
    if n == 1 { [get_2d(noise, x[0], y[0]); 4] } else { get_2d_x4(noise, x, y) }
}

/// A sum of octaves of 2D OpenSimplex2 at four points, `Σ amplitude · noise(p · frequency)`
/// over `octaves` of (seed, frequency, amplitude): exactly what summing FastNoiseLite
/// objects of no fractal with those seeds and frequencies gives, octave after octave, in
/// one call. `None` where this module cannot evaluate it (not on aarch64).
#[inline]
pub fn octaves_2d_x4(octaves: &[(i32, f32, f32)], x: [f32; 4], y: [f32; 4]) -> Option<[f32; 4]> {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is part of every aarch64 processor.
        Some(unsafe { neon::octaves_2d_x4(octaves, x, y) })
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (octaves, x, y);
        None
    }
}

/// Whether this module warps with `warp` itself: OpenSimplex2 progressive domain warp,
/// which is what the land is warped by.
pub fn supported_warp_2d(warp: &FastNoiseLite) -> bool {
    cfg!(target_arch = "aarch64") && warp.domain_warp_type == DomainWarpType::OpenSimplex2 && warp.fractal_type == FractalType::DomainWarpProgressive
}

/// `warp.domain_warp_2d` at four points, side by side when it is `supported_warp_2d`.
#[inline]
pub fn warp_2d_x4(warp: &FastNoiseLite, x: [f32; 4], y: [f32; 4]) -> ([f32; 4], [f32; 4]) {
    #[cfg(target_arch = "aarch64")]
    if supported_warp_2d(warp) {
        // SAFETY: NEON is part of every aarch64 processor.
        return unsafe { neon::warp_2d_x4(warp, x, y, fractal_bounding(warp)) };
    }
    let warped: [(f32, f32); 4] = std::array::from_fn(|k| warp.domain_warp_2d(x[k], y[k]));
    (warped.map(|w| w.0), warped.map(|w| w.1))
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

/// FastNoiseLite's random unit vectors, as pairs, which the 2D domain warp scales by the
/// gradient's value.
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
#[rustfmt::skip]
#[allow(clippy::excessive_precision, clippy::unreadable_literal)]
const RAND_VECS_2D: [f32; 512] = [
    -0.2700222198, -0.9628540911, 0.3863092627, -0.9223693152, 0.04444859006, -0.999011673, -0.5992523158, -0.8005602176, -0.7819280288, 0.6233687174, 0.9464672271, 0.3227999196, -0.6514146797, -0.7587218957, 0.9378472289, 0.347048376,
    -0.8497875957, -0.5271252623, -0.879042592, 0.4767432447, -0.892300288, -0.4514423508, -0.379844434, -0.9250503802, -0.9951650832, 0.0982163789, 0.7724397808, -0.6350880136, 0.7573283322, -0.6530343002, -0.9928004525, -0.119780055,
    -0.0532665713, 0.9985803285, 0.9754253726, -0.2203300762, -0.7665018163, 0.6422421394, 0.991636706, 0.1290606184, -0.994696838, 0.1028503788, -0.5379205513, -0.84299554, 0.5022815471, -0.8647041387, 0.4559821461, -0.8899889226,
    -0.8659131224, -0.5001944266, 0.0879458407, -0.9961252577, -0.5051684983, 0.8630207346, 0.7753185226, -0.6315704146, -0.6921944612, 0.7217110418, -0.5191659449, -0.8546734591, 0.8978622882, -0.4402764035, -0.1706774107, 0.9853269617,
    -0.9353430106, -0.3537420705, -0.9992404798, 0.03896746794, -0.2882064021, -0.9575683108, -0.9663811329, 0.2571137995, -0.8759714238, -0.4823630009, -0.8303123018, -0.5572983775, 0.05110133755, -0.9986934731, -0.8558373281, -0.5172450752,
    0.09887025282, 0.9951003332, 0.9189016087, 0.3944867976, -0.2439375892, -0.9697909324, -0.8121409387, -0.5834613061, -0.9910431363, 0.1335421355, 0.8492423985, -0.5280031709, -0.9717838994, -0.2358729591, 0.9949457207, 0.1004142068,
    0.6241065508, -0.7813392434, 0.662910307, 0.7486988212, -0.7197418176, 0.6942418282, -0.8143370775, -0.5803922158, 0.104521054, -0.9945226741, -0.1065926113, -0.9943027784, 0.445799684, -0.8951327509, 0.105547406, 0.9944142724,
    -0.992790267, 0.1198644477, -0.8334366408, 0.552615025, 0.9115561563, -0.4111755999, 0.8285544909, -0.5599084351, 0.7217097654, -0.6921957921, 0.4940492677, -0.8694339084, -0.3652321272, -0.9309164803, -0.9696606758, 0.2444548501,
    0.08925509731, -0.996008799, 0.5354071276, -0.8445941083, -0.1053576186, 0.9944343981, -0.9890284586, 0.1477251101, 0.004856104961, 0.9999882091, 0.9885598478, 0.1508291331, 0.9286129562, -0.3710498316, -0.5832393863, -0.8123003252,
    0.3015207509, 0.9534596146, -0.9575110528, 0.2883965738, 0.9715802154, -0.2367105511, 0.229981792, 0.9731949318, 0.955763816, -0.2941352207, 0.740956116, 0.6715534485, -0.9971513787, -0.07542630764, 0.6905710663, -0.7232645452,
    -0.290713703, -0.9568100872, 0.5912777791, -0.8064679708, -0.9454592212, -0.325740481, 0.6664455681, 0.74555369, 0.6236134912, 0.7817328275, 0.9126993851, -0.4086316587, -0.8191762011, 0.5735419353, -0.8812745759, -0.4726046147,
    0.9953313627, 0.09651672651, 0.9855650846, -0.1692969699, -0.8495980887, 0.5274306472, 0.6174853946, -0.7865823463, 0.8508156371, 0.52546432, 0.9985032451, -0.05469249926, 0.1971371563, -0.9803759185, 0.6607855748, -0.7505747292,
    -0.03097494063, 0.9995201614, -0.6731660801, 0.739491331, -0.7195018362, -0.6944905383, 0.9727511689, 0.2318515979, 0.9997059088, -0.0242506907, 0.4421787429, -0.8969269532, 0.9981350961, -0.061043673, -0.9173660799, -0.3980445648,
    -0.8150056635, -0.5794529907, -0.8789331304, 0.4769450202, 0.0158605829, 0.999874213, -0.8095464474, 0.5870558317, -0.9165898907, -0.3998286786, -0.8023542565, 0.5968480938, -0.5176737917, 0.8555780767, -0.8154407307, -0.5788405779,
    0.4022010347, -0.9155513791, -0.9052556868, -0.4248672045, 0.7317445619, 0.6815789728, -0.5647632201, -0.8252529947, -0.8403276335, -0.5420788397, -0.9314281527, 0.363925262, 0.5238198472, 0.8518290719, 0.7432803869, -0.6689800195,
    -0.985371561, -0.1704197369, 0.4601468731, 0.88784281, 0.825855404, 0.5638819483, 0.6182366099, 0.7859920446, 0.8331502863, -0.553046653, 0.1500307506, 0.9886813308, -0.662330369, -0.7492119075, -0.668598664, 0.743623444,
    0.7025606278, 0.7116238924, -0.5419389763, -0.8404178401, -0.3388616456, 0.9408362159, 0.8331530315, 0.5530425174, -0.2989720662, -0.9542618632, 0.2638522993, 0.9645630949, 0.124108739, -0.9922686234, -0.7282649308, -0.6852956957,
    0.6962500149, 0.7177993569, -0.9183535368, 0.3957610156, -0.6326102274, -0.7744703352, -0.9331891859, -0.359385508, -0.1153779357, -0.9933216659, 0.9514974788, -0.3076565421, -0.08987977445, -0.9959526224, 0.6678496916, 0.7442961705,
    0.7952400393, -0.6062947138, -0.6462007402, -0.7631674805, -0.2733598753, 0.9619118351, 0.9669590226, -0.254931851, -0.9792894595, 0.2024651934, -0.5369502995, -0.8436138784, -0.270036471, -0.9628500944, -0.6400277131, 0.7683518247,
    -0.7854537493, -0.6189203566, 0.06005905383, -0.9981948257, -0.02455770378, 0.9996984141, -0.65983623, 0.751409442, -0.6253894466, -0.7803127835, -0.6210408851, -0.7837781695, 0.8348888491, 0.5504185768, -0.1592275245, 0.9872419133,
    0.8367622488, 0.5475663786, -0.8675753916, -0.4973056806, -0.2022662628, -0.9793305667, 0.9399189937, 0.3413975472, 0.9877404807, -0.1561049093, -0.9034455656, 0.4287028224, 0.1269804218, -0.9919052235, -0.3819600854, 0.924178821,
    0.9754625894, 0.2201652486, -0.3204015856, -0.9472818081, -0.9874760884, 0.1577687387, 0.02535348474, -0.9996785487, 0.4835130794, -0.8753371362, -0.2850799925, -0.9585037287, -0.06805516006, -0.99768156, -0.7885244045, -0.6150034663,
    0.3185392127, -0.9479096845, 0.8880043089, 0.4598351306, 0.6476921488, -0.7619021462, 0.9820241299, 0.1887554194, 0.9357275128, -0.3527237187, -0.8894895414, 0.4569555293, 0.7922791302, 0.6101588153, 0.7483818261, 0.6632681526,
    -0.7288929755, -0.6846276581, 0.8729032783, -0.4878932944, 0.8288345784, 0.5594937369, 0.08074567077, 0.9967347374, 0.9799148216, -0.1994165048, -0.580730673, -0.8140957471, -0.4700049791, -0.8826637636, 0.2409492979, 0.9705377045,
    0.9437816757, -0.3305694308, -0.8927998638, -0.4504535528, -0.8069622304, 0.5906030467, 0.06258973166, 0.9980393407, -0.9312597469, 0.3643559849, 0.5777449785, 0.8162173362, -0.3360095855, -0.941858566, 0.697932075, -0.7161639607,
    -0.002008157227, -0.9999979837, -0.1827294312, -0.9831632392, -0.6523911722, 0.7578824173, -0.4302626911, -0.9027037258, -0.9985126289, -0.05452091251, -0.01028102172, -0.9999471489, -0.4946071129, 0.8691166802, -0.2999350194, 0.9539596344,
    0.8165471961, 0.5772786819, 0.2697460475, 0.962931498, -0.7306287391, -0.6827749597, -0.7590952064, -0.6509796216, -0.907053853, 0.4210146171, -0.5104861064, -0.8598860013, 0.8613350597, 0.5080373165, 0.5007881595, -0.8655698812,
    -0.654158152, 0.7563577938, -0.8382755311, -0.545246856, 0.6940070834, 0.7199681717, 0.06950936031, 0.9975812994, 0.1702942185, -0.9853932612, 0.2695973274, 0.9629731466, 0.5519612192, -0.8338697815, 0.225657487, -0.9742067022,
    0.4215262855, -0.9068161835, 0.4881873305, -0.8727388672, -0.3683854996, -0.9296731273, -0.9825390578, 0.1860564427, 0.81256471, 0.5828709909, 0.3196460933, -0.9475370046, 0.9570913859, 0.2897862643, -0.6876655497, -0.7260276109,
    -0.9988770922, -0.047376731, -0.1250179027, 0.992154486, -0.8280133617, 0.560708367, 0.9324863769, -0.3612051451, 0.6394653183, 0.7688199442, -0.01623847064, -0.9998681473, -0.9955014666, -0.09474613458, -0.81453315, 0.580117012,
    0.4037327978, -0.9148769469, 0.9944263371, 0.1054336766, -0.1624711654, 0.9867132919, -0.9949487814, -0.100383875, -0.6995302564, 0.7146029809, 0.5263414922, -0.85027327, -0.5395221479, 0.841971408, 0.6579370318, 0.7530729462,
    0.01426758847, -0.9998982128, -0.6734383991, 0.7392433447, 0.639412098, -0.7688642071, 0.9211571421, 0.3891908523, -0.146637214, -0.9891903394, -0.782318098, 0.6228791163, -0.5039610839, -0.8637263605, -0.7743120191, -0.6328039957,
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

    use super::{GRADIENTS_2D, GRADIENTS_3D, RAND_VECS_2D};

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

    /// Four floats into a vector.
    #[target_feature(enable = "neon")]
    fn load(v: [f32; 4]) -> float32x4_t {
        // SAFETY: four floats.
        unsafe { vld1q_f32(v.as_ptr()) }
    }

    /// A vector's four floats.
    #[target_feature(enable = "neon")]
    fn store(v: float32x4_t) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        // SAFETY: `out` holds four floats.
        unsafe { vst1q_f32(out.as_mut_ptr(), v) };
        out
    }

    /// OpenSimplex2's skew of 2D points, after scaling them by the frequency.
    #[target_feature(enable = "neon")]
    #[allow(clippy::excessive_precision)]
    fn skew(x: float32x4_t, y: float32x4_t) -> (float32x4_t, float32x4_t) {
        let sqrt3: f32 = 1.7320508075688772935274463415059;
        let t = vmulq_f32(vaddq_f32(x, y), vdupq_n_f32(0.5 * (sqrt3 - 1.)));
        (vaddq_f32(x, t), vaddq_f32(y, t))
    }

    /// FastNoiseLite's `fast_floor`: truncated, less one below zero (even at whole numbers).
    #[target_feature(enable = "neon")]
    fn fast_floor(v: float32x4_t) -> int32x4_t {
        vsubq_s32(vcvtq_s32_f32(v), vreinterpretq_s32_u32(vandq_u32(vcltq_f32(v, vdupq_n_f32(0.)), vdupq_n_u32(1))))
    }

    /// One octave of 2D OpenSimplex2 (the same algorithm as ordinary simplex noise) at
    /// skewed, scaled points, each with its own seed.
    #[target_feature(enable = "neon")]
    pub(super) fn simplex_2d(seeds: [i32; 4], x: [f32; 4], y: [f32; 4]) -> [f32; 4] {
        // SAFETY: four integers.
        let seeds = unsafe { vld1q_s32(seeds.as_ptr()) };
        store(simplex_2d_v(seeds, load(x), load(y)))
    }

    /// `simplex_2d`, on vectors.
    #[target_feature(enable = "neon")]
    #[allow(clippy::excessive_precision)]
    fn simplex_2d_v(seeds: int32x4_t, x: float32x4_t, y: float32x4_t) -> float32x4_t {
        let sqrt3: f32 = 1.7320508075688772935274463415059;
        let g2 = (3. - sqrt3) / 6.;
        let zero = vdupq_n_f32(0.);
        let (i, j) = (fast_floor(x), fast_floor(y));
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
        vmulq_f32(vaddq_f32(vaddq_f32(n0, n1), n2), vdupq_n_f32(99.83685446303647))
    }

    /// Fractal 2D noise at four points, one point per lane, each summing its octaves in
    /// order as FastNoiseLite does (no fractal is one octave, unscaled).
    #[target_feature(enable = "neon")]
    pub(super) fn fractal_2d_x4(noise: &FastNoiseLite, x: [f32; 4], y: [f32; 4], bounding: f32) -> [f32; 4] {
        let frequency = vdupq_n_f32(noise.frequency);
        let (mut x, mut y) = skew(vmulq_f32(load(x), frequency), vmulq_f32(load(y), frequency));
        let (octaves, mut amp) = match noise.fractal_type {
            FractalType::FBm | FractalType::Ridged => (noise.octaves, bounding),
            _ => return store(simplex_2d_v(vdupq_n_s32(noise.seed), x, y)),
        };
        let ridged = noise.fractal_type == FractalType::Ridged;
        let (mut seed, mut sum) = (noise.seed, vdupq_n_f32(0.));
        let lacunarity = vdupq_n_f32(noise.lacunarity);
        for _ in 0..octaves {
            let value = simplex_2d_v(vdupq_n_s32(seed), x, y);
            seed = seed.wrapping_add(1);
            // FastNoiseLite also multiplies the amplitude by its weighting, which at
            // strength 0 is exactly 1.
            let value = if ridged { vaddq_f32(vmulq_f32(vabsq_f32(value), vdupq_n_f32(-2.)), vdupq_n_f32(1.)) } else { value };
            sum = vaddq_f32(sum, vmulq_f32(value, vdupq_n_f32(amp)));
            (x, y) = (vmulq_f32(x, lacunarity), vmulq_f32(y, lacunarity));
            amp *= noise.gain;
        }
        store(sum)
    }

    /// `octaves_2d_x4`: each octave scaled and skewed as FastNoiseLite does, then summed
    /// in order, multiplying and adding apart (never fused), as the scalar sum would.
    #[target_feature(enable = "neon")]
    pub(super) fn octaves_2d_x4(octaves: &[(i32, f32, f32)], x: [f32; 4], y: [f32; 4]) -> [f32; 4] {
        let (x, y) = (load(x), load(y));
        let mut sum = vdupq_n_f32(0.);
        for &(seed, frequency, amplitude) in octaves {
            let frequency = vdupq_n_f32(frequency);
            let (sx, sy) = skew(vmulq_f32(x, frequency), vmulq_f32(y, frequency));
            let value = simplex_2d_v(vdupq_n_s32(seed), sx, sy);
            sum = vaddq_f32(sum, vmulq_f32(vdupq_n_f32(amplitude), value));
        }
        store(sum)
    }

    /// Progressive domain warp at four points, one point per lane: each octave warps the
    /// points the octave before left, so octaves cannot run side by side, but points can.
    #[target_feature(enable = "neon")]
    #[allow(clippy::excessive_precision)]
    pub(super) fn warp_2d_x4(warp: &FastNoiseLite, x: [f32; 4], y: [f32; 4], bounding: f32) -> ([f32; 4], [f32; 4]) {
        let (mut x, mut y) = (load(x), load(y));
        let (mut seed, mut amp, mut frequency) = (warp.seed, warp.domain_warp_amp * bounding, warp.frequency);
        for _ in 0..warp.octaves {
            let (xs, ys) = skew(x, y);
            (x, y) = warp_simplex_2d(seed, amp * 38.283687591552734375, frequency, (xs, ys), (x, y));
            seed = seed.wrapping_add(1);
            amp *= warp.gain;
            frequency *= warp.lacunarity;
        }
        (store(x), store(y))
    }

    /// One octave of OpenSimplex2 gradient domain warp: the skewed points `s` scaled by the
    /// frequency pick the simplex, whose corners push the points `r` along random vectors
    /// by their gradients' values. Corners add up in FastNoiseLite's order: first, last,
    /// middle.
    #[target_feature(enable = "neon")]
    #[allow(clippy::excessive_precision)]
    fn warp_simplex_2d(
        seed: i32,
        amp: f32,
        frequency: f32,
        (x, y): (float32x4_t, float32x4_t),
        (xr, yr): (float32x4_t, float32x4_t),
    ) -> (float32x4_t, float32x4_t) {
        let sqrt3: f32 = 1.7320508075688772935274463415059;
        let g2 = (3. - sqrt3) / 6.;
        let zero = vdupq_n_f32(0.);
        let seeds = vdupq_n_s32(seed);
        let (x, y) = (vmulq_f32(x, vdupq_n_f32(frequency)), vmulq_f32(y, vdupq_n_f32(frequency)));
        let (i, j) = (fast_floor(x), fast_floor(y));
        let (xi, yi) = (vsubq_f32(x, vcvtq_f32_s32(i)), vsubq_f32(y, vcvtq_f32_s32(j)));
        let t = vmulq_f32(vaddq_f32(xi, yi), vdupq_n_f32(g2));
        let (x0, y0) = (vsubq_f32(xi, t), vsubq_f32(yi, t));
        let (i, j) = (vmulq_s32(i, vdupq_n_s32(PRIME_X)), vmulq_s32(j, vdupq_n_s32(PRIME_Y)));
        // A corner's push where its falloff `w` is positive, and +0 elsewhere.
        let push = |w: float32x4_t, i: int32x4_t, j: int32x4_t, dx: float32x4_t, dy: float32x4_t| {
            let w2 = vmulq_f32(w, w);
            let w4 = vmulq_f32(w2, w2);
            let (px, py) = gradient_dual_2d(seeds, i, j, dx, dy);
            let inside = vcgtq_f32(w, zero);
            (vbslq_f32(inside, vmulq_f32(w4, px), zero), vbslq_f32(inside, vmulq_f32(w4, py), zero))
        };
        let a = vsubq_f32(vsubq_f32(vdupq_n_f32(0.5), vmulq_f32(x0, x0)), vmulq_f32(y0, y0));
        let (ax, ay) = push(a, i, j, x0, y0);
        let c = vaddq_f32(
            vmulq_f32(vdupq_n_f32(2. * (1. - 2. * g2) * (1. / g2 - 2.)), t),
            vaddq_f32(vdupq_n_f32(-2. * (1. - 2. * g2) * (1. - 2. * g2)), a),
        );
        let (x2, y2) = (vaddq_f32(x0, vdupq_n_f32(2. * g2 - 1.)), vaddq_f32(y0, vdupq_n_f32(2. * g2 - 1.)));
        let (ip, jp) = (vaddq_s32(i, vdupq_n_s32(PRIME_X)), vaddq_s32(j, vdupq_n_s32(PRIME_Y)));
        let (cx, cy) = push(c, ip, jp, x2, y2);
        // The middle corner: towards y where the point lies above the diagonal.
        let upper = vcgtq_f32(y0, x0);
        let x1 = vaddq_f32(x0, vbslq_f32(upper, vdupq_n_f32(g2), vdupq_n_f32(g2 - 1.)));
        let y1 = vaddq_f32(y0, vbslq_f32(upper, vdupq_n_f32(g2 - 1.), vdupq_n_f32(g2)));
        let b = vsubq_f32(vsubq_f32(vdupq_n_f32(0.5), vmulq_f32(x1, x1)), vmulq_f32(y1, y1));
        let (bx, by) = push(b, vbslq_s32(upper, i, ip), vbslq_s32(upper, jp, j), x1, y1);
        let vx = vaddq_f32(vaddq_f32(vaddq_f32(zero, ax), cx), bx);
        let vy = vaddq_f32(vaddq_f32(vaddq_f32(zero, ay), cy), by);
        let amp = vdupq_n_f32(amp);
        (vaddq_f32(xr, vmulq_f32(vx, amp)), vaddq_f32(yr, vmulq_f32(vy, amp)))
    }

    /// FastNoiseLite's dual 2D gradient: the dot product of offsets with one gradient of
    /// each lane's corner, times another random vector of it.
    #[target_feature(enable = "neon")]
    fn gradient_dual_2d(seed: int32x4_t, i: int32x4_t, j: int32x4_t, dx: float32x4_t, dy: float32x4_t) -> (float32x4_t, float32x4_t) {
        let hash = vmulq_s32(veorq_s32(veorq_s32(seed, i), j), vdupq_n_s32(0x27d4eb2d));
        let (mut first, mut second) = ([0i32; 4], [0i32; 4]);
        // SAFETY: each array holds four integers.
        unsafe {
            vst1q_s32(first.as_mut_ptr(), vandq_s32(hash, vdupq_n_s32(127 << 1)));
            vst1q_s32(second.as_mut_ptr(), vandq_s32(vshrq_n_s32::<7>(hash), vdupq_n_s32(255 << 1)));
        }
        let (gx, gy) = (first.map(|o| GRADIENTS_2D[o as usize]), first.map(|o| GRADIENTS_2D[o as usize | 1]));
        let (ox, oy) = (second.map(|o| RAND_VECS_2D[o as usize]), second.map(|o| RAND_VECS_2D[o as usize | 1]));
        let value = vaddq_f32(vmulq_f32(dx, load(gx)), vmulq_f32(dy, load(gy)));
        (vmulq_f32(value, load(ox)), vmulq_f32(value, load(oy)))
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
            assert_eq!(supported_2d(&n), cfg!(target_arch = "aarch64") && octaves >= 3);
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

    /// Random points for the four-at-a-time checks: whole blocks, fractions and far reaches.
    fn points_2d(rng: &mut Rng, n: usize) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| match i % 4 {
                0 => (rng.unit() * 2e6 - 1e6, rng.unit() * 2e6 - 1e6),
                1 => (rng.unit() * 77.7 - 40.0, rng.unit() * 77.7 - 40.0),
                _ => (rng.range(-60_000, 60_000) as f32, rng.range(-60_000, 60_000) as f32 + 0.5 * (i % 3) as f32),
            })
            .collect()
    }

    /// Four points at a time, a point per lane, for every octave count and fractal the
    /// land, the climate and the shapes use, and others.
    #[test]
    fn noise_2d_x4_matches_fastnoise_lite_exactly() {
        let mut rng = Rng::new(9, 9, 9, 9);
        let mut checked = 0;
        for (salt, frequency, fractal, octaves) in [
            (2, 1.0 / 2800.0, FractalType::FBm, 5),
            (60, 1.0 / 7000.0, FractalType::FBm, 3),
            (61, 1.0 / 5000.0, FractalType::FBm, 2),
            (4, 1.0 / 520.0, FractalType::Ridged, 5),
            (62, 1.0 / 70.0, FractalType::Ridged, 3),
            (5, 0.008, FractalType::FBm, 4),
            (7, 1.0, FractalType::FBm, 1),
            (8, 0.37, FractalType::None, 1),
            (11, 1.3, FractalType::FBm, 8),
        ] {
            let n = noise(0x5eed ^ salt, salt as u32, frequency, fractal, octaves);
            assert_eq!(supported_2d_x4(&n), cfg!(target_arch = "aarch64"));
            let points = points_2d(&mut rng, 40_000);
            for four in points.chunks(4) {
                let (x, y) = (std::array::from_fn(|k| four[k].0), std::array::from_fn(|k| four[k].1));
                for (k, v) in get_2d_x4(&n, x, y).into_iter().enumerate() {
                    assert_eq!(v.to_bits(), n.get_noise_2d(x[k], y[k]).to_bits(), "({}, {}) at {frequency} {fractal:?} {octaves}", x[k], y[k]);
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 360_000);
    }

    /// The land's progressive domain warp too, at its own settings and others.
    #[test]
    fn octaves_2d_x4_match_summed_fastnoise_lite_exactly() {
        let mut rng = Rng::new(9, 9, 9, 9);
        let noises: Vec<(FastNoiseLite, f32)> = (0..7)
            .map(|i| {
                let mut n = FastNoiseLite::with_seed(1000 + i);
                n.set_noise_type(Some(NoiseType::OpenSimplex2));
                n.set_frequency(Some(2f32.powi(i) / 3000.0));
                (n, 0.37 / (i + 1) as f32)
            })
            .collect();
        let kernel: Vec<(i32, f32, f32)> = noises.iter().map(|(n, a)| (n.seed, n.frequency, *a)).collect();
        for p in points_2d(&mut rng, 4000).chunks(4) {
            let (x, y): ([f32; 4], [f32; 4]) = (std::array::from_fn(|k| p[k].0), std::array::from_fn(|k| p[k].1));
            let fast = super::octaves_2d_x4(&kernel, x, y).unwrap();
            for k in 0..4 {
                let mut sum = 0.0f32;
                for (n, a) in &noises {
                    sum += a * n.get_noise_2d(x[k], y[k]);
                }
                assert_eq!(fast[k].to_bits(), sum.to_bits(), "at {:?}", p[k]);
            }
        }
    }

    #[test]
    fn warp_2d_x4_matches_fastnoise_lite_exactly() {
        let mut rng = Rng::new(10, 10, 10, 10);
        for (salt, frequency, octaves, amplitude) in [(1, 0.0009, 3, 120.0), (3, 0.01, 1, 30.0), (5, 0.2, 5, 7.5)] {
            let mut warp = noise(0x5eed ^ salt, salt as u32, frequency, FractalType::DomainWarpProgressive, octaves);
            warp.set_domain_warp_type(Some(fastnoise_lite::DomainWarpType::OpenSimplex2));
            warp.set_domain_warp_amp(Some(amplitude));
            assert_eq!(supported_warp_2d(&warp), cfg!(target_arch = "aarch64"));
            let points = points_2d(&mut rng, 40_000);
            for four in points.chunks(4) {
                let (x, y) = (std::array::from_fn(|k| four[k].0), std::array::from_fn(|k| four[k].1));
                let (wx, wy) = warp_2d_x4(&warp, x, y);
                for k in 0..4 {
                    let (ex, ey) = warp.domain_warp_2d(x[k], y[k]);
                    assert_eq!((wx[k].to_bits(), wy[k].to_bits()), (ex.to_bits(), ey.to_bits()), "({}, {}) at {frequency} {octaves}", x[k], y[k]);
                }
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

