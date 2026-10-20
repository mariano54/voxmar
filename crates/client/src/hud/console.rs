//! The console: a line opened with the chat key (T unless rebound), or / to start a
//! command, that says something to
//! everyone, or sends a command (`/time set noon`) to the server, which checks and runs
//! it. Replies and chat show above it, fading after a few seconds while it is closed.
//! Up and down recall earlier lines, Tab completes a command's name and Esc closes it.

use std::collections::VecDeque;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::text::FontSmoothing;
use voxmar_protocol::{ChatKind, MAX_LINE};

use crate::ui::widgets::space;
use crate::ui::ime::{Composing, ImeSystems};
use crate::menu::Screen;
use crate::ui::skin::Piece;
use crate::ui::widgets::{SHADOW, UiFont};
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;

pub struct ConsolePlugin;

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Console>()
            .add_systems(OnEnter(Screen::InGame), spawn)
            .add_systems(Update, (type_line, send_pending, draw).chain().after(ImeSystems).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut console: ResMut<Console>| console.leave());
    }
}

/// Lines shown at most, newest at the bottom.
const SHOWN: usize = 12;
/// Lines kept for scrolling back through history.
const KEPT: usize = 100;
/// Seconds a line stays once the console is closed, the last of them fading out.
const LINGER: f32 = 10.0;
const FADE: f32 = 1.5;
const TEXT_SIZE: f32 = 16.0;
const WIDTH: f32 = 640.0;
/// Space between the console's input line and the bottom of the screen, above the
/// hotbar, hearts and any Raise HUD offset (see `hud::bottom_offset`).
const ABOVE_HUD: f32 = 96.0;

#[derive(Resource, Default)]
pub struct Console {
    /// Lines received, oldest first.
    lines: VecDeque<Line>,
    /// What is being typed while the console is open.
    input: String,
    /// Lines sent before, oldest first, and which one up and down have recalled.
    history: Vec<String>,
    recalled: Option<usize>,
    /// Commands to run once the server has welcomed us (`--command`).
    pending: Vec<String>,
}

struct Line {
    kind: ChatKind,
    text: String,
    /// When it arrived, in seconds since startup.
    at: f32,
}

impl Console {
    /// Shows a line from the server; several lines if `text` has line breaks.
    pub fn push(&mut self, kind: ChatKind, text: &str, now: f32) {
        for line in text.lines() {
            self.lines.push_back(Line { kind, text: line.to_owned(), at: now });
        }
        while self.lines.len() > KEPT {
            self.lines.pop_front();
        }
    }

    /// The lines received strictly after `since` (seconds since startup), oldest first: what
    /// the gate reads the server's replies to an ask sent at `since` from. A line that came in
    /// the frame the ask went out is the answer to something asked before it, never to it.
    pub fn since(&self, since: f32) -> impl Iterator<Item = &str> {
        self.lines.iter().filter(move |line| line.at > since).map(|line| line.text.as_str())
    }

    /// Every line kept, oldest first.
    pub fn all(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|line| line.text.as_str())
    }

    /// Runs these lines (commands with a `/`) as soon as the game has started.
    pub fn run_when_joined(&mut self, lines: Vec<String>) {
        self.pending.extend(lines);
    }

    fn leave(&mut self) {
        self.lines.clear();
        self.input.clear();
        self.recalled = None;
    }

    fn recall(&mut self, older: bool) {
        let last = self.history.len().checked_sub(1);
        self.recalled = match (self.recalled, older) {
            (None, true) => last,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if Some(i) < last => Some(i + 1),
            _ => None,
        };
        self.input = self.recalled.map_or_else(String::new, |i| self.history[i].clone());
    }
}

/// The console's widgets, made for each game.
#[derive(Component)]
struct ConsoleRoot;
#[derive(Component)]
struct ConsoleLine(usize);
#[derive(Component)]
struct ConsoleInput;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = TextFont { font: (&font.0).into(), font_size: FontSize::Px(TEXT_SIZE), font_smoothing: FontSmoothing::None, ..default() };
    let root = Node {
        position_type: PositionType::Absolute,
        left: Val::Px(8.0),
        bottom: Val::Px(ABOVE_HUD),
        width: Val::Px(WIDTH),
        flex_direction: FlexDirection::Column,
        ..default()
    };
    commands.spawn((root, ConsoleRoot, DespawnOnExit(Screen::InGame))).with_children(|console| {
        for i in 0..SHOWN {
            console.spawn((
                Node { padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)), ..default() },
                BackgroundColor(Color::NONE),
                Visibility::Hidden,
                ConsoleLine(i),
                children![(Text::default(), font.clone(), TextColor(Color::WHITE), TextShadow { offset: Vec2::splat(2.0), color: SHADOW })],
            ));
        }
        console.spawn((
            Node { padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)), margin: UiRect::top(Val::Px(space::INNER)), ..default() },
            Piece::Well.image(),
            Visibility::Hidden,
            ConsoleInput,
            children![(Text::default(), font, TextColor(Color::WHITE))],
        ));
    });
}

/// Opens the console on T or /, and while it is open, edits the line and sends it.
#[allow(clippy::too_many_arguments)]
fn type_line(
    time: Res<Time>,
    mut keys: MessageReader<KeyboardInput>,
    held: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    mut console: ResMut<Console>,
    net: Res<Net>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    settings: Res<Settings>,
    mut composing: ResMut<Composing>,
) {
    let pressed: Vec<&KeyboardInput> = keys.read().filter(|key| key.state == ButtonState::Pressed).collect();
    if !ui.console_open {
        if ui.menu_open || ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || !player.joined {
            return;
        }
        let slash = pressed.iter().any(|key| key.logical_key == Key::Character("/".into()));
        if slash || pressed.iter().any(|key| key.key_code == settings.chat_key.0) {
            // The key that opened it is not typed into it.
            ui.console_open = true;
            console.input = if slash { "/".into() } else { String::new() };
            console.recalled = None;
        }
        return;
    }
    // Shortcuts (copy, select all, …) are not text.
    let shortcut = held.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mut typed = String::new();
    for key in pressed {
        match &key.logical_key {
            Key::Enter => {
                let line = console.input.trim().to_owned();
                if !line.is_empty() {
                    if console.history.last() != Some(&line) {
                        console.history.push(line.clone());
                    }
                    send(&net, &line);
                }
                console.input.clear();
                ui.console_open = false;
                return;
            }
            Key::Backspace => {
                console.input.pop();
            }
            Key::ArrowUp => console.recall(true),
            Key::ArrowDown => console.recall(false),
            Key::Tab => complete(&mut console, &launch, player.admin, time.elapsed_secs()),
            _ => {
                if let Some(text) = key.text.as_ref().filter(|_| !shortcut) {
                    typed.push_str(text);
                }
            }
        }
    }
    // Keys typed, then what the input method committed (Japanese, for one).
    typed.push_str(&composing.take(&typed));
    for c in typed.chars().filter(|c| !c.is_control()) {
        if console.input.chars().count() < MAX_LINE {
            console.input.push(c);
        }
    }
}

/// A command if the line starts with `/`, otherwise chat.
fn send(net: &Net, line: &str) {
    match line.strip_prefix('/') {
        Some(command) => net.command(command),
        None => net.chat(line),
    }
}

/// Completes the command name being typed, or lists the commands it could be.
fn complete(console: &mut Console, launch: &Launch, admin: bool, now: f32) {
    let Some(typed) = console.input.strip_prefix('/').filter(|name| !name.contains(' ')) else { return };
    let names = launch.content.commands.complete(typed, admin);
    match names.as_slice() {
        [] => {}
        [name] => console.input = format!("/{name} "),
        _ => {
            let common = names.iter().skip(1).fold(names[0], |common, name| {
                let length = common.chars().zip(name.chars()).take_while(|(a, b)| a == b).map(|(c, _)| c.len_utf8()).sum();
                &common[..length]
            });
            let list = names.iter().map(|name| format!("/{name}")).collect::<Vec<_>>().join("  ");
            console.input = format!("/{common}");
            console.push(ChatKind::Info, &list, now);
        }
    }
}

/// Sends `--command` lines once the server has welcomed us. Each is a command, with or
/// without its `/`: typed bare (`--command "fill ..."`) it once went out as chat, said and
/// never run, and a capture showed the world without what it asked for.
fn send_pending(mut console: ResMut<Console>, net: Res<Net>, player: Res<LocalPlayer>) {
    if player.joined && !console.pending.is_empty() {
        for line in std::mem::take(&mut console.pending) {
            let line = line.trim();
            net.command(line.strip_prefix('/').unwrap_or(line));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    time: Res<Time>,
    settings: Res<Settings>,
    mut root: Query<&mut Node, With<ConsoleRoot>>,
    ui: Res<UiState>,
    console: Res<Console>,
    mut lines: Query<(&ConsoleLine, &Children, &mut Visibility, &mut BackgroundColor), Without<ConsoleInput>>,
    mut input: Query<(&Children, &mut Visibility), With<ConsoleInput>>,
    mut texts: Query<(&mut Text, &mut TextColor)>,
    composing: Res<Composing>,
) {
    let now = time.elapsed_secs();
    let open = ui.console_open;
    if let Ok(mut node) = root.single_mut() {
        let bottom = Val::Px(super::bottom_offset(&settings) + ABOVE_HUD);
        if node.bottom != bottom {
            node.bottom = bottom;
        }
    }
    let first = console.lines.len().saturating_sub(SHOWN);
    let shown = console.lines.len() - first;
    for (slot, children, mut visibility, mut background) in &mut lines {
        // Slots fill from the bottom: the last slot holds the newest line.
        let line = (slot.0 + shown).checked_sub(SHOWN).map(|i| &console.lines[first + i]);
        // Over an open screen (inventory, character, menu) the chat would run into its words:
        // it waits until the screen closes, or the chat is opened.
        let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.recipes_open;
        let alpha = line.map_or(0.0, |line| if open { 1.0 } else if screen { 0.0 } else { ((line.at + LINGER - now) / FADE).clamp(0.0, 1.0) });
        visibility.set_if_neq(if alpha > 0.0 { Visibility::Inherited } else { Visibility::Hidden });
        let (Some(line), Some(&child)) = (line.filter(|_| alpha > 0.0), children.first()) else { continue };
        background.set_if_neq(BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45 * alpha)));
        if let Ok((mut text, mut color)) = texts.get_mut(child) {
            if text.0 != line.text {
                text.0.clone_from(&line.text);
            }
            let base = match line.kind {
                ChatKind::Chat => Color::WHITE,
                ChatKind::Info => Color::srgb(0.85, 0.88, 0.92),
                ChatKind::Error => Color::srgb(1.0, 0.58, 0.55),
            };
            color.set_if_neq(TextColor(base.with_alpha(alpha)));
        }
    }
    if let Ok((children, mut visibility)) = input.single_mut() {
        // Text the input method is composing shows after the line, before the caret.
        let composing = &composing.preedit;
        visibility.set_if_neq(if open { Visibility::Inherited } else { Visibility::Hidden });
        if let (true, Some(&child)) = (open, children.first())
            && let Ok((mut text, _)) = texts.get_mut(child)
        {
            let caret = if now.fract() < 0.55 { "|" } else { " " };
            let shown = format!("{}{composing}{caret}", console.input);
            if text.0 != shown {
                text.0 = shown;
            }
        }
    }
}
