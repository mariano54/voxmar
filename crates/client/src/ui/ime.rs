//! Typing through the system's input method, which is how Japanese is typed: keystrokes
//! compose kana and kanji (shown as they are composed) until the player commits them into
//! the text field or console line being typed. The input method is on only while
//! something takes text, so game keys never go to it.

use bevy::prelude::*;
use bevy::window::{Ime, PrimaryWindow};

use crate::player::UiState;
use crate::ui::widgets::Focused;

pub struct ImePlugin;

impl Plugin for ImePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Composing>().add_systems(Update, compose.in_set(ImeSystems));
    }
}

/// Reading the input method; whatever takes text runs after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImeSystems;

/// What the input method has for the text being typed.
#[derive(Resource, Default)]
pub struct Composing {
    /// Text being composed, not yet committed: shown after what is typed.
    pub preedit: String,
    /// Text committed since whatever takes text last took it.
    committed: String,
}

impl Composing {
    /// The committed text, unless it is `typed`: some platforms report plain keys both
    /// as typed text and as a commit.
    pub fn take(&mut self, typed: &str) -> String {
        let committed = std::mem::take(&mut self.committed);
        if committed == typed { String::new() } else { committed }
    }
}

/// Turns the input method on while a text field is focused or the console is open, and
/// gathers what it composes and commits.
fn compose(
    mut events: MessageReader<Ime>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    focused: Query<(), With<Focused>>,
    ui: Res<UiState>,
    mut composing: ResMut<Composing>,
) {
    let typing = !focused.is_empty() || ui.console_open;
    if let Ok(mut window) = window.single_mut()
        && window.ime_enabled != typing
    {
        window.ime_enabled = typing;
    }
    for event in events.read() {
        match event {
            Ime::Preedit { value, .. } => value.clone_into(&mut composing.preedit),
            Ime::Commit { value, .. } => {
                composing.committed.push_str(value);
                composing.preedit.clear();
            }
            Ime::Enabled { .. } | Ime::Disabled { .. } => composing.preedit.clear(),
        }
    }
    if !typing && !(composing.preedit.is_empty() && composing.committed.is_empty()) {
        *composing = Composing::default();
    }
}
