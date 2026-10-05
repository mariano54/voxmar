// Grass blades (in the spirit of the Grassier Grass mod): a tuft grown on every grass
// block open to the air near the camera, bending with the wind and parting around the
// player. No vertex buffer: each vertex pulls its block's seed (see voxmar_engine::mesh)
// from storage by its index, and the instance index carries the chunk's origin and how
// many blades each of its blocks grows (fewer further away). An index buffer repeating
// one blade's three triangles lets the GPU reuse each of its five points. Blades cast no
// shadows, which at this size nobody misses.

#import bevy_pbr::mesh_view_bindings::{view, globals}
#import voxmar::lighting::{shade_normal, apply_fog}
#import voxmar::wind::{sway, parting, wind_strength}

@group(2) @binding(2) var<storage, read> chunk_origins: array<vec4<i32>>;
@group(2) @binding(3) var<storage, read> blade_seeds: array<u32>;
@group(2) @binding(5) var<storage, read> column_colours: array<u32>;

// The most blades a block grows; terrain.rs asks for fewer further away, matching `kept`.
const BLADES: f32 = 16.0;
// A blade is two segments, five points of a strip from base to tip.
const POINTS: u32 = 5u;
const MIDDLE: f32 = 0.55;
// Blades thin out and shrink into the ground between these distances, in blocks
// (terrain.rs's BLADE_FADE).
const FADE = vec2(16.0, 34.0);

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) albedo: vec3<f32>,
    // x: sky light, y: block light, 0..1.
    @location(2) light: vec2<f32>,
    @location(3) normal: vec3<f32>,
    // Where the blade looks up its shadow: just above the ground it grows from, so the
    // ground's own depth never shadows it.
    @location(4) shadow_at: vec3<f32>,
};

fn hash(seed: vec3<u32>) -> vec4<f32> {
    var v = vec4(seed, seed.x ^ seed.z) * 1664525u + 1013904223u;
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    v ^= v >> vec4(16u);
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    return vec4<f32>(v >> vec4(8u)) / 16777216.0;
}

// The grass colour of a block, as its top is tinted (see voxel.wgsl's `biome_tint`).
fn grass_colour(colours: i32, local: vec3<i32>) -> vec3<f32> {
    let word = column_colours[u32(colours) + u32(local.z * 32 + local.x) * 3u];
    let c = unpack4x8unorm(word).rgb;
    return select(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), c > vec3(0.04045));
}

// How many of a block's blades still grow at a distance: all of them near, a quarter at
// the edge of the fade.
fn kept(fade: f32) -> f32 {
    return BLADES * (0.25 + 0.75 * fade);
}

@vertex
fn vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) instance: u32) -> VertexOutput {
    // instance: slot:24 | blades per block:5; index: point of every blade of the chunk's
    // blocks in turn, from the chunk's first seed.
    let blades = instance >> 24u;
    let point = index % POINTS;
    let global_blade = index / POINTS;
    let seed = blade_seeds[global_blade / blades];
    let blade = global_blade % blades;
    let local = vec3<i32>(vec3(seed & 31u, (seed >> 5u) & 31u, (seed >> 10u) & 31u));
    let chunk = chunk_origins[instance & 0xFFFFFFu];
    let block = chunk.xyz + local;

    let a = hash(vec3<u32>(block) + vec3(blade * 7919u, 0u, 0u));
    let b = hash(vec3<u32>(block) + vec3(0u, blade * 104729u + 31u, 0u));
    let root = vec3<f32>(block) + vec3(a.x, 1.0, a.y);
    let distance = length(root - view.world_position);
    // Far away, fewer blades, and those shrinking: drop the highest-numbered first.
    let fade = 1.0 - smoothstep(FADE.x, FADE.y, distance);
    let keep = f32(blade) + 0.5 < kept(fade);

    let height = mix(0.22, 0.62, a.z * a.z) * select(0.0, fade, keep);
    let width = mix(0.05, 0.09, a.w);
    let facing = b.x * 6.2832;
    let across = vec2(cos(facing), sin(facing));
    // Each blade gives to the wind a little more or less than its neighbours and flutters
    // at its own pace, a whole number of 64ths near twice the flutter phase; at rest it
    // leans a little its own way, less as the wind takes it.
    let own = fract(b * 64.0);
    let pace = floor(mix(58.0, 71.0, own.x)) / 32.0;
    let strength = wind_strength();
    let wind = sway(root.xz, pace, own.y * 1.5);
    let lean = (b.yz - 0.5) * 0.5 * (1.0 - 0.6 * strength) + wind.lean * mix(0.8, 1.15, own.z) + parting(root);
    let up = select(select(0.0, MIDDLE, point >= 2u), 1.0, point == 4u);
    let side = select(select(0.5, -0.5, point % 2u == 0u), 0.0, point == 4u) * (1.0 - up * 0.8);
    // Bent along a curve, and lower for it, so a blade keeps roughly its length: its tip
    // leans over to the angle whose tangent is `lean`.
    let bend = lean * up * up * height / sqrt(1.0 + dot(lean, lean));
    let lowered = height * up / sqrt(1.0 + dot(lean, lean) * up * up);
    let world = root + vec3(across.x * side * width + bend.x, lowered, across.y * side * width + bend.y);

    var out: VertexOutput;
    out.world_position = world;
    out.clip_position = view.clip_from_world * vec4(world, 1.0);
    // Darker at the root where blades crowd, lighter at the tip, and each its own shade. A
    // gust sweeping over shows as a lighter wave, as bent blades catch the sky.
    let sheen = 1.0 + 0.16 * (wind.gust - 0.5) * smoothstep(0.0, 0.3, strength);
    out.albedo = grass_colour(chunk.w, local) * mix(0.42, 0.95, up) * mix(0.8, 1.12, b.w) * sheen;
    out.light = vec2(f32((seed >> 19u) & 15u) / 15.0, f32((seed >> 15u) & 15u) / 15.0);
    out.normal = normalize(vec3(-across.y * 0.4, 1.0, across.x * 0.4));
    out.shadow_at = vec3(world.x, max(world.y, root.y + 0.12), world.z);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = shade_normal(in.albedo, in.shadow_at, normalize(in.normal), 0.92, in.light.x, in.light.y, 1.0, true, in.clip_position.xy);
    return vec4(apply_fog(color, in.world_position), 1.0);
}
