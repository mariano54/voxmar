//! World, column and chunk coordinate spaces.
//!
//! The world is infinite horizontally and `WORLD_HEIGHT` blocks tall. It is stored as
//! columns of `COLUMN_CHUNKS` cubic chunks of `CHUNK_SIZE`³ blocks each.

use glam::{IVec2, IVec3};
use serde::{Deserialize, Serialize};

pub const CHUNK_BITS: i32 = 5;
pub const CHUNK_SIZE: i32 = 1 << CHUNK_BITS;
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;
pub const COLUMN_CHUNKS: i32 = 8;
pub const WORLD_HEIGHT: i32 = CHUNK_SIZE * COLUMN_CHUNKS;

/// Position of a chunk column, in column units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColumnPos(pub IVec2);

/// Position of a chunk, in chunk units. `y` is in `0..COLUMN_CHUNKS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkPos(pub IVec3);

impl ColumnPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self(IVec2::new(x, z))
    }

    pub fn of_block(pos: IVec3) -> Self {
        Self::new(pos.x >> CHUNK_BITS, pos.z >> CHUNK_BITS)
    }

    /// World position of the column's minimum corner at y = 0.
    pub fn origin(self) -> IVec3 {
        IVec3::new(self.0.x << CHUNK_BITS, 0, self.0.y << CHUNK_BITS)
    }

    pub fn chunk(self, y: i32) -> ChunkPos {
        ChunkPos(IVec3::new(self.0.x, y, self.0.y))
    }

    pub fn offset(self, dx: i32, dz: i32) -> Self {
        Self::new(self.0.x + dx, self.0.y + dz)
    }

    /// Chebyshev distance in columns.
    pub fn distance(self, other: Self) -> i32 {
        (self.0 - other.0).abs().max_element()
    }
}

impl ChunkPos {
    pub fn of_block(pos: IVec3) -> Self {
        Self(pos >> CHUNK_BITS)
    }

    pub fn column(self) -> ColumnPos {
        ColumnPos::new(self.0.x, self.0.z)
    }

    pub fn origin(self) -> IVec3 {
        self.0 << CHUNK_BITS
    }
}

/// Block position relative to its chunk, each component in `0..CHUNK_SIZE`.
#[inline]
pub fn local(pos: IVec3) -> IVec3 {
    pos & (CHUNK_SIZE - 1)
}

/// Index of a local position inside a chunk's block array (y-major, then z, then x).
#[inline]
pub fn local_index(local: IVec3) -> usize {
    ((local.y << (2 * CHUNK_BITS)) | (local.z << CHUNK_BITS) | local.x) as usize
}

pub fn in_world_height(y: i32) -> bool {
    (0..WORLD_HEIGHT).contains(&y)
}

/// The six axis-aligned face directions. The discriminant is the face index used by
/// meshing and the shaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Face {
    PosX = 0,
    NegX = 1,
    PosY = 2,
    NegY = 3,
    PosZ = 4,
    NegZ = 5,
}

impl Face {
    pub const ALL: [Face; 6] = [Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ];

    pub const fn normal(self) -> IVec3 {
        match self {
            Face::PosX => IVec3::X,
            Face::NegX => IVec3::NEG_X,
            Face::PosY => IVec3::Y,
            Face::NegY => IVec3::NEG_Y,
            Face::PosZ => IVec3::Z,
            Face::NegZ => IVec3::NEG_Z,
        }
    }

    pub const fn opposite(self) -> Face {
        match self {
            Face::PosX => Face::NegX,
            Face::NegX => Face::PosX,
            Face::PosY => Face::NegY,
            Face::NegY => Face::PosY,
            Face::PosZ => Face::NegZ,
            Face::NegZ => Face::PosZ,
        }
    }

    pub fn from_normal(n: IVec3) -> Option<Face> {
        Face::ALL.into_iter().find(|f| f.normal() == n)
    }
}
