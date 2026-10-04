//! Player settings: the panel Esc opens in game (also the title screen's Settings),
//! applying each choice to the game, and keeping them in `voxmar/settings.ron` in the
//! platform's configuration directory.

use std::path::PathBuf;

use bevy::anti_alias::fxaa::Fxaa;
use bevy::anti_alias::smaa::Smaa;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};
use voxmar_engine::coords::ColumnPos;
use voxmar_protocol::MAX_FLY_SPEED;

use crate::keys::Key;
use crate::menu::Screen;
use crate::menu::widgets::{
    ACCENT, ButtonStyle, MUTED, Pressed, SCRIM, SOFT, TEXT, TRACK, UiFont, button_row, heading, panel, screen, spawn_button,
    text,
};
use crate::net::Net;
use crate::player::{LocalPlayer, UiState, Zoom};
use crate::render::DayCycle;
use crate::render::pack::{ActivePack, ShaderPacks};
use crate::render::lod::{self, Lod};
use crate::render::water::WaterQuality;
use crate::world::ClientWorld;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Rebinding>()
            .add_systems(PostStartup, spawn_menu)
            .add_systems(PreUpdate, rebind.after(InputSystems))
            .add_systems(
            Update,
            (
                toggle_debug,
                (
                    drag_sliders,
                    click_switches,
                    click_choices,
                    click_bindings,
                    click_buttons,
                    sync_controls.run_if(resource_changed::<Settings>.or_eager(resource_changed::<Rebinding>)),
                    show_menu,
                )
                    .chain(),
                (apply, select_pack).run_if(resource_changed::<Settings>),
                fit_lod_hole,
                sync_server.run_if(in_state(Screen::InGame)),
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
    /// Ambient sound volume (wind, birds, water), in percent of the sound effects'.
    pub ambience_volume: i32,
    pub debug_overlay: bool,
    /// Flight speed, in percent. Only admins may fly faster than normal.
    pub fly_speed: i32,
    pub anti_aliasing: AntiAliasing,
    /// Key held to zoom in.
    pub zoom_key: Key,
    /// Key that opens the console to chat (`/` always opens it for a command).
    pub chat_key: Key,
    /// Magnification when zooming in, in percent.
    pub zoom: i32,
    /// Whether the view follows the mouse smoothly while zoomed.
    pub cinematic_zoom: bool,
    /// Whether zooming works in survival too, not only in creative.
    pub zoom_in_survival: bool,
    /// How far the hotbar and hearts sit above the bottom edge, in HUD pixels.
    pub hud_raise: i32,
    /// Name of the shader pack in use (see `render::pack`).
    pub shader_pack: String,
    /// How much work water does: waves, and reflections of the terrain (`render::water`).
    pub water_quality: WaterQuality,
    /// Which texture pack comes first; takes effect the next time the game starts.
    pub textures: TextureOrder,
    /// How many particles effects show (breaking blocks).
    pub particles: ParticleAmount,
    /// How coarsely far chunks are meshed.
    pub far_detail: FarDetail,
}

/// How far from the player chunks keep every block, and how far they are meshed with two
/// or four blocks merged into one along x and z (see `voxmar_engine::mesh::mesh_chunk`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FarDetail {
    /// Every block everywhere.
    Full,
    #[default]
    Balanced,
    /// Coarser sooner, for slower machines and the largest view distances.
    Fast,
}

impl FarDetail {
    const ALL: [FarDetail; 3] = [FarDetail::Full, FarDetail::Balanced, FarDetail::Fast];

    fn label(self) -> &'static str {
        match self {
            FarDetail::Full => "Full",
            FarDetail::Balanced => "Balanced",
            FarDetail::Fast => "Fast",
        }
    }

    /// The distances in columns out to which chunks are meshed in full and half detail;
    /// further ones get a quarter. A block 320 blocks away is under 4 pixels wide at
    /// 1080p, so in Balanced merged blocks are never smaller on screen than a near
    /// block's texel pattern.
    pub fn range(self) -> [i32; 2] {
        match self {
            FarDetail::Full => [i32::MAX; 2],
            FarDetail::Balanced => [10, 20],
            FarDetail::Fast => [6, 12],
        }
    }
}

/// Which texture pack the game prefers (see `textures`). `$VOXMAR_TEXTURES` overrides both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureOrder {
    /// The player's Minecraft textures where they have them, ours for everything else.
    #[default]
    Minecraft,
    /// Our own pack for everything.
    Voxmar,
}

impl TextureOrder {
    fn label(self) -> &'static str {
        match self {
            TextureOrder::Minecraft => "Minecraft first",
            TextureOrder::Voxmar => "Voxmar",
        }
    }
}

/// The look new players start with.
const DEFAULT_PACK: &str = "Verdant";

/// How edges are smoothed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AntiAliasing {
    Off,
    /// A fast, slightly soft filter over the finished image.
    Fxaa,
    /// A sharper filter over the finished image.
    Smaa,
    /// Four samples per pixel: smooths block edges and, through alpha to coverage, the
    /// cut-outs of leaves and plants, which shimmer most in the distance.
    #[default]
    Msaa4,
}

impl AntiAliasing {
    const ALL: [AntiAliasing; 4] = [AntiAliasing::Off, AntiAliasing::Fxaa, AntiAliasing::Smaa, AntiAliasing::Msaa4];

    fn label(self) -> &'static str {
        match self {
            AntiAliasing::Off => "Off",
            AntiAliasing::Fxaa => "FXAA",
            AntiAliasing::Smaa => "SMAA",
            AntiAliasing::Msaa4 => "MSAA 4×",
        }
    }
}

/// How many particles effects show, like Minecraft's particle setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticleAmount {
    #[default]
    All,
    Fewer,
    Off,
}

impl ParticleAmount {
    const ALL: [ParticleAmount; 3] = [ParticleAmount::All, ParticleAmount::Fewer, ParticleAmount::Off];

    fn label(self) -> &'static str {
        match self {
            ParticleAmount::All => "All",
            ParticleAmount::Fewer => "Fewer",
            ParticleAmount::Off => "Off",
        }
    }

    /// The share of each effect's particles shown.
    pub fn share(self) -> f32 {
        match self {
            ParticleAmount::All => 1.0,
            ParticleAmount::Fewer => 0.35,
            ParticleAmount::Off => 0.0,
        }
    }
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
            ambience_volume: 100,
            debug_overlay: false,
            fly_speed: 100,
            anti_aliasing: AntiAliasing::default(),
            zoom_key: Key(KeyCode::KeyC),
            chat_key: Key(KeyCode::KeyT),
            zoom: 400,
            cinematic_zoom: true,
            zoom_in_survival: false,
            hud_raise: 3,
            shader_pack: DEFAULT_PACK.into(),
            water_quality: WaterQuality::default(),
            textures: TextureOrder::default(),
            particles: ParticleAmount::default(),
            far_detail: FarDetail::default(),
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
    Some(config_dir()?.join("settings.ron"))
}

/// Where settings, worlds and recent servers are kept: `$VOXMAR_CONFIG`, else `voxmar` in
/// the platform's configuration directory.
pub fn config_dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("VOXMAR_CONFIG") {
        return Some(dir);
    }
    let base = if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support")
    } else if cfg!(windows) {
        var("APPDATA")?
    } else {
        var("XDG_CONFIG_HOME").or_else(|| Some(var("HOME")?.join(".config")))?
    };
    Some(base.join("voxmar"))
}

/// A setting chosen with a slider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slider {
    ViewDistance,
    Fov,
    Sensitivity,
    Music,
    Sounds,
    Ambience,
    FlySpeed,
    Zoom,
    HudRaise,
}

impl Slider {
    const ALL: [Slider; 9] = [
        Slider::ViewDistance,
        Slider::Fov,
        Slider::HudRaise,
        Slider::Sensitivity,
        Slider::Music,
        Slider::Sounds,
        Slider::Ambience,
        Slider::FlySpeed,
        Slider::Zoom,
    ];

    fn label(self) -> &'static str {
        match self {
            Slider::ViewDistance => "Render distance",
            Slider::Fov => "Field of view",
            Slider::Sensitivity => "Mouse sensitivity",
            Slider::Music => "Music",
            Slider::Sounds => "Sound effects",
            Slider::Ambience => "Ambience",
            Slider::FlySpeed => "Fly speed",
            Slider::Zoom => "Zoom",
            Slider::HudRaise => "Raise HUD",
        }
    }

    /// Lowest value, highest value and step.
    fn range(self) -> (i32, i32, i32) {
        match self {
            Slider::ViewDistance => (4, 32, 2),
            Slider::Fov => (60, 110, 5),
            Slider::Sensitivity => (25, 300, 5),
            Slider::Music | Slider::Sounds | Slider::Ambience => (0, 100, 10),
            Slider::FlySpeed => (100, (MAX_FLY_SPEED * 100.0) as i32, 25),
            Slider::Zoom => ((Zoom::MIN * 100.0) as i32, 1000, 50),
            Slider::HudRaise => (0, 40, 1),
        }
    }

    fn get(self, settings: &Settings) -> i32 {
        match self {
            Slider::ViewDistance => settings.view_distance,
            Slider::Fov => settings.fov,
            Slider::Sensitivity => settings.sensitivity,
            Slider::Music => settings.music_volume,
            Slider::Sounds => settings.sound_volume,
            Slider::Ambience => settings.ambience_volume,
            Slider::FlySpeed => settings.fly_speed,
            Slider::Zoom => settings.zoom,
            Slider::HudRaise => settings.hud_raise,
        }
    }

    fn set(self, settings: &mut Settings, value: i32) {
        *match self {
            Slider::ViewDistance => &mut settings.view_distance,
            Slider::Fov => &mut settings.fov,
            Slider::Sensitivity => &mut settings.sensitivity,
            Slider::Music => &mut settings.music_volume,
            Slider::Sounds => &mut settings.sound_volume,
            Slider::Ambience => &mut settings.ambience_volume,
            Slider::FlySpeed => &mut settings.fly_speed,
            Slider::Zoom => &mut settings.zoom,
            Slider::HudRaise => &mut settings.hud_raise,
        } = value;
    }

    fn text(self, value: i32) -> String {
        match self {
            Slider::ViewDistance => format!("{value} chunks"),
            Slider::Fov => format!("{value}°"),
            Slider::Music | Slider::Sounds | Slider::Ambience if value == 0 => "Off".into(),
            Slider::Sensitivity | Slider::Music | Slider::Sounds | Slider::Ambience => format!("{value}%"),
            Slider::FlySpeed | Slider::Zoom => format!("{}×", value as f32 / 100.0),
            Slider::HudRaise if value == 0 => "Off".into(),
            Slider::HudRaise => format!("{value} px"),
        }
    }
}

/// A setting switched on or off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Toggle {
    Vsync,
    DebugOverlay,
    CinematicZoom,
    ZoomInSurvival,
}

impl Toggle {
    fn label(self) -> &'static str {
        match self {
            Toggle::Vsync => "VSync",
            Toggle::DebugOverlay => "Debug overlay",
            Toggle::CinematicZoom => "Cinematic zoom",
            Toggle::ZoomInSurvival => "Zoom in survival",
        }
    }

    fn get(self, settings: &Settings) -> bool {
        match self {
            Toggle::Vsync => settings.vsync,
            Toggle::DebugOverlay => settings.debug_overlay,
            Toggle::CinematicZoom => settings.cinematic_zoom,
            Toggle::ZoomInSurvival => settings.zoom_in_survival,
        }
    }

    fn set(self, settings: &mut Settings, on: bool) {
        *match self {
            Toggle::Vsync => &mut settings.vsync,
            Toggle::DebugOverlay => &mut settings.debug_overlay,
            Toggle::CinematicZoom => &mut settings.cinematic_zoom,
            Toggle::ZoomInSurvival => &mut settings.zoom_in_survival,
        } = on;
    }
}

/// A setting chosen from a few options; clicking steps to the next one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Choice {
    FarDetail,
    AntiAliasing,
    ShaderPack,
    Water,
    Textures,
    Particles,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Choice::FarDetail => "Far detail",
            Choice::AntiAliasing => "Anti-aliasing",
            Choice::ShaderPack => "Shader pack",
            Choice::Water => "Water",
            Choice::Textures => "Textures (on restart)",
            Choice::Particles => "Particles",
        }
    }

    fn text(self, settings: &Settings, packs: &ShaderPacks) -> String {
        match self {
            Choice::FarDetail => settings.far_detail.label().into(),
            Choice::AntiAliasing => settings.anti_aliasing.label().into(),
            Choice::ShaderPack => packs.get(&settings.shader_pack).name.clone(),
            Choice::Water => settings.water_quality.label().into(),
            Choice::Textures => settings.textures.label().into(),
            Choice::Particles => settings.particles.label().into(),
        }
    }

    fn step(self, settings: &mut Settings, packs: &ShaderPacks) {
        match self {
            Choice::FarDetail => {
                let i = FarDetail::ALL.iter().position(|&d| d == settings.far_detail).unwrap_or(0);
                settings.far_detail = FarDetail::ALL[(i + 1) % FarDetail::ALL.len()];
            }
            Choice::AntiAliasing => {
                let i = AntiAliasing::ALL.iter().position(|&a| a == settings.anti_aliasing).unwrap_or(0);
                settings.anti_aliasing = AntiAliasing::ALL[(i + 1) % AntiAliasing::ALL.len()];
            }
            Choice::ShaderPack => settings.shader_pack = packs.next(&settings.shader_pack).to_owned(),
            Choice::Water => {
                let i = WaterQuality::ALL.iter().position(|&q| q == settings.water_quality).unwrap_or(0);
                settings.water_quality = WaterQuality::ALL[(i + 1) % WaterQuality::ALL.len()];
            }
            Choice::Textures => {
                settings.textures = match settings.textures {
                    TextureOrder::Minecraft => TextureOrder::Voxmar,
                    TextureOrder::Voxmar => TextureOrder::Minecraft,
                }
            }
            Choice::Particles => {
                let i = ParticleAmount::ALL.iter().position(|&a| a == settings.particles).unwrap_or(0);
                settings.particles = ParticleAmount::ALL[(i + 1) % ParticleAmount::ALL.len()];
            }
        }
    }
}

/// A key the player can rebind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Binding {
    Zoom,
    Chat,
}

impl Binding {
    fn label(self) -> &'static str {
        match self {
            Binding::Zoom => "Zoom key",
            Binding::Chat => "Chat key",
        }
    }

    fn get(self, settings: &Settings) -> Key {
        match self {
            Binding::Zoom => settings.zoom_key,
            Binding::Chat => settings.chat_key,
        }
    }

    fn set(self, settings: &mut Settings, key: Key) {
        *match self {
            Binding::Zoom => &mut settings.zoom_key,
            Binding::Chat => &mut settings.chat_key,
        } = key;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Slider(Slider),
    Switch(Toggle),
    Choice(Choice),
    Key(Binding),
}

const SECTIONS: [(&str, &[Control]); 3] = [
    (
        "Video",
        &[
            Control::Slider(Slider::ViewDistance),
            Control::Choice(Choice::FarDetail),
            Control::Slider(Slider::Fov),
            Control::Choice(Choice::ShaderPack),
            Control::Choice(Choice::Textures),
            Control::Choice(Choice::AntiAliasing),
            Control::Choice(Choice::Water),
            Control::Choice(Choice::Particles),
            Control::Slider(Slider::HudRaise),
            Control::Switch(Toggle::Vsync),
            Control::Switch(Toggle::DebugOverlay),
        ],
    ),
    ("Audio", &[Control::Slider(Slider::Music), Control::Slider(Slider::Sounds), Control::Slider(Slider::Ambience)]),
    (
        "Controls",
        &[
            Control::Slider(Slider::Sensitivity),
            Control::Key(Binding::Chat),
            Control::Key(Binding::Zoom),
            Control::Slider(Slider::Zoom),
            Control::Switch(Toggle::CinematicZoom),
            Control::Switch(Toggle::ZoomInSurvival),
            Control::Slider(Slider::FlySpeed),
        ],
    ),
];

#[derive(Component, Clone, Copy)]
enum MenuButton {
    /// Back to the game.
    Resume,
    /// Back to the title screen, when opened from there.
    Done,
    QuitToTitle,
    Quit,
}

#[derive(Component)]
struct MenuRoot;
/// Parts of the panel that only make sense in some places.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ShownWhen {
    InGame,
    OnTitle,
    /// For admins in game, and always on the title screen, where it applies to the next
    /// world.
    Admin,
}
/// The draggable area of a slider.
#[derive(Component)]
struct SliderTrack(Slider);
#[derive(Component)]
struct SliderFill(Slider);
#[derive(Component)]
struct SliderValue(Slider);
#[derive(Component)]
struct Switch(Toggle);
#[derive(Component)]
struct ChoiceButton(Choice);
#[derive(Component)]
struct ChoiceValue(Choice);
#[derive(Component)]
struct KeyButton(Binding);
#[derive(Component)]
struct KeyValue(Binding);

/// The binding waiting for its new key, after its button was clicked.
#[derive(Resource, Default)]
struct Rebinding(Option<Binding>);

const PANEL_WIDTH: f32 = 520.0;
const SLIDER_WIDTH: f32 = 200.0;
const VALUE_WIDTH: f32 = 84.0;
const KNOB: f32 = 16.0;

fn spawn_menu(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((MenuRoot, screen(SCRIM), GlobalZIndex(1), Visibility::Hidden)).with_children(|root| {
        root.spawn(panel(PANEL_WIDTH)).with_children(|panel| {
            panel.spawn(heading(font, "Settings"));
            for (section, controls) in SECTIONS {
                let margin = UiRect::new(Val::ZERO, Val::ZERO, Val::Px(18.0), Val::Px(2.0));
                panel.spawn((Node { margin, ..default() }, text(font, section.to_uppercase(), 12.0, MUTED)));
                for &control in controls {
                    spawn_row(panel, font, control);
                }
            }
            panel.spawn(button_row(26.0)).with_children(|row| {
                spawn_button(row, font, "Resume", ButtonStyle::Primary, (MenuButton::Resume, ShownWhen::InGame));
                spawn_button(row, font, "Done", ButtonStyle::Primary, (MenuButton::Done, ShownWhen::OnTitle));
            });
            panel.spawn((button_row(12.0), ShownWhen::InGame)).with_children(|row| {
                spawn_button(row, font, "Save and quit to title", ButtonStyle::Secondary, MenuButton::QuitToTitle);
                spawn_button(row, font, "Quit game", ButtonStyle::Secondary, MenuButton::Quit);
            });
        });
    });
}

/// A label on the left and its control on the right.
fn spawn_row(panel: &mut ChildSpawnerCommands, font: &Handle<Font>, control: Control) {
    let label = match control {
        Control::Slider(slider) => slider.label(),
        Control::Switch(toggle) => toggle.label(),
        Control::Choice(choice) => choice.label(),
        Control::Key(binding) => binding.label(),
    };
    let node = Node {
        height: Val::Px(38.0),
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        ..default()
    };
    let mut row = panel.spawn(node);
    if control == Control::Slider(Slider::FlySpeed) {
        row.insert(ShownWhen::Admin);
    }
    row.with_children(|row| {
        row.spawn(text(font, label, 16.0, TEXT));
        match control {
            Control::Slider(slider) => spawn_slider(row, font, slider),
            Control::Switch(toggle) => spawn_switch(row, toggle),
            Control::Choice(choice) => spawn_choice(row, font, choice),
            Control::Key(binding) => spawn_key(row, font, binding),
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

/// A pill showing the current option.
fn spawn_choice(row: &mut ChildSpawnerCommands, font: &Handle<Font>, choice: Choice) {
    let pill = Node {
        width: Val::Px(120.0),
        height: Val::Px(26.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border_radius: BorderRadius::MAX,
        ..default()
    };
    row.spawn((Button, ChoiceButton(choice), pill, BackgroundColor(TRACK)))
        .with_child((ChoiceValue(choice), text(font, "", 14.0, TEXT)));
}

/// A pill showing the bound key; clicked, it waits for the next key pressed.
fn spawn_key(row: &mut ChildSpawnerCommands, font: &Handle<Font>, binding: Binding) {
    let pill = Node {
        width: Val::Px(120.0),
        height: Val::Px(26.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border_radius: BorderRadius::MAX,
        ..default()
    };
    row.spawn((Button, KeyButton(binding), pill, BackgroundColor(TRACK)))
        .with_child((KeyValue(binding), text(font, "", 14.0, TEXT)));
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

fn click_choices(
    mut settings: ResMut<Settings>,
    packs: Res<ShaderPacks>,
    choices: Query<(&ChoiceButton, &Interaction), Changed<Interaction>>,
) {
    for (choice, interaction) in &choices {
        if *interaction == Interaction::Pressed {
            choice.0.step(&mut settings, &packs);
        }
    }
}

/// Clicking a key's pill waits for a new key; clicking it again stops waiting.
fn click_bindings(mut rebinding: ResMut<Rebinding>, buttons: Query<(&KeyButton, &Interaction), Changed<Interaction>>) {
    for (button, interaction) in &buttons {
        if *interaction == Interaction::Pressed {
            rebinding.0 = if rebinding.0 == Some(button.0) { None } else { Some(button.0) };
        }
    }
}

/// Binds the first key pressed while waiting for one; Esc stops waiting. Runs before the
/// game sees the key, and hides it from the game, so binding E or Esc does not also open
/// the inventory or close the menu.
fn rebind(mut keys: ResMut<ButtonInput<KeyCode>>, mut rebinding: ResMut<Rebinding>, mut settings: ResMut<Settings>) {
    let Some(binding) = rebinding.0 else { return };
    let Some(&key) = keys.get_just_pressed().find(|&&key| key == KeyCode::Escape || Key::bindable(key)) else { return };
    keys.clear_just_pressed(key);
    rebinding.0 = None;
    if key != KeyCode::Escape && binding.get(&settings) != Key(key) {
        binding.set(&mut settings, Key(key));
    }
}

fn click_buttons(
    buttons: Query<(&MenuButton, &Interaction), Pressed>,
    mut ui: ResMut<UiState>,
    settings: Res<Settings>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: MessageWriter<AppExit>,
) {
    for (&button, interaction) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Resume => ui.menu_open = false,
            MenuButton::Done => next.set(Screen::Title),
            // Leaving the game stops the integrated server, which saves the world.
            MenuButton::QuitToTitle => next.set(Screen::Title),
            MenuButton::Quit => {
                settings.save();
                exit.write(AppExit::Success);
            }
        }
    }
}

/// Mirrors the settings in the menu's controls.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn sync_controls(
    settings: Res<Settings>,
    mut fills: Query<(&SliderFill, &mut Node), Without<Switch>>,
    mut values: Query<(&SliderValue, &mut Text)>,
    mut switches: Query<(&Switch, &mut Node, &mut BackgroundColor)>,
    mut choices: Query<(&ChoiceValue, &mut Text), Without<SliderValue>>,
    mut keys: Query<(&KeyValue, &mut Text), (Without<SliderValue>, Without<ChoiceValue>)>,
    rebinding: Res<Rebinding>,
    packs: Res<ShaderPacks>,
) {
    for (key, mut text) in &mut keys {
        let value = if rebinding.0 == Some(key.0) { "Press a key" } else { key.0.get(&settings).label() };
        if text.0 != value {
            value.clone_into(&mut text.0);
        }
    }
    for (choice, mut text) in &mut choices {
        let value = choice.0.text(&settings, &packs);
        if text.0 != value {
            text.0 = value;
        }
    }
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

/// Shows the menu while it is open in game or the title screen's Settings is up, with the
/// buttons that fit and admin-only rows for admins (and on the title screen), and saves
/// the settings when it closes.
#[allow(clippy::too_many_arguments)]
fn show_menu(
    ui: Res<UiState>,
    screen: Res<State<Screen>>,
    settings: Res<Settings>,
    player: Res<LocalPlayer>,
    net: Option<Res<Net>>,
    mut was_open: Local<bool>,
    mut root: Query<&mut Visibility, With<MenuRoot>>,
    mut nodes: Query<(&mut Node, &ShownWhen)>,
    quit_to_title: Query<(&MenuButton, &Children)>,
    mut texts: Query<&mut Text>,
    mut rebinding: ResMut<Rebinding>,
) {
    let in_game = *screen.get() == Screen::InGame;
    let open = (in_game && ui.menu_open) || *screen.get() == Screen::Settings;
    if *was_open && !open {
        settings.save();
        rebinding.0 = None;
    }
    *was_open = open;
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if open { Visibility::Inherited } else { Visibility::Hidden });
    }
    if !open {
        return;
    }
    for (mut node, when) in &mut nodes {
        let shown = match when {
            ShownWhen::InGame => in_game,
            ShownWhen::OnTitle => !in_game,
            ShownWhen::Admin => player.admin || !in_game,
        };
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // Leaving a server saves nothing of ours, so say what it does.
    let label = if net.is_some_and(|net| net.is_remote()) { "Disconnect" } else { "Save and quit to title" };
    for (button, children) in &quit_to_title {
        if matches!(button, MenuButton::QuitToTitle)
            && let Some(mut text) = children.first().and_then(|&child| texts.get_mut(child).ok())
            && text.0 != label
        {
            label.clone_into(&mut text.0);
        }
    }
}

/// Switches to the chosen shader pack.
fn select_pack(settings: Res<Settings>, packs: Res<ShaderPacks>, mut active: ResMut<ActivePack>) {
    let pack = packs.get(&settings.shader_pack);
    if active.0 != *pack {
        active.0 = pack.clone();
    }
}

/// Applies the settings other modules do not read for themselves. The field of view is
/// the player's camera's, which narrows while zooming.
fn apply(
    settings: Res<Settings>,
    mut world: ResMut<ClientWorld>,
    mut cycle: ResMut<DayCycle>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut camera: Query<(Entity, &mut Msaa), With<Camera3d>>,
    mut water: ResMut<WaterQuality>,
    mut commands: Commands,
) {
    water.set_if_neq(settings.water_quality);
    if world.view_distance != settings.view_distance {
        world.set_view_distance(settings.view_distance);
        cycle.view_distance = lod::view_distance(settings.view_distance);
    }
    world.set_far_detail(settings.far_detail.range());
    let present_mode = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if let Ok(mut window) = window.single_mut()
        && window.present_mode != present_mode
    {
        window.present_mode = present_mode;
    }
    let Ok((entity, mut msaa)) = camera.single_mut() else { return };
    msaa.set_if_neq(if settings.anti_aliasing == AntiAliasing::Msaa4 { Msaa::Sample4 } else { Msaa::Off });
    let mut camera = commands.entity(entity);
    match settings.anti_aliasing {
        AntiAliasing::Fxaa => camera.insert(Fxaa::default()).remove::<Smaa>(),
        AntiAliasing::Smaa => camera.insert(Smaa::default()).remove::<Fxaa>(),
        AntiAliasing::Off | AntiAliasing::Msaa4 => camera.remove::<(Fxaa, Smaa)>(),
    };
}

/// Fits the distant terrain's hole for real chunks to the render distance: at once when it
/// shrinks, and ring by ring as columns arrive when it grows, so that raising the render
/// distance never opens a gap in the terrain.
fn fit_lod_hole(
    world: Res<ClientWorld>,
    mut lod: ResMut<Lod>,
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
        lod.set_real_columns(columns);
    }
}

/// Tells the server the streaming radius and flight speed on joining and whenever they
/// change. The local player flies at the speed the server was told.
fn sync_server(settings: Res<Settings>, net: Res<Net>, mut player: ResMut<LocalPlayer>, mut sent: Local<Option<(i32, f32)>>) {
    let fly_speed = if player.admin { settings.fly_speed as f32 / 100.0 } else { 1.0 };
    let wanted = (settings.view_distance, fly_speed);
    if !player.joined {
        // A new session needs telling again.
        *sent = None;
        return;
    }
    if *sent == Some(wanted) {
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
