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
    // The flutter's phase, radians: it runs quicker in a stronger wind, integrated frame by
    // frame (see wind.rs), and wraps after 320 turns.
    flutter: f32,
}

@group(2) @binding(4) var<uniform> wind: Wind;

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
// the wind, and quicker ripples running through them, the waves that sweep a meadow. Both
// scales repeat on the offset's 1024-block wrap (the ripples, moving twice as fast, on two
// of it).
fn gust(xz: vec2<f32>) -> f32 {
    let fronts = periodic_noise((xz - wind.flow.zw) / 32.0, 32);
    let ripples = periodic_noise((xz - 2.0 * wind.flow.zw) / 8.0, 128);
    return smoothstep(0.2, 0.8, 0.5 * fronts + 0.5 * ripples);
}

// The wind's strength for foliage: 0 still air to 1 a gale (18 blocks a second).
fn wind_strength() -> f32 {
    return saturate(length(wind.flow.xy) / 18.0);
}

// Which way the wind blows, a unit vector along x and z.
fn wind_direction() -> vec2<f32> {
    let speed = length(wind.flow.xy);
    return select(vec2(1.0, 0.0), wind.flow.xy / speed, speed > 0.01);
}

struct Sway {
    // How far the plant leans, in blocks per block of height.
    lean: vec2<f32>,
    // How hard the gust there blows, 0..1.
    gust: f32,
}

// How foliage at `xz` leans, as the tangent of its angle from upright. The lean comes from
// the gusts rolling over the land, so neighbours bend together and waves of it sweep
// across a meadow with the wind (as in the Grassier Grass mod): barely a stir in still
// air, rolling waves in a breeze, streaming over in a gale. On top, a small
// flutter mostly across the wind, at `pace` times the flutter phase (whole 64ths of it, to
// stay in step as the phase wraps), offset by `phase` radians so neighbours are not in
// lockstep. Its phase follows the gusts too, so it ripples with them rather than shivers
// (and never with the position along the wind: thousands of blocks from the origin, the
// slightest turn of the wind would leap it).
fn sway(xz: vec2<f32>, pace: f32, phase: f32) -> Sway {
    let strength = wind_strength();
    let along = wind_direction();
    let g = gust(xz);
    // Gusty weather swings between lulls and gusts; steady wind barely does.
    let depth = 0.5 + 0.4 * wind.gustiness;
    let push = 1.6 * pow(strength, 0.8) * (1.0 - depth + depth * g);
    let flutter = sin(wind.flutter * pace - g * 4.0 + phase) * (0.02 + 0.04 * strength);
    return Sway(along * (push + 0.3 * flutter) + vec2(-along.y, along.x) * flutter, g);
}

// How far the player pushes foliage at `p` aside, in blocks per block of height.
fn parting(p: vec3<f32>) -> vec2<f32> {
    let away = p.xz - wind.player.xz;
    let distance = length(away);
    let near = (1.0 - smoothstep(0.2, 1.2, distance)) * (1.0 - smoothstep(0.5, 1.8, abs(p.y - wind.player.y)));
    return away / max(distance, 0.001) * near * wind.player.w * 1.3;
}

// A leaf vertex's flutter, livelier in a strong wind, and its lean with the gusts.
fn leaf_flutter(world: vec3<f32>) -> vec3<f32> {
    let strength = wind_strength();
    let phase = world.x * 0.7 + world.z * 0.5 + world.y * 0.3;
    let t = wind.flutter;
    let flutter = vec3(sin(t * 1.6 + phase), sin(t * 2.3 + phase * 1.3) * 0.5, cos(t * 1.9 + phase));
    let lean = wind_direction() * strength * gust(world.xz) * 0.08;
    return flutter * (0.015 + 0.04 * strength) + vec3(lean.x, 0.0, lean.y);
}
