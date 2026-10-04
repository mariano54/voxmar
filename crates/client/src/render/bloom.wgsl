// Bloom (see post.rs): what is brighter than a sunlit white block glows. The image is
// downsampled into a chain of ever smaller levels, then upsampled back up the chain, each
// level blending the blurrier one below into itself; the display pass adds the top of the
// chain to the image. This is the method of Call of Duty: Advanced Warfare (Jimenez,
// "Next Generation Post Processing in Call of Duty", 2014) as Bevy's bloom implements it
// (MIT or Apache-2.0), with the same filters, threshold and blend, done by compute
// dispatches in one pass instead of a render pass per level.

struct Step {
    // The soft threshold, precomputed: threshold, threshold - knee, 2 * knee and
    // 0.25 / knee (Catlike Coding's bloom tutorial).
    threshold: vec4<f32>,
    // How much of the blurrier level an upsample blends into this one.
    blend: f32,
}

// The level read (the image, for the first downsample).
@group(0) @binding(0) var input: texture_2d<f32>;
@group(0) @binding(1) var filtering: sampler;
// For upsamples, this level as the downsampling left it.
@group(0) @binding(2) var detail: texture_2d<f32>;
@group(0) @binding(3) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<uniform> params: Step;

const LUMINANCE = vec3(0.2126, 0.7152, 0.0722);

fn soft_threshold(color: vec3<f32>) -> vec3<f32> {
    let brightness = max(color.r, max(color.g, color.b));
    var softness = clamp(brightness - params.threshold.y, 0.0, params.threshold.z);
    softness = softness * softness * params.threshold.w;
    let contribution = max(brightness - params.threshold.x, softness) / max(brightness, 0.00001);
    return color * contribution;
}

// Brian Karis's weight, which keeps single very bright pixels from blooming into blobs.
fn karis(color: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + dot(color, LUMINANCE) / 4.0);
}

fn tap(uv: vec2<f32>, offset: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(input, filtering, uv + offset * texel, 0.0).rgb;
}

// The 13-tap downsample (Jimenez's slide 153), in texels of the level read.
fn downsample_13(uv: vec2<f32>, first: bool) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(input));
    let a = tap(uv, vec2(-2.0, 2.0), texel);
    let b = tap(uv, vec2(0.0, 2.0), texel);
    let c = tap(uv, vec2(2.0, 2.0), texel);
    let d = tap(uv, vec2(-2.0, 0.0), texel);
    let e = tap(uv, vec2(0.0, 0.0), texel);
    let f = tap(uv, vec2(2.0, 0.0), texel);
    let g = tap(uv, vec2(-2.0, -2.0), texel);
    let h = tap(uv, vec2(0.0, -2.0), texel);
    let i = tap(uv, vec2(2.0, -2.0), texel);
    let j = tap(uv, vec2(-1.0, 1.0), texel);
    let k = tap(uv, vec2(1.0, 1.0), texel);
    let l = tap(uv, vec2(-1.0, -1.0), texel);
    let m = tap(uv, vec2(1.0, -1.0), texel);
    if first {
        let g0 = (a + b + d + e) * (0.125 / 4.0);
        let g1 = (b + c + e + f) * (0.125 / 4.0);
        let g2 = (d + e + g + h) * (0.125 / 4.0);
        let g3 = (e + f + h + i) * (0.125 / 4.0);
        let g4 = (j + k + l + m) * (0.5 / 4.0);
        return g0 * karis(g0) + g1 * karis(g1) + g2 * karis(g2) + g3 * karis(g3) + g4 * karis(g4);
    }
    return (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (e + j + k + l + m) * 0.125;
}

// The 3x3 tent upsample (slide 162), in texels of the level read.
fn tent(uv: vec2<f32>) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(input));
    var sum = tap(uv, vec2(0.0, 0.0), texel) * 0.25;
    sum += (tap(uv, vec2(0.0, 1.0), texel) + tap(uv, vec2(-1.0, 0.0), texel) + tap(uv, vec2(1.0, 0.0), texel) + tap(uv, vec2(0.0, -1.0), texel)) * 0.125;
    sum += (tap(uv, vec2(-1.0, 1.0), texel) + tap(uv, vec2(1.0, 1.0), texel) + tap(uv, vec2(-1.0, -1.0), texel) + tap(uv, vec2(1.0, -1.0), texel)) * 0.0625;
    return sum;
}

// The texel this invocation writes, and its centre in uv, or nothing past the level's edge.
fn texel_at(id: vec2<u32>) -> vec4<f32> {
    let size = textureDimensions(output);
    if any(id >= size) {
        return vec4(-1.0);
    }
    return vec4((vec2<f32>(id) + 0.5) / vec2<f32>(size), vec2<f32>(id));
}

@compute @workgroup_size(8, 8, 1)
fn downsample_first(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = texel_at(id.xy);
    if at.x < 0.0 {
        return;
    }
    // The bounds keep a black image from turning blocks of the chain to black, and a
    // blinding one from overflowing.
    let color = clamp(downsample_13(at.xy, true), vec3(0.0001), vec3(3.4e37));
    textureStore(output, id.xy, vec4(soft_threshold(color), 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = texel_at(id.xy);
    if at.x < 0.0 {
        return;
    }
    textureStore(output, id.xy, vec4(downsample_13(at.xy, false), 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn upsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = texel_at(id.xy);
    if at.x < 0.0 {
        return;
    }
    let here = textureLoad(detail, id.xy, 0).rgb;
    textureStore(output, id.xy, vec4(mix(here, tent(at.xy), params.blend), 1.0));
}
