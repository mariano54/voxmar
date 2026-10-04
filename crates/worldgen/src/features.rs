//! Trees and ground decorations, placed from a jittered grid of candidates in a fixed
//! order of stages: patches of ground, rocks, trees and bushes, the ornaments each tree
//! makes of its own blocks, then fallen logs and stumps (see `decor`). Each candidate
//! holds at most one tree or decoration, decided by one roll from pure height and climate
//! functions, so a feature straddling two columns is generated identically by both, and
//! every column places them in the same order.

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::{IVec2, IVec3};

use crate::caves::CaveColumn;
use crate::content::WorldContent;
use crate::decor::{Area, DecorId, Probe, Stage};
use crate::ecology::Ground;
use crate::geology::{GeoGrid, GeoSample};
use crate::rng::Rng;
use crate::terrain::Blocks;
use crate::trees::{TreeId, TreeParts, Trees};
use crate::{Overworld, Sampler, Site};

const PURPOSE: u64 = 0x7233;
/// Patches take a roll of their own, so they lie under trees and rocks too.
const PATCHES: u64 = 0x9a7c;
/// Candidates lie one per cell this many blocks square.
pub const CELL: i32 = 4;
/// Furthest a feature reaches from its candidate position, horizontally.
pub const REACH: i32 = crate::trees::REACH;

pub struct Features {
    trees: Trees,
}

/// A candidate position, before deciding what grows there.
struct Candidate {
    x: i32,
    z: i32,
    geo: GeoSample,
    site: Site,
    rng: Rng,
    roll: f32,
}

/// What a candidate holds.
enum Holds {
    Tree(TreeId),
    Decoration(DecorId),
}

struct Planned {
    holds: Holds,
    /// The first block above the ground at the candidate.
    base: IVec3,
    site: Site,
    rng: Rng,
}

impl Features {
    pub fn new(content: &WorldContent, registry: &BlockRegistry) -> Self {
        Self { trees: Trees::new(content, registry) }
    }

    pub fn trees(&self) -> &Trees {
        &self.trees
    }

    /// The candidate of grid cell (cx, cz), unless it lies under deep water or over a cave
    /// mouth. `inside` may reject its position before anything costly is computed.
    fn candidate(&self, area: &Area, cx: i32, cz: i32, inside: impl Fn(i32, i32) -> bool) -> Option<Candidate> {
        let mut rng = Rng::new(area.world.seed, PURPOSE, cx, cz);
        let x = cx * CELL + rng.below(CELL as u32);
        let z = cz * CELL + rng.below(CELL as u32);
        if !inside(x, z) {
            return None;
        }
        let geo = area.geology.get(x, z);
        // Nothing grows under deep water or on a trail.
        if geo.water - geo.height > 2 || geo.trail.is_some_and(|t| t.clear) {
            return None;
        }
        let site = area.sampler.site_on(&geo, x, z, 0);
        let roll = rng.unit();
        // Nothing grows over a cave mouth.
        if area.caves.surface_open(x, z, &geo) {
            return None;
        }
        Some(Candidate { x, z, geo, site, rng, roll })
    }

    /// The tree growing at a candidate, if any, and the share of the roll trees may take
    /// there (the rest goes to decorations).
    fn tree(&self, area: &Area, c: &Candidate, chances: &mut Vec<(TreeId, f32)>) -> (Option<(TreeId, Site)>, f32) {
        let ecology = &area.world.ecology;
        ecology.tree_chances(&c.site, None, c.x, c.z, chances);
        let upper: f32 = chances.iter().map(|c| c.1).sum();
        if c.roll >= upper {
            return (None, upper);
        }
        // Only now find the ground, which needs the slope.
        let site = Site { slope: area.geology.slope(c.x, c.z), ..c.site };
        let ground = self.ground(area, &site, c);
        ecology.tree_chances(&site, Some(ground), c.x, c.z, chances);
        let mut left = c.roll;
        for &(tree, chance) in chances.iter() {
            if left < chance {
                return (Some((tree, site)), upper);
            }
            left -= chance;
        }
        (None, upper)
    }

    /// What the ground at a candidate offers.
    fn ground(&self, area: &Area, site: &Site, c: &Candidate) -> Ground {
        if c.geo.submerged() { Ground::Shallows } else { area.world.terrain.ground(area.world.surface_layers(site, c.x, c.z).top) }
    }

    /// Where a tree's trunk stands in grid cell (cx, cz), if a tree grows there: for
    /// decorations that must keep clear of trunks.
    pub fn tree_at(&self, area: &Area, cx: i32, cz: i32) -> Option<IVec2> {
        let c = self.candidate(area, cx, cz, |_, _| true)?;
        thread_local! {
            static CHANCES: std::cell::RefCell<Vec<(TreeId, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        CHANCES.with_borrow_mut(|chances| self.tree(area, &c, chances).0.map(|_| IVec2::new(c.x, c.z)))
    }

    pub fn place(&self, world: &Overworld, sampler: &Sampler, geology: &GeoGrid, caves: &CaveColumn, pos: ColumnPos, blocks: &mut Blocks) {
        let area = Area { world, sampler, geology, caves, trees: &self.trees };
        let decor = &world.decor;
        let origin = pos.origin();
        let inside = |x: i32, z: i32| {
            x >= origin.x - REACH && x < origin.x + CHUNK_SIZE + REACH && z >= origin.z - REACH && z < origin.z + CHUNK_SIZE + REACH
        };

        // Decide what every candidate within reach holds.
        let mut planned: Vec<Planned> = Vec::new();
        let mut chances = Vec::new();
        let first = (origin.x - REACH).div_euclid(CELL)..=(origin.x + CHUNK_SIZE + REACH).div_euclid(CELL);
        for cz in (origin.z - REACH).div_euclid(CELL)..=(origin.z + CHUNK_SIZE + REACH).div_euclid(CELL) {
            for cx in first.clone() {
                let Some(c) = self.candidate(&area, cx, cz, inside) else { continue };
                let base = IVec3::new(c.x, c.geo.height + 1, c.z);
                let mut slope = None;
                let mut slope_of = || *slope.get_or_insert_with(|| geology.slope(c.x, c.z));
                // Patches, on their own roll.
                if decor.has_patches(&c.site) && !c.geo.submerged() {
                    let mut rng = Rng::new(world.seed, PATCHES, cx, cz);
                    let probe = &mut Probe { rules: &world.rules, x: c.x, z: c.z, slope: &mut slope_of };
                    if let Some(id) = decor.choose_patch(&c.site, rng.unit(), probe) {
                        planned.push(Planned { holds: Holds::Decoration(id), base, site: c.site, rng });
                    }
                }
                let (tree, upper) = self.tree(&area, &c, &mut chances);
                if let Some((tree, site)) = tree {
                    planned.push(Planned { holds: Holds::Tree(tree), base, site, rng: c.rng });
                    continue;
                }
                if c.roll < upper || !decor.places_any(&c.site) {
                    continue;
                }
                let probe = &mut Probe { rules: &world.rules, x: c.x, z: c.z, slope: &mut slope_of };
                let Some(id) = decor.choose_placed(&c.site, c.roll - upper, probe) else { continue };
                let site = Site { slope: slope_of(), ..c.site };
                if decor.stands_on(id, self.ground(&area, &site, &c)) {
                    planned.push(Planned { holds: Holds::Decoration(id), base, site, rng: c.rng });
                }
            }
        }

        // Then place them stage by stage, each in candidate order.
        let stage = |p: &Planned| match p.holds {
            Holds::Tree(_) => Stage::Trees,
            Holds::Decoration(id) => decor.stage(id),
        };
        let mut parts = TreeParts::default();
        let mut grown: Vec<(TreeId, IVec3, Site, TreeParts)> = Vec::new();
        for now in [Stage::Patches, Stage::Rocks, Stage::Trees, Stage::Logs] {
            for p in planned.iter_mut().filter(|p| stage(p) == now) {
                match p.holds {
                    Holds::Tree(tree) => {
                        self.trees.grow(tree, p.base, p.site.stand, &mut p.rng, blocks, &mut parts);
                        if !parts.leaves.is_empty() || !parts.logs.is_empty() {
                            grown.push((tree, p.base, p.site, std::mem::take(&mut parts)));
                        }
                    }
                    Holds::Decoration(id) => decor.place(id, &area, &p.site, p.base, &mut p.rng, blocks, &mut parts),
                }
            }
            if now == Stage::Trees {
                for (tree, base, site, parts) in &grown {
                    decor.ornament(&area, site, *tree, *base, parts, blocks);
                }
            }
        }
    }
}
