//! Sky and block light (4 bits each) for one column, flood-filled over a padded region.
//!
//! Light travels at most 15 blocks, so a column's light, plus the one-block border the
//! mesher samples, depends only on blocks within `LIGHT_PAD` of the column. Computing a
//! column from its 3×3 neighbourhood is therefore exact, needs no shared mutable state,
//! and runs on any worker thread. Edits relight only the columns within reach.

use std::cell::RefCell;
use std::sync::Arc;

use glam::IVec3;

use crate::block::{BlockId, BlockRegistry};
use crate::coords::{CHUNK_SIZE, COLUMN_CHUNKS, WORLD_HEIGHT};
use crate::world::Column;

/// Horizontal padding around the column: light reach plus the mesher's border.
pub const LIGHT_PAD: i32 = 17;
const W: i32 = CHUNK_SIZE + 2 * LIGHT_PAD;
const H: i32 = WORLD_HEIGHT;
const PLANE: usize = (W * W) as usize;

pub const MAX_LIGHT: u8 = 15;

/// A column and its eight neighbours, indexed `(dz + 1) * 3 + (dx + 1)`.
pub type Neighborhood = [Arc<Column>; 9];

/// Blocks and light of a column plus `LIGHT_PAD` blocks around it.
///
/// Coordinates passed to accessors are relative to the column origin, so the column itself
/// spans x, z in `0..32` and y in `0..WORLD_HEIGHT`.
pub struct LitRegion {
    blocks: Vec<BlockId>,
    /// Sky light in the high nibble, block light in the low nibble.
    light: Vec<u8>,
}

#[inline]
fn index(x: i32, y: i32, z: i32) -> usize {
    ((y * W + z) * W + x) as usize
}

impl LitRegion {
    pub fn compute(registry: &BlockRegistry, columns: &Neighborhood) -> Self {
        let _span = tracing::info_span!("light_column").entered();
        let total = PLANE * H as usize;
        let mut region = Self { blocks: vec![BlockId::AIR; total], light: vec![0; total] };
        thread_local! {
            /// Attenuation and the flood's queue, kept between regions: freshly allocated,
            /// their megabytes cost as much in page faults as the light itself.
            static SCRATCH: RefCell<(Vec<u8>, Vec<u32>)> = const { RefCell::new((Vec::new(), Vec::new())) };
        }
        SCRATCH.with_borrow_mut(|(attenuation, queue)| {
            attenuation.clear();
            attenuation.resize(total, 1);
            queue.clear();
            region.copy_blocks(registry, columns, attenuation, queue);
            region.flood(attenuation, queue, 0);
            region.sky_light(attenuation, queue);
            region.flood(attenuation, queue, 4);
        });
        region
    }

    /// Block at a column-relative position. Above the world is air; below is `None`.
    #[inline]
    pub fn block(&self, pos: IVec3) -> Option<BlockId> {
        match pos.y {
            y if y >= H => Some(BlockId::AIR),
            y if y < 0 => None,
            _ => Some(self.blocks[index(pos.x + LIGHT_PAD, pos.y, pos.z + LIGHT_PAD)]),
        }
    }

    /// Blocks and light of `len` cells along x from column-relative (x, y, z), with y
    /// inside the world.
    #[inline]
    pub fn row(&self, x: i32, y: i32, z: i32, len: usize) -> (&[BlockId], &[u8]) {
        let start = index(x + LIGHT_PAD, y, z + LIGHT_PAD);
        (&self.blocks[start..start + len], &self.light[start..start + len])
    }

    /// Packed light (sky << 4 | block) at a column-relative position.
    #[inline]
    pub fn light(&self, pos: IVec3) -> u8 {
        match pos.y {
            y if y >= H => MAX_LIGHT << 4,
            y if y < 0 => 0,
            _ => self.light[index(pos.x + LIGHT_PAD, pos.y, pos.z + LIGHT_PAD)],
        }
    }

    /// Copies blocks into the region, fills per-cell attenuation and queues light emitters.
    fn copy_blocks(
        &mut self,
        registry: &BlockRegistry,
        columns: &Neighborhood,
        attenuation: &mut [u8],
        queue: &mut Vec<u32>,
    ) {
        // A palette's blocks with their attenuation and emission, and a row of indices.
        let mut looks: Vec<(BlockId, u8, u8)> = Vec::new();
        let mut indices = [0u16; CHUNK_SIZE as usize];
        for (n, column) in columns.iter().enumerate() {
            let (dx, dz) = (n as i32 % 3 - 1, n as i32 / 3 - 1);
            // Overlap of this neighbour with the region, in neighbour-local coordinates.
            let x_range = local_overlap(dx);
            let z_range = local_overlap(dz);
            let to_region = IVec3::new(dx * CHUNK_SIZE + LIGHT_PAD, 0, dz * CHUNK_SIZE + LIGHT_PAD);

            for (cy, chunk) in column.chunks.iter().enumerate() {
                let y0 = cy as i32 * CHUNK_SIZE;
                if let Some(block) = chunk.as_uniform() {
                    let att = registry.info(block).attenuation;
                    let emits = registry.info(block).emission > 0;
                    for y in y0..y0 + CHUNK_SIZE {
                        for z in z_range.clone() {
                            let row = index(x_range.start + to_region.x, y, z + to_region.z);
                            let len = x_range.len();
                            self.blocks[row..row + len].fill(block);
                            attenuation[row..row + len].fill(att);
                            if emits {
                                for i in row..row + len {
                                    self.light[i] = registry.info(block).emission;
                                    queue.push(i as u32);
                                }
                            }
                        }
                    }
                    continue;
                }

                // Only the rows the region needs, decoded straight from the packed indices,
                // with each block of the palette looked up once.
                looks.clear();
                looks.extend(chunk.palette().iter().map(|&block| (block, registry.info(block).attenuation, registry.info(block).emission)));
                let indices = &mut indices[..x_range.len()];
                for y in 0..CHUNK_SIZE {
                    for z in z_range.clone() {
                        chunk.indices(((y * CHUNK_SIZE + z) * CHUNK_SIZE + x_range.start) as usize, indices);
                        let row = index(x_range.start + to_region.x, y0 + y, z + to_region.z);
                        for (i, &look) in (row..).zip(indices.iter()) {
                            let (block, att, emission) = looks[look as usize];
                            self.blocks[i] = block;
                            attenuation[i] = att;
                            if emission > 0 {
                                self.light[i] = emission;
                                queue.push(i as u32);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Straight-down sky light, then queues cells where it can spread sideways.
    fn sky_light(&mut self, attenuation: &[u8], queue: &mut Vec<u32>) {
        // Lowest y reached by full-strength sky light, per (x, z).
        let mut floor = vec![H; PLANE];
        // Down through the planes that hold nothing but clear air (most of the sky), the
        // light stays at full strength everywhere.
        let mut y = H;
        while y > 0 && attenuation[(y - 1) as usize * PLANE..y as usize * PLANE].as_chunks::<{ W as usize }>().0.iter().all(|row| row.iter().fold(0, |odd, &a| odd | (a ^ 1)) == 0) {
            y -= 1;
            for light in &mut self.light[y as usize * PLANE..(y + 1) as usize * PLANE] {
                *light |= MAX_LIGHT << 4;
            }
            floor.fill(y);
        }
        // Then plane by plane, each (x, z) until its light is spent or blocked; 0 marks one
        // that has stopped. Whether it is still at full strength, per (x, z).
        let mut level = vec![MAX_LIGHT; PLANE];
        let mut full = vec![true; PLANE];
        for y in (0..y).rev() {
            let plane = y as usize * PLANE;
            let (light, attenuation) = (&mut self.light[plane..plane + PLANE], &attenuation[plane..plane + PLANE]);
            let mut lit = false;
            for i in 0..PLANE {
                if level[i] == 0 {
                    continue;
                }
                let att = attenuation[i];
                level[i] = if att >= MAX_LIGHT { 0 } else { level[i].saturating_sub(att - 1) };
                if level[i] == 0 {
                    continue;
                }
                light[i] |= level[i] << 4;
                lit = true;
                full[i] &= level[i] == MAX_LIGHT;
                if full[i] {
                    floor[i] = y;
                }
            }
            if !lit {
                break;
            }
        }

        // Only cells next to a column that is shaded at the same height can spread light.
        for z in 0..W {
            for x in 0..W {
                let own = floor[(z * W + x) as usize];
                let mut highest = own;
                for (nx, nz) in [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)] {
                    if (0..W).contains(&nx) && (0..W).contains(&nz) {
                        highest = highest.max(floor[(nz * W + nx) as usize]);
                    }
                }
                for y in own..highest.min(H) {
                    queue.push(index(x, y, z) as u32);
                }
            }
        }
    }

    /// Breadth-first propagation of the nibble at `shift` from every queued cell.
    fn flood(&mut self, attenuation: &[u8], queue: &mut Vec<u32>, shift: u8) {
        let mut head = 0;
        while head < queue.len() {
            let i = queue[head] as usize;
            head += 1;
            let level = (self.light[i] >> shift) & 0xF;
            if level <= 1 {
                continue;
            }
            let x = (i % W as usize) as i32;
            let z = ((i / W as usize) % W as usize) as i32;
            let y = (i / PLANE) as i32;
            let neighbours = [
                (x > 0, i.wrapping_sub(1)),
                (x < W - 1, i + 1),
                (z > 0, i.wrapping_sub(W as usize)),
                (z < W - 1, i + W as usize),
                (y > 0, i.wrapping_sub(PLANE)),
                (y < H - 1, i + PLANE),
            ];
            for (inside, n) in neighbours {
                if !inside {
                    continue;
                }
                let new = level.saturating_sub(attenuation[n]);
                if new > (self.light[n] >> shift) & 0xF {
                    self.light[n] = (self.light[n] & !(0xF << shift)) | (new << shift);
                    queue.push(n as u32);
                }
            }
        }
        queue.clear();
    }
}

/// Neighbour-local x (or z) range that falls inside the padded region.
fn local_overlap(d: i32) -> std::ops::Range<i32> {
    match d {
        -1 => CHUNK_SIZE - LIGHT_PAD..CHUNK_SIZE,
        0 => 0..CHUNK_SIZE,
        _ => 0..LIGHT_PAD,
    }
}

const _: () = assert!(LIGHT_PAD < CHUNK_SIZE && COLUMN_CHUNKS > 0);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockDef, RenderKind, Textures};

    fn registry() -> BlockRegistry {
        BlockRegistry::new(vec![
            BlockDef { name: "test:stone".into(), textures: Textures::All("stone".into()), ..Default::default() },
            BlockDef { name: "test:torch".into(), render: RenderKind::Cutout, light: 14, ..Default::default() },
        ])
        .unwrap()
    }

    fn neighborhood(column: Column) -> Neighborhood {
        let column = Arc::new(column);
        std::array::from_fn(|_| column.clone())
    }

    #[test]
    fn open_sky_is_full_and_caves_are_dark() {
        let reg = registry();
        let mut column = Column::default();
        for y in 0..64 {
            for z in 0..32 {
                for x in 0..32 {
                    column.set_block(IVec3::new(x, y, z), reg.expect("test:stone"));
                }
            }
        }
        column.set_block(IVec3::new(5, 30, 5), BlockId::AIR);
        let region = LitRegion::compute(&reg, &neighborhood(column));
        assert_eq!(region.light(IVec3::new(5, 100, 5)) >> 4, 15);
        assert_eq!(region.light(IVec3::new(5, 30, 5)), 0);
    }

    #[test]
    fn block_light_falls_off_one_per_block() {
        let reg = registry();
        let mut column = Column::default();
        column.set_block(IVec3::new(16, 100, 16), reg.expect("test:torch"));
        let region = LitRegion::compute(&reg, &neighborhood(column));
        assert_eq!(region.light(IVec3::new(16, 100, 16)) & 0xF, 14);
        assert_eq!(region.light(IVec3::new(20, 100, 16)) & 0xF, 10);
        // Light crosses into the neighbouring column's border.
        assert_eq!(region.light(IVec3::new(32, 100, 16)) & 0xF, 0);
        assert_eq!(region.light(IVec3::new(16, 100, 29)) & 0xF, 1);
    }

    /// The game's windows let light into a room walled in stone: daylight down a shaft
    /// outside the window and a torch set before it both reach the room through a lattice,
    /// a shuttered lattice left open, an arrow slit or an empty frame. Stone, or the shutters
    /// shut, keep the room dark.
    #[test]
    fn daylight_and_torchlight_come_in_through_a_window() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let reg = &content.blocks;
        let (stone, torch) = (reg.expect("base:stone"), reg.expect("base:torch"));
        // Stone to y 120; a three-block room behind a wall at x 11, the window in it, and
        // outside a cell at x 12 open up a shaft to the sky.
        let (window, outside, room) = (IVec3::new(11, 101, 9), IVec3::new(12, 101, 9), IVec3::new(10, 101, 9));
        let mut column = Column::default();
        for y in 0..120 {
            for z in 0..32 {
                for x in 0..32 {
                    column.set_block(IVec3::new(x, y, z), stone);
                }
            }
        }
        for x in 8..=10 {
            column.set_block(IVec3::new(x, 101, 9), BlockId::AIR);
        }
        for y in 101..120 {
            column.set_block(outside.with_y(y), BlockId::AIR);
        }
        let lit = |column: &Column, at: IVec3| LitRegion::compute(reg, &neighborhood(column.clone())).light(at);
        let emission = reg.info(torch).emission;
        for name in ["base:oak_lattice_window", "base:oak_shuttered_window", "base:stone_arrow_slit", "base:oak_window_frame", "base:stone", "base:oak_shuttered_window_shut"] {
            let through = !matches!(name, "base:stone" | "base:oak_shuttered_window_shut");
            let mut column = column.clone();
            column.set_block(window, reg.expect(name));
            let day = lit(&column, room) >> 4;
            column.set_block(outside.with_y(102), stone);
            column.set_block(outside, torch);
            let night = lit(&column, room);
            assert_eq!(night >> 4, 0, "{name}: the shaft is capped, no daylight left");
            match through {
                true => {
                    assert_eq!(day, 13, "{name}: daylight through the window, one less a cell");
                    assert_eq!(night & 0xF, emission - 2, "{name}: torchlight through the window");
                }
                false => assert_eq!((day, night & 0xF), (0, 0), "{name} let light into the room"),
            }
        }
    }
}
