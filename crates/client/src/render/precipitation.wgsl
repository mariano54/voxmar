// Rain streaks, drizzle, snowflakes, sleet and hail, and dust and glowing motes on the
// wind, in a box that follows the camera (see precipitation.rs).
// Each vertex is a drop number times four plus a corner; everything else is derived here.

#import bevy_pbr::mesh_view_bindings::view

struct Params {
    // xyz: the camera's position within the box (wrapped), w: seconds
    camera: vec4<f32>,
    // xz: the camera's position from the height map's corner, w: its height
    map: vec4<f32>,
    // xyz: how far rain has fallen and drifted (wrapped into the box), w: amount 0..1
    rain: vec4<f32>,
    // xyz: how far snow has fallen less where the camera is (wrapped into its box), w: the
    // fraction falling as snow
    snow: vec4<f32>,
    // xyz: direction rain falls in, w: streak length in blocks
    streak: vec4<f32>,
    // rgb: light falling on the drops
    light: vec4<f32>,
    // rgb: the colour of the sky's haze, which rain takes on in part
    haze: vec4<f32>,
    // xyz: how far the distant layer of rain has fallen less where the camera is (wrapped
    // into a box three times as wide), w: how many drips fall from leaves
    far: vec4<f32>,
    // Of the rain that is not snow: xyz how far drizzle, sleet and hail have fallen, w the
    // fraction that is each
    drizzle: vec4<f32>,
    sleet: vec4<f32>,
    hail: vec4<f32>,
    // xyz: how far dust and motes have drifted, w: how much of each the air holds
    dust: vec4<f32>,
    motes: vec4<f32>,
    // rgb: the dust's colour
    dust_color: vec4<f32>,
    // xyz: the wind, in blocks per second
    wind: vec4<f32>,
    // Four sets of ambient particles (pollen, fireflies, …), three entries each: xyz how
    // far they have drifted, w how many; rgb their colour, w 1 if they glow; x their size
    // in blocks, y how they move (0 drift, 1 hover, 2 fall, 3 rise, 4 blow)
    ambient: array<vec4<f32>, 12>,
    // Each column around the camera: the top of the highest block rain lands on, plus one
    // (bits 0..10), the same beneath any leaves (bits 10..20), and whether the top is water
    // (bit 20) or leaves (bit 21).
    heights: array<vec4<u32>, 1024>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: Params;

// As in precipitation.rs.
const BOX = vec3(40.0, 36.0, 40.0);
const MAP: i32 = 64;
const DROPS: u32 = 52000u;
// The last drops splash on the ground instead, within this many blocks of the camera.
const SPLASHES: u32 = 3000u;
// Before them, specks of dust, then motes, then four sets of ambient particles ride the
// air; before those, drips from leaves and the distant layer of rain; the rest fall.
const DUST: u32 = 9200u;
const MOTES: u32 = 800u;
const AMBIENT: u32 = 1500u;
const DRIPS: u32 = 1000u;
const FAR: u32 = 3000u;
const AIRBORNE: u32 = DROPS - SPLASHES - DUST - MOTES - 4u * AMBIENT;
const FALLING: u32 = AIRBORNE - DRIPS - FAR;
const WATER: u32 = 1048576u;
const LEAVES: u32 = 2097152u;
const SPLASH_REACH: f32 = 14.0;
const TAU: f32 = 6.28318531;
// Off-screen, for drops not drawn: all four corners collapse there.
const HIDDEN = vec4(2.0, 2.0, 2.0, 1.0);
// Falling drops fill a box from this far below the camera (as a share of its size) to the
// rest above it: most of what falls below the eye has already landed.
const LOW = vec3(0.5, 0.3, 0.5);

// Fades a falling drop out towards the top and bottom of its box.
fn edge(y: f32, height: f32) -> f32 {
    let t = y / height + LOW.y;
    return smoothstep(0.0, 0.1, t) * (1.0 - smoothstep(0.9, 1.0, t));
}

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    // Position on the drop, -1..1 across and along it.
    @location(0) uv: vec2<f32>,
    // Height above the highest block below, in blocks; drops end where they land.
    @location(1) above: f32,
    @location(2) @interpolate(flat) color: vec4<f32>,
    // 0: raindrop, 1: snowflake, 2: splash, 3: hailstone, 4: sleet, 5: drizzle, 6: dust,
    // 7: mote, 8: ripple, 9: drip
    @location(3) @interpolate(flat) kind: u32,
    // How far a splash or ripple has spread, 0..1; for snowflakes, their turn.
    @location(4) @interpolate(flat) spread: f32,
};

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// The height map's entry for the column under a point `offset` from the camera; 0 off the
// map.
fn column_at(offset: vec3<f32>) -> u32 {
    let column = vec2<i32>(floor(offset.xz + params.map.xz));
    if any(column < vec2(0)) || any(column >= vec2(MAP)) {
        return 0u;
    }
    let c = u32(column.y * MAP + column.x);
    return params.heights[c / 4u][c % 4u];
}

// Height of the highest block below a point `offset` from the camera, relative to the camera.
fn ground(offset: vec3<f32>) -> f32 {
    let word = column_at(offset);
    if word == 0u {
        return -1.0e9;
    }
    return f32(word & 1023u) - params.map.w;
}

// The colour of rain: the light it falls in, cooled, and half the haze, so it matches
// the sky it falls from.
fn rain_color() -> vec3<f32> {
    return mix(params.light.rgb * vec3(1.15, 1.2, 1.3), params.haze.rgb * 1.1, 0.5);
}

fn project(world: vec3<f32>) -> vec4<f32> {
    let view_position = (view.view_from_world * vec4(world, 0.0)).xyz;
    return view.clip_from_view * vec4(view_position, 1.0);
}

// A raindrop landing: a small crown that spreads and fades on the highest block, each
// time somewhere new.
fn splash(n: u32, corner: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    var h = pcg(n * 2246822519u + 777u);
    if unit(h) >= params.rain.w * (1.0 - params.snow.w) {
        return out;
    }
    h = pcg(h);
    let period = 0.35 + 0.4 * unit(h);
    h = pcg(h);
    let age = params.camera.w / period + unit(h);
    let life = fract(age);
    // Only the first part of each period shows a splash.
    if life > 0.3 {
        return out;
    }
    h = pcg(n ^ pcg(u32(floor(age))));
    let x = unit(h);
    h = pcg(h);
    let z = unit(h);
    var offset = vec3(x - 0.5, 0.0, z - 0.5) * 2.0 * SPLASH_REACH;
    let column = column_at(offset);
    let below = ground(offset);
    let distance = length(offset);
    // Under leaves, rain comes through as drips instead.
    if below < -BOX.y || distance < 1.0 || (column & LEAVES) != 0u {
        return out;
    }
    let fade = 1.0 - smoothstep(0.6, 1.0, length(offset.xz) / SPLASH_REACH);
    out.uv = corner;
    out.above = 1.0;
    out.spread = life / 0.3;
    if (column & WATER) != 0u {
        // A ring spreading on the water, a little below the top of the block.
        offset.y = below - 0.1;
        let size = 0.08 + 0.3 * out.spread;
        let world = offset + vec3(corner.x, 0.0, corner.y) * size;
        out.clip = project(world);
        out.kind = 8u;
        out.color = vec4(rain_color() * 1.3, 0.4 * fade * (1.0 - out.spread));
        return out;
    }
    offset.y = below + 0.02;
    let size = 0.04 + 0.08 * out.spread;
    let right = view.world_from_view[0].xyz;
    let world = offset + right * corner.x * size + vec3(0.0, (corner.y + 1.0) * size * 0.6, 0.0);
    out.clip = project(world);
    out.kind = 2u;
    out.color = vec4(rain_color() * 1.4, 0.35 * fade * (1.0 - out.spread));
    return out;
}

// A drop falling from leaves after gathering on them: big, slow and bright, now and then
// in each column under the canopy near the camera, from the leaves to the ground beneath.
fn drip(n: u32, corner: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    var h = pcg(n * 1597334677u + 31u);
    if unit(h) >= params.far.w {
        return out;
    }
    h = pcg(h);
    let period = 1.2 + 2.5 * unit(h);
    h = pcg(h);
    let age = params.camera.w / period + unit(h);
    h = pcg(n ^ pcg(u32(floor(age)) * 7919u));
    let x = unit(h);
    h = pcg(h);
    let z = unit(h);
    var offset = vec3(x - 0.5, 0.0, z - 0.5) * 2.0 * SPLASH_REACH;
    let column = column_at(offset);
    if (column & LEAVES) == 0u {
        return out;
    }
    let leaves = f32(column & 1023u) - params.map.w - 1.0;
    let below = f32((column >> 10u) & 1023u) - params.map.w;
    // It hangs a moment, then falls, faster and faster.
    let t = fract(age) * period - 0.3;
    if t < 0.0 {
        return out;
    }
    offset.y = leaves - 0.5 * 9.0 * t * t;
    if offset.y < below {
        return out;
    }
    let distance = length(offset);
    let pixel = distance * 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    let width = max(0.025, pixel * 0.8);
    let stretch = 0.06 + min(9.0 * t * 0.02, 0.25);
    let right = view.world_from_view[0].xyz;
    let world = offset + right * corner.x * width + vec3(0.0, corner.y * stretch, 0.0);
    out.clip = project(world);
    out.uv = corner;
    out.above = world.y - below;
    out.kind = 9u;
    let fade = (1.0 - smoothstep(0.6, 1.0, length(offset.xz) / SPLASH_REACH)) * smoothstep(1.0, 2.5, distance);
    out.color = vec4(rain_color() * 1.6, 0.6 * fade * 0.025 / width);
    return out;
}

// The distant layer of rain or snow: sparse, faint streaks or specks in a box three times
// as wide as the near one, beyond it, so the rain or snow carries on into the haze.
fn far_rain(n: u32, corner: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    var h = pcg(n * 2654435769u + 4242u);
    if unit(h) >= params.rain.w * (1.0 - params.hail.w) {
        return out;
    }
    let snowflake = params.snow.w > 0.5;
    h = pcg(h);
    let x = unit(h);
    h = pcg(h);
    let y = unit(h);
    h = pcg(h);
    let z = unit(h);
    let box = BOX * 3.0;
    let p = vec3(x, y, z) * box + params.far.xyz;
    var offset = p - box * floor(p / box) - box * LOW;
    let distance = length(offset.xz);
    if distance < BOX.x * 0.4 || offset.y < ground(offset) {
        return out;
    }
    let pixel = length(offset) * 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    var world: vec3<f32>;
    var color: vec3<f32>;
    if snowflake {
        let right = view.world_from_view[0].xyz;
        let up = view.world_from_view[1].xyz;
        world = offset + (right * corner.x + up * corner.y) * max(0.04, pixel * 0.9);
        color = params.light.rgb * 1.7;
        out.kind = 4u;
    } else {
        let along = params.streak.xyz;
        let stretch = params.streak.w * 2.0;
        let across = normalize(cross(along, offset / max(length(offset), 1e-3)));
        world = offset + across * corner.x * pixel * 0.5 + along * corner.y * stretch * 0.5;
        color = rain_color();
        out.kind = 0u;
    }
    out.clip = project(world);
    out.uv = corner;
    out.above = 1.0;
    let fade = smoothstep(BOX.x * 0.4, BOX.x * 0.6, distance) * (1.0 - smoothstep(box.x * 0.35, box.x * 0.5, distance));
    let strength = select(0.22, 0.45, snowflake);
    out.color = vec4(color, strength * fade * edge(offset.y, box.y));
    return out;
}

// A speck of dust, a glowing mote or an ambient particle (pollen, a firefly, a spore),
// carried on the air with a little turbulence, in a box half as wide as the falling
// drops'.
fn airborne(n: u32, corner: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    // Which group: 0 dust, 1 motes, 2.. ambient.
    var group = 0u;
    if n >= AIRBORNE + DUST + MOTES {
        group = 2u + (n - AIRBORNE - DUST - MOTES) / AMBIENT;
    } else if n >= AIRBORNE + DUST {
        group = 1u;
    }
    var drifted: vec3<f32>;
    var amount: f32;
    var color: vec3<f32>;
    var glow = false;
    var scale: f32;
    var motion: u32;
    var h = pcg(n * 3266489917u + 99u);
    let size = unit(pcg(h ^ 0x51u));
    if group == 0u {
        drifted = params.dust.xyz;
        amount = params.dust.w;
        color = params.dust_color.rgb;
        // Fine grains, a few coarser.
        scale = 0.02 + 0.05 * size * size;
        motion = 4u;
    } else if group == 1u {
        drifted = params.motes.xyz;
        amount = params.motes.w;
        // Pale gold and sky blue.
        color = mix(vec3(1.0, 0.85, 0.45), vec3(0.55, 0.85, 1.0), fract(size * 7.0));
        glow = true;
        scale = 0.05 + 0.07 * size;
        motion = 0u;
    } else {
        let i = (group - 2u) * 3u;
        drifted = params.ambient[i].xyz;
        amount = params.ambient[i].w;
        color = params.ambient[i + 1u].rgb;
        glow = params.ambient[i + 1u].w > 0.5;
        scale = params.ambient[i + 2u].x * (0.6 + 0.8 * size);
        motion = u32(params.ambient[i + 2u].y + 0.5);
    }
    if unit(h) >= amount {
        return out;
    }
    h = pcg(h);
    let x = unit(h);
    h = pcg(h);
    let y = unit(h);
    h = pcg(h);
    let z = unit(h);
    h = pcg(h);
    let phase = unit(h) * TAU;
    let box = BOX * 0.5;
    let p = vec3(x, y, z) * box + drifted - params.camera.xyz;
    var offset = p - box * floor(p / box) - box * 0.5;
    let t = params.camera.w;
    // Blown things tumble in the gusts, drifting ones wander, hovering ones bob.
    var swirl = 0.9;
    var rate = 0.25 + 0.2 * size;
    switch motion {
        case 1u: { swirl = 0.35; rate = 0.4 + 0.3 * size; }
        case 2u: { swirl = 0.3; rate = 0.8 + 0.4 * size; }
        case 3u: { swirl = 0.4; rate = 0.3 + 0.2 * size; }
        case 4u: { swirl = 0.6; rate = 1.3 + size; }
        default: {}
    }
    offset += vec3(sin(t * rate + phase), 0.5 * sin(t * rate * 1.3 + phase * 2.1), cos(t * rate * 0.8 + phase * 1.7)) * swirl;
    let below = ground(offset);
    if offset.y < below + 0.1 {
        return out;
    }
    let distance = length(offset);
    let pixel = distance * 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    let radius = scale * 0.5;
    let drawn = max(radius, pixel * 0.8);
    var world: vec3<f32>;
    let wind = params.wind.xyz;
    if motion != 4u || length(wind) < 1.0 {
        let right = view.world_from_view[0].xyz;
        let up = view.world_from_view[1].xyz;
        world = offset + (right * corner.x + up * corner.y) * drawn;
    } else {
        // Blown dust streaks along the wind, longer the harder it blows.
        let along = normalize(wind);
        let across = normalize(cross(along, offset / max(distance, 1e-3)));
        let streak = drawn + length(wind) * 0.012;
        world = offset + across * corner.x * drawn + along * corner.y * streak;
    }
    var opacity = radius / drawn;
    if glow {
        // Brighter than daylight so bloom haloes it. Motes twinkle; hovering lights
        // (fireflies) blink, lit for a moment every few seconds.
        var light = 0.6 + 0.4 * sin(t * (1.5 + 2.0 * size) + phase);
        if motion == 1u {
            light = smoothstep(0.55, 0.9, sin(t * (0.7 + 0.5 * size) + phase));
        }
        out.color = vec4(color * 4.0 * light, 0.0);
        out.kind = 7u;
    } else {
        // Specks catch the light; big soft puffs (mist) are faint.
        out.color = vec4(color * params.light.rgb * 2.6, 0.0);
        opacity *= mix(0.75, 0.2, smoothstep(0.15, 0.5, scale));
        out.kind = 6u;
    }
    let reach = length(offset.xz) / (box.x * 0.5);
    opacity *= (1.0 - smoothstep(0.6, 1.0, reach)) * (1.0 - smoothstep(0.8, 1.0, abs(offset.y) / (box.y * 0.5)));
    opacity *= smoothstep(0.8, 3.0, distance);
    out.color.a = opacity;
    out.clip = project(world);
    out.uv = corner;
    out.above = world.y - below;
    return out;
}

@vertex
fn vertex(@location(0) id: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    let drop = id >> 2u;
    let corner = vec2(f32(id & 1u), f32((id >> 1u) & 1u)) * 2.0 - 1.0;
    if drop >= DROPS - SPLASHES {
        return splash(drop, corner);
    }
    if drop >= AIRBORNE {
        return airborne(drop, corner);
    }
    if drop >= FALLING + FAR {
        return drip(drop, corner);
    }
    if drop >= FALLING {
        return far_rain(drop, corner);
    }
    var h = pcg(drop * 2654435769u + 12345u);
    // Lighter rain shows fewer of the drops.
    if unit(h) >= params.rain.w {
        return out;
    }
    h = pcg(h);
    // Snow, or of the rest hail, sleet, drizzle or rain, in the fractions asked for.
    let choice = unit(h);
    let snowflake = choice < params.snow.w;
    let rest = (choice - params.snow.w) / max(1.0 - params.snow.w, 1e-4);
    var kind = 0u;
    if snowflake {
        kind = 1u;
    } else if rest < params.hail.w {
        kind = 3u;
    } else if rest < params.hail.w + params.sleet.w {
        kind = 4u;
    } else if (rest - params.hail.w - params.sleet.w) / max(1.0 - params.hail.w - params.sleet.w, 1e-4) < params.drizzle.w {
        kind = 5u;
    }
    let round = kind == 1u || kind == 4u;
    h = pcg(h);
    let x = unit(h);
    h = pcg(h);
    let y = unit(h);
    h = pcg(h);
    let z = unit(h);
    h = pcg(h);
    let size = unit(h);
    h = pcg(h);
    let phase = unit(h) * TAU;

    // Where the drop is relative to the camera, wrapped into the box around it. Fine
    // drizzle crowds into a box half as wide, and slow snow into one 0.6 as wide, so they
    // are denser near the camera; their fallen offsets wrap at their own box sizes.
    var fallen = params.rain.xyz;
    switch kind {
        case 1u: { fallen = params.snow.xyz; }
        case 3u: { fallen = params.hail.xyz; }
        case 4u: { fallen = params.sleet.xyz; }
        case 5u: { fallen = params.drizzle.xyz; }
        default: {}
    }
    var box = BOX;
    if kind == 5u {
        box = BOX * 0.5;
    } else if kind == 1u {
        box = BOX * 0.6;
    }
    let p = vec3(x, y, z) * box + fallen - select(params.camera.xyz, vec3(0.0), kind == 1u);
    var offset = p - box * floor(p / box) - box * LOW;
    let t = params.camera.w;
    if snowflake {
        // Flakes flutter and swirl as they fall, the big ones more.
        let swirl = 0.2 + 0.4 * size;
        offset += vec3(
            sin(t * (0.4 + 0.8 * size) + phase) + 0.4 * sin(t * 1.7 + phase * 3.1),
            0.3 * sin(t * (0.9 + size) + phase * 2.3),
            cos(t * (0.35 + 0.7 * size) + phase * 1.7) + 0.4 * cos(t * 1.3 + phase * 2.7),
        ) * swirl;
    }
    let distance = length(offset);
    let below = ground(offset);

    // Thin drops are drawn about a pixel wide and fainter instead, so they do not shimmer.
    let pixel = distance * 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    var world: vec3<f32>;
    var opacity: f32;
    // How far the drop reaches above its centre; below the ground it is not drawn at all.
    var top: f32;
    if round {
        // Snowflakes: mostly fine, a few big; they glint as they turn. Sleet pellets:
        // small, icy and hard.
        var radius = 0.032 + 0.05 * size * size;
        var brightness = 1.5 + 0.6 * fract(phase * 3.7);
        var tint = vec3(1.0);
        var strength = 0.85 * (0.75 + 0.25 * sin(t * (1.5 + 2.0 * size) + phase * 5.0));
        out.spread = phase + t * (0.4 + size);
        if kind == 4u {
            radius = 0.015 + 0.015 * size;
            brightness = 1.6;
            tint = vec3(0.85, 0.92, 1.0);
            strength = 0.6;
        }
        let drawn = max(radius, pixel * 0.9);
        let right = view.world_from_view[0].xyz;
        let up = view.world_from_view[1].xyz;
        world = offset + (right * corner.x + up * corner.y) * drawn;
        top = drawn;
        opacity = strength * radius / drawn;
        out.color = vec4(params.light.rgb * tint * brightness, 0.0);
        out.kind = kind;
    } else {
        // Raindrops; drizzle, finer, slower and blown further by the wind; and hailstones,
        // white pellets falling hard, blurred into short streaks.
        let drizzle = kind == 5u;
        let hail = kind == 3u;
        var fall = params.streak.xyz;
        var stretch = params.streak.w;
        var width = 0.016 + 0.02 * size;
        if drizzle {
            let velocity = params.wind.xyz * 0.9 - vec3(0.0, 4.0, 0.0);
            fall = normalize(velocity);
            stretch = length(velocity) * 0.045;
            width = 0.006 + 0.004 * size;
        } else if hail {
            let velocity = params.wind.xyz * 0.3 - vec3(0.0, 18.0, 0.0);
            fall = normalize(velocity);
            stretch = length(velocity) * 0.015;
            width = 0.025 + 0.02 * size;
        }
        let along = fall;
        stretch *= 0.7 + 0.6 * size;
        let drawn = max(width, pixel * 0.8);
        let across = normalize(cross(along, offset / max(distance, 1e-3)));
        world = offset + across * corner.x * drawn * 0.5 + along * corner.y * stretch * 0.5;
        top = -along.y * stretch * 0.5;
        opacity = select(select(0.3, 0.3, drizzle), 0.85, hail) * width / drawn;
        out.color = vec4(select(rain_color(), params.light.rgb * vec3(2.0, 2.1, 2.3), hail), 0.0);
        out.kind = kind;
    }
    if offset.y + top < below {
        return out;
    }
    // A cylinder rather than a box, soft at its edges, with nothing right at the eye.
    let reach = length(offset.xz) / (box.x * 0.5);
    opacity *= (1.0 - smoothstep(0.6, 1.0, reach)) * edge(offset.y, box.y);
    // Drops right at the eye would smear across the screen.
    opacity *= smoothstep(1.5, 4.0, distance);
    out.color.a = opacity;
    out.clip = project(world);
    out.uv = corner;
    out.above = world.y - below;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.above < 0.0 {
        discard;
    }
    var shape: f32;
    if in.kind == 2u {
        // A crown of droplets: a thin ring, open at the top, spreading outwards.
        let r = length(in.uv);
        let rim = in.spread * 0.6 + 0.3;
        shape = (1.0 - smoothstep(0.0, 0.25, abs(r - rim))) * smoothstep(0.9, 0.2, in.uv.y);
    } else if in.kind == 1u {
        // A small flake: a bright core with six faint arms, turning.
        let r = length(in.uv);
        let arms = pow(abs(cos(3.0 * (atan2(in.uv.y, in.uv.x) + in.spread))), 8.0);
        shape = max(1.0 - smoothstep(0.0, 0.5, r), 0.7 * arms * (1.0 - smoothstep(0.3, 1.0, r)));
    } else if in.kind == 4u {
        shape = 1.0 - smoothstep(0.2, 1.0, length(in.uv));
    } else if in.kind == 8u {
        // A ripple: a thin ring spreading on the water.
        let r = length(in.uv);
        shape = 1.0 - smoothstep(0.0, 0.12, abs(r - 0.85));
    } else if in.kind == 9u {
        // A drip: a rounded drop, fuller at the bottom.
        shape = (1.0 - in.uv.x * in.uv.x) * smoothstep(1.0, 0.3, in.uv.y) * smoothstep(-1.0, -0.6, in.uv.y);
    } else if in.kind == 6u {
        // A soft puff.
        let r = 1.0 - smoothstep(0.0, 1.0, length(in.uv));
        shape = r * r;
    } else if in.kind == 7u {
        // A bright core in a soft glow.
        let r = length(in.uv);
        shape = (1.0 - smoothstep(0.0, 1.0, r)) * 0.5 + (1.0 - smoothstep(0.0, 0.35, r)) * 0.5;
    } else {
        // Soft across, tapering towards both ends.
        shape = (1.0 - in.uv.x * in.uv.x) * (1.0 - in.uv.y * in.uv.y * in.uv.y * in.uv.y);
    }
    return vec4(in.color.rgb, in.color.a * shape);
}
