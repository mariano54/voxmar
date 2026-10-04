//! Benchmarks of generation, and the test that keeps its output fixed while it is made
//! faster.
//!
//! The benchmarks are ignored tests, run in release builds:
//!
//! - `generation_stages`: where column generation spends its time, stage by stage (see
//!   [`crate::stage`]), and what lighting and meshing the columns costs;
//! - `first_column`: how long a new world takes to find its spawn point and generate the
//!   spawn column, and to fill the area around it on every core;
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
    let source = include_str!("../../../assets/content/base/blocks.ron");
    Arc::new(BlockRegistry::from_ron("base", source).unwrap())
}

/// CPU time this thread has used: unlike wall-clock time, it is not inflated when other
/// processes compete for the cores.
pub(crate) fn thread_time() -> Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `t` is a valid timespec for the call to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
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
            let start = Instant::now();
            let spawn = ColumnPos::of_block(world.spawn_point());
            let found = start.elapsed();
            let start = Instant::now();
            let (_, times) = stage::measure(|| world.generate(spawn));
            let generated = start.elapsed();
            let start = Instant::now();
            std::hint::black_box(crate::tests::generate_area(&world, spawn.offset(-4, -4), 9));
            let area = start.elapsed();
            let mut stages: Vec<(Stage, Duration)> = Stage::ALL.iter().map(|&s| (s, times.get(s))).filter(|(_, d)| ms(*d) >= 1.0).collect();
            stages.sort_by_key(|&(_, d)| std::cmp::Reverse(d));
            println!(
                "seed {seed}, {}: spawn point {:.0} ms, spawn column {:.0} ms ({}), then 9×9 columns {:.0} ms",
                if parallel { "every core" } else { "one core" },
                ms(found),
                ms(generated),
                stages.iter().map(|(s, d)| format!("{s:?} {:.0}", ms(*d))).collect::<Vec<_>>().join(", "),
                ms(area),
            );
        }
    }
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

/// Columns over rivers and lakes, caves, forests, mountains and coasts on three seeds.
const GOLDEN: [(u64, [(i32, i32); 8]); 3] = [
    (1, [(0, 0), (1, 0), (0, 2), (60, -90), (-120, 40), (150, 150), (-40, -200), (-9, -6)]),
    (42, [(-10, -3), (-9, -2), (-1, 0), (0, 0), (-33, -2), (93, -63), (-10, -11), (4, 31)]),
    (0x5eed, [(0, 0), (60, -90), (-120, 40), (150, 150), (-40, -200), (-128, 28), (47, 162), (8, 8)]),
];

/// The hash of `GOLDEN`'s columns per seed, as generation makes them now.
const EXPECTED: [u64; 3] = [0xc4a2259252bd18d5, 0xdf3eb9ba26a89b9d, 0xf299737c5df82079];

/// Generation's output stays exactly as it was: performance work must not change a block.
/// When a change to the output is deliberate (new terrain, a biome's colours), replace
/// `EXPECTED` with the hashes this test prints, and bump `Overworld::VERSION` if the terrain
/// changed.
#[test]
fn output_is_unchanged() {
    let registry = registry();
    let hashes: Vec<u64> = GOLDEN
        .iter()
        .map(|(seed, columns)| {
            let world = Overworld::new(*seed, registry.clone());
            columns.iter().fold(0, |h, &(x, z)| mix(h ^ column_hash(&world.generate(ColumnPos::new(x, z)), &registry)))
        })
        .collect();
    let printed = hashes.iter().map(|h| format!("{h:#018x}")).collect::<Vec<_>>().join(", ");
    assert_eq!(
        hashes, EXPECTED,
        "generation's output changed. If that is deliberate, set EXPECTED to [{printed}], and bump Overworld::VERSION ({}) if the terrain changed",
        Overworld::VERSION
    );
}

/// The hash of the light over three 3×3 areas, one per seed of `GOLDEN`, as lighting makes it now.
const LIGHT: u64 = 0x49b639018a76aeaf;

/// Lighting stays exactly as it was too: the light of the centre column of a 3×3 area on
/// each of `GOLDEN`'s seeds, with the border the mesher reads. When generation changes on
/// purpose this changes with it; set `LIGHT` to the hash this test prints.
#[test]
fn light_is_unchanged() {
    let registry = registry();
    let mut hash = 0;
    for (seed, columns) in GOLDEN {
        let world = Overworld::new(seed, registry.clone());
        let centre = ColumnPos::new(columns[0].0, columns[0].1);
        let around: [Arc<Column>; 9] = std::array::from_fn(|k| Arc::new(world.generate(centre.offset(k as i32 % 3 - 1, k as i32 / 3 - 1))));
        let region = LitRegion::compute(&registry, &around);
        for y in 0..voxmar_engine::coords::WORLD_HEIGHT {
            for z in -1..33 {
                hash = mix(hash ^ (-1..33).fold(0u64, |h, x| mix(h ^ region.light(voxmar_engine::glam::IVec3::new(x, y, z)) as u64)));
            }
        }
    }
    assert_eq!(hash, LIGHT, "lighting changed. If that is deliberate, set LIGHT to {hash:#018x}");
}
