// Voxel terrain. Vertices are two packed u32 words (see voxmar_engine::mesh).

#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}
#import bevy_pbr::mesh_view_bindings::view

struct VoxelParams {
    // xyz: direction towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // rgb: fog / horizon colour
    fog_color: vec4<f32>,
    // x: fog start, y: fog end, z: seconds, w: 1 when the camera is underwater
    fog: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var block_textures: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var block_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var colormaps: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var colormap_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> params: VoxelParams;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // x: ao, y: sky light, z: block light (all 0..1, interpolated for smooth lighting)
    @location(2) light: vec3<f32>,
    @location(3) @interpolate(flat) surface: u32,
    @location(4) @interpolate(flat) face: u32,
};

const NORMALS = array<vec3<f32>, 6>(
    vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0),
);
// Minecraft-style directional shading per face.
const FACE_SHADE = array<f32, 6>(0.6, 0.6, 1.0, 0.5, 0.8, 0.8);
const AO_CURVE = array<f32, 4>(0.42, 0.62, 0.82, 1.0);

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let w1 = in.packed.y;
    let local = vec3<f32>(f32(w0 & 63u), f32((w0 >> 6u) & 63u), f32((w0 >> 12u) & 63u));
    let face = (w0 >> 18u) & 7u;
    let ao = (w0 >> 21u) & 3u;

    let world_from_local = get_world_from_local(in.instance_index);
    var out: VertexOutput;
    out.clip_position = mesh_position_local_to_clip(world_from_local, vec4(local, 1.0));
    out.world_position = (world_from_local * vec4(local, 1.0)).xyz;

    // Texture coordinates from the chunk-local position, so merged quads tile per block.
    switch face {
        case 0u: { out.uv = vec2(-local.z, -local.y); }
        case 1u: { out.uv = vec2(local.z, -local.y); }
        case 2u: { out.uv = vec2(local.x, local.z); }
        case 3u: { out.uv = vec2(local.x, -local.z); }
        case 4u: { out.uv = vec2(local.x, -local.y); }
        default: { out.uv = vec2(-local.x, -local.y); }
    }
    out.light = vec3(AO_CURVE[ao], f32((w1 >> 24u) & 15u) / 15.0, f32(w1 >> 28u) / 15.0);
    out.surface = w1 & 0xFFFFFFu;
    out.face = face;
    return out;
}

// Minecraft's light curve: brightness rises slowly at low levels.
fn light_curve(level: f32) -> f32 {
    return level / (4.0 - 3.0 * level);
}

fn biome_tint(surface: u32) -> vec3<f32> {
    let kind = (surface >> 12u) & 3u;
    if kind == 0u {
        return vec3(1.0);
    }
    let temperature = f32((surface >> 14u) & 31u) / 31.0;
    let humidity = f32((surface >> 19u) & 31u) / 31.0 * temperature;
    if kind == 3u {
        // Water: teal in warm climates, deep blue in cold ones.
        return mix(vec3(0.20, 0.32, 0.80), vec3(0.18, 0.55, 0.70), temperature);
    }
    let uv = vec2(1.0 - temperature, 1.0 - humidity);
    return textureSampleLevel(colormaps, colormap_sampler, uv, i32(kind - 1u), 0.0).rgb;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let layer = i32(in.surface & 0xFFFu);
    var albedo = textureSample(block_textures, block_sampler, in.uv, layer);
#ifndef TRANSLUCENT
    if albedo.a < 0.5 {
        discard;
    }
    albedo.a = 1.0;
#endif
    let base = albedo.rgb * biome_tint(in.surface);

    let daylight = params.sun.w;
    let sun_dir = params.sun.xyz;
    let normal = NORMALS[in.face];
    let ao = in.light.x;
    let sky = light_curve(in.light.y);
    let block = light_curve(in.light.z);

    // Sky light: ambient from the sky dome plus direct sun on faces that see the sky.
    let sun_facing = max(dot(normal, sun_dir), 0.0);
    let direct = sun_facing * smoothstep(0.75, 1.0, in.light.y) * daylight;
    let sky_tint = mix(vec3(0.10, 0.12, 0.22), vec3(1.0, 0.98, 0.95), daylight);
    let sky_light = sky * sky_tint * (FACE_SHADE[in.face] * 0.75 + direct * 0.45);
    // Block light (torches, glowstone): warm, unaffected by daylight.
    let block_light = block * vec3(1.0, 0.82, 0.6) * mix(FACE_SHADE[in.face], 1.0, 0.5);
    let ambient = vec3(0.025, 0.025, 0.035);
    var color = base * (max(sky_light, block_light) + ambient) * ao;

    // Distance fog towards the horizon colour.
    let distance = length(in.world_position - view.world_position);
    var fog = smoothstep(params.fog.x, params.fog.y, distance);
    if params.fog.w > 0.5 {
        fog = 1.0 - exp(-distance * 0.06);
    }
    color = mix(color, params.fog_color.rgb, fog);
    return vec4(color, albedo.a);
}
