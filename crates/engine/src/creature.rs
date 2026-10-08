//! Creatures: animals, monsters, fish and spirits. Each species is content
//! (`creatures.ron`): its body, stats, where it lives, its temper and senses, what it eats,
//! the behaviours it chooses between, and what it drops. Every individual is an agent the
//! server runs (see `server::creatures`) with a utility controller ([`brain`]) and a body
//! that moves by the same collision as players ([`motion`]).
//!
//! Nothing about a creature is special to its kind. "Hostile" is not a type: it is high
//! aggression and little fear in its temperament, read by the same behaviours a rabbit
//! uses, so a bear is calm until something provokes it (docs/creature-ai.md). Species that
//! list no behaviours get a set made from their temperament and diet, the first layer of
//! the behaviour generator (docs/creature-ai.md, "Behaviour generation").
//!
//! Bodies are boxes from a small part library ([`Model`]), skinned with a texture painted
//! for each individual from its species' colours and markings ([`skin`]) and animated from
//! how the creature moves, so the client needs no model files.

pub mod anatomy;
pub mod brain;
pub mod condition;
pub mod husbandry;
pub mod model;
pub mod motion;
pub mod skin;
pub mod volume;

pub use model::{HornShape, Model, Part, Role, Swing};

use std::sync::Arc;

use glam::Vec3;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockRegistry, Rarity, RegistryError};
use crate::loot::{self, Loot};
use crate::skill::{SkillId, SkillRegistry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CreatureId(pub u16);

/// How a species spawns and is kept (see docs/creatures.md, "Kinds of creature").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Kind {
    /// From the land's population near players, by day or night as it prefers.
    #[default]
    Animal,
    /// In darkness near players; never kept.
    Monster,
    /// In water of the right climate.
    Fish,
    /// Only where aether is high; no body to butcher.
    Spirit,
}

/// Where a species can be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Water {
    /// On land; it wades but does not swim.
    #[default]
    Land,
    /// On land and in water (ducks, frogs, crocodiles).
    Amphibious,
    /// Only in water.
    Swim,
    /// Floats in the air (wisps); fliers proper come later.
    Air,
}

/// When a species is about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Active {
    #[default]
    Day,
    Night,
    /// At dawn and dusk, and less by day.
    Crepuscular,
    Any,
}

impl Active {
    /// How lively the species is at `daylight` (0 at night, 1 at noon), 0.2..=1.
    pub fn liveliness(self, daylight: f32) -> f32 {
        let d = daylight.clamp(0.0, 1.0);
        let raw = match self {
            Active::Day => d,
            Active::Night => 1.0 - d,
            Active::Crepuscular => 1.0 - (d - 0.45).abs() * 1.6,
            Active::Any => 1.0,
        };
        raw.clamp(0.2, 1.0)
    }
}

/// A creature's attack.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attack {
    /// Damage of a hit that lands, least and most.
    pub damage: (u8, u8),
    /// Blocks from its body's edge it hits at.
    #[serde(default = "reach")]
    pub reach: f32,
    /// Seconds between attacks.
    #[serde(default = "every")]
    pub every: f32,
    /// How it strikes, which armour resists (`combat::Style`): a bite stabs, claws slash,
    /// horns and hooves crush.
    #[serde(default)]
    pub style: crate::combat::Style,
    /// Seconds it winds up before the blow lands, the tell a player can block or dodge
    /// by, and seconds it recovers after, standing open to blows (unset, `combat::Feel`'s).
    #[serde(default)]
    pub windup: Option<f32>,
    #[serde(default)]
    pub recovery: Option<f32>,
    /// A heavy blow goes through any guard: only a dodge escapes it.
    #[serde(default)]
    pub heavy: bool,
    /// Blocks it lunges at its target as the blow lands, so backing off at a walk does not
    /// escape it (unset, `combat::Feel`'s; quick animals more, big ones less).
    #[serde(default)]
    pub lunge: Option<f32>,
    /// How fast it follows its target through the first part of a wind-up (unset,
    /// `combat::Feel`'s, a run; slow creatures walk).
    #[serde(default)]
    pub pace: Option<crate::combat::Pace>,
    /// What it adds to its attack roll (unset, `combat::Feel::creature_accuracy` of its
    /// level), as a weapon's accuracy does a player's.
    #[serde(default)]
    pub accuracy: Option<u16>,
}

fn reach() -> f32 {
    1.0
}

fn every() -> f32 {
    1.2
}

/// The shape of a species' body.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyDef {
    /// Width and height of its collision box, in blocks.
    pub size: (f32, f32),
    pub model: Model,
    /// Its colours: coat, belly or second colour, then features (horns, beak, glow).
    pub colours: Vec<[u8; 3]>,
    /// Drawn glowing: unshaded and bright, for supernatural creatures that give light.
    #[serde(default)]
    pub glows: bool,
    /// How its skin is painted: eyes, grain, belly, markings ([`skin`]).
    #[serde(default)]
    pub skin: skin::SkinDef,
    /// Drawn this much larger than its plan's numbers (and its box with it): small animals
    /// a little larger than life, so they read among the grass.
    #[serde(default = "unscaled")]
    pub scale: f32,
}

fn unscaled() -> f32 {
    1.0
}

impl BodyDef {
    /// The body as an individual's genes show it: colours replaced, markings added, the
    /// species' own markings dropped for a solid coat.
    pub fn with(&self, traits: &crate::genes::Traits) -> BodyDef {
        let mut body = self.clone();
        for &(index, rgb) in &traits.colours {
            if let Some(colour) = body.colours.get_mut(index as usize) {
                *colour = rgb;
            }
        }
        if traits.plain {
            body.skin.markings.clear();
        }
        body.skin.markings.extend(traits.markings.iter().cloned());
        body
    }
}

/// The climate a species is comfortable in, as plants.ron and crops.ron write it, plus
/// aether. Outside a range its numbers fall off over a margin.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Comfort {
    pub temperature: (f32, f32),
    pub moisture: (f32, f32),
    pub aether: (f32, f32),
}

impl Default for Comfort {
    fn default() -> Self {
        Self { temperature: (-50.0, 60.0), moisture: (0.0, 100.0), aether: (0.0, 1.0) }
    }
}

/// How well a place suits a species, 0..=1: 1 inside every range, fading to 0 over 5 °C,
/// 0.3 moisture and 0.15 aether beyond (as `crop::rate` does for crops).
pub fn fit(comfort: &Comfort, temperature: f32, moisture: f32, aether: f32) -> f32 {
    let fit = |value: f32, (low, high): (f32, f32), margin: f32| {
        let outside = (low - value).max(value - high).max(0.0);
        (1.0 - outside / margin).clamp(0.0, 1.0)
    };
    fit(temperature, comfort.temperature, 5.0) * fit(moisture, comfort.moisture, 0.3) * fit(aether, comfort.aether, 0.15)
}

/// Where a species lives.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Habitat {
    /// Biomes by `tag:` or by name, each with a weight. Empty: anywhere its comfort fits.
    pub biomes: Vec<(String, f32)>,
    pub comfort: Comfort,
    pub water: Water,
    pub active: Active,
    /// Lives under ground, in caves.
    pub caves: bool,
}

impl Habitat {
    /// The weight of a biome with `name` and `tags`, 0 if the species does not live there.
    pub fn biome_weight(&self, name: &str, tags: &[String]) -> f32 {
        if self.biomes.is_empty() {
            return 1.0;
        }
        let short = name.rsplit(':').next().unwrap_or(name);
        self.biomes
            .iter()
            .filter(|(selector, _)| match selector.strip_prefix("tag:") {
                Some(tag) => tags.iter().any(|t| t == tag),
                None => selector == name || selector.rsplit(':').next() == Some(short),
            })
            .map(|(_, weight)| *weight)
            .fold(0.0, f32::max)
    }
}

/// How many come together.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Group {
    pub size: (u8, u8),
}

impl Default for Group {
    fn default() -> Self {
        Self { size: (1, 1) }
    }
}

/// A species' temper, each 0..=1 (docs/creature-ai.md, "Aggression"). Individuals roll
/// their own personality about these values, up to `spread` either way (see
/// [`brain::Personality`]).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Temperament {
    pub aggression: f32,
    pub fear: f32,
    pub curiosity: f32,
    pub sociability: f32,
    pub territory: f32,
    /// Pace and restlessness.
    pub energy: f32,
    /// How easily startled.
    pub nervousness: f32,
    /// How far individuals differ from the species.
    pub spread: f32,
}

impl Default for Temperament {
    fn default() -> Self {
        Self { aggression: 0.0, fear: 0.0, curiosity: 0.0, sociability: 0.0, territory: 0.0, energy: 0.5, nervousness: 0.5, spread: 0.2 }
    }
}

/// How far it notices things, in blocks.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Senses {
    pub sight: f32,
    pub hearing: f32,
}

impl Default for Senses {
    fn default() -> Self {
        Self { sight: 16.0, hearing: 12.0 }
    }
}

/// How it moves, in blocks per second and degrees per second.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Movement {
    pub style: Style,
    pub walk: f32,
    pub run: f32,
    pub turn: f32,
    /// Upward speed of its jump; 0 for creatures that cannot jump.
    pub jump: f32,
}

impl Default for Movement {
    fn default() -> Self {
        Self { style: Style::Walker, walk: 1.6, run: 4.5, turn: 270.0, jump: 7.5 }
    }
}

/// The way a species moves, which the brain and the client's animation both read
/// (docs/creature-ai.md, "Movement styles").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Style {
    /// Walks with pauses: grazers, birds on the ground.
    #[default]
    Walker,
    /// Moves in hops with pauses; flees in zigzags.
    Hopper,
    /// Stalks low, freezes when watched, pounces: cats.
    Stalker,
    /// Steady trot, nose low: canids.
    Trotter,
    /// Slow and heavy, short fast charges: bears.
    Lumberer,
    /// Low and slow: snails, spiders' crawl, shamblers.
    Creeper,
    /// Floats and bobs: wisps.
    Drifter,
}

/// What it eats and what tempts it, by block or item name or `tag:`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Diet {
    pub eats: Vec<String>,
    pub favourite: Vec<String>,
}

/// One behaviour a species chooses between, with how strongly it favours it.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviourEntry {
    #[serde(rename = "do")]
    pub behaviour: Behaviour,
    #[serde(default = "unit")]
    pub weight: f32,
}

fn unit() -> f32 {
    1.0
}

/// The behaviour templates the engine knows; a species lists which it has (or gets a set
/// from its temperament and diet) and tunes them. See [`brain`] for how they score.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum Behaviour {
    /// Strolls about its home, pausing.
    Wander { radius: f32 },
    /// Stands and looks about.
    LookAround,
    /// Lifts its head or rears up to watch a player nearby.
    StandToLook,
    /// Stops to eat what it eats from the ground about it.
    Graze,
    /// Runs from threats closer than `radius`.
    Flee { radius: f32 },
    /// Holds still at a threat just beyond fleeing distance.
    Freeze,
    /// Keeps near others of its kind.
    Herd { spacing: f32 },
    /// Stands watch while kin graze.
    Sentinel,
    /// Follows a player holding its favourite food.
    Tempted { radius: f32 },
    /// Hunts players who come within `radius`: chases and attacks.
    Hunt { radius: f32 },
    /// Fights back when hurt (neutral creatures); without it, a hurt creature flees.
    Defend,
    /// Comes to look at players from a distance.
    Curious { distance: f32 },
    /// Blinks a few blocks away when fleeing (supernatural).
    Blink,
}

/// Experience for killing one, in `skill`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CreatureXp {
    pub kill: f32,
    pub skill: Option<String>,
}

/// A species as `creatures.ron` writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatureDef {
    pub name: String,
    /// What players see it called; from the name unless given.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub rarity: Rarity,
    /// Lives only where aether is high (docs/creatures.md, "Supernatural").
    #[serde(default)]
    pub supernatural: bool,
    /// Its combat level, RuneScape-style.
    #[serde(default = "first")]
    pub level: u8,
    pub health: u16,
    #[serde(default)]
    pub attack: Option<Attack>,
    /// Its defence level, against which hits are rolled.
    #[serde(default = "first")]
    pub defence: u8,
    /// The style of blow its defence counts half against (`combat::defence_against`).
    #[serde(default)]
    pub weak: Option<crate::combat::Style>,
    pub body: BodyDef,
    #[serde(default)]
    pub movement: Movement,
    #[serde(default)]
    pub habitat: Habitat,
    #[serde(default)]
    pub group: Group,
    #[serde(default)]
    pub temperament: Temperament,
    #[serde(default)]
    pub senses: Senses,
    /// How far from home it roams before turning back, in blocks.
    #[serde(default = "leash")]
    pub leash: f32,
    #[serde(default)]
    pub diet: Diet,
    /// Its behaviours; when empty, made from its temperament and diet ([`generated`]).
    #[serde(default)]
    pub behaviours: Vec<BehaviourEntry>,
    #[serde(default)]
    pub loot: Vec<Loot>,
    #[serde(default)]
    pub xp: CreatureXp,
    /// How players tame it, if they can (docs/breeding.md).
    #[serde(default)]
    pub taming: Option<husbandry::Taming>,
    /// How kept ones breed, if they do.
    #[serde(default)]
    pub breeding: Option<husbandry::Breeding>,
    /// What kept ones give on timers.
    #[serde(default)]
    pub products: Vec<husbandry::Product>,
    /// What kept ones need beyond food, water and their climate (docs/breeding.md).
    #[serde(default)]
    pub needs: condition::NeedsDef,
    /// What individuals inherit: coat colours, size, yield, temper (docs/breeding.md). At
    /// most [`crate::genes::MOST`].
    #[serde(default)]
    pub genes: Vec<crate::genes::GeneDef>,
    /// Named breeds, by their genes.
    #[serde(default)]
    pub breeds: Vec<crate::genes::BreedDef>,
    /// How it falls when it dies.
    #[serde(default)]
    pub death: Death,
}

/// How a creature falls when it dies, as the client draws it: it tips over onto its side
/// (away from whoever watches) over the first half, then sinks into the ground.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Death {
    /// Seconds from the blow to gone.
    pub seconds: f32,
    /// Degrees it rolls onto its side.
    pub roll: f32,
    /// How far it sinks, in its own heights.
    pub sink: f32,
    /// Seconds from the blow to its loot coming out; as its body goes unless given.
    pub drops: Option<f32>,
}

impl Default for Death {
    fn default() -> Self {
        Self { seconds: 0.9, roll: 90.0, sink: 1.0, drops: None }
    }
}

impl Death {
    /// Seconds from the blow to its loot coming out.
    pub fn drops_after(&self) -> f32 {
        self.drops.unwrap_or(self.seconds).max(0.0)
    }
}

fn first() -> u8 {
    1
}

fn leash() -> f32 {
    24.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatureFile {
    creatures: Vec<CreatureDef>,
    #[serde(default)]
    limits: Option<SetLimits>,
    #[serde(default)]
    defence: Option<SetDefenceTraining>,
    #[serde(default)]
    condition: Option<condition::SetConditionRules>,
}

/// One package's `creatures.ron`: its species, and what it sets of the world's rules.
#[derive(Debug)]
pub struct CreaturePackage {
    pub defs: Vec<CreatureDef>,
    pub limits: Option<SetLimits>,
    pub defence: Option<SetDefenceTraining>,
    pub condition: Option<condition::SetConditionRules>,
}

/// What packages set for every creature in the world: how many there are, how their
/// blows train Defence, and how kept animals' condition is reckoned. Each is single-valued data a later package replaces field by field
/// ([`crate::content::Settable`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rules {
    pub limits: Limits,
    pub defence: DefenceTraining,
    pub condition: condition::ConditionRules,
}

/// How many creatures the world holds. A package's `creatures.ron` may set any of them
/// (`limits`); a later package's replace an earlier one's, with a note naming both, and
/// what none sets keeps these defaults. Wild creatures and kept animals have budgets of
/// their own, so a rancher's herds never empty the world of wild animals and monsters for
/// everyone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Most alive at once, wild and kept, whatever the players: room for the wild ones and
    /// `keepers` players' kept animals. Kept animals still waiting in the save count against
    /// it. Past that many keepers at their cap, wild ones get fewer.
    pub most: usize,
    /// Most wild creatures alive at once.
    pub wild_most: usize,
    /// Most animals one player keeps, herds and pets together, alive or waiting in the
    /// save: taming and births past it are refused.
    pub kept_each: usize,
    /// How many players' kept animals `most` makes room for, beside the wild ones.
    pub keepers: usize,
    /// At most this many animals, and this many monsters, near each player.
    pub animals_near: usize,
    pub monsters_near: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { most: 1024, wild_most: 512, kept_each: 64, keepers: 8, animals_near: 6, monsters_near: 6 }
    }
}

crate::content::settable! {
    /// The limits one package sets: any of them.
    SetLimits for Limits { most: usize, wild_most: usize, kept_each: usize, keepers: usize, animals_near: usize, monsters_near: usize }
}

impl Limits {
    /// Whether these limits can hold: room for one wild creature, for what one player draws
    /// near, and for every keeper's animals beside the wild ones.
    pub fn check(&self) -> Result<(), RegistryError> {
        let error = |why: String| Err(RegistryError(format!("creature limits: {why}")));
        if self.wild_most == 0 {
            return error("`wild_most` must be at least 1".into());
        }
        if self.animals_near + self.monsters_near > self.wild_most {
            return error(format!(
                "{} animals and {} monsters near one player are more than `wild_most` ({})",
                self.animals_near, self.monsters_near, self.wild_most
            ));
        }
        let needed = self.keepers.checked_mul(self.kept_each).and_then(|kept| kept.checked_add(self.wild_most));
        if needed.is_none_or(|needed| self.most < needed) {
            return error(format!(
                "`most` ({}) is less than `wild_most` ({}) and {} keepers' {} kept animals each",
                self.most, self.wild_most, self.keepers, self.kept_each
            ));
        }
        Ok(())
    }
}

/// When a creature's blow that misses trains the player's Defence (design wiki draft 63):
/// only a blow from a creature the player landed a damaging blow on within
/// `fought_within` seconds, and of at least `least_level` times their Defence level. So
/// standing among weak monsters, or tapping a penned hen between their blows, trains
/// nothing, and each creature feeds only the landed blows it lives through. A package's
/// `creatures.ron` may set either (`defence`); a later package's replace an earlier one's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefenceTraining {
    /// Seconds a landed blow keeps the fight on.
    pub fought_within: f32,
    /// The least creature level that trains, as a share of the player's Defence level.
    pub least_level: f32,
}

impl Default for DefenceTraining {
    fn default() -> Self {
        Self { fought_within: 10.0, least_level: 0.5 }
    }
}

crate::content::settable! {
    /// What one package sets of Defence training.
    SetDefenceTraining for DefenceTraining { fought_within: f32, least_level: f32 }
}

impl DefenceTraining {
    /// Whether these rules make sense: a fight lasts a while, and no level share is negative.
    pub fn check(&self) -> Result<(), RegistryError> {
        if !(self.fought_within.is_finite() && self.fought_within > 0.0) {
            return Err(RegistryError(format!("defence: `fought_within` must be above 0 seconds, not {}", self.fought_within)));
        }
        if !(self.least_level.is_finite() && self.least_level >= 0.0) {
            return Err(RegistryError(format!("defence: `least_level` must be 0 or more, not {}", self.least_level)));
        }
        Ok(())
    }

    /// Whether a creature of `level` is a match for a player's Defence level `defence`.
    pub fn matches(&self, level: u8, defence: u8) -> bool {
        level as f32 >= self.least_level * defence as f32
    }
}

/// A species with its names resolved.
#[derive(Clone, Debug)]
pub struct Creature {
    pub def: CreatureDef,
    pub behaviours: Vec<BehaviourEntry>,
    /// The modules that run its behaviours, each with its weight, then the ones every
    /// creature has.
    pub modules: Vec<(Arc<dyn brain::Module>, f32)>,
    /// Its body's boxes at full size, and where each face lies in its skin.
    pub parts: Vec<Part>,
    /// How far its body reaches ahead of its centre, and its head's height, at full size,
    /// for collision ([`motion::Body::reach`]).
    pub reach: f32,
    pub head: f32,
    pub layout: skin::Layout,
    pub loot: Vec<loot::Resolved>,
    /// What it eats and what tempts it, as blocks.
    pub eats: Vec<crate::block::BlockId>,
    pub favourite: Vec<crate::block::BlockId>,
    pub xp_skill: Option<SkillId>,
    /// What tames it and what makes a kept one ready to breed, as items.
    pub tame_food: Vec<crate::block::BlockId>,
    pub breed_food: Vec<crate::block::BlockId>,
    /// Each product's item.
    pub products: Vec<crate::block::BlockId>,
}

impl Creature {
    /// Whether it fights back or hunts at all.
    pub fn fights(&self) -> bool {
        self.def.attack.is_some() && self.behaviours.iter().any(|b| matches!(b.behaviour, Behaviour::Defend | Behaviour::Hunt { .. }))
    }
}

/// Every species.
#[derive(Debug, Default)]
pub struct CreatureRegistry {
    creatures: Vec<Creature>,
    by_name: FxHashMap<String, CreatureId>,
    rules: Rules,
}

impl CreatureRegistry {
    /// Reads a package's `creatures.ron`, qualifying names with its namespace: its species,
    /// and what it sets of the world's rules.
    pub fn read(namespace: &str, source: &str) -> Result<CreaturePackage, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: CreatureFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/creatures.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') && !name.starts_with("tag:") {
                *name = format!("{namespace}:{name}");
            }
        };
        let (limits, defence, condition) = (file.limits, file.defence, file.condition);
        let defs = file
            .creatures
            .into_iter()
            .map(|mut def| {
                if def.title.is_empty() {
                    let short = def.name.rsplit(':').next().unwrap_or_default();
                    def.title = title_case(short);
                }
                qualify(&mut def.name);
                def.diet.eats.iter_mut().chain(&mut def.diet.favourite).for_each(qualify);
                def.loot.iter_mut().for_each(|loot| qualify(&mut loot.item));
                def.products.iter_mut().for_each(|p| qualify(&mut p.item));
                if let Some(taming) = &mut def.taming {
                    taming.food.iter_mut().for_each(qualify);
                }
                if let Some(breeding) = &mut def.breeding {
                    breeding.food.iter_mut().for_each(qualify);
                }
                if let Some(skill) = &mut def.xp.skill
                    && !skill.contains(':')
                {
                    *skill = format!("core:{skill}");
                }
                def
            })
            .collect();
        Ok(CreaturePackage { defs, limits, defence, condition })
    }

    /// Resolves species against the blocks (items) and skills.
    pub fn new(defs: Vec<CreatureDef>, rules: Rules, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        rules.limits.check()?;
        rules.defence.check()?;
        rules.condition.check()?;
        let mut registry = Self { rules, ..Self::default() };
        for mut def in defs {
            def.body.size = (def.body.size.0 * def.body.scale, def.body.size.1 * def.body.scale);
            let error = |message: String| RegistryError(format!("creature `{}`: {message}", def.name));
            if def.health == 0 || def.body.size.0 <= 0.0 || def.body.size.1 <= 0.0 || def.body.colours.is_empty() {
                return Err(error("needs health, a size and at least one colour".into()));
            }
            if def.genes.len() > crate::genes::MOST {
                return Err(error(format!("has more than {} genes", crate::genes::MOST)));
            }
            if def.supernatural && def.habitat.comfort.aether.0 <= 0.0 {
                return Err(error("a supernatural creature needs an aether minimum in its comfort".into()));
            }
            let loot = loot::resolve(&def.loot, blocks).map_err(error)?;
            let foods = |names: &[String]| -> Result<Vec<_>, RegistryError> {
                let mut found = Vec::new();
                for name in names {
                    match name.strip_prefix("tag:") {
                        Some(tag) => found.extend(blocks.tagged(tag)),
                        None => found.push(blocks.id(name).ok_or_else(|| error(format!("eats `{name}`, which does not exist")))?),
                    }
                }
                Ok(found)
            };
            let (eats, favourite) = (foods(&def.diet.eats)?, foods(&def.diet.favourite)?);
            let or_favourite = |names: Option<&Vec<String>>| -> Result<Vec<_>, RegistryError> {
                match names.filter(|n| !n.is_empty()) {
                    Some(names) => foods(names),
                    None => Ok(favourite.clone()),
                }
            };
            let tame_food = match &def.taming {
                Some(taming) => or_favourite(Some(&taming.food))?,
                None => Vec::new(),
            };
            let breed_food = match &def.breeding {
                Some(breeding) => or_favourite(Some(&breeding.food))?,
                None => Vec::new(),
            };
            let products = def
                .products
                .iter()
                .map(|p| blocks.id(&p.item).ok_or_else(|| error(format!("gives `{}`, which does not exist", p.item))))
                .collect::<Result<Vec<_>, _>>()?;
            let xp_skill = match &def.xp.skill {
                Some(name) => Some(skills.id(name).ok_or_else(|| error(format!("no skill `{name}`")))?),
                None => None,
            };
            let behaviours = if def.behaviours.is_empty() { generated(&def) } else { def.behaviours.clone() };
            let parts = def.body.model.parts(def.body.scale);
            let layout = skin::layout(&parts);
            // Horns, ears and tails give way; the head, muzzle and neck do not.
            let reach = parts
                .iter()
                .filter(|p| !matches!(p.role, Role::Horn | Role::Ear | Role::Tail | Role::Wing | Role::Mote | Role::Crest))
                .map(|p| {
                    let half = p.size * 0.5;
                    let along = (p.rest * Vec3::new(half.x, 0.0, 0.0)).z.abs() + (p.rest * Vec3::new(0.0, half.y, 0.0)).z.abs() + (p.rest * Vec3::new(0.0, 0.0, half.z)).z.abs();
                    along - p.centre.z
                })
                .fold(0.0f32, f32::max);
            let head = parts.iter().find(|p| p.role == Role::Head).map_or(def.body.size.1, |p| p.centre.y);
            let modules = behaviours
                .iter()
                .map(|entry| (brain::modules::module(entry.behaviour), entry.weight))
                .chain(brain::modules::always().into_iter().map(|module| (module, 1.0)))
                .collect();
            let id = CreatureId(registry.creatures.len() as u16);
            if registry.by_name.insert(def.name.clone(), id).is_some() {
                return Err(error("defined twice".into()));
            }
            registry.creatures.push(Creature { def, behaviours, modules, parts, reach, head, layout, loot, eats, favourite, xp_skill, tame_food, breed_food, products });
        }
        Ok(registry)
    }

    pub fn get(&self, id: CreatureId) -> &Creature {
        &self.creatures[id.0 as usize]
    }

    /// How many creatures the world holds.
    pub fn limits(&self) -> Limits {
        self.rules.limits
    }

    /// When creatures' blows train Defence.
    pub fn defence(&self) -> DefenceTraining {
        self.rules.defence
    }

    /// How kept animals' condition is reckoned.
    pub fn condition(&self) -> &condition::ConditionRules {
        &self.rules.condition
    }

    pub fn id(&self, name: &str) -> Option<CreatureId> {
        self.by_name.get(name).copied()
    }

    /// A species by its full name or its name alone (`rabbit` for `base:rabbit`).
    pub fn find(&self, name: &str) -> Option<CreatureId> {
        self.id(name).or_else(|| self.iter().find(|(_, c)| c.def.name.rsplit(':').next() == Some(name)).map(|(id, _)| id))
    }

    pub fn iter(&self) -> impl Iterator<Item = (CreatureId, &Creature)> {
        self.creatures.iter().enumerate().map(|(i, c)| (CreatureId(i as u16), c))
    }

    pub fn len(&self) -> usize {
        self.creatures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.creatures.is_empty()
    }
}

/// The behaviours a species that lists none gets, from its temperament, diet and attack:
/// the first, ecology-driven layer of behaviour generation (docs/creature-ai.md).
pub fn generated(def: &CreatureDef) -> Vec<BehaviourEntry> {
    let t = def.temperament;
    let entry = |behaviour, weight| BehaviourEntry { behaviour, weight };
    let mut list = vec![entry(Behaviour::Wander { radius: def.leash.min(12.0) }, 1.0), entry(Behaviour::LookAround, 1.0)];
    if !def.diet.eats.is_empty() {
        list.push(entry(Behaviour::Graze, 1.0));
    }
    if t.fear > 0.3 {
        list.push(entry(Behaviour::Flee { radius: 4.0 + 10.0 * t.fear }, 1.0));
    }
    if t.fear > 0.7 || t.nervousness > 0.7 {
        list.push(entry(Behaviour::Freeze, 1.0));
    }
    if t.nervousness > 0.4 && t.fear > 0.3 {
        list.push(entry(Behaviour::StandToLook, 1.0));
    }
    if t.sociability > 0.4 && def.group.size.1 > 1 {
        list.push(entry(Behaviour::Herd { spacing: def.body.size.0 * 3.0 + 1.0 }, 1.0));
        if t.fear > 0.4 {
            list.push(entry(Behaviour::Sentinel, 1.0));
        }
    }
    if !def.diet.favourite.is_empty() {
        list.push(entry(Behaviour::Tempted { radius: 10.0 }, 1.0));
    }
    if t.aggression > 0.5 && def.attack.is_some() {
        list.push(entry(Behaviour::Hunt { radius: 6.0 + 12.0 * t.aggression }, 1.0));
    } else if def.attack.is_some() {
        list.push(entry(Behaviour::Defend, 1.0));
    }
    if t.curiosity > 0.5 {
        list.push(entry(Behaviour::Curious { distance: 5.0 }, 1.0));
    }
    list
}

/// `wild_boar` → `Wild boar`.
fn title_case(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// A well-mixed 64-bit hash of two values: the seed of every random choice creatures make
/// (docs/creature-ai.md, decision 5), so a replay repeats them.
pub fn mix(a: u64, b: u64) -> u64 {
    let mut h = a ^ b.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// A number in 0..1 from a hash.
pub fn unit_from(hash: u64) -> f32 {
    (hash >> 40) as f32 / (1u64 << 24) as f32
}

/// How much daylight there is at `day` ticks after the first sunrise (24,000 a day): 1
/// through the middle of the day, 0 through the night, with ramps around sunrise and
/// sunset (a quarter of the way up at either). The sun is up two thirds of the day
/// (`calendar::sun_height`).
pub fn daylight(day: u64) -> f32 {
    let t = (day % 24_000) as f64 / 24_000.0;
    let elevation = crate::calendar::sun_height(t) as f32;
    ((elevation + 0.1) / 0.35).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;
    use crate::content::Settable;

    const RABBIT: &str = r#"(creatures: [(
        name: "rabbit", health: 6, level: 1,
        body: (size: (0.5, 0.45), model: Quadruped(body: (0.36, 0.3, 0.5), head: (0.28, 0.26, 0.28), legs: 0.12, ears: (0.08, 0.24), tail: (0.1, 0.1, 0.1)), colours: [(150, 120, 90), (230, 220, 200)]),
        habitat: (biomes: [("tag:meadow", 1.0), ("forest", 0.5)], comfort: (temperature: (-10.0, 28.0))),
        temperament: (fear: 0.9, sociability: 0.5), group: (size: (1, 3)),
        diet: (eats: ["short_grass"], favourite: ["carrot"]),
        loot: [(item: "carrot", count: (1, 2))],
    )])"#;

    fn registry() -> CreatureRegistry {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:short_grass".into(), ..Default::default() },
            BlockDef { name: "base:carrot".into(), ..Default::default() },
        ])
        .unwrap();
        let defs = CreatureRegistry::read("base", RABBIT).unwrap().defs;
        CreatureRegistry::new(defs, Rules::default(), &blocks, &SkillRegistry::default()).unwrap()
    }

    #[test]
    fn species_load_with_names_resolved_and_behaviours_made() {
        let registry = registry();
        let id = registry.find("rabbit").unwrap();
        let rabbit = registry.get(id);
        assert_eq!((rabbit.def.name.as_str(), rabbit.def.title.as_str()), ("base:rabbit", "Rabbit"));
        assert_eq!((rabbit.eats.len(), rabbit.favourite.len(), rabbit.loot.len()), (1, 1, 1));
        let kinds: Vec<_> = rabbit.behaviours.iter().map(|b| std::mem::discriminant(&b.behaviour)).collect();
        for expected in [Behaviour::Graze, Behaviour::Flee { radius: 0.0 }, Behaviour::Freeze, Behaviour::Herd { spacing: 0.0 }, Behaviour::Tempted { radius: 0.0 }] {
            assert!(kinds.contains(&std::mem::discriminant(&expected)), "{expected:?} missing");
        }
        // Body, four legs, head, two ears, a tail.
        assert_eq!(rabbit.parts.len(), 9);
        assert!(rabbit.parts.iter().all(|p| p.centre.y - p.size.y / 2.0 >= -1e-4), "a part pokes through the ground");
        assert_eq!(rabbit.layout.faces.len(), 9);
    }

    #[test]
    fn biomes_match_by_tag_or_name_and_climate_fades() {
        let registry = registry();
        let habitat = &registry.get(CreatureId(0)).def.habitat;
        assert_eq!(habitat.biome_weight("base:plains", &["meadow".into()]), 1.0);
        assert_eq!(habitat.biome_weight("base:forest", &[]), 0.5);
        assert_eq!(habitat.biome_weight("base:desert", &["dry".into()]), 0.0);
        let comfort = habitat.comfort;
        assert_eq!(fit(&comfort, 10.0, 1.0, 0.0), 1.0);
        assert!((fit(&comfort, 30.5, 1.0, 0.0) - 0.5).abs() < 1e-5);
        assert_eq!(fit(&comfort, 40.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn supernatural_creatures_need_aether() {
        let source = RABBIT.replace("health: 6", "health: 6, supernatural: true");
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:short_grass".into(), ..Default::default() },
            BlockDef { name: "base:carrot".into(), ..Default::default() },
        ])
        .unwrap();
        let defs = CreatureRegistry::read("base", &source).unwrap().defs;
        assert!(CreatureRegistry::new(defs, Rules::default(), &blocks, &SkillRegistry::default()).is_err());
    }

    #[test]
    fn limits_load_from_the_file_and_are_checked() {
        let source = RABBIT.replacen("(creatures:", "(limits: (wild_most: 20, kept_each: 5), creatures:", 1);
        let set = CreatureRegistry::read("base", &source).unwrap().limits.expect("set");
        assert_eq!(set, SetLimits { wild_most: Some(20), kept_each: Some(5), ..Default::default() });
        assert_eq!(CreatureRegistry::read("base", RABBIT).unwrap().limits, None, "a file without limits leaves them to another");
        // What a package sets replaces what was there; the rest stays.
        let (limits, changed) = Limits::default().with(&set);
        assert_eq!(changed, ["wild_most", "kept_each"]);
        assert_eq!((limits.wild_most, limits.kept_each, limits.most), (20, 5, Limits::default().most));
        assert!(Limits::default().check().is_ok());
        // Room for no wild ones, less than one player's surroundings draw, or less in all
        // than the wild ones and every keeper's animals, will not load.
        let ok = Limits::default();
        for bad in [
            Limits { wild_most: 0, animals_near: 0, monsters_near: 0, ..ok },
            Limits { wild_most: 10, ..ok },
            Limits { most: ok.wild_most + ok.keepers * ok.kept_each - 1, ..ok },
            Limits { keepers: usize::MAX, ..ok },
        ] {
            let blocks = BlockRegistry::new(vec![]).unwrap();
            assert!(CreatureRegistry::new(Vec::new(), Rules { limits: bad, ..Default::default() }, &blocks, &SkillRegistry::default()).is_err(), "{bad:?} loaded");
        }
    }

    #[test]
    fn defence_training_loads_from_the_file_and_is_checked() {
        let source = RABBIT.replacen("(creatures:", "(defence: (least_level: 0.75), creatures:", 1);
        let set = CreatureRegistry::read("base", &source).unwrap().defence.expect("set");
        assert_eq!(CreatureRegistry::read("base", RABBIT).unwrap().defence, None);
        // What it leaves out keeps what was there.
        let (rules, changed) = DefenceTraining::default().with(&set);
        assert_eq!((rules.fought_within, rules.least_level, changed), (10.0, 0.75, vec!["least_level"]));
        // At least three quarters of Defence 20 is 15.
        assert!(rules.matches(15, 20) && !rules.matches(14, 20));
        assert!(DefenceTraining::default().matches(10, 20) && !DefenceTraining::default().matches(9, 20));
        let blocks = BlockRegistry::new(vec![]).unwrap();
        for bad in [(0.0, 0.5), (f32::NAN, 0.5), (10.0, -0.1), (10.0, f32::INFINITY)] {
            let defence = DefenceTraining { fought_within: bad.0, least_level: bad.1 };
            assert!(CreatureRegistry::new(Vec::new(), Rules { defence, ..Default::default() }, &blocks, &SkillRegistry::default()).is_err(), "{bad:?} loaded");
        }
    }

    #[test]
    fn condition_rules_load_from_the_file_and_are_checked() {
        let source = RABBIT.replacen("(creatures:", "(condition: (up: 0.2, fed: (0.5, 1.5)), creatures:", 1);
        let set = CreatureRegistry::read("base", &source).unwrap().condition.expect("set");
        // What it leaves out keeps what was there.
        let (rules, changed) = condition::ConditionRules::default().with(&set);
        assert_eq!((rules.up, rules.fed, rules.down), (0.2, (0.5, 1.5), 0.25));
        assert_eq!(changed, ["up", "fed"]);
        let source = RABBIT.replace("health: 6,", "health: 6, needs: (space: 2, shelter: false),");
        let rabbit = &CreatureRegistry::read("base", &source).unwrap().defs[0];
        assert_eq!((rabbit.needs.space(0.5), rabbit.needs.shelter()), (2.0, false));
        let blocks = BlockRegistry::new(vec![]).unwrap();
        let bad = condition::ConditionRules { tamed: 2.0, ..Default::default() };
        assert!(CreatureRegistry::new(Vec::new(), Rules { condition: bad, ..Default::default() }, &blocks, &SkillRegistry::default()).is_err());
    }

    #[test]
    fn daylight_follows_the_sun() {
        assert_eq!(daylight(8000), 1.0);
        assert_eq!(daylight(20_000), 0.0);
        assert!(daylight(0) > 0.0 && daylight(0) < 1.0);
        assert!(daylight(16_000) > 0.0 && daylight(16_000) < 1.0);
        assert_eq!(daylight(30_000), 1.0);
    }
}
