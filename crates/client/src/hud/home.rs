//! The way home: away from home (the lit hearth, else the bed last slept in: `Vitals::home`,
//! from the server), a small hearth mark at the top of the screen sits over where home lies,
//! with how far it is. Turned away from it, the mark waits at the screen's edge on the side
//! to turn to. Within `home_hidden_within` blocks (`interface.ron`) nothing shows; from
//! dusk to dawn it shows beyond that, by day only beyond `home_shown_beyond`, fading in and
//! out over `home_fade` seconds; the "Way Home Marker" setting turns it off. The mark is
//! the icon of the item a package names as `home_icon` (base: the hearthstone).

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
    /// How far faded in, 0 (gone) to 1.
    pub shown: f32,
    /// Where across the screen it sits (-1 to 1) and the view's width as an angle, for the
    /// gate to read back the bearing it draws.
    pub x: f32,
    pub fov: f32,
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

/// Whether the way home is wanted `away` blocks from home at `tick` of the day (ticks after
/// sunrise): never within `home_hidden_within`, from dusk to dawn beyond it, and by day
/// beyond `home_shown_beyond`.
pub fn wanted(style: &super::inventory::InterfaceStyle, away: f32, tick: u64) -> bool {
    let (dusk, dawn) = (style.home_dusk, style.home_dawn);
    let night = if dusk < dawn { (dusk..dawn).contains(&tick) } else { tick >= dusk || tick < dawn };
    away > style.home_hidden_within && (night || away > style.home_shown_beyond)
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
    (settings, cycle, time): (Res<crate::settings::Settings>, Res<crate::render::DayCycle>, Res<Time>),
    mut mark: Query<(&mut HomeMark, &mut Node, &mut Visibility)>,
    mut icon: Query<&mut ImageNode, With<HomeIcon>>,
    mut words: Query<(&mut Text, &mut TextColor), With<HomeWords>>,
) {
    let Ok((mut mark, mut node, mut visibility)) = mark.single_mut() else { return };
    let home = player.vitals.home.map(|at| at.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
    let away = home.map(|home| (home - player.position).with_y(0.0).length());
    let tick = cycle.ticks.max(0.0) as u64 % voxmar_protocol::TICKS_PER_DAY;
    let want = settings.way_home_marker && ui.cursor_locked && !player.dead && away.is_some_and(|away| wanted(&style, away, tick));
    // Faded in and out, at once with no fade; gone at once with no home or on dying.
    let step = if style.home_fade > 0.0 { time.delta_secs() / style.home_fade } else { 1.0 };
    let shown = if home.is_none() || player.dead { 0.0 } else { (mark.shown + if want { step } else { -step }).clamp(0.0, 1.0) };
    if mark.shown != shown {
        mark.shown = shown;
    }
    let Some((home, away)) = home.zip(away).filter(|_| shown > 0.0) else {
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
    if (mark.x, mark.fov) != (x, fov) {
        (mark.x, mark.fov) = (x, fov);
    }
    let left = ((x + 1.0) * 0.5 * width - WIDTH * 0.5).clamp(0.0, (width - WIDTH).max(0.0));
    if node.left != Val::Px(left) {
        node.left = Val::Px(left);
    }
    visibility.set_if_neq(Visibility::Inherited);
    if let Ok(mut image) = icon.single_mut() {
        if let Some(id) = style.home_icon.as_deref().and_then(|name| world.registry.id(name))
            && let Some(handle) = images.icons.get(id.0 as usize)
            && image.image != *handle
        {
            image.image = handle.clone();
        }
        let faded = Color::WHITE.with_alpha(shown);
        if image.color != faded {
            image.color = faded;
        }
    }
    if let Ok((mut text, mut color)) = words.single_mut() {
        let said = locale.format("hud.home", &[("distance", &(away.round() as i64))]);
        if text.0 != said {
            text.0 = said;
        }
        let faded = SOFT.with_alpha(shown);
        if color.0 != faded {
            color.0 = faded;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inside the home nothing shows; from dusk to dawn it shows past the home's box; by day
    /// only far off. A night that runs past midnight wraps.
    #[test]
    fn the_way_home_shows_at_night_or_far_off() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let files: Vec<_> = content.files.iter().cloned().collect();
        let mut style = super::super::inventory::InterfaceStyle::load(&files).unwrap();
        let (noon, dusk, midnight, dawn) = (8000, style.home_dusk, 20000, style.home_dawn);
        let (inside, near, far) = (style.home_hidden_within - 1.0, style.home_hidden_within + 2.0, style.home_shown_beyond + 2.0);
        for tick in [noon, dusk, midnight, dawn] {
            assert!(!wanted(&style, inside, tick), "never inside the home, {tick}");
            assert!(wanted(&style, far, tick), "always far off, {tick}");
        }
        assert!(wanted(&style, near, dusk) && wanted(&style, near, midnight));
        assert!(!wanted(&style, near, noon) && !wanted(&style, near, dawn), "by day only far off");
        (style.home_dusk, style.home_dawn) = (22000, 2000);
        assert!(wanted(&style, near, 23000) && wanted(&style, near, 1000) && !wanted(&style, near, noon));
    }

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
