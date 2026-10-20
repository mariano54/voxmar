//! The in-game screens and the keys between them: the inventory (E), the character
//! screen (C), the journal (J), the quest book (L) and the menu (Esc). Each key opens its screen from play or from another
//! screen, and closes it again; every screen shows the keys to the others along its foot,
//! and the keys can be clicked too. The keys are the player's bindings (`Settings`).

use bevy::prelude::*;

use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::settings::Settings;
use crate::ui::skin::Piece;
use crate::ui::widgets::{BODY, GOLD, MUTED, TEXT, UiFont, shadowed, text};
use crate::ui::widgets::space;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, ((switch, click_hotkeys, label_hotkeys).chain(), clear_of_hud).in_set(ScreenSystems).run_if(in_state(Screen::InGame)));
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
    Journal,
    Quests,
    Menu,
}

impl GameScreen {
    const ALL: [GameScreen; 5] = [GameScreen::Inventory, GameScreen::Character, GameScreen::Journal, GameScreen::Quests, GameScreen::Menu];

    /// The key of its name (see `lang`).
    fn title(self) -> &'static str {
        match self {
            GameScreen::Inventory => "screen.inventory",
            GameScreen::Character => "screen.character",
            GameScreen::Journal => "screen.journal",
            GameScreen::Quests => "screen.quests",
            GameScreen::Menu => "screen.menu",
        }
    }

    fn key_label(self, settings: &Settings, locale: &Locale) -> String {
        match self {
            GameScreen::Inventory => settings.inventory_key.name(locale),
            GameScreen::Character => settings.character_key.name(locale),
            GameScreen::Journal => settings.journal_key.name(locale),
            GameScreen::Quests => settings.quests_key.name(locale),
            GameScreen::Menu => crate::keys::escape_name(locale),
        }
    }

    fn is_open(self, ui: &UiState) -> bool {
        match self {
            GameScreen::Inventory => ui.inventory_open,
            GameScreen::Character => ui.character_open,
            GameScreen::Journal => ui.journal_open,
            GameScreen::Quests => ui.quests_open,
            GameScreen::Menu => ui.menu_open,
        }
    }
}

/// Opens `screen` (closing any other), or closes it if it is the one open.
fn toggle(ui: &mut UiState, screen: GameScreen) {
    let open = !screen.is_open(ui);
    ui.inventory_open = open && screen == GameScreen::Inventory;
    ui.character_open = open && screen == GameScreen::Character;
    ui.journal_open = open && screen == GameScreen::Journal;
    ui.quests_open = open && screen == GameScreen::Quests;
    ui.menu_open = open && screen == GameScreen::Menu;
}

/// The inventory and character keys, from play or from any screen. Esc is the menu's,
/// handled with the pointer grab in `player`.
fn switch(keys: Res<ButtonInput<KeyCode>>, settings: Res<Settings>, player: Res<LocalPlayer>, mut ui: ResMut<UiState>) {
    if player.dead {
        ui.inventory_open = false;
        ui.character_open = false;
        ui.journal_open = false;
        ui.quests_open = false;
        return;
    }
    if ui.console_open {
        return;
    }
    if keys.just_pressed(settings.inventory_key.0) {
        toggle(&mut ui, GameScreen::Inventory);
    } else if keys.just_pressed(settings.character_key.0) {
        toggle(&mut ui, GameScreen::Character);
    } else if keys.just_pressed(settings.journal_key.0) {
        toggle(&mut ui, GameScreen::Journal);
    } else if keys.just_pressed(settings.quests_key.0) {
        toggle(&mut ui, GameScreen::Quests);
    }
}

/// The keys along a screen's foot, the screen's own lit in gold.
#[derive(Component)]
pub struct HotkeyBar(pub GameScreen);

/// A key in the bar, which switches to its screen when clicked.
#[derive(Component)]
struct Hotkey(GameScreen);

/// A look-up key's hint in a bar: hover an item and press it.
#[derive(Component)]
pub struct LookKey;

/// A bar of the in-game screens' keys, for the foot of `current`.
pub fn spawn_hotkeys(parent: &mut ChildSpawnerCommands, current: GameScreen) -> Entity {
    let bar = Node { margin: UiRect::top(Val::Px(space::GAP)), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: Val::Px(space::HAIR), ..default() };
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
    locale: Res<Locale>,
    font: Res<UiFont>,
    bars: Query<(Entity, &HotkeyBar, Ref<HotkeyBar>)>,
) {
    for (entity, bar, added) in &bars {
        if !added.is_added() && !settings.is_changed() && !locale.is_changed() {
            continue;
        }
        let keys = || Node { justify_content: JustifyContent::Center, column_gap: Val::Px(space::SECTION), ..default() };
        let cap = || Node { padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)), min_width: Val::Px(crate::ui::skin::CAP_MIN), justify_content: JustifyContent::Center, ..default() };
        let pair = || Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() };
        commands.entity(entity).despawn_children().with_children(|rows| {
            rows.spawn(keys()).with_children(|row| {
                for screen in GameScreen::ALL {
                    let current = screen == bar.0;
                    row.spawn((Button, Hotkey(screen), pair())).with_children(|key| {
                        let piece = if current { Piece::ButtonSelected } else { Piece::Button };
                        let label = shadowed(&font.0, screen.key_label(&settings, &locale), BODY, if current { GOLD } else { TEXT });
                        key.spawn((cap(), piece.image(), crate::ui::widgets::HoverLight)).with_child((label, crate::lang::TightLine));
                        key.spawn((Localized(screen.title()), text(&font.0, "", BODY, if current { GOLD } else { MUTED })));
                    });
                }
            });
            // Where items show that can be looked up, the keys that look them up, on a row
            // of their own under the screens'.
            if matches!(bar.0, GameScreen::Inventory | GameScreen::Quests) {
                rows.spawn(keys()).with_children(|row| {
                    for (binding, title) in [(&settings.recipes_key, "screen.look.recipes"), (&settings.uses_key, "screen.look.uses")] {
                        row.spawn((LookKey, pair())).with_children(|key| {
                            key.spawn((cap(), Piece::Well.image())).with_child((shadowed(&font.0, binding.name(&locale), BODY, TEXT), crate::lang::TightLine));
                            key.spawn((Localized(title), text(&font.0, "", BODY, MUTED)));
                        });
                    }
                });
            }
        });
    }
}

/// Keeps every screen, and the keys under it, clear of what stays in sight along the bottom
/// edge with a screen open (the hotbar, health, the prayer strip and effects): each screen's
/// root leaves that much room at its foot, however tall the HUD there grows. The menu, the
/// tallest screen, pauses the game instead and puts the HUD away while it is up.
fn clear_of_hud(
    ui: Res<UiState>,
    mut hud: Query<(&ComputedNode, &bevy::ui::UiGlobalTransform, &mut Visibility), With<super::Bottom>>,
    bars: Query<(&HotkeyBar, &ChildOf)>,
    mut roots: Query<(&mut Node, &ComputedNode)>,
) {
    let Ok((bottom, at, mut shown)) = hud.single_mut() else { return };
    shown.set_if_neq(if ui.menu_open { Visibility::Hidden } else { Visibility::Inherited });
    let scale = bottom.inverse_scale_factor();
    let top = at.translation.y - bottom.size().y / 2.0;
    for (bar, root) in &bars {
        let Ok((mut node, root)) = roots.get_mut(root.parent()) else { continue };
        // From the HUD's top to the foot of the screen, in interface pixels, and a gap.
        let room = if bar.0 == GameScreen::Menu { 0.0 } else { ((root.size().y - top) * scale).max(0.0) + space::GAP };
        if !matches!(node.padding.bottom, Val::Px(now) if (now - room).abs() < 0.5) {
            node.padding.bottom = Val::Px(room);
        }
    }
}
