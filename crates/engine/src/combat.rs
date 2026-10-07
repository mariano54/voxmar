//! Fighting: the three styles, weapons, and how hard and how often a blow lands
//! (docs/combat.md).
//!
//! **Timing.** How hard a blow lands for how long the striker waited: Minecraft's swing
//! charge without its knockback, tuned so timing matters a little and spamming is never
//! worth much. A full-strength blow takes the weapon's cooldown since the last (fists and
//! swords [`FULL_CHARGE`]); blows come no faster than half of it. In between, strength
//! rises in a straight line from [`SPAM_STRENGTH`] to 1. Damage per second is then
//! `strength(t) / t`, which is highest for well-timed blows and 90% of that at the quickest
//! (Mariano, 2026-10-06: spam-clicking about 10% worse); waiting longer than a full charge
//! gains nothing.
//!
//! **Styles.** Melee (stab, slash, crush), Ranged and Magic. Each weapon fights in one; a
//! creature attacks in one and may be weak to one, against which its defence counts half.
//!
//! **Weapons** are content (each package's `combat.ron`): a class (sword, dagger, bow) sets
//! the style, the cooldown and any extra reach; each weapon names its item, the level that
//! wields it (Attack for melee, Ranged, Magic) and its max hit at that level, which rises by
//! one for every eight levels above it, as a bare fist's does from 1. A weapon held below
//! its level strikes as a fist.

use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

/// Seconds since the last blow for one at full strength, with fists or a sword.
pub const FULL_CHARGE: f32 = 0.6;
/// The least seconds between blows with fists or a sword: quicker ones are not taken.
pub const QUICKEST: f32 = FULL_CHARGE / 2.0;
/// Strength of a blow at the quickest.
pub const SPAM_STRENGTH: f32 = 0.45;

/// The share of its full damage a blow `since` seconds after the last one deals with fists
/// or a sword (`None` for the first blow in a while: full).
pub fn strength(since: Option<f32>) -> f32 {
    strength_for(since, FULL_CHARGE)
}

/// The share of its full damage a blow `since` seconds after the last one deals with a
/// weapon that charges fully in `cooldown` seconds.
pub fn strength_for(since: Option<f32>, cooldown: f32) -> f32 {
    let Some(since) = since else { return 1.0 };
    let quickest = cooldown / 2.0;
    let t = ((since - quickest) / (cooldown - quickest)).clamp(0.0, 1.0);
    SPAM_STRENGTH + (1.0 - SPAM_STRENGTH) * t
}

/// Whole damage from `full` scaled by `strength`, the fraction rounded up with chance equal
/// to it (drawn as `roll` in 0..1), so the average is exact even for small blows.
pub fn scaled(full: u16, strength: f32, roll: f32) -> u16 {
    let exact = full as f32 * strength.clamp(0.0, 1.0);
    exact.floor() as u16 + u16::from(roll < exact.fract())
}

/// How a blow is struck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Style {
    /// Melee: daggers, spears, fangs and beaks.
    Stab,
    /// Melee: swords, axes, claws.
    Slash,
    /// Melee: fists, clubs, hammers, hooves and horns.
    #[default]
    Crush,
    /// Slings, bows, crossbows, thrown weapons and firearms.
    Ranged,
    /// Spells.
    Magic,
}

impl Style {
    pub fn is_melee(self) -> bool {
        matches!(self, Style::Stab | Style::Slash | Style::Crush)
    }

    /// The skill that wields weapons of the style and trains with their blows.
    pub fn skill(self) -> &'static str {
        match self {
            Style::Stab | Style::Slash | Style::Crush => "core:attack",
            Style::Ranged => "core:ranged",
            Style::Magic => "core:magic",
        }
    }

    /// Its lang key's last part.
    pub fn key(self) -> &'static str {
        match self {
            Style::Stab => "stab",
            Style::Slash => "slash",
            Style::Crush => "crush",
            Style::Ranged => "ranged",
            Style::Magic => "magic",
        }
    }
}

/// A defence level against a blow of `style`: halved when the defender is weak to it.
pub fn defence_against(level: u8, weak: Option<Style>, style: Style) -> u8 {
    if weak == Some(style) { level / 2 } else { level }
}

/// The chance a blow lands, RuneScape-style: an attack roll from the striker's level and
/// accuracy against a defence roll from the defender's level and armour.
pub fn hit_chance(attack_level: u8, accuracy: u16, defence_level: u8, armour: u16) -> f32 {
    let a = (attack_level as f32 + 8.0) * (accuracy as f32 + 64.0);
    let d = (defence_level as f32 + 8.0) * (armour as f32 + 64.0);
    if a > d { 1.0 - (d + 2.0) / (2.0 * (a + 1.0)) } else { a / (2.0 * (d + 1.0)) }
}

/// A class of weapon as content defines it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassDef {
    pub name: String,
    pub style: Style,
    /// Seconds a full-strength blow takes to charge; blows come no faster than half of it.
    pub cooldown: f32,
    /// Blocks it reaches past an arm's length.
    #[serde(default)]
    pub reach: f32,
}

/// A weapon as content defines it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponDef {
    /// The item, by name.
    pub item: String,
    /// Its class, by name.
    pub class: String,
    /// The level in its style's skill that wields it.
    #[serde(default = "first_level")]
    pub level: u8,
    /// The most a blow does at that level.
    pub max_hit: u16,
    /// What it adds to the attack roll; twice its max hit unless given.
    #[serde(default)]
    pub accuracy: Option<u16>,
}

fn first_level() -> u8 {
    1
}

/// One package's `combat.ron`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CombatFile {
    #[serde(default)]
    pub classes: Vec<ClassDef>,
    #[serde(default)]
    pub weapons: Vec<WeaponDef>,
}

/// What a blow is struck with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weapon {
    /// Its class, by index (`None` for fists).
    pub class: Option<usize>,
    pub style: Style,
    pub cooldown: f32,
    pub reach: f32,
    pub skill: Option<SkillId>,
    pub level: u8,
    pub max_hit: u16,
    pub accuracy: u16,
}

impl Weapon {
    /// A bare fist: crushing, a sword's speed, a max hit of 1 that rises with Attack.
    pub const FISTS: Weapon = Weapon { class: None, style: Style::Crush, cooldown: FULL_CHARGE, reach: 0.0, skill: None, level: 0, max_hit: 1, accuracy: 0 };

    /// The most a blow does for a striker at `level` in the weapon's skill: one more for
    /// every eight levels above the weapon's.
    pub fn max_hit(&self, level: u8) -> u16 {
        self.max_hit + (level.saturating_sub(self.level) / 8) as u16
    }

    /// The least seconds between blows.
    pub fn quickest(&self) -> f32 {
        self.cooldown / 2.0
    }
}

/// Every class and weapon, with each item's weapon worked out once.
#[derive(Debug, Default)]
pub struct CombatRegistry {
    classes: Vec<ClassDef>,
    /// Each block id's weapon, if it is one.
    weapons: Vec<Option<Weapon>>,
    /// The skill fists train and are judged by.
    attack: Option<SkillId>,
}

impl CombatRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<CombatFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut file: CombatFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/combat.ron: {e}")))?;
        for weapon in &mut file.weapons {
            if !weapon.item.contains(':') {
                weapon.item = format!("{namespace}:{}", weapon.item);
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<CombatFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let (mut classes, mut defs) = (Vec::new(), Vec::new());
        for file in files {
            classes.extend(file.classes);
            defs.extend(file.weapons);
        }
        let mut weapons = vec![None; blocks.len()];
        for def in defs {
            let error = |why: String| RegistryError(format!("combat.ron: {}: {why}", def.item));
            let item = blocks.id(&def.item).ok_or_else(|| error("no such item".into()))?;
            let index = classes.iter().position(|c| c.name == def.class).ok_or_else(|| error(format!("no class `{}`", def.class)))?;
            let class = &classes[index];
            let skill = skills.id(class.style.skill());
            if weapons[item.0 as usize].is_some() {
                return Err(error("listed twice".into()));
            }
            weapons[item.0 as usize] = Some(Weapon {
                class: Some(index),
                style: class.style,
                cooldown: class.cooldown.max(0.1),
                reach: class.reach,
                skill,
                level: def.level,
                max_hit: def.max_hit.max(1),
                accuracy: def.accuracy.unwrap_or(def.max_hit * 2),
            });
        }
        Ok(Self { classes, weapons, attack: skills.id("core:attack") })
    }

    /// What `item` is as a weapon, if it is one.
    pub fn weapon(&self, item: BlockId) -> Option<&Weapon> {
        self.weapons.get(item.0 as usize).and_then(Option::as_ref)
    }

    /// What a blow is struck with, holding `item`, for a striker whose own level in a skill
    /// is `level(skill)`: the weapon, or fists when nothing is held, the item is no weapon
    /// or the striker's level is below it. Boosts never help to wield a weapon.
    pub fn wielded(&self, item: Option<BlockId>, level: impl Fn(SkillId) -> u8) -> Weapon {
        let fists = Weapon { skill: self.attack, ..Weapon::FISTS };
        match item.and_then(|item| self.weapon(item)) {
            Some(weapon) if weapon.skill.is_none_or(|skill| level(skill) >= weapon.level) => *weapon,
            _ => fists,
        }
    }

    pub fn class(&self, index: usize) -> &ClassDef {
        &self.classes[index]
    }

    /// Every item that is a weapon, with its weapon, in registry order.
    pub fn weapons(&self) -> impl Iterator<Item = (BlockId, &Weapon)> + '_ {
        self.weapons.iter().enumerate().filter_map(|(i, w)| Some((BlockId(i as u16), w.as_ref()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_blows_beat_spam_by_a_tenth_and_waiting_longer_gains_nothing() {
        for cooldown in [FULL_CHARGE, 0.4, 0.8, 1.2] {
            let dps = |t: f32| strength_for(Some(t), cooldown) / t;
            let timed = dps(cooldown);
            assert!((dps(cooldown / 2.0) / timed - 0.9).abs() < 1e-4, "{cooldown}");
            for share in [0.58, 0.67, 0.83, 0.92] {
                let t = cooldown * share;
                assert!(dps(t) < timed && dps(t) > 0.9 * timed - 1e-4, "{cooldown} {t}");
            }
            assert!(dps(cooldown * 2.0) < timed, "waiting past a full charge only loses damage per second");
        }
        assert_eq!(strength(None), 1.0);
        assert_eq!(strength(Some(FULL_CHARGE)), strength_for(Some(FULL_CHARGE), FULL_CHARGE));
    }

    #[test]
    fn scaled_damage_averages_exactly() {
        let mean: f32 = (0..1000).map(|i| scaled(3, 0.45, i as f32 / 1000.0) as f32).sum::<f32>() / 1000.0;
        assert!((mean - 1.35).abs() < 0.01, "{mean}");
        assert_eq!(scaled(4, 1.0, 0.99), 4);
    }

    #[test]
    fn hit_chance_is_runescapes_and_accuracy_and_armour_move_it() {
        assert!((hit_chance(1, 0, 1, 0) - 0.4975).abs() < 0.01);
        assert!(hit_chance(60, 0, 1, 0) > 0.9);
        assert!(hit_chance(1, 0, 60, 0) < 0.1);
        assert!(hit_chance(20, 18, 20, 0) > hit_chance(20, 0, 20, 0));
        assert!(hit_chance(20, 0, 20, 30) < hit_chance(20, 0, 20, 0));
        // The creatures' rolls are the same with nothing worn or held.
        assert_eq!(hit_chance(12, 0, 7, 0), crate::creature::hit_chance(12, 7));
    }

    #[test]
    fn a_weakness_halves_defence_against_its_style_only() {
        assert_eq!(defence_against(20, Some(Style::Crush), Style::Crush), 10);
        assert_eq!(defence_against(20, Some(Style::Crush), Style::Slash), 20);
        assert_eq!(defence_against(21, None, Style::Magic), 21);
    }

    #[test]
    fn max_hit_rises_a_point_every_eight_levels_above_the_weapon() {
        assert_eq!(Weapon::FISTS.max_hit(1), 1);
        assert_eq!(Weapon::FISTS.max_hit(8), 2);
        assert_eq!(Weapon::FISTS.max_hit(99), 13);
        let sword = Weapon { level: 20, max_hit: 9, ..Weapon::FISTS };
        assert_eq!(sword.max_hit(20), 9);
        assert_eq!(sword.max_hit(27), 9);
        assert_eq!(sword.max_hit(28), 10);
        assert_eq!(sword.max_hit(1), 9);
    }

    fn registry(source: &str) -> Result<(CombatRegistry, BlockRegistry), RegistryError> {
        use crate::block::BlockDef;
        let block = |name: &str| BlockDef { name: format!("t:{name}"), ..Default::default() };
        let blocks = BlockRegistry::new(vec![block("air"), block("stick"), block("sword"), block("spear")]).unwrap();
        let mut skills = SkillRegistry::default();
        skills.extend("core", r#"(curve: RuneScape(max: 99), skills: [(name: "attack", title: "", description: "")])"#).unwrap();
        Ok((CombatRegistry::new(vec![CombatRegistry::read("t", source)?], &blocks, &skills)?, blocks))
    }

    #[test]
    fn weapons_are_wielded_from_their_level_and_otherwise_strike_as_fists() {
        let (combat, blocks) = registry(
            r#"(
                classes: [(name: "sword", style: Slash, cooldown: 0.6), (name: "spear", style: Stab, cooldown: 0.73, reach: 1.0)],
                weapons: [(item: "sword", class: "sword", level: 5, max_hit: 5), (item: "spear", class: "spear", max_hit: 3, accuracy: 10)],
            )"#,
        )
        .unwrap();
        let [sword, spear, stick] = ["t:sword", "t:spear", "t:stick"].map(|name| blocks.id(name).unwrap());
        let at = |level: u8| move |_| level;
        assert_eq!(combat.wielded(Some(sword), at(5)).max_hit, 5);
        assert_eq!(combat.wielded(Some(sword), at(5)).style, Style::Slash);
        assert_eq!(combat.wielded(Some(sword), at(5)).accuracy, 10);
        assert_eq!(combat.wielded(Some(sword), at(4)).class, None, "below its level a sword is a fist");
        assert_eq!(combat.wielded(Some(stick), at(99)).class, None);
        assert_eq!(combat.wielded(None, at(99)).max_hit(99), 13);
        let spear = combat.wielded(Some(spear), at(1));
        assert_eq!((spear.reach, spear.accuracy, spear.quickest()), (1.0, 10, 0.365));
        assert!(registry(r#"(weapons: [(item: "nothing", class: "sword", max_hit: 1)])"#).is_err());
        assert!(registry(r#"(classes: [(name: "sword", style: Slash, cooldown: 0.6)], weapons: [(item: "sword", class: "axe", max_hit: 1)])"#).is_err());
    }
}
