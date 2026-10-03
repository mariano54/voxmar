//! Trees and cacti, placed from a jittered grid of candidates. Candidates near a column
//! are evaluated from pure height and climate functions, so a tree straddling two columns
//! is generated identically by both.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::IVec3;

use crate::ecology::{Ecology, Ground, TREE_CHANCE, Tree};
use crate::rng::Rng;
use crate::terrain::Blocks;
use crate::{Overworld, SEA_LEVEL, Sampler, Site};

const PURPOSE: u64 = 0x7233;
const CELL: i32 = 4;
/// Furthest a feature reaches from its origin, horizontally.
pub const REACH: i32 = 3;

struct Wood {
    log: BlockId,
    leaves: BlockId,
}

pub struct Features {
    /// Per `Tree`.
    woods: [Wood; 5],
    cactus: BlockId,
}

impl Features {
    pub fn new(registry: &BlockRegistry) -> Self {
        let wood = |tree: Tree| Wood {
            log: registry.expect(&format!("base:{}_log", tree.name())),
            leaves: registry.expect(&format!("base:{}_leaves", tree.name())),
        };
        Self { woods: Tree::ALL.map(wood), cactus: registry.expect("base:cactus") }
    }

    pub fn place(&self, world: &Overworld, area: &Sampler, seed: u64, pos: ColumnPos, blocks: &mut Blocks) {
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
                let geo = world.geology.sample(x, z);
                if geo.height <= SEA_LEVEL {
                    continue;
                }
                let site = area.site_on(&geo, x, z, 0);
                let (tree, cactus) = (site.forest * TREE_CHANCE, Ecology::cactus_chance(&site));
                let roll = rng.unit();
                if roll >= tree + cactus {
                    continue;
                }
                // Only now find the ground, which needs the slope.
                let site = Site { slope: world.slope(x, z, geo.height), ..site };
                let ground = world.terrain.ground(world.terrain.surface_layers(&site, x, z).0);
                let base = IVec3::new(x, geo.height + 1, z);
                if roll < tree && ground == Ground::Soil {
                    let kind = world.ecology.tree(&site, x, z, &mut rng);
                    self.grow(kind, base, &mut rng, blocks);
                } else if roll >= tree && ground == Ground::Sand {
                    for y in 0..rng.range(1, 4) {
                        blocks.put_if_air(base + IVec3::Y * y, self.cactus);
                    }
                }
            }
        }
    }

    fn grow(&self, kind: Tree, base: IVec3, rng: &mut Rng, blocks: &mut Blocks) {
        let wood = &self.woods[kind as usize];
        let trunk = match kind {
            Tree::Oak => rng.range(4, 7),
            Tree::Birch => rng.range(5, 8),
            Tree::Spruce => rng.range(6, 10),
            Tree::Jungle => rng.range(8, 13),
            Tree::Acacia => rng.range(4, 6),
        };
        let top = base + IVec3::Y * trunk;

        match kind {
            Tree::Spruce => {
                // Cone of alternating radii from just above the ground to the tip.
                let mut radius = 0;
                for y in (2..=trunk + 1).rev() {
                    disc(blocks, base + IVec3::Y * y, radius, wood.leaves, rng, false);
                    radius = if radius >= 2 + (trunk - y) / 4 { 1 } else { radius + 1 };
                }
            }
            Tree::Acacia => {
                disc(blocks, top, 3, wood.leaves, rng, true);
                disc(blocks, top + IVec3::Y, 2, wood.leaves, rng, true);
            }
            Tree::Jungle => {
                for (dy, radius) in [(-2, 3), (-1, 3), (0, 2), (1, 1)] {
                    disc(blocks, top + IVec3::Y * dy, radius, wood.leaves, rng, true);
                }
            }
            Tree::Oak | Tree::Birch => {
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
