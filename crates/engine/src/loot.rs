//! Loot: what creatures (and later blocks, products and containers) give, and how often.
//!
//! One format for every source (docs/drops.md). An entry is an item, a count range and a
//! chance: 1 for what always drops (bones, raw meat), less for what may. Tools change the
//! odds through the tags of what the actor holds (a knife takes more meat and better hides),
//! and so do levels: uncommon and rarer entries come a little more often with the skill
//! that boosts them, as crops' harvests do ([`crate::crop::level_bonus`]), never more than
//! one rarity step better than their own. Conditions (`when`) read the moment: night, a
//! player's kill.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, Rarity};

/// One thing a source may give.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loot {
    pub item: String,
    /// How many, at least and at most.
    #[serde(default = "single")]
    pub count: (u8, u8),
    /// Chance it drops at all.
    #[serde(default = "always")]
    pub chance: f32,
    #[serde(default)]
    pub rarity: Rarity,
    /// Held items with these tags change the odds.
    #[serde(default)]
    pub tools: Vec<ToolBonus>,
    /// It drops only when all of these hold.
    #[serde(default)]
    pub when: Vec<When>,
}

fn single() -> (u8, u8) {
    (1, 1)
}

fn always() -> f32 {
    1.0
}

/// How a tool changes an entry: its chance multiplied, and extra items.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBonus {
    pub tag: String,
    #[serde(default = "always")]
    pub chance: f32,
    #[serde(default)]
    pub extra: u8,
}

/// When an entry drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum When {
    /// Only at night.
    Night,
    /// Only by day.
    Day,
    /// Only when a player made the kill.
    ByPlayer,
}

/// A loot entry with its item resolved.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub item: BlockId,
    pub loot: Loot,
}

/// Resolves entries against the items (blocks) that exist.
pub fn resolve(entries: &[Loot], blocks: &BlockRegistry) -> Result<Vec<Resolved>, String> {
    entries
        .iter()
        .map(|loot| {
            let item = blocks.id(&loot.item).ok_or_else(|| format!("drops `{}`, which does not exist", loot.item))?;
            if !(0.0..=1.0).contains(&loot.chance) || loot.count.0 > loot.count.1 {
                return Err(format!("`{}`: chance must be 0..=1 and the count low..=high", loot.item));
            }
            Ok(Resolved { item, loot: loot.clone() })
        })
        .collect()
}

/// The moment a source's loot is rolled in.
#[derive(Clone, Copy, Debug, Default)]
pub struct Moment<'a> {
    /// The level bonus of the skill that boosts it (see [`crate::crop::level_bonus`]).
    pub bonus: f32,
    /// Tags of what the actor holds.
    pub tool: &'a [String],
    pub night: bool,
    pub by_player: bool,
}

/// What a rarity's tier is worth against the next: common 1, uncommon 0.35, rare 0.1,
/// very rare 0.025, legendary 0.004 (research/farming.md 12.6).
pub fn tier_factor(rarity: Rarity) -> f32 {
    match rarity {
        Rarity::Common => 1.0,
        Rarity::Uncommon => 0.35,
        Rarity::Rare => 0.1,
        Rarity::VeryRare => 0.025,
        Rarity::Legendary => 0.004,
    }
}

/// The chance of an entry in a moment: its own, times its best matching tool, times the
/// level bonus for uncommon and rarer finds, capped one rarity step better.
pub fn chance(loot: &Loot, moment: &Moment) -> f32 {
    let tool = loot.tools.iter().filter(|t| moment.tool.contains(&t.tag)).map(|t| t.chance).fold(1.0, f32::max);
    let level = if loot.rarity > Rarity::Common { 1.0 + moment.bonus } else { 1.0 };
    let raw = loot.chance * tool * level;
    let cap = match loot.rarity {
        Rarity::Common => 1.0,
        Rarity::Uncommon => loot.chance / tier_factor(Rarity::Uncommon),
        Rarity::Rare => loot.chance * tier_factor(Rarity::Uncommon) / tier_factor(Rarity::Rare),
        Rarity::VeryRare => loot.chance * tier_factor(Rarity::Rare) / tier_factor(Rarity::VeryRare),
        Rarity::Legendary => loot.chance * tier_factor(Rarity::VeryRare) / tier_factor(Rarity::Legendary),
    };
    raw.min(cap).min(1.0)
}

/// Rolls `entries` in `moment`, drawing numbers in 0..1 from `random`.
pub fn roll(entries: &[Resolved], moment: &Moment, mut random: impl FnMut() -> f32) -> Vec<(BlockId, u8)> {
    let mut found = Vec::new();
    for entry in entries {
        let loot = &entry.loot;
        let holds = loot.when.iter().all(|when| match when {
            When::Night => moment.night,
            When::Day => !moment.night,
            When::ByPlayer => moment.by_player,
        });
        if !holds || random() >= chance(loot, moment) {
            continue;
        }
        let (low, high) = loot.count;
        let mut count = low + ((random() * (high - low + 1) as f32) as u8).min(high - low);
        count += loot.tools.iter().filter(|t| moment.tool.contains(&t.tag)).map(|t| t.extra).max().unwrap_or(0);
        if loot.rarity == Rarity::Common && random() < moment.bonus {
            count += 1;
        }
        if count > 0 {
            found.push((entry.item, count));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(chance: f32, rarity: Rarity) -> Loot {
        Loot { item: "x".into(), count: (1, 1), chance, rarity, tools: vec![ToolBonus { tag: "knife".into(), chance: 2.0, extra: 1 }], when: vec![] }
    }

    #[test]
    fn tools_and_levels_raise_chances_but_never_past_one_step() {
        let knife = ["knife".to_owned()];
        let bare = Moment::default();
        let skilled = Moment { bonus: 0.5, tool: &knife, ..Default::default() };
        let rare = entry(0.02, Rarity::Rare);
        assert_eq!(chance(&rare, &bare), 0.02);
        // ×2 knife ×1.5 level = 0.06, capped at the uncommon rate 0.02 × 0.35 / 0.1 = 0.07.
        assert!((chance(&rare, &skilled) - 0.06).abs() < 1e-6);
        let legendary = entry(0.001, Rarity::Legendary);
        // ×3 would be 0.003; very rare's rate caps it at 0.001 × 0.025 / 0.004 = 0.00625.
        assert!((chance(&legendary, &skilled) - 0.003).abs() < 1e-6);
        let common = entry(0.6, Rarity::Common);
        // Commons take the tool but not the level bonus.
        assert!((chance(&common, &skilled) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rolls_count_tools_extras_and_conditions() {
        let meat = Resolved { item: BlockId(5), loot: Loot { count: (1, 3), ..entry(1.0, Rarity::Common) } };
        let moon = Resolved { item: BlockId(6), loot: Loot { when: vec![When::Night], ..entry(1.0, Rarity::Rare) } };
        let knife = ["knife".to_owned()];
        // Low draws: every chance taken, bottom of every count; the knife adds one.
        let day = Moment { tool: &knife, ..Default::default() };
        assert_eq!(roll(&[meat.clone(), moon.clone()], &day, || 0.0), vec![(BlockId(5), 2)]);
        let night = Moment { night: true, ..Default::default() };
        assert_eq!(roll(&[meat, moon], &night, || 0.0), vec![(BlockId(5), 1), (BlockId(6), 1)]);
    }
}
