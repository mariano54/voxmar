//! The people of settlements (`people` in settlements.ron, planned by worldgen's
//! `settlements::people`), alive near players.
//!
//! A settlement comes to life when a player comes within `active` blocks of it (every
//! number here is `people.live` in settlements.ron): at most `per_settlement` residents
//! within `nearby` of a player appear where their day has them now (asleep at home at
//! night, at work by day), as a creature of
//! their species (`creatures_villagers.ron`, kind `Villager`) with the looks their seed
//! gives, the same each visit. When every player is `release` blocks away they go again,
//! and come back where their day has them by then.
//!
//! Each person's routine sends them from place to place through the day (`place_at`);
//! the server finds their way there through the settlement's lanes, doors and yards
//! (`way`, an A* over the cells a body can stand in) and hands the brain the next turn of
//! it as an errand, which the `Routine` behaviour walks. At a place where people stay a
//! while (the square, a field, the yard) they move to another spot of it now and then.
//!
//! Someone killed is gone until a newcomer takes their place `newcomer` days later, with
//! looks of their own, a settlement taking `newcomers_per_day` at most. Who died is kept
//! apart from the settlements near players and saved with the world (`villagers.ron`), so
//! leaving, walking far or a restart brings no one back sooner. A villager struck by a
//! player calls the guards near: they fight the one who did it (`Defend`), and one that
//! comes no nearer them for `give_up` seconds (they stand where it cannot go) breaks off,
//! goes home a while, teaching that player nothing during the cooldown. It heals only
//! after a further `give_up` seconds without being struck.
//!
//! They react to what is about them (`people.react`): a player who comes near is greeted
//! (they stop, turn to them and hum, then go on with their day), a monster or a hunter
//! near sends them home crying out a while (guards stand), one struck cries out, rain
//! sends them in from the square, the yards and the fields, neighbours who meet stop
//! a while to talk, and they sigh content as they reach their bed. What they say is a
//! bark (`Server::bark`).

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::brain::{Errand, Pose};
use voxmar_engine::creature::{Bark, Behaviour, CreatureId, Kind, mix, unit_from};
use voxmar_engine::genes;
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3, Vec3Swizzles};
use voxmar_protocol::{CreatureGone, TICKS_PER_DAY, TICKS_PER_SECOND};
use voxmar_worldgen::people::{LiveDef, Place, ReactDef, is, place_at};
use voxmar_worldgen::{BuildingKind, SettlementInfo, SettlementLevers};

use crate::Server;
use crate::save::SavedDead;

/// `seconds` in server ticks, at least one.
fn ticks(seconds: f32) -> u64 {
    ((seconds * TICKS_PER_SECOND as f32).round() as u64).max(1)
}

/// Every settlement near players, who of their people is alive as which creature, and who
/// of anyone's died.
#[derive(Default)]
pub struct Villagers {
    towns: BTreeMap<u64, Town>,
    by_creature: FxHashMap<u32, (u64, usize)>,
    /// Cells searched for ways this tick, so a whole village waking at once spreads its
    /// ways over ticks.
    searched: u32,
    /// Ways not found, by settlement, from where and to where, and the tick they may be
    /// searched for again: the same search from the same cell finds the same nothing, and
    /// one from anywhere else (another person, or this one moved on) still looks.
    failed: FxHashMap<(u64, IVec3, IVec3), u64>,
    /// Who died, by settlement and resident: kept whether or not the settlement is near
    /// anyone, and saved with the world.
    dead: BTreeMap<(u64, usize), Dead>,
    /// A short-lived guard cooldown outlives a forgotten town, so leaving/rejoining
    /// cannot reset a safe-spot fight. Entries expire on the server's clock.
    recovering: BTreeMap<(u64, usize), Recovery>,
    /// Who barked what to which player, until the tick they may again
    /// (`Barking::every`, `Server::bark`).
    pub(crate) barked: FxHashMap<(u32, u32, Bark), u64>,
}

/// One of a settlement's people who died.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Dead {
    /// Newcomers who have taken their place, each with looks of their own.
    generation: u64,
    /// The day (in days since the world began) the next newcomer comes.
    away_until: f64,
}

#[derive(Clone)]
struct Recovery {
    calm_until: u64,
    calm_players: BTreeMap<String, u64>,
    heal_after: Option<u64>,
    health: Option<u16>,
}

impl Villagers {
    fn remember_town(&mut self, info: SettlementInfo, margin: i32) {
        let mut town = Town::new(info, margin);
        for (i, person) in town.people.iter_mut().enumerate() {
            if let Some(r) = self.recovering.get(&(town.info.id, i)) {
                person.calm_until = r.calm_until;
                person.calm_players = r.calm_players.clone();
                person.heal_after = r.heal_after;
                person.health = r.health;
            }
        }
        self.towns.insert(town.info.id, town);
    }

    /// From who died, as saved.
    pub(crate) fn restore(saved: Vec<SavedDead>) -> Self {
        let dead = saved.into_iter().map(|d| ((d.town, d.person), Dead { generation: d.generation, away_until: d.away_until })).collect();
        Self { dead, ..Self::default() }
    }

    /// Who died, to save. One entry a person ever killed (their newcomers' looks hang on
    /// how many came), so it grows with the people of the places players fight in, not
    /// with every death.
    pub(crate) fn saved(&self) -> Vec<SavedDead> {
        self.dead.iter().map(|(&(town, person), d)| SavedDead { town, person, generation: d.generation, away_until: d.away_until }).collect()
    }

    fn generation(&self, town: u64, i: usize) -> u64 {
        self.dead.get(&(town, i)).map_or(0, |d| d.generation)
    }

    /// Whether a settlement's centre is within `reach` blocks of `at` (settlements that are
    /// loaded: those near a player).
    pub(crate) fn town_within(&self, at: Vec3, reach: f32) -> bool {
        self.towns.values().any(|t| Vec2::new(t.info.centre.x as f32 - at.x, t.info.centre.z as f32 - at.z).length() < reach)
    }

    /// Whether creature `id` is someone's, kept here rather than by the land.
    pub(crate) fn manages(&self, id: u32) -> bool {
        self.by_creature.contains_key(&id)
    }

    /// Creature `id` is kept here, as if one of its own (a test's).
    #[cfg(test)]
    pub(crate) fn keep_for_test(&mut self, id: u32) {
        self.by_creature.insert(id, (0, 0));
    }

    /// Where creature `id`'s routine sends it now, if it is someone's; a guard after
    /// someone goes straight at them within `handoff` blocks.
    pub(crate) fn errand(&self, id: u32, handoff: f32) -> Option<Errand> {
        let &(town, i) = self.by_creature.get(&id)?;
        let trip = self.towns.get(&town)?.people[i].trip.as_ref()?;
        let to = *trip.path.get(trip.step)?;
        let last = trip.step + 1 >= trip.path.len();
        Some(Errand { to: to.as_vec3() + Vec3::new(0.5, 0.0, 0.5), last, pose: trip.pose, face: trip.face, chase: trip.pursuit.map(|_| handoff), pace: 1.0, steady: false })
    }
}

struct Town {
    info: SettlementInfo,
    levers: SettlementLevers,
    /// The blocks its buildings stand on, grown by a margin: where ways are searched.
    min: IVec2,
    max: IVec2,
    people: Vec<Person>,
    alive: bool,
    /// Rain heavy enough to send people in falls on its square (looked at every
    /// `scan_every`).
    raining: bool,
}

#[derive(Default)]
struct Person {
    species: Option<CreatureId>,
    creature: Option<u32>,
    trip: Option<Trip>,
    /// A guard's quarry: the player, the nearest it has come to them, and when.
    quarry: Option<(u32, f32, u64)>,
    /// A guard that broke off a chase stays home, heeding no call, until this tick.
    calm_until: u64,
    /// Each quarry keeps its own expiry by `player_key`; alternating players cannot
    /// replace another's cooldown.
    calm_players: BTreeMap<String, u64>,
    /// Recovery is postponed by every blow while the guard walks home.
    heal_after: Option<u64>,
    /// Injuries survive releasing the body until its quiet recovery time.
    health: Option<u16>,
    /// Spots looked for in a row and none found: each miss draws its next pick otherwise.
    misses: u64,
    /// Greeting a player, until a tick.
    greeting: Option<(u32, u64)>,
    /// Each player greeted, by `player_key`, is greeted again no sooner than this tick.
    greet_after: BTreeMap<String, u64>,
    /// Frightened home by a monster or a hunter until this tick.
    afraid_until: u64,
    /// Struck, they cry out again no sooner than this tick.
    alarm_after: u64,
    /// The tick they last sighed content reaching their bed: once a night, however a
    /// night's sleep runs past midnight (its trip planned again on the new day).
    sighed: Option<u64>,
    /// Talking with a neighbour (their creature) until a tick.
    chat: Option<(u32, u64)>,
    /// They stop to talk with a neighbour again no sooner than this tick.
    chat_after: u64,
}

/// What came of looking for a way.
enum Way {
    Found(Vec<IVec3>),
    /// None within the search, or none found there lately.
    Unreached,
    /// This tick's search is spent: look again next time.
    Later,
}

/// Where someone is going and the way there.
struct Trip {
    place: Place,
    slot: usize,
    day: u64,
    /// The cells the way turns at, the last where they are going.
    path: Vec<IVec3>,
    step: usize,
    pose: Pose,
    face: Option<Vec3>,
    /// Tick to move to another spot of the place, for places one stays at a while.
    next_spot: Option<u64>,
    /// Patrols: which gate is next.
    leg: usize,
    /// The nearest the body has come to the next turn, and when.
    best: (f32, u64),
    /// Tick after which to find the way again when none was found.
    retry: u64,
    /// The player an angry guard is going after, by the way found to them.
    pursuit: Option<u32>,
}

impl Town {
    fn new(info: SettlementInfo, margin: i32) -> Self {
        let (mut min, mut max) = (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN));
        for b in &info.buildings {
            min = min.min(b.min);
            max = max.max(b.max);
        }
        for g in &info.gates {
            min = min.min(*g);
            max = max.max(*g);
        }
        let centre = IVec2::new(info.centre.x, info.centre.z);
        let (min, max) = (min.min(centre) - margin, max.max(centre) + margin);
        let people = info.people.iter().map(|_| Person::default()).collect();
        Self { levers: info.levers, info, min, max, people, alive: false, raining: false }
    }

    /// Blocks from `at` to the nearest of its ground.
    fn distance(&self, at: Vec3) -> f32 {
        let p = Vec2::new(at.x, at.z);
        let clamped = p.clamp(self.min.as_vec2(), self.max.as_vec2());
        p.distance(clamped)
    }

    /// Whether it has anywhere for `place`; `shrine` names the building Shrine is.
    fn has(&self, place: Place, shrine: &str) -> bool {
        let b = &self.info.buildings;
        match place {
            Place::Inn => b.iter().any(|b| b.kind == BuildingKind::Inn),
            Place::Shrine => b.iter().any(|b| is(b, shrine)),
            Place::Well => b.iter().any(|b| b.kind == BuildingKind::Well),
            Place::Gate | Place::Patrol => !self.info.gates.is_empty(),
            Place::Neighbour => b.iter().filter(|b| b.kind == BuildingKind::House && b.door.is_some()).count() > 1,
            Place::Wander => self.info.lanes.iter().any(|l| !l.is_empty()),
            _ => true,
        }
    }

    /// Outdoor errands use the ground around these features, never their walls or pool.
    fn outdoor_column(&self, at: IVec3) -> bool {
        !self.info.buildings.iter().any(|b| {
            matches!(b.kind, BuildingKind::Well | BuildingKind::Centrepiece | BuildingKind::Shrine)
                && at.x >= b.min.x && at.x <= b.max.x && at.z >= b.min.y && at.z <= b.max.y
        })
    }
}

/// Days since the world began, from the server's clock.
fn days(day: f64) -> f64 {
    day / TICKS_PER_DAY as f64
}

/// What person `seed` draws the spot for a trip by: the day, the place's slot, the trip's
/// leg there and the tries in a row that found none, never the tick it sets out on.
fn spot_pick(seed: u64, day: u64, slot: usize, leg: usize, misses: u64) -> u64 {
    mix(mix(mix(seed, day), (slot as u64) << 32 | leg as u64), misses)
}

impl Server {
    /// How settlements' people are kept alive (`people.live`).
    fn live(&self) -> LiveDef {
        self.overworld.settlement_people().live
    }

    /// Brings settlements near players to life and lets them go, notes who died, and
    /// moves everyone along their day.
    pub(crate) fn tick_villagers(&mut self) {
        let _span = tracing::info_span!("villagers").entered();
        self.villagers.searched = 0;
        let live = self.live();
        let tick = self.tick;
        if tick.is_multiple_of(ticks(live.scan_every)) {
            self.villagers.failed.retain(|_, until| *until > tick);
            self.scan_towns(&live);
            self.look_at_the_sky();
        }
        let ids: Vec<(u32, u64, usize)> = self.villagers.by_creature.iter().map(|(&id, &(t, i))| (id, t, i)).collect();
        for (id, town, i) in ids {
            if (tick + id as u64).is_multiple_of(ticks(live.errand_every)) {
                self.follow_day(id, town, i, &live);
            }
        }
        if tick.is_multiple_of(ticks(live.call_every)) {
            self.call_guards(&live);
        }
    }

    /// Whether rain heavy enough to send people in falls on each living settlement's square.
    fn look_at_the_sky(&mut self) {
        let rain = self.overworld.settlement_people().react.rain;
        let (tick, day) = (self.tick, self.day);
        for town in self.villagers.towns.values_mut().filter(|t| t.alive) {
            let at = Vec2::new(town.info.centre.x as f32 + 0.5, town.info.centre.z as f32 + 0.5);
            town.raining = self.weather.local(&mut self.temperature, at, tick, day).weather.precipitation >= rain;
        }
    }

    /// Notes the death of creature `id`, if it was someone's, as it dies.
    pub(crate) fn villager_died(&mut self, id: u32) {
        if let Some(&(town, i)) = self.villagers.by_creature.get(&id) {
            self.note_death(town, i, days(self.day));
        }
    }

    /// Notes who of every settlement near players died, for the save.
    pub(crate) fn note_all_deaths(&mut self) {
        let today = days(self.day);
        let ids: Vec<u64> = self.villagers.towns.keys().copied().collect();
        for id in ids {
            self.note_deaths(id, today);
        }
    }

    fn scan_towns(&mut self, live: &LiveDef) {
        let players: Vec<Vec3> = self.players.values().filter(|p| p.joined).map(|p| p.position).collect();
        for town in self.villagers.towns.values_mut() {
            for person in &mut town.people {
                person.calm_players.retain(|_, until| *until > self.tick);
            }
        }
        for recovery in self.villagers.recovering.values_mut() {
            recovery.calm_players.retain(|_, until| *until > self.tick);
        }
        self.villagers.recovering.retain(|_, r| r.calm_until > self.tick || r.heal_after.is_some_and(|due| due > self.tick));
        // Settlements coming into reach.
        for at in &players {
            for info in self.overworld.settlements_near(IVec2::new(at.x as i32, at.z as i32), live.active as i32 + live.scan_pad) {
                if !self.villagers.towns.contains_key(&info.id) {
                    self.villagers.remember_town(info, live.margin);
                }
            }
        }
        let today = days(self.day);
        let ids: Vec<u64> = self.villagers.towns.keys().copied().collect();
        for id in ids {
            // Who died since the last look, before anyone is let go: a death is kept
            // whether or not the players are still about.
            self.note_deaths(id, today);
            let town = &self.villagers.towns[&id];
            let nearest = players.iter().map(|&p| town.distance(p)).fold(f32::MAX, f32::min);
            // Let go of altogether (who died there is kept apart).
            if nearest > live.forget {
                self.release_town(id);
                self.villagers.towns.remove(&id);
                continue;
            }
            let alive = if town.alive { nearest < live.release } else { nearest < live.active };
            if !alive {
                if town.alive {
                    self.release_town(id);
                }
                continue;
            }
            self.villagers.towns.get_mut(&id).expect("listed").alive = true;
            let nearby = self.nearby(id, &players, live);
            for i in 0..self.villagers.towns[&id].people.len() {
                if nearby.contains(&i) {
                    self.bring_out(id, i);
                } else {
                    self.release_person(id, i);
                }
            }
        }
    }

    /// Who of settlement `id`'s people live, near `players`: only nearby residents, closest
    /// first, with an independent town cap.
    fn nearby(&self, id: u64, players: &[Vec3], live: &LiveDef) -> Vec<usize> {
        let t = &self.villagers.towns[&id];
        let mut nearby: Vec<(usize, f32)> = (0..t.people.len()).filter_map(|i| {
            let at = match t.people[i].creature.and_then(|c| self.creatures.live.get(&c)) {
                Some(c) => c.body.position,
                None => self.spot_for(id, i, mix(t.info.people[i].seed, self.villagers.generation(id, i)))?.0.as_vec3(),
            };
            let distance = players.iter().map(|p| p.with_y(0.0).distance(at.with_y(0.0))).fold(f32::MAX, f32::min);
            (distance <= live.nearby).then_some((i, distance))
        }).collect();
        nearby.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        nearby.truncate(live.per_settlement);
        nearby.into_iter().map(|(i, _)| i).collect()
    }

    /// Of the people of every living settlement near players, how many are out and how
    /// many are due out (those who live and are not dead, where the land under them is
    /// held), and which of those out a player at `at` is told of (`/creatures people`, for a
    /// test to wait on rather than guess).
    pub(crate) fn people_out(&self, at: Vec3) -> (usize, usize, Vec<u32>) {
        let live = self.live();
        let players: Vec<Vec3> = self.players.values().filter(|p| p.joined).map(|p| p.position).collect();
        let today = days(self.day);
        let (mut out, mut due, mut seen) = (0, 0, Vec::new());
        for (&id, town) in self.villagers.towns.iter().filter(|(_, t)| t.alive) {
            for i in self.nearby(id, &players, &live) {
                if self.villagers.dead.get(&(id, i)).is_some_and(|d| d.away_until > today) {
                    continue;
                }
                if let Some(creature) = town.people[i].creature {
                    (out, due) = (out + 1, due + 1);
                    if self.creatures.live.get(&creature).is_some_and(|c| c.body.position.distance(at) < crate::creatures::SEEN_WITHIN) {
                        seen.push(creature);
                    }
                } else if self.spot_for(id, i, mix(town.info.people[i].seed, self.villagers.generation(id, i))).is_some_and(|(spot, _)| self.world.contains(ColumnPos::of_block(spot))) {
                    due += 1;
                }
            }
        }
        seen.sort_unstable();
        (out, due, seen)
    }

    /// A disappearance without `creature_dies` releases a resident, never starts the
    /// newcomer clock (for example a body that fell out of the world).
    fn note_deaths(&mut self, id: u64, _today: f64) {
        let Some(town) = self.villagers.towns.get(&id) else { return };
        let gone: Vec<usize> = town.people.iter().enumerate().filter(|(_, p)| p.creature.is_some_and(|c| !self.creatures.live.contains_key(&c))).map(|(i, _)| i).collect();
        for i in gone {
            self.release_person(id, i);
        }
    }

    /// Person `i` of settlement `town` died: gone until a newcomer takes their place
    /// `newcomer` days on, and no sooner than a day's share (`newcomers_per_day`) after
    /// the settlement's last newcomer due, so several dead come back one by one.
    fn note_death(&mut self, town: u64, i: usize, today: f64) {
        let people = self.overworld.settlement_people();
        let (newcomer, apart) = (people.newcomer as f64, 1.0 / people.newcomers_per_day as f64);
        let Some(t) = self.villagers.towns.get_mut(&town) else { return };
        let person = &mut t.people[i];
        let Some(c) = person.creature.take() else { return };
        self.villagers.by_creature.remove(&c);
        self.villagers.recovering.remove(&(town, i));
        person.trip = None;
        person.quarry = None;
        person.health = None;
        person.heal_after = None;
        person.calm_until = 0;
        person.calm_players.clear();
        let last = self.villagers.dead.range((town, 0)..=(town, usize::MAX)).map(|(_, d)| d.away_until).fold(f64::MIN, f64::max);
        let dead = self.villagers.dead.entry((town, i)).or_insert(Dead { generation: 0, away_until: 0.0 });
        dead.generation += 1;
        dead.away_until = (today + newcomer).max(last + apart);
        tracing::debug!(town = %t.info.name.0, person = i, back = dead.away_until, "a villager died");
    }

    fn release_person(&mut self, town: u64, i: usize) {
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        person.trip = None;
        person.quarry = None;
        if let Some(c) = person.creature.take() {
            person.health = self.creatures.live.get(&c).map(|c| c.health);
            if person.calm_until > self.tick || person.heal_after.is_some_and(|due| due > self.tick) {
                self.villagers.recovering.insert((town, i), Recovery {
                    calm_until: person.calm_until, calm_players: person.calm_players.clone(),
                    heal_after: person.heal_after, health: person.health,
                });
            } else {
                self.villagers.recovering.remove(&(town, i));
            }
            self.villagers.by_creature.remove(&c);
            if self.creatures.live.remove(&c).is_some() {
                self.creatures.gone.push(CreatureGone { id: c, died: false });
            }
        }
    }

    /// Everyone of settlement `id` goes.
    fn release_town(&mut self, id: u64) {
        let Some(town) = self.villagers.towns.get_mut(&id) else { return };
        town.alive = false;
        for i in 0..town.people.len() {
            self.release_person(id, i);
        }
    }

    /// Puts person `i` of settlement `town` where their day has them, if they are not about
    /// and not gone.
    fn bring_out(&mut self, town: u64, i: usize) {
        let today = days(self.day);
        let t = &self.villagers.towns[&town];
        let person = &t.people[i];
        if person.creature.is_some() || self.villagers.dead.get(&(town, i)).is_some_and(|d| d.away_until > today) {
            return;
        }
        let resident = &t.info.people[i];
        let species = match person.species {
            Some(s) => s,
            None => {
                let Some(s) = self.content.creatures.id(&resident.species) else {
                    tracing::warn!(species = %resident.species, "settlement person of no species");
                    return;
                };
                s
            }
        };
        let seed = mix(resident.seed, self.villagers.generation(town, i));
        let home = t.info.buildings[resident.home].door.unwrap_or(t.info.centre).as_vec3();
        let Some(spot) = self.spot_for(town, i, seed) else { return };
        if !self.world.contains(ColumnPos::of_block(spot.0)) {
            return;
        }
        let Some(id) = self.spawn_seeded(species, spot.0.as_vec3() + Vec3::new(0.5, 0.0, 0.5), seed) else { return };
        self.start_rolls(id);
        // Looks from the people of the land about, the same each visit.
        let genome = self.wild_genome(species, home, seed);
        let def = &self.content.creatures.get(species).def;
        let traits = genes::express(&def.genes, &genome);
        let scale = self.overworld.settlement_people().live.scale;
        let creature = self.creatures.live.get_mut(&id).expect("just put");
        // Releasing an injured guard must not instantly heal it on the next visit.
        let p = &self.villagers.towns[&town].people[i];
        if p.heal_after.is_none_or(|due| self.tick < due)
            && let Some(health) = p.health
        {
            creature.health = health;
        }
        creature.home = home;
        creature.scale = traits.scale * (scale.0 + (scale.1 - scale.0) * unit_from(mix(seed, 0x5ca1e)));
        creature.adult_scale = creature.scale;
        creature.body.size = (def.body.size.0 * creature.scale, def.body.size.1 * creature.scale);
        creature.genome = genome;
        creature.traits = traits;
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        person.species = Some(species);
        person.creature = Some(id);
        person.trip = Some(spot.1);
        self.villagers.by_creature.insert(id, (town, i));
    }

    /// Where person `i` is meant to be now, and the trip that leaves them there.
    fn spot_for(&self, town: u64, i: usize, seed: u64) -> Option<(IVec3, Trip)> {
        let (place, slot, day) = self.place_now(town, i, seed);
        let mut trip = Trip { place, slot, day, path: Vec::new(), step: 0, pose: Pose::Idle, face: None, next_spot: None, leg: 0, best: (f32::MAX, self.tick), retry: 0, pursuit: None };
        let target = self.target(town, i, seed, &mut trip, &self.live())?;
        trip.path = vec![target];
        Some((target, trip))
    }

    /// Where person `i`'s routine sends them at this moment: the place, the slot of the
    /// day and the day.
    fn place_now(&self, town: u64, i: usize, seed: u64) -> (Place, usize, u64) {
        let t = &self.villagers.towns[&town];
        let resident = &t.info.people[i];
        let now = days(self.day);
        let day = now.floor() as u64;
        let people = self.overworld.settlement_people();
        // A guard who broke off a chase stays home a while.
        if t.people[i].calm_until > self.tick {
            return (Place::Home, usize::MAX, day);
        }
        let (place, slot) = place_at(people, resident.routine, seed, &t.levers, day, now.fract() as f32, |p| t.has(p, &people.shrine));
        // So does anyone frightened, a while; one abed or at home already stays there.
        if t.people[i].afraid_until > self.tick && !matches!(place, Place::Sleep | Place::Home) {
            return (Place::Home, usize::MAX, day);
        }
        // Rain sends everyone but the guards in from the open.
        let react = &people.react;
        let open = react.shelter.contains(&place)
            || (place == Place::Work && resident.work.map_or(react.shelter.contains(&Place::Yard), |w| react.outdoor.iter().any(|o| is(&t.info.buildings[w], o))));
        if t.raining && !resident.guard && open {
            return (Place::Home, slot, day);
        }
        (place, slot, day)
    }

    /// Moves person `i` along their day: a new place when the day says, another spot of a
    /// place they stay at, the next turn of the way, and a new way when stuck.
    fn follow_day(&mut self, id: u32, town: u64, i: usize, live: &LiveDef) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let at = creature.body.position;
        let seed = creature.seed;
        let tick = self.tick;
        if let Some(due) = self.villagers.towns[&town].people[i].heal_after
            && tick >= due
        {
            if let Some(c) = self.creatures.live.get_mut(&id) {
                c.health = self.content.creatures.get(c.species).def.health;
                c.changed = true;
            }
            self.villagers.towns.get_mut(&town).expect("listed").people[i].heal_after = None;
        }
        // An angry guard goes after the one it is angry with, by the lanes.
        let creature = &self.creatures.live[&id];
        let chasing = creature.mind.attacker().filter(|_| creature.mind.anger > 0.0 && self.villagers.towns[&town].info.people[i].guard);
        if let Some(player) = chasing {
            self.pursue(id, town, i, at, player, live);
            return;
        }
        self.villagers.towns.get_mut(&town).expect("listed").people[i].quarry = None;
        let react = self.overworld.settlement_people().react.clone();
        if self.react(id, town, i, at, &react) {
            return;
        }
        let (place, slot, day) = self.place_now(town, i, seed);
        let fresh = {
            let trip = self.villagers.towns[&town].people[i].trip.as_ref();
            match trip {
                None => true,
                Some(t) if t.pursuit.is_some() => true,
                Some(t) => t.place != place || t.slot != slot || t.day != day || t.next_spot.is_some_and(|n| tick >= n) || (t.path.len() <= 1 && t.retry != 0 && tick >= t.retry),
            }
        };
        if fresh {
            let leg = self.villagers.towns[&town].people[i].trip.as_ref().filter(|t| t.place == place).map_or(0, |t| t.leg + 1);
            let mut trip = Trip { place, slot, day, path: Vec::new(), step: 0, pose: Pose::Idle, face: None, next_spot: None, leg, best: (f32::MAX, tick), retry: 0, pursuit: None };
            // Which spot, by the day, the place's slot and the trip's leg there, never the tick
            // it sets out on; a miss draws the next try otherwise.
            let misses = self.villagers.towns[&town].people[i].misses;
            let pick = spot_pick(seed, day, slot, leg, misses);
            let found = self.target(town, i, pick, &mut trip, live);
            self.villagers.towns.get_mut(&town).expect("listed").people[i].misses = if found.is_some() { 0 } else { misses + 1 };
            let Some(target) = found else { return };
            let start = at.floor().as_ivec3();
            match self.find_way(start, target, town, live) {
                Way::Found(path) => trip.path = path,
                Way::Unreached => {
                    // No way found: stay put, and look again in a while.
                    trip.path = vec![start];
                    trip.pose = Pose::Idle;
                    trip.retry = tick + ticks(live.no_way_retry);
                }
                Way::Later => return,
            }
            self.villagers.towns.get_mut(&town).expect("listed").people[i].trip = Some(trip);
            return;
        }
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        let sighed = person.sighed;
        let trip = person.trip.as_mut().expect("checked");
        let Some(&next) = trip.path.get(trip.step) else { return };
        let to = next.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        let d = to.with_y(0.0).distance(at.with_y(0.0));
        let last = trip.step + 1 >= trip.path.len();
        if d < 0.9 && !last {
            trip.step += 1;
            trip.best = (f32::MAX, tick);
        } else if d < 1.0 && last && trip.place == Place::Sleep && sighed.is_none_or(|at| tick >= at + TICKS_PER_DAY as u64 / 2) {
            // In bed at last: a content sigh, once a night.
            person.sighed = Some(tick);
            self.bark(id, Bark::Content);
        } else if d < 1.0 && last && trip.place == Place::Patrol && trip.next_spot.is_none() {
            // A patrol stops a moment at each stop, then walks on.
            trip.next_spot = Some(tick + ticks(live.patrol_pause));
        } else if d + 0.2 < trip.best.0 {
            trip.best = (d, tick);
        } else if d > 1.0 && tick - trip.best.1 > ticks(live.stuck) {
            // Stuck: find the way again from here.
            trip.next_spot = Some(tick);
        }
    }

    /// Person `i` (creature `id`, at `at`) reacts to what is about them (`people.react`),
    /// by the one sight rule every creature looks by (`Server::in_view`: where they face,
    /// nothing hiding it): a monster or a hunter seen near frightens anyone but a guard home
    /// crying out, and a player near they have noticed and see is greeted: they stop where
    /// they stand, turn to them and hum. Whether they stand greeting, their day waiting.
    fn react(&mut self, id: u32, town: u64, i: usize, at: Vec3, react: &ReactDef) -> bool {
        let tick = self.tick;
        let guard = self.villagers.towns[&town].info.people[i].guard;
        let Some(me) = self.creatures.live.get(&id) else { return false };
        let (body, field) = (me.body, self.content.creatures.get(me.species).def.senses.field);
        let sees = |c: &crate::creatures::Live| self.in_view(&body, field, c.body.position + Vec3::Y * c.body.size.1 * 0.8);
        let threat = |c: &crate::creatures::Live| {
            let species = self.content.creatures.get(c.species);
            c.owner.is_none() && !c.companion && (species.def.kind == Kind::Monster || species.behaviours.iter().any(|b| matches!(b.behaviour, Behaviour::Hunt { .. })))
        };
        // Abed (in bed at last, not on the way to it), they sleep on; on the way, they take
        // fright and hurry on to bed (`place_now` keeps them going there).
        let abed = self.villagers.towns[&town].people[i].trip.as_ref().is_some_and(|t| {
            t.place == Place::Sleep && t.step + 1 >= t.path.len() && t.path.last().is_some_and(|c| (c.as_vec3() + Vec3::new(0.5, 0.0, 0.5)).with_y(0.0).distance(at.with_y(0.0)) < 1.0)
        });
        let frightened = !guard && !abed && self.creatures.live.iter().any(|(&other, c)| other != id && c.body.position.distance(at) <= react.fear && threat(c) && sees(c));
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        if frightened {
            let was = person.afraid_until > tick;
            person.afraid_until = tick + ticks(react.afraid);
            person.chat = None;
            if let Some(trip) = person.trip.as_mut().filter(|_| person.greeting.take().is_some()) {
                trip.next_spot = Some(tick);
                trip.face = None;
            }
            if !was {
                self.bark(id, Bark::Alarm);
            }
            return false;
        }
        let person = &self.villagers.towns[&town].people[i];
        // One struck, running from whoever struck them, greets no one.
        let afraid = person.afraid_until > tick || self.creatures.live.get(&id).is_some_and(|c| c.mind.attacker().is_some());
        let asleep = person.trip.as_ref().is_none_or(|t| t.place == Place::Sleep || t.pursuit.is_some());
        let due = |p: &crate::player::Player| person.greet_after.get(&crate::player_key(&p.name)).is_none_or(|&after| tick >= after);
        let me = &self.creatures.live[&id];
        let noticed = |p: &crate::player::Player| me.mind.noticed.iter().any(|n| n.0 == p.id) && self.in_view(&body, field, p.eye());
        let near = |p: &crate::player::Player| p.joined && p.position.with_y(0.0).distance(at.with_y(0.0)) <= react.greet && (p.position.y - at.y).abs() <= react.greet && noticed(p);
        let greeted = match person.greeting {
            Some((player, until)) if tick < until && !afraid => self.players.get(&player).filter(|p| p.joined).map(|p| (player, p.position, false)),
            Some(_) => None,
            None if !afraid && !asleep => self
                .players
                .values()
                .filter(|p| near(p) && due(p))
                .min_by(|a, b| a.position.distance(at).total_cmp(&b.position.distance(at)))
                .map(|p| (p.id, p.position, true)),
            None => None,
        };
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        let Some((player, eyes, new)) = greeted else {
            // A greeting over: back to their day from where they stand, no longer turned to
            // the player.
            if person.greeting.take().is_some()
                && let Some(trip) = person.trip.as_mut()
            {
                trip.next_spot = Some(tick);
                trip.face = None;
            }
            return self.talk(id, town, i, at, react, !afraid && !asleep && !guard);
        };
        // A player met ends any talk with a neighbour (the neighbour walks on alone).
        person.chat = None;
        let Some(trip) = person.trip.as_mut() else { return false };
        trip.path = vec![at.floor().as_ivec3()];
        trip.step = 0;
        trip.pose = Pose::Idle;
        trip.face = Some(eyes + Vec3::new(0.0, 1.6, 0.0));
        if new {
            person.greeting = Some((player, tick + ticks(react.greet_for)));
            if let Some(name) = self.players.get(&player).map(|p| crate::player_key(&p.name)) {
                person.greet_after.retain(|_, after| *after > tick);
                person.greet_after.insert(name, tick + ticks(react.greet_every));
            }
            self.bark(id, Bark::Greet);
        }
        true
    }

    /// Person `i` (creature `id`, at `at`), `free` (not afraid, abed nor a guard) and
    /// greeting no player, stops to pass the time of day with a neighbour of the town: one
    /// as free within `people.react.chat` they see, neither having talked within
    /// `chat_every`. Both stand turned to each other for `chat_for`, each with a contented
    /// hum, then go on with their day. Whether they stand talking.
    fn talk(&mut self, id: u32, town: u64, i: usize, at: Vec3, react: &ReactDef, free: bool) -> bool {
        let tick = self.tick;
        let t = &self.villagers.towns[&town];
        let idle = |j: usize| {
            let p = &t.people[j];
            !t.info.people[j].guard
                && p.greeting.is_none()
                && p.afraid_until <= tick
                && p.trip.as_ref().is_some_and(|t| t.place != Place::Sleep && t.pursuit.is_none())
                && p.creature.and_then(|c| self.creatures.live.get(&c)).is_some_and(|c| c.mind.attacker().is_none())
        };
        let head = |c: &crate::creatures::Live| c.body.position + Vec3::Y * c.body.size.1 * 0.8;
        if let Some((other, until)) = t.people[i].chat {
            // Talk goes on while both are still at it: the other still out, talking to them.
            let theirs = t.people.iter().position(|p| p.creature == Some(other));
            let on = free && tick < until && theirs.is_some_and(|j| t.people[j].chat.is_some_and(|(c, _)| c == id));
            let face = self.creatures.live.get(&other).map(head);
            let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
            let Some(trip) = person.trip.as_mut() else { return false };
            if on && let Some(face) = face {
                trip.face = Some(face);
                return true;
            }
            person.chat = None;
            trip.next_spot = Some(tick);
            trip.face = None;
            return false;
        }
        if !free || t.people[i].chat_after > tick || !idle(i) {
            return false;
        }
        let Some(me) = self.creatures.live.get(&id) else { return false };
        let (body, field) = (me.body, self.content.creatures.get(me.species).def.senses.field);
        let near = (0..t.people.len())
            .filter(|&j| j != i && t.people[j].chat.is_none() && t.people[j].chat_after <= tick && idle(j))
            .filter_map(|j| Some((j, t.people[j].creature?)))
            .filter_map(|(j, c)| Some((j, c, self.creatures.live.get(&c)?)))
            .filter(|(_, _, c)| {
                let p = c.body.position;
                p.with_y(0.0).distance(at.with_y(0.0)) <= react.chat && (p.y - at.y).abs() <= react.chat && self.in_view(&body, field, head(c))
            })
            .min_by(|a, b| a.2.body.position.distance(at).total_cmp(&b.2.body.position.distance(at)))
            .map(|(j, c, live)| (j, c, live.body.position));
        let Some((j, other, there)) = near else { return false };
        let mine = at + Vec3::Y * body.size.1 * 0.8;
        let theirs = head(&self.creatures.live[&other]);
        let people = &mut self.villagers.towns.get_mut(&town).expect("listed").people;
        for (k, with, stand, face) in [(i, other, at, theirs), (j, id, there, mine)] {
            let person = &mut people[k];
            person.chat = Some((with, tick + ticks(react.chat_for)));
            person.chat_after = tick + ticks(react.chat_every);
            let trip = person.trip.as_mut().expect("idle");
            trip.path = vec![stand.floor().as_ivec3()];
            trip.step = 0;
            trip.pose = Pose::Idle;
            trip.face = Some(face);
        }
        self.bark(id, Bark::Content);
        self.bark(other, Bark::Content);
        true
    }

    /// Person `i` (a guard, creature `id`) at `at` goes after `player`: a way to where they
    /// stand, found again every `repath`; `Defend` strikes once they are in reach. One that
    /// comes no nearer them for `give_up` gives up.
    fn pursue(&mut self, id: u32, town: u64, i: usize, at: Vec3, player: u32, live: &LiveDef) {
        let Some(quarry) = self.players.get(&player).map(|p| p.position) else { return };
        let tick = self.tick;
        // Require both the brain's starting range and the body's scaled striking range:
        // the old reach + 1 let a spear train a guard that could never strike back.
        let c = &self.creatures.live[&id];
        let def = &self.content.creatures.get(c.species).def;
        let reach = def.attack.map_or(1.0, |a| a.reach) + def.body.size.0.min(c.body.size.0) * 0.5 + 0.3;
        let in_reach = at.with_y(0.0).distance(quarry.with_y(0.0)) <= reach && (at.y - quarry.y).abs() < def.body.size.1.min(c.body.size.1) + 1.5;
        let d = at.distance(quarry);
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        let (best, since) = match person.quarry {
            Some((p, best, since)) if p == player && d + 0.5 >= best && !in_reach => (best, since),
            _ => (d, tick),
        };
        person.quarry = Some((player, best, since));
        if tick - since > ticks(live.give_up) {
            self.give_up(id, town, i, player, live);
            return;
        }
        let target = quarry.floor().as_ivec3();
        let trip = self.villagers.towns[&town].people[i].trip.as_ref();
        let due = trip.is_none_or(|t| t.pursuit != Some(player) || tick >= t.retry);
        let start = at.floor().as_ivec3();
        let way = if due { self.find_way(start, target, town, live) } else { Way::Later };
        let path = match way {
            Way::Found(path) => path,
            // No way there (they stand on a roof, past a wall): the guard holds where it is
            // rather than walk into the wall, and looks again after `repath`.
            Way::Unreached => vec![start],
            Way::Later => {
                if let Some(t) = self.villagers.towns.get_mut(&town).expect("listed").people[i].trip.as_mut() {
                    // Along the way, turn by turn.
                    if let Some(&next) = t.path.get(t.step)
                        && t.step + 1 < t.path.len()
                        && (next.as_vec3() + Vec3::new(0.5, 0.0, 0.5)).with_y(0.0).distance(at.with_y(0.0)) < 0.9
                    {
                        t.step += 1;
                    }
                }
                return;
            }
        };
        let (place, slot, day) = (Place::Patrol, usize::MAX, 0);
        let trip = Trip { place, slot, day, path, step: 0, pose: Pose::Idle, face: None, next_spot: None, leg: 0, best: (f32::MAX, tick), retry: tick + ticks(live.repath), pursuit: Some(player) };
        self.villagers.towns.get_mut(&town).expect("listed").people[i].trip = Some(trip);
    }

    /// Guard `id` breaks off its chase, teaches its quarry nothing for `calm`, and
    /// recovers only after another `give_up` seconds without a blow.
    fn give_up(&mut self, id: u32, town: u64, i: usize, player: u32, live: &LiveDef) {
        if let Some(c) = self.creatures.live.get_mut(&id) {
            c.mind.forgive();
            c.fought = None;
        }
        let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
        person.quarry = None;
        person.trip = None;
        person.calm_until = self.tick + ticks(live.calm);
        person.calm_players.retain(|_, until| *until > self.tick);
        if let Some(player) = self.players.get(&player) {
            person.calm_players.insert(crate::player_key(&player.name), person.calm_until);
        }
        person.heal_after = Some(self.tick + ticks(live.give_up));
        tracing::debug!(person = i, "a guard gave up the chase");
    }

    /// Whether a guard has given up on this player, so safe spots cannot train combat.
    pub(crate) fn villager_calm_for(&self, id: u32, player: u32) -> bool {
        self.villagers.by_creature.get(&id).is_some_and(|&(town, i)| {
            let p = &self.villagers.towns[&town].people[i];
            self.players.get(&player).is_some_and(|player| {
                p.calm_players.get(&crate::player_key(&player.name)).is_some_and(|until| *until > self.tick)
            })
        })
    }

    /// Records a blow (including misses and bleeding) before anger or experience is given.
    pub(crate) fn villager_struck(&mut self, id: u32, player: u32) -> bool {
        let delay = ticks(self.live().give_up);
        if let Some(&(town, i)) = self.villagers.by_creature.get(&id) {
            let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
            if person.heal_after.is_some() {
                person.heal_after = Some(self.tick + delay);
            }
        }
        self.villager_calm_for(id, player)
    }

    /// The way from `from` to `to` through settlement `town`, within this tick's search.
    fn find_way(&mut self, from: IVec3, to: IVec3, town: u64, live: &LiveDef) -> Way {
        let tick = self.tick;
        if self.villagers.failed.get(&(town, from, to)).is_some_and(|&until| until > tick) {
            return Way::Unreached;
        }
        if self.villagers.searched + live.search > live.search_per_tick {
            return Way::Later;
        }
        let (path, searched) = self.way(from, to, town, live.search);
        self.villagers.searched += searched;
        match path {
            Some(path) => Way::Found(path),
            None => {
                self.villagers.failed.insert((town, from, to), tick + ticks(live.failed_for));
                Way::Unreached
            }
        }
    }

    /// Picks the cell person `i` goes to for `trip`'s place, and how they are once there.
    fn target(&self, town: u64, i: usize, seed: u64, trip: &mut Trip, live: &LiveDef) -> Option<IVec3> {
        let t = &self.villagers.towns[&town];
        let resident = &t.info.people[i];
        let buildings = &t.info.buildings;
        let home = &buildings[resident.home];
        let centre = t.info.centre;
        let people = self.overworld.settlement_people();
        let (linger, r) = (people.linger, &live.spots);
        let stay = |trip: &mut Trip, salt: u64| {
            let seconds = linger.0 + (linger.1 - linger.0) * unit_from(mix(seed, salt));
            trip.next_spot = Some(self.tick + (seconds * TICKS_PER_SECOND as f32) as u64);
        };
        let first = |kind: BuildingKind| buildings.iter().enumerate().filter(|(_, b)| b.kind == kind).min_by_key(|(_, b)| ((b.min + b.max) / 2 - IVec2::new(home.min.x, home.min.y)).length_squared()).map(|(i, _)| i);
        let place = trip.place;
        let spot = match place {
            Place::Sleep | Place::Home => {
                trip.pose = if place == Place::Sleep { Pose::Resting } else { Pose::Idle };
                if place == Place::Home {
                    stay(trip, 1);
                }
                self.spot_inside(resident.home, t, seed)
            }
            Place::Yard => {
                stay(trip, 2);
                let door = home.door.unwrap_or(centre);
                self.near_spot(t, door, r.yard, seed)
            }
            Place::Work => match resident.work {
                Some(w) => {
                    let b = &buildings[w];
                    if matches!(b.kind, BuildingKind::House | BuildingKind::Trader | BuildingKind::Inn) {
                        stay(trip, 3);
                        trip.face = Some(((b.min + b.max).as_vec2() / 2.0).extend(b.floor as f32 + 1.5).xzy());
                        self.spot_inside(w, t, seed)
                    } else {
                        stay(trip, 4);
                        self.about(w, t, seed)
                    }
                }
                None => {
                    stay(trip, 2);
                    self.near_spot(t, home.door.unwrap_or(centre), r.yard, seed)
                }
            },
            Place::Square => {
                stay(trip, 5);
                trip.face = Some(centre.as_vec3() + Vec3::new(0.5, 1.5, 0.5));
                self.near_spot(t, centre, r.square, seed)
            }
            Place::Well => {
                stay(trip, 6);
                let w = first(BuildingKind::Well)?;
                trip.face = Some(((buildings[w].min + buildings[w].max).as_vec2() / 2.0 + 0.5).extend(buildings[w].floor as f32 + 1.5).xzy());
                self.about(w, t, seed)
            }
            Place::Inn => {
                stay(trip, 7);
                let inn = first(BuildingKind::Inn)?;
                self.spot_inside(inn, t, seed)
            }
            Place::Shrine => {
                stay(trip, 8);
                let s = buildings.iter().position(|b| is(b, &people.shrine))?;
                if buildings[s].kind == BuildingKind::House { self.spot_inside(s, t, seed) } else { self.about(s, t, seed) }
            }
            Place::Neighbour => {
                stay(trip, 9);
                let houses: Vec<usize> = (0..buildings.len()).filter(|&b| b != resident.home && buildings[b].kind == BuildingKind::House && buildings[b].door.is_some()).collect();
                let b = *houses.get((mix(seed, 10) % houses.len().max(1) as u64) as usize)?;
                let door = buildings[b].door?;
                trip.face = Some(door.as_vec3() - buildings[b].facing.step().extend(0).xzy().as_vec3() * 2.0 + Vec3::new(0.5, 1.5, 0.5));
                self.near_spot(t, door, r.neighbour, seed)
            }
            Place::Gate => {
                stay(trip, 11);
                let gates = &t.info.gates;
                let g = gates[(mix(seed, 12) % gates.len() as u64) as usize];
                let out = (g - IVec2::new(centre.x, centre.z)).as_vec2().normalize_or_zero();
                trip.face = Some(Vec3::new(g.x as f32 + out.x * 8.0, centre.y as f32 + 1.5, g.y as f32 + out.y * 8.0));
                self.near_spot(t, IVec3::new(g.x, centre.y, g.y), r.gate, seed)
            }
            Place::Patrol => {
                // Gate to gate by way of the square, a leg at a time.
                let gates = &t.info.gates;
                let stops = gates.len() * 2;
                let leg = (trip.leg + (seed % stops as u64) as usize) % stops;
                if leg % 2 == 1 {
                    self.near_spot(t, centre, r.patrol_square, seed)
                } else {
                    let g = gates[leg / 2];
                    self.near_spot(t, IVec3::new(g.x, centre.y, g.y), r.patrol_gate, seed)
                }
            }
            Place::Wander => {
                stay(trip, 13);
                let lanes: Vec<&Vec<Vec2>> = t.info.lanes.iter().filter(|l| !l.is_empty()).collect();
                let lane = lanes[(mix(seed, 14) % lanes.len() as u64) as usize];
                let p = lane[(mix(seed, 15) % lane.len() as u64) as usize];
                self.near_spot(t, IVec3::new(p.x as i32, centre.y, p.y as i32), r.wander, seed)
            }
        };
        spot
    }

    /// A cell a body can stand in inside building `b`, by `seed`.
    fn spot_inside(&self, b: usize, town: &Town, seed: u64) -> Option<IVec3> {
        let b = &town.info.buildings[b];
        let cells: Vec<IVec3> = b.room.iter()
            // Building::floor is the solid floor block; feet stand one above it.
            .map(|p| IVec3::new(p.x, b.floor + 1, p.y))
            .filter(|&c| self.stands(c))
            .collect();
        if cells.is_empty() {
            return None;
        }
        Some(cells[(mix(seed, 0x1a5) % cells.len() as u64) as usize])
    }

    /// A ground cell about a building. Wells, shrines and centrepieces use only the
    /// ring outside their footprint; nobody stands on their walls or in the pool.
    fn about(&self, b: usize, town: &Town, seed: u64) -> Option<IVec3> {
        let b = &town.info.buildings[b];
        let cells: Vec<IVec3> = (b.min.x - 1..=b.max.x + 1)
            .flat_map(|x| (b.min.y - 1..=b.max.y + 1).map(move |z| (x, z)))
            .filter(|&(x, z)| town.outdoor_column(IVec3::new(x, b.floor, z)))
            .filter_map(|(x, z)| (b.floor..=b.floor + 1).map(|y| IVec3::new(x, y, z)).find(|&c| self.stands(c)))
            .collect();
        if cells.is_empty() {
            return None;
        }
        Some(cells[(mix(seed, 0xab0) % cells.len() as u64) as usize])
    }

    /// A cell a body can stand in within `r` blocks of `at`, by `seed`.
    fn near_spot(&self, town: &Town, at: IVec3, r: i32, seed: u64) -> Option<IVec3> {
        for attempt in 0..12u64 {
            let h = mix(seed, 0x5907 + attempt);
            let dx = (h % (2 * r as u64 + 1)) as i32 - r;
            let dz = ((h >> 20) % (2 * r as u64 + 1)) as i32 - r;
            let column = at + IVec3::new(dx, 0, dz);
            if town.outdoor_column(column) && let Some(c) = self.standing_near(column, 4) {
                return Some(c);
            }
        }
        town.outdoor_column(at).then(|| self.standing_near(at, 6)).flatten()
    }

    /// The cell nearest `at`'s height in its column, within `reach` up or down, where a
    /// body can stand.
    fn standing_near(&self, at: IVec3, reach: i32) -> Option<IVec3> {
        (0..=reach).flat_map(|d| [d, -d - 1]).map(|dy| at + IVec3::Y * dy).find(|&c| self.stands(c))
    }

    /// Whether a body can stand in `cell`: ground beneath, room for its head, no water.
    fn stands(&self, cell: IVec3) -> bool {
        let info = |c: IVec3| self.block(c).map(|b| self.content.blocks.info(b));
        let free = |c: IVec3| info(c).is_some_and(|i| !i.solid && i.fluid.is_none());
        free(cell) && free(cell + IVec3::Y) && info(cell - IVec3::Y).is_some_and(|i| i.solid)
    }

    /// The way from `from` to `to` through settlement `town`, as the cells it turns at
    /// (the last `to`): an A* over the cells a body can stand in, stepping up one block
    /// or down two, searching `search` cells at most; and how many it searched.
    fn way(&self, from: IVec3, to: IVec3, town: u64, search: u32) -> (Option<Vec<IVec3>>, u32) {
        let t = &self.villagers.towns[&town];
        let (flat, goal) = (IVec2::new(from.x, from.z), IVec2::new(to.x, to.z));
        // The settlement and a margin, stretched to take in both ends (a guard's quarry may
        // stand outside it); `search` bounds the work whatever the box.
        let (min, max) = (t.min.min(flat).min(goal) - 8, t.max.max(flat).max(goal) + 8);
        let start = [from, from + IVec3::Y, from - IVec3::Y].into_iter().find(|&c| self.stands(c)).unwrap_or(from);
        let h = |c: IVec3| {
            let d = (c - to).abs();
            let (a, b) = (d.x.max(d.z), d.x.min(d.z));
            (a - b) as f32 + b as f32 * 1.414 + d.y as f32 * 0.5
        };
        let mut open = std::collections::BinaryHeap::new();
        let mut came: FxHashMap<IVec3, (IVec3, f32)> = FxHashMap::default();
        came.insert(start, (start, 0.0));
        open.push(Node(h(start), start));
        let mut searched = 0;
        let mut found = None;
        while let Some(Node(_, c)) = open.pop() {
            if c == to || (c.x == to.x && c.z == to.z && (c.y - to.y).abs() <= 1) {
                found = Some(c);
                break;
            }
            searched += 1;
            if searched > search {
                return (None, searched);
            }
            let g = came[&c].1;
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let side = IVec3::new(dx, 0, dz);
                let n = c + side;
                if n.x < min.x || n.x > max.x || n.z < min.y || n.z > max.y {
                    continue;
                }
                // Up one if there is room overhead to jump, else level, else down.
                let Some(next) = [n, n + IVec3::Y, n - IVec3::Y, n - IVec3::Y * 2].into_iter().find(|&m| self.stands(m) && (m.y <= c.y || self.passable(c + IVec3::Y * 2))) else { continue };
                // No cutting corners past walls.
                if dx != 0 && dz != 0 && !(self.passable(c + IVec3::new(dx, 0, 0)) && self.passable(c + IVec3::new(0, 0, dz)) && self.passable(c + IVec3::new(dx, 1, 0)) && self.passable(c + IVec3::new(0, 1, dz))) {
                    continue;
                }
                let cost = g + if dx != 0 && dz != 0 { 1.414 } else { 1.0 } + (next.y - c.y).abs() as f32 * 0.5;
                if came.get(&next).is_none_or(|&(_, old)| cost < old) {
                    came.insert(next, (c, cost));
                    open.push(Node(cost + h(next), next));
                }
            }
        }
        let Some(end) = found else { return (None, searched) };
        let mut cells = vec![end];
        let mut c = end;
        while c != start {
            c = came[&c].0;
            cells.push(c);
        }
        cells.reverse();
        // Only the turns, and the end.
        let mut turns = Vec::new();
        for w in 1..cells.len() {
            let last = w + 1 == cells.len();
            let bends = !last && (cells[w + 1] - cells[w]).xz() != (cells[w] - cells[w - 1]).xz();
            let climbs = !last && cells[w + 1].y != cells[w].y;
            if last || bends || climbs {
                turns.push(cells[w]);
            }
        }
        if turns.is_empty() {
            turns.push(end);
        }
        (Some(turns), searched)
    }

    fn passable(&self, cell: IVec3) -> bool {
        self.block(cell).is_some_and(|b| !self.content.blocks.info(b).solid)
    }

    /// Guards near a villager a player struck fight that player, but for those home after
    /// giving up a chase.
    fn call_guards(&mut self, live: &LiveDef) {
        let tick = self.tick;
        let mut calls: Vec<(u64, Vec3, u32)> = Vec::new();
        let mut alarms: Vec<(u32, u64, usize)> = Vec::new();
        for (&id, &(town, i)) in &self.villagers.by_creature {
            let Some(c) = self.creatures.live.get(&id) else { continue };
            if self.villagers.towns[&town].info.people[i].guard {
                continue;
            }
            if let Some(player) = c.mind.attacker() {
                calls.push((town, c.body.position, player));
                alarms.push((id, town, i));
            }
        }
        // Struck, they cry out (again no sooner than an `afraid` later).
        let afraid = ticks(self.overworld.settlement_people().react.afraid);
        for (id, town, i) in alarms {
            let person = &mut self.villagers.towns.get_mut(&town).expect("listed").people[i];
            if person.alarm_after <= tick {
                person.alarm_after = tick + afraid;
                self.bark(id, Bark::Alarm);
            }
        }
        for (town, at, player) in calls {
            let guards: Vec<u32> = self.villagers.towns[&town]
                .people
                .iter()
                .zip(&self.villagers.towns[&town].info.people)
                .filter(|(p, r)| r.guard && p.calm_until <= tick)
                .filter_map(|(p, _)| p.creature)
                .collect();
            for g in guards {
                let Some(guard) = self.creatures.live.get_mut(&g) else { continue };
                if guard.body.position.distance(at) > live.help || guard.mind.attacker() == Some(player) {
                    continue;
                }
                let species = self.content.creatures.get(guard.species);
                guard.mind.hurt(species, Some(player), 1.0);
            }
        }
    }
}

/// A cell to search, by its estimated cost, cheapest first.
#[derive(PartialEq)]
struct Node(f32, IVec3);

impl Eq for Node {}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::block::BlockId;

    /// Flat, loaded floor and a one-room settlement: lifecycle tests need no travel,
    /// streaming, wall clock, or expensive settlement search.
    fn fixture(species: &str, count: usize) -> Server {
        use voxmar_engine::world::Column;
        use voxmar_worldgen::{CultureInfo, SettlementBuilding, SettlementFacing, people::Resident};
        let mut server = crate::test_server();
        server.world = Default::default();
        let mut column = Column::default();
        let stone = server.content.blocks.expect("base:stone");
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, 199, z), stone);
                column.set_block(IVec3::new(x, 200, z), BlockId::AIR);
            }
        }
        server.world.insert(ColumnPos::of_block(IVec3::ZERO), std::sync::Arc::new(column));
        server.day = TICKS_PER_DAY as f64 * 0.8;
        let routine = server.overworld.settlement_people().routine("worker").unwrap();
        let info = SettlementInfo {
            id: 1, name: ("Test".into(), "テスト".into()), size: "village".into(), style: "timber".into(), layout: "test".into(),
            centre: IVec3::new(8, 200, 8), levers: SettlementLevers { levers: [0.5; 5], axes: [0.5; 8] }, gates: vec![IVec2::new(8, 12)], lanes: Vec::new(),
            culture: CultureInfo { family: "hearth".into(), title: ("Hearth".into(), "炉辺".into()), second: None,
                quirks: Vec::new(), axes: [0.5; 8], dropped: Vec::new(), borrowed: Vec::new() },
            buildings: vec![SettlementBuilding { kind: BuildingKind::House, min: IVec2::new(4, 4), max: IVec2::new(12, 12), floor: 199,
                room: (6..=10).flat_map(|x| (6..=10).map(move |z| IVec2::new(x, z))).collect(), byre: Vec::new(),
                door: Some(IVec3::new(8, 200, 13)), facing: SettlementFacing::South, what: "cottage".into(), work: None, beds: Vec::new(), seats: Vec::new() }],
            people: (0..count).map(|i| Resident { species: species.into(), home: 0, work: None, routine, guard: species == "base:guard", seed: 100 + i as u64 }).collect(),
            walls: Vec::new(), wall_block: None,
        };
        server.villagers.remember_town(info, 12);
        server
    }

    /// Creature `id` has noticed player `player` (as `brain::think` would, the player in its
    /// sight or close by).
    fn notice(server: &mut Server, id: u32, player: u32) {
        server.creatures.live.get_mut(&id).unwrap().mind.noticed.push((player, 10.0, 0.0));
    }

    fn player(server: &mut Server) {
        let (_, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, Vec3::new(8.0, 200.0, 8.0), 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        player.name = "Tëster".into();
        server.players.insert(1, player);
    }

    /// Sets the server's clock to the first time of day person 0 of settlement 1 is out in
    /// the open (not at home nor in bed).
    fn out_in_the_open(server: &mut Server) {
        let seed = mix(server.villagers.towns[&1].info.people[0].seed, 0);
        for step in 0..40 {
            server.day = TICKS_PER_DAY as f64 * step as f64 / 40.0;
            if !matches!(server.place_now(1, 0, seed).0, Place::Home | Place::Sleep) {
                return;
            }
        }
        panic!("never out of doors in a day");
    }

    /// Someone out of doors with a player stepping up to them stops where they stand, turns
    /// to them and greets them, once: their day waits until the greeting is over, and they
    /// greet no one again before `greet_every`.
    #[test]
    fn a_player_near_is_greeted_once_and_the_day_waits() {
        let mut server = fixture("base:villager", 1);
        out_in_the_open(&mut server);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let live = server.live();
        let react = server.overworld.settlement_people().react.clone();
        server.tick = 1_000;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].greeting.is_none(), "greeted no one");
        player(&mut server);
        let at = server.creatures.live[&id].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(react.greet - 1.0, 0.0, 0.0);
        // Noticed (close by, they hear them) but behind a wall: not seen, not greeted.
        notice(&mut server, id, 1);
        let wall: Vec<IVec3> = (0..3).map(|y| at.floor().as_ivec3() + IVec3::new(1, y, 0)).collect();
        let stone = server.content.blocks.expect("base:stone");
        wall.iter().for_each(|&cell| server.set_block(cell, stone));
        server.tick += 1;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].greeting.is_none(), "greeted a player through a wall");
        wall.iter().for_each(|&cell| server.set_block(cell, BlockId::AIR));
        server.tick += 1;
        server.follow_day(id, 1, 0, &live);
        let person = &server.villagers.towns[&1].people[0];
        let (whom, until) = person.greeting.expect("greeted the player");
        assert_eq!(whom, 1);
        let trip = person.trip.as_ref().unwrap();
        assert_eq!(trip.path, [at.floor().as_ivec3()], "walked on while greeting");
        assert!(trip.face.is_some_and(|f| f.x > at.x), "not turned to the player: {:?}", trip.face);
        let after = *person.greet_after.values().next().expect("the player's wait");
        server.tick = until - 1;
        server.follow_day(id, 1, 0, &live);
        assert_eq!(server.villagers.towns[&1].people[0].greet_after.values().copied().collect::<Vec<_>>(), [after], "greeted again within the greeting");
        server.tick = until;
        server.follow_day(id, 1, 0, &live);
        let person = &server.villagers.towns[&1].people[0];
        assert!(person.greeting.is_none(), "the greeting did not end");
        server.tick = after - 1;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].greeting.is_none(), "greeted again before greet_every");
        server.tick = after;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].greeting.is_some(), "no greeting once greet_every had passed");
    }

    /// The wait to greet again is each player's own; and one struck stops greeting the
    /// player who struck them, and greets no one while they remember the blow.
    #[test]
    fn the_greeting_wait_is_each_players_and_the_struck_greet_no_one() {
        let mut server = fixture("base:villager", 1);
        out_in_the_open(&mut server);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let live = server.live();
        let react = server.overworld.settlement_people().react.clone();
        server.tick = 1_000;
        player(&mut server);
        let at = server.creatures.live[&id].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(react.greet - 1.0, 0.0, 0.0);
        notice(&mut server, id, 1);
        server.follow_day(id, 1, 0, &live);
        let (_, until) = server.villagers.towns[&1].people[0].greeting.expect("greeted the first player");
        // The first gone and another come, after the greeting and well within the first's wait.
        server.players.remove(&1);
        server.tick = until;
        server.follow_day(id, 1, 0, &live);
        let (_, connection) = voxmar_protocol::local_pair();
        let mut other = crate::player::Player::new(2, connection, at + Vec3::new(react.greet - 1.0, 0.0, 0.0), 2, server.tick, server.survival.max_health, &server.config.limits);
        other.joined = true;
        other.name = "Other".into();
        server.players.insert(2, other);
        notice(&mut server, id, 2);
        server.tick += 1;
        server.follow_day(id, 1, 0, &live);
        let (whom, _) = server.villagers.towns[&1].people[0].greeting.expect("the second player waited on the first's greeting");
        assert_eq!(whom, 2);
        // Struck by them: the greeting ends there, and no one is greeted while it is remembered.
        server.creatures.live.get_mut(&id).unwrap().mind.remember(voxmar_engine::creature::brain::Memory::HurtBy(2), 30.0);
        server.tick += 1;
        server.follow_day(id, 1, 0, &live);
        let person = &server.villagers.towns[&1].people[0];
        assert!(person.greeting.is_none(), "still greeting the player who struck them");
        let eyes = server.players[&2].position + Vec3::new(0.0, 1.6, 0.0);
        assert!(person.trip.as_ref().is_none_or(|t| t.face != Some(eyes)), "still facing the player who struck them");
        server.tick += ticks(react.greet_every) + 1;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].greeting.is_none(), "greeted while running from a blow");
    }

    /// Two neighbours out and about who see each other stop and talk: both stand turned to
    /// each other for `chat_for`, then go on with their day, and stop to talk no sooner
    /// than `chat_every`. Guards keep watch and talk to no one.
    #[test]
    fn neighbours_who_meet_stop_to_talk_but_not_guards() {
        for (species, talk) in [("base:villager", true), ("base:guard", false)] {
            let mut server = fixture(species, 2);
            // A time of day both are out of doors.
            let seeds: Vec<u64> = (0..2).map(|i| mix(server.villagers.towns[&1].info.people[i].seed, 0)).collect();
            let out = (0..40).map(|step| TICKS_PER_DAY as f64 * step as f64 / 40.0).find(|&day| {
                server.day = day;
                (0..2).all(|i| !matches!(server.place_now(1, i, seeds[i]).0, Place::Home | Place::Sleep))
            });
            server.day = out.expect("never both out of doors in a day");
            server.bring_out(1, 0);
            server.bring_out(1, 1);
            let ids: Vec<u32> = (0..2).map(|i| server.villagers.towns[&1].people[i].creature.expect("out")).collect();
            let live = server.live();
            let react = server.overworld.settlement_people().react.clone();
            server.tick = 1_000;
            for i in 0..2 {
                server.follow_day(ids[i], 1, i, &live);
            }
            let at = server.creatures.live[&ids[0]].body.position;
            server.creatures.live.get_mut(&ids[1]).unwrap().body.position = at + Vec3::new(react.chat - 0.5, 0.0, 0.0);
            server.tick += 1;
            server.follow_day(ids[0], 1, 0, &live);
            let people = &server.villagers.towns[&1].people;
            if !talk {
                assert!(people.iter().all(|p| p.chat.is_none()), "{species} stopped to talk");
                continue;
            }
            assert_eq!(people[0].chat.map(|c| c.0), Some(ids[1]), "did not stop to talk");
            assert_eq!(people[1].chat.map(|c| c.0), Some(ids[0]), "the neighbour did not stop");
            for i in 0..2 {
                let other = &server.creatures.live[&ids[1 - i]];
                let face = people[i].trip.as_ref().and_then(|t| t.face).expect("turned to no one");
                assert!(face.with_y(0.0).distance(other.body.position.with_y(0.0)) < 0.01, "not turned to the neighbour");
            }
            // Talk goes on while it lasts, then both go on with their day.
            let until = people[0].chat.unwrap().1;
            server.tick = until - 1;
            for i in 0..2 {
                assert!(server.react(ids[i], 1, i, server.creatures.live[&ids[i]].body.position, &react), "stopped talking early");
            }
            server.tick = until;
            for i in 0..2 {
                server.follow_day(ids[i], 1, i, &live);
            }
            let people = &server.villagers.towns[&1].people;
            assert!(people.iter().all(|p| p.chat.is_none() && p.trip.as_ref().is_none_or(|t| t.face.is_none())), "still talking");
            // Still side by side, within `chat_every`: no more talk.
            let at = server.creatures.live[&ids[0]].body.position;
            server.creatures.live.get_mut(&ids[1]).unwrap().body.position = at + Vec3::new(1.0, 0.0, 0.0);
            server.tick += 1;
            server.follow_day(ids[0], 1, 0, &live);
            assert!(server.villagers.towns[&1].people[0].chat.is_none(), "talked again within chat_every");
        }
    }

    /// A monster near sends folk home crying out for `afraid`; a guard stands its ground.
    #[test]
    fn a_monster_near_sends_folk_home_but_not_guards() {
        for (species, frightened) in [("base:villager", true), ("base:guard", false)] {
            let mut server = fixture(species, 1);
            out_in_the_open(&mut server);
            server.bring_out(1, 0);
            let id = server.villagers.towns[&1].people[0].creature.expect("out");
            let live = server.live();
            server.tick = 1_000;
            let (before, _, _) = server.place_now(1, 0, server.creatures.live[&id].seed);
            assert_ne!(before, Place::Home, "{species}: home already");
            let monster = server.content.creatures.id("base:shambler").expect("a monster");
            let at = server.creatures.live[&id].body.position;
            server.place_creature(monster, at + Vec3::new(3.0, 0.0, 0.0), 9, 0.0).expect("placed");
            server.follow_day(id, 1, 0, &live);
            let afraid = server.villagers.towns[&1].people[0].afraid_until > server.tick;
            assert_eq!(afraid, frightened, "{species}: afraid {afraid}");
            let (place, _, _) = server.place_now(1, 0, server.creatures.live[&id].seed);
            assert_eq!(place == Place::Home, frightened, "{species} goes to {place:?}");
        }
    }

    /// A bark reaches the players within its own `barking` range who see the one barking
    /// (an alarm farther than a greeting), the same bark to the same player once in
    /// `barking.every` (another bark, or another player, waits on nothing), and an admin's
    /// `/creatures bark` heeds neither.
    #[test]
    fn a_bark_carries_its_range_and_waits_per_player() {
        use voxmar_protocol::ServerMessage;
        let mut server = fixture("base:villager", 1);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let at = server.creatures.live[&id].body.position;
        let barking = server.content.creatures.get(server.creatures.live[&id].species).def.barking;
        server.tick = 1_000;
        let mut ears = Vec::new();
        for (n, off) in [(1u32, 2.0), (2, barking.greet - 1.0), (3, barking.greet + 1.0)] {
            let (client, connection) = voxmar_protocol::local_pair();
            let mut p = crate::player::Player::new(n, connection, at + Vec3::new(off, 0.0, 0.0), 2, server.tick, server.survival.max_health, &server.config.limits);
            p.joined = true;
            p.name = format!("P{n}");
            server.players.insert(n, p);
            server.creatures.show(n, id);
            ears.push(client);
        }
        let heard = |ears: &[voxmar_protocol::ClientConnection]| -> Vec<Vec<Bark>> {
            ears.iter().map(|c| std::iter::from_fn(|| c.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Bark { creature, bark } if creature == id => Some(bark), _ => None }).collect()).collect()
        };
        server.bark(id, Bark::Greet);
        server.bark(id, Bark::Greet);
        server.bark(id, Bark::Alarm);
        assert!(barking.alarm > barking.greet + 1.0, "an alarm carries no farther than a greeting");
        assert_eq!(heard(&ears), [vec![Bark::Greet, Bark::Alarm], vec![Bark::Greet, Bark::Alarm], vec![Bark::Alarm]], "within range once each, a greeting's range out only the alarm");
        server.tick += ticks(barking.every) - 1;
        server.bark(id, Bark::Greet);
        assert_eq!(heard(&ears), [vec![], vec![], vec![]], "greeted again within `every`");
        server.bark_command(at, "greet");
        assert_eq!(heard(&ears), [vec![Bark::Greet], vec![Bark::Greet], vec![Bark::Greet]], "the command waited, or kept to the range");
        server.tick += ticks(barking.every) + 1;
        server.bark(id, Bark::Greet);
        assert_eq!(heard(&ears)[1], [Bark::Greet], "never greeted again after `every`");
    }

    /// Someone abed with a monster near sleeps on: fright sends folk home, and they are home.
    #[test]
    fn a_monster_near_leaves_sleepers_abed() {
        let mut server = fixture("base:villager", 1);
        let seed = mix(server.villagers.towns[&1].info.people[0].seed, 0);
        let night = (0..40).map(|step| TICKS_PER_DAY as f64 * step as f64 / 40.0).find(|&day| {
            server.day = day;
            server.place_now(1, 0, seed).0 == Place::Sleep
        });
        server.day = night.expect("never abed in a day");
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let live = server.live();
        server.tick = 1_000;
        server.follow_day(id, 1, 0, &live);
        assert_eq!(server.villagers.towns[&1].people[0].trip.as_ref().map(|t| t.place), Some(Place::Sleep), "not abed");
        let monster = server.content.creatures.id("base:shambler").expect("a monster");
        let at = server.creatures.live[&id].body.position;
        server.place_creature(monster, at + Vec3::new(2.0, 0.0, 0.0), 9, 0.0).expect("placed");
        server.tick += 1;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].afraid_until <= server.tick, "a sleeper frightened");
        assert_eq!(server.villagers.towns[&1].people[0].trip.as_ref().map(|t| t.place), Some(Place::Sleep), "got up for a monster");
        // And one frightened before bedtime goes to bed when the day says, not home to stand.
        server.villagers.towns.get_mut(&1).unwrap().people[0].afraid_until = server.tick + 100;
        assert_eq!(server.place_now(1, 0, seed).0, Place::Sleep, "kept from bed by a fright");
    }

    /// Reaching their bed they sigh content once a night, though the night's sleep runs
    /// past midnight and its trip is planned again on the new day.
    #[test]
    fn a_bedtime_sigh_comes_once_a_night() {
        let mut server = fixture("base:villager", 1);
        let seed = mix(server.villagers.towns[&1].info.people[0].seed, 0);
        let night = (0..40).map(|step| TICKS_PER_DAY as f64 * step as f64 / 40.0).find(|&day| {
            server.day = day;
            server.place_now(1, 0, seed).0 == Place::Sleep
        });
        server.day = night.expect("never abed in a day");
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let live = server.live();
        server.tick = 1_000;
        for _ in 0..3 {
            server.tick += 1;
            server.follow_day(id, 1, 0, &live);
        }
        let first = server.villagers.towns[&1].people[0].sighed.expect("no sigh in bed");
        // A new day's trip to the same bed, a moment on.
        server.villagers.towns.get_mut(&1).unwrap().people[0].trip = None;
        server.day += 0.01 * TICKS_PER_DAY as f64;
        for _ in 0..3 {
            server.tick += 1;
            server.follow_day(id, 1, 0, &live);
        }
        assert_eq!(server.villagers.towns[&1].people[0].sighed, Some(first), "sighed again the same night");
    }

    /// One still on the way to bed who sees a monster takes fright (and cries out), and
    /// still goes to bed.
    #[test]
    fn a_monster_near_frightens_one_on_the_way_to_bed() {
        let mut server = fixture("base:villager", 1);
        let seed = mix(server.villagers.towns[&1].info.people[0].seed, 0);
        let night = (0..40).map(|step| TICKS_PER_DAY as f64 * step as f64 / 40.0).find(|&day| {
            server.day = day;
            server.place_now(1, 0, seed).0 == Place::Sleep
        });
        server.day = night.expect("never abed in a day");
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let live = server.live();
        server.tick = 1_000;
        server.follow_day(id, 1, 0, &live);
        // Out of the house, on the way back to bed.
        let bed = server.villagers.towns[&1].people[0].trip.as_ref().and_then(|t| t.path.last().copied()).expect("a way to bed");
        let away = bed.as_vec3() + Vec3::new(0.5, 0.0, 6.5);
        server.creatures.live.get_mut(&id).unwrap().body.position = away;
        let monster = server.content.creatures.id("base:shambler").expect("a monster");
        server.place_creature(monster, away + Vec3::new(2.0, 0.0, 0.0), 9, 0.0).expect("placed");
        server.tick += 1;
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].afraid_until > server.tick, "one on the way to bed was not frightened");
        assert_eq!(server.place_now(1, 0, seed).0, Place::Sleep, "frightened from going to bed");
    }

    /// A monster near but out of sight (under the floor, as in a cave below the square)
    /// frightens no one: fear goes by the same look as everything a creature sees.
    #[test]
    fn a_monster_unseen_under_the_floor_frightens_no_one() {
        let mut server = fixture("base:villager", 1);
        out_in_the_open(&mut server);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        let live = server.live();
        server.tick = 1_000;
        let monster = server.content.creatures.id("base:shambler").expect("a monster");
        let at = server.creatures.live[&id].body.position;
        server.place_creature(monster, at + Vec3::new(2.0, -4.0, 0.0), 9, 0.0).expect("placed");
        server.follow_day(id, 1, 0, &live);
        assert!(server.villagers.towns[&1].people[0].afraid_until <= server.tick, "frightened by a monster under the floor");
    }

    /// Rain sends folk in from the open (a yard, for someone with no work to go to), and
    /// leaves the guards at their watch.
    #[test]
    fn rain_sends_folk_in_from_the_open() {
        for (species, sheltering) in [("base:villager", true), ("base:guard", false)] {
            let mut server = fixture(species, 1);
            out_in_the_open(&mut server);
            let seed = mix(server.villagers.towns[&1].info.people[0].seed, 0);
            let (dry, slot, _) = server.place_now(1, 0, seed);
            let open = dry != Place::Home && dry != Place::Sleep;
            assert!(open, "{species} indoors: {dry:?}");
            server.villagers.towns.get_mut(&1).unwrap().raining = true;
            let (wet, wet_slot, _) = server.place_now(1, 0, seed);
            assert_eq!((wet == Place::Home, wet_slot), (sheltering, slot), "{species}: {dry:?} in the dry, {wet:?} in the rain");
        }
    }

    /// A town's sky is looked at as the weather is: rain over it past `react.rain` makes it
    /// raining, clear skies dry again.
    #[test]
    fn a_town_knows_when_it_rains() {
        let mut server = fixture("base:villager", 1);
        server.villagers.towns.get_mut(&1).unwrap().alive = true;
        let centre = server.villagers.towns[&1].info.centre;
        let at = Vec2::new(centre.x as f32 + 0.5, centre.z as f32 + 0.5);
        let tick = server.tick;
        for (name, raining) in [("rain", true), ("clear", false), ("rain", true)] {
            let preset = server.content.weather.preset(name).expect("a preset").clone();
            server.weather.force(preset.at(1.0), at, 60, tick);
            server.look_at_the_sky();
            assert_eq!(server.villagers.towns[&1].raining, raining, "under {name}");
        }
    }

    /// The spot a person sets out for is drawn by the day, the slot and the leg, never the
    /// tick they set out on.
    #[test]
    fn a_persons_spot_draws_by_the_day_not_the_tick() {
        let set_out = |tick: u64| {
            let mut server = fixture("base:farmer", 1);
            server.bring_out(1, 0);
            let id = server.villagers.towns[&1].people[0].creature.expect("out");
            server.tick = tick;
            let live = server.live();
            server.follow_day(id, 1, 0, &live);
            server.villagers.towns[&1].people[0].trip.as_ref().map(|t| (t.place, t.slot, t.day, t.leg, t.path.clone()))
        };
        let first = set_out(5_000);
        assert!(first.as_ref().is_some_and(|t| !t.4.is_empty()), "no trip set out on: {first:?}");
        assert_eq!(set_out(5_000 + 7_919), first, "the same day's trip set out later went elsewhere");
    }

    /// Each try in a row that finds no spot counts a miss, and draws the next pick by it;
    /// a try that finds one counts the misses back to none.
    #[test]
    fn a_person_who_finds_no_spot_counts_misses_and_picks_anew() {
        let floor = |server: &mut Server, block: BlockId| {
            for x in 0..32 {
                for z in 0..32 {
                    server.set_block(IVec3::new(x, 199, z), block);
                }
            }
        };
        let mut server = fixture("base:farmer", 1);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.expect("out");
        server.tick = 5_000;
        let live = server.live();
        // No ground to stand on anywhere: every try misses, and counts it.
        floor(&mut server, BlockId::AIR);
        server.villagers.towns.get_mut(&1).unwrap().people[0].trip = None;
        let mut misses = Vec::new();
        for _ in 0..3 {
            misses.push(server.villagers.towns[&1].people[0].misses);
            server.follow_day(id, 1, 0, &live);
            assert!(server.villagers.towns[&1].people[0].trip.is_none(), "set out with nowhere to stand");
        }
        assert_eq!(misses, [0, 1, 2]);
        assert_eq!(server.villagers.towns[&1].people[0].misses, 3);
        let stone = server.content.blocks.expect("base:stone");
        floor(&mut server, stone);
        server.follow_day(id, 1, 0, &live);
        let after = server.villagers.towns[&1].people[0].trip.as_ref().and_then(|t| t.path.last().copied());
        assert!(after.is_some(), "found nowhere with ground again");
        assert_eq!(server.villagers.towns[&1].people[0].misses, 0, "a find did not count the misses back");
        // The same try with no misses before it picks otherwise.
        let mut fresh = fixture("base:farmer", 1);
        fresh.bring_out(1, 0);
        let id = fresh.villagers.towns[&1].people[0].creature.expect("out");
        fresh.tick = 5_000;
        fresh.villagers.towns.get_mut(&1).unwrap().people[0].trip = None;
        fresh.follow_day(id, 1, 0, &live);
        let unmissed = fresh.villagers.towns[&1].people[0].trip.as_ref().and_then(|t| t.path.last().copied());
        assert!(unmissed.is_some());
        assert_ne!(after, unmissed, "three misses picked as none did");
    }

    /// A resident is the same individual on each visit, but thinks afresh: each coming out
    /// starts its count of rolls at the world's next cast, not at none again.
    #[test]
    fn a_resident_out_again_is_the_same_but_thinks_afresh() {
        let mut server = fixture("base:farmer", 1);
        let mut out = Vec::new();
        for _ in 0..3 {
            server.bring_out(1, 0);
            let id = server.villagers.towns[&1].people[0].creature.expect("out");
            out.push((server.creatures.live[&id].seed, server.creatures.live[&id].rolls));
            server.release_town(1);
        }
        assert!(out.iter().all(|o| o.0 == out[0].0), "not the same individual: {out:?}");
        let starts: std::collections::BTreeSet<u64> = out.iter().map(|o| o.1).collect();
        assert_eq!(starts.len(), 3, "a visit started its thoughts where another did: {out:?}");
    }

    #[test]
    fn dead_restore_skips_until_due_then_changes_seed() {
        let mut server = fixture("base:farmer", 1);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.unwrap();
        let original = server.creatures.live[&id].seed;
        server.creature_dies(id);
        let saved = server.villagers.saved();
        assert_eq!(saved.len(), 1);
        let due = saved[0].away_until;
        let info = server.villagers.towns[&1].info.clone();
        server.villagers = Villagers::restore(saved);
        server.villagers.remember_town(info, 12);
        server.day = (due - 0.001) * TICKS_PER_DAY as f64;
        server.bring_out(1, 0);
        assert!(server.villagers.towns[&1].people[0].creature.is_none());
        server.day = due * TICKS_PER_DAY as f64;
        server.bring_out(1, 0);
        let next = server.villagers.towns[&1].people[0].creature.unwrap();
        assert_ne!(server.creatures.live[&next].seed, original);
        assert_eq!(server.villagers.generation(1, 0), 1);
        let new_seed = server.creatures.live[&next].seed;
        server.release_town(1);
        server.bring_out(1, 0);
        let returned = server.villagers.towns[&1].people[0].creature.unwrap();
        assert_eq!(server.creatures.live[&returned].seed, new_seed);
    }

    #[test]
    fn vanished_resident_is_released_without_a_newcomer() {
        let mut server = fixture("base:farmer", 1);
        server.bring_out(1, 0);
        let id = server.villagers.towns[&1].people[0].creature.unwrap();
        let seed = server.creatures.live[&id].seed;
        server.creatures.take_away(id);
        server.note_all_deaths();
        assert!(server.villagers.saved().is_empty());
        server.bring_out(1, 0);
        let next = server.villagers.towns[&1].people[0].creature.unwrap();
        assert_eq!(server.creatures.live[&next].seed, seed);
    }

    #[test]
    fn dispersing_wild_creatures_preserves_a_residents_body_and_mapping() {
        let mut server = fixture("base:farmer", 1);
        player(&mut server);
        server.players.get_mut(&1).unwrap().admin = true;
        server.bring_out(1, 0);
        let resident = server.villagers.towns[&1].people[0].creature.unwrap();
        server.creatures.live.get_mut(&resident).unwrap().health = 1;
        let sheep = server.content.creatures.id("base:mouflon").unwrap();
        let wild = server.spawn_creature(sheep, Vec3::new(8.0, 200.0, 8.0), 7).unwrap();
        server.command(1, "creatures disperse");
        assert!(!server.creatures.live.contains_key(&wild), "the command must actually disperse wild creatures");
        assert_eq!(server.creatures.live[&resident].health, 1, "dispersal must leave the resident's living body intact");
        assert_eq!(server.villagers.by_creature.get(&resident), Some(&(1, 0)));
        server.note_all_deaths();
        server.bring_out(1, 0);
        assert_eq!(server.villagers.towns[&1].people[0].creature, Some(resident), "the lifecycle scan must not replace the resident");
        assert_eq!(server.creatures.live[&resident].health, 1);
        assert_eq!(server.villagers.by_creature.get(&resident), Some(&(1, 0)));
        assert!(server.villagers.saved().is_empty(), "dispersal must not mark a resident dead");
    }

    #[test]
    fn a_guard_cooldown_stops_training_and_delays_recovery() {
        use voxmar_engine::combat::Blow;
        let mut server = fixture("base:guard", 1);
        player(&mut server);
        server.bring_out(1, 0);
        let mut id = server.villagers.towns[&1].people[0].creature.unwrap();
        let species = server.creatures.live[&id].species;
        let skill = server.content.skills.id("core:attack").unwrap();
        let defence = server.content.skills.id("core:defence").unwrap();
        server.players.get_mut(&1).unwrap().skills.grant(defence, server.content.skills.xp_for(20));
        server.hurt_creature(1, id, 1, Some(skill), Blow::Plain);
        let before = server.players[&1].skills.xp(skill);
        assert!(before > 0, "reachable guard teaches combat");
        let before_defence = server.players[&1].skills.xp(defence);
        server.defence_xp(1, id, species);
        assert!(server.players[&1].skills.xp(defence) > before_defence, "reachable guard teaches Defence");
        let live = server.live();
        // Above a short guard's striking height but inside the unscaled brain range,
        // it tries, makes no progress, and then breaks off.
        let creature = server.creatures.live.get_mut(&id).unwrap();
        creature.scale *= 0.5;
        creature.adult_scale *= 0.5;
        creature.body.size = (creature.body.size.0 * 0.5, creature.body.size.1 * 0.5);
        let at = server.creatures.live[&id].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::Y * 2.8;
        server.pursue(id, 1, 0, at, 1, &live);
        assert!(!server.villager_calm_for(id, 1), "gave up without trying");
        server.tick += ticks(live.give_up) + 1;
        server.pursue(id, 1, 0, at, 1, &live);
        let full = server.content.creatures.get(species).def.health;
        assert_eq!(server.creatures.live[&id].health, full - 1, "no instant heal");
        assert!(server.villager_calm_for(id, 1));
        assert!(!server.villager_calm_for(id, 2), "cooldown belongs to the quarry");
        let info = server.villagers.towns[&1].info.clone();
        server.release_town(1);
        server.villagers.towns.remove(&1);
        server.villagers.remember_town(info, 12);
        server.bring_out(1, 0);
        id = server.villagers.towns[&1].people[0].creature.unwrap();
        assert!(server.villager_calm_for(id, 1), "release lost the cooldown");
        assert_eq!(server.creatures.live[&id].health, full - 1, "forgetting healed it");
        let (_, connection) = voxmar_protocol::local_pair();
        let mut rejoined = crate::player::Player::new(2, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        rejoined.name = "tËSTER".into();
        server.players.insert(2, rejoined);
        assert!(server.villager_calm_for(id, 2), "rejoining reset the cooldown");
        server.tick += ticks(live.give_up) - 1;
        server.hurt_creature(1, id, 1, Some(skill), Blow::Plain);
        let defence_before = server.players[&1].skills.xp(defence);
        server.defence_xp(1, id, species);
        assert_eq!(server.players[&1].skills.xp(skill), before);
        assert_eq!(server.players[&1].skills.xp(defence), defence_before);
        assert!(server.creatures.live[&id].mind.attacker().is_none(), "hit restarted pursuit");
        server.tick += 2;
        server.follow_day(id, 1, 0, &live);
        assert_eq!(server.creatures.live[&id].health, full - 2, "latest hit delayed recovery");
        server.tick += ticks(live.give_up);
        server.follow_day(id, 1, 0, &live);
        assert_eq!(server.creatures.live[&id].health, full);
        server.tick = server.villagers.towns[&1].people[0].calm_until;
        server.hurt_creature(1, id, 1, Some(skill), Blow::Plain);
        assert!(server.players[&1].skills.xp(skill) > before, "cooldown ends");
        assert_eq!(server.creatures.live[&id].mind.attacker(), Some(1));
    }

    #[test]
    fn alternating_quarries_keep_their_own_guard_cooldowns() {
        use voxmar_engine::combat::Blow;
        let mut server = fixture("base:guard", 1);
        player(&mut server);
        let (_, connection) = voxmar_protocol::local_pair();
        let mut second = crate::player::Player::new(2, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        second.name = "Second".into();
        server.players.insert(2, second);
        server.bring_out(1, 0);
        let mut id = server.villagers.towns[&1].people[0].creature.unwrap();
        let species = server.creatures.live[&id].species;
        let attack = server.content.skills.id("core:attack").unwrap();
        let defence = server.content.skills.id("core:defence").unwrap();
        let live = server.live();
        for quarry in [1, 2] {
            server.players.get_mut(&quarry).unwrap().skills.grant(defence, server.content.skills.xp_for(20));
            server.hurt_creature(quarry, id, 1, Some(attack), Blow::Plain);
            let before = server.players[&quarry].skills.xp(defence);
            server.defence_xp(quarry, id, species);
            assert!(server.players[&quarry].skills.xp(attack) > 0);
            assert!(server.players[&quarry].skills.xp(defence) > before);
            server.tick += ticks(live.give_up) + 1;
            server.give_up(id, 1, 0, quarry, &live);
        }
        let expiries = server.villagers.towns[&1].people[0].calm_players.clone();
        assert_eq!(expiries.len(), 2);
        let first_due = expiries[&crate::player_key(&server.players[&1].name)];
        let second_due = expiries[&crate::player_key(&server.players[&2].name)];
        assert!(first_due < second_due);
        let info = server.villagers.towns[&1].info.clone();
        server.release_town(1);
        server.villagers.towns.remove(&1);
        server.villagers.remember_town(info, 12);
        server.bring_out(1, 0);
        id = server.villagers.towns[&1].people[0].creature.unwrap();
        assert_eq!(server.villagers.towns[&1].people[0].calm_players, expiries);
        for quarry in [1, 2] {
            assert!(server.villager_calm_for(id, quarry));
            let before = (server.players[&quarry].skills.xp(attack), server.players[&quarry].skills.xp(defence));
            server.hurt_creature(quarry, id, 1, Some(attack), Blow::Plain);
            server.defence_xp(quarry, id, species);
            assert_eq!((server.players[&quarry].skills.xp(attack), server.players[&quarry].skills.xp(defence)), before);
            assert!(server.creatures.live[&id].mind.attacker().is_none());
        }
        server.tick = first_due;
        assert!(!server.villager_calm_for(id, 1));
        assert!(server.villager_calm_for(id, 2));
        server.tick = second_due;
        assert!(!server.villager_calm_for(id, 2));
        server.scan_towns(&live);
        assert!(server.villagers.towns[&1].people[0].calm_players.is_empty());
    }

    #[test]
    fn people_have_a_reserve_and_settlements_have_an_active_cap() {
        let mut server = fixture("base:farmer", 80);
        player(&mut server);
        // Fill the animal cap; people still fit, and don't reduce animal capacity.
        let rabbit = server.content.creatures.id("base:rabbit").unwrap();
        for i in 0..server.content.creatures.limits().most {
            server.spawn_seeded(rabbit, Vec3::new(8.0, 200.0, 8.0), i as u64).unwrap();
        }
        assert_eq!(server.room_left(), 0);
        let live = server.live();
        server.scan_towns(&live);
        assert_eq!(server.villagers.by_creature.len(), 48);
        assert_eq!(server.room_left(), 0);
        server.players.get_mut(&1).unwrap().position.x += 100.0;
        server.scan_towns(&live);
        assert!(server.villagers.by_creature.len() < 48, "far people still live");
        assert!(server.villagers.saved().is_empty(), "releasing people marked them dead");
    }

    #[test]
    fn square_and_patrol_spots_reject_raised_features_and_the_centre_fallback() {
        let mut server = fixture("base:guard", 1);
        let stone = server.content.blocks.expect("base:stone");
        for x in 6..=10 {
            for z in 6..=10 {
                server.set_block(IVec3::new(x, 200, z), stone);
            }
        }
        let live = server.live();
        for kind in [BuildingKind::Well, BuildingKind::Centrepiece, BuildingKind::Shrine] {
            let b = &mut server.villagers.towns.get_mut(&1).unwrap().info.buildings[0];
            b.kind = kind;
            b.min = IVec2::splat(6);
            b.max = IVec2::splat(10);
            let town = &server.villagers.towns[&1];
            assert_eq!(server.standing_near(town.info.centre, 6).unwrap().y, 201, "raised feature must be standable to expose the bug");
            assert!(server.near_spot(town, town.info.centre, 0, 1).is_none(), "centre fallback stood on the feature");
            for place in [Place::Square, Place::Patrol] {
                for seed in 0..100 {
                    // Patrol's square leg, including both seed parities.
                    let mut trip = Trip { place, slot: 0, day: 0, path: Vec::new(), step: 0, pose: Pose::Idle, face: None,
                        next_spot: None, leg: 1 - (seed % 2) as usize, best: (f32::MAX, 0), retry: 0, pursuit: None };
                    let cell = server.target(1, 0, seed, &mut trip, &live).expect("usable outdoor ground");
                    assert!(town.outdoor_column(cell), "{place:?} selected raised {kind:?} at {cell}");
                    assert_eq!(cell.y, 200, "outdoor feet stay on the surrounding ground");
                }
            }
        }
    }

    #[test]
    fn well_spots_stay_outside_and_rooms_avoid_furnishings() {
        let mut server = fixture("base:farmer", 1);
        server.villagers.towns.get_mut(&1).unwrap().info.buildings[0].kind = BuildingKind::Well;
        for x in 4..=12 {
            for z in 4..=12 {
                server.set_block(IVec3::new(x, 200, z), server.content.blocks.expect("base:stone"));
            }
        }
        for seed in 0..100 {
            let town = &server.villagers.towns[&1];
            let cell = server.about(0, town, seed).unwrap();
            assert!(cell.x < 4 || cell.x > 12 || cell.z < 4 || cell.z > 12);
            assert!((199..=200).contains(&cell.y));
        }
        // A furnished room still has a usable floor. The old search could select the
        // block above the table; an empty-room check would not catch that.
        for x in 4..=12 {
            for z in 4..=12 {
                if (x, z) != (8, 8) { server.set_block(IVec3::new(x, 200, z), BlockId::AIR); }
            }
        }
        for seed in 0..100 {
            let cell = server.spot_inside(0, &server.villagers.towns[&1], seed).expect("room floor");
            assert_eq!(cell.y, 200, "stood on furniture");
            assert_ne!((cell.x, cell.z), (8, 8));
            assert!(server.villagers.towns[&1].info.buildings[0].room.contains(&IVec2::new(cell.x, cell.z)), "stood on graded exterior ground");
        }
        // The whole graded envelope and exterior door are standable, but neither is
        // a bedroom when all the actual room cells are blocked by furnishings.
        let room = server.villagers.towns[&1].info.buildings[0].room.clone();
        for p in room {
            server.set_block(IVec3::new(p.x, 200, p.y), server.content.blocks.expect("base:stone"));
        }
        let town = &server.villagers.towns[&1];
        assert!(server.stands(town.info.buildings[0].door.unwrap()));
        assert!(server.spot_inside(0, town, 1).is_none(), "fell back to an exterior doorstep");
        server.villagers.towns.get_mut(&1).unwrap().info.buildings[0].room.clear();
        assert!(server.spot_inside(0, &server.villagers.towns[&1], 1).is_none());
    }
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0).then_with(|| (other.1.x, other.1.y, other.1.z).cmp(&(self.1.x, self.1.y, self.1.z)))
    }
}
