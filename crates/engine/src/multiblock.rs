//! Multiblocks: structures recognised from a pattern of blocks in the world, as content
//! (each package's `multiblocks.ron`). A pattern is layers from the bottom up, each rows of
//! characters; a key says what each character takes (an item name, `tag:TAG`, or `a|b`),
//! `.` takes anything and ` ` only air. One character is the core, the block a player uses
//! to bring the structure to life; the pattern may stand turned any of four ways.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
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
    /// What the core takes.
    pub core: Vec<BlockId>,
    /// Each cell's offset from the core (unturned) and what it takes.
    cells: Vec<(IVec3, Cell)>,
}

impl Multiblock {
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
                    Some(tag) => (0..blocks.len() as u16).map(BlockId).filter(|&id| blocks.def(id).has_tag(tag)).collect(),
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
            let cells = cells.into_iter().filter(|(_, cell)| *cell != Cell::Any).map(|(at, cell)| (at - core, cell)).collect();
            multiblocks.push(Multiblock { name: def.name, title: def.title, hint: def.hint, home: def.home, core: core_takes, cells });
        }
        Ok(Self { multiblocks })
    }

    pub fn get(&self, name: &str) -> Option<&Multiblock> {
        self.multiblocks.iter().find(|m| m.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Multiblock> {
        self.multiblocks.iter()
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
    fn a_hearth_stands_turned_any_way_and_not_with_a_piece_missing() {
        let tagged = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![tagged("t:cobble", &["stone"]), tagged("t:brick", &["stone"]), tagged("t:hearthstone", &[]), tagged("t:chimney", &[])]).unwrap();
        let source = r#"(multiblocks: [(name: "hearth", layers: [["sss", "sHs", "sss"], ["C..", "...", "..."], ["C..", "...", "..."]],
            key: {'s': "tag:stone", 'H': "hearthstone", 'C': "chimney"}, core: 'H')])"#;
        let registry = MultiblockRegistry::new(vec![MultiblockRegistry::read("t", source).unwrap()], &blocks).unwrap();
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
