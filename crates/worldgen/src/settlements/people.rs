//! Who lives in a settlement and how their days go (`people` in settlements.ron).
//!
//! Residents are drawn when the settlement is planned, from the seed alone: each home
//! holds as many as its house kind says (fuller where it is crowded), each working
//! building takes its workers from the homes nearest, a village keeps guards by its
//! wealth and order, and everyone else is drawn from the folk list, leaning on the
//! levers (more elders where it is old, more children where it is crowded). The server
//! brings them to life near players (`crates/server/src/villagers.rs`).
//!
//! A routine is a day cut into slots, each with places to be and how much each is
//! wanted. A person's day shifts from the routine's by their own jitter (early risers,
//! late ones), and each slot's place is drawn again every day, so no two days, and no two
//! people, go quite alike.

use std::collections::BTreeMap;

use serde::Deserialize;
use voxmar_engine::glam::IVec2;

use super::Building;
use super::def::{HouseDef, Lean, Levers};
use super::house::Kind;
use crate::content::{ContentError, qualify};
use crate::rng::{Rng, mix};

/// Where a slot of a routine sends someone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Place {
    /// In bed at home, resting.
    Sleep,
    /// About the house, indoors.
    Home,
    /// Outside their own door.
    Yard,
    /// Where they work; about their yard when they have no work.
    Work,
    /// The square or green.
    Square,
    Well,
    Inn,
    /// The chapel, or a wayside shrine.
    Shrine,
    /// At another house's door.
    Neighbour,
    /// At the end of a lane leading out, watching the road.
    Gate,
    /// Walking from gate to gate by way of the square.
    Patrol,
    /// Strolling the lanes.
    Wander,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GoFile {
    place: Place,
    #[serde(default = "one")]
    weight: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotFile {
    until: f32,
    go: Vec<GoFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoutineFile {
    name: String,
    slots: Vec<SlotFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JobFile {
    at: String,
    species: String,
    count: (u32, u32),
    routine: String,
    #[serde(default = "one")]
    chance: f32,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FolkFile {
    species: String,
    weight: f32,
    routine: String,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
    #[serde(default)]
    work: Option<String>,
    #[serde(default = "yes")]
    alone: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuardFile {
    size: String,
    species: String,
    count: (u32, u32),
    routines: Vec<String>,
    #[serde(default)]
    lean: BTreeMap<String, f32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PeopleFile {
    #[serde(default)]
    jobs: Vec<JobFile>,
    #[serde(default)]
    folk: Vec<FolkFile>,
    #[serde(default)]
    guards: Vec<GuardFile>,
    #[serde(default)]
    routines: Vec<RoutineFile>,
    jitter: Option<f32>,
    linger: Option<(f32, f32)>,
    newcomer: Option<f32>,
    newcomers_per_day: Option<f32>,
    shrine: Option<String>,
    #[serde(default)]
    live: LiveFile,
    #[serde(default)]
    react: ReactFile,
}

/// `people.react` as written: each field it gives replaces the one before.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReactFile {
    greet: Option<f32>,
    greet_for: Option<f32>,
    greet_every: Option<f32>,
    fear: Option<f32>,
    afraid: Option<f32>,
    rain: Option<f32>,
    shelter: Option<Vec<Place>>,
    outdoor: Option<Vec<String>>,
}

/// `people.live` as written: each number it gives replaces the one before.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LiveFile {
    most: Option<usize>,
    per_settlement: Option<usize>,
    nearby: Option<f32>,
    active: Option<f32>,
    release: Option<f32>,
    forget: Option<f32>,
    margin: Option<i32>,
    scan_pad: Option<i32>,
    scan_every: Option<f32>,
    errand_every: Option<f32>,
    search: Option<u32>,
    search_per_tick: Option<u32>,
    failed_for: Option<f32>,
    stuck: Option<f32>,
    no_way_retry: Option<f32>,
    patrol_pause: Option<f32>,
    repath: Option<f32>,
    help: Option<f32>,
    call_every: Option<f32>,
    give_up: Option<f32>,
    calm: Option<f32>,
    handoff: Option<f32>,
    scale: Option<(f32, f32)>,
    spots: Option<SpotsDef>,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

/// One place a slot may send someone, by weight.
#[derive(Clone, Debug, PartialEq)]
pub struct GoDef {
    pub place: Place,
    pub weight: f32,
    pub lean: Lean,
}

/// A slot of a day: until `until` (the share of the day from sunrise, 1 the next
/// sunrise), one of `go`.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotDef {
    pub until: f32,
    pub go: Vec<GoDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutineDef {
    pub name: String,
    pub slots: Vec<SlotDef>,
}

/// Workers a building takes: every building whose work (or kind) is `at`.
#[derive(Clone, Debug, PartialEq)]
pub struct JobDef {
    pub at: String,
    pub species: String,
    pub count: (u32, u32),
    pub routine: String,
    pub chance: f32,
    pub lean: Lean,
}

/// Someone who lives at home, by weight; with `work`, they work at a building of that
/// work or kind, and are drawn only where there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct FolkDef {
    pub species: String,
    pub weight: f32,
    pub routine: String,
    pub lean: Lean,
    pub work: Option<String>,
    /// Whether they may be the first in a home (children may not).
    pub alone: bool,
}

/// A size's guards: how many, and the routines they take in turn (a day watch, a night
/// watch).
#[derive(Clone, Debug, PartialEq)]
pub struct GuardDef {
    pub size: String,
    pub species: String,
    pub count: (u32, u32),
    pub routines: Vec<String>,
    pub lean: Lean,
}

/// Every package's `people` together.
#[derive(Clone, Debug, PartialEq)]
pub struct PeopleDef {
    pub jobs: Vec<JobDef>,
    pub folk: Vec<FolkDef>,
    pub guards: Vec<GuardDef>,
    pub routines: Vec<RoutineDef>,
    /// How far, as a share of the day, each person's day may run early or late.
    pub jitter: f32,
    /// Seconds one stands at a spot before moving to another spot of the same place.
    pub linger: (f32, f32),
    /// Days before a newcomer takes the place of someone who died.
    pub newcomer: f32,
    /// Newcomers a settlement takes in a day at most: several dead come back one by one.
    pub newcomers_per_day: f32,
    /// The work or kind of building `Shrine` sends people to.
    pub shrine: String,
    pub live: LiveDef,
    pub react: ReactDef,
    setters: BTreeMap<String, String>,
    pub(crate) notes: Vec<String>,
}

impl Default for PeopleDef {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            folk: Vec::new(),
            guards: Vec::new(),
            routines: Vec::new(),
            jitter: 0.03,
            linger: (12.0, 50.0),
            newcomer: 5.0,
            newcomers_per_day: 1.0,
            shrine: "shrine".into(),
            live: LiveDef::default(),
            react: ReactDef::default(),
            setters: BTreeMap::new(),
            notes: Vec::new(),
        }
    }
}

/// How settlements' people react to what is about them (`people.react`;
/// crates/server/src/villagers.rs). Distances in blocks, times in seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactDef {
    /// A player within `greet` (and as far up or down) is greeted: they stop, turn to
    /// them and hum a greeting for `greet_for`, each greeting once in `greet_every`.
    pub greet: f32,
    pub greet_for: f32,
    pub greet_every: f32,
    /// A monster or a hunter within `fear` sends them home crying out, for `afraid`;
    /// guards stand their ground.
    pub fear: f32,
    pub afraid: f32,
    /// Rain of at least `rain` (its precipitation, 0 to 1) sends them home from the
    /// `shelter` places, and from work at the `outdoor` works (a field, a pen).
    pub rain: f32,
    pub shelter: Vec<Place>,
    pub outdoor: Vec<String>,
}

impl Default for ReactDef {
    fn default() -> Self {
        Self { greet: 4.0, greet_for: 3.0, greet_every: 90.0, fear: 12.0, afraid: 30.0, rain: 0.2, shelter: Vec::new(), outdoor: Vec::new() }
    }
}

/// How the server keeps settlements' people alive near players (`people.live`;
/// crates/server/src/villagers.rs). Distances in blocks, times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveDef {
    /// People have their own reserve, independent of wild and kept animals.
    pub most: usize,
    /// At most this many residents of a settlement live, within `nearby` of a player.
    pub per_settlement: usize,
    pub nearby: f32,
    /// A settlement comes to life within `active` of a player, and goes when every player
    /// is past `release`; past `forget` the server lets go of it altogether (who died
    /// there is kept in the world's save).
    pub active: f32,
    pub release: f32,
    pub forget: f32,
    /// Blocks round its buildings where ways are searched.
    pub margin: i32,
    /// Blocks past `active` looked through for settlements coming into reach.
    pub scan_pad: i32,
    /// Between looks for settlements coming to life or going.
    pub scan_every: f32,
    /// Between each person's looks at their day.
    pub errand_every: f32,
    /// Cells one way may search before it gives up, and all ways in a tick together.
    pub search: u32,
    pub search_per_tick: u32,
    /// How long a way not found from one cell to another is not searched for again.
    pub failed_for: f32,
    /// Without getting nearer the next turn, finding the way again.
    pub stuck: f32,
    /// With no way to where their day sends them, looking again.
    pub no_way_retry: f32,
    /// A patrol's stop at each gate and the square.
    pub patrol_pause: f32,
    /// Between ways found to a guard's quarry.
    pub repath: f32,
    /// Guards within `help` of a villager struck come, looked for every `call_every`.
    pub help: f32,
    pub call_every: f32,
    /// A guard that comes no nearer its quarry for `give_up` breaks off and goes home
    /// for `calm`. Recovery waits another `give_up` seconds without a blow.
    pub give_up: f32,
    pub calm: f32,
    /// Within this of its quarry a guard goes straight at it rather than by the way.
    pub handoff: f32,
    /// Each person's size, as a share of their genes', from the first to the second.
    pub scale: (f32, f32),
    pub spots: SpotsDef,
}

impl Default for LiveDef {
    fn default() -> Self {
        Self {
            most: 512,
            per_settlement: 48,
            nearby: 96.0,
            active: 112.0,
            release: 160.0,
            forget: 2000.0,
            margin: 12,
            scan_pad: 256,
            scan_every: 2.0,
            errand_every: 0.5,
            search: 8000,
            search_per_tick: 20000,
            failed_for: 10.0,
            stuck: 6.0,
            no_way_retry: 10.0,
            patrol_pause: 3.0,
            repath: 2.0,
            help: 40.0,
            call_every: 0.25,
            give_up: 8.0,
            calm: 60.0,
            handoff: 4.0,
            scale: (0.94, 1.06),
            spots: SpotsDef::default(),
        }
    }
}

/// How far from a place's middle people stand there, in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpotsDef {
    /// Outside their own door (and about their yard when they have no work).
    pub yard: i32,
    pub square: i32,
    pub gate: i32,
    /// A patrol's stops: the square and each gate.
    pub patrol_square: i32,
    pub patrol_gate: i32,
    /// Along a lane.
    pub wander: i32,
    /// At another house's door.
    pub neighbour: i32,
}

impl Default for SpotsDef {
    fn default() -> Self {
        Self { yard: 3, square: 6, gate: 2, patrol_square: 4, patrol_gate: 2, wander: 1, neighbour: 1 }
    }
}

impl PeopleDef {
    /// Adds a package's `people`: lists add, a routine replaces one of the same name, and
    /// the numbers it gives replace earlier ones.
    pub(super) fn extend(&mut self, at: &str, package: &str, file: PeopleFile) -> Result<(), ContentError> {
        let lean = |map: &BTreeMap<String, f32>| Lean::parse(at, map);
        for j in file.jobs {
            self.jobs.push(JobDef { lean: lean(&j.lean)?, at: j.at, species: qualify(package, &j.species), count: j.count, routine: j.routine, chance: j.chance });
        }
        for f in file.folk {
            self.folk.push(FolkDef { lean: lean(&f.lean)?, species: qualify(package, &f.species), weight: f.weight, routine: f.routine, work: f.work, alone: f.alone });
        }
        for g in file.guards {
            self.guards.push(GuardDef { lean: lean(&g.lean)?, size: g.size, species: qualify(package, &g.species), count: g.count, routines: g.routines });
        }
        for r in file.routines {
            self.note_setter(at, package, &format!("routines.{}", r.name));
            let slots = r
                .slots
                .into_iter()
                .map(|s| {
                    let go = s.go.into_iter().map(|g| Ok(GoDef { lean: lean(&g.lean)?, place: g.place, weight: g.weight })).collect::<Result<_, ContentError>>()?;
                    Ok(SlotDef { until: s.until, go })
                })
                .collect::<Result<_, ContentError>>()?;
            let routine = RoutineDef { name: r.name, slots };
            match self.routines.iter_mut().find(|old| old.name == routine.name) {
                Some(old) => *old = routine,
                None => self.routines.push(routine),
            }
        }
        if let Some(jitter) = file.jitter {
            self.note_setter(at, package, "jitter");
            self.jitter = jitter;
        }
        if let Some(linger) = file.linger {
            self.note_setter(at, package, "linger");
            self.linger = linger;
        }
        if let Some(newcomer) = file.newcomer {
            self.note_setter(at, package, "newcomer");
            self.newcomer = newcomer;
        }
        if let Some(n) = file.newcomers_per_day {
            self.note_setter(at, package, "newcomers_per_day");
            self.newcomers_per_day = n;
        }
        if let Some(shrine) = file.shrine {
            self.note_setter(at, package, "shrine");
            self.shrine = shrine;
        }
        let l = file.live;
        macro_rules! take {
            ($($field:ident),*) => {
                $(if let Some(v) = l.$field {
                    self.note_setter(at, package, concat!("live.", stringify!($field)));
                    self.live.$field = v;
                })*
            };
        }
        take!(most, per_settlement, nearby, active, release, forget, margin, scan_pad, scan_every, errand_every, search, search_per_tick, failed_for, stuck, no_way_retry, patrol_pause, repath, help, call_every, give_up, calm, handoff, scale, spots);
        let r = file.react;
        macro_rules! react {
            ($($field:ident),*) => {
                $(if let Some(v) = r.$field {
                    self.note_setter(at, package, concat!("react.", stringify!($field)));
                    self.react.$field = v;
                })*
            };
        }
        react!(greet, greet_for, greet_every, fear, afraid, rain, shelter, outdoor);
        Ok(())
    }

    fn note_setter(&mut self, at: &str, package: &str, field: &str) {
        if let Some(earlier) = self.setters.insert(field.into(), package.into())
            && earlier != package
        {
            self.notes.push(format!("{at} replaces the settlements.ron {earlier} set (people.{field})"));
        }
    }

    /// Checks that routines cover the day in order and that everything names a routine.
    pub(super) fn check(&self, sizes: &[String]) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("settlements.ron: people: {what}")));
        for r in &self.routines {
            let mut last = 0.0;
            for s in &r.slots {
                if s.until <= last || s.go.is_empty() || s.go.iter().any(|g| g.weight < 0.0) {
                    return fail(format!("routine `{}` needs slots in order, each with somewhere to go", r.name));
                }
                last = s.until;
            }
            if (last - 1.0).abs() > 1e-4 {
                return fail(format!("routine `{}` must end at 1.0, the next sunrise", r.name));
            }
        }
        let known = |name: &str| self.routines.iter().any(|r| r.name == name);
        let named = self.jobs.iter().map(|j| &j.routine).chain(self.folk.iter().map(|f| &f.routine)).chain(self.guards.iter().flat_map(|g| &g.routines));
        for name in named {
            if !known(name) {
                return fail(format!("no routine is called `{name}`"));
            }
        }
        for g in &self.guards {
            if g.routines.is_empty() || !sizes.contains(&g.size) || g.count.0 > g.count.1 {
                return fail(format!("guards of `{}` need a size there is, routines and a count low to high", g.size));
            }
        }
        if self.jobs.iter().any(|j| j.count.0 > j.count.1) || !(0.0..0.2).contains(&self.jitter) || self.linger.0 < 0.0 || self.linger.0 > self.linger.1 {
            return fail("counts low to high, a jitter under 0.2 and linger from nothing or more, low to high".into());
        }
        if self.jobs.iter().any(|j| !(0.0..=1.0).contains(&j.chance)) || self.folk.iter().any(|f| f.weight < 0.0) || self.routines.iter().any(|r| r.slots.iter().any(|s| s.go.iter().any(|g| g.weight < 0.0))) {
            return fail("job chances within 0 to 1 and weights of nothing or more".into());
        }
        // A newcomer sooner than a quarter day after a death makes a village a well of
        // experience to empty again and again.
        if self.newcomer < 0.25 || self.newcomers_per_day <= 0.0 {
            return fail("newcomers at least a quarter day after a death, and some each day".into());
        }
        let l = &self.live;
        let s = &l.spots;
        let times = [l.scan_every, l.errand_every, l.failed_for, l.stuck, l.no_way_retry, l.patrol_pause, l.repath, l.call_every, l.give_up, l.calm];
        if l.most == 0 || l.per_settlement == 0 || l.per_settlement > 48 || !(0.0 < l.nearby && l.nearby <= 96.0)
            || !(0.0 < l.active && l.active < l.release && l.release < l.forget)
            || l.margin < 0
            || l.scan_pad < 0
            || l.search == 0
            || l.search_per_tick < l.search
            || times.iter().any(|&t| t <= 0.0)
            || l.help < 0.0
            || l.handoff < 0.0
            || !(0.0 < l.scale.0 && l.scale.0 <= l.scale.1)
            || [s.yard, s.square, s.gate, s.patrol_square, s.patrol_gate, s.wander, s.neighbour].iter().any(|&r| r < 0)
        {
            return fail("live: a people reserve above nothing, at most 48 a settlement within 96 blocks, active under release under forget, searches above nothing (a tick's at least one way's), times above nothing, a scale low to high and no distance below nothing".into());
        }
        let r = &self.react;
        // Sleep and Home are where rain sends people: sheltering from them would leave
        // nowhere to go.
        if [r.greet_for, r.greet_every, r.afraid].iter().any(|&t| t <= 0.0) || r.greet_for >= r.greet_every || r.greet < 0.0 || r.fear < 0.0 || !(0.0..=1.0).contains(&r.rain) || r.shelter.iter().any(|p| matches!(p, Place::Sleep | Place::Home)) {
            return fail("react: times above nothing, a greeting shorter than the wait between them, no distance below nothing, rain within 0 to 1, and no shelter from Sleep or Home".into());
        }
        Ok(())
    }

    /// Every species that may live in a settlement, as written.
    pub fn species(&self) -> Vec<&str> {
        self.jobs.iter().map(|j| j.species.as_str()).chain(self.folk.iter().map(|f| f.species.as_str())).chain(self.guards.iter().map(|g| g.species.as_str())).collect()
    }

    /// Checks what the people are against the rest of the content: every species is a
    /// villager (placed by settlements, never by the land), and every job and folk work,
    /// the shrine and each of `react.outdoor` names a work or kind of building there is
    /// (`works`).
    pub fn check_against(&self, creatures: &voxmar_engine::creature::CreatureRegistry, works: &[String]) -> Result<(), ContentError> {
        let fail = |what: String| Err(ContentError(format!("settlements.ron: people: {what}")));
        let species = self.jobs.iter().map(|j| &j.species).chain(self.folk.iter().map(|f| &f.species)).chain(self.guards.iter().map(|g| &g.species));
        for name in species {
            match creatures.find(name).map(|id| creatures.get(id).def.kind) {
                Some(voxmar_engine::creature::Kind::Villager) => {}
                Some(_) => return fail(format!("`{name}` is not a villager (kind Villager)")),
                None => return fail(format!("there is no creature called `{name}`")),
            }
        }
        let places = self.jobs.iter().map(|j| &j.at).chain(self.folk.iter().filter_map(|f| f.work.as_ref())).chain(std::iter::once(&self.shrine)).chain(&self.react.outdoor);
        for name in places {
            if !works.contains(name) {
                return fail(format!("no building's work or kind is `{name}`"));
            }
        }
        Ok(())
    }

    pub fn routine(&self, name: &str) -> Option<usize> {
        self.routines.iter().position(|r| r.name == name)
    }
}

/// Someone who lives in a settlement.
#[derive(Clone, Debug, PartialEq)]
pub struct Resident {
    /// Their species, by its full name (`base:baker`).
    pub species: String,
    /// The building they live in, and the one they work at, as indices into its buildings.
    pub home: usize,
    pub work: Option<usize>,
    /// Their routine, as an index into the routines.
    pub routine: usize,
    pub guard: bool,
    pub seed: u64,
}

/// Whether building `b` is a place of `name`: by its work (`bakery`) or its kind (`inn`).
pub fn is(b: &Building, name: &str) -> bool {
    b.work.as_deref() == Some(name) || b.kind.name() == name
}

fn middle(b: &Building) -> IVec2 {
    (b.min + b.max) / 2
}

/// A count in `range`, more toward the top the more `lean` favours this place.
fn count(range: (u32, u32), lean: &Lean, levers: &Levers, rng: &mut Rng) -> u32 {
    let bias = lean.weigh(1.0, levers).max(0.05);
    let u = rng.unit().powf(1.0 / bias);
    (range.0 + (u * (range.1 - range.0 + 1) as f32) as u32).min(range.1)
}

/// The people of a settlement of size `size`, drawn from its seed.
pub fn residents(people: &PeopleDef, houses: &[HouseDef], size: &str, levers: &Levers, buildings: &[Building], gates: &[IVec2], seed: u64) -> Vec<Resident> {
    let mut rng = Rng::new(seed, 0x9e09_1e, 0, 0);
    let crowded = Lean(vec![(super::def::Key::Lever(super::def::Lever::Crowding), 1.0)]);
    // Room in each home: what its house kind holds, fuller where it is crowded.
    let room: Vec<u32> = buildings
        .iter()
        .map(|b| {
            if !matches!(b.kind, Kind::House | Kind::Trader | Kind::Inn) {
                return 0;
            }
            let range = houses.iter().find(|h| h.name == b.what).map_or_else(super::def::household, |h| h.people);
            count(range, &crowded, levers, &mut rng)
        })
        .collect();
    let mut used = vec![0u32; buildings.len()];
    let mut out: Vec<Resident> = Vec::new();
    // The home with room nearest `to`, preferring `first` when it has room.
    let home_near = |used: &[u32], first: Option<usize>, to: IVec2| -> Option<usize> {
        if let Some(i) = first.filter(|&i| used[i] < room[i]) {
            return Some(i);
        }
        (0..buildings.len()).filter(|&i| used[i] < room[i]).min_by_key(|&i| (middle(&buildings[i]) - to).length_squared())
    };
    let push = |out: &mut Vec<Resident>, species: &str, home: usize, work: Option<usize>, routine: &str, guard: bool| {
        let Some(routine) = people.routine(routine) else { return };
        let seed = mix(seed ^ mix(out.len() as u64 + 0x9e09));
        out.push(Resident { species: species.to_owned(), home, work, routine, guard, seed });
    };
    if let Some(guards) = people.guards.iter().rev().find(|g| g.size == size) {
        let n = count(guards.count, &guards.lean, levers, &mut rng);
        for i in 0..n as usize {
            // Living near the gates they watch. Housed before the workers, so a place
            // full of shops still keeps its watch.
            let to = if gates.is_empty() { buildings.first().map_or(IVec2::ZERO, middle) } else { gates[i % gates.len()] };
            let Some(home) = home_near(&used, None, to) else { break };
            used[home] += 1;
            push(&mut out, &guards.species, home, None, &guards.routines[i % guards.routines.len()], true);
        }
    }
    for job in &people.jobs {
        for (w, b) in buildings.iter().enumerate().filter(|(_, b)| is(b, &job.at)) {
            if !rng.chance(job.chance) {
                continue;
            }
            for _ in 0..count(job.count, &job.lean, levers, &mut rng) {
                // Above the shop when there is room, else the nearest home with room.
                let Some(home) = home_near(&used, Some(w), middle(b)) else { break };
                used[home] += 1;
                push(&mut out, &job.species, home, Some(w), &job.routine, false);
            }
        }
    }
    // Everyone else: drawn home by home.
    let places: Vec<(usize, &FolkDef, f32)> = people
        .folk
        .iter()
        .enumerate()
        .filter(|(_, f)| f.work.as_ref().is_none_or(|w| buildings.iter().any(|b| is(b, w))))
        .map(|(i, f)| (i, f, f.lean.weigh(f.weight, levers)))
        .collect();
    for home in 0..buildings.len() {
        while used[home] < room[home] {
            let fits: Vec<&(usize, &FolkDef, f32)> = places.iter().filter(|(_, f, _)| used[home] > 0 || f.alone).collect();
            let total: f32 = fits.iter().map(|p| p.2).sum();
            if total <= 0.0 {
                break;
            }
            let mut pick = rng.unit() * total;
            let Some(&&(_, folk, _)) = fits.iter().find(|p| {
                pick -= p.2;
                pick <= 0.0
            }) else {
                break;
            };
            // Working at the nearest building of their work.
            let work = folk.work.as_ref().and_then(|w| {
                (0..buildings.len()).filter(|&i| is(&buildings[i], w)).min_by_key(|&i| (middle(&buildings[i]) - middle(&buildings[home])).length_squared())
            });
            used[home] += 1;
            push(&mut out, &folk.species, home, work, &folk.routine, false);
        }
    }
    out
}

/// Where someone with `routine` is meant to be at `fraction` of day `day` (0 at sunrise),
/// and which slot of the day that is: their own day runs early or late by up to the
/// jitter, and each slot's place is drawn anew each day among those the settlement has
/// (`has`), leaning on its levers.
pub fn place_at(people: &PeopleDef, routine: usize, seed: u64, levers: &Levers, day: u64, fraction: f32, has: impl Fn(Place) -> bool) -> (Place, usize) {
    let r = &people.routines[routine];
    let shift = (unit(mix(seed ^ 0x51f7)) * 2.0 - 1.0) * people.jitter;
    let mut f = fraction - shift;
    let mut day = day;
    if f < 0.0 {
        f += 1.0;
        day = day.wrapping_sub(1);
    } else if f >= 1.0 {
        f -= 1.0;
        day += 1;
    }
    let slot = r.slots.iter().position(|s| f < s.until).unwrap_or(r.slots.len() - 1);
    let choices: Vec<(Place, f32)> = r.slots[slot].go.iter().filter(|g| has(g.place)).map(|g| (g.place, g.lean.weigh(g.weight, levers))).collect();
    let total: f32 = choices.iter().map(|c| c.1).sum();
    if total <= 0.0 {
        return (Place::Home, slot);
    }
    let mut pick = unit(mix(seed ^ mix(day ^ mix(slot as u64 + 0x5107)))) * total;
    let place = choices.iter().find(|c| {
        pick -= c.1;
        pick <= 0.0
    });
    (place.map_or(choices[0].0, |c| c.0), slot)
}

fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}
