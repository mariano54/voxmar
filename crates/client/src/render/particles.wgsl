// Particles (see particles.rs): one instance per slot of the ring, six vertices each. Every
// particle's place, size and fade follow from where and when it started, so nothing about
// it changes after it is spawned.

#import bevy_pbr::mesh_view_bindings::{view, globals}
#import bevy_pbr::view_transformations::{depth_ndc_to_view_z, position_world_to_view}
#import voxmar::lighting::{shade_normal, apply_fog, air_between}

struct Particle {
    origin: vec3<f32>,
    born: f32,
    velocity: vec3<f32>,
    floor: f32,
    size: f32,
    life: f32,
    gravity: f32,
    drag: f32,
    // layer:12 | grow:6 (eighths) | wander:6 (hundredths of a block) | block light:4 |
    // sky light:4
    surface: u32,
    // corner u:8 | corner v:8 | span:8 (256ths of the texture) | motion:2 | blend:2 |
    // flags:4
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

#ifdef BLENDED
// The scene's depth, which blended particles fade against (particles.rs, `blended_pass`).
#ifdef DEPTH_MULTISAMPLED
@group(3) @binding(0) var scene_depth: texture_depth_multisampled_2d;
#else
@group(3) @binding(0) var scene_depth: texture_depth_2d;
#endif
#endif

// globals.time wraps every hour.
const TIME_WRAP: f32 = 3600.0;
const MOTION_BALLISTIC: u32 = 0u;
const MOTION_RISE: u32 = 1u;
const FLAG_TEXTURED: u32 = 1u;
const FLAG_GLOW: u32 = 2u;
const FLAG_COOLS: u32 = 4u;
// How a particle is drawn (particles.rs, `blend_bits`): cut out, adding light, or a veil.
const BLEND_CUTOUT: u32 = 0u;
const BLEND_LIGHT: u32 = 1u;
const BLEND_SOFT: u32 = 2u;
// The blend this pipeline draws.
#ifdef BLEND_ADD
const DRAWN: u32 = BLEND_LIGHT;
#else ifdef BLEND_VEIL
const DRAWN: u32 = BLEND_SOFT;
#else
const DRAWN: u32 = BLEND_CUTOUT;
#endif
// Blocks over which a blended particle fades out as it nears whatever is behind it.
const SOFT_DEPTH: f32 = 0.4;
// What a cooling particle's colour is multiplied by at the end of its life: dimmer, and
// redder, as blue and green go first.
const COOLED: vec3<f32> = vec3(0.5, 0.2, 0.05);
// A flame's white-hot core, and what its edge is multiplied by: hue-shifted towards red.
const CORE: vec3<f32> = vec3(1.0, 0.86, 0.55);
const EDGE: vec3<f32> = vec3(0.75, 0.32, 0.16);
// Of the speed a particle hits the floor with, how much it hops back up.
const BOUNCE: f32 = 0.35;
// How quickly a particle that has touched the floor slows, per second.
const GROUND_DRAG: f32 = 8.0;
// Seconds a particle takes to fade out at the end of its life (at most half of it).
const FADE: f32 = 0.35;
// Brightness of a glowing particle against a sunlit block.
const GLOW: f32 = 2.5;
// Fire is less bright, so its oranges and reds hold rather than washing out to white: as
// bright as the fire's own sprite.
#ifdef BLEND_ADD
// Added, a flame is a share of light that builds up where tongues overlap.
const FIRE_GLOW: f32 = 2.2;
#else
const FIRE_GLOW: f32 = 1.25;
#endif

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

// Carried up by warmth: the velocity slowed by drag, gravity (negative lifts), and slow
// loops across that widen as it rises, at a pace and phase of its own.
fn rise(p: Particle, age: f32) -> vec3<f32> {
    let k = max(p.drag, 0.001);
    var position = p.origin + p.velocity * (1.0 - exp(-k * age)) / k;
    position.y -= 0.5 * p.gravity * age * age;
    let wander = f32((p.surface >> 18u) & 63u) / 100.0;
    let phase = f32(p.seed >> 22u) / 1024.0 * 6.2831853;
    let pace = 0.5 + f32((p.seed >> 12u) & 1023u) / 1023.0;
    let reach = wander * sqrt(min(age / max(p.life, 0.001), 1.0));
    let turn = age * pace * 2.5 + phase;
    return position + vec3(sin(turn), 0.0, cos(turn * 0.8 + 1.3 * phase)) * reach;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let p = particles[instance_index];
    let age = age_of(p);
    // Dead, never written, or drawn by another pipeline: every vertex at one point, so
    // nothing is drawn.
    if age >= p.life || ((p.sprite >> 26u) & 3u) != DRAWN {
        out.clip_position = vec4(0.0, 0.0, 0.0, 1.0);
        return out;
    }
    var center = p.origin;
    let motion = (p.sprite >> 24u) & 3u;
    var fade_time = min(FADE, 0.5 * p.life);
    var fade_in = 1.0;
    if motion == MOTION_BALLISTIC {
        center = ballistic(p, age);
    } else if motion == MOTION_RISE {
        center = rise(p, age);
        // Thinning out from the start (smoke is thickest over the fire), never popping in.
        fade_time = 0.8 * p.life;
        fade_in = smoothstep(0.0, min(0.4, 0.2 * p.life), age);
    }
    let fade = (1.0 - smoothstep(p.life - fade_time, p.life, age)) * fade_in;
    let corner = CORNERS[vertex_index];
    // Facing the camera, growing or shrinking over its life, and a little as it fades.
    let grow = f32((p.surface >> 12u) & 63u) / 8.0;
    let half = 0.5 * p.size * mix(1.0, grow, age / p.life) * mix(0.7, 1.0, fade);
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
    var linear = srgb_to_linear(color.rgb);
    if (out.flags & FLAG_COOLS) != 0u {
        linear *= mix(vec3(1.0), COOLED, smoothstep(0.0, 1.0, age / p.life));
    }
    out.color = vec4(linear, color.a * fade);
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
    // A texture piece is cut out where its texels are. A glowing speck (a flame, an ember)
    // is crisp, like the pixels of a fire; any other (smoke) is soft, densest in the
    // middle.
    let r2 = dot(in.corner, in.corner);
    let glowing = (in.flags & FLAG_GLOW) != 0u;
    let crisp = textured || glowing;
    // A glowing speck narrows upwards into a tongue, as flames and embers do.
    let taper = mix(1.0, 0.3, saturate(in.corner.y * 0.5 + 0.5));
    let tongue = length(vec2(in.corner.x / taper, in.corner.y));
    var coverage = select(saturate(1.0 - r2) * saturate(1.0 - r2), saturate(1.5 - 1.25 * tongue), glowing);
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
    if glowing && (in.flags & FLAG_COOLS) != 0u {
        let heat = 1.0 - tongue;
#ifdef BLENDED
        // Added light: a soft tongue whose colour runs smoothly from a deep red edge
        // through the particle's own to a white-yellow heart, so overlapping tongues merge
        // into one fire, brightest where they crowd.
        let glow = saturate(heat * 1.6);
        coverage = glow * glow * (3.0 - 2.0 * glow);
        albedo = mix(albedo * EDGE, albedo, smoothstep(0.0, 0.5, heat));
        albedo = mix(albedo, CORE, 0.7 * smoothstep(0.55, 1.0, heat));
#else
        // Fire is drawn in bands, as pixel art is: a white-yellow core, the particle's own
        // colour, and an edge turned towards deep red, cooling with it as it ages.
        albedo = select(albedo * EDGE, albedo, heat > 0.3);
        albedo = select(albedo, mix(albedo, CORE, 0.7), heat > 0.7);
#endif
    } else if !crisp {
        // Smoke is a little darker at its thin edge than in its body.
        albedo *= mix(0.8, 1.0, coverage);
    }
#ifdef BLENDED
    // Soft against the scene: no hard line where smoke or a flame meets a wall or the
    // ground. The depth is the nearest (reverse-Z largest) of the pixel's samples' first.
    let behind = depth_ndc_to_view_z(textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0));
    let soft = saturate((position_world_to_view(in.world_position).z - behind) / SOFT_DEPTH);
    // Smooth edges throughout: nothing here is cut out.
    let alpha = coverage * in.color.a * soft;
    if alpha <= 0.001 {
        discard;
    }
#else ifdef ALPHA_TO_COVERAGE
    // Sharpened so texel edges stay crisp; the fade spreads over the samples. Smoke
    // stays soft: its edge thins out like its fade.
    var alpha = saturate((coverage - 0.5) / max(fwidth(coverage), 0.0001) + 0.5) * in.color.a;
    if !crisp {
        alpha = coverage * in.color.a;
    }
    if alpha <= 0.0 {
        discard;
    }
#else
    let alpha = 1.0;
    let soft = select(coverage, 1.0, crisp);
    if coverage < select(0.02, 0.5, crisp) || in.color.a * soft < dither(in.clip_position.xy) {
        discard;
    }
#endif
    var color = albedo * select(GLOW, FIRE_GLOW, (in.flags & FLAG_COOLS) != 0u);
    if (in.flags & FLAG_GLOW) == 0u {
        let sky = f32(in.surface >> 28u) / 15.0;
        let block = f32((in.surface >> 24u) & 15u) / 15.0;
        // Lit like a plant: no face is turned away from the sun.
        // Hard-edged shadows: a particle is too small to show a penumbra.
        color = shade_normal(albedo, in.world_position, vec3(0.0, 1.0, 0.0), 0.85, sky, block, 1.0, true, false);
    }
#ifdef BLEND_ADD
    // Light added to the scene, dimmed by the air between, which adds no light of its own
    // (it is already in what lies behind).
    return vec4(color * alpha * air_between(in.world_position).transmittance, 0.0);
#else ifdef BLEND_VEIL
    // Premultiplied.
    return vec4(apply_fog(color, in.world_position) * alpha, alpha);
#else
    return vec4(apply_fog(color, in.world_position), alpha);
#endif
}
