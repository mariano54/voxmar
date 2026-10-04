//! Columns of chunks and the sparse map that holds the loaded world.

use std::sync::Arc;

use glam::IVec3;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::BlockId;
use crate::chunk::Chunk;
use crate::coords::{CHUNK_BITS, CHUNK_SIZE, COLUMN_CHUNKS, ColumnPos, in_world_height, local, local_index};

/// Per-column surface data, one entry per (x, z).
pub const COLUMN_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

/// A full-height stack of chunks plus the 2D data generators produce for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub chunks: [Chunk; COLUMN_CHUNKS as usize],
    /// Grass, foliage and water colours (sRGB) per (x, z), indexed `z * 32 + x`: the
    /// biomes' and the climate's, blended across borders.
    pub colors: Vec<[[u8; 3]; 3]>,
    /// Ground worn by feet, sparse and sorted by position. Only the server changes it.
    pub wear: Vec<Wear>,
}

/// Ground worn by feet at one (x, z) of a column: how much it has been walked on since it
/// last changed, how many stages of wear feet have brought it through (what healing may
/// undo; ground generated worn, such as a trail's tread, does not count), and when it was
/// last walked on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wear {
    /// `z * 32 + x`.
    pub at: u16,
    /// Steps, in quarters.
    pub steps: u8,
    pub stages: u8,
    /// Days since the world began.
    pub day: u32,
}

/// Colours for a column nothing has coloured: a mild green and blue.
pub const DEFAULT_COLORS: [[u8; 3]; 3] = [[124, 189, 107], [96, 161, 63], [63, 118, 228]];

impl Default for Column {
    fn default() -> Self {
        Self {
            chunks: Default::default(),
            colors: vec![DEFAULT_COLORS; COLUMN_AREA],
            wear: Vec::new(),
        }
    }
}

impl Column {
    /// `pos` is relative to the column origin (x, z in 0..32, y in world range).
    #[inline]
    pub fn block(&self, pos: IVec3) -> BlockId {
        self.chunks[(pos.y >> CHUNK_BITS) as usize].get(local_index(local(pos)))
    }

    pub fn set_block(&mut self, pos: IVec3, block: BlockId) {
        self.chunks[(pos.y >> CHUNK_BITS) as usize].set(local_index(local(pos)), block);
    }

    /// Grass, foliage and water colours at (x, z) relative to the column origin.
    pub fn colors_at(&self, x: i32, z: i32) -> [[u8; 3]; 3] {
        self.colors[(z * CHUNK_SIZE + x) as usize]
    }

    /// The wear at (x, z) relative to the column origin, if any.
    pub fn wear_at(&self, x: i32, z: i32) -> Option<&Wear> {
        let at = (z * CHUNK_SIZE + x) as u16;
        self.wear.binary_search_by_key(&at, |w| w.at).ok().map(|i| &self.wear[i])
    }

    /// The wear at (x, z), made if there is none yet.
    pub fn wear_mut(&mut self, x: i32, z: i32) -> &mut Wear {
        let at = (z * CHUNK_SIZE + x) as u16;
        let i = match self.wear.binary_search_by_key(&at, |w| w.at) {
            Ok(i) => i,
            Err(i) => {
                self.wear.insert(i, Wear { at, ..Wear::default() });
                i
            }
        };
        &mut self.wear[i]
    }
}

/// The loaded part of the world. Columns are shared immutably (`Arc`) so worker jobs can
/// read snapshots while the owner keeps editing through copy-on-write.
#[derive(Default)]
pub struct VoxelWorld {
    columns: FxHashMap<ColumnPos, Arc<Column>>,
}

impl VoxelWorld {
    pub fn column(&self, pos: ColumnPos) -> Option<&Arc<Column>> {
        self.columns.get(&pos)
    }

    pub fn contains(&self, pos: ColumnPos) -> bool {
        self.columns.contains_key(&pos)
    }

    pub fn insert(&mut self, pos: ColumnPos, column: Arc<Column>) {
        self.columns.insert(pos, column);
    }

    pub fn remove(&mut self, pos: ColumnPos) -> Option<Arc<Column>> {
        self.columns.remove(&pos)
    }

    pub fn columns(&self) -> impl Iterator<Item = (&ColumnPos, &Arc<Column>)> {
        self.columns.iter()
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Returns `None` if the position is in an unloaded column. Above the world is air,
    /// below it is reported as `None` too.
    pub fn block(&self, pos: IVec3) -> Option<BlockId> {
        if pos.y >= crate::coords::WORLD_HEIGHT {
            return Some(BlockId::AIR);
        }
        if pos.y < 0 {
            return None;
        }
        let column = self.columns.get(&ColumnPos::of_block(pos))?;
        Some(column.block(IVec3::new(pos.x & (CHUNK_SIZE - 1), pos.y, pos.z & (CHUNK_SIZE - 1))))
    }

    /// A loaded column to change, copied first if a job still reads it.
    pub fn column_mut(&mut self, pos: ColumnPos) -> Option<&mut Column> {
        self.columns.get_mut(&pos).map(Arc::make_mut)
    }

    /// Sets a block, returning the previous one. Fails if the column is not loaded.
    pub fn set_block(&mut self, pos: IVec3, block: BlockId) -> Option<BlockId> {
        if !in_world_height(pos.y) {
            return None;
        }
        let column = Arc::make_mut(self.columns.get_mut(&ColumnPos::of_block(pos))?);
        let local = IVec3::new(pos.x & (CHUNK_SIZE - 1), pos.y, pos.z & (CHUNK_SIZE - 1));
        let previous = column.block(local);
        column.set_block(local, block);
        Some(previous)
    }
}
