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
    blows: u32,
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
            content,
            inventory: Default::default(),
            weather: None,
            quick: false,
            chat: Vec::new(),
            day: None,
            creatures: Default::default(),
            deaths: 0,
            blows: 0,
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
            ServerMessage::PlayerState { health, inventory, skills, .. } => {
                self.health = health.current;
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
            ServerMessage::Struck { .. } => self.blows += 1,
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
    for line in ["/fill ~ ~-1 ~2 ~1 ~-1 ~2 dirt", "/fill ~ ~ ~2 ~1 ~1 ~2 air", "/give wheat_seeds 5", "/give saffron_seeds 5"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(ground).is_some_and(|b| b != BlockId::AIR) && c.inventory.get(5).is_some());
    let blocks = &client.content.blocks;
    let (sprout, ripe, wheat) = (blocks.expect("base:wheat_crop_0"), blocks.expect("base:wheat_crop"), blocks.expect("base:wheat"));
    assert_eq!(client.inventory.get(4).map(|s| s.item), blocks.id("base:wheat_seeds"));
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };

    // Saffron takes farming 60: refused, nothing sown, no experience.
    hold(&client, 5);
    client.propose("base:plant", ActionTarget::Block { block: other, face: Face::PosY });
    // Wheat takes level 1: sown, with its planting experience (2, in tenths).
    hold(&client, 4);
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(sprout) && c.earned == [20]);
    assert_eq!(client.world.block(other + IVec3::Y), Some(BlockId::AIR), "sown without the level");
    client.pump_until(|c| c.inventory.get(4).map(|s| s.count) == Some(4));

    // Grown ripe, then broken: its harvest pops out and its experience is earned.
    assert_eq!(client.ok("/grow ripe"), "Grew 1 crop.");
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(ripe));
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.earned == [20, 90] && c.items.values().any(|i| i.stack.item == wheat));
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
    client.connection.send(ClientMessage::Finished);
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
    assert_eq!((day % TICKS_PER_DAY, speed), (6000, 1.0));
    assert!(day >= before && day < before + TICKS_PER_DAY);
    client.ok("/night");
    assert_eq!(client.day.unwrap().0 % TICKS_PER_DAY, 13_000);
    client.ok("/time add 2h");
    // Two hours on, give or take the ticks that ran in between.
    assert!((client.day.unwrap().0 % TICKS_PER_DAY).abs_diff(15_000) < 40, "{:?}", client.day);
    assert!(client.ok("/time freeze").contains("frozen"));
    assert_eq!(client.day.unwrap().1, 0.0);
    assert!(client.ok("/time speed 10").contains("2.0 minutes per day"));
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
fn players_are_told_the_weather() {
    let mut client = Client::join();
    client.pump_until(|c| c.weather.is_some());
    let weather = client.weather.unwrap();
    let fractions = [weather.cover, weather.convection, weather.cirrus, weather.humidity, weather.precipitation, weather.snow];
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)), "{weather:?}");
    assert!(weather.wind.is_finite());
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
    guest._server.console("/time set midnight");
    guest.pump_until(|c| c.day.is_some_and(|(day, _)| day % TICKS_PER_DAY == 18_000));
    guest._server.console("say hello");
    guest.pump_until(|c| c.chat.iter().any(|(_, text)| text == "[Server] hello"));
    guest._server.console("gamemode creative guest");
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
