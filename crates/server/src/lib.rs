//! The authoritative game server. Runs headless as `voxmar-server`, or on a thread inside
//! the client for singleplayer.

mod actions;
mod commands;
/// The weather in words, for the debug overlay as for `/weather`.
pub use commands::{weather_name, weather_numbers};
mod cooking;
mod creatures;
mod fishing;
mod death;
mod hearth;
mod journal;
mod quests;
mod regrow;
mod farm;
mod husbandry;
mod inspect;
mod fluids;
mod items;
mod player;
mod process;
pub mod save;
mod villagers;
mod weather;
mod wear;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::content::Content;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, WORLD_HEIGHT};
use voxmar_engine::field::Field;
use voxmar_engine::fluid::{Budget, Fluids};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::{Column, VoxelWorld};
use voxmar_protocol::{PlayerId, ServerConnection, ServerMessage, TICKS_PER_SECOND};
use voxmar_worldgen::{Generator, Overworld, WorldContent};

use crate::actions::Executor;
use crate::player::Player;
use crate::process::Process;
use crate::save::{GameMode, WorldMeta, WorldSave};
use crate::weather::WeatherState;

pub struct ServerConfig {
    /// Seed of a new world. A saved world keeps its own.
    pub seed: u64,
    /// Largest radius, in columns, streamed around a player. Each player chooses their own
    /// up to this.
    pub view_distance: i32,
    pub allow_flight: bool,
    /// Whether the integrated server's own (in-process) player is an admin.
    pub local_admin: bool,
    /// Names of players with admin rights, besides the integrated server's own player.
    /// Names are not authenticated, so this only suits servers among friends.
    pub admins: Vec<String>,
    /// Directory the world is saved in; a new world is created there if it holds none. A
    /// saved world's game mode replaces `allow_flight`. `None` keeps the world in memory
    /// only.
    pub world: Option<PathBuf>,
    /// Makes the server run behind, to see how clients cope, as on a busy machine. None
    /// but in tests.
    pub lag: Lag,
}

/// How a server made to run behind does so (see `ServerConfig::lag`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lag {
    /// Extra time every tick takes; over a tick's 50 ms, the server keeps falling behind.
    pub per_tick: Duration,
    /// Extra time every `stall_every`th tick takes besides: a stall, during which players'
    /// messages wait, as when a burst of streaming or a slow tick holds the server up.
    pub stall: Duration,
    pub stall_every: u32,
    /// A stall just as every other action is proposed, so the server's clock for it starts
    /// that much after the player's while the next one's does not.
    pub proposal_stall: Duration,
}

impl Lag {
    /// The extra time tick number `tick` takes.
    fn of_tick(&self, tick: u64) -> Duration {
        let stalls = self.stall_every > 0 && tick.is_multiple_of(self.stall_every as u64);
        self.per_tick + if stalls { self.stall } else { Duration::ZERO }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            seed: 0x5eed,
            view_distance: 32,
            allow_flight: true,
            local_admin: true,
            admins: Vec::new(),
            world: None,
            lag: Lag::default(),
        }
    }
}

impl GameMode {
    /// Creative allows flight; survival does not. Either way the singleplayer host is an
    /// admin, who may change a player's mode with `/gamemode`.
    fn apply(self, config: &mut ServerConfig) {
        config.allow_flight = self == GameMode::Creative;
    }

    fn allows_flight(self) -> bool {
        self == GameMode::Creative
    }
}

/// How often edited columns, players and the time of day are saved.
const SAVE_INTERVAL: u64 = 30 * TICKS_PER_SECOND as u64;
/// Time a new world starts at: early morning.
const FIRST_TICK: u64 = 2000;

/// Finds the `assets` directory: `$VOXMAR_ASSETS`, then next to the working directory or
/// the executable (including cargo's `target/<profile>` layout).
pub fn assets_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VOXMAR_ASSETS") {
        return dir.into();
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let candidates = std::env::current_dir()
        .ok()
        .into_iter()
        .chain(exe_dir.iter().flat_map(|d| d.ancestors().take(3).map(Path::to_path_buf)));
    for base in candidates {
        let dir = base.join("assets");
        if dir.join("content").is_dir() {
            return dir;
        }
    }
    PathBuf::from("assets")
}

/// Loads every content package under `assets/content`, and checks the world
/// generator's definitions in them, so a broken package fails at startup.
pub fn load_content(assets: &Path) -> Result<Content, Box<dyn std::error::Error>> {
    let content = Content::load(&assets.join("content"))?;
    WorldContent::load(&content)?;
    wear::WearRules::load(&content.files, &content.blocks, 0)?;
    Ok(content)
}

/// A running server thread. Dropping it stops the server.
pub struct ServerHandle {
    connections: Sender<ServerConnection>,
    console: Sender<String>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    tick_times: Arc<Mutex<VecDeque<Duration>>>,
    ticks: Arc<AtomicU64>,
}

/// Ticks whose times a server keeps, for `ServerHandle::tick_times`: the last minute.
const TICK_TIMES: usize = 60 * TICKS_PER_SECOND as usize;

impl ServerHandle {
    /// How long the server's last ticks took to simulate, oldest first (up to a minute's
    /// worth), for benchmarks and diagnostics.
    pub fn tick_times(&self) -> Vec<Duration> {
        self.tick_times.lock().map(|t| t.iter().copied().collect()).unwrap_or_default()
    }

    /// How many ticks the server has simulated: its own clock, which a server running behind
    /// on a busy machine keeps, where wall-clock seconds do not.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    /// Sender for new connections (local or accepted by a TCP listener).
    pub fn connections(&self) -> Sender<ServerConnection> {
        self.connections.clone()
    }

    /// Runs a line typed into the server's own console, as an admin standing at spawn: a
    /// command (`time set noon`, with or without the `/`), or `say …` to tell everyone.
    /// Replies are logged.
    pub fn console(&self, line: impl Into<String>) {
        let _ = self.console.send(line.into());
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts a server thread. With `config.world` set, opens the world saved there (or
/// creates it); the server saves it periodically and when the handle is dropped.
pub fn spawn(mut config: ServerConfig, content: Content) -> std::io::Result<ServerHandle> {
    let (save, meta) = open_world(&mut config)?;
    let (connections, incoming) = unbounded();
    let (console, lines) = unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let tick_times = Arc::new(Mutex::new(VecDeque::with_capacity(TICK_TIMES)));
    let ticks = Arc::new(AtomicU64::new(0));
    let thread = {
        let (stop, tick_times, ticks) = (stop.clone(), tick_times.clone(), ticks.clone());
        std::thread::Builder::new()
            .name("server".into())
            .spawn(move || Server::new(config, meta, save, content, incoming, lines).run(&stop, &tick_times, &ticks))?
    };
    Ok(ServerHandle { connections, console, stop, thread: Some(thread), tick_times, ticks })
}

/// The world `config` asks for: the one saved in `config.world` (created if missing), or
/// an unsaved one.
fn open_world(config: &mut ServerConfig) -> std::io::Result<(Option<WorldSave>, WorldMeta)> {
    let new_mode = if config.allow_flight { GameMode::Creative } else { GameMode::Survival };
    let Some(dir) = &config.world else { return Ok((None, WorldMeta::new("world", config.seed, new_mode))) };
    let save = WorldSave::new(dir);
    let mut meta = if save.exists() {
        save.read_meta()?
    } else {
        let name = dir.file_name().map_or_else(|| "world".into(), |name| name.to_string_lossy().into_owned());
        let meta = WorldMeta::new(name, config.seed, new_mode);
        save.create(&meta)?;
        tracing::info!("created world {}", dir.display());
        meta
    };
    if meta.generator != Overworld::ID || meta.generator_version != Overworld::VERSION {
        tracing::warn!(
            "{} was made by {} version {}; new terrain comes from {} version {} and may not line up",
            dir.display(),
            meta.generator,
            meta.generator_version,
            Overworld::ID,
            Overworld::VERSION,
        );
    }
    meta.mode.apply(config);
    meta.last_played = save::unix_time();
    save.write_meta(&meta)?;
    Ok((Some(save), meta))
}

/// A column as it was loaded: from a save (`saved`), or freshly generated.
pub(crate) struct Loaded {
    column: Column,
    saved: bool,
}

/// A saved column if there is one, with its worn ground healed up to `today`, otherwise a
/// freshly generated one. Runs on workers.
pub(crate) fn load_column(
    save: Option<&WorldSave>,
    generator: &dyn Generator,
    registry: &BlockRegistry,
    wear: &wear::WearRules,
    today: u32,
    pos: ColumnPos,
) -> Loaded {
    if let Some(save) = save {
        match save.read_column(pos, registry) {
            Ok(Some(mut column)) => {
                wear.heal(&mut column, pos.origin(), today, registry);
                return Loaded { column, saved: true };
            }
            Ok(None) => {}
            Err(e) => tracing::error!("could not load column {pos:?}, generating it again: {e}"),
        }
    }
    Loaded { column: generator.generate(pos), saved: false }
}

pub(crate) struct Server {
    config: ServerConfig,
    meta: WorldMeta,
    save: Option<WorldSave>,
    content: Content,
    generator: Arc<dyn Generator>,
    /// The same generator, for what commands ask about the land.
    overworld: Arc<Overworld>,
    executor: Executor,
    /// Dropped items lying about.
    items: items::Items,
    /// Crops players planted.
    farm: farm::Farm,
    /// Blocks players placed that would teach a skill if broken (see `WorldMeta::placed`).
    placed: FxHashSet<IVec3>,
    /// Picked wild plants waiting to grow back (see `regrow`).
    regrowing: FxHashMap<IVec3, regrow::Regrowing>,
    /// When the player message being handled arrived (see `actions`).
    arrival: Instant,
    /// Animals and monsters about.
    creatures: creatures::Creatures,
    /// The people of settlements near players.
    villagers: villagers::Villagers,
    processes: Vec<Box<dyn Process>>,
    /// `core:temperature`, mean air temperature in °C, baseline from the climate generator.
    temperature: Field,
    /// How ground wears under feet and heals.
    wear_rules: Arc<wear::WearRules>,
    /// Humidity, cloud and precipitation, evolved by the weather process, and the weather
    /// admins have forced.
    weather: WeatherState,
    world: VoxelWorld,
    /// Water and lava waiting to flow, and how much of it a tick may move.
    fluids: Fluids,
    fluid_budget: Budget,
    /// Seconds the last tick took.
    tick_seconds: f64,
    /// Columns edited since they were last saved. They stay loaded until saved; without a
    /// save, that is forever.
    dirty: FxHashSet<ColumnPos>,
    generating: FxHashSet<ColumnPos>,
    generated_tx: Sender<(ColumnPos, Loaded)>,
    generated_rx: Receiver<(ColumnPos, Loaded)>,
    incoming: Receiver<ServerConnection>,
    /// Lines typed into the server's own console.
    console: Receiver<String>,
    players: FxHashMap<PlayerId, Player>,
    next_player: PlayerId,
    spawn: Vec3,
    /// Simulation ticks since the world began.
    tick: u64,
    /// The time of day, in ticks since the first sunrise; it runs `day_speed` ticks per
    /// tick, and commands may set it.
    day: f64,
    day_speed: f32,
}

const MAX_GENERATING: usize = 64;
const COLUMNS_PER_TICK: usize = 48;

impl Server {
    fn new(
        config: ServerConfig,
        meta: WorldMeta,
        save: Option<WorldSave>,
        content: Content,
        incoming: Receiver<ServerConnection>,
        console: Receiver<String>,
    ) -> Self {
        let world_content = WorldContent::load(&content).expect("checked by load_content");
        let overworld = Overworld::shared(meta.seed, &content, &world_content);
        let climate = overworld.clone();
        let temperature = Field::new("core:temperature", move |x, z| climate.site(x, z).temperature);
        let land = Arc::new(weather::OverworldLand { overworld: overworld.clone(), blocks: content.blocks.clone() });
        let chances = weather::chances(&content.weather, &overworld.dials().featured);
        let weather = WeatherState::new(meta.seed, overworld.dials().wind.into(), land, content.weather.clone(), chances);
        let processes = process::core_processes();
        let generator: Arc<dyn Generator> = overworld.clone();
        // Load the spawn column up front and stand on its highest solid block, so
        // features like trees never bury the player.
        let mut world = VoxelWorld::default();
        let spawn_block = generator.spawn_point();
        let spawn_column = ColumnPos::of_block(spawn_block);
        let tick = meta.time.max(FIRST_TICK);
        let wear_rules = Arc::new(wear::WearRules::load(&content.files, &content.blocks, meta.seed).expect("checked by load_content"));
        let today = wear::day(meta.day.unwrap_or(tick) as f64);
        let loaded = load_column(save.as_ref(), &*generator, &content.blocks, &wear_rules, today, spawn_column);
        world.insert(spawn_column, Arc::new(loaded.column));
        let mut fluids = Fluids::default();
        fluids.column_loaded(&content.blocks, &world, spawn_column, tick, loaded.saved);
        let spawn = find_ground(&world, &content.blocks, spawn_block);
        let (generated_tx, generated_rx) = unbounded();
        tracing::info!("world seed {:#x}, spawn at {spawn}", meta.seed);
        let saved_items = save.as_ref().map_or(Ok(Vec::new()), |save| save.read_items()).unwrap_or_else(|e| {
            tracing::error!("could not load dropped items: {e}");
            Vec::new()
        });
        let items = items::Items::restore(saved_items, |name| content.blocks.id(name));
        let saved_crops = save.as_ref().map_or(Ok(Vec::new()), |save| save.read_crops()).unwrap_or_else(|e| {
            tracing::error!("could not load crops: {e}");
            Vec::new()
        });
        let crops = &content.crops;
        let farm = farm::Farm::restore(saved_crops, |name| {
            crops.iter().find(|(_, c)| c.def.name == name).map(|(id, c)| (id, c.stages.len() as u8))
        });
        let placed = meta.placed.iter().map(|&p| IVec3::from_array(p)).collect();
        let regrowing = regrow::restore(&meta.regrowing, |name| content.blocks.id(name));
        let mut server = Self {
            config,
            tick,
            day: meta.day.unwrap_or(tick) as f64,
            day_speed: meta.day_speed,
            meta,
            save,
            content,
            generator,
            overworld,
            executor: Executor::default(),
            items,
            farm,
            placed,
            regrowing,
            arrival: Instant::now(),
            creatures: creatures::Creatures::default(),
            villagers: villagers::Villagers::default(),
            processes,
            temperature,
            wear_rules,
            weather,
            world,
            fluids,
            fluid_budget: Budget::default(),
            tick_seconds: 0.0,
            dirty: FxHashSet::default(),
            generating: FxHashSet::default(),
            generated_tx,
            generated_rx,
            incoming,
            console,
            players: FxHashMap::default(),
            next_player: 1,
            spawn,
        };
        server.show_grown_crops(spawn_column);
        let saved_creatures = server.save.as_ref().map_or(Ok(Vec::new()), |save| save.read_creatures()).unwrap_or_else(|e| {
            tracing::error!("could not load creatures: {e}");
            Vec::new()
        });
        server.restore_creatures(saved_creatures);
        let dead = server.save.as_ref().map_or(Ok(Vec::new()), |save| save.read_villagers()).unwrap_or_else(|e| {
            tracing::error!("could not load who of the villagers died: {e}");
            Vec::new()
        });
        server.villagers = villagers::Villagers::restore(dead);
        server
    }

    /// Runs fixed-rate ticks until `stop` is set, then saves. Between ticks, messages are
    /// handled as soon as they arrive so actions feel immediate.
    fn run(mut self, stop: &AtomicBool, tick_times: &Mutex<VecDeque<Duration>>, ticks: &AtomicU64) {
        let tick_length = Duration::from_secs(1) / TICKS_PER_SECOND;
        let mut next_tick = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            self.accept_connections();
            self.receive_generated();
            // Timed actions due by now complete after the messages that came before now,
            // which may have cancelled them.
            let now = Instant::now();
            self.handle_messages();
            self.complete_actions(now);
            while let Ok(line) = self.console.try_recv() {
                self.console(&line);
            }
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                ticks.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut times) = tick_times.lock() {
                    if times.len() == TICK_TIMES {
                        times.pop_front();
                    }
                    times.push_back(now.elapsed());
                }
                std::thread::sleep(self.config.lag.of_tick(self.tick));
                next_tick += tick_length;
                if now > next_tick + tick_length * 10 {
                    tracing::warn!("server is running behind, skipping ticks");
                    next_tick = now;
                }
            }
            std::thread::sleep(next_tick.saturating_duration_since(Instant::now()).min(Duration::from_millis(2)));
        }
        self.save();
    }

    fn accept_connections(&mut self) {
        while let Ok(connection) = self.incoming.try_recv() {
            let id = self.next_player;
            self.next_player += 1;
            self.players.insert(id, Player::new(id, connection, self.spawn, self.config.view_distance));
        }
    }

    fn receive_generated(&mut self) {
        while let Ok((pos, loaded)) = self.generated_rx.try_recv() {
            self.generating.remove(&pos);
            // A command may have loaded (and edited) the column in the meantime.
            if !self.world.contains(pos) {
                self.insert_column(pos, loaded);
            }
        }
    }

    /// Adds a loaded column to the world, and lets fluid waiting for it flow on.
    pub(crate) fn insert_column(&mut self, pos: ColumnPos, loaded: Loaded) {
        self.world.insert(pos, Arc::new(loaded.column));
        self.fluids.column_loaded(&self.content.blocks, &self.world, pos, self.tick, loaded.saved);
        self.show_grown_crops(pos);
    }

    fn handle_messages(&mut self) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            while let Some(player) = self.players.get(&id) {
                match player.connection.try_recv_at() {
                    Ok(Some((at, message))) => self.handle_message(id, message, at),
                    Ok(None) => break,
                    Err(_) => {
                        tracing::info!("{} left", player.name);
                        self.save_player(player);
                        self.players.remove(&id);
                    }
                }
            }
        }
    }

    fn tick(&mut self) {
        let _span = tracing::info_span!("server_tick").entered();
        let started = Instant::now();
        self.tick += 1;
        // At normal speed a day takes the content's `day_minutes`.
        self.day += (self.day_speed * self.content.commands.day_pace()) as f64;
        self.run_processes();
        self.bury_the_dead();
        self.regrow();
        self.look_around();
        self.check_quests();
        self.tick_items();
        self.tick_creatures();
        self.flow();
        self.replicate_owned_traits();
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for &id in &ids {
            self.stream_columns(id);
        }
        self.unload_columns();
        self.broadcast_players();
        if self.tick.is_multiple_of(TICKS_PER_SECOND as u64) {
            self.broadcast(self.time_message());
        }
        if self.tick.is_multiple_of(SAVE_INTERVAL) {
            self.save();
        }
        self.tick_seconds = started.elapsed().as_secs_f64();
    }

    /// Writes edited columns, joined players and the world's metadata. Failures are
    /// logged; columns that failed stay dirty and are tried again at the next save.
    fn save(&mut self) {
        if self.save.is_none() {
            return;
        }
        self.note_all_deaths();
        let Some(save) = &self.save else { return };
        let _span = tracing::info_span!("save_world").entered();
        let started = Instant::now();
        let (world, registry) = (&self.world, &self.content.blocks);
        let columns = self.dirty.len();
        self.dirty.retain(|&pos| {
            let Some(column) = world.column(pos) else { return false };
            let result = save.write_column(pos, column, registry);
            if let Err(e) = &result {
                tracing::error!("could not save column {pos:?}: {e}");
            }
            result.is_err()
        });
        for player in self.players.values().filter(|p| p.joined) {
            self.save_player(player);
        }
        let blocks = &self.content.blocks;
        if let Err(e) = save.write_items(&self.items.saved(|item| blocks.def(item).name.clone())) {
            tracing::error!("could not save dropped items: {e}");
        }
        if let Err(e) = save.write_creatures(&self.saved_creatures()) {
            tracing::error!("could not save creatures: {e}");
        }
        if let Err(e) = save.write_villagers(&self.villagers.saved()) {
            tracing::error!("could not save who of the villagers died: {e}");
        }
        let crops = &self.content.crops;
        if let Err(e) = save.write_crops(&self.farm.saved(|id| crops.get(id).def.name.clone())) {
            tracing::error!("could not save crops: {e}");
        }
        self.meta.time = self.tick;
        self.meta.placed = self.placed.iter().map(|p| p.to_array()).collect();
        self.meta.regrowing = self.saved_regrowing();
        self.meta.day = Some(self.day as u64);
        self.meta.day_speed = self.day_speed;
        self.meta.last_played = save::unix_time();
        if let Err(e) = save.write_meta(&self.meta) {
            tracing::error!("could not save {}: {e}", save.dir().display());
        }
        tracing::debug!("saved {columns} columns and {} players in {:?}", self.players.len(), started.elapsed());
    }

    fn save_player(&self, player: &Player) {
        if let Some(save) = &self.save
            && player.joined
            && let Err(e) = save.write_player(&player.name, &player.saved(&self.content.skills, &self.content.effects), &self.content.blocks)
        {
            tracing::error!("could not save player {}: {e}", player.name);
        }
    }

    /// The clocks, as players are told them.
    pub(crate) fn time_message(&self) -> ServerMessage {
        ServerMessage::Time { tick: self.tick, day: self.day as u64, day_speed: self.day_speed * self.content.commands.day_pace() }
    }

    pub(crate) fn broadcast(&self, message: ServerMessage) {
        for player in self.players.values().filter(|p| p.joined) {
            player.connection.send(message.clone());
        }
    }

    fn broadcast_players(&self) {
        if self.players.len() < 2 {
            return;
        }
        let views: Vec<_> = self.players.values().filter(|p| p.joined).map(Player::view).collect();
        for player in self.players.values().filter(|p| p.joined) {
            let others = views.iter().filter(|v| v.id != player.id).cloned().collect();
            player.connection.send(ServerMessage::Players { players: others });
        }
    }

    /// Sends loaded columns nearest-first and requests generation of missing ones.
    fn stream_columns(&mut self, id: PlayerId) {
        let Some(player) = self.players.get_mut(&id) else { return };
        if !player.joined {
            return;
        }
        let radius = player.view_distance;
        let center = ColumnPos::of_block(player.position.floor().as_ivec3());
        let mut sent = 0;
        for ring in 0..=radius {
            for pos in ring_positions(center, ring) {
                if player.sent.contains(&pos) {
                    continue;
                }
                if let Some(column) = self.world.column(pos) {
                    if sent < COLUMNS_PER_TICK {
                        player.connection.send(ServerMessage::Column { pos, column: column.clone() });
                        player.sent.insert(pos);
                        sent += 1;
                    }
                } else if self.generating.len() < MAX_GENERATING && self.generating.insert(pos) {
                    let (generator, save, registry) = (self.generator.clone(), self.save.clone(), self.content.blocks.clone());
                    let (wear, today) = (self.wear_rules.clone(), wear::day(self.day));
                    let tx = self.generated_tx.clone();
                    rayon::spawn(move || {
                        let _ = tx.send((pos, load_column(save.as_ref(), &*generator, &registry, &wear, today, pos)));
                    });
                }
            }
        }
        player.sent.retain(|&pos| {
            let keep = pos.distance(center) <= radius + 1;
            if !keep {
                player.connection.send(ServerMessage::Unload { pos });
            }
            keep
        });
    }

    fn unload_columns(&mut self) {
        if !self.tick.is_multiple_of(TICKS_PER_SECOND as u64 * 5) {
            return;
        }
        let keep: Vec<(ColumnPos, i32)> = self
            .players
            .values()
            .map(|p| (ColumnPos::of_block(p.position.floor().as_ivec3()), p.view_distance + 2))
            .collect();
        let unload: Vec<ColumnPos> = self
            .world
            .columns()
            .map(|(&pos, _)| pos)
            .filter(|pos| !self.dirty.contains(pos) && keep.iter().all(|(c, radius)| c.distance(*pos) > *radius))
            .collect();
        for pos in unload {
            self.world.remove(pos);
            self.fluids.column_unloaded(pos);
        }
    }

    pub(crate) fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.world.block(pos)
    }

    pub(crate) fn is_solid(&self, pos: IVec3) -> bool {
        self.block(pos).is_some_and(|b| self.content.blocks.info(b).solid)
    }

    /// Changes many blocks at once (a command's), telling each player about those in columns
    /// they have in one message, and waking any fluid they let flow.
    pub(crate) fn set_blocks(&mut self, changes: &[(IVec3, BlockId)]) {
        let mut done = Vec::with_capacity(changes.len());
        for &(pos, block) in changes {
            if self.world.set_block(pos, block).is_some() {
                self.dirty.insert(ColumnPos::of_block(pos));
                done.push((pos, block));
            }
        }
        for &(pos, _) in &done {
            self.fluids.changed(&self.content.blocks, &self.world, pos, self.tick);
        }
        self.send_changes(done);
    }

    /// Sends each player the changes in columns it has.
    pub(crate) fn send_changes(&self, changes: Vec<(IVec3, BlockId)>) {
        for player in self.players.values().filter(|p| p.joined) {
            let seen: Vec<_> = changes.iter().copied().filter(|(pos, _)| player.sent.contains(&ColumnPos::of_block(*pos))).collect();
            if !seen.is_empty() {
                player.connection.send(ServerMessage::Blocks { changes: seen });
            }
        }
    }

    /// Changes a block, telling only the players who have its column (the others get the
    /// change with the column), and wakes any fluid it lets flow.
    pub(crate) fn set_block(&mut self, pos: IVec3, block: BlockId) {
        let column = ColumnPos::of_block(pos);
        if self.world.set_block(pos, block).is_some() {
            self.dirty.insert(column);
            for player in self.players.values().filter(|p| p.joined && p.sent.contains(&column)) {
                player.connection.send(ServerMessage::BlockChanged { pos, block });
            }
            self.fluids.changed(&self.content.blocks, &self.world, pos, self.tick);
        }
    }
}

/// The spot nearest `start` within its column whose top block is opaque ground with two
/// free blocks above it (so the player never spawns in a tree).
fn find_ground(world: &VoxelWorld, registry: &BlockRegistry, start: IVec3) -> Vec3 {
    let origin = ColumnPos::of_block(start).origin();
    let mut candidates: Vec<IVec3> =
        (0..CHUNK_SIZE).flat_map(|z| (0..CHUNK_SIZE).map(move |x| origin + IVec3::new(x, 0, z))).collect();
    candidates.sort_by_key(|p| (p.x - start.x).abs() + (p.z - start.z).abs());
    let info = |p: IVec3| world.block(p).map(|b| registry.info(b));
    for column in candidates {
        let Some(ground) = (0..WORLD_HEIGHT).rev().map(|y| column.with_y(y)).find(|&p| info(p).is_some_and(|i| i.solid))
        else {
            continue;
        };
        let open = |p: IVec3| info(p).is_some_and(|i| !i.solid);
        if info(ground).is_some_and(|i| i.render == RenderKind::Opaque) && open(ground + IVec3::Y) && open(ground + IVec3::Y * 2) {
            return (ground + IVec3::Y).as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        }
    }
    start.as_vec3() + Vec3::new(0.5, 0.0, 0.5)
}

/// Column positions at exactly Chebyshev distance `ring` from `center`.
fn ring_positions(center: ColumnPos, ring: i32) -> impl Iterator<Item = ColumnPos> {
    let side = (2 * ring).max(1);
    (0..if ring == 0 { 1 } else { 4 * side }).map(move |i| {
        if ring == 0 {
            return center;
        }
        let (edge, t) = (i / side, i % side - ring);
        match edge {
            0 => center.offset(t, -ring),
            1 => center.offset(ring, t),
            2 => center.offset(-t, ring),
            _ => center.offset(-ring, -t),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_cover_square_once() {
        let center = ColumnPos::new(3, -2);
        let mut seen = FxHashSet::default();
        for ring in 0..=4 {
            for pos in ring_positions(center, ring) {
                assert_eq!(pos.distance(center), ring);
                assert!(seen.insert(pos));
            }
        }
        assert_eq!(seen.len(), 81);
    }
}
