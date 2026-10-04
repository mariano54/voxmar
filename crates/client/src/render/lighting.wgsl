// Shared terrain lighting: sky ambient, sun or moon with cascaded shadows, block light,
// water ripples, and aerial perspective glowing towards the sun. Everything dynamic comes
// from the view bind group (directional light, ambient light, fog), so day/night never
// touches material assets.

#define_import_path voxmar::lighting

#import bevy_pbr::mesh_view_bindings::{view, lights, fog, globals}
#import bevy_pbr::shadows::fetch_directional_shadow

// Per face index: the six cube faces, then a plant's two diagonal quads, which are lit
// as if facing up so both sides and every blade of a tuft read alike.
const NORMALS = array<vec3<f32>, 8>(
    vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0),
);
// Minecraft-style directional shading for ambient light.
const FACE_SHADE = array<f32, 8>(0.62, 0.62, 1.0, 0.5, 0.8, 0.8, 0.85, 0.85);
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

// Small animated ripples for the water surface normal. Their slopes have a variance of
// about 0.0013.
fn water_normal(world: vec3<f32>) -> vec3<f32> {
    let t = globals.time;
    let p = world.xz;
    let dx = sin(p.x * 0.9 + t * 1.3) * 0.5 + sin(p.x * 2.3 - p.y * 1.1 + t * 2.1) * 0.25 + sin(dot(p, vec2(0.6, 1.7)) + t * 0.8) * 0.3;
    let dz = cos(p.y * 1.1 + t * 1.1) * 0.5 + cos(p.y * 2.7 + p.x * 0.7 - t * 1.7) * 0.25 + cos(dot(p, vec2(-1.4, 0.5)) + t * 0.9) * 0.3;
    return normalize(vec3(dx * 0.08, 1.0, dz * 0.08));
}

// Aerial perspective. Haze is densest at sea level and thins with altitude, so valleys and
// distant lowlands fade sooner than peaks; the edge of the distant terrain (fog.be.x to
// fog.be.y) fades out completely. Real chunks, distant terrain and the sky's horizon all
// meet in the same colour.
const HAZE_DENSITY: f32 = 0.0002;
// Sea level (voxmar_worldgen::SEA_LEVEL).
const HAZE_BASE: f32 = 128.0;
const HAZE_HEIGHT: f32 = 90.0;

// How much of what lies `offset` from the eye is hidden by haze.
fn fog_amount(offset: vec3<f32>) -> f32 {
    let distance = length(offset);
    // Mean density along the ray, for density falling off exponentially above sea level.
    let start = max(view.world_position.y - HAZE_BASE, 0.0) / HAZE_HEIGHT;
    let end = max(view.world_position.y + offset.y - HAZE_BASE, 0.0) / HAZE_HEIGHT;
    let rise = end - start;
    let density = select((exp(-start) - exp(-end)) / rise, exp(-start), abs(rise) < 1e-3);
    let haze = 1.0 - exp(-HAZE_DENSITY * distance * density);
    let edge = smoothstep(fog.be.x, fog.be.y, distance);
    return 1.0 - (1.0 - haze) * (1.0 - edge);
}

// The haze looking along `direction`: the horizon colour, glowing around the sun with the
// same falloff as the sky (fog.directional_light_color carries the glow's colour).
fn fog_color(direction: vec3<f32>) -> vec3<f32> {
    let toward_sun = max(dot(direction, sun_direction()), 0.0);
    let glow = pow(toward_sun, 48.0) * 0.35 + pow(toward_sun, 6.0) * 0.1;
    return fog.base_color.rgb + fog.directional_light_color.rgb * glow;
}

fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let offset = world - view.world_position;
    return mix(color, fog_color(normalize(offset)), fog_amount(offset));
}
