//! The interface's shared look and parts: the pixel skin, the widgets screens are built
//! from, the blur behind menus and the sounds buttons make.

pub mod blur;
pub mod skin;
pub mod sound;
pub mod widgets;

use bevy::prelude::*;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((widgets::WidgetsPlugin, blur::BlurPlugin, sound::UiSoundPlugin));
    }
}
