//! The in-game screens and the keys between them: the inventory (E), the character
//! screen (C), the journal (J), the quest book (L) and the menu (Esc). Each key opens its
//! screen from play or from another screen, and closes it again. One bar of the keys runs
//! along an edge of the window while any screen is open (`menu_bar.ron` says which edge and
//! which screens), the open screen's key lit; its keys can be clicked too. The keys are the
//! player's bindings (`Settings`).

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;

use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::settings::Settings;
use crate::ui::skin::Piece;
use crate::ui::widgets::{BODY, GOLD, MUTED, SHADOW, TEXT, UiFont, shadowed, text};
use crate::ui::widgets::space;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn_bar)
            .add_systems(OnExit(Screen::InGame), leave_game)
            .add_systems(Update, ((switch, click_hotkeys, label_hotkeys).chain(), clear_of_hud).in_set(ScreenSystems).run_if(in_state(Screen::InGame)));
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

    /// Its name in `menu_bar.ron`.
    fn name(self) -> &'static str {
        match self {
            GameScreen::Inventory => "inventory",
            GameScreen::Character => "character",
            GameScreen::Journal => "journal",
            GameScreen::Quests => "quests",
            GameScreen::Menu => "menu",
        }
    }

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

    /// The screen open, if one is.
    fn open(ui: &UiState) -> Option<GameScreen> {
        GameScreen::ALL.into_iter().find(|screen| screen.is_open(ui))
    }
}

pub const FILE: &str = "menu_bar.ron";

/// The window's edge the bar of keys runs along.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum Edge {
    Top,
    Bottom,
}

/// Where the bar of the screens' keys runs and what it shows (`menu_bar.ron`), read and
/// checked at launch.
#[derive(Resource, Clone, PartialEq, Debug)]
pub struct MenuBar {
    pub edge: Edge,
    /// The screens the bar shows, in order.
    pub screens: Vec<GameScreen>,
    /// The screens on which the keys that look an item up show.
    pub look_keys: Vec<GameScreen>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BarFile {
    edge: Option<Edge>,
    screens: Option<Vec<String>>,
    look_keys: Option<Vec<String>>,
}

impl MenuBar {
    /// Every package's `menu_bar.ron`, a later one's fields over an earlier's.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut all = BarFile::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let given: BarFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            all.edge = given.edge.or(all.edge);
            all.screens = given.screens.or(all.screens);
            all.look_keys = given.look_keys.or(all.look_keys);
        }
        let screens = |names: Option<Vec<String>>, field: &str| -> Result<Vec<GameScreen>, String> {
            let names = names.ok_or(format!("no package's {FILE} gives {field}"))?;
            let mut screens = Vec::new();
            for name in &names {
                let screen = GameScreen::ALL.into_iter().find(|s| s.name() == name).ok_or(format!("{FILE}: {field} names `{name}`, which is no screen"))?;
                if screens.contains(&screen) {
                    return Err(format!("{FILE}: {field} names `{name}` twice"));
                }
                screens.push(screen);
            }
            Ok(screens)
        };
        let bar = MenuBar { edge: all.edge.ok_or(format!("no package's {FILE} gives edge"))?, screens: screens(all.screens, "screens")?, look_keys: screens(all.look_keys, "look_keys")? };
        if bar.screens.is_empty() {
            return Err(format!("{FILE}: the bar shows no screen"));
        }
        Ok(bar)
    }
}

/// A screen's root, kept clear of the bar and of the HUD along the bottom.
#[derive(Component)]
pub struct ScreenRoot(pub GameScreen);

/// A panel of its own beside a screen, its top kept this far under the bar along the top
/// (or the window's top, the bar along the bottom).
#[derive(Component)]
pub struct UnderBar(pub f32);

/// Marks the root `parent` spawns into as `screen`'s, under the one bar of keys.
pub fn mark_screen(parent: &mut ChildSpawnerCommands, screen: GameScreen) {
    let root = parent.target_entity();
    parent.commands().entity(root).insert(ScreenRoot(screen));
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

/// The one bar of the screens' keys, along the edge `MenuBar` gives, the open screen's lit
/// in gold.
#[derive(Component)]
pub struct HotkeyBar;

/// The bar's screens' keys, in the middle.
#[derive(Component)]
struct BarKeys;

/// The bar's look-up keys, at its right.
#[derive(Component)]
struct BarLooks;

/// A key in the bar, which switches to its screen when clicked.
#[derive(Component)]
struct Hotkey(GameScreen);

/// A look-up key's hint in a bar: hover an item and press it.
#[derive(Component)]
pub struct LookKey;

/// The band's shade: the world shows through a little, darker than a screen's scrim.
const BAND: Color = Color::srgba(0.03, 0.035, 0.06, 0.82);

fn spawn_bar(mut commands: Commands, bar: Res<MenuBar>) {
    let band = Node {
        position_type: PositionType::Absolute,
        left: Val::ZERO,
        right: Val::ZERO,
        top: if bar.edge == Edge::Top { Val::ZERO } else { Val::Auto },
        bottom: if bar.edge == Edge::Bottom { Val::ZERO } else { Val::Auto },
        align_items: AlignItems::Center,
        padding: UiRect::axes(Val::Px(space::SECTION), Val::Px(space::HAIR)),
        border: if bar.edge == Edge::Top { UiRect::bottom(Val::Px(crate::ui::skin::PX)) } else { UiRect::top(Val::Px(crate::ui::skin::PX)) },
        ..default()
    };
    // The keys stand in the middle, between two equal sides; the look-up keys at the right.
    let side = || Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, justify_content: JustifyContent::FlexEnd, column_gap: Val::Px(space::SECTION), ..default() };
    commands.spawn((HotkeyBar, band, BackgroundColor(BAND), BorderColor::all(SHADOW), Visibility::Hidden, GlobalZIndex(3))).with_children(|band| {
        band.spawn(side());
        band.spawn((BarKeys, Node { justify_content: JustifyContent::Center, column_gap: Val::Px(space::INNER), flex_shrink: 0.0, ..default() }));
        band.spawn((BarLooks, side()));
    });
}

fn click_hotkeys(keys: Query<(&Hotkey, &Interaction), Changed<Interaction>>, mut ui: ResMut<UiState>) {
    for (key, interaction) in &keys {
        if *interaction == Interaction::Pressed && !key.0.is_open(&ui) {
            toggle(&mut ui, key.0);
        }
    }
}

/// Fills the bar with the current bindings, the open screen's lit: when another screen
/// opens and whenever the settings change.
#[allow(clippy::too_many_arguments)]
fn label_hotkeys(
    mut commands: Commands,
    ui: Res<UiState>,
    bar: Res<MenuBar>,
    settings: Res<Settings>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    keys: Query<Entity, With<BarKeys>>,
    looks: Query<Entity, With<BarLooks>>,
    mut shown: Local<Option<Option<GameScreen>>>,
) {
    let open = GameScreen::open(&ui);
    if *shown == Some(open) && !settings.is_changed() && !locale.is_changed() && !bar.is_changed() {
        return;
    }
    *shown = Some(open);
    let cap = || Node { padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)), min_width: Val::Px(crate::ui::skin::CAP_MIN), justify_content: JustifyContent::Center, ..default() };
    let pair = || Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() };
    if let Ok(keys) = keys.single() {
        commands.entity(keys).despawn_children().with_children(|row| {
            for &screen in &bar.screens {
                let current = Some(screen) == open;
                // A tab: the key's cap and the screen's name, the open one's underlined in gold.
                let tab = Node {
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(space::INNER),
                    padding: UiRect::axes(Val::Px(space::INNER), Val::Px(crate::ui::skin::PX)),
                    border: UiRect::bottom(Val::Px(crate::ui::skin::PX)),
                    ..default()
                };
                row.spawn((Button, Hotkey(screen), tab, BorderColor::all(if current { GOLD } else { Color::NONE }))).with_children(|key| {
                    let piece = if current { Piece::ButtonSelected } else { Piece::Button };
                    let label = shadowed(&font.0, screen.key_label(&settings, &locale), BODY, if current { GOLD } else { TEXT });
                    key.spawn((cap(), piece.image(), crate::ui::widgets::HoverLight)).with_child((label, crate::lang::TightLine));
                    key.spawn((Localized(screen.title()), text(&font.0, "", BODY, if current { GOLD } else { MUTED })));
                });
            }
        });
    }
    // Where items show that can be looked up, the keys that look them up.
    if let Ok(looks) = looks.single() {
        commands.entity(looks).despawn_children();
        if open.is_some_and(|screen| bar.look_keys.contains(&screen)) {
            commands.entity(looks).with_children(|row| {
                for (binding, title) in [(&settings.recipes_key, "screen.look.recipes"), (&settings.uses_key, "screen.look.uses")] {
                    row.spawn((LookKey, pair())).with_children(|key| {
                        key.spawn((cap(), Piece::Well.image())).with_child((shadowed(&font.0, binding.name(&locale), BODY, TEXT), crate::lang::TightLine));
                        key.spawn((Localized(title), text(&font.0, "", BODY, MUTED)));
                    });
                }
            });
        }
    }
}

/// Shows the bar while a screen is open, along its edge, and keeps every screen clear of it
/// and of what stays in sight along the bottom edge with a screen open (the hotbar, health,
/// the prayer strip and effects): each screen's root leaves that much room, however tall
/// the HUD there grows. The menu, the tallest screen, pauses the game instead and puts the
/// HUD away while it is up.
#[allow(clippy::type_complexity)]
fn clear_of_hud(
    ui: Res<UiState>,
    menu_bar: Res<MenuBar>,
    mut hud: Query<(&ComputedNode, &bevy::ui::UiGlobalTransform, &mut Visibility), (With<super::Bottom>, Without<HotkeyBar>)>,
    mut bar: Query<(&mut Node, &ComputedNode, &mut Visibility), (With<HotkeyBar>, Without<ScreenRoot>)>,
    mut roots: Query<(&ScreenRoot, &mut Node, &ComputedNode), Without<HotkeyBar>>,
    mut beside: Query<(&UnderBar, &mut Node), (Without<HotkeyBar>, Without<ScreenRoot>)>,
) {
    let Ok((bottom, at, mut shown)) = hud.single_mut() else { return };
    shown.set_if_neq(if ui.menu_open { Visibility::Hidden } else { Visibility::Inherited });
    let Ok((mut band, band_size, mut band_shown)) = bar.single_mut() else { return };
    let open = GameScreen::open(&ui);
    band_shown.set_if_neq(if open.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    let scale = bottom.inverse_scale_factor();
    let top = at.translation.y - bottom.size().y / 2.0;
    let tall = band_size.size().y * band_size.inverse_scale_factor();
    // Written only when it moves, so a screen standing still is not laid out again.
    let moved = |val: Val, px: f32| !matches!(val, Val::Px(now) if (now - px).abs() < 0.5);
    for (screen, mut node, root) in &mut roots {
        // From the HUD's top to the foot of the screen, in interface pixels, and a gap.
        let hud_room = if screen.0 == GameScreen::Menu { 0.0 } else { ((root.size().y - top) * scale).max(0.0) + space::GAP };
        if menu_bar.edge == Edge::Bottom && open == Some(screen.0) && moved(band.bottom, hud_room) {
            band.bottom = Val::Px(hud_room);
        }
        let (above, below) = match menu_bar.edge {
            Edge::Top => (tall + space::HAIR, hud_room),
            Edge::Bottom => (space::SECTION, hud_room + tall + space::HAIR),
        };
        if moved(node.padding.top, above) {
            node.padding.top = Val::Px(above);
        }
        if moved(node.padding.bottom, below) {
            node.padding.bottom = Val::Px(below);
        }
    }
    for (under, mut node) in &mut beside {
        let top = under.0 + if menu_bar.edge == Edge::Top { tall } else { 0.0 };
        if moved(node.top, top) {
            node.top = Val::Px(top);
        }
    }
}

/// Out of a game the bar goes, and the menu, shown on the title too, has its room back.
fn leave_game(mut bar: Query<&mut Visibility, With<HotkeyBar>>, mut roots: Query<(&ScreenRoot, &mut Node)>) {
    for mut shown in &mut bar {
        shown.set_if_neq(Visibility::Hidden);
    }
    for (screen, mut node) in &mut roots {
        if screen.0 == GameScreen::Menu {
            (node.padding.top, node.padding.bottom) = (Val::ZERO, Val::ZERO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    fn shipped() -> ContentFile {
        let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core").join(FILE)).unwrap();
        file("core", &source)
    }

    #[test]
    fn the_bar_runs_along_the_top_with_every_screen() {
        let bar = MenuBar::load(&[shipped()]).expect("the shipped bar loads");
        assert_eq!(bar.edge, Edge::Top);
        assert_eq!(bar.screens, GameScreen::ALL.to_vec());
        assert_eq!(bar.look_keys, vec![GameScreen::Inventory, GameScreen::Quests]);
    }

    #[test]
    fn a_package_moves_the_bar_and_its_screens() {
        let bar = MenuBar::load(&[shipped(), file("mine", r#"(edge: Bottom, screens: ["quests", "inventory"])"#)]).unwrap();
        assert_eq!((bar.edge, bar.screens), (Edge::Bottom, vec![GameScreen::Quests, GameScreen::Inventory]));
        assert_eq!(bar.look_keys, vec![GameScreen::Inventory, GameScreen::Quests], "a field left out is kept");
    }

    #[test]
    fn a_bad_bar_is_refused_with_why() {
        for (bad, why) in [
            (r#"(screens: ["map"])"#, "`map`, which is no screen"),
            (r#"(screens: ["quests", "quests"])"#, "`quests` twice"),
            (r#"(screens: [])"#, "the bar shows no screen"),
            (r#"(look_keys: ["shop"])"#, "`shop`, which is no screen"),
            (r#"(edge: Left)"#, "Left"),
            (r#"(edges: Top)"#, "edges"),
        ] {
            let error = MenuBar::load(&[shipped(), file("bad", bad)]).err().unwrap_or_else(|| panic!("{bad} loaded"));
            assert!(error.contains(why), "{bad}: {error}");
        }
        assert!(MenuBar::load(&[]).unwrap_err().contains("gives edge"));
    }
}
