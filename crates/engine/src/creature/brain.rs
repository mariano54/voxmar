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
/// Seconds it recalls a player who frightened it (`Memory::ScaredBy`).
pub const SCARED_REMEMBERED: f32 = 10.0;
/// Below this liveliness (out of its active hours) a creature that sleeps
/// (`creature::Sleeps`) is asleep, as it shows resting below it (`modules::resting_pose`).
const SLEEPY: f32 = 0.4;
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
    /// Warning a player off before it attacks (`Warn`): in its warning stance, facing them.
    Warning,
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
    /// Whether it sees them from where it looks (`Senses::field`), with nothing between
    /// that hides them (the server's look). Out of it, it notices them only close by
    /// (`Senses::close`) or once they hurt it.
    pub in_view: bool,
    /// Holding a fire (an item tagged `fire`: a torch), which a creature afraid of fire
    /// (`creature::Fire`) gives ground from.
    pub fire: bool,
    /// Standing in a lit fire's ward at night (`Situation::ward`), where those afraid of
    /// fire go for no one unprovoked. Set only for kinds that keep out of wards.
    pub warded: bool,
    /// Going faster than a walk (a sprint), which wakes a sleeper further off
    /// (`creature::Sleeps::wakes`).
    pub loud: bool,
}

/// A lit fire's ward at night (`BlockDef::ward`, `MultiblockDef::ward`): `radius` blocks
/// round `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ward {
    pub at: Vec3,
    pub radius: f32,
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
    /// The lit fire's ward it stands in, at night, for kinds that keep out of wards
    /// (`creature::Fire::ward`).
    pub ward: Option<Ward>,
    /// The nearest food it steals lying on the ground, for kinds that scavenge
    /// (`creature::Scavenge`).
    pub scrap: Option<Vec3>,
    /// It carries a stolen piece of food off home (`creature::Scavenge`).
    pub carrying: bool,
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

/// Seconds a creature has warned (`Warn`), and seconds since it last did: past
/// `WARN_LAPSE` without warning, the warning is over and any next starts afresh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warning {
    pub seconds: f32,
    pub lapse: f32,
}

/// Seconds a warning holds over decisions spent on anything else (a step out of sight, a
/// look about), so it neither flickers nor starts over.
pub const WARN_LAPSE: f32 = 0.5;

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
    /// A warning it is giving (`Warn`): one clock for whoever it warns.
    pub warning: Option<Warning>,
    /// Zigzag side when fleeing, and other modules' small counters.
    pub zig: f32,
    /// Where it was startled with no one to blame (a snare it slipped): while it recalls
    /// the fright (`Memory::Startled`) it runs from there, not toward home.
    pub startled_at: Option<Vec3>,
    /// The player a boss came as near as it could and still could not reach (one down a
    /// hole too deep for it, or under a roof), and where they stood: while they stay there
    /// it keeps off out of their reach, watching them (`KEEP_OFF`), and comes on once they
    /// leave it. After anyone else, it holds off no longer.
    pub held_off: Option<(u32, Vec3)>,
    /// The players it has noticed (`Senses`), each with the seconds it keeps them in mind
    /// out of sight (`Senses::keep`) and the seconds since it first noticed them (its double take,
    /// `Senses::react`).
    pub noticed: Vec<(u32, f32, f32)>,
    /// Whether its last decision was a double take: what it chose then is still fresh when
    /// it first acts (a flight still raises the alarm to its kin).
    pub looking: bool,
    /// The individual's own hash, for choices that stay its own (which of its kin it keeps
    /// near, how far it likes to stray).
    pub seed: u64,
    /// Seconds it has run flat out lately (`creature::Tires`), back down as it rests.
    pub sprinted: f32,
    /// Seconds left of running winded (`creature::Tires::rest`), slowed.
    pub winded: f32,
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
            warning: None,
            seed,
            zig: 1.0,
            startled_at: None,
            sprinted: 0.0,
            winded: 0.0,
            held_off: None,
            noticed: Vec::new(),
            looking: false,
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

    /// A player it warned off (`Warn`) stayed: it goes for them as if they had struck it,
    /// held against them as a blow would be, though nothing hurt it.
    pub fn provoked(&mut self, by: u32) {
        self.warning = None;
        self.anger = 1.0;
        self.remember(Memory::HurtBy(by), HURT_REMEMBERED);
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
    /// It snatches a piece of the food lying here (`creature::Scavenge`).
    pub take: Option<Vec3>,
    /// It eats what it carries (`creature::Scavenge`), home with it.
    pub swallow: bool,
    /// It raises the alarm for kin nearby.
    pub alarm: bool,
    pub pose: Pose,
    /// The player it acts on, if any (whom it chases, flees, comes to).
    pub target: Option<u32>,
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
    // Whom it notices (`Senses`): those it sees where it looks, those close by, those
    // holding its food within its hearing (it smells it, wherever it looks: a sheep comes
    // round a wall to the wheat), those who hurt it and its keeper; it keeps them in mind a
    // while out of sight (`Senses::keep`), and senses no one else.
    for (_, left, since) in &mut mind.noticed {
        (*left, *since) = (*left - dt, *since + dt);
    }
    mind.noticed.retain(|n| n.1 > 0.0);
    // Asleep out of its hours (`creature::Sleeps`), with no one in mind, nothing afraid or
    // angry, wild and about no errand: eyes shut, it notices only a quiet step within its
    // `quiet`, and a sprint within its `wakes`, which wakes it with a start.
    let asleep = def.sleeps.filter(|_| lively < SLEEPY && mind.noticed.is_empty() && mind.fear < 0.3 && mind.anger <= 0.0 && !at.kept && at.errand.is_none());
    for p in at.players {
        let d = p.position.distance(at.position);
        let smelt = p.tempting && d < def.senses.hearing;
        if let Some(sleeps) = asleep {
            let hurt = mind.recalls(Memory::HurtBy(p.id));
            let startled = p.loud && d < sleeps.wakes && !p.friend && !hurt;
            if !(p.friend || hurt || d <= sleeps.quiet || startled) {
                continue;
            }
            mind.noticed.push((p.id, def.senses.keep, 0.0));
            // Woken with a start: a fighter comes up provoked, with no warning; the rest bolt.
            if startled && d > sleeps.quiet {
                if creature.fights() {
                    mind.provoked(p.id);
                } else {
                    mind.startled(p.position);
                    mind.remember(Memory::ScaredBy(p.id), SCARED_REMEMBERED);
                }
            }
            continue;
        }
        if p.friend || d <= def.senses.close || p.in_view && d < sight || smelt || mind.recalls(Memory::HurtBy(p.id)) {
            match mind.noticed.iter_mut().find(|n| n.0 == p.id) {
                Some(n) => n.1 = def.senses.keep,
                None => mind.noticed.push((p.id, def.senses.keep, 0.0)),
            }
        }
    }
    // A list of its own only when it leaves someone out.
    let known = |p: &Seen| mind.noticed.iter().any(|n| n.0 == p.id);
    let all = at.players.iter().all(known);
    let noticed: Vec<Seen> = if all { Vec::new() } else { at.players.iter().filter(|p| known(p)).copied().collect() };
    let at = &Situation { players: if all { at.players } else { &noticed }, ..*at };
    // Still asleep, no one woke it: it sleeps on.
    if asleep.is_some() && mind.noticed.is_empty() {
        mind.doing = None;
        lapse_warning(mind, dt);
        rest(mind, dt);
        return Decision { pose: Pose::Resting, ..Default::default() };
    }
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
        lapse_warning(mind, dt);
        rest(mind, dt);
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
    let fresh = mind.doing != Some(index) || mind.looking;
    if fresh {
        mind.doing = Some(index);
        mind.since = 0.0;
        mind.goal = None;
    }
    // Its double take (`Senses::react`): just noticed, it stands and looks at them before
    // it acts on them, unless they hurt it.
    let taking = want.target.filter(|p| {
        !mind.recalls(Memory::HurtBy(p.id)) && mind.noticed.iter().any(|n| n.0 == p.id && n.2 < def.senses.react)
    });
    // A decision spent on anything but its warning, a double take too, runs the warning's
    // lapse.
    let warns = creature.modules[index].0.name() == "warn";
    if !warns {
        lapse_warning(mind, dt);
    }
    mind.looking = taking.is_some();
    if let Some(p) = taking {
        return Decision { face: Some(p.position), pose: Pose::Alert, ..Default::default() };
    }
    // Not running, it gets its wind back (`Tires`).
    if creature.modules[index].0.name() != "flee" {
        rest(mind, dt);
    }
    let ctx = Ctx { creature, mind: &mind.clone(), at, lively, sight };
    let mut decision = creature.modules[index].0.act(&ctx, mind, want.target, fresh);
    // Provoked by its warning, it goes for them in this same decision.
    if warns && mind.warning.is_none() && mind.anger > 0.0 {
        mind.doing = None;
        return think(creature, mind, &Situation { dt: 0.0, ..*at });
    }
    decision.speed *= mind.personality.pace();
    decision.target = want.target.map(|p| p.id);
    decision
}

/// Not fleeing, it gets its wind back (`creature::Tires`): its sprint and any time it had
/// left winded run down a second for a second.
fn rest(mind: &mut Mind, dt: f32) {
    mind.sprinted = (mind.sprinted - dt).max(0.0);
    mind.winded = (mind.winded - dt).max(0.0);
}

/// Doing anything but warn, a warning lapses (`WARN_LAPSE`).
fn lapse_warning(mind: &mut Mind, dt: f32) {
    if let Some(warning) = &mut mind.warning {
        warning.lapse += dt;
        if warning.lapse > WARN_LAPSE {
            mind.warning = None;
        }
    }
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
        Situation { position: Vec3::ZERO, home: Vec3::ZERO, health: 1.0, phase: 0, scale: 1.0, daylight: 1.0, players, kin: &[], food: None, mate: None, kept: false, errand: None, ward: None, scrap: None, carrying: false, dt: 0.2, seed: 7 }
    }

    pub(super) fn player(at: Vec3) -> Seen {
        Seen { id: 1, position: at, tempting: false, combat_level: 3, creative: false, tolerated: false, friend: false, open: true, in_view: true, fire: false, warded: false, loud: false }
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

    /// A player out of its view (`Seen::in_view`, judged by the server from where it
    /// looks) is no one to a deer until close (`Senses::close`), or until they hurt it; one
    /// in view it notices, and keeps in mind a while once it has turned away (`Senses::keep`).
    #[test]
    fn a_deer_notices_whom_it_sees_or_hears_close_by_and_keeps_them_in_mind() {
        let deer = species(&format!(
            "(creatures: [(name: \"deer\", health: 6, {BODY}, senses: (sight: 20.0, close: 3.0), temperament: (fear: 0.9), behaviours: [(do: Wander(radius: 6.0)), (do: Flee(radius: 12.0))])])"
        ));
        let unseen = [Seen { in_view: false, ..player(Vec3::new(8.0, 0.0, 0.0)) }];
        let mut mind = Mind::new(&deer.def.temperament, 1);
        let _ = think(&deer, &mut mind, &situation(&unseen));
        assert_ne!(doing(&deer, &mind), "flee", "it ran from a player it never saw");
        let close = [Seen { in_view: false, ..player(Vec3::new(2.5, 0.0, 0.0)) }];
        let _ = think(&deer, &mut mind, &situation(&close));
        assert_eq!(doing(&deer, &mind), "flee", "it never heard a player close behind it");
        let mut mind = Mind::new(&deer.def.temperament, 1);
        let _ = think(&deer, &mut mind, &situation(&[player(Vec3::new(8.0, 0.0, 0.0))]));
        assert_eq!(doing(&deer, &mind), "flee", "it never saw a player before it");
        // Turned away, it still runs from them.
        let _ = think(&deer, &mut mind, &situation(&unseen));
        assert_eq!(doing(&deer, &mind), "flee", "it forgot a player once it turned from them");
        // Hurt by one it never saw, it knows them.
        let mut mind = Mind::new(&deer.def.temperament, 1);
        mind.hurt(&deer, Some(1), 0.5);
        let _ = think(&deer, &mut mind, &situation(&unseen));
        assert_ne!(mind.noticed.len(), 0, "it never noticed who hurt it");
    }

    /// A wolf with a double take (`Senses::react`) stands and looks at a player it has just
    /// noticed, then hunts them; struck, it fights back at once.
    #[test]
    fn a_wolf_takes_a_look_before_it_hunts() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, senses: (react: 0.6), attack: (damage: (1, 3)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let far = [player(Vec3::new(8.0, 0.0, 0.0))];
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let first = think(&wolf, &mut mind, &situation(&far));
        assert!(first.goal.is_none() && first.face == Some(far[0].position) && first.pose == Pose::Alert, "no look first: {first:?}");
        let (mut looked, mut chased) = (0.2, None);
        for _ in 0..10 {
            let decision = think(&wolf, &mut mind, &situation(&far));
            if decision.goal.is_some() {
                chased = Some(looked);
                break;
            }
            looked += 0.2;
        }
        assert!(chased.is_some_and(|s| (0.6..=1.0).contains(&s)), "chased after {chased:?} s of looking");
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.hurt(&wolf, Some(1), 1.0);
        let struck = think(&wolf, &mut mind, &situation(&far));
        assert!(struck.goal.is_some(), "struck, it stood and looked: {struck:?}");
    }

    /// A double take spent on anyone but the one it warns runs its warning's lapse, as any
    /// other decision does: a look at one unseen beside it does not hold a warning open.
    #[test]
    fn a_double_take_runs_a_warnings_lapse() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, senses: (react: 0.6), attack: (damage: (1, 3), reach: 1.0), warn: (near: 1.5, seconds: 1.0), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.warning = Some(Warning { seconds: 0.4, lapse: 0.0 });
        let close = [Seen { in_view: false, ..player(Vec3::new(2.5, 0.0, 0.0)) }];
        let decision = think(&wolf, &mut mind, &situation(&close));
        assert_eq!((decision.pose, doing(&wolf, &mind)), (Pose::Alert, "hunt"), "a look at the one beside it");
        assert!(mind.warning.is_some_and(|w| (w.lapse - 0.2).abs() < 1e-4), "the look held the warning: {:?}", mind.warning);
    }

    /// Prey with a double take looks, then flees, and its first decision to flee still
    /// raises the alarm to its kin, as one with no double take does at once.
    #[test]
    fn prey_that_looks_first_still_raises_the_alarm_when_it_runs() {
        let deer = species(&format!(
            "(creatures: [(name: \"deer\", health: 4, {BODY}, senses: (react: 0.4), temperament: (fear: 0.9), behaviours: [(do: Wander(radius: 6.0)), (do: Flee(radius: 10.0))])])"
        ));
        let near = [player(Vec3::new(5.0, 0.0, 0.0))];
        let mut mind = Mind::new(&deer.def.temperament, 1);
        let mut decisions = (0..6).map(|_| think(&deer, &mut mind, &situation(&near)));
        assert!(decisions.next().is_some_and(|d| d.goal.is_none() && d.face == Some(near[0].position)), "no look first");
        let first_flight = decisions.find(|d| d.goal.is_some()).expect("it never fled");
        assert!(first_flight.alarm, "its first flight raised no alarm: {first_flight:?}");
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

    /// A hunter that warns stands and warns a player its hunt would go for rather than
    /// going straight for them; they stay its `seconds` and it goes for them, its hunt
    /// taking over in that same decision. A decision spent on anything else does not start
    /// the warning over; backed off for longer, they are let be, and a later warning starts
    /// afresh. One who steps within `near` provokes it at once; a blow, as ever. Holding its
    /// food changes nothing: its chase outranked the food before.
    #[test]
    fn a_warning_hunter_warns_then_goes_for_one_who_stays() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), warn: (near: 3.0, seconds: 1.0, stance: (nod: 0.4)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let at = [player(Vec3::new(6.0, 0.0, 0.0))];
        let tempting = [Seen { tempting: true, ..at[0] }];
        for at in [&at, &tempting] {
            let mut mind = Mind::new(&wolf.def.temperament, 2);
            // dt 0.2: five looks warn, the sixth runs out its second.
            for _ in 0..5 {
                let decision = think(&wolf, &mut mind, &situation(at));
                assert_eq!((doing(&wolf, &mind), decision.pose, decision.goal, decision.face), ("warn", Pose::Warning, None, Some(at[0].position)));
            }
            let decision = think(&wolf, &mut mind, &situation(at));
            assert_eq!((mind.attacker(), mind.anger, doing(&wolf, &mind)), (Some(1), 1.0, "hunt"), "it stayed: provoked, and hunting at once");
            assert_eq!((decision.goal, decision.pose), (Some(at[0].position), Pose::Idle), "and goes for them");
        }

        // One it does not see, beyond close range and unhurt, it has not noticed and leaves
        // alone: it warns the farther one who stands in sight, and goes for them alone.
        let hidden = [Seen { in_view: false, ..at[0] }, Seen { id: 2, position: Vec3::new(8.0, 0.0, 0.0), ..at[0] }];
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        for _ in 0..5 {
            let decision = think(&wolf, &mut mind, &situation(&hidden));
            assert_eq!(
                (doing(&wolf, &mind), decision.pose, decision.goal, decision.face, decision.attack, mind.attacker()),
                ("warn", Pose::Warning, None, Some(hidden[1].position), None, None),
                "the seen one at 8 warned, the unseen one at 6 not hunted"
            );
        }
        let decision = think(&wolf, &mut mind, &situation(&hidden));
        assert_eq!((mind.attacker(), doing(&wolf, &mind), decision.goal), (Some(2), "hunt", Some(hidden[1].position)), "the warned one stayed: it goes for them, not the unseen one");
        // One it has noticed (seen once) and then cannot see, within `keep`, it goes for
        // unwarned: a warning goes only to one seen.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let _ = think(&wolf, &mut mind, &situation(&at));
        assert_eq!(doing(&wolf, &mind), "warn", "seen, warned");
        let gone = [Seen { in_view: false, ..at[0] }];
        let decision = think(&wolf, &mut mind, &situation(&gone));
        assert_eq!((doing(&wolf, &mind), decision.goal, decision.pose), ("hunt", Some(gone[0].position), Pose::Idle), "noticed, out of view: hunted unwarned");
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let _ = think(&wolf, &mut mind, &Situation { kept: true, ..situation(&at) });
        assert_eq!(doing(&wolf, &mind), "warn", "kept, it hunts as before, with its warning");

        // Hunting near and far (a mod's kind), it warns as far as its widest hunt.
        let pack = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), warn: (near: 3.0, seconds: 1.0, stance: (nod: 0.4)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 8.0)), (do: Hunt(radius: 16.0))])])"
        ));
        let far = [player(Vec3::new(12.0, 0.0, 0.0))];
        let mut mind = Mind::new(&pack.def.temperament, 2);
        for _ in 0..5 {
            let _ = think(&pack, &mut mind, &situation(&far));
            assert_eq!(doing(&pack, &mind), "warn", "past its near hunt, within its far one");
        }
        let decision = think(&pack, &mut mind, &situation(&far));
        assert_eq!((mind.attacker(), doing(&pack, &mind), decision.goal), (Some(1), "hunt", Some(far[0].position)), "it stayed: the far hunt takes them");

        // Backed off out of its reach, they are let be; come back, the warning starts again.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        for _ in 0..4 {
            let _ = think(&wolf, &mut mind, &situation(&at));
        }
        let away = [player(Vec3::new(40.0, 0.0, 0.0))];
        for _ in 0..3 {
            let _ = think(&wolf, &mut mind, &situation(&away));
            assert_ne!(doing(&wolf, &mind), "hunt");
        }
        assert_eq!((mind.warning, mind.attacker()), (None, None), "let be");
        for _ in 0..5 {
            let _ = think(&wolf, &mut mind, &situation(&at));
            assert_eq!(doing(&wolf, &mind), "warn", "warned afresh");
        }

        // Stepping within `near`, at once.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let close = [player(Vec3::new(1.0, 0.0, 0.0))];
        let decision = think(&wolf, &mut mind, &situation(&close));
        assert_eq!((mind.attacker(), decision.attack), (Some(1), Some(1)), "too close: it strikes");
        // A player it would leave be, it does not warn either.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let _ = think(&wolf, &mut mind, &situation(&[Seen { combat_level: 12, ..at[0] }]));
        assert_eq!(doing(&wolf, &mind), "nothing");
        // Struck, no warning: it fights.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.hurt(&wolf, Some(1), 0.9);
        let _ = think(&wolf, &mut mind, &situation(&at));
        assert_eq!(doing(&wolf, &mind), "hunt");
    }

    /// One that only defends itself and warns (a badger) warns a player it sees within its
    /// radius, and once provoked defends its ground in that same decision. One who steps a
    /// little past the radius is still warned (`WARN_EXIT`), so the edge does not flicker;
    /// further, and they are let be. One out of its sight, or holding the food that tempts
    /// it, it does not warn: the food leads it as before.
    #[test]
    fn a_defender_that_warns_defends_its_ground() {
        let badger = species(&format!(
            "(creatures: [(name: \"badger\", health: 10, {BODY}, attack: (damage: (1, 2)), warn: (radius: 5.0, near: 1.5, seconds: 0.6), temperament: (fear: 0.2), behaviours: [(do: Wander(radius: 6.0)), (do: Tempted(radius: 10.0)), (do: Defend)])])"
        ));
        let at = [player(Vec3::new(3.0, 0.0, 0.0))];
        let mut mind = Mind::new(&badger.def.temperament, 3);
        for _ in 0..3 {
            let decision = think(&badger, &mut mind, &situation(&at));
            assert_eq!((doing(&badger, &mind), decision.pose), ("warn", Pose::Warning));
        }
        let _ = think(&badger, &mut mind, &situation(&at));
        assert_eq!((doing(&badger, &mind), mind.attacker()), ("defend", Some(1)));

        // One decision away (out of sight) keeps the warning's clock.
        let mut mind = Mind::new(&badger.def.temperament, 3);
        let hidden = [Seen { in_view: false, ..at[0] }];
        for _ in 0..2 {
            let _ = think(&badger, &mut mind, &situation(&at));
        }
        let _ = think(&badger, &mut mind, &situation(&hidden));
        assert!(mind.warning.is_some(), "a moment out of sight starts nothing over");
        let _ = think(&badger, &mut mind, &situation(&at));
        assert_eq!(doing(&badger, &mind), "warn");
        let _ = think(&badger, &mut mind, &situation(&at));
        assert_eq!(doing(&badger, &mind), "defend", "its seconds ran out, the moment away not counted");

        // A step past the edge, still warned; well past it, let be.
        let mut mind = Mind::new(&badger.def.temperament, 3);
        let _ = think(&badger, &mut mind, &situation(&at));
        let _ = think(&badger, &mut mind, &situation(&[player(Vec3::new(6.0, 0.0, 0.0))]));
        assert_eq!(doing(&badger, &mind), "warn", "a step past the edge");
        let _ = think(&badger, &mut mind, &situation(&[player(Vec3::new(7.0, 0.0, 0.0))]));
        assert_ne!(doing(&badger, &mind), "warn", "well past it");
        assert_eq!(mind.attacker(), None);

        // Out of its sight, or tempting it, no warning; the tempted one follows the food.
        for (seen, doing_now) in [(Seen { in_view: false, ..at[0] }, None), (Seen { tempting: true, ..at[0] }, Some("tempted"))] {
            let mut mind = Mind::new(&badger.def.temperament, 3);
            for _ in 0..6 {
                let _ = think(&badger, &mut mind, &situation(&[seen]));
                assert_ne!(doing(&badger, &mind), "warn", "{seen:?}");
                if let Some(doing_now) = doing_now {
                    assert_eq!(doing(&badger, &mind), doing_now);
                }
            }
            assert_eq!(mind.attacker(), None);
        }
        // Kept, it warns no one.
        let mut mind = Mind::new(&badger.def.temperament, 3);
        let _ = think(&badger, &mut mind, &Situation { kept: true, ..situation(&at) });
        assert_ne!(doing(&badger, &mind), "warn");
    }

    /// What `warn` may say is checked at load: an attack to follow it, no boss, a radius
    /// only for one that does not hunt, and each number in its range.
    #[test]
    fn warnings_out_of_range_are_refused() {
        let build = |creature: &str| {
            let defs = CreatureRegistry::read("test", &format!("(creatures: [(name: \"c\", health: 2, {BODY}, {creature})])")).unwrap().defs;
            CreatureRegistry::new(defs, Default::default(), &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).map(|_| ()).map_err(|e| e.to_string())
        };
        let defends = "attack: (damage: (1, 2)), behaviours: [(do: Defend)]";
        let hunts = "attack: (damage: (1, 2)), behaviours: [(do: Hunt(radius: 10.0))]";
        assert_eq!(build(&format!("{defends}, warn: (radius: 5.0, near: 1.5, seconds: 0.6)")), Ok(()));
        assert_eq!(build(&format!("{hunts}, warn: (near: 3.0, seconds: 1.5)")), Ok(()));
        for (creature, refused) in [
            ("warn: (radius: 5.0, near: 1.5, seconds: 0.6)".to_owned(), "no `attack`"),
            (format!("boss: true, {defends}, warn: (radius: 5.0, near: 1.5, seconds: 0.6)"), "a boss warns no one"),
            (format!("{hunts}, warn: (radius: 8.0, near: 3.0, seconds: 1.5)"), "no `radius`"),
            (format!("{defends}, warn: (near: 1.5, seconds: 0.6)"), "needs a `radius`"),
            (format!("{defends}, warn: (radius: 5.0, near: 4.5, seconds: 0.6)"), "near 0.5"),
            (format!("{defends}, warn: (radius: 5.0, near: 6.0, seconds: 0.6)"), "near 0.5"),
            (format!("{defends}, warn: (radius: 1.0, near: 0.5, seconds: 0.6)"), "radius 2"),
            (format!("{defends}, warn: (radius: 20.0, near: 1.5, seconds: 0.6)"), "radius 2"),
            (format!("{defends}, warn: (radius: 5.0, near: 1.5, seconds: 0.0)"), "seconds 0.5"),
            (format!("{defends}, warn: (radius: 5.0, near: 1.5, seconds: -1.0)"), "seconds 0.5"),
            (format!("{defends}, warn: (radius: 5.0, near: 1.5, seconds: 5.0)"), "seconds 0.5"),
            (format!("{defends}, warn: (radius: 5.0, near: 1.5, seconds: 0.6, stance: (rear: 1.5))"), "stance 0 to 1"),
        ] {
            let built = build(&creature);
            assert!(built.as_ref().is_err_and(|e| e.contains(refused)), "{creature}: {built:?}");
        }
    }

    /// A torch held at one afraid of fire (`creature::Fire`) backs it off, facing the flame,
    /// its warning let lapse; at night it walks out of a lit fire's ward and goes for no one
    /// inside one. Provoked, kept, on an errand or bolder than its kind, fire holds it back
    /// no more.
    #[test]
    fn fire_keeps_the_wild_back() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), warn: (near: 2.0, seconds: 1.0, stance: (nod: 0.4)), fire: (fear: 1.0, give: 4.0), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let torch = [Seen { fire: true, ..player(Vec3::new(3.0, 0.0, 0.0)) }];
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.warning = Some(Warning { seconds: 0.6, lapse: 0.0 });
        for _ in 0..4 {
            let decision = think(&wolf, &mut mind, &situation(&torch));
            assert_eq!((doing(&wolf, &mind), decision.face, decision.pose), ("fire", Some(torch[0].position), Pose::Warning), "backing off, facing the flame");
            assert!(decision.goal.is_some_and(|g| g.x < -1.0), "away from it: {:?}", decision.goal);
        }
        assert_eq!((mind.warning, mind.attacker()), (None, None), "its warning lapsed, unprovoked");
        // Past `give` (and the slack once backing off) it warns, but while the torch is held
        // at it, it never comes, however long; put away, it does.
        let off = [Seen { fire: true, ..player(Vec3::new(5.5, 0.0, 0.0)) }];
        for _ in 0..20 {
            let _ = think(&wolf, &mut mind, &situation(&off));
            assert_eq!((doing(&wolf, &mind), mind.attacker()), ("warn", None), "held off by the torch");
        }
        let _ = think(&wolf, &mut mind, &situation(&[Seen { fire: false, ..off[0] }]));
        assert_eq!((doing(&wolf, &mind), mind.attacker()), ("hunt", Some(1)), "the torch put away, its warning ran out long since");
        // A torch from one it has not noticed (unseen, past its `close`), or in a creative
        // player's hand, is no flame to it.
        let behind = Seen { in_view: false, position: Vec3::new(3.5, 0.0, 0.0), ..torch[0] };
        for unseen in [behind, Seen { creative: true, ..torch[0] }] {
            let mut mind = Mind::new(&wolf.def.temperament, 2);
            let _ = think(&wolf, &mut mind, &situation(&[unseen]));
            assert_ne!(doing(&wolf, &mind), "fire", "{unseen:?}");
        }
        // Noticed, it backs from the flame though it turns from it, and never hunts them.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let _ = think(&wolf, &mut mind, &situation(&[Seen { in_view: true, ..behind }]));
        for _ in 0..10 {
            let _ = think(&wolf, &mut mind, &situation(&[behind]));
            assert_eq!(doing(&wolf, &mind), "fire", "turned from the flame, it still backs from it");
        }
        let away = [Seen { position: Vec3::new(6.0, 0.0, 0.0), ..behind }];
        for _ in 0..10 {
            let _ = think(&wolf, &mut mind, &situation(&away));
            assert_ne!(doing(&wolf, &mind), "hunt", "unseen past the torch, it does not go for them");
        }
        // Struck, it fights; kept, or on an errand, fire is nothing to it; nor to a bold one.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        mind.hurt(&wolf, Some(1), 0.9);
        let _ = think(&wolf, &mut mind, &situation(&torch));
        assert_eq!(doing(&wolf, &mind), "hunt", "provoked: the torch does not hold it");
        let errand = Errand { to: Vec3::new(-5.0, 0.0, 0.0), last: true, pose: Pose::Idle, face: None, chase: None, pace: 1.0, steady: false };
        for at in [Situation { kept: true, ..situation(&torch) }, Situation { errand: Some(errand), ..situation(&torch) }] {
            let mut mind = Mind::new(&wolf.def.temperament, 2);
            let _ = think(&wolf, &mut mind, &at);
            assert_ne!(doing(&wolf, &mind), "fire", "kept {} errand {:?}", at.kept, at.errand);
        }
        let bold = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), warn: (near: 2.0, seconds: 1.0), fire: (fear: 0.0, give: 4.0), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let mut mind = Mind::new(&bold.def.temperament, 2);
        let _ = think(&bold, &mut mind, &situation(&torch));
        assert_eq!(doing(&bold, &mind), "warn", "one of the bold: it stands its ground");
        // Its kind's `fear` is the share that give ground, each by its own roll.
        let half = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), warn: (near: 2.0, seconds: 1.0), fire: (fear: 0.5, give: 4.0), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let gave = (0..400u64)
            .filter(|&seed| {
                let mut mind = Mind::new(&half.def.temperament, seed);
                let _ = think(&half, &mut mind, &situation(&torch));
                doing(&half, &mind) == "fire"
            })
            .count();
        assert!((160..=240).contains(&gave), "{gave} of 400 gave ground at fear 0.5");

        // At night, in a lit fire's ward, it walks out, past the ward's edge.
        let ward = Ward { at: Vec3::new(-2.0, 0.0, 0.0), radius: 8.0 };
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let decision = think(&wolf, &mut mind, &Situation { ward: Some(ward), ..situation(&[]) });
        assert_eq!(doing(&wolf, &mind), "fire");
        assert!(decision.goal.is_some_and(|g| g.distance(ward.at) > ward.radius), "out of the light: {:?}", decision.goal);
        let unwarded = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), fire: (fear: 1.0, give: 4.0, ward: false), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let mut mind = Mind::new(&unwarded.def.temperament, 2);
        let _ = think(&unwarded, &mut mind, &Situation { ward: Some(ward), ..situation(&[]) });
        assert_ne!(doing(&unwarded, &mind), "fire", "`ward: false`: it pays the light no heed");
        // One in a ward it neither warns nor hunts, though in sight and in reach.
        let inside = [Seen { warded: true, ..player(Vec3::new(6.0, 0.0, 0.0)) }];
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let _ = think(&wolf, &mut mind, &situation(&inside));
        assert!(!["warn", "hunt"].contains(&doing(&wolf, &mind)), "{}", doing(&wolf, &mind));
        let badger = species(&format!(
            "(creatures: [(name: \"badger\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3), reach: 1.0), warn: (radius: 5.0, near: 1.5, seconds: 1.0), fire: (fear: 1.0, give: 3.0), behaviours: [(do: Defend)])])"
        ));
        let near = [Seen { warded: true, ..player(Vec3::new(2.5, 0.0, 0.0)) }];
        let mut mind = Mind::new(&badger.def.temperament, 2);
        let _ = think(&badger, &mut mind, &situation(&near));
        assert_ne!(doing(&badger, &mind), "warn", "a defender warns no one in the light");
        let _ = think(&badger, &mut mind, &situation(&[Seen { warded: false, ..near[0] }]));
        assert_eq!(doing(&badger, &mind), "warn", "out of it, as ever");
    }

    /// Prey that tires (`creature::Tires`) runs flat out its `after` seconds, then winded at
    /// its `pace` for `rest` seconds, then flat out again; kept, it runs as ever. Bad
    /// numbers are refused on load.
    #[test]
    fn prey_that_tires_runs_winded_after_a_sprint() {
        let rabbit = species(&format!(
            "(creatures: [(name: \"rabbit\", health: 3, {BODY}, movement: (walk: 2.0, run: 7.0), tires: (after: 1.0, pace: 0.5, rest: 2.0), temperament: (fear: 0.9), behaviours: [(do: Flee(radius: 10.0))])])"
        ));
        let near = [player(Vec3::new(2.0, 0.0, 0.0))];
        let speeds = |at: &Situation| {
            let mut mind = Mind::new(&rabbit.def.temperament, 2);
            let full = 7.0 * mind.personality.pace();
            (0..20)
                .map(|_| {
                    let decision = think(&rabbit, &mut mind, at);
                    assert_eq!(doing(&rabbit, &mind), "flee");
                    (decision.speed / full * 100.0).round() as u32
                })
                .collect::<Vec<_>>()
        };
        // Thinking every 0.2 s: five flat out, ten winded, then flat out again.
        let ran = speeds(&situation(&near));
        assert_eq!(ran, [[100; 5].as_slice(), &[50; 10], &[100; 5]].concat(), "{ran:?}");
        assert_eq!(speeds(&Situation { kept: true, ..situation(&near) }), [100; 20], "kept, it is not run down");
        // Hurt, it limps: one with `hurt: 0.5` at half its health runs 0.75 as fast, winded too.
        let limping = species(&format!(
            "(creatures: [(name: \"rabbit\", health: 3, {BODY}, movement: (walk: 2.0, run: 7.0), tires: (after: 1.0, pace: 0.5, rest: 2.0, hurt: 0.5), temperament: (fear: 0.9), behaviours: [(do: Flee(radius: 10.0))])])"
        ));
        let mut mind = Mind::new(&limping.def.temperament, 2);
        let full = 7.0 * mind.personality.pace();
        let ran: Vec<u32> = (0..10).map(|_| (think(&limping, &mut mind, &Situation { health: 0.5, ..situation(&near) }).speed / full * 1000.0).round() as u32).collect();
        assert_eq!(ran, [[750; 5].as_slice(), &[375; 5]].concat(), "{ran:?}");

        let build = |tires: &str| {
            let defs = CreatureRegistry::read("test", &format!("(creatures: [(name: \"c\", health: 2, {BODY}, tires: {tires})])")).unwrap().defs;
            CreatureRegistry::new(defs, Default::default(), &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).map(|_| ()).map_err(|e| e.to_string())
        };
        assert_eq!(build("(after: 3.0, pace: 0.5, rest: 4.0)"), Ok(()));
        for bad in ["(after: 3.0, pace: 0.5, rest: 4.0, hurt: 1.0)", "(after: 0.5, pace: 0.5, rest: 4.0)", "(after: 3.0, pace: 1.0, rest: 4.0)", "(after: 3.0, pace: 0.1, rest: 4.0)", "(after: 3.0, pace: 0.5, rest: 40.0)"] {
            assert!(build(bad).is_err_and(|e| e.contains("`tires` needs")), "{bad}");
        }
    }

    /// A game bird that sits tight (`creature::Flush`) crouches still while a noticed
    /// player is beyond its `hold`, then bursts away when they come within it: its first
    /// `seconds` at `burst` times its run, then at its run. Hurt, it does not burst; beyond
    /// its kind's flee distance it does not sit tight; kept, it flees at its kind's distance
    /// as ever. Bad numbers, or a flush on a kind that attacks, are refused.
    #[test]
    fn a_game_bird_sits_tight_then_bursts_away() {
        let bird = species(&format!(
            "(creatures: [(name: \"quail\", health: 2, {BODY}, movement: (walk: 1.0, run: 5.0), flush: (hold: 2.5, burst: 2.0, seconds: 0.6), temperament: (fear: 0.9), behaviours: [(do: Flee(radius: 10.0)), (do: Freeze)])])"
        ));
        let far = [player(Vec3::new(6.0, 0.0, 0.0))];
        let mut mind = Mind::new(&bird.def.temperament, 2);
        for _ in 0..10 {
            let decision = think(&bird, &mut mind, &situation(&far));
            assert_eq!((doing(&bird, &mind), decision.pose, decision.goal, decision.speed), ("hide", Pose::Crouched, None, 0.0), "sits tight at six blocks");
        }
        let near = [player(Vec3::new(2.0, 0.0, 0.0))];
        let full = 5.0 * mind.personality.pace();
        let ran: Vec<u32> = (0..6)
            .map(|_| {
                let decision = think(&bird, &mut mind, &situation(&near));
                assert_eq!(doing(&bird, &mind), "flee");
                (decision.speed / full * 100.0).round() as u32
            })
            .collect();
        // Thinking every 0.2 s: three at the burst, then its run.
        assert_eq!(ran, [200, 200, 200, 100, 100, 100], "{ran:?}");
        let _ = think(&bird, &mut mind, &situation(&far));
        assert_eq!(doing(&bird, &mind), "flee", "flushed, it runs clear and does not settle again at six blocks");
        // Hurt, it flees as any prey does: no burst.
        let mut hurt = Mind::new(&bird.def.temperament, 2);
        hurt.hurt(&bird, Some(1), 0.9);
        let ran: Vec<u32> = (0..3).map(|_| (think(&bird, &mut hurt, &situation(&near)).speed / full * 100.0).round() as u32).collect();
        assert_eq!((doing(&bird, &hurt), ran), ("flee", vec![100; 3]), "a hurt bird does not burst");
        // Its fright faded and its flight forgotten, it still recalls who hurt it, and flees
        // them at its kind's distance rather than wait for them at its hold.
        (hurt.fear, hurt.since) = (0.0, 0.0);
        hurt.memories.retain(|(m, _)| !matches!(m, Memory::ScaredBy(_)));
        let _ = think(&bird, &mut hurt, &situation(&far));
        assert_eq!(doing(&bird, &hurt), "flee", "wounded, at six blocks from the one who hurt it");
        // Nor does a bystander nearer than the hunter, beyond its hold, hide the hunter from it.
        let _ = think(&bird, &mut hurt, &situation(&[Seen { id: 2, ..player(Vec3::new(4.0, 0.0, 0.0)) }, far[0]]));
        assert_eq!(doing(&bird, &hurt), "flee", "a bystander at four blocks, the hunter at six");
        // It hides out to its widest flight.
        let wide = species(&format!(
            "(creatures: [(name: \"quail\", health: 2, {BODY}, movement: (walk: 1.0, run: 5.0), flush: (hold: 2.5, burst: 2.0, seconds: 0.6), temperament: (fear: 0.9), behaviours: [(do: Flee(radius: 4.0)), (do: Flee(radius: 10.0))])])"
        ));
        let mut calm = Mind::new(&wide.def.temperament, 2);
        let widest = 10.0 * (0.5 + calm.personality.fear());
        let _ = think(&wide, &mut calm, &situation(&[player(Vec3::new(widest - 1.0, 0.0, 0.0))]));
        assert_eq!(doing(&wide, &calm), "hide", "{:.1} blocks off, inside its widest flight", widest - 1.0);
        // Beyond the distance it would flee at, it lives its day rather than sit tight.
        let mut calm = Mind::new(&bird.def.temperament, 2);
        let flees_at = 10.0 * (0.5 + calm.personality.fear());
        let _ = think(&bird, &mut calm, &situation(&[player(Vec3::new(flees_at + 1.0, 0.0, 0.0))]));
        assert_ne!(doing(&bird, &calm), "hide", "{:.1} blocks off, outside its flee distance", flees_at + 1.0);
        let mut kept = Mind::new(&bird.def.temperament, 2);
        let _ = think(&bird, &mut kept, &Situation { kept: true, ..situation(&far) });
        assert_eq!(doing(&bird, &kept), "flee", "a kept bird sits tight for no one");

        let build = |creature: &str| {
            let defs = CreatureRegistry::read("test", &format!("(creatures: [(name: \"c\", health: 2, {BODY}, {creature})])")).unwrap().defs;
            CreatureRegistry::new(defs, Default::default(), &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).map(|_| ()).map_err(|e| e.to_string())
        };
        assert_eq!(build("flush: (hold: 3.0, burst: 1.8, seconds: 1.0)"), Ok(()));
        for bad in ["flush: (hold: 0.5, burst: 1.8, seconds: 1.0)", "flush: (hold: 8.0, burst: 1.8, seconds: 1.0)", "flush: (hold: 3.0, burst: 3.0, seconds: 1.0)", "flush: (hold: 3.0, burst: 0.5, seconds: 1.0)", "flush: (hold: 3.0, burst: 1.8, seconds: 5.0)"] {
            assert!(build(bad).is_err_and(|e| e.contains("`flush` needs")), "{bad}");
        }
        assert!(build("flush: (hold: 3.0, burst: 1.8, seconds: 1.0), attack: (damage: (1, 3))").is_err_and(|e| e.contains("for prey")));
    }

    /// A scavenger (`creature::Scavenge`) slinks in, crouched, to food lying on the ground
    /// while no player is within its `shy` of it or the food, snatches a piece when close,
    /// and makes off home with it, eating it there. Watched, kept or hurt it lets it lie.
    /// Bad numbers, and takes that are no item, are refused.
    #[test]
    fn a_fox_steals_meat_left_lying() {
        let meat_blocks = || BlockRegistry::new(vec![crate::block::BlockDef { name: "test:venison".into(), tags: vec!["meat".into()], ..Default::default() }]).unwrap();
        let defs = CreatureRegistry::read("test", &format!(
            "(creatures: [(name: \"fox\", health: 6, {BODY}, movement: (walk: 1.5, run: 6.0), scavenge: (takes: [\"tag:meat\"], reach: 16.0, shy: 6.0), temperament: (fear: 0.3, curiosity: 0.8), behaviours: [(do: Wander(radius: 8.0))])])"
        ))
        .unwrap()
        .defs;
        let fox = CreatureRegistry::new(defs, Default::default(), &meat_blocks(), &SkillRegistry::default()).unwrap().get(CreatureId(0)).clone();
        let scrap = Vec3::new(10.0, 0.0, 0.0);
        fn at(players: &[Seen]) -> Situation<'_> {
            Situation { scrap: Some(Vec3::new(10.0, 0.0, 0.0)), ..situation(players) }
        }
        let mut mind = Mind::new(&fox.def.temperament, 2);
        let decision = think(&fox, &mut mind, &at(&[]));
        assert_eq!((doing(&fox, &mind), decision.goal, decision.pose, decision.take), ("scavenge", Some(scrap), Pose::Crouched, None));
        // A player by the meat, or by the fox, and it keeps off.
        for by in [Vec3::new(13.0, 0.0, 0.0), Vec3::new(-3.0, 0.0, 0.0)] {
            let mut wary = Mind::new(&fox.def.temperament, 2);
            let _ = think(&fox, &mut wary, &at(&[player(by)]));
            assert_ne!(doing(&fox, &wary), "scavenge", "a player at {by}");
        }
        // One further off, it comes all the same.
        let mut bold = Mind::new(&fox.def.temperament, 2);
        let _ = think(&fox, &mut bold, &at(&[player(Vec3::new(-8.0, 0.0, 0.0))]));
        assert_eq!(doing(&fox, &bold), "scavenge");
        // Up to it, it snatches a piece; carrying it, it runs home and eats it there.
        let decision = think(&fox, &mut mind, &Situation { position: scrap - Vec3::X, ..at(&[]) });
        assert_eq!((decision.take, decision.goal), (Some(scrap), None));
        let away = Vec3::new(9.0, 0.0, 0.0);
        let decision = think(&fox, &mut mind, &Situation { position: away, carrying: true, ..situation(&[]) });
        assert_eq!((doing(&fox, &mind), decision.goal, decision.speed > 5.0, decision.swallow), ("scavenge", Some(Vec3::ZERO), true, false));
        let decision = think(&fox, &mut mind, &Situation { position: Vec3::new(1.0, 0.0, 0.0), carrying: true, ..situation(&[]) });
        assert!(decision.swallow, "home, it eats it");
        let mut kept = Mind::new(&fox.def.temperament, 2);
        let _ = think(&fox, &mut kept, &Situation { kept: true, ..at(&[]) });
        assert_ne!(doing(&fox, &kept), "scavenge", "a kept fox lets food lie");
        let mut hurt = Mind::new(&fox.def.temperament, 2);
        hurt.hurt(&fox, Some(1), 0.9);
        let _ = think(&fox, &mut hurt, &at(&[player(Vec3::new(-12.0, 0.0, 0.0))]));
        assert_ne!(doing(&fox, &hurt), "scavenge", "hurt, it thinks of nothing but its hide");

        let build = |creature: &str| {
            let defs = CreatureRegistry::read("test", &format!("(creatures: [(name: \"c\", health: 2, {BODY}, {creature})])")).unwrap().defs;
            CreatureRegistry::new(defs, Default::default(), &meat_blocks(), &SkillRegistry::default()).map(|_| ()).map_err(|e| e.to_string())
        };
        assert_eq!(build("scavenge: (takes: [\"tag:meat\"], reach: 16.0, shy: 6.0)"), Ok(()));
        for bad in ["(takes: [], reach: 16.0, shy: 6.0)", "(takes: [\"tag:meat\"], reach: 30.0, shy: 6.0)", "(takes: [\"tag:meat\"], reach: 16.0, shy: 1.0)", "(takes: [\"tag:meat\"], reach: 6.0, shy: 8.0)", "(takes: [\"tag:meat\"], reach: 16.0, shy: 6.0, give_up: 2.0)", "(takes: [\"tag:meat\"], reach: 16.0, shy: 6.0, give_up: 600.0)"] {
            assert!(build(&format!("scavenge: {bad}")).is_err_and(|e| e.contains("`scavenge` needs")), "{bad}");
        }
        assert!(build("scavenge: (takes: [\"tag:nothing\"], reach: 16.0, shy: 6.0)").is_err_and(|e| e.contains("nothing it `takes`")));
        assert!(build("scavenge: (takes: [\"stone\"], reach: 16.0, shy: 6.0)").is_err_and(|e| e.contains("does not exist")));
    }

    /// A creature that sleeps (`creature::Sleeps`) sleeps out the hours it is not about:
    /// eyes shut, it notices a player walking up only within its `quiet`, and then as ever
    /// (its double take, then whatever it does), so a careful hunter gets close. One going
    /// faster than a walk within its `wakes` wakes it with a start: a fighter comes up
    /// provoked, with no warning, and prey bolts. In its own hours it is awake as ever.
    #[test]
    fn a_sleeper_wakes_to_a_sprint_not_a_quiet_step() {
        let bear = species(&format!(
            "(creatures: [(name: \"bear\", health: 10, {BODY}, attack: (damage: (1, 2)), habitat: (active: Night), sleeps: (quiet: 3.0, wakes: 12.0), temperament: (fear: 0.2), behaviours: [(do: Wander(radius: 6.0)), (do: Defend)])])"
        ));
        fn noon(players: &[Seen]) -> Situation<'_> {
            Situation { daylight: 1.0, ..situation(players) }
        }
        let quiet = |at: f32| player(Vec3::new(at, 0.0, 0.0));
        let loud = |at: f32| Seen { loud: true, ..quiet(at) };
        // Asleep at noon: a quiet step at six blocks, in plain view, goes unnoticed.
        let mut mind = Mind::new(&bear.def.temperament, 2);
        for _ in 0..5 {
            let decision = think(&bear, &mut mind, &noon(&[quiet(6.0)]));
            assert_eq!((decision.pose, decision.goal, mind.noticed.len()), (Pose::Resting, None, 0), "asleep, a quiet step at six blocks");
        }
        // At two, within its quiet, it wakes as it would to anyone close: no grudge.
        let _ = think(&bear, &mut mind, &noon(&[quiet(2.0)]));
        assert_eq!((mind.noticed.len(), mind.anger, mind.attacker()), (1, 0.0, None), "woken quietly");
        // A sprint at ten blocks wakes it with a start, and it comes up fighting.
        let mut startled = Mind::new(&bear.def.temperament, 2);
        let _ = think(&bear, &mut startled, &noon(&[loud(10.0)]));
        assert_eq!((startled.anger, startled.attacker()), (1.0, Some(1)), "woken by a sprint");
        // Beyond its wakes, a sprint is nothing to it.
        let mut far = Mind::new(&bear.def.temperament, 2);
        let _ = think(&bear, &mut far, &noon(&[loud(14.0)]));
        assert_eq!(far.noticed.len(), 0, "a sprint at fourteen blocks");
        // At night, its own hours, it sees a quiet step at six as ever.
        let mut awake = Mind::new(&bear.def.temperament, 2);
        let _ = think(&bear, &mut awake, &Situation { daylight: 0.0, ..situation(&[quiet(6.0)]) });
        assert_eq!(awake.noticed.len(), 1, "awake at night");
        // Prey woken with a start bolts.
        let deer = species(&format!(
            "(creatures: [(name: \"deer\", health: 6, {BODY}, habitat: (active: Night), sleeps: (quiet: 3.0, wakes: 12.0), temperament: (fear: 0.9), behaviours: [(do: Flee(radius: 4.0))])])"
        ));
        let mut bolting = Mind::new(&deer.def.temperament, 2);
        let _ = think(&deer, &mut bolting, &noon(&[loud(10.0)]));
        assert_eq!(doing(&deer, &bolting), "flee", "prey woken by a sprint");

        let build = |creature: &str| {
            let defs = CreatureRegistry::read("test", &format!("(creatures: [(name: \"c\", health: 2, {BODY}, {creature})])")).unwrap().defs;
            CreatureRegistry::new(defs, Default::default(), &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).map(|_| ()).map_err(|e| e.to_string())
        };
        assert_eq!(build("sleeps: (quiet: 3.0, wakes: 12.0)"), Ok(()));
        for bad in ["(quiet: 0.5, wakes: 12.0)", "(quiet: 8.0, wakes: 12.0)", "(quiet: 3.0, wakes: 30.0)", "(quiet: 3.0, wakes: 2.5)"] {
            assert!(build(&format!("sleeps: {bad}")).is_err_and(|e| e.contains("`sleeps` needs")), "{bad}");
        }
        assert!(build("sleeps: (quiet: 3.0, wakes: 12.0), habitat: (active: Any)").is_err_and(|e| e.contains("never sleeps")));
    }

    #[test]
    fn fire_out_of_range_is_refused() {
        let build = |creature: &str| {
            let defs = CreatureRegistry::read("test", &format!("(creatures: [(name: \"c\", health: 2, {BODY}, {creature})])")).unwrap().defs;
            CreatureRegistry::new(defs, Default::default(), &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).map(|_| ()).map_err(|e| e.to_string())
        };
        assert_eq!(build("fire: (fear: 0.8, give: 4.0)"), Ok(()));
        assert_eq!(build("fire: (fear: 0.0, give: 2.0, ward: false)"), Ok(()));
        for (creature, refused) in [
            ("boss: true, attack: (damage: (1, 2)), fire: (fear: 0.8, give: 4.0)", "keeps neither back"),
            ("supernatural: true, fire: (fear: 0.8, give: 4.0)", "keeps neither back"),
            ("fire: (fear: 1.5, give: 4.0)", "fear 0 to 1"),
            ("fire: (fear: -0.1, give: 4.0)", "fear 0 to 1"),
            ("fire: (fear: 0.8, give: 1.0)", "give 2 to 8"),
            ("fire: (fear: 0.8, give: 12.0)", "give 2 to 8"),
        ] {
            let built = build(creature);
            assert!(built.as_ref().is_err_and(|e| e.contains(refused)), "{creature}: {built:?}");
        }
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
