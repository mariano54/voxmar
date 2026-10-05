//! The cover shown while joining a world, until the ground under the player has arrived
//! and been meshed. Behind it plays a pre-rendered flight over a world, softened and tinted
//! blue: `assets/ui/loading.png`, a sheet of frames (see `REEL`), crossfaded so a few
//! frames a second move smoothly. Without the sheet the cover is plain.

use bevy::prelude::*;
use voxmar_engine::coords::ColumnPos;

use super::Screen;
use crate::lang::Locale;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{BODY, GOLD, LARGE, MUTED, UiFont, shadowed};
use crate::net::Net;
use crate::player::LocalPlayer;
use crate::world::ClientWorld;

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::InGame), spawn).add_systems(Update, (update, play_reel).run_if(in_state(Screen::InGame)));
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

/// The loading reel's sheet: frames left to right, then top to bottom, each this many
/// pixels, shown this many a second.
const REEL: &str = "ui/loading.png";
const REEL_COLUMNS: u32 = 6;
const REEL_FRAMES: u32 = 36;
const REEL_FRAME: Vec2 = Vec2::new(320.0, 180.0);
const REEL_FPS: f32 = 6.0;

/// A layer of the reel: the frame it shows, or the next one fading in over it.
#[derive(Component)]
struct ReelLayer {
    next: bool,
}

const BAR: f32 = 110.0 * PX;
const PULSE: f32 = 30.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>, assets: Res<AssetServer>) {
    let font = &font.0;
    let reel: Handle<Image> = if voxmar_server::assets_dir().join(REEL).is_file() { assets.load(REEL) } else { Handle::default() };
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
        let full = || Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() };
        for next in [false, true] {
            let image = ImageNode { image: reel.clone(), image_mode: NodeImageMode::Stretch, rect: Some(frame_rect(0)), ..default() };
            cover.spawn((full(), image, ReelLayer { next }));
        }
        // A wash of night blue over the reel keeps the text readable.
        cover.spawn((full(), BackgroundColor(Color::srgba(0.03, 0.05, 0.12, 0.45))));
        cover.spawn((Title, shadowed(font, "", LARGE, GOLD)));
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

/// Where frame `index` sits in the reel's sheet.
fn frame_rect(index: u32) -> Rect {
    let (column, row) = (index % REEL_COLUMNS, index / REEL_COLUMNS);
    let min = Vec2::new(column as f32, row as f32) * REEL_FRAME;
    Rect::from_corners(min, min + REEL_FRAME)
}

/// Plays the reel: each layer pair shows a frame and fades the next in over it.
fn play_reel(time: Res<Time>, mut layers: Query<(&ReelLayer, &mut ImageNode)>) {
    let at = time.elapsed_secs() * REEL_FPS;
    let frame = at as u32 % REEL_FRAMES;
    let fade = at.fract();
    for (layer, mut image) in &mut layers {
        let index = if layer.next { (frame + 1) % REEL_FRAMES } else { frame };
        image.rect = Some(frame_rect(index));
        image.color = Color::WHITE.with_alpha(if layer.next { fade } else { 1.0 });
    }
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
    locale: Res<Locale>,
) {
    let Ok(cover) = cover.single() else { return };
    let ground = ColumnPos::of_block(player.position.floor().as_ivec3());
    if player.joined && world.voxels.contains(ground) && world.stats.chunk_entities > 0 {
        commands.entity(cover).despawn();
        return;
    }
    let (heading, message) = match (player.joined, net.is_remote()) {
        (false, true) => ("loading.joining", "loading.joining.detail"),
        (false, false) => ("loading.opening", "loading.opening.detail"),
        (true, _) => ("loading.building", "loading.building.detail"),
    };
    let (heading, message) = (locale.get(heading), locale.get(message));
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
