//! Hunger, warmth and the effects they have on a player: the survival side of the
//! character, shared by the server (which keeps them) and the client (which shows them).
//!
//! Hunger empties slowly as you live and faster as you sprint or shiver; food fills it.
//! The air's temperature (the `core:temperature` field), warmed by fires and torches
//! nearby and chilled by water, is how warm you feel. Together they decide your effects:
//! each is a buff or a debuff that changes how fast you heal, hunger and get hurt. The
//! numbers are plain functions here, so they can be read, tested and later turned into
//! content.

use serde::{Deserialize, Serialize};

/// Hunger points when full.
pub const MAX_HUNGER: u8 = 20;

/// How you are: hunger, how warm you feel and what that does to you.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vitals {
    pub hunger: u8,
    /// How warm you feel, in °C.
    pub felt: f32,
    pub effects: Vec<Effect>,
}

impl Default for Vitals {
    fn default() -> Self {
        Self { hunger: MAX_HUNGER, felt: 15.0, effects: Vec::new() }
    }
}

/// Something that changes how you heal, hunger or get hurt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    /// Nearly full: you heal faster.
    WellFed,
    /// Running low: you no longer heal.
    Hungry,
    /// Empty: hunger hurts.
    Starving,
    /// Close to a fire or a torch in the cold: it keeps you warm.
    ByTheFire,
    /// Cold: you heal slower and hunger faster.
    Chilly,
    /// Bitter cold: you stop healing, hunger fast, and it hurts.
    Freezing,
    /// Hot: you heal slower and hunger a little faster.
    Hot,
    /// Heat beyond bearing: you stop healing, and it hurts.
    Overheated,
}

impl Effect {
    pub fn title(self) -> &'static str {
        match self {
            Effect::WellFed => "Well fed",
            Effect::Hungry => "Hungry",
            Effect::Starving => "Starving",
            Effect::ByTheFire => "By the fire",
            Effect::Chilly => "Chilly",
            Effect::Freezing => "Freezing",
            Effect::Hot => "Hot",
            Effect::Overheated => "Overheated",
        }
    }

    /// What it does, in a line.
    pub fn description(self) -> &'static str {
        match self {
            Effect::WellFed => "Heal twice as fast.",
            Effect::Hungry => "No healing until you eat.",
            Effect::Starving => "Lose health until you eat.",
            Effect::ByTheFire => "Warmed by a fire or torch nearby.",
            Effect::Chilly => "Heal slower, hunger faster. Find warmth.",
            Effect::Freezing => "No healing, hunger fast, lose health. Get warm.",
            Effect::Hot => "Heal slower, hunger a little faster.",
            Effect::Overheated => "No healing, lose health. Find shade or water.",
        }
    }

    pub fn is_buff(self) -> bool {
        matches!(self, Effect::WellFed | Effect::ByTheFire)
    }
}

/// Felt temperatures (°C) where the bands of warmth begin.
pub const FREEZING: f32 = -6.0;
pub const CHILLY: f32 = 6.0;
pub const HOT: f32 = 30.0;
pub const OVERHEATED: f32 = 37.0;

/// The effects of a hunger and a felt temperature, buffs first. `warmed` says a fire
/// raised what was felt.
pub fn effects(hunger: u8, felt: f32, warmed: bool) -> Vec<Effect> {
    let mut effects = Vec::new();
    if hunger >= MAX_HUNGER - 2 {
        effects.push(Effect::WellFed);
    }
    if warmed {
        effects.push(Effect::ByTheFire);
    }
    if hunger == 0 {
        effects.push(Effect::Starving);
    } else if hunger <= 6 {
        effects.push(Effect::Hungry);
    }
    if felt < FREEZING {
        effects.push(Effect::Freezing);
    } else if felt < CHILLY {
        effects.push(Effect::Chilly);
    } else if felt > OVERHEATED {
        effects.push(Effect::Overheated);
    } else if felt > HOT {
        effects.push(Effect::Hot);
    }
    effects
}

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

pub fn rates(effects: &[Effect], sprinting: bool) -> Rates {
    let mut hunger = HUNGER_PER_MINUTE * if sprinting { 3.0 } else { 1.0 };
    let mut heal = Some(HEAL_EVERY);
    let mut hurt: Option<f32> = None;
    let mut hurts = |every: f32| hurt = Some(hurt.map_or(every, |h| h.min(every)));
    for &effect in effects {
        match effect {
            Effect::WellFed => heal = heal.map(|h| h / 2.0),
            Effect::ByTheFire => {}
            Effect::Hungry => heal = None,
            Effect::Starving => {
                heal = None;
                hurts(8.0);
            }
            Effect::Chilly => {
                hunger *= 1.5;
                heal = heal.map(|h| h * 2.0);
            }
            Effect::Freezing => {
                hunger *= 2.0;
                heal = None;
                hurts(10.0);
            }
            Effect::Hot => {
                hunger *= 1.25;
                heal = heal.map(|h| h * 2.0);
            }
            Effect::Overheated => {
                heal = None;
                hurts(12.0);
            }
        }
    }
    Rates { hunger_per_minute: hunger, heal_every: heal, hurt_every: hurt }
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
    fn effects_follow_hunger_and_warmth() {
        assert_eq!(effects(20, 15.0, false), vec![Effect::WellFed]);
        assert_eq!(effects(10, 15.0, false), Vec::<Effect>::new());
        assert_eq!(effects(4, 2.0, false), vec![Effect::Hungry, Effect::Chilly]);
        assert_eq!(effects(0, -10.0, false), vec![Effect::Starving, Effect::Freezing]);
        assert_eq!(effects(12, 8.0, true), vec![Effect::ByTheFire]);
        assert_eq!(effects(12, 40.0, false), vec![Effect::Overheated]);
        assert!(Effect::WellFed.is_buff() && !Effect::Chilly.is_buff());
    }

    #[test]
    fn rates_combine() {
        let calm = rates(&[], false);
        assert_eq!((calm.heal_every, calm.hurt_every), (Some(HEAL_EVERY), None));
        let fed = rates(&[Effect::WellFed], true);
        assert_eq!(fed.heal_every, Some(HEAL_EVERY / 2.0));
        assert!((fed.hunger_per_minute - 3.0 * HUNGER_PER_MINUTE).abs() < 1e-6);
        let worst = rates(&[Effect::Starving, Effect::Freezing], false);
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
