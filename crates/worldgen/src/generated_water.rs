//! Generated water must not move: loading it schedules no fluid updates, and ticking any of
//! it would change nothing (it is a fixed point of the fluid kernel), so a world full of
//! lakes, rivers, waterfalls and caves costs the kernel nothing until a player changes
//! something.

use std::sync::Arc;

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::content::Content;
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::fluid::{Fluids, unsettled};
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::VoxelWorld;

use crate::{Generator, Overworld, WaterSight, WorldContent};

/// Columns along a side of each generated area; the middle ones have all their neighbours.
const SIDE: i32 = 5;

/// Every package's content, as a server loads it.
fn load_content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

#[test]
fn generated_water_does_not_move() {
    let content = load_content();
    let world_content = WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let (mut fluid_cells, mut curtain, mut pinned) = (0, 0, 0);
    // Around every place water makes near spawn on a few seeds (waterfalls, highland
    // lakes, confluences, river mouths, and tunnels, islands and hot springs where there
    // are any), found afresh so the test follows any reseed, and land with caves; then on
    // more seeds only while too little of the water has been seen pouring or flowing.
    let mut kinds = std::collections::HashSet::new();
    let first = [(3, true, None), (11, true, None), (2, true, None), (5, true, None), (7, true, Some(IVec2::new(1500, 5200))), (0x5eed, false, Some(IVec2::ZERO))];
    let more = [13, 17, 19, 23, 29, 31].map(|seed| (seed, true, None));
    for (i, (seed, near_spawn, also)) in first.into_iter().chain(more).enumerate() {
        if i >= first.len() && pinned > 500 && curtain > 20 {
            break;
        }
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

/// Tarns, the small still lakes in high hollows, settle like any other lake: around the
/// tarn nearest the alpine meadows closest to spawn on a few seeds, found afresh.
#[test]
fn tarns_do_not_move() {
    let content = load_content();
    let world_content = WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let mut tarns = 0;
    for seed in [1, 2, 3, 4] {
        let generator = Overworld::with_content(seed, registry.clone(), &world_content);
        let spawn = generator.spawn_point();
        let Some(meadow) = generator.locate_biome("alpine_meadow", IVec2::new(spawn.x, spawn.z), 20000) else { continue };
        let Some(&(_, at)) = generator.water_sights(meadow, 400).iter().find(|(kind, _)| *kind == WaterSight::Tarn) else { continue };
        let fluids = check_area(&registry, &generator, seed, IVec2::new(at.x, at.z)).0;
        assert!(fluids > 0, "seed {seed}: no water around the tarn at {at}");
        tarns += 1;
    }
    assert!(tarns >= 2, "only {tarns} tarns found near alpine meadows");
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

/// The next generation version reseeds every world; its water must not move either: around
/// the water near spawn on a few seeds, and where seed 5 once stopped a waterfall's curtain
/// on a mangrove's roots, grown before it (the falling water stood on wood and wanted to
/// spill sideways).
#[test]
fn water_still_settles_at_the_next_version() {
    let content = load_content();
    let world_content = WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let mut fluid_cells = 0;
    for seed in [5, 7, 19] {
        let generator = Overworld::with_content_at(seed, registry.clone(), &world_content, Overworld::VERSION + 1);
        let spawn = generator.spawn_point();
        let mut places: Vec<IVec2> = generator.water_sights(IVec2::new(spawn.x, spawn.z), 600).into_iter().map(|(_, at)| IVec2::new(at.x, at.z)).collect();
        if seed == 5 {
            places.push(IVec2::new(851, 1030));
        }
        for place in places {
            let fluids = check_area(&registry, &generator, seed, place).0;
            assert!(!(seed == 5 && place == IVec2::new(851, 1030)) || fluids > 0, "no water at seed 5's old waterfall spot");
            fluid_cells += fluids;
        }
    }
    assert!(fluid_cells > 1000, "too little water seen at the next version: {fluid_cells}");
}

/// One spot, seed and generation version: `SEED`, `X`, `Z`, `VERSION` (default the
/// waterfall seed 5 left unsettled at version 24, at 851, 1030).
/// `cargo test -p voxmar_worldgen --lib generated_water::probe_water -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_water() {
    let env = |name: &str, default: i64| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
    let content = load_content();
    let world_content = WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let generator = Overworld::with_content_at(env("SEED", 5) as u64, registry.clone(), &world_content, env("VERSION", 24) as u32);
    let place = IVec2::new(env("X", 851) as i32, env("Z", 1030) as i32);
    let first = ColumnPos::of_block(IVec3::new(place.x, 0, place.y)).offset(-SIDE / 2, -SIDE / 2);
    let world = generate(&generator, first);
    for dz in 1..SIDE - 1 {
        for dx in 1..SIDE - 1 {
            for (cell, changes) in unsettled(&registry, &world, first.offset(dx, dz)) {
                println!("{cell} {}: {:?}", registry.def(world.block(cell).unwrap()).name, changes.iter().map(|&(p, b)| (p, registry.def(b).name.as_str())).collect::<Vec<_>>());
            }
        }
    }
    let name = |p: IVec3| world.block(p).map_or("?".to_string(), |b| registry.def(b).name.clone());
    let y0 = env("Y", 135) as i32;
    for y in (y0 - 6..=y0 + 4).rev() {
        let row: Vec<String> = (-3..=3).map(|dz| format!("{:>16}", name(IVec3::new(place.x, y, place.y + dz)).trim_start_matches("base:"))).collect();
        println!("y {y} (x {}, z {}..={}): {}", place.x, place.y - 3, place.y + 3, row.join(" "));
    }
}
