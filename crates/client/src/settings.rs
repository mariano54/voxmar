//! Player settings: the panel Esc opens in game (also the title screen's Settings),
//! applying each choice to the game, and keeping them in `voxmar/settings.ron` in the
//! platform's configuration directory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::anti_alias::fxaa::Fxaa;
use bevy::anti_alias::smaa::Smaa;
use bevy::input::InputSystems;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};
use voxmar_engine::coords::ColumnPos;
use voxmar_protocol::MAX_FLY_SPEED;

use crate::keys::Key;
use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::music::MusicCategories;
use crate::ui::skin::{InterfaceFont, PX, Piece};
use crate::ui::sound::{Silent, UiSounds};
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, GOLD, LARGE, MUTED, Pressed, SCRIM, SOFT, Selected, TEXT, UiFont, button_row, panel, screen,
    shadowed, spawn_button, spawn_rule, text,
};
use crate::ui::widgets::{HoverLight, space};
use crate::net::Net;
use crate::player::{LocalPlayer, UiState, Zoom};
use crate::render::{DayCycle, Quality, ShadowQuality};
use crate::render::pack::{ActivePack, ShaderPacks};
use crate::render::lod::{self, Lod};
use crate::render::scale::RenderScale;
use crate::render::water::WaterQuality;
use crate::world::ClientWorld;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Rebinding>()
            .init_resource::<OpenSection>()
            .init_resource::<OpenDropdown>()
            .add_systems(PostStartup, (spawn_menu, open_start_tab))
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
                    pick_section,
                    show_section,
                    click_dropdowns,
                    hover_options,
                    sync_controls.run_if(
                        resource_changed::<Settings>.or_eager(resource_changed::<Rebinding>).or_eager(resource_changed::<Locale>),
                    ),
                    show_menu,
                )
                    .chain(),
                (apply, select_pack).run_if(resource_changed::<Settings>),
                fit_lod_hole,
                autosave,
                sync_server.run_if(in_state(Screen::InGame)),
            ),
        );
    }
}

/// How many quick prayers have a key.
pub const QUICK_PRAYERS: usize = 5;

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
    /// The categories of music switched off (`music.ron`), by name; every other one plays,
    /// a mod's new category too.
    pub music_off: Vec<String>,
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
    /// Key that starts a sprint.
    pub sprint_key: Key,
    /// Key that dodges, with a direction (a backstep without).
    pub dodge_key: Key,
    /// Whether tapping forward twice quickly starts a sprint too.
    pub double_tap_sprint: bool,
    /// Whether the view widens a little while sprinting.
    pub fov_effects: bool,
    /// Magnification when zooming in, in percent.
    pub zoom: i32,
    /// Whether the view follows the mouse smoothly while zoomed.
    pub cinematic_zoom: bool,
    /// Whether zooming works in survival too, not only in creative.
    pub zoom_in_survival: bool,
    /// Whether a right click raises the guard and the next lowers it, rather than holding
    /// the button.
    pub toggle_guard: bool,
    /// Whether a fight shows the number each blow did where it landed (hitsplats).
    pub damage_numbers: bool,
    /// Whether a fight moves the view: a heavy blow's dip, a hurt's tip, a block's shake.
    pub screen_shake: bool,
    /// Whether the way home shows: a hearth mark toward home at night or far from it
    /// (`hud::home`).
    pub way_home_marker: bool,
    /// How far the hotbar and hearts sit above the bottom edge, in HUD pixels.
    pub hud_raise: i32,
    /// Name of the shader pack in use (see `render::pack`).
    pub shader_pack: String,
    /// How sun shadows are drawn.
    pub shadows: ShadowQuality,
    /// Shafts of sunlight through canopies and the sun's glow in haze and mist.
    pub volumetric_light: bool,
    /// How much work water does: waves, and reflections of the terrain (`render::water`).
    pub water_quality: WaterQuality,
    /// Which texture pack comes first; takes effect the next time the game starts.
    pub textures: TextureOrder,
    /// Whether the game borrows from the player's own Minecraft install: the textures in
    /// its client jar and the sounds the launcher keeps. Off for a new player, who gets
    /// Voxmar's own; a settings file from before the setting existed reads it as on when
    /// a jar is found, so nothing changes for anyone who had it (see `Settings::load`).
    /// Takes effect the next time the game starts.
    pub minecraft_assets: bool,
    /// With `minecraft_assets` on, music also comes from a Prism Launcher instance
    /// (`music::prism`): the launcher's data folder, empty for the standard one, and the
    /// instance's folder name, empty for the most recently launched GT New Horizons one.
    pub prism_folder: String,
    pub prism_instance: String,
    /// How many particles effects show (breaking blocks).
    pub particles: ParticleAmount,
    /// How coarsely far chunks are meshed.
    pub far_detail: FarDetail,
    /// How many pixels the world is drawn at (`render::scale`), in percent of the window's
    /// width and height: one of `RESOLUTIONS`.
    pub resolution: i32,
    /// The pixel font the interface writes with.
    pub font: InterfaceFont,
    /// How large the interface is drawn, in percent: one of `UI_SCALES`.
    pub ui_scale: i32,
    /// The tips shown already (`hud::tips`), each once.
    pub tips_seen: Vec<String>,
    /// The quests followed on screen (`hud::quests`), at most three.
    pub pinned_quests: Vec<String>,
    /// The sounds buttons and the inventory make.
    pub ui_sounds: UiSounds,
    /// Key that opens and closes the inventory.
    pub inventory_key: Key,
    /// Key that opens and closes the character screen (skills and how you are).
    pub character_key: Key,
    pub journal_key: Key,
    pub quests_key: Key,
    /// Keys that show, over an item, the recipes that make it and the ones it goes into.
    pub recipes_key: Key,
    pub uses_key: Key,
    /// Key that copies the frame to the clipboard and saves it (`screenshot`).
    pub screenshot_key: Key,
    /// Quick prayers: each key turns its prayer (by name, as content calls it) on or off.
    pub prayer_keys: [Key; QUICK_PRAYERS],
    pub quick_prayers: [String; QUICK_PRAYERS],
    /// The language's code (`en`, `ja`), or empty to follow the system's.
    pub language: String,
}

/// The sizes the interface can be drawn at, in percent. Steps of a quarter keep the pixel
/// art whole on a Retina screen (each art pixel a whole number of the screen's).
pub const UI_SCALES: [i32; 5] = [50, 75, 100, 125, 150];

/// The step of `steps` nearest `value`.
fn nearest(steps: &[i32], value: i32) -> i32 {
    steps.iter().copied().min_by_key(|step| (step - value).abs()).unwrap_or(value)
}

/// The resolutions the world can be drawn at, in percent of the window's width and
/// height, before it is stretched over the window; menus and text are always drawn at the
/// window's own. On a Retina screen 50% is the screen's size in points.
const RESOLUTIONS: [i32; 9] = [100, 90, 80, 75, 67, 60, 50, 40, 33];

/// The listed resolution nearest `percent`.
fn nearest_resolution(percent: i32) -> i32 {
    *RESOLUTIONS.iter().min_by_key(|&&r| (r - percent).abs()).expect("the list is not empty")
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

    fn key(self) -> &'static str {
        match self {
            FarDetail::Full => "settings.far_detail.full",
            FarDetail::Balanced => "settings.far_detail.balanced",
            FarDetail::Fast => "settings.far_detail.fast",
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

/// Which texture pack the game prefers (see `textures`). `$VOXMAR_TEXTURES` overrides them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureOrder {
    /// The player's Minecraft textures where they have them, ours for everything else:
    /// only with `minecraft_assets` on.
    Minecraft,
    /// Our own pack for everything.
    #[default]
    Voxmar,
    /// Voxmar HD (`assets/packs/voxmar_hd`, docs/voxmar-hd.md): our own 32×32 look.
    VoxmarHd,
}

impl TextureOrder {
    pub const ALL: [TextureOrder; 3] = [TextureOrder::Voxmar, TextureOrder::VoxmarHd, TextureOrder::Minecraft];

    /// The orders a player can pick: Minecraft's only when the game may use their install.
    fn choices(minecraft_assets: bool) -> &'static [TextureOrder] {
        if minecraft_assets { &Self::ALL } else { &Self::ALL[..2] }
    }

    fn key(self) -> &'static str {
        match self {
            TextureOrder::Minecraft => "settings.textures.minecraft",
            TextureOrder::Voxmar => "settings.textures.voxmar",
            TextureOrder::VoxmarHd => "settings.textures.voxmar_hd",
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

    fn key(self) -> &'static str {
        match self {
            AntiAliasing::Off => "settings.value.off",
            AntiAliasing::Fxaa => "settings.anti_aliasing.fxaa",
            AntiAliasing::Smaa => "settings.anti_aliasing.smaa",
            AntiAliasing::Msaa4 => "settings.anti_aliasing.msaa4",
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

    fn key(self) -> &'static str {
        match self {
            ParticleAmount::All => "settings.particles.all",
            ParticleAmount::Fewer => "settings.particles.fewer",
            ParticleAmount::Off => "settings.value.off",
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
            view_distance: 32,
            vsync: false,
            fov: 90,
            sensitivity: 100,
            music_volume: 100,
            music_off: Vec::new(),
            sound_volume: 100,
            ambience_volume: 100,
            debug_overlay: false,
            fly_speed: 100,
            anti_aliasing: AntiAliasing::Fxaa,
            zoom_key: Key(KeyCode::KeyZ),
            chat_key: Key(KeyCode::KeyT),
            sprint_key: Key(KeyCode::ControlLeft),
            dodge_key: Key(KeyCode::AltLeft),
            double_tap_sprint: true,
            fov_effects: true,
            zoom: 400,
            cinematic_zoom: true,
            zoom_in_survival: false,
            toggle_guard: false,
            damage_numbers: true,
            screen_shake: true,
            way_home_marker: true,
            hud_raise: 3,
            shader_pack: DEFAULT_PACK.into(),
            shadows: ShadowQuality::default(),
            volumetric_light: true,
            water_quality: WaterQuality::default(),
            textures: TextureOrder::default(),
            minecraft_assets: false,
            prism_folder: String::new(),
            prism_instance: String::new(),
            particles: ParticleAmount::default(),
            far_detail: FarDetail::default(),
            resolution: 50,
            font: InterfaceFont::default(),
            // A little smaller than the art's own size: still whole pixels on a Retina screen.
            ui_scale: 75,
            tips_seen: Vec::new(),
            pinned_quests: Vec::new(),
            ui_sounds: UiSounds::default(),
            inventory_key: Key(KeyCode::KeyE),
            character_key: Key(KeyCode::KeyC),
            journal_key: Key(KeyCode::KeyJ),
            quests_key: Key(KeyCode::KeyL),
            recipes_key: Key(KeyCode::KeyR),
            uses_key: Key(KeyCode::KeyU),
            screenshot_key: Key(KeyCode::F2),
            // The design's X C V B N, with G for C, which shows the character.
            prayer_keys: [KeyCode::KeyX, KeyCode::KeyV, KeyCode::KeyB, KeyCode::KeyN, KeyCode::KeyG].map(Key),
            quick_prayers: ["base:bark_skin", "base:ember_arm", "base:keen_mind", "base:mending", "base:keepsake"].map(String::from),
            language: String::new(),
        }
    }
}

impl Settings {
    /// The saved settings, or the defaults if there are none. A setting the file gets wrong
    /// (from an older or newer game, or edited by hand) keeps its default and the rest load,
    /// so one bad value never throws away everything else.
    pub fn load() -> Self {
        let Some(path) = path() else { return Self::default() };
        let Ok(source) = std::fs::read_to_string(&path) else { return Self::default() };
        let (settings, skipped) = Self::parse(&source, || crate::textures::find_minecraft_jar().is_some());
        if !skipped.is_empty() {
            warn!("{}: kept the defaults for {} ({})", path.display(), skipped.join(", "), "unreadable values");
        }
        settings
    }

    /// For a file written before `minecraft_assets` existed, when the game always used the
    /// player's Minecraft install if it found one: the setting is on if a jar is found
    /// (`jar_found`), and so is Minecraft first for a file older still, from before
    /// `textures`. Decided as the file is read, never written back by itself: an existing
    /// player's game looks and sounds as it did, and a new one's starts as Voxmar.
    fn keep_minecraft(&mut self, source: &str, jar_found: impl FnOnce() -> bool) {
        let has = |name: &str| top_level_fields(source).iter().any(|field| field.split(':').next().is_some_and(|field| field.trim() == name));
        if has("minecraft_assets") {
            return;
        }
        self.minecraft_assets = jar_found();
        if self.minecraft_assets && !has("textures") {
            self.textures = TextureOrder::Minecraft;
        }
    }

    /// The settings in `source`, and the names of the fields it could not read;
    /// `jar_found` says whether the player has Minecraft (see [`Settings::keep_minecraft`]).
    fn parse(source: &str, jar_found: impl FnOnce() -> bool) -> (Self, Vec<String>) {
        if let Ok(mut settings) = ron::from_str::<Settings>(source) {
            settings.keep_minecraft(source, jar_found);
            return (settings.clamped(), Vec::new());
        }
        // Add the file's fields one at a time, keeping each only if the settings still read.
        let mut kept: Vec<&str> = Vec::new();
        let mut skipped = Vec::new();
        for field in top_level_fields(source) {
            kept.push(field);
            if ron::from_str::<Settings>(&format!("({})", kept.join(", "))).is_err() {
                kept.pop();
                skipped.push(field.split(':').next().unwrap_or(field).trim().to_owned());
            }
        }
        let mut settings = ron::from_str::<Settings>(&format!("({})", kept.join(", "))).unwrap_or_default();
        settings.keep_minecraft(source, jar_found);
        (settings.clamped(), skipped)
    }

    pub fn save(&self) {
        if FILE_UNTOUCHED.load(Ordering::Relaxed) {
            return;
        }
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

    /// The settings with every value within its slider's range, a listed resolution, and
    /// no two bindings on one key, nor one on a key the game keeps for itself: a binding
    /// that collides goes back to its default (one moved off its default first), or to the
    /// first free key that is not a modifier or Enter if another holds its default, so a
    /// hand-edited file or a key a new screen took never leaves a screen that cannot open.
    /// Only what is read changes; nothing is written.
    pub fn clamped(mut self) -> Self {
        self.resolution = nearest_resolution(self.resolution);
        // Each quest pinned once (the quest book caps how many, from its data).
        let mut pinned: Vec<String> = Vec::new();
        for name in std::mem::take(&mut self.pinned_quests) {
            if !pinned.contains(&name) {
                pinned.push(name);
            }
        }
        self.pinned_quests = pinned;
        if !TextureOrder::choices(self.minecraft_assets).contains(&self.textures) {
            self.textures = TextureOrder::Voxmar;
        }
        let defaults = Settings::default();
        // Bindings on their defaults keep them, and the others yield to them (zoom used to be
        // on C, which now opens the character screen), then later ones to earlier ones.
        let mut order = Binding::ALL;
        order.sort_by_key(|b| b.get(&self) != b.get(&defaults));
        for (i, &binding) in order.iter().enumerate() {
            let key = binding.get(&self);
            let taken = FIXED_KEYS.contains(&key.0) || order[..i].iter().any(|other| other.get(&self) == key);
            if !taken {
                continue;
            }
            let free = |key: Key, settings: &Settings| !FIXED_KEYS.contains(&key.0) && !Binding::ALL.iter().any(|other| other.get(settings) == key);
            let default = binding.get(&defaults);
            let moved = if free(default, &self) { Some(default) } else { Key::all().find(|&k| !k.modifier_or_enter() && free(k, &self)) };
            if let Some(moved) = moved {
                binding.set(&mut self, moved);
            }
        }
        for slider in Slider::ALL {
            let (min, max, _) = slider.range();
            let value = slider.get(&self).clamp(min, max);
            slider.set(&mut self, value);
        }
        self
    }
}

/// The `name: value` fields of a RON struct, split at its top-level commas (outside any
/// brackets and strings).
fn top_level_fields(source: &str) -> Vec<&str> {
    let body = source.trim();
    let Some(body) = body.find('(').and_then(|open| body.rfind(')').map(|close| &body[open + 1..close])) else { return Vec::new() };
    let (mut fields, mut depth, mut start, mut in_string, mut escaped) = (Vec::new(), 0i32, 0, false, false);
    for (i, c) in body.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '(' | '[' | '{' if !in_string => depth += 1,
            ')' | ']' | '}' if !in_string => depth -= 1,
            ',' if !in_string && depth == 0 => {
                fields.push(body[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    fields.push(body[start..].trim());
    fields.into_iter().filter(|f| !f.is_empty() && !f.starts_with("//")).collect()
}

/// Set for runs that are not the player's own (`--capture`, `--gate`): they start from the
/// defaults and must never write the player's settings file, whatever they change.
static FILE_UNTOUCHED: AtomicBool = AtomicBool::new(false);

/// Makes `Settings::save` do nothing for the rest of the run.
pub fn leave_file_untouched() {
    FILE_UNTOUCHED.store(true, Ordering::Relaxed);
}

/// Whether this run leaves the settings file alone.
pub fn file_untouched() -> bool {
    FILE_UNTOUCHED.load(Ordering::Relaxed)
}

/// The settings file, if there is a configuration directory.
pub fn file() -> Option<PathBuf> {
    path()
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

    fn key(self) -> &'static str {
        match self {
            Slider::ViewDistance => "settings.render_distance",
            Slider::Fov => "settings.fov",
            Slider::Sensitivity => "settings.sensitivity",
            Slider::Music => "settings.music",
            Slider::Sounds => "settings.sounds",
            Slider::Ambience => "settings.ambience",
            Slider::FlySpeed => "settings.fly_speed",
            Slider::Zoom => "settings.zoom",
            Slider::HudRaise => "settings.hud_raise",
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

    fn text(self, value: i32, locale: &Locale) -> String {
        match self {
            Slider::ViewDistance => locale.format("settings.value.chunks", &[("count", &value)]),
            Slider::Fov => format!("{value}°"),
            Slider::Music | Slider::Sounds | Slider::Ambience | Slider::HudRaise if value == 0 => locale.get("settings.value.off").into(),
            Slider::Sensitivity | Slider::Music | Slider::Sounds | Slider::Ambience => format!("{value}%"),
            Slider::FlySpeed | Slider::Zoom => format!("{}×", value as f32 / 100.0),
            Slider::HudRaise => locale.format("settings.value.pixels", &[("count", &value)]),
        }
    }
}

/// A setting switched on or off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Toggle {
    Vsync,
    VolumetricLight,
    DebugOverlay,
    CinematicZoom,
    ZoomInSurvival,
    ToggleGuard,
    DamageNumbers,
    ScreenShake,
    WayHomeMarker,
    DoubleTapSprint,
    FovEffects,
    MinecraftAssets,
    /// A category of music (`music.ron`).
    Music(MusicSwitch),
}

/// A category of music's switch: its name, and the key of what the languages call it.
/// Categories come from the content, so these are made once, as the menu is built
/// (`music_switches`), and live as long as the game.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MusicSwitch {
    name: &'static str,
    key: &'static str,
    /// Its tracks all come from Minecraft, so it shows only with Minecraft assets on.
    borrowed_only: bool,
}

/// The switches of the music's categories, in their order.
fn music_switches(categories: &MusicCategories) -> Vec<MusicSwitch> {
    categories
        .0
        .iter()
        .map(|category| MusicSwitch {
            name: Box::leak(category.name.clone().into_boxed_str()),
            key: Box::leak(format!("music.{}", category.name).into_boxed_str()),
            borrowed_only: category.borrowed_only(),
        })
        .collect()
}

impl Toggle {
    fn key(self) -> &'static str {
        match self {
            Toggle::Vsync => "settings.vsync",
            Toggle::VolumetricLight => "settings.light_shafts",
            Toggle::DebugOverlay => "settings.debug_overlay",
            Toggle::CinematicZoom => "settings.cinematic_zoom",
            Toggle::ZoomInSurvival => "settings.zoom_in_survival",
            Toggle::ToggleGuard => "settings.toggle_guard",
            Toggle::DamageNumbers => "settings.damage_numbers",
            Toggle::ScreenShake => "settings.screen_shake",
            Toggle::WayHomeMarker => "settings.way_home_marker",
            Toggle::DoubleTapSprint => "settings.double_tap_sprint",
            Toggle::FovEffects => "settings.fov_effects",
            Toggle::MinecraftAssets => "settings.minecraft_assets",
            Toggle::Music(category) => category.key,
        }
    }

    fn get(self, settings: &Settings) -> bool {
        match self {
            Toggle::Vsync => settings.vsync,
            Toggle::VolumetricLight => settings.volumetric_light,
            Toggle::DebugOverlay => settings.debug_overlay,
            Toggle::CinematicZoom => settings.cinematic_zoom,
            Toggle::ZoomInSurvival => settings.zoom_in_survival,
            Toggle::ToggleGuard => settings.toggle_guard,
            Toggle::DamageNumbers => settings.damage_numbers,
            Toggle::ScreenShake => settings.screen_shake,
            Toggle::WayHomeMarker => settings.way_home_marker,
            Toggle::DoubleTapSprint => settings.double_tap_sprint,
            Toggle::FovEffects => settings.fov_effects,
            Toggle::MinecraftAssets => settings.minecraft_assets,
            Toggle::Music(category) => !settings.music_off.iter().any(|off| off == category.name),
        }
    }

    fn set(self, settings: &mut Settings, on: bool) {
        if let Toggle::Music(category) = self {
            settings.music_off.retain(|off| off != category.name);
            if !on {
                settings.music_off.push(category.name.to_owned());
            }
            return;
        }
        *match self {
            Toggle::Vsync => &mut settings.vsync,
            Toggle::VolumetricLight => &mut settings.volumetric_light,
            Toggle::DebugOverlay => &mut settings.debug_overlay,
            Toggle::CinematicZoom => &mut settings.cinematic_zoom,
            Toggle::ZoomInSurvival => &mut settings.zoom_in_survival,
            Toggle::ToggleGuard => &mut settings.toggle_guard,
            Toggle::DamageNumbers => &mut settings.damage_numbers,
            Toggle::ScreenShake => &mut settings.screen_shake,
            Toggle::WayHomeMarker => &mut settings.way_home_marker,
            Toggle::DoubleTapSprint => &mut settings.double_tap_sprint,
            Toggle::FovEffects => &mut settings.fov_effects,
            Toggle::MinecraftAssets => &mut settings.minecraft_assets,
            Toggle::Music(_) => return,
        } = on;
        // Without the install, its textures cannot come first.
        if !settings.minecraft_assets && settings.textures == TextureOrder::Minecraft {
            settings.textures = TextureOrder::Voxmar;
        }
    }
}

/// A setting chosen from a few options; clicking steps to the next one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Choice {
    FarDetail,
    AntiAliasing,
    ShaderPack,
    Shadows,
    Water,
    Textures,
    Particles,
    Font,
    UiScale,
    UiSounds,
    Language,
}

impl Choice {
    fn key(self) -> &'static str {
        match self {
            Choice::FarDetail => "settings.far_detail",
            Choice::Font => "settings.font",
            Choice::UiScale => "settings.ui_scale",
            Choice::UiSounds => "settings.ui_sounds",
            Choice::AntiAliasing => "settings.anti_aliasing",
            Choice::ShaderPack => "settings.shader_pack",
            Choice::Shadows => "settings.shadows",
            Choice::Water => "settings.water",
            Choice::Textures => "settings.textures",
            Choice::Particles => "settings.particles",
            Choice::Language => "settings.language",
        }
    }

    fn text(self, settings: &Settings, packs: &ShaderPacks, locale: &Locale) -> String {
        let words = |key: &str| locale.get(key).to_owned();
        match self {
            Choice::FarDetail => words(settings.far_detail.key()),
            // Fonts are named in lang, and shader packs too where a language names them.
            Choice::Font => words(settings.font.key()),
            Choice::UiScale => format!("{}%", settings.ui_scale),
            Choice::UiSounds => words(settings.ui_sounds.key()),
            Choice::AntiAliasing => words(settings.anti_aliasing.key()),
            Choice::ShaderPack => {
                packs.get(&settings.shader_pack).named(locale).to_owned()
            }
            Choice::Shadows => words(settings.shadows.key()),
            Choice::Water => words(settings.water_quality.key()),
            Choice::Textures => words(settings.textures.key()),
            Choice::Particles => words(settings.particles.key()),
            // Each language by its own name, so a player can find theirs from any other.
            Choice::Language => locale.language_name(locale.code()).to_owned(),
        }
    }

    /// Moves to the next option, or the previous one.
    fn step(self, settings: &mut Settings, packs: &ShaderPacks, locale: &Locale, forward: bool) {
        match self {
            Choice::Language => {
                let codes: Vec<&str> = locale.languages.iter().map(|language| language.code.as_str()).collect();
                settings.language = cycle(&codes, locale.code(), forward).to_owned();
            }
            Choice::FarDetail => settings.far_detail = cycle(&FarDetail::ALL, settings.far_detail, forward),
            Choice::Font => settings.font = cycle(&InterfaceFont::ALL, settings.font, forward),
            Choice::UiScale => settings.ui_scale = cycle(&UI_SCALES, nearest(&UI_SCALES, settings.ui_scale), forward),
            Choice::UiSounds => settings.ui_sounds = cycle(&UiSounds::ALL, settings.ui_sounds, forward),
            Choice::AntiAliasing => settings.anti_aliasing = cycle(&AntiAliasing::ALL, settings.anti_aliasing, forward),
            Choice::ShaderPack => {
                let current = settings.shader_pack.clone();
                let mut pick = packs.next(&current).to_owned();
                if !forward {
                    // The pack whose next is the current one; there are only a few.
                    let mut previous = current.clone();
                    for _ in 0..64 {
                        let next = packs.next(&previous).to_owned();
                        if next == current {
                            break;
                        }
                        previous = next;
                    }
                    pick = previous;
                }
                settings.shader_pack = pick;
            }
            Choice::Shadows => settings.shadows = cycle(&ShadowQuality::ALL, settings.shadows, forward),
            Choice::Water => settings.water_quality = cycle(&WaterQuality::ALL, settings.water_quality, forward),
            Choice::Textures => {
                settings.textures = cycle(TextureOrder::choices(settings.minecraft_assets), settings.textures, forward);
            }
            Choice::Particles => settings.particles = cycle(&ParticleAmount::ALL, settings.particles, forward),
        }
    }
}

/// The option after (or before) `current` in `all`, wrapping around.
fn cycle<T: Copy + PartialEq>(all: &[T], current: T, forward: bool) -> T {
    let i = all.iter().position(|&o| o == current).unwrap_or(0);
    let step = if forward { 1 } else { all.len() - 1 };
    all[(i + step) % all.len()]
}

/// Keys the game keeps for itself, which no binding may take: moving, jumping and
/// descending, flight, dropping, the music, the debug overlay, the hotbar and commands.
/// (R, which respawns on the death screen, stays bindable: it is the recipes key's default,
/// and while dead no screen opens, so a binding on R does nothing else there.)
const FIXED_KEYS: [KeyCode; 20] = [
    KeyCode::KeyW,
    KeyCode::KeyA,
    KeyCode::KeyS,
    KeyCode::KeyD,
    KeyCode::Space,
    KeyCode::ShiftLeft,
    KeyCode::KeyF,
    KeyCode::KeyQ,
    KeyCode::KeyM,
    KeyCode::F3,
    KeyCode::Slash,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// A key the player can rebind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Binding {
    Sprint,
    Dodge,
    Zoom,
    Chat,
    Inventory,
    Character,
    Journal,
    Quests,
    Recipes,
    Uses,
    Screenshot,
    /// A quick prayer's key, by its place.
    Prayer(usize),
}

impl Binding {
    const ALL: [Binding; 16] = [
        Binding::Sprint,
        Binding::Dodge,
        Binding::Zoom,
        Binding::Chat,
        Binding::Inventory,
        Binding::Character,
        Binding::Journal,
        Binding::Quests,
        Binding::Recipes,
        Binding::Uses,
        Binding::Screenshot,
        Binding::Prayer(0),
        Binding::Prayer(1),
        Binding::Prayer(2),
        Binding::Prayer(3),
        Binding::Prayer(4),
    ];

    /// Binds `key`. A key another binding has trades places with it, so both still work;
    /// a key the game keeps for itself (`FIXED_KEYS`) is refused. Returns whether anything
    /// changed.
    fn bind(self, settings: &mut Settings, key: Key) -> bool {
        let old = self.get(settings);
        if old == key || FIXED_KEYS.contains(&key.0) {
            return false;
        }
        if let Some(other) = Binding::ALL.into_iter().find(|other| *other != self && other.get(settings) == key) {
            other.set(settings, old);
        }
        self.set(settings, key);
        true
    }

    fn key(self) -> &'static str {
        match self {
            Binding::Sprint => "settings.key.sprint",
            Binding::Dodge => "settings.key.dodge",
            Binding::Zoom => "settings.key.zoom",
            Binding::Chat => "settings.key.chat",
            Binding::Inventory => "settings.key.inventory",
            Binding::Character => "settings.key.character",
            Binding::Journal => "settings.key.journal",
            Binding::Quests => "settings.key.quests",
            Binding::Recipes => "settings.key.recipes",
            Binding::Uses => "settings.key.uses",
            Binding::Screenshot => "settings.key.screenshot",
            Binding::Prayer(i) => ["settings.key.prayer_1", "settings.key.prayer_2", "settings.key.prayer_3", "settings.key.prayer_4", "settings.key.prayer_5"][i],
        }
    }

    fn get(self, settings: &Settings) -> Key {
        match self {
            Binding::Sprint => settings.sprint_key,
            Binding::Dodge => settings.dodge_key,
            Binding::Zoom => settings.zoom_key,
            Binding::Chat => settings.chat_key,
            Binding::Inventory => settings.inventory_key,
            Binding::Character => settings.character_key,
            Binding::Journal => settings.journal_key,
            Binding::Quests => settings.quests_key,
            Binding::Recipes => settings.recipes_key,
            Binding::Uses => settings.uses_key,
            Binding::Screenshot => settings.screenshot_key,
            Binding::Prayer(i) => settings.prayer_keys[i],
        }
    }

    fn set(self, settings: &mut Settings, key: Key) {
        *match self {
            Binding::Sprint => &mut settings.sprint_key,
            Binding::Dodge => &mut settings.dodge_key,
            Binding::Zoom => &mut settings.zoom_key,
            Binding::Chat => &mut settings.chat_key,
            Binding::Inventory => &mut settings.inventory_key,
            Binding::Character => &mut settings.character_key,
            Binding::Journal => &mut settings.journal_key,
            Binding::Quests => &mut settings.quests_key,
            Binding::Recipes => &mut settings.recipes_key,
            Binding::Uses => &mut settings.uses_key,
            Binding::Screenshot => &mut settings.screenshot_key,
            Binding::Prayer(i) => &mut settings.prayer_keys[i],
        } = key;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Slider(Slider),
    Switch(Toggle),
    Choice(Choice),
    Key(Binding),
    Dropdown(Dropdown),
}

/// A setting chosen from a list that drops down under its button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dropdown {
    Resolution,
}

impl Dropdown {
    fn key(self) -> &'static str {
        match self {
            Dropdown::Resolution => "settings.resolution",
        }
    }

    /// The options, as values and what the list shows for each: the resolutions as the
    /// pixels they are on this window.
    fn options(self, window: Option<UVec2>) -> Vec<(i32, String)> {
        match self {
            Dropdown::Resolution => RESOLUTIONS
                .iter()
                .map(|&percent| {
                    let label = match window {
                        Some(size) => {
                            let (w, h) = (size.x as i32 * percent / 100, size.y as i32 * percent / 100);
                            format!("{w} × {h} ({percent}%)")
                        }
                        None => format!("{percent}%"),
                    };
                    (percent, label)
                })
                .collect(),
        }
    }

    fn get(self, settings: &Settings) -> i32 {
        match self {
            Dropdown::Resolution => settings.resolution,
        }
    }

    fn set(self, settings: &mut Settings, value: i32) {
        match self {
            Dropdown::Resolution => settings.resolution = value,
        }
    }
}

impl Control {
    /// The key of the setting's name; its hint's is the same with `.hint` after it.
    fn key(self) -> &'static str {
        match self {
            Control::Slider(slider) => slider.key(),
            Control::Switch(toggle) => toggle.key(),
            Control::Choice(choice) => choice.key(),
            Control::Key(binding) => binding.key(),
            Control::Dropdown(dropdown) => dropdown.key(),
        }
    }

    /// The key of what the setting does, shown at the foot of the panel while the pointer is
    /// on it.
    fn hint(self) -> String {
        format!("{}.hint", self.key())
    }
}

/// The panel's tabs (by the keys of their names), each a page of settings. A new setting
/// goes into the page where players would look for it.
/// How many tabs the settings have.
pub(crate) const TABS: usize = SECTIONS.len();

const SECTIONS: [(&str, &[Control]); 6] = [
    (
        "settings.tab.video",
        &[
            Control::Slider(Slider::ViewDistance),
            Control::Choice(Choice::FarDetail),
            Control::Dropdown(Dropdown::Resolution),
            Control::Slider(Slider::Fov),
            Control::Switch(Toggle::FovEffects),
            Control::Choice(Choice::AntiAliasing),
            Control::Switch(Toggle::Vsync),
            Control::Switch(Toggle::MinecraftAssets),
            Control::Choice(Choice::Textures),
        ],
    ),
    (
        "settings.tab.graphics",
        &[
            Control::Choice(Choice::ShaderPack),
            Control::Choice(Choice::Shadows),
            Control::Switch(Toggle::VolumetricLight),
            Control::Choice(Choice::Water),
            Control::Choice(Choice::Particles),
        ],
    ),
    ("settings.tab.audio", &[Control::Slider(Slider::Music), Control::Slider(Slider::Sounds), Control::Slider(Slider::Ambience)]),
    (
        "settings.tab.controls",
        &[
            Control::Slider(Slider::Sensitivity),
            Control::Switch(Toggle::DoubleTapSprint),
            Control::Slider(Slider::Zoom),
            Control::Switch(Toggle::CinematicZoom),
            Control::Switch(Toggle::ZoomInSurvival),
            Control::Switch(Toggle::ToggleGuard),
            Control::Slider(Slider::FlySpeed),
        ],
    ),
    (
        "settings.tab.keys",
        &[
            Control::Key(Binding::Inventory),
            Control::Key(Binding::Character),
            Control::Key(Binding::Journal),
            Control::Key(Binding::Quests),
            Control::Key(Binding::Recipes),
            Control::Key(Binding::Uses),
            Control::Key(Binding::Screenshot),
            Control::Key(Binding::Chat),
            Control::Key(Binding::Sprint),
            Control::Key(Binding::Dodge),
            Control::Key(Binding::Zoom),
            Control::Key(Binding::Prayer(0)),
            Control::Key(Binding::Prayer(1)),
            Control::Key(Binding::Prayer(2)),
            Control::Key(Binding::Prayer(3)),
            Control::Key(Binding::Prayer(4)),
        ],
    ),
    (
        "settings.tab.interface",
        &[
            Control::Choice(Choice::Language),
            Control::Choice(Choice::Font),
            Control::Choice(Choice::UiScale),
            Control::Choice(Choice::UiSounds),
            Control::Slider(Slider::HudRaise),
            Control::Switch(Toggle::DamageNumbers),
            Control::Switch(Toggle::ScreenShake),
            Control::Switch(Toggle::WayHomeMarker),
            Control::Switch(Toggle::DebugOverlay),
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
/// The panel's title: "Game paused" in game, "Settings" on the title screen.
#[derive(Component)]
struct MenuTitle;
/// Parts of the panel that only make sense in some places.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ShownWhen {
    InGame,
    OnTitle,
    /// For admins in game, and always on the title screen, where it applies to the next
    /// world.
    Admin,
    /// With Minecraft assets on.
    MinecraftAssets,
}
/// A tab, by its index in `SECTIONS`.
#[derive(Component)]
struct SectionTab(usize);
/// A tab's page of settings.
#[derive(Component)]
struct SectionPage(usize);
/// A row of the panel, which shows its hint while the pointer is on it.
#[derive(Component)]
struct SettingRow(Control);
/// Where the hovered row's hint shows, and the box around it, as tall as the longest
/// hint in the player's language.
#[derive(Component)]
struct HintText;
#[derive(Component)]
struct HintBox;
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

/// A dropdown's button, value and list, and one option in the list with its value.
#[derive(Component)]
struct DropdownButton(Dropdown);
#[derive(Component)]
struct DropdownValue(Dropdown);
#[derive(Component)]
struct DropdownList(Dropdown);
#[derive(Component)]
struct DropdownOption(Dropdown, i32);

/// The dropdown whose list is open.
#[derive(Resource, Default)]
struct OpenDropdown(Option<Dropdown>);

/// The binding waiting for its new key, after its button was clicked.
#[derive(Resource, Default)]
struct Rebinding(Option<Binding>);

/// The tab showing; kept while the panel is closed, so it opens where it was left.
#[derive(Resource, Default)]
pub(crate) struct OpenSection(pub(crate) usize);

/// A settings tab to open at startup, by name (`--tab`, for captures).
#[derive(Resource)]
pub struct StartTab(pub String);

fn open_start_tab(start: Option<Res<StartTab>>, mut open: ResMut<OpenSection>) {
    // Tabs go by the last word of their name's key: `settings.tab.video` is `video`.
    let named = |key: &str, wanted: &str| key.rsplit('.').next().is_some_and(|name| name.eq_ignore_ascii_case(wanted));
    if let Some(index) = start.and_then(|start| SECTIONS.iter().position(|(key, _)| named(key, &start.0))) {
        open.0 = index;
    }
}

const PANEL_WIDTH: f32 = 430.0 * PX;
const TAB_WIDTH: f32 = 72.0 * PX;
const ROW: f32 = 18.0 * PX;
/// A control's height: room for a line of any language's letters inside its frame.
const CONTROL: f32 = 16.0 * PX;
const SLIDER_WIDTH: f32 = 96.0 * PX;
const VALUE_WIDTH: f32 = 62.0 * PX;
/// Every control takes the same width at the right of its row, so their edges line up: a
/// slider and its value, or a button.
const CONTROL_WIDTH: f32 = SLIDER_WIDTH + space::GAP + VALUE_WIDTH;
/// Lines kept for the hint at the foot of the panel: the longest hint wraps to two.
const HINT_LINES: usize = 2;
/// The faint light on the row under the pointer.
const ROW_LIT: Color = Color::srgba(0.37, 0.88, 0.82, 0.06);

/// For the gate: the tab of the page whose name's key is `key`.
pub(crate) fn tab_button(world: &mut World, key: &str) -> Option<Entity> {
    let index = SECTIONS.iter().position(|(name, _)| *name == key)?;
    world.query::<(Entity, &SectionTab)>().iter(world).find(|(_, tab)| tab.0 == index).map(|(entity, _)| entity)
}

/// For the gate: the music's categories whose switches are on screen, by name, in the
/// order they show, each with its switch and the words beside it (on or off).
pub(crate) fn music_switches_shown(world: &mut World) -> Vec<(String, Entity, String)> {
    let mut labels = world.query::<(&SwitchLabel, &Text)>();
    let labels: Vec<(Toggle, String)> = labels.iter(world).map(|(label, text)| (label.0, text.0.clone())).collect();
    let mut switches = world.query::<(Entity, &Switch, &ComputedNode, &bevy::ui::UiGlobalTransform)>();
    let mut shown: Vec<(f32, String, Entity, String)> = switches
        .iter(world)
        .filter(|(_, _, node, _)| node.size().x > 0.0)
        .filter_map(|(entity, switch, _, at)| match switch.0 {
            Toggle::Music(category) => {
                let words = labels.iter().find(|(toggle, _)| *toggle == switch.0).map(|(_, words)| words.clone()).unwrap_or_default();
                Some((at.translation.y, category.name.to_owned(), entity, words))
            }
            _ => None,
        })
        .collect();
    shown.sort_by(|a, b| a.0.total_cmp(&b.0));
    shown.into_iter().map(|(_, name, entity, words)| (name, entity, words)).collect()
}

/// The settings page the music's categories are switched on and off in, after its own.
const MUSIC_PAGE: &str = "settings.tab.audio";

/// The controls of each page: its own, and on the audio page a switch for each category of
/// music.
fn pages(music: &[MusicSwitch]) -> Vec<(&'static str, Vec<Control>)> {
    SECTIONS
        .iter()
        .map(|(name, controls)| {
            let mut controls = controls.to_vec();
            if *name == MUSIC_PAGE {
                controls.extend(music.iter().map(|&category| Control::Switch(Toggle::Music(category))));
            }
            (*name, controls)
        })
        .collect()
}

fn spawn_menu(mut commands: Commands, font: Res<UiFont>, categories: Option<Res<MusicCategories>>) {
    let font = &font.0;
    let pages = pages(&categories.map(|categories| music_switches(&categories)).unwrap_or_default());
    let tallest = pages.iter().map(|(_, controls)| controls.len()).max().unwrap_or(0) as f32;
    commands.spawn((MenuRoot, screen(SCRIM), GlobalZIndex(1), Visibility::Hidden)).with_children(|root| {
        root.spawn(panel(PANEL_WIDTH)).with_children(|panel| {
            let title = Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() };
            panel.spawn(title).with_children(|row| {
                row.spawn(shadowed(font, "◆", BODY, ACCENT));
                row.spawn((MenuTitle, shadowed(font, "", LARGE, GOLD)));
                row.spawn(shadowed(font, "◆", BODY, ACCENT));
            });
            spawn_rule(panel, 4.0 * PX);
            let body = Node { column_gap: Val::Px(space::SECTION), min_height: Val::Px(tallest * ROW), ..default() };
            panel.spawn(body).with_children(|body| {
                let tabs = Node { width: Val::Px(TAB_WIDTH), flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), ..default() };
                body.spawn(tabs).with_children(|tabs| {
                    for (index, (name, _)) in SECTIONS.iter().enumerate() {
                        tabs.spawn(Node::default()).with_children(|row| {
                            spawn_button(row, font, name, ButtonStyle::Secondary, SectionTab(index));
                        });
                    }
                });
                body.spawn(Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, ..default() }).with_children(|pages_node| {
                    for (index, (_, controls)) in pages.iter().enumerate() {
                        let page = Node { flex_direction: FlexDirection::Column, display: Display::None, ..default() };
                        pages_node.spawn((SectionPage(index), page)).with_children(|page| {
                            for &control in controls {
                                spawn_row(page, font, control);
                            }
                        });
                    }
                });
            });
            spawn_rule(panel, 4.0 * PX);
            let hint = Node { overflow: Overflow::clip(), ..default() };
            panel.spawn((HintBox, hint)).with_child((HintText, text(font, "", BODY, MUTED)));
            panel.spawn(button_row(4.0 * PX)).with_children(|row| {
                spawn_button(row, font, "menu.back_to_game", ButtonStyle::Primary, (MenuButton::Resume, ShownWhen::InGame));
                spawn_button(row, font, "menu.done", ButtonStyle::Primary, (MenuButton::Done, ShownWhen::OnTitle));
            });
            panel.spawn((button_row(4.0 * PX), ShownWhen::InGame)).with_children(|row| {
                // Its words depend on the game: see `show_menu`.
                spawn_button(row, font, "", ButtonStyle::Secondary, MenuButton::QuitToTitle);
                spawn_button(row, font, "menu.quit_game", ButtonStyle::Secondary, MenuButton::Quit);
            });
        });
        crate::hud::screens::mark_screen(root, crate::hud::screens::GameScreen::Menu);
    });
}

/// A label on the left and its control on the right.
fn spawn_row(panel: &mut ChildSpawnerCommands, font: &Handle<Font>, control: Control) {
    let node = Node { height: Val::Px(ROW), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() };
    let mut row = panel.spawn((Button, Silent, SettingRow(control), node, BackgroundColor(Color::NONE)));
    if control == Control::Slider(Slider::FlySpeed) {
        row.insert(ShownWhen::Admin);
    }
    if let Control::Switch(Toggle::Music(category)) = control
        && category.borrowed_only
    {
        row.insert(ShownWhen::MinecraftAssets);
    }
    row.with_children(|row| {
        row.spawn((Localized(control.key()), text(font, "", BODY, TEXT)));
        match control {
            Control::Slider(slider) => spawn_slider(row, font, slider),
            Control::Switch(toggle) => spawn_switch(row, font, toggle),
            Control::Choice(choice) => spawn_choice(row, font, choice),
            Control::Key(binding) => spawn_key(row, font, binding),
            Control::Dropdown(dropdown) => spawn_dropdown(row, font, dropdown),
        }
    });
}

/// A recessed track lit teal up to the value, a gold knob at its end, and the value.
fn spawn_slider(row: &mut ChildSpawnerCommands, font: &Handle<Font>, slider: Slider) {
    let knob = Piece::Knob.size();
    let area = Node { width: Val::Px(SLIDER_WIDTH), height: Val::Px(knob.y), align_items: AlignItems::Center, ..default() };
    let track = Node { width: Val::Percent(100.0), height: Val::Px(6.0 * PX), padding: UiRect::all(Val::Px(PX)), ..default() };
    let fill = Node { height: Val::Percent(100.0), ..default() };
    let knob = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(-knob.x / 2.0),
        top: Val::Px((4.0 * PX - knob.y) / 2.0),
        width: Val::Px(knob.x),
        height: Val::Px(knob.y),
        ..default()
    };
    let value = Node { width: Val::Px(VALUE_WIDTH), justify_content: JustifyContent::FlexEnd, ..default() };
    row.spawn(Node { width: Val::Px(CONTROL_WIDTH), align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() }).with_children(|control| {
        control.spawn((Button, SliderTrack(slider), RelativeCursorPosition::default(), area)).with_children(|area| {
            area.spawn((track, Piece::Well.image())).with_children(|track| {
                track.spawn((SliderFill(slider), fill, Piece::Bar.image())).with_child((knob, Piece::Knob.image(), HoverLight));
            });
        });
        control.spawn(value).with_child((SliderValue(slider), text(font, "", BODY, SOFT)));
    });
}

/// A recess with a gold knob that slides right, and lights teal, when on; and beside it,
/// in words, whether it is.
fn spawn_switch(row: &mut ChildSpawnerCommands, font: &Handle<Font>, toggle: Toggle) {
    let size = Piece::KnobSmall.size();
    let track = Node {
        width: Val::Px(2.5 * size.x + 4.0 * PX),
        height: Val::Px(size.y + 4.0 * PX),
        padding: UiRect::all(Val::Px(2.0 * PX)),
        ..default()
    };
    let knob = Node { width: Val::Px(size.x), height: Val::Px(size.y), ..default() };
    let cell = Node { width: Val::Px(CONTROL_WIDTH), align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() };
    row.spawn(cell).with_children(|cell| {
        cell.spawn((Button, Switch(toggle), track, Piece::Well.image(), HoverLight)).with_child((knob, Piece::KnobSmall.image(), HoverLight));
        cell.spawn((SwitchLabel(toggle), text(font, "", BODY, SOFT)));
    });
}

/// The words beside a switch: on or off.
#[derive(Component)]
struct SwitchLabel(Toggle);

/// A button showing the current option between arrows: a click steps forward, a right
/// click back.
fn spawn_choice(row: &mut ChildSpawnerCommands, font: &Handle<Font>, choice: Choice) {
    let mut button = Entity::PLACEHOLDER;
    row.spawn(Node { width: Val::Px(CONTROL_WIDTH), ..default() }).with_children(|cell| {
        button = spawn_button(cell, font, "", ButtonStyle::Secondary, ChoiceButton(choice));
    });
    row.commands().entity(button).insert(Node {
        flex_grow: 1.0,
        height: Val::Px(CONTROL),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    });
    row.commands().entity(button).with_child((ChoiceValue(choice), Node::default()));
}

/// A button showing the bound key; clicked, it waits for the next key pressed.
fn spawn_key(row: &mut ChildSpawnerCommands, font: &Handle<Font>, binding: Binding) {
    let mut button = Entity::PLACEHOLDER;
    row.spawn(Node { width: Val::Px(CONTROL_WIDTH), ..default() }).with_children(|cell| {
        button = spawn_button(cell, font, "", ButtonStyle::Secondary, KeyButton(binding));
    });
    row.commands().entity(button).insert(Node {
        flex_grow: 1.0,
        height: Val::Px(CONTROL),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    });
    row.commands().entity(button).with_child((KeyValue(binding), Node::default()));
}

/// A button showing the current option; clicked, it drops the list of options under it.
fn spawn_dropdown(row: &mut ChildSpawnerCommands, font: &Handle<Font>, dropdown: Dropdown) {
    let mut button = Entity::PLACEHOLDER;
    row.spawn(Node { width: Val::Px(CONTROL_WIDTH), ..default() }).with_children(|cell| {
        button = spawn_button(cell, font, "", ButtonStyle::Secondary, DropdownButton(dropdown));
        let list = Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(100.0),
            left: Val::ZERO,
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(PX),
            padding: UiRect::all(Val::Px(3.0 * PX)),
            margin: UiRect::top(Val::Px(PX)),
            display: Display::None,
            ..default()
        };
        cell.spawn((DropdownList(dropdown), list, Piece::Tooltip.image(), GlobalZIndex(5)));
    });
    row.commands().entity(button).insert(Node {
        flex_grow: 1.0,
        height: Val::Px(CONTROL),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    });
    row.commands().entity(button).with_child((DropdownValue(dropdown), Node::default()));
}

/// Opens a dropdown's list on its button, picks the option clicked, and closes the list on
/// any other click.
#[allow(clippy::too_many_arguments)]
fn click_dropdowns(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    font: Res<UiFont>,
    mut settings: ResMut<Settings>,
    mut open: ResMut<OpenDropdown>,
    buttons: Query<(&DropdownButton, &Interaction)>,
    options: Query<(&DropdownOption, &Interaction)>,
    mut lists: Query<(Entity, &DropdownList, &mut Node)>,
    window: Query<&Window, With<PrimaryWindow>>,
    root: Query<&Visibility, With<MenuRoot>>,
) {
    let shown = root.single().is_ok_and(|v| *v != Visibility::Hidden);
    let mut wanted = open.0.filter(|_| shown);
    if mouse.just_pressed(MouseButton::Left) {
        if let Some((option, _)) = options.iter().find(|(_, i)| **i == Interaction::Pressed) {
            if option.0.get(&settings) != option.1 {
                option.0.set(&mut settings, option.1);
            }
            wanted = None;
        } else if let Some((button, _)) = buttons.iter().find(|(_, i)| **i == Interaction::Pressed) {
            wanted = if wanted == Some(button.0) { None } else { Some(button.0) };
        } else {
            wanted = None;
        }
    }
    if open.0 == wanted {
        return;
    }
    open.0 = wanted;
    let size = window.single().ok().map(|w| w.physical_size());
    for (entity, list, mut node) in &mut lists {
        let display = if wanted == Some(list.0) { Display::Flex } else { Display::None };
        node.display = display;
        commands.entity(entity).despawn_children();
        if display == Display::None {
            continue;
        }
        let current = list.0.get(&settings);
        commands.entity(entity).with_children(|list_node| {
            for (value, label) in list.0.options(size) {
                let node = Node { height: Val::Px(11.0 * PX), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() };
                let mut option = list_node.spawn((Button, Silent, DropdownOption(list.0, value), node, BackgroundColor(Color::NONE)));
                option.with_child(text(&font.0, label, BODY, if value == current { GOLD } else { TEXT }));
            }
        });
    }
}

/// Lights the dropdown option under the pointer.
fn hover_options(mut options: Query<(&Interaction, &mut BackgroundColor), With<DropdownOption>>) {
    for (interaction, mut background) in &mut options {
        let color = if *interaction == Interaction::None { Color::NONE } else { ACCENT.with_alpha(0.22) };
        background.set_if_neq(BackgroundColor(color));
    }
}

/// F3 toggles the debug overlay, menu or not.
fn toggle_debug(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<Settings>) {
    if keys.just_pressed(KeyCode::F3) {
        settings.debug_overlay = !settings.debug_overlay;
    }
}

/// Sets a slider's value from the cursor while it is held, snapped to the slider's steps.
/// The wheel over a slider moves it a step.
fn drag_sliders(
    mut settings: ResMut<Settings>,
    sliders: Query<(&SliderTrack, &Interaction, &RelativeCursorPosition)>,
    wheel: Res<AccumulatedMouseScroll>,
) {
    for (slider, interaction, cursor) in &sliders {
        let (min, max, step) = slider.0.range();
        if *interaction == Interaction::Hovered && wheel.delta.y != 0.0 {
            let value = (slider.0.get(&settings) + step * wheel.delta.y.signum() as i32).clamp(min, max);
            if slider.0.get(&settings) != value {
                slider.0.set(&mut settings, value);
            }
        }
        let (Interaction::Pressed, Some(cursor)) = (interaction, cursor.normalized) else { continue };
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

/// A click steps a choice forward; a right click (or Control-click) steps it back.
fn click_choices(
    mut settings: ResMut<Settings>,
    packs: Res<ShaderPacks>,
    locale: Res<Locale>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    choices: Query<(&ChoiceButton, &Interaction)>,
) {
    let back_click = mouse.just_pressed(MouseButton::Right);
    for (choice, interaction) in &choices {
        let pressed = *interaction == Interaction::Pressed && mouse.just_pressed(MouseButton::Left);
        if pressed || (back_click && *interaction == Interaction::Hovered) {
            let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
            choice.0.step(&mut settings, &packs, &locale, pressed && !control);
        }
    }
}

/// Clicking a key's button waits for a new key; clicking it again stops waiting.
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
        binding.bind(&mut settings, Key(key));
    }
}

/// A tab clicked opens its page; Tab and Shift-Tab step through the pages.
fn pick_section(
    tabs: Query<(&SectionTab, &Interaction), Changed<Interaction>>,
    keys: Res<ButtonInput<KeyCode>>,
    rebinding: Res<Rebinding>,
    mut open: ResMut<OpenSection>,
    root: Query<&Visibility, With<MenuRoot>>,
) {
    for (tab, interaction) in &tabs {
        if *interaction == Interaction::Pressed && open.0 != tab.0 {
            open.0 = tab.0;
        }
    }
    let shown = root.single().is_ok_and(|v| *v != Visibility::Hidden);
    if shown && rebinding.0.is_none() && keys.just_pressed(KeyCode::Tab) {
        let back = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        open.0 = (open.0 + if back { SECTIONS.len() - 1 } else { 1 }) % SECTIONS.len();
    }
}

/// Shows the open tab's page, marks its tab, and says what the hovered row does.
fn show_section(
    mut commands: Commands,
    open: Res<OpenSection>,
    tabs: Query<(Entity, &SectionTab, Has<Selected>)>,
    mut pages: Query<(&SectionPage, &mut Node)>,
    mut rows: Query<(&SettingRow, &Interaction, &mut BackgroundColor)>,
    mut hint: Query<&mut Text, With<HintText>>,
    mut hint_box: Query<&mut Node, (With<HintBox>, Without<SectionPage>)>,
    locale: Res<Locale>,
) {
    if let Ok(mut node) = hint_box.single_mut() {
        // As tall as the longest hint, so the panel keeps its height as the pointer moves.
        let height = Val::Px(HINT_LINES as f32 * locale.line_height() * BODY);
        if node.height != height {
            node.height = height;
        }
    }
    for (entity, tab, selected) in &tabs {
        if (tab.0 == open.0) != selected {
            if selected {
                commands.entity(entity).remove::<Selected>();
            } else {
                commands.entity(entity).insert(Selected);
            }
        }
    }
    for (page, mut node) in &mut pages {
        let display = if page.0 == open.0 { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // The row under the pointer lights faintly, and its hint shows; with none, how to get one.
    for (_, interaction, mut background) in &mut rows {
        let lit = if *interaction == Interaction::None { Color::NONE } else { ROW_LIT };
        background.set_if_neq(BackgroundColor(lit));
    }
    let hovered = rows.iter().find(|(_, interaction, _)| **interaction != Interaction::None).map(|(row, _, _)| row.0.hint());
    let hovered = hovered.as_deref().map_or_else(|| locale.get("settings.hint.none"), |key| locale.get(key));
    if let Ok(mut text) = hint.single_mut()
        && text.0 != hovered
    {
        hovered.clone_into(&mut text.0);
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
    mut commands: Commands,
    font: Res<UiFont>,
    settings: Res<Settings>,
    mut fills: Query<(&SliderFill, &mut Node), Without<Switch>>,
    mut values: Query<(&SliderValue, &mut Text), Without<SwitchLabel>>,
    mut switch_labels: Query<(&SwitchLabel, &mut Text, &mut TextColor), Without<SliderValue>>,
    mut switches: Query<(&Switch, &mut Node, &mut ImageNode)>,
    choices: Query<(Entity, &ChoiceValue)>,
    keys: Query<(Entity, &KeyValue)>,
    dropdowns: Query<(Entity, &DropdownValue)>,
    window: Query<&Window, With<PrimaryWindow>>,
    rebinding: Res<Rebinding>,
    packs: Res<ShaderPacks>,
    locale: Res<Locale>,
) {
    // Choice, key and dropdown buttons show their value as one text, replaced when it
    // changes.
    let mut show = |entity: Entity, value: String, color: Color| {
        commands.entity(entity).despawn_children().with_child(shadowed(&font.0, value, BODY, color));
    };
    for (entity, key) in &keys {
        if rebinding.0 == Some(key.0) {
            show(entity, locale.get("settings.press_a_key").into(), ACCENT);
        } else {
            show(entity, key.0.get(&settings).name(&locale), TEXT);
        }
    }
    for (entity, choice) in &choices {
        show(entity, format!("◂ {} ▸", choice.0.text(&settings, &packs, &locale)), TEXT);
    }
    let size = window.single().ok().map(|w| w.physical_size());
    for (entity, dropdown) in &dropdowns {
        let current = dropdown.0.get(&settings);
        let label = dropdown.0.options(size).into_iter().find(|(value, _)| *value == current).map_or_else(String::new, |(_, label)| label);
        // The button keeps it short: the pixels, without the share.
        let short = label.split(" (").next().unwrap_or(&label).to_owned();
        show(entity, format!("{short} ▾"), TEXT);
    }
    for (fill, mut node) in &mut fills {
        let (min, max, _) = fill.0.range();
        node.width = Val::Percent((fill.0.get(&settings) - min) as f32 / (max - min) as f32 * 100.0);
    }
    for (value, mut text) in &mut values {
        text.0 = value.0.text(value.0.get(&settings), &locale);
    }
    for (label, mut text, mut color) in &mut switch_labels {
        let on = label.0.get(&settings);
        let words = locale.get(if on { "settings.value.on" } else { "settings.value.off" });
        if text.0 != words {
            words.clone_into(&mut text.0);
        }
        color.set_if_neq(TextColor(if on { TEXT } else { MUTED }));
    }
    for (switch, mut node, mut image) in &mut switches {
        let on = switch.0.get(&settings);
        node.justify_content = if on { JustifyContent::FlexEnd } else { JustifyContent::FlexStart };
        image.image = if on { Piece::Bar.handle() } else { Piece::Well.handle() };
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
    mut title: Query<&mut Text, With<MenuTitle>>,
    mut texts: Query<&mut Text, Without<MenuTitle>>,
    mut rebinding: ResMut<Rebinding>,
    locale: Res<Locale>,
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
    if let Ok(mut text) = title.single_mut() {
        let heading = locale.get(if in_game { "settings.paused" } else { "settings.title" });
        if text.0 != heading {
            heading.clone_into(&mut text.0);
        }
    }
    for (mut node, when) in &mut nodes {
        let shown = match when {
            ShownWhen::InGame => in_game,
            ShownWhen::OnTitle => !in_game,
            ShownWhen::Admin => player.admin || !in_game,
            ShownWhen::MinecraftAssets => settings.minecraft_assets,
        };
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // Leaving a server saves nothing of ours, so say what it does.
    let label = locale.get(if net.is_some_and(|net| net.is_remote()) { "menu.disconnect" } else { "menu.save_and_quit" });
    for (button, children) in &quit_to_title {
        if matches!(button, MenuButton::QuitToTitle)
            && let Some(mut text) = children.first().and_then(|&child| texts.get_mut(child).ok())
            && text.0 != label
        {
            label.clone_into(&mut text.0);
        }
    }
}

/// Seconds after the last change that settings are written, so dragging a slider writes
/// the file once.
const SAVE_DELAY: f32 = 1.0;

/// Keeps the settings file current: a moment after any change, and when the game quits
/// however it quits (the window closed, Cmd-Q, Quit game).
fn autosave(time: Res<Time<Real>>, settings: Res<Settings>, mut exits: MessageReader<AppExit>, mut due: Local<Option<f32>>, mut started: Local<bool>) {
    let now = time.elapsed_secs();
    // The settings as loaded at startup count as a change; they need no writing.
    if settings.is_changed() && *started {
        *due = Some(now + SAVE_DELAY);
    }
    *started = true;
    let quitting = exits.read().count() > 0;
    if due.is_some_and(|at| now >= at) || (quitting && due.is_some()) {
        settings.save();
        *due = None;
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
#[allow(clippy::too_many_arguments)]
fn apply(
    settings: Res<Settings>,
    mut world: ResMut<ClientWorld>,
    mut cycle: ResMut<DayCycle>,
    mut quality: ResMut<Quality>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut camera: Query<(Entity, &mut Msaa), With<Camera3d>>,
    mut water: ResMut<WaterQuality>,
    mut scale: ResMut<RenderScale>,
    mut commands: Commands,
) {
    water.set_if_neq(settings.water_quality);
    if world.view_distance != settings.view_distance {
        world.set_view_distance(settings.view_distance);
        cycle.view_distance = lod::view_distance(settings.view_distance);
    }
    quality.set_if_neq(Quality { shadows: settings.shadows, volumetric_light: settings.volumetric_light });
    world.set_far_detail(settings.far_detail.range());
    scale.set_if_neq(RenderScale(settings.resolution as f32 / 100.0));
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
    use voxmar_texgen::font::{EM_PIXELS, PixelFont};

    /// The panel reserves room for the hint, so a longer one would spill out of it, in any
    /// of the fonts a player can choose and any language.
    #[test]
    fn every_hint_fits_the_room_kept_for_it() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/fonts");
        let content = dir.join("../../content");
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let fallback = crate::ui::skin::read_fallback(&dir);
        // The music's categories' switches too, as the shipped content lists them.
        let shipped = voxmar_server::load_content(&dir.join("../..")).unwrap();
        let categories = crate::music::rules::MusicRules::load(&shipped.files).unwrap().categories;
        assert!(!categories.is_empty());
        let pages = pages(&music_switches(&MusicCategories(categories)));
        for file in ["ledger.txt", "block.txt", "voxmar.txt"] {
            let mut font = PixelFont::parse_with(&read(file).unwrap(), read).unwrap();
            font.fill_from(&fallback);
            // Glyphs get one font pixel of space after them; body text draws each font pixel
            // `scale` screen pixels wide, inside the panel's padding.
            let cell = font.monospace.unwrap_or(0);
            let advance = |c: char| font.glyphs.get(&c).map_or(EM_PIXELS, |glyph| glyph.width).max(cell) as f32 + 1.0;
            let scale = BODY / EM_PIXELS as f32;
            let room = (PANEL_WIDTH - 28.0 * PX) / scale;
            for language in voxmar_engine::lang::languages(&content) {
                let locale = voxmar_engine::lang::Lang::load(&content, &language.code);
                for (_, controls) in &pages {
                    for &control in controls {
                        let hint = locale.get(&control.hint()).to_owned();
                        // Lines break between words, and anywhere between kanji and kana.
                        let units = hint.split(' ').flat_map(|word| {
                            if word.chars().any(|c| c as u32 >= 0x3000) {
                                word.chars().map(String::from).collect::<Vec<_>>()
                            } else {
                                vec![word.to_owned()]
                            }
                        });
                        let (mut lines, mut width) = (1, 0.0);
                        for unit in units {
                            let unit_width: f32 = unit.chars().map(advance).sum();
                            let spaced = width != 0.0 && unit.chars().all(|c| (c as u32) < 0x3000);
                            let with_space = if spaced { width + advance(' ') + unit_width } else { width + unit_width };
                            if with_space > room {
                                lines += 1;
                                width = unit_width;
                            } else {
                                width = with_space;
                            }
                        }
                        assert!(lines <= HINT_LINES, "{file}, {}: {lines} lines: {hint}", language.code);
                    }
                }
            }
        }
    }

    /// A category's switch turns it off and on again by name, a file from before the
    /// switches reads every category as on, and one switched off stays off when saved.
    #[test]
    fn music_categories_switch_off_and_on_by_name() {
        let switches = music_switches(&MusicCategories(vec![
            crate::music::rules::Category { name: "suno".into(), folder: Some("suno".into()), contexts: Vec::new(), tracks: Vec::new() },
            crate::music::rules::Category { name: "minecraft_world".into(), folder: None, contexts: Vec::new(), tracks: Vec::new() },
        ]));
        assert_eq!(switches.iter().map(|s| (s.key, s.borrowed_only)).collect::<Vec<_>>(), [("music.suno", false), ("music.minecraft_world", true)]);
        let (suno, world) = (Toggle::Music(switches[0]), Toggle::Music(switches[1]));
        let old: Settings = ron::from_str("(fov: 80)").unwrap();
        assert!(old.music_off.is_empty() && suno.get(&old) && world.get(&old));
        let mut settings = Settings::default();
        suno.set(&mut settings, false);
        suno.set(&mut settings, false);
        assert_eq!(settings.music_off, ["suno"]);
        assert!(!suno.get(&settings) && world.get(&settings));
        let saved: Settings = ron::from_str(&ron::to_string(&settings).unwrap()).unwrap();
        assert!(!suno.get(&saved));
        suno.set(&mut settings, true);
        assert!(settings.music_off.is_empty() && suno.get(&settings));
        // The audio page lists them after its own controls.
        let audio = pages(&switches).into_iter().find(|(name, _)| *name == MUSIC_PAGE).unwrap().1;
        assert!(audio[audio.len() - 2..] == [Control::Switch(suno), Control::Switch(world)]);
    }

    #[test]
    fn settings_file_round_trips_and_tolerates_old_or_edited_files() {
        let settings = Settings { fov: 90, vsync: false, fly_speed: 250, ..Settings::default() };
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(ron::from_str::<Settings>(&text).unwrap(), settings);
        // Missing fields take their defaults and out-of-range values are clamped.
        let edited: Settings = ron::from_str("(view_distance: 99, music_volume: -5)").unwrap();
        assert_eq!(edited.clamped(), Settings { view_distance: 32, music_volume: 0, ..Settings::default() });
        // A value an older game wrote that no longer reads keeps its default, and only it.
        let (old, skipped) = Settings::parse("(fov: 80, resolution: Balanced, zoom_key: \"KeyC\")", || false);
        assert_eq!((old.fov, old.resolution, skipped), (80, 50, vec!["resolution".to_owned()]));
        // Zoom on C made way for the character screen.
        assert_eq!(old.zoom_key, Key(KeyCode::KeyZ));
    }

    #[test]
    fn rebinding_onto_a_taken_key_trades_places_and_a_kept_key_is_refused() {
        let mut settings = Settings::default();
        // The journal onto E: the inventory takes J, so both screens still open.
        assert!(Binding::Journal.bind(&mut settings, Key(KeyCode::KeyE)));
        assert_eq!((settings.journal_key, settings.inventory_key), (Key(KeyCode::KeyE), Key(KeyCode::KeyJ)));
        // Onto a movement key or a hotbar number: refused.
        assert!(!Binding::Character.bind(&mut settings, Key(KeyCode::KeyW)));
        assert!(!Binding::Uses.bind(&mut settings, Key(KeyCode::Digit3)));
        assert_eq!((settings.character_key, settings.uses_key), (Key(KeyCode::KeyC), Key(KeyCode::KeyU)));
        // Whatever is bound, no two bindings share a key and none takes a kept one.
        for binding in Binding::ALL {
            let key = binding.get(&settings);
            assert!(!FIXED_KEYS.contains(&key.0));
            assert_eq!(Binding::ALL.iter().filter(|b| b.get(&settings) == key).count(), 1, "{binding:?}");
        }
    }

    #[test]
    fn the_quick_prayer_keys_are_taken_like_any_other_binding() {
        let mut settings = Settings::default();
        // The uses key onto the first quick prayer's X: the two trade places.
        let prayer = Binding::Prayer(0).get(&settings);
        assert!(Binding::Uses.bind(&mut settings, prayer));
        assert_eq!((settings.uses_key, settings.prayer_keys[0]), (prayer, Key(KeyCode::KeyU)));
        // A file putting the journal on a quick prayer's key reads back with the prayer keeping
        // it, and the quests and dodge keys count as taken too.
        let (read, _) = Settings::parse("(journal_key: \"KeyV\", quests_key: \"KeyB\", dodge_key: \"KeyN\")", || false);
        assert_eq!(read.prayer_keys[1..4], [Key(KeyCode::KeyV), Key(KeyCode::KeyB), Key(KeyCode::KeyN)]);
        let keys = Binding::ALL.map(|b| b.get(&read));
        for key in keys {
            assert_eq!(keys.iter().filter(|&&k| k == key).count(), 1, "{key:?} is on two actions: {keys:?}");
        }
    }

    #[test]
    fn colliding_bindings_in_a_file_read_back_at_their_defaults() {
        // A hand-edited file: the journal on E (the inventory's) and the character screen on W.
        let (read, _) = Settings::parse("(journal_key: \"KeyE\", character_key: \"KeyW\")", || false);
        assert_eq!((read.journal_key, read.inventory_key, read.character_key), (Key(KeyCode::KeyJ), Key(KeyCode::KeyE), Key(KeyCode::KeyC)));
        // Two moved bindings on one key: the later yields.
        let (read, _) = Settings::parse("(recipes_key: \"KeyY\", uses_key: \"KeyY\")", || false);
        assert_eq!((read.recipes_key, read.uses_key), (Key(KeyCode::KeyY), Key(KeyCode::KeyU)));
        // Chat on the inventory's E and the journal on chat's T: chat cannot go back to T,
        // so it goes to the first key nobody holds.
        let (read, _) = Settings::parse("(chat_key: \"KeyE\", journal_key: \"KeyT\")", || false);
        assert_eq!((read.inventory_key, read.journal_key), (Key(KeyCode::KeyE), Key(KeyCode::KeyT)));
        let keys = Binding::ALL.map(|b| b.get(&read));
        assert!(!FIXED_KEYS.contains(&read.chat_key.0) && keys.iter().filter(|&&k| k == read.chat_key).count() == 1, "chat on {:?} with {keys:?}", read.chat_key);
    }

    /// A new player starts with Voxmar's own textures and sounds; a player whose settings
    /// file predates the choice keeps their Minecraft install's, decided as the file is
    /// read (nothing writes the file for it).
    #[test]
    fn minecraft_is_opted_into_except_by_files_from_before() {
        // No file: a fresh install.
        let fresh = Settings::default();
        assert_eq!((fresh.minecraft_assets, fresh.textures), (false, TextureOrder::Voxmar));
        assert_eq!(TextureOrder::ALL[0], TextureOrder::Voxmar, "the settings list Voxmar first");
        let found = || true;
        let not_found = || false;
        // A file from before the setting, with Minecraft found: as it was.
        let (old, _) = Settings::parse("(fov: 80, textures: Minecraft)", found);
        assert_eq!((old.minecraft_assets, old.textures, old.fov), (true, TextureOrder::Minecraft, 80));
        let (old, _) = Settings::parse("(textures: VoxmarHd)", found);
        assert_eq!((old.minecraft_assets, old.textures), (true, TextureOrder::VoxmarHd));
        // Older still, from before `textures`: Minecraft first was the default then.
        let (older, _) = Settings::parse("(fov: 80)", found);
        assert_eq!((older.minecraft_assets, older.textures), (true, TextureOrder::Minecraft));
        // No Minecraft to borrow from: Voxmar's.
        let (old, _) = Settings::parse("(textures: Minecraft)", not_found);
        assert_eq!((old.minecraft_assets, old.textures), (false, TextureOrder::Voxmar));
        // Once the file names the setting, it holds, and the install is not even looked for.
        let (off, _) = Settings::parse("(textures: Minecraft, minecraft_assets: false)", || panic!("looked for a jar"));
        assert_eq!((off.minecraft_assets, off.textures), (false, TextureOrder::Voxmar));
        let (on, _) = Settings::parse("(minecraft_assets: true)", not_found);
        assert_eq!((on.minecraft_assets, on.textures), (true, TextureOrder::Voxmar));
        // An unreadable value elsewhere does not lose the decision.
        let (old, skipped) = Settings::parse("(resolution: Balanced, textures: Minecraft)", found);
        assert_eq!((old.minecraft_assets, old.textures, skipped), (true, TextureOrder::Minecraft, vec!["resolution".to_owned()]));
        // Turning it off takes Minecraft out of the texture choices.
        let mut settings = Settings { minecraft_assets: true, textures: TextureOrder::Minecraft, ..Settings::default() };
        Toggle::MinecraftAssets.set(&mut settings, false);
        assert_eq!((settings.minecraft_assets, settings.textures), (false, TextureOrder::Voxmar));
        assert_eq!(TextureOrder::choices(false), [TextureOrder::Voxmar, TextureOrder::VoxmarHd]);
    }
}
