// Sky dome: horizon-to-zenith gradient, clouds shaped by the weather, sun and moon, and a
// night sky of stars, constellations, the Milky Way, faint galaxies and planets that turns
// with the sun.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view
#import voxmar::atmosphere::{Atmosphere, edge_fog}

struct SkyParams {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: direction towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // x: seconds of game time, y: low cloud cover 0..1, z: how heaped it is, from flat
    // layers (0) to towers (1), w: cirrus 0..1
    clouds: vec4<f32>,
    // x: fraction of the day, y: days elapsed (wrapped every 1000 days)
    stars: vec4<f32>,
    // xy: where the cloud layer is over the camera (its position and wind drift, in cloud
    // units), z: precipitation 0..1, w: lightning flash 0..1
    weather: vec4<f32>,
    // xy: wind direction
    wind: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyParams;
// The terrain's air (atmosphere.wgsl): fog and ground fog hide the sky as they hide the land.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> atmosphere: Atmosphere;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;

// The night sky turns about the same axis as the sun (z). In its own frame the sun sits at
// longitude 0; everything below is placed by (longitude, latitude) in degrees.
const SUN = vec3(0.97014, 0.0, 0.24254);
// The Milky Way runs along the great circle facing MILKY_POLE, brightest towards MILKY_CORE.
const MILKY_POLE = vec3(0.55627, 0.35719, -0.75032);
const MILKY_CORE = vec3(-0.80296, 0.46359, -0.37461);
// Turns the coarse star layer so its cube seams do not line up with the fine one.
const LAYER_TURN = mat3x3<f32>(
    vec3(0.6486, 0.6821, -0.3376), vec3(-0.5740, 0.7297, 0.3715), vec3(0.4998, -0.0472, 0.8649)
);

// Constellations: centre longitude and latitude, radius enclosing the stars, star count.
const CONSTELLATIONS = array<vec4<f32>, 8>(
    vec4(255.0, -5.0, 14.0, 10.0), // the Hunter
    vec4(282.0, 16.0, 2.0, 7.0), // the Sisters, a tight cluster
    vec4(195.0, 22.0, 8.0, 6.0), // the Lyre
    vec4(215.0, 52.0, 16.0, 8.0), // the Dipper
    vec4(222.0, -50.0, 11.0, 7.0), // the Cross and its pointers
    vec4(155.0, -24.0, 14.0, 13.0), // the Scorpion
    vec4(105.0, 16.0, 13.0, 8.0), // the Swan
    vec4(320.0, 45.0, 11.0, 5.0), // the Throne, a W
);

// Constellation stars, in order: offset from the centre in degrees (x east, y north),
// brightness, and colour from blue-white (0) through white and yellow to orange (1).
const STARS = array<vec4<f32>, 64>(
    // the Hunter
    vec4(-4.5, 9.0, 3.1, 0.95), vec4(4.0, 7.5, 1.7, 0.15), vec4(0.0, 12.0, 0.8, 0.2),
    vec4(-2.2, -0.6, 1.5, 0.1), vec4(0.0, 0.0, 1.7, 0.1), vec4(2.0, 0.7, 1.4, 0.1),
    vec4(0.2, -3.0, 0.6, 0.2), vec4(0.4, -4.2, 0.8, 0.15),
    vec4(-4.0, -8.5, 1.4, 0.15), vec4(5.0, -8.0, 3.1, 0.05),
    // the Sisters
    vec4(0.0, 0.0, 1.3, 0.1), vec4(0.6, 0.25, 1.0, 0.1), vec4(0.95, -0.15, 0.9, 0.1),
    vec4(0.45, -0.5, 0.8, 0.1), vec4(-0.45, -0.2, 0.8, 0.1), vec4(-0.15, 0.45, 0.8, 0.1),
    vec4(1.3, 0.15, 0.7, 0.1),
    // the Lyre
    vec4(0.0, 0.0, 3.6, 0.0), vec4(1.6, 1.4, 0.6, 0.2), vec4(1.4, -1.6, 0.7, 0.2),
    vec4(2.6, -4.6, 0.8, 0.7), vec4(0.6, -6.2, 1.0, 0.2), vec4(-0.6, -4.8, 0.9, 0.1),
    // the Dipper
    vec4(8.0, 3.0, 2.0, 0.8), vec4(8.0, -2.5, 1.5, 0.2), vec4(1.5, -3.5, 1.4, 0.25),
    vec4(0.5, 1.5, 1.0, 0.3), vec4(-4.5, 2.5, 1.8, 0.2), vec4(-9.0, 3.5, 1.5, 0.25),
    vec4(-8.6, 4.1, 0.4, 0.3), vec4(-14.0, 1.0, 1.7, 0.1),
    // the Cross and its pointers
    vec4(0.0, 3.2, 1.8, 0.85), vec4(0.0, -3.0, 2.2, 0.05), vec4(-2.6, 0.2, 2.0, 0.05),
    vec4(2.2, 0.9, 1.3, 0.15), vec4(1.0, -0.8, 0.6, 0.3), vec4(-9.0, -2.0, 2.8, 0.4),
    vec4(-6.0, -1.2, 2.1, 0.1),
    // the Scorpion
    vec4(-9.5, 7.0, 1.3, 0.1), vec4(-10.5, 4.0, 1.5, 0.1), vec4(-10.0, 1.0, 1.1, 0.15),
    vec4(-6.0, 3.0, 1.0, 0.2), vec4(-4.0, 1.0, 2.8, 1.0), vec4(-2.5, -0.5, 1.0, 0.2),
    vec4(-1.5, -4.0, 1.3, 0.2), vec4(-0.5, -7.5, 1.1, 0.2), vec4(1.5, -10.0, 1.1, 0.2),
    vec4(4.5, -11.5, 1.3, 0.2), vec4(7.0, -10.5, 1.4, 0.15), vec4(7.5, -8.0, 2.0, 0.1),
    vec4(8.5, -8.4, 1.1, 0.15),
    // the Swan
    vec4(0.0, 9.0, 2.5, 0.15), vec4(0.0, 2.5, 1.5, 0.45), vec4(-6.5, 0.5, 1.4, 0.6),
    vec4(-11.5, -1.0, 0.8, 0.5), vec4(6.0, 4.5, 1.3, 0.2), vec4(10.0, 6.0, 0.8, 0.3),
    vec4(0.5, -3.5, 1.0, 0.5), vec4(1.0, -10.0, 1.4, 0.95),
    // the Throne
    vec4(-9.0, 1.5, 1.4, 0.2), vec4(-4.5, -2.5, 1.5, 0.3), vec4(0.0, 0.5, 1.8, 0.7),
    vec4(4.5, -3.0, 1.5, 0.25), vec4(8.5, 0.5, 1.3, 0.2),
);

fn hash(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3(12.9898, 78.233, 37.719))) * 43758.5453);
}

// For whole-numbered `p`. An integer hash stays random however far the clouds drift.
fn hash2(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(p);
    return unit(pcg(u32(i.x) + pcg(u32(i.y) ^ 0x9e3779b9u)));
}

// Integer hash (PCG) for star cells, free of the banding a sine hash shows at large inputs.
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2(1.0, 0.0));
    let c = hash2(i + vec2(0.0, 1.0));
    let d = hash2(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Cloud noise. The finer octaves slide slowly over the coarse ones as seconds `t` pass,
// so shapes keep forming and breaking up as they drift.
fn fbm(p: vec2<f32>, t: f32) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
        value += value_noise(q) * amplitude;
        q = q * 2.03 + vec2(17.1, 9.3) + t * vec2(0.006, -0.004);
        amplitude *= 0.5;
    }
    return value;
}

// Feature sizes of the low cloud's body and the billows at its edges, and of the higher
// sheet, relative to the layers' coordinates.
const CLOUD_SCALE: f32 = 0.6;
const DETAIL_SCALE: f32 = 7.0;
const SHEET_SCALE: f32 = 0.5;

// The broad body of low cloud, about 0..1: three coarse octaves over a smooth warp, so
// outlines bulge and curl instead of following the noise grid. Like `fbm`, the finer
// octaves slide slowly over the coarse one.
fn cloud_body(p: vec2<f32>, t: f32) -> f32 {
    let warp = vec2(value_noise(p * 0.5 + vec2(5.2, 1.3)), value_noise(p * 0.5 + vec2(-3.7, 8.1))) - 0.5;
    var q = p + warp * 0.8;
    var value = 0.0;
    var amplitude = 0.5;
    for (var i = 0; i < 3; i++) {
        value += value_noise(q) * amplitude;
        q = q * 2.03 + vec2(17.1, 9.3) + t * vec2(0.006, -0.004);
        amplitude *= 0.5;
    }
    return value / 0.875;
}

// Billows and wisps, 0..1, that shape only the edges of cloud.
fn cloud_detail(p: vec2<f32>, t: f32) -> f32 {
    let q = p + t * vec2(0.01, -0.007);
    return value_noise(q) * 0.65 + value_noise(q * 2.07 + vec2(7.0, 3.0)) * 0.35;
}

fn value_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x00 = mix(hash(i), hash(i + vec3(1.0, 0.0, 0.0)), u.x);
    let x10 = mix(hash(i + vec3(0.0, 1.0, 0.0)), hash(i + vec3(1.0, 1.0, 0.0)), u.x);
    let x01 = mix(hash(i + vec3(0.0, 0.0, 1.0)), hash(i + vec3(1.0, 0.0, 1.0)), u.x);
    let x11 = mix(hash(i + vec3(0.0, 1.0, 1.0)), hash(i + vec3(1.0, 1.0, 1.0)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

fn fbm3(p: vec3<f32>) -> f32 {
    return value_noise3(p) * 0.55 + value_noise3(p * 2.1 + 5.3) * 0.3 + value_noise3(p * 4.3 + 1.7) * 0.15;
}

// Star colours from hot blue-white through white and yellow to cool orange.
fn star_tint(t: f32) -> vec3<f32> {
    let cool = mix(vec3(0.62, 0.75, 1.0), vec3(1.0), smoothstep(0.0, 0.45, t));
    return mix(cool, vec3(1.0, 0.66, 0.38), smoothstep(0.5, 1.0, t));
}

// A star as a soft point `d` pixels away, larger and with a faint halo when bright.
fn star_glow(d: f32, brightness: f32) -> f32 {
    let r = 0.6 + 0.5 * brightness;
    let x = d * d / (r * r);
    return brightness * (exp(-x) + 0.05 * exp(-x * 0.06));
}

// Length in pixels of an offset `o`, given how its coordinates change per pixel (`j`, with
// columns d/dx and d/dy). Measuring stars in pixels keeps them round and crisp anywhere on
// screen. `at_least`, the length where pixels span the widest angle, guards against the
// meaningless derivatives along seams.
fn pixels(o: vec2<f32>, j: mat2x2<f32>, at_least: f32) -> f32 {
    let s = vec2(j[1].y * o.x - j[1].x * o.y, j[0].x * o.y - j[0].y * o.x) / determinant(j);
    return max(length(s), at_least);
}

// One layer of background stars. The sphere is split into the six faces of a cube with
// equal-angle cells; a cell holds at most one star at a random spot away from its edges.
fn star_layer(p: vec3<f32>, cells: f32, density: f32, brightest: f32, seed: u32, px: f32) -> vec3<f32> {
    let a = abs(p);
    var uv: vec2<f32>;
    var face = seed;
    if a.x > a.y && a.x > a.z {
        uv = p.yz / a.x;
        face += select(0u, 1u, p.x < 0.0);
    } else if a.y > a.z {
        uv = p.zx / a.y;
        face += select(2u, 3u, p.y < 0.0);
    } else {
        uv = p.xy / a.z;
        face += select(4u, 5u, p.z < 0.0);
    }
    let g = (atan(uv) * (2.0 / PI) + 0.5) * cells;
    let j = mat2x2(dpdx(g), dpdy(g));
    let cell = floor(g);
    var h = pcg(u32(cell.x) + pcg(u32(cell.y) + pcg(face)));
    if unit(h) > density {
        return vec3(0.0);
    }
    h = pcg(h);
    let x = unit(h);
    h = pcg(h);
    let y = unit(h);
    h = pcg(h);
    let size = unit(h);
    h = pcg(h);
    // Most stars look white; strongly coloured ones are rarer.
    let spread = unit(h) * 2.0 - 1.0;
    let tint = 0.5 + 0.5 * spread * spread * spread;
    let o = g - cell - 0.2 - 0.6 * vec2(x, y);
    // Cells are at least 0.9 times as wide as at the middle of a cube face.
    let d = pixels(o, j, length(o) * (0.45 * PI / cells) / px);
    let brightness = brightest * (0.03 + 0.97 * size * size * size * size);
    return star_tint(tint) * star_glow(d, brightness);
}

// Whether `sph` (longitude, latitude) may lie within `radius` degrees of a point.
fn near(sph: vec2<f32>, lon: f32, lat: f32, radius: f32) -> bool {
    let dlon = abs(fract((sph.x - lon) / 360.0 + 0.5) - 0.5) * 360.0;
    return abs(sph.y - lat) < radius && dlon * cos(radians(sph.y)) < radius * 1.2;
}

// Directions east and north on the sky at (lon, lat).
fn frame(lon: f32, lat: f32) -> mat2x3<f32> {
    let l = radians(lon);
    let b = radians(lat);
    return mat2x3(vec3(-sin(l), cos(l), 0.0), vec3(-sin(b) * cos(l), -sin(b) * sin(l), cos(b)));
}

// Coordinates of `p` on a plane touching the sky at (lon, lat), in degrees, x east, y north.
fn tangent(p: vec3<f32>, lon: f32, lat: f32) -> vec2<f32> {
    return degrees(p * frame(lon, lat));
}

// `dpx` and `dpy`: change of `p` per pixel; `px`: degrees per pixel mid-screen.
fn constellations(p: vec3<f32>, sph: vec2<f32>, dpx: vec3<f32>, dpy: vec3<f32>, px: f32) -> vec3<f32> {
    var light = vec3(0.0);
    var first = 0u;
    for (var i = 0u; i < 8u; i++) {
        let group = CONSTELLATIONS[i];
        let last = first + u32(group.w);
        if near(sph, group.x, group.y, group.z) {
            let f = frame(group.x, group.y);
            let q = degrees(p * f);
            let j = mat2x2(degrees(dpx * f), degrees(dpy * f));
            for (var s = first; s < last; s++) {
                let star = STARS[s];
                let o = q - star.xy;
                light += star_tint(star.w) * star_glow(pixels(o, j, length(o) / px), star.z);
            }
        }
        first = last;
    }
    return light;
}

// The Milky Way: a soft, patchy band, warmer and wider towards its core, with a dark lane
// down the middle. Returns colour and, in w, how strongly the band is there.
fn milky_way(p: vec3<f32>) -> vec4<f32> {
    let core = dot(p, MILKY_CORE) * 0.5 + 0.5;
    let width = 0.13 + 0.08 * core;
    let h = dot(p, MILKY_POLE);
    let band = exp(-h * h / (width * width));
    if band < 0.02 {
        return vec4(0.0);
    }
    let clumps = fbm3(p * 9.0);
    let rift = value_noise3(p * 5.0 + 2.0);
    let across = (h + 0.06 * (rift - 0.5)) / (0.02 + 0.025 * core);
    let lane = exp(-across * across) * smoothstep(0.25, 0.6, rift) * core;
    let glow = band * (0.4 + 0.9 * clumps * clumps) * (1.0 - 0.7 * lane) * (0.5 + core * core);
    let tint = mix(vec3(0.55, 0.62, 0.9), vec3(0.95, 0.84, 0.68), core * core);
    return vec4(tint * glow * 0.07, band);
}

// A small spiral galaxy: a warm core in a bluish disc with two arms, tilted and flattened
// by how we see it.
fn galaxy(q: vec2<f32>, size: f32, tilt: f32, flatten: f32) -> vec3<f32> {
    let c = cos(radians(tilt));
    let s = sin(radians(tilt));
    let g = vec2(c * q.x + s * q.y, (c * q.y - s * q.x) / flatten) / size;
    let r = length(g);
    let arms = 0.5 + 0.5 * cos(2.0 * atan2(g.y, g.x) - 6.0 * log(r + 0.02));
    let disc = exp(-3.0 * r) * (0.45 + 0.55 * arms * smoothstep(0.05, 0.35, r));
    let core = exp(-r * r * 80.0);
    return vec3(0.6, 0.7, 1.0) * disc * 0.08 + vec3(1.0, 0.86, 0.65) * core * 0.12;
}

// A soft, coloured cloud of gas or distant stars; `ragged` 0..1 sets how wispy it is.
fn nebula(p: vec3<f32>, q: vec2<f32>, size: f32, tint: vec3<f32>, ragged: f32) -> vec3<f32> {
    let shape = exp(-dot(q, q) / (size * size));
    let wisps = fbm3(p * 70.0);
    return tint * shape * mix(1.0 - ragged, 1.0, smoothstep(0.3, 0.8, wisps));
}

// Faint galaxies and nebulae at fixed places.
fn deep_sky(p: vec3<f32>, sph: vec2<f32>) -> vec3<f32> {
    var light = vec3(0.0);
    if near(sph, 300.0, 30.0, 6.0) {
        light += galaxy(tangent(p, 300.0, 30.0), 3.0, 35.0, 0.35);
    }
    if near(sph, 170.0, 5.0, 3.0) {
        light += galaxy(tangent(p, 170.0, 5.0), 1.0, -20.0, 0.8);
    }
    if near(sph, 125.0, -30.0, 3.0) {
        light += galaxy(tangent(p, 125.0, -30.0), 1.2, 70.0, 0.35);
    }
    // In the Hunter's sword, and around the Sisters.
    if near(sph, 255.4, -9.2, 3.0) {
        light += nebula(p, tangent(p, 255.4, -9.2), 0.7, vec3(0.14, 0.05, 0.08), 0.6);
    }
    if near(sph, 282.4, 16.0, 3.0) {
        light += nebula(p, tangent(p, 282.4, 16.0), 1.1, vec3(0.03, 0.05, 0.12), 0.5);
    }
    // Two irregular dwarf galaxies near the pole, circling low over the horizon.
    if near(sph, 200.0, -66.0, 9.0) {
        light += nebula(p, tangent(p, 200.0, -66.0) * vec2(1.0, 1.6), 3.5, vec3(0.03, 0.033, 0.04), 0.4);
    }
    if near(sph, 232.0, -72.0, 6.0) {
        light += nebula(p, tangent(p, 232.0, -72.0) * vec2(1.0, 1.4), 2.0, vec3(0.025, 0.028, 0.035), 0.4);
    }
    return light;
}

// A planet as a small lit disc with a soft halo. Each wanders slowly against the stars.
// `kind` 0: rusty with a polar cap, 1: pale blue, 2: banded and ringed.
fn planet(p: vec3<f32>, sph: vec2<f32>, lon: f32, lat: f32, radius: f32, kind: u32, px: f32) -> vec4<f32> {
    if !near(sph, lon, lat, radius * 6.0) {
        return vec4(0.0);
    }
    let q = tangent(p, lon, lat) / radius;
    let r2 = dot(q, q);
    let aa = px / radius;
    // Lit from the side facing the sun, nearly full as seen from here.
    let to_sun = normalize(tangent(SUN, lon, lat));
    let light = normalize(vec3(to_sun * 0.7, 1.0));
    let normal = vec3(q, sqrt(max(1.0 - r2, 0.0)));
    let lit = 0.12 + 0.88 * max(dot(normal, light), 0.0);

    let disc = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, sqrt(r2));
    var tint: vec3<f32>;
    var surface: vec3<f32>;
    var ring = 0.0;
    if kind == 0u {
        tint = vec3(1.0, 0.42, 0.22);
        let patches = value_noise(q * 2.5 + 11.0);
        surface = mix(tint, vec3(0.62, 0.24, 0.14), smoothstep(0.45, 0.75, patches));
        surface = mix(surface, vec3(1.0, 0.95, 0.9), smoothstep(0.78, 0.86, q.y));
    } else if kind == 1u {
        tint = vec3(0.62, 0.85, 1.0);
        surface = tint * (0.92 + 0.08 * sin(q.y * 7.0));
    } else {
        tint = vec3(1.0, 0.86, 0.6);
        // Bands and ring share a tilt; the ring has a dark gap and its far half passes
        // behind the planet.
        let t = vec2(q.x * 0.94 + q.y * 0.34, q.y * 0.94 - q.x * 0.34);
        surface = mix(tint, vec3(0.8, 0.6, 0.4), 0.5 + 0.5 * sin(t.y * 11.0 + sin(t.x * 3.0)));
        let e = length(vec2(t.x, t.y / 0.32));
        let gap = smoothstep(1.7, 1.75, e) * (1.0 - smoothstep(1.8, 1.85, e));
        ring = smoothstep(1.35, 1.45, e) * (1.0 - smoothstep(2.05, 2.2, e)) * (1.0 - 0.6 * gap);
        ring *= select(1.0 - disc, 1.0, t.y < 0.0);
    }
    var color = mix(surface * lit * disc, vec3(0.95, 0.85, 0.7) * 0.5, ring);
    color += tint * exp(-r2 / 9.0) * 0.06;
    return vec4(color, max(disc, ring));
}

// Where `dir` falls on a disc of angular `radius` around `center`, measured in pixels like
// the stars so the disc stays round anywhere on screen at any field of view: xy in disc
// radii, oriented east and north on the sky, and z the pixel's coverage. `ddx` and `ddy`
// are the change of `dir` per pixel; `px` is the angle a pixel spans mid-screen, which
// sets the disc's size.
fn disc(dir: vec3<f32>, ddx: vec3<f32>, ddy: vec3<f32>, center: vec3<f32>, radius: f32, px: f32) -> vec3<f32> {
    let east = normalize(vec3(-center.y, center.x, 0.0));
    let f = mat2x3(east, cross(center, east));
    let j = mat2x2(ddx * f, ddy * f);
    // Pixels per unit of east and north (the inverse of `j`), and the pixel offset.
    let inv = mat2x2(vec2(j[1].y, -j[0].y), vec2(-j[1].x, j[0].x)) * (1.0 / determinant(j));
    let s = inv * (dir * f);
    let e = normalize(inv[0]);
    let n = normalize(inv[1] - dot(inv[1], e) * e);
    let r = radius / px;
    return vec3(vec2(dot(s, e), dot(s, n)) / r, clamp(r - length(s) + 0.5, 0.0, 1.0));
}

// Cloud as seen from below, in premultiplied colour and opacity: a low layer whose cover,
// build and darkness follow the weather, a higher sheet that closes its gaps as the sky
// clouds over, and high wisps of cirrus. All drift with the wind and sit at fixed places in
// the world, so they pass overhead as the player moves.
fn clouds(dir: vec3<f32>, daylight: f32, glow: f32) -> vec4<f32> {
    let rain = sky.weather.z;
    // Rain only falls from a well covered sky.
    let cover = min(sky.clouds.y + 0.25 * rain, 1.0);
    let convection = sky.clouds.z;
    let cirrus = sky.clouds.w;
    let t = sky.clouds.x;
    if dir.y <= 0.01 || cover + cirrus < 0.005 {
        return vec4(0.0);
    }
    let fade = smoothstep(0.01, 0.2, dir.y);
    // The layers bend down towards the horizon so they do not stretch out to infinity. Far
    // away, low cloud fades into the sky's colour rather than thinning out.
    let to_layer = dir.xz / (dir.y + 0.08);
    let edge_of_view = smoothstep(0.01, 0.05, dir.y);
    let haze = (1.0 - smoothstep(0.02, 0.2, dir.y)) * 0.8;
    let behind = mix(sky.horizon.rgb, sky.zenith.rgb, pow(dir.y, 0.55));
    let sun = sky.sun.xyz;

    // Light on the clouds: white by day, warm when the sun is low, pale blue moonlight.
    let low_sun = 1.0 - smoothstep(0.0, 0.35, sun.y);
    let sunlit = mix(vec3(1.0, 0.98, 0.95), vec3(1.0, 0.7, 0.5), low_sun);
    let lit = mix(vec3(0.6, 0.68, 0.85), sunlit, smoothstep(-0.2, -0.05, sun.y)) * mix(0.07, 1.0, daylight);
    let flash = vec3(0.75, 0.8, 1.0) * sky.weather.w;

    var result = vec4(0.0);
    if cirrus > 0.005 {
        // Higher, so it moves less as the player moves; streaked along the wind.
        let high = to_layer * 0.7 + sky.weather.xy * 0.35 + vec2(37.0, -11.0);
        let w = sky.wind.xy;
        let along = vec2(dot(high, w), dot(high, vec2(-w.y, w.x)));
        let streaks = fbm(along * vec2(0.7, 5.0), t);
        let patches = value_noise(high * 0.9);
        let density = smoothstep(0.48, 0.75, streaks) * smoothstep(0.3, 0.7, patches) * min(cirrus * 0.9, 0.65) * fade;
        result = vec4((lit * 1.1 + flash) * density, density);
    }
    if cover < 0.005 {
        return result;
    }
    // A thick layer lets little light through even where it is thin, and rain and storm
    // clouds hold back the most. Their shaded parts take the grey of the sky beneath.
    let storm = rain * (0.4 + 0.6 * convection);
    let through = (1.0 - 0.45 * smoothstep(0.6, 1.0, cover)) * (1.0 - 0.8 * storm);
    let grey = dot(sky.horizon.rgb, vec3(0.3, 0.55, 0.15));
    let shade = (mix(sky.horizon.rgb, vec3(grey), 0.5) * 0.45 + vec3(0.05, 0.055, 0.07) * daylight) * (1.0 - 0.75 * storm);

    // As the sky clouds over, a higher, even sheet spreads above the low cloud, so its gaps
    // show more cloud rather than blue sky.
    let sheet_cover = smoothstep(0.45, 0.95, cover);
    if sheet_cover > 0.0 {
        let high = to_layer * 1.2 + sky.weather.xy * 0.3 + vec2(-23.0, 41.0);
        let n = cloud_body(high * SHEET_SCALE, t);
        let density = smoothstep(1.0 - sheet_cover, 1.35 - sheet_cover, n + 0.25) * edge_of_view;
        let color = mix(mix(lit * through, shade, 0.3 + 0.35 * rain - 0.2 * n) + flash * 0.5, behind, haze);
        result = vec4(color * density + result.rgb * (1.0 - density), density + result.a * (1.0 - density));
    }

    let at = to_layer * 2.0 + sky.weather.xy;
    let body = cloud_body(at * CLOUD_SCALE, t);
    // How far above the threshold for cloud the body rises, from 0 at its edge: the
    // threshold sets how much of the sky is covered.
    let threshold = mix(0.72, 0.0, pow(cover, 1.3));
    let core = (body - threshold) / (1.0 - threshold);
    // Fine detail bites into and bulges out of the edges but leaves the cores whole: billows
    // on heaped cloud, soft frays on layers, and little of either once the sky is covered.
    let detail = cloud_detail(at * DETAIL_SCALE, t) - 0.5;
    let fray = mix(0.3, 0.4, convection) * (1.0 - 0.7 * smoothstep(0.6, 1.0, cover));
    let d = core + (detail - 0.15) * fray * (1.0 - clamp(core * 3.0, 0.0, 1.0));
    // Opacity rises quickly above the edge, so clouds are solid rather than see-through;
    // heaped cloud has crisper edges than layers.
    let density = smoothstep(0.0, mix(0.2, 0.1, convection), d) * smoothstep(0.0, 0.1, cover) * edge_of_view;
    if density > 0.0 {
        // Thin edges glow with the light coming through; thick cloud is dark underneath,
        // most of all overhead, where we see its base rather than its sunlit flanks. The
        // side away from the sun lies in the shadow of the cloud towards it.
        let depth = smoothstep(0.0, 0.4, d);
        let thick = smoothstep(0.2, 0.85, body);
        let underside = smoothstep(0.1, 0.7, dir.y);
        let towards_sun = cloud_body((at + normalize(sun.xz + vec2(1e-4, 0.0)) * 0.1) * CLOUD_SCALE, t);
        let shadowed = clamp(0.5 + (towards_sun - body) * 6.0, 0.0, 1.0) * daylight;
        // A low sun lights the bases from the side.
        let base = thick * mix(0.2, 0.6, underside) * (1.0 - 0.5 * low_sun);
        var dark = depth * (base + shadowed * 0.3) + detail * 0.25 * depth;
        // Layers are flatter and duller; rain clouds are dark, storm clouds darkest, and
        // darkest where they are thickest.
        dark += 0.2 * smoothstep(0.6, 1.0, cover) * (1.0 - convection) + rain * mix(0.25, 0.4, convection) * (0.5 + thick);
        dark = clamp(dark, 0.0, 0.95);
        var color = mix(lit * 1.4 * through, shade * (1.3 - 0.6 * thick * storm), dark);
        color += vec3(1.0, 0.85, 0.6) * glow * (1.0 - depth) * 2.0 + flash * (0.5 + 0.5 * depth);
        color = mix(color, behind, haze);
        result = vec4(color * density + result.rgb * (1.0 - density), density + result.a * (1.0 - density));
    }
    return result;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position);
    let ddx = dpdx(dir);
    let ddy = dpdy(dir);
    // Angle a pixel spans mid-screen, where pixels span the widest angle.
    let px = 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    let up = max(dir.y, 0.0);
    var color = mix(sky.horizon.rgb, sky.zenith.rgb, pow(up, 0.55));
    if dir.y < 0.0 {
        color = sky.horizon.rgb * mix(1.0, 0.7, min(-dir.y * 4.0, 1.0));
    }

    let daylight = sky.sun.w;
    let night = 1.0 - daylight;
    let sun_dot = dot(dir, sky.sun.xyz);
    // Thick cloud spreads the sun's glow into a dull bright patch.
    let overcast = smoothstep(0.4, 1.0, sky.clouds.y);
    let glow = (pow(max(sun_dot, 0.0), 48.0) * 0.35 + pow(max(sun_dot, 0.0), 6.0) * 0.1) * (1.0 - 0.7 * overcast);
    let cloud = clouds(dir, daylight, glow);

    // The night sky appears as daylight fades: planets first, then bright stars, then the
    // faint ones. It hides behind thick clouds and the haze near the horizon.
    let dusk = 1.0 - smoothstep(0.1, 0.55, daylight);
    let dark = 1.0 - smoothstep(0.05, 0.35, daylight);
    let deep = 1.0 - smoothstep(0.0, 0.12, daylight);
    if dusk > 0.0 {
        // Turn into the frame of the stars, which turns with the sun.
        let angle = sky.stars.x * TAU;
        let p = vec3(
            dir.x * cos(angle) + dir.y * sin(angle),
            dir.y * cos(angle) - dir.x * sin(angle),
            dir.z,
        );
        let dpx = dpdx(p);
        let dpy = dpdy(p);
        let sph = vec2(degrees(atan2(p.y, p.x)), degrees(asin(clamp(p.z, -1.0, 1.0))));
        let px_deg = degrees(px);
        let clear = smoothstep(-0.02, 0.18, dir.y) * (1.0 - smoothstep(0.15, 0.6, cloud.a));

        var night_sky = vec3(0.0);
        if dark > 0.0 {
            night_sky = constellations(p, sph, dpx, dpy, px_deg) * dark;
            night_sky += star_layer(LAYER_TURN * p, 48.0, 0.09, 0.55, 8u, px) * dark * dark;
            if deep > 0.0 {
                let milky = milky_way(p);
                let faint = star_layer(p, 150.0, 0.06 + 0.16 * milky.w, 0.35, 0u, px);
                night_sky += (milky.rgb + faint + deep_sky(p, sph)) * deep * deep;
            }
        }

        // Planet longitudes swing slowly over game days; the periods divide the 1000 days
        // after which the day count wraps.
        let days = sky.stars.y;
        let red = planet(p, sph, 200.0 + 30.0 * sin(days * TAU / 100.0), -8.0, 0.4, 0u, px_deg);
        let blue = planet(p, sph, 300.0 + 20.0 * sin(days * TAU / 40.0 + 2.0), 6.0, 0.42, 1u, px_deg);
        let ringed = planet(p, sph, 140.0 + 25.0 * sin(days * TAU / 250.0 + 4.0), 14.0, 0.45, 2u, px_deg);
        let planets = red + blue + ringed;
        night_sky = night_sky * (1.0 - min(planets.w, 1.0)) + planets.rgb * dusk;
        color += night_sky * clear;
    }

    color += vec3(1.0, 0.8, 0.55) * glow * clamp(daylight * 2.0, 0.0, 1.0);
    if sun_dot > 0.999 {
        let sun = disc(dir, ddx, ddy, sky.sun.xyz, 0.0275, px);
        color = mix(color, vec3(8.0, 7.2, 5.6), sun.z);
    }

    // The moon, opposite the sun and so always full, with faint darker seas.
    if -sun_dot > 0.999 && night > 0.0 {
        let moon = disc(dir, ddx, ddy, -sky.sun.xyz, 0.0237, px);
        let q = moon.xy;
        let seas = smoothstep(0.5, 0.7, value_noise(q * 2.2 + 5.0) * 0.7 + value_noise(q * 5.0 + 1.0) * 0.3);
        let limb = mix(0.8, 1.0, sqrt(max(1.0 - dot(q, q), 0.0)));
        color = mix(color, vec3(1.5, 1.55, 1.7) * limb * (1.0 - 0.22 * seas), moon.z * night);
    }

    // Fog and ground fog hide the sky exactly as they hide the far edge of the land.
    color = color * (1.0 - cloud.a) + cloud.rgb;
    return vec4(mix(color, atmosphere.fog.rgb, edge_fog(atmosphere, view.world_position, dir)), 1.0);
}
