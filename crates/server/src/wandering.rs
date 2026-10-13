//! Wandering events on the server: parties of people, animals and monsters that come by
//! on their own errands (`wandering.ron`, docs/wandering.md).
//!
//! Every `scan_every` seconds each player gets a chance of one: where the land, the hour
//! and what they have reached suit some event, a cast appears out of sight at the edge of
//! view (so a world stays uncrowded: one party near a player at a time, a gap between
//! them, the creature caps of the land counting them like any other animal or monster),
//! and a script walks them. The script is a plan the leader follows and an errand for each
//! member, handed over every `errand_every` seconds: the same `Errand` villagers follow, so
//! the `Routine` behaviour of the species walks it, and anything else the species does
//! (flee when struck, hunt a player who comes near) still beats the walk. Members keep their
//! formation round the leader by role; a pet circles its owner.
//!
//! Plans find their own way over open ground by probing: a stride of a few blocks ahead
//! toward the goal, turning aside where the ground drops away, rises past a step or is
//! water. A leader that makes no headway for a while detours, and gives up after a few
//! tries. An event ends when its script is done and nobody watches, when all its members
//! are far from every player or gone, or at its lifetime; the members still about then are
//! taken away without a death.
//!
//! Nothing here is saved: a party is a passing thing, and a restart brings no one back.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::brain::{Errand, Pose};
use voxmar_engine::creature::{CreatureId, Kind, mix, unit_from};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::traits;
use voxmar_engine::wander::{Role, Script, ResolvedNeed, ResolvedReward, Once};
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::commands::{Reply, Source};

/// Seconds without headway before a leader turns aside, and the detours it tries.
const STUCK: u64 = 6;
const DETOURS: u8 = 4;
/// How far a stride reaches, and the farthest the ground may rise or fall over it.
const STRIDE: f32 = 6.0;
const STEP: i32 = 2;
/// Blocks from a spot that count as being there.
const THERE: f32 = 2.5;
/// A body further than this from its slot starts to hurry.
const HURRY: f32 = 4.0;
/// Blocks a watcher must be from the party for it to be taken away unseen.
const UNSEEN_BEYOND: f32 = 40.0;

fn ticks(seconds: f32) -> u64 {
    (seconds * TICKS_PER_SECOND as f32).round().max(1.0) as u64
}

/// Every event running, who is in which, and what each player has lately met.
#[derive(Default)]
pub struct Wandering {
    next: u64,
    active: BTreeMap<u64, Instance>,
    by_creature: FxHashMap<u32, (u64, usize)>,
    /// The tick before which no event begins near a player.
    quiet_until: FxHashMap<PlayerId, u64>,
    /// What each player met lately, by event, and when.
    recent: FxHashMap<PlayerId, Vec<(usize, u64)>>,
}

struct Instance {
    id: u64,
    event: usize,
    seed: u64,
    born: u64,
    members: Vec<Member>,
    plan: Plan,
    /// Lines said to a player (by line index), for `Once::Player`, and used up for the
    /// whole party, for `Once::Party`.
    told: FxHashSet<(PlayerId, usize)>,
    spent: FxHashSet<usize>,
    /// The leader's nearest approach to what it walks toward, and when.
    best: (f32, u64),
    /// What `best` was measured toward: a new goal starts the count again.
    aim: Option<Vec3>,
    detour: Option<(Vec3, u64)>,
    lost: u8,
    /// The plan has been walked: members wait about until nobody watches.
    done: bool,
}

struct Member {
    creature: u32,
    group: usize,
    /// Where it keeps to the leader (right, behind), for its role.
    slot: Vec2,
    errand: Option<Errand>,
    /// The tick it picks another spot to stand at, while milling about.
    mill_until: u64,
    mill_at: Vec3,
}

enum Plan {
    /// A line to its goal.
    March { heading: Vec2, goal: Vec3 },
    /// A ring of stops, each paused at, a number of laps.
    Patrol { anchor: Vec3, next: usize, stops: u8, laps_left: u8, wait_until: Option<u64>, turn: f32 },
    /// To a spot, a stay there, then away.
    Camp { spot: Vec3, since: u64, stay_until: Option<u64>, arrive_by: u64, away: f32 },
    /// Keeping to a player, until a tick.
    Shadow { until: u64 },
}

impl Wandering {
    /// Whether creature `id` belongs to a running event.
    pub(crate) fn manages(&self, id: u32) -> bool {
        self.by_creature.contains_key(&id)
    }

    /// The errand creature `id`'s event hands it now, if it is in one.
    pub(crate) fn errand(&self, id: u32) -> Option<Errand> {
        let &(instance, member) = self.by_creature.get(&id)?;
        self.active.get(&instance)?.members.get(member)?.errand
    }

    /// Names of the running events with how many of their cast live, for the admin list.
    pub(crate) fn names(&self) -> Vec<(u64, usize, usize)> {
        self.active.values().map(|i| (i.id, i.event, i.members.len())).collect()
    }

    fn people(&self, server: &Server) -> usize {
        self.by_creature.keys().filter(|id| server.creatures.live.get(id).is_some_and(|c| server.content.creatures.get(c.species).def.kind == Kind::Villager)).count()
    }
}

/// Unit vector `angle` radians off `heading`.
fn turned(heading: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(heading.x * c - heading.y * s, heading.x * s + heading.y * c)
}

fn flat(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

fn xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

fn errand(to: Vec3, last: bool, pose: Pose, face: Option<Vec3>, pace: f32) -> Errand {
    Errand { to, last, pose, face, chase: None, pace }
}

impl Server {
    /// Starts and moves the events, and ends those that are over.
    pub(crate) fn tick_wandering(&mut self) {
        if self.content.wandering.is_empty() {
            return;
        }
        let _span = tracing::info_span!("wandering").entered();
        let rules = *self.content.wandering.rules();
        if self.tick.is_multiple_of(ticks(rules.scan_every)) {
            let ids: Vec<PlayerId> = self.players.values().filter(|p| p.joined).map(|p| p.id).collect();
            for (i, player) in ids.into_iter().enumerate() {
                self.try_wandering(player, i as u64);
            }
        }
        if self.tick.is_multiple_of(ticks(rules.errand_every)) {
            let ids: Vec<u64> = self.wandering.active.keys().copied().collect();
            for id in ids {
                self.update_event(id);
            }
        }
    }

    /// Whether a player could meet a new event now: it is past the first days, they are
    /// not in the gap after the last, and the world and their surroundings have room.
    fn wandering_welcome(&self, player: PlayerId) -> bool {
        let rules = self.content.wandering.rules();
        let Some(p) = self.players.get(&player).filter(|p| p.joined && p.health.alive()) else { return false };
        if self.day / (voxmar_protocol::TICKS_PER_DAY as f64) < rules.first_days as f64 || self.tick < self.wandering.quiet_until.get(&player).copied().unwrap_or(0) {
            return false;
        }
        if self.wandering.active.len() >= rules.most_total {
            return false;
        }
        let near = self
            .wandering
            .active
            .values()
            .filter(|i| i.members.iter().any(|m| self.creatures.live.get(&m.creature).is_some_and(|c| c.body.position.distance(p.position) < rules.near)))
            .count();
        near < rules.most_near
    }

    /// One try at an event near `player`.
    pub(crate) fn try_wandering(&mut self, player: PlayerId, salt: u64) {
        if !self.wandering_welcome(player) {
            return;
        }
        let rules = *self.content.wandering.rules();
        let seed = mix(mix(self.meta.seed, self.tick), salt ^ 0x3a9d);
        if unit_from(mix(seed, 1)) >= rules.chance {
            return;
        }
        let Some(spot) = self.wandering_spot(player, seed) else { return };
        let Some(event) = self.pick_event(player, spot, mix(seed, 5)) else { return };
        self.begin_event(Some(player), event, spot, mix(seed, 6));
    }

    /// A place at the edge of the player's view, out of sight, a body can stand on.
    fn wandering_spot(&self, player: PlayerId, seed: u64) -> Option<IVec3> {
        let rules = self.content.wandering.rules();
        let at = self.players.get(&player)?.position;
        let angle = unit_from(mix(seed, 2)) * std::f32::consts::TAU;
        let distance = rules.appear_near + unit_from(mix(seed, 3)) * (rules.appear_far - rules.appear_near);
        let (x, z) = ((at.x + angle.cos() * distance).floor() as i32, (at.z + angle.sin() * distance).floor() as i32);
        if !self.world.contains(ColumnPos::of_block(IVec3::new(x, 0, z))) {
            return None;
        }
        let spot = self.standing_spot(x, z, at.y.floor() as i32)?;
        if self.is_wet(spot) || (spot.y as f32 - at.y).abs() > 24.0 || self.seen_by_a_player(spot, self.content.creatures.limits().spawn_unseen_angle) {
            return None;
        }
        Some(spot)
    }

    /// Whether `cell` is water, or stands on it.
    fn is_wet(&self, cell: IVec3) -> bool {
        self.is_water(cell) || self.is_water(cell - IVec3::Y)
    }

    /// An event that suits `spot` and `player`, by its weights: its biome, the hour, what
    /// the player has reached, and a lesser chance for one they met lately.
    fn pick_event(&self, player: PlayerId, spot: IVec3, seed: u64) -> Option<usize> {
        let rules = self.content.wandering.rules();
        let p = self.players.get(&player)?;
        let site = self.overworld.site(spot.x, spot.z);
        let biome = self.overworld.biomes().get(site.biome);
        let daylight = self.daylight();
        let days = (self.day / voxmar_protocol::TICKS_PER_DAY as f64) as f32;
        let combat = self.combat_level(player);
        let recent = self.wandering.recent.get(&player);
        let mut choices: Vec<(usize, f32)> = Vec::new();
        for (index, event) in self.content.wandering.iter() {
            let needs = &event.def.needs;
            let suits = event.suits_light(daylight)
                && days >= needs.days
                && needs.home.is_none_or(|h| h == p.home.is_some())
                && needs.combat.is_none_or(|(low, high)| (low..=high).contains(&combat));
            let weight = event.biome_weight(&biome.name, &biome.tags);
            if !suits || weight <= 0.0 {
                continue;
            }
            let lately = recent.is_some_and(|r| r.iter().any(|&(e, at)| e == index && self.tick < at + ticks(rules.repeat_memory)));
            choices.push((index, event.def.weight * weight * if lately { rules.repeat_weight } else { 1.0 }));
        }
        let total: f32 = choices.iter().map(|c| c.1).sum();
        let mut pick = unit_from(seed) * total;
        choices.iter().find(|c| {
            pick -= c.1;
            pick <= 0.0
        }).map(|c| c.0)
    }

    /// Whether another of species `species` fits: people have their own budget, animals
    /// and monsters the creature caps of the land near every player.
    fn wandering_room(&self, species: CreatureId, at: Vec3) -> bool {
        let def = &self.content.creatures.get(species).def;
        let limits = self.content.creatures.limits();
        match def.kind {
            Kind::Villager => self.wandering.people(self) < self.content.wandering.rules().people_most,
            kind => {
                let monster = kind == Kind::Monster;
                let cap = if monster { self.monsters_cap() } else { limits.animals_near };
                self.wild_room() > 0 && self.nearby_room(at, monster, cap)
            }
        }
    }

    /// Brings a cast to `spot` and starts its script, for `player` (or for no one, from an
    /// admin's command). Returns the event's id, or `None` when the leader could not stand
    /// there or there was no room.
    pub(crate) fn begin_event(&mut self, player: Option<PlayerId>, event: usize, spot: IVec3, seed: u64) -> Option<u64> {
        self.begin_event_at(player, event, spot, seed, false)
    }

    /// `begin_event`, where `visible` lets the cast appear in a player's view.
    pub(crate) fn begin_event_at(&mut self, player: Option<PlayerId>, event: usize, spot: IVec3, seed: u64, visible: bool) -> Option<u64> {
        let rules = *self.content.wandering.rules();
        let def = self.content.wandering.get(event);
        let (script, groups) = (def.def.script, def.groups.clone());
        let at = spot.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        let angle = if visible { 0.0 } else { self.content.creatures.limits().spawn_unseen_angle };
        let mut members: Vec<Member> = Vec::new();
        for (g, (species, group)) in groups.iter().enumerate() {
            let (low, high) = group.count;
            let count = low + (unit_from(mix(seed, 10 + g as u64)) * (high - low + 1) as f32) as u8;
            for i in 0..count.min(high) {
                let stand = if members.is_empty() {
                    at
                } else {
                    let a = unit_from(mix(seed, 100 + (g * 16 + i as usize) as u64)) * std::f32::consts::TAU;
                    let r = 1.5 + 2.5 * unit_from(mix(seed, 200 + (g * 16 + i as usize) as u64));
                    let Some(cell) = self.standing_spot((at.x + a.cos() * r).floor() as i32, (at.z + a.sin() * r).floor() as i32, spot.y) else { continue };
                    cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5)
                };
                if !self.wandering_room(*species, stand) {
                    continue;
                }
                let member_seed = mix(seed, 1000 + (g * 16 + i as usize) as u64);
                let Some(creature) = self.place_creature(*species, stand, member_seed, angle) else { continue };
                // The rest of the leader's own group walk behind it.
                let role = if group.role == Role::Lead && !members.is_empty() { Role::Follow } else { group.role };
                members.push(Member { creature, group: g, slot: voxmar_engine::wander::slot(role, i as usize, count as usize, member_seed), errand: None, mill_until: 0, mill_at: stand });
            }
            // Without its leader a party is nothing.
            if g == 0 && members.is_empty() {
                return None;
            }
        }
        let id = self.wandering.next;
        self.wandering.next += 1;
        let target = player.and_then(|p| self.players.get(&p)).map(|p| p.position);
        let plan = self.plan_for(script, at, target, seed);
        for (i, member) in members.iter().enumerate() {
            self.wandering.by_creature.insert(member.creature, (id, i));
        }
        let born = self.tick;
        self.wandering.active.insert(id, Instance { id, event, seed, born, members, plan, told: Default::default(), spent: Default::default(), best: (f32::MAX, born), aim: None, detour: None, lost: 0, done: false });
        if let Some(player) = player {
            self.wandering.quiet_until.insert(player, self.tick + ticks(rules.gap));
            let recent = self.wandering.recent.entry(player).or_default();
            recent.retain(|&(_, at)| self.tick < at + ticks(rules.repeat_memory));
            recent.push((event, self.tick));
        }
        tracing::info!("wandering: {} begins at {spot}", self.content.wandering.get(event).def.name);
        self.update_event(id);
        Some(id)
    }

    /// The plan a script gives a party that stands at `at`, with a player at `target`.
    fn plan_for(&self, script: Script, at: Vec3, target: Option<Vec3>, seed: u64) -> Plan {
        let roll = |salt: u64| unit_from(mix(seed, salt));
        let towards = target.map_or_else(|| turned(Vec2::X, roll(30) * std::f32::consts::TAU), |t| (xz(t) - xz(at)).try_normalize().unwrap_or(Vec2::X));
        let side = if roll(31) < 0.5 { 1.0 } else { -1.0 };
        match script {
            Script::March { pass, length, .. } => {
                let distance = target.map_or(60.0, |t| flat(t, at)).max(8.0);
                let nearest = pass.0 + roll(32) * (pass.1 - pass.0);
                let heading = turned(towards, side * (nearest / distance).clamp(0.0, 0.95).asin());
                Plan::March { heading, goal: at + Vec3::new(heading.x, 0.0, heading.y) * length }
            }
            Script::Patrol { stops, laps, .. } => Plan::Patrol { anchor: at, next: 0, stops, laps_left: laps, wait_until: None, turn: side * std::f32::consts::TAU / stops as f32 },
            Script::Camp { near, stay, away, .. } => {
                let distance = target.map_or(60.0, |t| flat(t, at));
                let ahead = (distance - near).max(10.0);
                let spot = at + Vec3::new(towards.x, 0.0, towards.y) * ahead;
                let arrive_by = self.tick + ticks(ahead / 1.2 + 60.0);
                let _ = stay;
                Plan::Camp { spot, since: self.tick, stay_until: None, arrive_by, away }
            }
            Script::Shadow { seconds, .. } => Plan::Shadow { until: self.tick + ticks(seconds) },
        }
    }

    /// A point `reach` blocks from `from` toward `heading`, turning aside up to a right
    /// angle either way where the ground will not do, that a body can stand on.
    fn stride(&self, from: Vec3, heading: Vec2, reach: f32, bias: f32) -> Option<(Vec3, Vec2)> {
        let order = [0.0f32, 0.35, -0.35, 0.7, -0.7, 1.1, -1.1, 1.57, -1.57];
        for turn in order {
            let turn = turn * bias;
            let direction = turned(heading, turn);
            let at = |far: f32| {
                let p = xz(from) + direction * far;
                let cell = self.standing_spot(p.x.floor() as i32, p.y.floor() as i32, from.y.floor() as i32)?;
                ((cell.y - from.y.floor() as i32).abs() <= STEP && !self.is_wet(cell)).then_some(cell)
            };
            // Both halves of the stride must be ground, or a one-block fence is a wall.
            let (Some(half), Some(end)) = (at(reach * 0.5), at(reach)) else { continue };
            let _ = half;
            return Some((end.as_vec3() + Vec3::new(0.5, 0.0, 0.5), direction));
        }
        None
    }

    /// One update of event `id`: ends it if over, else walks its plan and hands every
    /// member an errand.
    fn update_event(&mut self, id: u64) {
        let Some(mut inst) = self.wandering.active.remove(&id) else { return };
        let rules = *self.content.wandering.rules();
        // Those gone are gone.
        inst.members.retain(|m| self.creatures.live.contains_key(&m.creature));
        let live: Vec<u32> = inst.members.iter().map(|m| m.creature).collect();
        self.wandering.by_creature.retain(|c, (i, _)| *i != id || live.contains(c));
        for (i, m) in inst.members.iter().enumerate() {
            self.wandering.by_creature.insert(m.creature, (id, i));
        }
        let players: Vec<(PlayerId, Vec3)> = self.players.values().filter(|p| p.joined).map(|p| (p.id, p.position)).collect();
        let positions: Vec<Vec3> = inst.members.iter().filter_map(|m| self.creatures.live.get(&m.creature).map(|c| c.body.position)).collect();
        let over = positions.is_empty()
            || self.tick >= inst.born + ticks(self.content.wandering.get(inst.event).def.lifetime)
            || players.iter().all(|(_, p)| positions.iter().all(|c| c.distance(*p) > rules.leave));
        let unwatched = players.iter().all(|(_, p)| positions.iter().all(|c| c.distance(*p) > UNSEEN_BEYOND));
        if over || (inst.done && unwatched) {
            self.end_event(inst);
            return;
        }
        self.walk_plan(&mut inst, &players);
        self.wandering.active.insert(id, inst);
    }

    /// Takes the cast away, those not dead, without a death.
    fn end_event(&mut self, inst: Instance) {
        tracing::info!("wandering: {} ends", self.content.wandering.get(inst.event).def.name);
        for member in &inst.members {
            self.wandering.by_creature.remove(&member.creature);
            self.creatures.take_away(member.creature);
        }
    }

    /// Ends every event at once (an admin's `/wander end`).
    pub(crate) fn end_all_events(&mut self) -> usize {
        let ids: Vec<u64> = self.wandering.active.keys().copied().collect();
        for &id in &ids {
            if let Some(inst) = self.wandering.active.remove(&id) {
                self.end_event(inst);
            }
        }
        ids.len()
    }

    /// Moves the plan on and sets each member's errand.
    fn walk_plan(&mut self, inst: &mut Instance, players: &[(PlayerId, Vec3)]) {
        let event = self.content.wandering.get(inst.event);
        let (script, groups) = (event.def.script, event.groups.clone());
        let leader = inst.members[0].creature;
        let lead = self.creatures.live[&leader].body.position;
        let nearest = players.iter().map(|&(_, p)| p).min_by(|a, b| a.distance(lead).total_cmp(&b.distance(lead)));
        let night = self.daylight() < 0.3;
        // Where the leader goes, and how it is once there.
        let (mut goal, mut pose, mut face, mut heading, mut last): (Option<Vec3>, Pose, Option<Vec3>, Vec2, bool) = (None, Pose::Idle, None, Vec2::new(1.0, 0.0), false);
        let mut pace = 1.0;
        match (&mut inst.plan, script) {
            (Plan::March { heading: h, goal: g }, Script::March { pace: p, .. }) => {
                heading = *h;
                pace = p;
                if flat(lead, *g) < THERE * 2.0 {
                    inst.done = true;
                    last = true;
                } else {
                    goal = Some(*g);
                }
            }
            (Plan::Patrol { anchor, next, stops, laps_left, wait_until, turn }, Script::Patrol { radius, pause, .. }) => {
                let angle = |k: usize| *turn * k as f32;
                let stop = *anchor + Vec3::new(angle(*next).cos(), 0.0, angle(*next).sin()) * radius;
                heading = (xz(stop) - xz(lead)).try_normalize().unwrap_or(heading);
                match *wait_until {
                    Some(until) if self.tick < until => {
                        last = true;
                        pose = Pose::Alert;
                        face = Some(lead + Vec3::new(heading.x, 0.0, heading.y) * 4.0);
                    }
                    Some(_) => {
                        *wait_until = None;
                        *next += 1;
                        if *next >= *stops as usize {
                            *next = 0;
                            *laps_left -= 1;
                            if *laps_left == 0 {
                                // Done with the round: march off, away from whoever is nearest.
                                let away = nearest.map_or(heading, |p| (xz(lead) - xz(p)).try_normalize().unwrap_or(heading));
                                inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 80.0 };
                            }
                        }
                    }
                    None if flat(lead, stop) < THERE => {
                        *wait_until = Some(self.tick + ticks(pause.0 + unit_from(mix(inst.seed, *next as u64 * 7 + inst.born)) * (pause.1 - pause.0)));
                        last = true;
                    }
                    None => goal = Some(stop),
                }
            }
            (Plan::Camp { spot, since, stay_until, arrive_by, away }, Script::Camp { stay, .. }) => {
                heading = (xz(*spot) - xz(lead)).try_normalize().unwrap_or(heading);
                match *stay_until {
                    None if flat(lead, *spot) < THERE * 1.5 || self.tick >= *arrive_by => {
                        *stay_until = Some(self.tick + ticks(stay.0 + unit_from(mix(inst.seed, *since)) * (stay.1 - stay.0)));
                        last = true;
                    }
                    None => goal = Some(*spot),
                    Some(until) if self.tick < until => {
                        // Camped: the leader sits, if it is night; the rest mill about.
                        last = true;
                        pose = if night { Pose::Resting } else { Pose::Idle };
                    }
                    Some(_) => {
                        let away_heading = turned(Vec2::X, unit_from(mix(inst.seed, 77)) * std::f32::consts::TAU);
                        let length = *away;
                        inst.plan = Plan::March { heading: away_heading, goal: *spot + Vec3::new(away_heading.x, 0.0, away_heading.y) * length };
                    }
                }
            }
            (Plan::Shadow { until }, Script::Shadow { keep, .. }) => match nearest {
                Some(p) if self.tick < *until => {
                    let d = flat(lead, p);
                    heading = (xz(p) - xz(lead)).try_normalize().unwrap_or(heading);
                    let watched = players.iter().any(|&(id, _)| self.players.get(&id).is_some_and(|pl| {
                        let to = (lead + Vec3::Y * 1.5 - pl.eye()).normalize_or_zero();
                        pl.look().dot(to) > 0.94
                    }));
                    face = Some(p);
                    if watched || (keep.0..=keep.1).contains(&d) {
                        // Still when looked at, and still where it keeps its distance.
                        last = true;
                        pose = Pose::Alert;
                    } else if d > keep.1 {
                        goal = Some(p);
                    } else {
                        goal = Some(lead - Vec3::new(heading.x, 0.0, heading.y) * 8.0);
                    }
                }
                _ => {
                    let away = nearest.map_or(heading, |p| (xz(lead) - xz(p)).try_normalize().unwrap_or(heading));
                    inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 80.0 };
                }
            },
            _ => {}
        }
        // The leader's errand, by a stride toward its goal.
        let mut errands: Vec<Option<Errand>> = vec![None; inst.members.len()];
        let stride_to = |this: &Self, inst: &mut Instance, goal: Vec3| -> Option<Vec3> {
            let toward = (xz(goal) - xz(lead)).try_normalize()?;
            // Detours, when it has made no headway.
            if let Some((via, until)) = inst.detour
                && this.tick < until
            {
                let h = (xz(via) - xz(lead)).try_normalize().unwrap_or(toward);
                return this.stride(lead, h, STRIDE, 1.0).map(|s| s.0);
            }
            inst.detour = None;
            let bias = if unit_from(mix(inst.seed, inst.lost as u64)) < 0.5 { 1.0 } else { -1.0 };
            this.stride(lead, toward, STRIDE.min(flat(lead, goal)).max(2.0), bias).map(|s| s.0)
        };
        if let Some(goal) = goal {
            // Headway: the nearest it has come to the goal; none for a while is being stuck.
            let d = flat(lead, goal);
            if inst.aim.is_none_or(|aim| flat(aim, goal) > 3.0) {
                (inst.aim, inst.best, inst.lost, inst.detour) = (Some(goal), (d, self.tick), 0, None);
            }
            if d + 1.0 < inst.best.0 {
                inst.best = (d, self.tick);
            } else if self.tick > inst.best.1 + ticks(STUCK as f32) {
                inst.lost += 1;
                inst.best = (d, self.tick);
                let side = if unit_from(mix(inst.seed, 900 + inst.lost as u64)) < 0.5 { 1.5 } else { -1.5 };
                let aside = turned((xz(goal) - xz(lead)).try_normalize().unwrap_or(heading), side);
                inst.detour = Some((lead + Vec3::new(aside.x, 0.0, aside.y) * 14.0, self.tick + ticks(6.0)));
                if inst.lost > DETOURS {
                    inst.done = true;
                }
            }
            if !inst.done {
                match stride_to(self, inst, goal) {
                    Some(to) => errands[0] = Some(errand(to, false, Pose::Idle, None, pace)),
                    None => {
                        // Boxed in: try the other way round next time.
                        inst.best.1 = 0;
                    }
                }
            }
        }
        if inst.done && goal.is_none() && !last {
            last = true;
        }
        if last || errands[0].is_none() {
            errands[0] = Some(errand(lead, true, pose, face, 1.0));
        }
        let forward = Vec3::new(heading.x, 0.0, heading.y);
        let right = Vec3::new(heading.y, 0.0, -heading.x);
        let owner_of = |inst: &Instance, group: usize| inst.members.iter().position(|m| m.group == group).unwrap_or(0);
        for i in 1..inst.members.len() {
            let (group, slot) = (inst.members[i].group, inst.members[i].slot);
            let role = groups[group].1.role;
            let me = self.creatures.live[&inst.members[i].creature].body.position;
            let out = match role {
                Role::Free => None,
                Role::Lead | Role::Follow | Role::Flank | Role::Rear | Role::Ahead => {
                    let to = lead - forward * slot.y + right * slot.x;
                    let apart = flat(me, to);
                    let there = apart < 1.2;
                    Some(errand(to, there, if last { pose } else { Pose::Idle }, face, if apart > HURRY { 1.0 + ((apart - HURRY) / 8.0).min(0.6) } else { 1.0 }))
                }
                Role::Pet(owner) => {
                    let boss = owner_of(inst, owner);
                    let b = self.creatures.live[&inst.members[boss].creature].body.position;
                    // Circles its owner slowly, and keeps up at a trot.
                    let theta = unit_from(mix(inst.seed, i as u64)) * std::f32::consts::TAU + self.tick as f32 / TICKS_PER_SECOND as f32 * 0.3;
                    let to = b + Vec3::new(theta.cos(), 0.0, theta.sin()) * (1.6 + slot.x.abs());
                    let apart = flat(me, to);
                    Some(errand(to, apart < 0.8, Pose::Idle, Some(b), if apart > 2.5 { 1.0 + ((apart - 2.5) / 6.0).min(1.2) } else { 1.0 }))
                }
            };
            errands[i] = out;
            // Milling about, at a camp: now and then another spot of it.
            if let (Plan::Camp { spot, stay_until: Some(until), .. }, Script::Camp { mill, .. }, Role::Follow | Role::Flank | Role::Rear | Role::Ahead) = (&inst.plan, script, role)
                && self.tick < *until
            {
                let member = &mut inst.members[i];
                if self.tick >= member.mill_until {
                    let a = unit_from(mix(inst.seed, self.tick ^ (i as u64) << 20)) * std::f32::consts::TAU;
                    let r = unit_from(mix(inst.seed, self.tick ^ (i as u64) << 24)) * mill;
                    let at = *spot + Vec3::new(a.cos(), 0.0, a.sin()) * r;
                    member.mill_at = at;
                    member.mill_until = self.tick + ticks(6.0 + 10.0 * unit_from(mix(inst.seed, self.tick ^ (i as u64) << 28)));
                }
                let there = flat(me, inst.members[i].mill_at) < 1.0;
                errands[i] = Some(errand(inst.members[i].mill_at, there, if night && there { Pose::Resting } else { Pose::Idle }, Some(*spot), 1.0));
            }
        }
        for (member, errand) in inst.members.iter_mut().zip(errands) {
            member.errand = errand;
        }
    }

    /// `player` right-clicks creature `id`: if it speaks for a wandering party, it answers
    /// (and gives), and the click is spent. Returns whether it did.
    pub(crate) fn meet(&mut self, player: PlayerId, id: u32) -> bool {
        let Some(&(instance, member)) = self.wandering.by_creature.get(&id) else { return false };
        let Some(inst) = self.wandering.active.get(&instance) else { return false };
        let event = self.content.wandering.get(inst.event);
        let Some(speaker) = event.speaker().filter(|&who| inst.members.get(member).is_some_and(|m| m.group == who)) else { return false };
        let _ = speaker;
        let Some(p) = self.players.get(&player).filter(|p| p.health.alive()) else { return false };
        let held = p.inventory.get(p.selected);
        let (levels, has_home, hurt) = (
            |skill| self.content.skills.level(p.skills.xp(skill)),
            p.home.is_some(),
            p.health.current < p.health.max,
        );
        let line = event.lines.iter().enumerate().find(|(i, line)| {
            let used = match line.once {
                Once::Always => false,
                Once::Player => inst.told.contains(&(player, *i)),
                Once::Party => inst.spent.contains(i),
            };
            !used
                && line.needs.iter().all(|need| match *need {
                    ResolvedNeed::Skill(skill, level) => levels(skill) >= level,
                    ResolvedNeed::Home(want) => has_home == want,
                    ResolvedNeed::Hurt => hurt,
                })
                && line.wants.is_none_or(|(item, count)| held.is_some_and(|h| h.item == item && h.count >= count))
        });
        let (name, line_gives, line_wants, index) = match line {
            Some((i, line)) => (event.key(&line.name), line.gives.clone(), line.wants, Some(i)),
            None => (event.key("greet"), Vec::new(), None, None),
        };
        let once = index.map(|i| event.lines[i].once);
        let species = self.creatures.live.get(&id).map(|c| c.species);
        let creature = species.map(|s| {
            let def = &self.content.creatures.get(s).def;
            Words::Own(def_key("creature", &def.name), def.title.clone())
        });
        let inst = self.wandering.active.get_mut(&instance).expect("checked");
        match (once, index) {
            (Some(Once::Player), Some(i)) => {
                inst.told.insert((player, i));
            }
            (Some(Once::Party), Some(i)) => {
                inst.spent.insert(i);
            }
            _ => {}
        }
        // What the player hands over, and what they are given.
        if let Some((_, count)) = line_wants {
            let p = self.players.get_mut(&player).expect("checked");
            let selected = p.selected;
            p.inventory.take(selected, count);
            p.changed(traits::INVENTORY);
        }
        for gift in line_gives {
            match gift {
                ResolvedReward::Item(item, (low, high)) => {
                    let count = low + (unit_from(mix(self.tick, id as u64 ^ item.0 as u64)) * (high - low + 1) as f32) as u8;
                    let p = self.players.get_mut(&player).expect("checked");
                    let left = p.inventory.add(item, count.min(high));
                    p.changed(traits::INVENTORY);
                    if left > 0 {
                        let body = voxmar_engine::dropped::thrown(p.eye(), p.look());
                        self.items.spawn(voxmar_engine::inventory::ItemStack { item, count: left }, body, voxmar_engine::dropped::PICKUP_DELAY);
                    }
                }
                ResolvedReward::Xp(skill, xp) => self.grant_xp(player, skill, xp),
                ResolvedReward::Effect(effect, seconds) => {
                    let p = self.players.get_mut(&player).expect("checked");
                    p.give_effects(&[(effect, seconds)]);
                    p.changed(traits::VITALS);
                }
                ResolvedReward::Heal(health) => {
                    let p = self.players.get_mut(&player).expect("checked");
                    p.health.heal(health);
                    p.changed(traits::HEALTH);
                }
            }
        }
        if let (Some(who), Some(p)) = (creature, self.players.get(&player)) {
            p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&name)) });
        }
        true
    }

    /// `/wander`: the parties about, the kinds there are, or (admin) start one ahead of the
    /// player or send every party away.
    pub(crate) fn wander_command(&mut self, source: Source, args: &[String]) -> Reply {
        let registry = self.content.wandering.clone();
        match args.first().map(String::as_str) {
            None => {
                let lines: Vec<Words> = self
                    .wandering
                    .names()
                    .into_iter()
                    .map(|(_, event, members)| Words::key("command.wander.line").with("event", Words::key(&registry.get(event).title_key())).with("count", Words::text(members)))
                    .collect();
                if lines.is_empty() {
                    return Ok(Words::key("command.wander.none"));
                }
                Ok(Words::Lines(std::iter::once(Words::key("command.wander.title").with("count", Words::text(lines.len()))).chain(lines).collect()))
            }
            Some("list") => Ok(Words::Lines(
                std::iter::once(Words::key("command.wander.kinds").with("count", Words::text(registry.len())))
                    .chain(registry.iter().map(|(_, e)| Words::key("command.wander.kind").with("name", e.def.name.as_str()).with("event", Words::key(&e.title_key()))))
                    .collect(),
            )),
            Some("end") => Ok(Words::key("command.wander.ended").with("count", Words::text(self.end_all_events()))),
            Some("start") => {
                let name = args.get(1).ok_or_else(|| Words::key("command.wander.what"))?;
                let event = registry.find(name).ok_or_else(|| Words::key("command.wander.unknown").with("name", name.as_str()))?;
                let (player, feet, look) = match source {
                    Source::Player(p) => {
                        let player = self.players.get(&p).ok_or_else(|| Words::key("command.wander.what"))?;
                        (Some(p), player.position, player.look().with_y(0.0).normalize_or(Vec3::NEG_Z))
                    }
                    Source::Console => (None, self.spawn, Vec3::NEG_Z),
                };
                let at = (feet + look * 40.0).floor().as_ivec3();
                self.load_area(at, at);
                let spot = self.standing_spot(at.x, at.z, at.y).ok_or_else(|| Words::key("command.wander.no_room"))?;
                // Seen or not: the admin asked for it.
                let seed = mix(self.meta.seed, self.tick);
                self.begin_event_at(player, event, spot, seed, true).ok_or_else(|| Words::key("command.wander.no_room"))?;
                Ok(Words::key("command.wander.started").with("event", Words::key(&registry.get(event).title_key())))
            }
            Some(_) => Err(Words::key("command.wander.what")),
        }
    }

    /// A wandering party member died: the rest of the party may mind (not yet).
    pub(crate) fn wandering_died(&mut self, id: u32) {
        self.wandering.by_creature.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::block::BlockId;
    use voxmar_engine::world::Column;
    use voxmar_protocol::TICKS_PER_DAY;

    /// A flat stone floor over 256 by 256 blocks at y 199 with air above, a morning, and a
    /// player standing in the middle: marches, patrols and meetings need no generated land.
    fn fixture() -> Server {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        for cx in 0..8 {
            for cz in 0..8 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        column.set_block(IVec3::new(x, 199, z), stone);
                        column.set_block(IVec3::new(x, 200, z), BlockId::AIR);
                    }
                }
                server.world.insert(ColumnPos::new(cx, cz), std::sync::Arc::new(column));
            }
        }
        server.day = TICKS_PER_DAY as f64 * 0.8;
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let mut player = crate::player::Player::new(1, connection, Vec3::new(128.5, 200.0, 128.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.name) = (true, "Tëster".into());
        server.players.insert(1, player);
        server
    }

    fn event(server: &Server, name: &str) -> usize {
        server.content.wandering.find(name).unwrap_or_else(|| panic!("no event `{name}`"))
    }

    /// Runs the world's creature tick `ticks` times.
    fn run(server: &mut Server, ticks: u64) {
        for _ in 0..ticks {
            server.tick += 1;
            server.tick_creatures();
        }
    }

    fn start(server: &mut Server, name: &str, from: (i32, i32)) -> u64 {
        let event = event(server, name);
        let spot = server.standing_spot(from.0, from.1, 200).expect("floor");
        server.begin_event_at(Some(1), event, spot, 7, true).expect("the party fits")
    }

    fn leader_at(server: &Server, id: u64) -> Option<Vec3> {
        let instance = server.wandering.active.get(&id)?;
        server.creatures.live.get(&instance.members.first()?.creature).map(|c| c.body.position)
    }

    #[test]
    fn a_march_passes_the_player_and_is_done() {
        let mut server = fixture();
        let id = start(&mut server, "pedlar_and_hound", (68, 128));
        let player = Vec3::new(128.5, 200.0, 128.5);
        let mut nearest = f32::MAX;
        let mut last = None;
        // It is taken away once it has walked its length and nobody is near.
        for _ in 0..80 {
            run(&mut server, 100);
            let Some(at) = leader_at(&server, id) else { break };
            nearest = nearest.min(flat(at, player));
            last = Some(at);
        }
        assert!(!server.wandering.active.contains_key(&id), "the march ran its length and was taken away");
        let (low, high) = (10.0, 22.0);
        assert!((low - 6.0..=high + 6.0).contains(&nearest), "it passed {nearest:.0} blocks off, the event says {low} to {high}");
        let at = last.expect("it was seen walking");
        assert!(at.x > 150.0, "it went on to the far side: {at}");
    }

    #[test]
    fn the_party_keeps_together_and_the_dog_its_person() {
        let mut server = fixture();
        let id = start(&mut server, "stroll_with_dog", (68, 128));
        run(&mut server, 600);
        let instance = &server.wandering.active[&id];
        assert_eq!(instance.members.len(), 2, "a person and their dog");
        let leader = server.creatures.live[&instance.members[0].creature].body.position;
        let dog = server.creatures.live[&instance.members[1].creature].body.position;
        assert!(leader.x > 75.0, "they have set off: {leader}");
        assert!(flat(leader, dog) < 8.0, "the dog is {:.1} blocks from its person", flat(leader, dog));
    }

    #[test]
    fn a_patrol_walks_its_ring_and_marches_off() {
        let mut server = fixture();
        // The player is well out of the raiders' sight, so they keep to their round.
        server.players.get_mut(&1).unwrap().position = Vec3::new(100.5, 200.0, 100.5);
        server.players.get_mut(&1).unwrap().health_base = 20;
        let id = start(&mut server, "bandit_patrol", (170, 170));
        let anchor = Vec3::new(170.5, 200.0, 170.5);
        let mut sides = FxHashSet::default();
        for _ in 0..120 {
            run(&mut server, 100);
            let Some(at) = leader_at(&server, id) else { break };
            if flat(at, anchor) > 14.0 {
                sides.insert(((at.x - anchor.x).signum() as i32, (at.z - anchor.z).signum() as i32));
            }
            if matches!(server.wandering.active[&id].plan, Plan::March { .. }) {
                break;
            }
        }
        assert!(sides.len() >= 3, "a ring of four stops visits at least three sides of its anchor: {sides:?}");
        assert!(matches!(server.wandering.active[&id].plan, Plan::March { .. }), "after its laps the patrol marches off");
    }

    #[test]
    fn a_shy_stranger_keeps_off_and_stands_still_when_watched() {
        let mut server = fixture();
        let id = start(&mut server, "hooded_stranger", (128, 80));
        // The player looks away (north is -z; the stranger is at -z, so look +z).
        {
            let p = server.players.get_mut(&1).unwrap();
            p.yaw = std::f32::consts::PI;
        }
        run(&mut server, 400);
        let player = Vec3::new(128.5, 200.0, 128.5);
        let at = leader_at(&server, id).unwrap();
        let d = flat(at, player);
        assert!((10.0..=24.0).contains(&d), "the stranger comes to about 12 to 20 blocks and no nearer: {d:.1}");
        // Looked at, it stands: face it and compare where it is over a few seconds.
        server.players.get_mut(&1).unwrap().yaw = 0.0;
        server.players.get_mut(&1).unwrap().pitch = 0.0;
        let before = leader_at(&server, id).unwrap();
        server.players.get_mut(&1).unwrap().position = Vec3::new(before.x + 0.0, 200.0, before.z + 30.0);
        run(&mut server, 60);
        let later = leader_at(&server, id).unwrap();
        assert!(flat(before, later) < 3.0, "watched, it holds still: moved {:.1}", flat(before, later));
    }

    fn give_held(server: &mut Server, item: &str, count: u8) {
        let item = server.content.blocks.expect(item);
        let p = server.players.get_mut(&1).unwrap();
        p.selected = 0;
        p.inventory.clear(|_| true);
        assert_eq!(p.inventory.add(item, count), 0);
        assert_eq!(p.held(), Some(item), "what was given is in the hand");
    }

    fn count_of(server: &Server, item: &str) -> u32 {
        let item = server.content.blocks.expect(item);
        (0..voxmar_engine::inventory::SLOTS).filter_map(|s| server.players[&1].inventory.get(s)).filter(|s| s.item == item).map(|s| s.count as u32).sum()
    }

    #[test]
    fn a_pedlar_swaps_what_is_held_and_nothing_else() {
        let mut server = fixture();
        let id = start(&mut server, "pedlar_and_hound", (68, 128));
        let pedlar = server.wandering.active[&id].members[0].creature;
        let hound = server.wandering.active[&id].members[1].creature;
        give_held(&mut server, "base:hide", 3);
        assert!(!server.meet(1, hound), "the dog has nothing to say: the click goes on to inspect it");
        assert!(server.meet(1, pedlar));
        assert_eq!((count_of(&server, "base:hide"), count_of(&server, "base:twine")), (1, 6), "two hides for six twine");
        give_held(&mut server, "base:hide", 1);
        assert!(server.meet(1, pedlar));
        assert_eq!((count_of(&server, "base:hide"), count_of(&server, "base:twine")), (1, 0), "one hide is not enough: only the greeting");
    }

    #[test]
    fn a_lesson_is_given_once_to_each_player_and_trains_magic() {
        let mut server = fixture();
        let id = start(&mut server, "mage_tutors", (68, 128));
        let mage = server.wandering.active[&id].members[0].creature;
        let magic = server.content.skills.id("core:magic").unwrap();
        give_held(&mut server, "base:aether_mote", 1);
        assert!(server.meet(1, mage));
        let after = server.players[&1].skills.xp(magic);
        assert_eq!(after, 150, "15 experience in tenths");
        assert!(!server.players[&1].timed_effects.is_empty(), "focused for a while");
        assert_eq!(count_of(&server, "base:aether_mote"), 0, "the mote was spent");
        give_held(&mut server, "base:aether_mote", 1);
        assert!(server.meet(1, mage));
        assert_eq!(server.players[&1].skills.xp(magic), after, "the same lesson is not taught twice");
        assert_eq!(count_of(&server, "base:aether_mote"), 1, "and the mote stays in hand");
    }

    #[test]
    fn a_party_without_players_near_is_taken_away() {
        let mut server = fixture();
        let id = start(&mut server, "bard_band", (68, 128));
        assert!(!server.creatures.live.is_empty());
        server.players.get_mut(&1).unwrap().position = Vec3::new(5000.0, 200.0, 5000.0);
        run(&mut server, 20);
        assert!(!server.wandering.active.contains_key(&id), "no one is near, so it ends");
        assert!(server.creatures.live.is_empty(), "and its people go with it");
        assert!(server.wandering.by_creature.is_empty());
    }

    #[test]
    fn one_party_at_a_time_near_a_player_and_a_gap_after() {
        let mut server = fixture();
        server.day = TICKS_PER_DAY as f64 * 3.4;
        start(&mut server, "bard_band", (68, 128));
        let began = server.tick;
        let before = server.wandering.active.len();
        for salt in 0..200 {
            server.tick += 1;
            server.try_wandering(1, salt);
        }
        assert_eq!(server.wandering.active.len(), before, "most_near is one");
        let gap = server.content.wandering.rules().gap;
        assert!(server.wandering.quiet_until[&1] >= began + (gap * TICKS_PER_SECOND as f32) as u64, "a gap after one began");
    }

    #[test]
    fn people_and_animals_of_a_party_use_their_own_budgets() {
        let mut server = fixture();
        // The settlements' people cap is untouched by travellers, and travelling animals count as animals.
        start(&mut server, "stroll_with_dog", (68, 128));
        let villagers = server.creatures.live.values().filter(|c| server.content.creatures.get(c.species).def.kind == Kind::Villager).count();
        assert_eq!(villagers, 1);
        let animals = server.creatures.live.values().filter(|c| server.content.creatures.get(c.species).def.kind == Kind::Animal).count();
        assert_eq!(animals, 1, "the dog is an animal near the player like any other");
        let hound = server.content.creatures.id("base:hound").unwrap();
        // Fill the animal budget near the player: no event with animals starts after that.
        let cap = server.content.creatures.limits().animals_near;
        for i in 0..cap {
            server.spawn_creature(hound, Vec3::new(128.5, 200.0, 128.5 + i as f32), i as u64);
        }
        assert!(!server.nearby_room(Vec3::new(128.5, 200.0, 128.5), false, cap));
        assert!(!server.wandering_room(hound, Vec3::new(128.5, 200.0, 128.5)), "no room for another dog");
    }
}
