// Shared terrain lighting: sky ambient, sun or moon with soft cascaded shadows, block light,
// water ripples, and the air between the eye and the surface. Everything dynamic comes
// from the view bind group (directional and ambient light) and the atmosphere uniform, so
// day/night never touches material assets.

#define_import_path voxmar::lighting

#import bevy_pbr::mesh_view_bindings::{view, lights, globals}
#import voxmar::atmosphere::{Atmosphere, Aerial, aerial}
#import voxmar::shadows::soft_shadow

// Bound with every terrain material (terrain.rs, binding 6 of group 2).
@group(2) @binding(6) var<uniform> atmosphere: Atmosphere;

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
// Share of the sunlight on open ground that it scatters back up, on average over grass,
// sand, rock and leaves.
const GROUND_ALBEDO: f32 = 0.3;

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

// Fraction of direct sun reaching `world` on a face with `normal`, from the cascaded
// shadow maps (see shadows.wgsl). Filtering depends on the world position alone, so
// `_frag_xy` (the pixel) is not needed.
fn sun_shadow(world: vec3<f32>, normal: vec3<f32>, _frag_xy: vec2<f32>) -> f32 {
    return soft_shadow(atmosphere, world, normal);
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
    // Sunlight bounced off the sunlit ground around: a wall sees half ground, an underside
    // all of it. Without it the shaded side of a one-block step was lit by the sky alone,
    // several times darker than the sunlit ground beside it, and the corners of terraced
    // beaches and grass ledges stood out as dark teeth.
    let bounce = sun_color() * max(to_sun.y, 0.0) * GROUND_ALBEDO * 0.5 * (1.0 - normal.y) * open * ao
        * atmosphere.ground.w;
    return albedo * (ambient + bounce + direct * mix(1.0, ao, 0.5) + torch + floor);
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

// The air between the eye and `world`: haze, ground fog and the weather's fog (see
// atmosphere.wgsl), as how much of what lies there shows through and the light the air
// adds on the way.
fn air_between(world: vec3<f32>) -> Aerial {
    return aerial(atmosphere, view.world_position, world - view.world_position, sun_direction());
}

// `color` at `world` seen through the air. Real chunks, distant terrain, water and the
// sky's horizon all fade into the same colours.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let air = air_between(world);
    return color * air.transmittance + air.light;
}
