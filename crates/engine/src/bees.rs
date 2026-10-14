//! Bees, as content (`bees.ron`, docs/bees.md): wild nests a swarm is taken from into a
//! skep, kept skeps that fill with honey while their bees forage, and what smoke, fire,
//! rain, night, flowers, crops and creatures do about them.
//!
//! A *take* is a right click on a block tagged with one (`base:tend_bees`): what must be in
//! hand, what the hand's item and the block become, and what it gives. Bees sting a player
//! who takes without smoke near (a fire within reach of the bees, or one in hand); a take
//! that `needs_smoke` then gets nothing, any other goes ahead stung. A kept hive is three
//! blocks: `empty` (a skep), `busy` (bees in it) and `full` (honey in it). The server's
//! apiary moves `busy` to `full` after `minutes` of foraging, faster with more flowers
//! near, never at night or in rain; a fire beside it drives the bees off; crops near a
//! hive with bees grow faster; creatures that come close are stung, and raiders (a bear)
//! eat a full hive's honey.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::creature::{CreatureId, CreatureRegistry};
use crate::crop::Loot;
use crate::effect::{EffectId, EffectRegistry};
use crate::skill::{SkillId, SkillRegistry};

/// One way of taking from bees.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeDef {
    /// The block it is taken from.
    pub block: String,
    /// What must be in hand, if anything (else anything or nothing).
    #[serde(default)]
    pub holding: Option<String>,
    /// What one of the held item becomes (a skep taking a swarm becomes a skep of bees).
    #[serde(default)]
    pub held_becomes: Option<String>,
    /// What the block becomes.
    pub becomes: String,
    /// What it gives, as a crop's harvest does.
    #[serde(default)]
    pub gives: Vec<Loot>,
    /// The skill it asks a level in and trains.
    #[serde(default = "farming")]
    pub skill: String,
    #[serde(default = "one")]
    pub level: u8,
    #[serde(default)]
    pub xp: f32,
    /// Without smoke, nothing is taken (the bees only sting).
    #[serde(default)]
    pub needs_smoke: bool,
}

fn farming() -> String {
    "core:farming".to_owned()
}

fn one() -> u8 {
    1
}

/// How kept hives work. A later package's `hive` replaces it whole.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiveDef {
    /// A skep with no bees, one with bees, and one full of honey.
    pub empty: String,
    pub busy: String,
    pub full: String,
    /// Minutes of foraging, with flowers enough, to fill a hive.
    pub minutes: f32,
    /// Blocks across and up or down within which the bees forage.
    pub forage_reach: i32,
    pub forage_height: i32,
    /// What each block they forage counts for, by the first of its tags listed.
    pub nectar: Vec<(String, f32)>,
    /// The nectar at which the bees forage at full pace; less, slower; none, not at all.
    pub nectar_full: f32,
    /// Crops within this many blocks of a hive with bees grow `pollinate` times as fast.
    pub pollinate_reach: f32,
    pub pollinate: f32,
    /// Seconds between the apiary's looks at each hive.
    pub survey_seconds: f32,
    /// Tags of blocks within `smoke_reach` of the bees, or of the item in hand, that
    /// smoke them calm.
    pub smoke: Vec<String>,
    pub smoke_reach: i32,
    /// One of the smoking blocks this near a kept hive (any way) drives its bees off.
    pub scorch_reach: i32,
    /// The bees stay in while more than this much falls, and while the daylight is below
    /// `dusk` (0 night, 1 day).
    pub rain: f32,
    pub dusk: f32,
    /// A sting: the damage (never the last of a player's health), and the effect it
    /// leaves for a while.
    pub sting: u8,
    pub stung: String,
    pub stung_seconds: f32,
    /// Creatures this near a hive with bees are stung for `creature_sting` (never the last
    /// of their health) at each look, but for those `unstung`; `raiders` eat a full
    /// hive's honey.
    pub creature_reach: f32,
    pub creature_sting: u16,
    #[serde(default)]
    pub unstung: Vec<String>,
    #[serde(default)]
    pub raiders: Vec<String>,
    /// A shot (a sling stone, an arrow) that lands within `shot_reach` of a block tagged
    /// `bees` (a wild nest, a kept hive) stirs them: unless smoke calms them, they sting
    /// the shooter. 0 leaves shots alone.
    #[serde(default)]
    pub shot_reach: f32,
    #[serde(default = "bees_tag")]
    pub bees: String,
}

fn bees_tag() -> String {
    "bees".to_owned()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeeFile {
    #[serde(default)]
    pub takes: Vec<TakeDef>,
    #[serde(default)]
    pub hive: Option<HiveDef>,
}

/// A take, resolved.
#[derive(Clone, Debug)]
pub struct Take {
    pub block: BlockId,
    pub holding: Option<BlockId>,
    pub held_becomes: Option<BlockId>,
    pub becomes: BlockId,
    pub gives: Vec<(BlockId, Loot)>,
    pub skill: SkillId,
    pub level: u8,
    pub xp: f32,
    pub needs_smoke: bool,
}

/// The hive rules, resolved.
#[derive(Clone, Debug)]
pub struct Hive {
    pub def: HiveDef,
    pub empty: BlockId,
    pub busy: BlockId,
    pub full: BlockId,
    pub stung: EffectId,
    pub unstung: Vec<CreatureId>,
    pub raiders: Vec<CreatureId>,
}

impl Hive {
    /// What a block counts for to bees foraging it.
    pub fn nectar(&self, blocks: &BlockRegistry, block: BlockId) -> f32 {
        let def = blocks.def(block);
        self.def.nectar.iter().find(|(tag, _)| def.has_tag(tag)).map_or(0.0, |(_, n)| *n)
    }

    /// How fast the bees fill the hive with `nectar` about them: 0 to 1.
    pub fn pace(&self, nectar: f32) -> f32 {
        (nectar / self.def.nectar_full).clamp(0.0, 1.0)
    }

    /// Whether the bees are out foraging in this weather and light.
    pub fn out(&self, precipitation: f32, daylight: f32) -> bool {
        precipitation <= self.def.rain && daylight >= self.def.dusk
    }

    /// Whether a block, or an item in hand, smokes bees.
    pub fn smokes(&self, blocks: &BlockRegistry, block: BlockId) -> bool {
        let def = blocks.def(block);
        self.def.smoke.iter().any(|tag| def.has_tag(tag))
    }

    /// Whether a kept hive has bees in it.
    pub fn has_bees(&self, block: BlockId) -> bool {
        block == self.busy || block == self.full
    }
}

#[derive(Clone, Debug, Default)]
pub struct Bees {
    pub takes: Vec<Take>,
    pub hive: Option<Hive>,
}

impl Bees {
    pub fn read(namespace: &str, source: &str) -> Result<BeeFile, RegistryError> {
        let mut file: BeeFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/bees.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        for take in &mut file.takes {
            qualify(&mut take.block);
            qualify(&mut take.becomes);
            take.holding.iter_mut().for_each(qualify);
            take.held_becomes.iter_mut().for_each(qualify);
            take.gives.iter_mut().for_each(|loot| qualify(&mut loot.item));
            qualify(&mut take.skill);
        }
        if let Some(hive) = &mut file.hive {
            for name in [&mut hive.empty, &mut hive.busy, &mut hive.full, &mut hive.stung] {
                qualify(name);
            }
            hive.unstung.iter_mut().chain(hive.raiders.iter_mut()).for_each(qualify);
        }
        Ok(file)
    }

    /// Every package's takes, and the last package's hive.
    pub fn new(files: Vec<BeeFile>, blocks: &BlockRegistry, skills: &SkillRegistry, effects: &EffectRegistry, creatures: &CreatureRegistry) -> Result<Self, RegistryError> {
        let block = |name: &str| blocks.id(name).ok_or_else(|| RegistryError(format!("bees.ron: no block `{name}`")));
        let creature = |name: &String| creatures.id(name).ok_or_else(|| RegistryError(format!("bees.ron: no creature `{name}`")));
        let mut bees = Bees::default();
        let mut hive_def = None;
        for file in files {
            for def in file.takes {
                let gives = def.gives.iter().map(|loot| Ok((block(&loot.item)?, loot.clone()))).collect::<Result<_, RegistryError>>()?;
                let skill = skills.id(&def.skill).ok_or_else(|| RegistryError(format!("bees.ron: no skill `{}`", def.skill)))?;
                if !def.xp.is_finite() || def.xp < 0.0 {
                    return Err(RegistryError(format!("bees.ron: taking from {} pays {} experience", def.block, def.xp)));
                }
                bees.takes.push(Take {
                    block: block(&def.block)?,
                    holding: def.holding.as_deref().map(block).transpose()?,
                    held_becomes: def.held_becomes.as_deref().map(block).transpose()?,
                    becomes: block(&def.becomes)?,
                    gives,
                    skill,
                    level: def.level,
                    xp: def.xp,
                    needs_smoke: def.needs_smoke,
                });
            }
            hive_def = file.hive.or(hive_def);
        }
        if let Some(def) = hive_def {
            let positive = [def.minutes, def.nectar_full, def.pollinate, def.survey_seconds, def.creature_reach, def.stung_seconds];
            if positive.iter().any(|v| !v.is_finite() || *v <= 0.0) || def.forage_reach < 0 || def.forage_height < 0 || def.smoke_reach < 0 || def.scorch_reach < 0 {
                return Err(RegistryError("bees.ron: the hive's times, reaches and paces must be above 0".to_owned()));
            }
            if !(def.shot_reach.is_finite() && (0.0..=16.0).contains(&def.shot_reach)) || (def.shot_reach > 0.0 && !blocks.ids().any(|id| blocks.def(id).has_tag(&def.bees))) {
                return Err(RegistryError(format!("bees.ron: shots stir bees within 0 to 16 blocks, of blocks tagged `{}`, which some block must be", def.bees)));
            }
            if def.scorch_reach > def.smoke_reach {
                return Err(RegistryError("bees.ron: a fire that drives the bees off must be near enough to smoke them".to_owned()));
            }
            if !blocks.ids().any(|id| def.nectar.iter().any(|(tag, _)| blocks.def(id).has_tag(tag))) {
                return Err(RegistryError("bees.ron: no block carries a tag the bees forage".to_owned()));
            }
            let stung = effects.id(&def.stung).ok_or_else(|| RegistryError(format!("bees.ron: no effect `{}`", def.stung)))?;
            bees.hive = Some(Hive {
                empty: block(&def.empty)?,
                busy: block(&def.busy)?,
                full: block(&def.full)?,
                stung,
                unstung: def.unstung.iter().map(creature).collect::<Result<_, _>>()?,
                raiders: def.raiders.iter().map(creature).collect::<Result<_, _>>()?,
                def,
            });
        }
        Ok(bees)
    }

    /// The take from `block` with `held` in hand, if any: the first that fits.
    pub fn take(&self, block: BlockId, held: Option<BlockId>) -> Option<&Take> {
        self.takes.iter().find(|t| t.block == block && t.holding.is_none_or(|h| Some(h) == held))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_qualified_and_the_hive_is_read() {
        let source = r#"(
            takes: [(block: "bee_nest", holding: Some("skep"), held_becomes: Some("bee_skep"), becomes: "core:air", level: 12, xp: 60.0, needs_smoke: true)],
            hive: Some((empty: "skep", busy: "bee_skep", full: "honey_skep", minutes: 20.0, forage_reach: 6, forage_height: 3,
                nectar: [("nectar", 4.0), ("flower", 1.0)], nectar_full: 12.0, pollinate_reach: 6.0, pollinate: 1.25, survey_seconds: 10.0,
                smoke: ["fire"], smoke_reach: 5, scorch_reach: 1, rain: 0.3, dusk: 0.2, sting: 2, stung: "stung", stung_seconds: 60.0,
                creature_reach: 2.5, creature_sting: 1, raiders: ["brown_bear"])),
        )"#;
        let file = Bees::read("base", source).unwrap();
        let take = &file.takes[0];
        assert_eq!((take.block.as_str(), take.becomes.as_str(), take.holding.as_deref(), take.skill.as_str()), ("base:bee_nest", "core:air", Some("base:skep"), "core:farming"));
        let hive = file.hive.unwrap();
        assert_eq!((hive.busy.as_str(), hive.stung.as_str(), hive.raiders[0].as_str()), ("base:bee_skep", "base:stung", "base:brown_bear"));
    }

    /// Rain and night: the game's bees stay in while it rains or is dark, and forage faster
    /// with more flowers about them, a honeydew lily worth four of a dandelion.
    #[test]
    fn the_games_bees_stay_in_from_rain_and_night_and_follow_the_flowers() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("content loads");
        let hive = content.bees.hive.as_ref().expect("base keeps bees");
        assert!(hive.out(0.0, 1.0), "a dry day");
        assert!(!hive.out(0.5, 1.0), "rain keeps them in");
        assert!(!hive.out(0.0, 0.0), "so does the night");
        let (lily, dandelion) = (content.blocks.expect("base:honeydew_lily"), content.blocks.expect("base:dandelion"));
        assert_eq!(hive.nectar(&content.blocks, lily), 4.0 * hive.nectar(&content.blocks, dandelion));
        assert!(hive.pace(0.0) == 0.0 && hive.pace(hive.def.nectar_full / 2.0) == 0.5 && hive.pace(100.0) == 1.0);
        // A swarm needs the skep in hand; honey comes out with anything.
        let (nest, skep, full) = (content.blocks.expect("base:bee_nest"), content.blocks.expect("base:skep"), content.blocks.expect("base:honey_skep"));
        assert!(content.bees.take(nest, None).is_none() && content.bees.take(nest, Some(skep)).is_some_and(|t| t.needs_smoke));
        assert!(content.bees.take(full, None).is_some_and(|t| !t.needs_smoke));
    }
}
