//! Player settings: the menu Esc opens, applying each choice to the game, and keeping them
//! in `voxmar/settings.ron` in the platform's configuration directory.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};
use voxmar_engine::coords::ColumnPos;
use voxmar_protocol::MAX_FLY_SPEED;

use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::render::DayCycle;
use crate::render::lod::{self, Lod, LodMaterial};
use crate::world::ClientWorld;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn_menu).add_systems(
            Update,
            (
                toggle_debug,
                (drag_sliders, click_switches, click_buttons, sync_controls.run_if(resource_changed::<Settings>), show_menu)
                    .chain(),
                apply.run_if(resource_changed::<Settings>),
                fit_lod_hole,
                sync_server,
            ),
        );
    }
}

/// The player's choices. Fields missing from the file keep their defaults.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Radius of real chunks around the player, in columns.
    pub view_distance: i32,
    pub vsync: bool,
    /// Vertical field of view, in degrees.
    pub fov: i32,
    /// Mouse look speed, in percent.
    pub sensitivity: i32,
    /// Music volume, in percent.
    pub music_volume: i32,
    /// Sound effects volume, in percent.
    pub sound_volume: i32,
    pub debug_overlay: bool,
    /// Flight speed, in percent. Only admins may fly faster than normal.
    pub fly_speed: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            view_distance: 16,
            vsync: true,
            fov: 75,
            sensitivity: 100,
            music_volume: 100,
            sound_volume: 100,
            debug_overlay: false,
            fly_speed: 100,
        }
    }
}

impl Settings {
    /// The saved settings, or the defaults if there are none or they cannot be read.
    pub fn load() -> Self {
        let Some(path) = path() else { return Self::default() };
        let Ok(source) = std::fs::read_to_string(&path) else { return Self::default() };
        match ron::from_str::<Settings>(&source) {
            Ok(settings) => settings.clamped(),
            Err(e) => {
                warn!("ignoring {}: {e}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            std::fs::create_dir_all(path.parent().expect("settings file is in a directory"))?;
            std::fs::write(&path, ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?)?;
            Ok(())
        };
        if let Err(e) = write() {
            warn!("could not save {}: {e}", path.display());
        }
    }

    /// The settings with every value within its slider's range.
    pub fn clamped(mut self) -> Self {
        for slider in Slider::ALL {
            let (min, max, _) = slider.range();
            let value = slider.get(&self).clamp(min, max);
            slider.set(&mut self, value);
        }
        self
    }
}

/// `voxmar/settings.ron` in the platform's configuration directory.
fn path() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support")
    } else if cfg!(windows) {
        var("APPDATA")?
    } else {
        var("XDG_CONFIG_HOME").or_else(|| Some(var("HOME")?.join(".config")))?
    };
    Some(base.join("voxmar").join("settings.ron"))
}

/// A setting chosen with a slider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slider {
    ViewDistance,
    Fov,
    Sensitivity,
    Music,
    Sounds,
    FlySpeed,
}

impl Slider {
    const ALL: [Slider; 6] = [Slider::ViewDistance, Slider::Fov, Slider::Sensitivity, Slider::Music, Slider::Sounds, Slider::FlySpeed];

    fn label(self) -> &'static str {
        match self {
            Slider::ViewDistance => "Render distance",
            Slider::Fov => "Field of view",
            Slider::Sensitivity => "Mouse sensitivity",
            Slider::Music => "Music",
            Slider::Sounds => "Sound effects",
            Slider::FlySpeed => "Fly speed",
        }
    }

    /// Lowest value, highest value and step.
    fn range(self) -> (i32, i32, i32) {
        match self {
            Slider::ViewDistance => (4, 32, 2),
            Slider::Fov => (60, 110, 5),
            Slider::Sensitivity => (25, 300, 5),
            Slider::Music | Slider::Sounds => (0, 100, 10),
            Slider::FlySpeed => (100, (MAX_FLY_SPEED * 100.0) as i32, 25),
        }
    }

    fn get(self, settings: &Settings) -> i32 {
        match self {
            Slider::ViewDistance => settings.view_distance,
            Slider::Fov => settings.fov,
            Slider::Sensitivity => settings.sensitivity,
            Slider::Music => settings.music_volume,
            Slider::Sounds => settings.sound_volume,
            Slider::FlySpeed => settings.fly_speed,
        }
    }

    fn set(self, settings: &mut Settings, value: i32) {
        *match self {
            Slider::ViewDistance => &mut settings.view_distance,
            Slider::Fov => &mut settings.fov,
            Slider::Sensitivity => &mut settings.sensitivity,
            Slider::Music => &mut settings.music_volume,
            Slider::Sounds => &mut settings.sound_volume,
            Slider::FlySpeed => &mut settings.fly_speed,
        } = value;
    }

    fn text(self, value: i32) -> String {
        match self {
            Slider::ViewDistance => format!("{value} chunks"),
            Slider::Fov => format!("{value}°"),
            Slider::Music | Slider::Sounds if value == 0 => "Off".into(),
            Slider::Sensitivity | Slider::Music | Slider::Sounds => format!("{value}%"),
            Slider::FlySpeed => format!("{}×", value as f32 / 100.0),
        }
    }
}

/// A setting switched on or off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Toggle {
    Vsync,
    DebugOverlay,
}

impl Toggle {
    fn label(self) -> &'static str {
        match self {
            Toggle::Vsync => "VSync",
            Toggle::DebugOverlay => "Debug overlay",
        }
    }

    fn get(self, settings: &Settings) -> bool {
        match self {
            Toggle::Vsync => settings.vsync,
            Toggle::DebugOverlay => settings.debug_overlay,
        }
    }

    fn set(self, settings: &mut Settings, on: bool) {
        *match self {
            Toggle::Vsync => &mut settings.vsync,
            Toggle::DebugOverlay => &mut settings.debug_overlay,
        } = on;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Slider(Slider),
    Switch(Toggle),
}

const SECTIONS: [(&str, &[Control]); 3] = [
    (
        "Video",
        &[
            Control::Slider(Slider::ViewDistance),
            Control::Slider(Slider::Fov),
            Control::Switch(Toggle::Vsync),
            Control::Switch(Toggle::DebugOverlay),
        ],
    ),
    ("Audio", &[Control::Slider(Slider::Music), Control::Slider(Slider::Sounds)]),
    ("Controls", &[Control::Slider(Slider::Sensitivity), Control::Slider(Slider::FlySpeed)]),
];

#[derive(Component, Clone, Copy)]
enum MenuButton {
    Resume,
    Quit,
}

impl MenuButton {
    fn label(self) -> &'static str {
        match self {
            MenuButton::Resume => "Resume",
            MenuButton::Quit => "Quit game",
        }
    }

    /// Text colour, background and hovered background.
    fn colors(self) -> (Color, Color, Color) {
        match self {
            MenuButton::Resume => (ON_ACCENT, ACCENT, ACCENT_HOVER),
            MenuButton::Quit => (TEXT, SUBTLE, SUBTLE_HOVER),
        }
    }
}

#[derive(Component)]
struct MenuRoot;
/// The draggable area of a slider.
#[derive(Component)]
struct SliderTrack(Slider);
#[derive(Component)]
struct SliderFill(Slider);
#[derive(Component)]
struct SliderValue(Slider);
#[derive(Component)]
struct Switch(Toggle);
/// Rows only admins see.
#[derive(Component)]
struct AdminOnly;

const PANEL_WIDTH: f32 = 520.0;
const SLIDER_WIDTH: f32 = 200.0;
const VALUE_WIDTH: f32 = 84.0;
const KNOB: f32 = 16.0;

const SCRIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.5);
const PANEL: Color = Color::srgb(0.08, 0.09, 0.11);
const OUTLINE: Color = Color::srgba(1.0, 1.0, 1.0, 0.07);
const TEXT: Color = Color::srgb(0.93, 0.94, 0.96);
const SOFT: Color = Color::srgba(0.93, 0.94, 0.96, 0.7);
const MUTED: Color = Color::srgba(0.93, 0.94, 0.96, 0.45);
const ACCENT: Color = Color::srgb(0.36, 0.78, 0.5);
const ACCENT_HOVER: Color = Color::srgb(0.46, 0.85, 0.59);
const ON_ACCENT: Color = Color::srgb(0.04, 0.1, 0.06);
const TRACK: Color = Color::srgba(1.0, 1.0, 1.0, 0.14);
const SUBTLE: Color = Color::srgba(1.0, 1.0, 1.0, 0.08);
const SUBTLE_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.14);

/// A clean sans-serif from the system; Bevy's built-in font is monospaced and ASCII only.
fn system_font(fonts: &mut Assets<Font>) -> Handle<Font> {
    const CANDIDATES: [&str; 5] = [
        "/System/Library/Fonts/SFNS.ttf",
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    CANDIDATES
        .iter()
        .find_map(|path| std::fs::read(path).ok())
        .map_or_else(Handle::default, |bytes| fonts.add(Font::from_bytes(bytes)))
}

fn text(font: &Handle<Font>, value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    (Text::new(value), TextFont { font: font.into(), font_size: FontSize::Px(size), ..default() }, TextColor(color))
}

fn spawn_menu(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let font = &system_font(&mut fonts);
    let root = Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    };
    let panel = Node {
        width: Val::Px(PANEL_WIDTH),
        flex_direction: FlexDirection::Column,
        padding: UiRect::new(Val::Px(36.0), Val::Px(36.0), Val::Px(28.0), Val::Px(32.0)),
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(18.0)),
        ..default()
    };
    let shadow = BoxShadow::new(Color::srgba(0.0, 0.0, 0.0, 0.45), Val::ZERO, Val::Px(16.0), Val::ZERO, Val::Px(48.0));
    commands.spawn((MenuRoot, root, BackgroundColor(SCRIM), GlobalZIndex(1), Visibility::Hidden)).with_children(|root| {
        root.spawn((panel, BackgroundColor(PANEL), BorderColor::all(OUTLINE), shadow)).with_children(|panel| {
            let mut title = text(font, "Settings", 30.0, TEXT);
            title.1.weight = FontWeight::SEMIBOLD;
            panel.spawn(title);
            for (section, controls) in SECTIONS {
                let margin = UiRect::new(Val::ZERO, Val::ZERO, Val::Px(18.0), Val::Px(2.0));
                panel.spawn((Node { margin, ..default() }, text(font, section.to_uppercase(), 12.0, MUTED)));
                for &control in controls {
                    spawn_row(panel, font, control);
                }
            }
            let buttons = Node { margin: UiRect::top(Val::Px(26.0)), column_gap: Val::Px(12.0), ..default() };
            panel.spawn(buttons).with_children(|row| {
                for button in [MenuButton::Resume, MenuButton::Quit] {
                    let (color, background, _) = button.colors();
                    let node = Node {
                        flex_grow: 1.0,
                        flex_basis: Val::ZERO,
                        height: Val::Px(44.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        ..default()
                    };
                    let mut label = text(font, button.label(), 16.0, color);
                    label.1.weight = FontWeight::MEDIUM;
                    row.spawn((Button, button, node, BackgroundColor(background))).with_child(label);
                }
            });
        });
    });
}

/// A label on the left and its control on the right.
fn spawn_row(panel: &mut ChildSpawnerCommands, font: &Handle<Font>, control: Control) {
    let label = match control {
        Control::Slider(slider) => slider.label(),
        Control::Switch(toggle) => toggle.label(),
    };
    let node = Node {
        height: Val::Px(38.0),
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        ..default()
    };
    let mut row = panel.spawn(node);
    if control == Control::Slider(Slider::FlySpeed) {
        row.insert(AdminOnly);
    }
    row.with_children(|row| {
        row.spawn(text(font, label, 16.0, TEXT));
        match control {
            Control::Slider(slider) => spawn_slider(row, font, slider),
            Control::Switch(toggle) => spawn_switch(row, toggle),
        }
    });
}

/// A thin track filled up to the value, with a round knob at its end, and the value.
fn spawn_slider(row: &mut ChildSpawnerCommands, font: &Handle<Font>, slider: Slider) {
    let rounded = |radius: f32| BorderRadius::all(Val::Px(radius));
    let area = Node { width: Val::Px(SLIDER_WIDTH), height: Val::Px(24.0), align_items: AlignItems::Center, ..default() };
    let track = Node { width: Val::Percent(100.0), height: Val::Px(4.0), border_radius: rounded(2.0), ..default() };
    let fill = Node { height: Val::Percent(100.0), border_radius: rounded(2.0), ..default() };
    let knob = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(-KNOB / 2.0),
        top: Val::Px((4.0 - KNOB) / 2.0),
        width: Val::Px(KNOB),
        height: Val::Px(KNOB),
        border_radius: rounded(KNOB / 2.0),
        ..default()
    };
    let value = Node { width: Val::Px(VALUE_WIDTH), justify_content: JustifyContent::FlexEnd, ..default() };
    row.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(16.0), ..default() }).with_children(|control| {
        control.spawn((Button, SliderTrack(slider), RelativeCursorPosition::default(), area)).with_children(|area| {
            area.spawn((track, BackgroundColor(TRACK))).with_children(|track| {
                track.spawn((SliderFill(slider), fill, BackgroundColor(ACCENT))).with_child((knob, BackgroundColor(Color::WHITE)));
            });
        });
        control.spawn(value).with_child((SliderValue(slider), text(font, "", 15.0, SOFT)));
    });
}

/// A pill whose knob sits right and lights up when on.
fn spawn_switch(row: &mut ChildSpawnerCommands, toggle: Toggle) {
    let track = Node {
        width: Val::Px(44.0),
        height: Val::Px(24.0),
        padding: UiRect::all(Val::Px(3.0)),
        border_radius: BorderRadius::MAX,
        ..default()
    };
    let knob = Node { width: Val::Px(18.0), height: Val::Px(18.0), border_radius: BorderRadius::MAX, ..default() };
    row.spawn((Button, Switch(toggle), track, BackgroundColor(TRACK))).with_child((knob, BackgroundColor(Color::WHITE)));
}

/// F3 toggles the debug overlay, menu or not.
fn toggle_debug(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<Settings>) {
    if keys.just_pressed(KeyCode::F3) {
        settings.debug_overlay = !settings.debug_overlay;
    }
}

/// Sets a slider's value from the cursor while it is held, snapped to the slider's steps.
fn drag_sliders(mut settings: ResMut<Settings>, sliders: Query<(&SliderTrack, &Interaction, &RelativeCursorPosition)>) {
    for (slider, interaction, cursor) in &sliders {
        let (Interaction::Pressed, Some(cursor)) = (interaction, cursor.normalized) else { continue };
        let (min, max, step) = slider.0.range();
        let along = (cursor.x + 0.5).clamp(0.0, 1.0);
        let value = (min + (along * (max - min) as f32 / step as f32).round() as i32 * step).min(max);
        if slider.0.get(&settings) != value {
            slider.0.set(&mut settings, value);
        }
    }
}

fn click_switches(mut settings: ResMut<Settings>, switches: Query<(&Switch, &Interaction), Changed<Interaction>>) {
    for (switch, interaction) in &switches {
        if *interaction == Interaction::Pressed {
            let on = switch.0.get(&settings);
            switch.0.set(&mut settings, !on);
        }
    }
}

fn click_buttons(
    mut buttons: Query<(&MenuButton, &Interaction, &mut BackgroundColor), Changed<Interaction>>,
    mut ui: ResMut<UiState>,
    settings: Res<Settings>,
    mut exit: MessageWriter<AppExit>,
) {
    for (&button, interaction, mut background) in &mut buttons {
        let (_, normal, hovered) = button.colors();
        background.0 = if *interaction == Interaction::None { normal } else { hovered };
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Resume => ui.menu_open = false,
            MenuButton::Quit => {
                settings.save();
                exit.write(AppExit::Success);
            }
        }
    }
}

/// Mirrors the settings in the menu's controls.
fn sync_controls(
    settings: Res<Settings>,
    mut fills: Query<(&SliderFill, &mut Node), Without<Switch>>,
    mut values: Query<(&SliderValue, &mut Text)>,
    mut switches: Query<(&Switch, &mut Node, &mut BackgroundColor)>,
) {
    for (fill, mut node) in &mut fills {
        let (min, max, _) = fill.0.range();
        node.width = Val::Percent((fill.0.get(&settings) - min) as f32 / (max - min) as f32 * 100.0);
    }
    for (value, mut text) in &mut values {
        text.0 = value.0.text(value.0.get(&settings));
    }
    for (switch, mut node, mut background) in &mut switches {
        let on = switch.0.get(&settings);
        node.justify_content = if on { JustifyContent::FlexEnd } else { JustifyContent::FlexStart };
        background.0 = if on { ACCENT } else { TRACK };
    }
}

/// Shows the menu while it is open, with admin-only rows for admins, and saves the
/// settings when it closes.
fn show_menu(
    ui: Res<UiState>,
    settings: Res<Settings>,
    player: Res<LocalPlayer>,
    mut was_open: Local<bool>,
    mut root: Query<&mut Visibility, With<MenuRoot>>,
    mut admin_rows: Query<&mut Node, With<AdminOnly>>,
) {
    if *was_open && !ui.menu_open {
        settings.save();
    }
    *was_open = ui.menu_open;
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.menu_open { Visibility::Inherited } else { Visibility::Hidden });
    }
    let display = if player.admin { Display::Flex } else { Display::None };
    for mut node in &mut admin_rows {
        if node.display != display {
            node.display = display;
        }
    }
}

/// Applies the settings other modules do not read for themselves.
fn apply(
    settings: Res<Settings>,
    mut world: ResMut<ClientWorld>,
    mut cycle: ResMut<DayCycle>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut camera: Query<&mut Projection, With<Camera3d>>,
) {
    if world.view_distance != settings.view_distance {
        world.set_view_distance(settings.view_distance);
        cycle.view_distance = lod::view_distance(settings.view_distance);
    }
    let present_mode = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if let Ok(mut window) = window.single_mut()
        && window.present_mode != present_mode
    {
        window.present_mode = present_mode;
    }
    if let Ok(mut projection) = camera.single_mut()
        && let Projection::Perspective(perspective) = &mut *projection
    {
        perspective.fov = (settings.fov as f32).to_radians();
    }
}

/// Fits the distant terrain's hole for real chunks to the render distance: at once when it
/// shrinks, and ring by ring as columns arrive when it grows, so that raising the render
/// distance never opens a gap in the terrain.
fn fit_lod_hole(
    world: Res<ClientWorld>,
    mut lod: ResMut<Lod>,
    mut materials: ResMut<Assets<LodMaterial>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let current = lod.real_columns();
    if current == world.view_distance {
        return;
    }
    let Ok(camera) = camera.single() else { return };
    let center = ColumnPos::of_block(camera.translation().floor().as_ivec3());
    // The hole covers the columns closer than `columns`, which mesh once the ring at
    // `columns` has arrived.
    let ring_arrived = |ring: i32| {
        (-ring..=ring).all(|i| {
            [(i, -ring), (i, ring), (-ring, i), (ring, i)].into_iter().all(|(dx, dz)| world.voxels.contains(center.offset(dx, dz)))
        })
    };
    let mut columns = current.min(world.view_distance);
    while columns < world.view_distance && ring_arrived(columns + 1) {
        columns += 1;
    }
    if columns != current {
        lod.set_real_columns(columns, &mut materials);
    }
}

/// Tells the server the streaming radius and flight speed on joining and whenever they
/// change. The local player flies at the speed the server was told.
fn sync_server(settings: Res<Settings>, net: Res<Net>, mut player: ResMut<LocalPlayer>, mut sent: Local<Option<(i32, f32)>>) {
    let fly_speed = if player.admin { settings.fly_speed as f32 / 100.0 } else { 1.0 };
    let wanted = (settings.view_distance, fly_speed);
    if !player.joined || *sent == Some(wanted) {
        return;
    }
    *sent = Some(wanted);
    player.fly_speed = fly_speed;
    net.send_settings(settings.view_distance as u8, fly_speed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_file_round_trips_and_tolerates_old_or_edited_files() {
        let settings = Settings { fov: 90, vsync: false, fly_speed: 250, ..Settings::default() };
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(ron::from_str::<Settings>(&text).unwrap(), settings);
        // Missing fields take their defaults and out-of-range values are clamped.
        let edited: Settings = ron::from_str("(view_distance: 99, music_volume: -5)").unwrap();
        assert_eq!(edited.clamped(), Settings { view_distance: 32, music_volume: 0, ..Settings::default() });
    }
}
