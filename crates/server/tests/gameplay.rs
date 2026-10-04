//! End-to-end server checks over in-process and TCP connections.

use std::time::{Duration, Instant};

use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::{
    ClientConnection, ClientMessage, PROTOCOL_VERSION, PlayerInput, ServerMessage, Weather, local_pair, tcp,
};
use voxmar_server::{ServerConfig, ServerHandle, load_content, spawn};

struct Client {
    connection: ClientConnection,
    world: VoxelWorld,
    position: Vec3,
    admin: bool,
    allow_flight: bool,
    /// The last position the server corrected us to.
    teleported: Option<Vec3>,
    health: u8,
    content: Content,
    inventory: voxmar_engine::inventory::Inventory,
    weather: Option<Weather>,
    _server: ServerHandle,
}

fn content() -> Content {
    load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads")
}

impl Client {
    /// Joins a new server in-process, as singleplayer does.
    fn join() -> Self {
        Self::join_with(ServerConfig { view_distance: 2, ..Default::default() })
    }

    fn join_with(config: ServerConfig) -> Self {
        let server = spawn(config, content()).unwrap();
        let (connection, server_side) = local_pair();
        server.connections().send(server_side).unwrap();
        Self::hello(connection, "test", server)
    }

    /// Joins a new server over TCP as `name`.
    fn join_remote(name: &str, admins: &[&str]) -> Self {
        let admins = admins.iter().map(|&a| a.to_owned()).collect();
        let server = spawn(ServerConfig { view_distance: 2, admins, ..Default::default() }, content()).unwrap();
        let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
        Self::hello(tcp::connect(addr).unwrap(), name, server)
    }

    fn hello(connection: ClientConnection, name: &str, server: ServerHandle) -> Self {
        let content = content();
        connection.send(ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content: content.fingerprint() });
        let mut client = Self {
            connection,
            world: VoxelWorld::default(),
            position: Vec3::ZERO,
            admin: false,
            allow_flight: false,
            teleported: None,
            health: 0,
            content,
            inventory: Default::default(),
            weather: None,
            _server: server,
        };
        client.pump_until(|c| c.health > 0 && c.world.contains(ColumnPos::of_block(c.position.as_ivec3())));
        client
    }

    fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for server");
            match self.connection.try_recv() {
                Ok(Some(message)) => self.apply(message),
                Ok(None) => std::thread::sleep(Duration::from_millis(1)),
                Err(_) => panic!("server disconnected"),
            }
        }
    }

    fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome { position, admin, allow_flight, .. } => {
                self.position = position;
                self.admin = admin;
                self.allow_flight = allow_flight;
            }
            ServerMessage::Teleport { position } => {
                self.position = position;
                self.teleported = Some(position);
            }
            ServerMessage::Column { pos, column } => self.world.insert(pos, column),
            ServerMessage::BlockChanged { pos, block } => {
                self.world.set_block(pos, block);
            }
            ServerMessage::PlayerState { health, inventory } => {
                self.health = health.current;
                self.inventory = inventory;
            }
            ServerMessage::Weather(weather) => self.weather = Some(weather),
            _ => {}
        }
    }

    fn propose(&self, action: &str, target: ActionTarget) {
        let action: ActionId = self.content.actions.expect(action);
        self.connection.send(ClientMessage::Propose { action, target });
    }

    fn send_position(&self, position: Vec3, flying: bool) {
        self.connection.send(ClientMessage::Input(PlayerInput { position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying }));
    }
}

#[test]
fn break_and_place_blocks() {
    let mut client = Client::join();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    let ground = client.world.block(below).unwrap();
    assert!(!ground.is_air());

    let target = ActionTarget::Block { block: below, face: Face::PosY };
    // Mining takes the block's hardness in seconds; cancelling before then changes nothing.
    client.propose("core:mine", target);
    client.connection.send(ClientMessage::Cancel);
    std::thread::sleep(Duration::from_millis(1500));
    client.pump_until(|_| true);
    assert_eq!(client.world.block(below), Some(ground));

    let started = std::time::Instant::now();
    client.propose("core:mine", target);
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    assert!(started.elapsed() >= Duration::from_millis(400), "mining completed too early");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.count == 1));

    // Place the first hotbar item back on the block underneath the hole.
    let item = client.inventory.get(0).unwrap();
    client.connection.send(ClientMessage::Input(PlayerInput {
        position: client.position,
        yaw: 0.0,
        pitch: 0.0,
        selected_slot: 0,
        flying: false,
    }));
    client.propose("core:place", ActionTarget::Block { block: below - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(below) == Some(item.item));
    client.pump_until(|c| c.inventory.get(0).map(|s| s.count) == Some(item.count - 1));
}

#[test]
fn falling_hurts() {
    let mut client = Client::join();
    let start = client.position;
    let feet = start.floor().as_ivec3();
    // Climb as high as the open air above spawn allows (at most 10 blocks), one plausible
    // step per input, then drop back down. Plants are no obstacle.
    let open = |p: IVec3| client.world.block(p).is_some_and(|b| !client.content.blocks.info(b).solid);
    let height = (1..=12).take_while(|&dy| open(feet + IVec3::Y * dy)).count() as i32 - 1;
    assert!(height >= 6, "need open sky above spawn, have {height} blocks");
    let path = (1..=height).chain((0..height).rev());
    for y in path {
        client.connection.send(ClientMessage::Input(PlayerInput {
            position: start + Vec3::Y * y as f32,
            yaw: 0.0,
            pitch: 0.0,
            selected_slot: 0,
            flying: false,
        }));
        std::thread::sleep(Duration::from_millis(60));
    }
    client.pump_until(|c| c.health < 20);
    let expected = 20 - (height - 3) as u8;
    assert_eq!(client.health, expected, "a {height} block fall should cost {} health", height - 3);
}

#[test]
fn only_admins_fly_fast() {
    // The integrated server's own player and players named with `--admin` are admins.
    let clients = [(Client::join(), true), (Client::join_remote("guest", &["boss"]), false), (Client::join_remote("boss", &["boss"]), true)];
    for (mut client, admin) in clients {
        assert_eq!(client.admin, admin);
        client.connection.send(ClientMessage::Settings { view_distance: 2, fly_speed: 4.0 });
        // A second after the last move, rise faster than normal flight allows but within
        // four times its speed, then make an impossible move so the server answers with
        // where it thinks we are.
        std::thread::sleep(Duration::from_millis(1100));
        let start = client.position;
        let up = start + Vec3::Y * 100.0;
        client.send_position(up, true);
        client.send_position(start + Vec3::Y * 5000.0, true);
        client.pump_until(|c| c.teleported.is_some());
        assert_eq!(client.teleported, Some(if admin { up } else { start }), "admin: {admin}");
    }
}

/// A fresh directory for a saved world.
fn world_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("voxmar-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn saved_worlds_keep_edits_and_players_across_restarts() {
    let dir = world_dir("restart");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.count == 1));
    // Step a little within the same block, so the position is our own rather than spawn.
    let moved = client.position + Vec3::new(0.2, 0.0, -0.2);
    client.send_position(moved, false);
    std::thread::sleep(Duration::from_millis(200));
    let inventory = client.inventory.clone();
    // Dropping the server saves the world.
    drop(client);

    let column = ColumnPos::of_block(below);
    assert!(dir.join("world.ron").is_file());
    assert!(dir.join(format!("columns/{}.{}.bin", column.0.x, column.0.y)).is_file());
    let mut client = Client::join_with(config());
    assert_eq!(client.position, moved);
    client.pump_until(|c| c.inventory == inventory);
    assert_eq!(client.world.block(below), Some(BlockId::AIR), "the mined block stays mined");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn game_mode_decides_flight_and_admin() {
    use voxmar_server::save::{GameMode, WorldMeta, WorldSave};
    for (mode, creative) in [(GameMode::Survival, false), (GameMode::Creative, true)] {
        let dir = world_dir(&format!("{mode:?}"));
        WorldSave::new(&dir).create(&WorldMeta::new("Mode", 9, mode)).unwrap();
        let client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
        assert_eq!((client.allow_flight, client.admin), (creative, creative), "{mode:?}");
        drop(client);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn players_are_told_the_weather() {
    let mut client = Client::join();
    client.pump_until(|c| c.weather.is_some());
    let weather = client.weather.unwrap();
    let fractions = [weather.cover, weather.convection, weather.cirrus, weather.humidity, weather.precipitation, weather.snow];
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)), "{weather:?}");
    assert!(weather.wind.is_finite());
}
