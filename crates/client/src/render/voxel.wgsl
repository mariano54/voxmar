// Voxel terrain. Vertices are two packed u32 words (see voxmar_engine::mesh):
//   word 0: x:6 | y:6 | z:6 | face:3 | ao:2 | flags:2
//   word 1: layer:12 | tint:2 | temperature:5 | humidity:5 | block:4 | sky:4
// Faces 0-5 are cube faces; 6 and 7 are a plant's two diagonal quads. The flags mark a
// translucent face covered by a block (water under ice) or a plant's top edge (FLAG_INNER),
// and a leaf face with leaves behind it, which casts a solid shadow (FLAG_BACKED).
// The same file provides the main pass and the depth prepass / shadow pass entry points.
// Shader defs pick the pass: none for opaque terrain (no alpha test, so the GPU keeps early
// depth and hidden surface removal), CUTOUT for leaves, glass and plants, plus INNER for the
// double-sided faces between two leaves, TRANSLUCENT for water and ice.

// Chunks are drawn from shared buffers (see terrain.rs): the instance index picks the
// chunk's origin from a table.

#import bevy_render::globals::Globals

#ifdef PREPASS_PIPELINE
#import bevy_render::view::View
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> globals: Globals;
#else
#import bevy_pbr::mesh_view_bindings::{view, globals, fog}
#import voxmar::lighting::{NORMALS, shade, apply_fog, sun_direction, sun_color, sun_shadow, water_normal}
#endif

@group(2) @binding(0) var block_textures: texture_2d_array<f32>;
@group(2) @binding(1) var block_sampler: sampler;
@group(2) @binding(2) var colormaps: texture_2d_array<f32>;
@group(2) @binding(3) var colormap_sampler: sampler;
@group(2) @binding(4) var<storage, read> chunk_origins: array<vec4<i32>>;

const TINT_NONE: u32 = 0u;
const TINT_FOLIAGE: u32 = 2u;
const TINT_WATER: u32 = 3u;
const FACE_PLANT: u32 = 6u;
const FLAG_INNER: u32 = 1u << 23u;
const FLAG_BACKED: u32 = 1u << 24u;
const PLANT_STACK_SHIFT: u32 = 25u;
const WATER_DEPTH_SHIFT: u32 = 25u;
// How fast light fades through water, per block.
const WATER_EXTINCTION: f32 = 0.32;
const FLAG_FRINGE: u32 = 1u << 27u;
// Leaf fringes shrink back into their block between these distances, in blocks (the
// renderer stops drawing them a little beyond).
const FRINGE_FADE = vec2(56.0, 88.0);
const AO_CURVE = array<f32, 4>(0.45, 0.65, 0.84, 1.0);
// Plants shrink away between these distances, in blocks.
const PLANT_FADE = vec2(192.0, 320.0);

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

fn face_of(w0: u32) -> u32 {
    return (w0 >> 18u) & 7u;
}

// World position, with leaves swaying gently and plants bending from their base. Shared
// by every pass so depth matches.
fn pcg3d(seed: vec3<u32>) -> vec3<f32> {
    var v = seed * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v >> vec3(8u)) / 16777216.0;
}

// Smooth noise over the ground, for patches of taller and shorter plants.
fn tufts(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let at = vec4(
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x), 7, i32(cell.y)))).x,
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x) + 1, 7, i32(cell.y)))).x,
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x), 7, i32(cell.y) + 1))).x,
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x) + 1, 7, i32(cell.y) + 1))).x,
    );
    return mix(mix(at.x, at.y, w.x), mix(at.z, at.w, w.x), w.y);
}

// A plant's vertex, with the plant placed as a whole from a hash of its cell, much as
// Rainbow's Foliage varies its models: turned up to 40° either way, shifted and sunk a
// little, and taller or shorter, in tufts across a meadow.
fn plant_position(w0: u32, origin: vec3<f32>) -> vec3<f32> {
    let corner = (w0 >> 21u) & 3u;
    let far = corner == 1u || corner == 2u;
    var across = select(vec2(0.0), vec2(1.0), far);
    if face_of(w0) != FACE_PLANT {
        across = select(vec2(1.0, 0.0), vec2(0.0, 1.0), far);
    }
    if (w0 & FLAG_FRINGE) != 0u {
        return fringe_position(w0, origin, across);
    }
    let up = f32((w0 >> 23u) & 1u) + f32((w0 >> PLANT_STACK_SHIFT) & 3u);
    // The corner of the cell the whole plant stands in.
    let base = origin + local_position(w0) - vec3(across.x, up, across.y);
    let cell = vec3<i32>(floor(base + 0.5));
    let a = pcg3d(vec3<u32>(cell));
    let b = pcg3d(vec3<u32>(cell + vec3(0, 991, 0)));
    let angle = (a.x - 0.5) * 1.4;
    let offset = across - 0.5;
    let turned = vec2(offset.x * cos(angle) - offset.y * sin(angle), offset.x * sin(angle) + offset.y * cos(angle));
    let shift = (a.yz - 0.5) * 0.3;
    let height = mix(0.6, 1.25, saturate(0.7 * tufts(base.xz * 0.12) + 0.3 * b.x));
    let sink = b.y * 0.25;
    return vec3(base.x + 0.5 + turned.x + shift.x, base.y + up * height - sink, base.z + 0.5 + turned.y + shift.y);
}

// A leaf fringe's vertex: two crossed quads through the leaf block, turned at random, grown
// past the block and leaning, so crowns look bushy rather than cubic.
fn fringe_position(w0: u32, origin: vec3<f32>, across: vec2<f32>) -> vec3<f32> {
    let top = f32((w0 >> 23u) & 1u);
    let corner = origin + local_position(w0) - vec3(across.x, top, across.y);
    let centre = corner + 0.5;
    let a = pcg3d(vec3<u32>(vec3<i32>(floor(centre))));
    let b = pcg3d(vec3<u32>(vec3<i32>(floor(centre)) + vec3(0, 577, 0)));
    var size = 1.45 + 0.45 * a.y;
#ifndef PREPASS_PIPELINE
    size = mix(size, 0.8, smoothstep(FRINGE_FADE.x, FRINGE_FADE.y, distance(centre, view.world_position)));
#endif
    // The second quad crosses the first at right angles.
    let angle = a.x * 6.2832 + select(0.0, 1.5708, face_of(w0) != FACE_PLANT);
    let along = vec2(cos(angle), sin(angle));
    let offset = (across.x - 0.5) * size;
    let up = (top - 0.5) * size;
    let lean = vec2(-along.y, along.x) * up * (a.z - 0.5) * 0.9;
    let jitter = (b - 0.5) * 0.2;
    return centre + vec3(along.x * offset + lean.x, up, along.y * offset + lean.y) + jitter;
}

fn world_position(in: Vertex) -> vec3<f32> {
    let origin = chunk_origins[in.instance_index];
    let t = globals.time;
    var world: vec3<f32>;
    if face_of(in.packed.x) >= FACE_PLANT {
        world = plant_position(in.packed.x, vec3<f32>(origin.xyz));
    } else {
        world = vec3<f32>(origin.xyz) + local_position(in.packed.x);
    }
    if face_of(in.packed.x) >= FACE_PLANT {
        if (in.packed.x & FLAG_INNER) != 0u {
            // Gusts roll across the field: a slow wave plus a quicker flutter.
            let phase = world.x * 0.35 + world.z * 0.25;
            let gust = sin(t * 1.3 + phase) * 0.6 + sin(t * 2.9 + phase * 2.1) * 0.25;
            world += vec3(gust, 0.0, gust * 0.6 + cos(t * 2.2 + world.x * 0.9) * 0.15) * 0.09;
        }
    } else if tint_kind(in.packed.y) == TINT_FOLIAGE {
        let phase = world.x * 0.7 + world.z * 0.5 + world.y * 0.3;
        world += vec3(sin(t * 1.6 + phase), sin(t * 2.3 + phase * 1.3) * 0.5, cos(t * 1.9 + phase)) * 0.035;
    }
    return world;
}

// Texture coordinates from the chunk-local position, so merged quads tile per block.
// Plant quads take u from their position along the diagonal.
fn face_uv(local: vec3<f32>, face: u32) -> vec2<f32> {
    switch face {
        case 0u: { return vec2(-local.z, -local.y); }
        case 1u: { return vec2(local.z, -local.y); }
        case 2u: { return vec2(local.x, local.z); }
        case 3u: { return vec2(local.x, -local.z); }
        case 4u: { return vec2(local.x, -local.y); }
        case 5u: { return vec2(-local.x, -local.y); }
        case 6u: { return vec2(local.x, -local.y); }
        default: { return vec2(local.z, -local.y); }
    }
}

// Cut-out texel test.
fn alpha_test(uv: vec2<f32>, surface: u32, solid: u32) {
    if solid == 0u && textureSample(block_textures, block_sampler, uv, i32(surface & 0xFFFu)).a < 0.5 {
        discard;
    }
}

#ifdef PREPASS_PIPELINE

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) surface: u32,
    // Skips the alpha test: a leaf face with leaves behind it shades solidly.
    @location(2) @interpolate(flat) solid: u32,
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(3) unclipped_depth: f32,
#endif
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    out.solid = select(0u, 1u, face_of(in.packed.x) < FACE_PLANT && (in.packed.x & FLAG_BACKED) != 0u);
    out.position = view.clip_from_world * vec4(world_position(in), 1.0);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
    out.uv = face_uv(local_position(in.packed.x), face_of(in.packed.x));
    out.surface = in.packed.y & 0xFFFFFFu;
    return out;
}

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
@fragment
fn fragment(in: VertexOutput) -> @builtin(frag_depth) f32 {
    alpha_test(in.uv, in.surface, in.solid);
    return in.unclipped_depth;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    alpha_test(in.uv, in.surface, in.solid);
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
    @location(5) @interpolate(flat) covered: u32,
    // Depth of water below a water surface, in blocks.
    @location(6) water_depth: f32,
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let w1 = in.packed.y;
    var out: VertexOutput;
    out.world_position = world_position(in);
    if face_of(w0) >= FACE_PLANT && (w0 & FLAG_INNER) != 0u {
        // Plants sink into the ground with distance, where they would only be a shimmer of
        // sub-pixel blades. Their shadows stay, which far away nobody can tell apart.
        let distance = length(out.world_position.xz - view.world_position.xz);
        out.world_position.y -= smoothstep(PLANT_FADE.x, PLANT_FADE.y, distance);
    }
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.face = face_of(w0);
    out.uv = face_uv(local_position(w0), out.face);
    // Plants have no ambient occlusion; their AO bits hold the corner.
    let ao = select(AO_CURVE[(w0 >> 21u) & 3u], 1.0, face_of(w0) >= FACE_PLANT);
    out.light = vec3(ao, f32(w1 >> 28u) / 15.0, f32((w1 >> 24u) & 15u) / 15.0);
    out.surface = w1 & 0xFFFFFFu;
    out.covered = select(0u, 1u, out.face < FACE_PLANT && (w0 & FLAG_INNER) != 0u);
    out.water_depth = f32((w0 >> WATER_DEPTH_SHIFT) & 7u);
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
    // Stay a texel inside the colour map's valid triangle: the texels past its diagonal
    // (humidity above temperature) are blank and would wash the colour out when filtered.
    let uv = vec2(1.0 - temperature, max(1.0 - humidity, 1.0 - temperature + 1.5 / 256.0));
    return textureSampleLevel(colormaps, colormap_sampler, uv, i32(kind - 1u), 0.0).rgb;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let layer = i32(in.surface & 0xFFFu);
    var albedo = textureSample(block_textures, block_sampler, in.uv, layer);
    let kind = tint_kind(in.surface);
#ifdef ALPHA_TO_COVERAGE
    // Sharpened so a texel edge stays crisp up close; once the texture is minified the
    // alpha spreads over a few samples instead of flickering between pass and fail.
    albedo.a = saturate((albedo.a - 0.5) / max(fwidth(albedo.a), 0.0001) + 0.5);
    if albedo.a <= 0.0 {
        discard;
    }
#else
#ifdef CUTOUT
    if albedo.a < 0.5 {
        discard;
    }
#endif
#ifndef TRANSLUCENT
    albedo.a = 1.0;
#endif
#endif
    let base = albedo.rgb * biome_tint(in.surface);
    let plant = in.face >= FACE_PLANT;
    // Plants look up their shadow just above their own cell: trees and terrain still shade
    // them, but the blades of a dense tuft do not darken each other.
    var shadow_at = in.world_position;
    if plant {
        shadow_at.y = floor(in.world_position.y - 0.001) + 1.1;
    }
    var face = in.face;
#ifdef INNER
    // Seen from its back, the face belongs to the leaf on the other side.
    face = select(face ^ 1u, face, front_facing);
#endif
    var color = shade(base, shadow_at, face, in.light.y, in.light.z, in.light.x, kind == TINT_FOLIAGE || plant, in.clip_position.xy);
    var alpha = albedo.a;

#ifdef TRANSLUCENT
    let to_eye = normalize(view.world_position - in.world_position);
    let sky = fog.base_color.rgb * 1.15;
    let open = smoothstep(0.5, 0.95, in.light.y);
    if kind == TINT_WATER {
        if in.face == 2u && in.covered == 0u {
            // Fresnel reflection of the sky and a sun glint on rippled water.
            let n = water_normal(in.world_position);
            let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(n, to_eye), 0.0), 5.0);
            let glint = pow(max(dot(reflect(-to_eye, n), sun_direction()), 0.0), 220.0) * open
                * sun_shadow(in.world_position, n, in.clip_position.xy);
            // Shallows show their bed: light through the water fades with depth, and what
            // remains takes the water's colour.
            let through = exp(-in.water_depth * WATER_EXTINCTION);
            color = mix(color, color * 0.55, 1.0 - through);
            color = mix(color, sky * open + color * (1.0 - open), fresnel) + sun_color() * glint * 4.0;
            alpha = mix(mix(0.92, 0.18, through), 1.0, fresnel);
        }
    } else {
        // Solid see-through blocks (ice) show only their outside, like any solid block, so
        // a sheet of ice never blends its far side over what lies beyond. Clearer than
        // their texture, they reflect the sky through their own tint, mirror-smooth, with
        // a sharp sun glint.
        if !front_facing {
            discard;
        }
        let n = NORMALS[in.face];
        let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(n, to_eye), 0.0), 5.0);
        let glint = pow(max(dot(reflect(-to_eye, n), sun_direction()), 0.0), 160.0) * open
            * sun_shadow(in.world_position, n, in.clip_position.xy);
        let hue = base / max(max(base.r, max(base.g, base.b)), 0.001);
        let reflection = sky * mix(vec3(1.0), hue, 0.5);
        color = mix(color, reflection * open + color * (1.0 - open), fresnel * 0.7) + sun_color() * glint * 3.0;
        alpha = mix(alpha * 0.55, 0.95, fresnel);
    }
#endif

    return vec4(apply_fog(color, in.world_position), alpha);
}

#endif
