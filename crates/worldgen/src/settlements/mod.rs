//! Stage 3c: settlements, hamlets and villages grown on good ground.
//!
//! The world is cut into squares (`grid` in settlements.ron) and about a third of them
//! hold a settlement, placed well inside so it never reaches the next. Within its square
//! a settlement takes the best of a few likely spots: firm, dry, gentle ground, away from
//! the channels and banks of rivers and lakes but with fresh water near, never on crags or
//! terrain shapes or in the biomes the file avoids ([`plan::site`]).
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
pub mod def;
mod house;
mod plan;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::{IVec2, IVec3, Vec2};

pub use self::canvas::Facing;
use self::canvas::{Palette, Put, Voxel};
pub use self::def::{Levers, SettlementsDef};
use self::def::Resolved;
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
    /// The height of its floor.
    pub floor: i32,
    /// The block just outside its door, at the height of the doorway's foot.
    pub door: Option<IVec3>,
    pub facing: Facing,
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
    pub buildings: Vec<Building>,
    /// Where lanes leading out end, which trails join.
    pub gates: Vec<IVec2>,
    /// Lanes' centre lines, for maps.
    pub lanes: Vec<Vec<Vec2>>,
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

    /// A pad at `height` over `core`, held exactly where `inside`, easing out over
    /// `falloff` blocks, its yard kept clear for `yard` blocks round it.
    fn pad(&mut self, core: &plan::Rect, height: i32, _round: bool, falloff: i32, yard: i32, inside: impl Fn(IVec2) -> bool) {
        for z in core.min.y - falloff..=core.max.y + falloff {
            for x in core.min.x - falloff..=core.max.x + falloff {
                let p = IVec2::new(x, z);
                let Some(i) = self.index(p) else { continue };
                let in_core = x >= core.min.x && x <= core.max.x && z >= core.min.y && z <= core.max.y && inside(p);
                let cell = &mut self.cells[i];
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
        self.0.as_ref().map_or(0, |p| {
            p.raster.cells.len() * std::mem::size_of::<Cell>()
                + p.voxels.values().map(|v| v.len() * std::mem::size_of::<Voxel>() + 32).sum::<usize>()
                + p.buildings.len() * std::mem::size_of::<Building>()
                + 256
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
    id: u64,
}

impl Settlements {
    pub fn new(seed: u64, def: &SettlementsDef, registry: &BlockRegistry) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            seed: crate::rng::mix(seed ^ 0x5e77_1e5e),
            def: def.clone(),
            resolved: Resolved::new(def, registry),
            fixed: fixed(registry),
            plans: Cache::new(32 * MB),
            id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// The square of the grid holding (x, z).
    pub fn cell_of(&self, x: i32, z: i32) -> IVec2 {
        IVec2::new(x, z).div_euclid(IVec2::splat(self.def.grid.spacing))
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
            let land = world.land();
            let ctx = plan::Ctx { world, land: &land, def: &self.def, resolved: &self.resolved, fixed, seed: self.seed };
            Arc::new(Slot(plan::plan(&ctx, cell)))
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
            let target = cell.target as i32;
            let height = if cell.core {
                target
            } else {
                let w = cell.weight as f32 / 255.0;
                (sample.height as f32 + (target - sample.height) as f32 * w).round() as i32
            };
            // Never below the water beside it, which would let it spill.
            sample.height = height.max(sample.water);
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
    pub fn build(&self, world: &Overworld, pos: ColumnPos, blocks: &mut Blocks, ground: &dyn Fn(i32, i32) -> i32) {
        let origin = pos.origin();
        let key = IVec2::new(origin.x, origin.z).div_euclid(IVec2::splat(CHUNK_SIZE));
        // The squares the column touches (it lies in one; a square's settlement never
        // reaches past it).
        let slot = self.slot(world, self.cell_of(origin.x, origin.z));
        let Some(plan) = &slot.0 else { return };
        let Some(voxels) = plan.voxels.get(&key) else { return };
        for v in voxels {
            let pos = if v.ground { IVec3::new(v.pos.x, ground(v.pos.x, v.pos.z) + v.pos.y, v.pos.z) } else { v.pos };
            if !blocks.contains(pos) {
                continue;
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
    }

    /// The settlements whose squares lie within `radius` blocks of `centre`.
    pub fn near(&self, world: &Overworld, centre: IVec2, radius: i32) -> Vec<Arc<Slot>> {
        let (first, last) = (self.cell_of(centre.x - radius, centre.y - radius), self.cell_of(centre.x + radius, centre.y + radius));
        let mut found = Vec::new();
        for cz in first.y..=last.y {
            for cx in first.x..=last.x {
                let slot = self.slot(world, IVec2::new(cx, cz));
                if slot.0.is_some() {
                    found.push(slot);
                }
            }
        }
        found
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

/// Blocks every settlement needs, and the lists every style shares.
fn fixed(registry: &BlockRegistry) -> Option<Fixed> {
    let id = |name: &str| registry.id(&format!("base:{name}"));
    let palette = |names: &[&str]| Palette(names.iter().filter_map(|n| id(n)).map(|b| (b, 1.0)).collect());
    let plain = |b: BlockId| {
        let def = registry.def(b);
        !def.has_tag("wonder") && !def.has_tag("supernatural") && !def.name.ends_with("_pair") && !def.name.ends_with("_clump") && def.rarity == voxmar_engine::block::Rarity::Common
    };
    let flowers = Palette(registry.tagged("flower").filter(|&b| plain(b) && registry.def(b).render == RenderKind::Plant).map(|b| (b, 1.0)).collect());
    let bushes = Palette(registry.tagged("bush").filter(|&b| plain(b) && registry.def(b).render == RenderKind::Plant).map(|b| (b, 1.0)).collect());
    let fruit = Palette(
        registry
            .tagged("fruit_leaves")
            .filter(|&b| registry.def(b).has_tag("ripe") && !registry.def(b).has_tag("wonder"))
            .filter(|&b| ["apple", "pear", "plum", "cherries", "orange", "lemon"].iter().any(|f| registry.def(b).name == format!("base:{f}_leaves")))
            .map(|b| (b, 1.0))
            .collect(),
    );
    // Torches, campfires, tables and farmland are other files' blocks: content without
    // them (worldgen's own tests) leaves lamps bare, hearths cold and fields on dirt.
    let dirt = id("dirt")?;
    Some(Fixed {
        torch: id("torch").unwrap_or(BlockId::AIR),
        campfire: id("campfire").unwrap_or(BlockId::AIR),
        table: id("crafting_table").unwrap_or(BlockId::AIR),
        farmland: id("farmland").unwrap_or(dirt),
        water: id("water")?,
        planter: id("herb_planter"),
        anvil: id("polished_basalt").or(id("polished_stone"))?,
        dirt,
        flowers,
        bushes,
        canopies: palette(&["red_terracotta", "orange_terracotta", "yellow_terracotta", "white_terracotta", "light_blue_terracotta", "cyan_terracotta", "lime_terracotta", "magenta_terracotta", "pink_terracotta", "green_terracotta", "blue_terracotta"]),
        compost: palette(&["coarse_dirt", "rooted_dirt", "podzol"]),
        boulder: palette(&["cobblestone", "mossy_cobblestone", "cobbled_andesite"]),
        fruit,
    })
}
