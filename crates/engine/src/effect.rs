//! Status effects: buffs and debuffs that change how a player heals, feels the cold,
//! moves, mines and learns.
//!
//! Effects are content: each package's `effects.ron` lists its own, so a mod adds a buff
//! the way it adds a block. What an effect may do is a small, fixed vocabulary of
//! [`Modifiers`] owned by the engine, and what brings one on is either a [`Condition`] the
//! server checks every second (warmth, the weather where the player stands) or
//! something the player did: food gives the effects its package's `foods` rules name for
//! the tags it has (cooked meat makes you hearty), and any with an `effect:NAME:SECONDS`
//! tag of its own, for that long. An effect may `replace` milder ones (freezing hides chilly), and while
//! several are active their modifiers combine: rates multiply, warmth and health add, the
//! quickest hurt wins.
//!
//! Home is a condition too. A player's home (the lit Homestead Hearth, else the bed they last
//! slept in) is surveyed for the tags of the blocks around it, and an effect may need the
//! player to be there (`AtHome`), under a roof (`Indoors`) and to have so many blocks with a
//! tag around it (`HomeHas("bed", 1)`), and walls about the room it stands in of so good a
//! shelter (`HomeWalls(Sound, 0.6)`: the walls' [`Shelter`] tags). A ladder of such effects, each replacing the one
//! below, is a home that gets cosier as it is furnished (base/effects.ron). Sleeping at a
//! bed gives the effects the file's `sleeps` rules name, those whose `needs` effect would
//! hold where the player sleeps: a better rest for a better home, and (`night`) the best
//! of them only after dark.
//!
//! Every effect has an icon (`ui/effect/<icon>` in the packs) and its name and a line on
//! what it does as language keys (`effect.<package>.<name>` and `….description`).

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, RegistryError};
use crate::weather::Weather;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EffectId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Kind {
    Buff,
    Debuff,
}

/// An effect as content defines it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDef {
    pub name: String,
    pub kind: Kind,
    /// The icon, `ui/effect/<icon>` in the packs; the effect's own name when not given.
    #[serde(default)]
    pub icon: String,
    /// What it is called and what it does, in English: the fallback for its language keys.
    pub title: String,
    pub description: String,
    /// What brings it on, checked every second; none for an effect only food or other
    /// actions give.
    #[serde(default)]
    pub when: Option<Condition>,
    /// Milder effects it hides while it lasts.
    #[serde(default)]
    pub replaces: Vec<String>,
    /// Seconds it stays once its `when` stops holding: an effect that clings (smoke on
    /// clothes). While `when` holds it is kept topped up to this, as a timed effect; 0 (the
    /// default) is an effect that lasts only while `when` holds.
    #[serde(default)]
    pub lingers: f32,
    /// What ends a lingering effect at once, once its `when` has stopped: wet clothes dry
    /// by a fire (`Warmed`). None: it lasts its `lingers` seconds out.
    #[serde(default)]
    pub ends: Option<Condition>,
    /// Whether the player is told (in chat) when it comes on, with what it does: for the
    /// steps of a ladder they work toward.
    #[serde(default)]
    pub announce: bool,
    /// The ladder an announced step belongs to; reaching it counts for quests as
    /// `Built("LADDER_NAME", 1)`, with the effect's name alone after the ladder's. `home`
    /// unless given.
    #[serde(default = "home_ladder")]
    pub ladder: String,
    /// For an announced step, what to do to reach the next, which the chat line says after
    /// what this one does (language key `….next`).
    #[serde(default)]
    pub next: String,
    #[serde(default)]
    pub modifiers: Modifiers,
    /// Levels it adds to skills' checks (a boost potion: `[("engineering", 3)]`); all
    /// boosts together stop at +6 (`level::MAX_BOOST`).
    #[serde(default)]
    pub boosts: Vec<(String, i8)>,
}

fn home_ladder() -> String {
    "home".to_owned()
}

/// What an effect changes. Rates are multipliers (1 changes nothing).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Modifiers {
    /// How fast health comes back by itself; 0 stops it.
    pub heal: f32,
    /// Seconds between losing a point of health, if it hurts.
    pub hurt_every: Option<f32>,
    /// Degrees (°C) added to how warm the player feels.
    pub warmth: f32,
    /// Degrees a home adds (its steps and rooms), 0 or more: they lift how warm the player
    /// feels only up to the top of the comfortable band (survival.ron's `comfort_top`),
    /// never past it, so a cosy home never makes its owner hot (`Modifiers::felt`).
    pub home_warmth: f32,
    /// Extra health points the player may have.
    pub max_health: i8,
    /// How much more (or less) health the player may have, as a share of what they would:
    /// 1.1 is a tenth more. The extra comes empty, to fill by healing.
    pub max_health_share: f32,
    /// How fast the player walks and runs.
    pub speed: f32,
    /// How fast the player mines.
    pub mining: f32,
    /// How much experience every skill earns.
    pub xp: f32,
    /// How often a cast line lands something.
    pub fishing: f32,
    /// How much a meal heals, and how long the effects it gives last.
    pub meal_heal: f32,
    pub meal_time: f32,
    /// What is added to the chance of an extra find when the player breaks a natural block
    /// (`tools.ron`'s rules): 0.05 is one more time in twenty. It adds to what the tool's
    /// `yield` and the skill level give.
    pub finds: f32,
    /// How many times its own slots a store in the player's home holds, opened by them
    /// (`Machine::grown_bin`): 2 is twice. Stores only grow, never shrink.
    pub store_room: f32,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self { heal: 1.0, hurt_every: None, warmth: 0.0, home_warmth: 0.0, max_health: 0, max_health_share: 1.0, speed: 1.0, mining: 1.0, xp: 1.0, fishing: 1.0, meal_heal: 1.0, meal_time: 1.0, finds: 0.0, store_room: 1.0 }
    }
}

impl Modifiers {
    /// How warm the player feels in air (and fire's heat) of `air` °C: `warmth` adds as it
    /// is, then `home_warmth` lifts what that comes to toward `comfort_top` and no further.
    /// What is already at or above the top is left as it is: a home warms a cold night and
    /// adds nothing to a hot day.
    pub fn felt(&self, air: f32, comfort_top: f32) -> f32 {
        let base = air + self.warmth;
        if base >= comfort_top { base } else { (base + self.home_warmth).min(comfort_top) }
    }

    /// Several effects' modifiers together.
    pub fn combine<'a>(all: impl IntoIterator<Item = &'a Modifiers>) -> Modifiers {
        all.into_iter().fold(Modifiers::default(), |sum, m| Modifiers {
            heal: sum.heal * m.heal,
            hurt_every: match (sum.hurt_every, m.hurt_every) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
            warmth: sum.warmth + m.warmth,
            home_warmth: sum.home_warmth + m.home_warmth,
            max_health: sum.max_health.saturating_add(m.max_health),
            max_health_share: sum.max_health_share * m.max_health_share,
            speed: sum.speed * m.speed,
            mining: sum.mining * m.mining,
            xp: sum.xp * m.xp,
            fishing: sum.fishing * m.fishing,
            meal_heal: sum.meal_heal * m.meal_heal,
            meal_time: sum.meal_time * m.meal_time,
            finds: sum.finds + m.finds,
            store_room: sum.store_room * m.store_room,
        })
    }
}

/// What brings an effect on. The weather's conditions hold only out in it (nothing solid
/// overhead), except wind, which reaches in under any roof that is not walled.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Condition {
    /// Feeling colder, or warmer, than this many °C.
    FeltBelow(f32),
    FeltAbove(f32),
    /// A fire or torch nearby warms the player in the cold.
    Warmed,
    /// Rain (not snow) falling at least this hard, 0 to 1.
    Rain(f32),
    /// Snow falling at least this hard.
    Snow(f32),
    /// Hail falling at least this hard.
    Hail(f32),
    /// Wind, gusts included, at least this many blocks a second.
    Wind(f32),
    /// Sand or dust in the air, at least this thick.
    Dust(f32),
    /// Lightning about at least this often (1 about once a minute).
    Storm(f32),
    /// Standing in a biome with this tag, or of this name (`forest`, `ocean`, `base:bog`).
    InBiome(String),
    /// Within reach of the player's home: the lit hearth, else the bed last slept in.
    AtHome,
    /// Under a roof (something solid overhead).
    Indoors,
    /// At home, with at least this many blocks carrying the tag around it. Besides block
    /// tags, `kept` counts the animals the player keeps around the home.
    HomeHas(String, u32),
    /// At home, with at least this share (above 0, at most 1) of the faces about the room
    /// it stands in this good a shelter or better: its walls and roof, an opening out of the
    /// home's reach counting against it (`HomeWalls(Sound, 0.6)`).
    HomeWalls(Shelter, f32),
    /// Under a roof while the inner condition (the weather) holds out in the open: the
    /// storm outside, from indoors.
    Sheltered(Box<Condition>),
    /// The item in the player's hand carries this tag (`Holding("fire")`: a torch).
    Holding(String),
    /// Something the player wears carries this tag (`Wearing("charge_charm")`: the charm
    /// in the charm slot, `inventory::WORN`).
    Wearing(String),
    /// A block carrying this tag lies within this many blocks of the player's feet, every
    /// way (`Near("fire", 4)`: a campfire close by). The reach is 1 to 8.
    Near(String, i32),
    /// Sitting on a seat (`core:sit`).
    Seated,
    /// Sitting on a seat carrying this tag (`Not(SeatedOn("footstool"))`: in a chair, not
    /// on the footstool before it).
    SeatedOn(String),
    /// After dark: the sun below the horizon (`calendar::sun_height`), from sunset to
    /// sunrise, a third of the day. An evening in by the fire, and a night's sleep.
    Night,
    /// The player has finished this quest (its full name, or its name alone in the package
    /// that lists the effect): what a quest taught stays with them, as an effect that holds
    /// from then on.
    Done(String),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

impl Condition {
    /// The quests `Done` asks after anywhere in this condition.
    fn quests<'a>(&'a self, into: &mut Vec<&'a str>) {
        match self {
            Condition::Done(quest) => into.push(quest),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.quests(into)),
            Condition::Not(c) => c.quests(into),
            _ => {}
        }
    }

    /// Names every quest `Done` asks after in full.
    fn qualify_quests(&mut self, qualify: &impl Fn(&mut String)) {
        match self {
            Condition::Done(quest) => qualify(quest),
            Condition::All(list) | Condition::Any(list) => list.iter_mut().for_each(|c| c.qualify_quests(qualify)),
            Condition::Not(c) => c.qualify_quests(qualify),
            _ => {}
        }
    }

    /// The shares `HomeWalls` asks anywhere in this condition.
    fn home_walls(&self, into: &mut Vec<f32>) {
        match self {
            Condition::HomeWalls(_, share) => into.push(*share),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.home_walls(into)),
            Condition::Not(c) | Condition::Sheltered(c) => c.home_walls(into),
            _ => {}
        }
    }

    /// The temperatures `FeltAbove` asks anywhere in this condition that feeling warmer
    /// brings it on: not under a `Not`, where feeling warmer takes it away.
    fn felt_above(&self, into: &mut Vec<f32>) {
        match self {
            Condition::FeltAbove(t) => into.push(*t),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.felt_above(into)),
            Condition::Sheltered(c) => c.felt_above(into),
            _ => {}
        }
    }

    /// The tags `HomeHas` asks of a home anywhere in this condition.
    fn home_tags<'a>(&'a self, into: &mut Vec<&'a str>) {
        match self {
            Condition::HomeHas(tag, _) => into.push(tag),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.home_tags(into)),
            Condition::Not(c) | Condition::Sheltered(c) => c.home_tags(into),
            _ => {}
        }
    }

    /// Whether `survey` has what this condition's `HomeHas` asks, taking every other ask
    /// (and anything under a `Not`) as met.
    fn home_met(&self, survey: &HomeSurvey) -> bool {
        match self {
            Condition::HomeHas(tag, n) => survey.count(tag) >= *n,
            Condition::All(list) => list.iter().all(|c| c.home_met(survey)),
            Condition::Any(list) => list.iter().any(|c| c.home_met(survey)),
            Condition::Sheltered(c) => c.home_met(survey),
            _ => true,
        }
    }

    /// The counts `HomeHas` asks of `tag` anywhere in this condition.
    fn home_needs(&self, tag: &str, into: &mut Vec<u32>) {
        match self {
            Condition::HomeHas(t, n) if t == tag => into.push(*n),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.home_needs(tag, into)),
            Condition::Not(c) | Condition::Sheltered(c) => c.home_needs(tag, into),
            _ => {}
        }
    }

    /// The tags and reaches `Near` asks about, the tags `Holding` asks of what the player
    /// has in hand, those `Wearing` asks of what they wear, and those `SeatedOn` asks of
    /// what they sit on, anywhere in this condition.
    fn asks<'a>(&'a self, near: &mut Vec<(&'a str, i32)>, held: &mut Vec<&'a str>, worn: &mut Vec<&'a str>, seat: &mut Vec<&'a str>) {
        match self {
            Condition::Near(tag, reach) => near.push((tag, *reach)),
            Condition::Holding(tag) => held.push(tag),
            Condition::Wearing(tag) => worn.push(tag),
            Condition::SeatedOn(tag) => seat.push(tag),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.asks(near, held, worn, seat)),
            Condition::Not(c) | Condition::Sheltered(c) => c.asks(near, held, worn, seat),
            _ => {}
        }
    }
}

/// What a home condition still lacks, as `/home` tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lack {
    /// A roof overhead.
    Roof,
    /// Out under the open sky, no roof overhead.
    Sky,
    /// Standing at home (the home survey is none).
    Away,
    /// Blocks with a tag: how many there are and how many are wanted.
    Tag { tag: String, have: u32, need: u32 },
    /// Walls of a shelter or better about the room: the share there is and the share
    /// wanted, in hundredths.
    Walls { shelter: Shelter, have: u32, need: u32 },
    /// Any one of these will do.
    OneOf(Vec<Vec<Lack>>),
}

impl Condition {
    /// Whether the condition asks the player to be out under the sky (`Not(Indoors)`, alone
    /// or among an `All`'s).
    fn wants_sky(&self) -> bool {
        match self {
            Condition::Not(inner) => **inner == Condition::Indoors,
            Condition::All(list) => list.iter().any(Condition::wants_sky),
            _ => false,
        }
    }

    /// Whether the condition is about a home (`AtHome` or `HomeHas` anywhere in it).
    fn about_home(&self) -> bool {
        match self {
            Condition::AtHome | Condition::HomeHas(..) | Condition::HomeWalls(..) => true,
            Condition::All(list) | Condition::Any(list) => list.iter().any(Condition::about_home),
            Condition::Not(c) => c.about_home(),
            _ => false,
        }
    }

    /// What this lacks at `s`, if it does not hold: only what a home can lack (a roof, the
    /// tags it wants, being there); the weather and the rest are never asked for.
    pub fn lacks(&self, s: &Situation) -> Vec<Lack> {
        if self.holds(s) {
            return Vec::new();
        }
        match self {
            Condition::AtHome => vec![Lack::Away],
            Condition::Indoors => vec![Lack::Roof],
            Condition::Not(inner) if **inner == Condition::Indoors => vec![Lack::Sky],
            Condition::HomeHas(tag, need) => vec![Lack::Tag { tag: tag.clone(), have: s.home.map_or(0, |h| h.count(tag)), need: *need }],
            Condition::HomeWalls(shelter, need) => {
                let have = s.home.map_or(0.0, |h| h.walls.share(*shelter));
                // Rounded down, so a share short of the need never reads as meeting it.
                vec![Lack::Walls { shelter: *shelter, have: (have * 100.0).floor() as u32, need: (need * 100.0).ceil() as u32 }]
            }
            Condition::All(list) => list.iter().flat_map(|c| c.lacks(s)).collect(),
            Condition::Any(list) => vec![Lack::OneOf(list.iter().map(|c| c.lacks(s)).collect())],
            _ => Vec::new(),
        }
    }
}

/// One chain of home steps (a ladder, or a pair of rooms) as `/home` tells it.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeProgress {
    /// The highest step of the chain the home has.
    pub reached: Option<EffectId>,
    /// The next one up and what it lacks, if there is one.
    pub next: Option<(EffectId, Vec<Lack>)>,
}

/// What the conditions are checked against: the player's state and the weather where they
/// stand.
#[derive(Clone, Copy, Debug)]
pub struct Situation<'a> {
    /// How warm the player feels, °C.
    pub felt: f32,
    pub warmed: bool,
    /// Nothing solid overhead.
    pub outdoors: bool,
    pub weather: &'a Weather,
    /// The biome where the player stands: its name and tags.
    pub biome: &'a str,
    pub biome_tags: &'a [String],
    /// What lies around the player's home, if they stand within reach of it.
    pub home: Option<&'a HomeSurvey>,
    /// The quests the player has finished (full names).
    pub done: &'a [String],
    /// The tags of the item in the player's hand.
    pub held: &'a [String],
    /// Which of the `Near` asks (`EffectRegistry::near_asks`) hold where the player stands.
    pub near: &'a [(String, i32)],
    /// The tags of what the player wears (of those `EffectRegistry::worn_tags` asks after).
    pub worn: &'a [String],
    /// The player sits on a seat.
    pub seated: bool,
    /// The tags of the seat they sit on (of those `EffectRegistry::seat_tags` asks after).
    pub seat: &'a [String],
    /// The sun is down (`Condition::Night`).
    pub night: bool,
}

/// What a player is sure to have to bring an effect on with (`EffectRegistry::can_feel`):
/// what they have, each with how many, whether they have a home, the quests they have done
/// (full names), and where the content wears each item.
pub struct Means<'a> {
    pub items: &'a [(BlockId, u32)],
    pub home: bool,
    pub done: &'a [String],
    /// Where each item is worn (`Content::worn_slots`).
    pub worn: &'a [Option<u8>],
}

/// How many blocks with each tag lie around a home: the tags are those effects ask of it
/// (`EffectRegistry::home_tags`). And the walls about the room it stands in, when effects
/// ask after them (`EffectRegistry::home_walls`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeSurvey {
    pub counts: Vec<(String, u32)>,
    pub walls: Walls,
}

impl HomeSurvey {
    pub fn count(&self, tag: &str) -> u32 {
        self.counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n)
    }
}

/// How well a wall keeps the weather out, as a building block's tag says:
/// `shelter:draughty` (hide, wattle, cloth: a solid block with no tag counts so too),
/// `shelter:sound` (logs, planks, cobble, daub, a shut door) or `shelter:fine` (dressed
/// stone, brick, glass). A better one counts for every
/// worse: fine walls are sound walls too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Shelter {
    Draughty,
    Sound,
    Fine,
}

impl Shelter {
    pub const ALL: [Shelter; 3] = [Shelter::Draughty, Shelter::Sound, Shelter::Fine];

    /// The block tag that marks it.
    pub fn tag(self) -> &'static str {
        match self {
            Shelter::Draughty => "shelter:draughty",
            Shelter::Sound => "shelter:sound",
            Shelter::Fine => "shelter:fine",
        }
    }

    /// Its name alone, for words (`home.shelter.<name>`).
    pub fn name(self) -> &'static str {
        self.tag().trim_start_matches("shelter:")
    }

    /// The best shelter among a block's tags, if it has one.
    pub fn of(tags: &[String]) -> Option<Shelter> {
        Shelter::ALL.into_iter().rev().find(|s| tags.iter().any(|t| t == s.tag()))
    }

    /// How a block shelters as a wall of a room: an explicit shelter tag always wins
    /// (cloth hung, a hay bale, a column); a solid block with none (earth, gravel, sand) is
    /// draughty; none for one that is no wall: open, or `furniture` (a furniture tag and no
    /// shelter tag), looked past to the wall behind.
    pub fn of_wall(tags: &[String], solid: bool, furniture: bool) -> Option<Shelter> {
        match Shelter::of(tags) {
            Some(shelter) => Some(shelter),
            None => (solid && !furniture).then_some(Shelter::Draughty),
        }
    }
}

/// What stands at a cell to a room check (`Walls::of_room`): a wall of its shelter
/// (`Shelter::of_wall`), else open; and whether it is solid, or furniture.
#[derive(Clone, Copy, Debug)]
pub struct RoomCell {
    pub wall: Option<Shelter>,
    pub solid: bool,
    pub furniture: bool,
}

/// The faces about a home's room: its walls and roof as a scan from the home out over the
/// open cells within its reach meets them, each one a face (its floor is not counted). A
/// face that leads out of the reach is an opening, of no shelter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Walls {
    /// Every face, openings with them.
    pub faces: u32,
    /// The faces of each shelter or better, by `Shelter` (draughty, sound, fine).
    pub at_least: [u32; 3],
}

impl Walls {
    /// A face of `shelter`, or an opening (`None`).
    pub fn add(&mut self, shelter: Option<Shelter>) {
        self.faces += 1;
        for s in Shelter::ALL.into_iter().filter(|&s| Some(s) <= shelter) {
            self.at_least[s as usize] += 1;
        }
    }

    /// The share of the faces this good a shelter or better: none for no room at all.
    pub fn share(&self, shelter: Shelter) -> f32 {
        if self.faces == 0 { 0.0 } else { self.at_least[shelter as usize] as f32 / self.faces as f32 }
    }

    /// The faces about the room at `anchor`, as the server judges a home's (its room
    /// check): from the first open cell at or over `anchor`, open cells within `reach`
    /// across, `up` and `down` of it are walked; each wall met is a face of its shelter,
    /// and a cell out of reach or not there (`cell` gives `None`) is an opening. A solid
    /// block or furniture under the room is floor, never stepped down into; the floor is
    /// not counted.
    pub fn of_room(anchor: glam::IVec3, reach: i32, up: i32, down: i32, cell: impl Fn(glam::IVec3) -> Option<RoomCell>) -> Self {
        use glam::IVec3;
        let inside = |p: IVec3| {
            let d = p - anchor;
            d.x.abs() <= reach && d.z.abs() <= reach && d.y <= up && d.y >= -down
        };
        let mut walls = Walls::default();
        // A hearth's home is its hearthstone, solid: the room is the first open cell over it.
        let Some(anchor) = (0..=up).map(|dy| anchor + IVec3::Y * dy).map_while(|p| cell(p).map(|c| (p, c))).find(|(_, c)| c.wall.is_none() && !c.solid).map(|(p, _)| p) else {
            return walls;
        };
        let mut seen = rustc_hash::FxHashSet::default();
        let mut queue = vec![anchor];
        seen.insert(anchor);
        while let Some(at) = queue.pop() {
            for side in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::Y, IVec3::NEG_Y] {
                let next = at + side;
                let face = side != IVec3::NEG_Y;
                let Some(c) = cell(next).filter(|_| inside(next)) else {
                    if face {
                        walls.add(None);
                    }
                    continue;
                };
                match c.wall {
                    Some(shelter) if face => walls.add(Some(shelter)),
                    Some(_) => {}
                    None if !face && (c.solid || c.furniture) => {}
                    None => {
                        if seen.insert(next) {
                            queue.push(next);
                        }
                    }
                }
            }
        }
        walls
    }

    /// The room check (`of_room`) on the least room a home can have, two cells tall on a
    /// solid floor, walled and roofed in the best walls among `items` (a solid block with a
    /// shelter tag), each as far as it goes, the rest of its nine faces left open.
    pub fn of_built_room(items: &[(BlockId, u32)], blocks: &crate::block::BlockRegistry) -> Self {
        use glam::IVec3;
        let mut walls: Vec<(Shelter, u32)> = items
            .iter()
            .filter(|&&(id, _)| blocks.info(id).solid)
            .filter_map(|&(id, n)| Shelter::of(&blocks.def(id).tags).map(|shelter| (shelter, n)))
            .collect();
        walls.sort_by(|a, b| b.0.cmp(&a.0));
        let mut faces: Vec<IVec3> = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].iter().flat_map(|&side| [side, side + IVec3::Y]).collect();
        faces.push(IVec3::Y * 2);
        let mut laid: Vec<(IVec3, Shelter)> = Vec::new();
        let mut walls = walls.into_iter().flat_map(|(shelter, n)| std::iter::repeat_n(shelter, n as usize));
        for face in faces {
            match walls.next() {
                Some(shelter) => laid.push((face, shelter)),
                None => break,
            }
        }
        Walls::of_room(IVec3::ZERO, 2, 2, 1, |p| {
            if p.y < 0 {
                return Some(RoomCell { wall: None, solid: true, furniture: false });
            }
            if p.x == 0 && p.z == 0 && p.y <= 1 {
                return Some(RoomCell { wall: None, solid: false, furniture: false });
            }
            laid.iter().find(|(at, _)| *at == p).map(|&(_, shelter)| RoomCell { wall: Some(shelter), solid: true, furniture: false })
        })
    }

    /// A room walled all about in `shelter`, `faces` of it.
    pub fn all(shelter: Shelter, faces: u32) -> Self {
        let mut walls = Walls::default();
        (0..faces).for_each(|_| walls.add(Some(shelter)));
        walls
    }
}

impl Condition {
    pub fn holds(&self, s: &Situation) -> bool {
        let w = s.weather;
        let falling = |share: f32| w.precipitation * share;
        match self {
            Condition::FeltBelow(t) => s.felt < *t,
            Condition::FeltAbove(t) => s.felt > *t,
            Condition::Warmed => s.warmed,
            Condition::Rain(x) => s.outdoors && falling(1.0 - w.snow - w.hail) >= *x,
            Condition::Snow(x) => s.outdoors && falling(w.snow) >= *x,
            Condition::Hail(x) => s.outdoors && falling(w.hail) >= *x,
            Condition::Wind(x) => w.wind.length() * (1.0 + w.gusts) >= *x,
            Condition::Dust(x) => s.outdoors && w.dust >= *x,
            Condition::Storm(x) => s.outdoors && w.lightning >= *x,
            Condition::InBiome(t) => s.biome_tags.contains(t) || s.biome == t || s.biome.rsplit(':').next() == Some(t.as_str()),
            Condition::AtHome => s.home.is_some(),
            Condition::Indoors => !s.outdoors,
            Condition::Seated => s.seated,
            Condition::SeatedOn(tag) => s.seated && s.seat.contains(tag),
            Condition::Night => s.night,
            Condition::HomeHas(tag, n) => s.home.is_some_and(|h| h.count(tag) >= *n),
            Condition::HomeWalls(shelter, share) => s.home.is_some_and(|h| h.walls.share(*shelter) >= *share),
            Condition::Sheltered(c) => !s.outdoors && c.holds(&Situation { outdoors: true, ..*s }),
            Condition::Done(quest) => s.done.iter().any(|d| d == quest),
            Condition::Holding(tag) => s.held.contains(tag),
            Condition::Wearing(tag) => s.worn.contains(tag),
            Condition::Near(tag, reach) => s.near.iter().any(|(t, r)| t == tag && r == reach),
            Condition::All(all) => all.iter().all(|c| c.holds(s)),
            Condition::Any(any) => any.iter().any(|c| c.holds(s)),
            Condition::Not(c) => !c.holds(s),
        }
    }
}

/// An effect a player has now, as the client is told: how many seconds it has left, or
/// none for one that lasts while its condition does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveEffect {
    pub effect: EffectId,
    pub seconds_left: Option<u16>,
}

/// Food with all of `tags` gives `effect` for `seconds` when eaten.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoodRule {
    pub tags: Vec<String>,
    pub effect: String,
    pub seconds: f32,
}

/// Sleeping at a bed gives `effect` for `seconds`, if the effect named by `needs` (when
/// there is one) would hold where the player sleeps, and the bed slept in carries the tag
/// `bed` names (when there is one: a proper bed, not a sleeping bag on the ground), and,
/// for a rule marked `night`, only a sleep after dark (`Condition::Night`): a nap by day
/// is no night's rest.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SleepRule {
    pub effect: String,
    pub seconds: f32,
    #[serde(default)]
    pub needs: Option<String>,
    #[serde(default)]
    pub bed: Option<String>,
    #[serde(default)]
    pub night: bool,
}

/// Coming home after `away` seconds or more away from it gives `effect` for `seconds`, if
/// the effect named by `needs` (when there is one) holds there: home again after a long day
/// out. The time away is counted only while the player has a home, and alive.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomecomingRule {
    pub effect: String,
    pub away: f32,
    pub seconds: f32,
    #[serde(default)]
    pub needs: Option<String>,
}

/// One package's `effects.ron`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EffectFile {
    #[serde(default)]
    pub effects: Vec<EffectDef>,
    #[serde(default)]
    pub foods: Vec<FoodRule>,
    #[serde(default)]
    pub sleeps: Vec<SleepRule>,
    #[serde(default)]
    pub homecomings: Vec<HomecomingRule>,
    /// Tags a home counts only where the block stands under the open sky (`yard`: things set
    /// out in the yard, not kept indoors). Each must be one a `HomeHas` asks.
    #[serde(default)]
    pub outdoors: Vec<String>,
}

/// Every effect, in the order packages list them.
#[derive(Debug, Default)]
pub struct EffectRegistry {
    defs: Vec<EffectDef>,
    by_name: FxHashMap<String, EffectId>,
    /// The effects each one hides.
    replaces: Vec<Vec<EffectId>>,
    foods: Vec<(Vec<String>, EffectId, f32)>,
    /// What sleeping gives: the effect, its seconds, the effect whose condition must hold,
    /// the bed tag it asks and whether only a night's sleep gives it.
    sleeps: Vec<(EffectId, f32, Option<EffectId>, Option<String>, bool)>,
    /// What coming home gives: the effect, its seconds, the seconds away it asks and the
    /// effect whose condition must hold.
    homecomings: Vec<(EffectId, f32, f32, Option<EffectId>)>,
    /// The tags effects ask of a home, each once.
    home_tags: Vec<String>,
    /// Those of them a home counts only under the open sky (`EffectFile::outdoors`).
    outdoor_tags: Vec<String>,
    /// Whether any effect asks after a home's walls (`HomeWalls`).
    home_walls: bool,
    /// The quests effects wait on, each once.
    quests: Vec<String>,
    /// The tags and reaches `Near` asks about, each once, and the tags `Holding` asks of
    /// the item in hand.
    near_asks: Vec<(String, i32)>,
    held_tags: Vec<String>,
    worn_tags: Vec<String>,
    seat_tags: Vec<String>,
}

impl EffectRegistry {
    /// Reads one package's `effects.ron`; unqualified names go into `namespace`.
    pub fn read(namespace: &str, source: &str) -> Result<EffectFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut file: EffectFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/effects.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        for def in &mut file.effects {
            qualify(&mut def.name);
            def.replaces.iter_mut().for_each(qualify);
            for when in def.when.iter_mut().chain(&mut def.ends) {
                when.qualify_quests(&qualify);
            }
            // Skills are all core's.
            for (skill, _) in &mut def.boosts {
                if !skill.contains(':') {
                    *skill = format!("core:{skill}");
                }
            }
            if def.icon.is_empty() {
                def.icon = def.name.rsplit(':').next().unwrap_or_default().to_owned();
            }
        }
        file.foods.iter_mut().for_each(|rule| qualify(&mut rule.effect));
        for rule in &mut file.sleeps {
            qualify(&mut rule.effect);
            rule.needs.iter_mut().for_each(qualify);
        }
        for rule in &mut file.homecomings {
            qualify(&mut rule.effect);
            rule.needs.iter_mut().for_each(qualify);
        }
        Ok(file)
    }

    pub fn new(files: Vec<EffectFile>) -> Result<Self, RegistryError> {
        let (mut defs, mut rules, mut sleeps, mut outdoors) = (Vec::new(), Vec::new(), Vec::new(), Vec::<String>::new());
        let mut homecomings = Vec::new();
        for file in files {
            defs.extend(file.effects);
            rules.extend(file.foods);
            sleeps.extend(file.sleeps);
            homecomings.extend(file.homecomings);
            outdoors.extend(file.outdoors);
        }
        let mut by_name = FxHashMap::default();
        for (i, def) in defs.iter().enumerate() {
            if by_name.insert(def.name.clone(), EffectId(i as u16)).is_some() {
                return Err(RegistryError(format!("duplicate effect `{}`", def.name)));
            }
        }
        let replaces = defs
            .iter()
            .map(|def| {
                def.replaces
                    .iter()
                    .map(|name| by_name.get(name).copied().ok_or_else(|| RegistryError(format!("effect `{}` replaces `{name}`, which does not exist", def.name))))
                    .collect()
            })
            .collect::<Result<_, _>>()?;
        let foods = rules
            .into_iter()
            .map(|rule| {
                let id = by_name.get(&rule.effect).copied().ok_or_else(|| RegistryError(format!("a food rule gives `{}`, which does not exist", rule.effect)))?;
                Ok((rule.tags, id, rule.seconds))
            })
            .collect::<Result<_, RegistryError>>()?;
        let sleeps = sleeps
            .into_iter()
            .map(|rule| {
                let id = by_name.get(&rule.effect).copied().ok_or_else(|| RegistryError(format!("a sleep rule gives `{}`, which does not exist", rule.effect)))?;
                if !(rule.seconds > 0.0) {
                    return Err(RegistryError(format!("the sleep rule for `{}` lasts {} seconds", rule.effect, rule.seconds)));
                }
                let needs = rule
                    .needs
                    .map(|name| {
                        let needed = by_name.get(&name).copied().ok_or_else(|| RegistryError(format!("the sleep rule for `{}` needs `{name}`, which does not exist", rule.effect)))?;
                        defs[needed.0 as usize].when.is_some().then_some(needed).ok_or_else(|| RegistryError(format!("the sleep rule for `{}` needs `{name}`, which has no `when` to hold", rule.effect)))
                    })
                    .transpose()?;
                Ok((id, rule.seconds, needs, rule.bed, rule.night))
            })
            .collect::<Result<_, RegistryError>>()?;
        // Coming home gives what only it gives (no `when` of its own), after some time away.
        let homecomings = homecomings
            .into_iter()
            .map(|rule: HomecomingRule| {
                let id = by_name.get(&rule.effect).copied().ok_or_else(|| RegistryError(format!("a homecoming gives `{}`, which does not exist", rule.effect)))?;
                if defs[id.0 as usize].when.is_some() {
                    return Err(RegistryError(format!("a homecoming gives `{}`, which comes on by its own `when`", rule.effect)));
                }
                if !(rule.seconds > 0.0 && rule.away > 0.0 && rule.seconds.is_finite() && rule.away.is_finite()) {
                    return Err(RegistryError(format!("the homecoming for `{}` asks {} seconds away and lasts {}: both above 0", rule.effect, rule.away, rule.seconds)));
                }
                let needs = rule
                    .needs
                    .map(|name| {
                        let needed = by_name.get(&name).copied().ok_or_else(|| RegistryError(format!("the homecoming for `{}` needs `{name}`, which does not exist", rule.effect)))?;
                        defs[needed.0 as usize].when.is_some().then_some(needed).ok_or_else(|| RegistryError(format!("the homecoming for `{}` needs `{name}`, which has no `when` to hold", rule.effect)))
                    })
                    .transpose()?;
                Ok((id, rule.seconds, rule.away, needs))
            })
            .collect::<Result<_, RegistryError>>()?;
        let mut tags: Vec<&str> = Vec::new();
        for def in &defs {
            for when in def.when.iter().chain(&def.ends) {
                when.home_tags(&mut tags);
            }
        }
        let (mut shares, mut home_walls) = (Vec::new(), false);
        for def in &defs {
            for when in def.when.iter().chain(&def.ends) {
                shares.clear();
                when.home_walls(&mut shares);
                if let Some(share) = shares.iter().find(|s| !(**s > 0.0 && **s <= 1.0)) {
                    return Err(RegistryError(format!("effect `{}` asks `HomeWalls` for a share of {share}: above 0, at most 1", def.name)));
                }
                home_walls |= !shares.is_empty();
            }
        }
        let mut home_tags: Vec<String> = Vec::new();
        for tag in tags {
            if !home_tags.iter().any(|t| t == tag) {
                home_tags.push(tag.to_owned());
            }
        }
        let mut asked: Vec<&str> = Vec::new();
        for def in &defs {
            for when in def.when.iter().chain(&def.ends) {
                when.quests(&mut asked);
            }
        }
        let mut quests: Vec<String> = Vec::new();
        for quest in asked {
            if !quests.iter().any(|q| q == quest) {
                quests.push(quest.to_owned());
            }
        }
        let (mut near, mut held, mut worn, mut seat) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for def in &defs {
            if !(def.modifiers.store_room.is_finite() && (1.0..=6.0).contains(&def.modifiers.store_room)) {
                return Err(RegistryError(format!("effect `{}` gives store_room {}: it takes 1 to 6", def.name, def.modifiers.store_room)));
            }
            if !(def.modifiers.home_warmth.is_finite() && def.modifiers.home_warmth >= 0.0) {
                return Err(RegistryError(format!("effect `{}` adds {} home warmth: a home only warms, 0 or more", def.name, def.modifiers.home_warmth)));
            }
            if !(def.lingers.is_finite() && def.lingers >= 0.0) || (def.lingers > 0.0 && def.when.is_none()) {
                return Err(RegistryError(format!("effect `{}` lingers {} seconds: it takes 0 or more, and a `when` to linger after", def.name, def.lingers)));
            }
            if def.ends.is_some() && def.lingers <= 0.0 {
                return Err(RegistryError(format!("effect `{}` has `ends` but does not linger: only what lingers can end early", def.name)));
            }
            for when in def.when.iter().chain(&def.ends) {
                when.asks(&mut near, &mut held, &mut worn, &mut seat);
            }
        }
        if let Some((tag, reach)) = near.iter().find(|(_, reach)| !(1..=8).contains(reach)) {
            return Err(RegistryError(format!("effects.ron: `Near(\"{tag}\", {reach})` reaches 1 to 8 blocks")));
        }
        let mut near_asks: Vec<(String, i32)> = Vec::new();
        for (tag, reach) in near {
            if !near_asks.iter().any(|(t, r)| t == tag && *r == reach) {
                near_asks.push((tag.to_owned(), reach));
            }
        }
        let once = |tags: Vec<&str>| {
            let mut once: Vec<String> = Vec::new();
            for tag in tags {
                if !once.iter().any(|t| t == tag) {
                    once.push(tag.to_owned());
                }
            }
            once
        };
        let (held_tags, worn_tags, seat_tags) = (once(held), once(worn), once(seat));
        if let Some(tag) = outdoors.iter().find(|tag| !home_tags.contains(tag)) {
            return Err(RegistryError(format!("effects.ron: `outdoors` names `{tag}`, which no `HomeHas` asks")));
        }
        let outdoor_tags = once(outdoors.iter().map(String::as_str).collect());
        Ok(Self { defs, by_name, replaces, foods, sleeps, homecomings, home_tags, outdoor_tags, home_walls, quests, near_asks, held_tags, worn_tags, seat_tags })
    }

    pub fn get(&self, id: EffectId) -> &EffectDef {
        &self.defs[id.0 as usize]
    }

    pub fn id(&self, name: &str) -> Option<EffectId> {
        self.by_name.get(name).copied()
    }

    /// An effect by its full name, or by its name alone when only one package has it (as a
    /// food's `effect:hearty:300` tag names it).
    pub fn find(&self, name: &str) -> Option<EffectId> {
        self.id(name).or_else(|| {
            let mut found = self.defs.iter().enumerate().filter(|(_, d)| d.name.rsplit(':').next() == Some(name));
            let (i, _) = found.next()?;
            found.next().is_none().then_some(EffectId(i as u16))
        })
    }

    pub fn ids(&self) -> impl Iterator<Item = EffectId> + '_ {
        (0..self.defs.len() as u16).map(EffectId)
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Levels `active` effects add to `skill`'s checks, capped at `level::MAX_BOOST` either way.
    pub fn boost(&self, active: &[ActiveEffect], skill: &str) -> i8 {
        let sum: i32 = active.iter().flat_map(|a| self.get(a.effect).boosts.iter()).filter(|(s, _)| s == skill).map(|&(_, n)| n as i32).sum();
        sum.clamp(-crate::level::MAX_BOOST as i32, crate::level::MAX_BOOST as i32) as i8
    }

    /// The effects a player has: those whose conditions hold, and the timed ones given
    /// (with seconds left), less any replaced by another present; buffs first, each kind
    /// in the order content lists them.
    pub fn active(&self, situation: &Situation, timed: &[(EffectId, f32)]) -> Vec<ActiveEffect> {
        let mut active: Vec<ActiveEffect> = timed.iter().map(|&(effect, left)| ActiveEffect { effect, seconds_left: Some(left.ceil().clamp(0.0, u16::MAX as f32) as u16) }).collect();
        for id in self.ids() {
            // One that lingers comes as a timed effect (`linger`), never while it holds only.
            if self.get(id).lingers > 0.0 {
                continue;
            }
            if !active.iter().any(|a| a.effect == id) && self.get(id).when.as_ref().is_some_and(|c| c.holds(situation)) {
                active.push(ActiveEffect { effect: id, seconds_left: None });
            }
        }
        let hidden: Vec<EffectId> = active.iter().flat_map(|a| self.replaces[a.effect.0 as usize].iter().copied()).collect();
        active.retain(|a| !hidden.contains(&a.effect));
        active.sort_by_key(|a| (self.get(a.effect).kind == Kind::Debuff, a.effect));
        active
    }

    /// The combined modifiers of some effects.
    pub fn modifiers(&self, active: &[ActiveEffect]) -> Modifiers {
        Modifiers::combine(active.iter().map(|a| &self.get(a.effect).modifiers))
    }

    /// The timed effects food with these tags gives when eaten: the food rules' whose
    /// tags it all has, and its own `effect:NAME:SECONDS` tags.
    pub fn given_by(&self, tags: &[String]) -> Vec<(EffectId, f32)> {
        let has = |tag: &String| tags.contains(tag);
        let ruled = self.foods.iter().filter(|(needs, ..)| needs.iter().all(has)).map(|&(_, id, seconds)| (id, seconds));
        let own = tags.iter().filter_map(|tag| {
            let (name, seconds) = tag.strip_prefix("effect:")?.rsplit_once(':')?;
            Some((self.find(name)?, seconds.parse::<f32>().ok().filter(|s| *s > 0.0)?))
        });
        let mut given: Vec<(EffectId, f32)> = Vec::new();
        for (id, seconds) in ruled.chain(own) {
            match given.iter_mut().find(|(e, _)| *e == id) {
                Some((_, longest)) => *longest = longest.max(seconds),
                None => given.push((id, seconds)),
            }
        }
        given
    }

    /// Whether `effect` can be had with only `means`, by the rules the server gives effects
    /// by: a food of `means` (one that fills) eaten (`given_by`), else its `when` met as
    /// each tick judges it (`linger`, then `active`, so a replaced one is not had). That is
    /// at some hour, in or out, sat (with a `seat`) or standing, in mild air or cold (and
    /// then warmed, with a `fire`, as the server warms only below `vitals::CHILLY`), with
    /// one thing of `means` in hand or none and one charm worn or none, all of it set within
    /// every reach, and at home (with one) or away. The home counts what `means` has of each
    /// tag, every one of it set about it, and its room is the server's room check
    /// (`Walls::of_room`) on a room of two cells walled and roofed in the best of `means`'
    /// walls, an opening where it runs short. The weather is calm: a feeling that needs
    /// weather or a biome is none a player can count on.
    pub fn can_feel(&self, effect: EffectId, means: &Means, blocks: &crate::block::BlockRegistry) -> bool {
        let tags = |id: BlockId| &blocks.def(id).tags;
        if means.items.iter().any(|&(id, _)| blocks.info(id).food > 0 && self.given_by(tags(id)).iter().any(|&(e, _)| e == effect)) {
            return true;
        }
        let has = |tag: &str| means.items.iter().any(|&(id, _)| blocks.def(id).has_tag(tag));
        let near: Vec<(String, i32)> = self.near_asks.iter().filter(|(tag, _)| has(tag)).cloned().collect();
        // One thing at a time in hand, one charm worn: each set of the tags effects ask, once.
        let mut helds: Vec<Vec<String>> = vec![Vec::new()];
        let mut worns: Vec<Vec<String>> = vec![Vec::new()];
        // Sat, on one of the seats of `means`: each set of the tags effects ask of it, once.
        let mut seats: Vec<Vec<String>> = Vec::new();
        for &(id, _) in means.items {
            if blocks.def(id).has_tag(crate::action::SEAT_TAG) {
                let seat = self.seat_by(blocks.def(id));
                if !seats.contains(&seat) {
                    seats.push(seat);
                }
            }
            let held = self.held_by(blocks.def(id));
            if !helds.contains(&held) {
                helds.push(held);
            }
            let worn = self.worn_by(means.worn.get(id.0 as usize).copied().flatten().map(usize::from), blocks.def(id));
            if !worns.contains(&worn) {
                worns.push(worn);
            }
        }
        let counts = self.home_tags.iter().map(|tag| (tag.clone(), means.items.iter().filter(|&&(id, _)| blocks.def(id).has_tag(tag)).map(|&(_, n)| n).sum())).collect();
        let survey = HomeSurvey { counts, walls: if means.home { Walls::of_built_room(means.items, blocks) } else { Walls::default() } };
        let weather = Weather::NONE;
        // Mild air, or cold, and in the cold a fire's least warming heat (with a `fire`).
        use crate::vitals::{felt_air, warmed, CHILLY, WARMING};
        let cold = CHILLY - 1.0;
        let mut airs = vec![(15.0, 0.0), (cold, 0.0)];
        if has("fire") {
            airs.push((cold, WARMING.next_up()));
        }
        for (air, heat) in airs {
            let (felt, warmed) = (felt_air(air, heat), warmed(heat, air));
            for bits in 0..16u8 {
                let bit = |n: u8| bits & (1 << n) != 0;
                let (outdoors, night, seated, at_home) = (bit(0), bit(1), bit(2), bit(3));
                if (seated && !has(crate::action::SEAT_TAG)) || (at_home && !means.home) {
                    continue;
                }
                let standing = [Vec::new()];
                for seat in if seated { &seats[..] } else { &standing[..] } {
                    for held in &helds {
                        for worn in &worns {
                            let situation = Situation { felt, warmed, outdoors, weather: &weather, biome: "", biome_tags: &[], home: at_home.then_some(&survey), done: means.done, held, near: &near, worn, seated, seat, night };
                            let mut timed = Vec::new();
                            self.linger(&situation, &mut timed);
                            if self.active(&situation, &timed).iter().any(|a| a.effect == effect) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Whether effect `winner` hides `loser` while it lasts.
    pub fn hides(&self, winner: EffectId, loser: EffectId) -> bool {
        self.replaces[winner.0 as usize].contains(&loser)
    }

    /// Where each chain of home steps stands at `situation`: the announced effects about a
    /// home, grouped by what replaces what, ordered by how much each replaces (the lowest
    /// first), a chain's highest step met and the lowest one it has yet to reach with what
    /// that lacks.
    pub fn home_progress(&self, situation: &Situation) -> Vec<HomeProgress> {
        let steps: Vec<EffectId> = self.ids().filter(|&id| self.get(id).announce && self.get(id).ladder == "home" && self.get(id).when.as_ref().is_some_and(Condition::about_home)).collect();
        // Chains: steps joined by one replacing the other.
        let mut chain: Vec<usize> = (0..steps.len()).collect();
        for (i, &a) in steps.iter().enumerate() {
            for (j, &b) in steps.iter().enumerate() {
                if self.replaces[a.0 as usize].contains(&b) {
                    let (from, to) = (chain[j], chain[i]);
                    chain.iter_mut().filter(|c| **c == from).for_each(|c| *c = to);
                }
            }
        }
        let mut firsts: Vec<usize> = chain.clone();
        firsts.sort_unstable();
        firsts.dedup();
        firsts
            .into_iter()
            .map(|c| {
                let mut members: Vec<EffectId> = steps.iter().zip(&chain).filter(|(_, k)| **k == c).map(|(&id, _)| id).collect();
                members.sort_by_key(|id| (self.replaces[id.0 as usize].len(), *id));
                // A step that holds only out of doors (a dooryard) is judged as if the player
                // stood in the yard, as the rest are as if they stood in the room.
                let outside = Situation { outdoors: true, ..*situation };
                let at = |w: &Condition| if w.wants_sky() { &outside } else { situation };
                let holds = |id: EffectId| self.get(id).when.as_ref().is_some_and(|w| w.holds(at(w)));
                // The highest step met, and the next one above it: a lower step left unmet
                // (a hearth lit with no torch by the bed) is behind, not next.
                let top = members.iter().rposition(|&id| holds(id));
                let reached = top.map(|i| members[i]);
                let next = members[top.map_or(0, |i| i + 1)..].iter().copied().find(|&id| !holds(id)).map(|id| (id, self.get(id).when.as_ref().map(|w| w.lacks(at(w))).unwrap_or_default()));
                HomeProgress { reached, next }
            })
            .collect()
    }

    /// The tags effects ask of a home (`HomeHas`): what surveying a home counts.
    /// Tops up `timed` with every effect that lingers whose `when` holds at `situation`:
    /// each kept at no less than its `lingers` seconds, so it lasts that long once it stops
    /// holding; one whose `when` no longer holds and whose `ends` does is gone at once.
    pub fn linger(&self, situation: &Situation, timed: &mut Vec<(EffectId, f32)>) {
        for id in self.ids() {
            let def = self.get(id);
            if def.lingers <= 0.0 {
                continue;
            }
            if !def.when.as_ref().is_some_and(|c| c.holds(situation)) {
                if def.ends.as_ref().is_some_and(|c| c.holds(situation)) {
                    timed.retain(|(e, _)| *e != id);
                }
                continue;
            }
            match timed.iter_mut().find(|(e, _)| *e == id) {
                Some((_, left)) => *left = left.max(def.lingers),
                None => timed.push((id, def.lingers)),
            }
        }
    }

    /// The tags and reaches effects ask about with `Near`, each once: the server looks for
    /// them around each player.
    pub fn near_asks(&self) -> &[(String, i32)] {
        &self.near_asks
    }

    /// What effects ask (`Holding`) of `item` in hand: those of its tags they ask after.
    pub fn held_by(&self, item: &crate::block::BlockDef) -> Vec<String> {
        self.held_tags.iter().filter(|tag| item.has_tag(tag)).cloned().collect()
    }

    /// What effects ask (`Wearing`) of `item` worn in `slot` (a `combat::Slot` index): only
    /// the charm slot's, so one charm's effect at a time, whatever else a pack tags. Nothing
    /// for what is worn elsewhere, or not at all.
    pub fn worn_by(&self, slot: Option<usize>, item: &crate::block::BlockDef) -> Vec<String> {
        if slot != Some(crate::combat::Slot::Charm.index()) {
            return Vec::new();
        }
        self.worn_tags.iter().filter(|tag| item.has_tag(tag)).cloned().collect()
    }

    /// The tags effects ask of the item in hand with `Holding`.
    pub fn held_tags(&self) -> &[String] {
        &self.held_tags
    }

    /// The tags effects ask of what is worn with `Wearing`.
    pub fn worn_tags(&self) -> &[String] {
        &self.worn_tags
    }

    /// What effects ask (`SeatedOn`) of `seat` sat on: those of its tags they ask after.
    pub fn seat_by(&self, seat: &crate::block::BlockDef) -> Vec<String> {
        self.seat_tags.iter().filter(|tag| seat.has_tag(tag)).cloned().collect()
    }

    /// The tags effects ask of the seat sat on with `SeatedOn`.
    pub fn seat_tags(&self) -> &[String] {
        &self.seat_tags
    }

    pub fn home_tags(&self) -> &[String] {
        &self.home_tags
    }

    /// The coolest a debuff comes on at as the player feels warmer (`FeltAbove`): Hot.
    /// None when no debuff asks.
    pub fn too_warm_at(&self) -> Option<f32> {
        let mut at = Vec::new();
        for def in self.defs.iter().filter(|d| d.kind == Kind::Debuff) {
            for when in def.when.iter() {
                when.felt_above(&mut at);
            }
        }
        at.into_iter().reduce(f32::min)
    }

    /// The highest announced effect (the last listed) whose home asks the home met in
    /// `before` and not in `after`, short in one of `tags`, with that tag and the count it
    /// wants: what a home loses when its lights burn out. Only `HomeHas` asks are judged (the
    /// rest, standing at home or a roof, are taken as met).
    pub fn falls_short(&self, tags: &[&str], before: &HomeSurvey, after: &HomeSurvey) -> Option<(EffectId, String, u32)> {
        self.ids()
            .filter(|&id| self.get(id).announce && self.get(id).when.as_ref().is_some_and(|when| when.home_met(before) && !when.home_met(after)))
            .filter_map(|id| Some((id, self.short_in(id, tags, after, |tag| before.count(tag))?)))
            .last()
            .map(|(id, (tag, need))| (id, tag, need))
    }

    /// Of the steps in `had`, the highest (the last listed) the home in `now` no longer
    /// meets, short in one of `tags`, with that tag and the count it wants: what lights
    /// burnt out while the player was away cost.
    pub fn lost(&self, had: &[EffectId], tags: &[&str], now: &HomeSurvey) -> Option<(EffectId, String, u32)> {
        self.ids()
            .filter(|id| had.contains(id) && self.get(*id).when.as_ref().is_some_and(|when| !when.home_met(now)))
            .filter_map(|id| Some((id, self.short_in(id, tags, now, |_| u32::MAX)?)))
            .last()
            .map(|(id, (tag, need))| (id, tag, need))
    }

    /// The announced steps asking a home for something (`HomeHas`) whose asks `survey` meets.
    pub fn home_steps(&self, survey: &HomeSurvey) -> Vec<EffectId> {
        self.ids()
            .filter(|&id| {
                let def = self.get(id);
                def.announce && def.when.as_ref().is_some_and(|when| when.about_home() && when.home_met(survey))
            })
            .collect()
    }

    /// The first of `tags` effect `id` wants more of than `now` has (and no more than `had`
    /// of it), with the most it wants.
    fn short_in(&self, id: EffectId, tags: &[&str], now: &HomeSurvey, had: impl Fn(&str) -> u32) -> Option<(String, u32)> {
        let when = self.get(id).when.as_ref()?;
        let mut needs = Vec::new();
        tags.iter().find_map(|&tag| {
            needs.clear();
            when.home_needs(tag, &mut needs);
            needs.iter().copied().filter(|&n| now.count(tag) < n && n <= had(tag)).max().map(|n| (tag.to_owned(), n))
        })
    }

    /// Whether a home counts blocks with `tag` only under the open sky.
    pub fn outdoor(&self, tag: &str) -> bool {
        self.outdoor_tags.iter().any(|t| t == tag)
    }

    /// Whether any effect asks after a home's walls: what surveying a home scans its room for.
    pub fn home_walls(&self) -> bool {
        self.home_walls
    }

    /// The announced steps of every ladder, by the names `Built` goals and tallies use:
    /// the effect's package, then the ladder and the effect's own name (`base:home_snug`).
    pub fn steps(&self) -> Vec<String> {
        self.defs
            .iter()
            .filter(|def| def.announce)
            .map(|def| {
                let (package, own) = def.name.split_once(':').unwrap_or(("", &def.name));
                format!("{package}:{}_{own}", def.ladder)
            })
            .collect()
    }

    /// The announced step named `name` (as `steps` gives it), if there is one.
    pub fn step(&self, name: &str) -> Option<&EffectDef> {
        self.defs.iter().filter(|def| def.announce).find(|def| {
            let (package, own) = def.name.split_once(':').unwrap_or(("", &def.name));
            name.strip_prefix(package).and_then(|rest| rest.strip_prefix(':')).and_then(|rest| rest.strip_prefix(def.ladder.as_str())).and_then(|rest| rest.strip_prefix('_')) == Some(own)
        })
    }

    /// The quests (full names) some effect waits on, each once: content checks they exist.
    pub fn quests(&self) -> &[String] {
        &self.quests
    }

    /// The bed tags sleep rules ask for, each once: content checks some bed carries each.
    pub fn sleep_beds(&self) -> impl Iterator<Item = &str> {
        let mut tags: Vec<&str> = self.sleeps.iter().filter_map(|(_, _, _, bed, _)| bed.as_deref()).collect();
        tags.sort_unstable();
        tags.dedup();
        tags.into_iter()
    }

    /// Whether a sleep giving `given` (`given_by_sleep`) is held back from a player with
    /// `active`: its best rest (`best_rest`), or one that hides it, is on them already, or
    /// something else it gives (Healing Sleep's mend) is still running. A nap's Rested, which
    /// the better rest hides, does not keep them from a night's better rest once its mend is
    /// over; a second nap rests no more, and no bed after bed mends again in one night.
    pub fn rested(&self, active: &[(EffectId, f32)], given: &[(EffectId, f32)]) -> bool {
        let Some(best) = self.best_rest(given) else {
            return self.sleeps.iter().any(|(effect, ..)| active.iter().any(|(a, _)| a == effect));
        };
        let running = |&(a, _): &(EffectId, f32)| a == best || self.hides(a, best) || (given.iter().any(|&(g, _)| g == a) && !self.hides(best, a));
        active.iter().any(running)
    }

    /// The best rest among `given`: the one none of the others hides (the one a bed says).
    pub fn best_rest(&self, given: &[(EffectId, f32)]) -> Option<EffectId> {
        given.iter().map(|&(id, _)| id).filter(|&id| !given.iter().any(|&(other, _)| other != id && self.hides(other, id))).next_back()
    }

    /// Whether some sleep gives `effect` (`sleeps`).
    pub fn from_sleep(&self, effect: EffectId) -> bool {
        self.sleeps.iter().any(|rule| rule.0 == effect)
    }

    /// The timed effects sleeping at a bed tagged `bed` gives where `situation` is: each
    /// sleep rule whose needed effect would hold there and whose bed tag the bed carries,
    /// and each effect once, for the longest of its rules that hold (two ways to the same
    /// rest: a snug home, or a lit corner by the bed).
    pub fn given_by_sleep(&self, situation: &Situation, bed: &[String]) -> Vec<(EffectId, f32)> {
        let mut given: Vec<(EffectId, f32)> = Vec::new();
        let holds = |needs: &Option<EffectId>, tag: &Option<String>, night: bool| {
            needs.is_none_or(|n| self.get(n).when.as_ref().is_some_and(|c| c.holds(situation))) && tag.as_ref().is_none_or(|tag| bed.contains(tag)) && (!night || situation.night)
        };
        for &(id, seconds, ..) in self.sleeps.iter().filter(|(_, _, needs, tag, night)| holds(needs, tag, *night)) {
            match given.iter_mut().find(|(e, _)| *e == id) {
                Some(had) => had.1 = had.1.max(seconds),
                None => given.push((id, seconds)),
            }
        }
        given
    }

    /// The timed effects coming home after `away` seconds away gives where `situation` is:
    /// each homecoming rule asking no more time away whose needed effect holds there, each
    /// effect once, for the longest of its rules.
    pub fn given_by_homecoming(&self, situation: &Situation, away: f32) -> Vec<(EffectId, f32)> {
        let mut given: Vec<(EffectId, f32)> = Vec::new();
        let needed = |needs: &Option<EffectId>| needs.is_none_or(|n| self.get(n).when.as_ref().is_some_and(|c| c.holds(situation)));
        for &(id, seconds, ..) in self.homecomings.iter().filter(|(_, _, asks, needs)| away >= *asks && needed(needs)) {
            match given.iter_mut().find(|(e, _)| *e == id) {
                Some(had) => had.1 = had.1.max(seconds),
                None => given.push((id, seconds)),
            }
        }
        given
    }

    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for def in &self.defs {
            for b in def.name.bytes().chain([0]) {
                h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Warmth adds as it is; a home's warmth lifts what that comes to up to the top of the
    /// comfortable band and no further, and adds nothing at or above it.
    #[test]
    fn a_homes_warmth_tops_out_at_the_comfortable_band() {
        let home = Modifiers { home_warmth: 15.0, ..Default::default() };
        assert_eq!(home.felt(0.0, 29.0), 15.0, "a cold night gets all of it");
        assert_eq!(home.felt(20.0, 29.0), 29.0, "a mild day is lifted to the top, not past it");
        assert_eq!(home.felt(29.0, 29.0), 29.0);
        assert_eq!(home.felt(34.0, 29.0), 34.0, "a hot day is no hotter, nor cooler");
        let soaked = Modifiers { warmth: -4.0, home_warmth: 2.0, ..Default::default() };
        assert_eq!(soaked.felt(10.0, 29.0), 8.0, "plain warmth first, then the home's");
        let combined = Modifiers::combine([&home, &soaked]);
        assert_eq!((combined.warmth, combined.home_warmth), (-4.0, 17.0));
    }

    /// A shelter tag always wins, furniture or not; a solid untagged block is draughty
    /// unless it is furniture; anything else is no wall.
    #[test]
    fn a_shelter_tag_always_wins_a_wall() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(Shelter::of_wall(&tags(&["decor", "shelter:draughty"]), true, true), Some(Shelter::Draughty), "cloth");
        assert_eq!(Shelter::of_wall(&tags(&["shelter:sound"]), false, false), Some(Shelter::Sound), "a shaped roof");
        assert_eq!(Shelter::of_wall(&tags(&["soil"]), true, false), Some(Shelter::Draughty), "earth");
        assert_eq!(Shelter::of_wall(&tags(&["storage"]), true, true), None, "a chest");
        assert_eq!(Shelter::of_wall(&tags(&[]), false, false), None, "air");
    }

    const EFFECTS: &str = r#"(effects: [
        (name: "by_the_fire", kind: Buff, title: "By the fire", description: "", when: Some(Warmed), modifiers: (heal: 2.0)),
        (name: "chilly", kind: Debuff, title: "Chilly", description: "", when: Some(FeltBelow(6.0)), modifiers: (heal: 0.5, speed: 0.9)),
        (name: "freezing", kind: Debuff, title: "Freezing", description: "", when: Some(FeltBelow(-6.0)), replaces: ["chilly"], modifiers: (heal: 0.0, hurt_every: Some(10.0))),
        (name: "soaked", kind: Debuff, title: "Soaked", description: "", when: Some(Rain(0.3)), modifiers: (warmth: -4.0)),
        (name: "hearty", kind: Buff, icon: "meat", title: "Hearty", description: "", modifiers: (max_health: 4)),
    ], foods: [(tags: ["meat", "cooked"], effect: "hearty", seconds: 200)])"#;

    fn registry() -> EffectRegistry {
        EffectRegistry::new(vec![EffectRegistry::read("core", EFFECTS).unwrap()]).unwrap()
    }

    fn situation(weather: &Weather, warmed: bool, felt: f32, outdoors: bool) -> Situation<'_> {
        Situation { felt, warmed, outdoors, weather, biome: "base:plains", biome_tags: &[], home: None, done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false }
    }

    #[test]
    fn conditions_bring_effects_on_and_stronger_ones_replace_milder() {
        let effects = registry();
        let calm = Weather::NONE;
        let names = |active: Vec<ActiveEffect>| active.iter().map(|a| effects.get(a.effect).name.clone()).collect::<Vec<_>>();
        assert!(effects.active(&situation(&calm, false, 15.0, true), &[]).is_empty());
        assert_eq!(names(effects.active(&situation(&calm, true, 2.0, true), &[])), ["core:by_the_fire", "core:chilly"]);
        assert_eq!(names(effects.active(&situation(&calm, false, -10.0, true), &[])), ["core:freezing"]);
        let rain = Weather { precipitation: 0.8, ..Weather::NONE };
        assert_eq!(names(effects.active(&situation(&rain, false, 15.0, true), &[])), ["core:soaked"]);
        assert!(effects.active(&situation(&rain, false, 15.0, false), &[]).is_empty(), "rain under a roof soaks no one");
        let snow = Weather { precipitation: 0.8, snow: 1.0, ..Weather::NONE };
        assert!(effects.active(&situation(&snow, false, 15.0, true), &[]).is_empty(), "snow is not rain");
    }

    /// What is in hand and what is near: `Holding` and `Near` hold off the situation's
    /// lists; an effect that `lingers` is no ambient one, but is topped up as a timed one
    /// while it holds and then runs down; asks are listed each once and checked at load.
    #[test]
    fn smoke_in_hand_or_near_clings_for_its_lingering_seconds() {
        let source = r#"(effects: [
            (name: "smoky", kind: Buff, title: "Smoky", description: "", when: Some(Any([Holding("fire"), Near("fire", 2), Near("fire", 2)])), lingers: 30.0),
        ])"#;
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).unwrap();
        assert_eq!(effects.near_asks(), [("fire".to_owned(), 2)], "each ask once");
        assert_eq!(effects.held_tags(), ["fire"]);
        let smoky = effects.find("smoky").unwrap();
        let calm = Weather::NONE;
        let (torch, fire) = (["fire".to_owned()], [("fire".to_owned(), 2)]);
        let holding = Situation { held: &torch, ..situation(&calm, false, 15.0, true) };
        let near = Situation { near: &fire, ..situation(&calm, false, 15.0, true) };
        let away = situation(&calm, false, 15.0, true);
        assert!(effects.active(&holding, &[]).is_empty(), "it comes as a timed effect, not while it holds");
        let mut timed = Vec::new();
        effects.linger(&holding, &mut timed);
        assert_eq!(timed, [(smoky, 30.0)]);
        timed[0].1 = 5.0;
        effects.linger(&near, &mut timed);
        assert_eq!(timed, [(smoky, 30.0)], "topped up, once");
        timed[0].1 = 5.0;
        effects.linger(&away, &mut timed);
        assert_eq!(timed, [(smoky, 5.0)], "away from smoke it runs down");
        assert_eq!(effects.active(&away, &timed).len(), 1);
        for bad in [r#"when: Some(Near("fire", 0)), lingers: 1.0"#, r#"when: Some(Near("fire", 9))"#, "lingers: 5.0", r#"when: Some(Holding("fire")), lingers: -1.0"#] {
            let source = format!(r#"(effects: [(name: "smoky", kind: Buff, title: "Smoky", description: "", {bad})])"#);
            assert!(EffectRegistry::new(vec![EffectRegistry::read("core", &source).unwrap()]).is_err(), "{bad}");
        }
    }

    /// What is worn: `Wearing` holds off the situation's worn tags, and only them: the same
    /// charm in hand is no charm worn.
    #[test]
    fn a_charm_works_while_worn_not_while_held() {
        let source = r#"(effects: [
            (name: "stride", kind: Buff, title: "Stride", description: "", when: Some(Wearing("charge_charm")), modifiers: (speed: 1.05)),
        ])"#;
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).unwrap();
        assert_eq!(effects.worn_tags(), ["charge_charm"]);
        assert!(effects.held_tags().is_empty(), "wearing asks nothing of the hand");
        let calm = Weather::NONE;
        let charm = ["charge_charm".to_owned()];
        let wearing = Situation { worn: &charm, ..situation(&calm, false, 15.0, true) };
        let holding = Situation { held: &charm, ..situation(&calm, false, 15.0, true) };
        let active = effects.active(&wearing, &[]);
        assert_eq!(active.len(), 1);
        assert_eq!(effects.modifiers(&active).speed, 1.05);
        assert!(effects.active(&holding, &[]).is_empty(), "held, not worn");
    }

    /// Rain soaks you, and you stay wet under a roof until you dry off by a fire (`ends`),
    /// or it wears off; out in the rain by a fire you stay wet.
    #[test]
    fn wet_through_you_dry_off_by_a_fire_and_not_before() {
        let source = r#"(effects: [
            (name: "soaked", kind: Debuff, title: "Soaked", description: "", when: Some(Rain(0.3)), lingers: 120.0, ends: Some(Any([Warmed, Near("fire", 3)]))),
        ])"#;
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).unwrap();
        assert_eq!(effects.near_asks(), [("fire".to_owned(), 3)], "what ends it is asked for too");
        let soaked = effects.find("soaked").unwrap();
        let (rain, fire) = (Weather { precipitation: 0.6, ..Weather::NONE }, [("fire".to_owned(), 3)]);
        let mut timed = Vec::new();
        effects.linger(&situation(&rain, false, 15.0, true), &mut timed);
        assert_eq!(timed, [(soaked, 120.0)]);
        effects.linger(&Situation { near: &fire, ..situation(&rain, false, 15.0, true) }, &mut timed);
        assert_eq!(timed, [(soaked, 120.0)], "out in the rain, a fire does not dry you");
        effects.linger(&situation(&rain, false, 15.0, false), &mut timed);
        assert_eq!(timed, [(soaked, 120.0)], "indoors, still wet");
        effects.linger(&Situation { near: &fire, ..situation(&rain, false, 15.0, false) }, &mut timed);
        assert!(timed.is_empty(), "indoors by a fire, dry");
        let quest = r#"(effects: [(name: "x", kind: Debuff, title: "X", description: "", when: Some(Rain(0.3)), lingers: 60.0, ends: Some(Done("dry_kit")))])"#;
        let quest = EffectRegistry::new(vec![EffectRegistry::read("core", quest).unwrap()]).unwrap();
        assert_eq!(quest.quests(), ["core:dry_kit"], "a quest in `ends` is qualified by the package too");
        let bad = r#"(effects: [(name: "x", kind: Debuff, title: "X", description: "", when: Some(Rain(0.3)), ends: Some(Warmed))])"#;
        assert!(EffectRegistry::new(vec![EffectRegistry::read("core", bad).unwrap()]).is_err(), "only what lingers ends");
    }

    /// A step's store_room grows stores 1 to 6 times; less (a shrinking store) or more is
    /// refused at launch, and steps together multiply.
    #[test]
    fn store_room_is_one_to_six() {
        let read = |room: &str| EffectRegistry::new(vec![EffectRegistry::read("core", &format!(r#"(effects: [(name: "roomy", kind: Buff, title: "Roomy", description: "", modifiers: (store_room: {room}))])"#)).unwrap()]);
        assert!(read("2.0").is_ok());
        for bad in ["0.5", "6.5", "inf"] {
            assert!(read(bad).is_err_and(|e| e.0.contains("store_room")), "store_room {bad}");
        }
        let two = Modifiers { store_room: 2.0, ..Modifiers::default() };
        assert_eq!(Modifiers::combine([&two, &two]).store_room, 4.0);
    }

    #[test]
    fn biomes_are_matched_by_tag_or_name() {
        let calm = Weather::NONE;
        let tags = ["wetland".to_owned(), "cold".to_owned()];
        let bog = Situation { biome: "base:bog", biome_tags: &tags, ..situation(&calm, false, 15.0, true) };
        assert!(Condition::InBiome("wetland".into()).holds(&bog));
        assert!(Condition::InBiome("bog".into()).holds(&bog));
        assert!(Condition::InBiome("base:bog".into()).holds(&bog));
        assert!(!Condition::InBiome("forest".into()).holds(&bog));
    }

    #[test]
    fn timed_effects_come_from_food_and_buffs_go_first() {
        let effects = registry();
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let hearty = effects.id("core:hearty").unwrap();
        assert_eq!(effects.given_by(&tags(&["meat", "cooked", "food:5"])), [(hearty, 200.0)]);
        assert!(effects.given_by(&tags(&["meat", "raw"])).is_empty());
        let given = effects.given_by(&tags(&["meat", "cooked", "effect:hearty:300"]));
        assert_eq!(given, [(hearty, 300.0)], "the longest of a rule and its own tag");
        let calm = Weather::NONE;
        let active = effects.active(&situation(&calm, false, 2.0, true), &given);
        assert_eq!(active[0], ActiveEffect { effect: effects.id("core:hearty").unwrap(), seconds_left: Some(300) });
        assert_eq!(active[1].effect, effects.id("core:chilly").unwrap());
        assert_eq!(effects.get(active[0].effect).icon, "meat");
        assert_eq!(effects.get(active[1].effect).icon, "chilly");
    }

    #[test]
    fn modifiers_combine() {
        let effects = registry();
        let calm = Weather::NONE;
        let given = [(effects.id("core:hearty").unwrap(), 10.0)];
        let m = effects.modifiers(&effects.active(&situation(&calm, false, -10.0, true), &given));
        assert_eq!((m.heal, m.hurt_every, m.max_health), (0.0, Some(10.0), 4));
        let m = effects.modifiers(&effects.active(&situation(&calm, true, 2.0, true), &[]));
        assert_eq!((m.heal, m.speed), (1.0, 0.9));
    }

    const HOME: &str = r#"(effects: [
        (name: "camp", kind: Buff, title: "Camp", description: "", announce: true, when: Some(All([AtHome, HomeHas("bed", 1)])), modifiers: (heal: 1.2)),
        (name: "snug", kind: Buff, title: "Snug", description: "", announce: true, when: Some(All([AtHome, Indoors, HomeHas("bed", 1), HomeHas("seat", 2)])), replaces: ["camp"], modifiers: (heal: 1.5)),
        (name: "rested", kind: Buff, title: "Rested", description: "", modifiers: (xp: 1.05)),
        (name: "well_rested", kind: Buff, title: "Well rested", description: "", replaces: ["rested"], modifiers: (xp: 1.1)),
    ], sleeps: [
        (effect: "rested", seconds: 600.0),
        (effect: "well_rested", seconds: 900.0, needs: "snug"),
    ])"#;

    fn home_registry() -> EffectRegistry {
        EffectRegistry::new(vec![EffectRegistry::read("core", HOME).unwrap()]).unwrap()
    }

    /// Losing a block a home step counts names the highest step it then falls short of,
    /// among those the home met; losing one more than any step needs, or one no step
    /// counts, names none. On return, the steps once met and no longer are found the same way.
    #[test]
    fn a_home_falling_short_names_the_highest_step_lost() {
        let effects = home_registry();
        let (camp, snug) = (effects.id("core:camp").unwrap(), effects.id("core:snug").unwrap());
        let survey = |bed: u32, seat: u32| HomeSurvey { counts: vec![("bed".into(), bed), ("seat".into(), seat)], ..Default::default() };
        assert_eq!(effects.falls_short(&["seat"], &survey(1, 2), &survey(1, 1)), Some((snug, "seat".into(), 2)));
        assert_eq!(effects.falls_short(&["bed"], &survey(1, 2), &survey(0, 2)), Some((snug, "bed".into(), 1)), "camp and snug both want it: the higher is named");
        assert_eq!(effects.falls_short(&["bed", "seat"], &survey(1, 0), &survey(0, 0)), Some((camp, "bed".into(), 1)), "snug was never met");
        assert_eq!(effects.falls_short(&["seat"], &survey(1, 4), &survey(1, 3)), None, "still enough");
        assert_eq!(effects.falls_short(&["rug"], &survey(1, 2), &survey(0, 2)), None, "lost for want of what is not asked about");
        assert_eq!(effects.home_steps(&survey(1, 2)), vec![camp, snug]);
        assert_eq!(effects.lost(&[camp, snug], &["seat"], &survey(1, 0)), Some((snug, "seat".into(), 2)));
        assert_eq!(effects.lost(&[camp], &["seat"], &survey(1, 0)), None, "snug was not had");
    }

    const KNOW_HOW: &str = r#"(effects: [
        (name: "steady", kind: Buff, title: "Steady", description: "", announce: true, ladder: "knack", when: Some(Done("full_kit")), modifiers: (mining: 1.1)),
        (name: "practised", kind: Buff, title: "Practised", description: "", when: Some(All([Done("full_kit"), Done("base:copper_kit")])), replaces: ["steady"], modifiers: (mining: 1.2, finds: 0.04)),
        (name: "mountain_craft", kind: Buff, title: "Mountain craft", description: "", when: Some(All([Done("full_kit"), InBiome("mountain")])), modifiers: (finds: 0.03)),
    ])"#;

    #[test]
    fn what_a_quest_taught_stays_as_an_effect_and_combines_with_where_you_are() {
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", KNOW_HOW).unwrap()]).unwrap();
        assert_eq!(effects.quests(), ["core:full_kit", "base:copper_kit"], "each quest once, qualified by the package unless it names one");
        assert_eq!(effects.steps(), ["core:knack_steady"], "an announced step is named by its ladder, and only an announced one");
        assert_eq!(effects.step("core:knack_steady").map(|d| d.title.as_str()), Some("Steady"));
        assert!(effects.step("core:knack_practised").is_none() && effects.step("core:home_steady").is_none() && effects.step("steady").is_none());
        let calm = Weather::NONE;
        let away = situation(&calm, false, 15.0, true);
        let names = |situation: &Situation| effects.active(situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).collect::<Vec<_>>();
        assert!(names(&away).is_empty(), "nothing is known before the quest");
        let kit = ["core:full_kit".to_owned()];
        assert_eq!(names(&Situation { done: &kit, ..away }), ["core:steady"]);
        let both = ["core:full_kit".to_owned(), "base:copper_kit".to_owned()];
        assert_eq!(names(&Situation { done: &both, ..away }), ["core:practised"], "the better step hides the milder");
        let tags = ["mountain".to_owned()];
        let high = Situation { biome: "base:crags", biome_tags: &tags, done: &kit, ..away };
        assert_eq!(names(&high), ["core:steady", "core:mountain_craft"], "what was learnt pairs with where it is used");
        let active = effects.active(&Situation { done: &both, ..high }, &[]);
        let modifiers = effects.modifiers(&active);
        assert_eq!((modifiers.mining, modifiers.finds), (1.2, 0.07), "speeds multiply and extra finds add");
    }

    fn survey(counts: &[(&str, u32)]) -> HomeSurvey {
        HomeSurvey { counts: counts.iter().map(|&(t, n)| (t.to_owned(), n)).collect(), ..Default::default() }
    }

    #[test]
    fn a_home_asks_for_its_furnishings_and_a_roof() {
        let effects = home_registry();
        assert_eq!(effects.home_tags(), ["bed", "seat"], "each tag once, in the order effects ask");
        let calm = Weather::NONE;
        let names = |situation: &Situation| effects.active(situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).collect::<Vec<_>>();
        let away = situation(&calm, false, 15.0, false);
        assert!(names(&away).is_empty(), "nothing away from home");
        let bare = survey(&[("bed", 0), ("seat", 0)]);
        assert!(names(&Situation { home: Some(&bare), ..away }).is_empty(), "home without a bed is just a place");
        let camp = survey(&[("bed", 1), ("seat", 1)]);
        assert_eq!(names(&Situation { home: Some(&camp), ..away }), ["core:camp"]);
        let snug = survey(&[("bed", 1), ("seat", 2)]);
        assert_eq!(names(&Situation { home: Some(&snug), ..away }), ["core:snug"], "snug replaces camp");
        let roofless = situation(&calm, false, 15.0, true);
        assert_eq!(names(&Situation { home: Some(&snug), ..roofless }), ["core:camp"], "no roof, no snug");
    }

    #[test]
    fn sleep_gives_the_rest_the_home_allows_once() {
        let effects = home_registry();
        let calm = Weather::NONE;
        let given = |situation: &Situation| effects.given_by_sleep(situation, &[]).iter().map(|&(id, s)| (effects.get(id).name.clone(), s)).collect::<Vec<_>>();
        assert_eq!(given(&situation(&calm, false, 15.0, true)), [("core:rested".to_owned(), 600.0)], "a bed in the open is a plain rest");
        let snug = survey(&[("bed", 1), ("seat", 2)]);
        let indoors = Situation { home: Some(&snug), ..situation(&calm, false, 15.0, false) };
        assert_eq!(given(&indoors), [("core:rested".to_owned(), 600.0), ("core:well_rested".to_owned(), 900.0)]);
        let rested = effects.id("core:rested").unwrap();
        let given_there = effects.given_by_sleep(&indoors, &[]);
        assert!(effects.rested(&[(rested, 5.0)], &[(rested, 600.0)]), "a second nap rests no more");
        assert!(!effects.rested(&[(rested, 5.0)], &given_there), "a nap does not keep a better rest away");
        assert!(effects.rested(&[(effects.id("core:well_rested").unwrap(), 5.0)], &given_there));
        assert!(!effects.rested(&[], &given_there));
        let active = effects.active(&indoors, &[(rested, 5.0), (effects.id("core:well_rested").unwrap(), 5.0)]);
        assert!(active.iter().all(|a| effects.get(a.effect).name != "core:rested"), "the better rest hides the plain one");
    }

    #[test]
    fn a_sleep_rule_with_a_bed_tag_holds_only_in_such_a_bed() {
        let source = r#"(effects: [(name: "soft", kind: Buff, title: "Soft", description: "")], sleeps: [(effect: "soft", seconds: 5.0, bed: "feather")])"#;
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).unwrap();
        let calm = Weather::NONE;
        let open = situation(&calm, false, 15.0, true);
        assert!(effects.given_by_sleep(&open, &["bed".to_owned()]).is_empty(), "a bag is no feather bed");
        assert_eq!(effects.given_by_sleep(&open, &["bed".to_owned(), "feather".to_owned()]).len(), 1);
        assert_eq!(effects.sleep_beds().collect::<Vec<_>>(), ["feather"]);
    }

    #[test]
    fn a_night_sleep_rule_holds_only_after_dark() {
        let source = r#"(effects: [(name: "nap", kind: Buff, title: "Nap", description: ""), (name: "deep", kind: Buff, title: "Deep", description: "")], sleeps: [(effect: "nap", seconds: 5.0), (effect: "deep", seconds: 9.0, night: true)])"#;
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).unwrap();
        let calm = Weather::NONE;
        let names = |s: &Situation| effects.given_by_sleep(s, &[]).iter().map(|(id, _)| effects.get(*id).name.clone()).collect::<Vec<_>>();
        let noon = situation(&calm, false, 15.0, false);
        assert_eq!(names(&noon), ["core:nap"], "a nap by day is no night's rest");
        assert_eq!(names(&Situation { night: true, ..noon }), ["core:nap", "core:deep"]);
        let dark = r#"(effects: [(name: "dark", kind: Buff, title: "Dark", description: "", when: Some(All([Night, Indoors])))])"#;
        let dark = EffectRegistry::new(vec![EffectRegistry::read("core", dark).unwrap()]).unwrap();
        assert!(dark.active(&noon, &[]).is_empty());
        assert_eq!(dark.active(&Situation { night: true, ..noon }, &[]).len(), 1);
        assert!(dark.active(&Situation { night: true, ..situation(&calm, false, 15.0, true) }, &[]).is_empty(), "out of doors");
    }

    /// Home again after long enough away, where the needed effect holds: not after a
    /// short step out, and not where the home lacks what it needs. Broken rules are refused.
    #[test]
    fn coming_home_after_long_away_gives_its_effect() {
        let source = r#"(effects: [(name: "roofed", kind: Buff, title: "Roofed", description: "", when: Some(Indoors)), (name: "again", kind: Buff, title: "Again", description: "")], homecomings: [(effect: "again", away: 600.0, seconds: 300.0, needs: "roofed")])"#;
        let effects = EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).unwrap();
        let calm = Weather::NONE;
        let (indoors, outdoors) = (situation(&calm, false, 15.0, false), situation(&calm, false, 15.0, true));
        let again = effects.id("core:again").unwrap();
        assert_eq!(effects.given_by_homecoming(&indoors, 600.0), [(again, 300.0)]);
        assert!(effects.given_by_homecoming(&indoors, 599.0).is_empty(), "a short step out is no homecoming");
        assert!(effects.given_by_homecoming(&outdoors, 900.0).is_empty(), "home without what it needs");
        let read = |source: &str| EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).map(|_| ()).unwrap_err().0;
        let own_when = r#"(effects: [(name: "x", kind: Buff, title: "X", description: "", when: Some(Indoors))], homecomings: [(effect: "x", away: 1.0, seconds: 1.0)])"#;
        assert!(read(own_when).contains("its own `when`"));
        let zero = r#"(effects: [(name: "x", kind: Buff, title: "X", description: "")], homecomings: [(effect: "x", away: 0.0, seconds: 1.0)])"#;
        assert!(read(zero).contains("both above 0"));
        let no_when = r#"(effects: [(name: "x", kind: Buff, title: "X", description: "")], homecomings: [(effect: "x", away: 1.0, seconds: 1.0, needs: "x")])"#;
        assert!(read(no_when).contains("no `when`"));
    }

    #[test]
    fn broken_sleep_rules_are_refused() {
        let read = |source: &str| EffectRegistry::new(vec![EffectRegistry::read("core", source).unwrap()]).map(|_| ()).unwrap_err().0;
        let day = r#"(effects: [(name: "x", kind: Buff, title: "X", description: "")], sleeps: [(effect: "y", seconds: 1.0)])"#;
        assert!(read(day).contains("gives `core:y`"));
        let no_when = r#"(effects: [(name: "x", kind: Buff, title: "X", description: "")], sleeps: [(effect: "x", seconds: 1.0, needs: "x")])"#;
        assert!(read(no_when).contains("no `when`"));
        let zero = r#"(effects: [(name: "x", kind: Buff, title: "X", description: "")], sleeps: [(effect: "x", seconds: 0.0)])"#;
        assert!(read(zero).contains("0 seconds"));
    }

    #[test]
    fn a_home_says_what_the_next_step_lacks() {
        let effects = home_registry();
        let calm = Weather::NONE;
        let have = survey(&[("bed", 1), ("seat", 1)]);
        let outdoors = Situation { home: Some(&have), ..situation(&calm, false, 15.0, true) };
        let progress = effects.home_progress(&outdoors);
        // `camp` and `snug` are one chain; nothing else is announced and about a home.
        assert_eq!(progress.len(), 1);
        let camp = effects.id("core:camp").unwrap();
        let snug = effects.id("core:snug").unwrap();
        assert_eq!(progress[0].reached, Some(camp));
        assert_eq!(
            progress[0].next,
            Some((snug, vec![Lack::Roof, Lack::Tag { tag: "seat".into(), have: 1, need: 2 }])),
            "a roof and a second seat"
        );
        let done = survey(&[("bed", 1), ("seat", 2)]);
        let indoors = Situation { home: Some(&done), ..situation(&calm, false, 15.0, false) };
        assert_eq!(effects.home_progress(&indoors)[0], HomeProgress { reached: Some(snug), next: None });
        let away = effects.home_progress(&situation(&calm, false, 15.0, false));
        assert_eq!(away[0].reached, None);
        assert_eq!(away[0].next.as_ref().map(|(id, lacks)| (*id, lacks.first().cloned())), Some((camp, Some(Lack::Away))));
    }

    #[test]
    fn a_choice_of_blocks_is_one_lack() {
        let any = Condition::Any(vec![Condition::HomeHas("hearth".into(), 1), Condition::HomeHas("heat".into(), 1)]);
        let calm = Weather::NONE;
        let none = survey(&[]);
        let s = Situation { home: Some(&none), ..situation(&calm, false, 15.0, true) };
        assert_eq!(
            any.lacks(&s),
            [Lack::OneOf(vec![vec![Lack::Tag { tag: "hearth".into(), have: 0, need: 1 }], vec![Lack::Tag { tag: "heat".into(), have: 0, need: 1 }]])]
        );
        let warm = survey(&[("heat", 1)]);
        assert!(any.lacks(&Situation { home: Some(&warm), ..s }).is_empty());
    }

    #[test]
    fn the_weather_outside_counts_from_indoors_only() {
        let storm = Weather { precipitation: 0.9, ..Weather::NONE };
        let fair = Weather::NONE;
        let cond = Condition::Sheltered(Box::new(Condition::Rain(0.3)));
        assert!(cond.holds(&situation(&storm, false, 15.0, false)), "it rains and they are under a roof");
        assert!(!cond.holds(&situation(&storm, false, 15.0, true)), "out in it they are not sheltered, they are soaked");
        assert!(!cond.holds(&situation(&fair, false, 15.0, false)), "a roof in fair weather shelters from nothing");
    }
}
