//! Fighting: the three styles, weapons, and how hard and how often a blow lands
//! (docs/combat.md).
//!
//! **Rhythm** (the design wiki's "Combat feel"). A blow is a swing: it winds up, lands,
//! then recovers until the class's cooldown meter is full again: that moment is the beat.
//! A click in the buffer just before the beat is held and fires on it; one in the beat
//! window just after is on it too. On the beat, a blow gains accuracy and a rhythm pip;
//! with all the pips, the next on-beat blow is a flourish, with more max hit and its
//! class's effect (bleed, cleave, pin, sunder, daze). A click earlier than the buffer, but
//! past half the cooldown, fires at once at part strength: the quickest at 45%, so spam-
//! clicking deals about 90% of timed damage per second (Mariano, 6 Oct) ([`Rhythm`]).
//!
//! **Guard and dodge.** Holding a melee weapon (or, later, a shield) up guards an arc in
//! front, stopping a share of melee blows (less of crush); raised just before a blow lands
//! it is a perfect block, which stops all of it, staggers the attacker and makes the next
//! blow a counter that cannot miss. A dodge is a short dash with a moment no blow lands.
//! Creatures wind up before every attack, so both can be read. Every number is content:
//! each class's, the [`Feel`] and the dodges in `combat.ron`.
//!
//! **Styles.** Melee (stab, slash, crush), Ranged and Magic. Each weapon fights in one; a
//! creature attacks in one and may be weak to one, against which its defence counts half.
//!
//! **Weapons** are content (each package's `combat.ron`): a class (sword, dagger, bow) sets
//! the style, the cooldown and any extra reach; each weapon names its item, the level that
//! wields it (Attack for melee, Ranged, Magic) and its max hit at that level, which rises by
//! one for every eight levels above it, as a bare fist's does from 1. A weapon held below
//! its level strikes as a fist.
//!
//! **Armour** is worn in five slots (head, body, legs, hands, feet), a set's pieces sharing
//! its material's armour by slot. Worn armour adds to the defence roll against every
//! blow, more or less by its weight (plate turns blades and arrows but not spells, robes the
//! reverse) and half again against a style its set resists. A piece worn below its Defence
//! level gives nothing, as a weapon held below its level strikes as a fist. A **shield** goes
//! in the off-hand: it guards in place of the weapon (stopping more, over a wider arc, and
//! arrows too) unless the weapon takes both hands. The heaviest armour worn picks the dodge.

use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

/// Seconds since the last blow for one at full strength, with fists or a sword.
pub const FULL_CHARGE: f32 = 0.6;
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
    hit_chance_with(attack_level, accuracy, defence_level, armour, 0.0)
}

/// [`hit_chance`] with the attack roll raised by `bonus` (0.05: 5%), as a blow on the beat's is.
pub fn hit_chance_with(attack_level: u8, accuracy: u16, defence_level: u8, armour: u16, bonus: f32) -> f32 {
    let a = (attack_level as f32 + 8.0) * (accuracy as f32 + 64.0) * (1.0 + bonus);
    let d = (defence_level as f32 + 8.0) * (armour as f32 + 64.0);
    if a > d { 1.0 - (d + 2.0) / (2.0 * (a + 1.0)) } else { a / (2.0 * (d + 1.0)) }
}

/// A class of weapon as content defines it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassDef {
    pub name: String,
    pub style: Style,
    /// Seconds from one swing to the next on the beat: the cooldown meter's length.
    pub cooldown: f32,
    /// Blocks it reaches past an arm's length.
    #[serde(default)]
    pub reach: f32,
    /// Seconds a swing winds up before its blow lands.
    #[serde(default)]
    pub windup: f32,
    /// Seconds before the beat a click is held for it, and after it a click is on it.
    #[serde(default = "buffer")]
    pub buffer: f32,
    #[serde(default = "beat")]
    pub beat: f32,
    /// How it guards, if it can.
    #[serde(default)]
    pub guard: Option<GuardDef>,
    /// What a flourish adds besides its max hit.
    #[serde(default)]
    pub flourish: Option<Flourish>,
    /// Held in both hands: no shield beside it.
    #[serde(default)]
    pub two_handed: bool,
}

fn buffer() -> f32 {
    0.15
}

fn beat() -> f32 {
    0.10
}

/// A guard: what share of a melee blow it stops, over what arc in front (degrees), the
/// seconds before a blow a raise counts as a perfect block, and the speed on foot while
/// guarding, as a share.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuardDef {
    pub stop: f32,
    pub arc: f32,
    pub perfect: f32,
    #[serde(default = "guard_speed")]
    pub speed: f32,
    /// Whether it stops arrows and thrown things too, as shields do.
    #[serde(default)]
    pub missiles: bool,
}

fn guard_speed() -> f32 {
    0.5
}

impl GuardDef {
    /// Whether a blow from `from` falls in the arc of a guard facing `facing` (both flat
    /// directions, any length).
    pub fn covers(&self, facing: glam::Vec3, from: glam::Vec3) -> bool {
        let (a, b) = (facing.with_y(0.0).normalize_or_zero(), from.with_y(0.0).normalize_or_zero());
        if a == glam::Vec3::ZERO || b == glam::Vec3::ZERO {
            return true;
        }
        a.dot(b) >= (self.arc.to_radians() / 2.0).cos() - 1e-4
    }

    /// The share of a blow in `style` it stops: melee (half of it against crush, which
    /// breaks guards), and missiles only if it is a shield; never magic.
    pub fn stops(&self, style: Style, feel: &Feel) -> f32 {
        match style {
            Style::Crush => self.stop * (1.0 - feel.crush_through),
            Style::Stab | Style::Slash => self.stop,
            Style::Ranged if self.missiles => self.stop,
            Style::Ranged | Style::Magic => 0.0,
        }
    }
}

/// What a flourish does to its target besides the extra max hit.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub enum Flourish {
    /// The share of the hit again, over `seconds`.
    Bleed { share: f32, seconds: f32 },
    /// A second creature in front takes the share of the hit.
    Cleave { share: f32 },
    /// The target moves slower by the share, for `seconds`.
    Pin { slow: f32, seconds: f32 },
    /// The target's defence is lower by the share, for `seconds`; again only renews it.
    Sunder { defence: f32, seconds: f32 },
    /// Interrupts the target's wind-up and staggers it (bosses are immune).
    Daze { stagger: f32 },
}

/// How a dodge goes in armour of a weight: how far and for how long the dash, the seconds
/// after it starts that blows pass through, and the seconds before the next.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DodgeDef {
    pub weight: String,
    pub distance: f32,
    pub time: f32,
    pub safe: (f32, f32),
    pub cooldown: f32,
}

impl Default for DodgeDef {
    fn default() -> Self {
        Self { weight: "light".into(), distance: 3.0, time: 0.25, safe: (0.04, 0.22), cooldown: 1.0 }
    }
}

/// How fighting feels beyond each class: every share and timing of the rhythm, guard and
/// creatures' attacks, as content sets them (a package's `combat.ron`; the last package
/// to set it wins, and what it leaves out keeps these defaults).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Feel {
    /// A click this share of the cooldown after the last swing is the soonest that fires,
    /// at [`Feel::spam_strength`]; strength rises to full at the buffer.
    pub quickest: f32,
    pub spam_strength: f32,
    /// What a blow on the beat adds to the attack roll.
    pub beat_accuracy: f32,
    /// Rhythm pips: how many make a flourish, and the seconds after the last blow they go.
    pub pips: u8,
    pub pip_timeout: f32,
    /// What a flourish, a counter and an on-beat falling blow add to the max hit.
    pub flourish_max_hit: f32,
    pub counter_max_hit: f32,
    pub falling_max_hit: f32,
    /// Guarding or dodging in your own wind-up cancels the swing, and the meter starts again
    /// from this share.
    pub cancel_to: f32,
    /// Of what a guard stops, the share crush blows put through anyway.
    pub crush_through: f32,
    /// A raise this soon after lowering opens no perfect window, so tapping does nothing.
    pub no_flicker: f32,
    /// Seconds a perfect block staggers the attacker (a boss), and seconds after it a blow
    /// is a counter.
    pub stagger: f32,
    pub boss_stagger: f32,
    pub counter: f32,
    /// Seconds after a perfect dodge a blow is a riposte (on the beat by itself).
    pub riposte: f32,
    /// A creature's attack: seconds winding up and recovering, unless it says.
    pub creature_windup: f32,
    pub creature_recovery: f32,
}

impl Default for Feel {
    fn default() -> Self {
        Self {
            quickest: 0.5,
            spam_strength: 0.45,
            beat_accuracy: 0.05,
            pips: 3,
            pip_timeout: 2.0,
            flourish_max_hit: 0.2,
            counter_max_hit: 0.25,
            falling_max_hit: 0.15,
            cancel_to: 0.5,
            crush_through: 0.5,
            no_flicker: 0.4,
            stagger: 0.8,
            boss_stagger: 0.3,
            counter: 1.0,
            riposte: 0.8,
            creature_windup: 0.5,
            creature_recovery: 0.6,
        }
    }
}

/// What came of a creature's blow at a player, for the player's screen and ears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Defended {
    /// It landed.
    Hit,
    /// The defence roll turned it.
    Missed,
    /// The guard took part of it.
    Blocked,
    /// A perfect block: none of it, and the attacker staggers.
    Perfect,
    /// It passed through a dodge.
    Dodged,
    /// The player was out of its reach when it landed.
    Evaded,
}

/// What kind of blow a player struck, for its hitsplat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blow {
    #[default]
    Plain,
    /// On the beat.
    Beat,
    /// A flourish, a counter after a perfect block, or bleeding from a flourish.
    Flourish,
    Counter,
    Bleed,
}

/// One swing: when it starts and lands, how hard, and whether it is on the beat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swing {
    pub starts: f64,
    pub lands: f64,
    pub strength: f32,
    pub on_beat: bool,
}

/// A class's timing, worked out: what [`Rhythm`] needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub cooldown: f32,
    pub windup: f32,
    pub buffer: f32,
    pub beat: f32,
}

/// A striker's swings, in seconds on any one clock. The meter runs from each swing's
/// start over the cooldown; its end is the beat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rhythm {
    /// When the meter started: the last swing's start.
    started: Option<f64>,
    /// The swing winding up or held for the beat, not yet landed.
    pending: Option<Swing>,
    /// Rhythm pips, and when the last blow landed.
    pub pips: u8,
    last_blow: f64,
}

/// What a click did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Click {
    /// Too soon after the last swing, or one is already winding up or held.
    Ignored,
    /// A swing, at once or held for the beat.
    Swing(Swing),
}

impl Rhythm {
    /// A click at `now`: on or just after the beat (or resting), a swing at full strength;
    /// inside the buffer before the beat, held for it; earlier but past the quickest, a
    /// swing at once at part strength.
    pub fn click(&mut self, now: f64, timing: &Timing, feel: &Feel) -> Click {
        if self.pending.is_some() {
            return Click::Ignored;
        }
        let swing = match self.started {
            None => Swing { starts: now, lands: 0.0, strength: 1.0, on_beat: false },
            Some(started) => {
                let beat = started + timing.cooldown as f64;
                let since = (now - started) as f32;
                if now >= beat {
                    Swing { starts: now, lands: 0.0, strength: 1.0, on_beat: now - beat <= timing.beat as f64 }
                } else if beat - now <= timing.buffer as f64 {
                    Swing { starts: beat, lands: 0.0, strength: 1.0, on_beat: true }
                } else if since >= timing.cooldown * feel.quickest {
                    // From the quickest to the buffer, strength rises to full.
                    let full_at = (timing.cooldown - timing.buffer).max(timing.cooldown * feel.quickest + 1e-3);
                    let t = ((since - timing.cooldown * feel.quickest) / (full_at - timing.cooldown * feel.quickest)).clamp(0.0, 1.0);
                    Swing { starts: now, lands: 0.0, strength: feel.spam_strength + (1.0 - feel.spam_strength) * t, on_beat: false }
                } else {
                    return Click::Ignored;
                }
            }
        };
        let swing = Swing { lands: swing.starts + timing.windup.max(0.0) as f64, ..swing };
        self.started = Some(swing.starts);
        self.pending = Some(swing);
        Click::Swing(swing)
    }

    /// The swing that has landed by `now`, if one has.
    pub fn landed(&mut self, now: f64) -> Option<Swing> {
        let swing = self.pending.filter(|s| s.lands <= now)?;
        self.pending = None;
        Some(swing)
    }

    /// The swing winding up or held, if any.
    pub fn pending(&self) -> Option<Swing> {
        self.pending
    }

    /// Guarding or dodging cancels a swing not yet landed: the meter starts again from
    /// [`Feel::cancel_to`] of the cooldown. Returns whether one was cancelled.
    pub fn cancel(&mut self, now: f64, timing: &Timing, feel: &Feel) -> bool {
        if self.pending.take().is_none() {
            return false;
        }
        self.started = Some(now - (timing.cooldown * feel.cancel_to) as f64);
        true
    }

    /// The meter starts again from empty (a bite of food costs a beat).
    pub fn restart(&mut self, now: f64) {
        self.pending = None;
        self.started = Some(now);
    }

    /// How full the meter is at `now`, 0 to 1 (full when resting).
    pub fn meter(&self, now: f64, timing: &Timing) -> f32 {
        self.started.map_or(1.0, |s| ((now - s) as f32 / timing.cooldown.max(1e-3)).clamp(0.0, 1.0))
    }

    /// When the meter is next full: the beat.
    pub fn beat(&self, timing: &Timing) -> Option<f64> {
        self.started.map(|s| s + timing.cooldown as f64)
    }

    /// A swing landed at `now`: what kind of blow it is by the pips, which it moves on.
    /// `riposte` and `counter` blows count as on the beat; a counter adds a pip besides.
    pub fn blow(&mut self, swing: &Swing, now: f64, bonus_pip: bool, feel: &Feel) -> Blow {
        if now - self.last_blow > feel.pip_timeout as f64 {
            self.pips = 0;
        }
        self.last_blow = now;
        let kind = if !swing.on_beat {
            self.pips = 0;
            Blow::Plain
        } else if self.pips >= feel.pips {
            self.pips = 0;
            Blow::Flourish
        } else {
            self.pips += 1;
            Blow::Beat
        };
        if bonus_pip {
            self.pips = (self.pips + 1).min(feel.pips);
        }
        kind
    }

    /// Pips are lost: a hit taken unguarded, a weapon switched.
    pub fn lose_pips(&mut self) {
        self.pips = 0;
    }

    /// The pips at `now`, counting their timeout.
    pub fn pips(&self, now: f64, feel: &Feel) -> u8 {
        if now - self.last_blow > feel.pip_timeout as f64 { 0 } else { self.pips }
    }
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
    #[serde(default)]
    pub feel: Option<Feel>,
    #[serde(default)]
    pub dodges: Vec<DodgeDef>,
    #[serde(default)]
    pub sets: Vec<SetDef>,
    #[serde(default)]
    pub shields: Vec<ShieldDef>,
}

/// Where a piece of armour is worn: the inventory's worn slots, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Slot {
    Head,
    Body,
    Legs,
    Hands,
    Feet,
    /// The off-hand: a shield, which guards in place of the weapon.
    Shield,
}

impl Slot {
    pub const ALL: [Slot; 6] = [Slot::Head, Slot::Body, Slot::Legs, Slot::Hands, Slot::Feet, Slot::Shield];

    /// Its worn slot, counted from the first (`inventory::WORN`).
    pub fn index(self) -> usize {
        self as usize
    }

    /// The armour a piece in this slot gives from a set of armour 1.0 (bronze): a whole
    /// set gives 36, as RuneScape's bronze does about.
    pub fn base(self) -> f32 {
        match self {
            Slot::Head => 6.0,
            Slot::Body => 14.0,
            Slot::Legs => 10.0,
            Slot::Hands | Slot::Feet => 3.0,
            Slot::Shield => 0.0,
        }
    }

    /// Its lang key's last part.
    pub fn key(self) -> &'static str {
        match self {
            Slot::Head => "head",
            Slot::Body => "body",
            Slot::Legs => "legs",
            Slot::Hands => "hands",
            Slot::Feet => "feet",
            Slot::Shield => "shield",
        }
    }
}

/// How heavy a set is, which sets what its armour is good against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weight {
    /// Leather and hides: quiet and quick, weaker against melee.
    Light,
    /// Scale, chain and mail.
    #[default]
    Medium,
    /// Plate: best against melee and arrows, worst against spells.
    Heavy,
    /// Cloth: for spells, weak against melee.
    Robe,
}

impl Weight {
    /// The share of its armour that counts against a blow of `style`.
    pub fn against(self, style: Style) -> f32 {
        match (self, style) {
            (Weight::Medium, _) => 1.0,
            (Weight::Light, Style::Ranged | Style::Magic) => 1.0,
            (Weight::Light, _) => 0.8,
            (Weight::Heavy, Style::Magic) => 0.6,
            (Weight::Heavy, _) => 1.2,
            (Weight::Robe, Style::Magic) => 1.5,
            (Weight::Robe, Style::Ranged) => 0.7,
            (Weight::Robe, _) => 0.5,
        }
    }

    /// Its lang key's last part.
    pub fn key(self) -> &'static str {
        match self {
            Weight::Light => "light",
            Weight::Medium => "medium",
            Weight::Heavy => "heavy",
            Weight::Robe => "robe",
        }
    }
}

/// How much more armour counts against a style its set resists.
pub const RESISTED: f32 = 1.5;

/// A set of armour as content defines it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetDef {
    pub name: String,
    #[serde(default)]
    pub weight: Weight,
    /// The Defence level that wears it.
    #[serde(default = "first_level")]
    pub level: u8,
    /// Its material's armour, which each piece shares by its slot (`Slot::base`): the wiki's
    /// "Armour ×".
    pub armour: f32,
    /// Styles it is better against.
    #[serde(default)]
    pub resists: Vec<Style>,
    /// Each piece, by slot and item name.
    pub pieces: Vec<(Slot, String)>,
}

/// A shield as content defines it: worn in the off-hand, it guards in place of the weapon
/// (unless the weapon takes both hands) from its Defence level, and adds its armour.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShieldDef {
    pub item: String,
    #[serde(default = "first_level")]
    pub level: u8,
    pub guard: GuardDef,
    #[serde(default)]
    pub armour: u16,
}

/// A shield, worked out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shield {
    pub level: u8,
    pub guard: GuardDef,
    pub armour: u16,
}

/// A piece of armour, worked out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    /// Its set, by index.
    pub set: usize,
    pub slot: Slot,
    pub weight: Weight,
    pub level: u8,
    pub armour: u16,
}

/// The class bare fists fight as, if content defines one by this name.
pub const FIST_CLASS: &str = "fist";

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

}

/// Every class and weapon, with each item's weapon worked out once.
#[derive(Debug, Default)]
pub struct CombatRegistry {
    classes: Vec<ClassDef>,
    /// Each block id's weapon, if it is one.
    weapons: Vec<Option<Weapon>>,
    /// The skill fists train and are judged by.
    attack: Option<SkillId>,
    /// The skill armour and shields are worn by.
    defence: Option<SkillId>,
    sets: Vec<SetDef>,
    /// Each block id's shield, if it is one.
    shields: Vec<Option<Shield>>,
    /// Each block id's piece of armour, if it is one.
    pieces: Vec<Option<Piece>>,
    pub feel: Feel,
    /// The class fists fight as, if there is one.
    fist: Option<usize>,
    /// Dodges by armour weight; the first is for no armour.
    dodges: Vec<DodgeDef>,
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
        for (_, item) in file.sets.iter_mut().flat_map(|set| &mut set.pieces) {
            if !item.contains(':') {
                *item = format!("{namespace}:{item}");
            }
        }
        for shield in &mut file.shields {
            if !shield.item.contains(':') {
                shield.item = format!("{namespace}:{}", shield.item);
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<CombatFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let (mut classes, mut defs, mut feel, mut dodges, mut sets, mut shield_defs) = (Vec::new(), Vec::new(), Feel::default(), Vec::<DodgeDef>::new(), Vec::new(), Vec::new());
        for file in files {
            classes.extend(file.classes);
            defs.extend(file.weapons);
            sets.extend(file.sets);
            shield_defs.extend(file.shields);
            feel = file.feel.unwrap_or(feel);
            for dodge in file.dodges {
                // A later package's dodge for a weight replaces an earlier one's.
                dodges.retain(|d| d.weight != dodge.weight);
                dodges.push(dodge);
            }
        }
        if dodges.is_empty() {
            dodges.push(DodgeDef::default());
        }
        if let Some(class) = classes.iter().find(|c| c.windup < 0.0 || c.windup >= c.cooldown || c.buffer < 0.0 || c.beat < 0.0) {
            return Err(RegistryError(format!("combat.ron: class `{}`: its wind-up must be under its cooldown, and no time may be negative", class.name)));
        }
        let mut pieces = vec![None; blocks.len()];
        for (index, set) in sets.iter().enumerate() {
            for (slot, name) in &set.pieces {
                let error = |why: &str| RegistryError(format!("combat.ron: set `{}`: {name}: {why}", set.name));
                let item = blocks.id(name).ok_or_else(|| error("no such item"))?;
                if pieces[item.0 as usize].is_some() {
                    return Err(error("listed twice"));
                }
                let armour = (slot.base() * set.armour).round().max(1.0) as u16;
                pieces[item.0 as usize] = Some(Piece { set: index, slot: *slot, weight: set.weight, level: set.level, armour });
            }
        }
        let mut shields = vec![None; blocks.len()];
        for def in shield_defs {
            let error = |why: &str| RegistryError(format!("combat.ron: shield {}: {why}", def.item));
            let item = blocks.id(&def.item).ok_or_else(|| error("no such item"))?;
            if pieces[item.0 as usize].is_some() || shields[item.0 as usize].is_some() {
                return Err(error("listed twice"));
            }
            shields[item.0 as usize] = Some(Shield { level: def.level, guard: def.guard, armour: def.armour });
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
        let fist = classes.iter().position(|c| c.name == FIST_CLASS);
        Ok(Self { classes, weapons, attack: skills.id("core:attack"), defence: skills.id("core:defence"), feel, fist, dodges, sets, shields, pieces })
    }

    /// What `item` is as a weapon, if it is one.
    pub fn weapon(&self, item: BlockId) -> Option<&Weapon> {
        self.weapons.get(item.0 as usize).and_then(Option::as_ref)
    }

    /// What a blow is struck with, holding `item`, for a striker whose own level in a skill
    /// is `level(skill)`: the weapon, or fists when nothing is held, the item is no weapon
    /// or the striker's level is below it. Boosts never help to wield a weapon.
    pub fn wielded(&self, item: Option<BlockId>, level: impl Fn(SkillId) -> u8) -> Weapon {
        let cooldown = self.fist.map_or(Weapon::FISTS.cooldown, |i| self.classes[i].cooldown);
        let fists = Weapon { skill: self.attack, cooldown, ..Weapon::FISTS };
        match item.and_then(|item| self.weapon(item)) {
            Some(weapon) if weapon.skill.is_none_or(|skill| level(skill) >= weapon.level) => *weapon,
            _ => fists,
        }
    }

    /// What `item` is as armour, if it is some.
    pub fn piece(&self, item: BlockId) -> Option<&Piece> {
        self.pieces.get(item.0 as usize).and_then(Option::as_ref)
    }

    pub fn set(&self, index: usize) -> &SetDef {
        &self.sets[index]
    }

    /// What `item` is as a shield, if it is one.
    pub fn shield(&self, item: BlockId) -> Option<&Shield> {
        self.shields.get(item.0 as usize).and_then(Option::as_ref)
    }

    /// Each block id's worn slot, if it is armour or a shield (`inventory::set_worn_slots`).
    pub fn worn_slots(&self) -> Vec<Option<u8>> {
        let shield = Slot::Shield.index() as u8;
        self.pieces.iter().zip(&self.shields).map(|(p, s)| p.map(|p| p.slot.index() as u8).or(s.map(|_| shield))).collect()
    }

    /// What `piece` adds to the defence roll against a blow of `style`.
    pub fn armour_against(&self, piece: &Piece, style: Style) -> f32 {
        let resisted = if self.sets[piece.set].resists.contains(&style) { RESISTED } else { 1.0 };
        piece.armour as f32 * piece.weight.against(style) * resisted
    }

    /// The armour `worn` gives against a blow of `style`, for a wearer whose own Defence
    /// level is `level`: pieces worn below their level give nothing.
    pub fn worn_armour(&self, worn: impl IntoIterator<Item = BlockId>, style: Style, level: u8) -> u16 {
        let total: f32 = worn
            .into_iter()
            .map(|item| match (self.piece(item), self.shield(item)) {
                (Some(p), _) if level >= p.level => self.armour_against(p, style),
                (_, Some(s)) if level >= s.level => s.armour as f32,
                _ => 0.0,
            })
            .sum();
        total.round() as u16
    }

    /// How a fighter holding `held`, with `worn` on (the inventory's worn slots), guards by
    /// their own levels (`level`).
    pub fn guard_of(&self, held: Option<BlockId>, worn: &[Option<crate::inventory::ItemStack>], level: impl Fn(SkillId) -> u8) -> Option<GuardDef> {
        let weapon = self.wielded(held, &level);
        let off_hand = worn.get(Slot::Shield.index()).copied().flatten().map(|s| s.item);
        self.guard_with(&weapon, off_hand, self.defence.map_or(1, &level))
    }

    /// The armour `worn` (the inventory's worn slots) gives against a blow of `style`, by
    /// the wearer's own Defence level.
    pub fn armour_of(&self, worn: &[Option<crate::inventory::ItemStack>], style: Style, level: impl Fn(SkillId) -> u8) -> u16 {
        self.worn_armour(worn.iter().flatten().map(|s| s.item), style, self.defence.map_or(1, level))
    }

    /// The dodge in `worn` (the inventory's worn slots).
    pub fn dodge_of(&self, worn: &[Option<crate::inventory::ItemStack>]) -> &DodgeDef {
        self.dodge_in(worn.iter().flatten().map(|s| s.item))
    }

    /// The heaviest weight among `worn`, if any of it is armour.
    pub fn heaviest(&self, worn: impl IntoIterator<Item = BlockId>) -> Option<Weight> {
        let rank = |w: Weight| match w {
            Weight::Robe => 0,
            Weight::Light => 1,
            Weight::Medium => 2,
            Weight::Heavy => 3,
        };
        worn.into_iter().filter_map(|item| self.piece(item)).map(|p| p.weight).max_by_key(|&w| rank(w))
    }

    /// The dodge in `worn`: by its heaviest weight (robes dodge as light armour).
    pub fn dodge_in(&self, worn: impl IntoIterator<Item = BlockId>) -> &DodgeDef {
        self.dodge(self.heaviest(worn).map(|w| if w == Weight::Robe { "light" } else { w.key() }))
    }

    /// The class `weapon` fights as: its own, or for fists the fist class, if any.
    pub fn class_of(&self, weapon: &Weapon) -> Option<&ClassDef> {
        weapon.class.or(self.fist).map(|i| &self.classes[i])
    }

    /// `weapon`'s rhythm.
    pub fn timing(&self, weapon: &Weapon) -> Timing {
        match self.class_of(weapon) {
            Some(class) => Timing { cooldown: weapon.cooldown, windup: class.windup, buffer: class.buffer, beat: class.beat },
            None => Timing { cooldown: weapon.cooldown, windup: 0.0, buffer: buffer(), beat: beat() },
        }
    }

    /// How `weapon` guards, if it can.
    pub fn guard(&self, weapon: &Weapon) -> Option<GuardDef> {
        self.class_of(weapon).and_then(|c| c.guard)
    }

    /// How a fighter guards with `weapon` and `off_hand` worn, at Defence `level`: with the
    /// shield, if it is one worn at its level and the weapon leaves a hand free; else with
    /// the weapon.
    pub fn guard_with(&self, weapon: &Weapon, off_hand: Option<BlockId>, level: u8) -> Option<GuardDef> {
        let two_handed = self.class_of(weapon).is_some_and(|c| c.two_handed);
        match off_hand.and_then(|item| self.shield(item)) {
            Some(shield) if !two_handed && level >= shield.level => Some(shield.guard),
            _ => self.guard(weapon),
        }
    }

    /// What a flourish with `weapon` does besides its max hit.
    pub fn flourish(&self, weapon: &Weapon) -> Option<Flourish> {
        self.class_of(weapon).and_then(|c| c.flourish)
    }

    /// The dodge in armour of `weight` (the first, for no armour, when none is listed).
    pub fn dodge(&self, weight: Option<&str>) -> &DodgeDef {
        weight.and_then(|w| self.dodges.iter().find(|d| d.weight == w)).unwrap_or(&self.dodges[0])
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
        let blocks = BlockRegistry::new(vec![block("air"), block("stick"), block("sword"), block("spear"), block("stone")]).unwrap();
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
        assert_eq!((spear.reach, spear.accuracy, spear.cooldown), (1.0, 10, 0.73));
        assert!(registry(r#"(weapons: [(item: "nothing", class: "sword", max_hit: 1)])"#).is_err());
        assert!(registry(r#"(classes: [(name: "sword", style: Slash, cooldown: 0.6)], weapons: [(item: "sword", class: "axe", max_hit: 1)])"#).is_err());
    }

    #[test]
    fn armour_counts_by_slot_weight_resistance_and_level() {
        let (combat, blocks) = registry(
            r#"(sets: [
                (name: "chain", weight: Medium, level: 20, armour: 1.2, resists: [Slash], pieces: [(Body, "sword"), (Head, "spear")]),
                (name: "plate", weight: Heavy, level: 1, armour: 1.0, pieces: [(Legs, "stick")]),
            ])"#,
        )
        .unwrap();
        let [shirt, hood, legs] = ["t:sword", "t:spear", "t:stick"].map(|name| blocks.id(name).unwrap());
        let piece = *combat.piece(shirt).unwrap();
        assert_eq!((piece.slot, piece.armour, piece.level), (Slot::Body, 17, 20));
        assert_eq!(combat.worn_slots()[hood.0 as usize], Some(Slot::Head.index() as u8));
        // 17 + 7 against a stab, half again against the slash chain resists.
        assert_eq!(combat.worn_armour([shirt, hood], Style::Stab, 20), 24);
        assert_eq!(combat.worn_armour([shirt, hood], Style::Slash, 20), 36);
        assert_eq!(combat.worn_armour([shirt, hood], Style::Slash, 19), 0, "below its level armour gives nothing");
        // Plate turns blades better than spells.
        assert_eq!((combat.worn_armour([legs], Style::Crush, 1), combat.worn_armour([legs], Style::Magic, 1)), (12, 6));
        assert!(hit_chance(20, 0, 20, 36) < hit_chance(20, 0, 20, 0) * 0.8);
        assert!(registry(r#"(sets: [(name: "x", armour: 1.0, pieces: [(Head, "nothing")])])"#).is_err());
        assert!(registry(r#"(sets: [(name: "x", armour: 1.0, pieces: [(Head, "sword"), (Body, "sword")])])"#).is_err());
    }

    #[test]
    fn a_shield_guards_unless_the_weapon_takes_both_hands_and_plate_dodges_short() {
        let (combat, blocks) = registry(
            r#"(
                classes: [
                    (name: "sword", style: Slash, cooldown: 0.6, guard: Some((stop: 0.35, arc: 90, perfect: 0.15))),
                    (name: "spear", style: Stab, cooldown: 0.73, two_handed: true, guard: Some((stop: 0.35, arc: 90, perfect: 0.12))),
                ],
                weapons: [(item: "sword", class: "sword", max_hit: 5), (item: "spear", class: "spear", max_hit: 5)],
                sets: [(name: "plate", weight: Heavy, armour: 1.0, pieces: [(Body, "stick")])],
                shields: [(item: "stone", level: 5, armour: 4, guard: (stop: 0.65, arc: 150, perfect: 0.2, missiles: true))],
                dodges: [(weight: "light", distance: 3.0, time: 0.25, safe: (0.04, 0.22), cooldown: 1.0), (weight: "heavy", distance: 2.2, time: 0.25, safe: (0.04, 0.18), cooldown: 1.5)],
            )"#,
        )
        .unwrap();
        let [sword, spear, plate, shield] = ["t:sword", "t:spear", "t:stick", "t:stone"].map(|name| blocks.id(name).unwrap());
        let level = |_| 99;
        let (sword, spear) = (combat.wielded(Some(sword), level), combat.wielded(Some(spear), level));
        assert_eq!(combat.worn_slots()[shield.0 as usize], Some(Slot::Shield.index() as u8));
        assert_eq!(combat.guard_with(&sword, Some(shield), 5).map(|g| g.stop), Some(0.65));
        assert_eq!(combat.guard_with(&sword, Some(shield), 4).map(|g| g.stop), Some(0.35), "below its level the sword guards");
        assert_eq!(combat.guard_with(&spear, Some(shield), 5).map(|g| g.stop), Some(0.35), "a spear takes both hands");
        assert_eq!(combat.worn_armour([plate, shield], Style::Stab, 5), 21, "plate turns a stab by 1.2: 16.8, and the shield 4");
        assert_eq!(combat.dodge_in([plate, shield]).distance, 2.2);
        assert_eq!(combat.dodge_in([shield]).distance, 3.0);
    }

    fn sword() -> Timing {
        Timing { cooldown: 0.6, windup: 0.1, buffer: 0.15, beat: 0.1 }
    }

    #[test]
    fn early_clicks_are_held_for_the_beat_and_spam_is_ninety_percent() {
        let feel = Feel::default();
        let t = sword();
        let mut rhythm = Rhythm::default();
        let Click::Swing(first) = rhythm.click(10.0, &t, &feel) else { panic!() };
        assert_eq!((first.starts, first.strength, first.on_beat), (10.0, 1.0, false));
        assert!((first.lands - 10.1).abs() < 1e-6);
        assert_eq!(rhythm.click(10.05, &t, &feel), Click::Ignored, "one swing at a time");
        assert_eq!(rhythm.landed(10.05), None);
        assert_eq!(rhythm.landed(10.11), Some(first));
        assert_eq!(rhythm.click(10.2, &t, &feel), Click::Ignored, "sooner than the quickest");
        // In the buffer: held for the beat at 10.6, on it.
        let Click::Swing(held) = rhythm.click(10.5, &t, &feel) else { panic!() };
        assert!((held.starts - 10.6).abs() < 1e-6 && held.on_beat && held.strength == 1.0);
        rhythm.landed(10.75).unwrap();
        // Just after the beat (11.2) is on it; later is not.
        let Click::Swing(on) = rhythm.click(11.25, &t, &feel) else { panic!() };
        assert!(on.on_beat);
        rhythm.landed(12.0).unwrap();
        let Click::Swing(late) = rhythm.click(12.5, &t, &feel) else { panic!() };
        assert!(!late.on_beat && late.strength == 1.0);
        rhythm.landed(13.0).unwrap();
        // The quickest blow, at half the cooldown, is 45%; spamming it is 90% of timed damage
        // per second.
        let mut spam = Rhythm::default();
        spam.click(0.0, &t, &feel);
        spam.landed(1.0);
        let Click::Swing(quick) = spam.click(0.3, &t, &feel) else { panic!() };
        assert!((quick.strength - 0.45).abs() < 1e-6);
        assert!(((quick.strength / 0.3) / (1.0 / 0.6) - 0.9).abs() < 1e-4);
    }

    #[test]
    fn pips_build_to_a_flourish_and_cancelling_restarts_the_meter_from_half() {
        let feel = Feel::default();
        let t = sword();
        let mut rhythm = Rhythm::default();
        let swing = |on_beat| Swing { starts: 0.0, lands: 0.0, strength: 1.0, on_beat };
        let kinds: Vec<Blow> = (0..5).map(|i| rhythm.blow(&swing(true), i as f64 * 0.6, false, &feel)).collect();
        assert_eq!(kinds, [Blow::Beat, Blow::Beat, Blow::Beat, Blow::Flourish, Blow::Beat]);
        assert_eq!(rhythm.blow(&swing(false), 3.0, false, &feel), Blow::Plain);
        assert_eq!(rhythm.pips, 0, "off the beat loses them");
        rhythm.blow(&swing(true), 3.6, true, &feel);
        assert_eq!(rhythm.pips(3.6, &feel), 2, "a counter adds a pip");
        assert_eq!(rhythm.pips(6.0, &feel), 0, "two seconds after the last blow they go");
        // Guarding mid wind-up cancels it; the meter is then half full.
        let mut rhythm = Rhythm::default();
        rhythm.click(5.0, &t, &feel);
        assert!(rhythm.cancel(5.05, &t, &feel));
        assert!((rhythm.meter(5.05, &t) - 0.5).abs() < 1e-4);
        assert_eq!(rhythm.landed(9.0), None);
    }

    #[test]
    fn guards_stop_melee_in_their_arc_and_crush_breaks_through() {
        use glam::Vec3;
        let feel = Feel::default();
        let sword = GuardDef { stop: 0.35, arc: 90.0, perfect: 0.15, speed: 0.5, missiles: false };
        assert!(sword.covers(Vec3::Z, Vec3::new(0.9, 0.0, 1.0)));
        assert!(!sword.covers(Vec3::Z, Vec3::new(1.0, 0.0, 0.2)), "beside is open");
        assert_eq!(sword.stops(Style::Slash, &feel), 0.35);
        assert_eq!(sword.stops(Style::Crush, &feel), 0.175);
        assert_eq!((sword.stops(Style::Ranged, &feel), sword.stops(Style::Magic, &feel)), (0.0, 0.0));
        assert_eq!(GuardDef { missiles: true, ..sword }.stops(Style::Ranged, &feel), 0.35);
        let (combat, blocks) = registry(
            r#"(classes: [(name: "fist", style: Crush, cooldown: 0.6, windup: 0.08), (name: "sword", style: Slash, cooldown: 0.6, windup: 0.1, guard: (stop: 0.35, arc: 90.0, perfect: 0.15), flourish: Cleave(share: 0.5))],
                weapons: [(item: "sword", class: "sword", max_hit: 3)], feel: (pips: 4), dodges: [(weight: "heavy", distance: 2.2, time: 0.25, safe: (0.04, 0.18), cooldown: 1.5)])"#,
        )
        .unwrap();
        let sword = combat.wielded(blocks.id("t:sword"), |_| 1);
        let fists = combat.wielded(None, |_| 1);
        assert_eq!(combat.timing(&fists).windup, 0.08);
        assert_eq!(combat.guard(&fists), None);
        assert_eq!(combat.guard(&sword).map(|g| g.stop), Some(0.35));
        assert_eq!(combat.flourish(&sword), Some(Flourish::Cleave { share: 0.5 }));
        assert_eq!((combat.feel.pips, combat.feel.stagger), (4, Feel::default().stagger));
        assert_eq!(combat.dodge(Some("heavy")).distance, 2.2);
        assert_eq!(combat.dodge(None).weight, "heavy", "the first listed, without armour");
        assert!(registry(r#"(classes: [(name: "x", style: Slash, cooldown: 0.6, windup: 0.7)])"#).is_err());
    }
}
