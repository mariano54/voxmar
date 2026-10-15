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
    /// loaded: each piece's cells, from the world's `seed`. A spot is natural ground (opaque,
    /// under any canopy of leaves) no player placed (`placed`), with room above for the piece
    /// (air, or meadow grass it lies over); the band `near` first, in an order drawn from
    /// the seed, then nearest first out to the columns' edges, so a small islet still takes
    /// it. Pieces that find no spot are left out.
    pub fn plan(&self, world: &VoxelWorld, registry: &BlockRegistry, spawn: Vec3, seed: u64, placed: impl Fn(IVec3) -> bool) -> Vec<(IVec3, BlockId)> {
        let feet = spawn.floor().as_ivec3();
        let tallest = self.pieces.iter().map(|p| p.3).max().unwrap_or(1) as i32;
        let def = |p: IVec3| world.block(p).map(|b| (b, registry.def(b), registry.info(b)));
        // Room for a piece: air, or a low meadow plant (never one two high).
        let room = |p: IVec3| def(p).is_some_and(|(b, d, i)| b.is_air() || !i.solid && d.has_tag("grass") && !d.has_tag("tall") && !placed(p));
        let spot = |x: i32, z: i32| -> Option<IVec3> {
            // Down from a little above the feet, past any canopy (a spawn may stand on one),
            // to the ground.
            let mut y = feet.y + 8;
            let top = loop {
                if y < feet.y - 32 {
                    return None;
                }
                let p = IVec3::new(x, y, z);
                let (_, d, i) = def(p)?;
                if i.solid && !["leaves", "log", "wood"].iter().any(|t| d.has_tag(t)) {
                    break p;
                }
                y -= 1;
            };
            let (_, ground, info) = def(top)?;
            let firm = info.render == RenderKind::Opaque && ground.has_tag("natural") && !placed(top);
            (firm && (1..=tallest).all(|dy| room(top + IVec3::Y * dy))).then_some(top + IVec3::Y)
        };
        let (lo, hi) = self.near;
        let mut spots: Vec<(bool, u64, IVec3)> = self
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
                let at = spot(x, z)?;
                let order = if d > hi { (d * 16.0) as u64 } else { mix(seed ^ 0x5b17, (x as u64) << 32 ^ z as u32 as u64) };
                Some((d > hi, order, at))
            })
            .collect();
        spots.sort_unstable_by_key(|&(far, order, at)| (far, order, at.x, at.z));
        let mut spots = spots.into_iter().map(|(_, _, at)| at);
        let mut laid = Vec::new();
        for (i, &(block, least, most, height)) in self.pieces.iter().enumerate() {
            let count = least + (mix(seed ^ 0x5b18, i as u64) % u64::from(most - least + 1)) as u8;
            for at in spots.by_ref().take(count as usize) {
                laid.extend((0..height as i32).map(|dy| (at + IVec3::Y * dy, block)));
            }
        }
        laid
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

    /// Every one of two hundred worlds' spawns gets the kit (slow: the full gate's).
    #[test]
    fn every_spawn_gets_flint_and_sticks() {
        let short = short_of_the_kit((1..=200).collect());
        assert!(short.is_empty(), "{short:?}");
    }

    /// The kit lays only into air on natural ground, never on what a player placed.
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
    }
}
