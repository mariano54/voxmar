//! Connection to the (local or remote) server: applies replicated state and sends the
//! player's input and actions.

use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_protocol::{
    Action, ClientConnection, ClientMessage, PlayerId, PlayerInput, PlayerView, ServerMessage, TICKS_PER_SECOND,
};
use voxmar_server::ServerHandle;

use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::world::ClientWorld;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RemotePlayers>()
            .add_systems(PreUpdate, receive)
            .add_systems(PostUpdate, (send_input, sync_remote_players));
    }
}

#[derive(Resource)]
pub struct Net {
    connection: ClientConnection,
    /// The integrated server, when playing singleplayer. Dropping it stops the server.
    _server: Option<ServerHandle>,
    pub status: Option<String>,
    input_timer: f32,
}

impl Net {
    pub fn new(connection: ClientConnection, server: Option<ServerHandle>, name: String, content: u64) -> Self {
        connection.send(ClientMessage::Hello { name, protocol: voxmar_protocol::PROTOCOL_VERSION, content });
        Self { connection, _server: server, status: Some("Connecting…".into()), input_timer: 0.0 }
    }

    pub fn act(&self, action: Action) {
        self.connection.send(ClientMessage::Action(action));
    }
}

#[derive(Resource, Default)]
pub struct RemotePlayers {
    players: FxHashMap<PlayerId, (PlayerView, Option<Entity>)>,
}

fn receive(
    mut commands: Commands,
    mut net: ResMut<Net>,
    mut world: ResMut<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut cycle: ResMut<DayCycle>,
    mut remote: ResMut<RemotePlayers>,
) {
    loop {
        let message = match net.connection.try_recv() {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(()) => {
                if net.status.is_none() {
                    net.status = Some("Disconnected from server".into());
                }
                break;
            }
        };
        match message {
            ServerMessage::Welcome { position, allow_flight, .. } => {
                player.position = position;
                player.allow_flight = allow_flight;
                player.joined = true;
                net.status = None;
            }
            ServerMessage::Rejected { reason } => net.status = Some(format!("Rejected: {reason}")),
            ServerMessage::Column { pos, column } => world.insert_column(pos, column),
            ServerMessage::Unload { pos } => world.remove_column(pos, &mut commands),
            ServerMessage::BlockChanged { pos, block } => {
                if world.block(pos) != Some(block) {
                    world.set_block(pos, block);
                }
            }
            ServerMessage::PlayerState { health, inventory } => {
                player.health = health;
                player.inventory = inventory;
                if health > 0 {
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
            None => *entity = Some(commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), transform)).id()),
        }
    }
}
