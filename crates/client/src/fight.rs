//! How fights feel on this side (`voxmar_engine::combat`; the server's `fight` decides what
//! lands): the swing rhythm this side keeps for the cooldown meter, the guard held up with
//! the right button, the dodge's dash, what the server said of blows lately, for
//! hitsplats, and the prayer points and prayers on, which quick keys turn on and off.
//!
//! The meter, pips, hitsplats and prayers are drawn by the interface from [`Fighting`].

use bevy::prelude::*;
use voxmar_engine::combat::{Blow, Click, Defended, Rhythm, Timing};

use crate::interact::Target;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;

/// How long what the server said of a blow is kept, for hitsplats.
const KEPT: f64 = 2.0;

#[derive(Resource, Default)]
pub struct Fighting {
    /// The player's swings as this side times them, in seconds of app time.
    pub rhythm: Rhythm,
    /// When the guard went up, while it is up.
    pub guard: Option<f64>,
    /// A dash under way: its direction, speed and seconds left.
    pub dash: Option<(Vec3, f32, f32)>,
    /// When the next dodge may start, and when it might before the last one, which a dodge
    /// the server refuses gives back.
    pub dodge_ready: f64,
    pub dodge_was: f64,
    /// When the player was last on the ground, for `Feel::dodge_grace`.
    pub grounded: f64,
    /// A right press this frame already did something (opened a table, ate), so it raises
    /// no guard until it is let go.
    pub right_used: bool,
    /// With the guard on a toggle (`Settings::toggle_guard`): whether a right click put it
    /// up, until the next.
    pub guard_toggled: bool,
    /// The player's blows the server answered lately: at which creature, for how much, what
    /// kind, when (app seconds), and whether that exact blow killed it.
    pub struck: Vec<(u32, u16, Blow, f64, bool)>,
    /// Creatures' blows at the player lately, and what came of each.
    pub hurt: Vec<(u32, u16, Defended, f64)>,
    /// How many blows each way the server has answered, ever: what changed since a count was
    /// taken is the tail of `struck` and `hurt`.
    pub struck_count: u32,
    pub hurt_count: u32,
    /// How many blows this side has asked the server to strike, ever (`core:attack`): a click
    /// at nothing in reach, or one held for the beat, asks nothing.
    pub strikes: u32,
    /// How many swings the player has cancelled before their blow, ever (raising the guard,
    /// dodging, or changing what is held): the server judges no blow for them (`fight_fx`'s shots).
    pub cancels: u32,
    /// How many of the server's pings (`ServerMessage::Ping`), which time the round trip
    /// guards and dodges are judged by, have been answered, ever.
    pub pings: u32,
    /// The share of their pace the player kept as they last moved, for a shot being drawn
    /// (`drawing_speed`); None when nothing was drawn.
    pub drawing_pace: Option<f32>,
    /// How many clicks at a creature out of reach have swung at the air, ever (`whiff`),
    /// and when the last did (app seconds).
    pub whiffs: u32,
    whiffed: f64,
    /// The item last swung, to lose the pips on a switch as the server does.
    held: Option<voxmar_engine::block::BlockId>,
    /// When the player and each creature last traded blows, either way (app seconds).
    fought: std::collections::HashMap<u32, f64>,
    /// Prayer points and the prayers on, as the server last said (`voxmar_engine::prayer`).
    pub prayers: Prayers,
}

/// A player's prayer points (whole, of the most they have) and the prayers they have on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prayers {
    pub points: u16,
    pub most: u16,
    pub on: Vec<voxmar_engine::prayer::PrayerId>,
}

impl Fighting {
    /// The player's blow at `creature` was answered, with their pips as the server counts
    /// them.
    pub fn struck(&mut self, creature: u32, damage: u16, blow: Blow, pips: u8, killed: bool, now: f64) {
        self.rhythm.set_pips(pips, now);
        self.fought.insert(creature, now);
        self.struck.retain(|s| now - s.3 < KEPT);
        self.struck.push((creature, damage, blow, now, killed));
        self.struck_count += 1;
    }

    /// A creature's blow at the player came down. An unguarded hit costs the pips, and
    /// spoils a shot being drawn with a weapon whose class says so (`spoil`: its timing),
    /// as the server does.
    pub fn hurt(&mut self, creature: u32, damage: u16, defended: Defended, now: f64, spoil: Option<(Timing, voxmar_engine::combat::Feel)>) {
        if defended == Defended::Hit && damage > 0 {
            self.rhythm.lose_pips();
            if let Some((timing, feel)) = spoil {
                self.cancel(now, &timing, &feel);
            }
        }
        self.fought.insert(creature, now);
        self.hurt.retain(|h| now - h.3 < KEPT);
        self.hurt.push((creature, damage, defended, now));
        self.hurt_count += 1;
    }

    /// Whether a shot or swing is being drawn at `now`: started, its blow not yet landed.
    pub fn drawn(&self, now: f64) -> bool {
        self.rhythm.pending().is_some_and(|s| s.lands > now)
    }

    /// Whether `creature` and the player have traded blows within `feel.engaged` of `now`:
    /// a right click at it guards rather than looks closely.
    pub fn engaged(&mut self, creature: u32, now: f64, feel: &voxmar_engine::combat::Feel) -> bool {
        self.fought.retain(|_, at| now - *at < feel.engaged as f64);
        self.fought.contains_key(&creature)
    }

    /// A click at a creature at `now`: whether a swing starts (or is held for the beat), so
    /// the click goes to the server. None while guarding.
    pub fn click(&mut self, now: f64, timing: &Timing, held: Option<voxmar_engine::block::BlockId>, feel: &voxmar_engine::combat::Feel) -> bool {
        if self.guard.is_some() {
            return false;
        }
        self.holding(now, timing, held, feel);
        matches!(self.rhythm.click(now, timing, feel), Click::Swing(_))
    }

    /// Whether a click out of reach at `now` would swing at the air (`whiff`): the meter
    /// full, no swing under way, no guard up, and a cooldown since the last whiff.
    pub fn can_whiff(&self, now: f64, timing: &Timing) -> bool {
        let rested = self.rhythm.pending().is_none() && self.rhythm.meter(now, timing) >= 1.0;
        self.guard.is_none() && rested && now - self.whiffed >= timing.cooldown as f64
    }

    /// A click at a creature out of the weapon's reach at `now`: the hand swings at the air,
    /// asking nothing of the server and leaving the rhythm as it is. Only with the meter
    /// full and no swing under way, at most once a cooldown, and not while guarding.
    pub fn whiff(&mut self, now: f64, timing: &Timing) -> bool {
        if !self.can_whiff(now, timing) {
            return false;
        }
        self.whiffed = now;
        self.whiffs += 1;
        true
    }

    /// Guarding, dodging or switching what is held cancels a swing not yet landed
    /// (`Rhythm::cancel`), counted in `cancels`.
    pub fn cancel(&mut self, now: f64, timing: &Timing, feel: &voxmar_engine::combat::Feel) {
        if self.rhythm.cancel(now, timing, feel) {
            self.cancels += 1;
        }
    }

    /// What is in hand at `now`: switching it drops the swing under way and the pips, as the
    /// server does. `timing` is the rhythm of what was held.
    pub fn holding(&mut self, now: f64, timing: &Timing, held: Option<voxmar_engine::block::BlockId>, feel: &voxmar_engine::combat::Feel) {
        if held != self.held {
            if self.rhythm.pending().is_some() {
                self.cancel(now, timing, feel);
            }
            self.rhythm.lose_pips();
            self.held = held;
        }
    }
}

/// Holding the right button with a weapon that guards raises the guard (or, with the guard
/// on a toggle, a right click raises it and the next lowers it), unless the press already
/// did something else (a station, food). The server is told when it goes up and
/// down; raising it cancels the player's own wind-up.
#[allow(clippy::too_many_arguments)]
pub fn guard(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    net: Res<Net>,
    player: Res<LocalPlayer>,
    target: Res<Target>,
    settings: Res<crate::settings::Settings>,
    mut fighting: ResMut<Fighting>,
) {
    let now = time.elapsed_secs_f64();
    fighting.rhythm.landed(now);
    let combat = &launch.content.combat;
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let level = |skill| launch.content.skills.level(player.skills.xp(skill));
    let weapon = combat.wielded(held, level);
    let was = combat.wielded(fighting.held, level);
    fighting.holding(now, &combat.timing(&was), held, &combat.feel);
    let can = combat.guard_of(held, &player.inventory.worn, level).is_some() && ui.cursor_locked && !player.dead && player.joined;
    if mouse.just_pressed(MouseButton::Right) {
        fighting.right_used = target.used_right;
        // On a toggle, each right click that did nothing else puts the guard up or down.
        if settings.toggle_guard && can && !fighting.right_used {
            fighting.guard_toggled = !fighting.guard_toggled;
        }
    }
    if !can || !settings.toggle_guard {
        fighting.guard_toggled = false;
    }
    let held_up = if settings.toggle_guard { fighting.guard_toggled } else { mouse.pressed(MouseButton::Right) && !fighting.right_used };
    let up = can && held_up && fighting.dash.is_none();
    if up != fighting.guard.is_some() {
        fighting.guard = up.then_some(now);
        if up {
            let timing = combat.timing(&weapon);
            fighting.cancel(now, &timing, &combat.feel);
        }
        net.guard(up);
    }
}

/// A quick prayer's key turns its prayer on, or off if the server has it on. The server
/// decides: a prayer above the player's Magic level, or with no points left, stays off.
pub fn quick_prayers(keys: Res<ButtonInput<KeyCode>>, settings: Res<crate::settings::Settings>, ui: Res<UiState>, launch: Res<Launch>, net: Res<Net>, player: Res<LocalPlayer>, fighting: Res<Fighting>) {
    if !ui.cursor_locked || player.dead || !player.joined {
        return;
    }
    for (key, name) in settings.prayer_keys.iter().zip(&settings.quick_prayers) {
        if keys.just_pressed(key.0)
            && let Some(prayer) = launch.content.prayers.id(name)
        {
            net.pray(prayer, !fighting.prayers.on.contains(&prayer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every way a swing is cancelled (the guard, a dodge, a switch of what is held) goes
    /// through `Fighting::cancel`, which counts it only when a swing was under way.
    #[test]
    fn a_cancelled_swing_is_counted_once() {
        let (timing, feel) = (Timing { cooldown: 1.0, windup: 0.4, buffer: 0.2, beat: 0.1 }, voxmar_engine::combat::Feel::default());
        let mut fighting = Fighting::default();
        fighting.cancel(0.0, &timing, &feel);
        assert_eq!(fighting.cancels, 0, "nothing under way to cancel");
        assert!(matches!(fighting.rhythm.click(0.0, &timing, &feel), Click::Swing(_)));
        fighting.cancel(0.1, &timing, &feel);
        assert_eq!(fighting.cancels, 1);
        fighting.cancel(0.2, &timing, &feel);
        assert_eq!(fighting.cancels, 1, "the swing is already cancelled");
        // A switch of what is held cancels the swing under way the same way.
        let later = 5.0;
        assert!(matches!(fighting.rhythm.click(later, &timing, &feel), Click::Swing(_)));
        fighting.holding(later + 0.1, &timing, Some(voxmar_engine::block::BlockId(3)), &feel);
        assert_eq!(fighting.cancels, 2);
    }

    /// A hit told of spoils a draw when the weapon's class says so, as the server does:
    /// the swing is cancelled and counted, and the player is no longer slowed by it. A
    /// blocked blow, or one with nothing to spoil, leaves the draw.
    #[test]
    fn a_hit_told_of_spoils_a_draw_that_spoils() {
        let (timing, feel) = (Timing { cooldown: 1.0, windup: 0.5, buffer: 0.2, beat: 0.1 }, voxmar_engine::combat::Feel::default());
        let mut fighting = Fighting::default();
        assert!(matches!(fighting.rhythm.click(0.0, &timing, &feel), Click::Swing(_)));
        assert!(fighting.drawn(0.2) && !fighting.drawn(0.6), "drawn through the wind-up only");
        fighting.hurt(7, 2, Defended::Blocked, 0.1, Some((timing, feel)));
        fighting.hurt(7, 2, Defended::Hit, 0.15, None);
        assert!(fighting.drawn(0.2), "a blocked blow, or a weapon that does not spoil, keeps the draw");
        fighting.hurt(7, 2, Defended::Hit, 0.2, Some((timing, feel)));
        assert!(!fighting.drawn(0.3) && fighting.cancels == 1, "the hit spoiled the draw");
    }

    /// A click at a creature out of reach swings at the air: once a cooldown, only from
    /// rest (no swing under way, the meter full), never while guarding, and the rhythm is
    /// left as it was, so the next real swing is timed as if it never happened.
    #[test]
    fn a_click_out_of_reach_whiffs_from_rest_and_leaves_the_rhythm() {
        let (timing, feel) = (Timing { cooldown: 1.0, windup: 0.4, buffer: 0.2, beat: 0.1 }, voxmar_engine::combat::Feel::default());
        let mut fighting = Fighting::default();
        assert!(fighting.whiff(10.0, &timing));
        assert_eq!((fighting.whiffs, fighting.rhythm.pending().is_none(), fighting.rhythm.meter(10.0, &timing)), (1, true, 1.0), "a whiff touched the rhythm");
        assert!(!fighting.whiff(10.5, &timing), "two whiffs within a cooldown");
        assert!(fighting.whiff(11.0, &timing));
        // Not mid-swing: a real swing winding up, or its meter not yet full.
        assert!(matches!(fighting.rhythm.click(20.0, &timing, &feel), Click::Swing(_)));
        assert!(!fighting.whiff(20.1, &timing), "a whiff over a swing winding up");
        let _ = fighting.rhythm.landed(20.5);
        assert!(!fighting.whiff(20.6, &timing), "a whiff before the meter was full");
        assert!(fighting.whiff(21.0, &timing));
        fighting.guard = Some(30.0);
        assert!(!fighting.whiff(30.0, &timing), "a whiff while guarding");
        assert_eq!(fighting.whiffs, 3);
    }
}
