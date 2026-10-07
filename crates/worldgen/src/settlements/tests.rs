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
                            // A ripe crop is `<crop>_crop`, a ripe fruit tree `<fruit>_leaves`.
                            let (crop, ripe) = match bare.rsplit_once("_crop") {
                                Some((crop, stage)) => (crop, stage.is_empty()),
                                None => match bare.strip_suffix("_leaves") {
                                    Some(fruit) => (fruit, true),
                                    None => continue,
                                },
                            };
                            let Some(&(need, rarity)) = def.crops.get(&format!("base:{crop}")) else { continue };
                            // Sown crops are the fields'; wild ones (plants.ron) may stand ripe by a house.
                            if bare.contains("_crop") && b.kind.name() != "field" {
                                continue;
                            }
                            planted += 1;
                            if ripe {
                                failures.push(format!("seed {seed} {}: ripe {bare} in the {} at {x} {y} {z}", s.name.0, b.what));
                            } else if bare.contains("_crop") && (need > most || rarity > voxmar_engine::block::Rarity::Uncommon) {
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
/// `target/houses_{front,back}_ROW_N.ppm`: for judging the drawing of each piece.
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
    let fixed = super::fixed(&registry, &def.furnishings).unwrap();
    let id = |n: &str| registry.id(&format!("base:{n}")).unwrap();
    let palette = |n: &str| super::canvas::Palette(vec![(id(n), 1.0)]);
    let mats = Mats {
        foundation: palette("cobblestone"),
        floor: id("spruce_planks"),
        wall: id("polished_calcite"),
        frame: id("dark_oak_log"),
        roof: id("dark_oak_planks"),
        trim: id("spruce_planks"),
        window: Some(id("glass")),
        weathered: palette("mossy_cobblestone"),
        hedge: palette("oak_leaves"),
        log: id("oak_log"),
        crops: Vec::new(),
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
    // the server's default seed has trails laid at spawn, 0 has none.
    for (seed, streams, grids, squares) in [(0x5eed, 36, 60, 25), (0, 8, 0, 12)] {
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
