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
    /// See [`FishingRules::outlevelled`]; a later package's replaces an earlier one's.
    #[serde(default)]
    pub outlevelled: Option<f32>,
    /// See [`FishingRules::water`]; a later package's replaces an earlier one's.
    #[serde(default)]
    pub water: Option<WaterBands>,
    #[serde(default)]
    pub catches: Vec<CatchDef>,
}

/// How what bites is weighed: single-valued data a package may set (design draft 91).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FishingRules {
    /// Each fish bites twice as often, against those below it, for every this many levels
    /// it asks (design draft 63: 15), so the water's best fish are what a high level mostly
    /// lands; infinite (the default) weighs them only by `weight`.
    pub outlevelled: f32,
    /// The water's climate, from its temperature, which catches read as a biome tag.
    pub water: WaterBands,
}

impl Default for FishingRules {
    fn default() -> Self {
        Self { outlevelled: f32::INFINITY, water: WaterBands::default() }
    }
}

crate::content::settable! {
    /// What one package's `fishing.ron` sets of the rules.
    SetFishingRules for FishingRules { outlevelled: f32, water: WaterBands }
}

impl FishingFile {
    /// The rules this file sets, if any.
    pub fn rules(&self) -> Option<SetFishingRules> {
        (self.outlevelled.is_some() || self.water.is_some()).then_some(SetFishingRules { outlevelled: self.outlevelled, water: self.water })
    }
}

/// Where water turns from one climate to the next, °C (as `climate.ron` reads the land):
/// below `frozen` it is frozen, below `cold` cold, and so on up to warm below `warm`, and
/// hot above. The sea has no biome for each climate, so a catch that wants cold water
/// (cod) or warm (tuna) finds it by the water's own temperature.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterBands {
    pub frozen: f32,
    pub cold: f32,
    pub cool: f32,
    pub temperate: f32,
    pub warm: f32,
}

impl Default for WaterBands {
    /// The land's own bands (`climate.ron`).
    fn default() -> Self {
        Self { frozen: -3.0, cold: 4.0, cool: 10.0, temperate: 17.0, warm: 22.0 }
    }
}

impl WaterBands {
    /// Every climate water may have, coldest first.
    pub const TAGS: [&'static str; 6] = ["frozen", "cold", "cool", "temperate", "warm", "hot"];

    /// The climate of water at `temperature` °C.
    pub fn tag(&self, temperature: f32) -> &'static str {
        let edges = [self.frozen, self.cold, self.cool, self.temperate, self.warm];
        Self::TAGS[edges.iter().take_while(|&&edge| temperature >= edge).count()]
    }

    fn check(&self) -> Result<(), RegistryError> {
        let edges = [self.frozen, self.cold, self.cool, self.temperate, self.warm];
        if edges.iter().any(|e| !e.is_finite()) || edges.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(RegistryError(format!("fishing.ron: the water's bands must rise from `frozen` to `warm`: {self:?}")));
        }
        Ok(())
    }
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
    pub rules: FishingRules,
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

    /// Every package's catches, with `rules` as the packages together set them (checked
    /// here, once all are applied).
    pub fn new(files: Vec<FishingFile>, rules: FishingRules, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        if !(rules.outlevelled > 0.0) {
            return Err(RegistryError(format!("fishing.ron: `outlevelled: {}` is no number of levels above 0", rules.outlevelled)));
        }
        rules.water.check()?;
        let catches = files
            .into_iter()
            .flat_map(|f| f.catches)
            .map(|def| {
                let item = blocks.id(&def.item).ok_or_else(|| RegistryError(format!("fishing.ron: no item `{}`", def.item)))?;
                Ok(Catch { def, item })
            })
            .collect::<Result<_, RegistryError>>()?;
        Ok(Self { catches, skill: skills.id("core:fishing"), rules })
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
        // Each fish bites twice as often, against those below it, for every `outlevelled`
        // levels it asks (written from the angler's level down, so it stays near 1).
        let outlevelled = |c: &Catch| 0.5f32.powf(spot.level.saturating_sub(c.def.level) as f32 / self.rules.outlevelled);
        let weight = |c: &Catch| c.def.weight * outlevelled(c) * if c.def.rarity > Rarity::Common { 1.0 + spot.rod_yield * 4.0 } else { 1.0 };
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
    use crate::content::Settable;

    fn setup() -> FishingRegistry {
        setup_with("")
    }

    fn setup_with(file: &str) -> FishingRegistry {
        let blocks = BlockRegistry::new(["t:shrimp", "t:trout", "t:tuna", "t:eel"].iter().map(|n| BlockDef { name: n.to_string(), ..Default::default() }).collect()).unwrap();
        let mut skills = SkillRegistry::default();
        skills.extend("core", r#"(curve: RuneScape(max: 99), skills: [(name: "fishing", title: "", description: "")])"#).unwrap();
        let source = format!(r#"({file} catches: [
            (item: "shrimp", level: 1, xp: 10.0),
            (item: "trout", level: 15, xp: 50.0, water: Fresh, biomes: ["cold", "cool"]),
            (item: "tuna", level: 40, xp: 80.0, water: Salt, biomes: ["warm", "hot"]),
            (item: "eel", level: 30, xp: 70.0, water: Fresh, when: [Night]),
        ])"#);
        let file = FishingRegistry::read("t", &source).unwrap();
        let rules = file.rules().map_or(FishingRules::default(), |set| FishingRules::default().with(&set).0);
        FishingRegistry::new(vec![file], rules, &blocks, &skills).unwrap()
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

    /// Each fish bites twice as often, against those below it, for every `outlevelled`
    /// levels it asks (design draft 63): with 15, trout (level 15) bite 2^(14/15) times as
    /// often as shrimp (level 1) of the same weight, at any level that catches both.
    #[test]
    fn outlevelled_fish_bite_less() {
        let cold = ["cold".to_owned()];
        let spot = Spot { salt: false, biome_tags: &cold, day: true, raining: false, level: 45, rod_tier: 1, rod_yield: 0.0, luck: 1.0 };
        let shrimp_share = |fishing: &FishingRegistry| {
            let picks = 10_000;
            let shrimp = (0..picks).filter(|&i| fishing.cast(&spot, 0.0, (i as f32 + 0.5) / picks as f32).is_some_and(|c| c.def.item == "t:shrimp")).count();
            shrimp as f32 / picks as f32
        };
        // Equal weights: half each without the rule.
        assert!((shrimp_share(&setup()) - 0.5).abs() < 0.01);
        let expected = 1.0 / (1.0 + 2f32.powf(14.0 / 15.0));
        let share = shrimp_share(&setup_with("outlevelled: 15.0,"));
        assert!((share - expected).abs() < 0.01, "shrimp bite {share} of the time, not {expected}");
        // Rules asking 0 levels are refused.
        let blocks = BlockRegistry::new(vec![BlockDef { name: "t:shrimp".into(), ..Default::default() }]).unwrap();
        let file = FishingRegistry::read("t", "(outlevelled: 0.0, catches: [])").unwrap();
        let rules = FishingRules::default().with(&file.rules().unwrap()).0;
        assert!(FishingRegistry::new(vec![file], rules, &blocks, &SkillRegistry::default()).is_err(), "0 levels is refused");
    }

    /// The game's catches teach by the doubling law, 10 × 2^((level − 1) ÷ 10.5) rounded
    /// (design draft 63), and cooking each teaches what catching it does.
    #[test]
    fn the_game_s_fish_teach_by_their_level() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap();
        let mut wrong = Vec::new();
        for catch in content.fishing.iter() {
            let law = (10.0 * 2f32.powf((catch.def.level as f32 - 1.0) / 10.5)).round();
            if catch.def.xp != law {
                wrong.push(format!("{} (level {}) teaches {}, the law {law}", catch.def.item, catch.def.level, catch.def.xp));
            }
            let cooked: Vec<f32> = content.cooking.iter().filter(|(input, _)| *input == catch.item).map(|(_, r)| r.def.xp).collect();
            if cooked != [catch.def.xp] {
                wrong.push(format!("cooking {} teaches {cooked:?}, catching it {}", catch.def.item, catch.def.xp));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        assert_eq!(content.fishing.rules.outlevelled, 15.0);
    }

    /// A fish's item carries its catch's rarity as its label (cooking then carries it to
    /// the cooked fish, server tests/loot_rarity.rs): the glimmer koi is as very rare in
    /// the hand as on the line.
    #[test]
    fn a_fish_is_labelled_as_rare_as_its_catch() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap();
        let wrong: Vec<String> = content
            .fishing
            .iter()
            .filter(|catch| content.blocks.def(catch.item).rarity != catch.def.rarity)
            .map(|catch| format!("{} bites {:?} but is labelled {:?}", catch.def.item, catch.def.rarity, content.blocks.def(catch.item).rarity))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// Water takes its climate from its temperature, by bands that must rise.
    #[test]
    fn water_takes_its_climate_from_its_temperature() {
        let bands = WaterBands::default();
        let tags: Vec<&str> = [-10.0, -3.0, 0.0, 4.0, 12.0, 21.9, 22.0, 35.0].into_iter().map(|t| bands.tag(t)).collect();
        assert_eq!(tags, ["frozen", "cold", "cold", "cool", "temperate", "warm", "hot", "hot"]);
        assert!(WaterBands { cool: 3.0, ..bands }.check().is_err(), "bands out of order are refused");
        assert!(bands.check().is_ok());
    }
}
