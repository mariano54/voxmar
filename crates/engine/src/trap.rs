//! Traps, as content (`traps.ron`, docs/traps.md): blocks a player sets where small
//! animals run, and comes back to. Each is two blocks: `set` (waiting) and `sprung` (it
//! caught something). A creature of a kind the trap `catches` that steps into a set trap's
//! cell is caught there: it is gone from the world, the trap shows sprung, and what it was
//! is kept with the trap. A right click on a sprung trap takes the catch, as the creature's
//! loot rolled for the player who takes it (their Hunter level, as a kill), with the
//! species' kill experience; the trap is set again. Taking asks the trap's `skill` at
//! `level`.
//!
//! What a trap costs and how sure it is (Oct 15, docs/traps.md "Snares as a hunt"): a
//! creature it catches is held by `chance` (or its own in `chances`), else it slips free,
//! frightened; one too big for it (`holds`, its health) or that fights tears it by `tear`;
//! each catch taken wears it out by `wear`; torn or worn out it is gone, leaving `leaves`.
//! A catch gives `xp` of the kill's experience, and none is caught within `shy_reach` of a
//! block tagged one of `shy_of` (game keeps off fire). A better hunter does better by
//! `skilled`: for each level of the trap's skill over `level` (up to `skilled.levels`), the
//! chance a catch is held rises by `skilled.chance`, and the chances of tearing and wearing
//! out fall by `skilled.tear` and `skilled.wear`: the setter's level when it was set, the
//! taker's when a catch is taken.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::creature::{CreatureId, CreatureRegistry};
use crate::skill::{SkillId, SkillRegistry};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrapDef {
    pub set: String,
    pub sprung: String,
    /// The species it catches.
    pub catches: Vec<String>,
    /// The skill asked to take a catch, at `level`.
    pub skill: String,
    #[serde(default = "one")]
    pub level: u8,
    /// The chance a creature it catches is held when it steps in; else it slips free.
    #[serde(default = "always")]
    pub chance: f32,
    /// Chances of its own for some of the species it catches, by name.
    #[serde(default)]
    pub chances: std::collections::BTreeMap<String, f32>,
    /// The most health a creature it holds may have: a wild one bigger, or one that fights,
    /// that steps in tears it by `tear`.
    #[serde(default = "most")]
    pub holds: u16,
    #[serde(default)]
    pub tear: f32,
    /// The chance taking a catch wears it out.
    #[serde(default)]
    pub wear: f32,
    /// What is left where it is torn or wears out, an item of each.
    #[serde(default)]
    pub leaves: Vec<String>,
    /// The share of a kill's experience a catch gives.
    #[serde(default = "always")]
    pub xp: f32,
    /// Block tags game keeps off: within `shy_reach` of one, nothing is caught.
    #[serde(default)]
    pub shy_of: Vec<String>,
    #[serde(default)]
    pub shy_reach: f32,
    /// How much better it does for a better hunter (`Skilled`).
    #[serde(default)]
    pub skilled: Skilled,
}

/// How a trap does better for each level of its skill over its own: the chance a catch is
/// held rises by `chance`, and the chances of tearing and of wearing out fall by `tear` and
/// `wear`, for up to `levels` levels.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Skilled {
    #[serde(default)]
    pub chance: f32,
    #[serde(default)]
    pub tear: f32,
    #[serde(default)]
    pub wear: f32,
    #[serde(default)]
    pub levels: u8,
}

fn one() -> u8 {
    1
}

fn always() -> f32 {
    1.0
}

fn most() -> u16 {
    u16::MAX
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrapFile {
    #[serde(default)]
    pub traps: Vec<TrapDef>,
}

/// A trap, resolved: `catches` is sorted, with each species' chance beside it.
#[derive(Clone, Debug)]
pub struct Trap {
    pub set: BlockId,
    pub sprung: BlockId,
    pub catches: Vec<CreatureId>,
    pub chances: Vec<f32>,
    pub skill: SkillId,
    pub level: u8,
    pub holds: u16,
    pub tear: f32,
    pub wear: f32,
    pub leaves: Vec<BlockId>,
    pub xp: f32,
    pub shy_of: Vec<String>,
    pub shy_reach: f32,
    pub skilled: Skilled,
}

impl Trap {
    /// The levels of its skill over its own that count at `level`, at most `skilled.levels`.
    fn over(&self, level: u8) -> f32 {
        level.saturating_sub(self.level).min(self.skilled.levels) as f32
    }

    /// The chance `species` is held, set by a hunter of `level`: 0 for one it does not
    /// catch.
    pub fn chance_at(&self, species: CreatureId, level: u8) -> f32 {
        if !self.catches(species) {
            return 0.0;
        }
        (self.chance(species) + self.skilled.chance * self.over(level)).min(1.0)
    }

    /// The chance it is torn, set by a hunter of `level`.
    pub fn tear_at(&self, level: u8) -> f32 {
        (self.tear - self.skilled.tear * self.over(level)).max(0.0)
    }

    /// The chance taking a catch wears it out, taken by a hunter of `level`.
    pub fn wear_at(&self, level: u8) -> f32 {
        (self.wear - self.skilled.wear * self.over(level)).max(0.0)
    }

    pub fn catches(&self, species: CreatureId) -> bool {
        self.catches.binary_search(&species).is_ok()
    }

    /// The chance `species` is held when it steps in: 0 for one it does not catch.
    pub fn chance(&self, species: CreatureId) -> f32 {
        self.catches.binary_search(&species).map_or(0.0, |i| self.chances[i])
    }

    /// Whether a wild creature of `species` tears it when it steps in, by `tear`: one too
    /// big for it, or one that fights.
    pub fn torn_by(&self, species: &crate::creature::Creature) -> bool {
        self.tear > 0.0 && (species.def.health > self.holds || species.fights())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Traps {
    pub traps: Vec<Trap>,
}

impl Traps {
    pub fn read(namespace: &str, source: &str) -> Result<TrapFile, RegistryError> {
        let mut file: TrapFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/traps.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        for trap in &mut file.traps {
            qualify(&mut trap.set);
            qualify(&mut trap.sprung);
            qualify(&mut trap.skill);
            trap.catches.iter_mut().for_each(qualify);
            trap.leaves.iter_mut().for_each(qualify);
            trap.chances = std::mem::take(&mut trap.chances)
                .into_iter()
                .map(|(mut name, n)| {
                    qualify(&mut name);
                    (name, n)
                })
                .collect();
        }
        Ok(file)
    }

    /// Every package's traps, checked: each block belongs to one trap, each species and
    /// skill is known, and the level is one a player can have.
    pub fn new(files: Vec<TrapFile>, blocks: &BlockRegistry, creatures: &CreatureRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let refuse = |what: String| RegistryError(format!("traps.ron: {what}"));
        let block = |name: &str| blocks.id(name).ok_or_else(|| refuse(format!("no block `{name}`")));
        let mut traps = Traps::default();
        for def in files.into_iter().flat_map(|f| f.traps) {
            let (set, sprung) = (block(&def.set)?, block(&def.sprung)?);
            if set == sprung || traps.traps.iter().any(|t| [t.set, t.sprung].iter().any(|b| *b == set || *b == sprung)) {
                return Err(refuse(format!("{}: its set and sprung blocks differ, and belong to no other trap", def.set)));
            }
            if blocks.info(set).solid || blocks.info(sprung).solid {
                return Err(refuse(format!("{}: a trap is stepped into, so neither block is solid", def.set)));
            }
            let mut catches = Vec::new();
            for name in &def.catches {
                catches.push(creatures.id(name).ok_or_else(|| refuse(format!("{}: no creature `{name}`", def.set)))?);
            }
            if catches.is_empty() {
                return Err(refuse(format!("{}: a trap catches some creature", def.set)));
            }
            catches.sort();
            catches.dedup();
            let skill = skills.id(&def.skill).ok_or_else(|| refuse(format!("{}: no skill `{}`", def.set, def.skill)))?;
            if def.level == 0 || def.level > skills.max_level() {
                return Err(refuse(format!("{}: level {} is not one a player can have", def.set, def.level)));
            }
            let share = |name: &str, n: f32| if n.is_finite() && n > 0.0 && n <= 1.0 { Ok(n) } else { Err(refuse(format!("{}: {name} {n} is not a share over 0 and at most 1", def.set))) };
            let chance = share("chance", def.chance)?;
            for (name, &n) in &def.chances {
                let species = creatures.id(name).ok_or_else(|| refuse(format!("{}: a chance for no creature `{name}`", def.set)))?;
                if !catches.contains(&species) {
                    return Err(refuse(format!("{}: a chance for `{name}`, which it does not catch", def.set)));
                }
                share(&format!("{name}'s chance"), n)?;
            }
            let chances: Vec<f32> = catches.iter().map(|&c| def.chances.get(&creatures.get(c).def.name).copied().unwrap_or(chance)).collect();
            // What it catches it holds: none too big for it, none that fights.
            for &c in &catches {
                let species = creatures.get(c);
                if species.def.health > def.holds || species.fights() {
                    return Err(refuse(format!("{}: it catches `{}`, which is too big for it to hold ({} health over {}) or fights", def.set, species.def.name, species.def.health, def.holds)));
                }
            }
            for (name, n) in [("tear", def.tear), ("wear", def.wear)] {
                if !(n.is_finite() && (0.0..=1.0).contains(&n)) {
                    return Err(refuse(format!("{}: {name} {n} is not a chance from 0 to 1", def.set)));
                }
            }
            let xp = share("xp", def.xp)?;
            let mut leaves = Vec::new();
            for name in &def.leaves {
                leaves.push(block(name)?);
            }
            if !(def.shy_reach.is_finite() && (0.0..=16.0).contains(&def.shy_reach)) || (def.shy_reach > 0.0) == def.shy_of.is_empty() {
                return Err(refuse(format!("{}: shy_reach is 0 to 16 blocks, with tags in shy_of exactly when it is over 0", def.set)));
            }
            if let Some(tag) = def.shy_of.iter().find(|tag| blocks.tagged(tag).next().is_none()) {
                return Err(refuse(format!("{}: shy of `{tag}`, which no block is tagged", def.set)));
            }
            // A better hunter: steps of at most 0.05 a level, over levels a player can have,
            // never taking a chance past 1 or below 0.
            let skilled = def.skilled;
            let steps = [("chance", skilled.chance), ("tear", skilled.tear), ("wear", skilled.wear)];
            if let Some((name, n)) = steps.iter().find(|(_, n)| !(n.is_finite() && (0.0..=0.05).contains(n))) {
                return Err(refuse(format!("{}: skilled {name} {n} is not a step from 0 to 0.05 a level", def.set)));
            }
            let levels = skilled.levels as f32;
            if skilled.levels > skills.max_level() - def.level {
                return Err(refuse(format!("{}: skilled over {} levels, past the most a player can have", def.set, skilled.levels)));
            }
            let best = chances.iter().copied().fold(0.0_f32, f32::max);
            if best + skilled.chance * levels > 1.0 + 1e-4 || def.tear - skilled.tear * levels < -1e-4 || def.wear - skilled.wear * levels < -1e-4 {
                return Err(refuse(format!("{}: over {} levels its skilled steps take a chance past 1 or below 0", def.set, skilled.levels)));
            }
            traps.traps.push(Trap { set, sprung, catches, chances, skill, level: def.level, holds: def.holds, tear: def.tear, wear: def.wear, leaves, xp, shy_of: def.shy_of, shy_reach: def.shy_reach, skilled });
        }
        Ok(traps)
    }

    /// The trap whose set block is `block`.
    pub fn set(&self, block: BlockId) -> Option<&Trap> {
        self.traps.iter().find(|t| t.set == block)
    }

    /// The trap whose sprung block is `block`.
    pub fn sprung(&self, block: BlockId) -> Option<&Trap> {
        self.traps.iter().find(|t| t.sprung == block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> crate::content::Content {
        crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads")
    }

    fn load(source: &str) -> Result<Traps, RegistryError> {
        let content = base();
        let file = Traps::read("base", source)?;
        Traps::new(vec![file], &content.blocks, &content.creatures, &content.skills)
    }

    /// Base's snare: set and sprung, catching rabbits and the like, taken at Hunter 1.
    #[test]
    fn a_snare_catches_small_game() {
        let content = base();
        let snare = content.traps.set(content.blocks.id("base:snare").unwrap()).expect("the snare is a trap");
        assert_eq!(content.traps.sprung(snare.sprung).map(|t| t.set), Some(snare.set));
        assert!(snare.catches(content.creatures.id("base:rabbit").unwrap()));
        assert!(!snare.catches(content.creatures.id("base:wolf").unwrap()), "no wolf in a snare");
        assert_eq!((content.skills.get(snare.skill).name.as_str(), snare.level), ("core:hunter", 1));
        // Not sure, and spent: a rabbit slips it some of the time, a deer or a fox tears it,
        // taking wears it out, and a catch is worth less than the kill.
        let [rabbit, deer, fox] = ["base:rabbit", "base:red_deer", "base:fox"].map(|n| content.creatures.id(n).unwrap());
        assert!((0.0..1.0).contains(&snare.chance(rabbit)) && snare.chance(rabbit) > 0.0, "a rabbit is held {}", snare.chance(rabbit));
        assert_eq!(snare.chance(deer), 0.0, "no deer is held");
        assert!(snare.torn_by(content.creatures.get(deer)) && snare.torn_by(content.creatures.get(fox)), "a deer and a fox tear a snare");
        assert!(!snare.torn_by(content.creatures.get(rabbit)), "a rabbit does not");
        assert!(snare.wear > 0.0 && snare.xp < 1.0 && !snare.leaves.is_empty() && snare.shy_reach > 0.0);
        // A better hunter catches more, and tears and wears out fewer, up to its levels.
        let (low, high) = (snare.level, snare.level + snare.skilled.levels);
        assert!(snare.chance_at(rabbit, high) > snare.chance_at(rabbit, low) && snare.chance_at(rabbit, low) == snare.chance(rabbit));
        assert!(snare.tear_at(high) < snare.tear_at(low) && snare.wear_at(high) < snare.wear_at(low));
        assert_eq!(snare.chance_at(rabbit, high), snare.chance_at(rabbit, 99), "no better past its levels");
        assert_eq!(snare.chance_at(deer, high), 0.0, "still no deer");
    }

    #[test]
    fn bad_traps_are_refused() {
        assert!(load(r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter")])"#).is_ok());
        assert!(load(r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit", "hare"], skill: "core:hunter", chance: 0.6, chances: {"hare": 0.5}, holds: 6, tear: 0.6, wear: 0.2, leaves: ["stick"], xp: 0.5, shy_of: ["fire"], shy_reach: 6.0, skilled: (chance: 0.008, tear: 0.01, wear: 0.004, levels: 30))])"#).is_ok());
        for bad in [
            r#"(traps: [(set: "snare", sprung: "snare", catches: ["rabbit"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: [], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["dragon"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", level: 0)])"#,
            r#"(traps: [(set: "stone", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:juggling")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", chance: 0.0)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", chance: 1.5)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", chances: {"hare": 0.5})])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", chances: {"rabbit": -0.1})])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit", "wolf"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", holds: 2)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", tear: 1.2)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", wear: -0.5)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", leaves: ["moonbeam"])])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", xp: 0.0)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", shy_reach: 4.0)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", shy_of: ["fire"])])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", shy_of: ["fire"], shy_reach: 40.0)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", shy_of: ["no_such_tag"], shy_reach: 6.0)])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", chance: 0.5, skilled: (chance: 0.2, levels: 2))])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", chance: 0.5, skilled: (chance: 0.05, levels: 20))])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", tear: 0.2, skilled: (tear: 0.01, levels: 30))])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", wear: 0.2, skilled: (wear: -0.01, levels: 5))])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", skilled: (levels: 250))])"#,
        ] {
            assert!(load(bad).is_err(), "{bad}");
        }
    }
}
