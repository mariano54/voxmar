// The wind foliage bends in (see wind.rs): the weather's wind, gusts carried along with
// it, and the player parting the grass. Bound with the terrain (group 2) in every terrain
// pass, so shadows bend with what casts them.

#define_import_path voxmar::wind

struct Wind {
    // xy: mean wind along x and z, in blocks per second. zw: how far gusts have travelled
    // with it, wrapped every 1024 blocks, which the gust noise repeats on.
    flow: vec4<f32>,
    // xyz: the player's feet; w: 1 in a game, 0 in the menus.
    player: vec4<f32>,
    // How much the wind gusts, 0..1.
    gustiness: f32,
}

@group(2) @binding(6) var<uniform> wind: Wind;

fn lattice_hash(cell: vec2<i32>, cells: i32) -> f32 {
    let c = vec2<u32>(((cell % cells) + cells) % cells);
    var h = (c.x * 0x8da6b343u) ^ (c.y * 0xd8163841u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h ^= h >> 12u;
    return f32(h >> 8u) / 16777216.0;
}

// Smooth noise 0..1 that repeats every `cells` lattice cells.
fn periodic_noise(p: vec2<f32>, cells: i32) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let a = lattice_hash(i, cells);
    let b = lattice_hash(i + vec2(1, 0), cells);
    let c = lattice_hash(i + vec2(0, 1), cells);
    let d = lattice_hash(i + vec2(1, 1), cells);
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

// Gust strength at `xz`, 0..1: fronts tens of blocks across rolling over the land with
// the wind, and quicker ripples running through them. Both scales repeat on the offset's
// 1024-block wrap (the ripples, moving twice as fast, on two of it).
fn gust(xz: vec2<f32>) -> f32 {
    let fronts = periodic_noise((xz - wind.flow.zw) / 32.0, 32);
    let ripples = periodic_noise((xz - 2.0 * wind.flow.zw) / 8.0, 128);
    return 0.7 * fronts + 0.3 * ripples;
}

// How far foliage at `xz` leans, in blocks per block of height: the mean wind's push,
// from lulls a little below it to gusts well above (as `voxmar_engine::weather::gust`),
// and a flutter across it that quickens as the wind rises.
fn sway(xz: vec2<f32>, time: f32) -> vec2<f32> {
    let speed = length(wind.flow.xy);
    let along = select(vec2(1.0, 0.0), wind.flow.xy / speed, speed > 0.01);
    let strength = saturate(speed / 16.0);
    let g = gust(xz);
    let push = strength * 0.6 * (1.0 + wind.gustiness * (1.8 * g * g - 0.45));
    let flutter = sin(time * (2.0 + 5.0 * strength) + dot(xz, vec2(1.7, 2.3)) + g * 6.0) * (0.04 + 0.1 * strength);
    return along * (push + 0.5 * flutter) + vec2(-along.y, along.x) * flutter;
}

// How far the player pushes foliage at `p` aside, in blocks per block of height.
fn parting(p: vec3<f32>) -> vec2<f32> {
    let away = p.xz - wind.player.xz;
    let distance = length(away);
    let near = (1.0 - smoothstep(0.2, 1.2, distance)) * (1.0 - smoothstep(0.5, 1.8, abs(p.y - wind.player.y)));
    return away / max(distance, 0.001) * near * wind.player.w * 1.3;
}

// A leaf vertex's flutter, livelier in a strong wind.
fn leaf_flutter(world: vec3<f32>, time: f32) -> vec3<f32> {
    let speed = length(wind.flow.xy);
    let strength = saturate(speed / 16.0);
    let phase = world.x * 0.7 + world.z * 0.5 + world.y * 0.3;
    let quick = 1.0 + strength;
    let flutter = vec3(sin(time * 1.6 * quick + phase), sin(time * 2.3 * quick + phase * 1.3) * 0.5, cos(time * 1.9 * quick + phase));
    let lean = select(vec2(0.0), wind.flow.xy / speed, speed > 0.01) * strength * gust(world.xz) * 0.06;
    return flutter * (0.02 + 0.05 * strength) + vec3(lean.x, 0.0, lean.y);
}
