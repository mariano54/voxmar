//! Stage 3b: hiking trails between the places worth walking to.
//!
//! Every region of 512 blocks has a few places of interest ([`sites`]): summits that
//! stand above the land around, lake shores, the banks of waterfalls, the edges of long
//! drops, and flat trailheads in the valleys. Nearby places are joined into a sparse
//! network ([`network`]); each trail is routed over the land by A* with costs that make it
//! follow the contours and switch back up steep slopes ([`route`]), merged into longer
//! trails it runs beside, and drawn into the ground ([`path`]): cut into hillsides and
//! raised on low banks, with stepping stones across shallow water, logs across creeks and
//! a deck across rivers. Trails are worn paths of packed earth, gravel or mud, not roads.
//! Cairns mark them above the treeline and stand on summits, old waymarkers stand at
//! trailheads, and summits and viewpoints keep a clearing free of trees.
//!
//! Trails lead to and between settlements, so they are only laid near one
//! ([`crate::content::TrailsWhere`]); with no settlements in the world yet, none are by
//! default.
//!
//! Everything is a pure function of the seed and the land, worked out by region on
//! demand and kept in caches bounded by bytes: the planning grid by patch, places by
//! region, the trails each region owns, and the stamps over 64-block tiles that columns
//! read. Where trails are laid everywhere a region owns the trails whose lower end lies
//! in it; near settlements, where every trail leaves a settlement's gate, it owns those
//! leaving its gates, so a region without a gate owns none and its land is never planned
//! unless a trail comes near. A trail reaches at most one region beyond its owner, so a
//! tile reads the trails of the 3×3 regions around it. Nothing depends on which column,
//! region or thread came first.

mod network;
mod path;
mod route;
mod sites;

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use voxmar_engine::glam::{IVec2, Vec2};

pub use self::sites::{Kind, Poi};
use self::path::{Path, Paths, Stamp};
use self::sites::{CELL, Cell, Grid, PATCH, REGION};
pub use crate::content::TrailsWhere;
use crate::geology::GeoSample;
use crate::rng;
use crate::stage::{self, Stage};
use crate::water::cache::{Cache, Weigh};

/// What a trail does to a column, as the rest of generation sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailMark {
    /// How much of the tread lies here, 0..1: worn ground towards 1.
    pub tread: f32,
    /// The height of a bridge deck over water here, whether it is of logs, and whether a
    /// pier holds it up here.
    pub deck: Option<i32>,
    pub log: bool,
    pub pier: bool,
    /// A stepping stone in shallow water here, flush with its surface.
    pub stone: bool,
    /// On the trail or its near bank, where no tree may stand.
    pub clear: bool,
}

/// Something built beside a trail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// A small cairn beside the trail, built where trees do not grow.
    Cairn,
    /// A larger cairn on a summit.
    Summit,
    /// An old, half-fallen waymarker at a trailhead.
    Waymarker,
}

/// The land trails are planned over: the ground with rivers and lakes, before trails.
pub trait Land: Sync {
    fn sample(&self, x: i32, z: i32) -> GeoSample;

    /// The same sample, sharing temporary terrain calculations within one planner.
    /// Synthetic land can keep the standalone implementation.
    fn sample_cached(&self, x: i32, z: i32, _scratch: &mut crate::water::Samples) -> GeoSample {
        self.sample(x, z)
    }

    /// At most 32 exact centre samples, sharing terrain calculations where supported.
    fn samples_cached(&self, points: &[(i32, i32)], scratch: &mut crate::water::Samples) -> Vec<GeoSample> {
        assert!(points.len() <= crate::water::SAMPLE_BATCH);
        points.iter().map(|&(x, z)| self.sample_cached(x, z, scratch)).collect()
    }

    /// Prove freshwater absent without a terrain sample, where supported. False
    /// means unknown, so synthetic land keeps its original full-sample predicate.
    fn freshwater_absent_cached(&self, _at: IVec2, _scratch: &mut crate::water::Samples) -> bool {
        false
    }

    /// Whether the close-look grid's circle contains freshwater. Synthetic land keeps
    /// its grid implementation; real river land can stop before unused terrain reads.
    /// Like settlement definitions, callers supply radius >= 0 and step >= 1.
    fn freshwater_near(&self, centre: IVec2, radius: i32, step: i32, _cache: &mut crate::water::Freshwater) -> bool {
        let first = centre - radius;
        let size = radius * 2 / step + 1;
        self.grid(first, step, size).iter().enumerate().any(|(i, g)| {
            let p = first + IVec2::new(i as i32 % size, i as i32 / size) * step;
            p.as_vec2().distance(centre.as_vec2()) <= radius as f32 && crate::water::freshwater(g)
        })
    }

    /// Reuse this land's already sampled grid in close-water queries where supported.
    /// Synthetic implementations keep their original full-grid query behaviour.
    fn remember_freshwater_grid(&self, _origin: IVec2, _step: i32, _size: i32, _samples: &[GeoSample], _cache: &mut crate::water::Freshwater) {}

    /// The `size`×`size` samples `step` blocks apart from `origin`, row by row: the same
    /// as sampling each, but free to share the work.
    fn grid(&self, origin: IVec2, step: i32, size: i32) -> Vec<GeoSample> {
        (0..size * size).map(|i| self.sample(origin.x + i % size * step, origin.y + i / size * step)).collect()
    }

    /// Whether a settlement lies within `reach` blocks of the box from `min` to `max`.
    fn settled(&self, _min: IVec2, _max: IVec2, _reach: i32) -> bool {
        false
    }

    /// Where settlements' lanes lead out within the box from `min` to `max`.
    fn gates(&self, _min: IVec2, _max: IVec2) -> Vec<IVec2> {
        Vec::new()
    }

    /// The ground settlements' buildings and walls take (each from its least corner to its
    /// most) within or touching the box from `min` to `max`, which trails keep off.
    fn built(&self, _min: IVec2, _max: IVec2) -> Vec<(IVec2, IVec2)> {
        Vec::new()
    }
}

/// How far from a settlement trails are laid: as far as a trail from its gate reaches,
/// with room for the detours routing takes.
const SETTLED_REACH: i32 = network::MAX_EDGE + 16 * CELL + 64;

/// Blocks along a side of a tile of stamps.
const TILE: i32 = 64;
const MB: usize = 1 << 20;

/// The trails of a world.
pub struct Trails {
    seed: u64,
    /// Where trails are laid.
    laid: TrailsWhere,
    /// The planning grid by patch.
    grids: Cache<IVec2, Arc<Grid>>,
    /// Settlements' gates by region.
    gates: Cache<IVec2, Arc<[Poi]>>,
    /// All places by region, gates among them.
    places: Cache<IVec2, Arc<[Poi]>>,
    routes: Cache<IVec2, Arc<Routes>>,
    paths: Cache<IVec2, Arc<Paths>>,
    tiles: Cache<IVec2, Arc<Tile>>,
    /// Tells this world's tiles apart in the per-thread cache.
    id: u64,
}

/// A routed trail, before snapping: the grid cells from its lower end to its upper.
#[derive(Clone, Debug)]
struct Route {
    cells: Vec<IVec2>,
    /// Length in blocks, and a tie-breaker; longer trails are trunks for shorter.
    length: f32,
    id: u64,
}

impl Route {
    /// Whether this trail ranks above `other`, as a trunk for it to join.
    fn above(&self, other: &Route) -> bool {
        (self.length, self.id) > (other.length, other.id)
    }
}

/// The trails a region owns.
struct Routes(Vec<Route>);

impl Weigh for Routes {
    fn bytes(&self) -> usize {
        self.0.iter().map(|r| std::mem::size_of::<Route>() + r.cells.len() * 8).sum()
    }
}

/// What trails do over one tile: per-column stamps (none where no trail comes), and
/// what is built there.
struct Tile {
    stamps: Option<Box<[Stamp]>>,
    markers: Vec<(IVec2, Marker)>,
}

impl Weigh for Tile {
    fn bytes(&self) -> usize {
        self.stamps.as_ref().map_or(0, |s| s.len() * std::mem::size_of::<Stamp>()) + self.markers.len() * 12
    }
}

thread_local! {
    /// The tiles this thread used last, sparing the shared cache's lock for the many
    /// columns of a chunk.
    static RECENT: RefCell<Vec<(u64, IVec2, Arc<Tile>)>> = const { RefCell::new(Vec::new()) };
}

impl Trails {
    pub fn new(seed: u64, laid: TrailsWhere) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            seed: rng::mix(seed ^ 0x7a11_5eed),
            laid,
            // Pure functions of their keys (settlement gates come from the plans, which wait),
            // which the server's tick may read: copying.
            grids: Cache::copying(48 * MB),
            gates: Cache::copying(MB),
            places: Cache::copying(MB),
            routes: Cache::copying(8 * MB),
            paths: Cache::copying(16 * MB),
            tiles: Cache::copying(48 * MB),
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// A column of `land` with the trails drawn into it.
    pub fn apply(&self, land: &dyn Land, mut sample: GeoSample, x: i32, z: i32) -> GeoSample {
        let tile = self.tile(land, IVec2::new(x, z).div_euclid(IVec2::splat(TILE)));
        let Some(stamps) = &tile.stamps else { return sample };
        let local = IVec2::new(x, z).rem_euclid(IVec2::splat(TILE));
        let stamp = stamps[(local.y * TILE + local.x) as usize];
        let Some(mark) = stamp.mark() else { return sample };
        if sample.submerged() {
            // A ford's stones fill the water up to its surface, unless a fall pours in
            // beside them: its water must land in water.
            let fall = || {
                [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().any(|(dx, dz)| land.sample(x + dx, z + dz).wet().is_some_and(|(_, top)| top >= sample.water + 2))
            };
            if mark.stone && sample.water - sample.height <= route::FORD_DEPTH && !fall() {
                sample.height = sample.water;
            }
        } else if sample.cavity.is_none() && stamp.weight > 0.0 {
            let blended = sample.height as f32 + (stamp.target - sample.height as f32) * stamp.weight;
            // Never below the water beside it, which would let it spill.
            sample.height = (blended.round() as i32).max(sample.water);
        }
        sample.trail = Some(mark);
        // A trail's ground stays whole under it.
        sample.crag.amplitude = 0.0;
        sample
    }

    /// What is built beside trails within the blocks from `min` to `max`.
    pub fn markers(&self, land: &dyn Land, min: IVec2, max: IVec2) -> Vec<(IVec2, Marker)> {
        let (first, last) = (min.div_euclid(IVec2::splat(TILE)), max.div_euclid(IVec2::splat(TILE)));
        let mut markers = Vec::new();
        for tz in first.y..=last.y {
            for tx in first.x..=last.x {
                let tile = self.tile(land, IVec2::new(tx, tz));
                markers.extend(tile.markers.iter().filter(|(at, _)| at.cmpge(min).all() && at.cmple(max).all()));
            }
        }
        markers
    }

    /// The trails through the blocks from `min` to `max`, as centrelines with each
    /// point's crossing, and the places they join, for maps and tools.
    pub fn sketch(&self, land: &dyn Land, min: IVec2, max: IVec2) -> (Vec<Vec<(Vec2, u8)>>, Vec<Poi>) {
        let (first, last) = (min.div_euclid(IVec2::splat(REGION)) - 1, max.div_euclid(IVec2::splat(REGION)) + 1);
        let mut lines = Vec::new();
        let mut places = Vec::new();
        for rz in first.y..=last.y {
            for rx in first.x..=last.x {
                let region = IVec2::new(rx, rz);
                if !self.laid_in(land, region) {
                    continue;
                }
                for path in self.paths(land, region).0.iter() {
                    let kind = |c: &path::Centre| match c.crossing {
                        path::Crossing::Ground => 0,
                        path::Crossing::Ford => 1,
                        path::Crossing::Bridge { .. } => 2,
                    };
                    lines.push(path.centres.iter().map(|c| (c.at, kind(c))).collect());
                }
                places.extend(self.places(land, region).iter().copied());
            }
        }
        (lines, places)
    }

    fn grid(&self, land: &dyn Land, patch: IVec2) -> Arc<Grid> {
        self.grids.get(patch, || Arc::new(Grid::new(land, patch)))
    }

    /// The grid at any cell, keeping the patches it reads for as long as it lives.
    fn cells<'a>(&'a self, land: &'a dyn Land) -> impl Fn(IVec2) -> Cell + 'a {
        let memo: RefCell<FxHashMap<IVec2, Arc<Grid>>> = RefCell::default();
        move |c: IVec2| {
            let patch = c.div_euclid(IVec2::splat(PATCH));
            let local = c - patch * PATCH;
            let index = (local.y * PATCH + local.x) as usize;
            if let Some(grid) = memo.borrow().get(&patch) {
                return grid.0[index];
            }
            let grid = self.grid(land, patch);
            let cell = grid.0[index];
            memo.borrow_mut().insert(patch, grid);
            cell
        }
    }

    /// Where settlements' lanes lead out of `region`, as places trails join.
    fn gates(&self, land: &dyn Land, region: IVec2) -> Arc<[Poi]> {
        self.gates.get(region, || {
            let cells = self.cells(land);
            let (min, max) = (region * REGION, region * REGION + REGION - 1);
            land.gates(min, max)
                .into_iter()
                .filter_map(|gate| {
                    let cell = gate.div_euclid(IVec2::splat(CELL));
                    let here = cells(cell);
                    (!here.submerged()).then_some(Poi { cell, height: here.height as i32, kind: Kind::Settlement })
                })
                .collect()
        })
    }

    /// The places of `region`: its places of interest and its settlements' gates.
    fn places(&self, land: &dyn Land, region: IVec2) -> Arc<[Poi]> {
        self.places.get(region, || {
            let mut places = sites::places(self.seed, region, &self.cells(land));
            places.extend(self.gates(land, region).iter().copied());
            places.into()
        })
    }

    /// The places within `radius` blocks of `centre`, and perhaps a few more.
    fn places_near(&self, land: &dyn Land, centre: Vec2, radius: f32) -> Vec<Poi> {
        near(centre, radius, |region| self.places(land, region))
    }

    /// The settlements' gates within `radius` blocks of `centre`, and perhaps a few more.
    fn gates_near(&self, land: &dyn Land, centre: Vec2, radius: f32) -> Vec<Poi> {
        near(centre, radius, |region| self.gates(land, region))
    }

    /// The trails a region owns, routed but not yet merged: where trails are laid
    /// everywhere, those whose lower end lies in it; near settlements, those leaving its
    /// settlements' gates (between two gates, the lower one's), so a region without one
    /// owns none and reads no land. Either way each trail runs from its lower end to its
    /// upper and is owned once.
    fn routes(&self, land: &dyn Land, region: IVec2) -> Arc<Routes> {
        self.routes.get(region, || {
            let everywhere = self.laid == TrailsWhere::Everywhere;
            let near = |centre: Vec2, radius: f32| self.places_near(land, centre, radius);
            let gates_near = |centre: Vec2, radius: f32| self.gates_near(land, centre, radius);
            let joins = |a: &Poi, b: &Poi| self.joins(a, b);
            let cells = self.cells(land);
            let mut routes = Vec::new();
            for from in self.starts(land, region).iter() {
                let mut ends = self.places_near(land, from.at().as_vec2(), network::MAX_EDGE as f32);
                ends.sort_by_key(|p| (p.cell.x, p.cell.y));
                for to in &ends {
                    let (lower, upper) = if from.below(to) {
                        (from, to)
                    } else if !everywhere && to.kind != Kind::Settlement {
                        (to, from)
                    } else {
                        continue;
                    };
                    // Near settlements a place that is no settlement joins only
                    // settlements, so from one only their gates can stand in the way.
                    let near: &dyn Fn(Vec2, f32) -> Vec<Poi> = if everywhere || lower.kind == Kind::Settlement { &near } else { &gates_near };
                    if !(self.joins(lower, upper) && network::keep(lower, upper, near, &joins)) {
                        continue;
                    }
                    let margin = (lower.cell - upper.cell).abs().max_element() / 3;
                    let margin = margin.clamp(6, 16);
                    let (min, max) = (lower.cell.min(upper.cell) - margin, lower.cell.max(upper.cell) + margin);
                    let budget = ((max - min + 1).x * (max - min + 1).y) as usize;
                    let Some(cells) = route::route(lower.cell, upper.cell, min, max, budget, &cells) else { continue };
                    if !bridges_short(&cells, &self.cells(land)) {
                        continue;
                    }
                    let length = cells.windows(2).map(|w| (w[1] - w[0]).as_vec2().length() * CELL as f32).sum();
                    // Ties in length are broken by the ends, the same for whoever compares.
                    let id = rng::mix(rng::hash3(self.seed, lower.cell.x, 0, lower.cell.y) ^ rng::hash3(self.seed, upper.cell.x, 1, upper.cell.y));
                    routes.push(Route { cells, length, id });
                }
            }
            Arc::new(Routes(routes))
        })
    }

    /// The places a region's trails start from (see `routes`): all its places where
    /// trails are laid everywhere, its settlements' gates near settlements.
    fn starts(&self, land: &dyn Land, region: IVec2) -> Arc<[Poi]> {
        match self.laid {
            TrailsWhere::Everywhere => self.places(land, region),
            TrailsWhere::NearSettlements => self.gates(land, region),
        }
    }

    /// The trails a region owns, merged into longer ones nearby and drawn.
    fn paths(&self, land: &dyn Land, region: IVec2) -> Arc<Paths> {
        self.paths.get(region, || {
            let own = self.routes(land, region);
            if own.0.is_empty() {
                return Arc::new(Paths(Vec::new()));
            }
            // Trunks owned by the regions around: a trail owned further off rarely runs
            // beside this region's, and looking further would cost far more planning.
            let around: Vec<Arc<Routes>> = regions_around(region, 1).map(|r| self.routes(land, r)).collect();
            let mut paths = Vec::new();
            for route in own.0.iter() {
                let line = network::points(&route.cells);
                let trunks: Vec<Vec<Vec2>> =
                    around.iter().flat_map(|r| r.0.iter()).filter(|other| other.above(route)).map(|other| network::points(&other.cells)).collect();
                let trunks: Vec<&[Vec2]> = trunks.iter().map(|t| t.as_slice()).collect();
                for piece in network::snap(&line, &trunks) {
                    paths.push(path::path(&piece, land));
                }
            }
            Arc::new(Paths(paths))
        })
    }

    /// Makes on every core what the trails of the regions around `region` are planned
    /// from and not made yet. A tile in `region` reads the paths of the regions next door,
    /// which read the routes of those two around as trunks; so in turn: which of those
    /// own trails (near settlements, those with a gate: asking plans their settlements),
    /// the places their trails may end at (within a trail's length of where they start,
    /// each reading its patches of the grid), their routes, and the paths next door.
    /// Anything further a route reads is made when asked for, so that nothing is made
    /// that is not needed.
    fn prepare(&self, land: &dyn Land, region: IVec2) {
        let regions: Vec<IVec2> = regions_around(region, 2).collect();
        if self.laid == TrailsWhere::NearSettlements {
            crate::parallel::for_each(&regions, |&r| {
                self.gates(land, r);
            });
        }
        let owners: Vec<IVec2> = regions.into_iter().filter(|&r| self.laid == TrailsWhere::Everywhere || !self.gates(land, r).is_empty()).collect();
        let mut reached: Vec<IVec2> = match self.laid {
            TrailsWhere::Everywhere => regions_around(region, 3).collect(),
            TrailsWhere::NearSettlements => owners
                .iter()
                .flat_map(|&r| self.gates(land, r).iter().flat_map(|g| regions_within(g.at().as_vec2(), network::MAX_EDGE as f32)).collect::<Vec<_>>())
                .collect(),
        };
        reached.sort_by_key(|r| (r.y, r.x));
        reached.dedup();
        let missing = |list: &[IVec2], has: &dyn Fn(IVec2) -> bool| list.iter().copied().filter(|&r| !has(r)).collect::<Vec<IVec2>>();
        crate::parallel::for_each(&missing(&reached, &|r| self.places.has(r)), |&r| {
            self.places(land, r);
        });
        crate::parallel::for_each(&missing(&owners, &|r| self.routes.has(r)), |&r| {
            self.routes(land, r);
        });
        let next: Vec<IVec2> = owners.iter().copied().filter(|r| (*r - region).abs().max_element() <= 1).collect();
        crate::parallel::for_each(&missing(&next, &|r| self.paths.has(r)), |&r| {
            self.paths(land, r);
        });
    }

    /// How many patches of the grid, and regions' routes, are made, for tests of how much
    /// a piece of generation needs.
    #[cfg(test)]
    pub(crate) fn made(&self) -> (usize, usize) {
        (self.grids.len(), self.routes.len())
    }

    /// Whether trails are laid in the region `region`: everywhere, or within reach of a
    /// settlement.
    fn laid_in(&self, land: &dyn Land, region: IVec2) -> bool {
        match self.laid {
            TrailsWhere::Everywhere => true,
            TrailsWhere::NearSettlements => land.settled(region * REGION, region * REGION + REGION - 1, SETTLED_REACH),
        }
    }

    /// Whether a trail may join `a` and `b`: any two places where trails are laid
    /// everywhere; near settlements, only a settlement to a place worth walking to or to
    /// another settlement, never two lanes of one settlement, so trails lead out of
    /// settlements rather than webbing the open land.
    fn joins(&self, a: &Poi, b: &Poi) -> bool {
        match self.laid {
            TrailsWhere::Everywhere => true,
            TrailsWhere::NearSettlements => {
                let (sa, sb) = (a.kind == Kind::Settlement, b.kind == Kind::Settlement);
                (sa || sb) && !(sa && sb && (a.cell - b.cell).as_vec2().length() * (CELL as f32) < 240.0)
            }
        }
    }

    fn tile(&self, land: &dyn Land, key: IVec2) -> Arc<Tile> {
        let found = RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            let i = recent.iter().position(|(id, k, _)| *id == self.id && *k == key)?;
            let entry = recent.remove(i);
            let tile = entry.2.clone();
            recent.insert(0, entry);
            Some(tile)
        });
        if let Some(tile) = found {
            return tile;
        }
        let tile = self.tiles.get(key, || {
            let _stage = stage::enter(Stage::TrailPlans);
            let region = (key * TILE).div_euclid(IVec2::splat(REGION));
            if !self.laid_in(land, region) {
                return Arc::new(Tile { stamps: None, markers: Vec::new() });
            }
            self.prepare(land, region);
            let owners: Vec<Arc<Paths>> = regions_around(region, 1).map(|r| self.paths(land, r)).collect();
            let paths: Vec<&Path> = owners.iter().flat_map(|p| p.0.iter()).collect();
            let (min, max) = (key * TILE, key * TILE + TILE - 1);
            let inside = |at: IVec2| at.cmpge(min).all() && at.cmple(max).all();
            // The places in reach of the tile: their clearings, cairns and waymarkers.
            let places = self.places_near(land, (min + TILE / 2).as_vec2(), TILE as f32);
            let clearings: Vec<IVec2> = places.iter().filter(|p| matches!(p.kind, Kind::Summit | Kind::Viewpoint)).map(|p| p.at()).collect();
            let mut markers: Vec<(IVec2, Marker)> = places
                .iter()
                .filter_map(|p| match p.kind {
                    Kind::Summit => Some((p.at(), Marker::Summit)),
                    Kind::Trailhead => Some((p.at() + IVec2::new(2, 1), Marker::Waymarker)),
                    _ => None,
                })
                .filter(|&(at, _)| inside(at))
                .collect();
            for path in &paths {
                markers.extend(path::cairns(path).into_iter().filter(|&at| inside(at)).map(|at| (at, Marker::Cairn)));
            }
            Arc::new(Tile { stamps: path::stamps(min, TILE, &paths, &clearings), markers })
        });
        RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            recent.insert(0, (self.id, key, tile.clone()));
            recent.truncate(8);
        });
        tile
    }
}

/// Of the places `of` gives by region, those within `radius` blocks of `centre`.
fn near(centre: Vec2, radius: f32, of: impl Fn(IVec2) -> Arc<[Poi]>) -> Vec<Poi> {
    regions_within(centre, radius).flat_map(|region| of(region).iter().filter(|p| p.at().as_vec2().distance(centre) <= radius).copied().collect::<Vec<_>>()).collect()
}

/// The regions within `reach` regions of `region`, it among them.
fn regions_around(region: IVec2, reach: i32) -> impl Iterator<Item = IVec2> {
    (-reach..=reach).flat_map(move |dz| (-reach..=reach).map(move |dx| region + IVec2::new(dx, dz)))
}

/// The regions the square `radius` blocks round `centre` touches.
fn regions_within(centre: Vec2, radius: f32) -> impl Iterator<Item = IVec2> {
    let first = ((centre - radius) / REGION as f32).floor().as_ivec2();
    let last = ((centre + radius) / REGION as f32).floor().as_ivec2();
    (first.y..=last.y).flat_map(move |rz| (first.x..=last.x).map(move |rx| IVec2::new(rx, rz)))
}

/// Whether every stretch of bridged water on a route is short enough to bridge (100
/// blocks); longer crossings are left unbuilt, and the trail with them.
fn bridges_short(cells: &[IVec2], cell: &dyn Fn(IVec2) -> Cell) -> bool {
    let mut span = 0.0;
    for w in cells.windows(2) {
        if cell(w[1]).depth() > route::FORD_DEPTH {
            span += (w[1] - w[0]).as_vec2().length() * CELL as f32;
            if span > 100.0 {
                return false;
            }
        } else {
            span = 0.0;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Overworld;

    /// A world with trails laid everywhere, as they will be near settlements.
    fn world(seed: u64) -> Overworld {
        let registry = std::sync::Arc::new(crate::test_registry());
        let mut content = crate::content::WorldContent::base(&registry).unwrap();
        content.trails.laid = TrailsWhere::Everywhere;
        Overworld::with_content(seed, registry, &content)
    }

    /// The ground along a trail is the same whichever region, tile or column is asked for
    /// first, and on whichever thread.
    #[test]
    fn trails_never_depend_on_order() {
        let around = |near: IVec2| (0..48 * 48).map(|i| (near.x - 24 + i % 48, near.y - 24 + i / 48)).collect::<Vec<(i32, i32)>>();
        // Each area is sampled first in a world of its own, cold.
        let sampled = |area: &[(i32, i32)]| {
            let a = world(7);
            area.iter().map(|&(x, z)| a.geo(x, z)).collect::<Vec<GeoSample>>()
        };
        let trailed = |samples: &[GeoSample]| samples.iter().any(|g| g.trail.is_some_and(|t| t.tread > 0.9));
        // Around a point on the trail nearest the origin, as a third world found it: kept
        // here, since planning the regions round the origin to find it took half the test,
        // and found afresh in a third world should the trails have moved off it.
        let mut area = around(IVec2::new(-262, -6));
        let mut forward = sampled(&area);
        if !trailed(&forward) {
            let (lines, _) = world(7).trails_in(IVec2::splat(-400), IVec2::splat(400));
            let near = lines.iter().flatten().map(|p| p.0).min_by(|p, q| p.length().total_cmp(&q.length())).expect("a trail near the origin");
            area = around(near.as_ivec2());
            forward = sampled(&area);
        }
        let b = world(7);
        // The other world plans from the far side first, on several threads.
        let _ = std::thread::scope(|s| {
            let far = s.spawn(|| b.geo(700, -500));
            let near = s.spawn(|| b.geo(-300, 400));
            (far.join().unwrap(), near.join().unwrap())
        });
        let backward: Vec<GeoSample> = area.iter().rev().map(|&(x, z)| b.geo(x, z)).collect();
        assert!(trailed(&forward), "a trail runs through the area");
        assert!(forward.iter().eq(backward.iter().rev()));
    }

    /// Near settlements, where each trail is planned from a settlement's gate, the ground
    /// along a trail is the same whichever region, tile or column is asked for first: here
    /// round a gate two trails leave, where their stamps meet with equal weights.
    #[test]
    fn trails_near_settlements_never_depend_on_order() {
        use crate::Generator;
        let registry = std::sync::Arc::new(crate::test_registry());
        let world = || Overworld::new(1, registry.clone());
        // The gates two trails leave (their ends within a block of each other, by a
        // settlement's gate), of the trails in a world's box from `min` to `max`.
        let shared = |world: &Overworld, min: IVec2, max: IVec2| {
            let (lines, places) = world.trails_in(min, max);
            let ends: Vec<Vec2> = lines.iter().filter(|l| l.len() > 1).flat_map(|l| [l[0].0, l[l.len() - 1].0]).collect();
            let gate = |e: Vec2| places.iter().any(|p| p.kind == Kind::Settlement && p.at().as_vec2().distance(e) <= CELL as f32);
            ends.iter().enumerate().filter(|&(i, &e)| gate(e) && ends.iter().enumerate().any(|(j, &f)| j != i && e.distance(f) <= 1.0)).map(|(_, &e)| e).collect::<Vec<Vec2>>()
        };
        let around = |gate: Vec2| {
            let corner = gate.as_ivec2() - 24;
            (0..48 * 48).map(|i| (corner.x + i % 48, corner.y + i / 48)).collect::<Vec<(i32, i32)>>()
        };
        // Round the gate nearest spawn that two trails leave, as a third world found it: kept
        // here, since planning the trails 1200 blocks round spawn to find it took half the
        // test. Once sampled, the first world's trails round it must still leave it two
        // ways; should they not, a third world finds the gate afresh, and a cold world
        // samples round it.
        let mut gate = Vec2::new(-576.0, -1864.0);
        let a = world();
        let mut forward: Vec<GeoSample> = around(gate).iter().map(|&(x, z)| a.geo(x, z)).collect();
        if !shared(&a, gate.as_ivec2(), gate.as_ivec2()).contains(&gate) {
            let third = world();
            let spawn = third.spawn_point();
            let centre = IVec2::new(spawn.x, spawn.z);
            // Culture layouts move the gates; include the settlements just beyond spawn's
            // first square while still requiring a real shared gate, not merely any trail.
            gate = shared(&third, centre - 1200, centre + 1200)
                .into_iter()
                .min_by(|p, q| p.distance(centre.as_vec2()).total_cmp(&q.distance(centre.as_vec2())))
                .expect("a gate near spawn that two trails leave");
            let a = world();
            forward = around(gate).iter().map(|&(x, z)| a.geo(x, z)).collect();
        }
        let area = around(gate);
        let corner = gate.as_ivec2() - 24;
        let b = world();
        // The other world plans the far side of the trail's settlement first, on several threads.
        let _ = std::thread::scope(|s| {
            let far = s.spawn(|| b.geo(corner.x + 700, corner.y - 500));
            let near = s.spawn(|| b.geo(corner.x - 300, corner.y + 400));
            (far.join().unwrap(), near.join().unwrap())
        });
        let backward: Vec<GeoSample> = area.iter().rev().map(|&(x, z)| b.geo(x, z)).collect();
        assert!(forward.iter().any(|g| g.trail.is_some_and(|t| t.tread > 0.9)), "a trail runs through the area");
        assert!(forward.iter().eq(backward.iter().rev()));
    }

    /// The planning grid is the land sampled every `CELL` blocks, whichever patch it is
    /// sampled in: a patch's cells match the land sampled one by one.
    #[test]
    fn grid_patches_sample_the_land() {
        let world = world(7);
        let land = world.land();
        for patch in [IVec2::new(0, 0), IVec2::new(-3, 5), IVec2::new(7, -2)] {
            let grid = Grid::new(&land, patch);
            for i in 0..PATCH * PATCH {
                let c = patch * PATCH + IVec2::new(i % PATCH, i / PATCH);
                let g = land.sample(c.x * CELL, c.y * CELL);
                let water = if g.submerged() { g.water } else { g.height };
                let cell = grid.0[i as usize];
                assert_eq!((cell.height as i32, cell.water as i32, cell.body), (g.height, water, g.body as u8), "cell {c} of patch {patch}");
            }
        }
    }

    /// `cargo test --profile iter -p voxmar_worldgen trail_stats -- --ignored --nocapture`
    /// counts, over a few regions of a world (`SEED`), the places, the trails the network
    /// wants, and why any were not built.
    #[test]
    #[ignore]
    fn trail_stats() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
        let world = world(seed);
        let land = world.land();
        let trails = &world.trails;
        // The grid of 8×8 regions, patch by patch.
        let patches = REGION / CELL / PATCH;
        let started = std::time::Instant::now();
        for pz in -4 * patches..4 * patches {
            for px in -4 * patches..4 * patches {
                trails.grid(&land, IVec2::new(px, pz));
            }
        }
        println!("64 regions' grids in {:.1?}", started.elapsed());
        let again = Trails::new(1, TrailsWhere::Everywhere);
        let started = std::time::Instant::now();
        for pz in -4 * patches..4 * patches {
            for px in -4 * patches..4 * patches {
                again.grid(&land, IVec2::new(px, pz));
            }
        }
        println!("64 regions' grids again, rivers known, in {:.1?}", started.elapsed());
        let started = std::time::Instant::now();
        for rz in -3..3 {
            for rx in -3..3 {
                trails.places(&land, IVec2::new(rx, rz));
            }
        }
        println!("36 regions' places in {:.1?}", started.elapsed());
        let (mut places, mut candidates, mut kept, mut routed, mut no_route, mut long_bridge) = (0, 0, 0, 0, 0, 0);
        let started = std::time::Instant::now();
        for rz in -2..2 {
            for rx in -2..2 {
                let region = IVec2::new(rx, rz);
                let near = |centre: Vec2, radius: f32| trails.places_near(&land, centre, radius);
                let cells = trails.cells(&land);
                for from in trails.places(&land, region).iter() {
                    places += 1;
                    for to in trails.places_near(&land, from.at().as_vec2(), network::MAX_EDGE as f32) {
                        if !from.below(&to) {
                            continue;
                        }
                        candidates += 1;
                        if !network::keep(from, &to, &near, &|a, b| trails.joins(a, b)) {
                            continue;
                        }
                        kept += 1;
                        let margin = ((from.cell - to.cell).abs().max_element() / 3).clamp(6, 16);
                        let (min, max) = (from.cell.min(to.cell) - margin, from.cell.max(to.cell) + margin);
                        match route::route(from.cell, to.cell, min, max, ((max - min + 1).x * (max - min + 1).y) as usize, &cells) {
                            None => {
                                no_route += 1;
                                println!("no route {:?} {} -> {:?} {}", from.kind, from.at(), to.kind, to.at());
                            }
                            Some(path) if !bridges_short(&path, &cells) => long_bridge += 1,
                            Some(_) => routed += 1,
                        }
                    }
                }
            }
        }
        println!("{places} places, {candidates} candidate pairs, {kept} kept, {routed} routed, {no_route} without a route, {long_bridge} with too long a bridge, in {:.1?}", started.elapsed());
    }
}

#[cfg(test)]
mod sights {
    use voxmar_engine::glam::{IVec2, Vec2};

    use crate::{Generator, Overworld};

    /// `cargo test --profile iter -p voxmar_worldgen trail_sights -- --ignored --nocapture`
    /// lists trail sights near spawn in a world (`SEED`, default 7): fords, bridges and
    /// switchbacks, each with a `--pose` for an in-game capture looking at it.
    #[test]
    #[ignore]
    fn trail_sights() {
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
        let registry = std::sync::Arc::new(crate::test_registry());
        let world = Overworld::new(seed, registry);
        let spawn = world.spawn_point();
        let eye = spawn.as_vec3() + voxmar_engine::glam::Vec3::Y * 1.62;
        let reach = 1500;
        let centre = IVec2::new(spawn.x, spawn.z);
        let (lines, places) = world.trails_in(centre - reach, centre + reach);
        println!("spawn {spawn}: {} trails, {} places", lines.len(), places.len());
        // A camera 14 blocks back and 9 up from a point, looking at it.
        let pose = |at: Vec2, from: Vec2| {
            let back = (from - at).normalize_or(Vec2::X);
            let ground = world.geo(at.x as i32, at.y as i32).height as f32;
            let camera = at + back * 14.0;
            let height = ground + 9.0;
            let yaw = (-(at - camera).x).atan2(-(at - camera).y).to_degrees();
            let pitch = -((height - ground) / 14.0).atan().to_degrees();
            format!("{:.0},{:.0},{:.0},{yaw:.0},{pitch:.0}", camera.x - eye.x, height - eye.y, camera.y - eye.z)
        };
        let mut shown = [0; 3];
        let mut lines: Vec<_> = lines.iter().filter(|l| l.len() > 4).collect();
        lines.sort_by_key(|l| ((l[0].0 - centre.as_vec2()).length() as i32, l[0].0.x as i32));
        for line in lines {
            for (kind, name) in [(1u8, "ford"), (2, "bridge")] {
                if let Some(i) = line.iter().position(|p| p.1 == kind)
                    && shown[kind as usize] < 3
                {
                    shown[kind as usize] += 1;
                    let at = line[i].0;
                    let side = line[i.saturating_sub(3)].0 - line[(i + 3).min(line.len() - 1)].0;
                    let from = at + Vec2::new(-side.y, side.x);
                    println!("{name} at {at:.0}, {:.0} from spawn: --pose {}", (at - centre.as_vec2()).length(), pose(at, from));
                }
            }
            // A switchback: the trail turning back on itself while climbing.
            if shown[0] < 3 {
                for i in (12..line.len() - 12).step_by(4) {
                    let (a, b, c) = (line[i - 12].0, line[i].0, line[i + 12].0);
                    let turn = (b - a).normalize_or_zero().dot((c - b).normalize_or_zero());
                    let climb = (world.geo(c.x as i32, c.y as i32).height - world.geo(a.x as i32, a.y as i32).height).abs();
                    if turn < -0.5 && climb >= 6 {
                        shown[0] += 1;
                        let from = b + (b - (a + c) / 2.0).normalize_or(Vec2::X) * 10.0;
                        println!("switchback at {b:.0} climbing {climb}, {:.0} from spawn: --pose {}", (b - centre.as_vec2()).length(), pose(b, from));
                        break;
                    }
                }
            }
        }
        for place in places.iter().filter(|p| (p.at() - centre).as_vec2().length() < 600.0) {
            println!("{:?} at {} height {}", place.kind, place.at(), place.height);
        }
    }
}

#[cfg(test)]
mod patch {
    use voxmar_engine::block::RenderKind;
    use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
    use voxmar_engine::glam::IVec3;

    use crate::{Generator, Overworld};

    /// `X=x Z=z cargo test --profile iter -p voxmar_worldgen trail_patch -- --ignored`
    /// generates the 160×160 blocks around (x, z) in a world (`SEED`, default 7) and
    /// draws them from above, four pixels a block, to `target/trail_patch.ppm`: each
    /// column's top block in its placeholder colour, shaded by height.
    #[test]
    #[ignore]
    fn trail_patch() {
        let env = |name: &str, default: i32| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
        let seed = env("SEED", 7) as u64;
        let registry = std::sync::Arc::new(crate::test_registry());
        let world = Overworld::new(seed, registry.clone());
        const SIZE: i32 = 160;
        const PIXEL: i32 = 4;
        let min = IVec3::new(env("X", 0) - SIZE / 2, 0, env("Z", 0) - SIZE / 2);
        let first = ColumnPos::of_block(min);
        let last = ColumnPos::of_block(min + IVec3::new(SIZE - 1, 0, SIZE - 1));
        let mut columns = Vec::new();
        for cz in first.0.y..=last.0.y {
            for cx in first.0.x..=last.0.x {
                let pos = ColumnPos(voxmar_engine::glam::IVec2::new(cx, cz));
                columns.push((pos, world.generate(pos)));
            }
        }
        let top = |x: i32, z: i32| -> (i32, [f32; 3]) {
            let pos = ColumnPos::of_block(IVec3::new(x, 0, z));
            let column = &columns.iter().find(|(p, _)| *p == pos).expect("generated").1;
            let local = |y: i32| IVec3::new(x.rem_euclid(32), y, z.rem_euclid(32));
            for y in (0..WORLD_HEIGHT).rev() {
                let block = column.block(local(y));
                let info = registry.info(block);
                if block.is_air() || matches!(info.render, RenderKind::Plant | RenderKind::Invisible) {
                    continue;
                }
                let color = registry.def(block).color.map(|c| c as f32 / 255.0);
                return (y, color);
            }
            (0, [0.0; 3])
        };
        let mut pixels = vec![0u8; (SIZE * PIXEL * SIZE * PIXEL * 3) as usize];
        for z in 0..SIZE {
            for x in 0..SIZE {
                let (h, color) = top(min.x + x, min.z + z);
                let (west, _) = if x > 0 { top(min.x + x - 1, min.z + z) } else { (h, color) };
                let (north, _) = if z > 0 { top(min.x + x, min.z + z - 1) } else { (h, color) };
                let shade = 1.0 + ((h - west) + (h - north)) as f32 * 0.08;
                let rgb = color.map(|c| ((c * shade).clamp(0.0, 1.0) * 255.0) as u8);
                for dz in 0..PIXEL {
                    for dx in 0..PIXEL {
                        let i = (((z * PIXEL + dz) * SIZE * PIXEL + x * PIXEL + dx) * 3) as usize;
                        pixels[i..i + 3].copy_from_slice(&rgb);
                    }
                }
            }
        }
        // Trail centrelines, one red pixel each, with `LINES=1`.
        if std::env::var("LINES").is_ok_and(|v| v == "1") {
            let (lines, _) = world.trails_in(voxmar_engine::glam::IVec2::new(min.x, min.z), voxmar_engine::glam::IVec2::new(min.x + SIZE, min.z + SIZE));
            for &(at, _) in lines.iter().flatten() {
                let (px, pz) = (((at.x - min.x as f32) * PIXEL as f32) as i32, ((at.y - min.z as f32) * PIXEL as f32) as i32);
                if (0..SIZE * PIXEL).contains(&px) && (0..SIZE * PIXEL).contains(&pz) {
                    let i = ((pz * SIZE * PIXEL + px) * 3) as usize;
                    pixels[i..i + 3].copy_from_slice(&[255, 0, 0]);
                }
            }
        }
        let mut file = format!("P6 {} {} 255\n", SIZE * PIXEL, SIZE * PIXEL).into_bytes();
        file.extend(pixels);
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/trail_patch.ppm"), file).unwrap();
    }
}
