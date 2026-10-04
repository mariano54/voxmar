//! Creating a world: its name, an optional seed and the game mode.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use voxmar_server::ServerConfig;
use voxmar_server::save::{GameMode, WorldMeta, WorldSave};

use crate::ui::skin::PX;
use crate::ui::widgets::{
    BODY, ButtonStyle, ERROR, MUTED, Pressed, SCRIM, Selected, TextField, UiFont, button_row, label, panel, spawn_heading,
    screen, spawn_button, spawn_text_field, text,
};
use super::{Screen, worlds_dir};
use crate::session::{self, Launch};

pub struct CreatePlugin;

impl Plugin for CreatePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::CreateWorld), spawn)
            .add_systems(Update, (click, show_mode).chain().run_if(in_state(Screen::CreateWorld)));
    }
}

#[derive(Resource)]
struct Mode(GameMode);

#[derive(Component, Clone, Copy)]
enum Action {
    Mode(GameMode),
    Create,
    Back,
}

#[derive(Component)]
struct NameField;
#[derive(Component)]
struct SeedField;
#[derive(Component)]
struct ModeHint;
#[derive(Component)]
struct ErrorText;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((DespawnOnExit(Screen::CreateWorld), screen(SCRIM))).with_children(|root| {
        root.spawn(panel(260.0 * PX)).with_children(|panel| {
            spawn_heading(panel, font, "Create world");
            panel.spawn(label(font, "World name"));
            spawn_text_field(panel, font, "New world", "World name", 32, 0, true, NameField);
            panel.spawn(label(font, "Seed"));
            spawn_text_field(panel, font, "", "Leave empty for a random seed", 40, 1, false, SeedField);
            panel.spawn(label(font, "Game mode"));
            panel.spawn(button_row(0.0)).with_children(|row| {
                spawn_button(row, font, "Survival", ButtonStyle::Secondary, Action::Mode(GameMode::Survival));
                spawn_button(row, font, "Creative", ButtonStyle::Secondary, Action::Mode(GameMode::Creative));
            });
            panel.spawn((ModeHint, Node { margin: UiRect::top(Val::Px(4.0 * PX)), ..default() }, text(font, "", BODY, MUTED)));
            panel.spawn((ErrorText, Node { margin: UiRect::top(Val::Px(4.0 * PX)), ..default() }, text(font, "", BODY, ERROR)));
            panel.spawn(button_row(6.0 * PX)).with_children(|row| {
                spawn_button(row, font, "Create world", ButtonStyle::Primary, Action::Create);
                spawn_button(row, font, "Back", ButtonStyle::Secondary, Action::Back);
            });
        });
    });
    commands.insert_resource(Mode(GameMode::Survival));
}

#[allow(clippy::too_many_arguments)]
fn click(
    mut commands: Commands,
    buttons: Query<(&Action, &Interaction), Pressed>,
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<Mode>,
    name: Query<&TextField, With<NameField>>,
    seed: Query<&TextField, With<SeedField>>,
    launch: Res<Launch>,
    mut errors: Query<&mut Text, With<ErrorText>>,
    mut next: ResMut<NextState<Screen>>,
) {
    let mut actions: Vec<Action> = buttons.iter().filter(|(_, i)| **i == Interaction::Pressed).map(|(a, _)| *a).collect();
    if keys.just_pressed(KeyCode::Enter) {
        actions.push(Action::Create);
    }
    for action in actions {
        match action {
            Action::Mode(chosen) => mode.0 = chosen,
            Action::Back => next.set(Screen::Worlds),
            Action::Create => {
                let (Ok(name), Ok(seed)) = (name.single(), seed.single()) else { continue };
                let created = create(name.value.trim(), parse_seed(&seed.value).unwrap_or_else(random_seed), mode.0)
                    .and_then(|dir| launch.singleplayer(ServerConfig { world: Some(dir), ..ServerConfig::default() }));
                match created {
                    Ok(net) => session::play(&mut commands, &mut next, net),
                    Err(error) => {
                        if let Ok(mut text) = errors.single_mut() {
                            text.0 = error;
                        }
                    }
                }
            }
        }
    }
}

/// Creates the world's directory and `world.ron`.
fn create(name: &str, seed: u64, mode: GameMode) -> Result<PathBuf, String> {
    if name.is_empty() {
        return Err("Give your world a name.".into());
    }
    let root = worlds_dir().ok_or("There is no configuration directory to save worlds in.")?;
    let dir = new_world_dir(&root, name);
    WorldSave::new(&dir).create(&WorldMeta::new(name, seed, mode)).map_err(|e| format!("Could not create the world: {e}"))?;
    info!("created world {name} in {}", dir.display());
    Ok(dir)
}

/// Marks the chosen mode and explains it.
fn show_mode(mut commands: Commands, mode: Res<Mode>, buttons: Query<(Entity, &Action)>, mut hint: Query<&mut Text, With<ModeHint>>) {
    if !mode.is_changed() {
        return;
    }
    for (entity, action) in &buttons {
        let Action::Mode(choice) = *action else { continue };
        if choice == mode.0 {
            commands.entity(entity).insert(Selected);
        } else {
            commands.entity(entity).remove::<Selected>();
        }
    }
    if let Ok(mut hint) = hint.single_mut() {
        hint.0 = match mode.0 {
            GameMode::Survival => "Explore, mine and build on foot. Falls hurt.",
            GameMode::Creative => "Fly anywhere, as fast as you like.",
        }
        .into();
    }
}

/// The seed for what was typed: a number as it is, any other text hashed, and nothing
/// for an empty field.
fn parse_seed(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(number) = text.parse::<u64>() {
        return Some(number);
    }
    if let Ok(number) = text.parse::<i64>() {
        return Some(number as u64);
    }
    // FNV-1a, so the same text gives the same world everywhere.
    Some(text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3)))
}

fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    // SplitMix64, to spread the clock's few changing bits over the whole seed.
    let mut z = nanos ^ ((std::process::id() as u64) << 32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A new directory in `root` for a world called `name`: the name made safe as a file name,
/// numbered if already taken.
fn new_world_dir(root: &Path, name: &str) -> PathBuf {
    let safe: String =
        name.chars().take(48).map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let base = match safe.trim() {
        "" => "world",
        trimmed => trimmed,
    };
    (1..)
        .map(|n| if n == 1 { root.join(base) } else { root.join(format!("{base} ({n})")) })
        .find(|dir| !dir.exists())
        .expect("some numbered name is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_from_numbers_text_or_nothing() {
        assert_eq!(parse_seed(" 42 "), Some(42));
        assert_eq!(parse_seed("-1"), Some(u64::MAX));
        assert_eq!(parse_seed(""), None);
        assert_eq!(parse_seed("hello"), parse_seed("hello"));
        assert_ne!(parse_seed("hello"), parse_seed("hellp"));
        assert_ne!(random_seed(), 0);
    }

    #[test]
    fn world_directories_are_safe_and_unique() {
        let root = std::env::temp_dir().join(format!("voxmar-worlds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(new_world_dir(&root, "My World"), root.join("My World"));
        assert_eq!(new_world_dir(&root, "../a/b:c"), root.join("___a_b_c"));
        assert_eq!(new_world_dir(&root, "  "), root.join("world"));
        std::fs::create_dir_all(root.join("Taken")).unwrap();
        assert_eq!(new_world_dir(&root, "Taken"), root.join("Taken (2)"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
