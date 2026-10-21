//! Multiblocks: structures recognised from a pattern of blocks in the world, as content
//! (each package's `multiblocks.ron`). A pattern is layers from the bottom up, each rows of
//! characters; a key says what each character takes (an item name, `tag:TAG`, or `a|b`),
//! `.` takes anything and ` ` only air. One character is the core, the block a player uses
//! to bring the structure to life; the pattern may stand turned any of four ways.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError, WARD_LEAST, WARD_MOST};
use crate::glam::IVec3;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiblockDef {
    pub name: String,
    /// What it is called, and what it needs, in English (language keys
    /// `multiblock.<package>.<name>` and `.hint`).
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub hint: String,
    /// Lit, it becomes the player's home, where they wake after dying.
    #[serde(default)]
    pub home: bool,
    /// Lit, how many blocks round its core its fire keeps the wild back at night (as a
    /// campfire's `BlockDef::ward`). 0 for none, else 2 to 24.
    #[serde(default)]
    pub ward: f32,
    /// Alternative fuel stacks consumed to light it. Empty means it needs no fuel.
    #[serde(default)]
    pub fuels: Vec<(String, u8)>,
    /// Bottom layer first; in each, rows run north to south and characters west to east.
    pub layers: Vec<Vec<String>>,
    pub key: FxHashMap<char, String>,
    pub core: char,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MultiblockFile {
    #[serde(default)]
    pub multiblocks: Vec<MultiblockDef>,
}

/// What a cell takes.
#[derive(Clone, Debug, PartialEq)]
enum Cell {
    Any,
    Air,
    /// Any of these, sorted.
    OneOf(Vec<BlockId>),
}

/// A multiblock with its key resolved.
#[derive(Clone, Debug)]
pub struct Multiblock {
    pub name: String,
    pub title: String,
    pub hint: String,
    pub home: bool,
    pub ward: f32,
    pub fuels: Vec<(BlockId, u8)>,
    /// What the core takes.
    pub core: Vec<BlockId>,
    /// Each cell's offset from the core (unturned) and what it takes.
    cells: Vec<(IVec3, Cell)>,
}

impl Multiblock {
    /// Could changing this cell break any turn of the structure round this core?
    pub fn includes(&self, core: IVec3, changed: IVec3) -> bool {
        let d = changed - core;
        self.cells.iter().any(|(at, _)| {
            let mut o = *at;
            (0..4).any(|_| {
                let same = o == d;
                o = IVec3::new(-o.z, o.y, o.x);
                same
            })
        })
    }
    /// What it is built of: each set of blocks a cell takes, with how many cells take it,
    /// the core's among them.
    pub fn pieces(&self) -> Vec<(&[BlockId], u32)> {
        let mut pieces: Vec<(&[BlockId], u32)> = Vec::new();
        for (_, cell) in &self.cells {
            if let Cell::OneOf(accepts) = cell {
                match pieces.iter_mut().find(|(takes, _)| *takes == accepts.as_slice()) {
                    Some((_, n)) => *n += 1,
                    None => pieces.push((accepts, 1)),
                }
            }
        }
        pieces
    }

    /// How far from its core, across, its farthest cell lies (any way it is turned).
    pub fn reach(&self) -> i32 {
        self.cells.iter().map(|(o, _)| o.x.abs().max(o.z.abs())).max().unwrap_or(0)
    }

    /// Whether the structure stands with its core at `core`, turned any way; `block` says
    /// what is where (none for a place not loaded, which never matches).
    pub fn stands(&self, core: IVec3, block: impl Fn(IVec3) -> Option<BlockId>) -> bool {
        (0..4).any(|turns| {
            self.cells.iter().all(|(offset, cell)| {
                let mut o = *offset;
                for _ in 0..turns {
                    o = IVec3::new(-o.z, o.y, o.x);
                }
                match (cell, block(core + o)) {
                    (_, None) => false,
                    (Cell::Any, _) => true,
                    (Cell::Air, Some(b)) => b.is_air(),
                    (Cell::OneOf(accepts), Some(b)) => accepts.binary_search(&b).is_ok(),
                }
            })
        })
    }
}

#[derive(Debug, Default)]
pub struct MultiblockRegistry {
    multiblocks: Vec<Multiblock>,
}

impl MultiblockRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<MultiblockFile, RegistryError> {
        let mut file: MultiblockFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/multiblocks.ron: {e}")))?;
        for def in &mut file.multiblocks {
            if !def.name.contains(':') {
                def.name = format!("{namespace}:{}", def.name);
            }
            for name in def.key.values_mut() {
                if !name.starts_with("tag:") {
                    *name = name.split('|').map(|n| if n.contains(':') { n.to_owned() } else { format!("{namespace}:{n}") }).collect::<Vec<_>>().join("|");
                }
            }
            for (name, _) in &mut def.fuels {
                if !name.contains(':') {
                    *name = format!("{namespace}:{name}");
                }
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<MultiblockFile>, blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut multiblocks = Vec::new();
        for def in files.into_iter().flat_map(|f| f.multiblocks) {
            let bad = |what: String| RegistryError(format!("multiblock {}: {what}", def.name));
            let mut key: FxHashMap<char, Cell> = FxHashMap::default();
            for (&c, name) in &def.key {
                let mut accepts: Vec<BlockId> = match name.strip_prefix("tag:") {
                    // A ring of stone is of whole blocks: no cut stone stands in for one.
                    Some(tag) => blocks.answers_whole(tag),
                    None => name.split('|').map(|n| blocks.id(n).ok_or_else(|| bad(format!("no block `{n}`")))).collect::<Result<_, _>>()?,
                };
                if accepts.is_empty() {
                    return Err(bad(format!("nothing has the tag in `{name}`")));
                }
                accepts.sort();
                key.insert(c, Cell::OneOf(accepts));
            }
            let mut cells = Vec::new();
            let mut core = None;
            for (y, layer) in def.layers.iter().enumerate() {
                for (z, row) in layer.iter().enumerate() {
                    for (x, c) in row.chars().enumerate() {
                        let at = IVec3::new(x as i32, y as i32, z as i32);
                        let cell = match c {
                            '.' => Cell::Any,
                            ' ' => Cell::Air,
                            c => key.get(&c).cloned().ok_or_else(|| bad(format!("`{c}` is not in the key")))?,
                        };
                        if c == def.core {
                            if core.is_some() {
                                return Err(bad("more than one core".into()));
                            }
                            core = Some(at);
                        }
                        cells.push((at, cell));
                    }
                }
            }
            let core = core.ok_or_else(|| bad(format!("no core `{}` in the layers", def.core)))?;
            let core_takes = match key.get(&def.core) {
                Some(Cell::OneOf(accepts)) => accepts.clone(),
                _ => Vec::new(),
            };
            // A core is known to need its structure by its tag (a block without it works alone).
            if let Some(&loose) = core_takes.iter().find(|&&b| !blocks.def(b).has_tag("multiblock_core")) {
                return Err(bad(format!("its core `{}` is not tagged `multiblock_core`, so it would work without the structure", blocks.def(loose).name)));
            }
            let cells = cells.into_iter().filter(|(_, cell)| *cell != Cell::Any).map(|(at, cell)| (at - core, cell)).collect();
            let fuels = def.fuels.iter().map(|(name, count)| {
                if *count == 0 {
                    return Err(bad("a fuel stack must hold at least one item".into()));
                }
                let item = blocks.id(name).ok_or_else(|| bad(format!("no fuel `{name}`")))?;
                if *count > crate::inventory::MAX_STACK {
                    return Err(bad(format!("{count} of `{name}` does not fit in a stack")));
                }
                Ok((item, *count))
            }).collect::<Result<Vec<_>, RegistryError>>()?;
            if def.ward != 0.0 && !(WARD_LEAST..=WARD_MOST).contains(&def.ward) {
                return Err(bad(format!("it wards {} blocks round it: 0 for none, else {WARD_LEAST} to {WARD_MOST}", def.ward)));
            }
            multiblocks.push(Multiblock { name: def.name, title: def.title, hint: def.hint, home: def.home, ward: def.ward, fuels, core: core_takes, cells });
        }
        Ok(Self { multiblocks })
    }

    pub fn get(&self, name: &str) -> Option<&Multiblock> {
        self.multiblocks.iter().find(|m| m.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Multiblock> {
        self.multiblocks.iter()
    }

    /// How far from its core any structure's farthest cell lies, on any one axis: a change
    /// farther than this from a core can break nothing built round it.
    pub fn reach(&self) -> i32 {
        self.multiblocks.iter().flat_map(|m| m.cells.iter()).map(|(at, _)| at.abs().max_element()).max().unwrap_or(0)
    }

    /// The multiblocks a block can be the core of.
    pub fn with_core(&self, block: BlockId) -> impl Iterator<Item = &Multiblock> {
        self.multiblocks.iter().filter(move |m| m.core.binary_search(&block).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    #[test]
    fn lighting_fuel_names_and_stack_sizes_are_checked_on_load() {
        let core = BlockDef { name: "t:core".into(), tags: vec!["multiblock_core".into()], ..Default::default() };
        let fuel = BlockDef { name: "t:fuel".into(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![core, fuel]).unwrap();
        let load = |name: &str, count: u8| {
            let source = format!("(multiblocks: [(name: \"kiln\", layers: [[\"C\"]], key: {{'C': \"core\"}}, core: 'C', fuels: [(\"{name}\", {count})])])");
            MultiblockRegistry::new(vec![MultiblockRegistry::read("t", &source).unwrap()], &blocks)
        };
        assert!(load("fuel", 32).is_ok());
        assert!(load("missing", 32).is_err());
        assert!(load("fuel", 0).is_err());
        assert!(load("fuel", crate::inventory::MAX_STACK + 1).is_err());
    }

    /// A lit fire's ward (`ward`), a multiblock's or a block's, is none or 2 to 24 blocks.
    #[test]
    fn wards_out_of_range_are_refused() {
        let core = BlockDef { name: "t:core".into(), tags: vec!["multiblock_core".into()], ..Default::default() };
        let blocks = BlockRegistry::new(vec![core.clone()]).unwrap();
        let load = |ward: f32| {
            let source = format!("(multiblocks: [(name: \"hearth\", ward: {ward:?}, layers: [[\"C\"]], key: {{'C': \"core\"}}, core: 'C')])");
            MultiblockRegistry::new(vec![MultiblockRegistry::read("t", &source).unwrap()], &blocks).map(|m| m.multiblocks[0].ward).map_err(|e| e.to_string())
        };
        let fire = |ward: f32| BlockRegistry::new(vec![BlockDef { name: "t:fire".into(), ward, ..Default::default() }]).map(|_| ()).map_err(|e| e.to_string());
        for ward in [0.0, 2.0, 16.0, 24.0] {
            assert_eq!(load(ward), Ok(ward));
            assert_eq!(fire(ward), Ok(()));
        }
        for ward in [-1.0, 1.0, 25.0] {
            assert!(load(ward).is_err_and(|e| e.contains("2 to 24")), "{ward}");
            assert!(fire(ward).is_err_and(|e| e.contains("2 to 24")), "{ward}");
        }
    }

    #[test]
    fn a_hearth_stands_turned_any_way_and_not_with_a_piece_missing() {
        let tagged = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![tagged("t:cobble", &["stone"]), tagged("t:brick", &["stone"]), tagged("t:hearthstone", &["multiblock_core"]), tagged("t:chimney", &[])]).unwrap();
        let source = r#"(multiblocks: [(name: "hearth", layers: [["sss", "sHs", "sss"], ["C..", "...", "..."], ["C..", "...", "..."]],
            key: {'s': "tag:stone", 'H': "hearthstone", 'C': "chimney"}, core: 'H')])"#;
        let registry = MultiblockRegistry::new(vec![MultiblockRegistry::read("t", source).unwrap()], &blocks).unwrap();
        // A core without the tag would work with no structure round it: refused.
        let loose = source.replace("'H': \"hearthstone\"", "'H': \"chimney\"").replace("'C': \"chimney\"", "'C': \"hearthstone\"");
        assert!(MultiblockRegistry::new(vec![MultiblockRegistry::read("t", &loose).unwrap()], &blocks).is_err());
        let hearth = registry.get("t:hearth").unwrap();
        let id = |n: &str| blocks.id(&format!("t:{n}")).unwrap();
        // Built with its chimney at the south-east corner: the pattern turned twice.
        let mut world: FxHashMap<IVec3, BlockId> = FxHashMap::default();
        for x in -1..=1 {
            for z in -1..=1 {
                world.insert(IVec3::new(x, 0, z), if (x + z) % 2 == 0 { id("cobble") } else { id("brick") });
            }
        }
        world.insert(IVec3::ZERO, id("hearthstone"));
        world.insert(IVec3::new(1, 1, 1), id("chimney"));
        world.insert(IVec3::new(1, 2, 1), id("chimney"));
        let look = |world: &FxHashMap<IVec3, BlockId>| hearth.stands(IVec3::ZERO, |p| Some(world.get(&p).copied().unwrap_or(BlockId::AIR)));
        assert!(look(&world));
        world.remove(&IVec3::new(1, 2, 1));
        assert!(!look(&world), "the chimney is two blocks tall");
        world.insert(IVec3::new(1, 2, 1), id("chimney"));
        world.insert(IVec3::new(-1, 0, 0), id("chimney"));
        assert!(!look(&world), "the ring is stone");
    }
}
