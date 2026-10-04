// Grass blades (in the spirit of the Grassier Grass mod): a tuft grown on every grass
// block open to the air near the camera, bending with the wind and parting around the
// player. No vertex buffer: each vertex pulls its block's seed (see voxmar_engine::mesh)
// from storage by its index, and the instance index picks the chunk's origin. Blades cast
// no shadows, which at this size nobody misses.

#import bevy_pbr::mesh_view_bindings::{view, globals}
#import voxmar::lighting::{shade_normal, apply_fog}
#import voxmar::wind::{sway, parting}

@group(2) @binding(2) var colormaps: texture_2d_array<f32>;
@group(2) @binding(3) var colormap_sampler: sampler;
@group(2) @binding(4) var<storage, read> chunk_origins: array<vec4<i32>>;
@group(2) @binding(5) var<storage, read> blade_seeds: array<u32>;

// Blades per block and vertices per blade; terrain.rs draws as many.
const BLADES: u32 = 16u;
const BLADE_VERTICES: u32 = 9u;
// A blade is two segments: three triangles over the points of a strip, base to tip.
const STRIP = array<u32, 9>(0u, 1u, 2u, 2u, 1u, 3u, 2u, 3u, 4u);
const MIDDLE: f32 = 0.55;
// Blades thin out and shrink into the ground between these distances, in blocks.
const FADE = vec2(24.0, 44.0);

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

// The grass colour for a climate, as grass block tops are tinted (see voxel.wgsl).
fn grass_tint(seed: u32) -> vec3<f32> {
    let temperature = f32((seed >> 15u) & 31u) / 31.0;
    let humidity = f32((seed >> 20u) & 31u) / 31.0 * temperature;
    let uv = vec2(1.0 - temperature, max(1.0 - humidity, 1.0 - temperature + 1.5 / 256.0));
    return textureSampleLevel(colormaps, colormap_sampler, uv, 0, 0.0).rgb;
}

@vertex
fn vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) slot: u32) -> VertexOutput {
    let seed = blade_seeds[index / (BLADES * BLADE_VERTICES)];
    let blade = index / BLADE_VERTICES % BLADES;
    let point = STRIP[index % BLADE_VERTICES];
    let local = vec3<i32>(vec3(seed & 31u, (seed >> 5u) & 31u, (seed >> 10u) & 31u));
    let block = chunk_origins[slot].xyz + local;

    let a = hash(vec3<u32>(block) + vec3(blade * 7919u, 0u, 0u));
    let b = hash(vec3<u32>(block) + vec3(0u, blade * 104729u + 31u, 0u));
    let root = vec3<f32>(block) + vec3(a.x, 1.0, a.y);
    let distance = length(root - view.world_position);
    // Far away, fewer blades, and those shrinking: drop the highest-numbered first.
    let fade = 1.0 - smoothstep(FADE.x, FADE.y, distance);
    let keep = f32(blade) + 0.5 < f32(BLADES) * (0.25 + 0.75 * fade);

    let height = mix(0.22, 0.62, a.z * a.z) * select(0.0, mix(0.3, 1.0, fade), keep);
    let width = mix(0.05, 0.09, a.w);
    let facing = b.x * 6.2832;
    let across = vec2(cos(facing), sin(facing));
    // Each blade leans a little its own way; wind and the player add to that.
    let lean = (b.yz - 0.5) * 0.5 + sway(root.xz, globals.time) + parting(root);
    let up = select(select(0.0, MIDDLE, point >= 2u), 1.0, point == 4u);
    let side = select(select(0.5, -0.5, point % 2u == 0u), 0.0, point == 4u) * (1.0 - up * 0.8);
    // Bent along a curve, and lower for it, so a blade keeps roughly its length.
    let bend = lean * up * up * height;
    let lowered = height * up / sqrt(1.0 + dot(lean, lean) * up * up);
    let world = root + vec3(across.x * side * width + bend.x, lowered, across.y * side * width + bend.y);

    var out: VertexOutput;
    out.world_position = world;
    out.clip_position = view.clip_from_world * vec4(world, 1.0);
    // Darker at the root where blades crowd, lighter at the tip, and each its own shade.
    out.albedo = grass_tint(seed) * mix(0.42, 0.95, up) * mix(0.8, 1.12, b.w);
    let block_light = (seed >> 25u) & 7u;
    out.light = vec2(f32(seed >> 28u) / 15.0, f32(block_light * 2u + min(block_light, 1u)) / 15.0);
    out.normal = normalize(vec3(-across.y * 0.4, 1.0, across.x * 0.4));
    out.shadow_at = vec3(world.x, max(world.y, root.y + 0.12), world.z);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = shade_normal(in.albedo, in.shadow_at, normalize(in.normal), 0.92, in.light.x, in.light.y, 1.0, true, in.clip_position.xy);
    return vec4(apply_fog(color, in.world_position), 1.0);
}
