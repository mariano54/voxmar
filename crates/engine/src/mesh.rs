//! Binary greedy meshing with smooth lighting and ambient occlusion.
//!
//! Face culling is done 64 cells at a time on bit columns. Visible faces are then merged
//! per slice into the largest rectangles whose faces share texture, tint, AO and light, so
//! lighting stays exact while open terrain collapses to a handful of quads.
//!
//! Plants are not merged: each is two crossed quads through its cell's diagonals.
//!
//! Each vertex is packed into 8 bytes:
//!
//! ```text
//! word 0: x:6 | y:6 | z:6 | face:3 | ao:2 | flags:2
//! word 1: layer:12 | tint:2 | free:10 | block:4 | sky:4
//! ```
//!
//! `face` is a `Face` (0..=5) for cube faces, or `PLANT_DIAGONAL` (6, the plane through
//! x = z) or `PLANT_ANTIDIAGONAL` (7, through x + z = 1) for plants. On leaf faces, flag 1
//! marks a double-sided face between two leaves (meshed apart, in `ChunkMesh::inner`), and
//! flag 2 one with leaves behind it, which casts a solid shadow. On translucent faces flag 1 marks a
//! face covered by a block (water under ice). On plant vertices flag 1 marks the top edge,
//! the one that sways in the wind. Plants carry no ambient occlusion: their `ao` bits hold the
//! vertex's corner (0..=3, in the order the quad is built), and bits 25..27 how many plant
//! cells the plant stands on, so the shader can place each plant as a whole (`voxel.wgsl`);
//! bits 28..29 its model (`PlantModel`) and 30..31 with the face's low bit which quad of the
//! model it is, which the shader shapes; word 1 bits 14..18 its height in eighths.
//! Bit 27 marks the same quads used as leaf cards (`ChunkMesh::fringe`), with flag 2 for a
//! flat card, bit 28 for a conifer's needles and bits 29..31 for the face of the crown it is
//! shaded as; their top edge is flag 1, as on a plant.
//! A block's shape goes as plant faces of the shape model (`PLANT_SHAPE`): its quad's number in
//! the renderer's shape table in word 1 bits 12..23 and, above those, word 0 bits 30..31.
//!
//! Faces of translucent fluids (water) carry the fluid's state instead of ambient
//! occlusion, which their shading has no use for:
//!
//! ```text
//! word 0: x:6 | y:6 | z:6 | face:3 | flow speed:2 | covered:1 | falling:1 | drop:3 | flow direction:4
//! word 1: layer:12 | tint:2 | open water:4 | free:6 | block:4 | sky:4
//! ```
//!
//! `drop` is how far the surface lies below the top of the block at the vertex, in eighths
//! (from `fluid::corner_heights`, so flows slope down from their source), on the top face
//! and the top edge of side faces. The flow (`fluid::flow`) is a direction in 16ths of a
//! turn counter-clockwise from +x towards +z and a speed (`FLOW_SPEEDS`), the cell's own,
//! the same at every corner.
//! `falling` marks the faces of falling cells, and the sides over which water pours down an
//! edge. `open water` is how much of the surface around the vertex (24 blocks across) is
//! open water at its level, in 15ths: a puddle or a sheltered pool is nearly 0, the middle
//! of a lake 15, so only open water carries wind waves. Faces between two cells of the same
//! fluid are hidden, whatever their states.

use std::cell::RefCell;

use glam::{IVec2, IVec3, UVec3, Vec2};

use crate::block::{BlockId, BlockRegistry, PlantModel, RenderKind, Support, Tint};
use crate::coords::{CHUNK_SIZE, Face, WORLD_HEIGHT};
use crate::fluid::{self, FluidState};
use crate::light::{LIGHT_PAD, LitRegion, MAX_LIGHT};
use crate::visibility::Connectivity;
use crate::world::Column;

const S: usize = CHUNK_SIZE as usize;
/// Chunk size plus a one-block border on every side.
const P: usize = S + 2;

pub type Vertex = [u32; 2];

/// Face index of a plant's quad through the cell diagonal from (0, 0) to (1, 1) in x, z.
pub const PLANT_DIAGONAL: u32 = 6;
/// Face index of a plant's quad through the diagonal from (1, 0) to (0, 1).
pub const PLANT_ANTIDIAGONAL: u32 = 7;
/// Vertex flag 1 (see the module docs).
const FLAG_INNER: u32 = 1 << 23;
/// Marks the quads of a leaf card, which the shader grows out of the leaf block.
const FLAG_FRINGE: u32 = 1 << 27;
/// On a fringe: the face of the crown the card is shaded as (bits 29..31).
const CARD_FACE_SHIFT: u32 = 29;
/// On a fringe: the card lies flat rather than upright (flag 2).
const FLAG_FLAT: u32 = 1 << 24;
/// On a fringe: the leaves are a conifer's needles, whose sprigs the shader keeps small and
/// drooping, so a spruce's tiers stay neat rather than bristling.
const FLAG_NEEDLES: u32 = 1 << 28;
/// On fluid faces: how far the surface lies below the block's top, in eighths (bits 25..27),
/// and the flow's direction in 16ths of a turn (bits 28..31); see the module docs.
const SURFACE_DROP_SHIFT: u32 = 25;
const FLOW_DIRECTION_SHIFT: u32 = 28;
/// Speeds a flow is rounded to, in blocks per second: still, then `fluid::Current`'s.
pub const FLOW_SPEEDS: [f32; 4] = [0.0, 0.7, 1.4, 2.8];
/// On fluid faces, word 1 bits 14..17 hold how open the water around the vertex is.
const OPEN_WATER_SHIFT: u32 = 14;
/// Half the side of the square of surface whose open water a vertex counts, in blocks.
const OPEN_WATER_REACH: i32 = 12;
/// On plant vertices, bits 25..27 hold how many plant cells lie below in the same stack
/// (for a plant on a wall, `PlantModel::Wall`, which way it faces).
const PLANT_STACK_SHIFT: u32 = 25;
/// On plant vertices, bits 28..29 the model and 30..31 the quad of it (with the face's low
/// bit); word 1 bits 14..18 the plant's height in eighths. Bit 24 is the model's third bit.
const PLANT_MODEL_SHIFT: u32 = 28;
/// On plant vertices, bit 27: the plant hangs from the block above (`Support::Above`).
const PLANT_HANGS_SHIFT: u32 = 27;
const PLANT_MODEL_HIGH_SHIFT: u32 = 24;
const PLANT_PIECE_SHIFT: u32 = 30;
/// The plant model a shape's quads go as (after `PlantModel`'s own): voxel.wgsl places
/// each from the renderer's shape table, and the renderer draws them one-sided with the
/// cut-out faces, not with the plants (see [`is_shape_quad`]).
pub const PLANT_SHAPE: u32 = 6;

// Every plant model numbers below the shape model, within the three bits a vertex keeps
// for it. The match is exhaustive, so a new model does not compile until it is listed
// here, where the assert then checks it.
const PLANT_MODELS: [PlantModel; 6] = [PlantModel::Cross, PlantModel::Hash, PlantModel::Cap, PlantModel::Bush, PlantModel::Rock, PlantModel::Wall];
const fn listed(model: PlantModel) -> bool {
    match model {
        PlantModel::Cross | PlantModel::Hash | PlantModel::Cap | PlantModel::Bush | PlantModel::Rock | PlantModel::Wall => true,
    }
}
const _: () = {
    let mut i = 0;
    while i < PLANT_MODELS.len() {
        assert!(listed(PLANT_MODELS[i]) && (PLANT_MODELS[i] as u32) < PLANT_SHAPE);
        i += 1;
    }
    assert!(PLANT_SHAPE < 8);
};

/// Whether a quad, by its first vertex's word 0, is a quad of a block's shape: a plant face
/// of the shape model.
pub fn is_shape_quad(w0: u32) -> bool {
    let model = ((w0 >> PLANT_MODEL_SHIFT) & 3) | ((w0 >> PLANT_MODEL_HIGH_SHIFT) & 1) << 2;
    (w0 >> 18) & 7 >= PLANT_DIAGONAL && model == PLANT_SHAPE
}
const PLANT_HEIGHT_SHIFT: u32 = 14;
/// Ambient occlusion level (0..=3) of the faces between two leaves.
const INNER_LEAF_AO: u64 = 1;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct MeshBuffers {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshBuffers {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct ChunkMesh {
    /// Fully opaque faces. Kept apart from cut-out faces so the GPU can use early depth
    /// testing and hidden surface removal, which an alpha test in the shader disables.
    pub opaque: MeshBuffers,
    /// Alpha-tested faces (leaves, glass, plants).
    pub cutout: MeshBuffers,
    /// Alpha-tested, double-sided faces between two leaves. They cast no shadow: the
    /// crown's shell already does.
    pub inner: MeshBuffers,
    pub translucent: MeshBuffers,
    /// Leaf cards on the outside of crowns (bushy leaves), only worth drawing nearby.
    pub fringe: MeshBuffers,
    /// One seed per grass block open to the air, from which the renderer grows a tuft of
    /// blades, packed by `blade_seed`.
    pub blades: Vec<u32>,
    pub connectivity: Connectivity,
}

/// Per-cell flags of the padded chunk, derived from the registry.
#[derive(Clone, Copy, Default)]
struct Cell {
    block: BlockId,
    occludes: bool,
    /// Casts ambient occlusion: opaque blocks, and leaves, which darken the inside of a
    /// crown as in Minecraft.
    shades: bool,
    light: u8,
    /// The fluid the cell holds, plus one; 0 for none.
    fluid: u8,
}

impl Cell {
    /// Whether the face between two see-through cells is never seen: the same block, or
    /// the same fluid in any state.
    fn joins(&self, other: &Cell) -> bool {
        self.block == other.block || (self.fluid != 0 && self.fluid == other.fluid)
    }
}

/// Meshes chunk `cy` of the column whose light region is `region`, at a level of detail:
/// 1 is every block, and 2 or 4 merge that many blocks along x and z into one (see
/// `LodGrid`), for chunks far enough away that the coarser shapes don't show. Far chunks
/// grow no plants, fringes or grass blades.
pub fn mesh_chunk(registry: &BlockRegistry, region: &LitRegion, column: &Column, cy: i32, detail: i32) -> ChunkMesh {
    let _span = tracing::info_span!("mesh_chunk").entered();
    let chunk = &column.chunks[cy as usize];
    if chunk.as_uniform() == Some(BlockId::AIR) {
        return ChunkMesh { connectivity: Connectivity::ALL, ..Default::default() };
    }
    thread_local! {
        static SCRATCH: RefCell<Option<Box<Scratch>>> = const { RefCell::new(None) };
    }
    SCRATCH.with_borrow_mut(|scratch| {
        let scratch = scratch.get_or_insert_with(Default::default);
        scratch.reset(registry);
        mesh_with(scratch, registry, region, column, cy, detail)
    })
}

/// What the mesher reads of a block, packed small so that looking it up for each of a
/// chunk's 39,304 padded cells stays in cache (a `BlockInfo` spans several cache lines).
#[derive(Clone, Copy)]
struct Look {
    occludes: bool,
    render: RenderKind,
    /// The fluid the block holds, plus one; 0 for none.
    fluid: u8,
    /// An opaque block whose top is tinted like grass, which grows grass blades.
    grass_top: bool,
    /// A solid cube that does not hide what is behind it (glass, ice, leaves): water and
    /// ice show no face against one, as they show none against stone, or the side of a
    /// pool of water seen through a pane of glass stood in it is a wall of deep water.
    wall: bool,
    /// Texture layer and tint of each face, by `Face`.
    layers: [u16; 6],
    tints: [Tint; 6],
}

impl Look {
    fn of(registry: &BlockRegistry, block: BlockId) -> Self {
        let info = registry.info(block);
        Self {
            occludes: info.occludes,
            render: info.render,
            fluid: info.fluid.map_or(0, |f| f.fluid.0 + 1),
            grass_top: info.render == RenderKind::Opaque && info.tints[Face::PosY as usize] == Tint::Grass,
            wall: info.solid
                && !info.occludes
                && matches!(info.render, RenderKind::Cutout | RenderKind::Leaves | RenderKind::Translucent)
                && info.fluid.is_none(),
            layers: info.layers,
            tints: info.tints,
        }
    }
}

/// A chunk's working tables, kept per thread between chunks: allocating and clearing
/// fresh ones cost as much as filling them.
struct Scratch {
    cells: Vec<Cell>,
    occ: Axes,
    /// `occ` and the see-through solid cubes (`Look::wall`), which hide translucent faces.
    walled: Axes,
    opaque: Axes,
    cutout: Axes,
    translucent: Axes,
    joins: Joins,
    /// Leaf cells, as bit columns along x.
    leaves: Box<[[u64; P]; P]>,
    /// Every block's look, and the registry they were read from (its address and size).
    looks: Vec<Look>,
    registry: (usize, usize),
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            occ: Axes::default(),
            walled: Axes::default(),
            opaque: Axes::default(),
            cutout: Axes::default(),
            translucent: Axes::default(),
            joins: Joins::default(),
            leaves: Box::new([[0; P]; P]),
            looks: Vec::new(),
            registry: (0, 0),
        }
    }
}

impl Scratch {
    /// Clears the bit tables; every cell is written before it is read.
    fn reset(&mut self, registry: &BlockRegistry) {
        self.cells.resize(P * P * P, Cell::default());
        for axes in [&mut self.occ, &mut self.walled, &mut self.opaque, &mut self.cutout, &mut self.translucent, &mut self.joins.hidden, &mut self.joins.single] {
            axes.clear();
        }
        *self.leaves = [[0; P]; P];
        let key = (registry as *const BlockRegistry as usize, registry.len());
        if self.registry != key {
            self.registry = key;
            self.looks = (0..registry.len()).map(|i| Look::of(registry, BlockId(i as u16))).collect();
        }
    }
}

fn mesh_with(scratch: &mut Scratch, registry: &BlockRegistry, region: &LitRegion, column: &Column, cy: i32, detail: i32) -> ChunkMesh {
    let Scratch { cells, occ, walled, opaque, cutout, translucent, joins, leaves, looks, .. } = scratch;
    let origin = IVec3::new(-1, cy * CHUNK_SIZE - 1, -1);
    let mut plants = Vec::new();
    let mut models = Vec::new();
    let mut grids: Vec<LodGrid> =
        LEVELS.into_iter().filter(|&level| level > 1).map(|level| LodGrid::new(looks, region, origin, level)).collect();
    let (mut occluding, mut grass) = (0, false);
    // A row of nothing but air, in full detail, leaves only its light: no face, and no
    // bit in the tables (half of a chunk at the surface).
    let air = looks[BlockId::AIR.0 as usize];
    let plain_air = detail == 1 && !air.occludes && air.render == RenderKind::Invisible && air.fluid == 0 && !air.wall && !air.grass_top;
    for y in 0..P {
        let world_y = origin.y + y as i32;
        for z in 0..P {
            // A row of the region; below the world is an invisible wall that hides bottom
            // faces, and above it, open sky.
            let row = (0..WORLD_HEIGHT).contains(&world_y).then(|| region.row(origin.x, world_y, origin.z + z as i32, P));
            if plain_air
                && let Some((blocks, lights)) = row
                && blocks.iter().all(|block| block.is_air())
            {
                for (x, &light) in lights.iter().enumerate() {
                    cells[pidx(x, y, z)] = Cell { block: BlockId::AIR, occludes: false, shades: false, light, fluid: 0 };
                }
                continue;
            }
            for x in 0..P {
                let pos = origin + IVec3::new(x as i32, y as i32, z as i32);
                let (block, light) = match row {
                    Some(_) if detail > 1 => {
                        grids.iter_mut().find(|grid| grid.detail == detail).expect("a grid for every coarser level").get(pos)
                    }
                    Some((blocks, lights)) => (blocks[x], lights[x]),
                    None if world_y < 0 => (BlockId::AIR, 0),
                    None => (BlockId::AIR, MAX_LIGHT << 4),
                };
                let look = looks[block.0 as usize];
                let (mut occludes, mut render) = if world_y < 0 { (true, RenderKind::Invisible) } else { (look.occludes, look.render) };
                // Far away a crown is a solid mass: its leaves are drawn as opaque blocks,
                // so the faces between them, a third of a distant forest's, go, and the
                // rest skip the alpha test and hide what is behind them. A shape (a slab,
                // a barrel) is too small there to make out, and is drawn as its block.
                if detail > 1 && matches!(render, RenderKind::Leaves | RenderKind::Model) {
                    (occludes, render) = (true, RenderKind::Opaque);
                }
                let mut shades = occludes || render == RenderKind::Leaves;
                // A chunk beside this one may be drawn at another level of detail, so a
                // border face is only hidden by a block every level shows there.
                let side = x == 0 || x == P - 1 || z == 0 || z == P - 1;
                if side && occludes && let Some((blocks, _)) = row {
                    for level in LEVELS.into_iter().filter(|&level| level != detail) {
                        let other = match grids.iter_mut().find(|grid| grid.detail == level) {
                            Some(grid) => grid.block(pos),
                            None => blocks[x],
                        };
                        let other = looks[other.0 as usize];
                        occludes &= other.occludes;
                        shades &= other.occludes || other.render == RenderKind::Leaves;
                    }
                }
                let cell = Cell { block, occludes, shades, light, fluid: look.fluid };
                cells[pidx(x, y, z)] = cell;
                if occludes {
                    occ.set(x, y, z);
                    walled.set(x, y, z);
                    occluding += 1;
                } else if look.wall {
                    walled.set(x, y, z);
                }
                grass |= look.grass_top;
                match render {
                    RenderKind::Opaque => opaque.set(x, y, z),
                    RenderKind::Cutout => cutout.set(x, y, z),
                    RenderKind::Leaves => {
                        cutout.set(x, y, z);
                        leaves[y][z] |= 1 << x;
                    }
                    RenderKind::Translucent => translucent.set(x, y, z),
                    RenderKind::Plant if [x, y, z].iter().all(|c| (1..=S).contains(c)) => plants.push(pidx(x, y, z)),
                    RenderKind::Model if [x, y, z].iter().all(|c| (1..=S).contains(c)) => models.push(pidx(x, y, z)),
                    RenderKind::Plant | RenderKind::Model | RenderKind::Invisible => {}
                }
                if matches!(render, RenderKind::Cutout | RenderKind::Translucent) {
                    // Cells with a lower coordinate are already filled in.
                    let hidden = &mut joins.hidden;
                    if x > 0 && cells[pidx(x - 1, y, z)].joins(&cell) {
                        hidden.x[y][z] |= 1 << (x - 1);
                    }
                    if y > 0 && cells[pidx(x, y - 1, z)].joins(&cell) {
                        hidden.y[z][x] |= 1 << (y - 1);
                    }
                    if z > 0 && cells[pidx(x, y, z - 1)].joins(&cell) {
                        hidden.z[y][x] |= 1 << (z - 1);
                    }
                }
            }
        }
    }
    // Buried in solid blocks on every side: nothing to draw.
    if occluding == P * P * P {
        return ChunkMesh { connectivity: Connectivity::NONE, ..Default::default() };
    }
    let (cells, occ, walled, opaque, cutout, translucent, joins, leaves) =
        (&*cells, &*occ, &*walled, &*opaque, &*cutout, &*translucent, joins, &**leaves);

    join_leaves(leaves, occ, joins);

    let mut mesher =
        Mesher { registry, looks, region, cells, chunk_y: cy * CHUNK_SIZE, detail, open_water: RefCell::default(), planes: [[0; S]; S], keys: [[0; S]; S] };
    let mut mesh = ChunkMesh { connectivity: Connectivity::compute(|x, y, z| cells[pidx(x + 1, y + 1, z + 1)].occludes), ..Default::default() };
    for face in Face::ALL {
        mesher.mesh_faces(face, occ, opaque, joins, &mut mesh.opaque);
        mesher.mesh_faces(face, occ, cutout, joins, &mut mesh.cutout);
        mesher.mesh_faces(face, walled, translucent, joins, &mut mesh.translucent);
    }
    mesh.inner = split_inner(&mut mesh.cutout);
    // Shapes' quads after the cut-out cube faces they are drawn with, before the plants.
    for i in models {
        let local = IVec3::new((i % P) as i32, (i / (P * P)) as i32, (i / P % P) as i32) - 1;
        emit_model(&mut mesh.cutout, registry, &cells, i, local);
    }
    for i in plants {
        let local = IVec3::new((i % P) as i32, (i / (P * P)) as i32, (i / P % P) as i32) - 1;
        emit_plant(&mut mesh.cutout, registry, column, &cells[i], local, cy * CHUNK_SIZE);
    }
    if detail > 1 {
        return mesh;
    }
    if grass {
        mesh.blades = grass_tops(looks, cells);
    }
    for (y, plane) in leaves.iter().enumerate().take(S + 1).skip(1) {
        for (z, &bits) in plane.iter().enumerate().take(S + 1).skip(1) {
            let mut row = bits & !1 & !(1 << (S + 1));
            while row != 0 {
                let x = row.trailing_zeros() as usize;
                row &= row - 1;
                emit_fringe(&mut mesh.fringe, registry, cells, IVec3::new(x as i32, y as i32, z as i32));
            }
        }
    }
    mesh
}

/// The levels of detail chunks are meshed at.
pub const LEVELS: [i32; 3] = [1, 2, 4];

/// What a chunk meshed at a coarser level of detail (see `mesh_chunk`) shows over its
/// padded cells, worked out once per group of columns and layer.
///
/// A cell shows the most common solid block (anything drawn but plants) in its layer of
/// the `detail`×`detail` group of columns around it, aligned to the world, when at least
/// half of the group is solid. Otherwise it shows a pillar running through the layer (a
/// tree trunk, which would leave its crown floating), or air. It sees the brightest light
/// of that layer, since the open cells a face looks into may be blocks in full detail.
/// Heights stay exact, so far chunks meet near ones without steps in the ground or the
/// water.
struct LodGrid<'a> {
    looks: &'a [Look],
    region: &'a LitRegion,
    detail: i32,
    /// The grid's first group and first layer, and its width in groups.
    first: IVec2,
    bottom: i32,
    width: i32,
    blocks: Vec<Option<BlockId>>,
    lights: Vec<Option<u8>>,
}

impl<'a> LodGrid<'a> {
    /// A grid over the padded chunk whose first cell is `origin`.
    fn new(looks: &'a [Look], region: &'a LitRegion, origin: IVec3, detail: i32) -> Self {
        let group = |x: i32, z: i32| IVec2::new(x.div_euclid(detail), z.div_euclid(detail));
        let first = group(origin.x, origin.z);
        let width = (group(origin.x + P as i32 - 1, origin.z + P as i32 - 1) - first).max_element() + 1;
        let size = (width * width) as usize * P;
        Self { looks, region, detail, first, bottom: origin.y, width, blocks: vec![None; size], lights: vec![None; size] }
    }

    fn get(&mut self, pos: IVec3) -> (BlockId, u8) {
        (self.block(pos), self.light(pos))
    }

    /// The cell's group (its index) and the group's first block in the layer.
    fn group(&self, pos: IVec3) -> (usize, IVec3) {
        let group = IVec2::new(pos.x.div_euclid(self.detail), pos.z.div_euclid(self.detail));
        let local = group - self.first;
        let i = ((pos.y - self.bottom) * self.width * self.width + local.y * self.width + local.x) as usize;
        (i, IVec3::new(group.x * self.detail, pos.y, group.y * self.detail))
    }

    fn block(&mut self, pos: IVec3) -> BlockId {
        let (i, corner) = self.group(pos);
        if let Some(block) = self.blocks[i] {
            return block;
        }
        // At most 16 distinct blocks in a group of 4×4.
        let mut counts = [(BlockId::AIR, 0); 16];
        let (mut distinct, mut solid) = (0, 0);
        for dz in 0..self.detail {
            for dx in 0..self.detail {
                let Some(block) = self.region.block(corner + IVec3::new(dx, 0, dz)) else { continue };
                if block.is_air() || matches!(self.looks[block.0 as usize].render, RenderKind::Plant | RenderKind::Invisible) {
                    continue;
                }
                solid += 1;
                match counts[..distinct].iter_mut().find(|(b, _)| *b == block) {
                    Some((_, n)) => *n += 1,
                    None => {
                        counts[distinct] = (block, 1);
                        distinct += 1;
                    }
                }
            }
        }
        let block = if solid * 2 < self.detail * self.detail {
            self.pillar(corner).unwrap_or(BlockId::AIR)
        } else {
            counts[..distinct].iter().max_by_key(|&&(_, n)| n).map_or(BlockId::AIR, |&(block, _)| block)
        };
        self.blocks[i] = Some(block);
        block
    }

    fn light(&mut self, pos: IVec3) -> u8 {
        let (i, corner) = self.group(pos);
        if let Some(light) = self.lights[i] {
            return light;
        }
        let (mut sky, mut torch) = (0, 0);
        for dz in 0..self.detail {
            for dx in 0..self.detail {
                let light = self.region.light(corner + IVec3::new(dx, 0, dz));
                sky = sky.max(light >> 4);
                torch = torch.max(light & 0xF);
            }
        }
        let light = sky << 4 | torch;
        self.lights[i] = Some(light);
        light
    }

    /// An opaque block of the group's layer at `corner` with the same block above and below.
    fn pillar(&self, corner: IVec3) -> Option<BlockId> {
        (0..self.detail * self.detail).find_map(|i| {
            let at = corner + IVec3::new(i % self.detail, 0, i / self.detail);
            let block = self.region.block(at)?;
            let opaque = self.looks[block.0 as usize].render == RenderKind::Opaque;
            let runs = |dy: i32| self.region.block(at + IVec3::Y * dy) == Some(block);
            (opaque && runs(1) && runs(-1)).then_some(block)
        })
    }
}

/// Where two cells meet, as bit columns along each axis: bit i pairs cell i with cell i + 1.
#[derive(Default)]
struct Joins {
    /// Neither face is drawn: the same see-through block on both sides (water, glass), whose
    /// shared face is never seen, or two leaves buried in foliage.
    hidden: Axes,
    /// One double-sided face is drawn, towards +axis: two leaves.
    single: Axes,
}

/// Faces between leaves are drawn, so crowns show the same depth through their holes from
/// every angle; where two leaves meet, one double-sided face serves both sides. Between two
/// leaves buried in foliage (or solid blocks) on every side there is none: those are only
/// seen through holes in two layers of leaves, where the faces further in still close the
/// view, and they make up much of a big crown.
fn join_leaves(leaves: &[[u64; P]; P], occ: &Axes, joins: &mut Joins) {
    let filled = |y: usize, z: usize| leaves[y][z] | occ.x[y][z];
    let mut buried = [[0u64; P]; P];
    for y in 1..P - 1 {
        for z in 1..P - 1 {
            let row = filled(y, z);
            buried[y][z] = leaves[y][z]
                & row << 1
                & row >> 1
                & filled(y - 1, z)
                & filled(y + 1, z)
                & filled(y, z - 1)
                & filled(y, z + 1);
        }
    }
    for (cells, pairs) in [(&buried, &mut joins.hidden), (leaves, &mut joins.single)] {
        for y in 0..P {
            for z in 0..P {
                let here = cells[y][z];
                pairs.x[y][z] |= here & here >> 1;
                if y + 1 < P {
                    for x in bits(here & cells[y + 1][z]) {
                        pairs.y[z][x] |= 1 << y;
                    }
                }
                if z + 1 < P {
                    for x in bits(here & cells[y][z + 1]) {
                        pairs.z[y][x] |= 1 << z;
                    }
                }
            }
        }
    }
}

/// Indices of the set bits.
fn bits(mut mask: u64) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        let i = mask.trailing_zeros() as usize;
        mask &= mask.wrapping_sub(1);
        (i < 64).then_some(i)
    })
}

/// Moves the faces flagged inner (between two leaves) out of `cutout`, which holds only
/// greedy quads at this point.
fn split_inner(cutout: &mut MeshBuffers) -> MeshBuffers {
    let (mut shell, mut inner) = (MeshBuffers::default(), MeshBuffers::default());
    for (quad, (vertices, indices)) in cutout.vertices.chunks(4).zip(cutout.indices.chunks(6)).enumerate() {
        let out = if vertices[0][0] & FLAG_INNER != 0 { &mut inner } else { &mut shell };
        let base = out.vertices.len() as u32;
        out.vertices.extend_from_slice(vertices);
        out.indices.extend(indices.iter().map(|&i| i - 4 * quad as u32 + base));
    }
    *cutout = shell;
    inner
}

#[inline]
fn pidx(x: usize, y: usize, z: usize) -> usize {
    (y * P + z) * P + x
}

/// Bit columns of the padded chunk along X, Y and Z.
struct Axes {
    /// Indexed `[y][z]`, bits over x.
    x: Box<[[u64; P]; P]>,
    /// Indexed `[z][x]`, bits over y.
    y: Box<[[u64; P]; P]>,
    /// Indexed `[y][x]`, bits over z.
    z: Box<[[u64; P]; P]>,
}

impl Default for Axes {
    fn default() -> Self {
        Self { x: Box::new([[0; P]; P]), y: Box::new([[0; P]; P]), z: Box::new([[0; P]; P]) }
    }
}

impl Axes {
    fn clear(&mut self) {
        for bits in [&mut self.x, &mut self.y, &mut self.z] {
            **bits = [[0; P]; P];
        }
    }

    fn set(&mut self, x: usize, y: usize, z: usize) {
        self.x[y][z] |= 1 << x;
        self.y[z][x] |= 1 << y;
        self.z[y][x] |= 1 << z;
    }

    /// The bit columns along axis 0 (x), 1 (y) or 2 (z).
    fn along(&self, axis: usize) -> &[[u64; P]; P] {
        match axis {
            0 => &self.x,
            1 => &self.y,
            _ => &self.z,
        }
    }
}

/// How a face direction maps onto (slice, row, column) of its greedy planes.
struct FaceAxes {
    /// Axis index (0 = x, 1 = y, 2 = z) of the normal, rows and columns.
    normal: usize,
    row: usize,
    col: usize,
    positive: bool,
    /// Whether (col, row) winds counter-clockwise when seen from outside.
    ccw: bool,
}

const fn face_axes(face: Face) -> FaceAxes {
    match face {
        Face::PosX => FaceAxes { normal: 0, row: 1, col: 2, positive: true, ccw: false },
        Face::NegX => FaceAxes { normal: 0, row: 1, col: 2, positive: false, ccw: true },
        Face::PosY => FaceAxes { normal: 1, row: 2, col: 0, positive: true, ccw: false },
        Face::NegY => FaceAxes { normal: 1, row: 2, col: 0, positive: false, ccw: true },
        Face::PosZ => FaceAxes { normal: 2, row: 1, col: 0, positive: true, ccw: true },
        Face::NegZ => FaceAxes { normal: 2, row: 1, col: 0, positive: false, ccw: false },
    }
}

fn unit(axis: usize) -> IVec3 {
    let mut v = IVec3::ZERO;
    v[axis] = 1;
    v
}

struct Mesher<'a> {
    registry: &'a BlockRegistry,
    looks: &'a [Look],
    /// Blocks around the column too, for fluids, whose surface and flow depend on cells
    /// a few blocks away.
    region: &'a LitRegion,
    cells: &'a [Cell],
    /// Height of the chunk's bottom in the column.
    chunk_y: i32,
    /// Blocks merged along x and z (see `mesh_chunk`).
    detail: i32,
    /// Open water surface around the column at each height water has a surface at, as
    /// summed-area tables (see `open_water`).
    open_water: RefCell<Vec<(i32, Vec<u32>)>>,
    /// `planes[slice][row]`: bit `col` set where a face is visible.
    planes: [[u32; S]; S],
    keys: [[u128; S]; S],
}

impl Mesher<'_> {
    /// Meshes the faces of `visible` cells along `face`. A face is hidden by an occluding
    /// neighbour, or where `joins` says so.
    fn mesh_faces(&mut self, face: Face, occ: &Axes, visible: &Axes, joins: &Joins, out: &mut MeshBuffers) {
        let fa = face_axes(face);
        self.planes = [[0; S]; S];
        let mut any = false;

        // Cull faces a whole column of cells at a time with bit operations.
        let (occ_cols, vis_cols) = (occ.along(fa.normal), visible.along(fa.normal));
        let (hidden_cols, single_cols) = (joins.hidden.along(fa.normal), joins.single.along(fa.normal));
        let neighbour = |bits: u64| if fa.positive { bits >> 1 } else { bits << 1 };
        for a in 1..=S {
            for b in 1..=S {
                // Bit i of `joins` pairs cell i with cell i + 1; single faces are drawn
                // towards +axis only.
                let joined = if fa.positive {
                    hidden_cols[a][b]
                } else {
                    (hidden_cols[a][b] | single_cols[a][b]) << 1
                };
                let hidden = neighbour(occ_cols[a][b]) | joined;
                let faces = ((vis_cols[a][b] & !hidden) >> 1) as u32;
                if faces == 0 {
                    continue;
                }
                any = true;
                // `Axes` stores the two non-normal coordinates in (row, col) order.
                let (row, col) = (a - 1, b - 1);
                let mut bits = faces;
                while bits != 0 {
                    let slice = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    if self.detail > 1 && self.unseen_from_afar(face, &fa, slice, row, col) {
                        continue;
                    }
                    self.planes[slice][row] |= 1 << col;
                }
            }
        }
        if !any {
            return;
        }

        for slice in 0..S {
            if self.planes[slice].iter().all(|&r| r == 0) {
                continue;
            }
            for row in 0..S {
                let mut bits = self.planes[slice][row];
                while bits != 0 {
                    let col = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    self.keys[row][col] = self.face_key(face, &fa, slice, row, col);
                }
            }
            self.greedy(face, &fa, slice, out);
        }
    }

    /// Interior cell coordinates of a (slice, row, col) face position.
    /// Whether a face of a chunk merged for distance looks into air no daylight reaches:
    /// cave walls, floors and ceilings at least fifteen blocks from any opening, which no
    /// one far away can see into. They are most of a far chunk's faces that point down
    /// or sideways. Faces under water stay, as the water above shows them.
    fn unseen_from_afar(&self, face: Face, fa: &FaceAxes, slice: usize, row: usize, col: usize) -> bool {
        let front = self.padded(Self::cell(fa, slice, row, col) + face.normal());
        front.light >> 4 == 0 && front.fluid == 0
    }

    fn cell(fa: &FaceAxes, slice: usize, row: usize, col: usize) -> IVec3 {
        let mut p = IVec3::ZERO;
        p[fa.normal] = slice as i32;
        p[fa.row] = row as i32;
        p[fa.col] = col as i32;
        p
    }

    fn padded(&self, p: IVec3) -> &Cell {
        &self.cells[pidx((p.x + 1) as usize, (p.y + 1) as usize, (p.z + 1) as usize)]
    }

    /// Everything that must match for two faces to merge:
    /// layer:12 tint:2 | ao 4×2 | light 4×8 | flags:2. Biome colours are looked up per
    /// block by the renderer, so they never split a face.
    fn face_key(&self, face: Face, fa: &FaceAxes, slice: usize, row: usize, col: usize) -> u128 {
        let pos = Self::cell(fa, slice, row, col);
        let info = self.looks[self.padded(pos).block.0 as usize];
        let (inner, backed) = match info.render {
            // Shadows from a crown only need its shell: faces between two leaves cast none,
            // and shell faces with more leaves behind them cast a solid one. Crowns shade as
            // densely as if every face cast, for the cost of the shell.
            RenderKind::Leaves => {
                let leaf = |p: IVec3| self.looks[self.padded(p).block.0 as usize].render == RenderKind::Leaves;
                (leaf(pos + face.normal()), leaf(pos - face.normal()))
            }
            // Water under ice is covered: still, and reflecting nothing.
            RenderKind::Translucent => (!self.padded(pos + face.normal()).block.is_air(), false),
            _ => (false, false),
        };
        let tint = info.tints[face as usize];
        let surface = info.layers[face as usize] as u64 | (tint as u64) << 12;

        let front = pos + face.normal();
        let (du, dv) = (unit(fa.col), unit(fa.row));
        let center = self.padded(front);
        // Faces under water are lit as the water's surface above them is: the water shader
        // dims what lies beneath by the path light really takes through the water, which
        // the light engine's falloff per block of water would only darken a second time.
        let surface_sky = self.sky_above_water(front);
        let mut ao = 0u64;
        let mut light = 0u64;
        for (corner, (su, sv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
            let side_u = self.padded(front + du * su);
            let side_v = self.padded(front + dv * sv);
            let diagonal = self.padded(front + du * su + dv * sv);
            let corner_ao = if side_u.shades && side_v.shades {
                0
            } else {
                3 - (side_u.shades as u64 + side_v.shades as u64 + diagonal.shades as u64)
            };
            ao |= corner_ao << (corner * 2);

            // Smooth light: average of the open cells touching this vertex. The diagonal
            // is unreachable when both sides are walls.
            let both_sides = side_u.occludes && side_v.occludes;
            let (mut sky, mut block, mut count) = ((center.light >> 4) as u32, (center.light & 0xF) as u32, 1);
            for (cell, open) in [(side_u, !side_u.occludes), (side_v, !side_v.occludes), (diagonal, !diagonal.occludes && !both_sides)] {
                if open {
                    sky += (cell.light >> 4) as u32;
                    block += (cell.light & 0xF) as u32;
                    count += 1;
                }
            }
            let sky = ((sky + count / 2) / count).max(surface_sky);
            let packed = (sky << 4) | ((block + count / 2) / count);
            light |= (packed as u64) << (corner * 8);
        }
        // Faces of see-through fluids carry the fluid's surface and flow in place of ambient
        // occlusion, which water's shading has no use for (see the module docs).
        let mut fluid = 0u128;
        let mut falling = false;
        let state = (info.render == RenderKind::Translucent && info.fluid != 0).then(|| self.registry.fluid(self.padded(pos).block)).flatten();
        let mut open = 0u128;
        if let Some(state) = state {
            ao = 0;
            if face == Face::PosY {
                let (du, dv) = (unit(fa.col), unit(fa.row));
                for (corner, (su, sv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
                    let o = du * su + dv * sv;
                    let at = self.in_column(pos) + (o + 1) / 2;
                    open |= (self.open_water(state, at.y, at.x, at.z) as u128) << (corner * 4);
                }
            }
            // Chunks merged for distance keep whole, still blocks: the flow's detail would
            // not show.
            if self.detail == 1 {
                for (corner, (drop, speed, direction)) in self.fluid_corners(face, fa, pos, state).into_iter().enumerate() {
                    ao |= (speed as u64) << (corner * 2);
                    fluid |= (drop as u128) << (corner * 3) | (direction as u128) << (12 + corner * 4);
                }
                // Falling water, and the side over which water pours down an edge (open in
                // front, nothing solid beneath that), streams downwards; other sides are the
                // edge of a surface.
                let pours = || {
                    let front = self.in_column(pos + face.normal());
                    let open = |p: IVec3| self.region.block(p).is_some_and(|b| !self.registry.info(b).solid);
                    face.normal().y == 0 && open(front) && open(front - IVec3::Y)
                };
                falling = state.falling || pours();
            }
        }
        if self.detail > 1 && state.is_none() {
            // Far away a face is a few pixels: lit evenly, without occlusion at its
            // corners, so neighbours merge into far fewer quads.
            let even = center.light.max((surface_sky as u8) << 4) as u64;
            light = even * 0x0101_0101;
            ao = 0b1111_1111;
        }
        if inner && info.render == RenderKind::Leaves {
            // One double-sided face serves both leaves: lit evenly by both, and shaded as
            // the inside of a crown. Even light also lets these faces merge widely.
            let (a, b) = (self.padded(pos).light as u64, center.light as u64);
            let average = |shift: u32| ((a >> shift & 15) + (b >> shift & 15)).div_ceil(2) << shift;
            light = (average(4) | average(0)) * 0x0101_0101;
            ao = INNER_LEAF_AO * 0b0101_0101;
        }
        (surface | ao << 24 | light << 32) as u128
            | (inner as u128) << 64
            | ((backed || falling) as u128) << 65
            | fluid << 66
            | open << 94
    }

    /// How much of the surface within `OPEN_WATER_REACH` of the corner at column-relative
    /// (x, z) is open water of the fluid at height `y` (water there, no solid or more of it
    /// above), in 15ths. Each height's table is built once per chunk.
    fn open_water(&self, state: FluidState, y: i32, x: i32, z: i32) -> u8 {
        const W: i32 = CHUNK_SIZE + 2 * LIGHT_PAD;
        let mut tables = self.open_water.borrow_mut();
        let table = match tables.iter().position(|(level, _)| *level == y) {
            Some(i) => &tables[i].1,
            None => {
                let open = |x: i32, z: i32| {
                    let here = self.region.block(IVec3::new(x, y, z)).and_then(|b| self.registry.fluid(b));
                    let above = self.region.block(IVec3::new(x, y + 1, z));
                    here.is_some_and(|s| s.fluid == state.fluid)
                        && above.is_some_and(|b| self.registry.fluid(b).is_none_or(|s| s.fluid != state.fluid) && !self.registry.info(b).solid)
                };
                // sums[(z + 1) * (W + 1) + x + 1]: open cells in [−PAD, x] × [−PAD, z].
                let mut sums = vec![0u32; ((W + 1) * (W + 1)) as usize];
                for j in 0..W {
                    let mut row = 0;
                    for i in 0..W {
                        row += open(i - LIGHT_PAD, j - LIGHT_PAD) as u32;
                        sums[((j + 1) * (W + 1) + i + 1) as usize] = sums[(j * (W + 1) + i + 1) as usize] + row;
                    }
                }
                tables.push((y, sums));
                &tables.last().expect("just pushed").1
            }
        };
        let at = |x: i32, z: i32| {
            let (i, j) = ((x + LIGHT_PAD).clamp(0, W), (z + LIGHT_PAD).clamp(0, W));
            table[(j * (W + 1) + i) as usize] as i32
        };
        let (x0, x1, z0, z1) = (x - OPEN_WATER_REACH, x + OPEN_WATER_REACH, z - OPEN_WATER_REACH, z + OPEN_WATER_REACH);
        let count = at(x1, z1) - at(x0, z1) - at(x1, z0) + at(x0, z0);
        let area = (2 * OPEN_WATER_REACH * 2 * OPEN_WATER_REACH) as f32;
        (count as f32 / area * 15.0).round() as u8
    }

    /// The sky light just above the surface of the see-through fluid filling the padded
    /// cell `front`, or 0 when `front` holds none or the fluid is roofed over.
    fn sky_above_water(&self, front: IVec3) -> u32 {
        let cell = self.padded(front);
        if cell.fluid == 0 || self.looks[cell.block.0 as usize].render != RenderKind::Translucent {
            return 0;
        }
        let mut p = self.in_column(front);
        for _ in 0..WORLD_HEIGHT {
            p.y += 1;
            match self.region.block(p) {
                Some(b) if self.looks[b.0 as usize].fluid == cell.fluid => {}
                Some(b) if !self.looks[b.0 as usize].occludes => return (self.region.light(p) >> 4) as u32,
                _ => return 0,
            }
        }
        0
    }

    /// Column-relative position of a cell of the chunk.
    fn in_column(&self, p: IVec3) -> IVec3 {
        IVec3::new(p.x, self.chunk_y + p.y, p.z)
    }

    /// A fluid face's corners, in the face's corner order: how far the surface lies below
    /// the block's top there (eighths), and the cell's flow's speed level and direction
    /// (16ths). Only the top face and the top edge of the sides drop, and not under more of
    /// the same fluid, which fills the cell.
    fn fluid_corners(&self, face: Face, fa: &FaceAxes, pos: IVec3, state: FluidState) -> [(u8, u8, u8); 4] {
        let block = |p: IVec3| self.region.block(p);
        let at = self.in_column(pos);
        let same = |p: IVec3| block(p).and_then(|b| self.registry.fluid(b)).is_some_and(|s| s.fluid == state.fluid);
        let heights = if same(at + IVec3::Y) { [1.0; 4] } else { fluid::corner_heights(self.registry, block, at) };
        // The cell's flow. Where water falls beside it, or into the pool beside it, its
        // surface churns away from the fall.
        let falling = |p: IVec3| block(p).and_then(|b| self.registry.fluid(b)).is_some_and(|s| s.fluid == state.fluid && s.falling);
        let churn: Vec2 = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
            .into_iter()
            .filter(|&side| face == Face::PosY && (falling(at + side) || falling(at + side + IVec3::Y)))
            .map(|side| -Vec2::new(side.x as f32, side.z as f32) * FLOW_SPEEDS[2])
            .sum();
        let flow = fluid::flow(self.registry, block, at) + churn;
        let (du, dv) = (unit(fa.col), unit(fa.row));
        [(-1, -1), (1, -1), (1, 1), (-1, 1)].map(|(su, sv)| {
            // Which way from the cell's centre the corner lies.
            let o = face.normal() + du * su + dv * sv;
            let height = heights[match (o.x, o.z) {
                (-1, -1) => 0,
                (1, -1) => 1,
                (1, 1) => 2,
                _ => 3,
            }];
            let drop = if o.y > 0 { ((1.0 - height) * 8.0).round().clamp(0.0, 7.0) as u8 } else { 0 };
            // The cell's own flow at every corner: its streaks turn as a whole, so where the
            // flow changes, the pattern changes at the edge of the block, as Minecraft's
            // flowing water does, rather than along a diagonal through it.
            let (speed, direction) = quantise_flow(flow);
            (drop, speed, direction)
        })
    }

    fn greedy(&mut self, face: Face, fa: &FaceAxes, slice: usize, out: &mut MeshBuffers) {
        let plane = &mut self.planes[slice];
        for row in 0..S {
            while plane[row] != 0 {
                let col = plane[row].trailing_zeros() as usize;
                let key = self.keys[row][col];

                let mut width = 1;
                while col + width < S && plane[row] & (1 << (col + width)) != 0 && self.keys[row][col + width] == key {
                    width += 1;
                }
                let span = if width == 32 { u32::MAX } else { ((1u32 << width) - 1) << col };

                let mut height = 1;
                'grow: while row + height < S {
                    let next = row + height;
                    if plane[next] & span != span {
                        break;
                    }
                    for c in col..col + width {
                        if self.keys[next][c] != key {
                            break 'grow;
                        }
                    }
                    height += 1;
                }
                for bits in &mut plane[row..row + height] {
                    *bits &= !span;
                }

                emit_quad(out, face, fa, slice, row, col, width, height, key);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_quad(
    out: &mut MeshBuffers,
    face: Face,
    fa: &FaceAxes,
    slice: usize,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    key: u128,
) {
    let plane = slice as i32 + fa.positive as i32;
    let corners = [(col, row), (col + width, row), (col + width, row + height), (col, row + height)];
    let surface = (key & 0xFF_FFFF) as u32;
    let flags = (key >> 64) as u32 & 3;
    let base = out.vertices.len() as u32;
    let mut ao = [0u32; 4];
    for (i, (c, r)) in corners.into_iter().enumerate() {
        let mut p = IVec3::ZERO;
        p[fa.normal] = plane;
        p[fa.row] = r as i32;
        p[fa.col] = c as i32;
        ao[i] = ((key >> (24 + i * 2)) & 3) as u32;
        let light = ((key >> (32 + i * 8)) & 0xFF) as u32;
        let drop = ((key >> (66 + i * 3)) & 7) as u32;
        let direction = ((key >> (78 + i * 4)) & 15) as u32;
        let open = ((key >> (94 + i * 4)) & 15) as u32;
        let w0 = p.x as u32
            | (p.y as u32) << 6
            | (p.z as u32) << 12
            | (face as u32) << 18
            | ao[i] << 21
            | flags << 23
            | drop << SURFACE_DROP_SHIFT
            | direction << FLOW_DIRECTION_SHIFT;
        out.vertices.push([w0, surface | open << OPEN_WATER_SHIFT | light << 24]);
    }

    // Split along the darker diagonal so AO interpolates without anisotropic streaks.
    let flip = ao[0] + ao[2] > ao[1] + ao[3];
    let order: [u32; 6] = match (fa.ccw, flip) {
        (true, false) => [0, 1, 2, 0, 2, 3],
        (true, true) => [1, 2, 3, 1, 3, 0],
        (false, false) => [0, 2, 1, 0, 3, 2],
        (false, true) => [1, 3, 2, 1, 0, 3],
    };
    out.indices.extend(order.map(|i| base + i));
}

/// A plant's quads (two crossed through the cell's diagonals, or its model's own), drawn
/// from both sides, lit by the plant's own cell and without ambient occlusion.
fn emit_plant(out: &mut MeshBuffers, registry: &BlockRegistry, column: &Column, cell: &Cell, pos: IVec3, chunk_y: i32) {
    let info = registry.info(cell.block);
    // How many plant cells this one stands on (tall grass, sugar cane), so the shader can
    // shape the whole stack as one plant.
    let is_plant = |y: i32| y >= 0 && registry.info(column.block(IVec3::new(pos.x, y, pos.z))).render == RenderKind::Plant;
    // A plant on a wall stands on nothing: these bits say which way it faces instead. One
    // hanging from above is drawn from the top of its cell, alone.
    let hangs = info.support == Support::Above;
    let stack = match (info.model, info.facing) {
        (PlantModel::Wall, Some(facing)) => facing as u32,
        _ if hangs => 0,
        _ => (1..=3).take_while(|&d| is_plant(chunk_y + pos.y - d)).count() as u32,
    };
    let tint = info.tints[Face::PosX as usize];
    let height = ((info.plant_height * 8.0).round() as u32).clamp(1, 31) << PLANT_HEIGHT_SHIFT;
    let w1 = |face: Face| info.layers[face as usize] as u32 | (tint as u32) << 12 | height | (cell.light as u32) << 24;
    // The model's quads (see voxel.wgsl `model_across`). Each is packed as a diagonal quad,
    // the even ones along one diagonal and the odd along the other, which gives the shader
    // the cell and the texture's run.
    let pieces = match info.model {
        PlantModel::Cross | PlantModel::Wall => 2,
        // A stem of two quads and a cap of six: crown, four sides, gills.
        PlantModel::Cap => 8,
        PlantModel::Hash | PlantModel::Bush => 4,
        // A lump of five quads (top and four sides) and a chip of three.
        PlantModel::Rock => 8,
    };
    for piece in 0..pieces {
        let (face, (x0, z0), (x1, z1)) =
            if piece % 2 == 0 { (PLANT_DIAGONAL, (0, 0), (1, 1)) } else { (PLANT_ANTIDIAGONAL, (1, 0), (0, 1)) };
        // A mushroom's stem is drawn with the block's side texture, its crown and sides with
        // the top texture and its gills with the bottom one (voxel.wgsl `cap_vertex`).
        let cap = info.model == PlantModel::Cap && piece >= 2 || info.model == PlantModel::Rock;
        let w1 = w1(match (cap, piece) {
            (false, _) => Face::PosX,
            (true, 7) if info.model == PlantModel::Cap => Face::NegY,
            (true, _) => Face::PosY,
        });
        let base = out.vertices.len() as u32;
        for (corner, (dx, dz, top)) in [(x0, z0, 0), (x1, z1, 0), (x1, z1, 1), (x0, z0, 1)].into_iter().enumerate() {
            let top = if cap { 1 } else { top };
            let p = pos + IVec3::new(dx, top, dz);
            let w0 = p.x as u32
                | (p.y as u32) << 6
                | (p.z as u32) << 12
                | face << 18
                | (corner as u32) << 21
                | (top as u32) << 23
                | stack << PLANT_STACK_SHIFT
                | (hangs as u32) << PLANT_HANGS_SHIFT
                | (info.model as u32 & 3) << PLANT_MODEL_SHIFT
                | (info.model as u32 >> 2) << PLANT_MODEL_HIGH_SHIFT
                | (piece as u32 / 2) << PLANT_PIECE_SHIFT;
            out.vertices.push([w0, w1]);
        }
        // Both windings, so back-face culling keeps one of them from either side.
        out.indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|i| base + i));
    }
}

/// The quads of a block drawn as its shape (`RenderKind::Model`), each a quad of the
/// renderer's shape table (`BlockRegistry::shapes`, in order). A quad inside the cell is lit
/// by the cell's own light, or, in a block that stops light (a roof, which holds none), by
/// the neighbour's it faces; one on its boundary by the brighter of the cell's and the
/// neighbour's (the one beyond, for a shape that fills its cell and so holds no light), and
/// it hides, if it may, behind a neighbour that hides faces. They go as plant faces of the
/// shape model (`PLANT_SHAPE`), with the table's quad numbered in 14 bits: its low 12 in
/// word 1 bits 12..23, where a plant has its tint and height, its high 2 in word 0 bits
/// 30..31, where a plant has which quad of its model it is (see [`shape_quad_index`]).
fn emit_model(out: &mut MeshBuffers, registry: &BlockRegistry, cells: &[Cell], padded: usize, pos: IVec3) {
    let cell = &cells[padded];
    let Some(first) = registry.shape_first_quad(cell.block) else { return };
    let Some(shape) = registry.shape(cell.block) else { return };
    let layers = registry.shape_layers(cell.block);
    // A block that dims light (a roof holds none, a drawn curtain little) lights its faces
    // inside the cell by the side they face, not by the dim light it holds.
    let dims = registry.info(cell.block).attenuation > 1;
    let at = IVec3::new((padded % P) as i32, (padded / (P * P)) as i32, (padded / P % P) as i32);
    for (q, quad) in shape.quads.iter().enumerate() {
        let mut light = cell.light;
        if let Some(face) = quad.boundary.or(if dims { Some(quad.face) } else { None }) {
            let n = at + face.normal();
            let beyond = &cells[pidx(n.x as usize, n.y as usize, n.z as usize)];
            if quad.boundary.is_some() && quad.cull && beyond.occludes {
                continue;
            }
            light = (light & 0xF0).max(beyond.light & 0xF0) | (light & 0x0F).max(beyond.light & 0x0F);
        }
        let (high, low) = shape_quad_bits((first + q) as u32);
        let w1 = layers[q] as u32 | low | (light as u32) << 24;
        let base = out.vertices.len() as u32;
        for corner in 0..4u32 {
            let w0 = pos.x as u32
                | (pos.y as u32) << 6
                | (pos.z as u32) << 12
                | PLANT_DIAGONAL << 18
                | corner << 21
                | (PLANT_SHAPE & 3) << PLANT_MODEL_SHIFT
                | (PLANT_SHAPE >> 2) << PLANT_MODEL_HIGH_SHIFT
                | high;
            out.vertices.push([w0, w1]);
        }
        out.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    }
}

/// A shape quad's number in the renderer's table as a vertex keeps it: (word 0 bits, word 1
/// bits). See [`shape_quad_index`].
fn shape_quad_bits(index: u32) -> (u32, u32) {
    debug_assert!((index as usize) < crate::shape::MAX_SHAPE_QUADS);
    ((index >> 12 & 3) << PLANT_PIECE_SHIFT, (index & 0xFFF) << 12)
}

/// The number in the renderer's table of a shape quad's vertex, from its two words, as
/// voxel.wgsl `shape_index` reads it.
pub fn shape_quad_index(w0: u32, w1: u32) -> usize {
    ((w1 >> 12 & 0xFFF) | (w0 >> PLANT_PIECE_SHIFT & 3) << 12) as usize
}

/// A blade seed for every grass block whose top is open to the air (or a plant).
fn grass_tops(looks: &[Look], cells: &[Cell]) -> Vec<u32> {
    let mut seeds = Vec::new();
    for y in 1..=S {
        for z in 1..=S {
            for x in 1..=S {
                if !looks[cells[pidx(x, y, z)].block.0 as usize].grass_top {
                    continue;
                }
                let above = &cells[pidx(x, y + 1, z)];
                if above.occludes || looks[above.block.0 as usize].render == RenderKind::Translucent {
                    continue;
                }
                let local = UVec3::new(x as u32 - 1, y as u32 - 1, z as u32 - 1);
                seeds.push(blade_seed(local, above.light));
            }
        }
    }
    seeds
}

/// Packs a grass block's chunk-local position and the light above it into one word:
/// x:5 | y:5 | z:5 | block light:4 | sky light:4. The renderer looks its colour up by
/// position.
fn blade_seed(local: UVec3, light: u8) -> u32 {
    local.x | local.y << 5 | local.z << 10 | ((light & 0xF) as u32) << 15 | ((light >> 4) as u32) << 19
}

/// A flow rounded to the vertex format: a speed level (an index into `FLOW_SPEEDS`) and a
/// direction in 16ths of a turn counter-clockwise from +x towards +z, which is 0 when still
/// so still water merges.
fn quantise_flow(flow: Vec2) -> (u8, u8) {
    let speed = flow.length();
    let level = (0..FLOW_SPEEDS.len()).min_by(|&a, &b| (FLOW_SPEEDS[a] - speed).abs().total_cmp(&(FLOW_SPEEDS[b] - speed).abs())).unwrap_or(0);
    if level == 0 {
        return (0, 0);
    }
    let turn = flow.y.atan2(flow.x) / std::f32::consts::TAU;
    (level as u8, ((turn * 16.0).round().rem_euclid(16.0)) as u8 % 16)
}

/// Leaf cards for a leaf block on the outside of a crown: two crossed upright quads
/// through the block, and a flat one if it is open to the sky, which the shader turns,
/// grows past the block and cuts to a round cluster (`voxel.wgsl`), lit by the open side.
/// `padded` is the block's position in the padded cells.
fn emit_fringe(out: &mut MeshBuffers, registry: &BlockRegistry, cells: &[Cell], padded: IVec3) {
    let cell = |p: IVec3| &cells[pidx(p.x as usize, p.y as usize, p.z as usize)];
    let clear = |c: &Cell| !c.occludes && registry.info(c.block).render != RenderKind::Leaves;
    let open: Vec<&Cell> = Face::ALL.iter().map(|face| cell(padded + face.normal())).filter(|c| clear(c)).collect();
    // A lone leaf block on the ground (a heap of fallen leaves) stays a block: grown into a
    // round cluster it would look like a ball resting on nothing.
    let lone = Face::ALL.iter().all(|face| registry.info(cell(padded + face.normal()).block).render != RenderKind::Leaves);
    if open.is_empty() || (lone && cell(padded - IVec3::Y).occludes) {
        return;
    }
    let flat = clear(cell(padded + IVec3::Y));
    // Cards are shaded as the face of the crown they grow from, so one seen edge on reads
    // as part of that face rather than a light seam across it: an open side, or the top
    // (or bottom) where no side is open.
    let side = [Face::PosX, Face::NegX, Face::PosZ, Face::NegZ, Face::PosY, Face::NegY]
        .into_iter()
        .find(|&face| clear(cell(padded + face.normal())))
        .unwrap_or(Face::PosY);
    let sky = open.iter().map(|c| c.light >> 4).max().unwrap_or(0);
    let block = open.iter().map(|c| c.light & 15).max().unwrap_or(0);
    let pos = padded - 1;
    let leaf = cell(padded).block;
    let info = registry.info(leaf);
    let needles = if registry.def(leaf).has_tag("conifer") { FLAG_NEEDLES } else { 0 };
    let tint = info.tints[Face::PosY as usize];
    let w1 = info.layers[Face::PosY as usize] as u32 | (tint as u32) << 12 | ((sky << 4 | block) as u32) << 24;
    let cards = [(PLANT_DIAGONAL, (0, 0), (1, 1), 0), (PLANT_ANTIDIAGONAL, (1, 0), (0, 1), 0), (PLANT_DIAGONAL, (0, 0), (1, 1), FLAG_FLAT)];
    for (face, (x0, z0), (x1, z1), shape) in cards.into_iter().take(if flat { 3 } else { 2 }) {
        let base = out.vertices.len() as u32;
        for (corner, (dx, dz, top)) in [(x0, z0, 0), (x1, z1, 0), (x1, z1, 1), (x0, z0, 1)].into_iter().enumerate() {
            let p = pos + IVec3::new(dx, top, dz);
            let w0 = p.x as u32 | (p.y as u32) << 6 | (p.z as u32) << 12 | face << 18 | (corner as u32) << 21 | (top as u32) << 23;
            out.vertices.push([w0 | FLAG_FRINGE | needles | shape | (side as u32) << CARD_FACE_SHIFT, w1]);
        }
        out.indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|i| base + i));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::block::{BlockDef, Textures};

    fn registry() -> BlockRegistry {
        let block = |name: &str, render| BlockDef {
            name: format!("test:{name}"),
            render,
            textures: Textures::All(name.into()),
            ..Default::default()
        };
        BlockRegistry::new(vec![
            block("stone", RenderKind::Opaque),
            BlockDef { solid: false, tint: crate::block::BlockTint::Grass, ..block("short_grass", RenderKind::Plant) },
            block("leaves", RenderKind::Leaves),
            block("glass", RenderKind::Cutout),
        ])
        .unwrap()
    }

    fn mesh_of(column: Column, cy: i32) -> ChunkMesh {
        mesh_at(column, cy, 1)
    }

    fn mesh_at(column: Column, cy: i32, detail: i32) -> ChunkMesh {
        let reg = registry();
        let column = Arc::new(column);
        let region = LitRegion::compute(&reg, &std::array::from_fn(|_| column.clone()));
        mesh_chunk(&reg, &region, &column, cy, detail)
    }

    /// Ground whose height changes from column to column, as on a rough hillside.
    fn rough_ground() -> Column {
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                let height = 4 + (x * 7 + z * 13) % 5 + x / 4;
                for y in 0..height {
                    column.set_block(IVec3::new(x, y, z), BlockId(1));
                }
            }
        }
        column
    }

    /// A slab's quads hide and take light as they lie: its bottom, on stone, hides; a side
    /// takes the brighter of its own cell's light and the light beyond, sky and block
    /// apart; its top, inside the cell, its cell's own; and beside another slab both sides
    /// stay, as a slab hides nothing.
    #[test]
    fn a_shapes_quads_hide_and_take_light_as_they_lie() {
        let file = crate::shape::ShapeFile::read("test", r#"(shapes: {"slab": (boxes: [(to: (16, 8, 16), cull: true)])})"#).unwrap();
        let shapes = crate::shape::Shapes::gather(vec![("test".into(), file)], crate::shape::ShapeLimits { boxes: 1, quads: 64 }).unwrap();
        let block = |name: &str, render| BlockDef { name: format!("test:{name}"), render, textures: Textures::All(name.into()), ..Default::default() };
        let reg = BlockRegistry::with_shapes(
            vec![
                block("stone", RenderKind::Opaque),
                BlockDef { shape: Some("test:slab".into()), ..block("slab", RenderKind::Model) },
                // A lamp that lets light by: a slab's side against it is not hidden.
                BlockDef { light: 14, solid: false, ..block("lamp", RenderKind::Cutout) },
            ],
            &shapes,
        )
        .unwrap();
        let (stone, slab, lamp) = (reg.expect("test:stone"), reg.expect("test:slab"), reg.expect("test:lamp"));
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                column.set_block(IVec3::new(x, 0, z), stone);
            }
        }
        // A roof over them all, so the sky's light is not full everywhere.
        for z in 2..9 {
            for x in 2..10 {
                column.set_block(IVec3::new(x, 4, z), stone);
            }
        }
        let (a, b) = (IVec3::new(5, 1, 5), IVec3::new(6, 1, 5));
        column.set_block(a, slab);
        column.set_block(b, slab);
        column.set_block(IVec3::new(4, 1, 5), lamp);
        let column = Arc::new(column);
        let region = LitRegion::compute(&reg, &std::array::from_fn(|_| column.clone()));
        let mesh = mesh_chunk(&reg, &region, &column, 0, 1);
        let shape = reg.shape(slab).unwrap();
        // Each shape quad drawn: the cell it is in, the quad of the shape, and its light.
        let first = reg.shape_first_quad(slab).unwrap();
        let drawn: Vec<(IVec3, &crate::shape::ShapeQuad, u8)> = mesh
            .cutout
            .vertices
            .chunks(4)
            .filter(|quad| is_shape_quad(quad[0][0]))
            .map(|quad| {
                let [w0, w1] = quad[0];
                let at = IVec3::new((w0 & 63) as i32, ((w0 >> 6) & 63) as i32, ((w0 >> 12) & 63) as i32);
                (at, &shape.quads[shape_quad_index(w0, w1) - first], (w1 >> 24) as u8)
            })
            .collect();
        let quad = |at: IVec3, face: Face| drawn.iter().find(|(p, q, _)| *p == at && q.face == face).map(|&(_, _, light)| light);
        let max = |x: u8, y: u8| (x & 0xF0).max(y & 0xF0) | (x & 0x0F).max(y & 0x0F);
        // On stone: no bottom.
        assert_eq!(quad(a, Face::NegY), None);
        // Towards the lamp, its side takes the lamp cell's block light, brighter than its own.
        let (own, lamp_light) = (region.light(a), region.light(a - IVec3::X));
        assert!(lamp_light & 15 > own & 15, "the lamp's cell is the brighter: {lamp_light:#x} against {own:#x}");
        assert_eq!(quad(a, Face::NegX), Some(max(own, lamp_light)));
        // Its top lies inside the cell: its own light.
        assert_eq!(quad(a, Face::PosY), Some(own));
        // Beside the other slab, both sides are drawn.
        assert!(quad(a, Face::PosX).is_some() && quad(b, Face::NegX).is_some());
        assert_eq!(drawn.iter().filter(|(p, ..)| *p == a).count(), 5);
    }

    /// A roof stops the sky's light: under it the room is dark, while its courses' faces
    /// inside its cell (each step's top and front) take the light of the open side they
    /// face, not the dark of the cell they are in.
    #[test]
    fn a_roof_darkens_under_it_and_its_inner_faces_take_the_light_they_face() {
        let file = crate::shape::ShapeFile::read(
            "test",
            r#"(shapes: {"roof": (boxes: [(to: (16, 8, 16), cull: true), (from: (0, 8, 8), to: (16, 16, 16), cull: true)])})"#,
        )
        .unwrap();
        let shapes = crate::shape::Shapes::gather(vec![("test".into(), file)], crate::shape::ShapeLimits { boxes: 2, quads: 64 }).unwrap();
        let block = |name: &str, render| BlockDef { name: format!("test:{name}"), render, textures: Textures::All(name.into()), ..Default::default() };
        let reg = BlockRegistry::with_shapes(
            vec![block("stone", RenderKind::Opaque), BlockDef { shape: Some("test:roof".into()), light_filter: 14, ..block("roof", RenderKind::Model) }],
            &shapes,
        )
        .unwrap();
        let (stone, roof) = (reg.expect("test:stone"), reg.expect("test:roof"));
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                column.set_block(IVec3::new(x, 0, z), stone);
            }
        }
        // A roof five by five, three up, over open ground.
        for z in 8..13 {
            for x in 8..13 {
                column.set_block(IVec3::new(x, 3, z), roof);
            }
        }
        let column = Arc::new(column);
        let region = LitRegion::compute(&reg, &std::array::from_fn(|_| column.clone()));
        let (centre, under, open) = (IVec3::new(10, 3, 10), IVec3::new(10, 1, 10), IVec3::new(2, 1, 2));
        assert_eq!(region.light(centre) >> 4, 0, "the roof holds no light");
        assert!(region.light(under) >> 4 <= (region.light(open) >> 4) - 3, "under the roof {:#x}, in the open {:#x}", region.light(under), region.light(open));
        let mesh = mesh_chunk(&reg, &region, &column, 0, 1);
        let shape = reg.shape(roof).unwrap();
        let first = reg.shape_first_quad(roof).unwrap();
        let drawn: Vec<(IVec3, &crate::shape::ShapeQuad, u8)> = mesh
            .cutout
            .vertices
            .chunks(4)
            .filter(|quad| is_shape_quad(quad[0][0]))
            .map(|quad| {
                let [w0, w1] = quad[0];
                let at = IVec3::new((w0 & 63) as i32, ((w0 >> 6) & 63) as i32, ((w0 >> 12) & 63) as i32);
                (at, &shape.quads[((w1 >> 12) & 0xFFF) as usize - first], (w1 >> 24) as u8)
            })
            .collect();
        // The lower course's top lies inside the cell, under the open sky above it.
        let inner_top = drawn.iter().find(|(p, q, _)| *p == centre && q.face == Face::PosY && q.boundary.is_none()).expect("the lower course's top is drawn");
        assert_eq!(inner_top.2, region.light(centre + IVec3::Y), "the inner top takes the light above it");
        assert_eq!(inner_top.2 >> 4, 15);
    }

    /// A cloth that dims light (a drawn curtain, `light_filter` 8) holds little of it, but
    /// its face inside the cell takes the light of the open side it faces, so it is not drawn
    /// dark in a lit room.
    #[test]
    fn a_dimming_cloths_inner_face_takes_the_light_it_faces() {
        let file = crate::shape::ShapeFile::read("test", r#"(shapes: {"cloth": (boxes: [(to: (2, 16, 16))])})"#).unwrap();
        let shapes = crate::shape::Shapes::gather(vec![("test".into(), file)], crate::shape::ShapeLimits { boxes: 2, quads: 64 }).unwrap();
        let block = |name: &str, render| BlockDef { name: format!("test:{name}"), render, textures: Textures::All(name.into()), ..Default::default() };
        let reg = BlockRegistry::with_shapes(
            vec![block("stone", RenderKind::Opaque), BlockDef { shape: Some("test:cloth".into()), light_filter: 8, solid: false, ..block("cloth", RenderKind::Model) }],
            &shapes,
        )
        .unwrap();
        let (stone, cloth) = (reg.expect("test:stone"), reg.expect("test:cloth"));
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                column.set_block(IVec3::new(x, 0, z), stone);
            }
        }
        let at = IVec3::new(10, 1, 10);
        column.set_block(at, cloth);
        let column = Arc::new(column);
        let region = LitRegion::compute(&reg, &std::array::from_fn(|_| column.clone()));
        let beside = region.light(at + IVec3::X);
        assert!(region.light(at) >> 4 < beside >> 4, "the cloth holds {:#x}, beside it {beside:#x}", region.light(at));
        let mesh = mesh_chunk(&reg, &region, &column, 0, 1);
        let shape = reg.shape(cloth).unwrap();
        let first = reg.shape_first_quad(cloth).unwrap();
        let inner = mesh
            .cutout
            .vertices
            .chunks(4)
            .filter(|quad| is_shape_quad(quad[0][0]))
            .map(|quad| (&shape.quads[((quad[0][1] >> 12) & 0xFFF) as usize - first], (quad[0][1] >> 24) as u8))
            .find(|(q, _)| q.face == Face::PosX && q.boundary.is_none())
            .expect("the cloth's inner face is drawn");
        assert_eq!(inner.1, beside, "the inner face takes the light beside it");
    }

    #[test]
    fn coarser_detail_takes_fewer_quads() {
        let quads = |detail| mesh_at(rough_ground(), 0, detail).opaque.vertices.len() / 4;
        let (full, half, quarter) = (quads(1), quads(2), quads(4));
        assert!(half * 2 < full + full / 4, "{full} quads in full detail, {half} at 2");
        assert!(quarter < half, "{half} quads at 2, {quarter} at 4");
    }

    #[test]
    fn coarser_detail_keeps_trunks() {
        // A lone trunk on flat ground, which a majority of its group would drop.
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                column.set_block(IVec3::new(x, 0, z), BlockId(1));
            }
        }
        for y in 1..8 {
            column.set_block(IVec3::new(5, y, 5), BlockId(1));
        }
        let mesh = mesh_at(column, 0, 4);
        let top = |v: &Vertex| (v[0] >> 6) & 63;
        assert!(mesh.opaque.vertices.iter().any(|v| top(v) >= 7), "the trunk still stands");
    }

    #[test]
    fn coarser_detail_keeps_heights() {
        // A group of columns whose majority stands 10 high shows 10 high, not lower.
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                let height = if (x + z) % 4 == 0 { 6 } else { 10 };
                for y in 0..height {
                    column.set_block(IVec3::new(x, y, z), BlockId(1));
                }
            }
        }
        let mesh = mesh_at(column, 0, 2);
        let top = |v: &Vertex| (v[0] >> 6) & 63;
        let highest = mesh.opaque.vertices.iter().map(top).max().unwrap();
        assert_eq!(highest, 10, "the top faces of 10-high ground sit at y = 10");
    }

    /// Every number the shape table may hold comes back out of a vertex as it went in, past
    /// the twelve bits word 1 has for it, and the shader reads it the same way.
    #[test]
    fn shape_quads_number_up_to_the_whole_table() {
        for index in [0, 1, 4095, 4096, 5000, crate::shape::MAX_SHAPE_QUADS as u32 - 1] {
            let (high, low) = shape_quad_bits(index);
            assert_eq!(high & !(3 << PLANT_PIECE_SHIFT), 0, "{index}: a shape quad's number spills out of word 0 bits 30..31");
            let w0 = PLANT_DIAGONAL << 18 | (PLANT_SHAPE & 3) << PLANT_MODEL_SHIFT | (PLANT_SHAPE >> 2) << PLANT_MODEL_HIGH_SHIFT | high;
            assert!(is_shape_quad(w0), "{index}: no longer read as a shape quad");
            assert_eq!(shape_quad_index(w0, low), index as usize);
        }
        // A plant's quad of its model, in the same bits, never reads it as a shape.
        for model in PLANT_MODELS {
            for piece in 0..4u32 {
                let w0 = PLANT_DIAGONAL << 18 | (model as u32 & 3) << PLANT_MODEL_SHIFT | (model as u32 >> 2) << PLANT_MODEL_HIGH_SHIFT | piece << PLANT_PIECE_SHIFT;
                assert!(!is_shape_quad(w0), "{model:?} quad {piece} read as a shape quad");
            }
        }
        let shader = include_str!("../../client/src/render/voxel.wgsl");
        assert!(shader.contains("return ((w1 >> 12u) & 0xFFFu) | (((w0 >> PLANT_PIECE_SHIFT) & 3u) << 12u);"), "voxel.wgsl reads a shape quad's number otherwise");
    }

    #[test]
    fn far_crowns_are_solid() {
        // A crown merged for distance draws only its outside, as opaque faces.
        let mut column = Column::default();
        for y in 40..44 {
            for z in 4..12 {
                for x in 4..12 {
                    column.set_block(IVec3::new(x, y, z), BlockId(3));
                }
            }
        }
        let mesh = mesh_at(column, 1, 2);
        assert!(mesh.cutout.vertices.is_empty() && mesh.inner.vertices.is_empty(), "no see-through leaf faces far away");
        assert!(!mesh.opaque.vertices.is_empty(), "the crown is still drawn");
    }

    #[test]
    fn far_chunks_leave_out_dark_caves() {
        // Solid rock with a sealed cave in it: up close its walls are meshed, far away
        // nothing inside is, as no daylight reaches it.
        let rock = |cave: bool| {
            let mut column = Column::default();
            for y in 0..64 {
                for z in 0..32 {
                    for x in 0..32 {
                        if !(cave && (8..24).contains(&x) && (8..24).contains(&z) && (40..50).contains(&y)) {
                            column.set_block(IVec3::new(x, y, z), BlockId(1));
                        }
                    }
                }
            }
            column
        };
        let quads = |cave, detail| mesh_at(rock(cave), 1, detail).opaque.vertices.len() / 4;
        assert!(quads(true, 1) > quads(false, 1), "the cave's walls up close");
        assert_eq!(quads(true, 2), quads(false, 2), "only the ground's top far away");
    }

    #[test]
    fn far_faces_merge_across_corners() {
        // Lit evenly and without corner occlusion, rough ground far away merges into far
        // fewer quads than its blocks would.
        let quads = |detail| mesh_at(rough_ground(), 0, detail).opaque.vertices.len() / 4;
        assert!(quads(4) * 4 < quads(1), "{} quads at 4, {} in full detail", quads(4), quads(1));
    }

    #[test]
    fn single_block_has_six_quads() {
        let mut column = Column::default();
        column.set_block(IVec3::new(4, 40, 4), BlockId(1));
        let mesh = mesh_of(column, 1);
        assert_eq!(mesh.opaque.vertices.len(), 24);
        assert_eq!(mesh.opaque.indices.len(), 36);
    }

    #[test]
    fn flat_lit_floor_merges_to_one_quad() {
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                column.set_block(IVec3::new(x, 0, z), BlockId(1));
            }
        }
        // Every neighbour column has the same floor, so the only visible faces are the
        // top of the floor, all lit by open sky.
        let mesh = mesh_of(column, 0);
        assert_eq!(mesh.opaque.vertices.len(), 4);
    }

    #[test]
    fn plant_is_two_crossed_double_sided_quads() {
        let mut column = Column::default();
        for z in 0..32 {
            for x in 0..32 {
                column.set_block(IVec3::new(x, 0, z), BlockId(1));
            }
        }
        column.set_block(IVec3::new(4, 1, 7), BlockId(2));
        let mesh = mesh_of(column, 0);
        // The floor still merges into one quad: the plant hides nothing and casts no AO.
        assert_eq!(mesh.opaque.vertices.len(), 4);
        assert_eq!(mesh.cutout.vertices.len(), 8);
        assert_eq!(mesh.cutout.indices.len(), 24);
        for (i, [w0, w1]) in mesh.cutout.vertices.iter().enumerate() {
            let (x, y, z) = (w0 & 63, (w0 >> 6) & 63, (w0 >> 12) & 63);
            let face = (w0 >> 18) & 7;
            assert_eq!(face, if i < 4 { PLANT_DIAGONAL } else { PLANT_ANTIDIAGONAL });
            assert!((4..=5).contains(&x) && (1..=2).contains(&y) && (7..=8).contains(&z));
            // The corner in place of occlusion, top edge flagged, alone in its stack, lit
            // by open sky.
            assert_eq!((w0 >> 21) & 3, i as u32 % 4);
            assert_eq!((w0 >> 23) & 1, y - 1);
            assert_eq!(w0 >> PLANT_STACK_SHIFT & 3, 0);
            assert_eq!(w1 >> 28, 15);
        }
        // Each quad lies on its diagonal plane.
        let corner = |v: &Vertex| ((v[0] & 63) as i32 - 4, ((v[0] >> 12) & 63) as i32 - 7);
        assert!(mesh.cutout.vertices[..4].iter().map(corner).all(|(x, z)| x == z));
        assert!(mesh.cutout.vertices[4..].iter().map(corner).all(|(x, z)| x + z == 1));
    }

    #[test]
    fn leaves_draw_the_faces_between_them() {
        let (leaves, glass) = (BlockId(3), BlockId(4));
        for (block, shared_faces) in [(leaves, 1), (glass, 0)] {
            let mut column = Column::default();
            column.set_block(IVec3::new(4, 40, 4), block);
            column.set_block(IVec3::new(5, 40, 4), block);
            let mesh = mesh_of(column, 1);
            // Between the two blocks, in the plane x = 5: one double-sided face for leaves,
            // which casts no shadow.
            let between = |buffers: &MeshBuffers| buffers.vertices.chunks(4).filter(|quad| quad.iter().all(|v| v[0] & 63 == 5)).count();
            assert_eq!((between(&mesh.cutout), between(&mesh.inner)), (0, shared_faces));
        }
    }

    /// Leaves on the outside of a crown grow cards, a flat one too where open to the sky,
    /// but a lone leaf block lying on the ground (a heap of fallen leaves) stays a block.
    #[test]
    fn leaf_cards_grow_on_crowns_not_on_heaps() {
        let (stone, leaves) = (BlockId(1), BlockId(3));
        let cards = |column: Column| mesh_of(column, 1).fringe.vertices.len() / 4;
        let mut crown = Column::default();
        crown.set_block(IVec3::new(4, 40, 4), leaves);
        crown.set_block(IVec3::new(5, 40, 4), leaves);
        assert_eq!(cards(crown), 6, "two crossed upright cards and a flat one on each leaf");
        let mut heap = Column::default();
        heap.set_block(IVec3::new(4, 39, 4), stone);
        heap.set_block(IVec3::new(4, 40, 4), leaves);
        assert_eq!(cards(heap), 0);
    }

    #[test]
    fn buried_leaves_hide_the_faces_between_them() {
        let mut column = Column::default();
        for y in 40..45 {
            for z in 4..9 {
                for x in 4..9 {
                    column.set_block(IVec3::new(x, y, z), BlockId(3));
                }
            }
        }
        let mesh = mesh_of(column, 1);
        // A 5³ crown has 3 × 4 × 25 pairs of neighbouring leaves. The faces between two of
        // the 27 buried ones (3 × 2 × 9 pairs) are hidden; each other pair has one.
        let area: i32 = mesh
            .inner
            .vertices
            .chunks(4)
            .map(|quad| {
                let axis = |shift: u32| quad.iter().map(move |v| (v[0] >> shift & 63) as i32);
                [0, 6, 12].map(|s| (axis(s).max().unwrap() - axis(s).min().unwrap()).max(1)).iter().product::<i32>()
            })
            .sum();
        assert_eq!(area, 3 * 4 * 25 - 3 * 2 * 9);
    }

    const FLUIDS: &str = r#"(
        blocks: [
            (name: "stone"),
            (name: "glass", render: Cutout),
            (
                name: "water", render: Translucent, solid: false, tint: Water, hardness: -1.0,
                fluid: (tick: 5, drop: 1, slope: 4, infinite: true, pools: true, currents: true),
            ),
        ],
    )"#;

    /// Water states placed on a stone floor at y 31, meshed: the registry, its water, and the
    /// translucent mesh of the chunk.
    fn water_mesh(place: impl FnOnce(&BlockRegistry, &crate::fluid::Fluid, &mut Column)) -> MeshBuffers {
        let reg = BlockRegistry::from_ron("test", FLUIDS).unwrap();
        let mut column = Column::default();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                column.set_block(IVec3::new(x, 31, z), reg.expect("test:stone"));
            }
        }
        place(&reg, &reg.fluids()[0], &mut column);
        let column = Arc::new(column);
        let region = LitRegion::compute(&reg, &std::array::from_fn(|_| column.clone()));
        mesh_chunk(&reg, &region, &column, 1, 1).translucent
    }

    /// Unpacked: chunk-local position, face, flow speed level, surface drop and flow
    /// direction.
    fn unpack(v: Vertex) -> (IVec3, u32, u32, u32, u32) {
        let w = v[0];
        let p = IVec3::new((w & 63) as i32, (w >> 6 & 63) as i32, (w >> 12 & 63) as i32);
        (p, w >> 18 & 7, w >> 21 & 3, w >> SURFACE_DROP_SHIFT & 7, w >> FLOW_DIRECTION_SHIFT)
    }

    #[test]
    fn water_shows_no_face_against_glass() {
        // A source with glass on one side and air on the other three: its top and the three
        // open sides, but nothing against the glass, as against stone.
        let mesh = water_mesh(|reg, water, column| {
            column.set_block(IVec3::new(5, 32, 5), water.source);
            column.set_block(IVec3::new(6, 32, 5), reg.expect("test:glass"));
        });
        let faces: Vec<u32> = mesh.vertices.chunks(4).map(|quad| unpack(quad[0]).1).collect();
        assert_eq!(faces.len(), 4, "{faces:?}");
        assert!(!faces.contains(&(Face::PosX as u32)), "{faces:?}");
    }

    #[test]
    fn faces_between_states_of_one_fluid_are_hidden() {
        let mesh = water_mesh(|_, water, column| {
            column.set_block(IVec3::new(5, 32, 5), water.source);
            column.set_block(IVec3::new(6, 32, 5), water.flowing(4));
        });
        // Two cells in a row: one top quad each (their surfaces differ), and no face where
        // they meet, at x = 6.
        let quads: Vec<Vec<_>> = mesh.vertices.chunks(4).map(|q| q.iter().map(|&v| unpack(v)).collect()).collect();
        let x_faces_at_6 = quads.iter().filter(|q| q.iter().all(|v| v.0.x == 6 && (v.1 == 0 || v.1 == 1))).count();
        assert_eq!(x_faces_at_6, 0);
        assert_eq!(quads.iter().filter(|q| q[0].1 == Face::PosY as u32).count(), 2);
    }

    #[test]
    fn flowing_water_slopes_down_from_its_source() {
        let mesh = water_mesh(|_, water, column| {
            column.set_block(IVec3::new(5, 32, 5), water.source);
            column.set_block(IVec3::new(6, 32, 5), water.flowing(6));
            column.set_block(IVec3::new(7, 32, 5), water.flowing(2));
        });
        let top: Vec<_> = mesh.vertices.iter().map(|&v| unpack(v)).filter(|v| v.1 == Face::PosY as u32).collect();
        let drop_at = |x: i32| top.iter().filter(|v| v.0.x == x).map(|v| v.3).max().unwrap();
        assert!(drop_at(6) < drop_at(7) && drop_at(7) < drop_at(8), "{:?}", [5, 6, 7, 8].map(drop_at));
        // The sides' top edges follow the surface; their bottoms stay on the floor.
        let side_tops = mesh.vertices.iter().map(|&v| unpack(v)).filter(|v| v.1 != Face::PosY as u32 && v.0.y == 1);
        assert!(side_tops.clone().any(|v| v.3 > 0));
        let side_bottoms = mesh.vertices.iter().map(|&v| unpack(v)).filter(|v| v.1 != Face::PosY as u32 && v.0.y == 0);
        assert!(side_bottoms.into_iter().all(|v| v.3 == 0));
    }

    #[test]
    fn river_currents_reach_the_vertices() {
        use crate::fluid::{Current, FluidState};
        let mesh = water_mesh(|_, water, column| {
            let current = FluidState { current: Some(Current { direction: 4, speed: 2 }), ..FluidState::source(water.id) };
            for z in 2..10 {
                for x in 2..10 {
                    column.set_block(IVec3::new(x, 32, z), water.block(current));
                }
            }
        });
        // Along +z at 1.4 blocks a second, everywhere on top but the corners at the edge of
        // the river, which also average the still air beside it.
        let inner: Vec<_> = mesh
            .vertices
            .iter()
            .map(|&v| unpack(v))
            .filter(|v| v.1 == Face::PosY as u32 && (3..10).contains(&v.0.x) && (3..10).contains(&v.0.z))
            .collect();
        assert!(!inner.is_empty());
        assert!(inner.iter().all(|v| (v.2, v.4) == (2, 4)), "{inner:?}");
    }

    #[test]
    fn flows_round_to_the_vertex_format() {
        assert_eq!(quantise_flow(Vec2::new(0.0, 1.4)), (2, 4));
        assert_eq!(quantise_flow(Vec2::new(-2.6, 0.1)), (3, 8));
        assert_eq!(quantise_flow(Vec2::new(0.6, -0.6)), (1, 14));
        assert_eq!(quantise_flow(Vec2::new(0.1, 0.1)), (0, 0));
    }

    #[test]
    fn only_open_water_is_open() {
        let open = |mesh: &MeshBuffers| -> Vec<u32> {
            mesh.vertices.iter().filter(|v| unpack(**v).1 == Face::PosY as u32).map(|v| v[1] >> OPEN_WATER_SHIFT & 15).collect()
        };
        // A lake filling the columns (the neighbours are the same column): open everywhere.
        let lake = water_mesh(|_, water, column| {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    column.set_block(IVec3::new(x, 32, z), water.source);
                }
            }
        });
        assert!(open(&lake).iter().all(|&o| o == 15), "{:?}", open(&lake));
        // A puddle two blocks across: hardly open at all.
        let puddle = water_mesh(|_, water, column| {
            for (x, z) in [(10, 10), (11, 10), (10, 11), (11, 11)] {
                column.set_block(IVec3::new(x, 32, z), water.source);
            }
        });
        assert!(open(&puddle).iter().all(|&o| o == 0), "{:?}", open(&puddle));
    }

    #[test]
    fn beds_are_lit_as_the_surface_is() {
        // A pool five deep: its floor's top faces carry the sky light above the water.
        let mesh = |reg: &BlockRegistry, column: Column| {
            let column = Arc::new(column);
            let region = LitRegion::compute(reg, &std::array::from_fn(|_| column.clone()));
            mesh_chunk(reg, &region, &column, 0, 1).opaque
        };
        let reg = BlockRegistry::from_ron("test", FLUIDS).unwrap();
        let stone = reg.expect("test:stone");
        let mut column = Column::default();
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                column.set_block(IVec3::new(x, 31, z), stone);
                for y in 32..37 {
                    column.set_block(IVec3::new(x, y, z), reg.fluids()[0].source);
                }
            }
        }
        let floor = mesh(&reg, column);
        let sky = |v: &Vertex| v[1] >> 28;
        let tops: Vec<u32> = floor.vertices.iter().filter(|v| unpack(**v).1 == Face::PosY as u32).map(sky).collect();
        assert!(!tops.is_empty() && tops.iter().all(|&s| s == 15), "{tops:?}");
    }
}
