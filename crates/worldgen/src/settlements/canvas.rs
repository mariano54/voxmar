//! Where a settlement's blocks are drawn before they go into columns, and the frame that
//! turns a building's own coordinates into the world's.

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::glam::{IVec2, IVec3};

use crate::rng::hash3;

/// How a block goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Put {
    /// Replaces whatever is there.
    Set(BlockId),
    /// Only into air, water, leaves or a small plant.
    Soft(BlockId),
    /// Empties the place, whatever is there.
    Clear,
    /// Fills down from here through air, water, leaves and plants, to at most ten blocks,
    /// so a post or a foundation reaches the ground.
    Down(BlockId),
}

/// One block of a plan: at an absolute height, or (`ground`) at a height over the ground
/// of its column as generation finds it, 0 being the ground's top block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Voxel {
    pub pos: IVec3,
    pub ground: bool,
    pub put: Put,
}

/// Blocks drawn so far, the last drawn at a place winning.
#[derive(Default)]
pub struct Canvas {
    pub absolute: FxHashMap<IVec3, Put>,
    pub grounded: FxHashMap<IVec3, Put>,
}

impl Canvas {
    pub fn put(&mut self, pos: IVec3, put: Put) {
        self.absolute.insert(pos, put);
    }

    pub fn set(&mut self, pos: IVec3, block: BlockId) {
        self.put(pos, Put::Set(block));
    }

    /// At `up` blocks over the ground of (x, z).
    pub fn on_ground(&mut self, x: i32, z: i32, up: i32, put: Put) {
        self.grounded.insert(IVec3::new(x, up, z), put);
    }

    /// Every block, sorted into the columns that hold them, in a fixed order.
    pub fn into_columns(self) -> FxHashMap<IVec2, Vec<Voxel>> {
        let mut columns: FxHashMap<IVec2, Vec<Voxel>> = FxHashMap::default();
        let mut all: Vec<Voxel> = self
            .absolute
            .into_iter()
            .map(|(pos, put)| Voxel { pos, ground: false, put })
            .chain(self.grounded.into_iter().map(|(pos, put)| Voxel { pos, ground: true, put }))
            .collect();
        // Clearing goes first, then from the bottom up, so what is built is never cleared
        // and posts fill down onto what is already there.
        // Each map has one voxel per position, and `ground` distinguishes the maps,
        // so no keys tie. An unstable sort gives the same order without merge storage.
        all.sort_unstable_by_key(|v| (v.ground, !matches!(v.put, Put::Clear), v.pos.y, v.pos.x, v.pos.z));
        for v in all {
            columns.entry(IVec2::new(v.pos.x, v.pos.z).div_euclid(IVec2::splat(voxmar_engine::coords::CHUNK_SIZE))).or_default().push(v);
        }
        columns
    }
}

/// Which way a building's front faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Facing {
    North,
    South,
    West,
    East,
}

impl Facing {
    /// The facing nearest the direction `d` (x, z).
    pub fn toward(d: voxmar_engine::glam::Vec2) -> Self {
        if d.x.abs() > d.y.abs() {
            if d.x > 0.0 { Facing::East } else { Facing::West }
        } else if d.y > 0.0 {
            Facing::South
        } else {
            Facing::North
        }
    }

    /// A building's own (x, z), its front at z = 0 looking towards −z, in the world with
    /// its own origin at (0, 0).
    pub fn turn(self, x: i32, z: i32) -> IVec2 {
        match self {
            Facing::North => IVec2::new(x, z),
            Facing::South => IVec2::new(-x, -z),
            Facing::West => IVec2::new(z, -x),
            Facing::East => IVec2::new(-z, x),
        }
    }

    pub fn step(self) -> IVec2 {
        self.turn(0, -1)
    }
}

/// A building's own coordinates in the world.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: IVec3,
    pub facing: Facing,
}

impl Frame {
    /// The frame that puts the building's own box from `low` to `high` (x, z) exactly
    /// over the world rectangle from `min`, its floor at `y`.
    pub fn fit(low: IVec2, high: IVec2, facing: Facing, min: IVec2, y: i32) -> Self {
        let a = facing.turn(low.x, low.y);
        let b = facing.turn(high.x, high.y);
        let lowest = a.min(b);
        let origin = min - lowest;
        Frame { origin: IVec3::new(origin.x, y, origin.y), facing }
    }

    /// The size in the world of the building's own box from `low` to `high`.
    pub fn extent(low: IVec2, high: IVec2, facing: Facing) -> IVec2 {
        let a = facing.turn(low.x, low.y);
        let b = facing.turn(high.x, high.y);
        (a - b).abs() + 1
    }

    pub fn at(&self, x: i32, y: i32, z: i32) -> IVec3 {
        let p = self.facing.turn(x, z);
        IVec3::new(self.origin.x + p.x, self.origin.y + y, self.origin.z + p.y)
    }
}

/// A weighted list of blocks, one picked by position, for materials laid block by block.
#[derive(Clone, Debug, Default)]
pub struct Palette(pub Vec<(BlockId, f32)>);

impl Palette {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The block for unit value `u` in 0..1.
    pub fn pick(&self, u: f32) -> Option<BlockId> {
        let total: f32 = self.0.iter().map(|(_, w)| w).sum();
        if total <= 0.0 {
            return self.0.first().map(|(b, _)| *b);
        }
        let mut t = u * total;
        for &(block, w) in &self.0 {
            if t < w {
                return Some(block);
            }
            t -= w;
        }
        self.0.last().map(|(b, _)| *b)
    }

    /// The block for a position, from the hash of `seed` there.
    pub fn at(&self, seed: u64, pos: IVec3) -> Option<BlockId> {
        self.pick(unit(hash3(seed, pos.x, pos.y, pos.z)))
    }
}

/// A hash's top bits as a unit value.
pub fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_sort_keeps_original_order_and_last_writes() {
        let positions: Vec<_> = (-19..19).flat_map(|z| (-23..23).flat_map(move |x|
            (-2..5).map(move |y| IVec3::new(x, y, z)))).collect();
        let put = |p: IVec3| match (p.x + p.y + p.z).rem_euclid(4) {
            0 => Put::Clear,
            1 => Put::Set(BlockId(1)),
            2 => Put::Soft(BlockId(2)),
            _ => Put::Down(BlockId(3)),
        };
        let mut previous = None;
        for reverse in [false, true] {
            let mut canvas = Canvas::default();
            for i in 0..positions.len() {
                let p = positions[if reverse { positions.len() - 1 - i } else { i }];
                canvas.put(p, Put::Clear);
                canvas.put(p, put(p));
                canvas.on_ground(p.x, p.z, p.y, Put::Set(BlockId(4)));
                canvas.on_ground(p.x, p.z, p.y, put(-p));
            }
            // The original full stable sort, independent of the new sorter; overlapping
            // positions in the two maps and all put forms must survive in exact order.
            let mut all: Vec<_> = canvas.absolute.iter().map(|(&pos, &put)| Voxel { pos, ground: false, put })
                .chain(canvas.grounded.iter().map(|(&pos, &put)| Voxel { pos, ground: true, put })).collect();
            let key = |v: &Voxel| (v.ground, !matches!(v.put, Put::Clear), v.pos.y, v.pos.x, v.pos.z);
            all.sort_by_key(key);
            assert!(all.windows(2).all(|pair| key(&pair[0]) < key(&pair[1])), "every key must be distinct");
            let mut original: FxHashMap<IVec2, Vec<Voxel>> = FxHashMap::default();
            for v in all {
                let column = IVec2::new(v.pos.x, v.pos.z).div_euclid(IVec2::splat(voxmar_engine::coords::CHUNK_SIZE));
                original.entry(column).or_default().push(v);
            }
            let actual = canvas.into_columns();
            assert_eq!(actual, original);
            assert!(actual.keys().any(|p| p.x < 0) && actual.keys().any(|p| p.x >= 0));
            assert_eq!(actual.values().map(Vec::len).sum::<usize>(), positions.len() * 2);
            if let Some(before) = &previous { assert_eq!(&actual, before, "insertion order must not affect painting"); }
            previous = Some(actual);
        }
    }
}
