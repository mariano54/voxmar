//! Biome levels (P15): how harsh, rare and aether-touched each biome is, 1 ..= 10.
//!
//! A level does two things. It makes a biome's rarer finds commoner, never exclusive:
//! each rarity has a home level (in the base data commons are at home at level 1 and
//! legendaries at 9), and up to it every level a biome stands makes a find `step` times
//! commoner (the wiki's about 1.4×), so a level-6 biome is the place for rare things that
//! still turn up, less often, in a meadow. Above its home a find keeps `earlier` of its
//! home rate (about two thirds, Mariano, 7 Oct), so a high-level biome still holds the
//! gentle biomes' commons beside its own rarer finds and nobody walks back for basics.
//! Level 1 is the data's own rates: the gentle land keeps the look it was tuned to. Levels
//! scale a biome's plants and, of its trees, only those `trees` names by tag (the fruit, nut
//! and berry trees, the forest's finds): the canopy's own kinds keep their shares, so a
//! level-3 taiga stays spruce and pine rather than turning to larch.
//!
//! And distance from spawn makes higher-level biomes likelier, never certain. The biome
//! picker weighs each claim by `lean` to the power of its level above 1 times the lean of
//! the place, which grows with distance from the origin (where players start) up to
//! `reach` blocks and is scaled by a slow wildness field, 0 ..= 2: some far lands stay
//! gentle and some near ones are already wild. At the origin the lean is nothing, so near
//! spawn every biome keeps its plain odds. This replaces the plan's remoteness package
//! (rare biomes about +4% per km up to +60%): at full lean a level-4 biome is 1.17³ ≈ 1.6
//! times likelier.

use serde::Deserialize;

use crate::rarity::{Rarity, Tier};

/// The highest level a biome may have.
pub const MAX_LEVEL: u8 = 10;

/// How biome levels scale finds and how distance leans the biome picker (`climate.ron`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LevelsDef {
    /// How much commoner each level of a biome, up to a find's home level, makes the find.
    pub step: f32,
    /// The share of its home rate a find keeps in biomes above its home level.
    pub earlier: f32,
    /// The level at which finds of each rarity are at home.
    pub homes: Homes,
    /// At full lean, how much likelier each level above 1 makes a biome.
    pub lean: f32,
    /// Distance from the origin, in blocks, at which the lean is full.
    pub reach: f32,
    /// The wavelength of the wildness field that scales the lean, in blocks.
    pub wildness: f32,
    /// Tags of the trees levels scale (finds); other trees keep their shares.
    pub trees: Vec<String>,
}

impl Default for LevelsDef {
    fn default() -> Self {
        Self { step: 1.4, earlier: 0.67, homes: Homes { common: 1, uncommon: 3, rare: 5, very_rare: 7, legendary: 9 }, lean: 1.17, reach: 15000.0, wildness: 3000.0, trees: vec!["gathering".into()] }
    }
}

/// The level at which finds of each rarity are at home.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Homes {
    pub common: u8,
    pub uncommon: u8,
    pub rare: u8,
    pub very_rare: u8,
    pub legendary: u8,
}

impl Homes {
    pub fn of(&self, rarity: Rarity) -> u8 {
        self.all()[rarity as usize]
    }

    fn all(&self) -> [u8; 5] {
        [self.common, self.uncommon, self.rare, self.very_rare, self.legendary]
    }
}

impl LevelsDef {
    /// The multiplier a biome of `level` gives a find of `rarity`: 1 at level 1, `step`
    /// more per level up to the find's home level, and `earlier` of that above it.
    pub fn finds(&self, level: u8, rarity: Rarity) -> f32 {
        let home = self.homes.of(rarity).max(1);
        let level = level.max(1);
        let rise = self.step.powi(i32::from(level.min(home)) - 1);
        if level > home { rise * self.earlier } else { rise }
    }

    /// How strongly distance favours higher-level biomes at `distance` blocks from the
    /// origin, where the wildness field (evenly spread over 0 ..= 1) is `wild`: 0 ..= 2.
    pub fn lean_at(&self, distance: f32, wild: f32) -> f32 {
        (distance / self.reach).min(1.0) * 2.0 * wild
    }

    /// The weight each level, 0 ..= `MAX_LEVEL`, gives a biome's claim where the lean is
    /// `lean`: 1 at level 1 (and 0), `lean`'s power of the level above 1 beyond it.
    pub fn leans(&self, lean: f32) -> Leans {
        let mut out = [1.0; MAX_LEVEL as usize + 1];
        if lean > 0.0 {
            let per = (lean * self.lean.ln()).exp();
            for level in 2..out.len() {
                out[level] = out[level - 1] * per;
            }
        }
        Leans(out)
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        let homes = self.homes.all();
        let homes = homes.windows(2).all(|w| w[0] <= w[1]) && homes.iter().all(|h| (1..=MAX_LEVEL).contains(h));
        if !(self.step >= 1.0 && self.earlier > 0.0 && self.earlier <= 1.0 && homes && self.lean >= 1.0 && self.reach > 0.0 && self.wildness > 0.0) {
            return Err(format!("levels: {self:?} needs step and lean at least 1, earlier in 0 ..= 1, homes rising within 1 ..= {MAX_LEVEL}, reach and wildness above 0"));
        }
        Ok(())
    }
}

/// A claim's weight by its biome's level at one place (`LevelsDef::leans`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leans([f32; MAX_LEVEL as usize + 1]);

impl Leans {
    /// No lean: every level weighs the same.
    pub const NONE: Leans = Leans([1.0; MAX_LEVEL as usize + 1]);

    pub fn of(&self, level: u8) -> f32 {
        self.0[(level as usize).min(MAX_LEVEL as usize)]
    }
}

/// A find's rarity multiplier with every boost on it (the world's featured boost, its
/// biome's level), kept within two tiers of its own rarity: a featured legendary in a
/// high-level biome is a rare find, never a weed.
pub fn capped(rarity: Rarity, multiplier: f32) -> f32 {
    multiplier.min(rarity.commoner(2).multiplier())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSES: [Rarity; 5] = [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::VeryRare, Rarity::Legendary];

    /// The wiki's rules (draft 39, Mariano 7 Oct): rarer finds about 1.4× commoner per
    /// level up to their home, earlier tiers at about two thirds above it, never nothing,
    /// and level 1 the data's own rates. Only commons, at home at level 1, grow scarcer
    /// than there in a higher biome (two thirds, as the wiki asks); every rarer find is at
    /// least as common in any higher biome as at level 1.
    #[test]
    fn levels_make_rarer_finds_commoner() {
        let levels = LevelsDef::default();
        for rarity in CLASSES {
            assert_eq!(levels.finds(1, rarity), 1.0, "{rarity:?} at level 1");
            let home = levels.homes.of(rarity);
            for level in 2..=MAX_LEVEL {
                let (below, here) = (levels.finds(level - 1, rarity), levels.finds(level, rarity));
                assert!(here > 0.0);
                if rarity == Rarity::Common {
                    assert_eq!(here, 0.67);
                } else {
                    assert!(here >= 1.0, "{rarity:?} at level {level}: ×{here}, rarer than at level 1");
                }
                if level <= home {
                    assert!((here / below - 1.4).abs() < 1e-4, "{rarity:?} from level {} to {level}: {below} to {here}", level - 1);
                } else {
                    let at_home = levels.finds(home, rarity);
                    assert!((here / at_home - 0.67).abs() < 1e-4, "{rarity:?} at level {level} keeps {here} of {at_home}");
                }
            }
        }
    }

    /// At every level a rarer find stays rarer than a commoner one, so levels shift the
    /// mix without turning it over.
    #[test]
    fn levels_keep_the_rarity_order() {
        let levels = LevelsDef::default();
        for level in 1..=MAX_LEVEL {
            let rates: Vec<f32> = CLASSES.iter().map(|&r| capped(r, r.multiplier() * levels.finds(level, r))).collect();
            assert!(rates.windows(2).all(|w| w[0] > w[1]), "level {level}: {rates:?}");
        }
        // Boosts stop two tiers up.
        assert_eq!(capped(Rarity::Legendary, 1.0), Rarity::Rare.multiplier());
        assert_eq!(capped(Rarity::Common, 0.67), 0.67);
    }

    #[test]
    fn distance_leans_towards_higher_levels() {
        let levels = LevelsDef::default();
        assert_eq!(levels.lean_at(0.0, 0.9), 0.0);
        assert_eq!(levels.leans(0.0), Leans::NONE);
        // At full reach and middling wildness a level-10 biome is about four times likelier
        // than a level-1 one, and a level-4 one about 1.6 times (the old remoteness curve).
        let full = levels.leans(levels.lean_at(levels.reach * 2.0, 0.5));
        assert!((full.of(10) - 1.17f32.powi(9)).abs() < 1e-3 && (full.of(4) - 1.6).abs() < 0.01, "{full:?}");
        assert_eq!(full.of(1), 1.0);
        assert!(levels.check().is_ok());
        let backwards = Homes { common: 3, uncommon: 1, ..LevelsDef::default().homes };
        assert!(LevelsDef { homes: backwards, ..LevelsDef::default() }.check().is_err());
    }
}
