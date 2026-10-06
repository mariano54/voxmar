//! Crops: plants players sow, tend and harvest. Each is content (`crops.ron`): the seed
//! that plants it, the blocks it grows through from sprout to ripe, the farming level it
//! takes and the experience it gives, the climate, light and ground it grows in, how long
//! it takes, what a ripe one yields, and the wild plants it comes from, which give its
//! seeds when foraged (Farmer's Delight's wild crops).
//!
//! Growth follows the research in `research/farming.md` (credited in INSPIRATIONS.md):
//! a crop's rate is the product of how well its place suits it, as IC2 and CropsNH
//! multiply their environment factors, with Minecraft's water within four blocks and
//! AgriCraft's climate band that fades out over a margin rather than stopping dead. The
//! server turns the rate into the time each stage takes (`server::farm`), so a growing
//! field costs nothing between stages. Levels raise yields, never speed: RuneScape's
//! level-interpolated harvests.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, Rarity, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CropId(pub u16);

/// A crop as `crops.ron` writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CropDef {
    pub name: String,
    /// The item that plants it; `<name>_seeds` unless given.
    #[serde(default)]
    pub seed: String,
    /// Whether players sow it at all: fruit on trees grows where the tree put it.
    #[serde(default = "yes")]
    pub sown: bool,
    /// It grows on a tree or bush, so the ground and water beneath don't matter to it.
    #[serde(default)]
    pub fruit: bool,
    /// The blocks it grows through, from sprout to ripe; unless given, the crop
    /// template's `<name>_crop_0` to `_2` and the ripe `<name>_crop`.
    #[serde(default)]
    pub stages: Vec<String>,
    /// The skill it trains, and the level that skill must be at to plant it.
    #[serde(default = "farming")]
    pub skill: String,
    #[serde(default = "one")]
    pub level: u8,
    /// Experience for planting one and for harvesting a ripe one.
    pub xp: CropXp,
    /// How rare it is, which wild ones and the colour its items are named in follow.
    #[serde(default)]
    pub rarity: Rarity,
    /// Real minutes from planting to ripe where everything suits it.
    pub minutes: f32,
    /// The climate it grows best in: temperature in °C and moisture, as plants.ron's.
    #[serde(default)]
    pub comfort: Comfort,
    /// The least light, sky or lamp, 0..=15, it grows in.
    #[serde(default = "nine")]
    pub light: u8,
    /// Tags the ground it is planted in must have one of: farmland a hoe tilled, unless
    /// it says.
    #[serde(default = "tilled")]
    pub ground: Vec<String>,
    /// For crops that keep fruiting (berries, tomatoes): picking a ripe one by hand takes
    /// it back to this stage instead of breaking it.
    #[serde(default)]
    pub regrow: Option<u8>,
    /// What a ripe one gives, picked or broken. Broken unripe, it gives its seed back.
    pub harvest: Vec<Loot>,
    /// Wild plants that are this crop gone wild (`wild_wheat`), and what foraging one
    /// gives, once for each plant in it (a clump of three, three times).
    #[serde(default)]
    pub wild: Vec<String>,
    #[serde(default)]
    pub forage: Vec<Loot>,
}

fn yes() -> bool {
    true
}

fn farming() -> String {
    "core:farming".into()
}

fn one() -> u8 {
    1
}

fn nine() -> u8 {
    9
}

fn tilled() -> Vec<String> {
    vec!["tilled".into()]
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CropXp {
    pub plant: f32,
    pub harvest: f32,
    /// For foraging a wild one.
    #[serde(default)]
    pub forage: f32,
}

/// The climate a crop is comfortable in. Outside a range it slows, to a stop a margin
/// beyond it.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Comfort {
    pub temperature: (f32, f32),
    pub moisture: (f32, f32),
}

impl Default for Comfort {
    fn default() -> Self {
        Self { temperature: (-50.0, 60.0), moisture: (0.0, 100.0) }
    }
}

/// One thing a harvest may give.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loot {
    pub item: String,
    /// How many, at least and at most.
    #[serde(default = "single")]
    pub count: (u8, u8),
    /// Chance it drops at all.
    #[serde(default = "always")]
    pub chance: f32,
    /// Uncommon and rarer finds come a little more often with level.
    #[serde(default)]
    pub rarity: Rarity,
}

fn single() -> (u8, u8) {
    (1, 1)
}

fn always() -> f32 {
    1.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CropFile {
    crops: Vec<CropDef>,
}

/// A crop with its names resolved.
#[derive(Clone, Debug)]
pub struct Crop {
    pub def: CropDef,
    /// The item that sows it, if players sow it.
    pub seed: Option<BlockId>,
    pub stages: Vec<BlockId>,
    pub skill: SkillId,
    pub harvest: Vec<(BlockId, Loot)>,
    pub forage: Vec<(BlockId, Loot)>,
}

impl Crop {
    pub fn ripe(&self) -> BlockId {
        *self.stages.last().expect("crops have stages")
    }

    pub fn ripe_stage(&self) -> u8 {
        (self.stages.len() - 1) as u8
    }
}

/// Every crop, found by its seed or by any of its blocks.
#[derive(Debug, Default)]
pub struct CropRegistry {
    crops: Vec<Crop>,
    by_seed: FxHashMap<BlockId, CropId>,
    /// Each growth block: its crop and stage.
    by_block: FxHashMap<BlockId, (CropId, u8)>,
    /// Each wild form, and each of its looks: its crop and how many plants it is.
    by_wild: FxHashMap<BlockId, (CropId, u8)>,
}

impl CropRegistry {
    /// Reads a package's `crops.ron`, qualifying names with its namespace.
    pub fn read(namespace: &str, source: &str) -> Result<Vec<CropDef>, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: CropFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/crops.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        Ok(file
            .crops
            .into_iter()
            .map(|mut def| {
                let short = def.name.rsplit(':').next().unwrap_or_default().to_owned();
                if def.seed.is_empty() {
                    def.seed = format!("{short}_seeds");
                }
                if def.stages.is_empty() {
                    def.stages = (0..3).map(|i| format!("{short}_crop_{i}")).chain([format!("{short}_crop")]).collect();
                }
                qualify(&mut def.name);
                qualify(&mut def.seed);
                def.stages.iter_mut().for_each(qualify);
                def.wild.iter_mut().for_each(qualify);
                def.harvest.iter_mut().chain(&mut def.forage).for_each(|loot| qualify(&mut loot.item));
                def
            })
            .collect())
    }

    /// Resolves crop definitions against the blocks and skills.
    pub fn new(defs: Vec<CropDef>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        for def in defs {
            let error = |message: String| RegistryError(format!("crop `{}`: {message}", def.name));
            let block = |name: &str| blocks.id(name).ok_or_else(|| error(format!("no block `{name}`")));
            if def.stages.len() < 2 || def.stages.len() > 16 {
                return Err(error("a crop grows through 2 to 16 stages".into()));
            }
            if def.minutes <= 0.0 || def.regrow.is_some_and(|stage| stage as usize >= def.stages.len() - 1) {
                return Err(error("needs a positive growing time, and to regrow from before ripe".into()));
            }
            let skill = skills.id(&def.skill).ok_or_else(|| error(format!("no skill `{}`", def.skill)))?;
            let id = CropId(registry.crops.len() as u16);
            let seed = if def.sown { Some(block(&def.seed)?) } else { None };
            let stages = def.stages.iter().map(|name| block(name)).collect::<Result<Vec<_>, _>>()?;
            let loot = |list: &[Loot]| list.iter().map(|loot| Ok((block(&loot.item)?, loot.clone()))).collect::<Result<Vec<_>, _>>();
            let (harvest, forage) = (loot(&def.harvest)?, loot(&def.forage)?);
            for name in &def.wild {
                for look in blocks.looks(block(name)?) {
                    registry.by_wild.insert(look.block, (id, look.size.max(1)));
                }
            }
            if let Some(seed) = seed
                && registry.by_seed.insert(seed, id).is_some()
            {
                return Err(error(format!("`{}` already plants another crop", def.seed)));
            }
            for (stage, &block) in stages.iter().enumerate() {
                if registry.by_block.insert(block, (id, stage as u8)).is_some() {
                    return Err(error(format!("`{}` is already a stage of another crop", def.stages[stage])));
                }
            }
            registry.crops.push(Crop { def, seed, stages, skill, harvest, forage });
        }
        Ok(registry)
    }

    pub fn get(&self, id: CropId) -> &Crop {
        &self.crops[id.0 as usize]
    }

    pub fn iter(&self) -> impl Iterator<Item = (CropId, &Crop)> {
        self.crops.iter().enumerate().map(|(i, crop)| (CropId(i as u16), crop))
    }

    /// The crop `item` plants, if it is a seed.
    pub fn planted_by(&self, item: BlockId) -> Option<CropId> {
        self.by_seed.get(&item).copied()
    }

    /// The crop and stage `block` is, if it is a growing crop.
    pub fn stage_of(&self, block: BlockId) -> Option<(CropId, u8)> {
        self.by_block.get(&block).copied()
    }

    /// The crop `block` is a wild form of, and how many plants it is.
    pub fn wild_form(&self, block: BlockId) -> Option<(CropId, u8)> {
        self.by_wild.get(&block).copied()
    }
}

/// What a crop's place is like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conditions {
    /// Mean air temperature, °C.
    pub temperature: f32,
    pub moisture: f32,
    /// The brighter of sky and block light, 0..=15.
    pub light: u8,
    /// The ground has the `fertile` tag.
    pub fertile: bool,
    /// Water within four blocks across, at the ground's level or one below (Minecraft's
    /// hydrated farmland).
    pub watered: bool,
}

/// Temperature, in °C, and moisture beyond a comfort range over which growth fades to
/// nothing (AgriCraft's widened band).
const TEMPERATURE_MARGIN: f32 = 5.0;
const MOISTURE_MARGIN: f32 = 0.3;
/// Rates below this are no growth: the crop struggles and waits.
const STALLED: f32 = 0.05;

/// How fast a crop grows where it is, as a share of its best: 1 in a fertile, watered
/// plot inside its comfort, less outside it, 0 when it cannot grow at all.
pub fn rate(def: &CropDef, at: Conditions) -> f32 {
    let fit = |value: f32, (low, high): (f32, f32), margin: f32| {
        let outside = (low - value).max(value - high).max(0.0);
        (1.0 - outside / margin).clamp(0.0, 1.0)
    };
    let climate = fit(at.temperature, def.comfort.temperature, TEMPERATURE_MARGIN) * fit(at.moisture, def.comfort.moisture, MOISTURE_MARGIN);
    let light = if at.light >= def.light { 1.0 } else { 0.0 };
    let soil = if at.fertile || def.fruit { 1.0 } else { 0.8 };
    let water = if at.watered || def.fruit { 1.0 } else { 0.6 };
    let rate = climate * light * soil * water;
    if rate < STALLED { 0.0 } else { rate }
}

/// How much more of uncommon and rarer finds a harvest gives at `level`, for a crop that
/// takes `required`: nothing at the level it needs, rising to half again at the highest
/// level, as RuneScape interpolates its yields between levels 1 and 99.
pub fn level_bonus(level: u8, required: u8, max: u8) -> f32 {
    if max <= required {
        return 0.0;
    }
    0.5 * (level.saturating_sub(required) as f32 / (max - required) as f32).min(1.0)
}

/// Rolls a ripe crop's harvest with `bonus` (see [`level_bonus`]), drawing numbers in
/// 0..1 from `random`. Common finds also gain an extra one now and then.
pub fn roll(harvest: &[(BlockId, Loot)], bonus: f32, mut random: impl FnMut() -> f32) -> Vec<(BlockId, u8)> {
    let mut found = Vec::new();
    for (item, loot) in harvest {
        let chance = if loot.rarity > Rarity::Common { loot.chance * (1.0 + bonus) } else { loot.chance };
        if random() >= chance {
            continue;
        }
        let (low, high) = (loot.count.0.min(loot.count.1), loot.count.0.max(loot.count.1));
        let mut count = low + ((random() * (high - low + 1) as f32) as u8).min(high - low);
        if loot.rarity == Rarity::Common && random() < bonus {
            count += 1;
        }
        if count > 0 {
            found.push((*item, count));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> CropDef {
        CropRegistry::read(
            "base",
            r#"(crops: [(
                name: "kale", stages: ["kale_0", "kale_1", "kale_2"], level: 20,
                xp: (plant: 10.0, harvest: 4.0), minutes: 30.0,
                comfort: (temperature: (0.0, 15.0), moisture: (0.6, 1.8)),
                harvest: [(item: "kale", count: (1, 3)), (item: "kale_seeds", chance: 0.5, rarity: Uncommon)],
            )])"#,
        )
        .unwrap()
        .remove(0)
    }

    const GOOD: Conditions = Conditions { temperature: 8.0, moisture: 1.0, light: 15, fertile: true, watered: true };

    #[test]
    fn names_are_qualified_and_defaults_filled() {
        let def = def();
        assert_eq!((def.name.as_str(), def.seed.as_str(), def.stages[2].as_str()), ("base:kale", "base:kale_seeds", "base:kale_2"));
        assert_eq!((def.skill.as_str(), def.light, def.ground.clone()), ("core:farming", 9, vec!["tilled".to_owned()]));
        assert_eq!(def.harvest[1].item, "base:kale_seeds");
        let plain = CropRegistry::read("base", r#"(crops: [(name: "leek", xp: (plant: 1.0, harvest: 1.0), minutes: 5.0, harvest: [], wild: ["wild_leek"])])"#).unwrap();
        assert_eq!(plain[0].stages, ["base:leek_crop_0", "base:leek_crop_1", "base:leek_crop_2", "base:leek_crop"]);
        assert_eq!((plain[0].seed.as_str(), plain[0].wild[0].as_str()), ("base:leek_seeds", "base:wild_leek"));
    }

    #[test]
    fn growth_falls_off_outside_comfort_and_stops_in_the_dark() {
        let def = def();
        assert_eq!(rate(&def, GOOD), 1.0);
        // Half way through the temperature margin: half as fast.
        assert!((rate(&def, Conditions { temperature: 17.5, ..GOOD }) - 0.5).abs() < 1e-4);
        assert_eq!(rate(&def, Conditions { temperature: 25.0, ..GOOD }), 0.0);
        assert_eq!(rate(&def, Conditions { light: 8, ..GOOD }), 0.0);
        // Dry, plain ground still grows, slower.
        assert!((rate(&def, Conditions { fertile: false, watered: false, ..GOOD }) - 0.48).abs() < 1e-4);
    }

    #[test]
    fn levels_raise_rare_finds_a_little_and_common_yields_now_and_then() {
        assert_eq!(level_bonus(20, 20, 99), 0.0);
        assert!((level_bonus(99, 20, 99) - 0.5).abs() < 1e-6);
        assert_eq!(level_bonus(5, 20, 99), 0.0);
        let def = def();
        let harvest: Vec<(BlockId, Loot)> = def.harvest.iter().enumerate().map(|(i, l)| (BlockId(i as u16 + 1), l.clone())).collect();
        // A low draw takes every chance and the bottom of every count.
        assert_eq!(roll(&harvest, 0.0, || 0.0), vec![(BlockId(1), 1), (BlockId(2), 1)]);
        // 0.6 misses the seed's 0.5 chance, but not with the full bonus (0.75).
        let mut draws = [0.0, 0.99, 0.9, 0.6, 0.0].into_iter();
        assert_eq!(roll(&harvest, 0.0, || draws.next().unwrap()), vec![(BlockId(1), 3)]);
        let mut draws = [0.0, 0.99, 0.9, 0.6, 0.0].into_iter();
        assert_eq!(roll(&harvest, 0.5, || draws.next().unwrap()), vec![(BlockId(1), 3), (BlockId(2), 1)]);
    }
}
