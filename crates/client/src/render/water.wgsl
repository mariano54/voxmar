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
