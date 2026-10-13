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
    /// Picked wild plants waiting to grow back, and plants herb planters grew back.
    #[serde(default)]
    pub regrowing: Vec<SavedRegrowing>,
    #[serde(default)]
    pub planter_grown: Vec<[i32; 3]>,
    /// Fuelled multiblocks and the server tick when their fire goes out. Older worlds
    /// open cold; completed quests stay completed in the player's own file.
    #[serde(default)]
    pub lit: Vec<crate::hearth::Lit>,
    /// The last whole save: files of a later one were cut short and are not used.
    #[serde(default)]
    pub generation: u64,
    /// The fingerprint of the content land was last made from (`WorldContent::fingerprint`):
    /// a world opened with different worldgen data warns that new land may not line up.
    /// `None` until a world is first opened with it.
    #[serde(default)]
    pub worldgen_content: Option<u64>,
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
            regrowing: Vec::new(),
            planter_grown: Vec::new(),
            lit: Vec::new(),
            generation: 0,
            worldgen_content: None,
        }
    }
}

/// Seconds since the Unix epoch.
pub fn unix_time() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
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
    /// Everything they have discovered.
    pub journal: Vec<voxmar_engine::journal::Entry>,
    /// Quests finished, those of them whose rewards were claimed, and the tallies quests
    /// count.
    pub quests: Vec<String>,
    pub quests_claimed: Vec<String>,
    pub tallies: Vec<(String, u32)>,
    /// Prayer points left; none in older saves, which come back full.
    pub prayer_points: Option<f32>,
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
                let current = registry.id(name).unwrap_or_else(|| {
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
                .and_then(|(item, count)| Some(ItemStack { item: registry.item_named(&item)?, count: count.min(MAX_STACK) }));
        }
        // Older saves' items that merged into one add up.
        for (item, count) in file.pouched.iter().filter_map(|(item, count)| Some((registry.item_named(item)?, *count))).filter(|&(_, n)| n > 0) {
            match inventory.pouched.iter_mut().find(|(have, _)| *have == item) {
                Some((_, n)) => *n = n.saturating_add(count),
                None => inventory.pouched.push((item, count)),
            }
        }
        for (slot, item) in inventory.worn.iter_mut().zip(file.worn) {
            *slot = item.and_then(|item| Some(ItemStack { item: registry.item_named(&item)?, count: 1 }));
        }
        let position = Vec3::from(file.position);
        if !position.is_finite() {
            return Err(invalid(&path, "position is not finite"));
        }
        let health = Health { current: file.health.current.min(file.health.max), max: file.health.max };
        let yaw = if file.yaw.is_finite() { file.yaw } else { 0.0 };
        Ok(Some(SavedPlayer { position, yaw, mode: file.mode, flying: file.flying, health, inventory, skills: file.skills, effects: file.effects, home: file.home, bed: file.bed, journal: merge_journal(file.journal, registry), quests_claimed: file.quests_claimed.unwrap_or_else(|| file.quests.clone()), quests: file.quests, tallies: file.tallies, prayer_points: file.prayer_points }))
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
            worn: held.worn.iter().map(|slot| slot.map(|stack| registry.def(stack.item).name.clone())).collect(),
            skills: player.skills.clone(),
            effects: player.effects.clone(),
            home: player.home,
            bed: player.bed,
            pouched: held.pouched.iter().map(|&(item, count)| (registry.def(item).name.clone(), count)).collect(),
            journal: player.journal.clone(),
            quests: player.quests.clone(),
            quests_claimed: Some(player.quests_claimed.clone()),
            tallies: player.tallies.clone(),
            prayer_points: player.prayer_points,
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
}

impl WorldSave {
    /// The dropped items, crops, kept creatures, machines and who of the villagers died. A file that is not what it should be is
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
        let source = include_str!("../../../assets/content/base/blocks.ron");
        let mut defs = BlockRegistry::parse("base", source).unwrap();
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
        let machines = |count: u8| vec![crate::machines::SavedMachine { position: [0, 64, 0], block: "base:chest".into(), inputs: Vec::new(), outputs: Vec::new(), fuel: None, burning: 0.0, energy: 0.0, condition: 0.0, made: 0, bin: vec![("base:iron_ore".into(), count)], owner: None, counted: false, work: None }];
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
            journal: Vec::new(),
            quests: vec!["base:a".into(), "base:b".into()],
            quests_claimed: vec!["base:a".into()],
            tallies: Vec::new(),
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
        let registry = registry(false);
        let mut inventory = Inventory::default();
        inventory.add(registry.expect("base:cobblestone"), 70);
        inventory.pouched.push((registry.expect("base:cobblestone"), 300));
        // What is worn keeps its slot (the save keeps names; content judges what fits).
        inventory.worn[1] = Some(ItemStack { item: registry.expect("base:dirt"), count: 1 });
        inventory.worn[5] = Some(ItemStack { item: registry.expect("base:cobblestone"), count: 1 });
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
            journal: vec![voxmar_engine::journal::Entry { page: voxmar_engine::journal::Page::Fish, name: "base:trout".into(), stage: voxmar_engine::journal::Stage::Studied, count: 12, found: 400 }],
            quests: vec!["base:first_night".into(), "base:stone_in_hand".into()],
            quests_claimed: vec!["base:first_night".into()],
            tallies: vec![("crafted:base:torch".into(), 8)],
            prayer_points: Some(7.5),
        };
        for name in ["steve", "Steve", "a/b", "../x", "ünï cödé"] {
            save.write_player(name, &player, &registry).unwrap();
        }
        assert_eq!(save.read_player("a/b", &registry).unwrap(), Some(player.clone()));
        assert_eq!(save.read_player("ünï cödé", &registry).unwrap(), Some(player.clone()));
        assert_eq!(save.read_player("alex", &registry).unwrap(), None);
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
            journal: Vec::new(),
            quests: Vec::new(),
            quests_claimed: Vec::new(),
            tallies: Vec::new(),
            prayer_points: None,
        };
        save.write_player("herbalist", &player, &registry).unwrap();
        let loaded = save.read_player("herbalist", &registry).unwrap().unwrap();
        let herbs = registry.expect("base:kitchen_herbs");
        assert_eq!(loaded.inventory.slots.iter().flatten().find(|s| s.item == herbs).map(|s| s.count), Some(7));
        assert_eq!(loaded.inventory.slots.iter().flatten().find(|s| s.item == registry.expect("base:cobblestone")).map(|s| s.count), Some(5));
        assert_eq!(loaded.inventory.pouched, vec![(herbs, 10)]);
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
}
