//! A play session: starting the integrated server or joining a remote one, and clearing
//! everything about the world away when the player returns to the menus, so another
//! world can be played in the same process.

use bevy::prelude::*;
use voxmar_engine::content::Content;
use voxmar_protocol::{ClientConnection, DEFAULT_PORT, local_pair, tcp};
use voxmar_server::ServerConfig;

use crate::interact::Target;
use crate::menu::Screen;
use crate::net::{Net, RemotePlayers};
use crate::player::{LocalPlayer, UiState};
use crate::render::DayCycle;
use crate::render::lod::WorldGenerator;
use crate::settings::Settings;
use crate::world::ClientWorld;

pub struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnExit(Screen::InGame), end_session).add_systems(Last, stop_server_on_exit);
    }
}

/// Who the player is and what starting or joining a game takes.
#[derive(Resource, Clone)]
pub struct Launch {
    pub name: String,
    /// Also open singleplayer worlds to the network.
    pub host: bool,
    pub content: Content,
}

impl Launch {
    /// Starts an integrated server for the world `config` describes and joins it.
    pub fn singleplayer(&self, config: ServerConfig) -> Result<Net, String> {
        let server = voxmar_server::spawn(config, self.content.clone()).map_err(|e| format!("Could not open the world: {e}"))?;
        if self.host {
            match tcp::listen(("0.0.0.0", DEFAULT_PORT), server.connections()) {
                Ok(addr) => info!("open to the network on {addr}"),
                Err(e) => warn!("could not open the world to the network: {e}"),
            }
        }
        let (client, server_side) = local_pair();
        server.connections().send(server_side).map_err(|_| "The world's server stopped".to_owned())?;
        Ok(Net::new(client, Some(server), self.name.clone(), self.content.fingerprint(), None))
    }

    /// Joins the server at `address` (`host:port`) over a new connection.
    pub fn multiplayer(&self, connection: ClientConnection, address: &str) -> Net {
        Net::new(connection, None, self.name.clone(), self.content.fingerprint(), Some(address))
    }
}

/// Starts playing over `net`.
pub fn play(commands: &mut Commands, next: &mut NextState<Screen>, net: Net) {
    commands.insert_resource(net);
    next.set(Screen::InGame);
}

/// Leaves the world: stops the integrated server (which saves the world), and removes
/// the world's chunks, distant terrain, players and per-session state.
fn end_session(
    mut commands: Commands,
    settings: Res<Settings>,
    mut world: ResMut<ClientWorld>,
    mut generator: ResMut<WorldGenerator>,
    mut cycle: ResMut<DayCycle>,
) {
    commands.remove_resource::<Net>();
    world.reset(settings.view_distance);
    generator.clear();
    cycle.underwater = false;
    commands.insert_resource(LocalPlayer::default());
    commands.insert_resource(UiState::default());
    commands.insert_resource(Target::default());
    commands.insert_resource(RemotePlayers::default());
}

/// Quitting the game, from the menu or by closing the window, stops the integrated server
/// first, so the world is saved.
fn stop_server_on_exit(mut commands: Commands, mut exit: MessageReader<AppExit>) {
    if exit.read().next().is_some() {
        commands.remove_resource::<Net>();
    }
}
