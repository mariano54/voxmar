//! The people of settlements (`people` in settlements.ron, planned by worldgen's
//! `settlements::people`), alive near players.
//!
//! A settlement comes to life when a player comes within `ACTIVE` blocks of it: each of
//! its residents appears where their day has them now (asleep at home at night, at work
//! by day), as a creature of their species (`creatures_villagers.ron`, kind `Villager`)
//! with the looks their seed gives, the same each visit. When every player is `RELEASE`
//! blocks away they go again, and come back where their day has them by then.
//!
//! Each person's routine sends them from place to place through the day (`place_at`);
//! the server finds their way there through the settlement's lanes, doors and yards
//! (`way`, an A* over the cells a body can stand in) and hands the brain the next turn of
//! it as an errand, which the `Routine` behaviour walks. At a place where people stay a
//! while (the square, a field, the yard) they move to another spot of it now and then.
//!
//! Someone killed is gone until a newcomer takes their place `newcomer` days later, with
//! looks of their own. A villager struck by a player calls the guards near: they fight
//! the one who did it (`Defend`).

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::brain::{Errand, Pose};
use voxmar_engine::creature::{CreatureId, mix, unit_from};
use voxmar_engine::genes;
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3, Vec3Swizzles};
use voxmar_protocol::{CreatureGone, TICKS_PER_DAY, TICKS_PER_SECOND};
use voxmar_worldgen::people::{Place, is, place_at};
use voxmar_worldgen::{BuildingKind, SettlementInfo, SettlementLevers};

use crate::Server;

/// A settlement comes to life within this many blocks of a player, and goes again when
/// every player is further than `RELEASE`.
const ACTIVE: f32 = 112.0;
const RELEASE: f32 = 160.0;
/// Settlements no player has been within this many blocks of are forgotten (and with
/// them who died there).
const FORGET: f32 = 2000.0;
/// Ticks between looks for settlements coming to life or going.
const SCAN_EVERY: u64 = 2 * TICKS_PER_SECOND as u64;
/// Ticks between each person's look at their day.
const ERRAND_EVERY: u64 = 10;
/// Ways found per tick at most, so a whole village waking at once spreads over ticks.
const WAYS_PER_TICK: usize = 6;
/// Cells a way may search before giving up.
const SEARCH: usize = 8000;
/// Blocks within which guards hear a villager struck.
const HELP: f32 = 40.0;
/// Seconds without getting nearer the next turn before finding the way again.
const STUCK: f32 = 6.0;

/// Every settlement near players, and who of their people is alive as which creature.
#[derive(Default)]
pub struct Villagers {
    towns: BTreeMap<u64, Town>,
    by_creature: FxHashMap<u32, (u64, usize)>,
    /// Ways found this tick.
    ways: usize,
}

impl Villagers {
    /// Whether creature `id` is someone's, kept here rather than by the land.
    pub(crate) fn manages(&self, id: u32) -> bool {
        self.by_creature.contains_key(&id)
    }

    /// Where creature `id`'s routine sends it now, if it is someone's.
    pub(crate) fn errand(&self, id: u32) -> Option<Errand> {
        let &(town, i) = self.by_creature.get(&id)?;
        let trip = self.towns.get(&town)?.people[i].trip.as_ref()?;
        let to = *trip.path.get(trip.step)?;
        let last = trip.step + 1 >= trip.path.len();
        Some(Errand { to: to.as_vec3() + Vec3::new(0.5, 0.0, 0.5), last, pose: trip.pose, face: trip.face })
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
}

#[derive(Default)]
struct Person {
    species: Option<CreatureId>,
    creature: Option<u32>,
    /// Newcomers so far in this place, each with their own looks.
    generation: u64,
    /// The day (in days since the world began) a newcomer arrives after a death.
    away_until: f64,
    trip: Option<Trip>,
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
}

impl Town {
    fn new(info: SettlementInfo) -> Self {
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
        let (min, max) = (min.min(centre) - 12, max.max(centre) + 12);
        let people = info.people.iter().map(|_| Person::default()).collect();
        Self { levers: SettlementLevers(info.levers), info, min, max, people, alive: false }
    }

    /// Blocks from `at` to the nearest of its ground.
    fn distance(&self, at: Vec3) -> f32 {
        let p = Vec2::new(at.x, at.z);
        let clamped = p.clamp(self.min.as_vec2(), self.max.as_vec2());
        p.distance(clamped)
    }

    /// Whether it has anywhere for `place`.
    fn has(&self, place: Place) -> bool {
        let b = &self.info.buildings;
        match place {
            Place::Inn => b.iter().any(|b| b.kind == BuildingKind::Inn),
            Place::Shrine => b.iter().any(|b| is(b, "shrine")),
            Place::Well => b.iter().any(|b| b.kind == BuildingKind::Well),
            Place::Gate | Place::Patrol => !self.info.gates.is_empty(),
            Place::Neighbour => b.iter().filter(|b| b.kind == BuildingKind::House && b.door.is_some()).count() > 1,
            Place::Wander => self.info.lanes.iter().any(|l| !l.is_empty()),
            _ => true,
        }
    }
}

/// Days since the world began, from the server's clock.
fn days(day: f64) -> f64 {
    day / TICKS_PER_DAY as f64
}

impl Server {
    /// Brings settlements near players to life and lets them go, notes who died, and
    /// moves everyone along their day.
    pub(crate) fn tick_villagers(&mut self) {
        self.villagers.ways = 0;
        if self.tick.is_multiple_of(SCAN_EVERY) {
            self.scan_towns();
        }
        let ids: Vec<(u32, u64, usize)> = self.villagers.by_creature.iter().map(|(&id, &(t, i))| (id, t, i)).collect();
        for (id, town, i) in ids {
            if (self.tick + id as u64).is_multiple_of(ERRAND_EVERY) {
                self.follow_day(id, town, i);
            }
        }
        self.call_guards();
    }

    fn scan_towns(&mut self) {
        let players: Vec<Vec3> = self.players.values().filter(|p| p.joined).map(|p| p.position).collect();
        // Settlements coming into reach.
        for at in &players {
            for info in self.overworld.settlements_near(IVec2::new(at.x as i32, at.z as i32), ACTIVE as i32 + 256) {
                if !self.villagers.towns.contains_key(&info.id) {
                    let town = Town::new(info);
                    self.villagers.towns.insert(town.info.id, town);
                }
            }
        }
        let today = days(self.day);
        let ids: Vec<u64> = self.villagers.towns.keys().copied().collect();
        for id in ids {
            let town = &self.villagers.towns[&id];
            let nearest = players.iter().map(|&p| town.distance(p)).fold(f32::MAX, f32::min);
            if nearest > FORGET {
                self.release_town(id);
                self.villagers.towns.remove(&id);
                continue;
            }
            let alive = if town.alive { nearest < RELEASE } else { nearest < ACTIVE };
            if !alive {
                if town.alive {
                    self.release_town(id);
                }
                continue;
            }
            // Who died since the last look.
            let town = self.villagers.towns.get_mut(&id).expect("listed");
            town.alive = true;
            let newcomer = self.overworld.settlement_people().newcomer as f64;
            for (i, person) in town.people.iter_mut().enumerate() {
                if let Some(c) = person.creature
                    && !self.creatures.live.contains_key(&c)
                {
                    self.villagers.by_creature.remove(&c);
                    person.creature = None;
                    person.trip = None;
                    person.generation += 1;
                    person.away_until = today + newcomer;
                    tracing::debug!(town = %town.info.name.0, person = i, "a villager died");
                }
            }
            for i in 0..self.villagers.towns[&id].people.len() {
                self.bring_out(id, i);
            }
        }
    }

    /// Everyone of settlement `id` goes.
    fn release_town(&mut self, id: u64) {
        let Some(town) = self.villagers.towns.get_mut(&id) else { return };
        town.alive = false;
        for person in &mut town.people {
            person.trip = None;
            if let Some(c) = person.creature.take() {
                self.villagers.by_creature.remove(&c);
                if self.creatures.live.remove(&c).is_some() {
                    self.creatures.gone.push(CreatureGone { id: c, died: false });
                }
            }
        }
    }

    /// Puts person `i` of settlement `town` where their day has them, if they are not about
    /// and not gone.
    fn bring_out(&mut self, town: u64, i: usize) {
        let today = days(self.day);
        let t = &self.villagers.towns[&town];
        let person = &t.people[i];
        if person.creature.is_some() || person.away_until > today {
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
        let seed = mix(resident.seed, person.generation);
        let home = t.info.buildings[resident.home].door.unwrap_or(t.info.centre).as_vec3();
        let Some(spot) = self.spot_for(town, i, seed) else { return };
        if !self.world.contains(ColumnPos::of_block(spot.0)) {
            return;
        }
        let Some(id) = self.spawn_seeded(species, spot.0.as_vec3() + Vec3::new(0.5, 0.0, 0.5), seed) else { return };
        // Looks from the people of the land about, the same each visit.
        let genome = self.wild_genome(species, home, seed);
        let def = &self.content.creatures.get(species).def;
        let traits = genes::express(&def.genes, &genome);
        let creature = self.creatures.live.get_mut(&id).expect("just put");
        creature.home = home;
        creature.scale = traits.scale * (0.94 + 0.12 * unit_from(mix(seed, 0x5ca1e)));
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
        let mut trip = Trip { place, slot, day, path: Vec::new(), step: 0, pose: Pose::Idle, face: None, next_spot: None, leg: 0, best: (f32::MAX, self.tick), retry: 0 };
        let target = self.target(town, i, seed, &mut trip)?;
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
        let (place, slot) = place_at(self.overworld.settlement_people(), resident.routine, seed, &t.levers, day, now.fract() as f32, |p| t.has(p));
        (place, slot, day)
    }

    /// Moves person `i` along their day: a new place when the day says, another spot of a
    /// place they stay at, the next turn of the way, and a new way when stuck.
    fn follow_day(&mut self, id: u32, town: u64, i: usize) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let at = creature.body.position;
        let seed = creature.seed;
        let (place, slot, day) = self.place_now(town, i, seed);
        let tick = self.tick;
        let fresh = {
            let trip = self.villagers.towns[&town].people[i].trip.as_ref();
            match trip {
                None => true,
                Some(t) => t.place != place || t.slot != slot || t.day != day || t.next_spot.is_some_and(|n| tick >= n) || (t.path.len() <= 1 && t.retry != 0 && tick >= t.retry),
            }
        };
        if fresh {
            if self.villagers.ways >= WAYS_PER_TICK {
                return;
            }
            self.villagers.ways += 1;
            let leg = self.villagers.towns[&town].people[i].trip.as_ref().filter(|t| t.place == place).map_or(0, |t| t.leg + 1);
            let mut trip = Trip { place, slot, day, path: Vec::new(), step: 0, pose: Pose::Idle, face: None, next_spot: None, leg, best: (f32::MAX, tick), retry: 0 };
            let Some(target) = self.target(town, i, mix(seed, tick / 200), &mut trip) else { return };
            let start = at.floor().as_ivec3();
            match self.way(start, target, town) {
                Some(path) => trip.path = path,
                None => {
                    // No way found: stay put, and look again in a while.
                    trip.path = vec![start];
                    trip.pose = Pose::Idle;
                    trip.retry = tick + 10 * TICKS_PER_SECOND as u64;
                }
            }
            self.villagers.towns.get_mut(&town).expect("listed").people[i].trip = Some(trip);
            return;
        }
        let trip = self.villagers.towns.get_mut(&town).expect("listed").people[i].trip.as_mut().expect("checked");
        let Some(&next) = trip.path.get(trip.step) else { return };
        let to = next.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        let d = to.with_y(0.0).distance(at.with_y(0.0));
        let last = trip.step + 1 >= trip.path.len();
        if d < 0.9 && !last {
            trip.step += 1;
            trip.best = (f32::MAX, tick);
        } else if d < 1.0 && last && trip.place == Place::Patrol && trip.next_spot.is_none() {
            // A patrol stops a moment at each stop, then walks on.
            trip.next_spot = Some(tick + 3 * TICKS_PER_SECOND as u64);
        } else if d + 0.2 < trip.best.0 {
            trip.best = (d, tick);
        } else if d > 1.0 && (tick - trip.best.1) as f32 > STUCK * TICKS_PER_SECOND as f32 {
            // Stuck: find the way again from here.
            trip.next_spot = Some(tick);
        }
    }

    /// Picks the cell person `i` goes to for `trip`'s place, and how they are once there.
    fn target(&self, town: u64, i: usize, seed: u64, trip: &mut Trip) -> Option<IVec3> {
        let t = &self.villagers.towns[&town];
        let resident = &t.info.people[i];
        let buildings = &t.info.buildings;
        let home = &buildings[resident.home];
        let centre = t.info.centre;
        let linger = self.overworld.settlement_people().linger;
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
                self.inside(resident.home, t, seed)
            }
            Place::Yard => {
                stay(trip, 2);
                let door = home.door.unwrap_or(centre);
                self.near_spot(door, 3, seed)
            }
            Place::Work => match resident.work {
                Some(w) => {
                    let b = &buildings[w];
                    if matches!(b.kind, BuildingKind::House | BuildingKind::Trader | BuildingKind::Inn) {
                        stay(trip, 3);
                        trip.face = Some(((b.min + b.max).as_vec2() / 2.0).extend(b.floor as f32 + 1.5).xzy());
                        self.inside(w, t, seed)
                    } else {
                        stay(trip, 4);
                        self.about(w, t, seed)
                    }
                }
                None => {
                    stay(trip, 2);
                    self.near_spot(home.door.unwrap_or(centre), 3, seed)
                }
            },
            Place::Square => {
                stay(trip, 5);
                trip.face = Some(centre.as_vec3() + Vec3::new(0.5, 1.5, 0.5));
                self.near_spot(centre, 6, seed)
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
                self.inside(inn, t, seed)
            }
            Place::Shrine => {
                stay(trip, 8);
                let s = buildings.iter().position(|b| is(b, "shrine"))?;
                if buildings[s].kind == BuildingKind::House { self.inside(s, t, seed) } else { self.about(s, t, seed) }
            }
            Place::Neighbour => {
                stay(trip, 9);
                let houses: Vec<usize> = (0..buildings.len()).filter(|&b| b != resident.home && buildings[b].kind == BuildingKind::House && buildings[b].door.is_some()).collect();
                let b = *houses.get((mix(seed, 10) % houses.len().max(1) as u64) as usize)?;
                let door = buildings[b].door?;
                trip.face = Some(door.as_vec3() - buildings[b].facing.step().extend(0).xzy().as_vec3() * 2.0 + Vec3::new(0.5, 1.5, 0.5));
                self.near_spot(door, 1, seed)
            }
            Place::Gate => {
                stay(trip, 11);
                let gates = &t.info.gates;
                let g = gates[(mix(seed, 12) % gates.len() as u64) as usize];
                let out = (g - IVec2::new(centre.x, centre.z)).as_vec2().normalize_or_zero();
                trip.face = Some(Vec3::new(g.x as f32 + out.x * 8.0, centre.y as f32 + 1.5, g.y as f32 + out.y * 8.0));
                self.near_spot(IVec3::new(g.x, centre.y, g.y), 2, seed)
            }
            Place::Patrol => {
                // Gate to gate by way of the square, a leg at a time.
                let gates = &t.info.gates;
                let stops = gates.len() * 2;
                let leg = (trip.leg + (seed % stops as u64) as usize) % stops;
                if leg % 2 == 1 {
                    self.near_spot(centre, 4, seed)
                } else {
                    let g = gates[leg / 2];
                    self.near_spot(IVec3::new(g.x, centre.y, g.y), 2, seed)
                }
            }
            Place::Wander => {
                stay(trip, 13);
                let lanes: Vec<&Vec<Vec2>> = t.info.lanes.iter().filter(|l| !l.is_empty()).collect();
                let lane = lanes[(mix(seed, 14) % lanes.len() as u64) as usize];
                let p = lane[(mix(seed, 15) % lane.len() as u64) as usize];
                self.near_spot(IVec3::new(p.x as i32, centre.y, p.y as i32), 1, seed)
            }
        };
        spot
    }

    /// A cell a body can stand in inside building `b`, by `seed`.
    fn inside(&self, b: usize, town: &Town, seed: u64) -> Option<IVec3> {
        let b = &town.info.buildings[b];
        let cells: Vec<IVec3> = (b.min.x + 1..b.max.x)
            .flat_map(|x| (b.min.y + 1..b.max.y).map(move |z| (x, z)))
            .filter_map(|(x, z)| (b.floor..=b.floor + 2).map(|y| IVec3::new(x, y, z)).find(|&c| self.stands(c)))
            .collect();
        if cells.is_empty() {
            return b.door.filter(|&d| self.stands(d));
        }
        Some(cells[(mix(seed, 0x1a5) % cells.len() as u64) as usize])
    }

    /// A cell a body can stand in on or round building `b` (a field, a pen, a well).
    fn about(&self, b: usize, town: &Town, seed: u64) -> Option<IVec3> {
        let b = &town.info.buildings[b];
        let cells: Vec<IVec3> = (b.min.x - 1..=b.max.x + 1)
            .flat_map(|x| (b.min.y - 1..=b.max.y + 1).map(move |z| (x, z)))
            .filter_map(|(x, z)| self.standing_near(IVec3::new(x, b.floor + 1, z), 3))
            .collect();
        if cells.is_empty() {
            return None;
        }
        Some(cells[(mix(seed, 0xab0) % cells.len() as u64) as usize])
    }

    /// A cell a body can stand in within `r` blocks of `at`, by `seed`.
    fn near_spot(&self, at: IVec3, r: i32, seed: u64) -> Option<IVec3> {
        for attempt in 0..12u64 {
            let h = mix(seed, 0x5907 + attempt);
            let dx = (h % (2 * r as u64 + 1)) as i32 - r;
            let dz = ((h >> 20) % (2 * r as u64 + 1)) as i32 - r;
            if let Some(c) = self.standing_near(at + IVec3::new(dx, 0, dz), 4) {
                return Some(c);
            }
        }
        self.standing_near(at, 6)
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
    /// or down two.
    fn way(&self, from: IVec3, to: IVec3, town: u64) -> Option<Vec<IVec3>> {
        let t = &self.villagers.towns[&town];
        let flat = IVec2::new(from.x, from.z);
        let (min, max) = (t.min.min(flat) - 8, t.max.max(flat) + 8);
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
            if searched > SEARCH {
                return None;
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
        let end = found?;
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
        Some(turns)
    }

    fn passable(&self, cell: IVec3) -> bool {
        self.block(cell).is_some_and(|b| !self.content.blocks.info(b).solid)
    }

    /// Guards near a villager a player struck fight that player.
    fn call_guards(&mut self) {
        if !self.tick.is_multiple_of(5) {
            return;
        }
        let mut calls: Vec<(u64, Vec3, u32)> = Vec::new();
        for (&id, &(town, i)) in &self.villagers.by_creature {
            let Some(c) = self.creatures.live.get(&id) else { continue };
            if self.villagers.towns[&town].info.people[i].guard {
                continue;
            }
            if let Some(player) = c.mind.attacker() {
                calls.push((town, c.body.position, player));
            }
        }
        for (town, at, player) in calls {
            let guards: Vec<u32> = self.villagers.towns[&town]
                .people
                .iter()
                .zip(&self.villagers.towns[&town].info.people)
                .filter(|(_, r)| r.guard)
                .filter_map(|(p, _)| p.creature)
                .collect();
            for g in guards {
                let Some(guard) = self.creatures.live.get_mut(&g) else { continue };
                if guard.body.position.distance(at) > HELP || guard.mind.attacker() == Some(player) {
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

impl Ord for Node {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0).then_with(|| (other.1.x, other.1.y, other.1.z).cmp(&(self.1.x, self.1.y, self.1.z)))
    }
}
