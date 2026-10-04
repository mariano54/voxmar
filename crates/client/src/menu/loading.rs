//! The cover shown while joining a world, until the ground under the player has arrived
//! and been meshed.

use bevy::prelude::*;
use voxmar_engine::coords::ColumnPos;

use super::Screen;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{BODY, GOLD, LARGE, MUTED, UiFont, shadowed};
use crate::net::Net;
use crate::player::LocalPlayer;
use crate::world::ClientWorld;

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::InGame), spawn).add_systems(Update, update.run_if(in_state(Screen::InGame)));
    }
}

#[derive(Component)]
struct Cover;
#[derive(Component)]
struct Title;
#[derive(Component)]
struct Detail;
/// The sliding bar showing that something is happening.
#[derive(Component)]
struct Pulse;

const BAR: f32 = 110.0 * PX;
const PULSE: f32 = 30.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let root = Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        row_gap: Val::Px(5.0 * PX),
        ..default()
    };
    // Above the HUD and the settings panel, which may open meanwhile.
    let cover = (Cover, DespawnOnExit(Screen::InGame), root, BackgroundColor(Color::srgb(0.05, 0.055, 0.085)), GlobalZIndex(2));
    commands.spawn(cover).with_children(|cover| {
        cover.spawn((Title, shadowed(font, "Joining", LARGE, GOLD)));
        cover.spawn((Detail, shadowed(font, "", BODY, MUTED)));
        let track = Node {
            width: Val::Px(BAR),
            height: Val::Px(6.0 * PX),
            margin: UiRect::top(Val::Px(8.0 * PX)),
            padding: UiRect::all(Val::Px(PX)),
            ..default()
        };
        let pulse = Node { width: Val::Px(PULSE), height: Val::Percent(100.0), ..default() };
        cover.spawn((track, Piece::Well.image())).with_child((Pulse, pulse, Piece::Bar.image()));
    });
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    time: Res<Time>,
    net: Res<Net>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    cover: Query<Entity, With<Cover>>,
    mut title: Query<&mut Text, (With<Title>, Without<Detail>)>,
    mut detail: Query<&mut Text, (With<Detail>, Without<Title>)>,
    mut pulse: Query<&mut Node, With<Pulse>>,
) {
    let Ok(cover) = cover.single() else { return };
    let ground = ColumnPos::of_block(player.position.floor().as_ivec3());
    if player.joined && world.voxels.contains(ground) && world.stats.chunk_entities > 0 {
        commands.entity(cover).despawn();
        return;
    }
    let (heading, message) = match (player.joined, net.is_remote()) {
        (false, true) => ("Joining server", "Waiting for the server to let you in"),
        (false, false) => ("Opening world", "Starting the world"),
        (true, _) => ("Loading world", "Building the terrain around you"),
    };
    if let Ok(mut text) = title.single_mut()
        && text.0 != heading
    {
        heading.clone_into(&mut text.0);
    }
    if let Ok(mut text) = detail.single_mut()
        && text.0 != message
    {
        message.clone_into(&mut text.0);
    }
    if let Ok(mut node) = pulse.single_mut() {
        // Back and forth along the track.
        let phase = (time.elapsed_secs() * 1.4).sin() * 0.5 + 0.5;
        node.left = Val::Px((phase * (BAR - 2.0 * PX - PULSE) / PX).round() * PX);
    }
}
