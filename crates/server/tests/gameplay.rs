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
    /// The last teleport applied (the server's count of them), which inputs echo.
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
    /// Refusals received, in English.
    refused: Vec<String>,
    /// The time of day and how fast it runs, as last told.
    day: Option<(u64, f32)>,
    /// The server's tick, as it last told (once a second): the clock waits are judged by.
    server_tick: u64,
    /// Creatures in sight, how many were seen to die, and our blows the server answered.
    creatures: std::collections::HashMap<u32, voxmar_protocol::CreatureView>,
    deaths: u32,
    /// Times we were told we died.
    died: u32,
    blows: u32,
    /// The damage each of our blows did (0 for a miss), what kind of blow each was, and our
    /// rhythm pips after it.
    damage: Vec<u16>,
    kinds: Vec<voxmar_engine::combat::Blow>,
    pips: Vec<u8>,
    /// Creatures' blows at us, what came of each, and when we heard.
    hurt: Vec<(u16, voxmar_engine::combat::Defended)>,
    hurt_at: Vec<Instant>,
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
            refused: Vec::new(),
            day: None,
            server_tick: 0,
            creatures: Default::default(),
            deaths: 0,
            died: 0,
            blows: 0,
            damage: Vec::new(),
            kinds: Vec::new(),
            pips: Vec::new(),
            hurt: Vec::new(),
            hurt_at: Vec::new(),
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
    ///
    /// Time this thread was off its cores is not the server's silence: on a busy machine
    /// (background QoS, load 50 and more) the whole test process can be held off for many
    /// seconds, and the check then ran before the messages waiting for it were read, failing
    /// random tests with "20 s since its last message". A pause between two looks at the
    /// connection longer than `AWAY` moves both limits on by it.
    fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        const AWAY: Duration = Duration::from_millis(500);
        let mut start = Instant::now();
        let mut heard = start;
        let mut looked = start;
        while !done(self) {
            let now = Instant::now();
            let away = now - looked;
            looked = now;
            if away > AWAY {
                eprintln!("pump_until: this thread was away {:.1}s", away.as_secs_f32());
                start += away;
                heard += away;
                continue;
            }
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
            ServerMessage::Refused { why } => self.refused.push(self.content.english.render(&why, &self.content.blocks)),
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
            ServerMessage::Died => self.died += 1,
            ServerMessage::Struck { damage, blow, pips, .. } => {
                self.blows += 1;
                self.damage.push(damage);
                self.kinds.push(blow);
                self.pips.push(pips);
            }
            ServerMessage::Hurt { damage, defended, .. } => {
                self.hurt.push((damage, defended));
                self.hurt_at.push(Instant::now());
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
    fn after_server_ticks(&mut self, ticks: u64) {
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

    /// Waits until the server's clock has run at least `seconds` on (it tells the time every
    /// second, so a little more).
    fn wait_server(&mut self, seconds: u64) {
        self.after_ticks(seconds * TICKS_PER_SECOND as u64);
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
/// test gives up on it. Silence as the gate allows it (`gate::SERVER_SILENT`): at load 60 a
/// whole machine starved the server thread for 20 s and more while this one ran, so a
/// shorter limit failed landings on a server that was only waiting for a core. A real hang
/// still fails, a little later.
const SERVER_SILENT: Duration = Duration::from_secs(60);
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
    client.after_server_ticks((hardness.as_secs_f32() * TICKS_PER_SECOND as f32).ceil() as u64 + 2);
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
    client.after_server_ticks(2);
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
        client.after_server_ticks(1);
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
    client.after_server_ticks(2);
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
    client.after_server_ticks(40);
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

/// Plants that merged into one item since a world was saved come back as that item from a
/// chest's contents and from items lying on the ground, as from the player's pockets.
#[test]
fn what_chests_and_the_ground_held_of_merged_plants_comes_back_merged() {
    use voxmar_server::save::{SavedItem, WorldSave};
    let dir = world_dir("merged-saved");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let (feet, spot) = (client.position, client.position.floor().as_ivec3() + IVec3::new(2, 0, 0));
    for line in ["/clear", "/fill ~2 ~-1 ~ ~2 ~-1 ~ stone", "/fill ~2 ~ ~ ~2 ~1 ~ air", "/setblock ~2 ~ ~ chest"] {
        client.ok(line);
    }
    let chest = client.content.blocks.expect("base:chest");
    client.pump_until(|c| c.world.block(spot) == Some(chest));
    drop(client);

    // The world as an older version left it: oregano in the chest, parsley on the ground.
    let save = WorldSave::new(&dir);
    let mut machines = save.read_machines().unwrap();
    let held = machines.iter_mut().find(|m| m.position == [spot.x, spot.y, spot.z]).expect("the chest was saved");
    held.bin = vec![("base:oregano".into(), 3)];
    save.write_machines(&machines).unwrap();
    save.write_items(&[SavedItem { item: "base:parsley".into(), count: 2, position: [feet.x + 4.5, feet.y + 1.0, feet.z], age: 0.0, owner: None, lifetime: None }]).unwrap();

    let mut client = Client::join_with(config());
    client.pump_until(|c| c.world.block(spot) == Some(chest));
    client.propose("core:mine", ActionTarget::Block { block: spot, face: Face::PosY });
    client.pump_until(|c| c.world.block(spot) == Some(BlockId::AIR));
    let herbs = client.content.blocks.expect("base:kitchen_herbs");
    let total = |c: &Client| {
        c.inventory.slots.iter().flatten().filter(|s| s.item == herbs).map(|s| s.count as usize).sum::<usize>()
            + c.items.values().filter(|i| i.stack.item == herbs).map(|i| i.stack.count as usize).sum::<usize>()
    };
    // The chest's three and the ground's two (the chest itself is not herbs).
    client.pump_until(|c| total(c) == 5);
    let oregano = client.content.blocks.expect("base:oregano");
    let parsley = client.content.blocks.expect("base:parsley");
    assert!(!client.items.values().any(|i| i.stack.item == oregano || i.stack.item == parsley), "an older item came back as itself");
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
    // A step a tick at most is seen (wear looks once a tick), so each waits for the tick
    // after it; stamped 110 ms apart, the walking pace the moves are judged by.
    let start = Instant::now();
    for i in 0..80 {
        client.send_position_at(if i % 2 == 0 { there } else { here }, false, start + Duration::from_millis(110) * i);
        client.after_server_ticks(0);
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
        // (Asked again after 8 server ticks, 400 ms at speed: a busy server is waited for.)
        let asked = client.ticks();
        client.pump_until(|c| c.blows > blows || !c.creatures.contains_key(&id) || c.ticks() >= asked + 8);
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
    // Waited for in real time: the server judges swings and creatures' blows by when the
    // messages arrive (`fight`), so stamping the clicks ahead of the wall would put them
    // in the future of the blows they answer.
    let strike = |client: &mut Client, landed: usize| {
        let start = client.damage.len();
        while client.damage[start..].iter().filter(|&&d| d > 0).count() < landed && client.creatures.contains_key(&id) {
            assert!(client.damage.len() - start < 80, "{} blows landed {landed}", client.damage.len() - start);
            let at = client.creatures[&id].position;
            client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
            client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot as u8, flying: false }));
            std::thread::sleep(Duration::from_millis(650));
            let blows = client.blows;
            client.propose("core:attack", ActionTarget::Creature(id));
            let asked = client.ticks();
            client.pump_until(|c| c.blows > blows || !c.creatures.contains_key(&id) || c.ticks() >= asked + 8);
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
    // Kept, newly tamed and just fed, so its condition shows (0.6, good).
    client.ok("/creatures tame");
    // At Farming 1: what anyone sees, its condition in a word, and that more comes with level.
    let first = look(&mut client);
    assert!(first.contains("health") && first.contains("Farming 10 shows more"), "{first}");
    assert!(!first.contains("Shows:"), "{first}");
    assert!(first.contains("In good condition") && !first.contains("(0.6"), "{first}");
    // At 40, everything: what shows, what it carries, the numbers and the odds.
    client.ok("/skill farming 40");
    let all = look(&mut client);
    for words in ["Shows:", "Carries", "Exactly:", "mutates", "condition (0.60)"] {
        assert!(all.contains(words), "{words} missing from {all}");
    }
    // A day and a half unfed, it says what it wants.
    assert!(!all.contains("It wants food"), "{all}");
    client.ok("/time add 1.5d");
    let hungry = look(&mut client);
    assert!(hungry.contains("It wants") && hungry.contains("food"), "{hungry}");
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
    client.clear();
    client.ok("/give stone_pickaxe 1");
    client.pump_until(|c| c.world.block(ore) == c.content.blocks.id("base:iron_ore") && c.inventory.get(0).is_some_and(|s| s.item == c.content.blocks.expect("base:stone_pickaxe")));
    let iron = client.world.block(ore).unwrap();
    let mine = |client: &Client| client.propose("core:mine", ActionTarget::Block { block: ore, face: Face::PosY });
    let hold = |client: &Client, slot: u8| {
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
    };
    // How long a break takes, in server ticks, with the pickaxe and by hand: a refusal
    // waits that long and more before it says nothing broke.
    let hardness = client.content.blocks.info(iron).hardness;
    let pickaxe = client.content.blocks.expect("base:stone_pickaxe");
    let ticks = |speed: f32| (hardness / speed * TICKS_PER_SECOND as f32).ceil() as u64;
    let (with_pickaxe, by_hand) = (ticks(client.content.tools.speed(Some(pickaxe), iron)), ticks(client.content.tools.speed(None, iron)));
    // At Mining 1, even with the pickaxe: refused.
    hold(&client, 0);
    mine(&client);
    client.settle();
    client.after_ticks(with_pickaxe + 20);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke at Mining 1");
    // At Mining 15 with a bare hand: refused too.
    client.ok("/skill mining 15");
    hold(&client, 1);
    mine(&client);
    client.settle();
    client.after_ticks(by_hand + 20);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke without a pickaxe");
    // At Mining 15 with the stone pickaxe: broken, quicker than by hand (by the server's
    // clock, told once a second), teaching Mining (35).
    hold(&client, 0);
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
    owner.after_server_ticks(3 * TICKS_PER_SECOND as u64);
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
    client.after_server_ticks(30);
    assert_eq!(client.world.block(far), Some(BlockId::AIR), "a flower came back on ground that was replaced");
}

#[test]
fn picking_any_wild_herb_gives_kitchen_herbs_and_the_herb_regrows() {
    let mut client = Client::join();
    let at = client.position.floor().as_ivec3() + IVec3::new(1, 0, 0);
    for line in ["/clear", "/fill ~1 ~-1 ~ ~2 ~-1 ~ grass_block", "/fill ~1 ~ ~ ~2 ~ ~ air", "/setblock ~1 ~ ~ oregano"] {
        client.ok(line);
    }
    let (oregano, herbs) = (client.content.blocks.expect("base:oregano"), client.content.blocks.expect("base:kitchen_herbs"));
    client.pump_until(|c| c.world.block(at) == Some(oregano));
    client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
    client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
    // One item for every wild herb: the oregano itself is not what is handed out.
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == herbs && s.count == 1));
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == oregano), "picked oregano is still an item of its own");
    // The wild spot grows the same plant back.
    client.ok("/grow");
    client.pump_until(|c| c.world.block(at) == Some(oregano));
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
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    assert!(!done(&client, "zz_test:sticks"), "two sticks finished it");
    assert_eq!(carried(&client, torch), 0);
    client.chat.clear();
    client.ok("/give stick 1");
    client.pump_until(|c| done(c, "zz_test:sticks") && carried(c, torch) == 2);
    assert!(client.chat.iter().any(|(_, line)| line.contains("Three Sticks")), "{:?}", client.chat);
    // Without the sticks, two checks later: still done, and the torches came once.
    client.ok("/clear stick");
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
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
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    assert!(!done(&client, "zz_test:miner"), "level 9 finished a level 10 quest");
    client.earned.clear();
    client.ok("/skill mining 10");
    client.pump_until(|c| done(c, "zz_test:miner") && c.earned.contains(&500));
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
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
    // Wait for the ore itself: the starting kit fills slot 0 before /clear and /give arrive.
    client.pump_until(|c| c.world.block(core) == Some(crucible) && count(c, ore) == 2);
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
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    let started = client.server_tick;
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
        client.after_server_ticks(2);
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
    client.after_server_ticks(2);
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
    // What they left lies in the same save as they do: both are written in one generation.
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
    for file in ["items.ron", "crops.ron", "creatures.ron", "machines.ron"] {
        std::fs::write(dir.join(file), "[(oops").unwrap();
    }
    let client = Client::join_with(config());
    drop(client);
    assert_eq!(set_aside(&dir.join("columns")).len(), 1, "the bad column is set aside");
    let aside = set_aside(&dir);
    for file in ["items.ron", "crops.ron", "creatures.ron", "machines.ron"] {
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
        self.after_server_ticks(2);
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
    client.after_server_ticks(2);
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
    // The server judges the time aloft by when each input arrives, so every input is sent
    // with its own time, 50 ms apart (a tick), as `walking_wears_a_path` does; the clock
    // of the test's own ticks does not decide when they count.
    let t0 = Instant::now();
    let stamp = |i: u32| t0 + Duration::from_millis(50) * i;
    // Hovering a second and a half, a tick at a time...
    for i in 0..30 {
        client.send_position_at(over, false, stamp(i));
        client.after_server_ticks(1);
    }
    assert!(client.teleported.is_none(), "let down before the grace ran out");
    // ...silent for two and a half (still alive to the server)...
    client.after_server_ticks(50);
    // ...and hovering on: the time aloft before the silence still counts, so they are let
    // down well before a fresh grace and sink would allow (three seconds on, at most).
    let mut i = 30 + 50;
    while !client.teleported.is_some_and(|to| to.y < over.y) {
        assert!(i < 30 + 50 + 3 * TICKS_PER_SECOND, "hovered on after going silent");
        client.send_position_at(over, false, stamp(i));
        client.after_server_ticks(1);
        i += 1;
    }
}

#[test]
fn a_player_silent_in_mid_jump_lands_without_being_pulled_back() {
    let mut client = on_a_test_floor();
    let start = client.position;
    // Up in a jump, then nothing for four seconds (a hitch, a dropped connection), then
    // the rest of the jump, down on the floor and walking on.
    client.send_position(start + Vec3::new(0.2, 1.0, 0.0), false);
    client.after_server_ticks(4 * TICKS_PER_SECOND as u64);
    client.send_position(start + Vec3::new(0.3, 0.8, 0.0), false);
    client.send_position(start + Vec3::new(0.4, 0.0, 0.0), false);
    for step in 1..=10 {
        client.send_position(start + Vec3::new(0.4 + step as f32 * 0.2, 0.0, 0.0), false);
        client.after_server_ticks(1);
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
    first.after_server_ticks(2 * TICKS_PER_SECOND as u64);
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
        client.after_server_ticks(2);
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
    client.after_server_ticks(3 * TICKS_PER_SECOND as u64);
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

/// Faces a player at `from` toward `at`, as `PlayerInput` takes it.
fn yaw_toward(from: Vec3, at: Vec3) -> f32 {
    let d = at - from;
    (-d.x).atan2(-d.z)
}

/// A player with an iron sword at Attack 20, beside a summoned brown bear facing it:
/// the bear's id and the sword's hotbar slot. Fights are timed on the clock the server takes
/// players' messages by (`fight`), the wall's, so a fight's waits are the wall's too; what
/// they wait for is answered by the server.
fn beside_a_bear(client: &mut Client) -> (u32, usize) {
    beside(client, "brown_bear")
}

/// As [`beside_a_bear`], beside a summoned creature of species `name`.
fn beside(client: &mut Client, name: &str) -> (u32, usize) {
    let bear = client.content.creatures.find(name).unwrap_or_else(|| panic!("a {name} in the content"));
    let sword = client.content.blocks.id("base:iron_sword").unwrap();
    // A flat floor with room around, so the bear and we stand on the ground.
    client.ok("/fill ~-8 ~-1 ~-8 ~8 ~-1 ~8 stone");
    client.ok("/fill ~-8 ~ ~-8 ~8 ~4 ~8 air");
    // Empty-handed: the starting kit fills most of the hotbar, and what is given next has
    // to land on it.
    client.ok("/clear");
    client.ok("/skill attack 20");
    client.ok("/give iron_sword 1");
    client.pump_until(|c| (0..voxmar_engine::inventory::HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == sword)));
    let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == sword)).unwrap();
    client.ok(&format!("/summon {name} 1 ~ ~ ~"));
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear));
    let id = client.creatures.iter().find(|(_, v)| v.species == bear).map(|(&id, _)| id).unwrap();
    (id, slot)
}

/// Faces creature `id` holding `slot` and provokes it, a blow every three server seconds
/// while it does not fight back, until it is seen to begin a wind-up: its attack pose
/// coming on, not one still shown from the blow before (its end not yet heard). Returns
/// how many of its blows had come before, for the next to be waited past.
fn provoke_until_it_winds_up(client: &mut Client, id: u32, slot: usize, name: &str) -> usize {
    let attacking = |c: &Client| c.creatures.get(&id).is_some_and(|v| v.pose == voxmar_engine::creature::brain::Pose::Attacking);
    client.pump_until(|c| !attacking(c));
    let before = client.hurt.len();
    let mut poked = None;
    loop {
        assert!(client.creatures.contains_key(&id), "the {name} went");
        face(client, id, slot, false);
        if poked.is_none_or(|at| client.server_tick >= at + 3 * TICKS_PER_SECOND as u64) {
            client.propose("core:attack", ActionTarget::Creature(id));
            poked = Some(client.server_tick);
        }
        let asked = client.server_tick;
        client.pump_until(|c| attacking(c) || !c.creatures.contains_key(&id) || c.server_tick >= asked + 4);
        if attacking(client) {
            return before;
        }
    }
}

/// Stands `client` 1.2 blocks from creature `id`, facing it, holding `slot`.
fn face(client: &mut Client, id: u32, slot: usize, flying: bool) {
    let at = client.creatures[&id].position;
    if client.position.with_y(0.0).distance(at.with_y(0.0)) > 1.6 || (client.position.y - at.y).abs() > 0.5 {
        client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
    }
    let yaw = yaw_toward(client.position, at);
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying }));
}

#[test]
fn blows_on_the_beat_build_to_a_flourish() {
    use voxmar_engine::combat::Blow;
    let mut client = Client::join();
    // Creative, so the bear's blows pass us by; held still, so every swing reaches it.
    client.ok("/gamemode creative");
    let (id, slot) = beside_a_bear(&mut client);
    client.ok("/creatures hold");
    face(&mut client, id, slot, true);
    client.pump_until(|c| c.creatures[&id].position.with_y(0.0).distance(c.position.with_y(0.0)) < 2.0);
    // A sword's beat is 0.6 s after each swing starts: the first click starts the meter, and
    // each next one comes a little before the beat, in the buffer, so it is held for it.
    // Each is stamped exactly when it is due (`propose_at`, the clock the server times
    // swings by), however late a busy machine lets this thread send it; it is sent no
    // sooner, as the swing before must land first.
    let start = Instant::now();
    for k in 0..5u32 {
        let due = start + Duration::from_secs_f32(if k == 0 { 0.0 } else { 0.6 * k as f32 - 0.06 });
        while Instant::now() < due {
            client.pump_until(|_| true);
            std::thread::sleep(Duration::from_millis(1));
        }
        client.propose_at("core:attack", ActionTarget::Creature(id), due);
    }
    client.pump_until(|c| c.kinds.len() >= 5 || !c.creatures.contains_key(&id));
    assert_eq!(client.kinds, [Blow::Plain, Blow::Beat, Blow::Beat, Blow::Beat, Blow::Flourish], "damage {:?}", client.damage);
    // The server tells the pips after each, for the client's meter: one a beat, spent on the
    // flourish.
    assert_eq!(client.pips, [0, 1, 2, 3, 0]);
}

#[test]
fn a_swing_is_dropped_when_its_weapon_leaves_the_hand() {
    let mut client = Client::join();
    client.ok("/gamemode creative");
    let (id, slot) = beside_a_bear(&mut client);
    client.ok("/creatures hold");
    face(&mut client, id, slot, true);
    let other = (slot + 1) % voxmar_engine::inventory::HOTBAR_SLOTS;
    // Swung with the sword, then switched to an empty hand before it lands (its wind-up is
    // 0.1 s): no blow lands, neither the sword's nor a fist's.
    client.propose("core:attack", ActionTarget::Creature(id));
    face(&mut client, id, other, true);
    client.wait_server(2);
    assert_eq!(client.blows, 0, "the dropped swing still landed, for {:?}", client.damage);
    // Swung and kept in hand, it lands.
    face(&mut client, id, slot, true);
    client.wait_server(1);
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.blows == 1);
}

/// Whether a dodge proposed now is taken: a second one at once must then be refused by its
/// cooldown, so exactly one Teleport comes back (two: the first was refused too; none: the
/// cooldown did not hold).
fn dodge_taken(client: &mut Client) -> bool {
    let before = client.teleports;
    client.propose("core:dodge", ActionTarget::None);
    client.propose("core:dodge", ActionTarget::None);
    client.wait_server(1);
    client.teleports - before == 1
}

#[test]
fn a_dodge_needs_the_ground_and_one_refused_puts_the_player_back() {
    let mut client = Client::join();
    client.ok("/gamemode survival");
    client.ok("/fill ~-2 ~-1 ~-2 ~2 ~-1 ~2 stone");
    client.ok("/tp ~ ~6 ~");
    client.pump_until(|c| c.teleported.is_some());
    // Six blocks up and told nothing else, the server has us in the air past the grace.
    client.wait_server(1);
    client.teleported = None;
    client.propose("core:dodge", ActionTarget::None);
    client.pump_until(|c| c.teleported.is_some());
    // Landing, told just before the dodge (between the server's ticks), counts as ground.
    client.ok("/tp ~ ~-5 ~");
    client.wait_server(1);
    client.teleported = None;
    let landed = client.position - Vec3::Y;
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: landed, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.position = landed;
    assert!(dodge_taken(&mut client), "a dodge as the player landed was refused");
    // On the ground it is taken.
    client.ok("/tp ~ ~ ~");
    client.wait_server(2);
    assert!(dodge_taken(&mut client), "a dodge on the ground was refused");
}

#[test]
fn a_perfect_block_staggers_a_bear_and_a_dodge_lets_its_blow_pass() {
    use voxmar_engine::combat::{Blow, Defended};
    let mut client = Client::join();
    // In survival: creatures' blows pass a player in creative by.
    client.ok("/gamemode survival");
    client.ok("/skill hitpoints 60");
    client.pump_until(|c| c.health >= 100);
    let (id, slot) = beside_a_bear(&mut client);
    let bear = client.creatures[&id].species;
    let attack = client.content.creatures.get(bear).def.attack.expect("bears attack");
    let feel = client.content.combat.feel;
    let (windup, recovery) = (attack.windup.unwrap_or(feel.creature_windup), attack.recovery.unwrap_or(feel.creature_recovery));
    // Waits for the bear to wind up at us (standing beside it, facing it, striking it every
    // few seconds so it fights back), then answers `respond` `after` seconds later; returns
    // what came of the blow.
    let fight = |client: &mut Client, after: f32, respond: &dyn Fn(&Client)| -> Defended {
        let before = provoke_until_it_winds_up(client, id, slot, "bear");
        std::thread::sleep(Duration::from_secs_f32(after));
        respond(client);
        client.pump_until(|c| c.hurt.len() > before);
        client.hurt[before].1
    };
    // Each answer's moment moves by what came of the last, so a slow machine is met where
    // its blows fall: a block not perfect was too soon, a hit too late.
    let answer = |client: &mut Client, want: Defended, respond: &dyn Fn(&Client), undo: &dyn Fn(&Client)| {
        let mut after = windup - 0.1;
        let mut outcomes = Vec::new();
        for _ in 0..8 {
            let outcome = fight(client, after.max(0.0), respond);
            undo(client);
            outcomes.push(outcome);
            if outcome == want {
                return;
            }
            after += if outcome == Defended::Blocked { 0.05 } else { -0.05 };
            client.wait_server(1);
        }
        panic!("answers to the bear's blows came to {outcomes:?}");
    };
    // Raised just before the blow: a perfect block, and the next blow a counter.
    let raise = |c: &Client| {
        let _ = c.connection.send(ClientMessage::Guard { up: true });
    };
    let lower = |c: &Client| {
        let _ = c.connection.send(ClientMessage::Guard { up: false });
    };
    let health = client.health;
    answer(&mut client, Defended::Perfect, &raise, &lower);
    let perfect = client.hurt.len() - 1;
    assert_eq!(client.hurt[perfect].0, 0, "a perfect block takes nothing");
    assert!(client.health <= health);
    let blows = client.kinds.len();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.kinds.len() > blows);
    assert_eq!(client.kinds[blows], Blow::Counter);
    assert!(client.damage[blows] > 0, "a counter cannot miss");
    // Staggered after its recovery: its next blow comes no sooner than both, and a wind-up.
    face(&mut client, id, slot, false);
    client.pump_until(|c| c.hurt.len() > perfect + 1);
    let gap = client.hurt_at[perfect + 1] - client.hurt_at[perfect];
    let least = recovery + feel.stagger + windup;
    assert!(gap.as_secs_f32() > least - 0.1, "its next blow came {:.2}s after the perfect block: under {least:.2}s", gap.as_secs_f32());
    // A dodge just before the blow: it passes through.
    client.wait_server(1);
    answer(&mut client, Defended::Dodged, &|c| c.propose("core:dodge", ActionTarget::None), &|_| {});
}

#[test]
fn armour_goes_on_from_the_hand_a_shield_guards_and_death_counts_what_is_worn() {
    use voxmar_engine::combat::Defended;
    use voxmar_engine::inventory::{HOTBAR_SLOTS, WORN};
    let mut client = Client::join();
    let bear = client.content.creatures.find("brown_bear").expect("a brown bear in the content");
    let [shirt, shield, sword] = ["base:chainmail_shirt", "base:wooden_shield", "base:iron_sword"].map(|n| client.content.blocks.id(n).unwrap());
    for line in ["/clear", "/gamemode survival", "/skill defence 20", "/skill attack 20", "/skill hitpoints 60", "/give chainmail_shirt 1", "/give wooden_shield 1", "/give iron_sword 1"] {
        client.ok(line);
    }
    client.pump_until(|c| c.health >= 100 && [shirt, shield, sword].iter().all(|&i| (0..HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == i))));
    let slot = |c: &Client, item| (0..HOTBAR_SLOTS).find(|&s| c.inventory.get(s).is_some_and(|s| s.item == item)).unwrap();
    // Each goes on in its own worn slot; anywhere else it is refused.
    let (from, sword_slot) = (slot(&client, shield), slot(&client, sword));
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: WORN as u8, count: 1 });
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (WORN + 5) as u8, count: 1 });
    let from = slot(&client, shirt);
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (WORN + 1) as u8, count: 1 });
    client.pump_until(|c| c.inventory.worn[1].is_some_and(|s| s.item == shirt) && c.inventory.worn[5].is_some_and(|s| s.item == shield));
    assert!(client.inventory.worn[0].is_none(), "a shield is not a helmet");
    // A guard held up long before the blow is no perfect block: the shield stops 55% of it.
    client.ok("/summon brown_bear 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear));
    let id = client.creatures.iter().find(|(_, v)| v.species == bear).map(|(&id, _)| id).unwrap();
    // Struck once so it fights back (again every three server seconds until a blow of ours
    // is heard), then the guard goes up while it is not winding up.
    let mut struck: Option<u64> = None;
    let mut guarded: Option<usize> = None;
    client.pump_until(|c| c.server_tick > 0);
    let started = client.server_tick;
    loop {
        assert!(client.server_tick < started + 30 * TICKS_PER_SECOND as u64, "no guarded blow came (guard up: {}, our blows {})", guarded.is_some(), client.kinds.len());
        let at = client.creatures[&id].position;
        if client.position.distance(at) > 2.0 {
            client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
        }
        let yaw = yaw_toward(client.position, at);
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: sword_slot as u8, flying: false }));
        if guarded.is_none() && struck.is_none_or(|at| client.server_tick >= at + 3 * TICKS_PER_SECOND as u64) {
            client.propose("core:attack", ActionTarget::Creature(id));
            struck = Some(client.server_tick);
        }
        let winding = client.creatures[&id].pose == voxmar_engine::creature::brain::Pose::Attacking;
        if guarded.is_none() && !client.kinds.is_empty() && !winding {
            let _ = client.connection.send(ClientMessage::Guard { up: true });
            guarded = Some(client.hurt.len());
        }
        if guarded.is_some_and(|n| client.hurt.len() > n) {
            break;
        }
        // One round trip: the server has handled all of the above, a tick at most.
        client.settle();
    }
    // Raised long before the blow: no perfect block, the shield stops 55% of it.
    let (damage, defended) = client.hurt[guarded.unwrap()];
    assert_eq!(defended, Defended::Blocked, "a guard up long before is a plain block");
    assert!(damage <= 3, "the shield stops 55% of a blow of at most 6: {damage}");
    let _ = client.connection.send(ClientMessage::Guard { up: false });
    // Death counts what is worn among what is carried: the three most valuable are kept.
    client.ok("/kill");
    client.pump_until(|c| c.health == 0);
    client.propose("core:respawn", ActionTarget::None);
    client.pump_until(|c| c.health > 0);
    client.pump_until(|c| c.inventory.worn.iter().all(Option::is_none));
    let carried = |item| client.inventory.slots.iter().flatten().any(|s| s.item == item);
    assert!(carried(shirt) && carried(shield) && carried(sword), "the shirt, the shield and the sword are kept");
}

#[test]
fn backing_off_at_a_walk_does_not_escape_a_creatures_blow() {
    use voxmar_engine::combat::Defended;
    // The wiki's draft 65: a wind-up follows its target at a run for its first part and
    // the blow lunges as it lands, so stepping back at a walk (4.7 blocks a second) as it
    // winds up is still within its reach; only a dodge passes it (the test above). Big and
    // slow (a bear, a frost stag) and small and quick (a cave crawler, an ember salamander).
    let walk = 4.7;
    for name in ["brown_bear", "frost_stag", "cave_crawler", "ember_salamander"] {
        let mut client = Client::join();
        client.ok("/clear");
        client.ok("/gamemode survival");
        client.ok("/skill hitpoints 60");
        client.pump_until(|c| c.health >= 100);
        let (id, _) = beside(&mut client, name);
        // Provoked with bare hands at Attack 1, so a small one (a salamander has 9
        // hitpoints, an iron sword's max hit) lives to strike twice.
        client.ok("/skill attack 1");
        let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_none()).expect("an empty hand");
        let mut outcomes = Vec::new();
        let mut askew = 0;
        while outcomes.len() < 2 {
            let before = provoke_until_it_winds_up(&mut client, id, slot, name);
            // Only a wind-up begun facing us: one begun facing away (a creature struck from
            // behind or the side) turns at its turn rate, and walks at a third of its pace
            // while it does, so stepping back from it is a different matter. (Its view goes
            // out the tick its pose comes on, with the way it faces then.) Such a blow is
            // let fall, and the creature provoked again.
            let view = client.creatures[&id];
            let to_us = (client.position - view.position).with_y(0.0).normalize_or_zero();
            let facing = Vec3::new(-view.yaw.sin(), 0.0, -view.yaw.cos());
            if to_us.dot(facing) < 45f32.to_radians().cos() {
                askew += 1;
                assert!(askew <= 6, "the {name} wound up facing away {askew} times");
                client.pump_until(|c| c.hurt.len() > before);
                client.wait_server(1);
                continue;
            }
            // Straight back from it, facing it, until the blow lands: a walk's worth each
            // server tick, as the creature moves by them (a server running behind moves
            // both slower). Ticks are read off the server as it runs: the `Time` it tells
            // once a second would move us in jumps of a whole second's walk.
            let away = (client.position - client.creatures[&id].position).with_y(0.0).normalize_or_zero();
            let started = client.ticks();
            let mut last = started;
            while client.hurt.len() == before {
                let now = client.ticks();
                assert!(now < started + 5 * TICKS_PER_SECOND as u64, "the {name}'s blow never came: {:?}", client.creatures.get(&id).map(|v| v.pose));
                client.position += away * walk * (now - last) as f32 / TICKS_PER_SECOND as f32;
                last = now;
                let yaw = yaw_toward(client.position, client.creatures[&id].position);
                client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: false }));
                client.pump_until(|c| c.hurt.len() > before || c.ticks() > last);
            }
            outcomes.push(client.hurt[before].1);
            client.wait_server(1);
        }
        assert!(outcomes.iter().all(|&o| matches!(o, Defended::Hit | Defended::Missed)), "backing off a {name} at a walk came to {outcomes:?}");
    }
}

/// Waits for the bear `id` to strike while the guard is up and the player looks `off`
/// radians away from it, holding `slot`; returns what came of the blow.
///
/// The bear, once struck, may wind up at once, and a blow from that wind-up can land before
/// the guard is raised or heard (on a busy machine it did, 4 runs in 8: `Hit` with the
/// guard down). So the guard goes up as soon as a blow of ours is heard, and only a blow
/// whose wind-up began after the server had the guard up counts: one after the bear is
/// seen out of its attack pose once the server has handled the raise. (A blow's `Hurt`
/// comes before the view of its recovery, so any blow from an earlier wind-up has come by
/// then.) Raised a whole wind-up before the blow, the guard blocks it, never perfectly.
fn guarded_blow(client: &mut Client, id: u32, slot: usize, off: f32) -> voxmar_engine::combat::Defended {
    use voxmar_engine::creature::brain::Pose;
    let blows = client.kinds.len();
    client.pump_until(|c| c.server_tick > 0);
    let started = client.server_tick;
    let limit = started + 60 * TICKS_PER_SECOND as u64;
    // Beside it, looking `off` away from it.
    let face = |client: &mut Client| {
        assert!(client.creatures.contains_key(&id), "the bear went");
        let at = client.creatures[&id].position;
        if client.position.with_y(0.0).distance(at.with_y(0.0)) > 1.6 || (client.position.y - at.y).abs() > 0.5 {
            client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
        }
        let yaw = yaw_toward(client.position, at) + off;
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: false }));
    };
    // Struck so it fights back: again every one and a half server seconds until a blow of
    // ours is heard. Each loop is one round trip: the server has handled all before it.
    let mut struck: Option<u64> = None;
    while client.kinds.len() == blows {
        assert!(client.server_tick < limit, "none of our blows was heard");
        face(client);
        if struck.is_none_or(|at| client.server_tick >= at + 3 * TICKS_PER_SECOND as u64 / 2) {
            client.propose("core:attack", ActionTarget::Creature(id));
            struck = Some(client.server_tick);
        }
        client.settle();
    }
    let _ = client.connection.send(ClientMessage::Guard { up: true });
    client.settle();
    // The blows before the first wind-up begun with the guard up.
    let mut before = None;
    loop {
        assert!(client.server_tick < limit, "no blow came with the guard up");
        face(client);
        let winding = client.creatures.get(&id).is_some_and(|v| v.pose == Pose::Attacking);
        if before.is_none() && !winding {
            before = Some(client.hurt.len());
        }
        if let Some(before) = before
            && client.hurt.len() > before
        {
            let _ = client.connection.send(ClientMessage::Guard { up: false });
            let outcome = client.hurt[before].1;
            client.wait_server(1);
            return outcome;
        }
        client.settle();
    }
}

#[test]
fn a_shield_guards_wider_than_a_sword_but_not_below_its_level_or_beside_a_spear() {
    use voxmar_engine::combat::Defended;
    use voxmar_engine::inventory::{HOTBAR_SLOTS, WORN};
    let mut client = Client::join();
    client.ok("/gamemode survival");
    client.ok("/skill hitpoints 60");
    client.ok("/skill defence 20");
    client.ok("/skill attack 20");
    client.pump_until(|c| c.health >= 100);
    let (id, sword) = beside_a_bear(&mut client);
    let [shield, spear] = ["base:iron_shield", "base:bronze_spear"].map(|n| client.content.blocks.id(n).unwrap());
    client.ok("/give iron_shield 1");
    client.ok("/give bronze_spear 1");
    let slot = |c: &Client, item| (0..HOTBAR_SLOTS).find(|&s| c.inventory.get(s).is_some_and(|s| s.item == item));
    client.pump_until(|c| slot(c, shield).is_some() && slot(c, spear).is_some());
    let from = slot(&client, shield).unwrap();
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (WORN + 5) as u8, count: 1 });
    client.pump_until(|c| c.inventory.worn[5].is_some_and(|s| s.item == shield));
    let spear = slot(&client, spear).unwrap();
    // Looking 60 degrees off the bear: inside the shield's 150 but outside a sword's 90.
    let off = 60f32.to_radians();
    // A guarded blow inside the guard's arc is blocked even when the defence roll turns it.
    assert_eq!(guarded_blow(&mut client, id, sword, off), Defended::Blocked, "the iron shield at Defence 20 guards 60 degrees off");
    let two_handed = guarded_blow(&mut client, id, spear, off);
    assert!(matches!(two_handed, Defended::Hit | Defended::Missed), "a spear takes both hands, but the shield guarded: {two_handed:?}");
    client.ok("/skill defence 1");
    let below = guarded_blow(&mut client, id, sword, off);
    assert!(matches!(below, Defended::Hit | Defended::Missed), "below its level the shield guarded: {below:?}");
}

#[test]
fn a_dodge_waits_longer_in_heavier_armour() {
    use voxmar_engine::inventory::WORN;
    let mut client = Client::join();
    client.ok("/gamemode survival");
    client.ok("/fill ~-2 ~-1 ~-2 ~2 ~-1 ~2 stone");
    let shirt = client.content.blocks.id("base:chainmail_shirt").unwrap();
    client.ok("/give chainmail_shirt 1");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == shirt));
    client.wait_server(1);
    // Two pairs of dodges, the pairs 1.0 s apart by the clock the server judges dodges by
    // (`propose_at`: exactly, however busy the machine). Of each pair the second is refused
    // by the first's cooldown, and puts us back. Unarmoured (light, 1.0 s less the 0.1 a
    // client's clock may run ahead) the second pair's first is taken; in chainmail (medium,
    // 1.2 s less 0.1) it is refused too. Returns whether it was.
    let twice = |client: &mut Client| {
        let before = client.teleports;
        let first = Instant::now();
        let second = first + Duration::from_millis(1000);
        for at in [first, second] {
            client.propose_at("core:dodge", ActionTarget::None, at);
            client.propose_at("core:dodge", ActionTarget::None, at);
        }
        // All four handled, and the refusals' teleports with them.
        client.settle();
        let refused = client.teleports - before;
        assert!(matches!(refused, 2 | 3), "of four dodges, {refused} were refused (the first was refused, or a second at once was not)");
        // Past any cooldown before the next pairs: the next are stamped after now.
        client.pump_until(|_| Instant::now() > second + Duration::from_secs(2));
        refused == 3
    };
    assert!(!twice(&mut client), "unarmoured, a dodge 1.0 s after the last was refused");
    let from = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == shirt)).unwrap();
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (WORN + 1) as u8, count: 1 });
    client.pump_until(|c| c.inventory.worn[1].is_some_and(|s| s.item == shirt));
    assert!(twice(&mut client), "in chainmail, a dodge 1.0 s after the last was taken");
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
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    let started = client.server_tick;
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
    let started = client.server_tick;
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
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    let started = client.server_tick;
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
    let started = client.server_tick;
    while !client.ok("/machine ~8 ~ ~").contains("needs a kept animal") {
        assert!(server_seconds(&client, started) < 10, "one aurochs walks two wheels");
        wait_server(&mut client, 1);
    }
    assert!(!client.ok("/machine ~7 ~ ~").contains("needs a kept animal"), "the nearer wheel still turns");
    // The aurochs, kept, can be taken away again; then the wheel stops.
    assert!(client.ok("/creatures remove aurochs").contains("for 1 creature within"));
    let started = client.server_tick;
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
    let started = client.server_tick;
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
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
        client.propose("base:use_machine", ActionTarget::Block { block: furnace, face: Face::NegX });
        client.pump_until(|c| count(c, item) == 0);
    }
    let started = client.server_tick;
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
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    let started = client.server_tick;
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:use_machine", ActionTarget::Block { block: at, face: Face::NegX });
}

/// Server seconds since `tick`, by the clock it last told.
fn server_seconds(client: &Client, tick: u64) -> u64 {
    client.server_tick.saturating_sub(tick) / TICKS_PER_SECOND as u64
}

/// Waits `seconds` of the server's clock, from the next time it tells it.
fn wait_server(client: &mut Client, seconds: u64) {
    client.after_ticks(seconds * TICKS_PER_SECOND as u64);
}

/// Asks `/machine` about a block until its answer contains `want`, or panics after `seconds`
/// of the server's clock.
fn machine_says(client: &mut Client, at: &str, want: &str, seconds: u64) -> String {
    let started = client.server_tick;
    loop {
        let said = client.ok(&format!("/machine {at}"));
        if said.contains(want) {
            return said;
        }
        assert!(server_seconds(client, started) < seconds, "/machine {at} never said `{want}`: {said}");
        std::thread::sleep(Duration::from_millis(300));
    }
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
    let started = client.server_tick;
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
    let (mut moved, mut last) = (client.server_tick, progress(&said));
    loop {
        let said = client.ok(&format!("/machine {crusher_at}"));
        if said.contains("idle") {
            break;
        }
        if progress(&said) != last {
            (moved, last) = (client.server_tick, progress(&said));
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
    let aside: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().contains("machines.ron.corrupt-")).collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "not a machine");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// How often the server saves on its own (`SAVE_INTERVAL`), in ticks.
const SAVE_EVERY: u64 = 30 * TICKS_PER_SECOND as u64;

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
    let told = client.server_tick;
    client.pump_until(|c| c.server_tick > told);
}

#[test]
fn only_a_joined_player_leaving_saves_the_world() {
    let dir = world_dir("leavers");
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    let autosaves = |from: u64, to: u64| (to / SAVE_EVERY).saturating_sub(from.saturating_sub(TICKS_PER_SECOND as u64) / SAVE_EVERY);
    after_this_loop(&mut client);
    let (before, from) = (saved_generation(&dir), client.server_tick);
    // Three connections that close without a hello: none of them is a player leaving.
    for _ in 0..3 {
        let (connection, server_side) = local_pair();
        drop(connection);
        client._server.as_ref().unwrap().connections().send(server_side).unwrap();
    }
    after_this_loop(&mut client);
    let saves = saved_generation(&dir) - before;
    assert!(saves <= autosaves(from, client.server_tick), "{saves} saves for connections that never joined");
    // A player who joined does save the world as they leave.
    let other = client.another("other");
    after_this_loop(&mut client);
    let (before, from) = (saved_generation(&dir), client.server_tick);
    drop(other);
    after_this_loop(&mut client);
    let saves = saved_generation(&dir) - before;
    assert!(saves >= 1 && saves <= 1 + autosaves(from, client.server_tick), "{saves} saves for a player leaving");
    drop(client);
    let _ = std::fs::remove_dir_all(&dir);
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
    let in_chest = said.split(' ').zip(said.split(' ').skip(1)).find_map(|(n, word)| (word == "items").then(|| n.parse::<u32>().ok()).flatten()).unwrap_or(0);
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
    after_this_loop(&mut client);
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
    after_this_loop(&mut client);
    assert!(saved_generation(&dir) > before, "the save after counted");
    drop(client);
    assert_eq!(staged_files(&dir), Vec::<std::path::PathBuf>::new());
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
    owner.connection.send(ClientMessage::Input(PlayerInput { teleport: owner.teleports, position: owner.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    other.connection.send(ClientMessage::Input(PlayerInput { teleport: other.teleports, position: other.position, yaw: 0.0, pitch: 0.0, selected_slot: empty, flying: false }));
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
    let started = client.server_tick;
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
    let started = client.server_tick;
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
    owner.connection.send(ClientMessage::Input(PlayerInput { teleport: owner.teleports, position: owner.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    other.connection.send(ClientMessage::Input(PlayerInput { teleport: other.teleports, position: other.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    other.propose("core:place", ActionTarget::Block { block: wheel - IVec3::Y, face: Face::PosY });
    other.pump_until(|c| c.world.block(wheel) == Some(treadwheel));
    let started = owner.server_tick;
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
    let started = client.server_tick;
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:tend", ActionTarget::Creature(id));
    client.pump_until(|c| c.inventory.get(0).is_none());
    let started = client.server_tick;
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.chat.clear();
    client.propose("base:plant", ActionTarget::Block { block: ground, face: Face::PosY });
    client.pump_until(|c| !c.refused.is_empty());
    // A refusal, for the interface to show as one, not a console line.
    let text = client.refused[0].clone();
    assert!(text.contains("water close by") || text.contains("here for"), "refused sowing said {text:?}");
    // Named as the player knows it: the rice they hold, not a growing stage.
    assert!(text.contains("Rice") && !text.contains("Crop"), "refused sowing named the crop as {text:?}");
    client.settle();
    assert!(client.chat.is_empty(), "a refused sowing also wrote to the console: {:?}", client.chat);
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
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
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
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

/// The merged wild items (kitchen herbs, wild roots, pumpkin...) are gardened like the
/// herbs they stand for: in a herb planter each regrows once picked, for a gardener whose
/// Farming level reaches its rarity's `planter_level`, and is refused below it.
#[test]
fn every_merged_item_regrows_in_a_planter_at_its_level() {
    let mut client = Client::join();
    let ground = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
    let at = ground + IVec3::Y;
    for line in ["/clear", "/fill ~ ~-1 ~2 ~ ~-1 ~2 herb_planter", "/fill ~ ~ ~2 ~ ~1 ~2 air"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(ground) == c.content.blocks.id("base:herb_planter"));
    let merged = ["sweet_berry", "tart_berry", "kitchen_herbs", "wild_roots", "wild_grain", "mushroom", "pumpkin"];
    let needs = |client: &Client, name: &str| {
        let plant = client.content.blocks.expect(&format!("base:{name}"));
        client.content.regrowth.rules.planter_level.of(client.content.blocks.def(plant).rarity) as u8
    };
    // Place the item on the planter and pick it again. What a picked plant drops is picked
    // up some time later, so this neither clears the inventory nor assumes a slot: it gives
    // one more, looks for the slot a plant is in, and places from that one.
    let pick = |client: &mut Client, name: &str| {
        let plant = client.content.blocks.expect(&format!("base:{name}"));
        let slot_of = |c: &Client| c.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == plant));
        client.refused.clear();
        client.ok(&format!("/give {name} 1"));
        client.pump_until(|c| slot_of(c).is_some());
        let slot = slot_of(client).unwrap() as u8;
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot, flying: false }));
        client.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
        client.pump_until(|c| c.world.block(at) == Some(plant));
        client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
        client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
        plant
    };
    // Below its level the gardener is told, and nothing comes back.
    for name in merged {
        let level = needs(&client, name);
        if level <= 1 {
            continue;
        }
        pick(&mut client, name);
        client.pump_until(|c| !c.refused.is_empty());
        client.ok("/grow");
        client.settle();
        assert_eq!(client.world.block(at), Some(BlockId::AIR), "{name} grew back in a planter for a gardener below Farming {level}");
    }
    // At Farming 40 (a Rare plant's level) each comes back.
    client.ok("/skill farming 40");
    for name in merged {
        assert!(needs(&client, name) <= 40, "{name} asks more than Farming 40 of a gardener");
        let plant = pick(&mut client, name);
        assert!(client.refused.is_empty(), "{name} was refused to a gardener at Farming 40: {:?}", client.refused);
        client.ok("/grow");
        client.pump_until(|c| c.world.block(at) == Some(plant));
        client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
        client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
    }
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
