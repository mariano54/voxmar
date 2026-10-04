//! Trees, cacti and boulders, placed from a jittered grid of candidates. Candidates near
//! a column are evaluated from pure height and climate functions, so a tree straddling
//! two columns is generated identically by both.

use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::{IVec3, Vec3};

use crate::caves::CaveColumn;
use crate::ecology::Ground;
use crate::geology::{GeoGrid, smoothstep};
use crate::rng::{Rng, hash3};
use crate::terrain::Blocks;
use crate::trees::{Tree, Trees};
use crate::{Overworld, SEA_LEVEL, Sampler, Site};

const PURPOSE: u64 = 0x7233;
const CELL: i32 = 4;
/// Furthest a feature reaches from its candidate position, horizontally.
pub const REACH: i32 = crate::trees::REACH;

pub struct Features {
    trees: Trees,
    mossy_cobblestone: BlockId,
    cobblestone: BlockId,
}

impl Features {
    pub fn new(registry: &BlockRegistry) -> Self {
        Self {
            trees: Trees::new(registry),
            mossy_cobblestone: registry.expect("base:mossy_cobblestone"),
            cobblestone: registry.expect("base:cobblestone"),
        }
    }

    pub fn trees(&self) -> &Trees {
        &self.trees
    }

    pub fn place(&self, world: &Overworld, area: &Sampler, geology: &GeoGrid, caves: &CaveColumn, pos: ColumnPos, blocks: &mut Blocks) {
        let seed = world.seed;
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
                let geo = geology.get(x, z);
                if geo.height < SEA_LEVEL - 2 {
                    continue;
                }
                let site = area.site_on(&geo, x, z, 0);
                let roll = rng.unit();
                let base = IVec3::new(x, geo.height + 1, z);
                // Nothing grows over a cave mouth.
                if caves.surface_open(x, z, &geo) {
                    continue;
                }
                let ecology = &world.ecology;
                if roll >= ecology.tree_chances(&site, None, x, z).iter().sum::<f32>() {
                    if roll > 1.0 - boulder_chance(&site) && site.height > SEA_LEVEL {
                        self.boulder(base, &mut rng, blocks);
                    }
                    continue;
                }
                // Only now find the ground, which needs the slope.
                let site = Site { slope: geology.slope(x, z), ..site };
                let ground = if site.height < SEA_LEVEL {
                    Ground::Shallows
                } else {
                    world.terrain.ground(world.terrain.surface_layers(&site, x, z).top)
                };
                let mut left = roll;
                for (tree, chance) in Tree::ALL.into_iter().zip(ecology.tree_chances(&site, Some(ground), x, z)) {
                    if left < chance {
                        self.trees.grow(tree, base, &mut rng, blocks);
                        break;
                    }
                    left -= chance;
                }
            }
        }
    }

    /// A lump of mossy cobblestone half sunk into the ground.
    fn boulder(&self, base: IVec3, rng: &mut Rng, blocks: &mut Blocks) {
        let r = 1.0 + rng.unit() * 1.2;
        let center = base.as_vec3() + Vec3::new(0.5, -0.3, 0.5);
        let seed = rng.next_u64();
        let span = r.ceil() as i32;
        for y in -span..=span {
            for z in -span..=span {
                for x in -span..=span {
                    let cell = base + IVec3::new(x, y, z);
                    let d = (cell.as_vec3() + 0.5 - center) / Vec3::new(r, r * 0.8, r);
                    let noise = (hash3(seed, cell.x, cell.y, cell.z) & 0xff) as f32 / 255.0;
                    if d.length_squared() <= 1.0 - 0.3 * noise {
                        blocks.put_if_air(cell, if noise < 0.6 { self.mossy_cobblestone } else { self.cobblestone });
                    }
                }
            }
        }
    }
}

/// Moss-covered boulders lie in damp, cool forests.
fn boulder_chance(site: &Site) -> f32 {
    0.012 * smoothstep(0.3, 0.6, site.forest) * smoothstep(0.9, 1.3, site.soil_moisture()) * (1.0 - smoothstep(10.0, 16.0, site.temperature))
}
