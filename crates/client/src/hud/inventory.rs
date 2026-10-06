//! The inventory screen, with Minecraft's mouse and keyboard handling:
//!
//! - click picks a stack up, puts it down, merges it or swaps it;
//! - right click (or Control-click) picks up half a stack, or puts down one item;
//! - drag a held stack across slots to spread it evenly (left button) or one by one
//!   (right button), previewed while dragging;
//! - double click gathers every stack of the item onto the pointer;
//! - shift click sends a stack between the hotbar and the rest, and shift double click
//!   sends every stack of that item;
//! - 1 to 9 over a slot swaps it with that hotbar slot;
//! - Q over a slot throws one item out into the world, Ctrl+Q the stack, and a click
//!   outside the panel throws the held stack (a right click, one item of it);
//! - hovering an item names it, coloured by rarity, with what it is good for, and for a
//!   seed the level it takes to plant;
//! - under the slots, every skill's level and how far it is to the next.
//!
//! Each gesture becomes moves of the engine's inventory rules (`voxmar_engine::inventory`),
//! proposed to the server as `core:transfer`, `core:quick_move`, `core:stow` and `core:drop` actions and
//! predicted here at once. Closing the screen puts a held stack back.

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::PrimaryWindow;
use voxmar_engine::action::ActionTarget;
use voxmar_engine::block::{BlockDef, Rarity};
use voxmar_engine::craft::GRID;
use voxmar_engine::inventory::{CRAFT, CURSOR, HOTBAR_SLOTS, Inventory, ItemStack, MAX_STACK, SLOTS};

use super::{SLOT, slot_contents};
use crate::lang::{Locale, Localized, TightLine};
use crate::Actions;
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::{Silent, UiSound};
use crate::session::Launch;
use crate::ui::widgets::{BODY, ERROR, GOLD, LARGE, MUTED, SCRIM, SOFT, TEXT, UiFont, panel, screen, shadowed, spawn_heading, text};
use crate::ui::widgets::space;
use crate::world::ClientWorld;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShownInventory>()
            .init_resource::<Gesture>()
            .add_systems(PostStartup, spawn)
            .add_systems(
                Update,
                ((toggle, click, craft, number_keys, preview).chain().in_set(InventorySystems).after(super::screens::ScreenSystems), (follow_pointer, tooltip, tap, hover_item).after(InventorySystems))
                    .run_if(in_state(Screen::InGame)),
            )
            .add_systems(OnExit(Screen::InGame), |mut gesture: ResMut<Gesture>| *gesture = Gesture::default());
    }
}

/// Reading the inventory screen's input and working out what it shows.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InventorySystems;

/// The inventory as the screen shows it: the player's, with a drag in progress spread.
#[derive(Resource, Default)]
pub struct ShownInventory(pub Inventory);

/// Seconds between two clicks on a slot for them to be a double click.
const DOUBLE_CLICK: f32 = 0.3;

/// What the mouse is in the middle of.
#[derive(Resource, Default)]
struct Gesture {
    /// A held stack being spread: by the left button (evenly) or the right (one each),
    /// over these slots so far.
    drag: Option<(MouseButton, Vec<usize>)>,
    /// The last left click: its slot, when, whether it picked a stack up and, for a shift
    /// click, the item it sent.
    last_click: Option<LastClick>,
}

#[derive(Clone, Copy)]
struct LastClick {
    slot: usize,
    at: f32,
    picked_up: bool,
    sent: Option<voxmar_engine::block::BlockId>,
}

#[derive(Component)]
struct InventoryScreen;
#[derive(Component)]
struct InventorySlot(usize);
/// The panel (and creative mode's catalogue beside it); clicks outside them throw the held
/// stack.
#[derive(Component)]
pub(crate) struct InventoryCard;
/// The held stack, drawn at the pointer.
#[derive(Component)]
struct PointerStack;
#[derive(Component)]
struct Tooltip;
#[derive(Component)]
struct TooltipLines;
/// What the crafting grid makes: click to craft one onto the pointer, shift-click to craft
/// as many as fit into the inventory.
#[derive(Component)]
struct OutputSlot;
#[derive(Component)]
struct OutputIcon;
/// The words beside the crafting grid's result: its name, then how to take it.
#[derive(Component)]
struct CraftName;
#[derive(Component)]
struct CraftHint;
/// Room for those words, wrapped.
const CRAFT_WORDS: f32 = 120.0 * PX;
#[derive(Component)]
struct OutputCount;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let columns = HOTBAR_SLOTS as f32;
    let grid_width = columns * SLOT + (columns - 1.0) * PX;
    commands.spawn((InventoryScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        let mut card = panel(grid_width + 76.0 * PX);
        card.0.align_items = AlignItems::Center;
        root.spawn((card, InventoryCard, RelativeCursorPosition::default())).with_children(|card| {
            card.spawn(Node { width: Val::Percent(100.0), flex_direction: FlexDirection::Column, ..default() })
                .with_children(|heading| spawn_heading(heading, font, "screen.inventory"));
            let grid = |top: f32| Node {
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::px(HOTBAR_SLOTS as u16, SLOT),
                row_gap: Val::Px(PX),
                column_gap: Val::Px(PX),
                margin: UiRect::top(Val::Px(top)),
                ..default()
            };
            let spawn_slot = |grid: &mut ChildSpawnerCommands, slot: usize| {
                let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() };
                grid.spawn((Button, Silent, InventorySlot(slot), RelativeCursorPosition::default(), node, Piece::Well.image()))
                    .with_children(|cell| slot_contents(cell, font, slot));
            };
            // The crafting grid and what it makes, above the pack.
            let crafting = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), margin: UiRect::bottom(Val::Px(space::GAP)), ..default() };
            card.spawn(crafting).with_children(|row| {
                let square = Node {
                    display: Display::Grid,
                    grid_template_columns: RepeatedGridTrack::px(2, SLOT),
                    row_gap: Val::Px(PX),
                    column_gap: Val::Px(PX),
                    ..default()
                };
                row.spawn(square).with_children(|grid| (CRAFT..CRAFT + GRID).for_each(|slot| spawn_slot(grid, slot)));
                row.spawn(shadowed(font, "→", LARGE, GOLD));
                let node = Node { width: Val::Px(SLOT + 4.0 * PX), height: Val::Px(SLOT + 4.0 * PX), ..default() };
                row.spawn((Button, Silent, OutputSlot, RelativeCursorPosition::default(), node, Piece::Frame.image())).with_children(|cell| {
                    let inset = (SLOT + 4.0 * PX - 16.0 * PX) / 2.0;
                    let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
                    cell.spawn((icon, ImageNode::default(), Visibility::Hidden, OutputIcon));
                    cell.spawn((Node { position_type: PositionType::Absolute, right: Val::Px(super::COUNT_INSET + 2.0 * PX), bottom: Val::Px(super::COUNT_INSET + 2.0 * PX), ..default() }, shadowed(font, "", BODY, TEXT), OutputCount));
                });
                // What the grid makes and how to take it, or how to find a recipe.
                let words = Node { width: Val::Px(CRAFT_WORDS), flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), ..default() };
                row.spawn(words).with_children(|words| {
                    words.spawn((shadowed(font, "", BODY, TEXT), CraftName));
                    words.spawn((text(font, "", BODY, MUTED), CraftHint));
                });
            });
            card.spawn(grid(0.0)).with_children(|grid| (HOTBAR_SLOTS..SLOTS).for_each(|slot| spawn_slot(grid, slot)));
            card.spawn(grid(5.0 * PX)).with_children(|grid| (0..HOTBAR_SLOTS).for_each(|slot| spawn_slot(grid, slot)));
            spawn_legend(card, font);
        });
        super::screens::spawn_hotkeys(root, super::screens::GameScreen::Inventory);
        // The held stack and the tooltip float above everything, at the pointer.
        let pointer = Node { position_type: PositionType::Absolute, width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() };
        root.spawn((PointerStack, pointer, GlobalZIndex(3))).with_children(|cell| slot_contents(cell, font, CURSOR));
        let tip = Node {
            position_type: PositionType::Absolute,
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(PX),
            padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)),
            ..default()
        };
        root.spawn((Tooltip, TooltipLines, tip, Piece::Tooltip.image(), Visibility::Hidden, GlobalZIndex(4)));
    });
}

/// A gesture's effect on the inventory.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Move {
    Items { from: usize, to: usize, count: u8 },
    Quick(usize),
    Stow,
    /// Throw items out of a slot (or off the pointer) into the world.
    Throw { slot: usize, count: u8 },
}

/// Predicts a move and proposes it to the server, which checks it by the same rules.
fn send(net: &Net, actions: &Actions, inventory: &mut Inventory, action: Move) {
    let (moved, name, target) = match action {
        Move::Items { from, to, count } => {
            (inventory.move_items(from, to, count), "core:transfer", ActionTarget::Slots { from: from as u8, to: to as u8, count })
        }
        Move::Quick(slot) => (inventory.quick_move(slot), "core:quick_move", ActionTarget::Slot(slot as u8)),
        Move::Stow => (inventory.stow_cursor(), "core:stow", ActionTarget::None),
        Move::Throw { slot, count } => {
            (inventory.take(slot, count).is_some(), "core:drop", ActionTarget::Stack { slot: slot as u8, count })
        }
    };
    if moved {
        net.propose(actions.0.expect(name), target);
    }
}

/// Shows what the crafting grid makes, and crafts it when clicked.
#[allow(clippy::too_many_arguments)]
fn craft(
    ui: Res<UiState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    launch: Res<crate::session::Launch>,
    images: Res<super::HudImages>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
    output: Query<&RelativeCursorPosition, With<OutputSlot>>,
    mut icon: Query<(&mut ImageNode, &mut Visibility), With<OutputIcon>>,
    mut count: Query<&mut Text, (With<OutputCount>, Without<CraftName>, Without<CraftHint>)>,
    mut name: Query<&mut Text, (With<CraftName>, Without<OutputCount>, Without<CraftHint>)>,
    mut hint: Query<&mut Text, (With<CraftHint>, Without<OutputCount>, Without<CraftName>)>,
    (locale, settings): (Res<Locale>, Res<Settings>),
    mut sounds: MessageWriter<UiSound>,
) {
    if !ui.inventory_open {
        return;
    }
    let recipes = launch.content.recipes.clone();
    let made = recipes.find(&player.inventory.grid()).map(|r| (r.output, r.count));
    let filled = player.inventory.grid().iter().any(Option::is_some);
    let (title, how) = match made {
        Some((item, _)) => (locale.block(launch.content.blocks.def(item)), locale.get("inventory.craft.take").to_owned()),
        None if filled => (locale.get("inventory.craft.nothing").to_owned(), locale.format("inventory.craft.lookup", &[("key", &settings.recipes_key.name(&locale))])),
        None => (locale.get("inventory.craft").to_owned(), locale.format("inventory.craft.lookup", &[("key", &settings.recipes_key.name(&locale))])),
    };
    for (query, words) in [(name.single_mut().ok(), title), (hint.single_mut().ok(), how)] {
        if let Some(mut text) = query
            && text.0 != words
        {
            text.0 = words;
        }
    }
    if let Ok((mut image, mut visibility)) = icon.single_mut() {
        match made {
            Some((item, _)) => {
                let handle = &images.icons[item.0 as usize];
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
    if let Ok(mut text) = count.single_mut() {
        let value = made.filter(|(_, n)| *n > 1).map_or(String::new(), |(_, n)| n.to_string());
        if text.0 != value {
            text.0 = value;
        }
    }
    let clicked = mouse.just_pressed(MouseButton::Left) && output.single().is_ok_and(|cursor| cursor.cursor_over());
    if !clicked || made.is_none() {
        return;
    }
    let all = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let recipe = |grid: &[Option<voxmar_engine::block::BlockId>; GRID]| recipes.find(grid).map(|r| (r.output, r.count));
    if player.inventory.craft(recipe, all) > 0 {
        net.propose(actions.0.expect("core:craft"), ActionTarget::Slot(if all { 0 } else { 1 }));
        sounds.write(UiSound::Click);
    }
}

/// Shows the screen while it is open (`screens` opens and closes it). Closing puts back
/// what the pointer holds.
fn toggle(
    mut ui: ResMut<UiState>,
    mut player: ResMut<LocalPlayer>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut gesture: ResMut<Gesture>,
    mut root: Query<&mut Visibility, With<InventoryScreen>>,
    mut was_open: Local<bool>,
) {
    if player.dead {
        ui.inventory_open = false;
    }
    if *was_open && !ui.inventory_open {
        *gesture = Gesture::default();
        send(&net, &actions, &mut player.inventory, Move::Stow);
    }
    *was_open = ui.inventory_open;
    if let Ok(mut visibility) = root.single_mut() {
        // The recipe viewer takes the screen while it is open.
        visibility.set_if_neq(if ui.inventory_open && !ui.recipes_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// The slot under the pointer.
fn hovered(slots: &Query<(&InventorySlot, &RelativeCursorPosition)>) -> Option<usize> {
    slots.iter().find(|(_, cursor)| cursor.cursor_over()).map(|(slot, _)| slot.0)
}

/// Whether a drag could drop some of `held` into `slot`.
fn accepts(inventory: &Inventory, slot: usize, held: ItemStack) -> bool {
    inventory.get(slot).is_none_or(|s| s.item == held.item && s.count < MAX_STACK)
}

/// The moves that finish a drag over `slots`: an even share (left button) or one item
/// (right) into each, as far as each has room.
fn spread(inventory: &Inventory, button: MouseButton, slots: &[usize]) -> Vec<Move> {
    let Some(held) = inventory.cursor else { return Vec::new() };
    let each = if button == MouseButton::Left { held.count / slots.len().max(1) as u8 } else { 1 };
    slots.iter().map(|&to| Move::Items { from: CURSOR, to, count: each.max(1) }).collect()
}

/// Every stack of `item` the pointer can take, fullest last so part stacks go first, as
/// moves onto the pointer.
fn gather(inventory: &Inventory, item: voxmar_engine::block::BlockId) -> Vec<Move> {
    let mut sources: Vec<(u8, usize)> =
        (0..SLOTS).filter_map(|slot| inventory.get(slot).filter(|s| s.item == item).map(|s| (s.count, slot))).collect();
    sources.sort();
    sources.into_iter().map(|(count, from)| Move::Items { from, to: CURSOR, count }).collect()
}

#[allow(clippy::too_many_arguments)]
fn click(
    time: Res<Time>,
    ui: Res<UiState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
    mut gesture: ResMut<Gesture>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
    card: Query<(&RelativeCursorPosition, &InheritedVisibility), With<InventoryCard>>,
) {
    if !ui.inventory_open {
        return;
    }
    let now = time.elapsed_secs();
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    // Control-click is the right click of one-button mice and trackpads on the Mac.
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let over = hovered(&slots);
    let inventory = &mut player.inventory;

    // A drag in progress picks up every slot the pointer crosses that can take some.
    if let Some((button, dragged)) = &mut gesture.drag {
        let button = *button;
        if let (Some(slot), Some(held)) = (over, inventory.cursor)
            && !dragged.contains(&slot)
            && accepts(inventory, slot, held)
            && dragged.len() < held.count as usize
        {
            dragged.push(slot);
        }
        if mouse.just_released(button) {
            let dragged = std::mem::take(dragged);
            gesture.drag = None;
            let moves = match (dragged.as_slice(), inventory.cursor) {
                // The server may have taken the stack meanwhile.
                ([], _) | (_, None) => Vec::new(),
                // A drag that stayed on one slot is a plain click on it.
                (&[slot], Some(held)) => {
                    let count = if button == MouseButton::Right && accepts(inventory, slot, held) { 1 } else { held.count };
                    vec![Move::Items { from: CURSOR, to: slot, count }]
                }
                (slots, Some(_)) => spread(inventory, button, slots),
            };
            for action in moves {
                send(&net, &actions, inventory, action);
            }
        }
        return;
    }

    let left = mouse.just_pressed(MouseButton::Left);
    let right = mouse.just_pressed(MouseButton::Right) || (left && control);
    let left = left && !control;
    // Outside the panel, a click throws the held stack, a right click one item of it.
    let outside = !card.is_empty() && !card.iter().any(|(card, visible)| visible.get() && card.cursor_over());
    if outside
        && (left || right)
        && let Some(held) = inventory.cursor
    {
        let count = if right { 1 } else { held.count };
        send(&net, &actions, inventory, Move::Throw { slot: CURSOR, count });
        return;
    }
    let Some(slot) = over.filter(|_| left || right) else { return };
    let double = gesture.last_click.filter(|c| left && c.slot == slot && now - c.at < DOUBLE_CLICK);
    let mut record = LastClick { slot, at: now, picked_up: false, sent: None };

    if left && shift {
        // Shift double click: every stack of the item just sent goes too.
        if let Some(item) = double.and_then(|c| c.sent) {
            let section = if slot < HOTBAR_SLOTS { 0..HOTBAR_SLOTS } else { HOTBAR_SLOTS..SLOTS };
            let matching: Vec<usize> = section.filter(|&s| inventory.get(s).is_some_and(|stack| stack.item == item)).collect();
            for other in matching {
                send(&net, &actions, inventory, Move::Quick(other));
            }
        } else {
            record.sent = inventory.get(slot).map(|s| s.item);
            send(&net, &actions, inventory, Move::Quick(slot));
        }
    } else if let Some(held) = inventory.cursor {
        if double.is_some_and(|c| c.picked_up) {
            // The second click of a double click gathers the item onto the pointer.
            for action in gather(inventory, held.item) {
                send(&net, &actions, inventory, action);
            }
        } else {
            let button = if right { MouseButton::Right } else { MouseButton::Left };
            gesture.drag = Some((button, vec![slot]));
        }
    } else if let Some(stack) = inventory.get(slot) {
        let count = if right { stack.count.div_ceil(2) } else { stack.count };
        send(&net, &actions, inventory, Move::Items { from: slot, to: CURSOR, count });
        record.picked_up = left;
    }
    if left {
        gesture.last_click = Some(record);
    }
}

/// 1 to 9 over a slot swap it with that hotbar slot.
fn number_keys(
    ui: Res<UiState>,
    keys: Res<ButtonInput<KeyCode>>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
    gesture: Res<Gesture>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
) {
    const DIGITS: [KeyCode; HOTBAR_SLOTS] = [
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
    if !ui.inventory_open || gesture.drag.is_some() || player.inventory.cursor.is_some() {
        return;
    }
    // Q throws one item from the slot under the pointer, Ctrl+Q all of it.
    if keys.just_pressed(KeyCode::KeyQ)
        && let Some(slot) = hovered(&slots)
        && let Some(stack) = player.inventory.get(slot)
    {
        let whole = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        send(&net, &actions, &mut player.inventory, Move::Throw { slot, count: if whole { stack.count } else { 1 } });
        return;
    }
    let (Some(slot), Some(key)) = (hovered(&slots), DIGITS.iter().position(|&k| keys.just_pressed(k))) else { return };
    let inventory = &mut player.inventory;
    let action = match (inventory.get(slot), inventory.get(key)) {
        _ if slot == key => return,
        (Some(stack), _) => Move::Items { from: slot, to: key, count: stack.count },
        (None, Some(stack)) => Move::Items { from: key, to: slot, count: stack.count },
        (None, None) => return,
    };
    send(&net, &actions, inventory, action);
}

/// What the screen shows: the inventory, with a drag over several slots already spread.
fn preview(player: Res<LocalPlayer>, gesture: Res<Gesture>, mut shown: ResMut<ShownInventory>) {
    let mut inventory = player.inventory.clone();
    if let Some((button, slots)) = &gesture.drag
        && (slots.len() > 1 || *button == MouseButton::Right)
    {
        for action in spread(&inventory.clone(), *button, slots) {
            if let Move::Items { from, to, count } = action {
                inventory.move_items(from, to, count);
            }
        }
    }
    if shown.0 != inventory {
        shown.0 = inventory;
    }
}

/// Keeps the held stack centred on the pointer, and lights the slot under it.
fn follow_pointer(
    ui: Res<UiState>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut held: Query<&mut Node, With<PointerStack>>,
    mut slots: Query<(&InventorySlot, &RelativeCursorPosition, &mut ImageNode)>,
    gesture: Res<Gesture>,
    scale: Res<UiScale>,
) {
    if !ui.inventory_open {
        return;
    }
    if let (Ok(window), Ok(mut node)) = (window.single(), held.single_mut())
        && let Some(at) = window.cursor_position()
    {
        // The pointer is in the window's points, the interface laid out at its own scale.
        let at = at / scale.0;
        node.left = Val::Px(at.x - SLOT / 2.0);
        node.top = Val::Px(at.y - SLOT / 2.0);
    }
    let dragged = gesture.drag.as_ref().map_or(&[][..], |(_, slots)| slots.as_slice());
    for (slot, cursor, mut image) in &mut slots {
        let piece = if dragged.contains(&slot.0) {
            Piece::WellFocus
        } else if cursor.cursor_over() {
            Piece::WellHover
        } else {
            Piece::Well
        };
        if image.image != piece.handle() {
            image.image = piece.handle();
        }
    }
}

/// A light tap whenever the pointer picks up or puts down items.
fn tap(ui: Res<UiState>, player: Res<LocalPlayer>, mut sounds: MessageWriter<UiSound>, mut held: Local<Option<ItemStack>>) {
    let now = player.inventory.cursor.filter(|_| ui.inventory_open);
    if now != *held {
        if ui.inventory_open {
            sounds.write(UiSound::Item);
        }
        *held = now;
    }
}

/// The colour an item's name is written in, from its rarity.
pub fn rarity_color(rarity: Rarity) -> Color {
    match rarity {
        Rarity::Common => TEXT,
        Rarity::Uncommon => Color::srgb(1.0, 0.89, 0.48),
        Rarity::Rare => Color::srgb(0.45, 0.94, 1.0),
        Rarity::VeryRare => Color::srgb(0.82, 0.58, 1.0),
        Rarity::Legendary => GOLD,
    }
}

/// The lines of an item's tooltip after its name: its rarity, what it is good for (from
/// its tags), and the package it comes from.
fn details(locale: &Locale, def: &BlockDef, ids: bool) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    let rarity = match def.rarity {
        Rarity::Common => None,
        Rarity::Uncommon => Some("rarity.uncommon"),
        Rarity::Rare => Some("rarity.rare"),
        Rarity::VeryRare => Some("rarity.very_rare"),
        Rarity::Legendary => Some("rarity.legendary"),
    };
    if let Some(rarity) = rarity {
        lines.push((locale.get(rarity).to_owned(), rarity_color(def.rarity)));
    }
    let mut uses: Vec<String> = Vec::new();
    for tag in &def.tags {
        match tag.as_str() {
            "edible" => uses.push(locale.get("item.edible").into()),
            "fuel" => uses.push(locale.get("item.fuel").into()),
            "light" => uses.push(locale.get("item.light").into()),
            _ => {
                if let Some(property) = tag.strip_prefix("alchemy:") {
                    let name = locale.get_or(&format!("alchemy.{property}"), &property.replace('_', " ")).to_owned();
                    uses.push(locale.format("item.alchemy", &[("property", &name)]));
                }
            }
        }
    }
    if !uses.is_empty() {
        lines.push((uses.join(locale.get("language.dot")), SOFT));
    }
    let package = def.name.split(':').next().unwrap_or("");
    lines.push((if ids { def.name.clone() } else { package.to_owned() }, MUTED));
    lines
}

/// For a seed, what it plants and the level that takes, red while the player is below it.
fn seed_line(
    locale: &Locale,
    content: &voxmar_engine::content::Content,
    player: &LocalPlayer,
    item: voxmar_engine::block::BlockId,
) -> Option<(String, Color)> {
    let crop = content.crops.get(content.crops.planted_by(item)?);
    let skill = locale.skill(content.skills.get(crop.skill));
    let ready = content.skills.level(player.skills.xp(crop.skill)) >= crop.def.level;
    let words = locale.format("item.plants", &[("crop", &locale.block_named(&content.blocks, &crop.def.name)), ("skill", &skill), ("level", &crop.def.level)]);
    Some((words, if ready { SOFT } else { ERROR }))
}

/// The mouse and key moves the inventory takes, as key caps with what each does, two to a
/// row under the slots.
const LEGEND: [(&str, &str); 6] = [
    ("inventory.key.right_click", "inventory.hint.half"),
    ("inventory.key.drag", "inventory.hint.spread"),
    ("inventory.key.double_click", "inventory.hint.gather"),
    ("inventory.key.shift_click", "inventory.hint.move"),
    ("inventory.key.numbers", "inventory.hint.swap"),
    ("inventory.key.throw", "inventory.hint.throw"),
];

fn spawn_legend(card: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    let legend = Node {
        display: Display::Grid,
        grid_template_columns: RepeatedGridTrack::auto(2),
        column_gap: Val::Px(space::SECTION),
        row_gap: Val::Px(space::HAIR),
        margin: UiRect::top(Val::Px(space::SECTION)),
        ..default()
    };
    card.spawn(legend).with_children(|legend| {
        for (key, hint) in LEGEND {
            legend.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() }).with_children(|entry| {
                let cap = Node { padding: UiRect::all(Val::Px(space::INNER)), min_width: Val::Px(crate::ui::skin::CAP_MIN), justify_content: JustifyContent::Center, ..default() };
                entry.spawn((cap, Piece::Button.image())).with_child((Localized(key), shadowed(font, "", BODY, SOFT), TightLine));
                entry.spawn((Localized(hint), text(font, "", BODY, MUTED)));
            });
        }
    });
}

/// Tells the recipe viewer which item the pointer is over: a slot's, or what the crafting
/// grid makes.
fn hover_item(
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
    output: Query<&RelativeCursorPosition, With<OutputSlot>>,
    mut over: ResMut<super::recipes::HoveredItem>,
    window: Query<&Window, With<PrimaryWindow>>,
    catalogue: Res<super::creative::Catalogue>,
) {
    // Without a pointer (no window: `--gate`, `--capture`) nothing is under it to change.
    if ui.recipes_open || window.single().ok().and_then(Window::cursor_position).is_none() {
        return;
    }
    let made = || output.iter().any(RelativeCursorPosition::cursor_over).then(|| launch.content.recipes.find(&player.inventory.grid()).map(|r| r.output)).flatten();
    let item = if ui.inventory_open { hovered(&slots).and_then(|slot| player.inventory.get(slot)).map(|stack| stack.item).or_else(made).or(catalogue.hovered) } else { None };
    if over.0 != item {
        over.0 = item;
    }
}

/// Names the item under the pointer, beside it, while nothing is held.
#[allow(clippy::too_many_arguments)]
fn tooltip(
    mut commands: Commands,
    ui: Res<UiState>,
    font: Res<UiFont>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    settings: Res<Settings>,
    gesture: Res<Gesture>,
    window: Query<&Window, With<PrimaryWindow>>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
    mut tip: Query<(Entity, &mut Node, &mut Visibility, &ComputedNode), With<Tooltip>>,
    mut shown: Local<Option<(voxmar_engine::block::BlockId, bool, bool)>>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    (keys, scale, catalogue): (Res<ButtonInput<KeyCode>>, Res<UiScale>, Res<super::creative::Catalogue>),
    mut japanese: Local<Option<Locale>>,
) {
    let Ok((entity, mut node, mut visibility, computed)) = tip.single_mut() else { return };
    let item = hovered(&slots)
        .filter(|_| ui.inventory_open && player.inventory.cursor.is_none() && gesture.drag.is_none())
        .and_then(|slot| player.inventory.get(slot))
        .map(|stack| stack.item)
        .or(catalogue.hovered.filter(|_| ui.inventory_open && player.inventory.cursor.is_none()));
    let Some(item) = item else {
        visibility.set_if_neq(Visibility::Hidden);
        *shown = None;
        return;
    };
    // Shift shows how the name reads: in Japanese, the kana over the name; in another
    // language, the Japanese name with its kana under it.
    let reading = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let key = (item, settings.debug_overlay, reading);
    if *shown != Some(key) || locale.is_changed() {
        *shown = Some(key);
        let def = world.registry.def(item);
        let readings: Vec<String> = if !reading {
            Vec::new()
        } else if locale.code() == "ja" {
            locale.block_reading(def).into_iter().collect()
        } else {
            let japanese = japanese.get_or_insert_with(|| Locale::load(&voxmar_server::assets_dir().join("content"), "ja"));
            [Some(japanese.block(def)), japanese.block_reading(def)].into_iter().flatten().collect()
        };
        let in_japanese = locale.code() == "ja";
        commands.entity(entity).despawn_children().with_children(|lines| {
            if in_japanese {
                for line in &readings {
                    lines.spawn(text(&font.0, line, BODY, SOFT));
                }
            }
            lines.spawn(shadowed(&font.0, locale.block(def), BODY, rarity_color(def.rarity)));
            if !in_japanese {
                for line in &readings {
                    lines.spawn(text(&font.0, line, BODY, SOFT));
                }
            }
            let lines_after = seed_line(&locale, &launch.content, &player, item).into_iter().chain(details(&locale, def, settings.debug_overlay));
            for (line, color) in lines_after {
                lines.spawn(text(&font.0, line, BODY, color));
            }
        });
    }
    visibility.set_if_neq(Visibility::Inherited);
    if let Ok(window) = window.single()
        && let Some(at) = window.cursor_position()
    {
        // Up and to the right of the pointer, kept on screen; the pointer is in the
        // window's points, the interface laid out at its own scale.
        let (at, width) = (at / scale.0, window.width() / scale.0);
        let size = computed.size() * computed.inverse_scale_factor();
        let left = (at.x + 6.0 * PX).min(width - size.x - 2.0 * PX);
        let top = (at.y - size.y - 2.0 * PX).max(2.0 * PX);
        node.left = Val::Px(left);
        node.top = Val::Px(top);
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::block::BlockId;

    use super::*;

    fn held(count: u8) -> Inventory {
        let mut inventory = Inventory::default();
        inventory.cursor = Some(ItemStack { item: BlockId(1), count });
        inventory
    }

    #[test]
    fn drags_spread_evenly_or_one_by_one() {
        let inventory = held(10);
        let moves = spread(&inventory, MouseButton::Left, &[0, 1, 2]);
        assert_eq!(moves, [0, 1, 2].map(|to| Move::Items { from: CURSOR, to, count: 3 }));
        let moves = spread(&inventory, MouseButton::Right, &[4, 5]);
        assert_eq!(moves, [4, 5].map(|to| Move::Items { from: CURSOR, to, count: 1 }));
        // What is left over stays on the pointer.
        let mut after = inventory.clone();
        for action in spread(&inventory, MouseButton::Left, &[0, 1, 2]) {
            let Move::Items { from, to, count } = action else { unreachable!() };
            after.move_items(from, to, count);
        }
        assert_eq!(after.cursor.map(|s| s.count), Some(1));
    }

    #[test]
    fn double_clicks_gather_part_stacks_first() {
        let mut inventory = held(5);
        inventory.slots[3] = Some(ItemStack { item: BlockId(1), count: 64 });
        inventory.slots[7] = Some(ItemStack { item: BlockId(1), count: 20 });
        inventory.slots[8] = Some(ItemStack { item: BlockId(2), count: 20 });
        let moves = gather(&inventory, BlockId(1));
        assert_eq!(moves, vec![Move::Items { from: 7, to: CURSOR, count: 20 }, Move::Items { from: 3, to: CURSOR, count: 64 }]);
        for action in moves {
            let Move::Items { from, to, count } = action else { unreachable!() };
            inventory.move_items(from, to, count);
        }
        assert_eq!(inventory.cursor.map(|s| s.count), Some(64));
        assert_eq!(inventory.get(3).map(|s| s.count), Some(25));
    }
}
