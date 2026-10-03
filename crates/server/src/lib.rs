//! The authoritative game server. Runs headless as `voxmar-server`, or on a thread inside
//! the client for singleplayer.

mod actions;
mod player;
mod process;
pub mod save;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::content::Content;
use voxmar_engine::field::Field;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, WORLD_HEIGHT};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::{Column, VoxelWorld};
use voxmar_protocol::{PlayerId, ServerConnection, ServerMessage, TICKS_PER_SECOND};
use voxmar_worldgen::{Generator, Overworld};

use crate::actions::Executor;
use crate::player::Player;
use crate::process::Process;
use crate::save::{GameMode, WorldMeta, WorldSave};

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
    /// saved world's game mode replaces `allow_flight` and `local_admin`. `None` keeps the
    /// world in memory only.
    pub world: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { seed: 0x5eed, view_distance: 32, allow_flight: true, local_admin: true, admins: Vec::new(), world: None }
    }
}

impl GameMode {
    /// Creative allows flight and makes the singleplayer player an admin; survival does
    /// neither.
    fn apply(self, config: &mut ServerConfig) {
        let creative = self == GameMode::Creative;
        config.allow_flight = creative;
        config.local_admin = creative;
    }
}

/// How often edited columns, players and the time of day are saved.
const SAVE_INTERVAL: u64 = 30 * TICKS_PER_SECOND as u64;
/// Time of day a new world starts at: early morning.
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

/// Loads every content package under `assets/content`.
pub fn load_content(assets: &Path) -> Result<Content, Box<dyn std::error::Error>> {
    Ok(Content::load(&assets.join("content"))?)
}

/// A running server thread. Dropping it stops the server.
pub struct ServerHandle {
    connections: Sender<ServerConnection>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Sender for new connections (local or accepted by a TCP listener).
    pub fn connections(&self) -> Sender<ServerConnection> {
        self.connections.clone()
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
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("server".into())
            .spawn(move || Server::new(config, meta, save, content, incoming).run(&stop))?
    };
    Ok(ServerHandle { connections, stop, thread: Some(thread) })
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

/// A saved column if there is one, otherwise a freshly generated one. Runs on workers.
fn load_column(save: Option<&WorldSave>, generator: &dyn Generator, registry: &BlockRegistry, pos: ColumnPos) -> Column {
    if let Some(save) = save {
        match save.read_column(pos, registry) {
            Ok(Some(column)) => return column,
            Ok(None) => {}
            Err(e) => tracing::error!("could not load column {pos:?}, generating it again: {e}"),
        }
    }
    generator.generate(pos)
}

pub(crate) struct Server {
    config: ServerConfig,
    meta: WorldMeta,
    save: Option<WorldSave>,
    content: Content,
    generator: Arc<dyn Generator>,
    executor: Executor,
    processes: Vec<Box<dyn Process>>,
    /// `core:temperature`, mean air temperature in °C, baseline from the climate generator.
    temperature: Field,
    world: VoxelWorld,
    /// Columns edited since they were last saved. They stay loaded until saved; without a
    /// save, that is forever.
    dirty: FxHashSet<ColumnPos>,
    generating: FxHashSet<ColumnPos>,
    generated_tx: Sender<(ColumnPos, Column)>,
    generated_rx: Receiver<(ColumnPos, Column)>,
    incoming: Receiver<ServerConnection>,
    players: FxHashMap<PlayerId, Player>,
    next_player: PlayerId,
    spawn: Vec3,
    tick: u64,
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
    ) -> Self {
        let overworld = Arc::new(Overworld::new(meta.seed, content.blocks.clone()));
        let climate = overworld.clone();
        let temperature = Field::new("core:temperature", move |x, z| climate.site(x, z).temperature);
        let generator: Arc<dyn Generator> = overworld;
        // Load the spawn column up front and stand on its highest solid block, so
        // features like trees never bury the player.
        let mut world = VoxelWorld::default();
        let spawn_block = generator.spawn_point();
        let spawn_column = ColumnPos::of_block(spawn_block);
        world.insert(spawn_column, Arc::new(load_column(save.as_ref(), &*generator, &content.blocks, spawn_column)));
        let spawn = find_ground(&world, &content.blocks, spawn_block);
        let (generated_tx, generated_rx) = unbounded();
        tracing::info!("world seed {:#x}, spawn at {spawn}", meta.seed);
        Self {
            config,
            tick: meta.time.max(FIRST_TICK),
            meta,
            save,
            content,
            generator,
            executor: Executor::default(),
            processes: process::core_processes(),
            temperature,
            world,
            dirty: FxHashSet::default(),
            generating: FxHashSet::default(),
            generated_tx,
            generated_rx,
            incoming,
            players: FxHashMap::default(),
            next_player: 1,
            spawn,
        }
    }

    /// Runs fixed-rate ticks until `stop` is set, then saves. Between ticks, messages are
    /// handled as soon as they arrive so actions feel immediate.
    fn run(mut self, stop: &AtomicBool) {
        let tick_length = Duration::from_secs(1) / TICKS_PER_SECOND;
        let mut next_tick = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            self.accept_connections();
            self.receive_generated();
            self.handle_messages();
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
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
        while let Ok((pos, column)) = self.generated_rx.try_recv() {
            self.generating.remove(&pos);
            self.world.insert(pos, Arc::new(column));
        }
    }

    fn handle_messages(&mut self) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            while let Some(player) = self.players.get(&id) {
                match player.connection.try_recv() {
                    Ok(Some(message)) => self.handle_message(id, message),
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
        self.tick += 1;
        self.complete_actions();
        self.run_processes();
        self.replicate_owned_traits();
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for &id in &ids {
            self.stream_columns(id);
        }
        self.unload_columns();
        self.broadcast_players();
        if self.tick.is_multiple_of(TICKS_PER_SECOND as u64) {
            self.broadcast(ServerMessage::Time { ticks: self.tick });
        }
        if self.tick.is_multiple_of(SAVE_INTERVAL) {
            self.save();
        }
    }

    /// Writes edited columns, joined players and the world's metadata. Failures are
    /// logged; columns that failed stay dirty and are tried again at the next save.
    fn save(&mut self) {
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
        self.meta.time = self.tick;
        self.meta.last_played = save::unix_time();
        if let Err(e) = save.write_meta(&self.meta) {
            tracing::error!("could not save {}: {e}", save.dir().display());
        }
        tracing::debug!("saved {columns} columns and {} players in {:?}", self.players.len(), started.elapsed());
    }

    fn save_player(&self, player: &Player) {
        if let Some(save) = &self.save
            && player.joined
            && let Err(e) = save.write_player(&player.name, &player.saved(), &self.content.blocks)
        {
            tracing::error!("could not save player {}: {e}", player.name);
        }
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
                    let tx = self.generated_tx.clone();
                    rayon::spawn(move || {
                        let _ = tx.send((pos, load_column(save.as_ref(), &*generator, &registry, pos)));
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
        }
    }

    pub(crate) fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.world.block(pos)
    }

    pub(crate) fn is_solid(&self, pos: IVec3) -> bool {
        self.block(pos).is_some_and(|b| self.content.blocks.info(b).solid)
    }

    pub(crate) fn set_block(&mut self, pos: IVec3, block: BlockId) {
        if self.world.set_block(pos, block).is_some() {
            self.dirty.insert(ColumnPos::of_block(pos));
            self.broadcast(ServerMessage::BlockChanged { pos, block });
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
