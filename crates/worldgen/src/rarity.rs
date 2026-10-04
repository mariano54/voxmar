//! Rarity, and what each world features.
//!
//! Biomes, variants, trees, plants and wonders each carry a rarity tier whose multiplier
//! scales how often they occur, so a legendary flower is a real find and an uncommon one
//! merely a pleasant surprise. The tiers are spread far apart on purpose: each is several
//! times rarer than the one before.
//!
//! On top of that, every world features a few things from its seed: it boosts a few rare
//! ones by a tier or two and suppresses a few common ones, so no two worlds share a rarity
//! table and players can't fully predict what they will find. The choice depends only on
//! the seed, the kind of thing and its name, so any system (biomes, plants, weather
//! effects) can ask for its own list without a central registry.

use serde::Deserialize;

use crate::rng::{hash3, mix};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
    VeryRare,
    Legendary,
}

impl Rarity {
    pub const ALL: [Rarity; 5] = [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::VeryRare, Rarity::Legendary];

    /// How often something of this rarity occurs relative to a common one.
    pub fn multiplier(self) -> f32 {
        match self {
            Rarity::Common => 1.0,
            Rarity::Uncommon => 0.3,
            Rarity::Rare => 0.08,
            Rarity::VeryRare => 0.015,
            Rarity::Legendary => 0.002,
        }
    }

    /// The tier `steps` more common than this one, stopping at common.
    fn commoner(self, steps: usize) -> Rarity {
        Self::ALL[(self as usize).saturating_sub(steps)]
    }
}

/// Multiplier applied to a suppressed common thing: it becomes about as scarce as an
/// uncommon one.
const SUPPRESSED: f32 = 0.3;

/// What a world features, drawn from its seed. Copy it freely; it is just the seed and
/// the world's biodiversity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Featured {
    seed: u64,
    /// 0 ..= 1: biodiverse worlds feature more things.
    biodiversity: f32,
}

impl Featured {
    pub fn new(seed: u64, biodiversity: f32) -> Self {
        Self { seed: mix(seed ^ 0xfea7), biodiversity }
    }

    /// The multiplier this world applies to each of `items` (name and rarity) of one
    /// `kind` ("biome", "plant", "weather" and so on), in the order given.
    ///
    /// About a tenth to a quarter of the uncommon-or-rarer items (at least one) are
    /// boosted one or two tiers, and about a tenth of the common ones (at least one, given
    /// four or more) are suppressed. Which ones depends only on the seed, the kind and the
    /// names, not on the order of `items`.
    pub fn multipliers(&self, kind: &str, items: &[(&str, Rarity)]) -> Vec<f32> {
        let rank = |salt: u64, name: &str| hash3(self.seed ^ salt ^ name_hash(kind), name_hash(name) as i32, 0, (name_hash(name) >> 32) as i32);
        let mut out = vec![1.0; items.len()];

        let mut rare: Vec<usize> = (0..items.len()).filter(|&i| items[i].1 != Rarity::Common).collect();
        rare.sort_by_key(|&i| (rank(1, items[i].0), i));
        let boosted = ((rare.len() as f32 * (0.1 + 0.15 * self.biodiversity)).round() as usize).max(1).min(rare.len());
        for &i in &rare[..boosted] {
            let rarity = items[i].1;
            let steps = 1 + (rank(2, items[i].0) % 2) as usize;
            out[i] = rarity.commoner(steps).multiplier() / rarity.multiplier();
        }

        let mut common: Vec<usize> = (0..items.len()).filter(|&i| items[i].1 == Rarity::Common).collect();
        common.sort_by_key(|&i| (rank(3, items[i].0), i));
        let suppressed = if common.len() >= 4 { ((common.len() as f32 * 0.1).round() as usize).max(1) } else { 0 };
        for &i in &common[..suppressed] {
            out[i] = SUPPRESSED;
        }
        out
    }
}

/// FNV-1a, for stable hashes of names.
pub fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<(String, Rarity)> {
        (0..40).map(|i| (format!("base:thing_{i}"), Rarity::ALL[i % 5])).collect()
    }

    fn featured(seed: u64) -> Vec<f32> {
        let items = items();
        let items: Vec<(&str, Rarity)> = items.iter().map(|(name, rarity)| (name.as_str(), *rarity)).collect();
        Featured::new(seed, 0.5).multipliers("plant", &items)
    }

    #[test]
    fn worlds_feature_different_things_within_bounds() {
        assert_eq!(featured(1), featured(1));
        assert_ne!(featured(1), featured(2));
        let items = items();
        for seed in 0..50 {
            let multipliers = featured(seed);
            let boosted = multipliers.iter().filter(|&&m| m > 1.0).count();
            let suppressed = multipliers.iter().filter(|&&m| m < 1.0).count();
            assert!((3..=8).contains(&boosted) && suppressed == 1, "seed {seed}: {multipliers:?}");
            for ((_, rarity), m) in items.iter().zip(&multipliers) {
                // A boost lifts by at most two tiers; nothing common is boosted.
                let boosted = rarity.multiplier() * m;
                assert!(boosted <= rarity.commoner(2).multiplier() + 1e-6 && (*rarity != Rarity::Common || *m <= 1.0));
            }
        }
    }

    #[test]
    fn featured_ignores_order() {
        let items = items();
        let mut forward: Vec<(&str, Rarity)> = items.iter().map(|(name, rarity)| (name.as_str(), *rarity)).collect();
        let a = Featured::new(9, 0.3).multipliers("biome", &forward);
        forward.reverse();
        let mut b = Featured::new(9, 0.3).multipliers("biome", &forward);
        b.reverse();
        assert_eq!(a, b);
    }
}
