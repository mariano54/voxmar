// Shared terrain lighting: warm sun or moon with soft cascaded shadows, cool sky light that
// falls off steeply into shade, ambient occlusion, block light, leaves glowing against
// the sun, water ripples, and the air between the eye and the surface. The sun, sky light
// and shadows come from the view bind group (directional and ambient light) and the
// atmosphere uniform, so day and night never touch material assets.

#define_import_path voxmar::lighting

#import bevy_pbr::mesh_view_bindings::{view, lights, globals}
#import voxmar::atmosphere::{Atmosphere, Aerial, aerial, cloud_across, cloud_light, cloud_seen, dusk_tint, far_color, water_glow}
#import voxmar::clouds::value_noise
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
// shadow maps (see shadows.wgsl): with soft edges as wide as the caster is far, or, not
// `soft`, the maps' own edges, a quarter of the cost.
fn sun_shadow(world: vec3<f32>, normal: vec3<f32>, soft: bool) -> f32 {
    return soft_shadow(atmosphere, world, normal, soft);
}

// Lit colour of a voxel face. `sky` and `block` are 0..1 light levels, `ao` the occlusion
// factor, `foliage` lights leaves and plants as thin and translucent, and `soft` picks
// soft-edged shadows (see `sun_shadow`).
fn shade(
    albedo: vec3<f32>,
    world: vec3<f32>,
    face: u32,
    sky: f32,
    block: f32,
    ao: f32,
    foliage: bool,
    soft: bool,
) -> vec3<f32> {
    return shade_normal(albedo, world, NORMALS[face], FACE_SHADE[face], sky, block, ao, foliage, soft);
}

// The block light a light held in the hand gives `world` (`atmosphere.held`): its level less
// one a block, as light spreads, on faces turned towards it (none through a wall's back).
fn held_light(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    let held = atmosphere.held;
    if held.w <= 0.0 {
        return 0.0;
    }
    let to_light = held.xyz - world;
    let reach = held.w - length(to_light) / 15.0;
    let facing = smoothstep(-0.25, 0.25, dot(normal, normalize(to_light + vec3(0.0, 1e-4, 0.0))));
    return max(reach, 0.0) * facing;
}

// Lit colour of any surface with a normal and an ambient shading factor, its shadows
// `soft` or not (see `sun_shadow`).
fn shade_normal(
    albedo: vec3<f32>,
    world: vec3<f32>,
    normal: vec3<f32>,
    ambient_shade: f32,
    sky: f32,
    block: f32,
    ao: f32,
    foliage: bool,
    soft: bool,
) -> vec3<f32> {
    let to_sun = sun_direction();
    var facing = dot(normal, to_sun);
    if foliage {
        facing = 0.35 + 0.65 * abs(facing);
    }
    // Sunlight only reaches places open to the sky (never into caves, whatever the shadow
    // maps missed); the shadow maps add the exact shapes, and passing clouds their shade.
    let open = smoothstep(0.35, 0.75, sky);
    var sunlit = 0.0;
    if facing > 0.0 && open > 0.0 {
        sunlit = open * sun_shadow(world, normal, soft);
        if sunlit > 0.0 {
            sunlit *= cloud_light(atmosphere, view.world_position, world, to_sun);
        }
    }
    let direct = sun_color() * facing * sunlit;
    // Sky light falls off steeply with the sky light level, so forest floors, overhangs and
    // the inside of crowns are much darker than open ground; occlusion grounds corners.
    let occlusion = pow(ao, atmosphere.surface.y);
    // At dusk and dawn, faces turned towards the sun take the warmer sky on its side, those
    // turned away the cooler one.
    let ambient = lights.ambient_color.rgb * dusk_tint(atmosphere, normal * 0.5) * pow(sky, atmosphere.surface.x) * ambient_shade * occlusion;
    let torch = BLOCK_LIGHT_COLOR * light_curve(max(block, held_light(world, normal))) * 1.4 * mix(ambient_shade, 1.0, 0.5) * ao;
    let floor = vec3(0.010, 0.010, 0.014) * ao;
    // Sunlight bounced off the sunlit ground around: a wall sees half ground, an underside
    // all of it. Without it the shaded side of a one-block step was lit by the sky alone,
    // several times darker than the sunlit ground beside it, and the corners of terraced
    // beaches and grass ledges stood out as dark teeth.
    let bounce = sun_color() * max(to_sun.y, 0.0) * GROUND_ALBEDO * 0.5 * (1.0 - normal.y) * open * ao
        * atmosphere.ground.w;
    var light = ambient + bounce + direct * mix(1.0, ao, 0.3) + torch + floor;
    if foliage && open > 0.0 && atmosphere.surface.z > 0.0 {
        // Sunlight through leaves and blades, brightest looking towards the sun. A leaf in
        // the shade of the crown it belongs to still glows where only a leaf or so lies
        // between it and the sun, as Complementary's leaves do: its shadow is looked up a
        // block and a quarter towards the sun, past the leaves in front of it, so the rim
        // and underside of a crown glow green against the light and its heart stays dark.
        var glow = sunlit;
        if sunlit < 0.5 {
            let through = 0.6 * open * sun_shadow(world + to_sun * 1.25, to_sun, false);
            if through > glow {
                glow = through * cloud_light(atmosphere, view.world_position, world, to_sun);
            }
        }
        if glow > 0.0 {
            let to_eye = normalize(view.world_position - world);
            let through = pow(max(dot(-to_eye, to_sun), 0.0), 4.0);
            light += sun_color() * glow * atmosphere.surface.z * (0.15 + through);
        }
    }
    return albedo * light;
}

// How much darker soaked ground is than dry (it reads darker and a little glossier, never
// paler).
const WET_DARKENING: f32 = 0.3;
// How much brighter than the soaked ground its gloss may make it, at most.
const WET_GLOSS: f32 = 1.15;

// A surface after the weather: darker where rain has wet it, darker still in puddles on
// open level ground (which rise and dry with the wetness), whitened by frost, and dusted
// with snow on top once enough lies around (the server turns grass snowy itself). Returns
// the albedo, and in w how wet it is (1 in a puddle), for `wet_sheen`.
fn weathered(albedo: vec3<f32>, world: vec3<f32>, normal: vec3<f32>, sky: f32) -> vec4<f32> {
    let wetness = atmosphere.ground.x;
    let frost = atmosphere.ground.y;
    let snow = atmosphere.ground.z;
    if wetness <= 0.0 && frost <= 0.0 && snow <= 0.0 {
        return vec4(albedo, 0.0);
    }
    // Rain and frost settle where the sky is open, most on top faces.
    let exposed = smoothstep(0.8, 1.0, sky);
    var wet = wetness * exposed * saturate(0.35 + 0.65 * normal.y);
    if normal.y > 0.9 && wetness > 0.0 {
        let patches = value_noise(world.xz * 0.21) * 0.65 + value_noise(world.xz * 0.53 + 7.3) * 0.35;
        let level = 0.75 - 0.3 * wetness;
        wet = max(wet, smoothstep(level, level + 0.04, patches) * smoothstep(0.3, 0.6, wetness) * exposed);
    }
    // Wet ground is darker, as soaked soil, sand and stone are (the saturation lift here
    // goes back into the gloss `wet_sheen` lays in the ground's own colour).
    var color = albedo * (1.0 - WET_DARKENING * min(wet, 1.0));
    let grey = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = max(mix(vec3(grey), color, 1.0 + 0.3 * min(wet, 1.0)), vec3(0.0));
    if frost > 0.0 {
        let rime = frost * exposed * saturate(normal.y * 0.8 + 0.2) * smoothstep(0.35, 0.6, value_noise(world.xz * 1.7 + world.y));
        color = mix(color, vec3(0.78, 0.84, 0.9), rime * 0.6);
    }
    // The dusting thins drift by drift as the cover falls, never all at once.
    if snow > 0.0 && normal.y > 0.9 {
        let drifts = value_noise(world.xz * 0.37 + 3.1) * 0.7 + value_noise(world.xz * 1.9) * 0.3;
        let dusting = smoothstep(1.0 - snow, 1.15 - snow, drifts) * exposed;
        color = mix(color, vec3(0.9, 0.93, 0.96), dusting * 0.85);
    }
    return vec4(color, wet);
}

// The sky mirrored by a wet surface, most at a glancing angle and in puddles. A puddle's
// surface ripples (and rings where rain falls on it), so what it mirrors moves from pixel
// to pixel and breaks up; it mirrors the clouds as well as the sky. A flat mirror of the
// sky's colour alone made every puddle one even tint, the same at every point
// (Mariano's playtest, 2026-10-06).
fn wet_sheen(color: vec3<f32>, world: vec3<f32>, normal: vec3<f32>, wet: f32, sky: f32) -> vec3<f32> {
    if wet <= 0.0 {
        return color;
    }
    let to_eye = normalize(view.world_position - world);
    let puddle = smoothstep(0.6, 1.0, wet);
    var surface = normal;
    if puddle > 0.0 && normal.y > 0.9 {
        // How hard it rains (the storm term of the cloud layer: falling rain, more in storms).
        let rain = saturate(atmosphere.cloud_shape.w * 2.0);
        surface = normalize(mix(normal, water_normal(world * 2.5), puddle) + vec3(raindrop_rings(world.xz, rain), 0.0).xzy * puddle);
    }
    let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(surface, to_eye), 0.0), 5.0);
    let mirrored = reflect(-to_eye, surface);
    var reflection = far_color(atmosphere, view.world_position, mirrored, sun_direction());
    if puddle > 0.0 {
        let cloud = reflected_cloud(world, mirrored);
        reflection = reflection * (1.0 - cloud.a) + cloud.rgb;
    }
    // Soaked ground mirrors the sky's brightness in its own colour, a gloss; a puddle, thin
    // water over the ground, half in the sky's colour. Sky blue mixed into red sand turned
    // it pink.
    let luminance = vec3(0.2126, 0.7152, 0.0722);
    let in_own_colour = dot(reflection, luminance) * color / max(dot(color, luminance), 1e-4);
    reflection = mix(in_own_colour, reflection, 0.5 * puddle);
    let sheened = mix(color, reflection * smoothstep(0.5, 0.9, sky), fresnel * mix(0.35 * wet, 0.6, puddle));
    // Never pale: a glancing sheen of bright sky over sand or gravel read as frost. What it
    // mirrors still shows, as a gloss a little brighter than the soaked ground, which keeps
    // its colour.
    let soaked = dot(color, vec3(0.2126, 0.7152, 0.0722)) * WET_GLOSS;
    let bright = dot(sheened, vec3(0.2126, 0.7152, 0.0722));
    return sheened * min(1.0, soaked / max(bright, 1e-4));
}

// The slope rings that falling rain spreads over a puddle at `xz`, `rain` 0..1: on a grid of
// cells half a block across, each a drop's ring at its own moment, widening and fading.
fn raindrop_rings(xz: vec2<f32>, rain: f32) -> vec2<f32> {
    if rain <= 0.0 {
        return vec2(0.0);
    }
    let cell = floor(xz * 2.0);
    let within = fract(xz * 2.0) - 0.5;
    let h = fract(sin(dot(cell, vec2(127.1, 311.7))) * 43758.5453);
    let t = fract(globals.time * (0.6 + 0.8 * h) + h * 7.0);
    let offset = vec2(fract(h * 13.0), fract(h * 29.0)) * 0.4 - 0.2;
    let d = within - offset;
    let r = length(d);
    let ring = sin((r - t * 0.45) * 40.0) * smoothstep(0.08, 0.0, abs(r - t * 0.45)) * (1.0 - t);
    return select(vec2(0.0), d / max(r, 1e-3) * ring * 0.25 * rain, h < rain);
}

// Light a glowing block gives off by itself (`glow` 0..1, from its light level): its bright
// texels, as the lit part of a lantern or the cracks of magma, glow brightly enough to
// bloom at full strength; its dark texels do not glow.
fn self_glow(albedo: vec3<f32>, texel: vec3<f32>, glow: f32) -> vec3<f32> {
    let bright = smoothstep(0.2, 0.6, dot(texel, vec3(0.2126, 0.7152, 0.0722)));
    return albedo * glow * bright * atmosphere.surface.w;
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

// The low cloud seen from `world` along `direction`, for the sky in reflections, in
// premultiplied colour and opacity.
fn reflected_cloud(world: vec3<f32>, direction: vec3<f32>) -> vec4<f32> {
    return cloud_seen(atmosphere, view.world_position, world, direction, sun_direction(), sun_color(), lights.ambient_color.rgb);
}

// How much of the sun's disc the low cloud leaves in view from `world`. A glint off water
// is the sun itself, mirrored, so it hides behind cloud as the sun does, not just dims as
// the light beneath does.
fn sun_unclouded(world: vec3<f32>) -> f32 {
    return 1.0 - cloud_across(atmosphere, view.world_position, world, sun_direction()).x;
}

// The horizon's colour `horizon` looking along `direction`, warmer towards the low sun
// and cooler away from it (`dusk_tint`).
fn horizon_along(horizon: vec3<f32>, direction: vec3<f32>) -> vec3<f32> {
    return horizon * dusk_tint(atmosphere, direction);
}

// Under water, what the water scatters towards the eye along a ray heading `direction`
// from just under its surface, as far as it goes.
fn glow_below_surface(direction: vec3<f32>) -> vec3<f32> {
    return water_glow(atmosphere, 0.0, direction.y, 1e4);
}

// `color` at `world` seen through the air. Real chunks, distant terrain, water and the
// sky's horizon all fade into the same colours.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let air = air_between(world);
    return color * air.transmittance + air.light;
}
