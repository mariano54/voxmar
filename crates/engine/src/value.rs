//! What things are worth, in gold (the design wiki's P51 price curve, draft 52): two coins
//! at level 1, two and a half times as much every fifteen levels, rounded to the nearest
//! five above a hundred, times its rarity's factor. A thing's level is what it takes to
//! use it: the Defence that wears it, the level that wields it, or the lowest level of a
//! recipe that makes it; a made thing is worth at least a quarter more than what goes
//! into it. Death ranks what a player keeps by it, a unit at a time
//! (`server::death`); shops price by it (P51). Every number is content: each package's
//! `value.ron`, field by field, the last package's winning.

use std::collections::HashMap;

use serde::Deserialize;

use crate::block::{BlockId, Rarity};

/// The price curve, worked out from every package's `value.ron`.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    /// Coins at level 1, and how many times as much `per` levels on.
    pub base: f32,
    pub growth: f32,
    pub per: f32,
    /// Above this, a price is rounded to the nearest `step`.
    pub round_above: u32,
    pub step: u32,
    /// Each rarity's factor, Common to Legendary.
    pub rarity: [u32; 5],
    /// A made thing is worth at least this many times what went into one of it.
    pub crafted: f32,
    /// What dying keeps.
    pub death: Death,
}

/// What a player keeps through death (Mariano, 2026-10-06): the `kept` units worth most,
/// `warded` with a grave-ward amulet (kept besides); the rest lies `grave` seconds where
/// they fell, `warded_grave` with the amulet.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Death {
    pub kept: u8,
    pub warded: u8,
    pub grave: f32,
    pub warded_grave: f32,
}

impl Default for Death {
    fn default() -> Self {
        Self { kept: 3, warded: 4, grave: 1800.0, warded_grave: 3600.0 }
    }
}

impl Default for Value {
    fn default() -> Self {
        Self { base: 2.0, growth: 2.5, per: 15.0, round_above: 100, step: 5, rarity: [1, 2, 5, 12, 30], crafted: 1.25, death: Death::default() }
    }
}

/// One package's `value.ron`: the fields it sets.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ValueFile {
    pub base: Option<f32>,
    pub growth: Option<f32>,
    pub per: Option<f32>,
    pub round_above: Option<u32>,
    pub step: Option<u32>,
    pub rarity: Option<RarityFactors>,
    pub crafted: Option<f32>,
    pub death: Option<Death>,
}

/// A factor for each rarity.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RarityFactors {
    pub common: u32,
    pub uncommon: u32,
    pub rare: u32,
    pub very_rare: u32,
    pub legendary: u32,
}

impl Value {
    pub fn read(namespace: &str, source: &str) -> Result<ValueFile, crate::block::RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| crate::block::RegistryError(format!("{namespace}/value.ron: {e}")))
    }

    /// The packages' files laid over the defaults in order.
    pub fn merge(files: Vec<ValueFile>) -> Self {
        let mut value = Value::default();
        for file in files {
            value.base = file.base.unwrap_or(value.base);
            value.growth = file.growth.unwrap_or(value.growth);
            value.per = file.per.unwrap_or(value.per).max(1e-3);
            value.round_above = file.round_above.unwrap_or(value.round_above);
            value.step = file.step.unwrap_or(value.step).max(1);
            if let Some(r) = file.rarity {
                value.rarity = [r.common, r.uncommon, r.rare, r.very_rare, r.legendary];
            }
            value.crafted = file.crafted.unwrap_or(value.crafted).max(0.0);
            value.death = file.death.unwrap_or(value.death);
        }
        value
    }

    /// Coins for one of a thing of `level` and `rarity`.
    pub fn of(&self, level: u8, rarity: Rarity) -> u32 {
        let exact = self.base * self.growth.powf(level.max(1).saturating_sub(1) as f32 / self.per);
        let coins = if exact > self.round_above as f32 {
            ((exact / self.step as f32).round() as u32) * self.step
        } else {
            exact.round().max(1.0) as u32
        };
        coins * self.rarity[rarity as usize]
    }
}

/// What one of `item` is worth: its level's price times its rarity's factor, and for a
/// made thing at least `crafted` times what goes into one of it by its cheapest recipe.
pub fn worth(content: &crate::content::Content, item: BlockId) -> u32 {
    worth_in(content, item, &mut HashMap::new())
}

/// [`worth`], remembering in `known` what it works out (and, while working an item out,
/// that it is under way, so a recipe that leads back to it counts it at its curve price).
pub fn worth_in(content: &crate::content::Content, item: BlockId, known: &mut HashMap<BlockId, u32>) -> u32 {
    if let Some(&coins) = known.get(&item) {
        return coins;
    }
    let value = &content.value;
    let curve = value.of(level_of(content, item), content.blocks.def(item).rarity);
    known.insert(item, curve);
    let mut made: Option<u32> = None;
    for recipe in content.recipes.iter().filter(|r| r.output == item && r.count > 0) {
        let mut inputs = 0u64;
        for cell in recipe.alternatives().into_iter().flatten() {
            inputs += cell.iter().map(|&i| worth_in(content, i, known)).min().unwrap_or(0) as u64;
        }
        let each = (inputs as f32 / recipe.count as f32 * value.crafted).round() as u32;
        made = Some(made.map_or(each, |m| m.min(each)));
    }
    let coins = curve.max(made.unwrap_or(0));
    known.insert(item, coins);
    coins
}

/// The level of `item` for its worth: the most of what wears or wields it, and the least
/// level of a recipe that makes it (1 when nothing says).
pub fn level_of(content: &crate::content::Content, item: BlockId) -> u8 {
    let worn = content.combat.wear_level(item);
    let wielded = content.combat.weapon(item).map(|w| w.level);
    let made = content.recipes.iter().filter(|r| r.output == item).map(|r| r.level).min();
    [worn, wielded, made].into_iter().flatten().max().unwrap_or(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_price_curve_is_p51s() {
        let value = Value::default();
        assert_eq!(value.of(1, Rarity::Common), 2);
        assert_eq!(value.of(16, Rarity::Common), 5);
        assert_eq!(value.of(20, Rarity::Common), 6, "2 x 2.5^(19/15) = 6.4");
        assert_eq!(value.of(46, Rarity::Common), 31, "31.25");
        // Above a hundred, to the nearest five: level 61 is 78, level 76 is 195.3.
        assert_eq!(value.of(76, Rarity::Common), 195);
        assert_eq!(value.of(1, Rarity::Rare), 10);
        assert_eq!(value.of(1, Rarity::Legendary), 60);
        let cheap = Value::merge(vec![Value::read("t", "(base: 1.0)").unwrap(), Value::read("u", "(step: 10)").unwrap()]);
        assert_eq!((cheap.base, cheap.step, cheap.growth), (1.0, 10, 2.5), "packages set field by field");
        let kept = Value::merge(vec![Value::read("t", "(death: (kept: 2))").unwrap()]).death;
        assert_eq!((kept.kept, kept.warded, kept.grave), (2, 4, 1800.0), "unset death numbers stay");
    }

    #[test]
    fn made_and_worn_things_are_worth_what_they_take() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let id = |name: &str| content.blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("{name}"));
        let of = |name: &str| worth(&content, id(name));
        for name in ["dirt", "stick", "iron_ingot", "iron_pickaxe", "iron_sword", "chainmail_shirt", "wooden_shield", "bread"] {
            println!("{name}: level {} worth {}", level_of(&content, id(name)), of(name));
        }
        assert!(of("iron_pickaxe") > 3 * of("iron_ingot"), "a pickaxe is worth more than its three ingots");
        assert!(of("iron_sword") >= content.value.of(20, Rarity::Common), "a sword is worth at least the level that wields it");
        assert!(of("chainmail_shirt") > of("bread") && of("chainmail_shirt") > of("dirt"));
        // Every item has a worth, and none is worth nothing.
        let mut known = HashMap::new();
        for item in (0..content.blocks.len() as u16).map(BlockId) {
            assert!(worth_in(&content, item, &mut known) >= 1);
        }
    }
}
