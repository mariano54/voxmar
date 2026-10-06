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

use crate::settings::Settings;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((widgets::WidgetsPlugin, blur::BlurPlugin, sound::UiSoundPlugin, ime::ImePlugin))
            .add_systems(Update, (change_font.run_if(resource_changed::<Settings>), fit_scale));
    }
}

/// The smallest view the interface is laid out for at full size, in logical pixels: the
/// tallest screen (the menu, with its keys under it) and the widest, with a margin.
const FULL_SIZE: Vec2 = Vec2::new(1100.0, 720.0);

/// Halves the interface in a view smaller than [`FULL_SIZE`], so every screen fits on it;
/// half keeps the pixel art whole, one screen pixel to each of its pixels.
fn fit_scale(cameras: Query<&Camera, With<Camera3d>>, mut scale: ResMut<UiScale>) {
    let Some(view) = cameras.iter().filter_map(Camera::logical_viewport_size).reduce(Vec2::max) else { return };
    let fits = view.x >= FULL_SIZE.x && view.y >= FULL_SIZE.y;
    let wanted = if fits { 1.0 } else { 0.5 };
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
