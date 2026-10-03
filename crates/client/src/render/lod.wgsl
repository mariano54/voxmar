// Distant terrain (level of detail): smooth heightfield tiles generated from the world
// generator's surface functions rather than real chunks.
//   word 0: x:6 | z:6 | y:10    (x, z in cells; y in half blocks)
//   word 1: r:8 | g:8 | b:8 | water:8   (sRGB colour)
//   word 2: octahedral normal x:8 | z:8

#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::{view, fog}
#import voxmar::lighting::{shade_normal, apply_fog, sun_direction, sun_color}

struct LodParams {
    // x: cell size of this level in blocks, y: cell size of the next finer level (0 for
    // the finest), z: tiles from a level's centre tile to its edge, w: real-chunk radius
    // in columns.
    rings: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: LodParams;

const CELLS_PER_TILE: f32 = 32.0;
const COLUMN: f32 = 32.0;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) packed: vec3<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) water: f32,
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

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let w1 = in.packed.y;
    let local = vec3<f32>(f32(w0 & 63u), f32((w0 >> 12u) & 1023u) * 0.5, f32((w0 >> 6u) & 63u));
    var out: VertexOutput;
    out.world_position = (get_world_from_local(in.instance_index) * vec4(local, 1.0)).xyz;
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.color = srgb_to_linear(vec3(f32(w1 & 255u), f32((w1 >> 8u) & 255u), f32((w1 >> 16u) & 255u)) / 255.0);
    out.water = f32(w1 >> 24u) / 255.0;
    out.normal = decode_normal(in.packed.z);
    return out;
}

// Whether a position is covered by finer detail (real chunks, which at long view distances
// reach past the finest rings, or the next finer level), derived from the camera position
// exactly as the client chooses its tiles.
fn covered_by_finer(xz: vec2<f32>) -> bool {
    let camera = view.world_position.xz;
    let column = floor(camera / COLUMN);
    let real_low = (column - (params.rings.w - 1.0)) * COLUMN;
    let real_high = (column + params.rings.w) * COLUMN;
    if all(xz >= real_low) && all(xz < real_high) {
        return true;
    }
    if params.rings.y == 0.0 {
        return false;
    }
    let tile = params.rings.y * CELLS_PER_TILE;
    let center = floor(camera / tile);
    let low = (center - params.rings.z) * tile;
    let high = (center + params.rings.z + 1.0) * tile;
    return all(xz >= low) && all(xz < high);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if covered_by_finer(in.world_position.xz) {
        discard;
    }
    let normal = normalize(in.normal);
    let ambient_shade = 0.62 + 0.38 * max(normal.y, 0.0);
    var color = shade_normal(in.color, in.world_position, normal, ambient_shade, 1.0, 0.0, 1.0, false, in.clip_position.xy);
    if in.water > 0.01 {
        // Same reflection as real water, with a flat surface.
        let to_eye = normalize(view.world_position - in.world_position);
        let fresnel = 0.02 + 0.98 * pow(1.0 - max(to_eye.y, 0.0), 5.0);
        let glint = pow(max(dot(reflect(-to_eye, vec3(0.0, 1.0, 0.0)), sun_direction()), 0.0), 220.0);
        let water = mix(color, fog.base_color.rgb * 1.15, fresnel) + sun_color() * glint * 4.0;
        color = mix(color, water, in.water);
    }
    return vec4(apply_fog(color, in.world_position), 1.0);
}
