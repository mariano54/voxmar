//! End-to-end server checks over in-process and TCP connections.

use std::time::{Duration, Instant};

use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::{
    ChatKind, ClientConnection, ClientMessage, ItemView, PROTOCOL_VERSION, PlayerId, PlayerInput, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND,
    Weather, local_pair, tcp,
};
use voxmar_server::{ServerConfig, ServerHandle, load_content, spawn};

struct Client {
    connection: ClientConnection,
    id: PlayerId,
    /// Dropped items in sight, and the ones we picked up.
    items: std::collections::HashMap<u32, ItemView>,
    collected: Vec<u32>,
    skills: voxmar_engine::skill::Skills,
    /// Experience earned, in tenths, in the order it was.
    earned: Vec<u32>,
    world: VoxelWorld,
    position: Vec3,
    admin: bool,
    allow_flight: bool,
    /// The last position the server corrected us to.
    teleported: Option<Vec3>,
    health: u8,
    max_health: u8,
    vitals: voxmar_engine::vitals::Vitals,
    content: Content,
    inventory: voxmar_engine::inventory::Inventory,
    weather: Option<Weather>,
    /// Whether the last weather report asked to change over quickly.
    quick: bool,
    /// Console lines received.
    chat: Vec<(ChatKind, String)>,
    /// The time of day and how fast it runs, as last told.
    day: Option<(u64, f32)>,
    /// The server's tick as last told (once a second).
    server_tick: u64,
    /// Creatures in sight, how many were seen to die, and our blows the server answered.
    creatures: std::collections::HashMap<u32, voxmar_protocol::CreatureView>,
    deaths: u32,
    blows: u32,
    /// The damage each of our blows did (0 for a miss).
    damage: Vec<u16>,
    journal: voxmar_engine::journal::Journal,
    quests: Vec<voxmar_engine::quest::QuestState>,
    /// The server, for the first player to join it (dropping it stops the server).
    _server: Option<ServerHandle>,
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
        Self::greet(connection, name, Some(server))
    }

    /// Joins this player's server too, as `name`.
    fn another(&self, name: &str) -> Self {
        let (connection, server_side) = local_pair();
        self._server.as_ref().expect("the first player holds the server").connections().send(server_side).unwrap();
        Self::greet(connection, name, None)
    }

    fn greet(connection: ClientConnection, name: &str, server: Option<ServerHandle>) -> Self {
        let content = content();
        connection.send(ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content: content.fingerprint() });
        let mut client = Self {
            connection,
            id: 0,
            items: Default::default(),
            collected: Vec::new(),
            skills: Default::default(),
            earned: Vec::new(),
            world: VoxelWorld::default(),
            position: Vec3::ZERO,
            admin: false,
            allow_flight: false,
            teleported: None,
            health: 0,
            max_health: 0,
            vitals: Default::default(),
            content,
            inventory: Default::default(),
            weather: None,
            quick: false,
            chat: Vec::new(),
            day: None,
            server_tick: 0,
            creatures: Default::default(),
            deaths: 0,
            blows: 0,
            damage: Vec::new(),
            journal: Default::default(),
            quests: Vec::new(),
            _server: server,
        };
        client.pump_until(|c| c.health > 0 && c.world.contains(ColumnPos::of_block(c.position.as_ivec3())));
        client
    }

    /// Applies what the server sends until `done`. The limits only catch a server that
    /// hangs, so they follow its progress rather than the clock: a fixed ten seconds failed
    /// four tests on a machine whose cores other builds had (generating the world around a
    /// joining player took longer), though the server was working all along.
    fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        let start = Instant::now();
        let mut heard = start;
        while !done(self) {
            let now = Instant::now();
            assert!(
                now - heard < SERVER_SILENT && now - start < SERVER_WAIT,
                "timed out waiting for server: {:.1}s in all, {:.1}s since its last message",
                (now - start).as_secs_f32(),
                (now - heard).as_secs_f32()
            );
            match self.connection.try_recv() {
                Ok(Some(message)) => {
                    heard = now;
                    self.apply(message);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(1)),
                Err(_) => panic!("server disconnected"),
            }
        }
    }

    fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome { player, position, admin, allow_flight, .. } => {
                self.id = player;
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
            ServerMessage::Blocks { changes } => {
                for (pos, block) in changes {
                    self.world.set_block(pos, block);
                }
            }
            ServerMessage::PlayerState { health, inventory, skills, vitals } => {
                self.health = health.current;
                self.max_health = health.max;
                self.vitals = vitals;
                self.inventory = inventory;
                self.skills = skills;
            }
            ServerMessage::Xp { tenths, .. } => self.earned.push(tenths),
            ServerMessage::Weather { weather, quick, .. } => {
                self.weather = Some(weather);
                self.quick = quick;
            }
            ServerMessage::Chat { kind, text } => self.chat.push((kind, self.content.english.render(&text, &self.content.blocks))),
            ServerMessage::Time { tick, day, day_speed } => {
                self.day = Some((day, day_speed));
                self.server_tick = tick;
            }
            ServerMessage::Abilities { allow_flight, admin } => {
                self.allow_flight = allow_flight;
                self.admin = admin;
            }
            ServerMessage::Creatures { changed, gone } => {
                for view in changed {
                    self.creatures.insert(view.id, view);
                }
                for gone in gone {
                    self.creatures.remove(&gone.id);
                    self.deaths += gone.died as u32;
                }
            }
            ServerMessage::Struck { damage, .. } => {
                self.blows += 1;
                self.damage.push(damage);
            }
            ServerMessage::Journal { entries } => self.journal.merge(entries),
            ServerMessage::Quests { states } => self.quests = states,
            ServerMessage::Items { changed, gone } => {
                for view in changed {
                    self.items.insert(view.id, view);
                }
                for gone in gone {
                    self.items.remove(&gone.id);
                    if gone.collector == Some(self.id) {
                        self.collected.push(gone.id);
                    }
                }
            }
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

    /// Runs a command and returns the console lines it answers with.
    fn command(&mut self, line: &str) -> Vec<(ChatKind, String)> {
        self.chat.clear();
        self.connection.send(ClientMessage::Command { line: line.into() });
        self.pump_until(|c| !c.chat.is_empty());
        std::mem::take(&mut self.chat)
    }

    /// Waits until the server has handled everything sent before (refused or instant
    /// actions included), by a command's answer coming back after them.
    fn settle(&mut self) {
        self.ok("/time");
    }

    /// Waits until at least `ticks` server ticks have passed from now, by the server's own
    /// clock (it tells it once a second).
    fn after_ticks(&mut self, ticks: u64) {
        let told = self.server_tick;
        self.pump_until(|c| c.server_tick > told);
        let from = self.server_tick;
        self.pump_until(|c| c.server_tick >= from + ticks);
    }

    /// Waits until `count` holds still across 20 server ticks after everything sent was
    /// handled (drops still landing, pick-ups on the way), and returns it.
    fn quiet(&mut self, count: impl Fn(&Self) -> usize) -> usize {
        for _ in 0..30 {
            self.settle();
            let before = count(self);
            self.after_ticks(20);
            self.settle();
            if count(self) == before {
                return before;
            }
        }
        panic!("a count never held still for 20 ticks");
    }

    /// Runs a command that should succeed, returning its answer.
    fn ok(&mut self, line: &str) -> String {
        let lines = self.command(line);
        assert!(lines.iter().all(|(kind, _)| *kind == ChatKind::Info), "{line}: {lines:?}");
        lines.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n")
    }
}

/// Ticks a server behind the clock runs back to back to catch up, at most (it gives up on
/// a longer backlog, see `Server::run`), plus the one in progress: how much earlier by the
/// clock than its ticks say an action can complete.
/// How long the server may send nothing, and how long a wait may last in all, before a
/// test gives up on it.
const SERVER_SILENT: Duration = Duration::from_secs(20);
const SERVER_WAIT: Duration = Duration::from_secs(120);

const CATCH_UP: Duration = Duration::from_millis(11 * 1000 / TICKS_PER_SECOND as u64);

#[test]
fn break_and_place_blocks() {
    let mut client = Client::join();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    // Stone, which takes long enough to mine that a server catching up cannot finish it
    // anywhere near at once.
    client.ok("/setblock ~ ~-1 ~ stone");
    client.pump_until(|c| c.world.block(below) == c.content.blocks.id("base:stone"));
    let ground = client.world.block(below).unwrap();
    let hardness = Duration::from_secs_f32(client.content.blocks.info(ground).hardness);

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
    assert!(started.elapsed() + CATCH_UP >= hardness, "mining completed too early: {:?} of {hardness:?}", started.elapsed());
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
fn broken_blocks_pop_out_items_that_are_picked_up_and_thrown_stacks_fly() {
    let mut client = Client::join();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    let ground = client.world.block(below).unwrap();
    let drop = client.content.blocks.info(ground).drops;
    client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
    // The drop appears in the world first, popping up from the broken block...
    client.pump_until(|c| c.items.values().any(|i| i.stack.item == drop && i.velocity.y > 0.0));
    let popped = *client.items.values().next().unwrap();
    assert!((popped.position.floor().as_ivec3() - below).abs().max_element() <= 1, "{popped:?} from {below}");
    // ...then, after the pickup delay, goes into the inventory of the player standing over it.
    let started = Instant::now();
    client.pump_until(|c| c.collected.contains(&popped.id));
    assert!(started.elapsed() >= Duration::from_millis(300), "picked up before it could be seen");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == drop));

    // Throwing one item from the first slot sends it flying the way we look (yaw 0: -z).
    let before = client.inventory.get(0).unwrap();
    client.propose("core:drop", ActionTarget::Stack { slot: 0, count: 1 });
    client.pump_until(|c| c.inventory.get(0).map(|s| s.count) == Some(before.count - 1));
    client.pump_until(|c| c.items.values().any(|i| i.stack == ItemStack { item: before.item, count: 1 } && i.velocity.z < -1.0));
}

#[test]
fn crops_are_sown_at_their_level_grow_and_are_harvested_for_experience() {
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let (ground, other) = (feet + IVec3::new(0, -1, 2), feet + IVec3::new(1, -1, 2));
    for line in ["/fill ~ ~-1 ~2 ~1 ~-1 ~2 dirt", "/fill ~ ~ ~2 ~1 ~1 ~2 air", "/give wheat_seeds 5", "/give saffron_seeds 5", "/give wooden_hoe 1"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(ground).is_some_and(|b| b != BlockId::AIR) && c.inventory.get(6).is_some());
    let blocks = &client.content.blocks;
    let (sprout, ripe, wheat) = (blocks.expect("base:wheat_crop_0"), blocks.expect("base:wheat_crop"), blocks.expect("base:wheat"));
    let farmland = blocks.expect("base:farmland");
    assert_eq!(client.inventory.get(4).map(|s| s.item), blocks.id("base:wheat_seeds"));
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };

    // Seeds go on farmland only: sowing on dirt is refused, and a hoe tills it.
    hold(&client, 4);
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    hold(&client, 6);
    for block in [ground, other] {
        client.propose("base:till", ActionTarget::Block { block, face: Face::PosY });
    }
    client.pump_until(|c| c.world.block(ground) == Some(farmland) && c.world.block(other) == Some(farmland));
    assert_eq!(client.world.block(ground + IVec3::Y), Some(BlockId::AIR), "sown on untilled dirt");

    // Saffron takes farming 60: refused, nothing sown, no experience.
    hold(&client, 5);
    client.propose("base:plant", ActionTarget::Block { block: other, face: Face::PosY });
    // Wheat takes level 1: sown, its planting experience waiting for the harvest.
    hold(&client, 4);
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout));
    assert!(client.earned.is_empty(), "sowing paid at once: {:?}", client.earned);
    assert_eq!(client.world.block(other + IVec3::Y), Some(BlockId::AIR), "sown without the level");
    client.pump_until(|c| c.inventory.get(4).map(|s| s.count) == Some(4));

    // Grown ripe, then broken: its harvest pops out and its experience is earned, the
    // sowing's (2) with the harvest's (9), in tenths.
    assert_eq!(client.ok("/grow ripe"), "Grew 1 plant.");
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(ripe));
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.earned == [110] && c.items.values().any(|i| i.stack.item == wheat));
    client.pump_until(|c| c.skills.xp(voxmar_engine::skill::SkillId(0)) == 110);

    // Ripe fruit on a tree is picked by hand: the fruit pops out and the
    // leaves go back to bare, to fruit again.
    let leaves = feet + IVec3::new(-1, 1, 1);
    client.ok("/setblock ~-1 ~1 ~1 apple_leaves");
    let (ripe, bare, apple) = {
        let blocks = &client.content.blocks;
        (blocks.expect("base:apple_leaves"), blocks.expect("base:apple_leaves_0"), blocks.expect("base:apple"))
    };
    client.pump_until(|c| c.world.block(leaves) == Some(ripe));
    client.propose("base:pick", ActionTarget::Block { block: leaves, face: Face::PosZ });
    client.pump_until(|c| c.world.block(leaves) == Some(bare) && c.items.values().any(|i| i.stack.item == apple));
    client.pump_until(|c| c.earned.last() == Some(&60));
}

#[test]
fn a_block_mined_nearly_long_enough_still_breaks_when_the_next_is_started() {
    // A busy server can complete a break later than the client, which shows it done and
    // starts on the next block; that must not cancel the first. Too early, it still does.
    // Stone takes long enough that a server catching up on its ticks cannot finish the
    // block only begun before it is replaced.
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let (a, b, c) = (feet + IVec3::new(0, -1, 1), feet + IVec3::new(0, -1, 2), feet + IVec3::new(1, -1, 1));
    client.ok("/fill ~ ~-1 ~1 ~1 ~-1 ~2 stone");
    client.pump_until(|cl| [a, b, c].into_iter().all(|p| cl.world.block(p) == cl.content.blocks.id("base:stone")));
    let stone = client.world.block(a).unwrap();
    let hardness = client.content.blocks.info(stone).hardness;
    assert!(Duration::from_secs_f32(hardness) > CATCH_UP * 2, "stone mines too fast for this test");
    let mine = |client: &Client, block: IVec3| client.propose("core:mine", ActionTarget::Block { block, face: Face::PosY });
    mine(&client, c);
    std::thread::sleep(Duration::from_millis(50));
    mine(&client, a);
    std::thread::sleep(Duration::from_secs_f32(hardness * 0.85));
    mine(&client, b);
    client.pump_until(|cl| cl.world.block(a) == Some(BlockId::AIR) && cl.world.block(b) == Some(BlockId::AIR));
    assert_eq!(client.world.block(c), Some(stone), "a block only begun was broken");
}

#[test]
fn a_block_the_client_finished_breaks_even_if_the_server_started_it_late() {
    // A server stalled when the proposal came (streaming, a slow tick) starts its clock
    // late; by that clock the client finishes early and starts on the next block at once.
    // Once the client says the first is done, the next proposal must not cancel it: it
    // breaks in its own time.
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let (a, b) = (feet + IVec3::new(0, -1, 1), feet + IVec3::new(0, -1, 2));
    client.ok("/fill ~ ~-1 ~1 ~ ~-1 ~2 stone");
    client.pump_until(|cl| [a, b].into_iter().all(|p| cl.world.block(p) == cl.content.blocks.id("base:stone")));
    let mine = |client: &Client, block: IVec3| client.propose("core:mine", ActionTarget::Block { block, face: Face::PosY });
    mine(&client, a);
    client.connection.send(ClientMessage::Finished { target: ActionTarget::Block { block: a, face: Face::PosY } });
    mine(&client, b);
    client.pump_until(|cl| cl.world.block(a) == Some(BlockId::AIR) && cl.world.block(b) == Some(BlockId::AIR));
}

#[test]
fn falling_hurts() {
    let mut client = Client::join();
    let start = client.position;
    let feet = start.floor().as_ivec3();
    // Climb as high as the open air (or grass) above spawn allows (at most 11 blocks), one
    // plausible step per input, then drop back down.
    let open = |c: &Client, p: IVec3| c.world.block(p).is_some_and(|b| !c.content.blocks.info(b).solid);
    let height = (1..=12).take_while(|&dy| open(&client, feet + IVec3::Y * dy)).count() as i32 - 1;
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
    let purse = client.content.blocks.id("base:coin_purse");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.count == 1 && Some(s.item) != purse));
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
fn a_world_placed_altar_drops_nothing_and_a_players_drops_itself_across_restarts() {
    let dir = world_dir("altar");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let feet = client.position.floor().as_ivec3();
    let (village, own) = (feet + IVec3::new(1, 0, 1), feet + IVec3::new(-1, 0, 1));
    // One set by the world (a village chapel's, as /setblock sets it), one the player places.
    for line in ["/clear", "/give stone_altar 1", "/setblock ~1 ~ ~1 stone_altar", "/fill ~-1 ~ ~1 ~-1 ~1 ~1 air", "/fill ~-1 ~-1 ~1 ~-1 ~-1 ~1 stone"] {
        client.ok(line);
    }
    let altar = client.content.blocks.expect("base:stone_altar");
    client.pump_until(|c| c.world.block(village) == Some(altar) && c.inventory.get(0).is_some_and(|s| s.item == altar));
    client.propose("core:place", ActionTarget::Block { block: own - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(altar));
    let altars = |c: &Client| {
        c.inventory.slots.iter().flatten().filter(|s| s.item == altar).map(|s| s.count as usize).sum::<usize>()
            + c.items.values().filter(|i| i.stack.item == altar).map(|i| i.stack.count as usize).sum::<usize>()
    };
    // The village's: nothing.
    client.propose("core:mine", ActionTarget::Block { block: village, face: Face::PosY });
    client.pump_until(|c| c.world.block(village) == Some(BlockId::AIR));
    assert_eq!(client.quiet(altars), 0, "a village's altar dropped itself");
    // Saved and loaded, the player's is still the player's: it gives itself back.
    drop(client);
    let mut client = Client::join_with(config());
    client.pump_until(|c| c.world.block(own) == Some(altar));
    client.propose("core:mine", ActionTarget::Block { block: own, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(BlockId::AIR));
    assert_eq!(client.quiet(altars), 1, "a player's altar did not give itself back");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn game_mode_decides_flight_and_the_host_is_admin() {
    use voxmar_server::save::{GameMode, WorldMeta, WorldSave};
    for (mode, creative) in [(GameMode::Survival, false), (GameMode::Creative, true)] {
        let dir = world_dir(&format!("{mode:?}"));
        WorldSave::new(&dir).create(&WorldMeta::new("Mode", 9, mode)).unwrap();
        let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
        let mut client = Client::join_with(config());
        assert_eq!((client.allow_flight, client.admin), (creative, true), "{mode:?}");
        // An admin can switch modes, and the player keeps theirs.
        let (other, name) = if creative { ("survival", "Survival") } else { ("creative", "Creative") };
        assert_eq!(client.ok(&format!("/gamemode {other}")), format!("You are now in {name} mode."));
        assert_eq!(client.allow_flight, !creative);
        drop(client);
        let client = Client::join_with(config());
        assert_eq!(client.allow_flight, !creative, "{mode:?} world after /gamemode {other}");
        drop(client);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn commands_set_the_time_and_the_weather() {
    let mut client = Client::join();
    client.pump_until(|c| c.day.is_some());
    assert!(client.ok("/seed").starts_with("Seed: "));
    assert!(client.ok("/help").contains("/weather"));
    assert!(client.ok("help time").contains("set <sunrise"));
    let (_, unknown) = &client.command("/nonsense")[0];
    assert!(unknown.contains("/help"));
    let errors = client.command("/time set teatime");
    assert_eq!(errors[0].0, ChatKind::Error);
    assert!(errors[0].1.contains("Usage: /time"), "{errors:?}");

    // Setting the time moves forward to it, and every player is told.
    let (before, _) = client.day.unwrap();
    assert!(client.ok("/time set noon").contains("12:00"));
    let (day, speed) = client.day.unwrap();
    // A day at normal speed takes the content's 30 minutes, so its ticks run at 20 / 30.
    assert_eq!((day % TICKS_PER_DAY, speed), (8000, 20.0 / 30.0));
    assert!(day >= before && day < before + TICKS_PER_DAY);
    client.ok("/night");
    assert_eq!(client.day.unwrap().0 % TICKS_PER_DAY, 17_000);
    client.ok("/time add 2h");
    // Two hours on, give or take the ticks that ran in between.
    assert!((client.day.unwrap().0 % TICKS_PER_DAY).abs_diff(19_000) < 40, "{:?}", client.day);
    assert!(client.ok("/time freeze").contains("frozen"));
    assert_eq!(client.day.unwrap().1, 0.0);
    assert!(client.ok("/time speed 10").contains("3.0 minutes per day"));
    assert_eq!(client.command("/time speed -1")[0].0, ChatKind::Error);

    // Forced weather arrives quickly, and stays.
    assert_eq!(client.ok("/weather storm max 60"), "Bringing max storm (100%) to the land around you for 60 seconds.");
    client.pump_until(|c| c.quick && c.weather.is_some_and(|w| w.precipitation == 1.0));
    let report = client.ok("/weather");
    // Named by its strongest effect, which follows the local climate (a thunderstorm, or
    // hail where it is cool), so the world's seed decides which.
    assert!(report.contains(", storm for 5"), "{report}");
    assert_eq!(client.command("/weather hurricane")[0].0, ChatKind::Error);
    assert!(client.ok("/weather list").contains("drizzle"));
    assert_eq!(client.ok("/weather natural"), "The natural weather is back.");

    // Wind on its own, and on top of forced weather.
    assert!(client.ok("/wind gale sw 30").contains("gale from the south-west"));
    client.pump_until(|c| c.weather.is_some_and(|w| (w.wind.length() - 18.0).abs() < 0.01 && w.wind.x > 0.0 && w.wind.y < 0.0));
    assert!(client.ok("/weather rain 0.5").starts_with("Bringing moderate rain (50%)"));
    assert_eq!(client.command("/weather rain 2.5")[0].0, ChatKind::Error);
    client.ok("/wind calm");
    client.pump_until(|c| c.weather.is_some_and(|w| w.precipitation == 0.5 && w.wind.length() < 0.5));
    assert!(client.ok("/wind").starts_with("Wind 0.3 blocks/s"));
    assert_eq!(client.command("/wind hurricane")[0].0, ChatKind::Error);
}

#[test]
fn teleporting() {
    let mut client = Client::join();
    let start = client.position;
    client.ok("/tp ~ ~20 ~");
    assert_eq!(client.teleported, Some(start + Vec3::Y * 20.0));
    client.ok("/tp 100 150 -40");
    assert_eq!(client.position, Vec3::new(100.5, 150.0, -39.5));
    // To the ground somewhere far away, which loads it.
    client.ok("/tp 5000 5000");
    let (x, z) = (5000, 5000);
    client.pump_until(|c| c.world.contains(ColumnPos::of_block(IVec3::new(x, 0, z))));
    let feet = client.position.floor().as_ivec3();
    assert_eq!((feet.x, feet.z), (x, z));
    assert!(client.world.block(feet - IVec3::Y).is_some_and(|b| !b.is_air()), "lands on the ground");
    // Standing in the open, or among grass and flowers.
    let open = |b: BlockId| !client.content.blocks.info(b).solid;
    assert!(client.world.block(feet).is_some_and(open) && client.world.block(feet + IVec3::Y).is_some_and(open), "room to stand");
    assert_eq!(client.command("/tp nobody")[0].0, ChatKind::Error);
    // Biomes: where we stand, and the nearest ocean.
    // Named in words (Alpine Meadow, not base:alpine_meadow), with its tags and climate.
    let biome = client.ok("/biome");
    assert!(!biome.contains("base:") && !biome.contains('_') && biome.contains(" °C mean"), "{biome}");
    assert!(client.ok("/locate biome ocean 8000").contains("/tp "));
    assert_eq!(client.command("/locate biome nowhere")[0].0, ChatKind::Error);
    assert_eq!(client.command("/tp 1 2 3 4 5")[0].0, ChatKind::Error);
}

#[test]
fn only_admins_run_admin_commands_and_everyone_chats() {
    let server = spawn(ServerConfig { view_distance: 2, admins: vec!["boss".into()], ..Default::default() }, content()).unwrap();
    let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    let mut guest = Client::hello(tcp::connect(addr).unwrap(), "guest", server);
    let errors = guest.command("/time set noon");
    assert_eq!(errors, [(ChatKind::Error, "Only admins may use /time.".into())]);
    assert!(guest.ok("/seed").starts_with("Seed: "));
    assert!(!guest.ok("/help").contains("/weather"), "help lists only what the player may use");
    assert!(guest.ok("/list").starts_with("1 online: guest"));
    guest.connection.send(ClientMessage::Chat { text: "hello there".into() });
    guest.pump_until(|c| c.chat.iter().any(|(kind, text)| *kind == ChatKind::Chat && text == "<guest> hello there"));
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

#[test]
fn health_is_twice_hitpoints_and_a_downpour_drenches_those_out_in_it() {
    let mut client = Client::join();
    // Hitpoints starts at 10: 20 health.
    client.pump_until(|c| c.max_health == 20);
    let named = |c: &Client| c.vitals.effects.iter().map(|a| c.content.effects.get(a.effect).name.clone()).collect::<Vec<_>>();
    client.ok("/fill ~-1 ~ ~-1 ~1 ~50 ~1 air");
    client.ok("/weather rain heavy 600");
    client.pump_until(|c| named(c).iter().any(|n| n == "core:soaked" || n == "core:drenched"));
    // Under a roof, it stops.
    client.ok("/fill ~-1 ~3 ~-1 ~1 ~3 ~1 stone");
    client.pump_until(|c| !named(c).iter().any(|n| n == "core:soaked" || n == "core:drenched"));
}

/// Water placed by a command flows, and players see it flow through batched updates; the
/// commands that build and give blocks work for admins.
#[test]
fn water_flows_for_everyone_to_see() {
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let (x, y, z) = (feet.x, feet.y + 20, feet.z);
    let home = ColumnPos::of_block(feet);
    client.pump_until(|c| (-1..=1).all(|dz| (-1..=1).all(|dx| c.world.contains(home.offset(dx, dz)))));
    // A stone floor in the sky, with a water source on it.
    client.ok(&format!("/fill {} {} {} {} {} {} stone", x - 9, y, z - 9, x + 9, y, z + 9));
    client.pump_until(|c| c.world.block(IVec3::new(x + 9, y, z + 9)) == c.content.blocks.id("base:stone"));
    client.ok(&format!("/setblock {x} {} {z} water", y + 1));
    let water = client.content.blocks.expect("base:water");
    let flowing = |c: &Client, dx: i32| c.world.block(IVec3::new(x + dx, y + 1, z)).and_then(|b| c.content.blocks.fluid(b)).map(|s| s.amount);
    client.pump_until(|c| flowing(c, 7) == Some(1));
    assert_eq!(flowing(&client, 0), Some(8));
    assert_eq!(flowing(&client, 3), Some(5));
    assert_eq!(client.world.block(IVec3::new(x, y + 1, z)), Some(water));
    // Water runs off the edge of the floor and falls.
    client.ok(&format!("/setblock {} {} {z} air", x + 5, y));
    client.pump_until(|c| c.world.block(IVec3::new(x + 5, y - 1, z)).and_then(|b| c.content.blocks.fluid(b)).is_some_and(|s| s.falling));
    // Give hands out blocks to place.
    let answer = client.ok("/give water 3");
    assert!(answer.contains("Gave 3 Water"), "{answer}");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == water && s.count == 3));
    let errors = client.command("/setblock ~ ~ ~ no_such_block");
    assert_eq!(errors[0].0, ChatKind::Error);
}

#[test]
fn the_server_console_runs_commands_as_an_admin() {
    let server = spawn(ServerConfig { view_distance: 2, ..Default::default() }, content()).unwrap();
    let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    let mut guest = Client::hello(tcp::connect(addr).unwrap(), "guest", server);
    guest.pump_until(|c| c.day.is_some());
    // The console speaks for the server, and needs no admin rights.
    guest._server.as_ref().unwrap().console("/time set midnight");
    guest.pump_until(|c| c.day.is_some_and(|(day, _)| day % TICKS_PER_DAY == 20_000));
    guest._server.as_ref().unwrap().console("say hello");
    guest.pump_until(|c| c.chat.iter().any(|(_, text)| text == "[Server] hello"));
    guest._server.as_ref().unwrap().console("gamemode creative guest");
    guest.pump_until(|c| c.allow_flight);
}

#[test]
fn walking_wears_a_path() {
    let mut client = Client::join();
    let start = client.position;
    let feet = start.floor().as_ivec3();
    let solid = |c: &Client, p: IVec3| c.world.block(p).is_some_and(|b| c.content.blocks.info(b).solid);
    // A neighbouring block of ground at the same height to pace to and fro across.
    let step = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
        .into_iter()
        .find(|&d| solid(&client, feet + d - IVec3::Y) && !solid(&client, feet + d) && !solid(&client, feet + d + IVec3::Y))
        .expect("level ground beside spawn");
    let under = feet - IVec3::Y;
    let before = client.world.block(under).unwrap();
    let blocks = client.content.blocks.clone();
    let id = |name: &str| blocks.expect(&format!("base:{name}"));
    let worn = [id("worn_grass_block"), id("packed_earth"), id("coarse_dirt")];
    assert_eq!(before, id("grass_block"), "spawn is on {}, not grass", blocks.def(before).name);
    // Each crossing is a step onto the other block: forty steps on each.
    let (here, there) = (start + step.as_vec3() * 0.3, start + step.as_vec3() * 0.7);
    for i in 0..80 {
        client.send_position(if i % 2 == 0 { there } else { here }, false);
        std::thread::sleep(Duration::from_millis(110));
    }
    client.pump_until(|c| c.world.block(under).is_some_and(|b| worn.contains(&b)));
    // Plants on the path are trampled away.
    let plant = client.world.block(feet).unwrap();
    assert!(plant.is_air() || plant == id("short_grass"), "{}", blocks.def(plant).name);
}

#[test]
fn summoned_creatures_are_seen_struck_and_drop_their_loot() {
    strike_a_snail(Client::join());
}

/// As the gate's server runs: behind, with stalls, and a stall as every other action is
/// proposed. Blows still land and the snail still dies.
#[test]
fn creatures_are_struck_when_the_server_runs_behind() {
    let lag = voxmar_server::Lag {
        per_tick: Duration::from_millis(60),
        stall: Duration::from_millis(400),
        stall_every: 9,
        proposal_stall: Duration::from_millis(600),
    };
    strike_a_snail(Client::join_with(ServerConfig { view_distance: 2, lag, ..Default::default() }));
}

fn strike_a_snail(mut client: Client) {
    let snail = client.content.creatures.find("lumen_snail").expect("a lumen snail in the content");
    let slime = client.content.blocks.id("base:lumen_slime").unwrap();
    client.ok("/summon lumen_snail 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == snail));
    let (&id, view) = client.creatures.iter().find(|(_, v)| v.species == snail).unwrap();
    // Stand beside it, and strike until it dies: four health, a hit in two at level 1.
    // Each blow waits for the server's answer, so a server running behind only slows it.
    let mut at = view.position;
    client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
    while client.creatures.contains_key(&id) {
        assert!(client.blows < 40, "{} blows did not kill it", client.blows);
        at = client.creatures[&id].position;
        let blows = client.blows;
        client.propose("core:attack", ActionTarget::Creature(id));
        // The answer, or a later swing if the server was still cooling down.
        let asked = Instant::now();
        client.pump_until(|c| c.blows > blows || !c.creatures.contains_key(&id) || asked.elapsed() > Duration::from_millis(400));
    }
    assert_eq!(client.deaths, 1);
    // Walk onto where it died, as a player goes for the loot.
    client.ok(&format!("/tp {} {} {}", at.x, at.y, at.z));
    // Its slime pops out beside us, and we pick it up.
    client.pump_until(|c| (0..voxmar_engine::inventory::SLOTS).filter_map(|s| c.inventory.get(s)).any(|s| s.item == slime));
}

/// A weapon hits up to its max hit from the level that wields it; below that level it
/// strikes as a bare fist (max hit 1, and 1 more for every eight levels).
#[test]
fn a_sword_hits_harder_than_a_fist_from_the_level_that_wields_it() {
    let mut client = Client::join();
    let bear = client.content.creatures.find("brown_bear").expect("a brown bear in the content");
    let sword = client.content.blocks.id("base:iron_sword").unwrap();
    // Creative, so the bear's blows pass us by.
    client.ok("/gamemode creative");
    client.ok("/skill attack 19");
    client.ok("/give iron_sword 1");
    client.pump_until(|c| (0..voxmar_engine::inventory::HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == sword)));
    let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == sword)).unwrap();
    client.ok("/summon brown_bear 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear));
    let id = client.creatures.iter().find(|(_, v)| v.species == bear).map(|(&id, _)| id).unwrap();
    // Struck a full charge apart, beside it wherever it goes, until `landed` blows land.
    let strike = |client: &mut Client, landed: usize| {
        let start = client.damage.len();
        while client.damage[start..].iter().filter(|&&d| d > 0).count() < landed && client.creatures.contains_key(&id) {
            assert!(client.damage.len() - start < 80, "{} blows landed {landed}", client.damage.len() - start);
            let at = client.creatures[&id].position;
            client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
            client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot as u8, flying: false }));
            std::thread::sleep(Duration::from_millis(650));
            let blows = client.blows;
            client.propose("core:attack", ActionTarget::Creature(id));
            let asked = Instant::now();
            client.pump_until(|c| c.blows > blows || !c.creatures.contains_key(&id) || asked.elapsed() > Duration::from_millis(400));
        }
        client.damage[start..].iter().copied().filter(|&d| d > 0).collect::<Vec<u16>>()
    };
    // Attack 19, a level short of iron: a fist's max hit, 1 + 19 / 8 = 3.
    let fists = strike(&mut client, 6);
    assert!(fists.iter().all(|&d| d <= 3), "below the sword's level, blows did {fists:?}");
    client.ok("/skill attack 20");
    let swung = strike(&mut client, 8);
    assert!(swung.iter().all(|&d| d <= 9) && swung.iter().any(|&d| d > 3), "with the sword, blows did {swung:?}");
}

#[test]
fn creatures_are_tamed_by_feeding_and_kept_ones_breed() {
    let mut client = Client::join();
    let hen = client.content.creatures.find("junglefowl").expect("junglefowl in the content");
    let wheat = client.content.blocks.id("base:wheat").unwrap();
    client.ok("/summon junglefowl 2 ~ ~ ~");
    client.ok("/give wheat 64");
    client.pump_until(|c| c.creatures.values().filter(|v| v.species == hen).count() == 2 && c.inventory.get(4).is_some_and(|s| s.item == wheat));
    // Hold the wheat (the start kit fills the first four slots).
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 4, flying: false }));
    let hens: Vec<u32> = client.creatures.iter().filter(|(_, v)| v.species == hen).map(|(&id, _)| id).collect();
    // Feed each until it is ours: three to five feedings at level 1.
    for &id in &hens {
        let mut fed = 0;
        while !client.creatures[&id].kept {
            assert!(fed < 12, "{fed} feedings did not tame it");
            let at = client.creatures[&id].position;
            client.ok(&format!("/tp {} {} {}", at.x + 0.8, at.y, at.z));
            client.propose("base:tend", ActionTarget::Creature(id));
            fed += 1;
            let asked = Instant::now();
            client.pump_until(|c| c.creatures[&id].kept || asked.elapsed() > Duration::from_millis(300));
        }
        assert_eq!(client.creatures[&id].owner, Some(client.id));
    }
    // Fed again, the two are ready, come together and a young one is born.
    for &id in &hens {
        let at = client.creatures[&id].position;
        client.ok(&format!("/tp {} {} {}", at.x + 0.8, at.y, at.z));
        client.propose("base:tend", ActionTarget::Creature(id));
        client.pump_until(|c| c.creatures[&id].ready || !c.creatures.values().any(|v| v.young));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !client.creatures.values().any(|v| v.young && v.kept) {
        assert!(Instant::now() < deadline, "no young one was born");
        let until = Instant::now() + Duration::from_millis(200);
        client.pump_until(|_| Instant::now() > until);
    }
    // The chick inherits its plumage from its parents: each of its two alleles is one of
    // theirs (no mutation is that likely here).
    let genes = &client.content.creatures.get(hen).def.genes;
    assert!(!genes.is_empty(), "junglefowl have genes");
    let alleles = |v: &voxmar_protocol::CreatureView| match voxmar_engine::genes::unpack(genes, &v.genes).0[0] {
        voxmar_engine::genes::Gene::Pair(a, b) => [a, b],
        _ => panic!("plumage is an allele gene"),
    };
    let parents: Vec<u8> = hens.iter().flat_map(|id| alleles(&client.creatures[id])).collect();
    let chick = client.creatures.values().find(|v| v.young && v.kept).expect("born");
    assert!(alleles(chick).iter().all(|a| parents.contains(a)), "{:?} from parents {parents:?}", alleles(chick));
}

#[test]
fn looking_closely_shows_more_with_farming() {
    let mut client = Client::join();
    let hen = client.content.creatures.find("junglefowl").expect("junglefowl in the content");
    client.ok("/summon junglefowl 1 ~ ~ ~-1");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == hen));
    let id = *client.creatures.iter().find(|(_, v)| v.species == hen).unwrap().0;
    let look = |client: &mut Client| -> String {
        let at = client.creatures[&id].position;
        client.ok(&format!("/tp {} {} {}", at.x + 0.8, at.y, at.z));
        client.ok("/creatures hold");
        client.chat.clear();
        client.propose("base:inspect", ActionTarget::Creature(id));
        client.pump_until(|c| c.chat.iter().any(|(_, text)| text.contains("level 1")));
        client.chat.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join("\n")
    };
    // At Farming 1: what anyone sees, and that more comes with level.
    let first = look(&mut client);
    assert!(first.contains("health") && first.contains("Farming 10 shows more"), "{first}");
    assert!(!first.contains("Shows:"), "{first}");
    // At 40, everything: what shows, what it carries, the numbers and the odds.
    client.ok("/skill farming 40");
    let all = look(&mut client);
    for words in ["Shows:", "Carries", "Exactly:", "mutates"] {
        assert!(all.contains(words), "{words} missing from {all}");
    }
}

#[test]
fn a_break_completes_on_time_by_the_clock_when_the_server_runs_behind_on_ticks() {
    // About nine ticks a second instead of twenty: judged by its ticks, stone would take
    // the server more than twice its time, long after the player saw it break.
    let lag = voxmar_server::Lag { per_tick: Duration::from_millis(60), ..Default::default() };
    let mut client = Client::join_with(ServerConfig { view_distance: 2, lag, ..Default::default() });
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.ok("/setblock ~ ~-1 ~ stone");
    client.pump_until(|c| c.world.block(below) == c.content.blocks.id("base:stone"));
    let hardness = client.content.blocks.info(client.world.block(below).unwrap()).hardness;
    let started = Instant::now();
    client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    let took = started.elapsed().as_secs_f32();
    assert!(took >= hardness - 0.05 && took <= hardness + 0.4, "stone ({hardness} s) broke after {took:.2} s");
}

#[test]
fn placing_or_throwing_while_mining_does_not_cancel_the_mining() {
    // Instant actions run alongside a timed one: a block placed or an item thrown halfway
    // through a break must not cost the break the player then sees finish.
    let mut client = Client::join_with(ServerConfig { view_distance: 2, lag: SLOW, ..Default::default() });
    let feet = client.position.floor().as_ivec3();
    let (a, b) = (feet + IVec3::new(0, -1, 1), feet + IVec3::new(0, -1, 2));
    client.ok("/fill ~ ~-1 ~1 ~ ~-1 ~2 stone");
    client.pump_until(|cl| [a, b].into_iter().all(|p| cl.world.block(p) == cl.content.blocks.id("base:stone")));
    let hardness = client.content.blocks.info(client.world.block(a).unwrap()).hardness;
    for (block, instant) in [(a, "core:drop"), (b, "core:place")] {
        let target = ActionTarget::Block { block, face: Face::PosY };
        client.propose("core:mine", target);
        std::thread::sleep(Duration::from_secs_f32(hardness * 0.5));
        match instant {
            "core:drop" => client.propose(instant, ActionTarget::Stack { slot: 0, count: 1 }),
            _ => client.propose(instant, ActionTarget::Block { block: feet + IVec3::new(2, -1, 0), face: Face::PosY }),
        }
        std::thread::sleep(Duration::from_secs_f32(hardness * 0.5));
        client.connection.send(ClientMessage::Finished { target });
    }
    client.pump_until(|cl| cl.world.block(a) == Some(BlockId::AIR) && cl.world.block(b) == Some(BlockId::AIR));
}

#[test]
fn a_finished_break_the_server_refused_is_undone_on_the_client() {
    // The client shows a break done; if the server refused it (here: out of reach), the
    // client must be told what is really there rather than keep a block only it broke.
    let mut client = Client::join();
    let far = client.position.floor().as_ivec3() + IVec3::new(0, -1, 9);
    client.ok("/setblock ~ ~-1 ~9 stone");
    client.pump_until(|c| c.world.block(far) == c.content.blocks.id("base:stone"));
    let stone = client.world.block(far).unwrap();
    let target = ActionTarget::Block { block: far, face: Face::PosY };
    client.propose("core:mine", target);
    client.settle();
    client.world.set_block(far, BlockId::AIR);
    client.connection.send(ClientMessage::Finished { target });
    client.pump_until(|c| c.world.block(far) == Some(stone));
}

/// A server held up as on a busy machine: every tick slow, every fifth stalled, and a
/// stall as every other action is proposed (as `voxmar_client::gate::SERVER_LAG`).
const SLOW: voxmar_server::Lag = voxmar_server::Lag {
    per_tick: Duration::from_millis(40),
    stall: Duration::from_millis(300),
    stall_every: 5,
    proposal_stall: Duration::from_millis(400),
};

#[test]
fn ores_ask_for_a_mining_level_and_a_pickaxe_and_teach_mining() {
    let mut client = Client::join();
    let ore = client.position.floor().as_ivec3() + IVec3::new(0, -1, 1);
    client.ok("/setblock ~ ~-1 ~1 iron_ore");
    client.ok("/give stone_pickaxe 1");
    client.pump_until(|c| c.world.block(ore) == c.content.blocks.id("base:iron_ore") && c.inventory.get(4).is_some());
    let iron = client.world.block(ore).unwrap();
    let mine = |client: &Client| client.propose("core:mine", ActionTarget::Block { block: ore, face: Face::PosY });
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };
    // How long a break takes, in server ticks, with the pickaxe and by hand: a refusal
    // waits that long and more before it says nothing broke.
    let hardness = client.content.blocks.info(iron).hardness;
    let pickaxe = client.content.blocks.expect("base:stone_pickaxe");
    let ticks = |speed: f32| (hardness / speed * TICKS_PER_SECOND as f32).ceil() as u64;
    let (with_pickaxe, by_hand) = (ticks(client.content.tools.speed(Some(pickaxe), iron)), ticks(client.content.tools.speed(None, iron)));
    // At Mining 1, even with the pickaxe: refused.
    hold(&client, 4);
    mine(&client);
    client.settle();
    client.after_ticks(with_pickaxe + 20);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke at Mining 1");
    // At Mining 15 with a bare hand: refused too.
    client.ok("/skill mining 15");
    hold(&client, 5);
    mine(&client);
    client.settle();
    client.after_ticks(by_hand + 20);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke without a pickaxe");
    // At Mining 15 with the stone pickaxe: broken, quicker than by hand (by the server's
    // clock, told once a second), teaching Mining (35).
    hold(&client, 4);
    let told = client.server_tick;
    client.pump_until(|c| c.server_tick > told);
    let started = client.server_tick;
    mine(&client);
    client.pump_until(|c| c.world.block(ore) == Some(BlockId::AIR) && c.earned.contains(&350));
    let seen = client.server_tick;
    client.pump_until(|c| c.server_tick > seen);
    let took = client.server_tick - started;
    assert!(took < by_hand, "a stone pickaxe took {took} ticks to mine iron ore, a hand's {by_hand}");
}

#[test]
fn a_crafting_table_makes_what_the_two_by_two_grid_cannot() {
    let mut client = Client::join();
    let table = client.position.floor().as_ivec3() + IVec3::new(1, 0, 1);
    for line in ["/clear", "/give cobblestone 3", "/give stick 2", "/fill ~1 ~ ~1 ~1 ~ ~1 crafting_table"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(table) == c.content.blocks.id("base:crafting_table"));
    let pickaxe = client.content.blocks.expect("base:stone_pickaxe");
    let lay = |client: &Client| {
        for (slot, cell) in [(0u8, 0u8), (0, 1), (0, 2), (1, 4), (1, 7)] {
            let to = voxmar_engine::inventory::CRAFT as u8 + cell;
            client.propose("core:transfer", ActionTarget::Slots { from: slot, to, count: 1 });
        }
        client.propose("core:craft", ActionTarget::Slot(0));
    };
    // Away from a table the three-by-three pattern makes nothing.
    lay(&client);
    // Handled, and the inventory told (it goes out as the tick ends).
    client.settle();
    client.after_ticks(2);
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == pickaxe), "a pickaxe was crafted without a table");
    // At the table it makes a stone pickaxe, teaching Engineering.
    client.propose("base:use_table", ActionTarget::Block { block: table, face: Face::PosY });
    client.propose("core:craft", ActionTarget::Slot(0));
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == pickaxe) && c.earned.contains(&60));
}

#[test]
fn cooked_food_takes_a_slot_each_and_ingredients_stack() {
    let mut client = Client::join();
    for line in ["/clear", "/give bread 3", "/give wheat 3"] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (bread, wheat) = (blocks.expect("base:bread"), blocks.expect("base:wheat"));
    client.pump_until(|c| c.inventory.slots.iter().flatten().filter(|s| s.item == bread).count() == 3 && c.inventory.slots.iter().flatten().any(|s| s.item == wheat));
    assert!(client.inventory.slots.iter().flatten().all(|s| s.item != bread || s.count == 1), "bread stacked");
    assert_eq!(client.inventory.slots.iter().flatten().filter(|s| s.item == wheat).map(|s| s.count).collect::<Vec<_>>(), [3], "wheat should stack");
}

#[test]
fn a_rod_cast_into_water_catches_fish_by_level_and_teaches_fishing() {
    let mut client = Client::join();
    let water = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    for line in ["/fill ~-1 ~-2 ~1 ~1 ~-1 ~3 water", "/fill ~-1 ~ ~1 ~1 ~3 ~3 air", "/give fishing_rod 1", "/skill fishing 80"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(water).is_some_and(|b| c.content.blocks.info(b).fluid.is_some()) && c.inventory.get(4).is_some());
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 4, flying: false }));
    let fish = |c: &Client| c.inventory.slots.iter().flatten().any(|s| c.content.blocks.def(s.item).has_tag("fish"));
    // Nine casts in ten land at level 80; a few casts make a miss streak vanishingly rare.
    for _ in 0..6 {
        client.propose("base:fish", ActionTarget::Block { block: water, face: Face::PosY });
        let up = Instant::now() + Duration::from_millis(5600);
        client.pump_until(|c| fish(c) || Instant::now() > up);
        if fish(&client) {
            break;
        }
    }
    assert!(fish(&client), "six casts at Fishing 80 caught nothing");
    client.pump_until(|c| !c.earned.is_empty());
}

#[test]
fn a_seed_pouch_carries_seeds_in_one_slot_and_sows_from_it() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    for line in ["/clear", "/give seed_pouch 1", "/give wheat_seeds 100", "/fill ~ ~-1 ~2 ~ ~-1 ~2 farmland", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    let seeds = client.content.blocks.expect("base:wheat_seeds");
    let sprout = client.content.blocks.expect("base:wheat_crop_0");
    client.pump_until(|c| c.inventory.pouched(seeds) == 100 && c.world.block(ground) == c.content.blocks.id("base:farmland"));
    assert!(client.inventory.slots.iter().flatten().all(|s| s.item != seeds), "seeds took slots with a pouch carried");
    // The pouch in hand sows its seeds.
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout) && c.inventory.pouched(seeds) == 99);
    // Right click empties it into the pack.
    client.propose("base:empty_pouch", ActionTarget::None);
    client.pump_until(|c| c.inventory.pouched(seeds) == 0 && c.inventory.slots.iter().flatten().filter(|s| s.item == seeds).map(|s| s.count as u32).sum::<u32>() == 99);
}

#[test]
fn the_dead_keep_three_items_and_only_they_may_pick_up_the_rest() {
    let mut owner = Client::join_remote("owner", &["owner"]);
    let mut thief = owner.another("thief");
    for line in ["/clear", "/give iron_pickaxe 1", "/give iron_ingot 5", "/give dirt 30", "/give stick 10", "/give bread 1"] {
        owner.ok(line);
    }
    let carried = |c: &Client| c.inventory.slots.iter().flatten().map(|s| s.count as u32).sum::<u32>();
    owner.pump_until(|c| carried(c) == 47);
    let said = owner.ok("/kill");
    owner.pump_until(|c| c.health == 0 && carried(c) == 3 && c.items.values().map(|i| i.stack.count as u32).sum::<u32>() == 44);
    let pickaxe = owner.content.blocks.expect("base:iron_pickaxe");
    assert!(owner.inventory.slots.iter().flatten().any(|s| s.item == pickaxe), "the pickaxe is the most valuable: {:?}", owner.inventory);
    assert!(said.contains("for 30 minutes") || owner.chat.iter().any(|(_, text)| text.contains("for 30 minutes")), "{said} {:?}", owner.chat);
    // The thief stands where the owner fell (both joined at spawn) and picks up nothing.
    let watch = Instant::now() + Duration::from_secs(3);
    thief.pump_until(|c| c.items.values().map(|i| i.stack.count as u32).sum::<u32>() == 44 && Instant::now() > watch);
    assert!(thief.collected.is_empty(), "someone else picked up a dead player's things");
    // The owner wakes at spawn, by their things, and gathers them all.
    owner.propose("core:respawn", ActionTarget::None);
    owner.pump_until(|c| c.health > 0);
    // Walking to whatever still lies about, as a player gathers it; a stack that rolled a
    // little way is out of reach from the spot itself. Each round waits on the server.
    for _ in 0..12 {
        let wait = Instant::now() + Duration::from_secs(5);
        owner.pump_until(|c| carried(c) == 47 || Instant::now() > wait);
        if carried(&owner) == 47 {
            break;
        }
        if let Some(at) = owner.items.values().map(|i| i.position).next() {
            owner.ok(&format!("/tp {} {} {}", at.x, at.y, at.z));
        }
    }
    let lying: Vec<_> = owner.items.values().map(|i| (i.stack.count, i.position)).collect();
    assert_eq!(carried(&owner), 47, "the owner at {:?} could not gather {lying:?}", owner.position);
}

#[test]
fn picked_wild_flowers_grow_back_unless_their_ground_is_broken() {
    let mut client = Client::join();
    let (near, far) = (client.position.floor().as_ivec3() + IVec3::new(2, 0, 0), client.position.floor().as_ivec3() + IVec3::new(-2, 0, 0));
    for line in ["/fill ~-2 ~-1 ~ ~2 ~-1 ~ grass_block", "/fill ~-2 ~ ~ ~2 ~ ~ air", "/setblock ~2 ~ ~ poppy", "/setblock ~-2 ~ ~ poppy"] {
        client.ok(line);
    }
    let poppy = client.content.blocks.expect("base:poppy");
    client.pump_until(|c| c.world.block(near) == Some(poppy) && c.world.block(far) == Some(poppy));
    for at in [near, far] {
        client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
        client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
    }
    // Both went into the journal, as did the biome stood in.
    use voxmar_engine::journal::Page;
    client.pump_until(|c| c.journal.get(Page::Plants, "base:poppy").is_some_and(|e| e.count == 2) && c.journal.found(Page::Biomes) > 0);
    // The far one's ground is dug out and put back: a different block, so it is gone.
    client.ok("/setblock ~-2 ~-1 ~ dirt");
    client.ok("/grow");
    client.pump_until(|c| c.world.block(near) == Some(poppy));
    std::thread::sleep(Duration::from_millis(1500));
    client.pump_until(|_| true);
    assert_eq!(client.world.block(far), Some(BlockId::AIR), "a flower came back on ground that was replaced");
}

#[test]
fn a_lit_homestead_hearth_is_home_and_the_dead_wake_there() {
    let mut client = Client::join_remote("owner", &["owner"]);
    let core = client.position.floor().as_ivec3() + IVec3::new(4, 0, 0);
    for line in [
        "/fill ~3 ~ ~-1 ~5 ~3 ~1 air",
        "/fill ~3 ~ ~-1 ~5 ~ ~1 cobblestone",
        "/setblock ~4 ~ ~ hearthstone",
    ] {
        client.ok(line);
    }
    let hearthstone = client.content.blocks.expect("base:hearthstone");
    client.pump_until(|c| c.world.block(core) == Some(hearthstone));
    let light = |client: &mut Client| {
        client.chat.clear();
        client.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
        client.pump_until(|c| c.chat.iter().any(|(_, t)| t.to_lowercase().contains("hearth")));
        std::mem::take(&mut client.chat).into_iter().map(|(_, t)| t).collect::<String>()
    };
    assert!(light(&mut client).contains("needs"), "no chimney yet");
    client.ok("/fill ~5 ~1 ~1 ~5 ~2 ~1 chimney");
    assert!(light(&mut client).contains("your home"));
    // Dying far off, they wake on the hearthstone.
    client.ok("/tp ~-20 ~ ~");
    client.ok("/kill");
    client.pump_until(|c| c.health == 0);
    client.propose("core:respawn", ActionTarget::None);
    client.pump_until(|c| c.health > 0 && c.teleported.is_some_and(|p| p.distance(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5)) < 0.1));
}

#[test]
fn quests_finish_from_what_was_done_and_reward_it() {
    let mut client = Client::join();
    let done = |c: &Client, name: &str| c.quests.iter().any(|q| q.name == name && q.done);
    client.pump_until(|c| !c.quests.is_empty());
    assert!(!done(&client, "base:crafting_table"));
    // Four planks in the hand's grid make a crafting table: the quest asking for one is
    // done, and its Engineering comes with it.
    for line in ["/clear", "/give oak_planks 4"] {
        client.ok(line);
    }
    // The four given, not the starting kit's planks still there before the clear.
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.count == 4));
    for cell in voxmar_engine::craft::SMALL {
        let to = (voxmar_engine::inventory::CRAFT + cell) as u8;
        client.propose("core:transfer", ActionTarget::Slots { from: 0, to, count: 1 });
    }
    client.propose("core:craft", ActionTarget::Slot(0));
    client.chat.clear();
    client.pump_until(|c| done(c, "base:crafting_table") && c.chat.iter().any(|(_, t)| t.contains("Quest done: Crafting Table")));
}

#[test]
fn a_crucible_smelts_only_in_its_lit_kiln_and_makes_bronze() {
    let mut client = Client::join();
    let core = client.position.floor().as_ivec3() + IVec3::new(4, 0, 0);
    for line in ["/clear", "/fill ~3 ~ ~-1 ~5 ~2 ~1 air", "/setblock ~4 ~ ~ crucible", "/give copper_ore 2", "/skill engineering 20"] {
        client.ok(line);
    }
    let crucible = client.content.blocks.expect("base:crucible");
    let (ore, ingot) = (client.content.blocks.expect("base:copper_ore"), client.content.blocks.expect("base:copper_ingot"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    // The ore given, in slot 0 once the clear has emptied the starting kit.
    client.pump_until(|c| c.world.block(core) == Some(crucible) && c.inventory.get(0).is_some_and(|s| s.item == ore && s.count == 2));
    let smelt = |client: &mut Client| {
        client.chat.clear();
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
        client.propose("base:cook", ActionTarget::Block { block: core, face: Face::PosY });
    };
    // A crucible alone smelts nothing, and says what the kiln needs.
    smelt(&mut client);
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("kiln needs")));
    assert_eq!(count(&client, ore), 2);
    // Ringed with kiln bricks and a pillar on each corner, it smelts.
    for line in ["/fill ~3 ~ ~-1 ~5 ~ ~1 kiln_bricks", "/setblock ~4 ~ ~ crucible", "/setblock ~3 ~1 ~-1 kiln_bricks", "/setblock ~5 ~1 ~-1 kiln_bricks", "/setblock ~3 ~1 ~1 kiln_bricks", "/setblock ~5 ~1 ~1 kiln_bricks"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(core) == Some(crucible) && c.world.block(core + IVec3::new(1, 1, 1)) == c.content.blocks.id("base:kiln_bricks"));
    client.chat.clear();
    client.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Crucible Kiln is lit")));
    smelt(&mut client);
    client.pump_until(|c| count(c, ingot) == 1 && count(c, ore) == 1);
}

#[test]
fn a_sleeping_bag_is_where_the_homeless_wake() {
    let mut client = Client::join_remote("sleeper", &["sleeper"]);
    let bag = client.position.floor().as_ivec3() + IVec3::new(3, 0, 0);
    for line in ["/fill ~3 ~ ~ ~3 ~1 ~ air", "/setblock ~3 ~ ~ sleeping_bag"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag"));
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("wake here")));
    client.ok("/tp ~-20 ~ ~");
    client.ok("/kill");
    client.pump_until(|c| c.health == 0);
    client.propose("core:respawn", ActionTarget::None);
    client.pump_until(|c| c.health > 0 && c.teleported.is_some_and(|p| p.distance(bag.as_vec3() + Vec3::new(0.5, 1.0, 0.5)) < 0.1));
}

#[test]
fn gravel_sometimes_breaks_to_flint_instead() {
    let mut client = Client::join();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    for line in ["/clear", "/give iron_shovel 1"] {
        client.ok(line);
    }
    let (gravel, flint) = (client.content.blocks.expect("base:gravel"), client.content.blocks.expect("base:flint"));
    let shovel = client.content.blocks.expect("base:iron_shovel");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == shovel));
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    // One break in eight gives flint: in sixty, all but certainly some.
    let mut flints = 0;
    for _ in 0..60 {
        client.ok("/setblock ~ ~-1 ~ gravel");
        client.pump_until(|c| c.world.block(below) == Some(gravel));
        client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
        client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
        // The drop comes just after the block goes; setting the gravel again at once could
        // bury it unseen.
        let seen = Instant::now() + Duration::from_millis(300);
        client.pump_until(|c| Instant::now() > seen || c.items.values().any(|i| i.stack.item == flint));
        flints = client.items.values().filter(|i| i.stack.item == flint).count() + client.inventory.slots.iter().flatten().filter(|s| s.item == flint).count();
        if flints > 0 {
            break;
        }
    }
    assert!(flints > 0, "sixty gravel broke without a flint");
}

#[test]
fn a_ripe_crop_above_the_farming_level_refuses_harvest() {
    let mut client = Client::join();
    let at = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    // Ripe tomatoes ask for Farming 30.
    for line in ["/fill ~2 ~-1 ~ ~2 ~-1 ~ farmland", "/fill ~2 ~ ~ ~2 ~1 ~ air", "/setblock ~2 ~ ~ tomato_crop"] {
        client.ok(line);
    }
    let ripe = client.content.blocks.expect("base:tomato_crop");
    client.pump_until(|c| c.world.block(at) == Some(ripe));
    let earned = client.earned.len();
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.propose("base:pick", ActionTarget::Block { block: at, face: Face::PosY });
    client.settle();
    assert_eq!(client.world.block(at), Some(ripe), "a level 1 farmer harvested Farming 30 tomatoes");
    assert_eq!(client.earned.len(), earned, "no experience for a refused harvest");
    // Nor by breaking the ground under it, which would take the crop with it.
    client.propose("core:mine", ActionTarget::Block { block: at - IVec3::Y, face: Face::PosY });
    client.settle();
    assert_eq!(client.world.block(at), Some(ripe), "breaking the ground harvested Farming 30 tomatoes");
    assert_eq!(client.world.block(at - IVec3::Y), client.content.blocks.id("base:farmland"), "the ground under a refused crop broke");
    assert_eq!(client.earned.len(), earned, "no experience for the ground under a refused harvest");
    // At Farming 30 it comes.
    client.ok("/skill farming 30");
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) != Some(ripe) && c.earned.len() > earned);
}

#[test]
fn breaking_a_sprout_teaches_nothing_and_sowing_pays_at_harvest() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    for line in ["/clear", "/give wheat_seeds 4", "/fill ~ ~-1 ~2 ~ ~-1 ~2 farmland", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    let (seeds, sprout) = (client.content.blocks.expect("base:wheat_seeds"), client.content.blocks.expect("base:wheat_crop_0"));
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == seeds) && c.world.block(ground) == c.content.blocks.id("base:farmland"));
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    let earned = client.earned.len();
    for _ in 0..3 {
        client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout));
        client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    }
    // Each sprout gave its seed back: the four are all there, held or on the ground.
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == seeds).map(|s| s.count as usize).sum::<usize>();
    client.pump_until(|c| held(c) + c.items.values().filter(|i| i.stack.item == seeds).map(|i| i.stack.count as usize).sum::<usize>() == 4);
    client.settle();
    assert_eq!(client.earned.len(), earned, "sowing and breaking sprouts paid experience");
}

#[test]
fn sowing_where_a_crop_cannot_grow_is_refused_with_why() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    // Rice grows only with water close by: none within reach of this plot.
    for line in ["/clear", "/skill farming 10", "/give rice 2", "/fill ~-5 ~-3 ~-3 ~5 ~-1 ~7 dirt", "/fill ~ ~-1 ~2 ~ ~-1 ~2 farmland", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    let rice = client.content.blocks.expect("base:rice");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == rice) && c.world.block(ground) == c.content.blocks.id("base:farmland"));
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.chat.clear();
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| !c.chat.is_empty());
    let (kind, text) = client.chat[0].clone();
    assert_eq!(kind, ChatKind::Info);
    assert!(text.contains("water close by") || text.contains("here for"), "refused sowing said {text:?}");
    // Named as the player knows it: the rice they hold, not a growing stage.
    assert!(text.starts_with("Rice ") && !text.contains("Crop"), "refused sowing named the crop as {text:?}");
    client.settle();
    assert_eq!(client.world.block(ground + IVec3::Y), Some(BlockId::AIR), "rice was sown where it cannot grow");
    assert_eq!(client.inventory.get(0).map(|s| s.count), Some(2), "a refused sowing took the seed");
}

#[test]
fn a_field_nobody_sowed_gives_little_and_a_sown_crop_its_seed() {
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    // A village's field, as the world grows it: unripe potatoes nobody sowed, 28 of them.
    for line in ["/clear", "/skill farming 12", "/give potato 1", "/fill ~-3 ~-1 ~1 ~3 ~-1 ~4 farmland", "/fill ~-3 ~ ~1 ~3 ~1 ~4 air", "/fill ~-3 ~ ~1 ~3 ~ ~4 potato_crop_1"] {
        client.ok(line);
    }
    let (potato, unripe) = (client.content.blocks.expect("base:potato"), client.content.blocks.expect("base:potato_crop_1"));
    let field: Vec<IVec3> = (1..=4).flat_map(|dz| (-3..=3).map(move |dx| feet + IVec3::new(dx, 0, dz))).collect();
    client.pump_until(|c| field.iter().all(|&p| c.world.block(p) == Some(unripe)) && c.inventory.get(0).is_some());
    let potatoes = |c: &Client| {
        c.inventory.slots.iter().flatten().filter(|s| s.item == potato).map(|s| s.count as usize).sum::<usize>()
            + c.items.values().filter(|i| i.stack.item == potato).map(|i| i.stack.count as usize).sum::<usize>()
    };
    for &cell in &field {
        client.propose("core:mine", ActionTarget::Block { block: cell, face: Face::PosY });
        client.pump_until(|c| c.world.block(cell) == Some(BlockId::AIR));
    }
    // About one in seven (`unsown` 0.15): enough to start a field, not 28 potatoes. One
    // potato a success, so 13 or more is a one-in-13,000 chance.
    let found = client.quiet(potatoes) - 1;
    assert!(found <= 12, "a field nobody sowed gave {found} potatoes of {}", field.len());
    // Ripe, as villages grow them: now and then a harvest, and never its experience.
    let earned = client.earned.len();
    let ripe = client.content.blocks.expect("base:potato_crop");
    client.ok("/fill ~-3 ~ ~1 ~3 ~ ~4 potato_crop");
    client.pump_until(|c| field.iter().all(|&p| c.world.block(p) == Some(ripe)));
    let before = client.quiet(potatoes);
    for &cell in &field {
        client.propose("core:mine", ActionTarget::Block { block: cell, face: Face::PosY });
        client.pump_until(|c| c.world.block(cell) == Some(BlockId::AIR));
    }
    // A harvest (2 to 4 potatoes) a success: about 4 of 28 at 0.15, some 13 potatoes; 41 or
    // more takes 11 successes or so, under one in 10,000. Every crop would give at least 56.
    let found = client.quiet(potatoes) - before;
    assert!(found <= 40, "a ripe field nobody sowed gave {found} potatoes from {} crops", field.len());
    assert_eq!(client.earned.len(), earned, "harvesting a field nobody sowed paid experience");
    // One the player sows gives its potato back, every time.
    let before = client.quiet(potatoes);
    let slot = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == potato)).unwrap() as u8;
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    let ground = feet + IVec3::new(0, -1, 1);
    let sprout = client.content.blocks.expect("base:potato_crop_0");
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    // Sown, and the potato seen gone: block changes go out at once but the inventory only
    // as the tick ends, so a sprout seen (and broken) within one tick once left the sown
    // potato showing in hand. When it was the only one (the field's drops not yet picked
    // up, on a loaded machine), the second sowing then held nothing and was refused.
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout) && potatoes(c) == before - 1);
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    // At least, so a count that overshoots fails on the exact check below, not a timeout.
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR) && potatoes(c) >= before);
    assert_eq!(client.quiet(potatoes), before, "a sown sprout broken gave other than its potato back");
    // Sown, grown ripe and harvested: the harvest's experience with the sowing's.
    let earned = client.earned.len();
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == potato));
    let slot = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == potato)).unwrap() as u8;
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout));
    client.ok("/grow ripe");
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(ripe));
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    let xp = client.content.crops.iter().find(|(_, c)| c.def.name == "base:potato").unwrap().1.def.xp;
    let paid = ((xp.harvest + xp.plant) * 10.0).round() as u32;
    client.pump_until(|c| c.earned.len() > earned);
    assert_eq!(client.earned[earned..], [paid], "a sown potato's harvest paid other than its harvest and sowing");
}

#[test]
fn a_wild_ripe_crop_forages_at_its_level_and_a_field_stage_rolls() {
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let at = feet + IVec3::new(0, 0, 2);
    // A ripe tomato nobody sowed, as plants.ron grows them wild: Farming 30.
    for line in ["/clear", "/skill farming 29", "/fill ~-3 ~-1 ~1 ~3 ~-1 ~4 farmland", "/fill ~-3 ~ ~1 ~3 ~1 ~4 air", "/setblock ~ ~ ~2 tomato_crop"] {
        client.ok(line);
    }
    let (ripe, regrowing, seeds) = {
        let blocks = &client.content.blocks;
        (blocks.expect("base:tomato_crop"), blocks.expect("base:tomato_crop_2"), blocks.expect("base:tomato_seeds"))
    };
    let tomato = client.content.crops.iter().find(|(_, c)| c.def.name == "base:tomato").unwrap().1.def.clone();
    let forage_xp = (tomato.xp.forage * 10.0).round() as u32;
    client.pump_until(|c| c.world.block(at) == Some(ripe));
    let earned = client.earned.len();
    // Below its level it is refused, picked or broken: nothing foraged, nothing taught.
    client.propose("base:pick", ActionTarget::Block { block: at, face: Face::PosY });
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.settle();
    assert_eq!(client.world.block(at), Some(ripe), "a wild tomato was taken below its level");
    assert_eq!(client.earned.len(), earned, "a refused wild tomato taught something");
    // At its level, picked: its forage and exactly the forage's experience, no harvest's
    // or sowing's.
    client.ok("/skill farming 30");
    client.propose("base:pick", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(regrowing));
    client.settle();
    assert_eq!(client.earned[earned..], [forage_xp], "a wild tomato paid other than its forage experience");
    // It grows back, still nobody's, and forages again.
    client.ok("/grow ripe");
    client.pump_until(|c| c.world.block(at) == Some(ripe));
    client.propose("base:pick", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(regrowing));
    client.settle();
    assert_eq!(client.earned[earned..], [forage_xp, forage_xp], "a regrown wild tomato paid other than its forage experience");
    // The last stage a village field grows (never ripe), broken: its seed now and then,
    // and no experience.
    client.ok("/fill ~-3 ~ ~1 ~3 ~ ~4 air");
    let field_stage = &tomato.stages[tomato.stages.len() - 2];
    client.ok(&format!("/fill ~-3 ~ ~1 ~3 ~ ~4 {}", field_stage.trim_start_matches("base:")));
    let field: Vec<IVec3> = (1..=4).flat_map(|dz| (-3..=3).map(move |dx| feet + IVec3::new(dx, 0, dz))).collect();
    client.pump_until(|c| field.iter().all(|&p| c.world.block(p) == Some(regrowing)));
    let earned = client.earned.len();
    // Held, in a pouch, or on the ground.
    let seeds_had = |c: &Client| {
        c.inventory.slots.iter().flatten().filter(|s| s.item == seeds).map(|s| s.count as usize).sum::<usize>()
            + c.inventory.pouched(seeds) as usize
            + c.items.values().filter(|i| i.stack.item == seeds).map(|i| i.stack.count as usize).sum::<usize>()
    };
    let before = client.quiet(seeds_had);
    for &cell in &field {
        client.propose("core:mine", ActionTarget::Block { block: cell, face: Face::PosY });
        client.pump_until(|c| c.world.block(cell) == Some(BlockId::AIR));
    }
    // One seed a success: about 4 of 28 at 0.15; 13 or more is a one-in-13,000 chance.
    let found = client.quiet(seeds_had).saturating_sub(before);
    assert!(found <= 12, "a field stage nobody sowed gave {found} seeds from {} crops", field.len());
    assert_eq!(client.earned.len(), earned, "breaking a field stage nobody sowed paid experience");
}

#[test]
fn a_flower_placed_and_broken_on_a_planter_teaches_nothing() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    for line in ["/clear", "/give poppy 3", "/fill ~ ~-1 ~2 ~ ~-1 ~2 herb_planter", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    let poppy = client.content.blocks.expect("base:poppy");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == poppy && s.count == 3) && c.world.block(ground) == c.content.blocks.id("base:herb_planter"));
    let earned = client.earned.len();
    // Only what the planter grows back pays: placing a flower there and breaking it, over
    // and over, pays nothing. (Three flowers, as a broken one's drop can roll out of reach.)
    for _ in 0..3 {
        client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == poppy));
        client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(poppy));
        client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    }
    client.settle();
    assert_eq!(client.earned.len(), earned, "placing and breaking a flower on a planter paid experience");
}

#[test]
fn a_planter_pays_once_for_each_flower_it_grew_back() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    for line in ["/clear", "/give poppy 2", "/fill ~ ~-1 ~2 ~ ~-1 ~2 herb_planter", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    let poppy = client.content.blocks.expect("base:poppy");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == poppy && s.count == 2) && c.world.block(ground) == c.content.blocks.id("base:herb_planter"));
    let earned = client.earned.len();
    let at = ground + IVec3::Y;
    // Placed and picked: nothing, but it will grow back in the planter.
    client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(poppy));
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
    client.settle();
    assert_eq!(client.earned.len(), earned, "picking a placed flower paid experience");
    // Grown back and picked: the planter's experience for a common flower, once.
    client.ok("/grow");
    client.pump_until(|c| c.world.block(at) == Some(poppy));
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR) && c.earned.len() > earned);
    let planter_xp = (client.content.regrowth.rules.planter_xp.common * 10.0).round() as u32;
    // Placed again and picked without growing back: nothing more.
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == poppy));
    client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(poppy));
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
    client.settle();
    assert_eq!(client.earned[earned..], [planter_xp], "the planter paid other than once for the flower it grew back");
}

#[test]
fn a_placed_wild_flower_gives_itself_back_and_no_forage() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    for line in ["/clear", "/give moonbloom 1", "/fill ~ ~-1 ~2 ~ ~-1 ~2 grass_block", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    let moonbloom = client.content.blocks.expect("base:moonbloom");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == moonbloom) && c.world.block(ground) == c.content.blocks.id("base:grass_block"));
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(moonbloom));
    let earned = client.earned.len();
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    // Its plain drop: the flower, and nothing foraged or taught.
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == moonbloom) || c.items.values().any(|i| i.stack.item == moonbloom));
    client.settle();
    assert_eq!(client.earned.len(), earned, "foraging a placed flower paid experience");
    let seeds = client.content.blocks.expect("base:moonbloom_seeds");
    assert!(!client.items.values().any(|i| i.stack.item == seeds), "a placed flower was foraged for seeds");
}

#[test]
fn crafting_all_stops_when_the_grid_makes_another_recipe() {
    let mut client = Client::join();
    for line in ["/clear", "/give oak_planks 6"] {
        client.ok(line);
    }
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.count == 6));
    // Grid cells 0 and 3 (down the left) two planks each, 1 and 4 one each: a crafting
    // table first; what is left, a plank over a plank, would make sticks.
    let to = |cell: usize| (voxmar_engine::inventory::CRAFT + cell) as u8;
    for (cell, count) in [(0, 2), (3, 2), (1, 1), (4, 1)] {
        client.propose("core:transfer", ActionTarget::Slots { from: 0, to: to(cell), count });
    }
    let earned = client.earned.len();
    client.propose("core:craft", ActionTarget::Slot(0));
    let (table, stick) = (client.content.blocks.expect("base:crafting_table"), client.content.blocks.expect("base:stick"));
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == table));
    // The first table also finishes the Crafting Table quest, whose reward (18 Engineering)
    // comes at the next quest check: wait for it, so both grants are counted exactly.
    client.pump_until(|c| c.quests.iter().any(|q| q.name == "base:crafting_table" && q.done));
    client.settle();
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == stick), "crafting all went on into another recipe");
    let reward = client.content.quests.get("base:crafting_table").unwrap().rewards.iter().find_map(|r| match r {
        voxmar_engine::quest::Reward::Xp(_, xp) => Some((xp * 10.0).round() as u32),
        _ => None,
    });
    assert_eq!(client.earned[earned..], [80, reward.unwrap()], "one table's Engineering, once, then the quest's reward");
}

/// A server started on a world another server holds open answers its first player: the
/// land round spawn, with its rivers, trails and settlements, is planned once per process
/// (`Overworld::shared`, whose sharing worldgen's `servers_on_one_world_share_its_planning`
/// proves), so both stand their players on the same spot; the join's own wait
/// (`SERVER_WAIT`) is the only clock on it.
#[test]
fn a_second_server_on_the_same_world_answers_a_join() {
    let first = Client::join();
    let second = Client::join();
    assert_eq!(first.position, second.position);
}

#[test]
fn a_creatures_loot_comes_out_once_its_body_has_fallen() {
    let mut client = Client::join();
    let rabbit = client.content.creatures.find("rabbit").expect("rabbits in the content");
    let falls = client.content.creatures.get(rabbit).def.death.drops_after();
    client.ok("/summon rabbit 1 ~ ~ ~-2");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == rabbit));
    let before = client.items.len();
    // Judged by the server's clock, with bounds that hold whenever this thread runs: no
    // reply carries the tick it ran on, but the tick read before the command is no later
    // than the kill's, and the tick read once the loot is seen is no earlier than its drop.
    let ticks = |c: &Client| c._server.as_ref().expect("the server").ticks();
    let asked = ticks(&client);
    client.ok("/creatures kill");
    // Less one: the counter is advanced after the tick's work, so loot sent in tick D is
    // seen while ticks() reads D-1.
    let fall = (falls * TICKS_PER_SECOND as f32).round() as u64 - 1;
    // While the server has not run a fall's length since we asked, nothing has dropped.
    while ticks(&client) < asked + fall {
        assert_eq!(client.items.len(), before, "loot came out of a body still falling, {} ticks after asking", ticks(&client) - asked);
        let until = Instant::now() + Duration::from_millis(10);
        client.pump_until(|_| Instant::now() > until);
    }
    // Then its meat is on the ground, and the server had run at least the fall by then.
    client.pump_until(|c| c.items.len() > before);
    assert!(ticks(&client) >= asked + fall, "loot seen {} ticks after asking, before the {fall}-tick fall", ticks(&client) - asked);
}

#[test]
fn a_kept_animal_whose_species_is_gone_waits_in_the_save() {
    // A mod turned off took its species away: the animal it gave stays in the save, and
    // comes back once the species does, rather than being lost.
    let dir = world_dir("unplaced");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let client = Client::join_with(config());
    let spawn = client.position;
    drop(client);
    // Beside it, a rabbit whose species is here, at spawn: the control that the save was
    // read and written again, not merely left alone.
    let path = dir.join("creatures.ron");
    let kept = |species: &str, seed: u32| {
        let at = format!("({}, {}, {})", spawn.x, spawn.y, spawn.z);
        format!("(species: \"{species}\", position: {at}, home: {at}, yaw: 0.0, health: 3, owner: \"test\", seed: {seed}, scale: 1.0, grows_in: None, products_in: [])")
    };
    let written = format!("[{}, {}]", kept("lostmod:ghost_cat", 7), kept("base:rabbit", 8));
    std::fs::write(&path, &written).unwrap();
    let client = Client::join_with(config());
    drop(client);
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_ne!(saved, written, "the save was not written again");
    assert_eq!(saved.matches("lostmod:ghost_cat").count(), 1, "the waiting animal is in the save once: {saved}");
    assert_eq!(saved.matches("base:rabbit").count(), 1, "the rabbit is in the save once: {saved}");
    // The rabbit came back into the world, so it is saved with the genes it was given there.
    assert_eq!(saved.replace(' ', "").matches("genome:Some").count(), 1, "only the rabbit came back with genes: {saved}");
    std::fs::remove_dir_all(&dir).unwrap();
}
