// Rain streaks and snowflakes in a box that follows the camera (see precipitation.rs).
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
    // Top of the highest block plus one in each column around the camera, 16 bits each.
    heights: array<vec4<u32>, 512>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: Params;

// As in precipitation.rs.
const BOX = vec3(40.0, 36.0, 40.0);
const MAP: i32 = 64;
const DROPS: u32 = 16000u;
// The last drops splash on the ground instead, within this many blocks of the camera.
const SPLASHES: u32 = 3000u;
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
    // 0: raindrop, 1: snowflake, 2: splash
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

@vertex
fn vertex(@location(0) id: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip = HIDDEN;
    let drop = id >> 2u;
    let corner = vec2(f32(id & 1u), f32((id >> 1u) & 1u)) * 2.0 - 1.0;
    if drop >= DROPS - SPLASHES {
        return splash(drop, corner);
    }
    var h = pcg(drop * 2654435769u + 12345u);
    // Lighter rain shows fewer of the drops.
    if unit(h) >= params.rain.w {
        return out;
    }
    h = pcg(h);
    let snowflake = unit(h) < params.snow.w;
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
    // snowflakes crowd into a box half as wide, which divides the full one, so they stay
    // put in the world too.
    let fallen = select(params.rain.xyz, params.snow.xyz, snowflake);
    let box = select(BOX, BOX * 0.5, snowflake);
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
    if snowflake {
        let radius = 0.03 + 0.04 * size;
        let drawn = max(radius, pixel * 0.7);
        let right = view.world_from_view[0].xyz;
        let up = view.world_from_view[1].xyz;
        world = offset + (right * corner.x + up * corner.y) * drawn;
        top = drawn;
        opacity = 0.9 * radius / drawn;
        out.color = vec4(params.light.rgb * 1.8, 0.0);
        out.kind = 1u;
    } else {
        let along = params.streak.xyz;
        let length = params.streak.w * (0.7 + 0.6 * size);
        let width = 0.012 + 0.008 * size;
        let drawn = max(width, pixel * 0.8);
        let across = normalize(cross(along, offset / max(distance, 1e-3)));
        world = offset + across * corner.x * drawn * 0.5 + along * corner.y * length * 0.5;
        top = -along.y * length * 0.5;
        opacity = 0.45 * width / drawn;
        out.color = vec4(params.light.rgb * vec3(1.15, 1.2, 1.3), 0.0);
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
    } else if in.kind == 1u {
        shape = 1.0 - smoothstep(0.2, 1.0, length(in.uv));
    } else {
        // Soft across, tapering towards both ends.
        shape = (1.0 - in.uv.x * in.uv.x) * (1.0 - in.uv.y * in.uv.y * in.uv.y * in.uv.y);
    }
    return vec4(in.color.rgb, in.color.a * shape);
}
