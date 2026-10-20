// Voxel terrain. Vertices are two packed u32 words (see voxmar_engine::mesh):
//   word 0: x:6 | y:6 | z:6 | face:3 | ao:2 | flags:2
//   word 1: layer:12 | tint:2 | temperature:5 | humidity:5 | block:4 | sky:4
// Faces 0-5 are cube faces; 6 and 7 are a plant's two diagonal quads. The flags mark a
// translucent face covered by a block (water under ice) or a plant's top edge (FLAG_INNER),
// and a leaf face with leaves behind it, which casts a solid shadow (FLAG_BACKED).
// The same file provides the main pass and the depth prepass / shadow pass entry points.
// Shader defs pick the pass: none for opaque terrain (no alpha test, so the GPU keeps early
// depth and hidden surface removal), CUTOUT for leaves, glass and plants, plus INNER for the
// double-sided faces between two leaves, TRANSLUCENT for water and ice.

// Chunks are drawn from shared pages (see terrain.rs), several chunks to a draw: vertices
// are handed out to chunks in blocks of 64, and a page's table says which chunk (an index
// into the origin table) owns each block.

#import bevy_render::globals::Globals
#import voxmar::wind::{sway, parting, leaf_flutter, motion_near}

#ifdef PREPASS_PIPELINE
#import bevy_render::view::View
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> globals: Globals;
#else
#import bevy_pbr::mesh_view_bindings::{view, globals, fog}
#import voxmar::lighting::{NORMALS, shade, apply_fog, sun_direction, sun_color, sun_shadow, weathered, wet_sheen, self_glow, light_curve}
#ifdef TRANSLUCENT
#import voxmar::water_surface::shade_water
#endif
#endif

@group(2) @binding(0) var block_textures: texture_2d_array<f32>;
@group(2) @binding(1) var block_sampler: sampler;
@group(2) @binding(2) var<storage, read> chunk_origins: array<vec4<i32>>;
// Bindings 3 and 4 are grass blade seeds (grass.wgsl) and the wind (wind.wgsl), 6 the
// atmosphere (lighting.wgsl).
// Biome colours of every column: grass, foliage and water for each block (sRGB, a word
// each), from the offset a chunk's origin keeps in w.
@group(2) @binding(5) var<storage, read> column_colours: array<u32>;
// Every shape quad (terrain.rs `shape_table`), for blocks drawn as their shapes: each
// corner's position in its cell and u, the corners' v, and the normal with the cube face
// the quad is lit as.
struct ShapeQuad {
    corners: array<vec4<f32>, 4>,
    v: vec4<f32>,
    normal: vec4<f32>,
};
@group(2) @binding(17) var<storage, read> shape_quads: array<ShapeQuad>;
#ifndef PREPASS_PIPELINE
// Each texture layer's overlay and glow (render/mod.rs, `layer_table`): r and g the layer
// of an overlay drawn over it, tinted (a grass block side's fringe), plus one (0 for
// none), b the overlay's tint, a how brightly the layer glows by itself (its low seven
// bits) and whether it glows by the block light through it (`THROUGH`), a lit window.
@group(2) @binding(7) var block_layers: texture_2d<f32>;
// The tint byte's bits for a layer whose texture is a strip of frames, and for one its
// pack gives other drawings of.
const ANIMATED: u32 = 32u;
// The glow byte's bit for a layer that glows by the block light passing through it.
const THROUGH: u32 = 128u;
// How brightly a window glows, at full block light behind it.
const THROUGH_GLOW: f32 = 0.7;
const VARIED: u32 = 16u;
// How bright a glowing block's brightest channel gets: the display transform rolls off
// gently up to here, and turns colours white past it.
const GLOW_PEAK: f32 = 1.15;
#endif
@group(3) @binding(0) var<storage, read> vertex_chunks: array<u32>;

const TINT_NONE: u32 = 0u;
const TINT_FOLIAGE: u32 = 2u;
const TINT_WATER: u32 = 3u;
const FACE_PLANT: u32 = 6u;
const FLAG_INNER: u32 = 1u << 23u;
const FLAG_BACKED: u32 = 1u << 24u;
const PLANT_STACK_SHIFT: u32 = 25u;
// Water faces carry the fluid's state (see voxmar_engine::mesh): how far the surface lies
// below the block's top in eighths, the flow's direction in 16ths of a turn and its speed
// (in the AO bits), and whether the cell is falling (flag 2).
const SURFACE_DROP_SHIFT: u32 = 25u;
const FLOW_DIRECTION_SHIFT: u32 = 28u;
const FLOW_SPEEDS = array<f32, 4>(0.0, 0.7, 1.4, 2.8);
const OPEN_WATER_SHIFT: u32 = 14u;
const FLAG_FALLING: u32 = 1u << 24u;
const FLAG_FRINGE: u32 = 1u << 27u;
const FLAG_NEEDLES: u32 = 1u << 28u;
const FLAG_FLAT: u32 = 1u << 24u;
// On a leaf card: the face of the crown it is shaded as.
const CARD_FACE_SHIFT: u32 = 29u;
// Leaf cards shrink back into their block between these distances, in blocks (the renderer
// stops drawing them a little beyond, terrain.rs `CARD_DISTANCE`).
const CARD_FADE = vec2(40.0, 60.0);
const AO_CURVE = array<f32, 4>(0.45, 0.65, 0.84, 1.0);
// Plants sink away between these distances, in blocks; terrain.rs stops drawing them
// beyond (PLANT_DISTANCE).
const PLANT_FADE = vec2(192.0, 320.0);

struct Vertex {
    @builtin(vertex_index) index: u32,
    @location(0) packed: vec2<u32>,
};

fn local_position(w0: u32) -> vec3<f32> {
    return vec3<f32>(f32(w0 & 63u), f32((w0 >> 6u) & 63u), f32((w0 >> 12u) & 63u));
}

fn tint_kind(surface: u32) -> u32 {
    return (surface >> 12u) & 3u;
}

fn face_of(w0: u32) -> u32 {
    return (w0 >> 18u) & 7u;
}

fn pcg3d(seed: vec3<u32>) -> vec3<f32> {
    var v = seed * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v >> vec3(8u)) / 16777216.0;
}

// Smooth noise over the ground, for patches of taller and shorter plants.
fn tufts(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let at = vec4(
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x), 7, i32(cell.y)))).x,
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x) + 1, 7, i32(cell.y)))).x,
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x), 7, i32(cell.y) + 1))).x,
        pcg3d(vec3<u32>(vec3<i32>(i32(cell.x) + 1, 7, i32(cell.y) + 1))).x,
    );
    return mix(mix(at.x, at.y, w.x), mix(at.z, at.w, w.x), w.y);
}

// Plant models (voxmar_engine::block::PlantModel) and where their bits lie (mesh.rs).
const MODEL_CROSS: u32 = 0u;
const MODEL_HASH: u32 = 1u;
const MODEL_CAP: u32 = 2u;
const MODEL_BUSH: u32 = 3u;
const MODEL_ROCK: u32 = 4u;
const MODEL_WALL: u32 = 5u;
// A quad of a block's shape (mesh.rs `emit_model`): placed from the shape table by the
// quad's number in word 1 (bits 12..23), lit and shaded as the cube face nearest its normal.
const MODEL_SHAPE: u32 = 6u;
// A plant on a wall (`MODEL_WALL`, a torch): how high its foot sits on the wall, how far
// out from the wall, and how far it leans out for every block it rises (22°, as a wall
// torch leans).
const WALL_FOOT: vec2<f32> = vec2(0.2, 0.08);
const WALL_LEAN: f32 = 0.4;
const PLANT_MODEL_SHIFT: u32 = 28u;
const PLANT_MODEL_HIGH_SHIFT: u32 = 24u;
const PLANT_PIECE_SHIFT: u32 = 30u;
const PLANT_HEIGHT_SHIFT: u32 = 14u;
// Set on a plant hanging from the block above (mesh.rs, `PLANT_HANGS_SHIFT`).
const PLANT_HANGS: u32 = 1u << 27u;
// A mushroom (`MODEL_CAP`, eight quads): a stem of two narrow quads crossing square with
// the cell, and a cap with volume: a flat crown, four sloping sides down to a wider rim and
// gills beneath it. Sizes are in blocks across and shares of the plant's height up.
const CAP_RIM: f32 = 0.56;
const CAP_CROWN: f32 = 0.74;
const CAP_RIM_HALF: f32 = 0.38;
const CAP_CROWN_HALF: f32 = 0.2;
const STEM_HALF: f32 = 0.075;

fn plant_model(w0: u32) -> u32 {
    return ((w0 >> PLANT_MODEL_SHIFT) & 3u) | ((w0 >> PLANT_MODEL_HIGH_SHIFT) & 1u) << 2u;
}

// Which quad of its model a plant vertex belongs to: the even ones are packed along one
// diagonal, the odd along the other.
fn plant_piece(w0: u32) -> u32 {
    return ((w0 >> PLANT_PIECE_SHIFT) & 3u) * 2u + (face_of(w0) - FACE_PLANT);
}

// A mushroom vertex: x and z across the cell from its centre, y up the plant, and its texture
// coordinates (the stem's own texture, the cap's top texture, or the gills).
struct CapVertex {
    at: vec3<f32>,
    uv: vec2<f32>,
}

fn cap_vertex(w0: u32) -> CapVertex {
    let corner = (w0 >> 21u) & 3u;
    let piece = plant_piece(w0);
    var square = array<vec2<f32>, 4>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    var out: CapVertex;
    if piece < 2u {
        // The stem: corners 0 and 1 at its foot, 2 and 3 under the cap.
        let far = select(-1.0, 1.0, corner == 1u || corner == 2u);
        let up = select(0.0, CAP_RIM, corner >= 2u);
        let across = select(vec2(far * STEM_HALF, 0.0), vec2(0.0, far * STEM_HALF), piece == 1u);
        out.at = vec3(across.x, up, across.y);
        out.uv = vec2(far * 0.5 + 0.5, 1.0 - up);
        return out;
    }
    if piece == 2u {
        let p = square[corner] * CAP_CROWN_HALF;
        out.at = vec3(p.x, CAP_CROWN, p.y);
        out.uv = 0.5 + p * (0.5 / CAP_RIM_HALF);
        return out;
    }
    if piece == 7u {
        let p = square[corner] * CAP_RIM_HALF * 0.96;
        out.at = vec3(p.x, CAP_RIM - 0.01, p.y);
        out.uv = 0.5 + square[corner] * 0.5;
        return out;
    }
    // A side, from the crown's edge down to the rim: pieces 3..6 face -z, +x, +z, -x.
    let side = piece - 3u;
    var ends = array<vec2<f32>, 4>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let first = ends[side];
    let second = ends[(side + 1u) % 4u];
    let along = select(first, second, corner == 1u || corner == 2u);
    let lower = corner < 2u;
    let p = along * select(CAP_CROWN_HALF, CAP_RIM_HALF, lower);
    out.at = vec3(p.x, select(CAP_CROWN, CAP_RIM, lower), p.y);
    out.uv = 0.5 + p * (0.5 / CAP_RIM_HALF);
    return out;
}

// A small rock (`MODEL_ROCK`, eight quads), as GregTech's surface rocks over a vein (the idea
// alone, see INSPIRATIONS.md): a lump with a flat top and four sloping sides
// whose feet sink a little into the ground, and a chip of three faces rising to a point
// beside it. Sizes in blocks; `a` and `b` (hashes of its cell) give each its own
// proportions, the tilt of its top and where its chip lies. Every face shows the whole
// texture, so a sample of ore shows its ore from every side. It lies still: no wind moves
// it. Its sides are shaded darker than its top, which gives it its volume (`shade`).
const ROCK_TOP: f32 = 0.16;
const ROCK_FOOT: f32 = 0.3;
const ROCK_HEIGHT: f32 = 0.27;
const ROCK_SUNK: f32 = 0.06;
const ROCK_SIDE_SHADE = array<f32, 4>(0.72, 0.84, 0.66, 0.78);

struct RockVertex {
    at: vec3<f32>,
    uv: vec2<f32>,
    brightness: f32,
}

fn rock_vertex(w0: u32, a: vec3<f32>, b: vec3<f32>) -> RockVertex {
    let corner = (w0 >> 21u) & 3u;
    let piece = plant_piece(w0);
    let far = corner == 1u || corner == 2u;
    let lower = corner < 2u;
    var square = array<vec2<f32>, 4>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let stretch = vec2(mix(0.8, 1.25, a.x), mix(0.75, 1.05, b.z));
    let height = ROCK_HEIGHT * mix(0.75, 1.3, a.z);
    let tilt = (vec2(a.y, b.x) - 0.5) * 0.12;
    var out: RockVertex;
    if piece == 0u {
        let p = square[corner];
        out.at = vec3(p.x * ROCK_TOP * stretch.x, height + dot(p, tilt), p.y * ROCK_TOP * stretch.y);
        out.uv = 0.5 + p * 0.5;
        out.brightness = 1.0;
        return out;
    }
    if piece <= 4u {
        let side = piece - 1u;
        let along = select(square[side], square[(side + 1u) % 4u], far);
        let p = along * select(ROCK_TOP, ROCK_FOOT, lower) * stretch;
        out.at = vec3(p.x, select(height + dot(along, tilt), -ROCK_SUNK, lower), p.y);
        out.uv = vec2(select(0.0, 1.0, far), select(0.0, 1.0, lower));
        out.brightness = ROCK_SIDE_SHADE[side];
        return out;
    }
    // The chip, of three faces, each from two feet up to its point.
    let k = f32(piece - 5u);
    let towards = vec2(cos(b.y * 6.2831853), sin(b.y * 6.2831853));
    let centre = towards * (ROCK_FOOT * max(stretch.x, stretch.y) + 0.04);
    let turn = a.x * 6.2831853 + select(k, k + 1.0, far) * 2.0943951;
    let foot = centre + vec2(cos(turn), sin(turn)) * 0.11;
    let point = vec3(centre.x, 0.14 * mix(0.8, 1.3, b.z), centre.y);
    out.at = select(point, vec3(foot.x, -ROCK_SUNK, foot.y), lower);
    out.uv = select(vec2(0.5, 0.25), vec2(select(0.25, 0.75, far), 0.75), lower);
    out.brightness = 0.86 - 0.08 * k;
    return out;
}

// How brightly a plant's quad is shaded: a rock's sides darker than its top, any other
// plant evenly.
fn plant_shade(w0: u32) -> f32 {
    if plant_model(w0) == MODEL_ROCK {
        return rock_vertex(w0, vec3(0.5), vec3(0.5)).brightness;
    }
    return 1.0;
}

// A bush or flower (`MODEL_BUSH`, four quads) splays from one root like a bouquet: each card
// faces out from the middle, its foot close to the centre and its top leaning out, so no
// card runs through the middle to be seen flat on, and from any side the cards stand at
// different depths. Distances in blocks from the plant's centre.
const BUSH_FOOT: f32 = 0.05;
const BUSH_SPREAD: f32 = 0.3;
const BUSH_WIDTH: f32 = 1.0;

// Where a model's quad's vertex lies across its cell: a # of four straight quads (crops), a
// cap with volume (mushrooms), four splayed cards (bushes and flowers), else the two
// crossed diagonal quads.
fn model_across(w0: u32) -> vec2<f32> {
    let corner = (w0 >> 21u) & 3u;
    let far = select(0.0, 1.0, corner == 1u || corner == 2u);
    let piece = plant_piece(w0);
    switch plant_model(w0) {
        case MODEL_HASH: {
            let side = select(0.3, 0.7, (piece & 1u) == 1u);
            return select(vec2(side, far), vec2(far, side), piece < 2u);
        }
        case MODEL_CAP: {
            return 0.5 + cap_vertex(w0).at.xz;
        }
        case MODEL_BUSH: {
            let angle = 0.7853982 + f32(piece) * 1.5707963;
            let outward = vec2(cos(angle), sin(angle));
            let along = vec2(-outward.y, outward.x);
            let reach = select(BUSH_FOOT, BUSH_SPREAD, corner >= 2u);
            return 0.5 + outward * reach + along * (far - 0.5) * BUSH_WIDTH;
        }
        default: {}
    }
    return plant_across(w0);
}

// A shape quad's number in the table: its low 12 bits where a plant has its tint, its high 2
// where a plant has which quad of its model it is (mesh.rs `shape_quad_index`).
fn shape_index(w0: u32, w1: u32) -> u32 {
    return ((w1 >> 12u) & 0xFFFu) | (((w0 >> PLANT_PIECE_SHIFT) & 3u) << 12u);
}

// A shape quad's corner: where it lies in its cell (xyz) and its u (w).
fn shape_corner(w0: u32, w1: u32) -> vec4<f32> {
    return shape_quads[shape_index(w0, w1)].corners[(w0 >> 21u) & 3u];
}

// The cube face a vertex is lit as: its own, a shape quad's nearest, or a plant's.
fn lit_face(w0: u32, w1: u32) -> u32 {
    if face_of(w0) >= FACE_PLANT && plant_model(w0) == MODEL_SHAPE {
        return u32(shape_quads[shape_index(w0, w1)].normal.w);
    }
    return face_of(w0);
}

// The texture coordinates of a vertex: a cube face's from where it is, a shape quad's from
// its table, a plant's along its quad.
fn vertex_uv(w0: u32, w1: u32) -> vec2<f32> {
    if face_of(w0) < FACE_PLANT {
        return face_uv(local_position(w0), face_of(w0));
    }
    if plant_model(w0) == MODEL_SHAPE {
        let quad = shape_quads[shape_index(w0, w1)];
        let corner = (w0 >> 21u) & 3u;
        return vec2(quad.corners[corner].w, quad.v[corner]);
    }
    return plant_uv(w0);
}

// The surface word fragments read: the layer, tint and the rest of word 1's low bits; a
// shape quad keeps only its layer (its quad number lies where a plant's tint is).
fn surface_of(w0: u32, w1: u32) -> u32 {
    if face_of(w0) >= FACE_PLANT && plant_model(w0) == MODEL_SHAPE {
        return w1 & 0xFFFu;
    }
    return w1 & 0xFFFFFFu;
}

// A plant vertex's texture coordinates: along the quad and up it, or across a flat cap.
fn plant_uv(w0: u32) -> vec2<f32> {
    if plant_model(w0) == MODEL_CAP {
        return cap_vertex(w0).uv;
    }
    if plant_model(w0) == MODEL_ROCK {
        return rock_vertex(w0, vec3(0.5), vec3(0.5)).uv;
    }
    return face_uv(local_position(w0), face_of(w0));
}

// Where a plant quad's vertex lies across its cell, from its corner: (0, 0) to (1, 1) along
// one diagonal, (1, 0) to (0, 1) along the other.
fn plant_across(w0: u32) -> vec2<f32> {
    let corner = (w0 >> 21u) & 3u;
    let far = corner == 1u || corner == 2u;
    if face_of(w0) == FACE_PLANT {
        return select(vec2(0.0), vec2(1.0), far);
    }
    return select(vec2(1.0, 0.0), vec2(0.0, 1.0), far);
}

// A plant's vertex, with the plant placed as a whole from a hash of its cell, much as
// Rainbow's Foliage varies its models: turned up to 40° either way, shifted and sunk a
// little, and taller or shorter, in tufts across a meadow. It bends from its base with the
// wind and away from the player.
fn plant_position(w0: u32, w1: u32, origin: vec3<f32>) -> vec3<f32> {
    let model = plant_model(w0);
    if model == MODEL_SHAPE {
        return origin + local_position(w0) + shape_corner(w0, w1).xyz;
    }
    let packed = plant_across(w0);
    // A plant on a wall keeps which way it faces where others keep their stack.
    let stack = select(f32((w0 >> PLANT_STACK_SHIFT) & 3u), 0.0, model == MODEL_WALL);
    var up = f32((w0 >> 23u) & 1u) + stack;
    // The corner of the cell the whole plant stands in.
    let base = origin + local_position(w0) - vec3(packed.x, up, packed.y);
    if model == MODEL_WALL {
        // Straight with the blocks and still: its foot against the wall behind it, leaning
        // out the way it faces (north, east, south, west: -z, +x, +z, -x).
        let facing = (w0 >> PLANT_STACK_SHIFT) & 3u;
        let outs = array(vec2(0.0, -1.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(-1.0, 0.0));
        let out = outs[facing];
        let rise = up * f32((w1 >> PLANT_HEIGHT_SHIFT) & 31u) / 8.0;
        let foot = vec2(0.5) - out * (0.5 - WALL_FOOT.y);
        let at = foot + (packed - 0.5) + out * rise * WALL_LEAN;
        return vec3(base.x + at.x, base.y + WALL_FOOT.x + rise, base.z + at.y);
    }
    var across = model_across(w0);
    let cell = vec3<i32>(floor(base + 0.5));
    let a = pcg3d(vec3<u32>(cell));
    let b = pcg3d(vec3<u32>(cell + vec3(0, 991, 0)));
    var tilt = vec2(0.0);
    if model == MODEL_ROCK {
        // Turned any way, a little off the middle of its cell, and still.
        let rock = rock_vertex(w0, a, b);
        let size = f32((w1 >> PLANT_HEIGHT_SHIFT) & 31u) / 8.0 * mix(0.85, 1.15, b.x);
        let angle = a.x * 6.2831853;
        let o = rock.at.xz * size;
        let turned = vec2(o.x * cos(angle) - o.y * sin(angle), o.x * sin(angle) + o.y * cos(angle));
        let shift = (a.yz - 0.5) * 0.3;
        return vec3(base.x + 0.5 + turned.x + shift.x, base.y + rock.at.y * size, base.z + 0.5 + turned.y + shift.y);
    }
    if model == MODEL_CAP {
        // Each mushroom its own size, as wide as it is tall, and leaning a little its own way.
        let cap = cap_vertex(w0);
        let size = mix(0.75, 1.15, b.x);
        across = 0.5 + cap.at.xz * size;
        up = stack + cap.at.y;
        tilt = (a.yz - 0.5) * 0.35;
    }
    // Crops stand straight with the blocks, in rows.
    let angle = select((a.x - 0.5) * 1.4, 0.0, model == MODEL_HASH);
    let offset = across - 0.5;
    let turned = vec2(offset.x * cos(angle) - offset.y * sin(angle), offset.x * sin(angle) + offset.y * cos(angle));
    let shift = (a.yz - 0.5) * 0.3;
    let own_height = f32((w1 >> PLANT_HEIGHT_SHIFT) & 31u) / 8.0;
    if (w0 & PLANT_HANGS) != 0u {
        // Hanging from the block above (a bell, herbs, roots, a stalactite): its top at the
        // top of the cell, as long as its data says, and still, so it stays on its beam.
        return vec3(base.x + 0.5 + turned.x + shift.x, base.y + 1.0 - (1.0 - up) * own_height, base.z + 0.5 + turned.y + shift.y);
    }
    let height = mix(0.6, 1.25, saturate(0.7 * tufts(base.xz * 0.12) + 0.3 * b.x)) * own_height;
    let sink = b.y * 0.25;
    // Bent along a curve from the base, and lower for it; the whole plant leans alike, so
    // a tall plant's two blocks stay joined.
    let centre = base + vec3(0.5, 0.0, 0.5);
    // Plants sway slower than grass blades, each a little out of step, and tall ones are
    // stiffer: above its first block a plant bends only with the root of its height.
    let near = motion_near(centre);
    var lean = vec2(0.0);
    if near > 0.0 {
        lean = (sway(centre.xz, 1.25, b.z * 2.0).lean + parting(centre)) * 0.4 * near;
    }
    let rise = up * height;
    let bend = lean * select(sqrt(rise), rise * rise, rise < 1.0) + tilt * rise;
    let lowered = rise / sqrt(1.0 + dot(lean, lean) * up * up);
    return vec3(centre.x + turned.x + shift.x + bend.x, base.y + lowered - sink, centre.z + turned.y + shift.y + bend.y);
}

// A leaf card's vertex. Leaf blocks on the outside of a crown carry two crossed upright
// cards (bushy leaves, after Better Leaves Lite, MIT; see INSPIRATIONS.md), each two blocks
// wide and 1.75 tall, so the leaf texture runs on past the block at its own 16 texels a
// block and `card_mask` cuts it to a ragged round cluster; leaves open to the sky also
// carry a flat card, tilted a little, so a crown seen from above is as bushy as from the
// side. The pair is turned in steps of 22.5° and set a little off the block's centre, so
// neighbouring blocks' cards never line up into streaks. A conifer's needles grow smaller
// cards that droop a little, so a spruce's tiers keep their shape.
struct Card {
    world: vec3<f32>,
    uv: vec2<f32>,
    // Varies the outline of the cluster from card to card.
    seed: u32,
}

const CARD_SIZE = vec2(2.0, 1.75);
const NEEDLE_SIZE = vec2(1.6, 1.35);

fn card_vertex(w0: u32, origin: vec3<f32>) -> Card {
    let across = plant_across(w0);
    let needles = (w0 & FLAG_NEEDLES) != 0u;
    let top = f32((w0 >> 23u) & 1u);
    let corner = origin + local_position(w0) - vec3(across.x, top, across.y);
    let cell = vec3<i32>(floor(corner + 0.5));
    let a = pcg3d(vec3<u32>(cell));
    let b = pcg3d(vec3<u32>(cell + vec3(0, 577, 0)));
    var size = select(CARD_SIZE, NEEDLE_SIZE, needles);
    // Far off the cards shrink back into their block.
    size = mix(size, vec2(1.0), smoothstep(CARD_FADE.x, CARD_FADE.y, distance(corner + 0.5, view.world_position)));
    // The second card crosses the first at 90° ± 22.5°.
    var angle = floor(a.x * 8.0) * 0.3927;
    if face_of(w0) != FACE_PLANT {
        angle += 1.5708 + select(-0.3927, 0.3927, a.y < 0.5);
    }
    let along = vec2(cos(angle), sin(angle));
    let offset = (across.x - 0.5) * size.x;
    var card: Card;
    card.seed = u32(face_of(w0) * 2u) + select(0u, 1u, (w0 & FLAG_FLAT) != 0u) + u32(a.z * 4096.0) * 8u;
    if (w0 & FLAG_FLAT) != 0u {
        // Flat and square, tilted up to 15° about its own axis.
        let tilt = (b.x - 0.5) * 0.52;
        let side = vec3(-along.y, 0.0, along.x) * cos(tilt) + vec3(0.0, sin(tilt), 0.0);
        size.y = size.x;
        card.world = corner + 0.5 + vec3(along.x * offset, 0.2 + 0.3 * b.y, along.y * offset) + side * (top - 0.5) * size.y;
    } else {
        var up = (top - 0.5) * size.y;
        if needles {
            // Sprigs hang a little: the top edge lower, so the card slopes down.
            up -= top * 0.2 * size.y;
        }
        let centre = corner + 0.5 + vec3(a.z - 0.5, b.y - 0.5, b.z - 0.5) * vec3(0.25, 0.15, 0.25);
        card.world = centre + vec3(along.x * offset, up, along.y * offset);
    }
    card.uv = vec2(0.5 + (across.x - 0.5) * size.x, 0.5 - (top - 0.5) * size.y);
    return card;
}

// Whether a leaf card is solid at `uv`: inside a round cluster the size of the card, its
// edge ragged in lobes a few texels across and single texels, as a pixel artist would cut
// it (the shapes, not the images, of Better Leaves Lite's masks), and different on every
// card. Once a texel is smaller than a pixel the edge is smooth, so it does not sparkle.
fn card_mask(uv: vec2<f32>, seed: u32, footprint: f32) -> bool {
    let texel = vec2<i32>(floor(uv * 16.0));
    let lobe = pcg3d(vec3<u32>(vec3(texel / 3, i32(seed)))).x;
    let grain = pcg3d(vec3<u32>(vec3(texel, i32(seed) + 77))).x;
    let rough = select(0.14 * lobe + 0.1 * grain, 0.12, footprint > 1.0);
    let at = select((vec2<f32>(texel) + 0.5) / 16.0, uv, footprint > 1.0) - 0.5;
    return length(at / (CARD_SIZE * 0.5)) < 1.0 - rough;
}

// World position, with leaves fluttering and plants bending in the wind. Shared by every
// pass so depth matches.
fn chunk_of(in: Vertex) -> vec4<i32> {
    return chunk_origins[vertex_chunks[in.index >> 6u]];
}

fn world_position(in: Vertex) -> vec3<f32> {
    let origin = vec3<f32>(chunk_of(in).xyz);
    if face_of(in.packed.x) >= FACE_PLANT {
        return plant_position(in.packed.x, in.packed.y, origin);
    }
    var world = origin + local_position(in.packed.x);
    let tint = tint_kind(in.packed.y);
    if tint == TINT_FOLIAGE {
        let near = motion_near(world);
        if near > 0.0 {
            world += leaf_flutter(world) * near;
        }
        return world;
    }
    if tint == TINT_WATER {
        // Flowing water slopes down from its source.
        world.y -= f32((in.packed.x >> SURFACE_DROP_SHIFT) & 7u) / 8.0;
    }
    return world;
}

// Texture coordinates from the chunk-local position, so merged quads tile per block.
// Plant quads take u from their position along the diagonal.
fn face_uv(local: vec3<f32>, face: u32) -> vec2<f32> {
    switch face {
        case 0u: { return vec2(-local.z, -local.y); }
        case 1u: { return vec2(local.z, -local.y); }
        case 2u: { return vec2(local.x, local.z); }
        case 3u: { return vec2(local.x, -local.z); }
        case 4u: { return vec2(local.x, -local.y); }
        case 5u: { return vec2(-local.x, -local.y); }
        case 6u: { return vec2(local.x, -local.y); }
        default: { return vec2(local.z, -local.y); }
    }
}

// Cut-out texel test. Shadow cascades test at the mip level whose texels are half again
// as large as the map's own, so a leaf's holes are never finer than the map can hold and
// the dappled shade under a crown stays put as the cascades move with the camera, instead
// of sparkling as each move samples the leaf texture at new points. (The mips keep each
// texture's coverage, so crowns cast as much shade far away as near.)
fn alpha_test(uv: vec2<f32>, surface: u32, solid: u32) {
    if solid != 0u {
        return;
    }
    let layer = i32(surface & 0xFFFu);
    var alpha: f32;
    if view.clip_from_view[3][3] == 1.0 {
        // Orthographic: a shadow cascade. Blocks per shadow texel, and texels per block.
        let texel = 2.0 / (view.clip_from_view[0][0] * view.viewport.z);
        let size = f32(textureDimensions(block_textures).x);
        alpha = textureSampleLevel(block_textures, block_sampler, uv, layer, max(log2(texel * size * 1.5), 0.0)).a;
    } else {
        alpha = textureSample(block_textures, block_sampler, uv, layer).a;
    }
    if alpha < 0.5 {
        discard;
    }
}

#ifdef PREPASS_PIPELINE

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) surface: u32,
    // Skips the alpha test: a leaf face with leaves behind it shades solidly.
    @location(2) @interpolate(flat) solid: u32,
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(3) unclipped_depth: f32,
#endif
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    out.solid = select(0u, 1u, face_of(in.packed.x) < FACE_PLANT && (in.packed.x & FLAG_BACKED) != 0u);
    out.position = view.clip_from_world * vec4(world_position(in), 1.0);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
    out.uv = vertex_uv(in.packed.x, in.packed.y);
    out.surface = surface_of(in.packed.x, in.packed.y);
    return out;
}

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
@fragment
fn fragment(in: VertexOutput) -> @builtin(frag_depth) f32 {
    alpha_test(in.uv, in.surface, in.solid);
    return in.unclipped_depth;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    alpha_test(in.uv, in.surface, in.solid);
}
#endif

#else // main pass

// Lighting inputs are interpolated at the centroid of the samples a triangle covers (see
// lod.wgsl): with MSAA, values taken at the centre of a pixel the triangle only clips are
// extrapolated, which on far faces seen edge on runs wild (light past full, a negative
// water depth, a point off the surface) and showed as black dashes and glints along the
// edge of distant water. Texture coordinates stay at the pixel centre, where their
// derivatives, which pick the mip level, are exact.
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(perspective, centroid) world_position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // x: ao, y: sky light, z: block light (all 0..1, interpolated for smooth lighting)
    @location(2) @interpolate(perspective, centroid) light: vec3<f32>,
    @location(3) @interpolate(flat) surface: u32,
    @location(4) @interpolate(flat) face: u32,
    @location(5) @interpolate(flat) covered: u32,
    // Where the chunk's column colours start in `column_colours`.
    @location(6) @interpolate(flat) colours: u32,
#ifdef FRINGE
    @location(10) @interpolate(flat) card: u32,
#endif
#ifdef TRANSLUCENT
    // Water's flow, blocks per second along x and z, whether it is falling, and how much
    // open water is around (0..1).
    @location(7) flow: vec2<f32>,
    @location(8) @interpolate(flat) falling: u32,
    @location(9) open_water: f32,
#endif
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    let w0 = in.packed.x;
    let w1 = in.packed.y;
    var out: VertexOutput;
#ifdef FRINGE
    // The fringe pass draws only leaf cards.
    let card = card_vertex(w0, vec3<f32>(chunk_of(in).xyz));
    out.world_position = card.world + leaf_flutter(card.world) * motion_near(card.world);
    out.card = card.seed;
#else
    out.world_position = world_position(in);
#endif
    if face_of(w0) >= FACE_PLANT && (w0 & FLAG_INNER) != 0u {
        // Plants sink into the ground with distance, where they would only be a shimmer of
        // sub-pixel blades. They cast no shadow that far out (the farthest sun cascade
        // leaves them out, terrain.rs `last_of_many`).
        let distance = length(out.world_position.xz - view.world_position.xz);
        out.world_position.y -= smoothstep(PLANT_FADE.x, PLANT_FADE.y, distance);
    }
    out.clip_position = view.clip_from_world * vec4(out.world_position, 1.0);
    out.face = lit_face(w0, w1);
    out.uv = vertex_uv(w0, w1);
#ifdef FRINGE
    out.uv = card.uv;
    // Lit as the face of the crown it grows from, so a card seen edge on matches it.
    out.face = w0 >> CARD_FACE_SHIFT;
#endif
    // Plants have no ambient occlusion; their AO bits hold the corner, and water's its flow.
    let water = tint_kind(w1) == TINT_WATER;
    let ao = select(select(AO_CURVE[(w0 >> 21u) & 3u], 1.0, water), plant_shade(w0), face_of(w0) >= FACE_PLANT);
    out.light = vec3(ao, f32(w1 >> 28u) / 15.0, f32((w1 >> 24u) & 15u) / 15.0);
    out.surface = surface_of(w0, w1);
    out.covered = select(0u, 1u, face_of(w0) < FACE_PLANT && (w0 & FLAG_INNER) != 0u);
    out.colours = u32(chunk_of(in).w);
#ifdef TRANSLUCENT
    if water {
        let turn = f32(w0 >> FLOW_DIRECTION_SHIFT) / 16.0 * 6.2831853;
        out.flow = vec2(cos(turn), sin(turn)) * FLOW_SPEEDS[(w0 >> 21u) & 3u];
        out.open_water = f32((w1 >> OPEN_WATER_SHIFT) & 15u) / 15.0;
        out.falling = select(0u, 1u, (w0 & FLAG_FALLING) != 0u);
    }
#endif
    return out;
}

// A block's own number, from where it is, for picking how its texture varies.
fn block_hash(cell: vec3<i32>) -> u32 {
    var h = (u32(cell.x) * 73856093u) ^ (u32(cell.y) * 19349663u) ^ (u32(cell.z) * 83492791u);
    h ^= h >> 13u;
    h *= 0x5bd1e995u;
    return h ^ (h >> 15u);
}

// How the texture of the block at `cell` is laid on its `face`: as `uv` runs over the block,
// mirrored left to right (`mirror`) and its top or bottom turned a quarter turn at a time
// (`turn`), each picked by the block's position. The texture coordinates within the block
// go through the same map as their changes per pixel, so mip levels stay right across the
// block's edges.
struct Laid {
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
};

fn laid(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, face: u32, cell: vec3<i32>, turn: bool, mirror: bool) -> Laid {
    let h = block_hash(cell);
    var m = mat2x2(1.0, 0.0, 0.0, 1.0);
    if mirror && (h & 4u) != 0u {
        m = mat2x2(-1.0, 0.0, 0.0, 1.0);
    }
    if turn && (face == 2u || face == 3u) {
        let quarter = mat2x2(0.0, 1.0, -1.0, 0.0);
        for (var k = 0u; k < (h & 3u); k++) {
            m = quarter * m;
        }
    }
    let base = floor(uv);
    return Laid(base + 0.5 + m * (uv - base - 0.5), m * dx, m * dy);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), c > vec3(0.04045));
}

// The biome colour a surface is tinted with, from the colours of the block column its face
// belongs to: half a block back from a cube face, where the plant stands for a plant.
fn biome_tint(surface: u32, colours: u32, world: vec3<f32>, face: u32) -> vec3<f32> {
    return biome_colour(tint_kind(surface), colours, world, face);
}

// The biome colour of tint `kind` (white for none) for the face at `world`.
fn biome_colour(kind: u32, colours: u32, world: vec3<f32>, face: u32) -> vec3<f32> {
    if kind == TINT_NONE {
        return vec3(1.0);
    }
    var inside = world;
    if face < FACE_PLANT {
        inside -= NORMALS[face] * 0.5;
    }
    let cell = vec2<u32>(vec2<i32>(floor(inside.xz)) & vec2(31));
    let word = column_colours[colours + (cell.y * 32u + cell.x) * 3u + kind - 1u];
    let colour = srgb_to_linear(unpack4x8unorm(word).rgb);
    if kind == TINT_FOLIAGE {
        // Leaves take the place's foliage colour a little lighter, yellower and less
        // saturated, as sunlit leaves read in the shader-pack screenshots we aim for: the
        // climates' foliage colours are saturated enough that, times a leaf texture, a
        // canopy read as one dark, pure green.
        let grey = dot(colour, vec3(0.2126, 0.7152, 0.0722));
        return mix(colour, grey * vec3(1.25, 1.2, 0.75), 0.35) * 1.15;
    }
    return colour;
}

#ifndef PREPASS_PIPELINE
// The layer showing now for an animated layer: its frames, from the layer table's second
// row (render/mod.rs, `layer_table`), each for its time, the layer itself first.
fn animation_frame(layer: i32) -> i32 {
    let animation = textureLoad(block_layers, vec2(layer, 1), 0);
    let first = u32(animation.r * 255.0 + 0.5) | (u32(animation.g * 255.0 + 0.5) << 8u);
    let frames = max(u32(animation.b * 255.0 + 0.5), 1u);
    let ticks = max(animation.a * 255.0, 1.0);
    let frame = u32(floor(globals.time * 20.0 / ticks)) % frames;
    return select(i32(first + frame) - 1, layer, frame == 0u);
}

// The drawing of a layer with others (render/mod.rs, `layer_table`) the block at `cell`
// shows: the layer itself or one of the others, by the block's position.
fn variant_layer(layer: i32, cell: vec3<i32>) -> i32 {
    let variants = textureLoad(block_layers, vec2(layer, 1), 0);
    let first = u32(variants.r * 255.0 + 0.5) | (u32(variants.g * 255.0 + 0.5) << 8u);
    let count = max(u32(variants.b * 255.0 + 0.5), 1u);
    // Bits the turn and mirror (`laid`) do not use, so the drawing does not follow them.
    let pick = (block_hash(cell) >> 8u) % count;
    return select(i32(first + pick) - 1, layer, pick == 0u);
}
#endif

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
#ifdef TRANSLUCENT
    // Blocks a pixel spans on the surface, for the water's waves, and the surface's own
    // normal, which tilts where flowing water slopes; derivatives are only defined here,
    // before any branch.
    let footprint = length(fwidth(in.world_position.xz));
    let tilt = cross(dpdx(in.world_position), dpdy(in.world_position));
#endif
    var layer = i32(in.surface & 0xFFFu);
    // The layer's overlay, tint, variety and glow (render/mod.rs, `layer_table`).
    let entry = textureLoad(block_layers, vec2(layer, 0), 0);
    let tint_byte = u32(entry.b * 255.0 + 0.5);
    if (tint_byte & ANIMATED) != 0u {
        layer = animation_frame(layer);
    }
    var look = Laid(in.uv, dpdx(in.uv), dpdy(in.uv));
    if tint_byte >= VARIED {
        let cell = vec3<i32>(floor(in.world_position - NORMALS[in.face] * 0.5));
        if (tint_byte & VARIED) != 0u {
            layer = variant_layer(layer, cell);
        }
        if tint_byte >= 64u {
            look = laid(in.uv, look.dx, look.dy, in.face, cell, (tint_byte & 64u) != 0u, (tint_byte & 128u) != 0u);
        }
    }
    var albedo = textureSampleGrad(block_textures, block_sampler, look.uv, layer, look.dx, look.dy);
    let kind = tint_kind(in.surface);
#ifdef FRINGE
    if !card_mask(in.uv, in.card, length(fwidth(in.uv)) * 16.0) {
        discard;
    }
#endif
#ifdef ALPHA_TO_COVERAGE
    // Sharpened so a texel edge stays crisp up close; once the texture is minified the
    // alpha spreads over a few samples instead of flickering between pass and fail.
    albedo.a = saturate((albedo.a - 0.5) / max(fwidth(albedo.a), 0.0001) + 0.5);
    if albedo.a <= 0.0 {
        discard;
    }
#else
#ifdef CUTOUT
    if albedo.a < 0.5 {
        discard;
    }
#endif
#ifndef TRANSLUCENT
    albedo.a = 1.0;
#endif
#endif
    var base = albedo.rgb * biome_tint(in.surface, in.colours, in.world_position, in.face);
#ifndef TRANSLUCENT
    // A tinted overlay, as the fringe of grass on a grass block's side has, takes the
    // biome colour the top has.
    var overlay = u32(entry.r * 255.0 + 0.5) | (u32(entry.g * 255.0 + 0.5) << 8u);
#ifdef FRINGE
    // A card shows its block's overlay (fruit among the leaves) only over the block itself,
    // not repeated across the cluster around it.
    if any(in.uv < vec2(0.0)) || any(in.uv > vec2(1.0)) {
        overlay = 0u;
    }
#endif
    if overlay != 0u {
        let over = textureSampleGrad(block_textures, block_sampler, look.uv, i32(overlay - 1u), look.dx, look.dy);
        let tint = biome_colour(tint_byte & 15u, in.colours, in.world_position, in.face);
        base = mix(base, over.rgb * tint, over.a);
    }
#endif
    let plant = in.face >= FACE_PLANT;
    // Plants look up their shadow just above their own cell: trees and terrain still shade
    // them, but the blades of a dense tuft do not darken each other.
    var shadow_at = in.world_position;
    if plant {
        shadow_at.y = floor(in.world_position.y - 0.001) + 1.1;
    }
    var face = in.face;
#ifdef INNER
    // Seen from its back, the face belongs to the leaf on the other side.
    face = select(face ^ 1u, face, front_facing);
#endif
    // Rain darkens and wets what it falls on, puddles gather on open ground, frost whitens.
    var surface = vec4(base, 0.0);
#ifndef TRANSLUCENT
    if !plant {
        surface = weathered(base, in.world_position, NORMALS[face], in.light.y);
    }
#endif
    // Plants and leaves take hard-edged shadows: a plant takes one shadow for its whole cell
    // (`shadow_at`), and the holes and blades of leaves and plants break any edge up, so they
    // need no soft filter, which cost the plants of a savanna half their shading (0.24 ms at
    // 2268×1473) and a dense forest a fifth of its terrain's pixels (0.4 ms).
    var color = shade(surface.rgb, shadow_at, face, in.light.y, in.light.z, in.light.x, kind == TINT_FOLIAGE || plant, !(plant || kind == TINT_FOLIAGE));
#ifndef TRANSLUCENT
    color = wet_sheen(color, in.world_position, NORMALS[face], surface.a, in.light.y);
#endif
    // A window glows by the light of the room behind it (the light reaching its face has
    // come through it): warm panes in a lit home, standing out at night as a lantern does.
    let glow_byte = u32(entry.a * 255.0 + 0.5);
    var glow = f32(glow_byte & 127u) / 127.0;
    if (glow_byte & THROUGH) != 0u {
        glow = max(glow, THROUGH_GLOW * light_curve(in.light.z));
    }
    if glow > 0.0 {
        color += self_glow(base, albedo.rgb, glow);
        // Its colour kept: the brightest channel held a little past where the display
        // transform starts rolling it towards white (display.wgsl), which turned a torch's
        // orange core pale peach.
        let peak = max(color.r, max(color.g, color.b));
        color *= min(1.0, GLOW_PEAK / max(peak, 1e-4));
    }
    var alpha = albedo.a;

#ifdef TRANSLUCENT
    let to_eye = normalize(view.world_position - in.world_position);
    let sky = fog.base_color.rgb * 1.15;
    let open = smoothstep(0.5, 0.95, in.light.y);
    if kind == TINT_WATER {
        // Water composes itself over the scene behind it, fog included (water_surface.wgsl).
        // Under ice it lies still and reflects nothing; ice draws over it.
        if in.covered == 0u {
            var normal = NORMALS[in.face];
            if in.face == 2u && dot(tilt, tilt) > 1e-14 {
                let sloped = normalize(tilt) * sign(tilt.y);
                normal = select(normal, sloped, sloped.y > 0.7);
            }
            let water = shade_water(in.world_position, normal, biome_tint(in.surface, in.colours, in.world_position, in.face), in.light.y, in.clip_position, footprint, in.flow, in.falling != 0u, in.open_water);
            return vec4(water, 1.0);
        }
    } else {
        // Solid see-through blocks (ice) show only their outside, like any solid block, so
        // a sheet of ice never blends its far side over what lies beyond. Clearer than
        // their texture, they reflect the sky through their own tint, mirror-smooth, with
        // a sharp sun glint.
        if !front_facing {
            discard;
        }
        let n = NORMALS[in.face];
        let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(n, to_eye), 0.0), 5.0);
        let glint = pow(max(dot(reflect(-to_eye, n), sun_direction()), 0.0), 160.0) * open
            * sun_shadow(in.world_position, n, true);
        let hue = base / max(max(base.r, max(base.g, base.b)), 0.001);
        let reflection = sky * mix(vec3(1.0), hue, 0.5);
        color = mix(color, reflection * open + color * (1.0 - open), fresnel * 0.7) + sun_color() * glint * 3.0;
        alpha = mix(alpha * 0.55, 0.95, fresnel);
    }
#endif

    return vec4(apply_fog(color, in.world_position), alpha);
}

#endif
