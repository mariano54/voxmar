// Distant terrain (level of detail): smooth heightfield tiles generated from the world
// generator's surface functions rather than real chunks, with a sheet for water and ice.
//   word 0: x:6 | z:6 | y:14 | sky:4 | kind:2   (x, z in cells; y in 1/32 blocks)
//   word 1: r:8 | g:8 | b:8 | a:8   (sRGB albedo of top faces; a: canopy cover, or the
//                                    opacity of ice)
//   word 2: octahedral normal x:8 | z:8 | r:5 | g:6 | b:5 (sRGB albedo of tree crowns)
//   word 3: r:8 | g:8 | b:8 | a:8   (sRGB albedo of side faces, or for sheets of the floor
//                                    beneath; a: cover of small plants)

#import bevy_pbr::mesh_view_bindings::{view, fog}
#import voxmar::lighting::{FACE_SHADE, shade_normal, apply_fog, sun_direction, sun_color, water_normal}

struct DistantView {
    // xy: column of the coverage mask's first bit, z: the mask's width in columns, w: the
    // real chunks' radius in columns.
    coverage: vec4<i32>,
};

// Tile origins (x, z) and cell size (w), by instance index; shared with the real chunks.
@group(2) @binding(0) var<storage, read> origins: array<vec4<i32>>;
@group(2) @binding(1) var<uniform> distant: DistantView;
// One bit per column around the camera that has real chunks.
@group(2) @binding(2) var<storage, read> coverage: array<u32>;

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
// Real water's ripples carry on past the real chunks and smooth out over this distance, into
// their average: a wider, dimmer sun glint, and a surface facing the eye a little more (the
// variance of the ripples' slope).
const RIPPLE_FADE: f32 = 256.0;
const GLINT_SHARPNESS = vec2(50.0, 220.0);
const GLINT = vec2(0.9, 1.0);
const RIPPLE_SLOPE_VARIANCE: f32 = 0.0013;
// How fast light fades through water, per block (as in voxel.wgsl).
const WATER_EXTINCTION: f32 = 0.32;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) packed: vec4<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) top: vec3<f32>,
    @location(2) side: vec3<f32>,
    @location(3) normal: vec3<f32>,
    // x: sky light 0..1, y: canopy cover, or the opacity of ice, z: plant cover
    @location(4) params: vec3<f32>,
    @location(5) @interpolate(flat) kind: u32,
    @location(6) leaves: vec3<f32>,
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
// ring, derived from the camera position exactly as the client chooses its tiles.
fn covered_by_finer(xz: vec2<f32>, margin: f32, cell: f32) -> bool {
    if real_column(xz + vec2(-margin, -margin)) && real_column(xz + vec2(margin, -margin))
        && real_column(xz + vec2(-margin, margin)) && real_column(xz + vec2(margin, margin)) {
        return true;
    }
    if cell <= FIRST_CELL {
        return false;
    }
    let tile = cell * 0.5 * CELLS_PER_TILE;
    let center = floor(view.world_position.xz / tile);
    let low = (center - RING) * tile;
    let high = (center + RING + 1.0) * tile;
    return all(xz > low + margin) && all(xz < high - margin);
}

// Light reaching a block face, as real chunks shade it.
fn face_light(in: VertexOutput, normal: vec3<f32>, ambient_shade: f32, ao: f32, foliage: bool) -> vec3<f32> {
    return shade_normal(vec3(1.0), in.world_position, normal, ambient_shade, in.params.x, 0.0, ao, foliage, in.clip_position.xy);
}

// Fraction of [y - width / 2, y + width / 2] where fract(y) lies in [a, b): a band repeating
// every block of height, box-filtered over a pixel.
fn band(y: f32, a: f32, b: f32, width: f32) -> f32 {
    let lo = y - width * 0.5;
    let hi = y + width * 0.5;
    let at_hi = floor(hi) * (b - a) + clamp(fract(hi) - a, 0.0, b - a);
    let at_lo = floor(lo) * (b - a) + clamp(fract(lo) - a, 0.0, b - a);
    return (at_hi - at_lo) / width;
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

// Tree crowns over the steps. From above the gaps between crowns show the ground; at a low
// angle crowns stand in front of the gaps behind them, so a forest seen from afar is all
// leaves, tops and sides. Where the surface is steeper than the ground beneath it, at the
// edge of a forest, it is the wall of crowns facing out of the forest.
fn terrain(in: VertexOutput, to_eye: vec3<f32>, y: f32, filter_width: f32, surface_slope: f32) -> vec3<f32> {
    let cover = in.params.y;
    let ground = steps(in, to_eye, y, filter_width);
    if cover == 0.0 {
        return ground;
    }
    let n = normalize(in.normal);
    let slope = -n.xz / max(n.y, 0.05);
    let edge = saturate(surface_slope - length(slope));
    // The eye's line through the crowns runs as far as it takes to rise one crown layer above
    // the ground, which slopes towards or away from the eye.
    let rise = max(to_eye.y - dot(slope, to_eye.xz), 0.02);
    let gaps = (1.0 - cover) * (1.0 - edge) * exp(-CANOPY_DEPTH * cover * length(to_eye.xz) / rise);
    let sides = CROWN_SIDES * abs(to_eye.xz);
    let x_side = vec3(sign(to_eye.x), 0.0, 0.0);
    let z_side = vec3(0.0, 0.0, sign(to_eye.z));
    let crown = in.leaves * (max(to_eye.y, 0.0) * face_light(in, UP, 1.0, 1.0, true)
        + sides.x * face_light(in, x_side, X_SIDE_SHADE, 1.0, true)
        + sides.y * face_light(in, z_side, Z_SIDE_SHADE, 1.0, true)) / max(to_eye.y + sides.x + sides.y, 1e-4);
    return mix(crown, ground, gaps);
}

// Water and ice: the surface over the floor beneath, which dims with depth as sky light fades
// through it. Water reflects the sky like real water does.
fn sheet(in: VertexOutput, to_eye: vec3<f32>) -> vec3<f32> {
    let surface = in.top * shade_normal(vec3(1.0), in.world_position, UP, 1.0, 1.0, 0.0, 1.0, false, in.clip_position.xy);
    let floor = in.side * face_light(in, UP, 1.0, 1.0, false);
    if in.kind == ICE {
        return mix(floor, surface, in.params.y);
    }
    let past_real = max(abs(in.world_position.x - view.world_position.x), abs(in.world_position.z - view.world_position.z))
        - f32(distant.coverage.w) * COLUMN;
    let ripples = 1.0 - smoothstep(0.0, RIPPLE_FADE, past_real);
    let n = normalize(mix(UP, water_normal(in.world_position), ripples));
    // Ripples facing the eye fill more of the view than those facing away, the more so the
    // lower the angle, so on average real water faces the eye more than a flat sheet.
    let facing = dot(n, to_eye) + (1.0 - ripples) * RIPPLE_SLOPE_VARIANCE / max(to_eye.y, 0.01);
    let fresnel = 0.02 + 0.98 * pow(1.0 - clamp(facing, 0.0, 1.0), 5.0);
    let glint = pow(max(dot(reflect(-to_eye, n), sun_direction()), 0.0), mix(GLINT_SHARPNESS.x, GLINT_SHARPNESS.y, ripples))
        * mix(GLINT.x, GLINT.y, ripples);
    let reflected = mix(surface, fog.base_color.rgb * 1.15, fresnel) + sun_color() * glint * 4.0;
    // As over real shallows, the floor shows through as far as light reaches it; sky light
    // at the floor fell by two levels per block of water.
    let depth = (1.0 - in.params.x) * 7.5;
    let through = exp(-depth * WATER_EXTINCTION);
    return mix(floor, reflected, mix(mix(0.92, 0.18, through), 1.0, fresnel));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Height of the ground under any crowns, and how much it changes across the pixel, taken
    // before any branch like every derivative.
    let y = in.world_position.y - CANOPY_HEIGHT * in.params.y;
    let filter_width = max(length(vec2(dpdx(y), dpdy(y))), 1e-3);
    // Steepness of the drawn surface, crowns included.
    let surface = normalize(cross(dpdx(in.world_position), dpdy(in.world_position)));
    let surface_slope = length(surface.xz) / max(abs(surface.y), 0.05);
    // Sunken terrain shows only in the first cell inside finer detail; sheets not at all.
    let margin = select(EDGE, in.cell, in.kind == TERRAIN);
    if covered_by_finer(in.world_position.xz, margin, in.cell) {
        discard;
    }
    let to_eye = normalize(view.world_position - in.world_position);
    var color: vec3<f32>;
    if in.kind == TERRAIN {
        color = terrain(in, to_eye, y, filter_width, surface_slope);
    } else {
        color = sheet(in, to_eye);
    }
    return vec4(apply_fog(color, in.world_position), 1.0);
}
