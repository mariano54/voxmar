//! Multiplayer: joining a server by address, with the last few servers remembered in
//! `voxmar/servers.ron` in the platform's configuration directory.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, bounded};
use voxmar_protocol::{ClientConnection, DEFAULT_PORT, tcp};

use crate::lang::Locale;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    BODY, ButtonStyle, Disabled, ERROR, MUTED, Pressed, SCRIM, SOFT, TextField, UiFont, button_row, spawn_heading,
    label, panel, screen, spawn_button, spawn_text_field, text,
};
use crate::ui::widgets::space;
use super::{Notice, Screen};
use crate::session::{self, Launch};
use crate::settings::config_dir;

const RECENT_SERVERS: usize = 5;

pub struct JoinPlugin;

impl Plugin for JoinPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::Join), spawn)
            .add_systems(OnExit(Screen::Join), |mut commands: Commands| commands.remove_resource::<Connecting>())
            .add_systems(Update, (pick_recent, click, finish_connecting, show_status).chain().run_if(in_state(Screen::Join)));
    }
}

/// A connection attempt running on a background thread, so the menu stays responsive.
#[derive(Resource)]
pub struct Connecting {
    address: String,
    result: Receiver<std::io::Result<ClientConnection>>,
}

/// Starts connecting to `address` (`host` or `host:port`).
pub fn connect(address: &str) -> Connecting {
    let address = with_port(address.trim());
    let (tx, result) = bounded(1);
    let target = address.clone();
    std::thread::spawn(move || {
        let _ = tx.send(tcp::connect(target.as_str()));
    });
    Connecting { address, result }
}

/// `address` with the default port added if it has none.
pub(crate) fn with_port(address: &str) -> String {
    if address.parse::<SocketAddr>().is_ok() {
        return address.to_owned();
    }
    if let Ok(ip) = address.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
        return SocketAddr::new(ip, DEFAULT_PORT).to_string();
    }
    if address.contains(':') { address.to_owned() } else { format!("{address}:{DEFAULT_PORT}") }
}

fn recent_path() -> Option<PathBuf> {
    Some(config_dir()?.join("servers.ron"))
}

/// Recently joined servers, most recent first.
fn load_recent() -> Vec<String> {
    let source = recent_path().and_then(|path| std::fs::read_to_string(path).ok());
    source.and_then(|source| ron::from_str(&source).ok()).unwrap_or_default()
}

fn remember(address: &str) {
    let mut recent = load_recent();
    recent.retain(|known| known != address);
    recent.insert(0, address.to_owned());
    recent.truncate(RECENT_SERVERS);
    let Some(path) = recent_path() else { return };
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(path.parent().expect("servers.ron is in a directory"))?;
        std::fs::write(&path, ron::to_string(&recent)?)?;
        Ok(())
    };
    if let Err(e) = write() {
        warn!("could not save {}: {e}", path.display());
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Action {
    Join,
    Back,
}

#[derive(Component)]
struct AddressField;
#[derive(Component)]
struct RecentServer(String);
#[derive(Component)]
struct Status;

fn spawn(mut commands: Commands, font: Res<UiFont>, connecting: Option<Res<Connecting>>, mut notice: ResMut<Notice>) {
    let font = &font.0;
    let recent = load_recent();
    let address = connecting.map(|c| c.address.clone()).or_else(|| recent.first().cloned()).unwrap_or_default();
    commands.spawn((DespawnOnExit(Screen::Join), screen(SCRIM))).with_children(|root| {
        root.spawn(panel(260.0 * PX)).with_children(|panel| {
            spawn_heading(panel, font, "menu.multiplayer");
            panel.spawn(label(font, "join.address"));
            spawn_text_field(panel, font, &address, "join.address.placeholder", 80, 0, true, AddressField);
            if !recent.is_empty() {
                panel.spawn(label(font, "join.recent"));
                let list = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::INNER), ..default() };
                panel.spawn(list).with_children(|list| {
                    for address in &recent {
                        let node = Node {
                            height: Val::Px(18.0 * PX),
                            padding: UiRect::horizontal(Val::Px(space::GAP)),
                            align_items: AlignItems::Center,
                            ..default()
                        };
                        list.spawn((Button, RecentServer(address.clone()), node, Piece::Button.image()))
                            .with_child(text(font, address.clone(), BODY, SOFT));
                    }
                });
            }
            let message = notice.0.take().unwrap_or_default();
            panel.spawn((Status, Node { margin: UiRect::top(Val::Px(space::GAP)), ..default() }, text(font, message, BODY, ERROR)));
            panel.spawn(button_row(6.0 * PX)).with_children(|row| {
                spawn_button(row, font, "join.join", ButtonStyle::Primary, Action::Join);
                spawn_button(row, font, "menu.back", ButtonStyle::Secondary, Action::Back);
            });
        });
    });
}

/// Clicking a recent server fills in its address; clicking it again joins it.
fn pick_recent(
    mut commands: Commands,
    mut servers: Query<(&RecentServer, Ref<Interaction>, &mut ImageNode)>,
    mut field: Query<&mut TextField, With<AddressField>>,
    connecting: Option<Res<Connecting>>,
) {
    for (server, interaction, mut image) in &mut servers {
        let piece = if *interaction == Interaction::None { Piece::Button } else { Piece::ButtonHover };
        if image.image != piece.handle() {
            image.image = piece.handle();
        }
        if *interaction != Interaction::Pressed || !interaction.is_changed() || connecting.is_some() {
            continue;
        }
        let Ok(mut field) = field.single_mut() else { continue };
        if field.value == server.0 {
            commands.insert_resource(connect(&server.0));
        } else {
            field.value.clone_from(&server.0);
        }
    }
}

fn click(
    mut commands: Commands,
    buttons: Query<(&Action, &Interaction), Pressed>,
    keys: Res<ButtonInput<KeyCode>>,
    field: Query<&TextField, With<AddressField>>,
    connecting: Option<Res<Connecting>>,
    locale: Res<Locale>,
    mut status: Query<&mut Text, With<Status>>,
    mut next: ResMut<NextState<Screen>>,
) {
    let mut actions: Vec<Action> = buttons.iter().filter(|(_, i)| **i == Interaction::Pressed).map(|(a, _)| *a).collect();
    if keys.just_pressed(KeyCode::Enter) {
        actions.push(Action::Join);
    }
    for action in actions {
        match action {
            Action::Back => next.set(Screen::Title),
            Action::Join if connecting.is_none() => {
                let Ok(field) = field.single() else { continue };
                if field.value.trim().is_empty() {
                    if let Ok(mut status) = status.single_mut() {
                        status.0 = locale.get("join.error.no_address").into();
                    }
                } else {
                    commands.insert_resource(connect(&field.value));
                }
            }
            Action::Join => {}
        }
    }
}

/// Joins the server once connected, or explains why that failed.
fn finish_connecting(
    mut commands: Commands,
    connecting: Option<Res<Connecting>>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    mut status: Query<&mut Text, With<Status>>,
    mut next: ResMut<NextState<Screen>>,
) {
    let Some(connecting) = connecting else { return };
    let Ok(result) = connecting.result.try_recv() else { return };
    commands.remove_resource::<Connecting>();
    match result {
        Ok(connection) => {
            remember(&connecting.address);
            session::play(&mut commands, &mut next, launch.multiplayer(connection, &connecting.address));
        }
        Err(e) => {
            let reason = match e.kind() {
                std::io::ErrorKind::ConnectionRefused => locale.get("join.error.refused").to_owned(),
                std::io::ErrorKind::TimedOut => locale.get("join.error.timed_out").to_owned(),
                _ => e.to_string(),
            };
            if let Ok(mut status) = status.single_mut() {
                status.0 = locale.format("join.error.failed", &[("address", &connecting.address), ("reason", &reason)]);
            }
        }
    }
}

/// Says that a connection is under way, and disables Join meanwhile.
fn show_status(
    mut commands: Commands,
    connecting: Option<Res<Connecting>>,
    mut shown: Local<Option<bool>>,
    locale: Res<Locale>,
    mut status: Query<(&mut Text, &mut TextColor), With<Status>>,
    join: Query<(Entity, &Action)>,
) {
    let busy = connecting.is_some();
    if *shown == Some(busy) {
        return;
    }
    *shown = Some(busy);
    let Ok((mut text, mut color)) = status.single_mut() else { return };
    if let Some(connecting) = &connecting {
        text.0 = locale.format("join.connecting", &[("address", &connecting.address)]);
        color.0 = MUTED;
    } else {
        // What remains is why the last attempt failed.
        color.0 = ERROR;
    }
    for (entity, action) in &join {
        if *action == Action::Join {
            if busy {
                commands.entity(entity).insert(Disabled);
            } else {
                commands.entity(entity).remove::<Disabled>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::with_port;

    #[test]
    fn addresses_get_the_default_port() {
        assert_eq!(with_port("example.org"), "example.org:25665");
        assert_eq!(with_port("example.org:1234"), "example.org:1234");
        assert_eq!(with_port("10.0.0.2"), "10.0.0.2:25665");
        assert_eq!(with_port("::1"), "[::1]:25665");
        assert_eq!(with_port("[::1]"), "[::1]:25665");
        assert_eq!(with_port("[::1]:99"), "[::1]:99");
    }
}
