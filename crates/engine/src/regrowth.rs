//! Wild plants that come back where they were picked, as content (`regrowth.ron`).
//!
//! Flowers, herbs, fungi and berry bushes the world grew regrow where they were picked, as
//! long as the block they grew from stays; break or replace that ground and the spot is
//! gone. How long they take follows their rarity: commons in tens of minutes, very rares
//! over days. One a player placed does not come back, unless it stands in a herb planter
//! and the player's Farming level is enough for it: herbs cannot be sown, so a planter is
//! how a herb garden is kept.

use serde::Deserialize;

use crate::block::{BlockDef, BlockId, BlockRegistry, Rarity, RegistryError};

/// One number for each rarity.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByRarity {
    pub common: f32,
    pub uncommon: f32,
    pub rare: f32,
    pub very_rare: f32,
    pub legendary: f32,
}

impl ByRarity {
    pub fn of(&self, rarity: Rarity) -> f32 {
        match rarity {
            Rarity::Common => self.common,
            Rarity::Uncommon => self.uncommon,
            Rarity::Rare => self.rare,
            Rarity::VeryRare => self.very_rare,
            Rarity::Legendary => self.legendary,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegrowthFile {
    /// A plant (not solid) with any of these tags regrows.
    pub tags: Vec<String>,
    /// Minutes of world time to come back.
    pub minutes: ByRarity,
    /// The block a herb garden is planted in.
    pub planter: String,
    /// The Farming level a plant of each rarity asks to grow back in a planter, and the
    /// experience picking it there gives.
    pub planter_level: ByRarity,
    pub planter_xp: ByRarity,
}

#[derive(Clone, Debug, Default)]
pub struct Regrowth {
    pub rules: RegrowthFile,
    pub planter: Option<BlockId>,
}

impl Regrowth {
    pub fn read(namespace: &str, source: &str) -> Result<RegrowthFile, RegistryError> {
        let mut file: RegrowthFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/regrowth.ron: {e}")))?;
        if !file.planter.is_empty() && !file.planter.contains(':') {
            file.planter = format!("{namespace}:{}", file.planter);
        }
        Ok(file)
    }

    /// The last package's rules win, with tags gathered from all of them.
    pub fn new(files: Vec<RegrowthFile>, blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut tags = Vec::new();
        let mut rules = RegrowthFile::default();
        for file in files {
            tags.extend(file.tags.iter().cloned());
            rules = file;
        }
        rules.tags = tags;
        let planter = match rules.planter.as_str() {
            "" => None,
            name => Some(blocks.id(name).ok_or_else(|| RegistryError(format!("regrowth.ron: no planter `{name}`")))?),
        };
        Ok(Self { rules, planter })
    }

    /// Whether a block is a plant that regrows.
    pub fn regrows(&self, def: &BlockDef, solid: bool) -> bool {
        !solid && self.rules.tags.iter().any(|tag| def.has_tag(tag))
    }

    /// World seconds a plant of `rarity` takes to come back.
    pub fn seconds(&self, rarity: Rarity) -> f32 {
        self.rules.minutes.of(rarity) * 60.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rarer_plants_take_longer_and_only_plants_with_the_tags_regrow() {
        let blocks = BlockRegistry::new(["t:box", "t:mint", "t:stone"].iter().map(|n| BlockDef { name: n.to_string(), ..Default::default() }).collect()).unwrap();
        let source = r#"(tags: ["herb"], minutes: (common: 20, uncommon: 60, rare: 720, very_rare: 2880, legendary: 5760),
            planter: "box", planter_level: (common: 1, uncommon: 15, rare: 40, very_rare: 60, legendary: 80),
            planter_xp: (common: 4, uncommon: 12, rare: 40, very_rare: 120, legendary: 400))"#;
        let regrowth = Regrowth::new(vec![Regrowth::read("t", source).unwrap()], &blocks).unwrap();
        assert_eq!(regrowth.planter, blocks.id("t:box"));
        let mint = BlockDef { name: "t:mint".into(), tags: vec!["herb".into()], ..Default::default() };
        assert!(regrowth.regrows(&mint, false));
        assert!(!regrowth.regrows(&mint, true), "a solid block is not a plant");
        assert!(!regrowth.regrows(&BlockDef { name: "t:stone".into(), ..Default::default() }, false));
        assert_eq!(regrowth.seconds(Rarity::Common), 1200.0);
        assert!(regrowth.seconds(Rarity::VeryRare) > 24.0 * 3600.0);
    }
}
