//! Stage 3: ore veins. Each column seeds its own veins; a column also places the parts of
//! its neighbours' veins that reach into it, so veins cross borders seamlessly. Where a
//! vein runs through deepslate it takes the ore's deepslate form (`deepslate_{name}`),
//! drawn over deepslate, so the deep layers' ore matches the rock around it.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::IVec3;

use crate::SEA_LEVEL;
use crate::rng::Rng;
use crate::terrain::Blocks;

const PURPOSE: u64 = 0x04e5;

struct Ore {
    block: BlockId,
    /// Its form in deepslate, if it has one.
    deep: Option<BlockId>,
    attempts: u32,
    heights: (i32, i32),
    size: u32,
}

pub struct Resources {
    ores: Vec<Ore>,
    deepslate: BlockId,
}

impl Resources {
    pub fn new(registry: &BlockRegistry) -> Self {
        let ore = |name, attempts, heights, size| Ore {
            block: registry.expect(&format!("base:{name}")),
            deep: registry.id(&format!("base:deepslate_{name}")),
            attempts,
            heights,
            size,
        };
        Self {
            deepslate: registry.expect("base:deepslate"),
            ores: vec![
                ore("coal_ore", 24, (SEA_LEVEL - 59, SEA_LEVEL + 126), 12),
                ore("copper_ore", 10, (SEA_LEVEL - 59, SEA_LEVEL + 46), 9),
                ore("iron_ore", 27, (5, SEA_LEVEL + 36), 8),
                ore("gold_ore", 9, (5, SEA_LEVEL - 40), 7),
                ore("diamond_ore", 8, (5, SEA_LEVEL - 88), 6),
            ],
        }
    }

    pub fn place(&self, seed: u64, pos: ColumnPos, blocks: &mut Blocks) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let source = pos.offset(dx, dz);
                let origin = source.origin();
                let mut rng = Rng::new(seed, PURPOSE, source.0.x, source.0.y);
                for ore in &self.ores {
                    for _ in 0..ore.attempts {
                        let mut p = origin + IVec3::new(rng.below(32), rng.range(ore.heights.0, ore.heights.1), rng.below(32));
                        for _ in 0..ore.size {
                            let block = match ore.deep {
                                Some(deep) if blocks.get(p) == self.deepslate => deep,
                                _ => ore.block,
                            };
                            blocks.replace_stone(p, block);
                            p[rng.below(3) as usize] += if rng.chance(0.5) { 1 } else { -1 };
                        }
                    }
                }
            }
        }
    }
}
