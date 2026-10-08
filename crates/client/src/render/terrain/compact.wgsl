// Writes the indices of runs of a page's quads into a view's index buffer (see
// compact.rs): one workgroup per run of up to 256 quads, a thread per quad.

struct Copy {
    first_vertex: u32,
    destination: u32,
    quads: u32,
    indices_per_quad: u32,
}

// x: this dispatch's first copy.
@group(0) @binding(0) var<uniform> params: vec4<u32>;
@group(0) @binding(1) var<storage, read> copies: array<Copy>;
// The page's vertices, two words each (see voxmar_engine::mesh).
@group(0) @binding(2) var<storage, read> vertices: array<u32>;
@group(0) @binding(3) var<storage, read_write> gathered: array<u32>;

// Whether a cube face's corners, as the mesher emits them, wind counter-clockwise seen
// from outside, by face (mesh.rs's `face_axes`). The one-sided plant faces, the quads of
// blocks' shapes (mesh.rs `emit_model`), all do.
const CCW = array<bool, 6>(false, true, false, true, true, false);
// Each quad's two triangles, by winding and by which diagonal they split along.
const ORDERS = array<array<u32, 6>, 4>(
    array<u32, 6>(0u, 2u, 1u, 0u, 3u, 2u),
    array<u32, 6>(1u, 3u, 2u, 1u, 0u, 3u),
    array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u),
    array<u32, 6>(1u, 2u, 3u, 1u, 3u, 0u),
);
// A quad drawn from both sides: both windings.
const BOTH = array<u32, 12>(0u, 1u, 2u, 0u, 2u, 3u, 0u, 2u, 1u, 0u, 3u, 2u);

fn ao(vertex: u32) -> u32 {
    return (vertices[vertex * 2u] >> 21u) & 3u;
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let copy = copies[params.x + group.x];
    for (var quad = lane; quad < copy.quads; quad += 64u) {
        let base = copy.first_vertex + quad * 4u;
        let out = copy.destination + quad * copy.indices_per_quad;
        if copy.indices_per_quad == 12u {
            for (var i = 0u; i < 12u; i++) {
                gathered[out + i] = base + BOTH[i];
            }
            continue;
        }
        // Split along the darker diagonal, so ambient occlusion interpolates evenly.
        let face = (vertices[base * 2u] >> 18u) & 7u;
        let flip = ao(base) + ao(base + 2u) > ao(base + 1u) + ao(base + 3u);
        let order = ORDERS[u32(face >= 6u || CCW[face]) * 2u + u32(flip)];
        for (var i = 0u; i < 6u; i++) {
            gathered[out + i] = base + order[i];
        }
    }
}
