//! End-to-end server checks over in-process and TCP connections.

use std::time::{Duration, Instant};

use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3, Vec3Swizzles};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::machine::Role;
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::{
    ChatKind, ClientConnection, ClientMessage, ItemView, PROTOCOL_VERSION, PlayerId, PlayerInput, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND,
    Weather, local_pair, tcp,
};
use voxmar_server::{Limits, ServerConfig, ServerHandle, load_content};

struct Client {
    connection: ClientConnection,
    id: PlayerId,
    name: String,
    phase: &'static str,
    /// Whether the server's pings are answered, as a running game does; a minimized one
    /// sends nothing from the game itself, only its network thread's keepalives.
    pongs: bool,
    /// Dropped items in sight, and the ones we picked up.
    items: std::collections::HashMap<u32, ItemView>,
    collected: Vec<u32>,
    skills: voxmar_engine::skill::Skills,
    /// Experience earned, in tenths, in the order it was.
    earned: Vec<u32>,
    /// Prayer points, the most, and the prayers on, as last told.
    prayers: Option<(u16, u16, Vec<voxmar_engine::prayer::PrayerId>)>,
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
    /// The same, as the server's words before any language renders them.
    said: Vec<voxmar_engine::lang::Words>,
    /// Refusals received, in English.
    refused: Vec<String>,
    /// The other players the server has named to us, until it says they left.
    others: std::collections::HashMap<PlayerId, String>,
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
    hurt_by: Vec<(u32, u16)>,
    hurt_at: Vec<Instant>,
    /// Each time a creature was seen begin a wind-up (its attack pose coming on).
    wound_at: Vec<(u32, Instant)>,
    /// When the server sent each creature's attacking pose now shown (in process, a
    /// message's stamp is when it was sent): a wind-up's start, however late this thread
    /// read it.
    attacking_since: std::collections::HashMap<u32, Instant>,
    /// Each time a creature was shown staggered (`STAGGERED`), from not: which, how many of
    /// creatures' blows at us had been heard by then, and the pose it showed.
    staggers: Vec<(u32, usize, voxmar_engine::creature::brain::Pose)>,
    /// The stamp of the message being applied.
    stamp: Option<Instant>,
    journal: voxmar_engine::journal::Journal,
    quests: Vec<voxmar_engine::quest::QuestState>,
    /// The server, for the first player to join it (dropping it stops the server).
    _server: Option<ServerHandle>,
}

/// How many times faster than the clock a test server's ticks run. Waits here are judged by
/// ticks (`Client::after_ticks`), so the pace only changes how long a test takes.
const TICK_SPEEDUP: u32 = 20;

thread_local! {
    /// Set by a test that runs its server at the clock's pace (`real_time`).
    static REAL_TIME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set by a test whose server may tick quicker still (`quicker`).
    static SPEEDUP: std::cell::Cell<u32> = const { std::cell::Cell::new(TICK_SPEEDUP) };
    /// Set by a test whose server ticks in step with its player (`lockstep`).
    static LOCKSTEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How many times quicker than the clock a server in lockstep may tick at most: in practice
/// as quick as its player takes the ticks in.
const LOCKSTEP_SPEEDUP: u32 = 1000;

/// Makes this test's servers tick only as far as its player has taken in
/// (`ServerConfig::lockstep`), as quick as the player goes, for a test that plays a bot
/// whose messages carry the game's clock (`Fresh::now`), or that bounds what it waits for
/// in the server's seconds while nothing it judges reads the clock (machines, which work
/// by ticks; `why` says what it plays). A player starved of the cores on a busy machine
/// then never finds the server ticks ahead of it, and every tick it waits on is one it
/// saw. The first player to join drives it: only for a test with one player.
fn lockstep(why: &str) {
    assert!(!why.is_empty());
    LOCKSTEP.set(true);
}

/// Makes this test's servers tick `speedup` times as quick as the clock, for a long test
/// whose player stamps every message with the game's clock (`Fresh::now`), so that nothing
/// it waits on keeps the clock's pace (`why` says what it plays).
fn quicker(speedup: u32, why: &str) {
    assert!(!why.is_empty() && speedup > TICK_SPEEDUP);
    SPEEDUP.set(speedup);
}

/// Makes this test's servers tick at the clock's pace, for a test whose claim is about what
/// the server judges by the clock, not by its ticks (`why` says which): what is judged by
/// when messages arrived (a break, a blow, a move, silence) takes the same time however
/// quickly the ticks come, so a wait of so many ticks would be short of it.
fn real_time(why: &str) {
    assert!(!why.is_empty());
    REAL_TIME.set(true);
}

/// Starts a server as the game does, its ticks `TICK_SPEEDUP` times as quick unless the test
/// is about the clock (`real_time`) or makes the server run behind on purpose (`lag`).
fn spawn(mut config: ServerConfig, content: Content) -> std::io::Result<ServerHandle> {
    // No wandering party comes by chance: at these ticks one may arrive while a test slowed
    // by load is still running, and stand (or fight) beside its player.
    config.wandering_starts = false;
    if config.lag == voxmar_server::Lag::default() && !REAL_TIME.get() {
        config.tick_speedup = SPEEDUP.get();
        if LOCKSTEP.get() {
            config.lockstep = true;
            config.tick_speedup = LOCKSTEP_SPEEDUP;
        }
    }
    keep_world(config.seed, &content);
    voxmar_server::spawn(config, content)
}

/// Keeps the generator of a seed alive for the rest of the run. A server on a world another
/// holds open shares its generator (`Overworld::shared`), with the land round spawn already
/// planned; one dropped between two tests made the next plan the spawn village again, some
/// 1.4 s of each join.
fn keep_world(seed: u64, content: &Content) {
    /// The columns of each held world kept for the next server on it, about 1100.
    const REMEMBERED: usize = 128 << 20;
    static HELD: std::sync::Mutex<Vec<(u64, std::sync::Arc<voxmar_worldgen::Overworld>)>> = std::sync::Mutex::new(Vec::new());
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.iter().all(|(s, _)| *s != seed) {
        let world = voxmar_worldgen::WorldContent::load(content).expect("world content loads");
        let world = voxmar_worldgen::Overworld::shared(seed, content, &world);
        // Most tests stand round the same spawn: each server's land is the last one's.
        world.remember_columns(REMEMBERED);
        held.push((seed, world));
    }
}

/// The game's content, loaded and checked once for the whole run: every server a test
/// starts gets the same, as a player's game gives each world it opens. The item tables a
/// load sets (stack limits, pouches, worn slots, store holds) are the process's own, so a
/// test that loads other content checks it set the same tables (`load_other`).
fn content() -> Content {
    base().0.clone()
}

/// The game's content, and its item tables as its load set them.
fn base() -> &'static (Content, BaseTables) {
    static LOADED: std::sync::OnceLock<(Content, BaseTables)> = std::sync::OnceLock::new();
    LOADED.get_or_init(|| {
        let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
        let tables = BaseTables::read(&content);
        (content, tables)
    })
}

/// The item tables content sets for the whole process, read for the game's own blocks.
#[derive(Debug, PartialEq)]
struct BaseTables {
    stacks: Vec<u8>,
    pouches: Vec<Option<(voxmar_engine::block::BlockId, u16)>>,
    worn: Vec<Option<usize>>,
    /// Per store, what of every block it takes: the game's stores only, so a mod that makes
    /// one of the game's other blocks a store is not caught.
    holds: Vec<(voxmar_engine::block::BlockId, Vec<bool>)>,
}

impl BaseTables {
    fn read(base: &Content) -> Self {
        use voxmar_engine::inventory;
        let ids: Vec<_> = base.blocks.ids().collect();
        let stores = base.machines.iter().filter(|m| m.is_store()).map(|m| m.block).collect::<Vec<_>>();
        Self {
            stacks: ids.iter().map(|&id| inventory::max_stack(id)).collect(),
            pouches: ids.iter().map(|&id| inventory::pouch_holding(id)).collect(),
            worn: ids.iter().map(|&id| inventory::worn_slot(id)).collect(),
            holds: stores.into_iter().map(|store| (store, ids.iter().map(|&id| inventory::store_takes(store, id)).collect())).collect(),
        }
    }
}

/// Loads the content under `assets` (a mod's, say) and checks the item tables it left are
/// still the game's own, for its blocks: the tables are the process's, shared by every test
/// running beside it. One such load at a time, each checked before the next, so a bad
/// mod's tables are never covered by another load landing between.
#[track_caller]
fn load_other(assets: &std::path::Path) -> Content {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let (base, tables) = base();
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let content = load_content(assets).expect("content loads");
    assert!(BaseTables::read(base) == *tables, "loading other content changed the game's item tables (stack limits, pouches, worn slots or store holds) for every test");
    content
}

/// Opens and registers a local connection only once its Hello is ready.
fn local_connection(accepted: crossbeam_channel::Sender<voxmar_protocol::ServerConnection>) -> ClientConnection {
    let (connection, server_side) = local_pair();
    accepted.send(server_side).unwrap();
    connection
}

impl Client {
    /// Joins a new server in-process, as singleplayer does.
    #[track_caller]
    fn join() -> Self {
        Self::join_with(ServerConfig { view_distance: 2, ..Default::default() })
    }

    #[track_caller]
    fn join_with(config: ServerConfig) -> Self {
        Self::join_named(config, "test")
    }

    /// Joins a new server on `config` as `name`.
    #[track_caller]
    fn join_named(config: ServerConfig, name: &str) -> Self {
        let content = content();
        let server = spawn(config, content.clone()).unwrap();
        let accepted = server.connections();
        Self::greet_content(|| local_connection(accepted), name, Some(server), None, || content)
    }

    /// Joins a new server over TCP as `name`.
    fn join_remote(name: &str, admins: &[&str]) -> Self {
        let admins = admins.iter().map(|&a| a.to_owned()).collect();
        let content = content();
        let server = spawn(ServerConfig { view_distance: 2, admins, ..Default::default() }, content.clone()).unwrap();
        let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
        Self::greet_content(|| tcp::connect(addr).unwrap(), name, Some(server), None, || content)
    }

    fn hello(open: impl FnOnce() -> ClientConnection, name: &str, server: ServerHandle) -> Self {
        Self::greet(open, name, Some(server))
    }

    /// Joins this player's server too, as `name`.
    fn another(&self, name: &str) -> Self {
        let accepted = self._server.as_ref().expect("the first player holds the server").connections();
        Self::greet_content(|| local_connection(accepted), name, None, None, || self.content.clone())
    }

    fn greet(open: impl FnOnce() -> ClientConnection, name: &str, server: Option<ServerHandle>) -> Self {
        Self::greet_with(open, name, server, None)
    }

    /// Joins as `name`, giving the server `admin_token`.
    fn greet_with(open: impl FnOnce() -> ClientConnection, name: &str, server: Option<ServerHandle>, admin_token: Option<&str>) -> Self {
        Self::greet_content(open, name, server, admin_token, content)
    }

    /// Prepares the content and Hello before opening a connection. Loading and hashing
    /// the fixture must not spend the server's pending-login deadline, especially when
    /// other tests keep this process off its cores. `prepare` can model that delay in ticks.
    #[track_caller]
    fn greet_content(open: impl FnOnce() -> ClientConnection, name: &str, server: Option<ServerHandle>, admin_token: Option<&str>, prepare: impl FnOnce() -> Content) -> Self {
        let content = prepare();
        let hello = ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: admin_token.map(str::to_owned) };
        let connection = open();
        assert!(connection.send(hello), "{name:?}: connection closed before Hello was queued");
        let mut client = Self {
            connection,
            id: 0,
            name: name.to_owned(),
            phase: "welcome",
            pongs: true,
            items: Default::default(),
            collected: Vec::new(),
            skills: Default::default(),
            earned: Vec::new(),
            prayers: None,
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
            said: Vec::new(),
            refused: Vec::new(),
            others: Default::default(),
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
            hurt_by: Vec::new(),
            hurt_at: Vec::new(),
            wound_at: Default::default(),
            attacking_since: Default::default(),
            staggers: Vec::new(),
            stamp: None,
            journal: Default::default(),
            quests: Vec::new(),
            _server: server,
        };
        // Joined once it stands in its land and has been told the time.
        client.pump_until(|c| c.health > 0 && c.world.contains(ColumnPos::of_block(c.position.as_ivec3())) && c.day.is_some());
        client.phase = "playing";
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
    ///
    /// Nor is a server at work silent: its thread's CPU time going up says it is working,
    /// told or not (`ServerHandle::cpu_time`), and only a server that has neither sent
    /// anything nor worked for `SERVER_SILENT` is taken for stuck. Before its first tick it
    /// is opening the world and generating where the player stands, never idle, so until
    /// then the time in all is the CPU time its thread had: at load 120 a server starting
    /// with a share of a core sent no welcome for 60 s while it worked all along.
    #[track_caller]
    fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        const AWAY: Duration = Duration::from_millis(500);
        let began = Instant::now();
        let mut looked = began;
        let mut heard = looked;
        let mut worked = looked;
        let mut in_all = Duration::ZERO;
        let mut cpu = self._server.as_ref().and_then(ServerHandle::cpu_time);
        while !done(self) {
            // In lockstep, the tick this player has taken in once nothing is left to read:
            // its messages are all sent before the tick counts.
            let taken = self._server.as_ref().filter(|s| s.lockstep()).map(|s| s.ticks());
            let now = Instant::now();
            // Whatever the limits below make of the server's work and this thread's
            // pauses, no wait outlasts ten times the longest they allow by the wall's clock.
            assert!(
                now - began < SERVER_WAIT * 10,
                "{:?} (id {}, {}, tick {}): still waiting after {:.0}s of the wall's clock",
                self.name, self.id, self.phase, self.server_tick, (now - began).as_secs_f32()
            );
            let away = now - looked;
            looked = now;
            if away > AWAY {
                eprintln!("pump_until: this thread was away {:.1}s", away.as_secs_f32());
                heard += away;
                worked += away;
                continue;
            }
            let starting = self._server.as_ref().is_some_and(|s| s.ticks() == 0);
            let now_cpu = self._server.as_ref().and_then(ServerHandle::cpu_time);
            let work = cpu.zip(now_cpu).map(|(then, now)| now.saturating_sub(then));
            if work.is_some_and(|w| w > Duration::ZERO) {
                worked = now;
            }
            in_all += match work {
                Some(work) if starting => work,
                _ => away,
            };
            cpu = now_cpu.or(cpu);
            assert!(
                now - heard.max(worked) < SERVER_SILENT && in_all < SERVER_WAIT,
                "{name:?} (id {id}, {phase}, tick {tick}, local {local}): timed out waiting for server: {:.1}s in all, {:.1}s since its last message, {:.1}s since its thread last worked",
                in_all.as_secs_f32(),
                (now - heard).as_secs_f32(),
                (now - worked).as_secs_f32(),
                name = self.name, id = self.id, phase = self.phase, tick = self.server_tick, local = self.connection.is_local()
            );
            match self.connection.try_recv_at() {
                Ok(Some((stamp, message))) => {
                    heard = now;
                    self.stamp = Some(stamp);
                    self.apply(message);
                }
                Ok(None) => match taken {
                    Some(taken) => {
                        let server = self._server.as_ref().expect("in lockstep");
                        server.allow_tick(taken + 1);
                        server.wait_past(taken, Duration::from_millis(1));
                    }
                    None => std::thread::sleep(Duration::from_millis(1)),
                },
                Err(_) => panic!("{:?} (id {}, {}, tick {}, local {}): server disconnected", self.name, self.id, self.phase, self.server_tick, self.connection.is_local()),
            }
        }
    }

    fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Rejected { reason } => panic!("turned away: {reason:?}"),
            ServerMessage::Welcome { player, position, admin, allow_flight, .. } => {
                self.id = player;
                self.phase = "initial world";
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
            ServerMessage::Prayers { points, most, on } => self.prayers = Some((points, most, on)),
            ServerMessage::Weather { weather, quick, .. } => {
                self.weather = Some(weather);
                self.quick = quick;
            }
            ServerMessage::Chat { kind, text } => {
                self.chat.push((kind, self.content.english.render(&text, &self.content.blocks)));
                self.said.push(text);
            }
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
                    let attacking = |v: &voxmar_protocol::CreatureView| v.pose == voxmar_engine::creature::brain::Pose::Attacking;
                    if attacking(&view) && self.creatures.get(&view.id).is_none_or(|was| !attacking(was)) {
                        self.wound_at.push((view.id, Instant::now()));
                    }
                    if !attacking(&view) {
                        self.attacking_since.remove(&view.id);
                    } else if !self.creatures.get(&view.id).is_some_and(attacking) {
                        self.attacking_since.insert(view.id, self.stamp.unwrap_or_else(Instant::now));
                    }
                    let staggered = |v: &voxmar_protocol::CreatureView| v.tell == voxmar_protocol::STAGGERED;
                    if staggered(&view) && !self.creatures.get(&view.id).is_some_and(staggered) {
                        self.staggers.push((view.id, self.hurt.len(), view.pose));
                    }
                    self.creatures.insert(view.id, view);
                }
                for gone in gone {
                    self.attacking_since.remove(&gone.id);
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
            ServerMessage::Ping { id } => {
                if self.pongs {
                    self.connection.send(ClientMessage::Pong { id });
                }
            }
            ServerMessage::Hurt { creature, damage, defended, .. } => {
                self.hurt.push((damage, defended));
                self.hurt_by.push((creature, damage));
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
            ServerMessage::Players { seen, names, .. } => {
                let named = seen.into_iter().filter_map(|view| Some((view.id, view.name?)));
                for (id, name) in named.chain(names) {
                    // A name told empty is one who left the server.
                    if name.is_empty() {
                        self.others.remove(&id);
                    } else {
                        self.others.insert(id, name);
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
    #[track_caller]
    fn after_server_ticks(&mut self, ticks: u64) {
        let ran = |c: &Self| c._server.as_ref().expect("the first player holds the server").ticks();
        let until = ran(self) + ticks + 1;
        self.pump_until(|c| ran(c) >= until);
        self.drain();
    }

    /// Applies every message waiting.
    fn drain(&mut self) {
        while let Ok(Some((stamp, message))) = self.connection.try_recv_at() {
            self.stamp = Some(stamp);
            self.apply(message);
        }
    }

    /// Applies what the server sends until it tells of a tick at least `ticks` past the
    /// last it told of, or `done`: a wait by the server's clock, which a busy machine
    /// slows with it.
    #[track_caller]
    fn pump_ticks(&mut self, ticks: u64, done: impl Fn(&Self) -> bool) {
        let until = self.server_tick + ticks;
        self.pump_until(|c| c.server_tick >= until || done(c));
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
    #[track_caller]
    fn wait_server(&mut self, seconds: u64) {
        self.after_ticks(seconds * TICKS_PER_SECOND as u64);
    }

    /// Runs a command and returns the console lines it answers with.
    #[track_caller]
    fn command(&mut self, line: &str) -> Vec<(ChatKind, String)> {
        self.chat.clear();
        self.said.clear();
        self.connection.send(ClientMessage::Command { line: line.into() });
        self.pump_until(|c| !c.chat.is_empty());
        std::mem::take(&mut self.chat)
    }

    /// Waits until the server has handled everything sent before (refused or instant
    /// actions included), by a command's answer coming back after them.
    #[track_caller]
    fn settle(&mut self) {
        self.ok("/time");
    }

    /// Waits until the land of the columns all round the player's is in, its own too. Tests
    /// build within a few blocks of where the player stands, and the server judges a
    /// structure (a kiln's cook, a machine's survey) only on land it holds; on a busy machine
    /// a column beside the player's took many seconds more to load than the player's own, and
    /// a check timed in the server's seconds ran out on land not yet there (a kiln across two
    /// columns refused its cook; a turbine hall stayed incomplete for 17 server seconds). The
    /// server sends only land it holds, so once the client has it, so has the server.
    #[track_caller]
    fn land_round(&mut self) {
        self.land_within(1);
    }

    /// Waits until the client has every column within `radius` columns of the player's
    /// (across, a square as the server streams them): with `radius` the view distance, all
    /// the land the server will load round where the player stands.
    #[track_caller]
    fn land_within(&mut self, radius: i32) {
        self.pump_until(|c| {
            let at = ColumnPos::of_block(c.position.floor().as_ivec3());
            (-radius..=radius).all(|dx| (-radius..=radius).all(|dz| c.world.contains(at.offset(dx, dz))))
        });
    }

    /// Waits until at least `ticks` server ticks have passed from now, by the server's own
    /// clock (it tells it once a second).
    #[track_caller]
    fn after_ticks(&mut self, ticks: u64) {
        let told = self.server_tick;
        self.pump_until(|c| c.server_tick > told);
        let from = self.server_tick;
        self.pump_until(|c| c.server_tick >= from + ticks);
    }

    /// Waits until at least `ticks` server ticks have passed from now, and little longer:
    /// it asks the server's clock (`ask_tick`) rather than waiting on its once-a-second
    /// tell, which made a one-second wait one to two. For an admin, who may ask.
    #[track_caller]
    fn after_ticks_asked(&mut self, ticks: u64) {
        let from = self.ask_tick();
        while self.ask_tick() < from + ticks {}
    }

    /// As [`Client::wait_server`], by the asked clock ([`Client::after_ticks_asked`]).
    #[track_caller]
    fn wait_server_asked(&mut self, seconds: u64) {
        self.after_ticks_asked(seconds * TICKS_PER_SECOND as u64);
    }

    /// The server's tick now: `/time add 0` tells it at once and moves nothing. Its answer
    /// is taken out of the chat, and whatever else was said there kept.
    #[track_caller]
    fn ask_tick(&mut self) -> u64 {
        let before = self.chat.len();
        self.connection.send(ClientMessage::Command { line: "/time add 0".into() });
        let answer = |c: &Client| c.chat[before..].iter().position(|(kind, text)| *kind == ChatKind::Info && text.starts_with("Day ") && text.contains("(tick "));
        self.pump_until(|c| answer(c).is_some());
        let at = before + answer(self).expect("answered");
        self.chat.remove(at);
        self.server_tick
    }

    /// Waits until `count` holds still across 20 server ticks after everything sent was
    /// handled (drops still landing, pick-ups on the way), and returns it.
    #[track_caller]
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

    /// Claims a finished quest's rewards in the quest screen (`ClientMessage::ClaimQuest`).
    fn claim(&mut self, quest: &str) {
        self.connection.send(ClientMessage::ClaimQuest { name: quest.into() });
    }

    /// Whether the server says a quest is finished and its rewards claimed.
    fn claimed(&self, quest: &str) -> bool {
        self.quests.iter().any(|q| q.name == quest && q.done && q.claimed)
    }

    /// Runs a command that should succeed, returning its answer.
    #[track_caller]
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
    real_time("a break takes its time on the clock (stamps of arrival), and a pick-up delay is waited for in seconds here");
    let mut client = Client::join();
    client.clear();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    // Cobblestone (a bare hand breaks it, unlike natural stone), which takes long enough to mine that a server catching up cannot finish it
    // anywhere near at once.
    client.ok("/setblock ~ ~-1 ~ cobblestone");
    client.pump_until(|c| c.world.block(below) == c.content.blocks.id("base:cobblestone"));
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
    real_time("a break takes its time on the clock (stamps of arrival), and a pick-up delay is waited for in seconds here");
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
    client.pump_until(|c| c.items.values().any(|i| i.stack == ItemStack::new(before.item, 1) && i.velocity.z < -1.0));
}

#[test]
fn crops_are_sown_at_their_level_grow_and_are_harvested_for_experience() {
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let (ground, other) = (feet + IVec3::new(0, -1, 2), feet + IVec3::new(1, -1, 2));
    client.clear();
    for line in ["/fill ~ ~-1 ~2 ~1 ~-1 ~2 dirt", "/fill ~ ~ ~2 ~1 ~1 ~2 air", "/give wheat_seeds 5", "/give saffron_seeds 5", "/give stone_hoe 1"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(ground).is_some_and(|b| b != BlockId::AIR) && c.inventory.get(2).is_some_and(|s| s.item == c.content.blocks.expect("base:stone_hoe")));
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

    // Grown ripe, then broken: its harvest pops out, the straw the Crucible Kiln's bricks
    // take among it, and its experience is earned, the sowing's (2) with the harvest's (9),
    // in tenths.
    assert_eq!(client.ok("/grow ripe"), "Grew 1 plant.");
    client.pump_until(|c| c.world.block(ground + IVec3::Y) == Some(ripe));
    client.propose("core:mine", ActionTarget::Block { block: ground + IVec3::Y, face: Face::PosY });
    let straw = client.content.blocks.expect("base:straw");
    client.pump_until(|c| c.earned == [110] && c.items.values().any(|i| i.stack.item == wheat) && c.items.values().any(|i| i.stack.item == straw));
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
    // Cobblestone (bare hands break it, unlike natural stone) takes long enough that a server catching up on its ticks cannot finish the
    // block only begun before it is replaced.
    let mut client = Client::join();
    let feet = client.position.floor().as_ivec3();
    let (a, b, c) = (feet + IVec3::new(0, -1, 1), feet + IVec3::new(0, -1, 2), feet + IVec3::new(1, -1, 1));
    client.ok("/fill ~ ~-1 ~1 ~1 ~-1 ~2 cobblestone");
    client.pump_until(|cl| [a, b, c].into_iter().all(|p| cl.world.block(p) == cl.content.blocks.id("base:cobblestone")));
    let stone = client.world.block(a).unwrap();
    let hardness = client.content.blocks.info(stone).hardness;
    assert!(Duration::from_secs_f32(hardness) > CATCH_UP * 2, "cobblestone mines too fast for this test");
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
    client.ok("/fill ~ ~-1 ~1 ~ ~-1 ~2 cobblestone");
    client.pump_until(|cl| [a, b].into_iter().all(|p| cl.world.block(p) == cl.content.blocks.id("base:cobblestone")));
    let mine = |client: &Client, block: IVec3| client.propose("core:mine", ActionTarget::Block { block, face: Face::PosY });
    mine(&client, a);
    client.connection.send(ClientMessage::Finished { target: ActionTarget::Block { block: a, face: Face::PosY } });
    mine(&client, b);
    client.pump_until(|cl| cl.world.block(a) == Some(BlockId::AIR) && cl.world.block(b) == Some(BlockId::AIR));
}

#[test]
fn falling_hurts() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = Client::join();
    let height = fall_from_open_sky(&mut client);
    let expected = 20 - (height - 3) as u8;
    assert_eq!(client.health, expected, "a {height} block fall should cost {} health", height - 3);
    // An admin heals to full at once.
    assert_eq!(client.ok("/heal"), "You are at full health.");
    client.pump_until(|c| c.health == 20);
}

/// Falls from as high as the open sky above spawn allows, so the player lands hurt, and
/// returns the height fallen.
fn fall_from_open_sky(client: &mut Client) -> i32 {
    let start = client.position;
    let feet = start.floor().as_ivec3();
    // Rise as high as the open air (or grass) above spawn allows (at most 11 blocks), by a
    // teleport (on foot nobody may rise more than a jump), then drop back down one plausible
    // step per input.
    let open = |c: &Client, p: IVec3| c.world.block(p).is_some_and(|b| !c.content.blocks.info(b).solid);
    let height = (1..=12).take_while(|&dy| open(client, feet + IVec3::Y * dy)).count() as i32 - 1;
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
    height
}

/// A bite of food takes 0.6 s, and while it is under way a click does not swing. Starting
/// to eat drops the swing already under way, so the blow it would have struck never lands;
/// a swing once the bite is done lands as usual.
#[test]
fn a_bite_of_food_holds_the_swing_and_drops_the_one_under_way() {
    let mut client = Client::join();
    fall_from_open_sky(&mut client);
    let snail = client.content.creatures.find("lumen_snail").expect("a lumen snail in the content");
    let bread = client.content.blocks.expect("base:bread");
    let eat = client.content.actions.expect("core:eat");
    let seconds = match client.content.actions.get(eat).expect("core:eat is defined").duration {
        voxmar_engine::action::Duration::Seconds(s) => s,
        other => panic!("a bite is timed in seconds, not {other:?}"),
    };
    assert!((seconds - 0.6).abs() < 1e-6, "a bite takes 0.6 s, not {seconds}");
    client.ok("/clear");
    client.ok("/give bread 1");
    client.ok("/summon lumen_snail 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == snail));
    let (&id, view) = client.creatures.iter().find(|(_, v)| v.species == snail).unwrap();
    let at = view.position;
    client.ok(&format!("/tp {} {} {}", at.x + 1.2, at.y, at.z));
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == bread));
    // A blow with what is in hand is on its way (0.08 s on foot) when the bite starts.
    let base = Instant::now();
    client.propose_at("core:attack", ActionTarget::Creature(id), base);
    client.propose_at("core:eat", ActionTarget::None, base + Duration::from_millis(20));
    // Still eating, and past the quickest a click fires: refused, so no blow.
    client.propose_at("core:attack", ActionTarget::Creature(id), base + Duration::from_millis(400));
    client.after_server_ticks(12);
    assert_eq!(client.blows, 0, "no blow while eating, and the swing under way was dropped");
    // Once the bite is done, a swing lands.
    client.propose_at("core:attack", ActionTarget::Creature(id), base + Duration::from_millis(650));
    client.pump_until(|c| c.blows >= 1);
    client.after_server_ticks(4);
    assert_eq!(client.blows, 1, "one swing after the bite, and nothing else");
}

#[test]
fn only_admins_fly_fast() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
        let accepted = server.connections();
        let client = Client::greet_content(|| local_connection(accepted), "test", Some(server), None, || content);
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
    let mossy = r#"tags: ["stone", "bricks", "building", "mossy", "wet", "shelter:sound"]"#;
    assert!(source.contains(mossy));
    std::fs::write(&blocks, source.replace(mossy, r#"tags: ["stone", "bricks", "building", "mossy", "wet", "shelter:sound", "tagged"]"#)).unwrap();
    let tagged = load_other(&assets);
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
    furnishing_drops_across_restarts("altar", "stone_altar", "stone_altar", 1);
}

#[test]
fn a_village_crate_stack_gives_nothing_but_a_players_comes_back_whole_across_restarts() {
    furnishing_drops_across_restarts("crate-stack", "oak_crate_stack", "oak_crate_stack", 1);
}

#[test]
fn a_village_rug_gives_nothing_but_a_players_comes_back_whole_across_restarts() {
    furnishing_drops_across_restarts("rug", "red_rug", "red_rug", 1);
}

#[test]
fn a_village_hay_bale_gives_nothing_but_a_players_breaks_into_nine_straw_across_restarts() {
    furnishing_drops_across_restarts("hay-bale", "hay_bale", "straw", 9);
}

/// Every village prop (settlements.ron's furnishings) that a player's own copy breaks into
/// something gives nothing where the world set it: each is set as a village sets it and
/// broken, and nothing of what it drops comes to hand or lies loose.
#[test]
fn village_props_set_by_the_world_give_nothing() {
    use voxmar_worldgen::{Select, WorldContent};
    let mut client = Client::join();
    let world = WorldContent::load(&client.content).expect("world content");
    let f = &world.settlements.furnishings;
    let blocks = &client.content.blocks;
    let mut props: Vec<BlockId> = f.barrel.iter().chain(&f.bell).map(|name| blocks.expect(name)).collect();
    for list in [&f.hay, &f.sacks, &f.crates, &f.hanging, &f.nets] {
        for select in list {
            match select {
                Select::Name(name, _) => props.push(blocks.expect(name)),
                Select::Tag(tag, _) => props.extend(blocks.tagged(tag)),
            }
        }
    }
    props.sort();
    props.dedup();
    let giving: Vec<(String, BlockId)> = props
        .into_iter()
        .filter(|&id| !blocks.info(id).drops.is_air() && blocks.info(id).drop_count > 0)
        .map(|id| (blocks.def(id).name.trim_start_matches("base:").to_owned(), blocks.info(id).drops))
        .collect();
    assert!(giving.iter().any(|(name, _)| name == "hay_bale"), "no village prop gives anything: {giving:?}");
    let spot = client.position.floor().as_ivec3() + IVec3::new(1, 0, 1);
    for line in ["/clear", "/fill ~1 ~-1 ~1 ~1 ~-1 ~1 stone", "/fill ~1 ~ ~1 ~1 ~1 ~1 air"] {
        client.ok(line);
    }
    for (name, drops) in &giving {
        let prop = client.content.blocks.expect(&format!("base:{name}"));
        client.ok(&format!("/setblock ~1 ~ ~1 {name}"));
        client.pump_until(|c| c.world.block(spot) == Some(prop));
        client.propose("core:mine", ActionTarget::Block { block: spot, face: Face::PosY });
        client.pump_until(|c| c.world.block(spot) == Some(BlockId::AIR));
        let goods = |c: &Client| {
            c.inventory.slots.iter().flatten().filter(|s| s.item == *drops).map(|s| s.count as usize).sum::<usize>()
                + c.items.values().filter(|i| i.stack.item == *drops).map(|i| i.stack.count as usize).sum::<usize>()
        };
        assert_eq!(client.quiet(goods), 0, "a village's {name} gave {}", client.content.blocks.def(*drops).name);
    }
}

#[test]
fn a_players_curtain_drawn_back_still_gives_itself_back() {
    // A curtain is a furnishing, which gives nothing unless a player set it (a door drops
    // itself either way, so it could not show the mark lost).
    swapped_furnishing_gives_itself_back("curtain", "red_curtain", "red_curtain_open");
}

/// A swap (opening a door, drawing a curtain back) is the same thing in another state: the
/// player's piece, opened and broken, still drops itself.
fn swapped_furnishing_gives_itself_back(label: &str, name: &str, open: &str) {
    let dir = world_dir(label);
    let mut client = Client::join_with(ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() });
    let feet = client.position.floor().as_ivec3();
    let own = feet + IVec3::new(-1, 0, 1);
    for line in ["/clear".to_owned(), format!("/give {name} 1"), "/fill ~-1 ~ ~1 ~-1 ~1 ~1 air".to_owned(), "/fill ~-1 ~-1 ~1 ~-1 ~-1 ~1 stone".to_owned()] {
        client.ok(&line);
    }
    let (shut, opened) = (client.content.blocks.expect(&format!("base:{name}")), client.content.blocks.expect(&format!("base:{open}")));
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == shut));
    client.propose("core:place", ActionTarget::Block { block: own - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(shut));
    client.propose("base:open", ActionTarget::Block { block: own, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(opened));
    client.propose("core:mine", ActionTarget::Block { block: own, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(BlockId::AIR));
    let back = |c: &Client| {
        c.inventory.slots.iter().flatten().filter(|s| s.item == shut).map(|s| s.count as usize).sum::<usize>()
            + c.items.values().filter(|i| i.stack.item == shut).map(|i| i.stack.count as usize).sum::<usize>()
    };
    assert_eq!(client.quiet(back), 1, "a player's {name}, opened and broken, gave nothing back");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

fn furnishing_drops_across_restarts(label: &str, name: &str, material: &str, count: usize) {
    let dir = world_dir(label);
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let feet = client.position.floor().as_ivec3();
    let (village, own) = (feet + IVec3::new(1, 0, 1), feet + IVec3::new(-1, 0, 1));
    // One set by the world (a village's furnishing, as /setblock sets it), one the player places.
    for line in ["/clear".to_owned(), format!("/give {name} 1"), format!("/setblock ~1 ~ ~1 {name}"), "/fill ~-1 ~ ~1 ~-1 ~1 ~1 air".to_owned(), "/fill ~-1 ~-1 ~1 ~-1 ~-1 ~1 stone".to_owned()] {
        client.ok(&line);
    }
    let furnishing = client.content.blocks.expect(&format!("base:{name}"));
    let material = client.content.blocks.expect(&format!("base:{material}"));
    client.pump_until(|c| c.world.block(village) == Some(furnishing) && c.inventory.get(0).is_some_and(|s| s.item == furnishing));
    client.propose("core:place", ActionTarget::Block { block: own - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(furnishing));
    let goods = |c: &Client| {
        c.inventory.slots.iter().flatten().filter(|s| s.item == material).map(|s| s.count as usize).sum::<usize>()
            + c.items.values().filter(|i| i.stack.item == material).map(|i| i.stack.count as usize).sum::<usize>()
    };
    // The village's: nothing.
    client.propose("core:mine", ActionTarget::Block { block: village, face: Face::PosY });
    client.pump_until(|c| c.world.block(village) == Some(BlockId::AIR));
    assert_eq!(client.quiet(goods), 0, "a village's {name} dropped goods");
    // Saved and loaded, the player's is still the player's: its material drop is retained.
    drop(client);
    let mut client = Client::join_with(config());
    client.pump_until(|c| c.world.block(own) == Some(furnishing));
    client.propose("core:mine", ActionTarget::Block { block: own, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(BlockId::AIR));
    assert_eq!(client.quiet(goods), count, "a player's {name} lost its material drop");
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
    save.write_items(&[SavedItem { item: "base:parsley".into(), count: 2, position: [feet.x + 4.5, feet.y + 1.0, feet.z], age: 0.0, owner: None, lifetime: None, wear: 0 }]).unwrap();

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

/// The creative catalogue is not trusted: a merged-away plant asked for is the item it merged
/// into, and what no player could hold (a crop's stage, a fluid's other state) is not given.
/// `/clear oregano` clears that item too.
#[test]
fn creative_takes_are_checked_and_merged_plants_are_their_item() {
    let mut client = Client::join();
    client.ok("/gamemode creative");
    client.ok("/clear");
    let id = |client: &Client, name: &str| client.content.blocks.expect(name);
    let (oregano, herbs, stage) = (id(&client, "base:oregano"), id(&client, "base:kitchen_herbs"), id(&client, "base:wheat_crop_1"));
    let flowing = client.content.blocks.ids().find(|&b| client.content.blocks.fluid(b).is_some() && !client.content.blocks.holdable(b)).expect("a fluid state");
    for item in [stage, flowing, oregano] {
        client.connection.send(ClientMessage::Take { item, count: 2, held: false });
    }
    client.settle();
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == herbs && s.count == 2));
    let held: Vec<_> = client.inventory.slots.iter().flatten().map(|s| s.item).collect();
    assert_eq!(held, [herbs], "a crop stage, a fluid state or a merged plant was given as itself: {held:?}");
    client.ok("/clear oregano");
    client.pump_until(|c| c.inventory.slots.iter().flatten().count() == 0);
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
    // The tests' own teleport (`Client::tp`) lands exactly where asked, on whole numbers too.
    client.tp(Vec3::new(100.0, 150.0, -40.0));
    assert_eq!(client.position, Vec3::new(100.0, 150.0, -40.0));
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
    let mut guest = Client::hello(|| tcp::connect(addr).unwrap(), "guest", server);
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
    // Under a roof the rain stops falling on you, but you are wet through for a while yet:
    // the wet runs down (seconds left), and drying off by a fire ends it.
    let wet = |c: &Client| c.vitals.effects.iter().find(|a| matches!(c.content.effects.get(a.effect).name.as_str(), "core:soaked" | "core:drenched")).and_then(|a| a.seconds_left);
    client.ok("/fill ~-1 ~3 ~-1 ~1 ~3 ~1 stone");
    let roofed = client.position.floor().as_ivec3() + IVec3::new(0, 3, 0);
    client.pump_until(|c| c.world.block(roofed) == c.content.blocks.id("base:stone"));
    let left = wet(&client).expect("wet as the roof goes on");
    client.pump_until(|c| wet(c).is_some_and(|now| now + 1 < left));
    client.ok("/setblock ~1 ~ ~ campfire");
    client.pump_until(|c| wet(c).is_none());
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
    let mut guest = Client::hello(|| tcp::connect(addr).unwrap(), "guest", server);
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    // Attack 9, a fist's max hit 2: a blow behind a stall costs a second or more, and the
    // snail's four health took four blows at level 1.
    client.ok("/skill attack 9");
    client.ok("/summon lumen_snail 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == snail));
    let (&id, view) = client.creatures.iter().find(|(_, v)| v.species == snail).unwrap();
    // Stand beside it, and strike until it dies: four health, a hit in two.
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    // A pack that turns mutation off (`genetics`), so the chick's genes are only ever its
    // parents': at the world's chance one birth in some 270 drew an allele neither had.
    // That births mutate at the data's chance is `genes`' test.
    let (assets, content) = with_mod("breeding-no-mutation", &[("creatures.ron", "(genetics: (mutation: 0.0), creatures: [])")]);
    assert_eq!(content.creatures.genetics().mutation, 0.0);
    let mut client = join_content(content);
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
    // theirs (mutation is off in this pack; at the game's rate, a few chicks in a
    // thousand would carry a new allele).
    let genes = &client.content.creatures.get(hen).def.genes;
    assert!(!genes.is_empty(), "junglefowl have genes");
    let alleles = |v: &voxmar_protocol::CreatureView| match voxmar_engine::genes::unpack(genes, &v.genes).0[0] {
        voxmar_engine::genes::Gene::Pair(a, b) => [a, b],
        _ => panic!("plumage is an allele gene"),
    };
    let parents: Vec<u8> = hens.iter().flat_map(|id| alleles(&client.creatures[id])).collect();
    let chick = client.creatures.values().find(|v| v.young && v.kept).expect("born");
    assert!(alleles(chick).iter().all(|a| parents.contains(a)), "{:?} from parents {parents:?}", alleles(chick));
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

#[test]
fn looking_closely_shows_more_with_farming() {
    let mut client = Client::join();
    let hen = client.content.creatures.find("junglefowl").expect("junglefowl in the content");
    // Held from the tick it arrives: a hen left to wander could be out of `/creatures tame`'s
    // reach by the time that came, on a busy machine.
    client.ok("/summon junglefowl 1 ~ ~ ~-1 held");
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
    // About nine ticks a second instead of twenty: judged by its ticks, cobblestone would take
    // the server more than twice its time, long after the player saw it break.
    let lag = voxmar_server::Lag { per_tick: Duration::from_millis(60), ..Default::default() };
    let mut client = Client::join_with(ServerConfig { view_distance: 2, lag, ..Default::default() });
    client.clear();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.ok("/setblock ~ ~-1 ~ cobblestone");
    client.pump_until(|c| c.world.block(below) == c.content.blocks.id("base:cobblestone"));
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
    assert!(took >= hardness - 0.05, "cobblestone ({hardness} s) broke after {took:.2} s");
    // Each tick takes at least 60 ms, so the clock finishes it in fewer ticks than counting
    // them would.
    let by_ticks = (hardness * TICKS_PER_SECOND as f32) as u64;
    assert!(ran < by_ticks, "cobblestone broke after {ran} ticks, as many as {by_ticks} would take counted");
}

#[test]
fn placing_or_throwing_while_mining_does_not_cancel_the_mining() {
    // Instant actions run alongside a timed one: a block placed or an item thrown halfway
    // through a break must not cost the break the player then sees finish.
    let mut client = Client::join_with(ServerConfig { view_distance: 2, lag: SLOW, ..Default::default() });
    let feet = client.position.floor().as_ivec3();
    let (a, b) = (feet + IVec3::new(0, -1, 1), feet + IVec3::new(0, -1, 2));
    client.ok("/fill ~ ~-1 ~1 ~ ~-1 ~2 cobblestone");
    client.pump_until(|cl| [a, b].into_iter().all(|p| cl.world.block(p) == cl.content.blocks.id("base:cobblestone")));
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
    client.ok("/setblock ~ ~-1 ~9 cobblestone");
    client.pump_until(|c| c.world.block(far) == c.content.blocks.id("base:cobblestone"));
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
    real_time("a break takes its time on the clock (stamps of arrival), and a pick-up delay is waited for in seconds here");
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
    client.after_ticks_asked(with_pickaxe + 20);
    assert_eq!(client.world.block(ore), Some(iron), "iron ore broke at Mining 1");
    // At Mining 15 with a bare hand: refused too.
    client.ok("/skill mining 15");
    hold(&client, 1);
    mine(&client);
    client.settle();
    client.after_ticks_asked(by_hand + 20);
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
    for line in ["/clear", "/give stone_head 3", "/give stick 2", "/fill ~1 ~ ~1 ~1 ~ ~1 crafting_table"] {
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
    // By day, the day held and the sky clear: what bites here (and so the odds) stays the same.
    for line in ["/time set day", "/time freeze", "/weather clear", "/fill ~-1 ~-2 ~1 ~1 ~-1 ~3 water", "/fill ~-1 ~ ~1 ~1 ~3 ~3 air", "/give fishing_rod 1", "/skill fishing 80"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(water).is_some_and(|b| c.content.blocks.info(b).fluid.is_some()) && c.inventory.get(0).is_some_and(|s| s.item == c.content.blocks.expect("base:fishing_rod")));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    let fish = |c: &Client| c.inventory.slots.iter().flatten().any(|s| c.content.blocks.def(s.item).has_tag("fish"));
    let miss = client.content.english.render(&voxmar_engine::lang::Words::key("fishing.nothing"), &client.content.blocks);
    let nothing = |c: &Client| c.chat.iter().any(|(_, text)| *text == miss);
    // At level 80 with this rod, by day here, a cast lands six times in ten (the best fish
    // biting asks level 55: 0.35 + 0.008 a level above it + 0.05 for the rod tier,
    // fishing.rs), so sixteen casts miss them all about once in two million worlds. The roll
    // is drawn from the player's count of casts (once from the tick a cast landed on, when
    // six casts failed in a third of runs on a busy machine), so every run lands the same.
    // A cast takes five seconds of the clock (`actions.ron`), judged by when the player's
    // messages arrive: each is proposed as if clicked at `at`, and a command stamped just
    // past its time is handled after the cast is done, so the casts follow one another
    // exactly, however busy the machine, without waiting the seconds out. Each ends in a
    // fish or nothing before the next.
    let mut at = Instant::now();
    for _ in 0..16 {
        client.chat.clear();
        client.propose_at("base:fish", ActionTarget::Block { block: water, face: Face::PosY }, at);
        at += Duration::from_millis(5100);
        client.connection.send_at(ClientMessage::Command { line: "/time".into() }, at);
        client.pump_until(|c| fish(c) || nothing(c) || !c.refused.is_empty());
        assert!(client.refused.is_empty(), "a cast was refused: {:?}", client.refused);
        if fish(&client) {
            break;
        }
        at += Duration::from_millis(100);
    }
    assert!(fish(&client), "sixteen casts at Fishing 80 caught nothing");
    client.pump_until(|c| !c.earned.is_empty());
}

/// A player's casts go on being numbered across leaving and joining again: their file keeps
/// the count, so a cast's draw is never drawn a second time.
#[test]
fn the_count_of_casts_survives_leaving_and_joining_again() {
    let dir = world_dir("casts");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let casts = |client: &mut Client, times: usize| {
        let water = client.position.floor().as_ivec3() + IVec3::new(0, -1, 2);
        for line in ["/fill ~-1 ~-2 ~1 ~1 ~-1 ~3 water", "/fill ~-1 ~ ~1 ~1 ~3 ~3 air", "/clear", "/give fishing_rod 1"] {
            client.ok(line);
        }
        client.pump_until(|c| c.world.block(water).is_some_and(|b| c.content.blocks.info(b).fluid.is_some()) && c.inventory.get(0).is_some_and(|s| s.item == c.content.blocks.expect("base:fishing_rod")));
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
        let miss = client.content.english.render(&voxmar_engine::lang::Words::key("fishing.nothing"), &client.content.blocks);
        let mut at = Instant::now();
        for _ in 0..times {
            client.chat.clear();
            client.earned.clear();
            client.propose_at("base:fish", ActionTarget::Block { block: water, face: Face::PosY }, at);
            at += Duration::from_millis(5100);
            client.connection.send_at(ClientMessage::Command { line: "/time".into() }, at);
            // Each cast ends in a fish (and its experience) or nothing before the next.
            client.pump_until(|c| !c.earned.is_empty() || c.chat.iter().any(|(_, text)| *text == miss) || !c.refused.is_empty());
            assert!(client.refused.is_empty(), "a cast was refused: {:?}", client.refused);
            at += Duration::from_millis(100);
        }
    };
    let saved = || voxmar_server::save::WorldSave::new(&dir).read_player("test", &content().blocks).unwrap().expect("the player was saved").casts;
    let mut client = Client::join_with(config());
    casts(&mut client, 3);
    drop(client);
    assert_eq!(saved(), 3, "three casts kept");
    let mut client = Client::join_with(config());
    casts(&mut client, 1);
    drop(client);
    assert_eq!(saved(), 4, "the next cast numbered on from them");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The count that numbers a player's cooking rolls (whether a go burns) is kept, so the
/// next go after joining again is drawn on from it, not drawn again.
#[test]
fn the_count_of_goes_cooked_survives_leaving_and_joining_again() {
    let dir = world_dir("cooks");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let cook = |client: &mut Client, times: u32| {
        let fire = client.position.floor().as_ivec3() + IVec3::new(3, 0, 0);
        let baked = |c: &Client| {
            let item = c.content.blocks.expect("base:baked_potato");
            c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>()
        };
        client.clear();
        for line in ["/setblock ~3 ~ ~ campfire", &format!("/give potato {times}"), "/skill cooking 99"] {
            client.ok(line);
        }
        client.pump_until(|c| c.world.block(fire) == c.content.blocks.id("base:campfire") && c.inventory.get(0).is_some_and(|s| s.count as u32 == times));
        client.send_position(client.position, false);
        for go in 1..=times {
            client.propose("base:cook", ActionTarget::Block { block: fire, face: Face::PosY });
            client.pump_until(|c| baked(c) == go);
        }
    };
    let saved = || voxmar_server::save::WorldSave::new(&dir).read_player("test", &content().blocks).unwrap().expect("the player was saved").draws.cooks;
    let mut client = Client::join_with(config());
    cook(&mut client, 2);
    drop(client);
    assert_eq!(saved(), 2, "two goes kept");
    let mut client = Client::join_with(config());
    cook(&mut client, 1);
    drop(client);
    assert_eq!(saved(), 3, "the next go numbered on from them");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Sets down a feed trough two blocks south of `client` (some quest's `Placed` counts it),
/// and gives back its cell.
fn place_a_trough(client: &mut Client) -> IVec3 {
    let cell = client.position.floor().as_ivec3() + IVec3::new(0, 0, 2);
    for line in ["/clear", "/give feed_trough 1", "/fill ~ ~ ~2 ~ ~1 ~2 air", "/fill ~ ~-1 ~2 ~ ~-1 ~2 stone"] {
        client.ok(line);
    }
    let trough = client.content.blocks.expect("base:feed_trough");
    client.pump_until(|c| c.world.block(cell) == Some(BlockId::AIR) && c.inventory.get(0).is_some_and(|s| s.item == trough));
    client.propose("core:place", ActionTarget::Block { block: cell - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(cell) == Some(trough));
    cell
}

/// Breaks the block at `cell` (as a command does, through the server's block changes).
fn clear_cell(client: &mut Client, cell: IVec3) {
    client.ok(&format!("/fill {0} {1} {2} {0} {1} {2} air", cell.x, cell.y, cell.z));
    client.pump_until(|c| c.world.block(cell) == Some(BlockId::AIR));
}

/// The saved count of feed troughs `name` has set down for quests, and the cells the world
/// keeps for them; none before they are first saved.
fn troughs_saved(dir: &std::path::Path, name: &str) -> Option<(u32, Vec<[i32; 3]>)> {
    let save = voxmar_server::save::WorldSave::new(dir);
    let player = save.read_player(name, &content().blocks).ok()??;
    let count = player.tallies.iter().find(|(k, _)| k == "placed:base:feed_trough").map_or(0, |(_, n)| *n);
    let cells = save.read_meta().ok()?.quest_placed.into_iter().filter(|(_, owner, _)| owner == name).map(|(cell, _, _)| cell).collect();
    Some((count, cells))
}

/// Drops `leaver` and waits for the save they left: the server saves as one leaves, before
/// telling the others they are gone, so once `watcher` is told it reads `want`.
#[track_caller]
fn leave_and_check_troughs(watcher: &mut Client, leaver: Client, dir: &std::path::Path, want: (u32, Vec<[i32; 3]>), why: &str) {
    let (id, name) = (leaver.id, leaver.name.clone());
    // Told of them first, or there is no leaving to be told of.
    watcher.pump_until(|c| c.others.contains_key(&id));
    drop(leaver);
    watcher.pump_until(|c| !c.others.contains_key(&id));
    assert_eq!(troughs_saved(dir, &name), Some(want), "{why}");
}

#[test]
fn a_placed_block_broken_after_joining_again_comes_off_the_quest_tally() {
    let dir = world_dir("placed");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let cell = place_a_trough(&mut client);
    drop(client);
    assert_eq!(troughs_saved(&dir, "test"), Some((1, vec![cell.to_array()])), "the trough set down counts, its cell kept with the world");
    // After a restart, kept standing, it still counts.
    drop(Client::join_with(config()));
    assert_eq!(troughs_saved(&dir, "test"), Some((1, vec![cell.to_array()])), "the kept trough still counts after the restart");
    // After another, the same player breaks it: it comes off, though the server that saw
    // it placed is gone.
    let mut client = Client::join_with(config());
    clear_cell(&mut client, cell);
    drop(client);
    assert_eq!(troughs_saved(&dir, "test"), Some((0, vec![])), "the broken trough no longer counts, nor is its cell kept");
    // Set down once more, it counts once.
    let mut client = Client::join_with(config());
    place_a_trough(&mut client);
    drop(client);
    assert_eq!(troughs_saved(&dir, "test").map(|(n, _)| n), Some(1), "the trough set down again counts once");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A block one player set down for a quest and another broke comes off the first one's
/// count: while they play, while they are away, and while they lie dead away.
#[test]
fn a_placed_block_another_player_breaks_comes_off_its_owners_tally() {
    let dir = world_dir("placed-by-another");
    let config = ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut breaker = Client::join_named(config, "bob");
    // Both playing: Bob breaks Ada's trough.
    let mut ada = breaker.another("ada");
    let cell = place_a_trough(&mut ada);
    clear_cell(&mut breaker, cell);
    leave_and_check_troughs(&mut breaker, ada, &dir, (0, vec![]), "broken while she played");
    // Ada away: Bob breaks it, and she finds it off her count when she comes back.
    let mut ada = breaker.another("ada");
    let cell = place_a_trough(&mut ada);
    leave_and_check_troughs(&mut breaker, ada, &dir, (1, vec![cell.to_array()]), "set down, as she left");
    clear_cell(&mut breaker, cell);
    let ada = breaker.another("ada");
    leave_and_check_troughs(&mut breaker, ada, &dir, (0, vec![]), "broken while she was away");
    // Ada left dead: the same, as she comes back to life.
    let mut ada = breaker.another("ada");
    let cell = place_a_trough(&mut ada);
    ada.ok("/kill");
    ada.pump_until(|c| c.health == 0);
    leave_and_check_troughs(&mut breaker, ada, &dir, (1, vec![cell.to_array()]), "set down, as she left dead");
    clear_cell(&mut breaker, cell);
    let ada = breaker.another("ada");
    leave_and_check_troughs(&mut breaker, ada, &dir, (0, vec![]), "broken while she lay dead away");
    drop(breaker);
    std::fs::remove_dir_all(&dir).unwrap();
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

/// A player rejoining in another case of their name is the same player: the animals they keep
/// and what they dropped dying are still theirs (every comparison of names saw "BOB" as
/// someone else than "Bob" before).
#[test]
fn a_player_rejoining_in_another_case_keeps_their_animals_and_their_drops() {
    let dir = world_dir("name-case");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_named(config(), "Bob");
    let hen = client.content.creatures.find("junglefowl").expect("junglefowl in the content");
    client.ok("/clear");
    client.ok("/summon junglefowl 1 ~ ~ ~");
    for line in ["/give wheat 64", "/give iron_pickaxe 1", "/give iron_ingot 5", "/give dirt 30", "/give stick 10", "/give bread 1"] {
        client.ok(line);
    }
    let wheat = client.content.blocks.expect("base:wheat");
    let carried = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item != wheat).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.creatures.values().any(|v| v.species == hen) && c.inventory.get(0).is_some_and(|s| s.item == wheat) && carried(c) == 47);
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    let id = client.creatures.iter().find(|(_, v)| v.species == hen).map(|(&id, _)| id).unwrap();
    let mut fed = 0;
    while !client.creatures[&id].kept {
        assert!(fed < 12, "{fed} feedings did not tame it");
        let at = client.creatures[&id].position;
        client.ok(&format!("/tp {} {} {}", at.x + 0.8, at.y, at.z));
        client.propose("base:tend", ActionTarget::Creature(id));
        fed += 1;
        client.after_server_ticks(TICKS_PER_SECOND as u64);
    }
    // Dying leaves three things on them and the rest where they fell, theirs to gather.
    client.ok("/kill");
    client.pump_until(|c| c.health == 0 && carried(c) <= 3 + 5 && c.items.values().map(|i| i.stack.count as u32).sum::<u32>() >= 40);
    let lying = client.items.values().map(|i| i.stack.count as u32).sum::<u32>();
    assert!(lying >= 40, "{lying} lying about");
    drop(client);

    let mut client = Client::join_named(config(), "BOB");
    assert!(client.health > 0, "the returning dead player wakes under their saved name");
    // The same player: the animal they kept is kept by them still.
    client.pump_until(|c| c.creatures.values().any(|v| v.species == hen && v.kept && v.owner == Some(c.id)));
    // And what they dropped is theirs to gather: walking to it, they pick it up.
    for _ in 0..12 {
        let from = client.server_tick;
        client.pump_until(|c| carried(c) >= 47 || c.server_tick.saturating_sub(from) >= 5 * TICKS_PER_SECOND as u64);
        if carried(&client) >= 47 {
            break;
        }
        if let Some(at) = client.items.values().map(|i| i.position).next() {
            client.ok(&format!("/tp {} {} {}", at.x, at.y, at.z));
        }
    }
    let rest: Vec<_> = client.items.values().map(|i| (i.stack.count, i.position)).collect();
    assert!(carried(&client) >= 47, "a player back as BOB could not gather what Bob dropped: {rest:?}");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn prayers_drain_ward_off_blows_and_fill_at_an_altar() {
    real_time("combat is judged on the clock, and the ward drains by the server's seconds while the bear winds up");
    use voxmar_engine::combat::Defended;
    let mut client = Client::join();
    let book = client.content.prayers.clone();
    let [ward, keepsake] = ["base:ward_melee", "base:keepsake"].map(|n| book.id(n).unwrap());
    client.ok("/gamemode survival");
    client.ok("/skill hitpoints 60");
    // Below a prayer's Magic level it stays off.
    client.ok("/skill magic 45");
    client.pump_until(|c| c.prayers.as_ref().is_some_and(|p| p.1 == 55));
    client.connection.send(ClientMessage::Pray { prayer: ward, on: true });
    client.wait_server_asked(1);
    assert!(client.prayers.as_ref().unwrap().2.is_empty(), "Ward Melee is Magic 46");
    // Points are 10 and one a Magic level, full for a new player.
    client.ok("/skill magic 46");
    client.pump_until(|c| c.prayers.as_ref().is_some_and(|p| p.1 == 56));
    let (id, slot) = beside_a_bear(&mut client);
    client.connection.send(ClientMessage::Pray { prayer: ward, on: true });
    client.pump_until(|c| c.prayers.as_ref().is_some_and(|p| p.2 == vec![ward]));
    // A ward stops 80% of a creature's blow in its styles: a bear's 2 to 6 lets 1.2 at most
    // through, which rounds by chance to 2 at most.
    let bear = client.content.creatures.find("brown_bear").unwrap();
    let high = client.content.creatures.get(bear).def.attack.unwrap().damage.1;
    let let_through = (high as f32 * (1.0 - book.rules.ward)).ceil() as u16;
    assert!(let_through < high as u16 / 2, "a ward lets {let_through} of a bear's {high} through");
    for _ in 0..3 {
        let before = provoke_until_it_winds_up(&mut client, id, slot, "bear");
        let blow = wait_for_blow(&mut client, id, before);
        let (damage, defended) = client.hurt[blow];
        assert!(defended != Defended::Hit || damage <= let_through, "a warded bear's blow did {damage}");
    }
    // Ward Melee drains 24 a minute: some are gone.
    let (points, most, _) = client.prayers.clone().unwrap();
    assert!(points < most, "{points} of {most} after a fight warded");
    // Off; at an altar they come back in full, and a bone in hand is offered for Magic
    // experience (16 a bone).
    client.connection.send(ClientMessage::Pray { prayer: ward, on: false });
    // The bear sent off, not killed: its loot would fall in a second or two of the server's
    // and come into our hands whenever it lands, on a busy machine after the `/clear` below,
    // and what we carry would never come to the 36 given.
    client.ok("/creatures disperse");
    let altar = client.position.floor().as_ivec3() + IVec3::new(0, 0, 2);
    client.ok("/setblock ~ ~ ~2 stone_altar");
    client.ok("/give bone 1");
    let bone = client.content.blocks.expect("base:bone");
    client.pump_until(|c| c.world.block(altar) == c.content.blocks.id("base:stone_altar") && c.inventory.slots.iter().flatten().any(|s| s.item == bone));
    let bone_slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == bone)).expect("the bone in the hotbar");
    let yaw = yaw_toward(client.position, altar.as_vec3() + 0.5);
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.3, selected_slot: bone_slot as u8, flying: false }));
    client.wait_server_asked(1);
    let earned = client.earned.len();
    client.propose("base:pray", ActionTarget::Block { block: altar, face: Face::NegZ });
    client.pump_until(|c| c.prayers.as_ref().is_some_and(|p| p.0 == p.1) && c.earned.len() > earned);
    assert!(client.earned[earned..].contains(&160), "a bone offered: {:?}", &client.earned[earned..]);
    // Keepsake keeps a fourth thing through death.
    client.connection.send(ClientMessage::Pray { prayer: keepsake, on: true });
    for line in ["/clear", "/give iron_pickaxe 1", "/give iron_ingot 5", "/give dirt 30"] {
        client.ok(line);
    }
    let carried = |c: &Client| c.inventory.slots.iter().flatten().map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| carried(c) == 36 && c.prayers.as_ref().is_some_and(|p| p.2 == vec![keepsake]));
    client.ok("/kill");
    client.pump_until(|c| c.health == 0 && carried(c) == 4);
    // Death puts every prayer out.
    client.pump_until(|c| c.prayers.as_ref().is_some_and(|p| p.2.is_empty()));
}

#[test]
fn a_dead_players_prayers_stay_on_until_death_counts_the_keepsake() {
    let mut client = Client::join();
    let keepsake = client.content.prayers.id("base:keepsake").unwrap();
    client.ok("/gamemode survival");
    client.ok("/skill magic 25");
    client.pump_until(|c| c.prayers.as_ref().is_some_and(|p| p.1 == 35));
    client.connection.send(ClientMessage::Pray { prayer: keepsake, on: true });
    for line in ["/clear", "/give iron_pickaxe 1", "/give iron_ingot 5", "/give dirt 30"] {
        client.ok(line);
    }
    let carried = |c: &Client| c.inventory.slots.iter().flatten().map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| carried(c) == 36 && c.prayers.as_ref().is_some_and(|p| p.2 == vec![keepsake]));
    // Turned off in the same breath as the death, after it: the dead cannot, so the
    // Keepsake still counts and a fourth thing is kept.
    client.connection.send(ClientMessage::Command { line: "/kill".into() });
    client.connection.send(ClientMessage::Pray { prayer: keepsake, on: false });
    client.pump_until(|c| c.health == 0 && carried(c) <= 4);
    assert_eq!(carried(&client), 4, "Keepsake keeps a fourth even when prayers were turned off after death");
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

/// An animal is found only from close up with nothing in between (`seen_within` in
/// journal.ron, 3 blocks for animals): not across the floor, not through a wall right
/// beside it, and at once when the wall goes. The rabbit is the one summoned (told apart
/// from any wild one by its id and height), held still from the tick it arrives (`/summon
/// ... held`: a hold sent after it could land when it had already fled, at these quick
/// ticks), and every wait is a count of server ticks.
#[test]
fn an_animal_is_found_only_from_close_up_in_plain_view() {
    use voxmar_engine::journal::Page;
    let mut client = on_a_test_floor();
    let at = client.position.floor().as_ivec3();
    let rabbit = client.content.creatures.find("rabbit").expect("rabbits in the content");
    let before: std::collections::HashSet<_> = client.creatures.keys().copied().collect();
    client.ok(&format!("/summon rabbit 1 {} {} {} held", at.x + 7, at.y, at.z));
    let ours = move |c: &Client| c.creatures.iter().find(|(id, v)| v.species == rabbit && !before.contains(id) && (v.position.y - at.y as f32).abs() < 1.0).map(|(&id, _)| id);
    client.pump_until(|c| ours(c).is_some());
    let id = ours(&client).unwrap();
    let found = |c: &Client| c.journal.get(Page::Animals, "base:rabbit").is_some();
    // Some blocks off in the open, held still: seen, but not found.
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    let p = client.creatures[&id].position;
    let off = p.with_y(0.0).distance(client.position.with_y(0.0));
    assert!((4.0..8.0).contains(&off), "the summoned rabbit is {off} blocks off, at {p:?}");
    assert!(!found(&client), "a rabbit {off} blocks away was found");
    // Within three blocks, with a wall between.
    let wall = p.x.floor() as i32 - 1;
    client.ok(&format!("/fill {wall} {} {} {wall} {} {} stone", at.y, at.z - 3, at.y + 3, at.z + 3));
    client.tp(Vec3::new(wall as f32 - 0.35, at.y as f32, p.z));
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    assert_eq!(client.creatures[&id].position, p, "the held rabbit moved");
    assert!(!found(&client), "a rabbit was found through a wall");
    // The wall gone, it is found from where the player stands within the journal's next
    // look round (once a second).
    client.ok(&format!("/fill {wall} {} {} {wall} {} {} air", at.y, at.z - 3, at.y + 3, at.z + 3));
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    let (now, me) = (client.creatures[&id].position, client.position);
    assert!(found(&client), "a rabbit at {now:?} was not found from {me:?} with nothing between");
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
    // The journal found the item, not the plant; `/give oregano` gives the item.
    use voxmar_engine::journal::Page;
    client.pump_until(|c| c.journal.get(Page::Plants, "base:kitchen_herbs").is_some());
    assert!(client.journal.get(Page::Plants, "base:oregano").is_none(), "the journal found a plant that is never listed");
    client.ok("/clear");
    client.ok("/give oregano 3");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == herbs && s.count == 3));
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == oregano), "/give oregano handed out the plant");
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
    // Finished, it waits to be claimed: the chat line points to the quest screen, and its
    // Engineering is not given with it.
    let paid = quest_xp(&client, "base:crafting_table");
    client.pump_until(|c| done(c, "base:crafting_table") && c.chat.iter().any(|(_, t)| t.contains("Quest done: Crafting Table. Claim its reward in the quest book")));
    client.settle();
    assert!(!client.earned.contains(&paid), "finishing paid before it was claimed: {:?}", client.earned);
    assert!(!client.claimed("base:crafting_table"));
    client.claim("base:crafting_table");
    client.pump_until(|c| c.claimed("base:crafting_table") && c.earned.contains(&paid));
    // Claiming again, or a quest not finished, pays nothing.
    client.claim("base:crafting_table");
    client.claim("base:first_ore");
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    assert_eq!(client.earned.iter().filter(|&&tenths| tenths == paid).count(), 1, "{:?}", client.earned);
    assert!(!client.claimed("base:first_ore"));
}

/// The Farm age's hand stations each beat the grid at what it already makes: a quern
/// grinds one wheat to a flour that bakes into bread (a grid loaf takes two), a spindle
/// spins a wool into three twine, and a loom weaves a wool into a
/// plain cloth, or with a dye flower held, two cloth of its colour. Each only at its own
/// station: wheat at the spindle is refused.
#[test]
fn the_quern_spindle_and_loom_grind_spin_and_weave() {
    let mut client = Client::join();
    let at = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let count = |c: &Client, name: &str| {
        let item = c.content.blocks.expect(name);
        c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>()
    };
    for line in ["/skill cooking 99", "/skill crafting 99", "/fill ~2 ~-1 ~ ~2 ~-1 ~ stone", "/fill ~2 ~ ~ ~2 ~1 ~ air"] {
        client.ok(line);
    }
    // Each go: the station set, the inventory cleared and given what is held (first, so it
    // is in hand) and what the pack carries, one click, and what comes of it.
    let work = |client: &mut Client, station: &str, give: &[(&str, u32)], made: (&str, u32), gone: (&str, u32)| {
        client.clear();
        client.ok(&format!("/setblock ~2 ~ ~ {station}"));
        for (item, n) in give {
            client.ok(&format!("/give {item} {n}"));
        }
        let block = client.content.blocks.expect(&format!("base:{station}"));
        client.pump_until(|c| c.world.block(at) == Some(block) && give.iter().all(|(item, n)| count(c, &format!("base:{item}")) == *n));
        client.send_position(client.position, false);
        client.propose("base:cook", ActionTarget::Block { block: at, face: Face::PosY });
        client.pump_until(|c| count(c, &format!("base:{}", made.0)) == made.1);
        assert_eq!(count(client, &format!("base:{}", gone.0)), gone.1, "{station}: what one go took");
    };
    work(&mut client, "quern_stones", &[("wheat", 2)], ("flour", 1), ("wheat", 1));
    work(&mut client, "quern_stones", &[("rye", 1)], ("flour", 1), ("rye", 0));
    work(&mut client, "campfire", &[("flour", 1)], ("bread", 1), ("flour", 0));
    work(&mut client, "spindle_stand", &[("wool", 1)], ("twine", 3), ("wool", 0));
    work(&mut client, "loom_frame", &[("wool", 2)], ("white_cloth", 1), ("wool", 1));
    work(&mut client, "loom_frame", &[("poppy", 1), ("wool", 1)], ("red_cloth", 2), ("wool", 0));
    assert_eq!(count(&client, "base:poppy"), 0, "the loom kept the dye flower");
    // Each refuses and keeps the wheat: the spindle works only its own, a village's quern
    // (its old_ twin) works nothing, and a quern below Cooking 3 nothing.
    let refuses = |client: &mut Client, station: &str, why: &str| {
        client.clear();
        client.ok(&format!("/setblock ~2 ~ ~ {station}"));
        client.ok("/give wheat 1");
        let block = client.content.blocks.expect(&format!("base:{station}"));
        client.pump_until(|c| c.world.block(at) == Some(block) && count(c, "base:wheat") == 1);
        client.propose("base:cook", ActionTarget::Block { block: at, face: Face::PosY });
        client.after_server_ticks(3 * TICKS_PER_SECOND as u64);
        client.settle();
        assert_eq!((count(client, "base:wheat"), count(client, "base:flour")), (1, 0), "{why}");
    };
    refuses(&mut client, "spindle_stand", "wheat was worked at a spindle");
    refuses(&mut client, "old_quern_stones", "a village's quern ground flour");
    client.ok("/skill cooking 2");
    refuses(&mut client, "quern_stones", "a quern ground flour at Cooking 2");
    // Broken, one the world set (an older world's village kept the real station) gives
    // nothing; one a player set down drops itself.
    for station in ["quern_stones", "spindle_stand", "loom_frame"] {
        let block = client.content.blocks.expect(&format!("base:{station}"));
        let loose = |c: &Client| count(c, &format!("base:{station}")) as usize + c.items.values().filter(|i| i.stack.item == block).map(|i| i.stack.count as usize).sum::<usize>();
        client.clear();
        client.ok(&format!("/setblock ~2 ~ ~ {station}"));
        client.pump_until(|c| c.world.block(at) == Some(block) && c.inventory.slots.iter().all(Option::is_none));
        client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
        client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
        assert_eq!(client.quiet(loose), 0, "a {station} the world set gave itself");
        client.ok(&format!("/give {station} 1"));
        // Held from whichever slot it came into.
        client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == block));
        let slot = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == block)).expect("given");
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot as u8, flying: false }));
        client.propose("core:place", ActionTarget::Block { block: at - IVec3::Y, face: Face::PosY });
        // Set down facing the player: one of its facings.
        client.pump_until(|c| c.world.block(at).is_some_and(|b| c.content.blocks.def(b).name.starts_with(&format!("base:{station}"))));
        client.propose("core:mine", ActionTarget::Block { block: at, face: Face::PosY });
        client.pump_until(|c| c.world.block(at) == Some(BlockId::AIR));
        assert_eq!(client.quiet(loose), 1, "a {station} a player set down did not drop itself");
    }
}

/// A stick held in a campfire a player set down comes back a firebrand in the hand, as the
/// gate's firebrand step does it: in its sealed stone room at midnight, the stick in a
/// hotbar slot after another item, and what the client sends for one press let go at once
/// (its hotbar slot, then the cook; a release sends nothing for a cook). Two sticks in hand
/// cook one: the brand goes where it fits and the other stick stays in hand, which is what
/// train 174's gate saw.
#[test]
fn a_stick_held_in_a_campfire_comes_back_a_firebrand() {
    let mut client = Client::join();
    let at = client.position.floor().as_ivec3() + IVec3::new(6, 0, 14);
    let fire = at + IVec3::new(2, 0, 0);
    let (stick, brand) = (client.content.blocks.expect("base:stick"), client.content.blocks.expect("base:firebrand"));
    client.clear();
    for line in ["/gamemode survival", "/time set midnight", "/creatures disperse", "/give dirt 1"] {
        client.ok(line);
    }
    client.ok(&format!("/fill {} {} {} {} {} {} stone", at.x - 3, at.y - 1, at.z - 3, at.x + 3, at.y + 5, at.z + 3));
    client.ok(&format!("/fill {} {} {} {} {} {} air", at.x - 2, at.y, at.z - 2, at.x + 2, at.y + 4, at.z + 2));
    client.ok(&format!("/setblock {} {} {} campfire", fire.x, fire.y, fire.z));
    client.tp(Vec3::new(at.x as f32 + 0.5, at.y as f32, at.z as f32 + 0.5));
    let named = |c: &Client, slot: usize| c.inventory.get(slot).map(|s| c.content.blocks.def(s.item).name.clone());
    // Two in hand (one picked up while it was held there): one catches and is held up all
    // the same, the other put by in the pack, so a lit brand is never left in the pack.
    for (sticks, hand_after) in [(1, "base:firebrand"), (2, "base:firebrand")] {
        client.ok("/clear stick");
        client.ok("/clear firebrand");
        client.ok(&format!("/give stick {sticks}"));
        client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == stick && s.count == sticks) && !c.inventory.slots.iter().flatten().any(|s| s.item == brand));
        let slot = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == stick)).unwrap();
        assert_eq!(slot, 1, "the stick after the dirt");
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot as u8, flying: false }));
        client.propose("base:cook", ActionTarget::Block { block: fire, face: Face::NegX });
        client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == brand) || c.chat.iter().any(|(_, t)| !t.is_empty()));
        assert_eq!(named(&client, slot).as_deref(), Some(hand_after), "{sticks} sticks in hand; said: {:?}", client.chat);
        let kept: u8 = client.inventory.slots.iter().flatten().filter(|s| s.item == stick).map(|s| s.count).sum();
        assert_eq!(kept, sticks as u8 - 1, "the rest kept");
    }
}

/// The First Night contract uses real grid and cooking events, and both kinds of
/// progress survive a restart before completion. Clay fired at a kiln (what is not food,
/// from another station) cannot stand in for a meal at the campfire.
#[test]
fn first_night_needs_a_crafted_campfire_and_food_cooked_on_it() {
    let dir = world_dir("first-night-food");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let fire = client.position.floor().as_ivec3() + IVec3::new(3, 0, 0);
    let done = |c: &Client| c.quests.iter().any(|q| q.name == "base:first_night" && q.done);
    let count = |c: &Client, name: &str| {
        let item = c.content.blocks.expect(name);
        c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>()
    };
    client.clear();
    for line in ["/setblock ~3 ~ ~ pit_kiln", "/give clay 1", "/skill cooking 99", "/skill crafting 99"] { client.ok(line); }
    client.pump_until(|c| c.world.block(fire) == c.content.blocks.id("base:pit_kiln") && count(c, "base:clay") == 1);
    // Loaded, left to fire and taken (stations.ron): it counts as cooked, but not as a meal.
    client.propose("base:use_station", ActionTarget::Block { block: fire, face: Face::PosY });
    client.pump_until(|c| c.world.block(fire) == c.content.blocks.id("base:pit_kiln_firing"));
    client.after_server_ticks(12 * TICKS_PER_SECOND as u64 + 2);
    client.pump_until(|c| c.world.block(fire) == c.content.blocks.id("base:pit_kiln_fired"));
    client.propose("base:use_station", ActionTarget::Block { block: fire, face: Face::PosY });
    client.pump_until(|c| count(c, "base:fired_clay") == 1);
    client.wait_server(2);
    assert!(!done(&client), "fired clay completed a food quest");
    assert_eq!(client.quests.iter().find(|q| q.name == "base:first_night").unwrap().tasks, vec![vec![(0, 1), (0, 1)]], "fired clay supplied cooked-food progress");
    client.ok("/setblock ~3 ~ ~ campfire");
    client.pump_until(|c| c.world.block(fire) == c.content.blocks.id("base:campfire"));
    client.clear();
    client.ok("/give raw_minnow 1");
    client.pump_until(|c| count(c, "base:raw_minnow") == 1);
    client.propose("base:cook", ActionTarget::Block { block: fire, face: Face::PosY });
    client.pump_until(|c| count(c, "base:cooked_minnow") == 1);
    client.wait_server(2);
    assert!(!done(&client), "a debug-given campfire replaced the crafting goal");
    drop(client);
    let mut client = Client::join_with(config());
    client.pump_until(|c| !c.quests.is_empty());
    let state = client.quests.iter().find(|q| q.name == "base:first_night").unwrap();
    assert_eq!(state.tasks, vec![vec![(0, 1), (1, 1)]], "saved cooking progress changed");
    client.clear();
    for line in ["/give oak_log 2", "/give cobblestone 2"] { client.ok(line); }
    client.pump_until(|c| count(c, "base:oak_log") == 2 && count(c, "base:cobblestone") == 2);
    for (from, cells) in [(0, &voxmar_engine::craft::SMALL[..2]), (1, &voxmar_engine::craft::SMALL[2..])] {
        for &cell in cells {
            client.propose("core:transfer", ActionTarget::Slots { from, to: (voxmar_engine::inventory::CRAFT + cell) as u8, count: 1 });
        }
    }
    client.propose("core:craft", ActionTarget::Slot(0));
    client.pump_until(|c| done(c) && count(c, "base:campfire") == 1);
    client.wait_server(2);
    assert_eq!(count(&client, "base:torch"), 0, "finishing paid the torches before they were claimed");
    drop(client);
    // The reward waits through a restart, in the save.
    let mut client = Client::join_with(config());
    client.pump_until(|c| done(c));
    client.wait_server(2);
    assert!(!client.claimed("base:first_night"), "rejoining claimed it");
    assert_eq!(count(&client, "base:torch"), 0);
    client.claim("base:first_night");
    client.pump_until(|c| c.claimed("base:first_night") && count(c, "base:torch") == 4);
    drop(client);
    let mut client = Client::join_with(config());
    client.pump_until(|c| done(c));
    client.wait_server(2);
    assert!(client.claimed("base:first_night"), "the claim was not saved");
    client.claim("base:first_night");
    client.wait_server(2);
    assert_eq!(count(&client, "base:torch"), 4, "rejoining or claiming again paid the quest twice");
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A standing kiln must receive a complete valid charge. Lighting progress and the
/// paid fire survive reload; taking out a brick and putting it back leaves it cold.
#[test]
fn kiln_fuel_and_lighting_progress_survive_reload_without_free_refuelling() {
    lockstep("a kiln's charge burns down by the server's ticks, so the cook after the reload finds it lit however long a busy machine takes by the clock");
    let dir = world_dir("kiln-fire");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let core = client.position.floor().as_ivec3() + IVec3::new(4, 0, 0);
    // The kiln lit, the quest's last part.
    let lit = |c: &Client| c.quests.iter().find(|q| q.name == "base:crucible_kiln").unwrap().tasks[0].last().unwrap().0;
    let light = |c: &Client| c.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
    // A cook takes its seconds of the clock, judged by when messages arrive: proposed as if
    // clicked now, with a command stamped just past its time, it is done at once, so no
    // clock time passes in lockstep while the charge burns by ticks. The command's answer is
    // read here, not taken for the next one's.
    let cook = |c: &mut Client| {
        let action = c.content.actions.expect("base:cook");
        let seconds = match c.content.actions.get(action).expect("base:cook is defined").duration {
            voxmar_engine::action::Duration::Seconds(s) => s,
            other => panic!("a cook is timed in seconds, not {other:?}"),
        };
        let at = Instant::now();
        c.propose_at("base:cook", ActionTarget::Block { block: core, face: Face::PosY }, at);
        c.chat.clear();
        c.connection.send_at(ClientMessage::Command { line: "/time".into() }, at + Duration::from_secs_f32(seconds + 0.1));
        c.pump_until(|c| c.chat.iter().any(|(_, text)| text.contains("(tick ")));
        c.chat.clear();
    };
    let count = |c: &Client, name: &str| {
        let item = c.content.blocks.expect(name);
        c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>()
    };
    client.clear();
    for line in ["/fill ~3 ~ ~-1 ~5 ~1 ~1 tempered_kiln_bricks", "/setblock ~4 ~ ~ crucible", "/give coal_ore 23", "/skill engineering 20"] { client.ok(line); }
    client.pump_until(|c| c.world.block(core) == c.content.blocks.id("base:crucible") && count(c, "base:coal_ore") == 23 && !c.quests.is_empty());
    client.send_position(client.position, false);
    light(&client);
    client.settle();
    client.wait_server(2);
    assert_eq!(lit(&client), 0, "an incomplete fuel charge counted as lit");
    assert_eq!(count(&client, "base:coal_ore"), 23);
    client.clear();
    client.ok("/give oak_log 32");
    client.pump_until(|c| count(c, "base:oak_log") == 32);
    light(&client);
    client.settle();
    client.wait_server(2);
    assert_eq!(lit(&client), 0, "a log was accepted as kiln fuel");
    assert_eq!(count(&client, "base:oak_log"), 32);
    client.clear();
    client.ok("/give coal_ore 24");
    client.pump_until(|c| count(c, "base:coal_ore") == 24);
    client.chat.clear();
    light(&client);
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Crucible Kiln is lit")) && lit(c) == 1);
    // The word and the quest can come before the inventory that paid for it: the charge as
    // it settles.
    assert_eq!(client.quiet(|c| count(c, "base:coal_ore") as usize), 0);
    drop(client);
    let meta = voxmar_server::save::WorldSave::new(&dir).read_meta().unwrap();
    let fire = meta.lit.iter().find(|fire| fire.position == core.to_array()).expect("saved paid fire").clone();
    let mut client = Client::join_with(config());
    // This kiln stands across two columns: a cook asked before the other is in is turned down.
    client.land_round();
    client.pump_until(|c| !c.quests.is_empty() && c.world.block(core) == c.content.blocks.id("base:crucible"));
    assert_eq!(lit(&client), 1);
    light(&client);
    client.settle();
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, "base:copper_ore") == 1);
    cook(&mut client);
    client.pump_until(|c| count(c, "base:copper_ingot") == 1);
    // Two consecutive edits must quench it even before any cooking or quest check.
    for line in ["/setblock ~3 ~1 ~-1 air", "/setblock ~3 ~1 ~-1 tempered_kiln_bricks"] { client.ok(line); }
    client.clear();
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, "base:copper_ore") == 1);
    client.chat.clear();
    cook(&mut client);
    client.pump_until(|c| c.refused.iter().any(|t| t.contains("is cold")));
    assert_eq!(count(&client, "base:copper_ore"), 1);
    assert_eq!(count(&client, "base:copper_ingot"), 0);
    assert_eq!(lit(&client), 1, "dismantling erased historical quest progress");
    drop(client);
    let meta = voxmar_server::save::WorldSave::new(&dir).read_meta().unwrap();
    assert!(meta.lit.is_empty(), "a quenched fire was saved");
    assert!(fire.until > meta.time, "the test accidentally waited for the charge to expire");
    std::fs::remove_dir_all(dir).unwrap();
}

/// The recipe viewer opened on an item is reported by the client: looking up the stone
/// pickaxe finishes "Look It Up", looking up anything else, or an item that does not
/// exist, does not.
#[test]
fn looking_an_item_up_finishes_the_look_it_up_quest() {
    let mut client = Client::join();
    let done = |c: &Client| c.quests.iter().any(|q| q.name == "base:look_it_up" && q.done);
    client.pump_until(|c| !c.quests.is_empty());
    assert!(!done(&client));
    let planks = client.content.blocks.expect("base:oak_planks");
    client.connection.send(ClientMessage::LookedUp { item: planks });
    client.connection.send(ClientMessage::LookedUp { item: BlockId(u16::MAX) });
    client.connection.send(ClientMessage::LookedUp { item: BlockId(0) });
    // Two server seconds, enough for the quest check to have run on all of it.
    client.wait_server(2);
    assert!(!done(&client), "other items' look-ups finished it");
    let pickaxe = client.content.blocks.expect("base:stone_pickaxe");
    client.chat.clear();
    client.earned.clear();
    client.connection.send(ClientMessage::LookedUp { item: pickaxe });
    client.pump_until(|c| done(c) && c.chat.iter().any(|(_, t)| t.contains("Quest done: Look It Up")));
    client.claim("base:look_it_up");
    let paid = quest_xp(&client, "base:look_it_up");
    client.pump_until(|c| c.earned.contains(&paid));
    for _ in 0..20 {
        client.connection.send(ClientMessage::LookedUp { item: pickaxe });
    }
    client.wait_server(2);
    assert_eq!(client.earned.iter().filter(|&&xp| xp == paid).count(), 1, "repeated look-ups rewarded XP again");
    assert_eq!(client.chat.iter().filter(|(_, text)| text.contains("Quest done: Look It Up")).count(), 1, "the quest completed again");
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
    let content = load_other(&assets);
    (assets, content)
}

/// Joins a new in-process server of `content`.
fn join_content(content: Content) -> Client {
    let server = spawn(ServerConfig { view_distance: 2, ..Default::default() }, content.clone()).unwrap();
    let accepted = server.connections();
    Client::greet_content(|| local_connection(accepted), "test", Some(server), None, || content)
}

/// Short names, as shipped quests write them: `stick` and `torch` are base's, found at load.
const TEST_QUESTS: &str = r#"(quests: [
    (name: "sticks", age: "nomad", branch: "main", title: "Three Sticks", why: "", tasks: [[Have("stick", 3)]], rewards: [Item("torch", 2)]),
    (name: "miner", age: "nomad", branch: "main", title: "Miner", why: "", tasks: [[Level("mining", 10)]], rewards: [Xp("crafting", 50.0)]),
    (name: "laden", age: "nomad", branch: "main", title: "Laden", why: "", tasks: [[Have("base:cobblestone", 2304)]], rewards: [Item("base:apple", 3)]),
    (name: "walk", age: "nomad", branch: "main", title: "Free Walk", why: "", tasks: [[Level("mining", 3)]]),
    (name: "dig", age: "nomad", branch: "main", title: "A Gravel Pit", why: "", tasks: [[Have("gravel", 1)]], rewards: [Item("clay", 4)]),
    (name: "potter", age: "nomad", branch: "main", title: "Potter", why: "", tasks: [[Have("clay", 5)]], rewards: [Item("base:apple", 1)]),
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
    client.pump_until(|c| done(c, "zz_test:sticks"));
    assert!(client.chat.iter().any(|(_, line)| line.contains("Three Sticks")), "{:?}", client.chat);
    // Done, the torches wait to be claimed, and come once.
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    assert_eq!(carried(&client, torch), 0);
    client.claim("zz_test:sticks");
    client.pump_until(|c| carried(c, torch) == 2);
    // Without the sticks, two checks later: still done, and the torches came once.
    client.ok("/clear stick");
    client.claim("zz_test:sticks");
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    assert!(done(&client, "zz_test:sticks"));
    assert_eq!(carried(&client, torch), 2);
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// What a quest taught stays as an effect (`Done` in an effect's `when`): it is not there
/// before the quest, it comes on at the check that finishes it with its say in chat, it
/// changes the player's modifiers, and as an announced step of a ladder it counts for a
/// quest that asks for the step (`Built("knack_trick", 1)`).
#[test]
fn a_finished_quest_teaches_an_effect_that_changes_modifiers_and_counts_as_a_step() {
    let quests = r#"(quests: [
    (name: "sticks", age: "nomad", branch: "main", title: "Three Sticks", why: "", tasks: [[Have("stick", 3)]], rewards: []),
    (name: "tricky", age: "nomad", branch: "main", title: "Tricky", why: "", tasks: [[Built("knack_trick", 1)]], after: ["sticks"], rewards: [Xp("crafting", 50.0)]),
])"#;
    let effects = r#"(effects: [
    (name: "trick", kind: Buff, title: "Trick", description: "A trick of the hand.", announce: true, ladder: "knack", when: Some(Done("sticks")), modifiers: (mining: 1.5, finds: 0.25)),
])"#;
    let (assets, content) = with_mod("quests-know-how", &[("quests.ron", quests), ("effects.ron", effects)]);
    let mut client = join_content(content);
    let known = |c: &Client| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == "zz_test:trick");
    let done = |c: &Client, name: &str| c.quests.iter().any(|q| q.name == name && q.done);
    client.pump_until(|c| !c.quests.is_empty());
    client.ok("/clear");
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    assert!(!known(&client) && !done(&client, "zz_test:tricky"), "nothing is known before the quest");
    assert_eq!(client.content.effects.modifiers(&client.vitals.effects).finds, 0.0);
    client.chat.clear();
    client.ok("/give stick 3");
    client.pump_until(|c| done(c, "zz_test:sticks") && known(c) && c.chat.iter().any(|(_, t)| t.contains("Trick")));
    let modifiers = client.content.effects.modifiers(&client.vitals.effects);
    assert_eq!((modifiers.mining, modifiers.finds), (1.5, 0.25));
    // Reaching the step counts for the quest that asks for it.
    client.pump_until(|c| done(c, "zz_test:tricky"));
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// An effect's `finds` adds to the chance of an extra piece from a natural block: at 1 the
/// extra always comes, so breaking one clay bank gives two clay.
#[test]
fn an_effect_that_adds_finds_turns_up_an_extra_piece_from_natural_ground() {
    let effects = r#"(effects: [(name: "lucky", kind: Buff, title: "Lucky", description: "", when: Some(Not(InBiome("zz_nowhere"))), modifiers: (finds: 1.0))])"#;
    let (assets, content) = with_mod("effects-finds", &[("effects.ron", effects)]);
    let mut client = join_content(content);
    let feet = client.position.floor().as_ivec3();
    let bank = feet + IVec3::new(2, 0, 0);
    client.ok("/clear");
    client.ok(&format!("/setblock {} {} {} clay", bank.x, bank.y, bank.z));
    client.pump_until(|c| c.world.block(bank) == c.content.blocks.id("base:clay") && c.content.effects.modifiers(&c.vitals.effects).finds == 1.0);
    client.propose("core:mine", ActionTarget::Block { block: bank, face: Face::NegX });
    client.pump_until(|c| holding(c, "base:clay") + loose(c, "base:clay") >= 2);
    assert_eq!(holding(&client, "base:clay") + loose(&client, "base:clay"), 2, "the clay and one more");
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
    // A quest not finished cannot be claimed.
    client.claim("zz_test:miner");
    client.earned.clear();
    client.ok("/skill mining 10");
    client.pump_until(|c| done(c, "zz_test:miner"));
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    let paid = quest_xp(&client, "zz_test:miner");
    // The 50 Crafting it writes, paid through the book's scale.
    let quests = &client.content.quests;
    assert_eq!(paid, (50.0 * quests.pays().of(quests.get("zz_test:miner").unwrap()) * 10.0).round() as u32);
    assert!(!client.earned.contains(&paid), "finishing paid before it was claimed: {:?}", client.earned);
    client.claim("zz_test:miner");
    client.pump_until(|c| c.earned.contains(&paid));
    client.claim("zz_test:miner");
    client.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    assert_eq!(client.earned.iter().filter(|&&tenths| tenths == paid).count(), 1, "{:?}", client.earned);
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// A reward the pack cannot hold is not given and not lost: the claim is refused with a
/// message, nothing is paid, and the quest stays claimable once there is room.
#[test]
fn a_quest_reward_that_does_not_fit_stays_claimable() {
    let (assets, content) = with_mod("quests-full", &[("quests.ron", TEST_QUESTS)]);
    let mut client = join_content(content);
    let apple = client.content.blocks.expect("base:apple");
    let apples = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == apple).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| !c.quests.is_empty());
    client.ok("/clear");
    // Every slot full of cobblestone.
    client.ok("/give cobblestone 2304");
    client.pump_until(|c| c.quests.iter().any(|q| q.name == "zz_test:laden" && q.done));
    client.chat.clear();
    client.claim("zz_test:laden");
    client.pump_until(|c| c.chat.iter().any(|(kind, text)| *kind == ChatKind::Error && text.contains("no room in your pack")));
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    assert!(!client.claimed("zz_test:laden"));
    assert_eq!(apples(&client), 0);
    assert!(!client.items.values().any(|i| i.stack.item == apple), "the reward dropped on the ground");
    // With room, the same quest pays out.
    client.ok("/clear cobblestone");
    client.claim("zz_test:laden");
    client.pump_until(|c| c.claimed("zz_test:laden") && apples(c) == 3);
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// A reward that finishes another quest (A Gravel Pit's four clay, the last of the five Potter
/// asks to carry: a reward never holds a goal alone, the load check refuses that) finishes it
/// as it is claimed, so that quest can be claimed at once, in the same moment,
/// not only after the next second's check.
#[test]
fn a_claimed_reward_finishes_the_quests_it_holds_at_once() {
    let (assets, content) = with_mod("quests-chain", &[("quests.ron", TEST_QUESTS)]);
    let mut client = join_content(content);
    let apple = client.content.blocks.expect("base:apple");
    client.pump_until(|c| !c.quests.is_empty());
    client.ok("/clear");
    client.ok("/give clay 1");
    client.ok("/give gravel 1");
    client.pump_until(|c| c.quests.iter().any(|q| q.name == "zz_test:dig" && q.done));
    assert!(!client.quests.iter().any(|q| q.name == "zz_test:potter" && q.done));
    // Both claims reach the server together: the first pays the clay, which finishes Potter
    // before the second is read.
    client.claim("zz_test:dig");
    client.claim("zz_test:potter");
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    assert!(client.claimed("zz_test:potter"), "{:?}", client.quests.iter().find(|q| q.name == "zz_test:potter"));
    assert!(client.inventory.slots.iter().flatten().any(|s| s.item == apple));
    drop(client);
    std::fs::remove_dir_all(&assets).unwrap();
}

/// A quest that pays nothing has nothing to claim: it is claimed as it finishes, and the
/// chat does not send the player to the quest screen.
#[test]
fn a_quest_that_pays_nothing_is_claimed_as_it_finishes() {
    let (assets, content) = with_mod("quests-free", &[("quests.ron", TEST_QUESTS)]);
    let mut client = join_content(content);
    client.pump_until(|c| !c.quests.is_empty());
    client.chat.clear();
    client.ok("/skill mining 3");
    client.pump_until(|c| c.claimed("zz_test:walk"));
    assert!(client.chat.iter().any(|(_, t)| t.contains("Quest done: Free Walk") && !t.contains("Claim")), "{:?}", client.chat);
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
    for line in ["/fill ~3 ~ ~-1 ~5 ~ ~1 tempered_kiln_bricks", "/setblock ~4 ~ ~ crucible", "/setblock ~3 ~1 ~-1 tempered_kiln_bricks", "/setblock ~5 ~1 ~-1 tempered_kiln_bricks", "/setblock ~3 ~1 ~1 tempered_kiln_bricks", "/setblock ~5 ~1 ~1 tempered_kiln_bricks"] {
        client.ok(line);
    }
    client.pump_until(|c| c.world.block(core) == Some(crucible) && c.world.block(core + IVec3::new(1, 1, 1)) == c.content.blocks.id("base:tempered_kiln_bricks"));
    smelt(&mut client);
    client.pump_until(|c| c.refused.iter().any(|t| t.contains("is cold")));
    assert_eq!(count(&client, ore), 2, "a standing but cold kiln consumed ore");
    client.clear();
    client.ok("/give coal_ore 24");
    let coal = client.content.blocks.expect("base:coal_ore");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == coal && s.count == 24));
    client.chat.clear();
    client.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Crucible Kiln is lit")));
    // The inventory update follows the chat line, so wait for it rather than race it.
    client.pump_until(|c| count(c, coal) == 0);
    client.ok("/give copper_ore 2");
    client.pump_until(|c| count(c, ore) == 2);
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
fn a_bed_set_placed_or_another_players_is_yours() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let refused = |c: &Client| c.refused.iter().any(|t| t.contains("belongs to someone"));
    let bed = client.content.blocks.expect("base:oak_bed");
    // One set by command (as an older save or a map maker might), one the player sets down
    // within the pickup reach, so broken it comes back to them.
    let (set, own) = (feet + IVec3::new(2, 0, -1), feet + IVec3::new(1, 0, 1));
    for line in ["/clear".to_owned(), "/give oak_bed 1".to_owned(), format!("/setblock {} {} {} oak_bed", set.x, set.y, set.z)] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(set) == Some(bed) && c.inventory.get(0).is_some_and(|s| s.item == bed));
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: set, face: Face::PosY });
    client.pump_until(|c| told(c, "wake here"));
    assert!(!refused(&client), "a bed no village planned was refused");
    client.propose("core:place", ActionTarget::Block { block: own - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(bed));
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: own, face: Face::PosY });
    client.pump_until(|c| told(c, "wake here"));
    assert!(!refused(&client), "the player's own bed was refused");
    // Picked up and set down again elsewhere, it is still theirs.
    client.propose("core:mine", ActionTarget::Block { block: own, face: Face::PosY });
    client.pump_until(|c| c.world.block(own) == Some(BlockId::AIR));
    // Back in the first slot, the one in hand, emptied when it was set down.
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == bed));
    let again = feet + IVec3::new(-2, 0, 2);
    client.propose("core:place", ActionTarget::Block { block: again - IVec3::Y, face: Face::PosY });
    client.pump_until(|c| c.world.block(again) == Some(bed));
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: again, face: Face::PosY });
    client.pump_until(|c| told(c, "wake here"));
    assert!(!refused(&client), "the bed set down again was refused");
    // Another player's bed is no village's: it works for a guest as for its owner.
    let mut guest = client.another("guest");
    client.ok(&format!("/tp guest {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5));
    guest.pump_until(|c| c.position.distance(feet.as_vec3() + Vec3::new(0.5, 0.0, 0.5)) < 1.0 && c.world.block(again) == Some(bed));
    guest.chat.clear();
    guest.propose("base:sleep", ActionTarget::Block { block: again, face: Face::PosY });
    guest.pump_until(|c| told(c, "wake here"));
    assert!(!refused(&guest), "another player's bed was refused the guest");
}

#[test]
fn a_furnished_home_heals_faster_and_sleeping_in_it_rests_you_once() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let has = |c: &Client, name: &str| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == name);
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    // A sleeping bag and a torch are a camp, once slept in; the sleep is a rest.
    let bag = feet + IVec3::new(2, 0, 0);
    for line in [format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z), format!("/setblock {} {} {} torch", feet.x + 3, feet.y, feet.z)] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag") && c.world.block(feet + IVec3::new(3, 0, 0)) == c.content.blocks.id("base:torch"));
    assert!(!has(&client, "base:camp"), "no bed slept in yet, so no camp");
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| has(c, "base:camp") && has(c, "base:rested") && told(c, "Camp:") && told(c, "You sleep soundly"));
    // The quests that ask for the steps see them (the whole of Make Camp's task holds; whether it is done waits on its earlier quests).
    client.pump_until(|c| quest_status(c, "make_camp").1);
    assert!(!quest_status(&client, "snug_for_winter").1, "a camp is not yet snug");
    // Rested, they are not tired again until it wears off.
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| told(c, "rested already"));
    assert!(!has(&client, "base:well_rested"));
    // Light a hearth with a chimney (a lit one is home), a second light, and walls and a
    // roof on a floor: snug.
    let core = feet + IVec3::new(4, 0, 0);
    let (low, high) = (feet + IVec3::new(-3, 0, -4), feet + IVec3::new(7, 3, 4));
    for line in [
        format!("/setblock {} {} {} hearthstone", core.x, core.y, core.z),
        format!("/setblock {} {} {} torch", feet.x + 3, feet.y + 1, feet.z),
        format!("/fill {} {} {} {} {} {} stone", feet.x - 2, feet.y + 4, feet.z - 3, feet.x + 6, feet.y + 4, feet.z + 3),
        format!("/fill {} {} {} {} {} {} stone", low.x, low.y - 1, low.z, high.x, low.y - 1, high.z),
        format!("/fill {} {} {} {} {} {} oak_planks", low.x, low.y, low.z, low.x, high.y, high.z),
        format!("/fill {} {} {} {} {} {} oak_planks", high.x, low.y, low.z, high.x, high.y, high.z),
        format!("/fill {} {} {} {} {} {} oak_planks", low.x, low.y, low.z, high.x, high.y, low.z),
        format!("/fill {} {} {} {} {} {} oak_planks", low.x, low.y, high.z, high.x, high.y, high.z),
    ] {
        client.ok(&line);
    }
    client.chat.clear();
    client.pump_until(|c| has(c, "base:snug") && told(c, "Snug:"));
    client.pump_until(|c| quest_status(c, "snug_for_winter").1);
    // A night, now in a snug home, rests them better (the better rests are a night's).
    client.ok("/time set midnight");
    client.ok("/effect clear");
    client.pump_until(|c| !has(c, "base:rested"));
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| has(c, "base:well_rested") && told(c, "Well rested"));
    assert!(!has(&client, "base:rested"), "the better rest hides the plain one");
    // Out of reach of home, none of it holds.
    client.tp(feet.as_vec3() + Vec3::new(30.5, 0.0, 0.5));
    client.pump_until(|c| !has(c, "base:snug") && has(c, "base:well_rested"));
}

/// Windows count at home (docs/home.md "Light in the windows"): two hide windows set in a
/// roofed home's wall make a Bright room, announced; one taken out, it goes.
#[test]
fn windows_at_home_make_a_bright_room() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let has = |c: &Client, name: &str| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == name);
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let bag = feet + IVec3::new(2, 0, 0);
    let (one, two) = (feet + IVec3::new(1, 1, -3), feet + IVec3::new(3, 1, -3));
    for line in [
        format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z),
        format!("/fill {} {} {} {} {} {} stone", feet.x - 2, feet.y + 3, feet.z - 3, feet.x + 6, feet.y + 3, feet.z + 3),
        format!("/setblock {} {} {} hide_window", one.x, one.y, one.z),
    ] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag") && c.world.block(one) == c.content.blocks.id("base:hide_window"));
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| has(c, "base:rested"));
    assert!(!has(&client, "base:bright_room"), "one window is not a bright room");
    client.chat.clear();
    client.ok(&format!("/setblock {} {} {} hide_window", two.x, two.y, two.z));
    client.pump_until(|c| has(c, "base:bright_room") && told(c, "Bright room:"));
    client.ok(&format!("/setblock {} {} {} air", one.x, one.y, one.z));
    client.pump_until(|c| !has(c, "base:bright_room"));
}

/// Things set out in the yard count at home (docs/home.md Rooms): an awning, and a bench and
/// a hay bale out in the open by a slept-in bag under the sky, make a Dooryard, announced.
/// The same pieces under a roof of their own count for nothing; the player under a roof, it
/// goes; the bale taken away, it goes again.
#[test]
fn an_awning_a_bench_and_a_bale_make_a_dooryard() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let has = |c: &Client, name: &str| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == name);
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let bag = feet + IVec3::new(2, 0, 0);
    let (awning, bench, bale) = (feet + IVec3::new(0, 2, 2), feet + IVec3::new(-2, 0, 2), feet + IVec3::new(4, 0, 2));
    // A shed roof over the bench and the bale: kept indoors.
    let shed = |block: &str| format!("/fill {} {} {} {} {} {} {block}", bench.x, bench.y + 3, bench.z - 1, bale.x, bale.y + 3, bale.z + 1);
    for line in [
        format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z),
        format!("/setblock {} {} {} red_awning", awning.x, awning.y, awning.z),
        format!("/setblock {} {} {} oak_bench", bench.x, bench.y, bench.z),
        format!("/setblock {} {} {} hay_bale", bale.x, bale.y, bale.z),
        shed("stone"),
    ] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag") && c.world.block(bale) == c.content.blocks.id("base:hay_bale"));
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| has(c, "base:rested"));
    assert!(!has(&client, "base:dooryard"), "a bench and a bale under a roof made a dooryard");
    client.chat.clear();
    client.ok(&shed("air"));
    client.pump_until(|c| has(c, "base:dooryard") && told(c, "Dooryard:"));
    // A roof over the player: indoors, not the yard.
    let roof = feet + IVec3::new(0, 2, 0);
    client.ok(&format!("/setblock {} {} {} stone", roof.x, roof.y, roof.z));
    client.pump_until(|c| !has(c, "base:dooryard"));
    client.ok(&format!("/setblock {} {} {} air", roof.x, roof.y, roof.z));
    client.pump_until(|c| has(c, "base:dooryard"));
    client.ok(&format!("/setblock {} {} {} air", bale.x, bale.y, bale.z));
    client.pump_until(|c| !has(c, "base:dooryard"));
}

/// A proper bed rests better than a bag (A Proper Bed: "something better than the ground"),
/// out under the sky with nothing around it: well rested, and a longer mend.
#[test]
fn a_proper_bed_rests_well_and_mends_longer_than_a_bag() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let effect = |c: &Client, name: &str| c.vitals.effects.iter().find(|a| c.content.effects.get(a.effect).name == name).copied();
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let (bag, bed) = (feet + IVec3::new(2, 0, 0), feet + IVec3::new(-2, 0, 0));
    for line in [format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z), format!("/setblock {} {} {} oak_bed", bed.x, bed.y, bed.z)] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag") && c.world.block(bed) == c.content.blocks.id("base:oak_bed"));
    // The bag: a plain rest, and a minute's mend.
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| effect(c, "base:rested").is_some() && effect(c, "base:healing_sleep").is_some());
    assert!(effect(&client, "base:healing_sleep").and_then(|a| a.seconds_left).is_some_and(|s| s <= 60));
    assert!(effect(&client, "base:well_rested").is_none(), "a bag on the ground is a plain rest");
    // The bed, at night: well rested, and two minutes' mend.
    client.ok("/time set midnight");
    client.ok("/effect clear");
    client.pump_until(|c| effect(c, "base:rested").is_none() && effect(c, "base:healing_sleep").is_none());
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| effect(c, "base:well_rested").is_some() && effect(c, "base:healing_sleep").is_some() && told(c, "Well rested"));
    assert!(effect(&client, "base:healing_sleep").and_then(|a| a.seconds_left).is_some_and(|s| s > 60), "a bed mends longer");
}

/// Nights told apart (docs/home.md "Nights"): a nap by day rests and mends but is no night's
/// rest, says so, and does not keep a night's better rest away; and after dark, indoors at
/// home by its fire, is an evening in, gone with the morning.
#[test]
fn a_nap_by_day_is_no_nights_rest_and_an_evening_in_is_by_the_fire_after_dark() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let has = |c: &Client, name: &str| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == name);
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let (bed, fire) = (feet + IVec3::new(2, 0, 0), feet + IVec3::new(-2, 0, 0));
    client.ok("/time set noon");
    for line in [
        format!("/setblock {} {} {} oak_bed", bed.x, bed.y, bed.z),
        format!("/setblock {} {} {} campfire", fire.x, fire.y, fire.z),
        format!("/fill {} {} {} {} {} {} stone", feet.x - 2, feet.y + 3, feet.z - 3, feet.x + 6, feet.y + 3, feet.z + 3),
    ] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bed) == c.content.blocks.id("base:oak_bed") && c.world.block(fire) == c.content.blocks.id("base:campfire"));
    // By day: a plain rest and a mend, and word of the better rest a night here gives.
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| has(c, "base:rested") && has(c, "base:healing_sleep") && told(c, "A night's sleep here would rest you better: Well rested"));
    assert!(!has(&client, "base:well_rested"), "a nap is no night's rest");
    assert!(!has(&client, "base:evening_in"), "home by the fire at noon is no evening");
    // A second nap rests no more.
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| told(c, "rested already"));
    // After dark: an evening in by the fire (the bed slept in is home). The night's rest
    // waits while the nap's mend runs, then comes though the nap's Rested is still on.
    client.ok("/time set midnight");
    client.chat.clear();
    client.pump_until(|c| has(c, "base:evening_in") && told(c, "Evening in:"));
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| told(c, "rested already"));
    assert!(!has(&client, "base:well_rested"), "no rest while the nap's mend runs");
    client.ok("/effect clear");
    client.ok("/effect rested 500");
    client.pump_until(|c| has(c, "base:rested") && !has(c, "base:healing_sleep"));
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| has(c, "base:well_rested") && told(c, "Well rested"));
    assert!(!told(&client, "would rest you better"), "a night's sleep says no better one");
    // Out from under the roof it is no evening in; the morning ends it too.
    client.tp(feet.as_vec3() + Vec3::new(8.5, 0.0, 0.5));
    client.pump_until(|c| !has(c, "base:evening_in"));
    client.tp(feet.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
    client.pump_until(|c| has(c, "base:evening_in"));
    client.ok("/time set day");
    client.pump_until(|c| !has(c, "base:evening_in"));
}

/// Rest is no way to heal in a fight (survival.ron): a bed refuses sleep with a creature that
/// hunts players near, and with wounds still fresh; once the hunter is gone and the wounds
/// have had their while, it gives its rest.
#[test]
fn no_sleep_with_a_hunter_near_or_wounds_fresh() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let healing = |c: &Client| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == "base:healing_sleep");
    let refused = |c: &Client, text: &str| c.refused.iter().any(|t| t.contains(text));
    let bag = feet + IVec3::new(2, 0, 0);
    client.ok(&format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z));
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag"));
    // A shambler hunts players: eight blocks off is too near.
    client.ok(&format!("/summon shambler 1 {} {} {}", feet.x + 8, feet.y, feet.z));
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| refused(c, "something hunting you"));
    assert!(!healing(&client), "no rest with a hunter near");
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    assert!(told(&client, "You will wake here after dying"), "a refused sleep claims the bed all the same");
    client.ok("/creatures kill shambler");
    // Hurt (a fall): the wounds are too fresh.
    client.ok(&format!("/fill {} {} {} {} {} {} air", feet.x - 2, feet.y + 1, feet.z - 3, feet.x + 3, feet.y + 14, feet.z + 3));
    client.pump_until(|c| c.world.block(feet + IVec3::Y * 12) == Some(BlockId::AIR));
    let before = client.health;
    fall_from_open_sky(&mut client);
    client.pump_until(|c| c.health < before);
    client.refused.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| refused(c, "wounds so fresh"));
    assert!(!healing(&client), "no rest with wounds fresh");
    // Their while out (15 s of the server's ticks), the bag gives its rest.
    client.after_server_ticks(15 * voxmar_protocol::TICKS_PER_SECOND as u64 + 1);
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(healing);
}

#[test]
fn a_larder_makes_more_of_a_meal() {
    real_time("eating takes its time on the clock (stamps of arrival)");
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    // Hurt by a fall first (food is eaten to heal), from air cleared high enough.
    client.ok(&format!("/fill {} {} {} {} {} {} air", feet.x - 2, feet.y + 1, feet.z - 3, feet.x + 9, feet.y + 14, feet.z + 3));
    client.pump_until(|c| c.world.block(feet + IVec3::Y * 12) == Some(BlockId::AIR));
    let at = |dx: i32| feet + IVec3::new(dx, 0, 0);
    let chip = |c: &Client, name: &str| c.vitals.effects.iter().find(|a| c.content.effects.get(a.effect).name == name).copied();
    // A bed to be home, slept in before the fall (nobody sleeps with wounds fresh).
    client.ok(&format!("/setblock {} {} {} sleeping_bag", at(2).x, at(2).y, at(2).z));
    client.pump_until(|c| c.world.block(at(2)) == c.content.blocks.id("base:sleeping_bag"));
    client.propose("base:sleep", ActionTarget::Block { block: at(2), face: Face::PosY });
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("wake here")));
    fall_from_open_sky(&mut client);
    // A hearth to cook by (its tag is all a home asks), four crates, a cauldron and a
    // table, under a roof: a larder.
    let mut lines = vec![format!("/fill {} {} {} {} {} {} stone", feet.x - 2, feet.y + 4, feet.z - 3, feet.x + 9, feet.y + 4, feet.z + 3)];
    for (dx, block) in [(3, "hearthstone"), (4, "crate"), (5, "crate"), (6, "crate"), (7, "crate"), (8, "cauldron"), (9, "oak_table")] {
        lines.push(format!("/setblock {} {} {} {block}", at(dx).x, at(dx).y, at(dx).z));
    }
    for line in lines {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(at(9)) == c.content.blocks.id("base:oak_table"));
    client.pump_until(|c| chip(c, "base:larder").is_some());
    // Bread gives Sustained for ten minutes; at a larder, for fifteen.
    for line in ["/clear", "/give bread 1", "/effect clear"] {
        client.ok(line);
    }
    let bread = client.content.blocks.expect("base:bread");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == bread));
    client.propose_at("core:eat", ActionTarget::None, Instant::now());
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    client.pump_until(|c| chip(c, "base:sustained").is_some());
    let left = chip(&client, "base:sustained").and_then(|a| a.seconds_left).unwrap();
    assert!(left > 800, "bread at a larder lasts 900 s, not {left}");
    // Away from home it is the plain ten minutes.
    client.ok("/effect clear");
    client.ok("/give bread 1");
    client.tp(feet.as_vec3() + Vec3::new(30.5, 0.0, 0.5));
    client.pump_until(|c| chip(c, "base:larder").is_none());
    client.propose_at("core:eat", ActionTarget::None, Instant::now());
    client.after_server_ticks(TICKS_PER_SECOND as u64);
    client.pump_until(|c| chip(c, "base:sustained").is_some());
    let left = chip(&client, "base:sustained").and_then(|a| a.seconds_left).unwrap();
    assert!(left <= 600, "bread away from home lasts 600 s, not {left}");
}

#[test]
fn a_home_answers_the_weather_the_animals_and_the_garden_around_it() {
    real_time("kept animals wander at the game's own pace, or they leave the house before they are counted");
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let has = |c: &Client, name: &str| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == name);
    let at = |dx: i32, dz: i32| feet + IVec3::new(dx, 0, dz);
    // A bag, a hearth and a roof: a snug home to react from.
    let mut lines = vec![format!("/fill {} {} {} {} {} {} stone", feet.x - 2, feet.y + 4, feet.z - 3, feet.x + 6, feet.y + 4, feet.z + 3)];
    for (dx, block) in [(2, "sleeping_bag"), (3, "hearthstone")] {
        lines.push(format!("/setblock {} {} {} {block}", at(dx, 0).x, at(dx, 0).y, at(dx, 0).z));
    }
    for line in lines {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(at(3, 0)) == c.content.blocks.id("base:hearthstone"));
    client.propose("base:sleep", ActionTarget::Block { block: at(2, 0), face: Face::PosY });
    client.pump_until(|c| has(c, "base:camp") || has(c, "base:hearthside"));
    // Fair weather, no animals, no crops: none of the reactions.
    for name in ["base:sheltered", "base:company", "base:full_house", "base:kitchen_garden"] {
        assert!(!has(&client, name), "{name} before anything is around");
    }
    // A storm outside and a fire inside: sheltered; out from under the roof it is just a storm.
    client.ok("/weather storm max 600");
    client.pump_until(|c| has(c, "base:sheltered"));
    client.tp(feet.as_vec3() + Vec3::new(0.5, 0.0, 6.5));
    client.pump_until(|c| !has(c, "base:sheltered"));
    client.tp(feet.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
    client.pump_until(|c| has(c, "base:sheltered"));
    client.ok("/weather clear");
    client.pump_until(|c| !has(c, "base:sheltered"));
    // One kept animal about the house is company; four, a full house that hides it.
    client.ok("/summon rabbit 1 ~4 ~ ~1");
    client.ok("/creatures tame rabbit");
    client.pump_until(|c| has(c, "base:company"));
    client.ok("/summon rabbit 3 ~4 ~ ~-1");
    client.ok("/creatures tame rabbit");
    client.pump_until(|c| has(c, "base:full_house"));
    assert!(!has(&client, "base:company"), "a full house hides company");
    // Out of reach of home, the animals are not at home with them.
    client.tp(feet.as_vec3() + Vec3::new(40.5, 0.0, 0.5));
    client.pump_until(|c| !has(c, "base:full_house"));
    client.tp(feet.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
    // Four crops by the door are a kitchen garden.
    let mut lines = vec![format!("/fill {} {} {} {} {} {} farmland", feet.x - 1, feet.y - 1, feet.z + 2, feet.x + 2, feet.y - 1, feet.z + 2)];
    for dx in -1..=2 {
        lines.push(format!("/setblock {} {} {} wheat_crop", feet.x + dx, feet.y, feet.z + 2));
    }
    for line in lines {
        client.ok(&line);
    }
    client.pump_until(|c| has(c, "base:kitchen_garden"));
}

#[test]
fn home_says_where_home_is_and_what_the_next_step_needs() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    // No bed, no hearth: no home.
    let lines = client.command("/home");
    assert!(lines.iter().any(|(kind, text)| *kind != ChatKind::Info && text.contains("no home yet")), "{lines:?}");
    // A bag and a torch, slept in, are a camp; the hint names the hearth as the next step.
    let bag = feet + IVec3::new(2, 0, 0);
    for line in [format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z), format!("/setblock {} {} {} torch", feet.x + 3, feet.y, feet.z)] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag") && c.world.block(feet + IVec3::new(3, 0, 0)) == c.content.blocks.id("base:torch"));
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    // Wait out the sleep's own lines and the step's announcement before asking.
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Camp:")));
    let said = client.ok("/home");
    assert!(said.contains(&format!("Your home is at {}, {}, {}", bag.x, bag.y + 1, bag.z)), "{said}");
    assert!(said.contains("Camp reached. Next, Hearthside: it needs hearth 0/1"), "{said}");
    // A hearthstone lights it: the ladder moves on and the next step names a roof, walls and the rest.
    let hearth = feet + IVec3::new(4, 0, 0);
    client.ok(&format!("/setblock {} {} {} hearthstone", hearth.x, hearth.y, hearth.z));
    client.pump_until(|c| c.world.block(hearth) == c.content.blocks.id("base:hearthstone"));
    client.chat.clear();
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Hearthside:")));
    let said = client.ok("/home");
    assert!(said.contains("Hearthside reached. Next, Snug: it needs a roof overhead, draughty or better walls and roof all about "), "{said}");
    assert!(said.contains("%/90%, light 1/2"), "{said}");
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
        "/fill ~3 ~ ~-1 ~5 ~ ~1 tempered_kiln_bricks",
        "/setblock ~4 ~ ~ crucible",
        "/setblock ~3 ~1 ~-1 tempered_kiln_bricks",
        "/setblock ~5 ~1 ~-1 tempered_kiln_bricks",
        "/setblock ~3 ~1 ~1 tempered_kiln_bricks",
        "/setblock ~5 ~1 ~1 tempered_kiln_bricks",
        "/give copper_dust 2",
        "/skill engineering 20",
    ] {
        client.ok(line);
    }
    let blocks = client.content.blocks.clone();
    let (crucible, dust, ingot) = (blocks.expect("base:crucible"), blocks.expect("base:copper_dust"), blocks.expect("base:copper_ingot"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(core) == Some(crucible) && c.world.block(core + IVec3::new(1, 1, 1)) == c.content.blocks.id("base:tempered_kiln_bricks") && count(c, dust) == 2);
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.clear();
    client.ok("/give charcoal 32");
    let charcoal = blocks.expect("base:charcoal");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == charcoal && s.count == 32));
    client.chat.clear();
    client.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("Crucible Kiln is lit")));
    client.ok("/give copper_dust 2");
    client.pump_until(|c| count(c, dust) == 2);
    client.propose("base:cook", ActionTarget::Block { block: core, face: Face::PosY });
    client.pump_until(|c| count(c, ingot) == 1 && count(c, dust) == 1);
}

#[test]
fn gravel_sometimes_breaks_to_flint_instead() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    // Draws by a pinned key: on a new world's random one, sixty breaks at one in eight gave
    // no flint about once in 3,300 runs (one key in 4,000 tried), and a loaded run once did.
    let mut client = Client::join_with(ServerConfig { view_distance: 2, draw_key: Some(GRAVEL_DRAWS), ..Default::default() });
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    for line in ["/clear", "/give iron_shovel 1"] {
        client.ok(line);
    }
    let (gravel, flint) = (client.content.blocks.expect("base:gravel"), client.content.blocks.expect("base:flint"));
    let shovel = client.content.blocks.expect("base:iron_shovel");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == shovel));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    // One break in eight gives flint: in sixty, all but certainly some.
    let seen = |c: &Client, item: BlockId| c.items.values().filter(|i| i.stack.item == item).map(|i| i.stack.count as u32).sum::<u32>() + c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    let mut flints = 0;
    for _ in 0..60 {
        client.ok("/setblock ~ ~-1 ~ gravel");
        client.pump_until(|c| c.world.block(below) == Some(gravel));
        let before = seen(&client, gravel) + seen(&client, flint);
        client.propose("core:mine", ActionTarget::Block { block: below, face: Face::PosY });
        client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
        // The break's drop, its gravel or flint instead, seen loose or in the pack before
        // the gravel is set again where it fell.
        client.pump_until(|c| seen(c, gravel) + seen(c, flint) > before);
        flints = seen(&client, flint);
        if flints > 0 {
            break;
        }
    }
    assert!(flints > 0, "sixty gravel broke without a flint");
}

/// The draw key `gravel_sometimes_breaks_to_flint_instead` pins.
const GRAVEL_DRAWS: u64 = 0x9a5e;

/// A standing log refuses a bare hand: the client's break it claims finished is undone, the
/// log put back. Held, a flint axe fells it, and the log comes to hand.
#[test]
fn a_bare_hand_fells_no_tree_and_a_flint_axe_does() {
    let mut client = Client::join();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    client.ok("/clear");
    client.ok("/setblock ~ ~-1 ~ oak_log");
    let (log, axe) = (client.content.blocks.expect("base:oak_log"), client.content.blocks.expect("base:flint_axe"));
    client.pump_until(|c| c.world.block(below) == Some(log));
    let target = ActionTarget::Block { block: below, face: Face::PosY };
    client.propose("core:mine", target);
    client.settle();
    client.world.set_block(below, BlockId::AIR);
    client.connection.send(ClientMessage::Finished { target });
    client.pump_until(|c| c.world.block(below) == Some(log));
    assert!(client.inventory.slots.iter().flatten().all(|s| s.item != log), "a bare hand took a log");
    client.ok("/give flint_axe 1");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == axe));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("core:mine", target);
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR));
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == log));
}

/// Tall grass gives twine one time in five to a knife (a stone one, a quarter), and never
/// by hand: the first upgrade on twine, which otherwise waits on wild wheat.
#[test]
fn a_knife_cuts_twine_from_tall_grass() {
    let mut client = Client::join();
    let cell = client.position.floor().as_ivec3() + IVec3::X;
    client.ok("/clear");
    let (grass, twine) = (client.content.blocks.expect("base:tall_grass"), client.content.blocks.expect("base:twine"));
    let twines = |c: &Client| c.items.values().filter(|i| i.stack.item == twine).count() as u32 + c.inventory.slots.iter().flatten().filter(|s| s.item == twine).map(|s| s.count as u32).sum::<u32>();
    let cut = |client: &mut Client| {
        client.ok("/setblock ~1 ~ ~ tall_grass");
        client.pump_until(|c| c.world.block(cell) == Some(grass));
        client.propose("core:mine", ActionTarget::Block { block: cell, face: Face::NegX });
        client.pump_until(|c| c.world.block(cell) == Some(BlockId::AIR));
        // The drop comes just after the block goes.
        client.after_server_ticks(2);
        twines(client)
    };
    for _ in 0..30 {
        assert_eq!(cut(&mut client), 0, "bare hands cut twine from tall grass");
    }
    client.ok("/give stone_knife 1");
    let knife = client.content.blocks.expect("base:stone_knife");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == knife));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    // A quarter of the time: in sixty, all but certainly some.
    let mut found = 0;
    for _ in 0..60 {
        found = cut(&mut client);
        if found > 0 {
            break;
        }
    }
    assert!(found > 0, "sixty tall grasses cut with a stone knife gave no twine");
}

/// A stone hoe cutting a ripe wheat in a patch of nine reaps all nine, each giving its
/// harvest; a bare hand or a knife cuts only the one.
#[test]
fn a_stone_hoe_reaps_the_ripe_crops_about_one() {
    let mut client = Client::join();
    let centre = client.position.floor().as_ivec3() + IVec3::new(2, 0, 0);
    let (ripe, wheat, seeds) = (client.content.blocks.expect("base:wheat_crop"), client.content.blocks.expect("base:wheat"), client.content.blocks.expect("base:wheat_seeds"));
    let cells: Vec<IVec3> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| centre + IVec3::new(dx, 0, dz))).collect();
    let got = |c: &Client| -> u32 {
        let lying: u32 = c.items.values().filter(|i| i.stack.item == wheat || i.stack.item == seeds).map(|i| i.stack.count as u32).sum();
        lying + c.inventory.slots.iter().flatten().filter(|s| s.item == wheat || s.item == seeds).map(|s| s.count as u32).sum::<u32>()
    };
    for (hoe, reaped) in [("bare hand", 1), ("flint_knife", 1), ("stone_hoe", 9)] {
        // The last round's harvest the player would take or draw in comes in once its pickup
        // delay is out, which on a loaded server lands after the /clear and fills a slot the
        // wait below wants empty: let it come in first, by the server's own rule. What came
        // to rest beyond the magnet stays where it is. One still sliding under 0.1 a second
        // goes about 0.012 further, so it counts as settled only that far clear of the magnet.
        client.pump_until(|c| {
            c.items.values().all(|i| {
                let item = voxmar_engine::dropped::Loose::new(i.position, i.velocity);
                let away = voxmar_engine::dropped::middle(c.position).distance(item.centre());
                let settled = i.velocity == Vec3::ZERO || (i.velocity.length() < 0.1 && away >= voxmar_engine::dropped::MAGNET + 0.02) || away >= voxmar_engine::dropped::CLAIM;
                settled && item.pull(c.position) == voxmar_engine::dropped::Pull::Left
            })
        });
        client.ok("/clear");
        client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
        // The tool in whatever slot it lands in, and a bare hand on an empty slot: the last
        // round's harvest lying about may be picked up into slot 0 after the clear. The give
        // is sent again until the server's word of it comes.
        let slot = if hoe == "bare hand" {
            client.pump_ticks(TICKS_PER_SECOND as u64, |_| false);
            client.inventory.slots.iter().position(Option::is_none).expect("an empty slot for a bare hand")
        } else {
            let item = client.content.blocks.expect(&format!("base:{hoe}"));
            let at = |c: &Client| c.inventory.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.item == item));
            for _ in 0..10 {
                client.ok(&format!("/give {hoe} 1"));
                client.pump_ticks(5 * TICKS_PER_SECOND as u64, |c| at(c).is_some());
                if at(&client).is_some() {
                    break;
                }
            }
            at(&client).unwrap_or_else(|| panic!("ten gives of a {hoe} never reached the pack"))
        };
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: slot as u8, flying: false }));
        client.ok(&format!("/fill {} {} {} {} {} {} farmland", centre.x - 1, centre.y - 1, centre.z - 1, centre.x + 1, centre.y - 1, centre.z + 1));
        client.ok(&format!("/fill {} {} {} {} {} {} wheat_crop", centre.x - 1, centre.y, centre.z - 1, centre.x + 1, centre.y, centre.z + 1));
        client.pump_until(|c| cells.iter().all(|&cell| c.world.block(cell) == Some(ripe)));
        // Cut again until it lands, by the server's clock: on a loaded machine the first
        // cut was once never answered (the Input choosing the tool racing the /give). A cut
        // at the air a landed one left does nothing.
        for _ in 0..10 {
            client.propose("core:mine", ActionTarget::Block { block: centre, face: Face::PosY });
            client.pump_ticks(2 * TICKS_PER_SECOND as u64, |c| c.world.block(centre) == Some(BlockId::AIR));
            if client.world.block(centre) == Some(BlockId::AIR) {
                break;
            }
        }
        assert_eq!(client.world.block(centre), Some(BlockId::AIR), "ten cuts with a {hoe} never took the middle wheat");
        // The rest go in the same tick as the one cut.
        client.after_server_ticks(2);
        let gone = cells.iter().filter(|&&cell| client.world.block(cell) == Some(BlockId::AIR)).count();
        assert_eq!(gone, reaped, "a {hoe} cut {gone} of the nine ripe wheat");
        // Each foraged (an unsown ripe wheat): at least a seed from every one.
        client.after_server_ticks(4);
        assert!(got(&client) >= reaped as u32, "{reaped} wheat cut with a {hoe} gave {} seeds and wheat", got(&client));
    }
    // Fruit on trees is no field: a stone hoe on one ripe apple leaf leaves the canopy about it.
    let apple = client.content.blocks.expect("base:apple_leaves");
    client.ok(&format!("/fill {} {} {} {} {} {} apple_leaves", centre.x - 1, centre.y, centre.z - 1, centre.x + 1, centre.y, centre.z + 1));
    client.pump_until(|c| cells.iter().all(|&cell| c.world.block(cell) == Some(apple)));
    for _ in 0..10 {
        client.propose("core:mine", ActionTarget::Block { block: centre, face: Face::PosY });
        client.pump_ticks(2 * TICKS_PER_SECOND as u64, |c| c.world.block(centre) != Some(apple));
        if client.world.block(centre) != Some(apple) {
            break;
        }
    }
    assert_ne!(client.world.block(centre), Some(apple), "ten cuts with a stone hoe never took the ripe apple leaf");
    client.after_server_ticks(2);
    let left = cells.iter().filter(|&&cell| client.world.block(cell) == Some(apple)).count();
    assert_eq!(left, 8, "a stone hoe on one ripe apple leaf took {} of the leaves about it", 8 - left);
}

/// Plain stone breaks under a flint pickaxe at Mining 1 into two loose stones and no
/// cobblestone (no level bonus, and flint finds no extra); four loose stones in the hand's
/// grid knock together into two cobblestone, and a cobblestone breaks back to one.
#[test]
fn stone_breaks_into_two_loose_stones_and_four_make_two_cobblestone() {
    let mut client = Client::join();
    let below = client.position.floor().as_ivec3() - IVec3::Y;
    for line in ["/clear", "/skill mining 1", "/give flint_pickaxe 1"] {
        client.ok(line);
    }
    let id = |c: &Client, name: &str| c.content.blocks.expect(name);
    let (stone, loose, cobble, pick) = (id(&client, "base:stone"), id(&client, "base:loose_stone"), id(&client, "base:cobblestone"), id(&client, "base:flint_pickaxe"));
    // Carried, or lying loose on the way to the pack.
    let has = |c: &Client, item: BlockId| -> u32 { c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>() + c.items.values().filter(|i| i.stack.item == item).map(|i| i.stack.count as u32).sum::<u32>() };
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == pick));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    let target = ActionTarget::Block { block: below, face: Face::PosY };
    client.ok("/setblock ~ ~-1 ~ stone");
    client.pump_until(|c| c.world.block(below) == Some(stone));
    client.propose("core:mine", target);
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR) && (has(c, loose) >= 2 || has(c, cobble) >= 1));
    client.after_server_ticks(4);
    assert_eq!((has(&client, loose), has(&client, cobble)), (2, 0), "plain stone breaks to two loose stones, no cobblestone");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == loose && s.count == 2));

    // Two more, and four laid one to a cell of the two by two grid.
    client.ok("/give loose_stone 2");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == loose && s.count == 4));
    let from = (0..voxmar_engine::inventory::SLOTS).find(|&s| client.inventory.get(s).is_some_and(|x| x.item == loose)).unwrap();
    for cell in [0, 1, 3, 4] {
        client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (voxmar_engine::inventory::CRAFT + cell) as u8, count: 1 });
    }
    client.pump_until(|c| c.inventory.craft.iter().flatten().count() == 4);
    client.propose("core:craft", ActionTarget::Slot(0));
    client.pump_until(|c| has(c, cobble) == 2);
    assert_eq!(has(&client, loose), 0, "all four went into the two cobblestone");

    // A cobblestone set down breaks back to itself, one.
    client.ok("/setblock ~ ~-1 ~ cobblestone");
    client.pump_until(|c| c.world.block(below) == Some(cobble));
    client.propose("core:mine", target);
    client.pump_until(|c| c.world.block(below) == Some(BlockId::AIR) && has(c, cobble) >= 3);
    client.after_server_ticks(4);
    assert_eq!((has(&client, cobble), has(&client, loose)), (3, 0), "a cobblestone breaks to one cobblestone");
}

// Seed 3 has a village at (368, 135, 784), 421 blocks from spawn (432, 134, 368).
// Keeping the locate radius small avoids planning hundreds of squares on the server.
const VILLAGE_SEED: u64 = 3;

/// The settlement tests' view distance, in columns.
const SETTLEMENT_VIEW: i32 = 2;

fn settlement_client() -> Client {
    Client::join_with(ServerConfig { seed: VILLAGE_SEED, view_distance: SETTLEMENT_VIEW, ..Default::default() })
}

/// Goes to the nearby test village within 2000 blocks at `time`, as a creative admin:
/// the `/tp` coordinates of its middle. An empty `size` selects the same village.
fn to_settlement(client: &mut Client, size: &str, time: &str) -> String {
    let size = if size.is_empty() { "village" } else { size };
    let found = client.ok(&format!("/locate settlement {size} 2000"));
    let tp = found.split("/tp ").nth(1).expect("a way there").split(" goes").next().unwrap().to_owned();
    client.ok(&format!("/time set {time}"));
    client.ok(&format!("/tp {tp}"));
    tp
}

/// The settlement people in sight.
fn villagers(c: &Client) -> Vec<voxmar_protocol::CreatureView> {
    c.creatures.values().filter(|v| c.content.creatures.get(v.species).def.kind == voxmar_engine::creature::Kind::Villager).cloned().collect()
}

fn species_name(c: &Client, v: &voxmar_protocol::CreatureView) -> String {
    c.content.creatures.get(v.species).def.name.clone()
}

/// Strikes creature `id` until `done`, following it when it runs, for at most `seconds`
/// of the server's clock.
fn strike_until(client: &mut Client, id: u32, seconds: u64, done: impl Fn(&Client) -> bool) {
    let until = client.server_tick + seconds * TICKS_PER_SECOND as u64;
    while !done(client) && client.server_tick < until {
        if let Some(v) = client.creatures.get(&id) {
            if v.position.distance(client.position) > 2.5 {
                // It ran: follow it, as a player chasing it would.
                let at = v.position;
                client.ok(&format!("/tp {} {} {}", at.x + 1.0, at.y, at.z));
            }
            client.propose("core:attack", ActionTarget::Creature(id));
        }
        client.pump_ticks(TICKS_PER_SECOND as u64 / 2, |_| false);
    }
}

/// The seeds of each species of villager asleep in sight (those awake at night, the
/// watch, come and go).
fn census(c: &Client) -> std::collections::BTreeMap<String, Vec<u32>> {
    let mut out: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
    for v in villagers(c).into_iter().filter(|v| v.pose == voxmar_engine::creature::brain::Pose::Resting) {
        out.entry(species_name(c, &v)).or_default().push(v.seed);
    }
    out.values_mut().for_each(|s| s.sort_unstable());
    out
}

/// A settlement's people are about when a player comes: asleep at home at night (resting,
/// inside a building), up and walking to their day's places in the morning, and gone when
/// the player leaves.
#[test]
fn villagers_sleep_at_night_and_go_about_their_day() {
    use voxmar_engine::creature::brain::Pose;
    let mut client = settlement_client();
    client.admin = true;
    to_settlement(&mut client, "", "midnight");
    client.pump_until(|c| villagers(c).len() >= 3);
    // In bed: most are resting at midnight (a night watchman is up).
    let asleep = |c: &Client| villagers(c).iter().filter(|v| v.pose == Pose::Resting).count();
    client.pump_until(|c| asleep(c) * 3 >= villagers(c).len() * 2);
    let world_content = voxmar_worldgen::WorldContent::load(&client.content).unwrap();
    // The server's own generator (`Overworld::shared`), the village round it already planned.
    let overworld = voxmar_worldgen::Overworld::shared(VILLAGE_SEED, &client.content, &world_content);
    let towns = overworld.settlements_near(client.position.floor().as_ivec3().xz(), 256);
    let mut resting: Vec<_> = villagers(&client).into_iter().filter(|v| v.pose == Pose::Resting)
        .map(|v| (v.seed, species_name(&client, &v), v.position)).collect();
    resting.sort_by_key(|v| v.0);
    eprintln!("VILLAGER resting residents: {resting:?}");
    for &(seed, ref species, position) in &resting {
        let cell = position.floor().as_ivec3();
        let home = towns.iter().flat_map(|t| &t.buildings)
            .find(|b| b.floor + 1 == cell.y && b.room.contains(&cell.xz()))
            .unwrap_or_else(|| panic!("{species} seed{seed} resting outside a room at {position:?}"));
        eprintln!("VILLAGER resting home seed{seed}: {home:?}");
    }
    let night: std::collections::HashMap<u32, Vec3> = villagers(&client).iter().map(|v| (v.id, v.position)).collect();
    // Morning: up and out, a minute of the server's clock on.
    client.ok("/time set day");
    let morning_done = |c: &Client| {
        let folk = villagers(c);
        let moved = folk.iter().filter(|v| night.get(&v.id).is_some_and(|p| p.distance(v.position) > 4.0)).count();
        moved >= 2 && asleep(c) * 2 < folk.len()
    };
    client.pump_ticks(60 * TICKS_PER_SECOND as u64, morning_done);
    let moved = villagers(&client).iter().filter(|v| night.get(&v.id).is_some_and(|p| p.distance(v.position) > 4.0)).count();
    assert!(moved >= 2, "only {moved} of {} people left their beds in the morning", night.len());
    let (awake, all) = (villagers(&client).len() - asleep(&client), villagers(&client).len());
    assert!(asleep(&client) * 2 < all, "most still asleep a minute into the day: {awake} of {all} up");
    // Far away, they go.
    let far = client.position + Vec3::new(600.0, 0.0, 600.0);
    client.ok(&format!("/tp {} {}", far.x as i32, far.z as i32));
    client.pump_until(|c| villagers(c).is_empty());
}

/// A settlement let go and come back to holds its people once each: at midnight, from the
/// same spot, the same people (by species and seed) as before, none twice.
#[test]
fn a_settlement_come_back_to_holds_its_people_once() {
    let mut client = settlement_client();
    let tp = to_settlement(&mut client, "", "midnight");
    client.ok("/time freeze");
    // A person comes out only onto land the server holds, so on a busy machine they kept
    // coming as the land came in, past twenty looks: the looks begin once it is all in.
    client.land_within(SETTLEMENT_VIEW);
    // Settled: the same people two looks running. On a loaded machine the server can still
    // be bringing them in after three looks, so it looks until they hold still.
    let mut before = census(&client);
    let mut settled = false;
    for _ in 0..20 {
        client.pump_ticks(3 * TICKS_PER_SECOND as u64, |_| false);
        let now = census(&client);
        settled = now == before && !now.is_empty();
        if settled {
            break;
        }
        before = now;
    }
    assert!(settled, "the people never held still: {before:?}");
    let seen_before: Vec<String> = villagers(&client).iter().map(|v| format!("{} {} {:?} at {:.1}", species_name(&client, v), v.seed, v.pose, v.position)).collect();
    // Away past `release` (well inside `forget`): the server lets them go.
    let far = client.position + Vec3::new(400.0, 0.0, 0.0);
    client.ok(&format!("/tp {} {}", far.x as i32, far.z as i32));
    client.pump_until(|c| villagers(c).is_empty());
    client.pump_ticks(5 * TICKS_PER_SECOND as u64, |_| false);
    client.ok(&format!("/tp {tp}"));
    client.pump_until(|c| census(c) == before || villagers(c).len() > before.values().map(Vec::len).sum());
    for _ in 0..2 {
        client.pump_ticks(3 * TICKS_PER_SECOND as u64, |_| false);
        let after = census(&client);
        for (species, seeds) in &after {
            let mut once = seeds.clone();
            once.dedup();
            assert_eq!(&once, seeds, "{species} twice over: {seeds:?}");
        }
        let seen_after: Vec<String> = villagers(&client).iter().map(|v| format!("{} {} {:?} at {:.1}", species_name(&client, v), v.seed, v.pose, v.position)).collect();
        assert_eq!(after, before, "the people came back other than they were: before {seen_before:?}, after {seen_after:?}");
    }
}

/// A strong ordinary weapon makes lifecycle checks exercise one death quickly; those
/// checks are about loot and persistence, not levelling a player through combat.
fn arm_for_villager_death(client: &mut Client) {
    client.clear();
    client.ok("/skill attack 60");
    client.ok("/give iron_sword 1");
    let sword = client.content.blocks.expect("base:iron_sword");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == sword));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
}

/// Killing one of a settlement's people pays nothing: no drops, no experience, nothing
/// in the journal.
#[test]
fn a_villager_killed_drops_nothing_and_teaches_nothing() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let mut client = settlement_client();
    to_settlement(&mut client, "", "noon");
    let folk = |c: &Client| villagers(c).into_iter().find(|v| species_name(c, v) != "base:guard");
    client.pump_until(|c| folk(c).is_some());
    let victim = folk(&client).unwrap();
    let name = species_name(&client, &victim);
    arm_for_villager_death(&mut client);
    let (earned, items) = (client.earned.len(), client.items.len());
    strike_until(&mut client, victim.id, 20, |c| !c.creatures.contains_key(&victim.id));
    assert!(client.deaths >= 1 && !client.creatures.contains_key(&victim.id), "the {name} did not die");
    assert!(client.blows > 0 && client.damage.iter().any(|&d| d > 0), "no blow landed");
    // Past when a body's loot comes out.
    client.pump_ticks(5 * TICKS_PER_SECOND as u64, |_| false);
    assert_eq!(client.items.len(), items, "the {name} dropped something");
    assert_eq!(client.earned.len(), earned, "killing the {name} taught {:?}", &client.earned[earned..]);
    assert!(client.journal.entries().all(|e| e.name != name), "the {name} went in the journal");
}

/// Who died stays dead: a restart brings no one back; the newcomer who takes their place comes `newcomer`
/// days on, with looks of their own.
#[test]
fn a_dead_villager_stays_dead_across_a_restart_until_a_newcomer_comes() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let dir = world_dir("villagers");
    let config = || ServerConfig { seed: VILLAGE_SEED, view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let mut client = Client::join_with(config());
    let tp = to_settlement(&mut client, "", "midnight");
    client.ok("/time freeze");
    // Someone asleep at home, and who else is there.
    use voxmar_engine::creature::brain::Pose;
    let sleeper = |c: &Client| villagers(c).into_iter().find(|v| v.pose == Pose::Resting && species_name(c, v) != "base:guard");
    client.pump_until(|c| sleeper(c).is_some());
    let victim = sleeper(&client).unwrap();
    let (name, bed) = (species_name(&client, &victim), victim.position);
    eprintln!("VILLAGER lifecycle: {name} seed {} resting at {bed:?}", victim.seed);
    // Where they lie: a block to one side may be a wall.
    client.ok(&format!("/tp {} {} {}", bed.x, bed.y, bed.z));
    // Everyone of their kind in sight, once all have come out: a person comes out only onto
    // land the server holds, and as its search allows, so the look waits until the server
    // says everyone due out near the player is (`/creatures people`), then until the
    // client holds every one of them the server tells it of.
    let kind = |c: &Client| -> std::collections::BTreeSet<u32> { villagers(c).into_iter().filter(|v| species_name(c, v) == name).map(|v| v.seed).collect() };
    let settle = |client: &mut Client| -> std::collections::BTreeSet<u32> {
        use voxmar_engine::lang::Words;
        client.land_within(SETTLEMENT_VIEW);
        let mut said = String::new();
        for _ in 0..60 {
            said = client.ok("/creatures people");
            // Read from the server's words, not from any language's rendering of them, and
            // from its answer, not whatever else came first.
            let answer = client.said.iter().find_map(|w| match w {
                Words::Key(key, args) if key == "command.creatures.people" => Some(args.clone()),
                _ => None,
            });
            let Some(args) = answer else { panic!("/creatures people: {said}") };
            let arg = |name: &str| args.iter().find(|(n, _)| n == name).map(|(_, w)| w.clone());
            let number = |name: &str| match arg(name) {
                Some(Words::Text(n)) => n.parse::<usize>().ok(),
                _ => None,
            };
            let seen: Vec<u32> = match arg("seen") {
                Some(Words::List(ids)) => ids.iter().filter_map(|w| if let Words::Text(id) = w { id.parse().ok() } else { None }).collect(),
                _ => panic!("/creatures people named no one in sight: {said}"),
            };
            if let (Some(out), Some(due)) = (number("out"), number("due"))
                && out == due
                && due > 0
            {
                // Everyone the server named, held; someone who walked out of sight since
                // is asked about again.
                client.pump_ticks(2 * TICKS_PER_SECOND as u64, |c| seen.iter().all(|id| c.creatures.contains_key(id)));
                if seen.iter().all(|id| client.creatures.contains_key(id)) {
                    return kind(client);
                }
                continue;
            }
            client.pump_ticks(TICKS_PER_SECOND as u64 / 2, |_| false);
        }
        panic!("the settlement's people never all came out and were all held: {said}");
    };
    // A restart first, so those in sight are the ones a restart brings out where the player
    // stands. One who came out as the player crossed the settlement stays out while near,
    // and was counted here, though the restart's people due out near the player left them
    // in (one asleep 87 blocks off: 6 of 7 back).
    drop(client);
    let mut client = Client::join_with(config());
    client.ok(&format!("/tp {} {} {}", bed.x, bed.y, bed.z));
    let housemates = settle(&mut client);
    let victim = villagers(&client).into_iter().find(|v| v.seed == victim.seed).unwrap_or(victim);
    assert!(housemates.contains(&victim.seed), "the {name} asleep was not among those in sight");
    arm_for_villager_death(&mut client);
    strike_until(&mut client, victim.id, 20, |c| !c.creatures.contains_key(&victim.id));
    assert!(!client.creatures.contains_key(&victim.id), "the {name} did not die");
    // Waiting for any of the settlement's people, not for those at the bed: the dead one
    // may have lived alone.
    let gone = |c: &Client| villagers(c).iter().all(|v| v.seed != victim.seed);
    // A restart (dropping the server saves the world), from the bed as the first was: the
    // kill may have moved the player, and a crossing on the way could bring in a newcomer.
    client.ok(&format!("/tp {} {} {}", bed.x, bed.y, bed.z));
    drop(client);
    assert!(dir.join("villagers.ron").is_file());
    let mut client = Client::join_with(config());
    // From the same spot, settled the same way: a {name} never seen before is a newcomer.
    client.ok(&format!("/tp {} {} {}", bed.x, bed.y, bed.z));
    settle(&mut client);
    // Everyone seen before but the dead one back, before anyone is judged a stranger: a
    // look at no one would find no stranger.
    let back: std::collections::BTreeSet<u32> = housemates.iter().copied().filter(|&s| s != victim.seed).collect();
    client.pump_ticks(20 * TICKS_PER_SECOND as u64, |c| back.is_subset(&kind(c)));
    let after = kind(&client);
    assert!(back.is_subset(&after), "not everyone came back after the restart: {after:?} of {back:?}");
    assert!(gone(&client), "the {name} came back with a restart");
    let strangers: Vec<u32> = after.iter().copied().filter(|s| !housemates.contains(s)).collect();
    assert!(strangers.is_empty(), "a newcomer came the same night: {strangers:?} beside {housemates:?}");
    // `newcomer` days on (the base game's five), someone new lives there.
    let newcomer = |c: &Client| villagers(c).into_iter().any(|v| species_name(c, &v) == name && v.position.distance(bed) < 16.0 && !housemates.contains(&v.seed) && v.seed != victim.seed);
    client.ok("/time add 6d");
    client.ok("/time set midnight");
    client.pump_ticks(6 * TICKS_PER_SECOND as u64, |c| newcomer(c));
    assert!(newcomer(&client), "no newcomer six days on");
    assert!(gone(&client), "the dead {name} came back");
    drop(client);
    let _ = tp;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Struck by a player, a villager runs, and the village's guards come for whoever did it,
/// and for no one else: a player nearby who struck no one goes unhurt.
#[test]
fn guards_come_for_a_player_who_strikes_a_villager() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let mut client = settlement_client();
    let mut bystander = client.another("bystander");
    to_settlement(&mut client, "village", "noon");
    // Guards leave creative players be, as every creature does.
    client.ok("/gamemode survival");
    client.ok("/gamemode survival bystander");
    // This checks whom the alarm calls the guards to, not a novice's fist: one that lands
    // a third of its blows, at part strength when the clock bunches them under load, now
    // and then hurt no one in all of its ten seconds. A practised fist lands nearly every
    // blow and kills no grown villager in one (its max hit 8 against their 12; a sword
    // did).
    client.ok("/skill attack 60");
    let guards = |c: &Client| -> Vec<voxmar_protocol::CreatureView> { villagers(c).into_iter().filter(|v| species_name(c, v) == "base:guard").collect() };
    // The closest pair, with a stable tie-break: hash-map order used to pick a distant
    // worker whose guard had no way through the intervening buildings.
    // Grown folk only: one the practised fist (max hit 8) cannot kill in one blow, so it is
    // seen hurt.
    let pick = |c: &Client| villagers(c).into_iter().filter(|v| species_name(c, v) != "base:guard" && c.content.creatures.get(v.species).def.health > 8)
        .flat_map(|v| guards(c).into_iter().map(move |g| { let d = g.position.distance(v.position); (v.clone(), g, d) }))
        .filter(|(_, _, d)| *d < 30.0)
        .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.id.cmp(&b.0.id)).then(a.1.id.cmp(&b.1.id)));
    client.pump_until(|c| pick(c).is_some());
    let (villager, guard, _) = pick(&client).unwrap();
    // Both where the villager stands, a cell anyone can stand in: a block to one side may
    // be a wall, and guards cannot reach a player inside one (they give that chase up).
    let at = villager.position;
    client.ok(&format!("/tp {} {} {}", at.x, at.y, at.z));
    client.ok(&format!("/tp bystander {} {} {}", at.x, at.y, at.z));
    bystander.pump_until(|c| c.teleported.is_some() && c.health > 0);
    let (blows_before, refused_before, started) = (client.damage.len(), client.refused.len(), client.server_tick);
    let landed = move |c: &Client| c.creatures.get(&villager.id).is_some_and(|v| v.health < villager.health);
    strike_until(&mut client, villager.id, 10, landed);
    let now = client.creatures.get(&villager.id).map(|v| format!("{} health {} at {:.1} {:?} ({:.1} off)", species_name(&client, v), v.health, v.position, v.pose, v.position.distance(client.position)));
    assert!(
        landed(&client),
        "no blow landed on the villager {} in {} ticks: now {now:?}; our blows {:?}; refused {:?}; the player at {:.1}",
        villager.id,
        client.server_tick - started,
        &client.damage[blows_before..],
        &client.refused[refused_before..],
        client.position
    );
    // Step beside the called guard without striking it. This checks whom the alarm
    // selects; the blocked-route and give-up test checks an unreachable quarry.
    let at = client.creatures[&guard.id].position;
    client.ok(&format!("/tp {} {} {}", at.x, at.y, at.z));
    client.ok(&format!("/tp bystander {} {} {}", at.x, at.y, at.z));
    let hurt_before = client.hurt.len();
    let calm_before = bystander.hurt.len();
    let struck_by_a_guard = |c: &Client| c.hurt[hurt_before..].iter().any(|&(damage, _)| damage > 0) && guards(c).iter().any(|g| g.position.distance(c.position) < 3.0);
    client.pump_ticks(20 * TICKS_PER_SECOND as u64, struck_by_a_guard);
    let hit = struck_by_a_guard(&client);
    let seen: Vec<String> = guards(&client).iter().map(|g| format!("{} at {:.1} {:?} ({:.1} off)", g.id, g.position, g.pose, g.position.distance(client.position))).collect();
    assert!(hit, "no called guard struck a reachable quarry in 20 seconds: the player at {:.1}, hurt {:?}; guards {seen:?} (called {})", client.position, &client.hurt[hurt_before..], guard.id);
    // The bystander, the while and a few seconds more.
    client.pump_ticks(5 * TICKS_PER_SECOND as u64, |_| false);
    bystander.drain();
    assert!(bystander.hurt[calm_before..].iter().all(|&(damage, _)| damage == 0), "a guard struck a player who struck no one");
}

/// A guard that cannot reach the player it is after (they stand on a pillar) gives up
/// within `give_up` seconds of the server's clock, then recovers after a further quiet
/// `give_up` seconds at home. A
/// player it can reach it strikes.
#[test]
fn guards_give_up_on_a_player_they_cannot_reach() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let mut client = settlement_client();
    to_settlement(&mut client, "village", "noon");
    // This checks pursuit and recovery, which require a living, wounded guard and a
    // living quarry. A novice can die to this level-25 guard before wounding it.
    client.ok("/skill hitpoints 60");
    client.pump_until(|c| c.health >= 100);
    client.ok("/gamemode survival");
    let guard = |c: &Client| villagers(c).into_iter().filter(|v| species_name(c, v) == "base:guard")
        .min_by(|a, b| a.position.distance(c.position).total_cmp(&b.position.distance(c.position)).then(a.id.cmp(&b.id)));
    client.pump_until(|c| guard(c).is_some());
    let g = guard(&client).unwrap();
    let max = client.content.creatures.get(g.species).def.health;
    // The guard's own standing cell is reachable; a cell one block to its side may
    // be inside a wall. Once angry, it follows us rather than fleeing.
    client.ok(&format!("/tp {} {} {}", g.position.x, g.position.y, g.position.z));
    // Within reach: struck back.
    let full = client.health;
    let hurt_by_before = client.hurt_by.len();
    let damage_before = client.damage.len();
    let struck = |c: &Client| c.health < full && c.hurt_by[hurt_by_before..].iter().any(|&(id, damage)| id == g.id && damage > 0);
    strike_until(&mut client, g.id, 60, |c| struck(c) || c.health == 0);
    assert!(struck(&client), "guard {} never struck a player beside it: health {}, deaths {}, incoming {:?}", g.id, client.health, client.died, &client.hurt_by[hurt_by_before..]);
    // Improve only the injury setup after proving real retaliation. A fist at this
    // level still cannot kill a full-health guard in one blow.
    client.ok("/skill attack 60");
    let hurt = |c: &Client| c.creatures.get(&g.id).is_some_and(|v| v.health > 0 && v.health < max);
    strike_until(&mut client, g.id, 30, |c| hurt(c) || c.health == 0 || !c.creatures.contains_key(&g.id));
    assert!(client.health > 0 && client.died == 0, "quarry died before the pillar: health {}, deaths {}, blows {:?}, incoming {:?}", client.health, client.died, &client.damage[damage_before..], &client.hurt_by[hurt_by_before..]);
    assert!(hurt(&client), "guard {} was not left alive and wounded: target {:?}, health {}, blows {:?}", g.id, client.creatures.get(&g.id).map(|v| (v.position, v.health)), client.health, &client.damage[damage_before..]);
    // Up a pillar four blocks high, out of its reach.
    client.ok("/fill ~ ~ ~ ~ ~3 ~ stone");
    client.ok("/tp ~ ~4 ~");
    let health = client.health;
    let hurt_before = client.hurt.len();
    let started = client.server_tick;
    let give_up = 8; // people.live.give_up in settlements.ron
    client.pump_ticks((give_up * 2 + 6) * TICKS_PER_SECOND as u64, |c| c.creatures.get(&g.id).is_some_and(|v| v.health == max));
    let healed = client.creatures.get(&g.id).is_some_and(|v| v.health == max);
    assert!(healed, "the guard kept after a player it cannot reach for {} ticks", client.server_tick - started);
    assert!(client.server_tick - started >= (give_up - 1) * TICKS_PER_SECOND as u64, "the guard gave up before trying for {give_up} s");
    assert!(client.hurt[hurt_before..].iter().all(|&(damage, _)| damage == 0), "struck up the pillar: {:?}", &client.hurt[hurt_before..]);
    assert!(client.health >= health, "lost health up the pillar");
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

/// The spawn kit's loose flint the client sees within twelve blocks of where it stands.
/// The loose flint within the kit's reach of the spawn (six blocks), where the land alone
/// seldom has four.
fn kit_flint(c: &Client) -> Vec<IVec3> {
    kit_pieces(c, 6, "base:loose_flint")
}

/// The cells of `piece` within `reach` blocks across of the player's feet.
fn kit_pieces(c: &Client, reach: i32, piece: &str) -> Vec<IVec3> {
    let piece = c.content.blocks.id(piece);
    let feet = c.position.floor().as_ivec3();
    (-reach..=reach).flat_map(|dx| (-reach..=reach).flat_map(move |dz| (-32..=8).map(move |dy| feet + IVec3::new(dx, dy, dz)))).filter(|&p| c.world.block(p) == piece).collect()
}

/// An older save whose player built over the spawn (set down by command, so nothing marks it
/// placed: a dirt floor, a plank roof four up on the east, and a hall's roof eighteen up on the
/// west, above where the kit looks for ground) gets its kit outside the build, never in it.
#[test]
fn the_spawn_kit_keeps_out_of_a_players_build() {
    let dir = world_dir("spawn-kit-build");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let joined = || {
        let mut c = Client::join_with(config());
        let feet = c.position.floor().as_ivec3();
        c.pump_until(|c| (-1..=1).all(|dx| (-1..=1).all(|dz| c.world.block(feet + IVec3::new(dx * 12, 0, dz * 12)).is_some())));
        c
    };
    let mut client = joined();
    // Cleared of the kit a new world gets, then built over: everything within eight across.
    for piece in ["base:loose_flint", "base:dead_log"] {
        for p in kit_pieces(&client, 12, piece) {
            client.ok(&format!("/setblock {} {} {} air", p.x, p.y, p.z));
        }
    }
    let feet = client.position.floor().as_ivec3();
    let (lo, hi) = (feet - IVec3::new(8, 0, 8), feet + IVec3::new(8, 0, 8));
    client.ok(&format!("/fill {} {} {} {} {} {} oak_planks", feet.x, feet.y + 4, lo.z, hi.x, feet.y + 4, hi.z));
    client.ok(&format!("/fill {} {} {} {} {} {} oak_planks", lo.x, feet.y + 18, lo.z, feet.x - 1, feet.y + 18, hi.z));
    client.ok(&format!("/fill {} {} {} {} {} {} dirt", lo.x - 1, feet.y - 1, lo.z - 1, hi.x + 1, feet.y - 1, hi.z + 1));
    let planks = client.content.blocks.id("base:oak_planks");
    client.pump_until(|c| c.world.block(hi + IVec3::Y * 4) == planks && c.world.block(lo + IVec3::Y * 18) == planks && kit_pieces(c, 12, "base:loose_flint").is_empty());
    drop(client);
    let file = dir.join("world.ron");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.lines().filter(|line| !line.contains("spawn_kit")).collect::<Vec<_>>().join("\n")).unwrap();
    let client = joined();
    let inside = |p: &IVec3| (lo.x..=hi.x).contains(&p.x) && (lo.z..=hi.z).contains(&p.z) && p.y < feet.y + if p.x < feet.x { 18 } else { 4 };
    for piece in ["base:loose_flint", "base:dead_log"] {
        let under: Vec<IVec3> = kit_pieces(&client, 8, piece).into_iter().filter(inside).collect();
        assert!(under.is_empty(), "{piece} laid inside the build: {under:?}");
    }
    assert!(!kit_pieces(&client, 12, "base:loose_flint").is_empty(), "the kit went nowhere");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_spawn_kit_is_laid_once_in_new_worlds_and_old_saves() {
    let dir = world_dir("spawn-kit");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let joined = || {
        let mut c = Client::join_with(config());
        let feet = c.position.floor().as_ivec3();
        c.pump_until(|c| (-1..=1).all(|dx| (-1..=1).all(|dz| c.world.block(feet + IVec3::new(dx * 12, 0, dz * 12)).is_some())));
        c
    };
    // A new world has it.
    let mut client = joined();
    let laid = kit_flint(&client);
    assert!(laid.len() >= 4, "the spawn kit's flint: {laid:?}");
    // Picked up, it is not laid again on the next start.
    for p in &laid {
        client.ok(&format!("/setblock {} {} {} air", p.x, p.y, p.z));
    }
    client.pump_until(|c| kit_flint(c).len() < laid.len());
    let left = kit_flint(&client);
    drop(client);
    let client = joined();
    assert_eq!(kit_flint(&client), left, "the kit was laid twice");
    drop(client);
    // A save from before the kit (no `spawn_kit` in its world.ron) gets it once.
    let file = dir.join("world.ron");
    let text = std::fs::read_to_string(&file).unwrap();
    let old: String = text.lines().filter(|line| !line.contains("spawn_kit")).collect::<Vec<_>>().join("\n");
    assert!(text.contains("spawn_kit: true") && !old.contains("spawn_kit"), "{text}");
    std::fs::write(&file, old).unwrap();
    let client = joined();
    let again = kit_flint(&client);
    assert!(again.len() >= left.len() + 4, "an old save got no kit: {again:?}");
    drop(client);
    let client = joined();
    assert_eq!(kit_flint(&client), again, "an old save got the kit twice");
    drop(client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_player_who_left_dead_wakes_at_home() {
    let dir = world_dir("dead-home");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    drop(Client::join_with(config()));
    let file = dir.join("players/test.ron");
    let text = std::fs::read_to_string(&file).unwrap();
    // Home with no hearth burning is the bed last slept in.
    let dead = text.replace("current: 20", "current: 0").replace("bed: None", "bed: Some((3.5, 90.0, -7.5))");
    assert!(dead.contains("current: 0") && dead.contains("bed: Some"), "{text}");
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
    for file in ["items.ron", "crops.ron", "creatures.ron", "machines.ron", "villagers.ron", "companions.ron"] {
        std::fs::write(dir.join(file), "[(oops").unwrap();
    }
    let client = Client::join_with(config());
    drop(client);
    assert_eq!(set_aside(&dir.join("columns")).len(), 1, "the bad column is set aside");
    let aside = set_aside(&dir);
    for file in ["items.ron", "crops.ron", "creatures.ron", "machines.ron", "villagers.ron", "companions.ron"] {
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
        // Debug keeps the decimal point a whole number would lose ("12.0", not "12"), which
        // the command would take as the middle of block 12.
        self.ok(&format!("/tp {:?} {:?} {:?}", to.x, to.y, to.z));
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
fn walkers_stand_on_shaped_blocks_where_they_stop_them() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // Southwards: a slab, a stair whose step faces north (the way it comes), a plank.
    client.fill(at, IVec3::new(0, 0, 1), IVec3::new(0, 0, 1), "oak_slab");
    client.fill(at, IVec3::new(0, 0, 2), IVec3::new(0, 0, 2), "oak_stairs");
    client.fill(at, IVec3::new(0, 0, 3), IVec3::new(0, 0, 3), "oak_planks");
    // On the slab's top, half a block up in its cell, then the stair's upper step and the
    // plank: where the client stepped up to, none of it inside a block.
    client.send_position(start + Vec3::new(0.0, 0.5, 1.0), false);
    client.send_position(start + Vec3::new(0.0, 1.0, 2.25), false);
    client.send_position(start + Vec3::new(0.0, 1.0, 3.0), false);
    assert!(client.all_accepted(), "a walker on shaped blocks was put back to {:?}", client.teleported);
    // But not sunk into the slab: its lower half stops movement.
    client.send_position(start + Vec3::new(0.0, 0.25, 1.0), false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start + Vec3::new(0.0, 1.0, 3.0)), "stood inside the slab");
}

/// Ground is what the feet rest on as drawn. Pressed into the corner of a shaft walled with
/// columns and barrels, a player's box reaches into both walls' cells but touches neither,
/// so it is no ground: a dodge there is refused, and the fall down it hurts. On a slab's top
/// it is ground.
#[test]
fn a_shaft_lined_with_shaped_blocks_is_no_ground_to_dodge_or_break_a_fall_on() {
    real_time("movement and combat are judged on the clock: how far a player may get between messages, a dodge's grace in seconds");
    let mut client = on_a_test_floor();
    client.ok("/gamemode survival");
    let start = client.position;
    let at = start.floor().as_ivec3();
    // Columns on the west, barrels on the south, five high from the floor.
    client.fill(at, IVec3::new(-1, 0, 0), IVec3::new(-1, 4, 0), "stone_column");
    client.fill(at, IVec3::new(0, 0, 1), IVec3::new(0, 4, 1), "barrel");
    // Clear of the columns' bases and capitals (2/16 in) and the barrels' hoops (1.5/16 in).
    let corner = at.as_vec3() + Vec3::new(0.2, 0.0, 0.75);
    let top = corner + Vec3::Y * 5.0;
    client.tp(top);
    // Past the dodge's grace since the floor.
    client.wait_server_asked(1);
    assert!(!dodge_taken(&mut client), "a dodge in mid-air beside columns and barrels was taken");
    client.teleported = None;
    for y in (0..5).rev() {
        client.send_position(corner + Vec3::Y * y as f32, false);
        // A server tick on each step, so the fall is seen block by block.
        client.after_server_ticks(1);
    }
    client.after_server_ticks(4);
    assert_eq!(client.health, 18, "a five block fall down the shaft costs two health: the shaft's walls broke the fall");
    assert!(client.teleported.is_none(), "the fall was put back to {:?}", client.teleported);
    // A slab's top is ground to dodge from.
    client.fill(at, IVec3::new(3, 0, 0), IVec3::new(3, 0, 0), "oak_slab");
    client.tp(start + Vec3::new(3.0, 0.5, 0.0));
    client.after_server_ticks(2);
    assert!(dodge_taken(&mut client), "a dodge on a slab's top was refused");
}

/// A sprint at a low frame rate climbs a slab and then a whole block in one input, under a
/// ceiling too low over its start to rise there first: neither the straight line, nor
/// across then up, nor up then across clears, but walking up each step in turn does.
#[test]
fn two_half_steps_in_one_input_are_climbed_under_a_low_ceiling() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    client.fill(at, IVec3::new(0, 0, 1), IVec3::new(0, 0, 1), "oak_slab");
    client.fill(at, IVec3::new(0, 0, 2), IVec3::new(0, 0, 2), "oak_planks");
    // 2.5 over the start, 3 over the slab: room to stand on the slab and step up from it.
    client.fill(at, IVec3::new(0, 2, 0), IVec3::new(0, 2, 0), "oak_slab_top");
    client.fill(at, IVec3::new(0, 3, 1), IVec3::new(0, 3, 1), "oak_planks");
    client.send_position(start + Vec3::new(0.0, 1.0, 2.0), false);
    assert!(client.all_accepted(), "two half-steps in one input were put back to {:?}", client.teleported);
    // A whole block is still a jump, not two steps: none of it on a stair of whole blocks.
    client.tp(start);
    client.fill(at, IVec3::new(0, 0, 1), IVec3::new(0, 0, 1), "oak_planks");
    client.fill(at, IVec3::new(0, 1, 2), IVec3::new(0, 1, 2), "oak_planks");
    client.send_position(start + Vec3::new(0.0, 2.0, 2.0), false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start), "climbed two whole blocks in one input");
}

/// A walk up a stair of slabs and planks onto the roof of a sealed room, in one input, ends
/// on the roof, not in the room: the way down from the top of the climb to where the input
/// says is through the roof.
#[test]
fn a_climb_in_one_input_ends_on_a_roof_not_in_the_room_under_it() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // Eastwards, each top half a block above the last: slab, plank, then the same a level
    // up and another, the last beside the room's roof.
    for (x, y, block) in [(1, 0, "oak_slab"), (2, 0, "oak_planks"), (3, 1, "oak_slab"), (4, 1, "oak_planks"), (5, 2, "oak_slab"), (6, 2, "oak_planks")] {
        client.fill(at, IVec3::new(x, 0, 0), IVec3::new(x, y - 1, 0), "oak_planks");
        client.fill(at, IVec3::new(x, y, 0), IVec3::new(x, y, 0), block);
    }
    // The room: one cell two high at x+7, walled and roofed in planks.
    client.fill(at, IVec3::new(6, 0, -1), IVec3::new(8, 2, 1), "oak_planks");
    client.fill(at, IVec3::new(7, 0, 0), IVec3::new(7, 1, 0), "air");
    client.tp(start);
    client.send_position(start + Vec3::new(7.0, 0.0, 0.0), false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start), "got into a sealed room through its roof");
}

/// A spot the server took as clear stays clear when a shape comes or goes beside it: which
/// of its two boxes judges the spot (the margin box near shapes, the share box among whole
/// blocks) never flips it into a wall, where only the end of the next move would be
/// checked and a player could step through any wall into a sealed room. Pressed a fortieth
/// into a plank wall (the share box allows it) with a slab then set in the cell their box
/// only touches; or a two-hundredth into the floor beside a headstone that is then mined.
#[test]
fn a_shape_placed_or_mined_beside_a_player_never_lets_them_through_a_wall() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // The room: one cell two high at x+4, walled and roofed in planks.
    let room = |client: &mut Client| {
        client.fill(at, IVec3::new(3, 0, -1), IVec3::new(5, 2, 1), "oak_planks");
        client.fill(at, IVec3::new(4, 0, 0), IVec3::new(4, 1, 0), "air");
    };
    let into = at.as_vec3() + Vec3::new(4.5, 0.0, 0.5);
    // A plank wall at x+1, the player against it with their box's south side on a cell's edge.
    room(&mut client);
    client.fill(at, IVec3::new(1, 0, -2), IVec3::new(1, 1, 2), "oak_planks");
    let pressed = at.as_vec3() + Vec3::new(1.0 - 0.3 + 0.025, 0.0, 0.7);
    client.tp(pressed);
    client.fill(at, IVec3::new(0, 0, 1), IVec3::new(0, 0, 1), "oak_slab");
    client.send_position(into, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(pressed), "into a sealed room through a wall, after a slab was set beside a player pressed against it");
    // A headstone at x+1, the player sunk a little into the floor beside it; then it is mined.
    client.fill(at, IVec3::new(0, 0, -2), IVec3::new(2, 2, 2), "air");
    room(&mut client);
    client.fill(at, IVec3::new(1, 0, 0), IVec3::new(1, 0, 0), "stone_headstone");
    let sunk = at.as_vec3() + Vec3::new(0.75, -0.005, 0.5);
    client.tp(sunk);
    client.fill(at, IVec3::new(1, 0, 0), IVec3::new(1, 0, 0), "air");
    client.send_position(into, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(sunk), "into a sealed room through a wall, after a headstone beside a player was mined");
}

/// The test client's world as the engine's movement takes it, as the game's client moves
/// its player: blocks by their boxes, no water.
struct Seen<'a>(&'a Client);

impl voxmar_engine::movement::Surroundings for Seen<'_> {
    fn solid(&self, cell: IVec3) -> bool {
        !self.boxes(cell).is_empty()
    }

    fn boxes(&self, cell: IVec3) -> &[voxmar_engine::physics::Aabb] {
        self.0.world.block(cell).map_or(&[], |b| self.0.content.blocks.collision(b))
    }

    fn water(&self, _: &voxmar_engine::physics::Aabb) -> (f32, voxmar_engine::glam::Vec2) {
        (0.0, voxmar_engine::glam::Vec2::ZERO)
    }

    fn climbable(&self, cell: IVec3) -> bool {
        self.0.world.block(cell).is_some_and(|b| self.0.content.blocks.info(b).climbable)
    }
}

/// An honest client among shapes: the engine's own movement at 30, 60 and 144 frames a
/// second, sending where it is twenty times a second, sprinting along each course straight
/// or drifting into its side (sliding along a wall, past the corners of shapes), jumping
/// or not, is never put back. The courses: a slab floor along a plank wall under a plank
/// ceiling the jumps hit; a row of barrels, pot shelves, columns, a plank block and
/// headstones beside the way; a slab, a stair and plank blocks climbed in half steps; and
/// a plank wall stepping back a block every four, run into at a slant round its outside
/// corners, on a slab floor (judged by the margin box) and on the plain floor (the share box).
/// Each input is stamped with the client's own clock (as `going_silent_does_not_buy_more_time_hovering`),
/// so the server's ticks may run quick.
#[test]
fn honest_movement_among_shapes_is_never_put_back() {
    use voxmar_engine::movement::{Body, Controls, advance};
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    let tuning = client.content.movement.clone();
    let wall_row = ["barrel", "oak_pot_shelf", "stone_column", "oak_planks", "stone_headstone"];
    // Each course: boxes of blocks to fill, corners from `at`.
    type Fills = Vec<(IVec3, IVec3, &'static str)>;
    // A plank wall two high north of the way, a block further back every four blocks along.
    let stepped = |floor: Option<&'static str>| -> Fills {
        let mut fills: Fills = floor.map(|slab| (IVec3::new(1, 0, -6), IVec3::new(20, 0, 1), slab)).into_iter().collect();
        fills.extend((0..5).map(|k| (IVec3::new(1 + 4 * k, 0, -2 - k), IVec3::new(4 + 4 * k, 1, -2 - k), "oak_planks")));
        fills
    };
    let (straight, slanting) = (&[0.0, 0.3][..], &[0.3, 0.6, 1.0][..]);
    let courses: [(&str, Fills, &[f32]); 5] = [
        ("slab floor along a wall under a ceiling", vec![
            (IVec3::new(1, 0, -1), IVec3::new(20, 0, 1), "oak_slab"),
            (IVec3::new(1, 0, -2), IVec3::new(20, 3, -2), "oak_planks"),
            (IVec3::new(1, 3, -1), IVec3::new(20, 3, 1), "oak_planks"),
        ], straight),
        ("shapes beside the way", (0..20).map(|i| {
            let cell = IVec3::new(1 + i, 0, -1);
            (cell, cell, wall_row[i as usize % wall_row.len()])
        }).collect(), straight),
        ("slab, stair and planks in half steps", vec![
            (IVec3::new(2, 0, -1), IVec3::new(2, 0, 1), "oak_slab"),
            (IVec3::new(3, 0, -1), IVec3::new(3, 0, 1), "oak_stairs_west"),
            (IVec3::new(4, 0, -1), IVec3::new(5, 0, 1), "oak_planks"),
            (IVec3::new(5, 1, -1), IVec3::new(5, 1, 1), "oak_slab"),
            (IVec3::new(6, 0, -1), IVec3::new(20, 1, 1), "oak_planks"),
        ], straight),
        ("a wall stepping back, on slabs", stepped(Some("oak_slab")), slanting),
        ("a wall stepping back, on the floor", stepped(None), slanting),
    ];
    // Ground under the stepping wall, which reaches past the test floor's north edge.
    client.fill(at, IVec3::new(1, -1, -6), IVec3::new(20, -1, -4), "stone");
    client.fill(at, IVec3::new(1, 0, -6), IVec3::new(20, 3, -4), "air");
    // The client's clock, on past every run's inputs.
    let mut clock = Instant::now();
    let mut put_back = Vec::new();
    for (course, blocks, drifts) in &courses {
        for &(from, to, block) in blocks {
            client.fill(at, from, to, block);
        }
        // The client's world has the course: its last block arrived.
        let (last, name) = blocks.last().map(|&(_, to, block)| (at + to, block)).unwrap();
        let id = client.content.blocks.expect(&format!("base:{name}"));
        client.pump_until(|c| c.world.block(last) == Some(id));
        for fps in [30.0, 60.0, 144.0] {
            for (&drift, jump) in drifts.iter().flat_map(|d| [(d, false), (d, true)]) {
                client.tp(start);
                let mut body = Body { position: start, on_ground: true, ..Default::default() };
                let controls = Controls { wish: Vec3::new(1.0, 0.0, -drift).normalize(), forward: true, sprint: true, jump, descend: false };
                let (dt, mut t, mut sent) = (1.0 / fps, 0.0f32, 0u32);
                while t < 2.5 {
                    advance(&mut body, &Seen(&client), &tuning, controls, dt);
                    t += dt;
                    if t >= (sent + 1) as f32 * 0.05 {
                        sent += 1;
                        client.send_position_at(body.position, false, clock + Duration::from_secs_f32(t));
                    }
                }
                let case = format!("{course} at {fps} fps, drift {drift}, jump {jump}");
                if !client.all_accepted() {
                    put_back.push(format!("{case}: at {} put back to {:?}", body.position - start, client.teleported.map(|to| to - start)));
                } else {
                    // Where the server has them (a teleport to where they are says): where the
                    // client got to, along the course, every input taken.
                    client.connection.send(ClientMessage::Command { line: "/tp ~ ~ ~".into() });
                    client.pump_until(|c| c.teleported.is_some());
                    let there = client.teleported.take().unwrap();
                    assert!(there.distance(body.position) < 1e-3 && body.position.x - start.x > 1.0, "{case}: the client got to {}, the server has {}", body.position - start, there - start);
                }
                clock += Duration::from_secs(4);
            }
        }
        client.fill(at, IVec3::new(1, 0, -6), IVec3::new(20, 3, 1), "air");
    }
    assert!(put_back.is_empty(), "honest moves put back:\n{}", put_back.join("\n"));
}

/// An honest client climbing a ladder up a wall four blocks high, the engine's own movement
/// at 30, 60 and 144 frames a second, sending where it is twenty times a second, is never
/// put back, and ends on top of the wall; and the server has them there. (Off the ground
/// for longer than a jump, the server takes a ladder their feet are in as something under
/// them.) Each input is stamped with the client's own clock, as
/// `honest_movement_among_shapes_is_never_put_back`.
#[test]
fn an_honest_climb_up_a_ladder_is_never_put_back() {
    use voxmar_engine::movement::{Body, Controls, advance};
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    let tuning = client.content.movement.clone();
    client.fill(at, IVec3::new(4, 0, -1), IVec3::new(7, 3, 1), "oak_planks");
    client.fill(at, IVec3::new(3, 0, 0), IVec3::new(3, 3, 0), "oak_ladder_west");
    let ladder = client.content.blocks.expect("base:oak_ladder_west");
    client.pump_until(|c| c.world.block(at + IVec3::new(3, 3, 0)) == Some(ladder));
    let mut clock = Instant::now();
    for fps in [30.0, 60.0, 144.0] {
        client.tp(start);
        let mut body = Body { position: start, on_ground: true, ..Default::default() };
        let controls = Controls { wish: Vec3::X, forward: true, sprint: false, jump: false, descend: false };
        let (dt, mut t, mut sent) = (1.0 / fps, 0.0f32, 0u32);
        while t < 3.0 {
            advance(&mut body, &Seen(&client), &tuning, controls, dt);
            t += dt;
            if t >= (sent + 1) as f32 * 0.05 {
                sent += 1;
                client.send_position_at(body.position, false, clock + Duration::from_secs_f32(t));
            }
        }
        assert!(body.on_ground && body.position.y - start.y == 4.0 && body.position.x - start.x > 4.0, "at {fps} fps the client did not climb onto the wall: {}", body.position - start);
        assert!(client.all_accepted(), "an honest climb at {fps} fps to {} was put back to {:?}", body.position - start, client.teleported.map(|to| to - start));
        client.connection.send(ClientMessage::Command { line: "/tp ~ ~ ~".into() });
        client.pump_until(|c| c.teleported.is_some());
        let there = client.teleported.take().unwrap();
        assert!(there.distance(body.position) < 1e-3, "at {fps} fps the client got to {}, the server has {}", body.position - start, there - start);
        clock += Duration::from_secs(4);
    }
}

/// As `an_honest_climb_up_a_ladder_is_never_put_back`, up a ladder through an open hatch
/// in a loft's floor three blocks up and out onto the loft: never put back, and the server
/// has them up there.
#[test]
fn an_honest_climb_through_an_open_hatch_is_never_put_back() {
    use voxmar_engine::movement::{Body, Controls, advance};
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    let tuning = client.content.movement.clone();
    client.fill(at, IVec3::new(4, 0, -1), IVec3::new(14, 2, 1), "oak_planks");
    client.fill(at, IVec3::new(0, 3, -1), IVec3::new(14, 3, 1), "oak_planks");
    client.fill(at, IVec3::new(3, 0, 0), IVec3::new(3, 2, 0), "oak_ladder_west");
    client.fill(at, IVec3::new(3, 3, 0), IVec3::new(3, 3, 0), "oak_hatch_open_west");
    let hatch = client.content.blocks.expect("base:oak_hatch_open_west");
    client.pump_until(|c| c.world.block(at + IVec3::new(3, 3, 0)) == Some(hatch));
    let mut clock = Instant::now();
    for fps in [30.0, 60.0, 144.0] {
        client.tp(start);
        let mut body = Body { position: start, on_ground: true, ..Default::default() };
        let controls = Controls { wish: Vec3::X, forward: true, sprint: false, jump: false, descend: false };
        let (dt, mut t, mut sent) = (1.0 / fps, 0.0f32, 0u32);
        while t < 3.0 {
            advance(&mut body, &Seen(&client), &tuning, controls, dt);
            t += dt;
            if t >= (sent + 1) as f32 * 0.05 {
                sent += 1;
                client.send_position_at(body.position, false, clock + Duration::from_secs_f32(t));
            }
        }
        assert!(body.on_ground && body.position.y - start.y == 4.0 && body.position.x - start.x > 4.0, "at {fps} fps the client did not climb through the hatch: {}", body.position - start);
        assert!(client.all_accepted(), "an honest climb through a hatch at {fps} fps to {} was put back to {:?}", body.position - start, client.teleported.map(|to| to - start));
        client.connection.send(ClientMessage::Command { line: "/tp ~ ~ ~".into() });
        client.pump_until(|c| c.teleported.is_some());
        let there = client.teleported.take().unwrap();
        assert!(there.distance(body.position) < 1e-3, "at {fps} fps the client got to {}, the server has {}", body.position - start, there - start);
        clock += Duration::from_secs(4);
    }
}

/// A gap between shaped blocks narrower or lower than the player is no way through, though
/// a share of their box would fit: between two pot shelves in a row (9/16 of a block, the
/// player is 0.6 wide), and on a headstone under an upside-down slab (1.75 high, the
/// player is 1.8 tall).
#[test]
fn gaps_between_shapes_smaller_than_the_player_are_no_way_through() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    // Two shelves facing the same way, south of each other: the second's free front is 9/16 deep.
    client.fill(at, IVec3::new(0, 0, 1), IVec3::new(0, 0, 2), "oak_pot_shelf");
    let beside = at.as_vec3() + Vec3::new(-0.5, 0.0, 2.0 + 9.0 / 32.0);
    client.tp(beside);
    client.send_position(beside + Vec3::X, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(beside), "squeezed between two pot shelves 9/16 apart");
    // As narrow between a whole block and a shelf, and off the gap's middle.
    client.fill(at, IVec3::new(5, 0, 1), IVec3::new(5, 0, 1), "oak_planks");
    client.fill(at, IVec3::new(5, 0, 2), IVec3::new(5, 0, 2), "oak_pot_shelf");
    let beside = at.as_vec3() + Vec3::new(4.5, 0.0, 2.271);
    client.tp(beside);
    client.send_position(beside + Vec3::X, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(beside), "squeezed between planks and a pot shelf 9/16 apart");
    // Onto a headstone (three quarters of a block high, a jump): open above, the player
    // stands on it...
    client.tp(start);
    client.fill(at, IVec3::new(2, 0, 0), IVec3::new(2, 0, 0), "stone_headstone");
    let on = start + Vec3::new(2.0, 0.75, 0.0);
    client.send_position(start + Vec3::X, false);
    client.send_position(on, false);
    assert!(client.all_accepted(), "a jump onto a headstone was put back to {:?}", client.teleported);
    // ...but not under a slab in the upper half of the cell above the next, 1.75 over its top
    // (set once they are off it: no fill shuts a player in).
    client.tp(start + Vec3::X);
    client.fill(at, IVec3::new(2, 2, 0), IVec3::new(2, 2, 0), "oak_slab_top");
    client.send_position(on, false);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start + Vec3::X), "stood on a headstone with 1.75 blocks of headroom");
}

/// A player walks through a doorway in a plank wall with its door open (the leaf swung
/// against the frame, no box), but a shut door is a wall like the planks beside it.
#[test]
fn a_player_walks_through_an_open_door_but_not_a_shut_one() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
    let mut client = on_a_test_floor();
    let start = client.position;
    let at = start.floor().as_ivec3();
    client.fill(at, IVec3::new(2, 0, -2), IVec3::new(2, 1, -1), "oak_planks");
    client.fill(at, IVec3::new(2, 0, 1), IVec3::new(2, 1, 2), "oak_planks");
    let walk = |client: &mut Client| {
        client.tp(start);
        for x in 1..=4 {
            client.send_position(start + Vec3::X * x as f32, false);
        }
    };
    client.fill(at, IVec3::new(2, 0, 0), IVec3::new(2, 0, 0), "wooden_door_open_east");
    client.fill(at, IVec3::new(2, 1, 0), IVec3::new(2, 1, 0), "wooden_door_open_top_east");
    walk(&mut client);
    assert!(client.all_accepted(), "walking through an open door was put back to {:?}", client.teleported);
    client.fill(at, IVec3::new(2, 0, 0), IVec3::new(2, 0, 0), "wooden_door_east");
    client.fill(at, IVec3::new(2, 1, 0), IVec3::new(2, 1, 0), "wooden_door_top_east");
    walk(&mut client);
    client.pump_until(|c| c.teleported.is_some());
    assert_eq!(client.teleported.take(), Some(start + Vec3::X), "walked through a shut door");
}

#[test]
fn swimmers_climb_out_onto_a_bank_but_nobody_jumps_out_of_a_dry_pit_or_lava() {
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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
    real_time("movement is checked against the clock: how far a player may get in the seconds between their messages");
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

/// A name already playing is turned away, in any case, from anywhere else, and from the
/// same address while that player's game is still heard (an address can be a whole
/// school's). A game that froze or lost its network without closing its socket still looks
/// joined until it is idle too long; once it has been silent `rejoin_silence`, its player
/// coming back from that address is them: the earlier connection leaves, saved, and the new
/// one joins as anyone joining would, with what the save holds and no admin rights it was
/// given (`/op`). The integrated server's own player always comes back.
#[test]
fn a_name_already_playing_is_turned_away_unless_it_comes_back_from_a_silent_game_there() {
    real_time("silence and floods are timed on the clock (idle timeouts, chat per second)");
    let dir = world_dir("names");
    let (mut host, addr) = serve(ServerConfig { world: Some(dir.clone()), ..Default::default() });
    let mut guest = Client::greet(|| tcp::connect(addr).unwrap(), "guest", None);
    host.connection.send(ClientMessage::Command { line: "/op guest".into() });
    guest.pump_until(|c| c.admin);
    guest.ok("/skill mining 7");
    // In process is not where either plays.
    for name in ["guest", "GuEsT", "host"] {
        let (connection, server_side) = local_pair();
        host._server.as_ref().unwrap().connections().send(server_side).unwrap();
        say_hello(&connection, name, &host.content);
        let reason = turned_away(&connection, &host.content);
        assert!(reason.contains("already playing"), "{name}: {reason}");
    }
    // From the same address while the guest plays: taken, and the guest plays on.
    let connection = tcp::connect(addr).unwrap();
    say_hello(&connection, "GuEsT", &host.content);
    let reason = turned_away(&connection, &host.content);
    assert!(reason.contains("already playing"), "a playing session was replaced: {reason}");
    assert!(guest.ok("/seed").contains(char::is_numeric), "the playing guest was let go");
    drop(guest);
    gone(&mut host, "guest");
    // The guest's game joins again and freezes with its socket open: it says hello and
    // then nothing, not even the network thread's keepalive. An admin makes it an admin.
    let mut frozen = std::net::TcpStream::connect(addr).unwrap();
    let hello = ClientMessage::Hello { name: "guest".into(), protocol: PROTOCOL_VERSION, content: host.content.fingerprint(), admin_token: None };
    let frame = postcard::to_stdvec(&hello).unwrap();
    std::io::Write::write_all(&mut frozen, &[&(frame.len() as u32).to_le_bytes()[..], &frame].concat()).unwrap();
    let ticks = |c: &Client| c._server.as_ref().unwrap().ticks();
    while host.command("/op guest").iter().any(|(kind, _)| *kind == ChatKind::Error) {
        host.after_ticks_asked(1);
    }
    // Silent past `rejoin_silence` by the server's clock, the guest comes back from there.
    let silence = Limits::default().rejoin_silence;
    let until = ticks(&host) + (silence * TICKS_PER_SECOND as f32) as u64 + 2;
    host.pump_until(|c| ticks(c) >= until);
    let mining = host.content.skills.id("core:mining").unwrap();
    let again = Client::greet(|| tcp::connect(addr).unwrap(), "GuEsT", None);
    assert!(!again.admin, "admin rights given for a session lasted into the next");
    assert_eq!(again.content.skills.level(again.skills.xp(mining)), 7, "joined again without what the save holds");
    // The same in process: the integrated server's own player coming back, never silent.
    let first = Client::join_with(ServerConfig { view_distance: 2, ..Default::default() });
    let second = first.another("test");
    assert!(second.admin, "the integrated server's own player is its admin");
    assert!(turned_away(&first.connection, &first.content).contains("joined again"));
    drop((again, frozen, host, second, first));
    // One file for the name in any case (macOS and Windows would not tell them apart).
    assert!(dir.join("players/guest.ron").is_file());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A game that says hello as `name` and, whatever it sends after (`frozen_says`), can go
/// silent with its socket open: frozen, or its network gone. The server hears nothing more,
/// not even keepalives.
fn frozen_game(addr: std::net::SocketAddr, name: &str, content: &Content) -> std::net::TcpStream {
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let hello = ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: None };
    frozen_says(&mut stream, hello);
    stream
}

fn frozen_says(stream: &mut std::net::TcpStream, message: ClientMessage) {
    let frame = postcard::to_stdvec(&message).unwrap();
    std::io::Write::write_all(stream, &[&(frame.len() as u32).to_le_bytes()[..], &frame].concat()).unwrap();
}

/// A player's game that went silent holds more than their file does: leaving saves the
/// whole world, and a save that cannot write the file (`cut_saves` stops every one short of
/// the players) leaves the old one. Joining again takes what the earlier session held, or
/// what they gained since the last save would be gone, and what they had dropped or stored
/// would come back with the file.
#[test]
fn joining_again_takes_what_the_silent_session_held_not_an_older_file() {
    real_time("a session is silent by the server's clock (rejoin_silence), which ticks run ahead of when sped up");
    let dir = world_dir("again");
    let (mut host, addr) = serve(ServerConfig { world: Some(dir.clone()), cut_saves: true, ..Default::default() });
    let mut frozen = frozen_game(addr, "guest", &host.content);
    // An admin made the guest one, so its commands count (it is theirs for the session).
    while host.command("/op guest").iter().any(|(kind, _)| *kind == ChatKind::Error) {
        host.after_ticks_asked(1);
    }
    for line in ["/skill mining 7", "/give iron_ore 5"] {
        frozen_says(&mut frozen, ClientMessage::Command { line: line.into() });
    }
    let ticks = |c: &Client| c._server.as_ref().unwrap().ticks();
    let until = ticks(&host) + (Limits::default().rejoin_silence * TICKS_PER_SECOND as f32) as u64 + 2 * TICKS_PER_SECOND as u64;
    host.pump_until(|c| ticks(c) >= until);
    let again = Client::greet(|| tcp::connect(addr).unwrap(), "guest", None);
    let (mining, ore) = (host.content.skills.id("core:mining").unwrap(), host.content.blocks.expect("base:iron_ore"));
    assert_eq!(again.content.skills.level(again.skills.xp(mining)), 7, "joined again without what the silent session had gained");
    let held: u32 = again.inventory.slots.iter().flatten().filter(|s| s.item == ore).map(|s| s.count as u32).sum();
    assert_eq!(held, 5, "joined again without what the silent session carried");
    assert!(!dir.join("players/guest.ron").exists(), "the cut saves wrote the player's file, so this proves nothing");
    drop((again, frozen, host));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A server listening on a local port, with `host` joined (over TCP) as an admin.
fn serve(config: ServerConfig) -> (Client, std::net::SocketAddr) {
    let server = spawn(ServerConfig { view_distance: 2, admins: vec!["host".into()], idle_timeout: SERVER_WAIT, ..config }, content()).unwrap();
    let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    (Client::hello(|| tcp::connect(addr).unwrap(), "host", server), addr)
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
    real_time("silence and floods are timed on the clock (idle timeouts, chat per second)");
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
    // Connecting and never saying hello: closed after the configured three server seconds.
    let stream = std::net::TcpStream::connect(addr).unwrap();
    closed_within(stream, Duration::from_secs(60));
}

#[test]
fn a_player_who_falls_silent_is_let_go_and_may_join_again() {
    real_time("silence and floods are timed on the clock (idle timeouts, chat per second)");
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

/// Slow fixture preparation is not a silent peer: the server has no connection to
/// expire yet. Preparation lasts past the actual deadline, by the server's own ticks.
fn prepared_login(remote: bool) {
    let mut host = Client::join();
    let accepted = host._server.as_ref().unwrap().connections();
    let addr = tcp::listen("127.0.0.1:0", accepted.clone()).unwrap();
    let hello_ticks = (Limits::default().hello_seconds * TICKS_PER_SECOND as f32) as u64;
    let guest = Client::greet_content(
        || if remote { tcp::connect(addr).unwrap() } else { local_connection(accepted) },
        "prepared", None, None,
        || {
            host.after_ticks(hello_ticks + 2);
            host.content.clone()
        },
    );
    assert!(guest.id != 0 && guest.health > 0, "prepared login was not welcomed (remote {remote})");
}

#[test]
fn preparing_local_content_does_not_spend_the_hello_deadline() {
    prepared_login(false);
}

#[test]
fn preparing_tcp_content_does_not_spend_the_hello_deadline() {
    prepared_login(true);
}

#[test]
fn a_player_who_sends_nothing_but_keepalives_is_kept() {
    real_time("silence and floods are timed on the clock (idle timeouts, chat per second)");
    // Let go after ten seconds silent, two keepalives' worth, rather than fifteen.
    let idle = 2 * voxmar_protocol::KEEPALIVE;
    let host = Client::join_with(ServerConfig { view_distance: 2, idle_timeout: idle, ..Default::default() });
    let addr = tcp::listen("127.0.0.1:0", host._server.as_ref().unwrap().connections()).unwrap();
    let mut guest = Client::greet(|| tcp::connect(addr).unwrap(), "guest", None);
    // A keepalive's worth past the idle timeout of the server's clock with no input, as
    // from a minimized game: only the connection's keepalives. (Being let go would end the
    // pump with a panic.)
    let ticks = |c: &Client| c._server.as_ref().unwrap().ticks();
    let until = ticks(&host) + (idle + voxmar_protocol::KEEPALIVE).as_secs() * TICKS_PER_SECOND as u64;
    guest.phase = "keepalive wait";
    guest.pongs = false;
    guest.pump_until(|_| ticks(&host) >= until);
    guest.pongs = true;
    assert!(guest.ok("/seed").contains(char::is_numeric), "the guest was let go");
    drop(host);
}

#[test]
fn chat_floods_slow_down_once_and_admins_make_admins_for_the_session() {
    real_time("silence and floods are timed on the clock (idle timeouts, chat per second)");
    let dir = world_dir("op");
    let (mut host, addr) = serve(ServerConfig { world: Some(dir.clone()), ..Default::default() });
    assert!(host.admin, "a name on the admin list is an admin from this machine");
    let mut guest = Client::greet(|| tcp::connect(addr).unwrap(), "guest", None);
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
    let guest = Client::greet(|| tcp::connect(addr).unwrap(), "guest", None);
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
    let wrong = Client::greet_with(|| tcp::connect(addr).unwrap(), "guest", None, Some("open sesame!"));
    assert!(!wrong.admin);
    let owner = Client::greet_with(|| tcp::connect(addr).unwrap(), "owner", None, Some("open sesame"));
    assert!(owner.admin);
}

#[test]
fn players_on_this_machine_are_known_over_ipv4_to_a_dual_stack_listener() {
    let server = spawn(ServerConfig { view_distance: 2, admins: vec!["host".into()], ..Default::default() }, content()).unwrap();
    let Ok(addr) = tcp::listen("[::]:0", server.connections()) else {
        eprintln!("skipped: no IPv6 here");
        return;
    };
    let v4 = std::net::SocketAddr::from(([127, 0, 0, 1], addr.port()));
    if tcp::connect(v4).is_err() {
        eprintln!("skipped: this IPv6 listener takes no IPv4");
        return;
    }
    // Seen as ::ffff:127.0.0.1. The probe above is closed without saying hello.
    let host = Client::hello(|| tcp::connect(v4).unwrap(), "host", server);
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
    real_time("silence and floods are timed on the clock (idle timeouts, chat per second)");
    // The operator lets two connections from one address wait to say hello.
    let limits = Limits { max_pending_per_address: 2, ..Default::default() };
    let server = spawn(ServerConfig { view_distance: 2, max_players: 3, idle_timeout: SERVER_WAIT, limits, ..Default::default() }, content()).unwrap();
    // Listening on IPv4, and on IPv6 too where the machine has it, so one machine has two
    // addresses to come from.
    let v4 = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    let v6 = tcp::listen("[::1]:0", server.connections()).ok();
    let guest = Client::hello(|| tcp::connect(v4).unwrap(), "guest", server);
    let second = Client::greet(|| tcp::connect(v4).unwrap(), "second", None);
    // Two connections from that address that never say hello: with them waiting, its next
    // connection is turned away at once...
    let silent: Vec<_> = (0..2).map(|_| std::net::TcpStream::connect(v4).unwrap()).collect();
    let connection = tcp::connect(v4).unwrap();
    assert!(turned_away(&connection, &guest.content).contains("full"));
    // ...while another address is not, and they are not players, so a third still fits
    // (in process, as from elsewhere, without IPv6).
    let third = match v6 {
        Some(v6) => Client::greet(|| tcp::connect(v6).unwrap(), "third", None),
        None => guest.another("third"),
    };
    assert!(third.health > 0);
    drop((guest, second, third, silent));
}

#[test]
fn an_address_gets_one_starting_kit_in_a_while() {
    // A day is too long to wait: a mod sets the window to two seconds, and gives a kit (base
    // starts a player with nothing).
    let mut content = content();
    let mut files = (*content.files).clone();
    files.push(voxmar_engine::content::ContentFile { package: "test".into(), name: "survival.ron".into(), source: r#"(kit_window: 2.0, starter_kit: [("base:bread", 4)])"#.into() });
    content.files = std::sync::Arc::new(files);
    let server = spawn(ServerConfig { view_distance: 2, ..Default::default() }, content.clone()).unwrap();
    let addr = tcp::listen("127.0.0.1:0", server.connections()).unwrap();
    let mut first = Client::greet_content(|| tcp::connect(addr).unwrap(), "first", Some(server), None, || content.clone());
    let (bread, purse) = (content.blocks.expect("base:bread"), content.blocks.expect("base:coin_purse"));
    let has = |c: &Client, item| c.inventory.slots.iter().flatten().any(|s| s.item == item);
    first.pump_until(|c| has(c, bread));
    // A second fresh name from the same address, inside the window: the purse, no kit.
    let mut second = Client::greet_content(|| tcp::connect(addr).unwrap(), "second", None, None, || content.clone());
    second.pump_until(|c| has(c, purse));
    assert_eq!(second.inventory.slots.iter().flatten().count(), 1, "{:?}", second.inventory);
    // Once the window has passed by the server's clock, the next one has the kit again.
    first.after_server_ticks(2 * TICKS_PER_SECOND as u64);
    let mut third = Client::greet_content(|| tcp::connect(addr).unwrap(), "third", None, None, || content);
    third.pump_until(|c| has(c, bread));
    drop((second, third, first));
}

#[test]
fn a_new_player_starts_with_nothing_but_the_purse() {
    let client = Client::join();
    let id = |name: &str| client.content.blocks.expect(name);
    assert_eq!(client.inventory.slots.iter().flatten().count(), 1, "{:?}", client.inventory);
    assert_eq!(client.inventory.slots[voxmar_engine::inventory::SLOTS - 1].map(|s| s.item), Some(id("base:coin_purse")));
    let carried = |item| client.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    client.drain();
    let before: std::collections::HashSet<_> = client.creatures.keys().copied().collect();
    client.ok(&format!("/summon {name} 1 ~ ~ ~"));
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear && !before.contains(&v.id) && v.position.distance(c.position) <= 8.0));
    let new: Vec<_> = client.creatures.values().filter(|v| v.species == bear && !before.contains(&v.id) && v.position.distance(client.position) <= 8.0).collect();
    assert_eq!(new.len(), 1, "one newly summoned {name} beside us: {new:?}; prior ids {before:?}");
    let id = new[0].id;
    eprintln!("beside: summoned {name} id {id}, health {}, at {:?}, tick {}, prior ids {}", new[0].health, new[0].position, client.server_tick, before.len());
    (id, slot)
}

#[test]
fn beside_selects_the_new_creature_when_one_is_already_in_view() {
    let mut client = Client::join();
    client.ok("/gamemode creative");
    let (first, _) = beside_a_bear(&mut client);
    client.ok("/creatures hold");
    let (second, _) = beside_a_bear(&mut client);
    assert_ne!(second, first, "the old bear must not stand in for the new summon");
    assert!(client.creatures.contains_key(&first), "the collision was with a real prior bear");
    assert!(client.creatures.contains_key(&second), "the new summoned bear was actually replicated");
}

/// Faces creature `id` holding `slot` and provokes it, a blow every three server seconds
/// only while it does not fight back, until it is seen to begin a wind-up: its attack pose
/// coming on, not one still shown from the blow before (its end not yet heard). Returns
/// how many of its blows had come before, for the next to be waited past.
fn provoke_until_it_winds_up(client: &mut Client, id: u32, slot: usize, name: &str) -> usize {
    provoke_until(client, id, slot, name, |c| c.creatures.get(&id).is_some_and(|v| v.pose == voxmar_engine::creature::brain::Pose::Attacking))
}

/// Server seconds [`provoke_until`] goes on provoking without seeing a wind-up, at most.
const PROVOKE_WAIT: f32 = 60.0;

/// `noticed` lets the regression below miss attack poses deliberately, as a delayed
/// observer can, while still applying every real server reply.
fn provoke_until(client: &mut Client, id: u32, slot: usize, name: &str, noticed: impl Fn(&Client) -> bool) -> usize {
    let attacking = |c: &Client| c.creatures.get(&id).is_some_and(|v| v.pose == voxmar_engine::creature::brain::Pose::Attacking);
    client.pump_until(|c| !attacking(c));
    let before = client.hurt.len();
    // A bear that already answered us needs time to begin its next blow, not another
    // sword hit on every call. A returned blow also renews that wait below: missing its
    // short attacking pose must not turn an active fight into repeated damaging pokes.
    let mut poked = client.hurt_by.iter().any(|&(by, _)| by == id).then_some(client.server_tick);
    let mut heard = before;
    let mut unseen = 0;
    let mut last_health = client.creatures.get(&id).map(|v| v.health);
    let (began, died) = (client.server_tick, client.died);
    eprintln!("provoke: {name} id {id}, health {last_health:?}, tick {}, player damage {:?}, creature blows {before}, deaths {}", client.server_tick, client.damage, client.deaths);
    loop {
        // A dead player is fought by nothing, and a creature that never answers is no
        // fight: either would be waited on for ever, so each fails, by the server's clock.
        assert!(
            client.died == died && client.health > 0,
            "the player died while provoking the {name} id {id} (health {}, {} creature blows, {} heard with the wind-up unseen)",
            client.health, client.hurt.len(), unseen
        );
        let waited = client.server_tick.saturating_sub(began) as f32 / TICKS_PER_SECOND as f32;
        assert!(
            waited < PROVOKE_WAIT,
            "the {name} id {id} began no wind-up in {waited:.0} server s of provoking (pose {:?}, health {last_health:?}, {} player blows, {} creature blows, {unseen} heard with the wind-up unseen, player health {})",
            client.creatures.get(&id).map(|v| v.pose), client.blows, client.hurt.len(), client.health
        );
        assert!(client.creatures.contains_key(&id), "the {name} id {id} went (last health {last_health:?}, tick {}, {} player blows, damage {:?}, {} creature blows, {} deaths)", client.server_tick, client.blows, client.damage, client.hurt.len(), client.deaths);
        face(client, id, slot, false);
        // Repositioning can pump replies too; judge retaliation after that round trip.
        assert!(client.creatures.contains_key(&id), "the {name} id {id} went while facing it (last health {last_health:?}, damage {:?}, deaths {})", client.damage, client.deaths);
        last_health = Some(client.creatures[&id].health);
        if client.hurt_by[heard..].iter().any(|&(by, _)| by == id) {
            poked = Some(client.server_tick);
            // A blow heard with its wind-up unseen: shown, the first few, so a wait that
            // never sees one tells a fight it keeps missing from a creature that stopped.
            unseen += 1;
            if unseen <= 5 || unseen % 20 == 0 {
                eprintln!("provoke: {name} id {id} blow {unseen} heard with its wind-up unseen, tick {}, pose {:?}, creature blows {}", client.server_tick, client.creatures[&id].pose, client.hurt.len());
            }
        }
        heard = client.hurt.len();
        if poked.is_none_or(|at| client.server_tick >= at + 3 * TICKS_PER_SECOND as u64) {
            eprintln!("provoke: strike {name} id {id}, health {last_health:?}, tick {}, damage {:?}, creature blows {}", client.server_tick, client.damage, client.hurt.len());
            client.propose("core:attack", ActionTarget::Creature(id));
            poked = Some(client.server_tick);
        }
        let asked = client.server_tick;
        client.pump_until(|c| noticed(c) || !c.creatures.contains_key(&id) || c.server_tick >= asked + 4);
        if noticed(client) {
            // Blows whose poses were missed belong to the wait, not this new wind-up.
            return client.hurt.len();
        }
    }
}

/// The selected creature's first answer from this wind-up onwards. Other creatures'
/// replies and earlier blows cannot satisfy a defence or supply its damage/timestamp.
fn blow_from(hurts: &[(u32, u16)], id: u32, after: usize) -> Option<usize> {
    hurts.iter().enumerate().skip(after).find_map(|(index, &(by, _))| (by == id).then_some(index))
}

fn wait_for_blow(client: &mut Client, id: u32, after: usize) -> usize {
    // Each change of the creature's pose while waiting is shown (the first twenty), with
    // how far it stands: a wait that times out then tells a wind-up that never ended from
    // one that ended with no blow heard, or a creature that went off.
    let seen = |c: &Client| c.creatures.get(&id).map(|v| (v.pose, v.position.with_y(0.0).distance(c.position.with_y(0.0)), v.position.y - c.position.y));
    let shown = std::cell::Cell::new(0);
    let last = std::cell::Cell::new(seen(client).map(|(pose, ..)| pose));
    eprintln!("wait: id {id} after {after} creature blows, tick {}, seen {:?}", client.server_tick, seen(client));
    client.pump_until(|c| {
        let now = seen(c);
        if now.map(|(pose, ..)| pose) != last.get() && shown.get() < 20 {
            shown.set(shown.get() + 1);
            eprintln!("wait: id {id} now {now:?}, tick {}, creature blows {}", c.server_tick, c.hurt.len());
        }
        last.set(now.map(|(pose, ..)| pose));
        blow_from(&c.hurt_by, id, after).is_some()
    });
    blow_from(&client.hurt_by, id, after).expect("the selected creature answered")
}

#[test]
fn a_blow_wait_keeps_other_creatures_and_earlier_blows_separate() {
    let (bear, other) = (7, 8);
    let hurts = [(bear, 0), (other, 0), (bear, 4)];
    assert_eq!(blow_from(&hurts[..2], bear, 1), None, "neither an old bear blow nor another creature answers this wind-up");
    assert_eq!(blow_from(&hurts, bear, 1), Some(2), "the selected bear's actual damage index");
    assert_eq!(blow_from(&hurts, other, 2), None, "an earlier unrelated blow cannot be reused");
}

#[test]
fn a_windup_wait_does_not_strike_a_creature_that_is_already_fighting() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let mut client = Client::join();
    client.ok("/gamemode survival");
    client.ok("/skill hitpoints 60");
    client.pump_until(|c| c.health >= 100);
    let (id, slot) = beside_a_bear(&mut client);
    let before = provoke_until_it_winds_up(&mut client, id, slot, "bear");
    wait_for_blow(&mut client, id, before);
    client.drain();
    let (blows, health, heard) = (client.blows, client.creatures[&id].health, client.hurt.len());
    // Deliberately ignore five complete wind-ups while their real Hurt replies arrive.
    // They prove that the bear is still fighting, even without its short pose being seen.
    let settled = std::cell::Cell::new(false);
    let deadline = client.ticks() + 30 * TICKS_PER_SECOND as u64;
    let next = provoke_until(&mut client, id, slot, "bear", |c| {
        assert!(c.ticks() < deadline, "no fresh wind-up after five missed blows");
        let attacking = c.creatures.get(&id).is_some_and(|v| v.pose == voxmar_engine::creature::brain::Pose::Attacking);
        if c.hurt_by[heard..].iter().filter(|&&(by, _)| by == id).count() >= 5 && !attacking {
            settled.set(true);
        }
        settled.get() && attacking
    });
    assert!(next >= heard + 5, "the missed blows really arrived");
    assert!(settled.get(), "the fifth attack ended before the next wind-up was noticed");
    assert_eq!(client.blows, blows, "an active fight needed no more provoking blows");
    assert_eq!(client.creatures[&id].health, health, "waiting did not damage the bear");
    let struck = wait_for_blow(&mut client, id, next);
    assert_eq!(client.hurt_by[struck].0, id, "the new wind-up really struck");
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    // swings by), however late a busy machine lets this thread send it. Wait for each
    // answered blow before the next click: the server holds only one pending swing.
    let start = Instant::now();
    for k in 0..5u32 {
        let due = start + Duration::from_secs_f32(if k == 0 { 0.0 } else { 0.6 * k as f32 - 0.06 });
        client.propose_at("core:attack", ActionTarget::Creature(id), due);
        client.pump_until(|c| c.kinds.len() > k as usize || !c.creatures.contains_key(&id));
        assert!(client.creatures.contains_key(&id), "the bear died before the flourish");
    }
    client.pump_until(|c| c.kinds.len() >= 5 || !c.creatures.contains_key(&id));
    assert_eq!(client.kinds, [Blow::Plain, Blow::Beat, Blow::Beat, Blow::Beat, Blow::Flourish], "damage {:?}", client.damage);
    // The server tells the pips after each, for the client's meter: one a beat, spent on the
    // flourish.
    assert_eq!(client.pips, [0, 1, 2, 3, 0]);
}

#[test]
fn a_swing_is_dropped_when_its_weapon_leaves_the_hand() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    client.wait_server_asked(2);
    assert_eq!(client.blows, 0, "the dropped swing still landed, for {:?}", client.damage);
    // Swung and kept in hand, it lands.
    face(&mut client, id, slot, true);
    client.wait_server_asked(1);
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.blows == 1);
}

/// How many of `item` the player carries.
fn carried_of(client: &Client, item: voxmar_engine::block::BlockId) -> u32 {
    (0..voxmar_engine::inventory::SLOTS).filter_map(|s| client.inventory.get(s)).filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

#[test]
fn a_sling_shoots_a_stone_a_clear_line_away_and_trains_ranged() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    // Draws by a key whose first shot hits: by chance, the shots it took to hit were four
    // on most keys, each a server second apart, and now and then none of sixteen hit.
    let mut client = Client::join_with(ServerConfig { view_distance: 2, draw_key: Some(5), ..Default::default() });
    let (sling, stone) = (client.content.blocks.expect("base:sling"), client.content.blocks.expect("base:cobblestone"));
    let ranged = client.content.skills.id("core:ranged").expect("a ranged skill");
    let bear = client.content.creatures.find("brown_bear").expect("a brown bear in the content");
    // Creative, so the bear's blows pass us by, on a wide flat floor: the land round loaded
    // first (a busy server streams it late), then the floor seen laid, by the server's word.
    client.ok("/gamemode creative");
    let feet = client.position.floor().as_ivec3();
    let corners = [(-12, -12), (-12, 12), (12, -12), (12, 12), (0, 0)].map(|(x, z)| feet + IVec3::new(x, -1, z));
    client.pump_until(|c| corners.iter().all(|&p| c.world.block(p).is_some()));
    client.ok("/fill ~-12 ~-1 ~-12 ~12 ~-1 ~12 stone");
    client.ok("/fill ~-12 ~ ~-12 ~12 ~4 ~12 air");
    let floor = client.content.blocks.id("base:stone");
    client.pump_until(|c| corners.iter().all(|&p| c.world.block(p) == floor && c.world.block(p + IVec3::Y).is_some_and(|b| b.is_air())));
    client.ok("/clear");
    client.ok("/skill ranged 10");
    client.ok("/give sling 1");
    client.pump_until(|c| carried_of(c, sling) == 1);
    let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == sling)).expect("the sling on the hotbar");
    client.drain();
    let before: std::collections::HashSet<_> = client.creatures.keys().copied().collect();
    client.ok("/summon brown_bear 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear && !before.contains(&v.id)));
    let id = client.creatures.values().find(|v| v.species == bear && !before.contains(&v.id)).unwrap().id;
    client.ok("/creatures hold");
    // Eight blocks off, out of any sword's reach, inside the sling's twelve.
    let at = client.creatures[&id].position;
    let aim = |client: &mut Client| {
        let yaw = yaw_toward(client.position, at);
        client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: true }));
    };
    client.ok(&format!("/tp {} {} {}", at.x + 8.0, at.y, at.z));
    aim(&mut client);
    client.settle();
    // Nothing to shoot: no swing, and the player is told what the sling needs.
    client.chat.clear();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.chat.iter().any(|(_, line)| line.contains("Nothing to shoot")));
    client.wait_server_asked(1);
    assert_eq!(client.blows, 0, "a sling with no stone still swung");
    // With stones: each shot takes one, hit or miss, and a hit trains Ranged.
    client.ok("/give cobblestone 16");
    client.pump_until(|c| carried_of(c, stone) == 16);
    let xp = client.skills.xp(ranged);
    let mut shots = 0;
    while client.damage.iter().all(|&d| d == 0) {
        assert!(shots < 16, "sixteen sling shots from eight blocks never hit: {:?}", client.damage);
        if shots > 0 {
            client.wait_server_asked(1);
        }
        client.propose("core:attack", ActionTarget::Creature(id));
        shots += 1;
        client.pump_until(|c| c.blows == shots);
        client.pump_until(|c| carried_of(c, stone) == 16 - shots);
    }
    client.pump_until(|c| c.skills.xp(ranged) > xp);
    // A wall between is a miss, and the stone is still spent.
    let x = at.x + 4.0;
    client.ok(&format!("/fill {} {} {} {} {} {} stone", x.floor(), at.y.floor(), at.z.floor() - 3.0, x.floor(), at.y.floor() + 4.0, at.z.floor() + 3.0));
    let wall = [0.0, 1.0, 2.0].map(|y| Vec3::new(x, at.y + y, at.z).floor().as_ivec3());
    client.pump_until(|c| wall.iter().all(|&p| c.world.block(p) == floor));
    // Two shots, the second on the sling's beat (its cooldown, from the content): both are
    // at the air, so off the beat, and earn no pip. The first waits out the cooldown of the
    // shot that hit, by the server's ticks: a click before it is ready is no shot.
    let beat = client.content.combat.timing(&client.content.combat.wielded(Some(sling), |_| 10)).cooldown;
    client.after_ticks_asked((beat * TICKS_PER_SECOND as f32).ceil() as u64);
    let (blows, left, pips) = (client.blows, carried_of(&client, stone), client.pips.last().copied().unwrap_or(0));
    let start = Instant::now();
    for k in 0..2u32 {
        client.propose_at("core:attack", ActionTarget::Creature(id), start + Duration::from_secs_f32(beat * k as f32 - if k == 0 { 0.0 } else { 0.06 }));
        client.pump_until(|c| c.blows == blows + k + 1);
        assert_eq!(client.damage.last(), Some(&0), "a shot through a wall hit");
    }
    let kinds = &client.kinds[client.kinds.len() - 2..];
    assert_eq!(kinds, [voxmar_engine::combat::Blow::Plain; 2], "shots into a wall counted as {kinds:?}");
    assert!(client.pips.last().copied().unwrap_or(0) <= pips, "shots into a wall earned a pip: {:?}", client.pips);
    client.pump_until(|c| carried_of(c, stone) == left - 2);
    // The wall down again: with a clear line, only the stone gone makes the next shot miss.
    client.ok(&format!("/fill {} {} {} {} {} {} air", x.floor(), at.y.floor(), at.z.floor() - 3.0, x.floor(), at.y.floor() + 4.0, at.z.floor() + 3.0));
    // The last stone gone while a shot winds up: still answered, as a miss at the air, and
    // the player told why.
    let slots: Vec<(usize, u8)> = (0..voxmar_engine::inventory::SLOTS).filter_map(|s| client.inventory.get(s).filter(|k| k.item == stone).map(|k| (s, k.count))).collect();
    for &(slot, count) in &slots[1..] {
        client.propose("core:drop", ActionTarget::Stack { slot: slot as u8, count });
    }
    let (last, count) = slots[0];
    if count > 1 {
        client.propose("core:drop", ActionTarget::Stack { slot: last as u8, count: count - 1 });
    }
    client.pump_until(|c| carried_of(c, stone) == 1);
    client.wait_server_asked(1);
    client.chat.clear();
    let blows = client.blows;
    client.propose("core:attack", ActionTarget::Creature(id));
    client.propose("core:drop", ActionTarget::Stack { slot: last as u8, count: 1 });
    client.pump_until(|c| c.blows == blows + 1);
    assert_eq!(client.damage.last(), Some(&0), "a shot with no stone left hit");
    client.pump_until(|c| c.chat.iter().any(|(_, line)| line.contains("Nothing to shoot")));
}

#[test]
fn an_arrow_that_hits_may_be_left_by_the_target_to_pick_up_again() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    // Draws by a key whose first shot hits and leaves its arrow: by chance, it took from two
    // shots to twenty-six, each a server second apart.
    let mut client = Client::join_with(ServerConfig { view_distance: 2, draw_key: Some(10), ..Default::default() });
    let (bow, arrow) = (client.content.blocks.expect("base:short_bow"), client.content.blocks.expect("base:arrow"));
    let bear = client.content.creatures.find("brown_bear").expect("a brown bear in the content");
    let recover = client.content.combat.recover(&client.content.combat.wielded(Some(bow), |_| 10));
    assert!(recover >= 0.5, "a bow leaves its arrow by what it hit at least half the time, not {recover}");
    client.ok("/gamemode creative");
    client.ok("/fill ~-12 ~-1 ~-12 ~12 ~-1 ~12 stone");
    client.ok("/fill ~-12 ~ ~-12 ~12 ~4 ~12 air");
    client.ok("/clear");
    client.ok("/skill ranged 10");
    client.ok("/give short_bow 1");
    client.ok("/give arrow 48");
    client.pump_until(|c| carried_of(c, bow) == 1 && carried_of(c, arrow) == 48);
    let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == bow)).expect("the bow on the hotbar");
    let before: std::collections::HashSet<_> = client.creatures.keys().copied().collect();
    client.ok("/summon brown_bear 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear && !before.contains(&v.id)));
    let id = client.creatures.values().find(|v| v.species == bear && !before.contains(&v.id)).unwrap().id;
    client.ok("/creatures hold");
    let at = client.creatures[&id].position;
    client.ok(&format!("/tp {} {} {}", at.x + 10.0, at.y, at.z));
    let yaw = yaw_toward(client.position, at);
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: true }));
    client.settle();
    // Shot until a hit leaves an arrow by the bear: a hit leaves one half the time, so a
    // forty-health bear dies only after a dozen or more hits, by which one has all but surely
    // been left. A miss in reach may land by it too (`recover_miss`), so only an arrow that
    // came with a hit counts.
    let lying = |c: &Client| c.items.values().filter(|v| v.stack.item == arrow && v.position.distance(at) < 3.0).map(|v| v.stack.count as u32).sum::<u32>();
    let mut shots = 0;
    loop {
        assert!(client.creatures.contains_key(&id) && shots < 48, "no arrow left by the bear by a hit after {shots} shots: {:?}", client.damage);
        let before = lying(&client);
        client.propose("core:attack", ActionTarget::Creature(id));
        shots += 1;
        client.pump_until(|c| c.blows == shots || !c.creatures.contains_key(&id));
        client.wait_server_asked(1);
        if client.damage.last().is_some_and(|&d| d > 0) && lying(&client) > before {
            break;
        }
    }
}

/// A shot that lands by a wild bees' nest stirs the bees, and they sting the shooter; with a
/// campfire beside the nest, smoke keeps them calm (bees.ron, `shot_reach`).
#[test]
fn a_shot_by_a_bees_nest_stirs_the_bees_unless_smoke_calms_them() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let mut client = Client::join();
    let (bow, arrow) = (client.content.blocks.expect("base:short_bow"), client.content.blocks.expect("base:arrow"));
    let bear = client.content.creatures.find("brown_bear").expect("a brown bear in the content");
    client.ok("/fill ~-12 ~-1 ~-12 ~12 ~-1 ~12 stone");
    client.ok("/fill ~-12 ~ ~-12 ~12 ~4 ~12 air");
    client.ok("/clear");
    client.ok("/skill ranged 10");
    client.ok("/give short_bow 1");
    client.ok("/give arrow 8");
    client.pump_until(|c| carried_of(c, bow) == 1 && carried_of(c, arrow) == 8);
    let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == bow)).expect("the bow on the hotbar");
    let before: std::collections::HashSet<_> = client.creatures.keys().copied().collect();
    client.ok("/summon brown_bear 1 ~ ~ ~");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == bear && !before.contains(&v.id)));
    let id = client.creatures.values().find(|v| v.species == bear && !before.contains(&v.id)).unwrap().id;
    client.ok("/creatures hold");
    let at = client.creatures[&id].position;
    let nest = at.floor().as_ivec3() + IVec3::new(0, 0, 2);
    client.ok(&format!("/setblock {} {} {} bee_nest", nest.x, nest.y, nest.z));
    client.ok(&format!("/tp {} {} {}", at.x + 10.0, at.y, at.z));
    let yaw = yaw_toward(client.position, at);
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: false }));
    // Bees sting only those who can be hurt, so no creative from here; till now creative, so
    // the bear summoned at the player's feet began no fight (a blow wound up walks a held
    // bear after the player, off from beside the nest).
    client.ok("/gamemode survival");
    client.wait_server_asked(1);
    let stung = |c: &Client| c.chat.iter().any(|(_, line)| line.starts_with("Stung!"));
    client.chat.clear();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.blows == 1);
    client.wait_server_asked(1);
    assert!(stung(&client), "a shot by the nest stirs its bees: {:?}", client.chat);
    // A campfire beside the nest: the next shot leaves them calm.
    client.ok(&format!("/setblock {} {} {} campfire", nest.x + 1, nest.y, nest.z));
    client.chat.clear();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.blows == 2);
    client.wait_server_asked(1);
    assert!(!stung(&client), "smoked bees stay calm: {:?}", client.chat);
    // A smoked nest on the bear's other side, met first in the search, and the first nest's
    // campfire moved off: the unsmoked nest's bees are stirred though the smoked one's stay calm.
    let other = at.floor().as_ivec3() + IVec3::new(0, 0, -2);
    client.ok(&format!("/setblock {} {} {} bee_nest", other.x, other.y, other.z));
    client.ok(&format!("/setblock {} {} {} air", nest.x + 1, nest.y, nest.z));
    client.ok(&format!("/setblock {} {} {} campfire", other.x, other.y, other.z - 3));
    client.chat.clear();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.blows == 3);
    client.wait_server_asked(1);
    assert!(stung(&client), "an unsmoked nest beside a smoked one stirs: {:?}", client.chat);
    // Smoke clinging to the shooter (effects.ron's `smoky`) calms every nest the shot is near.
    client.ok("/effect smoky 60");
    client.chat.clear();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.blows == 4);
    client.wait_server_asked(1);
    assert!(!stung(&client), "a smoky shooter stirs no bees: {:?}", client.chat);
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    let mut client = Client::join();
    client.ok("/gamemode survival");
    client.ok("/fill ~-2 ~-1 ~-2 ~2 ~-1 ~2 stone");
    client.ok("/tp ~ ~6 ~");
    client.pump_until(|c| c.teleported.is_some());
    // Six blocks up and told nothing else, the server has us in the air past the grace.
    client.wait_server_asked(1);
    client.teleported = None;
    client.propose("core:dodge", ActionTarget::None);
    client.pump_until(|c| c.teleported.is_some());
    // Landing, told just before the dodge (between the server's ticks), counts as ground.
    client.ok("/tp ~ ~-5 ~");
    client.wait_server_asked(1);
    client.teleported = None;
    let landed = client.position - Vec3::Y;
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: landed, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.position = landed;
    assert!(dodge_taken(&mut client), "a dodge as the player landed was refused");
    // On the ground it is taken.
    client.ok("/tp ~ ~ ~");
    client.wait_server_asked(2);
    assert!(dodge_taken(&mut client), "a dodge on the ground was refused");
}

#[test]
fn a_perfect_block_staggers_a_bear_and_a_dodge_lets_its_blow_pass() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    // few seconds only while it does not fight back), then answers `respond` `after`
    // seconds after the server sent the wind-up (its pose's stamp: on a busy machine this
    // thread may read it long after); returns the selected bear's answered blow index, and
    // how late past that moment the answer went.
    let fight = |client: &mut Client, after: f32, respond: &dyn Fn(&Client)| -> (usize, f32) {
        let before = provoke_until_it_winds_up(client, id, slot, "bear");
        let due = client.attacking_since[&id] + Duration::from_secs_f32(after);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let sent = Instant::now();
        respond(client);
        (wait_for_blow(client, id, before), sent.saturating_duration_since(due).as_secs_f32())
    };
    // Each answer's moment moves by what came of the last, so a slow machine is met where
    // its blows fall: a block not perfect was too soon, a hit too late. An answer that went
    // more than a step past its moment (the wind-up read after it, or this thread kept off
    // its cores while it waited) says nothing of the moment: it is set aside and the search
    // stays, `LATE_ANSWERS` of them at most.
    const STEP: f32 = 0.05;
    const LATE_ANSWERS: u32 = 24;
    let answer = |client: &mut Client, want: Defended, respond: &dyn Fn(&Client), undo: &dyn Fn(&Client)| {
        let mut after = windup - 0.1;
        let mut outcomes = Vec::new();
        let mut late = Vec::new();
        while outcomes.len() < 8 {
            let (struck, sent_late) = fight(client, after.max(0.0), respond);
            let outcome = client.hurt[struck].1;
            undo(client);
            if outcome == want {
                return struck;
            }
            client.wait_server_asked(1);
            if sent_late > STEP {
                late.push(sent_late);
                assert!(late.len() < LATE_ANSWERS as usize, "{} answers went more than {STEP} s past their moment ({late:.2?} s late), so none could say when the bear's blow is met; the rest came to {outcomes:?}", late.len());
                eprintln!("answer: set aside, sent {sent_late:.2} s past its moment ({outcome:?})");
                continue;
            }
            outcomes.push(outcome);
            after += if outcome == Defended::Blocked { STEP } else { -STEP };
        }
        panic!("answers to the bear's blows came to {outcomes:?}, {} more set aside as late", late.len());
    };
    // Raised just before the blow: a perfect block, and the next blow a counter.
    let raise = |c: &Client| {
        let _ = c.connection.send(ClientMessage::Guard { up: true });
    };
    let lower = |c: &Client| {
        let _ = c.connection.send(ClientMessage::Guard { up: false });
    };
    let health = client.health;
    let perfect = answer(&mut client, Defended::Perfect, &raise, &lower);
    assert_eq!(client.hurt[perfect].0, 0, "a perfect block takes nothing");
    assert!(client.health <= health);
    // Shown staggered by it, at rest (not a blow): the view goes out the tick the blow
    // came to nothing, after word of it.
    client.pump_until(|c| c.staggers.iter().any(|&(s, ..)| s == id));
    let shown: Vec<_> = client.staggers.iter().filter(|&&(s, ..)| s == id).collect();
    assert_eq!(shown.len(), 1, "staggered once: {shown:?}");
    assert_eq!(shown[0].1, perfect + 1, "staggered by the perfect block, not before it");
    assert_ne!(shown[0].2, voxmar_engine::creature::brain::Pose::Attacking, "a staggered bear is shown winding nothing up");
    let blows = client.kinds.len();
    client.propose("core:attack", ActionTarget::Creature(id));
    client.pump_until(|c| c.kinds.len() > blows);
    assert_eq!(client.kinds[blows], Blow::Counter);
    assert!(client.damage[blows] > 0, "a counter cannot miss");
    // A dodge just before the blow: it passes through. Tried first on the staggered bear's
    // next blow, which a dodge does not hasten or delay.
    face(&mut client, id, slot, false);
    let dodged = answer(&mut client, Defended::Dodged, &|c| c.propose("core:dodge", ActionTarget::None), &|_| {});
    assert_eq!(client.hurt[dodged].0, 0, "a dodged blow takes nothing");
    // Staggered after its recovery: its next blow came no sooner than both, and a wind-up.
    let next = blow_from(&client.hurt_by, id, perfect + 1).expect("the bear struck again");
    let gap = client.hurt_at[next] - client.hurt_at[perfect];
    let least = recovery + feel.stagger + windup;
    assert!(gap.as_secs_f32() > least - 0.1, "its next blow came {:.2}s after the perfect block: under {least:.2}s", gap.as_secs_f32());
}

#[test]
fn armour_goes_on_from_the_hand_a_shield_guards_and_death_counts_what_is_worn() {
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
            // A blow taken lets the creature recover a while before it is provoked again.
            if !outcomes.is_empty() {
                client.wait_server_asked(1);
            }
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
                wait_for_blow(&mut client, id, before);
                client.wait_server_asked(1);
                continue;
            }
            // Straight back from it, facing it, until the blow lands: a walk's worth each
            // server tick, as the creature moves by them (a server running behind moves
            // both slower). Ticks are read off the server as it runs: the `Time` it tells
            // once a second would move us in jumps of a whole second's walk.
            let away = (client.position - client.creatures[&id].position).with_y(0.0).normalize_or_zero();
            let started = client.ticks();
            let mut last = started;
            while blow_from(&client.hurt_by, id, before).is_none() {
                let now = client.ticks();
                assert!(now < started + 5 * TICKS_PER_SECOND as u64, "the {name}'s blow never came: {:?}", client.creatures.get(&id).map(|v| v.pose));
                client.position += away * walk * (now - last) as f32 / TICKS_PER_SECOND as f32;
                last = now;
                let yaw = yaw_toward(client.position, client.creatures[&id].position);
                client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: false }));
                client.pump_until(|c| blow_from(&c.hurt_by, id, before).is_some() || c.ticks() > last);
            }
            let struck = blow_from(&client.hurt_by, id, before).expect("the selected creature struck");
            outcomes.push(client.hurt[struck].1);
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
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
    real_time("combat is judged on the clock: blows by when they arrived, wind-ups and rhythm in seconds, not in ticks");
    use voxmar_engine::inventory::WORN;
    let mut client = Client::join();
    client.ok("/gamemode survival");
    client.ok("/fill ~-2 ~-1 ~-2 ~2 ~-1 ~2 stone");
    let shirt = client.content.blocks.id("base:chainmail_shirt").unwrap();
    client.ok("/give chainmail_shirt 1");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == shirt));
    client.wait_server_asked(1);
    // Two pairs of dodges, the pairs 1.0 s apart by the clock the server judges dodges by
    // (`propose_at`: exactly, however busy the machine). Of each pair the second is refused
    // by the first's cooldown, and puts us back. Unarmoured (light, 1.0 s less the 0.1 a
    // client's clock may run ahead) the second pair's first is taken; in chainmail (medium,
    // 1.2 s less 0.1) it is refused too. Returns whether it was.
    let twice = |client: &mut Client, first: Instant| {
        let before = client.teleports;
        let second = first + Duration::from_millis(1000);
        for at in [first, second] {
            client.propose_at("core:dodge", ActionTarget::None, at);
            client.propose_at("core:dodge", ActionTarget::None, at);
        }
        // All four handled, and the refusals' teleports with them.
        client.settle();
        let refused = client.teleports - before;
        assert!(matches!(refused, 2 | 3), "of four dodges, {refused} were refused (the first was refused, or a second at once was not)");
        refused == 3
    };
    let first = Instant::now();
    assert!(!twice(&mut client, first), "unarmoured, a dodge 1.0 s after the last was refused");
    let from = client.inventory.slots.iter().position(|s| s.is_some_and(|s| s.item == shirt)).unwrap();
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (WORN + 1) as u8, count: 1 });
    client.pump_until(|c| c.inventory.worn[1].is_some_and(|s| s.item == shirt));
    // Three seconds after the first pair, by the same clock, clears either cooldown.
    assert!(twice(&mut client, first + Duration::from_secs(3)), "in chainmail, a dodge 1.0 s after the last was taken");
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
        "/summon aurochs 1 ~7 ~ ~2 held",
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
    lockstep("the coke oven bakes by the server's ticks, so on a busy machine its 30 s is not done before it is first asked about");
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
        "/summon aurochs 1 ~7 ~ ~2 held",
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
    // The coal goes in once the land round is in, and the oven bakes it by its own heat.
    // Waited for after it went in (`machine_says`), the land took longer on a busy machine
    // than the bake: the first ask found it done, idle.
    client.land_round();
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
        client.after_ticks(10);
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
/// of the server's clock, counted once the land round the player is in (a machine's survey
/// looks only at land the server holds).
#[track_caller]
fn machine_says(client: &mut Client, at: &str, want: &str, seconds: u64) -> String {
    machine_shows(client, at, want, seconds, |said| said.contains(want))
}

/// As `machine_says`, until what `/machine` says passes `shows` (described as `what`).
#[track_caller]
fn machine_shows(client: &mut Client, at: &str, what: &str, seconds: u64, shows: impl Fn(&str) -> bool) -> String {
    client.land_round();
    let started = client.server_tick;
    loop {
        let said = client.ok(&format!("/machine {at}"));
        if shows(&said) {
            return said;
        }
        assert!(server_seconds(client, started) < seconds, "/machine {at} never said `{what}`: {said}");
        wait_server(client, 1);
    }
}

/// Tries the lock of the chest at `at` with the item in slot 0, and returns what the
/// server says.
fn pick_lock(client: &mut Client, at: IVec3) -> String {
    client.chat.clear();
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:pick_lock", ActionTarget::Block { block: at, face: Face::NegX });
    client.pump_until(|c| c.chat.iter().any(|(_, t)| t.contains("lock") || t.contains("pick")));
    client.chat.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>().join(" ")
}

#[test]
fn a_treasure_chest_opens_to_a_pick_of_its_age_or_the_one_before_and_holds_its_tables_finds() {
    let mut client = Client::join();
    let p = client.position.floor().as_ivec3();
    let at = |v: IVec3| format!("{} {} {}", v.x, v.y, v.z);
    let (bronze, iron) = (p + IVec3::new(2, 0, 0), p + IVec3::new(2, 0, 2));
    for line in ["/clear".to_owned(), format!("/setblock {} bronze_treasure_chest", at(bronze)), format!("/setblock {} iron_treasure_chest", at(iron))] {
        client.ok(&line);
    }
    let blocks = client.content.blocks.clone();
    let picks = |c: &Client, name: &str| c.inventory.slots.iter().flatten().filter(|s| s.item == c.content.blocks.expect(name)).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(iron) == blocks.id("base:iron_treasure_chest") && c.inventory.slots.iter().all(Option::is_none));
    // Nothing in hand: it is locked, and stays so.
    assert!(pick_lock(&mut client, bronze).contains("It is locked"));
    // A bone pick (Nomad) on an Iron chest, three ages short: too crude, and kept.
    client.ok("/give bone_lockpick 2");
    client.pump_until(|c| picks(c, "base:bone_lockpick") == 2);
    assert!(pick_lock(&mut client, iron).contains("too crude"));
    client.ok("/seed");
    assert_eq!(picks(&client, "base:bone_lockpick"), 2, "a pick too crude to try is kept");
    // On a Bronze chest, two ages short: a lucky try, the pick broken either way.
    let said = pick_lock(&mut client, bronze);
    assert!(said.contains("pick broke") || said.contains("lucky try"), "{said}");
    client.pump_until(|c| picks(c, "base:bone_lockpick") == 1);
    // A copper pick (Farm), the age before Bronze's: it opens, and is used up.
    client.ok("/clear");
    client.ok(&format!("/setblock {} bronze_treasure_chest", at(bronze)));
    client.ok("/give copper_lockpick 1");
    client.pump_until(|c| picks(c, "base:copper_lockpick") == 1 && c.world.block(bronze) == blocks.id("base:bronze_treasure_chest"));
    assert!(pick_lock(&mut client, bronze).contains("The lock gives"));
    client.pump_until(|c| c.world.block(bronze) == blocks.id("base:chest") && picks(c, "base:copper_lockpick") == 0);
    // A plain chest now, holding the Bronze table's finds: always its gold, as much as a
    // Common thing of the chest's level is worth, times a share (treasure.ron).
    let (low, high) = client.content.treasure.chest(blocks.expect("base:bronze_treasure_chest")).unwrap().gold;
    assert!(low >= 1 && low < high, "{low}..={high}");
    let said = client.ok(&format!("/machine {}", at(bronze)));
    assert!(items_held(&said) >= low, "{said}");
    use_machine(&mut client, bronze);
    client.pump_until(|c| picks(c, "base:gold_coin") > 0);
    assert!((low..=high).contains(&picks(&client, "base:gold_coin")), "{} gold, not {low}..={high}", picks(&client, "base:gold_coin"));
    // Nothing mines the Iron chest's lock away: three seconds at it, and it stands.
    client.propose("core:mine", ActionTarget::Block { block: iron, face: Face::NegX });
    wait_server(&mut client, 3);
    assert_eq!(client.world.block(iron), blocks.id("base:iron_treasure_chest"), "the locked chest was mined");
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
    lockstep("the macerator loses power within two server seconds of the break, counted while the player asks after it");
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
    let started = client.server_tick;
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
    let broken = client.server_tick;
    client.ok("/clear");
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
    client.ok("/give copper_ore 1");
    client.pump_until(|c| count(c, ore) == 1);
    use_machine(&mut client, macerator);
    client.pump_until(|c| count(c, ore) == 0);
    machine_says(&mut client, &at(macerator), "no power", 2);
    let after = client.server_tick - broken;
    assert!(after <= 2 * TICKS_PER_SECOND as u64, "the macerator ran on for {after} ticks");
    machine_says(&mut client, &at(core), "not complete", 1);
    // Nor does it make any more: the ore is still there, with no dust, a second after a
    // powered macerator would have finished it (its recipe's seconds over its speed).
    let crush_seconds = {
        let machines = &client.content.machines;
        let machine = machines.get(client.content.blocks.expect("base:macerator")).expect("a macerator is a machine");
        let Role::Processor { speed, .. } = &machine.role else { panic!("a macerator is a processor") };
        let crush = machine.processes.iter().map(|&id| machines.process(id)).find(|p| p.inputs.iter().any(|(accepts, _)| accepts.contains(&ore))).expect("the macerator crushes copper ore");
        crush.seconds / speed
    };
    wait_server(&mut client, crush_seconds.ceil() as u64 + 1);
    let said = machine_says(&mut client, &at(macerator), "no power", 1);
    use_machine(&mut client, macerator);
    // What it gave reaches the client with the next tick's state, after the chat line.
    wait_server(&mut client, 1);
    assert_eq!(count(&client, dust), 0, "the macerator made dust with its hall broken: {said}");
}

#[test]
fn chests_keep_what_they_hold_across_restarts_and_an_unreadable_machines_file_is_kept_aside() {
    let dir = world_dir("machines");
    lockstep("what machines hold within so many server seconds of asking, across restarts");
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
    // Its column may load after the player's: the server's seconds count from there.
    client.pump_until(|c| c.world.block(chest) == c.content.blocks.id("base:chest"));
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
        client.after_ticks(4);
    };
    // Its boiler taken away, it keeps the work but cannot go on with it, so however long
    // the next join takes (a busy machine generates land slowly, the server ticking on),
    // nothing is done before it is asked.
    let boiler_at = format!("{} {} {}", crusher.x + 1, crusher.y, crusher.z);
    client.ok(&format!("/setblock {boiler_at} air"));
    machine_says(&mut client, &crusher_at, "no power", 10);
    drop(client);
    let save = std::fs::read_to_string(dir.join("machines.ron")).unwrap();
    assert!(save.contains("base:iron_dust"), "the save keeps the work: {save}");
    let mut client = Client::join_with(config());
    client.pump_until(|c| c.world.block(crusher) == c.content.blocks.id("base:steam_crusher"));
    // Until its first step it says it is idle; then, given its boiler back, it goes on from
    // where it was.
    machine_says(&mut client, &crusher_at, "no power", 20);
    client.ok(&format!("/setblock {boiler_at} boiler"));
    client.ok("/give coal_ore 1");
    client.pump_until(|c| c.inventory.slots.iter().flatten().any(|s| s.item == coal) && c.world.block(crusher + IVec3::X) == c.content.blocks.id("base:boiler"));
    use_machine(&mut client, crusher + IVec3::X);
    client.pump_until(|c| c.inventory.slots.iter().all(Option::is_none));
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
        client.after_ticks(6);
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

/// How many items a machine's `/machine` answer says it holds: "… holds 64 items." (none
/// said, none held).
fn items_held(said: &str) -> u32 {
    let words: Vec<_> = said.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric())).collect();
    words.windows(2).find_map(|w| (w[1] == "items" || w[1] == "item").then(|| w[0].parse().ok()).flatten()).unwrap_or(0)
}

/// A player whose connection drops and who comes straight back is let in as themselves,
/// even when the server reads their new hello in the loop that finds the old connection
/// gone (connections are read oldest first).
#[test]
fn a_player_who_drops_and_comes_straight_back_is_let_in() {
    let client = Client::join();
    for _ in 0..16 {
        let bob = client.another("bob");
        drop(bob);
        // No wait: the hello goes with the new connection, right behind the drop.
        client.another("bob");
    }
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
    let cobblestone = client.content.blocks.expect("base:cobblestone");
    let held = |c: &Client| c.inventory.slots.iter().flatten().filter(|s| s.item == cobblestone).map(|s| s.count as u32).sum::<u32>();
    // What the player held as they came in (a starting kit) is no stack of the chest's.
    let held_at_join = held(&client);
    machine_says(&mut client, &at, &format!("holds {} items", 27 * 64), 10);
    // Out on the ground, or already picked up by the player standing beside it: the wait
    // for the land round the player (`machine_says`) can outlast the pick-up delay.
    let spilled = |c: &Client| {
        let lying = c.items.values().filter(|i| i.stack.item == cobblestone).map(|i| i.stack.count as u32).sum::<u32>();
        lying + held(c).saturating_sub(held_at_join)
    };
    // They fell out as the server started, so are there by the time the chest is read. The
    // wait is by the server's clock, well inside a dropped stack's life (`dropped::LIFETIME`):
    // one by the wall's could outlast them (half an hour of the server's, 90 s at the tests'
    // speed) and then only time out, saying nothing of what the player saw.
    client.pump_ticks(10 * TICKS_PER_SECOND as u64, |c| spilled(c) == 3 * 64);
    let lying: Vec<_> = client.items.values().filter(|i| i.stack.item == cobblestone).map(|i| (i.stack.count, i.position)).collect();
    assert_eq!(spilled(&client), 3 * 64, "lying {lying:?}, held {} ({held_at_join} as they came in), tick {}", held(&client), client.server_tick);
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
        client.after_ticks(6);
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
        "/summon aurochs 1 ~3 ~ ~5 held",
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
        client.after_ticks(6);
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
    for line in ["/summon aurochs 1 ~3 ~ ~2 held", "/creatures tame aurochs", "/creatures hold aurochs"] {
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
        owner.after_ticks(6);
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
    for line in ["/clear", "/fill ~2 ~ ~-1 ~4 ~1 ~3 air", "/setblock ~3 ~ ~ treadwheel", "/summon aurochs 1 ~3 ~ ~2 held", "/creatures tame aurochs", "/creatures hold aurochs"] {
        client.ok(line);
    }
    let start = client.position.floor().as_ivec3();
    let wheel = format!("{} {} {}", start.x + 3, start.y, start.z);
    // The kept one, by its keeper: a wild aurochs can come by (the land's own, by the draw),
    // and turns down food held out to it, so feeding whichever aurochs was in view hung the
    // test. One stands well off, held, so feeding the wrong one cannot pass by luck.
    client.ok("/summon aurochs 1 ~-12 ~ ~-12 held");
    let me = client.id;
    let mine = |c: &Client| c.creatures.iter().find(|(_, v)| v.species == aurochs && v.owner == Some(me)).map(|(&id, _)| id);
    client.pump_until(|c| mine(c).is_some() && c.creatures.values().filter(|v| v.species == aurochs).count() >= 2);
    let kept = mine(&client).expect("seen");
    let turns = |client: &mut Client| !client.ok(&format!("/machine {wheel}")).contains("needs a kept animal");
    let started = client.server_tick;
    while !turns(&mut client) {
        assert!(server_seconds(&client, started) < 10, "the tamed aurochs never walked the wheel: {:?} {:?}", client.ok(&format!("/machine {wheel}")), client.creatures.iter().filter(|(_, v)| v.species == aurochs).map(|(id, v)| (*id, v.position)).collect::<Vec<_>>());
        client.after_ticks(6);
    }
    // Three days on, unfed, it will not walk.
    client.ok("/time add 3d");
    let said = machine_says(&mut client, &wheel, "fed in the last 2 days", 10);
    assert!(said.contains("needs a kept animal"), "{said}");
    // Fed its wheat, it walks again.
    client.ok("/give wheat 1");
    let wheat = client.content.blocks.expect("base:wheat");
    client.pump_until(|c| c.inventory.get(0).is_some_and(|s| s.item == wheat) && c.creatures.contains_key(&kept));
    let at = client.creatures[&kept].position;
    client.ok(&format!("/tp {} {} {}", at.x + 0.8, at.y, at.z));
    client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position: client.position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: false }));
    client.propose("base:tend", ActionTarget::Creature(kept));
    client.pump_until(|c| c.inventory.get(0).is_none());
    let started = client.server_tick;
    while !turns(&mut client) {
        assert!(server_seconds(&client, started) < 10, "fed, the aurochs still does not walk the wheel: {:?} {:?}", client.ok(&format!("/machine {wheel}")), client.creatures.iter().filter(|(_, v)| v.species == aurochs).map(|(id, v)| (*id, v.position)).collect::<Vec<_>>());
        client.after_ticks(6);
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
    // The first table also finishes the Crafting Table quest, whose reward (its Engineering)
    // waits to be claimed: claim it, so both grants are counted exactly.
    client.pump_until(|c| c.quests.iter().any(|q| q.name == "base:crafting_table" && q.done));
    client.claim("base:crafting_table");
    client.pump_until(|c| c.claimed("base:crafting_table"));
    client.settle();
    assert!(!client.inventory.slots.iter().flatten().any(|s| s.item == stick), "crafting all went on into another recipe");
    assert_eq!(client.earned[earned..], [80, quest_xp(&client, "base:crafting_table")], "one table's Engineering, once, then the quest's reward");
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
    real_time("the loot is seen between ticks, and must not be picked up (pickup delay in ticks) before this thread has looked");
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

/// `/creatures kill` spares another player's kept animals, judged by who keeps them, not
/// by how a name is written: a rabbit Ann tamed is refused to the host while she is here,
/// and is killed for ANN, who is Ann come back with her name in capitals.
#[test]
fn creatures_kill_spares_another_players_kept_animals_whatever_the_case_of_their_name() {
    let mut host = Client::join_remote("host", &["host", "ann"]);
    let mut ann = host.another("Ann");
    let rabbit = ann.content.creatures.find("rabbit").expect("rabbits in the content");
    let kept = |c: &Client| c.creatures.values().find(|v| v.species == rabbit && v.kept).map(|v| v.id);
    // Held still beside the players while it is tamed and the host tries it.
    ann.ok("/summon rabbit 1 ~ ~ ~-2 held");
    ann.ok("/creatures hold rabbit");
    ann.pump_until(|c| c.creatures.values().any(|v| v.species == rabbit));
    ann.ok("/creatures tame rabbit");
    ann.pump_until(|c| c.creatures.values().any(|v| v.species == rabbit && v.owner == Some(c.id)));
    let id = kept(&ann).expect("the tamed rabbit");
    // The host, an admin too, may not kill it.
    host.pump_until(|c| c.creatures.contains_key(&id));
    host.ok("/creatures kill rabbit");
    wait_server(&mut host, 1);
    assert!(host.creatures.get(&id).is_some_and(|v| v.kept), "the host killed Ann's rabbit");
    // Ann leaves, and comes back as ANN: the rabbit is hers again, and hers to kill.
    drop(ann);
    let mut ann = host.another("ANN");
    ann.pump_until(|c| c.creatures.get(&id).is_some_and(|v| v.owner == Some(c.id)));
    // Beside it, wherever it has hopped to.
    let to = ann.creatures[&id].position;
    ann.ok(&format!("/tp {} {} {}", to.x, to.y, to.z));
    let said = ann.ok("/creatures kill rabbit");
    assert!(!said.contains("for 0 creatures"), "{said}");
    ann.pump_until(|c| !c.creatures.contains_key(&id) && c.deaths >= 1);
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

/// A freed prisoner who settled at a player's home is in `companions.ron`, and a world that
/// opens with one there has them standing where they were left, saved again at the end.
#[test]
fn a_settled_companion_is_back_when_the_world_opens_again() {
    let dir = world_dir("companions");
    let config = || ServerConfig { view_distance: 2, world: Some(dir.clone()), ..Default::default() };
    let client = Client::join_with(config());
    let spawn = client.position;
    drop(client);
    let path = dir.join("companions.ron");
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "[]", "nobody has settled yet");
    let at = format!("({}, {}, {})", spawn.x + 3.0, spawn.y, spawn.z);
    std::fs::write(&path, format!("[(species: \"base:captive\", seed: 9, position: {at}, yaw: 0.0, health: 5), (species: \"lostmod:ghost\", seed: 10, position: {at}, yaw: 0.0, health: 5)]")).unwrap();
    let mut client = Client::join_with(config());
    let captive = client.content.creatures.find("captive").expect("the captive in the content");
    client.pump_until(|c| c.creatures.values().any(|v| v.species == captive));
    drop(client);
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(saved.matches("base:captive").count(), 1, "the captive is saved again, once: {saved}");
    assert_eq!(saved.matches("lostmod:ghost").count(), 1, "and the one whose species is gone waits: {saved}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Bounded span timings for the ignored cost diagnostic, to distinguish pathing/brains
/// from the once-per-second process pass. No subscriber is installed by ordinary tests.
#[derive(Clone, Default)]
struct VillagerProfile(std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<&'static str, std::collections::VecDeque<f64>>>>);

struct SpanStarted(Option<Instant>);

impl<S> tracing_subscriber::Layer<S> for VillagerProfile
where S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a> {
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if matches!(attrs.metadata().name(), "villagers" | "creatures" | "processes" | "fluids") {
            ctx.span(id).unwrap().extensions_mut().insert(SpanStarted(None));
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(started) = span.extensions_mut().get_mut::<SpanStarted>()
        {
            started.0 = Some(Instant::now());
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(started) = span.extensions_mut().get_mut::<SpanStarted>()
            && let Some(at) = started.0.take()
        {
            let mut samples = self.0.lock().unwrap();
            let row = samples.entry(span.metadata().name()).or_default();
            if row.len() == 1200 { row.pop_front(); }
            row.push_back(at.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

impl VillagerProfile {
    fn clear(&self) { self.0.lock().unwrap().clear(); }

    fn report(&self, label: &str) {
        let rows = self.0.lock().unwrap().clone();
        for (name, row) in &rows {
            let mut times: Vec<_> = row.iter().copied().collect();
            times.sort_by(f64::total_cmp);
            eprintln!("COST {label} / {name}: median {:.3} ms, p99 {:.3} ms, max {:.3} ms over {} passes", times[times.len()/2], times[times.len()*99/100], times[times.len()-1], times.len());
        }
    }
}

/// Tick costs during the first 30 seconds of arrival, after settling, and with a second
/// village kept alive by another player. Use a release binary; no warm-up hides arrival.
#[test]
#[ignore]
fn villagers_cost_per_tick() {
    use tracing_subscriber::prelude::*;
    use voxmar_worldgen::Generator;
    let profile = VillagerProfile::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(profile.clone())).unwrap();
    let mut client = Client::join_with(ServerConfig { seed: VILLAGE_SEED, view_distance: 2, ..Default::default() });
    let sample = |client: &mut Client, label: &str, arrival: Option<&str>| {
        profile.clear();
        let from = client._server.as_ref().unwrap().ticks();
        if let Some(command) = arrival { client.ok(command); }
        // Start before teleporting, and use the handle's tick counter. `after_ticks`
        // first waits for another once-a-second Time message, hiding early arrival.
        client.pump_until(|c| c._server.as_ref().unwrap().ticks() >= from + 30 * TICKS_PER_SECOND as u64);
        let server = client._server.as_ref().unwrap();
        let count = (server.ticks() - from) as usize;
        let times = server.tick_times();
        let mut last: Vec<f64> = times[times.len().saturating_sub(count)..].iter().map(|t| t.as_secs_f64() * 1000.0).collect();
        last.sort_by(f64::total_cmp);
        let mean = last.iter().sum::<f64>() / last.len() as f64;
        eprintln!("COST {label}: mean {mean:.3} ms, median {:.3} ms, p99 {:.3} ms, max {:.3} ms over {} ticks", last[last.len() / 2], last[last.len() * 99 / 100], last[last.len() - 1], last.len());
        profile.report(label);
    };
    client.ok("/time set 8000");
    client.ok("/time freeze");
    sample(&mut client, "alone", None);
    assert!(villagers(&client).is_empty(), "baseline is already among people");
    let found = client.ok("/locate settlement village 2000");
    let tp = found.split("/tp ").nth(1).unwrap().split(" goes").next().unwrap();
    sample(&mut client, "first arrival", Some(&format!("/tp {tp}")));
    assert!(villagers(&client).len() >= 5);
    eprintln!("COST first village: {} people in sight", villagers(&client).len());
    sample(&mut client, "settled village", None);
    // Pure planning outside the measured interval gives a fixed second place; the two
    // players really stream both villages, with no synthetic creature population.
    let world_content = voxmar_worldgen::WorldContent::load(&client.content).unwrap();
    let overworld = voxmar_worldgen::Overworld::with_content(VILLAGE_SEED, client.content.blocks.clone(), &world_content);
    let spawn = overworld.spawn_point();
    let second = overworld.settlements_near(spawn.xz(), 5000).into_iter()
        .filter(|s| s.size == "village" && s.centre.as_vec3().distance(client.position) > 400.0)
        .min_by_key(|s| (s.centre.xz() - spawn.xz()).length_squared()).expect("another village");
    let mut guest = client.another("village_guest");
    let at = second.centre;
    sample(&mut client, "two villages first arrival", Some(&format!("/tp village_guest {} {} {}", at.x, at.y, at.z)));
    guest.drain();
    assert!(villagers(&guest).len() >= 5, "second village never came alive");
    let mut ids: std::collections::HashSet<_> = villagers(&client).iter().map(|v| v.id).collect();
    ids.extend(villagers(&guest).iter().map(|v| v.id));
    eprintln!("COST two villages: {} people alive in both views", ids.len());
    sample(&mut client, "two villages settled", None);
}

/// The HV age as the gate shows it (`gate.rs`, "HV age"): a standing Turbine Hall under a
/// boiler, its HV cable to a bank, an MV transformer, a processing array in its cube and an
/// induction smelter; the hall says it is running, the array and the smelter that they are
/// making, on the one HV network.
#[test]
fn an_hv_hall_runs_a_processing_array_and_an_induction_smelter_on_one_cable() {
    let mut client = Client::join();
    let p = client.position.floor().as_ivec3();
    let at = |v: IVec3| format!("{} {} {}", v.x, v.y, v.z);
    let hall = p + IVec3::new(4, 0, -6);
    let rel = |x: i32, y: i32, z: i32| hall + IVec3::new(x, y, z);
    let (core, boiler, cable, smelter) = (rel(0, 2, 1), rel(0, 3, 1), rel(-1, 2, 1), rel(-1, 3, 1));
    let (array, cube) = (rel(-1, 2, 4), rel(-2, 1, 4));
    let mut lines = vec![
        "/clear".to_owned(),
        format!("/fill {} {} air", at(rel(-4, 0, -1)), at(rel(5, 5, 8))),
        format!("/fill {} {} hv_casing", at(hall), at(rel(4, 2, 2))),
        format!("/fill {} {} air", at(rel(1, 1, 1)), at(rel(3, 1, 1))),
        format!("/setblock {} turbine_hall", at(core)),
        format!("/setblock {} boiler", at(boiler)),
        format!("/setblock {} hv_cable", at(cable)),
        format!("/setblock {} mv_transformer", at(rel(-1, 1, 1))),
        format!("/setblock {} hv_battery_bank", at(rel(-2, 2, 1))),
        format!("/setblock {} induction_smelter", at(smelter)),
        format!("/setblock {} hv_cable", at(rel(-1, 2, 2))),
        format!("/setblock {} hv_cable", at(rel(-1, 2, 3))),
        format!("/fill {} {} hv_casing", at(cube), at(cube + IVec3::new(2, 2, 2))),
        format!("/setblock {} air", at(cube + IVec3::ONE)),
        format!("/setblock {} processing_array", at(array)),
    ];
    for line in lines.drain(..) {
        client.ok(&line);
    }
    let blocks = &client.content.blocks;
    let (coal, ore) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ore"));
    let count = |c: &Client, item| c.inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
    client.pump_until(|c| c.world.block(array) == c.content.blocks.id("base:processing_array") && c.inventory.slots.iter().all(Option::is_none));
    // Beside it all, within reach of the boiler, the array and the smelter.
    client.tp(rel(-3, 0, 2).as_vec3() + Vec3::new(0.5, 0.0, 0.5));
    client.ok("/give coal_ore 4");
    client.pump_until(|c| count(c, coal) == 4);
    use_machine(&mut client, boiler);
    client.pump_until(|c| count(c, coal) == 0);
    for target in [array, smelter] {
        client.ok("/give copper_ore 32");
        client.pump_until(|c| count(c, ore) == 32);
        use_machine(&mut client, target);
        client.pump_until(|c| count(c, ore) == 0);
    }
    machine_says(&mut client, &at(core), "running", 20);
    machine_says(&mut client, &at(array), "making", 20);
    machine_says(&mut client, &at(smelter), "making", 20);
    let powered = |said: &str| said.split(", ").any(|part| {
        part.strip_prefix("at ").and_then(|p| p.strip_suffix("% power")).or_else(|| part.strip_suffix("% done"))
            .and_then(|n| n.parse::<f32>().ok()).is_some_and(|n| n > 0.0)
    });
    // Until each says it is powered: one look can come as it starts an item, at 0% done.
    for target in [array, smelter] {
        machine_shows(&mut client, &at(target), "making, powered", 20, |said| said.contains("making") && powered(said));
    }
}

// A fresh survival player's first day, played with legitimate actions only and timed by the
// server's own tick counter (never the wall clock), so the numbers say how long the early
// game takes. See docs/first-days.md, "Measured pacing".

/// Server seconds each milestone may take at most, counted from the moment the player can
/// first act. The budgets are generous on purpose, about three times what was measured: they
/// catch a stall, or a change that makes an early step far slower, not a few seconds of drift.
/// A slow machine slows ticks and actions together, so the server-second numbers hold, and
/// the test waits by ticks and by state, never for a fixed time.
const FRESH_BUDGETS: &[(&str, f32)] = &[
    ("timber", 60.0),
    ("crafting_table", 60.0),
    ("stone_in_hand", 120.0),
    ("first_night", 180.0),
    ("a_tempting_bite", 190.0),
    ("a_bed_of_your_own", 200.0),
    ("torchlight", 220.0),
    ("hearth_materials", 300.0),
    ("hearth_lit", 320.0),
    ("pit_kiln", 360.0),
    ("fire_the_clay", 500.0),
];

/// What the player's time goes on, besides crafting, placing and waiting.
#[derive(Clone, Copy)]
enum Act {
    Mine,
    Walk,
    Hunt,
    Cook,
}

/// A milestone: the server seconds since the clock started, and how much of that went on each
/// activity so far.
struct Mark {
    name: &'static str,
    at: f32,
    acts: [f32; 4],
    /// The player's levels then, by skill, as the client last heard.
    levels: Vec<(&'static str, u8)>,
    /// Side quests the way finished by then, claimed with it.
    side: Vec<String>,
}

/// The skills the first two ages train, as the timeline shows them.
const PACED_SKILLS: &[&str] = &["mining", "crafting", "engineering", "cooking", "construction"];

/// A player at the start of a survival game, and the plot built beside them to stand in for
/// terrain the test cannot walk to.
struct Fresh {
    c: Client,
    /// The cell the player started in; the plot is laid out from it, with the ground a block
    /// below.
    origin: IVec3,
    /// Where the player stands (feet) as last told to the server.
    at: Vec3,
    /// The hotbar slot held.
    held: u8,
    /// The server tick the clock started at.
    t0: u64,
    marks: Vec<Mark>,
    /// Blocks the player broke, and the server ticks spent on each activity.
    mined: u32,
    acts: [u64; 4],
    /// Times the lane ran out of animals and was stocked again.
    restocks: u32,
    /// A server tick and the moment it stood for: the game's clock, which the player's
    /// messages are stamped with (`Fresh::now`).
    epoch: (u64, Instant),
    /// The ticks the bot has let pass (`Fresh::tick`), each of them a turn it took: its own
    /// clock for how long it has tried. A busy machine can keep the bot from its turns while
    /// a quick server ticks on (sixty times the clock's pace runs thirty seconds of ticks in
    /// half a real one), so a try bounded by the server's ticks could end after a step or two.
    steps: u64,
    /// The creatures the bot swung at: each holds a grudge (`Memory::HurtBy`), a miss as
    /// much as a hit, though a miss leaves it whole.
    swung: Vec<u32>,
    /// What the pacing check counts (`Pacing`), kept as the player goes.
    pacing: std::cell::RefCell<Pacing>,
}

/// A kind of new thing the pacing check counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Novelty {
    /// An item first in the pack.
    Item,
    /// A biome stood in, or a cave, ruin or village entered (the journal's pages).
    Place,
    /// A creature first met (the journal's animals, monsters and bosses).
    Creature,
    /// A station first worked at: a block tagged `station`, a home station (`stations.ron`)
    /// or a multiblock's core.
    Station,
}

/// The pacing check (docs/first-days.md, "Pacing"): each new thing the player meets, by
/// kind and server second, and the stretches spent doing one action over and over.
#[derive(Default)]
struct Pacing {
    /// Each new thing: the server second it came, its kind and its name.
    new: Vec<(f32, Novelty, String)>,
    seen: std::collections::HashSet<(Novelty, String)>,
    /// The action going on over and over: what it is (the action and the block or item it
    /// is on), the second it began, and how many times so far.
    stretch: Option<(String, f32, u32)>,
    /// The longest such stretch so far: what, its seconds, and how many times.
    longest: (String, f32, u32),
}

impl Pacing {
    fn met(&mut self, at: f32, kind: Novelty, name: &str) {
        if self.seen.insert((kind, name.to_owned())) {
            self.new.push((at, kind, name.to_owned()));
        }
    }

    /// The player proposes `what` at `at`: the same as the stretch going on adds to it,
    /// anything else ends it.
    fn act(&mut self, at: f32, what: String) {
        match &mut self.stretch {
            Some((doing, _, times)) if *doing == what => *times += 1,
            _ => {
                self.end(at);
                self.stretch = Some((what, at, 1));
            }
        }
    }

    /// The stretch going on ends at `at`.
    fn end(&mut self, at: f32) {
        if let Some((what, from, times)) = self.stretch.take()
            && at - from > self.longest.1
        {
            self.longest = (what, at - from, times);
        }
    }

    /// The new things in each window of `window` server seconds from the start up to `to`,
    /// by kind.
    fn windows(&self, window: f32, to: f32) -> Vec<std::collections::BTreeMap<Novelty, Vec<&str>>> {
        let count = (to / window).ceil().max(1.0) as usize;
        let mut out = vec![std::collections::BTreeMap::new(); count];
        for (at, kind, name) in &self.new {
            let i = ((at / window) as usize).min(count - 1);
            out[i].entry(*kind).or_insert_with(Vec::new).push(name.as_str());
        }
        out
    }

    /// The longest wait with nothing new between `since` and `to`, counted from `since` for
    /// one that began before it: its seconds, the second it began, and the new thing that
    /// ended it (none, if `to` did).
    fn longest_gap(&self, since: f32, to: f32) -> (f32, f32, Option<&str>) {
        let times = self.new.iter().map(|(at, _, name)| (*at, Some(name.as_str()))).take_while(|&(at, _)| at < to).chain([(to, None)]);
        let mut from = 0.0f32;
        let mut longest = (0.0, since, None);
        for (at, name) in times {
            let start = from.max(since);
            if at > since && at - start > longest.0 {
                longest = (at - start, start, name);
            }
            from = from.max(at);
        }
        longest
    }
}

/// How many of `item` the pack and its pouches carry.
fn holding(c: &Client, item: &str) -> u32 {
    let id = c.content.blocks.expect(item);
    c.inventory.slots.iter().flatten().filter(|s| s.item == id).map(|s| s.count as u32).sum::<u32>() + c.inventory.pouched(id) as u32
}

/// How many of `item` lie loose in the world in sight.
fn loose(c: &Client, item: &str) -> u32 {
    let id = c.content.blocks.expect(item);
    c.items.values().filter(|i| i.stack.item == id).map(|i| i.stack.count as u32).sum()
}

/// The animals in the lane of the plot at `origin`, by id, place and species.
fn lane(c: &Client, origin: IVec3) -> Vec<(u32, Vec3, voxmar_engine::creature::CreatureId)> {
    let (x, z0, z1) = ((origin.x - 9) as f32, (origin.z - 10) as f32, origin.z as f32 + 1.0);
    c.creatures.values().filter(|v| v.position.x >= x && v.position.x < x + 1.0 && v.position.z >= z0 && v.position.z < z1).map(|v| (v.id, v.position, v.species)).collect()
}

/// Whether the block at `cell` is solid in the client's view (water and air are not).
fn solid(c: &Client, cell: IVec3) -> bool {
    c.world.block(cell).is_some_and(|b| c.content.blocks.info(b).solid)
}

/// Across the ground, between two points.
fn across(a: Vec3, b: Vec3) -> f32 {
    (a - b).with_y(0.0).length()
}

impl Fresh {
    fn secs(&self) -> f32 {
        (self.c.ticks() - self.t0) as f32 / TICKS_PER_SECOND as f32
    }

    /// Marks a milestone; one that finishes a quest waits for the book to show it done and
    /// claims its rewards, as a player does (the experience they pay is what later steps'
    /// levels come from).
    fn mark(&mut self, name: &'static str) {
        self.look_round();
        let (at, acts) = (self.secs(), self.acts.map(|t| t as f32 / TICKS_PER_SECOND as f32));
        let c = &self.c;
        let levels = PACED_SKILLS.iter().map(|&skill| (skill, c.content.skills.id(&format!("core:{skill}")).map_or(0, |id| c.content.skills.level(c.skills.xp(id))))).collect();
        self.marks.push(Mark { name, at, acts, levels, side: Vec::new() });
        if let Some(&(_, quest)) = FRESH_QUESTS.iter().find(|(mark, _)| *mark == name) {
            self.claim(quest);
        }
        // And any other the way finished meanwhile, as a player opening the book claims
        // what it shows done.
        let waiting: Vec<String> = self.c.quests.iter().filter(|q| q.done && !q.claimed).map(|q| q.name.trim_start_matches("base:").to_owned()).collect();
        for quest in waiting {
            self.claim(&quest);
            self.marks.last_mut().expect("marked").side.push(quest);
        }
    }

    fn rel(&self, x: i32, y: i32, z: i32) -> IVec3 {
        self.origin + IVec3::new(x, y, z)
    }

    /// The dirt standing above the plot's ground (its level is y -1), as the client sees it
    /// now: what is left of the banks laid out for digging.
    fn raised_dirt(&self) -> Vec<IVec3> {
        let dirt = self.c.content.blocks.id("base:dirt");
        (-13..=30).flat_map(|x| (-11..=13).map(move |z| (x, z))).map(|(x, z)| self.rel(x, 0, z)).filter(|&p| self.c.world.block(p) == dirt).collect()
    }

    /// The middle of `cell`'s floor.
    fn middle(cell: IVec3) -> Vec3 {
        cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5)
    }

    fn have(&self, item: &str) -> u32 {
        holding(&self.c, item)
    }

    /// Runs `run`, and counts the server ticks it took as spent on `act`.
    fn spend<T>(&mut self, act: Act, run: impl FnOnce(&mut Self) -> T) -> T {
        let from = self.c.ticks();
        let made = run(self);
        self.acts[act as usize] += self.c.ticks() - from;
        made
    }

    /// Waits for `done`, at most `limit` server seconds by the tick counter.
    /// The player stands still meanwhile, saying so each tick, which brings the game's clock
    /// to the server: a timed action is done once a message stamped after it arrives.
    fn wait(&mut self, limit: f32, what: &str, done: impl Fn(&Client) -> bool) {
        let finished = self.waited(limit, done);
        assert!(finished, "{what}: not done within {limit} server seconds (now {:.1} s into the run)", self.secs());
    }

    /// As `wait`, saying whether `done` came rather than failing.
    fn waited(&mut self, limit: f32, done: impl Fn(&Client) -> bool) -> bool {
        let until = self.c.ticks() + (limit * TICKS_PER_SECOND as f32) as u64;
        while !done(&self.c) && self.c.ticks() < until {
            self.send();
            self.tick();
        }
        done(&self.c)
    }

    /// Lets one server tick pass, and counts what is new in the pack and the journal.
    fn tick(&mut self) {
        let t = self.c.ticks();
        self.c.pump_until(|c| c.ticks() > t);
        self.steps += 1;
        self.look_round();
    }

    /// Counts what is new since the last look: items in the pack, places and creatures in
    /// the journal.
    fn look_round(&self) {
        use voxmar_engine::journal::Page;
        let (c, at) = (&self.c, self.secs());
        let mut pacing = self.pacing.borrow_mut();
        for stack in c.inventory.slots.iter().flatten() {
            pacing.met(at, Novelty::Item, &c.content.blocks.def(stack.item).name);
        }
        for entry in c.journal.entries() {
            let kind = match entry.page {
                Page::Biomes | Page::Places => Novelty::Place,
                Page::Animals | Page::Monsters | Page::Bosses => Novelty::Creature,
                _ => continue,
            };
            pacing.met(at, kind, &entry.name);
        }
    }

    /// The game's clock: the moment the server's tick stands for, at its pace. The player's
    /// messages are stamped with it (`Client::propose_at`), so what the server times by
    /// their arrival (a break, a cooking, a blow, a stride) takes the game's time however
    /// quickly a test server's ticks come, and the run waits for nothing but ticks.
    fn now(&self) -> Instant {
        self.epoch.1 + Duration::from_secs_f64((self.c.ticks() - self.epoch.0) as f64 / TICKS_PER_SECOND as f64)
    }

    fn propose(&self, action: &str, target: ActionTarget) {
        let c = &self.c;
        let name = |b: BlockId| c.content.blocks.def(b).name.clone();
        let on = match target {
            ActionTarget::Block { block, .. } => c.world.block(block),
            _ => None,
        };
        // What it is done with or to, for telling one action over and over from another:
        // the block or creature it is on, or what is in hand (the food cooked, the stack
        // crafted).
        let creature = match target {
            ActionTarget::Creature(id) => c.creatures.get(&id).map(|v| c.content.creatures.get(v.species).def.name.clone()),
            _ => None,
        };
        let with = creature.unwrap_or_else(|| on.or_else(|| c.inventory.get(self.held as usize).map(|s| s.item)).map_or_else(String::new, name));
        let mut pacing = self.pacing.borrow_mut();
        pacing.act(self.secs(), format!("{action} {with}"));
        if let Some(block) = on.filter(|_| !matches!(action, "core:mine" | "core:place")) {
            let def = c.content.blocks.def(block);
            let station = c.content.stations.of(block).map(|(s, _)| s.empty);
            if let Some(kind) = station.or_else(|| (def.has_tag("station") || def.has_tag("multiblock_core")).then_some(block)) {
                pacing.met(self.secs(), Novelty::Station, &name(kind));
            }
        }
        drop(pacing);
        self.c.propose_at(action, target, self.now());
    }

    fn send(&self) {
        let c = &self.c;
        let input = PlayerInput { teleport: c.teleports, position: self.at, yaw: 0.0, pitch: 0.0, selected_slot: self.held, flying: false };
        c.connection.send_at(ClientMessage::Input(input), self.now());
    }

    /// Walks towards `goal` over the ground, a fifth of a block a tick (four blocks a second,
    /// under the walking speed the server allows), until `stop` blocks from it. Where a
    /// block is in the way, as the table or a fire is, the player turns aside the way a
    /// person would.
    ///
    /// Across the plot (more than eight blocks), or where the walk is caught on the way round
    /// (a pond, a build), the player is teleported beside the goal: the test's shortcut for
    /// travel, since the way round is no part of what it checks.
    fn walk_toward(&mut self, goal: Vec3, stop: f32) {
        if across(goal, self.at) > 8.0 {
            self.hop_near(goal, stop);
        }
        self.spend(Act::Walk, |me| {
            let mut corrections = 0;
            // The closest the walk has come, and the step it came that close: one that has
            // come no closer for a while is caught on the way round (a pond, a build).
            let mut best = (f32::MAX, 0);
            for steps in 0..2000 {
                let to = (goal - me.at).with_y(0.0);
                let left = to.length() - stop;
                // (Coordinates are large: a step under a hundredth of a block changes nothing.)
                if left <= 0.01 {
                    return;
                }
                if left < best.0 - 0.5 {
                    best = (left, steps);
                }
                let want = to.normalize();
                // The first heading, turning aside by degrees, whose step has room for the body.
                let step = left.min(0.2);
                let clear = |me: &Self, heading: Vec3| me.has_room(me.at + heading * step);
                let turn = |degrees: f32| Vec3::new(want.x * degrees.to_radians().cos() - want.z * degrees.to_radians().sin(), 0.0, want.x * degrees.to_radians().sin() + want.z * degrees.to_radians().cos());
                let heading = [0.0, 45.0, -45.0, 90.0, -90.0, 135.0, -135.0].into_iter().map(turn).find(|&h| clear(me, h));
                let Some(heading) = heading.filter(|_| steps < best.1 + 60) else {
                    assert!(me.hop_near(goal, stop), "no way round to {goal:?} from {:?} ({:?} and {:?} from the plot's corner), and no room near it", me.at, goal - me.origin.as_vec3(), me.at - me.origin.as_vec3());
                    best = (f32::MAX, steps);
                    continue;
                };
                me.at += heading * step;
                me.send();
                me.tick();
                // Put back by the server (a hitch, a creature in the way): carry on from there.
                if let Some(back) = me.c.teleported.take() {
                    corrections += 1;
                    assert!(corrections <= 20, "the server keeps moving the player back, to {back:?}, walking to {goal:?}");
                    me.at = back;
                }
            }
            panic!("walked 2000 steps from {:?} and did not reach {goal:?}", me.at);
        })
    }

    /// Teleports to the nearest spot round `goal`, a little inside where a walk to it stops,
    /// with room for the body and a step every way; whether there was one. Where the goal is
    /// in a nook (a corner of a dug pit) with no such spot, any spot inside the stop with room
    /// for the body does.
    fn hop_near(&mut self, goal: Vec3, stop: f32) -> bool {
        let r = (stop - 0.3).max(0.0) + 0.25;
        let room = |me: &Self, p: Vec3| (-2..=2).all(|i| (-2..=2).all(|k| me.has_room(p + Vec3::new(i as f32 * 0.2, 0.0, k as f32 * 0.2))));
        let ring = |r: f32, at: Vec3| (0..24).map(|i| (i as f32 * 15.0).to_radians()).map(move |a| Vec3::new(goal.x + r * a.cos(), at.y, goal.z + r * a.sin()));
        let nearest = |spots: Vec<Vec3>, at: Vec3| spots.into_iter().min_by(|a, b| across(*a, at).total_cmp(&across(*b, at)));
        let roomy = nearest(ring(r, self.at).filter(|&p| room(self, p)).collect(), self.at);
        // Where it is tight, the nearest spot the body just fits, the goal itself among them:
        // a stop too short for any ring (0.05, beside a drop) still has the goal.
        let tight = || nearest([0.0, 0.05, 0.15, 0.25, 0.35, 0.45].into_iter().filter(|&r| r == 0.0 || r < stop).flat_map(|r| ring(r, self.at)).filter(|&p| self.has_room(p)).collect(), self.at);
        let Some(to) = roomy.or_else(tight) else { return false };
        self.c.tp(to);
        self.at = to;
        true
    }

    /// Goes to a drop at `p`: walking up to it on open ground; to the nearest place in reach
    /// of it with room for the body, where it lies with none (under a trunk cut from below);
    /// or by teleport to the nearest bank in reach of one that floated out on a pond (the walk
    /// keeps out of the water, and the way round a pond is no part of what the test checks).
    /// False where nowhere on the ground is in reach of it.
    fn go_near(&mut self, p: Vec3) -> bool {
        let floor = |me: &Self| solid(&me.c, IVec3::new(p.x.floor() as i32, me.origin.y - 1, p.z.floor() as i32));
        if floor(self) && self.has_room(p.with_y(self.at.y)) {
            self.walk_toward(p, 0.5);
            return true;
        }
        if floor(self) {
            let beside = (0..=4)
                .flat_map(|ring| (0..16).map(move |i| (ring as f32 * 0.3, (i as f32 * 22.5).to_radians())))
                .map(|(r, a)| Vec3::new(p.x + r * a.cos(), self.at.y, p.z + r * a.sin()))
                .filter(|&q| self.has_room(q) && self.in_reach(q, p))
                .min_by(|a, b| across(*a, self.at).total_cmp(&across(*b, self.at)));
            let Some(beside) = beside else { return false };
            self.walk_toward(beside, 0.05);
            return true;
        }
        // No floor under it and no bank about it: a drop down a hole rather than out on a
        // pond, for the caller to step down for (or leave).
        let Some(spot) = (1..=4)
            .flat_map(|ring| (0..16).map(move |i| (ring as f32 * 0.4, (i as f32 * 22.5).to_radians())))
            .map(|(r, a)| Vec3::new(p.x + r * a.cos(), self.at.y, p.z + r * a.sin()))
            .filter(|&q| self.has_room(q))
            .min_by(|a, b| across(*a, self.at).total_cmp(&across(*b, self.at)))
        else {
            return false;
        };
        if across(spot, self.at) > 0.3 {
            self.c.tp(spot);
            self.at = spot;
        }
        true
    }

    /// Whether the body has room standing at `at`: air at both its heights, at each corner,
    /// over something solid (the walk keeps out of ponds, where the player would sink).
    fn has_room(&self, at: Vec3) -> bool {
        [(-0.32, -0.32), (-0.32, 0.32), (0.32, -0.32), (0.32, 0.32)].iter().all(|&(dx, dz)| {
            let (x, z) = ((at.x + dx).floor() as i32, (at.z + dz).floor() as i32);
            (0..2).all(|dy| self.c.world.block(IVec3::new(x, self.origin.y + dy, z)) == Some(BlockId::AIR)) && solid(&self.c, IVec3::new(x, self.origin.y - 1, z))
        })
    }

    /// Puts `item` in the hand: on the hotbar already, or swapped onto its last slot.
    fn hold(&mut self, item: &str) {
        // Nothing: bare hands, on a hotbar slot with nothing in it.
        if item.is_empty() {
            let hotbar = voxmar_engine::inventory::HOTBAR_SLOTS;
            let empty = (0..hotbar).find(|&s| self.c.inventory.get(s).is_none()).expect("a free hotbar slot for bare hands");
            self.held = empty as u8;
            self.steady();
            return;
        }
        let id = self.c.content.blocks.expect(item);
        let on = |c: &Client, s: usize| c.inventory.get(s).is_some_and(|x| x.item == id);
        let hotbar = voxmar_engine::inventory::HOTBAR_SLOTS;
        let slot = match (0..hotbar).find(|&s| on(&self.c, s)) {
            Some(slot) => slot,
            None => {
                let from = (hotbar..voxmar_engine::inventory::SLOTS).find(|&s| on(&self.c, s)).unwrap_or_else(|| panic!("no {item} to hold"));
                self.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: hotbar as u8 - 1, count: u8::MAX });
                self.wait(5.0, "bringing it to the hotbar", |c| on(c, hotbar - 1));
                hotbar - 1
            }
        };
        self.held = slot as u8;
        self.steady();
    }

    /// Says where the player stands and what is in hand until the server has taken it: an
    /// input sent before the client heard of a correction (a walk's last step put back, heard
    /// a tick late) is dropped whole, the slot in hand with it, and a place judged by the slot
    /// held before is refused with nothing said.
    fn steady(&mut self) {
        loop {
            if let Some(back) = self.c.teleported.take() {
                self.at = back;
            }
            self.send();
            // (The server tells of a correction before it answers what was sent after it.)
            self.c.settle();
            if self.c.teleported.is_none() {
                return;
            }
        }
    }

    /// Breaks one block with what is in hand, waiting out its time by the server's ticks.
    fn mine(&mut self, block: IVec3) {
        self.propose("core:mine", ActionTarget::Block { block, face: Face::PosY });
        let what = {
            let c = &self.c;
            let mining = c.content.skills.id("core:mining").map(|id| c.content.skills.level(c.skills.xp(id)));
            let block_name = c.world.block(block).map(|b| c.content.blocks.def(b).name.clone());
            format!("mining {block_name:?} at Mining {mining:?} (last refusals {:?})", c.refused.iter().rev().take(3).collect::<Vec<_>>())
        };
        self.spend(Act::Mine, |me| me.wait(30.0, &what, |c| !solid(c, block)));
        self.mined += 1;
    }

    /// Breaks `targets` that lie within the drops' pull of `stands` (visited in turn) with
    /// `tool`, nearest and highest first, until the blocks broken will have given `need` of
    /// `item` (the player counts the drops coming, as one watching them would). Then waits for
    /// them to arrive.
    fn gather_from(&mut self, tool: &str, stands: &[Vec3], mut targets: Vec<IVec3>, item: &str, need: u32) {
        self.make_room_for(item, need, tool);
        self.hold(tool);
        let id = self.c.content.blocks.expect(item);
        let mut owed = self.have(item);
        loop {
            for &stand in stands {
                let near = |t: &IVec3| across(Self::middle(*t), stand) <= 2.3;
                targets.retain(|&t| solid(&self.c, t));
                if owed >= need || !targets.iter().any(near) {
                    continue;
                }
                self.walk_toward(stand, 0.05);
                while owed < need {
                    targets.retain(|&t| solid(&self.c, t));
                    let Some(target) = targets.iter().copied().filter(near).min_by_key(|t| ((across(Self::middle(*t), stand) * 10.0) as i32, -t.y)) else { break };
                    let info = self.c.content.blocks.info(self.c.world.block(target).unwrap());
                    owed += if info.drops == id { info.drop_count as u32 } else { 0 };
                    self.mine(target);
                }
            }
            assert!(owed >= need, "the blocks ran out: {owed} of {need} {item}");
            // Drops lying where no one can stand to reach them are lost, as a player would
            // leave them: more is dug for them.
            match self.collect_reachable(item, need) {
                0 => return,
                lost => owed -= lost,
            }
        }
    }

    /// Cobblestone, as a player makes it: plain stone broken into loose stones (two a block),
    /// knocked four at a time into two cobblestone in the grid, until `need` are in hand.
    fn cobble(&mut self, tool: &str, targets: Vec<IVec3>, need: u32) {
        let (cobble, loose_stone) = ("base:cobblestone", "base:loose_stone");
        let crafts = need.saturating_sub(self.have(cobble)).div_ceil(2);
        if crafts == 0 {
            return;
        }
        self.gather(tool, targets, loose_stone, 4 * crafts);
        self.craft(&[0, 1, 3, 4].map(|cell| (cell, loose_stone, crafts as u8)), cobble, 2 * crafts);
    }

    /// As [`Fresh::gather_from`] over open ground: the player walks up to the nearest block
    /// that is out of the pull and goes on.
    fn gather(&mut self, tool: &str, mut targets: Vec<IVec3>, item: &str, need: u32) {
        self.make_room_for(item, need, tool);
        self.hold(tool);
        let id = self.c.content.blocks.expect(item);
        let mut owed = self.have(item);
        loop {
            while owed < need {
                targets.retain(|&t| solid(&self.c, t));
                let from = self.at;
                let target = targets.iter().copied().min_by_key(|t| ((across(Self::middle(*t), from) * 10.0) as i32, t.y)).expect("blocks to gather from");
                let centre = Self::middle(target);
                if across(centre, self.at) > 2.3 {
                    self.walk_toward(centre, 1.3);
                } else {
                    let info = self.c.content.blocks.info(self.c.world.block(target).unwrap());
                    owed += if info.drops == id { info.drop_count as u32 } else { 0 };
                    self.mine(target);
                }
            }
            // As in `gather_from`: what lies out of every reach is dug again.
            match self.collect_reachable(item, need) {
                0 => return,
                lost => owed -= lost,
            }
        }
    }

    /// As [`Fresh::gather`], for what a block gives only sometimes in place of its own drop
    /// (`instead` in tools.ron: flint from gravel, a stick from leaves): breaks the nearest of
    /// `targets` until `need` of `item` are carried or lie loose, then collects them; as in
    /// `gather`, what lies out of every reach is dug again.
    fn gather_chance(&mut self, tool: &str, mut targets: Vec<IVec3>, item: &str, need: u32) {
        self.make_room_for(item, need, tool);
        self.hold(tool);
        // Only a target that can give `item`, as its drop or `instead`, is broken: another
        // block laid over the plot (the farm's copper) is passed over.
        let id = self.c.content.blocks.expect(item);
        let gives = |c: &Client, t: IVec3| c.world.block(t).is_some_and(|b| c.content.blocks.info(b).drops == id || c.content.tools.rule(b).and_then(|r| r.instead).is_some_and(|(given, _)| given == id));
        // Drops left lying out of every reach, still counted loose.
        let mut lost = 0;
        loop {
            while self.have(item) + loose(&self.c, item) < need + lost {
                targets.retain(|&t| solid(&self.c, t) && gives(&self.c, t));
                let from = self.at;
                let Some(target) = targets.iter().copied().min_by_key(|t| ((across(Self::middle(*t), from) * 10.0) as i32, t.y)) else {
                    panic!("{item}: every block was broken for {} of {need} ({lost} out of reach)", self.have(item) + loose(&self.c, item));
                };
                let centre = Self::middle(target);
                if across(centre, self.at) > 2.3 {
                    self.walk_toward(centre, 1.3);
                } else {
                    self.mine(target);
                }
            }
            match self.collect_reachable(item, need) {
                0 => return,
                short => lost += short,
            }
        }
    }

    /// Waits until `need` of `item` are carried, closing on the nearest loose one until it is
    /// taken: walking to one out of the pull, and onto one in reach that has lain there untaken.
    fn collect(&mut self, item: &str, need: u32) {
        let lost = self.collect_reachable(item, need);
        assert_eq!(lost, 0, "{item}: {lost} lie where no one can stand to reach them, with {} of {need} carried", self.have(item));
    }

    /// As [`Fresh::collect`], stepping down into a pit or hole for a drop that fell below the
    /// ground, as a player does, and back up once it is taken. A drop with nowhere a body
    /// fits in reach of it (a slot one block high) is left lying, as a player leaves it, and
    /// the bot goes on to the others: what is still short of `need` once only such drops
    /// are loose is returned, for the caller to dig again.
    fn collect_reachable(&mut self, item: &str, need: u32) -> u32 {
        let id = self.c.content.blocks.expect(item);
        // Where the bot climbs back to after stepping down for a drop, and the turn it went.
        let mut climb: Option<(Vec3, u64, u32)> = None;
        // Drops left lying where no one can reach them.
        let mut left: Vec<Vec3> = Vec::new();
        // Thirty seconds of the bot's turns (`Fresh::steps`), not the server's ticks.
        let until = self.steps + 30 * TICKS_PER_SECOND as u64;
        let short = self.spend(Act::Mine, |me| {
            // Ticks the nearest drop has lain in the body's reach, by the player's reckoning,
            // without coming in.
            let mut beside = 0;
            // The bot's turn the last drop came in at, and what the bot last did, for a
            // failure to tell.
            let (mut came, mut had) = (me.steps, me.have(item));
            let mut done: std::collections::VecDeque<String> = Default::default();
            while me.have(item) < need {
                assert!(me.steps < until, "{item}: only {} of {need} came in, {} loose, nearest {:?} from {:?} (last put at {:?}), lately {:?}; carrying {:?}", me.have(item), loose(&me.c, item), me.c.items.values().filter(|i| i.stack.item == id).map(|i| i.position).min_by(|a, b| across(*a, me.at).total_cmp(&across(*b, me.at))), me.at, me.c.position, done, (0..voxmar_engine::inventory::SLOTS).filter_map(|s| me.c.inventory.get(s).map(|x| (me.c.content.blocks.def(x.item).name.clone(), x.count))).collect::<Vec<_>>());
                let did = |what: String, done: &mut std::collections::VecDeque<String>| {
                    if done.len() == 8 {
                        done.pop_front();
                    }
                    done.push_back(what);
                };
                if me.have(item) != had {
                    (came, had) = (me.steps, me.have(item));
                }
                // Back up out of the pit once the drop came in, or a second on if it did not.
                if let Some((back, went, then)) = climb
                    && (me.have(item) > then || me.steps > went + TICKS_PER_SECOND as u64)
                {
                    climb = None;
                    did(format!("climbed back to {back:.2} from {:.2}", me.at), &mut done);
                    me.c.tp(back);
                    me.at = back;
                }
                // Put back by the server after a walk ended (its last step refused, heard a tick
                // late): the server picks up where it has the player, so the player goes on from
                // there rather than waiting beside a drop it has them nowhere near.
                if let Some(back) = me.c.teleported.take() {
                    did(format!("put back at {back:.2} from {:.2}", me.at), &mut done);
                    me.at = back;
                }
                let at = me.at;
                let reachable = |p: &Vec3| !left.iter().any(|l| l.distance(*p) < 0.3);
                let near = me.c.items.values().filter(|i| i.stack.item == id).map(|i| i.position).filter(reachable).min_by(|a, b| across(*a, at).total_cmp(&across(*b, at)));
                // Only drops no one can reach are loose: what is still short is the caller's to dig.
                if near.is_none() && climb.is_none() && !left.is_empty() && me.c.items.values().any(|i| i.stack.item == id) {
                    return need - me.have(item);
                }
                // Nothing in for three seconds with one loose: put by the server where it lies,
                // the test's shortcut where closing on it has not done (where the server has
                // the player, not the bot, is what takes it). One that fell below the ground
                // is reached from inside its pit, and one with no room for a body by it is left.
                if let Some(p) = near
                    && climb.is_none()
                    && me.steps > came + 3 * TICKS_PER_SECOND as u64
                {
                    came = me.steps;
                    let on = p.with_y(me.at.y);
                    let floor = me.at;
                    if me.in_reach(on, p) && me.has_room(on) {
                        me.c.tp(on);
                        me.at = on;
                        did(format!("hopped to {:.2} for the drop at {p:.2}", me.at), &mut done);
                    } else if me.in_reach(p.with_y(me.at.y), p) && me.hop_near(p, 0.5) {
                        did(format!("hopped to {:.2} for the drop at {p:.2}", me.at), &mut done);
                    } else if let Some(down) = me.room_in_reach(p) {
                        climb = Some((floor, me.steps, me.have(item)));
                        me.c.tp(down);
                        me.at = down;
                        did(format!("stepped down to {down:.2} for the drop at {p:.2}"), &mut done);
                    } else {
                        left.push(p);
                        did(format!("left the drop at {p:.2}: no room for a body in reach of it"), &mut done);
                    }
                    me.send();
                    me.tick();
                    continue;
                }
                match near {
                    // Walked up to rather than left to the pull, which stops at blocks: a drop
                    // with a trunk or a pit's side between would lie there, in the pull's range
                    // and out of it at once. Within 0.8 the body's reach takes it, walls or not.
                    Some(p) if across(p, me.at) > 0.8 => {
                        beside = 0;
                        let from = me.at;
                        if me.go_near(p) {
                            did(format!("went from {from:.2} to {:.2} for the drop at {p:.2}", me.at), &mut done);
                        } else {
                            // Nowhere on the ground in reach of it: straight to stepping down
                            // for it, or leaving it.
                            came = me.steps.saturating_sub(3 * TICKS_PER_SECOND as u64 + 1);
                            did(format!("found no room on the ground in reach of the drop at {p:.2}"), &mut done);
                        }
                    }
                    // In reach and not taken for half a second: stepped onto, on open ground.
                    Some(p) if beside >= TICKS_PER_SECOND / 2 && solid(&me.c, IVec3::new(p.x.floor() as i32, me.origin.y - 1, p.z.floor() as i32)) => {
                        beside = 0;
                        me.walk_toward(p, 0.3);
                        did(format!("stepped to {:.2} onto the drop at {p:.2}", me.at), &mut done);
                    }
                    Some(_) => beside += 1,
                    None => beside = 0,
                }
                // Standing there, as a client says each tick: the server judges the pickup by
                // its own reckoning of where the player is, and corrects one it disagrees with.
                me.send();
                me.tick();
            }
            if let Some((back, _, _)) = climb {
                me.c.tp(back);
                me.at = back;
            }
            0
        });
        short
    }

    /// Whether a body standing at `at` takes a drop at `p`: the server's reach is the body's
    /// box grown a block across and half a block up and down.
    fn in_reach(&self, at: Vec3, p: Vec3) -> bool {
        let (half, size) = (0.3, voxmar_engine::dropped::SIZE * 0.5);
        (p.x - at.x).abs() <= half + 1.0 + size && (p.z - at.z).abs() <= half + 1.0 + size && p.y + size >= at.y - 0.5 && p.y - size <= at.y + 1.8 + 0.5
    }

    /// The nearest place below the ground, down to the drop's own level, where a body stands
    /// (two cells clear of solid blocks, water allowed, over solid ground at each corner) and
    /// takes the drop at `p`: the bottom of the pit or hole it fell into.
    fn room_in_reach(&self, p: Vec3) -> Option<Vec3> {
        let stands = |q: Vec3| {
            [(-0.32, -0.32), (-0.32, 0.32), (0.32, -0.32), (0.32, 0.32)].iter().all(|&(dx, dz)| {
                let (x, y, z) = ((q.x + dx).floor() as i32, q.y as i32, (q.z + dz).floor() as i32);
                (0..2).all(|dy| !solid(&self.c, IVec3::new(x, y + dy, z))) && solid(&self.c, IVec3::new(x, y - 1, z))
            })
        };
        (p.y.floor() as i32 - 1..self.origin.y)
            .flat_map(|y| [0.0f32, 0.35, 0.7, 1.0].into_iter().flat_map(move |r| (0..16).map(move |i| (y, r, (i as f32 * 22.5).to_radians()))))
            .map(|(y, r, a)| Vec3::new(p.x + r * a.cos(), y as f32, p.z + r * a.sin()))
            .filter(|&q| self.in_reach(q, p) && stands(q))
            .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
    }

    /// Lays `cells` (grid cell, item, how many) in the crafting grid and crafts as often as
    /// they allow, waiting for `made` more of `makes`; leftovers go back to the pack.
    fn craft(&mut self, cells: &[(usize, &str, u8)], makes: &str, made: u32) {
        let before = self.have(makes);
        let slots = voxmar_engine::inventory::SLOTS;
        let mut left: Vec<u32> = (0..slots).map(|s| self.c.inventory.get(s).map_or(0, |x| x.count as u32)).collect();
        for &(cell, item, count) in cells {
            let id = self.c.content.blocks.expect(item);
            // From one stack, or topped up from the next where one holds too few.
            let mut wanted = count as u32;
            while wanted > 0 {
                let from = (0..slots)
                    .filter(|&s| self.c.inventory.get(s).is_some_and(|x| x.item == id) && left[s] > 0)
                    .max_by_key(|&s| left[s].min(wanted))
                    .unwrap_or_else(|| panic!("not enough {item} for {makes}: {count} wanted"));
                let take = left[from].min(wanted);
                left[from] -= take;
                wanted -= take;
                self.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (voxmar_engine::inventory::CRAFT + cell) as u8, count: take as u8 });
            }
        }
        self.propose("core:craft", ActionTarget::Slot(0));
        self.wait(10.0, &format!("crafting {makes}"), |c| holding(c, makes) >= before + made);
        self.propose("core:stow", ActionTarget::None);
        self.wait(10.0, "clearing the grid", |c| c.inventory.craft.iter().all(Option::is_none));
    }

    /// Goes near the crafting table at `table` and opens it.
    fn at_table(&mut self, table: IVec3) {
        self.walk_toward(Self::middle(table), 3.0);
        self.propose("base:use_table", ActionTarget::Block { block: table, face: Face::PosY });
    }

    /// Sets down what is in hand on top of `ground`, which must be in reach: a piece of a
    /// structure, which goes there or nowhere.
    fn place(&mut self, item: &str, ground: IVec3) -> IVec3 {
        self.place_on(item, &[ground])
    }

    /// Sets down what is in hand on top of `ground`, or, a creature in the way or the server
    /// refusing it, beside it: a thing that stands alone (a table, a fire, a chest). Where
    /// it went.
    fn place_near(&mut self, item: &str, ground: IVec3) -> IVec3 {
        self.place_on(item, &[ground, ground + IVec3::X, ground - IVec3::X, ground + IVec3::Z, ground - IVec3::Z])
    }

    /// Whether a creature the client is shown takes up any of `cell`, by the server's own
    /// rule (`motion::takes_cell`).
    fn creature_on(c: &Client, cell: IVec3) -> bool {
        c.creatures.values().any(|v| {
            let size = c.content.creatures.get(v.species).def.body.size;
            voxmar_engine::creature::motion::takes_cell(v.position, (size.0 * v.scale, size.1 * v.scale), cell)
        })
    }

    /// Sets down what is in hand on the first of `grounds` it goes onto, trying the next on a
    /// creature in the way or a refusal; the last waits for the creature to go, and must take.
    fn place_on(&mut self, item: &str, grounds: &[IVec3]) -> IVec3 {
        self.hold(item);
        for (i, &ground) in grounds.iter().enumerate() {
            let last = i + 1 == grounds.len();
            let cell = ground + IVec3::Y;
            // Open as the place action finds it: air, or a plant a block replaces
            // (not water), on solid ground.
            let open = |c: &Client| {
                c.world.block(cell).is_some_and(|b| c.content.blocks.info(b).gives_room())
                    && c.world.block(ground).is_some_and(|b| c.content.blocks.def(b).solid)
            };
            if !last && (!open(&self.c) || Self::creature_on(&self.c, cell)) {
                continue;
            }
            // A creature standing there keeps a block out (`Unobstructed`): set it down once
            // it has wandered off the cell, as the server shows it.
            if Self::creature_on(&self.c, cell) {
                let off = self.waited(5.0, |c| !Self::creature_on(c, cell));
                assert!(off, "placing {item} on {ground}: a creature stood on the cell for 5 server seconds");
            }
            let refused = self.c.refused.len();
            // Set is what was not there before: a grass tuft in the cell is not the piece.
            let before = self.c.world.block(cell);
            self.propose("core:place", ActionTarget::Block { block: ground, face: Face::PosY });
            let set = |c: &Client| c.world.block(cell).is_some_and(|b| b != BlockId::AIR && Some(b) != before);
            let placed = self.waited(5.0, |c| set(c) || c.refused.len() > refused);
            if !set(&self.c) && !last && self.c.refused.len() > refused {
                continue;
            }
            assert!(
                placed && set(&self.c),
                "placing {item} on {ground}: not done within 5 server seconds (now {:.1} s into the run), from {:.2} ({:.2} from the eye to the cell), ground {:?} and cell {:?} as the client sees them, holding slot {} ({:?}), refused since: {:?}",
                self.secs(),
                self.at,
                (self.at + Vec3::Y * voxmar_engine::physics::player::EYE_HEIGHT).distance(cell.as_vec3() + 0.5),
                self.c.world.block(ground).map(|b| self.c.content.blocks.def(b).name.clone()),
                self.c.world.block(cell).map(|b| self.c.content.blocks.def(b).name.clone()),
                self.held,
                self.c.inventory.get(self.held as usize).map(|s| self.c.content.blocks.def(s.item).name.clone()),
                &self.c.refused[refused.min(self.c.refused.len())..]
            );
            return cell;
        }
        unreachable!("no ground to place {item} on")
    }

    /// Cooks one `food` at `station` (two seconds); whether it came out as `cooked`, not burnt.
    fn cook(&mut self, station: IVec3, food: &str, cooked: &str) -> bool {
        self.hold(food);
        let (raw, done) = (self.have(food), self.have(cooked));
        self.propose("base:cook", ActionTarget::Block { block: station, face: Face::PosY });
        // A spoiled attempt can give the food back (a cracked clay is clay again), so the
        // count alone does not say it ended: it ended once the two seconds plus a second's
        // slack of server ticks have passed, or the cooked food arrived.
        let over = self.c.ticks() + 3 * TICKS_PER_SECOND as u64;
        self.spend(Act::Cook, |me| me.wait(10.0, "cooking", |c| holding(c, cooked) > done || holding(c, food) < raw || c.ticks() >= over));
        self.have(cooked) > done
    }

    /// Puts an animal in the lane at `z`. (/summon puts the first of a group 1.8 blocks west
    /// of the spot it is given.)
    fn stock(&mut self, species: &str, z: i32) {
        let at = self.rel(-7, 0, z);
        self.c.ok(&format!("/summon {species} 1 {} {} {}", at.x, at.y, at.z));
    }

    /// Hunts the lane's animals, walking in from its open end after the nearest of the kind
    /// `prefer` names (any kind, if it names none), until `done`. With the lane empty, a few
    /// more are brought in (the wild would hold more than a lane of them).
    fn hunt(&mut self, prefer: impl Fn(&Client) -> &'static str, done: impl Fn(&Client) -> bool) {
        let (mouth, lane_x) = (Self::middle(self.rel(-9, 0, 2)), self.rel(-9, 0, 0).x as f32 + 0.5);
        let mut swings = 0;
        let hand = if self.have("base:stone_axe") > 0 { "base:stone_axe" } else { "" };
        self.hold(hand);
        // In by the open end.
        if (self.at.x - lane_x).abs() > 0.5 {
            self.walk_toward(mouth, 0.05);
        }
        while !done(&self.c) {
            swings += 1;
            assert!(swings <= 400, "hunting took over 400 swings: at {:?}, lane {:?}, blows {} damage {:?}", self.at, lane(&self.c, self.origin), self.c.blows, self.c.damage);
            let (at, wanted) = (self.at, self.c.content.creatures.find(prefer(&self.c)));
            let Some((id, p, _)) = lane(&self.c, self.origin).into_iter().min_by(|a, b| {
                let key = |x: &(u32, Vec3, voxmar_engine::creature::CreatureId)| (Some(x.2) != wanted, (x.1.z - at.z).abs());
                key(a).0.cmp(&key(b).0).then(key(a).1.total_cmp(&key(b).1))
            }) else {
                for (species, z) in [("rabbit", -10), ("junglefowl", -8), ("junglefowl", -6), ("rabbit", -4)] {
                    self.stock(species, z);
                }
                self.restocks += 1;
                let origin = self.origin;
                self.wait(5.0, "animals in the lane", |c| !lane(c, origin).is_empty());
                continue;
            };
            // Up close behind it, in the lane.
            if (p.z - at.z).abs() > 1.6 {
                self.walk_toward(Vec3::new(lane_x, at.y, p.z + 1.3), 0.1);
            }
            self.propose("core:attack", ActionTarget::Creature(id));
            self.swung.push(id);
            // One swing a beat, and on to the next once it is down.
            let t = self.c.ticks();
            self.spend(Act::Hunt, |me| {
                while me.c.ticks() < t + 14 && me.c.creatures.contains_key(&id) {
                    me.send();
                    me.tick();
                }
            });
        }
    }

    /// Builds the plot beside a player who has just joined, and starts the clock. Coordinates
    /// are from the player's cell: +x east, +z south, ground at y -1.
    fn lay_out(mut c: Client) -> Self {
        let origin = c.position.floor().as_ivec3();
        let v = IVec3::new;
        // Level ground, clear of whatever grew there.
        c.fill(origin, v(-13, 0, -11), v(20, 10, 13), "air");
        c.fill(origin, v(-13, -4, -11), v(20, -1, 13), "dirt");
        // Four oak trunks, four logs each.
        for x in [1, 3, 5, 7] {
            c.fill(origin, v(x, 0, -5), v(x, 3, -5), "oak_log");
        }
        // Bushes of oak leaves along the plot's south edge, each a column four high with two
        // blocks to walk between (a drop under leaves left standing is in reach from either
        // side), and a dead snag at their end as the spawn kit lays one, for the sticks bare
        // hands pull from leaves and snags, and a gravel bank beside
        // them in two rows for flint, with a lane two wide between them, beyond the plot's edge
        // so they stand clear of the kiln's site and of the copper's drops: a new player's
        // first tools are flint on sticks.
        for x in (-12..=9).step_by(3) {
            c.fill(origin, v(x, 0, 12), v(x, 3, 12), "oak_leaves");
        }
        c.fill(origin, v(11, 0, 12), v(11, 2, 12), "dead_log");
        c.fill(origin, v(9, 0, 14), v(25, 10, 17), "air");
        c.fill(origin, v(9, -4, 14), v(25, -1, 17), "dirt");
        // A hundred and twenty gravel for the six flint the bot needs: at one flint in eight a
        // key finds fifteen on average, and about 1 key in 580 finds fewer than six (ninety
        // left about 1 in 40 short).
        for z in [14, 17] {
            c.fill(origin, v(10, 0, z), v(24, 3, z), "gravel");
        }
        // A patch of natural stone one block high, with two coal ores on its near edge; beyond
        // it nine more standing apart in two rows, with room to walk between, and a sample of
        // coal lying on the grass.
        c.fill(origin, v(9, 0, -3), v(17, 0, 4), "stone");
        c.fill(origin, v(9, 0, 3), v(9, 0, 4), "coal_ore");
        for (x, z) in FRESH_COAL {
            c.fill(origin, v(x, 0, z), v(x, 0, z), "coal_ore");
        }
        c.fill(origin, v(14, 0, 8), v(14, 0, 8), "coal_ore_sample");
        // Granite, the hard rock stone heads are ground from, in a far corner: nine blocks
        // one high, so what is left over stands out of the way. And a block of sand, for the
        // grindstone's dressing.
        c.fill(origin, v(18, 0, -10), v(20, 0, -8), "granite");
        c.fill(origin, v(3, 0, 1), v(3, 0, 1), "sand");
        // A pond two blocks wide, one block of water over two of clay.
        c.fill(origin, v(-2, -3, 2), v(-1, -2, 11), "clay");
        c.fill(origin, v(-2, -1, 2), v(-1, -1, 11), "water");
        // A lane two blocks high and one wide, closed at its far end and open at the near one,
        // where the animals cannot get past the player.
        c.fill(origin, v(-10, 0, -11), v(-8, 1, 0), "stone");
        c.fill(origin, v(-9, 0, -10), v(-9, 1, 0), "air");
        // Wild wheat growing by the lane's mouth, for A Tempting Bite.
        c.fill(origin, v(-7, 0, 2), v(-7, 0, 2), "wild_wheat");
        let probes = [(v(7, 3, -5), "base:oak_log"), (v(17, 0, 4), "base:stone"), (v(-2, -1, 11), "base:water"), (v(-8, 1, 0), "base:stone"), (v(-7, 0, 2), "base:wild_wheat"), (v(9, 3, 12), "base:oak_leaves"), (v(11, 2, 12), "base:dead_log"), (v(24, 2, 17), "base:gravel"), (v(20, 0, -8), "base:granite"), (v(3, 0, 1), "base:sand")];
        c.pump_until(|c| probes.iter().all(|&(at, name)| c.world.block(origin + at) == c.content.blocks.id(name)) && c.world.block(origin + v(-9, 1, 0)) == Some(BlockId::AIR));
        let (at, t0) = (c.position, c.ticks());
        let epoch = (t0, Instant::now());
        let mut plot = Self { c, origin, at, held: 0, t0, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch, steps: 0, swung: Vec::new(), pacing: Default::default() };
        // Ten animals in the lane: three rabbits and seven junglefowl.
        for (i, z) in (-10..=-1).enumerate() {
            plot.stock(if i % 3 == 1 { "rabbit" } else { "junglefowl" }, z);
        }
        plot.wait(10.0, "ten animals in the lane", |c| lane(c, origin).len() >= 10);
        // The clock starts when the player can act, with the animals standing in the lane.
        plot.t0 = plot.c.ticks();
        plot
    }
}

/// The bot's pickup (`Fresh::collect`) goes on from where the server has the player: put
/// back after a walk ended (its last step refused, heard a tick late), it closes on the drop
/// again rather than standing on it by its own reckoning while the server has it out of reach.
#[test]
fn the_bot_closes_on_a_drop_from_where_the_server_put_it() {
    lockstep("a bot closing on a drop");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    // A cobblestone four blocks off, beyond the pull.
    let cell = origin + IVec3::X * 4;
    f.c.ok(&format!("/drop cobblestone 1 {} {} {}", cell.x, cell.y, cell.z));
    f.c.pump_until(|c| loose(c, "base:cobblestone") == 1);
    // The player thinks they stand on it; the server has them where they were, and says so.
    f.at = f.c.items.values().next().expect("the drop").position;
    f.c.ok(&format!("/tp {:?} {:?} {:?}", feet.x, feet.y, feet.z));
    f.collect("base:cobblestone", 1);
    assert_eq!(f.have("base:cobblestone"), 1);
}

/// The bot's pickup (`Fresh::collect`) is bounded by its own turns, not the server's ticks:
/// on a busy machine a quick server ran the kiln run's thirty seconds of ticks while the bot
/// walked to one drop, and the try ended with the last cobblestone lying five blocks off.
/// Here the server ticks as quick as it can, far ahead of the bot, and the bot still walks
/// to each of six drops laid four blocks apart.
#[test]
fn the_bot_collects_while_the_server_ticks_far_ahead_of_it() {
    quicker(100_000, "as quick as the server can tick, to outrun the bot's turns");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    for i in 1..=6 {
        let cell = origin + IVec3::X * (4 * i);
        f.c.ok(&format!("/drop cobblestone 1 {} {} {}", cell.x, cell.y, cell.z));
    }
    f.c.pump_until(|c| loose(c, "base:cobblestone") == 6);
    f.collect("base:cobblestone", 6);
    assert_eq!(f.have("base:cobblestone"), 6);
}

/// In lockstep the server ticks once for each tick its bot lets pass, however quick it may
/// tick and however slowly the bot goes: never a tick the bot did not see.
#[test]
fn a_server_in_lockstep_ticks_once_for_each_tick_its_bot_lets_pass() {
    lockstep("counting the server's ticks against the bot's");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    for _ in 0..200 {
        let t = f.c.ticks();
        f.send();
        f.tick();
        assert_eq!(f.c.ticks(), t + 1, "the server ticked past what the bot let pass");
    }
}

/// A drop that fell below the ground (dug clay that rolled into the hole it came from, on a
/// busy Mac: "no room near the drop" two blocks under the floor) is taken from inside its
/// pit: the bot steps down into it, as a player would, and climbs back out.
#[test]
fn the_bot_steps_down_into_a_pit_for_a_drop_and_back_out() {
    lockstep("a bot stepping into a pit for a drop");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    // Ground under the floor, then a pit four blocks off, one wide and three deep, a
    // cobblestone at its bottom.
    f.c.fill(origin, IVec3::new(2, -5, -2), IVec3::new(6, -2, 2), "stone");
    let pit = origin + IVec3::X * 4;
    f.c.pump_until(|c| c.world.block(pit - IVec3::Y * 4) == c.content.blocks.id("base:stone"));
    for dy in 1..=3 {
        f.c.ok(&format!("/setblock {} {} {} air", pit.x, pit.y - dy, pit.z));
    }
    f.c.pump_until(|c| c.world.block(pit - IVec3::Y * 3) == Some(BlockId::AIR));
    let bottom = pit - IVec3::Y * 3;
    f.c.ok(&format!("/drop cobblestone 1 {} {} {}", bottom.x, bottom.y, bottom.z));
    f.c.pump_until(|c| c.items.values().any(|i| i.position.y < (origin.y - 2) as f32));
    f.collect("base:cobblestone", 1);
    assert_eq!(f.have("base:cobblestone"), 1);
    assert_eq!(f.at.y, feet.y, "back out of the pit on the ground");
}

/// A drop under a trunk cut from below lies in a one-high gap, where the body has no room:
/// the bot walks up beside it, in reach, rather than onto it (the kiln bot, gathering logs
/// beside a trunk it had just cut: "no way round ... and no room near it").
#[test]
fn the_bot_takes_a_drop_under_a_cut_trunk_from_beside_it() {
    lockstep("a bot taking a drop under a trunk");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    // A log hanging a block over the ground four blocks off, a cobblestone in the gap under it.
    let gap = origin + IVec3::X * 4;
    f.c.ok(&format!("/setblock {} {} {} oak_log", gap.x, gap.y + 1, gap.z));
    f.c.pump_until(|c| c.world.block(gap + IVec3::Y) == c.content.blocks.id("base:oak_log"));
    f.c.ok(&format!("/drop cobblestone 1 {} {} {}", gap.x, gap.y, gap.z));
    f.c.pump_until(|c| loose(c, "base:cobblestone") == 1);
    f.collect("base:cobblestone", 1);
    assert_eq!(f.have("base:cobblestone"), 1);
}

/// A spot the body just fits, off the middle of a one-wide nook, is somewhere to hop to: the
/// walk's way round, caught, ends there rather than finding no room (the Nomad bot, going
/// to a drop beside a trunk: "no way round ... and no room near it").
#[test]
fn the_bot_hops_to_a_spot_it_just_fits_in_a_one_wide_nook() {
    lockstep("a bot hopping into a one-wide nook");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    // Walls either side of a row of cells four to five blocks off, shut at its far end.
    f.c.fill(origin, IVec3::new(3, 0, -1), IVec3::new(6, 1, -1), "stone");
    f.c.fill(origin, IVec3::new(3, 0, 1), IVec3::new(6, 1, 1), "stone");
    f.c.fill(origin, IVec3::new(6, 0, 0), IVec3::new(6, 1, 0), "stone");
    let shut = origin + IVec3::new(6, 1, 0);
    f.c.pump_until(|c| c.world.block(shut) == c.content.blocks.id("base:stone"));
    let goal = Vec3::new(origin.x as f32 + 5.4, feet.y, origin.z as f32 + 0.45);
    assert!(f.has_room(goal), "the body fits at {goal:?}");
    assert!(f.hop_near(goal, 0.05), "no room near {goal:?}, where the body fits");
    assert!(across(f.at, goal) <= 0.05, "hopped to {:?}, not within 0.05 of {goal:?}", f.at);
}

/// A drop with no room for a body anywhere in reach of it (a slot one block high under the
/// ground) is left lying, as a player leaves it: the bot takes the others, and the gather
/// it was for digs another block for the one lost rather than waiting it out.
#[test]
fn the_bot_leaves_a_drop_no_one_can_reach_and_digs_another() {
    lockstep("a bot leaving a drop it cannot reach");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    // Ground under the floor, and in it a one-high slot two blocks down, three blocks off,
    // a cobblestone in it.
    f.c.fill(origin, IVec3::new(1, -5, -2), IVec3::new(5, -2, 2), "stone");
    let slot = origin + IVec3::new(3, -3, 0);
    f.c.pump_until(|c| c.world.block(slot) == c.content.blocks.id("base:stone"));
    f.c.ok(&format!("/setblock {} {} {} air", slot.x, slot.y, slot.z));
    f.c.pump_until(|c| c.world.block(slot) == Some(BlockId::AIR));
    f.c.ok(&format!("/drop cobblestone 1 {} {} {}", slot.x, slot.y, slot.z));
    f.c.pump_until(|c| loose(c, "base:cobblestone") == 1);
    // Two cobblestone blocks to break: the two dug must make three, with the one in the slot.
    f.c.ok("/give stone_pickaxe 1");
    f.c.pump_until(|c| holding(c, "base:stone_pickaxe") == 1);
    let stones: Vec<IVec3> = (0..3).map(|i| origin + IVec3::new(8 + 2 * i, 0, 2)).collect();
    for s in &stones {
        f.c.ok(&format!("/setblock {} {} {} cobblestone", s.x, s.y, s.z));
    }
    f.c.pump_until(|c| stones.iter().all(|&s| c.world.block(s) == c.content.blocks.id("base:cobblestone")));
    let have = f.have("base:cobblestone");
    f.gather("base:stone_pickaxe", stones.clone(), "base:cobblestone", have + 3);
    assert_eq!(f.have("base:cobblestone"), have + 3);
    assert!(stones.iter().all(|&s| !solid(&f.c, s)), "all three dug: two owed, and one more for the one left in the slot");
    assert_eq!(loose(&f.c, "base:cobblestone"), 1, "the one in the slot still lies there");
}

/// The bot's hand is the server's before it places: a move the server corrected, heard
/// after the bot changed slots, dropped that change, and the place was judged by the
/// pickaxe still in hand and refused unseen.
#[test]
fn the_bot_places_from_the_slot_it_holds_after_a_correction() {
    lockstep("a bot placing after a correction");
    let c = on_a_test_floor();
    let (feet, ticks) = (c.position, c.ticks());
    let origin = feet.floor().as_ivec3();
    let mut f = Fresh { c, origin, at: feet, held: 0, t0: ticks, marks: Vec::new(), mined: 0, acts: [0; 4], restocks: 0, epoch: (ticks, Instant::now()), steps: 0, swung: Vec::new(), pacing: Default::default() };
    f.c.ok("/give stone_pickaxe 1");
    f.c.ok("/give cobblestone 1");
    f.c.pump_until(|c| holding(c, "base:stone_pickaxe") == 1 && holding(c, "base:cobblestone") == 1);
    f.hold("base:stone_pickaxe");
    // The server moves the player (as it puts back a walk's last step) before it reads the
    // bot's next input, which the bot sends without having heard of it.
    f.c.connection.send(ClientMessage::Command { line: format!("/tp {:?} {:?} {:?}", feet.x, feet.y, feet.z) });
    f.place("base:cobblestone", origin + IVec3::new(2, -1, 0));
}

/// The fresh player's rolls (sticks from leaves, flint from gravel, burnt meat): one key, so
/// the plot plays the same each run and a failure can be run again.
const FRESH_DRAWS: u64 = 0x0f1a;

/// Takes 3 to 4 minutes (it plays real server time), too slow for every run: use
/// `cargo test -p voxmar_server --test gameplay a_fresh_survival -- --ignored --nocapture`
/// after changing early-age content, quests or recipes, and read the milestone block.
#[test]
#[ignore = "plays 3 to 4 minutes of real server time; run it after changing early-age content"]
fn a_fresh_survival_player_reaches_each_nomad_milestone() {
    real_time("it measures the early game's pace in server seconds, and a hunt's blows land by the clock");
    let c = Client::join_with(ServerConfig { view_distance: 2, draw_key: Some(FRESH_DRAWS), ..Default::default() });
    let mut f = Fresh::lay_out(c);
    the_first_days(&mut f);
    finish(f)
}

/// What the first days leave standing on the plot, which the Farm age comes back to.
#[derive(Clone)]
struct Site {
    table: IVec3,
    /// The pit kiln.
    kiln: IVec3,
    /// The ponds' clay and the banks it is dug from.
    bed: Vec<IVec3>,
    banks: Vec<Vec3>,
}

/// The Nomad age's main line, played with legitimate actions only, and the Farm age's first
/// fire: from bare hands to twelve fired clay.
fn the_first_days(f: &mut Fresh) -> Site {
    let v = IVec3::new;
    let up = IVec3::Y;
    let rel = |f: &Fresh, cells: Vec<IVec3>| -> Vec<IVec3> { cells.into_iter().map(|p| f.origin + p).collect() };
    let (cobble, clay, stick, log) = ("base:cobblestone", "base:clay", "base:stick", "base:oak_log");

    // Flint tools, with bare hands (a new player starts with nothing, and a bare hand fells
    // no tree and breaks no stone): sticks pulled from leaves and a snag, flint from gravel
    // (two kept for An Axe to It's spare), a flint axe and pickaxe made in the hand's grid.
    let leaves = (-12..=9).step_by(3).flat_map(|x| (0..=3).map(move |y| v(x, y, 12)));
    let (flint, sticks) = ("base:flint", rel(f, leaves.chain((0..=2).map(|y| v(11, y, 12))).collect()));
    f.gather_chance("", sticks, stick, 3);
    let gravel = rel(f, (10..=24).flat_map(|x| (0..=3).flat_map(move |y| [14, 17].map(move |z| v(x, y, z)))).collect());
    f.gather_chance("", gravel, flint, 6);
    f.craft(&[(0, flint, 1), (1, flint, 1), (3, stick, 1)], "base:flint_axe", 1);
    f.craft(&[(0, flint, 1), (1, flint, 1), (3, stick, 1), (4, stick, 1)], "base:flint_pickaxe", 1);
    f.mark("flint_tools");

    // Timber, with the flint axe: four logs, for the planks that make the table, the sticks
    // and the next tools.
    let trunks = rel(f, [1, 3, 5, 7].into_iter().flat_map(|x| (0..4).map(move |y| v(x, y, -5))).collect());
    f.gather("base:flint_axe", trunks.clone(), log, 4);
    f.mark("timber");
    f.craft(&[(0, log, 4)], "base:oak_planks", 16);

    // A crafting table from four planks, set down where it serves the stone and the fire, and
    // sticks from planks.
    let table = f.rel(6, 0, -1);
    f.craft(&[(0, "base:oak_planks", 1), (1, "base:oak_planks", 1), (3, "base:oak_planks", 1), (4, "base:oak_planks", 1)], "base:crafting_table", 1);
    f.mark("crafting_table");
    f.walk_toward(Fresh::middle(table), 3.0);
    let table = f.place_near("base:crafting_table", table - up);
    f.craft(&[(0, "base:oak_planks", 3), (3, "base:oak_planks", 3)], stick, 12);
    f.at_table(table);

    // An Axe to It: a spare flint axe, and the twelve logs the campfires and later planks
    // want chopped with it.
    f.craft(&[(0, flint, 1), (1, flint, 1), (3, stick, 1)], "base:flint_axe", 1);
    f.gather("base:flint_axe", trunks, log, 12);
    f.mark("an_axe_to_it");

    // Black Flecks: the sample of coal on the grass, lifted with the flint pickaxe (the seam
    // it marks wants a stone one).
    let sample = f.rel(14, 0, 8);
    f.walk_toward(Fresh::middle(f.rel(14, 0, 9)), 0.3);
    f.hold("base:flint_pickaxe");
    f.propose("core:mine", ActionTarget::Block { block: sample, face: Face::PosY });
    f.spend(Act::Mine, |me| me.wait(30.0, "lifting the sample of coal", |c| c.world.block(sample) == Some(BlockId::AIR)));
    f.collect("base:coal_ore", 1);
    f.mark("black_flecks");

    // Stone in Hand: cobblestone with the flint pickaxe (loose stones knocked together); sand by hand, and a grindstone (a
    // wheel of cobble dressed with sand, on two sticks) set down by the table; granite, the
    // hard rock, ground on it to seven stone heads: a stone pickaxe and axe hafted from six,
    // the seventh kept for the shovel.
    let stone = rel(f, (9..=17).flat_map(|x| (-3..=4).map(move |z| v(x, 0, z))).filter(|p| !(p.x == 9 && p.z >= 3)).collect());
    f.cobble("base:flint_pickaxe", stone.clone(), 3);
    let (sand, granite, head) = ("base:sand", "base:cobbled_granite", "base:stone_head");
    f.gather("base:flint_pickaxe", vec![f.rel(3, 0, 1)], sand, 1);
    f.craft(&[(0, sand, 1), (1, cobble, 1), (3, stick, 1), (4, stick, 1)], "base:grindstone", 1);
    f.walk_toward(Fresh::middle(table), 3.0);
    f.place_near("base:grindstone", f.rel(7, -1, -1));
    f.gather("base:flint_pickaxe", rel(f, (18..=20).flat_map(|x| (-10..=-8).map(move |z| v(x, 0, z))).collect()), granite, 7);
    f.at_table(table);
    f.craft(&[(0, granite, 7)], head, 7);
    f.craft(&[(0, head, 1), (1, head, 1), (2, head, 1), (4, stick, 1), (7, stick, 1)], "base:stone_pickaxe", 1);
    f.craft(&[(0, head, 1), (1, head, 1), (3, head, 1), (4, stick, 1), (7, stick, 1)], "base:stone_axe", 1);
    f.mark("stone_in_hand");

    // First Night: a campfire (two logs over two cobblestones), set down; three animals
    // hunted for meat, and a meal cooked on the fire. Cooking at Cooking 1 burns about half
    // of what it cooks, so more are hunted if the meat runs out first.
    let fire = f.rel(6, 0, 2);
    f.craft(&[(0, log, 1), (1, log, 1), (3, cobble, 1), (4, cobble, 1)], "base:campfire", 1);
    let fire = f.place_near("base:campfire", fire - up);
    let meats = [("base:raw_small_game", "base:cooked_small_game")];
    let raw = |c: &Client| meats.iter().map(|(raw, _)| holding(c, raw) + loose(c, raw)).sum::<u32>();
    f.hunt(|_| "", |c| raw(c) >= 3);
    for (raw, _) in meats {
        let seen = f.have(raw) + loose(&f.c, raw);
        f.collect(raw, seen);
    }
    f.walk_toward(Fresh::middle(fire), 2.5);
    while meats.iter().all(|(_, cooked)| f.have(cooked) == 0) {
        let Some(&(raw, cooked)) = meats.iter().find(|(raw, _)| f.have(raw) > 0) else {
            f.hunt(|_| "", |c| raw(c) >= 2);
            for (raw, _) in meats {
                let seen = f.have(raw) + loose(&f.c, raw);
                f.collect(raw, seen);
            }
            f.walk_toward(Fresh::middle(fire), 2.5);
            continue;
        };
        f.cook(fire, raw, cooked);
    }
    f.mark("first_night");

    // A Tempting Bite: the wild wheat by the lane broken by hand for its seeds, and the seeds
    // held in the lane until a junglefowl comes up to them (one is let in if the
    // night's hunt took them all).
    let wheat = f.rel(-7, 0, 2);
    f.walk_toward(Fresh::middle(wheat), 1.3);
    // `mine` waits for a solid block to go; a plant is picked at a touch.
    f.propose("core:mine", ActionTarget::Block { block: wheat, face: Face::PosY });
    f.wait(5.0, "picking the wild wheat", |c| c.world.block(wheat) == Some(BlockId::AIR));
    f.collect("base:wheat_seeds", 1);
    let fowl = f.c.content.creatures.find("junglefowl").expect("junglefowl in the content");
    // Only one the night's hunt never swung at comes to seeds: one struck holds its grudge
    // (`Tempted`) and runs, as a player sees it do, and so does one missed, whole as it is.
    // So one never swung at is looked for, and let in if there is none.
    let origin = f.origin;
    let swung = f.swung.clone();
    let unhurt = move |c: &Client| -> Vec<f32> {
        lane(c, origin).into_iter().filter(|&(id, _, species)| species == fowl && !swung.contains(&id)).map(|(_, p, _)| p.z).collect()
    };
    if unhurt(&f.c).is_empty() {
        f.stock("junglefowl", -4);
    }
    // The seeds in hand from the lane's mouth on, so a fowl that wanders while the player
    // walks in is tempted from wherever it ends up (seeds tempt from under ten blocks).
    f.hold("base:wheat_seeds");
    // In by the open end to five blocks short of the nearest junglefowl (they huddle at the
    // far end), where it can walk up the rest.
    f.walk_toward(Fresh::middle(f.rel(-9, 0, 2)), 0.3);
    // One may have stepped out by the open end while the player walked in: another is let in.
    let in_lane = |c: &Client| !unhurt(c).is_empty();
    if !in_lane(&f.c) {
        f.stock("junglefowl", -4);
        assert!(f.waited(2.0, in_lane), "the junglefowl let into the lane never showed");
    }
    // Where the nearest is now, once the player is in.
    let near = unhurt(&f.c).into_iter().fold(f32::MIN, f32::max);
    let lane_x = f.rel(-9, 0, 0).x as f32 + 0.5;
    f.walk_toward(Vec3::new(lane_x, f.at.y, (near + 5.0).min(f.rel(0, 0, 0).z as f32 + 0.5)), 0.3);
    // However crowded the lane's end, one walks out of the heap and up to the seeds.
    let drawn = f.waited(10.0, |c| quest_status(c, "a_tempting_bite").0);
    let fowls: Vec<_> = f.c.creatures.values().filter(|v| v.species == fowl && v.position.distance(f.at) < 12.0).map(|v| (v.id, v.position, v.pose, v.health)).collect();
    let cells: Vec<_> = (-11..=1).map(|z| (z, [-1, 0, 1].map(|y| f.c.world.block(f.rel(-9, y, z)).map(|b| f.c.content.blocks.def(b).name.clone())))).collect();
    assert!(drawn, "no junglefowl drawn up to the seeds: at {:?} holding {:?}, lane {:?}, fowl near (pose, health) {fowls:?}, cells {cells:?}, origin {:?}, quest {:?}", f.at, f.c.inventory.get(f.held as usize), lane(&f.c, f.origin), f.origin, f.c.quests.iter().find(|q| q.name == "base:a_tempting_bite").map(|q| (q.done, q.tasks.clone())));
    f.mark("a_tempting_bite");

    // A Bed of Your Own: two feathers (junglefowl) and two hides (rabbits) for a sleeping
    // bag, crafted in the two by two grid.
    // Rabbits give raw hide; the first bag takes it as it is.
    let (feather, hide) = ("base:feather", "base:raw_hide");
    let (feathers, hides) = (|c: &Client| holding(c, feather) + loose(c, feather), |c: &Client| holding(c, hide) + loose(c, hide));
    f.hunt(|c| if hides(c) < 2 { "rabbit" } else { "junglefowl" }, |c| feathers(c) >= 2 && hides(c) >= 2);
    f.collect(feather, 2);
    f.collect(hide, 2);
    f.craft(&[(0, feather, 1), (1, feather, 1), (3, hide, 1), (4, hide, 1)], "base:sleeping_bag", 1);
    f.mark("a_bed_of_your_own");

    // Coal and Cinders: eleven coal ores with the stone pickaxe, twelve with the sample. Torchlight: two of them, each with a stick making four torches.
    let coal = rel(f, [v(9, 0, 3), v(9, 0, 4)].into_iter().chain(FRESH_COAL.map(|(x, z)| v(x, 0, z))).collect());
    f.gather("base:stone_pickaxe", coal, "base:coal_ore", 12);
    f.mark("coal_and_cinders");
    f.at_table(table);
    f.craft(&[(0, "base:coal_ore", 2), (3, stick, 2)], "base:torch", 8);
    f.mark("torchlight");

    // The Homestead Hearth: a shovel for the clay (four, from the pond's bed), twenty-eight
    // cobblestones, a hearthstone (eight round an empty middle) and two chimneys (six and two
    // clay each) crafted at the table, the hearthstone set in a ring of eight stone with both
    // chimneys stacked on one corner stone, and lit.
    let bed = rel(f, (2..=11).flat_map(|z| [-2, -1].into_iter().flat_map(move |x| [-2, -3].into_iter().map(move |y| v(x, y, z)))).collect());
    let banks: Vec<Vec3> = (2..=11).step_by(2).map(|z| Fresh::middle(f.rel(0, 0, z))).collect();
    f.at_table(table);
    f.craft(&[(0, "base:stone_head", 1), (3, stick, 1), (6, stick, 1)], "base:stone_shovel", 1);
    f.gather_from("base:stone_shovel", &banks, bed.clone(), clay, 4);
    f.cobble("base:stone_pickaxe", stone.clone(), 28);
    f.mark("hearth_materials");
    f.at_table(table);
    f.craft(&[0, 1, 2, 3, 5, 6, 7, 8].map(|cell| (cell, cobble, 1)), "base:hearthstone", 1);
    f.craft(&[(0, cobble, 2), (1, clay, 2), (2, cobble, 2), (3, cobble, 2), (5, cobble, 2), (6, cobble, 2), (7, clay, 2), (8, cobble, 2)], "base:chimney", 2);
    // Set down from the north side of the site: the ring round (4, 7), the hearthstone in it.
    f.walk_toward(Fresh::middle(f.rel(4, 0, 4)), 0.05);
    for (dx, dz) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
        f.place(cobble, f.rel(4 + dx, -1, 7 + dz));
    }
    let core = f.place("base:hearthstone", f.rel(4, -1, 7));
    let first = f.place("base:chimney", f.rel(3, 0, 6));
    f.place("base:chimney", first);
    f.c.chat.clear();
    f.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
    f.wait(5.0, "lighting the hearth", |c| c.chat.iter().any(|(_, t)| t.contains("your home")));
    f.mark("hearth_lit");
    // The Farm age's rite (twenty Nomad quests, three of them path capstones) opens its
    // stations; this line walks only the main quests, so it is marked done for it.
    f.c.ok("/quest the_farm_age");
    // Its own line and the command's both in before any later command reads the chat.
    f.wait(5.0, "the Farm age's rite marked done", |c| quest_status(c, "the_farm_age").0);
    f.c.chat.clear();

    // The Farm age's first fire: a campfire and five cobblestones make a pit kiln, set down;
    // clay from the pond loaded into it twelve at a time and left to fire until twelve fired
    // clay are in hand.
    f.cobble("base:stone_pickaxe", stone, 7);
    f.craft(&[(0, log, 1), (1, log, 1), (3, cobble, 1), (4, cobble, 1)], "base:campfire", 1);
    f.at_table(table);
    f.craft(&[(0, cobble, 1), (2, cobble, 1), (3, cobble, 1), (4, "base:campfire", 1), (5, cobble, 1)], "base:pit_kiln", 1);
    let kiln = f.rel(2, 0, 1);
    f.walk_toward(Fresh::middle(kiln), 3.0);
    let kiln = f.place_near("base:pit_kiln", kiln - up);
    f.mark("pit_kiln");
    let site = Site { table, kiln, bed, banks };
    f.fire_clay(&site, 12);
    f.mark("fire_the_clay");
    site
}

/// The coal ores standing apart on the fresh plot, beyond the stone patch (x, z).
const FRESH_COAL: [(i32, i32); 9] = [(9, 6), (11, 6), (13, 6), (15, 6), (17, 6), (10, 10), (12, 10), (14, 10), (16, 10)];

/// The quests a new player follows from nothing to the lit Crucible Kiln, in the order the
/// book opens them: the Nomad main line to the lit Homestead Hearth, then the Farm main line.
const QUEST_BOOK_ORDER: &[&str] = &[
    "look_it_up",
    "timber",
    "crafting_table",
    "stone_in_hand",
    "first_night",
    "a_bed_of_your_own",
    "black_flecks",
    "coal_and_cinders",
    "torchlight",
    "homestead_hearth",
    "a_pit_kiln",
    "fire_the_clay",
    "straw_for_the_kiln",
    "the_potter",
    "fuel_to_hand",
    "charcoal",
    "kiln_plans",
    "a_clay_store",
    "a_kiln_load",
    "first_bricks",
    "brick_by_brick",
    "the_crucible",
    "the_kiln_ring",
    "the_kiln_charge",
    "crucible_kiln",
];

impl Fresh {
    /// More of the plot for the Farm age, east and north of the first days' ground: two more
    /// ponds like the first (two blocks wide, clay two deep under a block of water, a bank
    /// each side), fourteen more oak trunks, a bank of dirt and a bed of gravel for the
    /// charcoal clamp, a copper vein's ten ores, and wild wheat for straw.
    fn lay_out_the_farm(&mut self) {
        let (o, v) = (self.origin, IVec3::new);
        self.c.fill(o, v(21, 0, -11), v(30, 10, 13), "air");
        self.c.fill(o, v(21, -4, -11), v(30, -1, 13), "dirt");
        for x in [22, 26] {
            self.c.fill(o, v(x, -3, 2), v(x + 1, -2, 11), "clay");
            self.c.fill(o, v(x, -1, 2), v(x + 1, -1, 11), "water");
        }
        for z in [-7, -9] {
            for x in [1, 3, 5, 7, 11, 13, 15] {
                self.c.fill(o, v(x, 0, z), v(x, 3, z), "oak_log");
            }
        }
        self.c.fill(o, v(18, 0, 7), v(19, 0, 11), "copper_ore");
        // Six tufts of wild wheat beyond the east pond, for straw.
        self.c.fill(o, v(29, 0, 8), v(30, 0, 10), "wild_wheat");
        // A low bank of dirt, thirty-two blocks, for the charcoal clamp's walls and roof (24,
        // with some to spare for a drop out of reach): the only dirt above the ground, so the
        // bot finds it by looking (`Fresh::raised_dirt`).
        self.c.fill(o, v(21, 0, -11), v(28, 0, -8), "dirt");
        let probes = [(v(27, -1, 11), "base:water"), (v(15, 3, -9), "base:oak_log"), (v(19, 0, 11), "base:copper_ore"), (v(30, 0, 10), "base:wild_wheat"), (v(28, 0, -9), "base:dirt")];
        self.c.pump_until(|c| probes.iter().all(|&(at, name)| c.world.block(o + at) == c.content.blocks.id(name)));
    }

    /// Waits for the server to count `quest` done (it looks once a second), and claims its
    /// rewards in the quest screen.
    fn claim(&mut self, quest: &str) {
        // Each task's (have, need), as the server counted them, for a failure to say which fell short.
        let tasks = self.c.quests.iter().find(|q| q.name == format!("base:{quest}")).map(|q| q.tasks.clone());
        self.wait(5.0, &format!("the quest book showing {quest} done (done, a task holding: {:?}; tasks {tasks:?})", quest_status(&self.c, quest)), |c| quest_status(c, quest).0);
        let name = format!("base:{quest}");
        self.c.connection.send_at(ClientMessage::ClaimQuest { name: name.clone() }, self.now());
        self.wait(5.0, &format!("claiming {quest}"), |c| c.quests.iter().any(|q| q.name == name && q.claimed));
    }

    /// Fires clay in the pit kiln until `need` fired clay are in hand: loads it (up to twelve)
    /// when empty, takes what it fired when it shows fired, and while it fires digs the clay
    /// for the next load from the ponds, or waits by it when no more is wanted.
    fn fire_clay(&mut self, site: &Site, need: u32) {
        let (clay, fired) = ("base:clay", "base:fired_clay");
        while self.have(fired) < need {
            let kiln = |c: &Client| c.world.block(site.kiln).map(|b| c.content.blocks.def(b).name.clone()).unwrap_or_default();
            let here = kiln(&self.c);
            // What the kiln holds now is fired clay on the way, so only the rest is dug for, a
            // full load at a time.
            let coming = if here == "base:pit_kiln" { 0 } else { 12 };
            let wanted = need.saturating_sub(self.have(fired) + coming).min(12);
            if self.have(clay) < wanted {
                self.gather_from("base:stone_shovel", &site.banks, site.bed.clone(), clay, wanted);
            }
            if across(self.at, Self::middle(site.kiln)) > 3.5 {
                self.walk_toward(Self::middle(site.kiln), 3.0);
            }
            let use_kiln = ActionTarget::Block { block: site.kiln, face: Face::PosY };
            match kiln(&self.c).as_str() {
                "base:pit_kiln_firing" => self.wait(20.0, "the pit kiln firing its load", |c| kiln(c) == "base:pit_kiln_fired"),
                "base:pit_kiln_fired" => {
                    let before = self.have(fired);
                    self.propose("base:use_station", use_kiln);
                    self.wait(5.0, "taking the pit kiln's fired clay", |c| kiln(c) == "base:pit_kiln" && holding(c, fired) > before);
                }
                _ => {
                    self.hold(clay);
                    self.propose("base:use_station", use_kiln);
                    self.wait(5.0, "loading the pit kiln with clay", |c| kiln(c) == "base:pit_kiln_firing");
                }
            }
        }
    }

    /// Throws away what the rest of the way has no use for, until `free` slots are empty: at
    /// the far corner of the farm plot, out of the way of every site, then back. A full pack
    /// takes nothing more in (a quest's reward, a craft, dug clay), and cooked food takes a
    /// slot each, so by the Farm age a player clears theirs out as this does.
    fn make_room(&mut self, free: usize) {
        self.make_room_keeping(free, &[]);
    }

    /// Makes room for `need` of `item` in all, before gathering it with `tool` (both kept): the
    /// pack fills with the way's loot as it goes, the hunt's and the gravel's more on one run
    /// than another, and a drop with no room for it lies under the player for good.
    fn make_room_for(&mut self, item: &str, need: u32, tool: &str) {
        let id = self.c.content.blocks.expect(item);
        let most = voxmar_engine::inventory::max_stack(id) as u32;
        let topped: u32 = self.c.inventory.slots.iter().flatten().filter(|s| s.item == id).map(|s| most.saturating_sub(s.count as u32)).sum();
        let more = need.saturating_sub(self.have(item)).saturating_sub(topped);
        if more > 0 {
            self.make_room_keeping(more.div_ceil(most) as usize, &[item, tool]);
        }
    }

    fn make_room_keeping(&mut self, free: usize, keep: &[&str]) {
        const SPARE: [&str; 7] = ["base:burnt_food", "base:feather", "base:flint_pickaxe", "base:raw_small_game", "base:cooked_small_game", "base:oak_planks", "base:bread"];
        let slots = voxmar_engine::inventory::SLOTS;
        let empty = |c: &Client| (0..slots).filter(|&s| c.inventory.get(s).is_none()).count();
        if empty(&self.c) >= free {
            return;
        }
        let back = self.at;
        assert!(self.hop_near(Self::middle(self.rel(29, 0, -10)), 0.5), "no room at the plot's far corner");
        for item in SPARE.into_iter().filter(|i| !keep.contains(i)) {
            let id = self.c.content.blocks.expect(item);
            while empty(&self.c) < free {
                let Some(slot) = (0..slots).find(|&s| self.c.inventory.get(s).is_some_and(|x| x.item == id)) else { break };
                let count = self.c.inventory.get(slot).map_or(0, |x| x.count);
                self.propose("core:drop", ActionTarget::Stack { slot: slot as u8, count });
                self.wait(5.0, &format!("throwing away {item}"), |c| c.inventory.get(slot).is_none_or(|x| x.item != id));
            }
        }
        assert!(
            empty(&self.c) >= free,
            "only {} slots empty after throwing away the spare, the pack holding {:?}",
            empty(&self.c),
            (0..slots).filter_map(|s| self.c.inventory.get(s)).map(|x| format!("{} {}", x.count, self.c.content.blocks.def(x.item).name)).collect::<Vec<_>>()
        );
        self.c.tp(back);
        self.at = back;
    }
}

/// A new player's first two ages, played from nothing by what the quest book says: each
/// quest's text is the script, every item is gathered, crafted with a recipe the player has
/// unlocked, cooked or fired, and nothing is given (no /give; the plot built by /fill stands
/// in for terrain, as for the pacing test above). Each main quest must be done in the
/// book's order, so a step that cannot be finished from its text and recipes fails here.
///
/// The player's messages carry the game's clock (`Fresh::now`), so the run goes at the
/// test server's quick ticks; only the hunt's blows keep the clock's pace. It prints the
/// server seconds of each quest and the run's wall time, for the test-time budgets.
#[test]
fn a_new_player_follows_the_quest_book_to_the_lit_crucible_kiln() {
    lockstep("some eleven minutes of game time, from bare hands to the lit Crucible Kiln");
    let wall = Instant::now();
    let c = Client::join_with(ServerConfig { view_distance: 2, draw_key: Some(FRESH_DRAWS), ..Default::default() });
    let mut f = Fresh::lay_out(c);
    f.lay_out_the_farm();
    let o = f.origin;
    let (log, charcoal, stick) = ("base:oak_log", "base:charcoal", "base:stick");
    let (fired, bricks) = ("base:fired_clay", "base:kiln_bricks");
    let (straw, tempered) = ("base:straw", "base:tempered_kiln_bricks");

    // Look It Up, pinned in a new book: the stone pickaxe looked up in the recipe viewer.
    let pickaxe = f.c.content.blocks.expect("base:stone_pickaxe");
    f.c.connection.send_at(ClientMessage::LookedUp { item: pickaxe }, f.now());
    f.mark("look_it_up");

    // The Nomad main line to the lit Homestead Hearth, and the Farm age's pit kiln and first
    // twelve fired clay (the pacing test's script).
    let mut site = the_first_days(&mut f);
    // The Farm's ponds, dug from the bank on each side.
    for x in [22, 26] {
        site.bed.extend((2..=11).flat_map(|z| [x, x + 1].into_iter().flat_map(move |x| [-2, -3].into_iter().map(move |y| o + IVec3::new(x, y, z)))));
        for bank in [x - 1, x + 2] {
            site.banks.extend((2..=11).step_by(2).map(|z| Fresh::middle(o + IVec3::new(bank, 0, z))));
        }
    }

    // The Potter: six clay pots at the table, five fired clay round an empty top middle
    // making two (Crafting 2).
    f.fire_clay(&site, 15);
    f.at_table(site.table);
    f.craft(&[(0, fired, 3), (2, fired, 3), (3, fired, 3), (4, fired, 3), (5, fired, 3)], "base:clay_pot", 6);
    f.mark("the_potter");

    // Fuel to Hand: forty logs carried at once, past its thirty-two, for the clamp's charcoal
    // and the planks of a chest, and what the clamp is built of.
    let trunks: Vec<IVec3> = [-7, -9].into_iter().flat_map(|z| [1, 3, 5, 7, 11, 13, 15].into_iter().flat_map(move |x| (0..4).map(move |y| o + IVec3::new(x, y, z)))).collect();
    f.gather("base:stone_axe", trunks, log, 40);
    f.wait(5.0, "the server counting the logs carried", |c| quest_status(c, "fuel_to_hand").0);
    f.mark("fuel_to_hand");
    // Charcoal, from a clamp as the quest says, of the logs carried: one for the chopping
    // block, three split on it into six firewood to join The Potter's four (nine laid), one
    // for sticks. The chopping
    // block is a flint over a log (Crafting 2), the flint dug from what the first days left
    // of the gravel bank.
    // The pack is full after the forty logs: the spare goes, for the flint and the dirt.
    f.make_room(4);
    let gravel = (10..=24).flat_map(|x| (0..=2).flat_map(move |y| [14, 17].map(move |z| o + IVec3::new(x, y, z)))).collect();
    f.gather_chance("base:stone_shovel", gravel, "base:flint", f.have("base:flint") + 1);
    f.craft(&[(0, "base:flint", 1), (3, log, 1)], "base:chopping_block", 1);
    let block = f.rel(30, 0, -4);
    f.walk_toward(Fresh::middle(block - IVec3::X), 0.3);
    let block = f.place_near("base:chopping_block", block - IVec3::Y);
    // A log held to it, the stone axe carried: two firewood a log.
    while f.have("base:firewood") < 9 {
        f.cook(block, log, "base:firewood");
    }
    // Kindling of three sticks for the flue.
    if f.have(stick) < 3 {
        f.craft(&[(0, log, 1)], "base:oak_planks", 4);
        f.craft(&[(0, "base:oak_planks", 1), (3, "base:oak_planks", 1)], stick, 4);
    }
    f.craft(&[(0, stick, 1), (1, stick, 1), (3, stick, 1)], "base:kindling", 1);
    // Twenty-four dirt for the walls and roof, from the bank.
    let bank = f.raised_dirt();
    f.gather("base:stone_shovel", bank, "base:dirt", 24);
    // The clamp as multiblocks.ron draws it, on the farm plot's ground: the firewood three
    // by three, walled round with dirt and roofed with it, the kindling on the roof's middle.
    // Laid from its north side, then from its south (round its east end).
    let corner = f.rel(22, 0, -6);
    let cell = move |x: i32, y: i32, z: i32| corner + IVec3::new(x, y, z);
    let inner = |x: i32, z: i32| (1..4).contains(&x) && (1..4).contains(&z);
    let piece = |x: i32, y: i32, z: i32| match (y, inner(x, z)) {
        (0, true) => "base:firewood",
        (1, true) if (x, z) == (2, 2) => "base:kindling",
        _ => "base:dirt",
    };
    for (stand, rows) in [(cell(2, 0, 5), 2..5), (cell(2, 0, -2), 0..2)] {
        if stand.z < corner.z {
            f.walk_toward(Fresh::middle(cell(6, 0, 5)), 0.3);
            f.walk_toward(Fresh::middle(cell(6, 0, -2)), 0.3);
        }
        f.walk_toward(Fresh::middle(stand), 0.05);
        for y in 0..2 {
            for z in rows.clone() {
                for x in (0..5).filter(|&x| y == 0 || inner(x, z)) {
                    if f.c.world.block(cell(x, y, z)) == Some(BlockId::AIR) {
                        f.place(piece(x, y, z), cell(x, y - 1, z));
                    }
                }
            }
        }
    }
    // Lit from a torch in hand at the kindling: it smoulders two game hours, nobody by it,
    // while clay is put by in store, then fired by hand toward A Kiln Load's forty, open
    // since The Potter.
    let flue = cell(2, 1, 2);
    f.hold("base:torch");
    f.propose("base:light", ActionTarget::Block { block: flue, face: Face::PosY });
    let flue_is = |c: &Client, name: &str| c.world.block(flue) == c.content.blocks.id(name);
    f.wait(5.0, "lighting the clamp", |c| flue_is(c, "base:clamp_flue_catching"));
    let spent = move |c: &Client| c.world.block(flue) == Some(BlockId::AIR);
    // Clay in Store, while it smoulders: thirty-two clay dug (Clay Banks on the way), and a
    // chest by the pit kiln, eight planks round an empty middle at the table, with them put
    // by in it.
    f.make_room(8);
    let clay = "base:clay";
    f.gather_from("base:stone_shovel", &site.banks, site.bed.clone(), clay, 32);
    f.wait(5.0, "the server counting the clay carried", |c| quest_status(c, "clay_banks").0);
    let planks = "base:oak_planks";
    if f.have(planks) < 8 {
        let logs = (8 - f.have(planks)).div_ceil(4);
        f.craft(&[(0, log, logs as u8)], planks, logs * 4);
    }
    f.at_table(site.table);
    f.craft(&[(0, planks, 1), (1, planks, 1), (2, planks, 1), (3, planks, 1), (5, planks, 1), (6, planks, 1), (7, planks, 1), (8, planks, 1)], "base:chest", 1);
    f.walk_toward(Fresh::middle(f.rel(4, 0, 3)), 0.3);
    let chest = f.place_near("base:chest", f.rel(2, -1, 3));
    f.propose("base:open_store", ActionTarget::Block { block: chest, face: Face::PosY });
    f.wait(5.0, "the chest opening", |c| c.inventory.store.is_some());
    let clay_id = f.c.content.blocks.expect(clay);
    let (mut put, mut to) = (0u32, voxmar_engine::inventory::STORE);
    while put < 32 {
        let (from, count) = (0..voxmar_engine::inventory::SLOTS)
            .find_map(|s| f.c.inventory.get(s).filter(|x| x.item == clay_id).map(|x| (s, x.count as u32)))
            .expect("clay to put by");
        let n = count.min(32 - put);
        f.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: to as u8, count: n as u8 });
        f.wait(5.0, "the clay going into the chest", |c| c.inventory.get(to).is_some_and(|x| x.item == clay_id && x.count as u32 == n));
        put += n;
        to += 1;
    }
    f.propose("core:stow", ActionTarget::None);
    f.wait(5.0, "the server counting the clay put by", |c| quest_status(c, "a_clay_store").0);
    f.mark("a_clay_store");
    // While it smoulders, as Brick by Brick says, the pit kiln kept going: clay loaded a
    // dozen at a time to A Kiln Load's forty, then First Bricks, the straw picked, and the
    // rest shaped between firings (Crafting 5): seven plain, as the book counts them (A Kiln
    // Load's four were given, not shaped), then twelve tempered, two from four fired clay
    // and a straw at the table. What is left when it is spent waits for the charcoal.
    // Earthenware, open since The Potter: two bowls and a jar of fired clay in the grid
    // (Crafting 2 and 3), at the table: between loads the player may be off digging clay.
    let table = site.table;
    let bowls = |f: &mut Fresh| {
        f.at_table(table);
        f.craft(&[(0, fired, 1), (2, fired, 1), (4, fired, 1)], "base:earthen_bowl", 2);
    };
    let jar = |f: &mut Fresh| {
        f.at_table(table);
        f.craft(&[(1, fired, 1), (3, fired, 1), (4, fired, 1)], "base:earthen_jar", 1);
        f.wait(5.0, "the server counting the crockery", |c| quest_status(c, "earthenware").0);
    };
    let brick_work = |f: &mut Fresh| {
        // Counted in the pack, where the bricks wait for the ring and the crucible, less A
        // Kiln Load's four given: the server counts the crafts toward Brick by Brick once a
        // second, slower than the bot.
        let (plain, done_tempered) = (f.have(bricks).saturating_sub(4), f.have(tempered));
        if !quest_status(&f.c, "a_kiln_load").0 {
            let more = f.have(fired) + 12;
            f.fire_clay(&site, more);
            // Between loads, as A Kiln Load says: Earthenware's crockery from the fired clay,
            // the bowls after one load and the jar after the next.
            if !quest_status(&f.c, "earthenware").0 && f.have(fired) >= 3 {
                if f.have("base:earthen_bowl") == 0 {
                    bowls(f);
                } else if f.have("base:earthen_jar") == 0 {
                    jar(f);
                }
            }
            if quest_status(&f.c, "a_kiln_load").0 {
                f.mark("a_kiln_load");
            }
        } else if !quest_status(&f.c, "first_bricks").0 {
            // First Bricks: four kiln bricks, two from four fired clay in the grid, with
            // sixteen more fired clay kept for the next.
            f.fire_clay(&site, 24);
            f.craft(&[(0, fired, 2), (1, fired, 2), (3, fired, 2), (4, fired, 2)], bricks, 4);
            f.wait(5.0, "the server counting the bricks and the fired clay kept", |c| quest_status(c, "first_bricks").0);
            f.mark("first_bricks");
        } else if !quest_status(&f.c, "earthenware").0 {
            // Earthenware, if no load left fired clay to spare for it: fired for as the
            // bricks are.
            if f.have(fired) < 6 {
                let more = f.have(fired) + 12;
                f.fire_clay(&site, more);
            }
            if f.have("base:earthen_bowl") == 0 {
                bowls(f);
            }
            jar(f);
        } else if !quest_status(&f.c, "a_table_for_two").0 {
            // A Table for Two, open since the Farm Age, made between firings: two oak stools
            // (Construction 1) and a table (Construction 3) at the crafting table, set by the
            // hearth.
            f.make_room(4);
            f.at_table(site.table);
            while f.have(stick) < 8 {
                if f.have(planks) < 2 {
                    f.craft(&[(0, log, 1)], planks, 4);
                }
                f.craft(&[(0, planks, 1), (3, planks, 1)], stick, 4);
            }
            while f.have(planks) < 7 {
                f.craft(&[(0, log, 1)], planks, 4);
            }
            f.craft(&[(0, planks, 2), (1, planks, 2), (3, stick, 2), (4, stick, 2)], "base:oak_stool", 2);
            f.craft(&[(0, planks, 1), (1, planks, 1), (2, planks, 1), (3, stick, 1), (5, stick, 1), (6, stick, 1), (8, stick, 1)], "base:oak_table", 1);
            f.wait(5.0, "the server counting the seats and the table", |c| quest_status(c, "a_table_for_two").0);
            // Set by the hearth, from its north side, a stool each side of the table.
            f.walk_toward(Fresh::middle(f.rel(4, 0, 4)), 0.05);
            for (x, item) in [(3, "base:oak_stool"), (4, "base:oak_table"), (5, "base:oak_stool")] {
                f.place(item, f.rel(x, -1, 5));
            }
            // The crockery put by in the chest beside them, for room in the pack.
            f.propose("base:open_store", ActionTarget::Block { block: chest, face: Face::PosY });
            f.wait(5.0, "the chest opening", |c| c.inventory.store.is_some());
            for item in ["base:earthen_bowl", "base:earthen_jar", "base:earthen_plate", "base:earthen_jug"] {
                let id = f.c.content.blocks.expect(item);
                while let Some(from) = (0..voxmar_engine::inventory::SLOTS).find(|&s| f.c.inventory.get(s).is_some_and(|x| x.item == id)) {
                    let count = f.c.inventory.get(from).map_or(0, |x| x.count);
                    let to = voxmar_engine::inventory::STORE + f.c.inventory.store.as_ref().and_then(|store| store.slots.iter().position(Option::is_none)).expect("room in the chest");
                    f.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: to as u8, count });
                    f.wait(5.0, &format!("{item} going into the chest"), |c| c.inventory.get(from).is_none_or(|x| x.item != id));
                }
            }
            f.propose("core:stow", ActionTarget::None);
        } else if f.have(straw) < 12u32.saturating_sub(done_tempered).div_ceil(2) {
            // Straw for the Kiln: the tufts of wild wheat beyond the east pond, picked for the
            // straw that tempers the ring's bricks (one a tuft) and the seeds a field would take,
            // with room in the pack for them.
            f.make_room(4);
            for tuft in (8..=10).flat_map(|z| [29, 30].map(|x| o + IVec3::new(x, 0, z))) {
                if f.c.world.block(tuft) != Some(BlockId::AIR) {
                    f.walk_toward(Fresh::middle(tuft), 1.3);
                    f.propose("core:mine", ActionTarget::Block { block: tuft, face: Face::PosY });
                    f.wait(5.0, "picking the wild wheat", |c| c.world.block(tuft) == Some(BlockId::AIR));
                }
            }
            f.collect(straw, 12u32.saturating_sub(done_tempered).div_ceil(2));
            // Six kept at once, as the quest asks, before any is spent.
            f.wait(5.0, "the server counting the straw kept", |c| quest_status(c, "straw_for_the_kiln").0);
        } else if done_tempered >= 12 {
            f.wait(5.0, "the server counting the bricks shaped", |c| quest_status(c, "brick_by_brick").0);
            f.mark("brick_by_brick");
        } else if f.have(fired) < 4 {
            // A full load, the pit kiln's twelve, not just the four a craft takes: the kiln
            // fires a load in the same twelve seconds however many it holds.
            let more = f.have(fired) + 12;
            f.fire_clay(&site, more);
        } else if plain < 7 {
            f.craft(&[(0, fired, 1), (1, fired, 1), (3, fired, 1), (4, fired, 1)], bricks, 2);
        } else {
            f.at_table(site.table);
            f.craft(&[(0, fired, 1), (1, fired, 1), (3, fired, 1), (4, fired, 1), (2, straw, 1)], tempered, 2);
        }
    };
    let bricks_done = |f: &Fresh| quest_status(&f.c, "brick_by_brick").0;
    // The Kiln Ring's ground, chosen flat and clear, whatever the earlier steps left on it.
    let centre = f.rel(13, 0, 9);
    let ring_stand = Fresh::middle(f.rel(13, 0, 6));
    // Its far side and pillars laid while the clamp chars, once the bricks are shaped; the
    // near side waits for the crucible, set down over it.
    let far = [(-1, 0, 0), (1, 0, 0), (-1, 0, 1), (0, 0, 1), (1, 0, 1), (-1, 1, 1), (1, 1, 1)];
    let near = [(-1, 0, -1), (0, 0, -1), (1, 0, -1), (-1, 1, -1), (1, 1, -1)];
    let lay = |f: &mut Fresh, bricks: &[(i32, i32, i32)]| {
        f.walk_toward(ring_stand, 0.05);
        for &(dx, dy, dz) in bricks {
            f.place(tempered, centre + IVec3::new(dx, dy - 1, dz));
        }
    };
    let mut ring_begun = false;
    let clear_ring = |f: &mut Fresh| {
        f.c.fill(f.origin, IVec3::new(12, 0, 8), IVec3::new(14, 2, 10), "air");
        let site: Vec<IVec3> = (12..=14).flat_map(|x| (0..=2).flat_map(move |y| (8..=10).map(move |z| IVec3::new(x, y, z)))).map(|p| f.origin + p).collect();
        f.c.pump_until(|c| site.iter().all(|&p| c.world.block(p) == Some(BlockId::AIR)));
    };
    while !spent(&f.c) {
        if !bricks_done(&f) {
            brick_work(&mut f);
        } else if !ring_begun {
            clear_ring(&mut f);
            lay(&mut f, &far);
            ring_begun = true;
        } else if !quest_status(&f.c, "shelves_and_cupboards").0 {
            // Shelves and Cupboards, after A Table for Two: an oak shelf (Construction 4) and
            // a cupboard (Construction 6) at the crafting table, set by the hearth beside it.
            f.at_table(site.table);
            while f.have(planks) < 14 {
                f.craft(&[(0, log, 1)], planks, 4);
            }
            if f.have(stick) < 1 {
                f.craft(&[(0, planks, 1), (3, planks, 1)], stick, 4);
            }
            f.craft(&[0, 1, 2, 6, 7, 8].map(|cell| (cell, planks, 1)), "base:oak_shelf", 1);
            f.craft(&[(0, planks, 1), (1, planks, 1), (2, planks, 1), (3, planks, 1), (4, stick, 1), (5, planks, 1), (6, planks, 1), (7, planks, 1), (8, planks, 1)], "base:oak_cupboard", 1);
            f.wait(5.0, "the server counting the shelf and the cupboard", |c| quest_status(c, "shelves_and_cupboards").0);
            f.walk_toward(Fresh::middle(f.rel(4, 0, 4)), 0.05);
            for (x, item) in [(2, "base:oak_shelf"), (6, "base:oak_cupboard")] {
                f.place(item, f.rel(x, -1, 5));
            }
        } else if !quest_status(&f.c, "a_place_to_pray").0 {
            // A Place to Pray, open since A Pit Kiln: eight cobblestone (from the stone the
            // first days dug) fired in the pit kiln, now free, into stone; four stone in a
            // square make four stone bricks, and seven bricks an altar at the table.
            let (cobble, stone) = ("base:cobblestone", "base:stone");
            f.make_room(4);
            let quarry = (9..=17).flat_map(|x| (-3..=4).map(move |z| IVec3::new(x, 0, z))).filter(|p| !(p.x == 9 && p.z >= 3)).map(|p| o + p).collect();
            f.cobble("base:stone_pickaxe", quarry, 8);
            let kiln = |c: &Client| c.world.block(site.kiln).map(|b| c.content.blocks.def(b).name.clone()).unwrap_or_default();
            let use_kiln = ActionTarget::Block { block: site.kiln, face: Face::PosY };
            f.walk_toward(Fresh::middle(site.kiln), 3.0);
            while f.have(stone) < 8 {
                match kiln(&f.c).as_str() {
                    "base:pit_kiln_firing" => f.wait(20.0, "the pit kiln firing its load", |c| kiln(c) == "base:pit_kiln_fired"),
                    "base:pit_kiln_fired" => {
                        let before = f.have(stone) + f.have("base:fired_clay");
                        f.propose("base:use_station", use_kiln);
                        f.wait(5.0, "taking what the pit kiln fired", |c| kiln(c) == "base:pit_kiln" && holding(c, stone) + holding(c, "base:fired_clay") > before);
                    }
                    _ => {
                        f.hold(cobble);
                        f.propose("base:use_station", use_kiln);
                        f.wait(5.0, "loading the pit kiln with cobblestone", |c| kiln(c) == "base:pit_kiln_firing");
                    }
                }
            }
            f.craft(&[0, 1, 3, 4].map(|cell| (cell, stone, 2)), "base:stone_bricks", 8);
            f.at_table(site.table);
            f.craft(&[0, 1, 2, 4, 6, 7, 8].map(|cell| (cell, "base:stone_bricks", 1)), "base:stone_altar", 1);
            f.wait(5.0, "the server counting the altar", |c| quest_status(c, "a_place_to_pray").0);
            // Set down by the hearth, and the brick left over put by in the chest.
            f.walk_toward(Fresh::middle(f.rel(4, 0, 4)), 0.05);
            f.place("base:stone_altar", f.rel(6, -1, 3));
            let brick = f.c.content.blocks.expect("base:stone_bricks");
            if let Some(from) = (0..voxmar_engine::inventory::SLOTS).find(|&s| f.c.inventory.get(s).is_some_and(|x| x.item == brick)) {
                f.propose("base:open_store", ActionTarget::Block { block: chest, face: Face::PosY });
                f.wait(5.0, "the chest opening", |c| c.inventory.store.is_some());
                let count = f.c.inventory.get(from).map_or(0, |x| x.count);
                let to = voxmar_engine::inventory::STORE + f.c.inventory.store.as_ref().and_then(|store| store.slots.iter().position(Option::is_none)).expect("room in the chest");
                f.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: to as u8, count });
                f.wait(5.0, "the stone brick going into the chest", |c| c.inventory.get(from).is_none_or(|x| x.item != brick));
                f.propose("core:stow", ActionTarget::None);
            }
        } else {
            f.wait(500.0, "the clamp charring", spent);
        }
    }
    // Opened (roof and walls dug away), its charcoal heaps broken: four charcoal a heap.
    let earth: Vec<IVec3> = (0..5).flat_map(|x| (0..5).map(move |z| (x, z))).flat_map(|(x, z)| [(x, 0, z), (x, 1, z)]).filter(|&(x, y, z)| piece(x, y, z) == "base:dirt" && (y == 0 || inner(x, z))).map(|(x, y, z)| cell(x, y, z)).collect();
    let dirt = f.have("base:dirt") + earth.len() as u32;
    f.gather("base:stone_shovel", earth, "base:dirt", dirt);
    let heaps: Vec<IVec3> = (1..4).flat_map(|x| (1..4).map(move |z| cell(x, 0, z))).filter(|&p| f.c.world.block(p) == f.c.content.blocks.id("base:charcoal_heap")).collect();
    // Enough for Charcoal's sixteen, and for the line after it: three copper ingots and the
    // kiln's thirty-two, less The Crucible's sixteen, is nineteen.
    assert!(heaps.len() * 4 >= 19, "rain spoiled the clamp: {} heaps of charcoal of nine", heaps.len());
    let more = f.have(charcoal) + heaps.len() as u32 * 4;
    f.gather("base:stone_shovel", heaps, charcoal, more);
    f.mark("charcoal");
    f.make_room(8);
    // Plans for the Kiln, with fuel coming: the crucible and tempered kiln bricks looked up.
    for item in ["base:crucible", "base:tempered_kiln_bricks"] {
        let item = f.c.content.blocks.expect(item);
        f.c.connection.send_at(ClientMessage::LookedUp { item }, f.now());
    }
    f.wait(5.0, "the server counting the plans looked up", |c| quest_status(c, "kiln_plans").0);
    f.mark("kiln_plans");

    // A Kiln Load, First Bricks and Brick by Brick finished, if the clamp was spent first.
    while !bricks_done(&f) {
        brick_work(&mut f);
    }
    // A quest the server counted only after the work's own look, and has not yet claimed
    // (a claim on the way can be refused, a full pack turning A Kiln Load's bricks away),
    // is marked and claimed now, before the crucible needs its Engineering.
    for quest in ["a_kiln_load", "brick_by_brick"] {
        let name = format!("base:{quest}");
        if !f.waited(2.0, |c| c.claimed(&name)) {
            f.mark(quest);
        }
    }

    // The Crucible: seven kiln bricks round a clay pot at the table (Engineering 10), done
    // once it is set down in the kiln's middle.
    f.at_table(site.table);
    f.craft(&[(0, bricks, 1), (2, bricks, 1), (3, bricks, 1), (4, "base:clay_pot", 1), (5, bricks, 1), (6, bricks, 1), (7, bricks, 1), (8, bricks, 1)], "base:crucible", 1);

    // A Full Charge: thirty-two charcoal held at once, from the clamp.
    assert!(f.have(charcoal) >= 32, "the clamp's charcoal fell short of the kiln's charge: {}", f.have(charcoal));
    f.wait(5.0, "the server counting the charge carried", |c| quest_status(c, "the_kiln_charge").0);
    f.mark("the_kiln_charge");
    // The crucible set down in the middle of The Kiln Ring, the ring finished, and the kiln
    // lit: eight tempered kiln bricks round the crucible's cell, one more on each corner of
    // the ring; the crucible right-clicked with the charge in hand.
    // The far side, if the clamp was spent before it was laid.
    if !ring_begun {
        clear_ring(&mut f);
        lay(&mut f, &far);
    }
    f.walk_toward(ring_stand, 0.05);
    let core = f.place("base:crucible", centre - IVec3::Y);
    f.mark("the_crucible");
    lay(&mut f, &near);
    f.mark("the_kiln_ring");
    f.hold(charcoal);
    f.propose("base:light", ActionTarget::Block { block: core, face: Face::PosY });
    f.mark("crucible_kiln");

    // Every quest of the line done, none skipped, and the report for the budgets.
    assert_eq!(f.c.died, 0, "the player died on the way");
    let missed: Vec<&str> = QUEST_BOOK_ORDER.iter().copied().filter(|q| !quest_status(&f.c, q).0).collect();
    assert!(missed.is_empty(), "quests of the line not done: {missed:?}");
    eprintln!("{}", timeline(&f));
    eprintln!("a new player to the lit Crucible Kiln in {:.1} s of wall time", wall.elapsed().as_secs_f32());
    check_pacing(&mut f);
    // From the lit hearth on, the Farm line never goes longer with nothing new than one
    // action may go on: the kiln work between loads is work a player has not done before.
    let (hearth, end) = (f.marks.iter().find(|m| m.name == "hearth_lit").expect("the hearth lit").at, f.secs());
    let pacing = f.pacing.borrow();
    let (gap, from, ended) = pacing.longest_gap(hearth, end);
    eprintln!("from the lit hearth: the longest wait with nothing new {gap:.1} s from {from:.1} s, ended by {}", ended.unwrap_or("the end of the run"));
    assert!(gap <= PACING_WAIT, "{gap:.1} s with nothing new from {from:.1} s on the Farm line (the hearth lit at {hearth:.1} s), ended by {}", ended.unwrap_or("the end of the run"));
}

/// Something new comes every three minutes of the line, several things, and no one action
/// is done over and over for long (docs/first-days.md, "Pacing"): the report printed, and
/// the run failed past either floor.
fn check_pacing(f: &mut Fresh) {
    f.look_round();
    let end = f.secs();
    f.pacing.borrow_mut().end(end);
    let pacing = f.pacing.borrow();
    let report = pacing_report(&pacing, end);
    eprintln!("{report}");
    for (i, window) in pacing.windows(PACING_WINDOW, end).iter().enumerate() {
        let full = (i as f32 + 1.0) * PACING_WINDOW <= end;
        let new = window.values().map(Vec::len).sum::<usize>();
        assert!(!full || new >= PACING_NEW, "only {new} new things in three minutes from {} s:\n{report}", i as f32 * PACING_WINDOW);
    }
    assert!(pacing.longest.1 <= PACING_STRETCH, "one action over and over for too long:\n{report}");
}

/// The pacing check's window: new things are counted in each three minutes of the run.
const PACING_WINDOW: f32 = 180.0;

/// The fewest new things each whole window must bring (14 to 43 in 32 runs, Oct 20).
const PACING_NEW: usize = 5;

/// The longest the player may do one action over and over, in server seconds (firing
/// clay at the pit kiln, 29 to 40 s in 32 runs, Oct 20).
const PACING_STRETCH: f32 = 60.0;

/// The longest the Nomad line from the first campfire to the lit hearth, and the Farm line
/// from the lit hearth on, may go with nothing new, in server seconds: no longer than one
/// action may go on (`PACING_STRETCH`).
const PACING_WAIT: f32 = PACING_STRETCH;

/// The pacing check's report: what was new in each window of the run, by kind, and the
/// longest stretch of one action over and over.
fn pacing_report(pacing: &Pacing, end: f32) -> String {
    let mut out = String::from("pacing: new things per three minutes (item, place, creature, station)\n");
    for (i, window) in pacing.windows(PACING_WINDOW, end).iter().enumerate() {
        let from = i as f32 * PACING_WINDOW;
        let count = |kind| window.get(&kind).map_or(0, Vec::len);
        let all: Vec<String> = window.iter().map(|(kind, names)| format!("{kind:?}: {}", names.join(", "))).collect();
        out += &format!(
            "  {:>5.0} to {:>5.0} s: {:>3} new ({} items, {} places, {} creatures, {} stations)  {}\n",
            from,
            (from + PACING_WINDOW).min(end),
            window.values().map(Vec::len).sum::<usize>(),
            count(Novelty::Item),
            count(Novelty::Place),
            count(Novelty::Creature),
            count(Novelty::Station),
            all.join("; ")
        );
    }
    let (gap, from, ended) = pacing.longest_gap(0.0, end);
    out += &format!("  longest wait with nothing new: {gap:.1} s from {from:.1} s, ended by {}\n", ended.unwrap_or("the end of the run"));
    let (what, secs, times) = &pacing.longest;
    out += &format!("  longest single-action stretch: {what}, {times} times over {secs:.1} s\n");
    out += &format!("  the end of the line at {:.1} server min: a floor, since the bot knows the way (a new player's target is 20 to 30)", end / 60.0);
    out
}

/// Each milestone's server seconds, the phase before it and what that went on, and the
/// levels after it: the timeline docs/first-days.md keeps.
fn timeline(f: &Fresh) -> String {
    let mut out = String::from("milestone           at s   phase s  mining walking hunting cooking other  levels after\n");
    let mut before = (0.0f32, [0.0f32; 4]);
    for m in &f.marks {
        let spent: Vec<f32> = m.acts.iter().zip(before.1).map(|(now, then)| now - then).collect();
        let phase = m.at - before.0;
        let other = (phase - spent.iter().sum::<f32>()).max(0.0);
        let levels: Vec<String> = m.levels.iter().map(|(skill, level)| format!("{skill} {level}")).collect();
        let side = if m.side.is_empty() { String::new() } else { format!("; side quests {}", m.side.join(", ")) };
        out += &format!("{:<18} {:>6.1} {:>8.1} {:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>5.0}  {}{side}\n", m.name, m.at, phase, spent[0], spent[1], spent[2], spent[3], other, levels.join(", "));
        before = (m.at, m.acts);
    }
    out
}

/// What the server says of a quest: whether it is done, and whether one of its tasks holds.
fn quest_status(c: &Client, quest: &str) -> (bool, bool) {
    let name = format!("base:{quest}");
    c.quests.iter().find(|q| q.name == name).map_or((false, false), |q| (q.done, q.tasks.iter().any(|t| t.iter().all(|&(have, need)| have >= need))))
}

/// The quest each milestone is, as the server counts it.
const FRESH_QUESTS: &[(&str, &str)] = &[
    ("look_it_up", "look_it_up"),
    ("timber", "timber"),
    ("crafting_table", "crafting_table"),
    ("an_axe_to_it", "an_axe_to_it"),
    ("stone_in_hand", "stone_in_hand"),
    ("first_night", "first_night"),
    ("a_tempting_bite", "a_tempting_bite"),
    ("a_bed_of_your_own", "a_bed_of_your_own"),
    ("black_flecks", "black_flecks"),
    ("coal_and_cinders", "coal_and_cinders"),
    ("torchlight", "torchlight"),
    ("hearth_lit", "homestead_hearth"),
    ("pit_kiln", "a_pit_kiln"),
    ("fire_the_clay", "fire_the_clay"),
    ("the_potter", "the_potter"),
    ("a_kiln_load", "a_kiln_load"),
    ("fuel_to_hand", "fuel_to_hand"),
    ("kiln_plans", "kiln_plans"),
    ("a_clay_store", "a_clay_store"),
    ("first_bricks", "first_bricks"),
    ("the_kiln_ring", "the_kiln_ring"),
    ("the_kiln_charge", "the_kiln_charge"),
    ("charcoal", "charcoal"),
    ("brick_by_brick", "brick_by_brick"),
    ("the_crucible", "the_crucible"),
    ("crucible_kiln", "crucible_kiln"),
];

/// Checks the server's own books agree with what the player did, prints the run's one summary
/// block, and holds each milestone to its budget.
fn finish(mut f: Fresh) {
    assert_eq!(f.c.died, 0, "the player died on the way");
    let mut report = String::from("fresh survival player, server seconds since the first action\n");
    let mut before = (0.0f32, [0.0f32; 4]);
    for (name, at, acts) in f.marks.iter().map(|m| (m.name, m.at, m.acts)).collect::<Vec<_>>() {
        let quest = FRESH_QUESTS.iter().find(|(mark, _)| *mark == name).map(|(_, quest)| *quest);
        // The server checks quests once a second, so give it a few.
        if let Some(quest) = quest {
            f.wait(5.0, &format!("the server counting {quest}"), |c| quest_status(c, quest) != (false, false));
        }
        let status = quest.map_or(String::new(), |q| match quest_status(&f.c, q) {
            (true, _) => format!("; quest {q} done"),
            _ => format!("; quest {q} held, not done (it waits on earlier quests)"),
        });
        let spent: Vec<String> = ["mining", "walking", "hunting", "cooking"].iter().zip(acts.iter().zip(before.1)).map(|(what, (now, then))| format!("{what} {:.0}", now - then)).collect();
        let phase = at - before.0;
        let other = phase - acts.iter().zip(before.1).map(|(now, then)| now - then).sum::<f32>();
        report += &format!("  {name:<18} {at:>6.1} s  (phase {phase:>5.1} s: {}, other {:.0}{status})\n", spent.join(", "), other.max(0.0));
        before = (at, acts);
    }
    let settle = f.c.quests.iter().find(|q| q.name == "base:where_to_settle").map_or((0, 0), |q| q.tasks[0][0]);
    report += &format!(
        "  blocks mined {}, animals killed {} (lane restocked {} times), biomes seen {} of {} for Where to Settle, total {:.1} s",
        f.mined, f.c.deaths, f.restocks, settle.0, settle.1, f.secs()
    );
    eprintln!("{report}");
    // The Nomad line paced as the Farm's is, by the same floors, and from the first campfire
    // to the lit hearth never longer with nothing new than one action may go on.
    check_pacing(&mut f);
    let at = |name: &str| f.marks.iter().find(|m| m.name == name).unwrap_or_else(|| panic!("{name} was never reached")).at;
    let (fire, hearth) = (at("first_night"), at("hearth_lit"));
    {
        let pacing = f.pacing.borrow();
        let (gap, from, ended) = pacing.longest_gap(fire, hearth);
        eprintln!("from the first campfire to the lit hearth: the longest wait with nothing new {gap:.1} s from {from:.1} s, ended by {}", ended.unwrap_or("the lit hearth"));
        assert!(gap <= PACING_WAIT, "{gap:.1} s with nothing new from {from:.1} s on the Nomad line (the first campfire at {fire:.1} s, the hearth lit at {hearth:.1} s), ended by {}", ended.unwrap_or("the lit hearth"));
    }
    for &(name, budget) in FRESH_BUDGETS {
        let m = f.marks.iter().find(|m| m.name == name).unwrap_or_else(|| panic!("{name} was never reached"));
        assert!(m.at <= budget, "{name} took {:.1} server seconds, over its budget of {budget}", m.at);
    }
}

/// The charm slot (`combat::Slot::Charm`): a charm goes on only there, never in an armour
/// slot, and its `Wearing` effect holds while it is worn, untimed: Old Tusk's charge charm
/// quickens the stride. Held in hand it does nothing; taken off, the effect is gone.
#[test]
fn a_charm_goes_in_the_charm_slot_and_works_while_worn() {
    use voxmar_engine::combat::Slot;
    use voxmar_engine::inventory::{HOTBAR_SLOTS, WORN};
    let mut client = Client::join();
    let charm = client.content.blocks.id("base:charge_charm").unwrap();
    for line in ["/clear", "/effect clear", "/give charge_charm 1"] {
        client.ok(line);
    }
    client.pump_until(|c| (0..HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == charm)));
    let striding = |c: &Client| c.vitals.effects.iter().find(|a| c.content.effects.get(a.effect).name == "base:tusk_stride").copied();
    // In hand it is no charm worn.
    client.pump_ticks(TICKS_PER_SECOND as u64 * 2, |c| striding(c).is_some());
    assert!(striding(&client).is_none(), "held, not worn");
    let from = (0..HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == charm)).unwrap();
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: WORN as u8, count: 1 });
    client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: (WORN + Slot::Charm.index()) as u8, count: 1 });
    client.pump_until(|c| c.inventory.worn[Slot::Charm.index()].is_some_and(|s| s.item == charm) && striding(c).is_some());
    assert!(client.inventory.worn[0].is_none(), "a charm is not a helmet");
    assert_eq!(striding(&client).unwrap().seconds_left, None, "it holds while worn, untimed");
    assert!(client.content.effects.modifiers(&client.vitals.effects).speed > 1.0);
    client.ok("/clear charge_charm");
    client.pump_until(|c| c.inventory.worn[Slot::Charm.index()].is_none() && striding(c).is_none());
}

/// A charm's effect ends the moment it comes off, not at the next once-a-second pass:
/// taken off into the pack, or left in the grave by a death (worth less than three things
/// carried that death keeps first), the stride is gone as the inventory says so.
#[test]
fn a_charm_taken_off_or_lost_to_death_ends_its_effect_at_once() {
    use voxmar_engine::combat::Slot;
    use voxmar_engine::inventory::{HOTBAR_SLOTS, WORN};
    let mut client = Client::join();
    client.ok("/gamemode survival");
    let charm = client.content.blocks.id("base:charge_charm").unwrap();
    let striding = |c: &Client| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == "base:tusk_stride");
    let well = WORN + Slot::Charm.index();
    let wear = |client: &mut Client| {
        client.pump_until(|c| (0..HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == charm)));
        let from = (0..HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == charm)).unwrap();
        client.propose("core:transfer", ActionTarget::Slots { from: from as u8, to: well as u8, count: 1 });
        client.pump_until(|c| c.inventory.worn[Slot::Charm.index()].is_some() && striding(c));
    };
    for line in ["/clear", "/effect clear", "/give charge_charm 1"] {
        client.ok(line);
    }
    wear(&mut client);
    // Off into the pack: the same tick's word takes the stride away.
    client.propose("core:quick_move", ActionTarget::Slot(well as u8));
    client.pump_until(|c| c.inventory.worn[Slot::Charm.index()].is_none());
    client.pump_ticks(1, |_| false);
    assert!(!striding(&client), "the stride outlived the charm taken off");
    // Worn again, then a death: three things worth more are kept, the charm is not.
    let kept = (0..client.content.blocks.len() as u16)
        .map(voxmar_engine::block::BlockId)
        .filter(|&item| voxmar_engine::value::worth(&client.content, item) > voxmar_engine::value::worth(&client.content, charm))
        .find(|&item| client.content.blocks.def(item).item && voxmar_engine::inventory::worn_slot(item).is_none())
        .expect("something worth more than the charm");
    let kept_name = client.content.blocks.def(kept).name.clone();
    for line in ["/clear".to_owned(), format!("/give {kept_name} 3"), "/give charge_charm 1".to_owned()] {
        client.ok(&line);
    }
    // A command's answer can come before the inventory it changed (answered between ticks,
    // the inventory sent at the next): the charm worn before still shows in the hotbar until
    // the three things given do.
    client.pump_until(|c| c.inventory.slots.iter().flatten().filter(|s| s.item == kept).map(|s| s.count).sum::<u8>() == 3);
    wear(&mut client);
    client.ok("/kill");
    client.pump_until(|c| c.health == 0);
    client.propose("core:respawn", ActionTarget::None);
    client.pump_until(|c| c.health > 0);
    client.pump_ticks(1, |_| false);
    assert!(client.inventory.worn[Slot::Charm.index()].is_none(), "the charm went to the grave");
    assert!(!striding(&client), "the stride outlived the charm lost to death");
}

/// A stone sword's 0.6 s cooldown in server ticks.
const SWORD_TICKS: u64 = 600 * TICKS_PER_SECOND as u64 / 1000;

/// The first fights of the Nomad age as a new player meets them, measured: a bot with a
/// stone sword at Attack 5 stands up to each creature, facing it and swinging on the
/// sword's cooldown, never guarding or dodging, until it dies. For each: how long the kill
/// took, how many blows came back and how hard, and how long before each blow landed its
/// wind-up was seen (the pose coming on), the time there is to read it and answer. Run by
/// hand (`--ignored --nocapture`); it fights on the clock, so a loaded machine stretches it.
#[test]
#[ignore = "a measurement, printed: run by hand"]
fn first_fights_measured() {
    real_time("combat is judged on the clock: the wind-ups read are seconds between messages");
    let mut lines = Vec::new();
    let only = std::env::var("FIGHT_ONLY").ok();
    for name in ["wild_boar", "badger", "fox", "aurochs", "wolf", "brown_bear"].into_iter().filter(|n| only.as_deref().is_none_or(|o| o == *n)) {
        let mut client = Client::join();
        client.ok("/gamemode survival");
        // Enough health to see the fight through: what came back is counted, not survived.
        client.ok("/skill hitpoints 60");
        let species = client.content.creatures.find(name).unwrap();
        let attack = client.content.creatures.get(species).def.attack;
        let (id, _) = beside(&mut client, name);
        let sword = client.content.blocks.id("base:stone_sword").unwrap();
        client.ok("/clear");
        client.ok("/skill attack 5");
        client.ok("/give stone_sword 1");
        client.pump_until(|c| (0..voxmar_engine::inventory::HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == sword)));
        let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == sword)).unwrap();
        let start = Instant::now();
        let (before, mut swung) = (client.hurt.len(), 0u32);
        let mut reads: Vec<f32> = Vec::new();
        let mut unread = 0;
        // Swings on the sword's 0.6 s cooldown, and a look each tick, by the server's clock.
        let mut next_swing = client.ticks();
        let mut read_from = client.wound_at.iter().rev().find(|(by, _)| *by == id).map(|&(_, at)| at);
        while client.creatures.contains_key(&id) && start.elapsed() < Duration::from_secs(90) {
            face(&mut client, id, slot, false);
            if client.ticks() >= next_swing {
                client.propose("core:attack", ActionTarget::Creature(id));
                swung += 1;
                next_swing = client.ticks() + SWORD_TICKS;
            }
            let heard = client.hurt.len();
            let next = client.ticks() + 1;
            client.pump_until(|c| c.ticks() >= next);
            // Each blow read from the wind-up seen last before it, if one was seen since the
            // blow before.
            for i in heard..client.hurt.len() {
                if client.hurt_by[i].0 != id {
                    continue;
                }
                let last = client.wound_at.iter().rev().find(|&&(by, at)| by == id && at <= client.hurt_at[i]).map(|&(_, at)| at);
                match last.filter(|&at| Some(at) != read_from) {
                    Some(at) => {
                        reads.push((client.hurt_at[i] - at).as_secs_f32());
                        read_from = Some(at);
                    }
                    None => unread += 1,
                }
            }
        }
        let killed = !client.creatures.contains_key(&id);
        let blows: Vec<u16> = client.hurt_by[before..].iter().filter(|(by, _)| *by == id).map(|&(_, d)| d).collect();
        let readable = reads.iter().filter(|&&r| r >= 0.3).count();
        let line = format!(
            "{name:<11} windup {:<5} killed {killed:<5} in {:>5.1} s, {swung:>3} swings; {:>2} blows back for {:>2} damage; {readable}/{} read for 0.3 s or more (shortest {:.2} s, {unread} unseen)",
            attack.map_or("none".into(), |a| format!("{:.2}", a.windup.unwrap_or(client.content.combat.feel.creature_windup))),
            start.elapsed().as_secs_f32(),
            blows.len(),
            blows.iter().map(|&d| u32::from(d)).sum::<u32>(),
            blows.len(),
            reads.iter().copied().fold(f32::INFINITY, f32::min),
        );
        eprintln!("first fights: {line}");
        lines.push(line);
    }
    eprintln!("\nfirst fights:\n{}", lines.join("\n"));
}

/// The first-age fights as a brand-new player has them, measured: starting health, Attack
/// 1 and Defence 1, a stone sword (`FIGHT_WEAPON` for another), on an open floor walled
/// round, chasing on foot at a sprint (no teleports) and swinging on the cooldown whenever
/// the creature is in reach. Each creature is fought `FIGHT_RUNS` times (3 unless set)
/// twice over: never guarding, then raising the guard to meet every other wind-up just
/// before it lands (about half the tells answered well). For each: how long the kill took
/// and how much of it was spent chasing, the health left (or a death), and the blows that
/// came back. Run by hand (`--ignored --nocapture`); it fights on the clock, so a loaded
/// machine stretches it.
#[test]
#[ignore = "a measurement, printed: run by hand"]
fn first_age_fights_balanced() {
    real_time("combat is judged on the clock: chases, wind-ups and guards in seconds, not in ticks");
    use voxmar_engine::combat::Defended;
    /// A run: killed, died, seconds, seconds chasing, blows back, damage taken, health left.
    type Run = (bool, bool, f32, f32, usize, u32, f32);
    let only = std::env::var("FIGHT_ONLY").ok();
    let runs: u32 = std::env::var("FIGHT_RUNS").ok().and_then(|r| r.parse().ok()).unwrap_or(3);
    let weapon = std::env::var("FIGHT_WEAPON").unwrap_or_else(|_| "stone_sword".into());
    let modes: Vec<bool> = match std::env::var("FIGHT_GUARD").as_deref() {
        Ok("0") => vec![false],
        Ok("1") => vec![true],
        _ => vec![false, true],
    };
    let mut lines = Vec::new();
    for name in ["wild_boar", "badger", "fox", "aurochs", "wolf", "brown_bear"].into_iter().filter(|n| only.as_deref().is_none_or(|o| o == *n)) {
        for &guarding in &modes {
            let mut results: Vec<Run> = Vec::new();
            for run in 0..runs {
                let mut client = Client::join();
                client.ok("/gamemode survival");
                client.pump_until(|c| c.max_health > 0 && c.health == c.max_health);
                let species = client.content.creatures.find(name).unwrap();
                let def = client.content.creatures.get(species).def.clone();
                let feel = client.content.combat.feel;
                let windup = def.attack.map(|a| a.windup.unwrap_or(feel.creature_windup));
                // An open floor 80 blocks across, walled round so nothing flees off it.
                client.ok("/fill ~-40 ~-1 ~-40 ~40 ~-1 ~40 stone");
                client.ok("/fill ~-40 ~ ~-40 ~40 ~3 ~40 air");
                client.ok("/fill ~-40 ~4 ~-40 ~40 ~7 ~40 air");
                for wall in ["~-40 ~ ~-40 ~40 ~2 ~-40", "~-40 ~ ~40 ~40 ~2 ~40", "~-40 ~ ~-40 ~-40 ~2 ~40", "~40 ~ ~-40 ~40 ~2 ~40"] {
                    client.ok(&format!("/fill {wall} stone"));
                }
                let item = client.content.blocks.id(&format!("base:{weapon}")).unwrap();
                client.ok("/clear");
                client.ok(&format!("/give {weapon} 1"));
                client.pump_until(|c| (0..voxmar_engine::inventory::HOTBAR_SLOTS).any(|s| c.inventory.get(s).is_some_and(|s| s.item == item)));
                let slot = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| client.inventory.get(s).is_some_and(|s| s.item == item)).unwrap();
                client.drain();
                // Each run a few ticks later than the last: the server's rolls go by its tick.
                client.after_server_ticks(7 * u64::from(run));
                let known: std::collections::HashSet<_> = client.creatures.keys().copied().collect();
                client.ok(&format!("/summon {name} 1 ~ ~ ~3"));
                client.pump_until(|c| c.creatures.values().any(|v| v.species == species && !known.contains(&v.id)));
                let id = client.creatures.values().find(|v| v.species == species && !known.contains(&v.id)).unwrap().id;
                let (health, max) = (client.health, client.max_health);
                let (hurt_before, died_before) = (client.hurt.len(), client.died);
                let class = client.content.combat.wielded(Some(item), |_| 1);
                let reach = class.reach;
                let sprint = client.content.movement.sprint_speed * 0.95;
                let floor_y = client.position.y;
                let start = Instant::now();
                let mut last = Instant::now();
                let mut chasing = 0.0f32;
                // Swings on the cooldown, and a look each tick, by the server's clock.
                let mut next_swing = client.ticks();
                let mut swings = 0u32;
                let (mut tells, mut seen) = (0u32, None::<Instant>);
                let mut guard: Option<(Instant, Instant)> = None;
                let mut up = false;
                while client.creatures.contains_key(&id) && client.died == died_before && start.elapsed() < Duration::from_secs(120) {
                    let now = Instant::now();
                    let dt = (now - last).as_secs_f32().min(0.1);
                    last = now;
                    let view = &client.creatures[&id];
                    let at = view.position;
                    let half = def.body.size.0 * view.scale * 0.5;
                    let flat = (at - client.position).with_y(0.0);
                    let edge = flat.length() - half;
                    // Run at it, stopping a step short of touching it.
                    let mut position = client.position;
                    if edge > 1.0 {
                        position += flat.normalize() * (sprint * dt).min(edge - 0.9);
                        position.y = floor_y;
                    }
                    if edge > reach {
                        chasing += dt;
                    }
                    client.position = position;
                    let yaw = yaw_toward(position, at);
                    let _ = client.connection.send(ClientMessage::Input(PlayerInput { teleport: client.teleports, position, yaw, pitch: 0.0, selected_slot: slot as u8, flying: false }));
                    // Every other wind-up met with the guard just before it lands.
                    if let Some(&since) = client.attacking_since.get(&id)
                        && seen != Some(since)
                    {
                        seen = Some(since);
                        tells += 1;
                        if guarding && tells % 2 == 1 {
                            let w = windup.unwrap_or(feel.creature_windup);
                            guard = Some((since + Duration::from_secs_f32((w - 0.1).max(0.0)), since + Duration::from_secs_f32(w + 0.25)));
                        }
                    }
                    match guard {
                        Some((raise, _)) if !up && now >= raise => {
                            let _ = client.connection.send(ClientMessage::Guard { up: true });
                            up = true;
                        }
                        Some((_, lower)) if up && now >= lower => {
                            let _ = client.connection.send(ClientMessage::Guard { up: false });
                            up = false;
                            guard = None;
                        }
                        _ => {}
                    }
                    if !up && client.ticks() >= next_swing && edge <= reach {
                        client.propose("core:attack", ActionTarget::Creature(id));
                        swings += 1;
                        next_swing = client.ticks() + SWORD_TICKS;
                    }
                    let next = client.ticks() + 1;
                    client.pump_until(|c| c.ticks() >= next);
                }
                let killed = !client.creatures.contains_key(&id);
                let died = client.died > died_before;
                let back: Vec<_> = (hurt_before..client.hurt.len()).filter(|&i| client.hurt_by[i].0 == id).map(|i| client.hurt[i]).collect();
                let taken: u32 = back.iter().map(|&(d, _)| u32::from(d)).sum();
                let met = back.iter().filter(|(_, d)| matches!(d, Defended::Perfect | Defended::Blocked)).count();
                let left = if died { 0.0 } else { f32::from(client.health) / f32::from(max) };
                eprintln!(
                    "balance: {name} {} run {run}: killed {killed} died {died} in {:.1} s ({chasing:.1} s chasing), {swings} swings; {} blows back ({met} guarded, {tells} tells) for {taken}; health {}/{max} -> {:.0}% left",
                    if guarding { "guarding" } else { "open" },
                    start.elapsed().as_secs_f32(),
                    back.len(),
                    health,
                    left * 100.0,
                );
                results.push((killed, died, start.elapsed().as_secs_f32(), chasing, back.len(), taken, left));
            }
            let n = results.len() as f32;
            let mean = |f: &dyn Fn(&Run) -> f32| results.iter().map(f).sum::<f32>() / n;
            let line = format!(
                "{name:<11} {:<8} {}/{} killed, {} died; kill {:>5.1} s (chase {:>4.1} s); {:>4.1} blows back for {:>4.1}; {:>3.0}% health left (worst {:.0}%)",
                if guarding { "guarding" } else { "open" },
                results.iter().filter(|r| r.0).count(),
                results.len(),
                results.iter().filter(|r| r.1).count(),
                mean(&|r| r.2),
                mean(&|r| r.3),
                mean(&|r| r.4 as f32),
                mean(&|r| r.5 as f32),
                mean(&|r| r.6) * 100.0,
                results.iter().map(|r| r.6).fold(1.0, f32::min) * 100.0,
            );
            eprintln!("balance: {line}");
            lines.push(line);
        }
    }
    eprintln!("\nfirst-age fights:\n{}", lines.join("\n"));
}

/// A bed a village furnished a home with is its resident's: sleeping in it is refused, out
/// of creative, and it does not become where the player wakes; a bed set beside it, which
/// the village never planned, works.
#[test]
fn a_village_bed_is_someones() {
    let mut client = Client::join_with(ServerConfig { seed: VILLAGE_SEED, view_distance: 2, allow_flight: false, ..Default::default() });
    client.admin = true;
    to_settlement(&mut client, "", "noon");
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let refused = |c: &Client| c.refused.iter().any(|t| t.contains("belongs to someone"));
    let world_content = voxmar_worldgen::WorldContent::load(&client.content).unwrap();
    // The server's own generator (`Overworld::shared`), the village round it already planned.
    let overworld = voxmar_worldgen::Overworld::shared(VILLAGE_SEED, &client.content, &world_content);
    let towns = overworld.settlements_near(client.position.floor().as_ivec3().xz(), 256);
    // A ground-floor bed, by the floor its home stands on.
    let (home, bed) = towns.iter().flat_map(|t| &t.buildings).find_map(|b| b.beds.iter().find(|p| p.y == b.floor + 1).map(|&p| (b.clone(), p))).expect("a village home with a bed on its ground floor");
    let open = |c: &Client, p: IVec3| c.world.block(p).is_some_and(|b| !c.content.blocks.def(b).solid);
    client.ok(&format!("/tp {} {} {}", bed.x as f32 + 0.5, bed.y + 1, bed.z as f32 + 0.5));
    client.pump_until(|c| c.world.block(bed).is_some_and(|b| c.content.blocks.def(b).has_tag("bed")));
    // Stand beside it, on the floor of the room, where the way to it is open.
    let side = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].into_iter().map(|d| bed + d).find(|&p| open(&client, p) && open(&client, p + IVec3::Y) && home.room.contains(&p.xz()))
        .unwrap_or_else(|| panic!("the {} bed at {bed} has no open side", home.what));
    client.ok(&format!("/tp {} {} {}", side.x as f32 + 0.5, side.y, side.z as f32 + 0.5));
    client.pump_until(|c| c.position.distance(side.as_vec3() + Vec3::new(0.5, 0.0, 0.5)) < 1.0);
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(refused);
    assert!(!told(&client, "wake here"), "a village bed took as where the player wakes");
    // One set beside it works: the village never planned it. The player stands on the
    // village bed meanwhile, as no block is set where someone stands.
    let stand_on = |client: &mut Client, p: IVec3| {
        client.ok(&format!("/tp {} {} {}", p.x as f32 + 0.5, p.y + 1, p.z as f32 + 0.5));
        client.pump_until(|c| c.position.xz().distance(p.xz().as_vec2() + 0.5) < 0.5);
    };
    stand_on(&mut client, bed);
    client.ok(&format!("/setblock {} {} {} oak_bed", side.x, side.y, side.z));
    let oak = client.content.blocks.expect("base:oak_bed");
    client.pump_until(|c| c.world.block(side) == Some(oak));
    client.chat.clear();
    client.refused.clear();
    client.propose("base:sleep", ActionTarget::Block { block: side, face: Face::PosY });
    client.pump_until(|c| told(c, "wake here"));
    assert!(!refused(&client), "a bed set by command in the village was refused");
    // So does one set in the village bed's own place: it is no longer the bed the village put
    // there. The player stands on the new bed meanwhile.
    stand_on(&mut client, side);
    client.ok(&format!("/setblock {} {} {} sleeping_bag", bed.x, bed.y, bed.z));
    let bag = client.content.blocks.expect("base:sleeping_bag");
    client.pump_until(|c| c.world.block(bed) == Some(bag));
    client.chat.clear();
    client.refused.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| told(c, "wake here"));
    assert!(!refused(&client), "a bag set in a village bed's place was refused");
}

/// Soft things count at home (docs/home.md Rooms): a rug and a wall hanging by a slept-in
/// bag under a roof make Soft furnishings, announced; the hanging taken down, it goes.
#[test]
fn a_rug_and_a_hanging_make_soft_furnishings() {
    let mut client = on_a_test_floor();
    let feet = client.position.floor().as_ivec3();
    let has = |c: &Client, name: &str| c.vitals.effects.iter().any(|a| c.content.effects.get(a.effect).name == name);
    let told = |c: &Client, text: &str| c.chat.iter().any(|(_, t)| t.contains(text));
    let bag = feet + IVec3::new(2, 0, 0);
    let (rug, hanging) = (feet + IVec3::new(1, 0, 2), feet + IVec3::new(3, 1, -2));
    for line in [
        format!("/setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z),
        format!("/fill {} {} {} {} {} {} stone", feet.x - 2, feet.y + 3, feet.z - 3, feet.x + 6, feet.y + 3, feet.z + 3),
        format!("/setblock {} {} {} red_rug", rug.x, rug.y, rug.z),
    ] {
        client.ok(&line);
    }
    client.pump_until(|c| c.world.block(bag) == c.content.blocks.id("base:sleeping_bag") && c.world.block(rug) == c.content.blocks.id("base:red_rug"));
    client.propose("base:sleep", ActionTarget::Block { block: bag, face: Face::PosY });
    client.pump_until(|c| has(c, "base:rested"));
    assert!(!has(&client, "base:soft_furnishings"), "a rug alone is not soft furnishings");
    client.chat.clear();
    client.ok(&format!("/setblock {} {} {} red_wall_hanging", hanging.x, hanging.y, hanging.z));
    client.pump_until(|c| has(c, "base:soft_furnishings") && told(c, "Soft furnishings:"));
    client.ok(&format!("/setblock {} {} {} air", hanging.x, hanging.y, hanging.z));
    client.pump_until(|c| !has(c, "base:soft_furnishings"));
}

/// A bed in a town's farthest house, over 64 blocks from its middle, is as much the
/// town's as one by its well: sleep there is refused, as the bed's settlement is the one
/// whose square holds it, not one found within a reach of it.
#[test]
fn a_bed_far_out_in_a_town_is_someones() {
    let seed = 0x5eed;
    let mut client = Client::join_with(ServerConfig { seed, view_distance: 2, allow_flight: false, ..Default::default() });
    client.admin = true;
    let world_content = voxmar_worldgen::WorldContent::load(&client.content).unwrap();
    // The server's own generator (`Overworld::shared`): the town is planned once, for both.
    let overworld = voxmar_worldgen::Overworld::shared(seed, &client.content, &world_content);
    let town = overworld.nearest_settlement(client.position.floor().as_ivec3().xz(), 10000, |s| s.size == "town").expect("a town");
    // Its farthest ground-floor bed from the middle, and the home it stands in.
    let (far, home, bed) = town
        .buildings
        .iter()
        .flat_map(|b| b.beds.iter().filter(|p| p.y == b.floor + 1).map(move |&p| ((p.xz() - town.centre.xz()).as_vec2().length(), b, p)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("a town home with a bed on its ground floor");
    assert!(far > 64.0, "{}'s farthest bed is only {far:.0} blocks from its middle", town.name.0);
    let open = |c: &Client, p: IVec3| c.world.block(p).is_some_and(|b| !c.content.blocks.def(b).solid);
    client.ok("/time set noon");
    client.ok(&format!("/tp {} {} {}", bed.x as f32 + 0.5, bed.y + 1, bed.z as f32 + 0.5));
    client.pump_until(|c| c.world.block(bed).is_some_and(|b| c.content.blocks.def(b).has_tag("bed")));
    let side = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z].into_iter().map(|d| bed + d).find(|&p| open(&client, p) && open(&client, p + IVec3::Y) && home.room.contains(&p.xz()))
        .unwrap_or_else(|| panic!("the {} bed at {bed} has no open side", home.what));
    client.ok(&format!("/tp {} {} {}", side.x as f32 + 0.5, side.y, side.z as f32 + 0.5));
    client.pump_until(|c| c.position.distance(side.as_vec3() + Vec3::new(0.5, 0.0, 0.5)) < 1.0);
    client.chat.clear();
    client.propose("base:sleep", ActionTarget::Block { block: bed, face: Face::PosY });
    client.pump_until(|c| c.refused.iter().any(|t| t.contains("belongs to someone")));
    assert!(!client.chat.iter().any(|(_, t)| t.contains("wake here")), "the bed {far:.0} blocks out in {} took as where the player wakes", town.name.0);
}

/// A quest's experience reward in tenths, as the quest book pays it: the content's own
/// number, which carries the book's scale (`core/quests.ron` `pays`).
fn quest_xp(client: &Client, quest: &str) -> u32 {
    let rewards = &client.content.quests.get(quest).unwrap_or_else(|| panic!("no quest {quest}")).rewards;
    rewards
        .iter()
        .find_map(|r| match r {
            voxmar_engine::quest::Reward::Xp(_, xp) => Some((xp * 10.0).round() as u32),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{quest} pays no experience"))
}
