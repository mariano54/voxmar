// Copies ranges of a page's chunk indices into a view's index buffer (see compact.rs):
// one workgroup per range of up to 2048 indices.

struct Copy {
    source: u32,
    destination: u32,
    count: u32,
    padding: u32,
}

// x: this dispatch's first copy.
@group(0) @binding(0) var<uniform> params: vec4<u32>;
@group(0) @binding(1) var<storage, read> copies: array<Copy>;
@group(0) @binding(2) var<storage, read> source: array<u32>;
@group(0) @binding(3) var<storage, read_write> gathered: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let copy = copies[params.x + group.x];
    for (var i = lane; i < copy.count; i += 256u) {
        gathered[copy.destination + i] = source[copy.source + i];
    }
}
