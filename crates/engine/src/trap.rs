//! Traps, as content (`traps.ron`, docs/traps.md): blocks a player sets where small
//! animals run, and comes back to. Each is two blocks: `set` (waiting) and `sprung` (it
//! caught something). A creature of a kind the trap `catches` that steps into a set trap's
//! cell is caught there: it is gone from the world, the trap shows sprung, and what it was
//! is kept with the trap. A right click on a sprung trap takes the catch, as the creature's
//! loot rolled for the player who takes it (their Hunter level, as a kill), with the
//! species' kill experience; the trap is set again. Taking asks the trap's `skill` at
//! `level`.

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
}

fn one() -> u8 {
    1
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrapFile {
    #[serde(default)]
    pub traps: Vec<TrapDef>,
}

/// A trap, resolved: `catches` is sorted.
#[derive(Clone, Debug)]
pub struct Trap {
    pub set: BlockId,
    pub sprung: BlockId,
    pub catches: Vec<CreatureId>,
    pub skill: SkillId,
    pub level: u8,
}

impl Trap {
    pub fn catches(&self, species: CreatureId) -> bool {
        self.catches.binary_search(&species).is_ok()
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
            traps.traps.push(Trap { set, sprung, catches, skill, level: def.level });
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
    }

    #[test]
    fn bad_traps_are_refused() {
        assert!(load(r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter")])"#).is_ok());
        for bad in [
            r#"(traps: [(set: "snare", sprung: "snare", catches: ["rabbit"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: [], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["dragon"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter", level: 0)])"#,
            r#"(traps: [(set: "stone", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:hunter")])"#,
            r#"(traps: [(set: "snare", sprung: "snare_sprung", catches: ["rabbit"], skill: "core:juggling")])"#,
        ] {
            assert!(load(bad).is_err(), "{bad}");
        }
    }
}
