//! How fights feel (`voxmar_engine::combat`, docs/combat.md, the design wiki's "Combat
//! feel"): each player's swings in rhythm, their guard and dodges, creatures winding up
//! before every blow and recovering after, and what flourishes do.
//!
//! A click on a creature starts a swing with what is in hand, or is held for the beat; its
//! blow lands with that weapon when the wind-up is done, if the creature is still in reach.
//! Switching what is in hand first drops the swing. A creature that decides to attack winds
//! up first, showing it (the attacking pose): for the first part of the wind-up it turns to
//! follow its target and walks at it, then its aim locks; as the blow lands it lunges, then
//! stands to recover. On a player out of reach the blow falls short; on one mid-dodge it
//! passes through (a perfect dodge, and the next blow is a riposte); a guard facing it stops
//! part, or, raised just in time, all of it, staggering the creature after its recovery and
//! making the next blow a counter. Each is judged by the moment the blow lands.
//!
//! Times are seconds since the server started, on the clock player messages arrive by
//! (`Server::arrival`), so a busy server neither hurries nor slows anyone's rhythm. A
//! creature's blow lands on the same clock, a wind-up after it began.

use std::time::Instant;

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::combat::{Blow, Click, Defended, Effect, Guarding, Pace, Rhythm, Stroke, Swing, Weapon};
use voxmar_engine::creature::{brain, mix, unit_from};
use voxmar_engine::glam::Vec3;
use voxmar_engine::physics::Aabb;
use voxmar_engine::skill::SkillId;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// Half a player's width: how close a creature's lunge comes to them.
const PLAYER_HALF: f32 = 0.3;
/// The step a lunge is tried in, so it stops at a wall.
const LUNGE_STEP: f32 = 0.1;

/// One player's side of fights.
#[derive(Default)]
struct Fighter {
    rhythm: Rhythm,
    /// The swing under way: the creature it is aimed at, what it is struck with, and
    /// whether it is a counter or a riposte.
    aim: Option<Aim>,
    /// The item last swung, to lose the pips on a switch.
    held: Option<BlockId>,
    guarding: Guarding,
    /// Until when a blow is a counter (after a perfect block) or a riposte (after a
    /// perfect dodge).
    counter_until: f64,
    riposte_until: f64,
}

#[derive(Clone, Copy)]
struct Aim {
    creature: u32,
    counter: bool,
    riposte: bool,
    /// What the swing started with: the blow lands with it, and a switch drops it.
    weapon: Weapon,
    held: Option<BlockId>,
}

/// A creature's blow on its way.
struct Windup {
    creature: u32,
    player: PlayerId,
    lands: f64,
}

/// Bleeding from a flourish: so much more, a point at a time, a second apart, training the
/// skill of the weapon that drew it.
struct Bleed {
    creature: u32,
    player: PlayerId,
    skill: Option<SkillId>,
    left: u16,
    every: f64,
    next: f64,
}

pub(crate) struct Fights {
    epoch: Instant,
    fighters: FxHashMap<PlayerId, Fighter>,
    windups: Vec<Windup>,
    bleeds: Vec<Bleed>,
    /// Creatures slowed by a pin (the share, and until when) and sundered (their defence
    /// lower by the share, until when).
    pinned: FxHashMap<u32, (f32, f64)>,
    sundered: FxHashMap<u32, (f32, f64)>,
    /// Draws for what rounds by chance (`combat::by_chance`).
    draws: u64,
}

impl Default for Fights {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            fighters: FxHashMap::default(),
            windups: Vec::new(),
            bleeds: Vec::new(),
            pinned: FxHashMap::default(),
            sundered: FxHashMap::default(),
            draws: 0,
        }
    }
}

impl Fights {
    fn secs(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.epoch).as_secs_f64()
    }

    fn now(&self) -> f64 {
        self.secs(Instant::now())
    }

    /// The share a pin slows `creature` by now.
    pub(crate) fn slow(&self, creature: u32) -> f32 {
        let now = self.now();
        self.pinned.get(&creature).filter(|&&(_, until)| now <= until).map_or(0.0, |&(share, _)| share)
    }

    /// The share a sunder lowers `creature`'s defence by now.
    pub(crate) fn sunder(&self, creature: u32) -> f32 {
        let now = self.now();
        self.sundered.get(&creature).filter(|&&(_, until)| now <= until).map_or(0.0, |&(share, _)| share)
    }

    /// The rhythm pips `player` has now, for what the server tells them of a blow.
    pub(crate) fn pips(&self, player: PlayerId, feel: &voxmar_engine::combat::Feel) -> u8 {
        let now = self.now();
        self.fighters.get(&player).map_or(0, |f| f.rhythm.pips(now, feel))
    }

    /// A draw in 0..1 for rounding by chance, different each time.
    fn draw(&mut self, salt: u64) -> f32 {
        self.draws += 1;
        unit_from(mix(self.draws, salt))
    }
}

fn ticks(seconds: f32) -> u64 {
    (seconds.max(0.0) * TICKS_PER_SECOND as f32).ceil() as u64
}

impl Server {
    fn fight_arrival(&self) -> f64 {
        self.fights.secs(self.arrival)
    }

    fn wielded_by(&self, player: PlayerId) -> Option<Weapon> {
        let p = self.players.get(&player)?;
        Some(self.content.combat.wielded(p.held(), |skill| self.content.skills.level(p.skills.xp(skill))))
    }

    /// A player clicked a creature to strike it (`core:attack`): a swing starts with what
    /// is in hand, or waits for the beat. Nothing while their guard is up.
    pub(crate) fn swing_at(&mut self, player: PlayerId, creature: u32) {
        let Some(weapon) = self.wielded_by(player) else { return };
        let held = self.players.get(&player).and_then(|p| p.held());
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        let feel = self.content.combat.feel;
        let fighter = self.fights.fighters.entry(player).or_default();
        if fighter.guarding.is_up() {
            return;
        }
        if fighter.held != held {
            fighter.rhythm.lose_pips();
            fighter.held = held;
        }
        if let Click::Swing(_) = fighter.rhythm.click(now, &timing, &feel) {
            let counter = now <= fighter.counter_until;
            let riposte = now <= fighter.riposte_until;
            fighter.counter_until = 0.0;
            fighter.riposte_until = 0.0;
            fighter.aim = Some(Aim { creature, counter, riposte, weapon, held });
        }
    }

    /// A player raised or lowered their guard. Raising it cancels their own wind-up; a raise
    /// soon after lowering opens no perfect window.
    pub(crate) fn raise_guard(&mut self, player: PlayerId, up: bool) {
        let Some(weapon) = self.wielded_by(player) else { return };
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        let feel = self.content.combat.feel;
        let fighter = self.fights.fighters.entry(player).or_default();
        if up {
            if fighter.guarding.raise(now, &feel) && fighter.rhythm.cancel(now, &timing, &feel) {
                fighter.aim = None;
            }
        } else {
            fighter.guarding.lower(now);
        }
    }

    /// A player dodged (`core:dodge`): for a moment no blow lands on them. Their client
    /// carries the dash itself, within what movement allows. Only on foot on the ground (or
    /// just off it), not flying, eating or guarding, and not sooner than the last allows;
    /// it cancels their own wind-up. A dodge refused puts the player back where the server
    /// has them, so their screen never shows a dash no blow passes through.
    pub(crate) fn dodge(&mut self, player: PlayerId) {
        let Some(weapon) = self.wielded_by(player) else { return };
        let Some(p) = self.players.get(&player) else { return };
        let feel = self.content.combat.feel;
        let grounded = self.tick <= p.grounded + ticks(feel.dodge_grace);
        let may = p.health.alive() && !p.flying && grounded && !self.eating(player);
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        // Armour weight sets the dodge; without armour, the first.
        let dodge = self.content.combat.dodge(None).clone();
        let fighter = self.fights.fighters.entry(player).or_default();
        if !(may && fighter.guarding.dodge(now, &dodge, &feel)) {
            if let Some(p) = self.players.get(&player) {
                p.connection.send(ServerMessage::Teleport { position: p.position });
            }
            return;
        }
        if fighter.rhythm.cancel(now, &timing, &feel) {
            fighter.aim = None;
        }
    }

    /// A creature decided to strike a player: it winds up first, showing it, following them
    /// for the first part of it.
    pub(crate) fn wind_up(&mut self, creature: u32, player: PlayerId) {
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let Some(attack) = self.content.creatures.get(live.species).def.attack else { return };
        if self.fights.windups.iter().any(|w| w.creature == creature) {
            return;
        }
        let feel = self.content.combat.feel;
        let windup = attack.windup.unwrap_or(feel.creature_windup).max(0.0);
        let lands = self.fights.now() + windup as f64;
        let tick = self.tick;
        let live = self.creatures.live.get_mut(&creature).expect("checked above");
        live.winding = true;
        live.held_until = live.held_until.max(tick + ticks(windup));
        live.tracking = Some((player, tick + ticks(windup * feel.creature_tracking.clamp(0.0, 1.0))));
        live.changed = true;
        self.fights.windups.push(Windup { creature, player, lands });
    }

    /// Where a creature winding up at a player goes this tick, while it still follows them:
    /// at them, at the feel's pace, until it touches them; then it stands and faces them.
    pub(crate) fn tracking_drive(&self, creature: u32) -> Option<voxmar_engine::creature::motion::Drive> {
        let live = self.creatures.live.get(&creature)?;
        let (player, until) = live.tracking?;
        if self.tick >= until {
            return None;
        }
        let target = self.players.get(&player)?.position;
        let def = &self.content.creatures.get(live.species).def;
        let to = (target - live.body.position).with_y(0.0);
        let touching = live.body.size.0 * 0.5 + PLAYER_HALF + self.content.combat.feel.creature_clearance;
        let pace = match self.content.combat.feel.tracking_pace {
            Pace::Walk => def.movement.walk,
            Pace::Run => def.movement.run,
        };
        let walking = to.length() > touching;
        Some(voxmar_engine::creature::motion::Drive {
            wish: if walking { to.normalize_or_zero() } else { Vec3::ZERO },
            speed: if walking { pace * live.scale.max(0.5) * (1.0 - self.fights.slow(creature)) } else { 0.0 },
            face: Some(to),
        })
    }

    /// Lands every swing, creature blow and bleed that is due. Runs each tick, after
    /// creatures move.
    pub(crate) fn tick_fights(&mut self) {
        let now = self.fights.now();
        let feel = self.content.combat.feel;
        self.fights.fighters.retain(|id, _| self.players.contains_key(id));
        let live = &self.creatures.live;
        self.fights.pinned.retain(|id, &mut (_, until)| now <= until && live.contains_key(id));
        self.fights.sundered.retain(|id, &mut (_, until)| now <= until && live.contains_key(id));
        // A swing whose weapon left the hand is dropped, and the pips with it.
        for (&player, fighter) in &mut self.fights.fighters {
            let Some(aim) = fighter.aim else { continue };
            let held = self.players.get(&player).and_then(|p| p.held());
            if held != aim.held {
                let timing = self.content.combat.timing(&aim.weapon);
                fighter.rhythm.cancel(now, &timing, &feel);
                fighter.rhythm.lose_pips();
                fighter.aim = None;
                fighter.held = held;
            }
        }
        // Players' swings.
        let mut landing = Vec::new();
        for (&player, fighter) in &mut self.fights.fighters {
            if let Some(swing) = fighter.rhythm.landed(now) {
                let Some(aim) = fighter.aim.take() else { continue };
                let on_beat = swing.on_beat || aim.riposte || aim.counter;
                let kind = fighter.rhythm.blow(&Swing { on_beat, ..swing }, now, &feel);
                landing.push((player, aim, swing.strength, on_beat, kind));
            }
        }
        for (player, aim, strength, on_beat, kind) in landing {
            self.land_swing(player, aim, strength, on_beat, kind);
        }
        // Creatures' blows, each judged at the moment it lands.
        let (due, waiting): (Vec<Windup>, Vec<Windup>) = std::mem::take(&mut self.fights.windups).into_iter().partition(|w| w.lands <= now);
        self.fights.windups = waiting;
        for windup in due {
            self.creature_blow(windup.creature, windup.player, windup.lands);
        }
        // Bleeding.
        let (due, waiting): (Vec<Bleed>, Vec<Bleed>) = std::mem::take(&mut self.fights.bleeds).into_iter().partition(|b| b.next <= now);
        self.fights.bleeds = waiting;
        for mut bleed in due {
            if !self.players.contains_key(&bleed.player) {
                continue;
            }
            if self.hurt_creature(bleed.player, bleed.creature, 1, bleed.skill, Blow::Bleed).is_some() {
                bleed.left -= 1;
                bleed.next += bleed.every;
                if bleed.left > 0 {
                    self.fights.bleeds.push(bleed);
                }
            }
        }
    }

    /// A player's swing lands: at the creature it was aimed at, if still in reach, with the
    /// weapon it was swung with and what its rhythm made it.
    fn land_swing(&mut self, player: PlayerId, aim: Aim, strength: f32, on_beat: bool, kind: Blow) {
        let feel = self.content.combat.feel;
        let weapon = aim.weapon;
        let Some(p) = self.players.get(&player) else { return };
        let eye = p.eye();
        let falling = p.fall_start.is_some_and(|top| p.position.y < top - feel.falling_drop);
        let in_reach = self.creatures.reach_box(aim.creature).is_some_and(|aabb| eye.distance(eye.clamp(aabb.min, aabb.max)) <= weapon.reach + feel.reach_slack);
        if !in_reach {
            // Swung at the air: still answered, as a miss.
            self.struck(player, aim.creature, 0, false, Blow::Plain);
            return;
        }
        let stroke = Stroke::of(kind, strength, on_beat, aim.counter, aim.riposte, falling, &feel);
        let Some(damage) = self.land_blow(player, aim.creature, &weapon, stroke) else { return };
        if kind == Blow::Flourish && damage > 0 {
            self.flourish(player, aim.creature, &weapon, damage);
        }
    }

    /// Tells a player what came of their blow, with their pips as the server counts them.
    pub(crate) fn struck(&self, player: PlayerId, creature: u32, damage: u16, killed: bool, blow: Blow) {
        if let Some(p) = self.players.get(&player) {
            let pips = self.fights.pips(player, &self.content.combat.feel);
            p.connection.send(ServerMessage::Struck { creature, damage, killed, blow, pips });
        }
    }

    /// What a flourish does besides its max hit, by the weapon's class.
    fn flourish(&mut self, player: PlayerId, creature: u32, weapon: &Weapon, damage: u16) {
        let now = self.fights.now();
        let feel = self.content.combat.feel;
        let Some(flourish) = self.content.combat.flourish(weapon) else { return };
        let roll = self.fights.draw(0xf10u64 ^ creature as u64);
        match flourish.effect(damage, roll) {
            Some(Effect::Bleed { points, every }) => {
                let every = every as f64;
                self.fights.bleeds.push(Bleed { creature, player, skill: weapon.skill, left: points, every, next: now + every });
            }
            Some(Effect::Cleave { points }) => {
                let Some(p) = self.players.get(&player) else { return };
                let others = self.creatures.live.iter().filter(|&(&id, c)| id != creature && c.health > 0).map(|(&id, c)| (id, c.body.aabb(), c.owner.is_some()));
                if let Some(other) = voxmar_engine::combat::cleave_target(p.eye(), p.look(), weapon.reach + feel.reach_slack, feel.cleave_cone, others) {
                    self.hurt_creature(player, other, points, weapon.skill, Blow::Flourish);
                }
            }
            Some(Effect::Pin { slow, seconds }) => {
                self.fights.pinned.insert(creature, (slow, now + seconds as f64));
            }
            Some(Effect::Sunder { defence, seconds }) => {
                self.fights.sundered.insert(creature, (defence, now + seconds as f64));
            }
            Some(Effect::Daze { seconds }) => {
                self.fights.windups.retain(|w| w.creature != creature);
                self.stagger(creature, seconds);
            }
            None => {}
        }
    }

    /// Holds a creature still for `seconds` from now, its wind-up, if any, undone (a longer
    /// hold it is already in, other than a wind-up, stands).
    fn stagger(&mut self, creature: u32, seconds: f32) {
        let tick = self.tick;
        if let Some(live) = self.creatures.live.get_mut(&creature) {
            live.held_until = if live.winding { tick + ticks(seconds) } else { live.held_until.max(tick + ticks(seconds)) };
            live.winding = false;
            live.tracking = None;
            live.decision.pose = brain::Pose::Idle;
            live.changed = true;
        }
    }

    /// A creature's wound-up blow comes down on a player at `lands`: it lunges at them, then
    /// recovers; what came of it is judged by that moment.
    fn creature_blow(&mut self, creature: u32, player: PlayerId, lands: f64) {
        let feel = self.content.combat.feel;
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let species = live.species;
        let def = &self.content.creatures.get(species).def;
        let Some(attack) = def.attack else { return };
        // It stands to recover: the moment for the player's blows. Its next attack waits
        // out the rest of its `every`, as the brain counts it.
        let recovery = attack.recovery.unwrap_or(feel.creature_recovery).max(0.0);
        let windup = attack.windup.unwrap_or(feel.creature_windup).max(0.0);
        let lunge = attack.lunge.unwrap_or(feel.creature_lunge).max(0.0);
        let tick = self.tick;
        let live = self.creatures.live.get_mut(&creature).expect("checked above");
        // The telegraph ends as the blow comes down, so the wind-up reads as long as it is.
        live.winding = false;
        live.tracking = None;
        live.decision.pose = brain::Pose::Idle;
        live.held_until = live.held_until.max(tick + ticks(recovery));
        live.mind.cooldown = (attack.every - windup - recovery).max(0.0);
        live.changed = true;
        let Some(target) = self.players.get(&player) else { return };
        if !target.health.alive() || target.allow_flight {
            return;
        }
        let (to, look) = (target.position, target.look());
        let size = def.body.size;
        let at = self.lunge(creature, to, lunge);
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let scale = live.scale;
        // As the brain judges reach (`brain::modules::strike`), from where the lunge ended,
        // with slack for the lag.
        let reach = attack.reach + size.0 * scale * 0.5 + 0.3 + feel.reach_slack;
        let flat = (to - at).with_y(0.0).length();
        let tall = (to.y - at.y).abs() < size.1 * scale + 1.5;
        let guard = self.guard_of(player).and_then(|g| g.takes(attack.style, attack.heavy, look, at - to, &feel));
        let fighter = self.fights.fighters.entry(player).or_default();
        let defended = if flat > reach || !tall { Defended::Evaded } else { fighter.guarding.meets(lands, guard.as_ref()) };
        match defended {
            Defended::Dodged => fighter.riposte_until = lands + feel.riposte as f64,
            Defended::Perfect => fighter.counter_until = lands + feel.counter as f64,
            _ => {}
        }
        let damage = match defended {
            Defended::Hit => self.creature_strikes(species, player, 1.0),
            Defended::Blocked => self.creature_strikes(species, player, 1.0 - guard.map_or(0.0, |g| g.stops(attack.style, &feel))),
            Defended::Perfect => {
                // Staggered once it has recovered: the whole stagger is the player's to use.
                self.stagger(creature, recovery + feel.stagger);
                self.defence_xp(player, species);
                Some(0)
            }
            _ => Some(0),
        };
        let (damage, defended) = match damage {
            Some(damage) => (damage, defended),
            None => (0, if defended == Defended::Hit { Defended::Missed } else { defended }),
        };
        if defended == Defended::Hit && damage > 0 {
            // An unguarded hit costs the rhythm.
            if let Some(fighter) = self.fights.fighters.get_mut(&player) {
                fighter.rhythm.lose_pips();
            }
        }
        if let Some(p) = self.players.get(&player) {
            p.connection.send(ServerMessage::Hurt { creature, damage, defended });
        }
    }

    /// How `player` guards with what they hold, if they can.
    fn guard_of(&self, player: PlayerId) -> Option<voxmar_engine::combat::GuardDef> {
        self.content.combat.guard(&self.wielded_by(player)?)
    }

    /// A creature lunges up to `lunge` blocks the way it faces (its aim locked), stopping
    /// where it would touch the player at `to`, or at a wall. Returns where it ends.
    fn lunge(&mut self, creature: u32, to: Vec3, lunge: f32) -> Vec3 {
        let Some(live) = self.creatures.live.get(&creature) else { return to };
        let body = live.body;
        let forward = body.forward();
        let touching = body.size.0 * 0.5 + PLAYER_HALF + self.content.combat.feel.creature_clearance;
        // As far as its nearest pass by the player, short of touching them.
        let along = (to - body.position).with_y(0.0).dot(forward);
        let side = ((to - body.position).with_y(0.0) - forward * along).length();
        let room = (touching * touching - side * side).max(0.0).sqrt();
        let go = lunge.min((along - room).max(0.0));
        let mut at = body.position;
        let steps = (go / LUNGE_STEP).ceil() as u32;
        for i in 1..=steps {
            let next = body.position + forward * (go * i as f32 / steps as f32);
            if Aabb::standing(next, body.size.0, body.size.1).intersects(|p| self.is_solid(p)) {
                break;
            }
            at = next;
        }
        if at != body.position {
            let live = self.creatures.live.get_mut(&creature).expect("checked above");
            live.body.position = at;
            live.changed = true;
        }
        at
    }
}
