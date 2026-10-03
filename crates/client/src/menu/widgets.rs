//! The look the menus and the settings panel share (dark rounded panels, a clean system
//! font, green accents) and their widgets: text, panels, buttons with hover states,
//! scrolling lists and single-line text fields.

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_font)
            .add_systems(Update, (style_buttons, scroll_lists, (focus_fields, edit_fields, draw_fields).chain()));
    }
}

pub const SCRIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.5);
pub const PANEL: Color = Color::srgb(0.08, 0.09, 0.11);
pub const OUTLINE: Color = Color::srgba(1.0, 1.0, 1.0, 0.07);
pub const TEXT: Color = Color::srgb(0.93, 0.94, 0.96);
pub const SOFT: Color = Color::srgba(0.93, 0.94, 0.96, 0.7);
pub const MUTED: Color = Color::srgba(0.93, 0.94, 0.96, 0.45);
pub const ACCENT: Color = Color::srgb(0.36, 0.78, 0.5);
pub const ACCENT_HOVER: Color = Color::srgb(0.46, 0.85, 0.59);
pub const ON_ACCENT: Color = Color::srgb(0.04, 0.1, 0.06);
pub const TRACK: Color = Color::srgba(1.0, 1.0, 1.0, 0.14);
pub const SUBTLE: Color = Color::srgba(1.0, 1.0, 1.0, 0.08);
pub const SUBTLE_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.14);
pub const DANGER: Color = Color::srgb(0.84, 0.3, 0.3);
pub const DANGER_HOVER: Color = Color::srgb(0.91, 0.39, 0.39);
/// Text reporting a problem.
pub const ERROR: Color = Color::srgb(1.0, 0.58, 0.55);

/// The sans-serif UI font.
#[derive(Resource)]
pub struct UiFont(pub Handle<Font>);

/// A clean sans-serif from the system; Bevy's built-in font is monospaced and ASCII only.
fn load_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    const CANDIDATES: [&str; 5] = [
        "/System/Library/Fonts/SFNS.ttf",
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let font = CANDIDATES
        .iter()
        .find_map(|path| std::fs::read(path).ok())
        .map_or_else(Handle::default, |bytes| fonts.add(Font::from_bytes(bytes)));
    commands.insert_resource(UiFont(font));
}

pub fn text(font: &Handle<Font>, value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    (Text::new(value), TextFont { font: font.into(), font_size: FontSize::Px(size), ..default() }, TextColor(color))
}

/// Text in a heavier weight.
pub fn strong(font: &Handle<Font>, value: impl Into<String>, size: f32, color: Color, weight: FontWeight) -> (Text, TextFont, TextColor) {
    let mut text = text(font, value, size, color);
    text.1.weight = weight;
    text
}

/// A panel's title.
pub fn heading(font: &Handle<Font>, value: &str) -> (Text, TextFont, TextColor) {
    strong(font, value, 30.0, TEXT, FontWeight::SEMIBOLD)
}

/// A small upper-case label above a group of controls.
pub fn label(font: &Handle<Font>, value: &str) -> (Node, Text, TextFont, TextColor) {
    let margin = UiRect::new(Val::ZERO, Val::ZERO, Val::Px(18.0), Val::Px(8.0));
    let (text, font, color) = text(font, value.to_uppercase(), 12.0, MUTED);
    (Node { margin, ..default() }, text, font, color)
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

/// The dark rounded panel menus are laid out in.
pub fn panel(width: f32) -> (Node, BackgroundColor, BorderColor, BoxShadow) {
    let node = Node {
        width: Val::Px(width),
        flex_direction: FlexDirection::Column,
        padding: UiRect::new(Val::Px(36.0), Val::Px(36.0), Val::Px(28.0), Val::Px(32.0)),
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(18.0)),
        ..default()
    };
    let shadow = BoxShadow::new(Color::srgba(0.0, 0.0, 0.0, 0.45), Val::ZERO, Val::Px(16.0), Val::ZERO, Val::Px(48.0));
    (node, BackgroundColor(PANEL), BorderColor::all(OUTLINE), shadow)
}

/// A row of equally wide buttons.
pub fn button_row(top: f32) -> Node {
    Node { margin: UiRect::top(Val::Px(top)), column_gap: Val::Px(12.0), ..default() }
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
    /// Text colour, background and hovered background.
    fn colors(self) -> (Color, Color, Color) {
        match self {
            ButtonStyle::Primary => (ON_ACCENT, ACCENT, ACCENT_HOVER),
            ButtonStyle::Secondary => (TEXT, SUBTLE, SUBTLE_HOVER),
            ButtonStyle::Danger => (Color::WHITE, DANGER, DANGER_HOVER),
        }
    }
}

/// A button that cannot be used right now. It is drawn faded and ignores presses.
#[derive(Component)]
pub struct Disabled;

/// Filter for buttons pressed this frame.
pub type Pressed = (Changed<Interaction>, Without<Disabled>);

/// A button taking an equal share of its row, carrying `action` to identify it.
pub fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    style: ButtonStyle,
    action: impl Bundle,
) -> Entity {
    let (color, background, _) = style.colors();
    let node = Node {
        flex_grow: 1.0,
        flex_basis: Val::ZERO,
        height: Val::Px(44.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        // Transparent unless a screen marks the button as chosen.
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(12.0)),
        ..default()
    };
    parent
        .spawn((Button, style, node, BackgroundColor(background), BorderColor::all(Color::NONE), action))
        .with_child(strong(font, label, 16.0, color, FontWeight::MEDIUM))
        .id()
}

fn style_buttons(mut buttons: Query<(&ButtonStyle, &Interaction, Has<Disabled>, &mut BackgroundColor)>) {
    for (style, interaction, disabled, mut background) in &mut buttons {
        let (_, normal, hovered) = style.colors();
        let color = if disabled {
            normal.with_alpha(normal.alpha() * 0.35)
        } else if *interaction == Interaction::None {
            normal
        } else {
            hovered
        };
        background.set_if_neq(BackgroundColor(color));
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
    placeholder: &str,
    max_chars: usize,
    order: u8,
    focused: bool,
    marker: impl Bundle,
) -> Entity {
    let node = Node {
        height: Val::Px(44.0),
        padding: UiRect::horizontal(Val::Px(14.0)),
        align_items: AlignItems::Center,
        border: UiRect::all(Val::Px(1.0)),
        border_radius: BorderRadius::all(Val::Px(10.0)),
        overflow: Overflow::clip(),
        ..default()
    };
    let mut field = parent.spawn((Button, node, BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.05)), BorderColor::all(OUTLINE), marker));
    if focused {
        field.insert(Focused);
    }
    let (mut text_entity, mut caret, mut placeholder_entity) = (Entity::PLACEHOLDER, Entity::PLACEHOLDER, Entity::PLACEHOLDER);
    field.with_children(|field| {
        text_entity = field.spawn(text(font, value, 16.0, TEXT)).id();
        let caret_node = Node { width: Val::Px(2.0), height: Val::Px(20.0), margin: UiRect::horizontal(Val::Px(1.0)), ..default() };
        caret = field.spawn((caret_node, BackgroundColor(ACCENT))).id();
        placeholder_entity = field.spawn(text(font, placeholder, 16.0, MUTED)).id();
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
) {
    // Shortcuts (copy, select all, …) are not text.
    let shortcut = keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight]);
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
        } else if let Some(typed) = event.text.as_ref().filter(|_| !shortcut) {
            for c in typed.chars().filter(|c| !c.is_control()) {
                if field.value.chars().count() < field.max_chars {
                    field.value.push(c);
                }
            }
        }
    }
}

/// Shows each field's text, its placeholder while empty, a blinking caret while focused,
/// and a highlighted border while focused or hovered.
fn draw_fields(
    time: Res<Time>,
    mut fields: Query<(&TextField, Has<Focused>, &Interaction, &mut BorderColor)>,
    mut texts: Query<&mut Text>,
    mut nodes: Query<&mut Node>,
    mut visibility: Query<&mut Visibility>,
) {
    let blink_on = time.elapsed_secs().fract() < 0.55;
    for (field, focused, interaction, mut border) in &mut fields {
        if let Ok(mut text) = texts.get_mut(field.text)
            && text.0 != field.value
        {
            text.0.clone_from(&field.value);
        }
        if let Ok(mut node) = nodes.get_mut(field.placeholder) {
            let display = if field.value.is_empty() { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
        if let Ok(mut caret) = visibility.get_mut(field.caret) {
            caret.set_if_neq(if focused && blink_on { Visibility::Inherited } else { Visibility::Hidden });
        }
        let color = if focused {
            ACCENT
        } else if *interaction == Interaction::None {
            OUTLINE
        } else {
            TRACK
        };
        border.set_if_neq(BorderColor::all(color));
    }
}
