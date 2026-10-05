//! The engine's behaviour module library. Each module is a small behaviour with its own
//! considerations: what it reads to decide how much it wants to run, and what it then
//! asks of the body. A species lists the ones it has (`behaviours` in creatures.ron) with
//! a weight, or gets a set made from its temperament and diet (`creature::generated`).
//!
//! Ideas taken, as techniques only (credited in INSPIRATIONS.md; research/creature-ai.html):
//! Minecraft's random stroll and look-around, its fox's crouch and pounce and its
//! panic-when-hurt; RuneScape's level-gated aggression and tolerance; Rain World's
//! nervous lizards that freeze and watch; Untamed Wilds' herds and sentinels; Ark's
//! skittish and patient temperaments.

use std::sync::Arc;

use glam::Vec3;

use super::{Band, Ctx, Decision, Memory, Mind, Module, Pose, Seen, Want};
use crate::creature::{Behaviour, Style};

/// Blocks from a goal that count as there.
const ARRIVED: f32 = 0.8;

/// The module that runs a behaviour from data.
pub fn module(behaviour: Behaviour) -> Arc<dyn Module> {
    match behaviour {
        Behaviour::Wander { radius } => Arc::new(Wander { radius }),
        Behaviour::LookAround => Arc::new(LookAround),
        Behaviour::StandToLook => Arc::new(StandToLook),
        Behaviour::Graze => Arc::new(Graze),
        Behaviour::Flee { radius } => Arc::new(Flee { radius }),
        Behaviour::Freeze => Arc::new(Freeze),
        Behaviour::Blink => Arc::new(Blink),
        Behaviour::Herd { spacing } => Arc::new(Herd { spacing }),
        Behaviour::Sentinel => Arc::new(Sentinel),
        Behaviour::Tempted { radius } => Arc::new(Tempted { radius }),
        Behaviour::Hunt { radius } => Arc::new(Hunt { radius }),
        Behaviour::Defend => Arc::new(Defend),
        Behaviour::Curious { distance } => Arc::new(Curious { distance }),
    }
}

/// Every creature has these, whatever its data says: going home when it strays.
pub fn always() -> Vec<Arc<dyn Module>> {
    vec![Arc::new(ReturnHome)]
}

/// How a creature with nothing to do looks: resting outside its active hours.
pub fn resting_pose(ctx: &Ctx) -> Pose {
    if ctx.lively < 0.4 { Pose::Resting } else { Pose::Idle }
}

fn walk(ctx: &Ctx) -> f32 {
    ctx.creature.def.movement.walk
}

fn run(ctx: &Ctx) -> f32 {
    ctx.creature.def.movement.run
}

fn flat(a: Vec3, b: Vec3) -> f32 {
    a.with_y(0.0).distance(b.with_y(0.0))
}

/// Strolls about home, with pauses whose length follows its energy and style.
#[derive(Debug)]
struct Wander {
    radius: f32,
}

impl Module for Wander {
    fn name(&self) -> &'static str {
        "wander"
    }

    fn band(&self) -> Band {
        Band::Idle
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        Want::new(0.3 * ctx.lively * (0.6 + 0.8 * ctx.mind.personality.energy))
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let at = ctx.at;
        let arrived = mind.goal.is_none_or(|goal| flat(goal, at.position) < ARRIVED);
        if arrived && mind.pause == 0.0 {
            if mind.goal.take().is_some() {
                let style = ctx.creature.def.movement.style;
                let longest = match style {
                    Style::Stalker | Style::Lumberer => 10.0,
                    Style::Creeper => 3.0,
                    _ => 6.0,
                };
                mind.pause = 0.5 + ctx.random(3) * longest * (1.4 - ctx.mind.personality.energy);
            } else {
                let angle = ctx.random(4) * std::f32::consts::TAU;
                let reach = self.radius * (0.3 + 0.7 * ctx.random(5));
                // About home, drifting back toward it.
                let base = at.position.lerp(at.home, 0.3);
                mind.goal = Some(base + Vec3::new(angle.cos(), 0.0, angle.sin()) * reach);
            }
        }
        if mind.pause > 0.0 {
            return Decision { pose: resting_pose(ctx), ..Default::default() };
        }
        Decision { goal: mind.goal, speed: walk(ctx), ..Default::default() }
    }
}

/// Stands and looks about.
#[derive(Debug)]
struct LookAround;

impl Module for LookAround {
    fn name(&self) -> &'static str {
        "look_around"
    }

    fn band(&self) -> Band {
        Band::Idle
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        // Wanted less the longer it has looked.
        let tired = if ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == "look_around") { ctx.mind.since / 6.0 } else { 0.0 };
        Want::new(0.3 * (0.6 + 0.8 * ctx.mind.personality.energy) * ctx.lively * (1.0 - tired).max(0.0))
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let angle = ctx.random(11) * std::f32::consts::TAU;
        let face = (ctx.random(12) < 0.3).then(|| ctx.at.position + Vec3::new(angle.cos(), 0.0, angle.sin()));
        Decision { face, pose: resting_pose(ctx), ..Default::default() }
    }
}

/// Rears up or lifts its head to look at a player nearby: nervous ones more.
#[derive(Debug)]
struct StandToLook;

impl Module for StandToLook {
    fn name(&self) -> &'static str {
        "stand_to_look"
    }

    fn band(&self) -> Band {
        Band::Social
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = ctx.nearest()?;
        let d = ctx.distance(p.position);
        if d > ctx.sight || ctx.mind.since > 4.0 && ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name()) {
            return None;
        }
        Want::at(0.4 * ctx.mind.personality.nervousness * (1.0 - d / ctx.sight), *p)
    }

    fn act(&self, _: &Ctx, _: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        Decision { face: target.map(|p| p.position), pose: Pose::Alert, ..Default::default() }
    }
}

/// Goes to its food and eats, head down.
#[derive(Debug)]
struct Graze;

impl Module for Graze {
    fn name(&self) -> &'static str {
        "graze"
    }

    fn band(&self) -> Band {
        Band::Drive
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        ctx.at.food?;
        Want::new(1.3 * ctx.mind.hunger * ctx.mind.hunger)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let Some(block) = ctx.at.food else { return Decision::default() };
        let food = block.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        if flat(food, ctx.at.position) < ctx.creature.def.body.size.0 * 0.5 + 0.9 {
            let mut decision = Decision { face: Some(food), pose: Pose::Grazing, ..Default::default() };
            if mind.since > 2.0 + 2.0 * ctx.random(21) {
                decision.eat = Some(block);
                mind.hunger = 0.0;
            }
            decision
        } else {
            mind.since = 0.0;
            Decision { goal: Some(food), speed: walk(ctx), ..Default::default() }
        }
    }
}

/// Runs from a threat: a player who hurt it, or one who comes too close. Hoppers
/// zigzag. A player holding its favourite food is no threat unless it was hurt.
#[derive(Debug)]
struct Flee {
    radius: f32,
}

impl Flee {
    fn threat(&self, ctx: &Ctx) -> Option<Seen> {
        let radius = self.radius * (0.5 + ctx.mind.personality.fear());
        ctx.attacker()
            .filter(|_| ctx.mind.fear > 0.0)
            .or_else(|| ctx.nearest().filter(|p| !p.tempting && ctx.distance(p.position) < radius))
            .copied()
    }
}

impl Module for Flee {
    fn name(&self) -> &'static str {
        "flee"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        match self.threat(ctx) {
            Some(threat) => {
                let closeness = 1.0 - (ctx.distance(threat.position) / (self.radius * 1.5)).min(1.0);
                Want::at(3.0 * ctx.mind.fear.max(closeness * ctx.mind.personality.fear()), threat)
            }
            // Alarmed by kin, it runs from wherever it is toward home.
            None if ctx.mind.fear > 0.3 && ctx.mind.recalls(Memory::Alarm) => Want::new(ctx.mind.fear),
            None => None,
        }
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, fresh: bool) -> Decision {
        let at = ctx.at;
        let from = target.map_or(at.position * 2.0 - at.home, |p| p.position);
        let mut away = (at.position - from).with_y(0.0).normalize_or_zero();
        if away == Vec3::ZERO {
            away = Vec3::X;
        }
        let style = ctx.creature.def.movement.style;
        if style == Style::Hopper && (fresh || ctx.random(31) < at.dt * 1.5) {
            mind.zig = -mind.zig;
        }
        let side = Vec3::new(-away.z, 0.0, away.x) * if style == Style::Hopper { 0.7 * mind.zig } else { 0.0 };
        if let Some(p) = target {
            mind.remember(Memory::ScaredBy(p.id), 10.0);
        }
        Decision { goal: Some(at.position + (away + side).normalize() * 8.0), speed: run(ctx), pose: Pose::Alert, alarm: fresh, ..Default::default() }
    }
}

/// Holds still at a threat just beyond fleeing distance, watching it, before deciding.
#[derive(Debug)]
struct Freeze;

impl Module for Freeze {
    fn name(&self) -> &'static str {
        "freeze"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = ctx.nearest().filter(|p| !p.tempting && ctx.mind.fear < 0.3)?;
        let d = ctx.distance(p.position);
        let radius = 6.0 + 10.0 * ctx.mind.personality.fear();
        let running = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name());
        let patience = 1.5 + 3.0 * ctx.mind.personality.nervousness;
        if d < radius * 0.6 || d > radius * 1.2 || (running && ctx.mind.since > patience) {
            return None;
        }
        Want::at(1.2 * ctx.mind.personality.nervousness.max(ctx.mind.personality.fear()), *p)
    }

    fn act(&self, _: &Ctx, _: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        Decision { face: target.map(|p| p.position), pose: Pose::Alert, ..Default::default() }
    }
}

/// Blinks a few blocks away when a threat is close: supernatural flight.
#[derive(Debug)]
struct Blink;

impl Module for Blink {
    fn name(&self) -> &'static str {
        "blink"
    }

    fn band(&self) -> Band {
        Band::Reflex
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = ctx.attacker().or_else(|| ctx.nearest().filter(|p| !p.tempting))?;
        if ctx.distance(p.position) > 3.0 || ctx.mind.cooldown > 0.0 {
            return None;
        }
        Want::at(1.0, *p)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        let from = target.map_or(ctx.at.home, |p| p.position);
        let away = (ctx.at.position - from).with_y(0.0).normalize_or(Vec3::X);
        let angle = (ctx.random(41) - 0.5) * 1.6;
        let (sin, cos) = angle.sin_cos();
        let turned = Vec3::new(away.x * cos - away.z * sin, 0.0, away.x * sin + away.z * cos);
        mind.cooldown = 4.0;
        mind.fear = mind.fear.max(0.8);
        Decision { blink: Some(ctx.at.position + turned * (5.0 + 3.0 * ctx.random(42))), pose: Pose::Alert, ..Default::default() }
    }
}

/// Keeps with others of its kind: the further from them, the more it wants to close up.
#[derive(Debug)]
struct Herd {
    spacing: f32,
}

impl Module for Herd {
    fn name(&self) -> &'static str {
        "herd"
    }

    fn band(&self) -> Band {
        Band::Social
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        if ctx.at.kin.is_empty() {
            return None;
        }
        let centre = ctx.at.kin.iter().copied().sum::<Vec3>() / ctx.at.kin.len() as f32;
        let spacing = self.spacing * (1.5 - ctx.mind.personality.sociability);
        let apart = ((flat(centre, ctx.at.position) - spacing) / (spacing * 2.0)).clamp(0.0, 1.5);
        Want::new(0.8 * ctx.mind.personality.sociability * apart)
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let centre = ctx.at.kin.iter().copied().sum::<Vec3>() / ctx.at.kin.len().max(1) as f32;
        Decision { goal: Some(centre), speed: walk(ctx), ..Default::default() }
    }
}

/// Stands watch while kin graze, head up, facing the nearest player.
#[derive(Debug)]
struct Sentinel;

impl Module for Sentinel {
    fn name(&self) -> &'static str {
        "sentinel"
    }

    fn band(&self) -> Band {
        Band::Social
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        if ctx.at.kin.len() < 2 || ctx.mind.hunger > 0.6 {
            return None;
        }
        let p = ctx.nearest().filter(|p| ctx.distance(p.position) < ctx.sight * 1.5)?;
        Want::at(0.35 * ctx.mind.personality.nervousness, *p)
    }

    fn act(&self, _: &Ctx, _: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        Decision { face: target.map(|p| p.position), pose: Pose::Alert, ..Default::default() }
    }
}

/// Follows a player holding its favourite food, as Minecraft's animals do.
#[derive(Debug)]
struct Tempted {
    radius: f32,
}

impl Module for Tempted {
    fn name(&self) -> &'static str {
        "tempted"
    }

    fn band(&self) -> Band {
        Band::Drive
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = ctx.at.players.iter().find(|p| p.tempting && ctx.distance(p.position) < self.radius)?;
        // A grudge outweighs a carrot.
        if ctx.mind.attacker() == Some(p.id) {
            return None;
        }
        Want::at(1.2 + ctx.mind.hunger, *p)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        let Some(p) = target else { return Decision::default() };
        mind.remember(Memory::FedBy(p.id), 20.0);
        let keep = 1.6 + ctx.creature.def.body.size.0;
        let goal = (ctx.distance(p.position) > keep).then_some(p.position);
        Decision { goal, speed: walk(ctx) * 1.2, face: Some(p.position), pose: Pose::Alert, ..Default::default() }
    }
}

/// Hunts players who come near: chases and attacks. Monsters leave alone players more
/// than twice their level, and players who have settled in one spot (RuneScape's rules);
/// anger overrides both. Stalkers creep in crouched, then pounce.
#[derive(Debug)]
struct Hunt {
    radius: f32,
}

impl Module for Hunt {
    fn name(&self) -> &'static str {
        "hunt"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let def = &ctx.creature.def;
        let personality = &ctx.mind.personality;
        if let Some(p) = ctx.attacker().filter(|_| ctx.mind.anger > 0.0) {
            return Want::at(2.5 * ctx.mind.anger.max(personality.aggression), *p);
        }
        let radius = self.radius * (0.6 + 0.4 * ctx.lively) * (0.7 + 0.6 * ctx.mind.hunger) * (0.7 + 0.6 * personality.aggression);
        let prey = ctx.at.players.iter().find(|p| {
            !p.creative && !p.tolerated && ctx.distance(p.position) < radius.min(ctx.sight) && p.combat_level as u16 <= def.level as u16 * 2 + 1
        })?;
        Want::at(2.5 * personality.aggression, *prey)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        strike(ctx, mind, target, true)
    }
}

/// Fights back when hurt (neutral creatures): the boar that charges, the goose that bites.
#[derive(Debug)]
struct Defend;

impl Module for Defend {
    fn name(&self) -> &'static str {
        "defend"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = ctx.attacker().filter(|_| ctx.mind.anger > 0.0)?;
        Want::at(3.5 * ctx.mind.anger, *p)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        strike(ctx, mind, target, false)
    }
}

/// Closes on a target and strikes when in reach; stalkers creep crouched and pounce.
fn strike(ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, stalks: bool) -> Decision {
    let Some(p) = target else { return Decision::default() };
    let def = &ctx.creature.def;
    let reach = def.attack.map_or(1.0, |a| a.reach) + def.body.size.0 * 0.5 + 0.3;
    let d = flat(p.position, ctx.at.position);
    let close = d <= reach && (p.position.y - ctx.at.position.y).abs() < def.body.size.1 + 1.5;
    let mut decision = Decision { face: Some(p.position), ..Default::default() };
    let stalker = stalks && def.movement.style == Style::Stalker;
    if !close {
        decision.goal = Some(p.position);
        if stalker && d > 4.5 {
            decision.speed = walk(ctx) * 0.6;
            decision.pose = Pose::Crouched;
        } else if stalker && mind.cooldown == 0.0 && d <= 4.5 {
            // The pounce: a leap at the target, then a pause to recover.
            decision.leap = Some(p.position);
            mind.cooldown = 1.5;
        } else {
            decision.speed = run(ctx);
        }
    } else if mind.cooldown == 0.0 && def.attack.is_some() {
        decision.attack = Some(p.id);
        decision.pose = Pose::Attacking;
        mind.cooldown = def.attack.map_or(1.0, |a| a.every);
    }
    decision
}

/// Comes to look at players from a distance, and backs off if they come closer.
#[derive(Debug)]
struct Curious {
    distance: f32,
}

impl Module for Curious {
    fn name(&self) -> &'static str {
        "curious"
    }

    fn band(&self) -> Band {
        Band::Social
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = ctx.nearest().filter(|p| ctx.distance(p.position) < ctx.sight && ctx.mind.fear < 0.2)?;
        Want::at(0.6 * ctx.mind.personality.curiosity, *p)
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        let Some(p) = target else { return Decision::default() };
        let d = ctx.distance(p.position);
        let keep = self.distance * (1.5 - ctx.mind.personality.bravery);
        let goal = if d > keep + 1.5 {
            Some(p.position)
        } else if d < keep * 0.6 {
            Some(ctx.at.position + (ctx.at.position - p.position).with_y(0.0).normalize_or(Vec3::X) * 3.0)
        } else {
            None
        };
        Decision { goal, speed: walk(ctx) * 0.8, face: Some(p.position), pose: Pose::Alert, ..Default::default() }
    }
}

/// Goes back toward home when it has strayed past its leash.
#[derive(Debug)]
struct ReturnHome;

impl Module for ReturnHome {
    fn name(&self) -> &'static str {
        "return_home"
    }

    fn band(&self) -> Band {
        Band::Drive
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let leash = ctx.creature.def.leash;
        let from_home = flat(ctx.at.position, ctx.at.home);
        // Once going, it keeps on until well inside.
        let going = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name());
        let limit = if going { leash * 0.5 } else { leash };
        (from_home > limit).then(|| Want::new(1.0 + 2.0 * ((from_home - leash) / leash).clamp(0.0, 1.0))).flatten()
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        Decision { goal: Some(ctx.at.home), speed: walk(ctx), ..Default::default() }
    }
}
