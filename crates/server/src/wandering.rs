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
//! (fight when struck, hunt a player who comes near) still beats the walk. The errand is
//! `steady` until a player turns the party: till then a member runs only when its script
//! says, after that its own fear may run it. Members keep their
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
use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::brain::{Errand, Memory, Pose};
use voxmar_engine::creature::{CreatureId, Kind, mix, unit_from};
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3};
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::traits;
use voxmar_engine::wander::{Arrival, Hurt, Once, ResolvedNeed, ResolvedReward, Response, Role, Script, Stage, Want};
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::creatures::Budget;

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
/// How far ahead of an admin `/wander start` brings a party, in blocks: the first of these
/// with standing room, down to the admin's own feet.
const ADMIN_SPOT_AHEAD: [f32; 5] = [40.0, 24.0, 12.0, 4.0, 0.0];
/// A struck party's grudge: seconds it lasts without the player near, and how near that is.
const GRUDGE: f32 = 20.0;
const GRUDGE_RANGE: f32 = 45.0;
/// A bout ends, with nobody the winner, when the player walks this far from the fighter.
const BOUT_RANGE: f32 = 16.0;
/// A freed captive keeps this many blocks from the one it follows, and settles within this
/// many of that player's home.
const FOLLOW_CLOSE: f32 = 3.5;
const SETTLE_AT: f32 = 12.0;
/// A guided one has arrived this near a settlement's centre.
const TOWN_AT: f32 = 24.0;
/// Seconds a talk keeps a halted party standing.
const TALK_HOLDS: f32 = 12.0;
/// A chaser this near the quarry has caught it.
const CATCH: f32 = 2.5;
/// Seconds a wary player standing among a sprung-to-be ambush is waited out before the
/// ambushers give it up.
const WARY_WAIT: f32 = 20.0;
/// A lure's reveal is shown to a player this near it.
const REVEAL_WITHIN: f32 = 18.0;
/// A player who moves less than this between updates stands still, to a party that startles.
const STILL_MOVE: f32 = 0.6;
/// Blocks a startled party runs, and how fast.
const BOLT: f32 = 90.0;
const BOLT_PACE: f32 = 1.8;
/// A pointed-out block nearer the speaker than this is passed over: it can see that one.
const POINT_PAST: f32 = 6.0;
/// How far above and below the speaker a pointed-out block or a gatherer's work is looked for.
const SEEK_DEPTH: i32 = 12;
/// A hunt stalks (slowly) within this many blocks of its quarry, and rushes it from the
/// last few; it gives up on one it has not reached in so many seconds, and stands over a
/// kill this long.
const STALK: f32 = 10.0;
const RUSH: f32 = 3.0;
const HUNT_GIVE_UP: f32 = 60.0;
const OVER_THE_KILL: f32 = 6.0;
/// A journey's walk is at least this long (a town nearer is no journey), and a player must
/// keep with it this many seconds of the way to be thanked at its end.
const JOURNEY_LEAST: f32 = 80.0;
const ESCORTED: f32 = 30.0;
/// Rain or snow heavier than this falls, to a kind that comes only in the wet.
const FALLING: f32 = 0.05;

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
    /// Companions from the save whose species is not in the content (a mod turned off):
    /// saved again as they were, for when it comes back.
    waiting: Vec<SavedCompanion>,
    /// An admin's `/wander hold`: no event begins by chance until `/wander allow`. Kept only
    /// while the server runs.
    held: bool,
    /// Running events that one which comes after them has already joined.
    followed: FxHashSet<u64>,
}

struct Instance {
    id: u64,
    event: usize,
    seed: u64,
    /// Rolls of the party's own drawn so far (how long it holds, pauses, waits), so each is
    /// numbered by them, never by the tick it falls on.
    rolls: std::cell::Cell<u64>,
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
    /// Standing for a player until this tick (0 when not), and the earliest it halts again.
    halt: (u64, u64),
    /// The player whose blow, or whose cut ropes, turned the party against them.
    provoked: Option<PlayerId>,
    /// A freed captive, on their own.
    freed: bool,
    /// One befriended by a player, following them to a journey's end.
    guided: bool,
    /// The last player to strike one of the party.
    last_blow: Option<PlayerId>,
    /// To be let go: its members become creatures of the land like any other.
    release: bool,
    /// Let go at the end of a journey home (not given up on): its members stay as people
    /// of the player's home, and are saved with the world (`Live::companion`).
    settles: bool,
    /// How far its script has got (for the lines that go with it).
    stage: Stage,
    /// How each player's bouts with the party ended (`Won` or `Lost`): a bout's lines are
    /// for the one who fought it, not for everyone looking on.
    bouts: Vec<(PlayerId, Stage)>,
    /// The players who struck a member of each group of the cast.
    struck: FxHashSet<(PlayerId, usize)>,
    /// Over now: taken away at the end of this update (a lure that has shown what it led to).
    ended: bool,
    /// Works done by a gathering party, and the blocks it gave up on (none reached in time).
    worked: u16,
    skipped: Vec<IVec3>,
    /// The player a guarding party has warned, since a tick.
    warned: Option<(PlayerId, u64)>,
    /// A party that startles has bolted.
    startled: bool,
    /// Where each player near a party that startles stood, and since when.
    still: FxHashMap<PlayerId, (Vec3, u64)>,
    /// The tick each player near a camped party's show began watching, and those who have
    /// watched it through.
    watching: FxHashMap<PlayerId, u64>,
    shown: FxHashSet<PlayerId>,
}

impl Instance {
    fn new(id: u64, event: usize, seed: u64, born: u64, members: Vec<Member>, plan: Plan) -> Self {
        Self {
            id,
            event,
            seed,
            rolls: Default::default(),
            born,
            members,
            plan,
            told: Default::default(),
            spent: Default::default(),
            best: (f32::MAX, born),
            aim: None,
            detour: None,
            lost: 0,
            done: false,
            halt: (0, 0),
            provoked: None,
            freed: false,
            guided: false,
            last_blow: None,
            release: false,
            settles: false,
            stage: Stage::Passing,
            bouts: Vec::new(),
            struck: Default::default(),
            ended: false,
            worked: 0,
            skipped: Vec::new(),
            warned: None,
            startled: false,
            still: Default::default(),
            watching: Default::default(),
            shown: Default::default(),
        }
    }

    /// The party's next roll of a kind (`salt`), counted.
    fn next_roll(&self, salt: u64) -> u64 {
        next_party_roll(self.seed, &self.rolls, salt)
    }

    /// The party's next roll of a kind, as a share of one.
    fn roll(&self, salt: u64) -> f32 {
        unit_from(self.next_roll(salt))
    }
}

/// A party's next roll of a kind (`salt`) by its seed and count of rolls (`Instance::rolls`),
/// counted: for where the party is borrowed in part.
fn next_party_roll(seed: u64, rolls: &std::cell::Cell<u64>, salt: u64) -> u64 {
    let n = rolls.get();
    rolls.set(n + 1);
    mix(mix(seed, n), salt)
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
    /// What it works at, in a gathering party.
    work: Option<Work>,
}

/// A gatherer's block, where it stands to work it, since when it has been on its way, and
/// the tick it is done once there.
#[derive(Clone, Copy)]
struct Work {
    block: IVec3,
    stand: Vec3,
    since: u64,
    until: Option<u64>,
    /// Taking what was worked to `block`, where it is set down, not working it.
    carrying: bool,
}

enum Plan {
    /// A line to its goal, at a share of the walk.
    March { heading: Vec2, goal: Vec3, pace: f32 },
    /// A ring of stops, each paused at, a number of laps.
    Patrol { anchor: Vec3, next: usize, stops: u8, laps_left: u8, wait_until: Option<u64>, turn: f32 },
    /// To a spot, a stay there, then away.
    Camp { spot: Vec3, stay_until: Option<u64>, arrive_by: u64, away: f32 },
    /// Keeping to a player, until a tick.
    Shadow { until: u64 },
    /// Turned on a player: the party fights on by its own nature while they stay near.
    Avenge { player: PlayerId, until: u64 },
    /// A freed captive or a befriended one walking with a player, to `arrive`.
    Follow { player: PlayerId, until: u64, arrive: Arrival, guided: bool },
    /// Wandering about (round an anchor, or grazing along): to a goal, pausing, until a tick.
    Amble { anchor: Option<Vec3>, heading: Vec2, until: u64, goal: Option<Vec3>, pause_until: u64 },
    /// A chase: the quarry runs a line to its goal, its chasers on its heels.
    Chase { heading: Vec2, goal: Vec3, pace: f32 },
    /// Everyone stands until a tick, then the plan `next` goes on (a quarry held, a bait
    /// whose ambushers are beaten).
    Held { until: u64, next: Box<Plan> },
    /// An ambush: the bait comes to its `spot` (by `arrive_by` at the latest), settles, and
    /// waits there. A player in reach sets the trap to spring at a tick, by that player.
    Lie { heading: Vec2, spot: Vec3, arrive_by: u64, settled: Option<u64>, sprung: Option<(u64, PlayerId)>, wary_since: Option<u64> },
    /// A lure: waits for a follower, then leads to its goal and shows what it led to. It
    /// waits since a tick (when nobody follows), and has arrived at a tick.
    Lead { heading: Vec2, goal: Vec3, waiting: Option<u64>, arrived: Option<u64> },
    /// Working the blocks about an anchor, each worker its own.
    Gather { anchor: Vec3 },
    /// Walking to a town: the players near it on the way, by how many updates, and the tick
    /// it arrived.
    Journey { heading: Vec2, goal: Vec3, pace: f32, with: FxHashMap<PlayerId, u32>, arrived: Option<u64> },
    /// Hunting until a tick: after its quarry (since a tick), with kills `took`, standing
    /// over the last until a tick, and the quarry it gave up on.
    Hunt { target: Option<u32>, since: u64, took: u8, until: u64, rest_until: u64, passed: Vec<u32> },
    /// A bout: `fighter` spars with `player` until either is brought down to `down_to` of
    /// their health, the tick `until`, or the player walks off; then the plan `next` goes on.
    Bout { player: PlayerId, fighter: u32, down_to: f32, until: u64, next: Box<Plan> },
}

/// What the players near a party hold, and how the party's kind answers (`Reaction`).
#[derive(Default)]
struct Mood {
    wary: bool,
    /// Where the nearest player it keeps from, and the nearest it comes to, stand.
    shy: Option<Vec3>,
    draw: Option<Vec3>,
}

impl Wandering {
    /// Whether creature `id` belongs to a running event.
    pub(crate) fn manages(&self, id: u32) -> bool {
        self.by_creature.contains_key(&id)
    }

    /// Creature `id` is kept here, as if one of its own (a test's).
    #[cfg(test)]
    pub(crate) fn keep_for_test(&mut self, id: u32) {
        self.by_creature.insert(id, (0, 0));
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

/// A wanderer who settled at a player's home, as a save keeps them (`companions.ron`): the
/// individual comes back from its `seed`, where it was left.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCompanion {
    pub species: String,
    pub seed: u64,
    pub position: [f32; 3],
    pub yaw: f32,
    pub health: u16,
    /// Its own count of rolls (`Live::rolls`), so it thinks on rather than again; none in
    /// older saves.
    #[serde(default)]
    pub rolls: u64,
}

impl Server {
    /// The companions, as the save keeps them.
    pub(crate) fn saved_companions(&self) -> Vec<SavedCompanion> {
        let mut saved: Vec<SavedCompanion> = self
            .creatures
            .live
            .values()
            .filter(|c| c.companion)
            .map(|c| SavedCompanion { species: self.content.creatures.get(c.species).def.name.clone(), seed: c.seed, position: c.body.position.to_array(), yaw: c.body.yaw, health: c.health, rolls: c.rolls })
            .collect();
        // The same file for the same world, whatever order the creatures are kept in.
        saved.sort_by_key(|s| s.seed);
        saved.extend(self.wandering.waiting.iter().cloned());
        saved
    }

    /// Puts companions from a save back where they were.
    pub(crate) fn restore_companions(&mut self, saved: Vec<SavedCompanion>) {
        for s in saved {
            let Some(species) = self.content.creatures.id(&s.species) else {
                tracing::warn!("a companion {} waits: its species is not in the content", s.species);
                self.wandering.waiting.push(s);
                continue;
            };
            let at = Vec3::from_array(s.position);
            let Some(id) = self.place_creature(species, at, s.seed, 0.0) else { continue };
            let creature = self.creatures.live.get_mut(&id).expect("just made");
            creature.body.yaw = s.yaw;
            creature.health = s.health.min(creature.health).max(1);
            creature.rolls = s.rolls;
            (creature.companion, creature.wanderer) = (true, true);
        }
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

/// How far a party walks off when it gives up or is sent on its way: an ambush's own
/// `away`, else 80 blocks.
fn leaves(script: Script) -> f32 {
    match script {
        Script::Ambush { away, .. } => away,
        _ => 80.0,
    }
}

fn errand(to: Vec3, last: bool, pose: Pose, face: Option<Vec3>, pace: f32) -> Errand {
    Errand { to, last, pose, face, chase: None, pace, steady: true }
}

impl Server {
    /// Starts and moves the events, and ends those that are over.
    pub(crate) fn tick_wandering(&mut self) {
        if self.content.wandering.is_empty() {
            return;
        }
        let _span = tracing::info_span!("wandering").entered();
        let rules = *self.content.wandering.rules();
        if self.config.wandering_starts && self.tick.is_multiple_of(ticks(rules.scan_every)) {
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
        if self.wandering.held {
            return false;
        }
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
            self.try_follower(player, salt);
            return;
        }
        let rules = *self.content.wandering.rules();
        // By the world's count of rolls, never the tick it is tried on.
        let seed = mix(self.meta.roll_seed(crate::save::ROLL_PARTY), salt ^ 0x3a9d);
        if unit_from(mix(seed, 1)) >= rules.chance {
            return;
        }
        let Some(spot) = self.wandering_spot(player, seed) else { return };
        let (tick, day) = (self.tick, self.day);
        let wet = self.weather.local(&mut self.temperature, xz(spot.as_vec3()), tick, day).weather.precipitation > FALLING;
        let Some(event) = self.pick_event_in(player, spot, mix(seed, 5), wet) else { return };
        if let Some(id) = self.begin_event(Some(player), event, spot, mix(seed, 6)) {
            self.mark_forerunner(id, event, spot);
        }
    }

    /// A party that comes after another (`needs.after`) and started on its own: the nearest
    /// forerunner within reach that no party joined yet, the one its pick asked for
    /// (`forerunner_near`), counts as joined, so none joins it again through `try_follower`.
    fn mark_forerunner(&mut self, id: u64, event: usize, spot: IVec3) {
        let Some((forerunner, reach)) = self.content.wandering.get(event).def.needs.after.clone() else { return };
        if let Some(led) = self.forerunner_near(spot.as_vec3(), &forerunner, reach).filter(|&led| led != id) {
            self.wandering.followed.insert(led);
        }
    }

    /// One try at a party that comes after one already near `player` (`needs.after`): it
    /// joins its forerunner through the gap, `follow_after` seconds on at the earliest, and
    /// only once for each.
    fn try_follower(&mut self, player: PlayerId, salt: u64) {
        let rules = *self.content.wandering.rules();
        let Some(at) = self.players.get(&player).filter(|p| p.joined && p.health.alive()).map(|p| p.position) else { return };
        if self.wandering.held || self.wandering.active.len() >= rules.most_total {
            return;
        }
        let followed = |name: &str| self.content.wandering.iter().any(|(_, e)| e.def.needs.after.as_ref().is_some_and(|(n, _)| n == name));
        // Those near the player, about long enough and not yet joined, where their leaders stand.
        let ready: Vec<(u64, usize, Vec3)> = self
            .wandering
            .active
            .values()
            .filter(|inst| !self.wandering.followed.contains(&inst.id) && self.tick >= inst.born + ticks(rules.follow_after))
            .filter(|inst| followed(&self.content.wandering.get(inst.event).def.name))
            .filter_map(|inst| inst.members.first().and_then(|m| self.creatures.live.get(&m.creature)).map(|c| (inst.id, inst.event, c.body.position)))
            .filter(|(_, _, leader)| leader.distance(at) < rules.near)
            .collect();
        if ready.is_empty() {
            return;
        }
        let seed = mix(self.meta.roll_seed(crate::save::ROLL_FOLLOWER), salt ^ 0x5f0d);
        if unit_from(mix(seed, 1)) >= rules.chance {
            return;
        }
        let Some(spot) = self.wandering_spot(player, seed) else { return };
        let (tick, day) = (self.tick, self.day);
        let wet = self.weather.local(&mut self.temperature, xz(spot.as_vec3()), tick, day).weather.precipitation > FALLING;
        let Some(event) = self.pick_among(player, spot, mix(seed, 5), wet, true) else { return };
        // The one it joins is one of those ready: never a party another already joined.
        let Some((forerunner, reach)) = self.content.wandering.get(event).def.needs.after.clone() else { return };
        let kind = self.content.wandering.find(&forerunner);
        let led = ready
            .iter()
            .filter(|(_, e, leader)| Some(*e) == kind && flat(*leader, spot.as_vec3()) <= reach)
            .min_by(|a, b| flat(a.2, spot.as_vec3()).total_cmp(&flat(b.2, spot.as_vec3())))
            .map(|(id, _, _)| *id);
        let Some(led) = led else { return };
        if self.begin_event(Some(player), event, spot, mix(seed, 6)).is_some() {
            self.wandering.followed.insert(led);
        }
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

    /// The nearest of `ids` within `reach` blocks (across) of `from` and `SEEK_DEPTH` above or
    /// below, no nearer than `past`, and not one of `taken`. Chunks whose palette holds none
    /// of them are passed over whole, so a wide look is cheap.
    fn nearest_block(&self, from: Vec3, reach: f32, ids: &[BlockId], past: f32, taken: &[IVec3]) -> Option<IVec3> {
        use voxmar_engine::coords::{CHUNK_BITS, CHUNK_SIZE};
        let centre = from.floor().as_ivec3();
        let r = reach.ceil() as i32;
        let (low, high) = (centre - IVec3::new(r, SEEK_DEPTH, r), centre + IVec3::new(r, SEEK_DEPTH, r));
        let mut best: Option<(f32, IVec3)> = None;
        for cx in (low.x >> CHUNK_BITS)..=(high.x >> CHUNK_BITS) {
            for cz in (low.z >> CHUNK_BITS)..=(high.z >> CHUNK_BITS) {
                let Some(column) = self.world.column(ColumnPos::new(cx, cz)) else { continue };
                for cy in (low.y.max(0) >> CHUNK_BITS)..=(high.y.max(0) >> CHUNK_BITS) {
                    let Some(chunk) = column.chunks.get(cy as usize) else { continue };
                    if !chunk.palette().iter().any(|b| ids.contains(b)) {
                        continue;
                    }
                    let origin = IVec3::new(cx, cy, cz) * CHUNK_SIZE;
                    let (from_cell, to_cell) = (low.max(origin), high.min(origin + IVec3::splat(CHUNK_SIZE - 1)));
                    for y in from_cell.y..=to_cell.y {
                        for z in from_cell.z..=to_cell.z {
                            for x in from_cell.x..=to_cell.x {
                                let cell = IVec3::new(x, y, z);
                                let d = flat(cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5), from);
                                if d > reach || d < past || best.is_some_and(|(b, _)| b <= d) || taken.contains(&cell) {
                                    continue;
                                }
                                if ids.contains(&column.block(cell - IVec3::new(cx, 0, cz) * CHUNK_SIZE)) {
                                    best = Some((d, cell));
                                }
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_, cell)| cell)
    }

    /// Where a body stands to work block `cell`: a dry cell beside it (or in it, for what
    /// one stands in) at about its height.
    fn work_spot(&self, cell: IVec3) -> Option<Vec3> {
        [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::ZERO].into_iter().find_map(|side| {
            let at = cell + side;
            let stand = self.standing_spot(at.x, at.z, cell.y)?;
            ((stand.y - cell.y).abs() <= STEP && !self.is_wet(stand)).then(|| stand.as_vec3() + Vec3::new(0.5, 0.0, 0.5))
        })
    }

    /// Where a worker takes what it worked at `from`: beside the nearest of `deliver` to it
    /// within `reach` of the party's `anchor`, or (with nothing to deliver to, or none
    /// near) back to the anchor, where the party piles it.
    fn drop_spot(&self, anchor: Vec3, from: IVec3, reach: f32, deliver: &[BlockId]) -> Work {
        let to = (!deliver.is_empty())
            .then(|| self.nearest_block(from.as_vec3(), reach * 2.0, deliver, 0.0, &[]))
            .flatten()
            .filter(|cell| flat(cell.as_vec3(), anchor) <= reach * 2.0)
            .and_then(|cell| Some((cell, self.work_spot(cell)?)));
        let (block, stand) = to.unwrap_or((anchor.floor().as_ivec3() - IVec3::Y, anchor));
        Work { block, stand, since: self.tick, until: None, carrying: true }
    }

    /// What a `Point` from `from` names: the nearest of `want` within `reach`, past the few
    /// blocks around (so the way is worth telling), or nearer only if there is nothing else.
    fn point_at(&self, from: Vec3, want: &Want, reach: f32) -> Option<IVec3> {
        let ids = want.blocks(&self.content.blocks);
        self.nearest_block(from, reach, &ids, POINT_PAST, &[]).or_else(|| self.nearest_block(from, reach, &ids, 0.0, &[]))
    }

    /// Makes creature `id` young, to grow up as kept young do.
    /// The nearest settlement of the world's plan within `reach` blocks (across) of `from`,
    /// and the way to its centre.
    /// Where the leader of the nearest running party of kind `name` stands, within `reach`
    /// blocks of `from`.
    /// The nearest party of kind `name` within `reach` (across) of `from` that no party has
    /// come after yet: the one a party that comes after it joins.
    fn forerunner_near(&self, from: Vec3, name: &str, reach: f32) -> Option<u64> {
        let kind = self.content.wandering.find(name)?;
        self.wandering
            .active
            .values()
            .filter(|inst| inst.event == kind && !self.wandering.followed.contains(&inst.id))
            .filter_map(|inst| inst.members.first().and_then(|m| self.creatures.live.get(&m.creature)).map(|c| (inst.id, flat(c.body.position, from))))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    fn party_near(&self, from: Vec3, name: &str, reach: f32) -> Option<Vec3> {
        let kind = self.content.wandering.find(name)?;
        self.wandering
            .active
            .values()
            .filter(|inst| inst.event == kind)
            .filter_map(|inst| inst.members.first().and_then(|m| self.creatures.live.get(&m.creature)).map(|c| c.body.position))
            .filter(|at| flat(*at, from) <= reach)
            .min_by(|a, b| flat(*a, from).total_cmp(&flat(*b, from)))
    }

    fn town_near(&self, from: Vec3, reach: f32) -> Option<(Vec2, voxmar_worldgen::SettlementInfo)> {
        let at = IVec2::new(from.x.floor() as i32, from.z.floor() as i32);
        self.overworld
            .settlements_near(at, reach.ceil() as i32)
            .into_iter()
            .map(|s| (Vec2::new(s.centre.x as f32 + 0.5, s.centre.z as f32 + 0.5) - xz(from), s))
            .filter(|(to, _)| to.length() <= reach)
            .min_by(|a, b| a.0.length().total_cmp(&b.0.length()))
    }

    /// Whether creature `id` is fair game to a hunt after `quarry`: a live, wild one of
    /// those species, nobody's and of no party.
    fn is_quarry(&self, id: u32, quarry: &[CreatureId]) -> bool {
        self.creatures.live.get(&id).is_some_and(|c| c.health > 0 && c.owner.is_none() && !c.companion && quarry.contains(&c.species)) && !self.wandering.manages(id)
    }

    /// The nearest of `quarry` within `reach` blocks (across) of `from`, and not one of `passed`.
    fn quarry_near(&self, from: Vec3, reach: f32, quarry: &[CreatureId], passed: &[u32]) -> Option<u32> {
        self.creatures
            .live
            .iter()
            .filter(|(id, c)| !passed.contains(id) && flat(c.body.position, from) <= reach && (c.body.position.y - from.y).abs() <= SEEK_DEPTH as f32 && self.is_quarry(**id, quarry))
            .min_by(|a, b| flat(a.1.body.position, from).total_cmp(&flat(b.1.body.position, from)).then(a.0.cmp(b.0)))
            .map(|(id, _)| *id)
    }

    /// Whether `reward`, given from `from`, has something to tell: a `Point` or a `Town`
    /// only with one near.
    fn has_to_tell(&self, from: Vec3, reward: &ResolvedReward) -> bool {
        match reward {
            ResolvedReward::Point(want, reach) => self.point_at(from, want, *reach).is_some(),
            ResolvedReward::Town(reach) => self.town_near(from, *reach).is_some(),
            ResolvedReward::Toward(name, reach) => self.party_near(from, name, *reach).is_some(),
            ResolvedReward::OneOf(choices) => choices.iter().any(|c| self.has_to_tell(from, c)),
            _ => true,
        }
    }

    fn make_young(&mut self, id: u32) {
        let tick = self.tick;
        let Some(creature) = self.creatures.live.get_mut(&id) else { return };
        let def = &self.content.creatures.get(creature.species).def;
        let minutes = def.breeding.as_ref().map_or(10.0, |b| b.growth);
        creature.grows_up = Some(tick + ticks(minutes * 60.0));
        creature.adult_scale = creature.scale;
        creature.scale *= voxmar_engine::creature::husbandry::YOUNG_SCALE;
        creature.body.size = (def.body.size.0 * creature.scale, def.body.size.1 * creature.scale);
        creature.changed = true;
    }

    /// An event that suits `spot` and `player`, by its weights: its biome, the hour, what
    /// the player has reached, and a lesser chance for one they met lately.
    #[cfg(test)]
    fn pick_event(&self, player: PlayerId, spot: IVec3, seed: u64) -> Option<usize> {
        self.pick_event_in(player, spot, seed, false)
    }

    /// `pick_event`, where rain or snow falls (`wet`) or none does.
    fn pick_event_in(&self, player: PlayerId, spot: IVec3, seed: u64, wet: bool) -> Option<usize> {
        self.pick_among(player, spot, seed, wet, false)
    }

    /// `pick_event_in`, of only the kinds that come after another (`followers`), or of all.
    fn pick_among(&self, player: PlayerId, spot: IVec3, seed: u64, wet: bool, followers: bool) -> Option<usize> {
        let rules = self.content.wandering.rules();
        let p = self.players.get(&player)?;
        let site = self.overworld.site(spot.x, spot.z);
        let biome = self.overworld.biomes().get(site.biome);
        let daylight = self.daylight();
        let days = (self.day / voxmar_protocol::TICKS_PER_DAY as f64) as f32;
        let combat = self.combat_level(player);
        let recent = self.wandering.recent.get(&player);
        // The player's home, surveyed once and only if a kind asks how far it has come.
        let home = p.home_spot(&self.lit);
        let anchor = home.map(|h| h.floor().as_ivec3());
        let survey = std::cell::OnceCell::new();
        let home_is = |step: voxmar_engine::effect::EffectId| {
            let Some(anchor) = anchor else { return false };
            let survey = survey.get_or_init(|| self.survey_around(anchor, Some(player)));
            let calm = voxmar_engine::weather::Weather::NONE;
            let situation = voxmar_engine::effect::Situation { felt: 0.0, warmed: false, outdoors: self.open_sky(anchor + IVec3::Y * 2), weather: &calm, biome: "", biome_tags: &[], home: Some(survey), done: &p.quests_done, held: &[], near: &[], worn: &[], seated: false, night: self.night() };
            self.content.effects.get(step).when.as_ref().is_some_and(|when| when.holds(&situation))
        };
        let mut choices: Vec<(usize, f32)> = Vec::new();
        for (index, event) in self.content.wandering.iter() {
            let needs = &event.def.needs;
            if followers && needs.after.is_none() {
                continue;
            }
            let suits = event.suits_light(daylight)
                && event.suits_distance(flat(spot.as_vec3(), self.spawn))
                && days >= needs.days
                && needs.home.is_none_or(|h| h == home.is_some())
                && needs.combat.is_none_or(|(low, high)| (low..=high).contains(&combat))
                && needs.town.is_none_or(|reach| self.town_near(spot.as_vec3(), reach).is_some())
                && needs.wet.is_none_or(|w| w == wet)
                && needs.near_home.is_none_or(|near| home.is_some_and(|home| flat(home, p.position) <= near))
                && needs.after.as_ref().is_none_or(|(name, reach)| self.forerunner_near(spot.as_vec3(), name, *reach).is_some())
                && needs.met.as_ref().is_none_or(|name| p.tallies.get(&format!("met:{name}")).is_some_and(|&n| n > 0))
                && event.home_is.is_none_or(home_is);
            let weight = event.biome_weight(&biome.name, &biome.tags);
            if !suits || weight <= 0.0 {
                continue;
            }
            // A gathering party comes only where there is something to gather.
            if let Script::Gather { reach, .. } = event.def.script
                && (self.nearest_block(spot.as_vec3(), reach, &event.seek, 0.0, &[]).is_none()
                    || !event.deliver.is_empty() && self.nearest_block(spot.as_vec3(), reach, &event.deliver, 0.0, &[]).is_none())
            {
                continue;
            }
            // A journey comes only where there is a town to walk to, and a way to walk.
            if let Script::Journey { reach, .. } = event.def.script
                && self.town_near(spot.as_vec3(), reach).is_none_or(|(to, _)| to.length() < JOURNEY_LEAST)
            {
                continue;
            }
            // A hunt comes only where there is something to hunt.
            if let Script::Hunt { reach, .. } = event.def.script
                && self.quarry_near(spot.as_vec3(), reach, &event.quarry, &[]).is_none()
            {
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

    /// Whether the whole of a cast fits (`species`, each standing near `at`): people
    /// against their own budget, animals, birds and monsters against their caps of the land
    /// near every player and the world's wild room.
    fn cast_fits(&self, species: &[CreatureId], at: Vec3) -> bool {
        let limits = self.content.creatures.limits();
        let of = |s: &CreatureId| crate::creatures::budget(&self.content.creatures.get(*s).def);
        let people = species.iter().filter(|s| of(s).is_none()).count();
        // Room for `n` more under `cap`: no more than `cap - n` there now.
        let near = |budget: Budget, cap: usize| {
            let n = species.iter().filter(|s| of(s) == Some(budget)).count();
            n == 0 || (n <= cap && self.nearby_room(at, budget, cap + 1 - n))
        };
        self.wandering.people(self) + people <= self.content.wandering.rules().people_most
            && self.wild_room() >= species.len() - people
            && near(Budget::Animal, limits.animals_near)
            && near(Budget::Bird, limits.birds_near)
            && near(Budget::Monster, self.monsters_cap())
    }

    /// Brings a cast to `spot` and starts its script, for `player`, out of sight. Returns
    /// the event's id, or `None` when the leader could not stand there or the whole cast
    /// did not fit.
    pub(crate) fn begin_event(&mut self, player: Option<PlayerId>, event: usize, spot: IVec3, seed: u64) -> Option<u64> {
        self.begin_event_at(player, event, spot, seed, false)
    }

    /// `begin_event`, where `forced` is an admin's `/wander` start: the cast may appear in a
    /// player's view, and is brought whatever the spawn budgets say. A chance start brings
    /// the whole cast or none of it: never half a party.
    pub(crate) fn begin_event_at(&mut self, player: Option<PlayerId>, event: usize, spot: IVec3, seed: u64, forced: bool) -> Option<u64> {
        let rules = *self.content.wandering.rules();
        let def = self.content.wandering.get(event);
        let (script, groups) = (def.def.script, def.groups.clone());
        let at = spot.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        let angle = if forced { 0.0 } else { self.content.creatures.limits().spawn_unseen_angle };
        // Who comes, and where each stands: the leader on the spot, the rest about it.
        let mut cast: Vec<(usize, u8, u8, Vec3)> = Vec::new();
        for (g, (_, group)) in groups.iter().enumerate() {
            let (low, high) = group.count;
            let count = (low + (unit_from(mix(seed, 10 + g as u64)) * (high - low + 1) as f32) as u8).min(high);
            for i in 0..count {
                let stand = if cast.is_empty() {
                    at
                } else {
                    let a = unit_from(mix(seed, 100 + (g * 16 + i as usize) as u64)) * std::f32::consts::TAU;
                    let r = 1.5 + 2.5 * unit_from(mix(seed, 200 + (g * 16 + i as usize) as u64));
                    match self.standing_spot((at.x + a.cos() * r).floor() as i32, (at.z + a.sin() * r).floor() as i32, spot.y) {
                        Some(cell) => cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5),
                        None if forced => at,
                        None => return None,
                    }
                };
                cast.push((g, i, count, stand));
            }
        }
        if cast.is_empty() || (!forced && !self.cast_fits(&cast.iter().map(|c| groups[c.0].0).collect::<Vec<_>>(), at)) {
            return None;
        }
        let mut members: Vec<Member> = Vec::new();
        for (g, i, count, stand) in cast {
            let (species, group) = &groups[g];
            let member_seed = mix(seed, 1000 + (g * 16 + i as usize) as u64);
            let Some(creature) = self.place_creature(*species, stand, member_seed, angle) else {
                // One would be seen appearing: none of them come.
                for member in members {
                    self.creatures.take_away(member.creature);
                }
                return None;
            };
            self.creatures.live.get_mut(&creature).expect("just made").wanderer = true;
            if group.young {
                self.make_young(creature);
            }
            // The rest of the leader's own group walk behind it.
            let role = if group.role == Role::Lead && !members.is_empty() { Role::Follow } else { group.role };
            members.push(Member { creature, group: g, slot: voxmar_engine::wander::slot(role, i as usize, count as usize, member_seed), errand: None, mill_until: 0, mill_at: stand, work: None });
        }
        let id = self.wandering.next;
        self.wandering.next += 1;
        let target = player.and_then(|p| self.players.get(&p)).map(|p| p.position);
        let plan = self.plan_for(script, at, target, seed);
        for (i, member) in members.iter().enumerate() {
            self.wandering.by_creature.insert(member.creature, (id, i));
        }
        self.wandering.active.insert(id, Instance::new(id, event, seed, self.tick, members, plan));
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
            Script::March { pass, length, pace } => {
                let distance = target.map_or(60.0, |t| flat(t, at)).max(8.0);
                let nearest = pass.0 + roll(32) * (pass.1 - pass.0);
                let heading = turned(towards, side * (nearest / distance).clamp(0.0, 0.95).asin());
                Plan::March { heading, goal: at + Vec3::new(heading.x, 0.0, heading.y) * length, pace }
            }
            Script::Patrol { stops, laps, .. } => Plan::Patrol { anchor: at, next: 0, stops, laps_left: laps, wait_until: None, turn: side * std::f32::consts::TAU / stops as f32 },
            Script::Camp { near, stay, away, .. } => {
                let distance = target.map_or(60.0, |t| flat(t, at));
                let ahead = (distance - near).max(10.0);
                let spot = at + Vec3::new(towards.x, 0.0, towards.y) * ahead;
                let arrive_by = self.tick + ticks(ahead / 1.2 + 60.0);
                let _ = stay;
                Plan::Camp { spot, stay_until: None, arrive_by, away }
            }
            Script::Shadow { seconds, .. } => Plan::Shadow { until: self.tick + ticks(seconds) },
            Script::Chase { pass, length, pace, .. } => {
                let distance = target.map_or(60.0, |t| flat(t, at)).max(8.0);
                let nearest = pass.0 + roll(32) * (pass.1 - pass.0);
                let heading = turned(towards, side * (nearest / distance).clamp(0.0, 0.95).asin());
                Plan::Chase { heading, goal: at + Vec3::new(heading.x, 0.0, heading.y) * length, pace }
            }
            Script::Ambush { near, .. } => {
                let distance = target.map_or(60.0, |t| flat(t, at));
                let ahead = (distance - near).max(10.0);
                let spot = at + Vec3::new(towards.x, 0.0, towards.y) * ahead;
                Plan::Lie { heading: towards, spot, arrive_by: self.tick + ticks(ahead / 1.2 + 60.0), settled: None, sprung: None, wary_since: None }
            }
            // It stands where it appeared, watching, and leads off away from the player, a
            // little to one side, once one comes near.
            Script::Lure { length, .. } => {
                let heading = turned(-towards, side * 0.5);
                Plan::Lead { heading, goal: at + Vec3::new(heading.x, 0.0, heading.y) * length, waiting: None, arrived: None }
            }
            Script::Gather { .. } => Plan::Gather { anchor: at },
            // To the town's centre; with none in reach (an admin's start), on along a line.
            Script::Journey { reach, pace, .. } => match self.town_near(at, reach) {
                Some((to, town)) => {
                    let heading = to.try_normalize().unwrap_or(towards);
                    Plan::Journey { heading, goal: Vec3::new(town.centre.x as f32 + 0.5, at.y, town.centre.z as f32 + 0.5), pace, with: Default::default(), arrived: None }
                }
                None => Plan::March { heading: towards, goal: at + Vec3::new(towards.x, 0.0, towards.y) * 80.0, pace },
            },
            Script::Hunt { seconds, .. } => Plan::Hunt { target: None, since: self.tick, took: 0, until: self.tick + ticks(seconds), rest_until: 0, passed: Vec::new() },
            Script::Roam { seconds, .. } => Plan::Amble { anchor: Some(at), heading: towards, until: self.tick + ticks(seconds), goal: None, pause_until: 0 },
            Script::Drift { seconds, .. } => Plan::Amble { anchor: None, heading: turned(towards, side * 1.2), until: self.tick + ticks(seconds), goal: None, pause_until: 0 },
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
        // Those gone are gone, and one a player has taken in is theirs, not the party's.
        inst.members.retain(|m| self.creatures.live.get(&m.creature).is_some_and(|c| c.owner.is_none()));
        let live: Vec<u32> = inst.members.iter().map(|m| m.creature).collect();
        self.wandering.by_creature.retain(|c, (i, _)| *i != id || live.contains(c));
        // The leader gone, the next who can lead takes over; a pet, a flock or a captive cannot.
        let event = self.content.wandering.get(inst.event);
        let leads = |m: &Member| !matches!(event.groups[m.group].1.role, Role::Pet(_) | Role::Herd | Role::Captive | Role::Free);
        if !inst.freed && inst.members.first().is_some_and(|m| !leads(m))
            && let Some(next) = inst.members.iter().position(leads)
        {
            inst.members.swap(0, next);
        }
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
        // An ambush won: every lurker gone, the bait left standing a while.
        let registry = self.content.wandering.clone();
        let event = registry.get(inst.event);
        if inst.stage == Stage::Sprung
            && let Script::Ambush { hold, .. } = event.def.script
            && !inst.members.iter().any(|m| event.groups[m.group].1.role == Role::Lurk)
        {
            inst.stage = Stage::Won;
            inst.provoked = None;
            for member in &inst.members {
                if let Some(creature) = self.creatures.live.get_mut(&member.creature) {
                    creature.mind.forgive();
                }
            }
            let seconds = hold.0 + inst.roll(0x3c0) * (hold.1 - hold.0);
            let lead = positions.first().copied().unwrap_or(Vec3::ZERO);
            let nearest = players.iter().map(|&(_, p)| p).min_by(|a, b| a.distance(lead).total_cmp(&b.distance(lead)));
            let away = self.march_away(lead, nearest, Vec2::X, 0.9, leaves(event.def.script));
            inst.plan = Plan::Held { until: self.tick + ticks(seconds), next: Box::new(away) };
        }
        // Left with no captors, a captive is free.
        if event.has_captive() && !inst.freed {
            let captors = inst.members.iter().any(|m| event.groups[m.group].1.role != Role::Captive);
            if !captors && !inst.members.is_empty() {
                let by = inst.last_blow;
                self.free_captive(&mut inst, by);
            }
        }
        // With nobody left to lead (a pet or a flock whose person was killed), the rest are let go.
        let event = self.content.wandering.get(inst.event);
        if !inst.freed && !inst.members.iter().any(|m| !matches!(event.groups[m.group].1.role, Role::Pet(_) | Role::Herd | Role::Captive | Role::Free)) {
            inst.release = true;
        }
        if !inst.release {
            self.walk_plan(&mut inst, &players);
            self.show_to_watchers(&mut inst, &players);
        }
        if inst.ended {
            self.end_event(inst);
            return;
        }
        self.keep(inst);
    }

    /// A camped party's show: each player who has stood within its reach of the leader for
    /// its seconds is given what it gives, once, with the event's `show` line. Only while the
    /// party is settled and at peace; one who walks off starts again.
    fn show_to_watchers(&mut self, inst: &mut Instance, players: &[(PlayerId, Vec3)]) {
        let registry = self.content.wandering.clone();
        let event = registry.get(inst.event);
        let Some(show) = &event.def.show else { return };
        let settled = matches!(inst.plan, Plan::Camp { stay_until: Some(until), .. } if self.tick < until);
        let lead = inst.members.first().and_then(|m| self.creatures.live.get(&m.creature).map(|c| (m.creature, c.body.position)));
        let Some((speaker, at)) = lead.filter(|_| settled && inst.provoked.is_none() && !inst.startled) else {
            inst.watching.clear();
            return;
        };
        let near: Vec<PlayerId> = players
            .iter()
            .filter(|&&(id, p)| flat(p, at) <= show.near && (p.y - at.y).abs() <= 3.0 && self.players.get(&id).is_some_and(|pl| pl.health.alive()))
            .map(|&(id, _)| id)
            .collect();
        inst.watching.retain(|id, _| near.contains(id));
        for id in near {
            if inst.shown.contains(&id) {
                continue;
            }
            let since = *inst.watching.entry(id).or_insert(self.tick);
            if self.tick >= since + ticks(show.seconds) {
                inst.shown.insert(id);
                if let (Some(who), Some(p)) = (self.speaker_name(speaker), self.players.get(&id)) {
                    p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&event.key("show"))) });
                }
                self.tally(id, "met", &event.def.name, 1);
                self.hand_out(id, speaker, &event.show);
            }
        }
    }

    /// The plan of walking off from the nearest player (or on along `heading`), `pace` of a
    /// walk, for 80 blocks.
    fn march_away(&self, lead: Vec3, nearest: Option<Vec3>, heading: Vec2, pace: f32, far: f32) -> Plan {
        let away = nearest.map_or(heading, |p| (xz(lead) - xz(p)).try_normalize().unwrap_or(heading));
        Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * far, pace }
    }

    /// How the party's `kind` answers what the players round `at` hold (`Reaction`).
    fn mood(&self, kind: Kind, at: Vec3, players: &[(PlayerId, Vec3)]) -> Mood {
        let mut mood = Mood::default();
        let closer = |slot: &mut Option<Vec3>, to: Vec3| {
            if slot.is_none_or(|old| flat(to, at) < flat(old, at)) {
                *slot = Some(to);
            }
        };
        for reaction in self.content.wandering.reactions().iter().filter(|r| r.kinds.contains(&kind)) {
            for &(id, position) in players {
                if position.distance(at) > reaction.range {
                    continue;
                }
                let Some(p) = self.players.get(&id).filter(|p| p.health.alive()) else { continue };
                let holds = p.inventory.get(p.selected).is_some_and(|stack| self.content.blocks.def(stack.item).has_tag(&reaction.held));
                if !holds || reaction.combat.is_some_and(|(low, high)| !(low..=high).contains(&self.combat_level(id))) {
                    continue;
                }
                match reaction.response {
                    Response::Wary => mood.wary = true,
                    Response::Shy => closer(&mut mood.shy, position),
                    Response::Draw => closer(&mut mood.draw, position),
                }
            }
        }
        mood
    }

    /// Whether `player` holds what the party's `kind` keeps from or comes to (a `Shy` or
    /// `Draw` reaction), at any range: a guard is not raised against them.
    fn calms(&self, kind: Kind, player: PlayerId) -> bool {
        self.holds_for(kind, player, |r| matches!(r, Response::Shy | Response::Draw))
    }

    /// Whether `player` holds what draws the party's `kind` (a `Draw` reaction), at any range.
    fn draws(&self, kind: Kind, player: PlayerId) -> bool {
        self.holds_for(kind, player, |r| matches!(r, Response::Draw))
    }

    /// Whether `player` holds an item some reaction of `kind` passing `response` answers to.
    fn holds_for(&self, kind: Kind, player: PlayerId, response: impl Fn(&Response) -> bool) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let Some(held) = p.inventory.get(p.selected) else { return false };
        let def = self.content.blocks.def(held.item);
        self.content.wandering.reactions().iter().any(|r| {
            r.kinds.contains(&kind)
                && response(&r.response)
                && def.has_tag(&r.held)
                && r.combat.is_none_or(|(low, high)| (low..=high).contains(&self.combat_level(player)))
        })
    }

    /// Puts an instance back among the running, or lets its members go when it is released.
    fn keep(&mut self, inst: Instance) {
        if inst.release {
            // Those waiting in the save for their species count too, so they always fit back.
            let room = self.content.wandering.rules().companions_most.saturating_sub(self.creatures.live.values().filter(|c| c.companion).count() + self.wandering.waiting.len());
            for (i, member) in inst.members.iter().enumerate() {
                self.wandering.by_creature.remove(&member.creature);
                if let Some(creature) = self.creatures.live.get_mut(&member.creature) {
                    // Wherever they were let go is home now; those led home stay for good.
                    creature.mind.forgive();
                    creature.home = creature.body.position;
                    creature.companion = inst.settles && i < room;
                }
            }
            tracing::info!("wandering: {} lets its people go", self.content.wandering.get(inst.event).def.name);
            return;
        }
        for (i, m) in inst.members.iter().enumerate() {
            self.wandering.by_creature.insert(m.creature, (inst.id, i));
        }
        self.wandering.active.insert(inst.id, inst);
    }

    /// The name a creature is spoken of by.
    fn speaker_name(&self, id: u32) -> Option<Words> {
        let species = self.creatures.live.get(&id)?.species;
        let def = &self.content.creatures.get(species).def;
        Some(Words::Own(def_key("creature", &def.name), def.title.clone()))
    }

    /// Cuts the captive's bonds. Whoever freed it (if a player) is thanked, and with the
    /// chance the event gives it follows them; else it goes its own way, a peaceful
    /// wanderer of the land. When captors are left, it leaves them as an event of its own.
    fn free_captive(&mut self, inst: &mut Instance, by: Option<PlayerId>) {
        let event = self.content.wandering.get(inst.event).clone();
        let Some(freed) = event.def.freed.clone() else { return };
        let Some(at) = inst.members.iter().position(|m| event.groups[m.group].1.role == Role::Captive) else { return };
        let captive = inst.members[at].creature;
        // Whether it follows, by a living freer's own count of captives freed, never the
        // tick it is freed on; nobody's freeing draws nothing.
        let freer = by.filter(|p| self.players.get(p).is_some_and(|p| p.health.alive()));
        let follows = freer.filter(|&p| self.players.get_mut(&p).is_some_and(|p| p.draw(crate::save::Draw::Free)[0] < freed.follow));
        if let Some(player) = by.and_then(|p| self.players.get(&p).map(|_| p)) {
            if let Some(who) = self.speaker_name(captive) {
                self.players[&player].connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&event.key("thanks"))) });
            }
            self.tally(player, "met", &event.def.name, 1);
            self.hand_out(player, captive, &event.thanks);
        }
        let plan = follows.map(|player| Plan::Follow { player, until: self.tick + ticks(freed.follow_for), arrive: Arrival::Home, guided: false });
        let mut me = self.detach(inst, at);
        let target = me.as_mut().unwrap_or(inst);
        // Its own journey from now, on a clock of its own.
        (target.freed, target.born) = (true, self.tick);
        target.members[0].errand = None;
        match plan {
            Some(plan) => target.plan = plan,
            None => target.release = true,
        }
        if let Some(me) = me {
            self.keep(me);
        }
    }

    /// Takes member `at` out of `inst` to an instance of its own (kept by the caller), or
    /// `None` when it is alone, and `inst` itself is then what is to be changed.
    fn detach(&mut self, inst: &mut Instance, at: usize) -> Option<Instance> {
        if inst.members.len() == 1 {
            return None;
        }
        let member = inst.members.remove(at);
        let id = self.wandering.next;
        self.wandering.next += 1;
        // What was said to whom goes with it: a line its party has used up (the meal that
        // befriended it) is not offered again by the one who left.
        let mut alone = Instance::new(id, inst.event, inst.next_roll(0xde7a), self.tick, vec![member], Plan::Shadow { until: 0 });
        (alone.told, alone.spent) = (inst.told.clone(), inst.spent.clone());
        Some(alone)
    }

    /// The speaker `creature` of `inst` follows `player` toward its event's journey's end.
    fn befriend(&mut self, inst: &mut Instance, creature: u32, player: PlayerId) {
        let event = self.content.wandering.get(inst.event);
        let Some(guide) = event.def.guide.clone() else { return };
        let Some(at) = inst.members.iter().position(|m| m.creature == creature) else { return };
        let mut me = self.detach(inst, at);
        let target = me.as_mut().unwrap_or(inst);
        // Its own journey from now, on a clock of its own.
        (target.guided, target.born) = (true, self.tick);
        (target.provoked, target.members[0].errand) = (None, None);
        target.plan = Plan::Follow { player, until: self.tick + ticks(guide.follow_for), arrive: guide.arrive, guided: true };
        if let Some(me) = me {
            self.keep(me);
        }
    }

    /// Reward `gifts` to `player`, from creature `source` (where those that point the way
    /// start from); what is drawn, by the player's own count of rewards.
    fn hand_out(&mut self, player: PlayerId, source: u32, gifts: &[ResolvedReward]) {
        for gift in gifts {
            match *gift {
                ResolvedReward::Point(ref want, reach) => {
                    let from = self.creatures.live.get(&source).map(|c| c.body.position).or_else(|| self.players.get(&player).map(|p| p.position));
                    let (Some(from), Some(p)) = (from, self.players.get(&player)) else { return };
                    let text = match self.point_at(from, want, reach) {
                        Some(cell) => {
                            let to = xz(cell.as_vec3() + Vec3::splat(0.5)) - xz(from);
                            let far = ((to.length() / 10.0).round().max(1.0) * 10.0) as u32;
                            let block = self.world.block(cell).unwrap_or(BlockId::AIR);
                            Words::key("wander.point")
                                .with("thing", Words::Block(self.content.blocks.def(block).name.clone()))
                                .with("dir", crate::commands::compass(to))
                                .with("far", Words::text(far))
                        }
                        None => Words::key("wander.point_none"),
                    };
                    p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text });
                }
                ResolvedReward::Town(reach) => {
                    let from = self.creatures.live.get(&source).map(|c| c.body.position).or_else(|| self.players.get(&player).map(|p| p.position));
                    let (Some(from), Some(p)) = (from, self.players.get(&player)) else { return };
                    let text = match self.town_near(from, reach) {
                        Some((to, s)) => Words::key("wander.town")
                            .with("name", Words::Name(vec![("en".into(), s.name.0.clone()), ("ja".into(), s.name.1.clone())]))
                            .with("size", Words::key(&format!("settlement.size.{}", s.size)))
                            .with("dir", crate::commands::compass(to))
                            .with("far", Words::text(((to.length() / 50.0).round().max(1.0) * 50.0) as u32)),
                        None => Words::key("wander.town_none"),
                    };
                    p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text });
                }
                ResolvedReward::Toward(ref name, reach) => {
                    let from = self.creatures.live.get(&source).map(|c| c.body.position).or_else(|| self.players.get(&player).map(|p| p.position));
                    let (Some(from), Some(p)) = (from, self.players.get(&player)) else { return };
                    let (Some(at), Some(kind)) = (self.party_near(from, name, reach), self.content.wandering.find(name)) else { continue };
                    let to = xz(at) - xz(from);
                    let text = Words::key("wander.toward")
                        .with("who", Words::key(&self.content.wandering.get(kind).title_key()))
                        .with("dir", crate::commands::compass(to))
                        .with("far", Words::text(((to.length() / 10.0).round().max(1.0) * 10.0) as u32));
                    p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text });
                }
                ResolvedReward::Item(item, (low, high)) => {
                    // By the player's own count of rewards, never the tick it is given on.
                    let roll = self.draw_for(player, crate::save::Draw::Reward)[0];
                    let count = low + (roll * (high - low + 1) as f32) as u8;
                    let Some(p) = self.players.get_mut(&player) else { return };
                    let left = p.inventory.add(item, count.min(high));
                    p.changed(traits::INVENTORY);
                    if left > 0 {
                        let body = voxmar_engine::dropped::thrown(p.eye(), p.look());
                        self.items.spawn(voxmar_engine::inventory::ItemStack { item, count: left }, body, voxmar_engine::dropped::PICKUP_DELAY);
                    }
                }
                ResolvedReward::Xp(skill, xp) => self.grant_xp(player, skill, xp),
                ResolvedReward::Effect(effect, seconds) => {
                    let Some(p) = self.players.get_mut(&player) else { return };
                    p.give_effects(&[(effect, seconds)]);
                    p.changed(traits::VITALS);
                }
                ResolvedReward::Heal(health) => {
                    let Some(p) = self.players.get_mut(&player) else { return };
                    p.health.heal(health);
                    p.changed(traits::HEALTH);
                }
                ResolvedReward::OneOf(ref choices) => {
                    let from = self.creatures.live.get(&source).map(|c| c.body.position).or_else(|| self.players.get(&player).map(|p| p.position));
                    let Some(from) = from else { return };
                    let open: Vec<ResolvedReward> = choices.iter().filter(|c| self.has_to_tell(from, c)).cloned().collect();
                    if open.is_empty() {
                        continue;
                    }
                    let roll = self.draw_for(player, crate::save::Draw::Reward)[0];
                    let pick = ((roll * open.len() as f32) as usize).min(open.len() - 1);
                    self.hand_out(player, source, &open[pick..=pick]);
                }
                ResolvedReward::Weather(ref name, seconds) => {
                    let from = self.creatures.live.get(&source).map(|c| c.body.position).or_else(|| self.players.get(&player).map(|p| p.position));
                    let (Some(from), Some(preset)) = (from, self.content.weather.preset(name)) else { continue };
                    let preset = preset.at(preset.intensity);
                    self.weather.force(preset, xz(from), seconds.round() as u32, self.tick);
                }
                ResolvedReward::Prayer(points) => {
                    let most = self.prayer_most(player);
                    let Some(p) = self.players.get_mut(&player) else { return };
                    p.praying.points = (p.praying.points + points).min(most);
                }
                ResolvedReward::Grow(reach, stages) => {
                    let Some(from) = self.creatures.live.get(&source).map(|c| c.body.position) else { continue };
                    self.grow_around(from, reach, stages);
                }
                ResolvedReward::Free | ResolvedReward::Anger | ResolvedReward::Befriend | ResolvedReward::Leave | ResolvedReward::Hand(_) | ResolvedReward::Cook(..) | ResolvedReward::Spar(..) => {}
            }
        }
    }

    /// `player` struck creature `id`: if it is of a party, the party answers as its kind
    /// does (`hurt`): scatters, turns on them, or goes on as it was. A captive's pain is
    /// its captors' business, not an alarm.
    pub(crate) fn wandering_hurt(&mut self, id: u32, player: PlayerId) {
        let Some(&(instance, member)) = self.wandering.by_creature.get(&id) else { return };
        let Some(inst) = self.wandering.active.get_mut(&instance) else { return };
        inst.last_blow = Some(player);
        inst.struck.insert((player, inst.members[member].group));
        // A blow in a bout is the bout.
        if let Plan::Bout { player: p, fighter, .. } = inst.plan
            && p == player
            && fighter == id
        {
            return;
        }
        let role = self.content.wandering.get(inst.event).groups[inst.members[member].group].1.role;
        if role == Role::Captive || inst.freed {
            return;
        }
        self.provoke(instance, player);
    }

    /// Turns party `id` against `player`, by its event's `hurt`.
    fn provoke(&mut self, id: u64, player: PlayerId) {
        let Some(mut inst) = self.wandering.active.remove(&id) else { return };
        let event = self.content.wandering.get(inst.event);
        let lead = inst.members.first().and_then(|m| self.creatures.live.get(&m.creature)).map(|c| c.body.position);
        match (event.def.hurt, lead) {
            (Hurt::Avenge, _) => {
                if inst.stage == Stage::Waiting {
                    inst.stage = Stage::Sprung;
                }
                inst.provoked = Some(player);
                inst.plan = Plan::Avenge { player, until: self.tick + ticks(GRUDGE) };
                self.stir(&inst, player, true);
            }
            (Hurt::Scatter, Some(lead)) => {
                inst.provoked = Some(player);
                let from = self.players.get(&player).map_or(lead, |p| p.position);
                let away = (xz(lead) - xz(from)).try_normalize().unwrap_or(Vec2::X);
                inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 90.0, pace: 1.3 };
                inst.done = false;
                inst.halt = (0, u64::MAX);
                (inst.aim, inst.detour) = (None, None);
                self.stir(&inst, player, false);
            }
            _ => {}
        }
        self.wandering.active.insert(id, inst);
    }

    /// Angers (or, if they cannot fight, are young or are badly hurt, frightens) every
    /// member of a party at `player`.
    fn stir(&mut self, inst: &Instance, player: PlayerId, fight: bool) {
        for member in &inst.members {
            let Some(creature) = self.creatures.live.get_mut(&member.creature) else { continue };
            let species = self.content.creatures.get(creature.species);
            // One badly hurt that would run from a fight runs rather than be angered again.
            let health = f32::from(creature.health) / f32::from(species.def.health.max(1));
            let fights = fight && species.holds(creature.grows_up.is_some(), health, creature.mind.personality.bravery);
            if fights {
                creature.mind.anger = 1.0;
                creature.mind.remember(Memory::HurtBy(player), GRUDGE);
            } else {
                creature.mind.fear = 1.0;
                // Scattering, it drops any fight it had (a pet boar goes with its party).
                if species.flees() {
                    creature.mind.anger = 0.0;
                }
                creature.mind.remember(Memory::ScaredBy(player), GRUDGE);
            }
        }
    }

    /// Takes the cast away, those not dead, without a death.
    fn end_event(&mut self, inst: Instance) {
        tracing::info!("wandering: {} ends", self.content.wandering.get(inst.event).def.name);
        self.wandering.followed.remove(&inst.id);
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
        let registry = self.content.wandering.clone();
        let event = registry.get(inst.event);
        let (script, groups) = (event.def.script, event.groups.clone());
        let leader = inst.members[0].creature;
        let lead = self.creatures.live[&leader].body.position;
        let nearest = players.iter().map(|&(_, p)| p).min_by(|a, b| a.distance(lead).total_cmp(&b.distance(lead)));
        let night = self.daylight() < 0.3;
        // What the players near hold, and how this kind of party takes it: an ambush
        // answers as its lurkers do, anything else as its leader.
        let kind = {
            let lurker = inst.members.iter().find(|m| groups[m.group].1.role == Role::Lurk);
            let member = lurker.unwrap_or(&inst.members[0]);
            self.creatures.live.get(&member.creature).map_or(Kind::Animal, |c| self.content.creatures.get(c.species).def.kind)
        };
        let mood = self.mood(kind, lead, players);
        let chasers: Vec<Vec3> = inst.members.iter().filter(|m| groups[m.group].1.role == Role::Chaser).filter_map(|m| self.creatures.live.get(&m.creature).map(|c| c.body.position)).collect();
        let lurkers: Vec<Vec3> = inst.members.iter().filter(|m| groups[m.group].1.role == Role::Lurk).filter_map(|m| self.creatures.live.get(&m.creature).map(|c| c.body.position)).collect();
        // A party that startles bolts from a player who comes near, unless they have stood
        // still (or stand still holding what draws it), and a stalker comes nearer.
        if let Some(startle) = &event.def.startle
            && !inst.startled
            && inst.provoked.is_none()
        {
            let members: Vec<Vec3> = inst.members.iter().filter_map(|m| self.creatures.live.get(&m.creature).map(|c| c.body.position)).collect();
            let mut bolt = None;
            for &(id, at) in players {
                let d = members.iter().map(|m| flat(*m, at)).fold(f32::MAX, f32::min);
                if d > startle.near * 1.5 {
                    inst.still.remove(&id);
                    continue;
                }
                let tick = self.tick;
                // One first seen is taken to be standing still, until they move.
                let since = inst.still.entry(id).or_insert((at, 0));
                if flat(since.0, at) > STILL_MOVE {
                    *since = (at, tick);
                }
                let still = tick >= since.1 + ticks(startle.still_for);
                let skill = event.stalk.map_or(0.0, |(skill, per)| self.players.get(&id).map_or(0.0, |p| self.content.skills.level(p.skills.xp(skill)) as f32 * per));
                let reach = if still { startle.still } else { (startle.near - skill).max(startle.least) };
                let drawn = still && self.draws(kind, id);
                if d <= reach && !drawn {
                    bolt = Some(at);
                    break;
                }
            }
            if let Some(from) = bolt {
                inst.startled = true;
                let away = (xz(lead) - xz(from)).try_normalize().unwrap_or(Vec2::X);
                inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * BOLT, pace: BOLT_PACE };
                (inst.done, inst.aim, inst.detour) = (false, None, None);
                for member in &inst.members {
                    if let Some(creature) = self.creatures.live.get_mut(&member.creature) {
                        creature.mind.fear = 1.0;
                    }
                }
            }
        }
        // A party that guards warns a player off, then sets on one who stays.
        let mut warning: Option<Vec3> = None;
        if let Some(guard) = event.def.guard
            && inst.provoked.is_none()
            && matches!(inst.plan, Plan::March { .. } | Plan::Amble { .. } | Plan::Gather { .. } | Plan::Camp { .. } | Plan::Patrol { .. })
        {
            let members: Vec<Vec3> = inst.members.iter().filter_map(|m| self.creatures.live.get(&m.creature).map(|c| c.body.position)).collect();
            // The nearest player who holds nothing that calms the kind: one with a torch or
            // food is let be, and the rest are not.
            let close = players
                .iter()
                .filter(|&&(id, _)| !self.calms(kind, id))
                .map(|&(id, at)| (id, at, members.iter().map(|m| flat(*m, at)).fold(f32::MAX, f32::min)))
                .min_by(|a, b| a.2.total_cmp(&b.2));
            match close {
                Some((id, at, d)) if d <= guard.near => {
                    let since = match inst.warned {
                        Some((who, since)) if who == id => since,
                        _ => {
                            inst.warned = Some((id, self.tick));
                            if let Some(p) = self.players.get(&id) {
                                p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key(&event.key("warn")) });
                            }
                            self.tick
                        }
                    };
                    if d <= guard.near * 0.5 || self.tick >= since + ticks(guard.warn) {
                        inst.provoked = Some(id);
                        inst.warned = None;
                        inst.plan = Plan::Avenge { player: id, until: self.tick + ticks(GRUDGE) };
                    } else {
                        warning = Some(at);
                    }
                }
                Some((id, at, d)) if d < guard.calm && inst.warned.is_some_and(|(who, _)| who == id) => warning = Some(at),
                _ => inst.warned = None,
            }
        }
        // Standing for a player who has come near: a party that halts does, for a while, then
        // goes on, and not again for a while.
        let mut halted = false;
        if let (Some(halt), Plan::March { .. } | Plan::Amble { .. } | Plan::Gather { .. } | Plan::Journey { arrived: None, .. }) = (event.def.halt, &inst.plan)
            && !inst.done
            && inst.provoked.is_none()
            && !mood.wary
        {
            let close = nearest.filter(|p| flat(lead, *p) <= halt.near);
            if self.tick < inst.halt.0 {
                halted = nearest.is_some_and(|p| flat(lead, p) <= halt.near * 2.0);
                if !halted {
                    inst.halt = (0, self.tick + ticks(halt.cooldown));
                }
            } else if inst.halt.0 > 0 {
                inst.halt = (0, self.tick + ticks(halt.cooldown));
                inst.aim = None;
            } else if self.tick >= inst.halt.1 && close.is_some() {
                let seconds = halt.seconds.0 + inst.roll(0x4a17) * (halt.seconds.1 - halt.seconds.0);
                inst.halt.0 = self.tick + ticks(seconds);
                halted = true;
            }
        }
        // Where the leader goes, and how it is once there.
        let (mut goal, mut pose, mut face, mut heading, mut last): (Option<Vec3>, Pose, Option<Vec3>, Vec2, bool) = (None, Pose::Idle, None, Vec2::new(1.0, 0.0), false);
        let mut pace = 1.0;
        let mut free = false;
        let mut arrival: Option<(PlayerId, bool)> = None;
        let mut stir: Option<(PlayerId, bool)> = None;
        // What the script decides this update, done after it (the plan is borrowed meanwhile).
        let mut spring: Option<PlayerId> = None;
        let mut caught: Option<f32> = None;
        let mut resume: Option<Plan> = None;
        let mut leaving = false;
        let mut revealed = false;
        let mut take: Option<u32> = None;
        let mut escorted: Vec<PlayerId> = Vec::new();
        let mut hunted_out = false;
        let mut spar: Option<(u32, PlayerId)> = None;
        let mut bout_end: Option<(Stage, u32, PlayerId)> = None;
        match (&mut inst.plan, script) {
            // Warning a player off: everyone stands and faces them.
            (_, _) if warning.is_some() => {
                heading = warning.map_or(heading, |w| (xz(w) - xz(lead)).try_normalize().unwrap_or(heading));
                (last, pose, face) = (true, Pose::Alert, warning);
            }
            (Plan::March { heading: h, .. } | Plan::Amble { heading: h, .. } | Plan::Journey { heading: h, .. }, _) if halted => {
                heading = *h;
                last = true;
                face = nearest;
            }
            (Plan::Gather { .. }, _) if halted => {
                last = true;
                face = nearest;
            }
            // Gathering: each worker's errand is its own, set below.
            (Plan::Gather { .. }, _) => last = true,
            // Wild things keep from what they fear and come to what they like, whatever the
            // players near hold (`Reaction`).
            (Plan::Amble { heading: h, goal: g, pause_until, .. }, _) if mood.shy.is_some() || mood.draw.is_some() => {
                pace = 0.9;
                if let Some(from) = mood.shy {
                    let away = (xz(lead) - xz(from)).try_normalize().unwrap_or(*h);
                    (heading, *h, *g, *pause_until) = (away, away, None, 0);
                    goal = Some(lead + Vec3::new(away.x, 0.0, away.y) * 10.0);
                    pace = 1.3;
                } else if let Some(to) = mood.draw {
                    heading = (xz(to) - xz(lead)).try_normalize().unwrap_or(*h);
                    face = Some(to);
                    if flat(lead, to) > 3.0 {
                        goal = Some(to);
                    } else {
                        last = true;
                    }
                }
            }
            (Plan::March { heading: h, goal: g, pace: p }, _) => {
                heading = *h;
                pace = *p;
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
                                inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 80.0, pace: 1.0 };
                            }
                        }
                    }
                    None if flat(lead, stop) < THERE => {
                        *wait_until = Some(self.tick + ticks(pause.0 + unit_from(next_party_roll(inst.seed, &inst.rolls, 0x9a05)) * (pause.1 - pause.0)));
                        last = true;
                    }
                    None => goal = Some(stop),
                }
            }
            (Plan::Camp { spot, stay_until, arrive_by, away, .. }, Script::Camp { stay, .. }) => {
                heading = (xz(*spot) - xz(lead)).try_normalize().unwrap_or(heading);
                match *stay_until {
                    None if flat(lead, *spot) < THERE * 1.5 || self.tick >= *arrive_by => {
                        *stay_until = Some(self.tick + ticks(stay.0 + unit_from(next_party_roll(inst.seed, &inst.rolls, 0x57a7)) * (stay.1 - stay.0)));
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
                        inst.plan = Plan::March { heading: away_heading, goal: *spot + Vec3::new(away_heading.x, 0.0, away_heading.y) * length, pace: 1.0 };
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
                    inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 80.0, pace: 1.0 };
                }
            },
            (Plan::Avenge { player, until }, _) => {
                // The grudge holds while the player stays near; the party's own nature does
                // the fighting, and nothing walks it.
                let target = self.players.get(player).filter(|p| p.health.alive()).map(|p| p.position);
                if target.is_some_and(|p| flat(lead, p) < GRUDGE_RANGE) {
                    *until = (*until).max(self.tick + ticks(GRUDGE));
                }
                if self.tick >= *until || target.is_none() {
                    let away = nearest.map_or(heading, |p| (xz(lead) - xz(p)).try_normalize().unwrap_or(heading));
                    inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 80.0, pace: 1.0 };
                } else {
                    free = true;
                    stir = Some((*player, true));
                }
            }
            (Plan::Follow { player, until, arrive, guided }, _) => match self.players.get(player).filter(|p| p.health.alive()) {
                Some(p) if self.tick < *until => {
                    heading = (xz(p.position) - xz(lead)).try_normalize().unwrap_or(heading);
                    let arrived = match arrive {
                        Arrival::Home => p.home_spot(&self.lit).is_some_and(|h| flat(h, lead) < SETTLE_AT),
                        Arrival::Town => self.villagers.town_within(lead, TOWN_AT),
                    };
                    if arrived {
                        // At the journey's end it stays, a person of the land.
                        // At their own home it is one of theirs from now (`Live::companion`).
                        (inst.release, inst.settles) = (true, matches!(arrive, Arrival::Home));
                        arrival = Some((*player, *guided));
                    } else if flat(lead, p.position) <= FOLLOW_CLOSE {
                        last = true;
                        face = Some(p.position);
                    } else {
                        goal = Some(p.position);
                        pace = if flat(lead, p.position) > 12.0 { 1.5 } else { 1.1 };
                    }
                }
                _ => inst.release = true,
            },
            (Plan::Amble { anchor, heading: h, until, goal: g, pause_until }, _) => {
                let (pause, rest, leg, radius) = match script {
                    Script::Roam { pause, rest, radius, .. } => (pause, rest, (0.0, 0.0), radius),
                    Script::Drift { pause, leg, .. } => (pause, false, leg, 0.0),
                    _ => ((4.0, 8.0), false, (6.0, 12.0), 8.0),
                };
                let roll = |salt: u64| unit_from(next_party_roll(inst.seed, &inst.rolls, salt << 32));
                heading = *h;
                pace = 0.7;
                if self.tick >= *until {
                    let away = nearest.map_or(heading, |p| (xz(lead) - xz(p)).try_normalize().unwrap_or(heading));
                    inst.plan = Plan::March { heading: away, goal: lead + Vec3::new(away.x, 0.0, away.y) * 80.0, pace: 0.9 };
                } else if let Some(to) = *g {
                    if flat(lead, to) < THERE {
                        *g = None;
                        *pause_until = self.tick + ticks(pause.0 + roll(1) * (pause.1 - pause.0));
                        last = true;
                    } else {
                        goal = Some(to);
                    }
                } else if self.tick < *pause_until {
                    last = true;
                    pose = if rest { Pose::Resting } else { Pose::Idle };
                } else {
                    // Another spot: round the anchor, or on along a heading that turns a little.
                    *g = Some(match *anchor {
                        Some(at) => {
                            let a = roll(2) * std::f32::consts::TAU;
                            at + Vec3::new(a.cos(), 0.0, a.sin()) * radius * roll(3).sqrt()
                        }
                        None => {
                            *h = turned(*h, (roll(4) - 0.5) * 1.4);
                            lead + Vec3::new(h.x, 0.0, h.y) * (leg.0 + roll(5) * (leg.1 - leg.0))
                        }
                    });
                    last = true;
                }
            }
            // A chase: the quarry runs, its chasers on its heels; run down, it is held.
            (Plan::Chase { heading: h, goal: g, pace: p }, Script::Chase { hold, .. }) => {
                (heading, pace) = (*h, *p);
                if inst.members[0].group != 0 || chasers.is_empty() {
                    // No quarry left to run down, or no one left to run it down.
                    resume = Some(Plan::March { heading: *h, goal: *g, pace: *p });
                } else if chasers.iter().any(|c| flat(*c, lead) <= CATCH) {
                    caught = Some(hold.0 + inst.roll(0xca7) * (hold.1 - hold.0));
                } else if flat(lead, *g) < THERE * 2.0 {
                    inst.done = true;
                    last = true;
                } else {
                    goal = Some(*g);
                }
            }
            // A bout: the fighter's own nature fights it; it ends at a share of either's health.
            (Plan::Bout { player, fighter, down_to, until, next }, _) => {
                let them = self.players.get(player).filter(|p| p.health.alive());
                let me = self.creatures.live.get(fighter);
                let low = |health: f32, most: f32| health <= (most * *down_to).ceil();
                let end = match (them, me) {
                    (Some(p), Some(c)) if self.tick < *until && flat(p.position, c.body.position) <= BOUT_RANGE => {
                        if low(p.health.current as f32, p.health.max as f32) {
                            Some(Stage::Lost)
                        } else if low(c.health as f32, self.content.creatures.get(c.species).def.health as f32) {
                            Some(Stage::Won)
                        } else {
                            None
                        }
                    }
                    _ => Some(Stage::Passing),
                };
                match end {
                    None => (free, spar) = (true, Some((*fighter, *player))),
                    Some(stage) => {
                        resume = Some(std::mem::replace(&mut **next, Plan::Shadow { until: 0 }));
                        bout_end = Some((stage, *fighter, *player));
                    }
                }
            }
            (Plan::Held { until, next }, _) => {
                if let Plan::March { heading: h, .. } = &**next {
                    heading = *h;
                }
                last = true;
                face = nearest;
                if self.tick >= *until {
                    resume = Some(std::mem::replace(&mut **next, Plan::Shadow { until: 0 }));
                }
            }
            // An ambush: the bait settles ahead of the player, the lurkers about it. A player
            // in reach springs it a moment later, unless they stand there armed and wary.
            (Plan::Lie { heading: h, spot, arrive_by, settled, sprung, wary_since }, Script::Ambush { trigger, delay, patience, .. }) => {
                heading = *h;
                match *settled {
                    None if flat(lead, *spot) < THERE * 1.5 || self.tick >= *arrive_by => {
                        *settled = Some(self.tick);
                        last = true;
                    }
                    None => goal = Some(*spot),
                    Some(since) => {
                        last = true;
                        pose = Pose::Resting;
                        if sprung.is_none() {
                            inst.stage = Stage::Waiting;
                        }
                        let triggered = players.iter().find(|&&(_, p)| flat(lead, p) <= trigger || lurkers.iter().any(|l| flat(*l, p) <= trigger)).map(|&(id, _)| id);
                        match (*sprung, triggered) {
                            (None, Some(_)) if mood.wary => {
                                wary_since.get_or_insert(self.tick);
                            }
                            (None, Some(player)) => *sprung = Some((self.tick + ticks(delay.0 + unit_from(next_party_roll(inst.seed, &inst.rolls, 0x5b7)) * (delay.1 - delay.0)), player)),
                            (None, None) => *wary_since = None,
                            (Some((at, player)), _) if self.tick >= at => spring = Some(player),
                            _ => {}
                        }
                        if sprung.is_none() && (self.tick >= since + ticks(patience) || wary_since.is_some_and(|w| self.tick >= w + ticks(WARY_WAIT))) {
                            leaving = true;
                        }
                    }
                }
            }
            // A lure: stands watching until one comes near, then keeps ahead of them.
            (Plan::Lead { heading: h, goal: g, waiting, arrived }, Script::Lure { keep, pace: lead_pace, patience, linger, .. }) => {
                heading = *h;
                if let Some(at) = *arrived {
                    inst.stage = Stage::Arrived;
                    (last, face) = (true, nearest);
                    if self.tick >= at + ticks(linger) {
                        inst.ended = true;
                    }
                } else if flat(lead, *g) < THERE * 2.0 {
                    *arrived = Some(self.tick);
                    inst.stage = Stage::Arrived;
                    (last, revealed) = (true, true);
                } else {
                    let near = nearest.map(|p| flat(lead, p));
                    if near.is_some_and(|d| d <= keep.1) && mood.shy.is_none() {
                        *waiting = None;
                        inst.stage = Stage::Leading;
                        goal = Some(*g);
                        pace = lead_pace * if near.is_some_and(|d| d < keep.0) { 1.25 } else { 1.0 };
                    } else {
                        (last, pose, face) = (true, Pose::Alert, nearest);
                        let since = *waiting.get_or_insert(self.tick);
                        if self.tick >= since + ticks(patience) {
                            leaving = true;
                        }
                    }
                }
            }
            // A journey to town: on toward it, counting who keeps with it; at its end, those
            // who came the way are thanked, and it stays there.
            (Plan::Journey { heading: h, goal: g, pace: p, with, arrived }, Script::Journey { near, .. }) => {
                (heading, pace) = (*h, *p);
                if arrived.is_some() {
                    (last, face) = (true, nearest);
                } else {
                    for &(id, at) in players {
                        if flat(lead, at) <= near && (at.y - lead.y).abs() <= 8.0 {
                            *with.entry(id).or_default() += 1;
                        }
                    }
                    if flat(lead, *g) < THERE * 4.0 {
                        *arrived = Some(self.tick);
                        inst.stage = Stage::Arrived;
                        inst.done = true;
                        last = true;
                        let enough = (ESCORTED / self.content.wandering.rules().errand_every).ceil() as u32;
                        escorted = players.iter().filter(|&&(id, at)| flat(lead, at) <= near && with.get(&id).is_some_and(|&n| n >= enough)).map(|&(id, _)| id).collect();
                    } else {
                        goal = Some(*g);
                    }
                }
            }
            // A hunt: after the nearest quarry, stalking the last of the way and rushing it at
            // the end; one of them close enough brings it down.
            (Plan::Hunt { target, since, took, until, rest_until, passed }, Script::Hunt { reach, strike, takes, .. }) => {
                if self.tick < *rest_until {
                    (last, pose) = (true, Pose::Crouched);
                } else if *took >= takes || self.tick >= *until || inst.done {
                    hunted_out = true;
                } else {
                    if let Some(t) = *target
                        && (!self.is_quarry(t, &event.quarry) || self.tick > *since + ticks(HUNT_GIVE_UP))
                    {
                        passed.push(t);
                        *target = None;
                    }
                    if target.is_none() {
                        (*target, *since) = (self.quarry_near(lead, reach, &event.quarry, passed), self.tick);
                    }
                    match target.and_then(|t| self.creatures.live.get(&t).map(|c| (t, c.body.position))) {
                        None => hunted_out = true,
                        Some((t, at)) => {
                            heading = (xz(at) - xz(lead)).try_normalize().unwrap_or(heading);
                            face = Some(at);
                            let reached = inst.members.iter().any(|m| self.creatures.live.get(&m.creature).is_some_and(|c| flat(c.body.position, at) <= strike && (c.body.position.y - at.y).abs() <= 2.5));
                            if reached {
                                take = Some(t);
                                *took += 1;
                                *rest_until = self.tick + ticks(OVER_THE_KILL);
                                *target = None;
                                last = true;
                            } else {
                                let d = flat(lead, at);
                                pace = if d <= RUSH + strike { 1.6 } else if d <= STALK { 0.7 } else { 1.0 };
                                goal = Some(at);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(quarry) = take {
            // Whoever drew first blood is given a share, the kill's own loot with it.
            let first = self.creatures.live.get(&quarry).and_then(|c| c.hurt_by).filter(|p| self.players.get(p).is_some_and(|p| p.health.alive()));
            let shared = first.filter(|_| !event.share.is_empty());
            self.creature_felled(quarry, shared);
            inst.stage = Stage::Took;
            if let Some(player) = shared {
                if let (Some(who), Some(p)) = (self.speaker_name(leader), self.players.get(&player)) {
                    p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&event.key("share"))) });
                }
                self.tally(player, "met", &event.def.name, 1);
                self.hand_out(player, leader, &event.share);
            }
        }
        if !escorted.is_empty() {
            let event = self.content.wandering.get(inst.event).clone();
            for player in escorted {
                if let (Some(who), Some(p)) = (self.speaker_name(leader), self.players.get(&player)) {
                    p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&event.key("arrived"))) });
                }
                self.tally(player, "met", &event.def.name, 1);
                self.hand_out(player, leader, &event.reveal);
            }
        }
        if hunted_out {
            inst.plan = self.march_away(lead, nearest, heading, 0.9, leaves(script));
            (inst.done, inst.aim, inst.detour) = (false, None, None);
        }
        if let Some(seconds) = caught {
            inst.stage = Stage::Caught;
            let next = Plan::March { heading, goal: lead + Vec3::new(heading.x, 0.0, heading.y) * 80.0, pace: 0.9 };
            inst.plan = Plan::Held { until: self.tick + ticks(seconds), next: Box::new(next) };
        }
        if let Some(plan) = resume {
            inst.plan = plan;
            inst.stage = Stage::Passing;
        }
        // A bout over: the fighter bears no grudge and binds its scrapes; won or lost, it says so.
        if let Some((stage, fighter, player)) = bout_end {
            if stage != Stage::Passing {
                inst.bouts.retain(|&(who, _)| who != player);
                inst.bouts.push((player, stage));
            }
            if let Some(c) = self.creatures.live.get_mut(&fighter) {
                c.mind.forgive();
                c.health = self.content.creatures.get(c.species).def.health;
                c.changed = true;
            }
            let line = match stage {
                Stage::Won => Some("bout_won"),
                Stage::Lost => Some("bout_lost"),
                _ => None,
            };
            if let (Some(line), Some(who), Some(p)) = (line, self.speaker_name(fighter), self.players.get(&player)) {
                let key = self.content.wandering.get(inst.event).key(line);
                p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&key)) });
            }
        }
        if let Some((fighter, player)) = spar
            && let Some(c) = self.creatures.live.get_mut(&fighter)
        {
            c.mind.anger = 1.0;
            c.mind.remember(Memory::HurtBy(player), GRUDGE);
        }
        if leaving {
            inst.plan = self.march_away(lead, nearest, heading, 1.0, leaves(script));
            inst.stage = Stage::Passing;
        }
        if let Some(player) = spring {
            inst.stage = Stage::Sprung;
            inst.provoked = Some(player);
            inst.plan = Plan::Avenge { player, until: self.tick + ticks(GRUDGE) };
            (free, stir) = (true, Some((player, true)));
            let shout = inst.members.iter().find(|m| groups[m.group].1.role == Role::Lurk).map(|m| m.creature);
            if let (Some(who), Some(p)) = (shout.and_then(|c| self.speaker_name(c)), self.players.get(&player)) {
                let key = self.content.wandering.get(inst.event).key("sprung");
                p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&key)) });
            }
        }
        if revealed
            && let Some(&(player, _)) = players.iter().filter(|&&(_, p)| flat(lead, p) <= REVEAL_WITHIN).min_by(|a, b| flat(lead, a.1).total_cmp(&flat(lead, b.1)))
        {
            let event = self.content.wandering.get(inst.event).clone();
            if let Some(p) = self.players.get(&player) {
                p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key(&event.key("arrived")) });
            }
            self.tally(player, "met", &event.def.name, 1);
            self.hand_out(player, leader, &event.reveal);
        }
        if let Some((player, fight)) = stir {
            self.stir(inst, player, fight);
        }
        if let Some((player, guided)) = arrival {
            let event = self.content.wandering.get(inst.event).clone();
            if guided {
                if let Some(who) = self.speaker_name(leader) {
                    if let Some(p) = self.players.get(&player) {
                        p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", Words::key(&event.key("arrived"))) });
                    }
                }
                self.tally(player, "met", &event.def.name, 1);
                self.hand_out(player, leader, &event.guide_thanks);
            } else if let (Some(who), Some(p)) = (self.speaker_name(leader), self.players.get(&player)) {
                p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.settled").with("who", who) });
            }
        }
        if free || inst.release {
            for member in &mut inst.members {
                member.errand = None;
            }
            return;
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
                Role::Lead | Role::Follow | Role::Flank | Role::Rear | Role::Ahead | Role::Captive | Role::Herd | Role::Lurk => {
                    let to = lead - forward * slot.y + right * slot.x;
                    let apart = flat(me, to);
                    let there = apart < if role == Role::Herd { 3.0 } else { 1.2 };
                    // Lurkers lie crouched while the trap is set.
                    let pose = if role == Role::Lurk && inst.stage == Stage::Waiting && there { Pose::Crouched } else if last { pose } else { Pose::Idle };
                    Some(errand(to, there, pose, face, if apart > HURRY { 1.0 + ((apart - HURRY) / 8.0).min(0.6) } else { 1.0 }))
                }
                // On the quarry's heels; run down, they stand about it and, after, walk beside it.
                Role::Chaser => match (&inst.plan, script) {
                    (Plan::Chase { pace, .. }, Script::Chase { gain, .. }) => Some(errand(lead, false, Pose::Idle, None, pace * gain)),
                    _ => {
                        let flank = voxmar_engine::wander::slot(Role::Flank, i, inst.members.len(), mix(inst.seed, i as u64));
                        let to = lead - forward * flank.y + right * flank.x;
                        let held = inst.stage == Stage::Caught;
                        Some(errand(to, flat(me, to) < 1.2, if held { Pose::Alert } else { Pose::Idle }, held.then_some(lead), if flat(me, to) > HURRY { 1.4 } else { 1.0 }))
                    }
                },
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
                    let a = unit_from(next_party_roll(inst.seed, &inst.rolls, (i as u64) << 20)) * std::f32::consts::TAU;
                    let r = unit_from(next_party_roll(inst.seed, &inst.rolls, (i as u64) << 24)) * mill;
                    let at = *spot + Vec3::new(a.cos(), 0.0, a.sin()) * r;
                    member.mill_at = at;
                    member.mill_until = self.tick + ticks(6.0 + 10.0 * unit_from(next_party_roll(inst.seed, &inst.rolls, (i as u64) << 28)));
                }
                let there = flat(me, inst.members[i].mill_at) < 1.0;
                errands[i] = Some(errand(inst.members[i].mill_at, there, if night && there { Pose::Resting } else { Pose::Idle }, Some(*spot), 1.0));
            }
        }
        // Gathering: each worker to its own block, to work it a while, then the next.
        if let (Plan::Gather { anchor }, Script::Gather { reach, work, visits, carry }) = (&inst.plan, script) {
            let anchor = *anchor;
            if inst.worked >= visits as u16 && !halted {
                inst.plan = self.march_away(lead, nearest, heading, 0.9, leaves(script));
            } else {
                let mut taken: Vec<IVec3> = inst.members.iter().filter_map(|m| m.work.map(|w| w.block)).chain(inst.skipped.iter().copied()).collect();
                let mut idle = 0;
                let mut workers = 0;
                for i in 0..inst.members.len() {
                    let role = if i == 0 { Role::Lead } else { groups[inst.members[i].group].1.role };
                    if !matches!(role, Role::Lead | Role::Follow | Role::Flank | Role::Rear | Role::Ahead | Role::Herd) {
                        continue;
                    }
                    workers += 1;
                    let me = self.creatures.live[&inst.members[i].creature].body.position;
                    if halted {
                        errands[i] = Some(errand(me, true, Pose::Idle, nearest, 1.0));
                        continue;
                    }
                    if let Some(w) = inst.members[i].work
                        && w.carrying
                    {
                        // Set down where it goes (on time or not, it was carried).
                        let late = w.until.is_none() && self.tick > w.since + ticks(2.0 * reach / 0.8 + 20.0);
                        let done = w.until.is_some_and(|until| self.tick >= until);
                        if late || done {
                            inst.worked += 1;
                            if let (true, Some((near, stages))) = (done, carry.and_then(|c| c.tend)) {
                                self.grow_around(w.block.as_vec3() + Vec3::new(0.5, 1.0, 0.5), near, stages);
                            }
                            inst.members[i].work = None;
                        }
                    } else if let Some(w) = inst.members[i].work {
                        let gone = !self.block(w.block).is_some_and(|b| event.seek.contains(&b));
                        let late = w.until.is_none() && self.tick > w.since + ticks(reach / 0.8 + 20.0);
                        let done = w.until.is_some_and(|until| self.tick >= until);
                        if gone || late || done {
                            if late {
                                inst.skipped.push(w.block);
                            }
                            inst.members[i].work = None;
                            if done && !gone {
                                match carry {
                                    Some(_) => inst.members[i].work = Some(self.drop_spot(anchor, w.block, reach, &event.deliver)),
                                    None => inst.worked += 1,
                                }
                            }
                        }
                    }
                    if inst.members[i].work.is_none() {
                        while let Some(cell) = self.nearest_block(anchor, reach, &event.seek, 0.0, &taken) {
                            taken.push(cell);
                            if let Some(stand) = self.work_spot(cell) {
                                inst.members[i].work = Some(Work { block: cell, stand, since: self.tick, until: None, carrying: false });
                                break;
                            }
                            inst.skipped.push(cell);
                        }
                    }
                    errands[i] = Some(match &mut inst.members[i].work {
                        Some(w) => {
                            let there = flat(me, w.stand) < 1.0;
                            if there && w.until.is_none() {
                                let roll = unit_from(next_party_roll(inst.seed, &inst.rolls, (i as u64) << 40));
                                let (least, most) = match carry {
                                    Some(c) if w.carrying => c.set,
                                    _ => work,
                                };
                                w.until = Some(self.tick + ticks(least + roll * (most - least)));
                            }
                            errand(w.stand, there, if there { Pose::Crouched } else { Pose::Idle }, Some(w.block.as_vec3() + Vec3::splat(0.5)), 1.0)
                        }
                        None => {
                            idle += 1;
                            errand(me, true, Pose::Idle, None, 1.0)
                        }
                    });
                }
                // Nothing left to work: the party is done here.
                if workers > 0 && idle == workers {
                    inst.worked = visits as u16;
                }
            }
        }
        // A party nobody has turned walks its script and nothing else; once a player has
        // (a scatter, a sprung ambush) each member's own fear has its say again.
        let steady = inst.provoked.is_none();
        for (member, errand) in inst.members.iter_mut().zip(errands) {
            member.errand = errand.map(|e| Errand { steady, ..e });
        }
    }

    /// `player` right-clicks creature `id`: if it speaks for a wandering party, it answers
    /// (and gives), and the click is spent. Returns whether it did. A party that has been
    /// struck, or whose captive has been freed, says what suits that.
    /// Whether `player` stands near enough to wandering creature `id` to be heard
    /// (`speak_reach`).
    /// The share of health a blow between creature `fighter` and `player` may bring either
    /// down to, while the two are in a bout (`Spar`): neither falls in one.
    pub(crate) fn bout_floor(&self, fighter: u32, player: PlayerId) -> Option<f32> {
        let &(instance, _) = self.wandering.by_creature.get(&fighter)?;
        match self.wandering.active.get(&instance)?.plan {
            Plan::Bout { player: p, fighter: f, down_to, .. } if p == player && f == fighter => Some(down_to),
            _ => None,
        }
    }

    pub(crate) fn within_speech(&self, player: PlayerId, id: u32) -> bool {
        let (Some(p), Some(c)) = (self.players.get(&player), self.creatures.live.get(&id)) else { return false };
        flat(p.position, c.body.position) <= self.content.wandering.rules().speak_reach && (p.position.y - c.body.position.y).abs() <= self.content.wandering.rules().speak_reach
    }

    pub(crate) fn meet(&mut self, player: PlayerId, id: u32) -> bool {
        let Some(&(instance, member)) = self.wandering.by_creature.get(&id) else { return false };
        let Some(inst) = self.wandering.active.get(&instance) else { return false };
        if inst.provoked.is_some() || matches!(inst.plan, Plan::Bout { .. }) {
            return false;
        }
        let registry = self.content.wandering.clone();
        let event = registry.get(inst.event);
        let Some(meeting) = inst.members.get(member).and_then(|m| event.meeting(m.group)) else { return false };
        let Some(p) = self.players.get(&player).filter(|p| p.health.alive()) else { return false };
        let held = p.inventory.get(p.selected);
        let level = |skill| self.content.skills.level(p.skills.xp(skill));
        let (has_home, hurt) = (p.home_spot(&self.lit).is_some(), p.health.current < p.health.max);
        // A line that hands over one of the party says so only with one to hand, and room
        // among what the player keeps.
        let kept_room = self.player_name(player).is_some_and(|name| self.kept_by(&name) < self.content.creatures.limits().kept_each);
        let can_hand = |group: usize| kept_room && inst.members.iter().any(|m| m.group == group && self.creatures.live.get(&m.creature).is_some_and(|c| c.owner.is_none()));
        // A line that frees a captive is for a party that still holds one.
        let holds_captive = inst.members.iter().any(|m| event.groups[m.group].1.role == Role::Captive && self.creatures.live.contains_key(&m.creature));
        let seed = inst.seed;
        // Whether line `i` applies, and (with `held_too`) the player holds what it takes.
        let applies = |i: usize, line: &voxmar_engine::wander::Line, held_too: bool| {
            let used = match line.once {
                Once::Always => false,
                Once::Player => inst.told.contains(&(player, i + meeting.who * 64)),
                Once::Party => inst.spent.contains(&(i + meeting.who * 64)),
            };
            !used
                && line.needs.iter().all(|need| match need {
                    ResolvedNeed::When(stage) => inst.stage == *stage || inst.bouts.contains(&(player, *stage)),
                    ResolvedNeed::Struck(group) => inst.struck.contains(&(player, *group)),
                    ResolvedNeed::Skill(skill, at_least) => level(*skill) >= *at_least,
                    ResolvedNeed::Below(skill, below) => level(*skill) < *below,
                    ResolvedNeed::Home(want) => has_home == *want,
                    ResolvedNeed::Hurt => hurt,
                    ResolvedNeed::Holds(tag) => held.is_some_and(|h| self.content.blocks.def(h.item).has_tag(tag)),
                })
                && line.gives.iter().all(|g| match g {
                    ResolvedReward::Hand(group) => can_hand(*group),
                    ResolvedReward::Cook(station, ..) => held.is_some_and(|h| self.content.cooking.for_item(h.item, std::slice::from_ref(station)).is_some()),
                    // Where nothing of the kind is near, they have nothing to tell.
                    ResolvedReward::Point(..) | ResolvedReward::Town(_) | ResolvedReward::Toward(..) | ResolvedReward::OneOf(_) => self.creatures.live.get(&id).is_some_and(|c| self.has_to_tell(c.body.position, g)),
                    // Prayer is restored only to one short of it.
                    ResolvedReward::Prayer(_) => p.praying.points < self.prayer_most(player),
                    // A blessing on the fields wants a field with something growing in it.
                    ResolvedReward::Grow(reach, _) => self.creatures.live.get(&id).is_some_and(|c| self.unripe_near(c.body.position, *reach)),
                    // A bout is offered only to two who are both above where it would stop.
                    ResolvedReward::Spar(down_to, _) => {
                        let above = |health: f32, most: f32| health > (most * down_to).ceil();
                        above(p.health.current as f32, p.health.max as f32)
                            && self.creatures.live.get(&id).is_some_and(|c| above(c.health as f32, self.content.creatures.get(c.species).def.health as f32))
                    }
                    _ => true,
                })
                && (!held_too
                    || line.wants_of(seed).is_none_or(|(want, count)| {
                        held.is_some_and(|h| {
                            h.count >= count
                                && match want {
                                    Want::Item(item) => h.item == item,
                                    Want::Tag(tag) => self.content.blocks.def(h.item).has_tag(&tag),
                                }
                        })
                    }))
                && (holds_captive || !line.gives.contains(&ResolvedReward::Free))
        };
        let line = meeting.lines.iter().enumerate().filter(|_| !inst.freed).find(|&(i, line)| applies(i, line, true));
        // With no line to say, one that asks for what the player lacks says what it asks.
        let asked = match line {
            None if !inst.freed => meeting.lines.iter().enumerate().find(|&(i, line)| !line.asks.is_empty() && applies(i, line, false)).and_then(|(_, line)| {
                let (item, count) = line.ask_of(seed)?;
                Some(Words::key(&event.key(&format!("{}_ask", line.name))).with("thing", Words::Block(self.content.blocks.def(item).name.clone())).with("count", Words::text(count)))
            }),
            _ => None,
        };
        let (freed, guided) = (inst.freed, inst.guided);
        let (words, gives, wants, mark) = match line {
            Some((i, line)) => (Words::key(&event.key(&line.name)), line.gives.clone(), line.wants_of(seed), Some((i + meeting.who * 64, line.once))),
            None if freed => (Words::key(&event.key("freed")), Vec::new(), None, None),
            None if guided => (Words::key(&event.key("company")), Vec::new(), None, None),
            None => match asked {
                Some(asked) => (asked, Vec::new(), None, None),
                // Which greeting, by the listener's own count of greetings heard, never the
                // tick: drawn only when one is said.
                None => {
                    let greet = self.draw_for(player, crate::save::Draw::Greet)[0];
                    let variant = ((greet * meeting.greets as f32) as u8).min(meeting.greets - 1);
                    (Words::key(&event.greeting(meeting, variant)), Vec::new(), None, None)
                }
            },
        };
        // A line that takes or gives something; one said for its words alone is no dealing.
        let dealt = line.is_some_and(|(_, line)| !line.gives.is_empty() || line.wants_of(seed).is_some());
        let who = self.speaker_name(id);
        let halts = event.def.halt.is_some();
        let inst = self.wandering.active.get_mut(&instance).expect("checked");
        match mark {
            Some((i, Once::Player)) => {
                inst.told.insert((player, i));
            }
            Some((i, Once::Party)) => {
                inst.spent.insert(i);
            }
            _ => {}
        }
        // A talk keeps the party standing a while longer.
        if halts && inst.halt.0 > 0 {
            inst.halt.0 = inst.halt.0.max(self.tick + ticks(TALK_HOLDS));
        }
        // What the player hands over (all a cook will take of it, up to its most), and what
        // they are given: what a cook cooked, less what it keeps.
        let cook = gives.iter().find_map(|g| match g {
            ResolvedReward::Cook(station, most, keep) => Some((station.clone(), *most, *keep)),
            _ => None,
        });
        if let Some((_, count)) = wants {
            let p = self.players.get_mut(&player).expect("checked");
            let selected = p.selected;
            let held = p.inventory.get(selected);
            let count = match &cook {
                Some((_, most, _)) => held.map_or(count, |h| h.count.min(*most).max(count)),
                None => count,
            };
            let taken = held.map(|h| h.item);
            p.inventory.take(selected, count);
            p.changed(traits::INVENTORY);
            if let (Some((station, _, keep)), Some(item)) = (&cook, taken)
                && let Some(recipe) = self.content.cooking.for_item(item, std::slice::from_ref(station))
                && count > *keep
            {
                let back = count - keep;
                self.hand_out(player, id, &[ResolvedReward::Item(recipe.output, (back, back))]);
            }
        }
        if let (Some(who), Some(p)) = (who, self.players.get(&player)) {
            p.connection.send(ServerMessage::Chat { kind: ChatKind::Chat, text: Words::key("wander.said").with("who", who).with("text", words) });
        }
        // A dealing with the kind, for quests (`Met`).
        if dealt {
            self.tally(player, "met", &event.def.name, 1);
        }
        self.hand_out(player, id, &gives);
        if gives.contains(&ResolvedReward::Free)
            && let Some(mut inst) = self.wandering.active.remove(&instance)
        {
            self.free_captive(&mut inst, Some(player));
            self.keep(inst);
        }
        if gives.contains(&ResolvedReward::Befriend)
            && let Some(mut inst) = self.wandering.active.remove(&instance)
        {
            self.befriend(&mut inst, id, player);
            self.keep(inst);
        }
        for gift in &gives {
            if let ResolvedReward::Hand(group) = *gift {
                self.hand_over(instance, group, player);
            }
        }
        if gives.contains(&ResolvedReward::Anger) {
            self.provoke(instance, player);
        }
        if let Some(&ResolvedReward::Spar(down_to, seconds)) = gives.iter().find(|g| matches!(g, ResolvedReward::Spar(..)))
            && let Some(mut inst) = self.wandering.active.remove(&instance)
        {
            let next = std::mem::replace(&mut inst.plan, Plan::Shadow { until: 0 });
            inst.plan = Plan::Bout { player, fighter: id, down_to, until: self.tick + ticks(seconds), next: Box::new(next) };
            if let Some(c) = self.creatures.live.get_mut(&id) {
                c.mind.anger = 1.0;
                c.mind.remember(Memory::HurtBy(player), GRUDGE);
            }
            self.keep(inst);
        }
        if gives.contains(&ResolvedReward::Leave)
            && let Some(mut inst) = self.wandering.active.remove(&instance)
        {
            let lead = inst.members.first().and_then(|m| self.creatures.live.get(&m.creature)).map_or(Vec3::ZERO, |c| c.body.position);
            let from = self.players.get(&player).map(|p| p.position);
            inst.plan = self.march_away(lead, from, Vec2::X, 1.0, leaves(event.def.script));
            inst.stage = Stage::Passing;
            inst.provoked = None;
            inst.done = false;
            (inst.aim, inst.detour) = (None, None);
            self.keep(inst);
        }
        true
    }

    /// Hands a member of `group` of party `instance` to `player`, kept as if tamed: it leaves
    /// the party (at its next update) and settles where it stands.
    fn hand_over(&mut self, instance: u64, group: usize, player: PlayerId) {
        let Some(inst) = self.wandering.active.get(&instance) else { return };
        let Some(id) = inst.members.iter().filter(|m| m.group == group).map(|m| m.creature).find(|c| self.creatures.live.get(c).is_some_and(|c| c.owner.is_none())) else { return };
        let (name, day, tamed) = (self.player_name(player), self.day, self.content.creatures.condition().tamed);
        let Some(creature) = self.creatures.live.get_mut(&id) else { return };
        creature.owner = name;
        creature.owner_id = Some(player);
        (creature.condition, creature.reckoned, creature.fed) = (tamed, day, day);
        creature.mind.forgive();
        creature.mind.fear = 0.0;
        creature.changed = true;
        self.wandering.by_creature.remove(&id);
        self.settle(id);
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
            Some("hold") => {
                self.wandering.held = true;
                Ok(Words::key("command.wander.held"))
            }
            Some("allow") => {
                self.wandering.held = false;
                Ok(Words::key("command.wander.allowed"))
            }
            Some("start") => self.wander_start(args, source).inspect_err(|why| {
                // Every refusal reaches the log, so a gate session that started nothing says why.
                tracing::info!("wandering: /wander start {:?} refused: {why:?}", &args[1..]);
            }),
            Some(_) => Err(Words::key("command.wander.what")),
        }
    }

    /// An admin's /wander start: the party nearest ahead that has room, or why not.
    fn wander_start(&mut self, args: &[String], source: Source) -> Result<Words, Words> {
        let registry = self.content.wandering.clone();
        let name = args.get(1).ok_or_else(|| Words::key("command.wander.what"))?;
        let event = registry.find(name).ok_or_else(|| Words::key("command.wander.unknown").with("name", name.as_str()))?;
        let (player, feet, look) = match source {
            Source::Player(p) => {
                let player = self.players.get(&p).ok_or_else(|| Words::key("command.wander.what"))?;
                (Some(p), player.position, player.look().with_y(0.0).normalize_or(Vec3::NEG_Z))
            }
            Source::Console => (None, self.spawn, Vec3::NEG_Z),
        };
        // Ahead of them, out of the way, or nearer where the land ahead falls away or
        // rises out of reach: an admin's start never fails for want of a spot while
        // they stand on something.
        let spot = ADMIN_SPOT_AHEAD.into_iter().find_map(|ahead| {
            let at = (feet + look * ahead).floor().as_ivec3();
            self.load_area(at, at);
            self.standing_spot(at.x, at.z, at.y)
        });
        let name = &registry.get(event).def.name;
        let Some(spot) = spot else {
            tracing::info!("wandering: no standing room ahead of {feet} or at it for {name}");
            return Err(Words::key("command.wander.no_room"));
        };
        // Seen or not: the admin asked for it. Cast by the world's count of rolls, never the
        // tick.
        let seed = self.meta.roll_seed(crate::save::ROLL_WANDER);
        if self.begin_event_at(player, event, spot, seed, true).is_none() {
            tracing::info!("wandering: no cast of {name} could be brought to {spot}");
            return Err(Words::key("command.wander.no_room"));
        }
        Ok(Words::key("command.wander.started").with("event", Words::key(&registry.get(event).title_key())))
    }

    /// A wandering party member died: the rest of the party may mind (not yet).
    pub(crate) fn wandering_died(&mut self, id: u32) {
        self.wandering.by_creature.remove(&id);
    }
}

#[cfg(test)]
#[path = "wandering_audit.rs"]
mod audit;

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
    fn every_event_comes_to_biomes_the_world_has() {
        let server = fixture();
        let biomes = server.overworld.biomes();
        for (_, event) in server.content.wandering.iter() {
            for (selector, _) in &event.def.biomes {
                let known = match selector.strip_prefix("tag:") {
                    Some(tag) => biomes.iter().any(|(_, b)| b.tags.iter().any(|t| t == tag)),
                    None => biomes.iter().any(|(_, b)| b.name == *selector || b.name.rsplit(':').next() == selector.rsplit(':').next()),
                };
                assert!(known, "{} comes to `{selector}`, which no biome is or carries", event.def.name);
            }
        }
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
        run(&mut server, 1200);
        let gone = leader_at(&server, id).map(|at| flat(at, anchor)).unwrap_or(f32::MAX);
        assert!(gone > 40.0, "and it does march: {gone:.0} blocks from its ring");
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
        assert!(!server.nearby_room(Vec3::new(128.5, 200.0, 128.5), Budget::Animal, cap));
        assert!(!server.cast_fits(&[hound], Vec3::new(128.5, 200.0, 128.5)), "no room for another dog");
    }

    /// Fills the animal room near the player, at the middle of the floor.
    fn fill_animal_room(server: &mut Server) {
        let hound = server.content.creatures.id("base:hound").unwrap();
        for i in 0..server.content.creatures.limits().animals_near {
            server.spawn_creature(hound, Vec3::new(128.5, 200.0, 120.5 + i as f32 * 0.5), i as u64);
        }
    }

    #[test]
    fn a_chance_start_brings_the_whole_party_or_none_of_it() {
        let mut server = fixture();
        // Looking at the sky: nothing is seen appearing.
        server.players.get_mut(&1).unwrap().pitch = std::f32::consts::FRAC_PI_2;
        let walk = event(&server, "stroll_with_dog");
        let spot = server.standing_spot(110, 128, 200).unwrap();
        let id = server.begin_event(Some(1), walk, spot, 7).expect("room for both: a walk");
        assert_eq!(server.wandering.active[&id].members.len(), 2);
        server.wandering.active.clear();
        server.wandering.by_creature.clear();
        server.creatures.live.clear();
        fill_animal_room(&mut server);
        let before = server.creatures.live.len();
        assert!(server.begin_event(Some(1), walk, spot, 7).is_none(), "no room for the dog: no walk");
        assert_eq!(server.creatures.live.len(), before, "and not its person alone either");
        assert!(server.wandering.active.is_empty());
    }

    #[test]
    fn an_admins_start_brings_the_whole_party_past_the_spawn_budgets() {
        let mut server = fixture();
        fill_animal_room(&mut server);
        let id = start(&mut server, "stroll_with_dog", (110, 128));
        let kinds: Vec<Kind> = server.wandering.active[&id].members.iter().map(|m| server.content.creatures.get(server.creatures.live[&m.creature].species).def.kind).collect();
        assert_eq!(kinds, [Kind::Villager, Kind::Animal], "the person and the dog");
    }

    /// An admin on a pad high over the ground, the land forty blocks ahead out of reach:
    /// the party comes nearer, on the pad, rather than being refused.
    #[test]
    fn an_admins_start_on_a_pad_in_the_air_brings_the_party_to_the_pad() {
        let mut server = fixture();
        let stone = server.content.blocks.expect("base:stone");
        let pad: Vec<(IVec3, BlockId)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (IVec3::new(128 + x, 239, 128 + z), stone))).collect();
        server.set_blocks(&pad);
        let player = server.players.get_mut(&1).unwrap();
        (player.position, player.yaw, player.pitch) = (Vec3::new(128.5, 240.0, 128.5), 0.0, 0.0);
        let reply = server.wander_command(Source::Player(1), &["start".into(), "tinker_camp".into()]);
        assert!(reply.is_ok(), "the start was refused: {reply:?}");
        let (_, instance) = server.wandering.active.iter().next().expect("a party");
        for member in &instance.members {
            let at = server.creatures.live[&member.creature].body.position;
            assert!(at.y >= 240.0 && (at - Vec3::new(128.5, at.y, 128.5)).length() < 8.0, "a member stands at {at}, not on the pad");
        }
    }

    /// `/wander hold` keeps parties from setting out by chance, `/wander allow` lets them
    /// again; an admin's own start still comes while they are held.
    #[test]
    fn held_parties_set_out_only_when_an_admin_starts_one() {
        let mut server = fixture();
        assert!(server.wandering_welcome(1), "the fixture's player could meet a party");
        server.wander_command(Source::Player(1), &["hold".into()]).unwrap();
        assert!(!server.wandering_welcome(1), "held: none by chance");
        start(&mut server, "stroll_with_dog", (110, 128));
        server.wander_command(Source::Player(1), &["end".into()]).unwrap();
        server.wander_command(Source::Player(1), &["allow".into()]).unwrap();
        server.wandering.quiet_until.clear();
        assert!(server.wandering_welcome(1), "allowed again");
    }

    /// Clears the world of parties and creatures between tries on one server.
    fn clear(server: &mut Server) {
        server.wandering.active.clear();
        server.wandering.by_creature.clear();
        server.creatures.live.clear();
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.clear(|_| true);
        (player.home, player.position) = (None, Vec3::new(128.5, 200.0, 128.5));
    }

    fn members_of(server: &Server, id: u64, group: usize) -> Vec<u32> {
        server.wandering.active[&id].members.iter().filter(|m| m.group == group).map(|m| m.creature).collect()
    }

    fn put_next_to(server: &mut Server, id: u32, offset: Vec3) {
        let at = server.players[&1].position + offset;
        server.creatures.live.get_mut(&id).unwrap().body.position = at;
    }

    /// Raises skill `name` to `level`.
    fn skill_to(server: &mut Server, name: &str, level: u8) {
        let skill = server.content.skills.id(name).unwrap();
        let xp = server.content.skills.xp_for(level);
        server.players.get_mut(&1).unwrap().skills.grant(skill, xp);
    }

    #[test]
    fn a_peaceful_party_stops_for_a_player_who_comes_near_and_goes_on() {
        let mut server = fixture();
        let id = start(&mut server, "pedlar_and_hound", (68, 128));
        run(&mut server, 200);
        let pedlar = members_of(&server, id, 0)[0];
        let spot = server.creatures.live[&pedlar].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(spot.x + 12.0, 200.0, spot.z);
        // The party walks up to the player; the pedlar halts when it is within six blocks.
        run(&mut server, 400);
        let met = server.creatures.live[&pedlar].body.position;
        assert!(flat(met, server.players[&1].position) <= 8.0, "it came up to the player: {:.1}", flat(met, server.players[&1].position));
        run(&mut server, 100);
        let held = server.creatures.live[&pedlar].body.position;
        assert!(flat(met, held) < 2.0, "it stands for the player: moved {:.1}", flat(met, held));
        // After its stay (35 seconds at most) it goes on, even with the player still there.
        run(&mut server, 20 * 40);
        let later = server.creatures.live[&pedlar].body.position;
        assert!(flat(held, later) > 6.0, "and goes on: moved {:.1}", flat(held, later));
    }

    #[test]
    fn striking_a_peaceful_party_scatters_it_and_it_will_not_talk() {
        let mut server = fixture();
        let id = start(&mut server, "stroll_with_dog", (68, 128));
        let person = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:bone", 1);
        server.wandering_hurt(person, 1);
        let inst = &server.wandering.active[&id];
        assert_eq!(inst.provoked, Some(1));
        assert!(matches!(inst.plan, Plan::March { pace, .. } if pace > 1.0), "it hurries away");
        assert!(server.creatures.live[&person].mind.fear > 0.0, "and is afraid");
        assert!(!server.meet(1, person), "a person just struck says nothing");
        assert_eq!(count_of(&server, "base:bone"), 1);
    }

    #[test]
    fn striking_a_convoys_jailer_turns_the_jailers_on_the_player_and_the_prisoner_runs() {
        let mut server = fixture();
        let id = start(&mut server, "prison_convoy", (68, 128));
        let jailers = members_of(&server, id, 0).into_iter().chain(members_of(&server, id, 1)).collect::<Vec<_>>();
        let captive = members_of(&server, id, 2)[0];
        // Before a blow they leave the player be, however near.
        put_next_to(&mut server, jailers[0], Vec3::new(2.0, 0.0, 0.0));
        assert!(server.creatures.live[&jailers[0]].mind.anger == 0.0);
        server.wandering_hurt(jailers[0], 1);
        assert!(matches!(server.wandering.active[&id].plan, Plan::Avenge { player: 1, .. }));
        run(&mut server, 10);
        for &j in &jailers {
            assert!(server.creatures.live[&j].mind.anger > 0.0, "a jailer is angry");
        }
        assert!(server.creatures.live[&captive].mind.fear > 0.0, "the prisoner is afraid");
        assert!(server.creatures.live[&captive].mind.anger == 0.0);
    }

    #[test]
    fn killing_the_jailers_frees_the_prisoner_who_thanks_and_follows_or_goes_alone() {
        let (mut follows, mut alone) = (0, 0);
        let mut server = fixture();
        for seed in 0..12u64 {
            clear(&mut server);
            let event = event(&server, "prison_convoy");
            let spot = server.standing_spot(68, 128, 200).unwrap();
            let id = server.begin_event_at(Some(1), event, spot, seed * 7919 + 3, true).unwrap();
            let captive = members_of(&server, id, 2)[0];
            let jailers: Vec<u32> = members_of(&server, id, 0).into_iter().chain(members_of(&server, id, 1)).collect();
            for &j in &jailers {
                server.wandering_hurt(j, 1);
            }
            for &j in &jailers {
                server.creature_dies(j);
            }
            run(&mut server, 40);
            let coins = count_of(&server, "base:gold_coin");
            assert!((1..=3).contains(&coins), "the thanks is a few coins: {coins}");
            assert!(server.creatures.live.contains_key(&captive), "the prisoner lives");
            match server.wandering.by_creature.get(&captive) {
                Some(&(i, _)) => {
                    assert!(matches!(server.wandering.active[&i].plan, Plan::Follow { player: 1, .. }));
                    follows += 1;
                }
                None => alone += 1,
            }
        }
        assert!(follows > 0 && alone > 0, "both happen: {follows} follow, {alone} go alone");
    }

    /// Whether a freed captive follows is drawn by its living freer's own count of captives
    /// freed: the same freeings later go the same way, each counts on, and a freeing by
    /// nobody, or by a freer who is dead, draws nothing.
    #[test]
    fn a_freed_captive_follows_by_the_freers_count_not_the_tick() {
        let freed = |tick: u64| {
            let mut server = fixture();
            server.players.get_mut(&1).unwrap().draw_key = crate::player::player_draw_key(0x0d1a, "Tëster");
            server.tick = tick;
            let mut followed = Vec::new();
            for seed in 0..12u64 {
                clear(&mut server);
                let event = event(&server, "prison_convoy");
                let spot = server.standing_spot(68, 128, 200).unwrap();
                let id = server.begin_event_at(Some(1), event, spot, seed * 7919 + 3, true).unwrap();
                let captive = members_of(&server, id, 2)[0];
                server.free_in_test(id);
                followed.push(server.wandering.by_creature.get(&captive).is_some_and(|&(i, _)| matches!(server.wandering.active[&i].plan, Plan::Follow { player: 1, .. })));
                server.tick += 37;
            }
            let frees = server.players[&1].draws.frees;
            // Freed by nobody, and by a freer who is dead: nothing drawn.
            for by in [None, Some(1)] {
                clear(&mut server);
                if by.is_some() {
                    server.players.get_mut(&1).unwrap().health.damage(u8::MAX);
                }
                let event = event(&server, "prison_convoy");
                let spot = server.standing_spot(68, 128, 200).unwrap();
                let id = server.begin_event_at(Some(1), event, spot, 5, true).unwrap();
                let mut inst = server.wandering.active.remove(&id).unwrap();
                server.free_captive(&mut inst, by);
                server.keep(inst);
            }
            (followed, frees, server.players[&1].draws.frees)
        };
        let first = freed(1_000);
        assert_eq!(first.1, 12, "each freeing by a living player counted on");
        assert_eq!(first.2, first.1, "a freeing by nobody or by the dead drew");
        assert!(first.0.iter().any(|&f| f) && first.0.iter().any(|&f| !f), "twelve freed captives all went alike: {:?}", first.0);
        assert_eq!(freed(1_377), first, "the same freeings later went otherwise");
    }

    impl Server {
        /// Frees the captive of party `id` as if player 1 had.
        fn free_in_test(&mut self, id: u64) {
            let mut inst = self.wandering.active.remove(&id).unwrap();
            self.free_captive(&mut inst, Some(1));
            self.keep(inst);
        }
    }

    /// Frees a prisoner who follows player 1 and walks them to a home at `home`, where they
    /// settle; the prisoner's creature id.
    fn settled_prisoner(server: &mut Server, home: Vec3) -> u32 {
        for seed in 0..40u64 {
            // Whoever settled before stays.
            let companions: Vec<_> = std::mem::take(&mut server.creatures.live).into_iter().filter(|(_, c)| c.companion).collect();
            clear(server);
            server.creatures.live.extend(companions);
            let event = event(server, "prison_convoy");
            let spot = server.standing_spot(100, 128, 200).unwrap();
            let id = server.begin_event_at(Some(1), event, spot, seed, true).unwrap();
            let captive = members_of(server, id, 2)[0];
            server.free_in_test(id);
            if !server.wandering.by_creature.contains_key(&captive) {
                continue;
            }
            // The player has a home a little way off; the prisoner walks with them and stays.
            let player = server.players.get_mut(&1).unwrap();
            (player.bed, player.position) = (Some(home), home);
            run(server, 20 * 120);
            return captive;
        }
        panic!("some seed has the prisoner follow");
    }

    #[test]
    fn a_freed_prisoner_follows_the_player_and_settles_at_their_home() {
        let home = Vec3::new(150.5, 200.0, 128.5);
        let mut server = fixture();
        let captive = settled_prisoner(&mut server, home);
        assert!(server.creatures.live.contains_key(&captive));
        assert!(!server.wandering.by_creature.contains_key(&captive), "it is a person of the land now");
        let at = server.creatures.live[&captive].body.position;
        assert!(flat(at, home) < 20.0, "and it is by the player's home: {at}");
        assert!(server.creatures.live[&captive].companion, "a companion, who is saved");
    }

    #[test]
    fn a_prisoner_let_go_elsewhere_is_no_companion() {
        let mut server = fixture();
        for seed in 0..40u64 {
            clear(&mut server);
            let event = event(&server, "prison_convoy");
            let spot = server.standing_spot(100, 128, 200).unwrap();
            let id = server.begin_event_at(Some(1), event, spot, seed, true).unwrap();
            let captive = members_of(&server, id, 2)[0];
            server.free_in_test(id);
            if !server.wandering.by_creature.contains_key(&captive) {
                continue;
            }
            // The player has no home to walk them to: the prisoner follows a while, then goes.
            run(&mut server, 20 * 700);
            assert!(!server.wandering.by_creature.contains_key(&captive), "let go");
            assert!(!server.creatures.live[&captive].companion);
            assert!(server.saved_companions().is_empty(), "so nobody is saved");
            return;
        }
        panic!("some seed has the prisoner follow");
    }

    #[test]
    fn a_settled_prisoner_survives_a_restart() {
        let home = Vec3::new(150.5, 200.0, 128.5);
        let mut server = fixture();
        let captive = settled_prisoner(&mut server, home);
        let creature = server.creatures.live.get_mut(&captive).unwrap();
        (creature.health, creature.rolls) = (3, 600);
        let (species, at) = (server.creatures.live[&captive].species, server.creatures.live[&captive].body.position);
        let saved = server.saved_companions();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].rolls, 600);
        // Through the file, into a world that has just opened.
        let dir = std::env::temp_dir().join(format!("voxmar-companions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let save = crate::save::WorldSave::new(dir.clone());
        save.write_companions(&saved).unwrap();
        let state = save.read_state().unwrap();
        assert_eq!(state.companions, saved);
        let mut reopened = fixture();
        clear(&mut reopened);
        reopened.restore_companions(state.companions);
        let (id, back) = reopened.creatures.live.iter().find(|(_, c)| c.companion).expect("the prisoner is back");
        assert_eq!(back.species, species);
        assert_eq!(back.health, 3, "as wounded as it was");
        assert_eq!(back.rolls, 600, "it thinks on from where it was, not over again");
        assert!(flat(back.body.position, at) < 0.01, "where it stood");
        assert!(!reopened.wandering.manages(*id));
        assert_eq!(reopened.saved_companions(), saved, "and is saved again the same");
        std::fs::remove_dir_all(&dir).unwrap();
        // A file from before rolls were counted reads, and starts its count at none.
        let text = ron::ser::to_string(&saved).unwrap();
        let older = text.replace(",rolls:600", "");
        assert_ne!(older, text, "the field is in the text: {text}");
        let read: Vec<SavedCompanion> = ron::from_str(&older).unwrap();
        assert_eq!(read[0].rolls, 0);
    }

    #[test]
    fn a_companion_stays_when_every_player_is_far() {
        let mut server = fixture();
        let captive = settled_prisoner(&mut server, Vec3::new(150.5, 200.0, 128.5));
        server.players.get_mut(&1).unwrap().position = Vec3::new(-5000.0, 200.0, 0.0);
        server.despawn_far();
        assert!(server.creatures.live.contains_key(&captive), "far from every player, it is still there");
        server.creatures.live.get_mut(&captive).unwrap().companion = false;
        server.despawn_far();
        assert!(!server.creatures.live.contains_key(&captive), "while a wanderer let go is despawned like any");
    }

    #[test]
    fn only_so_many_companions_stay() {
        let mut server = fixture();
        let most = server.content.wandering.rules().companions_most;
        let home = Vec3::new(150.5, 200.0, 128.5);
        let first = settled_prisoner(&mut server, home);
        // The rest of the world's companions are already there.
        let species = server.creatures.live[&first].species;
        for i in 1..most {
            let id = server.spawn_creature(species, Vec3::new(120.5 + i as f32, 200.0, 140.5), i as u64).unwrap();
            server.creatures.live.get_mut(&id).unwrap().companion = true;
        }
        assert_eq!(server.creatures.live.values().filter(|c| c.companion).count(), most);
        let last = settled_prisoner(&mut server, home);
        assert!(!server.creatures.live.get(&last).is_some_and(|c| c.companion), "the world is full of companions");
    }

    #[test]
    fn companions_waiting_for_their_species_keep_their_places() {
        let mut server = fixture();
        let most = server.content.wandering.rules().companions_most;
        let gone = |n: u64| SavedCompanion { species: "lostmod:ghost".into(), seed: n, position: [128.5, 200.0, 140.5], yaw: 0.0, health: 5, rolls: 0 };
        server.restore_companions((0..most as u64).map(gone).collect());
        assert_eq!(server.wandering.waiting.len(), most);
        let led = settled_prisoner(&mut server, Vec3::new(150.5, 200.0, 128.5));
        assert!(!server.creatures.live[&led].companion, "every place is kept for those who will come back");
        assert_eq!(server.saved_companions().len(), most);
    }

    #[test]
    fn a_party_beyond_despawn_range_stays_until_it_leaves() {
        let mut server = fixture();
        let id = start(&mut server, "pedlar_and_hound", (68, 128));
        let members = members_of(&server, id, 0).into_iter().chain(members_of(&server, id, 1)).collect::<Vec<_>>();
        // Further than the land's own creatures are kept, nearer than the party's `leave`.
        server.players.get_mut(&1).unwrap().position = leader_at(&server, id).unwrap() + Vec3::new(140.0, 0.0, 0.0);
        server.despawn_far();
        assert!(members.iter().all(|m| server.creatures.live.contains_key(m)), "the party is not despawned as if wild");
    }

    #[test]
    fn the_next_jailer_leads_when_the_first_is_killed() {
        let mut server = fixture();
        let id = start(&mut server, "prison_convoy", (68, 128));
        let lead = members_of(&server, id, 0)[0];
        let flank = members_of(&server, id, 1)[0];
        server.creatures.live.remove(&lead);
        server.wandering_died(lead);
        server.update_event(id);
        assert_eq!(server.wandering.active[&id].members[0].creature, flank, "a jailer leads, not the prisoner");
    }

    #[test]
    fn a_hound_whose_pedlar_is_killed_is_let_go() {
        let mut server = fixture();
        let id = start(&mut server, "pedlar_and_hound", (68, 128));
        let (pedlar, hound) = (members_of(&server, id, 0)[0], members_of(&server, id, 1)[0]);
        server.creatures.live.remove(&pedlar);
        server.wandering_died(pedlar);
        server.update_event(id);
        assert!(!server.wandering.active.contains_key(&id), "nobody leads it now");
        assert!(server.creatures.live.contains_key(&hound) && !server.wandering.manages(hound), "the hound is an animal of the land");
    }

    #[test]
    fn a_speaker_is_heard_only_within_speak_reach() {
        let mut server = fixture();
        let id = start(&mut server, "pedlar_and_hound", (68, 128));
        let pedlar = members_of(&server, id, 0)[0];
        let reach = server.content.wandering.rules().speak_reach;
        put_next_to(&mut server, pedlar, Vec3::new(reach + 2.0, 0.0, 0.0));
        assert!(!server.within_speech(1, pedlar));
        put_next_to(&mut server, pedlar, Vec3::new(reach - 1.0, 0.0, 0.0));
        assert!(server.within_speech(1, pedlar));
    }

    #[test]
    fn no_bribe_is_taken_once_the_prisoner_is_gone() {
        let mut server = fixture();
        let id = start(&mut server, "prison_convoy", (68, 128));
        let jailer = members_of(&server, id, 0)[0];
        let captive = members_of(&server, id, 2)[0];
        // Killed just now: the party has not yet counted its members again.
        server.creatures.live.remove(&captive);
        server.wandering_died(captive);
        give_held(&mut server, "base:gold_coin", 3);
        assert!(server.meet(1, jailer), "the jailer still talks");
        assert_eq!(count_of(&server, "base:gold_coin"), 3, "but takes no coins for nobody");
    }

    #[test]
    fn parties_together_never_bring_more_people_than_the_budget() {
        let mut server = fixture();
        let most = server.content.wandering.rules().people_most;
        let pilgrims = event(&server, "pilgrims");
        // Chance starts (an admin's would bring them whatever the budget), with the player
        // looking at the sky so that none is seen appearing.
        server.players.get_mut(&1).unwrap().pitch = std::f32::consts::FRAC_PI_2;
        for seed in 0..40u64 {
            let spot = server.standing_spot(20 + (seed as i32 % 8) * 28, 20 + (seed as i32 / 8) * 40, 200).unwrap();
            server.begin_event(None, pilgrims, spot, seed);
        }
        assert!(server.wandering.people(&server) > 0, "some came");
        assert!(server.wandering.people(&server) <= most, "{} people, {most} allowed", server.wandering.people(&server));
    }

    #[test]
    fn a_freed_prisoner_is_a_wanderer_and_never_one_of_a_settlements_people() {
        let mut server = fixture();
        let id = start(&mut server, "prison_convoy", (68, 128));
        let captive = members_of(&server, id, 2)[0];
        server.free_in_test(id);
        assert!(server.creatures.live[&captive].wanderer, "it came with a party, so the settlements' cap never counts it");
    }

    #[test]
    fn a_freed_prisoner_follows_on_a_clock_of_its_own() {
        let mut server = fixture();
        let lifetime = server.content.wandering.get(event(&server, "prison_convoy")).def.lifetime;
        for seed in 0..40u64 {
            clear(&mut server);
            let event = event(&server, "prison_convoy");
            let spot = server.standing_spot(100, 128, 200).unwrap();
            let id = server.begin_event_at(Some(1), event, spot, seed, true).unwrap();
            let captive = members_of(&server, id, 2)[0];
            // Its jailers are killed late in the convoy's life; the player freed it.
            for jailer in members_of(&server, id, 0).into_iter().chain(members_of(&server, id, 1)) {
                server.creatures.live.remove(&jailer);
                server.wandering_died(jailer);
            }
            server.tick += ticks(lifetime - 5.0);
            server.wandering.active.get_mut(&id).unwrap().last_blow = Some(1);
            server.update_event(id);
            if !server.wandering.by_creature.contains_key(&captive) {
                continue;
            }
            run(&mut server, 20 * 30);
            assert!(server.creatures.live.contains_key(&captive), "the convoy's end does not take it away mid-walk");
            assert!(server.wandering.by_creature.contains_key(&captive), "and it still follows");
            return;
        }
        panic!("some seed has the prisoner follow");
    }

    #[test]
    fn three_coins_buy_the_prisoners_freedom_without_a_fight() {
        let mut server = fixture();
        let id = start(&mut server, "prison_convoy", (68, 128));
        let jailer = members_of(&server, id, 0)[0];
        let captive = members_of(&server, id, 2)[0];
        give_held(&mut server, "base:gold_coin", 3);
        assert!(server.meet(1, jailer));
        assert_eq!(count_of(&server, "base:gold_coin") >= 1 && count_of(&server, "base:gold_coin") <= 3, true, "three paid, one to three given back");
        assert!(server.wandering.active[&id].provoked.is_none(), "the jailers mind nothing");
        assert!(server.wandering.active[&id].members.iter().all(|m| m.creature != captive), "the prisoner is out of the party");
        assert!(server.creatures.live.contains_key(&captive));
    }

    #[test]
    fn a_knife_cuts_the_ropes_and_the_jailers_notice() {
        let mut server = fixture();
        let id = start(&mut server, "prison_convoy", (68, 128));
        let captive = members_of(&server, id, 2)[0];
        let jailer = members_of(&server, id, 0)[0];
        // Without a knife there is nothing to do but listen.
        give_held(&mut server, "base:bone", 1);
        assert!(server.meet(1, captive));
        assert!(server.wandering.active[&id].members.iter().any(|m| m.creature == captive), "still bound");
        give_held(&mut server, "base:flint_knife", 1);
        assert!(server.meet(1, captive));
        assert!(server.wandering.active[&id].members.iter().all(|m| m.creature != captive), "free");
        assert_eq!(server.wandering.active[&id].provoked, Some(1), "and the jailers saw");
        assert!(matches!(server.wandering.active[&id].plan, Plan::Avenge { .. }));
        run(&mut server, 10);
        assert!(server.creatures.live[&jailer].mind.anger > 0.0);
        assert_eq!(count_of(&server, "base:flint_knife"), 1, "the knife is kept");
    }

    #[test]
    fn a_tutor_teaches_a_beginner_and_sends_the_skilled_on() {
        let mut server = fixture();
        let id = start(&mut server, "mage_tutors", (68, 128));
        let mage = members_of(&server, id, 0)[0];
        let magic = server.content.skills.id("core:magic").unwrap();
        skill_to(&mut server, "core:magic", 20);
        let before = server.players[&1].skills.xp(magic);
        give_held(&mut server, "base:aether_mote", 1);
        assert!(server.meet(1, mage));
        assert_eq!(server.players[&1].skills.xp(magic), before, "one mote teaches nothing at level 20");
        assert_eq!(count_of(&server, "base:aether_mote"), 1, "and is kept");
        give_held(&mut server, "base:aether_mote", 3);
        assert!(server.meet(1, mage));
        assert!(server.players[&1].skills.xp(magic) > before, "three motes buy the second step");
        assert_eq!(count_of(&server, "base:aether_mote"), 0);
        skill_to(&mut server, "core:magic", 40);
        let high = server.players[&1].skills.xp(magic);
        give_held(&mut server, "base:aether_mote", 3);
        assert!(server.meet(1, mage));
        assert_eq!(server.players[&1].skills.xp(magic), high, "a master learns nothing here");
        assert_eq!(count_of(&server, "base:aether_mote"), 3);
    }

    #[test]
    fn far_parties_come_only_far_from_where_the_world_began() {
        let server = fixture();
        let tutors = server.content.wandering.get(event(&server, "mage_tutors"));
        assert!(!tutors.suits_distance(50.0) && tutors.suits_distance(400.0));
        let dog = server.content.wandering.get(event(&server, "stroll_with_dog"));
        assert!(dog.suits_distance(0.0), "a walk with a dog is anywhere");
    }

    #[test]
    fn a_flock_drifts_together_with_its_shepherd_and_gives_a_tip_for_any_food() {
        let mut server = fixture();
        // Out of the shepherd's sight so it keeps to its flock.
        server.players.get_mut(&1).unwrap().position = Vec3::new(60.5, 200.0, 60.5);
        let id = start(&mut server, "shepherd_and_flock", (128, 128));
        let shepherd = members_of(&server, id, 0)[0];
        let begun = server.creatures.live[&shepherd].body.position;
        run(&mut server, 1200);
        let at = server.creatures.live[&shepherd].body.position;
        assert!(flat(begun, at) > 6.0, "the flock drifts on: moved {:.1}", flat(begun, at));
        for sheep in members_of(&server, id, 1) {
            let d = flat(server.creatures.live[&sheep].body.position, at);
            assert!(d < 20.0, "a sheep strays {d:.0} blocks from the shepherd");
        }
        // Any food teaches a beginner, once; the tip is gated on Farming, and so is nothing else.
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + 2.0, 200.0, at.z);
        let farming = server.content.skills.id("core:farming").unwrap();
        let before = server.players[&1].skills.xp(farming);
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, shepherd));
        assert!(server.players[&1].skills.xp(farming) > before, "a crust of bread teaches a beginner");
        assert_eq!(count_of(&server, "base:bread"), 0);
        let after = server.players[&1].skills.xp(farming);
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, shepherd));
        assert_eq!(server.players[&1].skills.xp(farming), after, "once to each player");
    }

    #[test]
    fn a_lost_child_wanders_its_spot_and_follows_whoever_feeds_it_until_it_arrives() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().position = Vec3::new(60.5, 200.0, 60.5);
        let id = start(&mut server, "lost_child", (128, 128));
        let child = members_of(&server, id, 0)[0];
        let anchor = server.creatures.live[&child].body.position;
        let mut farthest: f32 = 0.0;
        for _ in 0..24 {
            run(&mut server, 100);
            farthest = farthest.max(flat(server.creatures.live[&child].body.position, anchor));
        }
        assert!((3.0..=20.0).contains(&farthest), "it roams its spot, no further: {farthest:.0}");
        // Fed, it follows: the player walks off and the child comes after.
        let near = server.creatures.live[&child].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(near.x + 2.0, 200.0, near.z);
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, child));
        let party = server.wandering.by_creature[&child].0;
        assert!(matches!(server.wandering.active[&party].plan, Plan::Follow { guided: true, .. }));
        server.players.get_mut(&1).unwrap().position = Vec3::new(near.x + 40.0, 200.0, near.z);
        run(&mut server, 20 * 40);
        let walked = flat(server.creatures.live[&child].body.position, server.players[&1].position);
        assert!(walked < 6.0, "it kept up: {walked:.1} blocks behind");
        // At journey's end (here, the player's home) the family pays and the child stays.
        let player = server.players.get_mut(&1).unwrap();
        player.bed = Some(player.position);
        let party = server.wandering.by_creature[&child].0;
        if let Plan::Follow { arrive, .. } = &mut server.wandering.active.get_mut(&party).unwrap().plan {
            *arrive = Arrival::Home;
        }
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        run(&mut server, 40);
        assert!(!server.wandering.by_creature.contains_key(&child), "it has arrived and stays");
        assert!(count_of(&server, "base:gold_coin") >= 1, "the family paid");
    }

    #[test]
    fn a_child_fed_late_in_its_wandering_follows_on_a_clock_of_its_own() {
        let mut server = fixture();
        let id = start(&mut server, "lost_child", (110, 128));
        let child = members_of(&server, id, 0)[0];
        let lifetime = server.content.wandering.get(event(&server, "lost_child")).def.lifetime;
        server.tick += ticks(lifetime - 5.0);
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, child));
        run(&mut server, 20 * 30);
        assert!(server.creatures.live.contains_key(&child), "the wandering's end does not take it away mid-walk");
        assert!(server.wandering.by_creature.contains_key(&child), "and it still follows");
    }

    #[test]
    fn a_flock_whose_shepherd_is_killed_is_let_go() {
        let mut server = fixture();
        let id = start(&mut server, "shepherd_and_flock", (68, 128));
        let shepherd = members_of(&server, id, 0)[0];
        let flock: Vec<u32> = server.wandering.active[&id].members.iter().skip(1).map(|m| m.creature).collect();
        server.creatures.live.remove(&shepherd);
        server.wandering_died(shepherd);
        server.update_event(id);
        assert!(!server.wandering.active.contains_key(&id), "no sheep leads the march");
        assert!(flock.iter().all(|s| !server.wandering.manages(*s)), "the flock are animals of the land");
    }

    #[test]
    fn striking_a_guided_child_sends_it_running_lost_again() {
        let mut server = fixture();
        let id = start(&mut server, "lost_child", (110, 128));
        let child = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, child));
        server.wandering_hurt(child, 1);
        let party = server.wandering.by_creature[&child].0;
        assert!(matches!(server.wandering.active[&party].plan, Plan::March { pace, .. } if pace > 1.0), "it runs");
        assert!(!server.meet(1, child));
    }

    #[test]
    fn a_stray_hound_is_tamed_with_meat_by_a_hunter_and_leaves_its_party() {
        let mut server = fixture();
        let id = start(&mut server, "stray_hound", (120, 128));
        let hound = members_of(&server, id, 0)[0];
        put_next_to(&mut server, hound, Vec3::new(1.5, 0.0, 0.0));
        skill_to(&mut server, "core:hunter", 2);
        for _ in 0..12 {
            give_held(&mut server, "base:raw_small_game", 1);
            server.tend(1, hound);
            if server.creatures.live[&hound].owner.is_some() {
                break;
            }
        }
        assert!(server.creatures.live[&hound].owner.is_some(), "meat and patience tame it");
        run(&mut server, 40);
        assert!(!server.wandering.by_creature.contains_key(&hound), "a kept hound is out of the party");
        assert!(!server.wandering.active.contains_key(&id), "and the party with no one left is gone");
        assert!(server.creatures.live.contains_key(&hound), "but the hound stays, kept");
    }

    #[test]
    fn a_hurt_hunter_shares_a_trick_for_food_and_has_nothing_more_to_teach_the_skilled() {
        let mut server = fixture();
        let id = start(&mut server, "wounded_hunter", (120, 128));
        let hunter = members_of(&server, id, 0)[0];
        let skill = server.content.skills.id("core:hunter").unwrap();
        let before = server.players[&1].skills.xp(skill);
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, hunter));
        assert!(server.players[&1].skills.xp(skill) > before);
        assert!(count_of(&server, "base:flint") >= 2, "and the flints");
        skill_to(&mut server, "core:hunter", 15);
        let high = server.players[&1].skills.xp(skill);
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, hunter));
        assert_eq!(server.players[&1].skills.xp(skill), high, "a skilled tracker learns nothing here");
        assert_eq!(count_of(&server, "base:bread"), 1);
    }

    /// Runs until `done` holds of the instance, up to `seconds` of the game's time.
    fn run_until(server: &mut Server, id: u64, seconds: u64, done: impl Fn(&Instance) -> bool) -> bool {
        for _ in 0..seconds {
            run(server, 20);
            if server.wandering.active.get(&id).is_some_and(&done) {
                return true;
            }
        }
        false
    }

    #[test]
    fn a_chase_runs_the_thief_down_holds_him_and_marches_on() {
        let mut server = fixture();
        let id = start(&mut server, "thief_chase", (60, 128));
        assert!(matches!(server.wandering.active[&id].plan, Plan::Chase { .. }));
        let thief = members_of(&server, id, 0)[0];
        let constables = members_of(&server, id, 1);
        assert!(!constables.is_empty());
        // The player strikes the thief as he runs: the watch do not mind, and keep the count.
        server.wandering_hurt(thief, 1);
        assert!(server.wandering.active[&id].provoked.is_none(), "striking the quarry turns no one on the player");
        assert!(server.wandering.active[&id].struck.contains(&(1, 0)));
        assert!(run_until(&mut server, id, 240, |i| i.stage == Stage::Caught), "the watch catch him");
        assert!(matches!(server.wandering.active[&id].plan, Plan::Held { .. }));
        let at = server.creatures.live[&thief].body.position;
        for &c in &constables {
            assert!(flat(server.creatures.live[&c].body.position, at) < 12.0, "the watch stand about him");
        }
        // Held: the thief pleads, and the constable pays whoever helped, once.
        let coins = count_of(&server, "base:gold_coin");
        assert!(server.meet(1, thief));
        assert!(server.meet(1, constables[0]));
        assert!(count_of(&server, "base:gold_coin") > coins, "the bounty for the one who struck him");
        let gold = count_of(&server, "base:gold_coin");
        assert!(server.meet(1, constables[0]));
        assert_eq!(count_of(&server, "base:gold_coin"), gold, "and only once: after that, the plain word");
        // After the hold they march him off.
        assert!(run_until(&mut server, id, 60, |i| i.stage == Stage::Passing && matches!(i.plan, Plan::March { .. })), "the hold ends");
    }

    #[test]
    fn a_thief_struck_by_a_player_beside_him_runs_on_into_the_watch() {
        let mut server = fixture();
        let id = start(&mut server, "thief_chase", (60, 128));
        let thief = members_of(&server, id, 0)[0];
        // A real blow, from beside him as he runs past: his fear is the chase's, not a bolt
        // of his own.
        run(&mut server, 20);
        let at = server.creatures.live[&thief].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(1.5, 0.0, 0.0);
        server.hurt_creature(1, thief, 1, None, voxmar_engine::combat::Blow::Plain);
        assert!(run_until(&mut server, id, 240, |i| i.stage == Stage::Caught), "the watch still catch him");
        // Walking up to him while he is held does not set him running.
        let held = server.creatures.live[&thief].body.position;
        server.players.get_mut(&1).unwrap().position = held + Vec3::new(1.5, 0.0, 0.0);
        run(&mut server, 20 * 4);
        assert!(flat(server.creatures.live[&thief].body.position, held) < 4.0, "he stays held");
        let constable = members_of(&server, id, 1)[0];
        assert!(server.meet(1, constable));
        assert!(count_of(&server, "base:gold_coin") > 0, "and the bounty is paid");
    }

    #[test]
    fn a_stranger_who_never_struck_the_thief_gets_no_bounty() {
        let mut server = fixture();
        let id = start(&mut server, "thief_chase", (60, 128));
        let constable = members_of(&server, id, 1)[0];
        assert!(run_until(&mut server, id, 240, |i| i.stage == Stage::Caught));
        assert!(server.meet(1, constable));
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
    }

    #[test]
    fn a_dead_thief_ends_the_chase_and_the_watch_go_home() {
        let mut server = fixture();
        let id = start(&mut server, "thief_chase", (60, 128));
        let thief = members_of(&server, id, 0)[0];
        server.creature_dies(thief);
        run(&mut server, 20);
        assert!(matches!(server.wandering.active[&id].plan, Plan::March { .. }), "no quarry, no chase");
        assert_ne!(server.wandering.active[&id].stage, Stage::Caught);
    }

    #[test]
    fn striking_the_watch_makes_them_fight_back_and_the_thief_runs_free() {
        let mut server = fixture();
        let id = start(&mut server, "thief_chase", (60, 128));
        let constable = members_of(&server, id, 1)[0];
        server.wandering_hurt(constable, 1);
        assert!(server.wandering.active[&id].provoked.is_none(), "the party as a whole does not turn: only the one struck fights");
        assert!(!server.wandering.active[&id].struck.contains(&(1, 0)));
        assert!(server.wandering.active[&id].struck.contains(&(1, 1)));
    }

    fn walk_to_waiting(server: &mut Server) -> u64 {
        let id = start(server, "wounded_traveller", (60, 128));
        assert!(matches!(server.wandering.active[&id].plan, Plan::Lie { .. }));
        // The player stands far off while the party settles.
        assert!(run_until(server, id, 300, |i| i.stage == Stage::Waiting), "the bait settles and the trap is set");
        id
    }

    fn near_bait(server: &mut Server, id: u64, blocks: f32) {
        let bait = server.wandering.active[&id].members[0].creature;
        let at = server.creatures.live[&bait].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + blocks, at.y, at.z);
    }

    #[test]
    fn an_ambush_waits_until_a_player_comes_near_then_springs_on_them() {
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        let raiders = members_of(&server, id, 1);
        assert!(raiders.len() >= 2);
        // A while with the player far: nothing happens.
        run(&mut server, 20 * 20);
        assert_eq!(server.wandering.active[&id].stage, Stage::Waiting);
        // Near: it springs after the delay (6 to 10 seconds), not at once.
        near_bait(&mut server, id, 5.0);
        run(&mut server, 20 * 2);
        assert_eq!(server.wandering.active[&id].stage, Stage::Waiting, "a moment to see it coming");
        assert!(run_until(&mut server, id, 15, |i| i.stage == Stage::Sprung), "and then it springs");
        assert!(matches!(server.wandering.active[&id].plan, Plan::Avenge { player: 1, .. }));
        run(&mut server, 10);
        for &r in &raiders {
            assert!(server.creatures.live[&r].mind.anger > 0.0, "a raider is angry");
        }
        let bait = server.wandering.active[&id].members[0].creature;
        assert!(server.creatures.live[&bait].mind.fear > 0.0, "the bait is afraid");
    }

    #[test]
    fn beating_the_ambushers_wins_the_baits_thanks_once() {
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        let bait = server.wandering.active[&id].members[0].creature;
        let raiders = members_of(&server, id, 1);
        near_bait(&mut server, id, 5.0);
        assert!(run_until(&mut server, id, 15, |i| i.stage == Stage::Sprung));
        for &r in &raiders {
            server.creature_dies(r);
        }
        assert!(run_until(&mut server, id, 10, |i| i.stage == Stage::Won), "every lurker gone is a win");
        assert!(matches!(server.wandering.active[&id].plan, Plan::Held { .. }));
        let defence = server.content.skills.id("core:defence").unwrap();
        let xp = server.players[&1].skills.xp(defence);
        assert!(server.meet(1, bait));
        assert!(count_of(&server, "base:gold_coin") >= 2);
        assert!(server.players[&1].skills.xp(defence) > xp);
        let gold = count_of(&server, "base:gold_coin");
        assert!(server.meet(1, bait));
        assert_eq!(count_of(&server, "base:gold_coin"), gold, "thanked once");
    }

    #[test]
    fn three_coins_to_a_lurker_before_it_springs_buy_the_road() {
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        let raider = members_of(&server, id, 1)[0];
        give_held(&mut server, "base:gold_coin", 3);
        near_bait(&mut server, id, 5.0);
        run(&mut server, 20 * 2);
        assert!(server.meet(1, raider));
        assert_eq!(count_of(&server, "base:gold_coin"), 0, "paid");
        let inst = &server.wandering.active[&id];
        assert!(matches!(inst.plan, Plan::March { .. }), "they go");
        assert_eq!(inst.stage, Stage::Passing);
        assert!(inst.provoked.is_none());
        run(&mut server, 20 * 30);
        assert!(server.creatures.live[&raider].mind.anger == 0.0, "and no grudge");
    }

    #[test]
    fn a_seasoned_fighter_with_a_weapon_is_let_by_and_the_ambushers_give_up() {
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        skill_to(&mut server, "core:attack", 25);
        skill_to(&mut server, "core:defence", 25);
        skill_to(&mut server, "core:hitpoints", 25);
        let level = server.combat_level(1);
        assert!(level >= 14, "combat level {level} is a seasoned fighter");
        give_held(&mut server, "base:stone_sword", 1);
        near_bait(&mut server, id, 5.0);
        run(&mut server, 20 * 15);
        assert_eq!(server.wandering.active[&id].stage, Stage::Waiting, "armed and seasoned: no spring");
        assert!(run_until(&mut server, id, 15, |i| matches!(i.plan, Plan::March { .. })), "they give it up");
        let inst = &server.wandering.active[&id];
        let Plan::March { goal, .. } = inst.plan else { unreachable!() };
        let lead = server.creatures.live[&inst.members[0].creature].body.position;
        assert!((110.0..=130.0).contains(&flat(goal, lead)), "they go the script's `away` (120 blocks), not a fixed march");
        // The same player with empty hands would have been sprung on.
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        skill_to(&mut server, "core:attack", 25);
        skill_to(&mut server, "core:defence", 25);
        skill_to(&mut server, "core:hitpoints", 25);
        near_bait(&mut server, id, 5.0);
        assert!(run_until(&mut server, id, 15, |i| i.stage == Stage::Sprung), "nothing in the hand: not feared");
    }

    #[test]
    fn the_bait_lies_still_when_a_player_walks_right_up_to_it() {
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        let bait = server.wandering.active[&id].members[0].creature;
        let at = server.creatures.live[&bait].body.position;
        near_bait(&mut server, id, 2.0);
        run(&mut server, 20 * 3);
        assert_eq!(server.wandering.active[&id].stage, Stage::Waiting);
        assert!(flat(at, server.creatures.live[&bait].body.position) < 1.0, "lying hurt is its errand: it does not bolt from a stranger");
    }

    #[test]
    fn a_beginner_with_a_weapon_is_not_feared() {
        let mut server = fixture();
        let id = walk_to_waiting(&mut server);
        give_held(&mut server, "base:stone_sword", 1);
        near_bait(&mut server, id, 5.0);
        assert!(run_until(&mut server, id, 15, |i| i.stage == Stage::Sprung));
    }

    /// Moves the player to `behind` blocks behind the lure's leader along its heading.
    fn follow(server: &mut Server, id: u64, behind: f32) {
        let inst = &server.wandering.active[&id];
        let Plan::Lead { heading, .. } = inst.plan else { return };
        let lead = server.creatures.live[&inst.members[0].creature].body.position;
        let at = lead - Vec3::new(heading.x, 0.0, heading.y) * behind;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x, 200.0, at.z);
    }

    #[test]
    fn a_lure_waits_for_a_follower_leads_them_the_whole_way_and_shows_its_gift() {
        let mut server = fixture();
        // The floor is 256 blocks across: the stag is east of the player, so its path (130
        // blocks, away from them) stays on it.
        server.players.get_mut(&1).unwrap().position = Vec3::new(200.5, 200.0, 128.5);
        let id = start(&mut server, "white_stag", (170, 128));
        let stag = server.wandering.active[&id].members[0].creature;
        let first = server.creatures.live[&stag].body.position;
        // Nobody near: it stands and watches.
        run(&mut server, 20 * 10);
        assert!(flat(first, server.creatures.live[&stag].body.position) < 3.0, "it waits");
        assert_eq!(server.wandering.active[&id].stage, Stage::Passing);
        let hunter = server.content.skills.id("core:hunter").unwrap();
        let xp = server.players[&1].skills.xp(hunter);
        // A follower seven blocks behind, near enough to scare a wild stag: it walks on, the whole way.
        for _ in 0..400 {
            follow(&mut server, id, 7.0);
            run(&mut server, 20);
            if server.wandering.active.get(&id).is_none_or(|i| i.stage == Stage::Arrived) {
                break;
            }
        }
        let inst = &server.wandering.active[&id];
        assert_eq!(inst.stage, Stage::Arrived, "it led all the way");
        assert!(flat(server.creatures.live[&stag].body.position, first) > 100.0, "a long way");
        assert!(count_of(&server, "base:antler") >= 1, "the antler");
        assert!(server.players[&1].skills.xp(hunter) > xp);
        // And once it has shown its gift, it goes.
        run(&mut server, 20 * 10);
        assert!(!server.wandering.active.contains_key(&id) && !server.creatures.live.contains_key(&stag), "gone without a death");
    }

    #[test]
    fn a_struck_stag_bolts_on_its_own_fear_not_at_a_walk() {
        let mut server = fixture();
        let id = start(&mut server, "white_stag", (60, 128));
        let stag = server.wandering.active[&id].members[0].creature;
        run(&mut server, 20);
        assert!(server.wandering.active[&id].members[0].errand.is_some_and(|e| e.steady), "leading: the script's walk");
        server.wandering_hurt(stag, 1);
        run(&mut server, 20);
        let inst = &server.wandering.active[&id];
        assert!(matches!(inst.plan, Plan::March { .. }), "it scatters");
        assert!(inst.members[0].errand.is_some_and(|e| !e.steady), "and its fear may run it");
    }

    #[test]
    fn a_lure_with_no_follower_gives_up() {
        let mut server = fixture();
        let id = start(&mut server, "white_stag", (60, 128));
        assert!(run_until(&mut server, id, 400, |i| matches!(i.plan, Plan::March { .. })), "after its patience it walks away");
        assert_eq!(count_of(&server, "base:antler"), 0);
    }

    #[test]
    fn a_wisp_hangs_back_from_a_torch_and_leads_again_when_it_is_put_away() {
        let mut server = fixture();
        let id = start(&mut server, "wisp_trail", (100, 128));
        let wisp = server.wandering.active[&id].members[0].creature;
        let near = |server: &mut Server| {
            let at = server.creatures.live[&wisp].body.position;
            server.players.get_mut(&1).unwrap().position = Vec3::new(at.x - 7.0, 200.0, at.z);
        };
        give_held(&mut server, "base:torch", 1);
        near(&mut server);
        run(&mut server, 20 * 6);
        assert_ne!(server.wandering.active[&id].stage, Stage::Leading, "it will not lead past a flame");
        give_held(&mut server, "base:bone", 1);
        near(&mut server);
        assert!(run_until(&mut server, id, 10, |i| i.stage == Stage::Leading), "the torch away, it goes on");
    }

    #[test]
    fn animals_come_to_food_and_keep_from_flame() {
        let mut server = fixture();
        let id = start(&mut server, "stray_hound", (116, 128));
        let hound = server.wandering.active[&id].members[0].creature;
        let distance = |server: &Server| flat(server.creatures.live[&hound].body.position, server.players[&1].position);
        let before = distance(&server);
        give_held(&mut server, "base:cooked_small_game", 1);
        run(&mut server, 20 * 12);
        assert!(distance(&server) < before - 3.0, "it came to the food: {before:.1} -> {:.1}", distance(&server));
        give_held(&mut server, "base:torch", 1);
        let close = distance(&server);
        run(&mut server, 20 * 8);
        assert!(distance(&server) > close + 3.0, "and drew back from the flame: {close:.1} -> {:.1}", distance(&server));
    }

    #[test]
    fn a_farmer_runs_a_fox_down_and_pays_eggs_to_whoever_turned_it() {
        let mut server = fixture();
        let id = start(&mut server, "fox_and_farmer", (60, 128));
        let fox = members_of(&server, id, 0)[0];
        let farmer = members_of(&server, id, 1)[0];
        server.wandering_hurt(fox, 1);
        assert!(run_until(&mut server, id, 240, |i| i.stage == Stage::Caught), "the farmer gets the fox");
        assert!(server.meet(1, farmer));
        assert!(count_of(&server, "base:egg") >= 3);
        assert!(!server.meet(1, fox), "the fox has nothing to say");
    }

    // Batch 4: the early land.

    /// Gives player 1 a fresh connection, and the other end to read what they are told.
    fn listen(server: &mut Server) -> voxmar_protocol::ClientConnection {
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        client
    }

    /// The keys of what player 1 has been told since `listen`, with their arguments.
    fn told(client: &voxmar_protocol::ClientConnection) -> Vec<(String, Vec<(String, Words)>)> {
        std::iter::from_fn(|| client.try_recv().ok().flatten())
            .filter_map(|message| match message {
                ServerMessage::Chat { text: Words::Key(key, args), .. } => Some((key, args)),
                _ => None,
            })
            .collect()
    }

    fn place(server: &mut Server, name: &str, at: IVec3) {
        let block = server.content.blocks.expect(name);
        server.world.set_block(at, block);
    }

    /// Moves player 1 to `to` in steps over `seconds`, as a walk.
    fn walk_player(server: &mut Server, to: Vec3, seconds: u64) {
        let from = server.players[&1].position;
        for step in 1..=seconds * 4 {
            let t = step as f32 / (seconds * 4) as f32;
            server.players.get_mut(&1).unwrap().position = from.lerp(to, t);
            run(server, 5);
        }
    }

    #[test]
    fn berry_pickers_work_a_bush_each_and_move_on_when_one_is_picked() {
        let mut server = fixture();
        // The player watches from well off, out of the pickers' halt.
        server.players.get_mut(&1).unwrap().position = Vec3::new(100.5, 200.0, 100.5);
        for (x, z) in [(140, 128), (144, 131), (139, 135), (146, 124), (150, 130), (142, 140)] {
            place(&mut server, "base:sweet_berry_bush", IVec3::new(x, 200, z));
        }
        let id = start(&mut server, "berry_pickers", (132, 128));
        assert!(matches!(server.wandering.active[&id].plan, Plan::Gather { .. }));
        assert!(
            run_until(&mut server, id, 60, |i| i.members.iter().all(|m| m.work.is_some_and(|w| w.until.is_some()))),
            "every picker reaches a bush and works it"
        );
        let inst = &server.wandering.active[&id];
        let blocks: FxHashSet<IVec3> = inst.members.iter().filter_map(|m| m.work.map(|w| w.block)).collect();
        assert_eq!(blocks.len(), inst.members.len(), "one bush each");
        for m in &inst.members {
            let at = server.creatures.live[&m.creature].body.position;
            let w = m.work.unwrap();
            assert!(flat(at, w.block.as_vec3() + Vec3::splat(0.5)) < 2.0, "a picker stands at their bush: {at} by {}", w.block);
        }
        // The player picks the bush the leader is working: the leader goes to another.
        let (leader, picked) = (inst.members[0].creature, inst.members[0].work.unwrap().block);
        server.world.set_block(picked, BlockId::AIR);
        run(&mut server, 20);
        let next = server.wandering.active[&id].members.iter().find(|m| m.creature == leader).and_then(|m| m.work).map(|w| w.block);
        assert!(next.is_some_and(|b| b != picked), "the picker moved on: {next:?}");
        // Done with the patch (ten works), they go.
        assert!(run_until(&mut server, id, 400, |i| matches!(i.plan, Plan::March { .. })), "the pickers leave when the patch is done");
        assert!(server.wandering.active[&id].worked >= 6, "they worked the bushes first: {}", server.wandering.active[&id].worked);
    }

    #[test]
    fn a_gathering_party_comes_only_where_there_is_something_to_gather() {
        let mut server = fixture();
        server.day = TICKS_PER_DAY as f64 * 3.4;
        let pickers = event(&server, "berry_pickers");
        let spot = IVec3::new(160, 200, 128);
        for seed in 0..300 {
            assert_ne!(server.pick_event(1, spot, seed), Some(pickers), "no bushes, no berry pickers");
        }
        let seek = server.content.wandering.get(pickers).seek.clone();
        assert!(server.nearest_block(spot.as_vec3(), 20.0, &seek, 0.0, &[]).is_none());
        place(&mut server, "base:sweet_berry_bush", IVec3::new(170, 200, 130));
        assert_eq!(server.nearest_block(spot.as_vec3(), 20.0, &seek, 0.0, &[]), Some(IVec3::new(170, 200, 130)));
    }

    #[test]
    fn pickers_say_where_mushrooms_grow_true_to_the_land_once() {
        let mut server = fixture();
        place(&mut server, "base:sweet_berry_bush", IVec3::new(112, 200, 128));
        let id = start(&mut server, "berry_pickers", (110, 128));
        let picker = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&picker].body.position;
        // A mushroom 30 blocks west and 30 north (north is -z) of the picker.
        place(&mut server, "base:chanterelle", IVec3::new(at.x as i32 - 30, 200, at.z as i32 - 30));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, picker));
        let said = told(&client);
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("a direction is given: {said:?}"));
        let arg = |name: &str| point.1.iter().find(|(n, _)| n == name).map(|(_, w)| w.clone());
        assert_eq!(arg("dir"), Some(Words::key("compass.north_west")));
        assert_eq!(arg("far"), Some(Words::text(40)), "about 42 blocks, said as 40");
        assert_eq!(arg("thing"), Some(Words::Block("base:chanterelle".into())));
        // Once to each player: after that, a greeting.
        assert!(server.meet(1, picker));
        assert!(told(&client).iter().all(|(key, _)| key != "wander.point"));
        // And where there is none, it says so.
        let fungus = voxmar_engine::wander::Want::Tag("fungus".into());
        server.world.set_block(IVec3::new(at.x as i32 - 30, 200, at.z as i32 - 30), BlockId::AIR);
        server.hand_out(1, picker, &[ResolvedReward::Point(fungus.clone(), 96.0)]);
        assert!(told(&client).iter().any(|(key, _)| key == "wander.point_none"));
        // One only a step away is still told of, not taken for none.
        place(&mut server, "base:chanterelle", IVec3::new(at.x as i32 + 3, 200, at.z as i32));
        server.hand_out(1, picker, &[ResolvedReward::Point(fungus, 96.0)]);
        let said = told(&client);
        assert!(said.iter().any(|(key, _)| key == "wander.point"), "a near one is pointed to: {said:?}");
    }

    /// The cartographer marks a locked chest left under rock nearby, true to where it is,
    /// once; where none lies near, the line is not said.
    #[test]
    fn a_cartographer_marks_a_locked_chest_nearby_once() {
        let mut server = fixture();
        let id = start(&mut server, "cartographer", (110, 128));
        let mapper = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&mapper].body.position;
        // A chest 40 blocks east of the cartographer, under a stone roof.
        let chest = IVec3::new(at.x as i32 + 40, 200, at.z as i32);
        place(&mut server, "base:nomad_treasure_chest", chest);
        place(&mut server, "base:stone", chest + IVec3::new(0, 3, 0));
        let client = listen(&mut server);
        assert!(server.meet(1, mapper));
        let said = told(&client);
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("a direction is given: {said:?}"));
        let arg = |name: &str| point.1.iter().find(|(n, _)| n == name).map(|(_, w)| w.clone());
        assert_eq!(arg("dir"), Some(Words::key("compass.east")));
        assert_eq!(arg("thing"), Some(Words::Block("base:nomad_treasure_chest".into())));
        let says_chest = |said: &[(String, Vec<(String, Words)>)]| said.iter().any(|(key, args)| key == "wander.said" && args.iter().any(|(_, w)| *w == Words::key("wander.base.cartographer.chest")));
        assert!(says_chest(&said), "{said:?}");
        // Once to each player.
        assert!(server.meet(1, mapper));
        assert!(!says_chest(&told(&client)));
        // Opened, it is a plain chest: nothing locked to point to.
        server.world.set_block(chest, server.content.blocks.expect("base:chest"));
        let locked = voxmar_engine::wander::Want::Tag("locked".into());
        server.hand_out(1, mapper, &[ResolvedReward::Point(locked, 128.0)]);
        assert!(told(&client).iter().any(|(key, _)| key == "wander.point_none"));
    }

    #[test]
    fn eight_berries_buy_a_loaf_from_the_pickers() {
        let mut server = fixture();
        place(&mut server, "base:sweet_berry_bush", IVec3::new(112, 200, 128));
        let id = start(&mut server, "berry_pickers", (110, 128));
        let picker = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:sweet_berry", 9);
        assert!(server.meet(1, picker));
        assert_eq!((count_of(&server, "base:sweet_berry"), count_of(&server, "base:bread")), (1, 1));
    }

    #[test]
    fn fishers_stand_at_the_waters_edge_and_help_a_beginner_to_a_rod() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().position = Vec3::new(100.5, 200.0, 100.5);
        // A pond to the east.
        for x in 150..158 {
            for z in 124..132 {
                place(&mut server, "base:water", IVec3::new(x, 199, z));
            }
        }
        let id = start(&mut server, "fisher_folk", (138, 128));
        assert!(run_until(&mut server, id, 60, |i| i.members[0].work.is_some_and(|w| w.until.is_some())), "the fisher reaches the water");
        let fisher = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&fisher].body.position;
        assert!((148.0..=159.0).contains(&at.x) && (122.0..=134.0).contains(&at.z), "at the pond's edge: {at}");
        assert!(!server.is_wet(at.floor().as_ivec3()), "on dry ground");
        assert_eq!(server.creatures.live[&fisher].decision.pose, Pose::Crouched, "fishing");
        // Three sticks from a beginner buy line for a rod, once.
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(2.0, 0.0, 0.0);
        give_held(&mut server, "base:stick", 6);
        assert!(server.meet(1, fisher));
        assert_eq!((count_of(&server, "base:stick"), count_of(&server, "base:twine")), (3, 2));
        assert!(server.meet(1, fisher));
        assert_eq!(count_of(&server, "base:stick"), 3, "once to each player");
        // With a rod in hand, a lesson.
        let fishing = server.content.skills.id("core:fishing").unwrap();
        give_held(&mut server, "base:fishing_rod", 1);
        assert!(server.meet(1, fisher));
        assert!(server.players[&1].skills.xp(fishing) > 0);
        assert_eq!(count_of(&server, "base:fishing_rod"), 1, "the rod is shown, not taken");
    }

    /// A sow and her litter, the player `blocks` east of the sow.
    fn sounder(server: &mut Server, blocks: f32) -> (u64, u32, Vec<u32>) {
        let id = start(server, "boar_sounder", (100, 128));
        let sow = members_of(server, id, 0)[0];
        let piglets = members_of(server, id, 1);
        let at = server.creatures.live[&sow].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + blocks, 200.0, at.z);
        (id, sow, piglets)
    }

    #[test]
    fn a_sow_warns_a_player_off_then_charges_one_who_stays() {
        let mut server = fixture();
        let (id, sow, piglets) = sounder(&mut server, 30.0);
        assert!(piglets.len() >= 2);
        for p in &piglets {
            assert!(server.creatures.live[p].grows_up.is_some(), "the litter is young");
            assert!(server.creatures.live[p].scale < server.creatures.live[&sow].scale, "and small");
        }
        run(&mut server, 40);
        assert!(server.wandering.active[&id].warned.is_none(), "far off, no warning");
        let client = listen(&mut server);
        let at = server.creatures.live[&sow].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + 7.0, 200.0, at.z);
        run(&mut server, 20);
        assert_eq!(server.wandering.active[&id].warned.map(|w| w.0), Some(1), "warned");
        assert!(server.wandering.active[&id].provoked.is_none(), "not yet charged");
        assert!(told(&client).iter().any(|(key, _)| key == "wander.base.boar_sounder.warn"), "the warning is told");
        // Backing off past `calm` ends it; coming back warns again, and staying is a charge.
        let at = server.creatures.live[&sow].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + 20.0, 200.0, at.z);
        run(&mut server, 20);
        assert!(server.wandering.active[&id].warned.is_none(), "backed off, calm again");
        let at = server.creatures.live[&sow].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + 7.0, 200.0, at.z);
        assert!(run_until(&mut server, id, 6, |i| i.provoked == Some(1)), "stayed, and she charges");
        run(&mut server, 10);
        assert!(server.creatures.live[&sow].mind.anger > 0.0, "the sow is angry");
        assert!(piglets.iter().filter_map(|p| server.creatures.live.get(p)).all(|p| p.mind.fear > 0.0), "the piglets run");
    }

    #[test]
    fn coming_close_to_the_litter_is_a_charge_at_once() {
        let mut server = fixture();
        let (id, _, _) = sounder(&mut server, 3.0);
        run(&mut server, 20);
        assert_eq!(server.wandering.active[&id].provoked, Some(1));
    }

    #[test]
    fn a_torch_or_food_in_hand_keeps_the_sow_from_charging() {
        for held in ["base:torch", "base:bread"] {
            let mut server = fixture();
            let (id, _, _) = sounder(&mut server, 6.0);
            give_held(&mut server, held, 1);
            run(&mut server, 20 * 6);
            let inst = &server.wandering.active[&id];
            assert!(inst.provoked.is_none() && inst.warned.is_none(), "{held}: no warning, no charge");
        }
    }

    #[test]
    fn a_torch_held_by_one_player_does_not_shield_another() {
        let mut server = fixture();
        let (id, sow, _) = sounder(&mut server, 4.0);
        give_held(&mut server, "base:torch", 1);
        // A second player, empty-handed, six blocks off on the other side.
        let at = server.creatures.live[&sow].body.position;
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let mut other = crate::player::Player::new(2, connection, Vec3::new(at.x - 6.0, 200.0, at.z), 2, server.tick, server.survival.max_health, &server.config.limits);
        other.joined = true;
        server.players.insert(2, other);
        run(&mut server, 20);
        assert_eq!(server.wandering.active[&id].warned.map(|w| w.0), Some(2), "the one without a torch is warned");
        assert!(run_until(&mut server, id, 6, |i| i.provoked == Some(2)), "and charged for staying");
    }

    /// A herd of deer settled where it began, the player `blocks` east of its stag.
    fn herd(server: &mut Server, blocks: f32) -> (u64, Vec3) {
        server.players.get_mut(&1).unwrap().position = Vec3::new(30.5, 200.0, 30.5);
        let id = start(server, "deer_herd", (128, 128));
        let stag = members_of(server, id, 0)[0];
        let at = server.creatures.live[&stag].body.position;
        server.players.get_mut(&1).unwrap().position = Vec3::new(at.x + blocks, 200.0, at.z);
        (id, at)
    }

    #[test]
    fn deer_bolt_from_a_player_who_walks_up() {
        let mut server = fixture();
        let (id, at) = herd(&mut server, 40.0);
        run(&mut server, 20);
        assert!(!server.wandering.active[&id].startled);
        walk_player(&mut server, Vec3::new(at.x + 12.0, 200.0, at.z), 6);
        assert!(server.wandering.active[&id].startled, "walked up to within reach, the herd bolts");
        assert!(matches!(server.wandering.active[&id].plan, Plan::March { pace, .. } if pace == BOLT_PACE));
        let stag = members_of(&server, id, 0)[0];
        assert!(server.creatures.live[&stag].mind.fear > 0.0);
    }

    #[test]
    fn a_still_player_is_let_near_and_a_still_hand_with_food_nearer() {
        let mut server = fixture();
        let (id, _) = herd(&mut server, 15.0);
        // Standing still fifteen blocks from the stag, well within a walker's 22: let be.
        run(&mut server, 20 * 4);
        assert!(!server.wandering.active[&id].startled, "a still player is let be");
        // A still hand holding food is let nearer still.
        let mut server = fixture();
        let (id, _) = herd(&mut server, 3.0);
        give_held(&mut server, "base:bread", 1);
        run(&mut server, 20 * 4);
        assert!(!server.wandering.active[&id].startled, "food held out, standing still, three blocks off");
        // Then a step: they bolt.
        let at = server.players[&1].position;
        walk_player(&mut server, at + Vec3::new(3.0, 0.0, 0.0), 1);
        assert!(server.wandering.active[&id].startled, "a moving hand is no still one");
    }

    #[test]
    fn every_still_hand_with_food_is_let_near_not_only_the_one_the_lead_heeds() {
        let mut server = fixture();
        let (id, at) = herd(&mut server, 3.0);
        give_held(&mut server, "base:bread", 1);
        // A second player on the other side, also still, also holding food out.
        let (client, connection) = voxmar_protocol::local_pair();
        let _keep = client;
        let mut other = crate::player::Player::new(2, connection, Vec3::new(at.x - 5.0, 200.0, at.z), 2, server.tick, server.survival.max_health, &server.config.limits);
        other.joined = true;
        other.selected = 0;
        assert_eq!(other.inventory.add(server.content.blocks.expect("base:bread"), 1), 0);
        server.players.insert(2, other);
        run(&mut server, 20 * 4);
        assert!(!server.wandering.active[&id].startled, "neither still hand with food startles the herd");
    }

    #[test]
    fn a_hunter_stalks_closer_than_a_beginner() {
        // Twenty blocks from the stag: within a beginner's 22, beyond a hunter of 40's 8
        // (22 less 0.35 a level), even counting the hinds a few blocks nearer.
        let near = 20.0;
        let mut beginner = fixture();
        let (id, at) = herd(&mut beginner, 40.0);
        walk_player(&mut beginner, Vec3::new(at.x + near, 200.0, at.z), 3);
        assert!(beginner.wandering.active[&id].startled, "a beginner walking to 20 blocks is seen");
        let mut hunter = fixture();
        skill_to(&mut hunter, "core:hunter", 40);
        let (id, at) = herd(&mut hunter, 40.0);
        walk_player(&mut hunter, Vec3::new(at.x + near, 200.0, at.z), 3);
        assert!(!hunter.wandering.active[&id].startled, "a hunter of 40 comes to 20 blocks unseen");
    }

    #[test]
    fn the_cook_cooks_what_is_handed_over_and_keeps_one() {
        let mut server = fixture();
        let id = start(&mut server, "travelling_cook", (110, 128));
        let cook = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:raw_small_game", 5);
        assert!(server.meet(1, cook));
        assert_eq!((count_of(&server, "base:raw_small_game"), count_of(&server, "base:cooked_small_game")), (0, 4), "five cooked, one kept");
        // One raw is not enough to bother the pot with.
        give_held(&mut server, "base:raw_small_game", 1);
        assert!(server.meet(1, cook));
        assert_eq!(count_of(&server, "base:raw_small_game"), 1);
        // Showing cooked food teaches a beginner, once, and it is not taken.
        let cooking = server.content.skills.id("core:cooking").unwrap();
        give_held(&mut server, "base:cooked_small_game", 1);
        assert!(server.meet(1, cook));
        let xp = server.players[&1].skills.xp(cooking);
        assert!(xp > 0);
        assert!(server.meet(1, cook));
        assert_eq!((server.players[&1].skills.xp(cooking), count_of(&server, "base:cooked_small_game")), (xp, 1));
    }

    #[test]
    fn a_cowherd_sells_the_calf_only_to_one_who_could_keep_it() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(128.5, 200.0, 128.5));
        let id = start(&mut server, "cowherd", (110, 128));
        let herder = members_of(&server, id, 0)[0];
        let calf = members_of(&server, id, 2)[0];
        assert!(server.creatures.live[&calf].grows_up.is_some(), "the calf is young");
        // A beginner with the coins is told no, and keeps them.
        give_held(&mut server, "base:gold_coin", 4);
        assert!(server.meet(1, herder));
        assert_eq!(count_of(&server, "base:gold_coin"), 4);
        assert!(server.creatures.live[&calf].owner.is_none());
        // With the Hunter level to tame an aurochs, four coins buy it.
        skill_to(&mut server, "core:hunter", 12);
        assert!(server.meet(1, herder));
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
        assert_eq!(server.creatures.live[&calf].owner.as_deref(), Some("Tëster"), "the calf is the player's");
        run(&mut server, 20);
        assert!(!server.wandering.by_creature.contains_key(&calf), "and out of the herd");
        assert!(server.creatures.live[&calf].grows_up.is_some(), "still a calf, to grow up kept");
        // The herd has no other calf to sell.
        give_held(&mut server, "base:gold_coin", 4);
        assert!(server.meet(1, herder));
        assert_eq!(count_of(&server, "base:gold_coin"), 4);
    }

    #[test]
    fn a_pail_held_out_is_filled_once() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(128.5, 200.0, 128.5));
        let id = start(&mut server, "cowherd", (110, 128));
        let herder = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:pail", 1);
        assert!(server.meet(1, herder));
        assert_eq!((count_of(&server, "base:pail"), count_of(&server, "base:milk")), (1, 1));
        assert!(server.meet(1, herder));
        assert_eq!(count_of(&server, "base:milk"), 1, "once to each player");
    }

    #[test]
    fn the_herb_gatherer_teaches_a_beginner_and_points_the_knowing_to_roots() {
        let mut server = fixture();
        place(&mut server, "base:yarrow", IVec3::new(112, 200, 128));
        let id = start(&mut server, "herb_gatherer", (110, 128));
        let elder = members_of(&server, id, 0)[0];
        let alchemy = server.content.skills.id("core:alchemy").unwrap();
        give_held(&mut server, "base:yarrow", 1);
        assert!(server.meet(1, elder));
        assert!(server.players[&1].skills.xp(alchemy) > 0, "a herb shown teaches a beginner");
        assert_eq!(count_of(&server, "base:yarrow"), 0);
        // A beginner hears nothing of roots; one with Alchemy 5 is pointed to them.
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, elder));
        assert!(told(&client).iter().all(|(key, _)| !key.starts_with("wander.point")));
        skill_to(&mut server, "core:alchemy", 5);
        let at = server.creatures.live[&elder].body.position;
        place(&mut server, "base:wild_carrot", IVec3::new(at.x as i32 + 40, 200, at.z as i32));
        assert!(server.meet(1, elder));
        let said = told(&client);
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("roots pointed to: {said:?}"));
        assert!(point.1.iter().any(|(n, w)| n == "dir" && *w == Words::key("compass.east")));
    }

    /// The first block with `tag`.
    fn tagged(server: &Server, tag: &str) -> BlockId {
        server.content.blocks.tagged(tag).next().unwrap_or_else(|| panic!("a block tagged {tag}"))
    }

    /// Whether a speaker said the line `key`.
    fn says(said: &[(String, Vec<(String, Words)>)], key: &str) -> bool {
        said.iter().any(|(k, args)| k == "wander.said" && args.iter().any(|(n, w)| n == "text" && *w == Words::key(key)))
    }

    fn has_effect(server: &Server, name: &str) -> bool {
        let effect = server.content.effects.find(name).unwrap();
        server.players[&1].timed_effects.iter().any(|&(e, _)| e == effect)
    }

    /// Wheat sprouts on farmland at y 199 for each x of `xs` along z `z`; returns the sprout.
    fn field(server: &mut Server, xs: std::ops::Range<i32>, z: i32) -> BlockId {
        let seed = server.content.blocks.expect("base:wheat_seeds");
        let wheat = server.content.crops.planted_by(seed).unwrap();
        let sprout = server.content.crops.get(wheat).stages[0];
        for x in xs {
            place(server, "base:farmland", IVec3::new(x, 199, z));
            server.world.set_block(IVec3::new(x, 200, z), sprout);
            server.farm.schedule(IVec3::new(x, 200, z), crate::farm::Planted { crop: wheat, stage: 0, due: u64::MAX - 1, sown: true });
        }
        sprout
    }

    #[test]
    fn water_carriers_come_only_where_water_and_a_field_lie_together() {
        let mut server = fixture();
        server.day = TICKS_PER_DAY as f64 * 3.4;
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(128.5, 200.0, 128.5));
        let carriers = event(&server, "water_carriers");
        let spot = IVec3::new(150, 200, 128);
        place(&mut server, "base:water", IVec3::new(160, 199, 128));
        for seed in 0..300 {
            assert_ne!(server.pick_event(1, spot, seed), Some(carriers), "water, but no field");
        }
        let deliver = server.content.wandering.get(carriers).deliver.clone();
        field(&mut server, 140..142, 128);
        assert_eq!(server.nearest_block(spot.as_vec3(), 24.0, &deliver, 0.0, &[]), Some(IVec3::new(141, 199, 128)));
    }

    #[test]
    fn water_carriers_fill_at_the_water_and_the_field_they_water_grows() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().position = Vec3::new(100.5, 200.0, 100.5);
        for x in 150..156 {
            for z in 124..132 {
                place(&mut server, "base:water", IVec3::new(x, 199, z));
            }
        }
        let sprout = field(&mut server, 132..136, 128);
        let id = start(&mut server, "water_carriers", (142, 128));
        assert!(
            run_until(&mut server, id, 90, |i| i.members.iter().any(|m| m.work.is_some_and(|w| w.carrying))),
            "a carrier fills at the water and sets off"
        );
        let to = server.wandering.active[&id].members.iter().find_map(|m| m.work.filter(|w| w.carrying)).unwrap();
        assert!((131..=136).contains(&to.block.x) && to.block.z == 128, "to the field: {:?}", to.block);
        assert!(run_until(&mut server, id, 90, |i| i.worked >= 1), "and sets it down there");
        let grown = (132..136).filter(|&x| server.block(IVec3::new(x, 200, 128)) != Some(sprout)).count();
        assert!(grown > 0, "the watered crops grow on a stage");
    }

    #[test]
    fn woodcutters_carry_each_load_back_to_their_pile() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().position = Vec3::new(100.5, 200.0, 100.5);
        for y in 200..204 {
            place(&mut server, "base:oak_log", IVec3::new(146, y, 128));
        }
        let id = start(&mut server, "woodcutters", (134, 128));
        let Plan::Gather { anchor } = server.wandering.active[&id].plan else { panic!("gathering") };
        assert!(run_until(&mut server, id, 90, |i| i.members.iter().any(|m| m.work.is_some_and(|w| w.carrying))), "a load is cut");
        let load = server.wandering.active[&id].members.iter().find_map(|m| m.work.filter(|w| w.carrying)).unwrap();
        assert!(flat(load.stand, anchor) < 1.0, "carried back to the pile where they began");
        assert_eq!(server.wandering.active[&id].worked, 0, "counted only once set down");
        assert!(run_until(&mut server, id, 60, |i| i.worked >= 1));
    }

    #[test]
    fn woodcutters_buy_logs_once_teach_the_axe_and_point_to_fruit_trees() {
        let mut server = fixture();
        place(&mut server, "base:oak_log", IVec3::new(112, 200, 128));
        let id = start(&mut server, "woodcutters", (110, 128));
        let cutter = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:oak_log", 10);
        assert!(server.meet(1, cutter));
        assert_eq!((count_of(&server, "base:oak_log"), count_of(&server, "base:gold_coin")), (2, 4));
        give_held(&mut server, "base:oak_log", 10);
        assert!(server.meet(1, cutter));
        assert_eq!(count_of(&server, "base:oak_log"), 10, "once to each player");
        let construction = server.content.skills.id("core:construction").unwrap();
        give_held(&mut server, "base:stone_axe", 1);
        assert!(server.meet(1, cutter));
        assert!(server.players[&1].skills.xp(construction) > 0, "a word on felling");
        // With no fruit tree near, they have nothing to tell of it.
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, cutter));
        assert!(!says(&told(&client), "wander.base.woodcutters.fruit"), "nothing near, nothing said");
        let at = server.creatures.live[&cutter].body.position;
        let leaves = tagged(&server, "fruit_leaves");
        server.world.set_block(IVec3::new(at.x as i32 + 40, 205, at.z as i32), leaves);
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, cutter));
        let said = told(&client);
        assert!(says(&said, "wander.base.woodcutters.fruit"), "{said:?}");
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("fruit trees pointed to: {said:?}"));
        assert!(point.1.iter().any(|(n, w)| n == "dir" && *w == Words::key("compass.east")));
    }

    #[test]
    fn the_harvest_family_trades_the_next_crop_up_by_farming_level() {
        let mut server = fixture();
        let id = start(&mut server, "harvest_family", (110, 128));
        let farmer = members_of(&server, id, 0)[0];
        skill_to(&mut server, "core:farming", 2);
        give_held(&mut server, "base:wheat", 6);
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:wheat"), 6, "rye only for a farmer who can sow it (3)");
        skill_to(&mut server, "core:farming", 3);
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:wheat"), 0);
        assert!((2..=3).contains(&count_of(&server, "base:rye")));
        give_held(&mut server, "base:wheat", 6);
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:wheat"), 6, "once");
        // Rye is not yet carrots below Farming 5.
        give_held(&mut server, "base:rye", 6);
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:rye"), 6);
        skill_to(&mut server, "core:farming", 5);
        assert!(server.meet(1, farmer));
        assert!((2..=3).contains(&count_of(&server, "base:carrot")));
        // A beginner with seed in hand learns to sow.
        let mut server = fixture();
        let id = start(&mut server, "harvest_family", (110, 128));
        let farmer = members_of(&server, id, 0)[0];
        let farming = server.content.skills.id("core:farming").unwrap();
        give_held(&mut server, "base:wheat_seeds", 2);
        assert!(server.meet(1, farmer));
        assert!(server.players[&1].skills.xp(farming) > 0);
        assert_eq!(count_of(&server, "base:wheat_seeds"), 2, "shown, not taken");
    }

    #[test]
    fn charcoal_burners_burn_a_dozen_logs_once_and_point_to_coal() {
        let mut server = fixture();
        let id = start(&mut server, "charcoal_burners", (110, 128));
        let burner = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:oak_log", 12);
        assert!(server.meet(1, burner));
        assert_eq!((count_of(&server, "base:oak_log"), count_of(&server, "base:charcoal")), (0, 2));
        give_held(&mut server, "base:oak_log", 12);
        assert!(server.meet(1, burner));
        assert_eq!(count_of(&server, "base:oak_log"), 12, "once");
        let at = server.creatures.live[&burner].body.position;
        place(&mut server, "base:coal_ore_sample", IVec3::new(at.x as i32, 200, at.z as i32 + 50));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, burner));
        let said = told(&client);
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("coal pointed to: {said:?}"));
        assert!(point.1.iter().any(|(n, w)| n == "dir" && *w == Words::key("compass.south")));
        assert!(point.1.iter().any(|(n, w)| n == "thing" && *w == Words::Block("base:coal_ore_sample".into())));
    }

    #[test]
    fn the_healer_mends_the_hurt_and_three_herbs_buy_a_salve() {
        let mut server = fixture();
        let id = start(&mut server, "wandering_healer", (110, 128));
        let healer = members_of(&server, id, 0)[0];
        let max = server.players[&1].health.max;
        server.players.get_mut(&1).unwrap().health.current = max - 8;
        assert!(server.meet(1, healer));
        assert_eq!(server.players[&1].health.current, max, "mended");
        let herb = tagged(&server, "herb");
        let name = server.content.blocks.def(herb).name.clone();
        give_held(&mut server, &name, 3);
        assert!(server.meet(1, healer));
        assert_eq!(count_of(&server, &name), 0);
        assert!(has_effect(&server, "soothed"), "a salve soothes");
        let alchemy = server.content.skills.id("core:alchemy").unwrap();
        assert!(server.players[&1].skills.xp(alchemy) > 0);
    }

    #[test]
    fn the_story_teller_tells_of_something_strange_and_near_once() {
        let mut server = fixture();
        let id = start(&mut server, "story_teller", (110, 128));
        let teller = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&teller].body.position;
        let strange = tagged(&server, "supernatural");
        server.world.set_block(IVec3::new(at.x as i32 - 60, 200, at.z as i32), strange);
        let client = listen(&mut server);
        assert!(server.meet(1, teller));
        let said = told(&client);
        assert!(says(&said, "wander.base.story_teller.tale"), "{said:?}");
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("pointed to: {said:?}"));
        assert!(point.1.iter().any(|(n, w)| n == "dir" && *w == Words::key("compass.west")));
        assert!(has_effect(&server, "focused"));
        assert!(server.meet(1, teller));
        assert!(told(&client).iter().all(|(key, _)| key != "wander.point"), "once");
    }

    #[test]
    fn each_hermit_asks_for_its_own_thing_and_says_what() {
        let mut server = fixture();
        let hermit_kind = event(&server, "hermit");
        let line = &server.content.wandering.get(hermit_kind).meetings[0].lines[0];
        let picks: std::collections::BTreeSet<_> = (0..64u64).filter_map(|seed| line.ask_of(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15))).collect();
        assert!(picks.len() >= 3, "parties ask different things: {picks:?}");
        let id = start(&mut server, "hermit", (110, 128));
        let hermit = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&hermit].body.position;
        server.world.set_block(IVec3::new(at.x as i32, 200, at.z as i32 + 40), tagged(&server, "wonder"));
        let (item, count) = server.content.wandering.get(hermit_kind).meetings[0].lines[0].ask_of(server.wandering.active[&id].seed).unwrap();
        let name = server.content.blocks.def(item).name.clone();
        // Empty-handed: the riddle, naming the thing and how many.
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, hermit));
        let said = told(&client);
        let ask = said.iter().find_map(|(_, args)| args.iter().find(|(n, _)| n == "text").map(|(_, w)| w.clone())).expect("said");
        assert_eq!(ask, Words::key("wander.base.hermit.riddle_ask").with("thing", Words::Block(name.clone())).with("count", Words::text(count)));
        // Bring it: taken, a wonder pointed to, and Magic, once.
        give_held(&mut server, &name, count + 1);
        assert!(server.meet(1, hermit));
        let said = told(&client);
        assert!(says(&said, "wander.base.hermit.riddle"), "{said:?}");
        assert!(said.iter().any(|(key, _)| key == "wander.point"));
        assert_eq!(count_of(&server, &name), 1);
        let magic = server.content.skills.id("core:magic").unwrap();
        assert!(server.players[&1].skills.xp(magic) > 0);
        assert!(server.meet(1, hermit));
        assert_eq!(count_of(&server, &name), 1, "once");
    }

    #[test]
    fn the_alchemist_wants_its_own_herb_not_any_herb() {
        let mut server = fixture();
        let id = start(&mut server, "travelling_alchemist", (110, 128));
        let alchemist = members_of(&server, id, 0)[0];
        let kind = server.wandering.active[&id].event;
        let (item, count) = server.content.wandering.get(kind).meetings[0].lines[0].ask_of(server.wandering.active[&id].seed).unwrap();
        let wanted = server.content.blocks.def(item).name.clone();
        let other = ["base:lavender", "base:chamomile"].into_iter().find(|n| *n != wanted).unwrap();
        give_held(&mut server, other, 4);
        let client = listen(&mut server);
        assert!(server.meet(1, alchemist));
        let said = told(&client);
        assert!(said.iter().any(|(_, args)| args.iter().any(|(n, w)| n == "text" && *w == Words::key("wander.base.travelling_alchemist.herb_ask").with("thing", Words::Block(wanted.clone())).with("count", Words::text(count)))), "it says which: {said:?}");
        assert_eq!(count_of(&server, other), 4, "another herb is not taken");
        give_held(&mut server, &wanted, count);
        assert!(server.meet(1, alchemist));
        assert_eq!(count_of(&server, &wanted), 0);
        assert!(has_effect(&server, "focused"));
        let alchemy = server.content.skills.id("core:alchemy").unwrap();
        assert!(server.players[&1].skills.xp(alchemy) > 0);
    }

    #[test]
    fn the_goose_girl_trades_feathers_for_seed_and_sells_a_duckling_to_a_hunter() {
        let mut server = fixture();
        let id = start(&mut server, "goose_girl", (110, 128));
        let girl = members_of(&server, id, 0)[0];
        let ducklings = members_of(&server, id, 2);
        assert_eq!(ducklings.len(), 1);
        assert!(server.creatures.live[&ducklings[0]].grows_up.is_some(), "the duckling is young");
        give_held(&mut server, "base:wheat_seeds", 4);
        assert!(server.meet(1, girl));
        assert!((2..=3).contains(&count_of(&server, "base:feather")));
        give_held(&mut server, "base:gold_coin", 2);
        assert!(server.meet(1, girl));
        assert_eq!(count_of(&server, "base:gold_coin"), 2, "not to a beginner (Hunter 3)");
        skill_to(&mut server, "core:hunter", 3);
        assert!(server.meet(1, girl));
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
        assert_eq!(server.creatures.live[&ducklings[0]].owner_id, Some(1), "the duckling is theirs");
    }

    #[test]
    fn the_prospector_works_surface_ore_and_points_the_skilled_to_more() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().position = Vec3::new(100.5, 200.0, 100.5);
        place(&mut server, "base:coal_ore_sample", IVec3::new(146, 200, 128));
        let id = start(&mut server, "prospector", (134, 128));
        assert!(run_until(&mut server, id, 60, |i| i.members[0].work.is_some_and(|w| w.until.is_some())), "at the ore");
        let prospector = members_of(&server, id, 0)[0];
        let mining = server.content.skills.id("core:mining").unwrap();
        give_held(&mut server, "base:stone_pickaxe", 1);
        assert!(server.meet(1, prospector));
        assert!(server.players[&1].skills.xp(mining) > 0, "a lesson for a pickaxe in hand");
        let at = server.creatures.live[&prospector].body.position;
        place(&mut server, "base:iron_ore_sample", IVec3::new(at.x as i32 + 60, 200, at.z as i32));
        skill_to(&mut server, "core:mining", 5);
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, prospector));
        let said = told(&client);
        assert!(says(&said, "wander.base.prospector.vein"), "{said:?}");
        assert!(said.iter().any(|(key, _)| key == "wander.point"));
    }

    #[test]
    fn a_flower_for_the_wedding_returns_bread_and_luck_and_for_the_funeral_a_prayer() {
        let mut server = fixture();
        let flower = tagged(&server, "flower");
        let name = server.content.blocks.def(flower).name.clone();
        let id = start(&mut server, "wedding", (110, 128));
        let priest = members_of(&server, id, 0)[0];
        give_held(&mut server, &name, 1);
        assert!(server.meet(1, priest));
        assert_eq!((count_of(&server, &name), count_of(&server, "base:bread")), (0, 1));
        assert!(has_effect(&server, "refreshed"));
        let mut server = fixture();
        let id = start(&mut server, "funeral", (110, 128));
        let priest = members_of(&server, id, 0)[0];
        // A funeral does not stop for a player walking up.
        let at = server.creatures.live[&priest].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(3.0, 0.0, 0.0);
        run(&mut server, 40);
        assert_eq!(server.wandering.active[&id].halt.0, 0, "the procession walks on");
        give_held(&mut server, &name, 1);
        assert!(server.meet(1, priest));
        let magic = server.content.skills.id("core:magic").unwrap();
        assert!(server.players[&1].skills.xp(magic) > 0);
        assert_eq!(count_of(&server, &name), 0);
    }

    fn xp_in(server: &Server, skill: &str) -> u32 {
        let skill = server.content.skills.id(skill).unwrap();
        server.players[&1].skills.xp(skill)
    }

    #[test]
    fn a_travelling_smith_whets_every_edge_for_a_coin_and_teaches_a_beginner_holding_a_tool() {
        let mut server = fixture();
        let id = start(&mut server, "travelling_smith", (110, 128));
        let smith = members_of(&server, id, 0)[0];
        // Empty-handed: a greeting, nothing given.
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, smith));
        assert!(!has_effect(&server, "keen_edge"));
        // A coin: every edge whetted, once.
        give_held(&mut server, "base:gold_coin", 2);
        assert!(server.meet(1, smith));
        assert!(says(&told(&client), "wander.base.travelling_smith.whet"));
        assert!(has_effect(&server, "keen_edge"));
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        assert!(server.meet(1, smith));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "once");
        // A tool in a beginner's hand: a word on its care, and Crafting.
        let tool = tagged(&server, "tool");
        let name = server.content.blocks.def(tool).name.clone();
        give_held(&mut server, &name, 1);
        assert!(server.meet(1, smith));
        assert!(says(&told(&client), "wander.base.travelling_smith.care"));
        assert!(xp_in(&server, "core:crafting") > 0);
        assert_eq!(count_of(&server, &name), 1, "the tool is not taken");
    }

    #[test]
    fn the_truffle_hunter_tells_a_good_mushroom_and_sells_a_truffle_spot_only_with_one_near() {
        let mut server = fixture();
        let id = start(&mut server, "truffle_hunter", (110, 128));
        let hunter = members_of(&server, id, 0)[0];
        assert_eq!(members_of(&server, id, 1).len(), 1, "a boar on a rope");
        // A mushroom shown to a beginner cook: which are good to eat, and it is kept.
        let fungus = tagged(&server, "fungus");
        let name = server.content.blocks.def(fungus).name.clone();
        give_held(&mut server, &name, 1);
        let client = listen(&mut server);
        assert!(server.meet(1, hunter));
        assert!(says(&told(&client), "wander.base.truffle_hunter.safe"));
        assert!(xp_in(&server, "core:cooking") > 0);
        assert_eq!(count_of(&server, &name), 1);
        // A coin with no truffle near: kept, nothing to tell.
        give_held(&mut server, "base:gold_coin", 1);
        assert!(server.meet(1, hunter));
        assert!(told(&client).iter().all(|(key, _)| key != "wander.point"));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "no secret, no sale");
        // A truffle forty blocks east: the coin buys the way to it.
        let at = server.creatures.live[&hunter].body.position;
        server.world.set_block(IVec3::new(at.x as i32 + 40, 200, at.z as i32), server.content.blocks.expect("base:truffle"));
        assert!(server.meet(1, hunter));
        let said = told(&client);
        let point = said.iter().find(|(key, _)| key == "wander.point").unwrap_or_else(|| panic!("pointed to: {said:?}"));
        assert!(point.1.iter().any(|(n, w)| n == "dir" && *w == Words::key("compass.east")));
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
    }

    #[test]
    fn the_silent_monk_binds_wounds_and_blesses_for_a_mote_of_aether() {
        let mut server = fixture();
        let id = start(&mut server, "wandering_monk", (110, 128));
        let monk = members_of(&server, id, 0)[0];
        let max = server.players[&1].health.max;
        server.players.get_mut(&1).unwrap().health.current = max - 6;
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, monk));
        assert_eq!(server.players[&1].health.current, max, "bound up");
        give_held(&mut server, "base:aether_mote", 1);
        assert!(server.meet(1, monk));
        assert!(has_effect(&server, "focused"));
        assert!(xp_in(&server, "core:magic") > 0);
        assert_eq!(count_of(&server, "base:aether_mote"), 0);
    }

    #[test]
    fn a_refugee_fed_follows_the_player_home_stays_and_gives_the_seed_they_carried() {
        let mut server = fixture();
        let id = start(&mut server, "refugees", (110, 128));
        let refugee = members_of(&server, id, 0)[0];
        let near = server.creatures.live[&refugee].body.position;
        // Empty-handed, they ask; a meal and one of them follows.
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, refugee));
        assert!(server.wandering.by_creature.contains_key(&refugee));
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, refugee));
        assert_eq!(count_of(&server, "base:bread"), 0);
        let party = server.wandering.by_creature[&refugee].0;
        assert!(party != id, "the one who follows leaves the family");
        assert!(matches!(server.wandering.active[&party].plan, Plan::Follow { guided: true, arrive: Arrival::Home, .. }));
        // Fed once is enough: talked to again with food in hand, they keep you company and
        // take nothing more.
        give_held(&mut server, "base:bread", 1);
        let client = listen(&mut server);
        assert!(server.meet(1, refugee));
        assert!(says(&told(&client), "wander.base.refugees.company"));
        assert_eq!(count_of(&server, "base:bread"), 1, "no second meal is taken");
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        // Home: they stay as one of the player's people, and the seed is the player's.
        let home = Vec3::new(near.x + 30.0, 200.0, near.z);
        let player = server.players.get_mut(&1).unwrap();
        (player.position, player.bed) = (home, Some(home));
        for _ in 0..60 {
            run(&mut server, 20);
            if !server.wandering.by_creature.contains_key(&refugee) {
                break;
            }
        }
        assert!(!server.wandering.by_creature.contains_key(&refugee), "it has arrived");
        assert!(server.creatures.live[&refugee].companion, "it stays at the player's home");
        assert!(count_of(&server, "base:seed_mix") >= 4);
        assert!(xp_in(&server, "core:farming") > 0);
    }

    /// Starts a camping party and runs it until it has settled at its spot; returns its
    /// leader, and puts player 1 `off` blocks east of it.
    fn camped(server: &mut Server, name: &str, off: f32) -> (u64, u32) {
        let id = start(server, name, (110, 128));
        assert!(run_until(server, id, 60, |inst| matches!(inst.plan, Plan::Camp { stay_until: Some(_), .. })), "the party settles");
        let lead = members_of(server, id, 0)[0];
        let at = server.creatures.live[&lead].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(off, 0.0, 0.0);
        (id, lead)
    }

    #[test]
    fn a_puppet_show_watched_through_refreshes_once_and_walking_off_starts_it_again() {
        let mut server = fixture();
        let (id, lead) = camped(&mut server, "puppeteers", 4.0);
        let at = server.creatures.live[&lead].body.position;
        let client = listen(&mut server);
        // Most of the play, then off: nothing.
        run(&mut server, 15 * TICKS_PER_SECOND as u64);
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(30.0, 0.0, 0.0);
        run(&mut server, 2 * TICKS_PER_SECOND as u64);
        assert!(!has_effect(&server, "refreshed"), "half a play is no play");
        // Back, and the whole of it: Refreshed, with the show's end.
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(4.0, 0.0, 0.0);
        run(&mut server, 15 * TICKS_PER_SECOND as u64);
        assert!(!has_effect(&server, "refreshed"), "the count began again");
        run(&mut server, 7 * TICKS_PER_SECOND as u64);
        assert!(has_effect(&server, "refreshed"));
        assert!(says(&told(&client), "wander.base.puppeteers.show"));
        assert!(server.wandering.active[&id].shown.contains(&1));
        // Once.
        run(&mut server, 25 * TICKS_PER_SECOND as u64);
        assert!(!says(&told(&client), "wander.base.puppeteers.show"), "the show is given once");
        // A coin in the hat.
        give_held(&mut server, "base:gold_coin", 1);
        assert!(server.meet(1, lead));
        assert!(has_effect(&server, "focused"));
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
    }

    #[test]
    fn a_struck_troupe_puts_on_no_show() {
        let mut server = fixture();
        let (id, lead) = camped(&mut server, "puppeteers", 4.0);
        server.wandering_hurt(lead, 1);
        run(&mut server, 25 * TICKS_PER_SECOND as u64);
        assert!(!has_effect(&server, "refreshed"));
        assert!(server.wandering.active.get(&id).is_none_or(|inst| !inst.shown.contains(&1)));
    }

    #[test]
    fn standing_in_a_dance_circle_warms_and_mends_and_a_round_trains_hitpoints() {
        let mut server = fixture();
        let (_, lead) = camped(&mut server, "dance_circle", 3.0);
        let max = server.players[&1].health.max;
        server.players.get_mut(&1).unwrap().health.current = max - 8;
        run(&mut server, 17 * TICKS_PER_SECOND as u64);
        assert!(has_effect(&server, "ember_warmed"));
        assert!(server.players[&1].health.current >= max - 4, "mended by the dance");
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, lead));
        assert!(xp_in(&server, "core:hitpoints") > 0);
    }

    #[test]
    fn the_cartographer_names_the_nearest_town_only_when_one_is_near() {
        let mut server = fixture();
        let id = start(&mut server, "cartographer", (110, 128));
        let drawer = members_of(&server, id, 0)[0];
        let towns = server.overworld.settlements_near(IVec2::new(128, 128), 6000);
        let town = towns.first().expect("a settlement in the world's plan");
        let centre = town.centre.as_vec3();
        // Far from every town, and a seasoned builder: nothing to tell.
        let lonely = (1..200)
            .map(|i| centre + Vec3::new(i as f32 * 400.0, 0.0, 0.0))
            .find(|&p| server.town_near(p, 600.0).is_none())
            .expect("somewhere far from towns");
        server.creatures.live.get_mut(&drawer).unwrap().body.position = lonely;
        skill_to(&mut server, "core:construction", 20);
        let client = listen(&mut server);
        assert!(server.meet(1, drawer));
        assert!(told(&client).iter().all(|(key, _)| key != "wander.town"), "no town near");
        // Two hundred blocks east of the town: its name, west, about 200.
        server.creatures.live.get_mut(&drawer).unwrap().body.position = centre + Vec3::new(200.0, 0.0, 0.0);
        let (to, nearest) = server.town_near(centre + Vec3::new(200.0, 0.0, 0.0), 600.0).unwrap();
        assert!(server.meet(1, drawer));
        let said = told(&client);
        let named = said.iter().find(|(key, _)| key == "wander.town").unwrap_or_else(|| panic!("named: {said:?}"));
        assert!(named.1.iter().any(|(n, w)| n == "name" && *w == Words::Name(vec![("en".into(), nearest.name.0.clone()), ("ja".into(), nearest.name.1.clone())])));
        assert!(named.1.iter().any(|(n, w)| n == "dir" && *w == crate::commands::compass(to)));
        assert!(server.meet(1, drawer));
        assert!(told(&client).iter().all(|(key, _)| key != "wander.town"), "once");
    }

    #[test]
    fn a_cartographer_teaches_a_beginner_to_pace_out_ground() {
        let mut server = fixture();
        let id = start(&mut server, "cartographer", (110, 128));
        let drawer = members_of(&server, id, 0)[0];
        // Far from every town, so the lesson is what is said.
        let lonely = (1..400).map(|i| Vec3::new(128.0 + i as f32 * 400.0, 200.0, 128.0)).find(|&p| server.town_near(p, 600.0).is_none()).unwrap();
        server.creatures.live.get_mut(&drawer).unwrap().body.position = lonely;
        assert!(server.meet(1, drawer));
        assert!(xp_in(&server, "core:construction") > 0);
    }

    #[test]
    fn whoever_reaches_the_scarecrow_finds_seed_in_its_pockets_once() {
        let mut server = fixture();
        let id = start(&mut server, "scarecrow", (110, 128));
        let scarecrow = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, scarecrow));
        let seed = count_of(&server, "base:seed_mix");
        assert!((3..=5).contains(&seed), "{seed} seed");
        assert!(xp_in(&server, "core:farming") > 0);
        assert!(server.meet(1, scarecrow));
        assert_eq!(count_of(&server, "base:seed_mix"), seed, "once");
    }

    /// A wild creature of `name` at `at`.
    fn wild(server: &mut Server, name: &str, at: Vec3, salt: u64) -> u32 {
        let species = server.content.creatures.id(name).unwrap();
        server.spawn_creature(species, at, salt).expect("room for it")
    }

    fn loot_falling(server: &Server) -> usize {
        server.creatures.falling_loot.len()
    }

    #[test]
    fn a_hunting_party_runs_down_the_nearest_boar_and_keeps_what_nobody_else_struck() {
        let mut server = fixture();
        let boar = wild(&mut server, "base:wild_boar", Vec3::new(100.5, 200.0, 150.5), 1);
        let id = start(&mut server, "hunting_party", (110, 128));
        let client = listen(&mut server);
        assert!(run_until(&mut server, id, 120, |inst| inst.stage == Stage::Took), "the hunt brings the boar down");
        assert!(!server.creatures.live.contains_key(&boar), "the boar is down");
        assert_eq!(loot_falling(&server), 0, "the hunters keep the whole of what they took themselves");
        assert!(!says(&told(&client), "wander.base.hunting_party.share"));
        // With nothing more in reach, they go.
        assert!(run_until(&mut server, id, 20, |inst| matches!(inst.plan, Plan::March { .. })), "nothing left to hunt");
        assert_eq!(server.wandering.active[&id].stage, Stage::Took, "and still say what they took");
    }

    #[test]
    fn whoever_drew_first_blood_on_a_hunts_kill_is_given_a_share() {
        let mut server = fixture();
        let boar = wild(&mut server, "base:wild_boar", Vec3::new(100.5, 200.0, 150.5), 1);
        server.creatures.live.get_mut(&boar).unwrap().hurt_by = Some(1);
        let id = start(&mut server, "hunting_party", (110, 128));
        let client = listen(&mut server);
        assert!(run_until(&mut server, id, 120, |inst| inst.stage == Stage::Took));
        assert!(says(&told(&client), "wander.base.hunting_party.share"));
        assert!(xp_in(&server, "core:hunter") > 0, "a share of the hunt's experience");
        assert!(loot_falling(&server) > 0, "and of the kill");
        let journal = &server.players[&1].journal;
        assert!(journal.get(voxmar_engine::journal::Page::Animals, "base:wild_boar").is_some(), "the kill in their journal");
        assert!(journal.get(voxmar_engine::journal::Page::Collection, "base:raw_pork").is_some(), "and its meat in their collection log");
    }

    #[test]
    fn a_hunt_with_nothing_to_hunt_goes_on_its_way() {
        let mut server = fixture();
        let id = start(&mut server, "hunting_party", (110, 128));
        assert!(run_until(&mut server, id, 3, |inst| matches!(inst.plan, Plan::March { .. })));
        assert_eq!(server.wandering.active[&id].stage, Stage::Passing);
    }

    #[test]
    fn a_hunter_shows_a_beginner_how_to_read_the_ground() {
        let mut server = fixture();
        let id = start(&mut server, "hunting_party", (110, 128));
        let hunter = members_of(&server, id, 0)[0];
        let client = listen(&mut server);
        assert!(server.meet(1, hunter));
        assert!(says(&told(&client), "wander.base.hunting_party.tracks"));
        assert!(xp_in(&server, "core:hunter") > 0);
    }

    #[test]
    fn a_poacher_takes_a_deer_and_pays_for_silence_once() {
        let mut server = fixture();
        let deer = wild(&mut server, "base:red_deer", Vec3::new(98.5, 200.0, 148.5), 2);
        let id = start(&mut server, "poacher", (110, 128));
        let poacher = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(run_until(&mut server, id, 120, |inst| inst.stage == Stage::Took), "the poacher takes the deer");
        assert!(!server.creatures.live.contains_key(&deer));
        assert!(server.meet(1, poacher));
        assert_eq!(count_of(&server, "base:raw_venison"), 1, "a cut for your silence");
        assert!(server.meet(1, poacher));
        assert_eq!(count_of(&server, "base:raw_venison"), 1, "once");
    }

    #[test]
    fn a_fighter_with_a_weapon_sends_the_poacher_off_and_the_deer_lives() {
        let mut server = fixture();
        let deer = wild(&mut server, "base:red_deer", Vec3::new(80.5, 200.0, 160.5), 2);
        let id = start(&mut server, "poacher", (110, 128));
        let poacher = members_of(&server, id, 0)[0];
        let weapon = tagged(&server, "weapon");
        let name = server.content.blocks.def(weapon).name.clone();
        give_held(&mut server, &name, 1);
        // A beginner with a weapon does not frighten him.
        let client = listen(&mut server);
        assert!(server.meet(1, poacher));
        assert!(!says(&told(&client), "wander.base.poacher.spare"));
        skill_to(&mut server, "core:attack", 10);
        assert!(server.meet(1, poacher));
        assert!(says(&told(&client), "wander.base.poacher.spare"));
        run(&mut server, 60 * TICKS_PER_SECOND as u64);
        assert!(server.creatures.live.contains_key(&deer), "the deer lives");
        assert!(server.wandering.active.get(&id).is_none_or(|inst| inst.stage != Stage::Took), "and nothing was taken");
    }

    #[test]
    fn a_trapper_takes_small_game_up_to_its_takes_then_goes() {
        let mut server = fixture();
        let rabbits: Vec<u32> = (0..4).map(|i| wild(&mut server, "base:rabbit", Vec3::new(96.5 + i as f32 * 3.0, 200.0, 146.5), 10 + i)).collect();
        let id = start(&mut server, "trapper", (110, 128));
        assert!(run_until(&mut server, id, 300, |inst| matches!(inst.plan, Plan::March { .. })), "the trapper is done");
        let left = rabbits.iter().filter(|r| server.creatures.live.contains_key(r)).count();
        assert_eq!(left, 1, "three taken, and one left to run");
    }

    #[test]
    fn a_trapper_teaches_a_beginner_the_snare() {
        let mut server = fixture();
        let id = start(&mut server, "trapper", (110, 128));
        let trapper = members_of(&server, id, 0)[0];
        assert!(server.meet(1, trapper));
        assert!(xp_in(&server, "core:hunter") > 0);
        skill_to(&mut server, "core:hunter", 20);
        let before = xp_in(&server, "core:hunter");
        assert!(server.meet(1, trapper));
        assert_eq!(xp_in(&server, "core:hunter"), before, "nothing for the skilled");
    }

    /// The centre of a settlement of the fixture world's plan, and a place far from every one.
    fn town_and_wild(server: &Server) -> (Vec3, Vec3) {
        let towns = server.overworld.settlements_near(IVec2::new(128, 128), 6000);
        let centre = towns.first().expect("a settlement in the world's plan").centre.as_vec3();
        let lonely = (1..400).map(|i| centre + Vec3::new(i as f32 * 400.0, 0.0, 0.0)).find(|&p| server.town_near(p, 1000.0).is_none()).expect("somewhere far from towns");
        (centre, lonely)
    }

    #[test]
    fn the_town_watch_comes_only_near_a_town() {
        let mut server = fixture();
        // Broad day, two days in.
        server.day = (0..20).map(|i| TICKS_PER_DAY as f64 * (2.0 + i as f64 / 20.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) > 0.8).expect("a bright hour");
        let guard = event(&server, "guard_patrol");
        let (centre, lonely) = town_and_wild(&server);
        let comes = |server: &Server, at: Vec3| (0..600u64).any(|seed| server.pick_event(1, at.floor().as_ivec3(), mix(seed, 3)) == Some(guard));
        assert!(comes(&server, centre + Vec3::new(120.0, 0.0, 0.0)), "the guard walk the town's land");
        assert!(!comes(&server, lonely), "and nowhere far from a town");
    }

    #[test]
    fn the_night_watch_binds_the_hurt_drills_a_beginner_and_tells_the_lost_the_way() {
        let mut server = fixture();
        let id = start(&mut server, "night_watchman", (110, 128));
        let watchman = members_of(&server, id, 0)[0];
        let (centre, _) = town_and_wild(&server);
        server.creatures.live.get_mut(&watchman).unwrap().body.position = centre + Vec3::new(150.0, 0.0, 0.0);
        let client = listen(&mut server);
        let max = server.players[&1].health.max;
        server.players.get_mut(&1).unwrap().health.current = max - 8;
        assert!(server.meet(1, watchman));
        assert!(server.players[&1].health.current > max - 8, "bound up");
        assert!(server.meet(1, watchman));
        assert!(xp_in(&server, "core:defence") > 0, "a word on keeping one's guard");
        assert!(server.meet(1, watchman));
        let said = told(&client);
        assert!(said.iter().any(|(key, _)| key == "wander.town"), "the way to town: {said:?}");
    }

    #[test]
    fn a_fang_earns_the_town_guards_bounty_and_a_beginner_with_a_weapon_is_drilled() {
        let mut server = fixture();
        let id = start(&mut server, "guard_patrol", (110, 128));
        let guard = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:fang", 2);
        assert!(server.meet(1, guard));
        assert_eq!(count_of(&server, "base:fang"), 1);
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "the town's bounty");
        assert!(server.meet(1, guard));
        assert_eq!(count_of(&server, "base:fang"), 1, "once");
        let weapon = tagged(&server, "weapon");
        let name = server.content.blocks.def(weapon).name.clone();
        give_held(&mut server, &name, 1);
        let before = xp_in(&server, "core:attack");
        assert!(server.meet(1, guard));
        assert!(xp_in(&server, "core:attack") > before, "drilled");
    }

    #[test]
    fn a_coin_buys_a_true_fortune_and_with_nothing_to_tell_the_coin_is_kept() {
        let mut server = fixture();
        let (id, teller) = camped(&mut server, "fortune_teller", 3.0);
        let at = server.creatures.live[&teller].body.position;
        let wonder = tagged(&server, "wonder");
        let name = server.content.blocks.def(wonder).name.clone();
        let spot = (at + Vec3::new(30.0, 0.0, 0.0)).floor().as_ivec3();
        place(&mut server, &name, IVec3::new(spot.x.min(250), 200, spot.z));
        give_held(&mut server, "base:gold_coin", 2);
        let client = listen(&mut server);
        assert!(server.meet(1, teller));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "the coin across the palm");
        let said = told(&client);
        assert!(says(&said, "wander.base.fortune_teller.fortune"));
        let town = server.town_near(at, 600.0).is_some();
        let told_of = |key: &str| said.iter().any(|(k, _)| k == key);
        assert!(told_of("wander.point") || town && told_of("wander.town"), "something true and near: {said:?}");
        if !town {
            let point = said.iter().find(|(k, _)| k == "wander.point").unwrap();
            assert!(point.1.iter().any(|(n, w)| n == "thing" && *w == Words::Block(name.clone())), "the wonder: {point:?}");
        }
        assert!(server.meet(1, teller));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "once");
        // Far from everything, another teller has nothing to tell, and takes nothing.
        let (_, lonely) = town_and_wild(&server);
        server.wandering.active.get_mut(&id).unwrap().told.clear();
        server.creatures.live.get_mut(&teller).unwrap().body.position = lonely;
        assert!(server.meet(1, teller));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "nothing to tell, nothing taken");
    }

    /// Broad day, a few days into the world.
    fn broad_day(server: &mut Server) {
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (3.0 + i as f64 / 40.0)).find(|&day| (0.6..0.85).contains(&voxmar_engine::creature::daylight(day as u64))).expect("a bright hour");
    }

    /// A spot near `from` whose biome kind `event` comes to, and that `also` suits.
    fn land_for(server: &Server, event: usize, from: Vec3, also: impl Fn(Vec3) -> bool) -> IVec3 {
        let e = server.content.wandering.get(event);
        (0..4000)
            .map(|i| {
                let (a, r) = (i as f32 * 2.399, 40.0 + i as f32 * 2.0);
                IVec3::new((from.x + a.cos() * r) as i32, 200, (from.z + a.sin() * r) as i32)
            })
            .find(|spot| {
                let biome = server.overworld.biomes().get(server.overworld.site(spot.x, spot.z).biome);
                e.biome_weight(&biome.name, &biome.tags) > 0.0 && also(spot.as_vec3())
            })
            .expect("land the kind comes to")
    }

    /// Market folk started, their town moved to `goal` (within the fixture's floor).
    fn market_folk_to(server: &mut Server, goal: Vec3) -> u64 {
        let id = start(server, "market_folk", (110, 128));
        let inst = server.wandering.active.get_mut(&id).unwrap();
        let lead = server.creatures.live[&inst.members[0].creature].body.position;
        inst.plan = Plan::Journey { heading: (xz(goal) - xz(lead)).normalize(), goal, pace: 0.9, with: Default::default(), arrived: None };
        id
    }

    #[test]
    fn market_folk_walk_to_town_and_thank_whoever_kept_them_company() {
        let mut server = fixture();
        let id = market_folk_to(&mut server, Vec3::new(40.5, 200.0, 200.5));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        // Walking along beside the leader the whole way.
        let arrived = (0..200).any(|_| {
            if let Some(at) = leader_at(&server, id) {
                server.players.get_mut(&1).unwrap().position = at + Vec3::new(3.0, 0.0, 0.0);
            }
            run(&mut server, 20);
            server.wandering.active.get(&id).is_some_and(|inst| inst.stage == Stage::Arrived)
        });
        assert!(arrived, "they reach the town");
        assert!(says(&told(&client), "wander.base.market_folk.arrived"));
        assert_eq!(count_of(&server, "base:bread"), 2, "their bread for the road");
        assert!(has_effect(&server, "refreshed"));
        let trader = members_of(&server, id, 0)[0];
        assert!(server.meet(1, trader));
        assert!(says(&told(&client), "wander.base.market_folk.there"), "and they say where the market is");
        assert_eq!(server.players[&1].tallies.get("met:base:market_folk"), Some(&1), "the walk is a dealing for quests; the words at the end are not");
    }

    #[test]
    fn market_folk_thank_only_those_who_walked_the_way() {
        let mut server = fixture();
        let id = market_folk_to(&mut server, Vec3::new(40.5, 200.0, 200.5));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        // Waiting at the town instead of walking with them.
        server.players.get_mut(&1).unwrap().position = Vec3::new(44.5, 200.0, 200.5);
        assert!(run_until(&mut server, id, 200, |inst| inst.stage == Stage::Arrived), "they reach the town");
        assert_eq!(count_of(&server, "base:bread"), 0, "met at the end is not walked with");
        assert_eq!(server.players[&1].tallies.get("met:base:market_folk"), None, "nor dealt with");
    }

    #[test]
    fn market_folk_come_only_where_a_town_is_a_walk_away() {
        let mut server = fixture();
        broad_day(&mut server);
        let folk = event(&server, "market_folk");
        let (centre, lonely) = town_and_wild(&server);
        let comes = |server: &Server, at: Vec3| (0..600u64).any(|seed| server.pick_event(1, at.floor().as_ivec3(), mix(seed, 3)) == Some(folk));
        let away = land_for(&server, folk, centre, |at| server.town_near(at, 600.0).is_some_and(|(to, _)| to.length() > 200.0));
        assert!(comes(&server, away.as_vec3()), "a town a walk away");
        assert!(!comes(&server, centre + Vec3::new(20.0, 0.0, 0.0)), "not at the town itself");
        assert!(!comes(&server, lonely), "nor where there is none");
    }

    #[test]
    fn market_folk_buy_wheat_once() {
        let mut server = fixture();
        let id = start(&mut server, "market_folk", (110, 128));
        let trader = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:wheat", 16);
        assert!(server.meet(1, trader));
        assert_eq!(count_of(&server, "base:wheat"), 8);
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        assert!(server.meet(1, trader));
        assert_eq!(count_of(&server, "base:wheat"), 8, "once");
    }

    #[test]
    fn rain_brings_drenched_travellers_and_never_rainmakers() {
        let mut server = fixture();
        broad_day(&mut server);
        let (drenched, rainmakers) = (event(&server, "drenched_travellers"), event(&server, "rainmakers"));
        let spot = land_for(&server, drenched, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let picks = |server: &Server, wet: bool| (0..800u64).filter_map(|seed| server.pick_event_in(1, spot, mix(seed, 3), wet)).collect::<Vec<_>>();
        let (in_rain, in_dry) = (picks(&server, true), picks(&server, false));
        assert!(in_rain.contains(&drenched) && !in_dry.contains(&drenched), "travellers caught in the rain come only in it");
        assert!(!in_rain.contains(&rainmakers), "nobody dances for rain in the rain");
    }

    #[test]
    fn a_light_warms_the_drenched_and_food_buys_word_of_ore() {
        let mut server = fixture();
        let id = start(&mut server, "drenched_travellers", (110, 128));
        let traveller = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:torch", 1);
        assert!(server.meet(1, traveller));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "a coin for the warmth");
        assert_eq!(count_of(&server, "base:torch"), 1, "the light is not taken");
        let at = server.creatures.live[&traveller].body.position.floor().as_ivec3();
        let ore = tagged(&server, "ore_sample");
        let name = server.content.blocks.def(ore).name.clone();
        place(&mut server, &name, IVec3::new(at.x + 20, 200, at.z));
        give_held(&mut server, "base:bread", 1);
        let client = listen(&mut server);
        assert!(server.meet(1, traveller));
        let said = told(&client);
        let point = said.iter().find(|(k, _)| k == "wander.point").unwrap_or_else(|| panic!("the ledge: {said:?}"));
        assert!(point.1.iter().any(|(n, w)| n == "thing" && *w == Words::Block(name.clone())));
    }

    #[test]
    fn two_coins_and_the_rainmakers_bring_rain() {
        let mut server = fixture();
        let (_, maker) = camped(&mut server, "rainmakers", 3.0);
        let at = server.creatures.live[&maker].body.position;
        assert!(server.weather.forced_at(xz(at), server.tick).is_none());
        give_held(&mut server, "base:gold_coin", 4);
        assert!(server.meet(1, maker));
        assert_eq!(count_of(&server, "base:gold_coin"), 2);
        let (preset, left) = server.weather.forced_at(xz(at), server.tick).expect("rain is on its way");
        assert_eq!(preset.short_name(), "rain");
        assert!(left > 500, "for ten minutes: {left}");
        assert!(server.meet(1, maker));
        assert_eq!(count_of(&server, "base:gold_coin"), 2, "one dance for the party");
    }

    #[test]
    fn the_shrine_fills_spent_prayer_and_shows_a_beginner_the_first() {
        let mut server = fixture();
        let id = start(&mut server, "shrine_bearer", (110, 128));
        let priest = members_of(&server, id, 0)[0];
        assert!(server.meet(1, priest));
        assert!(xp_in(&server, "core:magic") > 0, "a first prayer");
        skill_to(&mut server, "core:magic", 20);
        let most = server.prayer_most(1);
        server.players.get_mut(&1).unwrap().praying.points = most;
        let client = listen(&mut server);
        assert!(server.meet(1, priest));
        assert!(!says(&told(&client), "wander.base.shrine_bearer.renew"), "nothing to fill");
        server.players.get_mut(&1).unwrap().praying.points = 0.0;
        assert!(server.meet(1, priest));
        assert!(says(&told(&client), "wander.base.shrine_bearer.renew"));
        assert_eq!(server.players[&1].praying.points, most.min(30.0), "filled, to the most");
        server.players.get_mut(&1).unwrap().praying.points = 0.0;
        assert!(server.meet(1, priest));
        assert_eq!(server.players[&1].praying.points, 0.0, "once");
    }

    #[test]
    fn neighbours_call_only_on_one_at_home() {
        let mut server = fixture();
        broad_day(&mut server);
        let neighbours = event(&server, "neighbours");
        let spot = IVec3::new(128, 200, 128);
        let comes = |server: &Server| (0..800u64).any(|seed| server.pick_event(1, spot, mix(seed, 3)) == Some(neighbours));
        assert!(!comes(&server), "no home, no neighbours");
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(140.5, 200.0, 128.5));
        assert!(comes(&server), "at home");
        server.players.get_mut(&1).unwrap().position = Vec3::new(140.5 + 300.0, 200.0, 128.5);
        assert!(!comes(&server), "away from it");
    }

    #[test]
    fn neighbours_bring_a_plate_once_and_seed_for_a_cooked_meal() {
        let mut server = fixture();
        let id = start(&mut server, "neighbours", (110, 128));
        let neighbour = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, neighbour));
        assert_eq!(count_of(&server, "base:earthen_plate"), 1, "a housewarming gift");
        give_held(&mut server, "base:cooked_small_game", 1);
        assert!(server.meet(1, neighbour));
        assert_eq!(count_of(&server, "base:cooked_small_game"), 0);
        assert!(count_of(&server, "base:seed_mix") >= 3);
        assert!(has_effect(&server, "sustained"));
        assert_eq!(count_of(&server, "base:earthen_plate"), 0, "the plate came once, to the party");
    }

    #[test]
    fn wolf_hunters_bring_down_a_wolf_near_a_home() {
        let mut server = fixture();
        let wolf = wild(&mut server, "base:wolf", Vec3::new(100.5, 200.0, 160.5), 3);
        server.creatures.live.get_mut(&wolf).unwrap().hurt_by = Some(1);
        let id = start(&mut server, "wolf_hunters", (110, 128));
        let client = listen(&mut server);
        assert!(run_until(&mut server, id, 120, |inst| inst.stage == Stage::Took), "the wolf is brought down");
        assert!(!server.creatures.live.contains_key(&wolf));
        assert!(says(&told(&client), "wander.base.wolf_hunters.share"));
        assert!(xp_in(&server, "core:attack") > 0);
    }

    #[test]
    fn clay_diggers_fire_clay_without_a_crack_and_show_a_beginner_a_pot() {
        let mut server = fixture();
        let id = start(&mut server, "clay_diggers", (110, 128));
        let potter = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:clay", 5);
        assert!(server.meet(1, potter));
        assert_eq!((count_of(&server, "base:clay"), count_of(&server, "base:fired_clay")), (0, 4), "five fired whole, one kept");
        give_held(&mut server, "base:clay", 5);
        assert!(server.meet(1, potter));
        assert_eq!(count_of(&server, "base:clay"), 5, "once");
        give_held(&mut server, "base:fired_clay", 2);
        assert!(server.meet(1, potter));
        assert!(xp_in(&server, "core:crafting") > 0, "a word on throwing a pot");
        assert_eq!(count_of(&server, "base:fired_clay"), 1);
    }

    #[test]
    fn harvest_home_comes_only_to_one_at_home() {
        let mut server = fixture();
        broad_day(&mut server);
        let harvest = event(&server, "harvest_home");
        let spot = land_for(&server, harvest, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let comes = |server: &Server| (0..800u64).any(|seed| server.pick_event(1, spot, mix(seed, 3)) == Some(harvest));
        let home = spot.as_vec3() + Vec3::new(10.0, 0.0, 0.0);
        server.players.get_mut(&1).unwrap().position = home;
        assert!(!comes(&server), "no home, no harvest home");
        server.players.get_mut(&1).unwrap().bed = Some(home);
        assert!(comes(&server), "at home");
        server.players.get_mut(&1).unwrap().position = home + Vec3::new(200.0, 0.0, 0.0);
        assert!(!comes(&server), "away from it");
    }

    #[test]
    fn a_sheaf_on_the_harvest_dolly_grows_the_fields_about_them() {
        let mut server = fixture();
        let id = start(&mut server, "harvest_home", (110, 128));
        let farmer = members_of(&server, id, 0)[0];
        let client = listen(&mut server);
        // With nothing growing about them, the sheaf is not taken; a beginner learns the song.
        give_held(&mut server, "base:wheat", 6);
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:wheat"), 6, "no field, no blessing");
        assert!(count_of(&server, "base:wheat_seeds") >= 3, "seed for the singing");
        assert!(xp_in(&server, "core:farming") > 0);
        let at = server.creatures.live[&farmer].body.position.floor().as_ivec3();
        let sprout = field(&mut server, at.x - 2..at.x + 2, at.z + 6);
        assert!(server.meet(1, farmer));
        assert!(says(&told(&client), "wander.base.harvest_home.sheaf"));
        assert_eq!(count_of(&server, "base:wheat"), 3);
        let grown = (at.x - 2..at.x + 2).filter(|&x| server.block(IVec3::new(x, 200, at.z + 6)) != Some(sprout)).count();
        assert_eq!(grown, 4, "the field grows on a stage");
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:wheat"), 3, "one blessing for the party");
    }

    /// A party on a `Journey` started, its town moved to `goal` (within the fixture's floor).
    fn journey_to(server: &mut Server, name: &str, goal: Vec3) -> u64 {
        let id = start(server, name, (110, 128));
        let inst = server.wandering.active.get_mut(&id).unwrap();
        let lead = server.creatures.live[&inst.members[0].creature].body.position;
        inst.plan = Plan::Journey { heading: (xz(goal) - xz(lead)).normalize(), goal, pace: 0.8, with: Default::default(), arrived: None };
        id
    }

    #[test]
    fn coal_carters_share_their_coal_with_whoever_walked_to_town_with_them() {
        let mut server = fixture();
        let id = journey_to(&mut server, "coal_carters", Vec3::new(40.5, 200.0, 200.5));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let arrived = (0..240).any(|_| {
            if let Some(at) = leader_at(&server, id) {
                server.players.get_mut(&1).unwrap().position = at + Vec3::new(3.0, 0.0, 0.0);
            }
            run(&mut server, 20);
            server.wandering.active.get(&id).is_some_and(|inst| inst.stage == Stage::Arrived)
        });
        assert!(arrived, "they reach the town");
        assert!(count_of(&server, "base:coal_ore") >= 2, "a little of the coal");
        assert!(xp_in(&server, "core:mining") > 0);
    }

    #[test]
    fn coal_carters_show_a_beginner_a_torch_and_where_coal_shows() {
        let mut server = fixture();
        let id = start(&mut server, "coal_carters", (110, 128));
        let carter = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:stick", 4);
        assert!(server.meet(1, carter));
        assert_eq!((count_of(&server, "base:stick"), count_of(&server, "base:coal_ore")), (2, 1), "two sticks for a lump of coal");
        assert!(xp_in(&server, "core:engineering") > 0);
        let at = server.creatures.live[&carter].body.position.floor().as_ivec3();
        place(&mut server, "base:coal_ore_sample", IVec3::new(at.x + 20, 200, at.z));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, carter));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.point"), "where coal shows");
    }

    #[test]
    fn the_tanner_shows_a_beginner_the_curing_then_buys_four_hides_once() {
        let mut server = fixture();
        let id = start(&mut server, "tanner", (110, 128));
        let tanner = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:hide", 8);
        assert!(server.meet(1, tanner));
        assert!(xp_in(&server, "core:crafting") > 0, "how to cure a hide");
        assert_eq!(count_of(&server, "base:hide"), 8, "shown, not taken");
        assert!(server.meet(1, tanner));
        assert_eq!((count_of(&server, "base:hide"), count_of(&server, "base:gold_coin")), (4, 2));
        assert!(server.meet(1, tanner));
        assert_eq!(count_of(&server, "base:hide"), 4, "once");
    }

    #[test]
    fn the_piper_leads_the_mice_off_and_leaves_seed_for_whoever_followed() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().position = Vec3::new(200.5, 200.0, 128.5);
        let id = start(&mut server, "rat_catcher", (170, 128));
        let mice = members_of(&server, id, 1);
        assert!(mice.len() >= 3, "the mice at his heels");
        for _ in 0..400 {
            follow(&mut server, id, 7.0);
            run(&mut server, 20);
            if server.wandering.active.get(&id).is_none_or(|i| i.stage == Stage::Arrived) {
                break;
            }
        }
        assert_eq!(server.wandering.active[&id].stage, Stage::Arrived, "he led all the way");
        assert!(count_of(&server, "base:seed_mix") >= 3, "the seed they would have eaten");
        assert!(xp_in(&server, "core:farming") > 0);
        run(&mut server, 20 * 10);
        assert!(mice.iter().all(|m| !server.creatures.live.contains_key(m)), "and the mice go with him");
    }

    #[test]
    fn benighted_miners_come_only_by_night_and_far_from_the_start() {
        let mut server = fixture();
        let miners = event(&server, "benighted_miners");
        let far = land_for(&server, miners, Vec3::new(400.0, 200.0, 128.0), |at| flat(at, server.spawn) > 200.0);
        let comes = |server: &Server| (0..800u64).any(|seed| server.pick_event(1, far, mix(seed, 3)) == Some(miners));
        broad_day(&mut server);
        assert!(!comes(&server), "not by day");
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (3.0 + i as f64 / 40.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) < 0.1).expect("a dark hour");
        assert!(comes(&server), "benighted");
    }

    #[test]
    fn benighted_miners_light_up_again_for_coal_or_charcoal_once() {
        let mut server = fixture();
        let id = start(&mut server, "benighted_miners", (110, 128));
        let miner = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:charcoal", 2);
        assert!(server.meet(1, miner));
        assert_eq!((count_of(&server, "base:charcoal"), count_of(&server, "base:gold_coin")), (1, 1), "charcoal will do");
        assert!(xp_in(&server, "core:mining") > 0);
        give_held(&mut server, "base:coal_ore", 1);
        assert!(server.meet(1, miner));
        assert_eq!(count_of(&server, "base:coal_ore"), 1, "lit once is lit");
    }

    /// A bout begun with the sparring master, camped beside the player with a sword in hand.
    fn sparring(server: &mut Server) -> (u64, u32) {
        let (id, master) = camped(server, "sparring_master", 3.0);
        give_held(server, "base:stone_sword", 1);
        assert!(server.meet(1, master));
        assert!(matches!(server.wandering.active[&id].plan, Plan::Bout { player: 1, .. }), "a bout");
        (id, master)
    }

    #[test]
    fn a_bout_won_stops_at_the_share_and_the_master_teaches_the_winner() {
        let mut server = fixture();
        let (id, master) = sparring(&mut server);
        let most = server.content.creatures.get(server.creatures.live[&master].species).def.health;
        let client = listen(&mut server);
        server.hurt_creature(1, master, 1000, None, voxmar_engine::combat::Blow::Plain);
        assert_eq!(server.creatures.live[&master].health, (most as f32 * 0.7).ceil() as u16, "brought down to the share, no further");
        assert!(matches!(server.wandering.active[&id].plan, Plan::Bout { .. }), "a blow in a bout is no provocation");
        run(&mut server, 20);
        let inst = &server.wandering.active[&id];
        assert_eq!(inst.bouts, vec![(1, Stage::Won)]);
        assert!(matches!(inst.plan, Plan::Camp { .. }), "and the camp goes on");
        assert_eq!(server.creatures.live[&master].health, most, "scrapes bound");
        assert_eq!(server.creatures.live[&master].mind.anger, 0.0, "no grudge");
        assert!(says(&told(&client), "wander.base.sparring_master.bout_won"));
        assert!(server.meet(1, master));
        assert!(says(&told(&client), "wander.base.sparring_master.won"));
        assert!(xp_in(&server, "core:attack") > 0);
    }

    #[test]
    fn a_bout_lost_leaves_the_player_standing_and_still_teaches() {
        let mut server = fixture();
        let (id, master) = sparring(&mut server);
        let species = server.creatures.live[&master].species;
        let max = server.players[&1].health.max;
        let floor = (max as f32 * 0.7).ceil() as u8;
        for _ in 0..400 {
            server.tick += 1;
            server.creature_strikes(master, species, 1, 1.0);
        }
        let health = server.players[&1].health;
        assert!(health.alive() && health.current == floor, "beaten down to the share and no further: {} of {max}", health.current);
        let client = listen(&mut server);
        run(&mut server, 20);
        assert_eq!(server.wandering.active[&id].bouts, vec![(1, Stage::Lost)]);
        assert!(says(&told(&client), "wander.base.sparring_master.bout_lost"));
        assert!(server.meet(1, master));
        assert!(says(&told(&client), "wander.base.sparring_master.lost"));
        assert!(xp_in(&server, "core:defence") > 0, "a lesson in losing");
    }

    #[test]
    fn walking_off_ends_a_bout_with_no_winner() {
        let mut server = fixture();
        let (id, master) = sparring(&mut server);
        let at = server.creatures.live[&master].body.position;
        server.players.get_mut(&1).unwrap().position = at + Vec3::new(30.0, 0.0, 0.0);
        run(&mut server, 20);
        let inst = &server.wandering.active[&id];
        assert!(matches!(inst.plan, Plan::Camp { .. }));
        assert!(inst.bouts.is_empty(), "neither won");
        assert_eq!(server.creatures.live[&master].mind.anger, 0.0);
        // A blow after the bout is a blow: he fights in earnest.
        server.wandering_hurt(master, 1);
        assert!(matches!(server.wandering.active[&id].plan, Plan::Avenge { .. }));
    }

    #[test]
    fn the_champion_doubles_a_wager_won() {
        let mut server = fixture();
        let (id, champion) = camped(&mut server, "village_champion", 3.0);
        give_held(&mut server, "base:gold_coin", 6);
        assert!(server.meet(1, champion));
        assert_eq!(count_of(&server, "base:gold_coin"), 4, "the wager");
        assert!(matches!(server.wandering.active[&id].plan, Plan::Bout { .. }));
        server.hurt_creature(1, champion, 1000, None, voxmar_engine::combat::Blow::Plain);
        assert!(server.creatures.live.contains_key(&champion), "nobody falls");
        run(&mut server, 20);
        assert_eq!(server.wandering.active[&id].bouts, vec![(1, Stage::Won)]);
        assert!(server.meet(1, champion));
        assert_eq!(count_of(&server, "base:gold_coin"), 8, "doubled");
        assert!(server.meet(1, champion));
        assert_eq!(count_of(&server, "base:gold_coin"), 8, "once");
    }

    /// A second player beside the party, holding `item`.
    fn onlooker(server: &mut Server, near: u32, item: &str, count: u8) -> voxmar_protocol::ClientConnection {
        let at = server.creatures.live[&near].body.position;
        let (client, connection) = voxmar_protocol::local_pair();
        let mut other = crate::player::Player::new(2, connection, at - Vec3::new(3.0, 0.0, 0.0), 2, server.tick, server.survival.max_health, &server.config.limits);
        other.joined = true;
        assert_eq!(other.inventory.add(server.content.blocks.expect(item), count), 0);
        server.players.insert(2, other);
        client
    }

    #[test]
    fn a_bout_is_the_fighters_own_and_the_champion_takes_on_the_next_after_a_loss() {
        let mut server = fixture();
        let (id, champion) = camped(&mut server, "village_champion", 3.0);
        give_held(&mut server, "base:gold_coin", 2);
        let _client = onlooker(&mut server, champion, "base:gold_coin", 2);
        assert!(server.meet(1, champion));
        // Beaten down: player 1 loses the wager.
        let species = server.creatures.live[&champion].species;
        for _ in 0..400 {
            server.tick += 1;
            server.creature_strikes(champion, species, 1, 1.0);
        }
        run(&mut server, 20);
        assert_eq!(server.wandering.active[&id].bouts, vec![(1, Stage::Lost)]);
        assert!(server.meet(1, champion), "the loser hears it");
        // The one looking on is not the loser: no loss line in the way, the wager is theirs.
        assert!(server.meet(2, champion));
        assert!(matches!(server.wandering.active[&id].plan, Plan::Bout { player: 2, .. }), "a bout with the onlooker");
        server.hurt_creature(2, champion, 1000, None, voxmar_engine::combat::Blow::Plain);
        run(&mut server, 20);
        assert!(server.wandering.active[&id].bouts.contains(&(2, Stage::Won)));
        // Player 1 cannot take player 2's winnings.
        let coins = count_of(&server, "base:gold_coin");
        server.meet(1, champion);
        assert_eq!(count_of(&server, "base:gold_coin"), coins, "a win is the winner's");
    }

    #[test]
    fn a_player_already_down_to_the_share_is_offered_no_bout() {
        let mut server = fixture();
        let (id, champion) = camped(&mut server, "village_champion", 3.0);
        give_held(&mut server, "base:gold_coin", 2);
        let max = server.players[&1].health.max;
        server.players.get_mut(&1).unwrap().health.current = (max as f32 * 0.5).ceil() as u8;
        server.meet(1, champion);
        assert_eq!(count_of(&server, "base:gold_coin"), 2, "no wager taken from the wounded");
        assert!(!matches!(server.wandering.active[&id].plan, Plan::Bout { .. }));
    }

    #[test]
    fn stargazers_come_only_on_a_clear_night_and_the_sky_focuses_a_watcher() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (3.0 + i as f64 / 40.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) < 0.1).expect("a dark hour");
        let gazers = event(&server, "stargazers");
        let spot = land_for(&server, gazers, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let picks = |server: &Server, wet: bool| (0..800u64).filter_map(|seed| server.pick_event_in(1, spot, mix(seed, 3), wet)).collect::<Vec<_>>();
        assert!(picks(&server, false).contains(&gazers), "a clear night");
        assert!(!picks(&server, true).contains(&gazers), "not under cloud");
        let mut server = fixture();
        let (_, elder) = camped(&mut server, "stargazers", 4.0);
        run(&mut server, 27 * TICKS_PER_SECOND as u64);
        assert!(has_effect(&server, "focused"));
        assert!(xp_in(&server, "core:magic") > 0);
        let at = server.creatures.live[&elder].body.position.floor().as_ivec3();
        let wonder = tagged(&server, "wonder");
        let name = server.content.blocks.def(wonder).name.clone();
        place(&mut server, &name, IVec3::new(at.x + 20, 200, at.z));
        give_held(&mut server, "base:gold_coin", 1);
        let client = listen(&mut server);
        assert!(server.meet(1, elder));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.point"), "a look through the tube");
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
    }

    #[test]
    fn a_sweeps_handshake_brings_luck_once() {
        let mut server = fixture();
        let id = start(&mut server, "chimney_sweep", (110, 128));
        let sweep = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, sweep));
        assert!(says(&told(&client), "wander.base.chimney_sweep.luck"));
        let lucky = |server: &Server| count_of(server, "base:gold_coin") == 1 || has_effect(server, "refreshed");
        assert!(lucky(&server), "a coin found or a lighter step (no wonder near)");
        assert!(server.meet(1, sweep));
        assert!(count_of(&server, "base:gold_coin") <= 1, "once");
    }

    #[test]
    fn a_quack_tonic_does_something_harmless_for_a_coin_once() {
        let mut server = fixture();
        let id = start(&mut server, "quack_doctor", (110, 128));
        let quack = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().health.current = 2;
        give_held(&mut server, "base:gold_coin", 2);
        assert!(server.meet(1, quack));
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        let did = server.players[&1].health.current > 2 || ["refreshed", "focused", "soothed", "sustained"].iter().any(|e| has_effect(&server, e));
        assert!(did, "it does something");
        assert!(server.meet(1, quack));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "once");
    }

    #[test]
    fn a_cart_sticks_only_in_rain_and_a_shoulder_to_the_wheel_frees_it() {
        let mut server = fixture();
        broad_day(&mut server);
        let cart = event(&server, "stuck_cart");
        let spot = land_for(&server, cart, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let picks = |server: &Server, wet: bool| (0..800u64).filter_map(|seed| server.pick_event_in(1, spot, mix(seed, 3), wet)).collect::<Vec<_>>();
        assert!(picks(&server, true).contains(&cart) && !picks(&server, false).contains(&cart), "mud wants rain");
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        camped(&mut server, "stuck_cart", 3.0);
        run(&mut server, 14 * TICKS_PER_SECOND as u64);
        assert_eq!((count_of(&server, "base:gold_coin"), count_of(&server, "base:bread")), (1, 1));
    }

    #[test]
    fn wassailers_come_at_night_to_one_at_home_and_toast_the_fields_once() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (3.0 + i as f64 / 40.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) < 0.1).expect("a dark hour");
        let singers = event(&server, "wassailers");
        let spot = land_for(&server, singers, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let comes = |server: &Server| (0..800u64).any(|seed| server.pick_event(1, spot, mix(seed, 3)) == Some(singers));
        let home = spot.as_vec3() + Vec3::new(10.0, 0.0, 0.0);
        server.players.get_mut(&1).unwrap().position = home;
        assert!(!comes(&server), "no home, no wassail");
        server.players.get_mut(&1).unwrap().bed = Some(home);
        assert!(comes(&server), "at home, at night");
        broad_day(&mut server);
        assert!(!comes(&server), "not by day");

        let mut server = fixture();
        let (_, bard) = camped(&mut server, "wassailers", 3.0);
        give_held(&mut server, "base:bread", 2);
        assert!(!server.meet(1, bard) || count_of(&server, "base:bread") == 2, "no field, no toast");
        assert_eq!(count_of(&server, "base:bread"), 2);
        let at = server.creatures.live[&bard].body.position.floor().as_ivec3();
        let sprout = field(&mut server, at.x - 2..at.x + 2, at.z + 6);
        let client = listen(&mut server);
        assert!(server.meet(1, bard));
        assert!(says(&told(&client), "wander.base.wassailers.toast"));
        assert_eq!(count_of(&server, "base:bread"), 1);
        assert!(xp_in(&server, "core:farming") > 0);
        let grown = (at.x - 2..at.x + 2).filter(|&x| server.block(IVec3::new(x, 200, at.z + 6)) != Some(sprout)).count();
        assert_eq!(grown, 4, "the field grows on a stage");
        server.meet(1, bard);
        assert_eq!(count_of(&server, "base:bread"), 1, "one toast for the party");
    }

    #[test]
    fn thatchers_show_a_beginner_builder_for_four_wheat_once() {
        let mut server = fixture();
        let id = start(&mut server, "thatchers", (110, 128));
        let thatcher = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:wheat", 8);
        assert!(server.meet(1, thatcher));
        assert_eq!(count_of(&server, "base:wheat"), 4);
        assert!(xp_in(&server, "core:construction") > 0, "how straw is laid");
        let before = xp_in(&server, "core:construction");
        server.meet(1, thatcher);
        assert_eq!((count_of(&server, "base:wheat"), xp_in(&server, "core:construction")), (4, before), "once");
        // A builder past the lesson is not taught it.
        let mut server = fixture();
        let id = start(&mut server, "thatchers", (110, 128));
        let thatcher = members_of(&server, id, 0)[0];
        skill_to(&mut server, "core:construction", 12);
        give_held(&mut server, "base:wheat", 8);
        server.meet(1, thatcher);
        assert_eq!(count_of(&server, "base:wheat"), 8, "nothing taken from the skilled");
    }

    #[test]
    fn seed_swappers_trade_wheat_seed_for_another_crop_only_to_a_farmer_who_has_grown_a_little() {
        let mut server = fixture();
        let id = start(&mut server, "seed_swappers", (110, 128));
        let farmer = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:wheat_seeds", 8);
        assert!(server.meet(1, farmer));
        assert!(xp_in(&server, "core:farming") > 0, "when to sow");
        assert_eq!(count_of(&server, "base:wheat_seeds"), 8, "no swap for a beginner (Farming 3)");
        skill_to(&mut server, "core:farming", 3);
        assert!(server.meet(1, farmer));
        assert_eq!((count_of(&server, "base:wheat_seeds"), count_of(&server, "base:rye")), (4, 2), "rye, the one crop sown at Farming 3");
        // Carrot and chickpea are sown from Farming 7, and offered only then.
        skill_to(&mut server, "core:farming", 7);
        assert!(server.meet(1, farmer));
        assert_eq!(count_of(&server, "base:wheat_seeds"), 0, "a swap is a swap, again and again");
        let other = ["base:rye", "base:carrot", "base:chickpea"].iter().map(|crop| count_of(&server, crop)).sum::<u32>();
        assert_eq!(other, 4, "two more of a crop to sow");
    }

    #[test]
    fn flint_knappers_teach_trade_and_say_where_the_gravel_is() {
        let mut server = fixture();
        let id = start(&mut server, "flint_knappers", (110, 128));
        let knapper = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&knapper].body.position.floor().as_ivec3();
        let gravel = tagged(&server, "gravel");
        server.world.set_block(IVec3::new(at.x + 20, 200, at.z), gravel);
        give_held(&mut server, "base:flint", 2);
        let client = listen(&mut server);
        assert!(server.meet(1, knapper));
        assert_eq!(count_of(&server, "base:flint"), 0);
        assert!(xp_in(&server, "core:crafting") > 0, "the knack");
        assert!(server.meet(1, knapper));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.point"), "where the gravel lies");
        give_held(&mut server, "base:hide", 1);
        assert!(server.meet(1, knapper));
        assert!((3..=4).contains(&count_of(&server, "base:flint")), "a handful for a hide");
    }

    #[test]
    fn frog_giggers_take_frogs_and_teach_a_beginner_to_still_the_light() {
        let mut server = fixture();
        let frogs: Vec<u32> = (0..4).map(|i| wild(&mut server, "base:common_frog", Vec3::new(96.5 + i as f32 * 3.0, 200.0, 146.5), 20 + i)).collect();
        let id = start(&mut server, "frog_giggers", (110, 128));
        let gigger = members_of(&server, id, 0)[0];
        assert!(server.meet(1, gigger));
        assert!(xp_in(&server, "core:hunter") > 0);
        assert!(run_until(&mut server, id, 300, |inst| matches!(inst.plan, Plan::March { .. })), "the giggers are done");
        let left = frogs.iter().filter(|f| server.creatures.live.contains_key(f)).count();
        assert_eq!(left, 1, "three taken, one left to croak");
    }

    #[test]
    fn a_messenger_runs_to_town_and_stands_a_coin_to_one_who_kept_up() {
        let mut server = fixture();
        let id = journey_to(&mut server, "messenger", Vec3::new(40.5, 200.0, 200.5));
        if let Plan::Journey { pace, .. } = &mut server.wandering.active.get_mut(&id).unwrap().plan {
            *pace = 1.5;
        }
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        let arrived = (0..200).any(|_| {
            if let Some(at) = leader_at(&server, id) {
                server.players.get_mut(&1).unwrap().position = at + Vec3::new(3.0, 0.0, 0.0);
            }
            run(&mut server, 20);
            server.wandering.active.get(&id).is_some_and(|inst| inst.stage == Stage::Arrived)
        });
        assert!(arrived, "the letter reaches the town");
        assert!(says(&told(&client), "wander.base.messenger.arrived"));
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        assert!(xp_in(&server, "core:hitpoints") > 0, "a run's worth");
    }

    #[test]
    fn a_messenger_passes_on_news_of_something_strange_once() {
        let mut server = fixture();
        let id = start(&mut server, "messenger", (110, 128));
        let runner = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&runner].body.position.floor().as_ivec3();
        let wonder = tagged(&server, "wonder");
        server.world.set_block(IVec3::new(at.x + 30, 200, at.z), wonder);
        let client = listen(&mut server);
        assert!(server.meet(1, runner));
        assert!(told(&client).iter().filter(|(k, _)| k == "wander.point").count() == 1, "news of it");
        server.meet(1, runner);
        assert!(told(&client).iter().filter(|(k, _)| k == "wander.point").count() == 0, "told once");
    }

    #[test]
    fn a_sleepwalker_wanders_near_a_home_at_night_and_woken_pays_and_goes_home() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (3.0 + i as f64 / 40.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) < 0.1).expect("a dark hour");
        let walker = event(&server, "sleepwalker");
        let spot = land_for(&server, walker, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let comes = |server: &Server| (0..800u64).any(|seed| server.pick_event(1, spot, mix(seed, 3)) == Some(walker));
        let home = spot.as_vec3() + Vec3::new(10.0, 0.0, 0.0);
        server.players.get_mut(&1).unwrap().position = home;
        assert!(!comes(&server), "no home, nobody's sleepwalker");
        server.players.get_mut(&1).unwrap().bed = Some(home);
        assert!(comes(&server), "near home at night");
        broad_day(&mut server);
        assert!(!comes(&server), "awake by day");

        let mut server = fixture();
        let id = start(&mut server, "sleepwalker", (110, 128));
        let sleeper = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, sleeper));
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "a coin for the kindness");
        assert!(matches!(server.wandering.active[&id].plan, Plan::March { .. }), "and home they go");
        server.meet(1, sleeper);
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "once");
    }

    #[test]
    fn the_assayer_teaches_a_beginner_holding_ore_and_buys_copper_once() {
        let mut server = fixture();
        let id = start(&mut server, "ore_assayer", (110, 128));
        let assayer = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:copper_ore", 8);
        assert!(server.meet(1, assayer));
        assert!(xp_in(&server, "core:mining") > 0, "how to read ore");
        assert_eq!(count_of(&server, "base:copper_ore"), 8, "read, not taken");
        assert!(server.meet(1, assayer));
        assert_eq!((count_of(&server, "base:copper_ore"), count_of(&server, "base:gold_coin")), (4, 1));
        assert!(server.meet(1, assayer));
        assert_eq!(count_of(&server, "base:copper_ore"), 4, "once");
    }

    #[test]
    fn bloomery_smelters_smelt_copper_once_keeping_an_ingot() {
        let mut server = fixture();
        let id = start(&mut server, "bloomery_smelters", (110, 128));
        let smith = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:copper_ore", 1);
        assert!(server.meet(1, smith));
        assert_eq!(count_of(&server, "base:copper_ore"), 1, "one lump is not worth the fire");
        assert!(xp_in(&server, "core:engineering") > 0, "how bellows work");
        give_held(&mut server, "base:copper_ore", 3);
        assert!(server.meet(1, smith));
        assert_eq!((count_of(&server, "base:copper_ore"), count_of(&server, "base:copper_ingot")), (0, 2), "three smelted, one kept");
        give_held(&mut server, "base:copper_ore", 3);
        server.meet(1, smith);
        assert_eq!(count_of(&server, "base:copper_ore"), 3, "once");
    }

    #[test]
    fn moth_netters_pay_one_who_holds_up_a_flame() {
        let mut server = fixture();
        let id = start(&mut server, "moth_netters", (110, 128));
        let netter = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        server.meet(1, netter);
        assert_eq!(count_of(&server, "base:gold_coin"), 0, "no light, no moths");
        let flame = tagged(&server, "fire");
        let name = server.content.blocks.def(flame).name.clone();
        give_held(&mut server, &name, 1);
        assert!(server.meet(1, netter));
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        assert!(xp_in(&server, "core:hunter") > 0);
        server.meet(1, netter);
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "once");
    }

    #[test]
    fn the_potter_shows_a_beginner_a_burnish_and_buys_four_pots_once() {
        let mut server = fixture();
        let id = start(&mut server, "potter", (110, 128));
        let potter = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:fired_clay", 8);
        assert!(server.meet(1, potter));
        assert!(xp_in(&server, "core:crafting") > 0);
        assert_eq!(count_of(&server, "base:fired_clay"), 8, "shown, not taken");
        assert!(server.meet(1, potter));
        assert_eq!((count_of(&server, "base:fired_clay"), count_of(&server, "base:gold_coin")), (4, 2));
        server.meet(1, potter);
        assert_eq!(count_of(&server, "base:fired_clay"), 4, "once");
    }

    #[test]
    fn snail_races_are_run_in_the_rain_and_a_bet_pays_double_or_a_laugh() {
        let mut server = fixture();
        broad_day(&mut server);
        let races = event(&server, "snail_racers");
        let spot = land_for(&server, races, Vec3::new(128.0, 200.0, 128.0), |_| true);
        let picks = |server: &Server, wet: bool| (0..800u64).filter_map(|seed| server.pick_event_in(1, spot, mix(seed, 3), wet)).collect::<Vec<_>>();
        assert!(picks(&server, true).contains(&races) && !picks(&server, false).contains(&races), "snails race in the wet");
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let (_, racer) = camped(&mut server, "snail_racers", 3.0);
        run(&mut server, 22 * TICKS_PER_SECOND as u64);
        assert!(has_effect(&server, "refreshed"), "a race watched through");
        give_held(&mut server, "base:gold_coin", 1);
        assert!(server.meet(1, racer));
        let coins = count_of(&server, "base:gold_coin");
        assert!(coins == 0 || coins == 2, "lost or doubled, not {coins}");
        give_held(&mut server, "base:gold_coin", 1);
        server.meet(1, racer);
        assert_eq!(count_of(&server, "base:gold_coin"), 1, "one bet each");
    }

    #[test]
    fn the_henwife_teaches_trades_eggs_and_gives_a_hen_to_a_farmer_with_a_home() {
        let mut server = fixture();
        let id = start(&mut server, "henwife", (110, 128));
        let wife = members_of(&server, id, 0)[0];
        let hens = members_of(&server, id, 1);
        assert!((2..=3).contains(&hens.len()));
        give_held(&mut server, "base:wheat_seeds", 6);
        assert!(server.meet(1, wife));
        assert!(xp_in(&server, "core:farming") > 0, "where hens lay");
        assert_eq!(count_of(&server, "base:wheat_seeds"), 6, "no hen for a beginner without a home");
        skill_to(&mut server, "core:farming", 3);
        server.meet(1, wife);
        assert_eq!(count_of(&server, "base:wheat_seeds"), 6, "nor for one without a home");
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(128.5, 200.0, 128.5));
        assert!(server.meet(1, wife));
        assert_eq!(count_of(&server, "base:wheat_seeds"), 0);
        assert_eq!(hens.iter().filter(|h| server.creatures.live[h].owner.as_deref() == Some("Tëster")).count(), 1, "one hen is the player's");
        give_held(&mut server, "base:feather", 4);
        assert!(server.meet(1, wife));
        assert_eq!(count_of(&server, "base:egg"), 2, "feathers for eggs");
    }

    #[test]
    fn mushroom_pickers_come_after_rain_and_teach_point_and_trade() {
        let server = fixture();
        let pickers = server.content.wandering.get(event(&server, "mushroom_pickers"));
        assert_eq!(pickers.def.needs.wet, Some(true), "after rain (the wet rule's own tests cover the pick)");
        let mut server = fixture();
        let id = start(&mut server, "mushroom_pickers", (110, 128));
        let picker = members_of(&server, id, 0)[0];
        let at = server.creatures.live[&picker].body.position.floor().as_ivec3();
        place(&mut server, "base:brown_mushroom", IVec3::new(at.x + 20, 200, at.z));
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, picker));
        assert!(xp_in(&server, "core:cooking") > 0, "which are good to eat");
        assert!(server.meet(1, picker));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.point"), "where more grow");
        give_held(&mut server, "base:brown_mushroom", 4);
        assert!(server.meet(1, picker));
        assert_eq!((count_of(&server, "base:brown_mushroom"), count_of(&server, "base:bread")), (0, 1));
    }

    #[test]
    fn the_woodcarver_answers_a_beginner_and_a_carver_of_skill_differently() {
        let mut server = fixture();
        let id = start(&mut server, "woodcarver", (110, 128));
        let carver = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:oak_log", 8);
        let client = listen(&mut server);
        assert!(server.meet(1, carver));
        assert!(says(&told(&client), "wander.base.woodcarver.grain"));
        assert_eq!(count_of(&server, "base:oak_log"), 8, "shown, not taken");
        assert!(server.meet(1, carver));
        assert_eq!((count_of(&server, "base:oak_log"), count_of(&server, "base:gold_coin")), (0, 1), "eight logs for a coin");
        skill_to(&mut server, "core:crafting", 12);
        let before = xp_in(&server, "core:crafting");
        assert!(server.meet(1, carver));
        assert!(says(&told(&client), "wander.base.woodcarver.finer"));
        assert!(xp_in(&server, "core:crafting") > before, "a finer cut for one with the feel");
    }

    #[test]
    fn shearers_roll_a_fleece_for_a_beginner_and_buy_six_once() {
        let mut server = fixture();
        let id = start(&mut server, "shearers", (110, 128));
        let shearer = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:wool", 12);
        assert!(server.meet(1, shearer));
        assert!(xp_in(&server, "core:farming") > 0);
        assert_eq!(count_of(&server, "base:wool"), 12);
        assert!(server.meet(1, shearer));
        assert_eq!((count_of(&server, "base:wool"), count_of(&server, "base:gold_coin")), (6, 1));
        server.meet(1, shearer);
        assert_eq!(count_of(&server, "base:wool"), 6, "once");
    }

    #[test]
    fn an_apprentice_asks_a_master_for_a_lesson_in_each_skill_they_have() {
        let mut server = fixture();
        let id = start(&mut server, "apprentice", (110, 128));
        let youth = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        let client = listen(&mut server);
        assert!(server.meet(1, youth));
        assert!(says(&told(&client), "wander.base.apprentice.someday"), "a beginner is no master");
        assert_eq!(count_of(&server, "base:gold_coin"), 0);
        skill_to(&mut server, "core:farming", 15);
        skill_to(&mut server, "core:mining", 15);
        assert!(server.meet(1, youth));
        assert!(server.meet(1, youth));
        assert_eq!(count_of(&server, "base:gold_coin"), 2, "a lesson in each");
        let farming = xp_in(&server, "core:farming");
        server.meet(1, youth);
        assert_eq!((count_of(&server, "base:gold_coin"), xp_in(&server, "core:farming")), (2, farming), "once each");
    }

    #[test]
    fn the_hedge_witch_turns_three_herbs_to_a_mote_for_one_who_knows_herbs() {
        let mut server = fixture();
        let id = start(&mut server, "hedge_witch", (110, 128));
        let witch = members_of(&server, id, 0)[0];
        let herb = tagged(&server, "herb");
        let name = server.content.blocks.def(herb).name.clone();
        give_held(&mut server, &name, 3);
        assert!(server.meet(1, witch));
        assert!(xp_in(&server, "core:magic") > 0, "what the night air carries");
        assert_eq!(count_of(&server, &name), 3, "no mote for one who knows no herbs");
        skill_to(&mut server, "core:alchemy", 5);
        assert!(server.meet(1, witch));
        assert_eq!((count_of(&server, &name), count_of(&server, "base:aether_mote")), (0, 1));
        give_held(&mut server, &name, 3);
        server.meet(1, witch);
        assert_eq!(count_of(&server, &name), 3, "once");
    }

    /// Whether kind `name` can be picked for player 1 at land it suits.
    fn comes_at(server: &Server, name: &str, spot: IVec3) -> bool {
        let kind = event(server, name);
        (0..800u64).any(|seed| server.pick_event_in(1, spot, mix(seed, 3), false) == Some(kind))
    }

    /// Party `id`'s leader moved to `at`.
    fn lead_to(server: &mut Server, id: u64, at: Vec3) {
        let lead = server.wandering.active[&id].members[0].creature;
        server.creatures.live.get_mut(&lead).unwrap().body.position = at;
    }

    #[test]
    fn gamekeepers_come_only_after_a_poacher_at_work_and_tell_where_he_is() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (3.0 + i as f64 / 40.0)).find(|&day| (0.15..0.4).contains(&voxmar_engine::creature::daylight(day as u64))).expect("a dim hour");
        let spot = land_for(&server, event(&server, "gamekeepers"), Vec3::new(128.0, 200.0, 128.0), |_| true);
        assert!(!comes_at(&server, "gamekeepers", spot), "no poacher, no keepers");
        let poaching = start(&mut server, "poacher", (110, 128));
        lead_to(&mut server, poaching, spot.as_vec3() + Vec3::new(400.0, 0.0, 0.0));
        assert!(!comes_at(&server, "gamekeepers", spot), "not after one far off");
        lead_to(&mut server, poaching, spot.as_vec3() + Vec3::new(60.0, 0.0, 0.0));
        assert!(comes_at(&server, "gamekeepers", spot), "after the poacher");
        lead_to(&mut server, poaching, Vec3::new(110.5, 200.0, 128.5));
        let id = start(&mut server, "gamekeepers", (130, 128));
        let keeper = members_of(&server, id, 0)[0];
        let client = listen(&mut server);
        assert!(server.meet(1, keeper));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.toward"), "which way he went");
    }

    #[test]
    fn straggling_pilgrims_tell_where_the_shrine_went_and_pray_for_a_loaf() {
        let mut server = fixture();
        broad_day(&mut server);
        let spot = land_for(&server, event(&server, "stragglers"), Vec3::new(128.0, 200.0, 128.0), |_| true);
        assert!(!comes_at(&server, "stragglers", spot), "no shrine, nobody behind it");
        let shrine = start(&mut server, "shrine_bearer", (110, 128));
        lead_to(&mut server, shrine, spot.as_vec3() + Vec3::new(80.0, 0.0, 0.0));
        assert!(comes_at(&server, "stragglers", spot));
        lead_to(&mut server, shrine, Vec3::new(110.5, 200.0, 128.5));
        let id = start(&mut server, "stragglers", (130, 128));
        let pilgrim = members_of(&server, id, 0)[0];
        server.players.get_mut(&1).unwrap().praying.points = 0.0;
        give_held(&mut server, "base:bread", 1);
        let client = listen(&mut server);
        assert!(server.meet(1, pilgrim));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.toward"));
        assert!(server.meet(1, pilgrim));
        assert_eq!(count_of(&server, "base:bread"), 0);
        assert!(server.players[&1].praying.points > 0.0, "prayer restored");
    }

    #[test]
    fn the_gossip_tells_where_a_party_is_for_a_coin_and_takes_none_with_nothing_to_tell() {
        let mut server = fixture();
        let id = start(&mut server, "gossip", (110, 128));
        let gossip = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:gold_coin", 2);
        server.meet(1, gossip);
        assert_eq!(count_of(&server, "base:gold_coin"), 2, "nothing on the roads, nothing to sell");
        start(&mut server, "pedlar_and_hound", (150, 150));
        let client = listen(&mut server);
        assert!(server.meet(1, gossip));
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        let said = told(&client);
        assert!(said.iter().any(|(k, _)| k == "wander.toward"), "where the pedlar is: {said:?}");
    }

    #[test]
    fn kin_and_christenings_come_back_to_those_who_met_the_first_party() {
        let mut server = fixture();
        broad_day(&mut server);
        server.day += 6.0 * TICKS_PER_DAY as f64;
        let home = Vec3::new(128.5, 200.0, 128.5);
        server.players.get_mut(&1).unwrap().bed = Some(home);
        let comes = |server: &Server, name: &str| {
            let kind = event(server, name);
            let spot = land_for(server, kind, home, |_| true);
            (0..800u64).any(|seed| server.pick_event_in(1, spot, mix(seed, 3), false) == Some(kind))
        };
        let spot = land_for(&server, event(&server, "refugee_kin"), home, |_| true);
        server.players.get_mut(&1).unwrap().bed = Some(spot.as_vec3());
        server.players.get_mut(&1).unwrap().position = spot.as_vec3();
        assert!(!comes(&server, "refugee_kin") && !comes(&server, "christening"), "nobody met, nobody comes back");
        server.tally(1, "met", "base:refugees", 1);
        server.tally(1, "met", "base:wedding", 1);
        assert!(comes(&server, "refugee_kin"), "kin of the refugees taken in");
        assert!(comes(&server, "christening"), "the couple from the wedding");
        let mut server = fixture();
        let id = start(&mut server, "refugee_kin", (110, 128));
        let kin = members_of(&server, id, 0)[0];
        assert!(server.meet(1, kin));
        assert!(count_of(&server, "base:seed_mix") >= 4);
        let mut server = fixture();
        let id = start(&mut server, "christening", (110, 128));
        let priest = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, priest));
        assert!(has_effect(&server, "refreshed"));
    }

    #[test]
    fn the_poachers_gang_waits_only_for_one_who_met_him_and_pays_when_beaten() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (5.0 + i as f64 / 40.0)).find(|&day| (0.15..0.4).contains(&voxmar_engine::creature::daylight(day as u64))).expect("a dim hour");
        skill_to(&mut server, "core:attack", 10);
        skill_to(&mut server, "core:defence", 10);
        skill_to(&mut server, "core:hitpoints", 10);
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(128.0, 200.0, 128.0));
        let spot = land_for(&server, event(&server, "poachers_gang"), Vec3::new(128.0, 200.0, 128.0), |_| true);
        assert!(!comes_at(&server, "poachers_gang", spot), "a stranger to him");
        server.tally(1, "met", "base:poacher", 1);
        assert!(comes_at(&server, "poachers_gang", spot), "word got round");
        let id = start(&mut server, "poachers_gang", (110, 128));
        let poacher = members_of(&server, id, 0)[0];
        server.wandering.active.get_mut(&id).unwrap().stage = Stage::Won;
        server.players.get_mut(&1).unwrap().inventory.clear(|_| true);
        assert!(server.meet(1, poacher));
        assert!((2..=3).contains(&count_of(&server, "base:gold_coin")), "paid to be let go");
    }

    #[test]
    fn grateful_parents_and_a_journeyman_come_back_to_those_who_helped() {
        let mut server = fixture();
        broad_day(&mut server);
        server.day += 5.0 * TICKS_PER_DAY as f64;
        let spot = land_for(&server, event(&server, "grateful_parents"), Vec3::new(128.0, 200.0, 128.0), |_| true);
        server.players.get_mut(&1).unwrap().bed = Some(spot.as_vec3());
        server.players.get_mut(&1).unwrap().position = spot.as_vec3();
        assert!(!comes_at(&server, "grateful_parents", spot) && !comes_at(&server, "journeyman", spot), "nobody helped, nobody comes back");
        server.tally(1, "met", "base:lost_child", 1);
        server.tally(1, "met", "base:apprentice", 1);
        assert!(comes_at(&server, "grateful_parents", spot), "the lost child's parents");
        assert!(comes_at(&server, "journeyman", spot), "the apprentice, a journeyman now");
        let mut server = fixture();
        let id = start(&mut server, "grateful_parents", (110, 128));
        let parent = members_of(&server, id, 0)[0];
        assert!(server.meet(1, parent));
        assert!(count_of(&server, "base:gold_coin") >= 2 && count_of(&server, "base:bread") == 2);
        let mut server = fixture();
        let id = start(&mut server, "journeyman", (110, 128));
        let journeyman = members_of(&server, id, 0)[0];
        assert!(server.meet(1, journeyman));
        assert_eq!(count_of(&server, "base:gold_coin"), 2);
        let learned = ["core:farming", "core:mining", "core:crafting"].iter().filter(|s| xp_in(&server, s) > 0).count();
        assert_eq!(learned, 1, "one trade's lesson");
    }

    #[test]
    fn highwaymen_lie_in_wait_only_while_market_folk_are_on_the_road() {
        let mut server = fixture();
        broad_day(&mut server);
        skill_to(&mut server, "core:attack", 10);
        skill_to(&mut server, "core:defence", 10);
        skill_to(&mut server, "core:hitpoints", 10);
        let spot = land_for(&server, event(&server, "highwaymen"), Vec3::new(128.0, 200.0, 128.0), |_| true);
        assert!(!comes_at(&server, "highwaymen", spot), "an empty road");
        let market = start(&mut server, "market_folk", (110, 128));
        lead_to(&mut server, market, spot.as_vec3() + Vec3::new(60.0, 0.0, 0.0));
        assert!(comes_at(&server, "highwaymen", spot), "market day");
        let mut server = fixture();
        let id = start(&mut server, "highwaymen", (110, 128));
        let thief = members_of(&server, id, 0)[0];
        server.wandering.active.get_mut(&id).unwrap().stage = Stage::Won;
        assert!(server.meet(1, thief));
        assert!((2..=3).contains(&count_of(&server, "base:gold_coin")), "the purse given back");
    }

    #[test]
    fn a_late_pupil_and_beaters_say_where_their_party_went() {
        for (name, after) in [("late_pupil", "mage_tutors"), ("beaters", "hunting_party")] {
            let mut server = fixture();
            broad_day(&mut server);
            let spot = land_for(&server, event(&server, name), Vec3::new(128.0, 200.0, 128.0), |_| true);
            assert!(!comes_at(&server, name, spot), "{name}: nobody to come after");
            let ahead = start(&mut server, after, (110, 128));
            lead_to(&mut server, ahead, spot.as_vec3() + Vec3::new(60.0, 0.0, 0.0));
            assert!(comes_at(&server, name, spot), "{name}: after {after}");
            lead_to(&mut server, ahead, Vec3::new(110.5, 200.0, 128.5));
            let id = start(&mut server, name, (130, 128));
            let speaker = members_of(&server, id, 0)[0];
            let client = listen(&mut server);
            assert!(server.meet(1, speaker));
            assert!(told(&client).iter().any(|(k, _)| k == "wander.toward"), "{name}: which way");
        }
        let mut server = fixture();
        let id = start(&mut server, "late_pupil", (110, 128));
        let pupil = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:bread", 1);
        assert!(server.meet(1, pupil));
        assert_eq!(count_of(&server, "base:bread"), 0);
        assert!(xp_in(&server, "core:magic") > 0, "yesterday's lesson for a loaf");
    }

    #[test]
    fn the_rival_pedlar_undercuts_once_and_sneers_toward_the_other() {
        let mut server = fixture();
        let ahead = start(&mut server, "pedlar_and_hound", (150, 150));
        let id = start(&mut server, "rival_pedlar", (110, 128));
        let rival = members_of(&server, id, 0)[0];
        give_held(&mut server, "base:gold_coin", 2);
        assert!(server.meet(1, rival));
        assert_eq!(count_of(&server, "base:gold_coin"), 1);
        let bargains = ["base:bread", "base:seed_mix", "base:flint"].iter().map(|i| count_of(&server, i)).sum::<u32>();
        assert!(bargains >= 2, "one of the bargains");
        let client = listen(&mut server);
        assert!(server.meet(1, rival));
        assert!(told(&client).iter().any(|(k, _)| k == "wander.toward"), "where the other went");
        assert!(server.wandering.active.contains_key(&ahead));
    }

    /// Floor about land a rival pedlar comes to, and player 1 on it; where it is.
    fn rival_ground(server: &mut Server) -> IVec3 {
        let land = land_for(server, event(server, "rival_pedlar"), Vec3::new(128.0, 200.0, 128.0), |at| {
            let rival = server.content.wandering.get(event(server, "rival_pedlar"));
            (0..16).all(|i| {
                let (a, r) = (i as f32 * 0.4, 50.0 + i as f32 * 3.0);
                let biome = server.overworld.biomes().get(server.overworld.site((at.x + a.cos() * r) as i32, (at.z + a.sin() * r) as i32).biome);
                rival.biome_weight(&biome.name, &biome.tags) > 0.0
            })
        });
        let stone = server.content.blocks.expect("base:stone");
        let centre = ColumnPos::of_block(land);
        for dx in -4..=4 {
            for dz in -4..=4 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        column.set_block(IVec3::new(x, 199, z), stone);
                    }
                }
                server.world.insert(centre.offset(dx, dz), std::sync::Arc::new(column));
            }
        }
        server.players.get_mut(&1).unwrap().position = Vec3::new(land.x as f32 + 0.5, 200.0, land.z as f32 + 0.5);
        land
    }

    /// Whether a party comes after another, and which, is drawn by the world's count of
    /// rolls, never the tick it is tried on.
    #[test]
    fn a_follower_draws_by_the_worlds_count_not_the_tick() {
        let joined = |from: u64| {
            let mut server = fixture();
            server.meta.draw_key = 0x0d1a;
            broad_day(&mut server);
            let land = rival_ground(&mut server);
            server.tick = from;
            start(&mut server, "pedlar_and_hound", (land.x, land.z - 60));
            server.tick += ticks(server.content.wandering.rules().follow_after);
            let before: Vec<u64> = server.wandering.active.keys().copied().collect();
            let found = (0..400).find_map(|salt| {
                server.try_follower(1, salt);
                server.tick += 7;
                server.wandering.active.values().find(|i| !before.contains(&i.id)).map(|i| (salt, i.event, i.seed))
            });
            (found, server.meta.rolls)
        };
        let first = joined(3_000);
        assert!(first.0.is_some(), "no rival came: {first:?}");
        assert_eq!(joined(3_000 + 40_977), first, "the same tries later drew otherwise");
    }

    /// How long a party camps is drawn by its own count of rolls, never the tick: the same
    /// party set out later stays alike.
    #[test]
    fn a_partys_stay_draws_by_its_own_count_not_the_tick() {
        let stays = |from: u64| {
            let mut server = fixture();
            server.tick = from;
            let id = start(&mut server, "puppeteers", (110, 128));
            assert!(run_until(&mut server, id, 60, |inst| matches!(inst.plan, Plan::Camp { stay_until: Some(_), .. })), "the party settles");
            let inst = &server.wandering.active[&id];
            let Plan::Camp { stay_until: Some(until), .. } = inst.plan else { unreachable!() };
            (until - server.tick, inst.rolls.get())
        };
        // A whole number of every creature's beats on (see the creatures' own test), so
        // the party walks alike.
        let beats: u64 = (4..7).chain(20..23).product();
        let first = stays(2_000);
        assert!(first.1 > 0, "its stay drew nothing: {first:?}");
        assert_eq!(stays(2_000 + 7 * beats), first, "the same party set out later stayed otherwise");
    }

    /// A party that comes after another (`needs.after`) joins it through the gap that keeps
    /// other parties off, `follow_after` seconds on at the earliest, and only once for each.
    #[test]
    fn a_party_that_comes_after_another_joins_it_through_the_gap_once() {
        let mut server = fixture();
        broad_day(&mut server);
        let land = rival_ground(&mut server);
        let pedlar = start(&mut server, "pedlar_and_hound", (land.x, land.z - 60));
        assert!(!server.wandering_welcome(1), "the gap after the pedlar keeps others off");
        let follow = |server: &mut Server| {
            let before: Vec<u64> = server.wandering.active.keys().copied().collect();
            (0..400).find_map(|salt| {
                server.try_follower(1, salt);
                server.wandering.active.values().find(|i| !before.contains(&i.id)).map(|i| i.event)
            })
        };
        assert_eq!(follow(&mut server), None, "too soon after the pedlar set out");
        server.tick += ticks(server.content.wandering.rules().follow_after);
        assert_eq!(follow(&mut server), Some(event(&server, "rival_pedlar")), "the rival comes after the pedlar");
        assert!(server.wandering.followed.contains(&pedlar));
        assert_eq!(follow(&mut server), None, "once for each party it follows");
        // A second pedlar about: a rival may follow it, but never the first again.
        let second = start(&mut server, "pedlar_and_hound", (land.x + 40, land.z - 50));
        server.tick += ticks(server.content.wandering.rules().follow_after);
        assert_eq!(follow(&mut server), Some(event(&server, "rival_pedlar")), "the second pedlar has its rival");
        assert!(server.wandering.followed.contains(&second));
        assert_eq!(follow(&mut server), None, "both joined, none more");
        // A rival that came on its own, not through the gap, still counts as the third
        // pedlar's: no other rival joins it after.
        let third = start(&mut server, "pedlar_and_hound", (land.x - 40, land.z - 50));
        server.tick += ticks(server.content.wandering.rules().follow_after);
        let (x, z) = (land.x - 40, land.z - 40);
        let rival = start(&mut server, "rival_pedlar", (x, z));
        server.mark_forerunner(rival, event(&server, "rival_pedlar"), IVec3::new(x, 200, z));
        assert!(server.wandering.followed.contains(&third));
        assert_eq!(follow(&mut server), None, "a rival that came on its own counts as the join");
        // With every pedlar about joined, no rival comes by the ordinary pick either.
        assert!(!comes_at(&server, "rival_pedlar", IVec3::new(x, 200, z)), "a joined pedlar draws no second rival");
        server.wandering.followed.remove(&third);
        assert!(comes_at(&server, "rival_pedlar", IVec3::new(x, 200, z)), "an unjoined one does");
    }

    /// Callers come only to a home that has come far enough (`home_is`), judged at the home
    /// as `/home` judges it: fireside guests to a homely one, the hedgehog to a cosy one.
    #[test]
    fn callers_come_only_to_a_home_that_has_come_far_enough() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (4.0 + i as f64 / 40.0)).find(|&day| (0.1..0.5).contains(&voxmar_engine::creature::daylight(day as u64))).expect("an evening");
        let home = IVec3::new(128, 200, 128);
        // A hearth home, not burning, and the bed beside it: callers judge the bed's home.
        let player = server.players.get_mut(&1).unwrap();
        (player.home, player.bed) = (Some(home.as_vec3()), Some(home.as_vec3()));
        let spot = IVec3::new(128, 200, 150);
        let bare = ["base:hearthstone", "base:sleeping_bag", "base:candle"];
        for (i, name) in bare.iter().enumerate() {
            place(&mut server, name, home + IVec3::new(i as i32 + 1, 0, 0));
        }
        assert!(!comes_at(&server, "fireside_guests", spot), "a bare camp draws no callers");
        // Seats, a table, stores, more light, and sound plank walls and roof over it all
        // on a floor: homely.
        for (i, name) in ["base:oak_stool", "base:oak_stool", "base:oak_table", "base:crate", "base:crate", "base:candle", "base:candle"].iter().enumerate() {
            place(&mut server, name, home + IVec3::new(i as i32 - 3, 0, 2));
        }
        for x in -6..=6 {
            for z in -6..=6 {
                place(&mut server, "base:stone", home + IVec3::new(x, -1, z));
                place(&mut server, "base:oak_planks", home + IVec3::new(x, 4, z));
                if x.abs() == 6 || z.abs() == 6 {
                    for y in 0..4 {
                        place(&mut server, "base:oak_planks", home + IVec3::new(x, y, z));
                    }
                }
            }
        }
        assert!(comes_at(&server, "fireside_guests", spot), "a homely home has callers");
        assert!(!comes_at(&server, "hedgehog_lodger", spot), "homely is not yet cosy");
        // Far from home, nobody calls there.
        let player = server.players.get_mut(&1).unwrap();
        (player.home, player.bed) = (Some(Vec3::new(600.0, 200.0, 600.0)), Some(Vec3::new(600.0, 200.0, 600.0)));
        assert!(!comes_at(&server, "fireside_guests", spot));
    }

    /// Wanderers who call on a home ask where home is as respawn does (`home_spot`): a
    /// hearth burning, else the last bed. A hearth gone cold is no home to call on.
    #[test]
    fn neighbours_call_only_on_a_home_whose_hearth_burns_or_a_bed() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (4.0 + i as f64 / 40.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) >= 1.0).expect("midday");
        let core = IVec3::new(128, 199, 128);
        let player = server.players.get_mut(&1).unwrap();
        (player.home, player.bed, player.kindled) = (Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5)), None, true);
        player.position = Vec3::new(128.5, 200.0, 140.5);
        let spot = IVec3::new(128, 200, 150);
        assert!(!comes_at(&server, "neighbours", spot), "a cold hearth is no home to call on");
        server.lit.insert(core, crate::hearth::Lit { position: core.to_array(), structure: "base:hearth".into(), until: u64::MAX, owner: None });
        assert!(comes_at(&server, "neighbours", spot), "a burning one is");
        server.lit.clear();
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(130.5, 200.0, 130.5));
        assert!(comes_at(&server, "neighbours", spot), "and with it cold, the last bed is home");
    }

    /// The beekeeper comes by day only to a home with a bee garden: a skep of bees among
    /// flowers. An empty skep is no draw.
    #[test]
    fn the_beekeeper_comes_only_to_a_home_with_bees() {
        let mut server = fixture();
        server.day = (0..40).map(|i| TICKS_PER_DAY as f64 * (4.0 + i as f64 / 40.0)).find(|&day| voxmar_engine::creature::daylight(day as u64) >= 1.0).expect("midday");
        let home = IVec3::new(128, 200, 128);
        // A hearth home, not burning, and the bed beside it: callers judge the bed's home.
        let player = server.players.get_mut(&1).unwrap();
        (player.home, player.bed) = (Some(home.as_vec3()), Some(home.as_vec3()));
        let spot = IVec3::new(128, 200, 150);
        for (i, name) in ["base:hearthstone", "base:sleeping_bag", "base:dandelion", "base:dandelion", "base:dandelion"].iter().enumerate() {
            place(&mut server, name, home + IVec3::new(i as i32 + 1, 0, 0));
        }
        place(&mut server, "base:skep", home + IVec3::new(0, 0, 2));
        assert!(!comes_at(&server, "beekeeper", spot), "an empty skep draws no beekeeper");
        place(&mut server, "base:bee_skep", home + IVec3::new(0, 0, 2));
        assert!(comes_at(&server, "beekeeper", spot), "bees at home draw the beekeeper");
    }

    /// No party hands over a step of the ages' main line (docs/wandering.md, "How often"):
    /// what a main quest asks a player to make is never a gift, and what it asks them to
    /// cook, mine or carry comes, if at all, short of the count and never again and again.
    #[test]
    fn no_party_does_a_main_quest_step_for_the_player() {
        use voxmar_engine::quest::Goal;
        let server = crate::test_server();
        let content = &server.content;
        let item = |name: &str| content.blocks.id(name).or_else(|| content.blocks.id(&format!("base:{name}")));
        let (mut made, mut counted): (Vec<BlockId>, Vec<(BlockId, u32)>) = (Vec::new(), Vec::new());
        for quest in content.quests.iter().filter(|q| q.branch == "main") {
            for goal in quest.tasks.iter().flatten() {
                match goal {
                    Goal::Craft(name, _) => made.extend(item(name)),
                    Goal::Cook(name, n) | Goal::Mine(name, n) | Goal::Have(name, n) => counted.extend(item(name).map(|i| (i, *n))),
                    _ => {}
                }
            }
        }
        assert!(made.contains(&content.blocks.expect("base:torch")), "the torch is a main-line make");
        fn gifts(r: &ResolvedReward, out: &mut Vec<(BlockId, u8)>) {
            match r {
                ResolvedReward::Item(item, (_, most)) => out.push((*item, *most)),
                ResolvedReward::OneOf(c) => c.iter().for_each(|r| gifts(r, out)),
                _ => {}
            }
        }
        for (_, e) in content.wandering.iter() {
            let lines = e.meetings.iter().flat_map(|m| m.lines.iter()).map(|l| (l.name.as_str(), &l.gives[..], Some(l.once)));
            let others = [("thanks", &e.thanks[..]), ("guide_thanks", &e.guide_thanks[..]), ("reveal", &e.reveal[..]), ("show", &e.show[..]), ("share", &e.share[..])];
            for (line, gives, once) in lines.chain(others.into_iter().map(|(n, g)| (n, g, None))) {
                let mut got = Vec::new();
                gives.iter().for_each(|r| gifts(r, &mut got));
                for (gift, most) in got {
                    let name = &content.blocks.def(gift).name;
                    assert!(!made.contains(&gift), "{} {line} gives {name}, which a main quest asks the player to make", e.def.name);
                    if let Some(&(_, asked)) = counted.iter().find(|(i, _)| *i == gift) {
                        assert!((most as u32) < asked, "{} {line} gives up to {most} {name}, all a main quest asks", e.def.name);
                        assert!(once != Some(Once::Always), "{} {line} gives {name} over and over", e.def.name);
                    }
                }
            }
        }
    }

    /// What a wanderer gives, how many and which of several, is drawn by the player's own
    /// count of rewards: the same rewards given later, after the world rolled otherwise,
    /// give the same, and each draw counts on.
    #[test]
    fn rewards_draw_by_the_players_count_not_the_tick() {
        let given = |tick: u64, others: usize| {
            let mut server = fixture();
            server.meta.draw_key = 0x0d1a;
            server.players.get_mut(&1).unwrap().draw_key = crate::player::player_draw_key(0x0d1a, "Tëster");
            server.tick = tick;
            let (stick, cobble) = (server.content.blocks.expect("base:stick"), server.content.blocks.expect("base:cobblestone"));
            let mut got = Vec::new();
            for _ in 0..16 {
                for _ in 0..others {
                    server.meta.next_loot();
                    server.meta.next_roll(crate::save::ROLL_LITTER);
                }
                let player = server.players.get_mut(&1).unwrap();
                player.inventory.clear(|_| true);
                server.hand_out(1, 0, &[ResolvedReward::Item(stick, (1, 4)), ResolvedReward::OneOf(vec![ResolvedReward::Item(stick, (1, 1)), ResolvedReward::Item(cobble, (1, 1))])]);
                let inventory = &server.players[&1].inventory;
                got.push((inventory.count(stick), inventory.count(cobble)));
                server.tick += 37;
            }
            (got, server.players[&1].draws.rewards)
        };
        let first = given(1_000, 0);
        assert_eq!(first.1, 16 * 3, "each count, pick and picked count drawn on");
        assert!(first.0.windows(2).any(|w| w[0].0 != w[1].0), "sixteen rewards gave the same count: {:?}", first.0);
        assert!(first.0.iter().any(|g| g.1 > 0) && first.0.iter().any(|g| g.1 == 0), "sixteen picks of one of two all went alike: {:?}", first.0);
        assert_eq!(given(1_377, 3), first, "the same rewards given later, after other rolls, gave otherwise");
    }

    /// Which greeting a wanderer says is drawn by the listener's own count of greetings heard,
    /// never the tick: the same greetings heard later, after the world's other rolls, are the
    /// same words, and each one said counts on.
    #[test]
    fn a_greeting_goes_by_the_listeners_count_not_the_tick() {
        let heard = |tick: u64, rolls: u32| {
            let mut server = fixture();
            server.meta.draw_key = 0x0d1a;
            let (client, connection) = voxmar_protocol::local_pair();
            server.players.get_mut(&1).unwrap().connection = connection;
            let id = start(&mut server, "pedlar_and_hound", (68, 128));
            let pedlar = server.wandering.active[&id].members[0].creature;
            for _ in 0..rolls {
                server.meta.next_roll(crate::save::ROLL_SUMMON);
            }
            // One hide is not enough for a swap: only the greeting.
            give_held(&mut server, "base:hide", 1);
            server.tick = tick;
            while let Ok(Some(_)) = client.try_recv() {}
            let mut said = Vec::new();
            for _ in 0..12 {
                assert!(server.meet(1, pedlar));
                while let Ok(Some(message)) = client.try_recv() {
                    // Who said it, and the words said.
                    if let voxmar_protocol::ServerMessage::Chat { text, .. } = message {
                        said.push(format!("{text:?}"));
                    }
                }
                server.tick += 37;
            }
            (said, server.players[&1].draws.greets)
        };
        let first = heard(1_000, 0);
        assert_eq!(first.1, 12, "each greeting counted on");
        assert!(first.0.iter().any(|k| *k != first.0[0]), "12 greetings all alike: {:?}", first.0);
        assert_eq!(heard(1_377, 3), first, "the same greetings heard later, after other rolls, were otherwise");
    }

    /// A party `/wander start` brings is cast by the world's count of rolls, never the tick:
    /// the same ask later casts the same party, and the next ask another.
    #[test]
    fn a_wander_start_casts_by_the_worlds_count_not_the_tick() {
        let cast = |tick: u64| {
            let mut server = fixture();
            server.meta.draw_key = 0x0d1a;
            server.tick = tick;
            let mut seeds = Vec::new();
            for _ in 0..2 {
                server.wander_command(Source::Player(1), &["start".to_owned(), "pedlar_and_hound".to_owned()]).expect("started");
                let newest = server.wandering.active.keys().max().copied().expect("a party");
                seeds.push(server.wandering.active[&newest].seed);
                server.end_all_events();
                server.tick += 37;
            }
            (seeds, server.meta.rolls)
        };
        let first = cast(1_000);
        assert_eq!(first.1, 2, "each ask counted on");
        assert_ne!(first.0[0], first.0[1], "two asks cast alike");
        assert_eq!(cast(1_377), first, "the same asks later cast otherwise");
    }
}
