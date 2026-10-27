//! Fighting: the three styles, weapons, and how hard and how often a blow lands
//! (docs/combat.md).
//!
//! **Rhythm** (the design wiki's "Combat feel"). A blow is a swing: it winds up, lands,
//! then recovers until the class's cooldown meter is full again: that moment is the beat.
//! A click in the buffer just before the beat is held and fires on it; one in the beat
//! window just after is on it too. On the beat, a blow gains accuracy and a rhythm pip;
//! with all the pips, the next on-beat blow is a flourish, with more max hit and its
//! class's effect (bleed, cleave, pin, sunder, daze). A click earlier than the buffer, but
//! past half the cooldown, fires at once at part strength: 45% at the quickest, rising in a
//! straight line to full at the cooldown, so spam-clicking deals about 90% of timed damage
//! per second and no click off the beat beats waiting for it (Mariano, 6 Oct)
//! ([`Feel::strength`], [`Rhythm`]).
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
//! the style, the cooldown and its reach; each weapon names its item, the level that
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
//! arrows too), and adds its armour, unless the weapon takes both hands. The heaviest armour
//! worn picks the dodge. Slot shares, what each weight is good against and what resisting
//! adds are content too (`combat.ron`'s `armour:`, [`ArmourRules`]).

use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

/// Seconds since the last blow for one at full strength, with fists or a sword.
pub const FULL_CHARGE: f32 = 0.6;
/// Whole damage from `full` scaled by `strength`, the fraction rounded up with chance equal
/// to it (drawn as `roll` in 0..1), so the average is exact even for small blows.
pub fn scaled(full: u16, strength: f32, roll: f32) -> u16 {
    by_chance(full as f32 * strength.clamp(0.0, 1.0), roll)
}

/// `exact` as a whole number, rounded up with chance equal to its fraction (drawn as `roll`
/// in 0..1): a bonus of a share is worth exactly that share on average, however small the
/// number it raises.
pub fn by_chance(exact: f32, roll: f32) -> u16 {
    let exact = exact.max(0.0);
    exact.floor() as u16 + u16::from(roll < exact.fract())
}

/// How a blow is struck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// Blocks it reaches from the eye to a creature's nearest edge (unset, [`Feel::reach`]).
    #[serde(default)]
    pub reach: Option<f32>,
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
    /// What it shoots, by item or `tag:` (a sling's stones, a bow's arrows): each shot takes
    /// one from the pack, and with none it cannot shoot. A class that shoots needs a clear
    /// line from the eye to what it is aimed at, where a melee blow needs only reach.
    #[serde(default)]
    pub ammo: Option<String>,
    /// The chance a shot that hits leaves what it shot lying by the target, to be picked up
    /// again (an arrow pulled from a kill, a stone in the grass): 0 to 1.
    #[serde(default)]
    pub recover: f32,
    /// The chance a shot that misses lands where it flew, to be picked up again: 0 to 1.
    #[serde(default)]
    pub recover_miss: f32,
    /// A hit taken while a shot is drawn spoils it, as raising a guard does
    /// (`Rhythm::cancel`): the shot is not loosed, and the meter starts again from
    /// [`Feel::cancel_to`].
    #[serde(default)]
    pub spoiled_by_hits: bool,
    /// The speed on foot while a shot is drawn (its wind-up), as a share: 0.1 to 1.
    #[serde(default = "one")]
    pub drawing_speed: f32,
}

fn one() -> f32 {
    1.0
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
    /// Whether, held up facing it, it braces against a charge: the charger runs onto the
    /// point, heavy or not, and is stopped (a spear's, the wiki's "stops a charge when
    /// braced"; `creature::moves::Charge`).
    #[serde(default)]
    pub brace: bool,
}

fn guard_speed() -> f32 {
    0.5
}

impl GuardDef {
    /// Whether, facing `facing`, it braces against a charge coming from `from`.
    pub fn braces(&self, facing: glam::Vec3, from: glam::Vec3) -> bool {
        self.brace && self.covers(facing, from)
    }

    /// Whether a blow from `from` falls in the arc of a guard facing `facing` (both flat
    /// directions, any length).
    pub fn covers(&self, facing: glam::Vec3, from: glam::Vec3) -> bool {
        let (a, b) = (facing.with_y(0.0).normalize_or_zero(), from.with_y(0.0).normalize_or_zero());
        if a == glam::Vec3::ZERO || b == glam::Vec3::ZERO {
            return true;
        }
        a.dot(b) >= (self.arc.to_radians() / 2.0).cos() - 1e-4
    }

    /// This guard, facing `facing`, if it can take a blow in `style` from `from`: not a
    /// heavy one, which goes through any guard, nor one from outside its arc, nor one in a
    /// style it stops none of (magic; missiles, but for a shield).
    pub fn takes(self, style: Style, heavy: bool, facing: glam::Vec3, from: glam::Vec3, feel: &Feel) -> Option<GuardDef> {
        (!heavy && self.covers(facing, from) && self.stops(style, feel) > 0.0).then_some(self)
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

/// Declares [`Feel`] with its defaults, and [`FeelPatch`], what one package's `feel` sets.
macro_rules! feel {
    ($($(#[$doc:meta])* $field:ident: $ty:ty = $default:expr,)*) => {
        /// How fighting feels beyond each class: every share, distance and timing of the
        /// rhythm, guard and creatures' attacks, as content sets them (a package's
        /// `combat.ron`; each field a package sets replaces an earlier one's, and what none
        /// sets keeps these defaults).
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Feel {
            $($(#[$doc])* pub $field: $ty,)*
        }

        impl Default for Feel {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }

        /// One package's `feel`: the fields it sets.
        #[derive(Clone, Copy, Debug, Default, Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub struct FeelPatch {
            $(pub $field: Option<$ty>,)*
        }

        impl Feel {
            /// `patch` laid over this: what it sets replaces, what it leaves out stays.
            pub fn patch(&mut self, patch: &FeelPatch) {
                $(if let Some(value) = patch.$field {
                    self.$field = value;
                })*
            }
        }
    };
}

feel! {
    /// A click this share of the cooldown after the last swing is the soonest that fires,
    /// at `spam_strength`; strength rises in a straight line to full at the cooldown.
    quickest: f32 = 0.5,
    spam_strength: f32 = 0.45,
    /// What a blow on the beat adds to the attack roll.
    beat_accuracy: f32 = 0.05,
    /// Rhythm pips: how many make a flourish, and the seconds after the last blow they go.
    pips: u8 = 3,
    pip_timeout: f32 = 2.0,
    /// What a flourish, a counter and an on-beat falling blow add to the max hit.
    flourish_max_hit: f32 = 0.2,
    counter_max_hit: f32 = 0.25,
    falling_max_hit: f32 = 0.15,
    /// What a blow on a creature that has not noticed the striker (asleep, looking away,
    /// stalked from behind) adds to the max hit; such a blow cannot miss. Not on a boss.
    unaware_max_hit: f32 = 0.5,
    /// Blocks below the top of a fall a player must be for a blow to be a falling one.
    falling_drop: f32 = 0.3,
    /// Guarding or dodging in your own wind-up cancels the swing, and the meter starts again
    /// from this share.
    cancel_to: f32 = 0.5,
    /// Of what a guard stops, the share crush blows put through anyway.
    crush_through: f32 = 0.5,
    /// A raise this soon after lowering opens no perfect window, so tapping does nothing.
    no_flicker: f32 = 0.4,
    /// Seconds a perfect block staggers the attacker (a boss), after its recovery, and
    /// seconds after it a blow is a counter.
    stagger: f32 = 0.8,
    boss_stagger: f32 = 0.3,
    counter: f32 = 1.0,
    /// Seconds after a perfect dodge a blow is a riposte (on the beat by itself).
    riposte: f32 = 0.8,
    /// Seconds early a dodge may arrive on the server's clock, for a client's clock running
    /// ahead of it.
    dodge_early: f32 = 0.1,
    /// Seconds after leaving the ground a dodge may still start (stepping off an edge as
    /// you press it).
    dodge_grace: f32 = 0.15,
    /// The most seconds of a player's round trip a guard or dodge is judged earlier by: a
    /// player sees a wind-up begin and their answer arrives that much after they made it.
    /// Measured for each player, never past this, so a slower connection earns no wider a
    /// window (as breaking is judged by when the client's message came, `actions`).
    lag_allowance: f32 = 0.2,
    /// Player reach, blocks from the eye to a creature's nearest edge, for classes that set
    /// none (the wiki's "Reach": Minecraft's 3, less a little), and how much further a blow
    /// may still land, for the time a view of the fight lags.
    reach: f32 = 2.8,
    reach_slack: f32 = 0.6,
    /// How far a player reaches to touch a creature (tend, look closely at or let go of
    /// it), whatever is in hand: Minecraft's 3 with some slack, and `reach_slack` on top.
    touch: f32 = 3.6,
    /// Seconds a player's view of a moving creature may run behind the server's: a blow at
    /// one moving also reaches as far as it goes in that time (counted once, on top of
    /// `reach_slack`).
    reach_lag: f32 = 0.15,
    /// How far in front a flourish's cleave finds a second creature: the least cosine
    /// between the look and the way to it (0.5: within 60 degrees either side).
    cleave_cone: f32 = 0.5,
    /// A creature's attack: seconds winding up and recovering, and blocks it lunges as the
    /// blow lands, unless it says.
    creature_windup: f32 = 0.5,
    creature_recovery: f32 = 0.6,
    creature_lunge: f32 = 1.2,
    /// The share of a wind-up a creature turns to follow its target and goes at it (at
    /// `tracking_pace`, unless it says: a run, so backing off at a walk does not escape it,
    /// the wiki's draft 65); after it, its aim is locked.
    creature_tracking: f32 = 0.6,
    tracking_pace: Pace = Pace::Run,
    /// What a creature adds to its attack roll, as a share of its level, unless it says
    /// (the wiki, draft 62: a level-70 creature hits a resisted Defence 70 heavy set about a
    /// third of the time, a bare target more than half).
    creature_accuracy: f32 = 0.8,
    /// How far, past touching, a dashing player stops short of a creature's body.
    creature_clearance: f32 = 0.2,
    /// The share of its most health a charging creature loses for each block it falls past
    /// survival's `safe_fall` (`creature::moves::Charge`): a charge does not look where it
    /// goes, so a cliff hurts it as the player would want.
    charge_fall: f32 = 0.1,
    /// Seconds after a creature and the player last traded blows that a right click at it
    /// still guards (with something that guards) rather than looks closely.
    engaged: f32 = 6.0,
}

/// How fast a creature goes: its species' walk or run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Pace {
    #[default]
    Walk,
    Run,
}

impl Feel {
    /// The share of its full damage a blow deals `since` seconds after the last swing
    /// started, with a weapon whose meter fills in `cooldown`: from `spam_strength` at the
    /// quickest in a straight line to full at the cooldown. Damage per second, strength over
    /// the time between blows, is then highest on the beat.
    pub fn strength(&self, since: f32, cooldown: f32) -> f32 {
        let quickest = cooldown * self.quickest;
        let t = ((since - quickest) / (cooldown - quickest).max(1e-3)).clamp(0.0, 1.0);
        self.spam_strength + (1.0 - self.spam_strength) * t
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
    /// On a creature that had not noticed the striker (`Stroke::unaware`): sure, and harder.
    Unaware,
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
                    Swing { starts: now, lands: 0.0, strength: feel.strength(since, timing.cooldown), on_beat: false }
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
        // A swing already due has landed, whether or not it was counted yet.
        if self.pending.is_none_or(|s| s.lands <= now) {
            return false;
        }
        self.pending = None;
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

    /// A swing landed at `now`: what kind of blow it is by the pips, which it moves on: a
    /// pip for one on the beat (a counter or riposte is on it by itself), none off it.
    pub fn blow(&mut self, swing: &Swing, now: f64, feel: &Feel) -> Blow {
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
        kind
    }

    /// The pips as the server counts them, told at `now`, for a client's mirror.
    pub fn set_pips(&mut self, pips: u8, now: f64) {
        self.pips = pips;
        self.last_blow = now;
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

/// How a blow is struck, beyond the weapon: from its swing and what made it special.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    /// Its strength, 0 to 1, for clicks earlier than the buffer.
    pub strength: f32,
    /// Added to the attack roll, and to the max hit, as shares.
    pub accuracy: f32,
    pub max_hit: f32,
    /// A counter cannot miss.
    pub certain: bool,
    pub blow: Blow,
}

impl Stroke {
    /// The stroke of a swing of `strength` that landed as `kind` by the pips: on the beat or
    /// not, a counter (after a perfect block) or a riposte (after a perfect dodge), struck
    /// falling. A counter or riposte is a full blow and on the beat; a counter cannot miss.
    pub fn of(kind: Blow, strength: f32, on_beat: bool, counter: bool, riposte: bool, falling: bool, feel: &Feel) -> Stroke {
        let on_beat = on_beat || counter || riposte;
        let flourish = kind == Blow::Flourish;
        let mut max_hit = 0.0;
        if flourish {
            max_hit += feel.flourish_max_hit;
        }
        if counter {
            max_hit += feel.counter_max_hit;
        }
        if on_beat && falling {
            max_hit += feel.falling_max_hit;
        }
        Stroke {
            strength: if counter || riposte { 1.0 } else { strength },
            accuracy: if on_beat { feel.beat_accuracy } else { 0.0 },
            max_hit,
            certain: counter,
            blow: if flourish { Blow::Flourish } else if counter { Blow::Counter } else { kind },
        }
    }

    /// This stroke landing on a creature that has not noticed the striker: it cannot miss,
    /// and hits up to `Feel::unaware_max_hit` harder. A plain or on-beat blow shows as an
    /// unaware one (`Blow::Unaware`); a flourish or counter keeps its own kind.
    pub fn unaware(self, feel: &Feel) -> Stroke {
        let blow = if matches!(self.blow, Blow::Plain | Blow::Beat) { Blow::Unaware } else { self.blow };
        Stroke { max_hit: self.max_hit + feel.unaware_max_hit, certain: true, blow, ..self }
    }

    /// The most this stroke's blow does with a weapon whose max hit is `max_hit`, the bonus
    /// rounded by chance (`roll` in 0..1), never under 1.
    pub fn max_hit(&self, max_hit: u16, roll: f32) -> u16 {
        by_chance(max_hit as f32 * (1.0 + self.max_hit), roll).max(1)
    }
}

/// What a flourish does to its target, worked out from the damage its blow did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// `points` more, one at a time, `every` seconds apart.
    Bleed { points: u16, every: f32 },
    /// A second creature in front takes `points`.
    Cleave { points: u16 },
    /// Slower by the share, for the seconds.
    Pin { slow: f32, seconds: f32 },
    /// Defence lower by the share, for the seconds.
    Sunder { defence: f32, seconds: f32 },
    /// Its wind-up undone, held still for the seconds.
    Daze { seconds: f32 },
}

impl Flourish {
    /// What this flourish does after a blow of `damage`, shares of it rounded by chance
    /// (`roll` in 0..1); nothing for a bleed or cleave that comes to no points.
    pub fn effect(&self, damage: u16, roll: f32) -> Option<Effect> {
        match *self {
            Flourish::Bleed { share, seconds } => {
                let points = by_chance(damage as f32 * share, roll);
                (points > 0).then(|| Effect::Bleed { points, every: (seconds / points as f32).max(0.05) })
            }
            Flourish::Cleave { share } => {
                let points = by_chance(damage as f32 * share, roll);
                (points > 0).then_some(Effect::Cleave { points })
            }
            Flourish::Pin { slow, seconds } => Some(Effect::Pin { slow: slow.clamp(0.0, 1.0), seconds }),
            Flourish::Sunder { defence, seconds } => Some(Effect::Sunder { defence: defence.clamp(0.0, 1.0), seconds }),
            Flourish::Daze { stagger } => Some(Effect::Daze { seconds: stagger }),
        }
    }
}

/// Which of `others` a cleave from `eye` looking along `look` reaches: the nearest within
/// `reach` of the eye whose middle lies within the cone (`cone`, the least cosine from the
/// look, flat), and that is wild (`kept` ones are never cleft). Each is `(id, box, kept)`.
pub fn cleave_target(eye: glam::Vec3, look: glam::Vec3, reach: f32, cone: f32, others: impl Iterator<Item = (u32, crate::physics::Aabb, bool)>) -> Option<u32> {
    let look = look.with_y(0.0).normalize_or_zero();
    others
        .filter(|&(_, _, kept)| !kept)
        .filter_map(|(id, aabb, _)| {
            let d = eye.distance(eye.clamp(aabb.min, aabb.max));
            let to = ((aabb.min + aabb.max) / 2.0 - eye).with_y(0.0).normalize_or_zero();
            (d <= reach && to.dot(look) > cone).then_some((id, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// A player's guard and dodges over time, on one clock, so a creature's blow is judged by
/// the moment it lands, not by when the server gets to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guarding {
    /// When the guard went up, while it is up, and whether that raise may be perfect.
    up: Option<(f64, bool)>,
    /// The last raise that came down: when it went up and down, and whether it could be
    /// perfect.
    last: Option<(f64, f64, bool)>,
    /// When blows pass through the latest dodge, and when the next may start.
    dodging: (f64, f64),
    dodge_ready: f64,
}

impl Guarding {
    pub fn is_up(&self) -> bool {
        self.up.is_some()
    }

    /// The guard goes up at `now`; a raise sooner than [`Feel::no_flicker`] after the last
    /// came down opens no perfect window. Returns whether it went up (it was down).
    pub fn raise(&mut self, now: f64, feel: &Feel) -> bool {
        if self.up.is_some() {
            return false;
        }
        let perfect = self.last.is_none_or(|(_, down, _)| now - down >= feel.no_flicker as f64);
        self.up = Some((now, perfect));
        true
    }

    /// The guard comes down at `now`. Returns whether it was up.
    pub fn lower(&mut self, now: f64) -> bool {
        let Some((up, perfect)) = self.up.take() else { return false };
        // Never down before up, for a round trip measured longer between the two.
        self.last = Some((up, now.max(up), perfect));
        true
    }

    /// A dodge at `now`, if one may start: not with the guard up, nor before the last one's
    /// cooldown is out (less [`Feel::dodge_early`], for a client's clock ahead of this one).
    pub fn dodge(&mut self, now: f64, dodge: &DodgeDef, feel: &Feel) -> bool {
        if self.up.is_some() || now + (feel.dodge_early as f64) < self.dodge_ready {
            return false;
        }
        self.dodging = (now + dodge.safe.0 as f64, now + dodge.safe.1 as f64);
        self.dodge_ready = now + dodge.cooldown as f64;
        true
    }

    /// What a blow landing at `lands` meets: a dodge it passes through, a guard that takes
    /// it (perfectly, raised within its window before), or nothing. `guard` is the guard
    /// that could take it, if any: see [`GuardDef::takes`].
    pub fn meets(&self, lands: f64, guard: Option<&GuardDef>) -> Defended {
        if self.dodging.0 <= lands && lands <= self.dodging.1 {
            return Defended::Dodged;
        }
        let Some(guard) = guard else { return Defended::Hit };
        // The raise up as it landed: the one still up, or the last, if it came down after.
        let raise = match (self.up, self.last) {
            (Some((up, perfect)), _) if up <= lands => Some((up, perfect)),
            (_, Some((up, down, perfect))) if up <= lands && lands <= down => Some((up, perfect)),
            _ => None,
        };
        match raise {
            Some((up, true)) if lands - up <= guard.perfect as f64 => Defended::Perfect,
            Some(_) => Defended::Blocked,
            None => Defended::Hit,
        }
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
    pub feel: Option<FeelPatch>,
    #[serde(default)]
    pub dodges: Vec<DodgeDef>,
    #[serde(default)]
    pub sets: Vec<SetDef>,
    #[serde(default)]
    pub shields: Vec<ShieldDef>,
    #[serde(default)]
    pub armour: Option<ArmourFile>,
}

/// Where a piece of armour is worn: the inventory's worn slots, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Slot {
    Head,
    Body,
    Legs,
    Hands,
    Feet,
    /// The off-hand: a shield, which guards in place of the weapon.
    Shield,
    /// One charm: a rare keepsake whose effect holds while it is worn (`effect::Condition::Wearing`).
    Charm,
}

impl Slot {
    pub const ALL: [Slot; 7] = [Slot::Head, Slot::Body, Slot::Legs, Slot::Hands, Slot::Feet, Slot::Shield, Slot::Charm];

    /// Its worn slot, counted from the first (`inventory::WORN`).
    pub fn index(self) -> usize {
        self as usize
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
            Slot::Charm => "charm",
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

/// How armour counts, as content sets it (`combat.ron`'s `armour:`; each package's sets
/// what it names, slot by slot and weight by weight).
#[derive(Clone, Debug, PartialEq)]
pub struct ArmourRules {
    /// The armour a piece in each slot gives from a set of armour 1.0 (bronze): the wiki's
    /// 6, 14, 10, 3 and 3, so a whole set gives 36, as RuneScape's bronze does about.
    pub slots: std::collections::BTreeMap<Slot, f32>,
    /// Each weight, lightest first: the share of its armour that counts against each style
    /// (1 unless it says), and the dodge it wears (`dodges:`). The heaviest worn picks the
    /// dodge.
    pub weights: Vec<WeightDef>,
    /// How much more armour counts against a style its set resists.
    pub resisted: f32,
}

/// A weight of armour as content defines it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WeightDef {
    pub weight: Weight,
    #[serde(default)]
    pub against: std::collections::BTreeMap<Style, f32>,
    pub dodge: String,
}

/// One package's `armour:`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArmourFile {
    pub slots: std::collections::BTreeMap<Slot, f32>,
    pub weights: Vec<WeightDef>,
    pub resisted: Option<f32>,
}

impl Default for ArmourRules {
    /// The wiki's (drafts 59 and 62), which `combat.ron` restates.
    fn default() -> Self {
        use Style::*;
        let all = |share: f32| [Stab, Slash, Crush, Ranged, Magic].map(|s| (s, share)).into_iter().collect::<std::collections::BTreeMap<_, _>>();
        let with = |mut base: std::collections::BTreeMap<Style, f32>, more: &[(Style, f32)]| {
            base.extend(more.iter().copied());
            base
        };
        let weights = vec![
            WeightDef { weight: Weight::Robe, against: with(all(0.5), &[(Ranged, 0.7), (Magic, 1.5)]), dodge: "light".into() },
            WeightDef { weight: Weight::Light, against: with(Default::default(), &[(Crush, 0.8)]), dodge: "light".into() },
            WeightDef { weight: Weight::Medium, against: Default::default(), dodge: "medium".into() },
            WeightDef { weight: Weight::Heavy, against: with(all(1.2), &[(Magic, 0.6)]), dodge: "heavy".into() },
        ];
        let slots = [(Slot::Head, 6.0), (Slot::Body, 14.0), (Slot::Legs, 10.0), (Slot::Hands, 3.0), (Slot::Feet, 3.0)].into_iter().collect();
        Self { slots, weights, resisted: 1.5 }
    }
}

impl ArmourRules {
    fn patch(&mut self, file: ArmourFile) {
        self.slots.extend(file.slots);
        for def in file.weights {
            match self.weights.iter_mut().find(|w| w.weight == def.weight) {
                Some(earlier) => *earlier = def,
                None => self.weights.push(def),
            }
        }
        self.resisted = file.resisted.unwrap_or(self.resisted);
    }

    /// The armour a piece in `slot` gives from a set of armour 1.0.
    pub fn slot(&self, slot: Slot) -> f32 {
        self.slots.get(&slot).copied().unwrap_or(0.0)
    }

    /// The share of armour of `weight` that counts against a blow of `style`.
    pub fn against(&self, weight: Weight, style: Style) -> f32 {
        self.weight(weight).and_then(|w| w.against.get(&style).copied()).unwrap_or(1.0)
    }

    fn weight(&self, weight: Weight) -> Option<&WeightDef> {
        self.weights.iter().find(|w| w.weight == weight)
    }

    /// How heavy `weight` ranks: its place in the list, lightest first.
    fn rank(&self, weight: Weight) -> usize {
        self.weights.iter().position(|w| w.weight == weight).unwrap_or(0)
    }
}

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
/// Bare fists' reach without a fist class (the wiki's "Reach": a fist or dagger 2.5).
pub const FIST_REACH: f32 = 2.5;

/// What a blow is struck with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weapon {
    /// Its class, by index (`None` for fists).
    pub class: Option<usize>,
    pub style: Style,
    pub cooldown: f32,
    /// Blocks it reaches from the eye to a creature's nearest edge.
    pub reach: f32,
    pub skill: Option<SkillId>,
    pub level: u8,
    pub max_hit: u16,
    pub accuracy: u16,
}

impl Weapon {
    /// A bare fist: crushing, a sword's speed, a max hit of 1 that rises with Attack.
    pub const FISTS: Weapon = Weapon { class: None, style: Style::Crush, cooldown: FULL_CHARGE, reach: FIST_REACH, skill: None, level: 0, max_hit: 1, accuracy: 0 };

    /// The most a blow does for a striker at `level` in the weapon's skill: one more for
    /// every eight levels above the weapon's.
    pub fn max_hit(&self, level: u8) -> u16 {
        self.max_hit + (level.saturating_sub(self.level) / 8) as u16
    }

}

/// Every class and weapon, with each item's weapon worked out once.
#[derive(Clone, Debug, Default)]
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
    pub armour: ArmourRules,
    /// Each class's ammunition, by index: the items it shoots, none for a class that strikes.
    ammo: Vec<Vec<BlockId>>,
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
        for ammo in file.classes.iter_mut().filter_map(|class| class.ammo.as_mut()) {
            if !ammo.contains(':') {
                *ammo = format!("{namespace}:{ammo}");
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<CombatFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let (mut classes, mut defs, mut feel, mut dodges, mut sets, mut shield_defs) = (Vec::new(), Vec::new(), Feel::default(), Vec::<DodgeDef>::new(), Vec::new(), Vec::new());
        let mut armour = ArmourRules::default();
        for file in files {
            classes.extend(file.classes);
            defs.extend(file.weapons);
            sets.extend(file.sets);
            shield_defs.extend(file.shields);
            if let Some(patch) = &file.feel {
                feel.patch(patch);
            }
            if let Some(patch) = file.armour {
                armour.patch(patch);
            }
            for dodge in file.dodges {
                // A later package's dodge for a weight replaces an earlier one's, in its place.
                match dodges.iter_mut().find(|d| d.weight == dodge.weight) {
                    Some(earlier) => *earlier = dodge,
                    None => dodges.push(dodge),
                }
            }
        }
        if dodges.is_empty() {
            dodges.push(DodgeDef::default());
        }
        if !(0.0..=1.0).contains(&feel.charge_fall) {
            return Err(RegistryError("combat.ron: `feel.charge_fall` must be from 0 to 1".into()));
        }
        if !(0.0..=1.5).contains(&feel.unaware_max_hit) {
            return Err(RegistryError("combat.ron: `feel.unaware_max_hit` must be from 0 to 1.5".into()));
        }
        if let Some(class) = classes.iter().find(|c| c.windup < 0.0 || c.windup >= c.cooldown || c.buffer < 0.0 || c.beat < 0.0) {
            return Err(RegistryError(format!("combat.ron: class `{}`: its wind-up must be under its cooldown, and no time may be negative", class.name)));
        }
        let rules = &armour;
        let mut pieces = vec![None; blocks.len()];
        for (index, set) in sets.iter().enumerate() {
            for (slot, name) in &set.pieces {
                let error = |why: &str| RegistryError(format!("combat.ron: set `{}`: {name}: {why}", set.name));
                let item = blocks.id(name).ok_or_else(|| error("no such item"))?;
                if *slot == Slot::Charm {
                    return Err(error("the charm slot holds a charm, never a piece of a set"));
                }
                if pieces[item.0 as usize].is_some() {
                    return Err(error("listed twice"));
                }
                let armour = (rules.slot(*slot) * set.armour).round().max(1.0) as u16;
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
                reach: class.reach.unwrap_or(feel.reach),
                skill,
                level: def.level,
                max_hit: def.max_hit.max(1),
                accuracy: def.accuracy.unwrap_or(def.max_hit * 2),
            });
        }
        let fist = classes.iter().position(|c| c.name == FIST_CLASS);
        let mut ammo = Vec::with_capacity(classes.len());
        for class in &classes {
            let items: Vec<BlockId> = match class.ammo.as_deref() {
                None => Vec::new(),
                Some(name) => match name.split_once(':').filter(|(namespace, _)| *namespace == "tag") {
                    Some((_, tag)) => blocks.tagged(tag).collect(),
                    None => blocks.id(name).into_iter().collect(),
                },
            };
            if !(0.0..=1.0).contains(&class.recover) || (class.recover > 0.0 && class.ammo.is_none()) {
                return Err(RegistryError(format!("combat.ron: class `{}` recovers {} of its shots: it must shoot something, and recover 0 to 1", class.name, class.recover)));
            }
            if !(0.0..=1.0).contains(&class.recover_miss) || (class.recover_miss > 0.0 && class.ammo.is_none()) {
                return Err(RegistryError(format!("combat.ron: class `{}` recovers {} of its misses: it must shoot something, and recover_miss 0 to 1", class.name, class.recover_miss)));
            }
            if class.spoiled_by_hits && class.ammo.is_none() {
                return Err(RegistryError(format!("combat.ron: class `{}` has its shots spoiled by hits, but shoots nothing", class.name)));
            }
            if !(0.1..=1.0).contains(&class.drawing_speed) || (class.drawing_speed < 1.0 && class.ammo.is_none()) {
                return Err(RegistryError(format!("combat.ron: class `{}` moves at {} while drawing: it must shoot something, and drawing_speed 0.1 to 1", class.name, class.drawing_speed)));
            }
            if class.ammo.is_some() && items.is_empty() {
                return Err(RegistryError(format!("combat.ron: class `{}` shoots {:?}, which no item is", class.name, class.ammo.as_deref().unwrap_or_default())));
            }
            ammo.push(items);
        }
        Ok(Self { classes, weapons, attack: skills.id("core:attack"), defence: skills.id("core:defence"), feel, fist, dodges, sets, shields, pieces, armour, ammo })
    }

    /// What `item` is as a weapon, if it is one.
    pub fn weapon(&self, item: BlockId) -> Option<&Weapon> {
        self.weapons.get(item.0 as usize).and_then(Option::as_ref)
    }

    /// What a blow is struck with, holding `item`, for a striker whose own level in a skill
    /// is `level(skill)`: the weapon, or fists when nothing is held, the item is no weapon
    /// or the striker's level is below it. Boosts never help to wield a weapon.
    pub fn wielded(&self, item: Option<BlockId>, level: impl Fn(SkillId) -> u8) -> Weapon {
        let fist = self.fist.map(|i| &self.classes[i]);
        let cooldown = fist.map_or(Weapon::FISTS.cooldown, |c| c.cooldown);
        let reach = fist.map_or(Weapon::FISTS.reach, |c| c.reach.unwrap_or(self.feel.reach));
        let fists = Weapon { skill: self.attack, cooldown, reach, ..Weapon::FISTS };
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

    /// The Defence level that wears `item`, if it is armour or a shield.
    pub fn wear_level(&self, item: BlockId) -> Option<u8> {
        self.piece(item).map(|p| p.level).or(self.shield(item).map(|s| s.level))
    }

    /// Each block id's worn slot, if it is armour or a shield (`inventory::set_worn_slots`).
    pub fn worn_slots(&self) -> Vec<Option<u8>> {
        let shield = Slot::Shield.index() as u8;
        self.pieces.iter().zip(&self.shields).map(|(p, s)| p.map(|p| p.slot.index() as u8).or(s.map(|_| shield))).collect()
    }

    /// What `piece` adds to the defence roll against a blow of `style`.
    pub fn armour_against(&self, piece: &Piece, style: Style) -> f32 {
        let resisted = if self.sets[piece.set].resists.contains(&style) { self.armour.resisted } else { 1.0 };
        piece.armour as f32 * self.armour.against(piece.weight, style) * resisted
    }

    /// The armour `worn` gives against a blow of `style`, for a wearer whose own Defence
    /// level is `level`: pieces worn below their level give nothing, and a shield nothing
    /// beside a weapon in both hands (`two_handed`), which it cannot guard with.
    pub fn worn_armour(&self, worn: impl IntoIterator<Item = BlockId>, style: Style, level: u8, two_handed: bool) -> u16 {
        let total: f32 = worn
            .into_iter()
            .map(|item| match (self.piece(item), self.shield(item)) {
                (Some(p), _) if level >= p.level => self.armour_against(p, style),
                (_, Some(s)) if level >= s.level && !two_handed => s.armour as f32,
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
    /// the wearer's own levels, holding `held`.
    pub fn armour_of(&self, held: Option<BlockId>, worn: &[Option<crate::inventory::ItemStack>], style: Style, level: impl Fn(SkillId) -> u8) -> u16 {
        let two_handed = self.class_of(&self.wielded(held, &level)).is_some_and(|c| c.two_handed);
        self.worn_armour(worn.iter().flatten().map(|s| s.item), style, self.defence.map_or(1, level), two_handed)
    }

    /// The dodge in `worn` (the inventory's worn slots).
    pub fn dodge_of(&self, worn: &[Option<crate::inventory::ItemStack>]) -> &DodgeDef {
        self.dodge_in(worn.iter().flatten().map(|s| s.item))
    }

    /// The heaviest weight among `worn` (by the armour rules' order), if any of it is
    /// armour. Pieces worn below their level still count: they keep their weight.
    pub fn heaviest(&self, worn: impl IntoIterator<Item = BlockId>) -> Option<Weight> {
        worn.into_iter().filter_map(|item| self.piece(item)).map(|p| p.weight).max_by_key(|&w| self.armour.rank(w))
    }

    /// The dodge in `worn`: the one its heaviest weight wears (robes, light armour's).
    pub fn dodge_in(&self, worn: impl IntoIterator<Item = BlockId>) -> &DodgeDef {
        self.dodge(self.heaviest(worn).and_then(|w| self.armour.weight(w)).map(|w| w.dodge.as_str()))
    }

    /// The class `weapon` fights as: its own, or for fists the fist class, if any.
    pub fn class_of(&self, weapon: &Weapon) -> Option<&ClassDef> {
        weapon.class.or(self.fist).map(|i| &self.classes[i])
    }

    /// What `weapon` shoots: the items a shot takes one of, empty for a weapon that strikes.
    pub fn ammo(&self, weapon: &Weapon) -> &[BlockId] {
        weapon.class.and_then(|i| self.ammo.get(i)).map_or(&[], Vec::as_slice)
    }

    /// The chance a hit by `weapon` leaves its shot to be picked up again (0 for a weapon
    /// that strikes).
    pub fn recover(&self, weapon: &Weapon) -> f32 {
        self.class_of(weapon).map_or(0.0, |class| class.recover)
    }

    /// The chance a miss by `weapon` lands to be picked up again (0 for a weapon that
    /// strikes).
    pub fn recover_miss(&self, weapon: &Weapon) -> f32 {
        self.class_of(weapon).map_or(0.0, |class| class.recover_miss)
    }

    /// Whether a hit taken spoils `weapon`'s shot being drawn.
    pub fn spoiled_by_hits(&self, weapon: &Weapon) -> bool {
        self.class_of(weapon).is_some_and(|class| class.spoiled_by_hits)
    }

    /// The share of their speed a player keeps while drawing a shot with `weapon` (1 for
    /// a weapon that strikes).
    pub fn drawing_speed(&self, weapon: &Weapon) -> f32 {
        self.class_of(weapon).map_or(1.0, |class| class.drawing_speed)
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

    /// Every weapon class.
    pub fn classes(&self) -> &[ClassDef] {
        &self.classes
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
        // Every class in combat.ron, and a slow one: clicking every `t` seconds, damage per
        // second is the swing's strength over the time from one swing's start to the next.
        let feel = Feel::default();
        for (cooldown, buffer) in [(0.6, 0.15), (0.4, 0.12), (0.73, 0.15), (0.8, 0.18), (0.87, 0.18), (1.2, 0.18)] {
            let timing = Timing { cooldown, windup: 0.05, buffer, beat: 0.1 };
            let dps = |t: f32| {
                let mut rhythm = Rhythm::default();
                rhythm.click(0.0, &timing, &feel);
                rhythm.landed(0.06);
                let Click::Swing(swing) = rhythm.click(t as f64, &timing, &feel) else { panic!("{cooldown} {t}") };
                swing.strength / swing.starts as f32
            };
            let timed = 1.0 / cooldown;
            assert!((dps(cooldown) - timed).abs() < 1e-4);
            assert!((dps(cooldown / 2.0) / timed - 0.9).abs() < 1e-3, "{cooldown}: spam is 90% of timed");
            for share in [0.58, 0.67, 0.75, 0.83, 0.92, 0.99] {
                let t = cooldown * share;
                let held = cooldown - t <= buffer;
                assert!(dps(t) > 0.9 * timed - 1e-4, "{cooldown} {t}");
                assert!(if held { (dps(t) - timed).abs() < 1e-4 } else { dps(t) < timed }, "{cooldown} {t}: no click off the beat beats it");
            }
            assert!(dps(cooldown * 2.0) < timed, "waiting past the beat only loses damage per second");
        }
        assert_eq!(feel.strength(0.6, 0.6), 1.0);
        assert!((feel.strength(0.3, 0.6) - 0.45).abs() < 1e-6);
    }

    #[test]
    fn bonuses_round_by_chance_so_small_ones_count() {
        let mean = |exact: f32| (0..1000).map(|i| by_chance(exact, i as f32 / 1000.0) as f32).sum::<f32>() / 1000.0;
        // A flourish's +20% on a max hit of 3, which rounding would lose: 3.6 on average.
        assert!((mean(3.0 * 1.2) - 3.6).abs() < 0.01);
        assert!((mean(0.25) - 0.25).abs() < 0.01);
        assert_eq!(by_chance(-1.0, 0.0), 0);
    }

    #[test]
    fn a_blow_is_judged_by_when_it_lands() {
        use glam::Vec3;
        let feel = Feel::default();
        let sword = GuardDef { stop: 0.35, arc: 90.0, perfect: 0.15, speed: 0.5, missiles: false, brace: false };
        let dodge = DodgeDef::default();
        // Raised 0.1 s before it lands: perfect; 0.2 s before: a block; after: nothing.
        for (raised, lands, want) in [(10.0, 10.1, Defended::Perfect), (10.0, 10.2, Defended::Blocked), (10.2, 10.1, Defended::Hit)] {
            let mut g = Guarding::default();
            g.raise(raised, &feel);
            assert_eq!(g.meets(lands, Some(&sword)), want, "raised {raised}, lands {lands}");
        }
        // Lowered after it landed (before the server got to it): the guard still took it.
        let mut g = Guarding::default();
        g.raise(10.0, &feel);
        g.lower(10.3);
        assert_eq!(g.meets(10.25, Some(&sword)), Defended::Blocked);
        assert_eq!(g.meets(10.35, Some(&sword)), Defended::Hit, "lowered before it landed");
        // Raised again within 0.4 s of lowering: no perfect window, so tapping does nothing.
        g.raise(10.6, &feel);
        assert_eq!(g.meets(10.65, Some(&sword)), Defended::Blocked);
        g.lower(10.7);
        g.raise(11.2, &feel);
        assert_eq!(g.meets(11.25, Some(&sword)), Defended::Perfect, "0.5 s after lowering");
        assert_eq!(g.meets(11.25, None), Defended::Hit, "nothing that guards it");
        // A dodge: blows pass through from 0.04 to 0.22 s after it starts, judged at landing.
        let mut g = Guarding::default();
        assert!(g.dodge(20.0, &dodge, &feel));
        assert_eq!(g.meets(20.1, Some(&sword)), Defended::Dodged);
        assert_eq!(g.meets(20.03, None), Defended::Hit);
        assert_eq!(g.meets(20.23, None), Defended::Hit);
        // The next waits out its cooldown (1 s), less 0.1 s for a client's clock ahead.
        assert!(!g.dodge(20.85, &dodge, &feel));
        assert!(g.dodge(20.92, &dodge, &feel));
        g.raise(30.0, &feel);
        assert!(!g.dodge(35.0, &dodge, &feel), "not with the guard up");
        // What a guard takes: 35% of a slash in its arc, half that of a crush; no magic, no
        // heavy blow, nothing from beside; missiles only with a shield.
        let ahead = Vec3::Z;
        assert_eq!(sword.takes(Style::Slash, false, Vec3::Z, ahead, &feel).map(|g| g.stops(Style::Slash, &feel)), Some(0.35));
        assert_eq!(sword.takes(Style::Crush, false, Vec3::Z, ahead, &feel).map(|g| g.stops(Style::Crush, &feel)), Some(0.175));
        assert_eq!(sword.takes(Style::Magic, false, Vec3::Z, ahead, &feel), None);
        assert_eq!(sword.takes(Style::Slash, true, Vec3::Z, ahead, &feel), None, "a heavy blow goes through");
        assert_eq!(sword.takes(Style::Slash, false, Vec3::Z, Vec3::X, &feel), None, "from beside");
        assert_eq!(sword.takes(Style::Ranged, false, Vec3::Z, ahead, &feel), None);
        assert!(GuardDef { missiles: true, ..sword }.takes(Style::Ranged, false, Vec3::Z, ahead, &feel).is_some());
    }

    #[test]
    fn packages_patch_the_feel_field_by_field_and_dodges_in_place() {
        use crate::block::BlockDef;
        let blocks = BlockRegistry::new(vec![BlockDef { name: "t:air".into(), ..Default::default() }]).unwrap();
        let skills = SkillRegistry::default();
        let files = [
            r#"(feel: (pips: 4, reach: 3.0), dodges: [(weight: "light", distance: 3.0, time: 0.25, safe: (0.04, 0.22), cooldown: 1.0), (weight: "heavy", distance: 2.2, time: 0.25, safe: (0.04, 0.18), cooldown: 1.5)])"#,
            r#"(feel: (stagger: 1.0), dodges: [(weight: "light", distance: 3.5, time: 0.25, safe: (0.04, 0.22), cooldown: 1.0)])"#,
        ];
        let files = files.iter().map(|f| CombatRegistry::read("t", f).unwrap()).collect();
        let combat = CombatRegistry::new(files, &blocks, &skills).unwrap();
        assert_eq!((combat.feel.pips, combat.feel.reach, combat.feel.stagger), (4, 3.0, 1.0), "the later package keeps what it leaves out");
        assert_eq!(combat.feel.riposte, Feel::default().riposte);
        assert_eq!(combat.dodge(None).distance, 3.5, "the light dodge replaced, still first");
        assert_eq!(combat.dodge(Some("heavy")).distance, 2.2);
    }

    #[test]
    fn counters_ripostes_flourishes_and_falling_blows_strike_as_the_feel_says() {
        let feel = Feel::default();
        // A counter: full strength whatever the meter, on the beat, +25% max hit, sure.
        let counter = Stroke::of(Blow::Beat, 0.45, false, true, false, false, &feel);
        assert_eq!((counter.strength, counter.accuracy, counter.max_hit, counter.certain, counter.blow), (1.0, 0.05, 0.25, true, Blow::Counter));
        // Its max hit with an iron sword's 9: 11.25, so 11 or 12, 11.25 on average.
        let mean = (0..1000).map(|i| counter.max_hit(9, i as f32 / 1000.0) as f32).sum::<f32>() / 1000.0;
        assert!((mean - 11.25).abs() < 0.01, "{mean}");
        // A riposte: full and on the beat, but it can miss and adds no max hit.
        let riposte = Stroke::of(Blow::Beat, 0.5, false, false, true, false, &feel);
        assert_eq!((riposte.strength, riposte.max_hit, riposte.certain), (1.0, 0.0, false));
        // A flourish, struck falling on the beat: +20% and +15%.
        let flourish = Stroke::of(Blow::Flourish, 1.0, true, false, false, true, &feel);
        assert!((flourish.max_hit - 0.35).abs() < 1e-6 && flourish.blow == Blow::Flourish);
        // Off the beat: no accuracy, and falling adds nothing.
        let plain = Stroke::of(Blow::Plain, 0.7, false, false, false, true, &feel);
        assert_eq!((plain.strength, plain.accuracy, plain.max_hit), (0.7, 0.0, 0.0));
        assert_eq!(plain.max_hit(1, 0.0), 1);
    }

    /// A blow on a creature that has not noticed the striker cannot miss and adds the
    /// feel's `unaware_max_hit` to what else it adds; out of range, the feel is refused.
    #[test]
    fn an_unaware_target_is_struck_surely_and_harder() {
        let feel = Feel::default();
        let plain = Stroke::of(Blow::Plain, 1.0, false, false, false, false, &feel).unaware(&feel);
        assert_eq!((plain.certain, plain.max_hit, plain.blow), (true, 0.5, Blow::Unaware));
        let beat = Stroke::of(Blow::Beat, 1.0, true, false, false, false, &feel).unaware(&feel);
        assert_eq!(beat.blow, Blow::Unaware);
        let flourish = Stroke::of(Blow::Flourish, 1.0, false, false, false, false, &feel).unaware(&feel);
        assert!((flourish.max_hit - 0.7).abs() < 1e-6 && flourish.certain && flourish.blow == Blow::Flourish, "{flourish:?}");
        // With a sword's 9: 13.5 at most, on average.
        let mean = (0..1000).map(|i| plain.max_hit(9, i as f32 / 1000.0) as f32).sum::<f32>() / 1000.0;
        assert!((mean - 13.5).abs() < 0.01, "{mean}");
        assert!(registry("(feel: (unaware_max_hit: 1.5))").is_ok());
        for bad in ["-0.1", "1.6"] {
            assert!(registry(&format!("(feel: (unaware_max_hit: {bad}))")).is_err_and(|e| e.to_string().contains("unaware_max_hit")), "{bad}");
        }
    }

    #[test]
    fn each_flourish_does_what_its_class_says() {
        // combat.ron's: dagger, sword, spear, axe, mace.
        assert_eq!(Flourish::Bleed { share: 0.25, seconds: 3.0 }.effect(8, 0.5), Some(Effect::Bleed { points: 2, every: 1.5 }));
        assert_eq!(Flourish::Bleed { share: 0.25, seconds: 3.0 }.effect(2, 0.9), None, "half a point, rounded down this time");
        assert_eq!(Flourish::Bleed { share: 0.25, seconds: 3.0 }.effect(2, 0.1), Some(Effect::Bleed { points: 1, every: 3.0 }));
        assert_eq!(Flourish::Cleave { share: 0.5 }.effect(9, 0.6), Some(Effect::Cleave { points: 4 }));
        assert_eq!(Flourish::Cleave { share: 0.5 }.effect(9, 0.4), Some(Effect::Cleave { points: 5 }));
        assert_eq!(Flourish::Pin { slow: 0.4, seconds: 1.5 }.effect(1, 0.0), Some(Effect::Pin { slow: 0.4, seconds: 1.5 }));
        assert_eq!(Flourish::Sunder { defence: 0.1, seconds: 6.0 }.effect(1, 0.0), Some(Effect::Sunder { defence: 0.1, seconds: 6.0 }));
        assert_eq!(Flourish::Daze { stagger: 0.6 }.effect(1, 0.0), Some(Effect::Daze { seconds: 0.6 }));
        // A cleave finds the nearest wild creature in front, within reach.
        use crate::physics::Aabb;
        let eye = glam::Vec3::new(0.0, 1.6, 0.0);
        let at = |x: f32, z: f32| Aabb::standing(glam::Vec3::new(x, 0.0, z), 1.0, 1.0);
        let others = [(1, at(0.0, -2.0), true), (2, at(1.0, -2.5), false), (3, at(0.0, 2.0), false), (4, at(0.0, -6.0), false)];
        assert_eq!(cleave_target(eye, glam::Vec3::NEG_Z, 2.8, 0.5, others.into_iter()), Some(2), "not the kept one, nor behind, nor out of reach");
        assert_eq!(cleave_target(eye, glam::Vec3::NEG_Z, 2.8, 0.5, others.into_iter().filter(|o| o.0 != 2)), None);
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
                classes: [(name: "sword", style: Slash, cooldown: 0.6), (name: "spear", style: Stab, cooldown: 0.73, reach: 3.8)],
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
        assert_eq!((spear.reach, spear.accuracy, spear.cooldown), (3.8, 10, 0.73));
        assert_eq!(combat.wielded(Some(sword), at(5)).reach, Feel::default().reach, "a class without a reach has the feel's");
        assert_eq!(combat.wielded(None, at(1)).reach, FIST_REACH, "fists without a fist class");
        assert!(registry(r#"(weapons: [(item: "nothing", class: "sword", max_hit: 1)])"#).is_err());
        assert!(registry(r#"(classes: [(name: "sword", style: Slash, cooldown: 0.6)], weapons: [(item: "sword", class: "axe", max_hit: 1)])"#).is_err());
    }

    #[test]
    fn a_shooting_class_names_real_ammo_and_recovers_a_share_of_its_shots() {
        let (combat, blocks) = registry(r#"(classes: [(name: "sling", style: Ranged, cooldown: 0.5, ammo: "stone", recover: 0.25)], weapons: [(item: "stick", class: "sling", max_hit: 2)])"#).unwrap();
        let sling = combat.wielded(blocks.id("t:stick"), |_| 1);
        assert_eq!(combat.ammo(&sling), [blocks.id("t:stone").unwrap()]);
        assert_eq!(combat.recover(&sling), 0.25);
        assert_eq!(combat.recover(&combat.wielded(None, |_| 1)), 0.0, "fists recover nothing");
        assert!(registry(r#"(classes: [(name: "sling", style: Ranged, cooldown: 0.5, ammo: "nothing")])"#).is_err(), "ammo that is no item");
        assert!(registry(r#"(classes: [(name: "sling", style: Ranged, cooldown: 0.5, ammo: "stone", recover: 1.5)])"#).is_err(), "recover above 1");
        assert!(registry(r#"(classes: [(name: "sword", style: Slash, cooldown: 0.6, recover: 0.5)])"#).is_err(), "recovering with nothing to shoot");
        let (combat, blocks) = registry(r#"(classes: [(name: "bow", style: Ranged, cooldown: 1.0, windup: 0.5, ammo: "stone", recover_miss: 0.8, spoiled_by_hits: true, drawing_speed: 0.4)], weapons: [(item: "stick", class: "bow", max_hit: 2)])"#).unwrap();
        let bow = combat.wielded(blocks.id("t:stick"), |_| 1);
        assert_eq!((combat.recover_miss(&bow), combat.spoiled_by_hits(&bow), combat.drawing_speed(&bow)), (0.8, true, 0.4));
        let fists = combat.wielded(None, |_| 1);
        assert_eq!((combat.recover_miss(&fists), combat.spoiled_by_hits(&fists), combat.drawing_speed(&fists)), (0.0, false, 1.0));
        for bad in ["recover_miss: 1.5", "drawing_speed: 0.05", "drawing_speed: 1.2"] {
            assert!(registry(&format!(r#"(classes: [(name: "bow", style: Ranged, cooldown: 1.0, ammo: "stone", {bad})])"#)).is_err(), "{bad} is accepted");
        }
        for bad in ["recover_miss: 0.5", "spoiled_by_hits: true", "drawing_speed: 0.5"] {
            assert!(registry(&format!(r#"(classes: [(name: "sword", style: Slash, cooldown: 0.6, {bad})])"#)).is_err(), "{bad} with nothing to shoot is accepted");
        }
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
        assert_eq!(combat.worn_armour([shirt, hood], Style::Stab, 20, false), 24);
        assert_eq!(combat.worn_armour([shirt, hood], Style::Slash, 20, false), 36);
        assert_eq!(combat.worn_armour([shirt, hood], Style::Slash, 19, false), 0, "below its level armour gives nothing");
        // Plate turns blades better than spells.
        assert_eq!((combat.worn_armour([legs], Style::Crush, 1, false), combat.worn_armour([legs], Style::Magic, 1, false)), (12, 6));
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
        assert_eq!(combat.worn_armour([plate, shield], Style::Stab, 5, false), 21, "plate turns a stab by 1.2: 16.8, and the shield 4");
        assert_eq!(combat.worn_armour([plate, shield], Style::Stab, 5, true), 17, "no shield beside a spear");
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
    fn a_bite_of_food_restarts_the_meter_from_empty() {
        let feel = Feel::default();
        let t = sword();
        let mut rhythm = Rhythm::default();
        let Click::Swing(_) = rhythm.click(9.5, &t, &feel) else { panic!() };
        rhythm.landed(9.7).unwrap();
        rhythm.restart(10.0);
        assert_eq!(rhythm.meter(10.0, &t), 0.0, "empty from the bite");
        // Since the last swing (9.5) a click at 10.2 would have fired; since the bite it is too soon.
        assert_eq!(rhythm.click(10.2, &t, &feel), Click::Ignored);
        let Click::Swing(_) = rhythm.click(10.6, &t, &feel) else { panic!("the beat after the bite fires") };
    }

    #[test]
    fn pips_build_to_a_flourish_and_cancelling_restarts_the_meter_from_half() {
        let feel = Feel::default();
        let t = sword();
        let mut rhythm = Rhythm::default();
        let swing = |on_beat| Swing { starts: 0.0, lands: 0.0, strength: 1.0, on_beat };
        let kinds: Vec<Blow> = (0..5).map(|i| rhythm.blow(&swing(true), i as f64 * 0.6, &feel)).collect();
        assert_eq!(kinds, [Blow::Beat, Blow::Beat, Blow::Beat, Blow::Flourish, Blow::Beat]);
        assert_eq!(rhythm.blow(&swing(false), 3.0, &feel), Blow::Plain);
        assert_eq!(rhythm.pips, 0, "off the beat loses them");
        // A counter or riposte is on the beat by itself: one pip, as any on-beat blow.
        rhythm.blow(&swing(true), 3.6, &feel);
        assert_eq!(rhythm.pips(3.6, &feel), 1);
        assert_eq!(rhythm.pips(5.6, &feel), 1, "they last two seconds after the last blow");
        assert_eq!(rhythm.pips(5.7, &feel), 0, "and then go");
        rhythm.blow(&swing(true), 4.0, &feel);
        rhythm.lose_pips();
        assert_eq!(rhythm.pips(4.0, &feel), 0, "a hit taken or a weapon switched loses them");
        // A client's mirror takes the server's count.
        let mut mirror = Rhythm::default();
        mirror.set_pips(2, 100.0);
        assert_eq!((mirror.pips(101.0, &feel), mirror.pips(102.5, &feel)), (2, 0));
        // Guarding mid wind-up cancels it; the meter is then half full.
        let mut rhythm = Rhythm::default();
        rhythm.click(5.0, &t, &feel);
        assert!(rhythm.cancel(5.05, &t, &feel));
        assert!((rhythm.meter(5.05, &t) - 0.5).abs() < 1e-4);
        assert_eq!(rhythm.landed(9.0), None);
        // A swing already due is not cancelled, though nothing has counted it yet.
        let mut rhythm = Rhythm::default();
        let Click::Swing(swing) = rhythm.click(6.0, &t, &feel) else { panic!() };
        assert!(!rhythm.cancel(swing.lands + 0.01, &t, &feel));
        assert_eq!(rhythm.landed(swing.lands + 0.02), Some(swing));
    }

    #[test]
    fn guards_stop_melee_in_their_arc_and_crush_breaks_through() {
        use glam::Vec3;
        let feel = Feel::default();
        let sword = GuardDef { stop: 0.35, arc: 90.0, perfect: 0.15, speed: 0.5, missiles: false, brace: false };
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
