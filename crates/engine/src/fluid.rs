//! Fluids: water and lava that flow, Minecraft-style.
//!
//! **States are blocks.** A block with a `fluid` definition is a fluid's source, and the
//! registry makes the blocks of its other states right after it: seven flowing levels
//! (`water_flow_1`, the deepest, to `water_flow_7`), a falling state, and for rivers
//! (`currents`) still sources with a partial surface (`water_top_1..7`, in eighths) and
//! pinned water carrying a current (`water_current_<direction>_<speed>_<eighths>`). So
//! chunks stay a palette of ids, saves keep states by name and ordinary block diffs
//! replicate them. Every state draws like its source until the mesher reads
//! [`FluidState`] (see "Fluid state" in `docs/rendering.md`, and [`corner_heights`] and
//! [`flow`], which the mesher and the player's physics share).
//!
//! **The rule** is Minecraft's, read from its behaviour, with sources that never drain:
//! - A cell holds `amount` eighths: 8 for sources and falling cells. A flowing cell holds
//!   the most of its four sides less `drop` (water 1, so it runs 7 blocks over flat
//!   ground; lava 2), or falls when the same fluid lies above it, or dries up.
//! - Fluid falls first. What cannot fall spreads sideways (except flowing water standing
//!   on more of itself, which stays a column; a source over a hole spreads too when three
//!   sources surround it), but only towards the nearest way down within `slope` blocks
//!   (every way when there is none), so streams find the edge of a ledge instead of
//!   flooding a plateau. Like Minecraft's, a source placed in the air spills over its four
//!   sides as it pours.
//! - With `infinite`, a cell between two sources over ground (or a source) becomes a
//!   source: water taken out of a pool comes back. Two pinned river cells beside it
//!   make it pinned river water with their mean current, so a breach in a river bank
//!   carries the river's current.
//! - With `pools`, flowing water that has filled a hollow it cannot run out of (nothing
//!   open beside it, nothing beside it lower, ground beneath, not fed from above) becomes a
//!   source, and the hollow fills back towards the inflow: streams leave ponds in dips and
//!   ditches, and the pool under a waterfall stays when the fall stops.
//! - With `crust`, a flowing cell on the ground that dries up hardens instead (lava leaves
//!   its course as rock), and the thin edge of a flow that has rested for a while hardens too, so a lava
//!   flow ends in a rim of crust.
//! - Where a fluid touches another it `meets` (lava beside or under water), it turns
//!   solid: a source into one block, a flowing cell into another, and the fluid it flows
//!   down into into a third.
//!
//! **Generated water** must be a fixed point of the rule, which [`unsettled`] checks, so
//! that loading it costs nothing. Water lies on ground or water with ground or water beside
//! it, except where a waterfall pours, which generation stamps as the rule would pour it:
//! an air cell beside the upper water (the face of the fall) holds `water_flow_1`
//! (`water_flow_<9 − a>` beside a partial top of `a` eighths, nothing when `a` is 1), or
//! `water_falling` when the cell above it holds water; and every cell straight below that,
//! down to the lower water, holds `water_falling`. The lower water must be water, not ice:
//! falling water would spread over ice.
//!
//! **Event-driven.** Only cells beside a change enter the queue, each due after its
//! fluid's `tick` (plus a little jitter hashed from the cell, so a front does not move in
//! lockstep). A tick whose cell changes nothing schedules nothing, so settled water costs
//! nothing, and loading generated water schedules nothing. A tick runs at most a [`Budget`] of updates, which adapts to what
//! updates cost and how busy the server is; the rest wait, in order, for the next tick.
//! A cell whose update needs a column that is not loaded waits for that column.
//!
//! Everything is deterministic: cells run in the order they were scheduled, and nothing
//! depends on hashing, threads or the clock, given the same budget per tick.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};

use glam::{IVec3, Vec2};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockDef, BlockId, BlockRegistry, RegistryError};
use crate::coords::{CHUNK_BITS, CHUNK_SIZE, CHUNK_VOLUME, ColumnPos, WORLD_HEIGHT};
use crate::schedule::Tick;
use crate::world::{Column, VoxelWorld};

/// How a fluid behaves, as written on its source block (`fluid: (...)` in `blocks.ron`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FluidDef {
    /// Ticks between steps of a flow: water 5 (four blocks a second), lava 30.
    pub tick: u8,
    /// Eighths of depth a flow loses per block sideways: 1 runs seven blocks over flat
    /// ground, 2 three.
    pub drop: u8,
    /// How far, in blocks, a flow looks along the ground for a way down.
    pub slope: u8,
    /// A cell between two sources, over ground or a source, becomes a source too.
    pub infinite: bool,
    /// Flowing water that fills a hollow it cannot drain from stays there as a pool.
    pub pools: bool,
    /// What the fluid hardens into where it stops: lava's crust.
    pub crust: Option<CrustDef>,
    /// Makes the river states: still sources with a partial surface, and pinned water
    /// with a current.
    pub currents: bool,
    /// What happens where it touches another fluid.
    pub meets: Vec<MeetingDef>,
}

impl Default for FluidDef {
    fn default() -> Self {
        Self { tick: 5, drop: 1, slope: 4, infinite: false, pools: false, crust: None, currents: false, meets: Vec::new() }
    }
}

/// How a fluid crusts over: a flowing cell that dries up hardens at once, and the thin
/// edge of a flow that stays put hardens after `after` ticks, leaving a rim of rock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrustDef {
    pub block: String,
    pub after: u16,
}

/// Where a fluid touches another, by block names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingDef {
    /// The other fluid's source block.
    pub fluid: String,
    /// What a source of this fluid becomes with the other beside or above it.
    pub source: String,
    /// What a flowing or falling cell of this fluid becomes there.
    pub flowing: String,
    /// What the other fluid becomes where this one flows down into it.
    pub below: String,
}

/// A fluid, numbered in the order its source blocks load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FluidId(pub u8);

/// A river's current: the direction it flows and how fast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Current {
    /// In 16ths of a turn counter-clockwise from +x, towards +z (as the rivers measure it).
    pub direction: u8,
    /// 1 (slow) ..= 3 (rapids).
    pub speed: u8,
}

/// Blocks per second a current of each speed carries a swimmer, and a flow.
const CURRENT_SPEEDS: [f32; 4] = [0.0, 0.7, 1.4, 2.8];
/// Blocks per second a flowing (not pinned) fluid carries a swimmer.
pub const FLOW_SPEED: f32 = 1.4;

impl Current {
    pub const DIRECTIONS: u8 = 16;
    pub const MAX_SPEED: u8 = 3;

    /// Unit vector (x, z) of the direction.
    pub fn direction_vector(self) -> Vec2 {
        let angle = self.direction as f32 * std::f32::consts::TAU / Self::DIRECTIONS as f32;
        Vec2::new(angle.cos(), angle.sin())
    }

    /// How the current moves what floats in it, in blocks per second along (x, z).
    pub fn velocity(self) -> Vec2 {
        self.direction_vector() * CURRENT_SPEEDS[self.speed.min(Self::MAX_SPEED) as usize]
    }

    /// The current along `v`, whose length is the speed; none when it rounds to still.
    fn from_vector(v: Vec2) -> Option<Self> {
        let speed = (v.length().round() as u8).min(Self::MAX_SPEED);
        if speed == 0 {
            return None;
        }
        let turns = v.y.atan2(v.x) / std::f32::consts::TAU;
        let direction = ((turns * Self::DIRECTIONS as f32).round() as i32).rem_euclid(Self::DIRECTIONS as i32) as u8;
        Some(Self { direction, speed })
    }
}

/// What a fluid block is: which fluid, how much of the cell it fills and how it moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FluidState {
    pub fluid: FluidId,
    /// Eighths of the cell filled, 1..=8: 8 for a source or a falling cell, 7 down to 1
    /// as a flow thins out, and a river or lake's own surface for a partial source.
    pub amount: u8,
    /// Sources never drain; flowing and falling cells last while something feeds them.
    pub source: bool,
    /// Falling: fed from above, it fills the cell and pours down.
    pub falling: bool,
    /// Pinned river water's current (always a source).
    pub current: Option<Current>,
}

impl FluidState {
    pub const fn source(fluid: FluidId) -> Self {
        Self { fluid, amount: 8, source: true, falling: false, current: None }
    }

    pub const fn flowing(fluid: FluidId, amount: u8) -> Self {
        Self { fluid, amount, source: false, falling: false, current: None }
    }

    pub const fn falling(fluid: FluidId) -> Self {
        Self { fluid, amount: 8, source: false, falling: true, current: None }
    }

    /// Height of the surface in the cell, 0..=1: `amount` eighths.
    pub fn height(self) -> f32 {
        self.amount as f32 / 8.0
    }
}

/// A fluid as the registry compiled it: its definition and the blocks of its states.
#[derive(Clone, Debug)]
pub struct Fluid {
    pub id: FluidId,
    /// The source block's name.
    pub name: String,
    pub def: FluidDef,
    pub source: BlockId,
    pub meets: Vec<Meeting>,
    /// The block it crusts into, and after how many ticks at rest.
    pub crust: Option<(BlockId, Tick)>,
}

/// A [`MeetingDef`] with its blocks looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meeting {
    pub fluid: FluidId,
    pub source: BlockId,
    pub flowing: BlockId,
    pub below: BlockId,
}

/// Offsets of the states from the source block, in the order `state_defs` makes them.
const FLOWING: u16 = 1;
const FALLING: u16 = 8;
const TOPS: u16 = 9;
const PINNED: u16 = 16;
const PINNED_COUNT: u16 = Current::DIRECTIONS as u16 * Current::MAX_SPEED as u16 * 8;

/// The source block's definition followed by one for each of its other states, which draw
/// like the source but drop nothing.
pub(crate) fn state_defs(source: &BlockDef) -> Vec<BlockDef> {
    let currents = source.fluid.as_ref().is_some_and(|f| f.currents);
    let state = |suffix: String| BlockDef {
        name: format!("{}_{suffix}", source.name),
        drops: Some(BlockRegistry::AIR.into()),
        icon: None,
        fluid: None,
        ..source.clone()
    };
    let mut defs = vec![source.clone()];
    defs.extend((1..=7).map(|level| state(format!("flow_{level}"))));
    defs.push(state("falling".into()));
    if currents {
        defs.extend((1..=7).map(|amount| state(format!("top_{amount}"))));
        for direction in 0..Current::DIRECTIONS {
            for speed in 1..=Current::MAX_SPEED {
                defs.extend((1..=8).map(|amount| state(format!("current_{direction}_{speed}_{amount}"))));
            }
        }
    }
    defs
}

impl Fluid {
    /// Compiles fluid `id`, whose source is block `source` defined by `def`. `sources`
    /// holds every fluid's source block, by fluid id.
    pub(crate) fn compile(
        id: FluidId,
        source: BlockId,
        def: &BlockDef,
        by_name: &FxHashMap<String, BlockId>,
        sources: &[BlockId],
    ) -> Result<Self, RegistryError> {
        let fluid = def.fluid.clone().expect("only fluid blocks are compiled");
        let error = |message: String| RegistryError(format!("fluid `{}`: {message}", def.name));
        if fluid.tick == 0 || fluid.drop == 0 || fluid.drop > 7 || fluid.slope > MAX_SLOPE {
            return Err(error(format!("needs tick ≥ 1, drop 1..=7 and slope ≤ {MAX_SLOPE}")));
        }
        let block = |name: &str| by_name.get(name).copied().ok_or_else(|| error(format!("unknown block `{name}`")));
        let mut meets = Vec::new();
        for meeting in &fluid.meets {
            let other = block(&meeting.fluid)?;
            let other = sources.iter().position(|&s| s == other).ok_or_else(|| error(format!("`{}` is not a fluid", meeting.fluid)))?;
            meets.push(Meeting {
                fluid: FluidId(other as u8),
                source: block(&meeting.source)?,
                flowing: block(&meeting.flowing)?,
                below: block(&meeting.below)?,
            });
        }
        let crust = match &fluid.crust {
            Some(crust) => Some((block(&crust.block)?, crust.after.max(1) as Tick)),
            None => None,
        };
        Ok(Self { id, name: def.name.clone(), def: fluid, source, meets, crust })
    }

    /// Every state's block with its state.
    pub fn states(&self) -> Vec<(BlockId, FluidState)> {
        let id = self.id;
        let mut states = vec![(self.source, FluidState::source(id))];
        states.extend((1..=7).map(|amount| (self.flowing(amount), FluidState::flowing(id, amount))));
        states.push((self.falling(), FluidState::falling(id)));
        if self.def.currents {
            states.extend((1..8).map(|amount| (self.block(FluidState { amount, ..FluidState::source(id) }), FluidState { amount, ..FluidState::source(id) })));
            for direction in 0..Current::DIRECTIONS {
                for speed in 1..=Current::MAX_SPEED {
                    for amount in 1..=8 {
                        let state = FluidState { amount, current: Some(Current { direction, speed }), ..FluidState::source(id) };
                        states.push((self.block(state), state));
                    }
                }
            }
        }
        states
    }

    /// A flowing cell holding `amount` eighths, 1..=7.
    pub fn flowing(&self, amount: u8) -> BlockId {
        debug_assert!((1..=7).contains(&amount));
        BlockId(self.source.0 + FLOWING + 7 - amount as u16)
    }

    pub fn falling(&self) -> BlockId {
        BlockId(self.source.0 + FALLING)
    }

    /// The block of a state. A fluid without `currents` has no partial or pinned sources;
    /// those become its plain source.
    pub fn block(&self, state: FluidState) -> BlockId {
        let amount = state.amount.clamp(1, 8);
        if state.falling {
            self.falling()
        } else if !state.source {
            self.flowing(amount.min(7))
        } else if !self.def.currents {
            self.source
        } else if let Some(current) = state.current {
            let speed = current.speed.clamp(1, Current::MAX_SPEED) as u16;
            let index = ((current.direction % Current::DIRECTIONS) as u16 * Current::MAX_SPEED as u16 + speed - 1) * 8 + amount as u16 - 1;
            debug_assert!(index < PINNED_COUNT);
            BlockId(self.source.0 + PINNED + index)
        } else if amount == 8 {
            self.source
        } else {
            BlockId(self.source.0 + TOPS + amount as u16 - 1)
        }
    }
}

const SIDES: [IVec3; 4] = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z];
/// Farthest `slope` a fluid may have.
const MAX_SLOPE: u8 = 7;
/// Most flowing cells a hollow may hold and still fill into a pool.
const MAX_HOLLOW: usize = 256;

// ---------------------------------------------------------------------------------------
// The shape of the surface, for meshing, and the flow, for meshing and physics.

/// Heights of a fluid's surface at the four top corners of the cell at `pos`, 0..=1, in
/// the order (−x, −z), (+x, −z), (+x, +z), (−x, +z). `block` reads the world (`None`
/// counts as solid). Each corner is 1 where a cell beside it has the fluid above it, and
/// otherwise the mean over the four cells around the corner: the fluid's cells by their
/// height (cells at least 0.8 high weigh ten times as much), open cells (air, plants) as
/// 0, and solid cells and other fluids left out. Minecraft's rule, so shores dip and
/// flows slope smoothly.
pub fn corner_heights(registry: &BlockRegistry, block: impl Fn(IVec3) -> Option<BlockId>, pos: IVec3) -> [f32; 4] {
    let Some(fluid) = block(pos).and_then(|b| registry.fluid(b)).map(|s| s.fluid) else { return [0.0; 4] };
    let same = |p: IVec3| block(p).and_then(|b| registry.fluid(b)).filter(|s| s.fluid == fluid);
    // −1 leaves a cell out.
    let height = |p: IVec3| match block(p) {
        Some(b) => match registry.fluid(b) {
            Some(s) if s.fluid == fluid => {
                if same(p + IVec3::Y).is_some() {
                    1.0
                } else {
                    s.height()
                }
            }
            Some(_) => -1.0,
            None if registry.info(b).solid => -1.0,
            None => 0.0,
        },
        None => -1.0,
    };
    let own = height(pos);
    [(-1, -1), (1, -1), (1, 1), (-1, 1)].map(|(dx, dz)| {
        let (x, z) = (IVec3::new(dx, 0, 0), IVec3::new(0, 0, dz));
        let (a, b) = (height(pos + x), height(pos + z));
        if a >= 1.0 || b >= 1.0 {
            return 1.0;
        }
        let (mut sum, mut weight) = (0.0, 0.0);
        let mut add = |h: f32| {
            if h >= 0.8 {
                sum += h * 10.0;
                weight += 10.0;
            } else if h >= 0.0 {
                sum += h;
                weight += 1.0;
            }
        };
        if a > 0.0 || b > 0.0 {
            let diagonal = height(pos + x + z);
            if diagonal >= 1.0 {
                return 1.0;
            }
            add(diagonal);
        }
        add(own);
        add(a);
        add(b);
        if weight > 0.0 { sum / weight } else { own.max(0.0) }
    })
}

/// How the fluid at `pos` flows, in blocks per second along (x, z): a pinned cell's
/// current, otherwise [`FLOW_SPEED`] down the slope of the surface (towards lower
/// neighbours and over edges), and nothing on still water. `block` reads the world
/// (`None` counts as solid). What scrolls the texture is what pushes a swimmer.
pub fn flow(registry: &BlockRegistry, block: impl Fn(IVec3) -> Option<BlockId>, pos: IVec3) -> Vec2 {
    let Some(state) = block(pos).and_then(|b| registry.fluid(b)) else { return Vec2::ZERO };
    if let Some(current) = state.current {
        return current.velocity();
    }
    let own = state.height();
    let mut slope = Vec2::ZERO;
    for side in SIDES {
        let Some(b) = block(pos + side) else { continue };
        let diff = match registry.fluid(b) {
            Some(s) if s.fluid == state.fluid => own - s.height(),
            Some(_) => continue,
            None if registry.info(b).solid => continue,
            // Over an edge, towards the fluid below it.
            None => match block(pos + side - IVec3::Y).and_then(|b| registry.fluid(b)) {
                Some(s) if s.fluid == state.fluid => own - (s.height() - 1.0),
                _ => continue,
            },
        };
        slope += Vec2::new(side.x as f32, side.z as f32) * diff;
    }
    slope.normalize_or_zero() * FLOW_SPEED
}

// ---------------------------------------------------------------------------------------
// The kernel.

/// Cells waiting for their tick, and the cells waiting for a column to load.
#[derive(Default)]
pub struct Fluids {
    /// When each scheduled cell is due; `READY` once it is in `ready`.
    due: FxHashMap<IVec3, Tick>,
    /// Cells by the tick they are due at, in the order they were scheduled. Entries whose
    /// cell has since been scheduled sooner are skipped.
    buckets: BTreeMap<Tick, Vec<IVec3>>,
    /// Cells due now, or overdue when a tick's budget ran out, in order.
    ready: VecDeque<IVec3>,
    /// Cells whose update needs an unloaded column, by that column.
    waiting: FxHashMap<ColumnPos, Vec<IVec3>>,
    /// When the resting edges of crusting flows (lava) harden.
    cooling: FxHashMap<IVec3, Tick>,
    writes: Vec<(IVec3, BlockId)>,
}

/// Marks a cell in `ready`: anything scheduled for it later is already covered.
const READY: Tick = 0;

/// What one step did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepStats {
    /// Cells updated.
    pub updates: usize,
    /// Blocks changed.
    pub changes: usize,
}

impl Fluids {
    /// Cells scheduled or due, waiting for a tick.
    pub fn pending(&self) -> usize {
        self.due.len()
    }

    /// Cells parked until a column loads.
    pub fn waiting(&self) -> usize {
        self.waiting.values().map(Vec::len).sum()
    }

    /// A block changed at `pos` (placed, broken, or anything else that is not the kernel's
    /// own doing): wakes the fluid there and beside it.
    pub fn changed(&mut self, registry: &BlockRegistry, world: &VoxelWorld, pos: IVec3, now: Tick) {
        self.wake(registry, world, pos, now);
    }

    /// A column was loaded. Cells that waited for it are scheduled, and when it comes from
    /// a save (`saved`), so are the flowing and falling cells it holds, which may have been
    /// moving when it was saved. Generated columns hold only settled water and need nothing.
    /// Returns how many cells were scheduled.
    pub fn column_loaded(&mut self, registry: &BlockRegistry, world: &VoxelWorld, pos: ColumnPos, now: Tick, saved: bool) -> usize {
        let mut scheduled = 0;
        for cell in self.waiting.remove(&pos).unwrap_or_default() {
            scheduled += self.schedule(cell, now + 1) as usize;
        }
        let Some(column) = world.column(pos).filter(|_| saved) else { return scheduled };
        let moving = |b: BlockId| registry.fluid(b).is_some_and(|s| !s.source);
        for (cy, chunk) in column.chunks.iter().enumerate() {
            if !chunk.palette().iter().any(|&b| moving(b)) {
                continue;
            }
            for i in 0..CHUNK_VOLUME {
                if let Some(state) = registry.fluid(chunk.get(i)).filter(|s| !s.source) {
                    let local = IVec3::new(i as i32 & (CHUNK_SIZE - 1), (i as i32 >> (2 * CHUNK_BITS)) + cy as i32 * CHUNK_SIZE, (i as i32 >> CHUNK_BITS) & (CHUNK_SIZE - 1));
                    let cell = pos.origin() + local;
                    let tick = registry.fluid_def(state.fluid).def.tick;
                    scheduled += self.schedule(cell, now + tick as Tick + jitter(cell, now, tick)) as usize;
                }
            }
        }
        scheduled
    }

    /// A column was unloaded: forgets the timers of its cooling lava. Its scheduled cells
    /// wait for it to load again.
    pub fn column_unloaded(&mut self, pos: ColumnPos) {
        self.cooling.retain(|&cell, _| ColumnPos::of_block(cell) != pos);
    }

    /// Runs up to `budget` cell updates due by tick `now`, writing their changes into the
    /// world and appending them to `changes`.
    pub fn step(&mut self, registry: &BlockRegistry, world: &mut VoxelWorld, now: Tick, budget: usize, changes: &mut Vec<(IVec3, BlockId)>) -> StepStats {
        while let Some(entry) = self.buckets.first_entry()
            && *entry.key() <= now
        {
            let (at, cells) = entry.remove_entry();
            for cell in cells {
                if let Some(due) = self.due.get_mut(&cell)
                    && *due == at
                {
                    *due = READY;
                    self.ready.push_back(cell);
                }
            }
        }
        let mut stats = StepStats::default();
        let before = changes.len();
        while stats.updates < budget
            && let Some(cell) = self.ready.pop_front()
        {
            self.due.remove(&cell);
            self.update(registry, world, cell, now, changes);
            stats.updates += 1;
        }
        stats.changes = changes.len() - before;
        stats
    }

    /// Ticks one cell: works out its writes from the world as it is, then applies them
    /// and wakes what they touch.
    fn update(&mut self, registry: &BlockRegistry, world: &mut VoxelWorld, cell: IVec3, now: Tick, changes: &mut Vec<(IVec3, BlockId)>) {
        let mut planner = Planner::new(registry, world, std::mem::take(&mut self.writes));
        planner.tick(cell);
        let (mut writes, missing) = planner.finish();
        if let Some(column) = missing {
            self.waiting.entry(column).or_default().push(cell);
        } else {
            if writes.is_empty() {
                self.cool(registry, world, cell, now, &mut writes);
            } else {
                self.cooling.remove(&cell);
            }
            for &(pos, block) in &writes {
                if world.set_block(pos, block).is_some() {
                    changes.push((pos, block));
                }
            }
            for &(pos, _) in &writes {
                self.wake(registry, world, pos, now);
            }
        }
        writes.clear();
        self.writes = writes;
    }

    /// A cell that changed nothing: if it is the resting edge of a crusting flow (too thin
    /// to spread further), it hardens once it has rested long enough.
    fn cool(&mut self, registry: &BlockRegistry, world: &VoxelWorld, cell: IVec3, now: Tick, writes: &mut Vec<(IVec3, BlockId)>) {
        let state = world.block(cell).and_then(|b| registry.fluid(b));
        let crust = state.and_then(|s| {
            let fluid = registry.fluid_def(s.fluid);
            fluid.crust.filter(|_| !s.source && !s.falling && s.amount <= fluid.def.drop)
        });
        let Some((block, after)) = crust else {
            self.cooling.remove(&cell);
            return;
        };
        match self.cooling.get(&cell) {
            Some(&at) if now >= at => {
                self.cooling.remove(&cell);
                writes.push((cell, block));
            }
            Some(&at) => {
                self.schedule(cell, at);
            }
            None => {
                self.cooling.insert(cell, now + after);
                self.schedule(cell, now + after);
            }
        }
    }

    /// Schedules the fluid at `pos` and beside it.
    fn wake(&mut self, registry: &BlockRegistry, world: &VoxelWorld, pos: IVec3, now: Tick) {
        for cell in [pos, pos + IVec3::X, pos - IVec3::X, pos + IVec3::Y, pos - IVec3::Y, pos + IVec3::Z, pos - IVec3::Z] {
            let Some(state) = world.block(cell).and_then(|b| registry.fluid(b)) else { continue };
            let fluid = registry.fluid_def(state.fluid);
            // Fluids meeting turn solid at once; anything else waits for its next step.
            let meets = fluid.meets.iter().any(|m| {
                [IVec3::Y, IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
                    .iter()
                    .any(|&d| world.block(cell + d).and_then(|b| registry.fluid(b)).is_some_and(|s| s.fluid == m.fluid))
            });
            let delay = if meets { 1 } else { fluid.def.tick as Tick + jitter(cell, now, fluid.def.tick) };
            self.schedule(cell, now + delay);
        }
    }

    /// Schedules `cell` at tick `at` unless it is already due by then. Returns whether it
    /// was scheduled.
    fn schedule(&mut self, cell: IVec3, at: Tick) -> bool {
        let at = at.max(READY + 1);
        match self.due.get_mut(&cell) {
            Some(due) if *due <= at => return false,
            Some(due) => *due = at,
            None => {
                self.due.insert(cell, at);
            }
        }
        self.buckets.entry(at).or_default().push(cell);
        true
    }
}

/// A few ticks, hashed from the cell and the time, added to a step of `tick` ticks (up to a
/// quarter of it) so neighbouring cells drift apart instead of moving in lockstep.
fn jitter(cell: IVec3, now: Tick, tick: u8) -> Tick {
    let span = tick as u64 / 4;
    if span == 0 {
        return 0;
    }
    let mut h = (cell.x as u32 as u64) ^ ((cell.y as u32 as u64) << 21) ^ ((cell.z as u32 as u64) << 42) ^ now.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h % (span + 1)
}

/// Every write a tick of each fluid cell in `column` would make, by cell: empty when the
/// column's water is a fixed point of the rule, as generated water must be. Cells whose
/// update needs a column that is not loaded are left out.
pub fn unsettled(registry: &BlockRegistry, world: &VoxelWorld, column: ColumnPos) -> Vec<(IVec3, Vec<(IVec3, BlockId)>)> {
    let Some(data) = world.column(column) else { return Vec::new() };
    let mut found = Vec::new();
    for (cy, chunk) in data.chunks.iter().enumerate() {
        if !chunk.palette().iter().any(|&b| registry.fluid(b).is_some()) {
            continue;
        }
        for i in 0..CHUNK_VOLUME {
            if registry.fluid(chunk.get(i)).is_none() {
                continue;
            }
            let local = IVec3::new(i as i32 & (CHUNK_SIZE - 1), (i as i32 >> (2 * CHUNK_BITS)) + cy as i32 * CHUNK_SIZE, (i as i32 >> CHUNK_BITS) & (CHUNK_SIZE - 1));
            let cell = column.origin() + local;
            let mut planner = Planner::new(registry, world, Vec::new());
            planner.tick(cell);
            if let (writes, None) = planner.finish()
                && !writes.is_empty()
            {
                found.push((cell, writes));
            }
        }
    }
    found
}

/// What the rule sees in a cell.
#[derive(Clone, Copy)]
enum Kind {
    /// Fluids cannot enter: solid blocks, the void below the world, unloaded columns.
    Solid,
    /// Fluids flow in and replace it: air and plants.
    Open,
    Fluid(FluidState),
}

/// Works out one cell's update from the world as it stands plus the writes planned so far.
struct Planner<'a> {
    registry: &'a BlockRegistry,
    world: &'a VoxelWorld,
    /// The column read last, which most reads hit again.
    last: Cell<Option<(ColumnPos, &'a Column)>>,
    /// A column the update needed but is not loaded.
    missing: Cell<Option<ColumnPos>>,
    writes: Vec<(IVec3, BlockId)>,
}

impl<'a> Planner<'a> {
    fn new(registry: &'a BlockRegistry, world: &'a VoxelWorld, writes: Vec<(IVec3, BlockId)>) -> Self {
        Self { registry, world, last: Cell::new(None), missing: Cell::new(None), writes }
    }

    fn finish(self) -> (Vec<(IVec3, BlockId)>, Option<ColumnPos>) {
        (self.writes, self.missing.get())
    }

    /// The block at `pos`, `None` below the world or in an unloaded column.
    fn block(&self, pos: IVec3) -> Option<BlockId> {
        if let Some(&(_, block)) = self.writes.iter().rev().find(|(p, _)| *p == pos) {
            return Some(block);
        }
        if pos.y >= WORLD_HEIGHT {
            return Some(BlockId::AIR);
        }
        if pos.y < 0 {
            return None;
        }
        let at = ColumnPos::of_block(pos);
        let column = match self.last.get() {
            Some((c, column)) if c == at => column,
            _ => {
                let Some(column) = self.world.column(at) else {
                    self.missing.set(Some(at));
                    return None;
                };
                let column: &'a Column = column;
                self.last.set(Some((at, column)));
                column
            }
        };
        Some(column.block(IVec3::new(pos.x & (CHUNK_SIZE - 1), pos.y, pos.z & (CHUNK_SIZE - 1))))
    }

    fn kind(&self, pos: IVec3) -> Kind {
        let Some(block) = self.block(pos) else { return Kind::Solid };
        let info = self.registry.info(block);
        match info.fluid {
            Some(state) => Kind::Fluid(state),
            None if info.replaceable => Kind::Open,
            None => Kind::Solid,
        }
    }

    /// The state at `pos` if it holds fluid `id`.
    fn fluid(&self, pos: IVec3, id: FluidId) -> Option<FluidState> {
        match self.kind(pos) {
            Kind::Fluid(state) if state.fluid == id => Some(state),
            _ => None,
        }
    }

    /// Plans a write, taking any plant standing on a plant the fluid washes away with it.
    fn write(&mut self, pos: IVec3, block: BlockId) {
        let washed = self.block(pos).is_some_and(|b| self.registry.info(b).needs_support);
        self.writes.push((pos, block));
        if washed {
            let mut above = pos + IVec3::Y;
            while self.block(above).is_some_and(|b| self.registry.info(b).needs_support) {
                self.writes.push((above, BlockId::AIR));
                above += IVec3::Y;
            }
        }
    }

    fn tick(&mut self, pos: IVec3) {
        let Kind::Fluid(mut state) = self.kind(pos) else { return };
        let fluid = self.registry.fluid_def(state.fluid);
        for meeting in &fluid.meets {
            let touches = [IVec3::Y, IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
                .iter()
                .any(|&d| self.fluid(pos + d, meeting.fluid).is_some());
            if touches {
                self.write(pos, if state.source { meeting.source } else { meeting.flowing });
                return;
            }
        }
        if !state.source {
            match self.new_state(fluid, pos, Some(state)) {
                None => {
                    // Dried up; a crusting fluid leaves its crust where it lay on ground.
                    let crust = fluid.crust.filter(|_| matches!(self.kind(pos - IVec3::Y), Kind::Solid));
                    self.write(pos, crust.map_or(BlockId::AIR, |(crust, _)| crust));
                    return;
                }
                Some(new) if new != state => {
                    self.write(pos, fluid.block(new));
                    state = new;
                }
                Some(_) => {}
            }
        }
        self.spread(fluid, pos, state);
    }

    /// What a cell that is not a source would hold, judged by its neighbours, or `None` if
    /// nothing feeds it. `own` is what it holds now, if anything.
    fn new_state(&self, fluid: &Fluid, pos: IVec3, own: Option<FluidState>) -> Option<FluidState> {
        let id = fluid.id;
        let (mut most, mut sources, mut pinned, mut pinned_amount) = (0, 0, 0u32, 0u32);
        let (mut open, mut lowest) = (false, u8::MAX);
        let mut current = Vec2::ZERO;
        for side in SIDES {
            let state = match self.kind(pos + side) {
                Kind::Fluid(state) if state.fluid == id => state,
                Kind::Open => {
                    open = true;
                    continue;
                }
                _ => continue,
            };
            most = most.max(state.amount);
            lowest = lowest.min(state.amount);
            if state.source {
                sources += 1;
                if let Some(c) = state.current {
                    pinned += 1;
                    pinned_amount += state.amount as u32;
                    current += c.direction_vector() * c.speed as f32;
                }
            }
        }
        let grounded = || matches!(self.kind(pos - IVec3::Y), Kind::Solid) || self.fluid(pos - IVec3::Y, id).is_some_and(|s| s.source);
        if fluid.def.currents && pinned >= 2 && grounded() {
            let amount = (pinned_amount as f32 / pinned as f32).round() as u8;
            return Some(FluidState { amount, current: Current::from_vector(current / pinned as f32), ..FluidState::source(id) });
        }
        if fluid.def.infinite && sources >= 2 && grounded() {
            return Some(FluidState::source(id));
        }
        if self.fluid(pos + IVec3::Y, id).is_some() {
            return Some(FluidState::falling(id));
        }
        // Water that has filled a hollow it cannot run out of stays as a pool. Only a cell
        // that nothing beside it is lower than can be the last to fill, so only that looks.
        if fluid.def.pools
            && let Some(own) = own
            && !open
            && lowest >= own.amount
            && grounded()
            && self.enclosed(id, pos)
        {
            return Some(FluidState::source(id));
        }
        let amount = most.saturating_sub(fluid.def.drop);
        (amount > 0).then(|| FluidState::flowing(id, amount))
    }

    /// Whether the flowing water connected to `pos` at its height has filled a hollow: none
    /// of it can run anywhere, as nothing beside it is open and it all rests on ground or
    /// on sources. Gives up (not enclosed) past `MAX_HOLLOW` cells.
    fn enclosed(&self, id: FluidId, pos: IVec3) -> bool {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut queue = vec![pos];
        seen.insert(pos);
        while let Some(cell) = queue.pop() {
            let resting = matches!(self.kind(cell - IVec3::Y), Kind::Solid) || self.fluid(cell - IVec3::Y, id).is_some_and(|s| s.source);
            if !resting {
                return false;
            }
            for side in SIDES {
                let next = cell + side;
                match self.kind(next) {
                    Kind::Open => return false,
                    Kind::Fluid(s) if s.fluid == id && !s.source && seen.insert(next) => {
                        if seen.len() > MAX_HOLLOW {
                            return false;
                        }
                        queue.push(next);
                    }
                    _ => {}
                }
            }
        }
        true
    }

    fn spread(&mut self, fluid: &Fluid, pos: IVec3, state: FluidState) {
        let below = pos - IVec3::Y;
        match self.kind(below) {
            Kind::Open => {
                if let Some(new) = self.new_state(fluid, below, None) {
                    self.write(below, fluid.block(new));
                }
                // A source over a hole also feeds the pool around it.
                if state.source && SIDES.iter().filter(|&&s| self.fluid(pos + s, fluid.id).is_some_and(|s| s.source)).count() >= 3 {
                    self.spread_sides(fluid, pos, state);
                }
                return;
            }
            Kind::Fluid(other) if other.fluid != fluid.id => {
                if let Some(meeting) = fluid.meets.iter().find(|m| m.fluid == other.fluid) {
                    self.write(below, meeting.below);
                    return;
                }
            }
            _ => {}
        }
        // Flowing water over more of itself stays a column; sources and water on ground spread.
        let over_itself = self.fluid(below, fluid.id).is_some();
        if state.source || !over_itself {
            self.spread_sides(fluid, pos, state);
        }
    }

    fn spread_sides(&mut self, fluid: &Fluid, pos: IVec3, state: FluidState) {
        let amount = if state.falling { 8 } else { state.amount };
        if amount <= fluid.def.drop {
            return;
        }
        let ways = self.ways_down(fluid, pos);
        for (i, side) in SIDES.into_iter().enumerate() {
            let next = pos + side;
            if ways & (1 << i) != 0
                && matches!(self.kind(next), Kind::Open)
                && let Some(new) = self.new_state(fluid, next, None)
            {
                self.write(next, fluid.block(new));
            }
        }
    }

    /// Which sides (bits in `SIDES` order) lead soonest to a way down within the fluid's
    /// `slope`, through open cells and its own flowing cells; every side it can flow
    /// through when none does. A breadth-first search over a small square around `pos`,
    /// carrying for each cell the first steps of the shortest paths to it.
    fn ways_down(&self, fluid: &Fluid, pos: IVec3) -> u8 {
        const R: i32 = MAX_SLOPE as i32 + 1;
        const W: usize = (2 * R + 1) as usize;
        let id = fluid.id;
        let passable = |p: IVec3| match self.kind(p) {
            Kind::Open => true,
            Kind::Fluid(s) => s.fluid == id && !s.source,
            Kind::Solid => false,
        };
        let hole = |p: IVec3| match self.kind(p - IVec3::Y) {
            Kind::Open => true,
            Kind::Fluid(s) => s.fluid == id,
            Kind::Solid => false,
        };
        let index = |p: IVec3| ((p.z - pos.z + R) as usize) * W + (p.x - pos.x + R) as usize;
        // First steps reaching each cell, and the layer it was reached in (0 = unseen).
        let mut steps = [0u8; W * W];
        let mut layer = [0u8; W * W];
        layer[index(pos)] = u8::MAX;
        let (mut frontier, mut next) = (Vec::with_capacity(16), Vec::with_capacity(16));
        let (mut open, mut found) = (0u8, 0u8);
        for (i, side) in SIDES.into_iter().enumerate() {
            let p = pos + side;
            if passable(p) {
                open |= 1 << i;
                steps[index(p)] = 1 << i;
                layer[index(p)] = 1;
                if hole(p) {
                    found |= 1 << i;
                }
                frontier.push(p);
            }
        }
        for depth in 2..=fluid.def.slope + 1 {
            if found != 0 || frontier.is_empty() {
                break;
            }
            for &p in &frontier {
                let from = steps[index(p)];
                for side in SIDES {
                    let q = p + side;
                    let k = index(q);
                    if layer[k] == depth {
                        steps[k] |= from;
                    } else if layer[k] == 0 && passable(q) {
                        layer[k] = depth;
                        steps[k] = from;
                        next.push(q);
                    }
                }
            }
            for &q in &next {
                if hole(q) {
                    found |= steps[index(q)];
                }
            }
            std::mem::swap(&mut frontier, &mut next);
            next.clear();
        }
        if found != 0 { found } else { open }
    }
}

/// How many fluid updates a tick may run. It aims at a few milliseconds of fluid work per
/// tick, measuring what an update costs, and halves that while the server is busy, so a
/// flood slows the water rather than the server.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    updates: usize,
    /// Seconds per update, averaged.
    cost: f64,
}

impl Budget {
    /// Seconds of fluid work a tick aims for.
    pub const TARGET: f64 = 0.004;
    /// Updates a tick always allows, and at most.
    pub const MIN: usize = 256;
    pub const MAX: usize = 1 << 17;

    pub fn updates(&self) -> usize {
        self.updates
    }

    /// Records a step that ran `updates` in `seconds`, while the whole server tick used
    /// `busy` of its time (1 = all of it).
    pub fn record(&mut self, updates: usize, seconds: f64, busy: f64) {
        if updates >= 32 {
            self.cost = self.cost * 0.75 + seconds / updates as f64 * 0.25;
        }
        let target = if busy > 0.8 { Self::TARGET * 0.5 } else { Self::TARGET };
        self.updates = ((target / self.cost.max(1e-9)) as usize).clamp(Self::MIN, Self::MAX);
    }
}

impl Default for Budget {
    fn default() -> Self {
        let cost = 2e-6;
        Self { updates: (Self::TARGET / cost) as usize, cost }
    }
}

#[cfg(test)]
mod tests;
