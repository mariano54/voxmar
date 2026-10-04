//! Generated water must not move: loading it schedules no fluid updates, and ticking any of
//! it would change nothing (it is a fixed point of the fluid kernel), so a world full of
//! lakes, rivers, waterfalls and caves costs the kernel nothing until a player changes
//! something.

use std::sync::Arc;

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::fluid::{Fluids, unsettled};
use voxmar_engine::glam::IVec3;
use voxmar_engine::world::VoxelWorld;
use voxmar_server::load_content;
use voxmar_worldgen::{Generator, Overworld, WorldContent};

/// Columns along a side of each generated area; the middle ones have all their neighbours.
const SIDE: i32 = 5;

#[test]
fn generated_water_does_not_move() {
    let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
    let world_content = WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let (mut fluid_cells, mut curtain, mut pinned) = (0, 0, 0);
    // Waterfalls (seed 1 at 54, 462, seed 42 at -122, 274 and seed 7 at 112, -302), a
    // highland lake, a confluence, a river mouth, hot springs, and land with caves.
    for (seed, x, z) in [
        (1, 54, 462),
        (1, 58, 290),
        (1, 420, 148),
        (1, -146, 328),
        (42, -122, 274),
        (42, -32, 270),
        (42, -26, 18),
        (7, 112, -302),
        (0x5eed, 0, 0),
        (7, 1500, 5200),
    ] {
        let generator = Overworld::with_content(seed, registry.clone(), &world_content);
        let first = ColumnPos::of_block(IVec3::new(x, 0, z)).offset(-SIDE / 2, -SIDE / 2);
        let world = generate(&generator, first);
        let mut fluids = Fluids::default();
        for dz in 0..SIDE {
            for dx in 0..SIDE {
                let pos = first.offset(dx, dz);
                assert_eq!(fluids.column_loaded(&registry, &world, pos, 1, false), 0);
            }
        }
        assert_eq!(fluids.pending(), 0);
        for dz in 1..SIDE - 1 {
            for dx in 1..SIDE - 1 {
                let pos = first.offset(dx, dz);
                let (fluids, falls, rivers) = count_fluid(&registry, &world, pos);
                (fluid_cells, curtain, pinned) = (fluid_cells + fluids, curtain + falls, pinned + rivers);
                let moving = unsettled(&registry, &world, pos);
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
    }
    println!("{fluid_cells} fluid cells: {pinned} of rivers with a current, {curtain} pouring down falls");
    assert!(fluid_cells > 10_000 && pinned > 500 && curtain > 20);
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
