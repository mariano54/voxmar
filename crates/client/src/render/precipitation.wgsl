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
    // xyz: the same for snow, w: the fraction falling as snow
    snow: vec4<f32>,
    // xyz: direction rain falls in, w: streak length in blocks
    streak: vec4<f32>,
    // rgb: light falling on the drops
    light: vec4<f32>,
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
    // Top of the highest block plus one in each column around the camera, 16 bits each.
    heights: array<vec4<u32>, 512>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: Params;

// As in precipitation.rs.
const BOX = vec3(40.0, 36.0, 40.0);
const MAP: i32 = 64;
const DROPS: u32 = 22000u;
// The last drops splash on the ground instead, within this many blocks of the camera.
const SPLASHES: u32 = 3000u;
// Before them, specks of dust, then motes, ride the wind; the rest fall.
const DUST: u32 = 5200u;
const MOTES: u32 = 800u;
const FALLING: u32 = DROPS - SPLASHES - DUST - MOTES;
const SPLASH_REACH: f32 = 14.0;
const TAU: f32 = 6.28318531;
// Off-screen, for drops not drawn: all four corners collapse there.
const HIDDEN = vec4(2.0, 2.0, 2.0, 1.0);

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    // Position on the drop, -1..1 across and along it.
    @location(0) uv: vec2<f32>,
    // Height above the highest block below, in blocks; drops end where they land.
    @location(1) above: f32,
    @location(2) @interpolate(flat) color: vec4<f32>,
    // 0: raindrop, 1: snowflake, 2: splash, 3: hailstone, 4: sleet, 5: drizzle, 6: dust,
    // 7: mote
    @location(3) @interpolate(flat) kind: u32,
    // How far a splash has spread, 0..1.
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

// Height of the highest block below a point `offset` from the camera, relative to the camera.
fn ground(offset: vec3<f32>) -> f32 {
    let column = vec2<i32>(floor(offset.xz + params.map.xz));
    if any(column < vec2(0)) || any(column >= vec2(MAP)) {
        return -1.0e9;
    }
    let c = u32(column.y * MAP + column.x);
    let word = params.heights[c / 8u][(c / 2u) % 4u];
    return f32((word >> (16u * (c % 2u))) & 0xffffu) - params.map.w;
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
    let below = ground(offset);
    offset.y = below + 0.02;
    let distance = length(offset);
    if below < -BOX.y || distance < 1.0 {
        return out;
    }
    let size = 0.04 + 0.08 * life / 0.3;
    let right = view.world_from_view[0].xyz;
    let world = offset + right * corner.x * size + vec3(0.0, (corner.y + 1.0) * size * 0.6, 0.0);
    out.clip = project(world);
    out.uv = corner;
    out.above = 1.0;
    out.kind = 2u;
    out.spread = life / 0.3;
    let fade = 1.0 - smoothstep(0.6, 1.0, length(offset.xz) / SPLASH_REACH);
    out.color = vec4(params.light.rgb * 1.4, 0.35 * fade * (1.0 - out.spread));
    return out;
}

// A speck of dust or a glowing mote, carried on the wind with a little turbulence, in a
// box half as wide as the falling drops'.
fn airborne(n: u32, corner: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    let mote = n >= FALLING + DUST;
    let amount = select(params.dust.w, params.motes.w, mote);
    var h = pcg(n * 3266489917u + 99u);
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
    let size = unit(h);
    h = pcg(h);
    let phase = unit(h) * TAU;
    let box = BOX * 0.5;
    let drifted = select(params.dust.xyz, params.motes.xyz, mote);
    let p = vec3(x, y, z) * box + drifted - params.camera.xyz;
    var offset = p - box * floor(p / box) - box * 0.5;
    let t = params.camera.w;
    // Dust tumbles in the gusts; motes wander slowly.
    let swirl = select(0.6, 0.9, mote);
    let rate = select(1.3 + size, 0.25 + 0.2 * size, mote);
    offset += vec3(sin(t * rate + phase), 0.5 * sin(t * rate * 1.3 + phase * 2.1), cos(t * rate * 0.8 + phase * 1.7)) * swirl;
    let below = ground(offset);
    if offset.y < below + 0.1 {
        return out;
    }
    let distance = length(offset);
    let pixel = distance * 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    let radius = select(0.03 + 0.05 * size, 0.025 + 0.035 * size, mote);
    let drawn = max(radius, pixel * 0.8);
    var world: vec3<f32>;
    let wind = params.wind.xyz;
    if mote || length(wind) < 1.0 {
        let right = view.world_from_view[0].xyz;
        let up = view.world_from_view[1].xyz;
        world = offset + (right * corner.x + up * corner.y) * drawn;
    } else {
        // Dust streaks along the wind, longer the harder it blows.
        let along = normalize(wind);
        let across = normalize(cross(along, offset / max(distance, 1e-3)));
        let streak = drawn + length(wind) * 0.035;
        world = offset + across * corner.x * drawn + along * corner.y * streak;
    }
    var opacity = radius / drawn;
    if mote {
        // Pale gold and sky blue, glowing brighter than daylight so bloom haloes them, and
        // twinkling.
        let hue = mix(vec3(1.0, 0.85, 0.45), vec3(0.55, 0.85, 1.0), fract(size * 7.0));
        let twinkle = 0.6 + 0.4 * sin(t * (1.5 + 2.0 * size) + phase);
        out.color = vec4(hue * 4.0 * twinkle, 0.0);
        out.kind = 7u;
    } else {
        out.color = vec4(params.dust_color.rgb * params.light.rgb * 1.6, 0.0);
        opacity *= 0.7;
        out.kind = 6u;
    }
    let reach = length(offset.xz) / (box.x * 0.5);
    opacity *= (1.0 - smoothstep(0.6, 1.0, reach)) * (1.0 - smoothstep(0.8, 1.0, abs(offset.y) / (box.y * 0.5)));
    opacity *= smoothstep(0.6, 2.0, distance);
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
    if drop >= FALLING {
        return airborne(drop, corner);
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

    // Where the drop is relative to the camera, wrapped into the box around it. Slow
    // snowflakes and fine drizzle crowd into a box half as wide, which divides the full
    // one, so they stay put in the world too.
    var fallen = params.rain.xyz;
    switch kind {
        case 1u: { fallen = params.snow.xyz; }
        case 3u: { fallen = params.hail.xyz; }
        case 4u: { fallen = params.sleet.xyz; }
        case 5u: { fallen = params.drizzle.xyz; }
        default: {}
    }
    let box = select(BOX, BOX * 0.5, kind == 1u || kind == 5u);
    let p = vec3(x, y, z) * box + fallen - params.camera.xyz;
    var offset = p - box * floor(p / box) - box * 0.5;
    let t = params.camera.w;
    if snowflake {
        offset += vec3(sin(t * (0.6 + size) + phase), 0.0, cos(t * (0.5 + 0.8 * size) + phase * 1.7)) * 0.4;
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
        // Snowflakes, and sleet pellets: small, icy and hard.
        var radius = 0.03 + 0.04 * size;
        var brightness = 1.8;
        var tint = vec3(1.0);
        var strength = 0.9;
        if kind == 4u {
            radius = 0.015 + 0.015 * size;
            brightness = 1.6;
            tint = vec3(0.85, 0.92, 1.0);
            strength = 0.6;
        }
        let drawn = max(radius, pixel * 0.7);
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
        var width = 0.012 + 0.008 * size;
        if drizzle {
            let velocity = params.wind.xyz * 0.9 - vec3(0.0, 4.0, 0.0);
            fall = normalize(velocity);
            stretch = length(velocity) * 0.045;
            width = 0.006 + 0.004 * size;
        } else if hail {
            let velocity = params.wind.xyz * 0.3 - vec3(0.0, 18.0, 0.0);
            fall = normalize(velocity);
            stretch = length(velocity) * 0.02;
            width = 0.04 + 0.03 * size;
        }
        let along = fall;
        stretch *= 0.7 + 0.6 * size;
        let drawn = max(width, pixel * 0.8);
        let across = normalize(cross(along, offset / max(distance, 1e-3)));
        world = offset + across * corner.x * drawn * 0.5 + along * corner.y * stretch * 0.5;
        top = -along.y * stretch * 0.5;
        opacity = select(select(0.45, 0.35, drizzle), 0.95, hail) * width / drawn;
        out.color = vec4(params.light.rgb * select(vec3(1.15, 1.2, 1.3), vec3(2.0, 2.1, 2.3), hail), 0.0);
        out.kind = kind;
    }
    if offset.y + top < below {
        return out;
    }
    // A cylinder rather than a box, soft at its edges, with nothing right at the eye.
    let reach = length(offset.xz) / (box.x * 0.5);
    opacity *= (1.0 - smoothstep(0.6, 1.0, reach)) * (1.0 - smoothstep(0.8, 1.0, abs(offset.y) / (box.y * 0.5)));
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
    } else if in.kind == 1u || in.kind == 4u {
        shape = 1.0 - smoothstep(0.2, 1.0, length(in.uv));
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
