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
//! word 1: layer:12 | tint:2 | temperature:5 | humidity:5 | block:4 | sky:4
//! ```
//!
//! `face` is a `Face` (0..=5) for cube faces, or `PLANT_DIAGONAL` (6, the plane through
//! x = z) or `PLANT_ANTIDIAGONAL` (7, through x + z = 1) for plants. On leaf faces, flag 1
//! marks a double-sided face between two leaves (meshed apart, in `ChunkMesh::inner`), and
//! flag 2 one with leaves behind it, which casts a solid shadow. On translucent faces flag 1 marks a
//! face covered by a block (water under ice). On plant vertices flag 1 marks the top edge,
//! the one that sways in the wind. Plants carry no ambient occlusion: their `ao` bits hold the
//! vertex's corner (0..=3, in the order the quad is built), and bits 25..27 how many plant
//! cells the plant stands on, so the shader can place each plant as a whole (`voxel.wgsl`).
//! Bit 27 marks the same crossed quads used as a leaf fringe (`ChunkMesh::fringe`). On water
//! surfaces, bits 25..27 hold the depth of water below the vertex (0..=7 blocks).

use glam::IVec3;

use crate::block::{BlockId, BlockRegistry, RenderKind, Tint};
use crate::coords::{CHUNK_SIZE, Face};
use crate::light::LitRegion;
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
/// Marks the crossed quads of a leaf fringe, which the shader grows out of the leaf block.
const FLAG_FRINGE: u32 = 1 << 27;
/// On water surfaces, bits 25..27 hold the water's depth below the vertex, in blocks.
const WATER_DEPTH_SHIFT: u32 = 25;
const MAX_WATER_DEPTH: u8 = 7;
/// On plant vertices, bits 25..27 hold how many plant cells lie below in the same stack.
const PLANT_STACK_SHIFT: u32 = 25;
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
    /// Leafy sprigs sticking out of crowns (bushy leaves), only worth drawing nearby.
    pub fringe: MeshBuffers,
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
}

/// Meshes chunk `cy` of the column whose light region is `region`.
pub fn mesh_chunk(registry: &BlockRegistry, region: &LitRegion, column: &Column, cy: i32) -> ChunkMesh {
    let _span = tracing::info_span!("mesh_chunk").entered();
    let chunk = &column.chunks[cy as usize];
    if chunk.as_uniform() == Some(BlockId::AIR) {
        return ChunkMesh { connectivity: Connectivity::ALL, ..Default::default() };
    }

    let origin = IVec3::new(-1, cy * CHUNK_SIZE - 1, -1);
    let mut cells = vec![Cell::default(); P * P * P];
    // Bit columns along each axis (bit i = padded coordinate i).
    let mut occ = Axes::default();
    let mut opaque = Axes::default();
    let mut cutout = Axes::default();
    let mut translucent = Axes::default();
    let mut plants = Vec::new();
    let mut joins = Joins::default();
    // Leaf cells, as bit columns along x.
    let mut leaves = [[0u64; P]; P];
    for y in 0..P {
        for z in 0..P {
            for x in 0..P {
                let pos = origin + IVec3::new(x as i32, y as i32, z as i32);
                let (block, info_occludes, render) = match region.block(pos) {
                    Some(block) => {
                        let info = registry.info(block);
                        (block, info.occludes, info.render)
                    }
                    // Below the world: an invisible wall that hides bottom faces.
                    None => (BlockId::AIR, true, RenderKind::Invisible),
                };
                let shades = info_occludes || render == RenderKind::Leaves;
                cells[pidx(x, y, z)] = Cell { block, occludes: info_occludes, shades, light: region.light(pos) };
                if info_occludes {
                    occ.set(x, y, z);
                }
                match render {
                    RenderKind::Opaque => opaque.set(x, y, z),
                    RenderKind::Cutout => cutout.set(x, y, z),
                    RenderKind::Leaves => {
                        cutout.set(x, y, z);
                        leaves[y][z] |= 1 << x;
                    }
                    RenderKind::Translucent => translucent.set(x, y, z),
                    RenderKind::Plant if [x, y, z].iter().all(|c| (1..=S).contains(c)) => plants.push(pidx(x, y, z)),
                    RenderKind::Plant | RenderKind::Invisible => {}
                }
                if matches!(render, RenderKind::Cutout | RenderKind::Translucent) {
                    // Cells with a lower coordinate are already filled in.
                    let hidden = &mut joins.hidden;
                    if x > 0 && cells[pidx(x - 1, y, z)].block == block {
                        hidden.x[y][z] |= 1 << (x - 1);
                    }
                    if y > 0 && cells[pidx(x, y - 1, z)].block == block {
                        hidden.y[z][x] |= 1 << (y - 1);
                    }
                    if z > 0 && cells[pidx(x, y, z - 1)].block == block {
                        hidden.z[y][x] |= 1 << (z - 1);
                    }
                }
            }
        }
    }

    join_leaves(&leaves, &occ, &mut joins);

    let water_depth = water_depths(registry, column, &cells, origin);
    let mut mesher = Mesher { registry, column, cells: &cells, water_depth, planes: [[0; S]; S], keys: [[0; S]; S] };
    let mut mesh = ChunkMesh { connectivity: Connectivity::compute(|x, y, z| cells[pidx(x + 1, y + 1, z + 1)].occludes), ..Default::default() };
    for face in Face::ALL {
        mesher.mesh_faces(face, &occ, &opaque, &joins, &mut mesh.opaque);
        mesher.mesh_faces(face, &occ, &cutout, &joins, &mut mesh.cutout);
        mesher.mesh_faces(face, &occ, &translucent, &joins, &mut mesh.translucent);
    }
    mesh.inner = split_inner(&mut mesh.cutout);
    for i in plants {
        let local = IVec3::new((i % P) as i32, (i / (P * P)) as i32, (i / P % P) as i32) - 1;
        emit_plant(&mut mesh.cutout, registry, column, &cells[i], local, cy * CHUNK_SIZE);
    }
    for (y, plane) in leaves.iter().enumerate().take(S + 1).skip(1) {
        for (z, &bits) in plane.iter().enumerate().take(S + 1).skip(1) {
            let mut row = bits & !1 & !(1 << (S + 1));
            while row != 0 {
                let x = row.trailing_zeros() as usize;
                row &= row - 1;
                emit_fringe(&mut mesh.fringe, registry, column, &cells, IVec3::new(x as i32, y as i32, z as i32));
            }
        }
    }
    mesh
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
    column: &'a Column,
    cells: &'a [Cell],
    /// Water cells in a column counting down from each padded cell (0 for other blocks),
    /// capped at `MAX_WATER_DEPTH`; empty when the chunk has no water.
    water_depth: Vec<u8>,
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
    /// layer:12 tint:2 temperature:5 humidity:5 | ao 4×2 | light 4×8 | flags:2.
    fn face_key(&self, face: Face, fa: &FaceAxes, slice: usize, row: usize, col: usize) -> u128 {
        let pos = Self::cell(fa, slice, row, col);
        let info = self.registry.info(self.padded(pos).block);
        let (inner, backed) = match info.render {
            // Shadows from a crown only need its shell: faces between two leaves cast none,
            // and shell faces with more leaves behind them cast a solid one. Crowns shade as
            // densely as if every face cast, for the cost of the shell.
            RenderKind::Leaves => {
                let leaf = |p: IVec3| self.registry.info(self.padded(p).block).render == RenderKind::Leaves;
                (leaf(pos + face.normal()), leaf(pos - face.normal()))
            }
            // Water under ice is covered: still, and reflecting nothing.
            RenderKind::Translucent => (!self.padded(pos + face.normal()).block.is_air(), false),
            _ => (false, false),
        };
        let tint = info.tints[face as usize];
        let [temperature, humidity] = if tint == Tint::None { [0, 0] } else { self.column.climate_at(pos.x, pos.z) };
        let surface = info.layers[face as usize] as u64
            | (tint as u64) << 12
            | ((temperature >> 3) as u64) << 14
            | ((humidity >> 3) as u64) << 19;

        let front = pos + face.normal();
        let (du, dv) = (unit(fa.col), unit(fa.row));
        let center = self.padded(front);
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
            let packed = (((sky + count / 2) / count) << 4) | ((block + count / 2) / count);
            light |= (packed as u64) << (corner * 8);
        }
        // Water surfaces carry how deep the water is at each corner, so shallows can show
        // their bed: the mean over the four columns meeting there.
        let mut depth = 0u128;
        if face == Face::PosY && tint == Tint::Water && !self.water_depth.is_empty() {
            for (corner, (su, sv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
                let at = |p: IVec3| self.water_depth[pidx((p.x + 1) as usize, (p.y + 1) as usize, (p.z + 1) as usize)] as u32;
                let sum = at(pos) + at(pos + du * su) + at(pos + dv * sv) + at(pos + du * su + dv * sv);
                depth |= (sum.div_ceil(4) as u128) << (corner * 3);
            }
        }
        if inner && info.render == RenderKind::Leaves {
            // One double-sided face serves both leaves: lit evenly by both, and shaded as
            // the inside of a crown. Even light also lets these faces merge widely.
            let (a, b) = (self.padded(pos).light as u64, center.light as u64);
            let average = |shift: u32| ((a >> shift & 15) + (b >> shift & 15)).div_ceil(2) << shift;
            light = (average(4) | average(0)) * 0x0101_0101;
            ao = INNER_LEAF_AO * 0b0101_0101;
        }
        (surface | ao << 24 | light << 32) as u128 | (inner as u128) << 64 | (backed as u128) << 65 | depth << 66
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
        let depth = ((key >> (66 + i * 3)) & 7) as u32;
        let w0 = p.x as u32
            | (p.y as u32) << 6
            | (p.z as u32) << 12
            | (face as u32) << 18
            | ao[i] << 21
            | flags << 23
            | depth << WATER_DEPTH_SHIFT;
        out.vertices.push([w0, surface | light << 24]);
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

/// Two crossed quads through the cell's diagonals, drawn from both sides, lit by the
/// plant's own cell and without ambient occlusion.
fn emit_plant(out: &mut MeshBuffers, registry: &BlockRegistry, column: &Column, cell: &Cell, pos: IVec3, chunk_y: i32) {
    let info = registry.info(cell.block);
    // How many plant cells this one stands on (tall grass, sugar cane), so the shader can
    // shape the whole stack as one plant.
    let is_plant = |y: i32| y >= 0 && registry.info(column.block(IVec3::new(pos.x, y, pos.z))).render == RenderKind::Plant;
    let stack = (1..=3).take_while(|&d| is_plant(chunk_y + pos.y - d)).count() as u32;
    let tint = info.tints[Face::PosX as usize];
    let [temperature, humidity] = if tint == Tint::None { [0, 0] } else { column.climate_at(pos.x, pos.z) };
    let w1 = info.layers[Face::PosX as usize] as u32
        | (tint as u32) << 12
        | ((temperature >> 3) as u32) << 14
        | ((humidity >> 3) as u32) << 19
        | (cell.light as u32) << 24;
    for (face, (x0, z0), (x1, z1)) in [(PLANT_DIAGONAL, (0, 0), (1, 1)), (PLANT_ANTIDIAGONAL, (1, 0), (0, 1))] {
        let base = out.vertices.len() as u32;
        for (corner, (dx, dz, top)) in [(x0, z0, 0), (x1, z1, 0), (x1, z1, 1), (x0, z0, 1)].into_iter().enumerate() {
            let p = pos + IVec3::new(dx, top, dz);
            let w0 = p.x as u32
                | (p.y as u32) << 6
                | (p.z as u32) << 12
                | face << 18
                | (corner as u32) << 21
                | (top as u32) << 23
                | stack << PLANT_STACK_SHIFT;
            out.vertices.push([w0, w1]);
        }
        // Both windings, so back-face culling keeps one of them from either side.
        out.indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|i| base + i));
    }
}

/// Depth of water at and below each padded cell, counting cells below the chunk too, or
/// nothing when the chunk holds no water.
fn water_depths(registry: &BlockRegistry, column: &Column, cells: &[Cell], origin: IVec3) -> Vec<u8> {
    let is_water = |block: BlockId| registry.info(block).tints[Face::PosY as usize] == Tint::Water;
    if !cells.iter().any(|c| is_water(c.block)) {
        return Vec::new();
    }
    let mut depth = vec![0u8; P * P * P];
    for z in 0..P {
        for x in 0..P {
            // Water reaching up from below the chunk counts too.
            let (wx, wz) = (origin.x + x as i32, origin.z + z as i32);
            let inside = (0..CHUNK_SIZE).contains(&wx) && (0..CHUNK_SIZE).contains(&wz);
            let mut below = 0u8;
            if inside {
                let mut y = origin.y - 1;
                while y >= 0 && below < MAX_WATER_DEPTH && is_water(column.block(IVec3::new(wx, y, wz))) {
                    below += 1;
                    y -= 1;
                }
            }
            for y in 0..P {
                let i = pidx(x, y, z);
                below = if is_water(cells[i].block) { (below + 1).min(MAX_WATER_DEPTH) } else { 0 };
                depth[i] = below;
            }
        }
    }
    depth
}

/// A sprig of leaves out of a leaf block on the outside of a crown: two crossed quads
/// through the block, which the shader turns and grows beyond it (`voxel.wgsl`), lit by the
/// open side. `padded` is the block's position in the padded cells.
fn emit_fringe(out: &mut MeshBuffers, registry: &BlockRegistry, column: &Column, cells: &[Cell], padded: IVec3) {
    let cell = |p: IVec3| &cells[pidx(p.x as usize, p.y as usize, p.z as usize)];
    let open: Vec<&Cell> = Face::ALL
        .iter()
        .map(|face| cell(padded + face.normal()))
        .filter(|c| !c.occludes && registry.info(c.block).render != RenderKind::Leaves)
        .collect();
    if open.is_empty() {
        return;
    }
    let sky = open.iter().map(|c| c.light >> 4).max().unwrap_or(0);
    let block = open.iter().map(|c| c.light & 15).max().unwrap_or(0);
    let pos = padded - 1;
    let leaf = cell(padded).block;
    let info = registry.info(leaf);
    let tint = info.tints[Face::PosY as usize];
    let [temperature, humidity] = if tint == Tint::None { [0, 0] } else { column.climate_at(pos.x, pos.z) };
    let w1 = info.layers[Face::PosY as usize] as u32
        | (tint as u32) << 12
        | ((temperature >> 3) as u32) << 14
        | ((humidity >> 3) as u32) << 19
        | ((sky << 4 | block) as u32) << 24;
    for (face, (x0, z0), (x1, z1)) in [(PLANT_DIAGONAL, (0, 0), (1, 1)), (PLANT_ANTIDIAGONAL, (1, 0), (0, 1))] {
        let base = out.vertices.len() as u32;
        for (corner, (dx, dz, top)) in [(x0, z0, 0), (x1, z1, 0), (x1, z1, 1), (x0, z0, 1)].into_iter().enumerate() {
            let p = pos + IVec3::new(dx, top, dz);
            let w0 = p.x as u32 | (p.y as u32) << 6 | (p.z as u32) << 12 | face << 18 | (corner as u32) << 21 | (top as u32) << 23;
            out.vertices.push([w0 | FLAG_FRINGE, w1]);
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
            BlockDef { solid: false, tint: Tint::Grass, ..block("short_grass", RenderKind::Plant) },
            block("leaves", RenderKind::Leaves),
            block("glass", RenderKind::Cutout),
        ])
        .unwrap()
    }

    fn mesh_of(column: Column, cy: i32) -> ChunkMesh {
        let reg = registry();
        let column = Arc::new(column);
        let region = LitRegion::compute(&reg, &std::array::from_fn(|_| column.clone()));
        mesh_chunk(&reg, &region, &column, cy)
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
}
