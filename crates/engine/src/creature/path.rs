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
    /// Whether water two deep is a wall to it (a land animal).
    pub wary: bool,
    /// Blocks across its body takes up (`span_of` its width): a bear or an aurochs takes a
    /// square two across, the Rootmother three. A way for it is a way of such squares,
    /// each named by its corner cell (least x and z).
    pub span: i32,
}

/// The most cells a search looks at, and how far from where it starts it looks: a short way
/// round, not a way across the land.
pub const BUDGET: usize = 1500;
pub const RANGE: i32 = 24;

impl Walker {
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

/// What a search has learnt of one cell, as bits: `OPEN_KNOWN` and `OPEN` (the column from
/// it up is open as high as the walker stands), `GROUND_KNOWN` and the rest (what is in it
/// and under it), and `STANDS_KNOWN` and `STANDS` (of the square it is the corner of).
const OPEN_KNOWN: u8 = 1;
const OPEN: u8 = 2;
const GROUND_KNOWN: u8 = 4;
/// Solid under the cell.
const FLOOR: u8 = 8;
/// Water in the cell.
const WET: u8 = 16;
/// Water in the cell and under or over it (two deep for a land animal, from its top or
/// from its bed).
const DEEP: u8 = 32;
/// Whether it stands in the square with its corner at the cell, once known.
const STANDS_KNOWN: u8 = 64;
const STANDS: u8 = 128;

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
    known: Vec<std::cell::Cell<u8>>,
}

/// Cells across the grid a search keeps.
const ACROSS: i32 = 2 * (RANGE + 2) + 4;

impl<'a, W: Surroundings> Looker<'a, W> {
    /// A looker keeping what it learns about `about` (none kept for `None`).
    fn new(world: &'a W, walker: Walker, about: Option<IVec3>) -> Self {
        let span = walker.span.max(1);
        let origin = about.map_or(IVec3::ZERO, |at| at - IVec3::new(RANGE + 2, DEPTH, RANGE + 2));
        let size = about.map_or(0, |_| ((ACROSS + span) * (ACROSS + span) * 2 * DEPTH) as usize);
        Self { world, walker, origin, known: vec![std::cell::Cell::new(0); size] }
    }

    /// Where the grid keeps `cell`, if it does.
    fn slot(&self, cell: IVec3) -> Option<&std::cell::Cell<u8>> {
        let across = ACROSS + self.walker.span.max(1);
        let d = cell - self.origin;
        if d.x < 0 || d.z < 0 || d.y < 0 || d.x >= across || d.z >= across || d.y >= 2 * DEPTH {
            return None;
        }
        self.known.get(((d.y * across + d.z) * across + d.x) as usize)
    }

    /// Whether the column from `cell` up is open as high as it stands.
    fn open(&self, cell: IVec3) -> bool {
        let slot = self.slot(cell);
        if let Some(bits) = slot.map(|s| s.get()).filter(|b| b & OPEN_KNOWN != 0) {
            return bits & OPEN != 0;
        }
        let open = (0..self.walker.height).all(|h| !self.world.solid(cell + IVec3::Y * h));
        if let Some(slot) = slot {
            slot.set(slot.get() | OPEN_KNOWN | if open { OPEN } else { 0 });
        }
        open
    }

    /// What is in `cell` and under it (`FLOOR`, `WET`, `DEEP`).
    fn ground(&self, cell: IVec3) -> u8 {
        let slot = self.slot(cell);
        if let Some(bits) = slot.map(|s| s.get()).filter(|b| b & GROUND_KNOWN != 0) {
            return bits;
        }
        let wet = self.world.water(cell);
        let deep = wet && (self.world.water(cell - IVec3::Y) || self.world.water(cell + IVec3::Y));
        let bits = if self.world.footing(cell - IVec3::Y) { FLOOR } else { 0 } | if wet { WET } else { 0 } | if deep { DEEP } else { 0 };
        if let Some(slot) = slot {
            slot.set(slot.get() | GROUND_KNOWN | bits);
        }
        bits
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
        let stands = self.clear(cell)
            && self.square(cell).any(|c| self.ground(c) & if wary { FLOOR } else { FLOOR | WET } != 0)
            && !(wary && self.square(cell).any(|c| self.ground(c) & DEEP != 0));
        if let Some(slot) = slot {
            slot.set(slot.get() | STANDS_KNOWN | if stands { STANDS } else { 0 });
        }
        stands
    }

    /// Where it can go in one step from the square at `cell` toward (dx, dz): level, a
    /// block up, or down a short drop; diagonals only where it could stand in the squares to
    /// either side, so it cuts no corner of a wall or of a pit.
    fn step(&self, cell: IVec3, dx: i32, dz: i32) -> Option<IVec3> {
        if dx != 0 && dz != 0 && !(self.stands(cell + IVec3::new(dx, 0, 0)) && self.stands(cell + IVec3::new(0, 0, dz))) {
            return None;
        }
        let flat = cell + IVec3::new(dx, 0, dz);
        if self.stands(flat) {
            return Some(flat);
        }
        if self.square(flat).any(|c| self.world.solid(c)) {
            // Up a block: room over its head where it stands to jump, and the square up.
            let up = flat + IVec3::Y;
            let headroom = || self.square(cell).all(|c| !self.world.solid(c + IVec3::Y * self.walker.height));
            return (self.walker.climbs && dx * dz == 0 && headroom() && self.stands(up)).then_some(up);
        }
        // Over the edge: it drops while the square is clear, as far as a short drop.
        (1..=SAFE_DROP).map(|d| flat - IVec3::Y * d).take_while(|&c| self.clear(c)).find(|&c| self.stands(c))
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

    const WOLF: Walker = Walker { height: 1, climbs: true, wary: true, span: 1 };

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
        let wolf = Walker { height: 1, climbs: true, wary: true, span: 1 };
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
