//! A short way round for a creature whose straight way is shut (`motion::steer` finds no
//! way it likes): A* over the cells it can stand in, near it, in eight directions, up a
//! single block if it jumps and down no further than it would step of its own accord. It
//! is how a wolf after a player finds the door in a wall, or the gap in a fence, rather
//! than run up and down the wall.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::{IVec3, Vec3};
use rustc_hash::FxHashMap;

use super::motion::{SAFE_DROP, Surroundings};

/// What a body needs of the cells it goes through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walker {
    /// Blocks of headroom it needs.
    pub height: i32,
    /// Whether it gets up a single block.
    pub climbs: bool,
    /// How high it walks up without a hop (`Movement::step`): up a slab, and course by
    /// course up stairs or a roof, climber or not.
    pub step: f32,
    /// Whether water two deep, or lava, is a wall to it (a land animal).
    pub wary: bool,
    /// Blocks across its body takes up (`span_of` its width): a bear or an aurochs takes a
    /// square two across, the Rootmother three. A way for it is a way of such squares,
    /// each named by its corner cell (least x and z).
    pub span: i32,
    /// How far down it goes in one step of its own accord: `SAFE_DROP` in a chase, else
    /// only as far as it could climb back (`Walker::climb`). Further down only onto open
    /// ground, never into a pit it could not get out of (`way_out`).
    pub drop: i32,
}

/// The most cells a search looks at, and how far from where it starts it looks: a short way
/// round, not a way across the land.
pub const BUDGET: usize = 1500;
pub const RANGE: i32 = 24;

impl Walker {
    /// How high it gets up in one step: a block if it jumps.
    pub fn climb(&self) -> i32 {
        self.climbs as i32
    }

    /// Blocks across a body `width` wide takes up.
    pub fn span_of(width: f32) -> i32 {
        (width - 1e-3).ceil().max(1.0) as i32
    }

    /// The corner cell of the square a body centred at `at` stands in.
    pub fn corner(&self, at: Vec3) -> IVec3 {
        let half = (self.span - 1) as f32 * 0.5;
        IVec3::new((at.x - half).floor() as i32, at.y.floor() as i32, (at.z - half).floor() as i32)
    }

    /// Where a body stands in the middle of the square with corner `cell`, at its feet.
    pub fn middle(&self, cell: IVec3) -> Vec3 {
        let half = self.span as f32 * 0.5;
        cell.as_vec3() + Vec3::new(half, 0.0, half)
    }

    /// Whether it can stand in the square with corner `cell`.
    pub fn stands(&self, world: &impl Surroundings, cell: IVec3) -> bool {
        Looker::new(world, *self, None).stands(cell)
    }
}

/// Squares a fill out of a drop (`way_out`) looks at before it takes the ground down there
/// for open.
pub const OPEN_FLOOR: usize = 64;

/// Squares the fills out of drops in one search look at in all: past that, a drop it has not
/// learnt of is taken for one with no way out, and a walker that keeps to its climb does not
/// go down it. Many closed pits in reach cost a search no more than this.
pub const FILL_BUDGET: usize = 4 * OPEN_FLOOR;

/// What a fill out of a drop learnt of each square it reached: the highest square it
/// reached from there and how many (capped past `OPEN_FLOOR`). A walker goes up and down
/// alike there (no further than it climbs), so every square a fill reached reaches the
/// same, and a later fill from any of them is answered at once.
type Outs = std::cell::RefCell<FxHashMap<IVec3, (i32, usize)>>;

/// Whether a walker come down to the square with corner `from` gets back up to `top` (the
/// level it came down from) or out onto open ground: a fill over the squares it can go to
/// from there, going down no further than it climbs, reaching a square at `top` or above, or
/// more than `OPEN_FLOOR` of them. Down into a pit it would never get out of, a walker does
/// not go of its own accord.
pub fn way_out(world: &impl Surroundings, walker: &Walker, from: IVec3, top: i32) -> bool {
    Looker::new(world, *walker, None).way_out(from, top)
}

/// What a search has learnt of one cell, as bits: `OPEN_KNOWN` and `OPEN` (the column from
/// it up is open as high as the walker stands), `GROUND_KNOWN` and the rest (what is in it
/// and under it), and `STANDS_KNOWN` and `STANDS` (of the square it is the corner of).
const OPEN_KNOWN: u16 = 1;
const OPEN: u16 = 2;
const GROUND_KNOWN: u16 = 4;
/// Solid under the cell.
const FLOOR: u16 = 8;
/// Water in the cell.
const WET: u16 = 16;
/// Water in the cell and under or over it (two deep for a land animal, from its top or
/// from its bed), or lava in it.
const DEEP: u16 = 32;
/// Whether it stands in the square with its corner at the cell, once known.
const STANDS_KNOWN: u16 = 64;
const STANDS: u16 = 128;
/// Whether the cell is in the way, once known.
const SOLID_KNOWN: u16 = 256;
const SOLID: u16 = 512;

/// How far up and down from where a search starts it keeps what it learns; beyond that it
/// looks again each time.
const DEPTH: i32 = 8;

/// A walker in a world, keeping what it has looked at for the length of a search in a grid
/// about where it started, so a column is looked at once however many squares hold it.
struct Looker<'a, W> {
    world: &'a W,
    walker: Walker,
    /// The grid's least corner, and what it holds of each cell.
    origin: IVec3,
    known: Vec<std::cell::Cell<u16>>,
    /// Whether it goes down further than `Walker::drop` onto open ground (`way_out`).
    deep: bool,
    /// What its fills out of drops have learnt, for the length of a search, and how many
    /// squares they have looked at (`FILL_BUDGET`).
    outs: Outs,
    filled: std::cell::Cell<usize>,
}

/// Cells across the grid a search keeps.
const ACROSS: i32 = 2 * (RANGE + 2) + 4;

impl<'a, W: Surroundings> Looker<'a, W> {
    /// A looker keeping what it learns about `about` (none kept for `None`).
    fn new(world: &'a W, walker: Walker, about: Option<IVec3>) -> Self {
        let span = walker.span.max(1);
        let origin = about.map_or(IVec3::ZERO, |at| at - IVec3::new(RANGE + 2, DEPTH, RANGE + 2));
        let size = about.map_or(0, |_| ((ACROSS + span) * (ACROSS + span) * 2 * DEPTH) as usize);
        Self { world, walker, origin, known: vec![std::cell::Cell::new(0); size], deep: true, outs: Outs::default(), filled: std::cell::Cell::new(0) }
    }

    /// Where the grid keeps `cell`, if it does.
    fn slot(&self, cell: IVec3) -> Option<&std::cell::Cell<u16>> {
        let across = ACROSS + self.walker.span.max(1);
        let d = cell - self.origin;
        if d.x < 0 || d.z < 0 || d.y < 0 || d.x >= across || d.z >= across || d.y >= 2 * DEPTH {
            return None;
        }
        self.known.get(((d.y * across + d.z) * across + d.x) as usize)
    }

    /// Whether `cell` is in the way (`Surroundings::solid`).
    fn solid(&self, cell: IVec3) -> bool {
        let slot = self.slot(cell);
        if let Some(bits) = slot.map(|s| s.get()).filter(|b| b & SOLID_KNOWN != 0) {
            return bits & SOLID != 0;
        }
        let solid = self.world.solid(cell);
        if let Some(slot) = slot {
            slot.set(slot.get() | SOLID_KNOWN | if solid { SOLID } else { 0 });
        }
        solid
    }

    /// Whether the column from `cell` up is open as high as it stands.
    fn open(&self, cell: IVec3) -> bool {
        let slot = self.slot(cell);
        if let Some(bits) = slot.map(|s| s.get()).filter(|b| b & OPEN_KNOWN != 0) {
            return bits & OPEN != 0;
        }
        // Standing on something low in the cell (a slab, a sill), its head is that much
        // higher, in the cell over its height.
        let open = (0..self.walker.height).all(|h| !self.solid(cell + IVec3::Y * h))
            && (self.rise(cell) == 0.0 || !self.solid(cell + IVec3::Y * self.walker.height));
        if let Some(slot) = slot {
            slot.set(slot.get() | OPEN_KNOWN | if open { OPEN } else { 0 });
        }
        open
    }

    /// What is in `cell` and under it (`FLOOR`, `WET`, `DEEP`).
    fn ground(&self, cell: IVec3) -> u16 {
        let slot = self.slot(cell);
        if let Some(bits) = slot.map(|s| s.get()).filter(|b| b & GROUND_KNOWN != 0) {
            return bits;
        }
        let wet = self.world.water(cell);
        let deep = wet && (self.world.water(cell - IVec3::Y) || self.world.water(cell + IVec3::Y)) || self.world.lava(cell);
        let floor = self.world.footing(cell - IVec3::Y) || self.rise(cell) > 0.0;
        let bits = if floor { FLOOR } else { 0 } | if wet { WET } else { 0 } | if deep { DEEP } else { 0 };
        if let Some(slot) = slot {
            slot.set(slot.get() | GROUND_KNOWN | bits);
        }
        bits
    }

    /// How high what is in a cell that is not in the way comes (a slab, a sill): where a
    /// body in it stands.
    fn rise(&self, cell: IVec3) -> f32 {
        self.world.boxes(cell).iter().map(|b| b.max.y).fold(0.0, f32::max)
    }

    /// Whether, going in from where it stands at height `from` in its own cell toward
    /// (dx, dz), it walks up through `cell` without a hop, course by course (stairs from
    /// the step side, a roof from the eave), no course higher than a step over the last.
    fn walks_up(&self, cell: IVec3, from: f32, dx: i32, dz: i32) -> bool {
        const STRIPS: usize = 16;
        let boxes = self.world.boxes(cell);
        let (axis, back) = if dx != 0 { (0, dx < 0) } else { (2, dz < 0) };
        let mut at = from;
        (0..STRIPS).all(|i| {
            // The strip `i` sixteenths in from the side it comes in by.
            let (near, far) = (i as f32 / STRIPS as f32, (i + 1) as f32 / STRIPS as f32);
            let (lo, hi) = if back { (1.0 - far, 1.0 - near) } else { (near, far) };
            let top = boxes.iter().filter(|b| b.min[axis] < hi && b.max[axis] > lo).map(|b| b.max.y).fold(0.0, f32::max);
            let ok = top <= 1.0 + 1e-3 && top - at <= self.walker.step + 1e-3;
            at = top;
            ok
        })
    }

    /// The cells of the square with corner `cell`.
    fn square(&self, cell: IVec3) -> impl Iterator<Item = IVec3> + use<W> {
        let span = self.walker.span;
        (0..span).flat_map(move |x| (0..span).map(move |z| cell + IVec3::new(x, 0, z)))
    }

    /// Whether its body fits upright in the square with corner `cell`.
    fn clear(&self, cell: IVec3) -> bool {
        self.square(cell).all(|c| self.open(c))
    }

    /// Whether it can stand in the square with corner `cell`: the square clear, something
    /// under one of its columns to stand on (or water to swim in, for one not wary of it),
    /// and, for a land animal, none of it in water two deep. Across a ditch narrower than
    /// it, a wide body stands on the banks.
    fn stands(&self, cell: IVec3) -> bool {
        let slot = self.slot(cell);
        if let Some(bits) = slot.map(|s| s.get()).filter(|b| b & STANDS_KNOWN != 0) {
            return bits & STANDS != 0;
        }
        let wary = self.walker.wary;
        // Lava is in no walker's way (`DEEP` for one wary of water).
        let stands = self.clear(cell)
            && self.square(cell).any(|c| self.ground(c) & if wary { FLOOR } else { FLOOR | WET } != 0)
            && !self.square(cell).any(|c| self.ground(c) & DEEP != 0 && (wary || self.world.lava(c)));
        if let Some(slot) = slot {
            slot.set(slot.get() | STANDS_KNOWN | if stands { STANDS } else { 0 });
        }
        stands
    }

    /// `way_out` from the square at `from` down to which it came from `top`, answered from
    /// what an earlier fill learnt where one reached `from`, and keeping what this one
    /// learns; looking at cells through what the search keeps of them.
    fn way_out(&self, from: IVec3, top: i32) -> bool {
        let open = |(high, count): (i32, usize)| high >= top || count > OPEN_FLOOR;
        if let Some(&known) = self.outs.borrow().get(&from) {
            return open(known);
        }
        if self.filled.get() >= FILL_BUDGET {
            return false;
        }
        let mut seen = vec![from];
        let mut queue = vec![from];
        while let Some(cell) = queue.pop() {
            if seen.len() > OPEN_FLOOR {
                break;
            }
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                if let Some(next) = self.step_as(cell, dx, dz, self.walker.climb(), false)
                    && !seen.contains(&next)
                {
                    seen.push(next);
                    queue.push(next);
                }
            }
        }
        self.filled.set(self.filled.get() + seen.len());
        let known = (seen.iter().map(|c| c.y).max().unwrap_or(from.y), seen.len());
        let mut outs = self.outs.borrow_mut();
        for cell in seen {
            outs.insert(cell, known);
        }
        open(known)
    }

    /// Where it can go in one step from the square at `cell` toward (dx, dz): level, a
    /// block up, or down a short drop; diagonals only where it could stand in the squares to
    /// either side, so it cuts no corner of a wall or of a pit.
    fn step(&self, cell: IVec3, dx: i32, dz: i32) -> Option<IVec3> {
        self.step_as(cell, dx, dz, self.walker.drop, self.deep)
    }

    /// As `step`, going down of its own accord no further than `drop`, and further onto open
    /// ground only if `deep`.
    fn step_as(&self, cell: IVec3, dx: i32, dz: i32, drop: i32, deep: bool) -> Option<IVec3> {
        if dx != 0 && dz != 0 && !(self.stands(cell + IVec3::new(dx, 0, 0)) && self.stands(cell + IVec3::new(0, 0, dz))) {
            return None;
        }
        let flat = cell + IVec3::new(dx, 0, dz);
        if self.stands(flat) {
            return Some(flat);
        }
        if self.square(flat).any(|c| self.solid(c)) {
            // Up a block: room over its head where it stands to jump, and the square up.
            let up = flat + IVec3::Y;
            let headroom = || self.square(cell).all(|c| !self.solid(c + IVec3::Y * self.walker.height));
            // Or walked up, as stairs or a roof are, or a block from a slab.
            let from = self.square(cell).map(|c| self.rise(c)).fold(0.0, f32::max);
            let walks = || self.square(flat).all(|c| !self.solid(c) || self.walks_up(c, from, dx, dz));
            return (dx * dz == 0 && (self.walker.climbs || walks()) && headroom() && self.stands(up)).then_some(up);
        }
        // Over the edge: it drops while the square is clear, as far as a short drop; further
        // than it goes of its own accord only onto open ground.
        let down = (1..=SAFE_DROP).map(|d| flat - IVec3::Y * d).take_while(|&c| self.clear(c)).find(|&c| self.stands(c))?;
        (cell.y - down.y <= drop || deep && self.way_out(down, cell.y)).then_some(down)
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

/// The squares (by corner cell, see `Walker`), after `from`, that take a walker to within
/// a block of `to`, if there is such a way near `from` (within `RANGE`, looking at no more
/// than `BUDGET` squares).
pub fn find(world: &impl Surroundings, walker: &Walker, from: IVec3, to: IVec3) -> Option<Vec<IVec3>> {
    let looker = Looker::new(world, *walker, Some(from));
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
            let Some(next) = looker.step(cell, dx, dz) else { continue };
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

    const WOLF: Walker = Walker { height: 1, climbs: true, step: crate::creature::STEP, wary: true, span: 1, drop: SAFE_DROP };

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

    /// Flat ground with, from x = 2 to 4 along z = -1..=1, a pit `deep` blocks deep, and,
    /// with `open`, the land past x = 4 down at the pit's floor as far as one looks.
    struct Hole {
        deep: i32,
        open: bool,
    }

    impl Surroundings for Hole {
        fn solid(&self, c: IVec3) -> bool {
            let in_pit = (2..=4).contains(&c.x) && (-1..=1).contains(&c.z) || self.open && c.x > 4;
            c.y < if in_pit { -self.deep } else { 0 }
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    #[test]
    fn a_wanderer_goes_down_no_further_than_it_climbs_but_onto_open_ground() {
        let wanderer = Walker { drop: WOLF.climb(), ..WOLF };
        let into = |hole: &Hole, walker: &Walker| find(hole, walker, IVec3::new(0, 0, 0), IVec3::new(3, -hole.deep, 0));
        assert_eq!(into(&Hole { deep: 2, open: false }, &wanderer), None, "a wanderer went down into a pit it cannot get out of");
        assert!(into(&Hole { deep: 1, open: false }, &wanderer).is_some(), "a wanderer kept out of a pit it climbs out of");
        assert!(into(&Hole { deep: 2, open: true }, &wanderer).is_some(), "a wanderer kept off open ground two down");
        assert!(into(&Hole { deep: 2, open: false }, &WOLF).is_some(), "a chaser kept out of the pit");
        assert!(!way_out(&Hole { deep: 3, open: false }, &wanderer, IVec3::new(3, -3, 0), 0));
    }

    /// Ground cut by trenches a block across and two deep every other block, endless or
    /// closed at both ends (`long` blocks of them, short of a fill's `OPEN_FLOOR`), its cells
    /// looked at counted.
    struct Trenches {
        looks: std::cell::Cell<u64>,
        long: Option<i32>,
    }

    impl Surroundings for Trenches {
        fn solid(&self, c: IVec3) -> bool {
            self.looks.set(self.looks.get() + 1);
            let cut = c.x.rem_euclid(2) == 1 && self.long.is_none_or(|long| (-long / 4..long - long / 4).contains(&c.z));
            c.y < if cut { -2 } else { 0 }
        }
        fn water(&self, _: IVec3) -> bool {
            self.looks.set(self.looks.get() + 1);
            false
        }
    }

    /// A search by a wanderer (going down only onto open ground) over trenches looks at no
    /// more cells than a search may at its most (`BUDGET` squares, eight ways, two looks
    /// each): what a fill out of a drop learns, every square it reached answers, its looks
    /// go through what the search keeps, and its fills look at no more than `FILL_BUDGET`
    /// squares in all. Before: each drop it looked at filled afresh, 277 thousand looks to a
    /// chaser's 2.9 thousand; and closed trenches 62 long, each a fill of its own, took 54
    /// thousand.
    #[test]
    fn a_wanderers_search_over_trenches_is_bounded() {
        for long in [None, Some(62), Some(30), Some(8)] {
            let cost = |walker: &Walker| {
                let world = Trenches { looks: std::cell::Cell::new(0), long };
                let _ = find(&world, walker, IVec3::new(0, 0, 0), IVec3::new(0, 0, 40));
                world.looks.get()
            };
            let wanderer = Walker { drop: WOLF.climb(), ..WOLF };
            let (chaser, wanders) = (cost(&WOLF), cost(&wanderer));
            assert!(wanders <= (BUDGET * 8 * 2) as u64, "over trenches {long:?} long a wanderer's search looked {wanders} times, a chaser's {chaser}");
        }
    }

    /// Flat ground and a ledge a block up from x = 4, with, at x = 3 along z = 0, `piece`:
    /// stairs or a roof rising toward +x, or a slab.
    struct Flight {
        piece: Vec<crate::physics::Aabb>,
    }

    impl Surroundings for Flight {
        fn solid(&self, c: IVec3) -> bool {
            c.y < 0 || (c.x >= 4 && c.y < 1) || (c == IVec3::new(3, 0, 0) && self.piece.iter().any(|b| b.max.y > 0.501))
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
        fn boxes(&self, c: IVec3) -> &[crate::physics::Aabb] {
            if c == IVec3::new(3, 0, 0) {
                &self.piece
            } else if self.solid(c) {
                &crate::physics::CUBE
            } else {
                &[]
            }
        }
    }

    fn piece(boxes: &[([f32; 3], [f32; 3])]) -> Vec<crate::physics::Aabb> {
        boxes.iter().map(|&(min, max)| crate::physics::Aabb { min: Vec3::from(min) / 16.0, max: Vec3::from(max) / 16.0 }).collect()
    }

    #[test]
    fn one_that_cannot_jump_walks_up_stairs_a_roof_or_a_slab_from_the_low_side_only() {
        let snail = Walker { climbs: false, ..WOLF };
        let stairs = piece(&[([0.0, 0.0, 0.0], [16.0, 8.0, 16.0]), ([8.0, 8.0, 0.0], [16.0, 16.0, 16.0])]);
        let roof = piece(&[([0.0, 0.0, 0.0], [16.0, 4.0, 16.0]), ([4.0, 4.0, 0.0], [16.0, 8.0, 16.0]), ([8.0, 8.0, 0.0], [16.0, 12.0, 16.0]), ([12.0, 12.0, 0.0], [16.0, 16.0, 16.0])]);
        let slab = piece(&[([0.0, 0.0, 0.0], [16.0, 8.0, 16.0])]);
        for (name, piece) in [("stairs", stairs.clone()), ("roof", roof), ("slab", slab)] {
            let flight = Flight { piece };
            let way = find(&flight, &snail, IVec3::new(0, 0, 0), IVec3::new(7, 1, 0));
            let way = way.unwrap_or_else(|| panic!("no way up the {name}"));
            assert!(way.iter().all(|c| c.z.abs() <= 1), "the way up the {name} goes round: {way:?}");
        }
        // Stairs turned away, their back to it, are a wall a block high.
        let back = piece(&[([0.0, 0.0, 0.0], [16.0, 8.0, 16.0]), ([0.0, 8.0, 0.0], [8.0, 16.0, 16.0])]);
        let flight = Flight { piece: back };
        let way = find(&flight, &snail, IVec3::new(0, 0, 0), IVec3::new(7, 1, 0));
        assert!(way.is_none_or(|w| !w.contains(&IVec3::new(3, 1, 0))), "it went up the back of the stairs");
        // On top of the stairs, a block up, it has room to stand.
        let low = Flight { piece: stairs };
        assert!(snail.stands(&low, IVec3::new(3, 1, 0)), "nowhere to stand on top of the stairs");
    }

    #[test]
    fn a_wide_body_takes_no_way_through_a_doorway_one_block_across_but_does_through_two() {
        let bear = Walker { span: 2, ..WOLF };
        assert_eq!(find(&Yard { door: Some(5) }, &bear, IVec3::new(-3, 0, 0), IVec3::new(3, 0, 0)), None);
        assert!(find(&Yard { door: Some(5) }, &WOLF, IVec3::new(-3, 0, 0), IVec3::new(3, 0, 0)).is_some());
        let way = find(&Wide, &bear, IVec3::new(-3, 0, 0), IVec3::new(3, 0, 0)).expect("a way through the wide door");
        assert!(way.iter().any(|c| c.x == 0), "it never went through the door: {way:?}");
    }

    /// A wall two thick, its doorways a block apart: two across through the near side
    /// (z = 2, 3) and through the far side (z = 3, 4). Each cell of either is in some square
    /// two across that is open, but no square two across goes through both.
    struct Staggered;

    impl Surroundings for Staggered {
        fn solid(&self, c: IVec3) -> bool {
            c.y < 0 || (c.y < 3 && ((c.x == 0 && !(2..=3).contains(&c.z)) || (c.x == 1 && !(3..=4).contains(&c.z))))
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    #[test]
    fn a_wide_body_takes_no_way_through_a_staggered_wall_two_thick() {
        let bear = Walker { height: 2, span: 2, ..WOLF };
        assert_eq!(find(&Staggered, &bear, IVec3::new(-4, 0, 3), IVec3::new(5, 0, 3)), None);
        assert!(find(&Staggered, &WOLF, IVec3::new(-4, 0, 3), IVec3::new(5, 0, 3)).is_some());
    }

    /// Flat ground with a ditch a block across and a block deep along x = 0, dry or with
    /// water in it.
    struct Ditch {
        wet: bool,
    }

    impl Surroundings for Ditch {
        fn solid(&self, c: IVec3) -> bool {
            c.y < if c.x == 0 { -1 } else { 0 }
        }
        fn water(&self, c: IVec3) -> bool {
            self.wet && c.x == 0 && c.y == -1
        }
    }

    #[test]
    fn a_wide_body_steps_over_a_ditch_a_block_across() {
        let bear = Walker { height: 2, span: 2, ..WOLF };
        for wet in [false, true] {
            for walker in [WOLF, bear] {
                let way = find(&Ditch { wet }, &walker, IVec3::new(-4, 0, 0), IVec3::new(4, 0, 0));
                assert!(way.is_some(), "no way over the ditch (wet: {wet}) for {walker:?}");
            }
            let way = find(&Ditch { wet }, &bear, IVec3::new(-4, 0, 0), IVec3::new(4, 0, 0)).unwrap();
            assert!(way.iter().all(|c| c.y == 0), "it went down into the ditch (wet: {wet}): {way:?}");
        }
    }

    /// Flat ground, a step a block up from x = 3, and (`beams`) a beam two blocks up at
    /// x = 1 over every other z.
    struct Beams {
        beams: bool,
    }

    impl Surroundings for Beams {
        fn solid(&self, c: IVec3) -> bool {
            c.y < 0 || (c.x >= 3 && c.y < 1) || (self.beams && c.x == 1 && c.y == 2 && c.z.rem_euclid(2) == 1)
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    /// A body two across and two tall jumps up the step only with headroom over all four
    /// columns it stands in: under a beam over one of them, it would hit its head.
    #[test]
    fn a_wide_body_jumps_only_with_headroom_over_all_its_square() {
        let bear = Walker { height: 2, span: 2, ..WOLF };
        let (from, to) = (IVec3::new(-3, 0, 0), IVec3::new(5, 1, 0));
        assert!(find(&Beams { beams: false }, &bear, from, to).is_some());
        assert_eq!(find(&Beams { beams: true }, &bear, from, to), None);
    }

    /// A wall along x = 0 with a doorway two blocks across at z = 4 and 5.
    struct Wide;

    impl Surroundings for Wide {
        fn solid(&self, c: IVec3) -> bool {
            c.y < 0 || (c.x == 0 && c.y < 3 && !(4..=5).contains(&c.z))
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    /// Flat ground with a pond two deep sunk in it (its bed at y = -2, x and z in 0..4),
    /// shelving to a block deep along its far side (x = 3), and a pit four deep beside it
    /// (x in 0..4, z in -4..0). Along the bed and up the shelf is the short way across.
    struct Pond;

    impl Surroundings for Pond {
        fn solid(&self, c: IVec3) -> bool {
            let sunk = (0..4).contains(&c.x) && (-4..4).contains(&c.z);
            if sunk && c.z < 0 {
                c.y < -4
            } else if sunk && c.x == 3 {
                c.y < -1
            } else if sunk {
                c.y < -2
            } else {
                c.y < 0
            }
        }
        fn water(&self, c: IVec3) -> bool {
            (0..4).contains(&c.x) && (0..4).contains(&c.z) && (-2..0).contains(&c.y) && !self.solid(c)
        }
    }

    #[test]
    fn a_way_round_a_pond_keeps_off_its_bed_and_cuts_no_corner_of_a_pit() {
        let way = find(&Pond, &WOLF, IVec3::new(-2, 0, 2), IVec3::new(6, 0, 2)).expect("a way round the pond");
        let mut at = IVec3::new(-2, 0, 2);
        for &cell in &way {
            assert!(!Pond.water(cell + IVec3::Y), "it went along the pond's bed: {way:?}");
            assert!(!Pond.water(cell), "it went through the pond: {way:?}");
            let d = cell - at;
            if d.x != 0 && d.z != 0 {
                for side in [at + IVec3::new(d.x, 0, 0), at + IVec3::new(0, 0, d.z)] {
                    assert!(WOLF.stands(&Pond, side), "it cut a corner by {side} going from {at} to {cell}: {way:?}");
                }
            }
            at = cell;
        }
    }
}

#[cfg(test)]
mod cost {
    use std::cell::Cell;

    use super::*;

    /// Counts the block lookups a search makes.
    struct Counted<'a, W>(&'a W, Cell<u64>);

    impl<W: Surroundings> Surroundings for Counted<'_, W> {
        fn solid(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.solid(c)
        }
        fn water(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.water(c)
        }
        fn footing(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.footing(c)
        }
        fn lava(&self, c: IVec3) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.lava(c)
        }
        fn boxes(&self, c: IVec3) -> &[crate::physics::Aabb] {
            self.1.set(self.1.get() + 1);
            self.0.boxes(c)
        }
    }

    /// A wall along x = 0, a doorway `door` blocks across at z = 4 and on.
    struct Door(i32);

    impl Surroundings for Door {
        fn solid(&self, c: IVec3) -> bool {
            c.y < 0 || (c.x == 0 && c.y < 3 && !(4..4 + self.0).contains(&c.z))
        }
        fn water(&self, _: IVec3) -> bool {
            false
        }
    }

    /// Block lookups and time per search, for a body a block across and one two across,
    /// at a wall with no doorway (a failed search), one a block across (failed for the wide
    /// one) and one two across.
    /// `cargo test -p voxmar_engine --release search_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn search_cost() {
        let wolf = Walker { height: 1, climbs: true, step: crate::creature::STEP, wary: true, span: 1, drop: SAFE_DROP };
        let bear = Walker { height: 2, span: 2, ..wolf };
        for (name, walker) in [("wolf", wolf), ("bear", bear)] {
            for door in [0, 1, 2] {
                let world = Counted(&Door(door), Cell::new(0));
                let found = find(&world, &walker, IVec3::new(-3, 0, 0), IVec3::new(3, 0, 0)).is_some();
                let lookups = world.1.get();
                let runs = 50;
                let at = std::time::Instant::now();
                for _ in 0..runs {
                    std::hint::black_box(find(&Door(door), &walker, IVec3::new(-3, 0, 0), IVec3::new(3, 0, 0)));
                }
                let ms = at.elapsed().as_secs_f64() * 1000.0 / runs as f64;
                println!("{name} door {door}: found {found}, {lookups} lookups, {ms:.3} ms");
            }
        }
    }
}
