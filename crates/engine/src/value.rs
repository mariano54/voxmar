//! What things are worth, in gold (the design wiki's P51 price curve, draft 52): two coins
//! at level 1, two and a half times as much every fifteen levels, rounded to the nearest
//! five above a hundred, times its rarity's factor. A thing's level is what it takes to
//! use it: the Defence that wears it, the level that wields it, or the lowest level of a
//! recipe that makes it; a made thing is worth at least a quarter more than what goes
//! into it. Death ranks what a player keeps by it, a unit at a time
//! (`server::death`); shops price by it (P51). Every number is content: each package's
//! `value.ron`, field by field, the last package's winning.

use std::collections::{BTreeMap, HashMap};

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
    /// A made thing is worth at least this many times what went into one of it, a raw
    /// material in it (nothing makes, wears or wields it) counting `raw` coins times its
    /// rarity's factor (the wiki, P51 draft 73).
    pub crafted: f32,
    pub raw: f32,
    /// Things worth a set number of coins whatever their level (a gold coin is one).
    pub fixed: BTreeMap<String, u32>,
    /// Things priced on the curve at a set level, where nothing wears, wields or makes them
    /// at one (a boss's keepsake at its boss's level).
    pub levels: BTreeMap<String, u8>,
    /// What dying keeps.
    pub death: Death,
}

/// What a player keeps through death (Mariano, 2026-10-06): the `kept` units worth most,
/// `warded` with a grave-ward amulet (kept besides); the rest lies `grave` seconds where
/// they fell, `warded_grave` with the amulet. A prayer that keeps more (Keepsake) never
/// takes it past `most` (design d86: it doesn't stack with the amulet).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Death {
    pub kept: u8,
    pub warded: u8,
    pub most: u8,
    pub grave: f32,
    pub warded_grave: f32,
}

crate::content::settable! {
    /// What one package's `value.ron` sets of the death rules.
    SetDeath for Death { kept: u8, warded: u8, most: u8, grave: f32, warded_grave: f32 }
}

impl Default for Death {
    fn default() -> Self {
        Self { kept: 3, warded: 4, most: 4, grave: 1800.0, warded_grave: 3600.0 }
    }
}

impl Death {
    /// How many units death keeps (besides an amulet): `kept`, or `warded` with one, and
    /// `more` from prayers, never past `most`.
    pub fn keeps(&self, warded: bool, more: u8) -> u8 {
        let base = if warded { self.warded } else { self.kept };
        base.saturating_add(more).min(self.most.max(base))
    }
}

impl Default for Value {
    fn default() -> Self {
        Self { base: 2.0, growth: 2.5, per: 15.0, round_above: 100, step: 5, rarity: [1, 2, 5, 12, 30], crafted: 1.25, raw: 0.5, fixed: BTreeMap::new(), levels: BTreeMap::new(), death: Death::default() }
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
    pub raw: Option<f32>,
    pub fixed: BTreeMap<String, u32>,
    pub levels: BTreeMap<String, u8>,
    pub death: Option<SetDeath>,
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

    /// The packages' files (with their package's name) laid over the defaults in order. A
    /// later package's number replaces an earlier one's, and `notes` names both.
    pub fn merge(files: Vec<(String, ValueFile)>, notes: &mut Vec<String>) -> Self {
        use crate::content::Settable;
        let mut value = Value::default();
        let mut laid = crate::content::Laid::default();
        for (package, file) in files {
            let mut set = Vec::new();
            macro_rules! lay {
                ($($field:ident $(.$clamp:ident($min:expr))?),*) => {$(
                    if let Some(new) = file.$field {
                        value.$field = new$(.$clamp($min))?;
                        set.push(stringify!($field).to_owned());
                    }
                )*};
            }
            lay!(base, growth, per.max(1e-3), round_above, step.max(1), crafted.max(0.0), raw.max(0.0));
            if let Some(r) = file.rarity {
                value.rarity = [r.common, r.uncommon, r.rare, r.very_rare, r.legendary];
                set.push("rarity".to_owned());
            }
            if let Some(death) = &file.death {
                let (laid_death, names) = value.death.with(death);
                value.death = laid_death;
                set.extend(names.into_iter().map(|name| format!("death.{name}")));
            }
            set.extend(file.fixed.keys().map(|item| format!("fixed.{item}")));
            value.fixed.extend(file.fixed);
            set.extend(file.levels.keys().map(|item| format!("levels.{item}")));
            value.levels.extend(file.levels);
            laid.set(&package, "value.ron", "worth", set, notes);
        }
        value
    }

    /// Refuses numbers that cannot hold together: death's `most` under what it keeps
    /// without prayers.
    pub fn check(&self) -> Result<(), crate::block::RegistryError> {
        let d = self.death;
        if d.most < d.kept || d.most < d.warded {
            return Err(crate::block::RegistryError(format!("value.ron: death keeps at most {} but {} without prayers ({} with the amulet)", d.most, d.kept, d.warded)));
        }
        Ok(())
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

/// What one of `item` is worth: what `fixed` says, or its level's price times its rarity's
/// factor, and for a made thing at least `crafted` times what goes into one of it by its
/// cheapest recipe.
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
    if let Some(&coins) = value.fixed.get(&content.blocks.def(item).name) {
        known.insert(item, coins);
        return coins;
    }
    let curve = value.of(level_of(content, item), content.blocks.def(item).rarity);
    known.insert(item, curve);
    let mut made: Option<u32> = None;
    for recipe in content.recipes.iter().filter(|r| r.output == item && r.count > 0) {
        let mut inputs = 0.0f32;
        for cell in recipe.alternatives().into_iter().flatten() {
            let input = |i: BlockId, known: &mut HashMap<BlockId, u32>| match raw(content, i) {
                true => value.raw * value.rarity[content.blocks.def(i).rarity as usize] as f32,
                false => worth_in(content, i, known) as f32,
            };
            inputs += cell.iter().map(|&i| input(i, known)).reduce(f32::min).unwrap_or(0.0);
        }
        let each = (inputs / recipe.count as f32 * value.crafted).round() as u32;
        made = Some(made.map_or(each, |m| m.min(each)));
    }
    let coins = curve.max(made.unwrap_or(0));
    known.insert(item, coins);
    coins
}

/// Whether `item` is a raw material: worth no set price, and nothing makes, wears or wields it.
/// Cobblestone is not, since four loose stones make two: what is made of it (a campfire,
/// about 3 to 6; a stone sword, about 4 to 8) is worth a little more for what death keeps.
fn raw(content: &crate::content::Content, item: BlockId) -> bool {
    !content.value.fixed.contains_key(&content.blocks.def(item).name)
        && content.combat.wear_level(item).is_none()
        && content.combat.weapon(item).is_none()
        && !content.recipes.iter().any(|r| r.output == item)
}

/// The level of `item` for its worth: what `levels` sets, else the most of what wears or
/// wields it, the Fishing level that lands it, and the least level of a way to make it, at
/// the grid or at a station, a dish's Cooking level among them (1 when nothing says). A fish
/// and the dish cooked of it are worth alike: a glimmer koi caught at 75 and cooked at 75.
pub fn level_of(content: &crate::content::Content, item: BlockId) -> u8 {
    if let Some(&level) = content.value.levels.get(&content.blocks.def(item).name) {
        return level.max(1);
    }
    let worn = content.combat.wear_level(item);
    let wielded = content.combat.weapon(item).map(|w| w.level);
    let caught = content.fishing.iter().filter(|c| c.item == item).map(|c| c.def.level).min();
    let crafted = content.recipes.iter().filter(|r| r.output == item).map(|r| r.level);
    let cooked = content.cooking.iter().filter(|(_, r)| r.output == item).map(|(_, r)| r.def.level);
    let made = crafted.chain(cooked).min();
    [worn, wielded, caught, made].into_iter().flatten().max().unwrap_or(1).max(1)
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
        let mut notes = Vec::new();
        let cheap = Value::merge(vec![("t".into(), Value::read("t", "(base: 1.0, death: (kept: 2))").unwrap()), ("u".into(), Value::read("u", "(step: 10, death: (grave: 60.0))").unwrap())], &mut notes);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!((cheap.death.kept, cheap.death.grave), (2, 60.0), "a later package's death number leaves the earlier one's");
        assert_eq!((cheap.base, cheap.step, cheap.growth), (1.0, 10, 2.5), "packages set field by field");
        let kept = Value::merge(vec![("t".into(), Value::read("t", "(death: (kept: 2))").unwrap())], &mut notes).death;
        assert_eq!((kept.kept, kept.warded, kept.grave), (2, 4, 1800.0), "unset death numbers stay");
    }

    #[test]
    fn made_and_worn_things_are_worth_what_they_take() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let id = |name: &str| content.blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("{name}"));
        let of = |name: &str| worth(&content, id(name));
        for name in ["dirt", "stick", "iron_ingot", "iron_pickaxe", "iron_sword", "chainmail_shirt", "wooden_shield", "bread", "hearthstone"] {
            println!("{name}: level {} worth {}", level_of(&content, id(name)), of(name));
        }
        assert!(of("iron_pickaxe") > 3 * of("iron_ingot"), "a pickaxe is worth more than its three ingots");
        assert!(of("iron_sword") >= content.value.of(20, Rarity::Common), "a sword is worth at least the level that wields it");
        assert!(of("chainmail_shirt") > of("bread") && of("chainmail_shirt") > of("dirt"));
        assert_eq!(of("gold_coin"), 1, "a coin is worth itself");
        // Old Tusk's charm: priced at his level, set in data, as nothing wears it at one.
        let tusk = content.creatures.id("base:old_tusk").map(|id| content.creatures.get(id).def.level).expect("Old Tusk");
        assert_eq!(level_of(&content, id("charge_charm")), tusk, "the charm is priced at its boss's level");
        assert_eq!(of("charge_charm"), content.value.of(tusk, Rarity::VeryRare));
        // The wiki's P51 (draft 73): raw materials count 0.5 each inside a made thing.
        let pinned = ["hearthstone", "iron_ingot", "iron_sword", "iron_pickaxe", "chainmail_shirt", "bread"].map(of);
        assert_eq!(pinned, [5, 5, 15, 24, 50, 2], "hearthstone, ingot, sword, pickaxe, chainmail shirt, bread");
        // A cured hide took ten minutes on a rack: worth more than the raw one (value.ron).
        assert_eq!([of("raw_hide"), of("hide")], [2, 3], "raw hide, cured hide");
        // A fish is worth the Fishing level that lands it, and the dish cooked of it its
        // Cooking level, the same: neither is a raw material's half.
        let fish = ["raw_trout", "cooked_trout", "raw_tuna", "cooked_tuna", "raw_glimmer_koi", "cooked_glimmer_koi", "bronze_ingot"].map(of);
        assert_eq!(fish, [5, 5, 40, 40, 2220, 2220, 4], "trout, tuna and glimmer koi, raw and cooked; a bronze ingot at its crucible's level");
        // Every item has a worth, and none is worth nothing.
        let mut known = HashMap::new();
        for item in (0..content.blocks.len() as u16).map(BlockId) {
            assert!(worth_in(&content, item, &mut known) >= 1);
        }
    }

    #[test]
    fn a_keepsake_keeps_one_more_but_never_past_most_with_the_amulet() {
        // Design d86: the amulet and the Keepsake prayer don't stack; four kept at most.
        let death = Value::default().death;
        assert_eq!([death.keeps(false, 0), death.keeps(false, 1), death.keeps(true, 0), death.keeps(true, 1)], [3, 4, 4, 4]);
        let merged = |source: &str| Value::merge(vec![("t".to_owned(), Value::read("t", source).unwrap())], &mut Vec::new());
        let most = |source: &str| merged(source).check();
        assert!(most("(death: (most: 3))").is_err(), "under what the amulet keeps");
        assert!(most("(death: (most: 5))").is_ok());
        assert_eq!(merged("(death: (most: 5))").death.keeps(true, 1), 5);
    }
}
