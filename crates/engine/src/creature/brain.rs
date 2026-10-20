//! A creature's mind: drives, memories that expire, a seeded personality, and a chooser
//! that picks among its behaviour modules. Pure functions over a snapshot of what the
//! creature senses, so the server, tests and later WASM modules run the same decisions
//! (docs/creature-ai.md).
//!
//! The chooser is dual utility (Zoo Tycoon 2, Dave Mark's IAUS; research/creature-ai.html):
//! every module the species has says how much it wants to run, in a rank band (reflex,
//! danger, drive, social, idle). Only the highest band with any wish competes, so a rabbit
//! never finishes its meal with a fox at its tail. Within the band, the running module
//! keeps going while it stays within a tenth of the best (commitment, so creatures don't
//! dither); otherwise one is drawn at random among those within a tenth of the best,
//! weighted by score. Near-ties are where unpredictability comes from: two wolves in the
//! same spot don't always do the same thing.
//!
//! Shared state lives in drives (hunger, fear, anger) and in memories with a time to live,
//! as Minecraft's Brain keeps them: who hurt it, who scared it, who fed it. Cooldowns are
//! memories that run out. Personality is six seeded axes (Rain World's), pushed away from
//! the middle so individuals read clearly, and every axis changes something a player can
//! see: flight distance, curiosity, group spacing, pace, how long it pauses.
//!
//! Randomness comes from a seed the server derives from the world, the creature and the
//! tick, so a replay repeats every choice.

pub mod modules;

use glam::{IVec3, Vec3};
use serde::{Deserialize, Serialize};

use super::{Creature, Temperament, mix, unit_from};

/// Hunger a creature gains per second: from fed to hungry in about four minutes.
const HUNGER_RATE: f32 = 1.0 / 240.0;
/// Fear and anger fade per second.
const FEAR_FADE: f32 = 0.12;
/// How long a creature that gave up a chase leaves players be (`Mind::give_up`).
pub const GAVE_UP: f32 = 45.0;
/// How long a creature that found no way to its attacker runs from their blows rather
/// than fight back (`Memory::NoWay`).
pub const NO_WAY: f32 = 30.0;
/// How long a blow counts as just landed (`Memory::Struck`): a hunter past its leash
/// struck within it fights back rather than go home.
pub const STRUCK_LATELY: f32 = 3.0;
/// How long a creature that gave up a chase makes for home (`Memory::Homeward`).
pub const HOMEWARD: f32 = 20.0;
/// How far past a player's reach (`physics::player::REACH`, from their eyes) a boss held
/// off keeps its body: a player down a hole it cannot reach into strikes nothing from there.
pub const KEEP_OFF: f32 = 0.5;
/// Seconds a creature remembers the player who hurt it (`Memory::HurtBy`).
pub const HURT_REMEMBERED: f32 = 45.0;
const ANGER_FADE: f32 = 1.0 / 45.0;
/// Within a band, modules within this share of the best score are all in the draw, and
/// the running one keeps going while it is.
const NEAR_BEST: f32 = 0.9;
/// The least score that counts as wanting to run.
const LEAST: f32 = 0.02;
/// Most memories a creature keeps; the oldest goes first.
const MEMORIES: usize = 8;

/// The rank band a module competes in; higher bands win outright.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    Idle,
    Social,
    Drive,
    Danger,
    Reflex,
}

/// What a creature looks like it is doing, for the client's animation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pose {
    #[default]
    Idle,
    Grazing,
    /// Ears up, looking at something.
    Alert,
    Resting,
    Attacking,
    /// Crouched low: stalking, or about to pounce.
    Crouched,
}

/// A player as a creature sees them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub id: u32,
    /// Feet.
    pub position: Vec3,
    /// Holding something the creature finds irresistible.
    pub tempting: bool,
    pub combat_level: u8,
    /// Creative players are ignored by hunters, as in Minecraft.
    pub creative: bool,
    /// Has stayed in one small area long enough that hunters there lose interest
    /// (RuneScape's tolerance).
    pub tolerated: bool,
    /// Its owner, whom a kept creature neither fears nor hunts.
    pub friend: bool,
    /// Whether a blow from where the creature stands has a way through the air to them
    /// (the server's `blow_open`): none through a roof or a wall, so it is not close enough
    /// to strike however near.
    pub open: bool,
}

/// Where someone with a routine (villagers) is to be now, as the server's schedule and
/// path finding say: the next point on their way, and how to be once there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Errand {
    pub to: Vec3,
    /// Whether `to` is where they are going, not a turn on the way.
    pub last: bool,
    /// How they are once there: resting (asleep), or idle.
    pub pose: Pose,
    /// What they face once there.
    pub face: Option<Vec3>,
    /// For a way after someone (a guard's pursuit) rather than the day's: within how many
    /// blocks of them it goes straight at them instead.
    pub chase: Option<f32>,
    /// How fast it goes, as a share of its walk: 1 for the day's errands; a party's
    /// stragglers hurry to catch up with theirs.
    pub pace: f32,
    /// Its flight is the errand's (a wandering party's script, which runs a quarry and
    /// scatters a struck party itself): it does not `Flee` while it holds this one.
    pub steady: bool,
}

/// What a creature senses this decision.
#[derive(Clone, Copy, Debug)]
pub struct Situation<'a> {
    pub position: Vec3,
    pub home: Vec3,
    /// Its health as a share of its most.
    pub health: f32,
    /// The phase of its fight it has reached (`moves::Phase`): a boss mended back over a
    /// phase's line stays in it.
    pub phase: u8,
    /// Its individual size, as a share of its species' (what it reaches from grows with it).
    pub scale: f32,
    /// 0 at night .. 1 by day.
    pub daylight: f32,
    /// Players it can sense, nearest first.
    pub players: &'a [Seen],
    /// Where others of its kind are, nearby.
    pub kin: &'a [Vec3],
    /// The nearest block of its food, if any is near.
    pub food: Option<IVec3>,
    /// Another of its kind ready to breed, when it is ready too.
    pub mate: Option<Vec3>,
    /// Whether someone keeps it: a kept creature stays near its home.
    pub kept: bool,
    /// Where its routine sends it now, for those that have one.
    pub errand: Option<Errand>,
    /// Seconds since the last decision.
    pub dt: f32,
    /// The seed for this decision's random choices.
    pub seed: u64,
}

/// An individual's personality: six axes, each 0..=1 (Rain World's six).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    pub aggression: f32,
    pub bravery: f32,
    pub sociability: f32,
    pub curiosity: f32,
    /// Pace and restlessness: fast, short pauses at 1; slow and lazy at 0.
    pub energy: f32,
    /// Startles easily, freezes and looks about.
    pub nervousness: f32,
}

impl Personality {
    /// Rolls an individual from its species' temperament: each axis spread about the
    /// species value by up to `spread` each way, pushed away from the middle so most are
    /// near their kind and a few stand out.
    pub fn roll(species: &Temperament, seed: u64) -> Self {
        let axis = |salt: u64, value: f32| {
            let u = unit_from(mix(seed, salt)) * 2.0 - 1.0;
            let pushed = u.signum() * u.abs().powf(0.7);
            (value + pushed * species.spread).clamp(0.0, 1.0)
        };
        Self {
            aggression: axis(1, species.aggression),
            bravery: axis(2, 1.0 - species.fear),
            sociability: axis(3, species.sociability),
            curiosity: axis(4, species.curiosity),
            energy: axis(5, species.energy),
            nervousness: axis(6, species.nervousness),
        }
    }

    /// How fear-driven it is, 0..=1.
    pub fn fear(&self) -> f32 {
        1.0 - self.bravery
    }

    /// Its speed as a share of its species': 0.8 for the laziest, 1.2 for the liveliest.
    pub fn pace(&self) -> f32 {
        0.8 + 0.4 * self.energy
    }

    /// Words for the inspect action: "bold, curious, lazy".
    pub fn words(&self) -> Vec<&'static str> {
        let mut words = Vec::new();
        let mut word = |value: f32, high: &'static str, low: &'static str| {
            if value > 0.7 {
                words.push(high);
            } else if value < 0.3 {
                words.push(low);
            }
        };
        word(self.bravery, "bold", "timid");
        word(self.aggression, "fierce", "gentle");
        word(self.curiosity, "curious", "aloof");
        word(self.sociability, "sociable", "solitary");
        word(self.energy, "lively", "lazy");
        word(self.nervousness, "skittish", "calm");
        words
    }
}

/// Something a creature remembers for a while.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Memory {
    /// This player hurt it.
    HurtBy(u32),
    /// This player frightened it (came too close, sprinted at it).
    ScaredBy(u32),
    /// This player fed or tempted it.
    FedBy(u32),
    /// Kin nearby raised the alarm.
    Alarm,
    /// It gave up a chase it could not win (out past its leash, or no way to its target):
    /// it neither hunts nor fights back for a while, unless struck again.
    GaveUp,
    /// Having given up a chase, it makes for home.
    Homeward,
    /// It gave up a chase for want of any way to its target (a player behind a fence or
    /// up a ledge): struck again while it remembers, it runs rather than fight back.
    NoWay,
    /// A player's blow (not a bleed) landed on it within `STRUCK_LATELY` seconds.
    Struck,
    /// Something at `Mind::startled_at` startled it with no one near to blame (a snare it
    /// slipped): it runs from that spot. Kin's alarms raised after leave this be.
    Startled,
}

/// A creature's state of mind, kept between decisions.
#[derive(Clone, Debug)]
pub struct Mind {
    pub personality: Personality,
    pub hunger: f32,
    pub fear: f32,
    pub anger: f32,
    /// Memories and the seconds each has left.
    pub memories: Vec<(Memory, f32)>,
    /// The running module, as an index into the species' modules.
    pub doing: Option<usize>,
    /// Seconds in the current module.
    pub since: f32,
    /// Where the current module is heading.
    pub goal: Option<Vec3>,
    /// A pause, in seconds, before moving on.
    pub pause: f32,
    /// Seconds until it may attack again.
    pub cooldown: f32,
    /// Seconds left of circling its target between blows (`Attack::circle`), counted from
    /// the end of its recovery (held till then, it does not think).
    pub circling: f32,
    /// Zigzag side when fleeing, and other modules' small counters.
    pub zig: f32,
    /// Where it was startled with no one to blame (a snare it slipped): while it recalls
    /// the fright (`Memory::Startled`) it runs from there, not toward home.
    pub startled_at: Option<Vec3>,
    /// Where a boss's target stood when it came as near them as it could and still could
    /// not reach them (a player down a hole too deep for it, or under a roof): while they
    /// stay there it keeps off out of their reach, watching them (`KEEP_OFF`), and comes
    /// on once they leave it.
    pub held_off: Option<Vec3>,
    /// The individual's own hash, for choices that stay its own (which of its kin it keeps
    /// near, how far it likes to stray).
    pub seed: u64,
}

impl Mind {
    /// A new mind for an individual whose identity hashes to `seed`.
    pub fn new(species: &Temperament, seed: u64) -> Self {
        Self {
            personality: Personality::roll(species, seed),
            hunger: unit_from(mix(seed, 7)) * 0.6,
            fear: 0.0,
            anger: 0.0,
            memories: Vec::new(),
            doing: None,
            since: 0.0,
            goal: None,
            pause: 0.0,
            cooldown: 0.0,
            circling: 0.0,
            seed,
            zig: 1.0,
            startled_at: None,
            held_off: None,
        }
    }

    /// Remembers `memory` for `seconds`, refreshing it if already remembered.
    pub fn remember(&mut self, memory: Memory, seconds: f32) {
        if let Some(entry) = self.memories.iter_mut().find(|(m, _)| *m == memory) {
            entry.1 = entry.1.max(seconds);
            return;
        }
        if self.memories.len() >= MEMORIES {
            self.memories.remove(0);
        }
        self.memories.push((memory, seconds));
    }

    pub fn recalls(&self, memory: Memory) -> bool {
        self.memories.iter().any(|(m, _)| *m == memory)
    }

    /// The player it most recently remembers being hurt by.
    pub fn attacker(&self) -> Option<u32> {
        self.memories.iter().rev().find_map(|(m, _)| match m {
            Memory::HurtBy(id) => Some(*id),
            _ => None,
        })
    }

    /// It was hurt, by `by` if a player: it fears, and if it can fight and is not badly
    /// hurt, angers. Remembered for a while (Minecraft keeps revenge 15 s; a grudge here
    /// lasts longer, and bravery decides fight or flight).
    pub fn hurt(&mut self, creature: &Creature, by: Option<u32>, health: f32) {
        self.hurt_young(creature, by, health, false);
    }

    /// As [`Mind::hurt`], for one still `young`: the young of a kind that fights back (a
    /// piglet) run all the same, unless its kind's `young_fight`.
    pub fn hurt_young(&mut self, creature: &Creature, by: Option<u32>, health: f32, young: bool) {
        // A boss never runs: it fights to the end (`Creature::holds`).
        // Nor does one that has just found no way to its attacker: it takes itself off.
        let fights = creature.holds(young, health, self.personality.bravery) && (creature.def.boss || !self.recalls(Memory::NoWay));
        if fights {
            self.anger = 1.0;
        } else {
            self.fear = 1.0;
            // One that runs from a fight drops it, or the anger of the blows before would
            // hold it at bay until it died; one that never runs fights on, cornered.
            if creature.flees() {
                self.anger = 0.0;
            }
        }
        if let Some(id) = by {
            self.remember(Memory::HurtBy(id), HURT_REMEMBERED);
        }
        // Struck again, it takes up the fight it gave up: unless it gave up for want of any
        // way to them, when it runs and, its fright gone, still leaves them be.
        if creature.def.boss || !self.recalls(Memory::NoWay) {
            self.memories.retain(|(m, _)| !matches!(m, Memory::GaveUp | Memory::Homeward));
        }
    }

    /// Gives up the chase: its anger and grudges go, for `GAVE_UP` seconds it hunts no one
    /// (`Memory::GaveUp`), and it goes home (`Memory::Homeward`) rather than hang at a
    /// fence or follow a player across the land.
    pub fn give_up(&mut self) {
        self.anger = 0.0;
        self.memories.retain(|(m, _)| !matches!(m, Memory::HurtBy(_)));
        self.remember(Memory::GaveUp, GAVE_UP);
        self.remember(Memory::Homeward, HOMEWARD);
    }

    /// Gives up a chase it found no way to win (`give_up`), and remembers it for `NO_WAY`
    /// seconds (`Memory::NoWay`).
    pub fn find_no_way(&mut self) {
        self.give_up();
        self.remember(Memory::NoWay, NO_WAY);
    }

    /// Lets go of every grudge and fright: it was hurt by no one.
    pub fn forgive(&mut self) {
        self.anger = 0.0;
        self.fear = 0.0;
        self.memories.retain(|(m, _)| !matches!(m, Memory::HurtBy(_) | Memory::ScaredBy(_)));
    }

    /// Kin nearby raised the alarm.
    pub fn alarmed(&mut self) {
        self.fear = self.fear.max(0.5 + 0.5 * self.personality.nervousness);
        self.remember(Memory::Alarm, 8.0);
    }

    /// Something at `at` startled it with no one near to blame: alarmed (`alarmed`), it
    /// runs from that spot for as long as it is startled (`Memory::Startled`).
    pub fn startled(&mut self, at: Vec3) {
        self.alarmed();
        self.startled_at = Some(at);
        self.remember(Memory::Startled, 8.0);
    }
}

/// What the body should do until the next decision.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Decision {
    /// Where to go; none to stand.
    pub goal: Option<Vec3>,
    /// Blocks per second.
    pub speed: f32,
    /// A point to face when standing.
    pub face: Option<Vec3>,
    /// A player in reach to strike.
    pub attack: Option<u32>,
    /// A block of food it is eating.
    pub eat: Option<IVec3>,
    /// A spot to blink to (supernatural flight).
    pub blink: Option<Vec3>,
    /// A leap toward a point (pounce).
    pub leap: Option<Vec3>,
    /// It raises the alarm for kin nearby.
    pub alarm: bool,
    pub pose: Pose,
}

/// What a module reads when it decides whether it wants to run.
pub struct Ctx<'a> {
    pub creature: &'a Creature,
    pub mind: &'a Mind,
    pub at: &'a Situation<'a>,
    /// How lively the species is now, from its activity window and the daylight.
    pub lively: f32,
    /// How far it sees now.
    pub sight: f32,
}

impl Ctx<'_> {
    pub fn distance(&self, to: Vec3) -> f32 {
        to.distance(self.at.position)
    }

    /// The nearest player it senses who is not its owner.
    pub fn nearest(&self) -> Option<&Seen> {
        self.at.players.iter().find(|p| !p.friend)
    }

    /// A player it remembers being hurt by, if they are about.
    pub fn attacker(&self) -> Option<&Seen> {
        let id = self.mind.attacker()?;
        self.at.players.iter().find(|p| p.id == id)
    }

    /// A number in 0..1 for this decision and `salt`.
    pub fn random(&self, salt: u64) -> f32 {
        unit_from(mix(self.at.seed, salt))
    }
}

/// How much a module wants to run, and at whom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Want {
    pub score: f32,
    pub target: Option<Seen>,
}

impl Want {
    pub fn new(score: f32) -> Option<Self> {
        (score > LEAST).then_some(Self { score, target: None })
    }

    pub fn at(score: f32, target: Seen) -> Option<Self> {
        (score > LEAST).then_some(Self { score, target: Some(target) })
    }
}

/// A behaviour module: a small parameterised behaviour with its own considerations
/// (research/creature-ai.html, "Base module library"). The engine ships a library
/// ([`modules`]); a species lists which it has and tunes them in data.
pub trait Module: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn band(&self) -> Band;
    /// How much it wants to run now; none when it cannot.
    fn want(&self, ctx: &Ctx) -> Option<Want>;
    /// What it asks of the body, having been chosen. `fresh` is true the first time.
    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, fresh: bool) -> Decision;
}

/// Decides what a creature does now.
pub fn think(creature: &Creature, mind: &mut Mind, at: &Situation) -> Decision {
    let def = &creature.def;
    let dt = at.dt;
    mind.hunger = (mind.hunger + HUNGER_RATE * dt).min(1.0);
    mind.fear = (mind.fear - FEAR_FADE * dt * (1.5 - mind.personality.nervousness)).max(0.0);
    mind.anger = (mind.anger - ANGER_FADE * dt).max(0.0);
    mind.cooldown = (mind.cooldown - dt).max(0.0);
    mind.circling = (mind.circling - dt).max(0.0);
    mind.pause = (mind.pause - dt).max(0.0);
    mind.since += dt;
    mind.memories.retain_mut(|(_, left)| {
        *left -= dt;
        *left > 0.0
    });

    let lively = def.habitat.active.liveliness(at.daylight);
    let night_eyes = def.habitat.active == super::Active::Night;
    let sight = def.senses.sight * if night_eyes { 1.0 } else { 0.6 + 0.4 * at.daylight };
    let ctx = Ctx { creature, mind, at, lively, sight };

    let wants: Vec<(usize, Band, Want)> = creature
        .modules
        .iter()
        .enumerate()
        .filter_map(|(i, (module, weight))| module.want(&ctx).map(|w| (i, module.band(), Want { score: w.score * weight, ..w })))
        .collect();
    let Some(top) = wants.iter().map(|w| w.1).max() else {
        let pose = modules::resting_pose(&ctx);
        mind.doing = None;
        return Decision { pose, ..Default::default() };
    };
    let band: Vec<&(usize, Band, Want)> = wants.iter().filter(|w| w.1 == top).collect();
    let best = band.iter().map(|w| w.2.score).fold(0.0, f32::max);
    let near: Vec<&&(usize, Band, Want)> = band.iter().filter(|w| w.2.score >= best * NEAR_BEST).collect();
    let chosen: (usize, Band, Want) = match near.iter().find(|w| Some(w.0) == mind.doing) {
        Some(w) => ***w,
        None => {
            let total: f32 = near.iter().map(|w| w.2.score).sum();
            let mut pick = unit_from(mix(at.seed, 0xc4053)) * total;
            let drawn = near.iter().find(|w| {
                pick -= w.2.score;
                pick <= 0.0
            });
            ***drawn.unwrap_or(&near[0])
        }
    };
    let (index, _, want) = chosen;
    let fresh = mind.doing != Some(index);
    if fresh {
        mind.doing = Some(index);
        mind.since = 0.0;
        mind.goal = None;
    }
    let ctx = Ctx { creature, mind: &mind.clone(), at, lively, sight };
    let mut decision = creature.modules[index].0.act(&ctx, mind, want.target, fresh);
    decision.speed *= mind.personality.pace();
    decision
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::block::BlockRegistry;
    use crate::creature::{CreatureId, CreatureRegistry};
    use crate::skill::SkillRegistry;
    use crate::creature::motion;

    pub(super) fn species(source: &str) -> Creature {
        let defs = CreatureRegistry::read("test", source).unwrap().defs;
        CreatureRegistry::new(defs, Default::default(), &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).unwrap().get(CreatureId(0)).clone()
    }

    pub(super) const BODY: &str = "body: (size: (0.6, 0.6), model: Blob(body: (0.5, 0.5, 0.5)), colours: [(1, 2, 3)])";

    pub(super) fn situation<'a>(players: &'a [Seen]) -> Situation<'a> {
        Situation { position: Vec3::ZERO, home: Vec3::ZERO, health: 1.0, phase: 0, scale: 1.0, daylight: 1.0, players, kin: &[], food: None, mate: None, kept: false, errand: None, dt: 0.2, seed: 7 }
    }

    pub(super) fn player(at: Vec3) -> Seen {
        Seen { id: 1, position: at, tempting: false, combat_level: 3, creative: false, tolerated: false, friend: false, open: true }
    }

    fn doing<'a>(creature: &'a Creature, mind: &Mind) -> &'a str {
        mind.doing.map_or("nothing", |i| creature.modules[i].0.name())
    }

    #[test]
    fn timid_creatures_flee_and_tempted_ones_follow() {
        let rabbit = species(&format!(
            "(creatures: [(name: \"rabbit\", health: 3, {BODY}, temperament: (fear: 0.9), behaviours: [(do: Wander(radius: 6.0)), (do: Flee(radius: 10.0)), (do: Tempted(radius: 10.0))])])"
        ));
        let mut mind = Mind::new(&rabbit.def.temperament, 1);
        let near = [player(Vec3::new(3.0, 0.0, 0.0))];
        let decision = think(&rabbit, &mut mind, &situation(&near));
        assert_eq!(doing(&rabbit, &mind), "flee");
        assert!(decision.goal.unwrap().x < -1.0, "runs away from the player: {:?}", decision.goal);
        let mut mind = Mind::new(&rabbit.def.temperament, 1);
        let carrot = [Seen { tempting: true, ..player(Vec3::new(5.0, 0.0, 0.0)) }];
        let decision = think(&rabbit, &mut mind, &situation(&carrot));
        assert_eq!(doing(&rabbit, &mind), "tempted");
        assert!(decision.goal.unwrap().x > 4.0);
    }

    #[test]
    fn birds_take_wing_from_players_and_hop_or_fly_about() {
        let finch = species(&format!(
            "(creatures: [(name: \"finch\", health: 1, leash: 30.0, {BODY}, movement: (style: Flier, walk: 1.0, run: 7.0, jump: 3.0, fly: 5.0), temperament: (fear: 0.9), behaviours: [(do: Wander(radius: 6.0)), (do: Flee(radius: 8.0))])])"
        ));
        let flight = |speed: f32| motion::flies(&finch.def.movement, motion::Drive { wish: Vec3::X, speed, face: None });
        // Flushed by a player, it flies well clear: fast enough to be on the wing.
        let mut mind = Mind::new(&finch.def.temperament, 1);
        let near = [player(Vec3::new(3.0, 0.0, 0.0))];
        let decision = think(&finch, &mut mind, &situation(&near));
        assert_eq!(doing(&finch, &mind), "flee");
        assert!(decision.goal.unwrap().x < -15.0 && flight(decision.speed), "{decision:?}");
        // Left alone, it mostly hops a little way and now and then flies off across its range.
        let (mut hops, mut flights) = (0, 0);
        for seed in 0..200 {
            let mut mind = Mind::new(&finch.def.temperament, seed);
            let decision = think(&finch, &mut mind, &Situation { seed, ..situation(&[]) });
            let Some(goal) = decision.goal else { continue };
            if flight(decision.speed) {
                assert!(goal.length() > 6.0, "flew to a near goal {goal}");
                flights += 1;
            } else {
                assert!(goal.length() <= 3.0 + 1e-3, "walked to a far goal {goal}");
                hops += 1;
            }
        }
        assert!(hops > flights && flights > 10, "{hops} hops, {flights} flights");
    }

    #[test]
    fn hunters_attack_in_reach_but_leave_strong_and_settled_players_alone_unless_a_boss() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let near = [player(Vec3::new(1.0, 0.0, 0.0))];
        let decision = think(&wolf, &mut mind, &situation(&near));
        assert_eq!((doing(&wolf, &mind), decision.attack), ("hunt", Some(1)));
        // More than twice its level and one, or settled long enough, and it leaves them be.
        for seen in [Seen { combat_level: 12, ..near[0] }, Seen { tolerated: true, ..near[0] }] {
            let mut mind = Mind::new(&wolf.def.temperament, 2);
            let _ = think(&wolf, &mut mind, &situation(&[seen]));
            assert_eq!(doing(&wolf, &mind), "nothing");
            // Unless it is angry with them.
            mind.hurt(&wolf, Some(1), 1.0);
            let _ = think(&wolf, &mut mind, &situation(&[seen]));
            assert_eq!(doing(&wolf, &mind), "hunt");
        }
        // A boss always fights: neither rule leaves anyone be.
        let boss = species(&format!(
            "(creatures: [(name: \"warden\", boss: true, health: 10, level: 5, {BODY}, attack: (damage: (1, 3)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        for seen in [Seen { combat_level: 12, ..near[0] }, Seen { tolerated: true, ..near[0] }] {
            let mut mind = Mind::new(&boss.def.temperament, 2);
            let _ = think(&boss, &mut mind, &situation(&[seen]));
            assert_eq!(doing(&boss, &mind), "hunt");
        }
    }

    /// A hunter that gave up for want of any way to a player, struck again, runs and then
    /// leaves them be. Before: the blow dropped its giving up, so once its fright faded it
    /// hunted them again, back to the wall it found no way through.
    #[test]
    fn a_hunter_with_no_way_struck_again_does_not_hunt_again() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let near = [player(Vec3::new(1.0, 0.0, 0.0))];
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.find_no_way();
        mind.hurt(&wolf, Some(1), 1.0);
        assert_eq!((mind.fear, mind.anger), (1.0, 0.0), "struck with no way, it fought back rather than ran");
        mind.fear = 0.0;
        let _ = think(&wolf, &mut mind, &situation(&near));
        assert_ne!(doing(&wolf, &mind), "hunt");
        // Given up past its leash instead, struck again it takes the fight up.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.give_up();
        mind.hurt(&wolf, Some(1), 1.0);
        let _ = think(&wolf, &mut mind, &situation(&near));
        assert_eq!(doing(&wolf, &mind), "hunt");
    }

    #[test]
    fn neutral_creatures_fight_back_only_when_hurt() {
        let boar = species(&format!(
            "(creatures: [(name: \"boar\", health: 10, {BODY}, attack: (damage: (1, 2)), temperament: (fear: 0.2), behaviours: [(do: Wander(radius: 6.0)), (do: Defend)])])"
        ));
        let mut mind = Mind::new(&boar.def.temperament, 3);
        let near = [player(Vec3::new(1.0, 0.0, 0.0))];
        let _ = think(&boar, &mut mind, &situation(&near));
        assert_ne!(doing(&boar, &mind), "defend");
        mind.hurt(&boar, Some(1), 0.9);
        let decision = think(&boar, &mut mind, &situation(&near));
        assert_eq!((doing(&boar, &mind), decision.attack), ("defend", Some(1)));
    }

    #[test]
    fn creatures_go_home_when_far() {
        let snail = species(&format!("(creatures: [(name: \"snail\", health: 2, leash: 8.0, {BODY}, behaviours: [(do: Wander(radius: 4.0))])])"));
        let mut mind = Mind::new(&snail.def.temperament, 4);
        let far = Situation { position: Vec3::new(20.0, 0.0, 0.0), ..situation(&[]) };
        let decision = think(&snail, &mut mind, &far);
        assert_eq!((doing(&snail, &mind), decision.goal), ("return_home", Some(Vec3::ZERO)));
    }

    /// Someone the server sends about their day walks their errand however far it takes
    /// them from home, and a guard after someone follows only a way found after them, not
    /// the day's.
    #[test]
    fn errands_outrank_home_and_only_a_chase_leads_a_fight() {
        let guard = species(&format!(
            "(creatures: [(name: \"guard\", health: 10, leash: 8.0, {BODY}, attack: (damage: (1, 2)), behaviours: [(do: Routine(arrive: 0.5, look: 4.0)), (do: Defend)])])"
        ));
        let work = Errand { to: Vec3::new(40.0, 0.0, 0.0), last: true, pose: Pose::Idle, face: None, chase: None, pace: 1.0, steady: false };
        let mut mind = Mind::new(&guard.def.temperament, 6);
        let far = Situation { position: Vec3::new(30.0, 0.0, 0.0), errand: Some(work), ..situation(&[]) };
        let decision = think(&guard, &mut mind, &far);
        assert_eq!((doing(&guard, &mind), decision.goal), ("routine", Some(work.to)));
        // Struck by a player ten blocks off: straight at them while the errand is the day's,
        // along the way found when it is a chase.
        let near = [player(Vec3::new(20.0, 0.0, 0.0))];
        let mut mind = Mind::new(&guard.def.temperament, 6);
        mind.hurt(&guard, Some(1), 1.0);
        let at = Situation { position: Vec3::new(30.0, 0.0, 0.0), errand: Some(work), ..situation(&near) };
        let decision = think(&guard, &mut mind, &at);
        assert_eq!((doing(&guard, &mind), decision.goal), ("defend", Some(near[0].position)));
        let turn = Errand { to: Vec3::new(28.0, 0.0, 3.0), last: false, chase: Some(4.0), ..work };
        let decision = think(&guard, &mut mind, &Situation { errand: Some(turn), ..at });
        assert_eq!(decision.goal, Some(turn.to));
        // Within the handoff of its quarry, straight at them whatever the way says.
        let close = Errand { chase: Some(12.0), ..turn };
        let decision = think(&guard, &mut mind, &Situation { errand: Some(close), ..at });
        assert_eq!(decision.goal, Some(near[0].position));
    }

    #[test]
    fn forgiven_it_neither_fights_nor_flees() {
        let guard = species(&format!(
            "(creatures: [(name: \"guard\", health: 10, {BODY}, attack: (damage: (1, 2)), behaviours: [(do: Routine(arrive: 0.5, look: 4.0)), (do: Defend)])])"
        ));
        let mut mind = Mind::new(&guard.def.temperament, 6);
        mind.hurt(&guard, Some(1), 1.0);
        assert_eq!(mind.attacker(), Some(1));
        mind.forgive();
        assert_eq!((mind.attacker(), mind.anger, mind.fear), (None, 0.0, 0.0));
        let work = Errand { to: Vec3::new(40.0, 0.0, 0.0), last: true, pose: Pose::Idle, face: None, chase: None, pace: 1.0, steady: false };
        let near = [player(Vec3::new(20.0, 0.0, 0.0))];
        let at = Situation { position: Vec3::new(30.0, 0.0, 0.0), errand: Some(work), ..situation(&near) };
        let _ = think(&guard, &mut mind, &at);
        assert_eq!(doing(&guard, &mind), "routine");
    }

    #[test]
    fn danger_outranks_appetite() {
        let deer = species(&format!(
            "(creatures: [(name: \"deer\", health: 8, {BODY}, temperament: (fear: 0.8), behaviours: [(do: Graze, weight: 5.0), (do: Flee(radius: 12.0))])])"
        ));
        let mut mind = Mind::new(&deer.def.temperament, 5);
        mind.hunger = 1.0;
        let near = [player(Vec3::new(4.0, 0.0, 0.0))];
        let hungry = Situation { food: Some(IVec3::new(1, 0, 0)), ..situation(&near) };
        let _ = think(&deer, &mut mind, &hungry);
        assert_eq!(doing(&deer, &mind), "flee");
    }

    #[test]
    fn individuals_differ_and_read_clearly() {
        let species = Temperament { aggression: 0.5, fear: 0.5, curiosity: 0.5, sociability: 0.5, energy: 0.5, nervousness: 0.5, territory: 0.5, spread: 0.3 };
        let rolled: Vec<Personality> = (0..200).map(|seed| Personality::roll(&species, seed)).collect();
        let energy: Vec<f32> = rolled.iter().map(|p| p.energy).collect();
        let (low, high) = energy.iter().fold((1.0f32, 0.0f32), |(lo, hi), &e| (lo.min(e), hi.max(e)));
        assert!(low < 0.3 && high > 0.7, "too alike: {low}..{high}");
        assert!(rolled.iter().any(|p| !p.words().is_empty()));
        assert_eq!(Personality::roll(&species, 9), Personality::roll(&species, 9));
    }

    #[test]
    fn near_ties_are_drawn_at_random_and_commitment_holds() {
        let idler = species(&format!(
            "(creatures: [(name: \"idler\", health: 2, {BODY}, temperament: (energy: 1.0, spread: 0.0), behaviours: [(do: Wander(radius: 4.0)), (do: LookAround)])])"
        ));
        let mut picked = std::collections::BTreeSet::new();
        for seed in 0..64 {
            let mut mind = Mind::new(&idler.def.temperament, 1);
            let _ = think(&idler, &mut mind, &Situation { seed, ..situation(&[]) });
            picked.insert(doing(&idler, &mind).to_owned());
        }
        assert_eq!(picked.len(), 2, "both near-equal modules get chosen: {picked:?}");
    }
}

#[cfg(test)]
mod kept {
    use super::tests::*;
    use super::*;

    #[test]
    fn kept_creatures_do_not_fear_their_owner_and_court_a_mate() {
        let rabbit = species(&format!(
            "(creatures: [(name: \"rabbit\", health: 3, {BODY}, temperament: (fear: 0.9), behaviours: [(do: Wander(radius: 6.0)), (do: Flee(radius: 10.0))])])"
        ));
        let owner = [Seen { friend: true, ..player(Vec3::new(2.0, 0.0, 0.0)) }];
        let mut mind = Mind::new(&rabbit.def.temperament, 1);
        let _ = think(&rabbit, &mut mind, &situation(&owner));
        assert_ne!(mind.doing.map(|i| rabbit.modules[i].0.name()), Some("flee"));
        let ready = Situation { mate: Some(Vec3::new(5.0, 0.0, 0.0)), ..situation(&owner) };
        let decision = think(&rabbit, &mut mind, &ready);
        assert_eq!(mind.doing.map(|i| rabbit.modules[i].0.name()), Some("court"));
        assert_eq!(decision.goal, Some(Vec3::new(5.0, 0.0, 0.0)));
    }
}
