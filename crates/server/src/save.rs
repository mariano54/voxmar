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
//!
//! Every file is written to a temporary file and then renamed over the old one, so a crash
//! never leaves a half-written file behind.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// Where players put blocks that teach a skill when broken (ores), which then teach
    /// nothing: experience comes from what the world holds, not from placing and breaking.
    #[serde(default)]
    pub placed: Vec<[i32; 3]>,
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
    /// Where the player wakes after dying, if a bed or hearth set it.
    pub home: Option<[f32; 3]>,
}

/// A player file. Items are stored by name, like column blocks.
#[derive(Serialize, Deserialize)]
struct PlayerFile {
    position: [f32; 3],
    yaw: f32,
    #[serde(default)]
    mode: Option<GameMode>,
    flying: bool,
    health: Health,
    inventory: Vec<Option<(String, u8)>>,
    #[serde(default)]
    skills: Vec<(String, u32)>,
    #[serde(default)]
    effects: Vec<(String, f32)>,
    #[serde(default)]
    home: Option<[f32; 3]>,
    /// What pouches hold, by item name.
    #[serde(default)]
    pouched: Vec<(String, u16)>,
}

/// A column file, generic so that saving can borrow the column.
#[derive(Serialize, Deserialize)]
struct ColumnFile<C> {
    /// Name of every block id the column's chunks refer to.
    blocks: Vec<(u16, String)>,
    column: C,
}

/// A world directory. Cheap to clone, so worker threads can load columns.
#[derive(Clone, Debug)]
pub struct WorldSave {
    dir: Arc<Path>,
}

impl WorldSave {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into().into() }
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
        ZlibDecoder::new(compressed).read_to_end(&mut encoded)?;
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
        write_atomic(&path, &encoder.finish()?)
    }

    fn player_path(&self, name: &str) -> PathBuf {
        self.dir.join("players").join(format!("{}.ron", file_name(name)))
    }

    /// The saved state of the player called `name`. Items that no longer exist are dropped.
    pub fn read_player(&self, name: &str, registry: &BlockRegistry) -> io::Result<Option<SavedPlayer>> {
        let path = self.player_path(name);
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
                .and_then(|(item, count)| Some(ItemStack { item: registry.id(&item)?, count: count.min(MAX_STACK) }));
        }
        inventory.pouched = file.pouched.iter().filter_map(|(item, count)| Some((registry.id(item)?, *count))).filter(|&(_, n)| n > 0).collect();
        let position = Vec3::from(file.position);
        if !position.is_finite() {
            return Err(invalid(&path, "position is not finite"));
        }
        let health = Health { current: file.health.current.min(file.health.max), max: file.health.max };
        Ok(Some(SavedPlayer { position, yaw: file.yaw, mode: file.mode, flying: file.flying, health, inventory, skills: file.skills, effects: file.effects, home: file.home }))
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
            position: player.position.to_array(),
            yaw: player.yaw,
            mode: player.mode,
            flying: player.flying,
            health: player.health,
            inventory,
            skills: player.skills.clone(),
            effects: player.effects.clone(),
            home: player.home,
            pouched: held.pouched.iter().map(|&(item, count)| (registry.def(item).name.clone(), count)).collect(),
        };
        let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default()).map_err(io::Error::other)?;
        let path = self.player_path(name);
        std::fs::create_dir_all(path.parent().expect("player files are in a directory"))?;
        write_atomic(&path, text.as_bytes())
    }
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
        write_atomic(&self.dir.join(CROPS_FILE), text.as_bytes())
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
        write_atomic(&self.dir.join(CREATURES_FILE), text.as_bytes())
    }

    pub fn write_items(&self, items: &[SavedItem]) -> io::Result<()> {
        let text = ron::ser::to_string(items).map_err(io::Error::other)?;
        write_atomic(&self.dir.join(ITEMS_FILE), text.as_bytes())
    }
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

/// Writes a file through a temporary file and a rename, so readers and crashes only ever
/// see the old or the new contents.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
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

    #[test]
    fn players_round_trip_by_name() {
        let dir = temp_dir("players");
        let save = WorldSave::new(&dir);
        let registry = registry(false);
        let mut inventory = Inventory::default();
        inventory.add(registry.expect("base:cobblestone"), 70);
        inventory.pouched.push((registry.expect("base:cobblestone"), 300));
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
        std::fs::remove_dir_all(&dir).unwrap();
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
