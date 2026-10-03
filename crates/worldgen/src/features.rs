//! Surface features (trees, cacti), placed from a jittered grid of candidates. Candidates
//! near a column are evaluated from pure height and biome functions, so a tree straddling
//! two columns is generated identically by both.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::IVec3;

use crate::climate::Biome;
use crate::rng::Rng;
use crate::terrain::Blocks;
use crate::{Overworld, SEA_LEVEL};

const PURPOSE: u64 = 0x7233;
const CELL: i32 = 4;
/// Furthest a feature reaches from its origin, horizontally.
const REACH: i32 = 3;

#[derive(Clone, Copy)]
enum Kind {
    Oak,
    Birch,
    Spruce,
    Jungle,
    Acacia,
    Cactus,
}

struct Wood {
    log: BlockId,
    leaves: BlockId,
}

pub struct Features {
    oak: Wood,
    birch: Wood,
    spruce: Wood,
    jungle: Wood,
    acacia: Wood,
    cactus: BlockId,
}

impl Features {
    pub fn new(registry: &BlockRegistry) -> Self {
        let wood = |name: &str| Wood {
            log: registry.expect(&format!("base:{name}_log")),
            leaves: registry.expect(&format!("base:{name}_leaves")),
        };
        Self {
            oak: wood("oak"),
            birch: wood("birch"),
            spruce: wood("spruce"),
            jungle: wood("jungle"),
            acacia: wood("acacia"),
            cactus: registry.expect("base:cactus"),
        }
    }

    pub fn place(&self, world: &Overworld, seed: u64, pos: ColumnPos, blocks: &mut Blocks) {
        let origin = pos.origin();
        let first = (origin.x - REACH).div_euclid(CELL)..=(origin.x + CHUNK_SIZE + REACH).div_euclid(CELL);
        for cz in (origin.z - REACH).div_euclid(CELL)..=(origin.z + CHUNK_SIZE + REACH).div_euclid(CELL) {
            for cx in first.clone() {
                let mut rng = Rng::new(seed, PURPOSE, cx, cz);
                let x = cx * CELL + rng.below(CELL as u32);
                let z = cz * CELL + rng.below(CELL as u32);
                if x < origin.x - REACH
                    || x >= origin.x + CHUNK_SIZE + REACH
                    || z < origin.z - REACH
                    || z >= origin.z + CHUNK_SIZE + REACH
                {
                    continue;
                }
                let (height, climate) = world.surface(x, z);
                if height <= SEA_LEVEL {
                    continue;
                }
                let Some(kind) = pick(climate.biome, &mut rng) else { continue };
                self.grow(kind, IVec3::new(x, height + 1, z), &mut rng, blocks);
            }
        }
    }

    fn grow(&self, kind: Kind, base: IVec3, rng: &mut Rng, blocks: &mut Blocks) {
        let (wood, trunk) = match kind {
            Kind::Cactus => {
                for y in 0..rng.range(1, 4) {
                    blocks.put_if_air(base + IVec3::Y * y, self.cactus);
                }
                return;
            }
            Kind::Oak => (&self.oak, rng.range(4, 7)),
            Kind::Birch => (&self.birch, rng.range(5, 8)),
            Kind::Spruce => (&self.spruce, rng.range(6, 10)),
            Kind::Jungle => (&self.jungle, rng.range(8, 13)),
            Kind::Acacia => (&self.acacia, rng.range(4, 6)),
        };
        let top = base + IVec3::Y * trunk;

        match kind {
            Kind::Spruce => {
                // Cone of alternating radii from just above the ground to the tip.
                let mut radius = 0;
                for y in (2..=trunk + 1).rev() {
                    disc(blocks, base + IVec3::Y * y, radius, wood.leaves, rng, false);
                    radius = if radius >= 2 + (trunk - y) / 4 { 1 } else { radius + 1 };
                }
            }
            Kind::Acacia => {
                disc(blocks, top, 3, wood.leaves, rng, true);
                disc(blocks, top + IVec3::Y, 2, wood.leaves, rng, true);
            }
            Kind::Jungle => {
                for (dy, radius) in [(-2, 3), (-1, 3), (0, 2), (1, 1)] {
                    disc(blocks, top + IVec3::Y * dy, radius, wood.leaves, rng, true);
                }
            }
            _ => {
                for (dy, radius) in [(-2, 2), (-1, 2), (0, 1), (1, 1)] {
                    disc(blocks, top + IVec3::Y * dy, radius, wood.leaves, rng, dy >= 0);
                }
            }
        }
        for y in 0..trunk {
            blocks.put_log(base + IVec3::Y * y, wood.log);
        }
    }
}

/// A horizontal disc of leaves; `round` trims the corners.
fn disc(blocks: &mut Blocks, center: IVec3, radius: i32, leaves: BlockId, rng: &mut Rng, round: bool) {
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let corner = dx.abs() == radius && dz.abs() == radius && radius > 0;
            if corner && (round || rng.chance(0.5)) {
                continue;
            }
            blocks.put_if_air(center + IVec3::new(dx, 0, dz), leaves);
        }
    }
}

fn pick(biome: Biome, rng: &mut Rng) -> Option<Kind> {
    let (density, kind) = match biome {
        Biome::Forest => (0.45, if rng.chance(0.2) { Kind::Birch } else { Kind::Oak }),
        Biome::BirchForest => (0.4, Kind::Birch),
        Biome::Taiga => (0.4, Kind::Spruce),
        Biome::SnowyTundra => (0.03, Kind::Spruce),
        Biome::Mountains => (0.04, Kind::Spruce),
        Biome::Jungle => (0.6, if rng.chance(0.3) { Kind::Jungle } else { Kind::Oak }),
        Biome::Savanna => (0.05, Kind::Acacia),
        Biome::Plains => (0.02, Kind::Oak),
        Biome::Desert => (0.025, Kind::Cactus),
        _ => return None,
    };
    rng.chance(density).then_some(kind)
}
