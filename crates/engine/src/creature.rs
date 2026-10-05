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
//! Bodies are boxes from a small part library ([`Model`]), coloured by the species and
//! animated from how fast the creature moves, so the client needs no model files.

pub mod brain;
pub mod husbandry;
pub mod motion;

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
}

/// A body plan from the part library. Sizes are (width, height, length) in blocks.
#[derive(Clone, Debug, Deserialize)]
pub enum Model {
    /// Four legs: rabbits to bears.
    Quadruped {
        body: (f32, f32, f32),
        head: (f32, f32, f32),
        /// Leg length.
        legs: f32,
        #[serde(default)]
        ears: Option<(f32, f32)>,
        #[serde(default)]
        tail: Option<(f32, f32, f32)>,
        #[serde(default)]
        horns: Option<(f32, f32)>,
        #[serde(default)]
        snout: Option<(f32, f32, f32)>,
    },
    /// Two legs: birds and walking things.
    Biped {
        body: (f32, f32, f32),
        head: (f32, f32, f32),
        legs: f32,
        /// Arm length; none for birds.
        #[serde(default)]
        arms: Option<f32>,
        #[serde(default)]
        wings: Option<(f32, f32)>,
        #[serde(default)]
        beak: Option<(f32, f32, f32)>,
        #[serde(default)]
        tail: Option<(f32, f32, f32)>,
    },
    /// Many legs low to the ground: spiders and crabs.
    Crawler { body: (f32, f32, f32), head: (f32, f32, f32), legs: f32, pairs: u8 },
    /// A body with no legs: snails, slugs, wisps.
    Blob {
        body: (f32, f32, f32),
        #[serde(default)]
        head: Option<(f32, f32, f32)>,
        #[serde(default)]
        shell: Option<(f32, f32, f32)>,
    },
}

/// What a part does when the creature moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Swing {
    Still,
    /// Swings fore and aft with the stride; the phase is 0 or a half step.
    Leg { phase: f32 },
    /// Turns toward what the creature attends to and dips when it grazes.
    Head,
    /// Sways behind.
    Tail,
    /// Flaps, on the given side (−1 left, 1 right).
    Wing { side: f32 },
}

/// One box of a body, in model space: feet at the origin, facing −Z (as yaw 0 looks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub size: Vec3,
    /// The box's centre.
    pub centre: Vec3,
    /// The point it swings about (a leg's hip, the neck).
    pub pivot: Vec3,
    /// Index into the body's colours, or [`EYE`].
    pub colour: u8,
    pub swing: Swing,
}

/// The colour index of eyes, drawn dark (or glowing, for glowing bodies).
pub const EYE: u8 = u8::MAX;

impl Model {
    /// The boxes the plan is made of, scaled by `scale`.
    pub fn parts(&self, scale: f32) -> Vec<Part> {
        let v = |(x, y, z): (f32, f32, f32)| Vec3::new(x, y, z) * scale;
        let mut parts = Vec::new();
        let mut part = |size: Vec3, centre: Vec3, pivot: Vec3, colour: u8, swing: Swing| {
            parts.push(Part { size, centre, pivot, colour, swing });
        };
        // Two eyes on the front of a head, high on its sides.
        let eyes = |part: &mut dyn FnMut(Vec3, Vec3, Vec3, u8, Swing), head: Vec3, centre: Vec3, pivot: Vec3| {
            let eye = Vec3::splat((head.y * 0.18).max(0.03));
            for side in [-1.0, 1.0] {
                let at = centre + Vec3::new(side * head.x * 0.3, head.y * 0.15, -head.z / 2.0 - eye.z * 0.3);
                part(eye, at, pivot, EYE, Swing::Head);
            }
        };
        match self {
            Model::Quadruped { body, head, legs, ears, tail, horns, snout } => {
                let (body, head, legs) = (v(*body), v(*head), *legs * scale);
                let torso = Vec3::new(0.0, legs + body.y / 2.0, 0.0);
                part(body, torso, torso, 0, Swing::Still);
                let leg = Vec3::new((body.x * 0.28).max(0.06), legs, (body.z * 0.16).max(0.06));
                for (i, (sx, sz)) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
                    let hip = Vec3::new(sx * (body.x / 2.0 - leg.x / 2.0), legs, sz * (body.z / 2.0 - leg.z / 2.0));
                    let phase = if i == 0 || i == 3 { 0.0 } else { 0.5 };
                    part(leg, hip - Vec3::Y * legs / 2.0, hip, 1, Swing::Leg { phase });
                }
                // The head sits forward of the body, its bottom at the body's top third.
                let neck = Vec3::new(0.0, legs + body.y * 0.75, -body.z / 2.0);
                let centre = neck + Vec3::new(0.0, head.y * 0.25, -head.z * 0.4);
                part(head, centre, neck, 0, Swing::Head);
                eyes(&mut part, head, centre, neck);
                if let Some(snout) = snout.map(v) {
                    part(snout, centre + Vec3::new(0.0, -head.y / 2.0 + snout.y / 2.0, -head.z / 2.0 - snout.z / 2.0), neck, 1, Swing::Head);
                }
                if let Some((w, h)) = *ears {
                    let ear = Vec3::new(w * scale, h * scale, w * scale * 0.5);
                    for side in [-1.0, 1.0] {
                        part(ear, centre + Vec3::new(side * head.x * 0.3, head.y / 2.0 + ear.y / 2.0, head.z * 0.15), neck, 0, Swing::Head);
                    }
                }
                if let Some((w, h)) = *horns {
                    let horn = Vec3::new(w * scale, h * scale, w * scale);
                    for side in [-1.0, 1.0] {
                        part(horn, centre + Vec3::new(side * head.x * 0.4, head.y / 2.0 + horn.y / 2.0, 0.0), neck, 2, Swing::Head);
                    }
                }
                if let Some(tail) = tail.map(v) {
                    let root = Vec3::new(0.0, legs + body.y * 0.8, body.z / 2.0);
                    part(tail, root + Vec3::new(0.0, -tail.y * 0.3, tail.z / 2.0), root, 1, Swing::Tail);
                }
            }
            Model::Biped { body, head, legs, arms, wings, beak, tail } => {
                let (body, head, legs) = (v(*body), v(*head), *legs * scale);
                let torso = Vec3::new(0.0, legs + body.y / 2.0, 0.0);
                part(body, torso, torso, 0, Swing::Still);
                let leg = Vec3::new((body.x * 0.25).max(0.05), legs, (body.x * 0.25).max(0.05));
                for (side, phase) in [(-1.0, 0.0), (1.0, 0.5)] {
                    let hip = Vec3::new(side * body.x * 0.25, legs, 0.0);
                    part(leg, hip - Vec3::Y * legs / 2.0, hip, 2, Swing::Leg { phase });
                }
                let neck = Vec3::new(0.0, legs + body.y, -body.z * 0.2);
                let centre = neck + Vec3::new(0.0, head.y / 2.0, -head.z * 0.1);
                part(head, centre, neck, 0, Swing::Head);
                eyes(&mut part, head, centre, neck);
                if let Some(beak) = beak.map(v) {
                    part(beak, centre + Vec3::new(0.0, -head.y * 0.1, -head.z / 2.0 - beak.z / 2.0), neck, 2, Swing::Head);
                }
                if let Some(arms) = arms.map(|a| a * scale) {
                    let arm = Vec3::new(leg.x * 0.9, arms, leg.z * 0.9);
                    for (side, phase) in [(-1.0, 0.5), (1.0, 0.0)] {
                        let shoulder = Vec3::new(side * (body.x / 2.0 + arm.x / 2.0), legs + body.y * 0.9, 0.0);
                        part(arm, shoulder - Vec3::Y * arms / 2.0, shoulder, 0, Swing::Leg { phase });
                    }
                }
                if let Some((w, l)) = *wings {
                    let wing = Vec3::new(w * scale, body.y * 0.6, l * scale);
                    for side in [-1.0, 1.0] {
                        let shoulder = Vec3::new(side * body.x / 2.0, legs + body.y * 0.75, 0.0);
                        part(wing, shoulder + Vec3::new(side * wing.x / 2.0, -wing.y / 2.0, 0.0), shoulder, 1, Swing::Wing { side });
                    }
                }
                if let Some(tail) = tail.map(v) {
                    let root = Vec3::new(0.0, legs + body.y * 0.6, body.z / 2.0);
                    part(tail, root + Vec3::new(0.0, 0.0, tail.z / 2.0), root, 1, Swing::Tail);
                }
            }
            Model::Crawler { body, head, legs, pairs } => {
                let (body, head, legs) = (v(*body), v(*head), *legs * scale);
                let height = legs * 0.5;
                let torso = Vec3::new(0.0, height + body.y / 2.0, 0.0);
                part(body, torso, torso, 0, Swing::Still);
                let centre = Vec3::new(0.0, height + head.y / 2.0, -body.z / 2.0 - head.z / 2.0);
                part(head, centre, centre, 0, Swing::Head);
                eyes(&mut part, head, centre, centre);
                let pairs = (*pairs).clamp(1, 4) as usize;
                let leg = Vec3::new(legs, (legs * 0.12).max(0.04), (legs * 0.12).max(0.04));
                for i in 0..pairs {
                    let z = -body.z / 2.0 + body.z * (i as f32 + 0.5) / pairs as f32;
                    for side in [-1.0, 1.0] {
                        let hip = Vec3::new(side * body.x / 2.0, height + body.y * 0.3, z);
                        let phase = if (i % 2 == 0) == (side > 0.0) { 0.0 } else { 0.5 };
                        part(leg, hip + Vec3::new(side * legs / 2.0, -height * 0.4, 0.0), hip, 1, Swing::Leg { phase });
                    }
                }
            }
            Model::Blob { body, head, shell } => {
                let body = v(*body);
                let torso = Vec3::new(0.0, body.y / 2.0, 0.0);
                part(body, torso, torso, 0, Swing::Still);
                if let Some(head) = head.map(v) {
                    let centre = Vec3::new(0.0, body.y * 0.6 + head.y / 2.0, -body.z / 2.0 + head.z / 2.0);
                    part(head, centre, centre, 0, Swing::Head);
                    eyes(&mut part, head, centre, centre);
                } else {
                    eyes(&mut part, body, torso, torso);
                }
                if let Some(shell) = shell.map(v) {
                    part(shell, Vec3::new(0.0, body.y + shell.y / 2.0 - body.y * 0.2, body.z * 0.1), torso, 1, Swing::Still);
                }
            }
        }
        parts
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
}

/// A species with its names resolved.
#[derive(Clone, Debug)]
pub struct Creature {
    pub def: CreatureDef,
    pub behaviours: Vec<BehaviourEntry>,
    /// The modules that run its behaviours, each with its weight, then the ones every
    /// creature has.
    pub modules: Vec<(Arc<dyn brain::Module>, f32)>,
    /// Its body's boxes at full size.
    pub parts: Vec<Part>,
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
}

impl CreatureRegistry {
    /// Reads a package's `creatures.ron`, qualifying names with its namespace.
    pub fn read(namespace: &str, source: &str) -> Result<Vec<CreatureDef>, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: CreatureFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/creatures.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') && !name.starts_with("tag:") {
                *name = format!("{namespace}:{name}");
            }
        };
        Ok(file
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
            .collect())
    }

    /// Resolves species against the blocks (items) and skills.
    pub fn new(defs: Vec<CreatureDef>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        for def in defs {
            let error = |message: String| RegistryError(format!("creature `{}`: {message}", def.name));
            if def.health == 0 || def.body.size.0 <= 0.0 || def.body.size.1 <= 0.0 || def.body.colours.is_empty() {
                return Err(error("needs health, a size and at least one colour".into()));
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
            let parts = def.body.model.parts(1.0);
            let modules = behaviours
                .iter()
                .map(|entry| (brain::modules::module(entry.behaviour), entry.weight))
                .chain(brain::modules::always().into_iter().map(|module| (module, 1.0)))
                .collect();
            let id = CreatureId(registry.creatures.len() as u16);
            if registry.by_name.insert(def.name.clone(), id).is_some() {
                return Err(error("defined twice".into()));
            }
            registry.creatures.push(Creature { def, behaviours, modules, parts, loot, eats, favourite, xp_skill, tame_food, breed_food, products });
        }
        Ok(registry)
    }

    pub fn get(&self, id: CreatureId) -> &Creature {
        &self.creatures[id.0 as usize]
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
/// sunset (a quarter of the way up at either).
pub fn daylight(day: u64) -> f32 {
    let t = (day % 24_000) as f32 / 24_000.0;
    // The sun's height: up at sunrise (t = 0), highest at noon, down at sunset (t = ½).
    let elevation = (t * std::f32::consts::TAU).sin();
    ((elevation + 0.1) / 0.35).clamp(0.0, 1.0)
}

/// A creature's chance to hit, RuneScape-style: attack and defence rolls from levels.
pub fn hit_chance(attack_level: u8, defence_level: u8) -> f32 {
    let a = (attack_level as f32 + 8.0) * 64.0;
    let d = (defence_level as f32 + 8.0) * 64.0;
    if a > d { 1.0 - (d + 2.0) / (2.0 * (a + 1.0)) } else { a / (2.0 * (d + 1.0)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

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
        let defs = CreatureRegistry::read("base", RABBIT).unwrap();
        CreatureRegistry::new(defs, &blocks, &SkillRegistry::default()).unwrap()
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
        // Body, four legs, head, two eyes, two ears, a tail.
        assert_eq!(rabbit.parts.len(), 11);
        assert!(rabbit.parts.iter().all(|p| p.centre.y - p.size.y / 2.0 >= -1e-4), "a part pokes through the ground");
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
        let defs = CreatureRegistry::read("base", &source).unwrap();
        assert!(CreatureRegistry::new(defs, &blocks, &SkillRegistry::default()).is_err());
    }

    #[test]
    fn daylight_follows_the_sun() {
        assert_eq!(daylight(6000), 1.0);
        assert_eq!(daylight(18_000), 0.0);
        assert!(daylight(0) > 0.0 && daylight(0) < 1.0);
        assert!(daylight(12_000) > 0.0 && daylight(12_000) < 1.0);
        assert_eq!(daylight(30_000), 1.0);
    }

    #[test]
    fn hits_follow_levels() {
        assert!((hit_chance(1, 1) - 0.4975).abs() < 0.01);
        assert!(hit_chance(60, 1) > 0.9);
        assert!(hit_chance(1, 60) < 0.1);
    }
}
