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
/// Ticks a charge may make no way against something before it counts as a wall.
const CHARGE_STALL: u8 = 6;
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
    /// A charge that came to the player (`Charging`): judged where it stands, with no
    /// lunge and no new charge.
    contact: bool,
}

/// A creature running a charge it wound up (`creature::moves::Charge`): the way it runs,
/// the blocks left and how fast, how long a wall or a braced point dazes it, the player it
/// was wound up at, and the highest its feet have been since it last stood, for falls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Charging {
    pub player: PlayerId,
    pub along: Vec3,
    pub left: f32,
    pub speed: f32,
    pub stun: f32,
    pub top: f32,
    /// Ticks in a row it has made no way against something: a wall it cannot hop, even
    /// where its head met it first.
    pub stalled: u8,
}

impl Charging {
    /// Where it goes this tick: straight on, facing the way it runs.
    pub(crate) fn drive(&self, scale: f32) -> voxmar_engine::creature::motion::Drive {
        voxmar_engine::creature::motion::Drive { wish: self.along, speed: self.speed * scale.max(0.5), face: Some(self.along) }
    }
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

/// Ticks between pings to each player, for their round trip; a ping unanswered this long
/// is given up and sent again; and the share of each round trip measured the smoothed one
/// moves by.
const PING_EVERY: u64 = TICKS_PER_SECOND as u64;
const PING_LOST: std::time::Duration = std::time::Duration::from_secs(5);
const ROUND_TRIP_SMOOTHING: f32 = 0.25;

fn ticks(seconds: f32) -> u64 {
    (seconds.max(0.0) * TICKS_PER_SECOND as f32).ceil() as u64
}

impl Server {
    fn fight_arrival(&self) -> f64 {
        self.fights.secs(self.arrival)
    }

    /// How `player` guards: with their shield, or their weapon (`CombatRegistry::guard_of`).
    pub(crate) fn guard_of(&self, player: PlayerId) -> Option<voxmar_engine::combat::GuardDef> {
        let p = self.players.get(&player)?;
        self.content.combat.guard_of(p.held(), &p.inventory.worn, |skill| self.content.skills.level(p.skills.xp(skill)))
    }

    /// Pings each player every second (`ServerMessage::Ping`), for their round trip. A
    /// server made to run behind (`ServerConfig::lag`, the gate's) pings every tick while it
    /// does, one ping in flight at a time, so a round trip it has just been put behind by
    /// settles in a few ticks rather than as many seconds; a server for play never lags.
    pub(crate) fn ping_players(&mut self) {
        let lagging = self.config.lag != crate::Lag::default() && self.lagging.load(std::sync::atomic::Ordering::Relaxed);
        if !lagging && !self.tick.is_multiple_of(PING_EVERY) {
            return;
        }
        let number = self.tick as u32;
        for player in self.players.values_mut().filter(|p| p.joined) {
            if player.ping.is_some_and(|(_, sent)| sent.elapsed() < PING_LOST) {
                continue;
            }
            player.connection.send(ServerMessage::Ping { id: number });
            player.ping = Some((number, Instant::now()));
        }
    }

    /// A player's answer to ping `number`, which arrived `at`.
    pub(crate) fn pong(&mut self, player: PlayerId, number: u32, at: Instant) {
        let Some(p) = self.players.get_mut(&player) else { return };
        let Some((_, sent)) = p.ping.filter(|&(id, _)| id == number) else { return };
        let sample = at.saturating_duration_since(sent).as_secs_f32();
        p.round_trip = Some(p.round_trip.map_or(sample, |r| r + (sample - r) * ROUND_TRIP_SMOOTHING));
        p.ping = None;
    }

    /// Seconds a player's guard and dodges are judged earlier by, and their creatures'
    /// blows wait: their round trip (they see a wind-up begin and their answer arrives that
    /// much late), never past `Feel::lag_allowance`.
    fn lag_allowance(&self, player: PlayerId) -> f64 {
        let most = self.content.combat.feel.lag_allowance.max(0.0);
        self.players.get(&player).and_then(|p| p.round_trip).map_or(0.0, |r| r.clamp(0.0, most)) as f64
    }

    fn wielded_by(&self, player: PlayerId) -> Option<Weapon> {
        let p = self.players.get(&player)?;
        Some(self.content.combat.wielded(p.held(), |skill| self.content.skills.level(p.skills.xp(skill))))
    }

    /// A player clicked a creature to strike it (`core:attack`): a swing starts with what
    /// is in hand, or waits for the beat. Nothing while their guard is up or they are eating.
    pub(crate) fn swing_at(&mut self, player: PlayerId, creature: u32) {
        let Some(weapon) = self.wielded_by(player) else { return };
        if self.eating(player) {
            return;
        }
        // A sling or bow with nothing to shoot is no swing at all; the player is told why.
        if self.ammo_slot(player, &weapon) == Some(None) {
            self.no_ammo(player, &weapon);
            return;
        }
        let held = self.players.get(&player).and_then(|p| p.held());
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        let feel = self.content.combat.feel;
        let seen = now - self.lag_allowance(player);
        let fighter = self.fights.fighters.entry(player).or_default();
        if fighter.guarding.is_up() {
            return;
        }
        if fighter.held != held {
            fighter.rhythm.lose_pips();
            fighter.held = held;
        }
        if let Click::Swing(_) = fighter.rhythm.click(now, &timing, &feel) {
            // Windows a perfect answer opened count as of when the player saw it.
            let counter = seen <= fighter.counter_until;
            let riposte = seen <= fighter.riposte_until;
            fighter.counter_until = 0.0;
            fighter.riposte_until = 0.0;
            fighter.aim = Some(Aim { creature, counter, riposte, weapon, held });
        }
    }

    /// For a weapon that shoots: `Some` with the pack slot of the first thing it shoots,
    /// or `Some(None)` with nothing to shoot; `None` for a weapon that strikes.
    fn ammo_slot(&self, player: PlayerId, weapon: &Weapon) -> Option<Option<usize>> {
        let ammo = self.content.combat.ammo(weapon);
        if ammo.is_empty() {
            return None;
        }
        let p = self.players.get(&player)?;
        Some((0..voxmar_engine::inventory::SLOTS).find(|&slot| p.inventory.get(slot).is_some_and(|stack| ammo.contains(&stack.item))))
    }

    /// Tells a player their weapon has nothing to shoot, naming what it shoots.
    fn no_ammo(&self, player: PlayerId, weapon: &Weapon) {
        let (Some(p), Some(&first)) = (self.players.get(&player), self.content.combat.ammo(weapon).first()) else { return };
        let held = p.held().map(|item| voxmar_engine::lang::Words::Block(self.content.blocks.def(item).name.clone()));
        let words = voxmar_engine::lang::Words::key("combat.no_ammo")
            .with("weapon", held.unwrap_or_else(|| voxmar_engine::lang::Words::key("combat.no_ammo.weapon")))
            .with("ammo", voxmar_engine::lang::Words::Block(self.content.blocks.def(first).name.clone()));
        p.connection.send(ServerMessage::Chat { kind: voxmar_protocol::ChatKind::Info, text: words });
    }


    /// A player starts a bite of food (`core:eat`): a swing under way is dropped, and the
    /// meter starts again from empty, so a bite costs a beat (`Rhythm::restart`).
    pub(crate) fn bite_started(&mut self, player: PlayerId) {
        let now = self.fight_arrival();
        let fighter = self.fights.fighters.entry(player).or_default();
        fighter.rhythm.restart(now);
        fighter.aim = None;
    }

    /// A player raised or lowered their guard. Raising it cancels their own wind-up; a raise
    /// soon after lowering opens no perfect window.
    pub(crate) fn raise_guard(&mut self, player: PlayerId, up: bool) {
        let Some(weapon) = self.wielded_by(player) else { return };
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        // Judged as of when the player saw the blow coming (`lag_allowance`).
        let seen = now - self.lag_allowance(player);
        let feel = self.content.combat.feel;
        let fighter = self.fights.fighters.entry(player).or_default();
        if up {
            if fighter.guarding.raise(seen, &feel) && fighter.rhythm.cancel(now, &timing, &feel) {
                fighter.aim = None;
            }
        } else {
            fighter.guarding.lower(seen);
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
        // On the ground where the player is now (a landing told between ticks counts), or
        // left it no longer than the grace ago.
        let mut below = p.aabb();
        below.translate(voxmar_engine::glam::Vec3::new(0.0, -0.05, 0.0));
        let grounded = below.intersects(|cell| self.is_solid(cell)) || self.tick <= p.grounded + ticks(feel.dodge_grace);
        let may = p.health.alive() && !p.flying && grounded && !self.eating(player);
        let timing = self.content.combat.timing(&weapon);
        let now = self.fight_arrival();
        let seen = now - self.lag_allowance(player);
        // The heaviest armour worn sets the dodge; without armour, the first.
        let dodge = self.content.combat.dodge_of(&p.inventory.worn).clone();
        let fighter = self.fights.fighters.entry(player).or_default();
        if !(may && fighter.guarding.dodge(seen, &dodge, &feel)) {
            // Numbered, as every teleport is, so inputs sent before the client saw it are dropped.
            if let Some(p) = self.players.get_mut(&player) {
                p.teleport(p.position);
            }
            return;
        }
        if fighter.rhythm.cancel(now, &timing, &feel) {
            fighter.aim = None;
        }
    }

    /// Undoes a creature's blow on its way, if any (a boss rearing into a new phase).
    pub(crate) fn fights_forget(&mut self, creature: u32) {
        self.fights.windups.retain(|w| w.creature != creature);
        if let Some(live) = self.creatures.live.get_mut(&creature) {
            live.charging = None;
        }
    }

    /// A creature decided to strike a player: it winds up first, showing it, following them
    /// for the first part of it.
    pub(crate) fn wind_up(&mut self, creature: u32, player: PlayerId) {
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let Some(target) = self.players.get(&player) else { return };
        let to = (target.position - live.body.position).with_y(0.0);
        if self.fights.windups.iter().any(|w| w.creature == creature) {
            return;
        }
        // Which of its attacks: a boss's next move that fits its phase and how far away the
        // player stands, from its body's edge (`Creature::choose`).
        let species = self.content.creatures.get(live.species);
        let edge = species.edge(to.length(), live.scale);
        let Some(index) = species.choose(live.phase, edge, live.attack) else { return };
        let Some(attack) = species.attack_at(index) else { return };
        let feel = self.content.combat.feel;
        let windup = attack.windup.unwrap_or(feel.creature_windup).max(0.0);
        let lands = self.fights.now() + windup as f64;
        let tick = self.tick;
        let live = self.creatures.live.get_mut(&creature).expect("checked above");
        // Begin facing the target, before the tracking window can lock the aim. A slow
        // turn from an old wander goal otherwise spends the whole window facing away,
        // especially on a delayed tick. This yaw travels with the first attacking pose.
        if to.x.abs() + to.z.abs() > 1e-4 {
            live.body.yaw = voxmar_engine::creature::motion::yaw_of(to);
        }
        live.winding = true;
        live.attack = index;
        live.held_until = live.held_until.max(tick + ticks(windup));
        live.tracking = Some((player, tick + ticks(windup * feel.creature_tracking.clamp(0.0, 1.0))));
        live.changed = true;
        self.fights.windups.push(Windup { creature, player, lands, contact: false });
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
        let species = self.content.creatures.get(live.species);
        let def = &species.def;
        let to = (target - live.body.position).with_y(0.0);
        let touching = live.body.size.0 * 0.5 + PLAYER_HALF + self.content.combat.feel.creature_clearance;
        let pace = match species.attack_at(live.attack).and_then(|a| a.pace).unwrap_or(self.content.combat.feel.tracking_pace) {
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
        // The clock, or the stamp of the last message heard if that is later: never, over a
        // network, where messages are stamped as they arrive; in process, a test's player
        // can stamp its messages with a game clock that runs as quick as the ticks
        // (`Connection::send_at`), and its swings then land by that clock.
        let now = self.fights.now().max(self.fight_arrival());
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
        // Players' swings. One at the air counts as off the beat: it earns no pip and spends
        // no flourish. A shot needs something to shoot and a clear line as well as reach: one
        // into a wall, or with the last stone gone while it wound up, is at the air.
        let mut landing = Vec::new();
        let (world, blocks, combat) = (&self.world, &self.content.blocks, &self.content.combat);
        let solid = |cell: voxmar_engine::glam::IVec3| world.block(cell).is_some_and(|b| blocks.info(b).solid);
        for (&player, fighter) in &mut self.fights.fighters {
            if let Some(swing) = fighter.rhythm.landed(now) {
                let Some(aim) = fighter.aim.take() else { continue };
                let p = self.players.get(&player);
                let eye = p.map(|p| p.eye());
                let ammo = combat.ammo(&aim.weapon);
                let loaded = ammo.is_empty() || p.is_some_and(|p| (0..voxmar_engine::inventory::SLOTS).any(|slot| p.inventory.get(slot).is_some_and(|stack| ammo.contains(&stack.item))));
                let in_reach = loaded && eye.zip(self.creatures.reach_box(aim.creature, feel.reach_lag)).is_some_and(|(eye, aabb)| {
                    eye.distance(eye.clamp(aabb.min, aabb.max)) <= aim.weapon.reach + feel.reach_slack && (ammo.is_empty() || clear_shot(eye, &aabb, solid))
                });
                let on_beat = in_reach && (swing.on_beat || aim.riposte || aim.counter);
                let kind = fighter.rhythm.blow(&Swing { on_beat, ..swing }, now, &feel);
                landing.push((player, aim, swing.strength, on_beat, kind, in_reach));
            }
        }
        for (player, aim, strength, on_beat, kind, in_reach) in landing {
            self.land_swing(player, aim, strength, on_beat, kind, in_reach);
        }
        // Creatures' blows, each judged at the moment it lands, once every guard and dodge
        // that arrived before that moment has been handled: one that arrived after the last
        // messages were taken in, and before the blow landed, still counts. The player saw
        // the wind-up begin late by their round trip, and their answer arrives as late, so
        // the blow waits that much longer too (`lag_allowance`); their guard and dodges are
        // judged as of when they saw it. A blow so held is judged up to a tick later (and
        // the allowance later), so where the player and the creature stand, for its lunge
        // and reach, is that much later too.
        let heard = self.heard.map_or(now, |at| self.fights.secs(at));
        let windups = std::mem::take(&mut self.fights.windups);
        let (due, waiting): (Vec<Windup>, Vec<Windup>) = windups.into_iter().partition(|w| w.lands + self.lag_allowance(w.player) <= heard);
        self.fights.windups = waiting;
        for windup in due {
            self.creature_blow(windup.creature, windup.player, windup.lands, windup.contact);
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
    fn land_swing(&mut self, player: PlayerId, aim: Aim, strength: f32, on_beat: bool, kind: Blow, in_reach: bool) {
        let feel = self.content.combat.feel;
        let weapon = aim.weapon;
        let mut shot = None;
        // A shot takes one of what the weapon shoots, hit or miss; with none left (gone while
        // it wound up) it is answered as a miss at the air, and the player told why.
        match self.ammo_slot(player, &weapon) {
            Some(None) => {
                self.no_ammo(player, &weapon);
                self.struck(player, aim.creature, 0, false, Blow::Plain);
                return;
            }
            Some(Some(slot)) => {
                if let Some(p) = self.players.get_mut(&player) {
                    shot = p.inventory.get(slot).map(|stack| stack.item);
                    p.inventory.take(slot, 1);
                    p.changed(voxmar_engine::traits::INVENTORY);
                }
            }
            None => {}
        }
        let Some(p) = self.players.get(&player) else { return };
        let falling = p.fall_start.is_some_and(|top| p.position.y < top - feel.falling_drop);
        if !in_reach {
            // Swung at the air: still answered, as a miss.
            self.struck(player, aim.creature, 0, false, Blow::Plain);
            return;
        }
        let tick = self.tick;
        if let Some(p) = self.players.get_mut(&player) {
            p.last_blow = tick;
        }
        let stroke = Stroke::of(kind, strength, on_beat, aim.counter, aim.riposte, falling, &feel);
        let at = self.creatures.live.get(&aim.creature).map(|c| c.body.position);
        // A shot that lands near bees stirs them, hit or miss.
        if let (Some(_), Some(at)) = (shot, at) {
            self.stir_bees(player, at);
        }
        let Some(damage) = self.land_blow(player, aim.creature, &weapon, stroke) else { return };
        // A shot that hit may be left lying by the target, to be picked up again.
        if let (Some(item), Some(at), true) = (shot, at, damage > 0)
            && unit_from(mix(mix(self.tick, aim.creature as u64), 0xa770)) < self.content.combat.recover(&weapon)
        {
            let body = voxmar_engine::dropped::Loose::new(at + Vec3::Y * 0.25, Vec3::ZERO);
            self.items.spawn(voxmar_engine::inventory::ItemStack { item, count: 1 }, body, voxmar_engine::dropped::PICKUP_DELAY);
        }
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
            // A boss shakes off a daze (`combat::Effect::Daze`).
            Some(Effect::Daze { .. }) if self.creatures.live.get(&creature).is_some_and(|c| self.content.creatures.get(c.species).def.boss) => {}
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
            live.charging = None;
            live.decision.pose = brain::Pose::Idle;
            live.changed = true;
        }
    }

    /// A creature's wound-up blow comes down on a player at `lands`: it lunges at them, then
    /// recovers; what came of it is judged by that moment. A charge instead sets off at the
    /// end of its wind-up (`start_charge`), and its blow is judged when it reaches someone
    /// (`contact`).
    fn creature_blow(&mut self, creature: u32, player: PlayerId, lands: f64, contact: bool) {
        let feel = self.content.combat.feel;
        let Some(live) = self.creatures.live.get(&creature) else { return };
        let species = live.species;
        let creature_def = self.content.creatures.get(species);
        let def = &creature_def.def;
        let Some(attack) = creature_def.attack_at(live.attack) else { return };
        if let Some(charge) = attack.charge.filter(|_| !contact) {
            self.start_charge(creature, player, charge);
            return;
        }
        // It stands to recover: the moment for the player's blows. Its next attack waits
        // out the rest of its `every`, as the brain counts it.
        let recovery = attack.recovery.unwrap_or(feel.creature_recovery).max(0.0);
        let windup = attack.windup.unwrap_or(feel.creature_windup).max(0.0);
        let lunge = if contact { 0.0 } else { attack.lunge.unwrap_or(feel.creature_lunge).max(0.0) };
        let tick = self.tick;
        if let Some(p) = self.players.get_mut(&player) {
            p.last_blow = tick;
        }
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
        // A charge that runs onto a braced point is stopped on it, heavy or not.
        if let Some(charge) = attack.charge.filter(|_| contact) {
            let bracing = self.guard_of(player).filter(|g| g.braces(look, at - to));
            let fighter = self.fights.fighters.entry(player).or_default();
            if let Some(brace) = bracing
                && matches!(fighter.guarding.meets(lands, Some(&brace)), Defended::Blocked | Defended::Perfect)
            {
                fighter.counter_until = lands + feel.counter as f64;
                self.braced(creature, player, charge.stun);
                return;
            }
        }
        let guard = self.guard_of(player).and_then(|g| g.takes(attack.style, attack.heavy, look, at - to, &feel));
        let fighter = self.fights.fighters.entry(player).or_default();
        let defended = if flat > reach || !tall { Defended::Evaded } else { fighter.guarding.meets(lands, guard.as_ref()) };
        match defended {
            Defended::Dodged => fighter.riposte_until = lands + feel.riposte as f64,
            Defended::Perfect => fighter.counter_until = lands + feel.counter as f64,
            _ => {}
        }
        let damage = match defended {
            Defended::Hit => self.creature_strikes(creature, species, player, 1.0),
            Defended::Blocked => self.creature_strikes(creature, species, player, 1.0 - guard.map_or(0.0, |g| g.stops(attack.style, &feel))),
            Defended::Perfect => {
                // Staggered once it has recovered: the whole stagger is the player's to use; a
                // boss shakes it off sooner.
                let stagger = if self.content.creatures.get(species).def.boss { feel.boss_stagger } else { feel.stagger };
                self.stagger(creature, recovery + stagger);
                self.defence_xp(player, creature, species);
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

    /// A creature's charge sets off, the way it faces, its wind-up done.
    fn start_charge(&mut self, creature: u32, player: PlayerId, charge: voxmar_engine::creature::moves::Charge) {
        let tick = self.tick;
        let Some(live) = self.creatures.live.get_mut(&creature) else { return };
        live.winding = false;
        live.tracking = None;
        live.decision.pose = brain::Pose::Idle;
        live.charging = Some(Charging { player, along: live.body.forward(), left: charge.distance, speed: charge.speed, stun: charge.stun, top: live.body.position.y, stalled: 0 });
        // Held from thinking for the run, and a little over; `charge_ran` ends it sooner.
        live.held_until = live.held_until.max(tick + ticks(charge.seconds() + 0.5));
        live.changed = true;
    }

    /// A charging creature has moved this tick from `before`: it strikes a player it came
    /// to, is dazed by a wall it ran into, is hurt by a fall it ran off, or runs out.
    pub(crate) fn charge_ran(&mut self, creature: u32, before: Vec3) {
        let feel = self.content.combat.feel;
        let safe_fall = self.survival.safe_fall;
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let Some(live) = self.creatures.live.get_mut(&creature) else { return };
        let Some(mut charge) = live.charging else { return };
        let body = live.body;
        let species = self.content.creatures.get(live.species);
        let (size, scale, most) = (species.def.body.size, live.scale, species.def.health);
        let moved = (body.position - before).with_y(0.0).dot(charge.along).max(0.0);
        charge.left -= moved;
        let stuck = body.against_wall && moved < charge.speed * dt * 0.25;
        charge.stalled = if stuck { charge.stalled.saturating_add(1) } else { 0 };
        // A fall: counted from the highest its feet were since it last stood.
        let mut fell = 0.0;
        if body.on_ground {
            fell = charge.top - body.position.y;
            charge.top = body.position.y;
        } else {
            charge.top = charge.top.max(body.position.y);
        }
        live.charging = Some(charge);
        let fall = fell - safe_fall;
        if fall > 0.0 {
            let damage = ((fall.ceil() * feel.charge_fall * most as f32).round() as u16).max(1);
            self.hurt_creature(charge.player, creature, damage, None, Blow::Plain);
            if !self.creatures.live.get(&creature).is_some_and(|c| c.charging.is_some()) {
                return;
            }
        }
        // The first player in its way: the nearest it has come to.
        let touching = size.0 * scale * 0.5 + PLAYER_HALF + feel.creature_clearance + 0.2;
        let struck = self
            .players
            .values()
            .filter(|p| p.joined && p.health.alive() && !p.allow_flight)
            .filter(|p| {
                let to = (p.position - body.position).with_y(0.0);
                to.length() <= touching && to.dot(charge.along) >= 0.0 && (p.position.y - body.position.y).abs() < size.1 * scale + 1.0
            })
            .min_by(|a, b| (a.position - body.position).with_y(0.0).length_squared().total_cmp(&(b.position - body.position).with_y(0.0).length_squared()))
            .map(|p| p.id);
        if let Some(player) = struck {
            self.end_charge(creature, None);
            let lands = self.fights.now();
            self.fights.windups.push(Windup { creature, player, lands, contact: true });
            return;
        }
        // A wall: what it ran into is too high to hop, or it has made no way for a while
        // (a hop takes a tick or two).
        let rammed = stuck
            && (charge.stalled >= CHARGE_STALL || {
                let mut raised = body.aabb();
                raised.translate(Vec3::Y * 1.05 + charge.along * 0.3);
                raised.intersects(|p| self.is_solid(p))
            });
        if rammed {
            self.end_charge(creature, Some(charge.stun));
        } else if charge.left <= 0.0 && body.on_ground {
            // Run out: once on its feet, so a fall it ran off still counts when it lands.
            self.end_charge(creature, None);
        }
    }

    /// A charge is over: it stands to recover from its blow, or dazed for `stun` seconds.
    fn end_charge(&mut self, creature: u32, stun: Option<f32>) {
        let feel = self.content.combat.feel;
        let tick = self.tick;
        let Some(live) = self.creatures.live.get_mut(&creature) else { return };
        let attack = self.content.creatures.get(live.species).attack_at(live.attack);
        live.charging = None;
        live.body.velocity = live.body.velocity.with_x(0.0).with_z(0.0);
        let recovery = attack.and_then(|a| a.recovery).unwrap_or(feel.creature_recovery).max(0.0);
        let windup = attack.and_then(|a| a.windup).unwrap_or(feel.creature_windup).max(0.0);
        live.held_until = tick + ticks(recovery + stun.unwrap_or(0.0));
        // The rest of its `every` after the wind-up and recovery, as a blow that lands
        // leaves it: a miss, a wall or a braced point costs no more than a hit.
        live.mind.cooldown = attack.map_or(1.0, |a| (a.every - windup - recovery).max(0.0));
        live.changed = true;
    }

    /// A charge ran onto a braced point: the bracer's max hit, sure to land, and the
    /// charger stands dazed `stun` seconds.
    fn braced(&mut self, creature: u32, player: PlayerId, stun: f32) {
        let feel = self.content.combat.feel;
        let Some(weapon) = self.wielded_by(player) else { return };
        let level = self.level(player, weapon.skill);
        let damage = ((weapon.max_hit(level) as f32 * (1.0 + feel.counter_max_hit)).round() as u16).max(1);
        self.end_charge(creature, Some(stun));
        if let Some(p) = self.players.get(&player) {
            p.connection.send(ServerMessage::Hurt { creature, damage: 0, defended: Defended::Perfect });
        }
        self.hurt_creature(player, creature, damage, weapon.skill, Blow::Counter);
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

#[cfg(test)]
mod charges;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use voxmar_engine::combat::Defended;
    use voxmar_protocol::ClientMessage;

    use super::*;

    /// Even a wind-up with no tracking ticks starts aimed at its target, whatever the
    /// creature faced before it chose to attack. Its locked lunge follows that same yaw.
    #[test]
    fn a_wind_up_faces_its_target_before_tracking_can_lock() {
        let mut server = crate::test_server();
        let (_client, connection) = voxmar_protocol::local_pair();
        let player = 1;
        let at = server.spawn + Vec3::Y * 32.0;
        let joined = crate::player::Player::new(player, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        server.players.insert(player, joined);
        let species = server.content.creatures.id("base:brown_bear").expect("a bear");
        let bear = server.spawn_creature(species, at, 1).expect("room");
        std::sync::Arc::get_mut(&mut server.content.combat).expect("the test server owns its combat data").feel.creature_tracking = 0.0;
        for direction in [Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z, Vec3::X + Vec3::Z, Vec3::X - Vec3::Z, -Vec3::X + Vec3::Z, -Vec3::X - Vec3::Z] {
            let direction = direction.normalize();
            let target = at + direction * 4.0;
            server.players.get_mut(&player).unwrap().position = target;
            server.fights.windups.clear();
            let live = server.creatures.live.get_mut(&bear).unwrap();
            live.body.position = at;
            live.body.yaw = voxmar_engine::creature::motion::yaw_of(-direction);
            server.wind_up(bear, player);
            assert!(server.creatures.live[&bear].body.forward().dot(direction) > 0.999, "the wind-up faced away from {direction:?}");
            assert!(server.tracking_drive(bear).is_none(), "the test allowed time to turn");
            let landed = server.lunge(bear, target, 1.0);
            assert!((landed - at).dot(direction) > 0.9, "the locked lunge did not go at the target");
        }
        // No horizontal direction: keep the previous yaw rather than inventing one.
        server.fights.windups.clear();
        server.players.get_mut(&player).unwrap().position = at + Vec3::Y * 2.0;
        let live = server.creatures.live.get_mut(&bear).unwrap();
        live.body.position = at;
        live.body.yaw = 0.7;
        server.wind_up(bear, player);
        assert_eq!(server.creatures.live[&bear].body.yaw, 0.7);
        // Asking again cannot retarget a wind-up already in progress.
        server.players.get_mut(&player).unwrap().position = at + Vec3::X * 4.0;
        server.wind_up(bear, player);
        assert_eq!(server.creatures.live[&bear].body.yaw, 0.7);
        assert_eq!(server.fights.windups.len(), 1);
        // A player who left cannot start a new wind-up.
        server.fights.windups.clear();
        server.creatures.live.get_mut(&bear).unwrap().winding = false;
        server.wind_up(bear, player + 1);
        assert!(server.fights.windups.is_empty());
        assert!(!server.creatures.live[&bear].winding);
    }

    /// A guard raised just before a creature's blow lands counts, even when the server
    /// takes its message in only after the blow was due (a long tick, a stall): the blow
    /// waits to be judged until every message sent before it landed has been heard.
    #[test]
    fn a_guard_raised_before_a_blow_lands_counts_though_heard_after() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let player = 1;
        let mut joined = crate::player::Player::new(player, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        joined.joined = true;
        let sword = server.content.blocks.id("base:iron_sword").expect("a sword");
        joined.inventory.add(sword, 1);
        let attack = server.content.skills.id("core:attack").expect("Attack");
        joined.skills.grant(attack, server.content.skills.xp_for(20));
        let ahead = joined.position + joined.look().with_y(0.0).normalize() * 1.2;
        server.players.insert(player, joined);
        assert!(server.guard_of(player).is_some(), "the sword guards");
        let species = server.content.creatures.id("base:brown_bear").expect("a bear");
        let bear = server.spawn_creature(species, ahead, 1).expect("room");
        server.handle_messages();
        // The guard goes up, and the blow lands just after it, inside the perfect window...
        client.send(ClientMessage::Guard { up: true });
        let raised = server.fights.now();
        server.fights.windups.push(Windup { creature: bear, player, lands: raised + 0.05, contact: false });
        std::thread::sleep(Duration::from_millis(100));
        // ...but the server ticks before it takes the raise in: the blow waits for it.
        server.tick_fights();
        assert_eq!(server.fights.windups.len(), 1, "the blow was judged before the guard raised ahead of it was heard");
        server.handle_messages();
        server.tick_fights();
        assert!(server.fights.windups.is_empty(), "the blow was not judged once heard");
        let defended = std::iter::from_fn(|| client.try_recv().ok().flatten()).find_map(|message| match message {
            ServerMessage::Hurt { defended, .. } => Some(defended),
            _ => None,
        });
        assert_eq!(defended, Some(Defended::Perfect));
    }

    /// One server with a player holding a sword (Attack 20) and a bear a step ahead of
    /// them, for the answers below to be tried on in turn (a fresh server each would be
    /// most of the test's time).
    struct Rig {
        server: Server,
        client: voxmar_protocol::ClientConnection,
        bear: u32,
        ahead: Vec3,
    }

    impl Rig {
        fn new() -> Self {
            let mut server = crate::test_server();
            let (client, connection) = voxmar_protocol::local_pair();
            let player = 1;
            let mut joined = crate::player::Player::new(player, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
            joined.joined = true;
            joined.grounded = server.tick;
            joined.inventory.add(server.content.blocks.id("base:iron_sword").expect("a sword"), 1);
            let attack = server.content.skills.id("core:attack").expect("Attack");
            joined.skills.grant(attack, server.content.skills.xp_for(20));
            let ahead = joined.position + joined.look().with_y(0.0).normalize() * 1.2;
            server.players.insert(player, joined);
            let species = server.content.creatures.id("base:brown_bear").expect("a bear");
            let bear = server.spawn_creature(species, ahead, 1).expect("room");
            Self { server, client, bear, ahead }
        }

        /// The bear's wind-up in seconds.
        fn windup(&self) -> f32 {
            let feel = self.server.content.combat.feel;
            let species = self.server.creatures.live[&self.bear].species;
            self.server.content.creatures.get(species).def.attack.and_then(|a| a.windup).unwrap_or(feel.creature_windup)
        }

        /// What the bear's blow, landing at 10 s of the fight clock, comes to when the
        /// player (whose round trip the server measured as `round_trip`, with the allowance
        /// `allowance` if given) answers it `after` seconds after they saw its wind-up
        /// begin: they see it begin half the round trip late, and their answer arrives half
        /// of it late again. Everything is stamped, nothing waits.
        fn answered(&mut self, round_trip: f32, allowance: Option<f32>, after: f32, dodge: bool) -> Defended {
            let (player, lands) = (1, 10.0);
            if let Some(allowance) = allowance {
                std::sync::Arc::get_mut(&mut self.server.content.combat).expect("the test server owns its combat data").feel.lag_allowance = allowance;
            }
            // As new: no guard up, no dodge under way, the bear where it was, the player well.
            self.server.fights.fighters.clear();
            self.server.creatures.live.get_mut(&self.bear).unwrap().body.position = self.ahead;
            let max = self.server.survival.max_health;
            let p = self.server.players.get_mut(&player).unwrap();
            p.round_trip = Some(round_trip);
            p.health = voxmar_engine::traits::Health::full(max);
            self.server.arrival = self.server.fights.epoch + Duration::from_secs_f64(lands - self.windup() as f64 + after as f64 + round_trip as f64);
            if dodge {
                self.server.dodge(player);
            } else {
                self.server.raise_guard(player, true);
            }
            self.server.creature_blow(self.bear, player, lands, false);
            std::iter::from_fn(|| self.client.try_recv().ok().flatten()).find_map(|message| match message {
                ServerMessage::Hurt { defended, .. } => Some(defended),
                _ => None,
            }).expect("the blow told the player what came of it")
        }
    }

    /// Review: a player sees a wind-up begin half their round trip late, and their answer
    /// comes half of it late again. A guard or dodge is judged that much earlier, by the
    /// round trip the server measures, never past `feel.lag_allowance`: answered on time on
    /// their own screen at 0 or 150 ms, it lands; late, it does not; at 400 ms, past the
    /// allowance, even on time it does not. Without the allowance the 150 ms answers fail.
    #[test]
    fn a_guard_and_a_dodge_count_from_when_the_player_saw_the_wind_up() {
        let mut rig = Rig::new();
        let feel = rig.server.content.combat.feel;
        let perfect = rig.server.guard_of(1).expect("a sword guards").perfect;
        let dodge = rig.server.content.combat.dodge_of(&rig.server.players[&1].inventory.worn).clone();
        let windup = rig.windup();
        // On time on their screen: the middle of the perfect window, and of the dodge's.
        let on_time = [(windup - perfect / 2.0, false, Defended::Perfect), (windup - (dodge.safe.0 + dodge.safe.1) / 2.0, true, Defended::Dodged)];
        for round_trip in [0.0, 0.15, 0.4] {
            let allowed = round_trip <= feel.lag_allowance;
            for (after, dash, defended) in on_time {
                let came = rig.answered(round_trip, None, after, dash);
                if allowed {
                    assert_eq!(came, defended, "an answer on time at {} ms came to {came:?}", round_trip * 1000.0);
                } else {
                    assert!(matches!(came, Defended::Hit | Defended::Blocked), "an answer on time at {} ms, past the allowance, came to {came:?}", round_trip * 1000.0);
                }
                // Late on their screen too: the blow lands.
                let late = rig.answered(round_trip, None, windup + 0.25, dash);
                assert!(matches!(late, Defended::Hit | Defended::Missed), "an answer late at {} ms came to {late:?}", round_trip * 1000.0);
            }
        }
        // Without the allowance the same answers at 150 ms are late.
        for (after, dash, defended) in on_time {
            assert_ne!(rig.answered(0.15, Some(0.0), after, dash), defended, "the 150 ms answer needed no allowance");
        }
    }

    /// The round trip the allowance is made of: a ping answered later by the connection's
    /// round trip, smoothed, then capped by the feel.
    #[test]
    fn the_round_trip_a_ping_measures_is_the_allowance_up_to_its_cap() {
        let mut server = Rig::new().server;
        let player = 1;
        let cap = server.content.combat.feel.lag_allowance;
        assert!(cap > 0.15 && cap < 0.4, "the cases below assume a cap between 150 and 400 ms");
        for (round_trip, allowance) in [(0.15, 0.15), (0.4, cap as f64)] {
            let sent = Instant::now();
            let p = server.players.get_mut(&player).unwrap();
            p.round_trip = None;
            p.ping = Some((7, sent));
            server.pong(player, 7, sent + Duration::from_secs_f64(round_trip));
            assert!((server.lag_allowance(player) - allowance).abs() < 1e-6, "{round_trip}: {}", server.lag_allowance(player));
        }
    }
}

/// Whether nothing `solid` stands between an eye and the middle of `aabb`: a shot needs a
/// clear line where a blow needs only reach.
fn clear_shot(eye: Vec3, aabb: &Aabb, solid: impl Fn(voxmar_engine::glam::IVec3) -> bool) -> bool {
    let centre = (aabb.min + aabb.max) * 0.5;
    let distance = eye.distance(centre);
    voxmar_engine::physics::raycast(eye, centre - eye, distance, solid).is_none_or(|hit| hit.distance >= distance)
}
