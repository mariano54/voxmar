//! What lies by every world's spawn (`spawn_kit.ron`): enough for a new player's first flint
//! tools however poor the land, since bare hands break neither stone nor a living tree. Laid
//! once per world (`WorldMeta::spawn_kit`), on new worlds and old saves alike, through the
//! normal block path but not marked placed, so it counts as wild (loose flint regrows); only
//! into air on natural ground no player placed.

use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::content::ContentFile;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::creature::mix;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::VoxelWorld;

/// How far above a spot the kit looks for a block a player placed: a roof, a floor above, a
/// wall top. Higher than any room, low enough to stay a few lookups a column.
const ROOF: i32 = 24;

pub const FILE: &str = "spawn_kit.ron";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KitFile {
    /// How far from the spawn, in blocks across the ground, the pieces lie.
    near: (f32, f32),
    pieces: Vec<PieceDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PieceDef {
    block: String,
    /// How many, at least and at most (drawn from the world's seed).
    count: (u8, u8),
    /// Blocks stacked at each (a snag's trunk).
    #[serde(default = "one")]
    height: u8,
}

fn one() -> u8 {
    1
}

#[derive(Clone, Debug, Default)]
pub struct SpawnKit {
    near: (f32, f32),
    pieces: Vec<(BlockId, u8, u8, u8)>,
}

impl SpawnKit {
    /// The kit from every package's `spawn_kit.ron`, a later package's replacing an earlier's.
    pub fn load(files: &[ContentFile], registry: &BlockRegistry) -> Result<Self, String> {
        let mut kit = SpawnKit::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let at = |e: &dyn std::fmt::Display| format!("{}/{FILE}: {e}", file.package);
            let def: KitFile = ron::from_str(&file.source).map_err(|e| at(&e))?;
            let (lo, hi) = def.near;
            if !(lo >= 1.0 && lo <= hi && hi <= 12.0) {
                return Err(at(&format!("near ({lo}, {hi}) must be in order, from 1 to 12 blocks")));
            }
            let mut pieces = Vec::new();
            for piece in def.pieces {
                let name = if piece.block.contains(':') { piece.block.clone() } else { format!("{}:{}", file.package, piece.block) };
                let block = registry.id(&name).ok_or_else(|| at(&format!("unknown block `{name}`")))?;
                let (least, most) = piece.count;
                if !(1..=8).contains(&least) || least > most || most > 8 || !(1..=4).contains(&piece.height) {
                    return Err(at(&format!("`{name}`: count ({least}, {most}) must be in order from 1 to 8, height from 1 to 4")));
                }
                pieces.push((block, least, most, piece.height));
            }
            kit = SpawnKit { near: def.near, pieces };
        }
        Ok(kit)
    }

    /// The fewest of `block` the kit lays (0 if it lays none): what every spawn is sure of.
    pub fn least(&self, block: BlockId) -> u8 {
        self.pieces.iter().filter(|p| p.0 == block).map(|p| p.1).sum()
    }

    /// The columns the kit may lie in by `spawn`: those within its far edge (`near`), to be
    /// loaded before `plan`.
    pub fn columns(&self, spawn: Vec3) -> Vec<ColumnPos> {
        let reach = self.near.1.ceil() as i32;
        let feet = spawn.floor().as_ivec3();
        let mut columns: Vec<ColumnPos> = [(-reach, -reach), (reach, -reach), (-reach, reach), (reach, reach)].iter().map(|&(dx, dz)| ColumnPos::of_block(feet + IVec3::new(dx, 0, dz))).collect();
        columns.sort_unstable_by_key(|c| c.origin().to_array());
        columns.dedup();
        columns
    }

    /// Where the kit goes by `spawn` (the player's feet) in `world`, whose `columns` must be
    /// loaded: each piece's cells, from the world's `seed`. A spot is the first solid block
    /// down from a little over the feet, natural and opaque, that no player `touched` (placed,
    /// or changed from what the land was made with), with nothing touched above it either (an
    /// older save's house on unbuilt ground keeps its rooms clear), and room for
    /// the piece's own height (air, or meadow grass it lies over); the band `near` first, in an
    /// order drawn from the seed, then nearest first out to the columns' edges, so a small
    /// islet still takes it. Each piece takes the next spots tall enough for it; pieces that
    /// find none are left out.
    pub fn plan(&self, world: &VoxelWorld, registry: &BlockRegistry, spawn: Vec3, seed: u64, touched: impl Fn(IVec3) -> bool) -> Vec<(IVec3, BlockId)> {
        let feet = spawn.floor().as_ivec3();
        let tallest = self.pieces.iter().map(|p| p.3).max().unwrap_or(1) as i32;
        let def = |p: IVec3| world.block(p).map(|b| (b, registry.def(b), registry.info(b)));
        // Room for a piece: air, or a low meadow plant (never one two high).
        let room = |p: IVec3| def(p).is_some_and(|(b, d, i)| b.is_air() || !i.solid && d.has_tag("grass") && !d.has_tag("tall") && !touched(p));
        let spot = |x: i32, z: i32| -> Option<(IVec3, u8)> {
            // Down from a little above the feet to the first solid block, past a tree's own
            // leaves (a spawn may stand on a crown) but never past anything built: a roof or
            // a floor is the end of the search, not a spot.
            let top = (feet.y - 32..=feet.y + 8)
                .rev()
                .map(|y| IVec3::new(x, y, z))
                .find(|&p| def(p).is_some_and(|(_, d, i)| i.solid && !(d.has_tag("leaves") && !touched(p))))?;
            let (_, ground, info) = def(top)?;
            // Ground, not a stump, a fallen trunk or a cactus top.
            let firm = info.render == RenderKind::Opaque && ground.has_tag("natural") && !["log", "plant"].iter().any(|t| ground.has_tag(t)) && !touched(top);
            let built_over = (1..=ROOF).any(|dy| touched(top + IVec3::Y * dy));
            let free = (1..=tallest).take_while(|&dy| room(top + IVec3::Y * dy)).count() as u8;
            (firm && !built_over && free > 0).then_some((top + IVec3::Y, free))
        };
        let (lo, hi) = self.near;
        let mut spots: Vec<(bool, u64, IVec3, u8)> = self
            .columns(spawn)
            .into_iter()
            .filter(|c| world.contains(*c))
            .flat_map(|c| {
                let origin = c.origin();
                (0..CHUNK_SIZE).flat_map(move |dz| (0..CHUNK_SIZE).map(move |dx| (origin.x + dx, origin.z + dz)))
            })
            .filter_map(|(x, z)| {
                let d = Vec3::new(x as f32 + 0.5 - spawn.x, 0.0, z as f32 + 0.5 - spawn.z).length();
                // Never on the spawn spot itself or right beside it.
                if d < lo {
                    return None;
                }
                let (at, free) = spot(x, z)?;
                let order = if d > hi { (d * 16.0) as u64 } else { mix(seed ^ 0x5b17, (x as u64) << 32 ^ z as u32 as u64) };
                Some((d > hi, order, at, free))
            })
            .collect();
        spots.sort_unstable_by_key(|&(far, order, at, _)| (far, order, at.x, at.z));
        let mut spots: Vec<(IVec3, u8)> = spots.into_iter().map(|(_, _, at, free)| (at, free)).collect();
        let mut laid = Vec::new();
        for (i, &(block, least, most, height)) in self.pieces.iter().enumerate() {
            let count = least + (mix(seed ^ 0x5b18, i as u64) % u64::from(most - least + 1)) as u8;
            for _ in 0..count {
                let Some(next) = spots.iter().position(|&(_, free)| free >= height) else { break };
                let (at, _) = spots.remove(next);
                laid.extend((0..height as i32).map(|dy| (at + IVec3::Y * dy, block)));
            }
        }
        laid
    }

    /// The pieces `laid` (from `plan`) holds fewer of than their least count: name, laid and
    /// least, for a warning.
    pub fn short(&self, registry: &BlockRegistry, laid: &[(IVec3, BlockId)]) -> Vec<(String, usize, u8)> {
        self.pieces
            .iter()
            .filter_map(|&(block, least, _, height)| {
                let cells = laid.iter().filter(|&&(_, b)| b == block).count() / height as usize;
                (cells < least as usize).then(|| (registry.def(block).name.clone(), cells, least))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    use std::sync::Arc;
    use voxmar_worldgen::Generator;

    /// The seeds of `seeds` whose spawn would not get enough flint for two tools and a stick
    /// source (three snag blocks) within six blocks.
    fn short_of_the_kit(seeds: Vec<u64>) -> Vec<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_engine::content::Content::load(&dir).unwrap();
        let world_content = voxmar_worldgen::WorldContent::load(&content).unwrap();
        let blocks = &content.blocks;
        let kit = SpawnKit::load(&content.files, blocks).unwrap();
        let (flint, snag) = (blocks.expect("base:loose_flint"), blocks.expect("base:dead_log"));
        seeds
            .into_par_iter()
            .filter_map(|seed| {
                let overworld = voxmar_worldgen::Overworld::shared(seed, &content, &world_content);
                let start = overworld.spawn_point();
                let mut world = VoxelWorld::default();
                world.insert(ColumnPos::of_block(start), Arc::new(overworld.generate(ColumnPos::of_block(start))));
                let spawn = crate::find_ground(&world, blocks, start);
                for at in kit.columns(spawn) {
                    if world.contains(at) {
                        continue;
                    }
                    world.insert(at, Arc::new(overworld.generate(at)));
                }
                let laid = kit.plan(&world, blocks, spawn, seed, |_| false);
                let on: Vec<String> = laid
                    .iter()
                    .filter(|&&(p, _)| !laid.iter().any(|&(q, _)| q == p - IVec3::Y))
                    .filter_map(|&(p, _)| world.block(p - IVec3::Y).map(|b| blocks.def(b)))
                    .filter(|d| ["log", "leaves", "plant"].iter().any(|t| d.has_tag(t)))
                    .map(|d| d.name.clone())
                    .collect();
                if !on.is_empty() {
                    return Some(format!("seed {seed} at {spawn}: a piece stands on {on:?}"));
                }
                let within = |block: BlockId| laid.iter().filter(|&&(p, b)| b == block && Vec3::new(p.x as f32 + 0.5 - spawn.x, 0.0, p.z as f32 + 0.5 - spawn.z).length() <= 6.0).count();
                let (flints, snags) = (within(flint), within(snag));
                (flints < 4 || snags < 3).then(|| format!("seed {seed} at {spawn}: {flints} flint, {snags} snag blocks within 6"))
            })
            .collect()
    }

    /// However poor the land the spawn search lands on, the kit lays enough flint for two
    /// tools and a stick source within reach of the spawn: spawns that once had none (seed
    /// 90 an islet in the sea, 116 by a column's corner, 157 on a tree's crown).
    #[test]
    fn hard_spawns_get_flint_and_sticks() {
        let short = short_of_the_kit(vec![90, 116, 157]);
        assert!(short.is_empty(), "{short:?}");
    }

    /// Every one of the first 64 worlds' spawns gets the kit (all of the first 200 did, once,
    /// when it was written), four seeds a test: a spawn search takes about a second of one
    /// core, so the shards run side by side, each in the budget.
    macro_rules! every_spawn {
        ($($name:ident: $first:literal),* $(,)?) => {$(
            #[test]
            fn $name() {
                let short = short_of_the_kit(($first..$first + 4).collect());
                assert!(short.is_empty(), "{short:?}");
            }
        )*};
    }
    every_spawn! {
        every_spawn_gets_flint_and_sticks_001: 1, every_spawn_gets_flint_and_sticks_005: 5,
        every_spawn_gets_flint_and_sticks_009: 9, every_spawn_gets_flint_and_sticks_013: 13,
        every_spawn_gets_flint_and_sticks_017: 17, every_spawn_gets_flint_and_sticks_021: 21,
        every_spawn_gets_flint_and_sticks_025: 25, every_spawn_gets_flint_and_sticks_029: 29,
        every_spawn_gets_flint_and_sticks_033: 33, every_spawn_gets_flint_and_sticks_037: 37,
        every_spawn_gets_flint_and_sticks_041: 41, every_spawn_gets_flint_and_sticks_045: 45,
        every_spawn_gets_flint_and_sticks_049: 49, every_spawn_gets_flint_and_sticks_053: 53,
        every_spawn_gets_flint_and_sticks_057: 57, every_spawn_gets_flint_and_sticks_061: 61,
    }

    /// The kit lays only into air on natural ground, never on what a player placed.
    /// Where the band by the spawn is all built over, the kit still goes down: in the next
    /// columns for a spawn on a column's corner, and past the band's far edge.
    #[test]
    fn the_kit_reaches_past_a_built_spawn() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_engine::content::Content::load(&dir).unwrap();
        let world_content = voxmar_worldgen::WorldContent::load(&content).unwrap();
        let blocks = &content.blocks;
        let kit = SpawnKit::load(&content.files, blocks).unwrap();
        let overworld = voxmar_worldgen::Overworld::shared(7, &content, &world_content);
        let start = overworld.spawn_point();
        let mut world = VoxelWorld::default();
        let corner = ColumnPos::of_block(start).origin();
        world.insert(ColumnPos::of_block(start), Arc::new(overworld.generate(ColumnPos::of_block(start))));
        let ground = crate::find_ground(&world, blocks, IVec3::new(corner.x, start.y, corner.z));
        // On the corner of its column, a quarter of the band each way.
        let spawn = Vec3::new(corner.x as f32, ground.y, corner.z as f32);
        for at in kit.columns(spawn) {
            if !world.contains(at) {
                world.insert(at, Arc::new(overworld.generate(at)));
            }
        }
        let own = ColumnPos::of_block(spawn.floor().as_ivec3());
        let next = kit.plan(&world, blocks, spawn, 7, |p| ColumnPos::of_block(p) == own);
        assert!(kit.short(blocks, &next).is_empty() && next.iter().all(|&(p, _)| ColumnPos::of_block(p) != own), "{next:?}");
        let across = |p: IVec3| Vec3::new(p.x as f32 + 0.5 - spawn.x, 0.0, p.z as f32 + 0.5 - spawn.z).length();
        let far = kit.plan(&world, blocks, spawn, 7, |p| across(p) <= kit.near.1);
        assert!(kit.short(blocks, &far).is_empty() && far.iter().all(|&(p, _)| across(p) > kit.near.1), "{far:?}");
    }

    #[test]
    fn the_kit_keeps_off_placed_ground() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_engine::content::Content::load(&dir).unwrap();
        let world_content = voxmar_worldgen::WorldContent::load(&content).unwrap();
        let blocks = &content.blocks;
        let kit = SpawnKit::load(&content.files, blocks).unwrap();
        let overworld = voxmar_worldgen::Overworld::shared(7, &content, &world_content);
        let start = overworld.spawn_point();
        let mut world = VoxelWorld::default();
        world.insert(ColumnPos::of_block(start), Arc::new(overworld.generate(ColumnPos::of_block(start))));
        let spawn = crate::find_ground(&world, blocks, start);
        for at in kit.columns(spawn) {
            if world.contains(at) {
                continue;
            }
            world.insert(at, Arc::new(overworld.generate(at)));
        }
        let free = kit.plan(&world, blocks, spawn, 7, |_| false);
        let grounds: Vec<IVec3> = free.iter().map(|&(p, _)| p - IVec3::Y).collect();
        let kept = kit.plan(&world, blocks, spawn, 7, |p| grounds.contains(&p));
        assert!(!kept.is_empty() && kept.iter().all(|&(p, _)| !grounds.contains(&(p - IVec3::Y))), "{free:?} then {kept:?}");
        assert!(free.iter().all(|&(p, _)| world.block(p).is_some_and(|b| b.is_air() || content.blocks.def(b).has_tag("grass"))));
        // A roof a player set over a spot (an older save's house on unbuilt ground) keeps the
        // kit out from under it.
        let roofs: Vec<IVec3> = grounds.iter().map(|&g| g + IVec3::Y * 6).collect();
        let sheltered = kit.plan(&world, blocks, spawn, 7, |p| roofs.contains(&p));
        assert!(!sheltered.is_empty() && sheltered.iter().all(|&(p, _)| !grounds.contains(&(p - IVec3::Y))), "{free:?} then {sheltered:?}");
        // A spot with room for one block only still takes a flint, where the snag would not fit.
        let (flint, snag) = (blocks.expect("base:loose_flint"), blocks.expect("base:dead_log"));
        let first_flint = free.iter().find(|&&(_, b)| b == flint).map(|&(p, _)| p).unwrap();
        let snag_bases: Vec<IVec3> = free.iter().filter(|&&(p, b)| b == snag && !free.contains(&(p - IVec3::Y, snag))).map(|&(p, _)| p).collect();
        for &base in snag_bases.iter().chain([&first_flint]) {
            // Tall grass: no ground, so the search goes on down past it, but no room for a piece.
            world.set_block(base + IVec3::Y, blocks.expect("base:tall_grass"));
        }
        let squeezed = kit.plan(&world, blocks, spawn, 7, |_| false);
        // A stump where a flint would lie: its top is no ground, the flint goes elsewhere.
        // Room above it for a flint, so only the log keeps one off.
        let stump = free.iter().find(|&&(p, b)| b == flint && p != first_flint && world.block(p + IVec3::Y) == Some(BlockId::AIR)).map(|&(p, _)| p).unwrap();
        world.set_block(stump, blocks.expect("base:oak_log"));
        let around = kit.plan(&world, blocks, spawn, 7, |_| false);
        assert!(!around.iter().any(|&(p, _)| p == stump + IVec3::Y), "a piece on the stump: {around:?}");
        world.set_block(stump, BlockId::AIR);
        assert!(squeezed.contains(&(first_flint, flint)), "a one-high spot still takes its flint: {squeezed:?}");
        assert!(snag_bases.iter().all(|&b| !squeezed.iter().any(|&(p, k)| p == b && k == snag)), "no snag under a ceiling: {squeezed:?}");
        assert!(squeezed.iter().any(|&(_, k)| k == snag), "the snag finds another spot: {squeezed:?}");
    }
}
