use rayon::prelude::*;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, Tint};
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3, Vec3Swizzles};
use voxmar_engine::world::Column;

use crate::{Generator, Overworld};
use crate::tests::generate_area;

/// Generated columns over a box of blocks, to look blocks up in.
struct Area {
    columns: Vec<Column>,
    origin: IVec3,
    size: i32,
}

impl Area {
    fn new(world: &Overworld, min: IVec2, max: IVec2) -> Self {
        let first = ColumnPos::of_block(IVec3::new(min.x, 0, min.y));
        let last = ColumnPos::of_block(IVec3::new(max.x, 0, max.y));
        let size = (last.0 - first.0).max_element() + 1;
        Area { columns: generate_area(world, first, size), origin: first.origin(), size }
    }

    fn column(&self, x: i32, z: i32) -> Option<&Column> {
        let (cx, cz) = ((x - self.origin.x) >> 5, (z - self.origin.z) >> 5);
        ((0..self.size).contains(&cx) && (0..self.size).contains(&cz)).then(|| &self.columns[(cz * self.size + cx) as usize])
    }

}

/// Blocks to look at, from generated columns or a model.
trait Look {
    fn block(&self, p: IVec3) -> BlockId;
    fn color(&self, registry: &BlockRegistry, p: IVec3) -> Vec3;
}

impl Look for Area {
    fn block(&self, p: IVec3) -> BlockId {
        match self.column(p.x, p.z) {
            Some(c) if (0..WORLD_HEIGHT).contains(&p.y) => c.block(IVec3::new(p.x & 31, p.y, p.z & 31)),
            _ => BlockId::AIR,
        }
    }

    fn color(&self, registry: &BlockRegistry, p: IVec3) -> Vec3 {
        let def = registry.def(self.block(p));
        let colors = self.column(p.x, p.z).map_or([[0; 3]; 3], |c| c.colors_at(p.x & 31, p.z & 31));
        let v = |c: [u8; 3]| Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32);
        match def.tint.drawn() {
            Tint::Foliage => v(colors[1]) * 0.75,
            Tint::Grass => v(colors[0]) * 0.8,
            Tint::Water => v(colors[2]),
            Tint::None => v(def.color),
        }
    }
}

fn write_ppm(path: &str, w: i32, h: i32, image: &[[u8; 3]]) {
    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
    out.extend(image.iter().flatten());
    std::fs::write(path, out).unwrap();
}

/// The pictures at `paths` (as `write_ppm` writes them) side by side, bottoms lined up,
/// to `out`; the pieces are removed.
fn side_by_side(paths: &[String], out: &str) {
    let read = |path: &String| {
        let bytes = std::fs::read(path).unwrap();
        let header = bytes.iter().position(|&b| b == b'\n').unwrap();
        let size: Vec<usize> = std::str::from_utf8(&bytes[..header]).unwrap().split(' ').skip(1).take(2).map(|n| n.parse().unwrap()).collect();
        (size[0], size[1], bytes[header + 1..].to_vec())
    };
    let pictures: Vec<(usize, usize, Vec<u8>)> = paths.iter().map(read).collect();
    let (w, h) = (pictures.iter().map(|p| p.0).sum::<usize>(), pictures.iter().map(|p| p.1).max().unwrap_or(0));
    let mut image = vec![[200u8, 214, 230]; w * h];
    let mut left = 0;
    for (pw, ph, data) in &pictures {
        for y in 0..*ph {
            for x in 0..*pw {
                let i = (y * pw + x) * 3;
                image[(y + h - ph) * w + left + x] = [data[i], data[i + 1], data[i + 2]];
            }
        }
        left += pw;
    }
    write_ppm(out, w as i32, h as i32, &image);
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// The box from `min` to `max` from above, `scale` pixels a block, shaded by height.
fn top_down(area: &Area, registry: &BlockRegistry, min: IVec2, max: IVec2, scale: i32, path: &str) {
    let (w, h) = ((max.x - min.x + 1) * scale, (max.y - min.y + 1) * scale);
    let mut image = vec![[0u8; 3]; (w * h) as usize];
    for z in min.y..=max.y {
        for x in min.x..=max.x {
            let Some(col) = area.column(x, z) else { continue };
            let _ = col;
            let mut y = WORLD_HEIGHT - 1;
            while y > 0 && area.block(IVec3::new(x, y, z)).is_air() {
                y -= 1;
            }
            let p = IVec3::new(x, y, z);
            let shade = 0.75 + 0.5 * ((y % 8) as f32 / 8.0) * 0.4 + if area.block(p + IVec3::new(-1, 1, 0)).is_air() { 0.0 } else { -0.15 };
            let c = (area.color(registry, p) * shade).min(Vec3::splat(255.0));
            for dy in 0..scale {
                for dx in 0..scale {
                    let (px, py) = ((x - min.x) * scale + dx, (z - min.y) * scale + dy);
                    image[(py * w + px) as usize] = [c.x as u8, c.y as u8, c.z as u8];
                }
            }
        }
    }
    write_ppm(path, w, h, &image);
}

/// The box from `min` to `max`, from `low` to `high`, in an isometric view from the
/// south-east, `a` pixels a block, as `forest_preview` draws, cut clean at the box.
#[allow(clippy::too_many_arguments)]
fn isometric(area: &impl Look, registry: &BlockRegistry, min: IVec2, max: IVec2, low: i32, high: i32, a: f32, path: &str) {
    let project = |p: Vec3| Vec2::new((p.x - p.z) * a, (p.x + p.z) * a * 0.5 - p.y * a);
    let quads = [
        [Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
        [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 0.0, 1.0)],
        [Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
    ];
    let inside = |q: &[Vec2; 4], p: Vec2| {
        let sides: Vec<f32> = (0..4).map(|i| (q[(i + 1) % 4] - q[i]).perp_dot(p - q[i])).collect();
        sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
    };
    let r = (a * 3.0) as i32;
    let mut sprite: Vec<(i32, i32, usize)> = Vec::new();
    for py in -r..r {
        for px in -r..r {
            let p = Vec2::new(px as f32 + 0.5, py as f32 + 0.5);
            if let Some(face) = (0..3).find(|&f| inside(&quads[f].map(project), p)) {
                sprite.push((px, py, face));
            }
        }
    }
    let (sx, sz) = (max.x - min.x + 1, max.y - min.y + 1);
    let corner = |p: Vec3| project(p - Vec3::new(min.x as f32, low as f32, min.y as f32));
    let w = ((sx + sz) as f32 * a) as i32 + 16;
    let h = ((sx + sz) as f32 * a * 0.5 + (high - low) as f32 * a) as i32 + 16;
    let shift = Vec2::new(sz as f32 * a + 8.0, (high - low) as f32 * a + 8.0);
    let mut image = vec![[200u8, 214, 230]; (w * h) as usize];
    let open = |b: BlockId| b.is_air() || matches!(registry.def(b).render, RenderKind::Plant | RenderKind::Leaves) || !registry.def(b).solid;
    for sum in 0..(sx + sz + (high - low)) {
        for y in low..=high {
            for dz in 0..sz {
                let dx = sum - (y - low) - dz;
                if !(0..sx).contains(&dx) {
                    continue;
                }
                let p = IVec3::new(min.x + dx, y, min.y + dz);
                let b = area.block(p);
                // Cut clean at the box: what lies past it counts as open, so its near
                // sides show as solid walls down to `low` and nothing within shows past them.
                let past = |q: IVec3| q.x > max.x || q.z > max.y || q.y > high;
                if b.is_air() || ![IVec3::X, IVec3::Y, IVec3::Z].iter().any(|&d| past(p + d) || open(area.block(p + d))) {
                    continue;
                }
                // Glass as a dark pane, the room behind it, rather than its pale tint.
                let pane = registry.def(b).render == RenderKind::Cutout && registry.def(b).name.contains("glass");
                let base = if pane { area.color(registry, p) * 0.45 } else { area.color(registry, p) };
                let speckle = 0.9 + 0.2 * ((crate::rng::hash3(3, p.x, p.y, p.z) & 0xff) as f32 / 255.0);
                let at = corner(p.as_vec3()) + shift;
                for &(px, py, face) in &sprite {
                    let (x, y) = (at.x as i32 + px, at.y as i32 + py);
                    if x < 0 || y < 0 || x >= w || y >= h {
                        continue;
                    }
                    let c = (base * [1.0, 0.8, 0.62][face] * speckle).min(Vec3::splat(255.0));
                    image[(y * w + x) as usize] = [c.x as u8, c.y as u8, c.z as u8];
                }
            }
        }
    }
    write_ppm(path, w, h, &image);
}

/// `cargo test --profile iter -p voxmar_worldgen settlement_preview -- --ignored --nocapture`
/// (with `SEEDS=42,1`, `RADIUS` blocks around spawn, 3000, `COUNT` a seed, 3, `SCALE`
/// pixels a block, 4) lists the settlements near spawn with their levers and how long
/// planning took, and draws each from above and from the south-east to
/// `target/settlement_SEED_N_{top,iso}.ppm`: for judging them without the game.
#[test]
#[ignore]
fn settlement_preview() {
    let env = |name: &str| std::env::var(name).ok();
    let seeds: Vec<u64> = env("SEEDS").map_or(vec![42, 1, 7], |s| s.split(',').filter_map(|s| s.parse().ok()).collect());
    let radius: i32 = env("RADIUS").and_then(|s| s.parse().ok()).unwrap_or(3000);
    let count: usize = env("COUNT").and_then(|s| s.parse().ok()).unwrap_or(3);
    // `ONLY=Kinfell,villa,highland` draws only the settlements with those names, styles or
    // culture families.
    let names: Option<Vec<String>> = env("ONLY").map(|s| s.split(',').map(str::to_string).collect());
    let scale: f32 = env("SCALE").and_then(|s| s.parse().ok()).unwrap_or(4.0);
    // Every package's blocks, as the game loads them, so lamps, hearths and fields show.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    for seed in seeds {
        let world = Overworld::with_content(seed, registry.clone(), &world_content);
        let spawn = world.spawn_point();
        let start = std::time::Instant::now();
        let slots = world.settlements.near(&world, IVec2::new(spawn.x, spawn.z), radius);
        let cells = {
            let (a, b) = (world.settlements.cell_of(spawn.x - radius, spawn.z - radius), world.settlements.cell_of(spawn.x + radius, spawn.z + radius));
            (b.x - a.x + 1) * (b.y - a.y + 1)
        };
        println!("seed {seed}: {} settlements in {cells} squares, {:.0} ms", slots.len(), start.elapsed().as_secs_f64() * 1000.0);
        let mut plans: Vec<_> = slots.iter().filter_map(|s| s.0.as_ref()).collect();
        plans.sort_by_key(|p| (p.centre - IVec2::new(spawn.x, spawn.z)).length_squared());
        for (i, p) in plans.iter().enumerate() {
            let l = p.levers.levers;
            println!(
                "  {} / {}  {} {} {} in {}  at {}, {}, {}  {} buildings  wealth {:.2} age {:.2} order {:.2} crowding {:.2} green {:.2}",
                p.name.0, p.name.1, p.size, p.layout, p.style, world.biome_at(p.centre.x, p.centre.y).name, p.centre.x, p.height, p.centre.y, p.buildings.len(), l[0], l[1], l[2], l[3], l[4]
            );
            let info = world.settlements.culture_info(p);
            let axes: Vec<String> = super::culture::Axis::ALL.iter().zip(info.axes).map(|(a, v)| format!("{} {v:.2}", a.name())).collect();
            println!(
                "    culture {} ({}){}, quirks [{}]  {}\n    dropped [{}]  borrowed [{}]",
                info.family,
                info.title.0,
                info.second.as_ref().map_or(String::new(), |s| format!(", second {} {:.2}", s.0, s.1)),
                info.quirks.iter().map(|q| q.1.0.as_str()).collect::<Vec<_>>().join(", "),
                axes.join(" "),
                info.dropped.join(", "),
                info.borrowed.join(", ")
            );
            if let Some(block) = p.wall_block {
                println!("    walled: {} columns of {}", p.walls.len(), registry.def(block).name);
            }
            let named = names.as_ref().is_some_and(|n| n.iter().any(|n| p.name.0.contains(n.as_str()) || p.style == *n || info.family == *n));
            if !(named || names.is_none() && i < count) {
                continue;
            }
            let kinds: Vec<&str> = p.buildings.iter().map(|b| b.what.as_str()).collect();
            println!("    {}", kinds.join(", "));
            let (min, max) = (p.min - 6, p.max + 6);
            let start = std::time::Instant::now();
            let area = Area::new(&world, min, max);
            println!("    generated in {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
            if env("BLOCKS").is_some() {
                // What stands four or more blocks over the square: roofs, mostly.
                let mut counts: rustc_hash::FxHashMap<String, usize> = Default::default();
                for z in min.y..=max.y {
                    for x in min.x..=max.x {
                        for y in p.height + 4..p.height + 20 {
                            let b = area.block(IVec3::new(x, y, z));
                            if !b.is_air() {
                                *counts.entry(registry.def(b).name.clone()).or_default() += 1;
                            }
                        }
                    }
                }
                let mut counts: Vec<_> = counts.into_iter().collect();
                counts.sort_by_key(|c| std::cmp::Reverse(c.1));
                println!("    {:?}", &counts[..counts.len().min(12)]);
            }
            let dir = format!("{}/../../target", env!("CARGO_MANIFEST_DIR"));
            top_down(&area, &registry, min, max, 3, &format!("{dir}/settlement_{seed}_{i}_top.ppm"));
            let low = p.height - 12;
            let high = (p.height + 28).min(WORLD_HEIGHT - 1);
            isometric(&area, &registry, min, max, low, high, scale, &format!("{dir}/settlement_{seed}_{i}_iso.ppm"));
        }
        // `FOCUS=bakery,Windmill` (or `all`) draws the first of each such building close
        // up, to `target/building_SEED_WHAT.ppm`.
        if let Some(focus) = env("FOCUS") {
            let wanted: Vec<&str> = focus.split(',').collect();
            let mut drawn: Vec<String> = Vec::new();
            let dir = format!("{}/../../target", env!("CARGO_MANIFEST_DIR"));
            for p in &plans {
                for b in &p.buildings {
                    if drawn.contains(&b.what) || !(wanted.contains(&"all") || wanted.contains(&b.what.as_str())) {
                        continue;
                    }
                    drawn.push(b.what.clone());
                    let (min, max) = (b.min - 3, b.max + 3);
                    let area = Area::new(&world, min, max);
                    isometric(&area, &registry, min, max, b.floor - 3, (b.floor + 22).min(WORLD_HEIGHT - 1), 9.0, &format!("{dir}/building_{seed}_{}.ppm", b.what));
                }
            }
            println!("  drew {}", drawn.join(", "));
        }
    }
}


fn base_registry() -> std::sync::Arc<BlockRegistry> {
    std::sync::Arc::new(crate::test_registry())
}

pub(super) fn base_world(seed: u64) -> Overworld {
    Overworld::new(seed, base_registry())
}

/// One world per seed for the whole test binary, for the tests that only read a world: a
/// settlement's plan is made once however many tests look at it, and planning is most of
/// what these tests cost. The same worlds as the generator's own tests read
/// (`crate::tests::shared_world`): they plan the settlements round spawn too. Tests that
/// count what a world plans, or that compare a fresh world with another, use `base_world`.
fn shared_world(seed: u64) -> std::sync::Arc<Overworld> {
    crate::tests::shared_world(seed)
}

/// A settlement, its graded ground and its buildings are the same whichever column or
/// square is asked for first, and on whichever thread.
#[test]
fn settlements_never_depend_on_order() {
    let (a, b) = (base_world(7), base_world(7));
    let spawn = a.spawn_point();
    let found = base_world(7).settlements_near(IVec2::new(spawn.x, spawn.z), 3000);
    let near = found.iter().min_by_key(|s| (s.centre.x - spawn.x).pow(2) + (s.centre.z - spawn.z).pow(2)).expect("a settlement near spawn");
    let corner = IVec2::new(near.centre.x, near.centre.z) - 24;
    let area: Vec<(i32, i32)> = (0..48 * 48).map(|i| (corner.x + i % 48, corner.y + i / 48)).collect();
    let forward: Vec<_> = area.iter().map(|&(x, z)| a.geo(x, z)).collect();
    let column = ColumnPos::of_block(near.centre);
    let built = a.generate(column);
    // The other world plans other squares first, on several threads.
    std::thread::scope(|s| {
        s.spawn(|| b.settlements_near(IVec2::new(spawn.x + 2500, spawn.z - 2500), 1200));
        s.spawn(|| b.geo(near.centre.x + 700, near.centre.z + 300));
    });
    let backward: Vec<_> = area.iter().rev().map(|&(x, z)| b.geo(x, z)).collect();
    assert!(forward.iter().eq(backward.iter().rev()));
    assert_eq!(built, b.generate(column));
    let again = b.settlements_near(IVec2::new(spawn.x, spawn.z), 3000);
    assert_eq!(found.len(), again.len());
    for (x, y) in found.iter().zip(&again) {
        assert_eq!(x, y, "settlement facts, including culture and axes, depend on order");
    }
}

/// Every door opens onto air, inside and out, with a floor under it; no building stands
/// in water; and every settlement is named in English and Japanese.
#[test]
fn settlements_stand_dry_with_open_doors() {
    let mut failures = Vec::new();
    let mut doors = 0;
    for seed in [1u64, 42] {
        let registry = base_registry();
        let world = shared_world(seed);
        let spawn = world.spawn_point();
        // The settlements to look over: the squares round spawn hold a few.
        let found = world.settlements_near(IVec2::new(spawn.x, spawn.z), 3000);
        assert!(!found.is_empty(), "seed {seed}: no settlement within 3000 blocks of spawn");
        for s in found.iter().take(3) {
            assert!(!s.name.0.is_empty() && !s.name.1.is_empty(), "{:?}", s.name);
            let mut columns: rustc_hash::FxHashMap<ColumnPos, Column> = Default::default();
            let mut block = |p: IVec3| {
                let c = ColumnPos::of_block(p);
                columns.entry(c).or_insert_with(|| world.generate(c)).block(IVec3::new(p.x & 31, p.y, p.z & 31))
            };
            for b in &s.buildings {
                // Over the middle of its floor: not water.
                let middle = (b.min + b.max) / 2;
                let over = block(IVec3::new(middle.x, b.floor + 1, middle.y));
                if registry.def(over).name.contains("water") {
                    failures.push(format!("seed {seed} {}: water in the {} at {middle}", s.name.0, b.kind.name()));
                }
                let Some(door) = b.door else { continue };
                doors += 1;
                let step = b.facing.step();
                let out = door + IVec3::new(step.x, 0, step.y);
                let inside = door - IVec3::new(step.x, 0, step.y);
                for (place, p) in [("in", door), ("in", door + IVec3::Y), ("before", out), ("before", out + IVec3::Y), ("behind", inside), ("behind", inside + IVec3::Y)] {
                    let def = registry.def(block(p));
                    if def.solid {
                        failures.push(format!("seed {seed} {}: the {}'s door at {door} facing {:?} is blocked {place} it at {p} by {}", s.name.0, b.kind.name(), b.facing, def.name));
                    }
                }
                if !registry.def(block(door - IVec3::Y)).solid {
                    failures.push(format!("seed {seed} {}: no floor under the {}'s door at {door}", s.name.0, b.kind.name()));
                }
            }
        }
    }
    assert!(doors > 10, "only {doors} doors");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Wells hold water to their brim, uncapped by paving; fields grow only the crops their
/// size allows (none rarer than uncommon), and no field or tree in a settlement is ripe:
/// what a settlement grows is its grower's.
#[test]
fn settlement_wells_hold_water_and_crops_are_everyday() {
    let registry = base_registry();
    let content = crate::WorldContent::base(&registry).unwrap();
    let def = &content.settlements;
    let level = |size: &str| def.sizes.iter().find(|s| s.name == size).map_or(0, |s| s.crop_level);
    let mut failures = Vec::new();
    let (mut wells, mut planted) = (0, 0);
    for seed in [1u64, 42, 7] {
        let world = shared_world(seed);
        let spawn = world.spawn_point();
        for s in world.settlements_near(IVec2::new(spawn.x, spawn.z), 3000).iter().take(6) {
            let mut columns: rustc_hash::FxHashMap<ColumnPos, Column> = Default::default();
            let mut block = |p: IVec3| {
                let c = ColumnPos::of_block(p);
                columns.entry(c).or_insert_with(|| world.generate(c)).block(IVec3::new(p.x & 31, p.y, p.z & 31))
            };
            let most = level(&s.size);
            for b in &s.buildings {
                if b.kind.name() == "well" {
                    wells += 1;
                    for z in b.min.y + 1..b.max.y {
                        for x in b.min.x + 1..b.max.x {
                            let top = registry.def(block(IVec3::new(x, b.floor, z))).name.clone();
                            if !top.contains("water") {
                                failures.push(format!("seed {seed} {}: the well at {} holds {top} at its brim", s.name.0, b.min));
                            }
                        }
                    }
                }
                for z in b.min.y..=b.max.y {
                    for x in b.min.x..=b.max.x {
                        for y in b.floor - 1..=b.floor + 6 {
                            let name = registry.def(block(IVec3::new(x, y, z))).name.clone();
                            let bare = name.split_once(':').map_or(name.as_str(), |(_, n)| n);
                            // Every crop's stages are in crops.ron, the last ripe.
                            let Some((_, crop, stage)) = def.stage_of(&name) else { continue };
                            let (need, rarity, ripe) = (crop.level, crop.rarity, stage + 1 == crop.stages.len());
                            // Sown crops are the fields'; wild ones (plants.ron) may stand ripe by a house.
                            if bare.contains("_crop") && b.kind.name() != "field" {
                                continue;
                            }
                            planted += 1;
                            if ripe {
                                failures.push(format!("seed {seed} {}: ripe {bare} in the {} at {x} {y} {z}", s.name.0, b.what));
                            } else if need > most || rarity > voxmar_engine::block::Rarity::Uncommon {
                                failures.push(format!("seed {seed} {} ({}): {bare} needs Farming {need}, over {most}", s.name.0, s.size));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(wells >= 6 && planted > 100, "only {wells} wells and {planted} crops looked at");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `SEED=1 X=-2364 Z=742 R=300 cargo test --profile iter -p voxmar_worldgen settlement_at -- --ignored --nocapture`
/// lists the settlements within `R` blocks of a place and where their buildings stand.
#[test]
#[ignore]
fn settlement_at() {
    let env = |name: &str| std::env::var(name).ok().and_then(|s| s.parse::<i32>().ok());
    let world = base_world(env("SEED").unwrap_or(1) as u64);
    for s in world.settlements_near(IVec2::new(env("X").unwrap(), env("Z").unwrap()), env("R").unwrap_or(300)) {
        println!("{} {} {} at {} min {:?}", s.name.0, s.size, s.style, s.centre, s.buildings.iter().map(|b| (b.kind.name(), b.min)).collect::<Vec<_>>());
        let mut who: std::collections::BTreeMap<&str, u32> = Default::default();
        for r in &s.people {
            *who.entry(r.species.as_str()).or_default() += 1;
        }
        println!("  {} people: {who:?}", s.people.len());
    }
}


/// A canvas laid on flat grass at y = 0, as generation would lay it.
struct Model {
    blocks: rustc_hash::FxHashMap<IVec3, BlockId>,
}

impl Model {
    fn new(canvas: super::canvas::Canvas, registry: &BlockRegistry, min: IVec2, max: IVec2) -> Self {
        use super::canvas::Put;
        let id = |n: &str| registry.id(n).unwrap();
        let mut blocks: rustc_hash::FxHashMap<IVec3, BlockId> = Default::default();
        for z in min.y..=max.y {
            for x in min.x..=max.x {
                blocks.insert(IVec3::new(x, 0, z), id("base:grass_block"));
                blocks.insert(IVec3::new(x, -1, z), id("base:dirt"));
            }
        }
        let soft = |blocks: &rustc_hash::FxHashMap<IVec3, BlockId>, p: IVec3| {
            blocks.get(&p).is_none_or(|&b| b.is_air() || !registry.def(b).solid || registry.def(b).render == RenderKind::Leaves)
        };
        for v in canvas.into_columns().into_values().flatten() {
            let pos = v.pos;
            match v.put {
                Put::Set(b) => {
                    blocks.insert(pos, b);
                }
                Put::Clear => {
                    blocks.remove(&pos);
                }
                Put::Soft(b) => {
                    if soft(&blocks, pos) {
                        blocks.insert(pos, b);
                    }
                }
                Put::Down(b) => {
                    let mut p = pos;
                    for _ in 0..10 {
                        if !soft(&blocks, p) {
                            break;
                        }
                        blocks.insert(p, b);
                        p.y -= 1;
                    }
                }
            }
        }
        Model { blocks }
    }
}

impl Look for Model {
    fn block(&self, p: IVec3) -> BlockId {
        self.blocks.get(&p).copied().unwrap_or(BlockId::AIR)
    }

    fn color(&self, registry: &BlockRegistry, p: IVec3) -> Vec3 {
        let def = registry.def(self.block(p));
        match def.tint.drawn() {
            Tint::Foliage => Vec3::new(70.0, 120.0, 45.0),
            Tint::Grass => Vec3::new(105.0, 150.0, 70.0),
            Tint::Water => Vec3::new(60.0, 100.0, 200.0),
            Tint::None => Vec3::new(def.color[0] as f32, def.color[1] as f32, def.color[2] as f32),
        }
    }
}

/// `cargo test --profile iter -p voxmar_worldgen house_catalogue -- --ignored --nocapture`
/// draws every house in settlements.ron, and every roof and extra on some house, on flat
/// grass in rows, fronts and then backs to the south-east, to
/// `target/houses_{front,back}_ROW_N.ppm`: for judging the drawing of each piece; and a
/// row of houses in each style's own materials to `target/houses_style_STYLE_SIDE.ppm`.
#[test]
#[ignore]
fn house_catalogue() {
    use super::def::{Extra, Form, Roof};
    use super::house::{self, Kind, Mats, Spec};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let def = &world_content.settlements;
    let fixed = super::fixed(&registry, def).unwrap();
    let id = |n: &str| registry.id(&format!("base:{n}")).unwrap();
    let palette = |n: &str| super::canvas::Palette(vec![(id(n), 1.0)]);
    let mats = Mats {
        foundation: palette("cobblestone"),
        floor: id("spruce_planks"),
        wall: id("polished_calcite"),
        frame: id("dark_oak_log"),
        roof: id("dark_oak_planks"),
        trim: id("spruce_planks"),
        window: Some(id("old_glass")),
        weathered: palette("mossy_cobblestone"),
        hedge: palette("oak_leaves"),
        log: id("oak_log"),
        chimney: id("cobblestone"),
        crops: Vec::new(),
        fruit: palette("apple_leaves_2"), roof_pieces: Vec::new(), pieced: 0.0, jetty: None,
    };
    // Each house with the roof and extras it is shown with.
    use Extra::*;
    let shown: Vec<(&str, Option<Roof>, Vec<Extra>)> = vec![
        ("cottage", Some(Roof::Gable), vec![Chimney, Dormer, Shutters, FlowerBoxes, Lamp]),
        ("cottage", Some(Roof::HalfHip), vec![Weathervane, RainBarrel, HangingHerbs, Woodpile]),
        ("town_house", Some(Roof::Saltbox), vec![Balcony, Shutters, SignBoard]),
        ("farm_house", Some(Roof::Gambrel), vec![OutsideStair, Dovecote, Awning]),
        ("hall", Some(Roof::Hip), vec![Chimney, Dormer, Moss, Porch]),
        ("wing_house", Some(Roof::Gable), vec![Wing, Shed]),
        ("tower_house", Some(Roof::Conical), vec![Weathervane]),
        ("hut", Some(Roof::Shed), vec![Woodpile]),
        ("long_house", Some(Roof::Flat), vec![Chimney]),
        ("roundhouse", None, vec![Chimney]),
        ("a_frame", None, vec![Chimney]),
        ("courtyard_house", None, vec![Chimney]),
        ("shop_house", Some(Roof::Gable), vec![Dormer, SignBoard]),
    ];
    let mut rows: Vec<Vec<(String, Option<Roof>, Vec<Extra>)>> = vec![shown.iter().map(|(n, r, e)| (n.to_string(), *r, e.clone())).collect()];
    rows.push(def.houses.iter().filter(|h| h.work.is_some()).map(|h| (h.name.clone(), None, Vec::new())).collect());
    let out = format!("{}/../../target", env!("CARGO_MANIFEST_DIR"));
    // Every prop the code draws in a row, then every centrepiece.
    {
        use super::def::{CentreKind, PropKind};
        let mut canvas = super::canvas::Canvas::default();
        let mut x = 0;
        for p in &def.props {
            if let PropKind::Drawn(prop) = p.kind {
                let (w, d) = prop.extent();
                let frame = super::canvas::Frame::fit(IVec2::ZERO, IVec2::new(w - 1, d - 1), super::canvas::Facing::North, IVec2::new(x, 0), 0);
                super::plan::draw_prop(&fixed, &mats, &mut canvas, &frame, prop, x as u64);
                x += w + 2;
            }
        }
        let (min, max) = (IVec2::new(-2, -2), IVec2::new(x, 5));
        isometric(&Model::new(canvas, &registry, min, max), &registry, min, max, -2, 8, 12.0, &format!("{out}/props.ppm"));
        for (k, c) in def.centrepieces.iter().enumerate() {
            let CentreKind::Drawn(centre) = c.kind else { continue };
            let mut canvas = super::canvas::Canvas::default();
            let (w, d) = centre.extent();
            let spec = Spec {
                kind: Kind::Centrepiece, w, d, wall: 3, storeys: 1, roof: Roof::Gable, pitch: 1, ridge_x: false, extras: Vec::new(), door: 0, shed: 0,
                wing: None, stilts: 0, regular: true, window_chance: 0.0, form: Form::Box, inside: Vec::new(), centre: Some(centre), seed: k as u64,
            };
            let frame = super::canvas::Frame::fit(IVec2::ZERO, IVec2::new(w - 1, d - 1), super::canvas::Facing::South, IVec2::ZERO, 0);
            let named = |part: &str| c.parts.iter().find(|(p, _)| p == part).and_then(|(_, n)| registry.id(&super::def::fill(n, "oak", "oak")));
            let parts = super::centre::Parts::new(&named, &mats, &fixed, 1);
            super::centre::draw(centre, &spec, &frame, &mats, &fixed, &parts, &mut canvas);
            let (min, max) = (IVec2::splat(-2), IVec2::new(w + 1, d + 1));
            isometric(&Model::new(canvas, &registry, min, max), &registry, min, max, -3, 20, 12.0, &format!("{out}/centre_{centre:?}.ppm"));
        }
    }
    for (side, facing) in [("front", super::canvas::Facing::South), ("back", super::canvas::Facing::North)] {
        for (n, row) in rows.iter().enumerate() {
            for (k, (name, roof, extras)) in row.iter().enumerate() {
                let mut canvas = super::canvas::Canvas::default();
                let h = def.houses.iter().find(|h| h.name == *name).unwrap();
                let mut w = h.width.1 as i32;
                let d = if h.form == Form::Round { w } else { h.depth.1 as i32 };
                if h.form == Form::Round && w % 2 == 0 {
                    w -= 1;
                }
                let mut extras = extras.clone();
                extras.extend(h.always.iter().copied());
                let roof = roof.unwrap_or_else(|| match h.form {
                    Form::Round => Roof::Conical,
                    Form::Courtyard => Roof::Hip,
                    _ => h.roofs.first().map_or(Roof::Gable, |r| r.0),
                });
                let spec = Spec {
                    kind: Kind::House,
                    w,
                    d,
                    wall: if h.form == Form::AFrame { 0 } else { h.walls.1 as i32 },
                    storeys: if h.form == Form::AFrame { 1 } else { h.storeys.1 as i32 },
                    roof,
                    pitch: 1,
                    ridge_x: h.form != Form::AFrame && (roof == Roof::Saltbox || w > d || matches!(extras.first(), Some(Dormer))),
                    door: w / 2,
                    shed: if extras.contains(&Shed) { 3 } else { 0 },
                    wing: extras.contains(&Wing).then_some((3, 4)),
                    stilts: h.stilts.unwrap_or(0) as i32,
                    extras,
                    regular: true,
                    window_chance: 0.6,
                    form: h.form,
                    inside: h.inside.clone(),
                    centre: None,
                    seed: k as u64 + 1,
                };
                let (low, high) = spec.envelope();
                let frame = super::canvas::Frame::fit(low, high, facing, IVec2::ZERO, 0);
                house::draw(&spec, &frame, &mats, &fixed, &mut canvas);
                let extent = super::canvas::Frame::extent(low, high, facing);
                let (min, max) = (IVec2::splat(-3), extent + 2);
                let model = Model::new(canvas, &registry, min, max);
                isometric(&model, &registry, min, max, -2, 24, 12.0, &format!("{out}/houses_{side}_{n}_{k}.ppm"));
            }
        }
    }
    // One row per style, in its own materials (the first of each list, its first wood),
    // roofs and stilts: `target/houses_style_STYLE_{front,back}.ppm`.
    for name in ["log_cabin", "turf_house", "mudbrick", "villa", "stilts", "jungle_stilts"] {
        let style = def.styles.iter().find(|s| s.name == name).unwrap();
        let wood = &style.woods[0].0;
        let block = |m: &super::def::Material| registry.id(&super::def::fill(&m.0[0].0, wood, wood)).unwrap();
        let mats = Mats {
            foundation: super::canvas::Palette(style.foundation.0.iter().map(|(n, w)| (registry.id(&super::def::fill(n, wood, wood)).unwrap(), *w)).collect()),
            floor: block(&style.floor),
            wall: block(&style.walls),
            frame: block(&style.frame),
            roof: block(&style.roof),
            trim: block(&style.trim),
            window: style.window.as_deref().and_then(|w| registry.id(w)),
            weathered: palette("mossy_cobblestone"),
            hedge: palette("oak_leaves"),
            log: registry.id(&super::def::fill(&style.log, wood, wood)).unwrap(),
            chimney: block(&style.chimney),
            crops: Vec::new(),
            fruit: palette("apple_leaves_2"), roof_pieces: Vec::new(), pieced: 0.0, jetty: None,
        };
        let row: [(&str, Vec<Extra>); 6] = [
            ("cottage", vec![Chimney, Shutters]),
            ("hut", vec![Woodpile]),
            ("a_frame", vec![]),
            ("roundhouse", vec![Chimney]),
            ("long_house", vec![Chimney, Porch]),
            ("town_house", vec![Balcony]),
        ];
        for (side, facing) in [("front", super::canvas::Facing::South), ("back", super::canvas::Facing::North)] {
            let mut pieces = Vec::new();
            for (k, (house_name, extras)) in row.iter().enumerate() {
                let h = def.houses.iter().find(|h| h.name == *house_name).unwrap();
                let w = h.width.0 as i32 | 1;
                let d = if h.form == Form::Round { w } else { h.depth.0 as i32 };
                let mut extras = extras.clone();
                extras.extend(h.always.iter().copied());
                let roof = match h.form {
                    Form::Round => Roof::Conical,
                    Form::Courtyard => Roof::Hip,
                    Form::AFrame => Roof::Gable,
                    _ => style.roofs.first().map_or(Roof::Gable, |r| r.0),
                };
                let spec = Spec {
                    kind: Kind::House,
                    w,
                    d,
                    wall: if h.form == Form::AFrame { 0 } else { h.walls.0 as i32 },
                    storeys: if h.form == Form::AFrame { 1 } else { h.storeys.1 as i32 },
                    roof,
                    pitch: 1,
                    ridge_x: h.form != Form::AFrame && w > d,
                    door: w / 2,
                    shed: 0,
                    wing: None,
                    stilts: h.stilts.unwrap_or(style.stilts) as i32,
                    extras,
                    regular: true,
                    window_chance: style.window_chance,
                    form: h.form,
                    inside: h.inside.clone(),
                    centre: None,
                    seed: k as u64 + 1,
                };
                let (low, high) = spec.envelope();
                let extent = super::canvas::Frame::extent(low, high, facing);
                let frame = super::canvas::Frame::fit(low, high, facing, IVec2::ZERO, 0);
                let mut canvas = super::canvas::Canvas::default();
                house::draw(&spec, &frame, &mats, &fixed, &mut canvas);
                let (min, max) = (IVec2::splat(-3), extent + 2);
                let piece = format!("{out}/houses_style_{name}_{side}_{k}.ppm");
                isometric(&Model::new(canvas, &registry, min, max), &registry, min, max, -2, 24, 12.0, &piece);
                pieces.push(piece);
            }
            side_by_side(&pieces, &format!("{out}/houses_style_{name}_{side}.ppm"));
        }
    }
}


/// `cargo test --profile iter -p voxmar_worldgen settlement_census -- --ignored --nocapture`
/// counts the settlements over 24 by 24 squares of four worlds (`SEEDS`): how many squares
/// hold one, by size, how many to a square kilometre, how far from spawn the nearest of
/// each size is, and the layouts and styles drawn.
#[test]
#[ignore]
fn settlement_census() {
    let seeds: Vec<u64> = std::env::var("SEEDS").ok().map_or(vec![1, 42, 7, 0x5eed], |s| s.split(',').filter_map(|s| s.parse().ok()).collect());
    let mut nearest: std::collections::BTreeMap<String, Vec<f32>> = Default::default();
    let (mut all_squares, mut all_settled) = (0u32, 0u32);
    for seed in seeds {
        let world = base_world(seed);
        let spacing = world.settlements.def.grid.spacing;
        let mut squares = 0;
        let mut sizes: std::collections::BTreeMap<String, u32> = Default::default();
        let mut layouts: std::collections::BTreeMap<String, u32> = Default::default();
        let mut buildings: std::collections::BTreeMap<String, u32> = Default::default();
        let start = std::time::Instant::now();
        for cz in -12..12 {
            for cx in -12..12 {
                squares += 1;
                let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
                let Some(plan) = &slot.0 else { continue };
                *sizes.entry(plan.size.clone()).or_default() += 1;
                *layouts.entry(plan.layout.clone()).or_default() += 1;
                for b in &plan.buildings {
                    *buildings.entry(b.what.clone()).or_default() += 1;
                }
            }
        }
        let given_up: Vec<u32> = super::plan::GIVEN_UP.iter().map(|c| c.swap(0, std::sync::atomic::Ordering::Relaxed)).collect();
        println!("  given up (no spot, unfit spots, climate, no lanes, no well, missing a must, few homes, no field or pen, unfit land, poor soil): {given_up:?}");
        let fails: Vec<u32> = super::plan::PLACE_FAILS.iter().map(|c| c.swap(0, std::sync::atomic::Ordering::Relaxed)).collect();
        println!("  lots missed (no house fits, out of bounds, too close, on a lane, unfit ground, too steep, off the lane's height): {fails:?}");
        let settled: u32 = sizes.values().sum();
        let km2 = squares as f32 * (spacing as f32 / 1000.0).powi(2);
        println!("seed {seed}: {settled} of {squares} squares settled {sizes:?}, {:.3} a square km, in {:?}, {spacing}", settled as f32 / km2, start.elapsed());
        let spawn = world.spawn_point();
        let from = IVec2::new(spawn.x, spawn.z);
        let near = world.settlements.near(&world, from, 6000);
        for size in ["any", "hamlet", "village", "town"] {
            let d = near.iter().filter_map(|s| s.0.as_ref()).filter(|p| size == "any" || p.size == size).map(|p| (p.centre - from).as_vec2().length()).fold(f32::MAX, f32::min);
            println!("  nearest {size} to spawn: {}", if d == f32::MAX { "none within 6000".to_owned() } else { format!("{d:.0}") });
            nearest.entry(size.to_owned()).or_default().push(d);
        }
        all_squares += squares;
        all_settled += settled;
        println!("  layouts {layouts:?}");
        println!("  buildings {buildings:?}");
    }
    println!("all: {all_settled} of {all_squares} squares settled");
    for (size, mut d) in nearest {
        d.sort_by(f32::total_cmp);
        println!("nearest {size} to spawn: median {:.0}, each {d:.0?}", d[d.len() / 2]);
    }
}

/// How much of the world a new player waits for: finding spawn and making its column
/// plans the settlements and lays the trails round it, and the river regions under both.
/// Trails near settlements plan the land within a trail's length of the gates near
/// spawn, in 128-block patches of their grid, so this is most of the time a fresh server
/// takes to answer; the budgets are what it needs now, and a change needing more must
/// say why here. Counts, not times, so a busy machine cannot fail it.
#[test]
fn spawn_needs_only_so_much_planning() {
    use crate::Generator;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    // Seed, then the most stream-tier river regions, patches of the trails' grid and
    // settlement squares: the server's default seed has trails laid at spawn, 0 has none.
    // Seed 0 plans the squares in reach of spawn first, for the settlement promised there
    // (rules, `spawn_settlement_within`). Since trails near settlements plan only round
    // the gates near spawn, the default seed reads 25 stream regions (34 before) and 19
    // squares (28), and its trails' grid 257 patches of 128 blocks (60 whole regions).
    for (seed, streams, grids, squares) in [(0x5eed, 25, 257, 19), (0, 9, 0, 9)] {
        let world = Overworld::with_content(seed, content.blocks.clone(), &world_content);
        world.generate(ColumnPos::of_block(world.spawn_point()));
        let made = (world.rivers.made().1, world.trails.made().0, world.settlements.made());
        assert!(made.0 <= streams && made.1 <= grids && made.2 <= squares, "seed {seed}: {made:?} river regions, trail grid patches and squares, over {:?}", (streams, grids, squares));
    }
}

/// Servers on one world share one generator (`Overworld::shared`): the second finds the
/// land round spawn planned and plans nothing more; another seed gets a generator of its
/// own, and once nothing holds a world it is let go.
#[test]
fn servers_on_one_world_share_its_planning() {
    use crate::Generator;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let made = |world: &Overworld| (world.rivers.made(), world.trails.made(), world.settlements.made());
    let first = Overworld::shared(0x5eed, &content, &world_content);
    let spawn = ColumnPos::of_block(first.spawn_point());
    let column = first.generate(spawn);
    let planned = made(&first);
    let second = Overworld::shared(0x5eed, &content, &world_content);
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(ColumnPos::of_block(second.spawn_point()), spawn);
    assert_eq!(second.generate(spawn), column);
    assert_eq!(made(&second), planned, "the second server planned more");
    let other = Overworld::shared(0x5eee, &content, &world_content);
    assert!(!std::sync::Arc::ptr_eq(&first, &other));
    let held = std::sync::Arc::downgrade(&first);
    drop((first, second));
    assert!(held.upgrade().is_none(), "a world nothing holds is kept");
}

/// Japanese names run their parts together as the English does: Sab + ir is サビル, not
/// サブイル; a consonant into a consonant, or a vowel into a vowel, joins as it is.
#[test]
fn japanese_names_join_as_they_are_said() {
    let world = shared_world(1);
    let kana = &world.settlements.def.kana;
    let join = |a: (&str, &str), b: (&str, &str)| super::plan::join_kana(kana, &(a.0.into(), a.1.into()), &(b.0.into(), b.1.into()));
    assert_eq!(join(("Sab", "サブ"), ("ir", "イル")), "サビル");
    assert_eq!(join(("Glen", "グレン"), ("ach", "アック")), "グレナック");
    assert_eq!(join(("Oas", "オアス"), ("is", "イス")), "オアシス");
    assert_eq!(join(("Aber", "アバー"), ("ach", "アック")), "アバラック");
    assert_eq!(join(("Ush", "ウシュ"), ("ah", "ア")), "ウシャ");
    assert_eq!(join(("Glen", "グレン"), ("more", "モア")), "グレンモア");
    assert_eq!(join(("Ma", "マ"), ("ana", "アナ")), "マアナ");
    assert_eq!(join(("Haw", "ホー"), ("en", "エン")), "ホーエン");
}

/// What every plan keeps to, over the squares round two seeds: about one square in seven
/// settled, mostly hamlets; each plan inside its square, its buildings apart, enough
/// homes for its size, none in a biome settlements avoid or on a river or lake; and every
/// chapel and wayside shrine with its altar (in snowy land too, where the snow blanket
/// once covered it).
#[test]
fn plans_keep_to_their_squares_and_rules() {
    use crate::trails::Land;
    use super::house::Kind;
    let (mut squares, mut hamlets, mut villages, mut chapels, mut shrines) = (0, 0, 0, 0, 0);
    for seed in [1u64, 42] {
        let world = shared_world(seed);
        let def = &world.settlements.def;
        let spacing = def.grid.spacing;
        let land = world.land();
        let registry = base_registry();
        let altar = registry.id(def.furnishings.altar.as_deref().unwrap()).unwrap();
        for cz in -5..5 {
            for cx in -5..5 {
                squares += 1;
                let cell = IVec2::new(cx, cz);
                let slot = world.settlements.slot(&world, cell);
                let Some(plan) = &slot.0 else { continue };
                let size = def.sizes.iter().find(|s| s.name == plan.size).unwrap();
                if size.name == "hamlet" { hamlets += 1 } else { villages += 1 }
                let at = format!("seed {seed}, {} at {}", plan.name.0, plan.centre);
                assert!(plan.min.cmpge(cell * spacing).all() && plan.max.cmplt(cell * spacing + spacing).all(), "{at}: reaches out of its square");
                let homes = plan.buildings.iter().filter(|b| b.kind == Kind::House && b.work.is_none()).count() as u32;
                assert!(homes >= size.houses.0, "{at}: {homes} homes, fewer than {:?}", size.houses);
                for (i, a) in plan.buildings.iter().enumerate() {
                    for b in &plan.buildings[i + 1..] {
                        let apart = a.max.x < b.min.x || b.max.x < a.min.x || a.max.y < b.min.y || b.max.y < a.min.y;
                        assert!(apart, "{at}: {} at {} overlaps {} at {}", a.what, a.min, b.what, b.min);
                    }
                    let middle = (a.min + a.max) / 2;
                    let biome = world.biome_at(middle.x, middle.y);
                    assert!(!def.avoid.iter().any(|t| biome.has_tag(t)), "{at}: {} in an avoided biome", a.what);
                    if a.what != "boathouse" {
                        for p in [a.min, a.max, IVec2::new(a.min.x, a.max.y), IVec2::new(a.max.x, a.min.y), middle] {
                            let g = land.sample(p.x, p.y);
                            assert!(!g.submerged() && g.river < 0.5, "{at}: {} at {} stands in water ({p})", a.what, a.min);
                        }
                    }
                    let looked = if a.what == "chapel" { &mut chapels } else { &mut shrines };
                    if (a.what == "chapel" || a.what == "shrine") && *looked < 3 {
                        *looked += 1;
                        let area = Area::new(&world, a.min, a.max);
                        let found = (a.min.y..=a.max.y).any(|z| (a.min.x..=a.max.x).any(|x| (a.floor..a.floor + 4).any(|y| area.block(IVec3::new(x, y, z)) == altar)));
                        let planned: Vec<_> = plan.voxels.iter().flat_map(|(column, voxels)| voxels.iter().filter_map(move |v| {
                            matches!(v.put, super::canvas::Put::Set(b) if b == altar).then_some((column, v.pos, v.ground))
                        })).collect();
                        assert!(found, "{at} ({}): the {} at {} floor {} has no altar; planned altars {planned:?}", plan.style, a.what, a.min, a.floor);
                    }
                }
            }
        }
    }
    let settled = hamlets + villages;
    let share = settled as f32 / squares as f32;
    assert!((0.08..=0.25).contains(&share), "{settled} of {squares} squares settled");
    assert!(hamlets > villages && villages > 0, "{hamlets} hamlets, {villages} villages");
    assert!(chapels > 0 && shrines > 0, "{chapels} chapels and {shrines} shrines to look in");
}

/// The settlements of a few squares, held fixed: a change to planning that moves a single
/// building shows here, to be looked at in shots and the hash then renewed.
#[test]
fn settlements_are_unchanged() {
    const EXPECTED: u64 = 13394484238960257129;
    let world = shared_world(1);
    let mut text = String::new();
    for cz in -3..3 {
        for cx in -3..3 {
            let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
            let Some(p) = &slot.0 else { continue };
            text += &format!("{:?} {} {} {} {} {:?}\n", p.name, p.size, p.layout, p.style, p.centre, p.gates);
            for b in &p.buildings {
                text += &format!("  {} {} {} {} {:?} {:?}\n", b.what, b.min, b.max, b.floor, b.door, b.work);
            }
        }
    }
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    assert_eq!(hash, EXPECTED, "settlements changed:\n{text}");
}

/// Every crop a style names is sown in some settlement's fields: they are where a player
/// first finds rye, chickpea, rice and taro. Slow (seed 1 finds chickpea, grown only in
/// warm villages, 19 squares out), so run by hand:
/// `cargo test --profile iter -p voxmar_worldgen every_style_crop -- --ignored --nocapture`.
#[test]
#[ignore]
fn every_style_crop_is_sown_somewhere() {
    use super::canvas::Put;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let world = Overworld::with_content(1, content.blocks.clone(), &world_content);
    let registry = &content.blocks;
    let mut wanted: std::collections::BTreeSet<String> = world.settlements.def.styles.iter().flat_map(|s| s.crops.iter().map(|(c, _)| c.clone())).collect();
    let mut seen = 0;
    'search: for ring in 0..40 {
        for step in 0..(ring * 8).max(1) {
            let (cx, cz) = crate::spiral(ring, step);
            let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
            let Some(plan) = &slot.0 else { continue };
            seen += 1;
            for v in plan.voxels.values().flatten() {
                if let Put::Set(b) = v.put {
                    let name = &registry.def(b).name;
                    if let Some(crop) = name.strip_suffix("_crop_0").or_else(|| name.strip_suffix("_crop_1")).or_else(|| name.strip_suffix("_crop_2")) {
                        wanted.remove(crop);
                    }
                }
            }
            if wanted.is_empty() {
                println!("every style crop sown within {seen} settlements, {ring} squares out");
                break 'search;
            }
        }
    }
    assert!(wanted.is_empty(), "no field in {seen} settlements grows {wanted:?}");
}

/// How far out from the origin each regional crop is first sown in a field, in squares
/// (rings of the grid), on seeds 1, 7 and 42, with biome levels (P15) and without them
/// (every biome level 1 and no lower chance above level 3, as before levels): design asked
/// that preferring gentle land leave no crop's nearest field more than a few squares
/// further out (draft 95). Slow, by hand:
/// `cargo test --profile iter -p voxmar_worldgen regional_crop_reach -- --ignored --nocapture`
/// (`SEEDS=1,7`, and `LEVELS=gentle,chance` to try another `site.levels`).
#[test]
#[ignore]
fn regional_crop_reach() {
    use super::canvas::Put;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let mut leveled = crate::content::WorldContent::load(&content).unwrap();
    if let Some((gentle, chance)) = std::env::var("LEVELS").ok().and_then(|v| v.split_once(',').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))) {
        leveled.settlements.site.levels = (gentle, chance);
    }
    let mut plain = leveled.clone();
    for biome in &mut plain.biomes {
        biome.level = 1;
    }
    plain.settlements.site.levels.1 = 1.0;
    let crops = ["base:rye", "base:chickpea", "base:rice", "base:taro"];
    let registry = &content.blocks;
    println!("seed | {} (squares out, without levels -> with)", crops.join(", "));
    let seeds: Vec<u64> = std::env::var("SEEDS").map_or(vec![1, 7, 42], |v| v.split(',').map(|s| s.parse().unwrap()).collect());
    for seed in seeds {
        let mut rings = Vec::new();
        for world_content in [&plain, &leveled] {
            let world = Overworld::with_content(seed, content.blocks.clone(), world_content);
            let mut first: [Option<i32>; 4] = [None; 4];
            'search: for ring in 0..40 {
                for step in 0..(ring * 8).max(1) {
                    let (cx, cz) = crate::spiral(ring, step);
                    let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
                    let Some(plan) = &slot.0 else { continue };
                    for v in plan.voxels.values().flatten() {
                        if let Put::Set(b) = v.put {
                            let name = &registry.def(b).name;
                            if let Some(crop) = name.strip_suffix("_crop_0").or_else(|| name.strip_suffix("_crop_1")).or_else(|| name.strip_suffix("_crop_2"))
                                && let Some(i) = crops.iter().position(|c| *c == crop)
                            {
                                first[i].get_or_insert(ring);
                            }
                        }
                    }
                    if first.iter().all(Option::is_some) {
                        break 'search;
                    }
                }
            }
            rings.push(first);
        }
        let cells: Vec<String> = (0..4).map(|i| format!("{:?} -> {:?}", rings[0][i], rings[1][i])).collect();
        println!("{seed} | {}", cells.join(", "));
    }
}

/// No lane runs through the well or a centrepiece, or within the ring of one block round
/// them: lanes stop at the square's edge or go round.
#[test]
fn lanes_keep_off_wells_and_centrepieces() {
    use super::house::Kind;
    let mut looked = 0;
    for seed in [1u64, 42, 7] {
        let world = shared_world(seed);
        for cz in -5..5 {
            for cx in -5..5 {
                let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
                let Some(p) = &slot.0 else { continue };
                for b in p.buildings.iter().filter(|b| matches!(b.kind, Kind::Well | Kind::Centrepiece)) {
                    looked += 1;
                    let ring = |q: Vec2| {
                        let (lo, hi) = (b.min.as_vec2() - 1.5, b.max.as_vec2() + 1.5);
                        (lo - q).max(q - hi).max(Vec2::ZERO).length()
                    };
                    for (lane, half) in p.lanes.iter().zip(&p.lane_halves) {
                        let near = lane.windows(2).flat_map(|w| (0..=12).map(move |k| w[0].lerp(w[1], k as f32 / 12.0))).map(ring).fold(f32::MAX, f32::min);
                        assert!(near >= *half, "seed {seed}, {}: a lane runs within {near:.1} of the {} at {}", p.name.0, b.what, b.min);
                    }
                }
            }
        }
    }
    assert!(looked > 20, "only {looked} wells and centrepieces");
}

/// Chimneys stand inside the walls, one stone all the way up from the hearth, and top out
/// one or two blocks over the roof beside them, whatever the house, roof and ridge; every
/// style names what its chimneys are built of.
#[test]
fn chimneys_stand_inside_and_just_clear_the_roof() {
    use super::def::{Extra, Form, Roof};
    use super::house::{self, Kind, Mats, Spec};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let def = &world_content.settlements;
    for style in &def.styles {
        assert!(!style.chimney.0.is_empty(), "the {} style names no chimney stone", style.name);
    }
    let fixed = super::fixed(&registry, def).unwrap();
    let id = |n: &str| registry.id(&format!("base:{n}")).unwrap();
    let palette = |n: &str| super::canvas::Palette(vec![(id(n), 1.0)]);
    let (roof_block, stack) = (id("dark_oak_planks"), id("cobbled_granite"));
    let mats = Mats {
        foundation: palette("cobblestone"),
        floor: id("spruce_planks"),
        wall: id("polished_calcite"),
        frame: id("dark_oak_log"),
        roof: roof_block,
        trim: id("spruce_planks"),
        window: Some(id("old_glass")),
        weathered: palette("mossy_cobblestone"),
        hedge: palette("oak_leaves"),
        log: id("oak_log"),
        chimney: stack,
        crops: Vec::new(),
        fruit: palette("apple_leaves_2"), roof_pieces: Vec::new(), pieced: 0.0, jetty: None,
    };
    let mut drawn = 0;
    for (n, h) in def.houses.iter().enumerate() {
        let roofs: Vec<Roof> = match h.form {
            Form::Round => vec![Roof::Conical, Roof::Flat],
            Form::Courtyard => vec![Roof::Hip, Roof::Flat],
            Form::AFrame => vec![Roof::Gable],
            _ => vec![Roof::Gable, Roof::HalfHip, Roof::Saltbox, Roof::Gambrel, Roof::Hip, Roof::Flat, Roof::Shed, Roof::Conical],
        };
        let boxy = matches!(h.form, Form::Box | Form::ShopFront);
        for (k, &roof) in roofs.iter().enumerate() {
            for (ridge_x, wing, size) in [(true, false, 0), (false, false, 1), (false, true, 1), (true, true, 0)] {
                if (wing && !boxy) || (h.form == Form::AFrame && ridge_x) {
                    continue;
                }
                let mut w = if size == 0 { h.width.0 } else { h.width.1 } as i32;
                let d = if h.form == Form::Round { w } else if size == 0 { h.depth.1 as i32 } else { h.depth.0 as i32 };
                if h.form == Form::Round && w % 2 == 0 {
                    w -= 1;
                }
                let mut extras = vec![Extra::Chimney];
                extras.extend(h.always.iter().copied());
                let spec = Spec {
                    kind: Kind::House,
                    w,
                    d,
                    wall: if h.form == Form::AFrame { 0 } else { h.walls.1 as i32 },
                    storeys: if h.form == Form::AFrame { 1 } else { h.storeys.1 as i32 },
                    roof,
                    pitch: 1,
                    ridge_x,
                    door: w / 2,
                    shed: 0,
                    wing: wing.then_some((3, 4)).filter(|(ww, _)| *ww < w - 1),
                    stilts: h.stilts.unwrap_or(0) as i32,
                    extras,
                    regular: true,
                    window_chance: 0.6,
                    form: h.form,
                    inside: h.inside.clone(),
                    centre: None,
                    seed: (n * 31 + k) as u64,
                };
                let (low, high) = spec.envelope();
                let frame = super::canvas::Frame::fit(low, high, super::canvas::Facing::North, low, 0);
                let mut canvas = super::canvas::Canvas::default();
                house::draw(&spec, &frame, &mats, &fixed, &mut canvas);
                let is = |p: &IVec3, b: BlockId| matches!(canvas.absolute.get(p), Some(super::canvas::Put::Set(x)) if *x == b);
                if spec.inside.contains(&super::def::Fitting::Altar) {
                    let altar = fixed.altar.expect("base altar");
                    assert!(canvas.absolute.values().any(|put| matches!(put, super::canvas::Put::Set(b) if *b == altar)), "chapel altar lost with {roof:?}, ridge {ridge_x}, wing {wing}");
                    assert_ne!(spec.chimney().map(|c| c.0), Some((w / 2, d - 2)), "a chimney must keep off its altar");
                }
                let cells: Vec<IVec3> = canvas.absolute.keys().filter(|p| is(p, stack)).copied().collect();
                let what = format!("{} with a {roof:?} roof, ridge {}, {w} by {d}{}", h.name, if ridge_x { "across" } else { "back" }, if wing { ", a wing" } else { "" });
                let Some(first) = cells.first() else { continue };
                drawn += 1;
                for c in &cells {
                    assert!((c.x, c.z) == (first.x, first.z), "{what}: the chimney is more than one column");
                    assert!(c.x >= 1 && c.x <= w - 2 && c.z >= 1 && c.z <= d - 2, "{what}: the chimney stands at {c}, outside the walls");
                }
                let (bottom, top) = (cells.iter().map(|c| c.y).min().unwrap(), cells.iter().map(|c| c.y).max().unwrap());
                assert_eq!(cells.len() as i32, top - bottom + 1, "{what}: the chimney has gaps");
                // The roof beside it, and anything on the roof there (a flat roof's parapet).
                let top_of = |x: i32, z: i32, only: Option<BlockId>| canvas.absolute.keys().filter(|p| p.x == x && p.z == z && only.is_none_or(|b| is(p, b))).map(|p| p.y).max();
                let beside = |only| [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().filter_map(|&(dx, dz)| top_of(first.x + dx, first.z + dz, only)).max();
                let (roof, any) = (beside(Some(roof_block)).unwrap_or_else(|| panic!("{what}: no roof beside the chimney")), beside(None).unwrap());
                assert!(top - roof <= 2 && top > any, "{what}: the chimney tops out at {top}, the roof beside it at {roof}, anything beside at {any}");
            }
        }
    }
    assert!(drawn > 100, "only {drawn} chimneys drawn");
}

/// Numbers a mod could set that would hang planning are refused on load: lamps along lanes
/// that come no further apart.
#[test]
fn numbers_that_would_hang_planning_are_refused() {
    let registry = base_registry();
    let mut def = super::SettlementsDef::default();
    def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
    def.cultures.extend("base", "base", include_str!("../../../../assets/content/base/cultures.ron")).unwrap();
    def.check(&registry).unwrap();
    let refused = |change: &dyn Fn(&mut super::SettlementsDef)| {
        let mut d = def.clone();
        change(&mut d);
        d.check(&registry).is_err()
    };
    assert!(refused(&|d| d.rules.dressing.every = (0.0, 0.0)), "lamps every (0, 0)");
    assert!(refused(&|d| d.rules.dressing.every = (-4.0, 14.0)), "lamps every (-4, 14)");
    assert!(refused(&|d| d.rules.dressing.every = (9.0, f32::NAN)), "lamps every (9, NaN)");
    assert!(refused(&|d| d.sizes[0].lamps = 0.0), "a size's lamps 0");
    assert!(refused(&|d| d.sizes[0].lamps = 0.01), "a size's lamps 0.01");
}

/// Each number a mod could set that planning cannot work with is refused on load.
#[test]
fn numbers_planning_cannot_use_are_refused() {
    use super::def::Form;
    let registry = base_registry();
    let mut def = super::SettlementsDef::default();
    def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
    def.cultures.extend("base", "base", include_str!("../../../../assets/content/base/cultures.ron")).unwrap();
    def.check(&registry).unwrap();
    let refused = |change: &dyn Fn(&mut super::SettlementsDef)| {
        let mut d = def.clone();
        change(&mut d);
        d.check(&registry).is_err()
    };
    let round = def.houses.iter().position(|h| h.form == Form::Round).unwrap();
    let a_frame = def.houses.iter().position(|h| h.form == Form::AFrame).unwrap();
    let courtyard = def.houses.iter().position(|h| h.form == Form::Courtyard).unwrap();
    type Change<'a> = &'a dyn Fn(&mut super::SettlementsDef);
    let cases: [(&str, Change); 21] = [
        ("a round house of two storeys", &|d| d.houses[round].storeys = (1, 2)),
        ("an A-frame of two storeys", &|d| d.houses[a_frame].storeys = (2, 2)),
        ("a courtyard house of two storeys", &|d| d.houses[courtyard].storeys = (1, 2)),
        ("a trader 7 to 5 across", &|d| d.rules.houses.trader.0 = (7, 5)),
        ("a trader 3 across", &|d| d.rules.houses.trader.0 = (3, 6)),
        ("a trader 20 deep", &|d| d.rules.houses.trader.1 = (5, 20)),
        ("an inn of 4 storeys", &|d| d.rules.houses.inn.0 = 4),
        ("an inn 13 across", &|d| d.rules.houses.inn.1 = 13),
        ("an inn 17 deep", &|d| d.rules.houses.inn.2 = 17),
        ("lots no gap apart", &|d| d.rules.lots.gap.0 = 0),
        ("side lanes no width", &|d| d.rules.lanes.side = 0.0),
        ("lanes no way apart", &|d| d.rules.lanes.apart = 0.0),
        ("a green past 16", &|d| d.rules.lanes.green = (3.0, 4)),
        ("water looked for 300 out", &|d| d.site.water = 300),
        ("a riverside street looking 300 out", &|d| d.rules.lanes.riverside = 300),
        ("levers steep at nothing", &|d| d.rules.levers.steep = 0.0),
        ("the spawn's square trying no spots", &|d| d.rules.spawn_tries = 0),
        ("the spawn's square trying 33 times the spots", &|d| d.rules.spawn_tries = 33),
        ("the spawn's square laying out with no draws", &|d| d.rules.spawn_salts = 0),
        ("the spawn's square laying out with 33 draws", &|d| d.rules.spawn_salts = 33),
        ("fertility over 1", &|d| d.site.fertility = 1.5),
    ];
    for (what, change) in cases {
        assert!(refused(change), "{what} is let through");
    }
    assert!(refused(&|d| { d.site.poor.insert("cold".into(), -0.2); }), "a poor fertility under nothing is let through");
    assert!(refused(&|d| d.rules.houses.grand = 13), "an inn with no house grand enough is let through");
}

/// On a slope the ground is cut back level round a building and eased out beyond, so no
/// wall stands buried: the land a block out from each building's box (porch, shed and eaves
/// included) is at most a block over its floor, in the worlds as generated.
#[test]
fn slopes_never_bury_walls() {
    use super::house::Kind;
    let registry = base_registry();
    let mut failures = Vec::new();
    let mut looked = 0;
    for seed in [0u64, 1, 42, 7] {
        let world = shared_world(seed);
        let spawn = world.spawn_point();
        let mut columns: rustc_hash::FxHashMap<ColumnPos, Column> = Default::default();
        for s in world.settlements_near(IVec2::new(spawn.x, spawn.z), 3000).iter().take(4) {
            for b in s.buildings.iter().filter(|b| matches!(b.kind, Kind::House | Kind::Inn | Kind::Trader | Kind::Smithy)) {
                let (lo, hi) = (b.min - 1, b.max + 1);
                for z in lo.y..=hi.y {
                    for x in lo.x..=hi.x {
                        if x != lo.x && x != hi.x && z != lo.y && z != hi.y {
                            continue;
                        }
                        let c = ColumnPos::of_block(IVec3::new(x, 0, z));
                        let col = columns.entry(c).or_insert_with(|| world.generate(c));
                        // The land: soil, or natural stone; not a woodpile or a wall.
                        let land = |y: i32| {
                            let d = registry.def(col.block(IVec3::new(x & 31, y, z & 31)));
                            d.solid && (d.has_tag("soil") || d.has_tag("surface") || (d.has_tag("stone") && d.has_tag("natural") && !d.has_tag("building")))
                        };
                        let Some(top) = (b.floor - 12..=b.floor + 8).rev().find(|&y| land(y)) else { continue };
                        looked += 1;
                        if top > b.floor + 1 {
                            failures.push(format!("seed {seed} {}: the {}'s walls are buried {} deep at {x}, {z}", s.name.0, b.what, top - b.floor));
                        }
                    }
                }
            }
        }
    }
    assert!(looked > 2000, "only {looked} columns round buildings");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Rejected candidate paths need no allocation, but the first valid flight and the
/// holes it leaves for the next storey must keep their original order.
#[test]
fn stair_candidates_keep_original_first_valid_paths() {
    use super::def::{Form, Roof};
    use super::house::{self, Kind, Spec};
    let mut spec = Spec {
        kind: Kind::House, w: 5, d: 5, wall: 3, storeys: 3, roof: Roof::Gable, pitch: 1, ridge_x: false,
        extras: Vec::new(), door: 2, shed: 0, wing: None, stilts: 0, regular: true, window_chance: 0.5,
        form: Form::Box, inside: Vec::new(), centre: None, seed: 1,
    };
    let back = vec![(3, 3), (2, 3), (1, 3), (1, 2)];
    let front = vec![(1, 1), (2, 1), (3, 1), (3, 2)];
    assert_eq!(house::stairs(&spec, 2, None), vec![Some(back.clone()), Some(front.clone())]);
    assert_eq!(house::stairs(&spec, 2, Some((3, 3))), vec![Some(vec![(2, 3), (1, 3), (1, 2), (1, 1)]), None]);
    spec.kind = Kind::Inn;
    assert_eq!(house::stairs(&spec, 2, None), vec![Some(back.clone()), Some(front)]);
    spec.form = Form::ShopFront;
    assert_eq!(house::stairs(&spec, 2, None), vec![None, Some(back)]);
}

/// Every storey but the top of every house of more than one has a stair up, round the
/// walls clear of the door, a shop's counter and the chimney, whatever its size; an inn's
/// stairs keep clear of both columns of its wide door.
#[test]
fn every_storey_has_a_stair_up() {
    use super::def::{Extra, Form, Roof};
    use super::house::{self, Kind, Spec};
    let world = shared_world(1);
    let def = &world.settlements.def;
    let mut flights = 0;
    for h in def.houses.iter().filter(|h| h.storeys.1 >= 2) {
        assert!(matches!(h.form, Form::Box | Form::ShopFront), "{} has storeys but no room for stairs", h.name);
        for w in h.width.0..=h.width.1 {
            for d in h.depth.0..=h.depth.1 {
                for (kind, chimney) in [(Kind::House, false), (Kind::House, true), (Kind::Inn, true)] {
                    let (w, d) = (w as i32, d as i32);
                    let mut spec = Spec {
                        kind, w, d, wall: h.walls.0 as i32, storeys: h.storeys.1 as i32, roof: Roof::Gable, pitch: 1, ridge_x: w > d,
                        extras: if chimney { vec![Extra::Chimney] } else { Vec::new() }, door: w / 2, shed: 0, wing: None, stilts: 0,
                        regular: true, window_chance: 0.5, form: h.form, inside: h.inside.clone(), centre: None, seed: 1,
                    };
                    let envelope = spec.envelope();
                    house::make_room_for_stairs(&mut spec);
                    assert_eq!(spec.envelope(), envelope, "stair adaptation must not alter placement bounds");
                    let door = spec.door.clamp(1, w - 2);
                    let hearth = spec.chimney().map(|c| c.0);
                    let up = house::stairs(&spec, door, hearth);
                    assert_eq!(up.len() as i32, spec.storeys - 1, "{} {w} by {d}", h.name);
                    for (storey, flight) in up.iter().enumerate() {
                        let flight = flight.as_ref().unwrap_or_else(|| panic!("{} {w} by {d} ({kind:?}, chimney {chimney}): no stair up from storey {storey}", h.name));
                        flights += 1;
                        for &(x, z) in flight {
                            assert!(Some((x, z)) != hearth, "{} {w} by {d}: its stair runs up the chimney", h.name);
                            if storey == 0 {
                                let wide = if kind == Kind::Inn { 1 } else { 0 };
                                assert!(!(z <= 2 && x >= door && x <= door + wide), "{} {w} by {d}: its stair blocks the door", h.name);
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(flights > 50, "only {flights} flights");
    // A mod's house too narrow for any stair is built a storey high, never with a storey
    // above and no way up.
    for (w, d) in [(4, 4), (3, 6), (5, 5)] {
        let mut spec = Spec {
            kind: Kind::House, w, d, wall: 5, storeys: 3, roof: Roof::Gable, pitch: 1, ridge_x: false, extras: vec![Extra::Chimney], door: w / 2, shed: 0,
            wing: None, stilts: 0, regular: true, window_chance: 0.5, form: Form::Box, inside: Vec::new(), centre: None, seed: 1,
        };
        house::make_room_for_stairs(&mut spec);
        let up = house::stairs(&spec, spec.door.clamp(1, w - 2), spec.chimney().map(|c| c.0));
        assert!(up.len() as i32 == spec.storeys - 1 && up.iter().all(Option::is_some), "{w} by {d}: {} storeys, stairs {up:?}", spec.storeys);
    }
}

/// A mod's partial `rules` replace only the sections it names.
#[test]
fn a_mods_rules_replace_only_their_sections() {
    let mut def = super::SettlementsDef::default();
    def.extend("base", "base", include_str!("../../../../assets/content/base/settlements.ron")).unwrap();
    let base = def.clone();
    def.extend("mod", "mod", "(rules: (lanes: (side: 1.25), specials: {\"field\": (width: (5, 7), depth: (5, 7), step: 2, slope: 2)}))").unwrap();
    assert_eq!(def.rules.lanes.side, 1.25);
    assert_eq!(def.rules.lots, base.rules.lots);
    assert_eq!(def.rules.levers, base.rules.levers);
    assert_eq!(def.rules.houses, base.rules.houses);
    assert_eq!(def.rules.dressing, base.rules.dressing);
    assert_eq!(def.rules.specials["field"].width, (5, 7));
    assert_eq!(def.rules.specials["smithy"], base.rules.specials["smithy"]);
}

/// Every settlement round spawn is joined to the trails at one of its gates (where a main
/// lane leads out) when it has somewhere to go, and every gate on dry ground is a place
/// trails may set out from. Not every gate gets one: two gates towards the same viewpoint
/// share the shorter trail, so trails never web the land.
#[test]
fn every_settlement_is_joined_to_the_trails() {
    let mut failures = Vec::new();
    let mut joined = 0;
    for seed in [1u64, 42, 7] {
        let world = shared_world(seed);
        let spawn = world.spawn_point();
        for s in world.settlements_near(IVec2::new(spawn.x, spawn.z), 3000).iter().take(3) {
            let centre = IVec2::new(s.centre.x, s.centre.z);
            let (lines, places) = world.trails_in(centre - 256, centre + 256);
            let somewhere = places.iter().any(|p| p.kind != crate::PlaceKind::Settlement && s.gates.iter().any(|g| p.at().as_vec2().distance(g.as_vec2()) < 300.0));
            let at_gate = s.gates.iter().any(|g| lines.iter().flatten().any(|(at, _)| at.distance(g.as_vec2()) <= 12.0));
            if somewhere && !at_gate {
                failures.push(format!("seed {seed} {}: no trail sets out from any of its gates {:?}", s.name.0, s.gates));
            }
            joined += at_gate as i32;
            for gate in &s.gates {
                let cell = gate.div_euclid(IVec2::splat(8));
                let dry = !world.geo(cell.x * 8, cell.y * 8).submerged();
                if dry && !places.iter().any(|p| p.kind == crate::PlaceKind::Settlement && p.cell == cell) {
                    failures.push(format!("seed {seed} {}: its gate at {gate} is no place trails set out from", s.name.0));
                }
            }
        }
    }
    assert!(joined >= 7, "only {joined} settlements joined");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Where snow lies round a house, none lies inside it: the blanket stops at roofs,
/// however tall the house.
#[test]
fn no_snow_falls_indoors() {
    use super::house::Kind;
    let registry = base_registry();
    let snowy = |b: BlockId| registry.def(b).name.contains("snow");
    let boxy = ["cottage", "long_house", "town_house", "farm_house", "hall", "tower_house", "wing_house"];
    let (mut failures, mut looked) = (Vec::new(), 0);
    'seeds: for seed in [1u64, 42, 7, 3, 11] {
        let world = shared_world(seed);
        for cz in -4..4 {
            for cx in -4..4 {
                let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
                let Some(plan) = &slot.0 else { continue };
                if !["log_cabin", "turf_house", "stone_cottage"].iter().any(|s| plan.style.ends_with(s)) {
                    continue;
                }
                for b in plan.buildings.iter().filter(|b| b.kind == Kind::House && boxy.contains(&b.what.as_str())) {
                    let area = Area::new(&world, b.min - 2, b.max + 2);
                    // Snow on the open ground round it.
                    let ring = (b.min.x - 2..=b.max.x + 2).flat_map(|x| [IVec2::new(x, b.min.y - 2), IVec2::new(x, b.max.y + 2)]);
                    let outside = ring.filter(|p| (b.floor - 4..b.floor + 4).any(|y| snowy(area.block(IVec3::new(p.x, y, p.y))))).count();
                    if outside < 4 {
                        continue;
                    }
                    looked += 1;
                    for z in b.min.y + 1..b.max.y {
                        for x in b.min.x + 1..b.max.x {
                            for y in b.floor + 1..=b.floor + 2 {
                                if snowy(area.block(IVec3::new(x, y, z))) {
                                    failures.push(format!("seed {seed} {}: snow in the {} at {x} {y} {z}", plan.name.0, b.what));
                                }
                            }
                        }
                    }
                    if looked >= 12 {
                        break 'seeds;
                    }
                }
            }
        }
    }
    assert!(looked >= 4, "only {looked} houses in snow");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Room metadata follows the painted footprint after rotation, rather than the graded
/// construction envelope. Connected wings include the opening through the back wall.
#[test]
fn room_columns_exclude_exterior_walls_and_courtyards() {
    use super::canvas::{Facing, Frame};
    use super::def::{Extra, Form, Roof};
    use super::house::{Kind, Spec};
    let mut spec = Spec {
        kind: Kind::House, w: 13, d: 13, wall: 3, storeys: 1, roof: Roof::Gable,
        pitch: 1, ridge_x: false, extras: vec![Extra::Porch], door: 6, shed: 3,
        wing: None, stilts: 2, regular: true, window_chance: 0.0, form: Form::Box,
        inside: Vec::new(), centre: None, seed: 3,
    };
    for facing in [Facing::North, Facing::South, Facing::West, Facing::East] {
        let frame = Frame { origin: IVec3::new(100, 132, 200), facing };
        for form in [Form::Box, Form::Round, Form::Courtyard] {
            spec.form = form;
            let room = spec.room(&frame);
            let contains = |x, z| { let p = frame.at(x, 0, z); room.contains(&IVec2::new(p.x, p.z)) };
            assert!(contains(2, 6), "no interior for {form:?} facing {facing:?}");
            for (x, z) in [(6, -2), (14, 6), (6, 13), (0, 6), (12, 6)] {
                assert!(!contains(x, z), "porch, shed or wall included for {form:?} facing {facing:?}");
            }
            if form == Form::Courtyard {
                assert!(!contains(6, 6), "open courtyard included");
                assert!(!contains(3, 6), "courtyard wall included");
            } else {
                assert!(contains(6, 6));
            }
            if form == Form::Round { assert!(!contains(1, 1), "round corner yard included"); }
        }
        spec.form = Form::Box;
        spec.wing = Some((5, 3));
        let room = spec.room(&frame);
        assert_eq!(room.len(), 121 + 9);
        for (x, z) in [(10, 12), (10, 13), (10, 14)] {
            let p = frame.at(x, 0, z);
            assert!(room.contains(&IVec2::new(p.x, p.z)), "connected wing floor omitted");
        }
        for (x, z) in [(8, 13), (12, 13), (10, 15)] {
            let p = frame.at(x, 0, z);
            assert!(!room.contains(&IVec2::new(p.x, p.z)), "wing wall included");
        }
        spec.wing = None;
    }
    spec.kind = Kind::Well;
    assert!(spec.room(&Frame { origin: IVec3::ZERO, facing: Facing::North }).is_empty());
}

/// From the lane, through the door and up the stairs, every storey of every house of more
/// than one can be walked to, at its least and greatest size, with a chimney and without,
/// and as an inn: a step up at a time, with room overhead.
#[test]
fn every_storey_can_be_walked_to() {
    use super::canvas::{Facing, Frame};
    use super::def::{Extra, Roof};
    use super::house::{self, Kind, Mats, Spec};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let def = &world_content.settlements;
    let fixed = super::fixed(&registry, def).unwrap();
    let id = |n: &str| registry.id(&format!("base:{n}")).unwrap();
    let palette = |n: &str| super::canvas::Palette(vec![(id(n), 1.0)]);
    let mats = Mats {
        foundation: palette("cobblestone"), floor: id("spruce_planks"), wall: id("polished_calcite"), frame: id("dark_oak_log"),
        roof: id("dark_oak_planks"), trim: id("spruce_planks"), window: Some(id("old_glass")), weathered: palette("mossy_cobblestone"),
        hedge: palette("oak_leaves"), log: id("oak_log"), chimney: id("cobblestone"), crops: Vec::new(), fruit: palette("apple_leaves_2"), roof_pieces: Vec::new(), pieced: 0.0, jetty: None,
    };
    let solid = |b: BlockId| registry.def(b).solid;
    let mut walked = 0;
    for h in def.houses.iter().filter(|h| h.storeys.1 >= 2) {
        for (w, d) in [(h.width.0, h.depth.0), (h.width.1, h.depth.1)] {
            for (kind, chimney) in [(Kind::House, false), (Kind::House, true), (Kind::Inn, true)] {
                let (w, d) = (w as i32, d as i32);
                let wall = h.walls.1 as i32;
                let mut spec = Spec {
                    kind, w, d, wall, storeys: h.storeys.1 as i32, roof: Roof::Gable, pitch: 1, ridge_x: w > d,
                    extras: if chimney { vec![Extra::Chimney] } else { Vec::new() }, door: w / 2, shed: 0, wing: None, stilts: 0,
                    regular: true, window_chance: 0.5, form: h.form, inside: h.inside.clone(), centre: None, seed: 3,
                };
                house::make_room_for_stairs(&mut spec);
                let (low, high) = spec.envelope();
                let frame = Frame::fit(low, high, Facing::South, IVec2::ZERO, 0);
                let mut canvas = super::canvas::Canvas::default();
                house::draw(&spec, &frame, &mats, &fixed, &mut canvas);
                let extent = Frame::extent(low, high, Facing::South);
                let (min, max) = (IVec2::splat(-3), extent + 2);
                let model = Model::new(canvas, &registry, min, max);
                let stands = |p: IVec3| solid(model.block(p - IVec3::Y)) && !solid(model.block(p)) && !solid(model.block(p + IVec3::Y));
                // Walk from everywhere round the edge of the ground.
                let mut seen: rustc_hash::FxHashSet<IVec3> = Default::default();
                let mut queue: Vec<IVec3> = (min.x..=max.x).flat_map(|x| [IVec3::new(x, 1, min.y), IVec3::new(x, 1, max.y)]).filter(|&p| stands(p)).collect();
                seen.extend(queue.iter().copied());
                while let Some(p) = queue.pop() {
                    for step in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
                        for rise in [1, 0, -1, -2, -3] {
                            let q = p + step + IVec3::Y * rise;
                            if q.x < min.x || q.x > max.x || q.z < min.y || q.z > max.y || seen.contains(&q) || !stands(q) {
                                continue;
                            }
                            // Up a step needs room overhead to climb; down, room to fall past.
                            let clear = if rise > 0 { !solid(model.block(p + IVec3::Y * 2)) } else { (rise + 1..=0).all(|r| !solid(model.block(p + step + IVec3::Y * (r + 1)))) };
                            if clear {
                                seen.insert(q);
                                queue.push(q);
                            }
                        }
                    }
                }
                // The ground floor is where one stands just inside the door.
                let interior: Vec<(i32, i32)> = (1..w - 1).flat_map(|x| (1..d - 1).map(move |z| (x, z))).collect();
                let ground = (0..8).find(|&y| interior.iter().any(|&(x, z)| seen.contains(&frame.at(x, y, z))));
                let ground = ground.unwrap_or_else(|| panic!("{} {w} by {d} ({kind:?}): its ground floor cannot be walked to", h.name));
                for storey in 1..spec.storeys {
                    let y = ground + storey * (spec.wall + 1);
                    let inside = interior.iter().any(|&(x, z)| seen.contains(&frame.at(x, y, z)));
                    assert!(inside, "{} {w} by {d} ({kind:?}, chimney {}): storey {storey} cannot be walked to", h.name, spec.has(Extra::Chimney));
                    walked += 1;
                }
            }
        }
    }
    assert!(walked > 20, "only {walked} storeys walked to");
}

/// A new player finds a settlement within `spawn_settlement_within` of where they spawn
/// (rules), measured from the spawn point itself, and never spawns inside one; over twenty
/// worlds. Only the land may excuse a world (design, draft 81): one where no square in
/// reach has a spot a settlement could stand on, as on seed 0. A square with spots tries
/// each with other draws and as the smallest place before it gives up (`plan::plan`).
#[test]
fn a_settlement_stands_near_spawn() {
    let (mut far, mut excused) = (Vec::new(), Vec::new());
    // 0x5eed: the world a new game makes unless told otherwise.
    let seeds: Vec<u64> = (0..20).chain([42, 99, 0x5eed]).collect();
    // Each world on its own core, its outcome kept in seed order.
    let outcomes: Vec<(u64, Option<Result<(), String>>)> = seeds.par_iter().map(|&seed| {
        let world = shared_world(seed);
        let within = world.settlements.def.rules.spawn_settlement_within as f32;
        let spawn = world.spawn_point();
        let at = IVec2::new(spawn.x, spawn.z);
        assert!(!world.settlements.covers(&world, spawn.x, spawn.z), "seed {seed}: spawn at {spawn} is inside a settlement");
        // A settlement stands inside its own square of the grid, so any within reach is in
        // a square that the box `within` round spawn touches: those alone are planned to
        // decide, and further ones only to say how far the nearest is when none is.
        let nearest = |radius: i32| world.settlements_near(at, radius).iter().map(|s| IVec2::new(s.centre.x, s.centre.z).as_vec2().distance(at.as_vec2())).fold(f32::MAX, f32::min);
        if nearest(within as i32) <= within {
            return (seed, None);
        }
        let nearest = nearest(2048);
        let sites = world.settlements.spawn_has_sites(&world);
        eprintln!("seed {seed}: none within {within} of spawn at {spawn}; spots in reach: {sites}");
        if sites {
            (seed, Some(Err(format!("seed {seed}: the nearest settlement is {nearest:.0} blocks from spawn at {spawn}, though the land has spots for one"))))
        } else {
            (seed, Some(Ok(())))
        }
    }).collect();
    for (seed, outcome) in outcomes {
        match outcome {
            Some(Err(why)) => far.push(why),
            Some(Ok(())) => excused.push(seed),
            None => {}
        }
    }
    assert!(far.is_empty(), "{}", far.join("\n"));
    assert_eq!(excused, [0], "the worlds whose land has no spot near spawn");
}

/// Loading refuses ripe or rare fruit wherever a settlement puts leaves: orchards' fruit by
/// name or by tag, and centrepieces' leaves in any wood a style picks.
#[test]
fn ripe_or_rare_fruit_is_refused() {
    use crate::content::Select;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let base = crate::content::WorldContent::load(&content).unwrap().settlements;
    let refused = |change: &dyn Fn(&mut super::SettlementsDef)| {
        let mut def = base.clone();
        change(&mut def);
        def.check_furnishings(&content.blocks).is_err()
    };
    assert!(!refused(&|_| {}));
    assert!(refused(&|d| d.furnishings.fruit = vec![Select::Name("base:apple_leaves".into(), 1.0)]));
    assert!(refused(&|d| d.furnishings.fruit = vec![Select::Tag("fruit_leaves".into(), 1.0)]), "the tag holds ripe and legendary leaves");
    assert!(refused(&|d| d.furnishings.fruit = vec![Select::Name("base:golden_apple_leaves_2".into(), 1.0)]));
    assert!(refused(&|d| d.centrepieces[0].parts = vec![("leaves".into(), "base:plum_leaves".into())]));
    assert!(refused(&|d| {
        d.centrepieces[0].parts = vec![("leaves".into(), "base:{wood}_leaves".into())];
        d.styles[0].woods.push(("apple".into(), 1.0));
    }));
}

/// People's numbers that would not play are refused at load: a newcomer sooner than a
/// quarter day (a village to empty of experience again and again), chances past 1,
/// weights below nothing, a settlement let go before it comes to life, a tick's search
/// smaller than one way's.
#[test]
fn people_numbers_that_would_not_play_are_refused() {
    let world = shared_world(1);
    let def = &world.settlements.def;
    let sizes: Vec<String> = def.sizes.iter().map(|s| s.name.clone()).collect();
    assert!(def.people.check(&sizes).is_ok());
    let spoilt: [(&str, fn(&mut super::people::PeopleDef)); 7] = [
        ("newcomer in a fifth of a day", |p| p.newcomer = 0.2),
        ("no newcomers a day", |p| p.newcomers_per_day = 0.0),
        ("a job chance of 1.5", |p| p.jobs[0].chance = 1.5),
        ("a folk weight below nothing", |p| p.folk[0].weight = -1.0),
        ("release inside active", |p| p.live.release = p.live.active - 1.0),
        ("a tick's search under a way's", |p| p.live.search_per_tick = p.live.search - 1),
        ("linger from below nothing", |p| p.linger.0 = -1.0),
    ];
    for (what, spoil) in spoilt {
        let mut people = def.people.clone();
        spoil(&mut people);
        assert!(people.check(&sizes).is_err(), "{what} was let through");
    }
}

/// People name species that are villagers and works there are, or the content fails to
/// load (rather than warn at every look once a player comes near).
#[test]
fn people_name_villagers_and_works_there_are() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world = crate::content::WorldContent::load(&content).unwrap();
    let (people, works) = (&world.settlements.people, world.settlements.works());
    assert!(people.check_against(&content.creatures, &works).is_ok());
    let spoilt: [(&str, fn(&mut super::people::PeopleDef)); 5] = [
        ("an animal for a folk", |p| p.folk[0].species = "base:rabbit".into()),
        ("no such guard", |p| p.guards[0].species = "base:gaurd".into()),
        ("a job at no such work", |p| p.jobs[0].at = "bakehouse".into()),
        ("folk working at no such work", |p| p.folk.iter_mut().find(|f| f.work.is_some()).expect("a folk works").work = Some("mine".into())),
        ("a shrine of no such kind", |p| p.shrine = "temple".into()),
    ];
    for (what, spoil) in spoilt {
        let mut spoilt = people.clone();
        spoil(&mut spoilt);
        assert!(spoilt.check_against(&content.creatures, &works).is_err(), "{what} was let through");
    }
}

/// Every settlement has people: each lives in a home and works at a building that takes
/// workers, a village keeps guards and an innkeeper, every routine sends them somewhere
/// each slot of the day, and no two people of a place keep quite the same day.
#[test]
fn settlements_are_peopled() {
    use super::Kind;
    use super::people::{Place, is, place_at};
    let mut villages = 0;
    for seed in [1u64, 42, 7] {
        let world = shared_world(seed);
        let spawn = world.spawn_point();
        let people = world.settlement_people();
        for s in world.settlements_near(IVec2::new(spawn.x, spawn.z), 3000) {
            let what = format!("seed {seed} {} ({})", s.name.0, s.size);
            assert!(!s.people.is_empty(), "{what}: nobody lives there");
            for r in &s.people {
                assert!(matches!(s.buildings[r.home].kind, Kind::House | Kind::Trader | Kind::Inn), "{what}: {} lives in a {}", r.species, s.buildings[r.home].kind.name());
                if let Some(w) = r.work {
                    let b = &s.buildings[w];
                    assert!(people.jobs.iter().any(|j| j.species == r.species && is(b, &j.at)) || people.folk.iter().any(|f| f.species == r.species && f.work.as_deref().is_some_and(|w| is(b, w))), "{what}: {} works at a {}", r.species, b.what);
                }
            }
            let homes = s.buildings.iter().filter(|b| b.kind == Kind::House && b.work.is_none()).count();
            assert!(s.people.len() >= homes, "{what}: {} people in {homes} homes", s.people.len());
            if s.size == "village" {
                villages += 1;
                assert!(s.people.iter().any(|r| r.guard), "{what}: no guards");
                assert!(s.people.iter().any(|r| r.species == "base:innkeeper"), "{what}: nobody keeps the inn");
            }
            let levers = s.levers;
            let mut days = std::collections::BTreeSet::new();
            for r in &s.people {
                let day: Vec<Place> = (0..100).map(|i| place_at(people, r.routine, r.seed, &levers, 3, i as f32 / 100.0, |_| true).0).collect();
                // Asleep at the dead of night, unless they keep the night watch.
                assert!(day[90] == Place::Sleep || people.routines[r.routine].name == "watch_night", "{what}: {} up at night: {:?}", r.species, day[90]);
                days.insert(day);
            }
            assert!(days.len() * 3 >= s.people.len(), "{what}: {} people keep only {} days", s.people.len(), days.len());
        }
    }
    assert!(villages > 0, "no village to look at");
}

/// Loading refuses a furnishing slot whose block would drop itself from every village
/// (design draft 80): one that drops nothing passes, and so does one tagged `furnishing`,
/// which gives itself back only where a player placed it.
#[test]
fn furnishings_that_drop_themselves_are_refused_unless_tagged() {
    use crate::content::Select;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let base = crate::content::WorldContent::load(&content).unwrap().settlements;
    let refused = |change: &dyn Fn(&mut super::SettlementsDef)| {
        let mut def = base.clone();
        change(&mut def);
        def.check_furnishings(&content.blocks).is_err()
    };
    assert!(!refused(&|_| {}), "the game's own settlements load");
    // Planks drop themselves and are no furnishing; the crafting table is one.
    assert!(refused(&|d| d.furnishings.barrel = Some("base:oak_planks".into())));
    assert!(!refused(&|d| d.furnishings.barrel = Some("base:crafting_table".into())));
    assert!(refused(&|d| d.furnishings.hay = vec![Select::Name("base:oak_planks".into(), 1.0)]));
    assert!(refused(&|d| d.furnishings.sacks = vec![Select::Tag("planks".into(), 1.0)]), "by tag too");
    assert!(refused(&|d| d.furnishings.campfire = Some("base:torch".into())), "a torch drops itself and is no furnishing");
    // An altar is checked once it is one: the stone altar passes, and so does a building
    // block standing in for it.
    assert!(!refused(&|d| d.furnishings.altar = Some("base:stone_altar".into())));
    assert!(!refused(&|d| d.furnishings.altar = Some("base:polished_calcite".into())));
    assert!(refused(&|d| d.furnishings.nets = vec![Select::Name("base:oak_planks".into(), 1.0)]));
    // A centrepiece's own bell too.
    assert!(refused(&|d| d.centrepieces[0].parts = vec![("bell".into(), "base:oak_planks".into())]));
}

/// Snow lies over a snowy village without turning its built stone to snow: chimney
/// stacks, foundation courses, well rims and stacked stone keep their blocks (and their
/// drops), as the same village generated with no snow blanket at all has them (Review,
/// 7 Oct: the blanket once covered every block tagged `stone`, cobbles and bricks too).
/// The blanket still works there: natural stone open to the sky in and around the
/// village turns to snow, and its sod roofs whiten as the ground does.
#[test]
fn snowy_villages_keep_their_built_stone() {
    let registry = base_registry();
    let content = crate::content::WorldContent::base(&registry).unwrap();
    let mut bare = content.clone();
    bare.climate.frost.blanket.clear();
    let def = |b: BlockId| registry.def(b);
    let built = |b: BlockId| def(b).has_tag("stone") && def(b).has_tag("building");
    let natural_stone = |b: BlockId| def(b).has_tag("stone") && !def(b).has_tag("building");
    let (snow, grass, snowy_grass) = (registry.expect("base:snow_block"), registry.expect("base:grass_block"), registry.expect("base:snowy_grass_block"));
    let (mut villages, mut kept, mut snowed_stone, mut snowed_sod) = (0, 0, 0, 0);
    // Seeds known to have snowy settlements that between them prove everything below come
    // first (18: kept stone, snowed stone and sod; 1: more), so the walk usually ends in two
    // worlds; the others are the search it falls back to when content moves the snow.
    const SNOWY: [u64; 2] = [18, 1];
    'seeds: for seed in SNOWY.into_iter().chain((1u64..=60).filter(|seed| !SNOWY.contains(seed))) {
        let world = Overworld::with_content(seed, registry.clone(), &content);
        let spawn = world.spawn_point();
        for s in world.settlements_near(IVec2::new(spawn.x, spawn.z), 4000) {
            // Snowy where snow lies on the ground about its middle.
            let column = world.generate(ColumnPos::of_block(s.centre));
            let snowy = [(0, 0), (6, 0), (-6, 0), (0, 6), (0, -6)].iter().any(|&(dx, dz)| {
                let (x, z) = ((s.centre.x + dx) & 31, (s.centre.z + dz) & 31);
                let top = (0..WORLD_HEIGHT).rev().map(|y| column.block(IVec3::new(x, y, z))).find(|b| !b.is_air());
                top.is_some_and(|b| def(b).has_tag("snow"))
            });
            if !snowy {
                continue;
            }
            // Every column over the village and 16 blocks round it, generated once with the
            // blanket and once without, then borrowed.
            let plain = Overworld::with_content(seed, registry.clone(), &bare);
            let min = s.buildings.iter().fold(IVec2::splat(i32::MAX), |m, b| m.min(b.min)) - 16;
            let max = s.buildings.iter().fold(IVec2::splat(i32::MIN), |m, b| m.max(b.max)) + 16;
            let (first, last) = (ColumnPos::of_block(IVec3::new(min.x, 0, min.y)), ColumnPos::of_block(IVec3::new(max.x, 0, max.y)));
            let size = (last.0 - first.0).max_element() + 1;
            let (snowed, unsnowed) = (generate_area(&world, first, size), generate_area(&plain, first, size));
            let both = |x: i32, z: i32| {
                let at = ColumnPos::of_block(IVec3::new(x, 0, z)).0 - first.0;
                let i = (at.y * size + at.x) as usize;
                (&snowed[i], &unsnowed[i])
            };
            for b in &s.buildings {
                for z in b.min.y..=b.max.y {
                    for x in b.min.x..=b.max.x {
                        let (with, without) = both(x, z);
                        for y in b.floor - 2..(b.floor + 24).min(WORLD_HEIGHT) {
                            let local = IVec3::new(x & 31, y, z & 31);
                            let (want, got) = (without.block(local), with.block(local));
                            if built(want) {
                                assert_eq!(def(got).name, def(want).name, "seed {seed}, {}: the {} at {x} {y} {z} turned to snow", s.name.0, b.what);
                                kept += 1;
                            }
                        }
                    }
                }
            }
            // The positive control: over the village and 16 blocks round it, natural stone
            // open to the sky without the blanket is snow with it, and grass roofs and
            // banks over the ground are snowy grass.
            for z in min.y..=max.y {
                for x in min.x..=max.x {
                    let (with, without) = both(x, z);
                    let mut open = true;
                    for y in (1..WORLD_HEIGHT).rev() {
                        let local = IVec3::new(x & 31, y, z & 31);
                        let (want, got) = (without.block(local), with.block(local));
                        if open && natural_stone(want) && got == snow {
                            snowed_stone += 1;
                        }
                        if open && want == grass && got == snowy_grass {
                            snowed_sod += 1;
                        }
                        open &= want.is_air() || def(want).render == RenderKind::Plant;
                    }
                }
            }
            println!("seed {seed}: snowy {} {} at {}", s.size, s.name.0, s.centre);
            villages += 1;
            if villages >= 2 && kept > 50 && snowed_stone > 0 && snowed_sod > 0 {
                break 'seeds;
            }
        }
    }
    assert!(villages > 0, "no snowy settlement found");
    assert!(kept > 50, "only {kept} built stone blocks in {villages} snowy settlements");
    assert!(snowed_stone > 0, "no natural stone open to the sky turned to snow in or around {villages} snowy settlements");
    assert!(snowed_sod > 0, "no grass over the ground turned snowy in or around {villages} snowy settlements");
    println!("kept {kept} built stone blocks; {snowed_stone} natural stone and {snowed_sod} grass blocks snowed");
}

/// No tree's crown lies over a roof, and no tree's trunk stands within the glade's treeless
/// ring of a building's walls, in the woods round spawn (Fernford, seed 1, stands in a forest).
#[test]
fn trees_keep_off_roofs_and_walls() {
    use super::house::Kind;
    let registry = base_registry();
    let leaves = |b: BlockId| registry.def(b).render == RenderKind::Leaves && registry.def(b).has_tag("natural");
    let log = |b: BlockId| registry.def(b).has_tag("log") && registry.def(b).has_tag("natural");
    let (mut failures, mut roofs) = (Vec::new(), 0);
    for seed in [1u64, 42, 7] {
        let world = shared_world(seed);
        // Where a trunk may be rooted, less the block a thick trunk spreads to.
        let trunks = world.settlements.def.site.glade.0 - 1;
        let spawn = world.spawn_point();
        let mut near = world.settlements_near(IVec2::new(spawn.x, spawn.z), 3000);
        near.sort_by_key(|s| (s.centre.x - spawn.x).pow(2) + (s.centre.z - spawn.z).pow(2));
        for s in near.iter().take(3) {
            for b in s.buildings.iter().filter(|b| matches!(b.kind, Kind::House | Kind::Inn | Kind::Trader | Kind::Smithy) && b.what != "courtyard_house") {
                let area = Area::new(&world, b.min - trunks, b.max + trunks);
                for z in b.min.y..=b.max.y {
                    for x in b.min.x..=b.max.x {
                        // The highest block over the footprint: never a tree's leaves.
                        let Some(top) = (b.floor + 1..b.floor + 48).rev().find(|&y| !area.block(IVec3::new(x, y, z)).is_air()) else { continue };
                        roofs += 1;
                        let block = area.block(IVec3::new(x, top, z));
                        if leaves(block) {
                            failures.push(format!("seed {seed} {}: {} over the {} at {x} {top} {z}", s.name.0, registry.def(block).name, b.what));
                        }
                    }
                }
                // No trunk (three or more natural logs one over another) beside its walls.
                for z in b.min.y - trunks..=b.max.y + trunks {
                    for x in b.min.x - trunks..=b.max.x + trunks {
                        // As the glade measures: straight out from the nearest wall.
                        let (dx, dz) = ((b.min.x - x).max(x - b.max.x), (b.min.y - z).max(z - b.max.y));
                        if dx.max(dz) <= 0 || dx.max(0).pow(2) + dz.max(0).pow(2) > trunks * trunks {
                            continue;
                        }
                        // A tree's: logs one over another up into leaves (a post or a
                        // woodpile has none over it).
                        let logs: Vec<i32> = (b.floor - 2..b.floor + 12).filter(|&y| log(area.block(IVec3::new(x, y, z)))).collect();
                        let crowned = logs.last().is_some_and(|&t| (t + 1..t + 4).any(|y| leaves(area.block(IVec3::new(x, y, z)))));
                        if logs.len() >= 3 && crowned {
                            failures.push(format!("seed {seed} {}: a trunk beside the {} at {x} {z}", s.name.0, b.what));
                        }
                    }
                }
            }
        }
    }
    assert!(roofs > 1000, "only {roofs} roof columns");
    failures.dedup();
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

/// No two settlements within `rules.names_apart` blocks of each other share a name, a word
/// before it aside ("Old Ashcote" and "Ashcote" would not do): seed 1 had two Ashcotes.
#[test]
fn near_settlements_have_names_of_their_own() {
    let mut clashes = Vec::new();
    let mut pairs = 0;
    for seed in [1u64, 42] {
        let world = shared_world(seed);
        let def = &world.settlements.def;
        let apart = def.rules.names_apart as f32;
        let bare = |name: &str| {
            let prefix = def.cultures.families.iter().flat_map(|f| &f.names.prefixes).find(|p| name.starts_with(&format!("{} ", p.0)));
            prefix.map_or(name, |p| &name[p.0.len() + 1..]).to_string()
        };
        let spawn = world.spawn_point();
        let near = world.settlements_near(IVec2::new(spawn.x, spawn.z), def.rules.names_apart);
        for (i, a) in near.iter().enumerate() {
            for b in &near[i + 1..] {
                if IVec2::new(a.centre.x, a.centre.z).as_vec2().distance(IVec2::new(b.centre.x, b.centre.z).as_vec2()) > apart {
                    continue;
                }
                pairs += 1;
                if bare(&a.name.0) == bare(&b.name.0) || a.name.1 == b.name.1 {
                    clashes.push(format!("seed {seed}: {} at {} and {} at {}", a.name.0, a.centre, b.name.0, b.centre));
                }
            }
        }
    }
    assert!(pairs > 30, "only {pairs} pairs near enough to compare");
    assert!(clashes.is_empty(), "{}", clashes.join("\n"));
}

/// Naming keeps near squares apart whatever names list and form each draws from: over a
/// stretch of squares wide enough that the same draws would clash many times over.
#[test]
fn naming_never_repeats_near() {
    let world = shared_world(1);
    let def = &world.settlements.def;
    let (grid, apart) = (&def.grid, def.rules.names_apart as f32);
    // A quarter of the squares hamlets with small names, then every one: the small names
    // are few, so near ones run out and fall back on first and last parts. The squares
    // round the middle are promised to a spawn, settled whatever their roll.
    let rolled = |x: i32, z: i32| crate::rng::Rng::new(world.settlements.seed, 0x5e771e, x, z).chance(grid.chance);
    let promised: Vec<IVec2> = (-2..=2).flat_map(|z| (-2..=2).map(move |x| IVec2::new(x, z))).filter(|c| !rolled(c.x, c.y)).collect();
    assert!(!promised.is_empty());
    for every in [4u64, 1] {
        let mut named = Vec::new();
        for z in -20..20 {
            for x in -20..20 {
                let cell = IVec2::new(x, z);
                if !rolled(x, z) && !promised.contains(&cell) {
                    continue;
                }
                // A list and form per square, as its biome and size would pick.
                let h = crate::rng::hash3(7, x, 0, z);
                let list = (h % def.cultures.families.len() as u64) as usize;
                let small = (h >> 32) % every == 0 && !def.cultures.families[list].names.small.is_empty();
                let name = super::plan::unique_name(def, world.settlements.seed, &promised, cell, list, small);
                let span = super::plan::name_span(def);
                let colour = (cell.y.rem_euclid(span) * span + cell.x.rem_euclid(span)) as usize;
                assert_eq!(super::plan::name_owner(&name.1, span), colour, "selected name must belong to its cell");
                named.push((cell, list, name));
            }
        }
        let gap = |a: IVec2, b: IVec2| ((a - b).abs() * grid.spacing - (grid.spacing - 2 * grid.margin)).max(IVec2::ZERO).as_vec2().length();
        let mut clashes = Vec::new();
        for (i, a) in named.iter().enumerate() {
            for b in &named[i + 1..] {
                if (a.2.0 == b.2.0 || a.2.1 == b.2.1) && gap(a.0, b.0) <= apart {
                    clashes.push(format!("{:?} in squares {} and {}", a.2, a.0, b.0));
                }
            }
        }
        assert!(named.len() > 500, "only {} squares named", named.len());
        assert!(clashes.is_empty(), "one in {every} small: {} clashes:\n{}", clashes.len(), clashes.join("\n"));
    }
}

/// A name is never one a neighbour offers while the list has another. Thirty names and
/// two dozen neighbours offering three each leave a few free: draws alone often miss
/// them, so a square past its own offers walks the list to one.
#[test]
fn a_taken_name_only_when_the_list_has_no_other() {
    use super::plan::{NAME_OFFERS, offered_name, unique_name};
    let world = shared_world(1);
    let mut def = world.settlements.def.clone();
    let mut list = def.cultures.families[0].names.clone();
    list.first = vec![("Ash".to_owned(), "アッシュ".to_owned())];
    // All thirty names belong to the middle square's colour, so the entire
    // catalogue remains available to this adversarial whole-list fallback check.
    let span = 5;
    list.last = (0..100_000).map(|i| (format!("b{i}"), format!("ブ{i}")))
        .filter(|p| super::plan::name_owner(&format!("アッシュ{}", p.1), span) == 0).take(30).collect();
    assert_eq!(list.last.len(), 30, "bounded fixture needs thirty owned names");
    list.small = Vec::new();
    def.cultures.families.truncate(1);
    def.cultures.families[0].names = list;
    // A five by five block of squares, all promised (no roll settles any other), all
    // within `names_apart` of the middle one.
    (def.grid.chance, def.rules.names_apart) = (0.0, def.grid.spacing * 3);
    let promised: Vec<IVec2> = (-2..=2).flat_map(|z| (-2..=2).map(move |x| IVec2::new(x, z))).collect();
    let (def, mut walked) = (&def, 0);
    for seed in 0..400u64 {
        let taken: std::collections::HashSet<String> =
            promised.iter().filter(|&&c| c != IVec2::ZERO).flat_map(|&c| (0..NAME_OFFERS).map(move |k| offered_name(def, seed, c, 0, k, false).0)).collect();
        if taken.len() == 30 {
            continue;
        }
        let name = unique_name(def, seed, &promised, IVec2::ZERO, 0, false).0;
        assert!(!taken.contains(&name), "seed {seed}: took {name}, which a neighbour offers, though {} were free", 30 - taken.len());
        walked += usize::from(!(0..NAME_OFFERS + 32).any(|k| offered_name(def, seed, IVec2::ZERO, 0, k, false).0 == name));
    }
    // Some found their name only by walking the list.
    assert!(walked > 0);
}

/// Unnameable colours and English aliases with conflicting readings are rejected
/// before generation can select a name from an empty or inconsistent owned pool.
#[test]
fn name_ownership_refuses_empty_colours_and_conflicting_aliases() {
    let world = base_world(1);
    let mut def = world.settlements.def.clone();
    super::plan::check_name_ownership(&def).unwrap();
    let names = &mut def.cultures.families[0].names;
    names.first.truncate(1);
    names.last.truncate(1);
    assert!(super::plan::check_name_ownership(&def).is_err_and(|e| e.0.contains("naming colours")));
    let mut def = world.settlements.def.clone();
    let span = super::plan::name_span(&def);
    let names = &mut def.cultures.families[0].names;
    names.last[1].0 = names.last[0].0.clone();
    let owner = super::plan::name_owner(&format!("{}{}", names.first[0].1, names.last[0].1), span);
    names.last[1].1 = (0..100_000).map(|i| format!("別{i}"))
        .find(|last| super::plan::name_owner(&format!("{}{}", names.first[0].1, last), span) != owner).unwrap();
    assert!(super::plan::check_name_ownership(&def).is_err_and(|e| e.0.contains("different owners")));
}

/// The final namespace includes rare echoed offers and names composed with
/// prefixes. Aliases in either language must retain the same underlying owner.
#[test]
fn name_ownership_checks_echoes_and_prefixes_in_both_languages() {
    let world = base_world(1);
    let mut def = world.settlements.def.clone();
    let span = super::plan::name_span(&def);
    let names = &mut def.cultures.families[0].names;
    let owner = super::plan::name_owner("ラネ", span);
    let different = (0..100_000).map(|i| format!("別{i}"))
        .find(|s| super::plan::name_owner(&format!("{s}ヨ"), span) != owner).expect("bounded conflicting echo reading");
    // Qw+erwer is non-echoing; Qwer+wer produces the same English name and is
    // echoing. All parts are nonempty and otherwise valid catalogue additions.
    names.first.extend([("Qw".into(), "ラ".into()), ("Qwer".into(), different)]);
    names.last.extend([("erwer".into(), "ネ".into()), ("wer".into(), "ヨ".into())]);
    assert!(super::plan::check_name_ownership(&def).is_err_and(|e| e.0.contains("different owners")));
    for japanese in [false, true] {
        let mut def = world.settlements.def.clone();
        let span = super::plan::name_span(&def);
        let names = &mut def.cultures.families[0].names;
        let prefix = names.prefixes.iter().find(|p| p.2 > 0.0).unwrap().clone();
        let (first, last) = names.first.iter().flat_map(|first| names.last.iter().map(move |last| (first, last)))
            .find(|(first, last)| super::plan::name_owner(&format!("{}{}", first.1, last.1), span)
                != super::plan::name_owner(&format!("{}・{}{}", prefix.1, first.1, last.1), span))
            .map(|(first, last)| (first.clone(), last.clone())).expect("a prefix changes a reading's owner");
        let original = super::plan::name_owner(&format!("{}{}", first.1, last.1), span);
        let different = (0..100_000).map(|i| format!("別{i}"))
            .find(|s| super::plan::name_owner(&format!("{s}{}", last.1), span) != original).expect("bounded conflicting prefix reading");
        let extra = if japanese {
            ("Unique alias".into(), format!("{}・{}", prefix.1, first.1))
        } else {
            (format!("{} {}", prefix.0, first.0), different)
        };
        if japanese {
            assert_ne!(super::plan::name_owner(&format!("{}{}", extra.1, last.1), span), original);
        }
        names.first.push(extra);
        assert!(super::plan::check_name_ownership(&def).is_err_and(|e| e.0.contains("different owners")), "prefix alias language {japanese}");
    }
    let mut def = world.settlements.def.clone();
    let span = super::plan::name_span(&def);
    let names = &mut def.cultures.families[0].names;
    let first = names.first[0].clone();
    let last = names.last[0].clone();
    let owner = super::plan::name_owner(&format!("{}{}", first.1, last.1), span);
    let different = (0..100_000).map(|i| format!("別{i}"))
        .find(|s| super::plan::name_owner(&format!("{s}{}", last.1), span) != owner).expect("bounded zero-prefix alias");
    names.first.push((format!("Earlier {}", first.0), different));
    names.prefixes.push(("Earlier".into(), "前の".into(), 0.0, Default::default()));
    assert_eq!(names.prefixes.last().unwrap().2, 0.0);
    assert!(super::plan::check_name_ownership(&def).is_err_and(|e| e.0.contains("different owners")), "selectable last-zero prefix alias must be checked");

}

/// Settling the spawn plans each square in its reach once: the plans made finding the
/// spawn's square are the squares' own. Prints how long settling the spawn takes once the
/// land round it is known (design's bar: under 300 ms on the cloud machine); a print, not
/// an assert, as a busy machine stretches it.
#[test]
fn the_spawn_plans_each_square_once() {
    use std::sync::atomic::Ordering;
    for seed in [0u64, 1, 7, 42] {
        let world = base_world(seed);
        let settlements = &world.settlements;
        // The land round spawn first, which the spawn's search needs anyway; then settling.
        let started = std::time::Instant::now();
        let at = world.spawn_estimate();
        let land = started.elapsed();
        let started = std::time::Instant::now();
        settlements.slot(&world, settlements.cell_of(at.x, at.y));
        let took = started.elapsed();
        let spawn = settlements.spawn.get().unwrap();
        let mut cells: Vec<IVec2> = spawn.early.lock().unwrap().keys().copied().chain(std::iter::once(settlements.cell_of(at.x, at.y))).collect();
        cells.sort_by_key(|c| (c.x, c.y));
        cells.dedup();
        let planned = settlements.planned.load(Ordering::Relaxed);
        for &c in &cells {
            settlements.slot(&world, c);
        }
        eprintln!("seed {seed}: the spawn's square {:?} settled in {took:?} after {land:?} for the land, {planned} plans (tries near the spawn included) for {} squares in reach", spawn.square.map(|s| s.1), cells.len());
        assert_eq!(settlements.planned.load(Ordering::Relaxed), planned, "seed {seed}: a square in the spawn's reach was planned again");
        assert!(spawn.early.lock().unwrap().is_empty());
    }
}

// ---------------------------------------------------------------------------------------
// Cultures (cultures.ron)

/// The family each region cell of `world` names, with the land asked once per cell.
fn cell_families(world: &Overworld) -> impl Fn(IVec2) -> usize + '_ {
    let probe = |p: IVec2| world.regional_biome(p.x, p.y);
    move |g| world.settlements.def.cultures.cached_family(world.settlements.seed, g, &world.settlements.cells, &probe)
}

/// Learning the family of distant country must not generate that country's rivers.
#[test]
fn culture_regions_do_not_plan_distant_rivers() {
    let world = base_world(7);
    let before = world.rivers.made();
    let family = cell_families(&world);
    for z in -6..6 {
        for x in -6..6 {
            family(IVec2::new(x, z));
        }
    }
    assert_eq!(world.rivers.made(), before, "culture fits planned distant water regions");
    // The coarse decision uses the same home biome as the full site, including borders.
    for at in [IVec2::ZERO, IVec2::new(2000, -4000), IVec2::new(-6000, 8000)] {
        assert_eq!(world.regional_biome(at.x, at.y).0.name, world.biomes().get(world.site(at.x, at.y).home).name);
    }
}

/// A culture rolled at `at` in square `cell` with middling levers, a village.
fn rolled(world: &Overworld, cell: IVec2, at: IVec2, fresh: bool) -> super::culture::Culture {
    let def = &world.settlements.def.cultures;
    let families = cell_families(world);
    let site = super::culture::Site { seed: world.settlements.seed, cell, at, level: world.level_at(at.x, at.y) as u32, fresh, relief: 0.3, forest: 0.3, levers: super::Levers { levers: [0.5; 5], axes: [0.5; 8] }, cell_family: &families };
    let mut c = def.roll(&site);
    def.finish(&mut c, &site, "village");
    c
}

/// A culture is a pure function of the seed and the place: the same twice, in a world built
/// again, every number in range, mutations as the rules say, and every borrowed entry one
/// its donor family has.
#[test]
fn a_culture_is_a_function_of_the_seed_and_the_place() {
    let (a, b) = (base_world(7), base_world(7));
    let da = &a.settlements.def.cultures;
    // The 144 cells, each on its own core (a roll reads the land's level); the counts are
    // taken from the results in cell order.
    let cells: Vec<IVec2> = (-6..6).flat_map(|cz| (-6..6).map(move |cx| IVec2::new(cx, cz))).collect();
    let counted: Vec<(bool, bool, bool)> = cells.par_iter().map(|&cell| {
        let at = cell * 1024 + 512;
        let (x, y) = (rolled(&a, cell, at, true), rolled(&b, cell, at, true));
        assert_eq!(x, y, "{cell}: rolled differently");
        assert!(x.axes.iter().all(|v| (0.0..=1.0).contains(v)), "{cell}: axes {:?}", x.axes);
        let f = &da.families[x.family];
        assert!(x.dropped.len() <= da.rules.drop.1 as usize, "{cell}: dropped {}", x.dropped.len());
        assert!(x.dropped.iter().all(|e| f.entries.contains(e) && e.mult > 1.0), "{cell}: dropped what the family does not favour");
        assert!(x.borrowed.len() <= da.rules.borrow as usize, "{cell}: borrowed {}", x.borrowed.len());
        for (donor, e) in &x.borrowed {
            assert!(*donor != x.family && da.families[*donor].entries.contains(e), "{cell}: borrowed {e:?} from {donor}, which does not have it");
        }
        let kept = x.palette.materials().iter().map(|m| m.0.len()).sum::<usize>();
        assert!((da.rules.keep.0 as usize..=da.rules.keep.1 as usize).contains(&kept), "{cell}: kept {kept} colours");
        assert!((1..=2).contains(&x.quirks.len()), "{cell}: {} quirks", x.quirks.len());
        if let Some(s) = &x.second {
            assert!(s.family != x.family && (0.1..=0.45).contains(&s.share) || x.claimed && s.share == 0.3, "{cell}: second family {} share {}", s.family, s.share);
        }
        (x.second.is_some(), !x.borrowed.is_empty(), x.claimed)
    }).collect();
    let made = counted.len();
    let second = counted.iter().filter(|c| c.0).count();
    let borrowed = counted.iter().filter(|c| c.1).count();
    let claimed = counted.iter().filter(|c| c.2).count();
    assert_eq!(made, 144);
    assert!(second > 5 && borrowed > 5 && claimed > 5, "{second} with a second family, {borrowed} borrowing, {claimed} claimed by river folk, of {made}");
    // The planned settlements carry the same culture in two worlds of one seed.
    let (pa, pb) = rayon::join(|| a.settlements_near(IVec2::ZERO, 4096), || b.settlements_near(IVec2::ZERO, 4096));
    assert!(pa.len() > 3, "{} settlements near", pa.len());
    assert_eq!(pa.iter().map(|s| &s.culture).collect::<Vec<_>>(), pb.iter().map(|s| &s.culture).collect::<Vec<_>>());
}

/// Neighbours share a family: regions 3 km apart have the same family at least 70% of the
/// time (a family is a region, not a speckle), over several seeds, in every direction; and
/// so do the planned settlements within 3 km of one another. Several families turn up.
#[test]
fn neighbouring_places_share_a_family() {
    let (mut same, mut pairs) = (0, 0);
    let mut seen = std::collections::BTreeSet::new();
    for seed in [1u64, 7, 42, 99, 0x5eed] {
        let world = shared_world(seed);
        let families = cell_families(&world);
        let region = |at: IVec2| world.settlements.def.cultures.region_at(world.settlements.seed, at, &families).0;
        // Each pair on its own core (a region lookup is a few milliseconds).
        let regions: Vec<(usize, usize)> = (0..400u64).into_par_iter().map(|k| {
            let h = crate::rng::hash3(seed, k as i32, 3, 99);
            let at = IVec2::new((h % 160_000) as i32 - 80_000, ((h >> 20) % 160_000) as i32 - 80_000);
            let angle = (h >> 40) as f32 / 1e6;
            let other = at + IVec2::new((angle.cos() * 3000.0) as i32, (angle.sin() * 3000.0) as i32);
            (region(at), region(other))
        }).collect();
        for (a, b) in regions {
            seen.insert(a);
            pairs += 1;
            same += usize::from(a == b);
        }
    }
    let share = same as f32 / pairs as f32;
    assert!(share >= 0.7, "{same} of {pairs} places 3 km apart share a family: {share:.2}");
    println!("culture regions: {same}/{pairs} neighbours share a family ({share:.3}), {} families seen", seen.len());
    let families = shared_world(1).settlements.def.cultures.families.len();
    assert!(seen.len() >= 9, "only {} of {families} families turn up as a region's: {seen:?}", seen.len());
    // Settlements near one another, as planned (some claimed by river folk, which is why
    // this one counts the region's family, the second of a claimed place).
    let (mut near_same, mut near_pairs) = (0, 0);
    let planned: Vec<_> = [1u64, 42].into_par_iter().map(|seed| shared_world(seed).settlements_near(IVec2::ZERO, 12_000)).collect();
    for all in planned {
        let region = |c: &super::CultureInfo| if c.family == "river" { c.second.as_ref().map(|s| s.0.clone()).unwrap_or_default() } else { c.family.clone() };
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                if IVec2::new(a.centre.x, a.centre.z).as_vec2().distance(IVec2::new(b.centre.x, b.centre.z).as_vec2()) <= 3000.0 {
                    near_pairs += 1;
                    near_same += usize::from(region(&a.culture) == region(&b.culture));
                }
            }
        }
    }
    assert!(near_pairs > 20, "only {near_pairs} settlements within 3 km of another");
    let share = near_same as f32 / near_pairs as f32;
    assert!(share >= 0.7, "{near_same} of {near_pairs} planned neighbours within 3 km share a family: {share:.2}");
    println!("culture settlements: {near_same}/{near_pairs} neighbours share a family ({share:.3})");
}

/// Every regional family can win somewhere it fits, including the rare ones: homogeneous
/// regions from the real climate picker, with the ordinary luck and neighbour smoothing.
/// This checks the catalogue independently of which rare biomes a geographical sample finds.
#[test]
fn every_regional_family_can_win_its_climate() {
    use crate::biome::Point;
    let world = base_world(1);
    let def = &world.settlements.def;
    let wanted: std::collections::BTreeSet<usize> = def.cultures.families.iter().enumerate()
        .filter(|(_, f)| f.claim == super::culture::Claim::Region && f.weight > 0.0)
        .map(|(i, _)| i).collect();
    let temperatures = [-9.0, -2.0, 2.0, 7.0, 13.0, 19.0, 27.0];
    let mut rng = crate::rng::Rng::new(1, 57, 0, 0);
    let mut climates = std::collections::BTreeSet::new();
    for k in 0..20_000 {
        let t = k % temperatures.len();
        let p = Point([temperatures[t], rng.unit() * 2.0, 0.05 + rng.unit() * 0.95,
            rng.unit(), rng.unit(), rng.unit(), rng.unit(), 1.0]);
        let id = world.biomes().resolve(&world.biomes().remap(p));
        if !def.avoid.iter().any(|tag| world.biomes().get(id).has_tag(tag)) {
            climates.insert((t, id));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    'seeds: for seed in 0..128u64 {
        for &(t, id) in &climates {
            let probe = |_: IVec2| (world.biomes().get(id), temperatures[t]);
            let cell = IVec2::new(seed as i32 - 64, 0);
            let fits = |g| def.cultures.cell_fits(seed, g, &probe);
            let family = def.cultures.cell_family(seed, cell, &fits);
            if fits(cell).1[family] > 0.0 && seen.insert(family) {
                println!("culture witness: {} in {} at {} C, seed {seed}, cell {cell}",
                    def.cultures.families[family].name, world.biomes().get(id).name, temperatures[t]);
            }
            if seen == wanted { break 'seeds }
        }
    }
    let missing: Vec<_> = wanted.difference(&seen).map(|&i| def.cultures.families[i].name.as_str()).collect();
    assert!(missing.is_empty(), "regional families cannot win in {} sampled climates: {missing:?}", climates.len());
    println!("all {} regional families can win in {} real biome/temperature fixtures", wanted.len(), climates.len());
}

/// Every biome a settlement may stand in suits two families or more, fully or at half
/// weight (temperatures aside).
#[test]
fn every_settleable_biome_has_two_families_or_more() {
    let world = base_world(1);
    let def = &world.settlements.def;
    let mut checked = 0;
    for (_, biome) in world.biomes().iter() {
        if biome.layer != crate::content::Layer::Surface || def.avoid.iter().any(|t| biome.has_tag(t)) {
            continue;
        }
        let suits: Vec<&str> = def
            .cultures
            .families
            .iter()
            .filter(|f| f.claim == super::culture::Claim::Region && f.weight > 0.0 && f.biome.iter().chain(&f.half).any(|t| biome.has_tag(t)))
            .map(|f| f.name.as_str())
            .collect();
        assert!(suits.len() >= 2, "biome {} ({:?}) suits {suits:?}", biome.name, biome.tags);
        checked += 1;
    }
    assert!(checked > 15, "only {checked} settleable biomes");
}

/// Every family's English and Japanese name parts join as the English does: wherever the
/// English runs a consonant into a vowel, a kana rule merges the Japanese too (Sab + ir is
/// サビル), so no name is left reading サブイル, and none doubles a kana (ツ + ツ). Kanji
/// families run their parts together as they are. A first part that ends in a vowel sound
/// the English spells with a consonant (w, y, h) is let be.
#[test]
fn every_family_names_join_under_the_kana_rules() {
    use super::culture::Script;
    let world = base_world(1);
    let def = &world.settlements.def;
    let vowel = |c: Option<char>| c.is_some_and(|c| "aeiou".contains(c.to_ascii_lowercase()));
    let mut bad = Vec::new();
    let mut pairs = 0;
    let (mut kanji, mut kana) = (0, 0);
    for f in &def.cultures.families {
        let n = &f.names;
        for first in &n.first {
            for last in &n.last {
                pairs += 1;
                if n.script == Script::Kanji {
                    kanji += 1;
                    if !first.1.chars().chain(last.1.chars()).all(|c| !('ぁ'..='ヿ').contains(&c)) {
                        bad.push(format!("{}: {}+{} / {}+{}: a kanji name with kana in it", f.name, first.0, last.0, first.1, last.1));
                    }
                    continue;
                }
                kana += 1;
                let joined = super::plan::join_kana(&def.kana, first, last);
                let (a, b) = (first.1.chars().last(), last.1.chars().next());
                if a == b && a.is_some_and(|c| c != 'ー' && !"アイウエオ".contains(c)) && joined.contains(&format!("{}{}", a.unwrap(), b.unwrap())) {
                    bad.push(format!("{}: {}+{} / {}+{}: doubled kana", f.name, first.0, last.0, first.1, last.1));
                }
                let end = first.0.chars().last();
                let runs_on = !vowel(end) && end.is_some_and(|c| c.is_ascii_alphabetic() && !"yhw".contains(c.to_ascii_lowercase())) && vowel(last.0.chars().next());
                let opening = last.1.chars().next().is_some_and(|c| "アイウエオ".contains(c));
                if !(runs_on && opening) {
                    continue;
                }
                let english = first.0.to_lowercase();
                if !def.kana.iter().any(|j| first.1.ends_with(&j.kana) && english.ends_with(&j.after)) {
                    bad.push(format!("{}: {}+{} / {}+{}", f.name, first.0, last.0, first.1, last.1));
                }
            }
        }
        for small in &n.small {
            assert!(!small.0.is_empty() && !small.1.is_empty(), "{}: an empty small name", f.name);
        }
    }
    assert!(pairs > 5000 && kanji > 100 && kana > 4000, "{pairs} name pairs, {kanji} in kanji");
    bad.dedup();
    assert!(bad.is_empty(), "{} names the kana table does not join:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
    // The new row: Itz + ak.
    let join = |a: (&str, &str), b: (&str, &str)| super::plan::join_kana(&def.kana, &(a.0.into(), a.1.into()), &(b.0.into(), b.1.into()));
    assert_eq!(join(("Itz", "イツ"), ("ak", "アク")), "イツァク");
    assert_eq!(join(("Ost", "オスト"), ("ak", "アク")), "オスタク");
    assert_eq!(join(("Oak", "オーク"), ("combe", "クーム")), "オークーム");
    assert_eq!(join(("Stor", "ストル"), ("lund", "ルンド")), "ストルンド");
    assert_eq!(join(("Oa", "オア"), ("ara", "アラ")), "オアアラ");
    assert_eq!(join(("Tor", "トー"), ("or", "ーア")), "トーーア");
}

/// A lean names a lever or an axis and nothing else (a file with another fails to load);
/// every axis is leaned on somewhere in settlements.ron or a quirk; every family tilts
/// plenty and has its eight colours.
#[test]
fn every_lean_names_a_known_lever_or_axis() {
    use super::culture::Axis;
    use super::def::{Key, Lean};
    let bad = std::collections::BTreeMap::from([("luck".to_owned(), 1.0f32)]);
    assert!(Lean::parse("t", &bad).is_err(), "an unknown lean passes");
    let ok = std::collections::BTreeMap::from([("piety".to_owned(), 1.0f32), ("wealth".to_owned(), 0.5)]);
    assert_eq!(Lean::parse("t", &ok).unwrap().0.len(), 2);
    let mut def = super::SettlementsDef::default();
    let err = def.extend("t", "base", r#"(houses: [(name: "x", weight: 1.0, lean: {"cleverness": 1.0}, width: (3, 3), depth: (3, 3), walls: (3, 3), storeys: (1, 1))])"#);
    assert!(err.is_err_and(|e| e.0.contains("no lever or axis is called `cleverness`")));
    let world = base_world(1);
    let def = &world.settlements.def;
    let mut leaned: std::collections::BTreeSet<Axis> = Default::default();
    let leans = def
        .sizes
        .iter()
        .map(|s| &s.lean)
        .chain(def.layouts.iter().map(|l| &l.lean))
        .chain(def.houses.iter().map(|h| &h.lean))
        .chain(def.extras.iter().map(|e| &e.lean))
        .chain(def.props.iter().map(|p| &p.lean))
        .chain(def.roofs.iter().map(|r| &r.lean))
        .chain(def.centrepieces.iter().map(|c| &c.lean))
        .chain(def.cultures.quirks.iter().map(|q| &q.lean))
        .chain(def.cultures.families.iter().flat_map(|f| f.names.prefixes.iter().map(|p| &p.3)));
    for lean in leans {
        leaned.extend(lean.0.iter().filter_map(|(k, _)| if let Key::Axis(a) = k { Some(*a) } else { None }));
    }
    for axis in Axis::ALL {
        assert!(leaned.contains(&axis), "no list leans on {}", axis.name());
    }
    assert_eq!(def.cultures.families.len(), 17);
    assert_eq!(def.cultures.quirks.len(), 40);
    for f in &def.cultures.families {
        assert!(f.entries.len() >= 20, "family {} tilts only {} entries", f.name, f.entries.len());
        assert_eq!(f.palette.materials().iter().map(|m| m.0.len()).sum::<usize>(), 8, "family {}: not eight colours", f.name);
    }
}

/// Mutated and quirked family weights favour glen towers over hearth towers. Sample the
/// same bounded house-selection loop the planner uses, without planning thousands of squares.
#[test]
fn a_family_shows_in_what_it_builds() {
    use super::culture::{Listing, Site};
    let world = base_world(1);
    let def = &world.settlements.def;
    let towers = |name: &str| {
        let family = def.cultures.families.iter().position(|f| f.name == name).unwrap();
        let fixed_family = |_| family;
        let mut towers = 0;
        for k in 0..256 {
            let site = Site {
                seed: world.settlements.seed, cell: IVec2::new(k, 0), at: IVec2::new(k * 1024, 0),
                level: 1, fresh: false, relief: 0.3, forest: 0.3,
                levers: super::Levers { levers: [0.5; 5], axes: [0.5; 8] }, cell_family: &fixed_family,
            };
            let mut culture = def.cultures.roll(&site);
            def.cultures.finish(&mut culture, &site, "village");
            let levers = super::Levers { axes: culture.axes, ..site.levers };
            let weights: Vec<_> = def.houses.iter().enumerate().filter(|(_, h)| h.work.is_none())
                .map(|(i, h)| (i, h.lean.weigh(h.weight, &levers) * culture.mult(Listing::House, &h.name))).collect();
            let mut rng = crate::rng::Rng::new(site.seed, 123, k, 0);
            for _ in 0..16 {
                let i = super::plan::pick(&mut rng, &weights).unwrap();
                towers += usize::from(def.houses[i].name == "tower_house");
            }
        }
        towers
    };
    let (glen, hearth) = (towers("glen"), towers("hearth"));
    assert!(glen > hearth * 2, "tower draws: glen {glen}, hearth {hearth}");

}

/// Mutations make places of a family unalike: across many places the quirks are many, a
/// good share borrow something, and few share every entry.
#[test]
fn settlements_of_a_family_are_not_alike() {
    use super::culture::Site;
    let world = base_world(1);
    let mut def = world.settlements.def.cultures.clone();
    // Hold the family fixed so the test measures mutations within one family. Claims
    // and the real region field are covered separately.
    def.rules.claim.0 = 0.0;
    let family = def.families.iter().position(|f| f.name == "hearth").unwrap();
    let fixed_family = |_| family;
    let mut quirks = std::collections::BTreeSet::new();
    let mut borrowed = 0;
    let mut sets = std::collections::BTreeSet::new();
    for k in 0..300i32 {
        let cell = IVec2::new(k, 5);
        let site = Site {
            seed: world.settlements.seed, cell, at: cell * 1024 + 300, level: 1,
            fresh: k % 2 == 0, relief: 0.3, forest: 0.3,
            levers: super::Levers { levers: [0.5; 5], axes: [0.5; 8] }, cell_family: &fixed_family,
        };
        let mut c = def.roll(&site);
        def.finish(&mut c, &site, "village");
        assert_eq!(c.family, family);
        quirks.extend(c.quirks.iter().copied());
        borrowed += usize::from(!c.borrowed.is_empty());
        sets.insert((c.family, c.entries.iter().map(|e| format!("{}{}{}", e.listing.name(), e.name, e.mult)).collect::<Vec<_>>(), format!("{:?}", c.palette.walls.0)));
    }
    assert!(quirks.len() >= 25, "{} quirks over 300 places", quirks.len());
    assert!(borrowed >= 20, "only {borrowed} of 300 borrowed anything");
    assert!(sets.len() >= 100, "{} different entry sets in 300 places", sets.len());
}

/// Tuning aid for the region field: the share of pairs 3 km apart that share a family under
/// different luck, computed from one pass over the land (`cargo test ... region_tuning
/// -- --ignored --nocapture`).
#[test]
#[ignore]
fn region_tuning() {
    let world = base_world(42);
    let def = &world.settlements.def.cultures;
    let seed = world.settlements.seed;
    let c = def.rules.field.cell;
    let mut fits = rustc_hash::FxHashMap::<IVec2, (IVec2, Vec<f32>)>::default();
    for gz in -8..8 {
        for gx in -8..8 {
            let g = IVec2::new(gx, gz);
            let probe = |p: IVec2| {
                let s = world.site(p.x, p.y);
                (world.biomes().get(s.biome), s.temperature)
            };
            fits.insert(g, def.cell_fits(seed, g, &probe));
        }
    }
    for (luck, luck_cell, smooth) in [(0.6, 18432, 0.05), (0.6, 18432, 0.08), (0.6, 18432, 0.12)] {
        let mut d = def.clone();
        d.rules.luck = luck;
        d.rules.luck_cell = luck_cell;
        let pick: rustc_hash::FxHashMap<IVec2, usize> = fits
            .iter()
            .map(|(g, (m, f))| {
                let mut sum = f.clone();
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if (dx, dz) == (0, 0) {
                            continue;
                        }
                        if let Some((_, n)) = fits.get(&(*g + IVec2::new(dx, dz))) {
                            for (s, v) in sum.iter_mut().zip(n) {
                                *s += v * smooth;
                            }
                        }
                    }
                }
                (*g, d.cell_pick(seed, *m, &sum))
            })
            .collect();
        let family = |g: IVec2| pick.get(&g).copied().unwrap_or(0);
        let (mut same, mut pairs, mut kinds) = (0, 0, std::collections::BTreeSet::new());
        for k in 0..4000u64 {
            let h = crate::rng::hash3(1, k as i32, 3, 99);
            let at = IVec2::new((h % (4 * c as u64)) as i32 - 2 * c, ((h >> 20) % (4 * c as u64)) as i32 - 2 * c);
            let angle = (h >> 40) as f32 / 1e6;
            let other = at + IVec2::new((angle.cos() * 3000.0) as i32, (angle.sin() * 3000.0) as i32);
            let (a, b) = (d.region_at(seed, at, &family).0, d.region_at(seed, other, &family).0);
            kinds.insert(a);
            pairs += 1;
            same += usize::from(a == b);
        }
        let all: std::collections::BTreeSet<usize> = pick.values().copied().collect();
        println!("luck {luck} over {luck_cell} smooth {smooth}: {:.3} share, {} families over {} cells", same as f32 / pairs as f32, all.len(), pick.len());
    }
}

/// A site is as likely in a biome of level 5 or below as anywhere, and keeps `levels.1` of
/// its chance above level 5 (design asked "above 3"; at 3 a regional crop's villages thin
/// out too far, see `regional_crop_reach`).
#[test]
fn sites_thin_out_above_their_level() {
    let site = super::def::SiteDef::default();
    assert_eq!((site.level(1), site.level(5), site.level(6), site.level(10)), (1.0, 1.0, 0.7, 0.7));
}

/// Culture loading rejects invalid rules and catalogue references before planning.
#[test]
fn culture_load_checks_reject_invalid_numbers_and_references() {
    let world = base_world(1);
    let base = &world.settlements.def;
    let refused = |change: &dyn Fn(&mut super::culture::CulturesDef)| {
        let mut cultures = base.cultures.clone();
        change(&mut cultures);
        cultures.check(base, &base_registry()).is_err()
    };
    assert!(!refused(&|_| {}));
    assert!(refused(&|c| c.rules.borrow = u32::MAX));
    assert!(refused(&|c| c.rules.drop.1 = u32::MAX));
    assert!(refused(&|c| c.rules.keep = (3, u32::MAX)));
    assert!(refused(&|c| c.rules.local = f32::NAN));
    assert!(refused(&|c| c.rules.field.border = f32::INFINITY));
    assert!(refused(&|c| c.rules.field.share.1 = f32::NAN));
    assert!(refused(&|c| c.rules.level_martial.1 = f32::NAN));
    assert!(refused(&|c| c.families[0].weight = f32::NAN));
    assert!(refused(&|c| c.families[0].weight = f32::MAX));
    assert!(refused(&|c| c.quirks[0].weight = f32::MAX));
    assert!(refused(&|c| c.families[0].names.prefixes[0].2 = f32::MAX));
    assert!(refused(&|c| { c.families[0].quirks.insert("bell_mad".into(), f32::MAX); }));
    assert!(refused(&|c| c.families[0].palette.walls.0[0].1 = f32::INFINITY));
    assert!(refused(&|c| c.families[0].palette.walls.0[0].1 = f32::MAX));
    assert!(refused(&|c| c.families[0].palette.walls.0[0].0 = "base:missing".into()));
    assert!(refused(&|c| c.families[0].entries[0].name = "missing".into()));
    assert!(refused(&|c| { c.families[0].quirks.insert("missing".into(), 1.0); }));
    assert!(refused(&|c| c.quirks[0].axes[0] = f32::NAN));
    assert!(super::def::Lean::parse("test", &std::collections::BTreeMap::from([("piety".into(), f32::NAN)])).is_err());
}

/// A later package can change one culture rule without resetting the other values,
/// and WorldContent exposes the package and exact field it replaced.
#[test]
fn culture_rules_are_layered_and_report_replacements() {
    let registry = base_registry();
    let mut files: Vec<_> = crate::content::BASE_FILES.iter().map(|(name, source)|
        voxmar_engine::content::ContentFile { package: "base".into(), name: (*name).into(), source: (*source).into() }).collect();
    files.push(voxmar_engine::content::ContentFile { package: "mod".into(), name: "cultures.ron".into(),
        source: "(rules: (field: (border: 400.0), size_shift: {\"town\": (trade: 0.2)}))".into() });
    let content = crate::content::WorldContent::from_files(files.iter(), &registry).unwrap();
    assert!(content.notes.iter().any(|n| n.ends_with("(rules.field.border)")));
    let cultures = &content.settlements.cultures;
    assert_eq!(cultures.rules.field.border, 400.0);
    assert_eq!(cultures.rules.field.cell, 6144);
    assert_eq!(cultures.rules.size_shift["town"].trade, Some(0.2));
    assert_eq!(cultures.rules.size_shift["town"].hierarchy, Some(0.1));
    assert_eq!(cultures.families.len(), 17);
    assert!(cultures.notes.iter().any(|n| n == "mod/cultures.ron replaces the cultures.ron base set (rules.field.border)"));
    assert!(cultures.notes.iter().any(|n| n.ends_with("(rules.size_shift.town.trade)")));
}

/// Large palettes keep exactly the selected entries; indices after 15 never alias.
#[test]
fn culture_palettes_keep_their_count_with_large_mod_lists() {
    use super::culture::{Claim, Site};
    let world = base_world(1);
    let mut def = world.settlements.def.cultures.clone();
    def.families[0].claim = Claim::Region;
    def.families[0].palette.walls.0 = (0..32).map(|_| ("base:white_terracotta".to_owned(), 1.0)).collect();
    let family = |_| 0;
    let mut later = false;
    for k in 0..32 {
        let site = Site { seed: k as u64, cell: IVec2::new(k, 0), at: IVec2::ZERO, level: 1, fresh: false,
            relief: 0.0, forest: 0.0, levers: super::Levers::default(), cell_family: &family };
        let c = def.roll(&site);
        let kept = c.palette.materials().iter().map(|m| m.0.len()).sum::<usize>();
        assert!((3..=5).contains(&kept), "kept {kept}");
        later |= !c.palette.walls.0.is_empty();
    }
    assert!(later);
}

/// Region memo eviction and concurrent asks leave the culture unchanged.
#[test]
fn culture_generation_is_independent_of_order_and_threads() {
    let world = base_world(7);
    let cells: Vec<_> = (0..12).map(|k| IVec2::new(k - 6, k % 3)).collect();
    let expected: Vec<_> = cells.iter().map(|&c| rolled(&world, c, c * 1024 + 300, true)).collect();
    // Force terrain fits to be evicted before every next ask.
    let mut other = base_world(7);
    other.settlements.cells = crate::water::cache::Cache::new(1);
    for (i, &c) in cells.iter().enumerate().rev() {
        assert_eq!(rolled(&other, c, c * 1024 + 300, true), expected[i]);
    }
    std::thread::scope(|scope| {
        for (i, &c) in cells.iter().enumerate() {
            let (other, expected) = (&other, &expected);
            scope.spawn(move || assert_eq!(rolled(other, c, c * 1024 + 300, true), expected[i]));
        }
    });
}

/// Borders blend weights, water quirks require water, and level shifts use the rules.
#[test]
fn culture_borders_site_requirements_and_levels() {
    use super::culture::{Axis, Need, Site};
    let world = base_world(1);
    let mut def = world.settlements.def.cultures.clone();
    def.rules.claim.0 = 0.0;
    def.rules.second_quirk = 0.0;
    let family = |g: IVec2| g.x.rem_euclid(2) as usize;
    let site_at = |at| Site { seed: 12, cell: IVec2::ZERO, at, level: 1, fresh: false,
        relief: 0.0, forest: 0.0, levers: super::Levers::default(), cell_family: &family };
    let edge = (0..6144).step_by(8).map(|x| IVec2::new(x, 2000)).find(|&p| {
        let (a, b, edge) = def.region_at(12, p, &family);
        a != b && edge < 400.0
    }).expect("a border in the test strip");
    let site = site_at(edge);
    let low = def.roll(&site);
    let second = low.second.as_ref().expect("a border mixes families");
    assert!((0.1..=0.45).contains(&second.share));
    let high = def.roll(&Site { level: 7, ..site_at(edge) });
    assert!(high.axes[Axis::Martial as usize] > low.axes[Axis::Martial as usize]);
    assert!(high.axes[Axis::Piety as usize] > low.axes[Axis::Piety as usize]);
    def.quirks.retain(|q| q.need == Need::Water);
    let mut dry = def.roll(&site);
    def.finish(&mut dry, &site, "village");
    assert!(dry.quirks.is_empty());
    let wet_site = Site { fresh: true, ..site_at(edge) };
    let mut wet = def.roll(&wet_site);
    def.finish(&mut wet, &wet_site, "village");
    assert_eq!(wet.quirks.len(), 1);
}

/// Styles see quirks as every other catalogue does. A style-only quirk can remove all
/// eligible styles, so planning must refuse the site rather than choose a style early.
#[test]
fn culture_quirks_are_applied_before_style_selection() {
    use super::culture::{Entry, Listing, Need};
    let registry = base_registry();
    let mut content = crate::content::WorldContent::base(&registry).unwrap();
    let ordinary = shared_world(7);
    assert!(!ordinary.settlements_near(IVec2::ZERO, 2048).is_empty());
    let def = &mut content.settlements;
    def.cultures.quirks.truncate(1);
    let q = &mut def.cultures.quirks[0];
    q.weight = 1.0;
    q.need = Need::Nothing;
    q.lean = Default::default();
    q.axes = [0.0; 8];
    q.entries = def.styles.iter().map(|s| Entry { listing: Listing::Style, name: s.name.clone(), mult: 0.0 }).collect();
    for f in &mut def.cultures.families {
        f.quirks.clear();
    }
    def.cultures.check(def, &registry).unwrap();
    let quirky = Overworld::with_content(7, registry, &content);
    assert!(quirky.settlements_near(IVec2::ZERO, 2048).is_empty(), "styles were picked before their zero quirk weights applied");
}

/// A palette must resolve mixed wood/trim pairs, even when diagonal pairs exist.
#[test]
fn culture_palette_checks_every_wood_and_trim_combination() {
    let world = base_world(1);
    let mut def = world.settlements.def.clone();
    let mut blocks = BlockRegistry::parse("base", include_str!("../../../../assets/content/base/blocks.ron")).unwrap();
    let template = blocks.iter().find(|b| b.name == "base:oak_planks").unwrap().clone();
    for style in &mut def.styles {
        style.woods.truncate(1);
    }
    def.styles[0].woods = vec![("a".into(), 1.0)];
    let woods: std::collections::BTreeSet<_> = def.styles.iter().flat_map(|s| s.woods.iter().map(|w| w.0.clone())).chain(["b".into()]).collect();
    for wood in woods {
        let mut block = template.clone();
        block.name = format!("base:{wood}_{wood}_planks");
        blocks.push(block);
    }
    let registry = BlockRegistry::new(blocks).unwrap();
    for family in &mut def.cultures.families {
        for material in [&mut family.palette.walls, &mut family.palette.trim, &mut family.palette.roof] {
            for (name, _) in &mut material.0 {
                if name.contains('{') {
                    *name = "base:oak_planks".into();
                }
            }
        }
    }
    def.cultures.families[0].palette.walls.0[0].0 = "base:{wood}_{trim}_planks".into();
    def.cultures.check(&def, &registry).unwrap();
    def.styles[0].woods.push(("b".into(), 1.0));
    let error = def.cultures.check(&def, &registry).unwrap_err();
    assert!(error.0.contains("base:a_b_planks"), "{error}");
}

/// Spare allocation capacity must count towards the plan cache, even when its lengths
/// and its generated output have not changed.
#[test]
fn culture_plan_weight_counts_spare_allocations() {
    use crate::water::cache::Weigh;
    let world = base_world(7);
    let spawn = world.spawn_point();
    let found = world.settlements.near(&world, IVec2::new(spawn.x, spawn.z), 2048);
    let plan = found.iter().find_map(|s| s.0.as_ref()).expect("a settlement near spawn");
    let cell = world.settlements.cell_of(plan.centre.x, plan.centre.y);
    let spawn_at = world.settlements.spawn.get().and_then(|s| s.square).filter(|s| s.1 == cell).map(|s| s.0);
    let again = world.settlements.plan(&world, world.settlements.fixed.as_ref().unwrap(), cell, spawn_at).unwrap();
    let mut slot = super::Slot(Some(again));
    let plan = slot.0.as_mut().unwrap();
    plan.name.0.shrink_to_fit();
    plan.gates.shrink_to_fit();
    plan.lanes[0].shrink_to_fit();
    let before = slot.bytes();
    let plan = slot.0.as_mut().unwrap();
    plan.name.0.reserve_exact(4096);
    plan.gates.reserve_exact(1024);
    plan.lanes[0].reserve_exact(1024);
    assert!(slot.bytes() >= before + 4096 + 1024 * std::mem::size_of::<IVec2>() + 1024 * std::mem::size_of::<Vec2>());
    // Villagers adds resident records, owned species names and room columns. Their
    // spare allocations must count even when the population and room stay unchanged.
    let plan = slot.0.as_mut().unwrap();
    assert!(!plan.people.is_empty());
    let room = plan.buildings.iter().position(|b| !b.room.is_empty()).expect("an indoor room");
    plan.people.shrink_to_fit();
    plan.people[0].species.shrink_to_fit();
    plan.buildings[room].room.shrink_to_fit();
    let before = slot.bytes();
    let plan = slot.0.as_mut().unwrap();
    plan.people.reserve_exact(1024);
    plan.people[0].species.reserve_exact(4096);
    plan.buildings[room].room.reserve_exact(1024);
    assert!(slot.bytes() >= before + 1024 * std::mem::size_of::<super::people::Resident>() + 4096 + 1024 * std::mem::size_of::<IVec2>());
}

/// Creeks missed by the coarse grid require a closer search, but water-positive sites
/// beyond the examined prefix must not cost more grids. Dry candidates still get the
/// full search allowance: stop after qualifying sites, not before filtering them.
#[test]
fn site_water_search_stops_after_the_used_prefix() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::geology::GeoSample;
    use crate::trails::Land;
    struct Creeks {
        step: i32,
        creek: i32,
        wet: bool,
        close: AtomicUsize,
    }
    impl Land for Creeks {
        fn sample(&self, x: i32, _z: i32) -> GeoSample {
            let wet = self.wet && x.rem_euclid(self.step) == self.creek;
            GeoSample { height: if wet { 139 } else { 140 }, water: 140, mountain: 0.0, river: 0.0,
                top: 8, body: crate::Body::Creek, current: None, cavity: None, crag: Default::default(),
                trail: None, form: Default::default() }
        }
        fn grid(&self, origin: IVec2, step: i32, size: i32) -> Vec<GeoSample> {
            if step == self.creek { self.close.fetch_add(1, Ordering::Relaxed); }
            (0..size * size).map(|i| self.sample(origin.x + i % size * step, origin.y + i / size * step)).collect()
        }
    }
    let world = base_world(1);
    let mut def = world.settlements.def.clone();
    def.avoid.clear();
    def.site.mild = (-1000.0, 1000.0);
    def.site.levels = (7, 1.0);
    def.site.caves = 0;
    for spawn in [None, Some(IVec2::splat(def.grid.spacing / 2))] {
        let tries = def.site.tries * if spawn.is_some() { 8 } else { 1 };
        for (wet, usable) in [(true, true), (true, false), (false, true), (false, false)] {
            def.site.temperature = if usable { (-1000.0, 1000.0) } else { (10_000.0, 10_001.0) };
            let land = Creeks { step: def.site.step, creek: def.site.creek, wet, close: AtomicUsize::new(0) };
            let ctx = super::plan::Ctx { world: &world, land: &land, def: &def, resolved: &world.settlements.resolved,
                fixed: world.settlements.fixed.as_ref().unwrap(), seed: world.settlements.seed,
                spawn, promised: &[], cells: &world.settlements.cells };
            let found = super::plan::site(&ctx, IVec2::ZERO, spawn);
            let used = if spawn.is_some() { 8 } else { def.site.others + 1 };
            assert_eq!(found.len(), if wet && usable { used } else { 0 }, "spawn {spawn:?}, wet {wet}, usable {usable}");
            assert!(found.iter().all(|s| s.1), "every selected site has fresh water");
            let expected = if !wet { tries * def.site.look } else if usable { used } else { tries };
            assert_eq!(land.close.load(Ordering::Relaxed), expected,
                "spawn {spawn:?}, wet {wet}, usable {usable}: unused candidates were probed or dry ones skipped");
        }
    }
}

/// Repeat one known expensive whole plan for an external CPU sampler. This diagnostic
/// does not supply acceptance timings; culture_planning_budget retains all 20 seeds.
#[test]
#[ignore]
fn culture_planning_hotspots() {
    assert!(!cfg!(debug_assertions), "run this diagnostic with --release");
    crate::perf::prefer_fast_cores();
    crate::parallel::set_enabled(false);
    struct Helpers;
    impl Drop for Helpers {
        fn drop(&mut self) { crate::parallel::set_enabled(true); }
    }
    let _helpers = Helpers;
    let world = base_world(14);
    let spawn = world.spawn_point();
    let centre = IVec2::new(spawn.x, spawn.z);
    let found = world.settlements.near(&world, centre, 2048);
    let plan = found.iter().filter_map(|s| s.0.as_ref()).find(|p| p.size == "village").expect("seed 14 village");
    let cell = world.settlements.cell_of(plan.centre.x, plan.centre.y);
    let spawn_at = world.settlements.spawn.get().and_then(|s| s.square).filter(|s| s.1 == cell).map(|s| s.0);
    eprintln!("planning sampler ready: pid {}, centre {}", std::process::id(), plan.centre);
    std::thread::sleep(std::time::Duration::from_secs(2));
    for _ in 0..512 {
        let again = world.settlements.plan(&world, world.settlements.fixed.as_ref().unwrap(), cell, spawn_at).unwrap();
        assert_eq!((again.centre, &again.name, &again.culture), (plan.centre, &plan.name, &plan.culture));
        std::hint::black_box(again);
    }
}

/// Compare the complete planner with its original empty layout cache and per-row
/// terrain batches over all twenty budget seeds. This is an output oracle, not timing.
#[test]
#[ignore]
fn terrain_batches_preserve_all_twenty_seed_plans() {
    use crate::water::cache::Weigh;
    crate::parallel::set_enabled(false);
    struct Helpers;
    impl Drop for Helpers {
        fn drop(&mut self) { crate::parallel::set_enabled(true); }
    }
    let _helpers = Helpers;
    let mut counts = [0; 2];
    for seed in 1..=20 {
        let (world, found) = super::plan::with_original_rows(|| {
            let world = base_world(seed);
            let spawn = world.spawn_point();
            let centre = IVec2::new(spawn.x, spawn.z);
            let mut found = world.settlements.near(&world, centre, 2048);
            if !found.iter().any(|s| s.0.as_ref().is_some_and(|p| p.size == "village")) {
                found = world.settlements.near(&world, centre, 4096);
            }
            (world, found)
        });
        for (i, size) in ["hamlet", "village"].into_iter().enumerate() {
            let Some(slot) = found.iter().find(|s| s.0.as_ref().is_some_and(|p| p.size == size)) else { continue };
            let known = slot.0.as_ref().unwrap();
            let cell = world.settlements.cell_of(known.centre.x, known.centre.y);
            let spawn_at = world.settlements.spawn.get().and_then(|s| s.square).filter(|s| s.1 == cell).map(|s| s.0);
            let replan = || world.settlements.plan(&world, world.settlements.fixed.as_ref().unwrap(), cell, spawn_at).unwrap();
            let original = super::Slot(Some(super::plan::with_original_rows(replan)));
            let batched = super::Slot(Some(replan()));
            let (a, b) = (original.0.as_ref().unwrap(), batched.0.as_ref().unwrap());
            assert_eq!((a.id, &a.size, &a.layout, &a.style, &a.name, a.centre, a.height),
                (b.id, &b.size, &b.layout, &b.style, &b.name, b.centre, b.height), "seed {seed} {size}");
            assert_eq!(a.levers, b.levers);
            assert_eq!(a.culture, b.culture);
            assert_eq!(a.buildings, b.buildings);
            assert_eq!(a.gates, b.gates);
            assert_eq!(a.lanes, b.lanes);
            assert_eq!(a.lane_halves, b.lane_halves);
            assert_eq!(a.people, b.people);
            assert_eq!((a.min, a.max, a.raster.min, a.raster.max), (b.min, b.max, b.raster.min, b.raster.max));
            let cells = |p: &super::Plan| p.raster.cells.iter().map(|c| (c.target, c.weight, c.core, c.tread, c.clear)).collect::<Vec<_>>();
            assert_eq!(cells(a), cells(b));
            let sums = |p: &super::Plan| p.raster.sums.iter().map(|s| (s.0.to_bits(), s.1.to_bits(), s.2.to_bits())).collect::<Vec<_>>();
            assert_eq!(sums(a), sums(b));
            assert_eq!(a.voxels, b.voxels, "exact order of every column's voxels");
            assert_eq!(original.bytes(), batched.bytes(), "retained allocations");
            for (column, voxels) in &a.voxels { assert_eq!(voxels.capacity(), b.voxels[column].capacity()); }
            counts[i] += 1;
        }
    }
    assert_eq!(counts, [17, 20], "all original budget plans must be compared");
    println!("terrain-batch original oracle: {} complete plans over all 20 seeds", counts.iter().sum::<usize>());
}

/// Planning budgets are measured in release with thread CPU time, after the surrounding
/// land is known. Report cold region discovery separately in settlement_preview.
#[test]
#[ignore]
fn culture_planning_budget() {
    use crate::water::cache::Weigh;
    assert!(!cfg!(debug_assertions), "run this budget with --release");
    crate::perf::prefer_fast_cores();
    // Keep all measured work on this thread; a cache miss must not hide helper CPU.
    crate::parallel::set_enabled(false);
    struct Helpers;
    impl Drop for Helpers {
        fn drop(&mut self) { crate::parallel::set_enabled(true); }
    }
    let _helpers = Helpers;
    let profile = std::env::var_os("PROFILE_PLAN").is_some();
    let mut failures = Vec::new();
    let mut count = 0;
    let mut peak = 0usize;
    let mut times: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for seed in 1..=20 {
        let world = base_world(seed);
        let spawn = world.spawn_point();
        let centre = IVec2::new(spawn.x, spawn.z);
        let mut found = world.settlements.near(&world, centre, 2048);
        if !found.iter().any(|s| s.0.as_ref().is_some_and(|p| p.size == "village")) {
            found = world.settlements.near(&world, centre, 4096);
        }
        for size in ["hamlet", "village", "town"] {
            let Some(slot) = found.iter().find(|s| s.0.as_ref().is_some_and(|p| p.size == size)) else { continue };
            let plan = slot.0.as_ref().unwrap();
            let cell = world.settlements.cell_of(plan.centre.x, plan.centre.y);
            let spawn_at = world.settlements.spawn.get().and_then(|s| s.square).filter(|s| s.1 == cell).map(|s| s.0);
            if profile { eprintln!("budget seed {seed} {size} at {}", plan.centre); }
            super::plan::profile(profile);
            let start = crate::perf::thread_time();
            let replan = || world.settlements.plan(&world, world.settlements.fixed.as_ref().unwrap(), cell, spawn_at).unwrap();
            let (again, stages) = if profile { crate::stage::measure(replan) } else { (replan(), crate::stage::Times::default()) };
            let ms = (crate::perf::thread_time() - start).as_secs_f64() * 1000.0;
            super::plan::profile(false);
            if profile {
                eprintln!("  plan total {ms:.3} ms CPU; river-region creation {:.3} ms elapsed", stages.get(crate::stage::Stage::RiverRegions).as_secs_f64() * 1000.0);
            }
            assert_eq!(again.culture, plan.culture);
            let budget = match plan.size.as_str() { "hamlet" => 3.0, "village" => 6.0, "town" => 14.0, other => panic!("unbudgeted size {other}") };
            if ms > budget * 2.0 {
                failures.push(format!("seed {seed} {} at {}: {ms:.2} ms CPU, budget {budget} ms", plan.size, plan.centre));
            }
            times.entry(plan.size.clone()).or_default().push(ms);
            let bytes = slot.bytes();
            if bytes > 2 * super::MB {
                failures.push(format!("seed {seed} {size} at {}: {bytes} allocated bytes exceeds 2 MiB", plan.centre));
            }
            peak = peak.max(bytes);
            count += 1;
        }
    }
    for size in ["hamlet", "village"] {
        assert!(times.get(size).is_some_and(|ms| ms.len() >= 5), "only {:?} {size} samples over 20 seeds", times.get(size).map(Vec::len));
    }
    for (size, mut ms) in times {
        ms.sort_by(f64::total_cmp);
        println!("culture budget: {size}, {} plans, median {:.2} ms CPU, p99/max {:.2} ms CPU", ms.len(), ms[ms.len()/2], ms[ms.len()-1]);
    }
    assert!(count >= 20, "only {count} plans");
    println!("culture budget: {count} plans over 20 seeds, peak {peak} bytes including culture");
    assert!(failures.is_empty(), "planning budgets failed:\n{}", failures.join("\n"));
}

/// Towns are rare and big, and most are walled: a ring round their square and buildings
/// that cuts no building, leaves a gate wherever a lane passes, and stands a tower at least
/// every `towers` blocks; a village is now and then walled, a hamlet never.
#[test]
fn towns_are_big_rare_and_walled_round_their_heart() {
    use super::house::Kind;
    let (mut sizes, mut walled) = (std::collections::BTreeMap::<String, usize>::new(), std::collections::BTreeMap::<String, usize>::new());
    let mut towns = Vec::new();
    for seed in [1u64, 42, 7] {
        let world = shared_world(seed);
        let rules = &world.settlements.def.rules.walls;
        for cz in -5..5 {
            for cx in -5..5 {
                let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
                let Some(p) = &slot.0 else { continue };
                *sizes.entry(p.size.clone()).or_default() += 1;
                if p.walls.is_empty() {
                    continue;
                }
                *walled.entry(p.size.clone()).or_default() += 1;
                assert!(p.wall_block.is_some(), "seed {seed}, {}: a wall of nothing", p.name.0);
                for &(at, top) in &p.walls {
                    assert!((2..=12).contains(&top), "seed {seed}, {}: the wall at {at} stands {top} high", p.name.0);
                    for b in &p.buildings {
                        let inside = at.cmpge(b.min - 1).all() && at.cmple(b.max + 1).all();
                        assert!(!inside, "seed {seed}, {}: the wall at {at} runs into the {} at {}..{}", p.name.0, b.what, b.min, b.max);
                    }
                    // The wall leaves every lane a way through: none of it stands on a lane.
                    for (lane, half) in p.lanes.iter().zip(&p.lane_halves) {
                        let near = lane.windows(2).flat_map(|w| (0..=16).map(move |k| w[0].lerp(w[1], k as f32 / 16.0))).map(|q| q.distance(at.as_vec2())).fold(f32::MAX, f32::min);
                        assert!(near > *half, "seed {seed}, {}: the wall at {at} stands {near:.1} from a lane {half} wide each side", p.name.0);
                    }
                }
                // Towers stand along it: the tallest columns, never too far apart round it.
                let tall = p.walls.iter().map(|w| w.1).max().unwrap();
                let towers = p.walls.iter().filter(|w| w.1 == tall).count();
                assert!(towers >= 4, "seed {seed}, {}: {towers} tower corners along {} blocks of wall", p.name.0, p.walls.len());
                if p.size == "town" {
                    let homes = p.buildings.iter().filter(|b| b.kind == Kind::House && b.work.is_none()).count();
                    towns.push((seed, p.name.0.clone(), homes, p.walls.len(), rules.towers.0));
                }
            }
        }
    }
    eprintln!("sizes {sizes:?}, walled {walled:?}, towns {towns:?}");
    // The ring closes round every lot it would have run against (a field, a pen, a shrine).
    assert_eq!(super::plan::WALL_ON_LOTS.load(std::sync::atomic::Ordering::Relaxed), 0, "a wall left open against a lot");
    let all: usize = sizes.values().sum();
    let town_count = sizes.get("town").copied().unwrap_or(0);
    assert!(town_count >= 2 && town_count * 100 <= all * 15, "{town_count} towns in {all} settlements");
    assert!(walled.get("hamlet").is_none(), "a hamlet walled");
    assert!(walled.get("town").copied().unwrap_or(0) * 2 >= town_count, "few towns walled: {walled:?} of {town_count}");
    assert!(towns.iter().all(|t| t.2 >= 14), "a small town: {towns:?}");
}

/// Homes are furnished for who lives in them (`interiors`): beds for their residents,
/// tagged `bed` where the plan says they stand, a table with seats round it, and every bed
/// and seat on the ground floor reached on foot from the door, which nothing stands in
/// front of. `--nocapture` prints a few floor plans.
#[test]
fn homes_are_furnished_for_their_people() {
    use super::canvas::Put;
    use rustc_hash::FxHashMap;
    let registry = base_registry();
    let content = crate::content::WorldContent::base(&registry).unwrap();
    let solid = |b: Option<&BlockId>| b.is_some_and(|&b| registry.info(b).solid);
    let tagged = |b: Option<&BlockId>, tag: &str| b.is_some_and(|&b| registry.def(b).has_tag(tag));
    let (mut homes, mut wanted, mut beds, mut tables, mut seats, mut tops, mut surfaces, mut stairs, mut bare) = (0, 0, 0, 0, 0, 0, 0, 0, Vec::new());
    let mut shown = 0;
    for seed in [42u64, 1, 7] {
        let world = Overworld::with_content(seed, registry.clone(), &content);
        let spawn = world.spawn_point();
        for slot in world.settlements.near(&world, IVec2::new(spawn.x, spawn.z), 2500) {
            let Some(plan) = slot.0.as_ref() else { continue };
            let mut at: FxHashMap<IVec3, BlockId> = FxHashMap::default();
            for v in plan.voxels.values().flatten().filter(|v| !v.ground) {
                match v.put {
                    Put::Set(b) | Put::Soft(b) | Put::Down(b) => at.insert(v.pos, b),
                    Put::Clear => at.insert(v.pos, BlockId::AIR),
                };
            }
            for (i, b) in plan.buildings.iter().enumerate() {
                if b.kind != super::house::Kind::House || b.work.is_some() || b.room.is_empty() {
                    continue;
                }
                homes += 1;
                let residents = plan.people.iter().filter(|r| r.home == i).count();
                wanted += residents.min(4);
                beds += b.beds.len();
                seats += b.seats.len();
                if residents > 0 && b.beds.is_empty() {
                    bare.push(format!("{} {} at {:?}, {} people", plan.name.0, b.what, b.min, residents));
                }
                for bed in &b.beds {
                    assert!(tagged(at.get(bed), "bed"), "{}'s {} has no bed at {bed}", plan.name.0, b.what);
                }
                let y = b.floor;
                let room: rustc_hash::FxHashSet<IVec2> = b.room.iter().copied().collect();
                let walk = |p: IVec2| room.contains(&p) && solid(at.get(&IVec3::new(p.x, y, p.y))) && !solid(at.get(&IVec3::new(p.x, y + 1, p.y))) && !solid(at.get(&IVec3::new(p.x, y + 2, p.y)));
                // A table on any storey.
                if (b.min.x..=b.max.x).any(|x| (b.min.y..=b.max.y).any(|z| (y + 1..y + 16).any(|up| tagged(at.get(&IVec3::new(x, up, z)), "table")))) {
                    tables += 1;
                }
                // Things set on the tables and cupboards: on a solid piece of furniture, not
                // cloth.
                for x in b.min.x..=b.max.x {
                    for z in b.min.y..=b.max.y {
                        for up in y + 1..y + 16 {
                            let below = at.get(&IVec3::new(x, up, z));
                            if !(tagged(below, "furniture") && solid(below)) {
                                continue;
                            }
                            surfaces += 1;
                            let above = at.get(&IVec3::new(x, up + 1, z));
                            if above.is_some_and(|b| !b.is_air()) && !tagged(above, "cloth") && !tagged(above, "furniture") {
                                tops += 1;
                            }
                        }
                    }
                }
                // On foot from just inside the door.
                let step = b.facing.step();
                let door = b.door.expect("a home's door");
                let inside = IVec2::new(door.x, door.z) - step * 3;
                assert!(walk(inside), "{}'s {} at {:?}: the way in at {inside} is blocked", plan.name.0, b.what, b.min);
                let mut seen = rustc_hash::FxHashSet::default();
                seen.insert(inside);
                let mut queue = vec![inside];
                while let Some(p) = queue.pop() {
                    for s in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
                        if walk(p + s) && seen.insert(p + s) {
                            queue.push(p + s);
                        }
                    }
                }
                let mut fail = None;
                // The stair up, if any: its lowest step (a block up from the floor, the next
                // step beside it a block higher) has a walked-to column beside it.
                let plain = |p: IVec3| solid(at.get(&p)) && !tagged(at.get(&p), "furniture") && !tagged(at.get(&p), "furnishing");
                let sides = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y];
                let lowest: Vec<IVec2> = room.iter().copied().filter(|p| {
                    plain(IVec3::new(p.x, y + 1, p.y)) && !solid(at.get(&IVec3::new(p.x, y + 2, p.y)))
                        && sides.iter().any(|s| plain(IVec3::new(p.x + s.x, y + 2, p.y + s.y)) && !solid(at.get(&IVec3::new(p.x + s.x, y + 3, p.y + s.y))))
                }).collect();
                stairs += usize::from(!lowest.is_empty());
                if !lowest.is_empty() && !lowest.iter().any(|p| sides.iter().any(|s| seen.contains(&(*p + *s)))) {
                    fail = Some(format!("{}'s {} at {:?}: the stair's foot at {lowest:?} can't be walked to", plan.name.0, b.what, b.min));
                }
                for bed in b.beds.iter().filter(|p| p.y == y + 1) {
                    let p = IVec2::new(bed.x, bed.z);
                    if ![IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y].iter().any(|s| seen.contains(&(p + *s))) {
                        fail = Some(format!("{}'s {} at {:?}: the bed at {bed} can't be walked to", plan.name.0, b.what, b.min));
                    }
                }
                for seat in b.seats.iter() {
                    if !seen.contains(&IVec2::new(seat.x, seat.z)) {
                        fail = Some(format!("{}'s {} at {:?}: the seat at {seat} can't be walked to", plan.name.0, b.what, b.min));
                    }
                }
                // A few floor plans of the ground floor, north up (and any that fails).
                if fail.is_some() || (shown < 6 && residents >= 2) {
                    shown += 1;
                    let (lo, hi) = (b.min, b.max);
                    println!("{} ({}), {} for {residents}: {:?}", plan.name.0, plan.culture.family, b.what, plan.people.iter().filter(|r| r.home == i).map(|r| r.species.as_str()).collect::<Vec<_>>());
                    for z in lo.y..=hi.y {
                        let row: String = (lo.x..=hi.x)
                            .map(|x| {
                                let p = IVec2::new(x, z);
                                let b1 = at.get(&IVec3::new(x, y + 1, z));
                                let name = b1.map(|&b| registry.def(b).name.clone()).unwrap_or_default();
                                if p == inside {
                                    'D'
                                } else if tagged(b1, "bed") {
                                    'B'
                                } else if tagged(b1, "table") {
                                    'T'
                                } else if tagged(b1, "seat") {
                                    'c'
                                } else if tagged(b1, "rug") {
                                    'r'
                                } else if tagged(b1, "storage") || tagged(b1, "shelf") {
                                    'S'
                                } else if name.contains("campfire") || name.contains("torch") || tagged(b1, "light") {
                                    '*'
                                } else if room.contains(&p) && walk(p) {
                                    '.'
                                } else if room.contains(&p) {
                                    'x'
                                } else if solid(b1) {
                                    '#'
                                } else {
                                    ' '
                                }
                            })
                            .collect();
                        println!("    {row}");
                    }
                }
                if let Some(fail) = fail {
                    panic!("{fail}");
                }
            }
        }
    }
    println!("{homes} homes: {beds} beds for {wanted} wanted, {tables} tables, {seats} seats, {tops} things set on {surfaces} tables and cupboards, {stairs} stairs; bare: {bare:?}");
    assert!(stairs > 0, "no stairs to walk to");
    assert!(homes >= 20, "only {homes} homes near spawn");
    assert!(beds * 10 >= wanted * 9, "{beds} beds for {wanted} residents");
    assert!(tables * 20 >= homes * 17, "{tables} tables in {homes} homes");
    // Tops' chance is about a half (leaning): a quarter at least of tables and cupboards.
    assert!(tops * 4 >= surfaces && surfaces > 0, "{tops} things set on {surfaces} tables and cupboards");
    assert!(bare.len() * 50 <= homes, "homes with nobody's bed: {bare:?}");
}

/// Loading refuses a trade piece for someone who never lives in a settlement, and a bed
/// that is not a furnishing (which would be anyone's to sleep in).
#[test]
fn interiors_load_checks_refuse_strangers_and_unclaimed_beds() {
    use super::interior::Role;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world = crate::content::WorldContent::load(&content).unwrap();
    let s = &world.settlements;
    let (species, works) = (s.people.species(), s.works());
    assert!(s.interiors.check_who(&species, &works).is_ok());
    let mut stranger = s.interiors.clone();
    stranger.pieces.iter_mut().find(|p| p.role == Role::Trade).unwrap().who = vec!["nobody_at_all".into()];
    assert!(stranger.check_who(&species, &works).is_err(), "a trade piece for nobody was let through");
    let mut bag = s.interiors.clone();
    bag.pieces.iter_mut().find(|p| p.role == Role::Bed).unwrap().block = "base:sleeping_bag".into();
    assert!(bag.check(&content.blocks).is_err(), "a bed that is no furnishing was let through");
}

/// Every village as the world makes it, in full content: no flame of its own stands within
/// reach of its thatch (rules.fire, as a home's fire catches), and its gables are laid in
/// slope pieces whose low edge faces the eave, under a ridge cap.
#[test]
fn village_flames_never_reach_its_thatch() {
    use super::roofs::{Fire, SIDES, SUFFIXES};
    use voxmar_engine::coords::ColumnPos;
    let (content, world_content) = crate::full_content();
    let registry = content.blocks.clone();
    let def = &world_content.settlements;
    let fire = Fire::new(&def.rules.fire, &registry, []);
    let tagged = |b: BlockId, tag: &str| registry.def(b).has_tag(tag);
    let facing = |b: BlockId| -> usize {
        let name = &registry.def(b).name;
        (1..4).find(|&i| name.ends_with(SUFFIXES[i])).unwrap_or(0)
    };
    let before = super::plan::ROOFS_RELAID.load(std::sync::atomic::Ordering::Relaxed);
    let (mut places, mut thatch, mut shingles, mut slopes, mut ridges, mut flames, mut rising) = (0, 0, 0, 0, 0, 0, 0);
    for seed in [42u64, 1] {
        let world = crate::shared_full_world(seed);
        let spawn = world.spawn_point();
        for slot in world.settlements.near(&world, IVec2::new(spawn.x, spawn.z), 2500) {
            let Some(plan) = slot.0.as_ref() else { continue };
            places += 1;
            let mut columns: rustc_hash::FxHashMap<ColumnPos, voxmar_engine::world::Column> = Default::default();
            let mut at = |p: IVec3| -> BlockId {
                let pos = ColumnPos::of_block(p);
                let column = columns.entry(pos).or_insert_with(|| world.generate(pos));
                let o = pos.origin();
                column.block(IVec3::new(p.x - o.x, p.y, p.z - o.z))
            };
            let low = plan.buildings.iter().map(|b| b.floor).min().unwrap_or(crate::SEA_LEVEL) - 6;
            let high = plan.buildings.iter().map(|b| b.floor).max().unwrap_or(crate::SEA_LEVEL) + 28;
            let mut blocks: rustc_hash::FxHashMap<IVec3, BlockId> = Default::default();
            for z in plan.min.y..=plan.max.y {
                for x in plan.min.x..=plan.max.x {
                    for y in low..=high {
                        let p = IVec3::new(x, y, z);
                        let b = at(p);
                        if !b.is_air() {
                            blocks.insert(p, b);
                        }
                    }
                }
            }
            let exposed = fire.exposed(&blocks);
            assert!(exposed.is_empty(), "{}: a flame reaches what catches: {:?}", plan.name.0,
                exposed.iter().take(4).map(|(f, c)| format!("{} at {f} -> {} at {c}", registry.def(blocks[f]).name, registry.def(blocks[c]).name)).collect::<Vec<_>>());
            flames += blocks.values().filter(|&&b| fire.flame(b)).count();
            for (&p, &b) in &blocks {
                if !tagged(b, "roof") || registry.info(b).render != voxmar_engine::block::RenderKind::Model {
                    continue;
                }
                if tagged(b, "thatch") { thatch += 1 } else if tagged(b, "shingle") { shingles += 1 }
                let d = SIDES[facing(b)];
                if tagged(b, "ridge") {
                    ridges += 1;
                    continue;
                }
                if !tagged(b, "shingle") && !tagged(b, "thatch") {
                    continue;
                }
                slopes += 1;
                // Behind its high edge the roof goes on up (or meets the other slope at
                // the peak): a slope turned the wrong way has the roof going down there.
                let back = p - IVec3::new(d.x, 0, d.y);
                let up = blocks.get(&(back + IVec3::Y)).is_some_and(|&n| tagged(n, "roof"));
                let peak = blocks.get(&back).is_some_and(|&n| tagged(n, "roof") && !tagged(n, "ridge") && facing(n) == (facing(b) + 2) % 4);
                let ridge = blocks.get(&back).is_some_and(|&n| tagged(n, "ridge"));
                if up || peak || ridge {
                    rising += 1;
                }
            }
        }
    }
    let relaid = super::plan::ROOFS_RELAID.load(std::sync::atomic::Ordering::Relaxed) - before;
    eprintln!("{places} places: {thatch} thatch and {shingles} shingle pieces ({slopes} slopes, {rising} rising behind, {ridges} ridges), {flames} flames; {relaid} pieces relaid in shingles");
    assert!(places >= 4 && thatch > 100 && shingles > 100 && ridges > 10, "too few shaped roofs to judge");
    // Chimneys and dormers break through a few.
    assert!(rising * 100 >= slopes * 95, "only {rising} of {slopes} slopes have the roof rising behind them");
}

/// Loading refuses roof pieces that are not whole: a piece missing a facing or in some
/// wood of its style, a slope or ridge not tagged so, a fire `instead` that catches, and a
/// share or reach out of range.
#[test]
fn roof_pieces_and_fire_are_load_checked() {
    use super::def::RoofPiecesDef;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let base = crate::content::WorldContent::load(&content).unwrap().settlements;
    let refused = |change: &dyn Fn(&mut super::SettlementsDef)| {
        let mut def = base.clone();
        change(&mut def);
        def.check(&content.blocks).is_err() || def.check_furnishings(&content.blocks).is_err()
    };
    let piece = |slope: &str, ridge: &str| RoofPiecesDef { slope: format!("base:{slope}"), ridge: format!("base:{ridge}"), weight: 1.0 };
    let timber = |d: &mut super::SettlementsDef| d.styles.iter().position(|s| s.name == "timber_frame").unwrap();
    assert!(!refused(&|_| {}));
    assert!(base.styles.iter().any(|s| !s.roof_pieces.is_empty()) && base.rules.fire.instead.is_some());
    // Spruce makes no shingles, and timber-frame villages may be built of spruce.
    assert!(refused(&|d| { let i = timber(d); d.styles[i].roof_pieces.push(piece("{wood}_shingles", "{wood}_shingle_ridge")) }));
    assert!(refused(&|d| { let i = timber(d); d.styles[i].roof_pieces.push(piece("oak_planks", "thatch_ridge")) }), "a whole block faces no way");
    assert!(refused(&|d| { let i = timber(d); d.styles[i].roof_pieces.push(piece("thatch_ridge", "thatch_roof")) }), "the slope and ridge swapped");
    assert!(refused(&|d| d.rules.fire.instead = Some(piece("thatch_roof", "thatch_ridge"))), "the fire's roof catches itself");
    assert!(refused(&|d| { let i = timber(d); d.styles[i].pieced = 1.5 }));
    assert!(refused(&|d| d.rules.fire.reach = 5));
    use super::def::JettyDef;
    let jetty = |beam: &str, chance: f32| Some(JettyDef { beam: format!("base:{beam}"), chance });
    assert!(base.styles.iter().any(|s| s.jetty.is_some()));
    assert!(refused(&|d| { let i = timber(d); d.styles[i].jetty = jetty("dark_oak_jetty_beam", 1.5) }));
    assert!(refused(&|d| { let i = timber(d); d.styles[i].jetty = jetty("dark_oak_beam", 0.5) }), "a beam not tagged jetty");
    assert!(refused(&|d| { let i = timber(d); d.styles[i].jetty = jetty("no_such_jetty_beam", 0.5) }));
}

/// Trails come into a walled place through its gates: none runs over a column of its wall.
#[test]
fn trails_never_cross_a_wall() {
    let (mut places, mut trails) = (0, 0);
    let mut over = Vec::new();
    for seed in [0x5eedu64, 7] {
        let world = shared_world(seed);
        let spawn = world.spawn_point();
        for s in world.settlements_near(IVec2::new(spawn.x, spawn.z), 9000).into_iter().filter(|s| !s.walls.is_empty()) {
            places += 1;
            let walls: rustc_hash::FxHashSet<IVec2> = s.walls.iter().map(|w| w.0).collect();
            let (lo, hi) = (walls.iter().fold(IVec2::MAX, |a, &b| a.min(b)), walls.iter().fold(IVec2::MIN, |a, &b| a.max(b)));
            let (lines, _) = world.trails_in(lo - 8, hi + 8);
            trails += lines.len();
            for &(p, _) in lines.iter().flatten() {
                if walls.contains(&p.round().as_ivec2()) {
                    let c = p.round().as_ivec2();
                    let gate = s.gates.iter().map(|g| (*g - c).abs().max_element()).min().unwrap_or(-1);
                    over.push(format!("{} at {c}, {gate} from a gate", s.name.0));
                }
            }
        }
    }
    assert!(places >= 6 && trails >= 20, "only {places} walled places and {trails} trails by them to judge");
    assert!(over.is_empty(), "{} trail points on a wall: {:?}", over.len(), &over);
}

/// Jettied houses in full content: each jetty beam stands out from the wall below it (its
/// brace against that wall), under the overhanging floor, with that storey's wall a block
/// further over the street than the one below.
#[test]
fn jettied_storeys_stand_out_on_their_beams() {
    use super::roofs::{SIDES, SUFFIXES};
    use voxmar_engine::coords::ColumnPos;
    let registry = crate::full_content().0.blocks.clone();
    let solid = |b: BlockId| registry.info(b).solid;
    let (mut places, mut beams, mut wrong) = (0, 0, Vec::new());
    for seed in [42u64, 7] {
        let world = crate::shared_full_world(seed);
        let spawn = world.spawn_point();
        for slot in world.settlements.near(&world, IVec2::new(spawn.x, spawn.z), 2500) {
            let Some(plan) = slot.0.as_ref() else { continue };
            places += 1;
            let mut columns: rustc_hash::FxHashMap<ColumnPos, voxmar_engine::world::Column> = Default::default();
            let mut at = |p: IVec3| -> BlockId {
                let pos = ColumnPos::of_block(p);
                let column = columns.entry(pos).or_insert_with(|| world.generate(pos));
                let o = pos.origin();
                column.block(IVec3::new(p.x - o.x, p.y, p.z - o.z))
            };
            let low = plan.buildings.iter().map(|b| b.floor).min().unwrap_or(crate::SEA_LEVEL) + 1;
            let high = plan.buildings.iter().map(|b| b.floor).max().unwrap_or(crate::SEA_LEVEL) + 12;
            for z in plan.min.y..=plan.max.y {
                for x in plan.min.x..=plan.max.x {
                    for y in low..=high {
                        let p = IVec3::new(x, y, z);
                        let b = at(p);
                        if !registry.def(b).has_tag("jetty") {
                            continue;
                        }
                        beams += 1;
                        let name = &registry.def(b).name;
                        let d = SIDES[(1..4).find(|&i| name.ends_with(SUFFIXES[i])).unwrap_or(0)];
                        let out = IVec3::new(d.x, 0, d.y);
                        // Behind it the wall below; over it the floor, and over that the
                        // storey's wall, standing over the street where the floor below ends.
                        let ok = solid(at(p - out)) && solid(at(p + IVec3::Y)) && solid(at(p - out + IVec3::Y)) && !solid(at(p + out));
                        if !ok && wrong.len() < 6 {
                            wrong.push(format!("{} at {p} ({name})", plan.name.0));
                        }
                    }
                }
            }
        }
    }
    eprintln!("{places} places, {beams} jetty beams");
    assert!(beams >= 10, "only {beams} jetty beams in {places} places");
    assert!(wrong.is_empty(), "jetty beams not under a jetty: {wrong:?}");
}

/// `/locate settlement` finds what a look at every square in reach would find nearest,
/// planning only the squares nearer than the answer: every square within thousands of
/// blocks planned at once held a frame (and the server's tick) for seconds.
#[test]
fn the_nearest_settlement_is_found_nearest_first() {
    let radius = 4000;
    let every = shared_world(0x5eed);
    let spawn = every.spawn_point();
    for (from, size) in [(IVec2::new(spawn.x, spawn.z), None), (IVec2::new(spawn.x, spawn.z), Some("town")), (IVec2::new(spawn.x + 2500, spawn.z - 1800), Some("village"))] {
        let away = |s: &crate::SettlementInfo| (s.centre.xz() - from).as_i64vec2().length_squared();
        let wanted = every
            .settlements_near(from, radius)
            .iter()
            .filter(|s| away(s) <= i64::from(radius).pow(2) && size.is_none_or(|size| s.size == size))
            .min_by_key(|s| (away(s), s.centre.x, s.centre.z))
            .map(|s| s.id);
        let fresh = base_world(0x5eed);
        let found = fresh.nearest_settlement(from, radius, |s| size.is_none_or(|size| s.size == size));
        assert_eq!(found.as_ref().map(|s| s.id), wanted, "from {from} for {size:?}: {:?}", found.map(|s| (s.name.0, s.centre)));
        // Planned: the squares whose nearest point is no farther than the answer (all in
        // reach where there is none), besides those any world plans first for its spawn,
        // and the rest of the last few planned side by side.
        let spacing = fresh.settlements.def.grid.spacing;
        let answer = found.as_ref().map_or(i64::from(radius).pow(2), away);
        let (lo, hi) = (fresh.settlements.cell_of(from.x - radius, from.y - radius), fresh.settlements.cell_of(from.x + radius, from.y + radius));
        let squares: Vec<IVec2> = (lo.y..=hi.y).flat_map(|z| (lo.x..=hi.x).map(move |x| IVec2::new(x, z))).collect();
        let nearer = squares.iter().filter(|&&c| (from.clamp(c * spacing, (c + 1) * spacing - 1) - from).as_i64vec2().length_squared() <= answer).count();
        let promised = fresh.settlements.promised(&fresh).iter().filter(|c| !squares.contains(c)).count();
        let planned = fresh.settlements.made();
        let side_by_side = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8) - 1;
        assert!(planned <= nearer + promised + side_by_side, "from {from} for {size:?}: {planned} squares planned, only {nearer} as near as the answer (+{promised} for the spawn, +{side_by_side} side by side)");
        eprintln!("from {from} for {size:?}: {planned} of {} squares in reach planned", squares.len());
    }
}

/// A block belongs to the settlement of its own square or none (a settlement never
/// reaches past its square), so a bed's settlement is found by planning that square
/// alone, however far the bed stands from the settlement's middle.
#[test]
fn a_bed_belongs_to_the_settlement_of_its_square() {
    let world = shared_world(0x5eed);
    let spawn = world.spawn_point();
    let town = world
        .settlements_near(IVec2::new(spawn.x, spawn.z), 6000)
        .into_iter()
        .filter(|s| s.size == "town")
        .max_by_key(|s| s.buildings.len())
        .expect("a town");
    let (far, bed) = town
        .buildings
        .iter()
        .flat_map(|b| b.beds.iter().map(|&p| ((p.xz() - town.centre.xz()).as_vec2().length(), p)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("a bed in the town");
    assert!(far > 64.0, "{}'s farthest bed is only {far:.0} blocks from its middle", town.name.0);
    let fresh = base_world(0x5eed);
    let holding = fresh.settlement_holding(bed.xz()).expect("the bed's square settled");
    assert_eq!(holding.id, town.id, "the bed at {bed} ({far:.0} blocks out) is {}'s, not {}'s", town.name.0, holding.name.0);
    assert!(holding.buildings.iter().any(|b| b.beds.contains(&bed)));
}

/// A jettied house of three storeys keeps a floor along its old front line on every storey
/// over the ground, and a sign board at the top of walls of three is not laid over by the
/// row of beams under the overhang.
#[test]
fn jettied_houses_keep_their_floors_and_their_sign() {
    use super::def::{Extra, Form, Roof};
    use super::house::{self, Kind, Mats, Spec};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    let registry = content.blocks.clone();
    let def = &world_content.settlements;
    let fixed = super::fixed(&registry, def).unwrap();
    let id = |n: &str| registry.id(&format!("base:{n}")).unwrap();
    let palette = |n: &str| super::canvas::Palette(vec![(id(n), 1.0)]);
    let (floor, beam) = (id("spruce_planks"), id("stone_bricks"));
    let mats = Mats {
        foundation: palette("cobblestone"),
        floor,
        wall: id("polished_calcite"),
        frame: id("dark_oak_log"),
        roof: id("dark_oak_planks"),
        trim: id("spruce_planks"),
        window: Some(id("old_glass")),
        weathered: palette("mossy_cobblestone"),
        hedge: palette("oak_leaves"),
        log: id("oak_log"),
        chimney: id("cobbled_granite"),
        crops: Vec::new(),
        fruit: palette("apple_leaves_2"), roof_pieces: Vec::new(), pieced: 0.0, jetty: Some(([beam; 4], 1.0)),
    };
    let (w, d, wall, storeys) = (7, 7, 3, 3);
    let mut drawn = 0;
    for seed in 0..12u64 {
        let spec = Spec {
            kind: Kind::House, w, d, wall, storeys, roof: Roof::Gable, pitch: 1, ridge_x: true, door: w / 2, shed: 0, wing: None, stilts: 0,
            extras: vec![Extra::SignBoard], regular: true, window_chance: 0.0, form: Form::Box, inside: Vec::new(), centre: None, seed,
        };
        let (low, high) = spec.envelope();
        let frame = super::canvas::Frame::fit(low, high, super::canvas::Facing::North, low, 0);
        let mut canvas = super::canvas::Canvas::default();
        house::draw(&spec, &frame, &mats, &fixed, &mut canvas);
        let set = |x: i32, y: i32, z: i32| match canvas.absolute.get(&frame.at(x, y, z)) {
            Some(super::canvas::Put::Set(b)) => Some(*b),
            _ => None,
        };
        // Jettied: the beams under the first storey's front.
        if !(0..w).any(|x| set(x, wall, -1) == Some(beam)) {
            continue;
        }
        drawn += 1;
        for storey in 1..storeys {
            for x in 1..w - 1 {
                assert!(set(x, storey * (wall + 1), 0).is_some_and(|b| registry.info(b).solid), "seed {seed}: no floor at {x} on storey {storey} where the front wall stood");
            }
        }
        let sign = if w / 2 + 1 < w - 1 { w / 2 + 1 } else { w / 2 - 1 };
        assert!(set(sign, 3, -1).is_some_and(|b| b != beam), "seed {seed}: the sign board by the door was laid over by a beam");
    }
    assert!(drawn > 0, "no house of the twelve was jettied");
}
