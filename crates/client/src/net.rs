//! Connection to the (local or remote) server: applies replicated state and sends the
//! player's input and actions. Present while in game.

use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_protocol::{
    ClientConnection, ClientMessage, PlayerId, PlayerInput, PlayerView, ServerMessage, TICKS_PER_SECOND,
};
use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_server::ServerHandle;

use crate::menu::{Notice, Screen};
use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::render::lod::Lod;
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        let in_game = in_state(Screen::InGame);
        app.init_resource::<RemotePlayers>()
            .add_systems(PreUpdate, receive.run_if(in_game.clone()))
            .add_systems(PostUpdate, (send_input, sync_remote_players).run_if(in_game));
    }
}

#[derive(Resource)]
pub struct Net {
    // Declared first so it closes before the server stops.
    connection: ClientConnection,
    /// The integrated server, when playing singleplayer. Dropping it stops the server,
    /// which saves the world.
    server: Option<ServerHandle>,
    input_timer: f32,
}

impl Net {
    pub fn new(connection: ClientConnection, server: Option<ServerHandle>, name: String, content: u64) -> Self {
        connection.send(ClientMessage::Hello { name, protocol: voxmar_protocol::PROTOCOL_VERSION, content });
        Self { connection, server, input_timer: 0.0 }
    }

    /// Whether this is a remote server rather than singleplayer's own.
    pub fn is_remote(&self) -> bool {
        self.server.is_none()
    }

    pub fn propose(&self, action: ActionId, target: ActionTarget) {
        self.connection.send(ClientMessage::Propose { action, target });
    }

    pub fn cancel(&self) {
        self.connection.send(ClientMessage::Cancel);
    }

    pub fn send_settings(&self, view_distance: u8, fly_speed: f32) {
        self.connection.send(ClientMessage::Settings { view_distance, fly_speed });
    }
}

#[derive(Resource, Default)]
pub struct RemotePlayers {
    players: FxHashMap<PlayerId, (PlayerView, Option<Entity>)>,
}

/// Applies what the server sent. If it refuses us or goes away, returns to the menus with
/// the reason.
#[allow(clippy::too_many_arguments)]
fn receive(
    mut commands: Commands,
    net: Res<Net>,
    mut world: ResMut<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut cycle: ResMut<DayCycle>,
    mut remote: ResMut<RemotePlayers>,
    mut lod: ResMut<Lod>,
    mut notice: ResMut<Notice>,
    mut next: ResMut<NextState<Screen>>,
    mut weather: ResMut<ClientWeather>,
) {
    let menu = if net.is_remote() { Screen::Join } else { Screen::Title };
    loop {
        let message = match net.connection.try_recv() {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(_) => {
                let reason = if net.is_remote() { "Lost the connection to the server." } else { "The world stopped unexpectedly." };
                notice.0 = Some(reason.into());
                next.set(menu);
                break;
            }
        };
        match message {
            ServerMessage::Welcome { position, yaw, flying, allow_flight, admin, seed, .. } => {
                lod.start(seed, world.registry.clone());
                player.position = position;
                player.yaw = yaw;
                player.flying = flying;
                player.allow_flight = allow_flight;
                player.admin = admin;
                player.joined = true;
            }
            ServerMessage::Rejected { reason } => {
                notice.0 = Some(format!("The server turned you away: {reason}."));
                next.set(menu);
                break;
            }
            ServerMessage::Column { pos, column } => world.insert_column(pos, column),
            ServerMessage::Unload { pos } => world.remove_column(pos),
            ServerMessage::BlockChanged { pos, block } => {
                if world.block(pos) != Some(block) {
                    world.set_block(pos, block);
                }
            }
            ServerMessage::PlayerState { health, inventory } => {
                player.health = health.current;
                player.inventory = inventory;
                if health.alive() {
                    player.dead = false;
                }
            }
            ServerMessage::Teleport { position } => {
                player.position = position;
                player.velocity = Vec3::ZERO;
            }
            ServerMessage::Players { players } => {
                let seen: Vec<PlayerId> = players.iter().map(|p| p.id).collect();
                for view in players {
                    let entry = remote.players.entry(view.id).or_insert((view.clone(), None));
                    entry.0 = view;
                }
                remote.players.retain(|id, (_, entity)| {
                    let keep = seen.contains(id);
                    if !keep && let Some(entity) = entity {
                        commands.entity(*entity).despawn();
                    }
                    keep
                });
            }
            ServerMessage::Time { ticks } => cycle.ticks = ticks as f64,
            ServerMessage::Weather(report) => weather.receive(report),
            ServerMessage::Lightning { at } => weather.lightning(at.distance(player.position.xz())),
            ServerMessage::Died => player.dead = true,
        }
    }
}

fn send_input(time: Res<Time>, mut net: ResMut<Net>, player: Res<LocalPlayer>) {
    net.input_timer += time.delta_secs();
    let interval = 1.0 / TICKS_PER_SECOND as f32;
    if net.input_timer < interval || !player.joined {
        return;
    }
    net.input_timer %= interval;
    net.connection.send(ClientMessage::Input(PlayerInput {
        position: player.position,
        yaw: player.yaw,
        pitch: player.pitch,
        selected_slot: player.selected as u8,
        flying: player.flying,
    }));
}

/// Other players are drawn as simple boxes.
fn sync_remote_players(
    mut commands: Commands,
    mut remote: ResMut<RemotePlayers>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut look: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    let (mesh, material) = look
        .get_or_insert_with(|| {
            let size = voxmar_engine::physics::player::WIDTH;
            (
                meshes.add(Cuboid::new(size, voxmar_engine::physics::player::HEIGHT, size)),
                materials.add(StandardMaterial { base_color: Color::srgb(0.85, 0.35, 0.3), unlit: true, ..default() }),
            )
        })
        .clone();
    for (view, entity) in remote.players.values_mut() {
        let transform = Transform::from_translation(view.position + Vec3::Y * voxmar_engine::physics::player::HEIGHT / 2.0)
            .with_rotation(Quat::from_rotation_y(view.yaw));
        match entity {
            Some(entity) => {
                if let Ok(mut t) = transforms.get_mut(*entity) {
                    *t = transform;
                }
            }
            None => {
                let body = (Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), transform, DespawnOnExit(Screen::InGame));
                *entity = Some(commands.spawn(body).id());
            }
        }
    }
}
