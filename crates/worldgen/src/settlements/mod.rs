//! Stage 3c: settlements, hamlets and villages grown on good ground.
//!
//! The world is cut into squares (`grid` in settlements.ron) and about a third of them
//! hold a settlement, placed well inside so it never reaches the next. Within its square
//! a settlement takes the best of a few likely spots: firm, dry, gentle ground, away from
//! the channels and banks of rivers and lakes but with fresh water near, never on crags or
//! terrain shapes or in the biomes the file avoids ([`plan::plan`]).
//!
//! Each then draws its character, five levers (wealth, age, order, crowding and green),
//! partly from its site and partly from luck, and every list it rolls from leans on them
//! ([`def::Lean`]): its size, how its lanes are laid, the kinds of house, their extras,
//! the props along its lanes, and the few materials it keeps of its biome's style. So a
//! rich, orderly village on a fertile river plain and an old, poor hamlet on a cold hill
//! come out as different as they should, and no two alike.
//!
//! The plan grades the ground: flat pads under buildings and the square, easing back
//! into the land over a few blocks, and trodden lanes, all marked clear so trees,
//! decorations, plants and ore samples keep off them through the same marks trails use
//! ([`Settlements::apply`]). Generation then builds what the plan drew into each column
//! ([`Settlements::build`]). Lanes that lead out end in gates, which trails join.
//!
//! Like trails, everything is a pure function of the seed and the land with its rivers,
//! planned by square on demand and cached, so nothing depends on which column, square or
//! thread came first.

mod canvas;
mod centre;
pub mod culture;
pub mod def;
mod house;
pub mod interior;
pub mod people;
mod plan;
pub mod roofs;
#[cfg(test)]
mod tests;
mod walls;

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::{IVec2, IVec3, Vec2};

pub use self::canvas::Facing;
use self::canvas::{Palette, Put, Voxel};
pub use self::def::{Levers, SettlementsDef};
use self::def::{FurnishingsDef, Resolved};
use crate::content::Select;
pub use self::house::Kind;
use self::house::Fixed;
use crate::Overworld;
use crate::geology::GeoSample;
use crate::terrain::Blocks;
use crate::trails::TrailMark;
use crate::water::cache::{Cache, Weigh};

const MB: usize = 1 << 20;

/// A building of a settlement, for gameplay: who lives or works where.
#[derive(Clone, Debug, PartialEq)]
pub struct Building {
    pub kind: Kind,
    /// The blocks it stands on, everything it puts down included.
    pub min: IVec2,
    pub max: IVec2,
    /// The height of its solid floor block; feet stand one block above it indoors.
    pub floor: i32,
    /// Ground-floor room columns in world (x, z), clear of walls and exterior extras.
    /// Empty for buildings whose activity is outdoors.
    pub room: Vec<IVec2>,
    /// The floor columns of its byre, behind the room, for the animals kept there.
    pub byre: Vec<IVec2>,
    /// Where its path leaves the door, two blocks out, at the height of a foot standing there.
    pub door: Option<IVec3>,
    pub facing: Facing,
    /// What it is in the lists: a house's name (`cottage`, `bakery`), a centrepiece's
    /// (`GreatTree`), or else the kind's.
    pub what: String,
    /// The work done in it (`bakery`, `mill`, `shrine`), for the people who work there.
    pub work: Option<String>,
    /// Where its beds and seats stand (the blocks themselves), for its people to sleep and
    /// sit: homes only, furnished by `interiors`.
    pub beds: Vec<IVec3>,
    pub seats: Vec<IVec3>,
}

impl Building {
    /// Whether it answers to `name`: its kind's, what it is in the lists (`what`), or the
    /// work done in it.
    pub fn answers(&self, name: &str) -> bool {
        self.kind.name() == name || self.what == name || self.work.as_deref() == Some(name)
    }
}

/// Whose a settlement is, as gameplay and tools see it (`cultures.ron`).
#[derive(Clone, Debug, PartialEq)]
pub struct CultureInfo {
    /// The family's name in the file, and its title in English and Japanese.
    pub family: String,
    pub title: (String, String),
    /// The second family mixed in (near a border, or the region's under a site claim) and
    /// its share of every weight.
    pub second: Option<(String, f32)>,
    /// Its quirks' names and titles (one, now and then two).
    pub quirks: Vec<(String, (String, String))>,
    /// Piety, martial, trade, craft, festivity, outlook, hierarchy and lore, each 0 ..= 1.
    pub axes: [f32; culture::AXES],
    /// The family entries it dropped and the ones it borrowed from another family, as
    /// `houses:hall` and `family:props:Lamp`.
    pub dropped: Vec<String>,
    pub borrowed: Vec<String>,
}

/// A planned settlement.
pub struct Plan {
    pub id: u64,
    /// Its size (`hamlet`, `village`), layout and style, by their names in the file.
    pub size: String,
    pub layout: String,
    pub style: String,
    /// Its name, in English and in Japanese.
    pub name: (String, String),
    /// The middle of its square or green, and the square's height.
    pub centre: IVec2,
    pub height: i32,
    pub levers: Levers,
    /// Whose it is: its family, quirk and axes (`cultures.ron`).
    pub culture: culture::Culture,
    pub buildings: Vec<Building>,
    /// Where lanes leading out end, which trails join.
    pub gates: Vec<IVec2>,
    /// Lanes' centre lines, for maps.
    pub lanes: Vec<Vec<Vec2>>,
    /// Who lives there (`people`).
    pub people: Vec<people::Resident>,
    /// Each lane's half width, beside `lanes`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub lane_halves: Vec<f32>,
    /// Its wall, if it is walled: each column and how many blocks over the ground it
    /// stands, and what it is built of.
    pub walls: Vec<(IVec2, i32)>,
    pub wall_block: Option<voxmar_engine::block::BlockId>,
    /// The blocks the settlement touches, grading included.
    pub min: IVec2,
    pub max: IVec2,
    raster: Raster,
    voxels: FxHashMap<IVec2, Vec<Voxel>>,
}

impl Plan {
    fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min.x && z >= self.min.y && x <= self.max.x && z <= self.max.y
    }
}

/// What the plan does to the ground, per block of its area.
#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    target: i16,
    /// 0..=255: how far the ground is pulled to `target`; `core` holds it exactly.
    weight: u8,
    core: bool,
    /// 0..=255: how much lane is trodden here.
    tread: u8,
    clear: bool,
}

impl Cell {
    /// The ground's height where it is graded, from its own `sample`.
    fn graded(&self, sample: &GeoSample) -> i32 {
        let target = self.target as i32;
        let height = if self.core {
            target
        } else {
            let w = self.weight as f32 / 255.0;
            (sample.height as f32 + (target - sample.height) as f32 * w).round() as i32
        };
        // Never below the water beside it, which would let it spill.
        height.max(sample.water)
    }
}

pub(crate) struct Raster {
    min: IVec2,
    max: IVec2,
    cells: Vec<Cell>,
    /// While planning: summed weight, summed weighted target and the largest weight.
    sums: Vec<(f32, f32, f32)>,
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Raster {
    fn new(min: IVec2, max: IVec2) -> Self {
        let size = max - min + 1;
        let n = (size.x * size.y) as usize;
        Self { min, max, cells: vec![Cell::default(); n], sums: vec![(0.0, 0.0, 0.0); n] }
    }

    fn index(&self, p: IVec2) -> Option<usize> {
        let size = self.max - self.min + 1;
        let l = p - self.min;
        (l.x >= 0 && l.y >= 0 && l.x < size.x && l.y < size.y).then(|| (l.y * size.x + l.x) as usize)
    }

    fn get(&self, x: i32, z: i32) -> Option<&Cell> {
        self.index(IVec2::new(x, z)).map(|i| &self.cells[i])
    }

    /// The height of the ground at (x, z) once graded, from its own `sample`, as `apply`
    /// gives it.
    fn ground(&self, x: i32, z: i32, sample: &GeoSample) -> i32 {
        match self.get(x, z) {
            Some(cell) if !sample.submerged() && sample.cavity.is_none() && cell.weight > 0 => cell.graded(sample),
            _ => sample.height,
        }
    }

    /// A pad at `height` over `core`, held exactly where `inside`, easing out over
    /// `falloff` blocks, its yard kept clear for `yard` blocks round it. Where `under` is
    /// false it leaves alone ground another pad already holds.
    fn pad(&mut self, core: &plan::Rect, height: i32, falloff: i32, yard: i32, under: bool, inside: impl Fn(IVec2) -> bool) {
        for z in core.min.y - falloff..=core.max.y + falloff {
            for x in core.min.x - falloff..=core.max.x + falloff {
                let p = IVec2::new(x, z);
                let Some(i) = self.index(p) else { continue };
                let in_core = x >= core.min.x && x <= core.max.x && z >= core.min.y && z <= core.max.y && inside(p);
                let cell = &mut self.cells[i];
                if cell.core && !under {
                    continue;
                }
                if in_core {
                    cell.core = true;
                    cell.target = height as i16;
                    cell.weight = 255;
                    cell.clear = true;
                    continue;
                }
                let dx = (core.min.x - x).max(x - core.max.x).max(0) as f32;
                let dz = (core.min.y - z).max(z - core.max.y).max(0) as f32;
                let d = (dx * dx + dz * dz).sqrt().max(if inside(p) { 0.0 } else { 0.7 });
                if d <= yard as f32 {
                    cell.clear = true;
                }
                let w = smoothstep(1.0 - d / (falloff as f32 + 1.0));
                if w > 0.0 {
                    let s = &mut self.sums[i];
                    s.0 += w;
                    s.1 += w * height as f32;
                    s.2 = s.2.max(w);
                }
            }
        }
    }

    fn clear_rect(&mut self, r: &plan::Rect) {
        for z in r.min.y..=r.max.y {
            for x in r.min.x..=r.max.x {
                if let Some(i) = self.index(IVec2::new(x, z)) {
                    self.cells[i].clear = true;
                }
            }
        }
    }

    /// Opens a glade round the buildings `rects`: no trees within `near` blocks of one, so
    /// no crown hangs over a roof, then fewer towards `far`, where the land's own trees
    /// take over; `keep` is the share of trees left just past `near`.
    fn glade(&mut self, rects: &[plan::Rect], near: i32, far: i32, keep: f32, seed: u64) {
        let mut nearest = vec![f32::MAX; self.cells.len()];
        for r in rects {
            for z in r.min.y - far..=r.max.y + far {
                for x in r.min.x - far..=r.max.x + far {
                    let Some(i) = self.index(IVec2::new(x, z)) else { continue };
                    let dx = (r.min.x - x).max(x - r.max.x).max(0) as f32;
                    let dz = (r.min.y - z).max(z - r.max.y).max(0) as f32;
                    nearest[i] = nearest[i].min((dx * dx + dz * dz).sqrt());
                }
            }
        }
        let size = self.max - self.min + 1;
        for (i, &d) in nearest.iter().enumerate() {
            if d > far as f32 {
                continue;
            }
            let t = ((d - near as f32) / (far - near) as f32).clamp(0.0, 1.0);
            let kept = if d <= near as f32 { 0.0 } else { keep + (1.0 - keep) * t * t };
            let (x, z) = (self.min.x + i as i32 % size.x, self.min.y + i as i32 / size.x);
            if canvas::unit(crate::rng::hash3(seed, x, 7, z)) >= kept {
                self.cells[i].clear = true;
            }
        }
    }

    /// Settles the blended targets of the ground between pads.
    fn finish(&mut self) {
        for (cell, s) in self.cells.iter_mut().zip(&self.sums) {
            if !cell.core && s.0 > 0.0 {
                cell.target = (s.1 / s.0).round() as i16;
                cell.weight = (s.2 * 255.0).round() as u8;
            }
        }
        self.sums = Vec::new();
    }

    fn lane(&mut self, lane: &plan::Lane) {
        let reach = lane.half + 1.5;
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in &lane.points {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let (lo, hi) = ((lo - reach).floor().as_ivec2(), (hi + reach).ceil().as_ivec2());
        for z in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let Some(i) = self.index(IVec2::new(x, z)) else { continue };
                let d = lane_distance(&lane.points, Vec2::new(x as f32, z as f32));
                let tread = if d <= lane.half { 1.0 } else { 1.0 - (d - lane.half) / 1.2 };
                if tread > 0.0 {
                    let cell = &mut self.cells[i];
                    cell.tread = cell.tread.max((tread.min(1.0) * 255.0) as u8);
                }
                if d <= lane.half + 1.0 {
                    self.cells[i].clear = true;
                }
            }
        }
    }
}

fn lane_distance(points: &[Vec2], p: Vec2) -> f32 {
    let mut best = points.first().map_or(f32::MAX, |q| q.distance(p));
    for w in points.windows(2) {
        let ab = w[1] - w[0];
        let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        best = best.min(p.distance(w[0] + ab * t));
    }
    best
}

/// A square of the grid: its settlement, if it has one.
pub struct Slot(pub Option<Plan>);

impl Weigh for Slot {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.0.as_ref().map_or(0, |p| {
            // Capacity counts allocated storage, even when a vector is only partly full.
            // The hash table has vacant buckets and one control byte per bucket, plus
            // a trailing control group. Round capacity up for its bucket allocation.
            let buckets = if p.voxels.capacity() == 0 { 0 } else { p.voxels.capacity().next_power_of_two() };
            let table = buckets * (std::mem::size_of::<(IVec2, Vec<Voxel>)>() + 1) + usize::from(buckets > 0) * 16;
            p.raster.cells.capacity() * std::mem::size_of::<Cell>()
                + p.raster.sums.capacity() * std::mem::size_of::<(f32, f32, f32)>()
                + table + p.voxels.values().map(|v| v.capacity() * std::mem::size_of::<Voxel>()).sum::<usize>()
                + p.buildings.capacity() * std::mem::size_of::<Building>()
                + p.buildings.iter().map(|b| b.what.capacity() + b.work.as_ref().map_or(0, String::capacity) + b.room.capacity() * std::mem::size_of::<IVec2>() + (b.beds.capacity() + b.seats.capacity()) * std::mem::size_of::<IVec3>()).sum::<usize>()
                + p.people.capacity() * std::mem::size_of::<people::Resident>()
                + p.people.iter().map(|r| r.species.capacity()).sum::<usize>()
                + p.gates.capacity() * std::mem::size_of::<IVec2>()
                + p.lanes.capacity() * std::mem::size_of::<Vec<Vec2>>()
                + p.lanes.iter().map(|l| l.capacity() * std::mem::size_of::<Vec2>()).sum::<usize>()
                + p.lane_halves.capacity() * std::mem::size_of::<f32>()
                + p.walls.capacity() * std::mem::size_of::<(IVec2, i32)>()
                + p.size.capacity() + p.layout.capacity() + p.style.capacity() + p.name.0.capacity() + p.name.1.capacity()
                + p.culture.bytes() - std::mem::size_of::<culture::Culture>()
        })
    }
}

thread_local! {
    /// The squares this thread used last, sparing the shared cache's lock.
    static RECENT: RefCell<Vec<(u64, IVec2, Arc<Slot>)>> = const { RefCell::new(Vec::new()) };
}

/// The settlements of a world.
pub struct Settlements {
    seed: u64,
    def: SettlementsDef,
    resolved: Resolved,
    /// None where the content lacks a block every settlement needs; then there are none.
    fixed: Option<Fixed>,
    plans: Cache<IVec2, Arc<Slot>>,
    /// The land's own tree blocks (natural leaves and logs) by block id, which no roof
    /// lies under.
    wild_tree: Vec<bool>,
    /// How high over a building's floor they are cut back: over the tallest tree rooted
    /// up to a glade's width uphill.
    crowns: i32,
    /// Near where a new player spawns (see `Overworld::spawn_estimate`), and the square
    /// settled for them there, if none was already within reach.
    spawn: std::sync::OnceLock<Spawn>,
    /// The squares in reach of where a new player spawns, one of which may be settled
    /// for them (`spawn_square`).
    promised: std::sync::OnceLock<Vec<IVec2>>,
    id: u64,
    /// Terrain fits by region, shared by nearby settlements, bounded by owned bytes.
    cells: Cache<IVec2, culture::Fits>,
    /// How many squares were planned, for tests that no square is planned twice.
    #[cfg(test)]
    pub(crate) planned: std::sync::atomic::AtomicUsize,
}

/// What finding the spawn's square settled: the square, and the plans made on the way,
/// which the squares' first asks take instead of planning again.
struct Spawn {
    square: Option<(IVec2, IVec2)>,
    early: std::sync::Mutex<FxHashMap<IVec2, Arc<Slot>>>,
}

impl Settlements {
    /// `trees`: how far over its foot the land's tallest tree reaches (`trees::tallest`).
    pub fn new(seed: u64, def: &SettlementsDef, registry: &BlockRegistry, trees: i32) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            seed: crate::rng::mix(seed ^ 0x5e77_1e5e),
            def: def.clone(),
            resolved: Resolved::new(def, registry),
            fixed: fixed(registry, def),
            plans: Cache::new(32 * MB),
            wild_tree: (0..registry.len())
                .map(|i| {
                    let b = registry.def(BlockId(i as u16));
                    b.has_tag("natural") && (b.render == RenderKind::Leaves || b.has_tag("log"))
                })
                .collect(),
            crowns: trees + def.site.glade.0 + 1,
            spawn: std::sync::OnceLock::new(),
            promised: std::sync::OnceLock::new(),
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            cells: Cache::new(MB),
            #[cfg(test)]
            planned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The lists and routines of the people who live in settlements.
    pub fn people(&self) -> &people::PeopleDef {
        &self.def.people
    }

    /// The names of the sizes settlements come in, as written.
    pub fn sizes(&self) -> Vec<String> {
        self.def.sizes.iter().map(|s| s.name.clone()).collect()
    }

    /// The square of the grid holding (x, z).
    pub fn cell_of(&self, x: i32, z: i32) -> IVec2 {
        IVec2::new(x, z).div_euclid(IVec2::splat(self.def.grid.spacing))
    }

    /// How many squares are planned, for tests of how much a piece of generation needs.
    #[cfg(test)]
    pub(crate) fn made(&self) -> usize {
        self.plans.len()
    }

    /// Whose `plan` is: its family, second family, quirks, axes and mutations.
    pub fn culture_info(&self, plan: &Plan) -> CultureInfo {
        let def = &self.def.cultures;
        let c = &plan.culture;
        let family = &def.families[c.family];
        let entry = |e: &culture::Entry| format!("{}:{}", e.listing.name(), e.name);
        CultureInfo {
            family: family.name.clone(),
            title: family.title.clone(),
            second: c.second.as_ref().map(|s| (def.families[s.family].name.clone(), s.share)),
            quirks: c.quirks.iter().map(|&q| (def.quirks[q].name.clone(), def.quirks[q].title.clone())).collect(),
            axes: c.axes,
            dropped: c.dropped.iter().map(entry).collect(),
            borrowed: c.borrowed.iter().map(|(f, e)| format!("{}:{}", def.families[*f].name, entry(e))).collect(),
        }
    }

    /// The settlement planned in square `cell`, if any.
    pub fn slot(&self, world: &Overworld, cell: IVec2) -> Arc<Slot> {
        let found = RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            let i = recent.iter().position(|(id, c, _)| *id == self.id && *c == cell)?;
            let entry = recent.remove(i);
            let slot = entry.2.clone();
            recent.insert(0, entry);
            Some(slot)
        });
        if let Some(slot) = found {
            return slot;
        }
        let slot = self.plans.get(cell, || {
            let _stage = crate::stage::enter(crate::stage::Stage::Settlements);
            let Some(fixed) = &self.fixed else { return Arc::new(Slot(None)) };
            if self.def.styles.is_empty() || self.def.sizes.is_empty() {
                return Arc::new(Slot(None));
            }
            let spawn = self.spawn.get_or_init(|| self.spawn_square(world, fixed));
            if let Some(slot) = spawn.early.lock().unwrap_or_else(|p| p.into_inner()).remove(&cell) {
                return slot;
            }
            let at = spawn.square.filter(|&(_, square)| square == cell).map(|(at, _)| at);
            Arc::new(Slot(self.plan(world, fixed, cell, at)))
        });
        RECENT.with(|recent| {
            let mut recent = recent.borrow_mut();
            recent.insert(0, (self.id, cell, slot.clone()));
            recent.truncate(6);
        });
        slot
    }

    /// The ground at (x, z) with the settlement over it graded in: pads pulled flat,
    /// lanes trodden, and both marked clear of trees and plants.
    pub fn apply(&self, world: &Overworld, mut sample: GeoSample, x: i32, z: i32) -> GeoSample {
        let slot = self.slot(world, self.cell_of(x, z));
        let Some(plan) = &slot.0 else { return sample };
        if !plan.contains(x, z) {
            return sample;
        }
        let Some(cell) = plan.raster.get(x, z) else { return sample };
        if cell.weight == 0 && cell.tread == 0 && !cell.clear {
            return sample;
        }
        if !sample.submerged() && sample.cavity.is_none() && cell.weight > 0 {
            sample.height = cell.graded(&sample);
            sample.crag.amplitude = 0.0;
        }
        let tread = cell.tread as f32 / 255.0;
        let mine = TrailMark { tread, deck: None, log: false, pier: false, stone: false, clear: cell.clear || tread > 0.0 };
        sample.trail = Some(match sample.trail {
            Some(t) => TrailMark { tread: t.tread.max(mine.tread), clear: t.clear || mine.clear, ..t },
            None => mine,
        });
        sample
    }

    /// Builds what settlements drew into the column at `pos`. `ground` gives the height of
    /// a column's top ground block.
    /// Returns the highest block it set, if any, so the snow blanket looks down from over
    /// the tallest roof.
    pub fn build(&self, world: &Overworld, pos: ColumnPos, blocks: &mut Blocks, ground: &dyn Fn(i32, i32) -> i32) -> Option<i32> {
        let origin = pos.origin();
        let key = IVec2::new(origin.x, origin.z).div_euclid(IVec2::splat(CHUNK_SIZE));
        // The squares the column touches (it lies in one; a square's settlement never
        // reaches past it).
        let slot = self.slot(world, self.cell_of(origin.x, origin.z));
        let plan = slot.0.as_ref()?;
        self.prune(plan, origin, blocks);
        let voxels = plan.voxels.get(&key)?;
        let mut top = None;
        for v in voxels {
            let pos = if v.ground { IVec3::new(v.pos.x, ground(v.pos.x, v.pos.z) + v.pos.y, v.pos.z) } else { v.pos };
            if !blocks.contains(pos) {
                continue;
            }
            if !matches!(v.put, Put::Clear) {
                top = top.max(Some(pos.y));
            }
            match v.put {
                Put::Set(block) => blocks.set(pos, block),
                Put::Clear => blocks.set(pos, BlockId::AIR),
                Put::Soft(block) => {
                    if blocks.soft(pos) {
                        blocks.set(pos, block);
                    }
                }
                Put::Down(block) => {
                    let mut p = pos;
                    for _ in 0..10 {
                        if !blocks.contains(p) || !blocks.soft(p) {
                            break;
                        }
                        blocks.set(p, block);
                        p.y -= 1;
                    }
                }
            }
        }
        top
    }

    /// Cuts back the land's trees over each building and `site.eaves` blocks round it:
    /// the glade keeps their trunks `site.glade.0` blocks off, but a crown reaches further,
    /// and one over a roof buries it. Done before the settlement's own blocks go in, so its
    /// planted trees stand whole.
    fn prune(&self, plan: &Plan, origin: IVec3, blocks: &mut Blocks) {
        let eaves = self.def.site.eaves;
        let (lo, hi) = (IVec2::new(origin.x, origin.z), IVec2::new(origin.x, origin.z) + CHUNK_SIZE - 1);
        for b in &plan.buildings {
            let (min, max) = ((b.min - eaves).max(lo), (b.max + eaves).min(hi));
            for z in min.y..=max.y {
                for x in min.x..=max.x {
                    for y in b.floor + 1..=b.floor + self.crowns {
                        let p = IVec3::new(x, y, z);
                        if blocks.contains(p) && self.wild_tree[blocks.get(p).0 as usize] {
                            blocks.set(p, BlockId::AIR);
                        }
                    }
                }
            }
        }
    }

    /// The settlements whose squares lie within `radius` blocks of `centre`.
    pub fn near(&self, world: &Overworld, centre: IVec2, radius: i32) -> Vec<Arc<Slot>> {
        let (first, last) = (self.cell_of(centre.x - radius, centre.y - radius), self.cell_of(centre.x + radius, centre.y + radius));
        let cells: Vec<IVec2> = (first.y..=last.y).flat_map(|cz| (first.x..=last.x).map(move |cx| IVec2::new(cx, cz))).collect();
        // Squares nobody has planned yet are planned side by side first (see
        // `crate::parallel`): each may need a river region of its own, and trails ask for
        // several at once, which one after another kept a new world's first columns waiting.
        let missing: Vec<IVec2> = cells.iter().copied().filter(|&c| !self.plans.has(c)).collect();
        if missing.len() > 1 {
            crate::parallel::for_each(&missing, |&c| {
                self.slot(world, c);
            });
        }
        cells.into_iter().map(|c| self.slot(world, c)).filter(|slot| slot.0.is_some()).collect()
    }

    /// The settlement nearest `from` by its middle, within `radius` of it, that `keep`
    /// takes. Squares are planned nearest first, a few side by side at a time, and the
    /// search stops once no square left could hold a nearer one: a settlement never
    /// reaches past its square, so the nearest point of a square bounds how near any of
    /// its places can be. Only the squares nearer than the answer are planned, where
    /// `near` would plan every one within `radius`.
    pub fn nearest(&self, world: &Overworld, from: IVec2, radius: i32, keep: impl Fn(&Plan) -> bool) -> Option<Arc<Slot>> {
        let spacing = self.def.grid.spacing;
        let reach = i64::from(radius) * i64::from(radius);
        let away = |p: IVec2| (p - from).as_i64vec2().length_squared();
        let (first, last) = (self.cell_of(from.x - radius, from.y - radius), self.cell_of(from.x + radius, from.y + radius));
        let mut cells: Vec<(i64, IVec2)> = (first.y..=last.y)
            .flat_map(|cz| (first.x..=last.x).map(move |cx| IVec2::new(cx, cz)))
            .map(|c| (away(from.clamp(c * spacing, (c + 1) * spacing - 1)), c))
            .filter(|&(bound, _)| bound <= reach)
            .collect();
        cells.sort_unstable_by_key(|&(bound, c)| (bound, c.x, c.y));
        let batch = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
        // Ties go to the lower x, then z, of the middle, as a sorted scan would give.
        let mut best: Option<((i64, i32, i32), Arc<Slot>)> = None;
        for group in cells.chunks(batch) {
            if best.as_ref().is_some_and(|(d, _)| group[0].0 > d.0) {
                break;
            }
            let missing: Vec<IVec2> = group.iter().map(|&(_, c)| c).filter(|&c| !self.plans.has(c)).collect();
            if missing.len() > 1 {
                crate::parallel::for_each(&missing, |&c| {
                    self.slot(world, c);
                });
            }
            for &(_, c) in group {
                let slot = self.slot(world, c);
                let Some(plan) = slot.0.as_ref() else { continue };
                let d = (away(plan.centre), plan.centre.x, plan.centre.y);
                // The cheap checks before `keep`, which may look the plan over.
                if d.0 <= reach && best.as_ref().is_none_or(|(b, _)| d < *b) && keep(plan) {
                    best = Some((d, slot.clone()));
                }
            }
        }
        best.map(|(_, slot)| slot)
    }

    /// The settlement of the square holding (x, z), planning that square alone: the only
    /// one any block there can belong to.
    pub fn holding(&self, world: &Overworld, x: i32, z: i32) -> Option<Arc<Slot>> {
        Some(self.slot(world, self.cell_of(x, z))).filter(|slot| slot.0.is_some())
    }

    /// Plans square `cell`, settled near `spawn` if it is the spawn's.
    fn plan(&self, world: &Overworld, fixed: &Fixed, cell: IVec2, spawn: Option<IVec2>) -> Option<Plan> {
        #[cfg(test)]
        self.planned.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let land = world.land();
        let ctx = plan::Ctx { world, land: &land, def: &self.def, resolved: &self.resolved, fixed, seed: self.seed, spawn, promised: self.promised(world), cells: &self.cells };
        plan::plan(&ctx, cell)
    }

    /// Where a new player will spawn, and the square settled for them within
    /// `spawn_settlement_within` of it: none where a square near is settled anyway within
    /// reach; else the nearest square that can be, trying the squares in reach nearest
    /// first. Planned once per world, before any square, so every square's plan is the
    /// same whichever is asked for first. The squares in reach are planned side by side,
    /// and each plan made here that stands is kept for the square's first ask, so none is
    /// planned twice.
    fn spawn_square(&self, world: &Overworld, fixed: &Fixed) -> Spawn {
        let mut spawn = Spawn { square: None, early: std::sync::Mutex::new(FxHashMap::default()) };
        let within = self.def.rules.spawn_settlement_within;
        if within <= 0 {
            return spawn;
        }
        let at = world.spawn_estimate();
        let cells = self.promised(world).to_vec();
        // Each square as any other would plan it, side by side.
        let made = std::sync::Mutex::new(FxHashMap::default());
        crate::parallel::for_each(&cells, |&c| {
            let plan = self.plan(world, fixed, c, None);
            made.lock().unwrap_or_else(|p| p.into_inner()).insert(c, plan);
        });
        let mut made = made.into_inner().unwrap_or_else(|p| p.into_inner());
        let near = |p: &Plan| p.centre.as_vec2().distance(at.as_vec2()) <= within as f32;
        if !cells.iter().any(|c| made[c].as_ref().is_some_and(near)) {
            // None near: the nearest square that can be settled is, near the spawn. Each
            // is tried side by side, as on a coast under mountains none may be.
            let forced = std::sync::Mutex::new(FxHashMap::default());
            crate::parallel::for_each(&cells, |&c| {
                let plan = self.plan(world, fixed, c, Some(at));
                forced.lock().unwrap_or_else(|p| p.into_inner()).insert(c, plan);
            });
            let mut forced = forced.into_inner().unwrap_or_else(|p| p.into_inner());
            if let Some(&c) = cells.iter().find(|c| forced[c].is_some()) {
                made.insert(c, forced.remove(&c).flatten());
                spawn.square = Some((at, c));
            }
        }
        spawn.early = std::sync::Mutex::new(made.into_iter().map(|(c, p)| (c, Arc::new(Slot(p)))).collect());
        spawn
    }

    /// The squares one of which may be settled for a new player (see `promised`).
    fn promised(&self, world: &Overworld) -> &[IVec2] {
        self.promised.get_or_init(|| if self.def.rules.spawn_settlement_within > 0 { self.in_reach(world.spawn_estimate()) } else { Vec::new() })
    }

    /// The squares whose ground lies within `spawn_settlement_within` of `at`, nearest
    /// first.
    fn in_reach(&self, at: IVec2) -> Vec<IVec2> {
        let within = self.def.rules.spawn_settlement_within;
        let grid = &self.def.grid;
        let inner = |cell: IVec2| (cell * grid.spacing + grid.margin, (cell + 1) * grid.spacing - grid.margin);
        let reach = |cell: IVec2| {
            let (lo, hi) = inner(cell);
            at.clamp(lo, hi).as_vec2().distance(at.as_vec2())
        };
        let (first, last) = (self.cell_of(at.x - within, at.y - within), self.cell_of(at.x + within, at.y + within));
        let mut cells: Vec<IVec2> = (first.y..=last.y).flat_map(|z| (first.x..=last.x).map(move |x| IVec2::new(x, z))).filter(|&c| reach(c) <= within as f32).collect();
        cells.sort_by(|a, b| reach(*a).total_cmp(&reach(*b)).then((a.x, a.y).cmp(&(b.x, b.y))));
        cells
    }

    /// Whether any square in reach of the spawn has a spot a settlement could stand on
    /// (see `plan::site`): where none has, the land itself keeps the promise from being
    /// kept.
    #[cfg(test)]
    pub(crate) fn spawn_has_sites(&self, world: &Overworld) -> bool {
        let Some(fixed) = &self.fixed else { return false };
        let at = world.spawn_estimate();
        let land = world.land();
        let ctx = plan::Ctx { world, land: &land, def: &self.def, resolved: &self.resolved, fixed, seed: self.seed, spawn: Some(at), promised: self.promised(world), cells: &self.cells };
        self.in_reach(at).into_iter().any(|c| !plan::site(&ctx, c, Some(at)).is_empty())
    }

    /// Whether (x, z) lies within a settlement's bounds, a few blocks round included.
    pub fn covers(&self, world: &Overworld, x: i32, z: i32) -> bool {
        let p = IVec2::new(x, z);
        self.slot(world, self.cell_of(x, z)).0.as_ref().is_some_and(|plan| p.cmpge(plan.min - 4).all() && p.cmple(plan.max + 4).all())
    }

    /// Whether any settlement's gate lies within `reach` blocks of the box from `min` to
    /// `max`.
    pub fn any_near(&self, world: &Overworld, min: IVec2, max: IVec2, reach: i32) -> bool {
        let centre = (min + max) / 2;
        let radius = (max - min).max_element() / 2 + reach;
        self.near(world, centre, radius).iter().any(|slot| {
            slot.0.as_ref().is_some_and(|p| p.gates.iter().chain(std::iter::once(&p.centre)).any(|g| g.cmpge(min - reach).all() && g.cmple(max + reach).all()))
        })
    }
}

impl Settlements {
    /// The roof pieces the plan of the settlement whose square's middle is at `centre`
    /// sets, and where: for tools judging a settlement as the world holds it.
    pub fn roofs_of(&self, world: &Overworld, centre: IVec2) -> Option<Vec<(IVec3, BlockId)>> {
        let fixed = self.fixed.as_ref()?;
        self.planned(world, centre, &|b| fixed.fire.piece(b))
    }

    /// The blocks `keep` picks that the plan of the settlement whose square's middle is at
    /// `centre` sets, and where.
    pub fn planned(&self, world: &Overworld, centre: IVec2, keep: &dyn Fn(BlockId) -> bool) -> Option<Vec<(IVec3, BlockId)>> {
        let slot = self.slot(world, self.cell_of(centre.x, centre.y));
        let plan = slot.0.as_ref().filter(|p| p.centre == centre)?;
        let mut roofs: Vec<(IVec3, BlockId)> = plan
            .voxels
            .values()
            .flatten()
            .filter(|v| !v.ground)
            .filter_map(|v| match v.put {
                Put::Set(b) if keep(b) => Some((v.pos, b)),
                _ => None,
            })
            .collect();
        roofs.sort_unstable_by_key(|r| (r.0.y, r.0.x, r.0.z));
        Some(roofs)
    }

    /// As [`Settlements::planned`], the blocks set at a height over the ground of their
    /// column rather than at one of their own (a lamp's torch on its post): each with its
    /// height over the ground's top block, 0 being that block.
    pub fn planned_on_ground(&self, world: &Overworld, centre: IVec2, keep: &dyn Fn(BlockId) -> bool) -> Option<Vec<(IVec3, BlockId)>> {
        let slot = self.slot(world, self.cell_of(centre.x, centre.y));
        let plan = slot.0.as_ref().filter(|p| p.centre == centre)?;
        let mut set: Vec<(IVec3, BlockId)> = plan
            .voxels
            .values()
            .flatten()
            .filter(|v| v.ground)
            .filter_map(|v| match v.put {
                Put::Set(b) if keep(b) => Some((v.pos, b)),
                _ => None,
            })
            .collect();
        set.sort_unstable_by_key(|r| (r.0.y, r.0.x, r.0.z));
        Some(set)
    }

    /// What its flames are kept from (`rules.fire`).
    pub fn fire(&self) -> Option<&roofs::Fire> {
        self.fixed.as_ref().map(|f| &f.fire)
    }
}

/// Whether a settlement furnishes itself with block `b` picked by tag: a plain common
/// sort, no wonder, nothing supernatural, no pair or clump.
fn plain(registry: &BlockRegistry, b: BlockId) -> bool {
    let def = registry.def(b);
    !def.has_tag("wonder") && !def.has_tag("supernatural") && !def.name.ends_with("_pair") && !def.name.ends_with("_clump") && def.rarity == voxmar_engine::block::Rarity::Common
}

/// Whether a settlement's flowers and bushes, picked by tag, take block `b`: a plain small
/// plant.
pub(crate) fn small_plant(registry: &BlockRegistry, b: BlockId) -> bool {
    plain(registry, b) && registry.def(b).render == RenderKind::Plant
}

/// The blocks settlements furnish themselves with (`furnishings` in settlements.ron):
/// none without water and dirt, and only the plain common sorts of flowers and bushes.
fn fixed(registry: &BlockRegistry, def: &SettlementsDef) -> Option<Fixed> {
    let f: &FurnishingsDef = &def.furnishings;
    let id = |name: &Option<String>| name.as_deref().and_then(|n| registry.id(n));
    let palette = |list: &[Select], filter: &dyn Fn(BlockId) -> bool| {
        Palette(
            list.iter()
                .flat_map(|s| -> Vec<(BlockId, f32)> {
                    match s {
                        Select::Name(name, w) => registry.id(name).map(|b| (b, *w)).into_iter().collect(),
                        Select::Tag(tag, w) => registry.tagged(tag).filter(|&b| filter(b)).map(|b| (b, *w)).collect(),
                    }
                })
                .filter(|(_, w)| *w > 0.0)
                .collect(),
        )
    };
    let small_plant = |b: BlockId| small_plant(registry, b);
    let any = |_: BlockId| true;
    let dirt = id(&f.dirt)?;
    Some(Fixed {
        torch: id(&f.torch).unwrap_or(BlockId::AIR),
        campfire: id(&f.campfire).unwrap_or(BlockId::AIR),
        table: id(&f.table).unwrap_or(BlockId::AIR),
        farmland: id(&f.farmland).unwrap_or(dirt),
        water: id(&f.water)?,
        planter: id(&f.planter),
        anvil: id(&f.anvil).unwrap_or(BlockId::AIR),
        barrel: id(&f.barrel),
        bell: id(&f.bell),
        altar: id(&f.altar),
        dirt,
        flowers: palette(&f.flowers, &small_plant),
        bushes: palette(&f.bushes, &small_plant),
        canopies: palette(&f.canopies, &any),
        compost: palette(&f.compost, &any),
        boulder: palette(&f.boulders, &any),
        // Each tree's fruit's level, from the crop its leaves are a stage of (crops.ron);
        // none known, none needed.
        fruit: palette(&f.fruit, &any).0.into_iter().map(|(b, w)| (b, w, def.stage_of(&registry.def(b).name).map_or(0, |(_, c, _)| c.level))).collect(),
        hay: palette(&f.hay, &any),
        hanging: palette(&f.hanging, &any),
        nets: palette(if f.nets.is_empty() { &f.hanging } else { &f.nets }, &any),
        sacks: palette(&f.sacks, &any),
        crates: palette(&f.crates, &any),
        cloth: palette(&f.cloth, &any),
        interiors: interior::Interiors::new(&def.interiors, registry),
        fire: roofs::Fire::new(&def.rules.fire, registry, def.styles.iter().flat_map(|s| {
            let woods = &s.woods;
            s.roof_pieces.iter().flat_map(move |p| {
                woods.iter().flat_map(move |(wood, _)| woods.iter().filter_map(move |(trim, _)| roofs::RoofPieces::resolve(p, wood, trim, |n| registry.id(n))))
            })
        })),
        plinth: def.rules.houses.plinth,
        moss: def.rules.houses.moss,
    })
}
