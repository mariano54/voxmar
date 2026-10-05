// Particles (see particles.rs): one instance per slot of the ring, six vertices each. Every
// particle's place, size and fade follow from where and when it started, so nothing about
// it changes after it is spawned.

#import bevy_pbr::mesh_view_bindings::{view, globals}
#import voxmar::lighting::{shade_normal, apply_fog}

struct Particle {
    origin: vec3<f32>,
    born: f32,
    velocity: vec3<f32>,
    floor: f32,
    size: f32,
    life: f32,
    gravity: f32,
    drag: f32,
    // layer:12 | free:12 | block light:4 | sky light:4
    surface: u32,
    // corner u:8 | corner v:8 | span:8 (256ths of the texture) | motion:4 | flags:4
    sprite: u32,
    // sRGB colour and opacity
    color: u32,
    seed: u32,
};

@group(2) @binding(0) var block_textures: texture_2d_array<f32>;
@group(2) @binding(1) var block_sampler: sampler;
@group(2) @binding(2) var<storage, read> particles: array<Particle>;
// Each texture layer's overlay (render/mod.rs, `layer_table`; voxel.wgsl draws it alike).
@group(2) @binding(7) var block_layers: texture_2d<f32>;

// globals.time wraps every hour.
const TIME_WRAP: f32 = 3600.0;
const MOTION_BALLISTIC: u32 = 0u;
const FLAG_TEXTURED: u32 = 1u;
const FLAG_GLOW: u32 = 2u;
// Of the speed a particle hits the floor with, how much it hops back up.
const BOUNCE: f32 = 0.35;
// How quickly a particle that has touched the floor slows, per second.
const GROUND_DRAG: f32 = 8.0;
// Seconds a particle takes to fade out at the end of its life (at most half of it).
const FADE: f32 = 0.35;
// Brightness of a glowing particle against a sunlit block.
const GLOW: f32 = 2.5;

const CORNERS = array<vec2<f32>, 6>(
    vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
    vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
);

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // -1..1 across the quad.
    @location(2) corner: vec2<f32>,
    @location(3) @interpolate(flat) surface: u32,
    @location(4) @interpolate(flat) flags: u32,
    // Linear colour, and opacity times how far it has faded.
    @location(5) @interpolate(flat) color: vec4<f32>,
};

fn age_of(p: Particle) -> f32 {
    var age = globals.time - p.born;
    if age < -1.0 {
        age += TIME_WRAP;
    }
    return max(age, 0.0);
}

// Thrown, falling and slowed by drag, then hopping on the floor a few times, lower each
// time, sliding to rest. Drag only acts across: a fall of a block or two hardly feels it,
// and it keeps the landing time exact.
fn ballistic(p: Particle, age: f32) -> vec3<f32> {
    let g = max(p.gravity, 0.001);
    let k = max(p.drag, 0.001);
    let v = p.velocity;
    let floor = min(p.floor, p.origin.y);
    let land = (v.y + sqrt(v.y * v.y + 2.0 * g * (p.origin.y - floor))) / g;
    let t = min(age, land);
    var across = p.origin.xz + v.xz * (1.0 - exp(-k * t)) / k;
    var y = p.origin.y + v.y * t - 0.5 * g * t * t;
    if age > land {
        let since = age - land;
        across += v.xz * exp(-k * land) * (1.0 - exp(-GROUND_DRAG * since)) / GROUND_DRAG;
        var speed = (g * land - v.y) * BOUNCE;
        var hop = since;
        y = floor;
        for (var i = 0; i < 3; i++) {
            let airborne = 2.0 * speed / g;
            if hop < airborne {
                y = floor + speed * hop - 0.5 * g * hop * hop;
                break;
            }
            hop -= airborne;
            speed *= BOUNCE;
        }
    }
    return vec3(across.x, y, across.y);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let p = particles[instance_index];
    let age = age_of(p);
    // Dead or never written: every vertex at one point, so nothing is drawn.
    if age >= p.life {
        out.clip_position = vec4(0.0, 0.0, 0.0, 1.0);
        return out;
    }
    var center = p.origin;
    let motion = (p.sprite >> 24u) & 15u;
    if motion == MOTION_BALLISTIC {
        center = ballistic(p, age);
    }
    let fade = 1.0 - smoothstep(p.life - min(FADE, 0.5 * p.life), p.life, age);
    let corner = CORNERS[vertex_index];
    // Facing the camera, and shrinking a little as it fades.
    let half = 0.5 * p.size * mix(0.7, 1.0, fade);
    let right = view.world_from_view[0].xyz;
    let up = view.world_from_view[1].xyz;
    out.world_position = center + (right * corner.x + up * corner.y) * half;
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.corner = corner;
    let piece = vec2(f32(p.sprite & 255u), f32((p.sprite >> 8u) & 255u)) / 256.0;
    let span = f32((p.sprite >> 16u) & 255u) / 256.0;
    out.uv = piece + vec2(corner.x * 0.5 + 0.5, 0.5 - corner.y * 0.5) * span;
    out.surface = p.surface;
    out.flags = p.sprite >> 28u;
    let color = unpack4x8unorm(p.color);
    out.color = vec4(srgb_to_linear(color.rgb), color.a * fade);
    return out;
}

// Interleaved gradient noise, for fading by discarding pixels without MSAA.
fn dither(xy: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(xy, vec2(0.06711056, 0.00583715))));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let textured = (in.flags & FLAG_TEXTURED) != 0u;
    let layer = i32(in.surface & 0xFFFu);
    let texel = textureSample(block_textures, block_sampler, in.uv, layer);
    let uv_dx = dpdx(in.uv);
    let uv_dy = dpdy(in.uv);
    // A texture piece is cut out where its texels are; a speck is a soft disc.
    var coverage = 1.0 - smoothstep(0.55, 1.0, length(in.corner));
    var albedo = in.color.rgb;
    if textured {
        coverage = texel.a;
        let entry = textureLoad(block_layers, vec2(layer, 0), 0);
        let overlay = u32(entry.r * 255.0 + 0.5) | (u32(entry.g * 255.0 + 0.5) << 8u);
        if overlay != 0u {
            // The colour is the overlay's: a grass block's side keeps its dirt.
            let over = textureSampleGrad(block_textures, block_sampler, in.uv, i32(overlay - 1u), uv_dx, uv_dy);
            albedo = mix(texel.rgb, over.rgb * albedo, over.a);
        } else {
            albedo *= texel.rgb;
        }
    }
#ifdef ALPHA_TO_COVERAGE
    // Sharpened so texel edges stay crisp; the fade spreads over the samples.
    let alpha = saturate((coverage - 0.5) / max(fwidth(coverage), 0.0001) + 0.5) * in.color.a;
    if alpha <= 0.0 {
        discard;
    }
#else
    let alpha = 1.0;
    if coverage < 0.5 || in.color.a < dither(in.clip_position.xy) {
        discard;
    }
#endif
    var color = albedo * GLOW;
    if (in.flags & FLAG_GLOW) == 0u {
        let sky = f32(in.surface >> 28u) / 15.0;
        let block = f32((in.surface >> 24u) & 15u) / 15.0;
        // Lit like a plant: no face is turned away from the sun.
        color = shade_normal(albedo, in.world_position, vec3(0.0, 1.0, 0.0), 0.85, sky, block, 1.0, true, in.clip_position.xy);
    }
    return vec4(apply_fog(color, in.world_position), alpha);
}
