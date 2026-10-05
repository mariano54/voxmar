//! Connection to the (local or remote) server: applies replicated state and sends the
//! player's input and actions. Present while in game.

use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_protocol::{
    ClientConnection, ClientMessage, PlayerId, PlayerInput, PlayerView, ServerMessage, TICKS_PER_SECOND,
};
use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::RenderKind;
use voxmar_server::ServerHandle;

use crate::drops::Drops;
use crate::hud::skills::SkillToasts;
use crate::session::Launch;
use crate::breaking::{BlockBroken, SHOWN_WITHIN, shows_breaking};
use crate::hud::console::Console;
use crate::menu::{Notice, Screen};
use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::render::lod::WorldGenerator;
use crate::weather::ClientWeather;
use crate::weather::atmosphere::ClientAtmosphere;
use crate::world::ClientWorld;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        let in_game = in_state(Screen::InGame);
        app.init_resource::<RemotePlayers>()
            .init_resource::<Clock>()
            .add_systems(PreUpdate, receive.run_if(in_game.clone()))
            .add_systems(Update, advance_day.run_if(in_game.clone()))
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

    /// The integrated server, when playing singleplayer.
    pub fn server(&self) -> Option<&ServerHandle> {
        self.server.as_ref()
    }

    pub fn propose(&self, action: ActionId, target: ActionTarget) {
        self.connection.send(ClientMessage::Propose { action, target });
    }

    /// Proposes an action aimed into the world (mining, placing, throwing), telling the
    /// server where the player is and looks first: inputs go only twenty times a second,
    /// and the server judges reach and aim from the last it heard, which while sprinting or
    /// flying could be well behind.
    pub fn propose_from(&self, player: &LocalPlayer, action: ActionId, target: ActionTarget) {
        self.connection.send(ClientMessage::Input(input(player)));
        self.propose(action, target);
    }

    pub fn cancel(&self) {
        self.connection.send(ClientMessage::Cancel);
    }

    /// Tells the server the action in progress shows done here (see `ClientMessage::Finished`).
    pub fn finished(&self) {
        self.connection.send(ClientMessage::Finished);
    }

    pub fn send_settings(&self, view_distance: u8, fly_speed: f32) {
        self.connection.send(ClientMessage::Settings { view_distance, fly_speed });
    }

    /// Asks the server to run a command (`time set noon`).
    pub fn command(&self, line: &str) {
        self.connection.send(ClientMessage::Command { line: line.into() });
    }

    /// Says something to everyone.
    pub fn chat(&self, text: &str) {
        self.connection.send(ClientMessage::Chat { text: text.into() });
    }
}

/// The server's time of day, as last told and advanced locally since.
#[derive(Resource, Default)]
pub struct Clock {
    /// The time of day (in ticks) when last told, and how fast it runs.
    day: f64,
    day_speed: f32,
    /// When that was, in seconds since startup.
    told_at: f64,
}

impl Clock {
    /// The time of day now, in ticks (`now` is seconds since startup).
    pub fn day(&self, now: f64) -> f64 {
        self.day + (now - self.told_at) * TICKS_PER_SECOND as f64 * self.day_speed as f64
    }
}

#[derive(Resource, Default)]
pub struct RemotePlayers {
    players: FxHashMap<PlayerId, (PlayerView, Option<Entity>)>,
}

impl RemotePlayers {
    /// Where another player stands, if they are in sight.
    pub fn position(&self, id: PlayerId) -> Option<Vec3> {
        self.players.get(&id).map(|(view, _)| view.position)
    }
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
    mut generator: ResMut<WorldGenerator>,
    mut notice: ResMut<Notice>,
    mut next: ResMut<NextState<Screen>>,
    mut weather: ResMut<ClientWeather>,
    (time, mut clock, mut console, mut air): (Res<Time>, ResMut<Clock>, ResMut<Console>, ResMut<ClientAtmosphere>),
    (mut drops, mut toasts, launch): (ResMut<Drops>, ResMut<SkillToasts>, Res<Launch>),
    mut broken: MessageWriter<BlockBroken>,
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
            ServerMessage::Welcome { position, yaw, flying, allow_flight, admin, seed, weather_seed, .. } => {
                weather.set_seed(weather_seed);
                air.start(seed);
                generator.start(seed, world.registry.clone());
                player.position = position;
                player.yaw = yaw;
                player.flying = flying;
                player.allow_flight = allow_flight;
                player.admin = admin;
                player.seed = seed;
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
                let old = world.block(pos);
                if old != Some(block) {
                    // A break the player did not predict: someone else's, or the player's own
                    // when the server's word came first, shown nearby. Plants also vanish
                    // under feet (the server's wear), which is not breaking them, so only
                    // blocks count; a player's own plants break the moment they click.
                    if let Some(old) = old
                        && block.is_air()
                        && shows_breaking(world.registry.info(old))
                        && world.registry.info(old).render != RenderKind::Plant
                        && pos.as_vec3().distance(player.position) < SHOWN_WITHIN
                    {
                        broken.write(BlockBroken { pos, block: old });
                    }
                    world.set_block(pos, block);
                }
            }
            ServerMessage::Blocks { changes } => {
                for (pos, block) in changes {
                    if world.block(pos) != Some(block) {
                        world.set_block(pos, block);
                    }
                }
            }
            ServerMessage::PlayerState { health, inventory, skills, vitals } => {
                player.health = health.current;
                player.vitals = vitals;
                player.inventory = inventory;
                // A level reached since the last word is announced (not on joining).
                let registry = &launch.content.skills;
                if player.skills_known && player.skills != skills {
                    for id in registry.ids() {
                        let level = registry.level(skills.xp(id));
                        if level > registry.level(player.skills.xp(id)) {
                            toasts.level_up(id, level, time.elapsed_secs());
                        }
                    }
                }
                player.skills = skills;
                player.skills_known = true;
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
            ServerMessage::Time { tick, day, day_speed } => {
                *clock = Clock { day: day as f64, day_speed, told_at: time.elapsed_secs_f64() };
                weather.set_tick(tick, time.elapsed_secs_f64());
                cycle.ticks = day as f64;
            }
            ServerMessage::Abilities { allow_flight, admin } => {
                player.allow_flight = allow_flight;
                player.flying &= allow_flight;
                player.admin = admin;
            }
            ServerMessage::Chat { kind, text } => {
                // Logged too, so the replies to a capture's `--command`s are on record.
                debug!("chat: {text}");
                console.push(kind, &text, time.elapsed_secs())
            }
            ServerMessage::Weather { weather: report, effects, nearby, quick } => {
                weather.receive(report, effects, nearby, quick, time.elapsed_secs())
            }
            ServerMessage::Lightning { at, seed, cloud } => weather.lightning(at, seed, at.distance(player.eye()), cloud),
            ServerMessage::Died => player.dead = true,
            ServerMessage::Items { changed, gone } => drops.receive(changed, gone, time.elapsed_secs()),
            ServerMessage::Xp { skill, tenths } => toasts.gained(skill, tenths, time.elapsed_secs()),
        }
    }
}

/// Moves the sun with the server's time of day, which may run faster than usual or not at
/// all.
fn advance_day(time: Res<Time>, clock: Res<Clock>, mut cycle: ResMut<DayCycle>) {
    if clock.told_at > 0.0 {
        cycle.ticks = clock.day(time.elapsed_secs_f64());
    }
}

fn send_input(time: Res<Time>, mut net: ResMut<Net>, player: Res<LocalPlayer>) {
    net.input_timer += time.delta_secs();
    let interval = 1.0 / TICKS_PER_SECOND as f32;
    if net.input_timer < interval || !player.joined {
        return;
    }
    net.input_timer %= interval;
    net.connection.send(ClientMessage::Input(input(&player)));
}

/// The player's state as the server is told it.
fn input(player: &LocalPlayer) -> PlayerInput {
    PlayerInput {
        position: player.position,
        yaw: player.yaw,
        pitch: player.pitch,
        selected_slot: player.selected as u8,
        flying: player.flying,
    }
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
