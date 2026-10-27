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
    /// Where home is, the block of its lit hearth (else the bed last slept in), if the
    /// player has one: what the interface points the way back to.
    #[serde(default)]
    pub home: Option<glam::IVec3>,
    /// The seat the player sits on, if they sit: the eye is lower (`eye`).
    #[serde(default)]
    pub seated: Option<glam::IVec3>,
    /// What the stores in the player's home hold, by item, while they stand in it (empty
    /// elsewhere): what they can make there counts it, as laying a recipe takes from it.
    #[serde(default)]
    pub home_stores: Vec<(BlockId, u32)>,
}

impl Default for Vitals {
    fn default() -> Self {
        Self { felt: 15.0, effects: Vec::new(), home: None, seated: None, home_stores: Vec::new() }
    }
}

/// How high a seated player's eye is over their feet: a stool's top and a body sat on it,
/// a third of a block under a standing eye.
pub const SEATED_EYE: f32 = 1.3;
/// How far across from their seat's middle a player may be and still sit on it.
pub const SEATED_WITHIN: f32 = 0.45;

/// The tag of a seat that rocks the view of whoever sits in it (a rocking chair).
pub const ROCKS: &str = "rocks";
/// Seconds of one rock, forward and back.
pub const ROCK_PERIOD: f32 = 3.2;
/// How far the view tips forward and back on a rocking seat, radians (about 2°).
pub const ROCK_PITCH: f32 = 0.035;
/// How far the eye dips at either end of a rock, blocks.
pub const ROCK_LIFT: f32 = 0.04;

/// How a rocking seat moves the view `seconds` into sitting: the tip forward (positive)
/// or back, and how far the eye dips, still at the start and round again each period.
pub fn rock(seconds: f32) -> (f32, f32) {
    let phase = std::f32::consts::TAU * seconds / ROCK_PERIOD;
    (ROCK_PITCH * phase.sin(), -ROCK_LIFT * phase.sin().powi(2))
}

/// Where a player sitting on the seat at `seat` stands: in one that is not solid (base's
/// stools, chairs, benches and cushions), on top of a solid one (a mod's), in its middle.
pub fn seated_feet(seat: glam::IVec3, solid: bool) -> glam::Vec3 {
    seat.as_vec3() + glam::Vec3::new(0.5, if solid { 1.0 } else { 0.0 }, 0.5)
}

/// Where the eye is of a player whose feet are at `feet`, standing at `standing` above
/// them: lower while they sit on `seated` and are still on it.
pub fn eye(feet: glam::Vec3, standing: f32, seated: Option<glam::IVec3>) -> glam::Vec3 {
    match seated.filter(|&seat| still_seated(feet, seat)) {
        Some(_) => feet + glam::Vec3::Y * SEATED_EYE,
        None => feet + glam::Vec3::Y * standing,
    }
}

/// Whether a player whose feet are at `feet` is still on the seat at `seat`: over its
/// middle and on it, not stepped off.
pub fn still_seated(feet: glam::Vec3, seat: glam::IVec3) -> bool {
    let middle = seat.as_vec3() + glam::Vec3::splat(0.5);
    (feet.x - middle.x).abs() <= SEATED_WITHIN && (feet.z - middle.z).abs() <= SEATED_WITHIN && (seat.y as f32 - 0.1..=seat.y as f32 + 1.2).contains(&feet.y)
}

/// Felt temperature (°C) below which a fire nearby counts as warming you.
pub const CHILLY: f32 = 6.0;

/// Heat (°C) from fires and glowing blocks near above which they warm a player at all.
pub const WARMING: f32 = 0.5;

/// Whether the heat near warms a player in `air` (°C, after the wet's chill): only in the
/// cold, below `CHILLY`, and only with some heat. Effects ask it (`Condition::Warmed`).
pub fn warmed(heat: f32, air: f32) -> bool {
    heat > WARMING && air < CHILLY
}

/// How warm a player feels (°C) before effects: the air, after the wet's chill, and the
/// heat near.
pub fn felt_air(air: f32, heat: f32) -> f32 {
    air + heat
}

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

/// How fast things happen under some effects' combined modifiers, and `boost` on healing
/// besides (a prayer's): healing by itself comes no faster than `heal_most` points a second
/// (`survival.ron`), however many buffs stack, as `skill::MAX_XP_BOOST` caps experience.
pub fn rates(modifiers: &Modifiers, boost: f32, heal_most: f32) -> Rates {
    let heal = modifiers.heal * boost;
    let heal_every = (heal > 0.0).then(|| (HEAL_EVERY / heal).max(1.0 / heal_most));
    Rates { heal_every, hurt_every: modifiers.hurt_every }
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

/// What cooked food heals by the level that cooks it: 0.4 × that level, at least 4 (design
/// drafts 55 and 107). Wonder fruit heals 0.4 × the Farming level that picks it, at least 2.
pub fn cooked_heal(level: u8) -> u8 {
    ((0.4 * level as f32).round() as u8).max(4)
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

    /// Sat in a rocking chair the view rocks gently: still at the start, back and forth
    /// within its pitch and lift, and round again each period.
    #[test]
    fn a_rocking_seat_rocks_the_view_gently() {
        assert_eq!(rock(0.0), (0.0, 0.0));
        for step in 0..200 {
            let t = step as f32 * 0.05;
            let (pitch, lift) = rock(t);
            assert!(pitch.abs() <= ROCK_PITCH + 1e-6 && lift.abs() <= ROCK_LIFT + 1e-6, "{t}: {pitch} {lift}");
            let (again, _) = rock(t + ROCK_PERIOD);
            assert!((again - pitch).abs() < 1e-4, "{t}: not round again");
        }
        let (forward, _) = rock(ROCK_PERIOD / 4.0);
        let (back, _) = rock(ROCK_PERIOD * 3.0 / 4.0);
        assert!(forward > 0.9 * ROCK_PITCH && back < -0.9 * ROCK_PITCH, "back and forth: {forward} {back}");
    }

    #[test]
    fn rates_follow_modifiers() {
        let calm = rates(&Modifiers::default(), 1.0, 1.0);
        assert_eq!((calm.heal_every, calm.hurt_every), (Some(HEAL_EVERY), None));
        let soothed = rates(&Modifiers { heal: 2.0, ..Default::default() }, 1.0, 1.0);
        assert_eq!(soothed.heal_every, Some(HEAL_EVERY / 2.0));
        let prayed = rates(&Modifiers { heal: 2.0, ..Default::default() }, 2.0, 1.0);
        assert_eq!(prayed.heal_every, Some(HEAL_EVERY / 4.0));
        let worst = rates(&Modifiers { heal: 0.0, hurt_every: Some(8.0), ..Default::default() }, 2.0, 1.0);
        assert_eq!((worst.heal_every, worst.hurt_every), (None, Some(8.0)));
        // However much stacks, no faster than `heal_most` a second.
        let stacked = rates(&Modifiers { heal: 40.0, ..Default::default() }, 2.0, 1.0);
        assert_eq!(stacked.heal_every, Some(1.0));
        assert_eq!(rates(&Modifiers { heal: 40.0, ..Default::default() }, 1.0, 0.5).heal_every, Some(2.0));
    }

    #[test]
    fn food_comes_from_tags() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(food_value(&tags(&["plant", "berry", "edible"])), 2);
        assert_eq!(food_value(&tags(&["flower", "edible"])), 1);
        assert_eq!(food_value(&tags(&["edible", "heal:7"])), 7);
        assert_eq!(food_value(&tags(&["edible", "fruit", "produce"])), 2);
        assert_eq!(food_value(&tags(&["edible", "meat", "raw"])), 1);
        assert_eq!((cooked_heal(1), cooked_heal(12), cooked_heal(65)), (4, 5, 26));
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
