//! The widgets every screen is built from, in the interface's one look (see `skin`):
//! pixel text, gold-trimmed slate panels, raised buttons that light up teal under the
//! pointer and sink when pressed, recessed fields, and scrolling lists.

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::text::FontSmoothing;
use bevy::ui::RelativeCursorPosition;

use super::ime::{Composing, ImeSystems};
use super::skin::{FONT, PX, Piece, font_size};
use crate::lang::Localized;

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(UiFont(FONT))
            .add_systems(Update, (style_buttons, scroll_lists, show_scroll_more, (focus_fields, edit_fields, draw_fields).chain().after(ImeSystems)));
    }
}

/// Behind a centred panel.
pub const SCRIM: Color = Color::srgba(0.02, 0.02, 0.05, 0.45);
/// Warm parchment white, for most text.
pub const TEXT: Color = Color::srgb(0.95, 0.92, 0.84);
pub const SOFT: Color = Color::srgb(0.78, 0.79, 0.86);
pub const MUTED: Color = Color::srgb(0.55, 0.58, 0.70);
/// Headings, and what is chosen.
pub const GOLD: Color = Color::srgb(0.96, 0.80, 0.42);
/// Arcane teal: what the pointer is on, what is on, what is focused.
pub const ACCENT: Color = Color::srgb(0.37, 0.88, 0.82);
/// Text reporting a problem.
pub const ERROR: Color = Color::srgb(1.0, 0.52, 0.48);
/// The dark under pixel text.
pub const SHADOW: Color = Color::srgba(0.0, 0.0, 0.02, 0.85);

/// The interface's spacing scale, in screen pixels: every margin, padding and gap is one
/// of these steps, so screens line up with one another and nothing sits on a border.
pub mod space {
    use crate::ui::skin::PX;
    /// Between lines of one block, and a mark and its words.
    pub const HAIR: f32 = 2.0 * PX;
    /// Inside small elements (chips, caps, tiles), and between rows of a list.
    pub const INNER: f32 = 4.0 * PX;
    /// Between tiles, controls and the elements of a section.
    pub const GAP: f32 = 6.0 * PX;
    /// Between the sections of a panel.
    pub const SECTION: f32 = 8.0 * PX;
    /// A panel's margin inside its frame.
    pub const OUTER: f32 = 12.0 * PX;
}

/// Text sizes: two screen pixels per font pixel for body text, three for headings.
pub const BODY: f32 = font_size(2);
pub const LARGE: f32 = font_size(3);

/// The pixel font. A resource so the screens that predate the skin can keep asking for
/// it; it is always the same handle, `skin::FONT`.
#[derive(Resource)]
pub struct UiFont(pub Handle<Font>);

/// Pixel text. Sizes snap to whole screen pixels per font pixel, at least two.
pub fn text(font: &Handle<Font>, value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    let size = font_size((size / font_size(1)).round().max(2.0) as u32);
    let font = TextFont { font: font.into(), font_size: FontSize::Px(size), font_smoothing: FontSmoothing::None, ..default() };
    (Text::new(value), font, TextColor(color))
}

/// Pixel text with a drop shadow one font pixel down and right, for text over the world
/// or on buttons.
pub fn shadowed(font: &Handle<Font>, value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor, TextShadow) {
    let (text, font, color) = text(font, value, size, color);
    let FontSize::Px(px) = font.font_size else { unreachable!("pixel text has a size in pixels") };
    (text, font, color, TextShadow { offset: Vec2::splat(px / font_size(1)), color: SHADOW })
}

/// A panel's title, in gold between two teal diamonds, over a gold rule. `key` names its
/// words in the player's language (see `lang`), as for every widget with a label.
pub fn spawn_heading(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, key: &'static str) {
    let row = Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() };
    parent.spawn(row).with_children(|row| {
        row.spawn(shadowed(font, "◆", BODY, ACCENT));
        row.spawn((Localized(key), shadowed(font, "", LARGE, GOLD)));
        row.spawn(shadowed(font, "◆", BODY, ACCENT));
    });
    spawn_rule(parent, 4.0 * PX);
}

/// A thin gold line across a panel, dividing it.
pub fn spawn_rule(parent: &mut ChildSpawnerCommands, margin: f32) {
    let node = Node { height: Val::Px(2.0 * PX), margin: UiRect::vertical(Val::Px(margin)), flex_direction: FlexDirection::Column, ..default() };
    parent.spawn(node).with_children(|rule| {
        rule.spawn((Node { height: Val::Px(PX), ..default() }, BackgroundColor(Color::srgb(0.42, 0.29, 0.12))));
        rule.spawn((Node { height: Val::Px(PX), ..default() }, BackgroundColor(Color::srgb(0.69, 0.51, 0.21))));
    });
}

/// A small label above a group of controls.
pub fn label(font: &Handle<Font>, key: &'static str) -> (Node, Localized, Text, TextFont, TextColor) {
    let margin = UiRect::new(Val::ZERO, Val::ZERO, Val::Px(space::SECTION), Val::Px(space::INNER));
    let (text, font, color) = text(font, "", BODY, MUTED);
    (Node { margin, ..default() }, Localized(key), text, font, color)
}

/// A full-screen layer that centres its content, over a tinted backdrop.
pub fn screen(backdrop: Color) -> (Node, BackgroundColor) {
    let node = Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    };
    (node, BackgroundColor(backdrop))
}

/// The gold-trimmed slate panel menus are laid out in.
pub fn panel(width: f32) -> (Node, ImageNode) {
    let node = Node {
        width: Val::Px(width),
        flex_direction: FlexDirection::Column,
        padding: UiRect::all(Val::Px(space::OUTER)),
        ..default()
    };
    (node, Piece::Panel.image())
}

/// A row of equally wide buttons.
pub fn button_row(top: f32) -> Node {
    Node { margin: UiRect::top(Val::Px(top)), column_gap: Val::Px(space::INNER), ..default() }
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonStyle {
    /// The main action.
    Primary,
    Secondary,
    /// An action that destroys something.
    Danger,
}

impl ButtonStyle {
    /// The pieces drawn at rest, under the pointer and pressed.
    fn pieces(self) -> [Piece; 3] {
        match self {
            ButtonStyle::Primary => [Piece::Primary, Piece::PrimaryHover, Piece::PrimaryDown],
            ButtonStyle::Secondary => [Piece::Button, Piece::ButtonHover, Piece::ButtonDown],
            ButtonStyle::Danger => [Piece::Danger, Piece::DangerHover, Piece::DangerDown],
        }
    }
}

/// A button that cannot be used right now. It is drawn faded and ignores presses.
#[derive(Component)]
pub struct Disabled;

/// A button showing the option currently chosen, ringed in gold.
#[derive(Component)]
pub struct Selected;

/// Filter for buttons pressed this frame.
pub type Pressed = (Changed<Interaction>, Without<Disabled>);

/// Height of a button.
pub const BUTTON: f32 = 20.0 * PX;

/// A button taking an equal share of its row, carrying `action` to identify it. An empty
/// `key` leaves the label for its screen to write.
pub fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    key: &'static str,
    style: ButtonStyle,
    action: impl Bundle,
) -> Entity {
    let node = Node {
        flex_grow: 1.0,
        flex_basis: Val::ZERO,
        height: Val::Px(BUTTON),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    };
    let mut button = parent.spawn((Button, style, node, style.pieces()[0].image(), action));
    if key.is_empty() {
        button.with_child(shadowed(font, "", BODY, TEXT));
    } else {
        button.with_child((Localized(key), shadowed(font, "", BODY, TEXT)));
    }
    button.id()
}

/// A button's top padding as built, kept so pressing it only adds to it.
#[derive(Component, Clone, Copy)]
struct RestPadding(Val);

/// Draws each button for its state: the hover piece under the pointer, the pressed piece
/// with its label two pixels lower while held, gold when selected, faded when disabled.
/// Screens have a few dozen buttons at most, so every one is checked each frame and only
/// what differs is written.
fn style_buttons(
    mut commands: Commands,
    mut buttons: Query<(Entity, &ButtonStyle, &Interaction, Has<Disabled>, Has<Selected>, &mut ImageNode, &mut Node, &Children, Option<&RestPadding>)>,
    mut colors: Query<&mut TextColor>,
) {
    for (entity, style, interaction, disabled, selected, mut image, mut node, children, rest_padding) in &mut buttons {
        // The padding it was built with, which sinking adds to.
        let Some(&RestPadding(rest_top)) = rest_padding else {
            commands.entity(entity).insert(RestPadding(node.padding.top));
            continue;
        };
        let [rest, hover, down] = style.pieces();
        let piece = match (disabled, interaction) {
            (true, _) => rest,
            (false, Interaction::Pressed) => down,
            (false, Interaction::Hovered) => hover,
            (false, Interaction::None) if selected && *style == ButtonStyle::Secondary => Piece::ButtonSelected,
            (false, Interaction::None) => rest,
        };
        let tint = if disabled { Color::srgba(1.0, 1.0, 1.0, 0.45) } else { Color::WHITE };
        if image.image != piece.handle() || image.color != tint {
            image.image = piece.handle();
            image.color = tint;
        }
        let sunk = !disabled && *interaction == Interaction::Pressed;
        let top = if sunk { rest_top.try_add(Val::Px(2.0 * PX)).unwrap_or(rest_top) } else { rest_top };
        if node.padding.top != top {
            node.padding.top = top;
        }
        let color = match (disabled, selected) {
            (true, _) => MUTED,
            (false, true) => GOLD,
            (false, false) => TEXT,
        };
        for &child in children {
            if let Ok(mut text) = colors.get_mut(child) {
                text.set_if_neq(TextColor(color));
            }
        }
    }
}

/// A vertically scrolling container, scrolled with the mouse wheel while hovered.
#[derive(Component)]
#[require(RelativeCursorPosition, ScrollPosition)]
pub struct ScrollList;

fn scroll_lists(
    mut wheel: MessageReader<MouseWheel>,
    mut lists: Query<(&mut ScrollPosition, &RelativeCursorPosition, &ComputedNode), With<ScrollList>>,
) {
    let delta: f32 = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y * 32.0,
            MouseScrollUnit::Pixel => event.y,
        })
        .sum();
    if delta == 0.0 {
        return;
    }
    for (mut scroll, cursor, node) in &mut lists {
        if cursor.cursor_over() {
            let max = (node.content_size().y - node.size().y).max(0.0) * node.inverse_scale_factor();
            scroll.0.y = (scroll.0.y - delta).clamp(0.0, max);
        }
    }
}

/// A small gold arrow in the bottom right corner of a [`ScrollList`]'s parent, shown while
/// there is more of the list below, so text cut off at the edge reads as "scroll on".
#[derive(Component)]
pub struct ScrollMore(Entity);

/// The arrow for `list`, which must share `parent` with it.
pub fn spawn_scroll_more(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, list: Entity) {
    let corner = Node { position_type: PositionType::Absolute, right: Val::Px(space::INNER), bottom: Val::Px(space::HAIR), ..default() };
    parent.spawn((corner, shadowed(font, "▼", BODY, GOLD), ScrollMore(list), Visibility::Hidden, ZIndex(1)));
}

fn show_scroll_more(lists: Query<(&ScrollPosition, &ComputedNode, &Node), With<ScrollList>>, mut arrows: Query<(&ScrollMore, &mut Visibility)>) {
    for (more, mut visibility) in &mut arrows {
        let below = lists.get(more.0).is_ok_and(|(scroll, computed, node)| {
            let max = (computed.content_size().y - computed.size().y).max(0.0) * computed.inverse_scale_factor();
            node.display != Display::None && scroll.0.y < max - 1.0
        });
        visibility.set_if_neq(if below { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// A single-line text input: click to focus it, type, Backspace to delete, Tab to move to
/// the next field.
#[derive(Component)]
pub struct TextField {
    pub value: String,
    max_chars: usize,
    /// Position in the Tab order.
    order: u8,
    text: Entity,
    caret: Entity,
    placeholder: Entity,
}

/// The text field keyboard input goes to.
#[derive(Component)]
pub struct Focused;

#[allow(clippy::too_many_arguments)]
pub fn spawn_text_field(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    value: &str,
    placeholder: &'static str,
    max_chars: usize,
    order: u8,
    focused: bool,
    marker: impl Bundle,
) -> Entity {
    let node = Node {
        height: Val::Px(BUTTON),
        padding: UiRect::horizontal(Val::Px(space::INNER)),
        align_items: AlignItems::Center,
        overflow: Overflow::clip(),
        ..default()
    };
    let mut field = parent.spawn((Button, node, Piece::Well.image(), marker));
    if focused {
        field.insert(Focused);
    }
    let (mut text_entity, mut caret, mut placeholder_entity) = (Entity::PLACEHOLDER, Entity::PLACEHOLDER, Entity::PLACEHOLDER);
    field.with_children(|field| {
        text_entity = field.spawn(text(font, value, BODY, TEXT)).id();
        let caret_node = Node { width: Val::Px(PX), height: Val::Px(8.0 * PX), margin: UiRect::left(Val::Px(PX)), ..default() };
        caret = field.spawn((caret_node, BackgroundColor(ACCENT))).id();
        placeholder_entity = field.spawn((Localized(placeholder), text(font, "", BODY, MUTED.with_alpha(0.7)))).id();
    });
    field.insert(TextField {
        value: value.to_owned(),
        max_chars,
        order,
        text: text_entity,
        caret,
        placeholder: placeholder_entity,
    });
    field.id()
}

fn focus_fields(
    mut commands: Commands,
    fields: Query<(Entity, Ref<Interaction>), With<TextField>>,
    focused: Query<Entity, With<Focused>>,
) {
    for (entity, interaction) in &fields {
        if interaction.is_changed() && *interaction == Interaction::Pressed {
            for old in focused.iter().filter(|&old| old != entity) {
                commands.entity(old).remove::<Focused>();
            }
            commands.entity(entity).insert(Focused);
        }
    }
}

fn edit_fields(
    mut commands: Commands,
    mut input: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut fields: Query<(Entity, &mut TextField, Has<Focused>)>,
    mut composing: ResMut<Composing>,
) {
    // Shortcuts (copy, select all, …) are not text.
    let shortcut = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mut typed = String::new();
    for event in input.read().filter(|event| event.state == ButtonState::Pressed) {
        if event.logical_key == Key::Tab {
            let mut order: Vec<(u8, Entity, bool)> = fields.iter().map(|(entity, field, focused)| (field.order, entity, focused)).collect();
            order.sort_by_key(|&(order, ..)| order);
            let Some(current) = order.iter().position(|&(.., focused)| focused) else { continue };
            let step = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { order.len() - 1 } else { 1 };
            let next = order[(current + step) % order.len()].1;
            commands.entity(order[current].1).remove::<Focused>();
            commands.entity(next).insert(Focused);
            continue;
        }
        let Some((_, mut field, _)) = fields.iter_mut().find(|(.., focused)| *focused) else { continue };
        if event.logical_key == Key::Backspace {
            field.value.pop();
        } else if let Some(text) = event.text.as_ref().filter(|_| !shortcut) {
            for c in text.chars().filter(|c| !c.is_control()) {
                typed.push(c);
                if field.value.chars().count() < field.max_chars {
                    field.value.push(c);
                }
            }
        }
    }
    // What the input method committed (Japanese, for one) goes into the focused field.
    let committed = composing.take(&typed);
    if let Some((_, mut field, _)) = fields.iter_mut().find(|(.., focused)| *focused) {
        for c in committed.chars().filter(|c| !c.is_control()) {
            if field.value.chars().count() < field.max_chars {
                field.value.push(c);
            }
        }
    }
}

/// Shows each field's text, its placeholder while empty, a blinking caret while focused,
/// and a teal rim while focused (fainter while hovered).
fn draw_fields(
    time: Res<Time>,
    composing: Res<Composing>,
    mut fields: Query<(&TextField, Has<Focused>, &Interaction, &mut ImageNode)>,
    mut texts: Query<&mut Text>,
    mut nodes: Query<&mut Node>,
    mut visibility: Query<&mut Visibility>,
) {
    let blink_on = time.elapsed_secs().fract() < 0.55;
    for (field, focused, interaction, mut image) in &mut fields {
        // Text the input method is composing shows after the field's, until committed.
        let shown = if focused && !composing.preedit.is_empty() { format!("{}{}", field.value, composing.preedit) } else { field.value.clone() };
        if let Ok(mut text) = texts.get_mut(field.text)
            && text.0 != shown
        {
            text.0 = shown;
        }
        if let Ok(mut node) = nodes.get_mut(field.placeholder) {
            let display = if field.value.is_empty() && (!focused || composing.preedit.is_empty()) { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
        if let Ok(mut caret) = visibility.get_mut(field.caret) {
            caret.set_if_neq(if focused && blink_on { Visibility::Inherited } else { Visibility::Hidden });
        }
        let piece = if focused {
            Piece::WellFocus
        } else if *interaction == Interaction::None {
            Piece::Well
        } else {
            Piece::WellHover
        };
        if image.image != piece.handle() {
            image.image = piece.handle();
        }
    }
}

/// A recessed meter that fills with `piece` and reads "15/20" in the middle: health,
/// hunger, a skill's progress. Set it with [`set_meter`].
#[derive(Component)]
pub struct Meter {
    fill: Entity,
    label: Entity,
}

/// A meter `width` screen pixels wide.
pub fn spawn_meter(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, piece: Piece, width: Val, marker: impl Bundle) -> Entity {
    // Tall enough for its number to sit inside the frame with a pixel to spare.
    let well = Node { width, height: Val::Px(12.0 * PX), padding: UiRect::all(Val::Px(PX)), ..default() };
    let mut meter = parent.spawn((well, Piece::Well.image(), marker));
    let (mut fill, mut label) = (Entity::PLACEHOLDER, Entity::PLACEHOLDER);
    meter.with_children(|well| {
        fill = well.spawn((Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() }, piece.image())).id();
        let centre = Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        };
        label = well.spawn(centre).with_children(|centre| {
            centre.spawn((shadowed(font, "", BODY, TEXT), crate::lang::TightLine));
        }).id();
    });
    meter.insert(Meter { fill, label });
    meter.id()
}

/// Fills a meter to `share` (0..1) and writes `text` over it.
pub fn set_meter<F: bevy::ecs::query::QueryFilter, G: bevy::ecs::query::QueryFilter>(
    meter: &Meter,
    share: f32,
    text_value: &str,
    nodes: &mut Query<&mut Node, F>,
    children: &Query<&Children>,
    texts: &mut Query<&mut Text, G>,
) {
    if let Ok(mut node) = nodes.get_mut(meter.fill) {
        let width = Val::Percent(share.clamp(0.0, 1.0) * 100.0);
        if node.width != width {
            node.width = width;
        }
    }
    if let Some(mut text) = children.get(meter.label).ok().and_then(|c| c.first()).and_then(|&t| texts.get_mut(t).ok())
        && text.0 != text_value
    {
        text_value.clone_into(&mut text.0);
    }
}
