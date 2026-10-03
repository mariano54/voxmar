//! Chunk visibility graph (occlusion culling through caves and terrain).
//!
//! Each chunk records which pairs of its faces are connected through non-occluding
//! cells. A breadth-first walk from the camera's chunk that only crosses connected faces
//! and never turns back reaches every chunk that could possibly be seen; everything else
//! (most of the underground) is skipped before any draw call is issued.

use std::collections::VecDeque;

use glam::IVec3;
use rustc_hash::FxHashSet;

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

/// Returns every chunk potentially visible from `camera`, walking at most `radius` chunks
/// horizontally. `connectivity` returns `None` for chunks with no data, which are treated
/// as open space.
pub fn visible_chunks(
    camera: IVec3,
    radius: i32,
    connectivity: impl Fn(ChunkPos) -> Option<Connectivity>,
) -> FxHashSet<ChunkPos> {
    let start = ChunkPos(camera.div_euclid(IVec3::splat(CHUNK_SIZE)).clamp(
        IVec3::new(i32::MIN, 0, i32::MIN),
        IVec3::new(i32::MAX, COLUMN_CHUNKS - 1, i32::MAX),
    ));
    let mut visible = FxHashSet::default();
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
            let next = ChunkPos(chunk.0 + face.normal());
            if !(0..COLUMN_CHUNKS).contains(&next.0.y)
                || (next.0.x - start.0.x).abs() > radius
                || (next.0.z - start.0.z).abs() > radius
                || !visible.insert(next)
            {
                continue;
            }
            queue.push_back((next, Some(face.opposite()), travelled | 1 << face as u8));
        }
    }
    visible
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
        assert!(visible.contains(&ChunkPos(IVec3::new(1, 0, 0))));
        assert!(!visible.contains(&ChunkPos(IVec3::new(2, 0, 0))));
    }
}
