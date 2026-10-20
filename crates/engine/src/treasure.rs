//! Treasure: chests locked by the age they belong to, the lockpicks that open them, and
//! what they hold, as content (each package's `treasure.ron`).
//!
//! A chest is locked by its age (1 Nomad, 2 Farm, 3 Bronze ...). A lockpick of the age
//! before or later opens it, and is used up; one two ages short opens it only on a rare
//! lucky try, and breaks either way; anything cruder does not. Opened, the chest becomes
//! a plain one (`opens_to`) holding what its table rolled. A table's `one_of` groups give
//! at most one of their items, which is how a chest never holds a whole set of anything.
//! Its gold is by worth: a Common thing of the chest's `level` (value.rs's price curve)
//! times a share between `centre - spread` and `centre + spread`, in full stacks.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::loot::{Loot, Resolved};

/// The rarest a find may be: one chest in this many (the design's floor).
pub const RAREST: f32 = 1.0 / 400.0;
/// The tag every locked chest block carries, which the `Locked` requirement looks for.
pub const LOCKED_TAG: &str = "locked";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreasureFile {
    /// The chance a lockpick two ages short opens a chest; it breaks either way.
    #[serde(default)]
    pub lucky: Option<f32>,
    /// The coin a chest's gold comes in, and its share of a level's worth.
    #[serde(default)]
    pub gold: Option<GoldDef>,
    #[serde(default)]
    pub picks: Vec<PickDef>,
    #[serde(default)]
    pub chests: Vec<ChestDef>,
    #[serde(default)]
    pub tables: Vec<TableDef>,
}

/// A chest's gold: `item` coins, `centre` ± `spread` times the worth of a Common thing of
/// the chest's level.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldDef {
    pub item: String,
    pub centre: f32,
    pub spread: f32,
}

impl crate::content::Settable for f32 {
    type Set = f32;
    fn with(self, set: &f32) -> (Self, Vec<&'static str>) {
        (*set, vec!["lucky"])
    }
}

impl crate::content::Settable for Option<GoldDef> {
    type Set = GoldDef;
    fn with(self, set: &GoldDef) -> (Self, Vec<&'static str>) {
        (Some(set.clone()), vec!["item", "centre", "spread"])
    }
}

/// A lockpick, and the age it belongs to.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickDef {
    pub item: String,
    pub age: u8,
}

/// A locked chest block: its age, the level its gold is worth, its table, and the block it
/// becomes when opened.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChestDef {
    pub block: String,
    pub age: u8,
    pub level: u8,
    pub table: String,
    pub opens_to: String,
    /// What may wake when its lock gives: creatures that slept beside it.
    #[serde(default)]
    pub wakes: Vec<WakeDef>,
}

/// Creatures a chest may wake as it opens: `count` of `creature`, on `chance`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WakeDef {
    pub creature: String,
    pub count: (u8, u8),
    pub chance: f32,
}

/// What a chest may hold: each `loot` entry on its own chance, and from each `one_of`
/// group, on its chance, one of its items.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableDef {
    pub name: String,
    #[serde(default)]
    pub loot: Vec<Loot>,
    #[serde(default)]
    pub one_of: Vec<OneOf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneOf {
    pub chance: f32,
    /// How many of the one picked, at least and at most.
    #[serde(default = "single")]
    pub count: (u8, u8),
    pub items: Vec<String>,
}

fn single() -> (u8, u8) {
    (1, 1)
}

/// A locked chest, resolved.
#[derive(Clone, Debug)]
pub struct Chest {
    pub age: u8,
    pub table: usize,
    pub opens_to: BlockId,
    /// How many gold coins it holds, at least and at most.
    pub gold: (u32, u32),
    /// What may wake as it opens: a species, how many at least and at most, and the chance.
    pub wakes: Vec<(crate::creature::CreatureId, (u8, u8), f32)>,
}

/// A table, resolved.
#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    most_stacks: usize,
    pub loot: Vec<Resolved>,
    pub one_of: Vec<(f32, (u8, u8), Vec<BlockId>)>,
}


/// What a lockpick does to a chest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Try {
    /// It opens it, and is used up.
    Opens,
    /// It may, on a lucky try, and breaks either way.
    Lucky,
    /// It is too crude to try.
    TooCrude,
}

#[derive(Clone, Debug, Default)]
pub struct Treasure {
    pub lucky: f32,
    /// The gold coin, and how many one stack of it holds.
    gold: Option<(BlockId, u8)>,
    picks: FxHashMap<BlockId, u8>,
    chests: FxHashMap<BlockId, Chest>,
    pub tables: Vec<Table>,
}

impl Treasure {
    pub fn read(namespace: &str, source: &str) -> Result<TreasureFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut file: TreasureFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/treasure.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        if let Some(gold) = &mut file.gold {
            qualify(&mut gold.item);
        }
        for pick in &mut file.picks {
            qualify(&mut pick.item);
        }
        for chest in &mut file.chests {
            qualify(&mut chest.block);
            qualify(&mut chest.opens_to);
            for wake in &mut chest.wakes {
                qualify(&mut wake.creature);
            }
        }
        for table in &mut file.tables {
            for loot in &mut table.loot {
                qualify(&mut loot.item);
            }
            for group in &mut table.one_of {
                group.items.iter_mut().for_each(qualify);
            }
        }
        Ok(file)
    }

    /// Every package's treasure, a later package's gold, pick, chest or table replacing an
    /// earlier one of the same item, block or name; gold is priced by `value`, and stacks
    /// as `stacks` (each item's most per stack, by block id) say; `creature` finds the
    /// species a chest wakes.
    pub fn new(files: Vec<(String, TreasureFile)>, blocks: &BlockRegistry, creature: &dyn Fn(&str) -> Option<crate::creature::CreatureId>, value: &crate::value::Value, stacks: &[u8], notes: &mut Vec<String>) -> Result<Self, RegistryError> {
        let error = |message: String| RegistryError(format!("treasure.ron: {message}"));
        let mut lucky = crate::content::Setting::<f32>::default();
        let mut gold = crate::content::Setting::<Option<GoldDef>>::default();
        let mut picks = Vec::<PickDef>::new();
        let mut chests = Vec::<ChestDef>::new();
        let mut tables = Vec::<TableDef>::new();
        for (package, file) in files {
            lucky.set(&package, "treasure.ron", "treasure lucky chance", file.lucky, notes);
            gold.set(&package, "treasure.ron", "treasure gold", file.gold, notes);
            for pick in file.picks {
                picks.retain(|p| p.item != pick.item);
                picks.push(pick);
            }
            for chest in file.chests {
                chests.retain(|c| c.block != chest.block);
                chests.push(chest);
            }
            for table in file.tables {
                tables.retain(|t| t.name != table.name);
                tables.push(table);
            }
        }
        let (lucky, gold) = (lucky.value, gold.value);
        if !(0.0..=1.0).contains(&lucky) {
            return Err(error(format!("lucky must be 0..=1, not {lucky}")));
        }
        let id = |name: &str| blocks.id(name).ok_or_else(|| error(format!("no item `{name}`")));
        let gold = match &gold {
            Some(g) if !(g.centre.is_finite() && g.spread.is_finite() && g.spread >= 0.0 && g.centre > g.spread) => {
                return Err(error(format!("gold: the centre ({}) must be above the spread ({}), and the spread at least 0", g.centre, g.spread)));
            }
            Some(g) => {
                let item = id(&g.item)?;
                let stack = stacks.get(item.0 as usize).copied().unwrap_or(crate::inventory::MAX_STACK).max(1);
                Some((g, item, stack))
            }
            None => None,
        };
        let mut by_pick = FxHashMap::default();
        for pick in &picks {
            if pick.age == 0 {
                return Err(error(format!("`{}`: ages start at 1", pick.item)));
            }
            by_pick.insert(id(&pick.item)?, pick.age);
        }
        let mut resolved = Vec::new();
        for table in &tables {
            let loot = crate::loot::resolve(&table.loot, blocks).map_err(|e| error(format!("table `{}`: {e}", table.name)))?;
            let mut one_of = Vec::new();
            for group in &table.one_of {
                if group.items.is_empty() || !(0.0..=1.0).contains(&group.chance) || group.count.0 == 0 || group.count.0 > group.count.1 {
                    return Err(error(format!("table `{}`: a one_of needs items, a chance 0..=1 and a count low..=high", table.name)));
                }
                one_of.push((group.chance, group.count, group.items.iter().map(|i| id(i)).collect::<Result<Vec<_>, _>>()?));
            }
            // Every rarer mark runs below every plainer mark, even when a tier is absent.
            // Frequent unmarked staples are no bar to marked finds (the design's tenth).
            for rarer in &loot {
                if rarer.loot.count.0 == 0 {
                    return Err(error(format!("table `{}`: `{}` needs a count of at least one", table.name, rarer.loot.item)));
                }
                if !rarer.loot.when.is_empty() || !rarer.loot.tools.is_empty() {
                    return Err(error(format!("table `{}`: `{}` cannot use when or tools in a chest", table.name, rarer.loot.item)));
                }
                for plainer in &loot {
                    if rarer.loot.rarity > plainer.loot.rarity
                        && (plainer.loot.rarity != crate::block::Rarity::Common || plainer.loot.chance <= 0.1)
                        && rarer.loot.chance >= plainer.loot.chance
                    {
                        return Err(error(format!("table `{}`: `{}` ({}) must come less often than `{}` ({}), which is marked plainer", table.name, rarer.loot.item, rarer.loot.chance, plainer.loot.item, plainer.loot.chance)));
                    }
                }
            }
            // Nothing rarer than one chest in 400: each find's own chance, and each of a
            // group's items at its share of the group's.
            let rarest = loot.iter().map(|r| (r.loot.item.as_str(), r.loot.chance)).chain(
                table.one_of.iter().flat_map(|g| g.items.iter().map(move |i| (i.as_str(), g.chance / g.items.len() as f32))),
            );
            for (item, chance) in rarest {
                if chance < RAREST * 0.999 {
                    return Err(error(format!("table `{}`: `{item}` comes in one chest in {:.0}, rarer than one in 400", table.name, 1.0 / chance)));
                }
            }
            let stack_count = |item: BlockId, count: u8| usize::from(count).div_ceil(usize::from(stacks.get(item.0 as usize).copied().unwrap_or(crate::inventory::MAX_STACK).max(1)));
            let most_stacks = loot.iter().map(|r| stack_count(r.item, r.loot.count.1)).sum::<usize>()
                + one_of.iter().map(|(_, (_, high), items)| items.iter().map(|&item| stack_count(item, *high)).max().unwrap_or(0)).sum::<usize>();
            resolved.push(Table { name: table.name.clone(), most_stacks, loot, one_of });
        }
        let mut by_block = FxHashMap::default();
        for chest in &chests {
            let block = id(&chest.block)?;
            let def = blocks.def(block);
            if !def.has_tag(LOCKED_TAG) || def.hardness >= 0.0 {
                return Err(error(format!("`{}` must be tagged `{LOCKED_TAG}` and unbreakable (hardness below 0), or the lock is no lock", chest.block)));
            }
            if chest.age == 0 {
                return Err(error(format!("`{}`: ages start at 1", chest.block)));
            }
            let table = resolved.iter().position(|t| t.name == chest.table).ok_or_else(|| error(format!("`{}`: no table `{}`", chest.block, chest.table)))?;
            let gold = match gold {
                Some((g, ..)) => {
                    let worth = value.of(chest.level, crate::block::Rarity::Common) as f32;
                    ((worth * (g.centre - g.spread)).round() as u32, (worth * (g.centre + g.spread)).round() as u32)
                }
                None => (0, 0),
            };
            let mut wakes = Vec::new();
            for wake in &chest.wakes {
                let species = creature(&wake.creature).ok_or_else(|| error(format!("`{}` wakes `{}`, which is no creature", chest.block, wake.creature)))?;
                let (low, high) = wake.count;
                if low == 0 || low > high || high > 8 || !(wake.chance > 0.0 && wake.chance <= 1.0) {
                    return Err(error(format!("`{}`: what it wakes comes 1 to 8 at a time, least first, on a chance in (0, 1]", chest.block)));
                }
                wakes.push((species, wake.count, wake.chance));
            }
            by_block.insert(block, Chest { age: chest.age, table, opens_to: id(&chest.opens_to)?, gold, wakes });
        }
        if let Some(stray) = blocks.ids().find(|&b| blocks.def(b).has_tag(LOCKED_TAG) && !by_block.contains_key(&b)) {
            return Err(error(format!("`{}` is tagged `{LOCKED_TAG}` but is no chest here", blocks.def(stray).name)));
        }
        Ok(Self { lucky, gold: gold.map(|(_, item, stack)| (item, stack)), picks: by_pick, chests: by_block, tables: resolved })
    }

    pub fn chest(&self, block: BlockId) -> Option<&Chest> {
        self.chests.get(&block)
    }

    pub fn chests(&self) -> impl Iterator<Item = (BlockId, &Chest)> {
        self.chests.iter().map(|(&b, c)| (b, c))
    }

    /// The coin chests hold their gold in, if any.
    pub fn gold(&self) -> Option<BlockId> {
        self.gold.map(|(coin, _)| coin)
    }

    /// The age of a lockpick, if `item` is one.
    pub fn pick_age(&self, item: BlockId) -> Option<u8> {
        self.picks.get(&item).copied()
    }

    /// The least pick that opens an age, ties by block id so names are deterministic.
    pub fn opening_pick(&self, age: u8) -> Option<BlockId> {
        self.picks.iter().filter(|(_, p)| Self::try_pick(**p, age) == Try::Opens).min_by_key(|(b, p)| (**p, b.0)).map(|(&b, _)| b)
    }

    /// The most stacks `chest` can give at once: its most gold in full stacks, and one for
    /// each of its table's finds and groups.
    pub fn most_stacks(&self, chest: &Chest) -> usize {
        let table = &self.tables[chest.table];
        let gold = self.gold.map_or(0, |(_, stack)| chest.gold.1.div_ceil(stack as u32) as usize);
        gold + table.most_stacks
    }

    /// What a lockpick of `pick` age does to a chest of `chest` age.
    pub fn try_pick(pick: u8, chest: u8) -> Try {
        let (pick, chest) = (u16::from(pick), u16::from(chest));
        if pick + 1 >= chest {
            Try::Opens
        } else if pick + 2 == chest {
            Try::Lucky
        } else {
            Try::TooCrude
        }
    }

    /// What `chest` holds, drawing numbers in 0..1 from `random`.
    pub fn roll(&self, chest: &Chest, mut random: impl FnMut() -> f32) -> Vec<(BlockId, u8)> {
        let table = &self.tables[chest.table];
        let mut found = Vec::new();
        if let Some((coin, stack)) = self.gold {
            let (low, high) = chest.gold;
            let mut coins = low + ((random() * (high - low + 1) as f32) as u32).min(high - low);
            while coins > 0 {
                let here = coins.min(stack as u32);
                found.push((coin, here as u8));
                coins -= here;
            }
        }
        found.extend(crate::loot::roll(&table.loot, &crate::loot::Moment::default(), &mut random));
        for (chance, (low, high), items) in &table.one_of {
            if random() >= *chance {
                continue;
            }
            let item = items[((random() * items.len() as f32) as usize).min(items.len() - 1)];
            let count = low + ((random() * (high - low + 1) as f32) as u8).min(high - low);
            found.push((item, count));
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn blocks() -> BlockRegistry {
        let def = |name: &str, tags: &[&str], hardness: f32| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), hardness, ..Default::default() };
        BlockRegistry::new(vec![
            def("chest", &[], 2.0),
            def("bronze_chest", &["locked"], -1.0),
            def("coin", &[], 1.0),
            def("sword", &[], 1.0),
            def("helm", &[], 1.0),
            def("bone_pick", &[], 1.0),
        ])
        .unwrap()
    }

    const FILE: &str = r#"(
        lucky: 0.05,
        gold: (item: "coin", centre: 1.5, spread: 0.5),
        picks: [(item: "bone_pick", age: 1)],
        chests: [(block: "bronze_chest", age: 3, level: 16, table: "bronze", opens_to: "chest")],
        tables: [(name: "bronze", loot: [(item: "bone_pick", count: (1, 2))], one_of: [(chance: 0.5, items: ["sword", "helm"])])],
    )"#;

    /// A census of the game's own chests, ten thousand of each opened: everything a chest
    /// can hold comes in at least one chest in 400, no chest holds two pieces of gear
    /// (tools, weapons, armour) so none holds a set, and every chest has a pick
    /// that opens it and one a lucky try opens.
    #[test]
    fn the_games_chests_hold_nothing_rarer_than_one_in_400_and_never_two_pieces_of_gear() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, treasure) = (&content.blocks, &content.treasure);
        let gear = |item: BlockId| ["tool", "weapon", "armour"].iter().any(|t| blocks.def(item).has_tag(t));
        const OPENED: u32 = 10_000;
        let mut seed = 0x9e37_79b9u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        let pick_ages: Vec<u8> = blocks.ids().filter_map(|b| treasure.pick_age(b)).collect();
        let mut chests: Vec<_> = treasure.chests().collect();
        chests.sort_by_key(|(_, c)| c.age);
        assert!(chests.len() >= 5, "a chest for each age to Steam");
        // Gold by worth, Nomad to Steam (levels 5, 15, 25, 37 and 52; docs/treasure.md).
        let gold: Vec<(u32, u32)> = chests.iter().take(5).map(|(_, c)| c.gold).collect();
        assert_eq!(gold, [(3, 6), (5, 10), (9, 18), (18, 36), (45, 90)]);
        for (block, chest) in chests {
            let name = &blocks.def(block).name;
            assert!(pick_ages.iter().any(|&p| Treasure::try_pick(p, chest.age) == Try::Opens), "{name}: no pick opens it");
            let table = &treasure.tables[chest.table];
            for item in table.loot.iter().map(|r| r.item).chain(table.one_of.iter().flat_map(|(_, _, items)| items.iter().copied())) {
                if let Some(age) = treasure.pick_age(item) {
                    assert!(age <= chest.age.saturating_sub(1).max(1), "{name} holds a pick of age {age}, making an age ladder");
                }
            }
            let mut seen: FxHashMap<BlockId, u32> = FxHashMap::default();
            for _ in 0..OPENED {
                let found = treasure.roll(chest, &mut random);
                assert!(found.iter().filter(|(i, _)| gear(*i)).count() <= 1, "{name} held two pieces of gear: {found:?}");
                for (item, count) in found {
                    assert!(count >= 1);
                    *seen.entry(item).or_default() += 1;
                }
            }
            let possible = table.loot.iter().map(|r| r.item).chain(table.one_of.iter().flat_map(|(_, _, items)| items.iter().copied()));
            for item in possible {
                let rate = seen.get(&item).copied().unwrap_or(0) as f32 / OPENED as f32;
                assert!(rate >= RAREST * 0.6, "{name}: {} came in {rate:.4} of chests, rarer than one in 400", blocks.def(item).name);
            }
        }
    }

    #[test]
    fn a_pick_opens_one_age_ahead_and_two_only_by_luck() {
        assert_eq!(Treasure::try_pick(2, 3), Try::Opens);
        assert_eq!(Treasure::try_pick(5, 3), Try::Opens);
        assert_eq!(Treasure::try_pick(1, 3), Try::Lucky);
        assert_eq!(Treasure::try_pick(1, 4), Try::TooCrude);
        assert_eq!(Treasure::try_pick(255, 255), Try::Opens);
        assert_eq!(Treasure::try_pick(253, 255), Try::Lucky);
        assert_eq!(Treasure::try_pick(252, 255), Try::TooCrude);
    }

    #[test]
    fn a_chest_gives_at_most_one_of_a_group_and_bad_tables_are_refused() {
        let blocks = blocks();
        // Coins stack to four here.
        let stacks: Vec<u8> = blocks.ids().map(|b| if blocks.def(b).name == "t:coin" { 4 } else { 64 }).collect();
        let value = crate::value::Value::default();
        let treasure = Treasure::new(vec![("t".into(), Treasure::read("t", FILE).unwrap())], &blocks, &|_| None, &value, &stacks, &mut Vec::new()).unwrap();
        let chest = treasure.chest(blocks.id("t:bronze_chest").unwrap()).unwrap().clone();
        assert_eq!(chest.age, 3);
        // Level 16 is worth 5, so its gold is 5 x (1.5 ± 0.5): 5 to 10 coins, in up to three
        // stacks, beside the table's one find and one group.
        assert_eq!(chest.gold, (5, 10));
        assert_eq!(treasure.most_stacks(&chest), 5);
        assert_eq!(treasure.pick_age(blocks.id("t:bone_pick").unwrap()), Some(1));
        let (sword, helm) = (blocks.id("t:sword").unwrap(), blocks.id("t:helm").unwrap());
        let mut seed = 1u32;
        let mut random = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        for _ in 0..500 {
            let found = treasure.roll(&chest, &mut random);
            assert!(found.iter().filter(|(i, _)| *i == sword || *i == helm).count() <= 1, "{found:?}");
            let coins: Vec<u8> = found.iter().filter(|(i, _)| *i == blocks.id("t:coin").unwrap()).map(|&(_, n)| n).collect();
            assert!((5..=10).contains(&coins.iter().map(|&n| n as u32).sum::<u32>()) && coins.iter().all(|&n| (1..=4).contains(&n)), "{coins:?}");
            assert!(found.len() <= treasure.most_stacks(&chest));
        }
        let bad = |file: &str| Treasure::new(vec![("t".into(), Treasure::read("t", file).unwrap())], &blocks, &|_| None, &value, &stacks, &mut Vec::new()).is_err();
        // Rarer than one in 400, a breakable lock, a chest that is not tagged, a missing table.
        assert!(bad(r#"(tables: [(name: "x", loot: [(item: "coin", chance: 0.001)])])"#));
        assert!(bad(r#"(tables: [(name: "x", one_of: [(chance: 0.004, items: ["sword", "helm"])])])"#));
        assert!(bad(r#"(chests: [(block: "chest", age: 1, level: 5, table: "x", opens_to: "chest")], tables: [(name: "x")])"#));
        assert!(bad(r#"(chests: [(block: "bronze_chest", age: 1, level: 5, table: "y", opens_to: "chest")], tables: [(name: "x")])"#));
        assert!(bad(r#"(tables: [])"#), "a locked block with no chest entry");
        // Gold whose spread reaches the centre, or runs below nothing.
        let chest = r#"chests: [(block: "bronze_chest", age: 1, level: 5, table: "x", opens_to: "chest")], tables: [(name: "x")]"#;
        assert!(!bad(&format!(r#"(gold: (item: "coin", centre: 1.5, spread: 0.5), {chest})"#)));
        assert!(bad(&format!(r#"(gold: (item: "coin", centre: 0.5, spread: 0.5), {chest})"#)));
        assert!(bad(&format!(r#"(gold: (item: "coin", centre: 1.0, spread: -0.1), {chest})"#)));
        // A Very rare find as common as a Rare one, a Rare one as common as a plain one.
        let tables = |loot: &str| format!(r#"(chests: [(block: "bronze_chest", age: 1, level: 5, table: "x", opens_to: "chest")], tables: [(name: "x", loot: [{loot}])])"#);
        assert!(!bad(&tables(r#"(item: "sword", chance: 0.01, rarity: VeryRare), (item: "helm", chance: 0.02, rarity: Rare), (item: "coin", chance: 0.05)"#)));
        assert!(bad(&tables(r#"(item: "sword", chance: 0.02, rarity: VeryRare), (item: "helm", chance: 0.02, rarity: Rare)"#)));
        assert!(bad(&tables(r#"(item: "helm", chance: 0.05, rarity: Rare), (item: "coin", chance: 0.04)"#)));
        assert!(bad(&tables(r#"(item: "helm", chance: 0.09, rarity: VeryRare), (item: "coin", chance: 0.05)"#)), "missing Rare tier still compares with Common");
        assert!(bad(&tables(r#"(item: "helm", chance: 0.05, rarity: Uncommon), (item: "coin", chance: 0.04)"#)));
        assert!(bad(&tables(r#"(item: "helm", chance: 0.03, rarity: Legendary), (item: "coin", chance: 0.02, rarity: Rare)"#)));
        assert!(!bad(&tables(r#"(item: "helm", chance: 0.01, rarity: Legendary), (item: "coin", chance: 0.02, rarity: Uncommon)"#)));
        assert!(bad(&tables(r#"(item: "coin", when: [Night])"#)));
        assert!(bad(&tables(r#"(item: "coin", tools: [(tag: "knife")])"#)));
        assert!(bad(&tables(r#"(item: "coin", count: (0, 255))"#)), "zero counts overflow the byte roll span");
        assert!(bad(r#"(chests: [(block: "bronze_chest", age: 1, level: 5, table: "x", opens_to: "chest")], tables: [(name: "x", one_of: [(chance: 1.0, count: (0, 255), items: ["coin"])])])"#));
        // The accepted byte boundary is safe, and room counts full stacks even for a group.
        let file = Treasure::read("t", r#"(chests: [(block: "bronze_chest", age: 1, level: 5, table: "x", opens_to: "chest")], tables: [(name: "x", loot: [(item: "coin", count: (1, 255))], one_of: [(chance: 1.0, count: (1, 255), items: ["coin"])])])"#).unwrap();
        let many = Treasure::new(vec![("t".into(), file)], &blocks, &|_| None, &value, &stacks, &mut Vec::new()).unwrap();
        let chest = many.chest(blocks.expect("t:bronze_chest")).unwrap();
        assert_eq!(many.most_stacks(chest), 128, "two 255-coin finds need 64 four-coin stacks each");
        assert_eq!(many.roll(chest, || 0.999).iter().map(|(_, n)| *n).collect::<Vec<_>>(), [255, 255]);
        assert!(!bad(&tables(r#"(item: "helm", chance: 0.05, rarity: Rare), (item: "coin", chance: 0.5)"#)), "plain finds above a tenth are no bar");
    }

    /// A chest's wakes name a real creature, come one to eight at a time on a real chance,
    /// and are kept resolved on the chest.
    #[test]
    fn what_a_chest_wakes_is_a_real_creature_a_few_at_a_time() {
        let blocks = blocks();
        let stacks: Vec<u8> = blocks.ids().map(|_| 64).collect();
        let value = crate::value::Value::default();
        let bat = |name: &str| (name == "t:bat").then_some(crate::creature::CreatureId(3));
        let with = |wakes: &str| {
            let file = format!(r#"(chests: [(block: "bronze_chest", age: 1, level: 5, table: "x", opens_to: "chest", wakes: [{wakes}])], tables: [(name: "x")])"#);
            Treasure::new(vec![("t".into(), Treasure::read("t", &file).unwrap())], &blocks, &bat, &value, &stacks, &mut Vec::new())
        };
        let treasure = with(r#"(creature: "bat", count: (2, 4), chance: 0.5)"#).unwrap();
        let chest = treasure.chest(blocks.id("t:bronze_chest").unwrap()).unwrap();
        assert_eq!(chest.wakes, [(crate::creature::CreatureId(3), (2, 4), 0.5)]);
        assert!(with(r#"(creature: "wolf", count: (1, 1), chance: 0.5)"#).is_err(), "no such creature");
        assert!(with(r#"(creature: "bat", count: (0, 2), chance: 0.5)"#).is_err(), "none at a time");
        assert!(with(r#"(creature: "bat", count: (3, 2), chance: 0.5)"#).is_err(), "least first");
        assert!(with(r#"(creature: "bat", count: (1, 9), chance: 0.5)"#).is_err(), "a swarm, not a guard");
        assert!(with(r#"(creature: "bat", count: (1, 2), chance: 0.0)"#).is_err(), "never");
    }
}
