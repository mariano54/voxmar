//! How fights feel (`voxmar_engine::combat`, docs/combat.md, the design wiki's "Combat
//! feel"): each player's swings in rhythm, their guard and dodges, creatures winding up
//! before every blow and recovering after, and what flourishes do.
//!
//! A click on a creature starts a swing, or is held for the beat; its blow lands when the
//! wind-up is done, if the creature is still in reach. A creature that decides to attack
//! winds up first, standing still and showing it (the attacking pose); its blow lands only
//! then, and it stands to recover. On a player out of reach the blow falls short; on one
//! mid-dodge it passes through (a perfect dodge, and the next blow is a riposte); a guard
//! facing it stops part, or, raised just in time, all of it, staggering the creature and
//! making the next blow a counter.
//!
//! Times are seconds since the server started, on the clock player messages arrive by
//! (`Server::arrival`), so a busy server neither hurries nor slows anyone's rhythm.

use std::time::Instant;

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::combat::{Blow, Click, Defended, Flourish, Rhythm, Weapon};
use voxmar_engine::creature::brain;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// How far past its reach a blow may still land, for the time a view of the fight lags.
const SLACK: f32 = 0.6;

/// One player's side of fights.
#[derive(Default)]
struct Fighter {
    rhythm: Rhythm,
    /// The swing under way: the creature it is aimed at, and whether it is a counter or a
    /// riposte.
    aim: Option<Aim>,
    /// The item last swung, to lose the pips on a switch.
    held: Option<BlockId>,
    /// When the guard went up, while it is up, whether that raise may be a perfect block,
    /// and when it last came down.
    guard: Option<f64>,
    perfect_ok: bool,
    lowered: f64,
    /// When blows pass through a dodge, and when the next dodge may start.
    dodging: (f64, f64),
    dodge_ready: f64,
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
}

/// A creature's blow on its way.
struct Windup {
    creature: u32,
    player: PlayerId,
    lands: f64,
}

/// Bleeding from a flourish: so much more, a point at a time, a second apart.
struct Bleed {
    creature: u32,
    player: PlayerId,
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
}

/// How a blow is struck, beyond the weapon: from its swing and what made it special.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stroke {
    /// Its strength, 0 to 1, for clicks earlier than the buffer.
    pub strength: f32,
    /// Added to the attack roll, and to the max hit, as shares.
    pub accuracy: f32,
    pub max_hit: f32,
    /// A counter cannot miss.
    pub certain: bool,
    pub blow: Blow,
}

impl Server {
    fn fight_arrival(&self) -> f64 {
        self.fights.secs(self.arrival)
    }

    fn wielded_by(&self, player: PlayerId) -> Option<Weapon> {
        let p = self.players.get(&player)?;
        Some(self.content.combat.wielded(p.held(), |skill| self.content.skills.level(p.skills.xp(skill))))
    }

    /// A player clicked a creature to strike it (`core:attack`): a swing starts, or waits
    /// for the beat. Nothing while their guard is up.
    pub(crate) fn swing_at(&mut self, player: PlayerId, creature: u32) {
        let Some(weapon) = self.wielded_by(player) else { return };
        let held = self.players.get(&player).and_then(|p| p.held());
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        let feel = self.content.combat.feel;
        let fighter = self.fights.fighters.entry(player).or_default();
        if fighter.guard.is_some() {
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
            fighter.aim = Some(Aim { creature, counter, riposte });
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
        match (up, fighter.guard) {
            (true, None) => {
                fighter.guard = Some(now);
                fighter.perfect_ok = now - fighter.lowered >= feel.no_flicker as f64;
                if fighter.rhythm.cancel(now, &timing, &feel) {
                    fighter.aim = None;
                }
            }
            (false, Some(_)) => {
                fighter.guard = None;
                fighter.lowered = now;
            }
            _ => {}
        }
    }

    /// A player dodged (`core:dodge`): for a moment no blow lands on them. Their client
    /// carries the dash itself, within what movement allows. Not while guarding; it
    /// cancels their own wind-up.
    pub(crate) fn dodge(&mut self, player: PlayerId) {
        let Some(weapon) = self.wielded_by(player) else { return };
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        let feel = self.content.combat.feel;
        // Armour weight sets the dodge; without armour, the first.
        let dodge = self.content.combat.dodge(None).clone();
        let fighter = self.fights.fighters.entry(player).or_default();
        // A little early is the client's clock running ahead of the arrival clock.
        if fighter.guard.is_some() || now + 0.1 < fighter.dodge_ready {
            return;
        }
        fighter.dodging = (now + dodge.safe.0 as f64, now + dodge.safe.1 as f64);
        fighter.dodge_ready = now + dodge.cooldown as f64;
        if fighter.rhythm.cancel(now, &timing, &feel) {
            fighter.aim = None;
        }
    }

    /// A creature decided to strike a player: it winds up first, held still and showing it.
    pub(crate) fn wind_up(&mut self, creature: u32, player: PlayerId) {
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let Some(attack) = self.content.creatures.get(live.species).def.attack else { return };
        if self.fights.windups.iter().any(|w| w.creature == creature) {
            return;
        }
        let windup = attack.windup.unwrap_or(self.content.combat.feel.creature_windup).max(0.0);
        let lands = self.fights.now() + windup as f64;
        let ticks = (windup * TICKS_PER_SECOND as f32).ceil() as u64;
        let live = self.creatures.live.get_mut(&creature).expect("checked above");
        live.winding = true;
        live.held_until = live.held_until.max(self.tick + ticks);
        live.changed = true;
        self.fights.windups.push(Windup { creature, player, lands });
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
        // Players' swings.
        let mut landing = Vec::new();
        for (&player, fighter) in &mut self.fights.fighters {
            if let Some(swing) = fighter.rhythm.landed(now) {
                let Some(aim) = fighter.aim.take() else { continue };
                let on_beat = swing.on_beat || aim.riposte || aim.counter;
                let kind = fighter.rhythm.blow(&voxmar_engine::combat::Swing { on_beat, ..swing }, now, aim.counter || aim.riposte, &feel);
                landing.push((player, aim, swing.strength, on_beat, kind));
            }
        }
        for (player, aim, strength, on_beat, kind) in landing {
            self.land_swing(player, aim, strength, on_beat, kind);
        }
        // Creatures' blows.
        let (due, waiting): (Vec<Windup>, Vec<Windup>) = std::mem::take(&mut self.fights.windups).into_iter().partition(|w| w.lands <= now);
        self.fights.windups = waiting;
        for windup in due {
            self.creature_blow(windup.creature, windup.player, now);
        }
        // Bleeding.
        let (due, waiting): (Vec<Bleed>, Vec<Bleed>) = std::mem::take(&mut self.fights.bleeds).into_iter().partition(|b| b.next <= now);
        self.fights.bleeds = waiting;
        for mut bleed in due {
            let Some(weapon) = self.wielded_by(bleed.player) else { continue };
            if self.hurt_creature(bleed.player, bleed.creature, 1, weapon.skill, Blow::Bleed).is_some() {
                bleed.left -= 1;
                bleed.next += bleed.every;
                if bleed.left > 0 {
                    self.fights.bleeds.push(bleed);
                }
            }
        }
    }

    /// A player's swing lands: at the creature it was aimed at, if still in reach, with
    /// what its rhythm made it.
    fn land_swing(&mut self, player: PlayerId, aim: Aim, strength: f32, on_beat: bool, kind: Blow) {
        let feel = self.content.combat.feel;
        let Some(weapon) = self.wielded_by(player) else { return };
        let Some(p) = self.players.get(&player) else { return };
        let eye = p.eye();
        let falling = p.fall_start.is_some_and(|top| p.position.y < top - 0.3);
        let in_reach = self.creatures.reach_box(aim.creature).is_some_and(|aabb| {
            eye.distance(eye.clamp(aabb.min, aabb.max)) <= voxmar_engine::action::REACH_CREATURE + weapon.reach + SLACK
        });
        if !in_reach {
            // Swung at the air: still answered, as a miss.
            p.connection.send(ServerMessage::Struck { creature: aim.creature, damage: 0, killed: false, blow: Blow::Plain });
            return;
        }
        let flourish = kind == Blow::Flourish;
        let mut max_hit = 0.0;
        if flourish {
            max_hit += feel.flourish_max_hit;
        }
        if aim.counter {
            max_hit += feel.counter_max_hit;
        }
        if on_beat && falling {
            max_hit += feel.falling_max_hit;
        }
        let blow = if flourish { Blow::Flourish } else if aim.counter { Blow::Counter } else { kind };
        // A counter or riposte is a full blow, whatever the meter says.
        let strength = if aim.counter || aim.riposte { 1.0 } else { strength };
        let stroke = Stroke { strength, accuracy: if on_beat { feel.beat_accuracy } else { 0.0 }, max_hit, certain: aim.counter, blow };
        let Some(damage) = self.land_blow(player, aim.creature, &weapon, stroke) else { return };
        if flourish && damage > 0 {
            self.flourish(player, aim.creature, &weapon, damage);
        }
    }

    /// What a flourish does besides its max hit, by the weapon's class.
    fn flourish(&mut self, player: PlayerId, creature: u32, weapon: &Weapon, damage: u16) {
        let now = self.fights.now();
        match self.content.combat.flourish(weapon) {
            Some(Flourish::Bleed { share, seconds }) => {
                let left = (damage as f32 * share).round() as u16;
                if left > 0 {
                    let every = (seconds / left as f32).max(0.05) as f64;
                    self.fights.bleeds.push(Bleed { creature, player, left, every, next: now + every });
                }
            }
            Some(Flourish::Cleave { share }) => {
                // The nearest other creature in front, within reach.
                let Some(p) = self.players.get(&player) else { return };
                let (eye, look) = (p.eye(), p.look());
                let reach = voxmar_engine::action::REACH_CREATURE + weapon.reach;
                let other = self
                    .creatures
                    .live
                    .iter()
                    .filter(|&(&id, c)| id != creature && c.health > 0)
                    .filter_map(|(&id, c)| {
                        let aabb = c.body.aabb();
                        let nearest = eye.clamp(aabb.min, aabb.max);
                        let to = (aabb.min + aabb.max) / 2.0 - eye;
                        (eye.distance(nearest) <= reach && to.with_y(0.0).normalize_or_zero().dot(look.with_y(0.0).normalize_or_zero()) > 0.5).then_some((id, eye.distance(nearest)))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                let cleft = (damage as f32 * share).round() as u16;
                if let Some((other, _)) = other.filter(|_| cleft > 0) {
                    self.hurt_creature(player, other, cleft, weapon.skill, Blow::Flourish);
                }
            }
            Some(Flourish::Pin { slow, seconds }) => {
                self.fights.pinned.insert(creature, (slow.clamp(0.0, 1.0), now + seconds as f64));
            }
            Some(Flourish::Sunder { defence, seconds }) => {
                self.fights.sundered.insert(creature, (defence.clamp(0.0, 1.0), now + seconds as f64));
            }
            Some(Flourish::Daze { stagger }) => {
                self.fights.windups.retain(|w| w.creature != creature);
                self.stagger(creature, stagger);
            }
            None => {}
        }
    }

    /// Holds a creature still for `seconds`, its wind-up, if any, undone.
    fn stagger(&mut self, creature: u32, seconds: f32) {
        let ticks = (seconds * TICKS_PER_SECOND as f32).ceil() as u64;
        if let Some(live) = self.creatures.live.get_mut(&creature) {
            live.winding = false;
            live.decision.pose = brain::Pose::Idle;
            live.held_until = live.held_until.max(self.tick + ticks);
            live.changed = true;
        }
    }

    /// A creature's wound-up blow comes down on a player, then it recovers.
    fn creature_blow(&mut self, creature: u32, player: PlayerId, now: f64) {
        let feel = self.content.combat.feel;
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let species = live.species;
        let def = &self.content.creatures.get(species).def;
        let Some(attack) = def.attack else { return };
        let (at, scale, size) = (live.body.position, live.scale, def.body.size);
        // It stands to recover: the moment for the player's blows. Its next attack waits
        // out the rest of its `every`, as the brain counts it.
        let recovery = attack.recovery.unwrap_or(feel.creature_recovery).max(0.0);
        let windup = attack.windup.unwrap_or(feel.creature_windup).max(0.0);
        let live = self.creatures.live.get_mut(&creature).expect("checked above");
        // The telegraph ends as the blow comes down, so the wind-up reads as long as it is.
        live.winding = false;
        live.decision.pose = brain::Pose::Idle;
        live.held_until = live.held_until.max(self.tick + (recovery * TICKS_PER_SECOND as f32).ceil() as u64);
        live.mind.cooldown = (attack.every - windup - recovery).max(0.0);
        live.changed = true;
        let Some(target) = self.players.get(&player) else { return };
        if !target.health.alive() || target.allow_flight {
            return;
        }
        // As the brain judges reach (`brain::modules::strike`), with slack for the lag.
        let reach = attack.reach + size.0 * scale * 0.5 + 0.3 + SLACK;
        let flat = (target.position - at).with_y(0.0).length();
        let tall = (target.position.y - at.y).abs() < size.1 * scale + 1.5;
        let (look, facing) = (target.look(), at - target.position);
        let weapon = self.wielded_by(player).unwrap_or(Weapon::FISTS);
        let guard = self.content.combat.guard(&weapon).filter(|g| !attack.heavy && g.covers(look, facing));
        let fighter = self.fights.fighters.entry(player).or_default();
        let raised = fighter.guard;
        let defended = if flat > reach || !tall {
            Defended::Evaded
        } else if now >= fighter.dodging.0 && now <= fighter.dodging.1 {
            fighter.riposte_until = now + feel.riposte as f64;
            Defended::Dodged
        } else if let (Some(guard), Some(raised)) = (guard, raised) {
            if fighter.perfect_ok && now - raised <= guard.perfect as f64 {
                fighter.counter_until = now + feel.counter as f64;
                Defended::Perfect
            } else {
                Defended::Blocked
            }
        } else {
            Defended::Hit
        };
        let damage = match defended {
            Defended::Hit => self.creature_strikes(species, player, 1.0),
            Defended::Blocked => self.creature_strikes(species, player, 1.0 - guard.map_or(0.0, |g| g.stops(attack.style, &feel))),
            Defended::Perfect => {
                self.stagger(creature, feel.stagger);
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
}
