//! A short way round for a creature whose straight way is shut (`motion::steer` finds no
//! way it likes): A* over the cells it can stand in, near it, in eight directions, up a
//! single block if it jumps and down no further than it would step of its own accord. It
//! is how a wolf after a player finds the door in a wall, or the gap in a fence, rather
//! than run up and down the wall.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::IVec3;
use rustc_hash::FxHashMap;

use super::motion::{SAFE_DROP, Surroundings};

/// What a body needs of the cells it goes through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walker {
    /// Blocks of headroom it needs.
    pub height: i32,
    /// Whether it gets up a single block.
    pub climbs: bool,
    /// Whether water two deep is a wall to it (a land animal).
    pub wary: bool,
}

/// The most cells a search looks at, and how far from where it starts it looks: a short way
/// round, not a way across the land.
pub const BUDGET: usize = 1500;
pub const RANGE: i32 = 24;

impl Walker {
    /// Whether it can stand with its feet in `cell`.
    pub fn stands(&self, world: &impl Surroundings, cell: IVec3) -> bool {
        if !world.solid(cell - IVec3::Y) && !(world.water(cell) && !self.wary) {
            return false;
        }
        if self.wary && world.water(cell) && world.water(cell - IVec3::Y) {
            return false;
        }
        self.clear(world, cell)
    }

    /// Whether its body fits upright with its feet in `cell`.
    fn clear(&self, world: &impl Surroundings, cell: IVec3) -> bool {
        (0..self.height).all(|h| !world.solid(cell + IVec3::Y * h))
    }

    /// Where it can go in one step from `cell` toward (dx, dz): level, a block up, or down
    /// a short drop; diagonals only where both sides are open, so it cuts no corner.
    fn step(&self, world: &impl Surroundings, cell: IVec3, dx: i32, dz: i32) -> Option<IVec3> {
        if dx != 0 && dz != 0 && !(self.clear(world, cell + IVec3::new(dx, 0, 0)) && self.clear(world, cell + IVec3::new(0, 0, dz))) {
            return None;
        }
        let flat = cell + IVec3::new(dx, 0, dz);
        if self.stands(world, flat) {
            return Some(flat);
        }
        if world.solid(flat) {
            let up = flat + IVec3::Y;
            return (self.climbs && dx * dz == 0 && !world.solid(cell + IVec3::Y * self.height) && self.stands(world, up)).then_some(up);
        }
        // Over the edge: it drops while the column is open, as far as a short drop.
        (1..=SAFE_DROP).map(|d| flat - IVec3::Y * d).take_while(|&c| self.clear(world, c)).find(|&c| self.stands(world, c))
    }
}

#[derive(PartialEq)]
struct Open {
    guess: f32,
    cell: IVec3,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other.guess.total_cmp(&self.guess)
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The cells, after `from`, that take a walker to within a block of `to`, if there is such
/// a way near `from` (within `RANGE`, looking at no more than `BUDGET` cells).
pub fn find(world: &impl Surroundings, walker: &Walker, from: IVec3, to: IVec3) -> Option<Vec<IVec3>> {
    let near = |c: IVec3| (c.x - to.x).abs() <= 1 && (c.z - to.z).abs() <= 1 && (c.y - to.y).abs() <= 1;
    let guess = |c: IVec3| {
        let (dx, dz) = ((c.x - to.x).abs() as f32, (c.z - to.z).abs() as f32);
        dx.max(dz) + 0.414 * dx.min(dz) + (c.y - to.y).abs() as f32 * 0.5
    };
    let mut came: FxHashMap<IVec3, (IVec3, f32)> = FxHashMap::default();
    came.insert(from, (from, 0.0));
    let mut open = BinaryHeap::from([Open { guess: guess(from), cell: from }]);
    let mut looked = 0;
    while let Some(Open { cell, .. }) = open.pop() {
        if near(cell) {
            let (mut way, mut at) = (Vec::new(), cell);
            while at != from {
                way.push(at);
                at = came[&at].0;
            }
            way.reverse();
            return Some(way);
        }
        looked += 1;
        if looked > BUDGET {
            return None;
        }
        let cost = came[&cell].1;
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let Some(next) = walker.step(world, cell, dx, dz) else { continue };
            if (next.x - from.x).abs() > RANGE || (next.z - from.z).abs() > RANGE {
                continue;
            }
            let cost = cost + if dx * dz != 0 { 1.414 } else { 1.0 } + (next.y - cell.y).abs() as f32 * 0.5;
            if came.get(&next).is_none_or(|&(_, known)| cost < known) {
                came.insert(next, (cell, cost));
                open.push(Open { guess: cost + guess(next), cell: next });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat ground, a wall along x = 0 with a door at z = `door` (None: no door), and a
    /// one-block step at x = 4.
    struct Yard {
        door: Option<i32>,
    }

    impl Surroundings for Yard {
        fn solid(&self, c: IVec3) -> bool {
            c.y < 0 || (c.x == 0 && c.y < 3 && Some(c.z) != self.door) || (c.x >= 4 && c.y < 1)
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    const WOLF: Walker = Walker { height: 1, climbs: true, wary: true };

    #[test]
    fn a_way_goes_through_the_door_and_up_the_step() {
        let yard = Yard { door: Some(5) };
        let way = find(&yard, &WOLF, IVec3::new(-3, 0, 0), IVec3::new(5, 1, 0)).expect("a way through the door");
        assert!(way.iter().any(|c| c.x == 0 && c.z == 5), "it never went through the door: {way:?}");
        assert!(way.last().is_some_and(|c| c.y == 1), "it never got up the step: {way:?}");
        for pair in way.windows(2) {
            let d = pair[1] - pair[0];
            assert!(d.x.abs() <= 1 && d.z.abs() <= 1 && d.y <= 1, "a jump in the way: {pair:?}");
            assert!(WOLF.stands(&yard, pair[1]));
        }
    }

    #[test]
    fn no_way_through_a_wall_without_a_door_or_up_two_blocks() {
        assert_eq!(find(&Yard { door: None }, &WOLF, IVec3::new(-3, 0, 0), IVec3::new(3, 0, 0)), None);
        // One that cannot jump keeps off the step.
        let snail = Walker { climbs: false, ..WOLF };
        assert_eq!(find(&Yard { door: Some(0) }, &snail, IVec3::new(2, 0, 0), IVec3::new(6, 1, 0)), None);
    }
}
