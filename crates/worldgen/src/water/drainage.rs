//! Water routing on a grid of square cells, shared by both tiers of rivers.
//!
//! Every cell drains to the neighbour down its steepest slope, over eight directions,
//! with ties broken by position so that "lower" is a total order and water can never
//! run in a circle. Of two diagonals crossing in one square, only the one from the higher
//! cell is kept, so rivers never cross.
//!
//! A cell lower than all its neighbours is a pit. A priority flood rises from it until
//! the water would spill towards somewhere lower; what happens then depends on the pit.
//! Shallow or tiny pits are breached: the river cuts through the rim and never rises
//! above the pit. Others become lakes at their spill height, or, in dry climates, lower
//! than that, with no outlet at all (down to a dry salt flat). In the trunk tier, a
//! deep closed basin may instead drain through a tunnel under its rim. A lake is keyed by
//! its pit and flooded in a window around the pit alone, so every region that meets it
//! sees exactly the same lake.
//!
//! Flow is accumulated region by region: each cell sheds its runoff, which adds up
//! downstream. Water leaving a region enters a neighbour, so each region records where
//! water crosses its border: the cells where it enters, and for each cell where it
//! leaves, the water gathered inside plus which entries feed it. The total through an
//! exit is then its local part plus whatever the upstream regions send in, worked out
//! lazily crossing by crossing, following the water upstream; the work grows only with the
//! area actually upstream, every cell belongs to exactly one region, and a river may be
//! any length.
//!
//! Each region routes its own cells, and its view decides where they send their water: an
//! entry counts a cell outside as a feeder only where that cell's own region sends it there,
//! so every crossing is decided once. Water runs downhill, but lakes rise to their spill
//! points, and two regions that resolve a lake across their border a little differently
//! could pass water round in a circle. So the crossings are a function from each exit to the
//! next one downstream, and in the rare circle the exit with the lowest cell keeps its water
//! (`cut`): a choice that depends on nothing but the circle, never on which region or thread
//! asked first. Without it, two threads could each wait on the other's half of the circle.

use std::cell::RefCell;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::glam::IVec2;

use super::cache::{Cache, Shared, Weigh};
use crate::SEA_LEVEL;
use crate::rng::hash3;

/// The eight neighbours, counter-clockwise from +x; odd indices are diagonals.
pub const DIRS: [IVec2; 8] = [
    IVec2::new(1, 0),
    IVec2::new(1, 1),
    IVec2::new(0, 1),
    IVec2::new(-1, 1),
    IVec2::new(-1, 0),
    IVec2::new(-1, -1),
    IVec2::new(0, -1),
    IVec2::new(1, -1),
];
const LENGTH: [f32; 8] = [1.0, std::f32::consts::SQRT_2, 1.0, std::f32::consts::SQRT_2, 1.0, std::f32::consts::SQRT_2, 1.0, std::f32::consts::SQRT_2];

/// Routing codes besides the directions 0..8.
pub const SEA: u8 = 8;
/// Into the water of the bigger tier (a trunk river or its lake).
pub const ABSORBED: u8 = 9;
const SINK: u8 = 10;
/// Water ends here: a closed basin.
pub const END: u8 = 11;

/// `Tile::absorbed` where no bigger water covers a cell.
pub const DRY: i32 = i32::MIN;
/// The sea's surface, in eighths of a block: the top of the top water block.
pub const SEA_SURFACE: i32 = (SEA_LEVEL + 1) * 8;

/// The grid and the rules of one tier.
pub struct Tier {
    /// Cell size, in blocks.
    pub cell: i32,
    /// Cells along a side of a region.
    pub side: i32,
    /// Cells along a side of a tile of ground samples; divides `side`.
    pub tile: i32,
    /// Furthest a lake reaches from its pit, in cells.
    pub lake_radius: i32,
    /// Most cells a lake may have.
    pub lake_cells: usize,
    /// Furthest a breach may cut beyond a pit's rim, in cells.
    pub breach: i32,
    /// Pits shallower than this many blocks, or with fewer cells below their spill
    /// height than `min_area`, are breached rather than filled.
    pub min_depth: f32,
    pub min_area: usize,
    /// How far a river's surface lies below the ground's (the top of its top block), in
    /// blocks.
    pub incision: f32,
    /// Whether wet basins may fill as lakes. Without, a river breaches or tunnels out of
    /// any basin it can, and a basin it cannot leave stays dry.
    pub lakes: bool,
    /// The chance that a small pit, which would be breached, fills as a hot spring.
    pub springs: f32,
    /// When a small pit high in the hills holds a tarn instead of being breached.
    pub tarns: Option<TarnRule>,
    pub tunnel: Option<TunnelRule>,
    pub seed: u64,
}

/// When a small pit, which would be breached, holds a tarn: a still lake one block under its
/// rim, in a hollow high in the hills (`content::TarnsDef`).
pub struct TarnRule {
    /// The lowest the pit's bottom may lie, in blocks: tarns are a highland thing, and
    /// lowland water never changes for them.
    pub floor: f32,
    /// Depth of the pit below its spill point, in blocks, from which it may hold one.
    pub depth: f32,
    /// The span of the natural water footprint, in blocks.
    pub size: (i32, i32),
    /// The chance that such a pit holds one where its ground lets tarns lie fully
    /// (`Ground::tarns`).
    pub chance: f32,
}

/// When a deep basin drains through a tunnel under its rim instead of filling.
pub struct TunnelRule {
    /// Depth of the basin below its spill point, in blocks, from which a tunnel is possible.
    pub depth: f32,
    /// The chance that such a basin tunnels.
    pub chance: f32,
    /// Longest tunnel, in cells.
    pub length: i32,
}

impl Tier {
    /// Cells around a region that its basin examines: enough to find every pit whose
    /// lake, breach or tunnel reaches within a few cells of the region.
    fn halo(&self) -> i32 {
        self.reach() + CORE + 3
    }

    /// Furthest anything a pit decides reaches from it, in cells.
    fn reach(&self) -> i32 {
        self.lake_radius.max(self.tunnel.as_ref().map_or(0, |t| t.length)) + self.breach + 1
    }

    /// Centre of a cell, in blocks.
    pub fn centre(&self, cell: IVec2) -> IVec2 {
        cell * self.cell + self.cell / 2
    }

    /// The river's surface, in eighths, over ground whose top block is at `height`.
    pub fn target(&self, height: f32) -> i32 {
        (((height + 1.0 - self.incision) * 8.0).floor() as i32).max(SEA_SURFACE)
    }
}

/// Cells around a region whose routing and levels a basin keeps, for drawing the region's
/// rivers and lakes.
pub const CORE: i32 = 4;

/// Ground samples over a tile of cells.
pub struct Tile {
    /// Height of the ground at each cell's centre (rows of `Tier::tile` cells).
    pub height: Box<[f32]>,
    /// Water each cell sheds into the rivers.
    pub runoff: Box<[f32]>,
    /// Surface, in eighths, of bigger water covering each cell, or `DRY`.
    pub absorbed: Box<[i32]>,
}

impl Weigh for Tile {
    fn bytes(&self) -> usize {
        self.height.bytes() + self.runoff.bytes() + self.absorbed.bytes()
    }
}

/// Where a tier reads the ground.
pub trait Ground: Sync {
    /// The samples of the tile at `key` (in tile units).
    fn tile(&self, key: IVec2) -> Arc<Tile>;
    /// Whether the tile at `key` is made or being made.
    fn has(&self, key: IVec2) -> bool;
    /// How humid the climate is at a cell, 0 (arid) ..= 1 (wet enough that lakes stay
    /// full).
    fn humidity(&self, cell: IVec2) -> f32;
    /// How fully tarns may lie at a cell, 0 ..= 1: their climate and country, and a
    /// climate humid enough to keep them full.
    fn tarns(&self, _cell: IVec2) -> f32 {
        0.0
    }
    /// The cells that fill one complete natural pool of the requested size. The
    /// stream ground reads the land, trunks and published routing, never final geometry.
    fn tarn_cells(&self, _pit: IVec2, _level: i32, _size: (i32, i32)) -> Option<Vec<IVec2>> {
        None
    }
}

/// Reads cells of any tile, keeping the tiles it has used.
pub(super) struct Reader<'a> {
    size: i32,
    ground: &'a dyn Ground,
    tiles: FxHashMap<IVec2, Arc<Tile>>,
}

impl<'a> Reader<'a> {
    pub(super) fn new(tier: &Tier, ground: &'a dyn Ground) -> Self {
        Self { size: tier.tile, ground, tiles: FxHashMap::default() }
    }

    fn tile(&mut self, key: IVec2) -> &Arc<Tile> {
        let ground = self.ground;
        self.tiles.entry(key).or_insert_with(|| ground.tile(key))
    }

    /// Height and covering water at a cell.
    pub(super) fn at(&mut self, cell: IVec2) -> (f32, i32) {
        let size = self.size;
        let local = cell.rem_euclid(IVec2::splat(size));
        let i = (local.y * size + local.x) as usize;
        let tile = self.tile(cell.div_euclid(IVec2::splat(size)));
        (tile.height[i], tile.absorbed[i])
    }
}

/// A cell's height with its position, as a total order.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    height: f32,
    cell: IVec2,
}

impl Eq for Key {}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.height.total_cmp(&other.height).then(self.cell.x.cmp(&other.cell.x)).then(self.cell.y.cmp(&other.cell.y))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn direction(step: IVec2) -> u8 {
    DIRS.iter().position(|&d| d == step).expect("neighbouring cells") as u8
}

/// The direction of steepest descent from a cell, over neighbours lower in the total
/// order, leaving out `skip`.
fn steepest(at: &mut impl FnMut(IVec2) -> (f32, i32), cell: IVec2, skip: Option<u8>) -> Option<u8> {
    let here = Key { height: at(cell).0, cell };
    let mut best: Option<(f32, u8)> = None;
    for (k, d) in DIRS.iter().enumerate() {
        if skip == Some(k as u8) {
            continue;
        }
        let next = cell + *d;
        let height = at(next).0;
        if (Key { height, cell: next }) < here {
            let slope = (here.height - height) / LENGTH[k];
            if best.is_none_or(|(steepest, _)| slope > steepest) {
                best = Some((slope, k as u8));
            }
        }
    }
    best.map(|(_, k)| k)
}

/// `steepest` for every cell of a square window of heights (rows of `width`) but its
/// outermost ring, rows of `width - 2`: the same rule, read straight from the window.
fn steepest_inside(heights: &[f32], width: i32) -> Vec<Option<u8>> {
    let inner = width - 2;
    let offsets = DIRS.map(|d| (d.y * width + d.x) as isize);
    // Of two cells of equal height, the lower in the total order is the one first by x,
    // then by z: the neighbour, where it lies towards −x, or straight towards −z.
    let first_on_ties = DIRS.map(|d| (d.x, d.y) < (0, 0));
    // Each height as an integer in `total_cmp`'s order, worked out once rather than for
    // every neighbour that reads it.
    let order: Vec<i32> = heights.iter().map(|h| total_order(*h)).collect();
    let mut out = Vec::with_capacity((inner * inner) as usize);
    for row in 1..=inner {
        for at in (row * width + 1) as isize..(row * width + 1 + inner) as isize {
            let (here, rank) = (heights[at as usize], order[at as usize]);
            let mut best: Option<(f32, u8)> = None;
            for k in 0..8 {
                let n = (at + offsets[k]) as usize;
                if order[n] < rank || (order[n] == rank && first_on_ties[k]) {
                    let slope = (here - heights[n]) / LENGTH[k];
                    if best.is_none_or(|(steepest, _)| slope > steepest) {
                        best = Some((slope, k as u8));
                    }
                }
            }
            out.push(best.map(|(_, k)| k));
        }
    }
    out
}

/// `f32::total_cmp`'s order as an integer's.
fn total_order(h: f32) -> i32 {
    let bits = h.to_bits() as i32;
    bits ^ (((bits >> 31) as u32) >> 1) as i32
}

/// Where water at a cell goes before pits are resolved: a direction, `SEA`, `ABSORBED`
/// or `SINK`.
fn route(at: &mut impl FnMut(IVec2) -> (f32, i32), cell: IVec2) -> u8 {
    let mut best = |c: IVec2, at: &mut dyn FnMut(IVec2) -> (f32, i32)| {
        let mut at = |c: IVec2| at(c);
        steepest(&mut at, c, None)
    };
    route_by(at, &mut best, cell)
}

/// `route`, with each cell's steepest descent given by `best`.
fn route_with(at: &mut impl FnMut(IVec2) -> (f32, i32), best: &impl Fn(IVec2) -> Option<u8>, cell: IVec2) -> u8 {
    route_by(at, &mut |c: IVec2, _: &mut dyn FnMut(IVec2) -> (f32, i32)| best(c), cell)
}

fn route_by(
    at: &mut impl FnMut(IVec2) -> (f32, i32),
    best: &mut impl FnMut(IVec2, &mut dyn FnMut(IVec2) -> (f32, i32)) -> Option<u8>,
    cell: IVec2,
) -> u8 {
    let (height, absorbed) = at(cell);
    if height < SEA_LEVEL as f32 {
        return SEA;
    }
    if absorbed != DRY {
        return ABSORBED;
    }
    let Some(k) = best(cell, at) else { return SINK };
    if k % 2 == 1 {
        // The other diagonal of the same square runs between the two side cells. Of two
        // crossing diagonals, the one from the higher cell stays.
        let d = DIRS[k as usize];
        let (a, b) = (cell + IVec2::new(d.x, 0), cell + IVec2::new(0, d.y));
        let mut runs_to = |from: IVec2, to: IVec2| {
            let (height, absorbed) = at(from);
            height >= SEA_LEVEL as f32 && absorbed == DRY && best(from, at) == Some(direction(to - from))
        };
        let rival = if runs_to(a, b) {
            Some(a)
        } else if runs_to(b, a) {
            Some(b)
        } else {
            None
        };
        if let Some(rival) = rival
            && (Key { height, cell }) < (Key { height: at(rival).0, cell: rival })
        {
            return steepest(at, cell, Some(k)).unwrap_or(SINK);
        }
    }
    k
}

/// Follows the water down from `start` until its river would lie at or below `level`:
/// the cells on the way, or nothing if that takes more than `limit` cells.
fn descend_below(at: &mut impl FnMut(IVec2) -> (f32, i32), tier: &Tier, start: IVec2, level: i32, limit: i32) -> Option<Vec<IVec2>> {
    let mut cells = Vec::new();
    let mut cell = start;
    for _ in 0..=limit {
        if tier.target(at(cell).0) <= level {
            return Some(cells);
        }
        cells.push(cell);
        match route(at, cell) {
            k if k < 8 => cell += DIRS[k as usize],
            SINK => return None,
            _ => return Some(cells),
        }
    }
    None
}

/// The top water block under a surface in eighths.
fn channel_top(surface: i32) -> i32 {
    (surface - 1).div_euclid(8)
}

/// The cells of a straight, 8-connected line from `a` to `b`, both included.
fn line(a: IVec2, b: IVec2) -> Vec<IVec2> {
    let d = b - a;
    let steps = d.abs().max_element();
    (0..=steps)
        .map(|i| a + (d.as_vec2() * (i as f32 / steps.max(1) as f32)).round().as_ivec2())
        .collect()
}

/// What a pit becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The river cuts through the rim, never rising above the pit.
    Breach,
    /// A lake at its spill height, drained by a river from its outlet.
    Lake,
    /// A lake below its spill height, or a dry basin, with no outlet.
    Closed,
    /// The basin drains through a tunnel under its rim.
    Tunnel,
    /// Too big a basin for a lake: the water sinks into the ground at the pit.
    Sink,
}

/// A pit and what water does there.
pub struct Lake {
    pub pit: IVec2,
    pub kind: Kind,
    /// Height at which the basin overflows, in blocks.
    pub spill: f32,
    /// In eighths of a block: the water surface of a lake, or the level a river keeps
    /// through a breach or tunnel.
    pub level: i32,
    /// Height of the bottom of the pit.
    pub bottom: f32,
    /// Basin cells lower than the spill height.
    pub cells: Vec<IVec2>,
    /// The way out: the pit, the cells water crosses, and the first cell beyond the rim.
    pub path: Vec<IVec2>,
    /// Cells, along and beyond the way out, whose river may not rise above `level`.
    pub clamp: Vec<IVec2>,
    /// A hot spring: a small, warm pool that never freezes, rimmed with travertine.
    pub spring: bool,
    /// Filled by the highland tarn rule, rather than an ordinary or closed lake.
    pub tarn: bool,
    /// Whether the pit passes the tarn rule's tests that read no climate or geometry, so
    /// that weighing tarns may decide it otherwise than the ordinary pass did.
    tarn_roll: bool,
}

impl Lake {
    /// Whether the lake holds water: at least half a block over its bottom.
    pub fn wet(&self) -> bool {
        matches!(self.kind, Kind::Lake | Kind::Closed) && self.level as f32 >= (self.bottom + 0.5) * 8.0
    }

    /// Whether the basin is closed and dry enough to crust with salt.
    pub fn salt(&self) -> bool {
        self.kind == Kind::Closed && (self.level as f32) < (self.bottom + 2.0) * 8.0
    }
}

impl Weigh for Lake {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + (self.cells.capacity() + self.path.capacity() + self.clamp.capacity()) * 8
    }
}

/// Water entering a region: a cell inside, fed by cells just outside.
struct Entry {
    cell: IVec2,
    feeders: Vec<IVec2>,
}

/// Water leaving a region from a cell inside.
struct Exit {
    cell: IVec2,
    /// The cell outside the region it drains into, by its own region's routing.
    to: IVec2,
    /// Whether it ends a circle of crossings (see the module's docs), once known.
    cut: Shared<bool>,
    /// Water gathered inside the region.
    local: f32,
    /// The entries whose water passes through this exit.
    upstream: Vec<u32>,
    total: Shared<f32>,
}

/// A region's record of where water crosses its border.
pub struct Flow {
    entries: Vec<Entry>,
    /// Sorted by cell (x, then z).
    exits: Vec<Exit>,
}

impl Weigh for Flow {
    fn bytes(&self) -> usize {
        let entries: usize = self.entries.iter().map(|e| 32 + e.feeders.len() * 8).sum();
        let exits: usize = self.exits.iter().map(|e| 80 + e.upstream.len() * 4).sum();
        entries + exits
    }
}

/// The routing and gathered water of one region.
pub struct Basin {
    pub region: IVec2,
    /// First cell of the core: the region plus `CORE` cells around it.
    core_origin: IVec2,
    core: i32,
    /// Over the core: routing (a direction, `SEA`, `ABSORBED` or `END`), the river
    /// surface in eighths, and whether a tunnel carries the river.
    next: Box<[u8]>,
    level: Box<[i32]>,
    tunnel: Box<[bool]>,
    /// Whether a lake's water covers the cell's centre.
    submerged: Box<[bool]>,
    /// Over the region: water gathered from inside it.
    local: Box<[f32]>,
    /// Lakes, wet or dry, whose basins reach the core.
    pub lakes: Vec<Arc<Lake>>,
    flow: Arc<Flow>,
    totals: Shared<Box<[f32]>>,
    /// Whether the tarn rule passes over every pit the basin examined.
    tarn_free: bool,
}

impl Basin {
    fn core_index(&self, cell: IVec2) -> Option<usize> {
        let p = cell - self.core_origin;
        ((0..self.core).contains(&p.x) && (0..self.core).contains(&p.y)).then(|| (p.y * self.core + p.x) as usize)
    }

    /// Where water goes from a cell within `CORE` cells of the region.
    pub fn next(&self, cell: IVec2) -> u8 {
        self.core_index(cell).map_or(END, |i| self.next[i])
    }

    /// The river surface at a cell within `CORE` cells of the region, in eighths.
    pub fn level(&self, cell: IVec2) -> i32 {
        self.core_index(cell).map_or(SEA_SURFACE, |i| self.level[i])
    }

    /// Whether a lake's water covers a cell within `CORE` cells of the region.
    pub fn submerged(&self, cell: IVec2) -> bool {
        self.core_index(cell).is_some_and(|i| self.submerged[i])
    }

    /// Whether the river at a cell within `CORE` cells of the region runs in a tunnel.
    pub fn tunnel(&self, cell: IVec2) -> bool {
        self.core_index(cell).is_some_and(|i| self.tunnel[i])
    }
}

impl Weigh for Basin {
    /// Including the lakes and border record it keeps alive.
    fn bytes(&self) -> usize {
        let lakes: usize = self.lakes.iter().map(|l| 8 + l.bytes()).sum();
        let totals = self.totals.get().map_or(0, |t| t.bytes());
        self.next.bytes() + self.level.bytes() + self.tunnel.bytes() + self.submerged.bytes() + self.local.bytes() + lakes + self.flow.bytes() + totals
    }
}

/// How many bytes a tier's caches may hold.
pub struct Budget {
    pub lakes: usize,
    pub basins: usize,
    pub flows: usize,
}

/// One tier's routing, lakes and flow, cached by region.
pub struct Drainage {
    pub tier: Tier,
    lakes: Cache<IVec2, Arc<Lake>>,
    /// Ordinary pit decisions and routing, before tarn geometry is considered. Stream
    /// tarns keep the ordinary way out, so this publishes flow without asking geometry
    /// that may itself need the completed flow.
    ordinary_lakes: Cache<IVec2, Arc<Lake>>,
    routing: Cache<IVec2, Arc<Basin>>,
    basins: Cache<IVec2, Arc<Basin>>,
    flows: Cache<IVec2, Arc<Flow>>,
    /// Distinguishes this drainage in the per-thread guard against loops.
    id: u64,
}

/// How often the guard against loops had to end one, in tests: never, now that circles of
/// crossings are cut where they close.
/// Breaks every loop in `next`, a way per cell of a `core`-wide square (`k < 8` a
/// direction, anything else an end), at the cell on the loop whose way was set by the
/// lowest `writer` (raw routing, `usize::MAX`, counts as the highest), giving it `fallback`
/// instead; a loop that still closes ends there. Deterministic: cells are visited in order.
fn break_loops(next: &mut [u8], tunnel: &mut [bool], writer: &[usize], core: i32, fallback: impl Fn(usize) -> u8) {
    let n = (core * core) as usize;
    let step = |next: &[u8], i: usize| -> Option<usize> {
        let k = next[i];
        if k >= 8 {
            return None;
        }
        let d = DIRS[k as usize];
        let (x, z) = (i as i32 % core + d.x, i as i32 / core + d.y);
        ((0..core).contains(&x) && (0..core).contains(&z)).then(|| (z * core + x) as usize)
    };
    // 0 unseen, 1 on the current walk, 2 done.
    let mut state = vec![0u8; n];
    let mut tried = vec![false; n];
    for start in 0..n {
        let mut walk = Vec::new();
        let mut at = Some(start);
        while let Some(i) = at {
            match state[i] {
                2 => break,
                1 => {
                    // A loop: the cells of the walk from `i` on.
                    let from = walk.iter().position(|&c| c == i).unwrap();
                    let cut = *walk[from..].iter().min_by_key(|&&c| (writer[c], c)).unwrap();
                    let way = fallback(cut);
                    next[cut] = if tried[cut] || way == next[cut] { END } else { way };
                    tried[cut] = true;
                    tunnel[cut] = false;
                    // Walk again from the start with the loop broken.
                    for &c in &walk {
                        state[c] = 0;
                    }
                    walk.clear();
                    at = Some(start);
                    continue;
                }
                _ => {
                    state[i] = 1;
                    walk.push(i);
                    at = step(next, i);
                }
            }
        }
        for &c in &walk {
            state[c] = 2;
        }
    }
}

#[cfg(test)]
pub(crate) static LOOPS_ENDED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

thread_local! {
    /// Exits whose totals this thread is working out, so that a loop in the water (which
    /// routing rules out) would end instead of waiting on itself.
    static PENDING: RefCell<FxHashSet<(u64, IVec2)>> = RefCell::default();
}

impl Drainage {
    pub fn new(tier: Tier, budget: Budget) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stages = if tier.tarns.is_some() { 2 } else { 1 };
        Self {
            tier,
            // Pure functions of their regions, which the server's tick may read: copying.
            lakes: Cache::copying(budget.lakes / stages),
            ordinary_lakes: Cache::copying(budget.lakes / 2),
            basins: Cache::copying(budget.basins / stages),
            routing: Cache::copying(budget.basins / 2),
            flows: Cache::copying(budget.flows),
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// The region containing a cell.
    pub fn region_of(&self, cell: IVec2) -> IVec2 {
        cell.div_euclid(IVec2::splat(self.tier.side))
    }

    pub fn basin(&self, ground: &dyn Ground, region: IVec2) -> Arc<Basin> {
        self.basins.get(region, || {
            // Where the tarn rule passes over every pit, weighing tarns changes nothing:
            // the basin is its routing, made once.
            if self.tier.tarns.is_some() {
                let routing = self.routing(ground, region);
                if routing.tarn_free {
                    return routing;
                }
            }
            Arc::new(self.compute_basin(ground, region, true))
        })
    }

    /// Routing and flow without a tarn's changed water level, cells or clamps. In the
    /// stream tier (no tunnels), its ordinary and tarn paths are identical.
    pub fn routing(&self, ground: &dyn Ground, region: IVec2) -> Arc<Basin> {
        if self.tier.tarns.is_none() {
            return self.basin(ground, region);
        }
        self.routing.get(region, || Arc::new(self.compute_basin(ground, region, false)))
    }

    /// A region's border record, which outlives its basin in a cache of its own, since
    /// it is small and every region downstream reads it.
    fn flow(&self, ground: &dyn Ground, region: IVec2) -> Arc<Flow> {
        self.flows.get(region, || self.routing(ground, region).flow.clone())
    }

    /// Water flowing through every cell of a basin's region, from everywhere upstream
    /// (rows of `Tier::side` cells).
    pub fn totals<'b>(&self, ground: &dyn Ground, basin: &'b Basin) -> &'b [f32] {
        basin.totals.get_or_make(|| {
            self.prepare_upstream(ground, basin.flow.entries.iter());
            let mut totals = basin.local.clone();
            let side = self.tier.side;
            let origin = basin.region * side;
            for entry in &basin.flow.entries {
                let inflow = self.inflow(ground, entry);
                // Carry it downstream through the region.
                let mut cell = entry.cell;
                for _ in 0..side * side {
                    let p = cell - origin;
                    if !((0..side).contains(&p.x) && (0..side).contains(&p.y)) {
                        break;
                    }
                    totals[(p.y * side + p.x) as usize] += inflow;
                    match basin.next(cell) {
                        k if k < 8 => cell += DIRS[k as usize],
                        _ => break,
                    }
                }
            }
            totals
        })
    }

    /// Water leaving its region through `cell`, which must drain out of that region.
    pub fn exit_total(&self, ground: &dyn Ground, cell: IVec2) -> f32 {
        let flow = self.flow(ground, self.region_of(cell));
        let Some(exit) = Self::exit_of(&flow, cell) else {
            debug_assert!(false, "{cell} drains out of its region in a neighbour's view but not its own");
            return 0.0;
        };
        if let Some(&total) = exit.total.get() {
            return total;
        }
        let key = (self.id, cell);
        if !PENDING.with(|pending| pending.borrow_mut().insert(key)) {
            #[cfg(test)]
            LOOPS_ENDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug_assert!(false, "water loops through {cell}");
            return exit.local;
        }
        let total = *exit.total.get_or_make(|| {
            self.prepare_upstream(ground, exit.upstream.iter().map(|&e| &flow.entries[e as usize]));
            exit.local + exit.upstream.iter().map(|&e| self.inflow(ground, &flow.entries[e as usize])).sum::<f32>()
        });
        PENDING.with(|pending| pending.borrow_mut().remove(&key));
        total
    }

    /// Makes on every core the border records of the regions that feed `entries` and are
    /// not made yet, which working out their inflow reads next.
    fn prepare_upstream<'e>(&self, ground: &dyn Ground, entries: impl Iterator<Item = &'e Entry>) {
        let mut regions: Vec<IVec2> = entries.flat_map(|e| e.feeders.iter().map(|&f| self.region_of(f))).collect();
        regions.sort_unstable_by_key(|r| (r.x, r.y));
        regions.dedup();
        regions.retain(|&r| !self.flows.has(r));
        crate::parallel::for_each(&regions, |&r| {
            self.flow(ground, r);
        });
    }

    /// Water entering through `entry`: the totals of the cells outside whose own regions
    /// send their water into it, but for a cell that ends a circle.
    fn inflow(&self, ground: &dyn Ground, entry: &Entry) -> f32 {
        entry.feeders.iter().filter(|&&feeder| self.feeds(ground, feeder, entry.cell)).map(|&feeder| self.exit_total(ground, feeder)).sum()
    }

    /// The exit record of a cell in its own region, if its region sends water out there.
    fn exit_of(flow: &Flow, cell: IVec2) -> Option<&Exit> {
        flow.exits.binary_search_by(|exit| (exit.cell.x, exit.cell.y).cmp(&(cell.x, cell.y))).ok().map(|i| &flow.exits[i])
    }

    /// Whether `feeder` passes its water into the entry at `cell`.
    fn feeds(&self, ground: &dyn Ground, feeder: IVec2, cell: IVec2) -> bool {
        let flow = self.flow(ground, self.region_of(feeder));
        Self::exit_of(&flow, feeder).is_some_and(|exit| exit.to == cell) && !self.cut(ground, feeder)
    }

    /// The exit downstream of the exit at `cell`, where the water it sends out leaves the
    /// next region; `None` where it ends there (the sea, a closed lake).
    fn next_exit(&self, ground: &dyn Ground, cell: IVec2) -> Option<IVec2> {
        let to = Self::exit_of(&self.flow(ground, self.region_of(cell)), cell)?.to;
        let flow = self.flow(ground, self.region_of(to));
        let entry = flow.entries.iter().position(|e| e.cell == to)? as u32;
        flow.exits.iter().find(|exit| exit.upstream.binary_search(&entry).is_ok()).map(|exit| exit.cell)
    }

    /// Whether the exit at `cell` lies on a circle of crossings and has the lowest cell of
    /// it, so that its water stops there and the circle is broken.
    fn cut(&self, ground: &dyn Ground, cell: IVec2) -> bool {
        /// Longest chain of crossings followed; rivers cross far fewer regions than this.
        const LONGEST: usize = 512;
        let flow = self.flow(ground, self.region_of(cell));
        let Some(exit) = Self::exit_of(&flow, cell) else { return false };
        *exit.cut.get_or_make(|| {
            let mut seen = vec![cell];
            let mut at = cell;
            while seen.len() < LONGEST {
                match self.next_exit(ground, at) {
                    None => return false,
                    Some(next) if next == cell => return seen.iter().all(|c| (c.x, c.y) >= (cell.x, cell.y)),
                    Some(next) if seen.contains(&next) => return false,
                    Some(next) => {
                        seen.push(next);
                        at = next;
                    }
                }
            }
            false
        })
    }

    /// What water does at a pit.
    fn lake(&self, reader: &mut Reader, pit: IVec2, tarns: bool) -> Arc<Lake> {
        let cache = if tarns { &self.lakes } else { &self.ordinary_lakes };
        cache.get(pit, || {
            // A pit the tarn rule passes over is the same either way: flooded once.
            if tarns {
                let ordinary = self.lake(reader, pit, false);
                if !ordinary.tarn_roll {
                    return ordinary;
                }
            }
            let mut lake = self.flood(reader, pit, tarns);
            lake.cells = match lake.kind {
                Kind::Lake if lake.tarn => lake.cells,
                Kind::Lake | Kind::Closed => self.under(reader, pit, lake.level),
                _ => Vec::new(),
            };
            Arc::new(lake)
        })
    }

    /// The cells around a pit, connected to it, whose ground lies under a water surface
    /// (in eighths): the cells of its lake. Ground beyond the spill point stays dry, since
    /// the spill point itself is never under water.
    fn under(&self, reader: &mut Reader, pit: IVec2, surface: i32) -> Vec<IVec2> {
        let top = channel_top(surface);
        let radius = self.tier.lake_radius;
        let mut cells = vec![pit];
        let mut seen = FxHashSet::default();
        seen.insert(pit);
        let mut head = 0;
        while head < cells.len() {
            let cell = cells[head];
            head += 1;
            for d in DIRS {
                let next = cell + d;
                if (next - pit).abs().max_element() <= radius && seen.insert(next) && (reader.at(next).0.floor() as i32) < top {
                    cells.push(next);
                }
            }
        }
        cells.sort_unstable_by_key(|c| (c.x, c.y));
        cells
    }

    /// Floods a pit until it overflows, and decides what it becomes. The cells it
    /// returns are those the flood passed, for finding tunnels; `lake` replaces them.
    fn flood(&self, reader: &mut Reader, pit: IVec2, tarns: bool) -> Lake {
        let tier = &self.tier;
        let ground = reader.ground;
        let mut at = |c: IVec2| reader.at(c);
        let bottom = at(pit).0;
        let floor = Key { height: bottom, cell: pit };

        // Whether water at a cell runs off to somewhere lower than this pit.
        let mut drains: FxHashMap<IVec2, bool> = FxHashMap::default();
        let mut drains_lower = |start: IVec2, at: &mut dyn FnMut(IVec2) -> (f32, i32)| -> bool {
            let mut at = |c: IVec2| at(c);
            let mut walked = Vec::new();
            let mut cell = start;
            let result = loop {
                if let Some(&known) = drains.get(&cell) {
                    break known;
                }
                walked.push(cell);
                match route(&mut at, cell) {
                    SEA | ABSORBED => break true,
                    SINK => break Key { height: at(cell).0, cell } < floor,
                    k => cell += DIRS[k as usize],
                }
                if walked.len() > 4096 {
                    break true;
                }
            };
            for cell in walked {
                drains.insert(cell, result);
            }
            result
        };

        // Priority flood: always take the lowest cell on the shore. Once the water level
        // passes a cell that drains somewhere lower, the basin overflows there.
        let mut heap = BinaryHeap::new();
        let mut seen = FxHashSet::default();
        seen.insert(pit);
        let mut parent: FxHashMap<IVec2, IVec2> = FxHashMap::default();
        let mut expand = |cell: IVec2, heap: &mut BinaryHeap<Reverse<Key>>, at: &mut dyn FnMut(IVec2) -> (f32, i32)| {
            for d in DIRS {
                let next = cell + d;
                if (next - pit).abs().max_element() <= tier.lake_radius && seen.insert(next) {
                    parent.insert(next, cell);
                    heap.push(Reverse(Key { height: at(next).0, cell: next }));
                }
            }
        };
        heap.push(Reverse(floor));
        let mut level = bottom;
        let mut cells = Vec::new();
        let mut outlet = None;
        let mut overflowed = false;
        while let Some(Reverse(key)) = heap.pop() {
            if cells.len() >= tier.lake_cells || (key.cell - pit).abs().max_element() >= tier.lake_radius {
                overflowed = true;
                break;
            }
            if key.height < level && drains_lower(key.cell, &mut at) {
                outlet = Some(key.cell);
                break;
            }
            level = level.max(key.height);
            cells.push(key.cell);
            expand(key.cell, &mut heap, &mut at);
        }
        let spill = level;
        cells.sort_unstable_by_key(|c| (c.x, c.y));
        let mut lake = Lake { pit, kind: Kind::Closed, spill, level: 0, bottom, cells, path: Vec::new(), clamp: Vec::new(), spring: false, tarn: false, tarn_roll: false };
        let way_out = |outlet: IVec2| {
            // Parents form a tree rooted at the pit, so this ends there; the bound is a
            // second guard that keeps any mistake from growing without limit.
            let mut path = vec![outlet];
            let mut cell = outlet;
            while let Some(&p) = parent.get(&cell) {
                path.push(p);
                cell = p;
                if path.len() > parent.len() + 1 {
                    debug_assert!(false, "lake parents loop at {cell}");
                    break;
                }
            }
            path.reverse();
            path
        };
        let unit = |salt: i32| (hash3(tier.seed ^ 0x1a4e, pit.x, salt, pit.y) >> 40) as f32 / (1u64 << 24) as f32;
        let pit_level = tier.target(bottom);
        let depth = spill - bottom;

        if overflowed {
            // Bigger than a lake may be: the water sinks into the ground here.
            lake.kind = Kind::Sink;
            lake.cells.clear();
            lake.level = pit_level;
            return lake;
        }
        // In dry climates basins evaporate down by up to eight blocks below their spill
        // height, so they have no outlet, down to dry, salty flats.
        let big = depth >= tier.min_depth && lake.cells.len() >= tier.min_area;
        let drop = if big { (unit(2) * 8.0 * (1.0 - ground.humidity(pit))).floor() } else { 0.0 };
        if drop >= 1.0 || outlet.is_none() {
            let tunnelled = outlet.is_none() && tier.tunnel.as_ref().is_some_and(|rule| self.tunnel(&mut at, &mut lake, rule));
            if !tunnelled {
                lake.level = if tier.lakes { ((spill - 1.0 - drop) * 8.0).floor() as i32 } else { pit_level };
            }
            return lake;
        }
        let outlet = outlet.expect("checked above");
        // Now and then, where the ground is volcanic, a small pit is a hot spring.
        if !big && depth >= 1.5 && unit(3) < tier.springs {
            lake.kind = Kind::Lake;
            lake.spring = true;
            lake.path = way_out(outlet);
            lake.level = ((spill - 1.0) * 8.0).floor() as i32;
            return lake;
        }
        // High in the hills, now and then a small pit holds a tarn rather than being breached.
        // The cheap roll first: most pits are passed over without weighing their climate.
        lake.tarn_roll = !big && tier.tarns.as_ref().is_some_and(|rule| bottom >= rule.floor && depth >= rule.depth && unit(4) < rule.chance);
        if tarns && lake.tarn_roll
            && let Some(rule) = &tier.tarns
            && unit(4) < rule.chance * ground.tarns(pit)
            && let Some(cells) = ground.tarn_cells(pit, ((spill - 1.0) * 8.0).floor() as i32, rule.size)
        {
            lake.kind = Kind::Lake;
            lake.tarn = true;
            lake.cells = cells;
            lake.path = way_out(outlet);
            lake.level = ((spill - 1.0) * 8.0).floor() as i32;
            return lake;
        }
        if let Some(rule) = &tier.tunnel
            && depth >= rule.depth
            && unit(1) < rule.chance
            && self.tunnel(&mut at, &mut lake, rule)
        {
            return lake;
        }
        // Shallow or small pits are breached: the river keeps the pit's level until the
        // land beyond falls below it. Without lakes, any pit is, as far as a breach can
        // reach, then by a tunnel.
        let path = way_out(outlet);
        if (!tier.lakes || depth < tier.min_depth || lake.cells.len() < tier.min_area)
            && let Some(beyond) = descend_below(&mut at, tier, outlet, pit_level, tier.breach)
        {
            lake.kind = Kind::Breach;
            lake.level = pit_level;
            lake.clamp = path.iter().copied().chain(beyond).collect();
            lake.path = path;
            return lake;
        }
        if !tier.lakes {
            if let Some(rule) = &tier.tunnel
                && self.tunnel(&mut at, &mut lake, rule)
            {
                return lake;
            }
            // Cut through however far it takes, which makes a gorge.
            let beyond = descend_below(&mut at, tier, outlet, pit_level, 4 * tier.breach).unwrap_or_default();
            lake.kind = Kind::Breach;
            lake.level = pit_level;
            lake.clamp = path.iter().copied().chain(beyond).collect();
            lake.path = path;
            return lake;
        }
        // A lake one block under its spill height, drained from its outlet.
        lake.kind = Kind::Lake;
        lake.path = path;
        lake.level = ((spill - 1.0) * 8.0).floor() as i32;
        lake
    }

    /// Turns a basin into one that drains through a tunnel to the nearest lower ground
    /// beyond its rim, if there is any within reach.
    fn tunnel(&self, at: &mut impl FnMut(IVec2) -> (f32, i32), lake: &mut Lake, rule: &TunnelRule) -> bool {
        let pit = lake.pit;
        let inside = |c: IVec2| lake.cells.binary_search_by_key(&(c.x, c.y), |c| (c.x, c.y)).is_ok();
        for r in 2..=rule.length {
            let mut best: Option<Key> = None;
            for i in -r..r {
                for c in [pit + IVec2::new(i, -r), pit + IVec2::new(r, i), pit + IVec2::new(-i, r), pit + IVec2::new(-r, -i)] {
                    let (height, _) = at(c);
                    let key = Key { height, cell: c };
                    if height < lake.bottom - 2.0 && !inside(c) && best.is_none_or(|b| key < b) {
                        best = Some(key);
                    }
                }
            }
            if let Some(end) = best {
                lake.kind = Kind::Tunnel;
                lake.level = self.tier.target(lake.bottom);
                lake.path = line(pit, end.cell);
                lake.clamp = lake.path.clone();
                return true;
            }
        }
        false
    }

    fn compute_basin(&self, ground: &dyn Ground, region: IVec2, tarns: bool) -> Basin {
        let tier = &self.tier;
        let side = tier.side;
        let halo = tier.halo();
        let origin = region * side;
        let low = origin - halo;
        let width = side + 2 * halo;
        let mut reader = Reader::new(tier, ground);

        // The tiles of the window, made on every core, then a dense copy of the window and
        // the runoff of the region.
        let size = tier.tile;
        let (first, last) = (low.div_euclid(IVec2::splat(size)), (low + width - 1).div_euclid(IVec2::splat(size)));
        let keys: Vec<IVec2> = (first.y..=last.y).flat_map(|ty| (first.x..=last.x).map(move |tx| IVec2::new(tx, ty))).collect();
        let missing: Vec<IVec2> = keys.iter().copied().filter(|&key| !ground.has(key)).collect();
        crate::parallel::for_each(&missing, |&key| {
            ground.tile(key);
        });
        let mut heights = vec![0.0f32; (width * width) as usize];
        let mut covered = vec![DRY; (width * width) as usize];
        let mut runoff = vec![0.0f32; (side * side) as usize];
        for &key in &keys {
            let tile = reader.tile(key).clone();
            for j in 0..size {
                for i in 0..size {
                    let cell = key * size + IVec2::new(i, j);
                    let (w, t) = (cell - low, (j * size + i) as usize);
                    if (0..width).contains(&w.x) && (0..width).contains(&w.y) {
                        heights[(w.y * width + w.x) as usize] = tile.height[t];
                        covered[(w.y * width + w.x) as usize] = tile.absorbed[t];
                    }
                    let r = cell - origin;
                    if (0..side).contains(&r.x) && (0..side).contains(&r.y) {
                        runoff[(r.y * side + r.x) as usize] = tile.runoff[t];
                    }
                }
            }
        }
        let mut at = |c: IVec2| {
            let w = c - low;
            debug_assert!((0..width).contains(&w.x) && (0..width).contains(&w.y), "{c} outside the window");
            let i = (w.y.clamp(0, width - 1) * width + w.x.clamp(0, width - 1)) as usize;
            (heights[i], covered[i])
        };

        // Raw routing wherever it can be worked out from the window (two cells short of
        // its edge), and every pit there with its lake.
        // Each cell's steepest descent is found once, one cell short of the edge, then
        // shared by the crossing rule of its neighbours.
        let inner = width - 2;
        let best = steepest_inside(&heights, width);
        let span = width - 4;
        let raw: Vec<u8> = (0..span * span)
            .map(|i| {
                let cell = low + 2 + IVec2::new(i % span, i / span);
                route_with(&mut at, &|c: IVec2| best[((c.y - low.y - 1) * inner + c.x - low.x - 1) as usize], cell)
            })
            .collect();
        let raw_at = |c: IVec2| raw[((c.y - low.y - 2) * span + c.x - low.x - 2) as usize];
        let mut lakes: Vec<Arc<Lake>> = Vec::new();
        let reach = tier.reach() + CORE + 1;
        for z in origin.y - reach..origin.y + side + reach {
            for x in origin.x - reach..origin.x + side + reach {
                let cell = IVec2::new(x, z);
                if raw_at(cell) == SINK {
                    lakes.push(self.lake(&mut reader, cell, tarns));
                }
            }
        }
        let tarn_free = !lakes.iter().any(|lake| lake.tarn_roll);
        // Where two lakes' ways out cross, the bigger lake's wins.
        lakes.sort_by(|a, b| a.spill.total_cmp(&b.spill).then((a.pit.x, a.pit.y).cmp(&(b.pit.x, b.pit.y))));

        // Routing and river levels over the core.
        let core = side + 2 * CORE;
        let core_origin = origin - CORE;
        let core_index = |c: IVec2| {
            let p = c - core_origin;
            ((0..core).contains(&p.x) && (0..core).contains(&p.y)).then(|| (p.y * core + p.x) as usize)
        };
        let mut next = vec![END; (core * core) as usize];
        let mut clamp = vec![i32::MAX; (core * core) as usize];
        let mut floor = vec![i32::MIN; (core * core) as usize];
        let mut tunnel = vec![false; (core * core) as usize];
        for i in 0..core * core {
            let cell = core_origin + IVec2::new(i % core, i / core);
            next[i as usize] = match raw_at(cell) {
                SINK => END,
                k => k,
            };
        }
        // Which lake last set each cell's way (lakes are sorted smallest first).
        let mut writer = vec![usize::MAX; (core * core) as usize];
        for (l, lake) in lakes.iter().enumerate() {
            for pair in lake.path.windows(2) {
                if let Some(i) = core_index(pair[0]) {
                    next[i] = direction(pair[1] - pair[0]);
                    tunnel[i] |= lake.kind == Kind::Tunnel;
                    writer[i] = l;
                }
            }
            for &cell in &lake.clamp {
                if let Some(i) = core_index(cell) {
                    clamp[i] = clamp[i].min(lake.level);
                }
            }
            if lake.wet() {
                for &cell in &lake.cells {
                    if let Some(i) = core_index(cell) {
                        floor[i] = floor[i].max(lake.level);
                    }
                }
            }
        }
        // A bigger lake's way out can end on a cell whose way, set by a smaller lake,
        // leads back into it, closing a loop. Each loop gives way at the cell set by the
        // smallest lake on it, which returns to its own steepest descent (or ends there).
        break_loops(&mut next, &mut tunnel, &writer, core, |i| match raw_at(core_origin + IVec2::new(i as i32 % core, i as i32 / core)) {
            SINK => END,
            k => k,
        });
        let submerged: Vec<bool> = (0..core * core)
            .map(|i| {
                let height = at(core_origin + IVec2::new(i % core, i / core)).0;
                let floor = floor[i as usize];
                floor != i32::MIN && (height.floor() as i32) < (floor - 1).div_euclid(8)
            })
            .collect();
        let level: Vec<i32> = (0..core * core)
            .map(|i| {
                let cell = core_origin + IVec2::new(i % core, i / core);
                let (height, absorbed) = at(cell);
                let i = i as usize;
                match next[i] {
                    SEA => SEA_SURFACE,
                    ABSORBED => absorbed,
                    _ => tier.target(height).min(clamp[i]).max(floor[i]).max(SEA_SURFACE),
                }
            })
            .collect();

        // Gather the water inside the region, upstream first. Routing never loops, but
        // should it, the loop's water would end there.
        let region_index = |c: IVec2| {
            let p = c - origin;
            ((0..side).contains(&p.x) && (0..side).contains(&p.y)).then(|| (p.y * side + p.x) as usize)
        };
        let downstream = |i: usize, next: &[u8]| -> Option<usize> {
            let cell = origin + IVec2::new(i as i32 % side, i as i32 / side);
            let k = next[core_index(cell)?];
            if k < 8 { region_index(cell + DIRS[k as usize]) } else { None }
        };
        let order = loop {
            let mut incoming = vec![0u32; (side * side) as usize];
            for i in 0..(side * side) as usize {
                if let Some(d) = downstream(i, &next) {
                    incoming[d] += 1;
                }
            }
            let mut order: Vec<usize> = (0..(side * side) as usize).filter(|&i| incoming[i] == 0).collect();
            let mut head = 0;
            while head < order.len() {
                if let Some(d) = downstream(order[head], &next) {
                    incoming[d] -= 1;
                    if incoming[d] == 0 {
                        order.push(d);
                    }
                }
                head += 1;
            }
            if order.len() == (side * side) as usize {
                break order;
            }
            debug_assert!(false, "water loops in region {region}");
            for (i, &n) in incoming.iter().enumerate() {
                if n > 0 {
                    let cell = origin + IVec2::new(i as i32 % side, i as i32 / side);
                    next[core_index(cell).unwrap()] = END;
                }
            }
        };

        // Where water crosses the border.
        let mut entries: Vec<Entry> = Vec::new();
        let mut entry_of: FxHashMap<usize, u32> = FxHashMap::default();
        for i in 0..core * core {
            let cell = core_origin + IVec2::new(i % core, i / core);
            let p = cell - origin;
            let ring = (-1..=side).contains(&p.x) && (-1..=side).contains(&p.y) && region_index(cell).is_none();
            let k = next[i as usize];
            if ring && k < 8 && let Some(target) = region_index(cell + DIRS[k as usize]) {
                let e = *entry_of.entry(target).or_insert_with(|| {
                    entries.push(Entry { cell: cell + DIRS[k as usize], feeders: Vec::new() });
                    entries.len() as u32 - 1
                });
                entries[e as usize].feeders.push(cell);
            }
        }
        let mut local = runoff;
        let mut upstream: Vec<Vec<u32>> = vec![Vec::new(); (side * side) as usize];
        let mut exits = Vec::new();
        for &i in &order {
            if let Some(&e) = entry_of.get(&i) {
                upstream[i].push(e);
                upstream[i].sort_unstable();
            }
            match downstream(i, &next) {
                Some(d) => {
                    local[d] += local[i];
                    let carried = std::mem::take(&mut upstream[i]);
                    let merged = &mut upstream[d];
                    merged.extend(carried.iter().copied());
                    merged.sort_unstable();
                    merged.dedup();
                    upstream[i] = carried;
                }
                None => {
                    let cell = origin + IVec2::new(i as i32 % side, i as i32 / side);
                    if next[core_index(cell).unwrap()] < 8 {
                        let to = cell + DIRS[next[core_index(cell).unwrap()] as usize];
                        exits.push(Exit { cell, to, cut: Shared::default(), local: local[i], upstream: upstream[i].clone(), total: Shared::default() });
                    }
                }
            }
        }
        exits.sort_unstable_by_key(|exit| (exit.cell.x, exit.cell.y));

        // Lakes, wet or dry, that reach the core.
        lakes.retain(|lake| lake.cells.iter().any(|&c| core_index(c).is_some()));
        lakes.sort_by_key(|lake| (lake.pit.x, lake.pit.y));
        Basin {
            region,
            core_origin,
            core,
            next: next.into(),
            level: level.into(),
            tunnel: tunnel.into(),
            submerged: submerged.into(),
            local: local.into(),
            lakes,
            flow: Arc::new(Flow { entries, exits }),
            totals: Shared::default(),
            tarn_free,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The steepest descent read straight from a window is the rule's, ties included.
    #[test]
    fn loops_break_at_the_smallest_lakes_cell() {
        // A 3-wide square: cell 0 (set by lake 1) points east into cell 1, which (set by
        // lake 0) points back west. Lake 0's cell gives way to its fallback, south.
        let mut next = vec![END; 9];
        next[0] = 0;
        next[1] = 4;
        let mut tunnel = vec![false; 9];
        let writer = [1, 0, usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX];
        break_loops(&mut next, &mut tunnel, &writer, 3, |_| 2);
        assert_eq!((next[0], next[1]), (0, 2));
        // A fallback that loops again ends the cell instead.
        let mut next = vec![END; 9];
        next[0] = 0;
        next[1] = 4;
        break_loops(&mut next, &mut tunnel, &writer, 3, |_| 4);
        assert_eq!((next[0], next[1]), (0, END));
    }

    #[test]
    fn total_order_matches_total_cmp() {
        let values = [f32::NEG_INFINITY, -1e9, -64.5, -1.0, -f32::MIN_POSITIVE, -0.0, 0.0, f32::MIN_POSITIVE, 0.5, 63.0, 63.000004, 1e9, f32::INFINITY];
        for a in values {
            for b in values {
                assert_eq!(total_order(a).cmp(&total_order(b)), a.total_cmp(&b), "{a} against {b}");
            }
        }
    }

    #[test]
    fn steepest_inside_matches_steepest() {
        let mut rng = crate::rng::Rng::new(5, 6, 7, 8);
        for width in [3, 4, 9, 40] {
            // Few distinct heights, so many neighbours tie.
            let heights: Vec<f32> = (0..width * width).map(|_| 120.0 + rng.range(0, 5) as f32 * 0.5).collect();
            let low = IVec2::new(-17, 30);
            let mut at = |c: IVec2| {
                let w = c - low;
                (heights[(w.y * width + w.x) as usize], DRY)
            };
            let inside = steepest_inside(&heights, width);
            for (i, &best) in inside.iter().enumerate() {
                let cell = low + 1 + IVec2::new(i as i32 % (width - 2), i as i32 / (width - 2));
                assert_eq!(best, steepest(&mut at, cell, None), "at {cell} in a window {width} wide");
            }
        }
    }
}
