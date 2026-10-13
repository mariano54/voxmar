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
//! tag around it (`HomeHas("bed", 1)`). A ladder of such effects, each replacing the one
//! below, is a home that gets cosier as it is furnished (base/effects.ron). Sleeping at a
//! bed gives the effects the file's `sleeps` rules name, those whose `needs` effect would
//! hold where the player sleeps: a better rest for a better home.
//!
//! Every effect has an icon (`ui/effect/<icon>` in the packs) and its name and a line on
//! what it does as language keys (`effect.<package>.<name>` and `….description`).

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::RegistryError;
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
    /// Whether the player is told (in chat) when it comes on, with what it does: for the
    /// steps of a ladder they work toward.
    #[serde(default)]
    pub announce: bool,
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
}

impl Default for Modifiers {
    fn default() -> Self {
        Self { heal: 1.0, hurt_every: None, warmth: 0.0, max_health: 0, max_health_share: 1.0, speed: 1.0, mining: 1.0, xp: 1.0, fishing: 1.0 }
    }
}

impl Modifiers {
    /// Several effects' modifiers together.
    pub fn combine<'a>(all: impl IntoIterator<Item = &'a Modifiers>) -> Modifiers {
        all.into_iter().fold(Modifiers::default(), |sum, m| Modifiers {
            heal: sum.heal * m.heal,
            hurt_every: match (sum.hurt_every, m.hurt_every) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
            warmth: sum.warmth + m.warmth,
            max_health: sum.max_health.saturating_add(m.max_health),
            max_health_share: sum.max_health_share * m.max_health_share,
            speed: sum.speed * m.speed,
            mining: sum.mining * m.mining,
            xp: sum.xp * m.xp,
            fishing: sum.fishing * m.fishing,
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
    /// At home, with at least this many blocks carrying the tag around it.
    HomeHas(String, u32),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

impl Condition {
    /// The tags `HomeHas` asks of a home anywhere in this condition.
    fn home_tags<'a>(&'a self, into: &mut Vec<&'a str>) {
        match self {
            Condition::HomeHas(tag, _) => into.push(tag),
            Condition::All(list) | Condition::Any(list) => list.iter().for_each(|c| c.home_tags(into)),
            Condition::Not(c) => c.home_tags(into),
            _ => {}
        }
    }
}

/// What a home condition still lacks, as `/home` tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lack {
    /// A roof overhead.
    Roof,
    /// Standing at home (the home survey is none).
    Away,
    /// Blocks with a tag: how many there are and how many are wanted.
    Tag { tag: String, have: u32, need: u32 },
    /// Any one of these will do.
    OneOf(Vec<Vec<Lack>>),
}

impl Condition {
    /// Whether the condition is about a home (`AtHome` or `HomeHas` anywhere in it).
    fn about_home(&self) -> bool {
        match self {
            Condition::AtHome | Condition::HomeHas(..) => true,
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
            Condition::HomeHas(tag, need) => vec![Lack::Tag { tag: tag.clone(), have: s.home.map_or(0, |h| h.count(tag)), need: *need }],
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
}

/// How many blocks with each tag lie around a home: the tags are those effects ask of it
/// (`EffectRegistry::home_tags`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeSurvey {
    pub counts: Vec<(String, u32)>,
}

impl HomeSurvey {
    pub fn count(&self, tag: &str) -> u32 {
        self.counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n)
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
            Condition::HomeHas(tag, n) => s.home.is_some_and(|h| h.count(tag) >= *n),
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
/// there is one) would hold where the player sleeps.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SleepRule {
    pub effect: String,
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
}

/// Every effect, in the order packages list them.
#[derive(Debug, Default)]
pub struct EffectRegistry {
    defs: Vec<EffectDef>,
    by_name: FxHashMap<String, EffectId>,
    /// The effects each one hides.
    replaces: Vec<Vec<EffectId>>,
    foods: Vec<(Vec<String>, EffectId, f32)>,
    /// What sleeping gives: the effect, its seconds, and the effect whose condition must hold.
    sleeps: Vec<(EffectId, f32, Option<EffectId>)>,
    /// The tags effects ask of a home, each once.
    home_tags: Vec<String>,
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
        Ok(file)
    }

    pub fn new(files: Vec<EffectFile>) -> Result<Self, RegistryError> {
        let (mut defs, mut rules, mut sleeps) = (Vec::new(), Vec::new(), Vec::new());
        for file in files {
            defs.extend(file.effects);
            rules.extend(file.foods);
            sleeps.extend(file.sleeps);
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
                Ok((id, rule.seconds, needs))
            })
            .collect::<Result<_, RegistryError>>()?;
        let mut tags: Vec<&str> = Vec::new();
        for def in &defs {
            if let Some(when) = &def.when {
                when.home_tags(&mut tags);
            }
        }
        let mut home_tags: Vec<String> = Vec::new();
        for tag in tags {
            if !home_tags.iter().any(|t| t == tag) {
                home_tags.push(tag.to_owned());
            }
        }
        Ok(Self { defs, by_name, replaces, foods, sleeps, home_tags })
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

    /// Whether effect `winner` hides `loser` while it lasts.
    pub fn hides(&self, winner: EffectId, loser: EffectId) -> bool {
        self.replaces[winner.0 as usize].contains(&loser)
    }

    /// Where each chain of home steps stands at `situation`: the announced effects about a
    /// home, grouped by what replaces what, ordered by how much each replaces (the lowest
    /// first), a chain's highest step met and the lowest one it has yet to reach with what
    /// that lacks.
    pub fn home_progress(&self, situation: &Situation) -> Vec<HomeProgress> {
        let steps: Vec<EffectId> = self.ids().filter(|&id| self.get(id).announce && self.get(id).when.as_ref().is_some_and(Condition::about_home)).collect();
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
                let holds = |id: EffectId| self.get(id).when.as_ref().is_some_and(|w| w.holds(situation));
                // The highest step met, and the next one above it: a lower step left unmet
                // (a hearth lit with no torch by the bed) is behind, not next.
                let top = members.iter().rposition(|&id| holds(id));
                let reached = top.map(|i| members[i]);
                let next = members[top.map_or(0, |i| i + 1)..].iter().copied().find(|&id| !holds(id)).map(|id| (id, self.get(id).when.as_ref().map(|w| w.lacks(situation)).unwrap_or_default()));
                HomeProgress { reached, next }
            })
            .collect()
    }

    /// The tags effects ask of a home (`HomeHas`): what surveying a home counts.
    pub fn home_tags(&self) -> &[String] {
        &self.home_tags
    }

    /// Whether the effects a sleep gives are among `active` (the player is rested already).
    pub fn rested(&self, active: &[(EffectId, f32)]) -> bool {
        self.sleeps.iter().any(|(effect, ..)| active.iter().any(|(a, _)| a == effect))
    }

    /// The timed effects sleeping at a bed gives where `situation` is: each sleep rule whose
    /// needed effect would hold there.
    pub fn given_by_sleep(&self, situation: &Situation) -> Vec<(EffectId, f32)> {
        self.sleeps.iter().filter(|(_, _, needs)| needs.is_none_or(|n| self.get(n).when.as_ref().is_some_and(|c| c.holds(situation)))).map(|&(id, seconds, _)| (id, seconds)).collect()
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
        Situation { felt, warmed, outdoors, weather, biome: "base:plains", biome_tags: &[], home: None }
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

    fn survey(counts: &[(&str, u32)]) -> HomeSurvey {
        HomeSurvey { counts: counts.iter().map(|&(t, n)| (t.to_owned(), n)).collect() }
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
        let given = |situation: &Situation| effects.given_by_sleep(situation).iter().map(|&(id, s)| (effects.get(id).name.clone(), s)).collect::<Vec<_>>();
        assert_eq!(given(&situation(&calm, false, 15.0, true)), [("core:rested".to_owned(), 600.0)], "a bed in the open is a plain rest");
        let snug = survey(&[("bed", 1), ("seat", 2)]);
        let indoors = Situation { home: Some(&snug), ..situation(&calm, false, 15.0, false) };
        assert_eq!(given(&indoors), [("core:rested".to_owned(), 600.0), ("core:well_rested".to_owned(), 900.0)]);
        let rested = effects.id("core:rested").unwrap();
        assert!(effects.rested(&[(rested, 5.0)]));
        assert!(!effects.rested(&[]));
        let active = effects.active(&indoors, &[(rested, 5.0), (effects.id("core:well_rested").unwrap(), 5.0)]);
        assert!(active.iter().all(|a| effects.get(a.effect).name != "core:rested"), "the better rest hides the plain one");
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
}
