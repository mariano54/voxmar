//! Generated water must not move: loading it schedules no fluid updates, and ticking any of
//! it would change nothing (it is a fixed point of the fluid kernel), so a world full of
//! lakes, rivers and caves costs the kernel nothing until a player changes something.

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
    let (mut fluid_cells, mut faces) = (0, 0);
    // Creeks, waterfalls (seed 1 at 348, 84 and seed 42 at -124, -502), a highland lake, a
    // confluence, and land with caves.
    for (seed, x, z) in [(1, 10, 88), (1, 348, 84), (1, 218, 96), (42, -124, -502), (42, -306, -70), (0x5eed, 0, 0), (7, 1500, 5200)] {
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
                fluid_cells += count_fluid(&registry, &world, pos);
                for (cell, writes) in unsettled(&registry, &world, pos) {
                    // Until the rivers stamp the curtain of water a fall pours (see
                    // "Generated water" in engine/src/fluid.rs), a waterfall's face pours.
                    assert!(
                        waterfall_face(&registry, &world, cell),
                        "seed {seed}: {} at {cell} would change: {:?}",
                        registry.def(world.block(cell).unwrap()).name,
                        writes.iter().map(|&(p, b)| (p, registry.def(b).name.as_str())).collect::<Vec<_>>()
                    );
                    faces += 1;
                }
            }
        }
    }
    println!("{fluid_cells} fluid cells, {faces} on waterfall faces");
    assert!(fluid_cells > 10_000);
}

/// Water with air beside it over lower water (or ice): the face of a waterfall.
fn waterfall_face(registry: &BlockRegistry, world: &VoxelWorld, cell: IVec3) -> bool {
    let water = registry.expect("base:water");
    world.block(cell) == Some(water)
        && [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].into_iter().any(|side| {
            let beside = cell + side;
            world.block(beside).is_some_and(|b| b.is_air())
                && (0..beside.y)
                    .rev()
                    .filter_map(|y| world.block(beside.with_y(y)))
                    .find(|b| !b.is_air())
                    .is_some_and(|b| registry.fluid(b).is_some() || registry.def(b).name.ends_with("ice"))
        })
}

fn count_fluid(registry: &BlockRegistry, world: &VoxelWorld, pos: ColumnPos) -> usize {
    let origin = pos.origin();
    let mut count = 0;
    for y in 0..WORLD_HEIGHT {
        for z in 0..32 {
            for x in 0..32 {
                count += world.block(origin + IVec3::new(x, y, z)).is_some_and(|b| registry.fluid(b).is_some()) as usize;
            }
        }
    }
    count
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
