//! Fishing: what a line cast into water brings up, as content (each package's
//! `fishing.ron`).
//!
//! A catch is an item with the Fishing level it takes, the experience it gives, a weight
//! (how often it bites among those that can), and where and when it bites: salt or fresh
//! water, biome tags, day or night, rain. Casting takes a few seconds; then the line comes
//! up with something that can bite there, or nothing. Higher levels land more often and
//! open better water; a better rod lands more often and brings rarer fish a little more
//! often. Fish never come faster for a level, only more reliably.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, Rarity, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

/// What water a catch bites in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Water {
    #[default]
    Any,
    /// The sea: a biome tagged `ocean`.
    Salt,
    /// Rivers, lakes and ponds: anywhere else.
    Fresh,
}

/// When a catch bites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum When {
    Day,
    Night,
    /// Rain (or snow) falling where the line is.
    Rain,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchDef {
    pub item: String,
    pub level: u8,
    pub xp: f32,
    #[serde(default = "one")]
    pub weight: f32,
    #[serde(default)]
    pub water: Water,
    /// It bites only in a biome with one of these tags (anywhere when empty).
    #[serde(default)]
    pub biomes: Vec<String>,
    /// It bites only while all of these hold.
    #[serde(default)]
    pub when: Vec<When>,
    #[serde(default)]
    pub rarity: Rarity,
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FishingFile {
    #[serde(default)]
    pub catches: Vec<CatchDef>,
}

/// A catch with its item resolved.
#[derive(Clone, Debug)]
pub struct Catch {
    pub def: CatchDef,
    pub item: BlockId,
}

/// Where and when a line is cast.
#[derive(Clone, Copy, Debug)]
pub struct Spot<'a> {
    pub salt: bool,
    pub biome_tags: &'a [String],
    pub day: bool,
    pub raining: bool,
    /// The angler's Fishing level, and the rod's tier and yield.
    pub level: u8,
    pub rod_tier: u8,
    pub rod_yield: f32,
    /// How much more often than usual a cast lands, from effects (waterside, say).
    pub luck: f32,
}

/// Chance a cast lands anything: from a third at a fishing spot's best level-1 water, up
/// to nine in ten with levels and a good rod.
const LAND_LEAST: f32 = 0.35;
const LAND_MOST: f32 = 0.9;

#[derive(Debug, Default)]
pub struct FishingRegistry {
    catches: Vec<Catch>,
    pub skill: Option<SkillId>,
}

impl FishingRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<FishingFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut file: FishingFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/fishing.ron: {e}")))?;
        for catch in &mut file.catches {
            if !catch.item.contains(':') {
                catch.item = format!("{namespace}:{}", catch.item);
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<FishingFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let catches = files
            .into_iter()
            .flat_map(|f| f.catches)
            .map(|def| {
                let item = blocks.id(&def.item).ok_or_else(|| RegistryError(format!("fishing.ron: no item `{}`", def.item)))?;
                Ok(Catch { def, item })
            })
            .collect::<Result<_, RegistryError>>()?;
        Ok(Self { catches, skill: skills.id("core:fishing") })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Catch> {
        self.catches.iter()
    }

    /// The catches that can bite at `spot` for its angler's level.
    pub fn biting(&self, spot: &Spot) -> Vec<&Catch> {
        self.catches
            .iter()
            .filter(|c| {
            let def = &c.def;
            let water = match def.water {
                Water::Any => true,
                Water::Salt => spot.salt,
                Water::Fresh => !spot.salt,
            };
            let biome = def.biomes.is_empty() || def.biomes.iter().any(|t| spot.biome_tags.contains(t));
            let when = def.when.iter().all(|w| match w {
                When::Day => spot.day,
                When::Night => !spot.day,
                When::Rain => spot.raining,
            });
            def.level <= spot.level && water && biome && when
            })
            .collect()
    }

    /// What one cast at `spot` brings up, from two draws in 0..1: whether anything bites,
    /// then which. Rarer fish take a little more of the weight with a better rod.
    pub fn cast(&self, spot: &Spot, land: f32, pick: f32) -> Option<&Catch> {
        let biting = self.biting(spot);
        let best = biting.iter().map(|c| c.def.level).max()?;
        // Each level above what the water's best fish asks lands a little more often.
        let chance = ((LAND_LEAST + 0.008 * spot.level.saturating_sub(best) as f32 + 0.05 * spot.rod_tier as f32) * spot.luck).min(LAND_MOST);
        if land >= chance {
            return None;
        }
        let weight = |c: &Catch| c.def.weight * if c.def.rarity > Rarity::Common { 1.0 + spot.rod_yield * 4.0 } else { 1.0 };
        let total: f32 = biting.iter().map(|c| weight(c)).sum();
        let mut left = pick * total;
        for c in &biting {
            left -= weight(c);
            if left < 0.0 {
                return Some(c);
            }
        }
        biting.last().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn setup() -> FishingRegistry {
        let blocks = BlockRegistry::new(["t:shrimp", "t:trout", "t:tuna", "t:eel"].iter().map(|n| BlockDef { name: n.to_string(), ..Default::default() }).collect()).unwrap();
        let mut skills = SkillRegistry::default();
        skills.extend("core", r#"(curve: RuneScape(max: 99), skills: [(name: "fishing", title: "", description: "")])"#).unwrap();
        let source = r#"(catches: [
            (item: "shrimp", level: 1, xp: 10.0),
            (item: "trout", level: 15, xp: 50.0, water: Fresh, biomes: ["cold", "cool"]),
            (item: "tuna", level: 40, xp: 80.0, water: Salt, biomes: ["warm", "hot"]),
            (item: "eel", level: 30, xp: 70.0, water: Fresh, when: [Night]),
        ])"#;
        FishingRegistry::new(vec![FishingRegistry::read("t", source).unwrap()], &blocks, &skills).unwrap()
    }

    #[test]
    fn what_bites_follows_level_water_biome_and_time() {
        let fishing = setup();
        let cold = ["cold".to_owned()];
        let names = |spot: &Spot| fishing.biting(spot).into_iter().map(|c| c.def.item.clone()).collect::<Vec<_>>();
        let spot = Spot { salt: false, biome_tags: &cold, day: true, raining: false, level: 50, rod_tier: 1, rod_yield: 0.0, luck: 1.0 };
        assert_eq!(names(&spot), ["t:shrimp", "t:trout"]);
        assert_eq!(names(&Spot { day: false, ..spot }), ["t:shrimp", "t:trout", "t:eel"]);
        assert_eq!(names(&Spot { level: 10, ..spot }), ["t:shrimp"]);
        assert_eq!(names(&Spot { salt: true, ..spot }), ["t:shrimp"], "tuna wants warm sea");
    }

    #[test]
    fn casts_land_more_with_levels_and_rods() {
        let fishing = setup();
        let tags = ["cold".to_owned()];
        let novice = Spot { salt: false, biome_tags: &tags, day: true, raining: false, level: 15, rod_tier: 1, rod_yield: 0.0, luck: 1.0 };
        let expert = Spot { level: 80, rod_tier: 3, ..novice };
        let landed = |spot: &Spot| (0..100).filter(|&i| fishing.cast(spot, i as f32 / 100.0, 0.5).is_some()).count();
        assert_eq!(landed(&novice), 40);
        assert!(landed(&expert) > 85 && landed(&expert) <= 90, "{}", landed(&expert));
    }
}
