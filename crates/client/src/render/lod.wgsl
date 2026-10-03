// Distant terrain (level of detail). Each tile is a blocky heightfield of coarse cells,
// generated from the world generator's surface functions rather than real chunks.
//   word 0: x:6 | z:6 | y:9 | face:3 | water:1    (x, z in cells; y in blocks)
//   word 1: r:8 | g:8 | b:8

#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::{view, globals, fog}
#import voxmar::lighting::{NORMALS, shade, apply_fog, sun_direction, sun_color}

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
    @location(0) packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) @interpolate(flat) water: u32,
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let w1 = in.packed.y;
    let local = vec3<f32>(f32(w0 & 63u), f32((w0 >> 12u) & 511u), f32((w0 >> 6u) & 63u));
    var out: VertexOutput;
    out.world_position = (get_world_from_local(in.instance_index) * vec4(local, 1.0)).xyz;
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.color = vec3(f32(w1 & 255u), f32((w1 >> 8u) & 255u), f32((w1 >> 16u) & 255u)) / 255.0;
    out.face = (w0 >> 21u) & 7u;
    out.water = (w0 >> 24u) & 1u;
    return out;
}

// Whether a position is covered by finer detail (real chunks or the next finer level),
// derived from the camera position exactly as the client chooses its tiles.
fn covered_by_finer(xz: vec2<f32>) -> bool {
    let camera = view.world_position.xz;
    var low: vec2<f32>;
    var high: vec2<f32>;
    if params.rings.y == 0.0 {
        let column = floor(camera / COLUMN);
        low = (column - (params.rings.w - 1.0)) * COLUMN;
        high = (column + params.rings.w) * COLUMN;
    } else {
        let tile = params.rings.y * CELLS_PER_TILE;
        let center = floor(camera / tile);
        low = (center - params.rings.z) * tile;
        high = (center + params.rings.z + 1.0) * tile;
    }
    return all(xz >= low) && all(xz < high);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if covered_by_finer(in.world_position.xz) {
        discard;
    }
    var color = shade(in.color, in.world_position, in.face, 1.0, 0.0, 1.0, false, in.clip_position.xy);
    if in.water != 0u {
        let to_eye = normalize(view.world_position - in.world_position);
        // Flat water reflects more than rippled water would; scale down to match the
        // real chunks it borders.
        let fresnel = (0.02 + 0.98 * pow(1.0 - max(to_eye.y, 0.0), 5.0)) * 0.5;
        let glint = pow(max(dot(reflect(-to_eye, vec3(0.0, 1.0, 0.0)), sun_direction()), 0.0), 120.0);
        color = mix(color, fog.base_color.rgb * 1.15, fresnel) + sun_color() * glint * 2.0;
    }
    return vec4(apply_fog(color, in.world_position), 1.0);
}
