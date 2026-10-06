//! Skills, RuneScape-style: each player has experience in every skill, which a level curve
//! turns into a level. Actions grant experience and may require a level; levels make
//! actions yield a little more, never the world run faster (see docs/design-philosophy.md,
//! "Agents, skills and factories").
//!
//! Skills are content: each package's `skills.ron` lists its skills, and one package (core)
//! sets the curve, so a mod adds a skill the way it adds a block, and switching from
//! RuneScape's 99 levels to a short ladder of tiers is a change to one file.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::RegistryError;

/// The most effects together multiply experience by: a quarter more (Rested, Focused,
/// house rooms and feats all count toward it).
pub const MAX_XP_BOOST: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SkillId(pub u8);

/// A skill as content defines it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillDef {
    pub name: String,
    /// What players see it called.
    pub title: String,
    /// One line on what it covers.
    pub description: String,
    /// What the skill opens up as it rises, beyond what content says for itself (each
    /// crop's level is listed from crops.ron): shown in the skill's guide.
    #[serde(default)]
    pub guide: Vec<GuideStep>,
    /// The level every player starts at (Hitpoints starts at 10, as in RuneScape).
    #[serde(default = "first_level")]
    pub start: u8,
}

fn first_level() -> u8 {
    1
}

/// A line of a skill's guide: something open from `level` on.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideStep {
    pub level: u8,
    pub text: String,
    /// The item drawn beside it, by name; the skill's own icon when none is given.
    #[serde(default)]
    pub icon: Option<String>,
}

/// How much experience each level takes.
#[derive(Clone, Debug, Deserialize)]
pub enum Curve {
    /// RuneScape's: each level takes about a tenth more than the last, so level 92 is
    /// half way to 99 (13,034,431 experience).
    RuneScape { max: u8 },
    /// The experience each level from 2 up starts at, in order: a short ladder of tiers.
    Table(Vec<u32>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillFile {
    #[serde(default)]
    curve: Option<Curve>,
    #[serde(default)]
    skills: Vec<SkillDef>,
}

/// Every skill, and the experience each level starts at.
#[derive(Debug, Default)]
pub struct SkillRegistry {
    defs: Vec<SkillDef>,
    by_name: FxHashMap<String, SkillId>,
    /// Experience, in tenths, that level `i + 1` starts at; level 1 at 0.
    levels: Vec<u32>,
}

impl SkillRegistry {
    /// Adds a package's skills; unqualified names go into `namespace`.
    pub fn extend(&mut self, namespace: &str, source: &str) -> Result<(), RegistryError> {
        let error = |e: String| RegistryError(format!("{namespace}/skills.ron: {e}"));
        // `curve: RuneScape(max: 99)` as well as `curve: Some(...)`.
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: SkillFile = options.from_str(source).map_err(|e| error(e.to_string()))?;
        if let Some(curve) = file.curve {
            if !self.levels.is_empty() {
                return Err(error("another package already sets the level curve".into()));
            }
            self.levels = levels(&curve).map_err(error)?;
        }
        for mut def in file.skills {
            if !def.name.contains(':') {
                def.name = format!("{namespace}:{}", def.name);
            }
            if self.defs.len() >= u8::MAX as usize {
                return Err(error("too many skills".into()));
            }
            let id = SkillId(self.defs.len() as u8);
            if self.by_name.insert(def.name.clone(), id).is_some() {
                return Err(error(format!("duplicate skill `{}`", def.name)));
            }
            self.defs.push(def);
        }
        Ok(())
    }

    /// Checks that a curve was set if there are skills to level.
    pub fn validate(&self) -> Result<(), RegistryError> {
        if !self.defs.is_empty() && self.levels.is_empty() {
            return Err(RegistryError("no package sets the level curve (skills.ron)".into()));
        }
        Ok(())
    }

    pub fn id(&self, name: &str) -> Option<SkillId> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: SkillId) -> &SkillDef {
        &self.defs[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = SkillId> {
        (0..self.defs.len() as u8).map(SkillId)
    }

    /// The highest level.
    pub fn max_level(&self) -> u8 {
        self.levels.len().max(1) as u8
    }

    /// The level `xp` tenths of experience reach.
    pub fn level(&self, xp: u32) -> u8 {
        self.levels.partition_point(|&start| start <= xp).max(1) as u8
    }

    /// Tenths of experience `level` starts at.
    pub fn xp_for(&self, level: u8) -> u32 {
        self.levels.get((level.max(1) - 1) as usize).copied().unwrap_or(u32::MAX)
    }

    /// Raises any skill below its starting level to it: for new players, and players saved
    /// before a skill started higher.
    pub fn start(&self, skills: &mut Skills) {
        for id in self.ids() {
            let start = self.xp_for(self.get(id).start);
            if skills.xp(id) < start {
                skills.grant(id, start - skills.xp(id));
            }
        }
    }

    /// The level of a skill by name, 1 if there is no such skill.
    pub fn level_of(&self, skills: &Skills, name: &str) -> u8 {
        self.id(name).map_or(1, |id| self.level(skills.xp(id)))
    }

    /// A player's combat level, RuneScape-style (docs/skills.md), from Attack, Defence and
    /// Hitpoints; Hitpoints counts as at least 10, where it starts.
    pub fn combat_level(&self, skills: &Skills) -> u8 {
        let level = |name: &str| self.id(name).map_or(1.0, |id| self.level(skills.xp(id)) as f32);
        let hitpoints = level("core:hitpoints").max(10.0);
        ((level("core:defence") + hitpoints) / 4.0 + level("core:attack") * 0.65) as u8
    }

    /// How far `xp` is through its level, 0..1 (1 at the highest level).
    pub fn progress(&self, xp: u32) -> f32 {
        let level = self.level(xp);
        if level >= self.max_level() {
            return 1.0;
        }
        let (start, next) = (self.xp_for(level), self.xp_for(level + 1));
        (xp - start) as f32 / (next - start).max(1) as f32
    }

    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in self.defs.iter().flat_map(|d| d.name.bytes().chain([0])).chain(self.levels.iter().flat_map(|l| l.to_le_bytes())) {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
        h
    }
}

/// The experience, in tenths, each level starts at.
fn levels(curve: &Curve) -> Result<Vec<u32>, String> {
    match curve {
        Curve::RuneScape { max } => {
            if *max < 2 {
                return Err("a curve needs at least two levels".into());
            }
            let mut total = 0.0f64;
            let mut levels = vec![0u32];
            for level in 1..*max as u32 {
                total += (level as f64 + 300.0 * 2f64.powf(level as f64 / 7.0)).floor();
                levels.push((total / 4.0).floor() as u32 * 10);
            }
            Ok(levels)
        }
        Curve::Table(starts) => {
            if starts.windows(2).any(|w| w[0] >= w[1]) || starts.first() == Some(&0) {
                return Err("each level must start at more experience than the last".into());
            }
            Ok(std::iter::once(0).chain(starts.iter().map(|xp| xp.saturating_mul(10))).collect())
        }
    }
}

/// `core:skills`: a player's experience in every skill, in tenths, by skill id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skills(pub Vec<u32>);

impl Skills {
    pub fn xp(&self, skill: SkillId) -> u32 {
        self.0.get(skill.0 as usize).copied().unwrap_or(0)
    }

    /// Adds `tenths` of experience; returns the new total.
    pub fn grant(&mut self, skill: SkillId, tenths: u32) -> u32 {
        let index = skill.0 as usize;
        if self.0.len() <= index {
            self.0.resize(index + 1, 0);
        }
        self.0[index] = self.0[index].saturating_add(tenths);
        self.0[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(source: &str) -> SkillRegistry {
        let mut registry = SkillRegistry::default();
        registry.extend("core", source).unwrap();
        registry.validate().unwrap();
        registry
    }

    #[test]
    fn the_runescape_curve_matches_its_table() {
        let skills = registry(r#"(curve: RuneScape(max: 99), skills: [(name: "farming", title: "Farming", description: "")])"#);
        // RuneScape's published experience table, in tenths.
        for (level, xp) in [(2, 83), (10, 1_154), (20, 4_470), (50, 101_333), (92, 6_517_253), (99, 13_034_431)] {
            assert_eq!(skills.xp_for(level), xp * 10, "level {level}");
            assert_eq!(skills.level(xp * 10), level);
            assert_eq!(skills.level(xp * 10 - 1), level - 1);
        }
        assert_eq!((skills.level(0), skills.max_level(), skills.level(u32::MAX)), (1, 99, 99));
        assert!((skills.progress(skills.xp_for(10) + (skills.xp_for(11) - skills.xp_for(10)) / 2) - 0.5).abs() < 0.01);
        assert_eq!(skills.id("core:farming"), Some(SkillId(0)));
    }

    #[test]
    fn a_table_makes_a_short_ladder() {
        let skills = registry(r#"(curve: Table([100, 400, 1000]), skills: [])"#);
        assert_eq!(skills.max_level(), 4);
        assert_eq!([0, 999, 1000, 3999, 4000, 10_000, 99_999].map(|xp| skills.level(xp)), [1, 1, 2, 2, 3, 4, 4]);
        let mut bad = SkillRegistry::default();
        assert!(bad.extend("core", "(curve: Table([100, 50]))").is_err());
    }

    #[test]
    fn combat_levels_start_at_three() {
        let skills = registry(
            r#"(curve: Table([100, 400, 1000]), skills: [(name: "attack", title: "Attack", description: ""), (name: "defence", title: "Defence", description: "")])"#,
        );
        let mut player = Skills::default();
        // Hitpoints counts as 10 though this registry has none: (1 + 10) / 4 + 0.65.
        assert_eq!(skills.combat_level(&player), 3);
        player.grant(skills.id("core:attack").unwrap(), 10_000);
        assert_eq!(skills.combat_level(&player), 5);
    }

    #[test]
    fn skills_start_at_their_level() {
        let skills = registry(r#"(curve: RuneScape(max: 99), skills: [(name: "farming", title: "", description: ""), (name: "hitpoints", title: "", description: "", start: 10)])"#);
        let mut player = Skills::default();
        skills.start(&mut player);
        assert_eq!((skills.level_of(&player, "core:farming"), skills.level_of(&player, "core:hitpoints")), (1, 10));
        let had = player.xp(SkillId(1));
        skills.start(&mut player);
        assert_eq!(player.xp(SkillId(1)), had, "starting again changes nothing");
    }

    #[test]
    fn experience_adds_up_per_skill() {
        let mut skills = Skills::default();
        assert_eq!(skills.grant(SkillId(2), 185), 185);
        assert_eq!(skills.grant(SkillId(2), 15), 200);
        assert_eq!((skills.xp(SkillId(0)), skills.xp(SkillId(2)), skills.xp(SkillId(7))), (0, 200, 0));
    }
}
