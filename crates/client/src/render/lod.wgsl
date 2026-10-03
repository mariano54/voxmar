// Distant terrain (level of detail): smooth heightfield tiles generated from the world
// generator's surface functions rather than real chunks, with a sheet for water and ice.
//   word 0: x:6 | z:6 | y:14 | sky:4 | kind:2   (x, z in cells; y in 1/32 blocks)
//   word 1: r:8 | g:8 | b:8 | a:8   (sRGB albedo of top faces; a: canopy cover, or the
//                                    opacity of ice)
//   word 2: octahedral normal x:8 | z:8
//   word 3: r:8 | g:8 | b:8 | a:8   (sRGB albedo of side faces, or for sheets of the floor
//                                    beneath; a: cover of small plants)

#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::{view, fog}
#import voxmar::lighting::{FACE_SHADE, shade_normal, apply_fog, sun_direction, sun_color}

struct LodParams {
    // x: cell size of this level in blocks, y: cell size of the next finer level (0 for
    // the finest), z: tiles from a level's centre tile to its edge, w: real-chunk radius
    // in columns.
    rings: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: LodParams;

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
// Side area of tree crowns per unit of canopy, and of small plants per unit of plant cover,
// seen edge-on.
const CROWN_SIDES: f32 = 0.8;
const PLANT_SIDES: f32 = 0.5;
// Real water's sun glint, averaged over its ripples into a wider, dimmer highlight.
const GLINT_SHARPNESS: f32 = 50.0;
const GLINT: f32 = 0.9;

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

fn decode_color(bits: u32) -> vec3<f32> {
    let c = vec3(f32(bits & 255u), f32((bits >> 8u) & 255u), f32((bits >> 16u) & 255u)) / 255.0;
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let local = vec3<f32>(f32(w0 & 63u), f32((w0 >> 12u) & 16383u) / 32.0, f32((w0 >> 6u) & 63u));
    var out: VertexOutput;
    out.world_position = (get_world_from_local(in.instance_index) * vec4(local, 1.0)).xyz;
    out.kind = w0 >> 30u;
    if out.kind == TERRAIN && covered_by_finer(out.world_position.xz, EDGE) {
        out.world_position.y -= SINK;
    }
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.top = decode_color(in.packed.y);
    out.side = decode_color(in.packed.w);
    out.normal = decode_normal(in.packed.z);
    out.params = vec3(f32((w0 >> 26u) & 15u), f32(in.packed.y >> 24u), f32(in.packed.w >> 24u)) / vec3(15.0, 255.0, 255.0);
    return out;
}

// Whether a position lies more than `margin` inside finer detail (real chunks, which at long
// view distances reach past the finest rings, or the next finer level), derived from the
// camera position exactly as the client chooses its tiles.
fn covered_by_finer(xz: vec2<f32>, margin: f32) -> bool {
    let camera = view.world_position.xz;
    let column = floor(camera / COLUMN);
    let real_low = (column - (params.rings.w - 1.0)) * COLUMN;
    let real_high = (column + params.rings.w) * COLUMN;
    if all(xz > real_low + margin) && all(xz < real_high - margin) {
        return true;
    }
    if params.rings.y == 0.0 {
        return false;
    }
    let tile = params.rings.y * CELLS_PER_TILE;
    let center = floor(camera / tile);
    let low = (center - params.rings.z) * tile;
    let high = (center + params.rings.z + 1.0) * tile;
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
// sides, ambient occlusion where they meet, and the sides of tree crowns and small plants,
// which stand in front of the steps when seen at a low angle. Near the real chunks the
// risers resolve into the same thin contour lines; further out they average into a tone
// that darkens with slope.
fn staircase(in: VertexOutput, to_eye: vec3<f32>, filter_width: f32) -> vec3<f32> {
    let n = normalize(in.normal);
    let cover = in.params.y;
    let slope = -n.xz / max(n.y, 0.05);
    // Areas seen from the eye per unit of ground. Risers facing the eye show; risers facing
    // away hide the inner corner of the tread below them.
    let along = slope * to_eye.xz;
    let risers = max(-along, vec2(0.0));
    let hidden = max(along.x, 0.0) + max(along.y, 0.0);
    let corners = max(to_eye.y, 0.0) * min(abs(slope.x) + abs(slope.y), 1.0);
    let treads = max(to_eye.y - max(corners, hidden), 0.0);
    let crowns = CROWN_SIDES * cover * abs(to_eye.xz);
    let plants = PLANT_SIDES * in.params.z * length(to_eye.xz);
    let ground = risers.x + risers.y + max(corners - hidden, 0.0) + treads;
    let total = max(ground + crowns.x + crowns.y + plants, 1e-4);

    // Visible sides face the eye, whichever way the slope runs.
    let x_side = vec3(sign(to_eye.x), 0.0, 0.0);
    let z_side = vec3(0.0, 0.0, sign(to_eye.z));
    let riser_ao = 1.0 - (1.0 - CORNER_AO) / max(length(slope), 1.0);
    let riser = in.side * (risers.x * face_light(in, x_side, X_SIDE_SHADE, riser_ao, false)
        + risers.y * face_light(in, z_side, Z_SIDE_SHADE, riser_ao, false)) / max(risers.x + risers.y, 1e-5);
    var tread = in.top * face_light(in, UP, 1.0, 1.0, false);
    var corner = in.top * face_light(in, UP, 1.0, CORNER_AO, false);
    var crown = vec3(0.0);
    if cover > 0.0 {
        // Leaves are lit as foliage.
        let leaf_top = in.top * face_light(in, UP, 1.0, 1.0, true);
        tread = mix(tread, leaf_top, cover);
        corner = mix(corner, leaf_top * CORNER_AO, cover);
        crown = in.side * (crowns.x * face_light(in, x_side, X_SIDE_SHADE, 1.0, true)
            + crowns.y * face_light(in, z_side, Z_SIDE_SHADE, 1.0, true)) / max(crowns.x + crowns.y, 1e-5);
    }

    // Each block of height is one step: tread, its occluded inner corner, then the riser.
    // Crowns hide the steps beneath them, so forests blur the pattern.
    let r = (risers.x + risers.y) / max(ground, 1e-4);
    let c = max(corners - hidden, 0.0) / max(ground, 1e-4);
    let width = max(filter_width, cover);
    let y = in.world_position.y;
    let in_riser = band(y, 1.0 - r, 1.0, width);
    let in_corner = band(y, 1.0 - r - c, 1.0 - r, width);
    let steps = in_riser * riser + in_corner * corner + (1.0 - in_riser - in_corner) * tread;
    // Plants are lit like real ones: as foliage facing up.
    let plant = in.top * face_light(in, UP, FACE_SHADE[6], 1.0, true);
    return (steps * ground + crown * (crowns.x + crowns.y) + plant * plants) / total;
}

// Water and ice: the surface over the floor beneath, which dims with depth as sky light fades
// through it. Water reflects the sky like real water does.
fn sheet(in: VertexOutput, to_eye: vec3<f32>) -> vec3<f32> {
    let surface = in.top * shade_normal(vec3(1.0), in.world_position, UP, 1.0, 1.0, 0.0, 1.0, false, in.clip_position.xy);
    let floor = in.side * face_light(in, UP, 1.0, 1.0, false);
    if in.kind == ICE {
        return mix(floor, surface, in.params.y);
    }
    let fresnel = 0.02 + 0.98 * pow(1.0 - max(to_eye.y, 0.0), 5.0);
    let glint = pow(max(dot(reflect(-to_eye, UP), sun_direction()), 0.0), GLINT_SHARPNESS) * GLINT;
    let reflected = mix(surface, fog.base_color.rgb * 1.15, fresnel) + sun_color() * glint * 4.0;
    return mix(floor, reflected, mix(0.72, 1.0, fresnel));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Pixel footprint in blocks of height, taken before any branch.
    let y = in.world_position.y;
    let filter_width = max(length(vec2(dpdx(y), dpdy(y))), 1e-3);
    // Sunken terrain shows only in the first cell inside finer detail; sheets not at all.
    let margin = select(EDGE, params.rings.x, in.kind == TERRAIN);
    if covered_by_finer(in.world_position.xz, margin) {
        discard;
    }
    let to_eye = normalize(view.world_position - in.world_position);
    var color: vec3<f32>;
    if in.kind == TERRAIN {
        color = staircase(in, to_eye, filter_width);
    } else {
        color = sheet(in, to_eye);
    }
    return vec4(apply_fog(color, in.world_position), 1.0);
}
