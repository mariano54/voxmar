//! Generated water must not move: loading it schedules no fluid updates, and ticking any of
//! it would change nothing (it is a fixed point of the fluid kernel), so a world full of
//! lakes, rivers, waterfalls and caves costs the kernel nothing until a player changes
//! something.

use std::sync::Arc;

use rayon::prelude::*;

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::fluid::{Fluids, unsettled};
use voxmar_engine::glam::{IVec2, IVec3};
use voxmar_engine::world::{Column, VoxelWorld};

use crate::{Generator, Overworld, WaterSight};

/// Columns along a side of each generated area; the middle ones have all their neighbours.
const SIDE: i32 = 5;

#[test]
fn generated_water_does_not_move() {
    let registry = crate::full_content().0.blocks.clone();
    let (mut fluid_cells, mut curtain, mut pinned) = (0, 0, 0);
    // Around every place water makes near spawn on a few seeds (waterfalls, highland
    // lakes, confluences, river mouths, and tunnels, islands and hot springs where there
    // are any), found afresh so the test follows any reseed, and land with caves; then on
    // more seeds only while too little of the water has been seen pouring or flowing.
    let mut kinds = std::collections::HashSet::new();
    let first = [(3, true, None), (11, true, None), (2, true, None), (5, true, None), (7, true, Some(IVec2::new(1500, 5200))), (0x5eed, false, Some(IVec2::ZERO))];
    let more = [13, 17, 19, 23, 29, 31].map(|seed| (seed, true, None));
    // Around the places of one seed, each on its own core: the kinds of water found and
    // the counts of each place.
    let seed_areas = |(seed, near_spawn, also): (u64, bool, Option<IVec2>)| {
        let generator = crate::shared_full_world(seed);
        let mut places: Vec<IVec2> = also.into_iter().collect();
        let mut found = Vec::new();
        if near_spawn {
            let spawn = generator.spawn_point();
            let sights = generator.water_sights(IVec2::new(spawn.x, spawn.z), 600);
            found.extend(sights.iter().map(|&(kind, _)| kind));
            places.extend(sights.into_iter().map(|(_, at)| IVec2::new(at.x, at.z)));
        }
        let columns = generate_around(&generator, &places);
        let counts: Vec<(usize, usize, usize)> = places.par_iter().map(|&place| check_area(&registry, &columns, seed, place)).collect();
        (found, counts)
    };
    // The first seeds side by side, the rest one at a time and only while needed, all
    // counted in seed order.
    let mut count = |(found, counts): (Vec<WaterSight>, Vec<(usize, usize, usize)>)| {
        kinds.extend(found);
        for (fluids, falls, rivers) in counts {
            (fluid_cells, curtain, pinned) = (fluid_cells + fluids, curtain + falls, pinned + rivers);
        }
        pinned > 500 && curtain > 20
    };
    let mut enough = false;
    for areas in first.into_par_iter().map(seed_areas).collect::<Vec<_>>() {
        enough = count(areas);
    }
    for seed in more {
        if enough {
            break;
        }
        enough = count(seed_areas(seed));
    }
    println!("{fluid_cells} fluid cells: {pinned} of rivers with a current, {curtain} pouring down falls; found {kinds:?}");
    for kind in [WaterSight::Waterfall, WaterSight::HighlandLake, WaterSight::Confluence, WaterSight::RiverMouth] {
        assert!(kinds.contains(&kind), "no {} near any spawn", kind.name());
    }
    assert!(fluid_cells > 10_000 && pinned > 500 && curtain > 20);
}

/// Tarns, the small still lakes in high hollows, settle like any other lake: around the
/// first two pits filled by the tarn rule in the regions around the alpine meadow
/// nearest the origin, on a few seeds, found afresh.
#[test]
fn tarns_do_not_move() {
    let registry = crate::full_content().0.blocks.clone();
    // Each seed on its own core.
    let tarns: usize = [1u64, 2, 3, 4].par_iter().map(|&seed| {
        let generator = crate::shared_full_world(seed);
        let floor = generator.rivers.tarn_floor().expect("the base content has tarns");
        let Some(meadow) = generator.locate_biome("alpine_meadow", IVec2::ZERO, 60000) else { return 0 };
        let region = meadow.div_euclid(IVec2::splat(crate::water::REGION));
        let found: Vec<IVec2> = (0..9)
            .flat_map(|r| generator.rivers.stream_lakes(generator.stages(), region + IVec2::new(r % 3 - 1, r / 3 - 1)))
            .filter(|&(_, water, _, tarn)| water as f32 >= floor && tarn)
            .map(|(pit, _, _, _)| pit)
            .take(2)
            .collect();
        let columns = generate_around(&generator, &found);
        for &pit in &found {
            let fluids = check_area(&registry, &columns, seed, pit).0;
            assert!(fluids > 0, "seed {seed}: no water around the tarn at {pit}");
        }
        found.len()
    }).sum();
    assert!(tarns >= 4, "only {tarns} tarns found around alpine meadows");
}

/// Checks that none of the water in the area around `place` (its columns from `columns`)
/// would move: none is scheduled when its columns load, and ticking any of it would change
/// nothing. Counts its fluid cells as `count_fluid` does.
fn check_area(registry: &BlockRegistry, columns: &Columns, seed: u64, place: IVec2) -> (usize, usize, usize) {
    let first = area_from(place);
    let mut world = VoxelWorld::default();
    for i in 0..SIDE * SIDE {
        let pos = first.offset(i % SIDE, i / SIDE);
        world.insert(pos, columns[&pos].clone());
    }
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

/// Generated columns, by place.
type Columns = std::collections::HashMap<ColumnPos, Arc<Column>>;

/// The first column of the `SIDE` × `SIDE` area around `place`.
fn area_from(place: IVec2) -> ColumnPos {
    ColumnPos::of_block(IVec3::new(place.x, 0, place.y)).offset(-SIDE / 2, -SIDE / 2)
}

/// The columns of the areas around `places`, each generated once however many areas it lies
/// in (places near each other share some), side by side on the test threads' pool.
fn generate_around(generator: &Overworld, places: &[IVec2]) -> Columns {
    let mut positions: Vec<ColumnPos> = places.iter().flat_map(|&place| {
        let first = area_from(place);
        (0..SIDE * SIDE).map(move |i| first.offset(i % SIDE, i / SIDE))
    }).collect();
    positions.sort_by_key(|p| (p.0.x, p.0.y));
    positions.dedup();
    positions.into_par_iter().map(|pos| (pos, Arc::new(generator.generate(pos)))).collect()
}

/// A `SIDE` × `SIDE` area of columns from `first`, generated on the test threads' pool.
fn generate(generator: &Overworld, first: ColumnPos) -> VoxelWorld {
    let positions: Vec<ColumnPos> = (0..SIDE * SIDE).map(|i| first.offset(i % SIDE, i / SIDE)).collect();
    let columns: Vec<Column> = positions.par_iter().map(|&pos| generator.generate(pos)).collect();
    let mut world = VoxelWorld::default();
    for (pos, column) in positions.into_iter().zip(columns) {
        world.insert(pos, Arc::new(column));
    }
    world
}

/// The next generation version reseeds every world; its water must not move either: around
/// the water near spawn on a few seeds, and where seed 5 once stopped a waterfall's curtain
/// on a mangrove's roots, grown before it (the falling water stood on wood and wanted to
/// spill sideways).
#[test]
fn water_still_settles_at_the_next_version() {
    let (content, world_content) = crate::full_content();
    let registry = content.blocks.clone();
    // Each seed on its own core, and each place of it.
    let fluid_cells: usize = [5u64, 7, 19].par_iter().map(|&seed| {
        let generator = Overworld::with_content_at(seed, registry.clone(), &world_content, Overworld::VERSION + 1);
        let spawn = generator.spawn_point();
        let mut places: Vec<IVec2> = generator.water_sights(IVec2::new(spawn.x, spawn.z), 600).into_iter().map(|(_, at)| IVec2::new(at.x, at.z)).collect();
        if seed == 5 {
            places.push(IVec2::new(851, 1030));
        }
        let columns = generate_around(&generator, &places);
        places.par_iter().map(|&place| {
            let fluids = check_area(&registry, &columns, seed, place).0;
            assert!(!(seed == 5 && place == IVec2::new(851, 1030)) || fluids > 0, "no water at seed 5's old waterfall spot");
            fluids
        }).sum::<usize>()
    }).sum();
    assert!(fluid_cells > 1000, "too little water seen at the next version: {fluid_cells}");
}

/// One spot, seed and generation version: `SEED`, `X`, `Z`, `VERSION` (default the
/// waterfall seed 5 left unsettled at version 24, at 851, 1030).
/// `cargo test -p voxmar_worldgen --lib generated_water::probe_water -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_water() {
    let env = |name: &str, default: i64| std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default);
    let (content, world_content) = crate::full_content();
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
