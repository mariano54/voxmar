//! The interface's shared look and parts: the pixel skin, the widgets screens are built
//! from, the blur behind menus, the sounds buttons make, and typing through the system's
//! input method.

pub mod blur;
pub mod fit;
pub mod ime;
pub mod skin;
pub mod sound;
pub mod widgets;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::settings::Settings;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((widgets::WidgetsPlugin, blur::BlurPlugin, sound::UiSoundPlugin, ime::ImePlugin))
            .add_systems(Update, (change_font.run_if(resource_changed::<Settings>), fit_scale));
    }
}

/// The smallest view the interface is laid out for at full size, in logical pixels: the
/// tallest screen (the menu, under the bar of the screens' keys) and the widest, with a margin.
const FULL_SIZE: Vec2 = Vec2::new(1200.0, 800.0);

/// Draws the interface at the size the settings ask for, or at the largest smaller step
/// that fits the view, so every screen fits on it.
/// The size the interface is laid out on, in the window's points: the window's own size (the
/// world's camera may draw into a smaller image, the resolution setting, which the interface
/// is not), or without a window (captures, the gate) the image drawn off screen.
pub fn view_size(windows: &Query<&Window, With<PrimaryWindow>>, cameras: &Query<&Camera, With<Camera3d>>) -> Option<Vec2> {
    match windows.single() {
        Ok(window) => Some(Vec2::new(window.width(), window.height())),
        Err(_) => cameras.iter().filter_map(Camera::logical_viewport_size).reduce(Vec2::max),
    }
}

fn fit_scale(settings: Res<Settings>, windows: Query<&Window, With<PrimaryWindow>>, cameras: Query<&Camera, With<Camera3d>>, mut scale: ResMut<UiScale>) {
    let Some(view) = view_size(&windows, &cameras) else { return };
    let wanted = settings.ui_scale.clamp(50, 150);
    let fits = |percent: i32| view.cmpge(FULL_SIZE * percent as f32 / 100.0).all();
    let percent = crate::settings::UI_SCALES.iter().rev().copied().find(|&step| step <= wanted && fits(step)).unwrap_or(50);
    let wanted = percent as f32 / 100.0;
    if scale.0 != wanted {
        scale.0 = wanted;
    }
}

/// Rebuilds the font when the setting changes, and lays every text out again in it.
fn change_font(settings: Res<Settings>, mut fonts: ResMut<Assets<Font>>, mut texts: Query<&mut TextFont>, mut built: Local<Option<skin::InterfaceFont>>) {
    // Startup built the saved choice.
    let current = *built.get_or_insert(settings.font);
    if current == settings.font {
        return;
    }
    *built = Some(settings.font);
    skin::set_font(settings.font, &mut fonts);
    for mut font in &mut texts {
        font.set_changed();
    }
}
