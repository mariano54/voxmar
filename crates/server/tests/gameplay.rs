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
use voxmar_server::{Limits, ServerConfig, ServerHandle, load_content, spawn};

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
    /// The last teleport applied, which inputs echo.
    teleports: u32,
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
    /// Creatures in sight, how many were seen to die, and our blows the server answered.
    creatures: std::collections::HashMap<u32, voxmar_protocol::CreatureView>,
    deaths: u32,
    /// Times we were told we died.
    died: u32,
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
        Self::greet_with(connection, name, server, None)
    }

    /// Joins as `name`, giving the server `admin_token`.
    fn greet_with(connection: ClientConnection, name: &str, server: Option<ServerHandle>, admin_token: Option<&str>) -> Self {
        Self::greet_content(connection, name, server, admin_token, content())
    }

    /// Joins as `name` with `content`, which must be the server's.
    fn greet_content(connection: ClientConnection, name: &str, server: Option<ServerHandle>, admin_token: Option<&str>, content: Content) -> Self {
        connection.send(ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: admin_token.map(str::to_owned) });
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
            teleports: 0,
            health: 0,
            max_health: 0,
            vitals: Default::default(),
            content,
            inventory: Default::default(),
            weather: None,
            quick: false,
            chat: Vec::new(),
            day: None,
            creatures: Default::default(),
            deaths: 0,
            died: 0,
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
            ServerMessage::Teleport { position, seq } => {
                self.position = position;
                self.teleports = seq;
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
            ServerMessage::Time { day, day_speed, .. } => self.day = Some((day, day_speed)),
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
            ServerMessage::Died => self.died += 1,
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

    /// Waits until the server has run `ticks` whole ticks from now, by its own clock, and
    /// applies everything they sent: a barrier after which whatever the server was going to
    /// do about the messages sent before it is done, for checking that something did not
    /// happen without waiting a fixed time. Sound only for in-process clients, whose
    /// messages reach the server at once; over TCP a message may still be in flight.
    fn after_ticks(&mut self, ticks: u64) {
        let ran = |c: &Self| c._server.as_ref().expect("the first player holds the server").ticks();
        let until = ran(self) + ticks + 1;
        self.pump_until(|c| ran(c) >= until);
        self.drain();
    }

    /// Applies every message waiting.
    fn drain(&mut self) {
        while let Ok(Some(message)) = self.connection.try_recv() {
            self.apply(message);
        }
    }

    /// The server's ticks so far, by its own clock.
    fn ticks(&self) -> u64 {
        self._server.as_ref().expect("the first player holds the server").ticks()
    }

    fn propose(&self, action: &str, target: ActionTarget) {
        self.propose_at(action, target, Instant::now());
    }

    /// Proposes as if the player clicked at `at`: in process, the clock the server times
    /// actions and blows by (see `Connection::send_at`), so a test spaces clicks exactly
    /// without waiting.
    fn propose_at(&self, action: &str, target: ActionTarget, at: Instant) {
        let action: ActionId = self.content.actions.expect(action);
        self.connection.send_at(ClientMessage::Propose { action, target }, at);
    }

    fn send_position(&self, position: Vec3, flying: bool) {
        self.send_position_at(position, flying, Instant::now());
    }

    /// Sends a move as made at `at` (see `propose_at`).
    fn send_position_at(&self, position: Vec3, flying: bool, at: Instant) {
        self.connection.send_at(ClientMessage::Input(PlayerInput { teleport: self.teleports, position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying }), at);
    }

    /// Runs a command and returns the console lines it answers with.
    fn command(&mut self, line: &str) -> Vec<(ChatKind, String)> {
        self.chat.clear();
        self.connection.send(ClientMessage::Command { line: line.into() });
        self.pump_until(|c| !c.chat.is_empty());
        std::mem::take(&mut self.chat)
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
    client.clear();
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
    client.after_ticks((hardness.as_secs_f32() * TICKS_PER_SECOND as f32).ceil() as u64 + 2);
    assert_eq!(client.world.block(below), Some(ground));

    let started = std::time::Instant::now();
    client.propose("core:mine", target);
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    assert!(started.elapsed() + CATCH_UP >= hardness, "mining completed too early: {:?} of {hardness:?}", started.elapsed());
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.count == 1));

    // Place the first hotbar item back on the block underneath the hole.
    let item = client.inventory.get(0).unwrap();
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports,
        position: client.position,
        yaw: 0.0,
        pitch: 0.0,
        selected_slot: 0,
        flying: false,
    }));
    client.propose("core:place", ActionTarget::Block { block: below - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(below) == Some(item.item));
    client.pump_until(|c| c.inventory.get(0).map_or(0, |s| s.count) == item.count - 1);
}

#[test]
fn broken_blocks_pop_out_items_that_are_picked_up_and_thrown_stacks_fly() {
    let mut client = Client::join();
    client.clear();
    client.ok("/give oak_planks 64");
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
    client.clear();
    for line in ["/fill ~ ~-1 ~2 ~1 ~-1 ~2 dirt", "/fill ~ ~ ~2 ~1 ~1 ~2 air", "/give wheat_seeds 5", "/give saffron_seeds 5", "/give wooden_hoe 1"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(ground).is_some_and(|b| b != BlockId::AIR) && c.inventory.get(2).is_some_and(|s| s.item == c.content.blocks.expect("base:wooden_hoe")));
    let blocks = &client.content.blocks;
    let (sprout, ripe, wheat) = (blocks.expect("base:wheat_crop_0"), blocks.expect("base:wheat_crop"), blocks.expect("base:wheat"));
    let farmland = blocks.expect("base:farmland");
    assert_eq!(client.inventory.get(0).map(|s| s.item), blocks.id("base:wheat_seeds"));
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };

    // Seeds go on farmland only: sowing on dirt is refused, and a hoe tills it.
    hold(&client, 0);
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    hold(&client, 2);
    for block in [ground, other] {
        client.propose("base:till", ActionTarget::Block { block, face: Face::PosY });
    }
    client.pump_until(|c| c.world.block(ground) == Some(farmland) && c.world.block(other) == Some(farmland));
    assert_eq!(client.world.block(ground + IVec3::Y), Some(BlockId::AIR), "sown on untilled dirt");

    // Saffron takes farming 60: refused, nothing sown, no experience.
    hold(&client, 1);
    client.propose("base:plant", ActionTarget::Block { block: other, face: Face::PosY });
    // Wheat takes level 1: sown, its planting experience waiting for the harvest.
    hold(&client, 0);
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout));
    assert!(client.earned.is_empty(), "sowing paid at once: {:?}", client.earned);
    assert_eq!(client.world.block(other + IVec3::Y), Some(BlockId::AIR), "sown without the level");
    client.pump_until(|c| c.inventory.get(0).map(|s| s.count) == Some(4));

    // Grown ripe, then broken: its harvest pops out and its experience is earned, the
    // sowing's (2) with the harvest's (9), in tenths.
    assert_eq!(client.ok("/grow ripe"), "Grew 1 plant.");
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(ripe));
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.earned == [110] && c.items.values().any(|i| i.stack.item == wheat));
    client.pump_until(|c| c.skills.xp(voxmar_engine::skill::SkillId(0)) == 110);

    // Ripe fruit on a tree is picked by hand, at any level: the fruit pops out and the
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
    // Clicked 50 ms apart, then the next once 85% of the second's time has passed.
    let mine = |client: &Client, block: IVec3, at: Instant| client.propose_at("core:mine", ActionTarget::Block { block, face: Face::PosY }, at);
    let start = Instant::now();
    mine(&client, c, start);
    mine(&client, a, start + Duration::from_millis(50));
    mine(&client, b, start + Duration::from_millis(50) + Duration::from_secs_f32(hardness * 0.85));
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
    // Rise as high as the open air (or grass) above spawn allows (at most 11 blocks), by a
    // teleport (on foot nobody may rise more than a jump), then drop back down one plausible
    // step per input.
    let open = |c: &Client, p: IVec3| c.world.block(p).is_some_and(|b| !c.content.blocks.info(b).solid);
    let height = (1..=12).take_while(|&dy| open(&client, feet + IVec3::Y * dy)).count() as i32 - 1;
    assert!(height >= 6, "need open sky above spawn, have {height} blocks");
    let top = start + Vec3::Y * height as f32;
    client.ok(&format!("/tp {} {} {}", top.x, top.y, top.z));
    client.pump_until(|c| c.position == top);
    // A tick or two up there, so the fall is counted from the top.
    client.after_ticks(2);
    for y in (0..height).rev() {
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports,
            position: start + Vec3::Y * y as f32,
            yaw: 0.0,
            pitch: 0.0,
            selected_slot: 0,
            flying: false,
        }));
        // A server tick on each step, so the fall is seen block by block however loaded the
        // machine.
        client.after_ticks(1);
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
        // The first move since joining (a full allowance), rise faster than normal flight
        // allows but within four times its speed, then make an impossible move so the server
        // answers with where it thinks we are.
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
    client.clear();
    // Two stacks besides the one mined, so a save that kept only one slot would show.
    client.ok("/give torch 5");
    client.ok("/give stick 7");
    client.pump_until(|c| c.inventory.slots.iter().flatten().count() == 2);
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.count == 1));
    // Step a little within the same block, so the position is our own rather than spawn.
    let moved = client.position + Vec3::new(0.2, 0.0, -0.2);
    client.send_position(moved, false);
    client.after_ticks(2);
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

/// A creature's loot waits for its body to fall; a server stopped in that moment saves it
/// where it will drop. The server runs behind, so it stops well before the loot is due.
#[test]
fn loot_still_falling_when_the_server_stops_is_there_after_a_restart() {
    let dir = world_dir("falling-loot");
    let lag = voxmar_server::Lag { per_tick: Duration::from_millis(100), ..Default::default() };
    let config = |lag| ServerConfig { view_distance: 2, world: Some(dir.clone()), lag, ..Default::default() };
    let mut client = Client::join_with(config(lag));
    let slime = client.content.blocks.id("base:lumen_slime").unwrap();
    let snail = client.content.creatures.find("lumen_snail").unwrap();
    // Out of reach, so the loot stays on the ground to be counted.
    client.ok("/summon lumen_snail 1 ~5 ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == snail));
    client.ok("/creatures kill");
    assert!(!client.items.values().any(|i| i.stack.item == slime), "the loot dropped before the server stopped");
    drop(client);

    let mut client = Client::join_with(config(Default::default()));
    client.pump_until(|c| c.items.values().any(|i| i.stack.item == slime));
    // Once: the save wrote it lying, not also still to fall.
    client.after_ticks(40);
    assert_eq!(client.items.values().filter(|i| i.stack.item == slime).count(), 1, "{:?}", client.items);
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A world opened with other worldgen data than its land was last made from tells the
/// admin who joins that new land may not line up, once for each change, and still opens.
#[test]
fn a_world_whose_worldgen_data_changed_says_so_once_and_still_opens() {
    let dir = world_dir("worldgen-content");
    let tuned = {
        let mut content = content();
        let mut files = (*content.files).clone();
        files.iter_mut().find(|f| f.package == "base" && f.name == "ores.ron").unwrap().source.push_str("\n// a vein tuned\n");
        content.files = std::sync::Arc::new(files);
        content
    };
    let warned = |content: Content| {
        let server = spawn(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() }, content.clone()).unwrap();
        let (connection, server_side) = local_pair();
        server.connections().send(server_side).unwrap();
        let client = Client::greet_content(connection, "test", Some(server), None, content);
        let warning = client.content.english.render(&voxmar_engine::lang::Words::key("world.generation_changed"), &client.content.blocks);
        client.chat.iter().any(|(_, line)| *line == warning)
    };
    assert!(!warned(content()), "a new world");
    assert!(!warned(content()), "the same data");
    assert!(warned(tuned.clone()), "ores.ron changed");
    assert!(!warned(tuned), "the change was told");
    assert!(warned(content()), "changed back");
    // Land picks blocks by their tags, so a block given another tag warns too.
    let assets = world_dir("tagged-content");
    copy_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), &assets.join("content"));
    let blocks = assets.join("content/base/blocks.ron");
    let source = std::fs::read_to_string(&blocks).unwrap();
    let mossy = r#"tags: ["stone", "bricks", "building", "mossy", "wet"]"#;
    assert!(source.contains(mossy));
    std::fs::write(&blocks, source.replace(mossy, r#"tags: ["stone", "bricks", "building", "mossy", "wet", "tagged"]"#)).unwrap();
    let tagged = load_content(&assets).expect("content loads");
    assert!(warned(tagged), "a block's tags changed");
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&assets).unwrap();
}

/// Copies a content directory's definitions and languages (not its sounds) to `to`.
fn copy_content(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (path, name) = (entry.path(), entry.file_name());
        if path.is_dir() {
            if name != "sounds" {
                copy_content(&path, &to.join(&name));
            }
        } else {
            std::fs::copy(&path, to.join(&name)).unwrap();
        }
    }
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
    // A step a tick at most is seen (wear looks once a tick), so each waits for the tick
    // after it; stamped 110 ms apart, the walking pace the moves are judged by.
    let start = Instant::now();
    for i in 0..80 {
        client.send_position_at(if i % 2 == 0 { there } else { here }, false, start + Duration::from_millis(110) * i);
        client.after_ticks(0);
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
    // Struck a full charge (650 ms) apart, beside it wherever it goes, until `landed` blows
    // land. The clicks are stamped apart rather than waited for.
    let mut click = Instant::now();
    let mut strike = |client: &mut Client, landed: usize| {
        let start = client.damage.len();
        while client.damage[start..].iter().filter(|&&d| d > 0).count() < landed && client.creatures.contains_key(&id) {
            assert!(client.damage.len() - start < 80, "{} blows landed {landed}", client.damage.len() - start);
            let at = client.creatures[&id].position;
            client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
            client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot as u8, flying: false }));
            click += Duration::from_millis(650);
            let blows = client.blows;
            client.propose_at("core:attack", ActionTarget::Creature(id), click);
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
    client.clear();
    client.ok("/give wheat 64");
    client.pump_until(|c| c.creatures.values().filter(|v| v.species == hen).count() == 2 && c.inventory.get(0).is_some_and(|s| s.item == wheat));
    // Hold the wheat.
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    client.clear();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.ok("/setblock ~ ~-1 ~ stone");
    client.pump_until(|c| c.world.block(below) == c.content.blocks.id("base:stone"));
    let hardness = client.content.blocks.info(client.world.block(below).unwrap()).hardness;
    let (started, from) = (Instant::now(), client.ticks());
    client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
    let ran = std::cell::Cell::new(0);
    client.pump_until(|c| {
        ran.set(c.ticks() - from);
        c.world.block(below) == Some(BlockId::AIR)
    });
    let ran = ran.get();
    let took = started.elapsed().as_secs_f32();
    assert!(took >= hardness - 0.05, "stone ({hardness} s) broke after {took:.2} s");
    // Each tick takes at least 60 ms, so the clock finishes it in fewer ticks than counting
    // them would.
    let by_ticks = (hardness * TICKS_PER_SECOND as f32) as u64;
    assert!(ran < by_ticks, "stone broke after {ran} ticks, as many as {by_ticks} would take counted");
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
    // Each click half the break's time after the last, by the clock the server judges by.
    let (mut at, half) = (Instant::now(), Duration::from_secs_f32(hardness * 0.5));
    for (block, instant) in [(a, "core:drop"), (b, "core:place")] {
        let target = ActionTarget::Block { block, face: Face::PosY };
        client.propose_at("core:mine", target, at);
        at += half;
        match instant {
            "core:drop" => client.propose_at(instant, ActionTarget::Stack { slot: 0, count: 1 }, at),
            _ => client.propose_at(instant, ActionTarget::Block { block: feet + IVec3::new(2, -1, 0), face: Face::PosY }, at),
        }
        at += half;
        client.connection.send_at(ClientMessage::Finished { target }, at);
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
    let refused = Instant::now() + Duration::from_millis(300);
    client.pump_until(|_| Instant::now() > refused);
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
    client.clear();
    client.ok("/give stone_pickaxe 1");
    client.pump_until(|c| c.world.block(ore) == c.content.blocks.id("base:iron_ore") && c.inventory.get(0).is_some_and(|s| s.item == c.content.blocks.expect("base:stone_pickaxe")));
    let iron = client.world.block(ore).unwrap();
    // Long enough for the ore to break by hand, had it been allowed.
    let by_hand = (client.content.blocks.info(iron).hardness * TICKS_PER_SECOND as f32) as u64 + 2;
    let mine = |client: &Client| client.propose("core:mine", ActionTarget::Block { block: ore, face: Face::PosY });
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };
    // At Mining 1, even with the pickaxe: refused.
    hold(&client, 0);
    mine(&client);
    client.after_ticks(by_hand);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke at Mining 1");
    // At Mining 15 with a bare hand: refused too.
    client.ok("/skill mining 15");
    hold(&client, 1);
    mine(&client);
    client.after_ticks(by_hand);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke without a pickaxe");
    // At Mining 15 with the stone pickaxe: broken, quicker than by hand, teaching Mining (35).
    hold(&client, 0);
    let from = client.ticks();
    mine(&client);
    client.pump_until(|c| c.world.block(ore) == Some(BlockId::AIR) && c.earned.contains(&350));
    let hardness = client.content.blocks.info(iron).hardness;
    let ran = client.ticks() - from;
    assert!((ran as f32) < hardness * 0.5 * TICKS_PER_SECOND as f32, "a stone pickaxe should mine iron ore at four times a hand's speed, not in {ran} ticks");
}

#[test]
fn a_crafting_table_makes_what_the_two_by_two_grid_cannot() {
    let mut client = Client::join();
    let table = client.position.floor().as_ivec3() + IVec3::new(1, 0, 1);
    for line in ["/clear", "/give cobblestone 3", "/give stick 2", "/fill ~1 ~ ~1 ~1 ~ ~1 crafting_table"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(table) == c.content.blocks.id("base:crafting_table") && c.inventory.get(1).is_some());
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
    client.after_ticks(16);
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
    client.clear();
    for line in ["/fill ~-1 ~-2 ~1 ~1 ~-1 ~3 water", "/fill ~-1 ~ ~1 ~1 ~3 ~3 air", "/give fishing_rod 1", "/skill fishing 80"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(water).is_some_and(|b| c.content.blocks.info(b).fluid.is_some()) && c.inventory.get(0).is_some_and(|s| s.item == c.content.blocks.expect("base:fishing_rod")));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    thief.pump_until(|c| c.items.values().map(|i| i.stack.count as u32).sum::<u32>() == 44);
    owner.after_ticks(3 * TICKS_PER_SECOND as u64);
    thief.drain();
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
    client.after_ticks(30);
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

/// A content directory with one more package, `zz_test`, holding `files`: what a mod adds.
/// Returns where it is (to remove) and the content loaded from it.
fn with_mod(name: &str, files: &[(&str, &str)]) -> (std::path::PathBuf, Content) {
    let assets = world_dir(name);
    copy_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), &assets.join("content"));
    let package = assets.join("content/zz_test");
    std::fs::create_dir_all(&package).unwrap();
    for (file, source) in files {
        std::fs::write(package.join(file), source).unwrap();
    }
    let content = load_content(&assets).expect("content loads");
    (assets, content)
}

/// Joins a new in-process server of `content`.
fn join_content(content: Content) -> Client {
    let server = spawn(ServerConfig { view_distance: 2, ..Default::default() }, content.clone()).unwrap();
    let (connection, server_side) = local_pair();
    server.connections().send(server_side).unwrap();
    Client::greet_content(connection, "test", Some(server), None, content)
}

const TEST_QUESTS: &str = r#"(quests: [
    (name: "sticks", age: "nomad", branch: "main", title: "Three Sticks", why: "", tasks: [[Have("base:stick", 3)]], rewards: [Item("base:torch", 2)]),
    (name: "miner", age: "nomad", branch: "main", title: "Miner", why: "", tasks: [[Level("mining", 10)]], rewards: [Xp("crafting", 50.0)]),
    (name: "laden", age: "nomad", branch: "main", title: "Laden", why: "", tasks: [[Have("base:cobblestone", 2304)]], rewards: [Item("base:apple", 3)]),
])"#;

/// Quests are checked once a second by the server's clock: one a goal short is still open
/// after a check, and one whose goal holds is done at the next, rewarded once, and stays
/// done when what it asked for is gone.
#[test]
fn a_quest_is_done_at_the_next_check_once_its_goal_holds_and_rewards_once() {
    let (assets, content) = with_mod("quests-have", &[("quests.ron", TEST_QUESTS)]);
    let mut client = join_content(content);
    let done = |c: &Client, name: &str| c.quests.iter().any(|q| q.name == name && q.done);
    let torch = client.content.blocks.expect("base:torch");
    let carried = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| !c.quests.is_empty());
    client.ok("/clear");
    client.ok("/give stick 2");
    // A whole second of the server's ticks: a check has run, and two sticks are not three.
    client.after_ticks(TICKS_PER_SECOND as u64);
    assert!(!done(&client, "zz_test:sticks"), "two sticks finished it");
    assert_eq!(carried(&client, torch), 0);
    client.chat.clear();
    client.ok("/give stick 1");
    client.pump_until(|c| done(c, "zz_test:sticks") && carried(c, torch) == 2);
    assert!(client.chat.iter().any(|(_, line)| line.contains("Three Sticks")), "{:?}", client.chat);
    // Without the sticks, two checks later: still done, and the torches came once.
    client.ok("/clear stick");
    client.after_ticks(2 * TICKS_PER_SECOND as u64);
    assert!(done(&client, "zz_test:sticks"));
    assert_eq!(carried(&client, torch), 2);
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// A level goal holds from that level, not a level before, and its experience comes once.
#[test]
fn a_level_quest_waits_for_the_level_and_grants_its_experience() {
    let (assets, content) = with_mod("quests-level", &[("quests.ron", TEST_QUESTS)]);
    let mut client = join_content(content);
    let done = |c: &Client, name: &str| c.quests.iter().any(|q| q.name == name && q.done);
    client.pump_until(|c| !c.quests.is_empty());
    client.ok("/skill mining 9");
    client.after_ticks(TICKS_PER_SECOND as u64);
    assert!(!done(&client, "zz_test:miner"), "level 9 finished a level 10 quest");
    client.earned.clear();
    client.ok("/skill mining 10");
    client.pump_until(|c| done(c, "zz_test:miner") && c.earned.contains(&500));
    client.after_ticks(2 * TICKS_PER_SECOND as u64);
    assert_eq!(client.earned.iter().filter(|&&tenths| tenths == 500).count(), 1, "{:?}", client.earned);
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// A reward that does not fit lies at the player's feet rather than being lost.
#[test]
fn a_quest_reward_that_does_not_fit_drops_at_the_players_feet() {
    let (assets, content) = with_mod("quests-full", &[("quests.ron", TEST_QUESTS)]);
    let mut client = join_content(content);
    let done = |c: &Client, name: &str| c.quests.iter().any(|q| q.name == name && q.done);
    let apple = client.content.blocks.expect("base:apple");
    client.pump_until(|c| !c.quests.is_empty());
    client.ok("/clear");
    // Every slot full of cobblestone.
    client.ok("/give cobblestone 2304");
    client.pump_until(|c| done(c, "zz_test:laden"));
    client.pump_until(|c| c.items.values().any(|i| i.stack.item == apple && i.stack.count == 3));
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == apple));
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// Every age a first-party quest belongs to names its chapter in English and Japanese
/// (`quest.age.<age>`), which the quest book heads its chapters with.
#[test]
fn every_quest_age_is_named_in_english_and_japanese() {
    let content = content();
    let japanese = voxmar_engine::lang::Lang::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), "ja");
    let ages: std::collections::BTreeSet<&str> = content.quests.iter().map(|quest| quest.age.as_str()).collect();
    assert!(!ages.is_empty());
    for age in ages {
        let key = format!("quest.age.{age}");
        assert!(content.english.find(&key).is_some(), "no English for {key}");
        assert!(japanese.find(&key).is_some(), "no Japanese for {key}");
    }
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
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    // One break in eight gives flint: in sixty, all but certainly some.
    let mut flints = 0;
    for _ in 0..60 {
        client.ok("/setblock ~ ~-1 ~ gravel");
        client.pump_until(|c| c.world.block(below) == Some(gravel));
        client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
        client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
        // The drop comes just after the block goes; setting the gravel again at once could
        // bury it unseen.
        client.after_ticks(2);
        flints = client.items.values().filter(|i| i.stack.item == flint).count() + client.inventory.slots.iter().flatten().filter(|s| s.item == flint).count();
        if flints > 0 {
            break;
        }
    }
    assert!(flints > 0, "sixty gravel broke without a flint");
}

/// Whatever a player carries, counted.
fn carried(client: &Client) -> u32 {
    client.inventory.slots.iter().flatten().map(|s| s.count as u32).sum()
}

/// Gives the player 47 items over six stacks, three of them worth keeping.
fn give_a_load(client: &mut Client) {
    for line in ["/clear", "/give iron_pickaxe 1", "/give iron_ingot 5", "/give dirt 30", "/give stick 10", "/give bread 1"] {
        client.ok(line);
    }
    client.pump_until(|c| carried(c) == 47);
}

/// A server held up after every tick, so that messages sent together are handled together
/// before the next tick: the races a quick client wins now and then, every time.
fn stalled() -> voxmar_server::Lag {
    voxmar_server::Lag { stall: Duration::from_millis(300), stall_every: 1, ..Default::default() }
}

#[test]
fn a_respawn_sent_the_moment_a_player_dies_still_leaves_their_things_behind() {
    let mut client = Client::join_with(ServerConfig { view_distance: 2, lag: stalled(), ..Default::default() });
    give_a_load(&mut client);
    // Dying and asking to respawn in one breath, before a tick could bury.
    client.connection.send(ClientMessage::Command { line: "/kill".into() });
    client.propose("core:respawn", ActionTarget::None);
    client.pump_until(|c| c.died > 0);
    client.after_ticks(2);
    let lying = client.items.values().map(|i| i.stack.count as u32).sum::<u32>();
    assert_eq!((carried(&client), lying), (3, 44), "the respawned player kept {:?}", client.inventory);
}

/// Waits until `name` has left `host`'s server.
fn gone(host: &mut Client, name: &str) {
    let online = |c: &Client| c.chat.iter().find(|(kind, text)| *kind == ChatKind::Info && text.contains("online")).map(|(_, text)| text.clone());
    for _ in 0..1000 {
        host.chat.clear();
        host.connection.send(ClientMessage::Command { line: "/list".into() });
        host.pump_until(|c| online(c).is_some());
        if !online(host).unwrap().contains(name) {
            return;
        }
    }
    panic!("{name} never left");
}

#[test]
fn leaving_dead_leaves_their_things_behind_too() {
    let dir = world_dir("leave-dead");
    let mut host = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), lag: stalled(), ..Default::default() });
    let mut guest = host.another("guest");
    give_a_load(&mut guest);
    // Dying and leaving at once, before a tick could bury.
    guest.connection.send(ClientMessage::Command { line: "/kill".into() });
    drop(guest);
    gone(&mut host, "guest");
    let content = content();
    let saved = voxmar_server::save::WorldSave::new(&dir).read_player("guest", &content.blocks).unwrap().expect("the guest was saved");
    let kept: u32 = saved.inventory.slots.iter().flatten().map(|s| s.count as u32).sum();
    assert_eq!(kept, 3, "the guest left dead with {:?}", saved.inventory);
    // What they left was written before they were.
    let items = std::fs::read_to_string(dir.join("items.ron")).unwrap();
    assert!(items.contains("dirt"), "what the guest carried lies where they fell: {items}");
    drop(host);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_player_who_left_dead_wakes_at_home() {
    let dir = world_dir("dead-home");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    drop(Client::join_with(config()));
    let file = dir.join("players/test.ron");
    let text = std::fs::read_to_string(&file).unwrap();
    let dead = text.replace("current: 20", "current: 0").replace("home: None", "home: Some((3.5, 90.0, -7.5))");
    assert!(dead.contains("current: 0") && dead.contains("home: Some"), "{text}");
    std::fs::write(&file, dead).unwrap();
    let client = Client::join_with(config());
    assert_eq!(client.position, Vec3::new(3.5, 90.0, -7.5), "woke at the spawn point rather than at home");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The files in `dir` set aside as unreadable.
fn set_aside(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".corrupt-")).collect()
}

#[test]
fn an_unreadable_player_file_is_set_aside_not_overwritten() {
    let dir = world_dir("corrupt-player");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    drop(Client::join_with(config()));
    let file = dir.join("players/test.ron");
    assert!(file.is_file());
    std::fs::write(&file, "(position: [1.0, 2.0").unwrap();
    let mut client = Client::join_with(config());
    client.pump_until(|c| c.chat.iter().any(|(kind, _)| *kind == ChatKind::Error));
    assert!(client.chat.iter().any(|(_, text)| text.contains("test.ron.corrupt-")), "{:?}", client.chat);
    // Kept as it was before anything could be saved over it...
    let aside = set_aside(&dir.join("players"));
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read_to_string(dir.join("players").join(&aside[0])).unwrap(), "(position: [1.0, 2.0", "the bad file is kept as it was");
    // ...and the player saved afresh beside it.
    drop(client);
    assert!(std::fs::read_to_string(&file).unwrap().contains("position"), "the player is saved afresh beside it");
    assert_eq!(set_aside(&dir.join("players")).len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unreadable_world_files_are_set_aside_and_the_world_still_opens() {
    let dir = world_dir("corrupt-world");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    drop(client);
    let column = ColumnPos::of_block(below);
    let column_file = dir.join(format!("columns/{}.{}.bin", column.0.x, column.0.y));
    assert!(column_file.is_file());
    std::fs::write(&column_file, b"vxc4 not zlib").unwrap();
    for file in ["items.ron", "crops.ron", "creatures.ron"] {
        std::fs::write(dir.join(file), "[(oops").unwrap();
    }
    let client = Client::join_with(config());
    drop(client);
    assert_eq!(set_aside(&dir.join("columns")).len(), 1, "the bad column is set aside");
    let aside = set_aside(&dir);
    for file in ["items.ron", "crops.ron", "creatures.ron"] {
        assert!(aside.iter().any(|n| n.starts_with(&format!("{file}.corrupt-"))), "{file} was not set aside: {aside:?}");
        assert_ne!(std::fs::read_to_string(dir.join(file)).unwrap(), "[(oops", "{file} is written afresh");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A survival player standing on a stone floor high in the open air, 40 blocks long along
/// +x, with nothing around it: ground to test movement on.
fn on_a_test_floor() -> Client {
    let mut client = Client::join_with(ServerConfig { view_distance: 2, allow_flight: false, ..Default::default() });
    let at = client.position.floor().as_ivec3().with_y(300);
    client.ok(&format!("/fill {} {} {} {} {} {} air", at.x - 2, at.y - 6, at.z - 3, at.x + 42, at.y + 6, at.z + 3));
    client.ok(&format!("/fill {} {} {} {} {} {} stone", at.x - 2, at.y - 1, at.z - 3, at.x + 42, at.y - 1, at.z + 3));
    let feet = at.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
    client.tp(feet);
    client.pump_until(|c| c.world.block(at - IVec3::Y).is_some_and(|b| b != BlockId::AIR));
    client
}

impl Client {
    /// Teleports the player, and forgets the correction that moved them.
    fn tp(&mut self, to: Vec3) {
        self.ok(&format!("/tp {} {} {}", to.x, to.y, to.z));
        self.pump_until(|c| c.position == to);
        self.teleported = None;
    }

    /// Fills a box of blocks, corners given from `at`.
    fn fill(&mut self, at: IVec3, from: IVec3, to: IVec3, block: &str) {
        let (a, b) = (at + from, at + to);
        self.ok(&format!("/fill {} {} {} {} {} {} {block}", a.x, a.y, a.z, b.x, b.y, b.z));
    }

    /// Empties the inventory, and waits until it shows empty.
    fn clear(&mut self) {
        self.ok("/clear");
        self.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
    }

    /// Whether the server has corrected none of the inputs sent so far.
    fn all_accepted(&mut self) -> bool {
        self.after_ticks(2);
        self.teleported.is_none()
    }
}

#[test]
fn a_burst_of_inputs_gets_no_more_ground_than_the_clock_allows() {
    let mut client = on_a_test_floor();
    let start = client.position;
    // Thirty one-block steps at once: each was allowed on its own (a twentieth of a
    // second at speed), so a client could cover any distance in one go.
    for i in 1..=30 {
        client.send_position(start + Vec3::X * i as f32, false);
    }
    client.pump_until(|c| c.teleported.is_some());
    let reached = client.teleported.unwrap().x - start.x;
    // Two seconds at a quarter over top speed, and half a block.
    let allowed = 1.25 * client.content.movement.top_ground_speed() * 2.0 + 0.5;
    assert!(reached <= allowed && reached >= allowed - 2.0, "one burst carried the player {reached} blocks (two seconds' worth is {allowed})");
}

#[test]
fn on_foot_nobody_rises_higher_than_a_jump() {
    let mut client = on_a_test_floor();
    let start = client.position;
    // Straight up, five blocks: no jump goes that high.
    client.send_position(start + Vec3::Y * 5.0, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start));
    // A jump's height is fine, but a second one from the air is not.
    client.send_position(start + Vec3::Y * 1.2, false);
    client.send_position(start + Vec3::Y * 2.4, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start + Vec3::Y * 1.2));
    // Nor is climbing onto a wall two blocks high from the ground.
    client.send_position(start, false);
    client.send_position(start + Vec3::Y * 2.0, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start));
}

#[test]
fn nobody_walks_through_walls() {
    let mut client = on_a_test_floor();
    let start = client.position;
    // Three and a half blocks in one step (a hitch's worth) over open floor, and back.
    client.send_position(start + Vec3::X * 3.5, false);
    client.send_position(start, false);
    assert!(client.all_accepted(), "a long step over open floor was refused");
    // The same step with a wall two blocks high in the way.
    let at = start.floor().as_ivec3();
    client.fill(at, IVec3::new(2, 0, 0), IVec3::new(2, 1, 0), "stone");
    client.send_position(start + Vec3::X * 3.5, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported, Some(start), "walked through a wall");
}

#[test]
fn long_steps_off_ledges_down_terraces_and_up_steps_are_not_snapped() {
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // Past x+3 the floor steps down a block at a time: x+3 one below, x+4 two, then three.
    client.fill(at, IVec3::new(3, -4, -3), IVec3::new(42, -1, 3), "air");
    client.fill(at, IVec3::new(3, -2, -3), IVec3::new(3, -2, 3), "stone");
    client.fill(at, IVec3::new(4, -3, -3), IVec3::new(4, -3, 3), "stone");
    client.fill(at, IVec3::new(5, -4, -3), IVec3::new(42, -4, 3), "stone");
    let at_ledge = start + Vec3::X * 2.0;
    client.send_position(at_ledge, false);
    // Off the ledge in one long step, as a client that hitched sends it: the straight line
    // from the top to a block down cuts the ledge's corner, which the player went round.
    client.send_position(at_ledge + Vec3::new(1.0, -1.0, 0.0), false);
    // Down the other two terraces in one step.
    client.send_position(at_ledge + Vec3::new(4.0, -3.0, 0.0), false);
    // And back up one: up first, then across.
    client.send_position(at_ledge + Vec3::new(2.0, -2.0, 0.0), false);
    assert!(client.all_accepted(), "snapped back to {:?}", client.teleported);
}

#[test]
fn swimmers_climb_out_onto_a_bank_but_nobody_jumps_out_of_a_dry_pit_or_lava() {
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // Three pits two blocks deep in the floor, walled in stone: water at x+3..x+5, dry at
    // x+9..x+11 and lava at x+15..x+17.
    for (x, inside) in [(3, "water"), (9, "air"), (15, "lava")] {
        client.fill(at, IVec3::new(x - 1, -3, -2), IVec3::new(x + 3, -1, 2), "stone");
        client.fill(at, IVec3::new(x, -2, -1), IVec3::new(x + 2, -1, 1), inside);
    }
    // From the bottom of the water onto the bank: a jump and the lift out of water.
    client.tp(start + Vec3::new(5.0, -2.0, 0.0));
    client.send_position(start + Vec3::new(6.0, 0.0, 0.0), false);
    assert!(client.all_accepted(), "a swimmer was refused the bank: {:?}", client.teleported);
    // From the bottom of the dry pit, two blocks is more than a jump.
    let pit = start + Vec3::new(11.0, -2.0, 0.0);
    client.tp(pit);
    client.send_position(start + Vec3::new(12.0, 0.0, 0.0), false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported, Some(pit));
    // Nor out of lava, which nobody swims in.
    let pit = start + Vec3::new(17.0, -2.0, 0.0);
    client.tp(pit);
    client.send_position(start + Vec3::new(18.0, 0.0, 0.0), false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported, Some(pit));
}

#[test]
fn nobody_hovers_over_a_ravine() {
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // The floor ends at x+3; past it there is nothing for a long way down.
    client.fill(at, IVec3::new(3, -1, -3), IVec3::new(42, -1, 3), "air");
    let over = start + Vec3::X * 3.0;
    // Keeping on sending that height, a tick at a time, as a client that ignores gravity.
    let ticks = |c: &Client| c._server.as_ref().unwrap().ticks();
    let until = ticks(&client) + 6 * TICKS_PER_SECOND as u64;
    while !client.teleported.is_some_and(|to| to.y < over.y) {
        assert!(ticks(&client) < until, "hovered six seconds over a ravine");
        client.send_position(over, false);
        let next = ticks(&client) + 1;
        client.pump_until(|c| ticks(c) >= next);
    }
}

#[test]
fn a_move_sent_before_a_teleport_does_not_undo_it() {
    let mut client = on_a_test_floor();
    let start = client.position;
    let to = start + Vec3::X * 5.0;
    // The command and a step from where they stood, sent together: the step was made
    // before the client knew of the teleport, and is well within a burst back from it.
    client.connection.send(ClientMessage::Command { line: format!("/tp {} {} {}", to.x, to.y, to.z) });
    client.send_position(start + Vec3::X * 0.1, false);
    client.pump_until(|c| c.position == to);
    client.after_ticks(2);
    // Where the server has them: a teleport to where they are says.
    client.teleported = None;
    client.connection.send(ClientMessage::Command { line: "/tp ~ ~ ~".into() });
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported, Some(to), "the stale step undid the teleport");
    // Moves sent after it count as ever.
    client.teleported = None;
    client.send_position(to + Vec3::X * 0.2, false);
    assert!(client.all_accepted(), "{:?}", client.teleported);
}

#[test]
fn going_silent_does_not_buy_more_time_hovering() {
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    client.fill(at, IVec3::new(3, -1, -3), IVec3::new(42, -1, 3), "air");
    let over = start + Vec3::X * 3.0;
    // Hovering a second and a half, a tick at a time...
    for _ in 0..30 {
        client.send_position(over, false);
        client.after_ticks(1);
    }
    assert!(client.teleported.is_none(), "let down before the grace ran out");
    // ...silent for two and a half (still alive to the server)...
    client.after_ticks(50);
    // ...and hovering on: the time aloft before the silence still counts, so they are let
    // down well before a fresh grace and sink would allow.
    let until = client.ticks() + 3 * TICKS_PER_SECOND as u64;
    while !client.teleported.is_some_and(|to| to.y < over.y) {
        assert!(client.ticks() < until, "hovered on after going silent");
        client.send_position(over, false);
        client.after_ticks(1);
    }
}

#[test]
fn a_player_silent_in_mid_jump_lands_without_being_pulled_back() {
    let mut client = on_a_test_floor();
    let start = client.position;
    // Up in a jump, then nothing for four seconds (a hitch, a dropped connection), then
    // the rest of the jump, down on the floor and walking on.
    client.send_position(start + Vec3::new(0.2, 1.0, 0.0), false);
    client.after_ticks(4 * TICKS_PER_SECOND as u64);
    client.send_position(start + Vec3::new(0.3, 0.8, 0.0), false);
    client.send_position(start + Vec3::new(0.4, 0.0, 0.0), false);
    for step in 1..=10 {
        client.send_position(start + Vec3::new(0.4 + step as f32 * 0.2, 0.0, 0.0), false);
        client.after_ticks(1);
    }
    assert!(client.all_accepted(), "pulled back to {:?} after landing", client.teleported);
}

/// Reads what the server answers a connection's hello until it turns them away, and
/// returns why (in English); panics if they are let in.
fn turned_away(connection: &ClientConnection, content: &Content) -> String {
    let deadline = Instant::now() + SERVER_WAIT;
    loop {
        assert!(Instant::now() < deadline, "the login was never answered");
        match connection.try_recv() {
            Ok(Some(ServerMessage::Rejected { reason })) => return content.english.render(&reason, &content.blocks),
            Ok(Some(ServerMessage::Welcome { .. })) => panic!("let in"),
            Ok(_) => std::thread::sleep(Duration::from_millis(1)),
            Err(_) => panic!("disconnected without a reason"),
        }
    }
}

fn say_hello(connection: &ClientConnection, name: &str, content: &Content) {
    connection.send(ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: None });
}

#[test]
fn a_name_already_playing_is_turned_away_in_any_case() {
    let dir = world_dir("names");
    let client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    for name in ["test", "TeSt"] {
        let (connection, server_side) = local_pair();
        client._server.as_ref().unwrap().connections().send(server_side).unwrap();
        say_hello(&connection, name, &client.content);
        let reason = turned_away(&connection, &client.content);
        assert!(reason.contains("already playing"), "{name}: {reason}");
    }
    drop(client);
    // One file for the name in any case (macOS and Windows would not tell them apart).
    assert!(dir.join("players/test.ron").is_file());
    let joined_as = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    drop(joined_as);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A server listening on a local port, with `host` joined (over TCP) as an admin.
fn serve(config: ServerConfig) -> (Client, std::net::SocketAddr) {
    let server = spawn(ServerConfig { view_distance: 2, admins: vec!["host".into()], idle_timeout: SERVER_WAIT, ..config }, content()).unwrap();
    let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    (Client::hello(tcp::connect(addr).unwrap(), "host", server), addr)
}

/// Reads a connection until the server closes it, or panics after `limit`.
fn closed_within(mut stream: std::net::TcpStream, limit: Duration) {
    use std::io::Read;
    stream.set_read_timeout(Some(limit)).unwrap();
    let mut buffer = [0u8; 4096];
    let started = Instant::now();
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return,
            Err(e) => panic!("the server kept the connection open for {:?}: {e}", started.elapsed()),
        }
        assert!(started.elapsed() < limit, "the server kept the connection open");
    }
}

#[test]
fn malformed_and_oversized_frames_and_silence_close_the_connection() {
    use std::io::Write;
    let (_host, addr) = serve(ServerConfig::default());
    // A frame larger than any message a client sends: closed before it is read.
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream.write_all(&(1u32 << 20).to_le_bytes()).unwrap();
    closed_within(stream, Duration::from_secs(10));
    // Bytes that are no message.
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream.write_all(&4u32.to_le_bytes()).unwrap();
    stream.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
    closed_within(stream, Duration::from_secs(10));
    // Connecting and never saying hello: closed after ten seconds of the server's clock.
    let stream = std::net::TcpStream::connect(addr).unwrap();
    closed_within(stream, Duration::from_secs(60));
}

#[test]
fn a_player_who_falls_silent_is_let_go_and_may_join_again() {
    let host = Client::join_with(ServerConfig { view_distance: 2, idle_timeout: Duration::from_secs(1), ..Default::default() });
    let addr = tcp::listen("127.0.0.1:0", host._server.as_ref().unwrap().connections()).unwrap();
    // The host plays in process, and is never let go however long it says nothing.
    for _ in 0..2 {
        let connection = tcp::connect(addr).unwrap();
        say_hello(&connection, "guest", &host.content);
        let mut welcomed = false;
        let deadline = Instant::now() + SERVER_WAIT;
        loop {
            assert!(Instant::now() < deadline, "a silent guest was kept");
            match connection.try_recv() {
                Ok(Some(ServerMessage::Welcome { .. })) => welcomed = true,
                Ok(Some(ServerMessage::Rejected { reason })) => panic!("{reason:?}"),
                Ok(_) => std::thread::sleep(Duration::from_millis(1)),
                Err(_) => break,
            }
        }
        assert!(welcomed, "the guest was let go before joining");
    }
    assert!(host._server.as_ref().unwrap().ticks() > 0);
}

#[test]
fn a_player_who_sends_nothing_but_keepalives_is_kept() {
    let host = Client::join();
    let addr = tcp::listen("127.0.0.1:0", host._server.as_ref().unwrap().connections()).unwrap();
    let mut guest = Client::greet(tcp::connect(addr).unwrap(), "guest", None);
    // Twenty seconds of the server's clock with no input, as from a minimized game: only
    // the connection's keepalives. (Being let go would end the pump with a panic.)
    let ticks = |c: &Client| c._server.as_ref().unwrap().ticks();
    let until = ticks(&host) + 20 * TICKS_PER_SECOND as u64;
    guest.pump_until(|_| ticks(&host) >= until);
    assert!(guest.ok("/seed").contains(char::is_numeric), "the guest was let go");
    drop(host);
}

#[test]
fn chat_floods_slow_down_once_and_admins_make_admins_for_the_session() {
    let dir = world_dir("op");
    let (mut host, addr) = serve(ServerConfig { world: Some(dir.clone()), ..Default::default() });
    assert!(host.admin, "a name on the admin list is an admin from this machine");
    let mut guest = Client::greet(tcp::connect(addr).unwrap(), "guest", None);
    assert!(!guest.admin);
    for i in 0..20 {
        guest.connection.send(ClientMessage::Chat { text: format!("spam {i}") });
    }
    // An input no player could make, whose correction comes after everything the flood
    // was answered with.
    guest.send_position(guest.position + Vec3::X * 100.0, false);
    guest.pump_until(|c| c.teleported.is_some());
    let said = guest.chat.iter().filter(|(kind, _)| *kind == ChatKind::Chat).count();
    let told = guest.chat.iter().filter(|(kind, text)| *kind == ChatKind::Error && text.contains("too quickly")).count();
    assert!((8..=10).contains(&said), "{said} lines of a flood of 20 were said");
    assert_eq!(told, 1, "told to slow down {told} times for one flood");
    // (The host hears the flood, so its own reply is not the first line it gets.)
    host.connection.send(ClientMessage::Command { line: "/op guest".into() });
    guest.pump_until(|c| c.admin);
    // ...until an admin says otherwise...
    host.connection.send(ClientMessage::Command { line: "/deop guest".into() });
    guest.pump_until(|c| !c.admin);
    host.connection.send(ClientMessage::Command { line: "/op guest".into() });
    guest.pump_until(|c| c.admin);
    // ...or they leave: it is never saved, so it lasts for the session only.
    drop(guest);
    gone(&mut host, "guest");
    let guest = Client::greet(tcp::connect(addr).unwrap(), "guest", None);
    assert!(!guest.admin, "a player made an admin was still one when they came back");
    // An admin by right (the admin list, from this machine) is told it lasts until they
    // join again.
    host.chat.clear();
    host.connection.send(ClientMessage::Command { line: "/deop host".into() });
    host.pump_until(|c| !c.admin && c.chat.iter().any(|(_, text)| text.contains("until you join again")));
    drop((guest, host));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_admin_token_makes_an_admin_from_anywhere() {
    let (_host, addr) = serve(ServerConfig { admin_token: Some("open sesame".into()), ..Default::default() });
    let wrong = Client::greet_with(tcp::connect(addr).unwrap(), "guest", None, Some("open sesame!"));
    assert!(!wrong.admin);
    let owner = Client::greet_with(tcp::connect(addr).unwrap(), "owner", None, Some("open sesame"));
    assert!(owner.admin);
}

#[test]
fn players_on_this_machine_are_known_over_ipv4_to_a_dual_stack_listener() {
    let server = spawn(ServerConfig { view_distance: 2, admins: vec!["host".into()], ..Default::default() }, content()).unwrap();
    let Ok(addr) = tcp::listen("[::]:0", server.connections()) else {
        eprintln!("skipped: no IPv6 here");
        return;
    };
    let Ok(connection) = tcp::connect(std::net::SocketAddr::from(([127, 0, 0, 1], addr.port()))) else {
        eprintln!("skipped: this IPv6 listener takes no IPv4");
        return;
    };
    // Seen as ::ffff:127.0.0.1.
    let host = Client::hello(connection, "host", server);
    assert!(host.admin, "a name on the admin list connecting from this machine over IPv4");
}

#[test]
fn a_full_server_turns_newcomers_away() {
    let (host, addr) = serve(ServerConfig { max_players: 1, ..Default::default() });
    let connection = tcp::connect(addr).unwrap();
    say_hello(&connection, "guest", &host.content);
    assert!(turned_away(&connection, &host.content).contains("full"));
}

#[test]
fn silent_sockets_neither_count_as_players_nor_pile_up() {
    // The operator lets two connections from one address wait to say hello.
    let limits = Limits { max_pending_per_address: 2, ..Default::default() };
    let server = spawn(ServerConfig { view_distance: 2, max_players: 3, idle_timeout: SERVER_WAIT, limits, ..Default::default() }, content()).unwrap();
    // Listening on IPv4, and on IPv6 too where the machine has it, so one machine has two
    // addresses to come from.
    let v4 = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    let v6 = tcp::listen("[::1]:0", server.connections()).ok();
    let guest = Client::hello(tcp::connect(v4).unwrap(), "guest", server);
    let second = Client::greet(tcp::connect(v4).unwrap(), "second", None);
    // Two connections from that address that never say hello: with them waiting, its next
    // connection is turned away at once...
    let silent: Vec<_> = (0..2).map(|_| std::net::TcpStream::connect(v4).unwrap()).collect();
    let connection = tcp::connect(v4).unwrap();
    assert!(turned_away(&connection, &guest.content).contains("full"));
    // ...while another address is not, and they are not players, so a third still fits
    // (in process, as from elsewhere, without IPv6).
    let third = match v6 {
        Some(v6) => Client::greet(tcp::connect(v6).unwrap(), "third", None),
        None => guest.another("third"),
    };
    assert!(third.health > 0);
    drop((guest, second, third, silent));
}

#[test]
fn an_address_gets_one_starting_kit_in_a_while() {
    // A day is too long to wait: a mod sets the window to two seconds.
    let mut content = content();
    let mut files = (*content.files).clone();
    files.push(voxmar_engine::content::ContentFile { package: "test".into(), name: "survival.ron".into(), source: "(kit_window: 2.0)".into() });
    content.files = std::sync::Arc::new(files);
    let server = spawn(ServerConfig { view_distance: 2, ..Default::default() }, content.clone()).unwrap();
    let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    let mut first = Client::greet_content(tcp::connect(addr).unwrap(), "first", Some(server), None, content.clone());
    let (bread, purse) = (content.blocks.expect("base:bread"), content.blocks.expect("base:coin_purse"));
    let has = |c: &Client, item| c.inventory.slots.iter().flatten().any(|s| s.item == item);
    first.pump_until(|c| has(c, bread));
    // A second fresh name from the same address, inside the window: the purse, no kit.
    let mut second = Client::greet_content(tcp::connect(addr).unwrap(), "second", None, None, content.clone());
    second.pump_until(|c| has(c, purse));
    assert_eq!(second.inventory.slots.iter().flatten().count(), 1, "{:?}", second.inventory);
    // Once the window has passed by the server's clock, the next one has the kit again.
    first.after_ticks(2 * TICKS_PER_SECOND as u64);
    let mut third = Client::greet_content(tcp::connect(addr).unwrap(), "third", None, None, content);
    third.pump_until(|c| has(c, bread));
    drop((second, third, first));
}

#[test]
fn a_new_player_starts_with_tools_food_light_wood_and_the_purse() {
    let client = Client::join();
    let id = |name: &str| client.content.blocks.expect(name);
    let carried = |item| client.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    assert_eq!([carried(id("base:bread")), carried(id("base:torch")), carried(id("base:oak_planks"))], [4, 8, 16], "{:?}", client.inventory);
    assert_eq!([carried(id("base:wooden_pickaxe")), carried(id("base:wooden_axe"))], [1, 1], "{:?}", client.inventory);
    assert_eq!(client.inventory.slots[voxmar_engine::inventory::SLOTS - 1].map(|s| s.item), Some(id("base:coin_purse")));
    assert_eq!(carried(id("base:cobblestone")) + carried(id("base:glass")) + carried(id("base:glowstone")), 0);
}

#[test]
fn lava_burns_by_the_survival_rules_and_the_void_kills() {
    let mut client = on_a_test_floor();
    let rules = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core/survival.ron")).unwrap();
    assert!(rules.contains("burn: 4,"), "the test reads lava's burn as 4");
    let feet = client.position.floor().as_ivec3();
    client.ok(&format!("/setblock {} {} {} lava", feet.x, feet.y, feet.z));
    client.pump_until(|c| c.health < 20);
    assert_eq!(client.health, 16, "the first burn takes 4");
    client.pump_until(|c| c.health < 16);
    assert_eq!(client.health, 12, "and so does the next");
    // Out of the lava and down below the world.
    client.ok(&format!("/setblock {} {} {} air", feet.x, feet.y, feet.z));
    let pos = client.position;
    client.ok(&format!("/tp {} -20 {}", pos.x, pos.z));
    client.pump_until(|c| c.health == 0);
}

#[test]
fn what_is_lost_in_the_void_or_lava_waits_where_the_player_last_stood() {
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // A pit of lava two deep at x+3..x+5, walled in stone.
    client.fill(at, IVec3::new(2, -3, -2), IVec3::new(6, -1, 2), "stone");
    client.fill(at, IVec3::new(3, -2, -1), IVec3::new(5, -1, 1), "lava");
    let deaths = [
        format!("/tp {} -20 {}", start.x, start.z),
        // Into the lava in one step from where they stand.
        String::new(),
    ];
    for (i, death) in deaths.iter().enumerate() {
        give_a_load(&mut client);
        // A tick on the floor, so the server knows they stood there.
        client.after_ticks(2);
        if death.is_empty() {
            client.send_position(start + Vec3::new(4.0, -2.0, 0.0), false);
        } else {
            client.ok(death);
        }
        client.pump_until(|c| c.died > i as u32);
        client.propose("core:respawn", ActionTarget::None);
        client.pump_until(|c| c.health > 0 && carried(c) == 3);
        // Back where they last stood, everything is there to pick up again.
        client.tp(start);
        client.pump_until(|c| carried(c) == 47);
    }
}

#[test]
fn fires_warm_and_water_chills() {
    let mut client = on_a_test_floor();
    // Survival judges warmth once a second: a few judgements on the floor give the air's.
    client.after_ticks(3 * TICKS_PER_SECOND as u64);
    let feet = client.position.floor().as_ivec3();
    let air = client.vitals.felt;
    // A glowstone a block away warms by most of the rules' 14 degrees.
    client.ok(&format!("/setblock {} {} {} glowstone", feet.x + 1, feet.y, feet.z));
    client.pump_until(|c| c.vitals.felt >= air + 5.0);
    client.ok(&format!("/setblock {} {} {} air", feet.x + 1, feet.y, feet.z));
    client.pump_until(|c| c.vitals.felt < air + 5.0);
    // Water at the feet chills by the rules' 8 degrees, and more if soaked.
    client.ok(&format!("/setblock {} {} {} water", feet.x, feet.y, feet.z));
    client.pump_until(|c| c.vitals.felt <= air - 7.0);
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
    client.after_ticks(2);
    assert_eq!(client.world.block(at), Some(ripe), "a level 1 farmer harvested Farming 30 tomatoes");
    assert_eq!(client.earned.len(), earned, "no experience for a refused harvest");
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    let earned = client.earned.len();
    for _ in 0..3 {
        client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout));
        client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    }
    client.after_ticks(2);
    assert_eq!(client.earned.len(), earned, "sowing and breaking sprouts paid experience");
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(moonbloom));
    let earned = client.earned.len();
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    // Its plain drop: the flower, and nothing foraged or taught.
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == moonbloom) || c.items.values().any(|i| i.stack.item == moonbloom));
    client.after_ticks(2);
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
    client.ok("/time");
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == stick), "crafting all went on into another recipe");
    assert_eq!(client.earned[earned..], [80, 180], "one table's Engineering, once, then the quest's reward");
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
