//! Trees and ground decorations, placed from a jittered grid of candidates in a fixed
//! order of stages: patches of ground, trees and bushes, the ornaments each tree makes of
//! its own blocks, rocks, then fallen logs and stumps (see `decor`). Rocks come after the
//! trees so that they settle round trunks and roots, and never cut a limb whose far end
//! another column would still draw. Each candidate holds at most one tree or decoration,
//! decided by one roll from pure height and climate functions, so a feature straddling two
//! columns is generated identically by both, and every column places them in the same
//! order. Trees keep clear of arches and leaning spires for the same reason.
//!
//! Leaves take their colour from their own tree's trunk (see `tint`), so a crown is one
//! colour all through.

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::{IVec2, IVec3};

use crate::caves::CaveColumn;
use crate::content::WorldContent;
use crate::decor::{Area, DecorId, Probe, Stage};
use crate::ecology::Ground;
use crate::geology::{GeoGrid, GeoSample};
use crate::rng::Rng;
use crate::shapes::Piece;
use crate::stage;
use crate::terrain::Blocks;
use crate::trees::{TreeId, TreeParts, Trees};
use crate::{Overworld, Sampler, Site};

const PURPOSE: u64 = 0x7233;
/// Patches take a roll of their own, so they lie under trees and rocks too.
const PATCHES: u64 = 0x9a7c;
/// Each tree's own shade of its foliage colour.
const TINT: u64 = 0x7147;
/// Candidates lie one per cell this many blocks square.
pub const CELL: i32 = 4;
/// Furthest a feature reaches from its candidate position, horizontally.
pub const REACH: i32 = crate::trees::REACH;
/// How far from a column the arches and leaning spires that trees keep clear of must be
/// known: trees reach `REACH` from candidates up to `REACH` outside the column, and
/// decorations ask about trunks up to a log's length further out.
pub const ROCK_REACH: i32 = 3 * REACH;

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
                return (Self::clear_of_rock(area, c).then_some((tree, site)), upper);
            }
            left -= chance;
        }
        (None, upper)
    }

    /// Whether no arch or leaning spire stands within a tree's reach of a candidate, where
    /// a limb running into the rock would leave its far end hanging in the air.
    fn clear_of_rock(area: &Area, c: &Candidate) -> bool {
        let y = c.geo.height;
        area.pieces.iter().all(|p| {
            p.extent(y - 4, y + 52).is_none_or(|(lo, hi)| hi.x < c.x - REACH || lo.x > c.x + REACH || hi.y < c.z - REACH || lo.y > c.z + REACH)
        })
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

    /// Places the trees and decorations reaching the column at `pos`, and colours the
    /// foliage of every block column holding their leaves (`colors`, grass, foliage and
    /// water per (x, z)) as their trunks' (see `tint`).
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &self,
        world: &Overworld,
        sampler: &Sampler,
        geology: &GeoGrid,
        caves: &CaveColumn,
        pieces: &[Piece],
        pos: ColumnPos,
        blocks: &mut Blocks,
        colors: &mut [[[u8; 3]; 3]],
    ) {
        let area = Area { world, sampler, geology, caves, pieces, trees: &self.trees };
        let ground = |x: i32, z: i32| geology.get(x, z).height;
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
                let (tree, upper) = {
                    let _stage = stage::enter(stage::Stage::Trees);
                    self.tree(&area, &c, &mut chances)
                };
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
        // The leaves of bushes, by the bush's base and site, for their colour.
        let mut bushes: Vec<(IVec3, Site, Vec<IVec3>)> = Vec::new();
        for now in [Stage::Patches, Stage::Trees, Stage::Rocks, Stage::Logs] {
            for p in planned.iter_mut().filter(|p| stage(p) == now) {
                match p.holds {
                    Holds::Tree(tree) => {
                        let _stage = stage::enter(stage::Stage::Trees);
                        self.trees.grow(tree, p.base, p.site.stand, &ground, &mut p.rng, blocks, &mut parts);
                        if !parts.leaves.is_empty() || !parts.logs.is_empty() {
                            grown.push((tree, p.base, p.site, std::mem::take(&mut parts)));
                        }
                    }
                    Holds::Decoration(id) => {
                        parts.leaves.clear();
                        decor.place(id, &area, &p.site, p.base, &mut p.rng, blocks, &mut parts);
                        if !parts.leaves.is_empty() {
                            bushes.push((p.base, p.site, std::mem::take(&mut parts.leaves)));
                        }
                    }
                }
            }
            if now == Stage::Trees {
                for (tree, base, site, parts) in &grown {
                    decor.ornament(&area, site, *tree, *base, parts, blocks);
                }
            }
        }

        let crowns = grown.iter().map(|(_, base, site, parts)| (*base, site, &parts.leaves));
        tint(world, sampler, origin, crowns.chain(bushes.iter().map(|(base, site, leaves)| (*base, site, leaves))), colors);
    }
}

/// Colours the foliage of every block column holding leaves of the given trees and bushes
/// (base, site and leaves in the column) as the climate at their trunks colours it, each
/// a shade lighter or darker, warmer or cooler than the next. A crown is then one colour
/// all through, however sharply the climate's colours change under it (by a river, across
/// a biome's border), and every column reaches the same colours: the trees, and the
/// climate at their trunks, are the same from every side.
fn tint<'a>(world: &Overworld, sampler: &Sampler, origin: IVec3, crowns: impl Iterator<Item = (IVec3, &'a Site, &'a Vec<IVec3>)>, colors: &mut [[[u8; 3]; 3]]) {
    let crowns = crowns.filter(|c| !c.2.is_empty()).map(|(base, site, leaves)| {
        let foliage = sampler.colors_of(site, base.x, base.z).foliage;
        let bits = crate::rng::hash3(world.seed ^ TINT, base.x, base.y, base.z);
        let unit = |shift: u32| ((bits >> shift) & 0xffff) as f32 / 65536.0;
        let (shade, warmth) = (0.94 + 0.12 * unit(0), 0.06 * (unit(16) - 0.5));
        ([foliage[0] as f32 * shade * (1.0 + warmth), foliage[1] as f32 * shade, foliage[2] as f32 * shade * (1.0 - warmth)], leaves.as_slice())
    });
    paint(crowns, origin, colors);
}

/// Sets the foliage colour of every block column of the column at `origin` that holds
/// leaves of the given crowns (a colour and the leaves) to their colours, mixed by how
/// many of its leaves each holds where crowns overlap.
fn paint<'a>(crowns: impl Iterator<Item = ([f32; 3], &'a [IVec3])>, origin: IVec3, colors: &mut [[[u8; 3]; 3]]) {
    let mut sums = [[0.0f32; 4]; (CHUNK_SIZE * CHUNK_SIZE) as usize];
    for (color, leaves) in crowns {
        for leaf in leaves {
            let sum = &mut sums[((leaf.z - origin.z) * CHUNK_SIZE + leaf.x - origin.x) as usize];
            for k in 0..3 {
                sum[k] += color[k];
            }
            sum[3] += 1.0;
        }
    }
    for (sum, color) in sums.iter().zip(colors.iter_mut()) {
        if sum[3] > 0.0 {
            color[1] = std::array::from_fn(|k| (sum[k] / sum[3]).round().clamp(0.0, 255.0) as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crown's leaves take its colour in every block column they hold, overlapping
    /// crowns mix by their leaves there, and columns without leaves keep theirs.
    #[test]
    fn crowns_are_painted_one_colour() {
        let origin = IVec3::new(64, 0, -32);
        let field = [[10, 20, 30], [40, 50, 60], [70, 80, 90]];
        let mut colors = vec![field; (CHUNK_SIZE * CHUNK_SIZE) as usize];
        let at = |x: i32, y: i32, z: i32| origin + IVec3::new(x, y, z);
        let first = [at(0, 70, 0), at(0, 71, 0), at(1, 70, 0), at(2, 70, 5)];
        let second = [at(1, 72, 0)];
        paint([([100.0, 150.0, 50.0], first.as_slice()), ([200.0, 90.0, 20.0], second.as_slice())].into_iter(), origin, &mut colors);
        let foliage = |x: i32, z: i32| colors[(z * CHUNK_SIZE + x) as usize][1];
        assert_eq!(foliage(0, 0), [100, 150, 50]);
        assert_eq!(foliage(2, 5), [100, 150, 50]);
        assert_eq!(foliage(1, 0), [150, 120, 35]);
        assert_eq!(foliage(3, 0), field[1]);
        // Grass and water are left alone.
        assert_eq!(colors[0][0], field[0]);
        assert_eq!(colors[0][2], field[2]);
    }
}
