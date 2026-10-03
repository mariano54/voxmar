//! End-to-end server checks over an in-process connection.

use std::time::{Duration, Instant};

use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::{ClientConnection, ClientMessage, PROTOCOL_VERSION, PlayerInput, ServerMessage, local_pair};
use voxmar_server::{ServerConfig, ServerHandle, load_content, spawn};

struct Client {
    connection: ClientConnection,
    world: VoxelWorld,
    position: Vec3,
    health: u8,
    content: Content,
    inventory: voxmar_engine::inventory::Inventory,
    _server: ServerHandle,
}

impl Client {
    fn join() -> Self {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = load_content(&assets).expect("content loads");
        let fingerprint = content.fingerprint();
        let server = spawn(ServerConfig { view_distance: 2, ..Default::default() }, load_content(&assets).unwrap()).unwrap();
        let (connection, server_side) = local_pair();
        server.connections().send(server_side).unwrap();
        connection.send(ClientMessage::Hello { name: "test".into(), protocol: PROTOCOL_VERSION, content: fingerprint });
        let mut client = Self {
            connection,
            world: VoxelWorld::default(),
            position: Vec3::ZERO,
            health: 0,
            content,
            inventory: Default::default(),
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
            ServerMessage::Welcome { position, .. } | ServerMessage::Teleport { position } => self.position = position,
            ServerMessage::Column { pos, column } => self.world.insert(pos, column),
            ServerMessage::BlockChanged { pos, block } => {
                self.world.set_block(pos, block);
            }
            ServerMessage::PlayerState { health, inventory } => {
                self.health = health.current;
                self.inventory = inventory;
            }
            _ => {}
        }
    }

    fn propose(&self, action: &str, target: ActionTarget) {
        let action: ActionId = self.content.actions.expect(action);
        self.connection.send(ClientMessage::Propose { action, target });
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
    // step per input, then drop back down.
    let height = (1..=12).take_while(|&dy| client.world.block(feet + IVec3::Y * dy) == Some(BlockId::AIR)).count() as i32 - 1;
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
