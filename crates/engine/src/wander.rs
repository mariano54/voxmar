//! Wandering events: parties of people, animals and monsters that come by on their own
//! errands, as content (each package's `wandering.ron`).
//!
//! An event is a cast (groups of a species, with a role each), a script that tells the
//! cast where to go (march past the player, patrol a loop, camp a while, shadow a
//! stranger) and, for those who talk, a meeting: what right-clicking one of them says and
//! gives. The server starts one now and then near a player where the land, the hour and
//! the player's progress suit it, keeps them at their script with the same errands
//! villagers use, and lets them go once they are far and unseen (docs/wandering.md).
//!
//! Everything a number or a list can be is here, not in code: how often events come, how
//! many at once, which biomes and hours suit each, who walks with whom, what a meeting
//! asks and gives. Names that point at other content (species, items, skills, effects) are
//! checked when the content loads.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::cooking::CookingRegistry;
use crate::creature::{CreatureId, CreatureRegistry, Kind};
use crate::effect::{EffectId, EffectRegistry};
use crate::skill::{SkillId, SkillRegistry};
use crate::weather::WeatherTable;

/// How wandering events come and go (`rules` in `wandering.ron`; a later package's fields
/// replace an earlier one's, Design draft 91).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// Seconds between tries to start an event near each player.
    pub scan_every: f32,
    /// The chance a try starts one, before the events' own weights.
    pub chance: f32,
    /// Seconds after one began near a player before another may.
    pub gap: f32,
    /// At most this many events (running now) with a member within `near` blocks of a player.
    pub most_near: usize,
    pub near: f32,
    /// At most this many events running in the whole world.
    pub most_total: usize,
    /// At most this many people (villager-kind members of events) alive at once, apart
    /// from the settlements' own.
    pub people_most: usize,
    /// At most this many wanderers led to a player's home may stay in the world (they are
    /// saved with it); later ones go their own way.
    pub companions_most: usize,
    /// How far from the player a cast first stands, no nearer and no further.
    pub appear_near: f32,
    pub appear_far: f32,
    /// An event whose members are all this far from every player, or gone, ends.
    pub leave: f32,
    /// Seconds between a script's updates (the errands it hands its cast).
    pub errand_every: f32,
    /// Days of a new world before the first event.
    pub first_days: f32,
    /// How far a speaker is heard and answers, in blocks.
    pub speak_reach: f32,
    /// How much less likely an event is for a player who met it lately (a share of its
    /// weight), for as many seconds.
    pub repeat_weight: f32,
    pub repeat_memory: f32,
    /// Seconds after a party began before one that comes after it (`needs.after`) may
    /// join it near the same player: such a follower comes through the `gap` and past
    /// `most_near`, once for each party it follows, or a chain could never be met alone.
    pub follow_after: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            scan_every: 20.0,
            chance: 0.25,
            gap: 600.0,
            most_near: 1,
            near: 110.0,
            most_total: 8,
            people_most: 16,
            companions_most: 24,
            appear_near: 55.0,
            appear_far: 85.0,
            leave: 150.0,
            errand_every: 0.5,
            first_days: 0.4,
            speak_reach: 4.0,
            repeat_weight: 0.15,
            repeat_memory: 1800.0,
            follow_after: 90.0,
        }
    }
}

crate::content::settable! {
    /// The rules one package sets: any of them.
    SetRules for Rules {
        scan_every: f32, chance: f32, gap: f32, most_near: usize, near: f32, most_total: usize, people_most: usize,
        companions_most: usize,
        appear_near: f32, appear_far: f32, leave: f32, errand_every: f32, first_days: f32, speak_reach: f32,
        repeat_weight: f32, repeat_memory: f32, follow_after: f32,
    }
}

impl Rules {
    /// Whether these rules can hold.
    pub fn check(&self) -> Result<(), RegistryError> {
        let error = |why: String| Err(RegistryError(format!("wandering rules: {why}")));
        if !(self.scan_every.is_finite() && self.scan_every >= 0.5) {
            return error(format!("`scan_every` ({}) must be at least half a second", self.scan_every));
        }
        if !(0.0..=1.0).contains(&self.chance) || !(0.0..=1.0).contains(&self.repeat_weight) {
            return error("`chance` and `repeat_weight` are shares from 0 to 1".into());
        }
        if !(self.follow_after.is_finite() && self.follow_after >= 0.0) {
            return error(format!("`follow_after` ({}) must be 0 seconds or more", self.follow_after));
        }
        if !(self.errand_every.is_finite() && self.errand_every >= 0.1) {
            return error(format!("`errand_every` ({}) must be at least 0.1 seconds", self.errand_every));
        }
        if !(self.appear_near > 0.0 && self.appear_near <= self.appear_far) {
            return error("`appear_near` must be above 0 and no more than `appear_far`".into());
        }
        if self.leave <= self.appear_far {
            return error(format!("`leave` ({}) must be further than `appear_far` ({}), or events end as they begin", self.leave, self.appear_far));
        }
        if self.most_near == 0 || self.most_total < self.most_near || self.people_most == 0 {
            return error("`most_near`, `most_total` and `people_most` must allow at least one event, and `most_total` at least `most_near`".into());
        }
        Ok(())
    }
}

/// What a package's `wandering.ron` holds.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WanderFile {
    #[serde(default)]
    pub rules: Option<SetRules>,
    #[serde(default)]
    pub events: Vec<EventDef>,
    /// How the kinds of creature answer what a player holds, in any event.
    #[serde(default)]
    pub reactions: Vec<Reaction>,
}

/// What a package gives: its rules, its events (names qualified) and its reactions.
#[derive(Clone, Debug)]
pub struct WanderPackage {
    pub rules: Option<SetRules>,
    pub events: Vec<EventDef>,
    pub reactions: Vec<Reaction>,
}

/// How a kind of creature answers a player who holds an item with a tag, whatever the
/// event: one rule in data that every present and later event inherits (docs/wandering.md,
/// "Reactions"). It applies within `range` blocks of the party's lead (of its first
/// lurker, in an ambush), to a player whose combat level is within `combat` if that is set.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reaction {
    /// The tag the held item has.
    pub held: String,
    pub range: f32,
    /// Which kinds of creature answer so.
    pub kinds: Vec<Kind>,
    pub response: Response,
    /// The combat levels (least and most) of the players it answers, any if unset.
    #[serde(default)]
    pub combat: Option<(u8, u8)>,
}

/// What a creature does about a held thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Response {
    /// Does not stop, and does not spring a trap: it sees what it is up against.
    Wary,
    /// Keeps away: walks off from the player while it is held.
    Shy,
    /// Comes to the player, and stays near while it is held.
    Draw,
}

/// One kind of event as `wandering.ron` writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    pub name: String,
    /// How likely it is among the events that suit a place, about 1 for a common one.
    #[serde(default = "one")]
    pub weight: f32,
    /// Biome names and `tag:` selectors with a weight each, as a species' habitat; empty
    /// is anywhere.
    #[serde(default)]
    pub biomes: Vec<(String, f32)>,
    /// The daylight (0 at night, 1 by day) it comes in, least and most. Daylight is a full
    /// 1 for more than half the day and 0 for most of the night, so a day party's window
    /// ends at 1: one that ends short of it comes only at dawn and dusk.
    #[serde(default = "any_light")]
    pub daylight: (f32, f32),
    /// What the player must have reached.
    #[serde(default)]
    pub needs: Needs,
    /// Who comes. The first group's first member leads.
    pub cast: Vec<Group>,
    pub script: Script,
    /// What those who talk say and give, a meeting for each group that speaks.
    #[serde(default)]
    pub meetings: Vec<Meeting>,
    /// Whether the party stops for a player who comes near, and how.
    #[serde(default)]
    pub halt: Option<Halt>,
    /// What the party does when a player strikes one of its own.
    #[serde(default)]
    pub hurt: Hurt,
    /// What becomes of a captive when its captors are gone.
    #[serde(default)]
    pub freed: Option<Freed>,
    /// What befriending one of the party (the `Befriend` reward) comes to.
    #[serde(default)]
    pub guide: Option<Guide>,
    /// What a led one's guide gives the player who followed to the end (a `Lure` script).
    #[serde(default)]
    pub reveal: Vec<Reward>,
    /// What a `Gather` party works at: block names and `tag:` selectors. It comes only
    /// where one of them is within its `reach`.
    #[serde(default)]
    pub seek: Vec<String>,
    /// Where a `Gather` party that carries takes what it worked: block names and `tag:`
    /// selectors. It comes only where one of them is within its `reach` as well.
    #[serde(default)]
    pub deliver: Vec<String>,
    /// Whether the party warns off a player who comes near, then turns on them.
    #[serde(default)]
    pub guard: Option<Guard>,
    /// Whether the party bolts from a player who comes near, and how near a still or a
    /// skilled one may come.
    #[serde(default)]
    pub startle: Option<Startle>,
    /// What a camped party puts on for whoever stays to watch (a puppet play, a dance).
    #[serde(default)]
    pub show: Option<Show>,
    /// What a `Hunt` party hunts: wild creatures of these species. It comes only where one
    /// is within its `reach`.
    #[serde(default)]
    pub quarry: Vec<String>,
    /// What a `Hunt` party gives the player who drew first blood on what it took, besides
    /// the kill's own loot, with the event's `share` line. Without it, they keep it all.
    #[serde(default)]
    pub share: Vec<Reward>,
    /// Seconds it lasts at most; it ends sooner when it has gone by.
    #[serde(default = "lifetime")]
    pub lifetime: f32,
}

/// A show a `Camp` party puts on while it stays: a player who watches it, within `near`
/// blocks of its leader for `seconds` together, is given `gives` once, with the event's
/// `show` line. Walking off starts the count again.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show {
    pub near: f32,
    pub seconds: f32,
    pub gives: Vec<Reward>,
}

fn one() -> f32 {
    1.0
}

fn greet() -> String {
    "greet".into()
}

fn one_u8() -> u8 {
    1
}

fn any_light() -> (f32, f32) {
    (0.0, 1.0)
}

fn lifetime() -> f32 {
    900.0
}

/// What a player must have reached for an event to come near them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Needs {
    /// Whether they have a home (a hearth they woke at): `Some(false)` is for those
    /// still without, `Some(true)` for those with one.
    pub home: Option<bool>,
    /// The combat level range: least and most.
    pub combat: Option<(u8, u8)>,
    /// World days that have passed.
    pub days: f32,
    /// How far from where the world began it comes, least and most, in blocks: the far
    /// parts of the world have the rare parties.
    pub away: Option<(f32, f32)>,
    /// It comes only within this many blocks of a settlement of the world's plan (the
    /// town watch keeps near its town).
    pub town: Option<f32>,
    /// It comes only while rain or snow falls where it would appear (`Some(true)`), or only
    /// while none does (`Some(false)`).
    pub wet: Option<bool>,
    /// It comes only to a player within this many blocks of their home.
    pub near_home: Option<f32>,
    /// It comes only while a party of this kind is about, within so many blocks of where
    /// it would appear: those who come after another (the keepers after a poacher).
    pub after: Option<(String, f32)>,
    /// It comes only to a player who has met a party of this kind before (spoken with it,
    /// walked with it, freed it): one visit sets up the next.
    pub met: Option<String>,
    /// It comes only to a player whose home has reached this step: an effect (a home step
    /// such as `homely`) whose condition holds at their home, judged as `/home` judges it.
    /// Callers who come only to a cosy home.
    pub home_is: Option<String>,
}

/// A party that stops for a player: when one comes within `near` blocks of its leader, it
/// stands for `seconds` (least and most), then goes on, and does not stop again for
/// `cooldown` seconds. Talking to it keeps it standing.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Halt {
    pub near: f32,
    pub seconds: (f32, f32),
    #[serde(default = "cooldown")]
    pub cooldown: f32,
}

fn cooldown() -> f32 {
    90.0
}

/// Where a befriended one is led, and what it comes to: it follows the player for up to
/// `follow_for` seconds and, at `arrive`, thanks them (`arrived` is said, with `thanks`) and
/// stays there, a person of the land. Led nowhere in time, it is lost again.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guide {
    pub follow_for: f32,
    pub arrive: Arrival,
    #[serde(default)]
    pub thanks: Vec<Reward>,
}

/// Where a follower is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Arrival {
    /// The player's home.
    Home,
    /// Any settlement.
    Town,
}

/// A party that keeps players off (a sow with her young): a player within `near` blocks of
/// any member is warned (they stand and face them, and the event's `warn` line is told
/// them); one who stays `warn` seconds, or comes within half of `near`, is set upon by those
/// that fight, while those that cannot run. Back beyond `calm` blocks before that and the
/// warning is over. A player they shy from (a torch) or are drawn to (food) is not warned.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guard {
    pub near: f32,
    pub warn: f32,
    pub calm: f32,
}

/// A party that bolts (a herd of deer): a player who comes within `near` blocks, less
/// `stalk.1` a level of skill `stalk.0`, no nearer than `least`, sends it running; one who
/// has stood still for `still_for` seconds is only noticed at `still` blocks.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Startle {
    pub near: f32,
    pub least: f32,
    pub still: f32,
    #[serde(default = "still_for")]
    pub still_for: f32,
    #[serde(default)]
    pub stalk: Option<(String, f32)>,
}

fn still_for() -> f32 {
    2.0
}

/// What a party does when a player strikes one of its members.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Hurt {
    /// They run, and the party breaks up and goes (the people who cannot fight).
    #[default]
    Scatter,
    /// Those who fight turn on the player and the rest run.
    Avenge,
    /// Nothing changes: what already fights, fights.
    Ignore,
}

/// What a freed captive does: it thanks whoever freed it (`thanks` is said, with these
/// rewards), then with chance `follow` it follows them, for `follow_for` seconds or until
/// it reaches their home, where it settles; else it goes its own way, a peaceful wanderer
/// of the land for as long as anyone is near. Its words are `thanks` (at the moment) and
/// `freed` (when talked to after).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Freed {
    pub follow: f32,
    pub follow_for: f32,
    #[serde(default)]
    pub thanks: Vec<Reward>,
}

/// A group of one species in a cast.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub species: String,
    #[serde(default = "one_one")]
    pub count: (u8, u8),
    #[serde(default)]
    pub role: Role,
    /// Its members come young (a sow's piglets, a cow's calf), and grow up as kept young do.
    #[serde(default)]
    pub young: bool,
}

fn one_one() -> (u8, u8) {
    (1, 1)
}

/// What a group does beside the leader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Role {
    /// The head of the party: the first group, one member (the others of it follow).
    #[default]
    Lead,
    /// Walks behind the leader in a loose file.
    Follow,
    /// Keeps to the leader's sides, a little ahead.
    Flank,
    /// Trails far behind, the last of the party.
    Rear,
    /// Goes on ahead of the leader and waits for them to come up.
    Ahead,
    /// Keeps close to the first member of the group at this index.
    Pet(usize),
    /// Has no errand: acts by its own nature where the party stands.
    Free,
    /// Part of a herd: keeps loosely about the leader, wherever it grazes.
    Herd,
    /// Held by the rest: walks close behind the leader until every other member is gone or
    /// the bonds are cut (the `Free` reward), then is a freed captive (`freed`).
    Captive,
    /// After the leader (a `Chase`): runs the leader down. Caught, they stand round it.
    Chaser,
    /// Lies in wait about the leader (an `Ambush`), crouched, until the trap is sprung.
    Lurk,
}

/// Where the cast goes.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum Script {
    /// Walks a line that passes the player `pass` blocks away at its nearest (least and
    /// most) and goes on `length` blocks.
    March { pass: (f32, f32), length: f32, pace: f32 },
    /// Walks a ring of `stops` places round where they began, `radius` out, pausing at
    /// each, `laps` times round, then leaves.
    Patrol { radius: f32, stops: u8, pause: (f32, f32), laps: u8 },
    /// Comes to a place in the open `near` blocks off the player's way, stays `stay`
    /// seconds (least and most) milling about within `mill` blocks, then marches `away`.
    Camp { near: f32, stay: (f32, f32), mill: f32, away: f32 },
    /// Keeps `keep` blocks (least and most) from the nearest player, standing still when
    /// looked at, for `seconds`, then goes.
    Shadow { keep: (f32, f32), seconds: f32 },
    /// Wanders about the place it began, within `radius` blocks, pausing `pause` seconds
    /// (least and most) at each spot, sitting if `rest`, for `seconds`, then goes.
    Roam { radius: f32, pause: (f32, f32), rest: bool, seconds: f32 },
    /// Grazes along: short legs of `leg` blocks (least and most) in a slowly turning
    /// direction with a pause between, for `seconds`, then goes.
    Drift { leg: (f32, f32), pause: (f32, f32), seconds: f32 },
    /// The leader runs a line that passes the player `pass` blocks away at its nearest
    /// (least and most) and on `length` blocks, at `pace`, with its `Chaser`s on its heels
    /// at `gain` times its speed. Run down, it is held `hold` seconds (least and most),
    /// the stage `Caught`, then marched on by them.
    Chase { pass: (f32, f32), length: f32, pace: f32, gain: f32, hold: (f32, f32) },
    /// The leader (the bait) settles `near` blocks short of the player on their way, the
    /// `Lurk`ers about it. A player who comes within `trigger` blocks of any of them springs
    /// it `delay` seconds (least and most) later; sprung, they turn on that player. Won (every
    /// lurker gone), the bait stays `hold` seconds (least and most). Nobody comes in
    /// `patience` seconds, or a `Wary` player stands by, and they go `away` blocks.
    Ambush { near: f32, trigger: f32, delay: (f32, f32), patience: f32, away: f32, hold: (f32, f32) },
    /// The leader waits to be followed, then keeps `keep` blocks (least and most) ahead of
    /// the nearest player, along a line `length` blocks long: waiting when they fall behind
    /// (and, after `patience` seconds without a follower, giving up), and when they have come
    /// all the way, the `reveal` is theirs and it goes `linger` seconds later.
    Lure { keep: (f32, f32), length: f32, pace: f32, patience: f32, linger: f32 },
    /// Everyone who walks works: each goes to the nearest block the event `seek`s within
    /// `reach` blocks of where the party began (one each), stands at it `work` seconds
    /// (least and most), then the next, until the party has worked `visits` times, then
    /// they go. A block that is gone (picked, cut) sends its worker to another. With
    /// `carry`, each worker then takes what it worked to the nearest block the event
    /// `deliver`s to (or back to where the party began) before the next.
    Gather {
        reach: f32,
        work: (f32, f32),
        visits: u8,
        #[serde(default)]
        carry: Option<Carry>,
    },
    /// Goes after the nearest wild creature of the event's `quarry` within `reach` blocks
    /// of its leader, stalking the last of the way; when one of the party comes within
    /// `strike` blocks of it, it falls to them (the stage `Took`), and they stand over it a
    /// moment. After `takes` kills, or `seconds`, or with nothing left in reach, they go.
    Hunt { reach: f32, strike: f32, takes: u8, seconds: f32 },
    /// Walks to the nearest settlement of the world's plan within `reach` blocks, at
    /// `pace`. A player who keeps within `near` blocks of the leader for most of the way
    /// (30 seconds of it at least) and is with it when it arrives is given the event's
    /// `reveal`, with the line `arrived`; the stage is `Arrived`. It comes only where a
    /// settlement is within `reach`, and not so near that there is no way to walk.
    Journey { reach: f32, pace: f32, near: f32 },
}

/// What a `Gather` worker does with what it worked.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carry {
    /// Seconds spent setting it down (least and most).
    pub set: (f32, f32),
    /// Planted crops within this many blocks of where it is set down grow on by this many
    /// stages (water poured on a field).
    #[serde(default)]
    pub tend: Option<(f32, u8)>,
}

/// What speakers of an event say and give.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meeting {
    /// The group that speaks: an index into the cast.
    #[serde(default)]
    pub who: usize,
    /// The name of the greeting said when no line applies (`greet`), and how many ways
    /// it is said: the first by that name, the others with `_2`, `_3`... after it.
    #[serde(default = "greet")]
    pub greet: String,
    #[serde(default = "one_u8")]
    pub greets: u8,
    /// Tried in order; the first that applies is said and given. If none applies, the
    /// greeting is said.
    pub lines: Vec<LineDef>,
}

/// One thing a speaker may say and give.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineDef {
    /// The line's name; its words are the lang key `wander.NAMESPACE.EVENT.NAME`, and the
    /// greeting (the line that applies to anyone, last) is named `greet`.
    pub name: String,
    #[serde(default)]
    pub needs: Vec<Need>,
    /// What the player must hold in hand, which it takes.
    #[serde(default)]
    pub wants: Option<(String, u8)>,
    /// Instead of `wants`: items (and how many) of which each party asks for one, picked
    /// by its seed, so two meetings with a kind need not ask the same. Until the player
    /// holds it, a speaker whose line otherwise applies says what it asks (the line's
    /// `NAME_ask` words, with `{thing}` and `{count}`) in place of its greeting.
    #[serde(default)]
    pub asks: Vec<(String, u8)>,
    #[serde(default)]
    pub gives: Vec<Reward>,
    #[serde(default)]
    pub once: Once,
}

/// How often a line may apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Once {
    /// Every time.
    #[default]
    Always,
    /// Once to each player in a party.
    Player,
    /// Once to anyone in a party; after that, the next line.
    Party,
}

/// How far a script has got, for the lines that go with it. Every party starts `Passing`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Stage {
    #[default]
    Passing,
    /// A `Chase` run down: the quarry is held.
    Caught,
    /// An `Ambush` set and waiting.
    Waiting,
    /// An `Ambush` sprung: the lurkers are on the player.
    Sprung,
    /// An `Ambush` won: the lurkers are gone, the bait left standing. Or a bout (`Spar`)
    /// won by the player.
    Won,
    /// A bout (`Spar`) lost by the player.
    Lost,
    /// A `Lure` on its way, a follower behind.
    Leading,
    /// A `Lure` or a `Journey` at its journey's end.
    Arrived,
    /// A `Hunt` that has taken something.
    Took,
}

impl Stage {
    /// The script this stage belongs to, if any is needed.
    fn belongs_to(self, script: &Script) -> bool {
        match self {
            Stage::Passing => true,
            Stage::Caught => matches!(script, Script::Chase { .. }),
            Stage::Waiting | Stage::Sprung | Stage::Won => matches!(script, Script::Ambush { .. }),
            Stage::Leading => matches!(script, Script::Lure { .. }),
            Stage::Arrived => matches!(script, Script::Lure { .. } | Script::Journey { .. }),
            Stage::Took => matches!(script, Script::Hunt { .. }),
            // A bout's, checked by the lines that spar.
            Stage::Lost => false,
        }
    }
}

/// What a player must be, for a line.
#[derive(Clone, Debug, Deserialize)]
pub enum Need {
    /// The party is at this stage of its script.
    When(Stage),
    /// The player struck a member of this group of the cast (the party did not turn on them
    /// for it, or it has been left behind): the one who helped, or harmed.
    Struck(usize),
    /// At least this level in a skill.
    Skill(String, u8),
    /// Below this level in a skill: a teacher teaches no more than it knows to be useful.
    Below(String, u8),
    Home(bool),
    /// Hurt: below full health.
    Hurt,
    /// Holds an item with this tag (which is not taken).
    Holds(String),
}

/// What a line gives.
#[derive(Clone, Debug, Deserialize)]
pub enum Reward {
    /// Items, a count from the range.
    Item(String, (u8, u8)),
    /// Experience in a skill.
    Xp(String, f32),
    /// An effect for so many seconds.
    Effect(String, f32),
    /// Health restored.
    Heal(u8),
    /// The speaker follows the player toward the event's `guide` arrival.
    Befriend,
    /// Cuts the bonds of the party's captive, and does nothing for the captors' mood.
    Free,
    /// The party turns on the player, as if one of them had been struck.
    Anger,
    /// The party gives up what it was about and goes, bearing no grudge.
    Leave,
    /// Tells the player which way, and about how far, the nearest block of a name or a
    /// `tag:` lies within so many blocks of the speaker: true to the land, or that there is
    /// none near.
    Point(String, f32),
    /// Tells the player the name of the nearest settlement within so many blocks of the
    /// speaker, which way and how far, by the world's own plan (as `/locate settlement`
    /// does). Said only when there is one.
    Town(f32),
    /// Hands one member of this group of the cast to the player, kept as if tamed (a calf
    /// sold): it leaves the party. Said only when there is one, and room for it.
    Hand(usize),
    /// Cooks what the line takes, by the game's own recipes at a station with this tag
    /// (cooking.ron), never burnt: up to the second number of what is in hand, keeping the
    /// third of the cooked for the speaker's pot. Said only with something it cooks in hand.
    Cook(String, u8, u8),
    /// One of these (items, experience, effects, healing, a `Point`, a `Town` or a `Toward`), picked
    /// by chance among those that have something to tell where the speaker stands: a
    /// fortune. Said only when one of them has.
    OneOf(Vec<Reward>),
    /// Brings weather of this preset (weather.ron) over the land about the speaker for so
    /// many seconds, as `/weather` does: a rain dance answered.
    Weather(String, f32),
    /// Prayer points restored, up to the player's most. Said only to one short of them.
    Prayer(f32),
    /// The crops planted within so many blocks of the speaker grow on so many stages, as
    /// water poured on a field does: a blessing on the fields. Said only with one there to
    /// grow.
    Grow(f32, u8),
    /// The speaker spars with the player: a bout fought for real, which ends when either is
    /// brought down to this share of their health (neither falls), after so many seconds,
    /// or when the player walks off. Won or lost, the party's stage says so (`Won`,
    /// `Lost`). From a line only; the speaker must be one that fights.
    Spar(f32, f32),
    /// Tells the player which way, and about how far, the nearest party of this kind is,
    /// within so many blocks of the speaker (16 to 256). Said only when one is.
    Toward(String, f32),
}

/// A kind of event with its names resolved.
#[derive(Clone, Debug)]
pub struct Event {
    pub def: EventDef,
    pub groups: Vec<(CreatureId, Group)>,
    pub meetings: Vec<ResolvedMeeting>,
    /// What a freed captive gives its rescuer.
    pub thanks: Vec<ResolvedReward>,
    /// What a led one gives the player who brought it to its journey's end.
    pub guide_thanks: Vec<ResolvedReward>,
    /// What a lure shows the player who followed it to the end.
    pub reveal: Vec<ResolvedReward>,
    /// The blocks a `Gather` party works at.
    pub seek: Vec<BlockId>,
    /// The blocks a carrying `Gather` party takes its work to.
    pub deliver: Vec<BlockId>,
    /// The skill whose level lets a player come nearer a startled party, and by how much a level.
    pub stalk: Option<(SkillId, f32)>,
    /// What watching its show gives.
    pub show: Vec<ResolvedReward>,
    /// What a `Hunt` party hunts, and what it gives the one who drew first blood.
    pub quarry: Vec<CreatureId>,
    pub share: Vec<ResolvedReward>,
    /// The home step a player's home must have reached (`needs.home_is`).
    pub home_is: Option<EffectId>,
}

/// A meeting with its lines resolved.
#[derive(Clone, Debug)]
pub struct ResolvedMeeting {
    pub who: usize,
    pub greet: String,
    pub greets: u8,
    pub lines: Vec<Line>,
}

/// A meeting line with its names resolved.
#[derive(Clone, Debug)]
pub struct Line {
    pub name: String,
    pub needs: Vec<ResolvedNeed>,
    pub wants: Option<(Want, u8)>,
    /// What a party may ask instead, one picked by its seed (see `LineDef::asks`).
    pub asks: Vec<(BlockId, u8)>,
    pub gives: Vec<ResolvedReward>,
    pub once: Once,
}

impl Line {
    /// What the line takes from a party with `seed`: its `wants`, or the ask picked for it.
    pub fn wants_of(&self, seed: u64) -> Option<(Want, u8)> {
        self.wants.clone().or_else(|| self.ask_of(seed).map(|(item, count)| (Want::Item(item), count)))
    }

    /// The ask picked for a party with `seed`, if the line asks.
    pub fn ask_of(&self, seed: u64) -> Option<(BlockId, u8)> {
        (!self.asks.is_empty()).then(|| self.asks[(seed.rotate_left(17) ^ 0x9e37_79b9) as usize % self.asks.len()])
    }
}

/// What a line takes from the player's hand.
#[derive(Clone, Debug, PartialEq)]
pub enum Want {
    Item(BlockId),
    /// Any item with this tag.
    Tag(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedNeed {
    When(Stage),
    Struck(usize),
    Skill(SkillId, u8),
    Below(SkillId, u8),
    Home(bool),
    Hurt,
    Holds(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedReward {
    Item(BlockId, (u8, u8)),
    Xp(SkillId, f32),
    Effect(EffectId, f32),
    Heal(u8),
    Befriend,
    Free,
    Anger,
    Leave,
    Point(Want, f32),
    Town(f32),
    Hand(usize),
    /// A station's tag, the most cooked at once, and how many of them are kept.
    Cook(String, u8, u8),
    OneOf(Vec<ResolvedReward>),
    Weather(String, f32),
    Prayer(f32),
    Grow(f32, u8),
    /// The share of health either may be brought down to, and the bout's seconds.
    Spar(f32, f32),
    /// The full name of a kind of party, and how far to look for one.
    Toward(String, f32),
}

/// A block name or a `tag:` selector, resolved: what a `seek` or a `Point` looks for.
fn selector(name: &str, blocks: &BlockRegistry) -> Result<Want, String> {
    match name.strip_prefix("tag:") {
        Some(tag) if blocks.tagged(tag).next().is_none() => Err(format!("no block has the tag `{tag}`")),
        Some(tag) => Ok(Want::Tag(tag.to_owned())),
        None => blocks.id(name).map(Want::Item).ok_or_else(|| format!("no block `{name}`")),
    }
}

impl Want {
    /// The blocks it names.
    pub fn blocks(&self, blocks: &BlockRegistry) -> Vec<BlockId> {
        match self {
            Want::Item(id) => vec![*id],
            Want::Tag(tag) => blocks.tagged(tag).collect(),
        }
    }
}

/// A reward with its names resolved, or what is wrong with it.
/// A reward of an event of package `namespace`, whose short skill names are that package's
/// own skill first, then core's (`SkillRegistry::named`).
fn resolve_reward(reward: &Reward, namespace: &str, blocks: &BlockRegistry, skills: &SkillRegistry, effects: &EffectRegistry, captive: bool) -> Result<ResolvedReward, String> {
    resolve_reward_in(reward, namespace, blocks, skills, effects, captive, None)
}

/// `resolve_reward`, with the weather a `Weather` reward may bring (none: it may bring none).
fn resolve_reward_in(
    reward: &Reward,
    namespace: &str,
    blocks: &BlockRegistry,
    skills: &SkillRegistry,
    effects: &EffectRegistry,
    captive: bool,
    weather: Option<&WeatherTable>,
) -> Result<ResolvedReward, String> {
    Ok(match reward {
        Reward::Item(name, (low, high)) if low > high || *high == 0 => return Err(format!("gives {low} to {high} of `{name}`")),
        Reward::Item(name, count) => ResolvedReward::Item(blocks.id(name).ok_or_else(|| format!("no item `{name}`"))?, *count),
        Reward::Xp(name, xp) if !(xp.is_finite() && *xp > 0.0 && *xp <= 500.0) => return Err(format!("gives {xp} experience in `{name}`: 0 to 500")),
        Reward::Xp(name, xp) => ResolvedReward::Xp(skills.named(namespace, name).ok_or_else(|| format!("no skill `{name}`"))?, *xp),
        Reward::Effect(name, seconds) => ResolvedReward::Effect(effects.find(name).ok_or_else(|| format!("no effect `{name}`"))?, *seconds),
        Reward::Heal(health) => ResolvedReward::Heal(*health),
        Reward::Befriend => ResolvedReward::Befriend,
        Reward::Free if !captive => return Err("frees a captive, but there is none".into()),
        Reward::Free => ResolvedReward::Free,
        Reward::Anger => ResolvedReward::Anger,
        Reward::Leave => ResolvedReward::Leave,
        Reward::Point(_, reach) if !(8.0..=128.0).contains(reach) => return Err(format!("points within {reach} blocks: 8 to 128")),
        Reward::Point(name, reach) => ResolvedReward::Point(selector(name, blocks)?, *reach),
        Reward::Town(reach) if !(64.0..=1000.0).contains(reach) => return Err(format!("names a settlement within {reach} blocks: 64 to 1000")),
        Reward::Town(reach) => ResolvedReward::Town(*reach),
        Reward::Toward(_, reach) if !(16.0..=256.0).contains(reach) => return Err(format!("tells of a party within {reach} blocks: 16 to 256")),
        // The kind is checked once every event is read, by `WanderRegistry::new`.
        Reward::Toward(event, reach) => ResolvedReward::Toward(event.clone(), *reach),
        // The group is checked against the cast, and the station against the recipes, by the caller.
        Reward::Hand(group) => ResolvedReward::Hand(*group),
        Reward::Cook(_, most, keep) if *most == 0 || keep >= most => return Err(format!("cooks up to {most} and keeps {keep}: it must give some back")),
        Reward::Cook(station, most, keep) => ResolvedReward::Cook(station.clone(), *most, *keep),
        Reward::OneOf(choices) if choices.len() < 2 => return Err("`OneOf` picks one of two or more".into()),
        Reward::OneOf(choices) => ResolvedReward::OneOf(
            choices
                .iter()
                .map(|choice| match choice {
                    Reward::Item(..) | Reward::Xp(..) | Reward::Effect(..) | Reward::Heal(_) | Reward::Point(..) | Reward::Town(_) | Reward::Toward(..) => resolve_reward_in(choice, namespace, blocks, skills, effects, false, weather),
                    _ => Err("`OneOf` picks among items, experience, effects, healing, `Point`s, `Town`s and `Toward`s".into()),
                })
                .collect::<Result<_, _>>()?,
        ),
        Reward::Weather(_, seconds) if !(60.0..=1800.0).contains(seconds) => return Err(format!("brings weather for {seconds} seconds: 60 to 1800")),
        Reward::Weather(name, seconds) => match weather {
            Some(table) if table.preset(name).is_some() => ResolvedReward::Weather(name.clone(), *seconds),
            Some(_) => return Err(format!("no weather `{name}`")),
            None => return Err("brings weather only from a meeting's line".into()),
        },
        Reward::Prayer(points) if !(1.0..=99.0).contains(points) => return Err(format!("restores {points} prayer points: 1 to 99")),
        Reward::Prayer(points) => ResolvedReward::Prayer(*points),
        Reward::Grow(reach, stages) if !(4.0..=48.0).contains(reach) || !(1..=3).contains(stages) => {
            return Err(format!("grows crops within {reach} blocks by {stages} stages: 4 to 48 blocks, 1 to 3 stages"))
        }
        Reward::Grow(reach, stages) => ResolvedReward::Grow(*reach, *stages),
        Reward::Spar(down_to, seconds) if !(0.2..=0.8).contains(down_to) || !(20.0..=180.0).contains(seconds) => {
            return Err(format!("spars down to {down_to} of health for {seconds} seconds: 0.2 to 0.8, 20 to 180 seconds"))
        }
        Reward::Spar(down_to, seconds) => ResolvedReward::Spar(*down_to, *seconds),
    })
}

/// Qualifies the names a reward gives with `namespace`, as `qualify` does.
fn qualify_reward(reward: &mut Reward, qualify: &impl Fn(&mut String)) {
    match reward {
        Reward::Item(item, _) | Reward::Point(item, _) => qualify(item),
        Reward::Effect(effect, _) => qualify(effect),
        Reward::OneOf(choices) => choices.iter_mut().for_each(|c| qualify_reward(c, qualify)),
        Reward::Toward(event, _) => qualify(event),
        _ => {}
    }
}

impl Event {
    /// The lang key of one of its lines: `wander.base.mage_tutors.lesson`.
    pub fn key(&self, line: &str) -> String {
        format!("wander.{}.{line}", self.def.name.replace(':', "."))
    }

    /// The lang key of its name for lists.
    pub fn title_key(&self) -> String {
        self.key("title")
    }

    /// What group `group` says, if it speaks.
    pub fn meeting(&self, group: usize) -> Option<&ResolvedMeeting> {
        self.meetings.iter().find(|m| m.who == group)
    }

    /// The lang key of a meeting's greeting, the `variant`th way (counting from 0).
    pub fn greeting(&self, meeting: &ResolvedMeeting, variant: u8) -> String {
        match variant {
            0 => self.key(&meeting.greet),
            n => self.key(&format!("{}_{}", meeting.greet, n + 1)),
        }
    }

    /// Whether the cast holds a captive.
    pub fn has_captive(&self) -> bool {
        self.def.cast.iter().any(|g| g.role == Role::Captive)
    }

    /// Whether a place `distance` blocks from where the world began suits it.
    pub fn suits_distance(&self, distance: f32) -> bool {
        self.def.needs.away.is_none_or(|(low, high)| (low..=high).contains(&distance))
    }

    /// The weight of a biome with `name` and `tags`, 0 if the event does not come there.
    pub fn biome_weight(&self, name: &str, tags: &[String]) -> f32 {
        if self.def.biomes.is_empty() {
            return 1.0;
        }
        let short = name.rsplit(':').next().unwrap_or(name);
        self.def
            .biomes
            .iter()
            .filter(|(selector, _)| match selector.strip_prefix("tag:") {
                Some(tag) => tags.iter().any(|t| t == tag),
                None => selector == name || selector.rsplit(':').next() == Some(short),
            })
            .map(|(_, weight)| *weight)
            .fold(0.0, f32::max)
    }

    /// Whether the daylight suits it.
    pub fn suits_light(&self, daylight: f32) -> bool {
        (self.def.daylight.0..=self.def.daylight.1).contains(&daylight)
    }
}

/// Every kind of event, and the rules they come by.
#[derive(Debug, Default)]
pub struct WanderRegistry {
    rules: Rules,
    reactions: Vec<Reaction>,
    events: Vec<Event>,
    by_name: FxHashMap<String, usize>,
}

impl WanderRegistry {
    /// Reads a package's `wandering.ron`, qualifying names with its namespace.
    pub fn read(namespace: &str, source: &str) -> Result<WanderPackage, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: WanderFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/wandering.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') && !name.starts_with("tag:") {
                *name = format!("{namespace}:{name}");
            }
        };
        let mut events = file.events;
        for event in &mut events {
            qualify(&mut event.name);
            if let Some((after, _)) = &mut event.needs.after {
                qualify(after);
            }
            if let Some(met) = &mut event.needs.met {
                qualify(met);
            }
            if let Some(step) = &mut event.needs.home_is {
                qualify(step);
            }
            event.cast.iter_mut().for_each(|g| qualify(&mut g.species));
            event.seek.iter_mut().for_each(qualify);
            event.deliver.iter_mut().for_each(qualify);
            event.quarry.iter_mut().for_each(qualify);
            for reward in event
                .freed
                .iter_mut()
                .flat_map(|f| f.thanks.iter_mut())
                .chain(event.guide.iter_mut().flat_map(|g| g.thanks.iter_mut()))
                .chain(event.reveal.iter_mut())
                .chain(event.show.iter_mut().flat_map(|s| s.gives.iter_mut()))
                .chain(event.share.iter_mut())
            {
                qualify_reward(reward, &qualify);
            }
            for meeting in &mut event.meetings {
                for line in &mut meeting.lines {
                    if let Some((item, _)) = &mut line.wants {
                        qualify(item);
                    }
                    line.asks.iter_mut().for_each(|(item, _)| qualify(item));
                    for reward in &mut line.gives {
                        qualify_reward(reward, &qualify);
                    }
                }
            }
        }
        Ok(WanderPackage { rules: file.rules, events, reactions: file.reactions })
    }

    /// Resolves every name against the content, and checks what the events promise.
    pub fn new(
        packages: Vec<WanderPackage>,
        rules: Rules,
        creatures: &CreatureRegistry,
        blocks: &BlockRegistry,
        skills: &SkillRegistry,
        effects: &EffectRegistry,
        cooking: &CookingRegistry,
        weather: &WeatherTable,
    ) -> Result<Self, RegistryError> {
        rules.check()?;
        let mut reactions: Vec<Reaction> = Vec::new();
        for reaction in packages.iter().flat_map(|p| &p.reactions) {
            let error = |why: String| RegistryError(format!("wandering reaction to `{}`: {why}", reaction.held));
            if blocks.tagged(&reaction.held).next().is_none() {
                return Err(error("no item has this tag".into()));
            }
            if !(reaction.range.is_finite() && reaction.range >= 1.0) || reaction.kinds.is_empty() {
                return Err(error(format!("needs a range of at least 1 block and a kind to answer, not {} and {:?}", reaction.range, reaction.kinds)));
            }
            if reaction.combat.is_some_and(|(low, high)| low > high) {
                return Err(error("`combat` is a range, least first".into()));
            }
            if reactions.contains(reaction) {
                return Err(error("another of the same".into()));
            }
            reactions.push(reaction.clone());
        }
        let mut registry = Self { rules, reactions, ..Self::default() };
        for def in packages.into_iter().flat_map(|p| p.events) {
            let error = |why: String| RegistryError(format!("wandering event `{}`: {why}", def.name));
            if registry.by_name.contains_key(&def.name) {
                return Err(error("another event has this name".into()));
            }
            if !(def.weight.is_finite() && def.weight > 0.0) {
                return Err(error("`weight` must be above 0".into()));
            }
            if def.daylight.0 > def.daylight.1 || def.daylight.0 < 0.0 || def.daylight.1 > 1.0 {
                return Err(error("`daylight` is a range within 0 to 1".into()));
            }
            if def.lifetime < 30.0 {
                return Err(error("`lifetime` must be at least 30 seconds".into()));
            }
            let Some(first) = def.cast.first() else { return Err(error("has no cast".into())) };
            if first.role != Role::Lead || first.count.0 == 0 {
                return Err(error("its first group must lead and bring at least one".into()));
            }
            let mut groups = Vec::new();
            for (i, group) in def.cast.iter().enumerate() {
                let species = creatures.id(&group.species).ok_or_else(|| error(format!("no creature `{}`", group.species)))?;
                if group.count.0 > group.count.1 || group.count.1 == 0 {
                    return Err(error(format!("group {i} has a count range of {:?}", group.count)));
                }
                if i > 0 && group.role == Role::Lead {
                    return Err(error(format!("group {i} leads, but only the first group does")));
                }
                if let Role::Pet(owner) = group.role
                    && (owner >= i || def.cast[owner].role == Role::Free)
                {
                    return Err(error(format!("group {i} is a pet of group {owner}, which must come before it and walk")));
                }
                groups.push((species, group.clone()));
            }
            let total: u32 = def.cast.iter().map(|g| g.count.1 as u32).sum();
            if total > 16 {
                return Err(error(format!("brings up to {total} at once; 16 is the most")));
            }
            match def.script {
                Script::March { pass, length, pace } if pass.0 > pass.1 || pass.0 < 0.0 || length < 40.0 || !(0.3..=2.0).contains(&pace) => {
                    return Err(error(format!("March needs 0 <= pass.0 <= pass.1, a length of at least 40 and a pace of 0.3 to 2, not {pass:?}, {length}, {pace}")));
                }
                Script::Patrol { radius, stops, pause, laps } if radius < 4.0 || !(2..=12).contains(&stops) || pause.0 > pause.1 || laps == 0 => {
                    return Err(error(format!("Patrol needs a radius of at least 4, 2 to 12 stops, pause.0 <= pause.1 and a lap: not {radius}, {stops}, {pause:?}, {laps}")));
                }
                Script::Camp { near, stay, mill, away } if near < 8.0 || stay.0 > stay.1 || mill < 1.0 || away < 40.0 => {
                    return Err(error(format!("Camp needs near of at least 8, stay.0 <= stay.1, a mill of at least 1 and away of at least 40: not {near}, {stay:?}, {mill}, {away}")));
                }
                Script::Roam { radius, pause, seconds, .. } if !(0.0..=40.0).contains(&radius) || pause.0 > pause.1 || pause.0 < 0.0 || seconds < 30.0 => {
                    return Err(error(format!("Roam needs a radius of 0 to 40, pause.0 <= pause.1 and 30 seconds or more, not {radius}, {pause:?}, {seconds}")));
                }
                Script::Drift { leg, pause, seconds } if leg.0 < 3.0 || leg.0 > leg.1 || pause.0 > pause.1 || pause.0 < 0.0 || seconds < 30.0 => {
                    return Err(error(format!("Drift needs legs of at least 3 blocks (least <= most), pause.0 <= pause.1 and 30 seconds or more, not {leg:?}, {pause:?}, {seconds}")));
                }
                Script::Shadow { keep, seconds } if keep.0 < 4.0 || keep.0 > keep.1 || seconds < 10.0 => {
                    return Err(error(format!("Shadow needs keep of at least 4 (least <= most) and 10 seconds or more, not {keep:?}, {seconds}")));
                }
                Script::Chase { pass, length, pace, gain, hold }
                    if pass.0 > pass.1 || pass.0 < 0.0 || length < 40.0 || !(0.3..=2.0).contains(&pace) || !(1.02..=2.0).contains(&gain) || hold.0 < 2.0 || hold.0 > hold.1 =>
                {
                    return Err(error(format!(
                        "Chase needs 0 <= pass.0 <= pass.1, a length of at least 40, a pace of 0.3 to 2, a gain of 1.02 to 2 and a hold of at least 2 seconds (least <= most), not {pass:?}, {length}, {pace}, {gain}, {hold:?}"
                    )));
                }
                Script::Ambush { near, trigger, delay, patience, away, hold }
                    if near < 12.0 || trigger < 3.0 || trigger >= near || delay.0 < 0.5 || delay.0 > delay.1 || patience < 30.0 || away < 40.0 || hold.0 < 2.0 || hold.0 > hold.1 =>
                {
                    return Err(error(format!(
                        "Ambush needs near of at least 12, a trigger of at least 3 and under near, a delay of at least half a second (least <= most), patience of 30 seconds, away of 40 and a hold of 2 seconds (least <= most), not {near}, {trigger}, {delay:?}, {patience}, {away}, {hold:?}"
                    )));
                }
                Script::Lure { keep, length, pace, patience, linger } if keep.0 < 3.0 || keep.0 > keep.1 || length < 40.0 || !(0.3..=2.0).contains(&pace) || patience < 20.0 || linger < 2.0 => {
                    return Err(error(format!("Lure needs keep of at least 3 (least <= most), a length of at least 40, a pace of 0.3 to 2, patience of 20 seconds and a linger of 2, not {keep:?}, {length}, {pace}, {patience}, {linger}")));
                }
                Script::Gather { reach, work, visits, .. } if !(4.0..=32.0).contains(&reach) || work.0 < 2.0 || work.0 > work.1 || visits == 0 => {
                    return Err(error(format!("Gather needs a reach of 4 to 32, work of at least 2 seconds (least <= most) and a visit, not {reach}, {work:?}, {visits}")));
                }
                Script::Hunt { reach, strike, takes, seconds } if !(8.0..=64.0).contains(&reach) || !(1.5..=4.0).contains(&strike) || !(1..=4).contains(&takes) || seconds < 60.0 => {
                    return Err(error(format!("Hunt needs a reach of 8 to 64, a strike of 1.5 to 4, 1 to 4 takes and 60 seconds or more, not {reach}, {strike}, {takes}, {seconds}")));
                }
                Script::Journey { reach, pace, near } if !(100.0..=1000.0).contains(&reach) || !(0.3..=2.0).contains(&pace) || !(4.0..=32.0).contains(&near) => {
                    return Err(error(format!("Journey needs a reach of 100 to 1000, a pace of 0.3 to 2 and near of 4 to 32, not {reach}, {pace}, {near}")));
                }
                Script::Gather { carry: Some(Carry { set, tend }), .. }
                    if set.0 < 1.0 || set.0 > set.1 || tend.is_some_and(|(near, stages)| !(1.0..=6.0).contains(&near) || !(1..=2).contains(&stages)) =>
                {
                    return Err(error(format!("Gather's carry needs a set of at least 1 second (least <= most), and tends 1 to 6 blocks by 1 or 2 stages, not {set:?}, {tend:?}")));
                }
                _ => {}
            }
            let has = |role: Role| def.cast.iter().any(|g| g.role == role);
            let (chase, ambush, lure) = (matches!(def.script, Script::Chase { .. }), matches!(def.script, Script::Ambush { .. }), matches!(def.script, Script::Lure { .. }));
            if has(Role::Chaser) != chase || has(Role::Lurk) != ambush {
                return Err(error("a `Chaser` goes with a `Chase` script, a `Lurk` with an `Ambush`, and neither without".into()));
            }
            if (chase || ambush || lure) && (def.halt.is_some() || def.cast.iter().any(|g| g.role == Role::Captive) || def.guide.is_some()) {
                return Err(error("a chase, an ambush or a lure has its own way of meeting a player: no `halt`, captive or `guide`".into()));
            }
            let journey = matches!(def.script, Script::Journey { .. });
            if lure && def.reveal.is_empty() || !lure && !journey && !def.reveal.is_empty() {
                return Err(error("a `Lure` shows `reveal` at its end, a `Journey` may, and nothing else has one".into()));
            }
            if journey && (def.guard.is_some() || def.startle.is_some() || def.cast.iter().any(|g| g.role == Role::Captive) || def.guide.is_some()) {
                return Err(error("a journey walks to town: no `guard`, `startle`, captive or `guide`".into()));
            }
            let gather = matches!(def.script, Script::Gather { .. });
            if gather == def.seek.is_empty() {
                return Err(error("a `Gather` works at what it `seek`s, and nothing else seeks".into()));
            }
            let mut seek = Vec::new();
            for name in &def.seek {
                seek.extend(selector(name, blocks).map_err(|why| error(format!("seek: {why}")))?.blocks(blocks));
            }
            let carries = matches!(def.script, Script::Gather { carry: Some(_), .. });
            if !carries && !def.deliver.is_empty() {
                return Err(error("only a `Gather` that carries `deliver`s".into()));
            }
            let mut deliver = Vec::new();
            for name in &def.deliver {
                deliver.extend(selector(name, blocks).map_err(|why| error(format!("deliver: {why}")))?.blocks(blocks));
            }
            if (chase || ambush || lure) && (def.guard.is_some() || def.startle.is_some()) {
                return Err(error("a chase, an ambush or a lure does not `guard` or `startle`".into()));
            }
            let hunt = matches!(def.script, Script::Hunt { .. });
            if hunt == def.quarry.is_empty() || !hunt && !def.share.is_empty() {
                return Err(error("a `Hunt` goes after its `quarry` and gives its `share`, and nothing else has either".into()));
            }
            if hunt && (def.halt.is_some() || def.guard.is_some() || def.startle.is_some() || captives_in(&def) > 0 || def.guide.is_some()) {
                return Err(error("a hunt minds its quarry: no `halt`, `guard`, `startle`, captive or `guide`".into()));
            }
            let mut quarry = Vec::new();
            for name in &def.quarry {
                let species = creatures.id(name).ok_or_else(|| error(format!("quarry: no creature `{name}`")))?;
                if creatures.get(species).def.kind != Kind::Animal {
                    return Err(error(format!("quarry: `{name}` is no animal")));
                }
                quarry.push(species);
            }
            if def.guard.is_some() && def.startle.is_some() || def.startle.is_some() && def.halt.is_some() {
                return Err(error("a party that bolts (`startle`) neither stands its ground (`guard`) nor stops for a player (`halt`)".into()));
            }
            if let Some(guard) = def.guard {
                if guard.near < 3.0 || guard.warn < 0.5 || guard.calm <= guard.near {
                    return Err(error(format!("`guard` needs near of at least 3, a warning of half a second and calm further than near, not {guard:?}")));
                }
                if !groups.iter().any(|(species, group)| creatures.get(*species).fights_as(group.young)) {
                    return Err(error("guards, but nobody in the cast fights".into()));
                }
            }
            let mut stalk = None;
            if let Some(startle) = &def.startle {
                if startle.least < 2.0 || startle.least > startle.near || startle.still < 1.0 || startle.still > startle.near || startle.still_for < 0.5 {
                    return Err(error(format!("`startle` needs 2 <= least <= near, still of 1 up to near and still_for of half a second, not {startle:?}")));
                }
                if let Some((name, per)) = &startle.stalk {
                    if !(per.is_finite() && *per > 0.0 && *per <= 2.0) {
                        return Err(error(format!("`stalk` takes 0 to 2 blocks a level, not {per}")));
                    }
                    stalk = Some((skills.named(def.name.split_once(':').map_or("core", |(ns, _)| ns), name).ok_or_else(|| error(format!("stalk: no skill `{name}`")))?, *per));
                }
            }
            let captives = def.cast.iter().filter(|g| g.role == Role::Captive).count();
            if captives > 1 {
                return Err(error("only one group may be captive".into()));
            }
            if captives == 1 && def.cast.iter().all(|g| matches!(g.role, Role::Captive | Role::Free)) {
                return Err(error("a captive needs captors that walk".into()));
            }
            match (captives, &def.freed) {
                (1, Some(f)) if !(0.0..=1.0).contains(&f.follow) || f.follow_for < 10.0 || f.follow_for > def.lifetime => {
                    return Err(error(format!("`freed` needs a `follow` chance of 0 to 1 and a `follow_for` of 10 seconds up to the lifetime, not {}, {}", f.follow, f.follow_for)));
                }
                (1, None) => return Err(error("has a captive, so it needs `freed`".into())),
                (0, Some(_)) => return Err(error("has `freed` but no captive".into())),
                _ => {}
            }
            if let Some(g) = &def.guide
                && (g.follow_for < 30.0 || g.follow_for > def.lifetime)
            {
                return Err(error(format!("`guide.follow_for` ({}) must be 30 seconds up to the lifetime ({})", g.follow_for, def.lifetime)));
            }
            let befriends = def.meetings.iter().flat_map(|m| &m.lines).any(|l| l.gives.iter().any(|r| matches!(r, Reward::Befriend)));
            if befriends != def.guide.is_some() {
                return Err(error("`guide` and the `Befriend` reward go together: one without the other does nothing".into()));
            }
            if let Some(halt) = def.halt
                && (halt.near < 2.0 || halt.seconds.0 < 1.0 || halt.seconds.0 > halt.seconds.1 || halt.cooldown < 0.0)
            {
                return Err(error(format!("`halt` needs near of at least 2, seconds of at least 1 (least <= most) and a cooldown, not {halt:?}")));
            }
            if let Some((low, high)) = def.needs.away
                && (low < 0.0 || low > high)
            {
                return Err(error("`away` is a range from 0 up".into()));
            }
            if let Some(town) = def.needs.town
                && !(16.0..=1000.0).contains(&town)
            {
                return Err(error(format!("`town` is 16 to 1000 blocks, not {town}")));
            }
            if let Some(near) = def.needs.near_home
                && !(16.0..=400.0).contains(&near)
            {
                return Err(error(format!("`near_home` is 16 to 400 blocks, not {near}")));
            }
            if def.needs.near_home.is_some() && def.needs.home == Some(false) {
                return Err(error("`near_home` comes to a player with a home, and `home: Some(false)` to one without".into()));
            }
            let mut meetings = Vec::new();
            let mut names: Vec<String> = vec!["title".into()];
            if captives == 1 {
                names.extend(["thanks".into(), "freed".into()]);
            }
            let namespace = def.name.split_once(':').map_or("core", |(ns, _)| ns);
            if def.guide.is_some() {
                names.extend(["arrived".into(), "company".into()]);
            }
            if lure || journey {
                names.push("arrived".into());
            }
            if def.guard.is_some() {
                names.push("warn".into());
            }
            let mut reveal = Vec::new();
            for reward in &def.reveal {
                if matches!(reward, Reward::Free | Reward::Anger | Reward::Befriend | Reward::Leave | Reward::Hand(_) | Reward::Cook(..) | Reward::Spar(..)) {
                    return Err(error("a lure's `reveal` gives items, experience, effects or healing only".into()));
                }
                reveal.push(resolve_reward(reward, namespace, blocks, skills, effects, false).map_err(|why| error(format!("reveal: {why}")))?);
            }
            let mut show = Vec::new();
            if let Some(def_show) = &def.show {
                if !matches!(def.script, Script::Camp { .. }) {
                    return Err(error("only a `Camp` party puts on a `show`".into()));
                }
                if !(2.0..=16.0).contains(&def_show.near) || !(3.0..=120.0).contains(&def_show.seconds) || def_show.gives.is_empty() {
                    return Err(error(format!("a show is watched from 2 to 16 blocks for 3 to 120 seconds and gives something, not {} blocks for {} s", def_show.near, def_show.seconds)));
                }
                for reward in &def_show.gives {
                    if matches!(reward, Reward::Free | Reward::Anger | Reward::Befriend | Reward::Leave | Reward::Hand(_) | Reward::Cook(..) | Reward::Spar(..) | Reward::Point(..) | Reward::Town(_) | Reward::Toward(..) | Reward::OneOf(_)) {
                        return Err(error("a show gives items, experience, effects or healing only".into()));
                    }
                    show.push(resolve_reward(reward, namespace, blocks, skills, effects, false).map_err(|why| error(format!("show: {why}")))?);
                }
                names.push("show".into());
            }
            let mut share = Vec::new();
            for reward in &def.share {
                if matches!(reward, Reward::Free | Reward::Anger | Reward::Befriend | Reward::Leave | Reward::Hand(_) | Reward::Cook(..) | Reward::Spar(..)) {
                    return Err(error("a hunt's `share` gives items, experience, effects or healing only".into()));
                }
                share.push(resolve_reward(reward, namespace, blocks, skills, effects, false).map_err(|why| error(format!("share: {why}")))?);
            }
            if !share.is_empty() {
                names.push("share".into());
            }
            let mut guide_thanks = Vec::new();
            for reward in def.guide.iter().flat_map(|g| &g.thanks) {
                if matches!(reward, Reward::Free | Reward::Anger | Reward::Befriend | Reward::Leave | Reward::Hand(_) | Reward::Cook(..) | Reward::Spar(..)) {
                    return Err(error("a guide's `thanks` gives items, experience, effects or healing only".into()));
                }
                guide_thanks.push(resolve_reward(reward, namespace, blocks, skills, effects, false).map_err(|why| error(format!("guide thanks: {why}")))?);
            }
            let mut thanks = Vec::new();
            for reward in def.freed.iter().flat_map(|f| &f.thanks) {
                if matches!(reward, Reward::Free | Reward::Anger | Reward::Befriend | Reward::Leave | Reward::Hand(_) | Reward::Cook(..) | Reward::Spar(..)) {
                    return Err(error("a captive's `thanks` gives items, experience, effects or healing only".into()));
                }
                thanks.push(resolve_reward(reward, namespace, blocks, skills, effects, true).map_err(|why| error(format!("thanks: {why}")))?);
            }
            // A bout's two ends are stages of the party too, told as they come.
            let spars = def.meetings.iter().flat_map(|m| &m.lines).any(|l| l.gives.iter().any(|r| matches!(r, Reward::Spar(..))));
            if spars {
                names.extend(["bout_won".into(), "bout_lost".into()]);
            }
            for meeting in &def.meetings {
                if meeting.who >= def.cast.len() || matches!(def.cast[meeting.who].role, Role::Free) && meeting.who != 0 {
                    return Err(error(format!("the speaker is group {}, which is not in the cast or does not walk with it", meeting.who)));
                }
                if meetings.iter().any(|m: &ResolvedMeeting| m.who == meeting.who) {
                    return Err(error(format!("group {} has two meetings", meeting.who)));
                }
                if meeting.lines.is_empty() {
                    return Err(error("a meeting needs at least one line".into()));
                }
                if meeting.greets == 0 || meeting.greets > 9 || meeting.greet == "title" {
                    return Err(error(format!("`greets` is 1 to 9 and `greet` is not `title`, not {} of `{}`", meeting.greets, meeting.greet)));
                }
                for n in 0..meeting.greets {
                    let name = if n == 0 { meeting.greet.clone() } else { format!("{}_{}", meeting.greet, n + 1) };
                    if names.contains(&name) {
                        return Err(error(format!("`{name}` is used twice: greetings and lines need names of their own")));
                    }
                    names.push(name);
                }
                let mut lines = Vec::new();
                for line in &meeting.lines {
                    let at = |why: String| error(format!("line `{}`: {why}", line.name));
                    if names.contains(&line.name) {
                        return Err(at("another line or a greeting has this name (`title` is reserved)".into()));
                    }
                    let item = |name: &str| blocks.id(name).ok_or_else(|| at(format!("no item `{name}`")));
                    let skill = |name: &str| skills.named(namespace, name).ok_or_else(|| at(format!("no skill `{name}`")));
                    let mut needs = Vec::new();
                    for need in &line.needs {
                        needs.push(match need {
                            Need::When(stage) if !stage.belongs_to(&def.script) && !(spars && matches!(stage, Stage::Won | Stage::Lost)) => {
                                return Err(at(format!("`{stage:?}` is no stage of this event's script")));
                            }
                            Need::When(stage) => ResolvedNeed::When(*stage),
                            Need::Struck(group) if *group >= def.cast.len() => return Err(at(format!("`Struck({group})`: the cast has {} groups", def.cast.len()))),
                            Need::Struck(group) => ResolvedNeed::Struck(*group),
                            Need::Skill(name, level) => ResolvedNeed::Skill(skill(name)?, *level),
                            Need::Below(name, level) if *level == 0 => return Err(at(format!("below level 0 of `{name}` is no one"))),
                            Need::Below(name, level) => ResolvedNeed::Below(skill(name)?, *level),
                            Need::Home(has) => ResolvedNeed::Home(*has),
                            Need::Hurt => ResolvedNeed::Hurt,
                            Need::Holds(tag) => ResolvedNeed::Holds(tag.clone()),
                        });
                    }
                    let wants = match &line.wants {
                        Some((name, count)) if *count == 0 => return Err(at(format!("wants 0 of `{name}`"))),
                        Some((name, count)) => Some((
                            match name.strip_prefix("tag:") {
                                Some(tag) => Want::Tag(tag.to_owned()),
                                None => Want::Item(item(name)?),
                            },
                            *count,
                        )),
                        None => None,
                    };
                    if !line.asks.is_empty() && line.wants.is_some() {
                        return Err(at("a line `wants` one thing or `asks` for one of several, not both".into()));
                    }
                    let mut asks = Vec::new();
                    for (name, count) in &line.asks {
                        if !(1..=64).contains(count) {
                            return Err(at(format!("asks {count} of `{name}`: 1 to 64")));
                        }
                        asks.push((item(name)?, *count));
                    }
                    let mut gives = Vec::new();
                    for reward in &line.gives {
                        if let Reward::Hand(group) = reward
                            && (*group >= groups.len() || *group == meeting.who || creatures.get(groups[*group].0).def.taming.is_none())
                        {
                            return Err(at(format!("`Hand({group})` hands over a group of the cast that can be kept, not the speaker's own")));
                        }
                        if matches!(reward, Reward::Spar(..)) && !creatures.get(groups[meeting.who].0).fights() {
                            return Err(at("`Spar`: the speaker does not fight".into()));
                        }
                        if let Reward::Cook(station, ..) = reward {
                            if line.wants.is_none() && line.asks.is_empty() {
                                return Err(at("`Cook` cooks what the line `wants`, and it wants nothing".into()));
                            }
                            if !cooking.iter().any(|(_, r)| r.def.station == *station) {
                                return Err(at(format!("`Cook` at `{station}`: no recipe is cooked at a station of that tag")));
                            }
                        }
                        gives.push(resolve_reward_in(reward, namespace, blocks, skills, effects, captives == 1, Some(weather)).map_err(&at)?);
                    }
                    names.push(line.name.clone());
                    if !asks.is_empty() {
                        names.push(format!("{}_ask", line.name));
                    }
                    lines.push(Line { name: line.name.clone(), needs, wants, asks, gives, once: line.once });
                }
                meetings.push(ResolvedMeeting { who: meeting.who, greet: meeting.greet.clone(), greets: meeting.greets, lines });
            }
            registry.by_name.insert(def.name.clone(), registry.events.len());
            let home_is = match &def.needs.home_is {
                None => None,
                Some(step) => match effects.find(step) {
                    Some(id) if effects.get(id).when.is_some() => Some(id),
                    Some(_) => return Err(error(format!("home_is: `{step}` holds anywhere, not a home step"))),
                    None => return Err(error(format!("home_is: no effect `{step}`"))),
                },
            };
            registry.events.push(Event { def, groups, meetings, thanks, guide_thanks, reveal, seek, deliver, stalk, show, quarry, share, home_is });
        }
        // Parties that come after, or tell of, another kind: that kind must be one read.
        for event in &registry.events {
            let error = |why: String| RegistryError(format!("wandering event `{}`: {why}", event.def.name));
            if let Some((after, reach)) = &event.def.needs.after {
                if !registry.by_name.contains_key(after) || *after == event.def.name {
                    return Err(error(format!("comes after `{after}`, which is no other event")));
                }
                if !(16.0..=256.0).contains(reach) {
                    return Err(error(format!("comes after a party within {reach} blocks: 16 to 256")));
                }
            }
            if let Some(met) = &event.def.needs.met
                && (!registry.by_name.contains_key(met) || *met == event.def.name)
            {
                return Err(error(format!("comes to those who met `{met}`, which is no other event")));
            }
            fn told(reward: &ResolvedReward, out: &mut Vec<String>) {
                match reward {
                    ResolvedReward::Toward(name, _) => out.push(name.clone()),
                    ResolvedReward::OneOf(choices) => choices.iter().for_each(|c| told(c, out)),
                    _ => {}
                }
            }
            let mut named = Vec::new();
            for reward in event.meetings.iter().flat_map(|m| m.lines.iter()).flat_map(|l| l.gives.iter()).chain(&event.thanks).chain(&event.guide_thanks).chain(&event.reveal).chain(&event.share) {
                told(reward, &mut named);
            }
            if let Some(name) = named.iter().find(|name| !registry.by_name.contains_key(*name)) {
                return Err(error(format!("tells of `{name}`, which is no event")));
            }
        }
        Ok(registry)
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// How creatures answer what a player holds.
    pub fn reactions(&self) -> &[Reaction] {
        &self.reactions
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn get(&self, index: usize) -> &Event {
        &self.events[index]
    }

    /// An event's index by full name, or by its name alone when only one package has it.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied().or_else(|| {
            let mut found = self.events.iter().enumerate().filter(|(_, e)| e.def.name.rsplit(':').next() == Some(name));
            let (i, _) = found.next()?;
            found.next().is_none().then_some(i)
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &Event)> {
        self.events.iter().enumerate()
    }
}

/// How many groups of a cast are captive.
fn captives_in(def: &EventDef) -> usize {
    def.cast.iter().filter(|g| g.role == Role::Captive).count()
}

/// Where member `i` of `n` stands behind (positive `depth`) or beside a leader walking
/// along `heading`, for a role: a loose file, two flanks, a long tail. Pure geometry, from
/// the member's seed so that two parties do not walk the same pattern.
pub fn slot(role: Role, i: usize, n: usize, seed: u64) -> glam::Vec2 {
    let jitter = |salt: u64| crate::creature::unit_from(crate::creature::mix(seed, salt)) - 0.5;
    let rank = i as f32 + 1.0;
    // Offsets are (right, behind) the leader.
    match role {
        Role::Follow => glam::Vec2::new(jitter(1) * 2.2, 1.6 + rank * 1.5),
        Role::Flank => {
            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
            glam::Vec2::new(side * (2.4 + (i / 2) as f32 * 1.4 + jitter(2) * 0.6), -1.0 - jitter(3) * 2.0)
        }
        Role::Rear => glam::Vec2::new(jitter(4) * 3.0, 9.0 + rank * 2.0 + n as f32 * 0.5),
        Role::Ahead => glam::Vec2::new(jitter(5) * 5.0, -14.0 - rank * 2.0),
        Role::Pet(_) => glam::Vec2::new(1.6 + jitter(6) * 1.4, 0.4 + jitter(7) * 1.6),
        Role::Captive => glam::Vec2::new(jitter(8) * 0.8, 2.4),
        // Chasers start well behind the quarry, a little apart; lurkers lie to the sides of
        // the road, a little beyond the bait.
        Role::Chaser => glam::Vec2::new(jitter(11) * 3.0, 8.0 + rank * 1.5),
        Role::Lurk => glam::Vec2::new(if i % 2 == 0 { 1.0 } else { -1.0 } * (5.0 + (jitter(12) + 0.5) * 4.0), -0.5 - (jitter(13) + 0.5) * 3.0),
        Role::Herd => glam::Vec2::new(jitter(9) * 9.0, 1.0 + (jitter(10) + 0.5) * 6.0),
        Role::Lead | Role::Free => glam::Vec2::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> WanderPackage {
        WanderRegistry::read("base", source).expect("parses")
    }

    #[test]
    fn names_are_qualified_and_rules_check() {
        let package = parse(
            "(events: [(name: \"pilgrims\", cast: [(species: \"priest\"), (species: \"villager\", count: (1, 3), role: Follow)], \
             script: March(pass: (10.0, 20.0), length: 120.0, pace: 1.0))])",
        );
        assert_eq!(package.events[0].name, "base:pilgrims");
        assert_eq!(package.events[0].cast[1].species, "base:villager");
        assert_eq!(package.events[0].cast[0].role, Role::Lead);
        assert!(Rules::default().check().is_ok());
        assert!(Rules { leave: 60.0, ..Rules::default() }.check().is_err(), "events would end as they begin");
        assert!(Rules { chance: 1.5, ..Rules::default() }.check().is_err());
    }

    #[test]
    fn a_misspelt_field_is_an_error() {
        assert!(WanderRegistry::read("base", "(events: [(name: \"x\", cast: [], script: March(pass: (1.0, 2.0), length: 50.0, pace: 1.0), colour: 3)])").is_err());
    }

    #[test]
    fn slots_put_each_role_where_it_belongs() {
        let behind = slot(Role::Follow, 0, 1, 7);
        assert!(behind.y > 1.0, "followers walk behind: {behind}");
        let ahead = slot(Role::Ahead, 0, 1, 7);
        assert!(ahead.y < -10.0, "scouts walk ahead: {ahead}");
        let (a, b) = (slot(Role::Flank, 0, 2, 7), slot(Role::Flank, 1, 2, 7));
        assert!(a.x > 0.0 && b.x < 0.0, "flanks take opposite sides: {a} {b}");
        assert_eq!(slot(Role::Follow, 2, 3, 9), slot(Role::Follow, 2, 3, 9), "the same seed puts them in the same place");
        assert_eq!(slot(Role::Lead, 0, 1, 9), glam::Vec2::ZERO);
    }
}
