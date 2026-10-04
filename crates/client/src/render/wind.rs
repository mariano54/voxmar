//! The wind foliage bends in. It is the weather's wind (`weather::Wind`), so grass stands
//! nearly still on a calm morning and streams flat in a gale, with gusts rolling across a
//! meadow at the wind's speed. The player parts the grass they walk through.
//!
//! Gust fronts are drawn with their own noise, carried along by the mean wind as it blows
//! (an offset integrated frame by frame), rather than with `voxmar_engine::weather::gust`:
//! that function places its fronts at `mean * seconds`, which leaps whenever the mean wind
//! changes, and on screen a whole meadow would lurch. The flutter is integrated the same
//! way: `time * pace` leaps by `time * Δpace` whenever the pace changes, so an hour into a
//! game every change of wind sent the grass into a fit, and a changing wind kept it
//! shaking far faster than its pace.

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};

use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::weather::Wind;

/// Gusts repeat every this many blocks (see wind.wgsl), so their offset wraps unseen.
const GUST_PERIOD: f32 = 1024.0;
/// The flutter phase wraps after 320 turns, so a flutter at any whole number of tenths or
/// 64ths of it (see wind.wgsl) is still in step across the wrap.
const FLUTTER_PERIOD: f32 = 320.0 * std::f32::consts::TAU;
/// A gale, in blocks per second, where foliage's response to the wind tops out (as
/// `wind_strength` in wind.wgsl).
const GALE: f32 = 18.0;

pub struct WindPlugin;

impl Plugin for WindPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Breeze>().add_plugins(ExtractResourcePlugin::<Breeze>::default()).add_systems(Update, blow);
    }
}

/// What the terrain shaders bend foliage with this frame.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub struct Breeze {
    /// Mean wind along x and z, in blocks per second.
    pub velocity: Vec2,
    /// How much it gusts, 0..1.
    pub gustiness: f32,
    /// How far gusts have travelled with the wind, wrapped every `GUST_PERIOD` blocks.
    pub offset: Vec2,
    /// The flutter's phase in radians, which runs quicker in a stronger wind.
    pub flutter: f32,
    /// The player's feet, while in a game.
    pub player: Option<Vec3>,
}

fn blow(time: Res<Time>, wind: Res<Wind>, player: Res<LocalPlayer>, screen: Res<State<Screen>>, mut breeze: ResMut<Breeze>) {
    let velocity = wind.mean();
    breeze.velocity = velocity;
    breeze.gustiness = wind.gustiness;
    breeze.offset = (breeze.offset + velocity * time.delta_secs()).rem_euclid(Vec2::splat(GUST_PERIOD));
    let pace = 1.0 + 1.4 * (velocity.length() / GALE).min(1.0);
    breeze.flutter = (breeze.flutter + pace * time.delta_secs()).rem_euclid(FLUTTER_PERIOD);
    breeze.player = (*screen.get() == Screen::InGame && player.joined).then_some(player.position);
}
