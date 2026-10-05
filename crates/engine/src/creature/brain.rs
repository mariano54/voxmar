//! A creature's mind: drives, utility scoring of its behaviours, and what the chosen one
//! asks of the body. Pure functions over a snapshot of what the creature senses, so the
//! server, tests and later WASM behaviours run the same decisions (docs/creature-ai.md).
//!
//! Every few ticks the server gathers a [`Situation`] (players and kin nearby, food
//! within reach, daylight, home) and calls [`think`]. Each behaviour the species has
//! scores itself from the drives (hunger, fear, anger), the temperament, the situation and
//! its weight in the species' data; the best wins, with a margin for the one already
//! running so creatures don't dither. The winner turns into a [`Decision`]: where to go,
//! how fast, what to face, whom to attack, what to eat. Randomness (where to wander, how
//! long to pause) comes from a seed the server derives from the world, the creature and
//! the tick, so a replay repeats every choice.

use glam::{IVec3, Vec3};
use serde::{Deserialize, Serialize};

use super::{Behaviour, Creature, Style, Temperament, mix, unit_from};

/// Hunger a creature gains per second: from fed to hungry in about four minutes.
const HUNGER_RATE: f32 = 1.0 / 240.0;
/// Fear and anger fade per second.
const FEAR_FADE: f32 = 0.12;
const ANGER_FADE: f32 = 1.0 / 45.0;
/// The running behaviour's score is raised by this, so a close rival doesn't flip it.
const HYSTERESIS: f32 = 0.15;
/// Blocks from a goal that count as there.
const ARRIVED: f32 = 0.8;

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
}

/// The behaviour a creature is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Doing {
    #[default]
    Idle,
    Wander,
    Graze,
    Flee,
    Freeze,
    Herd,
    Tempted,
    Hunt,
    Defend,
    Curious,
    Return,
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
}

/// What a creature senses this decision.
#[derive(Clone, Copy, Debug)]
pub struct Situation<'a> {
    pub position: Vec3,
    pub home: Vec3,
    /// Its health as a share of its most.
    pub health: f32,
    /// 0 at night .. 1 by day.
    pub daylight: f32,
    /// Players it can sense, nearest first.
    pub players: &'a [Seen],
    /// Where others of its kind are, nearby.
    pub kin: &'a [Vec3],
    /// The nearest block of its food, if any is near.
    pub food: Option<IVec3>,
    /// Seconds since the last decision.
    pub dt: f32,
    /// The seed for this decision's random choices.
    pub seed: u64,
}

/// A creature's state of mind, kept between decisions.
#[derive(Clone, Copy, Debug)]
pub struct Mind {
    /// Its own temper: the species' with a seeded twist (docs/creature-ai.md, "Individual
    /// personality").
    pub temper: Temperament,
    pub hunger: f32,
    pub fear: f32,
    pub anger: f32,
    /// The player it is angry with, or fleeing.
    pub foe: Option<u32>,
    pub doing: Doing,
    /// Seconds in the current behaviour.
    pub since: f32,
    /// Where the current behaviour is heading.
    pub goal: Option<Vec3>,
    /// A pause, in seconds, before wandering on.
    pub pause: f32,
    /// Seconds until it may attack again.
    pub cooldown: f32,
    /// Zigzag side when fleeing.
    pub zig: f32,
}

impl Mind {
    /// A new mind for an individual whose identity hashes to `seed`.
    pub fn new(species: &Temperament, seed: u64) -> Self {
        let twist = |salt: u64, value: f32| (value * (0.85 + 0.3 * unit_from(mix(seed, salt)))).clamp(0.0, 1.0);
        let temper = Temperament {
            aggression: twist(1, species.aggression),
            fear: twist(2, species.fear),
            curiosity: twist(3, species.curiosity),
            sociability: twist(4, species.sociability),
            territory: twist(5, species.territory),
        };
        Self {
            temper,
            hunger: unit_from(mix(seed, 6)) * 0.6,
            fear: 0.0,
            anger: 0.0,
            foe: None,
            doing: Doing::Idle,
            since: 0.0,
            goal: None,
            pause: 0.0,
            cooldown: 0.0,
            zig: 1.0,
        }
    }

    /// It was hurt by `by` (a player's id, if a player): it fears, and if it fights back,
    /// angers.
    pub fn hurt(&mut self, creature: &Creature, by: Option<u32>, health: f32) {
        let fights = creature.behaviours.iter().any(|b| matches!(b.behaviour, Behaviour::Defend | Behaviour::Hunt { .. }));
        if fights && health > 0.25 {
            self.anger = 1.0;
        } else {
            self.fear = 1.0;
        }
        if by.is_some() {
            self.foe = by;
        }
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
    pub pose: Pose,
}

/// Decides what a creature does now.
pub fn think(creature: &Creature, mind: &mut Mind, at: &Situation) -> Decision {
    let def = &creature.def;
    let dt = at.dt;
    mind.hunger = (mind.hunger + HUNGER_RATE * dt).min(1.0);
    mind.fear = (mind.fear - FEAR_FADE * dt).max(0.0);
    mind.anger = (mind.anger - ANGER_FADE * dt).max(0.0);
    mind.cooldown = (mind.cooldown - dt).max(0.0);
    mind.pause = (mind.pause - dt).max(0.0);
    mind.since += dt;
    if mind.anger == 0.0 && mind.fear == 0.0 {
        mind.foe = None;
    }

    let lively = def.habitat.active.liveliness(at.daylight);
    let from_home = at.position.distance(at.home);
    let nearest = at.players.first();
    let foe = mind.foe.and_then(|id| at.players.iter().find(|p| p.id == id));
    let distance = |p: &Seen| p.position.distance(at.position);
    // Night hunters see further at night, day animals less.
    let sight = def.senses.sight * if def.habitat.active == super::Active::Night { 1.0 } else { 0.6 + 0.4 * at.daylight };

    let mut best = (Doing::Idle, 0.12 * (1.2 - lively), None::<Seen>);
    let mut consider = |doing: Doing, score: f32, who: Option<Seen>| {
        let score = if doing == mind.doing { score + HYSTERESIS } else { score };
        if score > best.1 {
            best = (doing, score, who);
        }
    };
    for entry in &creature.behaviours {
        let w = entry.weight;
        match entry.behaviour {
            Behaviour::Wander { .. } => consider(Doing::Wander, w * 0.3 * lively, None),
            Behaviour::Graze => {
                if at.food.is_some() {
                    consider(Doing::Graze, w * 1.3 * mind.hunger * mind.hunger, None);
                }
            }
            Behaviour::Flee { radius } => {
                // A player holding its favourite food is no threat unless it was hurt.
                let threat = foe.or_else(|| nearest.filter(|p| !p.tempting && distance(p) < radius * (0.5 + mind.temper.fear)));
                if let Some(threat) = threat {
                    let closeness = 1.0 - (distance(&threat) / (radius * 1.5)).min(1.0);
                    consider(Doing::Flee, w * 3.0 * mind.fear.max(closeness * mind.temper.fear), Some(threat));
                } else if mind.fear > 0.0 && foe.is_none() {
                    consider(Doing::Flee, w * 0.5 * mind.fear, None);
                }
            }
            Behaviour::Freeze => {
                if let Some(p) = nearest.filter(|p| !p.tempting && mind.fear < 0.3) {
                    let d = distance(p);
                    let radius = 6.0 + 10.0 * mind.temper.fear;
                    if d > radius * 0.6 && d < radius * 1.2 && mind.since < 4.0 || (mind.doing == Doing::Freeze && mind.since < 2.5 && d < radius * 1.2) {
                        consider(Doing::Freeze, w * 1.1 * mind.temper.fear, Some(*p));
                    }
                }
            }
            Behaviour::Herd { spacing } => {
                if !at.kin.is_empty() {
                    let centre = at.kin.iter().copied().sum::<Vec3>() / at.kin.len() as f32;
                    let apart = (centre.distance(at.position) / (spacing * 3.0)).clamp(0.0, 1.5);
                    consider(Doing::Herd, w * 0.6 * mind.temper.sociability * apart, None);
                }
            }
            Behaviour::Tempted { radius } => {
                if let Some(p) = at.players.iter().find(|p| p.tempting && distance(p) < radius) {
                    consider(Doing::Tempted, w * 2.0, Some(*p));
                }
            }
            Behaviour::Hunt { radius } => {
                let radius = radius * (0.6 + 0.4 * lively) * (0.7 + 0.6 * mind.hunger);
                // RuneScape: monsters leave alone players more than twice their level.
                let prey = at.players.iter().find(|p| {
                    !p.creative && distance(p) < radius.min(sight) && p.combat_level as u16 <= def.level as u16 * 2 + 1
                });
                if let Some(p) = foe.filter(|_| mind.anger > 0.0).or(prey) {
                    consider(Doing::Hunt, w * 2.5 * mind.temper.aggression.max(mind.anger), Some(*p));
                }
            }
            Behaviour::Defend => {
                if let Some(p) = foe.filter(|_| mind.anger > 0.0) {
                    consider(Doing::Defend, w * 3.5 * mind.anger, Some(*p));
                }
            }
            Behaviour::Curious { distance: keep } => {
                if let Some(p) = nearest.filter(|p| distance(p) > keep && distance(p) < sight && mind.fear < 0.2) {
                    consider(Doing::Curious, w * 0.5 * mind.temper.curiosity, Some(*p));
                }
            }
            Behaviour::Blink => {}
        }
    }
    if from_home > def.leash {
        consider(Doing::Return, 4.0 * ((from_home - def.leash) / def.leash).min(1.0), None);
    }

    let (doing, _, who) = best;
    if doing != mind.doing {
        mind.doing = doing;
        mind.since = 0.0;
        mind.goal = None;
    }
    act(creature, mind, at, who)
}

/// What the chosen behaviour asks for.
fn act(creature: &Creature, mind: &mut Mind, at: &Situation, who: Option<Seen>) -> Decision {
    let def = &creature.def;
    let movement = &def.movement;
    let walk = movement.walk;
    let run = movement.run;
    let random = |salt: u64| unit_from(mix(at.seed, salt));
    let mut decision = Decision::default();
    match mind.doing {
        Doing::Idle => {
            decision.pose = if def.habitat.active.liveliness(at.daylight) < 0.4 { Pose::Resting } else { Pose::Idle };
            if random(1) < 0.15 {
                let angle = random(2) * std::f32::consts::TAU;
                decision.face = Some(at.position + Vec3::new(angle.cos(), 0.0, angle.sin()));
            }
        }
        Doing::Wander => {
            let radius = creature.behaviours.iter().find_map(|b| match b.behaviour {
                Behaviour::Wander { radius } => Some(radius),
                _ => None,
            });
            let arrived = mind.goal.is_none_or(|goal| goal.with_y(0.0).distance(at.position.with_y(0.0)) < ARRIVED);
            if arrived && mind.pause == 0.0 {
                if mind.goal.is_some() {
                    // Stand a while where it got to: long pauses for grazers and stalkers.
                    mind.pause = 1.0 + random(3) * if movement.style == Style::Stalker { 8.0 } else { 5.0 };
                    mind.goal = None;
                } else {
                    let angle = random(4) * std::f32::consts::TAU;
                    let reach = radius.unwrap_or(8.0) * (0.3 + 0.7 * random(5));
                    // Wander about home, drifting back toward it.
                    let base = at.position.lerp(at.home, 0.3);
                    mind.goal = Some(base + Vec3::new(angle.cos(), 0.0, angle.sin()) * reach);
                }
            }
            if mind.pause == 0.0 {
                decision.goal = mind.goal;
                decision.speed = walk;
            }
        }
        Doing::Graze => {
            let food = at.food.map(|b| b.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            if let Some(food) = food {
                if food.with_y(0.0).distance(at.position.with_y(0.0)) < def.body.size.0 * 0.5 + 0.9 {
                    decision.pose = Pose::Grazing;
                    decision.face = Some(food);
                    if mind.since > 2.5 {
                        decision.eat = at.food;
                        mind.hunger = 0.0;
                    }
                } else {
                    decision.goal = Some(food);
                    decision.speed = walk;
                    mind.since = 0.0;
                }
            }
        }
        Doing::Flee => {
            let from = who.map_or(at.home, |p| p.position);
            let mut away = (at.position - from).with_y(0.0).normalize_or_zero();
            if away == Vec3::ZERO {
                away = Vec3::X;
            }
            // Hoppers zigzag, switching side every second or so.
            if movement.style == Style::Hopper && (mind.since * 1.2).fract() < at.dt * 1.2 {
                mind.zig = -mind.zig;
            }
            let side = Vec3::new(-away.z, 0.0, away.x) * if movement.style == Style::Hopper { 0.6 * mind.zig } else { 0.0 };
            decision.goal = Some(at.position + (away + side).normalize() * 8.0);
            decision.speed = run;
            decision.pose = Pose::Alert;
            let blinks = creature.behaviours.iter().any(|b| b.behaviour == Behaviour::Blink);
            if blinks && who.is_some_and(|p| p.position.distance(at.position) < 4.0) && random(6) < 0.25 {
                decision.blink = Some(at.position + away * 6.0);
            }
        }
        Doing::Freeze => {
            decision.face = who.map(|p| p.position);
            decision.pose = Pose::Alert;
        }
        Doing::Herd => {
            if !at.kin.is_empty() {
                let centre = at.kin.iter().copied().sum::<Vec3>() / at.kin.len() as f32;
                decision.goal = Some(centre);
                decision.speed = walk;
            }
        }
        Doing::Tempted | Doing::Curious => {
            if let Some(p) = who {
                let keep = if mind.doing == Doing::Tempted {
                    1.6 + def.body.size.0
                } else {
                    creature.behaviours.iter().find_map(|b| match b.behaviour {
                        Behaviour::Curious { distance } => Some(distance),
                        _ => None,
                    }).unwrap_or(5.0)
                };
                let d = p.position.distance(at.position);
                if d > keep {
                    decision.goal = Some(p.position);
                    decision.speed = if mind.doing == Doing::Tempted { walk * 1.2 } else { walk * 0.8 };
                }
                decision.face = Some(p.position);
                decision.pose = Pose::Alert;
            }
        }
        Doing::Hunt | Doing::Defend => {
            if let Some(p) = who {
                let reach = def.attack.map_or(1.0, |a| a.reach) + def.body.size.0 * 0.5 + 0.3;
                let d = p.position.with_y(0.0).distance(at.position.with_y(0.0));
                let close = d <= reach && (p.position.y - at.position.y).abs() < def.body.size.1 + 1.5;
                // Stalkers creep until close, then rush.
                let speed = if movement.style == Style::Stalker && d > 6.0 { walk * 0.6 } else { run };
                if !close {
                    decision.goal = Some(p.position);
                    decision.speed = speed;
                }
                decision.face = Some(p.position);
                if close && mind.cooldown == 0.0 && def.attack.is_some() {
                    decision.attack = Some(p.id);
                    decision.pose = Pose::Attacking;
                    mind.cooldown = def.attack.map_or(1.0, |a| a.every);
                }
            }
        }
        Doing::Return => {
            decision.goal = Some(at.home);
            decision.speed = walk;
        }
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockRegistry;
    use crate::creature::CreatureRegistry;
    use crate::skill::SkillRegistry;

    fn species(source: &str) -> Creature {
        let defs = CreatureRegistry::read("test", source).unwrap();
        CreatureRegistry::new(defs, &BlockRegistry::new(vec![]).unwrap(), &SkillRegistry::default()).unwrap().get(super::super::CreatureId(0)).clone()
    }

    const BODY: &str = "body: (size: (0.6, 0.6), model: Blob(body: (0.5, 0.5, 0.5)), colours: [(1, 2, 3)])";

    fn situation<'a>(players: &'a [Seen]) -> Situation<'a> {
        Situation { position: Vec3::ZERO, home: Vec3::ZERO, health: 1.0, daylight: 1.0, players, kin: &[], food: None, dt: 0.2, seed: 7 }
    }

    fn player(at: Vec3) -> Seen {
        Seen { id: 1, position: at, tempting: false, combat_level: 3, creative: false }
    }

    #[test]
    fn timid_creatures_flee_and_tempted_ones_follow() {
        let rabbit = species(&format!("(creatures: [(name: \"rabbit\", health: 3, {BODY}, temperament: (fear: 0.9), diet: (favourite: []), behaviours: [(do: Wander(radius: 6.0)), (do: Flee(radius: 10.0)), (do: Tempted(radius: 10.0))])])"));
        let mut mind = Mind::new(&rabbit.def.temperament, 1);
        let near = [player(Vec3::new(3.0, 0.0, 0.0))];
        let decision = think(&rabbit, &mut mind, &situation(&near));
        assert_eq!(mind.doing, Doing::Flee);
        assert!(decision.goal.unwrap().x < -1.0, "runs away from the player: {:?}", decision.goal);
        let mut mind = Mind::new(&rabbit.def.temperament, 1);
        let carrot = [Seen { tempting: true, ..player(Vec3::new(5.0, 0.0, 0.0)) }];
        let decision = think(&rabbit, &mut mind, &situation(&carrot));
        assert_eq!(mind.doing, Doing::Tempted);
        assert!(decision.goal.unwrap().x > 4.0);
    }

    #[test]
    fn hunters_attack_in_reach_but_leave_strong_players_alone() {
        let wolf = species(&format!(
            "(creatures: [(name: \"wolf\", health: 10, level: 5, {BODY}, attack: (damage: (1, 3)), temperament: (aggression: 0.9), behaviours: [(do: Hunt(radius: 16.0))])])"
        ));
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let near = [player(Vec3::new(1.0, 0.0, 0.0))];
        let decision = think(&wolf, &mut mind, &situation(&near));
        assert_eq!((mind.doing, decision.attack), (Doing::Hunt, Some(1)));
        // Twice its level and one more is the limit.
        let mut mind = Mind::new(&wolf.def.temperament, 2);
        let strong = [Seen { combat_level: 12, ..player(Vec3::new(1.0, 0.0, 0.0)) }];
        let _ = think(&wolf, &mut mind, &situation(&strong));
        assert_ne!(mind.doing, Doing::Hunt);
        // Unless it is angry with them.
        mind.hurt(&wolf, Some(1), 1.0);
        let _ = think(&wolf, &mut mind, &situation(&strong));
        assert_eq!(mind.doing, Doing::Hunt);
    }

    #[test]
    fn neutral_creatures_fight_back_only_when_hurt() {
        let boar = species(&format!(
            "(creatures: [(name: \"boar\", health: 10, {BODY}, attack: (damage: (1, 2)), temperament: (fear: 0.2), behaviours: [(do: Wander(radius: 6.0)), (do: Defend)])])"
        ));
        let mut mind = Mind::new(&boar.def.temperament, 3);
        let near = [player(Vec3::new(1.0, 0.0, 0.0))];
        let _ = think(&boar, &mut mind, &situation(&near));
        assert_ne!(mind.doing, Doing::Defend);
        mind.hurt(&boar, Some(1), 0.9);
        let decision = think(&boar, &mut mind, &situation(&near));
        assert_eq!((mind.doing, decision.attack), (Doing::Defend, Some(1)));
    }

    #[test]
    fn creatures_go_home_when_far() {
        let snail = species(&format!("(creatures: [(name: \"snail\", health: 2, leash: 8.0, {BODY}, behaviours: [(do: Wander(radius: 4.0))])])"));
        let mut mind = Mind::new(&snail.def.temperament, 4);
        let far = Situation { position: Vec3::new(20.0, 0.0, 0.0), ..situation(&[]) };
        let decision = think(&snail, &mut mind, &far);
        assert_eq!((mind.doing, decision.goal), (Doing::Return, Some(Vec3::ZERO)));
    }
}
