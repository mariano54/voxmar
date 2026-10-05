//! Chunk visibility graph (occlusion culling through caves and terrain).
//!
//! Each chunk records which pairs of its faces are connected through non-occluding
//! cells. A breadth-first walk from the camera's chunk that only crosses connected faces
//! and never turns back reaches every chunk that could possibly be seen; everything else
//! (most of the underground) is skipped before any draw call is issued.

use std::collections::VecDeque;

use glam::{IVec2, IVec3};

use crate::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ChunkPos, Face};

/// Symmetric 6×6 face connectivity matrix, bit `a * 6 + b`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Connectivity(u64);

impl Connectivity {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self((1 << 36) - 1);

    pub fn connects(self, a: Face, b: Face) -> bool {
        self.0 & (1 << (a as u32 * 6 + b as u32)) != 0
    }

    /// Flood-fills the open cells of a chunk. `occludes(x, y, z)` takes local coordinates.
    pub fn compute(occludes: impl Fn(usize, usize, usize) -> bool) -> Self {
        const S: usize = CHUNK_SIZE as usize;
        let mut visited = vec![false; S * S * S];
        let mut any_wall = false;
        for (i, v) in visited.iter_mut().enumerate() {
            if occludes(i % S, i / (S * S), (i / S) % S) {
                *v = true;
                any_wall = true;
            }
        }
        if !any_wall {
            return Self::ALL;
        }

        let mut result = 0u64;
        let mut stack = Vec::new();
        for start in 0..visited.len() {
            if visited[start] {
                continue;
            }
            visited[start] = true;
            stack.push(start);
            let mut faces = 0u8;
            while let Some(i) = stack.pop() {
                let (x, y, z) = (i % S, i / (S * S), (i / S) % S);
                let edges = [
                    (x == S - 1, Face::PosX, i.wrapping_add(1)),
                    (x == 0, Face::NegX, i.wrapping_sub(1)),
                    (y == S - 1, Face::PosY, i.wrapping_add(S * S)),
                    (y == 0, Face::NegY, i.wrapping_sub(S * S)),
                    (z == S - 1, Face::PosZ, i.wrapping_add(S)),
                    (z == 0, Face::NegZ, i.wrapping_sub(S)),
                ];
                for (at_edge, face, next) in edges {
                    if at_edge {
                        faces |= 1 << face as u8;
                    } else if !visited[next] {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
            for a in 0..6 {
                for b in 0..6 {
                    if faces & (1 << a) != 0 && faces & (1 << b) != 0 {
                        result |= 1 << (a * 6 + b);
                    }
                }
            }
        }
        Self(result)
    }
}

/// The chunks a walk from the camera reached: a bit per chunk over the square of columns
/// it could reach, so asking about a chunk costs an index rather than a hash.
#[derive(Clone, Debug, Default)]
pub struct VisibleChunks {
    /// The square's corner column (least x and z), in chunks.
    min: IVec2,
    /// Columns along each side of the square.
    side: i32,
    /// One word per column, a bit per chunk from the bottom.
    columns: Vec<u16>,
}

const _: () = assert!(COLUMN_CHUNKS <= 16, "a column's chunks fit a u16");

impl VisibleChunks {
    fn around(centre: IVec2, radius: i32) -> Self {
        let side = 2 * radius + 1;
        Self { min: centre - IVec2::splat(radius), side, columns: vec![0; (side * side) as usize] }
    }

    /// The column word and bit of `chunk`, if it lies within the square.
    fn slot(&self, chunk: ChunkPos) -> Option<(usize, u16)> {
        let at = IVec2::new(chunk.0.x, chunk.0.z) - self.min;
        let inside = at.cmpge(IVec2::ZERO).all() && at.cmplt(IVec2::splat(self.side)).all() && (0..COLUMN_CHUNKS).contains(&chunk.0.y);
        inside.then(|| ((at.y * self.side + at.x) as usize, 1 << chunk.0.y))
    }

    pub fn contains(&self, chunk: ChunkPos) -> bool {
        self.slot(chunk).is_some_and(|(column, bit)| self.columns[column] & bit != 0)
    }

    /// Marks `chunk` reached, telling whether it was not already.
    fn insert(&mut self, chunk: ChunkPos) -> bool {
        let Some((column, bit)) = self.slot(chunk) else { return false };
        let new = self.columns[column] & bit == 0;
        self.columns[column] |= bit;
        new
    }

    pub fn len(&self) -> usize {
        self.columns.iter().map(|c| c.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.iter().all(|&c| c == 0)
    }
}

/// Returns every chunk potentially visible from `camera`, walking at most `radius` chunks
/// horizontally. `connectivity` returns `None` for chunks with no data, which are treated
/// as open space.
pub fn visible_chunks(
    camera: IVec3,
    radius: i32,
    connectivity: impl Fn(ChunkPos) -> Option<Connectivity>,
) -> VisibleChunks {
    let start = ChunkPos(camera.div_euclid(IVec3::splat(CHUNK_SIZE)).clamp(
        IVec3::new(i32::MIN, 0, i32::MIN),
        IVec3::new(i32::MAX, COLUMN_CHUNKS - 1, i32::MAX),
    ));
    let mut visible = VisibleChunks::around(IVec2::new(start.0.x, start.0.z), radius);
    let mut queue = VecDeque::new();
    visible.insert(start);
    // (chunk, face it was entered through, directions travelled so far)
    queue.push_back((start, None::<Face>, 0u8));

    while let Some((chunk, entered, travelled)) = queue.pop_front() {
        let conn = connectivity(chunk).unwrap_or(Connectivity::ALL);
        for face in Face::ALL {
            // Never step back against a direction already travelled.
            if travelled & (1 << face.opposite() as u8) != 0 {
                continue;
            }
            if let Some(entered) = entered
                && !conn.connects(entered, face)
            {
                continue;
            }
            // Outside the square (or the column) is out of reach.
            let next = ChunkPos(chunk.0 + face.normal());
            if !visible.insert(next) {
                continue;
            }
            queue.push_back((next, Some(face.opposite()), travelled | 1 << face as u8));
        }
    }
    visible
}

/// `visible_chunks` for the chunks `chunks` lists with their connectivity, looked up from
/// a grid over the square the walk can reach rather than chunk by chunk from a map.
pub fn visible_among(camera: IVec3, radius: i32, chunks: impl IntoIterator<Item = (ChunkPos, Connectivity)>) -> VisibleChunks {
    let centre = camera.div_euclid(IVec3::splat(CHUNK_SIZE));
    let min = IVec2::new(centre.x, centre.z) - IVec2::splat(radius);
    let side = 2 * radius + 1;
    let index = |chunk: ChunkPos| {
        let at = IVec2::new(chunk.0.x, chunk.0.z) - min;
        let inside = at.cmpge(IVec2::ZERO).all() && at.cmplt(IVec2::splat(side)).all() && (0..COLUMN_CHUNKS).contains(&chunk.0.y);
        inside.then(|| ((at.y * side + at.x) * COLUMN_CHUNKS + chunk.0.y) as usize)
    };
    let mut grid = vec![None; (side * side * COLUMN_CHUNKS) as usize];
    for (chunk, connectivity) in chunks {
        if let Some(i) = index(chunk) {
            grid[i] = Some(connectivity);
        }
    }
    visible_chunks(camera, radius, |chunk| index(chunk).and_then(|i| grid[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_splits_chunk() {
        // A solid plane at x = 10 separates -X from +X.
        let conn = Connectivity::compute(|x, _, _| x == 10);
        assert!(!conn.connects(Face::NegX, Face::PosX));
        assert!(conn.connects(Face::NegX, Face::PosY));
        assert!(conn.connects(Face::PosX, Face::PosZ));
    }

    #[test]
    fn solid_chunk_blocks_walk() {
        let visible = visible_chunks(IVec3::new(16, 16, 16), 2, |pos| {
            (pos.0.x == 1).then_some(Connectivity::NONE)
        });
        assert!(visible.contains(ChunkPos(IVec3::new(1, 0, 0))));
        assert!(!visible.contains(ChunkPos(IVec3::new(2, 0, 0))));
    }

    #[test]
    fn walking_a_grid_reaches_what_walking_a_map_does() {
        // Scattered walls and caves around a camera, some chunks without data, and some
        // listed beyond the walk's reach.
        let mut chunks = rustc_hash::FxHashMap::default();
        for x in -6..=6 {
            for z in -6..=6 {
                for y in 0..COLUMN_CHUNKS {
                    let h = (x * 73 + z * 151 + y * 37).rem_euclid(11);
                    if h == 0 {
                        continue;
                    }
                    let conn = match h {
                        1..=3 => Connectivity::NONE,
                        4 => Connectivity::compute(|x, _, _| x == 7),
                        5 => Connectivity::compute(|_, y, _| y == 3),
                        _ => Connectivity::ALL,
                    };
                    chunks.insert(ChunkPos(IVec3::new(x, y, z)), conn);
                }
            }
        }
        let camera = IVec3::new(5, 70, -9);
        let radius = 4;
        let walked = visible_chunks(camera, radius, |pos| chunks.get(&pos).copied());
        let gridded = visible_among(camera, radius, chunks.iter().map(|(&pos, &conn)| (pos, conn)));
        assert_eq!(walked.len(), gridded.len());
        assert!(walked.len() > 100);
        for &pos in chunks.keys() {
            assert_eq!(walked.contains(pos), gridded.contains(pos), "{pos:?}");
        }
        // Nothing outside the square, and nothing below or above the column.
        assert!(!gridded.contains(ChunkPos(IVec3::new(0 + radius + 1, 4, 0))));
        assert!(!gridded.contains(ChunkPos(IVec3::new(0, -1, 0))));
    }
}
