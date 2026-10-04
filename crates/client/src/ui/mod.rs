//! The interface's shared look and parts: the pixel skin, the widgets screens are built
//! from, and the blur behind menus.

pub mod blur;
pub mod skin;
pub mod widgets;

use bevy::prelude::*;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((widgets::WidgetsPlugin, blur::BlurPlugin));
    }
}
