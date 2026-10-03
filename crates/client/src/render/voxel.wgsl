// Voxel terrain. Vertices are two packed u32 words (see voxmar_engine::mesh):
//   word 0: x:6 | y:6 | z:6 | face:3 | ao:2 | backed:1
//   word 1: layer:12 | tint:2 | temperature:5 | humidity:5 | block:4 | sky:4
// The same file provides the main pass and the depth prepass / shadow pass entry points.

#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::view
#import bevy_render::globals::Globals

#ifdef PREPASS_PIPELINE
@group(0) @binding(1) var<uniform> globals: Globals;
#else
#import bevy_pbr::mesh_view_bindings::globals
#import voxmar::lighting::{NORMALS, shade, apply_fog, sun_direction, sun_color, sun_shadow}
#import bevy_pbr::mesh_view_bindings::fog
#endif

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var block_textures: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var block_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var colormaps: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var colormap_sampler: sampler;

const TINT_NONE: u32 = 0u;
const TINT_FOLIAGE: u32 = 2u;
const TINT_WATER: u32 = 3u;
const AO_CURVE = array<f32, 4>(0.45, 0.65, 0.84, 1.0);

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) packed: vec2<u32>,
};

fn local_position(w0: u32) -> vec3<f32> {
    return vec3<f32>(f32(w0 & 63u), f32((w0 >> 6u) & 63u), f32((w0 >> 12u) & 63u));
}

fn tint_kind(surface: u32) -> u32 {
    return (surface >> 12u) & 3u;
}

// World position, with leaves swaying gently. Shared by every pass so depth matches.
fn world_position(in: Vertex) -> vec3<f32> {
    let world_from_local = get_world_from_local(in.instance_index);
    var world = (world_from_local * vec4(local_position(in.packed.x), 1.0)).xyz;
    if tint_kind(in.packed.y) == TINT_FOLIAGE {
        let t = globals.time;
        let phase = world.x * 0.7 + world.z * 0.5 + world.y * 0.3;
        world += vec3(sin(t * 1.6 + phase), sin(t * 2.3 + phase * 1.3) * 0.5, cos(t * 1.9 + phase)) * 0.035;
    }
    return world;
}

// Texture coordinates from the chunk-local position, so merged quads tile per block.
fn face_uv(local: vec3<f32>, face: u32) -> vec2<f32> {
    switch face {
        case 0u: { return vec2(-local.z, -local.y); }
        case 1u: { return vec2(local.z, -local.y); }
        case 2u: { return vec2(local.x, local.z); }
        case 3u: { return vec2(local.x, -local.z); }
        case 4u: { return vec2(local.x, -local.y); }
        default: { return vec2(-local.x, -local.y); }
    }
}

// Cut-out texel test. Backed faces (inside a crown) fill their holes instead.
fn alpha_test(uv: vec2<f32>, surface: u32, backed: bool) -> f32 {
    let alpha = textureSample(block_textures, block_sampler, uv, i32(surface & 0xFFFu)).a;
    if alpha < 0.5 && !backed {
        discard;
    }
    return alpha;
}

#ifdef PREPASS_PIPELINE

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) surface: u32,
    @location(2) @interpolate(flat) backed: u32,
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(3) unclipped_depth: f32,
#endif
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    out.position = view.clip_from_world * vec4(world_position(in), 1.0);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
    out.uv = face_uv(local_position(in.packed.x), (in.packed.x >> 18u) & 7u);
    out.surface = in.packed.y & 0xFFFFFFu;
    out.backed = (in.packed.x >> 23u) & 1u;
    return out;
}

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
@fragment
fn fragment(in: VertexOutput) -> @builtin(frag_depth) f32 {
    alpha_test(in.uv, in.surface, in.backed != 0u);
    return in.unclipped_depth;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    alpha_test(in.uv, in.surface, in.backed != 0u);
}
#endif

#else // main pass

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // x: ao, y: sky light, z: block light (all 0..1, interpolated for smooth lighting)
    @location(2) light: vec3<f32>,
    @location(3) @interpolate(flat) surface: u32,
    @location(4) @interpolate(flat) face: u32,
    @location(5) @interpolate(flat) backed: u32,
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let w1 = in.packed.y;
    var out: VertexOutput;
    out.world_position = world_position(in);
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.face = (w0 >> 18u) & 7u;
    out.uv = face_uv(local_position(w0), out.face);
    out.light = vec3(AO_CURVE[(w0 >> 21u) & 3u], f32(w1 >> 28u) / 15.0, f32((w1 >> 24u) & 15u) / 15.0);
    out.surface = w1 & 0xFFFFFFu;
    out.backed = (w0 >> 23u) & 1u;
    return out;
}

fn biome_tint(surface: u32) -> vec3<f32> {
    let kind = tint_kind(surface);
    if kind == TINT_NONE {
        return vec3(1.0);
    }
    let temperature = f32((surface >> 14u) & 31u) / 31.0;
    let humidity = f32((surface >> 19u) & 31u) / 31.0 * temperature;
    if kind == TINT_WATER {
        // Teal in warm climates, deep blue in cold ones.
        return mix(vec3(0.16, 0.30, 0.78), vec3(0.14, 0.50, 0.66), temperature);
    }
    let uv = vec2(1.0 - temperature, 1.0 - humidity);
    return textureSampleLevel(colormaps, colormap_sampler, uv, i32(kind - 1u), 0.0).rgb;
}

// Small animated ripples for the water surface normal.
fn water_normal(world: vec3<f32>) -> vec3<f32> {
    let t = globals.time;
    let p = world.xz;
    let dx = sin(p.x * 0.9 + t * 1.3) * 0.5 + sin(p.x * 2.3 - p.y * 1.1 + t * 2.1) * 0.25 + sin(dot(p, vec2(0.6, 1.7)) + t * 0.8) * 0.3;
    let dz = cos(p.y * 1.1 + t * 1.1) * 0.5 + cos(p.y * 2.7 + p.x * 0.7 - t * 1.7) * 0.25 + cos(dot(p, vec2(-1.4, 0.5)) + t * 0.9) * 0.3;
    return normalize(vec3(dx * 0.08, 1.0, dz * 0.08));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let layer = i32(in.surface & 0xFFFu);
    var albedo = textureSample(block_textures, block_sampler, in.uv, layer);
    let kind = tint_kind(in.surface);
#ifndef TRANSLUCENT
    if albedo.a < 0.5 {
        if in.backed == 0u {
            discard;
        }
        // Inside a tree crown: fill the gap with the leaf's own average colour, in shade.
        let average = textureSampleLevel(block_textures, block_sampler, in.uv, layer, 8.0).rgb;
        albedo = vec4(average * 0.45, 1.0);
    }
    albedo.a = 1.0;
#endif
    let base = albedo.rgb * biome_tint(in.surface);
    var color = shade(base, in.world_position, in.face, in.light.y, in.light.z, in.light.x, kind == TINT_FOLIAGE, in.clip_position.xy);
    var alpha = albedo.a;

#ifdef TRANSLUCENT
    if kind == TINT_WATER && in.face == 2u {
        // Fresnel reflection of the sky and a sun glint on rippled water.
        let n = water_normal(in.world_position);
        let to_eye = normalize(view.world_position - in.world_position);
        let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(n, to_eye), 0.0), 5.0);
        let sky = fog.base_color.rgb * 1.15;
        let open = smoothstep(0.5, 0.95, in.light.y);
        let glint = pow(max(dot(reflect(-to_eye, n), sun_direction()), 0.0), 220.0) * open
            * sun_shadow(in.world_position, n, in.clip_position.xy);
        color = mix(color, sky * open + color * (1.0 - open), fresnel) + sun_color() * glint * 4.0;
        alpha = mix(0.72, 1.0, fresnel);
    }
#endif

    return vec4(apply_fog(color, in.world_position), alpha);
}

#endif
