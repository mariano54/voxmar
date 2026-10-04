use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::FxHasher;

use super::*;
use crate::chunk::Chunk;

const BLOCKS: &str = r#"(
    blocks: [
        (name: "stone"),
        (name: "cobblestone"),
        (name: "basalt"),
        (name: "grass", render: Plant, solid: false, hardness: 0.0),
        (name: "tall_grass", render: Plant, solid: false, hardness: 0.0),
        (
            name: "water", render: Translucent, solid: false, hardness: -1.0, tags: ["natural"],
            fluid: (tick: 5, drop: 1, slope: 4, infinite: true, pools: true, currents: true),
        ),
        (
            name: "lava", solid: false, hardness: -1.0, light: 15,
            fluid: (
                tick: 30, drop: 2, slope: 2, crust: (block: "basalt", after: 100),
                meets: [(fluid: "water", source: "stone", flowing: "cobblestone", below: "stone")],
            ),
        ),
    ],
)"#;

/// Ground is stone up to and including this height.
const GROUND: i32 = 31;

struct Sim {
    registry: Arc<BlockRegistry>,
    world: VoxelWorld,
    fluids: Fluids,
    tick: Tick,
    budget: usize,
    /// Every change the kernel made, in order.
    log: Vec<(IVec3, BlockId)>,
}

impl Sim {
    /// Columns within `radius` of the origin, stone up to `GROUND` and air above.
    fn new(radius: i32) -> Self {
        let registry = Arc::new(BlockRegistry::from_ron("test", BLOCKS).unwrap());
        let stone = registry.expect("test:stone");
        let mut world = VoxelWorld::default();
        for z in -radius..=radius {
            for x in -radius..=radius {
                let mut column = Column::default();
                column.chunks[0] = Chunk::uniform(stone);
                world.insert(ColumnPos::new(x, z), Arc::new(column));
            }
        }
        Self { registry, world, fluids: Fluids::default(), tick: 1, budget: usize::MAX, log: Vec::new() }
    }

    fn id(&self, name: &str) -> BlockId {
        self.registry.expect(&format!("test:{name}"))
    }

    fn water(&self) -> &Fluid {
        &self.registry.fluids()[0]
    }

    fn get(&self, pos: IVec3) -> BlockId {
        self.world.block(pos).unwrap()
    }

    fn state(&self, pos: IVec3) -> Option<FluidState> {
        self.registry.fluid(self.get(pos))
    }

    /// Changes a block as a player would.
    fn set(&mut self, pos: IVec3, block: BlockId) {
        self.world.set_block(pos, block).unwrap();
        self.fluids.changed(&self.registry, &self.world, pos, self.tick);
    }

    /// Changes a block without telling the kernel, as generation does.
    fn place(&mut self, pos: IVec3, block: BlockId) {
        self.world.set_block(pos, block).unwrap();
    }

    fn fill(&mut self, from: IVec3, to: IVec3, block: BlockId) {
        for y in from.y..=to.y {
            for z in from.z..=to.z {
                for x in from.x..=to.x {
                    self.place(IVec3::new(x, y, z), block);
                }
            }
        }
    }

    fn step(&mut self) -> StepStats {
        self.tick += 1;
        self.fluids.step(&self.registry, &mut self.world, self.tick, self.budget, &mut self.log)
    }

    /// Runs until nothing is pending, returning the ticks it took; panics after `limit`.
    fn settle(&mut self, limit: u64) -> u64 {
        let start = self.tick;
        while self.fluids.pending() > 0 {
            assert!(self.tick - start < limit, "still flowing after {limit} ticks ({} pending)", self.fluids.pending());
            self.step();
        }
        self.tick - start
    }

    /// A horizontal slice around the origin, for debugging: `#` solid, `.` air, a digit the
    /// amount of water (`F` falling, `S` source, `L` lava).
    #[allow(dead_code)]
    fn print(&self, y: i32, r: i32) {
        for z in -r..=r {
            let row: String = (-r..=r)
                .map(|x| {
                    let b = self.get(at(x, y, z));
                    match self.registry.fluid(b) {
                        Some(s) if s.fluid != FluidId(0) => 'L',
                        Some(s) if s.falling => 'F',
                        Some(s) if s.source => 'S',
                        Some(s) => (b'0' + s.amount) as char,
                        None if b.is_air() => '.',
                        None => '#',
                    }
                })
                .collect();
            println!("{row}");
        }
    }

    fn hash(&self) -> u64 {
        let mut hasher = FxHasher::default();
        let mut columns: Vec<_> = self.world.columns().collect();
        columns.sort_by_key(|(p, _)| (p.0.x, p.0.y));
        for (pos, column) in columns {
            pos.hash(&mut hasher);
            for chunk in &column.chunks {
                let mut dense = vec![BlockId::AIR; CHUNK_VOLUME];
                chunk.copy_to_dense(&mut dense);
                dense.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

fn at(x: i32, y: i32, z: i32) -> IVec3 {
    IVec3::new(x, GROUND + y, z)
}

#[test]
fn states_are_blocks_after_their_source() {
    let sim = Sim::new(0);
    let registry = &sim.registry;
    let (water, lava) = (&registry.fluids()[0], &registry.fluids()[1]);
    assert_eq!(water.name, "test:water");
    assert_eq!(registry.id("test:water_flow_1"), Some(water.flowing(7)));
    assert_eq!(registry.id("test:water_flow_7"), Some(water.flowing(1)));
    assert_eq!(registry.id("test:water_falling"), Some(water.falling()));
    assert_eq!(registry.id("test:water_top_3"), Some(water.block(FluidState { amount: 3, ..FluidState::source(water.id) })));
    let pinned = FluidState { amount: 5, current: Some(Current { direction: 9, speed: 2 }), ..FluidState::source(water.id) };
    assert_eq!(registry.id("test:water_current_9_2_5"), Some(water.block(pinned)));
    // Every state maps to its block and back.
    for fluid in registry.fluids() {
        for (block, state) in fluid.states() {
            assert_eq!(registry.fluid(block), Some(state), "{}", registry.def(block).name);
            assert_eq!(fluid.block(state), block);
        }
    }
    assert_eq!(water.states().len(), 9 + 7 + 16 * 3 * 8);
    assert_eq!(lava.states().len(), 9);
    // Lava has no river states: they fall back to its source.
    assert_eq!(lava.block(pinned), lava.source);
    // Flowing states draw like their source, drop nothing and are not listed by tag.
    let flow = registry.info(water.flowing(4));
    assert_eq!((flow.render, flow.layers), (registry.info(water.source).render, registry.info(water.source).layers));
    assert!(flow.drops.is_air() && flow.replaceable);
    assert_eq!(registry.tagged("natural").collect::<Vec<_>>(), [water.source]);
    // Lava can be built into, like water.
    assert!(registry.info(lava.source).replaceable && registry.info(lava.falling()).replaceable);
    assert_eq!(lava.meets, [Meeting { fluid: water.id, source: sim.id("stone"), flowing: sim.id("cobblestone"), below: sim.id("stone") }]);
    assert_eq!((lava.crust, water.crust), (Some((sim.id("basalt"), 100)), None));
}

#[test]
fn a_source_spreads_seven_blocks_and_dries_up_without_it() {
    let mut sim = Sim::new(1);
    let source = at(0, 1, 0);
    sim.set(source, sim.water().source);
    let ticks = sim.settle(200);
    // A diamond of levels around the source, one eighth less per block.
    for dz in -9i32..=9 {
        for dx in -9i32..=9 {
            let d = dx.abs() + dz.abs();
            let state = sim.state(at(dx, 1, dz));
            match d {
                0 => assert_eq!(state, Some(FluidState::source(FluidId(0)))),
                1..=7 => assert_eq!(state, Some(FluidState::flowing(FluidId(0), 8 - d as u8)), "at {dx},{dz}"),
                _ => assert!(state.is_none(), "water at {dx},{dz}"),
            }
        }
    }
    // Seven steps of five or six ticks.
    assert!((35..=60).contains(&ticks), "{ticks} ticks");
    sim.set(source, BlockId::AIR);
    sim.settle(200);
    assert!((-8..=8).all(|dz| (-8..=8).all(|dx| sim.state(at(dx, 1, dz)).is_none())), "water is left");
}

#[test]
fn flows_head_for_the_nearest_way_down() {
    let mut sim = Sim::new(1);
    // A platform three blocks high with a hole three blocks east of the source.
    sim.fill(at(-10, 1, -10), at(10, 3, 10), sim.id("stone"));
    sim.place(at(3, 3, 0), BlockId::AIR);
    sim.place(at(3, 2, 0), BlockId::AIR);
    sim.set(at(0, 4, 0), sim.water().source);
    sim.settle(400);
    // Only the way east was taken, and the water falls down the hole.
    assert!(sim.state(at(-1, 4, 0)).is_none() && sim.state(at(0, 4, 1)).is_none());
    assert_eq!(sim.state(at(2, 4, 0)).map(|s| s.amount), Some(6));
    assert!(sim.state(at(3, 4, 0)).is_some());
    assert!(sim.state(at(3, 3, 0)).is_some_and(|s| s.falling));
    assert!(sim.state(at(3, 2, 0)).is_some_and(|s| s.falling));
}

/// As in Minecraft, a source in the air spills over all four sides as well as falling, so
/// it pours as a column three wide, and spreads again where it lands.
#[test]
fn water_falls_and_spreads_where_it_lands() {
    let mut sim = Sim::new(1);
    sim.set(at(0, 10, 0), sim.water().source);
    sim.settle(400);
    assert_eq!(sim.state(at(1, 10, 0)), Some(FluidState::flowing(FluidId(0), 7)));
    for y in 1..10 {
        for p in [at(0, y, 0), at(1, y, 0), at(0, y, -1)] {
            assert_eq!(sim.state(p), Some(FluidState::falling(FluidId(0))), "{p}");
        }
        assert!(sim.state(at(1, y, 1)).is_none_or(|_| y == 1));
    }
    // Landed water spreads like a source, one level down.
    assert_eq!(sim.state(at(2, 1, 0)), Some(FluidState::flowing(FluidId(0), 7)));
    assert_eq!(sim.state(at(8, 1, 0)), Some(FluidState::flowing(FluidId(0), 1)));
    assert!(sim.state(at(9, 1, 0)).is_none());
}

#[test]
fn two_sources_make_a_third() {
    let mut sim = Sim::new(1);
    let water = sim.water().source;
    sim.set(at(0, 1, 0), water);
    sim.set(at(2, 1, 0), water);
    sim.settle(200);
    assert_eq!(sim.state(at(1, 1, 0)), Some(FluidState::source(FluidId(0))));
    // Water taken out of a pool comes back as a source, and a pit dug beneath it fills.
    let mut sim = Sim::new(1);
    sim.fill(at(-3, 1, -3), at(3, 1, 3), sim.water().source);
    sim.set(at(0, 1, 0), BlockId::AIR);
    sim.set(at(2, 0, 2), BlockId::AIR);
    sim.settle(200);
    assert_eq!(sim.state(at(0, 1, 0)), Some(FluidState::source(FluidId(0))));
    assert_eq!(sim.state(at(2, 0, 2)), Some(FluidState::falling(FluidId(0))));
    assert!((-4..=4).all(|z| (-4..=4).all(|x| sim.state(at(x, 1, z)).is_some_and(|s| s.source) == (x.abs() <= 3 && z.abs() <= 3))));
    // Lava is not infinite.
    let mut sim = Sim::new(1);
    let lava = sim.registry.fluids()[1].source;
    sim.set(at(0, 1, 0), lava);
    sim.set(at(2, 1, 0), lava);
    sim.settle(400);
    assert!(sim.state(at(1, 1, 0)).is_some_and(|s| !s.source && s.amount == 6));
}

#[test]
fn water_washes_plants_away() {
    let mut sim = Sim::new(1);
    let (grass, tall) = (sim.id("grass"), sim.id("tall_grass"));
    sim.place(at(1, 1, 0), grass);
    sim.place(at(2, 1, 0), tall);
    sim.place(at(2, 2, 0), tall);
    sim.set(at(0, 1, 0), sim.water().source);
    sim.settle(200);
    assert!(sim.state(at(1, 1, 0)).is_some() && sim.state(at(2, 1, 0)).is_some());
    assert!(sim.get(at(2, 2, 0)).is_air(), "the top of the tall grass goes too");
}

#[test]
fn water_pools_in_hollows() {
    // A stream fills a dead-end ditch, which becomes a pond.
    let mut sim = Sim::new(1);
    let stone = sim.id("stone");
    sim.fill(at(-1, 1, -1), at(8, 1, 1), stone);
    sim.fill(at(0, 1, 0), at(6, 1, 0), BlockId::AIR);
    sim.set(at(0, 1, 0), sim.water().source);
    sim.settle(400);
    assert!((0..=6).all(|x| sim.state(at(x, 1, 0)) == Some(FluidState::source(FluidId(0)))));
    // A hollow a stream pours into keeps its water when the stream stops.
    let mut sim = Sim::new(1);
    sim.fill(at(4, 0, -1), at(6, 0, 1), BlockId::AIR);
    let source = at(0, 1, 0);
    sim.set(source, sim.water().source);
    sim.set(at(4, 0, 0), BlockId::AIR);
    sim.settle(400);
    assert!(sim.state(at(5, 0, 0)).is_some(), "the stream reached the hollow");
    sim.set(source, BlockId::AIR);
    sim.settle(400);
    assert!((-1..=1).all(|z| (4..=6).all(|x| sim.state(at(x, 0, z)) == Some(FluidState::source(FluidId(0))))));
    assert!((-8..=8).all(|z| (-8..=8).all(|x| sim.state(at(x, 1, z)).is_none())), "the stream is gone");
}

#[test]
fn lava_crusts_over() {
    let mut sim = Sim::new(1);
    let (lava, basalt) = (sim.registry.fluids()[1].clone(), sim.id("basalt"));
    let source = at(0, 1, 0);
    sim.set(source, lava.source);
    sim.settle(1000);
    // The thin edge of the flow hardens; the rest stays molten.
    assert_eq!(sim.get(at(3, 1, 0)), basalt);
    assert_eq!(sim.get(at(2, 1, 1)), basalt);
    assert_eq!(sim.state(at(1, 1, 0)), Some(FluidState::flowing(lava.id, 6)));
    assert_eq!(sim.state(at(2, 1, 0)), Some(FluidState::flowing(lava.id, 4)));
    // Without its source the flow dries into rock where it ran.
    sim.set(source, BlockId::AIR);
    sim.settle(1000);
    assert!(sim.get(source).is_air());
    for (x, z) in [(1, 0), (2, 0), (0, -2), (1, 1)] {
        assert_eq!(sim.get(at(x, 1, z)), basalt, "{x},{z}");
    }
    // Lava poured from the air leaves no column of rock behind, only what lay on ground.
    let mut sim = Sim::new(1);
    sim.set(at(0, 6, 0), lava.source);
    sim.settle(1000);
    sim.set(at(0, 6, 0), BlockId::AIR);
    sim.settle(1000);
    assert!((2..6).all(|y| sim.get(at(0, y, 0)).is_air()));
    assert_eq!(sim.get(at(0, 1, 0)), basalt);
}

#[test]
fn lava_and_water_turn_to_stone() {
    let pool = || {
        let mut sim = Sim::new(1);
        sim.set(at(0, 1, 0), sim.water().source);
        sim.settle(200);
        let lava = sim.registry.fluids()[1].source;
        (sim, lava)
    };
    // A lava source poured into water turns to stone.
    let (mut sim, lava) = pool();
    sim.set(at(3, 1, 0), lava);
    sim.settle(1000);
    assert_eq!(sim.get(at(3, 1, 0)), sim.id("stone"));
    // Lava flowing up to water turns to cobblestone.
    let (mut sim, lava) = pool();
    sim.set(at(9, 1, 0), lava);
    sim.settle(1000);
    assert_eq!(sim.get(at(8, 1, 0)), sim.id("cobblestone"));
    assert_eq!(sim.get(at(9, 1, 0)), lava);
    for z in -10..=10 {
        for x in -10..=14 {
            let p = at(x, 1, z);
            if sim.state(p).is_some_and(|s| s.fluid == FluidId(1)) {
                assert!(SIDES.iter().all(|&d| sim.state(p + d).is_none_or(|s| s.fluid == FluidId(1))), "lava beside water at {p}");
            }
        }
    }
    // Lava flowing down into water makes stone of the water.
    let mut sim = Sim::new(1);
    let lava = sim.registry.fluids()[1].source;
    sim.fill(at(-2, 1, -2), at(2, 1, 2), sim.water().source);
    sim.set(at(0, 4, 0), lava);
    sim.settle(1000);
    assert_eq!(sim.get(at(0, 1, 0)), sim.id("stone"));
}

#[test]
fn river_breaches_carry_the_current() {
    let mut sim = Sim::new(1);
    let water = sim.water().clone();
    let pinned = |direction, amount| water.block(FluidState { amount, current: Some(Current { direction, speed: 2 }), ..FluidState::source(water.id) });
    // A gap between two river cells over ground fills with river water and their mean current.
    sim.place(at(0, 1, 0), pinned(0, 8));
    sim.place(at(2, 1, 0), pinned(2, 8));
    sim.place(at(1, 1, 1), sim.id("stone"));
    sim.place(at(1, 1, -1), sim.id("stone"));
    sim.set(at(1, 1, 0), BlockId::AIR);
    sim.settle(200);
    let state = sim.state(at(1, 1, 0)).unwrap();
    assert_eq!(state.current, Some(Current { direction: 1, speed: 2 }));
    assert!(state.source && state.amount == 8);
    // A partial river surface spreads thinner water.
    let mut sim = Sim::new(1);
    sim.set(at(0, 1, 0), pinned(0, 3));
    sim.settle(200);
    assert_eq!(sim.state(at(1, 1, 0)), Some(FluidState::flowing(water.id, 2)));
    assert_eq!(sim.state(at(2, 1, 0)), Some(FluidState::flowing(water.id, 1)));
    assert!(sim.state(at(3, 1, 0)).is_none());
}

#[test]
fn surfaces_and_flow_follow_the_levels() {
    let mut sim = Sim::new(1);
    sim.set(at(0, 1, 0), sim.water().source);
    sim.settle(200);
    let registry = sim.registry.clone();
    let read = |p: IVec3| sim.world.block(p);
    // Downhill is away from the source.
    let east = flow(&registry, read, at(3, 1, 0));
    assert!((east - Vec2::new(FLOW_SPEED, 0.0)).length() < 1e-4, "{east}");
    let diagonal = flow(&registry, read, at(2, 1, 2));
    assert!(diagonal.x > 0.5 && diagonal.y > 0.5, "{diagonal}");
    // The source's corners stand above the flowing water around it.
    assert!(corner_heights(&registry, read, at(0, 1, 0)).iter().all(|&h| h > 0.9 && h < 1.0));
    // A flowing cell's corners slope down away from the source.
    let [a, b, c, d] = corner_heights(&registry, read, at(3, 1, 0));
    assert!(a > b && d > c && (a - d).abs() < 1e-6, "{a} {b} {c} {d}");
    // Still water does not flow; pinned water flows with its current.
    let mut sim = Sim::new(1);
    sim.fill(at(-2, 1, -2), at(2, 1, 2), sim.water().source);
    let water = sim.water().clone();
    assert_eq!(flow(&registry, |p| sim.world.block(p), at(0, 1, 0)), Vec2::ZERO);
    sim.place(at(0, 1, 0), water.block(FluidState { current: Some(Current { direction: 4, speed: 3 }), ..FluidState::source(water.id) }));
    let current = flow(&registry, |p| sim.world.block(p), at(0, 1, 0));
    assert!((current - Vec2::new(0.0, CURRENT_SPEEDS[3])).length() < 1e-4, "{current}");
}

/// A dam holding a reservoir breaks: the water runs out over the plain below and settles
/// within a bound, the same way every time and whatever the budget.
#[test]
fn a_dam_break_settles_deterministically() {
    let run = |budget: usize| {
        let mut sim = Sim::new(1);
        sim.budget = budget;
        // A reservoir 3 deep behind a wall, on a terrace 2 above the plain.
        sim.fill(at(-20, 1, -20), at(20, 2, -5), sim.id("stone"));
        sim.fill(at(-20, 3, -20), at(20, 5, -4), sim.id("stone"));
        sim.fill(at(-6, 3, -12), at(6, 5, -5), sim.water().source);
        for x in -1..=1 {
            for y in 3..=5 {
                sim.set(at(x, y, -4), BlockId::AIR);
            }
        }
        let ticks = sim.settle(2000);
        (ticks, sim.hash(), sim.log.len(), sim)
    };
    let (ticks, hash, changes, sim) = run(usize::MAX);
    assert!(changes > 50, "{changes} changes");
    assert!(ticks < 300, "{ticks} ticks");
    // The water ran over the terrace's edge onto the plain.
    assert!((-3..=3).any(|x| sim.state(at(x, 1, -3)).is_some()));
    assert_eq!(run(usize::MAX).1, hash, "the same every time");
    // With a small budget it takes longer but still settles, and the same way each time.
    let (slow, slow_hash, _, sim) = run(16);
    assert!(slow >= ticks && slow < 2000, "{slow} ticks");
    assert!((-3..=3).any(|x| sim.state(at(x, 1, -3)).is_some()));
    assert_eq!(run(16).1, slow_hash);
}

/// Water as generation leaves it: sealed, so ticking any of it changes nothing, and
/// loading it schedules nothing. A waterfall's face (air beside the upper water, over the
/// pool) is not settled until the curtain the rule would pour is stamped as well.
#[test]
fn generated_water_is_a_fixed_point() {
    let mut sim = Sim::new(1);
    let water = sim.water().clone();
    // A lake in a basin, an upper river ending at a cliff, and its plunge pool.
    sim.fill(at(-12, 1, -12), at(12, 12, 12), sim.id("stone"));
    sim.fill(at(-8, 9, -8), at(-2, 12, -2), water.source);
    sim.fill(at(2, 10, -2), at(10, 12, 0), water.source);
    sim.fill(at(2, 4, 1), at(10, 12, 6), BlockId::AIR);
    sim.fill(at(2, 1, 1), at(10, 3, 6), water.source);
    for pos in [ColumnPos::new(0, 0), ColumnPos::new(-1, -1), ColumnPos::new(0, -1)] {
        assert_eq!(sim.fluids.column_loaded(&sim.registry, &sim.world, pos, sim.tick, false), 0);
    }
    let unsettled = |sim: &Sim| -> Vec<IVec3> {
        let mut cells: Vec<IVec3> = [(-1, -1), (0, -1), (-1, 0), (0, 0)]
            .into_iter()
            .flat_map(|(x, z)| unsettled(&sim.registry, &sim.world, ColumnPos::new(x, z)))
            .map(|(cell, _)| cell)
            .collect();
        cells.sort_by_key(|p| (p.x, p.y, p.z));
        cells
    };
    // Only the face of the fall moves: the upper water along the cliff edge.
    let face = unsettled(&sim);
    assert!(!face.is_empty() && face.iter().all(|p| p.z == 0 && (2..=10).contains(&p.x)), "{face:?}");
    // Stamp the curtain: flowing level 1 beside the top water, falling water below it down
    // to the pool. Then nothing moves.
    for x in 2..=10 {
        sim.place(at(x, 12, 1), water.flowing(7));
        sim.fill(at(x, 4, 1), at(x, 11, 1), water.falling());
    }
    assert_eq!(unsettled(&sim), Vec::<IVec3>::new());
    // And the curtain is exactly what the kernel pours.
    let stamped = sim.hash();
    sim.fill(at(2, 4, 1), at(10, 12, 1), BlockId::AIR);
    for x in 2..=10 {
        sim.fluids.changed(&sim.registry, &sim.world, at(x, 12, 0), sim.tick);
    }
    sim.settle(400);
    assert_eq!(sim.hash(), stamped);
}

#[test]
fn saved_flows_resume_and_unloaded_columns_wait() {
    let mut sim = Sim::new(1);
    // Flowing water saved mid-flow carries on once its column loads.
    let water = sim.water().clone();
    sim.place(at(0, 1, 0), water.source);
    sim.place(at(1, 1, 0), water.flowing(7));
    assert_eq!(sim.fluids.column_loaded(&sim.registry, &sim.world, ColumnPos::new(0, 0), sim.tick, true), 1);
    sim.settle(200);
    assert_eq!(sim.state(at(7, 1, 0)), Some(FluidState::flowing(water.id, 1)));
    // Water reaching the edge of the loaded world waits there for the next column.
    let mut sim = Sim::new(0);
    let edge = CHUNK_SIZE - 3;
    sim.set(at(edge, 1, 16), water.source);
    sim.settle(200);
    assert!(sim.fluids.waiting() > 0);
    assert_eq!(sim.state(at(CHUNK_SIZE - 1, 1, 16)), None, "it stopped short of the edge");
    let mut column = Column::default();
    column.chunks[0] = Chunk::uniform(sim.id("stone"));
    sim.world.insert(ColumnPos::new(1, 0), Arc::new(column));
    sim.fluids.column_loaded(&sim.registry, &sim.world, ColumnPos::new(1, 0), sim.tick, false);
    sim.settle(200);
    assert_eq!(sim.fluids.waiting(), 0);
    assert_eq!(sim.state(at(edge + 7, 1, 16)), Some(FluidState::flowing(water.id, 1)));
}

/// A large flood: a lake 48 blocks across and 4 deep loses its floor over a cavern 60
/// blocks below, and pours in. Reports the ticks and time it takes.
#[test]
fn a_large_flood_is_fast() {
    let mut sim = Sim::new(2);
    let stone = sim.id("stone");
    let water = sim.water().source;
    sim.fill(at(-50, 61, -50), at(50, 70, 50), stone);
    sim.fill(at(-24, 67, -24), at(23, 70, 23), water);
    let started = Instant::now();
    sim.fill(at(-24, 61, -24), at(23, 66, 23), BlockId::AIR);
    for z in -24..24 {
        for x in -24..24 {
            sim.fluids.changed(&sim.registry, &sim.world, at(x, 66, z), sim.tick);
        }
    }
    let (mut ticks, mut updates, mut slowest) = (0, 0, 0.0f64);
    while sim.fluids.pending() > 0 {
        let tick = Instant::now();
        updates += sim.step().updates;
        slowest = slowest.max(tick.elapsed().as_secs_f64());
        ticks += 1;
        assert!(ticks < 3000);
    }
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "flood: {ticks} ticks, {updates} updates, {} changes, {:.1} ms in all, {:.2} µs per update, slowest tick {:.1} ms",
        sim.log.len(),
        seconds * 1e3,
        seconds * 1e6 / updates as f64,
        slowest * 1e3,
    );
    // The cavern floor is wet beneath the whole lake.
    assert!(sim.state(at(0, 1, 0)).is_some() && sim.state(at(-20, 1, 20)).is_some());
}

#[test]
fn budgets_follow_the_cost_of_updates() {
    let mut budget = Budget::default();
    // Cheap updates: up to the most allowed.
    for _ in 0..40 {
        budget.record(budget.updates(), budget.updates() as f64 * 1e-9, 0.2);
    }
    assert_eq!(budget.updates(), Budget::MAX);
    // Expensive ones: as many as fit in the target, and half that when the server is busy.
    for _ in 0..80 {
        budget.record(1000, 1000.0 * 4e-6, 0.2);
    }
    assert!((budget.updates() as f64 - Budget::TARGET / 4e-6).abs() < 20.0, "{}", budget.updates());
    budget.record(1000, 1000.0 * 4e-6, 0.95);
    assert!((budget.updates() as f64 - Budget::TARGET / 8e-6).abs() < 20.0, "{}", budget.updates());
}
