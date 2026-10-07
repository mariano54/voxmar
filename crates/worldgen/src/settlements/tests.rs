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
/// south-east, `a` pixels a block, as `forest_preview` draws.
#[allow(clippy::too_many_arguments)]
fn isometric(area: &Area, registry: &BlockRegistry, min: IVec2, max: IVec2, low: i32, high: i32, a: f32, path: &str) {
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
                if b.is_air() || ![IVec3::X, IVec3::Y, IVec3::Z].iter().any(|&d| open(area.block(p + d))) {
                    continue;
                }
                let base = area.color(registry, p);
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
            let kinds: Vec<&str> = p.buildings.iter().map(|b| b.kind.name()).collect();
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
        let found = world.settlements_near(IVec2::new(spawn.x, spawn.z), 2000);
        assert!(!found.is_empty(), "seed {seed}: no settlement within 2000 blocks of spawn");
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
