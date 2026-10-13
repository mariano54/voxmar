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
use crate::creature::{CreatureId, CreatureRegistry};
use crate::effect::{EffectId, EffectRegistry};
use crate::skill::{SkillId, SkillRegistry};

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
        }
    }
}

crate::content::settable! {
    /// The rules one package sets: any of them.
    SetRules for Rules {
        scan_every: f32, chance: f32, gap: f32, most_near: usize, near: f32, most_total: usize, people_most: usize,
        companions_most: usize,
        appear_near: f32, appear_far: f32, leave: f32, errand_every: f32, first_days: f32, speak_reach: f32,
        repeat_weight: f32, repeat_memory: f32,
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
}

/// What a package gives: its rules and its events, names qualified.
#[derive(Clone, Debug)]
pub struct WanderPackage {
    pub rules: Option<SetRules>,
    pub events: Vec<EventDef>,
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
    /// The daylight (0 at night, 1 by day) it comes in, least and most.
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
    /// Seconds it lasts at most; it ends sooner when it has gone by.
    #[serde(default = "lifetime")]
    pub lifetime: f32,
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
    /// Held by the rest: walks close behind the leader until every other member is gone or
    /// the bonds are cut (the `Free` reward), then is a freed captive (`freed`).
    Captive,
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

/// What a player must be, for a line.
#[derive(Clone, Debug, Deserialize)]
pub enum Need {
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
    /// Cuts the bonds of the party's captive, and does nothing for the captors' mood.
    Free,
    /// The party turns on the player, as if one of them had been struck.
    Anger,
}

/// A kind of event with its names resolved.
#[derive(Clone, Debug)]
pub struct Event {
    pub def: EventDef,
    pub groups: Vec<(CreatureId, Group)>,
    pub meetings: Vec<ResolvedMeeting>,
    /// What a freed captive gives its rescuer.
    pub thanks: Vec<ResolvedReward>,
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
    pub wants: Option<(BlockId, u8)>,
    pub gives: Vec<ResolvedReward>,
    pub once: Once,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedNeed {
    Skill(SkillId, u8),
    Below(SkillId, u8),
    Home(bool),
    Hurt,
    Holds(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedReward {
    Item(BlockId, (u8, u8)),
    Xp(SkillId, f32),
    Effect(EffectId, f32),
    Heal(u8),
    Free,
    Anger,
}

/// A reward with its names resolved, or what is wrong with it.
fn resolve_reward(reward: &Reward, blocks: &BlockRegistry, skills: &SkillRegistry, effects: &EffectRegistry, captive: bool) -> Result<ResolvedReward, String> {
    Ok(match reward {
        Reward::Item(name, (low, high)) if low > high || *high == 0 => return Err(format!("gives {low} to {high} of `{name}`")),
        Reward::Item(name, count) => ResolvedReward::Item(blocks.id(name).ok_or_else(|| format!("no item `{name}`"))?, *count),
        Reward::Xp(name, xp) if !(xp.is_finite() && *xp > 0.0 && *xp <= 500.0) => return Err(format!("gives {xp} experience in `{name}`: 0 to 500")),
        Reward::Xp(name, xp) => ResolvedReward::Xp(skills.id(name).ok_or_else(|| format!("no skill `{name}`"))?, *xp),
        Reward::Effect(name, seconds) => ResolvedReward::Effect(effects.find(name).ok_or_else(|| format!("no effect `{name}`"))?, *seconds),
        Reward::Heal(health) => ResolvedReward::Heal(*health),
        Reward::Free if !captive => return Err("frees a captive, but there is none".into()),
        Reward::Free => ResolvedReward::Free,
        Reward::Anger => ResolvedReward::Anger,
    })
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
            event.cast.iter_mut().for_each(|g| qualify(&mut g.species));
            if let Some(freed) = &mut event.freed {
                for reward in &mut freed.thanks {
                    match reward {
                        Reward::Item(item, _) => qualify(item),
                        Reward::Xp(skill, _) if !skill.contains(':') => *skill = format!("core:{skill}"),
                        Reward::Effect(effect, _) => qualify(effect),
                        _ => {}
                    }
                }
            }
            for meeting in &mut event.meetings {
                for line in &mut meeting.lines {
                    if let Some((item, _)) = &mut line.wants {
                        qualify(item);
                    }
                    for reward in &mut line.gives {
                        match reward {
                            Reward::Item(item, _) => qualify(item),
                            Reward::Xp(skill, _) if !skill.contains(':') => *skill = format!("core:{skill}"),
                            Reward::Effect(effect, _) => qualify(effect),
                            _ => {}
                        }
                    }
                    for need in &mut line.needs {
                        if let Need::Skill(skill, _) | Need::Below(skill, _) = need
                            && !skill.contains(':')
                        {
                            *skill = format!("core:{skill}");
                        }
                    }
                }
            }
        }
        Ok(WanderPackage { rules: file.rules, events })
    }

    /// Resolves every name against the content, and checks what the events promise.
    pub fn new(
        packages: Vec<WanderPackage>,
        rules: Rules,
        creatures: &CreatureRegistry,
        blocks: &BlockRegistry,
        skills: &SkillRegistry,
        effects: &EffectRegistry,
    ) -> Result<Self, RegistryError> {
        rules.check()?;
        let mut registry = Self { rules, ..Self::default() };
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
                Script::Shadow { keep, seconds } if keep.0 < 4.0 || keep.0 > keep.1 || seconds < 10.0 => {
                    return Err(error(format!("Shadow needs keep of at least 4 (least <= most) and 10 seconds or more, not {keep:?}, {seconds}")));
                }
                _ => {}
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
            let mut meetings = Vec::new();
            let mut names: Vec<String> = vec!["title".into()];
            if captives == 1 {
                names.extend(["thanks".into(), "freed".into()]);
            }
            let mut thanks = Vec::new();
            for reward in def.freed.iter().flat_map(|f| &f.thanks) {
                if matches!(reward, Reward::Free | Reward::Anger) {
                    return Err(error("a captive's `thanks` gives items, experience, effects or healing only".into()));
                }
                thanks.push(resolve_reward(reward, blocks, skills, effects, true).map_err(|why| error(format!("thanks: {why}")))?);
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
                    let skill = |name: &str| skills.id(name).ok_or_else(|| at(format!("no skill `{name}`")));
                    let mut needs = Vec::new();
                    for need in &line.needs {
                        needs.push(match need {
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
                        Some((name, count)) => Some((item(name)?, *count)),
                        None => None,
                    };
                    let mut gives = Vec::new();
                    for reward in &line.gives {
                        gives.push(resolve_reward(reward, blocks, skills, effects, captives == 1).map_err(&at)?);
                    }
                    names.push(line.name.clone());
                    lines.push(Line { name: line.name.clone(), needs, wants, gives, once: line.once });
                }
                meetings.push(ResolvedMeeting { who: meeting.who, greet: meeting.greet.clone(), greets: meeting.greets, lines });
            }
            registry.by_name.insert(def.name.clone(), registry.events.len());
            registry.events.push(Event { def, groups, meetings, thanks });
        }
        Ok(registry)
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
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
