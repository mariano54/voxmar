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

/// How much eating an item heals, from its tags: `heal:N` or `food:N` (the larger) say it
/// outright; otherwise fruit and produce 4, roots and grain 3, berries and fungi 2,
/// anything else edible 1. Nothing for what is not food.
pub fn food_value(tags: &[String]) -> u8 {
    let tagged = |prefix: &str| tags.iter().find_map(|tag| tag.strip_prefix(prefix)?.parse::<u8>().ok());
    if let Some(value) = tagged("heal:").max(tagged("food:")) {
        return value;
    }
    let has = |tag: &str| tags.iter().any(|t| t == tag);
    if !has("edible") {
        0
    } else if has("fruit") || has("produce") {
        4
    } else if has("root") || has("grain") {
        3
    } else if has("berry") || has("fungus") {
        2
    } else {
        1
    }
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
        assert_eq!(food_value(&tags(&["produce", "fruit", "edible"])), 4);
        assert_eq!(food_value(&tags(&["plant", "berry", "edible"])), 2);
        assert_eq!(food_value(&tags(&["flower", "edible"])), 1);
        assert_eq!(food_value(&tags(&["edible", "food:7"])), 7);
        assert_eq!(food_value(&tags(&["edible", "food:5", "heal:4"])), 5);
        assert_eq!(food_value(&tags(&["edible", "food:3", "heal:6"])), 6);
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
