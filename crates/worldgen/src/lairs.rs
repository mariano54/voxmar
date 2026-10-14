//! Lairs: a boss's room, dug under the land of its biomes (`lairs.ron`, docs/bosses.md).
//!
//! The world is cut into squares (`grid`); in a share of them (`chance`) the lair tries a
//! few spots well inside the square (`tries`) and takes the first that lies deep in one of
//! its biomes, on dry, gentle ground. There it hollows a dome of `radius` and `height`
//! with its floor `depth` under the ground, lines it with its `wall` (unbreakable, so the
//! fight keeps to its room), lays its `floor` (the block boss music knows the room by),
//! and digs a sloping passage up to the surface the way the spot's hash points.
//!
//! Like settlements, a lair is a pure function of the seed and the land, planned by square
//! on demand and cached, and each column builds its own part of it, so nothing depends on
//! which column or thread came first. Gameplay finds lairs with [`Lairs::near`]; the boss
//! itself is the server's to put in it.

use std::sync::Mutex;

use voxmar_engine::glam::{IVec2, IVec3};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, WORLD_HEIGHT};

use crate::Overworld;

/// A kind of lair, as `lairs.ron` writes it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LairDef {
    pub name: String,
    /// The creature that lives in it (the server puts it there).
    pub boss: String,
    /// The biomes it is dug under, by name; the spot must lie at least `inside` blocks
    /// from their edge.
    pub biomes: Vec<String>,
    #[serde(default = "inside")]
    pub inside: f32,
    pub grid: Grid,
    /// Spots tried in a square before giving it up.
    #[serde(default = "tries")]
    pub tries: u32,
    /// The dome's half-width and height inside, and how far under the ground its floor
    /// lies.
    pub radius: i32,
    pub height: i32,
    pub depth: i32,
    pub wall: String,
    pub floor: String,
    /// Blocks around its floor's centre where no wild creature is born.
    #[serde(default)]
    pub quiet: f32,
    /// Days of the game's clock a lair stands empty after its boss falls.
    #[serde(default = "rest")]
    pub rest: f32,
    /// How near a player comes before its boss is there, in blocks.
    #[serde(default = "wakes")]
    pub wakes: f32,
}

fn rest() -> f32 {
    1.0
}

fn wakes() -> f32 {
    48.0
}

fn inside() -> f32 {
    24.0
}

fn tries() -> u32 {
    6
}

/// The squares lairs are planned in.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub spacing: i32,
    pub chance: f32,
    /// Blocks from the square's edge a lair's centre keeps.
    pub margin: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LairFile {
    lairs: Vec<LairDef>,
}

/// Reads a package's `lairs.ron`, qualifying names with its namespace.
pub(crate) fn read(at: &str, package: &str, source: &str) -> Result<Vec<LairDef>, crate::content::ContentError> {
    let file: LairFile = ron::from_str(source).map_err(|e| crate::content::ContentError(format!("{at}: {e}")))?;
    let q = |name: &str| if name.contains(':') { name.to_owned() } else { format!("{package}:{name}") };
    Ok(file
        .lairs
        .into_iter()
        .map(|l| LairDef { name: q(&l.name), boss: q(&l.boss), biomes: l.biomes.iter().map(|b| q(b)).collect(), wall: q(&l.wall), floor: q(&l.floor), ..l })
        .collect())
}

impl LairDef {
    /// Checks a lair's numbers, blocks and biomes.
    pub(crate) fn check(&self, registry: &BlockRegistry, biome: impl Fn(&str) -> bool) -> Result<(), String> {
        let at = format!("lairs.ron: lair `{}`", self.name);
        for name in [&self.wall, &self.floor] {
            if registry.id(name).is_none() {
                return Err(format!("{at}: unknown block `{name}`"));
            }
        }
        if self.biomes.is_empty() {
            return Err(format!("{at}: names no biome"));
        }
        if let Some(name) = self.biomes.iter().find(|b| !biome(b)) {
            return Err(format!("{at}: unknown biome `{name}`"));
        }
        let g = self.grid;
        if !(g.spacing >= 64 && g.margin >= 0 && g.margin * 2 < g.spacing && g.chance > 0.0 && g.chance <= 1.0) {
            return Err(format!("{at}: the grid needs a spacing of 64 or more, a margin under half of it, and a chance in (0, 1]"));
        }
        if !(self.radius >= 3 && self.height >= 3 && self.depth > self.height + 1 && self.tries >= 1) {
            return Err(format!("{at}: needs a radius and height of 3 or more, a depth below its height and roof, and a try"));
        }
        if self.radius + self.reach() > g.margin {
            return Err(format!("{at}: its room and passage reach {} blocks, past its margin {}", self.radius + self.reach(), g.margin));
        }
        if !(self.inside >= 0.0 && self.quiet >= 0.0 && self.rest >= 0.0 && self.wakes > 0.0) {
            return Err(format!("{at}: `inside`, `quiet` and `rest` must be 0 or more, and `wakes` more than 0"));
        }
        Ok(())
    }

    /// How far the passage runs past the dome's edge: up one block for every two along,
    /// with room to come out on the ground.
    fn reach(&self) -> i32 {
        self.depth * 2 + 6
    }
}

/// A lair dug at a spot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LairSite {
    /// Which lair (its index in `lairs.ron`'s list, all packages').
    pub lair: usize,
    /// The middle of its floor: the block a boss stands on top of is one below.
    pub heart: IVec3,
    /// The way its passage leaves, a unit step on the ground.
    pub out: IVec2,
}

/// Every kind of lair, and the squares planned so far.
pub struct Lairs {
    defs: Vec<LairDef>,
    biomes: Vec<Vec<String>>,
    blocks: Vec<(BlockId, BlockId)>,
    seed: u64,
    planned: Mutex<FxHashMap<(usize, IVec2), Option<LairSite>>>,
}

impl Lairs {
    pub(crate) fn new(seed: u64, defs: &[LairDef], registry: &BlockRegistry) -> Self {
        Self {
            blocks: defs.iter().map(|d| (registry.expect(&d.wall), registry.expect(&d.floor))).collect(),
            biomes: defs.iter().map(|d| d.biomes.clone()).collect(),
            defs: defs.to_vec(),
            seed,
            planned: Mutex::new(FxHashMap::default()),
        }
    }

    pub fn defs(&self) -> &[LairDef] {
        &self.defs
    }

    /// The lair of kind `lair` in square `cell`, if it has one.
    fn site(&self, world: &Overworld, lair: usize, cell: IVec2) -> Option<LairSite> {
        if let Some(found) = self.planned.lock().expect("lair plans").get(&(lair, cell)) {
            return *found;
        }
        let found = self.plan(world, lair, cell);
        self.planned.lock().expect("lair plans").insert((lair, cell), found);
        found
    }

    fn plan(&self, world: &Overworld, lair: usize, cell: IVec2) -> Option<LairSite> {
        let def = &self.defs[lair];
        let g = def.grid;
        let salt = crate::rng::hash3(self.seed ^ 0x1a12, cell.x, cell.y, lair as i32);
        if (salt >> 11) as f64 / (1u64 << 53) as f64 >= g.chance as f64 {
            return None;
        }
        let span = (g.spacing - g.margin * 2).max(1) as u64;
        (0..def.tries).find_map(|t| {
            let h = crate::rng::hash3(salt, t as i32, 0x5eed, 7);
            let at = cell * g.spacing + IVec2::splat(g.margin) + IVec2::new((h % span) as i32, ((h >> 24) % span) as i32);
            let site = world.site(at.x, at.y);
            let biome = &world.biomes().get(site.home).name;
            let fits = self.biomes[lair].iter().any(|b| b == biome)
                && site.edge >= def.inside
                && site.height > site.water
                && site.river <= 0.0
                && site.steepness < 0.6
                && site.height - def.depth > 8;
            if !fits || world.settlement_covers(at.x, at.y) {
                return None;
            }
            const OUT: [IVec2; 4] = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y];
            Some(LairSite { lair, heart: IVec3::new(at.x, site.height - def.depth, at.y), out: OUT[(h >> 52) as usize % 4] })
        })
    }

    /// The squares of kind `lair` whose lairs could reach the box from `min` to `max`.
    fn cells(&self, lair: usize, min: IVec2, max: IVec2) -> impl Iterator<Item = IVec2> {
        let s = self.defs[lair].grid.spacing;
        let (lo, hi) = (IVec2::new(min.x.div_euclid(s), min.y.div_euclid(s)), IVec2::new(max.x.div_euclid(s), max.y.div_euclid(s)));
        (lo.y..=hi.y).flat_map(move |z| (lo.x..=hi.x).map(move |x| IVec2::new(x, z)))
    }

    /// The lairs within `radius` blocks of `centre` (flat), nearest first.
    pub fn near(&self, world: &Overworld, centre: IVec2, radius: i32) -> Vec<LairSite> {
        let mut found: Vec<LairSite> = (0..self.defs.len())
            .flat_map(|lair| self.cells(lair, centre - radius, centre + radius).filter_map(move |cell| self.site(world, lair, cell)))
            .filter(|s| (IVec2::new(s.heart.x, s.heart.z) - centre).as_vec2().length() <= radius as f32)
            .collect();
        found.sort_by_key(|s| (IVec2::new(s.heart.x, s.heart.z) - centre).length_squared());
        found
    }

    /// Digs the parts of every lair that reach the column at `origin` into `blocks`, with
    /// `ground` the height of the ground at a position (inside the column) and `plant`
    /// whether a block is one that grows on it.
    pub(crate) fn build(&self, world: &Overworld, origin: IVec3, blocks: &mut crate::terrain::Blocks, ground: &dyn Fn(i32, i32) -> i32, plant: &dyn Fn(BlockId) -> bool) {
        let corner = IVec2::new(origin.x, origin.z);
        for lair in 0..self.defs.len() {
            let def = &self.defs[lair];
            let reach = def.radius + def.reach() + 1;
            for cell in self.cells(lair, corner - reach, corner + CHUNK_SIZE - 1 + reach) {
                if let Some(site) = self.site(world, lair, cell) {
                    self.dig(def, self.blocks[lair], site, origin, blocks, ground, plant);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dig(&self, def: &LairDef, (wall, floor): (BlockId, BlockId), site: LairSite, origin: IVec3, blocks: &mut crate::terrain::Blocks, ground: &dyn Fn(i32, i32) -> i32, plant: &dyn Fn(BlockId) -> bool) {
        let (r, h) = (def.radius as f32, def.height as f32);
        let base = site.heart.y;
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let (x, z) = (origin.x + lx, origin.z + lz);
                let (dx, dz) = ((x - site.heart.x) as f32, (z - site.heart.z) as f32);
                let flat = (dx * dx + dz * dz).sqrt();
                let top = ground(x, z);
                // The dome: hollow inside, its floor laid, lined with its wall below the
                // ground (never over it: a hill's flank keeps its own face).
                if flat <= r + 1.5 {
                    for y in (base - 1).max(0)..=(base + def.height + 1).min(WORLD_HEIGHT - 1) {
                        let up = (y - base) as f32 + 0.5;
                        let inner = y >= base && (flat / r).powi(2) + (up / h).powi(2) < 1.0;
                        let shell = y == base - 1 || (flat / (r + 1.0)).powi(2) + (up / (h + 1.0)).powi(2) < 1.0;
                        if inner {
                            blocks.set_local(lx, y, lz, BlockId::AIR);
                        } else if y == base - 1 && flat <= r + 0.5 {
                            blocks.set_local(lx, y, lz, floor);
                        } else if shell && y < top {
                            blocks.set_local(lx, y, lz, wall);
                        }
                    }
                }
                // The passage: three wide and three high, up a block for every two along,
                // out of the dome's side to the ground.
                let along = dx * site.out.x as f32 + dz * site.out.y as f32;
                let side = (dx * site.out.y as f32 - dz * site.out.x as f32).abs();
                if side > 1.0 || along < r - 1.0 {
                    continue;
                }
                let low = base + ((along - r + 1.0) * 0.5).floor() as i32;
                if low > top {
                    continue;
                }
                for y in low.max(0)..(low + 3).min(WORLD_HEIGHT) {
                    blocks.set_local(lx, y, lz, BlockId::AIR);
                }
                // Where it comes out, the ground over it opens, and what grew there goes.
                if low + 3 > top {
                    for y in (low + 3).max(0)..(top + 3).min(WORLD_HEIGHT) {
                        let block = blocks.get_local(lx, y, lz);
                        if y <= top || plant(block) {
                            blocks.set_local(lx, y, lz, BlockId::AIR);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use voxmar_engine::content::Content;
    use voxmar_engine::coords::ColumnPos;

    use super::*;
    use crate::{Generator, WorldContent};

    fn dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")
    }

    /// A Rootmother's hollow: found under one of its biomes, its floor of hollow floor
    /// with room to stand over it, lined with root no tool cuts, and a passage up that
    /// comes out on the ground.
    #[test]
    fn a_lair_is_a_room_of_unbreakable_root_under_its_biome_with_a_way_in() {
        let registry = Arc::new(crate::test_registry());
        let world = crate::Overworld::new(1, registry.clone());
        let site = world.lairs_near(IVec2::ZERO, 2000).into_iter().next().expect("a lair within 2 km");
        let def = &world.lair_defs()[site.lair];
        let home = world.biomes().get(world.site(site.heart.x, site.heart.z).home).name.clone();
        assert!(def.biomes.contains(&home), "a lair in {home}");
        let (wall, floor) = (registry.expect(&def.wall), registry.expect(&def.floor));
        assert!(registry.def(wall).hardness < 0.0 && registry.def(floor).hardness < 0.0, "nothing cuts a lair's room");
        let column = world.generate(ColumnPos::of_block(site.heart));
        let at = |p: IVec3| column.block(p - ColumnPos::of_block(site.heart).origin());
        assert_eq!(at(site.heart - IVec3::Y), floor);
        for up in 0..3 {
            assert!(at(site.heart + IVec3::Y * up).is_air(), "room to stand over the floor's middle");
        }
        let roof = (1..=def.height + 2).map(|up| at(site.heart + IVec3::Y * up)).find(|b| !b.is_air());
        assert_eq!(roof, Some(wall), "lined with root overhead");
        // Along the passage, somewhere it comes out where the sky is open.
        let out = IVec3::new(site.out.x, 0, site.out.y);
        let mouth = (def.radius..def.radius + def.reach()).find(|&d| {
            let p = site.heart + out * d;
            let column = world.generate(ColumnPos::of_block(p));
            let local = |q: IVec3| column.block(q - ColumnPos::of_block(p).origin());
            let ground = world.site(p.x, p.z).height;
            (ground - 1..ground + 6).all(|y| local(IVec3::new(p.x, y, p.z)).is_air() || registry.def(local(IVec3::new(p.x, y, p.z))).render == voxmar_engine::block::RenderKind::Plant)
        });
        assert!(mouth.is_some(), "the passage comes out on the ground");
    }

    /// Lairs are rare: a few in the squares around spawn of a world with old woods near,
    /// never two of a kind closer than a square's margins allow.
    #[test]
    fn lairs_are_few_and_far_between() {
        let world = crate::Overworld::new(1, Arc::new(crate::test_registry()));
        let found = world.lairs_near(IVec2::ZERO, 3000);
        assert!(!found.is_empty() && found.len() <= 40, "{} lairs within 3 km", found.len());
        for (i, a) in found.iter().enumerate() {
            for b in &found[i + 1..] {
                assert!((a.heart - b.heart).as_vec3().length() > (world.lair_defs()[a.lair].grid.margin * 2) as f32);
            }
        }
    }

    /// The Rootmother quest says where her hollows lie: it names every biome they are dug
    /// under, by the biome's own English name, so the hint stays true to the world data.
    #[test]
    fn the_rootmother_quest_names_every_biome_her_hollows_lie_under() {
        let content = Content::load(&dir()).unwrap();
        let world = WorldContent::load(&content).unwrap();
        let english: HashMap<String, String> = ron::from_str(&std::fs::read_to_string(dir().join("base/lang/en.ron")).unwrap()).unwrap();
        let hint = &english["quest.base.the_rootmother.why"];
        let lair = world.lairs.iter().find(|l| l.boss == "base:rootmother").expect("her lair");
        for biome in &lair.biomes {
            let name = &english[&format!("biome.{}", biome.replacen(':', ".", 1))];
            assert!(hint.contains(name.as_str()), "her hollows lie under the {name}, which the quest does not name");
        }
    }

    /// Every boss that never comes from the land's own population lives in some lair.
    #[test]
    fn every_boss_has_a_lair() {
        let content = Content::load(&dir()).unwrap();
        let world = WorldContent::load(&content).unwrap();
        for (_, creature) in content.creatures.iter().filter(|(_, c)| c.def.boss && c.def.habitat.party_only) {
            assert!(world.lairs.iter().any(|l| l.boss == creature.def.name), "{} has no lair", creature.def.name);
        }
    }

    #[test]
    fn lairs_are_checked() {
        let registry = crate::test_registry();
        let good = LairDef {
            name: "base:test".into(),
            boss: "base:rootmother".into(),
            biomes: vec!["base:dark_forest".into()],
            inside: 24.0,
            grid: Grid { spacing: 768, chance: 0.5, margin: 64 },
            tries: 4,
            radius: 7,
            height: 5,
            depth: 10,
            wall: "base:hollow_root".into(),
            floor: "base:hollow_floor".into(),
            quiet: 32.0,
            rest: 1.0,
            wakes: 48.0,
        };
        let biome = |name: &str| name == "base:dark_forest";
        assert_eq!(good.check(&registry, biome), Ok(()));
        assert!(LairDef { wall: "base:nothing".into(), ..good.clone() }.check(&registry, biome).unwrap_err().contains("unknown block"));
        assert!(LairDef { biomes: vec!["base:moon".into()], ..good.clone() }.check(&registry, biome).unwrap_err().contains("unknown biome"));
        assert!(LairDef { depth: 5, ..good.clone() }.check(&registry, biome).unwrap_err().contains("depth"));
        assert!(LairDef { grid: Grid { margin: 20, ..good.grid }, ..good.clone() }.check(&registry, biome).unwrap_err().contains("margin"));
    }
}

#[cfg(test)]
mod census {
    use std::sync::Arc;

    use super::*;

    /// How many lairs lie near spawn, and how many of their biomes' land there is: a few
    /// seeds, counted. Slow; run by hand (`--ignored --nocapture`) after changing a lair.
    #[test]
    #[ignore]
    fn lair_census() {
        for seed in [0u64, 1, 2, 3, 4, 5, 0x5eed] {
            let world = crate::Overworld::new(seed, Arc::new(crate::test_registry()));
            let found = world.lairs_near(IVec2::ZERO, 4000);
            let nearest = found.first().map(|s| IVec2::new(s.heart.x, s.heart.z).as_vec2().length());
            let mut land = 0;
            for x in (-4000..4000).step_by(256) {
                for z in (-4000..4000).step_by(256) {
                    let site = world.site(x, z);
                    let name = &world.biomes().get(site.home).name;
                    if world.lair_defs()[0].biomes.contains(name) {
                        land += 1;
                    }
                }
            }
            println!("seed {seed}: {} lairs within 4 km, nearest {nearest:?}; their biomes at {land} of 961 probes", found.len());
        }
    }
}
