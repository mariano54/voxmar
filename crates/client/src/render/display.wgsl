// Display transform, run in place of a tonemapper, after adding the bloom (bloom.wgsl, see
// post.rs) to the image. Colours up to KNEE pass
// through unchanged, so sunlit blocks show their texture colours as Minecraft does; brighter
// ones (the sun, glints, light sources) roll off smoothly towards white. This is the
// highlight compression of Khronos PBR Neutral, without its toe, which would crush night
// scenes to black. The shader pack's grading follows, on the display image, so it can
// shape contrast and colour without pushing anything past white.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct DisplayTransform {
    exposure: f32,
    contrast: f32,
    saturation: f32,
    vibrance: f32,
    warmth: f32,
    // How much of the bloom chain's top level replaces the image.
    bloom: f32,
};

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: DisplayTransform;
@group(0) @binding(3) var bloom: texture_2d<f32>;

const KNEE: f32 = 0.8;
const DESATURATION: f32 = 0.15;

const LUMINANCE = vec3(0.2126, 0.7152, 0.0722);
// Contrast pivots around this linear grey (about 46% in display terms).
const MID_GREY: f32 = 0.18;

// The pack's contrast, as an S-curve in display terms around mid grey: below it exactly a
// power of the linear value (as before), above it rolling into white instead of past it,
// so bright clouds and snow keep their shading rather than blowing out.
fn contrast(display: vec3<f32>) -> vec3<f32> {
    let pivot = pow(MID_GREY, 1.0 / 2.2);
    let p = pow(saturate(display), vec3(1.0 / 2.2));
    let k = settings.contrast;
    let low = pivot * pow(p / pivot, vec3(k));
    let high = 1.0 - (1.0 - pivot) * pow(max((1.0 - p) / (1.0 - pivot), vec3(0.0)), vec3(k));
    return pow(select(high, low, p < vec3(pivot)), vec3(2.2));
}

// The pack's colour grading on the display image: contrast, warmth that leaves highlights
// neutral (white clouds stay white, sunlit grass gets warmer), then saturation and
// vibrance, which lifts dull colours more than vivid ones.
fn grade(display: vec3<f32>) -> vec3<f32> {
    var color = contrast(display);
    let lift = settings.warmth * (1.0 - smoothstep(0.45, 0.9, dot(color, LUMINANCE)));
    color *= vec3(1.0 + lift, 1.0, 1.0 - lift);
    let luma = dot(color, LUMINANCE);
    let chroma = max(color.r, max(color.g, color.b)) - min(color.r, min(color.g, color.b));
    let boost = settings.saturation + settings.vibrance * (1.0 - saturate(chroma / max(luma, 1e-4)));
    return max(mix(vec3(luma), color, boost), vec3(0.0));
}

// The bloom chain's top level, upsampled with a 3x3 tent (as bloom.wgsl's `tent`).
fn bloom_at(uv: vec2<f32>) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(bloom));
    var sum = textureSample(bloom, screen_sampler, uv).rgb * 0.25;
    sum += (textureSample(bloom, screen_sampler, uv + vec2(0.0, t.y)).rgb + textureSample(bloom, screen_sampler, uv - vec2(t.x, 0.0)).rgb
        + textureSample(bloom, screen_sampler, uv + vec2(t.x, 0.0)).rgb + textureSample(bloom, screen_sampler, uv - vec2(0.0, t.y)).rgb) * 0.125;
    sum += (textureSample(bloom, screen_sampler, uv + vec2(-t.x, t.y)).rgb + textureSample(bloom, screen_sampler, uv + t).rgb
        + textureSample(bloom, screen_sampler, uv - t).rgb + textureSample(bloom, screen_sampler, uv + vec2(t.x, -t.y)).rgb) * 0.0625;
    return sum;
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let source = textureSample(screen, screen_sampler, in.uv);
    var hdr = source.rgb;
    if settings.bloom > 0.0 {
        hdr = mix(hdr, bloom_at(in.uv), settings.bloom);
    }
    var color = max(hdr * settings.exposure, vec3(0.0));
    let peak = max(color.r, max(color.g, color.b));
    if peak > KNEE {
        let d = 1.0 - KNEE;
        let compressed = 1.0 - d * d / (peak + d - KNEE);
        color *= compressed / peak;
        let white = 1.0 - 1.0 / (DESATURATION * (peak - compressed) + 1.0);
        color = mix(color, vec3(compressed), white);
    }
    color = grade(color);
    // Dither in (roughly) display space so dark gradients do not band at 8 bits.
    let noise = fract(vec3(dot(vec2(171.0, 231.0), in.position.xy)) / vec3(103.0, 71.0, 97.0)) - 0.5;
    color = pow(max(pow(color, vec3(1.0 / 2.2)) + noise / 255.0, vec3(0.0)), vec3(2.2));
    return vec4(color, source.a);
}
