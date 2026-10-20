//! Heads-up display: crosshair, hotbar, health, the held item's name, debug overlay, the
//! inventory screen and the console, all in the interface's pixel look (`crate::ui`).

pub mod celebrate;
pub mod character;
pub mod combat;
pub mod console;
pub mod creative;
pub mod death;
pub mod effects;
pub mod guide;
pub mod home;
pub mod inventory;
pub mod journal;
pub mod levels;
pub mod makeable;
pub mod prayers;
pub mod quest_look;
pub mod quests;
pub mod screens;
pub mod recipes;
pub mod skills;
pub mod target;
pub mod tips;

use std::collections::VecDeque;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory};
use voxmar_engine::effect::{ActiveEffect, EffectDef, EffectId, Kind};
use voxmar_engine::vitals::Vitals;

use crate::lang::Locale;
use crate::interact::Target;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::lod::Lod;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, SOFT, Meter, SHADOW, TEXT, UiFont, set_meter, shadowed, spawn_signed_meter};
use crate::ui::widgets::space;
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((console::ConsolePlugin, inventory::InventoryPlugin, skills::SkillHudPlugin, screens::ScreensPlugin, character::CharacterPlugin, target::TargetPlugin, recipes::RecipesPlugin, death::DeathPlugin, effects::EffectsPlugin, creative::CreativePlugin))
            .add_plugins((tips::TipsPlugin, journal::JournalPlugin, levels::LevelsPlugin, quests::QuestsPlugin, combat::CombatHudPlugin, prayers::PrayersPlugin, home::HomePlugin, celebrate::CelebratePlugin, makeable::MakeablePlugin))
            .add_systems(PostStartup, spawn_hud)
            .add_systems(
                Update,
                (
                    (update_slots.after(inventory::InventorySystems), follow_counts).chain(),
                    update_vitals,
                    count_down,
                    update_status,
                    update_debug,
                    show_held_name,
                    raise.run_if(resource_changed::<Settings>),
                )
                    .run_if(in_state(Screen::InGame)),
            )
            .add_systems(OnEnter(Screen::InGame), show_hud::<true>)
            .add_systems(OnExit(Screen::InGame), show_hud::<false>);
    }
}

/// Images used by the HUD, prepared at startup from the texture source.
#[derive(Resource)]
pub struct HudImages {
    /// Item icon per block id.
    pub icons: Vec<Handle<Image>>,
    pub crosshair: Handle<Image>,
    /// Icon per skill id.
    pub skills: Vec<Handle<Image>>,
    /// Icon per effect id: its own (`ui/effect/<icon>`), or the plain one for a buff or a
    /// debuff.
    pub effects: Vec<Handle<Image>>,
}

impl HudImages {
    /// An effect's icon.
    pub fn effect(&self, effect: EffectId) -> Handle<Image> {
        self.effects.get(effect.0 as usize).cloned().unwrap_or_default()
    }
}

/// How far a stack's count sits in from its slot's corner: inside the frame, with a pixel
/// to spare, at any interface size.
pub const COUNT_INSET: f32 = 2.0 * PX;

/// A slot's side: an item's 16 texels plus a 2-texel rim, in screen pixels.
pub const SLOT: f32 = 20.0 * PX;
/// The gap between slots.
const GAP: f32 = PX;
/// The hotbar frame's border around its slots.
const FRAME: f32 = 3.0 * PX;
/// Gap under the hotbar before the settings raise it, in screen pixels.
const BOTTOM: f32 = 4.0 * PX;

/// The HUD's root, shown only in game.
#[derive(Component)]
struct HudRoot;
/// The icon showing what is in a slot (a hotbar or inventory slot, or `CURSOR`).
#[derive(Component)]
pub struct SlotIcon(pub usize);
#[derive(Component)]
pub struct SlotCount(pub usize);
#[derive(Component)]
struct HotbarSelection;
/// What sits along the bottom edge: the hotbar, health and effects.
#[derive(Component)]
pub(crate) struct Bottom;
/// The health bar's meter (a blow that hurts flashes it, `combat`).
#[derive(Component)]
pub struct HealthMeter;
/// The row of effects over the meters.
#[derive(Component)]
struct EffectChips;

#[derive(Component)]
pub(crate) struct DebugText;
/// The plate the debug overlay sits in.
#[derive(Component)]
pub struct DebugPlate;
#[derive(Component)]
struct BreakBar;
/// The name of the item in hand, shown above the hotbar for a moment after it changes.
#[derive(Component)]
struct HeldName;

fn absolute(left: f32, top: f32, width: f32, height: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(left),
        top: Val::Px(top),
        width: Val::Px(width),
        height: Val::Px(height),
        ..default()
    }
}

/// An item icon centred in a slot, with its stack count in the bottom-right corner.
pub fn slot_contents(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, slot: usize) {
    let inset = (SLOT - 16.0 * PX) / 2.0;
    parent.spawn((absolute(inset, inset, 16.0 * PX, 16.0 * PX), ImageNode::default(), Visibility::Hidden, SlotIcon(slot)));
    let corner = Node { position_type: PositionType::Absolute, right: Val::Px(COUNT_INSET), bottom: Val::Px(COUNT_INSET), ..default() };
    spawn_count(parent, font, corner, SlotCount(slot));
}

/// A stack count at `corner`, ringed by a dark pixel all round so it reads over any icon
/// (pale coins too); `marker` goes on the count itself, whose words the ring follows.
pub fn spawn_count(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, corner: Node, marker: impl Bundle) {
    parent.spawn(corner).with_children(|corner| {
        let count = corner.spawn((crate::ui::widgets::text(font, "", BODY, TEXT), ZIndex(1), marker)).id();
        // One font pixel each way, and the corner below right, under the count.
        let pixel = BODY / crate::ui::skin::font_size(1);
        for (x, y) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (1.0, 1.0)] {
            let at = Node { position_type: PositionType::Absolute, left: Val::Px(x * pixel), top: Val::Px(y * pixel), ..default() };
            corner.spawn((at, crate::ui::widgets::text(font, "", BODY, OUTLINE), Ring(count), crate::ui::fit::Decoration));
        }
    });
}

/// The dark ring round a stack count (and a hitsplat's words, `combat`).
pub(crate) const OUTLINE: Color = Color::srgb(0.0, 0.0, 0.02);

/// One of the copies ringing the count it names.
#[derive(Component)]
pub struct Ring(pub Entity);

/// Each ring says what its count says.
fn follow_counts(counts: Query<&Text, (Changed<Text>, Without<Ring>)>, mut rings: Query<(&Ring, &mut Text)>) {
    for (ring, mut text) in &mut rings {
        if let Ok(count) = counts.get(ring.0)
            && text.0 != count.0
        {
            text.0 = count.0.clone();
        }
    }
}

fn spawn_hud(mut commands: Commands, images: Res<HudImages>, font: Res<UiFont>) {
    let font = &font.0;
    let root = Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() };
    commands.spawn((HudRoot, root, Visibility::Hidden)).with_children(|root| {
        // Crosshair and block-breaking progress.
        root.spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_direction: FlexDirection::Column,
            ..default()
        })
        .with_children(|center| {
            center.spawn((Node { width: Val::Px(15.0 * PX), height: Val::Px(15.0 * PX), ..default() }, ImageNode::new(images.crosshair.clone())));
            let track = Node { width: Val::Px(22.0 * PX), height: Val::Px(4.0 * PX), margin: UiRect::top(Val::Px(space::INNER)), padding: UiRect::all(Val::Px(PX)), ..default() };
            center.spawn((track, Piece::Well.image(), Visibility::Hidden, BreakBar)).with_child((
                Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                Piece::Bar.image(),
            ));
        });

        // Hotbar, selection and hearts, anchored bottom-centre and raised by the settings.
        let bottom = Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(BOTTOM),
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            ..default()
        };
        root.spawn((Bottom, bottom)).with_children(|bottom| {
            let name = Node { height: Val::Px(12.0 * PX), margin: UiRect::bottom(Val::Px(space::HAIR)), ..default() };
            bottom.spawn(name).with_child((HeldName, Visibility::Inherited, shadowed(font, "", BODY, TEXT)));
            let width = HOTBAR_SLOTS as f32 * (SLOT + GAP) - GAP + 2.0 * FRAME;
            // The effects on you, then health across the hotbar's width.
            let chips = Node { width: Val::Px(width), column_gap: Val::Px(space::HAIR), row_gap: Val::Px(space::HAIR), margin: UiRect::bottom(Val::Px(space::INNER)), flex_wrap: FlexWrap::Wrap, ..default() };
            bottom.spawn((chips, EffectChips));
            // The prayer points and the prayers on, while any is.
            prayers::spawn_strip(bottom, font, width);
            // Health, the one meter: food heals rather than fills a stomach.
            let meters = Node { width: Val::Px(width), margin: UiRect::bottom(Val::Px(space::INNER)), ..default() };
            bottom.spawn(meters).with_children(|meters| spawn_signed_meter(meters, font, Piece::Heart, Piece::BarHealth, Val::Percent(100.0), HealthMeter));
            let frame = Node { width: Val::Px(width), height: Val::Px(SLOT + 2.0 * FRAME), padding: UiRect::all(Val::Px(FRAME)), column_gap: Val::Px(GAP), ..default() };
            bottom.spawn((frame, Piece::Frame.image())).with_children(|bar| {
                for slot in 0..HOTBAR_SLOTS {
                    bar.spawn((Node { width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() }, Piece::Well.image()))
                        .with_children(|cell| slot_contents(cell, font, slot));
                }
                let ring = SLOT + 4.0 * PX;
                bar.spawn((absolute(FRAME - 2.0 * PX, FRAME - 2.0 * PX, ring, ring), Piece::Selection.image(), ZIndex(1), HotbarSelection));
            });
        });

        // The debug overlay, in a dark plate so it reads over any sky and keeps clear of the
        // window's edges.
        let plate = Node { position_type: PositionType::Absolute, left: Val::Px(space::GAP), top: Val::Px(space::GAP), padding: UiRect::all(Val::Px(space::GAP)), ..default() };
        root.spawn((plate, Piece::Tooltip.image(), DebugPlate, Visibility::Hidden)).with_child((shadowed(font, "", BODY, TEXT), DebugText));
    });
}

fn show_hud<const SHOWN: bool>(mut root: Query<&mut Visibility, With<HudRoot>>) {
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = if SHOWN { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// How far the hotbar sits above the screen's bottom edge, in pixels.
pub fn bottom_offset(settings: &Settings) -> f32 {
    BOTTOM + settings.hud_raise as f32 * PX
}

/// Lifts the bottom of the HUD off the screen's edge by the setting, in HUD pixels (like
/// the Raised mod), which keeps it clear of rounded corners and the dock.
fn raise(settings: Res<Settings>, mut bottom: Query<&mut Node, With<Bottom>>) {
    let offset = Val::Px(bottom_offset(&settings));
    for mut node in &mut bottom {
        if node.bottom != offset {
            node.bottom = offset;
        }
    }
}

/// Shows what is in every slot: the hotbar's, the inventory screen's and the pointer's,
/// as the inventory screen would have them (with a drag's spread previewed).
fn update_slots(
    shown: Res<inventory::ShownInventory>,
    player: Res<LocalPlayer>,
    images: Res<HudImages>,
    mut icons: Query<(&SlotIcon, &mut ImageNode, &mut Visibility)>,
    mut counts: Query<(&SlotCount, &mut Text)>,
    mut selection: Query<&mut Node, With<HotbarSelection>>,
    mut drawn: Local<Option<(Inventory, usize)>>,
) {
    // Only touch the UI when the inventory or selection actually changed.
    let state = (shown.0.clone(), player.selected);
    if drawn.as_ref() == Some(&state) {
        return;
    }
    *drawn = Some(state);
    for (slot, mut image, mut visibility) in &mut icons {
        match shown.0.get(slot.0) {
            Some(stack) => {
                let icon = &images.icons[stack.item.0 as usize];
                if image.image != *icon {
                    image.image = icon.clone();
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (slot, mut text) in &mut counts {
        let count = shown.0.get(slot.0).map_or(0, |s| s.count);
        let value = if count > 1 { count.to_string() } else { String::new() };
        if text.0 != value {
            text.0 = value;
        }
    }
    if let Ok(mut node) = selection.single_mut() {
        node.left = Val::Px(FRAME - 2.0 * PX + player.selected as f32 * (SLOT + GAP));
    }
}

/// Health as a meter reading "15/20", and a chip for each effect on you: teal
/// for a buff, red for a debuff.
#[allow(clippy::too_many_arguments)]
fn update_vitals(
    mut commands: Commands,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    health: Query<&Meter, With<HealthMeter>>,
    chips: Query<Entity, With<EffectChips>>,
    mut nodes: Query<&mut Node>,
    children: Query<&Children>,
    mut texts: Query<&mut Text>,
    mut shown: Local<Option<(u8, u8, Vitals)>>,
    (locale, launch, images): (Res<Locale>, Res<crate::session::Launch>, Res<HudImages>),
) {
    let state = (player.health, player.max_health, player.vitals.clone());
    if shown.as_ref() == Some(&state) && !locale.is_changed() {
        return;
    }
    // The chips are drawn again when the effects come or go, not as their time runs down.
    let kept = |v: &Vitals| v.effects.iter().map(|a| (a.effect, a.seconds_left.is_some())).collect::<Vec<_>>();
    let effects_changed = locale.is_changed() || shown.as_ref().is_none_or(|(_, _, v)| kept(v) != kept(&state.2));
    *shown = Some(state);
    if let Ok(meter) = health.single() {
        let most = player.max_health.max(1);
        set_meter(meter, player.health as f32 / most as f32, &format!("{}/{most}", player.health), &mut nodes, &children, &mut texts);
    }
    if effects_changed && let Ok(row) = chips.single() {
        commands.entity(row).despawn_children().with_children(|row| {
            for active in &player.vitals.effects {
                spawn_effect_chip(row, &font.0, &locale, &images, active, launch.content.effects.get(active.effect));
            }
        });
    }
}

/// A small label naming an effect, teal for a buff and red for a debuff.
pub fn spawn_effect_chip(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, _locale: &Locale, images: &HudImages, active: &ActiveEffect, effect: &EffectDef) {
    // Just the icon (and the time left on a timed one), underlined teal for a buff and red
    // for a debuff; its name and what it does are on hover.
    let chip = Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: Val::Px(PX), padding: UiRect::all(Val::Px(space::HAIR)), ..default() };
    let color = if effect.kind == Kind::Buff { ACCENT } else { ERROR };
    let id = active.effect;
    parent.spawn((chip, Piece::Tooltip.image(), Button, effects::EffectHover(id), bevy::ui::RelativeCursorPosition::default(), crate::ui::widgets::HoverLight)).with_children(|chip| {
        chip.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::HAIR), ..default() }).with_children(|line| {
            line.spawn(effect_icon(images, id));
            if active.seconds_left.is_some() {
                line.spawn((shadowed(font, "", BODY, SOFT), crate::lang::TightLine, EffectTimer(id)));
            }
        });
        chip.spawn((Node { width: Val::Percent(100.0), height: Val::Px(PX), ..default() }, BackgroundColor(color)));
    });
}

/// An effect's icon, its 10 pixels of art drawn at the interface's pixel size.
pub fn effect_icon(images: &HudImages, id: EffectId) -> (Node, ImageNode) {
    (Node { width: Val::Px(10.0 * PX), height: Val::Px(10.0 * PX), flex_shrink: 0.0, ..default() }, ImageNode::new(images.effect(id)))
}

/// The time left on a timed effect, counting down between the server's word on it.
#[derive(Component)]
pub struct EffectTimer(pub EffectId);

/// Seconds left on `effect` now, if it is timed and on the player.
pub fn seconds_left(player: &LocalPlayer, effect: EffectId, now: f32) -> Option<u32> {
    let active = player.vitals.effects.iter().find(|a| a.effect == effect)?;
    let told = active.seconds_left? as f32;
    Some((told - (now - player.vitals_at)).max(0.0).ceil() as u32)
}

/// A time as a clock reads it: 4:05, or 1:02:03 past the hour.
pub fn clock(seconds: u32) -> String {
    let (hours, minutes, secs) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{secs:02}") } else { format!("{minutes}:{secs:02}") }
}

/// Counts every timed effect's clock down.
fn count_down(time: Res<Time>, player: Res<LocalPlayer>, mut timers: Query<(&EffectTimer, &mut Text)>) {
    let now = time.elapsed_secs();
    for (timer, mut text) in &mut timers {
        let words = seconds_left(&player, timer.0, now).map_or_else(String::new, clock);
        if text.0 != words {
            text.0 = words;
        }
    }
}

fn update_status(
    target: Res<Target>,
    mut bar: Query<(&mut Visibility, &Children), With<BreakBar>>,
    mut fill: Query<&mut Node>,
) {
    if let Ok((mut visibility, children)) = bar.single_mut() {
        let progress = target.breaking.map(|(_, p)| p);
        visibility.set_if_neq(if progress.is_some() { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(mut node) = children.first().and_then(|&c| fill.get_mut(c).ok()) {
            let width = Val::Percent(progress.unwrap_or(0.0) * 100.0);
            if node.width != width {
                node.width = width;
            }
        }
    }
}

/// Seconds the held item's name stays above the hotbar, the last of them fading.
const NAME_SECONDS: f32 = 2.0;
const NAME_FADE: f32 = 0.5;

/// Names the item in hand above the hotbar for a moment whenever it changes, coloured by
/// its rarity.
fn show_held_name(
    time: Res<Time>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    locale: Res<Locale>,
    mut name: Query<(&mut Text, &mut TextColor, &mut TextShadow, &mut Visibility), With<HeldName>>,
    mut last: Local<(Option<voxmar_engine::block::BlockId>, f32)>,
) {
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let now = time.elapsed_secs();
    let Ok((mut text, mut color, mut shadow, mut visibility)) = name.single_mut() else { return };
    let changed = held != last.0;
    if changed {
        *last = (held, now);
    }
    if changed || locale.is_changed() {
        text.0 = held.map(|item| locale.block(world.registry.def(item))).unwrap_or_default();
    }
    let alpha = ((last.1 + NAME_SECONDS - now) / NAME_FADE).clamp(0.0, 1.0);
    let base = held.map_or(TEXT, |item| inventory::rarity_color(world.registry.def(item).rarity));
    color.set_if_neq(TextColor(base.with_alpha(alpha)));
    shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
    // Fully faded words no longer participate in layout checks or drawing. Inherited
    // visibility still follows the HUD's root when a new held item restores the label.
    visibility.set_if_neq(if alpha == 0.0 { Visibility::Hidden } else { Visibility::Inherited });
}

#[allow(clippy::too_many_arguments)]
/// How far back the debug overlay looks for its worst frames, in seconds.
const RECENT_SECONDS: f32 = 5.0;
/// A frame over this many ms is counted as slow on the debug overlay (as in slow_frames.rs).
const SLOW_FRAME_MS: f32 = 25.0;
/// How often the debug overlay's text is rewritten, in seconds. New text means laying out
/// the UI again; rewritten every frame, that was 10 to 25 ms of Mariano's slow frames.
pub(crate) const DEBUG_REFRESH: f32 = 0.25;

/// The wall-clock length of every frame in the last `RECENT_SECONDS`, kept while the overlay
/// is hidden too, so its worst frames are there the moment it opens.
#[derive(Default)]
struct RecentFrames(VecDeque<(f32, f32)>);

impl RecentFrames {
    fn push(&mut self, now: f32, ms: f32) {
        self.0.push_back((now, ms));
        while self.0.front().is_some_and(|&(at, _)| at < now - RECENT_SECONDS) {
            self.0.pop_front();
        }
    }

    /// The worst frame, the 99th percentile and how many were over `SLOW_FRAME_MS`, in ms.
    fn summary(&self) -> (f32, f32, usize) {
        let mut times: Vec<f32> = self.0.iter().map(|&(_, ms)| ms).collect();
        if times.is_empty() {
            return (0.0, 0.0, 0);
        }
        times.sort_by(f32::total_cmp);
        let p99 = times[(times.len() * 99 / 100).min(times.len() - 1)];
        let slow = times.iter().filter(|&&ms| ms > SLOW_FRAME_MS).count();
        (times[times.len() - 1], p99, slow)
    }
}

fn update_debug(
    time: Res<Time<Real>>,
    mut recent: Local<RecentFrames>,
    mut next_refresh: Local<f32>,
    diagnostics: Res<DiagnosticsStore>,
    world: Res<ClientWorld>,
    player: Res<LocalPlayer>,
    settings: Res<Settings>,
    target: Res<Target>,
    lod: Res<Lod>,
    weather: Res<ClientWeather>,
    locale: Res<Locale>,
    mut text: Query<&mut Text, With<DebugText>>,
    mut plate: Query<&mut Visibility, With<DebugPlate>>,
) {
    let now = time.elapsed_secs();
    recent.push(now, time.delta_secs() * 1000.0);
    let (Ok(mut text), Ok(mut visibility)) = (text.single_mut(), plate.single_mut()) else { return };
    let opened = visibility.set_if_neq(if settings.debug_overlay { Visibility::Inherited } else { Visibility::Hidden }) && settings.debug_overlay;
    if !settings.debug_overlay || (!opened && now < *next_refresh) {
        return;
    }
    *next_refresh = now + DEBUG_REFRESH;
    let fps = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let frame_ms = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let (worst, p99, slow) = recent.summary();
    let stats = world.stats;
    let p = player.position;
    let looking = target.hit.and_then(|hit| world.block(hit.block).map(|b| (world.registry.def(b).name.clone(), hit.block))).map_or_else(
        || locale.get("hud.debug.looking_at_nothing").to_owned(),
        |(block, at)| locale.format("hud.debug.looking", &[("block", &block), ("position", &at)]),
    );
    use voxmar_engine::lang::Words;
    let state = Words::key(&format!("weather.state.{}", weather.current.describe().replace(' ', "_")));
    let report = Words::key("command.weather.report").with("state", state).with("numbers", voxmar_server::weather_numbers(&weather.current));
    let mut sky = locale.render(&report, &world.registry);
    if !weather.effects.is_empty() {
        let effects = Words::List(weather.effects.iter().map(|name| voxmar_server::weather_name("effect", name)).collect());
        sky.push('\n');
        sky.push_str(&locale.render(&Words::key("hud.debug.effects").with("effects", effects), &world.registry));
    }
    let (cells, raining) = weather.nearby_rain().fold((0, 0), |(cells, raining), (_, rain)| (cells + 1, raining + usize::from(rain > 0.1)));
    if raining > 0 {
        sky.push_str(locale.get("hud.debug.separator"));
        sky.push_str(&locale.format("hud.debug.rain", &[("count", &raining), ("cells", &cells)]));
    }
    let separator = locale.get("hud.debug.separator");
    let frames = locale.format(
        "hud.debug.frames",
        &[
            ("fps", &format!("{fps:.0}")),
            ("ms", &format!("{frame_ms:.2}")),
            ("seconds", &format!("{RECENT_SECONDS:.0}")),
            ("worst", &format!("{worst:.1}")),
            ("p99", &format!("{p99:.1}")),
            ("slow", &slow),
            ("limit", &format!("{SLOW_FRAME_MS:.0}")),
        ],
    );
    let (x, y, z) = (format!("{:.1}", p.x), format!("{:.1}", p.y), format!("{:.1}", p.z));
    let position = locale.format(if player.flying { "hud.debug.position_flying" } else { "hud.debug.position" }, &[("x", &x), ("y", &y), ("z", &z)]);
    let chunks = [
        locale.format("hud.debug.columns", &[("count", &world.voxels.len())]),
        locale.format("hud.debug.meshes", &[("count", &stats.chunk_entities)]),
        locale.format("hud.debug.quads", &[("count", &stats.quads)]),
        locale.format("hud.debug.visible", &[("count", &stats.visible_chunks)]),
    ]
    .join(separator);
    let jobs = [
        locale.format("hud.debug.jobs", &[("count", &stats.jobs_in_flight)]),
        locale.format("hud.debug.waiting", &[("count", &stats.pending_columns)]),
        locale.format("hud.debug.distant", &[("done", &lod.progress().0), ("all", &lod.progress().1)]),
    ]
    .join(separator);
    let keys = locale.format(
        "hud.debug.keys",
        &[("inventory", &settings.inventory_key.name(&locale)), ("chat", &settings.chat_key.name(&locale)), ("escape", &crate::keys::escape_name(&locale))],
    );
    let new = [frames, position, chunks, jobs, looking, sky, keys].join("\n");
    // Only new text: an unchanged one assigned would still count as a change to lay out.
    if text.0 != new {
        text.0 = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faded_held_names_hide_and_new_items_restore_inherited_visibility() {
        use bevy::camera::visibility::VisibilityPlugin;
        use voxmar_engine::inventory::ItemStack;
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let planks = content.blocks.id("base:oak_planks").unwrap();
        let torch = content.blocks.id("base:torch").unwrap();
        let registry = content.blocks.clone();
        let mut player = LocalPlayer::default();
        player.inventory.slots[0] = Some(ItemStack { item: planks, count: 1 });
        let mut app = App::new();
        app.add_plugins(bevy::app::TaskPoolPlugin::default())
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>()
            .add_plugins(VisibilityPlugin)
            .insert_resource(Time::<()>::default())
            .insert_resource(player)
            .insert_resource(ClientWorld::new(registry.clone(), 1, crate::render::terrain::Terrain::detached()))
            .insert_resource(Locale::load(&assets.join("content"), "en"))
            .add_systems(Update, show_held_name);
        let parent = app.world_mut().spawn(Visibility::Inherited).id();
        let label = app.world_mut().spawn((ChildOf(parent), HeldName, Text::new(""), TextColor(TEXT), TextShadow::default(), Visibility::Inherited)).id();
        let visible = |app: &App| app.world().get::<InheritedVisibility>(label).unwrap().get();
        app.update();
        assert!(visible(&app));
        assert_eq!(app.world().get::<Text>(label).unwrap().0, app.world().resource::<Locale>().block(registry.def(planks)));

        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(1750));
        app.update();
        assert!(visible(&app), "partly faded words still draw");
        assert_eq!(app.world().get::<TextColor>(label).unwrap().0.alpha(), 0.5);
        app.insert_resource(Locale::load(&assets.join("content"), "ja"));
        app.update();
        let japanese = app.world().resource::<Locale>().block(registry.def(planks));
        assert_eq!(app.world().get::<Text>(label).unwrap().0, japanese, "active words follow the locale");
        assert_eq!(app.world().get::<TextColor>(label).unwrap().0.alpha(), 0.5, "locale refresh must not restart the fade");
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(250));
        app.update();
        assert!(!visible(&app), "transparent text must also be hidden from layout checks");
        assert_eq!(*app.world().get::<Visibility>(label).unwrap(), Visibility::Hidden);
        assert_eq!(app.world().get::<TextShadow>(label).unwrap().color.alpha(), 0.0);

        app.insert_resource(Locale::load(&assets.join("content"), "en"));
        app.update();
        assert!(!visible(&app), "a language change cannot revive an expired label");
        app.world_mut().entity_mut(parent).insert(Visibility::Hidden);
        app.world_mut().resource_mut::<LocalPlayer>().inventory.slots[0] = Some(ItemStack { item: torch, count: 1 });
        app.update();
        assert_eq!(*app.world().get::<Visibility>(label).unwrap(), Visibility::Inherited);
        assert_eq!(app.world().get::<TextColor>(label).unwrap().0.alpha(), 1.0);
        assert_eq!(app.world().get::<Text>(label).unwrap().0, app.world().resource::<Locale>().block(registry.def(torch)));
        assert!(!visible(&app), "restored words must respect a hidden HUD parent");
        app.world_mut().entity_mut(parent).insert(Visibility::Inherited);
        app.update();
        assert!(visible(&app), "a new held item becomes visible with its HUD");
    }
}
