// Noise for clouds and ground fog: the body of cloud, the billows at its edges and 3D noise,
// from integer hashes on lattices that repeat every 256 cells (2D) and 64 cells (3D), far
// wider than anything they shape. The volumetric pass, which samples them most, reads the
// same lattices from two small textures (render/volumetric.rs, NOISE_TEXTURES) instead of
// hashing, so the cloud it draws and the shadows the terrain takes stay one. The low cloud
// is a layer in the world (atmosphere.wgsl, `cloud_density`); the sky draws the higher
// sheet and cirrus.

#define_import_path voxmar::clouds

// Cells before each lattice repeats (render::volumetric::NOISE_2D and NOISE_3D).
const NOISE_2D: i32 = 256;
const NOISE_3D: i32 = 64;

// Integer hash (PCG), free of the banding a sine hash shows at large inputs.
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// For whole-numbered `p`, 0..1.
fn hash2(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(p) & vec2(NOISE_2D - 1);
    return unit(pcg(u32(i.x) + pcg(u32(i.y) ^ 0x9e3779b9u)));
}

// For whole-numbered `p`, 0..1.
fn hash3(p: vec3<i32>) -> f32 {
    let i = p & vec3(NOISE_3D - 1);
    return unit(pcg(u32(i.x) + pcg(u32(i.y) + pcg(u32(i.z) ^ 0x2545f491u))));
}

#ifdef NOISE_TEXTURES
@group(2) @binding(2) var noise_2d: texture_2d<f32>;
@group(2) @binding(3) var noise_3d: texture_3d<f32>;
@group(2) @binding(4) var noise_sampler: sampler;

// Value noise, 0..1: the lattice's texels blended by the filter at the smoothed fraction.
fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return textureSampleLevel(noise_2d, noise_sampler, (i + u + 0.5) / f32(NOISE_2D), 0.0).r;
}

// 3D value noise, 0..1, for billows and ground fog.
fn value_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return textureSampleLevel(noise_3d, noise_sampler, (i + u + 0.5) / f32(NOISE_3D), 0.0).r;
}
#else
// Value noise, 0..1.
fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2(1.0, 0.0));
    let c = hash2(i + vec2(0.0, 1.0));
    let d = hash2(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 3D value noise, 0..1, for billows and ground fog.
fn value_noise3(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x00 = mix(hash3(i), hash3(i + vec3(1, 0, 0)), u.x);
    let x10 = mix(hash3(i + vec3(0, 1, 0)), hash3(i + vec3(1, 1, 0)), u.x);
    let x01 = mix(hash3(i + vec3(0, 0, 1)), hash3(i + vec3(1, 0, 1)), u.x);
    let x11 = mix(hash3(i + vec3(0, 1, 1)), hash3(i + vec3(1, 1, 1)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}
#endif

// The broad body of low cloud, about 0..1: three coarse octaves over a smooth warp, so
// outlines bulge and curl instead of following the noise grid. The finer octaves slide
// slowly over the coarse one as seconds `t` pass, so shapes form and break up as they drift.
fn cloud_body(p: vec2<f32>, t: f32) -> f32 {
    let warp = vec2(value_noise(p * 0.5 + vec2(5.2, 1.3)), value_noise(p * 0.5 + vec2(-3.7, 8.1))) - 0.5;
    var q = p + warp * 0.8;
    var value = 0.0;
    var amplitude = 0.5;
    for (var i = 0; i < 3; i++) {
        value += value_noise(q) * amplitude;
        q = q * 2.03 + vec2(17.1, 9.3) + t * vec2(0.006, -0.004);
        amplitude *= 0.5;
    }
    return value / 0.875;
}

// The body's value above which there is cloud, for a fraction `cover` of the sky.
fn cloud_threshold(cover: f32) -> f32 {
    return mix(0.72, 0.0, pow(cover, 1.3));
}

// Billows and wisps, 0..1, that shape only the edges of cloud.
fn cloud_detail(p: vec2<f32>, t: f32) -> f32 {
    let q = p + t * vec2(0.01, -0.007);
    return value_noise(q) * 0.65 + value_noise(q * 2.07 + vec2(7.0, 3.0)) * 0.35;
}
