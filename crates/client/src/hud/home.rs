//! The way home: away from home (the lit hearth, else the bed last slept in: `Vitals::home`,
//! from the server), a small hearth mark at the top of the screen sits over where home lies,
//! with how far it is. Turned away from it, the mark waits at the screen's edge on the side
//! to turn to. Within `home_hidden_within` blocks (`interface.ron`) nothing shows; the
//! mark is the icon of the item a package names as `home_icon` (base: the hearthstone).

use bevy::prelude::*;
use bevy::ui::UiScale;
use bevy::window::PrimaryWindow;

use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::ui::skin::PX;
use crate::ui::widgets::{BODY, SOFT, UiFont, shadowed, space};

pub struct HomePlugin;

impl Plugin for HomePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn)
            .add_systems(Update, show.run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), hide);
    }
}

/// The mark's width, for keeping it whole on screen.
const WIDTH: f32 = 40.0 * PX;

/// The mark, and the screen's edge it waits at (-1 left, 1 right) while home is out of
/// view, for the gate.
#[derive(Component, Default)]
pub(crate) struct HomeMark {
    pub edge: Option<f32>,
}
#[derive(Component)]
struct HomeIcon;
#[derive(Component)]
pub(crate) struct HomeWords;

/// Out of the world the mark goes, as the combat meter does.
fn hide(mut marks: Query<&mut Visibility, With<HomeMark>>) {
    for mut visibility in &mut marks {
        *visibility = Visibility::Hidden;
    }
}

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let mark = Node {
        position_type: PositionType::Absolute,
        top: Val::Px(space::GAP),
        width: Val::Px(WIDTH),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: Val::Px(space::HAIR),
        ..default()
    };
    commands.spawn((HomeMark::default(), mark, Visibility::Hidden)).with_children(|mark| {
        mark.spawn((HomeIcon, Node { width: Val::Px(10.0 * PX), height: Val::Px(10.0 * PX), ..default() }, ImageNode::default()));
        mark.spawn((HomeWords, shadowed(&font.0, "", BODY, SOFT)));
    });
}

/// Where across the screen, from -1 (its left edge) to 1 (its right), a bearing `angle`
/// radians to the right of the view lies with a horizontal field of view `fov`; held at the
/// edges when it lies outside the view, with which edge (-1 or 1) if so.
pub fn across(angle: f32, fov: f32) -> (f32, Option<f32>) {
    let half = (fov * 0.5).clamp(0.05, 1.5);
    if angle.abs() < half {
        (angle.tan() / half.tan(), None)
    } else {
        let side = angle.signum();
        (side, Some(side))
    }
}

/// The bearing to `to` from `from`, in radians to the right of a view turned `yaw`.
pub fn bearing(from: Vec3, yaw: f32, to: Vec3) -> f32 {
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let way = Vec2::new(to.x - from.x, to.z - from.z);
    // Right of the view is forward turned clockwise, seen from above.
    let right = Vec2::new(-forward.y, forward.x);
    way.dot(right).atan2(way.dot(forward))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn show(
    player: Res<LocalPlayer>,
    ui: Res<UiState>,
    style: Res<super::inventory::InterfaceStyle>,
    camera: Query<&Projection, With<Camera3d>>,
    locale: Res<Locale>,
    images: Res<super::HudImages>,
    world: Res<crate::world::ClientWorld>,
    (window, cameras, scale): (Query<&Window, With<PrimaryWindow>>, Query<&Camera, With<Camera3d>>, Res<UiScale>),
    mut mark: Query<(&mut HomeMark, &mut Node, &mut Visibility)>,
    mut icon: Query<&mut ImageNode, With<HomeIcon>>,
    mut words: Query<&mut Text, With<HomeWords>>,
) {
    let Ok((mut mark, mut node, mut visibility)) = mark.single_mut() else { return };
    let home = player.vitals.home.map(|at| at.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
    let away = home.map(|home| (home - player.position).with_y(0.0).length());
    let Some((home, away)) = home.zip(away).filter(|&(_, away)| ui.cursor_locked && !player.dead && away > style.home_hidden_within) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    // The window, or with none (the gate, captures) the image drawn off screen; the
    // interface is laid out at its own scale.
    let Some(view) = crate::ui::view_size(&window, &cameras) else { return };
    let (width, height) = (view.x.max(1.0) / scale.0.max(0.01), view.y.max(1.0) / scale.0.max(0.01));
    // The view's width as an angle, from its height's (the camera's) and the view's shape.
    let tall = camera.iter().find_map(|p| if let Projection::Perspective(p) = p { Some(p.fov) } else { None }).unwrap_or(1.2);
    let fov = 2.0 * ((tall * 0.5).tan() * width / height).atan();
    let (x, edge) = across(bearing(player.position, player.yaw, home), fov);
    if mark.edge != edge {
        mark.edge = edge;
    }
    let left = ((x + 1.0) * 0.5 * width - WIDTH * 0.5).clamp(0.0, (width - WIDTH).max(0.0));
    if node.left != Val::Px(left) {
        node.left = Val::Px(left);
    }
    visibility.set_if_neq(Visibility::Inherited);
    if let (Ok(mut image), Some(id)) = (icon.single_mut(), style.home_icon.as_deref().and_then(|name| world.registry.id(name)))
        && let Some(handle) = images.icons.get(id.0 as usize)
        && image.image != *handle
    {
        image.image = handle.clone();
    }
    if let Ok(mut text) = words.single_mut() {
        let said = locale.format("hud.home", &[("distance", &(away.round() as i64))]);
        if text.0 != said {
            text.0 = said;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Home dead ahead sits in the middle; to the right, right of it; behind, at the edge
    /// on the side to turn to.
    #[test]
    fn the_way_home_lies_where_home_is_seen() {
        let at = Vec3::ZERO;
        // Facing yaw 0 looks along -z.
        let ahead = bearing(at, 0.0, Vec3::new(0.0, 0.0, -50.0));
        assert!(ahead.abs() < 1e-4);
        assert_eq!(across(ahead, 1.6).0, 0.0);
        let right = bearing(at, 0.0, Vec3::new(20.0, 0.0, -50.0));
        let (x, edge) = across(right, 1.6);
        assert!(right > 0.0 && x > 0.0 && x < 1.0 && edge.is_none(), "{right} {x}");
        let behind_left = bearing(at, 0.0, Vec3::new(-5.0, 0.0, 50.0));
        assert_eq!(across(behind_left, 1.6), (-1.0, Some(-1.0)));
        // Turned a quarter to the left (yaw grows to the left), what was ahead is right.
        assert!((bearing(at, std::f32::consts::FRAC_PI_2, Vec3::new(0.0, 0.0, -50.0)) - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }
}
