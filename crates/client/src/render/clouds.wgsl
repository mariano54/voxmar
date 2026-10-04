// The low cloud's body, shared by the sky, which draws it (sky.wgsl), and the terrain and
// volumetric light, which it shades: a cloud passing in front of the sun casts its shadow
// on the land and through the air below.
//
// The sky draws the layer by direction, `at = 2 · dir.xz / dir.y + offset` (with a little
// bending towards the horizon), with the offset moving 1 per CLOUD_UNIT blocks the camera
// moves: that is a plane LAYER_HEIGHT = 2 · CLOUD_UNIT blocks above the eye, so a point's
// cloud is where the ray from it towards the sun meets that plane.

#define_import_path voxmar::clouds

// Feature size of the low cloud's body, relative to the layer's coordinates.
const CLOUD_SCALE: f32 = 0.6;

// Integer hash (PCG), free of the banding a sine hash shows at large inputs.
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// For whole-numbered `p`. An integer hash stays random however far the clouds drift.
fn hash2(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(p);
    return unit(pcg(u32(i.x) + pcg(u32(i.y) ^ 0x9e3779b9u)));
}

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

// How opaque the low cloud's body is at layer coordinates `at`, without the billows at its
// edges (too fine to show in a shadow).
fn cloud_opacity(at: vec2<f32>, t: f32, cover: f32, convection: f32) -> f32 {
    if cover < 0.005 {
        return 0.0;
    }
    let threshold = cloud_threshold(cover);
    let core = (cloud_body(at * CLOUD_SCALE, t) - threshold) / (1.0 - threshold);
    return smoothstep(0.0, mix(0.2, 0.1, convection), core) * smoothstep(0.0, 0.1, cover);
}
