//! World saves.
//!
//! A world is a directory:
//! - `world.ron`: name, seed, game mode, creation and last-played times, the generator
//!   that made it, the world's age and the time of day and how fast it runs.
//! - `columns/<x>.<z>.bin`: each column players have edited, as zlib-compressed postcard
//!   together with the names of the blocks it uses, so saves survive content changes that
//!   renumber blocks. Untouched columns are not stored: generation recreates them exactly.
//! - `players/<name>.ron`: each player's position, game mode, health, inventory, skills and effects.
//! - `items.ron`: the dropped items lying about, by item name.
//! - `crops.ron`: the crops players planted, by crop name, with their stage and the tick
//!   their next is due.
//! - `villagers.ron`: who of each settlement's people died: how many newcomers have taken
//!   their place, and the day the next comes.
//! - `machines.ron`: the machines players built, by block name, with what they hold.
//!
//! Every file is written to a temporary file and then renamed over the old one, so a crash
//! never leaves a half-written file behind. A file that cannot be read is never silently
//! replaced: it is moved aside to `<file>.corrupt-<unix time>` (`set_aside`) and the error
//! logged, and if it cannot even be moved the server refuses to start (world files) or the
//! player to join (player files).
//!
//! A save is one generation: everything but `world.ron` is first written beside its file as
//! `<file>.<generation>.new`, then `world.ron` takes the new generation (the moment the save
//! counts), then each is renamed into place. A save cut short before `world.ron` leaves the
//! last whole save as it was, so nothing a player put in a machine (or took out) is both
//! held and in the machine; one cut short after it is finished the next time the world
//! opens ([`WorldSave::recover`]).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::chunk::Chunk;
use voxmar_engine::coords::{CHUNK_VOLUME, ColumnPos};
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::{Inventory, ItemStack, MAX_STACK};
use voxmar_engine::traits::Health;
use voxmar_engine::world::{COLUMN_AREA, Column};
use voxmar_worldgen::Overworld;

const META_FILE: &str = "world.ron";
/// Kept creatures (tamed or bred).
const CREATURES_FILE: &str = "creatures.ron";
const ITEMS_FILE: &str = "items.ron";
const CROPS_FILE: &str = "crops.ron";
const VILLAGERS_FILE: &str = "villagers.ron";
const MACHINES_FILE: &str = "machines.ron";
/// Wanderers who settled at a player's home.
const COMPANIONS_FILE: &str = "companions.ron";
/// What a file being saved ends with, after its generation.
const STAGED: &str = "new";
/// Column files start with this, followed by the zlib stream. Bump the digit when the
/// layout changes.
const COLUMN_MAGIC: &[u8; 4] = b"vxc4";

/// How a world is played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameMode {
    /// No flight.
    #[default]
    Survival,
    /// Flight is allowed.
    Creative,
}

/// `world.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldMeta {
    pub name: String,
    pub seed: u64,
    pub mode: GameMode,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub last_played: u64,
    /// Generator id and version the world was created with. Columns nobody edited are
    /// generated again, so a different generator would leave seams.
    pub generator: String,
    pub generator_version: u32,
    /// Server ticks since the world began: the clock the simulation (and the weather)
    /// runs on.
    #[serde(default)]
    pub time: u64,
    /// The time of day in ticks since the first sunrise, which commands can set; `None`
    /// in older worlds, whose time of day was `time`.
    #[serde(default)]
    pub day: Option<u64>,
    /// How fast the time of day runs, 0 when frozen.
    #[serde(default = "normal_speed")]
    pub day_speed: f32,
    /// Cells whose block a player placed: ores, which then teach nothing (experience comes
    /// from what the world holds, not from placing and breaking); plants, which don't regrow
    /// by themselves; and furnishings, which give themselves back only from these cells.
    #[serde(default)]
    pub placed: Vec<[i32; 3]>,
    /// Blocks set down that an unfinished quest's `Placed` counts: cell, whose (their name
    /// in lower case) and what item (none in older worlds). Saved with the world, so one
    /// broken while its owner is away, or after a restart, still comes off their count.
    #[serde(default)]
    pub quest_placed: Vec<([i32; 3], String, String)>,
    /// Picked wild plants waiting to grow back, and plants herb planters grew back.
    #[serde(default)]
    pub regrowing: Vec<SavedRegrowing>,
    /// Lights set down and burning, and the world's day tick each goes out (`flames`).
    #[serde(default)]
    pub flames: Vec<SavedFlame>,
    /// Columns whose lights set down before lights burnt out (or by the world) were given a
    /// burn the first time they loaded, sorted: each is given one once.
    #[serde(default)]
    pub flame_columns: Vec<[i32; 2]>,
    #[serde(default)]
    pub planter_grown: Vec<[i32; 3]>,
    /// Blocks waiting to catch fire or to burn out (`blaze`). Older worlds have none.
    #[serde(default)]
    pub blazes: Vec<crate::blaze::Blaze>,
    /// Kept hives with bees and the seconds each has foraged (`apiary`). Older worlds
    /// have none: their skeps start over.
    #[serde(default)]
    pub hives: Vec<crate::apiary::SavedHive>,
    /// Loaded home stations: what is on each and the tick it is done (`stations`). Older
    /// worlds have none.
    #[serde(default)]
    pub stations: Vec<crate::stations::SavedLoad>,
    /// What each sprung trap caught (`traps`). Older worlds have none.
    #[serde(default)]
    pub catches: Vec<crate::traps::SavedCatch>,
    /// Fuelled multiblocks and the server tick when their fire goes out. Older worlds
    /// open cold; completed quests stay completed in the player's own file.
    #[serde(default)]
    pub lit: Vec<crate::hearth::Lit>,
    /// Structures smouldering (a charcoal clamp), by the world's day ticks. Older worlds
    /// have none.
    #[serde(default)]
    pub smoulders: Vec<crate::smoulder::Smouldering>,
    /// Whose each home hearth is, lit or cold, until its hearthstone is broken
    /// (`hearth::HearthOwner`). Older worlds have none: their lit hearths keep the owner
    /// saved with the fire.
    #[serde(default)]
    pub hearth_owners: Vec<crate::hearth::HearthOwner>,
    /// The last whole save: files of a later one were cut short and are not used.
    #[serde(default)]
    pub generation: u64,
    /// The fingerprint of the content land was last made from (`WorldContent::fingerprint`):
    /// a world opened with different worldgen data warns that new land may not line up.
    /// `None` until a world is first opened with it.
    #[serde(default)]
    pub worldgen_content: Option<u64>,
    /// Lairs standing empty after their boss fell, until a time of day (`lairs`).
    #[serde(default)]
    pub lairs: Vec<crate::lairs::SavedLair>,
    /// The secret the world's draws hash (a cast's catch), never sent to a player, so no
    /// client can work out what a cast will bring up. 0 in older worlds, which are given
    /// one when they open (`ServerConfig::draw_key`, else a random one).
    #[serde(default)]
    pub draw_key: u64,
    /// Drops popped from broken blocks so far, which numbers where each lands (`popped`);
    /// none in older worlds.
    #[serde(default)]
    pub drops: u64,
    /// Loot rolls drawn for creatures killed so far, which number each; none in older
    /// worlds.
    #[serde(default)]
    pub loot: u64,
    /// Rolls drawn for what no player's draws number (a kept animal's own products, a
    /// litter, each young born, and a player's kind of draw when no player is there to
    /// draw it, `Server::draw_for`), each kind by its salt (`ROLL_PRODUCT` and the rest);
    /// none in older worlds.
    #[serde(default)]
    pub rolls: u64,
    /// Whether the spawn kit (`spawn_kit.ron`) lies by the spawn: laid once, on the first
    /// start of a world or of an older save without it.
    #[serde(default)]
    pub spawn_kit: bool,
}

/// What tells the world's rolls (`WorldMeta::next_roll`) of one kind from another's.
pub const ROLL_PRODUCT: u64 = 0x9a0d;
pub const ROLL_LITTER: u64 = 0x1177;
pub const ROLL_YOUNG: u64 = 0x7a46;
pub const ROLL_GRAZE: u64 = 0x6a2e;
pub const ROLL_SUMMON: u64 = 0x5c3d;
pub const ROLL_WANDER: u64 = 0x3a9e;
pub const ROLL_SPAWN: u64 = 0x5b4a;
pub const ROLL_PARTY: u64 = 0x9a27;
pub const ROLL_FOLLOWER: u64 = 0xf011;
/// Where a creature with a fixed seed (a lair's boss, a settlement's resident) starts its
/// own count of rolls (`Live::rolls`), so each waking draws its thoughts afresh.
pub const ROLL_START: u64 = 0x57a2;

/// The key the world's popped drops scatter by, made from its draw key: where a drop lands
/// and how it flies are seen by every client, and the draw key must not be read back from
/// them. This is a hash, not a proven one-way function. What protects the key is that
/// neither pops nor casts (`cast_key`) run on it or on any stream drawn from it
/// (`numbered_draws`), so even this key won back from many pops foretells only where drops
/// fly; and getting the draw key from this one means solving the XOR of two of `mix`'s
/// passes over different inputs for a 64-bit key, where each pass alone is a bijection that
/// unwinds step by step but their XOR has no known inverse.
pub fn pop_key(draw_key: u64) -> u64 {
    use voxmar_engine::creature::mix;
    mix(draw_key, 0x0909_d50f) ^ mix(draw_key ^ 0x5bd1_e995, 0x2545_f491).rotate_left(29)
}

/// How a burning light set down is saved: where, what by name, and the world's day tick
/// it goes out; or, `tended` in a settlement, the ticks of burn it has left.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedFlame {
    pub pos: [i32; 3],
    pub block: String,
    pub out: u64,
    #[serde(default)]
    pub tended: bool,
}

/// The key the world's casts draw by (`WorldMeta::next_cast`), made from its draw key as
/// `pop_key` is, with passes of its own: a cast's seed becomes a creature's, whose looks
/// every client sees (part of the seed itself), so the draw key must not be searched for
/// from them. Won back, this key foretells only casts.
pub fn cast_key(draw_key: u64) -> u64 {
    use voxmar_engine::creature::mix;
    mix(draw_key, 0x0ca5_7c0d) ^ mix(draw_key ^ 0x2f6b_3c1d, 0x6c8e_9cf5).rotate_left(23)
}

/// How a regrowing plant is saved, by block names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedRegrowing {
    pub pos: [i32; 3],
    pub plant: String,
    pub ground: String,
    pub due: u64,
}

fn normal_speed() -> f32 {
    1.0
}

impl WorldMeta {
    /// The next of the world's loot rolls (a kill nobody made, or by a player not here),
    /// counted.
    pub fn next_loot(&mut self) -> f32 {
        use voxmar_engine::creature::{mix, unit_from};
        self.loot += 1;
        unit_from(mix(mix(self.draw_key ^ 0x1007, self.loot), 0xd2a3))
    }

    /// The next two of the world's rolls of a kind (`salt`, `ROLL_PRODUCT` and the rest),
    /// counted.
    pub fn next_roll(&mut self, salt: u64) -> [f32; 2] {
        let draws = numbered_draws(self.draw_key, salt, self.rolls);
        self.rolls += 1;
        draws
    }

    /// The next two of the world's casts of a kind: rolls whose outcome every client sees
    /// (a creature's seed, its looks), drawn by the cast key (`cast_key`) rather than the
    /// draw key, on the same count as `next_roll`.
    pub fn next_cast(&mut self, salt: u64) -> [f32; 2] {
        let draws = numbered_draws(cast_key(self.draw_key), salt, self.rolls);
        self.rolls += 1;
        draws
    }

    /// A seed made from the world's next cast of a kind (`next_cast`), for what is cast
    /// from it (a spawn's place, a party's cast).
    pub fn roll_seed(&mut self, salt: u64) -> u64 {
        let [a, b] = self.next_cast(salt);
        voxmar_engine::creature::mix(self.seed, u64::from(a.to_bits()) << 32 | u64::from(b.to_bits()))
    }

    /// Where the next drop popped at `cell` flies (`dropped::popped`), by the world's count
    /// of drops and a key that gives nothing of the draw key away (`pop_key`).
    pub fn next_pop(&mut self, cell: voxmar_engine::glam::IVec3, salt: u32) -> [f32; 3] {
        let popped = self.drops;
        self.drops += 1;
        crate::items::random3(pop_key(self.draw_key) ^ popped.wrapping_mul(0x9e37_79b9_7f4a_7c15), cell, salt)
    }

    pub fn new(name: impl Into<String>, seed: u64, mode: GameMode) -> Self {
        let now = unix_time();
        Self {
            name: name.into(),
            seed,
            mode,
            created: now,
            last_played: now,
            generator: Overworld::ID.into(),
            generator_version: Overworld::VERSION,
            time: 0,
            day: None,
            day_speed: 1.0,
            placed: Vec::new(),
            quest_placed: Vec::new(),
            regrowing: Vec::new(),
            flames: Vec::new(),
            flame_columns: Vec::new(),
            planter_grown: Vec::new(),
            blazes: Vec::new(),
            hives: Vec::new(),
            stations: Vec::new(),
            catches: Vec::new(),
            lit: Vec::new(),
            smoulders: Vec::new(),
            hearth_owners: Vec::new(),
            generation: 0,
            worldgen_content: None,
            lairs: Vec::new(),
            draw_key: random_key(),
            drops: 0,
            loot: 0,
            rolls: 0,
            spawn_kit: false,
        }
    }
}

/// A random key, never 0 (which marks a world or player without one).
pub fn random_key() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    loop {
        // The standard library seeds each `RandomState` from the system's randomness.
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(unix_time());
        let key = hasher.finish();
        if key != 0 {
            return key;
        }
    }
}

/// Seconds since the Unix epoch.
pub fn unix_time() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What a player's rolls are numbered by, a count for each kind (`Draw`), saved, so no
/// roll is drawn twice and none hangs on the tick it falls on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draws {
    /// Blows the player has struck at creatures.
    pub strikes: u64,
    /// Creatures' blows that have come down on the player.
    pub blows: u64,
    /// Feeds given to creatures being tamed.
    pub feeds: u64,
    /// Goes cooked at a station.
    pub cooks: u64,
    /// Shots that hit, each of which may be left lying to pick up again.
    pub recovers: u64,
    /// Flourishes landed, for what their effect rounds by chance.
    pub flourishes: u64,
    /// Blocks broken for a reward, for their bonus drop and what they give instead.
    pub mines: u64,
    /// Products taken from kept animals, for how many come.
    pub yields: u64,
    /// Uses of a tool or weapon that might have worn it, for whether skill saved the wear.
    pub wears: u64,
    /// Wild crops taken, for whether one nobody sowed gives anything.
    pub forages: u64,
    /// Harvests rolled (a crop's loot, a skep's), each draw of the roll.
    pub harvests: u64,
    /// Locks tried, for the lucky pick.
    pub locks: u64,
    /// Sprung traps emptied, each draw of the catch's loot roll.
    pub catches: u64,
    /// Chests opened, for what wakes beside them.
    pub wakes: u64,
    /// Wanderers' rewards given, for how many come and which of several.
    pub rewards: u64,
    /// Captives freed, for whether one follows its freer.
    pub frees: u64,
    /// Kills made, each draw of the kill's loot roll.
    pub kills: u64,
    /// Wanderers' greetings heard, for which of their greetings is said.
    pub greets: u64,
    /// Shots that missed, each of which may land to pick up again (`recover_miss`).
    pub strays: u64,
}

/// A kind of roll a player's draws number (`Draws`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    Strike,
    Blow,
    Feed,
    Cook,
    Recover,
    Flourish,
    Mine,
    Yield,
    Wear,
    Forage,
    Harvest,
    Lock,
    Catch,
    Wake,
    Reward,
    Free,
    Kill,
    Greet,
    Stray,
}

impl Draw {
    /// What tells this kind's draws from another's at the same count.
    pub fn salt(self) -> u64 {
        match self {
            Draw::Strike => 0x5717,
            Draw::Blow => 0x0b10,
            Draw::Feed => 0x7a3e,
            Draw::Cook => 0xc00c,
            Draw::Recover => 0xa770,
            Draw::Flourish => 0xf10,
            Draw::Mine => 0xd1ce,
            Draw::Yield => 0x7ee1,
            Draw::Wear => 0x3ea2,
            Draw::Forage => 0x5eed,
            Draw::Harvest => 0x4a27,
            Draw::Lock => 0x1ac7,
            Draw::Catch => 0xca7c,
            Draw::Wake => 0xa4e1,
            Draw::Reward => 0x7e3a,
            Draw::Free => 0xf4ee,
            Draw::Kill => 0xb10d,
            Draw::Greet => 0x6e37,
            Draw::Stray => 0x57a7,
        }
    }
}

impl Draws {
    /// The next two draws of `kind` for the player whose draw key is `key`, counted.
    pub fn next(&mut self, key: u64, kind: Draw) -> [f32; 2] {
        let count = match kind {
            Draw::Strike => &mut self.strikes,
            Draw::Blow => &mut self.blows,
            Draw::Feed => &mut self.feeds,
            Draw::Cook => &mut self.cooks,
            Draw::Recover => &mut self.recovers,
            Draw::Flourish => &mut self.flourishes,
            Draw::Mine => &mut self.mines,
            Draw::Yield => &mut self.yields,
            Draw::Wear => &mut self.wears,
            Draw::Forage => &mut self.forages,
            Draw::Harvest => &mut self.harvests,
            Draw::Lock => &mut self.locks,
            Draw::Catch => &mut self.catches,
            Draw::Wake => &mut self.wakes,
            Draw::Reward => &mut self.rewards,
            Draw::Free => &mut self.frees,
            Draw::Kill => &mut self.kills,
            Draw::Greet => &mut self.greets,
            Draw::Stray => &mut self.strays,
        };
        let draws = numbered_draws(key, kind.salt(), *count);
        *count += 1;
        draws
    }
}

/// Two draws for the `count`th roll of a kind (`salt`) by the player whose draw key is
/// `key`: each count its own, whatever tick it falls on.
pub fn numbered_draws(key: u64, salt: u64, count: u64) -> [f32; 2] {
    use voxmar_engine::creature::{mix, unit_from};
    let h = mix(mix(key ^ salt, count), 0xd2a3);
    [unit_from(h), unit_from(mix(h, 1))]
}

/// What is kept of a player between sessions.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedPlayer {
    pub position: Vec3,
    pub yaw: f32,
    /// The player's own game mode, once an admin has changed it; otherwise the world's.
    pub mode: Option<GameMode>,
    pub flying: bool,
    pub health: Health,
    pub inventory: Inventory,
    /// Experience in tenths, by skill name.
    pub skills: Vec<(String, u32)>,
    /// Effects food gave, by name, with seconds left.
    pub effects: Vec<(String, f32)>,
    /// Where the player wakes after dying, if a hearth set it, and the bed they last
    /// slept in.
    pub home: Option<[f32; 3]>,
    pub bed: Option<[f32; 3]>,
    /// Whether their home hearth has been kept lit since hearths kept their fire; a save
    /// from before has its standing hearth lit again once (`hearth::kindle_home`).
    pub kindled: bool,
    /// Everything they have discovered.
    pub journal: Vec<voxmar_engine::journal::Entry>,
    /// Quests finished, those of them whose rewards were claimed, and the tallies quests
    /// count.
    pub quests: Vec<String>,
    pub quests_claimed: Vec<String>,
    pub tallies: Vec<(String, u32)>,
    /// Prayer points left; none in older saves, which come back full.
    pub prayer_points: Option<f32>,
    /// Lines cast so far (none in older saves).
    pub casts: u64,
    /// What the player's other rolls are numbered by (none in older saves).
    pub draws: Draws,
    /// The home steps their home last met, by name, and where that home stood, to tell
    /// them on return what lights burnt out while they were away cost (none in older saves).
    pub home_steps: Vec<String>,
    pub home_anchor: Option<[i32; 3]>,
    /// What an older save carried in stacks that now come one at a time (tools), with no
    /// empty slot left for it: dropped at the player's feet when they join. Never saved.
    pub spilled: Vec<ItemStack>,
}

/// Tallies quests count (`kind:name`, or `kind:station:name`) under the names items have
/// now: what an older save counted as cooked rabbit counts as cooked small game, added to
/// any it already had, so a quest half done keeps its progress.
fn renamed_tallies(tallies: Vec<(String, u32)>, registry: &BlockRegistry) -> Vec<(String, u32)> {
    let mut merged: Vec<(String, u32)> = Vec::with_capacity(tallies.len());
    for (key, count) in tallies {
        // The item's name is the key's last two parts (`base:cooked_rabbit`).
        let name_at = key.rmatch_indices(':').nth(1).map_or(0, |(i, _)| i + 1);
        let (prefix, name) = key.split_at(name_at);
        let key = match registry.id(name) {
            None if !prefix.is_empty() => registry.saved_id(name).map_or(key.clone(), |id| format!("{prefix}{}", registry.def(id).name)),
            _ => key.clone(),
        };
        match merged.iter_mut().find(|(have, _)| *have == key) {
            Some((_, n)) => *n = n.saturating_add(count),
            None => merged.push((key, count)),
        }
    }
    merged
}

/// A player file. Items are stored by name, like column blocks.
#[derive(Serialize, Deserialize)]
struct PlayerFile {
    /// The name as the player first played it (the file is named in lower case), which they
    /// keep playing under whatever case they type later.
    #[serde(default)]
    name: Option<String>,
    position: [f32; 3],
    yaw: f32,
    #[serde(default)]
    mode: Option<GameMode>,
    flying: bool,
    health: Health,
    inventory: Vec<Option<(String, u8)>>,
    /// How worn the tools in the inventory are, by slot (only those worn at all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    wear: Vec<(u8, u16)>,
    /// What is worn, slot by slot (`inventory::WORN`).
    #[serde(default)]
    worn: Vec<Option<String>>,
    #[serde(default)]
    skills: Vec<(String, u32)>,
    #[serde(default)]
    effects: Vec<(String, f32)>,
    #[serde(default)]
    home: Option<[f32; 3]>,
    #[serde(default)]
    bed: Option<[f32; 3]>,
    #[serde(default)]
    kindled: bool,
    /// What pouches hold, by item name.
    #[serde(default)]
    pouched: Vec<(String, u16)>,
    #[serde(default)]
    journal: Vec<voxmar_engine::journal::Entry>,
    #[serde(default)]
    quests: Vec<String>,
    /// Absent in saves from before rewards were claimed in the quest screen, whose finished
    /// quests were all paid when they finished.
    #[serde(default)]
    quests_claimed: Option<Vec<String>>,
    #[serde(default)]
    tallies: Vec<(String, u32)>,
    #[serde(default)]
    prayer_points: Option<f32>,
    #[serde(default)]
    casts: u64,
    #[serde(default)]
    draws: Draws,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    home_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    home_anchor: Option<[i32; 3]>,
}

/// A column file, generic so that saving can borrow the column.
#[derive(Serialize, Deserialize)]
struct ColumnFile<C> {
    /// Name of every block id the column's chunks refer to.
    blocks: Vec<(u16, String)>,
    column: C,
}

/// A save being written: its generation and the files it has staged.
type Staging = (u64, Vec<PathBuf>);

/// A world directory. Cheap to clone, so worker threads can load columns.
#[derive(Clone, Debug)]
pub struct WorldSave {
    dir: Arc<Path>,
    /// The save being written, if one is.
    staging: Arc<Mutex<Option<Staging>>>,
}

impl WorldSave {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into().into(), staging: Arc::default() }
    }

    /// Starts a save of `generation`: until [`WorldSave::commit`], files are written beside
    /// their own and the world as last saved stays as it was.
    pub fn begin(&self, generation: u64) {
        *self.staging.lock().expect("save staging") = Some((generation, Vec::new()));
    }

    /// Ends the save begun: `meta`, carrying its generation, is written, and with that the
    /// save counts (an error: it does not); then its files take their places. Returns
    /// whether all did (any left are moved when the world next opens).
    pub fn commit(&self, meta: &WorldMeta) -> io::Result<bool> {
        let staged = self.staging.lock().expect("save staging").take();
        if let Err(e) = self.write_meta(meta) {
            if let Some((generation, files)) = &staged {
                discard(*generation, files);
            }
            return Err(e);
        }
        let Some((generation, files)) = staged else { return Ok(true) };
        let mut moved = true;
        for file in files {
            if let Err(e) = std::fs::rename(staged_path(&file, generation), &file) {
                tracing::error!("could not move the saved {} into place: {e}", file.display());
                moved = false;
            }
        }
        Ok(moved)
    }

    /// Gives up the save begun, as a failed write must: what it staged is deleted and the
    /// world stays as last saved, so nothing held is ever both there and in a newer file.
    pub fn abandon(&self) {
        if let Some((generation, files)) = self.staging.lock().expect("save staging").take() {
            discard(generation, &files);
        }
    }

    /// Where a file of the save being written goes: beside its own while one is begun.
    fn target(&self, path: PathBuf) -> PathBuf {
        let mut staging = self.staging.lock().expect("save staging");
        match staging.as_mut() {
            Some((generation, files)) => {
                let staged = staged_path(&path, *generation);
                files.push(path);
                staged
            }
            None => path,
        }
    }

    /// Finishes or forgets a save cut short: files of the generation `meta` counts are moved
    /// into place (it was cut while moving them), those of a later one deleted (it was cut
    /// before it counted). Run before anything is read.
    pub fn recover(&self, meta: &WorldMeta) {
        for dir in [self.dir.to_path_buf(), self.dir.join("players"), self.dir.join("columns")] {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some((file, generation)) = staged_of(&path) else { continue };
                let result = if generation == meta.generation {
                    tracing::warn!("finishing a save cut short: {}", file.display());
                    std::fs::rename(&path, &file)
                } else {
                    tracing::warn!("forgetting a save cut short: {}", path.display());
                    std::fs::remove_file(&path)
                };
                if let Err(e) = result {
                    tracing::error!("could not recover {}: {e}", path.display());
                }
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the directory holds a world.
    pub fn exists(&self) -> bool {
        self.dir.join(META_FILE).is_file()
    }

    /// Creates the directory and writes `world.ron`. Fails if a world is already there.
    pub fn create(&self, meta: &WorldMeta) -> io::Result<()> {
        if self.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already holds a world", self.dir.display())));
        }
        std::fs::create_dir_all(&self.dir)?;
        self.write_meta(meta)
    }

    pub fn read_meta(&self) -> io::Result<WorldMeta> {
        let path = self.dir.join(META_FILE);
        let source = std::fs::read_to_string(&path)?;
        ron::from_str(&source).map_err(|e| invalid(&path, e))
    }

    pub fn write_meta(&self, meta: &WorldMeta) -> io::Result<()> {
        let text = ron::ser::to_string_pretty(meta, ron::ser::PrettyConfig::default()).map_err(io::Error::other)?;
        write_atomic(&self.dir.join(META_FILE), text.as_bytes())
    }

    fn column_path(&self, pos: ColumnPos) -> PathBuf {
        self.dir.join("columns").join(format!("{}.{}.bin", pos.0.x, pos.0.y))
    }

    /// Whether the column at `pos` was ever saved.
    pub fn has_column(&self, pos: ColumnPos) -> bool {
        self.column_path(pos).is_file()
    }

    /// The saved column at `pos`, if it was ever saved. Blocks are mapped to the current
    /// ids by name; blocks that no longer exist become air.
    pub fn read_column(&self, pos: ColumnPos, registry: &BlockRegistry) -> io::Result<Option<Column>> {
        let path = self.column_path(pos);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(compressed) = bytes.strip_prefix(COLUMN_MAGIC) else {
            return Err(invalid(&path, "not a column file"));
        };
        let mut encoded = Vec::new();
        ZlibDecoder::new(compressed).read_to_end(&mut encoded).map_err(|e| invalid(&path, e))?;
        let file: ColumnFile<Column> = postcard::from_bytes(&encoded).map_err(|e| invalid(&path, e))?;
        let mut column = file.column;
        if column.colors.len() != COLUMN_AREA || !column.chunks.iter().all(Chunk::is_valid) {
            return Err(invalid(&path, "inconsistent column data"));
        }

        let ids: FxHashMap<u16, BlockId> = file
            .blocks
            .iter()
            .map(|(id, name)| {
                let current = registry.saved_id(name).unwrap_or_else(|| {
                    tracing::warn!("{}: unknown block `{name}` becomes air", path.display());
                    BlockId::AIR
                });
                (*id, current)
            })
            .collect();
        let map = |block: BlockId| ids.get(&block.0).copied().unwrap_or(BlockId::AIR);
        for chunk in &mut column.chunks {
            if chunk.palette().iter().any(|&block| map(block) != block) {
                let mut blocks = vec![BlockId::AIR; CHUNK_VOLUME];
                chunk.copy_to_dense(&mut blocks);
                blocks.iter_mut().for_each(|block| *block = map(*block));
                *chunk = Chunk::from_dense(&blocks);
            }
        }
        Ok(Some(column))
    }

    pub fn write_column(&self, pos: ColumnPos, column: &Column, registry: &BlockRegistry) -> io::Result<()> {
        let mut ids: Vec<BlockId> = column.chunks.iter().flat_map(|c| c.palette().iter().copied()).collect();
        ids.sort_unstable();
        ids.dedup();
        let blocks = ids.into_iter().map(|id| (id.0, registry.def(id).name.clone())).collect();
        let file = ColumnFile { blocks, column };
        let encoded = postcard::to_stdvec(&file).map_err(io::Error::other)?;
        let mut encoder = ZlibEncoder::new(COLUMN_MAGIC.to_vec(), Compression::fast());
        encoder.write_all(&encoded)?;
        let path = self.column_path(pos);
        std::fs::create_dir_all(path.parent().expect("column files are in a directory"))?;
        write_atomic(&self.target(path), &encoder.finish()?)
    }

    /// Where the player called `name` is saved: by the name in lower case, since names
    /// differing only in case are one player (and one file on macOS and Windows).
    fn player_path(&self, name: &str) -> PathBuf {
        self.dir.join("players").join(format!("{}.ron", file_name(&name.to_lowercase())))
    }

    /// The file the player called `name` is read from: theirs, or one saved before names
    /// were kept in lower case, under the name as some player typed it, in any case
    /// (written back to theirs at the next save).
    fn player_file(&self, name: &str) -> PathBuf {
        let path = self.player_path(name);
        if path.exists() {
            return path;
        }
        // Compared by the names they hold, so a name escaped in the file (`Ü` as `~dc~`)
        // matches in any case too.
        let wanted = name.to_lowercase();
        let older = std::fs::read_dir(self.dir.join("players"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|older| older.extension().is_some_and(|e| e == "ron"))
            .filter(|older| older.file_stem().and_then(|n| player_name(&n.to_string_lossy())).is_some_and(|n| n.to_lowercase() == wanted))
            .min();
        older.unwrap_or(path)
    }

    /// The name the player called `name` is saved under, in the case they first played it,
    /// if they have been here and the file says (older files do not).
    pub fn saved_name(&self, name: &str) -> Option<String> {
        #[derive(Deserialize)]
        struct Named {
            #[serde(default)]
            name: Option<String>,
        }
        let source = std::fs::read_to_string(self.player_file(name)).ok()?;
        ron::from_str::<Named>(&source).ok()?.name.filter(|saved| saved.to_lowercase() == name.to_lowercase())
    }

    /// Spellings preserved by actual older file names, before player files were named in
    /// lower case. Read directory entries: a synthesised path loses the case on macOS.
    pub(crate) fn older_player_names(&self, name: &str) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(self.dir.join("players"))
            .into_iter().flatten().flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "ron"))
            .filter_map(|path| path.file_stem().and_then(|stem| player_name(&stem.to_string_lossy())))
            .filter(|saved| crate::player::same_name(saved, name))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The saved state of the player called `name`. Items that no longer exist are dropped.
    pub fn read_player(&self, name: &str, registry: &BlockRegistry) -> io::Result<Option<SavedPlayer>> {
        let path = self.player_file(name);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let file: PlayerFile = ron::from_str(&source).map_err(|e| invalid(&path, e))?;
        let mut inventory = Inventory::default();
        for (slot, stack) in inventory.slots.iter_mut().zip(file.inventory) {
            *slot = stack
                .filter(|&(_, count)| count > 0)
                .and_then(|(item, count)| Some(ItemStack::new(registry.item_named(&item)?, count.min(MAX_STACK))));
        }
        for &(slot, wear) in &file.wear {
            // (A stack of lights picked up part-burnt keeps its wear too.)
            if let Some(stack) = inventory.slots.get_mut(slot as usize).and_then(Option::as_mut) {
                stack.wear = wear;
            }
        }
        // Tools stacked before they wore come apart, one to a slot.
        let spilled = inventory.split_overfull();
        // Older saves' items that merged into one add up.
        for (item, count) in file.pouched.iter().filter_map(|(item, count)| Some((registry.item_named(item)?, *count))).filter(|&(_, n)| n > 0) {
            match inventory.pouched.iter_mut().find(|(have, _)| *have == item) {
                Some((_, n)) => *n = n.saturating_add(count),
                None => inventory.pouched.push((item, count)),
            }
        }
        for (slot, item) in inventory.worn.iter_mut().zip(file.worn) {
            *slot = item.and_then(|item| Some(ItemStack::new(registry.item_named(&item)?, 1)));
        }
        let position = Vec3::from(file.position);
        if !position.is_finite() {
            return Err(invalid(&path, "position is not finite"));
        }
        let health = Health { current: file.health.current.min(file.health.max), max: file.health.max };
        let yaw = if file.yaw.is_finite() { file.yaw } else { 0.0 };
        Ok(Some(SavedPlayer { position, yaw, mode: file.mode, flying: file.flying, health, inventory, skills: file.skills, effects: file.effects, home: file.home, bed: file.bed, kindled: file.kindled, journal: merge_journal(file.journal, registry), quests_claimed: file.quests_claimed.unwrap_or_else(|| file.quests.clone()), quests: file.quests, tallies: renamed_tallies(file.tallies, registry), prayer_points: file.prayer_points, casts: file.casts, draws: file.draws, home_steps: file.home_steps, home_anchor: file.home_anchor, spilled }))
    }

    /// Moves the file of the player called `name` aside, as unreadable (`set_aside`).
    pub fn set_aside_player(&self, name: &str) -> io::Result<PathBuf> {
        set_aside(&self.player_file(name))
    }

    /// Moves the column file at `pos` aside, as unreadable (`set_aside`).
    pub fn set_aside_column(&self, pos: ColumnPos) -> io::Result<PathBuf> {
        set_aside(&self.column_path(pos))
    }

    pub fn write_player(&self, name: &str, player: &SavedPlayer, registry: &BlockRegistry) -> io::Result<()> {
        // A stack held on the pointer is saved back in the inventory (only a full one
        // loses it).
        let mut held = player.inventory.clone();
        held.stow_cursor();
        let inventory = held
            .slots
            .iter()
            .map(|slot| slot.map(|stack| (registry.def(stack.item).name.clone(), stack.count)))
            .collect();
        let file = PlayerFile {
            name: Some(name.to_owned()),
            position: player.position.to_array(),
            yaw: player.yaw,
            mode: player.mode,
            flying: player.flying,
            health: player.health,
            inventory,
            wear: held.slots.iter().enumerate().filter_map(|(slot, stack)| stack.filter(|s| s.wear > 0).map(|s| (slot as u8, s.wear))).collect(),
            worn: held.worn.iter().map(|slot| slot.map(|stack| registry.def(stack.item).name.clone())).collect(),
            skills: player.skills.clone(),
            effects: player.effects.clone(),
            home: player.home,
            bed: player.bed,
            kindled: player.kindled,
            pouched: held.pouched.iter().map(|&(item, count)| (registry.def(item).name.clone(), count)).collect(),
            journal: player.journal.clone(),
            quests: player.quests.clone(),
            quests_claimed: Some(player.quests_claimed.clone()),
            tallies: player.tallies.clone(),
            prayer_points: player.prayer_points,
            casts: player.casts,
            draws: player.draws,
            home_steps: player.home_steps.clone(),
            home_anchor: player.home_anchor,
        };
        let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default()).map_err(io::Error::other)?;
        let path = self.player_path(name);
        std::fs::create_dir_all(path.parent().expect("player files are in a directory"))?;
        write_atomic(&self.target(path), text.as_bytes())
    }
}

/// A journal as an older save kept it, with entries for plants that have since merged into
/// one item folded into that item's: the earliest finding, the furthest stage, and the picks
/// of all added up.
fn merge_journal(entries: Vec<voxmar_engine::journal::Entry>, registry: &BlockRegistry) -> Vec<voxmar_engine::journal::Entry> {
    let mut merged: Vec<voxmar_engine::journal::Entry> = Vec::with_capacity(entries.len());
    for mut entry in entries {
        if entry.page == voxmar_engine::journal::Page::Plants
            && let Some(item) = registry.item_named(&entry.name)
        {
            entry.name = registry.def(item).name.clone();
        }
        // A fish whose item was renamed (shrimp, now minnows) counts as the new one.
        if entry.page == voxmar_engine::journal::Page::Fish
            && registry.id(&entry.name).is_none()
            && let Some(item) = registry.saved_id(&entry.name)
        {
            entry.name = registry.def(item).name.clone();
        }
        match merged.iter_mut().find(|have| have.page == entry.page && have.name == entry.name) {
            Some(have) => {
                have.stage = have.stage.max(entry.stage);
                have.count = have.count.saturating_add(entry.count);
                have.found = have.found.min(entry.found);
            }
            None => merged.push(entry),
        }
    }
    merged
}

/// A dropped item as a save keeps it: by item name, so it survives content changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedItem {
    pub item: String,
    pub count: u8,
    pub position: [f32; 3],
    /// Seconds it has lain.
    pub age: f32,
    /// Whose alone it is (a hash of their name), for what a player left where they died.
    #[serde(default)]
    pub owner: Option<u64>,
    /// Seconds it lasts, if not the usual.
    #[serde(default)]
    pub lifetime: Option<f32>,
    /// Uses worn off it, a tool's (`ItemStack::wear`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wear: u16,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

/// A planted crop as a save keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCrop {
    pub position: [i32; 3],
    pub crop: String,
    pub stage: u8,
    /// World tick its next stage is due (`u64::MAX` once ripe).
    pub due: u64,
    /// A player sowed it. Saves from before the flag held only sown crops.
    #[serde(default = "sown_before")]
    pub sown: bool,
}

fn sown_before() -> bool {
    true
}

/// One of a settlement's people who died, as saved: settlement `town`'s `person`th
/// resident, `generation` newcomers on, the next due on day `away_until` (days since the
/// world began).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedDead {
    pub town: u64,
    pub person: usize,
    pub generation: u64,
    pub away_until: f64,
}

impl WorldSave {
    /// The planted crops, none if the world has no crops file.
    pub fn read_crops(&self) -> io::Result<Vec<SavedCrop>> {
        let path = self.dir.join(CROPS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => ron::from_str(&source).map_err(|e| invalid(&path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    pub fn write_crops(&self, crops: &[SavedCrop]) -> io::Result<()> {
        let text = ron::ser::to_string(crops).map_err(io::Error::other)?;
        write_atomic(&self.target(self.dir.join(CROPS_FILE)), text.as_bytes())
    }

    /// The machines, none if the world has no machines file.
    pub fn read_machines(&self) -> io::Result<Vec<crate::machines::SavedMachine>> {
        let path = self.dir.join(MACHINES_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => ron::from_str(&source).map_err(|e| invalid(&path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    pub fn write_machines(&self, machines: &[crate::machines::SavedMachine]) -> io::Result<()> {
        let text = ron::ser::to_string(machines).map_err(io::Error::other)?;
        write_atomic(&self.target(self.dir.join(MACHINES_FILE)), text.as_bytes())
    }

    /// The dropped items lying about, none if the world has no items file.
    pub fn read_items(&self) -> io::Result<Vec<SavedItem>> {
        let path = self.dir.join(ITEMS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => ron::from_str(&source).map_err(|e| invalid(&path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// The kept creatures, none if the world has no creatures file.
    pub fn read_creatures(&self) -> io::Result<Vec<crate::husbandry::SavedCreature>> {
        let path = self.dir.join(CREATURES_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => ron::from_str(&source).map_err(|e| invalid(&path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    pub fn write_creatures(&self, creatures: &[crate::husbandry::SavedCreature]) -> io::Result<()> {
        let text = ron::ser::to_string(creatures).map_err(io::Error::other)?;
        write_atomic(&self.target(self.dir.join(CREATURES_FILE)), text.as_bytes())
    }

    /// Who of the settlements' people died, none if the world has no villagers file.
    pub fn read_villagers(&self) -> io::Result<Vec<SavedDead>> {
        let path = self.dir.join(VILLAGERS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => ron::from_str(&source).map_err(|e| invalid(&path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// The wanderers settled at players' homes, none if the world has no companions file.
    pub fn read_companions(&self) -> io::Result<Vec<crate::wandering::SavedCompanion>> {
        let path = self.dir.join(COMPANIONS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => ron::from_str(&source).map_err(|e| invalid(&path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    pub fn write_companions(&self, companions: &[crate::wandering::SavedCompanion]) -> io::Result<()> {
        let text = ron::ser::to_string(companions).map_err(io::Error::other)?;
        write_atomic(&self.target(self.dir.join(COMPANIONS_FILE)), text.as_bytes())
    }

    pub fn write_villagers(&self, dead: &[SavedDead]) -> io::Result<()> {
        let text = ron::ser::to_string(dead).map_err(io::Error::other)?;
        write_atomic(&self.target(self.dir.join(VILLAGERS_FILE)), text.as_bytes())
    }

    pub fn write_items(&self, items: &[SavedItem]) -> io::Result<()> {
        let text = ron::ser::to_string(items).map_err(io::Error::other)?;
        write_atomic(&self.target(self.dir.join(ITEMS_FILE)), text.as_bytes())
    }
}

/// What a world keeps besides its columns and players, as read when it opens.
#[derive(Debug, Default)]
pub(crate) struct WorldState {
    pub items: Vec<SavedItem>,
    pub crops: Vec<SavedCrop>,
    pub creatures: Vec<crate::husbandry::SavedCreature>,
    pub machines: Vec<crate::machines::SavedMachine>,
    pub villagers: Vec<SavedDead>,
    pub companions: Vec<crate::wandering::SavedCompanion>,
}

impl WorldSave {
    /// The dropped items, crops, kept creatures, machines, who of the villagers died and the settled companions. A file that is not what it should be is
    /// moved aside and logged, and the world opens without what it held; a file that could
    /// not be read at all, or moved aside (and so would be overwritten at the next save), is
    /// an error.
    pub(crate) fn read_state(&self) -> io::Result<WorldState> {
        Ok(WorldState {
            items: self.read_or_set_aside(ITEMS_FILE, Self::read_items)?,
            crops: self.read_or_set_aside(CROPS_FILE, Self::read_crops)?,
            creatures: self.read_or_set_aside(CREATURES_FILE, Self::read_creatures)?,
            machines: self.read_or_set_aside(MACHINES_FILE, Self::read_machines)?,
            villagers: self.read_or_set_aside(VILLAGERS_FILE, Self::read_villagers)?,
            companions: self.read_or_set_aside(COMPANIONS_FILE, Self::read_companions)?,
        })
    }

    fn read_or_set_aside<T: Default>(&self, file: &str, read: fn(&Self) -> io::Result<T>) -> io::Result<T> {
        let error = match read(self) {
            Ok(value) => return Ok(value),
            // Not read at all (no permission, a failing disk): the world does not open.
            Err(error) if error.kind() != io::ErrorKind::InvalidData => return Err(error),
            Err(error) => error,
        };
        let path = self.dir.join(file);
        match set_aside(&path) {
            Ok(aside) => {
                tracing::error!("could not read {}: {error}; kept it as {} and opened the world without it", path.display(), aside.display());
                Ok(T::default())
            }
            Err(moving) => Err(io::Error::new(error.kind(), format!("could not read {}: {error}; nor move it aside: {moving}", path.display()))),
        }
    }
}

/// Moves an unreadable save file aside, to `<file>.corrupt-<unix time>` beside it, so the
/// next save does not replace it and it can be looked at or restored by hand. Returns
/// where it went.
pub fn set_aside(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stamp = unix_time();
    for n in 0..1000 {
        let suffix = if n == 0 { String::new() } else { format!("-{n}") };
        let aside = path.with_file_name(format!("{name}.corrupt-{stamp}{suffix}"));
        if !aside.exists() {
            std::fs::rename(path, &aside)?;
            return Ok(aside);
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} has been set aside too often", path.display())))
}

/// Every world in `root`, most recently played first. Directories without a readable
/// `world.ron` are skipped.
pub fn list_worlds(root: &Path) -> Vec<(WorldSave, WorldMeta)> {
    let mut worlds: Vec<(WorldSave, WorldMeta)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| WorldSave::new(entry.path()))
        .filter(WorldSave::exists)
        .filter_map(|save| match save.read_meta() {
            Ok(meta) => Some((save, meta)),
            Err(e) => {
                tracing::warn!("skipping world in {}: {e}", save.dir().display());
                None
            }
        })
        .collect();
    worlds.sort_by(|a, b| b.1.last_played.cmp(&a.1.last_played).then_with(|| a.1.name.cmp(&b.1.name)));
    worlds
}

/// Where `path` is written while a save of `generation` is staged.
fn staged_path(path: &Path, generation: u64) -> PathBuf {
    let mut name = path.file_name().expect("saved files have names").to_os_string();
    name.push(format!(".{generation}.{STAGED}"));
    path.with_file_name(name)
}

/// Deletes the files a save of `generation` staged for `files`. One left behind is deleted
/// when the world next opens, as that generation never counted.
fn discard(generation: u64, files: &[PathBuf]) {
    for file in files {
        let staged = staged_path(file, generation);
        if let Err(e) = std::fs::remove_file(&staged)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::error!("could not delete the unsaved {}: {e}", staged.display());
        }
    }
}

/// The file a staged one is for, and its generation.
fn staged_of(path: &Path) -> Option<(PathBuf, u64)> {
    let name = path.file_name()?.to_str()?;
    let (rest, generation) = name.strip_suffix(&format!(".{STAGED}"))?.rsplit_once('.')?;
    Some((path.with_file_name(rest), generation.parse().ok()?))
}

/// Writes a file through a temporary file and a rename, so readers and crashes only ever
/// see the old or the new contents.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// A file name for any player name: letters, digits, `-` and `_` are kept and everything
/// else is written as `~hex~`, so distinct names never share a file.
fn file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push_str(&format!("~{:x}~", c as u32));
        }
    }
    out
}

/// The player name a `file_name` holds, or `None` if it is not one.
fn player_name(file: &str) -> Option<String> {
    let mut out = String::with_capacity(file.len());
    let mut rest = file;
    while let Some(c) = rest.chars().next() {
        if c == '~' {
            let (hex, after) = rest[1..].split_once('~')?;
            out.push(char::from_u32(u32::from_str_radix(hex, 16).ok()?)?);
            rest = after;
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    Some(out)
}

fn invalid(path: &Path, error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::glam::IVec3;
    use voxmar_worldgen::Generator;

    fn registry(extra_first: bool) -> Arc<BlockRegistry> {
        // Every base block file, as the game loads them: worldgen places blocks from several.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with("blocks")).collect();
        files.sort();
        let packages = files.iter().map(|f| ("base".to_owned(), BlockRegistry::read("base", &std::fs::read_to_string(f).unwrap()).unwrap())).collect();
        let mut defs = BlockRegistry::expand(packages).unwrap();
        if extra_first {
            // A mod loaded before `base` renumbers every base block.
            defs.insert(0, voxmar_engine::block::BlockDef { name: "aaa:marble".into(), ..defs[0].clone() });
        }
        Arc::new(BlockRegistry::new(defs).unwrap())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxmar-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A save is one generation: cut before `world.ron` takes it, none of it counts; cut
    /// while its files are moved into place, the next opening finishes it.
    #[test]
    fn a_save_cut_short_counts_whole_or_not_at_all() {
        let dir = temp_dir("generations");
        let save = WorldSave::new(&dir);
        let mut meta = WorldMeta::new("Test", 7, GameMode::Creative);
        save.create(&meta).unwrap();
        let crops = |stage: u8| vec![SavedCrop { position: [1, 2, 3], crop: "base:wheat".into(), stage, due: 9, sown: true }];
        let machines = |count: u8| vec![crate::machines::SavedMachine { position: [0, 64, 0], block: "base:chest".into(), inputs: Vec::new(), outputs: Vec::new(), fuel: None, burning: 0.0, energy: 0.0, condition: 0.0, made: 0, bin: vec![("base:iron_ore".into(), count)], bin_wear: Vec::new(), owner: None, counted: false, work: None }];
        // Generation 1, whole.
        save.begin(1);
        save.write_machines(&machines(0)).unwrap();
        save.write_crops(&crops(0)).unwrap();
        meta.generation = 1;
        assert!(save.commit(&meta).unwrap());
        // Generation 2 cut after its machines: the machines of 1, and nothing left over.
        save.begin(2);
        save.write_machines(&machines(64)).unwrap();
        let meta = save.read_meta().unwrap();
        assert_eq!(meta.generation, 1);
        save.recover(&meta);
        assert_eq!(save.read_machines().unwrap(), machines(0));
        assert_eq!(save.read_crops().unwrap(), crops(0));
        assert!(!staged_path(&dir.join(MACHINES_FILE), 2).exists(), "the cut save is forgotten");
        // Generation 2 again, cut once `world.ron` took it but before its files moved: the
        // next opening moves them.
        save.begin(2);
        save.write_machines(&machines(64)).unwrap();
        save.write_crops(&crops(1)).unwrap();
        let mut taken = save.read_meta().unwrap();
        taken.generation = 2;
        save.write_meta(&taken).unwrap();
        save.recover(&save.read_meta().unwrap());
        assert_eq!(save.read_machines().unwrap(), machines(64));
        assert_eq!(save.read_crops().unwrap(), crops(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A save with a part that could not be written never counts, and leaves nothing
    /// staged: what moved since the last save is never in two places.
    #[test]
    fn a_save_with_a_failed_write_or_meta_leaves_the_last_one_whole() {
        let dir = temp_dir("failed");
        let save = WorldSave::new(&dir);
        let mut meta = WorldMeta::new("Test", 7, GameMode::Creative);
        save.create(&meta).unwrap();
        let crops = |stage: u8| vec![SavedCrop { position: [1, 2, 3], crop: "base:wheat".into(), stage, due: 9, sown: true }];
        save.begin(1);
        save.write_crops(&crops(0)).unwrap();
        meta.generation = 1;
        assert!(save.commit(&meta).unwrap());
        // Generation 2: the crops write, the machines cannot (their place is a directory).
        let blocked = staged_path(&dir.join(MACHINES_FILE), 2);
        std::fs::create_dir_all(&blocked).unwrap();
        save.begin(2);
        save.write_crops(&crops(1)).unwrap();
        assert!(save.write_machines(&[]).is_err());
        save.abandon();
        assert_eq!(save.read_meta().unwrap().generation, 1);
        assert!(!staged_path(&dir.join(CROPS_FILE), 2).exists(), "nothing of it is left staged");
        assert_eq!(save.read_crops().unwrap(), crops(0));
        std::fs::remove_dir_all(&blocked).unwrap();
        // Generation 3: everything writes, but `world.ron` cannot: it does not count either.
        let meta_blocked = dir.join(META_FILE).with_extension("tmp");
        std::fs::create_dir_all(&meta_blocked).unwrap();
        save.begin(3);
        save.write_crops(&crops(2)).unwrap();
        save.write_machines(&[]).unwrap();
        meta.generation = 3;
        assert!(save.commit(&meta).is_err());
        std::fs::remove_dir_all(&meta_blocked).unwrap();
        assert_eq!(save.read_meta().unwrap().generation, 1);
        assert!(!staged_path(&dir.join(CROPS_FILE), 3).exists() && !staged_path(&dir.join(MACHINES_FILE), 3).exists(), "nothing of it is left staged");
        assert_eq!(save.read_crops().unwrap(), crops(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn columns_round_trip_and_survive_renumbered_blocks() {
        let dir = temp_dir("columns");
        let save = WorldSave::new(&dir);
        save.create(&WorldMeta::new("Test", 7, GameMode::Creative)).unwrap();
        assert!(save.create(&WorldMeta::new("Again", 8, GameMode::Survival)).is_err(), "never overwrite a world");
        assert_eq!(save.read_meta().unwrap().seed, 7);

        let old = registry(false);
        let pos = ColumnPos::new(-3, 5);
        let mut column = Overworld::new(7, old.clone()).generate(pos);
        let glass = old.expect("base:glass");
        column.set_block(IVec3::new(4, 100, 9), glass);
        save.write_column(pos, &column, &old).unwrap();
        assert_eq!(save.read_column(pos, &old).unwrap(), Some(column.clone()));
        assert_eq!(save.read_column(ColumnPos::new(0, 0), &old).unwrap(), None);

        let new = registry(true);
        let loaded = save.read_column(pos, &new).unwrap().unwrap();
        assert_eq!(loaded.block(IVec3::new(4, 100, 9)), new.expect("base:glass"));
        for y in 0..voxmar_engine::coords::WORLD_HEIGHT {
            let at = IVec3::new(17, y, 30);
            assert_eq!(new.def(loaded.block(at)).name, old.def(column.block(at)).name);
        }

        // A damaged file is an error, not a panic or a silently wrong column.
        let path = save.column_path(pos);
        let mut bytes = std::fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x5a;
        std::fs::write(&path, bytes).unwrap();
        assert!(save.read_column(pos, &old).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn biome_colours_travel_with_columns() {
        let registry = registry(false);
        let pos = ColumnPos::new(4, -9);
        let column = Overworld::new(11, registry.clone()).generate(pos);
        assert_eq!(column.colors.len(), voxmar_engine::world::COLUMN_AREA);
        assert!(column.colors.iter().any(|c| *c != voxmar_engine::world::DEFAULT_COLORS), "generation colours columns");

        // Through a save.
        let dir = temp_dir("colours");
        let save = WorldSave::new(&dir);
        save.create(&WorldMeta::new("Colours", 11, GameMode::Creative)).unwrap();
        save.write_column(pos, &column, &registry).unwrap();
        assert_eq!(save.read_column(pos, &registry).unwrap().unwrap().colors, column.colors);
        std::fs::remove_dir_all(&dir).unwrap();

        // Over the network, encoded as the TCP transport does.
        let message = voxmar_protocol::ServerMessage::Column { pos, column: Arc::new(column.clone()) };
        let bytes = postcard::to_stdvec(&message).unwrap();
        let voxmar_protocol::ServerMessage::Column { column: received, .. } = postcard::from_bytes(&bytes).unwrap() else {
            panic!("decoded another message");
        };
        assert_eq!(*received, column);
    }

    /// A player file from before rewards were claimed in the quest screen has no list of
    /// claimed quests: everything it finished was paid then, so all of it counts as claimed.
    /// Saved with a list, it keeps the finished-but-unclaimed ones waiting.
    #[test]
    fn quests_finished_before_claiming_existed_count_as_claimed() {
        let dir = temp_dir("claimed");
        let save = WorldSave::new(&dir);
        let registry = registry(false);
        let player = SavedPlayer {
            position: Vec3::new(0.5, 80.0, 0.5),
            yaw: 0.0,
            mode: None,
            flying: false,
            health: Health { current: 20, max: 20 },
            inventory: Inventory::default(),
            skills: Vec::new(),
            effects: Vec::new(),
            home: None,
            bed: None,
            kindled: true,
            journal: Vec::new(),
            quests: vec!["base:a".into(), "base:b".into()],
            quests_claimed: vec!["base:a".into()],
            tallies: Vec::new(),
            prayer_points: None,
            casts: 0,
            draws: Draws::default(),
            home_steps: Vec::new(),
            home_anchor: None,
            spilled: Vec::new(),
        };
        save.write_player("ada", &player, &registry).unwrap();
        assert_eq!(save.read_player("ada", &registry).unwrap().unwrap().quests_claimed, ["base:a"], "a waiting reward is kept");
        let path = dir.join("players/ada.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        // The field is a list over several lines: cut from its name to the end of its list.
        let from = text.find("quests_claimed").unwrap();
        let to = from + text[from..].find("]),").unwrap() + "]),".len();
        let old = format!("{}{}", &text[..from], text[to..].trim_start());
        assert!(!old.contains("quests_claimed"));
        std::fs::write(&path, old).unwrap();
        assert_eq!(save.read_player("ada", &registry).unwrap().unwrap().quests_claimed, ["base:a", "base:b"], "an older save's finished quests were all paid");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn players_round_trip_by_name() {
        let dir = temp_dir("players");
        let save = WorldSave::new(&dir);
        // The game's blocks, and the charm (a creature's drop, from another blocks file).
        let registry = {
            let mut defs = BlockRegistry::parse("base", include_str!("../../../assets/content/base/blocks.ron")).unwrap();
            defs.push(voxmar_engine::block::BlockDef { name: "base:charge_charm".into(), ..defs[0].clone() });
            Arc::new(BlockRegistry::new(defs).unwrap())
        };
        let mut inventory = Inventory::default();
        inventory.add(registry.expect("base:cobblestone"), 70);
        inventory.pouched.push((registry.expect("base:cobblestone"), 300));
        // What is worn keeps its slot (the save keeps names; content judges what fits).
        inventory.worn[1] = Some(ItemStack::new(registry.expect("base:dirt"), 1));
        inventory.worn[5] = Some(ItemStack::new(registry.expect("base:cobblestone"), 1));
        inventory.worn[6] = Some(ItemStack::new(registry.expect("base:charge_charm"), 1));
        // A worn tool keeps how worn it is.
        inventory.slots[7] = Some(ItemStack { wear: 7, ..ItemStack::new(registry.expect("base:dirt"), 1) });
        let player = SavedPlayer {
            position: Vec3::new(1.5, 80.0, -20.25),
            yaw: 1.25,
            mode: Some(GameMode::Creative),
            flying: true,
            health: Health { current: 13, max: 20 },
            inventory,
            skills: vec![("core:farming".into(), 1234)],
            effects: vec![("base:hearty".into(), 120.5)],
            home: Some([3.5, 70.0, -2.5]),
            bed: Some([1.5, 64.0, 9.5]),
            kindled: true,
            journal: vec![voxmar_engine::journal::Entry { page: voxmar_engine::journal::Page::Fish, name: "base:trout".into(), stage: voxmar_engine::journal::Stage::Studied, count: 12, found: 400 }],
            quests: vec!["base:first_night".into(), "base:stone_in_hand".into()],
            quests_claimed: vec!["base:first_night".into()],
            tallies: vec![("crafted:base:torch".into(), 8)],
            prayer_points: Some(7.5),
            casts: 12,
            draws: Draws { strikes: 31, blows: 9, feeds: 4, cooks: 17, recovers: 5, flourishes: 2, mines: 40, yields: 3, forages: 6, harvests: 11, locks: 1, catches: 8, wakes: 2, rewards: 7, frees: 3, kills: 12, greets: 4, strays: 6, wears: 12 },
            home_steps: Vec::new(),
            home_anchor: None,
            spilled: Vec::new(),
        };
        for name in ["steve", "Steve", "a/b", "../x", "ünï cödé"] {
            save.write_player(name, &player, &registry).unwrap();
        }
        assert_eq!(save.read_player("a/b", &registry).unwrap(), Some(player.clone()));
        assert_eq!(save.read_player("ünï cödé", &registry).unwrap(), Some(player.clone()));
        assert_eq!(save.read_player("alex", &registry).unwrap(), None);
        // The charm slot keeps its charm, and a save from before the charm slot, with six
        // worn entries, loads with the six as they were and the charm slot empty.
        assert_eq!(save.read_player("steve", &registry).unwrap().unwrap().inventory.worn[6], player.inventory.worn[6]);
        let six = dir.join("players/six.ron");
        save.write_player("six", &player, &registry).unwrap();
        let text = std::fs::read_to_string(&six).unwrap();
        // Pretty-printed, the worn list has an entry a line: the charm's, last, goes.
        assert_eq!(text.lines().filter(|line| line.contains("base:charge_charm")).count(), 1, "the charm saved once: {text}");
        let without: Vec<&str> = text.lines().filter(|line| !line.contains("base:charge_charm")).collect();
        std::fs::write(&six, without.join("\n")).unwrap();
        let old = save.read_player("six", &registry).unwrap().unwrap().inventory.worn;
        assert_eq!((old[..6].to_vec(), old[6]), (player.inventory.worn[..6].to_vec(), None), "a six-slot save's worn, the charm slot empty");
        std::fs::remove_file(&six).unwrap();
        // Names that are not plain file names stay inside the players directory.
        let files = std::fs::read_dir(dir.join("players")).unwrap().count();
        assert!(files >= 4, "{files} player files");
        // Names differing only in case are one player, with one file, on any filesystem.
        assert_eq!(save.read_player("STEVE", &registry).unwrap(), Some(player.clone()));
        assert_eq!(save.saved_name("STEVE").as_deref(), Some("Steve"));
        assert!(dir.join("players/steve.ron").exists() && !dir.join("players/Steve.ron").exists() || cfg!(not(target_os = "linux")));
        // A file saved under the name as typed, before names were kept in lower case, is
        // still found, and the next save goes to the lower-case one.
        let old = SavedPlayer { yaw: 0.5, ..player.clone() };
        let text = std::fs::read_to_string(dir.join("players/a~2f~b.ron")).unwrap();
        std::fs::write(dir.join("players/Ada.ron"), text.replace("yaw: 1.25", "yaw: 0.5")).unwrap();
        assert_eq!(save.read_player("Ada", &registry).unwrap(), Some(old.clone()));
        assert_eq!(save.read_player("ada", &registry).unwrap(), Some(old.clone()), "on a case-sensitive disk too");
        save.write_player("Ada", &player, &registry).unwrap();
        assert_eq!(save.read_player("ADA", &registry).unwrap(), Some(player.clone()));
        // A name escaped in the old file is matched by what it holds: `Über` as `~dc~ber`.
        std::fs::write(dir.join("players/~dc~ber.ron"), text.replace("yaw: 1.25", "yaw: 0.5")).unwrap();
        assert_eq!(save.read_player("über", &registry).unwrap(), Some(old.clone()));
        assert_eq!(player_name("a~2f~b").as_deref(), Some("a/b"));
        assert_eq!(player_name("a~zz~b"), None);
        // Legacy files without a saved display name still load, and a mismatched name
        // cannot turn the session into a different player.
        let mut legacy: PlayerFile = ron::from_str(&text).unwrap();
        legacy.name = None;
        let legacy_text = ron::ser::to_string(&legacy).unwrap();
        std::fs::write(dir.join("players/legacy.ron"), legacy_text).unwrap();
        assert_eq!(save.saved_name("LEGACY"), None);
        assert_eq!(save.older_player_names("LEGACY"), ["legacy"]);
        assert_eq!(save.read_player("LEGACY", &registry).unwrap(), Some(player.clone()));
        legacy.name = Some("someone-else".into());
        std::fs::write(dir.join("players/legacy.ron"), ron::ser::to_string(&legacy).unwrap()).unwrap();
        assert_eq!(save.saved_name("LEGACY"), None);
        assert_eq!(save.older_player_names("über"), ["Über"], "decode the actual legacy file name");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A player saved before kills, greetings and stray shots were counted loads with each
    /// at none and every other count kept, and the next save writes them.
    #[test]
    fn a_save_without_kill_or_greeting_counts_loads_them_at_none() {
        let dir = temp_dir("draws-before-kills");
        let save = WorldSave::new(&dir);
        let registry = registry(false);
        let draws = Draws { strikes: 31, mines: 40, frees: 3, kills: 12, greets: 4, strays: 6, ..Draws::default() };
        let player = SavedPlayer { position: Vec3::new(1.5, 80.0, -20.25), yaw: 1.25, mode: None, flying: false, health: Health { current: 13, max: 20 }, inventory: Inventory::default(), skills: Vec::new(), effects: Vec::new(), home: None, bed: None, kindled: false, journal: Vec::new(), quests: Vec::new(), quests_claimed: Vec::new(), tallies: Vec::new(), prayer_points: None, casts: 0, draws, home_steps: Vec::new(), home_anchor: None, spilled: Vec::new() };
        save.write_player("ada", &player, &registry).unwrap();
        let file = dir.join("players/ada.ron");
        let mut text = std::fs::read_to_string(&file).unwrap();
        // The file as an older build wrote it: no `kills`, `greets` or `strays` among the draws.
        for key in ["kills", "greets", "strays"] {
            // ` kills:`, not the end of `skills:`.
            let at = 1 + text.find(&format!(" {key}:")).unwrap_or_else(|| panic!("no `{key}` in {text}"));
            let end = at + text[at..].find([',', ')']).unwrap();
            let start = text[..at].trim_end().strip_suffix(',').map_or(at, |before| before.len());
            text.replace_range(start..end, "");
        }
        assert!(!text.contains(" kills:") && !text.contains(" greets:") && !text.contains(" strays:") && text.contains(" frees:"), "{text}");
        std::fs::write(&file, &text).unwrap();
        let old = save.read_player("ada", &registry).unwrap().expect("loads");
        assert_eq!(old.draws, Draws { kills: 0, greets: 0, strays: 0, ..draws }, "the other counts kept, the new ones at none");
        save.write_player("ada", &SavedPlayer { draws: Draws { kills: 1, greets: 2, strays: 3, ..old.draws }, ..old }, &registry).unwrap();
        let again = std::fs::read_to_string(&file).unwrap();
        assert!(again.contains(" kills:") && again.contains(" greets:") && again.contains(" strays:"), "the next save writes them: {again}");
        assert_eq!(save.read_player("ada", &registry).unwrap().unwrap().draws, Draws { kills: 1, greets: 2, strays: 3, ..draws });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Each kind of roll counts on by itself, and a count's draws are its own: another
    /// count, kind or player's key draws otherwise.
    #[test]
    fn draws_count_on_by_kind_and_differ_by_count_kind_and_key() {
        let mut draws = Draws::default();
        let (ada, bea) = (crate::player::player_draw_key(0x0d1a, "ada"), crate::player::player_draw_key(0x0d1a, "bea"));
        let first = draws.next(ada, Draw::Strike);
        assert_eq!(first, numbered_draws(ada, Draw::Strike.salt(), 0));
        assert_eq!(draws.next(ada, Draw::Strike), numbered_draws(ada, Draw::Strike.salt(), 1));
        draws.next(ada, Draw::Cook);
        assert_eq!(draws, Draws { strikes: 2, cooks: 1, ..Draws::default() });
        let kinds = [Draw::Strike, Draw::Blow, Draw::Feed, Draw::Cook, Draw::Recover, Draw::Flourish, Draw::Mine, Draw::Yield, Draw::Forage, Draw::Harvest, Draw::Lock, Draw::Catch, Draw::Wake, Draw::Reward, Draw::Free, Draw::Kill, Draw::Greet, Draw::Stray];
        // Every kind's salt, the world's kinds of roll and its loot rolls' are told apart.
        let salts: Vec<u64> = kinds.iter().map(|k| k.salt()).chain([ROLL_PRODUCT, ROLL_LITTER, ROLL_YOUNG, ROLL_GRAZE, ROLL_SUMMON, ROLL_WANDER, ROLL_SPAWN, ROLL_PARTY, ROLL_FOLLOWER, ROLL_START, 0x1007]).collect();
        let unique: std::collections::BTreeSet<_> = salts.iter().collect();
        assert_eq!(unique.len(), salts.len(), "two kinds of roll share a salt: {salts:x?}");
        let others: Vec<_> = kinds.iter().skip(1).map(|k| numbered_draws(ada, k.salt(), 0)).chain([numbered_draws(ada, Draw::Strike.salt(), 1), numbered_draws(bea, Draw::Strike.salt(), 0)]).collect();
        assert!(others.iter().all(|other| *other != first), "{first:?} drawn again among {others:?}");
        assert!(others.iter().chain([&first]).flatten().all(|d| (0.0..1.0).contains(d)));
    }

    /// Items an older save holds that have since merged into one come back as that item
    /// (every kind of wild herb is kitchen herbs now), pouched ones added up.
    #[test]
    fn items_merged_since_a_save_come_back_as_the_merged_item() {
        let dir = temp_dir("merged");
        let save = WorldSave::new(&dir);
        let registry = registry(false);
        let mut inventory = Inventory::default();
        inventory.add(registry.expect("base:oregano"), 7);
        inventory.add(registry.expect("base:cobblestone"), 5);
        inventory.pouched.push((registry.expect("base:parsley"), 4));
        inventory.pouched.push((registry.expect("base:dill"), 6));
        let player = SavedPlayer {
            position: Vec3::new(0.5, 80.0, 0.5),
            yaw: 0.0,
            mode: None,
            flying: false,
            health: Health { current: 20, max: 20 },
            inventory,
            skills: Vec::new(),
            effects: Vec::new(),
            home: None,
            bed: None,
            kindled: true,
            journal: Vec::new(),
            quests: Vec::new(),
            quests_claimed: Vec::new(),
            tallies: Vec::new(),
            prayer_points: None,
            casts: 0,
            draws: Draws::default(),
            home_steps: Vec::new(),
            home_anchor: None,
            spilled: Vec::new(),
        };
        save.write_player("herbalist", &player, &registry).unwrap();
        let loaded = save.read_player("herbalist", &registry).unwrap().unwrap();
        let herbs = registry.expect("base:kitchen_herbs");
        assert_eq!(loaded.inventory.slots.iter().flatten().find(|s| s.item == herbs).map(|s| s.count), Some(7));
        assert_eq!(loaded.inventory.slots.iter().flatten().find(|s| s.item == registry.expect("base:cobblestone")).map(|s| s.count), Some(5));
        assert_eq!(loaded.inventory.pouched, vec![(herbs, 10)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An older save's raw and cooked rabbit and chicken (merged into small game, Oct 14)
    /// come back as small game, in its slots, pouches, dropped items and drying racks, so
    /// nothing a player held is lost.
    #[test]
    fn renamed_items_in_an_older_save_come_back_under_their_new_name() {
        let dir = temp_dir("renamed");
        let save = WorldSave::new(&dir);
        let blocks = crate::test_content().blocks;
        std::fs::create_dir_all(dir.join("players")).unwrap();
        let file = r#"(position: (0.5, 80.0, 0.5), yaw: 0.0, flying: false, health: (current: 20, max: 20),
            inventory: [Some(("base:raw_rabbit", 3)), Some(("base:raw_chicken", 2)), Some(("base:cooked_chicken", 4)), None],
            pouched: [("base:raw_rabbit", 5), ("base:raw_chicken", 6)])"#;
        std::fs::write(dir.join("players/hunter.ron"), file).unwrap();
        let loaded = save.read_player("hunter", &blocks).unwrap().unwrap();
        assert_eq!(loaded.casts, 0, "an older save has cast nothing yet");
        assert_eq!(loaded.draws, Draws::default(), "an older save has drawn nothing yet");
        let (raw, cooked) = (blocks.expect("base:raw_small_game"), blocks.expect("base:cooked_small_game"));
        let slots: Vec<(BlockId, u8)> = loaded.inventory.slots.iter().flatten().map(|s| (s.item, s.count)).collect();
        // Cooked food comes one to a slot now: the old stack of four comes apart.
        assert_eq!(slots, [(raw, 3), (raw, 2), (cooked, 1), (cooked, 1), (cooked, 1), (cooked, 1)]);
        assert!(loaded.spilled.is_empty());
        assert_eq!(loaded.inventory.pouched, [(raw, 11)], "pouched rabbit and chicken add up");
        assert_eq!(blocks.item_named("base:cooked_rabbit"), Some(cooked), "dropped items and station loads resolve the same way");
        assert_eq!(blocks.saved_id("base:raw_chicken"), Some(raw), "and a column's block names");
        assert_eq!(blocks.id("base:raw_rabbit"), None, "the old names are no items now");
        assert!(!loaded.kindled, "a save from before hearths kept their fire is not yet kindled");
        assert_eq!(blocks.item_named("base:cooked_shrimp"), Some(blocks.expect("base:cooked_minnow")), "shrimp are minnows now");
        assert_eq!(blocks.item_named("base:raw_frog"), Some(blocks.expect("base:raw_small_game")), "frog is small game now");
        assert_eq!(blocks.item_named("base:bronze_blend"), Some(blocks.expect("base:bronze_ingot")), "a blend is the bronze it smelted into");
        // The wooden tools, gone: each is the tool that does its work now.
        for (old, new) in [("wooden_pickaxe", "flint_pickaxe"), ("wooden_axe", "flint_axe"), ("wooden_knife", "flint_knife"), ("wooden_shovel", "digging_stick"), ("wooden_hoe", "stone_hoe")] {
            assert_eq!(blocks.item_named(&format!("base:{old}")), Some(blocks.expect(&format!("base:{new}"))), "an older save's {old} is a {new}");
        }
        // What quests counted under the old names counts under the new one.
        let tallies = vec![("cooked:base:cooked_rabbit".into(), 2), ("cooked:base:cooked_chicken".into(), 1), ("cooked_at:campfire:base:cooked_rabbit".into(), 3), ("lit:base:crucible_kiln".into(), 1)];
        let mut tallies = renamed_tallies(tallies, &blocks);
        tallies.sort();
        assert_eq!(tallies, [("cooked:base:cooked_small_game".to_owned(), 3), ("cooked_at:campfire:base:cooked_small_game".to_owned(), 3), ("lit:base:crucible_kiln".to_owned(), 1)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A player file from when blocks set down for quests were kept in it (`placed`, now
    /// the world's `quest_placed`) still loads, its tallies as they were.
    #[test]
    fn a_player_file_that_kept_placed_blocks_still_loads() {
        let dir = temp_dir("placed-in-player");
        let save = WorldSave::new(&dir);
        let blocks = crate::test_content().blocks;
        std::fs::create_dir_all(dir.join("players")).unwrap();
        let file = r#"(position: (0.5, 80.0, 0.5), yaw: 0.0, flying: false, health: (current: 20, max: 20),
            inventory: [None], tallies: [("placed:base:feed_trough", 1)], placed: [((4, 65, -7), "base:feed_trough")])"#;
        std::fs::write(dir.join("players/keeper.ron"), file).unwrap();
        let loaded = save.read_player("keeper", &blocks).unwrap().expect("it loads");
        assert_eq!(loaded.tallies, [("placed:base:feed_trough".to_owned(), 1)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Plants found as separate entries before they merged are one entry now: found first
    /// when the earliest was, studied as far as the furthest, picked as many times as all.
    #[test]
    fn journal_entries_of_plants_that_merged_fold_into_the_item() {
        use voxmar_engine::journal::{Entry, Page, Stage};
        let registry = registry(false);
        let entry = |name: &str, stage, count, found| Entry { page: Page::Plants, name: name.into(), stage, count, found };
        let merged = merge_journal(
            vec![
                entry("base:oregano", Stage::Seen, 3, 900),
                entry("base:poppy", Stage::Studied, 2, 50),
                entry("base:parsley", Stage::Mastered, 4, 300),
                entry("base:dill", Stage::Seen, 1, 100),
                Entry { page: Page::Fish, name: "base:oregano".into(), stage: Stage::Seen, count: 1, found: 5 },
            ],
            &registry,
        );
        let herbs = merged.iter().find(|e| e.page == Page::Plants && e.name == "base:kitchen_herbs").expect("one entry for the item");
        assert_eq!((herbs.stage, herbs.count, herbs.found), (Stage::Mastered, 8, 100));
        assert_eq!(merged.iter().filter(|e| e.page == Page::Plants && e.name.contains("oregano")).count(), 0);
        assert!(merged.iter().any(|e| e.name == "base:poppy" && e.stage == Stage::Studied), "an unmerged plant is kept");
        assert_eq!(merged.len(), 3, "{merged:?}");
    }

    /// Shrimp caught before they became minnows are minnows in the journal: one kind, the
    /// counts added, so a fish tally or a kinds count does not lose or double them.
    #[test]
    fn journal_fish_that_were_renamed_fold_into_the_new_fish() {
        use voxmar_engine::journal::{Entry, Page, Stage};
        let registry = crate::test_content().blocks;
        let fish = |name: &str, stage, count, found| Entry { page: Page::Fish, name: name.into(), stage, count, found };
        let merged = merge_journal(vec![fish("base:raw_shrimp", Stage::Studied, 5, 40), fish("base:raw_minnow", Stage::Seen, 2, 90)], &registry);
        assert_eq!(merged.len(), 1, "{merged:?}");
        let minnow = &merged[0];
        assert_eq!((minnow.name.as_str(), minnow.stage, minnow.count, minnow.found), ("base:raw_minnow", Stage::Studied, 7, 40));
    }

    #[test]
    fn worlds_are_listed_newest_first() {
        let root = temp_dir("list");
        for (name, last_played) in [("old", 100), ("new", 300), ("middle", 200)] {
            let meta = WorldMeta { last_played, ..WorldMeta::new(name, 1, GameMode::Survival) };
            WorldSave::new(root.join(name)).create(&meta).unwrap();
        }
        std::fs::create_dir_all(root.join("not a world")).unwrap();
        let names: Vec<String> = list_worlds(&root).into_iter().map(|(_, meta)| meta.name).collect();
        assert_eq!(names, ["new", "middle", "old"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Where drops pop is seen by every client, so it is drawn from a key made one way
    /// from the draw key, never the draw key itself; and the world's rolls count on.
    #[test]
    fn pops_never_show_the_draw_key_and_world_rolls_count_on() {
        let mut meta = WorldMeta::new("w", 1, GameMode::Survival);
        meta.draw_key = 0x0d1a;
        let cell = voxmar_engine::glam::IVec3::new(4, 70, -2);
        assert_ne!(pop_key(meta.draw_key), meta.draw_key);
        assert_ne!(pop_key(1), pop_key(2), "two keys pop alike");
        let pop = meta.next_pop(cell, 3);
        assert_eq!(pop, crate::items::random3(pop_key(0x0d1a), cell, 3), "the first pop is not drawn from the pop key");
        assert_ne!(pop, crate::items::random3(0x0d1a, cell, 3), "a pop was drawn from the draw key itself");
        assert_eq!(meta.drops, 1);
        let rolled = meta.next_roll(ROLL_PRODUCT);
        assert_eq!(rolled, numbered_draws(0x0d1a, ROLL_PRODUCT, 0));
        assert_ne!(meta.next_roll(ROLL_PRODUCT), rolled, "the next roll drew the same");
        assert_eq!(meta.rolls, 2);
    }

    /// What every client sees cast (a creature's seed, the start of its own count) is drawn
    /// by the cast key, made one way from the draw key, on the world's count of rolls.
    #[test]
    fn casts_draw_by_the_cast_key_never_the_draw_key() {
        let mut meta = WorldMeta::new("w", 9, GameMode::Survival);
        meta.draw_key = 0x0d1a;
        assert_ne!(cast_key(meta.draw_key), meta.draw_key);
        assert_ne!(cast_key(meta.draw_key), pop_key(meta.draw_key), "casts and pops share a key");
        assert_ne!(cast_key(1), cast_key(2), "two keys cast alike");
        let cast = meta.next_cast(ROLL_START);
        assert_eq!(cast, numbered_draws(cast_key(0x0d1a), ROLL_START, 0), "a cast is not drawn from the cast key");
        assert_ne!(cast, numbered_draws(0x0d1a, ROLL_START, 0), "a cast was drawn from the draw key itself");
        let [a, b] = numbered_draws(cast_key(0x0d1a), ROLL_SPAWN, 1);
        let seed = voxmar_engine::creature::mix(9, u64::from(a.to_bits()) << 32 | u64::from(b.to_bits()));
        assert_eq!(meta.roll_seed(ROLL_SPAWN), seed, "a spawn's seed is not cast by the cast key");
        assert_eq!(meta.rolls, 2);
    }
}
