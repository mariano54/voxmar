// Display transform, run after bloom in place of a tonemapper. Colours up to KNEE pass
// through unchanged, so sunlit blocks show their texture colours as Minecraft does; brighter
// ones (the sun, glints, light sources) roll off smoothly towards white. This is the
// highlight compression of Khronos PBR Neutral, without its toe, which would crush night
// scenes to black.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct DisplayTransform {
    exposure: f32,
};

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: DisplayTransform;

const KNEE: f32 = 0.8;
const DESATURATION: f32 = 0.15;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let source = textureSample(screen, screen_sampler, in.uv);
    var color = max(source.rgb * settings.exposure, vec3(0.0));
    let peak = max(color.r, max(color.g, color.b));
    if peak > KNEE {
        let d = 1.0 - KNEE;
        let compressed = 1.0 - d * d / (peak + d - KNEE);
        color *= compressed / peak;
        let white = 1.0 - 1.0 / (DESATURATION * (peak - compressed) + 1.0);
        color = mix(color, vec3(compressed), white);
    }
    // Dither in (roughly) display space so dark gradients do not band at 8 bits.
    let noise = fract(vec3(dot(vec2(171.0, 231.0), in.position.xy)) / vec3(103.0, 71.0, 97.0)) - 0.5;
    color = pow(max(pow(color, vec3(1.0 / 2.2)) + noise / 255.0, vec3(0.0)), vec3(2.2));
    return vec4(color, source.a);
}
