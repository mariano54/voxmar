//! Taming, keeping and breeding (docs/breeding.md): how a wild creature comes to trust a
//! player, what a kept one gives on timers, and how two kept adults make a young one.
//!
//! This first slice has the simple layer of each. Taming is by feeding: each favourite
//! food a player gives raises the creature's trust in them by a seeded amount; at full
//! trust it is theirs (Hunter, RimWorld's taming by feeding). Products come on timers,
//! laid on the ground (eggs) or taken with a tool (wool with shears, milk with a pail),
//! the due-time model crops use. Breeding is Minecraft's: two kept adults fed their breeding
//! food come together and a young one is born, which grows up over minutes (Farming). Genes,
//! condition, trust decay and treatment come next.

use serde::Deserialize;

/// How a species is tamed.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taming {
    /// What it accepts from a hand, by item name or `tag:`; its favourite foods unless given.
    #[serde(default)]
    pub food: Vec<String>,
    /// The Hunter level it takes.
    #[serde(default = "first")]
    pub level: u8,
    /// Trust each feeding gives, least and most (1 is tame).
    #[serde(default = "per_feed")]
    pub per_feed: (f32, f32),
    /// Hunter experience for taming one.
    pub xp: f32,
}

/// How a kept species breeds.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Breeding {
    /// What makes it ready to breed; its favourite foods unless given.
    #[serde(default)]
    pub food: Vec<String>,
    /// The Farming level it takes.
    #[serde(default = "first")]
    pub level: u8,
    /// Young born at once, least and most.
    #[serde(default = "one_young")]
    pub litter: (u8, u8),
    /// Real minutes for the young to grow up.
    #[serde(default = "growth")]
    pub growth: f32,
    /// Real minutes before a parent breeds again.
    #[serde(default = "cooldown")]
    pub cooldown: f32,
    /// Farming experience for each birth.
    pub xp: f32,
}

/// Something a kept creature gives over and over.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub item: String,
    #[serde(default = "one_each")]
    pub count: (u8, u8),
    /// Real minutes between one and the next.
    pub every: f32,
    /// The tag of the tool that takes it (`shears`, `pail`); none for what it leaves on the
    /// ground by itself (eggs).
    #[serde(default)]
    pub tool: Option<String>,
    /// Farming experience for taking it.
    #[serde(default)]
    pub xp: f32,
}

fn first() -> u8 {
    1
}

fn per_feed() -> (f32, f32) {
    (0.2, 0.45)
}

fn one_young() -> (u8, u8) {
    (1, 1)
}

fn one_each() -> (u8, u8) {
    (1, 1)
}

fn growth() -> f32 {
    10.0
}

fn cooldown() -> f32 {
    5.0
}

/// How long a fed, kept adult stays ready to breed, in seconds.
pub const IN_LOVE_SECONDS: f32 = 30.0;
/// How close two ready adults must be to breed, in blocks.
pub const MATE_RANGE: f32 = 10.0;
/// A young one's size against an adult's.
pub const YOUNG_SCALE: f32 = 0.55;
/// How far a kept creature strolls from its home, and how far it strays before it heads
/// back: farm animals stay where they are kept, with or without a fence.
pub const KEPT_ROAM: f32 = 4.0;
pub const KEPT_LEASH: f32 = 7.0;

/// The trust a feeding gives, from a draw `u` in 0..1.
pub fn trust_gain(taming: &Taming, u: f32) -> f32 {
    let (low, high) = taming.per_feed;
    low + (high - low) * u.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeding_raises_trust_within_its_range() {
        let taming = Taming { food: vec![], level: 1, per_feed: (0.2, 0.4), xp: 10.0 };
        assert_eq!(trust_gain(&taming, 0.0), 0.2);
        assert!((trust_gain(&taming, 1.0) - 0.4).abs() < 1e-6);
        // Between three and five feedings to tame.
        assert!((1.0 / 0.4f32).ceil() as u32 == 3 && (1.0 / 0.2f32).ceil() as u32 == 5);
    }
}
