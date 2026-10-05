// Sky dome: horizon-to-zenith gradient, clouds shaped by the weather, sun and moon, and a
// night sky of stars, constellations, the Milky Way, faint galaxies, planets and comets,
// laid out for each world (night.rs), that turns with the sun and, over a year, against it.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view
#import voxmar::atmosphere::{Atmosphere, band, dusk_tint, edge_fog, under_water}
#import voxmar::clouds::{pcg, unit, value_noise, cloud_body}

struct SkyParams {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: direction towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // x: seconds of game time, y: low cloud cover 0..1, z: how heaped it is, from flat
    // layers (0) to towers (1), w: cirrus 0..1
    clouds: vec4<f32>,
    // x: fraction of the day, y: days elapsed (wrapped every 1000 days), z: fraction of
    // the year
    stars: vec4<f32>,
    // xyz: direction towards the moon, w: how much of it is lit, 0 (new) to 1 (full)
    moon: vec4<f32>,
    // xy: where the cloud layer is over the camera (its position and wind drift, in cloud
    // units), z: precipitation 0..1, w: lightning flash 0..1
    weather: vec4<f32>,
    // Light in the sky, 0..1: x rainbow (above 0.75 a double one), y halo (above 0.5 with
    // sun dogs), z aurora, w falling stars (1 a meteor shower)
    optics: vec4<f32>,
};

// The world's night sky; see night.rs for what each holds.
struct SkyLayout {
    constellations: array<vec4<f32>, 10>,
    stars: array<vec4<f32>, 96>,
    planets: array<vec4<f32>, 3>,
    orbits: array<vec4<f32>, 3>,
    deep_sky: array<vec4<f32>, 8>,
    deep_look: array<vec4<f32>, 8>,
    comets: array<vec4<f32>, 3>,
    comet_looks: array<vec4<f32>, 3>,
    milky_pole: vec4<f32>,
    milky_core: vec4<f32>,
    seeds: vec4<u32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyParams;
// The terrain's air (atmosphere.wgsl): fog and ground fog hide the sky as they hide the land.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> atmosphere: Atmosphere;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> heavens: SkyLayout;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;

// The night sky turns about the same axis as the sun (z). In the frame of the stars the
// sun sits at longitude 0 on day 0 of a year, and is drawn back through them over it.
const SUN = vec3(0.97014, 0.0, 0.24254);
// Turns the coarse star layer so its cube seams do not line up with the fine one.
const LAYER_TURN = mat3x3<f32>(
    vec3(0.6486, 0.6821, -0.3376), vec3(-0.5740, 0.7297, 0.3715), vec3(0.4998, -0.0472, 0.8649)
);

fn hash(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3(12.9898, 78.233, 37.719))) * 43758.5453);
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

// Feature size of the higher sheet, relative to its coordinates.
const SHEET_SCALE: f32 = 0.5;
// The direction cirrus streaks run in, along the winds high above, which hardly change.
const HIGH_WIND = vec2(0.906, 0.423);

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

// A star as a soft point `d` pixels away, a little larger and with a faint halo when
// bright.
fn star_glow(d: f32, brightness: f32) -> f32 {
    let r = 0.55 + 0.35 * brightness;
    let x = d * d / (r * r);
    return brightness * (exp(-x) + 0.02 * exp(-x * 0.1));
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
    for (var i = 0u; i < 10u; i++) {
        let group = heavens.constellations[i];
        let last = first + u32(group.w);
        if group.w > 0.0 && near(sph, group.x, group.y, group.z) {
            let f = frame(group.x, group.y);
            let q = degrees(p * f);
            let j = mat2x2(degrees(dpx * f), degrees(dpy * f));
            for (var s = first; s < last; s++) {
                let star = heavens.stars[s];
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
    let core = dot(p, heavens.milky_core.xyz) * 0.5 + 0.5;
    let width = 0.13 + 0.08 * core;
    let h = dot(p, heavens.milky_pole.xyz);
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
    return vec4(tint * glow * 0.03, band);
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

// The world's faint galaxies and nebulae.
fn deep_sky(p: vec3<f32>, sph: vec2<f32>) -> vec3<f32> {
    var light = vec3(0.0);
    for (var i = 0u; i < 8u; i++) {
        let object = heavens.deep_sky[i];
        if object.w < 0.0 || !near(sph, object.x, object.y, object.z * 2.5 + 1.0) {
            continue;
        }
        let look = heavens.deep_look[i];
        let q = tangent(p, object.x, object.y);
        let kind = u32(object.w);
        if kind == 0u {
            light += galaxy(q, object.z, look.x, look.y);
        } else if kind == 1u {
            light += nebula(p, q, object.z, vec3(0.14, 0.05, 0.08), look.z);
        } else if kind == 2u {
            light += nebula(p, q, object.z, vec3(0.03, 0.05, 0.12), look.z);
        } else {
            light += nebula(p, q * vec2(1.0, look.y), object.z, vec3(0.03, 0.033, 0.04), look.z);
        }
    }
    return light;
}

// A planet as a small lit disc with a soft halo, at (lon, lat) among the stars, `sun` the
// sun's direction in their frame. `kind` 0: rusty with a polar cap, 1: pale blue, 2:
// banded and ringed.
fn planet(p: vec3<f32>, sph: vec2<f32>, lon: f32, lat: f32, radius: f32, kind: u32, px: f32, sun: vec3<f32>) -> vec4<f32> {
    if !near(sph, lon, lat, radius * 6.0) {
        return vec4(0.0);
    }
    let q = tangent(p, lon, lat) / radius;
    let r2 = dot(q, q);
    let aa = px / radius;
    // Lit from the side facing the sun, nearly full as seen from here.
    let to_sun = normalize(tangent(sun, lon, lat));
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

// Comet `i`, while it visits on `days`: a fuzzy head drifting slowly against the stars, with
// a tail streaming away from the sun (`sun`, in the stars' frame), widening and fading,
// bent a little as a dust tail is. It brightens over its first days and fades over its last.
fn comet(p: vec3<f32>, sph: vec2<f32>, i: u32, days: f32, sun: vec3<f32>) -> vec3<f32> {
    let visit = heavens.comets[i];
    let age = (days - visit.x) / visit.y;
    if visit.y <= 0.0 || age < 0.0 || age > 1.0 {
        return vec3(0.0);
    }
    let look = heavens.comet_looks[i];
    let elapsed = days - visit.x;
    let lat = visit.w + look.y * elapsed;
    let lon = visit.z + look.x * elapsed / max(cos(radians(lat)), 0.2);
    let tail = look.z;
    if !near(sph, lon, lat, tail + 2.0) {
        return vec3(0.0);
    }
    let l = radians(lon);
    let b = radians(lat);
    let head = vec3(cos(b) * cos(l), cos(b) * sin(l), sin(b));
    let away = normalize(head * dot(head, sun) - sun);
    let side = cross(head, away);
    // Degrees from the head along the tail, and across it.
    let along = degrees(atan2(dot(p, away), dot(p, head)));
    let across = degrees(asin(clamp(dot(p, side), -1.0, 1.0)));
    var light = vec3(0.0);
    if along > 0.0 && dot(p, head) > 0.0 {
        let t = along / tail;
        let width = 0.2 + 1.1 * t;
        let bent = across - 0.12 * along * t;
        light += vec3(0.75, 0.85, 1.0) * exp(-bent * bent / (width * width)) * exp(-2.5 * t) * 0.1;
    }
    let r = degrees(acos(clamp(dot(p, head), -1.0, 1.0)));
    light += vec3(0.8, 0.95, 0.9) * (exp(-r * r / 0.06) * 0.5 + exp(-r * r / 0.5) * 0.06);
    return light * smoothstep(0.0, 0.25, age) * (1.0 - smoothstep(0.6, 1.0, age)) * look.w;
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

// A 22° halo round the sun or moon (`light`), reddish on its inner edge, and with `strength`
// over 0.5 sun dogs: bright, slightly coloured spots 22° to either side at its height,
// strongest with the light low.
fn halo(dir: vec3<f32>, light: vec3<f32>, strength: f32) -> vec3<f32> {
    let a = degrees(acos(clamp(dot(dir, light), -1.0, 1.0)));
    if a > 30.0 {
        return vec3(0.0);
    }
    let edge = saturate((a - 21.5) / 1.5);
    var glow = mix(vec3(1.0, 0.72, 0.55), vec3(0.85, 0.92, 1.0), edge) * band(a, 21.5, 23.0) * 0.25 * strength;
    let dogs = saturate(2.0 * (strength - 0.5));
    if dogs > 0.0 {
        // Angles across (along the horizon, at the light's height) and up from the light.
        let flat_light = normalize(light.xz);
        let flat_dir = normalize(dir.xz);
        let across = degrees(acos(clamp(dot(flat_dir, flat_light), -1.0, 1.0))) * length(light.xz);
        let up = degrees(asin(clamp(dir.y, -1.0, 1.0)) - asin(clamp(light.y, -1.0, 1.0)));
        // Taller than wide, with a faint tail away from the light.
        let d = vec2(across - 22.0, up * 0.7);
        let spot = exp(-dot(d, d) / 3.0) + 0.15 * exp(-(up * up) / 2.0) * smoothstep(22.0, 23.0, across) * exp(-(across - 22.0) / 6.0);
        let low = 1.0 - smoothstep(0.3, 0.7, light.y);
        glow += mix(vec3(1.0, 0.6, 0.45), vec3(1.0, 0.97, 0.9), saturate((across - 21.0) / 2.0)) * spot * 1.5 * dogs * (0.4 + 0.6 * low);
    }
    return glow;
}

// The aurora: curtains low over the northern horizon (north is -z), from about 10° to 60°
// up, of fine vertical rays along folded ribbons that drift slowly, green at the bottom
// fading to violet at the top. `t` is seconds.
fn aurora(dir: vec3<f32>, t: f32) -> vec3<f32> {
    let north = -dir.z / max(length(dir.xz), 1e-4);
    if dir.y <= 0.0 || north < -0.3 {
        return vec3(0.0);
    }
    let across = atan2(dir.x, -dir.z);
    let up = degrees(asin(dir.y));
    // Two ribbons, each folding back and forth along the horizon as it drifts.
    var light = vec3(0.0);
    for (var i = 0; i < 2; i++) {
        let k = f32(i);
        let fold = value_noise(vec2(across * 2.2 + t * 0.012 + k * 7.0, k * 3.1)) - 0.5;
        let base = 11.0 + 7.0 * k + 9.0 * fold;
        let rise = up - base;
        if rise < -2.0 {
            continue;
        }
        let height = 25.0 + 15.0 * value_noise(vec2(across * 3.0 - t * 0.008, 11.0 + k));
        let foot = smoothstep(-2.0, 1.5, rise);
        let fade = exp(-max(rise, 0.0) / height);
        // Fine rays, flickering and drifting along the ribbon.
        let rays = 0.35 + 0.65 * value_noise(vec2(across * 90.0 + fold * 30.0 + t * 0.15, k * 5.0 + t * 0.05));
        let brightness = value_noise(vec2(across * 1.3 - t * 0.02, 20.0 + k)) * 1.4;
        let colour = mix(vec3(0.2, 1.0, 0.45), vec3(0.6, 0.3, 1.0), smoothstep(0.0, 35.0, rise));
        light += colour * foot * fade * rays * brightness * (1.0 - 0.4 * k);
    }
    return light * smoothstep(-0.3, 0.5, north) * 0.06;
}

// Falling stars: about two a second at `rate` 1 (a meteor shower), one every few minutes
// at an ordinary night's few thousandths, all streaking away from the night's one radiant:
// streaks 5° to 15° long that brighten then fade over one to two seconds. `t` is seconds,
// `night` the day's number (a new radiant each night), `px` the angle a pixel spans.
fn falling_stars(dir: vec3<f32>, t: f32, rate: f32, night: f32, px: f32) -> vec3<f32> {
    // Slots of half a second, each holding a star or not; a star lasts at most 2.2 s, so it
    // starts in one of the last five slots.
    let slot = floor(t * 2.0);
    var r = pcg(u32(night) * 7919u + 17u);
    let radiant = normalize(vec3(unit(r) * 2.0 - 1.0, 0.4 + 0.6 * unit(pcg(r)), unit(pcg(pcg(r))) * 2.0 - 1.0));
    var light = vec3(0.0);
    for (var k = 0; k < 5; k++) {
        let s = slot - f32(k);
        var h = pcg(u32(i32(s)) * 2654435761u ^ pcg(u32(night)));
        if unit(h) > rate {
            continue;
        }
        h = pcg(h);
        let start = s * 0.5 + unit(h) * 0.5;
        h = pcg(h);
        let life = 0.9 + 1.3 * unit(h);
        let age = (t - start) / life;
        if age < 0.0 || age > 1.0 {
            continue;
        }
        // Where it begins: high in the sky, 20° to 80° from the radiant.
        h = pcg(h);
        let side = normalize(cross(radiant, vec3(unit(h) - 0.5, 0.3, unit(pcg(h)) - 0.5)));
        h = pcg(pcg(h));
        let off = radians(20.0 + 60.0 * unit(h));
        let begin = normalize(radiant * cos(off) + side * sin(off));
        if begin.y < 0.35 {
            continue;
        }
        // It runs away from the radiant, along the great circle through both: `along` is
        // that way at its start, `normal` the circle's axis.
        let along = normalize(begin * dot(begin, radiant) - radiant);
        let normal = cross(begin, along);
        h = pcg(h);
        let span = radians(5.0 + 10.0 * unit(h));
        // How far behind the head the pixel's direction lies along the circle, and how far
        // off it, as angles.
        let back = span * age - atan2(dot(dir, along), dot(dir, begin));
        let tail = span * min(age * 1.5, 0.6);
        let across = abs(dot(dir, normal));
        if back < -px * 2.0 || back > tail + px * 2.0 || dot(dir, begin) < 0.0 {
            continue;
        }
        let width = across / px;
        let fade = sin(3.14159 * age) * (1.0 - saturate(back / max(tail, 1e-4)));
        light += vec3(0.9, 0.95, 1.0) * exp(-width * width * 0.8) * fade * 1.5;
    }
    return light;
}

// The high cloud, in premultiplied colour and opacity: an even sheet that spreads as the sky
// clouds over, so gaps in the low cloud (a layer in the world, drawn by the volumetric
// pass) show more cloud rather than blue sky, and wisps of cirrus streaked along the winds
// high above. Both drift with the wind and lie far above any mountain.
fn clouds(dir: vec3<f32>, daylight: f32) -> vec4<f32> {
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
    // away, cloud fades into the sky's colour rather than thinning out.
    let to_layer = dir.xz / (dir.y + 0.08);
    let edge_of_view = smoothstep(0.01, 0.05, dir.y);
    let haze = (1.0 - smoothstep(0.02, 0.2, dir.y)) * 0.8;
    let behind = mix(sky.horizon.rgb, sky.zenith.rgb, pow(dir.y, 0.55));
    let sun = sky.sun.xyz;

    // Light on the clouds: white by day, warm when the sun is low, pale blue moonlight.
    let low_sun = 1.0 - smoothstep(0.0, 0.35, sun.y);
    let sunlit = mix(vec3(1.0, 0.98, 0.95), vec3(1.0, 0.7, 0.5), low_sun);
    let lit = mix(vec3(0.6, 0.68, 0.85), sunlit, smoothstep(-0.2, -0.05, sun.y)) * mix(0.012, 1.0, daylight);
    let flash = vec3(0.75, 0.8, 1.0) * sky.weather.w;

    var result = vec4(0.0);
    if cirrus > 0.005 {
        // Higher, so it moves less as the player moves; streaked along the high winds. (The
        // streaks once turned with the gusts at the camera, swinging across the sky.)
        let high = to_layer * 0.7 + sky.weather.xy * 0.35 + vec2(37.0, -11.0);
        let w = HIGH_WIND;
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

    return result;
}

// Sunrise and sunset over `color`. A wide warm glow round the low sun, widest with the sun
// on the horizon; with the sun still under it, the glow stands over where it will rise (or
// has set), in rays fanning up from behind the land. The other way the sky dims, and in
// twilight the earth's shadow rises from the horizon as the sun sinks, under a faint pink
// band.
fn sunrise(color: vec3<f32>, dir: vec3<f32>, sun_dot: f32, overcast: f32) -> vec3<f32> {
    let sun = sky.sun.xyz;
    let low = smoothstep(-0.12, -0.02, sun.y) * (1.0 - smoothstep(0.08, 0.35, sun.y)) * (1.0 - 0.85 * overcast);
    if low <= 0.0 {
        return color;
    }
    let toward = max(sun_dot, 0.0);
    // The angle about the sun, 0 straight up from it; rays only while it is hidden.
    let side = normalize(cross(sun, vec3(0.0, 1.0, 0.0)));
    let around = atan2(dot(dir, side), dot(dir, cross(side, sun)));
    let rays = 0.4 + 0.6 * smoothstep(0.3, 0.7, value_noise(vec2(around * 11.0, 3.0 + sky.clouds.x * 0.002)));
    let hidden = (1.0 - smoothstep(-0.01, 0.04, sun.y)) * (1.0 - smoothstep(0.5, 1.5, abs(around)));
    // Brighter and wider once the sun itself shows.
    let risen = 1.0 + 0.8 * smoothstep(-0.02, 0.04, sun.y);
    let aureole = (pow(toward, 8.0) * 0.16 + pow(toward, 3.0) * 0.03) * risen * mix(1.0, rays, hidden * 0.6);
    var lit = color + vec3(1.0, 0.55, 0.28) * aureole * low * smoothstep(-0.06, 0.04, dir.y);

    let flat_dir = dir.xz / max(length(dir.xz), 1e-4);
    let flat_sun = sun.xz / max(length(sun.xz), 1e-4);
    let away = max(-dot(flat_dir, flat_sun), 0.0);
    lit *= 1.0 - 0.2 * low * away * away * (1.0 - smoothstep(0.0, 0.7, dir.y));
    let twilight = smoothstep(-0.2, -0.04, sun.y) * (1.0 - smoothstep(-0.02, 0.05, sun.y)) * (1.0 - 0.85 * overcast);
    let top = 0.02 + 0.6 * max(-sun.y, 0.0);
    let shadow = (1.0 - smoothstep(top - 0.03, top + 0.03, dir.y)) * away * away * twilight;
    let belt = exp(-pow((dir.y - top - 0.07) / 0.06, 2.0)) * away * twilight;
    let horizon = dot(sky.horizon.rgb, vec3(0.2126, 0.7152, 0.0722));
    return lit * (1.0 - 0.45 * shadow) + vec3(1.0, 0.55, 0.65) * belt * horizon * 0.45;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position);
    let ddx = dpdx(dir);
    let ddy = dpdy(dir);
    // Under water nothing of the sky shows but through the surface, which draws it itself:
    // only the water, deep beyond everything.
    if atmosphere.water_light.w > 0.0 {
        return vec4(under_water(atmosphere, view.world_position, dir * 1e4).light, 1.0);
    }
    // Angle a pixel spans mid-screen, where pixels span the widest angle.
    let px = 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    let up = max(dir.y, 0.0);
    // Warmer towards the low sun at dusk and dawn, cooler away from it.
    let horizon = sky.horizon.rgb * dusk_tint(atmosphere, dir);
    var color = mix(horizon, sky.zenith.rgb, pow(up, 0.55));
    if dir.y < 0.0 {
        color = horizon * mix(1.0, 0.7, min(-dir.y * 4.0, 1.0));
    }

    let daylight = sky.sun.w;
    let night = 1.0 - daylight;
    let sun_dot = dot(dir, sky.sun.xyz);
    // Thick cloud spreads the sun's glow into a dull bright patch.
    let overcast = smoothstep(0.4, 1.0, sky.clouds.y);
    let glow = (pow(max(sun_dot, 0.0), 48.0) * 0.35 + pow(max(sun_dot, 0.0), 6.0) * 0.1) * (1.0 - 0.7 * overcast);
    let cloud = clouds(dir, daylight);

    // The night sky appears as daylight fades: planets first, then bright stars, then the
    // faint ones. It hides behind thick clouds and the haze near the horizon.
    let dusk = 1.0 - smoothstep(0.1, 0.55, daylight);
    let dark = 1.0 - smoothstep(0.05, 0.35, daylight);
    let deep = 1.0 - smoothstep(0.0, 0.12, daylight);
    if dusk > 0.0 {
        // Turn into the frame of the stars, which turns with the sun, and against it over
        // the year: each season shows other constellations at night.
        let angle = (sky.stars.x + sky.stars.z) * TAU;
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
            night_sky += star_layer(LAYER_TURN * p, 48.0, 0.09, 0.45, heavens.seeds.x, px) * dark * dark;
            if deep > 0.0 {
                let milky = milky_way(p);
                let faint = star_layer(p, 150.0, 0.06 + 0.16 * milky.w, 0.32, heavens.seeds.y, px);
                night_sky += (milky.rgb + faint + deep_sky(p, sph)) * deep * deep;
            }
        }

        // The sun among the stars, drawn back through them over the year.
        let sun_lon = -sky.stars.z * 360.0;
        let sun = vec3(SUN.x * cos(radians(sun_lon)), SUN.x * sin(radians(sun_lon)), SUN.z);
        // Planets move along the sun's path at their own pace, an outer one round all the
        // stars, an inner one to and fro about the sun; their periods divide the 1000 days
        // after which the day count wraps.
        let days = sky.stars.y;
        var planets = vec4(0.0);
        for (var i = 0u; i < 3u; i++) {
            let look = heavens.planets[i];
            if look.w == 0.0 {
                continue;
            }
            let orbit = heavens.orbits[i];
            let lon = select(orbit.x + orbit.y * days, sun_lon + orbit.z * sin(TAU * days / orbit.y + orbit.x), orbit.w > 0.5);
            planets += planet(p, sph, lon, look.x, look.y, u32(look.z), px_deg, sun);
        }
        night_sky = night_sky * (1.0 - min(planets.w, 1.0)) + planets.rgb * dusk;
        if dark > 0.0 {
            for (var i = 0u; i < 3u; i++) {
                night_sky += comet(p, sph, i, days, sun) * dark;
            }
        }
        // The aurora and falling stars, in the sky's own frame.
        if sky.optics.z > 0.0 && dark > 0.0 {
            night_sky += aurora(dir, sky.clouds.x) * sky.optics.z * dark;
        }
        if sky.optics.w > 0.0 && dark > 0.0 {
            night_sky += falling_stars(dir, sky.clouds.x, sky.optics.w, floor(sky.stars.y), px) * dark;
        }
        color += night_sky * clear;
    }

    // A halo round the sun by day, fainter round the moon by night.
    if sky.optics.y > 0.0 {
        let light = select(sky.moon.xyz, sky.sun.xyz, sky.sun.y > 0.0);
        let bright = select(0.12 * night * sky.moon.w, daylight, sky.sun.y > 0.0);
        color += halo(dir, light, sky.optics.y) * bright;
    }

    color += vec3(1.0, 0.8, 0.55) * glow * clamp(daylight * 2.0, 0.0, 1.0);
    color = sunrise(color, dir, sun_dot, overcast);
    if sun_dot > 0.999 {
        let sun = disc(dir, ddx, ddy, sky.sun.xyz, 0.0275, px);
        color = mix(color, vec3(8.0, 7.2, 5.6), sun.z);
    }

    // The moon, with faint darker seas, lit on the side towards the sun: full opposite it,
    // a crescent beside it. By day it is pale. Its dark side hides the stars behind it,
    // faintly lit by the land.
    let to_moon = sky.moon.xyz;
    if dot(dir, to_moon) > 0.999 {
        let moon = disc(dir, ddx, ddy, to_moon, 0.0237, px);
        let q = moon.xy;
        let seas = smoothstep(0.5, 0.7, value_noise(q * 2.2 + 5.0) * 0.7 + value_noise(q * 5.0 + 1.0) * 0.3);
        let facing = sqrt(max(1.0 - dot(q, q), 0.0));
        let limb = mix(0.8, 1.0, facing);
        // The sun's direction in the disc's frame: east, north and towards the eye.
        let east = normalize(vec3(-to_moon.y, to_moon.x, 0.0));
        let north = cross(to_moon, east);
        let towards_sun = vec3(dot(sky.sun.xyz, east), dot(sky.sun.xyz, north), -dot(sky.sun.xyz, to_moon));
        let lit = smoothstep(-0.04, 0.06, dot(vec3(q, facing), towards_sun));
        let face = vec3(1.5, 1.55, 1.7) * limb * (1.0 - 0.22 * seas);
        color = mix(color, face, moon.z * lit * mix(1.0, 0.45, daylight));
        color = mix(color, face * 0.012, moon.z * (1.0 - lit) * night);
    }

    // Fog and ground fog hide the sky exactly as they hide the far edge of the land.
    color = color * (1.0 - cloud.a) + cloud.rgb;
    return vec4(mix(color, atmosphere.fog.rgb, edge_fog(atmosphere, view.world_position, dir)), 1.0);
}
