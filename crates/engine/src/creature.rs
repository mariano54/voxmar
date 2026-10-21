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
pub mod moves;
pub mod path;
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
    /// People of settlements: placed by their settlement near players, never by the land,
    /// never kept, and kept by the server only while a player is near their home.
    Villager,
}

/// A short, wordless sound one of a settlement's people makes as they react to something
/// (`Server::bark`), heard as their voice's sound of that name (`creature_sounds.ron`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bark {
    /// A hum of greeting, to a player they meet.
    Greet,
    /// A sharp cry of alarm: a threat, a fire, someone struck.
    Alarm,
    /// A content sigh: a meal, rest at home, a job done.
    Content,
}

impl Bark {
    pub const ALL: [Bark; 3] = [Bark::Greet, Bark::Alarm, Bark::Content];

    /// Its name, as commands and logs write it.
    pub fn name(self) -> &'static str {
        match self {
            Bark::Greet => "greet",
            Bark::Alarm => "alarm",
            Bark::Content => "content",
        }
    }
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
    /// Floats in the air (wisps). Birds that fly are `Style::Flier`s on land.
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
    /// Between blows it circles its target rather than standing nose to nose (unset, it
    /// closes at once): once it has recovered it backs off and moves round them, facing
    /// them, then comes in for its next blow (`Circle`).
    #[serde(default)]
    pub circle: Option<Circle>,
    /// A charge: once wound up it runs straight at where its target stood, blind to walls,
    /// holes and cliff edges (`moves::Charge`, docs/bosses.md).
    #[serde(default)]
    pub charge: Option<moves::Charge>,
}

/// How a creature circles its target between blows (a wolf): for `seconds` after its
/// recovery it backs off to `away` blocks beyond its reach and moves round the target at
/// `pace` of its run, swinging round by up to `round` degrees, facing them all the while;
/// then it closes and strikes. Those seconds come on top of its `every`.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Circle {
    pub seconds: f32,
    pub away: f32,
    pub pace: f32,
    pub round: f32,
}

impl Circle {
    /// The least and most of each: long enough to show, short enough that a fight keeps
    /// its beat; off to a side, not out of the fight.
    pub fn check(&self) -> Result<(), String> {
        if !(0.3..=2.0).contains(&self.seconds) || !(0.3..=2.5).contains(&self.away) || !(0.2..=1.0).contains(&self.pace) || !(15.0..=120.0).contains(&self.round) {
            return Err(format!(
                "`circle` needs seconds 0.3 to 2, away 0.3 to 2.5 blocks, pace 0.2 to 1 of its run and round 15 to 120 degrees, not {self:?}"
            ));
        }
        Ok(())
    }
}

/// How a creature warns a player before it attacks (docs/creature-ai.md, "Warnings"): it
/// stands and faces them in its warning stance (`stance`, drawn as a tell is, with its
/// `warn` sound) for `seconds`; then, or at once if they come within `near` blocks, it
/// goes for them as if struck. A player who backs off first is let be. A hunter warns
/// those its hunt would go for, as far off (and so has no `radius`): the warning only
/// tells its chase before it comes. One that only defends itself (a badger, a bear) warns
/// those who come within `radius` blocks of it, then defends its ground.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Warn {
    #[serde(default)]
    pub radius: Option<f32>,
    pub near: f32,
    pub seconds: f32,
    #[serde(default)]
    pub stance: moves::Tell,
}

impl Warn {
    /// Far enough to be seen and backed away from, near enough to be about this creature;
    /// long enough to be read, short enough that it is no free stare. `hunts`: whether the
    /// creature hunts, warning at its hunt's radius rather than its own.
    pub fn check(&self, hunts: bool) -> Result<(), String> {
        match (hunts, self.radius) {
            (true, Some(_)) => return Err("a hunter warns as far off as it hunts: its `warn` has no `radius`".into()),
            (false, None) => return Err("one that does not hunt needs a `radius` to warn within".into()),
            _ => {}
        }
        let radius = self.radius.unwrap_or(16.0);
        let stance = [self.stance.nod, self.stance.crouch, self.stance.rear];
        if !(2.0..=16.0).contains(&radius) || !(0.5..=radius - 1.0).contains(&self.near) || !(0.5..=4.0).contains(&self.seconds) || stance.iter().any(|v| !(0.0..=1.0).contains(v)) {
            return Err(format!(
                "`warn` needs radius 2 to 16 blocks, near 0.5 to a block inside it, seconds 0.5 to 4 and its stance 0 to 1, not {self:?}"
            ));
        }
        Ok(())
    }
}

/// How a wild creature takes fire (docs/creature-ai.md, "Fire"). A player holding a fire at
/// it (anything tagged `fire`: a torch) within `give` blocks, one it has noticed, it backs away
/// from, facing them, until they are `give` off, its warning let lapse; unless it was
/// provoked, or is one of the bold (`fear` is the share of its kind that give ground, each
/// individual by its own roll; the rest stand theirs).
/// At night, if `ward`, it keeps out of a lit fire's ward (`BlockDef::ward`,
/// `MultiblockDef::ward`: a campfire, a lit hearth or kiln), and goes for no one inside one
/// unprovoked. Kept animals and those on an errand (villagers, guards) pay fire no heed.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fire {
    pub fear: f32,
    pub give: f32,
    #[serde(default = "yes")]
    pub ward: bool,
}

fn yes() -> bool {
    true
}

/// What a held item is tagged to be a fire to a creature (`Fire`): a torch, as the
/// `Holding("fire")` of status effects.
pub const HELD_FIRE: &str = "fire";

impl Fire {
    /// Whether the individual of hash `seed` (`brain::Mind::seed`) is one of the share
    /// (`fear`) of its kind that give ground before a torch; the rest stand theirs.
    pub fn gives_ground(&self, seed: u64) -> bool {
        unit_from(mix(seed, 0xf12e)) < self.fear
    }

    /// Afraid enough that some of its kind give ground, far enough to be backed from, near
    /// enough that a torch is held at it, not waved from across a field.
    pub fn check(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.fear) || !(2.0..=8.0).contains(&self.give) {
            return Err(format!("`fire` needs fear 0 to 1 and give 2 to 8 blocks, not {self:?}"));
        }
        Ok(())
    }
}

/// How a game bird sits tight (docs/creature-ai.md, "Flush"): a player it has noticed
/// beyond `hold` blocks it lets be, crouched still in the grass and hoping not to be seen;
/// one who comes within `hold` it bursts away from, at `burst` times its run for its first
/// `seconds`, and its kin burst with it. So a stalker who keeps coming gets close, and is
/// startled at the last. Hurt, it flees as any prey does; a kept bird or one on an errand
/// sits tight for no one.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flush {
    pub hold: f32,
    pub burst: f32,
    pub seconds: f32,
}

impl Flush {
    /// Close enough to be a surprise, but not so close it is stepped on; a burst faster
    /// than its run, and short.
    pub fn check(&self) -> Result<(), String> {
        if !(1.0..=6.0).contains(&self.hold) || !(1.0..=2.5).contains(&self.burst) || !(0.2..=3.0).contains(&self.seconds) {
            return Err(format!("`flush` needs hold 1 to 6 blocks, burst 1 to 2.5 and seconds 0.2 to 3, not {self:?}"));
        }
        Ok(())
    }
}

/// How a creature scavenges (docs/creature-ai.md, "Scavenging"): it slinks in to food
/// left lying on the ground within `reach` blocks, by item name or `tag:` (`takes`), and
/// makes off home with one piece, which it eats there; killed first, it drops it. Stuck
/// short of food (a fence, a wall between), it leaves it be for `give_up` seconds. It
/// comes only while no player is within `shy` blocks of it or the food, so a hunter who
/// leaves the kill lying, turns their back and walks off may lose a share of it. Wild
/// only: a kept one or one on an errand lets food lie.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scavenge {
    pub takes: Vec<String>,
    pub reach: f32,
    pub shy: f32,
    /// Seconds it leaves be food it got stuck short of (a fence, a wall between), as a
    /// chase with no way is given up.
    #[serde(default = "scrap_give_up")]
    pub give_up: f32,
}

fn scrap_give_up() -> f32 {
    30.0
}

impl Scavenge {
    /// Near enough to be watched coming, and shy of a player close enough to stop it.
    pub fn check(&self) -> Result<(), String> {
        if self.takes.is_empty() || !(4.0..=24.0).contains(&self.reach) || !(2.0..=12.0).contains(&self.shy) || self.shy >= self.reach || !(5.0..=120.0).contains(&self.give_up) {
            return Err(format!("`scavenge` needs something it takes, reach 4 to 24 blocks, shy 2 to 12 blocks, less than its reach, and give_up 5 to 120 seconds, not {self:?}"));
        }
        Ok(())
    }
}

/// How a creature tires as it flees (docs/creature-ai.md, "Winded"): after `after`
/// seconds of running flat out it is winded, and runs at `pace` of its run for `rest`
/// seconds; then flat out again. Time not spent fleeing gets its wind back a second for a
/// second. Hurt, it limps: it runs `hurt` slower at the last of its health, in proportion
/// above (0 for none, up to 0.9), so each blow a hunter lands brings the catch nearer. A
/// kept animal or one on an errand runs as ever.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tires {
    pub after: f32,
    pub pace: f32,
    pub rest: f32,
    #[serde(default)]
    pub hurt: f32,
}

impl Tires {
    /// A real sprint first, a real slowing after, and a rest long enough to be caught in
    /// but not so long that it is caught standing.
    pub fn check(&self) -> Result<(), String> {
        if !(1.0..=20.0).contains(&self.after) || !(0.2..=0.9).contains(&self.pace) || !(1.0..=30.0).contains(&self.rest) || !(0.0..=0.9).contains(&self.hurt) {
            return Err(format!("`tires` needs after 1 to 20 seconds, pace 0.2 to 0.9, rest 1 to 30 seconds and hurt 0 to 0.9, not {self:?}"));
        }
        Ok(())
    }
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
    /// Comes only with a wandering party (`wandering.ron`), or a boss in its lairs
    /// (`lairs.ron`), never from the land's own population: a bandit patrol's raiders, a
    /// hunter's hound, the Rootmother.
    pub party_only: bool,
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

/// How far it notices things, in blocks, and where it looks.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Senses {
    pub sight: f32,
    pub hearing: f32,
    /// How far either way of where it faces it sees, in degrees (a half angle): 180 sees
    /// all round; a deer's eyes on the sides of its head see most of the way round, a
    /// wolf's to the front. A player out of it, or behind something that hides them, it
    /// notices only close by (`close`) or once they hurt it.
    pub field: f32,
    /// How near, in blocks, it notices a player wherever it looks: it hears and smells them.
    pub close: f32,
    /// Seconds it stands and looks at a player it has just noticed before it acts on
    /// them (runs, chases, comes to look): a double take, the moment a player has to back
    /// off. Not for one who hurts it.
    pub react: f32,
    /// Seconds it keeps a player it noticed in mind once it no longer senses them
    /// (`brain::Mind::noticed`): turned away from one it has seen, it does not forget them.
    pub keep: f32,
}

/// How one of a settlement's people barks (`Server::bark`): heard by players within
/// `range` blocks who see them, each bark to the same player no more than once in `every`
/// seconds (a greeting, an alarm and a sigh each keep their own).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Barking {
    pub range: f32,
    pub every: f32,
}

/// The farthest, in blocks, a bark carries.
pub const MOST_BARK_RANGE: f32 = 64.0;
/// The longest, in seconds, between one bark and the same again to the same player.
pub const MOST_BARK_EVERY: f32 = 600.0;

impl Default for Barking {
    fn default() -> Self {
        Self { range: 24.0, every: 6.0 }
    }
}

impl Barking {
    pub fn check(&self) -> Result<(), String> {
        if !(self.range >= 1.0 && self.range <= MOST_BARK_RANGE) {
            return Err(format!("`barking.range` is from 1 to {MOST_BARK_RANGE} blocks, not {}", self.range));
        }
        if !(self.every.is_finite() && (0.0..=MOST_BARK_EVERY).contains(&self.every)) {
            return Err(format!("`barking.every` is from 0 to {MOST_BARK_EVERY} seconds, not {}", self.every));
        }
        Ok(())
    }
}

impl Default for Senses {
    fn default() -> Self {
        Self { sight: 16.0, hearing: 12.0, field: 180.0, close: 3.0, react: 0.0, keep: 10.0 }
    }
}

impl Senses {
    pub fn check(&self) -> Result<(), String> {
        if !(self.field > 0.0 && self.field <= 180.0) {
            return Err(format!("`senses.field` is a half angle above 0 and up to 180 degrees, not {}", self.field));
        }
        if !(self.close.is_finite() && (0.0..=self.sight.max(0.0)).contains(&self.close)) {
            return Err(format!("`senses.close` is from 0 to its sight ({}), not {}", self.sight, self.close));
        }
        if !(self.react.is_finite() && (0.0..=MOST_REACT).contains(&self.react)) {
            return Err(format!("`senses.react` is from 0 to {MOST_REACT} seconds, not {}", self.react));
        }
        if !(self.keep > 0.0 && self.keep <= MOST_KEEP) {
            return Err(format!("`senses.keep` is above 0 and up to {MOST_KEEP} seconds, not {}", self.keep));
        }
        Ok(())
    }
}

/// The longest double take a creature's data may give it (`Senses::react`).
pub const MOST_REACT: f32 = 2.0;

/// The longest a creature's data may have it keep a player in mind (`Senses::keep`).
pub const MOST_KEEP: f32 = 60.0;

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
    /// How high it walks up without a hop, in blocks (a slab, a stair's tread), as a
    /// player steps: from a half block (what the way-finder takes for open ground) to a
    /// whole one.
    pub step: f32,
    /// How high a flier keeps over the ground (or the treetops) in flight, in blocks; only
    /// for `Flier`s.
    pub fly: f32,
    /// The highest a hop up a step rises, in blocks: the world's `climbing.step_rise`
    /// (`Climbing`), set on every species at load, not by a species.
    #[serde(skip)]
    pub step_rise: f32,
}

/// What a species walks up without a hop unless it says (`Movement::step`): a player's
/// step, nine sixteenths of a block.
pub const STEP: f32 = 0.5625;

impl Movement {
    /// How high it hops up what is in its way, if it jumps at all: the world's step rise.
    pub fn hop(&self) -> Option<f32> {
        (self.jump > 0.0).then_some(self.step_rise)
    }
}

impl Default for Movement {
    fn default() -> Self {
        Self { style: Style::Walker, walk: 1.6, run: 4.5, turn: 270.0, jump: 7.5, step: STEP, fly: 0.0, step_rise: Climbing::default().step_rise }
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
    /// Hops about on the ground and takes wing to go further or faster than a walk, keeping
    /// `fly` blocks over the land in flight and fluttering down to land: small birds.
    Flier,
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
    /// Goes where its day's routine sends it (`Situation::errand`): villagers. There once
    /// within `arrive` blocks of the spot; awake, it turns to a player within `look`.
    Routine { arrive: f32, look: f32 },
}

/// Experience for killing one, in `skill`, and how much its fights teach.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CreatureXp {
    pub kill: f32,
    pub skill: Option<String>,
    /// The share of the usual combat experience fighting it gives: for each point of
    /// damage dealt it and each of its blows turned aside. 0 where a fight should teach
    /// nothing (villagers who do not fight back).
    pub combat: f32,
}

impl Default for CreatureXp {
    fn default() -> Self {
        Self { kill: 0.0, skill: None, combat: 1.0 }
    }
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
    /// A boss: while one is alive near a player, the music of boss fights plays for them
    /// (`Boss` in `music.ron`).
    #[serde(default)]
    pub boss: bool,
    /// The music a fight with a boss plays: tags of boss tracks (`biomes` in `music.ron`,
    /// `forest` for the Rootmother). Tracks with none of them never play for it; the
    /// boss tracks with no tags at all are every boss's.
    #[serde(default)]
    pub music: Vec<String>,
    /// Its combat level, RuneScape-style.
    #[serde(default = "first")]
    pub level: u8,
    pub health: u16,
    #[serde(default)]
    pub attack: Option<Attack>,
    /// It warns a player who comes near before it attacks them (`Warn`): a wolf growls, a
    /// badger hisses, a bear stands up. Unset, a hunter goes straight for them and one that
    /// only defends itself lets them be.
    #[serde(default)]
    pub warn: Option<Warn>,
    /// It is afraid of fire (`Fire`): it gives ground from a torch held at it, and keeps out
    /// of a lit fire's ward at night. Unset, fire is nothing to it.
    #[serde(default)]
    pub fire: Option<Fire>,
    /// It tires as it flees (`Tires`): after a sprint it runs slower for a while, so a
    /// hunter on foot can run it down. Unset, it runs flat out as long as it flees.
    #[serde(default)]
    pub tires: Option<Tires>,
    /// It sits tight and then bursts away (`Flush`): a game bird lets a player close and
    /// startles them. Unset, it flees at its kind's distance.
    #[serde(default)]
    pub flush: Option<Flush>,
    /// It steals food left lying on the ground (`Scavenge`): a fox makes off with a
    /// hunter's meat. Unset, it lets dropped food lie.
    #[serde(default)]
    pub scavenge: Option<Scavenge>,
    /// Attacks besides `attack`, chosen by phase and distance (`moves::Creature::choose`).
    #[serde(default)]
    pub moves: Vec<moves::Move>,
    /// How far up and down it strikes (`moves::Strike`).
    #[serde(default)]
    pub strike: moves::Strike,
    /// The stages of its fight, by the health it falls to.
    #[serde(default)]
    pub phases: Vec<moves::Phase>,
    /// How it mends its wounds while it fights, if it does.
    #[serde(default)]
    pub mend: Option<moves::Mend>,
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
    /// How far its barks carry and how often it makes one to the same player.
    #[serde(default)]
    pub barking: Barking,
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
    /// The share of its health below which one that fights back runs instead (0 to 1); unset,
    /// a braver one holds longer (`Creature::holds`).
    #[serde(default)]
    pub flees_below: Option<f32>,
    /// Whether its young fight back as its grown do; unset, they run (a piglet).
    #[serde(default)]
    pub young_fight: bool,
}

/// How a creature falls when it dies, as the client draws it: it tips over onto its side
/// (away from whoever watches) over `fall`, lies there a beat, and as it goes (the last of
/// its death, `combat_fx.ron`'s `death.shrink`) sinks and shrinks into a puff of smoke, its
/// loot bursting out with it.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Death {
    /// Seconds from the blow to gone, its loot bursting out as it goes.
    pub seconds: f32,
    /// Seconds it takes to tip over, once the killing blow's freeze is over. It then lies a
    /// beat: the client refuses at launch a death that, less that freeze and the time it
    /// takes to go (`combat_fx.ron`), lies less than `LIES` (`FightLook::unlain`).
    pub fall: f32,
    /// Degrees it rolls onto its side.
    pub roll: f32,
    /// How far it sinks as it goes, in its own heights.
    pub sink: f32,
    /// Seconds from the blow to its loot coming out, and its body going with it, if sooner
    /// than `seconds`.
    pub drops: Option<f32>,
}

impl Default for Death {
    fn default() -> Self {
        Self { seconds: 0.9, fall: 0.3, roll: 90.0, sink: 0.4, drops: None }
    }
}

/// The least a creature lies fallen before it goes, in seconds.
pub const LIES: f32 = 0.3;

impl Death {
    /// Whether it tips over at all, lies a beat, goes within 10 s, sinks a sensible way and
    /// lets its loot out by the time it goes.
    pub fn check(&self) -> Result<(), String> {
        let lies = self.seconds.is_finite() && self.seconds <= 10.0 && self.fall > 0.0 && self.fall + LIES <= self.drops_after();
        if !lies || !(0.0..=2.0).contains(&self.sink) || self.drops.is_some_and(|d| !(0.0..=self.seconds).contains(&d)) {
            return Err(format!(
                "its `death` tips over in above 0 s, lies at least {LIES} s and is gone within 10 s, sinks 0 to 2 heights as it goes, and drops its loot no later than it goes (seconds {}, fall {}, sink {}, drops {:?})",
                self.seconds, self.fall, self.sink, self.drops
            ));
        }
        Ok(())
    }

    /// Seconds from the blow to its loot coming out, and its body going up in smoke.
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
    #[serde(default)]
    climbing: Option<SetClimbing>,
    #[serde(default)]
    genetics: Option<SetGenetics>,
}

/// One package's `creatures.ron`: its species, and what it sets of the world's rules.
#[derive(Debug)]
pub struct CreaturePackage {
    pub defs: Vec<CreatureDef>,
    pub limits: Option<SetLimits>,
    pub defence: Option<SetDefenceTraining>,
    pub condition: Option<condition::SetConditionRules>,
    pub climbing: Option<SetClimbing>,
    pub genetics: Option<SetGenetics>,
}

/// What packages set for every creature in the world: how many there are, how their
/// blows train Defence, how kept animals' condition is reckoned, how high a hop up a step
/// goes, and how often a birth mutates. Each is single-valued data a later package replaces field by field
/// ([`crate::content::Settable`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rules {
    pub limits: Limits,
    pub defence: DefenceTraining,
    pub condition: condition::ConditionRules,
    pub climbing: Climbing,
    pub genetics: Genetics,
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
    /// And this many small birds (`Flier`s), on a budget of their own, so birds never crowd
    /// out the animals on the ground.
    pub birds_near: usize,
    /// The most monsters near a player during a new world's first night (until its first
    /// dawn), so a first night in the open is a scare, not a siege.
    pub first_night_monsters: usize,
    /// Seconds between spawn attempts around each player, one spot an attempt.
    pub spawn_every: f32,
    /// How far from the player a spot may lie, in blocks: no nearer than `spawn_near` (so
    /// nothing appears at their feet) and no further than `spawn_far`. Nearer spots put
    /// more of what the caps allow within sight.
    pub spawn_near: f32,
    pub spawn_far: f32,
    /// Nothing spawns where a player could see it appear: within this many degrees of
    /// where they look (a half angle: 75 covers the screen) and in line of sight. 0 turns
    /// it off.
    pub spawn_unseen_angle: f32,
    /// How heavy "nothing here" weighs against the species that suit a spot (each weighs
    /// its biome, climate, rarity and the hour, about 1 for a common one at home): an
    /// attempt where only monsters suit it (a dark place) or where animals do. Higher
    /// leaves more spots empty.
    pub empty_for_monsters: f32,
    pub empty_for_animals: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { most: 1024, wild_most: 512, kept_each: 64, keepers: 8, animals_near: 12, monsters_near: 8, birds_near: 4, first_night_monsters: 3, spawn_every: 1.5, spawn_near: 20.0, spawn_far: 40.0, spawn_unseen_angle: 75.0, empty_for_monsters: 1.5, empty_for_animals: 2.5 }
    }
}

crate::content::settable! {
    /// The limits one package sets: any of them.
    SetLimits for Limits {
        most: usize, wild_most: usize, kept_each: usize, keepers: usize, animals_near: usize, monsters_near: usize, birds_near: usize, first_night_monsters: usize,
        spawn_every: f32, spawn_near: f32, spawn_far: f32, spawn_unseen_angle: f32, empty_for_monsters: f32, empty_for_animals: f32,
    }
}

impl Limits {
    /// Whether these limits can hold: room for one wild creature, for what one player draws
    /// near, and for every keeper's animals beside the wild ones.
    pub fn check(&self) -> Result<(), RegistryError> {
        let error = |why: String| Err(RegistryError(format!("creature limits: {why}")));
        if self.wild_most == 0 {
            return error("`wild_most` must be at least 1".into());
        }
        if !(self.spawn_every.is_finite() && self.spawn_every >= 0.05) {
            return error(format!("`spawn_every` ({}) must be at least 0.05 seconds", self.spawn_every));
        }
        if !(self.spawn_near.is_finite() && self.spawn_near >= 0.0 && self.spawn_far.is_finite() && self.spawn_far > self.spawn_near) {
            return error(format!("`spawn_far` ({}) must be more than `spawn_near` ({}), which is 0 or more", self.spawn_far, self.spawn_near));
        }
        if !(self.spawn_unseen_angle.is_finite() && (0.0..=180.0).contains(&self.spawn_unseen_angle)) {
            return error(format!("`spawn_unseen_angle` ({}) must be from 0 to 180 degrees", self.spawn_unseen_angle));
        }
        if !(self.empty_for_monsters.is_finite() && self.empty_for_monsters >= 0.0 && self.empty_for_animals.is_finite() && self.empty_for_animals >= 0.0) {
            return error("`empty_for_monsters` and `empty_for_animals` must be 0 or more".into());
        }
        if self.animals_near + self.monsters_near + self.birds_near > self.wild_most {
            return error(format!(
                "{} animals, {} monsters and {} birds near one player are more than `wild_most` ({})",
                self.animals_near, self.monsters_near, self.birds_near, self.wild_most
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

/// How high anything hops up a step (`motion::hop_speed`): a great leaper's own jump is
/// held to a rise of `step_rise` blocks, and anything that jumps at all gets up a block. A
/// rise of a block and a half keeps a snow leopard (its jump 11, over two blocks) springing
/// on from a block before a gap off a fence two high beyond it. A package's
/// `creatures.ron` may set it (`climbing`); a later package's replaces an earlier one's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Climbing {
    /// The highest a hop up a step rises, in blocks: above the 1.1 that gets up a block,
    /// and under 2, so no hop clears a fence two high.
    pub step_rise: f32,
}

impl Default for Climbing {
    fn default() -> Self {
        Self { step_rise: 1.45 }
    }
}

crate::content::settable! {
    /// What one package sets of climbing.
    SetClimbing for Climbing { step_rise: f32 }
}

/// How often a gene mutates at a birth (docs/breeding.md, "Mutation"): `mutation` per gene,
/// before aether, which raises it up to five times where it is strongest. A package's
/// `creatures.ron` may set it (`genetics`); a later package's replaces an earlier one's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Genetics {
    /// The chance per gene, from 0 (young are only ever their parents' genes) to 0.2, so
    /// five times it at the strongest aether is still a chance.
    pub mutation: f32,
}

impl Default for Genetics {
    fn default() -> Self {
        Self { mutation: crate::genes::MUTATION }
    }
}

crate::content::settable! {
    /// What one package sets of genetics.
    SetGenetics for Genetics { mutation: f32 }
}

impl Genetics {
    /// Whether the chance is one: from 0 to 0.2.
    pub fn check(&self) -> Result<(), RegistryError> {
        if !(self.mutation.is_finite() && (0.0..=0.2).contains(&self.mutation)) {
            return Err(RegistryError(format!("genetics: `mutation` ({}) must be from 0 to 0.2, a chance per gene that five times over is still one", self.mutation)));
        }
        Ok(())
    }

    /// The chance a gene mutates at a birth where the aether is `aether` (0 to 1).
    pub fn mutation_at(&self, aether: f32) -> f32 {
        self.mutation * (1.0 + 4.0 * aether.clamp(0.0, 1.0))
    }
}

impl Climbing {
    /// Whether a hop at this rise gets up a block and clears no fence two high.
    pub fn check(&self) -> Result<(), RegistryError> {
        if !(self.step_rise.is_finite() && (1.1..2.0).contains(&self.step_rise)) {
            return Err(RegistryError(format!("climbing: `step_rise` ({}) must be from 1.1 (a block and a little) to under 2 (a fence two high)", self.step_rise)));
        }
        Ok(())
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
    /// What it steals from the ground (`Scavenge::takes`), as items.
    pub scraps: Vec<crate::block::BlockId>,
}

impl Creature {
    /// Whether it fights back or hunts at all.
    pub fn fights(&self) -> bool {
        self.def.attack.is_some() && self.behaviours.iter().any(|b| matches!(b.behaviour, Behaviour::Defend | Behaviour::Hunt { .. }))
    }

    /// Whether one of its kind, `young` or grown, fights back at all: its young only if
    /// `young_fight`.
    pub fn fights_as(&self, young: bool) -> bool {
        self.fights() && (!young || self.def.young_fight)
    }

    /// Whether one that fights back, `young` or grown, at `health` (0 to 1) of its own and
    /// of `bravery`, holds its ground rather than runs: a boss always; else above
    /// `flees_below`, or unset, above 0.5 less 0.35 x its bravery.
    pub fn holds(&self, young: bool, health: f32, bravery: f32) -> bool {
        self.fights_as(young) && (self.def.boss || health > self.def.flees_below.unwrap_or(0.5 - 0.35 * bravery))
    }

    /// Whether it ever runs from a fight (a `Flee` behaviour).
    pub fn flees(&self) -> bool {
        self.behaviours.iter().any(|b| matches!(b.behaviour, Behaviour::Flee { .. }))
    }
}

/// Every species.
#[derive(Debug, Default, Clone)]
pub struct CreatureRegistry {
    creatures: Vec<Creature>,
    by_name: FxHashMap<String, CreatureId>,
    rules: Rules,
}

impl CreatureRegistry {
    /// A species' data to change in place, for tests only (`test-support`): what a package
    /// would set, past the load checks.
    #[cfg(any(test, feature = "test-support"))]
    pub fn get_mut(&mut self, id: CreatureId) -> &mut Creature {
        &mut self.creatures[id.0 as usize]
    }

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
        let (limits, defence, condition, climbing, genetics) = (file.limits, file.defence, file.condition, file.climbing, file.genetics);
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
        Ok(CreaturePackage { defs, limits, defence, condition, climbing, genetics })
    }

    /// Resolves species against the blocks (items) and skills.
    pub fn new(defs: Vec<CreatureDef>, rules: Rules, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        rules.limits.check()?;
        rules.defence.check()?;
        rules.condition.check()?;
        rules.climbing.check()?;
        rules.genetics.check()?;
        let mut registry = Self { rules, ..Self::default() };
        for mut def in defs {
            def.body.size = (def.body.size.0 * def.body.scale, def.body.size.1 * def.body.scale);
            def.movement.step_rise = rules.climbing.step_rise;
            let error = |message: String| RegistryError(format!("creature `{}`: {message}", def.name));
            if def.body.skin.eye.is_some_and(|r| !r.is_finite() || r <= 0.0) {
                return Err(error("skin.eye must be a finite positive radius".into()));
            }
            if def.health == 0 || def.body.size.0 <= 0.0 || def.body.size.1 <= 0.0 || def.body.colours.is_empty() {
                return Err(error("needs health, a size and at least one colour".into()));
            }
            moves::check(&def).map_err(error)?;
            for circle in def.attack.iter().chain(def.moves.iter().map(|m| &m.attack)).filter_map(|a| a.circle) {
                circle.check().map_err(error)?;
            }
            if let Some(warn) = def.warn {
                if def.attack.is_none() {
                    return Err(error("warns (`warn`) but has no `attack` to follow it with".into()));
                }
                if def.boss {
                    return Err(error("is a boss, and a boss warns no one (`warn`): it fights".into()));
                }
                let behaviours = if def.behaviours.is_empty() { generated(&def) } else { def.behaviours.clone() };
                warn.check(behaviours.iter().any(|b| matches!(b.behaviour, Behaviour::Hunt { .. }))).map_err(error)?;
            }
            if let Some(fire) = def.fire {
                if def.boss || def.supernatural {
                    return Err(error("is a boss or supernatural, and fire keeps neither back (`fire`)".into()));
                }
                fire.check().map_err(error)?;
            }
            if let Some(tires) = def.tires {
                tires.check().map_err(error)?;
            }
            if let Some(scavenge) = &def.scavenge {
                if def.boss || def.supernatural {
                    return Err(error("is a boss or supernatural, and steals no one's food (`scavenge`)".into()));
                }
                scavenge.check().map_err(error)?;
            }
            if let Some(flush) = def.flush {
                if def.boss || def.supernatural || def.attack.is_some() {
                    return Err(error("sits tight and bursts away (`flush`), which is for prey: not a boss, a supernatural kind or one that attacks".into()));
                }
                flush.check().map_err(error)?;
            }
            def.death.check().map_err(error)?;
            def.senses.check().map_err(error)?;
            def.barking.check().map_err(error)?;
            if def.flees_below.is_some_and(|f| !(0.0..=1.0).contains(&f)) {
                return Err(error(format!("`flees_below` is a share of its health, 0 to 1, not {:?}", def.flees_below)));
            }
            if !def.boss && !def.music.is_empty() {
                return Err(error("has fight `music` but is no boss".into()));
            }
            if def.genes.len() > crate::genes::MOST {
                return Err(error(format!("has more than {} genes", crate::genes::MOST)));
            }
            if def.supernatural && def.habitat.comfort.aether.0 <= 0.0 {
                return Err(error("a supernatural creature needs an aether minimum in its comfort".into()));
            }
            let flier = def.movement.style == Style::Flier;
            if flier != (def.movement.fly > 0.0) || !(def.movement.fly.is_finite() && def.movement.fly <= 32.0) {
                return Err(error(format!("a `Flier` needs `fly` from above 0 to 32 blocks, and only a flier has one (`fly: {}`)", def.movement.fly)));
            }
            if !(def.movement.step.is_finite() && (0.5..=1.0).contains(&def.movement.step)) {
                return Err(error(format!("`step` must be from 0.5 (a slab) to 1 block, not {}", def.movement.step)));
            }
            if flier && def.movement.run <= def.movement.walk * 1.5 {
                return Err(error("a flier takes wing above one and a half times its walk, so its `run` must be faster than that".into()));
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
            let scraps = match &def.scavenge {
                Some(scavenge) => foods(&scavenge.takes)?,
                None => Vec::new(),
            };
            if def.scavenge.is_some() && scraps.is_empty() {
                return Err(error("scavenges (`scavenge`), but nothing it `takes` is an item".into()));
            }
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
                .map(|p| {
                    if p.nests && p.tool.is_some() {
                        return Err(error(format!("`{}` nests, but is taken with a tool: only what it lays by itself goes to the stores", p.item)));
                    }
                    blocks.id(&p.item).ok_or_else(|| error(format!("gives `{}`, which does not exist", p.item)))
                })
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
                // Anything may be sent on an errand (a wandering party's), so every species
                // can follow one; it wants nothing until it has one.
                .chain((!behaviours.iter().any(|entry| matches!(entry.behaviour, Behaviour::Routine { .. }))).then(|| (brain::modules::module(Behaviour::Routine { arrive: 0.8, look: 4.0 }), 1.0)))
                .chain(brain::modules::always().into_iter().map(|module| (module, 1.0)))
                .collect();
            let id = CreatureId(registry.creatures.len() as u16);
            if registry.by_name.insert(def.name.clone(), id).is_some() {
                return Err(error("defined twice".into()));
            }
            registry.creatures.push(Creature { def, behaviours, modules, parts, reach, head, layout, loot, eats, favourite, xp_skill, tame_food, breed_food, products, scraps });
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

    /// How often a birth mutates.
    pub fn genetics(&self) -> &Genetics {
        &self.rules.genetics
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

    /// Barking in range passes; a range or wait out of it on either side is refused.
    #[test]
    fn barking_out_of_range_is_refused() {
        let good = Barking::default();
        assert!(good.check().is_ok());
        let broken = [
            Barking { range: 0.5, ..good },
            Barking { range: MOST_BARK_RANGE + 1.0, ..good },
            Barking { every: -1.0, ..good },
            Barking { every: MOST_BARK_EVERY + 1.0, ..good },
            Barking { every: f32::NAN, ..good },
        ];
        for b in broken {
            assert!(b.check().is_err_and(|e| e.contains("barking")), "{b:?} was let through");
        }
    }

    /// A circle in range passes; each number out of its range on either side is refused,
    /// naming `circle`.
    #[test]
    fn a_circle_out_of_range_is_refused() {
        let good = Circle { seconds: 0.8, away: 0.6, pace: 0.55, round: 50.0 };
        assert!(good.check().is_ok());
        let broken = [
            Circle { seconds: 0.2, ..good },
            Circle { seconds: 2.1, ..good },
            Circle { away: 0.2, ..good },
            Circle { away: 2.6, ..good },
            Circle { pace: 0.1, ..good },
            Circle { pace: 1.1, ..good },
            Circle { round: 14.0, ..good },
            Circle { round: 121.0, ..good },
            Circle { seconds: f32::NAN, ..good },
        ];
        for circle in broken {
            assert!(circle.check().is_err_and(|e| e.contains("`circle`")), "{circle:?} passed");
        }
    }

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


    /// A death tips over, lies a beat and goes, its loot out by then; one that would vanish
    /// as it lands, or drop its loot after it is gone, is refused.
    #[test]
    fn a_death_lies_a_beat_and_lets_its_loot_out_as_it_goes() {
        let death = Death::default();
        assert_eq!(death.check(), Ok(()));
        assert_eq!(death.drops_after(), death.seconds, "the loot comes out with the puff");
        for refused in [
            Death { fall: death.seconds - LIES + 0.05, ..death },
            Death { fall: 0.0, ..death },
            Death { seconds: 12.0, ..death },
            Death { sink: 3.0, ..death },
            Death { drops: Some(death.seconds + 0.1), ..death },
        ] {
            assert!(refused.check().is_err(), "{refused:?} passed");
        }
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

    /// Only what an animal lays by itself may go to the home's stores: a product taken with
    /// a tool that `nests` is refused as it loads.
    #[test]
    fn only_what_is_laid_nests() {
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "base:short_grass".into(), ..Default::default() },
            BlockDef { name: "base:carrot".into(), ..Default::default() },
        ])
        .unwrap();
        let load = |product: &str| {
            let defs = CreatureRegistry::read("base", &RABBIT.replace("loot:", &format!("products: [{product}], loot:"))).unwrap().defs;
            CreatureRegistry::new(defs, Rules::default(), &blocks, &SkillRegistry::default())
        };
        let laid = load(r#"(item: "carrot", every: 5.0, nests: true)"#).unwrap();
        assert!(laid.get(CreatureId(0)).def.products[0].nests);
        assert!(!load(r#"(item: "carrot", every: 5.0)"#).unwrap().get(CreatureId(0)).def.products[0].nests, "nests is off unless asked");
        let error = load(r#"(item: "carrot", every: 5.0, tool: "shears", nests: true)"#).unwrap_err();
        assert!(error.0.contains("nests"), "{error:?}");
    }

    #[test]
    fn eye_radius_loads_from_data_and_rejects_invalid_values() {
        let mut defs = CreatureRegistry::read("base", &RABBIT.replace("colours:", "skin: (eyes: Front, eye: 0.065), colours:")).unwrap().defs;
        assert_eq!(defs[0].body.skin.eye, Some(0.065));
        // Leave unrelated item references out, so each rejection exercises skin.eye.
        defs[0].diet = Default::default();
        defs[0].loot.clear();
        let blocks = BlockRegistry::new(vec![]).unwrap();
        assert!(CreatureRegistry::new(defs.clone(), Rules::default(), &blocks, &SkillRegistry::default()).is_ok());
        for radius in [0.0, -0.065, f32::NAN, f32::INFINITY] {
            defs[0].body.skin.eye = Some(radius);
            let error = CreatureRegistry::new(defs.clone(), Rules::default(), &blocks, &SkillRegistry::default()).unwrap_err();
            assert!(error.0.contains("skin.eye"), "{error:?}");
        }
    }

    #[test]
    fn a_step_is_from_a_slab_to_a_block() {
        let mut defs = CreatureRegistry::read("base", RABBIT).unwrap().defs;
        defs[0].diet = Default::default();
        defs[0].loot.clear();
        let blocks = BlockRegistry::new(vec![]).unwrap();
        for step in [0.5, 0.5625, 1.0] {
            defs[0].movement.step = step;
            assert!(CreatureRegistry::new(defs.clone(), Rules::default(), &blocks, &SkillRegistry::default()).is_ok(), "{step} refused");
        }
        for step in [0.0, 0.49, 1.01, f32::NAN, f32::INFINITY] {
            defs[0].movement.step = step;
            let error = CreatureRegistry::new(defs.clone(), Rules::default(), &blocks, &SkillRegistry::default()).unwrap_err();
            assert!(error.0.contains("`step`"), "{step}: {error:?}");
        }
    }

    #[test]
    fn limits_load_from_the_file_and_are_checked() {
        let source = RABBIT.replacen("(creatures:", "(limits: (wild_most: 20, kept_each: 5, spawn_unseen_angle: 45.0), creatures:", 1);
        let set = CreatureRegistry::read("base", &source).unwrap().limits.expect("set");
        assert_eq!(set, SetLimits { wild_most: Some(20), kept_each: Some(5), spawn_unseen_angle: Some(45.0), ..Default::default() });
        assert_eq!(CreatureRegistry::read("base", RABBIT).unwrap().limits, None, "a file without limits leaves them to another");
        // What a package sets replaces what was there; the rest stays.
        let (limits, changed) = Limits::default().with(&set);
        assert_eq!(changed, ["wild_most", "kept_each", "spawn_unseen_angle"]);
        assert_eq!(limits.spawn_unseen_angle, 45.0);
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
            Limits { spawn_every: 0.0, ..ok },
            Limits { spawn_far: ok.spawn_near, ..ok },
            Limits { spawn_near: -1.0, ..ok },
            Limits { spawn_unseen_angle: 200.0, ..ok },
            Limits { spawn_unseen_angle: -1.0, ..ok },
            Limits { spawn_unseen_angle: f32::NAN, ..ok },
            Limits { spawn_unseen_angle: f32::INFINITY, ..ok },
            Limits { spawn_every: f32::NAN, ..ok },
            Limits { empty_for_monsters: f32::INFINITY, ..ok },
            Limits { empty_for_animals: -1.0, ..ok },
        ] {
            let blocks = BlockRegistry::new(vec![]).unwrap();
            assert!(CreatureRegistry::new(Vec::new(), Rules { limits: bad, ..Default::default() }, &blocks, &SkillRegistry::default()).is_err(), "{bad:?} loaded");
        }
    }

    /// A package sets how often a birth mutates; the shipped default is `genes::MUTATION`,
    /// aether raises it up to five times, and a chance past 0.2 (or none at all) is refused.
    #[test]
    fn genetics_loads_from_the_file_and_is_checked() {
        let source = RABBIT.replacen("(creatures:", "(genetics: (mutation: 0.0), creatures:", 1);
        let set = CreatureRegistry::read("base", &source).unwrap().genetics.expect("set");
        assert_eq!(CreatureRegistry::read("base", RABBIT).unwrap().genetics, None);
        let (rules, changed) = Genetics::default().with(&set);
        assert_eq!((rules.mutation, changed), (0.0, vec!["mutation"]));
        assert_eq!(Genetics::default().mutation, crate::genes::MUTATION);
        assert_eq!((Genetics::default().mutation_at(0.0), Genetics::default().mutation_at(1.0)), (crate::genes::MUTATION, 5.0 * crate::genes::MUTATION));
        let blocks = BlockRegistry::new(vec![]).unwrap();
        for mutation in [-0.1, 0.21, f32::NAN] {
            assert!(CreatureRegistry::new(Vec::new(), Rules { genetics: Genetics { mutation }, ..Default::default() }, &blocks, &SkillRegistry::default()).is_err(), "{mutation} loaded");
        }
        assert!(CreatureRegistry::new(Vec::new(), Rules { genetics: Genetics { mutation: 0.0 }, ..Default::default() }, &blocks, &SkillRegistry::default()).is_ok());
    }

    #[test]
    fn climbing_loads_from_the_file_and_is_checked() {
        let source = RABBIT.replacen("(creatures:", "(climbing: (step_rise: 1.3), creatures:", 1);
        let set = CreatureRegistry::read("base", &source).unwrap().climbing.expect("set");
        assert_eq!(CreatureRegistry::read("base", RABBIT).unwrap().climbing, None);
        let (rules, changed) = Climbing::default().with(&set);
        assert_eq!((rules.step_rise, changed), (1.3, vec!["step_rise"]));
        assert_eq!(Movement::default().step_rise, Climbing::default().step_rise);
        // Under a block and a little, nothing gets up a block; two or more, a hop clears a
        // fence two high.
        let blocks = BlockRegistry::new(vec![]).unwrap();
        for step_rise in [1.0, 2.0, 2.5, f32::NAN] {
            assert!(CreatureRegistry::new(Vec::new(), Rules { climbing: Climbing { step_rise }, ..Default::default() }, &blocks, &SkillRegistry::default()).is_err(), "{step_rise} loaded");
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
