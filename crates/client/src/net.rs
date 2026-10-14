//! Connection to the (local or remote) server: applies replicated state and sends the
//! player's input and actions. Present while in game.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_protocol::{
    ClientConnection, ClientMessage, PlayerId, PlayerInput, PlayerView, ServerMessage, TICKS_PER_SECOND,
};
use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::RenderKind;
use voxmar_server::ServerHandle;

use crate::lang::Locale;
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
    /// Says hello over `connection`, giving the admin token kept for `address` (the
    /// server's `host:port`, none in singleplayer) if there is one.
    pub fn new(connection: ClientConnection, server: Option<ServerHandle>, name: String, content: u64, address: Option<&str>) -> Self {
        let tokens = std::env::var("VOXMAR_ADMIN_TOKENS").unwrap_or_default();
        let admin_token = address.and_then(|address| admin_token_for(address, &tokens));
        connection.send(ClientMessage::Hello { name, protocol: voxmar_protocol::PROTOCOL_VERSION, content, admin_token });
        Self { connection, server, input_timer: 0.0 }
    }

    /// The next message from the server, for tests that play without the app.
    #[cfg(test)]
    pub fn try_recv(&self) -> Result<Option<voxmar_protocol::ServerMessage>, voxmar_protocol::Disconnected> {
        self.connection.try_recv()
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

    /// Tells the server the timed action proposed at `target` shows done here (see
    /// `ClientMessage::Finished`).
    pub fn finished(&self, target: ActionTarget) {
        self.connection.send(ClientMessage::Finished { target });
    }

    /// The player raised or lowered their guard (`fight`).
    pub fn guard(&self, up: bool) {
        self.connection.send(ClientMessage::Guard { up });
    }

    /// Answers the server's ping `id` (`ServerMessage::Ping`).
    pub fn pong(&self, id: u32) {
        self.connection.send(ClientMessage::Pong { id });
    }

    /// The player turned a prayer on or off (`fight::quick_prayers`).
    pub fn pray(&self, prayer: voxmar_engine::prayer::PrayerId, on: bool) {
        self.connection.send(ClientMessage::Pray { prayer, on });
    }

    pub fn send_settings(&self, view_distance: u8, fly_speed: f32) {
        self.connection.send(ClientMessage::Settings { view_distance, fly_speed });
    }

    /// Asks the server to run a command (`time set noon`).
    pub fn command(&self, line: &str) {
        self.connection.send(ClientMessage::Command { line: line.into() });
    }

    /// Takes `count` of `item` from creative mode's catalogue, onto the pointer (`held`) or
    /// into the inventory (see `ClientMessage::Take`).
    pub fn take(&self, item: voxmar_engine::block::BlockId, count: u8, held: bool) {
        self.connection.send(ClientMessage::Take { item, count, held });
    }

    /// Says the recipe viewer was opened on `item` (see `ClientMessage::LookedUp`).
    pub fn looked_up(&self, item: voxmar_engine::block::BlockId) {
        self.connection.send(ClientMessage::LookedUp { item });
    }

    /// Claims a finished quest's rewards (see `ClientMessage::ClaimQuest`).
    pub fn claim_quest(&self, name: &str) {
        self.connection.send(ClientMessage::ClaimQuest { name: name.into() });
    }

    /// Asks what is on the station at `block`, looked at (see `ClientMessage::LookAtStation`).
    pub fn look_at_station(&self, block: IVec3) {
        self.connection.send(ClientMessage::LookAtStation { block });
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
    /// What another player is called, while they are here.
    pub fn name(&self, id: PlayerId) -> Option<&str> {
        self.players.get(&id).map(|(view, _)| view.name.as_str())
    }

    /// The player nearest along the look ray within `within` blocks of the eye, and how far.
    pub fn looked_at(&self, eye: Vec3, look: Vec3, within: f32) -> Option<(PlayerId, f32)> {
        use voxmar_engine::physics::{Aabb, player};
        self.players
            .iter()
            .filter_map(|(&id, (view, _))| Some((id, crate::creatures::ray_box(eye, look, &Aabb::standing(view.position, player::WIDTH, player::HEIGHT))?)))
            .filter(|&(_, distance)| distance <= within)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Where another player stands, if they are in sight.
    pub fn position(&self, id: PlayerId) -> Option<Vec3> {
        self.players.get(&id).map(|(view, _)| view.position)
    }
}

/// Applies what the server sent. If it refuses us or goes away, returns to the menus with
/// the reason.
#[allow(clippy::too_many_arguments)]
pub(crate) fn receive(
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
    (mut drops, mut toasts, launch, locale, mut creatures): (ResMut<Drops>, ResMut<SkillToasts>, Res<Launch>, Res<Locale>, ResMut<crate::creatures::Creatures>),
    mut broken: MessageWriter<BlockBroken>,
    mut fighting: ResMut<crate::fight::Fighting>,
    mut stations: ResMut<crate::hud::target::StationSeen>,
) {
    let menu = if net.is_remote() { Screen::Join } else { Screen::Title };
    loop {
        let message = match net.connection.try_recv() {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(_) => {
                let reason = if net.is_remote() { "net.lost" } else { "net.world_stopped" };
                notice.0 = Some(locale.get(reason).into());
                next.set(menu);
                break;
            }
        };
        match message {
            ServerMessage::Welcome { player: me, position, yaw, flying, allow_flight, admin, seed, weather_seed } => {
                creatures.me = Some(me);
                weather.set_seed(weather_seed);
                air.start(seed);
                generator.start(seed, &launch.content);
                player.position = position;
                player.yaw = yaw;
                player.flying = flying;
                player.allow_flight = allow_flight;
                player.admin = admin;
                player.seed = seed;
                player.joined = true;
                player.journal = Default::default();
                player.quests.clear();
            }
            ServerMessage::Rejected { reason } => {
                notice.0 = Some(locale.format("net.rejected", &[("reason", &locale.render(&reason, &world.registry))]));
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
                player.states += 1;
                player.health = health.current;
                player.max_health = health.max;
                player.vitals = vitals;
                player.vitals_at = time.elapsed_secs();
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
            ServerMessage::Teleport { position, seq } => {
                player.position = position;
                player.teleports = seq;
                player.velocity = Vec3::ZERO;
                // Put somewhere, a dash under way stops: a dodge the server refused, whose
                // cooldown never ran there either.
                if fighting.dash.take().is_some() {
                    fighting.dodge_ready = fighting.dodge_was;
                }
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
                let text = locale.render(&text, &world.registry);
                // Logged too, so the replies to a capture's `--command`s are on record.
                debug!("chat: {text}");
                console.push(kind, &text, time.elapsed_secs())
            }
            ServerMessage::Refused { why } => {
                let why = locale.render(&why, &world.registry);
                debug!("refused: {why}");
                toasts.refused(why, time.elapsed_secs())
            }
            ServerMessage::Weather { weather: report, effects, nearby, quick } => {
                weather.receive(report, effects, nearby, quick, time.elapsed_secs())
            }
            ServerMessage::Lightning { at, seed, cloud } => weather.lightning(at, seed, at.distance(player.eye()), cloud),
            ServerMessage::Died => player.dead = true,
            ServerMessage::Journal { entries } => player.journal.merge(entries),
            ServerMessage::Quests { states } => {
                player.quests = states;
                player.quests_rev = player.quests_rev.wrapping_add(1);
            }
            ServerMessage::Items { changed, gone } => drops.receive(changed, gone, time.elapsed_secs()),
            ServerMessage::Creatures { changed, gone } => creatures.receive(changed, gone),
            ServerMessage::Struck { creature, damage, killed, blow, pips } => {
                creatures.struck(creature, damage, killed, time.elapsed_secs());
                fighting.struck(creature, damage, blow, pips, killed, time.elapsed_secs_f64());
            }
            ServerMessage::Hurt { creature, damage, defended } => fighting.hurt(creature, damage, defended, time.elapsed_secs_f64()),
            ServerMessage::Ping { id } => {
                net.pong(id);
                fighting.pings += 1;
            }
            ServerMessage::Prayers { points, most, on } => fighting.prayers = crate::fight::Prayers { points, most, on },
            ServerMessage::Xp { skill, tenths } => toasts.gained(skill, tenths, time.elapsed_secs()),
            ServerMessage::Station { block, load, fire } => stations.tell(block, load, fire, time.elapsed_secs_f64()),
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

/// The admin token `tokens` keeps for the server at `address`: `tokens` lists
/// `host:port=token` entries, separated by spaces, commas or semicolons, so a server's token
/// goes to that server only (a server's admin token is its owner's, given with
/// `VOXMAR_ADMIN_TOKENS`; see the dedicated server's `--admin-token-file`).
pub fn admin_token_for(address: &str, tokens: &str) -> Option<String> {
    tokens
        .split([' ', ',', ';', '\n'])
        .filter_map(|entry| entry.split_once('='))
        .find(|(server, _)| crate::menu::join::with_port(server.trim()).eq_ignore_ascii_case(&crate::menu::join::with_port(address.trim())))
        .map(|(_, token)| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

/// The player's state as the server is told it.
fn input(player: &LocalPlayer) -> PlayerInput {
    PlayerInput {
        position: player.position,
        yaw: player.yaw,
        pitch: player.pitch,
        selected_slot: player.selected as u8,
        flying: player.flying,
        teleport: player.teleports,
    }
}

/// Other players are drawn as simple boxes, which cast no shadow. Their pipelines compile as
/// the world loads (`render::warm`), not when the first other player comes into view.
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
            let mesh = meshes.add(Cuboid::new(size, voxmar_engine::physics::player::HEIGHT, size));
            let material = materials.add(StandardMaterial { base_color: Color::srgb(0.85, 0.35, 0.3), unlit: true, ..default() });
            crate::render::warm::warm(&mut commands, mesh.clone(), material.clone(), false, DespawnOnExit(Screen::InGame));
            (mesh, material)
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
                let body = (Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), transform, NotShadowCaster, DespawnOnExit(Screen::InGame));
                *entity = Some(commands.spawn(body).id());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_admin_token_goes_to_its_own_server_only() {
        let tokens = "play.example.org:25665=open-sesame, 192.168.1.4:25665=lan;[::1]:4000=six";
        assert_eq!(admin_token_for("play.example.org:25665", tokens).as_deref(), Some("open-sesame"));
        assert_eq!(admin_token_for("PLAY.example.org:25665", tokens).as_deref(), Some("open-sesame"));
        assert_eq!(admin_token_for("192.168.1.4:25665", tokens).as_deref(), Some("lan"));
        assert_eq!(admin_token_for("[::1]:4000", tokens).as_deref(), Some("six"));
        assert_eq!(admin_token_for("play.example.org:25666", tokens), None, "another port is another server");
        assert_eq!(admin_token_for("evil.example.org:25665", tokens), None);
        assert_eq!(admin_token_for("play.example.org:25665", ""), None);
        // An address typed without its port is the default port, on either side.
        assert_eq!(admin_token_for("play.example.org", tokens).as_deref(), Some("open-sesame"));
        assert_eq!(admin_token_for("192.168.1.4:25665", "192.168.1.4=bare").as_deref(), Some("bare"));
        assert_eq!(admin_token_for("::1", "[::1]:25665=six").as_deref(), Some("six"));
    }

    #[test]
    fn a_player_is_looked_at_within_range_and_aim() {
        let mut players = RemotePlayers::default();
        let view = PlayerView { id: 9, name: "Ada".to_owned(), position: Vec3::new(0.0, 0.0, -10.0), yaw: 0.0 };
        players.players.insert(9, (view, None));
        let eye = Vec3::new(0.0, 1.62, 0.0);
        let (who, distance) = players.looked_at(eye, Vec3::NEG_Z, 24.0).unwrap();
        assert_eq!(who, 9);
        assert!((distance - 9.7).abs() < 1e-4, "met at their near face: {distance}");
        assert_eq!(players.looked_at(eye, Vec3::NEG_Z, 8.0), None, "further than they are named from");
        assert_eq!(players.looked_at(eye + Vec3::X * 2.0, Vec3::NEG_Z, 24.0), None, "looking past them");
    }
}
