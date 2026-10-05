//! The interface's shared look and parts: the pixel skin, the widgets screens are built
//! from, the blur behind menus, the sounds buttons make, and typing through the system's
//! input method.

pub mod blur;
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
            .add_systems(Update, change_font.run_if(resource_changed::<Settings>));
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
