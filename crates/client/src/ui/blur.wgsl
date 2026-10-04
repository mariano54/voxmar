// One pass of the blur behind menus: a Gaussian along one axis. Four passes (across, down,
// across, down) make a smooth, wide blur from few taps each.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct Blur {
    // The axis, in texels.
    direction: vec2<f32>,
    // Standard deviation in texels; the pass is skipped by the CPU when nothing is blurred.
    sigma: f32,
};

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> blur: Blur;

// Taps on each side of the centre. They spread out to reach 2.5 sigma, sampling between
// texels so that each bilinear fetch averages two.
const TAPS: i32 = 12;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = blur.direction / vec2<f32>(textureDimensions(screen));
    let sigma = max(blur.sigma, 0.01);
    let spacing = max(1.0, 2.5 * sigma / f32(TAPS));
    var sum = textureSample(screen, screen_sampler, in.uv);
    var total = 1.0;
    for (var i = 1; i <= TAPS; i++) {
        let x = f32(i) * spacing;
        let weight = exp(-x * x / (2.0 * sigma * sigma));
        let offset = texel * x;
        sum += (textureSample(screen, screen_sampler, in.uv + offset) + textureSample(screen, screen_sampler, in.uv - offset)) * weight;
        total += 2.0 * weight;
    }
    return sum / total;
}
