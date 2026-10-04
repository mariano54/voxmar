// The surface of real water, drawn by the water pass (water.rs) with the scene behind it
// bound for reading: its depth, and its colour so far.
//
// - Waves: octaves of afl_ext's exp(sin) waves, each dragging the next along its slope
//   ("Very fast procedural ocean", https://www.shadertoy.com/view/MdXyzX, MIT, as adapted
//   in Glimmer's waveNormals.glsl by jbritain, MIT). Their directions, frequencies and
//   phases come from water.rs, where the wind turns them; here the slope is summed
//   analytically in one pass instead of three height samples, and octaves shorter than a
//   few pixels fade out so distant water does not sparkle.
// - What lies behind: how far the view travels through water to the scene behind, so
//   shallows are clear and deep water takes its colour; the bed bent by the waves; the
//   light reaching the bed dimmed by the water above it.
// - Reflections: a march through the depth buffer in screen space (after Glimmer's
//   screenSpaceRayTrace.glsl, jbritain, MIT, itself after Belmu), with a sky gradient
//   where the reflection leaves the screen; dark where no sky reaches.
// - Foam where the surface meets the shore.
// See THIRD_PARTY_NOTICES.md for the licences.

#define_import_path voxmar::water_surface

#import bevy_pbr::mesh_view_bindings::{view, globals}
#import voxmar::lighting::{shade_normal, sun_direction, sun_color, sun_shadow, fog_amount, fog_color, water_normal}
#import voxmar::water::{absorption, scattering, transmittance, inscattered, fresnel, sky_gradient}

struct Water {
    // Wave octaves: xy the direction crests travel, z radians per block, w phase at
    // `origin` now.
    waves: array<vec4<f32>, 16>,
    // Weight of each octave; the octaves in use sum to one.
    weights: array<vec4<f32>, 4>,
    // xz: where wave positions are measured from; w: how high the waves are (the pack's
    // strength times the wind's).
    origin: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // x: absorption, y: scattering, z: refraction (the pack's), w: 1 with the eye under water
    optics: vec4<f32>,
    // x: reflections, y: foam (the pack's), z: 1 for pixelated water
    strengths: vec4<f32>,
    // x: wave octaves, y: steps of the reflection march, z: halvings that refine a hit
    budget: vec4<u32>,
};

#ifdef MULTISAMPLED
@group(4) @binding(0) var scene_depth: texture_depth_multisampled_2d;
#else
@group(4) @binding(0) var scene_depth: texture_depth_2d;
#endif
@group(4) @binding(1) var scene_color: texture_2d<f32>;
@group(4) @binding(2) var scene_sampler: sampler;
@group(4) @binding(3) var<uniform> water: Water;

// How far each wave drags the next along its slope (afl_ext's 0.2 to 0.38 for weights that
// start at one; ours sum to one).
const DRAG: f32 = 1.0;
// Height of the waves at strength one, in blocks, and of the ripples over them.
const WAVE_HEIGHT: f32 = 0.14;
const RIPPLE_HEIGHT: f32 = 0.05;
// Ripples per block.
const RIPPLE_SCALE: f32 = 2.5;
// Texels per block of the block textures.
const TEXELS: f32 = 16.0;
// Flowing water: seconds its waves are carried along before a layer starts again, octaves
// per layer (it draws two), how much choppier it is per block per second of speed, and the
// speed from which it froths.
const FLOW_CYCLE: f32 = 1.6;
const FLOWING_OCTAVES: u32 = 10u;
const FLOW_CHOP: f32 = 0.35;
const RAPIDS = vec2(1.6, 2.8);
// How far the waves bend the view through the surface.
const BEND: f32 = 0.6;
// Furthest the bent view reaches beyond the surface, in blocks, so a far bed does not
// swing across the screen.
const BEND_REACH: f32 = 6.0;
// Depth thickness assumed of what a reflected ray passes behind, in blocks.
const THICKNESS: f32 = 1.5;
// Each step of the reflection march is this much longer than the last.
const STEP_GROWTH: f32 = 1.15;
// Glints: a sharp highlight and a broader sheen around it.
const GLINT = vec2(1600.0, 160.0);
const GLINT_STRENGTH = vec2(40.0, 0.6);
// Reflections weaker than this skip the march; it is all but invisible.
const MIN_REFLECTION: f32 = 0.03;
// Foam lines shores where the water is shallower than this, in blocks.
const FOAM_DEPTH: f32 = 0.3;
// Falling water: how fast its streaks run down, in blocks per second, how many there are
// per block across and along the fall, and how deep they are.
const FALL_SPEED: f32 = 7.0;
const STREAKS = vec2(5.0, 0.8);
const STREAK_HEIGHT: f32 = 0.04;

// Screen position (uv over the viewport, reverse-Z depth) of a world position.
fn project(world: vec3<f32>) -> vec3<f32> {
    let clip = view.clip_from_world * vec4(world, 1.0);
    let ndc = clip.xyz / clip.w;
    return vec3(ndc.xy * vec2(0.5, -0.5) + 0.5, ndc.z);
}

// World position at a screen position; depth 0 (the sky) is taken as very far.
fn unproject(screen: vec3<f32>) -> vec3<f32> {
    let ndc = vec3((screen.xy - 0.5) * vec2(2.0, -2.0), max(screen.z, 1e-7));
    let world = view.world_from_clip * vec4(ndc, 1.0);
    return world.xyz / world.w;
}

fn pixel(uv: vec2<f32>) -> vec2<i32> {
    let at = view.viewport.xy + clamp(uv, vec2(0.0), vec2(1.0)) * view.viewport.zw;
    return min(vec2<i32>(at), vec2<i32>(view.viewport.xy + view.viewport.zw) - 1);
}

// Depth of the scene behind the water (reverse-Z: 1 at the eye, 0 at infinity).
fn depth_at(uv: vec2<f32>) -> f32 {
    return textureLoad(scene_depth, pixel(uv), 0);
}

fn color_at(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(scene_color));
    let at = (view.viewport.xy + clamp(uv, vec2(0.0), vec2(1.0)) * view.viewport.zw) / size;
    return textureSampleLevel(scene_color, scene_sampler, at, 0.0).rgb;
}

// Integer hash of a lattice cell, 0..1.
fn lattice(cell: vec2<i32>) -> f32 {
    var h = (u32(cell.x) * 0x8da6b343u) ^ (u32(cell.y) * 0xd8163841u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h ^= h >> 12u;
    return f32(h >> 8u) / 16777216.0;
}

// Value noise's gradient at `p`.
fn noise_slope(p: vec2<f32>) -> vec2<f32> {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let c = vec2<i32>(i);
    let a = lattice(c);
    let b = lattice(c + vec2(1, 0));
    let d = lattice(c + vec2(0, 1));
    let e = lattice(c + vec2(1, 1));
    let k = a - b - d + e;
    return du * vec2(b - a + k * u.y, d - a + k * u.x);
}

// Small ripples over the waves: two layers of noise drifting across each other, along and
// against the longest wave.
fn ripple_slope(p: vec2<f32>, footprint: f32) -> vec2<f32> {
    let fade = saturate(2.0 - RIPPLE_SCALE * 2.0 * footprint * 0.64);
    if fade <= 0.0 {
        return vec2(0.0);
    }
    let t = globals.time;
    let along = water.waves[0].xy;
    let first = noise_slope(p * RIPPLE_SCALE + along * (t * 0.7)) * RIPPLE_SCALE;
    // The second layer turned by 37 degrees, so the two lattices never line up.
    let turn = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
    let second = transpose(turn) * noise_slope(turn * p * (RIPPLE_SCALE * 2.0) - along * (t * 1.1)) * (RIPPLE_SCALE * 2.0);
    return (first + second * 0.5) * fade;
}

// The slope of the waves at `xz` (d height / d xz), from the first `octaves` octaves.
// `footprint` is the blocks a pixel spans there: octaves shorter than about three pixels
// fade out, and the loop stops at the first that has. Pixelated water (a shader pack's
// choice, after Complementary's Reimagined style) takes one slope per texel of the block
// textures, so the surface glitters in little squares, until texels are smaller than pixels.
fn wave_slope(xz: vec2<f32>, footprint: f32, octaves: u32) -> vec2<f32> {
    var at = xz;
    if water.strengths.z > 0.5 && footprint < 1.0 / TEXELS {
        at = (floor(xz * TEXELS) + 0.5) / TEXELS;
    }
    var p = at - water.origin.xz;
    var slope = vec2(0.0);
    for (var i = 0u; i < octaves; i++) {
        let wave = water.waves[i];
        let fade = saturate(2.0 - wave.z * footprint * 0.64);
        if fade <= 0.0 {
            break;
        }
        let weight = water.weights[i / 4u][i % 4u];
        let x = dot(wave.xy, p) * wave.z + wave.w;
        let height = exp(sin(x) - 1.0);
        let dx = height * cos(x);
        p -= wave.xy * (dx * weight * DRAG);
        slope += wave.xy * (dx * wave.z * weight * fade);
    }
    return slope * WAVE_HEIGHT + ripple_slope(at, footprint) * RIPPLE_HEIGHT;
}

// The normal of the waves on a top face at `xz`, flowing at `flow` blocks per second.
// Flowing water carries its waves along in two layers half a cycle apart, each fading out
// as it starts again, so the pattern moves with the water without ever stretching; it is
// choppier the faster it runs.
fn wave_normal(xz: vec2<f32>, footprint: f32, strength: f32, flow: vec2<f32>) -> vec3<f32> {
    let octaves = water.budget.x;
    if octaves == 0u {
        return water_normal(vec3(xz.x, 0.0, xz.y));
    }
    let speed = length(flow);
    var slope: vec2<f32>;
    if speed < 0.05 {
        slope = wave_slope(xz, footprint, octaves);
    } else {
        let cycle = fract(globals.time / FLOW_CYCLE);
        let later = fract(cycle + 0.5);
        let weight = 1.0 - abs(2.0 * cycle - 1.0);
        let fewer = min(octaves, FLOWING_OCTAVES);
        let a = wave_slope(xz - flow * (cycle * FLOW_CYCLE), footprint, fewer);
        let b = wave_slope(xz - flow * (later * FLOW_CYCLE), footprint, fewer);
        // Two independent layers mixed lose contrast in the middle; this keeps it.
        slope = (a * weight + b * (1.0 - weight)) / sqrt(weight * weight + (1.0 - weight) * (1.0 - weight));
        slope *= 1.0 + speed * FLOW_CHOP;
    }
    let s = slope * (water.origin.w * strength);
    return normalize(vec3(-s.x, 1.0, -s.y));
}

// Marches `direction` from `origin` through the depth buffer in screen space, where a
// straight line stays straight (uv and reverse-Z depth are both affine in 1/z), in steps
// that lengthen as they go, then halves its way back to the surface it passed behind.
// Returns the hit's uv and depth, and in w how far to trust it: 0 for no hit, fading out
// towards the edges of the screen.
fn trace(origin: vec3<f32>, direction: vec3<f32>) -> vec4<f32> {
    let steps = water.budget.y;
    if steps == 0u {
        return vec4(0.0);
    }
    let near = view.clip_from_view[3][2];
    let o = (view.view_from_world * vec4(origin, 1.0)).xyz;
    let d = (view.view_from_world * vec4(direction, 0.0)).xyz;
    // Coming back towards the eye, the ray must stay in front of the near plane.
    var reach = 1.0;
    if d.z > 0.0 {
        reach = min(reach, 0.5 * (-o.z - near) / d.z);
        if reach <= 0.0 {
            return vec4(0.0);
        }
    }
    let start = project(origin);
    let along = project(origin + direction * reach) - start;
    // How many `along`s to the edge of the screen, or to infinity (depth 0).
    let to_far = select((vec3(1.0) - start) / along, -start / along, along < vec3(0.0));
    let span = min(min(select(1e6, to_far.x, along.x != 0.0), select(1e6, to_far.y, along.y != 0.0)), select(1e6, to_far.z, along.z != 0.0));
    // Steps grow geometrically to cover the whole span.
    let total = (pow(STEP_GROWTH, f32(steps)) - 1.0) / (STEP_GROWTH - 1.0);
    var delta = along * (span / total);
    var ray = start;
    var hit = false;
    for (var i = 0u; i < steps; i++) {
        ray += delta;
        let scene = depth_at(ray.xy);
        // Passed behind something, but not so far behind that it went past it.
        let slack = max(abs(delta.z) * 2.0, ray.z * ray.z * THICKNESS / near);
        if scene > ray.z && scene - ray.z < slack {
            hit = true;
            break;
        }
        delta *= STEP_GROWTH;
    }
    if !hit {
        return vec4(0.0);
    }
    for (var i = 0u; i < water.budget.z; i++) {
        delta *= 0.5;
        ray += select(delta, -delta, depth_at(ray.xy) > ray.z);
    }
    let edge = max(abs(ray.x - 0.5), abs(ray.y - 0.5)) * 2.0;
    return vec4(ray, 1.0 - smoothstep(0.85, 1.0, edge));
}

// The air between the eye and a point: how much of the point shows through it, and the
// light it adds on the way.
struct Air {
    through: f32,
    light: vec3<f32>,
};

fn air_to(world: vec3<f32>) -> Air {
    let offset = world - view.world_position;
    let haze = fog_amount(offset);
    return Air(1.0 - haze, haze * fog_color(normalize(offset)));
}

// A smooth noise for foam.
fn foam_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let h = vec4(
        fract(sin(dot(i, vec2(127.1, 311.7))) * 43758.5453),
        fract(sin(dot(i + vec2(1.0, 0.0), vec2(127.1, 311.7))) * 43758.5453),
        fract(sin(dot(i + vec2(0.0, 1.0), vec2(127.1, 311.7))) * 43758.5453),
        fract(sin(dot(i + vec2(1.0, 1.0), vec2(127.1, 311.7))) * 43758.5453),
    );
    return mix(mix(h.x, h.y, w.x), mix(h.z, h.w, w.x), w.y);
}

// The colour of water at `world`, fog included: the scene behind seen through it, the
// reflection, a sun glint and foam. `normal` is the face's, `tint` the biome's water
// colour, `sky` the sky light at the surface (0..1), `frag` the fragment's position and
// `footprint` how many blocks its pixel spans, `flow` the water's flow (blocks per second
// along x and z) and `falling` whether it falls.
fn shade_water(world: vec3<f32>, normal: vec3<f32>, tint: vec3<f32>, sky_light: f32, frag: vec4<f32>, footprint: f32, flow: vec2<f32>, falling: bool) -> vec3<f32> {
    // Saturated: seen edge-on, a face interpolates at pixel centres outside it.
    let sky = saturate(sky_light);
    let eye = view.world_position;
    let to_eye = normalize(eye - world);
    let uv = (frag.xy - view.viewport.xy) / view.viewport.zw;
    let absorbed = absorption(tint, water.optics.x);
    let scattered = scattering(tint, water.optics.y);
    let open = smoothstep(0.5, 0.95, sky);
    let distance_to_eye = distance(eye, world);
    let air = air_to(world);

    // Waves on top, calmer where little sky reaches (in caves), frothing where it runs fast.
    // Where it falls, or pours over an edge, streaks run down the sides, partly white.
    var waves = normal;
    var aerated = 0.0;
    if normal.y > 0.5 {
        // Tilted with the surface where flowing water slopes.
        waves = normalize(wave_normal(world.xz, footprint, 0.3 + 0.7 * open, flow) + normal - vec3(0.0, 1.0, 0.0));
        let froth = smoothstep(RAPIDS.x, RAPIDS.y, length(flow));
        if froth > 0.0 {
            let t = globals.time;
            let along = world.xz - flow * t;
            aerated = froth * smoothstep(0.45, 0.8, foam_noise(along * 2.0) * 0.6 + foam_noise(along * 6.0 + 9.0) * 0.4);
        }
    } else if abs(normal.y) < 0.5 && (falling || dot(flow, normal.xz) > 0.5) {
        let across = vec2(-normal.z, normal.x);
        let streak = vec2(dot(world.xz, across) * STREAKS.x, (world.y + globals.time * FALL_SPEED) * STREAKS.y);
        let slope = noise_slope(streak) * STREAKS * STREAK_HEIGHT;
        waves = normalize(normal + vec3(across.x, 0.0, across.y) * slope.x + vec3(0.0, slope.y, 0.0));
        let fine = vec2(streak.x * 2.3, streak.y * 0.6 + 17.0);
        aerated = 0.25 + 0.5 * smoothstep(0.3, 0.8, foam_noise(fine));
    }
    let facing = dot(waves, to_eye);

    // Under water, looking up at the surface: inside Snell's window the world above shows,
    // bent; outside it the surface mirrors the deep water below.
    if water.optics.w > 0.5 && dot(to_eye, normal) < 0.0 {
        let out = refract(-to_eye, -waves, 1.33);
        let light = shade_normal(vec3(1.0), world, vec3(0.0, 1.0, 0.0), 1.0, sky, 0.0, 1.0, false, frag.xy);
        let deep = inscattered(absorbed, scattered, 1e4, light);
        var seen_above = deep;
        if dot(out, out) > 0.0 {
            let bent = project(world + out * 4.0);
            seen_above = mix(color_at(bent.xy), deep, 1.0 - saturate(dot(out, -normal) * 3.0));
        }
        // The water between the eye and the surface, as the air under water paints it.
        return seen_above * air.through + air.light;
    }

    // What lies behind the surface along the view, and the same along the view bent by the
    // waves, unless what is there is nearer than the surface.
    let surface_depth = frag.z;
    let straight = unproject(vec3(uv, depth_at(uv)));
    let thickness = max(distance(eye, straight) - distance_to_eye, 0.0);
    var behind_uv = uv;
    let bend = (waves - normal) * (BEND * water.optics.z);
    if dot(bend, bend) > 1e-8 {
        let bent = project(world + normalize(-to_eye + bend) * min(thickness, BEND_REACH));
        if all(bent.xy == saturate(bent.xy)) && depth_at(bent.xy) < surface_depth {
            behind_uv = bent.xy;
        }
    }
    let behind = unproject(vec3(behind_uv, depth_at(behind_uv)));
    let path = max(distance(eye, behind) - distance_to_eye, 0.0);
    // Sun and sky light at the surface, and what is left of it at the bed.
    let light = shade_normal(vec3(1.0), world, vec3(0.0, 1.0, 0.0), 1.0, sky, 0.0, 1.0, false, frag.xy);
    let down = max(world.y - behind.y, 0.0);
    let through = transmittance(absorbed, scattered, path + down);
    // The scene behind already has the air between it and the eye; the air in front of the
    // surface is taken back out, the water's effect applied, and the air put back.
    let seen = through * max(color_at(behind_uv) - air.light, vec3(0.0))
        + air.through * inscattered(absorbed, scattered, path, light)
        + air.light;

    // The reflection: the scene, where the march finds it; otherwise the sky, where sky
    // light reaches, and the dim water around elsewhere.
    var r = reflect(-to_eye, waves);
    let rise = dot(r, normal);
    if rise < 0.02 {
        r = normalize(r + normal * (0.02 - rise));
    }
    // Thin water at the very edge of the shore hardly reflects, so the edge stays soft.
    let reflectance = fresnel(facing) * water.strengths.x * smoothstep(0.0, 0.3, path);
    let sky_color = sky_gradient(r, water.zenith.rgb, water.horizon.rgb);
    var reflected = mix(seen, sky_color * air.through + air.light, open);
    if reflectance > MIN_REFLECTION {
        let hit = trace(world + normal * 0.02, r);
        // Nothing under the surface can be reflected.
        if hit.w > 0.0 && dot(unproject(hit.xyz) - world, normal) > -0.05 {
            let trust = hit.w * smoothstep(MIN_REFLECTION, MIN_REFLECTION * 2.0, reflectance);
            reflected = mix(reflected, color_at(hit.xy), trust);
        }
    }

    // The sun's glint off the waves, as far as the sun reaches the surface (looked up only
    // near the glint, where it can show).
    let toward_sun = max(dot(reflect(-to_eye, waves), sun_direction()), 0.0);
    var glint = 0.0;
    if toward_sun > 0.95 && open > 0.0 {
        glint = (pow(toward_sun, GLINT.x) * GLINT_STRENGTH.x + pow(toward_sun, GLINT.y) * GLINT_STRENGTH.y)
            * open * sun_shadow(world, normal, frag.xy) * water.strengths.x;
    }
    var color = mix(seen, reflected, reflectance) + sun_color() * glint * air.through;

    // Foam along the shore, where the bed comes up to the surface: broken patches that
    // drift and change slowly, thickest right at the edge. Falling and fast water froths.
    var foam = aerated;
    let shore = 1.0 - saturate((world.y - straight.y) / FOAM_DEPTH);
    if normal.y > 0.5 && shore > 0.0 {
        let t = globals.time * 0.25;
        let bubbles = foam_noise(world.xz * 2.5 + vec2(t, -t * 0.7)) * 0.65 + foam_noise(world.xz * 9.0 - vec2(t * 1.6, t)) * 0.35;
        foam = max(foam, smoothstep(0.45, 0.75, bubbles) * shore * shore * 0.5);
    }
    foam = saturate(foam * water.strengths.y);
    if foam > 0.0 {
        color = mix(color, (light * 0.8 + 0.02) * air.through + air.light, foam);
    }
    return color;
}
