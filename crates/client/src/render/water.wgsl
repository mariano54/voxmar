// What water does to light, shared by real water (water_surface.wgsl) and the distant
// sheets (lod.wgsl) so the two meet in the same colour.
//
// Light fades through water by absorption, which takes red first, much less green and
// hardly any blue, as real water does, so shallows show their bed clearly in a cyan tint
// and only deep water turns blue; the biome's water colour shifts the absorption towards
// its hue. A little light is scattered back towards the eye, the colour of deep water.

#define_import_path voxmar::water

// Absorption of clear water per block (a block is a metre), and how much more of the
// channels the biome's water colour lacks it takes.
const PURE_WATER = vec3(0.15, 0.035, 0.018);
const TINT_ABSORPTION: f32 = 0.05;
// Light scattered back per block, as a fraction of the biome's water colour, and how much
// of that colour it keeps (deep water is a muted blue, not the colour's full saturation).
const SCATTERING: f32 = 0.01;
const SCATTER_SATURATION: f32 = 0.45;
// Reflectance of water seen straight on.
const F0: f32 = 0.02;

// Absorption per block of water whose colour is `tint`; `scale` is the shader pack's.
fn absorption(tint: vec3<f32>, scale: f32) -> vec3<f32> {
    let hue = tint / max(max(tint.r, tint.g), max(tint.b, 1e-3));
    return (PURE_WATER + (1.0 - hue) * TINT_ABSORPTION) * scale;
}

// Scattering per block of water whose colour is `tint`.
fn scattering(tint: vec3<f32>, scale: f32) -> vec3<f32> {
    let grey = dot(tint, vec3(0.2126, 0.7152, 0.0722));
    return mix(vec3(grey), tint, SCATTER_SATURATION) * SCATTERING * scale;
}

// Fraction of light left after `distance` blocks.
fn transmittance(absorbed: vec3<f32>, scattered: vec3<f32>, distance: f32) -> vec3<f32> {
    return exp(-(absorbed + scattered) * distance);
}

// Light the water itself sends to the eye along `distance` blocks lit by `light`: what
// scatters back, less what is lost on the way out.
fn inscattered(absorbed: vec3<f32>, scattered: vec3<f32>, distance: f32, light: vec3<f32>) -> vec3<f32> {
    let extinction = absorbed + scattered;
    return light * scattered / extinction * (1.0 - exp(-extinction * distance));
}

// How much light water reflects: Schlick's approximation with a softer falloff (a cube in
// place of the fifth power), as stylised packs do, so water seen from a hiker's height
// carries a visible sheen of sky and waves instead of turning glass-clear.
fn fresnel(cos_theta: f32) -> f32 {
    let c = 1.0 - saturate(cos_theta);
    return F0 + (1.0 - F0) * c * c * c;
}

// The sky's colour along `direction`, as sky.wgsl paints it without clouds or stars:
// for reflections that leave the screen.
fn sky_gradient(direction: vec3<f32>, zenith: vec3<f32>, horizon: vec3<f32>) -> vec3<f32> {
    if direction.y < 0.0 {
        return horizon * mix(1.0, 0.7, min(-direction.y * 4.0, 1.0));
    }
    return mix(horizon, zenith, pow(direction.y, 0.55));
}

// The water pass's uniform (water.rs, `WaterUniform`), which the distant terrain reads too,
// so its sheets carry real water's waves.
struct Water {
    // Wave layers (water/waves.rs): cos and sin of the layer's turn, 1 / its size in
    // blocks, and its weight.
    layers: array<vec4<f32>, 4>,
    // xy: the texture coordinates of `origin` in each layer.
    phases: array<vec4<f32>, 4>,
    // xz: where wave positions are measured from; y: how windy it is, 0..1; w: how high the
    // waves are on open water (the pack's strength times the wind's).
    origin: vec4<f32>,
    // xz: where flowing water's waves are measured from (snapped more coarsely).
    flow_origin: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // x: absorption, y: scattering, z: refraction (the pack's), w: 1 with the eye under water
    optics: vec4<f32>,
    // x: reflections, y: foam (the pack's), z: 1 for pixelated water, w: waves (the pack's)
    strengths: vec4<f32>,
    // x: wave layers, y: steps of the reflection march, z: halvings that refine a hit
    budget: vec4<u32>,
};

// Texels along a side of the wave texture, and the waves' steepest slope at full height.
const WAVE_TEXELS: f32 = 256.0;
const WAVE_SLOPE: f32 = 0.16;
// Texels per block of the block textures, for pixelated water.
const BLOCK_TEXELS: f32 = 16.0;
// Each wave layer's strength wanders between these over broad patches this many of the
// layer's own sizes across, so no layer repeats evenly across open water.
const PATCHES = vec2(0.7, 1.3);
const PATCH_SIZE: f32 = 9.0;


// The slope of the wind's waves on still water at `xz` (d height / d xz), at full height,
// from the first `count` layers of `waves` (water/waves.rs). `footprint` is the blocks a
// pixel spans there, which picks each layer's mip, so waves smaller than a pixel average
// out instead of sparkling, alike near and far. Each layer's strength wanders over broad
// patches, so the layers' repeats do not show as a pattern across the sea. Pixelated water
// (a shader pack's choice, after Complementary's Reimagined style) takes one slope per texel
// of the block textures.
fn wind_wave_slope(waves: texture_2d<f32>, wave_sampler: sampler, water: Water, xz: vec2<f32>, footprint: f32, count: u32) -> vec2<f32> {
    var at = xz;
    if water.strengths.z > 0.5 && footprint < 1.0 / BLOCK_TEXELS {
        at = (floor(xz * BLOCK_TEXELS) + 0.5) / BLOCK_TEXELS;
    }
    let p = at - water.origin.xz;
    var slope = vec2(0.0);
    var weights = 0.0;
    for (var i = 0u; i < count; i++) {
        let layer = water.layers[i];
        weights += layer.w;
        // A pixel spanning a whole wave of the layer sees it average out flat.
        if footprint * layer.z >= 1.0 {
            continue;
        }
        let turned = vec2(layer.x * p.x - layer.y * p.y, layer.y * p.x + layer.x * p.y);
        let uv = turned * layer.z + water.phases[i].xy;
        let g = textureSampleLevel(waves, wave_sampler, uv, log2(max(footprint * layer.z * WAVE_TEXELS, 1.0))).xy;
        let strength = mix(PATCHES.x, PATCHES.y, value_noise_01(at * (layer.z / PATCH_SIZE) + f32(i) * 17.3));
        // Back from the layer's turned axes to x and z.
        slope += vec2(layer.x * g.x + layer.y * g.y, layer.x * g.y - layer.y * g.x) * layer.w * strength;
    }
    return slope * (WAVE_SLOPE / max(weights, 1e-3));
}

// Blocks across a pixel over which the sky and clouds a surface mirrors stop following the
// waves: far off each pixel spans many waves, whose mirrored sky would average to a soft
// gloss, but sampled once a pixel it broke into a dither of cloud and no cloud.
const GLOSS_FOOTPRINT = vec2(0.06, 0.6);

// The direction to look up the mirrored sky and clouds in, from the waves' normal `waves`
// and the flat surface's `normal`, for a pixel spanning `footprint` blocks.
fn mirrored_sky(to_eye: vec3<f32>, waves: vec3<f32>, normal: vec3<f32>, footprint: f32) -> vec3<f32> {
    let n = normalize(mix(waves, normal, smoothstep(GLOSS_FOOTPRINT.x, GLOSS_FOOTPRINT.y, footprint)));
    var r = reflect(-to_eye, n);
    let rise = dot(r, normal);
    if rise < 0.02 {
        r = normalize(r + normal * (0.02 - rise));
    }
    return r;
}

// Smooth value noise in 0..1 at `p`.
fn value_noise_01(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash_01(i);
    let b = hash_01(i + vec2(1.0, 0.0));
    let c = hash_01(i + vec2(0.0, 1.0));
    let d = hash_01(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn hash_01(c: vec2<f32>) -> f32 {
    return fract(sin(dot(c, vec2(127.1, 311.7))) * 43758.5453);
}
