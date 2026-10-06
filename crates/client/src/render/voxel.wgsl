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
#import voxmar::lighting::{NORMALS, shade, apply_fog, sun_direction, sun_color, sun_shadow, weathered, wet_sheen, self_glow}
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
#ifndef PREPASS_PIPELINE
// Each texture layer's overlay and glow (render/mod.rs, `layer_table`): r and g the layer
// of an overlay drawn over it, tinted (a grass block side's fringe), plus one (0 for
// none), b the overlay's tint, a how brightly the layer glows by itself.
@group(2) @binding(7) var block_layers: texture_2d<f32>;
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
// In the fringe pass: fruit hanging from fruiting leaves (see `fruit_vertex`), and unripe.
const FLAG_FRUIT: u32 = 1u << 25u;
const FLAG_UNRIPE: u32 = 1u << 26u;
// A fruit's `card`, which no leaf card's seed reaches: ripe and unripe.
const FRUIT_UNRIPE: u32 = 0xfffffffeu;
const FRUIT_RIPE: u32 = 0xffffffffu;
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
const PLANT_MODEL_SHIFT: u32 = 28u;
const PLANT_PIECE_SHIFT: u32 = 30u;
const PLANT_HEIGHT_SHIFT: u32 = 14u;
// How far up its plant a mushroom's cap lies, and how far it spreads in from the cell's edge.
const CAP_HEIGHT: f32 = 0.6;
const CAP_INSET: f32 = 0.12;

fn plant_model(w0: u32) -> u32 {
    return (w0 >> PLANT_MODEL_SHIFT) & 3u;
}

// Which quad of its model a plant vertex belongs to: the even ones are packed along one
// diagonal, the odd along the other.
fn plant_piece(w0: u32) -> u32 {
    return ((w0 >> PLANT_PIECE_SHIFT) & 3u) * 2u + (face_of(w0) - FACE_PLANT);
}

fn is_cap(w0: u32) -> bool {
    return plant_model(w0) == MODEL_CAP && plant_piece(w0) == 2u;
}

// Where a model's quad's vertex lies across its cell: past the two crossed diagonal quads
// every model starts with, a # of four straight quads (crops), a flat cap (mushrooms) or a
// straight cross through the middle (bushes).
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
            if piece == 2u {
                var square = array<vec2<f32>, 4>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
                return CAP_INSET + square[corner] * (1.0 - 2.0 * CAP_INSET);
            }
        }
        case MODEL_BUSH: {
            if piece == 2u {
                return vec2(far, 0.5);
            }
            if piece == 3u {
                return vec2(0.5, far);
            }
        }
        default: {}
    }
    return plant_across(w0);
}

// A plant vertex's texture coordinates: along the quad and up it, or across a flat cap.
fn plant_uv(w0: u32) -> vec2<f32> {
    if is_cap(w0) {
        return model_across(w0);
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
    let packed = plant_across(w0);
    let stack = f32((w0 >> PLANT_STACK_SHIFT) & 3u);
    var up = f32((w0 >> 23u) & 1u) + stack;
    // The corner of the cell the whole plant stands in.
    let base = origin + local_position(w0) - vec3(packed.x, up, packed.y);
    let model = plant_model(w0);
    let across = model_across(w0);
    if is_cap(w0) {
        up = stack + CAP_HEIGHT;
    }
    let cell = vec3<i32>(floor(base + 0.5));
    let a = pcg3d(vec3<u32>(cell));
    let b = pcg3d(vec3<u32>(cell + vec3(0, 991, 0)));
    // Crops stand straight with the blocks, in rows.
    let angle = select((a.x - 0.5) * 1.4, 0.0, model == MODEL_HASH);
    let offset = across - 0.5;
    let turned = vec2(offset.x * cos(angle) - offset.y * sin(angle), offset.x * sin(angle) + offset.y * cos(angle));
    let shift = (a.yz - 0.5) * 0.3;
    let height = mix(0.6, 1.25, saturate(0.7 * tufts(base.xz * 0.12) + 0.3 * b.x)) * f32((w1 >> PLANT_HEIGHT_SHIFT) & 31u) / 8.0;
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
    let bend = lean * select(sqrt(rise), rise * rise, rise < 1.0);
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
    // Fruit only: the point it hangs from, which it sways about as one piece, the face
    // it is lit as, its colour and material, and its shading (`fruit_vertex`).
    pivot: vec3<f32>,
    face: u32,
    fruit: u32,
    ao: f32,
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

// Fruit is modelled from boxes, as a voxel artist would build it: a round fruit is three
// crossed boxes, a pixel shy of a cube at every edge, with a stem and perhaps a leaf; a pear
// adds a neck, cherries hang in pairs on long stems, bananas grow up a stalk in hands. Each
// box is two corners in model pixels (16 to a block) hanging from the origin, with its
// material in the first corner's w. Which boxes are whose: mesh.rs `FRUIT_MODELS`.
const FRUIT_BOXES = array<vec4<f32>, 334>(
    // apple: 5
    vec4(-3.0, -7.0, -2.0, 0.0), vec4(3.0, -3.0, 2.0, 0.0),
    vec4(-2.0, -8.0, -2.0, 0.0), vec4(2.0, -2.0, 2.0, 0.0),
    vec4(-2.0, -7.0, -3.0, 0.0), vec4(2.0, -3.0, 3.0, 0.0),
    vec4(-0.5, -2.5, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    vec4(0.5, -1.25, -0.75, 2.0), vec4(3.0, -0.75, 0.75, 0.0),
    // plum: 4
    vec4(-2.5, -6.0, -1.5, 0.0), vec4(2.5, -3.0, 1.5, 0.0),
    vec4(-1.5, -7.0, -1.5, 0.0), vec4(1.5, -2.0, 1.5, 0.0),
    vec4(-1.5, -6.0, -2.5, 0.0), vec4(1.5, -3.0, 2.5, 0.0),
    vec4(-0.5, -2.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // walnut: 4
    vec4(-3.0, -7.0, -2.0, 0.0), vec4(3.0, -3.0, 2.0, 0.0),
    vec4(-2.0, -8.0, -2.0, 0.0), vec4(2.0, -2.0, 2.0, 0.0),
    vec4(-2.0, -7.0, -3.0, 0.0), vec4(2.0, -3.0, 3.0, 0.0),
    vec4(-0.5, -2.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // orange: 6
    vec4(-3.5, -8.0, -2.5, 0.0), vec4(3.5, -3.0, 2.5, 0.0),
    vec4(-2.5, -9.0, -2.5, 0.0), vec4(2.5, -2.0, 2.5, 0.0),
    vec4(-2.5, -8.0, -3.5, 0.0), vec4(2.5, -3.0, 3.5, 0.0),
    vec4(-1.0, -2.25, -1.0, 2.0), vec4(1.0, -1.75, 1.0, 0.0),
    vec4(-0.5, -1.75, -0.5, 3.0), vec4(0.5, -1.0, 0.5, 0.0),
    vec4(-0.5, -1.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // ember: 6
    vec4(-2.5, -7.5, -1.5, 0.0), vec4(2.5, -4.5, 1.5, 0.0),
    vec4(-1.5, -8.5, -1.5, 0.0), vec4(1.5, -3.5, 1.5, 0.0),
    vec4(-1.5, -7.5, -2.5, 0.0), vec4(1.5, -4.5, 2.5, 0.0),
    vec4(-1.5, -4.0, -1.5, 4.0), vec4(1.5, -2.5, 1.5, 0.0),
    vec4(-0.5, -2.5, -0.5, 4.0), vec4(0.5, -1.5, 0.5, 0.0),
    vec4(-0.5, -1.5, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // pear: 6
    vec4(-3.0, -9.0, -2.0, 0.0), vec4(3.0, -5.0, 2.0, 0.0),
    vec4(-2.0, -10.0, -2.0, 0.0), vec4(2.0, -4.0, 2.0, 0.0),
    vec4(-2.0, -9.0, -3.0, 0.0), vec4(2.0, -5.0, 3.0, 0.0),
    vec4(-2.0, -5.5, -2.0, 0.0), vec4(2.0, -4.0, 2.0, 0.0),
    vec4(-1.5, -4.0, -1.5, 0.0), vec4(1.5, -2.0, 1.5, 0.0),
    vec4(-0.5, -2.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // avocado: 6
    vec4(-3.0, -10.5, -2.0, 0.0), vec4(3.0, -5.5, 2.0, 0.0),
    vec4(-2.0, -11.5, -2.0, 0.0), vec4(2.0, -4.5, 2.0, 0.0),
    vec4(-2.0, -10.5, -3.0, 0.0), vec4(2.0, -5.5, 3.0, 0.0),
    vec4(-2.0, -5.0, -2.0, 0.0), vec4(2.0, -3.5, 2.0, 0.0),
    vec4(-1.5, -3.5, -1.5, 0.0), vec4(1.5, -2.5, 1.5, 0.0),
    vec4(-0.5, -2.5, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // fig: 5
    vec4(-2.5, -7.5, -1.5, 0.0), vec4(2.5, -4.5, 1.5, 0.0),
    vec4(-1.5, -8.5, -1.5, 0.0), vec4(1.5, -3.5, 1.5, 0.0),
    vec4(-1.5, -7.5, -2.5, 0.0), vec4(1.5, -4.5, 2.5, 0.0),
    vec4(-1.0, -4.0, -1.0, 0.0), vec4(1.0, -2.5, 1.0, 0.0),
    vec4(-0.5, -2.5, -0.5, 2.0), vec4(0.5, 0.5, 0.5, 0.0),
    // lemon: 6
    vec4(-2.5, -8.0, -1.5, 0.0), vec4(2.5, -3.0, 1.5, 0.0),
    vec4(-1.5, -9.0, -1.5, 0.0), vec4(1.5, -2.0, 1.5, 0.0),
    vec4(-1.5, -8.0, -2.5, 0.0), vec4(1.5, -3.0, 2.5, 0.0),
    vec4(-0.5, -10.0, -0.5, 0.0), vec4(0.5, -9.0, 0.5, 0.0),
    vec4(-0.5, -2.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    vec4(0.5, -1.25, -0.75, 2.0), vec4(3.0, -0.75, 0.75, 0.0),
    // mango: 4
    vec4(-3.0, -10.0, -1.5, 0.0), vec4(3.0, -3.0, 1.5, 0.0),
    vec4(-2.0, -11.0, -1.5, 0.0), vec4(2.0, -2.0, 1.5, 0.0),
    vec4(-2.0, -10.0, -2.5, 0.0), vec4(2.0, -3.0, 2.5, 0.0),
    vec4(-0.5, -2.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // cherries: 10
    vec4(-3.25, -8.0, -1.0, 0.0), vec4(-0.25, -6.0, 1.0, 0.0),
    vec4(-2.75, -8.5, -1.0, 0.0), vec4(-0.75, -5.5, 1.0, 0.0),
    vec4(-2.75, -8.0, -1.5, 0.0), vec4(-0.75, -6.0, 1.5, 0.0),
    vec4(0.0, -7.0, -0.5, 0.0), vec4(3.0, -5.0, 1.5, 0.0),
    vec4(0.5, -7.5, -0.5, 0.0), vec4(2.5, -4.5, 1.5, 0.0),
    vec4(0.5, -7.0, -1.0, 0.0), vec4(2.5, -5.0, 2.0, 0.0),
    vec4(-2.0, -5.5, -0.25, 1.0), vec4(-1.5, -0.75, 0.25, 0.0),
    vec4(1.25, -4.5, 0.25, 1.0), vec4(1.75, -0.75, 0.75, 0.0),
    vec4(-2.0, -0.75, -0.25, 1.0), vec4(1.75, -0.25, 0.75, 0.0),
    vec4(-0.25, -0.25, 0.0, 1.0), vec4(0.25, 0.5, 0.5, 0.0),
    // rowan: 9
    vec4(-0.25, -3.0, -0.25, 1.0), vec4(0.25, 0.5, 0.25, 0.0),
    vec4(-0.875, -4.875, -0.875, 0.0), vec4(0.875, -3.125, 0.875, 0.0),
    vec4(1.125, -4.375, -0.375, 0.0), vec4(2.875, -2.625, 1.375, 0.0),
    vec4(-2.875, -4.375, -1.375, 0.0), vec4(-1.125, -2.625, 0.375, 0.0),
    vec4(-0.375, -4.375, 1.125, 0.0), vec4(1.375, -2.625, 2.875, 0.0),
    vec4(-1.375, -4.375, -2.875, 0.0), vec4(0.375, -2.625, -1.125, 0.0),
    vec4(0.625, -6.125, -2.375, 0.0), vec4(2.375, -4.375, -0.625, 0.0),
    vec4(-2.375, -6.125, 0.625, 0.0), vec4(-0.625, -4.375, 2.375, 0.0),
    vec4(0.375, -6.375, 0.625, 0.0), vec4(2.125, -4.625, 2.375, 0.0),
    // olives: 7
    vec4(-0.25, -5.0, -0.25, 1.0), vec4(0.25, 0.5, 0.25, 0.0),
    vec4(-2.25, -5.75, -0.75, 0.0), vec4(-0.75, -2.25, 0.75, 0.0),
    vec4(-2.5, -5.25, -1.0, 0.0), vec4(-0.5, -2.75, 1.0, 0.0),
    vec4(0.5, -6.75, 0.0, 0.0), vec4(2.0, -3.25, 1.5, 0.0),
    vec4(0.25, -6.25, -0.25, 0.0), vec4(2.25, -3.75, 1.75, 0.0),
    vec4(-0.75, -8.5, -2.0, 0.0), vec4(0.75, -5.0, -0.5, 0.0),
    vec4(-1.0, -8.0, -2.25, 0.0), vec4(1.0, -5.5, -0.25, 0.0),
    // hazelnut: 10
    vec4(-0.25, -2.5, -0.25, 1.0), vec4(0.25, 0.5, 0.25, 0.0),
    vec4(-2.5, -5.75, -1.0, 0.0), vec4(-0.5, -2.75, 1.0, 0.0),
    vec4(-2.75, -5.25, -1.25, 0.0), vec4(-0.25, -3.25, 1.25, 0.0),
    vec4(-3.0, -3.5, -1.5, 2.0), vec4(0.0, -2.5, 1.5, 0.0),
    vec4(0.5, -6.25, -0.5, 0.0), vec4(2.5, -3.25, 1.5, 0.0),
    vec4(0.25, -5.75, -0.75, 0.0), vec4(2.75, -3.75, 1.75, 0.0),
    vec4(0.0, -4.0, -1.0, 2.0), vec4(3.0, -3.0, 2.0, 0.0),
    vec4(-1.0, -6.75, -2.75, 0.0), vec4(1.0, -3.75, -0.75, 0.0),
    vec4(-1.25, -6.25, -3.0, 0.0), vec4(1.25, -4.25, -0.5, 0.0),
    vec4(-1.5, -4.5, -3.25, 2.0), vec4(1.5, -3.5, -0.25, 0.0),
    // chestnut: 13
    vec4(-2.5, -6.5, -1.5, 0.0), vec4(2.5, -3.5, 1.5, 0.0),
    vec4(-1.5, -7.5, -1.5, 0.0), vec4(1.5, -2.5, 1.5, 0.0),
    vec4(-1.5, -6.5, -2.5, 0.0), vec4(1.5, -3.5, 2.5, 0.0),
    vec4(2.5, -5.5, -0.5, 4.0), vec4(3.5, -4.5, 0.5, 0.0),
    vec4(-3.5, -5.5, -0.5, 4.0), vec4(-2.5, -4.5, 0.5, 0.0),
    vec4(-0.5, -5.5, 2.5, 4.0), vec4(0.5, -4.5, 3.5, 0.0),
    vec4(-0.5, -5.5, -3.5, 4.0), vec4(0.5, -4.5, -2.5, 0.0),
    vec4(-0.5, -8.5, -0.5, 4.0), vec4(0.5, -7.5, 0.5, 0.0),
    vec4(1.5, -3.5, 1.5, 4.0), vec4(2.5, -2.5, 2.5, 0.0),
    vec4(-2.5, -3.5, -2.5, 4.0), vec4(-1.5, -2.5, -1.5, 0.0),
    vec4(1.5, -7.5, -2.5, 4.0), vec4(2.5, -6.5, -1.5, 0.0),
    vec4(-2.5, -7.5, 1.5, 4.0), vec4(-1.5, -6.5, 2.5, 0.0),
    vec4(-0.5, -2.5, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // almond: 4
    vec4(-1.5, -6.0, -1.0, 0.0), vec4(1.5, -2.5, 1.0, 0.0),
    vec4(-1.0, -6.75, -0.75, 0.0), vec4(1.0, -2.0, 0.75, 0.0),
    vec4(-1.0, -5.5, -1.25, 0.0), vec4(1.0, -3.0, 1.25, 0.0),
    vec4(-0.5, -2.0, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // banana: 14
    vec4(-0.75, -13.0, -0.75, 2.0), vec4(0.75, 0.5, 0.75, 0.0),
    vec4(-1.25, -16.0, -1.25, 5.0), vec4(1.25, -13.0, 1.25, 0.0),
    vec4(1.5, -6.5, -0.75, 0.0), vec4(3.0, -1.5, 0.75, 0.0),
    vec4(2.0, -1.5, -0.5, 3.0), vec4(3.0, -0.75, 0.5, 0.0),
    vec4(-0.75, -6.5, 1.5, 0.0), vec4(0.75, -1.5, 3.0, 0.0),
    vec4(-0.5, -1.5, 2.0, 3.0), vec4(0.5, -0.75, 3.0, 0.0),
    vec4(-3.0, -6.5, -0.75, 0.0), vec4(-1.5, -1.5, 0.75, 0.0),
    vec4(-3.0, -1.5, -0.5, 3.0), vec4(-2.0, -0.75, 0.5, 0.0),
    vec4(-0.75, -6.5, -3.0, 0.0), vec4(0.75, -1.5, -1.5, 0.0),
    vec4(-0.5, -1.5, -3.0, 3.0), vec4(0.5, -0.75, -2.0, 0.0),
    vec4(1.5, -12.0, -0.75, 0.0), vec4(3.0, -7.0, 0.75, 0.0),
    vec4(2.0, -7.0, -0.5, 3.0), vec4(3.0, -6.25, 0.5, 0.0),
    vec4(-3.0, -12.0, -0.75, 0.0), vec4(-1.5, -7.0, 0.75, 0.0),
    vec4(-3.0, -7.0, -0.5, 3.0), vec4(-2.0, -6.25, 0.5, 0.0),
    // coconut: 5
    vec4(-4.0, -7.0, -2.0, 0.0), vec4(4.0, -3.0, 2.0, 0.0),
    vec4(-2.0, -9.0, -2.0, 0.0), vec4(2.0, -1.0, 2.0, 0.0),
    vec4(-2.0, -7.0, -4.0, 0.0), vec4(2.0, -3.0, 4.0, 0.0),
    vec4(-3.0, -8.0, -3.0, 0.0), vec4(3.0, -2.0, 3.0, 0.0),
    vec4(-0.75, -1.5, -0.75, 2.0), vec4(0.75, 0.5, 0.75, 0.0),
    // cacao: 9
    vec4(-2.5, -9.5, -1.5, 0.0), vec4(2.5, -2.5, 1.5, 0.0),
    vec4(-1.5, -10.5, -1.5, 0.0), vec4(1.5, -1.5, 1.5, 0.0),
    vec4(-1.5, -9.5, -2.5, 0.0), vec4(1.5, -2.5, 2.5, 0.0),
    vec4(-0.5, -11.5, -0.5, 0.0), vec4(0.5, -10.5, 0.5, 0.0),
    vec4(2.5, -9.0, -0.25, 3.0), vec4(2.75, -3.0, 0.25, 0.0),
    vec4(-2.75, -9.0, -0.25, 3.0), vec4(-2.5, -3.0, 0.25, 0.0),
    vec4(-0.25, -9.0, 2.5, 3.0), vec4(0.25, -3.0, 2.75, 0.0),
    vec4(-0.25, -9.0, -2.75, 3.0), vec4(0.25, -3.0, -2.5, 0.0),
    vec4(-0.5, -1.5, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
    // baobab: 4
    vec4(-0.25, -6.0, -0.25, 1.0), vec4(0.25, 0.5, 0.25, 0.0),
    vec4(-2.5, -14.0, -1.5, 0.0), vec4(2.5, -7.0, 1.5, 0.0),
    vec4(-1.5, -15.0, -1.5, 0.0), vec4(1.5, -6.0, 1.5, 0.0),
    vec4(-1.5, -14.0, -2.5, 0.0), vec4(1.5, -7.0, 2.5, 0.0),
    // dates: 15
    vec4(-0.5, -4.0, -0.5, 6.0), vec4(0.5, 0.5, 0.5, 0.0),
    vec4(-2.0, -4.5, -0.25, 6.0), vec4(2.0, -4.0, 0.25, 0.0),
    vec4(-0.25, -4.5, -2.0, 6.0), vec4(0.25, -4.0, 2.0, 0.0),
    vec4(1.5, -12.0, -0.25, 6.0), vec4(2.0, -4.5, 0.25, 0.0),
    vec4(-2.0, -12.0, -0.25, 6.0), vec4(-1.5, -4.5, 0.25, 0.0),
    vec4(-0.25, -12.0, 1.5, 6.0), vec4(0.25, -4.5, 2.0, 0.0),
    vec4(-0.25, -12.0, -2.0, 6.0), vec4(0.25, -4.5, -1.5, 0.0),
    vec4(1.7, -8.25, -0.75, 0.0), vec4(3.2, -5.75, 0.75, 0.0),
    vec4(1.7, -12.25, -0.75, 0.0), vec4(3.2, -9.75, 0.75, 0.0),
    vec4(-3.2, -8.25, -0.75, 0.0), vec4(-1.7, -5.75, 0.75, 0.0),
    vec4(-3.2, -12.25, -0.75, 0.0), vec4(-1.7, -9.75, 0.75, 0.0),
    vec4(-0.75, -9.75, 1.7, 0.0), vec4(0.75, -7.25, 3.2, 0.0),
    vec4(-0.75, -13.75, 1.7, 0.0), vec4(0.75, -11.25, 3.2, 0.0),
    vec4(-0.75, -9.75, -3.2, 0.0), vec4(0.75, -7.25, -1.7, 0.0),
    vec4(-0.75, -13.75, -3.2, 0.0), vec4(0.75, -11.25, -1.7, 0.0),
    // tamarind: 7
    vec4(-2.75, -5.0, -0.5, 0.0), vec4(-1.25, -0.5, 0.5, 0.0),
    vec4(-2.25, -9.0, -0.5, 0.0), vec4(-0.75, -5.0, 0.5, 0.0),
    vec4(-0.5, -5.0, 0.5, 0.0), vec4(1.0, -0.5, 1.5, 0.0),
    vec4(0.0, -9.0, 0.5, 0.0), vec4(1.5, -5.0, 1.5, 0.0),
    vec4(1.25, -5.0, -1.25, 0.0), vec4(2.75, -0.5, -0.25, 0.0),
    vec4(1.75, -9.0, -1.25, 0.0), vec4(3.25, -5.0, -0.25, 0.0),
    vec4(-2.0, 0.0, -0.25, 1.0), vec4(2.75, 0.5, 1.0, 0.0),
    // pinecone: 8
    vec4(-3.0, -7.0, -2.0, 0.0), vec4(3.0, -2.0, 2.0, 0.0),
    vec4(-2.0, -8.0, -2.0, 0.0), vec4(2.0, -1.0, 2.0, 0.0),
    vec4(-2.0, -7.0, -3.0, 0.0), vec4(2.0, -2.0, 3.0, 0.0),
    vec4(-3.25, -3.25, -2.0, 3.0), vec4(3.25, -2.75, 2.0, 0.0),
    vec4(-2.0, -3.25, -3.25, 3.0), vec4(2.0, -2.75, 3.25, 0.0),
    vec4(-3.25, -6.25, -2.0, 3.0), vec4(3.25, -5.75, 2.0, 0.0),
    vec4(-2.0, -6.25, -3.25, 3.0), vec4(2.0, -5.75, 3.25, 0.0),
    vec4(-0.5, -1.5, -0.5, 1.0), vec4(0.5, 0.5, 0.5, 0.0),
);
// Materials: the fruit's own colour (from its texture), wood, green (leaf, calyx, husk),
// the fruit darker and lighter, a banana flower and a date stalk.
const FRUIT_WOOD = vec3(0.10, 0.048, 0.019);
const FRUIT_GREEN = vec3(0.078, 0.195, 0.025);
const FRUIT_BUD = vec3(0.10, 0.010, 0.040);
const FRUIT_GOLD = vec3(0.48, 0.25, 0.030);
// A box face's two edges, u × v its outward normal, so it winds counter-clockwise seen from
// outside: by face (+x, -x, +y, -y, +z, -z), the axis each runs along.
const BOX_U = array<u32, 6>(1u, 2u, 2u, 0u, 0u, 1u);
const FRUIT_FACE_SHADE = array<f32, 6>(0.84, 0.84, 1.0, 0.62, 0.93, 0.93);
const BOX_V = array<u32, 6>(2u, 1u, 0u, 2u, 1u, 0u);

// A fruit's colour: its texture's at the middle of the fruit drawn there (an apple's, a
// coconut's), or else of the left cherry of a pair, as every fruit's texture is drawn.
fn fruit_skin(layer: i32) -> vec3<f32> {
    let texels = f32(textureDimensions(block_textures).x) / 16.0;
    let spots = array<vec2<f32>, 3>(vec2(8.5, 8.5), vec2(3.5, 10.5), vec2(12.5, 10.5));
    var colour = vec3(0.3, 0.15, 0.1);
    for (var i = 2; i >= 0; i--) {
        let texel = textureLoad(block_textures, vec2<i32>(spots[i] * texels), layer, 0);
        if texel.a > 0.5 {
            colour = texel.rgb;
        }
    }
    return colour;
}

// A vertex of fruit hanging from a fruiting leaf block (mesh.rs `emit_fruit`): a corner of
// a face of one of its model's boxes. The model hangs under the block (or out of an open
// side) at a hash of the block, turned and sized at random and by ripeness: unripe fruit
// is smaller and green.
fn fruit_vertex(w0: u32, w1: u32, origin: vec3<f32>) -> Card {
    let cell = origin + local_position(w0);
    let corner = (w0 >> 21u) & 3u;
    let side = (w0 >> 28u) & 7u;
    let index = w0 >> 31u;
    let id = (w1 >> 12u) & 255u;
    let lo = FRUIT_BOXES[id * 2u];
    let hi = FRUIT_BOXES[id * 2u + 1u];
    let unripe = (w0 & FLAG_UNRIPE) != 0u;
    let block = vec3<i32>(floor(cell + 0.5));
    let h = pcg3d(vec3<u32>(block + vec3(i32(index) * 13, 211, 0)));
    let g = pcg3d(vec3<u32>(block + vec3(0, 307, 0)));

    // The corner, in model pixels.
    let axis = side / 2u;
    let u = BOX_U[side];
    let v = BOX_V[side];
    let along = vec2(f32(corner == 1u || corner == 2u), f32(corner >= 2u));
    var p = select(lo.xyz, hi.xyz, side % 2u == 0u);
    p[u] = mix(lo[u], hi[u], along.x);
    p[v] = mix(lo[v], hi[v], along.y);

    // Where it hangs from.
    // A block's two fruits hang on opposite sides of it, a hash's way round, so they never
    // run together into one shape.
    let hang = face_of(w0);
    let away = select(0.24, -0.24, index == 1u) + (h.y - 0.5) * 0.08;
    let bearing = g.x * 6.2832;
    let spread = vec2(cos(bearing), sin(bearing)) * away;
    var pivot = cell + vec3(0.5 + spread.x, 0.1, 0.5 + spread.y);
    if hang != 3u {
        // NORMALS lives in lighting.wgsl, which the depth prepass doesn't import.
        var n = vec3(0.0);
        n[hang / 2u] = 1.0 - 2.0 * f32(hang % 2u);
        let t = vec3(n.z, 0.0, -n.x);
        pivot = cell + 0.5 + n * 0.68 + t * select(0.24, -0.24, index == 1u) + vec3(0.0, 0.3 + 0.15 * (h.z - 0.5), 0.0);
    }
    // Turned a quarter turn at random and a little more, so its pixels stay square on to
    // the light.
    let turn = floor(h.z * 4.0) * 1.5708 + (h.y - 0.5) * 0.35;
    let c = cos(turn);
    let s = sin(turn);
    let size = select(1.0, 0.7, unripe) * (0.9 + 0.2 * h.x) / 16.0;
    let q = p * size;
    var fruit: Card;
    fruit.world = pivot + vec3(q.x * c - q.z * s, q.y, q.x * s + q.z * c);
    fruit.pivot = pivot;
    fruit.uv = vec2(p[u] - lo[u], p[v] - lo[v]);
    fruit.seed = select(FRUIT_RIPE, FRUIT_UNRIPE, unripe);
    // Lit as the cube face its own face turned towards.
    fruit.face = side;
    if axis != 1u {
        var n = vec2(0.0);
        n[axis / 2u] = 1.0 - 2.0 * f32(side % 2u);
        let m = vec2(n.x * c - n.y * s, n.x * s + n.y * c);
        fruit.face = select(select(5u, 4u, m.y > 0.0), select(1u, 0u, m.x > 0.0), abs(m.x) > abs(m.y));
    }
    // Shaded as an entity's model is, so its volume reads even in the crown's shade: each
    // face by which way it turns (top lightest, underside darkest), and darker towards the
    // bottom of the fruit, as light from above falls off round it.
    fruit.ao = clamp(1.0 + p.y * 0.035, 0.72, 1.0);

    // Its colour: unripe fruit greys towards green; the materials not its own keep theirs.
    var skin = fruit_skin(i32(w1 & 0xFFFu));
    if unripe {
        skin = dot(skin, vec3(0.3, 0.55, 0.15)) * vec3(0.75, 1.15, 0.45);
    }
    let material = u32(lo.w);
    var colour = skin;
    switch material {
        case 1u: { colour = FRUIT_WOOD; }
        case 2u: { colour = FRUIT_GREEN; }
        case 3u: { colour = skin * 0.55; }
        case 4u: { colour = min(skin * 1.35, vec3(1.0)); }
        case 5u: { colour = FRUIT_BUD; }
        case 6u: { colour = FRUIT_GOLD; }
        default: {}
    }
    colour *= FRUIT_FACE_SHADE[fruit.face];
    // Packed as its square root, so dark fruit keeps its shades in 8 bits; the alpha holds
    // the material and a seed for the pixels' pattern.
    fruit.fruit = pack4x8unorm(vec4(sqrt(colour), 0.0)) | ((material | (u32(h.x * 31.0) << 3u)) << 24u);
    return fruit;
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
    out.uv = select(face_uv(local_position(in.packed.x), face_of(in.packed.x)), plant_uv(in.packed.x), face_of(in.packed.x) >= FACE_PLANT);
    out.surface = in.packed.y & 0xFFFFFFu;
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
    // Fruit's colour and material (`fruit_vertex`).
    @location(11) @interpolate(flat) fruit: u32,
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
    // The fringe pass draws only leaf cards and fruit.
    let fruit = (w0 & FLAG_FRUIT) != 0u;
    var card: Card;
    if fruit {
        card = fruit_vertex(w0, w1, vec3<f32>(chunk_of(in).xyz));
    } else {
        card = card_vertex(w0, vec3<f32>(chunk_of(in).xyz));
        card.pivot = card.world;
    }
    out.world_position = card.world + leaf_flutter(card.pivot) * motion_near(card.pivot);
    out.card = card.seed;
    out.fruit = card.fruit;
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
    out.face = face_of(w0);
    out.uv = select(face_uv(local_position(w0), out.face), plant_uv(w0), out.face >= FACE_PLANT);
#ifdef FRINGE
    out.uv = card.uv;
    // Lit as the face of the crown it grows from, so a card seen edge on matches it.
    out.face = select(w0 >> CARD_FACE_SHIFT, card.face, fruit);
#endif
    // Plants have no ambient occlusion; their AO bits hold the corner, and water's its flow.
    let water = tint_kind(w1) == TINT_WATER;
    let ao = select(AO_CURVE[(w0 >> 21u) & 3u], 1.0, face_of(w0) >= FACE_PLANT || water);
    out.light = vec3(ao, f32(w1 >> 28u) / 15.0, f32((w1 >> 24u) & 15u) / 15.0);
    out.surface = w1 & 0xFFFFFFu;
#ifdef FRINGE
    if fruit {
        // Its texture layer alone: fruit takes no tint; and its own shading, not a corner's.
        out.surface = w1 & 0xFFFu;
        out.light.x = card.ao;
    }
#endif
    out.covered = select(0u, 1u, out.face < FACE_PLANT && (w0 & FLAG_INNER) != 0u);
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
        var leaves = mix(colour, grey * vec3(1.25, 1.2, 0.75), 0.35) * 1.15;
        // Neighbouring crowns differ a little in shade and hue, as real trees do, so a
        // canopy from above is a quilt of greens rather than one flat colour: smooth noise
        // at about a crown's size.
        let shade = tufts(inside.xz * 0.19 + vec2(41.0, 17.0));
        let hue = tufts(inside.xz * 0.13 + vec2(-23.0, 88.0));
        leaves *= 0.8 + 0.4 * shade;
        leaves *= mix(vec3(0.9, 1.0, 1.12), vec3(1.14, 1.04, 0.8), hue);
        return leaves;
    }
    return colour;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
#ifdef TRANSLUCENT
    // Blocks a pixel spans on the surface, for the water's waves, and the surface's own
    // normal, which tilts where flowing water slopes; derivatives are only defined here,
    // before any branch.
    let footprint = length(fwidth(in.world_position.xz));
    let tilt = cross(dpdx(in.world_position), dpdy(in.world_position));
#endif
    let layer = i32(in.surface & 0xFFFu);
    // The layer's overlay, tint, variety and glow (render/mod.rs, `layer_table`).
    let entry = textureLoad(block_layers, vec2(layer, 0), 0);
    let tint_byte = u32(entry.b * 255.0 + 0.5);
    var look = Laid(in.uv, dpdx(in.uv), dpdy(in.uv));
    if tint_byte >= 64u {
        let cell = vec3<i32>(floor(in.world_position - NORMALS[in.face] * 0.5));
        look = laid(in.uv, look.dx, look.dy, in.face, cell, (tint_byte & 64u) != 0u, (tint_byte & 128u) != 0u);
    }
    var albedo = textureSampleGrad(block_textures, block_sampler, look.uv, layer, look.dx, look.dy);
    let kind = tint_kind(in.surface);
#ifdef FRINGE
    if in.card < FRUIT_UNRIPE && !card_mask(in.uv, in.card, length(fwidth(in.uv)) * 16.0) {
        discard;
    }
    if in.card >= FRUIT_UNRIPE {
        // Fruit is solid, in its colour in a pixel artist's three shades: mostly the middle
        // one, some pixels darker and a few lighter.
        let packed = unpack4x8unorm(in.fruit);
        let pattern = in.fruit >> 24u;
        let pixel = vec2<u32>(vec2<i32>(floor(in.uv)) + 64);
        let r = pcg3d(vec3(pixel, in.face * 32u + (pattern >> 3u))).x;
        let shade = select(select(1.0, 0.78, r < 0.22), 1.2, r > 0.86);
        albedo = vec4(packed.rgb * packed.rgb * shade, 1.0);
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
    if any(in.uv < vec2(0.0)) || any(in.uv > vec2(1.0)) || in.card >= FRUIT_UNRIPE {
        overlay = 0u;
    }
#endif
    if overlay != 0u {
        let over = textureSampleGrad(block_textures, block_sampler, look.uv, i32(overlay - 1u), look.dx, look.dy);
        let tint = biome_colour(tint_byte & 63u, in.colours, in.world_position, in.face);
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
        var weather_normal = NORMALS[face];
#ifdef FRINGE
        // A flat leaf card, and the upper half of an upright one, catch snow and frost as
        // the top of the crown does, so a canopy whitens under snow.
        if in.card < FRUIT_UNRIPE && ((in.card & 1u) != 0u || in.uv.y < 0.5) {
            weather_normal = vec3(0.0, 1.0, 0.0);
        }
#endif
        surface = weathered(base, in.world_position, weather_normal, in.light.y);
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
    let glow = entry.a;
    if glow > 0.0 {
        color += self_glow(base, albedo.rgb, glow);
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
