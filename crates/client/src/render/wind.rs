//! The wind foliage bends in. It is the weather's wind (`weather::Wind`), so grass stands
//! nearly still on a calm morning and streams flat in a gale, with gusts rolling across a
//! meadow at the wind's speed. The player parts the grass they walk through.
//!
//! Gust fronts are drawn with their own noise, carried along by the mean wind as it blows
//! (an offset integrated frame by frame), rather than with `voxmar_engine::weather::gust`:
//! that function places its fronts at `mean * seconds`, which leaps whenever the mean wind
//! changes, and on screen a whole meadow would lurch.

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};

use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::weather::Wind;

/// Gusts repeat every this many blocks (see wind.wgsl), so their offset wraps unseen.
const GUST_PERIOD: f32 = 1024.0;

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
    /// The player's feet, while in a game.
    pub player: Option<Vec3>,
}

fn blow(time: Res<Time>, wind: Res<Wind>, player: Res<LocalPlayer>, screen: Res<State<Screen>>, mut breeze: ResMut<Breeze>) {
    let velocity = wind.mean();
    breeze.velocity = velocity;
    breeze.gustiness = wind.gustiness;
    breeze.offset = (breeze.offset + velocity * time.delta_secs()).rem_euclid(Vec2::splat(GUST_PERIOD));
    breeze.player = (*screen.get() == Screen::InGame && player.joined).then_some(player.position);
}
