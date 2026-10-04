//! Sky and block light (4 bits each) for one column, flood-filled over a padded region.
//!
//! Light travels at most 15 blocks, so a column's light, plus the one-block border the
//! mesher samples, depends only on blocks within `LIGHT_PAD` of the column. Computing a
//! column from its 3×3 neighbourhood is therefore exact, needs no shared mutable state,
//! and runs on any worker thread. Edits relight only the columns within reach.

use std::sync::Arc;

use glam::IVec3;

use crate::block::{BlockId, BlockRegistry};
use crate::coords::{CHUNK_SIZE, CHUNK_VOLUME, COLUMN_CHUNKS, WORLD_HEIGHT};
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
        let mut attenuation = vec![1u8; total];
        let mut queue = Vec::new();

        region.copy_blocks(registry, columns, &mut attenuation, &mut queue);
        region.flood(&attenuation, &mut queue, 0);
        region.sky_light(&attenuation, &mut queue);
        region.flood(&attenuation, &mut queue, 4);
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
        let mut dense = vec![BlockId::AIR; CHUNK_VOLUME];
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

                chunk.copy_to_dense(&mut dense);
                for y in 0..CHUNK_SIZE {
                    for z in z_range.clone() {
                        let src = ((y * CHUNK_SIZE + z) * CHUNK_SIZE) as usize;
                        let row = index(0, y0 + y, z + to_region.z);
                        for x in x_range.clone() {
                            let block = dense[src + x as usize];
                            let info = registry.info(block);
                            let i = row + (x + to_region.x) as usize;
                            self.blocks[i] = block;
                            attenuation[i] = info.attenuation;
                            if info.emission > 0 {
                                self.light[i] = info.emission;
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
}
