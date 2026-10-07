//! How fights feel on this side (`voxmar_engine::combat`; the server's `fight` decides what
//! lands): the swing rhythm this side keeps for the cooldown meter, the guard held up with
//! the right button, the dodge's dash, and what the server said of blows lately, for
//! hitsplats.
//!
//! The meter, pips and hitsplats are drawn by the interface from [`Fighting`].

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
    /// When the next dodge may start.
    pub dodge_ready: f64,
    /// A right press this frame already did something (opened a table, ate), so it raises
    /// no guard until it is let go.
    pub right_used: bool,
    /// The player's blows the server answered lately: at which creature, for how much, what
    /// kind, and when (app seconds).
    pub struck: Vec<(u32, u16, Blow, f64)>,
    /// Creatures' blows at the player lately, and what came of each.
    pub hurt: Vec<(u32, u16, Defended, f64)>,
    /// How many blows each way the server has answered, ever: what changed since a count was
    /// taken is the tail of `struck` and `hurt`.
    pub struck_count: u32,
    pub hurt_count: u32,
    /// The item last swung, to lose the pips on a switch as the server does.
    held: Option<voxmar_engine::block::BlockId>,
}

impl Fighting {
    /// The player's blow at `creature` was answered.
    pub fn struck(&mut self, creature: u32, damage: u16, blow: Blow, now: f64) {
        self.rhythm.pips = match blow {
            Blow::Beat | Blow::Counter => self.rhythm.pips + 1,
            Blow::Plain | Blow::Flourish => 0,
            Blow::Bleed => self.rhythm.pips,
        }
        .min(3);
        self.struck.retain(|s| now - s.3 < KEPT);
        self.struck.push((creature, damage, blow, now));
        self.struck_count += 1;
    }

    /// A creature's blow at the player came down.
    pub fn hurt(&mut self, creature: u32, damage: u16, defended: Defended, now: f64) {
        if defended == Defended::Hit && damage > 0 {
            self.rhythm.lose_pips();
        }
        self.hurt.retain(|h| now - h.3 < KEPT);
        self.hurt.push((creature, damage, defended, now));
        self.hurt_count += 1;
    }

    /// A click at a creature at `now`: whether a swing starts (or is held for the beat), so
    /// the click goes to the server. None while guarding.
    pub fn click(&mut self, now: f64, timing: &Timing, held: Option<voxmar_engine::block::BlockId>, feel: &voxmar_engine::combat::Feel) -> bool {
        if self.guard.is_some() {
            return false;
        }
        if held != self.held {
            self.rhythm.lose_pips();
            self.held = held;
        }
        matches!(self.rhythm.click(now, timing, feel), Click::Swing(_))
    }
}

/// Holding the right button with a weapon that guards raises the guard, unless the press
/// already did something else (a station, food). The server is told when it goes up and
/// down; raising it cancels the player's own wind-up.
pub fn guard(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    net: Res<Net>,
    player: Res<LocalPlayer>,
    target: Res<Target>,
    mut fighting: ResMut<Fighting>,
) {
    let now = time.elapsed_secs_f64();
    fighting.rhythm.landed(now);
    let combat = &launch.content.combat;
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let level = |skill| launch.content.skills.level(player.skills.xp(skill));
    let weapon = combat.wielded(held, level);
    let can = combat.guard_of(held, &player.inventory.worn, level).is_some() && ui.cursor_locked && !player.dead && player.joined;
    if mouse.just_pressed(MouseButton::Right) {
        fighting.right_used = target.used_right;
    }
    let up = can && mouse.pressed(MouseButton::Right) && !fighting.right_used && fighting.dash.is_none();
    if up != fighting.guard.is_some() {
        fighting.guard = up.then_some(now);
        if up {
            let timing = combat.timing(&weapon);
            fighting.rhythm.cancel(now, &timing, &combat.feel);
        }
        net.guard(up);
    }
}
