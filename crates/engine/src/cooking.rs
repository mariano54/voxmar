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
//! This first slice cooks one thing at a time over a fire. Recipes of several kinds,
//! stations that take time and the dish computation come next (docs/food-and-cooking.md).

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
}

impl Recipe {
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
            let index = registry.recipes.len();
            for input in inputs {
                registry.by_input.entry(input).or_default().push(index);
            }
            registry.recipes.push(Recipe { def, output, burnt, skill });
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

    #[test]
    fn burning_falls_to_nothing_at_its_level() {
        let registry = registry();
        let recipe = registry.for_item(BlockId(1), &["campfire".into()]).unwrap();
        assert_eq!(recipe.burn_chance(1), BURN_AT_START);
        assert!((recipe.burn_chance(6) - BURN_AT_START / 2.0).abs() < 1e-6);
        assert_eq!(recipe.burn_chance(11), 0.0);
        assert_eq!(recipe.burn_chance(50), 0.0);
    }
}
