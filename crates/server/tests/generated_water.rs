//! Generated water must not move: loading it schedules no fluid updates, and ticking any of
//! it would change nothing (it is a fixed point of the fluid kernel), so a world full of
//! lakes, rivers, waterfalls and caves costs the kernel nothing until a player changes
//! something.

use std::sync::Arc;

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::fluid::{Fluids, unsettled};
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::VoxelWorld;
use voxmar_server::load_content;
use voxmar_worldgen::{Generator, Overworld, WaterSight, WorldContent};

/// Columns along a side of each generated area; the middle ones have all their neighbours.
const SIDE: i32 = 5;

#[test]
fn generated_water_does_not_move() {
    let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
    let world_content = WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let (mut fluid_cells, mut curtain, mut pinned) = (0, 0, 0);
    // Around every place water makes near spawn on a few seeds (waterfalls, highland
    // lakes, confluences, river mouths, and tunnels, islands and hot springs where there
    // are any), found afresh so the test follows any reseed, and land with caves.
    let mut kinds = std::collections::HashSet::new();
    for (seed, near_spawn, also) in [(3, true, None), (11, true, None), (2, true, None), (5, true, None), (7, true, Some(IVec2::new(1500, 5200))), (0x5eed, false, Some(IVec2::ZERO))] {
        let generator = Overworld::with_content(seed, registry.clone(), &world_content);
        let mut places: Vec<IVec2> = also.into_iter().collect();
        if near_spawn {
            let spawn = generator.spawn_point();
            let sights = generator.water_sights(IVec2::new(spawn.x, spawn.z), 600);
            kinds.extend(sights.iter().map(|&(kind, _)| kind));
            places.extend(sights.into_iter().map(|(_, at)| IVec2::new(at.x, at.z)));
        }
        for place in places {
            let (fluids, falls, rivers) = check_area(&registry, &generator, seed, place);
            (fluid_cells, curtain, pinned) = (fluid_cells + fluids, curtain + falls, pinned + rivers);
        }
    }
    println!("{fluid_cells} fluid cells: {pinned} of rivers with a current, {curtain} pouring down falls; found {kinds:?}");
    for kind in [WaterSight::Waterfall, WaterSight::HighlandLake, WaterSight::Confluence, WaterSight::RiverMouth] {
        assert!(kinds.contains(&kind), "no {} near any spawn", kind.name());
    }
    assert!(fluid_cells > 10_000 && pinned > 500 && curtain > 20);
}

/// Generates the area around `place` and checks that none of its water would move: none is
/// scheduled when its columns load, and ticking any of it would change nothing. Counts its
/// fluid cells as `count_fluid` does.
fn check_area(registry: &BlockRegistry, generator: &Overworld, seed: u64, place: IVec2) -> (usize, usize, usize) {
    let first = ColumnPos::of_block(IVec3::new(place.x, 0, place.y)).offset(-SIDE / 2, -SIDE / 2);
    let world = generate(generator, first);
    let mut fluids = Fluids::default();
    for dz in 0..SIDE {
        for dx in 0..SIDE {
            assert_eq!(fluids.column_loaded(registry, &world, first.offset(dx, dz), 1, false), 0);
        }
    }
    assert_eq!(fluids.pending(), 0);
    let mut counts = (0, 0, 0);
    for dz in 1..SIDE - 1 {
        for dx in 1..SIDE - 1 {
            let pos = first.offset(dx, dz);
            let (all, pouring, rivers) = count_fluid(registry, &world, pos);
            counts = (counts.0 + all, counts.1 + pouring, counts.2 + rivers);
            let moving = unsettled(registry, &world, pos);
            assert!(
                moving.is_empty(),
                "seed {seed}: {} generated fluid cells would change, first {} at {}: {:?}",
                moving.len(),
                registry.def(world.block(moving[0].0).unwrap()).name,
                moving[0].0,
                moving[0].1.iter().map(|&(p, b)| (p, registry.def(b).name.as_str())).collect::<Vec<_>>()
            );
        }
    }
    counts
}

/// Fluid cells in a column: all, those flowing or falling (the curtains of waterfalls),
/// and rivers' pinned water.
fn count_fluid(registry: &BlockRegistry, world: &VoxelWorld, pos: ColumnPos) -> (usize, usize, usize) {
    let origin = pos.origin();
    let (mut all, mut pouring, mut pinned) = (0, 0, 0);
    for y in 0..WORLD_HEIGHT {
        for z in 0..32 {
            for x in 0..32 {
                if let Some(state) = world.block(origin + IVec3::new(x, y, z)).and_then(|b| registry.fluid(b)) {
                    all += 1;
                    pouring += !state.source as usize;
                    pinned += state.current.is_some() as usize;
                }
            }
        }
    }
    (all, pouring, pinned)
}

/// A `SIDE` × `SIDE` area of columns from `first`, generated on all cores.
fn generate(generator: &Overworld, first: ColumnPos) -> VoxelWorld {
    let positions: Vec<ColumnPos> = (0..SIDE * SIDE).map(|i| first.offset(i % SIDE, i / SIDE)).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut world = VoxelWorld::default();
    std::thread::scope(|scope| {
        let handles: Vec<_> = positions
            .chunks(positions.len().div_ceil(threads))
            .map(|part| scope.spawn(move || part.iter().map(|&pos| (pos, generator.generate(pos))).collect::<Vec<_>>()))
            .collect();
        for handle in handles {
            for (pos, column) in handle.join().unwrap() {
                world.insert(pos, Arc::new(column));
            }
        }
    });
    world
}
