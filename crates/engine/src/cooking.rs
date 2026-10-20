//! Cooking: turning ingredients into food at a station, as `cooking.ron` lists it
//! (docs/food-and-cooking.md). A recipe names what goes in (an item, or any item with a
//! tag), what comes out, the station it needs (by the station block's tag: a campfire is
//! `campfire`), the Cooking level it takes and the experience it gives.
//!
//! Cooking can burn, RuneScape-style: each recipe sets the level at which you start
//! (burning often) and the level at which you stop burning it; between them the chance
//! falls in a straight line. Burnt food gives no experience. Levels never make it faster
//! (docs/skills.md, decision 4).
//!
//! A recipe cooks what is held, `uses` of it at a time (one unless it says), and may take
//! other items from the inventory too (`with`, by name or tag): a crucible makes four
//! bronze from three copper ingots in hand and a tin ore, or a grain of crushed tin,
//! carried. It makes `count` of its output. A recipe may also ask for a `tool` carried, by
//! tag, which it does not take but wears a use: ore is crushed on a stone anvil only with
//! a hammer about you. Stations
//! that take time and the dish computation come next (docs/food-and-cooking.md).

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

/// A recipe as `cooking.ron` writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeDef {
    /// The item cooked, or `tag:` and a tag for any item that has it.
    pub input: String,
    pub output: String,
    /// The tag the station block must have.
    #[serde(default = "campfire")]
    pub station: String,
    #[serde(default = "first")]
    pub level: u8,
    /// Experience for cooking one without burning it.
    pub xp: f32,
    /// The level burning stops at; at `level` the chance is `burn_start`. 0 for what
    /// never burns (metal, charcoal).
    #[serde(default)]
    pub stop_burning: Option<u8>,
    /// The chance of burning at `level`; it falls in a straight line to nothing at
    /// `stop_burning`. A quarter unless given.
    #[serde(default = "burn_start")]
    pub burn_start: f32,
    /// What burning it leaves; nothing (the food is lost) unless given.
    #[serde(default)]
    pub burnt: Option<String>,
    /// The skill it trains and asks a level in: Cooking unless given (a kiln's pottery is
    /// Crafting, a crucible's metal Engineering).
    #[serde(default)]
    pub skill: Option<String>,
    /// How many of the held input one go takes.
    #[serde(default = "one")]
    pub uses: u8,
    /// Other items one go takes from the inventory, by name or `tag:` (any one item with
    /// it: a tin ore or a grain of crushed tin), with how many of each.
    #[serde(default)]
    pub with: Vec<(String, u8)>,
    /// How many of `output` one go makes (what burns leaves one `burnt`).
    #[serde(default = "one")]
    pub count: u8,
    /// The tag of a tool one go needs carried (in hand or the pack), not taken but worn a use
    /// a go: a `hammer` to crush ore on an anvil, an `axe` to split a log on a chopping block.
    #[serde(default)]
    pub tool: Option<String>,
}

fn one() -> u8 {
    1
}

fn campfire() -> String {
    "campfire".into()
}

fn first() -> u8 {
    1
}

/// The chance of burning a recipe at the level it starts at, unless it says (a quarter: a
/// new cook loses some food, not half of it).
pub const BURN_AT_START: f32 = 0.25;

fn burn_start() -> f32 {
    BURN_AT_START
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CookingFile {
    recipes: Vec<RecipeDef>,
}

/// A recipe with its names resolved.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub def: RecipeDef,
    pub output: BlockId,
    pub burnt: Option<BlockId>,
    pub skill: Option<SkillId>,
    /// The other items it takes (`RecipeDef::with`), resolved: for each, the items any one
    /// of which will do, and how many.
    pub with: Vec<(Vec<BlockId>, u8)>,
}

impl Recipe {
    /// Everything one go takes when `input` is held: that many of it, then the rest, each
    /// as the items any one of which will do.
    pub fn ingredients(&self, input: BlockId) -> Vec<(Vec<BlockId>, u8)> {
        std::iter::once((vec![input], self.def.uses)).chain(self.with.iter().cloned()).collect()
    }

    /// What one go takes from a cook who has `count` of each item, `input` held: of each
    /// thing it takes besides, the first that will do they have enough of. Short of any,
    /// the same list as an error, naming the first item that would do where they are short.
    pub fn wants(&self, input: BlockId, count: impl Fn(BlockId) -> u32) -> Result<Vec<(BlockId, u8)>, Vec<(BlockId, u8)>> {
        let mut short = count(input) < self.def.uses as u32;
        let mut wants = vec![(input, self.def.uses)];
        for (items, n) in &self.with {
            let item = items.iter().copied().find(|&item| count(item) >= *n as u32);
            short |= item.is_none();
            wants.push((item.unwrap_or(items[0]), *n));
        }
        if short { Err(wants) } else { Ok(wants) }
    }

    /// Whether `carried` (everything the cook carries) holds the tool it asks for, if any.
    pub fn has_tool(&self, blocks: &BlockRegistry, carried: impl IntoIterator<Item = BlockId>) -> bool {
        let Some(tag) = &self.def.tool else { return true };
        carried.into_iter().any(|item| blocks.def(item).has_tag(tag))
    }

    /// The chance of burning it at `level`.
    pub fn burn_chance(&self, level: u8) -> f32 {
        if self.def.stop_burning == Some(0) {
            return 0.0;
        }
        let start = self.def.level;
        let stop = self.def.stop_burning.unwrap_or(start.saturating_add(20)).max(start + 1);
        if level >= stop {
            return 0.0;
        }
        let through = level.saturating_sub(start) as f32 / (stop - start) as f32;
        self.def.burn_start * (1.0 - through)
    }
}

/// Every cooking recipe, found by what goes in.
#[derive(Debug, Default)]
pub struct CookingRegistry {
    recipes: Vec<Recipe>,
    /// Each input item's recipes, in the order they were defined.
    by_input: FxHashMap<BlockId, Vec<usize>>,
    pub skill: Option<SkillId>,
}

impl CookingRegistry {
    /// Reads a package's `cooking.ron`, qualifying names with its namespace.
    pub fn read(namespace: &str, source: &str) -> Result<Vec<RecipeDef>, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: CookingFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/cooking.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        Ok(file
            .recipes
            .into_iter()
            .map(|mut def| {
                qualify(&mut def.input);
                qualify(&mut def.output);
                if let Some(burnt) = &mut def.burnt {
                    qualify(burnt);
                }
                def.with.iter_mut().for_each(|(name, _)| qualify(name));
                def
            })
            .collect())
    }

    pub fn new(defs: Vec<RecipeDef>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let mut registry = Self { skill: skills.id("core:cooking"), ..Default::default() };
        for def in defs {
            let error = |message: String| RegistryError(format!("cooking recipe for `{}`: {message}", def.input));
            let block = |name: &str| blocks.id(name).ok_or_else(|| error(format!("no item `{name}`")));
            if !(0.0..=1.0).contains(&def.burn_start) {
                return Err(error(format!("burn_start {} is not a chance between 0 and 1", def.burn_start)));
            }
            let output = block(&def.output)?;
            let burnt = def.burnt.as_deref().map(block).transpose()?;
            let skill = match &def.skill {
                Some(name) => Some(skills.id(name).ok_or_else(|| error(format!("no skill `{name}`")))?),
                None => registry.skill,
            };
            let inputs: Vec<BlockId> = match def.input.strip_prefix("tag:") {
                Some(tag) => blocks.tagged(tag).collect(),
                None => vec![block(&def.input)?],
            };
            if inputs.is_empty() {
                return Err(error("nothing has that tag".into()));
            }
            if def.uses == 0 || def.count == 0 {
                return Err(error("it uses and makes at least one".into()));
            }
            let mut with: Vec<(Vec<BlockId>, u8)> = Vec::new();
            for (name, n) in &def.with {
                let items: Vec<BlockId> = match name.strip_prefix("tag:") {
                    Some(tag) => blocks.tagged(tag).collect(),
                    None => vec![block(name)?],
                };
                if items.is_empty() {
                    return Err(error(format!("`with` takes `{name}`, which nothing is")));
                }
                let taken = |item: &BlockId| inputs.contains(item) || with.iter().any(|(others, _)| others.contains(item));
                if *n == 0 || items.iter().any(taken) {
                    return Err(error(format!("`with` takes `{name}` no times, twice, or as well as the input: count it once")));
                }
                with.push((items, *n));
            }
            if let Some(tag) = &def.tool
                && (blocks.tagged(tag).next().is_none() || inputs.iter().chain(with.iter().flat_map(|(items, _)| items)).any(|&item| blocks.def(item).has_tag(tag)))
            {
                return Err(error(format!("its `tool` `{tag}` is carried by nothing, or by what it cooks")));
            }
            let index = registry.recipes.len();
            for input in inputs {
                registry.by_input.entry(input).or_default().push(index);
            }
            registry.recipes.push(Recipe { def, output, burnt, skill, with });
        }
        Ok(registry)
    }

    /// The recipe that cooks `item` at a station with `tags`, if there is one.
    pub fn for_item(&self, item: BlockId, station: &[String]) -> Option<&Recipe> {
        self.by_input.get(&item)?.iter().map(|&i| &self.recipes[i]).find(|r| station.contains(&r.def.station))
    }

    /// Every recipe with each item it takes, by item, then in the order defined.
    pub fn iter(&self) -> impl Iterator<Item = (BlockId, &Recipe)> {
        let mut inputs: Vec<&BlockId> = self.by_input.keys().collect();
        inputs.sort();
        inputs.into_iter().flat_map(move |&input| self.by_input[&input].iter().map(move |&i| (input, &self.recipes[i])))
    }

    /// Every recipe, in the order defined.
    pub fn recipes(&self) -> impl Iterator<Item = &Recipe> {
        self.recipes.iter()
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn registry() -> CookingRegistry {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:raw_rabbit".into(), tags: vec!["meat".into()], ..Default::default() },
            BlockDef { name: "base:cooked_rabbit".into(), ..Default::default() },
            BlockDef { name: "base:burnt_food".into(), ..Default::default() },
        ])
        .unwrap();
        let defs = CookingRegistry::read(
            "base",
            r#"(recipes: [(input: "raw_rabbit", output: "cooked_rabbit", xp: 30.0, stop_burning: 11, burnt: "burnt_food")])"#,
        )
        .unwrap();
        CookingRegistry::new(defs, &blocks, &SkillRegistry::default()).unwrap()
    }

    #[test]
    fn a_recipe_may_start_burning_at_its_own_chance_within_zero_and_one() {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:raw_rabbit".into(), ..Default::default() },
            BlockDef { name: "base:cooked_rabbit".into(), ..Default::default() },
        ])
        .unwrap();
        let build = |chance: &str| {
            let source = format!(r#"(recipes: [(input: "raw_rabbit", output: "cooked_rabbit", xp: 1.0, stop_burning: 11, burn_start: {chance})])"#);
            CookingRegistry::new(CookingRegistry::read("base", &source).unwrap(), &blocks, &SkillRegistry::default())
        };
        let registry = build("0.1").unwrap();
        let recipe = registry.iter().next().unwrap().1;
        assert_eq!(recipe.burn_chance(1), 0.1);
        assert!(build("1.5").is_err());
    }

    #[test]
    fn recipes_are_found_by_input_and_station() {
        let registry = registry();
        let raw = BlockId(1);
        assert!(registry.for_item(raw, &["campfire".into()]).is_some());
        assert!(registry.for_item(raw, &["oven".into()]).is_none());
        assert!(registry.for_item(BlockId(2), &["campfire".into()]).is_none());
    }

    /// A recipe may take several of what is held and other items besides, and make several:
    /// three copper ingots and a tin ore make four bronze. What goes with it may be any item
    /// with a tag: a grain of crushed tin does as a tin ore. Load refuses what could not be
    /// counted.
    #[test]
    fn a_recipe_takes_several_inputs_and_makes_several() {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:copper_ingot".into(), ..Default::default() },
            BlockDef { name: "base:tin_ore".into(), tags: vec!["metal:tin".into()], ..Default::default() },
            BlockDef { name: "base:bronze_ingot".into(), ..Default::default() },
            BlockDef { name: "base:crushed_tin".into(), tags: vec!["metal:tin".into()], ..Default::default() },
        ])
        .unwrap();
        let build = |extra: &str| {
            let source = format!(r#"(recipes: [(input: "copper_ingot", output: "bronze_ingot", station: "crucible", xp: 88.0, stop_burning: 0, {extra})])"#);
            CookingRegistry::new(CookingRegistry::read("base", &source).unwrap(), &blocks, &SkillRegistry::default())
        };
        let registry = build(r#"uses: 3, with: [("tin_ore", 1)], count: 4"#).unwrap();
        let (copper, tin) = (BlockId(1), BlockId(2));
        let recipe = registry.for_item(copper, &["crucible".into()]).unwrap();
        assert_eq!((recipe.ingredients(copper), recipe.def.count), (vec![(vec![copper], 3), (vec![tin], 1)], 4));
        assert!(registry.for_item(tin, &["crucible".into()]).is_none(), "what goes with it is not held to cook");
        let registry = build(r#"uses: 3, with: [("tag:metal:tin", 1)], count: 4"#).unwrap();
        let recipe = registry.for_item(copper, &["crucible".into()]).unwrap();
        let crushed = BlockId(4);
        let having = |have: &'static [(BlockId, u32)]| move |item: BlockId| have.iter().filter(|h| h.0 == item).map(|h| h.1).sum::<u32>();
        assert_eq!(recipe.wants(copper, having(&[(BlockId(1), 3), (BlockId(4), 1)])), Ok(vec![(copper, 3), (crushed, 1)]), "crushed tin does as a tin ore");
        assert_eq!(recipe.wants(copper, having(&[(BlockId(1), 3), (BlockId(2), 1), (BlockId(4), 1)])), Ok(vec![(copper, 3), (tin, 1)]), "the first that will do");
        assert_eq!(recipe.wants(copper, having(&[(BlockId(1), 3)])), Err(vec![(copper, 3), (tin, 1)]), "short, it names the first that would do");
        for refused in [r#"with: [("tin_ore", 0)]"#, r#"with: [("copper_ingot", 1)]"#, r#"with: [("tin_ore", 1), ("tin_ore", 1)]"#, r#"with: [("tin_ore", 1), ("tag:metal:tin", 1)]"#, r#"with: [("tag:ore", 1)]"#, r#"with: [("zinc", 1)]"#, "uses: 0", "count: 0"] {
            assert!(build(refused).is_err(), "{refused}");
        }
    }

    /// A recipe's `tool` is a tag something carries, and not what it cooks: split with an
    /// axe about you, which is found in what is carried.
    #[test]
    fn a_recipe_tool_is_carried_by_something_other_than_what_it_cooks() {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:oak_log".into(), tags: vec!["log".into()], ..Default::default() },
            BlockDef { name: "base:firewood".into(), ..Default::default() },
            BlockDef { name: "base:stone_axe".into(), tags: vec!["axe".into()], ..Default::default() },
        ])
        .unwrap();
        let build = |tool: &str| {
            let source = format!(r#"(recipes: [(input: "oak_log", output: "firewood", station: "chopping_block", xp: 1.0, stop_burning: 0, count: 2, tool: Some("{tool}"))])"#);
            CookingRegistry::new(CookingRegistry::read("base", &source).unwrap(), &blocks, &SkillRegistry::default())
        };
        let registry = build("axe").unwrap();
        let recipe = registry.for_item(BlockId(1), &["chopping_block".into()]).unwrap();
        assert!(!recipe.has_tool(&blocks, [BlockId(1), BlockId(2)]));
        assert!(recipe.has_tool(&blocks, [BlockId(1), BlockId(3)]));
        assert!(build("saw").is_err(), "carried by nothing");
        assert!(build("log").is_err(), "carried by what it cooks");
    }

    #[test]
    fn burning_falls_to_nothing_at_its_level() {
        let registry = registry();
        let recipe = registry.for_item(BlockId(1), &["campfire".into()]).unwrap();
        assert_eq!(recipe.burn_chance(1), BURN_AT_START);
        assert!((recipe.burn_chance(6) - BURN_AT_START / 2.0).abs() < 1e-6);
        assert_eq!(recipe.burn_chance(11), 0.0);
        assert_eq!(recipe.burn_chance(50), 0.0);
    }

    #[test]
    fn a_tool_is_asked_for_by_tag_and_not_taken() {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:copper_ore".into(), ..Default::default() },
            BlockDef { name: "base:crushed_copper".into(), ..Default::default() },
            BlockDef { name: "base:stone_hammer".into(), tags: vec!["hammer".into()], ..Default::default() },
        ])
        .unwrap();
        let build = |tool: &str| {
            let source = format!(r#"(recipes: [(input: "copper_ore", output: "crushed_copper", station: "anvil", xp: 1.0, stop_burning: 0, tool: "{tool}")])"#);
            CookingRegistry::new(CookingRegistry::read("base", &source).unwrap(), &blocks, &SkillRegistry::default())
        };
        let registry = build("hammer").unwrap();
        let (ore, hammer) = (blocks.id("base:copper_ore").unwrap(), blocks.id("base:stone_hammer").unwrap());
        let recipe = registry.for_item(ore, &["anvil".into()]).unwrap();
        assert!(recipe.has_tool(&blocks, [ore, hammer]), "a hammer carried");
        assert!(!recipe.has_tool(&blocks, [ore]), "no hammer");
        assert_eq!(recipe.ingredients(ore), vec![(vec![ore], 1)], "the hammer is not taken (it wears, server side)");
        // A tool nothing carries could never be had.
        assert!(build("chisel").unwrap_err().0.contains("carried by nothing"));
    }
}
