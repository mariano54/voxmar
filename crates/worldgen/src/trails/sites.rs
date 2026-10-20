//! The planning grid trails are routed over, and the places they lead to.
//!
//! The grid samples the land (rivers and lakes carved in) every [`CELL`] blocks, a patch
//! of [`PATCH`] cells at a time, so that what reads only part of a region (a route's
//! search, or a place judged against the land beyond its region's edge) samples only
//! that part. Each region of [`REGION`] blocks has its places of interest, from its own
//! cells, reading neighbours' cells only to judge them: a summit must top the land
//! around it, a waterfall needs two water levels side by side. A place therefore depends
//! only on the seed and the land near it, never on what was generated first.

use voxmar_engine::glam::IVec2;

use super::Land;
use crate::SEA_LEVEL;
use crate::rng::hash3;
use crate::water::Body;
use crate::water::cache::Weigh;

/// Blocks between grid samples.
pub const CELL: i32 = 8;
/// Blocks along a side of a region, the unit places and trails are planned in.
pub const REGION: i32 = 512;
/// Cells along a side of a region.
pub const CELLS: i32 = REGION / CELL;
/// Cells along a side of a patch of the grid, the unit it is sampled and kept in: a
/// region's places read the regions around only within 10 cells of its edge.
pub const PATCH: i32 = 16;

/// The land at one grid point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub height: i16,
    /// The water surface over the ground, when it is submerged; the ground otherwise.
    pub water: i16,
    pub body: u8,
    /// A settlement's building stands in it: trails go round.
    pub built: bool,
}

impl Cell {
    pub fn submerged(&self) -> bool {
        self.water > self.height
    }

    pub fn depth(&self) -> i32 {
        (self.water - self.height).max(0) as i32
    }

    pub fn sea(&self) -> bool {
        self.submerged() && self.body == Body::Sea as u8
    }

    fn lake(&self) -> bool {
        self.submerged() && self.body == Body::Lake as u8 && self.water as i32 > SEA_LEVEL
    }
}

/// One patch's cells, row by row.
pub struct Grid(pub Box<[Cell]>);

impl Weigh for Grid {
    fn bytes(&self) -> usize {
        self.0.bytes()
    }
}

impl Grid {
    /// The cells of patch `patch`, the `PATCH`×`PATCH` cells from `patch * PATCH`.
    pub fn new(land: &dyn Land, patch: IVec2) -> Self {
        let first = patch * PATCH;
        let origin = first * CELL;
        let mut cells: Box<[Cell]> = land
            .grid(origin, CELL, PATCH)
            .into_iter()
            .map(|g| {
                let submerged = g.submerged();
                Cell { height: g.height as i16, water: if submerged { g.water } else { g.height } as i16, body: g.body as u8, built: false }
            })
            .collect();
        // Cells whose ground a building takes, or comes within a block of.
        for (min, max) in land.built(origin - CELL, origin + PATCH * CELL + CELL) {
            let (lo, hi) = ((min - 1 + CELL / 2).div_euclid(IVec2::splat(CELL)), (max + 1 + CELL / 2).div_euclid(IVec2::splat(CELL)));
            for cz in lo.y.max(first.y)..=hi.y.min(first.y + PATCH - 1) {
                for cx in lo.x.max(first.x)..=hi.x.min(first.x + PATCH - 1) {
                    cells[((cz - first.y) * PATCH + cx - first.x) as usize].built = true;
                }
            }
        }
        Self(cells)
    }
}

/// What makes a place worth a trail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// The top of a hill or mountain that stands above the land around.
    Summit,
    /// A dry spot on the shore of a lake above the sea.
    Lake,
    /// The bank beside a waterfall.
    Waterfall,
    /// The edge of a drop with a long view.
    Viewpoint,
    /// Flat, low ground where trails start: a meadow or valley floor, often by a stream.
    Trailhead,
    /// Where a settlement's lane leads out.
    Settlement,
}

/// A place of interest, at a grid point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Poi {
    /// Grid coordinates (blocks / `CELL`).
    pub cell: IVec2,
    pub height: i32,
    pub kind: Kind,
}

impl Poi {
    /// Position in blocks.
    pub fn at(&self) -> IVec2 {
        self.cell * CELL
    }

    /// The order of a trail's ends: it is routed from the lower, which owns it only where
    /// trails are laid everywhere (near settlements, its gate's region does).
    pub fn below(&self, other: &Poi) -> bool {
        (self.height, self.cell.x, self.cell.y) < (other.height, other.cell.x, other.cell.y)
    }
}

/// The places of interest in `region`, at most one of each kind besides summits (two when
/// far enough apart). `cell` gives the grid at any grid point.
pub fn places(seed: u64, region: IVec2, cell: &dyn Fn(IVec2) -> Cell) -> Vec<Poi> {
    let first = region * CELLS;
    let unit = |c: IVec2, salt: u64| (hash3(seed ^ salt, c.x, 0, c.y) >> 40) as f32 / (1u64 << 24) as f32;
    let around = |c: IVec2, r: i32| (-r..=r).flat_map(move |j| (-r..=r).map(move |i| c + IVec2::new(i, j))).filter(move |&n| n != c);
    // Candidates of each kind, by score, best first; ties by position.
    let mut summits = Vec::new();
    let [mut lake, mut fall, mut view, mut head]: [Option<(f32, IVec2)>; 4] = [None; 4];
    let better = |best: &mut Option<(f32, IVec2)>, score: f32, c: IVec2| {
        if best.is_none_or(|(s, b)| score > s || (score == s && (c.x, c.y) < (b.x, b.y))) {
            *best = Some((score, c));
        }
    };
    for i in 0..CELLS * CELLS {
        let c = first + IVec2::new(i % CELLS, i / CELLS);
        let here = cell(c);
        // Nothing built in or beside it: a trail may cross what is built next to its end.
        if here.submerged() || here.built || around(c, 1).any(|n| cell(n).built) {
            continue;
        }
        let h = here.height as i32;
        let higher = |n: IVec2| {
            let other = cell(n).height as i32;
            other > h || (other == h && (n.x, n.y) < (c.x, c.y))
        };
        // A summit tops everything within 64 blocks and stands well above the ring
        // 80 blocks out.
        if h >= SEA_LEVEL + 28 && !around(c, 1).any(higher) && !around(c, 8).any(higher) {
            let ring: i32 = (0..16)
                .map(|k| {
                    let (dx, dz) = RING[k];
                    cell(c + IVec2::new(dx, dz)).height as i32
                })
                .sum();
            let prominence = h as f32 - ring as f32 / 16.0;
            if prominence >= 20.0 {
                summits.push((prominence, c));
            }
        }
        let neighbours: Vec<Cell> = around(c, 1).map(cell).collect();
        // The shore of a lake above the sea.
        let lake_sides = neighbours.iter().filter(|n| n.lake()).count();
        if lake_sides >= 2 {
            let water = around(c, 3).filter(|&n| cell(n).lake()).count();
            better(&mut lake, water as f32 + unit(c, 0x1a4e), c);
        }
        // Beside a waterfall: water at two levels at least three blocks apart.
        let levels = neighbours.iter().filter(|n| n.submerged() && !n.sea()).map(|n| n.water as i32);
        if let (Some(top), Some(bottom)) = (levels.clone().max(), levels.min())
            && top - bottom >= 3
        {
            better(&mut fall, (top - bottom) as f32 + unit(c, 0xfa11), c);
        }
        // The edge of a drop of at least 24 blocks within 24 blocks.
        if h >= SEA_LEVEL + 20 {
            let lowest = around(c, 3).map(|n| cell(n).height as i32).min().unwrap_or(h);
            if h - lowest >= 24 {
                better(&mut view, (h - lowest) as f32 + unit(c, 0x7e3), c);
            }
        }
        // Flat, low, dry ground, preferably near a stream.
        if (SEA_LEVEL + 3..=SEA_LEVEL + 70).contains(&h) && neighbours.iter().all(|n| !n.submerged() && (n.height as i32 - h).abs() <= 2) {
            let stream = around(c, 3).any(|n| {
                let w = cell(n);
                w.submerged() && !w.sea()
            });
            better(&mut head, unit(c, 0x4ead) + if stream { 0.6 } else { 0.0 }, c);
        }
    }
    summits.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.1.x, a.1.y).cmp(&(b.1.x, b.1.y))));
    let mut places: Vec<Poi> = Vec::new();
    let poi = |c: IVec2, kind| Poi { cell: c, height: cell(c).height as i32, kind };
    for &(_, c) in summits.iter() {
        if places.len() < 2 && places.iter().all(|p| (p.cell - c).length_squared() >= 30 * 30) {
            places.push(poi(c, Kind::Summit));
        }
    }
    let far = |places: &[Poi], c: IVec2| places.iter().all(|p| (p.cell - c).length_squared() >= 12 * 12);
    for (best, kind) in [(lake, Kind::Lake), (fall, Kind::Waterfall), (view, Kind::Viewpoint)] {
        if let Some((_, c)) = best
            && far(&places, c)
        {
            places.push(poi(c, kind));
        }
    }
    // Most regions have a trailhead; some are left wild.
    if let Some((_, c)) = head
        && unit(region, 0x7ea1) < 0.75
        && far(&places, c)
    {
        places.push(poi(c, Kind::Trailhead));
    }
    places
}

/// Sixteen points on a ring 10 cells out, for prominence.
const RING: [(i32, i32); 16] = [
    (10, 0),
    (9, 4),
    (7, 7),
    (4, 9),
    (0, 10),
    (-4, 9),
    (-7, 7),
    (-9, 4),
    (-10, 0),
    (-9, -4),
    (-7, -7),
    (-4, -9),
    (0, -10),
    (4, -9),
    (7, -7),
    (9, -4),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A cone 60 blocks high at (40, 40) cells, a lake east of it, flat land elsewhere.
    fn cell(c: IVec2) -> Cell {
        let d = ((c - IVec2::new(40, 40)).as_vec2().length() * CELL as f32) as i32;
        let height = (SEA_LEVEL + 10 + (60 - d / 3).max(0)) as i16;
        if (c - IVec2::new(56, 40)).abs().max_element() <= 2 {
            return Cell { height: height - 3, water: height + 1, body: Body::Lake as u8, built: false };
        }
        Cell { height, water: height, body: Body::Sea as u8, built: false }
    }

    #[test]
    fn finds_the_summit_and_the_lake_shore() {
        let places = places(7, IVec2::ZERO, &cell);
        let summit = places.iter().find(|p| p.kind == Kind::Summit).expect("a summit");
        assert_eq!(summit.cell, IVec2::new(40, 40));
        let lake = places.iter().find(|p| p.kind == Kind::Lake).expect("a lake shore");
        assert_eq!((lake.cell - IVec2::new(56, 40)).abs().max_element(), 3);
        assert!(places.iter().all(|p| !cell(p.cell).submerged()));
        // The same in any order: only the seed and the land decide.
        assert_eq!(places, super::places(7, IVec2::ZERO, &cell));
    }
}
