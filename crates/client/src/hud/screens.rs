//! The in-game screens and the keys between them: the inventory (E), the character
//! screen (C) and the menu (Esc). Each key opens its screen from play or from another
//! screen, and closes it again; every screen shows the keys to the others along its foot,
//! and the keys can be clicked too. The keys are the player's bindings (`Settings`).

use bevy::prelude::*;

use crate::lang::Localized;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::settings::Settings;
use crate::ui::skin::Piece;
use crate::ui::widgets::{BODY, GOLD, MUTED, TEXT, UiFont, shadowed, text};
use crate::ui::widgets::space;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (switch, click_hotkeys, label_hotkeys).chain().in_set(ScreenSystems).run_if(in_state(Screen::InGame)));
    }
}

/// Opening and closing the in-game screens; the screens themselves run after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScreenSystems;

/// One of the screens a key opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameScreen {
    Inventory,
    Character,
    Menu,
}

impl GameScreen {
    const ALL: [GameScreen; 3] = [GameScreen::Inventory, GameScreen::Character, GameScreen::Menu];

    /// The key of its name (see `lang`).
    fn title(self) -> &'static str {
        match self {
            GameScreen::Inventory => "screen.inventory",
            GameScreen::Character => "screen.character",
            GameScreen::Menu => "screen.menu",
        }
    }

    fn key_label(self, settings: &Settings) -> &'static str {
        match self {
            GameScreen::Inventory => settings.inventory_key.label(),
            GameScreen::Character => settings.character_key.label(),
            GameScreen::Menu => "Esc",
        }
    }

    fn is_open(self, ui: &UiState) -> bool {
        match self {
            GameScreen::Inventory => ui.inventory_open,
            GameScreen::Character => ui.character_open,
            GameScreen::Menu => ui.menu_open,
        }
    }
}

/// Opens `screen` (closing any other), or closes it if it is the one open.
fn toggle(ui: &mut UiState, screen: GameScreen) {
    let open = !screen.is_open(ui);
    ui.inventory_open = open && screen == GameScreen::Inventory;
    ui.character_open = open && screen == GameScreen::Character;
    ui.menu_open = open && screen == GameScreen::Menu;
}

/// The inventory and character keys, from play or from any screen. Esc is the menu's,
/// handled with the pointer grab in `player`.
fn switch(keys: Res<ButtonInput<KeyCode>>, settings: Res<Settings>, player: Res<LocalPlayer>, mut ui: ResMut<UiState>) {
    if player.dead {
        ui.inventory_open = false;
        ui.character_open = false;
        return;
    }
    if ui.console_open {
        return;
    }
    if keys.just_pressed(settings.inventory_key.0) {
        toggle(&mut ui, GameScreen::Inventory);
    } else if keys.just_pressed(settings.character_key.0) {
        toggle(&mut ui, GameScreen::Character);
    }
}

/// The keys along a screen's foot, the screen's own lit in gold.
#[derive(Component)]
pub struct HotkeyBar(pub GameScreen);

/// A key in the bar, which switches to its screen when clicked.
#[derive(Component)]
struct Hotkey(GameScreen);

/// A bar of the in-game screens' keys, for the foot of `current`.
pub fn spawn_hotkeys(parent: &mut ChildSpawnerCommands, current: GameScreen) -> Entity {
    let bar = Node { margin: UiRect::top(Val::Px(space::GAP)), justify_content: JustifyContent::Center, column_gap: Val::Px(space::SECTION), ..default() };
    parent.spawn((HotkeyBar(current), bar)).id()
}

fn click_hotkeys(keys: Query<(&Hotkey, &Interaction), Changed<Interaction>>, mut ui: ResMut<UiState>) {
    for (key, interaction) in &keys {
        if *interaction == Interaction::Pressed && !key.0.is_open(&ui) {
            toggle(&mut ui, key.0);
        }
    }
}

/// Fills the bars with the current bindings: when they are spawned and whenever the
/// settings change.
fn label_hotkeys(
    mut commands: Commands,
    settings: Res<Settings>,
    font: Res<UiFont>,
    bars: Query<(Entity, &HotkeyBar, Ref<HotkeyBar>)>,
) {
    for (entity, bar, added) in &bars {
        if !added.is_added() && !settings.is_changed() {
            continue;
        }
        commands.entity(entity).despawn_children().with_children(|row| {
            for screen in GameScreen::ALL {
                let current = screen == bar.0;
                let node = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() };
                row.spawn((Button, Hotkey(screen), node)).with_children(|key| {
                    let cap = Node { padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)), min_width: Val::Px(crate::ui::skin::CAP_MIN), justify_content: JustifyContent::Center, ..default() };
                    let piece = if current { Piece::ButtonSelected } else { Piece::Button };
                    let label = shadowed(&font.0, screen.key_label(&settings), BODY, if current { GOLD } else { TEXT });
                    key.spawn((cap, piece.image(), crate::ui::widgets::HoverLight)).with_child((label, crate::lang::TightLine));
                    key.spawn((Localized(screen.title()), text(&font.0, "", BODY, if current { GOLD } else { MUTED })));
                });
            }
        });
    }
}
