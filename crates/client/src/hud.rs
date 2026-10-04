//! Heads-up display: crosshair, hotbar, health, inventory screen and debug overlay.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory, SLOTS};
use voxmar_engine::action::ActionTarget;

use crate::Actions;
use crate::interact::Target;
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::render::lod::Lod;
use crate::settings::Settings;
use crate::weather::{self, ClientWeather};
use crate::world::ClientWorld;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn_hud)
            .add_systems(
                Update,
                (
                    toggle_screens,
                    update_slots,
                    update_hearts,
                    update_status,
                    update_debug,
                    click_inventory,
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
    pub hotbar: Handle<Image>,
    pub selection: Handle<Image>,
    pub crosshair: Handle<Image>,
    pub heart_full: Handle<Image>,
    pub heart_half: Handle<Image>,
    pub heart_container: Handle<Image>,
}

const SCALE: f32 = 2.0;
const SLOT: f32 = 20.0 * SCALE;
/// Gap under the hotbar before the settings raise it, in screen pixels.
const BOTTOM: f32 = 8.0;

/// The HUD's root, shown only in game.
#[derive(Component)]
struct HudRoot;
#[derive(Component)]
struct SlotIcon(usize);
#[derive(Component)]
struct SlotCount(usize);
#[derive(Component)]
struct HotbarSelection;
/// What sits along the bottom edge: the hotbar and hearts.
#[derive(Component)]
struct Bottom;
#[derive(Component)]
struct Heart(usize);
#[derive(Component)]
struct StatusText;
#[derive(Component)]
struct DebugText;
#[derive(Component)]
struct BreakBar;
#[derive(Component)]
struct InventoryPanel;
#[derive(Component)]
struct InventorySlot(usize);

/// Slot picked up in the inventory screen, waiting for a destination.
#[derive(Resource, Default)]
struct HeldSlot(Option<usize>);

fn text(value: impl Into<String>, size: f32) -> (Text, TextFont, TextColor, TextShadow) {
    (Text::new(value), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(Color::WHITE), TextShadow::default())
}

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

/// An item icon with a stack count, for hotbar and inventory slots.
fn slot_contents(parent: &mut ChildSpawnerCommands, slot: usize) {
    parent.spawn((absolute(4.0 * SCALE, 4.0 * SCALE, 16.0 * SCALE, 16.0 * SCALE), ImageNode::default(), SlotIcon(slot)));
    parent.spawn((
        Node { position_type: PositionType::Absolute, right: Val::Px(2.0), bottom: Val::Px(0.0), ..default() },
        text("", 14.0),
        SlotCount(slot),
    ));
}

fn spawn_hud(mut commands: Commands, images: Res<HudImages>) {
    commands.insert_resource(HeldSlot::default());
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
            center.spawn((Node { width: Val::Px(15.0 * SCALE), height: Val::Px(15.0 * SCALE), ..default() }, ImageNode::new(images.crosshair.clone())));
            center.spawn((
                Node { width: Val::Px(0.0), height: Val::Px(3.0), margin: UiRect::top(Val::Px(10.0)), ..default() },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
                BreakBar,
            ));
        });

        // Hotbar, selection frame and hearts, anchored bottom-centre and raised by the
        // settings.
        let bottom = Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(BOTTOM),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        };
        root.spawn((Bottom, bottom))
        .with_children(|bottom| {
            bottom
                .spawn((
                    Node { width: Val::Px(182.0 * SCALE), height: Val::Px(22.0 * SCALE), ..default() },
                    ImageNode::new(images.hotbar.clone()),
                ))
                .with_children(|bar| {
                    for slot in 0..HOTBAR_SLOTS {
                        bar.spawn(absolute(slot as f32 * SLOT - SCALE, -SCALE, SLOT + 2.0 * SCALE, SLOT + 2.0 * SCALE))
                            .with_children(|cell| slot_contents(cell, slot));
                    }
                    bar.spawn((absolute(-SCALE, -SCALE, 24.0 * SCALE, 23.0 * SCALE), ImageNode::new(images.selection.clone()), HotbarSelection));
                    for i in 0..10 {
                        let left = i as f32 * 8.0 * SCALE;
                        let top = -10.0 * SCALE;
                        bar.spawn((absolute(left, top, 9.0 * SCALE, 9.0 * SCALE), ImageNode::new(images.heart_container.clone())));
                        bar.spawn((absolute(left, top, 9.0 * SCALE, 9.0 * SCALE), ImageNode::new(images.heart_full.clone()), Heart(i)));
                    }
                });
        });

        root.spawn((Node { position_type: PositionType::Absolute, left: Val::Px(8.0), top: Val::Px(8.0), ..default() }, text("", 14.0), DebugText));
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(35.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            text("", 28.0),
            StatusText,
        ));

        // Inventory screen: 3 storage rows above the hotbar row.
        root.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            Visibility::Hidden,
            InventoryPanel,
        ))
        .with_children(|panel| {
            panel
                .spawn((
                    Node {
                        display: Display::Grid,
                        grid_template_columns: RepeatedGridTrack::px(HOTBAR_SLOTS as u16, SLOT + 2.0 * SCALE),
                        row_gap: Val::Px(2.0),
                        column_gap: Val::Px(2.0),
                        padding: UiRect::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.78, 0.78, 0.78)),
                ))
                .with_children(|grid| {
                    let order = (HOTBAR_SLOTS..SLOTS).chain(0..HOTBAR_SLOTS);
                    for slot in order {
                        grid.spawn((
                            Button,
                            Node { width: Val::Px(SLOT + 2.0 * SCALE), height: Val::Px(SLOT + 2.0 * SCALE), ..default() },
                            BackgroundColor(Color::srgb(0.55, 0.55, 0.55)),
                            InventorySlot(slot),
                        ))
                        .with_children(|cell| slot_contents(cell, slot));
                    }
                });
        });
    });
}

fn show_hud<const SHOWN: bool>(mut root: Query<&mut Visibility, With<HudRoot>>, mut held: ResMut<HeldSlot>) {
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = if SHOWN { Visibility::Inherited } else { Visibility::Hidden };
    }
    held.0 = None;
}

fn toggle_screens(
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    player: Res<LocalPlayer>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut panel: Query<&mut Visibility, With<InventoryPanel>>,
    mut held: ResMut<HeldSlot>,
) {
    if keys.just_pressed(KeyCode::KeyE) && !player.dead && !ui.menu_open {
        ui.inventory_open = !ui.inventory_open;
        held.0 = None;
    }
    if player.dead && keys.just_pressed(KeyCode::KeyR) {
        net.propose(actions.0.expect("core:respawn"), ActionTarget::None);
    }
    if let Ok(mut visibility) = panel.single_mut() {
        visibility.set_if_neq(if ui.inventory_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// Lifts the bottom of the HUD off the screen's edge by the setting, in HUD pixels (like
/// the Raised mod), which keeps it clear of rounded corners and the dock.
fn raise(settings: Res<Settings>, mut bottom: Query<&mut Node, With<Bottom>>) {
    let offset = Val::Px(BOTTOM + settings.hud_raise as f32 * SCALE);
    for mut node in &mut bottom {
        if node.bottom != offset {
            node.bottom = offset;
        }
    }
}

fn update_slots(
    player: Res<LocalPlayer>,
    images: Res<HudImages>,
    mut icons: Query<(&SlotIcon, &mut ImageNode, &mut Visibility)>,
    mut counts: Query<(&SlotCount, &mut Text)>,
    mut selection: Query<&mut Node, With<HotbarSelection>>,
    mut shown: Local<Option<(Inventory, usize)>>,
) {
    // The player resource changes every frame (movement); only touch the UI when the
    // inventory or selection actually changed.
    let state = (player.inventory.clone(), player.selected);
    if shown.as_ref() == Some(&state) {
        return;
    }
    *shown = Some(state);
    for (slot, mut image, mut visibility) in &mut icons {
        match player.inventory.get(slot.0) {
            Some(stack) => {
                image.image = images.icons[stack.item.0 as usize].clone();
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (slot, mut text) in &mut counts {
        let count = player.inventory.get(slot.0).map_or(0, |s| s.count);
        let value = if count > 1 { count.to_string() } else { String::new() };
        if text.0 != value {
            text.0 = value;
        }
    }
    if let Ok(mut node) = selection.single_mut() {
        node.left = Val::Px(player.selected as f32 * SLOT - SCALE);
    }
}

fn update_hearts(
    player: Res<LocalPlayer>,
    images: Res<HudImages>,
    mut hearts: Query<(&Heart, &mut ImageNode, &mut Visibility)>,
    mut shown: Local<Option<u8>>,
) {
    if *shown == Some(player.health) {
        return;
    }
    *shown = Some(player.health);
    for (heart, mut image, mut visibility) in &mut hearts {
        let points = player.health as usize;
        let shown = match points.saturating_sub(heart.0 * 2) {
            0 => None,
            1 => Some(&images.heart_half),
            _ => Some(&images.heart_full),
        };
        match shown {
            Some(handle) => {
                if image.image != *handle {
                    image.image = handle.clone();
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

fn update_status(player: Res<LocalPlayer>, target: Res<Target>, mut status: Query<&mut Text, With<StatusText>>, mut bar: Query<&mut Node, With<BreakBar>>) {
    let message = if player.dead { "You died. Press R to respawn." } else { "" };
    if let Ok(mut text) = status.single_mut()
        && text.0 != message
    {
        message.clone_into(&mut text.0);
    }
    if let Ok(mut node) = bar.single_mut() {
        node.width = Val::Px(target.breaking.map_or(0.0, |(_, p)| p * 40.0));
    }
}

#[allow(clippy::too_many_arguments)]
fn update_debug(
    diagnostics: Res<DiagnosticsStore>,
    world: Res<ClientWorld>,
    player: Res<LocalPlayer>,
    settings: Res<Settings>,
    target: Res<Target>,
    lod: Res<Lod>,
    weather: Res<ClientWeather>,
    mut text: Query<(&mut Text, &mut Visibility), With<DebugText>>,
) {
    let Ok((mut text, mut visibility)) = text.single_mut() else { return };
    visibility.set_if_neq(if settings.debug_overlay { Visibility::Inherited } else { Visibility::Hidden });
    if !settings.debug_overlay {
        return;
    }
    let fps = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let frame_ms = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let stats = world.stats;
    let p = player.position;
    let looking = target
        .hit
        .and_then(|hit| world.block(hit.block).map(|b| format!("{} at {}", world.registry.def(b).name, hit.block)))
        .unwrap_or_else(|| "nothing".into());
    let w = &weather.current;
    let sky = format!(
        "{}: cloud {:.0}% ({:.0}% heaped), cirrus {:.0}%, humidity {:.0}%, {} {:.0}%, wind {:.1} blocks/s",
        weather::describe(w),
        w.cover * 100.0,
        w.convection * 100.0,
        w.cirrus * 100.0,
        w.humidity * 100.0,
        if w.snow > 0.5 { "snow" } else { "rain" },
        w.precipitation * 100.0,
        w.wind.length(),
    );
    text.0 = format!(
        "{fps:.0} fps ({frame_ms:.2} ms)\n\
         xyz {:.1} {:.1} {:.1}{}\n\
         columns {} | chunk meshes {} | quads {} | visible chunks {}\n\
         mesh jobs {} | waiting {} | distant tiles {}/{}\n\
         looking at {looking}\n\
         {sky}\n\
         F3 debug  F fly  E inventory  M music  Esc menu",
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
}

fn click_inventory(
    ui: Res<UiState>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut held: ResMut<HeldSlot>,
    mut player: ResMut<LocalPlayer>,
    mut slots: Query<(&InventorySlot, &Interaction, &mut BackgroundColor), Changed<Interaction>>,
) {
    if !ui.inventory_open {
        return;
    }
    for (slot, interaction, mut background) in &mut slots {
        match interaction {
            Interaction::Pressed => match held.0.take() {
                Some(from) if from != slot.0 => {
                    let target = ActionTarget::Slots { from: from as u8, to: slot.0 as u8 };
                    net.propose(actions.0.expect("core:transfer"), target);
                    player.inventory.move_stack(from, slot.0);
                }
                Some(_) => {}
                None if player.inventory.get(slot.0).is_some() => held.0 = Some(slot.0),
                None => {}
            },
            Interaction::Hovered => background.0 = Color::srgb(0.7, 0.7, 0.7),
            Interaction::None => background.0 = Color::srgb(0.55, 0.55, 0.55),
        }
        if held.0 == Some(slot.0) {
            background.0 = Color::srgb(0.9, 0.8, 0.35);
        }
    }
}
