//! The in-game screens and the keys between them: the inventory (E), the character
//! screen (C) and the menu (Esc). Each key opens its screen from play or from another
//! screen, and closes it again; every screen shows the keys to the others along its foot,
//! and the keys can be clicked too. The keys are the player's bindings (`Settings`).

use bevy::prelude::*;

use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{BODY, GOLD, MUTED, TEXT, UiFont, shadowed, text};

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

    fn title(self) -> &'static str {
        match self {
            GameScreen::Inventory => "Inventory",
            GameScreen::Character => "Character",
            GameScreen::Menu => "Menu",
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
    let bar = Node {
        position_type: PositionType::Absolute,
        bottom: Val::Px(6.0 * PX),
        width: Val::Percent(100.0),
        justify_content: JustifyContent::Center,
        column_gap: Val::Px(8.0 * PX),
        ..default()
    };
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
                let node = Node { align_items: AlignItems::Center, column_gap: Val::Px(3.0 * PX), ..default() };
                row.spawn((Button, Hotkey(screen), node)).with_children(|key| {
                    let cap = Node { padding: UiRect::new(Val::Px(3.0 * PX), Val::Px(3.0 * PX), Val::Px(PX), Val::Px(2.0 * PX)), ..default() };
                    let piece = if current { Piece::ButtonSelected } else { Piece::Button };
                    key.spawn((cap, piece.image())).with_child(shadowed(&font.0, screen.key_label(&settings), BODY, if current { GOLD } else { TEXT }));
                    key.spawn(text(&font.0, screen.title(), BODY, if current { GOLD } else { MUTED }));
                });
            }
        });
    }
}
