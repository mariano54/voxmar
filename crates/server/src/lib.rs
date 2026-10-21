//! The authoritative game server. Runs headless as `voxmar-server`, or on a thread inside
//! the client for singleplayer.

mod actions;
mod apiary;
mod barks;
mod commands;
mod condition;
/// The weather in words, for the debug overlay as for `/weather`.
pub use barks::BARK_COMMAND_REACH;
pub use commands::{weather_name, weather_numbers};
pub use creatures::NEAR_THINK_LONGEST;
mod cooking;
mod creatures;
mod fight;
mod fishing;
mod flames;
mod death;
#[cfg(test)]
mod doors;
#[cfg(test)]
mod seats;
mod hearth;
mod journal;
mod lairs;
mod machines;
mod quests;
mod blaze;
mod coat;
mod regrow;
mod farm;
mod husbandry;
mod inspect;
mod fluids;
mod items;
mod player;
mod prayers;
mod process;
mod rules;
mod laying;
mod smoulder;
mod stations;
mod traps;
mod stores;
mod treasure;
mod troughs;
pub mod save;
mod spawn_kit;
mod villagers;
mod wandering;
mod weather;
mod tool_wear;
mod wear;

use std::collections::VecDeque;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind};
use voxmar_engine::content::Content;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, WORLD_HEIGHT};
use voxmar_engine::field::Field;
use voxmar_engine::fluid::Fluids;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::world::{Column, VoxelWorld};
use voxmar_protocol::{KEEPALIVE, PlayerId, ServerConnection, ServerMessage, TICKS_PER_SECOND};
use voxmar_worldgen::{ContentError, Generator, Overworld, WorldContent};

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
    /// Most players playing at once; more are turned away when they say hello.
    pub max_players: usize,
    /// A password that makes whoever joins with it an admin, from anywhere (the dedicated
    /// server reads it from `$VOXMAR_ADMIN_TOKEN` or `--admin-token-file`; a client sends the
    /// one `VOXMAR_ADMIN_TOKENS` keeps for the server's address). `None` for none.
    pub admin_token: Option<String>,
    /// How long a player connected from elsewhere may send nothing before they are let go:
    /// a connection that died without closing would otherwise keep their name taken. A
    /// client's network thread sends a keepalive every `transport::KEEPALIVE` whatever the
    /// game is doing, so a minimized or busy client is never let go.
    pub idle_timeout: Duration,
    /// Directory the world is saved in; a new world is created there if it holds none. A
    /// saved world's game mode replaces `allow_flight`. `None` keeps the world in memory
    /// only.
    pub world: Option<PathBuf>,
    /// How much any one connection may send and how many may wait to say hello (the
    /// dedicated server reads them from `--limits FILE`).
    pub limits: Limits,
    /// Makes the server run behind, to see how clients cope, as on a busy machine. None
    /// but in tests.
    pub lag: Lag,
    /// Cuts every save short once its machines are written, as a crash would. Only in
    /// tests.
    pub cut_saves: bool,
    /// How many times faster than real time the server's ticks run: 1 for a game, more
    /// only in tests, whose waits are judged by ticks (`ServerHandle::ticks`) and not by
    /// the clock. What the server judges by arrival stamps (a break, a blow) keeps the
    /// clock's pace, since players' messages carry theirs.
    pub tick_speedup: u32,
    /// The key a new world's draws hash (`WorldMeta::draw_key`), else a random one. The gate
    /// sets one, as it sets the seed, so its world draws the same every run.
    pub draw_key: Option<u64>,
    /// Whether the server ticks only as far as its test's player allows
    /// (`ServerHandle::allow_tick`): one tick past the last the player has taken in, so a
    /// player starved of the cores on a busy machine is never left behind by a quick
    /// server, and every tick the player waits on is one it saw. Off but in tests; off,
    /// the server ticks at its pace alone.
    pub lockstep: bool,
    /// Whether wandering events start by chance near players (`try_wandering`). Off only
    /// in tests, whose quick ticks would otherwise bring a party beside the player some
    /// game days in, when a test slowed by load is still running; `/wander start` and
    /// the wandering tests start theirs by hand either way.
    pub wandering_starts: bool,
    /// The chance a gene mutates in a birth, in place of `genes::MUTATION` (aether still
    /// raises it). `None` but in tests, which set 0 where a birth must take only its
    /// parents' genes.
    pub mutation: Option<f32>,
}

/// How much any one connection may ask of the server: a server operator's numbers, read
/// by the dedicated server from a RON file (`--limits FILE`) that sets any of them.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Messages a player's handled per tick, at most; the rest wait for the next (and a
    /// connection whose queue fills is cut off). Inputs come one a tick.
    pub messages_per_tick: u32,
    /// Lines of chat in a burst, and per second after it.
    pub chat_burst: f32,
    pub chat_per_second: f32,
    /// Seconds a connection may take to say hello before it is closed, how many may be
    /// waiting to at once, and how many of those from one address.
    pub hello_seconds: f32,
    pub max_pending: usize,
    pub max_pending_per_address: usize,
    /// Characters of a player's name kept.
    pub name_length: usize,
    /// How many leading bits of an IPv6 address the limits per address count by: one
    /// host usually holds a whole /64, so each of its addresses is not another player.
    pub ipv6_prefix: u8,
    /// Seconds a joined player's connection must have been silent (not even a keepalive)
    /// before the same name from the same address replaces it: a game that froze or lost
    /// its network. Over the client's keepalive, so a playing session is never ended so.
    pub rejoin_silence: f32,
    /// Blocks from a player within which they are told of others (`ServerMessage::Players`).
    pub players_seen: f32,
    /// Blocks past `players_seen` a player already told of must go before they are told
    /// gone, so one hovering at the edge is not told of and gone again on alternate ticks.
    pub players_seen_margin: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { messages_per_tick: 64, chat_burst: 8.0, chat_per_second: 2.0, hello_seconds: 3.0, max_pending: 16, max_pending_per_address: 4, name_length: 32, ipv6_prefix: 64, rejoin_silence: KEEPALIVE.as_secs_f32() + 1.0, players_seen: 160.0, players_seen_margin: 8.0 }
    }
}

impl Limits {
    /// Limits from a RON file's text, refusing any that would shut everyone out or let
    /// one connection hold the server up.
    pub fn from_ron(source: &str) -> Result<Self, String> {
        let limits: Limits = ron::from_str(source).map_err(|e| e.to_string())?;
        let Limits { messages_per_tick, chat_burst, chat_per_second, hello_seconds, max_pending, max_pending_per_address, name_length, ipv6_prefix, rejoin_silence, players_seen, players_seen_margin } = limits;
        let rates = [chat_burst, chat_per_second, hello_seconds];
        if rates.iter().any(|n| !n.is_finite() || *n < 0.0) {
            Err("chat_burst, chat_per_second and hello_seconds must be zero or more".into())
        } else if !(2..=1024).contains(&messages_per_tick) {
            Err("messages_per_tick must be from 2 to 1024 (a player sends an input and more every tick)".into())
        } else if !(0.5..=60.0).contains(&hello_seconds) {
            Err("hello_seconds must be from 0.5 to 60".into())
        } else if max_pending == 0 || max_pending_per_address == 0 {
            Err("max_pending and max_pending_per_address must be at least 1, or nobody could join".into())
        } else if !(1..=64).contains(&name_length) {
            Err("name_length must be from 1 to 64".into())
        } else if !(16..=128).contains(&ipv6_prefix) {
            Err("ipv6_prefix must be from 16 to 128".into())
        } else if !(rejoin_silence > KEEPALIVE.as_secs_f32() && rejoin_silence <= 3600.0) {
            Err(format!("rejoin_silence must be over the client's keepalive ({} s), or a playing session could be ended, and at most 3600", KEEPALIVE.as_secs()))
        } else if !(8.0..=2048.0).contains(&players_seen) {
            Err("players_seen must be from 8 to 2048 blocks".into())
        } else if !(0.0..=64.0).contains(&players_seen_margin) {
            Err("players_seen_margin must be from 0 to 64 blocks".into())
        } else {
            Ok(limits)
        }
    }

    /// The address the limits per address count `ip` under: an IPv4 address (an IPv4
    /// client of a dual-stack listener too) as itself, an IPv6 one by its first
    /// `ipv6_prefix` bits.
    pub fn address_key(&self, ip: IpAddr) -> IpAddr {
        match ip.to_canonical() {
            IpAddr::V6(v6) => {
                let mask = u128::MAX.checked_shl(128 - self.ipv6_prefix.min(128) as u32).unwrap_or(0);
                IpAddr::V6((u128::from(v6) & mask).into())
            }
            v4 => v4,
        }
    }
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
            max_players: 64,
            admin_token: None,
            idle_timeout: Duration::from_secs(15),
            world: None,
            limits: Limits::default(),
            lag: Lag::default(),
            cut_saves: false,
            tick_speedup: 1,
            draw_key: None,
            lockstep: false,
            wandering_starts: true,
            mutation: None,
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

/// Actions the client and server name in code: content that leaves one out fails at
/// startup rather than in play.
const NAMED_ACTIONS: &[&str] =
    &["core:mine", "core:place", "core:transfer", "core:quick_move", "core:craft", "core:stow", "core:drop", "core:attack", "core:eat", "core:respawn"];

/// Healing by itself, however many buffs and prayers are on, is held under `heal_most` a
/// second (`survival.ron`), which must be below what the slowest food heals a second eaten
/// bite after bite (`core:eat`'s time): food stays the way to heal in a hurry.
fn heal_most_below_food(content: &Content, heal_most: f32) -> Result<(), String> {
    let eat = content.actions.get(content.actions.expect("core:eat")).expect("named");
    let voxmar_engine::action::Duration::Seconds(bite) = eat.duration else {
        return Err("core:eat must take a time (Seconds), the bite that heal_most is held under".into());
    };
    let slowest = content.blocks.ids().map(|id| voxmar_engine::vitals::food_value(&content.blocks.def(id).tags)).filter(|&heal| heal > 0).min();
    match slowest.map(|heal| heal as f32 / bite.max(f32::EPSILON)) {
        Some(per_second) if heal_most >= per_second => Err(format!("survival.ron's heal_most ({heal_most} a second) must be below what the slowest food heals a second, a bite each {bite} s ({per_second:.2})")),
        _ => Ok(()),
    }
}

/// Loads every content package under `assets/content`, and checks the world
/// generator's definitions in them, so a broken package fails at startup.
/// How each block (by id) shelters as a wall of a home's room, as `Server::room_walls`
/// judges it (survival.ron's furniture looked past), for the client to hear a home's walls
/// as the server judges them.
pub fn wall_shelters(content: &Content) -> Vec<Option<voxmar_engine::effect::Shelter>> {
    let Ok(rules) = rules::SurvivalRules::load(&content.files, &content.blocks, &mut Vec::new()) else { return vec![None; content.blocks.len()] };
    (0..content.blocks.len())
        .map(|id| {
            let id = voxmar_engine::block::BlockId(id as u16);
            let tags = &content.blocks.def(id).tags;
            voxmar_engine::effect::Shelter::of_wall(tags, content.blocks.info(id).solid, rules.furniture(tags))
        })
        .collect()
}

pub fn load_content(assets: &Path) -> Result<Content, Box<dyn std::error::Error>> {
    let mut content = Content::load(&assets.join("content"))?;
    if let Some(missing) = NAMED_ACTIONS.iter().find(|name| content.actions.id(name).is_none()) {
        return Err(format!("no package defines the action {missing}, which the game needs").into());
    }
    let world = WorldContent::load(&content)?;
    sources_known(&content, &world)?;
    wear::WearRules::load(&content.files, &content.blocks, 0)?;
    spawn_kit::SpawnKit::load(&content.files, &content.blocks)?;
    let mut notes = Vec::new();
    let survival = rules::SurvivalRules::load(&content.files, &content.blocks, &mut notes)?;
    heal_most_below_food(&content, survival.heal_most)?;
    survival.check_shelters(&content.blocks)?;
    std::sync::Arc::make_mut(&mut content.notes).extend(notes);
    for fluid in content.blocks.fluids().iter().filter(|fluid| !swum_or_burns(&content.blocks, fluid)) {
        tracing::warn!("the fluid {} is neither swum in (swim: true) nor burns (tagged \"lava\"): players wade through it unharmed", fluid.name);
    }
    Ok(content)
}

/// The recipe book: every way the content gives to get each item, what the world lays
/// among them (found, broken loose).
pub fn recipe_book(content: &Content) -> Result<voxmar_engine::recipe_book::RecipeBook, ContentError> {
    Ok(recipe_book_of(content, &WorldContent::load(content)?))
}

/// As `recipe_book`, with `world` the world content `content` loads.
fn recipe_book_of(content: &Content, world: &WorldContent) -> voxmar_engine::recipe_book::RecipeBook {
    let mut book = voxmar_engine::recipe_book::RecipeBook::new(content);
    book.found(content, world.placed(content.files.iter(), &content.blocks));
    book
}

/// Every item a player could hold has a way to get it in the recipe book, or is tagged
/// `creative_only`: a package that adds an item nothing gives fails here, naming it and the
/// package, rather than shipping a dead entry. A tag on an item that has a way is noted.
fn sources_known(content: &Content, world: &WorldContent) -> Result<(), String> {
    let (dead, stale) = recipe_book_of(content, world).unsourced(&content.blocks);
    let named = |ids: &[voxmar_engine::block::BlockId]| {
        ids.iter()
            .map(|&id| {
                let name = &content.blocks.def(id).name;
                format!("`{name}` (package {})", name.split(':').next().unwrap_or(name))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    for &id in &stale {
        tracing::info!("{} is tagged {} but the recipe book has a way to it", content.blocks.def(id).name, voxmar_engine::recipe_book::CREATIVE_ONLY);
    }
    if dead.is_empty() {
        return Ok(());
    }
    Err(format!(
        "no way in survival gives {}: give each a recipe, a drop or a place in the world, or tag it \"{}\"",
        named(&dead),
        voxmar_engine::recipe_book::CREATIVE_ONLY
    ))
}

/// Whether players swim in `fluid` or are burned by it: one that does neither (a mod's
/// lava that lost its tag, say) is only waded through.
fn swum_or_burns(blocks: &voxmar_engine::block::BlockRegistry, fluid: &voxmar_engine::fluid::Fluid) -> bool {
    fluid.def.swim || blocks.def(fluid.source).has_tag("lava")
}

/// What tells players apart (for who keeps an animal or owns a wheel, and name comparisons):
/// names that differ only in case are one player (`player::same_name`).
pub(crate) fn player_key(name: &str) -> String {
    name.to_lowercase()
}

/// A running server thread. Dropping it stops the server.
pub struct ServerHandle {
    connections: Sender<ServerConnection>,
    console: Sender<String>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    tick_times: Arc<Mutex<VecDeque<Duration>>>,
    ticks: Arc<AtomicU64>,
    lagging: Arc<AtomicBool>,
    hurried: Arc<AtomicU32>,
    /// The tick the server may tick up to, in lockstep (`ServerConfig::lockstep`).
    leash: Option<Arc<Leash>>,
}

/// A server in lockstep with its test's player (`ServerConfig::lockstep`): the ticks it may
/// make, and the ticks made, which the player waits on.
#[derive(Default)]
struct Leash {
    allowed: AtomicU64,
    made: Mutex<u64>,
    ticked: std::sync::Condvar,
}

#[cfg(target_os = "linux")]
fn thread_cpu_time(thread: &JoinHandle<()>) -> Option<Duration> {
    use std::os::unix::thread::JoinHandleExt;
    let mut clock: libc::clockid_t = 0;
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: the thread is not joined (the handle holds it), so its pthread_t is valid; the
    // calls write only the clock id and the time they are given.
    unsafe {
        if libc::pthread_getcpuclockid(thread.as_pthread_t(), &mut clock) != 0 || libc::clock_gettime(clock, &mut time) != 0 {
            return None;
        }
    }
    Some(Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}

#[cfg(target_os = "macos")]
fn thread_cpu_time(thread: &JoinHandle<()>) -> Option<Duration> {
    use std::os::unix::thread::JoinHandleExt;
    let mut info = std::mem::MaybeUninit::<libc::thread_basic_info>::zeroed();
    let mut count = libc::THREAD_BASIC_INFO_COUNT;
    // SAFETY: the thread is not joined, so its pthread_t is valid; thread_info writes at
    // most `count` integers into the buffer, read only when it says it succeeded.
    let info = unsafe {
        let port = libc::pthread_mach_thread_np(thread.as_pthread_t());
        if libc::thread_info(port, libc::THREAD_BASIC_INFO as libc::thread_flavor_t, info.as_mut_ptr().cast(), &mut count) != libc::KERN_SUCCESS {
            return None;
        }
        info.assume_init()
    };
    let time = |t: libc::time_value_t| Duration::new(t.seconds as u64, t.microseconds as u32 * 1000);
    Some(time(info.user_time) + time(info.system_time))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn thread_cpu_time(_: &JoinHandle<()>) -> Option<Duration> {
    None
}

/// Ticks whose times a server keeps, for `ServerHandle::tick_times`: the last minute.
const TICK_TIMES: usize = 60 * TICKS_PER_SECOND as usize;

impl ServerHandle {
    /// How long the server's last ticks took to simulate, oldest first (up to a minute's
    /// worth), for benchmarks and diagnostics.
    pub fn tick_times(&self) -> Vec<Duration> {
        self.tick_times.lock().map(|t| t.iter().copied().collect()).unwrap_or_default()
    }

    /// How much CPU time the server's thread has had: work it has done, whether or not it has
    /// a tick or a message to show for it yet (opening the world, generating the spawn
    /// column). A test's wait on a silent server tells by it a server at work on a busy
    /// machine from a stuck one. `None` once the thread has ended, or where the system does
    /// not say.
    pub fn cpu_time(&self) -> Option<Duration> {
        let thread = self.thread.as_ref().filter(|t| !t.is_finished())?;
        thread_cpu_time(thread)
    }

    /// How many ticks the server has simulated: its own clock, which a server running behind
    /// on a busy machine keeps, where wall-clock seconds do not.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Acquire)
    }

    /// Whether a server made to run behind (`ServerConfig::lag`) does so from now on: it
    /// does from the start. The gate's session holds it off in the steps that do not play
    /// against a server running behind.
    pub fn set_lagging(&self, lagging: bool) {
        self.lagging.store(lagging, Ordering::Relaxed);
    }

    /// Ticks this many times a second's worth each second from now on (1: the game's pace),
    /// as `ServerConfig::tick_speedup` does from the start: the gate's session hurries the
    /// machine steps, whose bloom and dust take their recipes' full server time.
    pub fn set_hurried(&self, times: u32) {
        self.hurried.store(times.max(1), Ordering::Relaxed);
    }

    /// In lockstep (`ServerConfig::lockstep`), lets the server tick until it has made
    /// `ticks` ticks: a player who has taken in every message of tick `n` allows `n + 1`.
    /// Whatever the player sent before allowing it is handled before that tick. Nothing,
    /// out of lockstep.
    pub fn allow_tick(&self, ticks: u64) {
        if let Some(leash) = &self.leash {
            leash.allowed.fetch_max(ticks, Ordering::Release);
            if let Some(thread) = &self.thread {
                thread.thread().unpark();
            }
        }
    }

    /// In lockstep, waits up to `wait` for the server to have made more than `ticks` ticks.
    /// Out of lockstep, returns at once.
    pub fn wait_past(&self, ticks: u64, wait: Duration) {
        if let Some(leash) = &self.leash {
            let made = leash.made.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = leash.ticked.wait_timeout_while(made, wait, |made| *made <= ticks);
        }
    }

    /// Whether the server ticks in lockstep with its test's player (`ServerConfig::lockstep`).
    pub fn lockstep(&self) -> bool {
        self.leash.is_some()
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
    let (save, mut meta) = open_world(&mut config)?;
    // Land made from different worldgen data than before may not line up with it: warned
    // about, never refused (content-only changes keep the generator's version).
    let worldgen_content = WorldContent::fingerprint(content.files.iter(), &content.blocks);
    let generation_changed = meta.worldgen_content.is_some_and(|before| before != worldgen_content);
    if meta.worldgen_content != Some(worldgen_content) {
        meta.worldgen_content = Some(worldgen_content);
        if let Some(save) = &save {
            if generation_changed {
                tracing::warn!("{}: {}", save.dir().display(), content.english.render(&voxmar_engine::lang::Words::key("world.generation_changed"), &content.blocks));
            }
            save.write_meta(&meta)?;
        }
    }
    let state = save.as_ref().map_or(Ok(save::WorldState::default()), WorldSave::read_state)?;
    let (connections, incoming) = unbounded();
    let (console, lines) = unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let tick_times = Arc::new(Mutex::new(VecDeque::with_capacity(TICK_TIMES)));
    let ticks = Arc::new(AtomicU64::new(0));
    let lagging = Arc::new(AtomicBool::new(true));
    let hurried = Arc::new(AtomicU32::new(1));
    let leash = config.lockstep.then(|| Arc::new(Leash::default()));
    let thread = {
        let (stop, tick_times, ticks, lagging, hurried, leash) = (stop.clone(), tick_times.clone(), ticks.clone(), lagging.clone(), hurried.clone(), leash.clone());
        std::thread::Builder::new()
            .name("server".into())
            .spawn(move || {
                frame_class();
                let mut server = Server::new(config, meta, save, state, content, incoming, lines);
                server.generation_changed = generation_changed;
                server.lagging = lagging;
                server.hurried = hurried;
                server.leash = leash;
                server.run(&stop, &tick_times, &ticks)
            })?
    };
    Ok(ServerHandle { connections, console, stop, thread: Some(thread), tick_times, ticks, lagging, hurried, leash })
}

/// Puts the server's thread in the class of a game client's frame threads on macOS (the
/// client's `priority.rs`), said outright, since whether a new thread takes its creator's
/// class is not promised. Never below them: in a single-player game the server runs the
/// world, and at the default class, on a machine at a load of 70-90, the gate's creatures
/// stood still for seconds while the frames went on. On every platform it is marked urgent
/// for world generation, which then never has it wait on a background worker's build
/// (`voxmar_worldgen::mark_urgent`). `VOXMAR_THREAD_PRIORITY=off` leaves its class as it
/// starts, as it does the client's threads; it is urgent all the same.
fn frame_class() {
    // Never waiting on a region a streaming worker is building.
    voxmar_worldgen::mark_urgent();
    if std::env::var("VOXMAR_THREAD_PRIORITY").is_ok_and(|v| v == "off") {
        return;
    }
    // SAFETY: only changes the scheduling class of the current thread.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
    };
}

/// The world `config` asks for: the one saved in `config.world` (created if missing), or
/// an unsaved one.
fn open_world(config: &mut ServerConfig) -> std::io::Result<(Option<WorldSave>, WorldMeta)> {
    let new_mode = if config.allow_flight { GameMode::Creative } else { GameMode::Survival };
    let keyed = |mut meta: WorldMeta| {
        if let Some(key) = config.draw_key {
            meta.draw_key = key;
        }
        meta
    };
    let Some(dir) = &config.world else { return Ok((None, keyed(WorldMeta::new("world", config.seed, new_mode)))) };
    let save = WorldSave::new(dir);
    let mut meta = if save.exists() {
        let mut meta = save.read_meta()?;
        save.recover(&meta);
        // A world from before its draws had a key gets one now, kept from its next save.
        if meta.draw_key == 0 {
            meta.draw_key = config.draw_key.unwrap_or_else(save::random_key);
        }
        meta
    } else {
        let name = dir.file_name().map_or_else(|| "world".into(), |name| name.to_string_lossy().into_owned());
        let meta = keyed(WorldMeta::new(name, config.seed, new_mode));
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
    /// The column's file could not be read, nor moved aside, so edits to the column
    /// generated in its place are not saved over it (`Server::kept`).
    keep_file: bool,
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
                return Loaded { column, saved: true, keep_file: false };
            }
            Ok(None) => {}
            // Not a column file: set aside, so the column generated in its place never
            // overwrites it.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => match save.set_aside_column(pos) {
                Ok(aside) => tracing::error!("could not load column {pos:?}: {e}; kept it as {} and generated the column again", aside.display()),
                Err(moving) => {
                    tracing::error!("could not load column {pos:?}: {e}; nor move it aside ({moving}), so edits to it will not be saved");
                    return Loaded { column: generator.generate(pos), saved: false, keep_file: true };
                }
            },
            // Not read at all (no permission, a failing disk): left alone.
            Err(e) => {
                tracing::error!("could not read column {pos:?}: {e}; edits to it will not be saved");
                return Loaded { column: generator.generate(pos), saved: false, keep_file: true };
            }
        }
    }
    Loaded { column: generator.generate(pos), saved: false, keep_file: false }
}

pub(crate) struct Server {
    config: ServerConfig,
    /// Whether its players' land is missing, as it last said (taken back with the server).
    land_missing: voxmar_engine::jobs::LandMissing,
    meta: WorldMeta,
    /// The last save generation begun, whether or not it counted: none is begun twice, so
    /// files a failed save left can never pass for a later one's.
    generation: u64,
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
    /// Machines players built, and the power between them.
    factory: voxmar_engine::factory::Factory,
    /// Lit multiblocks, by core cell: a kiln burning its fuel charge, a hearth (which needs
    /// none) until it breaks.
    lit: FxHashMap<IVec3, hearth::Lit>,
    /// Structures smouldering, by core cell: a charcoal clamp charring (smoulder.rs).
    smoulders: FxHashMap<IVec3, smoulder::Smouldering>,
    /// Whose each home hearth is (a player's key, or no one's for one lit before fires were
    /// kept by owner), by core cell: set when it is first lit, kept while its fire is out or
    /// a piece of it is broken, forgotten only when its owner breaks its hearthstone or lights
    /// a home hearth elsewhere, so relighting never changes it. Its box keeps out others' fire
    /// all that while. An older world's cold home hearth has none.
    hearth_owners: FxHashMap<IVec3, Option<String>>,
    /// The burning fires players were last told of (`ServerMessage::Fires`).
    fires_sent: Vec<IVec3>,
    /// Cells whose block a player placed: ores, which then teach nothing; plants, which
    /// don't regrow by themselves; and furnishings, which give themselves back only from
    /// these cells (see `WorldMeta::placed`).
    placed: FxHashSet<IVec3>,
    /// Blocks set down that an unfinished quest's `Placed` counts, by cell: whose (their
    /// name in lower case) and what item. Each counts on their tally while the block in the
    /// cell still drops that item (`WorldMeta::quest_placed`).
    quest_placed: FxHashMap<IVec3, (String, String)>,
    /// Picked wild plants waiting to grow back (see `regrow`), and plants a herb planter
    /// grew back, whose picking pays the planter's experience.
    regrowing: FxHashMap<IVec3, regrow::Regrowing>,
    /// Lights set down and burning, by where they stand (`flames`).
    flames: FxHashMap<IVec3, flames::Flame>,
    /// Blocks waiting to catch fire or to burn out (see `blaze`).
    blazes: FxHashMap<IVec3, blaze::Blaze>,
    /// Blocks set smouldering this tick, for telling whoever's change did it (`blaze_hint`).
    blaze_risked: Vec<IVec3>,
    /// The smouldering blocks players were last told of.
    smoulders_sent: Vec<IVec3>,
    /// Whose doing a fire that starts now is (a player's key, `player_key`): the player whose
    /// message is being handled or whose action commits, whoever lights a hearth as it is lit
    /// (`hearth_blaze`), a burning block's own fire as it spreads; none for a flood or a
    /// command. Homes catch only from their owner (fire.ron).
    fire_origin: Option<String>,
    /// The player whose own action is committing now (their key, set by `commit` however late
    /// it falls due), and no one while a fire spreads, a flood runs or a command works: only
    /// their own breaking of their home hearth's hearthstone forgets whose it is
    /// (`home_changed`).
    changed_by: Option<String>,
    planter_grown: FxHashSet<IVec3>,
    /// Kept hives with bees, and how far each is toward its honey (`apiary`).
    apiary: apiary::Apiary,
    /// What is on each loaded home station, and when it is done (`stations`).
    stations: stations::Loads,
    /// What each sprung trap caught (`traps`).
    catches: traps::Catches,
    /// When the player message being handled arrived (see `actions`).
    arrival: Instant,
    /// The moment before which every player message has been handled: the last time all
    /// were taken in, except messages past a player's share for the tick (the
    /// `messages_left` cap), which only that player pays for. A creature's blow is judged
    /// once it has landed by then (`fight`).
    /// None until messages are first taken in (unit tests tick without them).
    heard: Option<Instant>,
    /// Animals and monsters about.
    creatures: creatures::Creatures,
    /// The people of settlements near players.
    villagers: villagers::Villagers,
    /// Parties that come by on their own errands (`wandering`).
    wandering: wandering::Wandering,
    lairs: lairs::Lairs,
    /// A new player's health and kit, and what falls, lava, fires and water do.
    survival: Arc<rules::SurvivalRules>,
    /// Swings, guards, dodges and creatures' wind-ups (`fight`).
    fights: fight::Fights,
    processes: Vec<Box<dyn Process>>,
    /// `core:temperature`, mean air temperature in °C, baseline from the climate generator.
    temperature: Field,
    /// How ground wears under feet and heals.
    wear_rules: Arc<wear::WearRules>,
    /// Humidity, cloud and precipitation, evolved by the weather process, and the weather
    /// admins have forced.
    weather: WeatherState,
    world: VoxelWorld,
    /// Water and lava waiting to flow.
    fluids: Fluids,
    /// Columns edited since they were last saved. They stay loaded until saved; without a
    /// save, that is forever.
    dirty: FxHashSet<ColumnPos>,
    /// Columns whose files could not be read, which saving leaves alone (`Loaded`).
    kept: FxHashSet<ColumnPos>,
    /// The world's worldgen data changed since land was last made in it, so admins who
    /// join are warned that new land may not line up (`WorldMeta::worldgen_content`).
    generation_changed: bool,
    /// Whether a server made to run behind (`ServerConfig::lag`) does so now
    /// (`ServerHandle::set_lagging`).
    lagging: Arc<AtomicBool>,
    /// How many times the game's pace the server ticks now, on top of
    /// `ServerConfig::tick_speedup` (`ServerHandle::set_hurried`).
    hurried: Arc<AtomicU32>,
    /// The tick the server may tick up to, in lockstep (`ServerHandle::allow_tick`).
    leash: Option<Arc<Leash>>,
    /// When each address's new players had the starting kit, in ticks (`Server::grant_kit`).
    /// Kept only while the server runs.
    kits: FxHashMap<std::net::IpAddr, Vec<u64>>,
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
/// Columns on each side of a player's whose land must be in for streaming to stay capped
/// (`voxmar_engine::jobs`); as the client's `workers.rs` has it.
const LAND_AROUND: i32 = 2;
const COLUMNS_PER_TICK: usize = 48;

impl Server {
    fn new(
        config: ServerConfig,
        meta: WorldMeta,
        save: Option<WorldSave>,
        state: save::WorldState,
        content: Content,
        incoming: Receiver<ServerConnection>,
        console: Receiver<String>,
    ) -> Self {
        let overworld = Overworld::shared_loading(meta.seed, &content).expect("checked by load_content");
        let climate = overworld.clone();
        let temperature = Field::new("core:temperature", move |x, z| climate.site(x, z).temperature);
        let land = Arc::new(weather::OverworldLand { overworld: overworld.clone(), blocks: content.blocks.clone() });
        let chances = weather::chances(&content.weather, &overworld.dials().featured);
        let weather = WeatherState::new(meta.seed, overworld.dials().wind.into(), land, content.weather.clone(), chances);
        let processes = process::core_processes();
        let generator: Arc<dyn Generator> = overworld.clone();
        // Load the spawn column up front and stand on its highest ground under any tree,
        // so trees never bury the player or hold them up on a crown (`find_ground`). The
        // spawn found with what settles by it, before any tick (`Overworld::settle_spawn`).
        let mut world = VoxelWorld::default();
        let spawn_block = overworld.settle_spawn();
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
        let items = items::Items::restore(state.items, |name| content.blocks.item_named(name));
        let crops = &content.crops;
        let farm = farm::Farm::restore(state.crops, |name| {
            crops.iter().find(|(_, c)| c.def.name == name).map(|(id, c)| (id, c.stages.len() as u8))
        });
        let placed = meta.placed.iter().map(|&p| IVec3::from_array(p)).collect();
        let quest_placed = quests::restore_placed(&meta.quest_placed, &content.blocks, &content.quests);
        let regrowing = regrow::restore(&meta.regrowing, |name| content.blocks.id(name));
        let flames = flames::restore(&meta.flames, |name| content.blocks.id(name), |block| content.flames.burn_life(block) > 0);
        let blazes = blaze::restore(&meta.blazes);
        let lairs = lairs::Lairs::restore(&meta.lairs);
        let planter_grown = meta.planter_grown.iter().map(|&p| IVec3::from_array(p)).collect();
        let apiary = apiary::Apiary::restore(&meta.hives);
        let stations = stations::Loads::restore(&meta.stations, |name| content.blocks.item_named(name));
        let catches = traps::Catches::restore(&meta.catches, &meta.standing_traps, meta.trap_serials, |name| content.creatures.id(name));
        let lit = meta.lit.iter().filter(|fire| fire.until > tick && content.multiblocks.get(&fire.structure).is_some())
            .map(|fire| (IVec3::from_array(fire.position), fire.clone())).collect::<FxHashMap<_, _>>();
        let hearth_owners = hearth::HearthOwner::restore(&meta.hearth_owners, &lit, |name| content.multiblocks.get(name).is_some_and(|m| m.home));
        let smoulders = smoulder::restore(&meta.smoulders, |name| content.multiblocks.get(name).is_some_and(|m| m.smoulder.is_some()));
        let survival = Arc::new(rules::SurvivalRules::load(&content.files, &content.blocks, &mut Vec::new()).expect("checked by load_content"));
        let mut server = Self {
            config,
            land_missing: Default::default(),
            tick,
            day: meta.day.unwrap_or(tick) as f64,
            day_speed: meta.day_speed,
            generation: meta.generation,
            meta,
            save,
            content,
            generator,
            overworld,
            executor: Executor::default(),
            items,
            farm,
            factory: voxmar_engine::factory::Factory::default(),
            lit,
            smoulders,
            hearth_owners,
            fires_sent: Vec::new(),
            placed,
            quest_placed,
            regrowing,
            flames,
            blazes,
            blaze_risked: Vec::new(),
            smoulders_sent: Vec::new(),
            fire_origin: None,
            changed_by: None,
            planter_grown,
            apiary,
            stations,
            catches,
            arrival: Instant::now(),
            heard: None,
            creatures: creatures::Creatures::default(),
            villagers: villagers::Villagers::restore(state.villagers),
            wandering: Default::default(),
            lairs,
            survival,
            fights: fight::Fights::default(),
            processes,
            temperature,
            wear_rules,
            weather,
            world,
            fluids,
            dirty: FxHashSet::default(),
            kept: if loaded.keep_file { FxHashSet::from_iter([spawn_column]) } else { FxHashSet::default() },
            generation_changed: false,
            lagging: Arc::new(AtomicBool::new(true)),
            hurried: Arc::new(AtomicU32::new(1)),
            leash: None,
            kits: FxHashMap::default(),
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
        server.lay_spawn_kit();
        server.restore_creatures(state.creatures);
        server.restore_companions(state.companions);
        server.restore_machines(state.machines);
        server
    }

    /// Lays the spawn kit by the spawn, once per world (`WorldMeta::spawn_kit`): its column is
    /// loaded by now. Not marked placed, so it is wild.
    fn lay_spawn_kit(&mut self) {
        if self.meta.spawn_kit {
            return;
        }
        let kit = spawn_kit::SpawnKit::load(&self.content.files, &self.content.blocks).expect("checked by load_content");
        // A spawn by a column's edge has the kit's ground in its neighbours too.
        let today = wear::day(self.day);
        for pos in kit.columns(self.spawn) {
            if self.world.contains(pos) {
                continue;
            }
            let loaded = load_column(self.save.as_ref(), &*self.generator, &self.content.blocks, &self.wear_rules, today, pos);
            self.insert_column(pos, loaded);
        }
        // An older save's columns are as players left them: a cell that differs from the
        // land made fresh was touched, even where `placed` does not know it (dug ground,
        // dirt set down).
        let mut fresh = VoxelWorld::default();
        for pos in kit.columns(self.spawn) {
            if self.save.as_ref().is_some_and(|save| save.has_column(pos)) {
                fresh.insert(pos, Arc::new(self.generator.generate(pos)));
            }
        }
        let touched = |p: IVec3| self.placed.contains(&p) || fresh.contains(ColumnPos::of_block(p)) && fresh.block(p) != self.world.block(p);
        let laid = kit.plan(&self.world, &self.content.blocks, self.spawn, self.meta.seed, touched);
        for (name, laid, least) in kit.short(&self.content.blocks, &laid) {
            let older = if self.meta.generator_version != Overworld::VERSION { " (the save is from an older world generator, so its land rarely matches what is generated now)" } else { "" };
            tracing::warn!("the spawn kit laid {laid} {name} by the spawn, short of its {least}: no more untouched ground in reach{older}");
        }
        for (at, block) in laid {
            self.set_block(at, block);
        }
        self.meta.spawn_kit = true;
    }

    /// Runs fixed-rate ticks until `stop` is set, then saves. Between ticks, messages are
    /// handled as soon as they arrive so actions feel immediate.
    fn run(mut self, stop: &AtomicBool, tick_times: &Mutex<VecDeque<Duration>>, ticks: &AtomicU64) {
        let real_tick = Duration::from_secs(1) / TICKS_PER_SECOND;
        let mut next_tick = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            // In lockstep, the tick its player allows, read before its messages: what the
            // player sent before allowing it is then handled before the tick.
            let allowed = self.leash.as_ref().map(|leash| leash.allowed.load(Ordering::Acquire));
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
            let held = allowed.is_some_and(|allowed| ticks.load(Ordering::Relaxed) >= allowed);
            if held {
                // Waiting on the player, not behind: its next tick goes as soon as allowed.
                next_tick = now;
                std::thread::park_timeout(Duration::from_millis(2));
                continue;
            }
            if now >= next_tick {
                self.tick();
                // Released after the tick's messages are sent: whoever reads this count
                // finds them waiting (`ServerHandle::ticks`).
                ticks.fetch_add(1, Ordering::Release);
                if let Some(leash) = &self.leash {
                    *leash.made.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = ticks.load(Ordering::Relaxed);
                    leash.ticked.notify_all();
                }
                if let Ok(mut times) = tick_times.lock() {
                    if times.len() == TICK_TIMES {
                        times.pop_front();
                    }
                    times.push_back(now.elapsed());
                }
                if self.lagging.load(Ordering::Relaxed) {
                    std::thread::sleep(self.config.lag.of_tick(self.tick));
                }
                // Land being generated on other threads arrives at the clock's pace, so a
                // server whose ticks run quicker than the clock (tests) waits for it at that
                // pace too: what happens in so many ticks is then what a game would see.
                let speedup = self.config.tick_speedup.max(1) * self.hurried.load(Ordering::Relaxed);
                next_tick += if speedup > 1 && !self.generating.is_empty() { real_tick } else { real_tick / speedup };
                if now > next_tick + real_tick * 10 {
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
            // Those still to say hello count against their own small limit, and fewer from
            // any one address, so sockets left silent cannot fill the server or crowd out
            // everyone else (players are counted at hello).
            let pending = || self.players.values().filter(|p| !p.joined);
            let limits = &self.config.limits;
            let key = |connection: &ServerConnection| connection.peer().map(|ip| limits.address_key(ip));
            let peer = key(&connection);
            if pending().count() >= limits.max_pending || peer.is_some() && pending().filter(|p| key(&p.connection) == peer).count() >= limits.max_pending_per_address
            {
                connection.send(ServerMessage::Rejected { reason: voxmar_engine::lang::Words::key("net.rejected.full") });
                continue;
            }
            let id = self.next_player;
            self.next_player += 1;
            self.players.insert(id, Player::new(id, connection, self.spawn, self.config.view_distance, self.tick, self.survival.max_health, &self.config.limits));
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
        if loaded.keep_file {
            self.kept.insert(pos);
        }
        self.world.insert(pos, Arc::new(loaded.column));
        self.fluids.column_loaded(&self.content.blocks, &self.world, pos, self.tick, loaded.saved);
        self.show_grown_crops(pos);
        self.reconcile_machines(pos, loaded.saved);
        self.flames_loaded(pos);
    }

    fn handle_messages(&mut self) {
        // A message is stamped as it goes into its channel, so every one stamped before now
        // is handled below, except messages past a player's share for the tick (the
        // `messages_left` cap), which only that player pays for.
        let heard = Instant::now();
        // Oldest connection first, so a player's dropped connection is seen gone before a
        // new one's hello under their name is answered (else a quick rejoin is turned away).
        let mut ids: Vec<PlayerId> = self.players.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            // A player's messages beyond their share of this tick wait for the next.
            while let Some(player) = self.players.get_mut(&id)
                && player.messages_left > 0
            {
                match player.connection.try_recv_at() {
                    Ok(Some((at, message))) => {
                        player.messages_left -= 1;
                        self.handle_message(id, message, at);
                    }
                    Ok(None) => {
                        if self.players[&id].idle(Instant::now(), self.config.idle_timeout) {
                            tracing::info!("{} sent nothing for {:?}; letting them go", self.players[&id].name, self.config.idle_timeout);
                            self.leave(id);
                        }
                        break;
                    }
                    Err(_) => self.leave(id),
                }
            }
        }
        self.heard = Some(heard);
    }

    /// Lets a player go. Leaving dead buries first, or rejoining would revive them with
    /// everything they carried. One who had joined is saved with the whole world: what they
    /// hold is saved as one with what machines and the ground hold, or a crash could double
    /// it. A connection that never said hello holds nothing, and opening and closing sockets
    /// must not make the server save.
    fn leave(&mut self, id: PlayerId) {
        self.bury_if_dead(id);
        let player = &self.players[&id];
        if player.joined {
            tracing::info!("{} left", player.name);
            self.save();
        }
        self.players.remove(&id);
    }

    /// The dropped items to save, and loot still waiting for a creature's body to fall,
    /// saved as lying where it will drop, or a server stopped in that moment would lose it
    /// (the creature is gone).
    fn saved_items(&self) -> Vec<save::SavedItem> {
        let blocks = &self.content.blocks;
        let mut saved = self.items.saved(|item| blocks.def(item).name.clone());
        saved.extend(self.creatures.falling_loot.iter().map(|(_, stack, body)| save::SavedItem {
            item: blocks.def(stack.item).name.clone(),
            count: stack.count,
            position: body.position.to_array(),
            age: 0.0,
            owner: None,
            lifetime: None,
            wear: stack.wear,
        }));
        saved
    }

    fn tick(&mut self) {
        let _span = tracing::info_span!("server_tick").entered();
        self.tick += 1;
        self.limit_players();
        // At normal speed a day takes the content's `day_minutes`.
        self.day += (self.day_speed * self.content.commands.day_pace()) as f64;
        self.run_processes();
        self.bury_the_dead();
        self.regrow();
        self.look_around();
        self.check_quests();
        self.tick_items();
        self.tick_creatures();
        self.tick_fights();
        self.ping_players();
        self.tick_prayers();
        self.flow();
        self.blaze();
        self.watch_stores();
        self.replicate_owned_traits();
        // Loading again around any player whose land is missing (one just come, or gone
        // beyond their view): every worker streams until it has come (voxmar_engine::jobs).
        let missing = self.players.values().any(|p| p.joined && self.land_missing_around(p.position));
        self.land_missing.set(missing);
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for &id in &ids {
            self.stream_columns(id);
        }
        self.unload_columns();
        self.broadcast_players();
        if self.tick.is_multiple_of(TICKS_PER_SECOND as u64) {
            self.broadcast(self.time_message());
            for id in self.players.iter().filter(|(_, p)| !p.kindled).map(|(&id, _)| id).collect::<Vec<_>>() {
                self.kindle_home(id);
            }
            self.send_fires();
        }
        if self.tick.is_multiple_of(SAVE_INTERVAL) {
            self.save();
        }
        if self.tick.is_multiple_of(60 * TICKS_PER_SECOND as u64) {
            self.forget_old_kits();
        }
    }

    /// Writes edited columns, joined players and the world's metadata, as one generation:
    /// if any of it fails, none of it counts, and edited columns are tried again next time.
    fn save(&mut self) {
        if self.save.is_none() {
            return;
        }
        self.note_all_deaths();
        // A player who died since the last tick (a command, say) is saved buried.
        self.bury_the_dead();
        let Some(save) = &self.save else { return };
        let _span = tracing::info_span!("save_world").entered();
        let started = Instant::now();
        // One generation: nothing counts until `world.ron` does, so a save cut short leaves
        // the last whole one, and what a player holds never also stays in a machine.
        let generation = self.generation.max(self.meta.generation) + 1;
        self.generation = generation;
        save.begin(generation);
        let mut ok = true;
        let mut failed = |what: &str, e: std::io::Error| {
            tracing::error!("could not save {what}: {e}");
            ok = false;
        };
        let (world, registry, kept) = (&self.world, &self.content.blocks, &self.kept);
        // A column no longer in the world has nothing left to save, and one whose file
        // could not be read is never saved over it (`kept`).
        self.dirty.retain(|&pos| {
            if world.column(pos).is_none() {
                return false;
            }
            if kept.contains(&pos) {
                tracing::warn!("not saving column {pos:?} over a file that could not be read");
                return false;
            }
            true
        });
        let mut written = FxHashSet::default();
        for &pos in &self.dirty {
            let Some(column) = world.column(pos) else { continue };
            match save.write_column(pos, column, registry) {
                Ok(()) => {
                    written.insert(pos);
                }
                Err(e) => failed(&format!("column {pos:?}"), e),
            }
        }
        if let Err(e) = save.write_machines(&self.saved_machines()) {
            failed("machines", e);
        }
        if self.config.cut_saves {
            tracing::warn!("save {generation} cut short after its machines, as asked");
            return;
        }
        for player in self.players.values().filter(|p| p.joined) {
            if let Err(e) = save.write_player(&player.name, &player.saved(&self.content.skills, &self.content.effects), registry) {
                failed(&format!("player {}", player.name), e);
            }
        }
        if let Err(e) = save.write_items(&self.saved_items()) {
            failed("dropped items", e);
        }
        if let Err(e) = save.write_creatures(&self.saved_creatures()) {
            failed("creatures", e);
        }
        if let Err(e) = save.write_companions(&self.saved_companions()) {
            failed("settled companions", e);
        }
        if let Err(e) = save.write_villagers(&self.villagers.saved()) {
            failed("who of the villagers died", e);
        }
        let crops = &self.content.crops;
        if let Err(e) = save.write_crops(&self.farm.saved(|id| crops.get(id).def.name.clone())) {
            failed("crops", e);
        }
        if !ok {
            // A part missing would let what moved since the last save be in two places, or
            // none: the world stays as last saved.
            save.abandon();
            tracing::error!("save {generation} of {} abandoned", save.dir().display());
            return;
        }
        let mut meta = self.meta.clone();
        meta.time = self.tick;
        meta.placed = self.placed.iter().map(|p| p.to_array()).collect();
        let mut quest_placed: Vec<([i32; 3], String, String)> = self.quest_placed.iter().map(|(cell, (owner, item))| (cell.to_array(), owner.clone(), item.clone())).collect();
        quest_placed.sort();
        meta.quest_placed = quest_placed;
        meta.regrowing = self.saved_regrowing();
        meta.flames = self.saved_flames();
        meta.blazes = self.saved_blazes();
        let mut grown: Vec<[i32; 3]> = self.planter_grown.iter().map(|p| p.to_array()).collect();
        grown.sort();
        meta.planter_grown = grown;
        meta.lairs = self.lairs.saved();
        meta.hives = self.apiary.saved();
        meta.stations = self.stations.saved(|item| self.content.blocks.def(item).name.clone());
        meta.catches = self.catches.saved(|species| self.content.creatures.get(species).def.name.clone());
        (meta.standing_traps, meta.trap_serials) = self.catches.saved_traps();
        meta.lit = self.lit.values().filter(|fire| fire.until > self.tick).cloned().collect();
        meta.lit.sort_by_key(|fire| fire.position);
        meta.smoulders = self.saved_smoulders();
        meta.hearth_owners = hearth::HearthOwner::saved(&self.hearth_owners);
        meta.day = Some(self.day as u64);
        meta.day_speed = self.day_speed;
        meta.last_played = save::unix_time();
        meta.generation = generation;
        match save.commit(&meta) {
            Ok(moved) => {
                self.meta = meta;
                // Columns saved are clean; any not moved into place are written again next time.
                if moved {
                    self.dirty.retain(|pos| !written.contains(pos));
                }
            }
            Err(e) => tracing::error!("could not save {}: {e}", save.dir().display()),
        }
        tracing::debug!("saved {} columns and {} players in {:?}", written.len(), self.players.len(), started.elapsed());
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

    /// Tells each player of the others near enough to see (`Limits::players_seen`), only
    /// as they change: a view sent again only once it differs from the last one sent them, a
    /// name only the first time (near or far: those out of sight are named in `names`, so
    /// a creature they keep shows its keeper), and those gone or out of sight named once as
    /// gone. One already told of goes out of sight only past `players_seen_margin` more.
    fn broadcast_players(&mut self) {
        let views: Vec<voxmar_protocol::PlayerView> = self.players.values().filter(|p| p.joined).map(|p| voxmar_protocol::PlayerView { fight: self.fight_view(p.id), ..p.view() }).collect();
        let (seen, margin) = (self.config.limits.players_seen, self.config.limits.players_seen_margin);
        for player in self.players.values_mut().filter(|p| p.joined) {
            let near: Vec<&voxmar_protocol::PlayerView> = views
                .iter()
                .filter(|v| v.id != player.id && v.position.distance(player.position) <= if player.told.contains_key(&v.id) { seen + margin } else { seen })
                .collect();
            let mut told = Vec::new();
            for &view in &near {
                let last = player.told.get(&view.id);
                if last == Some(view) {
                    continue;
                }
                let name = view.name.as_deref().unwrap_or_default();
                let named = player.named.get(&view.id).map(String::as_str) != Some(name);
                if named {
                    player.named.insert(view.id, name.to_owned());
                }
                told.push(voxmar_protocol::PlayerView { name: if named { view.name.clone() } else { None }, ..view.clone() });
                player.told.insert(view.id, view.clone());
            }
            let mut names = Vec::new();
            for view in views.iter().filter(|v| v.id != player.id && !near.iter().any(|n| n.id == v.id)) {
                let name = view.name.as_deref().unwrap_or_default();
                if player.named.get(&view.id).map(String::as_str) != Some(name) {
                    player.named.insert(view.id, name.to_owned());
                    names.push((view.id, name.to_owned()));
                }
            }
            // Those who left the server are named once more, with no name, to be forgotten.
            player.named.retain(|id, _| {
                let here = views.iter().any(|v| v.id == *id);
                if !here {
                    names.push((*id, String::new()));
                }
                here
            });
            let gone: Vec<PlayerId> = player.told.keys().copied().filter(|id| !near.iter().any(|v| v.id == *id)).collect();
            for id in &gone {
                player.told.remove(id);
            }
            if !(told.is_empty() && gone.is_empty() && names.is_empty()) {
                player.connection.send(ServerMessage::Players { seen: told, gone, names });
            }
        }
    }

    /// Whether a column within `LAND_AROUND` of `position`'s is not loaded.
    fn land_missing_around(&self, position: Vec3) -> bool {
        let center = ColumnPos::of_block(position.floor().as_ivec3());
        (-LAND_AROUND..=LAND_AROUND).any(|dz| (-LAND_AROUND..=LAND_AROUND).any(|dx| !self.world.contains(center.offset(dx, dz))))
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
                } else if self.generating.len() < MAX_GENERATING
                    && !self.generating.contains(&pos)
                    // Generating on fewer workers than there are, when a game playing on this
                    // machine asks it to (voxmar_engine::jobs).
                    && let Some(slot) = voxmar_engine::jobs::start(voxmar_engine::jobs::Kind::Columns)
                {
                    self.generating.insert(pos);
                    let (generator, save, registry) = (self.generator.clone(), self.save.clone(), self.content.blocks.clone());
                    let (wear, today) = (self.wear_rules.clone(), wear::day(self.day));
                    let tx = self.generated_tx.clone();
                    rayon::spawn(move || {
                        let _slot = slot;
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
            self.kept.remove(&pos);
            self.fluids.column_unloaded(pos);
        }
    }

    pub(crate) fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.world.block(pos)
    }

    /// Whether a live player's body takes up any of `cell`.
    pub(crate) fn player_in(&self, cell: IVec3) -> bool {
        self.players.values().any(|p| p.joined && p.health.alive() && p.aabb().intersects(|c| c == cell))
    }

    /// Whether a live creature's body is in `cell`.
    pub(crate) fn creature_in(&self, cell: IVec3) -> bool {
        self.creatures.live.values().any(|c| c.health > 0 && c.body.aabb().intersects(|at| at == cell))
    }

    /// Whether putting `block` at `cell` would shut a player inside it: a solid block where
    /// a player stands, which no hand may place. Admin fills leave such a cell as it is
    /// (`/fill`, `/setblock`); regrowth brings back only plants that are not solid.
    pub(crate) fn would_trap_a_player(&self, cell: IVec3, block: BlockId) -> bool {
        self.content.blocks.info(block).solid && self.player_in(cell)
    }

    pub(crate) fn is_solid(&self, pos: IVec3) -> bool {
        self.block(pos).is_some_and(|b| self.content.blocks.info(b).solid)
    }

    /// What a cell stops a player with, in blocks from its lower corner (see
    /// `BlockRegistry::collision`): the boxes the client moves its player against. Space
    /// not loaded (or under the world) is a whole cube, as the client judges it, so the two
    /// agree at the edge of the loaded world: a player waits there rather than walk or fall
    /// into land not loaded yet.
    pub(crate) fn boxes(&self, pos: IVec3) -> &[voxmar_engine::physics::Aabb] {
        self.block(pos).map_or(&voxmar_engine::physics::CUBE, |b| self.content.blocks.collision(b))
    }

    /// Changes many blocks at once (a command's), telling each player about those in columns
    /// they have in one message, and waking any fluid they let flow.
    pub(crate) fn set_blocks(&mut self, changes: &[(IVec3, BlockId)]) {
        let mut done = Vec::with_capacity(changes.len());
        for &(pos, block) in changes {
            if let Some(old) = self.world.set_block(pos, block) {
                self.dirty.insert(ColumnPos::of_block(pos));
                self.forget_placed(pos);
                self.unplaced(pos, block);
                done.push((pos, block));
                self.machine_changed(pos, old, block);
                self.hive_changed(pos, old, block);
                self.station_changed(pos, old, block);
                self.trap_changed(pos, block);
                self.fire_changed(pos);
                self.smoulder_changed(pos);
                self.home_changed(pos, block);
                self.flame_changed(pos, block);
                self.blaze_changed(pos, block);
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
    /// A block changed: whatever a player placed there, or a planter grew, is gone (those
    /// who place and grow mark it again after setting it).
    fn forget_placed(&mut self, pos: IVec3) {
        self.placed.remove(&pos);
        self.planter_grown.remove(&pos);
    }

    pub(crate) fn set_block(&mut self, pos: IVec3, block: BlockId) {
        let column = ColumnPos::of_block(pos);
        if let Some(old) = self.world.set_block(pos, block) {
            self.dirty.insert(column);
            self.forget_placed(pos);
            self.unplaced(pos, block);
            self.machine_changed(pos, old, block);
            self.hive_changed(pos, old, block);
            self.station_changed(pos, old, block);
            self.trap_changed(pos, block);
            self.fire_changed(pos);
            self.smoulder_changed(pos);
            self.home_changed(pos, block);
            self.flame_changed(pos, block);
            self.blaze_changed(pos, block);
            for player in self.players.values().filter(|p| p.joined && p.sent.contains(&column)) {
                player.connection.send(ServerMessage::BlockChanged { pos, block });
            }
            self.fluids.changed(&self.content.blocks, &self.world, pos, self.tick);
        }
    }
}

/// The spot nearest `start` within its column whose highest ground, under any tree (blocks
/// tagged `tree`: trunks, bark, roots, leaves, giant mushroom caps), is opaque with two
/// free blocks above it: the player wakes on the land under or beside a tree, never on its
/// crown or in it.
fn find_ground(world: &VoxelWorld, registry: &BlockRegistry, start: IVec3) -> Vec3 {
    let origin = ColumnPos::of_block(start).origin();
    let mut candidates: Vec<IVec3> =
        (0..CHUNK_SIZE).flat_map(|z| (0..CHUNK_SIZE).map(move |x| origin + IVec3::new(x, 0, z))).collect();
    candidates.sort_by_key(|p| (p.x - start.x).abs() + (p.z - start.z).abs());
    let info = |p: IVec3| world.block(p).map(|b| registry.info(b));
    let tree = |p: IVec3| world.block(p).is_some_and(|b| registry.def(b).has_tag("tree"));
    for column in candidates {
        let Some(ground) = (0..WORLD_HEIGHT).rev().map(|y| column.with_y(y)).find(|&p| info(p).is_some_and(|i| i.solid) && !tree(p))
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

/// A server on the game's own content with a new world kept in memory, for unit tests.
#[cfg(test)]
pub(crate) fn test_server() -> Server {
    test_server_on(test_content())
}

/// The draw key of every test server's world (`test_server_on`).
#[cfg(test)]
pub(crate) const TEST_DRAW_KEY: u64 = 0x0d1a;

/// The game's own content, as `test_server` loads it: loaded once for the run (0.14 s a
/// load, and some 300 tests ask for it) and handed out as copies, which share its tables.
/// A test that changes a table of its copy changes its own (`Arc::make_mut`), never the
/// one the others are handed.
#[cfg(test)]
pub(crate) fn test_content() -> Content {
    static CONTENT: std::sync::OnceLock<Content> = std::sync::OnceLock::new();
    CONTENT.get_or_init(|| load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads")).clone()
}

/// As `test_server`, on `content` already loaded: a test that makes many servers loads it
/// once (a copy shares its tables, so one is changed through `Arc::make_mut`).
#[cfg(test)]
pub(crate) fn test_server_on(content: Content) -> Server {
    // One draw key for every test server, as a world's own is random: wandering, grazing
    // and the world's other keyed rolls draw the same in every run, so a test's outcome
    // depends on its inputs alone.
    let mut config = ServerConfig { draw_key: Some(TEST_DRAW_KEY), ..ServerConfig::default() };
    let (save, meta) = open_world(&mut config).expect("no save to open");
    // Every test server is on the one seed: its world, held for the whole run, is the one
    // each shares (`Overworld::shared`), with the land round spawn planned once rather than
    // again for each (1.7 s a server; a test that made 34 took 75 s).
    static HELD: std::sync::OnceLock<Arc<Overworld>> = std::sync::OnceLock::new();
    // Its columns too, about 1170 of them: every test server stands round the same spawn.
    // Servers sharing one held world wait on its inner caches less than servers on worlds
    // of their own would; worldgen's own thread tests still cover that contention.
    HELD.get_or_init(|| {
        let world = Overworld::shared(meta.seed, &content, &WorldContent::load(&content).expect("checked by load_content"));
        world.remember_columns(128 << 20);
        world
    });
    let (_, incoming) = crossbeam_channel::unbounded();
    let (_, console) = crossbeam_channel::unbounded();
    Server::new(config, meta, save, save::WorldState::default(), content, incoming, console)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a new world of `seed` wakes its first player, from the spawn column alone as
    /// `Server::new` loads it, and the block they stand on.
    fn spawn_on(seed: u64, content: &Content, world_content: &WorldContent) -> (Vec3, Option<BlockId>) {
        let generator = Overworld::shared(seed, content, world_content);
        let start = generator.spawn_point();
        let mut world = VoxelWorld::default();
        world.insert(ColumnPos::of_block(start), Arc::new(generator.generate(ColumnPos::of_block(start))));
        let spawn = find_ground(&world, &content.blocks, start);
        (spawn, world.block((spawn - Vec3::Y * 0.5).floor().as_ivec3()))
    }

    /// A new player never wakes on a tree (a block tagged `tree`): with seed 157 they woke
    /// on an oak's crown of bark, its top the highest solid block in the column; now they
    /// stand on the land under or beside it. And over a run of seeds, none does.
    #[test]
    fn a_new_player_never_wakes_on_a_tree() {
        let content = test_content();
        let world_content = WorldContent::load(&content).expect("checked by load_content");
        let tree = |b: Option<BlockId>| b.is_some_and(|b| content.blocks.def(b).has_tag("tree"));
        // Each seed plans its land round spawn (about 1.6 s), so they plan side by side.
        let woke: Vec<_> = std::thread::scope(|scope| {
            let (content, world_content) = (&content, &world_content);
            let runs: Vec<_> = [157, 1, 2, 3, 4, 5].map(|seed| scope.spawn(move || (seed, spawn_on(seed, content, world_content)))).into_iter().collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        for (seed, (spawn, under)) in woke {
            let name = under.map(|b| content.blocks.def(b).name.clone());
            assert!(under.is_some_and(|b| content.blocks.info(b).solid) && !tree(under), "seed {seed}: the player wakes at {spawn} on {name:?}");
        }
    }

    /// A world's draw key is made with it (the config's, else a random one) and kept in its
    /// meta; an older world, saved without one, is given one when it opens.
    #[test]
    fn a_world_keeps_its_draw_key_and_an_older_one_is_given_one() {
        let dir = std::env::temp_dir().join(format!("voxmar-draw-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = |key| ServerConfig { world: Some(dir.clone()), draw_key: key, ..Default::default() };
        let (save, meta) = open_world(&mut config(Some(42))).unwrap();
        assert_eq!(meta.draw_key, 42, "made with the config's key");
        assert_eq!(open_world(&mut config(None)).unwrap().1.draw_key, 42, "kept");
        save.as_ref().unwrap().write_meta(&WorldMeta { drops: 5, loot: 7, rolls: 11, ..meta.clone() }).unwrap();
        let kept = open_world(&mut config(None)).unwrap().1;
        assert_eq!((kept.drops, kept.loot, kept.rolls), (5, 7, 11), "the counts of drops popped, loot rolled and world rolls kept");
        save.unwrap().write_meta(&WorldMeta { draw_key: 0, ..meta }).unwrap();
        assert_ne!(open_world(&mut config(None)).unwrap().1.draw_key, 0, "an older world given one");
        assert_ne!(WorldMeta::new("a", 1, GameMode::Survival).draw_key, WorldMeta::new("a", 1, GameMode::Survival).draw_key, "random without one asked");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Test servers draw by one key, so wandering and the world's other rolls come out the
    /// same in every run (a random one left a density test failing one run in eight).
    #[test]
    fn test_servers_draw_the_same_every_run() {
        let content = test_content();
        let (mut a, mut b) = (test_server_on(content.clone()), test_server_on(content));
        assert_eq!((a.meta.draw_key, b.meta.draw_key), (TEST_DRAW_KEY, TEST_DRAW_KEY));
        assert_eq!(a.meta.next_roll(crate::save::ROLL_WANDER), b.meta.next_roll(crate::save::ROLL_WANDER));
    }

    /// A drop and its rejoin are queued together before one reading loop: the old
    /// connection must leave before the new hello is answered, without scheduler luck.
    #[test]
    fn a_dropped_player_rejoining_in_the_same_loop_is_let_in() {
        use voxmar_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, local_pair};
        let mut server = test_server();
        let (connections, incoming) = unbounded();
        server.incoming = incoming;
        let fingerprint = server.content.fingerprint();
        let join = |server: &mut Server| {
            let (client, server_side) = local_pair();
            connections.send(server_side).unwrap();
            client.send(ClientMessage::Hello { name: "bob".into(), protocol: PROTOCOL_VERSION, content: fingerprint, admin_token: None });
            server.accept_connections();
            server.handle_messages();
            client
        };
        let answer = |client: &voxmar_protocol::ClientConnection| loop {
            match client.try_recv() {
                Ok(Some(ServerMessage::Welcome { .. })) => break Ok(()),
                Ok(Some(ServerMessage::Rejected { reason })) => break Err(reason),
                Ok(Some(_)) => {}
                other => panic!("no answer to the hello: {other:?}"),
            }
        };
        let mut bob = join(&mut server);
        answer(&bob).expect("the first bob is let in");
        for round in 0..8 {
            drop(bob);
            bob = join(&mut server);
            answer(&bob).unwrap_or_else(|reason| panic!("round {round}: same-loop rejoin rejected: {reason:?}"));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_threads_cpu_time_counts_its_work_and_not_its_sleep() {
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let (done, spun) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            // A fixed amount of work, however long a busy machine takes to give it.
            let mut spin = 0u64;
            for _ in 0..20_000_000 {
                spin = std::hint::black_box(spin.wrapping_add(1));
            }
            done.send(()).unwrap();
            let _ = wait.recv();
        });
        // Asleep on the channel after its spin, it does no more work.
        spun.recv().unwrap();
        let worked = thread_cpu_time(&worker).expect("the system tells a thread's CPU time");
        assert!(worked > Duration::ZERO, "no CPU time for its spin");
        std::thread::sleep(Duration::from_millis(50));
        let later = thread_cpu_time(&worker).unwrap();
        assert!(later - worked < Duration::from_millis(10), "{worked:?} then {later:?} while it slept");
        go.send(()).unwrap();
        worker.join().unwrap();
    }

    /// Preparation can happen before connecting; an accepted peer still has to say
    /// hello within the configured deadline. Check the boundary without wall-clock luck.
    #[test]
    fn a_silent_pending_login_expires_after_its_configured_ticks() {
        let mut server = test_server();
        let (accepted, incoming) = unbounded();
        server.incoming = incoming;
        let (client, server_side) = voxmar_protocol::local_pair();
        accepted.send(server_side).unwrap();
        server.accept_connections();
        let (&id, player) = server.players.iter().next().unwrap();
        let connected = player.connected_tick;
        let hello_ticks = (server.config.limits.hello_seconds * TICKS_PER_SECOND as f32) as u64;
        assert_eq!(server.config.limits.hello_seconds, 3.0);
        server.tick = connected + hello_ticks;
        server.limit_players();
        assert!(server.players.contains_key(&id), "expired before the deadline");
        server.tick += 1;
        server.limit_players();
        assert!(!server.players.contains_key(&id), "kept a silent pending connection");
        assert!(client.try_recv().is_err(), "the expired connection was still open");
    }

    #[test]
    fn loot_waiting_for_a_body_to_fall_is_saved_where_it_will_drop() {
        let content = load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let dir = std::env::temp_dir().join(format!("voxmar-falling-loot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = WorldMeta::new("loot", 1, GameMode::Survival);
        let save = WorldSave::new(&dir);
        save.create(&meta).unwrap();
        let ((_, incoming), (_, console)) = (unbounded(), unbounded());
        let config = ServerConfig { view_distance: 2, ..Default::default() };
        let mut server = Server::new(config, meta, Some(save), save::WorldState::default(), content.clone(), incoming, console);
        let slime = content.blocks.expect("base:lumen_slime");
        let at = Vec3::new(3.5, 70.0, -2.5);
        server.creatures.falling_loot.push((server.tick + 100, voxmar_engine::inventory::ItemStack::new(slime, 2), voxmar_engine::dropped::Loose::new(at, Vec3::Y)));
        server.save();
        let saved = WorldSave::new(&dir).read_items().unwrap();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!((saved[0].item.as_str(), saved[0].count, saved[0].position), ("base:lumen_slime", 2, at.to_array()));
        // Saving takes nothing from the queue: the loot still drops when its body has fallen.
        assert_eq!(server.creatures.falling_loot.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Every item a player could hold has a way in the recipe book or is tagged
    /// creative_only; a package adding an item nothing gives fails to load, named with its
    /// package, until it is given a way or the tag.
    #[cfg(unix)]
    #[test]
    fn an_item_no_way_gives_fails_to_load_named_with_its_package() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = load_content(&assets).unwrap();
        let named = |ids: Vec<voxmar_engine::block::BlockId>| ids.into_iter().map(|id| content.blocks.def(id).name.clone()).collect::<Vec<_>>();
        let (dead, stale) = recipe_book(&content).unwrap().unsourced(&content.blocks);
        assert_eq!((named(dead), named(stale)), (Vec::new(), Vec::new()), "the game's own items each have a way, and no creative_only tag is stale");
        // Found where a village builds with it, named with its style's wood ("stripped_{wood}_log"),
        // and foraged from a ripe crop the world grows wild.
        let book = recipe_book(&content).unwrap();
        let id = |name: &str| content.blocks.id(name).unwrap();
        let spruce = book.making(id("base:stripped_spruce_log"));
        assert!(spruce.iter().any(|e| matches!(e.method, voxmar_engine::recipe_book::Method::Found { .. })), "{spruce:?}");
        // A village's bed is no find: broken where the world placed it, it gives nothing.
        let bed = book.making(id("base:oak_bed"));
        assert!(!bed.iter().any(|e| matches!(e.method, voxmar_engine::recipe_book::Method::Found { .. })), "{bed:?}");
        let tomato = book.making(id("base:tomato_seeds"));
        assert!(tomato.iter().any(|e| matches!(e.method, voxmar_engine::recipe_book::Method::Harvest { forage: true, from, .. } if from == id("base:tomato_crop"))), "{tomato:?}");
        let dir = std::env::temp_dir().join(format!("voxmar-no-way-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mods = dir.join("content");
        std::fs::create_dir_all(mods.join("moonmod")).unwrap();
        for package in ["base", "core"] {
            std::os::unix::fs::symlink(assets.join("content").join(package).canonicalize().unwrap(), mods.join(package)).unwrap();
        }
        let block = |tags: &str| format!(r#"(blocks: [(name: "moon_rock", textures: All("stone"), hardness: 1.0, color: (90, 90, 110){tags})])"#);
        std::fs::write(mods.join("moonmod/blocks.ron"), block("")).unwrap();
        let Err(error) = load_content(&dir).map(|_| ()) else { panic!("a moon rock nothing gives loaded") };
        let error = error.to_string();
        assert!(error.contains("`moonmod:moon_rock` (package moonmod)") && error.contains("creative_only"), "{error}");
        // A recipe for it is a way: it loads.
        std::fs::write(mods.join("moonmod/recipes.ron"), r#"(recipes: [(ingredients: ["base:cobblestone", "base:flint"], output: "moon_rock")])"#).unwrap();
        let content = load_content(&dir).unwrap();
        let rock = content.blocks.expect("moonmod:moon_rock");
        assert!(recipe_book(&content).unwrap().making(rock).iter().any(|e| matches!(e.method, voxmar_engine::recipe_book::Method::Craft { .. })));
        // So does the tag, without one.
        std::fs::remove_file(mods.join("moonmod/recipes.ron")).unwrap();
        std::fs::write(mods.join("moonmod/blocks.ron"), block(r#", tags: ["creative_only"]"#)).unwrap();
        load_content(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_shipped_fluid_is_swum_in_or_burns() {
        let content = load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        assert!(blocks.fluids().iter().all(|fluid| swum_or_burns(blocks, fluid)));
        // Water that is not swum in would be named at load.
        let mut water = blocks.fluid_def(blocks.fluid(blocks.expect("base:water")).unwrap().fluid).clone();
        water.def.swim = false;
        assert!(!swum_or_burns(blocks, &water));
    }

    #[test]
    fn addresses_count_together_by_ipv4_address_or_ipv6_prefix() {
        let limits = Limits::default();
        let key = |ip: &str| limits.address_key(ip.parse().unwrap());
        assert_eq!(key("::ffff:127.0.0.1"), key("127.0.0.1"));
        assert_eq!(key("::ffff:127.0.0.1"), "127.0.0.1".parse::<IpAddr>().unwrap());
        // One host's /64 shares a kit and a share of those waiting to say hello...
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:2:abcd:ef01:2345:6789"));
        // ...while the next /64, and other IPv4 addresses, do not.
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
        assert_ne!(key("10.0.0.1"), key("10.0.0.2"));
        let whole = Limits { ipv6_prefix: 128, ..Limits::default() };
        assert_ne!(whole.address_key("2001:db8::1".parse().unwrap()), whole.address_key("2001:db8::2".parse().unwrap()));
    }

    #[test]
    fn one_ipv6_host_or_one_machine_over_both_gets_one_kit_and_old_kits_are_forgotten() {
        let content = load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let ((_, incoming), (_, console)) = (unbounded(), unbounded());
        let config = ServerConfig { view_distance: 2, ..Default::default() };
        let mut server = Server::new(config, WorldMeta::new("kits", 1, GameMode::Survival), None, save::WorldState::default(), content, incoming, console);
        let mut grant = |ip: &str| server.grant_kit(ip.parse().unwrap());
        assert!(grant("2001:db8:1:2::1"));
        assert!(!grant("2001:db8:1:2::77"), "the same /64");
        assert!(grant("2001:db8:1:3::1"), "the next /64");
        assert!(grant("127.0.0.1"));
        assert!(!grant("::ffff:127.0.0.1"), "the same machine through a dual-stack listener");
        server.forget_old_kits();
        assert_eq!(server.kits.len(), 3);
        server.tick += (server.survival.kit_window * TICKS_PER_SECOND as f32) as u64;
        server.forget_old_kits();
        assert!(server.kits.is_empty());
    }

    #[test]
    fn a_limits_file_sets_what_it_names_and_refuses_numbers_that_shut_everyone_out() {
        let limits = Limits::from_ron("(chat_burst: 4.0, max_pending_per_address: 2)").unwrap();
        assert_eq!(limits, Limits { chat_burst: 4.0, max_pending_per_address: 2, ..Limits::default() });
        assert_eq!(Limits::from_ron("()").unwrap(), Limits::default());
        assert_eq!(Limits::default().rejoin_silence, 6.0, "a keepalive and a second");
        for bad in ["(max_pending: 0)", "(max_pending_per_address: 0)", "(messages_per_tick: 1)", "(hello_seconds: 0.0)", "(chat_per_second: -1.0)", "(name_length: 0)", "(ipv6_prefix: 8)", "(ipv6_prefix: 129)", "(rejoin_silence: 5.0)", "(rejoin_silence: 4000.0)", "(chat_brust: 4.0)", "(players_seen: 2.0)", "(players_seen: 5000.0)", "(players_seen_margin: -1.0)", "(players_seen_margin: 100.0)"] {
            assert!(Limits::from_ron(bad).is_err(), "{bad} is accepted");
        }
    }

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

    /// Village furnishings give nothing when broken (design draft 80), and nothing about
    /// them lets them be picked for more: regrowth does not take them, and the journal does
    /// not log them as wild plants. The props are read from settlements.ron's furnishings
    /// (barrel, bell, hay, sacks, crates, hanging things, nets), so a mod's own props are held to it
    /// too.
    #[test]
    fn village_furnishings_drop_nothing() {
        use voxmar_worldgen::Select;
        let content = load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content");
        let world = WorldContent::load(&content).expect("world content");
        let f = &world.settlements.furnishings;
        let blocks = &content.blocks;
        let mut props: Vec<voxmar_engine::block::BlockId> = f.barrel.iter().chain(&f.bell).map(|name| blocks.expect(name)).collect();
        // A list may pick by tag: every block with the tag is a prop.
        for list in [&f.hay, &f.sacks, &f.crates, &f.hanging, &f.nets] {
            for select in list {
                match select {
                    Select::Name(name, _) => props.push(blocks.expect(name)),
                    Select::Tag(tag, _) => props.extend(blocks.tagged(tag)),
                }
            }
        }
        assert!(props.len() >= 6, "only {} props", props.len());
        for id in props {
            let name = blocks.def(id).name.clone();
            let (def, info) = (blocks.def(id), blocks.info(id));
            assert_eq!(info.drops, voxmar_engine::block::BlockId::AIR, "{name} drops something");
            assert!(!content.regrowth.regrows(def, info.solid), "{name} regrows when picked");
            let wild = ["plant", "flower", "herb", "fungus"].iter().any(|t| def.has_tag(t));
            assert!(info.solid || !wild, "{name} would be logged as a wild plant");
        }
    }
}
