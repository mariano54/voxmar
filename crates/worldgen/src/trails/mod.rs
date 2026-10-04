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
//! Everything is a pure function of the seed and the land, worked out by region on
//! demand and kept in caches bounded by bytes: places by region, the trails each region
//! owns (those whose lower end lies in it), and the stamps over 64-block tiles that
//! columns read. A trail reaches at most one region beyond its owner, so a tile reads
//! the trails of the 3×3 regions around it. Nothing depends on which column, region or
//! thread came first.

mod network;
mod path;
mod route;
mod sites;

use std::cell::RefCell;
use std::sync::Arc;

use voxmar_engine::glam::{IVec2, Vec2};

pub use self::sites::{Kind, Poi};
use self::path::{Path, Paths, Stamp};
use self::sites::{CELL, CELLS, Cell, Grid, REGION};
use crate::geology::GeoSample;
use crate::rng;
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
}

/// Blocks along a side of a tile of stamps.
const TILE: i32 = 64;
const MB: usize = 1 << 20;

/// The trails of a world.
pub struct Trails {
    seed: u64,
    grids: Cache<IVec2, Arc<Grid>>,
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
    pub fn new(seed: u64) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            seed: rng::mix(seed ^ 0x7a11_5eed),
            grids: Cache::new(48 * MB),
            places: Cache::new(MB),
            routes: Cache::new(8 * MB),
            paths: Cache::new(16 * MB),
            tiles: Cache::new(48 * MB),
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
            if mark.stone && sample.water - sample.height <= route::FORD_DEPTH {
                sample.height = sample.water;
            }
        } else if sample.cavity.is_none() && stamp.weight > 0.0 {
            let blended = sample.height as f32 + (stamp.target - sample.height as f32) * stamp.weight;
            // Never below the water beside it, which would let it spill.
            sample.height = (blended.round() as i32).max(sample.water);
        }
        sample.trail = Some(mark);
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

    fn grid(&self, land: &dyn Land, region: IVec2) -> Arc<Grid> {
        self.grids.get(region, || Arc::new(Grid::new(land, region)))
    }

    /// The grid at any cell, reading regions through a small memo.
    fn cells<'a>(&'a self, land: &'a dyn Land) -> impl Fn(IVec2) -> Cell + 'a {
        let memo: RefCell<Vec<(IVec2, Arc<Grid>)>> = RefCell::new(Vec::new());
        move |c: IVec2| {
            let region = c.div_euclid(IVec2::splat(CELLS));
            let local = c - region * CELLS;
            let index = (local.y * CELLS + local.x) as usize;
            if let Some((_, grid)) = memo.borrow().iter().find(|(r, _)| *r == region) {
                return grid.0[index];
            }
            let grid = self.grid(land, region);
            let cell = grid.0[index];
            let mut memo = memo.borrow_mut();
            memo.push((region, grid));
            if memo.len() > 16 {
                memo.remove(0);
            }
            cell
        }
    }

    fn places(&self, land: &dyn Land, region: IVec2) -> Arc<[Poi]> {
        self.places.get(region, || sites::places(self.seed, region, &self.cells(land)).into())
    }

    /// The places within `radius` blocks of `centre`, and perhaps a few more.
    fn places_near(&self, land: &dyn Land, centre: Vec2, radius: f32) -> Vec<Poi> {
        let first = ((centre - radius) / REGION as f32).floor().as_ivec2();
        let last = ((centre + radius) / REGION as f32).floor().as_ivec2();
        let mut near = Vec::new();
        for rz in first.y..=last.y {
            for rx in first.x..=last.x {
                near.extend(self.places(land, IVec2::new(rx, rz)).iter().filter(|p| p.at().as_vec2().distance(centre) <= radius));
            }
        }
        near
    }

    /// The trails a region owns, routed but not yet merged.
    fn routes(&self, land: &dyn Land, region: IVec2) -> Arc<Routes> {
        self.routes.get(region, || {
            let near = |centre: Vec2, radius: f32| self.places_near(land, centre, radius);
            let cells = self.cells(land);
            let mut routes = Vec::new();
            for from in self.places(land, region).iter() {
                let mut ends = self.places_near(land, from.at().as_vec2(), network::MAX_EDGE as f32);
                ends.sort_by_key(|p| (p.cell.x, p.cell.y));
                for to in ends.iter().filter(|to| from.below(to) && network::keep(from, to, &near)) {
                    let margin = (from.cell - to.cell).abs().max_element() / 3;
                    let margin = margin.clamp(6, 16);
                    let (min, max) = (from.cell.min(to.cell) - margin, from.cell.max(to.cell) + margin);
                    let budget = ((max - min + 1).x * (max - min + 1).y) as usize;
                    let Some(cells) = route::route(from.cell, to.cell, min, max, budget, &cells) else { continue };
                    if !bridges_short(&cells, &self.cells(land)) {
                        continue;
                    }
                    let length = cells.windows(2).map(|w| (w[1] - w[0]).as_vec2().length() * CELL as f32).sum();
                    // Ties in length are broken by the ends, the same for whoever compares.
                    let id = rng::mix(rng::hash3(self.seed, from.cell.x, 0, from.cell.y) ^ rng::hash3(self.seed, to.cell.x, 1, to.cell.y));
                    routes.push(Route { cells, length, id });
                }
            }
            Arc::new(Routes(routes))
        })
    }

    /// The trails a region owns, merged into longer ones nearby and drawn.
    fn paths(&self, land: &dyn Land, region: IVec2) -> Arc<Paths> {
        self.paths.get(region, || {
            // Trunks owned by the regions around: a trail owned further off rarely runs
            // beside this region's, and looking further would cost far more planning.
            let around: Vec<Arc<Routes>> =
                (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| IVec2::new(dx, dz))).map(|d| self.routes(land, region + d)).collect();
            let mut paths = Vec::new();
            for route in self.routes(land, region).0.iter() {
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
            let region = (key * TILE).div_euclid(IVec2::splat(REGION));
            let owners: Vec<Arc<Paths>> =
                (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| IVec2::new(dx, dz))).map(|d| self.paths(land, region + d)).collect();
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

    fn world(seed: u64) -> Overworld {
        let source = include_str!("../../../../assets/content/base/blocks.ron");
        Overworld::new(seed, std::sync::Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", source).unwrap()))
    }

    /// The ground along a trail is the same whichever region, tile or column is asked for
    /// first, and on whichever thread.
    #[test]
    fn trails_never_depend_on_order() {
        // Seed 7 has a trail fording a creek near (100, 15).
        let (a, b) = (world(7), world(7));
        let area: Vec<(i32, i32)> = (0..48 * 48).map(|i| (76 + i % 48, -9 + i / 48)).collect();
        let forward: Vec<GeoSample> = area.iter().map(|&(x, z)| a.geo(x, z)).collect();
        // The other world plans from the far side first, on several threads.
        let _ = std::thread::scope(|s| {
            let far = s.spawn(|| b.geo(700, -500));
            let near = s.spawn(|| b.geo(-300, 400));
            (far.join().unwrap(), near.join().unwrap())
        });
        let backward: Vec<GeoSample> = area.iter().rev().map(|&(x, z)| b.geo(x, z)).collect();
        assert!(forward.iter().any(|g| g.trail.is_some_and(|t| t.tread > 0.9)), "a trail runs through the area");
        assert!(forward.iter().eq(backward.iter().rev()));
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
        let started = std::time::Instant::now();
        for rz in -4..4 {
            for rx in -4..4 {
                trails.grid(&land, IVec2::new(rx, rz));
            }
        }
        println!("64 grids in {:.1?}", started.elapsed());
        let again = Trails::new(1);
        let started = std::time::Instant::now();
        for rz in -4..4 {
            for rx in -4..4 {
                again.grid(&land, IVec2::new(rx, rz));
            }
        }
        println!("64 grids again, rivers known, in {:.1?}", started.elapsed());
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
                        if !network::keep(from, &to, &near) {
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
        let source = include_str!("../../../../assets/content/base/blocks.ron");
        let registry = std::sync::Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", source).unwrap());
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
        let source = include_str!("../../../../assets/content/base/blocks.ron");
        let registry = std::sync::Arc::new(voxmar_engine::block::BlockRegistry::from_ron("base", source).unwrap());
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
