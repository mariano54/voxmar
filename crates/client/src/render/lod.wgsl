// Distant terrain (level of detail): smooth heightfield tiles generated from the world
// generator's surface functions rather than real chunks, with a sheet for water and ice.
//   word 0: x:6 | z:6 | y:14 | sky:4 | kind:2   (x, z in cells; y in 1/32 blocks)
//   word 1: r:8 | g:8 | b:8 | a:8   (sRGB albedo of top faces; a: canopy cover, or the
//                                    opacity of ice)
//   word 2: octahedral normal x:8 | z:8 | r:5 | g:6 | b:5 (sRGB albedo of tree crowns)
//   word 3: r:8 | g:8 | b:8 | a:8   (sRGB albedo of side faces, or for sheets of the floor
//                                    beneath; a: cover of small plants)

#import bevy_pbr::mesh_view_bindings::{view, fog}
#import voxmar::lighting::{FACE_SHADE, shade_normal, apply_fog, sun_direction, sun_color, sun_unclouded, reflected_cloud, horizon_along}
#import voxmar::water::{Water, absorption, scattering, transmittance, inscattered, fresnel, sky_gradient, wind_wave_slope, mirrored_sky}

struct DistantView {
    // xy: column of the coverage mask's first bit, z: the mask's width in columns, w: the
    // real chunks' radius in columns.
    coverage: vec4<i32>,
    // The sky's zenith and horizon colours, for water's reflection.
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xy: the block position across the land the rings are laid out around, which follows
    // the camera only once the finer rings' tiles there have arrived.
    rings: vec4<i32>,
};

// Tile origins (x, z) and cell size (w), by instance index; shared with the real chunks.
@group(2) @binding(0) var<storage, read> origins: array<vec4<i32>>;
@group(2) @binding(1) var<uniform> distant: DistantView;
// One bit per column around the camera that has real chunks.
@group(2) @binding(2) var<storage, read> coverage: array<u32>;
// Real water's uniform and wave texture (water.rs), so the sheets carry the same waves.
@group(2) @binding(14) var<uniform> water: Water;
@group(2) @binding(15) var wave_texture: texture_2d<f32>;
@group(2) @binding(16) var wave_sampler: sampler;

const FIRST_CELL: f32 = 4.0;
const RING: f32 = 6.0;
const CELLS_PER_TILE: f32 = 32.0;
const COLUMN: f32 = 32.0;
// Points this close to the edge of finer detail count as outside it, so walls standing on
// the edge are drawn whole rather than wherever rounding puts them.
const EDGE: f32 = 0.05;
// Terrain under finer detail sinks this far. The first cell inside the edge is still drawn,
// sloping down beneath the finer surface, which closes any crack between the two.
const SINK: f32 = 24.0;

const TERRAIN: u32 = 0u;
const WATER: u32 = 1u;
const ICE: u32 = 2u;

const UP = vec3(0.0, 1.0, 0.0);
// Ambient shading of X and Z facing block sides.
const X_SIDE_SHADE: f32 = FACE_SHADE[0];
const Z_SIDE_SHADE: f32 = FACE_SHADE[4];
// Mean ambient occlusion over the block next to an inner corner of the staircase.
const CORNER_AO: f32 = 0.825;
// Side area of tree crowns per unit of crown top, and of small plants per unit of plant
// cover, seen edge-on.
const CROWN_SIDES: f32 = 0.8;
const PLANT_SIDES: f32 = 0.5;
// How far crowns raise the canopy above the ground (as in lod.rs), and how tall a forest is
// for its spacing, which sets how soon crowns hide the ground between them.
const CANOPY_HEIGHT: f32 = 6.0;
const CANOPY_DEPTH: f32 = 1.0;
// Crowns as the eye resolves them: blobs about a tree across (a noise cell of this many
// blocks), covering the canopy's share of the ground. The pattern shows where a cell spans
// a few pixels and fades into its average before it could shimmer.
const CROWN_CELL: f32 = 5.0;
const CROWN_FADE = vec2(0.2, 0.5);
// How much a crown's dome tilts its surface, per unit of the noise's slope across a cell.
const CROWN_DOME: f32 = 2.4;
// Light left on the ground of a forest by the crowns around it, where they cover all of it.
const FOREST_FLOOR: f32 = 0.55;
// Patches a few blocks across, lighter and darker as real ground is with its mix of blocks,
// and how strongly; they fade where they would be a pixel or two, like the crowns.
const PATCH_CELL: f32 = 4.0;
const PATCHES: f32 = 0.3;
// The sun's glint off the waves, as real water's (water_surface.wgsl): sharpness and
// strength of its core and of its wider sheen.
const GLINT = vec2(1600.0, 160.0);
const GLINT_STRENGTH = vec2(40.0, 0.6);
// Depth of water at which sky light at the floor has run out, in blocks (it falls two
// levels per block).
const SHEET_DEPTH: f32 = 7.5;
// Falling water's foam, lit: a little blue, as real waterfalls' streaks (water_surface.wgsl).
const FALLING_FOAM = vec3(0.85, 0.9, 0.95);

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) packed: vec4<u32>,
};

// Interpolated at the centroid of the samples a triangle covers. With MSAA the fragment
// shader runs once per pixel, by default at the pixel's centre even when only a corner of
// the pixel is covered; far away, where triangles are slivers seen edge on, values
// extrapolated that far outside the triangle are wild (negative colours, a position off
// the surface, a bent normal), which showed as black dashes and sparkling glints along
// distant shores.
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(perspective, centroid) world_position: vec3<f32>,
    @location(1) @interpolate(perspective, centroid) top: vec3<f32>,
    @location(2) @interpolate(perspective, centroid) side: vec3<f32>,
    @location(3) @interpolate(perspective, centroid) normal: vec3<f32>,
    // x: sky light 0..1, y: canopy cover, or the opacity of ice, z: plant cover
    @location(4) @interpolate(perspective, centroid) params: vec3<f32>,
    @location(5) @interpolate(flat) kind: u32,
    @location(6) @interpolate(perspective, centroid) leaves: vec3<f32>,
    // Cell size of the tile's ring, in blocks.
    @location(7) @interpolate(flat) cell: f32,
};

fn decode_normal(bits: u32) -> vec3<f32> {
    let e = vec2(f32(bits & 255u), f32((bits >> 8u) & 255u)) / 255.0 * 2.0 - 1.0;
    var n = vec3(e.x, 1.0 - abs(e.x) - abs(e.y), e.y);
    if n.y < 0.0 {
        let folded = (1.0 - abs(n.zx)) * sign(n.xz);
        n = vec3(folded.x, n.y, folded.y);
    }
    return normalize(n);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

fn decode_color(bits: u32) -> vec3<f32> {
    return srgb_to_linear(vec3(f32(bits & 255u), f32((bits >> 8u) & 255u), f32((bits >> 16u) & 255u)) / 255.0);
}

fn decode_rgb565(bits: u32) -> vec3<f32> {
    return srgb_to_linear(vec3(f32(bits & 31u), f32((bits >> 5u) & 63u), f32((bits >> 11u) & 31u)) / vec3(31.0, 63.0, 31.0));
}

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let origin = origins[in.instance_index];
    var out: VertexOutput;
    out.cell = f32(origin.w);
    let cells = vec2(f32(w0 & 63u), f32((w0 >> 6u) & 63u));
    let xz = vec2<f32>(origin.xz) + cells * out.cell;
    out.world_position = vec3(xz.x, f32((w0 >> 12u) & 16383u) / 32.0, xz.y);
    out.kind = w0 >> 30u;
    if out.kind == TERRAIN && covered_by_finer(out.world_position.xz, EDGE, out.cell) {
        out.world_position.y -= SINK;
    }
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.top = decode_color(in.packed.y);
    out.side = decode_color(in.packed.w);
    out.normal = decode_normal(in.packed.z);
    out.leaves = decode_rgb565(in.packed.z >> 16u);
    out.params = vec3(f32((w0 >> 26u) & 15u), f32(in.packed.y >> 24u), f32(in.packed.w >> 24u)) / vec3(15.0, 255.0, 255.0);
    return out;
}

// Whether the column holding a position has real chunks.
fn real_column(xz: vec2<f32>) -> bool {
    let at = vec2<i32>(floor(xz / COLUMN)) - distant.coverage.xy;
    let width = distant.coverage.z;
    if any(at < vec2(0)) || any(at >= vec2(width)) {
        return false;
    }
    let i = u32(at.y * width + at.x);
    return (coverage[i >> 5u] & (1u << (i & 31u))) != 0u;
}

// Whether a position lies more than `margin` inside finer detail: real chunks, wherever they
// have been meshed (at long view distances past the finest rings too), or the next finer
// ring, derived from where the rings are laid out exactly as the client chooses its tiles.
fn covered_by_finer(xz: vec2<f32>, margin: f32, cell: f32) -> bool {
    if real_column(xz + vec2(-margin, -margin)) && real_column(xz + vec2(margin, -margin))
        && real_column(xz + vec2(-margin, margin)) && real_column(xz + vec2(margin, margin)) {
        return true;
    }
    if cell <= FIRST_CELL {
        return false;
    }
    let tile = cell * 0.5 * CELLS_PER_TILE;
    let center = floor(vec2<f32>(distant.rings.xy) / tile);
    let low = (center - RING) * tile;
    let high = (center + RING + 1.0) * tile;
    return all(xz > low + margin) && all(xz < high - margin);
}

// Light reaching a block face, as real chunks shade it.
fn face_light(in: VertexOutput, normal: vec3<f32>, ambient_shade: f32, ao: f32, foliage: bool) -> vec3<f32> {
    return shade_normal(vec3(1.0), in.world_position, normal, ambient_shade, in.params.x, 0.0, ao, foliage, true);
}

// Fraction of [y - width / 2, y + width / 2] where fract(y) lies in [a, b): a band repeating
// every block of height, box-filtered over a pixel.
fn band(y: f32, a: f32, b: f32, width: f32) -> f32 {
    let lo = y - width * 0.5;
    let hi = y + width * 0.5;
    let at_hi = floor(hi) * (b - a) + clamp(fract(hi) - a, 0.0, b - a);
    let at_lo = floor(lo) * (b - a) + clamp(fract(lo) - a, 0.0, b - a);
    return saturate((at_hi - at_lo) / width);
}

// Real terrain is a staircase of one-block steps, which the smooth tile stands in for. Each
// pixel gets the colour that staircase would show from the eye: treads lit as tops, risers as
// sides, ambient occlusion where they meet, and small plants, which stand in front of the
// steps when seen at a low angle. Near the real chunks the risers resolve into the same thin
// contour lines; further out they average into a tone that darkens with slope.
fn steps(in: VertexOutput, to_eye: vec3<f32>, y: f32, filter_width: f32) -> vec3<f32> {
    let n = normalize(in.normal);
    let slope = -n.xz / max(n.y, 0.05);
    // Areas seen from the eye per unit of ground. Risers facing the eye show; risers facing
    // away hide the inner corner of the tread below them.
    let along = slope * to_eye.xz;
    let risers = max(-along, vec2(0.0));
    let hidden = max(along.x, 0.0) + max(along.y, 0.0);
    let corners = max(max(to_eye.y, 0.0) * min(abs(slope.x) + abs(slope.y), 1.0) - hidden, 0.0);
    let treads = max(to_eye.y - corners - hidden, 0.0);
    let ground = max(risers.x + risers.y + corners + treads, 1e-4);
    let plants = PLANT_SIDES * in.params.z * length(to_eye.xz);

    // Visible sides face the eye, whichever way the slope runs.
    let x_side = vec3(sign(to_eye.x), 0.0, 0.0);
    let z_side = vec3(0.0, 0.0, sign(to_eye.z));
    let riser_ao = 1.0 - (1.0 - CORNER_AO) / max(length(slope), 1.0);
    let riser = in.side * (risers.x * face_light(in, x_side, X_SIDE_SHADE, riser_ao, false)
        + risers.y * face_light(in, z_side, Z_SIDE_SHADE, riser_ao, false)) / max(risers.x + risers.y, 1e-5);
    let tread = in.top * face_light(in, UP, 1.0, 1.0, false);
    let corner = in.top * face_light(in, UP, 1.0, CORNER_AO, false);
    // Plants are lit like real ones: as foliage facing up.
    let plant = in.top * face_light(in, UP, FACE_SHADE[6], 1.0, true);

    // Each block of height is one step: tread, its occluded inner corner, then the riser.
    let r = (risers.x + risers.y) / ground;
    let c = corners / ground;
    let in_riser = band(y, 1.0 - r, 1.0, filter_width);
    let in_corner = band(y, 1.0 - r - c, 1.0 - r, filter_width);
    let stairs = in_riser * riser + in_corner * corner + (1.0 - in_riser - in_corner) * tread;
    return mix(stairs, plant, plants / (ground + plants));
}

fn lattice(cell: vec2<i32>) -> f32 {
    var h = (u32(cell.x) * 0x8da6b343u) ^ (u32(cell.y) * 0xd8163841u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h ^= h >> 12u;
    return f32(h >> 8u) / 16777216.0;
}

// Smooth value noise 0..1 over cells of one unit, and its gradient (yz).
fn value_noise(p: vec2<f32>) -> vec3<f32> {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let dw = 6.0 * f * (1.0 - f);
    let a = lattice(i);
    let b = lattice(i + vec2(1, 0));
    let c = lattice(i + vec2(0, 1));
    let d = lattice(i + vec2(1, 1));
    let value = mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
    let gradient = vec2(mix(b - a, d - c, w.y) * dw.x, mix(c - a, d - b, w.x) * dw.y);
    return vec3(value, gradient);
}

// Where the crowns of a forest covering `cover` of the ground stand, around `xz`: x is the
// share of the pixel under a crown (soft at the crown's edge, by the pixel's footprint in
// blocks), yz how its dome tilts the surface. The level that leaves `cover` of the noise
// above it was measured from the noise's distribution.
fn crowns(xz: vec2<f32>, cover: f32, footprint: vec2<f32>) -> vec3<f32> {
    let q = value_noise(xz / CROWN_CELL);
    let d = cover - 0.5;
    let level = 0.5 - 0.64 * d - 0.7 * d * d * d;
    let soft = 0.04 + footprint.x / CROWN_CELL * length(q.yz);
    return vec3(smoothstep(level - soft, level + soft, q.x), q.yz * CROWN_DOME);
}

// Tree crowns over the steps. From above the gaps between crowns show the ground; at a low
// angle crowns stand in front of the gaps behind them, so a forest seen from afar is all
// leaves, tops and sides. Where the surface is steeper than the ground beneath it, at the
// edge of a forest, it is the wall of crowns facing out of the forest. Up to a few hundred
// blocks past the real chunks the crowns resolve into single domes, lit on the sun's side,
// over the ground between them, as the trees of the real chunks do.
fn terrain(in: VertexOutput, to_eye: vec3<f32>, y: f32, filter_width: f32, surface_slope: f32, footprint: vec2<f32>) -> vec3<f32> {
    let cover = in.params.y;
    var ground = steps(in, to_eye, y, filter_width);
    if cover == 0.0 {
        return ground;
    }
    // The ground between trees lies in their shade.
    ground *= mix(1.0, FOREST_FLOOR, cover);
    let n = normalize(in.normal);
    let slope = -n.xz / max(n.y, 0.05);
    let edge = saturate(surface_slope - length(slope));
    // The eye's line through the crowns runs as far as it takes to rise one crown layer above
    // the ground, which slopes towards or away from the eye.
    let rise = max(to_eye.y - dot(slope, to_eye.xz), 0.02);
    var gaps = (1.0 - cover) * (1.0 - edge) * exp(-CANOPY_DEPTH * cover * length(to_eye.xz) / rise);
    var top = UP;
    let resolved = 1.0 - smoothstep(CROWN_FADE.x, CROWN_FADE.y, footprint.y / CROWN_CELL);
    if resolved > 0.0 {
        let here = crowns(in.world_position.xz, cover, footprint);
        // The gaps the eye sees through, in proportion to the ground left open right here.
        gaps = mix(gaps, gaps / max(1.0 - cover, 0.01) * (1.0 - here.x), resolved);
        top = normalize(vec3(-here.y * resolved, 1.0, -here.z * resolved));
    }
    let sides = CROWN_SIDES * abs(to_eye.xz);
    let x_side = vec3(sign(to_eye.x), 0.0, 0.0);
    let z_side = vec3(0.0, 0.0, sign(to_eye.z));
    let crown = in.leaves * (max(to_eye.y, 0.0) * face_light(in, top, 1.0, 1.0, true)
        + sides.x * face_light(in, x_side, X_SIDE_SHADE, 1.0, true)
        + sides.y * face_light(in, z_side, Z_SIDE_SHADE, 1.0, true)) / max(to_eye.y + sides.x + sides.y, 1e-4);
    return mix(crown, ground, saturate(gaps));
}

// Water and ice: the surface over the floor beneath. Water is shaded as real water is
// (water_surface.wgsl), with its waves (`wind_wave_slope`, the same texture, layers and
// filtering), its reflection of the sky and the low cloud, and its sun glint, so the two
// meet without a seam at the edge of the real chunks: the floor seen through it, dimmed
// and coloured by the water it is seen and lit through, the water's own colour where it is
// deep. Where the sheet falls steeply (`surface_slope`, rise per block: a river dropping
// off a cliff) it is white water, as real falling water is, not a mirror: a level sheet's
// reflection there took the horizon's colour, and waterfalls showed as sky-blue holes in
// the hills.
fn sheet(in: VertexOutput, to_eye: vec3<f32>, surface_slope: f32, footprint: f32) -> vec3<f32> {
    let light = shade_normal(vec3(1.0), in.world_position, UP, 1.0, 1.0, 0.0, 1.0, false, true);
    let floor = saturate(in.side) * face_light(in, UP, 1.0, 1.0, false);
    if in.kind == ICE {
        return mix(floor, in.top * light, in.params.y);
    }
    // Open water: waves as high as the wind lets them grow.
    let slope = wind_wave_slope(wave_texture, wave_sampler, water, in.world_position.xz, footprint, water.budget.x) * water.origin.w;
    let n = normalize(vec3(-slope.x, 1.0, -slope.y));
    let facing = dot(n, to_eye);
    var r = reflect(-to_eye, n);
    if r.y < 0.02 {
        r = normalize(r + UP * (0.02 - r.y));
    }
    // The top colour is the biome's water colour, as real water's. Saturated, like every
    // colour here: seen edge-on, a sheet interpolates at pixel centres outside it.
    let tint = saturate(in.top);
    let absorbed = absorption(tint, water.optics.x);
    let scattered = scattering(tint, water.optics.y);
    // Sky light at the floor fell by two levels per block of water above it. Saturated,
    // because a sheet seen edge-on interpolates its vertices' values at pixel centres
    // outside it, which an exponential would blow up.
    let sky = saturate(in.params.x);
    let depth = (1.0 - sky) * SHEET_DEPTH;
    let path = select(depth / max(to_eye.y, 0.05), 1e4, sky <= 0.0);
    let seen = floor * transmittance(absorbed, scattered, path + depth) + inscattered(absorbed, scattered, path, light);
    let up = mirrored_sky(to_eye, n, UP, footprint);
    let cloud = reflected_cloud(in.world_position, up);
    let reflected = sky_gradient(up, water.zenith.rgb, horizon_along(water.horizon.rgb, up)) * (1.0 - cloud.a) + cloud.rgb;
    let toward_sun = max(dot(r, sun_direction()), 0.0);
    var glint = 0.0;
    if toward_sun > 0.95 {
        glint = (pow(toward_sun, GLINT.x) * GLINT_STRENGTH.x + pow(toward_sun, GLINT.y) * GLINT_STRENGTH.y)
            * sun_unclouded(in.world_position) * water.strengths.x;
    }
    let still = mix(seen, reflected, fresnel(facing) * water.strengths.x) + sun_color() * glint;
    let falls = smoothstep(0.7, 1.5, surface_slope);
    let white_water = mix(seen, light * FALLING_FOAM, 0.55);
    return mix(still, white_water, falls);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Height of the ground under any crowns, and how much it changes across the pixel, taken
    // before any branch like every derivative.
    let y = in.world_position.y - CANOPY_HEIGHT * in.params.y;
    let filter_width = max(length(vec2(dpdx(y), dpdy(y))), 1e-3);
    // Blocks of ground across the pixel: x along its longer side (foreshortened at a low
    // angle), y the side of a square of the same area. Detail fades by area, so a crown
    // still shows where it spans several pixels across even if only one or two deep, and
    // its edge softens by the longer side, so it blurs rather than shimmers.
    let ground_x = length(dpdx(in.world_position.xz));
    let ground_y = length(dpdy(in.world_position.xz));
    let footprint = vec2(max(ground_x, ground_y), sqrt(ground_x * ground_y));
    // Real water's measure of a pixel on the surface, for the sheets' waves.
    let water_footprint = length(fwidth(in.world_position.xz));
    // Steepness of the drawn surface, crowns included.
    // Degenerate where a triangle is thinner than a pixel; such pixels count as flat.
    let across = cross(dpdx(in.world_position), dpdy(in.world_position));
    let surface = select(UP, across * inverseSqrt(dot(across, across)), dot(across, across) > 1e-12);
    let surface_slope = length(surface.xz) / max(abs(surface.y), 0.05);
    // Sunken terrain shows only in the first cell inside finer detail; sheets not at all.
    let margin = select(EDGE, in.cell, in.kind == TERRAIN);
    if covered_by_finer(in.world_position.xz, margin, in.cell) {
        discard;
    }
    let to_eye = normalize(view.world_position - in.world_position);
    var color: vec3<f32>;
    if in.kind == TERRAIN {
        // The staircase estimate can overshoot on slivers along shores; real terrain never
        // gets brighter than a sunlit white block.
        color = clamp(terrain(in, to_eye, y, filter_width, surface_slope, footprint), vec3(0.0), vec3(2.0));
        let patches = 1.0 - smoothstep(CROWN_FADE.x, CROWN_FADE.y, footprint.y / PATCH_CELL);
        if patches > 0.0 {
            color *= 1.0 + PATCHES * patches * (value_noise(in.world_position.xz / PATCH_CELL + 17.0).x - 0.5);
        }
    } else {
        color = sheet(in, to_eye, surface_slope, water_footprint);
    }
    // A NaN from a degenerate sliver would bloom into a blob; drop it.
    color = select(color, vec3(0.0), color != color);
    return vec4(apply_fog(color, in.world_position), 1.0);
}
