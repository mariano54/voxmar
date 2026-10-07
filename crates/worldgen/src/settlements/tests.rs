use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, Tint};
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3};
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
        match def.tint {
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
    // `ONLY=Kinfell,villa` draws only the settlements with those names or styles.
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
            let l = p.levers.0;
            println!(
                "  {} / {}  {} {} {} in {}  at {}, {}, {}  {} buildings  wealth {:.2} age {:.2} order {:.2} crowding {:.2} green {:.2}",
                p.name.0, p.name.1, p.size, p.layout, p.style, world.biome_at(p.centre.x, p.centre.y).name, p.centre.x, p.height, p.centre.y, p.buildings.len(), l[0], l[1], l[2], l[3], l[4]
            );
            let named = names.as_ref().is_some_and(|n| n.iter().any(|n| p.name.0.contains(n.as_str()) || p.style == *n));
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
    let source = include_str!("../../../../assets/content/base/blocks.ron");
    std::sync::Arc::new(BlockRegistry::from_ron("base", source).unwrap())
}

fn base_world(seed: u64) -> Overworld {
    Overworld::new(seed, base_registry())
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
        assert_eq!((x.id, &x.name, &x.buildings), (y.id, &y.name, &y.buildings));
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
        let world = Overworld::new(seed, registry.clone());
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
        let world = Overworld::new(seed, registry.clone());
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
        match def.tint {
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
        fruit: palette("apple_leaves_2"),
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
            fruit: palette("apple_leaves_2"),
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
/// counts the settlements over 24 by 24 squares of three worlds: how many squares hold
/// one, hamlets to villages, and the layouts and styles drawn.
#[test]
#[ignore]
fn settlement_census() {
    for seed in [1u64, 42, 7] {
        let world = base_world(seed);
        let spacing = world.settlements.def.grid.spacing;
        let (mut squares, mut hamlets, mut villages) = (0, 0, 0);
        let mut layouts: std::collections::BTreeMap<String, u32> = Default::default();
        let mut buildings: std::collections::BTreeMap<String, u32> = Default::default();
        let start = std::time::Instant::now();
        for cz in -12..12 {
            for cx in -12..12 {
                squares += 1;
                let slot = world.settlements.slot(&world, IVec2::new(cx, cz));
                let Some(plan) = &slot.0 else { continue };
                if plan.size == "hamlet" { hamlets += 1 } else { villages += 1 }
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
        println!("seed {seed}: {} of {squares} squares settled ({hamlets} hamlets, {villages} villages) in {:?}, {spacing}", hamlets + villages, start.elapsed());
        println!("  layouts {layouts:?}");
        println!("  buildings {buildings:?}");
    }
}

/// How much of the world a new player waits for: finding spawn and making its column
/// plans the settlements and lays the trails round it, and the river regions under both.
/// Trails near settlements read far (each tile the grids of the regions three around),
/// so this is most of the time a fresh server takes to answer; the budgets are what it
/// needs now, and a change needing more must say why here. Counts, not times, so a busy
/// machine cannot fail it.
#[test]
fn spawn_needs_only_so_much_planning() {
    use crate::Generator;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = voxmar_engine::content::Content::load(&dir).unwrap();
    let world_content = crate::content::WorldContent::load(&content).unwrap();
    // Seed, then the most stream-tier river regions, trail grids and settlement squares:
    // the server's default seed has trails laid at spawn, 0 has none. Seed 0 plans the
    // squares in reach of spawn first, for the settlement promised there (rules,
    // `spawn_settlement_within`), which reads one river region more. Since names draw
    // from a stream of their own (`names_apart`), every plan draws its layout afresh, and
    // the default seed's trails reach three squares more.
    for (seed, streams, grids, squares) in [(0x5eed, 36, 60, 28), (0, 9, 0, 12)] {
        let world = Overworld::with_content(seed, content.blocks.clone(), &world_content);
        world.generate(ColumnPos::of_block(world.spawn_point()));
        let made = (world.rivers.made().1, world.trails.made().0, world.settlements.made());
        assert!(made.0 <= streams && made.1 <= grids && made.2 <= squares, "seed {seed}: {made:?} river regions, trail grids and squares, over {:?}", (streams, grids, squares));
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
    let world = base_world(1);
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
/// chapel with its altar (in snowy land too, where the snow blanket once covered it).
#[test]
fn plans_keep_to_their_squares_and_rules() {
    use crate::trails::Land;
    use super::house::Kind;
    let (mut squares, mut hamlets, mut villages, mut chapels) = (0, 0, 0, 0);
    for seed in [1u64, 42] {
        let world = base_world(seed);
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
                    if a.what == "chapel" && chapels < 3 {
                        chapels += 1;
                        let area = Area::new(&world, a.min, a.max);
                        let found = (a.min.y..=a.max.y).any(|z| (a.min.x..=a.max.x).any(|x| (a.floor..a.floor + 4).any(|y| area.block(IVec3::new(x, y, z)) == altar)));
                        assert!(found, "{at}: the chapel at {} has no altar", a.min);
                    }
                }
            }
        }
    }
    let settled = hamlets + villages;
    let share = settled as f32 / squares as f32;
    assert!((0.08..=0.25).contains(&share), "{settled} of {squares} squares settled");
    assert!(hamlets > villages && villages > 0, "{hamlets} hamlets, {villages} villages");
    assert!(chapels > 0, "no chapel to look in");
}

/// The settlements of a few squares, held fixed: a change to planning that moves a single
/// building shows here, to be looked at in shots and the hash then renewed.
#[test]
fn settlements_are_unchanged() {
    const EXPECTED: u64 = 7909794199593065950;
    let world = base_world(1);
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
        let world = base_world(seed);
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
        fruit: palette("apple_leaves_2"),
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
    let cases: [(&str, Change); 17] = [
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
    for seed in [1u64, 42, 7] {
        let world = Overworld::new(seed, registry.clone());
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
                        // Snow is land only on land: the snow blanket also caps a prop's
                        // stone (climate.ron, `blanket`).
                        let snow = |y: i32| registry.def(col.block(IVec3::new(x & 31, y, z & 31))).has_tag("snow");
                        let Some(top) = (b.floor - 12..=b.floor + 8).rev().find(|&y| land(y) && (!snow(y) || land(y - 1))) else { continue };
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

/// Every storey but the top of every house of more than one has a stair up, round the
/// walls clear of the door, a shop's counter and the chimney, whatever its size; an inn's
/// stairs keep clear of both columns of its wide door.
#[test]
fn every_storey_has_a_stair_up() {
    use super::def::{Extra, Form, Roof};
    use super::house::{self, Kind, Spec};
    let world = base_world(1);
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
                    house::make_room_for_stairs(&mut spec);
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
        let world = base_world(seed);
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
        let world = Overworld::new(seed, registry.clone());
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
        hedge: palette("oak_leaves"), log: id("oak_log"), chimney: id("cobblestone"), crops: Vec::new(), fruit: palette("apple_leaves_2"),
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
    for &seed in &seeds {
        let world = base_world(seed);
        let within = world.settlements.def.rules.spawn_settlement_within as f32;
        let spawn = world.spawn_point();
        let at = IVec2::new(spawn.x, spawn.z);
        assert!(!world.settlements.covers(&world, spawn.x, spawn.z), "seed {seed}: spawn at {spawn} is inside a settlement");
        let nearest = world.settlements_near(at, 2048).iter().map(|s| IVec2::new(s.centre.x, s.centre.z).as_vec2().distance(at.as_vec2())).fold(f32::MAX, f32::min);
        if nearest <= within {
            continue;
        }
        let sites = world.settlements.spawn_has_sites(&world);
        eprintln!("seed {seed}: none within {within} of spawn at {spawn}; spots in reach: {sites}");
        if sites {
            far.push(format!("seed {seed}: the nearest settlement is {nearest:.0} blocks from spawn at {spawn}, though the land has spots for one"));
        } else {
            excused.push(seed);
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
    'seeds: for seed in 1u64..=60 {
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
            if villages >= 2 {
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
        let world = base_world(seed);
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
        let world = base_world(seed);
        let def = &world.settlements.def;
        let apart = def.rules.names_apart as f32;
        let bare = |name: &str| {
            let prefix = def.names.iter().flat_map(|n| &n.prefixes).find(|p| name.starts_with(&format!("{} ", p.0)));
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
    let world = base_world(1);
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
                let list = (h % def.names.len() as u64) as usize;
                let small = (h >> 32) % every == 0 && !def.names[list].small.is_empty();
                let name = super::plan::unique_name(def, world.settlements.seed, &promised, cell, list, small).0;
                named.push((cell, list, name));
            }
        }
        let gap = |a: IVec2, b: IVec2| ((a - b).abs() * grid.spacing - (grid.spacing - 2 * grid.margin)).max(IVec2::ZERO).as_vec2().length();
        let mut clashes = Vec::new();
        for (i, a) in named.iter().enumerate() {
            for b in &named[i + 1..] {
                if a.2 == b.2 && gap(a.0, b.0) <= apart {
                    clashes.push(format!("{} in squares {} and {}", a.2, a.0, b.0));
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
    let world = base_world(1);
    let mut def = world.settlements.def.clone();
    let mut list = def.names[0].clone();
    list.first = vec![("Ash".to_owned(), "アッシュ".to_owned())];
    list.last = (0..30).map(|i| (format!("b{i}"), format!("ブ{i}"))).collect();
    list.small = Vec::new();
    def.names = vec![list];
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
