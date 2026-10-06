//! Hunger, warmth and the effects they have on a player: the survival side of the
//! character, shared by the server (which keeps them) and the client (which shows them).
//!
//! Hunger empties slowly as you live and faster as you sprint or shiver; food fills it.
//! The air's temperature (the `core:temperature` field), warmed by fires and torches
//! nearby and chilled by water and weather, is how warm you feel. Together with the
//! weather and what you ate they decide your effects (see [`crate::effect`], content in
//! `effects.ron`), whose modifiers set how fast you heal, hunger and get hurt.

use serde::{Deserialize, Serialize};

use crate::effect::{ActiveEffect, Modifiers};

/// Hunger points when full.
pub const MAX_HUNGER: u8 = 20;

/// How you are: hunger, how warm you feel and what that does to you.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vitals {
    pub hunger: u8,
    /// How warm you feel, in °C.
    pub felt: f32,
    /// Buffs first, then debuffs.
    pub effects: Vec<ActiveEffect>,
}

impl Default for Vitals {
    fn default() -> Self {
        Self { hunger: MAX_HUNGER, felt: 15.0, effects: Vec::new() }
    }
}

/// Felt temperature (°C) below which a fire nearby counts as warming you.
pub const CHILLY: f32 = 6.0;

/// How fast things happen under some effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    /// Hunger points lost a minute.
    pub hunger_per_minute: f32,
    /// Seconds between healing a point, if healing at all.
    pub heal_every: Option<f32>,
    /// Seconds between losing a point, if anything hurts.
    pub hurt_every: Option<f32>,
}

/// One hunger point every two and a half minutes at rest: a full stomach lasts about 50
/// minutes, a long Minecraft day and a half.
const HUNGER_PER_MINUTE: f32 = 0.4;
const HEAL_EVERY: f32 = 4.0;

/// How fast things happen under some effects' combined modifiers.
pub fn rates(modifiers: &Modifiers, sprinting: bool) -> Rates {
    let hunger = HUNGER_PER_MINUTE * if sprinting { 3.0 } else { 1.0 } * modifiers.hunger;
    let heal = (modifiers.heal > 0.0).then(|| HEAL_EVERY / modifiers.heal);
    Rates { hunger_per_minute: hunger, heal_every: heal, hurt_every: modifiers.hurt_every }
}

/// Hunger points healing a point of health costs.
pub const HEAL_COST: f32 = 0.25;

/// How much eating an item fills you, from its tags: `food:N` says it outright; otherwise
/// fruit and produce 4, roots and grain 3, berries and fungi 2, anything else edible 1.
/// Nothing for what is not food.
pub fn food_value(tags: &[String]) -> u8 {
    if let Some(value) = tags.iter().find_map(|tag| tag.strip_prefix("food:")?.parse().ok()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_follow_modifiers() {
        let calm = rates(&Modifiers::default(), false);
        assert_eq!((calm.heal_every, calm.hurt_every), (Some(HEAL_EVERY), None));
        let fed = rates(&Modifiers { heal: 2.0, ..Default::default() }, true);
        assert_eq!(fed.heal_every, Some(HEAL_EVERY / 2.0));
        assert!((fed.hunger_per_minute - 3.0 * HUNGER_PER_MINUTE).abs() < 1e-6);
        let worst = rates(&Modifiers { heal: 0.0, hunger: 2.0, hurt_every: Some(8.0), ..Default::default() }, false);
        assert_eq!((worst.heal_every, worst.hurt_every), (None, Some(8.0)));
        assert!((worst.hunger_per_minute - 2.0 * HUNGER_PER_MINUTE).abs() < 1e-6);
    }

    #[test]
    fn food_comes_from_tags() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(food_value(&tags(&["produce", "fruit", "edible"])), 4);
        assert_eq!(food_value(&tags(&["plant", "berry", "edible"])), 2);
        assert_eq!(food_value(&tags(&["flower", "edible"])), 1);
        assert_eq!(food_value(&tags(&["edible", "food:7"])), 7);
        assert_eq!(food_value(&tags(&["stone"])), 0);
    }
}
