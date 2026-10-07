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
use voxmar_server::save_rules::SaveRules;
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
    /// The server's tick, as it last told (once a second): the clock waits are judged by.
    tick: u64,
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
            tick: 0,
            creatures: Default::default(),
            deaths: 0,
            blows: 0,
            damage: Vec::new(),
            journal: Default::default(),
            quests: Vec::new(),
            _server: server,
        };
        // Joined once it stands in its land and has been told the time.
        client.pump_until(|c| c.health > 0 && c.world.contains(ColumnPos::of_block(c.position.as_ivec3())) && c.day.is_some());
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
            ServerMessage::Unload { pos } => {
                self.world.remove(pos);
            }
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
                self.tick = tick;
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
    assert!(client.ok("/biome").starts_with("base:"));
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
fn a_listed_admin_is_one_under_their_name_in_any_case() {
    // A name is one player in any case, so the admin list folds case too.
    let mut boss = Client::join_remote("BOSS", &["boss"]);
    boss.ok("/time set noon");
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
    client.ok("/give stone_pickaxe 1");
    client.pump_until(|c| c.world.block(ore) == c.content.blocks.id("base:iron_ore") && c.inventory.get(4).is_some());
    let iron = client.world.block(ore).unwrap();
    let mine = |client: &Client| client.propose("core:mine", ActionTarget::Block { block: ore, face: Face::PosY });
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };
    // At Mining 1, even with the pickaxe: refused.
    hold(&client, 4);
    mine(&client);
    let waited = Instant::now() + Duration::from_millis(1500);
    client.pump_until(|_| Instant::now() > waited);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke at Mining 1");
    // At Mining 15 with a bare hand: refused too.
    client.ok("/skill mining 15");
    hold(&client, 5);
    mine(&client);
    let waited = Instant::now() + Duration::from_millis(1500);
    client.pump_until(|_| Instant::now() > waited);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke without a pickaxe");
    // At Mining 15 with the stone pickaxe: broken, quicker than by hand, teaching Mining (35).
    hold(&client, 4);
    let started = Instant::now();
    mine(&client);
    client.pump_until(|c| c.world.block(ore) == Some(BlockId::AIR) && c.earned.contains(&350));
    let hardness = client.content.blocks.info(iron).hardness;
    assert!(started.elapsed().as_secs_f32() < hardness * 0.5, "a stone pickaxe should mine iron ore at four times a hand's speed");
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
    let waited = Instant::now() + Duration::from_millis(800);
    client.pump_until(|_| Instant::now() > waited);
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
    // Wait for the ore itself: the starting kit fills slot 0 before /clear and /give arrive.
    client.pump_until(|c| c.world.block(core) == Some(crucible) && count(c, ore) == 2);
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
fn a_boiler_drives_a_steam_crusher_that_doubles_ore() {
    let mut client = Client::join();
    let boiler = client.position.floor().as_ivec3() + IVec3::new(3, 0, 0);
    let crusher = boiler + IVec3::X;
    for line in ["/clear", "/fill ~3 ~ ~-1 ~4 ~1 ~1 air", "/setblock ~3 ~ ~ boiler", "/setblock ~4 ~ ~ steam_crusher"] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (coal, ore, dust) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ore"), blocks.expect("base:copper_dust"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    let use_at = |client: &mut Client, at: IVec3| {
        client.chat.clear();
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
        client.propose("base:use_machine", ActionTarget::Block { block: at, face: Face::NegX });
    };
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none) && c.world.block(crusher) == c.content.blocks.id("base:steam_crusher"));
    // A crusher with nothing to work says so; coal in hand goes into the boiler.
    use_at(&mut client, crusher);
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Steam Crusher: idle")));
    client.ok("/give coal_ore 2");
    client.pump_until(|c| count(c, coal) == 2);
    use_at(&mut client, boiler);
    client.pump_until(|c| count(c, coal) == 0 && c.chat.iter().any(|(_, t)| t.contains("Put in 2")));
    // Ore in hand goes into the crusher, which works it on the boiler's steam.
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, ore) == 1);
    use_at(&mut client, crusher);
    client.pump_until(|c| count(c, ore) == 0 && c.chat.iter().any(|(_, t)| t.contains("making")));
    // With an empty hand, what it made comes out: two dust from one ore.
    let started = client.tick;
    while count(&client, dust) == 0 {
        assert!(server_seconds(&client, started) < 30, "the crusher made nothing: {:?}", client.chat);
        wait_server(&mut client, 1);
        use_at(&mut client, crusher);
        client.pump_until(|c| !c.chat.is_empty());
    }
    assert_eq!(count(&client, dust), 2);
}

#[test]
fn crushed_dust_smelts_in_the_crucible_kiln_like_its_ore() {
    let mut client = Client::join();
    let core = client.position.floor().as_ivec3() + IVec3::new(4, 0, 0);
    for line in [
        "/clear",
        "/fill ~3 ~ ~-1 ~5 ~2 ~1 air",
        "/fill ~3 ~ ~-1 ~5 ~ ~1 kiln_bricks",
        "/setblock ~4 ~ ~ crucible",
        "/setblock ~3 ~1 ~-1 kiln_bricks",
        "/setblock ~5 ~1 ~-1 kiln_bricks",
        "/setblock ~3 ~1 ~1 kiln_bricks",
        "/setblock ~5 ~1 ~1 kiln_bricks",
        "/give copper_dust 2",
        "/skill engineering 20",
    ] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (crucible, dust, ingot) = (blocks.expect("base:crucible"), blocks.expect("base:copper_dust"), blocks.expect("base:copper_ingot"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(core) == Some(crucible) && c.world.block(core + IVec3::new(1, 1, 1)) == c.content.blocks.id("base:kiln_bricks") && count(c, dust) == 2);
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:cook", ActionTarget::Block { block: core, face: Face::PosY });
    client.pump_until(|c| count(c, ingot) == 1 && count(c, dust) == 1);
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
fn a_turbine_powers_a_washer_through_a_transformer_once_water_lies_beside_it() {
    let mut client = Client::join();
    let transformer = client.position.floor().as_ivec3() + IVec3::new(3, 0, 0);
    let washer = transformer + IVec3::X;
    // Boiler and turbine above the transformer (MV in on top), the washer beside it (LV).
    for line in [
        "/clear",
        "/fill ~3 ~ ~-1 ~6 ~1 ~1 air",
        "/setblock ~3 ~ ~ lv_transformer",
        "/setblock ~3 ~1 ~ steam_turbine",
        "/setblock ~4 ~1 ~ boiler",
        "/setblock ~4 ~ ~ ore_washer",
    ] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (coal, ore, dust) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ore"), blocks.expect("base:copper_dust"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    let use_at = |client: &mut Client, at: IVec3| {
        client.chat.clear();
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
        client.propose("base:use_machine", ActionTarget::Block { block: at, face: Face::NegX });
    };
    client.pump_until(|c| c.world.block(washer) == c.content.blocks.id("base:ore_washer") && c.inventory.slots.iter().all(Option::is_none));
    client.ok("/give coal_ore 4");
    client.pump_until(|c| count(c, coal) == 4);
    use_at(&mut client, washer + IVec3::new(0, 1, 0));
    client.pump_until(|c| count(c, coal) == 0);
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, ore) == 1);
    use_at(&mut client, washer);
    client.pump_until(|c| count(c, ore) == 0);
    // Dry, it waits and says what it needs.
    let started = client.tick;
    loop {
        assert!(server_seconds(&client, started) < 20, "the washer never said it needs water: {:?}", client.chat);
        wait_server(&mut client, 1);
        use_at(&mut client, washer);
        client.pump_until(|c| !c.chat.is_empty());
        if client.chat.iter().any(|(_, t)| t.contains("needs water")) {
            break;
        }
    }
    // `/machine` tells the same.
    assert!(client.ok("/machine ~4 ~ ~").contains("needs water"));
    // Water beside it: three dust for the one ore.
    client.ok("/setblock ~5 ~ ~ water");
    let started = client.tick;
    while count(&client, dust) < 3 {
        assert!(server_seconds(&client, started) < 40, "the washer made {} dust: {:?}", count(&client, dust), client.chat);
        wait_server(&mut client, 1);
        use_at(&mut client, washer);
        client.pump_until(|c| !c.chat.is_empty());
    }
    assert_eq!(count(&client, dust), 3);
}

#[test]
fn an_aurochs_on_a_treadwheel_blows_the_bellows_of_a_bloomery_that_makes_a_bloom() {
    let mut client = Client::join();
    let core = client.position.floor().as_ivec3() + IVec3::new(4, 0, 0);
    for line in ["/clear", "/fill ~3 ~ ~-1 ~8 ~2 ~3 air", "/setblock ~4 ~ ~ bloomery"] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (ore, coal, bloom) = (blocks.expect("base:iron_ore"), blocks.expect("base:coal_ore"), blocks.expect("base:iron_bloom"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    let use_at = |client: &mut Client, at: IVec3| {
        client.chat.clear();
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
        client.propose("base:use_machine", ActionTarget::Block { block: at, face: Face::NegX });
    };
    client.pump_until(|c| c.world.block(core) == c.content.blocks.id("base:bloomery") && c.inventory.slots.iter().all(Option::is_none));
    // Alone, the bloomery says what its structure needs.
    use_at(&mut client, core);
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("needs kiln bricks")));
    // The shaft round it, bellows in its ring, a shaft out to a treadwheel, and an aurochs,
    // tamed and held, beside the wheel.
    for line in [
        "/fill ~3 ~ ~-1 ~5 ~2 ~1 kiln_bricks",
        "/fill ~4 ~1 ~ ~4 ~2 ~ air",
        "/setblock ~4 ~ ~ bloomery",
        "/setblock ~5 ~ ~ bellows",
        "/setblock ~6 ~ ~ shaft",
        "/setblock ~7 ~ ~ treadwheel",
        "/summon aurochs 1 ~7 ~ ~2",
        "/creatures tame aurochs",
        "/creatures hold aurochs",
        "/give iron_ore 2",
        "/give coal_ore 1",
    ] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(core + IVec3::X) == c.content.blocks.id("base:bellows") && count(c, ore) == 2 && count(c, coal) == 1);
    use_at(&mut client, core);
    client.pump_until(|c| count(c, ore) == 0);
    // The coal is in the next slot now selected... put it in from wherever it lies.
    client.ok("/clear");
    client.ok("/give coal_ore 1");
    client.pump_until(|c| count(c, coal) == 1 && c.inventory.get(0).is_some_and(|s| s.item == coal));
    use_at(&mut client, core);
    client.pump_until(|c| count(c, coal) == 0 && c.chat.iter().any(|(_, t)| t.contains("making")));
    client.ok("/clear");
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
    // Twenty seconds of work at the bellows: an empty hand takes the bloom.
    let started = client.tick;
    while count(&client, bloom) == 0 {
        if server_seconds(&client, started) > 60 {
            let wheel = client.ok("/machine ~7 ~ ~");
            panic!("the bloomery made no bloom: {:?}; the wheel: {wheel}", client.chat);
        }
        wait_server(&mut client, 1);
        use_at(&mut client, core);
        client.pump_until(|c| !c.chat.is_empty());
    }
    assert_eq!(count(&client, bloom), 1);
    // One animal walks one wheel: a second wheel beside it, farther from the aurochs, stays
    // still while the first turns.
    client.ok("/setblock ~8 ~ ~ treadwheel");
    let started = client.tick;
    while !client.ok("/machine ~8 ~ ~").contains("needs a kept animal") {
        assert!(server_seconds(&client, started) < 10, "one aurochs walks two wheels");
        wait_server(&mut client, 1);
    }
    assert!(!client.ok("/machine ~7 ~ ~").contains("needs a kept animal"), "the nearer wheel still turns");
    // The aurochs, kept, can be taken away again; then the wheel stops.
    assert!(client.ok("/creatures remove aurochs").contains("for 1 creatures"));
    let started = client.tick;
    while !client.ok("/machine ~7 ~ ~").contains("needs a kept animal") {
        assert!(server_seconds(&client, started) < 10, "the wheel still turns without its aurochs");
        wait_server(&mut client, 1);
    }
}

#[test]
fn a_coke_oven_bakes_coal_into_coke_that_a_blast_furnace_blows_with_ore_into_two_ingots_an_ore() {
    let mut client = Client::join();
    let start = client.position.floor().as_ivec3();
    // The coke oven: a chamber of kiln bricks three on a side round a hollow middle, the
    // oven in the middle of its south face.
    let oven = start + IVec3::new(-3, 1, 3);
    // The blast furnace: the bloomery's ring four high, bellows in it, blown by an aurochs
    // on a treadwheel out east.
    let furnace = start + IVec3::new(4, 0, 0);
    for line in [
        "/clear",
        "/fill ~-5 ~ ~ ~8 ~4 ~5 air",
        "/fill ~-4 ~ ~1 ~-2 ~2 ~3 kiln_bricks",
        "/setblock ~-3 ~1 ~2 air",
        "/setblock ~-3 ~1 ~3 coke_oven",
        "/fill ~3 ~ ~-1 ~5 ~3 ~1 kiln_bricks",
        "/fill ~4 ~1 ~ ~4 ~3 ~ air",
        "/setblock ~4 ~ ~ blast_furnace",
        "/setblock ~5 ~ ~ bellows",
        "/setblock ~6 ~ ~ shaft",
        "/setblock ~7 ~ ~ treadwheel",
        "/summon aurochs 1 ~7 ~ ~2",
        "/creatures tame aurochs",
        "/creatures hold aurochs",
        "/give coal_ore 1",
    ] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (coal, coke, ore, ingot) = (blocks.expect("base:coal_ore"), blocks.expect("base:coke"), blocks.expect("base:iron_ore"), blocks.expect("base:iron_ingot"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| count(c, coal) == 1 && c.world.block(oven) == c.content.blocks.id("base:coke_oven") && c.world.block(furnace + IVec3::X) == c.content.blocks.id("base:bellows"));
    // The coal goes in, and the oven bakes it by its own heat.
    use_machine(&mut client, oven);
    client.pump_until(|c| count(c, coal) == 0);
    machine_says(&mut client, &format!("{} {} {}", oven.x, oven.y, oven.z), "making Coke", 10);
    // Taken out once baked (each `/seed` answer comes after the server dealt with the take).
    let started = client.tick;
    while count(&client, coke) == 0 {
        assert!(server_seconds(&client, started) < 60, "the coke oven baked no coke: {}", client.ok(&format!("/machine {} {} {}", oven.x, oven.y, oven.z)));
        wait_server(&mut client, 1);
        use_machine(&mut client, oven);
        client.ok("/seed");
    }
    assert_eq!(count(&client, coke), 1);
    // Two ore and the coke into the blast furnace: four ingots.
    client.ok("/give iron_ore 2");
    client.pump_until(|c| count(c, ore) == 2);
    for item in [ore, coke] {
        let slot = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == item)).expect("held") as u8;
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
        client.propose("base:use_machine", ActionTarget::Block { block: furnace, face: Face::NegX });
        client.pump_until(|c| count(c, item) == 0);
    }
    let started = client.tick;
    while count(&client, ingot) == 0 {
        assert!(server_seconds(&client, started) < 60, "the blast furnace made no iron: {}", client.ok(&format!("/machine {} {} {}", furnace.x, furnace.y, furnace.z)));
        wait_server(&mut client, 1);
        client.ok("/creatures hold aurochs");
        use_machine(&mut client, furnace);
        client.ok("/seed");
    }
    assert_eq!(count(&client, ingot), 4, "two ore and a coke, four ingots");
}

#[test]
fn a_hopper_feeds_a_crusher_by_conveyor_and_a_chest_above_collects_its_dust() {
    let mut client = Client::join();
    let start = client.position.floor().as_ivec3();
    let (boiler, hopper) = (start + IVec3::new(4, 0, -1), start + IVec3::new(2, 0, 0));
    for line in [
        "/clear",
        "/fill ~2 ~ ~-1 ~4 ~2 ~1 air",
        "/setblock ~4 ~ ~-1 boiler",
        "/setblock ~4 ~ ~ steam_crusher",
        "/setblock ~3 ~ ~ conveyor",
        "/setblock ~2 ~ ~ hopper",
        "/setblock ~4 ~1 ~ chute",
        "/setblock ~4 ~2 ~ chest",
    ] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (coal, ore) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ore"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    let use_at = |client: &mut Client, at: IVec3| {
        client.chat.clear();
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
        client.propose("base:use_machine", ActionTarget::Block { block: at, face: Face::NegX });
    };
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none) && c.world.block(start + IVec3::new(4, 2, 0)) == c.content.blocks.id("base:chest"));
    let conveyor_says = client.ok("/machine ~3 ~ ~");
    assert!(conveyor_says.contains("moves up to 4 items a second"), "{conveyor_says}");
    client.ok("/give coal_ore 2");
    client.pump_until(|c| count(c, coal) == 2);
    use_at(&mut client, boiler);
    client.pump_until(|c| count(c, coal) == 0);
    // Ore put into the hopper rides the conveyor into the crusher; the crusher's dust goes
    // up the chute into the chest, not back into the hopper.
    client.ok("/give copper_ore 2");
    client.pump_until(|c| count(c, ore) == 2);
    client.earned.clear();
    use_at(&mut client, hopper);
    client.pump_until(|c| count(c, ore) == 0 && c.chat.iter().any(|(_, t)| t.contains("Put in 2")));
    let started = client.tick;
    loop {
        let chest_says = client.ok("/machine ~4 ~2 ~");
        if chest_says.contains("holds 4 items") {
            break;
        }
        assert!(server_seconds(&client, started) < 40, "the chest got no dust: {chest_says}; crusher: {}", client.ok("/machine ~4 ~ ~"));
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(client.ok("/machine ~2 ~ ~").contains("holds 0 items"));
    // A running line earns nothing: Engineering xp is for making and placing its parts.
    assert!(client.earned.is_empty(), "the running line paid xp: {:?}", client.earned);
}

/// Right clicks the machine at `at` with what is in the first slot (`base:use_machine`).
fn use_machine(client: &mut Client, at: IVec3) {
    client.chat.clear();
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:use_machine", ActionTarget::Block { block: at, face: Face::NegX });
}

/// Server seconds since `tick`, by the clock it last told.
fn server_seconds(client: &Client, tick: u64) -> u64 {
    client.tick.saturating_sub(tick) / TICKS_PER_SECOND as u64
}

/// Waits `seconds` of the server's clock, from the next time it tells it.
fn wait_server(client: &mut Client, seconds: u64) {
    let told = client.tick;
    client.pump_until(|c| c.tick > told);
    let start = client.tick;
    client.pump_until(|c| c.tick >= start + seconds * TICKS_PER_SECOND as u64);
}

/// Asks `/machine` about a block until its answer contains `want`, or panics after `seconds`
/// of the server's clock.
fn machine_says(client: &mut Client, at: &str, want: &str, seconds: u64) -> String {
    let started = client.tick;
    loop {
        let said = client.ok(&format!("/machine {at}"));
        if said.contains(want) {
            return said;
        }
        assert!(server_seconds(client, started) < seconds, "/machine {at} never said `{want}`: {said}");
        wait_server(client, 1);
    }
}

#[test]
fn a_processing_array_works_only_inside_its_cube_of_hv_casing() {
    let mut client = Client::join();
    let p = client.position.floor().as_ivec3();
    let at = |v: IVec3| format!("{} {} {}", v.x, v.y, v.z);
    // The cube's corner; the array in the middle of its near face.
    let cube = p + IVec3::new(3, 0, 2);
    let core = cube + IVec3::new(0, 1, 1);
    for line in [
        "/clear".to_owned(),
        format!("/fill {} {} air", at(cube - IVec3::ONE), at(cube + IVec3::splat(3))),
        format!("/setblock {} processing_array", at(core)),
    ] {
        client.ok(&line);
    }
    let ore = client.content.blocks.expect("base:copper_ore");
    let count = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == ore).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(core) == c.content.blocks.id("base:processing_array") && c.inventory.slots.iter().all(Option::is_none));
    client.ok("/give copper_ore 8");
    client.pump_until(|c| count(c) == 8);
    use_machine(&mut client, core);
    client.pump_until(|c| count(c) == 0);
    // Alone, it says its structure is unfinished.
    machine_says(&mut client, &at(core), "not complete", 10);
    // The cube round it, hollow in the middle: it stands, and wants only power.
    for line in [
        format!("/fill {} {} hv_casing", at(cube + IVec3::X), at(cube + IVec3::new(2, 2, 2))),
        format!("/fill {} {} hv_casing", at(cube), at(cube + IVec3::new(0, 0, 2))),
        format!("/fill {} {} hv_casing", at(cube + IVec3::new(0, 2, 0)), at(cube + IVec3::new(0, 2, 2))),
        format!("/setblock {} hv_casing", at(core - IVec3::Z)),
        format!("/setblock {} hv_casing", at(core + IVec3::Z)),
        format!("/setblock {} air", at(cube + IVec3::ONE)),
    ] {
        client.ok(&line);
    }
    machine_says(&mut client, &at(core), "no power", 10);
    // Filled in, it is no cube of HV casing.
    client.ok(&format!("/setblock {} hv_casing", at(cube + IVec3::ONE)));
    machine_says(&mut client, &at(core), "not complete", 10);
}

#[test]
fn a_turbine_hall_gives_hv_from_steam_only_while_its_hall_stands_and_transformers_bring_it_to_lv() {
    let mut client = Client::join();
    let p = client.position.floor().as_ivec3();
    let at = |v: IVec3| format!("{} {} {}", v.x, v.y, v.z);
    // The hall's corner; its core at the top of its near end, steam from a boiler above it,
    // HV out of its end into a cable on an MV transformer, on an LV transformer beside a
    // macerator.
    let hall = p + IVec3::new(4, 0, 2);
    let core = hall + IVec3::new(0, 2, 1);
    let boiler = core + IVec3::Y;
    let cable = core - IVec3::X;
    let mv = cable - IVec3::Y;
    let lv = mv - IVec3::Y;
    let macerator = lv - IVec3::X;
    let far = hall + IVec3::new(4, 2, 2);
    for line in [
        "/clear".to_owned(),
        format!("/fill {} {} air", at(hall - IVec3::new(3, 0, 1)), at(far + IVec3::new(1, 2, 1))),
        format!("/setblock {} turbine_hall", at(core)),
        format!("/setblock {} boiler", at(boiler)),
        format!("/setblock {} hv_cable", at(cable)),
        format!("/setblock {} mv_transformer", at(mv)),
        format!("/setblock {} lv_transformer", at(lv)),
        format!("/setblock {} macerator", at(macerator)),
    ] {
        client.ok(&line);
    }
    let blocks = &client.content.blocks;
    let (coal, ore, dust) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ore"), blocks.expect("base:copper_dust"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(macerator) == c.content.blocks.id("base:macerator") && c.inventory.slots.iter().all(Option::is_none));
    client.ok("/give coal_ore 4");
    client.pump_until(|c| count(c, coal) == 4);
    use_machine(&mut client, boiler);
    client.pump_until(|c| count(c, coal) == 0);
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, ore) == 1);
    use_machine(&mut client, macerator);
    client.pump_until(|c| count(c, ore) == 0);
    // The core alone is no turbine: it says so, and the macerator has no power.
    machine_says(&mut client, &at(core), "not complete", 10);
    machine_says(&mut client, &at(macerator), "no power", 10);
    // The hall of casing round it, hollow along its middle.
    for line in [
        format!("/fill {} {} hv_casing", at(hall), at(far)),
        format!("/fill {} {} air", at(hall + IVec3::new(1, 1, 1)), at(hall + IVec3::new(3, 1, 1))),
        format!("/setblock {} turbine_hall", at(core)),
    ] {
        client.ok(&line);
    }
    machine_says(&mut client, &at(macerator), "making", 20);
    let started = client.tick;
    while count(&client, dust) < 2 {
        assert!(server_seconds(&client, started) < 30, "the macerator made {} dust: {}", count(&client, dust), client.ok(&format!("/machine {}", at(macerator))));
        wait_server(&mut client, 1);
        use_machine(&mut client, macerator);
        client.ok("/seed");
    }
    assert_eq!(count(&client, dust), 2, "a macerator makes two dust an ore");
    // A casing broken out of the hall: the turbine stops, and a player at the macerator
    // sees it has no power within a survey (`survey_seconds`, 1) and the converters' lag.
    client.ok(&format!("/setblock {} air", at(hall + IVec3::new(2, 0, 0))));
    let broken = client.tick;
    client.ok("/clear");
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, ore) == 1);
    use_machine(&mut client, macerator);
    client.pump_until(|c| count(c, ore) == 0);
    machine_says(&mut client, &at(macerator), "no power", 2);
    let after = client.tick - broken;
    assert!(after <= 2 * TICKS_PER_SECOND as u64, "the macerator ran on for {after} ticks");
    machine_says(&mut client, &at(core), "not complete", 1);
    // Nor does it make any more: the ore is still there, with no dust.
    wait_server(&mut client, 3);
    let said = machine_says(&mut client, &at(macerator), "no power", 1);
    use_machine(&mut client, macerator);
    client.ok("/seed");
    assert_eq!(count(&client, dust), 0, "the macerator made dust with its hall broken: {said}");
}

#[test]
fn chests_keep_what_they_hold_across_restarts_and_an_unreadable_machines_file_is_kept_aside() {
    let dir = world_dir("machines");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let chest = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let at = format!("{} {} {}", chest.x, chest.y, chest.z);
    for line in ["/clear", &format!("/setblock {at} chest"), "/give copper_ore 5"] {
        client.ok(line);
    }
    let ore = client.content.blocks.expect("base:copper_ore");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == ore && s.count == 5) && c.world.block(chest) == c.content.blocks.id("base:chest"));
    use_machine(&mut client, chest);
    machine_says(&mut client, &at, "holds 5 items", 10);
    drop(client);
    assert!(std::fs::read_to_string(dir.join("machines.ron")).unwrap().contains("chest"));

    let mut client = Client::join_with(config());
    machine_says(&mut client, &at, "holds 5 items", 10);

    // Work half done is not done again after a restart.
    let crusher = chest + IVec3::new(0, 0, 2);
    let crusher_at = format!("{} {} {}", crusher.x, crusher.y, crusher.z);
    for line in [
        "/clear".to_string(),
        format!("/setblock {} {} {} boiler", crusher.x + 1, crusher.y, crusher.z),
        format!("/setblock {crusher_at} steam_crusher"),
        "/give coal_ore 1".to_string(),
    ] {
        client.ok(&line);
    }
    let coal = client.content.blocks.expect("base:coal_ore");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == coal) && c.world.block(crusher) == c.content.blocks.id("base:steam_crusher"));
    use_machine(&mut client, crusher + IVec3::X);
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
    client.ok("/give iron_ore 1");
    client.pump_until(|c| c.inventory.slots.iter().any(Option::is_some));
    use_machine(&mut client, crusher);
    let progress = |said: &str| said.split(", ").find_map(|part| part.strip_suffix("% done")).and_then(|p| p.trim_end_matches('.').parse::<u32>().ok());
    let started = client.tick;
    let done = loop {
        let said = client.ok(&format!("/machine {crusher_at}"));
        if let Some(done) = progress(&said).filter(|&p| p >= 40) {
            break done;
        }
        assert!(server_seconds(&client, started) < 20, "the crusher never got going: {said}");
        std::thread::sleep(Duration::from_millis(200));
    };
    drop(client);
    let save = std::fs::read_to_string(dir.join("machines.ron")).unwrap();
    assert!(save.contains("base:iron_dust"), "the save keeps the work: {save}");
    let mut client = Client::join_with(config());
    // Until its first step it says it is idle; then it goes on from where it was.
    let said = machine_says(&mut client, &crusher_at, "making", 20);
    assert!(progress(&said).is_some_and(|p| p >= done), "the crusher began again: {said}, was {done}% done");
    // Finished, the one ore made two dust: not two lots of it, nor none. The limit is the
    // server's time with no progress: a crowded test machine runs the server slow.
    let (mut moved, mut last) = (client.tick, progress(&said));
    loop {
        let said = client.ok(&format!("/machine {crusher_at}"));
        if said.contains("idle") {
            break;
        }
        if progress(&said) != last {
            (moved, last) = (client.tick, progress(&said));
        }
        assert!(server_seconds(&client, moved) < 20, "the crusher stopped after the restart: {said}");
        std::thread::sleep(Duration::from_millis(300));
    }
    let dust = client.content.blocks.expect("base:iron_dust");
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == dust).map(|s| s.count as u32).sum::<u32>();
    use_machine(&mut client, crusher);
    client.pump_until(|c| held(c) > 0);
    let made = held(&client);
    assert_eq!(made, 2, "one iron ore crushed across a restart");
    drop(client);

    // A machines file that cannot be read is kept aside before the next save writes over it,
    // the server goes on, and the machines standing in saved land join the factory again,
    // empty.
    std::fs::write(dir.join("machines.ron"), "not a machine").unwrap();
    let mut client = Client::join_with(config());
    client.pump_until(|c| c.world.block(chest) == c.content.blocks.id("base:chest"));
    client.ok(&format!("/machine {at}"));
    drop(client);
    let aside: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().contains("machines.unreadable.")).collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "not a machine");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// How many items a machine's `/machine` answer says it holds: "… holds 64 items." (none
/// said, none held).
fn items_held(said: &str) -> u32 {
    let words: Vec<_> = said.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric())).collect();
    words.windows(2).find_map(|w| (w[1] == "items" || w[1] == "item").then(|| w[0].parse().ok()).flatten()).unwrap_or(0)
}

/// When the server saves, as the content says.
fn save_rules() -> SaveRules {
    SaveRules::load(&content().files).expect("saves.ron")
}

/// The generation of the save in `dir`, from its `world.ron`.
fn saved_generation(dir: &std::path::Path) -> u64 {
    let meta = std::fs::read_to_string(dir.join("world.ron")).expect("world.ron");
    let at = meta.find("generation:").expect("a generation") + "generation:".len();
    meta[at..].trim_start().split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok()).expect("a number")
}

/// Waits until the server has finished the loop that handled what was sent before now: a
/// `/seed` answer, then the next time it tells, sent by a later step of that loop or after.
fn after_this_loop(client: &mut Client) {
    client.ok("/seed");
    let told = client.tick;
    client.pump_until(|c| c.tick > told);
}

/// Waits out the time in which a player leaving brings a save: whatever saves were due by
/// then are done (the time is told after a save).
fn after_leave_saves(client: &mut Client) {
    after_this_loop(client);
    wait_server(client, save_rules().after_leaving.div_ceil(TICKS_PER_SECOND as u64) + 1);
}

/// Saves the server made on its own between ticks `from` and `to`, at most.
fn autosaves(from: u64, to: u64) -> u64 {
    let every = save_rules().every;
    (to / every).saturating_sub(from.saturating_sub(TICKS_PER_SECOND as u64) / every)
}

#[test]
fn only_joined_players_leaving_save_the_world_and_many_leaving_at_once_save_it_once() {
    let dir = world_dir("leavers");
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    after_this_loop(&mut client);
    let (before, from) = (saved_generation(&dir), client.tick);
    // Three connections that close without a hello: none of them is a player leaving.
    for _ in 0..3 {
        let (connection, server_side) = local_pair();
        drop(connection);
        client._server.as_ref().unwrap().connections().send(server_side).unwrap();
    }
    after_leave_saves(&mut client);
    let saves = saved_generation(&dir) - before;
    assert!(saves <= autosaves(from, client.tick), "{saves} saves for connections that never joined");
    // Three who joined leave together: one save holds them all.
    let others: Vec<_> = ["a", "b", "c"].iter().map(|name| client.another(name)).collect();
    after_this_loop(&mut client);
    let (before, from) = (saved_generation(&dir), client.tick);
    drop(others);
    after_leave_saves(&mut client);
    let saves = saved_generation(&dir) - before;
    assert!(saves >= 1 && saves <= 1 + autosaves(from, client.tick), "{saves} saves for three players leaving at once");
    for name in ["a", "b", "c"] {
        assert!(dir.join("players").join(format!("{name}.ron")).is_file(), "{name} was saved as they left");
    }
    drop(client);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_leaver_back_under_their_name_in_another_case_is_who_they_were_and_one_name_plays_once() {
    let dir = world_dir("case");
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    let ore = client.content.blocks.expect("base:iron_ore");
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == ore).map(|s| s.count as u32).sum::<u32>();
    // Bob puts 54 ore in a chest and keeps 10.
    let mut bob = client.another("Bob");
    let chest = bob.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let at = format!("{} {} {}", chest.x, chest.y, chest.z);
    for line in ["/clear", &format!("/setblock {at} chest"), "/give iron_ore 54"] {
        bob.ok(line);
    }
    bob.pump_until(|c| held(c) == 54 && c.world.block(chest) == c.content.blocks.id("base:chest"));
    use_machine(&mut bob, chest);
    bob.pump_until(|c| held(c) == 0);
    bob.ok("/give iron_ore 10");
    bob.pump_until(|c| held(c) == 10);
    machine_says(&mut bob, &at, "holds 54 items", 10);
    // He leaves while his file can't be written, so he is kept as he left...
    let before = saved_generation(&dir);
    let blocked: Vec<_> = (before + 1..before + 40).map(|g| dir.join("players").join(format!("bob.ron.{g}.new"))).collect();
    for path in &blocked {
        std::fs::create_dir_all(path).unwrap();
    }
    drop(bob);
    after_leave_saves(&mut client);
    assert_eq!(saved_generation(&dir), before, "a save without his file counted");
    // ... and back as BOB, he is that Bob: 64 in all, not his old file's nor none.
    let mut bob = client.another("BOB");
    bob.ok("/seed");
    let said = bob.ok(&format!("/machine {at}"));
    assert_eq!(held(&bob) + items_held(&said), 64, "BOB holds {}, the chest: {said}", held(&bob));
    // While he plays, no one else joins as bob.
    let (connection, server_side) = local_pair();
    client._server.as_ref().unwrap().connections().send(server_side).unwrap();
    connection.send(ClientMessage::Hello { name: "bob".into(), protocol: PROTOCOL_VERSION, content: client.content.fingerprint() });
    let started = Instant::now();
    let reason = loop {
        assert!(started.elapsed() < SERVER_WAIT, "the second bob was neither let in nor turned away");
        match connection.try_recv() {
            Ok(Some(ServerMessage::Rejected { reason })) => break reason,
            Ok(Some(ServerMessage::Welcome { .. })) => panic!("a second bob joined while BOB plays"),
            Ok(_) => std::thread::sleep(Duration::from_millis(1)),
            Err(_) => panic!("the server dropped the second bob without a word"),
        }
    };
    assert!(format!("{reason:?}").contains("net.rejected.name_taken"), "{reason:?}");
    // Once a save holding him counts, he is his file, found as bob too.
    for path in &blocked {
        let _ = std::fs::remove_dir_all(path);
    }
    drop(bob);
    after_leave_saves(&mut client);
    assert!(saved_generation(&dir) > before, "the save after BOB left did not count");
    let mut bob = client.another("bob");
    bob.ok("/seed");
    let said = bob.ok(&format!("/machine {at}"));
    assert_eq!(held(&bob) + items_held(&said), 64, "bob holds {}, the chest: {said}", held(&bob));
    drop(bob);
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_save_cut_short_after_the_machines_neither_loses_nor_doubles_what_went_in() {
    let dir = world_dir("cut");
    let config = |cut_saves| ServerConfig { view_distance: 2, world: Some(dir.clone()), cut_saves, ..Default::default() };
    let mut client = Client::join_with(config(false));
    let chest = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let at = format!("{} {} {}", chest.x, chest.y, chest.z);
    for line in ["/clear", &format!("/setblock {at} chest"), "/give iron_ore 64"] {
        client.ok(line);
    }
    let ore = client.content.blocks.expect("base:iron_ore");
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == ore).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| held(c) == 64 && c.world.block(chest) == c.content.blocks.id("base:chest"));
    drop(client);
    // The ore goes into the chest, and every save after is cut short once the machines
    // are written: the chest holding it is saved, the player without it is not.
    let mut client = Client::join_with(config(true));
    client.pump_until(|c| held(c) == 64);
    use_machine(&mut client, chest);
    client.pump_until(|c| held(c) == 0);
    machine_says(&mut client, &at, "holds 64 items", 10);
    drop(client);
    // The last whole save: the ore in hand, the chest empty. Sixty-four in all.
    let mut client = Client::join_with(config(false));
    client.pump_until(|c| held(c) > 0);
    let said = client.ok(&format!("/machine {at}"));
    let in_chest = items_held(&said);
    assert_eq!(held(&client) + in_chest, 64, "the player holds {}, the chest: {said}", held(&client));
    drop(client);
    assert_eq!(staged_files(&dir), Vec::<std::path::PathBuf>::new(), "the cut save was forgotten");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Files a save staged and left, in the world and its players and columns.
fn staged_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    [dir.to_path_buf(), dir.join("players"), dir.join("columns")]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".new"))
        .collect()
}

#[test]
fn a_save_whose_machines_cannot_be_written_does_not_count_and_leaves_nothing_staged() {
    let dir = world_dir("failing");
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    after_this_loop(&mut client);
    let before = saved_generation(&dir);
    // The machines can't be written for the next saves: a directory stands where they go.
    let blocked: Vec<_> = (before + 1..before + 40).map(|g| dir.join(format!("machines.ron.{g}.new"))).collect();
    for path in &blocked {
        std::fs::create_dir_all(path).unwrap();
    }
    // A player leaving saves the world, players and all; none of it counts.
    let other = client.another("other");
    after_this_loop(&mut client);
    drop(other);
    after_leave_saves(&mut client);
    assert_eq!(saved_generation(&dir), before, "a save missing its machines counted");
    let staged: Vec<_> = staged_files(&dir).into_iter().filter(|p| !blocked.contains(p)).collect();
    assert!(staged.is_empty(), "the failed save left {staged:?}");
    // With room again, the next save counts.
    for path in &blocked {
        std::fs::remove_dir_all(path).unwrap();
    }
    let other = client.another("other");
    after_this_loop(&mut client);
    drop(other);
    after_leave_saves(&mut client);
    assert!(saved_generation(&dir) > before, "the save after counted");
    drop(client);
    assert_eq!(staged_files(&dir), Vec::<std::path::PathBuf>::new());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_player_who_leaves_while_their_file_cannot_be_written_is_kept_until_a_save_holds_them() {
    let dir = world_dir("kept");
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    let ore = client.content.blocks.expect("base:iron_ore");
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == ore).map(|s| s.count as u32).sum::<u32>();
    // Another player takes 64 ore and leaves: their file holds it.
    let mut other = client.another("other");
    let chest = other.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let at = format!("{} {} {}", chest.x, chest.y, chest.z);
    for line in ["/clear", &format!("/setblock {at} chest"), "/give iron_ore 64"] {
        other.ok(line);
    }
    other.pump_until(|c| held(c) == 64 && c.world.block(chest) == c.content.blocks.id("base:chest"));
    let before = saved_generation(&dir);
    drop(other);
    after_leave_saves(&mut client);
    assert!(saved_generation(&dir) > before, "their leaving was saved");
    // Back, they put it all in the chest.
    let mut other = client.another("other");
    other.pump_until(|c| held(c) == 64);
    use_machine(&mut other, chest);
    other.pump_until(|c| held(c) == 0);
    machine_says(&mut other, &at, "holds 64 items", 10);
    // They leave while their file can't be written: that save does not count.
    let before = saved_generation(&dir);
    let blocked: Vec<_> = (before + 1..before + 40).map(|g| dir.join("players").join(format!("other.ron.{g}.new"))).collect();
    for path in &blocked {
        std::fs::create_dir_all(path).unwrap();
    }
    drop(other);
    after_leave_saves(&mut client);
    assert_eq!(saved_generation(&dir), before, "a save without their file counted");
    // With room again, the next save holds the chest and them as they left, together.
    for path in &blocked {
        std::fs::remove_dir_all(path).unwrap();
    }
    let third = client.another("third");
    after_this_loop(&mut client);
    drop(third);
    after_leave_saves(&mut client);
    assert!(saved_generation(&dir) > before, "the save after counted");
    let mut other = client.another("other");
    other.ok("/seed");
    let said = other.ok(&format!("/machine {at}"));
    let in_chest = items_held(&said);
    assert_eq!(held(&other) + in_chest, 64, "they hold {}, the chest: {said}", held(&other));
    drop(other);
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_multiblock_counts_as_built_for_whoever_placed_its_core_not_whoever_uses_it_first() {
    let dir = world_dir("builder");
    let mut owner = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    let core = owner.position.floor().as_ivec3() + IVec3::new(4, 0, 0);
    for line in ["/clear", "/fill ~1 ~ ~-1 ~6 ~2 ~1 air", "/fill ~1 ~-1 ~-1 ~6 ~-1 ~1 stone", "/give bloomery 1"] {
        owner.ok(line);
    }
    let bloomery = owner.content.blocks.expect("base:bloomery");
    owner.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == bloomery) && c.world.block(core - IVec3::Y) == c.content.blocks.id("base:stone"));
    // The core placed by hand on open ground, then the shaft built round it.
    owner.connection.send(ClientMessage::Input(PlayerInput { position: owner.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    owner.propose("core:place", ActionTarget::Block { block: core - IVec3::Y, face: Face::PosY });
    owner.pump_until(|c| c.world.block(core) == Some(bloomery));
    for line in ["/fill ~3 ~ ~-1 ~5 ~2 ~-1 kiln_bricks", "/fill ~3 ~ ~1 ~5 ~2 ~1 kiln_bricks", "/fill ~3 ~ ~ ~3 ~2 ~ kiln_bricks", "/fill ~5 ~1 ~ ~5 ~2 ~ kiln_bricks", "/setblock ~5 ~ ~ bellows"] {
        owner.ok(line);
    }
    owner.pump_until(|c| c.world.block(core + IVec3::X) == c.content.blocks.id("base:bellows"));
    // Another player uses it first: the build is the owner's.
    let mut other = owner.another("other");
    other.pump_until(|c| c.world.block(core) == Some(bloomery));
    // Empty-handed (its starting kit's first slot holds what a bloomery does not take).
    let empty = other.inventory.slots.iter().take(9).position(Option::is_none).expect("an empty hotbar slot") as u8;
    other.connection.send(ClientMessage::Input(PlayerInput { position: other.position, yaw: 0.0, pitch: 0.0, selected_slot: empty, flying: false }));
    other.propose("base:use_machine", ActionTarget::Block { block: core, face: Face::NegX });
    other.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Bloomery")));
    drop(other);
    drop(owner);
    let read = |name: &str| std::fs::read_to_string(dir.join("players").join(format!("{name}.ron"))).unwrap();
    assert!(read("test").contains("built:base:bellows_bloomery"), "the owner built it: {}", read("test"));
    assert!(!read("other").contains("built:"), "the first to use it did not: {}", read("other"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn stacks_a_machine_has_no_slot_for_after_a_change_fall_out_where_it_stands() {
    let dir = world_dir("spill");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let chest = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let at = format!("{} {} {}", chest.x, chest.y, chest.z);
    client.ok(&format!("/setblock {at} chest"));
    client.pump_until(|c| c.world.block(chest) == c.content.blocks.id("base:chest"));
    drop(client);
    // Thirty stacks saved in a chest of twenty-seven slots, as when a package gives it fewer.
    let stacks = vec![r#"("base:cobblestone",64)"#; 30].join(",");
    let saved = format!(r#"[(position:({},{},{}),block:"base:chest",inputs:[],outputs:[],fuel:None,burning:0.0,energy:0.0,condition:0.0,made:0,bin:[{stacks}])]"#, chest.x, chest.y, chest.z);
    std::fs::write(dir.join("machines.ron"), saved).unwrap();
    let mut client = Client::join_with(config());
    machine_says(&mut client, &at, &format!("holds {} items", 27 * 64), 10);
    let cobblestone = client.content.blocks.expect("base:cobblestone");
    client.pump_until(|c| c.items.values().filter(|i| i.stack.item == cobblestone).map(|i| i.stack.count as u32).sum::<u32>() == 3 * 64);
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_broken_chest_drops_what_it_held_and_a_chute_in_hand_is_not_put_in() {
    let mut client = Client::join();
    let chest = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    for line in ["/clear", "/setblock ~2 ~ ~ chest", "/give copper_ore 7"] {
        client.ok(line);
    }
    let (ore, chute) = (client.content.blocks.expect("base:copper_ore"), client.content.blocks.expect("base:chute"));
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == ore && s.count == 7) && c.world.block(chest) == c.content.blocks.id("base:chest"));
    use_machine(&mut client, chest);
    machine_says(&mut client, "~2 ~ ~", "holds 7 items", 10);
    // A machine in hand is laid against a bin, not put in it.
    client.ok("/give chute 1");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == chute));
    use_machine(&mut client, chest);
    client.ok("/seed");
    machine_says(&mut client, "~2 ~ ~", "holds 7 items", 5);
    assert!(client.inventory.slots.iter().flatten().any(|s| s.item == chute), "the chute stays in hand");
    // Broken, it drops all it held.
    client.ok("/setblock ~2 ~ ~ air");
    client.pump_until(|c| c.items.values().filter(|i| i.stack.item == ore).map(|i| i.stack.count as u32).sum::<u32>() == 7);
}

#[test]
fn a_water_wheel_turns_on_falling_water_but_not_on_water_poured_out_on_flat_ground() {
    let mut client = Client::join();
    for line in ["/clear", "/fill ~2 ~ ~-2 ~6 ~3 ~2 air", "/fill ~2 ~-1 ~-2 ~6 ~-1 ~2 stone", "/setblock ~4 ~ ~ water_wheel", "/setblock ~5 ~ ~ shaft"] {
        client.ok(line);
    }
    // Poured beside it, water spreads but does not run: the wheel stays still.
    client.ok("/setblock ~3 ~ ~ water");
    machine_says(&mut client, "~4 ~ ~", "a fall of water", 10);
    wait_server(&mut client, 2);
    machine_says(&mut client, "~4 ~ ~", "a fall of water", 5);
    // Falling past it, from a spring above, it turns.
    for line in ["/fill ~2 ~ ~-2 ~3 ~2 ~2 air", "/setblock ~3 ~3 ~ water"] {
        client.ok(line);
    }
    let started = client.tick;
    while client.ok("/machine ~4 ~ ~").contains("a fall of water") {
        assert!(server_seconds(&client, started) < 15, "falling water never turned the wheel");
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
fn water_wheels_within_four_blocks_of_another_share_its_water_and_both_turn_once_it_moves_away() {
    let mut client = Client::join();
    let start = client.position.floor().as_ivec3();
    for line in ["/clear", "/fill ~2 ~ ~-2 ~10 ~4 ~2 air", "/fill ~2 ~-1 ~-2 ~10 ~-1 ~2 stone", "/setblock ~3 ~4 ~ water", "/setblock ~4 ~1 ~ water_wheel"] {
        client.ok(line);
    }
    let at = |dx: i32, dz: i32| format!("{} {} {}", start.x + dx, start.y + 1, start.z + dz);
    // Alone beside the fall it turns: idle, with nothing to drive.
    machine_says(&mut client, &at(4, 0), "idle", 20);
    // A second wheel on the same fall shares it, and neither turns.
    client.ok("/setblock ~2 ~1 ~ water_wheel");
    machine_says(&mut client, &at(4, 0), "a fall of water", 20);
    machine_says(&mut client, &at(2, 0), "a fall of water", 20);
    client.ok("/setblock ~2 ~1 ~ air");
    machine_says(&mut client, &at(4, 0), "idle", 20);
    // On a fall of its own four blocks off (the data's `water_spacing`, diagonals too),
    // it stills the first all the same.
    client.ok("/setblock ~8 ~4 ~ water");
    client.ok("/setblock ~8 ~1 ~1 water_wheel");
    machine_says(&mut client, &at(4, 0), "a fall of water", 20);
    machine_says(&mut client, &at(8, 1), "a fall of water", 20);
    // Five blocks off, both turn.
    client.ok("/setblock ~8 ~1 ~1 air");
    client.ok("/setblock ~9 ~1 ~ water_wheel");
    machine_says(&mut client, &at(4, 0), "idle", 20);
    machine_says(&mut client, &at(9, 0), "idle", 20);
}

#[test]
fn machines_keep_working_while_their_land_is_unloaded_but_no_animal_walks_a_wheel_there() {
    // A world folder, so edited land is saved and may unload (unsaved land never does).
    let dir = world_dir("unloaded");
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    let start = client.position.floor().as_ivec3();
    let (boiler, crusher) = (start + IVec3::new(3, 0, 0), start + IVec3::new(4, 0, 0));
    for line in [
        "/clear",
        "/fill ~3 ~ ~-1 ~4 ~1 ~1 air",
        "/setblock ~3 ~ ~ boiler",
        "/setblock ~4 ~ ~ steam_crusher",
        "/fill ~2 ~ ~3 ~4 ~1 ~5 air",
        "/setblock ~3 ~ ~4 treadwheel",
        "/summon aurochs 1 ~3 ~ ~5",
        "/creatures tame aurochs",
        "/creatures hold aurochs",
        "/give coal_ore 2",
        "/fill ~2 ~3 ~-6 ~4 ~5 ~-4 air",
        "/setblock ~3 ~4 ~-5 windmill",
        "/wind calm 600",
    ] {
        client.ok(line);
    }
    let blocks = &client.content.blocks;
    let (coal, ore, dust) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ore"), blocks.expect("base:copper_dust"));
    // In calm air the windmill does not turn.
    let windmill = format!("{} {} {}", start.x + 3, start.y + 4, start.z - 5);
    machine_says(&mut client, &windmill, "no wind", 20);
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| count(c, coal) == 2 && c.world.block(crusher) == c.content.blocks.id("base:steam_crusher"));
    let wheel = format!("{} {} {}", start.x + 3, start.y, start.z + 4);
    let started = client.tick;
    while client.ok(&format!("/machine {wheel}")).contains("needs a kept animal") {
        assert!(server_seconds(&client, started) < 10, "the aurochs never walked the wheel");
        std::thread::sleep(Duration::from_millis(300));
    }
    use_machine(&mut client, boiler);
    client.pump_until(|c| count(c, coal) == 0);
    client.ok("/give copper_ore 3");
    client.pump_until(|c| count(c, ore) == 3);
    use_machine(&mut client, crusher);
    client.pump_until(|c| count(c, ore) == 0);
    // Far away, the land unloads once saved: the crusher works the ore all the same, and
    // the wheel, its aurochs not about, stops.
    let at = format!("{} {} {}", crusher.x, crusher.y, crusher.z);
    client.ok(&format!("/tp {} {}", start.x + 3000, start.z));
    machine_says(&mut client, &wheel, "needs a kept animal", 60);
    machine_says(&mut client, &at, "idle", 60);
    // Unloaded, the windmill is reckoned to catch an average wind (`wind_mean`), not the
    // calm it was left in.
    machine_says(&mut client, &windmill, "idle", 20);
    client.ok(&format!("/tp {} {} {}", start.x, start.y, start.z));
    client.pump_until(|c| c.world.block(crusher) == c.content.blocks.id("base:steam_crusher"));
    use_machine(&mut client, crusher);
    client.pump_until(|c| count(c, dust) == 6);
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_windmill_within_four_blocks_of_another_is_becalmed_and_both_turn_once_it_moves_away() {
    let mut client = Client::join();
    let start = client.position.floor().as_ivec3();
    for line in ["/clear", "/fill ~3 ~3 ~-2 ~12 ~6 ~5 air", "/wind gale w 30", "/setblock ~3 ~4 ~ windmill"] {
        client.ok(line);
    }
    let at = |dx: i32| format!("{} {} {}", start.x + dx, start.y + 4, start.z);
    // Alone in a gale it has wind: idle, with nothing to turn (becalmed, it says "no wind").
    machine_says(&mut client, &at(3), "idle", 20);
    // Sails four blocks off (the data's `wind_spacing`) steal its wind, and it theirs.
    client.ok("/setblock ~7 ~4 ~ windmill");
    machine_says(&mut client, &at(3), "no wind", 20);
    machine_says(&mut client, &at(7), "no wind", 20);
    // Four off on both ways (diagonals count), the same.
    client.ok("/setblock ~7 ~4 ~ air");
    client.ok("/setblock ~7 ~4 ~4 windmill");
    machine_says(&mut client, &at(3), "no wind", 20);
    // Five blocks off, both turn.
    client.ok("/setblock ~7 ~4 ~4 air");
    client.ok("/setblock ~8 ~4 ~ windmill");
    machine_says(&mut client, &at(3), "idle", 20);
    machine_says(&mut client, &at(8), "idle", 20);
}

#[test]
fn a_treadwheel_a_player_placed_is_walked_only_by_their_own_animals() {
    let config = ServerConfig { view_distance: 2, admins: vec!["other".into()], ..Default::default() };
    let mut owner = Client::join_with(config);
    let mut other = owner.another("other");
    let wheel = owner.position.floor().as_ivec3() + IVec3::new(3, 0, 0);
    let at = format!("{} {} {}", wheel.x, wheel.y, wheel.z);
    for line in ["/clear", "/fill ~2 ~ ~-1 ~4 ~1 ~3 air", "/fill ~2 ~-1 ~-1 ~4 ~-1 ~3 stone", "/give treadwheel 1"] {
        owner.ok(line);
    }
    let treadwheel = owner.content.blocks.expect("base:treadwheel");
    owner.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == treadwheel) && c.world.block(wheel - IVec3::Y) == c.content.blocks.id("base:stone"));
    owner.connection.send(ClientMessage::Input(PlayerInput { position: owner.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    owner.propose("core:place", ActionTarget::Block { block: wheel - IVec3::Y, face: Face::PosY });
    owner.pump_until(|c| c.world.block(wheel) == Some(treadwheel));
    // Another player's aurochs beside it does not walk it.
    for line in ["/summon aurochs 1 ~3 ~ ~2", "/creatures tame aurochs", "/creatures hold aurochs"] {
        other.ok(line);
    }
    machine_says(&mut owner, &at, "needs a kept animal", 10);
    wait_server(&mut owner, 3);
    machine_says(&mut owner, &at, "needs a kept animal", 1);
    // Their own wheel in its place, it does.
    owner.ok(&format!("/setblock {at} air"));
    for line in ["/clear", "/give treadwheel 1"] {
        other.ok(line);
    }
    other.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == treadwheel) && c.world.block(wheel).is_some_and(|b| b.is_air()));
    other.connection.send(ClientMessage::Input(PlayerInput { position: other.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    other.propose("core:place", ActionTarget::Block { block: wheel - IVec3::Y, face: Face::PosY });
    other.pump_until(|c| c.world.block(wheel) == Some(treadwheel));
    let started = owner.tick;
    while owner.ok(&format!("/machine {at}")).contains("needs a kept animal") {
        assert!(server_seconds(&owner, started) < 10, "the other player's aurochs never walked their own wheel");
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
fn a_full_hopper_leaves_the_stack_in_hand_and_gives_nothing_back() {
    let mut client = Client::join();
    let hopper = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    for line in ["/clear", "/setblock ~2 ~ ~ hopper"] {
        client.ok(line);
    }
    let ore = client.content.blocks.expect("base:copper_ore");
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == ore).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(hopper) == c.content.blocks.id("base:hopper"));
    // Five stacks fill its five slots.
    for filled in 1..=5 {
        client.ok("/give copper_ore 64");
        client.pump_until(|c| held(c) == 64);
        use_machine(&mut client, hopper);
        client.pump_until(|c| held(c) == 0);
        machine_says(&mut client, "~2 ~ ~", &format!("holds {} items", filled * 64), 5);
    }
    // A sixth stays in hand, and the hopper keeps what it holds.
    client.ok("/give copper_ore 64");
    client.pump_until(|c| held(c) == 64);
    use_machine(&mut client, hopper);
    client.ok("/seed");
    machine_says(&mut client, "~2 ~ ~", "holds 320 items", 5);
    assert_eq!(held(&client), 64, "the full hopper emptied into the inventory");
}

#[test]
fn a_treadwheel_stops_once_its_animal_goes_two_days_unfed_and_turns_again_when_it_is_fed() {
    let mut client = Client::join();
    let aurochs = client.content.creatures.find("aurochs").expect("aurochs in the content");
    for line in ["/clear", "/fill ~2 ~ ~-1 ~4 ~1 ~3 air", "/setblock ~3 ~ ~ treadwheel", "/summon aurochs 1 ~3 ~ ~2", "/creatures tame aurochs", "/creatures hold aurochs"] {
        client.ok(line);
    }
    let start = client.position.floor().as_ivec3();
    let wheel = format!("{} {} {}", start.x + 3, start.y, start.z);
    let turns = |client: &mut Client| !client.ok(&format!("/machine {wheel}")).contains("needs a kept animal");
    let started = client.tick;
    while !turns(&mut client) {
        assert!(server_seconds(&client, started) < 10, "the tamed aurochs never walked the wheel");
        std::thread::sleep(Duration::from_millis(300));
    }
    // Three days on, unfed, it will not walk.
    client.ok("/time add 3d");
    let said = machine_says(&mut client, &wheel, "fed in the last 2 days", 10);
    assert!(said.contains("needs a kept animal"), "{said}");
    // Fed its wheat, it walks again.
    client.ok("/give wheat 1");
    let wheat = client.content.blocks.expect("base:wheat");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == wheat) && c.creatures.values().any(|v| v.species == aurochs));
    let (&id, view) = client.creatures.iter().find(|(_, v)| v.species == aurochs).unwrap();
    let at = view.position;
    client.ok(&format!("/tp {} {} {}", at.x + 0.8, at.y, at.z));
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:tend", ActionTarget::Creature(id));
    client.pump_until(|c| c.inventory.get(0).is_none());
    let started = client.tick;
    while !turns(&mut client) {
        assert!(server_seconds(&client, started) < 10, "fed, the aurochs still does not walk the wheel");
        std::thread::sleep(Duration::from_millis(300));
    }
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
    let wait = Instant::now() + Duration::from_millis(1500);
    client.pump_until(|_| Instant::now() > wait);
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
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    let earned = client.earned.len();
    for _ in 0..3 {
        client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout));
        client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
        client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    }
    let wait = Instant::now() + Duration::from_millis(800);
    client.pump_until(|_| Instant::now() > wait);
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
    client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(moonbloom));
    let earned = client.earned.len();
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(BlockId::AIR));
    // Its plain drop: the flower, and nothing foraged or taught.
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == moonbloom) || c.items.values().any(|i| i.stack.item == moonbloom));
    let wait = Instant::now() + Duration::from_millis(800);
    client.pump_until(|_| Instant::now() > wait);
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
/// proves), so both stand their players on the same spot. The time is only a backstop: a
/// busy machine stretches it.
#[test]
fn a_second_server_on_the_same_world_answers_a_join() {
    let first = Client::join();
    let started = Instant::now();
    let second = Client::join();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "joining a second server on the same world took {took:?}");
    assert_eq!(first.position, second.position);
}
