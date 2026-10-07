//! Warmth, food and the effects they have on a player: the survival side of the
//! character, shared by the server (which keeps them) and the client (which shows them).
//!
//! There is no hunger (Mariano, 2026-10-06): food heals. Health comes back slowly by
//! itself; eating is how you heal in a hurry, and eating the same thing over and over
//! heals a little less each time, so a varied pack is worth carrying (see [`Recent`]).
//! The air's temperature (the `core:temperature` field), warmed by fires and torches
//! nearby and chilled by water and weather, is how warm you feel. Together with the
//! weather and what you ate it decides your effects (see [`crate::effect`], content in
//! `effects.ron`), whose modifiers set how fast you heal and get hurt.

use serde::{Deserialize, Serialize};

use crate::block::BlockId;
use crate::effect::{ActiveEffect, Modifiers};

/// How you are: how warm you feel and what that, the weather and food do to you.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vitals {
    /// How warm you feel, in °C.
    pub felt: f32,
    /// Buffs first, then debuffs.
    pub effects: Vec<ActiveEffect>,
}

impl Default for Vitals {
    fn default() -> Self {
        Self { felt: 15.0, effects: Vec::new() }
    }
}

/// Felt temperature (°C) below which a fire nearby counts as warming you.
pub const CHILLY: f32 = 6.0;

/// How fast things happen under some effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    /// Seconds between healing a point, if healing at all.
    pub heal_every: Option<f32>,
    /// Seconds between losing a point, if anything hurts.
    pub hurt_every: Option<f32>,
}

/// Seconds between healing a point by itself: slow, as RuneScape's, so food matters.
const HEAL_EVERY: f32 = 10.0;

/// How fast things happen under some effects' combined modifiers.
pub fn rates(modifiers: &Modifiers) -> Rates {
    let heal = (modifiers.heal > 0.0).then(|| HEAL_EVERY / modifiers.heal);
    Rates { heal_every: heal, hurt_every: modifiers.hurt_every }
}

/// How much eating an item heals, from its tags: `heal:N` says it outright (cooked food,
/// by the level that cooks it); otherwise fruit and berries 2 and anything else edible 1,
/// raw produce and meat among it (design draft 63). Nothing for what is not food.
pub fn food_value(tags: &[String]) -> u8 {
    if let Some(value) = tags.iter().find_map(|tag| tag.strip_prefix("heal:")?.parse::<u8>().ok()) {
        return value;
    }
    let has = |tag: &str| tags.iter().any(|t| t == tag);
    if !has("edible") {
        0
    } else if has("fruit") || has("berry") {
        2
    } else {
        1
    }
}

/// What cooked food heals by the level that cooks it: 0.4 × that level, at least 2 (design
/// draft 55). Wonder fruit heals so by the Farming level that picks it.
pub fn cooked_heal(level: u8) -> u8 {
    ((0.4 * level as f32).round() as u8).max(2)
}

/// What a player ate lately, for how much the same food heals again: each time in a row
/// takes a tenth off, down to half; eating something else, or a minute passing, gives a
/// tenth back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recent {
    /// Each food eaten lately and how many tenths it has lost.
    eaten: Vec<(BlockId, u8)>,
}

/// The most tenths the same food loses.
const MOST_LOST: u8 = 5;

impl Recent {
    /// The share of its healing `food` gives now.
    pub fn share(&self, food: BlockId) -> f32 {
        let lost = self.eaten.iter().find(|(f, _)| *f == food).map_or(0, |&(_, lost)| lost);
        1.0 - lost as f32 / 10.0
    }

    /// Eats `food`: it loses a tenth for next time, and every other food gets one back.
    pub fn eat(&mut self, food: BlockId) {
        for (f, lost) in &mut self.eaten {
            if *f != food {
                *lost = lost.saturating_sub(1);
            }
        }
        match self.eaten.iter_mut().find(|(f, _)| *f == food) {
            Some((_, lost)) => *lost = (*lost + 1).min(MOST_LOST),
            None => self.eaten.push((food, 1)),
        }
        self.eaten.retain(|&(_, lost)| lost > 0);
    }

    /// A minute passes: every food gets a tenth back.
    pub fn recover(&mut self) {
        for (_, lost) in &mut self.eaten {
            *lost = lost.saturating_sub(1);
        }
        self.eaten.retain(|&(_, lost)| lost > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_follow_modifiers() {
        let calm = rates(&Modifiers::default());
        assert_eq!((calm.heal_every, calm.hurt_every), (Some(HEAL_EVERY), None));
        let soothed = rates(&Modifiers { heal: 2.0, ..Default::default() });
        assert_eq!(soothed.heal_every, Some(HEAL_EVERY / 2.0));
        let worst = rates(&Modifiers { heal: 0.0, hurt_every: Some(8.0), ..Default::default() });
        assert_eq!((worst.heal_every, worst.hurt_every), (None, Some(8.0)));
    }

    #[test]
    fn food_comes_from_tags() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(food_value(&tags(&["plant", "berry", "edible"])), 2);
        assert_eq!(food_value(&tags(&["flower", "edible"])), 1);
        assert_eq!(food_value(&tags(&["edible", "heal:7"])), 7);
        assert_eq!(food_value(&tags(&["edible", "fruit", "produce"])), 2);
        assert_eq!(food_value(&tags(&["edible", "meat", "raw"])), 1);
        assert_eq!((cooked_heal(1), cooked_heal(12), cooked_heal(65)), (2, 5, 26));
        assert_eq!(food_value(&tags(&["stone"])), 0);
    }

    #[test]
    fn the_same_food_heals_less_and_variety_brings_it_back() {
        let (bread, fish) = (BlockId(1), BlockId(2));
        let mut recent = Recent::default();
        assert_eq!(recent.share(bread), 1.0);
        for _ in 0..8 {
            recent.eat(bread);
        }
        assert_eq!(recent.share(bread), 0.5, "floored at half");
        recent.eat(fish);
        recent.eat(fish);
        assert!((recent.share(bread) - 0.7).abs() < 1e-6);
        assert!((recent.share(fish) - 0.8).abs() < 1e-6);
        recent.recover();
        assert!((recent.share(fish) - 0.9).abs() < 1e-6);
    }
}
