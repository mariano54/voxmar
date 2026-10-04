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

/// The areas `generation_speed` generates: 8×8 columns at each of these, far enough
/// apart that each needs rivers and trails of its own.
const AREAS: [(i32, i32); 5] = [(0, 0), (60, -90), (-120, 40), (150, 150), (-40, -200)];
const SEEDS: [u64; 3] = [1, 3, 0x5eed];

/// `cargo test --release -p voxmar_worldgen generation_stages -- --ignored --nocapture`
/// generates the same columns as `generation_speed`, on fresh worlds, and reports the
/// time per column of each stage, then lights and meshes the inner 6×6 columns of each
/// area and reports those per column too.
#[test]
#[ignore]
fn generation_stages() {
    let registry = registry();
    let (mut times, mut cpu, mut columns) = (Times::default(), Duration::ZERO, 0);
    let (mut light, mut mesh, mut lit) = (Duration::ZERO, Duration::ZERO, 0);
    for seed in SEEDS {
        let world = Overworld::new(seed, registry.clone());
        for (cx, cz) in AREAS {
            let start = thread_time();
            let (area, t) = stage::measure(|| (0..64).map(|i| Arc::new(world.generate(ColumnPos::new(cx + i % 8, cz + i / 8)))).collect::<Vec<_>>());
            cpu += thread_time() - start;
            times += t;
            columns += area.len();
            for j in 1..7 {
                for i in 1..7 {
                    let around: [Arc<Column>; 9] = std::array::from_fn(|k| area[(j + k / 3 - 1) * 8 + i + k % 3 - 1].clone());
                    let start = thread_time();
                    let region = LitRegion::compute(&registry, &around);
                    let lighting = thread_time();
                    for cy in 0..COLUMN_CHUNKS {
                        std::hint::black_box(mesh_chunk(&registry, &region, &around[4], cy));
                    }
                    light += lighting - start;
                    mesh += thread_time() - lighting;
                    lit += 1;
                }
            }
        }
    }
    // Shares from the wall clock, scaled to the CPU time the whole pass took.
    let per_column = |d: Duration| d.as_secs_f64() * 1e3 / columns as f64;
    let scale = cpu.as_secs_f64() / times.total().as_secs_f64();
    println!("{:>14} {:>9} {:>6}", "stage", "ms/column", "share");
    for stage in Stage::ALL {
        let d = times.get(stage);
        println!("{:>14} {:9.3} {:5.1}%", format!("{stage:?}"), per_column(d) * scale, 100.0 * d.as_secs_f64() / times.total().as_secs_f64());
    }
    println!("{:>14} {:9.3} ({columns} columns, CPU time; wall clock {:.3})", "generation", per_column(cpu), per_column(times.total()));
    let per_lit = |d: Duration| d.as_secs_f64() * 1e3 / lit as f64;
    println!("{:>14} {:9.3} ({lit} columns)", "light", per_lit(light));
    println!("{:>14} {:9.3} (all {COLUMN_CHUNKS} chunks of each)", "mesh", per_lit(mesh));
}

/// `cargo test --release -p voxmar_worldgen first_column -- --ignored --nocapture`
/// reports, for new worlds, the time to find the spawn point, to generate the spawn
/// column after it (where the game waits before letting the player in), stage by stage,
/// and to fill the 9×9 columns around spawn on every core, the way the game first loads.
#[test]
#[ignore]
fn first_column() {
    let registry = registry();
    for seed in SEEDS {
        let world = Overworld::new(seed, registry.clone());
        let (wall, cpu) = (Instant::now(), thread_time());
        let spawn = ColumnPos::of_block(world.spawn_point());
        let found = (wall.elapsed(), thread_time() - cpu);
        let (wall, cpu) = (Instant::now(), thread_time());
        let (_, times) = stage::measure(|| world.generate(spawn));
        let generated = (wall.elapsed(), thread_time() - cpu);
        let wall = Instant::now();
        std::hint::black_box(crate::tests::generate_area(&world, spawn.offset(-4, -4), 9));
        let area = wall.elapsed();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        println!(
            "seed {seed}: spawn point {:.0} ms ({:.0} CPU), spawn column {:.0} ms ({:.0} CPU), then 9×9 columns {:.0} ms",
            ms(found.0),
            ms(found.1),
            ms(generated.0),
            ms(generated.1),
            ms(area),
        );
        let mut stages: Vec<(Stage, Duration)> = Stage::ALL.iter().map(|&s| (s, times.get(s))).filter(|(_, d)| ms(*d) >= 1.0).collect();
        stages.sort_by_key(|&(_, d)| std::cmp::Reverse(d));
        println!("  spawn column: {}", stages.iter().map(|(s, d)| format!("{s:?} {:.0} ms", ms(*d))).collect::<Vec<_>>().join(", "));
    }
}

/// A hash of everything a column holds: its blocks, climate and colours.
fn column_hash(column: &Column) -> u64 {
    let mut h = 0u64;
    let mut dense = vec![voxmar_engine::block::BlockId::AIR; voxmar_engine::coords::CHUNK_VOLUME];
    for chunk in &column.chunks {
        chunk.copy_to_dense(&mut dense);
        for four in dense.chunks(4) {
            h = mix(h ^ four.iter().fold(0u64, |w, b| w << 16 | b.0 as u64));
        }
    }
    for c in &column.climate {
        h = mix(h ^ (c[0] as u64) << 8 ^ c[1] as u64);
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
const EXPECTED: [u64; 3] = [0xd536ee7b9175747d, 0xd9abcffd6e4c3202, 0x4a8befeaa6edbca7];

/// Generation's output stays exactly as it was: performance work must not change a block.
/// When a change to the output is deliberate, bump `Overworld::VERSION` and replace
/// `EXPECTED` with the hashes this test prints.
#[test]
fn output_is_unchanged() {
    let registry = registry();
    let hashes: Vec<u64> = GOLDEN
        .iter()
        .map(|(seed, columns)| {
            let world = Overworld::new(*seed, registry.clone());
            columns.iter().fold(0, |h, &(x, z)| mix(h ^ column_hash(&world.generate(ColumnPos::new(x, z)))))
        })
        .collect();
    let printed = hashes.iter().map(|h| format!("{h:#018x}")).collect::<Vec<_>>().join(", ");
    assert_eq!(
        hashes, EXPECTED,
        "generation's output changed. If that is deliberate, bump Overworld::VERSION ({}) and set EXPECTED to [{printed}]",
        Overworld::VERSION
    );
}
