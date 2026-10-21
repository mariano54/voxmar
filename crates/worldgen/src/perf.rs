//! Benchmarks of generation, and the test that keeps its output fixed while it is made
//! faster.
//!
//! The benchmarks are ignored tests, run in release builds:
//!
//! - `generation_stages`: where column generation spends its time, stage by stage (see
//!   [`crate::stage`]), and what lighting and meshing the columns costs;
//! - `first_column`: how long a new world takes to find its spawn point and generate the
//!   spawn column, and to fill the area around it on every core;
//! - `all_cores`: the wall-clock time per column of generating the same areas on every
//!   core, cold and warm;
//! - `land_samples`: the cost of one sample of the land on the rivers' and trails' 8-block
//!   grids, the work most of a new region's time goes to;
//! - `distant_tiles`: the cost of one sample of the client's distant land round spawn,
//!   stage by stage, cold and warm;
//!
//! next to `generation_speed`, `river_speed` and `column_timing` in the crate's tests.
//! Times are thread CPU time where they can be, so they hold up on a busy machine; stage
//! shares come from a wall clock (CPU time costs a system call per reading, far too much
//! per stage), which a busy machine blurs but does not bias.

use std::sync::Arc;
use std::time::{Duration, Instant};

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::{COLUMN_CHUNKS, ColumnPos};
use voxmar_engine::light::LitRegion;
use voxmar_engine::mesh::mesh_chunk;
use voxmar_engine::world::Column;

use crate::rng::mix;
use crate::stage::{self, Stage, Times};
use crate::{Generator, Overworld};

fn registry() -> Arc<BlockRegistry> {
    Arc::new(crate::test_registry())
}

/// CPU time this thread has used: unlike wall-clock time, it is not inflated when other
/// processes compete for the cores.
#[cfg(unix)]
pub(crate) fn thread_time() -> Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `t` is a valid timespec for the call to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// Wall-clock time since the first call, where there is no `clock_gettime` (Windows): the
/// benchmarks still run, but a busy machine inflates their times.
#[cfg(not(unix))]
pub(crate) fn thread_time() -> Duration {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// Asks the scheduler to keep this thread on the performance cores: on a busy machine it
/// may otherwise move a benchmark to the efficiency cores, which run it twice as slowly
/// in CPU time.
pub(crate) fn prefer_fast_cores() {
    #[cfg(target_os = "macos")]
    // SAFETY: no preconditions; it only changes this thread's scheduling class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// The areas `generation_speed` generates: 8×8 columns at each of these, far enough
/// apart that each needs rivers and trails of its own.
const AREAS: [(i32, i32); 5] = [(0, 0), (60, -90), (-120, 40), (150, 150), (-40, -200)];
const SEEDS: [u64; 3] = [1, 3, 0x5eed];

/// `cargo test --release -p voxmar_worldgen generation_stages -- --ignored --nocapture`
/// generates the same columns as `generation_speed`, on fresh worlds, and reports the
/// time per column of each stage: cold, the first time, which includes building the
/// rivers and trails of each area's regions (shared by only 64 columns here, against
/// about a thousand in play), and warm, generating the same columns again. Then it lights
/// and meshes the inner 6×6 columns of each area and reports those per column too.
#[test]
#[ignore]
fn generation_stages() {
    // CPU time on this thread measures all the work only if it all stays here.
    crate::parallel::set_enabled(false);
    prefer_fast_cores();
    let registry = registry();
    // Cold and warm: stage times, CPU time, columns.
    let mut passes = [(Times::default(), Duration::ZERO); 2];
    let mut columns = 0;
    let (mut light, mut mesh, mut lit) = (Duration::ZERO, Duration::ZERO, 0);
    for seed in SEEDS {
        let world = Overworld::new(seed, registry.clone());
        for (cx, cz) in AREAS {
            let mut area = Vec::new();
            for (times, cpu) in &mut passes {
                let start = thread_time();
                let (columns, t) = stage::measure(|| (0..64).map(|i| Arc::new(world.generate(ColumnPos::new(cx + i % 8, cz + i / 8)))).collect::<Vec<_>>());
                *cpu += thread_time() - start;
                *times += t;
                area = columns;
            }
            columns += area.len();
            for j in 1..7 {
                for i in 1..7 {
                    let around: [Arc<Column>; 9] = std::array::from_fn(|k| area[(j + k / 3 - 1) * 8 + i + k % 3 - 1].clone());
                    let start = thread_time();
                    let region = LitRegion::compute(&registry, &around);
                    let lighting = thread_time();
                    for cy in 0..COLUMN_CHUNKS {
                        std::hint::black_box(mesh_chunk(&registry, &region, &around[4], cy, 1));
                    }
                    light += lighting - start;
                    mesh += thread_time() - lighting;
                    lit += 1;
                }
            }
        }
    }
    // Shares from the wall clock, scaled to the CPU time each pass took.
    let ms = |d: Duration, n: usize| d.as_secs_f64() * 1e3 / n as f64;
    let scale = passes.map(|(times, cpu)| cpu.as_secs_f64() / times.total().as_secs_f64());
    println!("{:>14} {:>9} {:>6} {:>9} {:>6}   (ms per column, {columns} columns)", "stage", "cold", "share", "warm", "share");
    for stage in Stage::ALL {
        let [cold, warm] = passes.map(|(times, _)| times);
        let share = |times: &Times| 100.0 * times.get(stage).as_secs_f64() / times.total().as_secs_f64();
        println!(
            "{:>14} {:9.3} {:5.1}% {:9.3} {:5.1}%",
            format!("{stage:?}"),
            ms(cold.get(stage), columns) * scale[0],
            share(&cold),
            ms(warm.get(stage), columns) * scale[1],
            share(&warm),
        );
    }
    println!("{:>14} {:9.3} {:>6} {:9.3}   (CPU time)", "generation", ms(passes[0].1, columns), "", ms(passes[1].1, columns));
    println!("{:>14} {:9.3}   ({lit} columns)", "light", ms(light, lit));
    println!("{:>14} {:9.3}   (all {COLUMN_CHUNKS} chunks of each)", "mesh", ms(mesh, lit));
    crate::parallel::set_enabled(true);
}

/// `cargo test --release -p voxmar_worldgen first_column -- --ignored --nocapture`
/// reports, for new worlds, the wall-clock time to find the spawn point, to generate the
/// spawn column after it (the game waits for both before letting the player in; the
/// spawn column's stages are listed), and to fill the 9×9 columns around spawn on every
/// core, the way the game first loads: once building regions on the asking thread
/// alone, once on every core.
#[test]
#[ignore]
fn first_column() {
    prefer_fast_cores();
    let registry = registry();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    for seed in SEEDS {
        for parallel in [false, true] {
            crate::parallel::set_enabled(parallel);
            let world = Overworld::new(seed, registry.clone());
            let (start, cpu) = (Instant::now(), thread_time());
            let (spawn, finding) = stage::measure(|| ColumnPos::of_block(world.spawn_point()));
            let found = (start.elapsed(), thread_time() - cpu);
            let (start, cpu) = (Instant::now(), thread_time());
            let (_, times) = stage::measure(|| world.generate(spawn));
            let generated = (start.elapsed(), thread_time() - cpu);
            let start = Instant::now();
            std::hint::black_box(crate::tests::generate_area(&world, spawn.offset(-4, -4), 9));
            let area = start.elapsed();
            let listed = |times: &Times| {
                let mut stages: Vec<(Stage, Duration)> = Stage::ALL.iter().map(|&s| (s, times.get(s))).filter(|(_, d)| ms(*d) >= 1.0).collect();
                stages.sort_by_key(|&(_, d)| std::cmp::Reverse(d));
                stages.iter().map(|(s, d)| format!("{s:?} {:.0}", ms(*d))).collect::<Vec<_>>().join(", ")
            };
            // On one core all the work is this thread's, so its CPU time counts it all.
            let both = |(wall, cpu): (Duration, Duration)| if parallel { format!("{:.0} ms", ms(wall)) } else { format!("{:.0} ms ({:.0} CPU)", ms(wall), ms(cpu)) };
            println!(
                "seed {seed}, {}: spawn point {} ({}), spawn column {} ({}), then 9×9 columns {:.0} ms",
                if parallel { "every core" } else { "one core" },
                both(found),
                listed(&finding),
                both(generated),
                listed(&times),
                ms(area),
            );
        }
    }
}

/// `cargo test --release -p voxmar_worldgen all_cores -- --ignored --nocapture` generates
/// the areas of `generation_stages` on every core, the way the game loads land, and reports
/// the wall-clock time per column: cold, on fresh worlds (building rivers and trails on
/// the way), and warm, generating the same columns again.
#[test]
#[ignore]
fn all_cores() {
    let registry = registry();
    let (mut cold, mut warm, mut columns) = (Duration::ZERO, Duration::ZERO, 0);
    for seed in SEEDS {
        let world = Overworld::new(seed, registry.clone());
        for (cx, cz) in AREAS {
            for pass in [&mut cold, &mut warm] {
                let start = Instant::now();
                std::hint::black_box(crate::tests::generate_area(&world, ColumnPos::new(cx, cz), 8));
                *pass += start.elapsed();
            }
            columns += 64;
        }
    }
    let ms = |d: Duration| d.as_secs_f64() * 1e3 / columns as f64;
    println!("every core: cold {:.3} ms per column, warm {:.3} ms per column ({columns} columns)", ms(cold), ms(warm));
}

/// `cargo test --release -p voxmar_worldgen land_samples -- --ignored --nocapture` samples
/// the land on an 8-block grid in tiles of 32×32, as river tiles ask for it, over 4 km
/// squares of three worlds, and reports the CPU time per sample. Under
/// `/usr/bin/time -l`, with `SIDE=0` for the cost of making the worlds alone, it gives
/// instructions and cycles per sample, which a busy machine does not change.
#[test]
#[ignore]
fn land_samples() {
    let registry = registry();
    let side: i32 = std::env::var("SIDE").ok().and_then(|s| s.parse().ok()).unwrap_or(512);
    let worlds: Vec<Overworld> = SEEDS.iter().map(|&seed| Overworld::new(seed, registry.clone())).collect();
    let start = thread_time();
    let mut sum = 0.0;
    for world in &worlds {
        for tile in 0..(side / 32) * (side / 32) {
            let (tx, tz) = (tile % (side / 32), tile / (side / 32));
            let points: Vec<(i32, i32)> = (0..32 * 32).map(|i| ((tx * 32 + i % 32) * 8 + 4 - side * 4, (tz * 32 + i / 32) * 8 + 4 - side * 4)).collect();
            sum += world.geology.lands(&points).iter().map(|l| l.height).sum::<f32>();
        }
    }
    std::hint::black_box(sum);
    let samples = (worlds.len() as i32 * side * side).max(1);
    println!("{:.0} ns per land sample", (thread_time() - start).as_nanos() as f64 / samples as f64);
}

/// A hash of everything a column holds: its blocks (by name, so that adding a block to
/// the registry does not change it) and colours.
fn column_hash(column: &Column, registry: &BlockRegistry) -> u64 {
    let names: Vec<u64> = (0..registry.len()).map(|i| crate::rarity::name_hash(&registry.def(voxmar_engine::block::BlockId(i as u16)).name)).collect();
    let mut h = 0u64;
    let mut dense = vec![voxmar_engine::block::BlockId::AIR; voxmar_engine::coords::CHUNK_VOLUME];
    for chunk in &column.chunks {
        chunk.copy_to_dense(&mut dense);
        for block in &dense {
            h = mix(h ^ names[block.0 as usize]);
        }
    }
    for c in &column.colors {
        h = mix(h ^ c.iter().flatten().fold(0u64, |w, &v| w << 8 | v as u64));
    }
    h
}

/// Columns over rivers and lakes, caves, forests, mountains and coasts on three seeds,
/// around a waterfall on a fourth, and spread kilometres apart on two more.
const GOLDEN: [(u64, [(i32, i32); 8]); 6] = [
    (1, [(0, 0), (1, 0), (0, 2), (60, -90), (-120, 40), (150, 150), (-40, -200), (-9, -6)]),
    (42, [(-10, -3), (-9, -2), (-1, 0), (0, 0), (-33, -2), (93, -63), (-10, -11), (4, 31)]),
    (0x5eed, [(0, 0), (60, -90), (-120, 40), (150, 150), (-40, -200), (-128, 28), (47, 162), (8, 8)]),
    // Around a waterfall at (-894, 298): its curtain, pool and the water feeding its lip.
    (0, [(-28, 9), (-29, 9), (-27, 9), (-28, 8), (-28, 10), (-29, 8), (-27, 10), (-29, 10)]),
    (7, [(0, 0), (-1, 0), (31, -17), (-75, 120), (240, 9), (-201, -233), (5, 310), (-330, 77)]),
    (99, [(2, -3), (2, -2), (-64, -64), (128, -40), (-250, 180), (300, 290), (-14, -400), (411, -9)]),
];

/// The hash of `GOLDEN`'s columns per seed, as generation makes them now.
const EXPECTED: [u64; 6] = [0x6fe0c486f705eed6, 0x07e39c2961008711, 0xf315e242d3aba9c3, 0xefd2a5f5d8aa5492, 0xe1e02ed3215f490e, 0xee605fccb7cebada];

/// Generation's output stays exactly as it was: performance work must not change a block.
/// When a change to the output is deliberate (new terrain, a biome's colours), replace
/// `EXPECTED` with the hashes this test prints, and bump `Overworld::VERSION` if the terrain
/// changed.
#[test]
fn output_is_unchanged() {
    use rayon::prelude::*;
    let registry = registry();
    // Every seed and column on its own core, mixed in order.
    let hashes: Vec<u64> = GOLDEN
        .par_iter()
        .map(|(seed, columns)| {
            let world = Overworld::new(*seed, registry.clone());
            let each: Vec<u64> = columns.par_iter().map(|&(x, z)| column_hash(&world.generate(ColumnPos::new(x, z)), &registry)).collect();
            each.into_iter().fold(0, |h, column| mix(h ^ column))
        })
        .collect();
    let printed = hashes.iter().map(|h| format!("{h:#018x}")).collect::<Vec<_>>().join(", ");
    assert_eq!(
        hashes, EXPECTED,
        "generation's output changed. If that is deliberate, set EXPECTED to [{printed}], and bump Overworld::VERSION ({}) if the terrain changed",
        Overworld::VERSION
    );
}

/// The hash of the light over four 3×3 areas, one per seed of `GOLDEN`'s first four, as
/// lighting makes it now.
const LIGHT: u64 = 0x2923024e18114411;

/// Lighting stays exactly as it was too: the light of the centre column of a 3×3 area on
/// each of `GOLDEN`'s first four seeds, with the border the mesher reads. When generation changes on
/// purpose this changes with it; set `LIGHT` to the hash this test prints.
#[test]
fn light_is_unchanged() {
    use rayon::prelude::*;
    let registry = registry();
    // Every seed and column on its own core, the light mixed in seed order.
    let regions: Vec<LitRegion> = GOLDEN[..4]
        .par_iter()
        .map(|&(seed, columns)| {
            let world = Overworld::new(seed, registry.clone());
            let centre = ColumnPos::new(columns[0].0, columns[0].1);
            let around: Vec<Arc<Column>> = (0..9).into_par_iter().map(|k| Arc::new(world.generate(centre.offset(k as i32 % 3 - 1, k as i32 / 3 - 1)))).collect();
            let around: [Arc<Column>; 9] = around.try_into().ok().expect("nine columns");
            LitRegion::compute(&registry, &around)
        })
        .collect();
    let mut hash = 0;
    for region in &regions {
        for y in 0..voxmar_engine::coords::WORLD_HEIGHT {
            for z in -1..33 {
                hash = mix(hash ^ (-1..33).fold(0u64, |h, x| mix(h ^ region.light(voxmar_engine::glam::IVec3::new(x, y, z)) as u64)));
            }
        }
    }
    assert_eq!(hash, LIGHT, "lighting changed. If that is deliberate, set LIGHT to {hash:#018x}");
}

/// `cargo test --release -p voxmar_worldgen distant_tiles -- --ignored --nocapture`
/// samples what the client's distant land (`render::lod`) samples round the spawn of
/// seed 1, on one thread: each tile's corner heights, then its sampler and each corner's
/// surface, one at a time, on a world nothing has built, then again with the land's
/// regions known; then as the client samples them, each corner's ground once for its
/// height and its surface. It reports the time per sample, stage by stage. The rings are
/// the client's:
/// four, of cells 4 to 32 blocks, 13×13 tiles of 32×32 cells each, less what the finer
/// ring covers.
#[test]
#[ignore]
fn distant_tiles() {
    use voxmar_engine::glam::IVec2;
    const LEVELS: i32 = 4;
    const FIRST_CELL: i32 = 4;
    const CELLS: i32 = 32;
    const RING: i32 = 6;
    const W: i32 = CELLS + 3;
    prefer_fast_cores();
    let (content, world_content) = crate::full_content();
    let world = Overworld::with_content(1, content.blocks.clone(), world_content);
    let spawn = world.settle_spawn();
    let camera = IVec2::new(spawn.x, spawn.z);
    let mut tiles = Vec::new();
    for level in 0..LEVELS {
        let (cell, tile) = (FIRST_CELL << level, (FIRST_CELL << level) * CELLS);
        let center = camera.div_euclid(IVec2::splat(tile));
        let finer = camera.div_euclid(IVec2::splat(tile / 2));
        let finer = ((finer - RING) * (tile / 2), (finer + RING + 1) * (tile / 2));
        for z in center.y - RING..=center.y + RING {
            for x in center.x - RING..=center.x + RING {
                let low = IVec2::new(x, z) * tile;
                if level == 0 || !(low.cmpge(finer.0).all() && (low + tile).cmple(finer.1).all()) {
                    tiles.push((level, cell, low));
                }
            }
        }
    }
    let samples = tiles.len() as f64 * (W * W) as f64;
    let heights = |world: &Overworld| {
        tiles.iter().map(|&(_, cell, origin)| (0..W * W).map(|i| world.height(origin.x + (i % W - 1) * cell, origin.y + (i / W - 1) * cell)).collect::<Vec<_>>()).collect::<Vec<_>>()
    };
    let surfaces = |world: &Overworld, heights: &[Vec<i32>]| {
        for (&(_, cell, origin), heights) in tiles.iter().zip(heights) {
            let sampler = world.sampler(origin - 2 * cell, origin + (CELLS + 2) * cell);
            for i in 0..W * W {
                std::hint::black_box(sampler.surface(origin.x + (i % W - 1) * cell, origin.y + (i / W - 1) * cell, heights[i as usize], 0));
            }
        }
    };
    let report = |label: &str, cpu: Duration, times: Times| {
        let per = |d: Duration| d.as_secs_f64() * 1e6 / samples;
        let stages: Vec<String> = Stage::ALL.iter().filter(|&&s| times.get(s) > Duration::from_millis(5)).map(|&s| format!("{s:?} {:.1}", per(times.get(s)))).collect();
        eprintln!("{label}: {:.1} µs a sample ({:.1}s CPU), stages in µs: {}", per(cpu), cpu.as_secs_f64(), stages.join(", "));
    };
    eprintln!("seed 1: {} tiles, {} samples round spawn {spawn}", tiles.len(), samples);
    for round in ["first", "again"] {
        let started = thread_time();
        let (h, times) = stage::measure(|| heights(&world));
        report(&format!("{round}: heights"), thread_time() - started, times);
        let started = thread_time();
        let ((), times) = stage::measure(|| surfaces(&world, &h));
        report(&format!("{round}: surfaces"), thread_time() - started, times);
    }
    // As the client samples them: the ground at each corner once, for its height and the
    // surface on it.
    let started = thread_time();
    let ((), times) = stage::measure(|| {
        for &(_, cell, origin) in &tiles {
            let grounds = world.grounds((0..W * W).map(|i| (origin.x + (i % W - 1) * cell, origin.y + (i / W - 1) * cell)));
            let sampler = world.sampler(origin - 2 * cell, origin + (CELLS + 2) * cell);
            for (i, ground) in grounds.iter().enumerate() {
                let i = i as i32;
                std::hint::black_box(sampler.surface_on(ground, origin.x + (i % W - 1) * cell, origin.y + (i / W - 1) * cell, ground.height(), 0));
            }
        }
    });
    report("again: grounds once, then surfaces", thread_time() - started, times);
}
