//! Singleplayer: the saved worlds, most recently played first, to play, create or delete.

use std::path::PathBuf;

use bevy::prelude::*;
use voxmar_server::ServerConfig;
use voxmar_server::save::{GameMode, WorldMeta, WorldSave, list_worlds, unix_time};

use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    BODY, ButtonStyle, Disabled, ERROR, GOLD, MUTED, Pressed, SCRIM, SOFT, ScrollList, TEXT, UiFont, button_row, panel, screen,
    shadowed, spawn_button, spawn_heading, text,
};
use super::{Notice, Screen};
use crate::session::{self, Launch};
use crate::settings::config_dir;

pub struct WorldsPlugin;

impl Plugin for WorldsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::Worlds), spawn)
            .add_systems(Update, (pick_world, click, show_state).chain().run_if(in_state(Screen::Worlds)));
    }
}

/// `voxmar/worlds` in the platform's configuration directory.
pub fn worlds_dir() -> Option<PathBuf> {
    Some(config_dir()?.join("worlds"))
}

#[derive(Resource)]
struct WorldList {
    worlds: Vec<(WorldSave, WorldMeta)>,
    selected: Option<usize>,
    /// Asking whether to delete the selected world.
    confirming: bool,
    /// Row and time of the last click, to open a world with a double click.
    last_click: Option<(usize, f32)>,
    /// A world was double-clicked.
    open: bool,
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Action {
    Play,
    Create,
    Delete,
    Back,
    ConfirmDelete,
    CancelDelete,
}

#[derive(Component)]
struct WorldRow(usize);
#[derive(Component)]
struct Rows;
/// The usual buttons, or the delete confirmation that replaces them.
#[derive(Component)]
struct Confirmation(bool);
#[derive(Component)]
struct ConfirmText;
#[derive(Component)]
struct ErrorText;

fn spawn(mut commands: Commands, font: Res<UiFont>, mut notice: ResMut<Notice>) {
    let font = &font.0;
    let worlds = worlds_dir().map(|dir| list_worlds(&dir)).unwrap_or_default();
    let selected = (!worlds.is_empty()).then_some(0);
    commands.spawn((DespawnOnExit(Screen::Worlds), screen(SCRIM))).with_children(|root| {
        root.spawn(panel(340.0 * PX)).with_children(|panel| {
            spawn_heading(panel, font, "Singleplayer");
            let list = Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0 * PX),
                max_height: Val::Px(160.0 * PX),
                margin: UiRect::bottom(Val::Px(6.0 * PX)),
                overflow: Overflow::scroll_y(),
                ..default()
            };
            panel.spawn((Rows, ScrollList, list)).with_children(|list| spawn_rows(list, font, &worlds));
            let error = notice.0.take().unwrap_or_default();
            panel.spawn((ErrorText, Node { margin: UiRect::bottom(Val::Px(3.0 * PX)), ..default() }, text(font, error, BODY, ERROR)));

            let normal = Node { flex_direction: FlexDirection::Column, ..default() };
            panel.spawn((Confirmation(false), normal)).with_children(|column| {
                column.spawn(button_row(0.0)).with_children(|row| {
                    spawn_button(row, font, "Play", ButtonStyle::Primary, Action::Play);
                    spawn_button(row, font, "Create new world", ButtonStyle::Secondary, Action::Create);
                });
                column.spawn(button_row(4.0 * PX)).with_children(|row| {
                    spawn_button(row, font, "Delete", ButtonStyle::Secondary, Action::Delete);
                    spawn_button(row, font, "Back", ButtonStyle::Secondary, Action::Back);
                });
            });
            let confirm = Node { flex_direction: FlexDirection::Column, display: Display::None, ..default() };
            panel.spawn((Confirmation(true), confirm)).with_children(|column| {
                column.spawn((ConfirmText, text(font, "", BODY, SOFT)));
                column.spawn(button_row(5.0 * PX)).with_children(|row| {
                    spawn_button(row, font, "Delete forever", ButtonStyle::Danger, Action::ConfirmDelete);
                    spawn_button(row, font, "Cancel", ButtonStyle::Secondary, Action::CancelDelete);
                });
            });
        });
    });
    commands.insert_resource(WorldList { worlds, selected, confirming: false, last_click: None, open: false });
}

fn spawn_rows(list: &mut ChildSpawnerCommands, font: &Handle<Font>, worlds: &[(WorldSave, WorldMeta)]) {
    if worlds.is_empty() {
        let node = Node { padding: UiRect::vertical(Val::Px(14.0 * PX)), justify_content: JustifyContent::Center, ..default() };
        list.spawn(node).with_child(text(font, "No worlds yet. Create one to start playing.", BODY, MUTED));
        return;
    }
    let now = unix_time();
    for (index, (_, meta)) in worlds.iter().enumerate() {
        let node = Node {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            row_gap: Val::Px(2.0 * PX),
            padding: UiRect::new(Val::Px(6.0 * PX), Val::Px(6.0 * PX), Val::Px(5.0 * PX), Val::Px(6.0 * PX)),
            ..default()
        };
        let mode = match meta.mode {
            GameMode::Survival => "Survival",
            GameMode::Creative => "Creative",
        };
        let details = format!("{mode} · Seed {} · Played {}", meta.seed, ago(meta.last_played, now));
        list.spawn((Button, WorldRow(index), node, Piece::Button.image())).with_children(|row| {
            row.spawn(shadowed(font, meta.name.clone(), BODY, TEXT));
            row.spawn(text(font, details, BODY, MUTED));
        });
    }
}

/// How long ago `then` was, in words.
fn ago(then: u64, now: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let seconds = now.saturating_sub(then);
    let (count, unit) = match seconds {
        s if s < MINUTE => return "just now".into(),
        s if s < HOUR => (s / MINUTE, "minute"),
        s if s < DAY => (s / HOUR, "hour"),
        s if s < 2 * DAY => return "yesterday".into(),
        s if s < 30 * DAY => (s / DAY, "day"),
        s if s < 365 * DAY => (s / (30 * DAY), "month"),
        s => (s / (365 * DAY), "year"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

/// Selects a world on click; a second click soon after plays it.
fn pick_world(time: Res<Time>, rows: Query<(&WorldRow, &Interaction), Changed<Interaction>>, mut list: ResMut<WorldList>) {
    for (row, interaction) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let now = time.elapsed_secs();
        list.open = list.last_click.is_some_and(|(last, at)| last == row.0 && now - at < 0.4);
        list.last_click = Some((row.0, now));
        list.selected = Some(row.0);
        list.confirming = false;
    }
}

#[allow(clippy::too_many_arguments)]
fn click(
    mut commands: Commands,
    buttons: Query<(&Action, &Interaction), Pressed>,
    keys: Res<ButtonInput<KeyCode>>,
    mut list: ResMut<WorldList>,
    launch: Res<Launch>,
    font: Res<UiFont>,
    row_lists: Query<Entity, With<Rows>>,
    mut errors: Query<&mut Text, With<ErrorText>>,
    mut next: ResMut<NextState<Screen>>,
) {
    let mut actions: Vec<Action> = buttons.iter().filter(|(_, i)| **i == Interaction::Pressed).map(|(a, _)| *a).collect();
    let open = list.open;
    if open {
        list.open = false;
    }
    if open || keys.just_pressed(KeyCode::Enter) {
        actions.push(if list.confirming { Action::ConfirmDelete } else { Action::Play });
    }
    let mut error = None;
    for action in actions {
        let selected = list.selected.and_then(|i| list.worlds.get(i)).cloned();
        match (action, selected) {
            (Action::Play, Some((save, _))) => {
                let config = ServerConfig { world: Some(save.dir().to_owned()), ..ServerConfig::default() };
                match launch.singleplayer(config) {
                    Ok(net) => session::play(&mut commands, &mut next, net),
                    Err(e) => error = Some(e),
                }
            }
            (Action::Create, _) => next.set(Screen::CreateWorld),
            (Action::Delete, Some(_)) => list.confirming = true,
            (Action::Back, _) => next.set(Screen::Title),
            (Action::ConfirmDelete, Some((save, meta))) => {
                list.confirming = false;
                match std::fs::remove_dir_all(save.dir()) {
                    Ok(()) => {
                        info!("deleted world {} in {}", meta.name, save.dir().display());
                        let index = list.selected.unwrap_or(0);
                        list.worlds.remove(index);
                        list.selected = (!list.worlds.is_empty()).then(|| index.min(list.worlds.len() - 1));
                        for entity in &row_lists {
                            commands.entity(entity).despawn_children().with_children(|rows| spawn_rows(rows, &font.0, &list.worlds));
                        }
                    }
                    Err(e) => error = Some(format!("Could not delete {}: {e}", meta.name)),
                }
            }
            (Action::CancelDelete, _) => list.confirming = false,
            _ => {}
        }
    }
    if let Some(error) = error
        && let Ok(mut text) = errors.single_mut()
    {
        text.0 = error;
    }
}

/// Highlights the selected world, enables the buttons that need one, and swaps in the
/// delete confirmation.
fn show_state(
    mut commands: Commands,
    list: Res<WorldList>,
    mut rows: Query<(&WorldRow, &Interaction, &mut ImageNode, &Children)>,
    mut names: Query<&mut TextColor>,
    buttons: Query<(Entity, &Action)>,
    mut groups: Query<(&Confirmation, &mut Node)>,
    mut confirm_text: Query<&mut Text, With<ConfirmText>>,
) {
    for (row, interaction, mut image, children) in &mut rows {
        let selected = list.selected == Some(row.0);
        let piece = match (selected, interaction) {
            (_, Interaction::Hovered | Interaction::Pressed) => Piece::ButtonHover,
            (true, Interaction::None) => Piece::ButtonSelected,
            (false, Interaction::None) => Piece::Button,
        };
        if image.image != piece.handle() {
            image.image = piece.handle();
        }
        if let Some(mut color) = children.first().and_then(|&name| names.get_mut(name).ok()) {
            color.set_if_neq(TextColor(if selected { GOLD } else { TEXT }));
        }
    }
    if !list.is_changed() {
        return;
    }
    for (entity, action) in &buttons {
        if matches!(action, Action::Play | Action::Delete) {
            if list.selected.is_some() {
                commands.entity(entity).remove::<Disabled>();
            } else {
                commands.entity(entity).insert(Disabled);
            }
        }
    }
    for (group, mut node) in &mut groups {
        node.display = if group.0 == list.confirming { Display::Flex } else { Display::None };
    }
    if let Ok(mut text) = confirm_text.single_mut()
        && let Some((_, meta)) = list.selected.and_then(|i| list.worlds.get(i))
    {
        text.0 = format!("Delete “{}”? This cannot be undone.", meta.name);
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn times_read_naturally() {
        let now = 1_000_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 60, now), "1 minute ago");
        assert_eq!(ago(now - 3 * 3600 - 5, now), "3 hours ago");
        assert_eq!(ago(now - 30 * 3600, now), "yesterday");
        assert_eq!(ago(now - 5 * 86400, now), "5 days ago");
        assert_eq!(ago(now - 400 * 86400, now), "1 year ago");
        assert_eq!(ago(now + 100, now), "just now");
    }
}
