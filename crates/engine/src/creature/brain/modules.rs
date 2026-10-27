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
use crate::creature::{Behaviour, Creature, Style};

/// Blocks from a goal that count as there.
const ARRIVED: f32 = 0.8;
/// How far a player a boss is held off from moves before it comes on (`Mind::held_off`):
/// out of the hole, or out from under the roof.
pub const HELD_OFF_MOVED: f32 = 1.0;

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
        Behaviour::Routine { arrive, look } => Arc::new(Routine { arrive, look }),
    }
}

/// Every creature has these, whatever its data says: going home when it strays, going to
/// a mate when both are ready to breed, and warning players off if its kind does (`warn`).
pub fn always() -> Vec<Arc<dyn Module>> {
    vec![Arc::new(ReturnHome), Arc::new(Court), Arc::new(Roam), Arc::new(Warn), Arc::new(Fire), Arc::new(Hide), Arc::new(Scavenge)]
}

/// Now and then a curious, lively individual sets off alone across its range, so groups
/// split and drift instead of moving as one.
#[derive(Debug)]
struct Roam;

impl Module for Roam {
    fn name(&self) -> &'static str {
        "roam"
    }

    fn band(&self) -> Band {
        Band::Idle
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let p = &ctx.mind.personality;
        // Not while someone is with it: a player close by, one offering food, or one who
        // hurt it. An animal being fed, tamed or fought stays in the moment.
        let engaged = ctx.attacker().is_some()
            || ctx.at.players.iter().any(|p| ctx.distance(p.position) < 6.0 || (p.tempting && ctx.distance(p.position) < ctx.sight.max(16.0)));
        if engaged {
            return None;
        }
        let going = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name());
        // Most decisions it does not cross its mind; once going, it keeps on.
        if !going && ctx.random(92) > 0.12 {
            return None;
        }
        Want::new(0.5 * p.curiosity * (0.4 + p.energy) * ctx.lively * (1.2 - p.sociability * 0.6))
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, _: Option<Seen>, fresh: bool) -> Decision {
        if fresh || mind.goal.is_none_or(|g| flat(g, ctx.at.position) < ARRIVED) {
            let leash = if ctx.at.kept { super::super::husbandry::KEPT_ROAM } else { ctx.creature.def.leash };
            let angle = ctx.random(93) * std::f32::consts::TAU;
            let reach = leash * (0.35 + 0.45 * ctx.random(94));
            mind.goal = Some(ctx.at.home + Vec3::new(angle.cos(), 0.0, angle.sin()) * reach);
        }
        Decision { goal: mind.goal, speed: walk(ctx) * (0.9 + 0.3 * ctx.mind.personality.energy), ..Default::default() }
    }
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

/// Whether it is a bird, which flies where others walk far.
fn flier(ctx: &Ctx) -> bool {
    ctx.creature.def.movement.style == Style::Flier
}

/// A bird flies to a goal further than this many blocks off, and hops to a nearer one.
pub(super) const FLIES_PAST: f32 = 6.0;

/// A bird flies to a goal more than this many blocks above or below it, however near.
pub(super) const HOPS_UP: f32 = 1.5;

/// The speed a bird goes to `goal` at, wanting to go at `speed`: on the wing to anywhere
/// far, or further up or down than it hops (off a treetop it hopped up into, up a bank, home
/// across a valley), whatever has it go there; hopping, it would stand at the edge for good.
pub(super) fn on_the_wing(creature: &Creature, at: Vec3, goal: Vec3, speed: f32) -> f32 {
    let far = flat(goal, at) > FLIES_PAST || (goal.y - at.y).abs() > HOPS_UP;
    if creature.def.movement.style == Style::Flier && far { speed.max(creature.def.movement.run * 0.8) } else { speed }
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
                let radius = if at.kept { self.radius.min(super::super::husbandry::KEPT_ROAM) } else { self.radius };
                // Each individual strays as far as its curiosity takes it.
                let mut reach = radius * (0.6 + 0.9 * ctx.mind.personality.curiosity) * (0.3 + 0.7 * ctx.random(5));
                // A bird mostly hops a little way, and now and then flies off across its range.
                if flier(ctx) {
                    reach = if ctx.random(6) < 0.3 * (0.5 + ctx.mind.personality.energy) { (ctx.creature.def.leash * (0.4 + 0.5 * ctx.random(7))).max(FLIES_PAST * 1.5) } else { reach.min(3.0) };
                }
                // About home, drifting back toward it (a kept one more so).
                let mut base = at.position.lerp(at.home, if at.kept { 0.6 } else { 0.3 });
                // And toward its kin, as much as it cares for company: strolling off from the
                // herd only to herd back to it is a herd swapping places to and fro.
                if !at.kin.is_empty() {
                    let middle = at.kin.iter().copied().sum::<Vec3>() / at.kin.len() as f32;
                    base = base.lerp(middle.with_y(base.y), 0.7 * ctx.mind.personality.sociability);
                    reach *= 1.0 - 0.6 * ctx.mind.personality.sociability;
                }
                let mut way = Vec3::new(angle.cos(), 0.0, angle.sin());
                // Having given up a chase, it leaves players be: it strolls the far side.
                if ctx.mind.recalls(Memory::GaveUp)
                    && let Some(p) = ctx.nearest()
                {
                    let toward = (p.position - base).with_y(0.0).normalize_or_zero();
                    way -= toward * 2.0 * way.dot(toward).max(0.0);
                }
                mind.goal = Some(base + way * reach);
            }
        }
        if mind.pause > 0.0 {
            // A bird at rest pecks about the ground, looking up between pecks.
            let pose = if flier(ctx) && ctx.lively >= 0.4 && (mind.pause * 1.3).fract() < 0.6 { Pose::Grazing } else { resting_pose(ctx) };
            return Decision { pose, ..Default::default() };
        }
        // A bird flies to where is far (`on_the_wing`), and hops the last of the way.
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
        let radius = flees_at(ctx, self.radius);
        // One who hurt it while it is still afraid, or come within its distance again: not a
        // bystander nearer than them, once its fright has faded.
        ctx.attacker()
            .filter(|p| ctx.mind.fear > 0.0 || ctx.distance(p.position) < radius)
            .or_else(|| ctx.nearest().filter(|p| !p.tempting && ctx.distance(p.position) < sitting(ctx, p).map_or(radius, |f| f.hold)))
            .copied()
    }

    /// A player in sight holds its favourite food, and never hurt it.
    fn calmed(&self, ctx: &Ctx) -> bool {
        ctx.at.players.iter().any(|p| p.tempting && ctx.distance(p.position) < ctx.sight && ctx.mind.attacker() != Some(p.id))
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
        if ctx.at.errand.is_some_and(|e| e.steady) {
            return None;
        }
        match self.threat(ctx) {
            Some(threat) => {
                let closeness = 1.0 - (ctx.distance(threat.position) / (self.radius * 1.5)).min(1.0);
                Want::at(3.0 * ctx.mind.fear.max(closeness * ctx.mind.personality.fear()), threat)
            }
            // Alarmed by kin, it runs from wherever it is toward home (or from where it was
            // startled, as by a snare it slipped); but a player in
            // sight holding its favourite food (and who never hurt it) settles it, so kin
            // that were scared off come to the food rather than run into wherever home is.
            None if ctx.mind.fear > 0.3 && ctx.mind.recalls(Memory::Alarm) && !self.calmed(ctx) => Want::new(ctx.mind.fear),
            None => None,
        }
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, fresh: bool) -> Decision {
        let at = ctx.at;
        // With no one to run from, it runs from where it was startled, else toward home.
        let startled = ctx.mind.startled_at.filter(|_| ctx.mind.recalls(Memory::Startled));
        let from = target.map_or(startled.unwrap_or(at.position * 2.0 - at.home), |p| p.position);
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
            mind.remember(Memory::ScaredBy(p.id), super::SCARED_REMEMBERED);
        }
        // A bird takes wing and goes well clear before it settles again.
        let far = if flier(ctx) { 22.0 } else { 8.0 };
        // One that sat tight bursts away, its first seconds faster than it can run.
        // Not one that was hurt: it flees as any prey does.
        let burst = flush(ctx).filter(|f| ctx.attacker().is_none() && mind.since < f.seconds).map_or(1.0, |f| f.burst);
        Decision { goal: Some(at.position + (away + side).normalize() * far), speed: run(ctx) * burst * winded(ctx, mind), pose: Pose::Alert, alarm: fresh, ..Default::default() }
    }
}

/// How it sits tight (`creature::Flush`), if its kind does and it is neither kept nor on
/// an errand.
fn flush(ctx: &Ctx) -> Option<crate::creature::Flush> {
    ctx.creature.def.flush.filter(|_| !ctx.at.kept && ctx.at.errand.is_none())
}

/// How near a player comes before it flees, its `Flee` radius by how fearful it is.
fn flees_at(ctx: &Ctx, radius: f32) -> f32 {
    radius * (0.5 + ctx.mind.personality.fear())
}

/// How it sits tight from `p` (`creature::Flush`): not once they flushed it, while it
/// recalls being scared by them, so a flushed bird runs well clear and does not settle
/// again at the edge of its `hold`; nor from one who hurt it, while it recalls that, so a
/// wounded bird flees them at its kind's distance.
fn sitting(ctx: &Ctx, p: &Seen) -> Option<crate::creature::Flush> {
    flush(ctx).filter(|_| !ctx.mind.recalls(Memory::ScaredBy(p.id)) && !ctx.mind.recalls(Memory::HurtBy(p.id)))
}

/// A game bird sits tight (`creature::Flush`): a player it has noticed, beyond its `hold`
/// and within the distance it would otherwise flee at (`flees_at`), it lets be, crouched
/// still in the grass, facing nowhere in particular so as not to be seen looking; one
/// further off, it lives its day. Not once it is hurt or its kin are alarmed: then it
/// runs (`Flee`).
#[derive(Debug)]
struct Hide;

impl Module for Hide {
    fn name(&self) -> &'static str {
        "hide"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        if flush(ctx).is_none() || ctx.mind.fear >= 0.3 || ctx.attacker().is_some() {
            return None;
        }
        // Its widest flight, as `Warn` takes the widest hunt.
        let wary = ctx
            .creature
            .behaviours
            .iter()
            .filter_map(|b| match b.behaviour {
                Behaviour::Flee { radius } => Some(flees_at(ctx, radius)),
                _ => None,
            })
            .reduce(f32::max)?;
        let p = ctx.nearest().filter(|p| !p.tempting && sitting(ctx, p).is_some_and(|f| (f.hold..wary).contains(&ctx.distance(p.position))))?;
        Want::at(2.0, *p)
    }

    fn act(&self, _: &Ctx, mind: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        mind.goal = None;
        Decision { pose: Pose::Crouched, ..Default::default() }
    }
}

/// Blocks from food on the ground that it snatches a piece from (`creature::Scavenge`).
const SNATCH: f32 = 1.3;
/// Blocks from home that it eats what it carried off.
const HOME_WITH_IT: f32 = 2.0;

/// A scavenger (`creature::Scavenge`) slinks in, crouched, to food left lying on the ground
/// while no player it has noticed is within its `shy` of it or of the food, snatches a
/// piece and makes off home with it at a run, and eats it there. Not once it is hurt or
/// afraid, nor kept or on an errand: those let food lie.
#[derive(Debug)]
struct Scavenge;

impl Module for Scavenge {
    fn name(&self) -> &'static str {
        "scavenge"
    }

    fn band(&self) -> Band {
        Band::Drive
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let scavenge = ctx.creature.def.scavenge.as_ref()?;
        if ctx.at.kept || ctx.at.errand.is_some() || ctx.attacker().is_some() || ctx.mind.fear >= 0.3 {
            return None;
        }
        if ctx.at.carrying {
            return Want::new(1.5);
        }
        let scrap = ctx.at.scrap.filter(|&s| flat(s, ctx.at.position) <= scavenge.reach)?;
        let watched = ctx.at.players.iter().any(|p| !p.friend && (ctx.distance(p.position) < scavenge.shy || flat(p.position, scrap) < scavenge.shy));
        if watched {
            return None;
        }
        Want::new(1.2 * (0.6 + 0.6 * ctx.mind.personality.curiosity))
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let at = ctx.at;
        if at.carrying {
            mind.goal = Some(at.home);
            let home = flat(at.home, at.position) < HOME_WITH_IT;
            return Decision { goal: Some(at.home).filter(|_| !home), speed: run(ctx), swallow: home, ..Default::default() };
        }
        let Some(scrap) = at.scrap else { return Decision::default() };
        mind.goal = Some(scrap);
        let close = flat(scrap, at.position) < SNATCH;
        Decision { goal: Some(scrap).filter(|_| !close), speed: walk(ctx), face: Some(scrap), take: Some(scrap).filter(|_| close), pose: Pose::Crouched, ..Default::default() }
    }
}

/// The share of its run a fleeing creature runs at (`creature::Tires`): flat out until it
/// has run its `after` seconds, then winded at its `pace` for `rest` seconds, and flat out
/// again after; and the more it is hurt the slower, by its `hurt`. Each decision spent fleeing counts toward it; time spent otherwise rests it
/// a second for a second. Not for one that is kept or on an errand: a tame or a village's
/// animal is not run down.
fn winded(ctx: &Ctx, mind: &mut Mind) -> f32 {
    let Some(tires) = ctx.creature.def.tires.filter(|_| !ctx.at.kept && ctx.at.errand.is_none()) else { return 1.0 };
    // Hurt, it limps: `hurt` slower at the last of its health, none at full.
    let limp = 1.0 - tires.hurt * (1.0 - ctx.at.health.clamp(0.0, 1.0));
    // Winded while more than half a decision of its rest is left: as many decisions as
    // the rest holds.
    if mind.winded > ctx.at.dt * 0.5 {
        mind.winded -= ctx.at.dt;
        return tires.pace * limp;
    }
    mind.sprinted += ctx.at.dt;
    if mind.sprinted >= tires.after {
        (mind.sprinted, mind.winded) = (0.0, tires.rest);
    }
    limp
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
        // Each keeps near one companion of its own choosing, not the herd's middle, and
        // cares more or less from moment to moment, so a herd spreads, strings out and
        // regroups instead of moving in lockstep.
        let friend = herd_friend(ctx);
        let spacing = self.spacing * (1.5 - ctx.mind.personality.sociability);
        let apart = ((flat(friend, ctx.at.position) - spacing) / (spacing * 2.0)).clamp(0.0, 1.5);
        // Once on its way it goes on until it is beside its companion, at a steady will: a
        // will drawn afresh each tick dips under its others' and back, and the animal turns
        // to and fro between herding and wandering, twitching in place.
        let going = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name());
        if going {
            return (flat(beside(ctx, spacing), ctx.at.position) > 0.6).then(|| Want::new(0.8 * ctx.mind.personality.sociability * apart.max(0.25))).flatten();
        }
        // And it sets off only once well apart (its spacing past it), so a companion
        // shifting a step does not start it each time.
        (apart > 0.5).then(|| Want::new(0.8 * ctx.mind.personality.sociability * apart * (0.5 + ctx.random(91)))).flatten()
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        // It keeps to the same companion while it goes (`herd_friend`).
        mind.goal = Some(herd_friend(ctx));
        let spacing = self.spacing * (1.5 - ctx.mind.personality.sociability);
        Decision { goal: Some(beside(ctx, spacing)), speed: walk(ctx), ..Default::default() }
    }
}

/// Where a herd animal goes: beside its companion on its own side of it, not onto it, nor
/// round to the far side: two that keep near each other would otherwise swap places, to
/// and fro.
fn beside(ctx: &Ctx, spacing: f32) -> Vec3 {
    let friend = herd_friend(ctx);
    friend + (ctx.at.position - friend).with_y(0.0).normalize_or(Vec3::X) * spacing * 0.6
}

/// The kin an individual keeps near: one of its own choosing among those about, and once it
/// is going to one, the same one (the nearest to where that one last was), not whichever
/// the list of those about now puts in its place.
fn herd_friend(ctx: &Ctx) -> Vec3 {
    let kin = &ctx.at.kin;
    let going = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == "herd");
    if let Some(last) = ctx.mind.goal.filter(|_| going) {
        return kin.iter().copied().min_by(|a, b| a.distance(last).total_cmp(&b.distance(last))).unwrap_or(last);
    }
    kin[(ctx.mind.seed % kin.len().max(1) as u64) as usize % kin.len().max(1)]
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
        // Once following, it keeps after the food a while further than it first noticed it,
        // so a player walking on does not lose it at the edge.
        let following = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name());
        let radius = if following { self.radius * 1.5 } else { self.radius };
        let p = ctx.at.players.iter().find(|p| p.tempting && ctx.distance(p.position) < radius)?;
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
        let d = ctx.distance(p.position);
        // Too close (carried in by a hop, or pushed by those behind): a step back, so a
        // crowd after the food does not pile onto the player.
        let back = ctx.at.position + (ctx.at.position - p.position).with_y(0.0).normalize_or(Vec3::X) * keep;
        let goal = if d > keep { Some(p.position) } else if d < keep * 0.6 { Some(back) } else { None };
        // It walks up to the food, and hurries after it as the player draws ahead (up to a
        // run four blocks behind its place), so it keeps up with one walking on.
        let behind = ((d - keep - 1.0) / 3.0).clamp(0.0, 1.0);
        let speed = walk(ctx) * 1.2 + (run(ctx) - walk(ctx) * 1.2).max(0.0) * behind;
        Decision { goal, speed, face: Some(p.position), pose: Pose::Alert, ..Default::default() }
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
        if ctx.mind.recalls(Memory::GaveUp) {
            return None;
        }
        if let Some(p) = ctx.attacker().filter(|_| ctx.mind.anger > 0.0) {
            return Want::at(2.5 * ctx.mind.anger.max(personality.aggression), *p);
        }
        // Whom it goes for is as ever; one that warns (`Warn`) warns them first if it sees
        // them, and goes for them once provoked. One it does not see it goes for unwarned.
        let prey = hunted(ctx, self.radius, 0.0)?;
        if def.warn.is_some() && prey.in_view {
            return None;
        }
        // A torch held keeps one that gives ground before fire from going for them, seen or
        // not (`Fire`); it warns them from beyond it, if it sees them.
        if prey.fire && def.fire.is_some_and(|f| f.gives_ground(ctx.mind.seed)) {
            return None;
        }
        Want::at(2.5 * personality.aggression, *prey)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        strike(ctx, mind, target, true)
    }
}

/// Warns a player before it attacks them (`creature::Warn`): it stands and faces them in
/// its warning stance; if they stay its `seconds` (warning any it would, one after
/// another, on one clock), or come within its `near` blocks, it is provoked and goes for
/// them as if struck (`Mind::provoked`), its hunt or defence taking over that very
/// decision (`think`). One who backs off `WARN_EXIT` blocks past where it began to warn,
/// or out of its sight, is let be once it has not warned for `WARN_LAPSE` seconds.
///
/// A warning only tells a hunt before it comes, and never changes whom a hunter goes for or
/// when: it warns the one its hunt would go for (`hunted`, the nearest it would hunt), as
/// far off, if it sees them (`Seen::in_view`); one it does not see its hunt goes for
/// unwarned, as before. It warns on the hunt's own terms (kept, frightened or on an errand
/// as much as not). One that only defends itself warns those who come within its `radius`
/// in plain sight, but no one holding the food that tempts it, and not while frightened,
/// kept, on an errand or having given up a chase.
#[derive(Debug)]
struct Warn;

/// Blocks past its radius a player must go to end a warning, so one at the edge does not
/// set it flickering on and off.
pub const WARN_EXIT: f32 = 1.5;

impl Module for Warn {
    fn name(&self) -> &'static str {
        "warn"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let def = &ctx.creature.def;
        let warn = def.warn?;
        if def.boss || ctx.mind.recalls(Memory::GaveUp) {
            return None;
        }
        if ctx.attacker().is_some() && ctx.mind.anger > 0.0 {
            return None;
        }
        let slack = if ctx.mind.warning.is_some() { WARN_EXIT } else { 0.0 };
        // Its widest hunt: every player one of its hunts would go for, it warns first.
        let hunt = ctx.creature.behaviours.iter().filter_map(|b| match b.behaviour {
            Behaviour::Hunt { radius } => Some(radius),
            _ => None,
        }).reduce(f32::max);
        if hunt.is_none() && (ctx.at.kept || ctx.at.errand.is_some() || ctx.mind.fear > 0.2) {
            return None;
        }
        let p = match (hunt, warn.radius) {
            (Some(radius), _) => hunted(ctx, radius, slack).filter(|p| p.in_view)?,
            (None, Some(radius)) => {
                let radius = radius * (0.6 + 0.4 * ctx.lively) + slack;
                ctx.at.players.iter().find(|p| {
                    !p.creative && !p.friend && !p.tolerated && !p.tempting && !p.warded && p.in_view
                        && ctx.distance(p.position) < radius.min(ctx.sight + slack)
                        && p.combat_level as u16 <= def.level as u16 * 2 + 1
                })?
            }
            (None, None) => return None,
        };
        Want::at(2.0, *p)
    }

    fn act(&self, ctx: &Ctx, mind: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        let (Some(p), Some(warn)) = (target, ctx.creature.def.warn) else { return Decision::default() };
        let seconds = mind.warning.map_or(0.0, |w| w.seconds + ctx.at.dt);
        // A torch held at one that gives ground before it (`Fire`) keeps it warning from
        // beyond its reach for as long as it is held: its time runs out, but it does not come.
        let held_off = p.fire && ctx.creature.def.fire.is_some_and(|f| f.gives_ground(ctx.mind.seed));
        if (seconds >= warn.seconds && !held_off) || ctx.distance(p.position) <= warn.near {
            // `think` decides afresh, its hunt or defence taking it up.
            mind.provoked(p.id);
            return Decision::default();
        }
        mind.warning = Some(super::Warning { seconds, lapse: 0.0 });
        Decision { face: Some(p.position), pose: Pose::Warning, ..Default::default() }
    }
}

/// The player a hunter of `radius` (its `Hunt`) goes for now, unprovoked, reckoned `slack`
/// blocks further: the nearest in reach of its hunt by its liveliness, hunger and
/// aggression, and in its sight. RuneScape's rules leave be a player who has settled long
/// in one place, or one of more than twice the creature's level; a boss always fights. One
/// in a lit fire's ward at night (`Seen::warded`) it leaves be (`Fire`).
fn hunted<'a>(ctx: &'a Ctx, radius: f32, slack: f32) -> Option<&'a Seen> {
    let def = &ctx.creature.def;
    let radius = radius * (0.6 + 0.4 * ctx.lively) * (0.7 + 0.6 * ctx.mind.hunger) * (0.7 + 0.6 * ctx.mind.personality.aggression) + slack;
    ctx.at.players.iter().find(|p| {
        !p.creative && !p.friend && !p.warded && ctx.distance(p.position) < radius.min(ctx.sight + slack)
            && (def.boss || (!p.tolerated && p.combat_level as u16 <= def.level as u16 * 2 + 1))
    })
}

/// Fire keeps it back (`creature::Fire`): it backs away from a player holding a fire at it,
/// facing them, until they are its `give` off, and walks out of a lit fire's ward at
/// night. Not once provoked (it fights), nor kept or on an errand (tame animals, villagers
/// and guards pay fire no heed); of its kind only its `fear`'s share give ground before a
/// torch (by each one's own roll), the bold rest stand theirs, though none in a ward's
/// light.
#[derive(Debug)]
struct Fire;

/// Blocks further off a fire's bearer must be to end its giving ground, so it does not
/// step back and forth at the edge.
pub const FIRE_EXIT: f32 = 1.0;

impl Fire {
    /// The player holding a fire at it, near enough to give ground from: one it has noticed
    /// (`Senses`), so a flame it lost sight of as it turned away it still backs from.
    fn bearer<'a>(ctx: &'a Ctx, fire: crate::creature::Fire) -> Option<&'a Seen> {
        if !fire.gives_ground(ctx.mind.seed) {
            return None;
        }
        let slack = if ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == "fire") { FIRE_EXIT } else { 0.0 };
        ctx.at.players.iter().find(|p| p.fire && !p.creative && !p.friend && ctx.distance(p.position) < fire.give + slack)
    }
}

impl Module for Fire {
    fn name(&self) -> &'static str {
        "fire"
    }

    fn band(&self) -> Band {
        Band::Danger
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        let fire = ctx.creature.def.fire?;
        if ctx.at.kept || ctx.at.errand.is_some() || (ctx.attacker().is_some() && ctx.mind.anger > 0.0) {
            return None;
        }
        if let Some(p) = Self::bearer(ctx, fire) {
            return Want::at(4.0, *p);
        }
        let ward = ctx.at.ward.filter(|_| fire.ward)?;
        (flat(ctx.at.position, ward.at) < ward.radius).then(|| Want::new(4.0)).flatten()
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, target: Option<Seen>, _: bool) -> Decision {
        let at = ctx.at;
        let def = &ctx.creature.def;
        let away = |from: Vec3| (at.position - from).with_y(0.0).normalize_or(Vec3::X);
        if let (Some(p), Some(fire)) = (target, def.fire) {
            // Facing the flame, a step or so past where it would be let be.
            let back = (fire.give + FIRE_EXIT - ctx.distance(p.position)).max(1.0);
            let pose = if def.warn.is_some() { Pose::Warning } else { Pose::Alert };
            return Decision { goal: Some(at.position + away(p.position) * back), face: Some(p.position), speed: walk(ctx), pose, ..Default::default() };
        }
        let Some(ward) = at.ward else { return Decision::default() };
        Decision { goal: Some(ward.at + away(ward.at) * (ward.radius + 2.0)), speed: walk(ctx), pose: Pose::Alert, ..Default::default() }
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
    // Past its leash it gives the chase up and goes home: a wolf does not follow a player
    // across the land. A boss fights to the end, and those the server sends after someone
    // (guards) go where it says. Struck there within the last few seconds, it fights back:
    // a player who led it past its leash and stands to fight is fought.
    let leash = if ctx.at.kept { def.leash.min(super::super::husbandry::KEPT_LEASH) } else { def.leash };
    if !mind.recalls(Memory::Struck) && !def.boss && ctx.at.errand.is_none() && flat(ctx.at.position, ctx.at.home) > leash {
        mind.give_up();
        return Decision::default();
    }
    // Close when an attack of the phase it has reached fits, measured as its wind-up
    // measures it (from its body's edge, at its own size): a boss with a volley strikes
    // from afar, and closes in where none of its moves fits (`moves::Creature::reaches`).
    let d = flat(p.position, ctx.at.position);
    // Between blows, one that circles backs off past its reach and moves round the target,
    // facing it, aiming `round` degrees on round from where it stands (each its own way).
    if mind.circling > 0.0
        && let Some((reach, circle)) = def.attack.and_then(|a| a.circle.map(|c| (a.reach, c)))
    {
        let from = (ctx.at.position - p.position).with_y(0.0).normalize_or(Vec3::X);
        let side = if mind.seed & 1 == 0 { 1.0 } else { -1.0 };
        let round = glam::Quat::from_rotation_y(side * circle.round.to_radians()) * from;
        let off = def.body.size.0 * ctx.at.scale * 0.5 + reach + circle.away;
        return Decision { goal: Some(p.position + round * off), speed: run(ctx) * circle.pace, face: Some(p.position), ..Default::default() };
    }
    let reaches = ctx.creature.reaches(ctx.at.phase, ctx.creature.edge(d, ctx.at.scale), ctx.creature.level(ctx.at.position.y, p.position.y, ctx.at.scale));
    let close = reaches && p.open && ctx.creature.in_band(ctx.at.position.y, p.position.y, crate::physics::player::HEIGHT, ctx.at.scale);
    let mut decision = Decision { face: Some(p.position), ..Default::default() };
    let stalker = stalks && def.movement.style == Style::Stalker;
    // Held off (`Mind::held_off`): while the player stays where it could not reach them,
    // it keeps out of their reach, facing them, on the side it stands (a keyed side when
    // right over them), and stands once there.
    if mind.held_off.is_some_and(|(id, at)| id != p.id || at.distance(p.position) > HELD_OFF_MOVED) {
        mind.held_off = None;
    }
    if !close && def.boss && mind.held_off.is_some() {
        let off = (ctx.at.position - p.position).with_y(0.0);
        let away = off.try_normalize().unwrap_or_else(|| {
            let turn = ctx.random(0x4e1d_0ff5) * std::f32::consts::TAU;
            Vec3::new(turn.cos(), 0.0, turn.sin())
        });
        // Their reach along the ground at the height it stands over their eyes.
        let over = (ctx.at.position.y - p.position.y - crate::physics::player::EYE_HEIGHT).max(0.0);
        let reach = (crate::physics::player::REACH.powi(2) - over * over).max(0.0).sqrt() + super::KEEP_OFF;
        if ctx.creature.edge(d, ctx.at.scale) < reach - 0.5 {
            let keep = reach + ctx.creature.edge(0.0, ctx.at.scale).abs();
            decision.goal = Some(p.position.with_y(ctx.at.position.y) + away * keep);
            decision.speed = run(ctx);
        }
    } else if !close {
        // Those with a way found after the target (guards going after someone) follow it
        // until the target is near; the day's errands are no way to anyone.
        decision.goal = Some(ctx.at.errand.filter(|e| e.chase.is_some_and(|handoff| d > handoff)).map_or(p.position, |e| e.to));
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

/// Goes where its routine sends it, by the turns of the way the server found, and once
/// there rests or stands as the errand says, looking at a player who comes close.
#[derive(Debug)]
struct Routine {
    arrive: f32,
    look: f32,
}

impl Module for Routine {
    fn name(&self) -> &'static str {
        "routine"
    }

    fn band(&self) -> Band {
        Band::Drive
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        ctx.at.errand.and_then(|_| Want::new(1.0))
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let Some(errand) = ctx.at.errand else { return Decision::default() };
        let there = errand.last && flat(errand.to, ctx.at.position) < self.arrive;
        if !there {
            return Decision { goal: Some(errand.to), speed: walk(ctx) * errand.pace, ..Default::default() };
        }
        // Awake, they turn to a player close by.
        let near = ctx.nearest().filter(|p| ctx.distance(p.position) < self.look && errand.pose != Pose::Resting);
        Decision { face: near.map(|p| p.position).or(errand.face), pose: errand.pose, ..Default::default() }
    }
}

/// How near home a creature that gave up a chase goes, in blocks.
const GAVE_UP_HOME: f32 = 3.0;

/// Goes back toward home when it has strayed past its leash, or gave up a chase.
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
        // Those the server sends about their day (villagers) go home when it says.
        if ctx.at.errand.is_some() {
            return None;
        }
        let leash = if ctx.at.kept { ctx.creature.def.leash.min(super::super::husbandry::KEPT_LEASH) } else { ctx.creature.def.leash };
        let from_home = flat(ctx.at.position, ctx.at.home);
        // Once going, it keeps on until well inside.
        let going = ctx.mind.doing.is_some_and(|i| ctx.creature.modules[i].0.name() == self.name());
        // Having given up a chase, it goes all the way home rather than hang about where
        // it gave up (at a fence, round a box it could not get into).
        let limit = if ctx.mind.recalls(Memory::Homeward) {
            GAVE_UP_HOME
        } else if going {
            leash * 0.5
        } else {
            leash
        };
        (from_home > limit).then(|| Want::new(1.0 + 2.0 * ((from_home - leash) / leash).clamp(0.0, 1.0))).flatten()
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        // Having given up a chase, it lopes off rather than amble about where it gave up.
        let speed = if ctx.mind.recalls(Memory::Homeward) { walk(ctx) + (run(ctx) - walk(ctx)) * 0.3 } else { walk(ctx) };
        Decision { goal: Some(ctx.at.home), speed, ..Default::default() }
    }
}

/// Goes to another of its kind that is ready to breed, when it is ready too (the server
/// brings them together into a young one once they meet).
#[derive(Debug)]
struct Court;

impl Module for Court {
    fn name(&self) -> &'static str {
        "court"
    }

    fn band(&self) -> Band {
        Band::Drive
    }

    fn want(&self, ctx: &Ctx) -> Option<Want> {
        ctx.at.mate?;
        Want::new(2.5)
    }

    fn act(&self, ctx: &Ctx, _: &mut Mind, _: Option<Seen>, _: bool) -> Decision {
        let Some(mate) = ctx.at.mate else { return Decision::default() };
        let goal = (flat(mate, ctx.at.position) > 0.8).then_some(mate);
        Decision { goal, speed: walk(ctx), face: Some(mate), pose: Pose::Alert, ..Default::default() }
    }
}
