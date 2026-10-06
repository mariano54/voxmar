//! Heads-up display: crosshair, hotbar, health, the held item's name, debug overlay, the
//! inventory screen and the console, all in the interface's pixel look (`crate::ui`).

pub mod character;
pub mod console;
pub mod death;
pub mod effects;
pub mod inventory;
pub mod screens;
pub mod recipes;
pub mod skills;
pub mod target;

use std::collections::VecDeque;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory};
use voxmar_engine::effect::{ActiveEffect, EffectDef, EffectId, Kind};
use voxmar_engine::vitals::{MAX_HUNGER, Vitals};

use crate::lang::Locale;
use crate::interact::Target;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::lod::Lod;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, SOFT, Meter, SHADOW, TEXT, UiFont, set_meter, shadowed, spawn_meter};
use crate::ui::widgets::space;
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((console::ConsolePlugin, inventory::InventoryPlugin, skills::SkillHudPlugin, screens::ScreensPlugin, character::CharacterPlugin, target::TargetPlugin, recipes::RecipesPlugin, death::DeathPlugin, effects::EffectsPlugin))
            .add_systems(PostStartup, spawn_hud)
            .add_systems(
                Update,
                (
                    update_slots.after(inventory::InventorySystems),
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
/// What sits along the bottom edge: the hotbar, health, hunger and effects.
#[derive(Component)]
struct Bottom;
#[derive(Component)]
struct HealthMeter;
#[derive(Component)]
struct HungerMeter;
/// The row of effects over the meters.
#[derive(Component)]
struct EffectChips;

#[derive(Component)]
struct DebugText;
/// The plate the debug overlay sits in.
#[derive(Component)]
struct DebugPlate;
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
    parent.spawn((
        Node { position_type: PositionType::Absolute, right: Val::Px(COUNT_INSET), bottom: Val::Px(COUNT_INSET), ..default() },
        shadowed(font, "", BODY, TEXT),
        SlotCount(slot),
    ));
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
            bottom.spawn(name).with_child((HeldName, shadowed(font, "", BODY, TEXT)));
            let width = HOTBAR_SLOTS as f32 * (SLOT + GAP) - GAP + 2.0 * FRAME;
            // The effects on you, then health and hunger side by side.
            let chips = Node { width: Val::Px(width), column_gap: Val::Px(space::HAIR), margin: UiRect::bottom(Val::Px(space::HAIR)), flex_wrap: FlexWrap::Wrap, ..default() };
            bottom.spawn((chips, EffectChips));
            let meters = Node { width: Val::Px(width), column_gap: Val::Px(space::HAIR), margin: UiRect::bottom(Val::Px(space::HAIR)), ..default() };
            bottom.spawn(meters).with_children(|meters| {
                spawn_meter(meters, font, Piece::BarHealth, Val::Percent(50.0), HealthMeter);
                spawn_meter(meters, font, Piece::BarHunger, Val::Percent(50.0), HungerMeter);
            });
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

/// Health and hunger as meters reading "15/20", and a chip for each effect on you: teal
/// for a buff, red for a debuff.
#[allow(clippy::too_many_arguments)]
fn update_vitals(
    mut commands: Commands,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    health: Query<&Meter, With<HealthMeter>>,
    hunger: Query<&Meter, With<HungerMeter>>,
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
    if let Ok(meter) = hunger.single() {
        let value = player.vitals.hunger;
        set_meter(meter, value as f32 / MAX_HUNGER as f32, &format!("{value}/{MAX_HUNGER}"), &mut nodes, &children, &mut texts);
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
pub fn spawn_effect_chip(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, locale: &Locale, images: &HudImages, active: &ActiveEffect, effect: &EffectDef) {
    let chip = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::HAIR), padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)), ..default() };
    let color = if effect.kind == Kind::Buff { ACCENT } else { ERROR };
    let id = active.effect;
    parent.spawn((chip, Piece::Tooltip.image(), Button, effects::EffectHover(id), bevy::ui::RelativeCursorPosition::default(), crate::ui::widgets::HoverLight)).with_children(|chip| {
        chip.spawn(effect_icon(images, id));
        chip.spawn((shadowed(font, locale.effect(effect), BODY, color), crate::lang::TightLine));
        if active.seconds_left.is_some() {
            let gap = Node { margin: UiRect::left(Val::Px(space::INNER)), ..default() };
            chip.spawn((shadowed(font, "", BODY, SOFT), gap, crate::lang::TightLine, EffectTimer(id)));
        }
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
    mut name: Query<(&mut Text, &mut TextColor, &mut TextShadow), With<HeldName>>,
    mut last: Local<(Option<voxmar_engine::block::BlockId>, f32)>,
) {
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let now = time.elapsed_secs();
    let Ok((mut text, mut color, mut shadow)) = name.single_mut() else { return };
    if held != last.0 {
        *last = (held, now);
        text.0 = held.map(|item| locale.block(world.registry.def(item))).unwrap_or_default();
    }
    let alpha = ((last.1 + NAME_SECONDS - now) / NAME_FADE).clamp(0.0, 1.0);
    let base = held.map_or(TEXT, |item| inventory::rarity_color(world.registry.def(item).rarity));
    color.set_if_neq(TextColor(base.with_alpha(alpha)));
    shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
}

#[allow(clippy::too_many_arguments)]
/// How far back the debug overlay looks for its worst frames, in seconds.
const RECENT_SECONDS: f32 = 5.0;
/// A frame over this many ms is counted as slow on the debug overlay (as in slow_frames.rs).
const SLOW_FRAME_MS: f32 = 25.0;
/// How often the debug overlay's text is rewritten, in seconds. New text means laying out
/// the UI again; rewritten every frame, that was 10 to 25 ms of Mariano's slow frames.
const DEBUG_REFRESH: f32 = 0.25;

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
    let looking = target
        .hit
        .and_then(|hit| world.block(hit.block).map(|b| format!("{} at {}", world.registry.def(b).name, hit.block)))
        .unwrap_or_else(|| "nothing".into());
    let mut sky = format!("{}: {}", weather.current.describe(), weather.current.summary());
    if !weather.effects.is_empty() {
        sky.push_str(&format!("\neffects: {}", weather.effects.join(", ").replace('_', " ")));
    }
    let raining = weather.nearby_rain().filter(|&(_, rain)| rain > 0.1).count();
    if raining > 0 {
        sky.push_str(&format!(" | rain in {raining} of the 25 cells around"));
    }
    let new = format!(
        "{fps:.0} fps ({frame_ms:.2} ms) | last {RECENT_SECONDS:.0} s: worst {worst:.1} ms, p99 {p99:.1} ms, {slow} over {SLOW_FRAME_MS:.0} ms\n\
         xyz {:.1} {:.1} {:.1}{}\n\
         columns {} | chunk meshes {} | quads {} | visible chunks {}\n\
         mesh jobs {} | waiting {} | distant tiles {}/{}\n\
         looking at {looking}\n\
         {sky}\n\
         F3 debug  F fly  E inventory  T chat  / command  M music  Esc menu",
        p.x,
        p.y,
        p.z,
        if player.flying { " (flying)" } else { "" },
        world.voxels.len(),
        stats.chunk_entities,
        stats.quads,
        stats.visible_chunks,
        stats.jobs_in_flight,
        stats.pending_columns,
        lod.progress().0,
        lod.progress().1,
    );
    // Only new text: an unchanged one assigned would still count as a change to lay out.
    if text.0 != new {
        text.0 = new;
    }
}
