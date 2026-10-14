//! Hoards: the locked treasure chests the land keeps (`hoards.ron`). Each is left on a floor
//! under rock, a cave's floor or a ledge under an overhang, where exploring pays off, never
//! out in the open. The land's level (`levels`) picks which age's chest it is, so the first
//! ages' chests lie near where the world began and later ones further out, and each square
//! of a wide grid holds at most a few: the world stays uncrowded, and a rarer chest stands in
//! fewer squares than a plainer one.
//!
//! Every choice is a pure function of the world's seed and the square, and a chest stands in
//! one column, decided from that column's own blocks, so it comes out the same whichever
//! column is made first.

use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::glam::IVec2;

use crate::content::{ContentError, qualify};
use crate::rng::Rng;
use crate::Overworld;

const PURPOSE: u64 = 0x40a2_d000;

/// How far from its spot a try looks for a floor under rock, in blocks.
pub const SEARCH: i32 = 6;

/// The grid hoards are spread over: one square of `spacing` blocks a side holds `slots`
/// tries, each at its own spot.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoardGrid {
    pub spacing: i32,
    pub slots: u32,
}

impl Default for HoardGrid {
    fn default() -> Self {
        Self { spacing: 128, slots: 2 }
    }
}

/// A kind of place a chest is left: which chest, on what land, how deep under its surface,
/// and how often a try is one.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoardDef {
    /// The chest block (tagged `locked`, `treasure.ron`).
    pub chest: String,
    /// The land's levels it is left on, least first (1 ..= 10).
    pub levels: (u8, u8),
    /// How far below the column's surface its floor lies, in blocks, least first: a few for
    /// a ledge under an overhang or the floor of an arch, more for a cave.
    pub depth: (i32, i32),
    /// The most air over the floor before rock: a low roof is a hollow, not a hall.
    #[serde(default = "roof")]
    pub roof: i32,
    /// The chance a try on such land is this kind (tried in file order, the first that
    /// comes is the one).
    pub chance: f32,
}

fn roof() -> i32 {
    6
}

/// `hoards.ron`, as packages left it.
#[derive(Clone, Debug, Default)]
pub struct HoardsDef {
    pub grid: HoardGrid,
    pub hoards: Vec<HoardDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HoardFile {
    grid: Option<HoardGrid>,
    #[serde(default)]
    hoards: Vec<HoardDef>,
}

impl HoardsDef {
    /// Adds a package's `hoards.ron`, its chests qualified by its package. A later grid
    /// replaces an earlier one; hoards accumulate.
    pub fn extend(&mut self, at: &str, package: &str, source: &str) -> Result<(), ContentError> {
        let file: HoardFile = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(source)
            .map_err(|e| ContentError(format!("{at}: {e}")))?;
        if let Some(grid) = file.grid {
            self.grid = grid;
        }
        self.hoards.extend(file.hoards.into_iter().map(|h| HoardDef { chest: qualify(package, &h.chest), ..h }));
        Ok(())
    }

    /// Checks the grid's numbers, and that each hoard is a locked chest left on real levels,
    /// at a depth and chance that can be.
    pub fn check(&self, registry: &BlockRegistry) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("hoards.ron: {what}")));
        if self.grid.spacing < 64 || self.grid.slots == 0 || self.grid.slots > 4 {
            return fail(format!("the grid needs a spacing of 64 blocks or more and 1 to 4 slots: {:?}", self.grid));
        }
        for hoard in &self.hoards {
            let chest = &hoard.chest;
            let Some(block) = registry.id(chest) else {
                return fail(format!("`{chest}` is no block"));
            };
            if !registry.def(block).has_tag("locked") {
                return fail(format!("`{chest}` is not a locked chest (tag `locked`, treasure.ron)"));
            }
            let (lo, hi) = hoard.levels;
            if lo == 0 || lo > hi || hi > 10 {
                return fail(format!("`{chest}`: levels are 1 ..= 10, least first, not {:?}", hoard.levels));
            }
            if hoard.depth.0 < 1 || hoard.depth.0 > hoard.depth.1 || hoard.depth.1 > 200 || !(2..=24).contains(&hoard.roof) {
                return fail(format!("`{chest}`: its depth is 1 ..= 200 blocks below the surface, least first, and its roof 2 ..= 24 over the floor"));
            }
            if !(hoard.chance > 0.0 && hoard.chance <= 1.0) {
                return fail(format!("`{chest}`: its chance is in (0, 1]"));
            }
        }
        Ok(())
    }
}

/// A hoard resolved against the blocks.
#[derive(Clone, Debug)]
struct Hoard {
    chest: BlockId,
    levels: (u8, u8),
    depth: (i32, i32),
    roof: i32,
    chance: f32,
}

/// One try of the grid that came: where its column is, and what is left there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Try {
    pub x: i32,
    pub z: i32,
    /// Index into the hoards.
    pub hoard: usize,
}

/// The hoards, ready to place.
#[derive(Clone, Debug, Default)]
pub struct Hoards {
    grid: HoardGrid,
    hoards: Vec<Hoard>,
}

impl Hoards {
    pub fn new(def: &HoardsDef, registry: &BlockRegistry) -> Self {
        let hoards = def
            .hoards
            .iter()
            .map(|h| Hoard { chest: registry.expect(&h.chest), levels: h.levels, depth: h.depth, roof: h.roof, chance: h.chance })
            .collect();
        Self { grid: def.grid, hoards }
    }

    /// The chest of hoard `index`.
    pub fn chest(&self, index: usize) -> BlockId {
        self.hoards[index].chest
    }

    /// The tries that came in grid square `square`: a spot each, kept two blocks inside the
    /// square, and the hoard its land's level picked (in file order, by its chance).
    pub fn tries(&self, world: &Overworld, square: IVec2) -> Vec<Try> {
        let spacing = self.grid.spacing;
        let mut out = Vec::new();
        for slot in 0..self.grid.slots {
            let mut rng = Rng::new(world.seed, PURPOSE + slot as u64, square.x, square.y);
            let (x, z) = (square.x * spacing + 2 + rng.below((spacing - 4) as u32), square.y * spacing + 2 + rng.below((spacing - 4) as u32));
            let level = world.level_at(x, z);
            let roll = rng.unit();
            let mut below = 0.0;
            for (hoard, h) in self.hoards.iter().enumerate() {
                if !(h.levels.0..=h.levels.1).contains(&level) {
                    continue;
                }
                below += h.chance;
                if roll < below {
                    out.push(Try { x, z, hoard });
                    break;
                }
            }
        }
        out
    }

    /// The tries whose column is `pos`'s.
    pub fn tries_in(&self, world: &Overworld, pos: ColumnPos) -> Vec<Try> {
        if self.hoards.is_empty() {
            return Vec::new();
        }
        let origin = pos.origin();
        let spacing = self.grid.spacing;
        let square = IVec2::new(origin.x.div_euclid(spacing), origin.z.div_euclid(spacing));
        // A square is 64 blocks or more, as wide as a column or wider, so a column's blocks
        // lie in one square, or two or four across a corner.
        let last = IVec2::new((origin.x + CHUNK_SIZE - 1).div_euclid(spacing), (origin.z + CHUNK_SIZE - 1).div_euclid(spacing));
        let mut out = Vec::new();
        for sx in square.x..=last.x {
            for sz in square.y..=last.y {
                out.extend(self.tries(world, IVec2::new(sx, sz)).into_iter().filter(|t| {
                    (origin.x..origin.x + CHUNK_SIZE).contains(&t.x) && (origin.z..origin.z + CHUNK_SIZE).contains(&t.z)
                }));
            }
        }
        out
    }

    /// The columns of blocks a try looks in for its floor, as (x, z) inside the column at
    /// `origin`: its own spot first, then the others within `SEARCH` blocks of it, nearest
    /// first, never outside the column (so a chest is decided from its column's blocks).
    pub fn spots(t: &Try, origin: IVec2) -> Vec<(i32, i32)> {
        let (x, z) = (t.x - origin.x, t.z - origin.y);
        let mut spots: Vec<(i32, i32)> = (-SEARCH..=SEARCH)
            .flat_map(|dz| (-SEARCH..=SEARCH).map(move |dx| (dx, dz)))
            .filter(|&(dx, dz)| dx * dx + dz * dz <= SEARCH * SEARCH)
            .map(|(dx, dz)| (x + dx, z + dz))
            .filter(|&(x, z)| (0..CHUNK_SIZE).contains(&x) && (0..CHUNK_SIZE).contains(&z))
            .collect();
        spots.sort_by_key(|&(sx, sz)| ((sx - x).pow(2) + (sz - z).pow(2), sz, sx));
        spots
    }

    /// The floor a try's chest stands on in a column of blocks, if it has one: the highest
    /// air with air over it and firm natural ground under it, `depth` below the surface,
    /// roofed by natural rock or earth within `roof` blocks. `at` is asked of no block above
    /// `top`, so a roof is looked for no higher than that.
    pub fn floor(&self, hoard: usize, surface: i32, top: i32, at: impl Fn(i32) -> Floor) -> Option<i32> {
        let h = &self.hoards[hoard];
        let (high, low) = ((surface - h.depth.0).min(top - 1), (surface - h.depth.1).max(2));
        (low..=high).rev().find(|&y| {
            at(y) == Floor::Air
                && at(y + 1) == Floor::Air
                && at(y - 1) == Floor::Ground
                && (y + 2..=(y + h.roof).min(top)).any(|r| at(r) == Floor::Ground)
        })
    }
}

/// What a block in a column is to a hoard looking for a floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Floor {
    /// Open air (no water, no lava, nothing standing there).
    Air,
    /// Firm natural ground, rock or earth: a floor under, a roof over.
    Ground,
    /// Anything else: water, plants, wood, a building.
    Other,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::glam::IVec3;

    use super::*;
    use crate::Generator;

    fn hoards() -> Hoards {
        Hoards { grid: HoardGrid { spacing: 128, slots: 2 }, hoards: vec![Hoard { chest: BlockId(1), levels: (1, 10), depth: (4, 30), roof: 6, chance: 1.0 }] }
    }

    #[test]
    fn a_chest_stands_on_the_highest_roofed_floor_in_its_depth() {
        let hoards = hoards();
        // Surface at 100; rock to 80, a hollow of air 80..=82 with rock over it, rock below.
        let column = |y: i32| if (80..=82).contains(&y) { Floor::Air } else { Floor::Ground };
        assert_eq!(hoards.floor(0, 100, 120, column), Some(80));
        // Open to the sky above: no roof within six blocks, no chest.
        let open = |y: i32| if y >= 80 { Floor::Air } else { Floor::Ground };
        assert_eq!(hoards.floor(0, 100, 120, open), None);
        // A hollow of water is no floor; nor one only a block high.
        let wet = |y: i32| if (80..=82).contains(&y) { Floor::Other } else { Floor::Ground };
        assert_eq!(hoards.floor(0, 100, 120, wet), None);
        let low = |y: i32| if y == 80 { Floor::Air } else { Floor::Ground };
        assert_eq!(hoards.floor(0, 100, 120, low), None);
        // Too shallow (2 below the surface) or too deep (40 below) for its depth.
        let shallow = |y: i32| if (98..=99).contains(&y) { Floor::Air } else { Floor::Ground };
        assert_eq!(hoards.floor(0, 100, 120, shallow), None);
        let deep = |y: i32| if (60..=62).contains(&y) { Floor::Air } else { Floor::Ground };
        assert_eq!(hoards.floor(0, 100, 120, deep), None);
        // Land up to the top of the world: nothing above `top` is asked about. Open to the
        // top, no roof and no chest; rock at the top, a roof.
        let open_top = |y: i32| {
            assert!(y <= 120, "asked about {y}, over the top");
            if y >= 116 { Floor::Air } else { Floor::Ground }
        };
        assert_eq!(hoards.floor(0, 120, 120, open_top), None);
        let roofed_top = |y: i32| {
            assert!(y <= 120, "asked about {y}, over the top");
            if (116..=119).contains(&y) { Floor::Air } else { Floor::Ground }
        };
        assert_eq!(hoards.floor(0, 120, 120, roofed_top), Some(116));
    }

    /// The game's hoards over the land near where a world begins, made column by column on
    /// two seeds: every chest is one of the hoards' chests, on firm ground with headroom and
    /// rock over it, never in the open; at most one a column; Nomad chests outnumber Farm
    /// ones and both are found; and the land stays uncrowded.
    #[test]
    fn chests_lie_under_rock_rarer_further_out_and_far_apart() {
        let registry = Arc::new(crate::test_registry());
        let ground = |b: BlockId| crate::terrain::earth(&registry, b);
        for seed in [0x5eed, 1] {
            let world = Overworld::new(seed, registry.clone());
            let hoards = &world.terrain.hoards;
            let spawn = world.spawn_point();
            let spacing = hoards.grid.spacing;
            let centre = IVec2::new(spawn.x.div_euclid(spacing), spawn.z.div_euclid(spacing));
            let mut columns = std::collections::BTreeMap::new();
            for sz in -5..5 {
                for sx in -5..5 {
                    let came = hoards.tries(&world, centre + IVec2::new(sx, sz));
                    assert!(came.len() <= hoards.grid.slots as usize);
                    for t in came {
                        let pos = ColumnPos::of_block(IVec3::new(t.x, 0, t.z));
                        columns.entry((pos.origin().x, pos.origin().z)).or_insert(pos);
                    }
                }
            }
            let (mut nomad, mut farm) = (0u32, 0u32);
            // The columns are made side by side, as the game makes them.
            use rayon::prelude::*;
            let made: Vec<_> = columns.into_values().collect::<Vec<_>>().into_par_iter().map(|pos| (pos, world.generate(pos))).collect();
            for (pos, column) in made {
                let mut here = Vec::new();
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        for y in 2..voxmar_engine::coords::WORLD_HEIGHT - 2 {
                            let at = |dy: i32| column.block(IVec3::new(x, y + dy, z));
                            let Some(h) = hoards.hoards.iter().find(|h| h.chest == at(0)) else { continue };
                            assert!(ground(at(-1)) && at(1).is_air(), "seed {seed}: a chest at {x} {y} {z} of {pos:?} stands on ground with room over it");
                            assert!((2..=h.roof).any(|dy| ground(at(dy))), "seed {seed}: a chest at {x} {y} {z} of {pos:?} has rock over it");
                            here.push(registry.def(at(0)).name.clone());
                        }
                    }
                }
                assert!(here.len() <= 1, "seed {seed}: one chest a column, not {here:?} in {pos:?}");
                for name in here {
                    if name.ends_with("nomad_treasure_chest") {
                        nomad += 1;
                    } else if name.ends_with("farm_treasure_chest") {
                        farm += 1;
                    }
                }
            }
            println!("seed {seed}: {nomad} Nomad chests, {farm} Farm chests in a hundred squares");
            assert!(farm > 0 && nomad > farm, "seed {seed}: Nomad chests ({nomad}) outnumber Farm ones ({farm}), and both are found");
            // A hundred squares of 128 blocks: about 1.3 km a side, a chest to every three or
            // more squares.
            assert!(nomad + farm <= 40, "seed {seed}: {} chests in a hundred squares", nomad + farm);
        }
    }
}
