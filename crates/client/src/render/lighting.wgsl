// Shared terrain lighting: sky ambient, sun or moon with cascaded shadows, block light,
// and distance fog with sun inscattering. Everything dynamic comes from the view bind group
// (directional light, ambient light, fog), so day/night never touches material assets.

#define_import_path voxmar::lighting

#import bevy_pbr::mesh_view_bindings::{view, lights, fog}
#import bevy_pbr::shadows::fetch_directional_shadow

const NORMALS = array<vec3<f32>, 6>(
    vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0),
);
// Minecraft-style directional shading for ambient light.
const FACE_SHADE = array<f32, 6>(0.62, 0.62, 1.0, 0.5, 0.8, 0.8);
const BLOCK_LIGHT_COLOR = vec3(1.0, 0.72, 0.42);

// Brightness rises slowly at low light levels, like Minecraft's light table.
fn light_curve(level: f32) -> f32 {
    return level / (2.5 - 1.5 * level);
}

fn sun_direction() -> vec3<f32> {
    return lights.directional_lights[0].direction_to_light;
}

fn sun_color() -> vec3<f32> {
    return lights.directional_lights[0].color.rgb;
}

// Fraction of direct sun reaching `world`, from the cascaded shadow maps.
fn sun_shadow(world: vec3<f32>, normal: vec3<f32>, frag_xy: vec2<f32>) -> f32 {
    let light = lights.directional_lights[0];
    if lights.n_directional_lights == 0u || (light.flags & 1u) == 0u {
        return 1.0;
    }
    let view_z = (view.view_from_world * vec4(world, 1.0)).z;
    return fetch_directional_shadow(0u, vec4(world, 1.0), normal, view_z, frag_xy);
}

// Lit colour of a voxel face. `sky` and `block` are 0..1 light levels, `ao` the occlusion
// factor, `foliage` softens the sun term for leaves so crowns glow when back-lit.
fn shade(
    albedo: vec3<f32>,
    world: vec3<f32>,
    face: u32,
    sky: f32,
    block: f32,
    ao: f32,
    foliage: bool,
    frag_xy: vec2<f32>,
) -> vec3<f32> {
    return shade_normal(albedo, world, NORMALS[face], FACE_SHADE[face], sky, block, ao, foliage, frag_xy);
}

// Lit colour of any surface with a normal and an ambient shading factor.
fn shade_normal(
    albedo: vec3<f32>,
    world: vec3<f32>,
    normal: vec3<f32>,
    ambient_shade: f32,
    sky: f32,
    block: f32,
    ao: f32,
    foliage: bool,
    frag_xy: vec2<f32>,
) -> vec3<f32> {
    let to_sun = sun_direction();
    var facing = dot(normal, to_sun);
    if foliage {
        facing = 0.35 + 0.65 * abs(facing);
    }
    // Sunlight only reaches places open to the sky; shadow maps add the exact shapes.
    let open = smoothstep(0.5, 0.95, sky);
    var direct = vec3(0.0);
    if facing > 0.0 && open > 0.0 {
        direct = sun_color() * facing * open * sun_shadow(world, normal, frag_xy);
    }
    let ambient = lights.ambient_color.rgb * light_curve(sky) * ambient_shade * ao;
    let torch = BLOCK_LIGHT_COLOR * light_curve(block) * 1.4 * mix(ambient_shade, 1.0, 0.5) * ao;
    let floor = vec3(0.010, 0.010, 0.014) * ao;
    return albedo * (ambient + direct * mix(1.0, ao, 0.5) + torch + floor);
}

// Blends towards the horizon colour with distance, brightened towards the sun.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let offset = world - view.world_position;
    let distance = length(offset);
    let amount = smoothstep(fog.be.x, fog.be.y, distance);
    let glow = pow(max(dot(offset / distance, sun_direction()), 0.0), 8.0);
    let fog_color = fog.base_color.rgb + sun_color() * glow * 0.15;
    return mix(color, fog_color, amount);
}
