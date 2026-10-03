//! Binary greedy meshing with smooth lighting and ambient occlusion.
//!
//! Face culling is done 64 cells at a time on bit columns. Visible faces are then merged
//! per slice into the largest rectangles whose faces share texture, tint, AO and light, so
//! lighting stays exact while open terrain collapses to a handful of quads.
//!
//! Each vertex is packed into 8 bytes:
//!
//! ```text
//! word 0: x:6 | y:6 | z:6 | face:3 | ao:2
//! word 1: layer:12 | tint:2 | temperature:5 | humidity:5 | sky:4 | block:4
//! ```

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
    pub opaque: MeshBuffers,
    pub translucent: MeshBuffers,
    pub connectivity: Connectivity,
}

/// Per-cell flags of the padded chunk, derived from the registry.
#[derive(Clone, Copy, Default)]
struct Cell {
    block: BlockId,
    occludes: bool,
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
                cells[pidx(x, y, z)] = Cell { block, occludes: info_occludes, light: region.light(pos) };
                if info_occludes {
                    occ.set(x, y, z);
                }
                match render {
                    RenderKind::Opaque => opaque.set(x, y, z),
                    RenderKind::Cutout => {
                        opaque.set(x, y, z);
                        cutout.set(x, y, z);
                    }
                    RenderKind::Translucent => translucent.set(x, y, z),
                    RenderKind::Invisible => {}
                }
            }
        }
    }

    let mut mesher = Mesher { registry, column, cells: &cells, planes: [[0; S]; S], keys: [[0; S]; S] };
    let mut mesh = ChunkMesh { connectivity: Connectivity::compute(|x, y, z| cells[pidx(x + 1, y + 1, z + 1)].occludes), ..Default::default() };
    for face in Face::ALL {
        mesher.mesh_faces(face, &occ, &opaque, &cutout, &mut mesh.opaque);
        mesher.mesh_faces(face, &occ, &translucent, &translucent, &mut mesh.translucent);
    }
    mesh
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
    /// `planes[slice][row]`: bit `col` set where a face is visible.
    planes: [[u32; S]; S],
    keys: [[u64; S]; S],
}

impl Mesher<'_> {
    /// Meshes the faces of `visible` cells along `face`. A face is hidden by an occluding
    /// neighbour, or when both cells belong to `joined` (water next to water, leaves next
    /// to leaves), whose shared faces are never seen.
    fn mesh_faces(&mut self, face: Face, occ: &Axes, visible: &Axes, joined: &Axes, out: &mut MeshBuffers) {
        let fa = face_axes(face);
        self.planes = [[0; S]; S];
        let mut any = false;

        // Cull faces a whole column of cells at a time with bit operations.
        let (occ_cols, vis_cols, joined_cols) = match fa.normal {
            0 => (&occ.x, &visible.x, &joined.x),
            1 => (&occ.y, &visible.y, &joined.y),
            _ => (&occ.z, &visible.z, &joined.z),
        };
        let neighbour = |bits: u64| if fa.positive { bits >> 1 } else { bits << 1 };
        for a in 1..=S {
            for b in 1..=S {
                let joined = joined_cols[a][b];
                let hidden = neighbour(occ_cols[a][b]) | (joined & neighbour(joined));
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

    /// Everything that must match for two faces to merge, packed into 64 bits:
    /// layer:12 tint:2 temperature:5 humidity:5 | ao 4×2 | light 4×8.
    fn face_key(&self, face: Face, fa: &FaceAxes, slice: usize, row: usize, col: usize) -> u64 {
        let pos = Self::cell(fa, slice, row, col);
        let info = self.registry.info(self.padded(pos).block);
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
            let both_sides = side_u.occludes && side_v.occludes;
            let corner_ao = if both_sides {
                0
            } else {
                3 - (side_u.occludes as u64 + side_v.occludes as u64 + diagonal.occludes as u64)
            };
            ao |= corner_ao << (corner * 2);

            // Smooth light: average of the open cells touching this vertex. The diagonal
            // is unreachable when both sides are walls.
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
        surface | ao << 24 | light << 32
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
    key: u64,
) {
    let plane = slice as i32 + fa.positive as i32;
    let corners = [(col, row), (col + width, row), (col + width, row + height), (col, row + height)];
    let surface = (key & 0xFF_FFFF) as u32;
    let base = out.vertices.len() as u32;
    let mut ao = [0u32; 4];
    for (i, (c, r)) in corners.into_iter().enumerate() {
        let mut p = IVec3::ZERO;
        p[fa.normal] = plane;
        p[fa.row] = r as i32;
        p[fa.col] = c as i32;
        ao[i] = ((key >> (24 + i * 2)) & 3) as u32;
        let light = ((key >> (32 + i * 8)) & 0xFF) as u32;
        let w0 = p.x as u32 | (p.y as u32) << 6 | (p.z as u32) << 12 | (face as u32) << 18 | ao[i] << 21;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::block::{BlockDef, Textures};

    fn registry() -> BlockRegistry {
        BlockRegistry::new(vec![
            BlockDef { name: "test:stone".into(), textures: Textures::All("stone".into()), ..Default::default() },
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
}
