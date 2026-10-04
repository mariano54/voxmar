// What water does to light, shared by real water (water_surface.wgsl) and the distant
// sheets (lod.wgsl) so the two meet in the same colour.
//
// Light fades through water by absorption, which takes red first, then green, so shallows
// are nearly clear, a few blocks take the water's colour and deep water is dark; a little
// is scattered back towards the eye, the colour of deep water. Both follow the biome's
// water colour: the absorption is what turns white light into that colour over a few
// blocks.

#define_import_path voxmar::water

// Blocks of water that turn white light into the biome's water colour.
const COLOR_DEPTH: f32 = 6.0;
// Absorption of clear water itself, per block, on top of the colour's.
const CLEAR_ABSORPTION = vec3(0.045, 0.012, 0.006);
// Light scattered back per block, as a fraction of the biome's water colour, and how much
// of that colour it keeps (deep water is a muted blue, not the colour's full saturation).
const SCATTERING: f32 = 0.012;
const SCATTER_SATURATION: f32 = 0.45;
// Reflectance of water seen straight on.
const F0: f32 = 0.02;

// Absorption per block of water whose colour is `tint`; `scale` is the shader pack's.
fn absorption(tint: vec3<f32>, scale: f32) -> vec3<f32> {
    return (-log(clamp(tint, vec3(0.02), vec3(1.0))) / COLOR_DEPTH + CLEAR_ABSORPTION) * scale;
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

// Schlick's approximation of how much light water reflects.
fn fresnel(cos_theta: f32) -> f32 {
    return F0 + (1.0 - F0) * pow(1.0 - saturate(cos_theta), 5.0);
}

// The sky's colour along `direction`, as sky.wgsl paints it without clouds or stars:
// for reflections that leave the screen.
fn sky_gradient(direction: vec3<f32>, zenith: vec3<f32>, horizon: vec3<f32>) -> vec3<f32> {
    if direction.y < 0.0 {
        return horizon * mix(1.0, 0.7, min(-direction.y * 4.0, 1.0));
    }
    return mix(horizon, zenith, pow(direction.y, 0.55));
}
