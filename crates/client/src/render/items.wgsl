// Dropped items (see items.rs): one instance per item, 36 vertices of a box. Blocks are
// cubes with their six faces; sprites are a slab a sixteenth thick, marched through texel
// by texel so the texture's own outline and the walls along its edges show, as
// Minecraft's extruded item models do.

#import bevy_pbr::mesh_view_bindings::{view, globals}
#import voxmar::lighting::{shade_normal, apply_fog}

struct Item {
    // centre before bobbing, and seconds into its turning and bobbing at time 0; for the
    // item in hand, where it is in the view (right, up, forward of the eye)
    center: vec3<f32>,
    phase: f32,
    // where this copy of a stack sits, in the item's turning frame; for the item in hand,
    // its rotation vector (axis times angle)
    offset_x: f32,
    offset_y: f32,
    offset_z: f32,
    size: f32,
    // shape:4 | tinted faces:6 | held:1 | free:13 | block light:4 | sky light:4
    flags: u32,
    // two faces' texture layers each, the lower face in the low half
    layers: array<u32, 3>,
    // sRGB tint
    tint: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(2) @binding(0) var block_textures: texture_2d_array<f32>;
@group(2) @binding(1) var block_sampler: sampler;
@group(2) @binding(2) var<storage, read> items: array<Item>;
// Each texture layer's overlay (render/mod.rs, `layer_table`; voxel.wgsl draws it alike).
@group(2) @binding(7) var block_layers: texture_2d<f32>;

const SHAPE_SPRITE: u32 = 1u;
// The item in the player's hand (see `Held` in items.rs).
const FLAG_HELD: u32 = 1u << 10u;
// An item held still in the world, turned by the rotation vector in its offset (see
// `ItemModel::posed` in items.rs): one in another player's hand.
const FLAG_POSED: u32 = 1u << 11u;
// Where its depth is held: just inside the near plane (reverse depth, 1 at the near plane),
// so no wall or block the eye stands against can hide or cut it.
const HELD_DEPTH: f32 = 0.9;
// Turn per second, in radians (Minecraft's one radian a second), and the bob: a tenth of
// a block up and down at two radians a second.
const SPIN: f32 = 1.0;
const BOB: f32 = 0.1;
const BOB_RATE: f32 = 2.0;
// Half the thickness of a sprite's slab, in widths of the item.
const HALF_THICK: f32 = 1.0 / 32.0;

const NORMALS = array<vec3<f32>, 6>(
    vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0),
);
// The two corners of a face's square, as (a, b) in 0..1 across it.
const CORNERS = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    // Position in the item's own box, each axis 0..1.
    @location(1) local: vec3<f32>,
    @location(2) @interpolate(flat) instance: u32,
    @location(3) @interpolate(flat) face: u32,
};

// Where an item is and how it is turned, now: model space to the world.
struct Pose {
    center: vec3<f32>,
    rotation: mat3x3<f32>,
};

fn pose_of(item: Item) -> Pose {
    var pose: Pose;
    if (item.flags & FLAG_HELD) != 0u {
        // The camera's own axes: the hand is where the eye is and moves as it turns. The
        // view looks down -z, so forward is minus.
        let view_axes = mat3x3<f32>(view.world_from_view[0].xyz, view.world_from_view[1].xyz, view.world_from_view[2].xyz);
        pose.center = view.world_position + view_axes * vec3(item.center.x, item.center.y, -item.center.z);
        pose.rotation = view_axes * rotation_of(vec3(item.offset_x, item.offset_y, item.offset_z));
        return pose;
    }
    if (item.flags & FLAG_POSED) != 0u {
        pose.center = item.center;
        pose.rotation = rotation_of(vec3(item.offset_x, item.offset_y, item.offset_z));
        return pose;
    }
    let age = globals.time + item.phase;
    let angle = age * SPIN;
    pose.rotation = turned(angle);
    pose.center = item.center + vec3(0.0, sin(age * BOB_RATE) * BOB, 0.0)
        + turn(vec3(item.offset_x, item.offset_y, item.offset_z), angle);
    return pose;
}

// The turn about the vertical axis by `angle`, as a matrix (what `turn` does to a vector).
fn turned(angle: f32) -> mat3x3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return mat3x3<f32>(vec3(c, 0.0, -s), vec3(0.0, 1.0, 0.0), vec3(s, 0.0, c));
}

// The rotation a rotation vector (axis times angle) stands for, as a matrix (Rodrigues').
fn rotation_of(v: vec3<f32>) -> mat3x3<f32> {
    let angle = length(v);
    if angle < 1e-5 {
        return mat3x3<f32>(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0));
    }
    let k = v / angle;
    let s = sin(angle);
    let c = cos(angle);
    let t = 1.0 - c;
    return mat3x3<f32>(
        vec3(c + t * k.x * k.x, s * k.z + t * k.x * k.y, -s * k.y + t * k.x * k.z),
        vec3(-s * k.z + t * k.x * k.y, c + t * k.y * k.y, s * k.x + t * k.y * k.z),
        vec3(s * k.y + t * k.x * k.z, -s * k.x + t * k.y * k.z, c + t * k.z * k.z),
    );
}

fn turn(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

// The item's model space: a box centred on the origin, an item wide; a sprite's is thin
// along z.
fn half_extent(item: Item) -> vec3<f32> {
    if (item.flags & 15u) == SHAPE_SPRITE {
        return vec3(0.5, 0.5, HALF_THICK);
    }
    return vec3(0.5);
}

fn layer_of(instance: u32, face: u32) -> i32 {
    let pair = items[instance].layers[face / 2u];
    return i32(select(pair & 0xFFFFu, pair >> 16u, (face & 1u) == 1u));
}

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let item = items[instance_index];
    let pose = pose_of(item);
    let face = vertex_index / 6u;
    let normal = NORMALS[face];
    let world_normal = pose.rotation * normal;
    // A face turned away from the eye collapses to nothing.
    let reach = half_extent(item);
    let toward = view.world_position - (pose.center + world_normal * reach[face / 2u] * item.size);
    if dot(world_normal, toward) <= 0.0 {
        out.clip_position = vec4(0.0, 0.0, 0.0, 1.0);
        return out;
    }
    let corner = CORNERS[vertex_index % 6u];
    // The face's square in the unit box: the axis along the normal fixed at its side.
    let axis = face / 2u;
    var local = vec3(0.0);
    local[axis] = select(0.0, 1.0, (face & 1u) == 0u);
    local[(axis + 1u) % 3u] = corner.x;
    local[(axis + 2u) % 3u] = corner.y;
    let model = (local * 2.0 - 1.0) * half_extent(item);
    out.world_position = pose.center + (pose.rotation * model) * item.size;
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    if (item.flags & FLAG_HELD) != 0u {
        out.clip_position.z = out.clip_position.w * HELD_DEPTH;
    }
    out.local = local;
    out.instance = instance_index;
    out.face = face;
    return out;
}

// Texture coordinates of a cube face, as the terrain lays them (voxel.wgsl's `face_uv`).
fn face_uv(local: vec3<f32>, face: u32) -> vec2<f32> {
    switch face {
        case 0u: { return vec2(1.0 - local.z, 1.0 - local.y); }
        case 1u: { return vec2(local.z, 1.0 - local.y); }
        case 2u: { return vec2(local.x, local.z); }
        case 3u: { return vec2(local.x, 1.0 - local.z); }
        case 4u: { return vec2(local.x, 1.0 - local.y); }
        default: { return vec2(1.0 - local.x, 1.0 - local.y); }
    }
}

// Minecraft-style directional shading for ambient light, for any normal: tops brightest,
// then faces along z, faces along x, and bottoms darkest.
fn ambient_shade(n: vec3<f32>) -> f32 {
    return n.x * n.x * 0.62 + n.z * n.z * 0.8 + n.y * n.y * select(0.5, 1.0, n.y > 0.0);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

struct Hit {
    texel: vec2<i32>,
    // Model-space normal of the wall the ray met.
    normal: vec3<f32>,
};

// Marches the view ray through a sprite's slab from where it enters at `start` (in the
// unit box), cell by cell across the texture, and returns the first opaque texel inside
// the slab's thickness and the side it was met on; none if the ray leaves first.
fn march(start: vec3<f32>, direction: vec3<f32>, entry_normal: vec3<f32>, layer: i32, n: i32) -> Hit {
    var hit: Hit;
    hit.texel = vec2(-1);
    let size = f32(n);
    // Texels across x and y, the slab's full thickness along z.
    let p = vec3(start.xy * size, start.z);
    let d = vec3(direction.xy * size, direction.z);
    var cell = clamp(vec2<i32>(floor(p.xy)), vec2(0), vec2(n - 1));
    let step = vec2<i32>(sign(d.xy));
    let inv = 1.0 / max(abs(d.xy), vec2(1e-6));
    let next = vec2<f32>(cell) + select(vec2(0.0), vec2(1.0), d.xy > vec2(0.0));
    var t_next = abs(next - p.xy) * inv;
    // When the ray is within the slab's thickness (z between 0 and 1).
    var z_in = 0.0;
    var z_out = 1e9;
    if abs(d.z) > 1e-6 {
        let a = (0.0 - p.z) / d.z;
        let b = (1.0 - p.z) / d.z;
        z_in = min(a, b);
        z_out = max(a, b);
    }
    var t = 0.0;
    var normal = entry_normal;
    for (var i = 0; i < 2 * n + 2; i++) {
        let t_exit = min(t_next.x, t_next.y);
        if max(t, z_in) <= min(t_exit, z_out) + 1e-5 {
            let texel = vec2(cell.x, n - 1 - cell.y);
            if textureLoad(block_textures, texel, layer, 0).a >= 0.5 {
                hit.texel = texel;
                // Entered this cell through the slab's face rather than a texel's wall.
                if i > 0 && z_in > t {
                    normal = vec3(0.0, 0.0, -sign(d.z));
                }
                hit.normal = normal;
                return hit;
            }
        }
        if t_exit > z_out {
            return hit;
        }
        // Into the next cell, through a wall across x or y.
        if t_next.x < t_next.y {
            cell.x += step.x;
            t = t_next.x;
            t_next.x += inv.x;
            normal = vec3(-f32(step.x), 0.0, 0.0);
        } else {
            cell.y += step.y;
            t = t_next.y;
            t_next.y += inv.y;
            normal = vec3(0.0, -f32(step.y), 0.0);
        }
        if any(cell < vec2(0)) || any(cell >= vec2(n)) {
            return hit;
        }
    }
    return hit;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let item = items[in.instance];
    let pose = pose_of(item);
    let sprite = (item.flags & 15u) == SHAPE_SPRITE;
    let layer = layer_of(in.instance, select(in.face, 0u, sprite));
    // Sampled before any branch, where derivatives are defined.
    let uv = face_uv(in.local, in.face);
    let sampled = textureSample(block_textures, block_sampler, uv, layer);
    let uv_dx = dpdx(uv);
    let uv_dy = dpdy(uv);

    var texel = sampled;
    var normal = NORMALS[in.face];
    var tinted = ((item.flags >> 4u) & (1u << in.face)) != 0u;
    var overlay_uv = uv;
    if sprite {
        // The view ray in the unit box.
        let eye = (transpose(pose.rotation) * (view.world_position - pose.center)) / item.size;
        let here = (in.local * 2.0 - 1.0) * half_extent(item);
        let direction = normalize(here - eye) / (2.0 * half_extent(item));
        let n = i32(textureDimensions(block_textures).x);
        let hit = march(in.local, direction, normal, layer, n);
        if hit.texel.x < 0 {
            discard;
        }
        texel = textureLoad(block_textures, hit.texel, layer, 0);
        normal = hit.normal;
        tinted = (item.flags & (1u << 4u)) != 0u;
        overlay_uv = (vec2<f32>(hit.texel) + 0.5) / f32(n);
    } else if sampled.a < 0.5 {
        discard;
    }

    let color = srgb_to_linear(unpack4x8unorm(item.tint).rgb);
    let tint = select(vec3(1.0), color, tinted);
    var albedo = texel.rgb * tint;
    let entry = textureLoad(block_layers, vec2(layer, 0), 0);
    let overlay = u32(entry.r * 255.0 + 0.5) | (u32(entry.g * 255.0 + 0.5) << 8u);
    if overlay != 0u {
        // The colour is the overlay's: a grass block's side keeps its dirt.
        let over = textureSampleGrad(block_textures, block_sampler, overlay_uv, i32(overlay - 1u), uv_dx, uv_dy);
        albedo = mix(texel.rgb, over.rgb * color, over.a);
    }
    let world_normal = pose.rotation * normal;
    let sky = f32(item.flags >> 28u) / 15.0;
    let block = f32((item.flags >> 24u) & 15u) / 15.0;
    // Sprites (plants) are lit as thin and translucent, as the plants they came from are.
    // Hard-edged shadows: an item is too small to show a penumbra.
    let lit = shade_normal(albedo, in.world_position, world_normal, ambient_shade(world_normal), sky, block, 1.0, sprite, false);
    return vec4(apply_fog(lit, in.world_position), 1.0);
}
