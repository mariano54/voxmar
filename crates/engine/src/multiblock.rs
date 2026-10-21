//! Multiblocks: structures recognised from a pattern of blocks in the world, as content
//! (each package's `multiblocks.ron`). A pattern is layers from the bottom up, each rows of
//! characters; a key says what each character takes (an item name, `tag:TAG`, or `a|b`),
//! `.` takes anything and ` ` only air. One character is the core, the block a player uses
//! to bring the structure to life; the pattern may stand turned any of four ways.
//!
//! A structure is lit with a fuel charge from the hand (`fuels`, burning at least the
//! `heat` it asks: a kiln wants charcoal), or with nothing used up but an item tagged
//! `lit_by` in hand (a clamp, lit from a torch). One that `smoulder`s chars what its
//! `wood` cells hold for some game hours, then turns them into what it made; opened early,
//! or rained on through its open core, it makes worse.

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
    /// The heat (machines.ron's fuel `heat`) each of its fuels must give.
    #[serde(default)]
    pub heat: u8,
    /// The tag of an item in hand that lights it, used up never (a torch lights a clamp).
    #[serde(default)]
    pub lit_by: Option<String>,
    /// Lit, it smoulders, charring its `wood` cells.
    #[serde(default)]
    pub smoulder: Option<SmoulderDef>,
    /// Bottom layer first; in each, rows run north to south and characters west to east.
    pub layers: Vec<Vec<String>>,
    pub key: FxHashMap<char, String>,
    pub core: char,
}

/// How a structure smoulders: what it chars, for how long, and into what.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmoulderDef {
    /// Game hours from lighting until it is done.
    pub hours: f32,
    /// The pattern's character for the cells it chars (the stacked firewood).
    pub wood: char,
    /// What each of those becomes done well; done with the structure opened before its
    /// time; and spoiled by rain (the share of it that the hours it was rained on are).
    pub done: String,
    pub early: String,
    pub spoiled: String,
    /// What its core is while it smoulders (taken by the core's key too), and once done.
    pub burning: String,
    pub spent: String,
    /// What its core shows instead from a share of the burn on (0 lit, 1 done), so the
    /// fire is seen catching and dying down: `[(0.0, "flue_catching"), (0.85, "flue_dying")]`,
    /// `burning` between. Each is taken by the core's key too.
    #[serde(default)]
    pub stages: Vec<(f32, String)>,
}

/// A smoulder with its blocks resolved.
#[derive(Clone, Debug)]
pub struct Smoulder {
    pub hours: f32,
    /// Each cell it chars, as an offset from the core (unturned).
    pub wood: Vec<IVec3>,
    pub done: BlockId,
    pub early: BlockId,
    pub spoiled: BlockId,
    pub burning: BlockId,
    pub spent: BlockId,
    /// From which share of the burn its core shows which block, in order; `burning` until
    /// the first and where none is given.
    pub stages: Vec<(f32, BlockId)>,
}

impl Smoulder {
    /// What its core shows `share` of the way through its burn.
    pub fn burning_at(&self, share: f32) -> BlockId {
        self.stages.iter().rev().find(|&&(from, _)| share >= from).map_or(self.burning, |&(_, block)| block)
    }

    /// Whether its core holding `block` is it smouldering, at any stage.
    pub fn is_burning(&self, block: BlockId) -> bool {
        block == self.burning || self.stages.iter().any(|&(_, b)| b == block)
    }
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
    pub heat: u8,
    pub lit_by: Option<String>,
    pub smoulder: Option<Smoulder>,
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

    /// One way to build it, unturned: each cell's offset from the core and the first block
    /// it takes (air where it must be empty). A gate step builds a structure so.
    pub fn example(&self) -> Vec<(IVec3, BlockId)> {
        self.cells
            .iter()
            .filter_map(|(at, cell)| match cell {
                Cell::Any => None,
                Cell::Air => Some((*at, BlockId::AIR)),
                Cell::OneOf(accepts) => Some((*at, accepts[0])),
            })
            .collect()
    }

    /// What the cell at `offset` from the core (unturned) takes; none for one that takes
    /// any block or air, or is not a cell of it.
    pub fn takes(&self, offset: IVec3) -> &[BlockId] {
        self.cells.iter().find(|(at, _)| *at == offset).map_or(&[], |(_, cell)| match cell {
            Cell::OneOf(accepts) => accepts.as_slice(),
            Cell::Any | Cell::Air => &[],
        })
    }

    /// How far from its core, across, its farthest cell lies (any way it is turned).
    pub fn reach(&self) -> i32 {
        self.cells.iter().map(|(o, _)| o.x.abs().max(o.z.abs())).max().unwrap_or(0)
    }

    /// Where each of its cells lies with its core at `core`, turned `turns` quarter turns.
    pub fn cells_at(&self, core: IVec3, turns: u8) -> impl Iterator<Item = IVec3> + '_ {
        self.cells.iter().map(move |(offset, _)| core + turned(*offset, turns))
    }

    /// Whether the structure stands with its core at `core`, turned any way; `block` says
    /// what is where (none for a place not loaded, which never matches).
    pub fn stands(&self, core: IVec3, block: impl Fn(IVec3) -> Option<BlockId>) -> bool {
        self.turn(core, block).is_some()
    }

    /// The first turn (quarter turns) it stands at with its core at `core`, if any.
    pub fn turn(&self, core: IVec3, block: impl Fn(IVec3) -> Option<BlockId>) -> Option<u8> {
        (0..4).find(|&turns| self.turn_matches(core, turns, &block))
    }

    /// Whether it stands with its core at `core`, turned `turns` quarter turns.
    pub fn turn_matches(&self, core: IVec3, turns: u8, block: impl Fn(IVec3) -> Option<BlockId>) -> bool {
        self.cells.iter().all(|(offset, cell)| match (cell, block(core + turned(*offset, turns))) {
            (_, None) => false,
            (Cell::Any, _) => true,
            (Cell::Air, Some(b)) => b.is_air(),
            (Cell::OneOf(accepts), Some(b)) => accepts.binary_search(&b).is_ok(),
        })
    }
}

/// `offset` turned a quarter `turns` times about the core.
pub fn turned(offset: IVec3, turns: u8) -> IVec3 {
    let mut o = offset;
    for _ in 0..turns {
        o = IVec3::new(-o.z, o.y, o.x);
    }
    o
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
            if let Some(tag) = &def.lit_by
                && !blocks.ids().any(|b| blocks.def(b).has_tag(tag))
            {
                return Err(bad(format!("`lit_by` `{tag}`, which no item is tagged")));
            }
            let smoulder = match &def.smoulder {
                None => None,
                Some(s) => {
                    if !(s.hours.is_finite() && s.hours > 0.0) {
                        return Err(bad("it smoulders a number of hours above 0".into()));
                    }
                    if !fuels.is_empty() || def.home || def.lit_by.is_none() {
                        return Err(bad("what smoulders is lit by `lit_by` in hand, with no fuel charge, and is no home".into()));
                    }
                    let block = |name: &str| {
                        let name = if name.contains(':') { name.to_owned() } else { format!("{}:{name}", def.name.split(':').next().unwrap_or("base")) };
                        blocks.id(&name).ok_or_else(|| bad(format!("no block `{name}`")))
                    };
                    let (done, early, spoiled, burning, spent) = (block(&s.done)?, block(&s.early)?, block(&s.spoiled)?, block(&s.burning)?, block(&s.spent)?);
                    if core_takes.binary_search(&burning).is_err() {
                        return Err(bad(format!("its core's key does not take `{}`, what it is while it smoulders, so it would not stand lit", s.burning)));
                    }
                    let wood: Vec<IVec3> = def
                        .layers
                        .iter()
                        .enumerate()
                        .flat_map(|(y, layer)| layer.iter().enumerate().flat_map(move |(z, row)| row.chars().enumerate().filter(|&(_, c)| c == s.wood).map(move |(x, _)| IVec3::new(x as i32, y as i32, z as i32))))
                        .map(|at| at - core)
                        .collect();
                    if wood.is_empty() || s.wood == def.core {
                        return Err(bad(format!("no cell `{}` to char, besides the core", s.wood)));
                    }
                    let mut stages = Vec::new();
                    for (from, name) in &s.stages {
                        if !(0.0..1.0).contains(from) || stages.last().is_some_and(|&(last, _)| *from <= last) {
                            return Err(bad(format!("its stages start at shares of the burn from 0 up to below 1, each later than the last (`{name}` at {from})")));
                        }
                        let stage = block(name)?;
                        if core_takes.binary_search(&stage).is_err() {
                            return Err(bad(format!("its core's key does not take `{name}`, a stage of its burn, so it would not stand lit")));
                        }
                        stages.push((*from, stage));
                    }
                    Some(Smoulder { hours: s.hours, wood, done, early, spoiled, burning, spent, stages })
                }
            };
            multiblocks.push(Multiblock { name: def.name, title: def.title, hint: def.hint, home: def.home, ward: def.ward, fuels, heat: def.heat, lit_by: def.lit_by, smoulder, core: core_takes, cells });
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

    /// A structure that smoulders standing with `at` among its built cells, and where its
    /// core is: a clamp is lit from any of it, its earth as well as its flue, as a player
    /// on the floor beside it sees only its earth. `block` says what is where.
    pub fn smoulder_round(&self, at: IVec3, block: impl Fn(IVec3) -> Option<BlockId>) -> Option<(IVec3, &Multiblock)> {
        self.multiblocks.iter().filter(|m| m.smoulder.is_some()).find_map(|m| {
            let built = m.cells.iter().filter(|(_, cell)| matches!(cell, Cell::OneOf(_)));
            // Each core `at` could be a cell of, at the turn that puts it there: standing at
            // another turn, `at` may lie outside it.
            let cores = built.flat_map(|(offset, _)| (0..4u8).map(move |turns| (at - turned(*offset, turns), turns)));
            cores
                .filter(|&(core, _)| block(core).is_some_and(|b| m.core.binary_search(&b).is_ok()))
                .find(|&(core, turns)| m.turn_matches(core, turns, &block))
                .map(|(core, _)| (core, m))
        })
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

    /// A smouldering structure is lit by a tagged item in hand, chars cells it names into
    /// blocks that exist, keeps standing while its core burns, and finds its cells however
    /// it is turned; load refuses one that does not hold together.
    #[test]
    fn what_smoulders_is_checked_on_load_and_found_turned() {
        let def = |name: &str, tags: &[&str]| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![
            def("air", &[]),
            def("wood", &[]),
            def("earth", &[]),
            def("kindling", &["multiblock_core"]),
            def("flue", &["multiblock_core"]),
            def("coal", &[]),
            def("torch", &["fire"]),
            def("embers", &["multiblock_core"]),
        ])
        .unwrap();
        let load = |lit_by: &str, smoulder: &str, core_key: &str| {
            let source = format!(
                "(multiblocks: [(name: \"clamp\", lit_by: {lit_by}, smoulder: {smoulder}, layers: [[\"eWe\"], [\".K.\"]], key: {{'W': \"wood\", 'e': \"earth\", 'K': \"{core_key}\"}}, core: 'K')])"
            );
            MultiblockRegistry::new(vec![MultiblockRegistry::read("t", &source).unwrap()], &blocks)
        };
        let good = "Some((hours: 8.0, wood: 'W', done: \"coal\", early: \"wood\", spoiled: \"earth\", burning: \"flue\", spent: \"air\"))";
        let registry = load("Some(\"fire\")", good, "kindling|flue").unwrap();
        let clamp = registry.get("t:clamp").unwrap();
        let smoulder = clamp.smoulder.as_ref().unwrap();
        assert_eq!((smoulder.wood.as_slice(), smoulder.done), (&[IVec3::new(0, -1, 0)][..], blocks.id("t:coal").unwrap()));
        // Built turned a quarter: its wood is found under the core all the same, east-west
        // walls now north-south.
        let core = IVec3::new(5, 1, 5);
        let at = |p: IVec3| {
            Some(match (p - core).to_array() {
                [0, 0, 0] => blocks.id("t:kindling").unwrap(),
                [0, -1, 0] => blocks.id("t:wood").unwrap(),
                [0, -1, -1] | [0, -1, 1] => blocks.id("t:earth").unwrap(),
                _ => BlockId::AIR,
            })
        };
        let turn = clamp.turn(core, at).expect("stands turned");
        assert_eq!(core + turned(smoulder.wood[0], turn), core - IVec3::Y);
        // Its flue catching while young and dying down near the end, `burning` between.
        let staged = good.replace("spent: \"air\")", "spent: \"air\", stages: [(0.0, \"kindling\"), (0.8, \"embers\")])");
        assert!(load("Some(\"fire\")", &staged, "kindling|flue").is_err(), "a stage the core's key does not take");
        let registry = load("Some(\"fire\")", &staged, "kindling|flue|embers").unwrap();
        let staged = registry.get("t:clamp").unwrap().smoulder.as_ref().unwrap();
        let id = |name: &str| blocks.id(name).unwrap();
        assert_eq!([0.0, 0.5, 0.8, 0.99].map(|share| staged.burning_at(share)), [id("t:kindling"), id("t:kindling"), id("t:embers"), id("t:embers")]);
        assert!(["t:kindling", "t:flue", "t:embers"].iter().all(|b| staged.is_burning(id(b))) && !staged.is_burning(id("t:coal")));
        assert_eq!(smoulder.burning_at(0.5), id("t:flue"), "no stages: burning throughout");
        for stages in ["[(0.5, \"embers\"), (0.2, \"kindling\")]", "[(1.0, \"embers\")]", "[(0.2, \"nothing\")]"] {
            let bad = good.replace("spent: \"air\")", &format!("spent: \"air\", stages: {stages})"));
            assert!(load("Some(\"fire\")", &bad, "kindling|flue|embers").is_err(), "{stages} loaded");
        }
        for (lit_by, smoulder, core_key) in [
            ("None", good, "kindling|flue"),
            ("Some(\"nothing_has_it\")", good, "kindling|flue"),
            ("Some(\"fire\")", good, "kindling"),
            ("Some(\"fire\")", &good.replace("8.0", "0.0"), "kindling|flue"),
            ("Some(\"fire\")", &good.replace("wood: 'W'", "wood: 'K'"), "kindling|flue"),
            ("Some(\"fire\")", &good.replace("\"coal\"", "\"nothing\""), "kindling|flue"),
        ] {
            assert!(load(lit_by, smoulder, core_key).is_err(), "{lit_by} {smoulder} {core_key} loaded");
        }
    }

    /// A clamp that is not the same turned about is lit from a cell of it, never from one
    /// where only another turn of it would have a cell: that turn does not stand.
    #[test]
    fn a_turned_clamp_is_lit_only_from_its_own_cells() {
        let def = |name: &str, tags: &[&str]| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![def("air", &[]), def("wood", &[]), def("earth", &[]), def("kindling", &["multiblock_core"]), def("flue", &["multiblock_core"]), def("coal", &[]), def("torch", &["fire"])]).unwrap();
        // Wood under the core, earth east of the wood, air east of the core.
        let source = "(multiblocks: [(name: \"clamp\", lit_by: Some(\"fire\"), smoulder: Some((hours: 8.0, wood: 'W', done: \"coal\", early: \"wood\", spoiled: \"earth\", burning: \"flue\", spent: \"air\")), layers: [[\"We\"], [\"K \"]], key: {'W': \"wood\", 'e': \"earth\", 'K': \"kindling|flue\"}, core: 'K')])";
        let registry = MultiblockRegistry::new(vec![MultiblockRegistry::read("t", source).unwrap()], &blocks).unwrap();
        let (id, core) = (|n: &str| blocks.id(n).unwrap(), IVec3::new(5, 1, 5));
        // Built unturned, with earth west of the wood and beside the core too: turned a half,
        // the west earth would be its earth, but the earth beside the core fills its air.
        let at = |p: IVec3| {
            Some(match (p - core).to_array() {
                [0, 0, 0] => id("t:kindling"),
                [0, -1, 0] => id("t:wood"),
                [1, -1, 0] | [-1, -1, 0] | [-1, 0, 0] => id("t:earth"),
                _ => BlockId::AIR,
            })
        };
        assert_eq!(registry.smoulder_round(core + IVec3::new(1, -1, 0), at).map(|(c, _)| c), Some(core), "its own earth");
        assert_eq!(registry.smoulder_round(core + IVec3::new(-1, -1, 0), at).map(|(c, _)| c), None, "earth only a half turn of it would have");
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
