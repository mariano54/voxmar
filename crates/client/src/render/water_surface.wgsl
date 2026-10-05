// The surface of real water, drawn by the water pass (water.rs) with the scene behind it
// bound for reading: its depth, and its colour so far.
//
// - Waves: a tiling texture of swells (water/waves.rs) in four layers of unrelated sizes and
//   turns drifting with the wind, as high as the wind and the open water around let them
//   grow, so puddles lie still; flowing water carries streaks along its flow.
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
#import voxmar::lighting::{shade_normal, sun_direction, sun_color, sun_shadow, air_between, reflected_cloud}
#import voxmar::water::{absorption, scattering, transmittance, inscattered, fresnel, sky_gradient}

struct Water {
    // Wave layers (water/waves.rs): cos and sin of the layer's turn, 1 / its size in
    // blocks, and its weight.
    layers: array<vec4<f32>, 4>,
    // xy: the texture coordinates of `origin` in each layer.
    phases: array<vec4<f32>, 4>,
    // xz: where wave positions are measured from; y: how windy it is, 0..1; w: how high the
    // waves are on open water (the pack's strength times the wind's).
    origin: vec4<f32>,
    // xz: where flowing water's waves are measured from (snapped more coarsely).
    flow_origin: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // x: absorption, y: scattering, z: refraction (the pack's), w: 1 with the eye under water
    optics: vec4<f32>,
    // x: reflections, y: foam (the pack's), z: 1 for pixelated water, w: waves (the pack's)
    strengths: vec4<f32>,
    // x: wave layers, y: steps of the reflection march, z: halvings that refine a hit
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
// The nearest depth of each 2×2 pixels (water_depth.wgsl), which the reflection march reads.
@group(4) @binding(4) var march_depth: texture_2d<f32>;
// The swells the waves are made of (water/waves.rs): slope along u and v, height.
@group(4) @binding(5) var wave_texture: texture_2d<f32>;
@group(4) @binding(6) var wave_sampler: sampler;

// Texels along a side of the wave texture, and the waves' steepest slope at full height.
const WAVE_TEXELS: f32 = 256.0;
const WAVE_SLOPE: f32 = 0.16;
// How high waves are on water with hardly any open water around it (a puddle, a sheltered
// pool), as a fraction of those on open water.
const SHELTERED: f32 = 0.06;
// Texels per block of the block textures.
const TEXELS: f32 = 16.0;
// Flowing water: its swells are this many blocks across and stretched this much along the
// flow, into streaks; they are carried along in two layers that start again every
// FLOW_CYCLE seconds; their slope at the slowest and fastest flows; and the speed from
// which it froths.
const FLOW_SIZE = vec2(11.0, 4.3);
const FLOW_STRETCH: f32 = 4.0;
const FLOW_CYCLE: f32 = 2.0;
const FLOW_SLOPE = vec2(0.05, 0.16);
const RAPIDS = vec2(2.0, 3.2);
// How far the waves move the point the bed is seen through, in blocks per unit of slope,
// over the first block of depth.
const REFRACTION: f32 = 2.5;
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
// How much of its own colour, lit, water shows however shallow it is, as a glaze over the
// bed (Complementary's water is never quite clear).
const GLAZE: f32 = 0.28;
// How much of the biome colour's saturation the glaze keeps.
const GLAZE_SATURATION: f32 = 0.6;
// Over the first fraction of a block of depth at the very edge of the water, the glaze and
// the reflection fade in, so the shoreline is soft rather than a hard rim.
const EDGE: f32 = 0.06;
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

// Depth of what lies behind a surface at depth `surface` in the pixel at `uv`. With MSAA
// the pixel's samples may straddle the edge of something in front of the water (a bank,
// a plant); the nearest sample that is behind the surface stands for the pixel, where the
// first sample alone would put the bank under the water and draw foam along every edge.
fn depth_behind(uv: vec2<f32>, surface: f32) -> f32 {
#ifdef MULTISAMPLED
    let at = pixel(uv);
    var behind = 0.0;
    var farthest = 1.0;
    for (var i = 0; i < 4; i++) {
        let d = textureLoad(scene_depth, at, i);
        farthest = min(farthest, d);
        if d < surface && d > behind {
            behind = d;
        }
    }
    return select(farthest, behind, behind > 0.0);
#else
    return depth_at(uv);
#endif
}

// The half-resolution depth at `uv`, for the march.
fn coarse_depth_at(uv: vec2<f32>) -> f32 {
    let at = (view.viewport.xy + clamp(uv, vec2(0.0), vec2(1.0)) * view.viewport.zw) * 0.5;
    return textureLoad(march_depth, min(vec2<i32>(at), vec2<i32>(textureDimensions(march_depth)) - 1), 0).r;
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

// The wave texture at `uv`: slope along u and v (the steepest ±1) and height. `texels` is
// how many texels a pixel spans, which picks the mip level, so waves smaller than a pixel
// average out instead of sparkling.
fn swell(uv: vec2<f32>, texels: f32) -> vec3<f32> {
    return textureSampleLevel(wave_texture, wave_sampler, uv, log2(max(texels, 1.0))).xyz;
}

// The slope of the wind's waves on still water at `xz` (d height / d xz), at full height,
// from the first `count` layers. `footprint` is the blocks a pixel spans there. Pixelated
// water (a shader pack's choice, after Complementary's Reimagined style) takes one slope per
// texel of the block textures, so the surface glitters in little squares, until texels are
// smaller than pixels.
fn wave_slope(xz: vec2<f32>, footprint: f32, count: u32) -> vec2<f32> {
    var at = xz;
    if water.strengths.z > 0.5 && footprint < 1.0 / TEXELS {
        at = (floor(xz * TEXELS) + 0.5) / TEXELS;
    }
    let p = at - water.origin.xz;
    var slope = vec2(0.0);
    var weights = 0.0;
    for (var i = 0u; i < count; i++) {
        let layer = water.layers[i];
        let turned = vec2(layer.x * p.x - layer.y * p.y, layer.y * p.x + layer.x * p.y);
        let g = swell(turned * layer.z + water.phases[i].xy, footprint * layer.z * WAVE_TEXELS).xy;
        // Back from the layer's turned axes to x and z.
        slope += vec2(layer.x * g.x + layer.y * g.y, layer.x * g.y - layer.y * g.x) * layer.w;
        weights += layer.w;
    }
    return slope * (WAVE_SLOPE / max(weights, 1e-3));
}

// The slope of flowing water's streaks at `xz`, for a flow of `speed` blocks per second
// along `along`, carried `travelled` blocks: the swells stretched along the flow, so the
// surface reads as running water, as Minecraft's (and the Streams mod's) flowing texture
// does.
fn streak_slope(p: vec2<f32>, along: vec2<f32>, travelled: f32, footprint: f32) -> vec2<f32> {
    let across = vec2(-along.y, along.x);
    let q = vec2(dot(p, along) - travelled, dot(p, across));
    var slope = vec2(0.0);
    for (var i = 0u; i < 2u; i++) {
        let size = FLOW_SIZE[i];
        let uv = vec2(q.x / (size * FLOW_STRETCH), q.y / size) + f32(i) * vec2(0.37, 0.71);
        // Two mip levels softer than the pixel needs: running water reads as long, smooth
        // streaks, not grain.
        let g = swell(uv, footprint / size * WAVE_TEXELS * 4.0).xy;
        let local = vec2(g.x / FLOW_STRETCH, g.y) * (1.0 - 0.45 * f32(i));
        slope += along * local.x + across * local.y;
    }
    return slope / 1.55;
}

// The normal of the waves on a top face at `xz`: the wind's waves, as high as `height`
// (0..1), and where the water flows at `flow` blocks per second, streaks running with it.
// The streaks are carried along in two layers half a cycle apart, each fading out as it
// starts again, so the pattern moves with the water without ever stretching.
fn wave_normal(xz: vec2<f32>, footprint: f32, height: f32, flow: vec2<f32>) -> vec3<f32> {
    var slope = vec2(0.0);
    if height > 0.002 {
        slope = wave_slope(xz, footprint, water.budget.x) * height;
    }
    let speed = length(flow);
    if speed > 0.05 {
        let along = flow / speed;
        let p = xz - water.flow_origin.xz;
        let cycle = fract(globals.time / FLOW_CYCLE);
        let later = fract(cycle + 0.5);
        let weight = 1.0 - abs(2.0 * cycle - 1.0);
        let a = streak_slope(p, along, speed * cycle * FLOW_CYCLE, footprint);
        let b = streak_slope(p + vec2(13.1, 7.7), along, speed * later * FLOW_CYCLE, footprint);
        // Two independent layers mixed lose contrast in the middle; this keeps it.
        let streaks = (a * weight + b * (1.0 - weight)) / sqrt(weight * weight + (1.0 - weight) * (1.0 - weight));
        slope += streaks * mix(FLOW_SLOPE.x, FLOW_SLOPE.y, saturate(speed / 2.8)) * water.strengths.w;
    }
    return normalize(vec3(-slope.x, 1.0, -slope.y));
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
        let scene = coarse_depth_at(ray.xy);
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
    // The march saw the nearest of each 2×2 pixels, which along silhouettes includes
    // things the full-resolution pixel has not: the hit must hold there too.
    let scene = depth_at(ray.xy);
    if abs(scene - ray.z) > max(abs(delta.z) * 8.0, ray.z * ray.z * THICKNESS / near) {
        return vec4(0.0);
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
    let air = air_between(world);
    return Air(air.transmittance, air.light);
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
// along x and z), `falling` whether it falls, and `open_water` how much open water is
// around (0 in a puddle, 1 on a lake), which wind waves need.
fn shade_water(
    world: vec3<f32>,
    normal: vec3<f32>,
    tint: vec3<f32>,
    sky_light: f32,
    frag: vec4<f32>,
    footprint: f32,
    flow: vec2<f32>,
    falling: bool,
    open_water: f32,
) -> vec3<f32> {
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

    // Waves on top: the wind's, as high as the open water around lets them grow and calmer
    // where little sky reaches (in caves), and streaks where it flows, frothing where it
    // runs fast. Where it falls, or pours over an edge, streaks run down the sides, partly
    // white.
    var waves = normal;
    var aerated = 0.0;
    let height = water.origin.w * mix(SHELTERED, 1.0, saturate(open_water)) * (0.3 + 0.7 * open);
    if normal.y > 0.5 {
        // Tilted with the surface where flowing water slopes.
        waves = normalize(wave_normal(world.xz, footprint, height, flow) + normal - vec3(0.0, 1.0, 0.0));
        let froth = smoothstep(RAPIDS.x, RAPIDS.y, length(flow));
        if froth > 0.0 {
            let t = globals.time;
            let along = world.xz - flow * t;
            aerated = froth * 0.35 * smoothstep(0.5, 0.8, foam_noise(along * 7.0) * 0.55 + foam_noise(along * 17.0 + 9.0) * 0.45);
        }
    } else if abs(normal.y) < 0.5 && falling {
        let across = vec2(-normal.z, normal.x);
        let streak = vec2(dot(world.xz, across) * STREAKS.x, (world.y + globals.time * FALL_SPEED) * STREAKS.y);
        let slope = noise_slope(streak) * STREAKS * STREAK_HEIGHT;
        waves = normalize(normal + vec3(across.x, 0.0, across.y) * slope.x + vec3(0.0, slope.y, 0.0));
        let fine = vec2(streak.x * 2.3, streak.y * 0.6 + 17.0);
        aerated = 0.12 + 0.33 * smoothstep(0.3, 0.8, foam_noise(fine));
    } else if abs(normal.y) < 0.5 {
        // The side of water that does not pour (the thin edge of a flow, water against
        // glass) is the edge of its surface, and looks like it rather than like a wall of
        // water catching the sky.
        waves = vec3(0.0, 1.0, 0.0);
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
            // The sky through the window, or what stands above the water where the bent
            // view finds it on screen behind the surface.
            var above = sky_gradient(out, water.zenith.rgb, water.horizon.rgb) * open;
            let bent = project(world + out * 4.0);
            if all(bent.xy == saturate(bent.xy)) && depth_at(bent.xy) < frag.z {
                above = color_at(bent.xy);
            }
            seen_above = mix(above, deep, 1.0 - saturate(dot(out, -normal) * 3.0));
        }
        // The water between the eye and the surface, as the air under water paints it.
        return seen_above * air.through + air.light;
    }

    // What lies behind the surface along the view, and what is seen through the waves: the
    // view through a point of the surface moved by their slope, more so over the first
    // block of depth, so at the very edge of the shore nothing wobbles. A point the move
    // lands on in front of the water is not behind it: there the straight view is kept.
    let surface_depth = frag.z;
    let straight_depth = depth_behind(uv, surface_depth);
    let straight = unproject(vec3(uv, straight_depth));
    var behind_uv = uv;
    var behind_depth = straight_depth;
    let bend = (waves - normal).xz * (REFRACTION * water.optics.z * saturate(world.y - straight.y));
    if dot(bend, bend) > 1e-8 {
        let bent = project(world + vec3(bend.x, 0.0, bend.y));
        let bent_depth = depth_behind(bent.xy, surface_depth);
        if all(bent.xy == saturate(bent.xy)) && bent_depth < surface_depth {
            behind_uv = bent.xy;
            behind_depth = bent_depth;
        }
    }
    let behind = unproject(vec3(behind_uv, behind_depth));
    let path = max(distance(eye, behind) - distance_to_eye, 0.0);
    // Sun and sky light at the surface, and what is left of it at the bed.
    let light = shade_normal(vec3(1.0), world, vec3(0.0, 1.0, 0.0), 1.0, sky, 0.0, 1.0, false, frag.xy);
    // Light reached the bed through the water above it too (the mesher lights the bed as
    // the surface is, leaving the dimming to this).
    let down = max(world.y - behind.y, 0.0);
    let through = transmittance(absorbed, scattered, path + down);
    // The scene behind already has the air between it and the eye; the air in front of the
    // surface is taken back out, the water's effect applied, and the air put back.
    var seen = through * max(color_at(behind_uv) - air.light, vec3(0.0))
        + air.through * inscattered(absorbed, scattered, path, light);
    // A glaze of the water's own colour, lit, however shallow it is.
    let edge = smoothstep(0.0, EDGE, path);
    let glaze = mix(vec3(dot(tint, vec3(0.2126, 0.7152, 0.0722))), tint, GLAZE_SATURATION);
    seen = mix(seen, glaze * light * air.through, GLAZE * edge) + air.light;

    // The reflection: the scene, where the march finds it; otherwise the sky, where sky
    // light reaches, and the dim water around elsewhere.
    var r = reflect(-to_eye, waves);
    let rise = dot(r, normal);
    if rise < 0.02 {
        r = normalize(r + normal * (0.02 - rise));
    }
    let reflectance = fresnel(facing) * water.strengths.x * edge;
    let cloud = reflected_cloud(world, r);
    let sky_color = sky_gradient(r, water.zenith.rgb, water.horizon.rgb) * (1.0 - cloud.a) + cloud.rgb;
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

    // Foam: where waves break on the shore, as the bed comes up to the surface (none on
    // still water), in broken patches that drift and change slowly; where water falls or
    // runs fast. It takes a little of the water's colour.
    var foam = aerated;
    let shore = 1.0 - saturate((world.y - straight.y) / FOAM_DEPTH);
    let surf = saturate(height * 2.5);
    if normal.y > 0.5 && shore > 0.0 && surf > 0.0 {
        let t = globals.time * 0.25;
        let bubbles = foam_noise(world.xz * 2.5 + vec2(t, -t * 0.7)) * 0.65 + foam_noise(world.xz * 9.0 - vec2(t * 1.6, t)) * 0.35;
        foam = max(foam, smoothstep(0.5, 0.8, bubbles) * shore * shore * 0.4 * surf);
    }
    foam = saturate(foam * water.strengths.y);
    if foam > 0.0 {
        let white = mix(vec3(0.85), tint, 0.25) * light;
        color = mix(color, white * air.through + air.light, foam);
    }
    return color;
}
