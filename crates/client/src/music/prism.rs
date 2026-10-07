//! Music from the player's own Prism Launcher instance (their newest GT New Horizons one,
//! unless the settings name another), with the `minecraft_assets` setting on. Nothing is
//! copied or shipped: tracks are found where the launcher and the modpack keep them and
//! read one at a time, as each is about to play.
//!
//! Two kinds of place hold tracks:
//! - the launcher's shared asset store, whose index for the instance's Minecraft version
//!   names each vanilla file by hash (`assets/objects/<hash prefix>/<hash>`), as in
//!   `sfx/minecraft.rs`;
//! - packs in the instance's game folder (mod jars, resource packs, a music mod's
//!   soundtrack zip), each shaped like a resource pack: `assets/<namespace>/sounds/...ogg`,
//!   with `assets/<namespace>/sounds.json` naming its sound events. Which folders are
//!   looked in, and what counts as music there, is data (`prism` in `music.ron`).
//!
//! A track is known by its location, `<namespace>:<path under sounds/>` without the
//! extension (`minecraft:music/game/calm1`), which the rules in `music.ron` match to
//! choose where it plays. Packs repeat each other's tracks (a resource pack shipping the
//! same files as a music mod), so a file name found in an earlier place is skipped later.

use std::io::Read;
use std::path::{Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;

use super::rules::glob;

/// Where to look in a Prism Launcher instance, and what counts as music there.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Discovery {
    /// Instances whose folder or display name contains one of these, any case; the most
    /// recently launched plays.
    pub instances: Vec<String>,
    /// Folders of the game folder to look in, in order after the asset store.
    pub sources: Vec<SourceFolder>,
    /// Locations that are music by their path (`*music/*`).
    pub music_paths: Vec<String>,
    /// Sound events whose files are music by the event's name (`record*`) or category.
    pub music_events: Vec<String>,
    pub music_categories: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFolder {
    /// Relative to the instance's game folder.
    pub folder: String,
    /// Every sound file in its packs is a track (a music mod's soundtrack).
    #[serde(default)]
    pub all_music: bool,
}

/// What to look in: the launcher's folder, and the instance in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    /// Prism Launcher's data folder; empty for the standard one.
    pub launcher: String,
    /// The instance's folder name (or a full path); empty for the newest matching one.
    pub instance: String,
}

/// A track found in the instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// `<namespace>:<path under sounds/>`, without the extension.
    pub location: String,
    pub file: TrackFile,
}

/// Where a track's bytes are.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackFile {
    /// A file of its own: in the asset store, or in a pack that is a folder.
    Loose(PathBuf),
    /// An entry in a zip or jar.
    Zipped { archive: PathBuf, entry: String },
}

impl TrackFile {
    /// The encoded track (Ogg Vorbis).
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        match self {
            TrackFile::Loose(path) => std::fs::read(path),
            TrackFile::Zipped { archive, entry } => {
                let mut archive = zip::ZipArchive::new(std::fs::File::open(archive)?).map_err(std::io::Error::other)?;
                let mut file = archive.by_name(entry).map_err(std::io::Error::other)?;
                let mut bytes = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut bytes)?;
                Ok(bytes)
            }
        }
    }
}

/// What a search found, and where.
#[derive(Debug, Default)]
pub struct Found {
    pub instance: Option<PathBuf>,
    pub tracks: Vec<Track>,
    /// How many tracks each place added, for the log.
    pub counts: Vec<(String, usize)>,
}

/// Prism Launcher's standard data folders, the first existing one being used.
fn launcher_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from) {
        folders.push(home.join("Library/Application Support/PrismLauncher"));
        folders.push(home.join(".local/share/PrismLauncher"));
        folders.push(home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        folders.push(appdata.join("PrismLauncher"));
    }
    folders
}

/// Finds the instance and its tracks. Only the launcher's `instances` and `assets` folders
/// are read.
pub fn discover(place: &Place, rules: &Discovery) -> Found {
    let launcher = if place.launcher.is_empty() {
        launcher_folders().into_iter().find(|folder| folder.join("instances").is_dir())
    } else {
        Some(PathBuf::from(&place.launcher))
    };
    let Some(launcher) = launcher else { return Found::default() };
    let instance = if place.instance.is_empty() {
        newest_instance(&launcher.join("instances"), &rules.instances)
    } else {
        Some(launcher.join("instances").join(&place.instance)).filter(|path| path.is_dir())
    };
    let Some(instance) = instance else { return Found::default() };
    let mut found = search(&launcher.join("assets"), &instance, rules);
    found.instance = Some(instance);
    found
}

/// The most recently launched instance whose folder or display name matches.
fn newest_instance(instances: &Path, names: &[String]) -> Option<PathBuf> {
    let names: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(instances).ok()?.flatten() {
        let path = entry.path();
        let Ok(config) = std::fs::read_to_string(path.join("instance.cfg")) else { continue };
        let value = |key: &str| config.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=').map(str::trim));
        let folder = entry.file_name().to_string_lossy().to_lowercase();
        let shown = value("name").unwrap_or_default().to_lowercase();
        if !names.iter().any(|n| folder.contains(n.as_str()) || shown.contains(n.as_str())) {
            continue;
        }
        let launched = value("lastLaunchTime").and_then(|t| t.parse().ok()).unwrap_or(0);
        if best.as_ref().is_none_or(|(newest, _)| launched > *newest) {
            best = Some((launched, path));
        }
    }
    best.map(|(_, path)| path)
}

/// The game folder of an instance: `.minecraft`, or `minecraft` in newer launchers.
fn game_folder(instance: &Path) -> PathBuf {
    [".minecraft", "minecraft"].iter().map(|name| instance.join(name)).find(|path| path.is_dir()).unwrap_or_else(|| instance.join(".minecraft"))
}

/// The instance's tracks: the asset store's for its Minecraft version, then the packs'.
pub fn search(assets: &Path, instance: &Path, rules: &Discovery) -> Found {
    let mut found = Found::default();
    let mut names = FxHashSet::default();
    let mut add = |found: &mut Found, label: String, tracks: Vec<Track>| {
        // Within one place every track counts, even two of one name in different folders.
        let fresh: Vec<Track> = tracks.into_iter().filter(|t| !names.contains(file_name(&t.location))).collect();
        names.extend(fresh.iter().map(|t| file_name(&t.location).to_owned()));
        found.counts.push((label, fresh.len()));
        found.tracks.extend(fresh);
    };
    if let Some(version) = minecraft_version(instance) {
        let index = assets.join("indexes").join(format!("{version}.json"));
        add(&mut found, format!("assets {version}"), store_tracks(&index, &assets.join("objects"), rules));
    }
    let game = game_folder(instance);
    for source in &rules.sources {
        let folder = game.join(&source.folder);
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut packs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        packs.sort();
        for pack in packs {
            let tracks = match Pack::open(&pack) {
                Some(pack) => pack.tracks(source.all_music, rules),
                None => continue,
            };
            if !tracks.is_empty() {
                let label = format!("{}/{}", source.folder, pack.file_name().unwrap_or_default().to_string_lossy());
                add(&mut found, label, tracks);
            }
        }
    }
    found
}

/// The last part of a location: `calm1` of `minecraft:music/game/calm1`.
fn file_name(location: &str) -> &str {
    location.rsplit(['/', ':']).next().unwrap_or(location)
}

/// The instance's Minecraft version, from the components Prism keeps in `mmc-pack.json`.
fn minecraft_version(instance: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Components {
        components: Vec<Component>,
    }
    #[derive(Deserialize)]
    struct Component {
        uid: String,
        #[serde(default)]
        version: Option<String>,
    }
    let pack: Components = serde_json::from_slice(&std::fs::read(instance.join("mmc-pack.json")).ok()?).ok()?;
    // Minecraft itself is the component `net.minecraft`.
    pack.components.into_iter().find(|c| c.uid.split('.').eq(["net", "minecraft"]))?.version
}

/// The music files an asset index names (`minecraft/sounds/music/game/calm1.ogg`) whose
/// objects are in the store.
fn store_tracks(index: &Path, objects: &Path, rules: &Discovery) -> Vec<Track> {
    #[derive(Deserialize)]
    struct Index {
        objects: FxHashMap<String, Object>,
    }
    #[derive(Deserialize)]
    struct Object {
        hash: String,
    }
    let Some(index) = std::fs::read(index).ok().and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok()) else {
        return Vec::new();
    };
    let mut tracks: Vec<Track> = index
        .objects
        .into_iter()
        .filter_map(|(name, object)| {
            let (namespace, path) = name.split_once("/sounds/")?;
            let location = format!("{namespace}:{}", path.strip_suffix(".ogg")?);
            let file = objects.join(object.hash.get(..2)?).join(&object.hash);
            (is_music_path(&location, rules) && file.is_file()).then_some(Track { location, file: TrackFile::Loose(file) })
        })
        .collect();
    tracks.sort_by(|a, b| a.location.cmp(&b.location));
    tracks
}

fn is_music_path(location: &str, rules: &Discovery) -> bool {
    let path = location.split_once(':').map_or(location, |(_, path)| path);
    rules.music_paths.iter().any(|pattern| glob(pattern, path))
}

/// A resource-pack-shaped zip, jar or folder.
enum Pack {
    Zipped { archive: PathBuf, names: Vec<String> },
    Folder { root: PathBuf, names: Vec<String> },
}

impl Pack {
    fn open(path: &Path) -> Option<Self> {
        if path.is_dir() {
            let mut names = Vec::new();
            list(path, "", 0, &mut names);
            return Some(Pack::Folder { root: path.to_owned(), names });
        }
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        if extension != "zip" && extension != "jar" {
            return None;
        }
        let archive = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
        let names = archive.file_names().map(str::to_owned).collect();
        Some(Pack::Zipped { archive: path.to_owned(), names })
    }

    fn names(&self) -> &[String] {
        match self {
            Pack::Zipped { names, .. } | Pack::Folder { names, .. } => names,
        }
    }

    fn file(&self, name: &str) -> TrackFile {
        match self {
            Pack::Zipped { archive, .. } => TrackFile::Zipped { archive: archive.clone(), entry: name.to_owned() },
            Pack::Folder { root, .. } => TrackFile::Loose(root.join(name)),
        }
    }

    /// The pack's music: every sound file if `all_music`, else those whose path, or the
    /// sound event naming them, says they are music.
    fn tracks(&self, all_music: bool, rules: &Discovery) -> Vec<Track> {
        let mut sounds: Vec<(String, &String)> = self
            .names()
            .iter()
            .filter_map(|name| {
                let (namespace, path) = name.strip_prefix("assets/")?.split_once("/sounds/")?;
                let path = path.strip_suffix(".ogg").or_else(|| path.strip_suffix(".OGG"))?;
                (!namespace.contains('/')).then(|| (format!("{namespace}:{path}"), name))
            })
            .collect();
        if sounds.is_empty() {
            return Vec::new();
        }
        let named = if all_music { FxHashSet::default() } else { self.event_music(rules) };
        sounds.retain(|(location, _)| all_music || named.contains(location) || is_music_path(location, rules));
        sounds.sort();
        sounds.into_iter().map(|(location, name)| Track { file: self.file(name), location }).collect()
    }

    /// Locations of the files of every music event in the pack's `sounds.json` files.
    fn event_music(&self, rules: &Discovery) -> FxHashSet<String> {
        let mut music = FxHashSet::default();
        for name in self.names() {
            let Some(namespace) = name.strip_prefix("assets/").and_then(|n| n.strip_suffix("/sounds.json")) else { continue };
            if namespace.contains('/') {
                continue;
            }
            let Some(bytes) = self.read(name) else { continue };
            let Ok(events) = serde_json::from_slice::<FxHashMap<String, Event>>(&bytes) else { continue };
            for (event, definition) in events {
                let by_category = definition.category.as_ref().is_some_and(|c| rules.music_categories.contains(c));
                if !by_category && !rules.music_events.iter().any(|pattern| glob(pattern, &event)) {
                    continue;
                }
                for sound in definition.sounds {
                    let (file, is_event) = match sound {
                        Sound::Name(file) => (file, false),
                        Sound::Full { name, kind } => (name, kind.as_deref() == Some("event")),
                    };
                    if !is_event {
                        music.insert(if file.contains(':') { file } else { format!("{namespace}:{file}") });
                    }
                }
            }
        }
        music
    }

    /// A small file of the pack (its `sounds.json`).
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.file(name).read().ok()
    }
}

/// A sound event in `sounds.json`.
#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    sounds: Vec<Sound>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Sound {
    Name(String),
    Full {
        name: String,
        #[serde(default, rename = "type")]
        kind: Option<String>,
    },
}

/// Every file under `folder`, as paths relative to the pack's root with `/` between parts.
fn list(folder: &Path, prefix: &str, depth: u32, names: &mut Vec<String>) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => list(&entry.path(), &format!("{name}/"), depth + 1, names),
            Ok(kind) if kind.is_file() => names.push(name),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A fresh empty folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxmar-prism-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Stand-in track bytes: an Ogg page header and a name, not real audio.
    fn ogg(name: &str) -> Vec<u8> {
        [b"OggS\0\x02".as_slice(), name.as_bytes()].concat()
    }

    fn zip(path: &Path, files: &[(&str, Vec<u8>)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn rules() -> Discovery {
        Discovery {
            instances: vec!["gt new horizons".into(), "gt_new_horizons".into()],
            sources: vec![
                SourceFolder { folder: "soundtrack".into(), all_music: true },
                SourceFolder { folder: "resourcepacks".into(), all_music: false },
                SourceFolder { folder: "mods".into(), all_music: false },
            ],
            music_paths: vec!["*music/*".into(), "*records/*".into()],
            music_events: vec!["record*".into(), "*music*".into()],
            music_categories: vec!["music".into(), "record".into()],
        }
    }

    /// A launcher folder with an asset store, two GT New Horizons instances and another.
    fn launcher(root: &Path) {
        let assets = root.join("assets");
        let object = |name: &str, hash: &str| {
            write(&assets.join("objects").join(&hash[..2]).join(hash), &ogg(name));
        };
        object("calm1", "aa11");
        object("menu1", "bb22");
        object("cat", "cc33");
        object("grass1", "dd44");
        write(
            &assets.join("indexes/1.7.10.json"),
            br#"{"objects": {
                "minecraft/sounds/music/game/calm1.ogg": {"hash": "aa11", "size": 10},
                "minecraft/sounds/music/menu/menu1.ogg": {"hash": "bb22", "size": 10},
                "minecraft/sounds/records/cat.ogg": {"hash": "cc33", "size": 10},
                "minecraft/sounds/dig/grass1.ogg": {"hash": "dd44", "size": 10},
                "minecraft/sounds/music/game/missing.ogg": {"hash": "ee55", "size": 10},
                "minecraft/sounds.json": {"hash": "ff66", "size": 10}
            }}"#,
        );
        let instances = root.join("instances");
        let instance = |folder: &str, name: &str, launched: u64| {
            let dir = instances.join(folder);
            write(&dir.join("instance.cfg"), format!("[General]\nname={name}\nlastLaunchTime={launched}\n").as_bytes());
            write(&dir.join("mmc-pack.json"), br#"{"components": [{"uid": "org.lwjgl"}, {"uid": "net.minecraft", "version": "1.7.10"}]}"#);
            dir.join(".minecraft")
        };
        instance("GT_New_Horizons_2.8.0", "GTNH 2.8.0", 100);
        instance("Vanilla", "Vanilla", 900);
        let game = instance("GT_New_Horizons_2.9.0-beta-3_Java_17-26", "GTNH 2.9.0-beta-3", 500);
        // A music mod's soundtrack: every file a track, named only through hashed events.
        zip(
            &game.join("soundtrack/Soundtrack.zip"),
            &[
                ("assets/melodies/sounds.json", br#"{"track.0123": {"sounds": [{"name": "melodies:minecraft/music/game/calm1", "stream": true}]}}"#.to_vec()),
                ("assets/melodies/sounds/minecraft/music/game/calm1.ogg", ogg("calm1 again")),
                ("assets/melodies/sounds/statech/overworld/forest_dawn.ogg", ogg("forest_dawn")),
                ("assets/melodies/sounds/statech/caves/deep_hum.ogg", ogg("deep_hum")),
                ("assets/melodies/sounds/statech/end/theme.ogg", ogg("end theme")),
                ("assets/melodies/sounds/statech/nether/theme.ogg", ogg("nether theme")),
            ],
        );
        // A resource pack repeating one of the soundtrack's files, and adding one.
        zip(
            &game.join("resourcepacks/StaTech-Music.zip"),
            &[
                ("pack.mcmeta", b"{}".to_vec()),
                ("assets/st/sounds/music/overworld/forest_dawn.ogg", ogg("forest_dawn copy")),
                ("assets/st/sounds/music/overworld/meadow.ogg", ogg("meadow")),
            ],
        );
        // A folder resource pack.
        write(&game.join("resourcepacks/Loose/assets/loose/sounds/music/rain_walk.ogg"), &ogg("rain_walk"));
        // A mod with a music disc named by its event, and block sounds that are not music.
        zip(
            &game.join("mods/discs.jar"),
            &[
                (
                    "assets/discs/sounds.json",
                    br#"{"record.sky": {"sounds": [{"name": "disc/sky", "stream": true}]}, "block.hum": {"sounds": ["hum"]}}"#.to_vec(),
                ),
                ("assets/discs/sounds/disc/sky.ogg", ogg("sky")),
                ("assets/discs/sounds/hum.ogg", ogg("hum")),
                ("mcmod.info", b"[]".to_vec()),
            ],
        );
        zip(&game.join("mods/old.jar.disabled"), &[("assets/old/sounds/music/old.ogg", ogg("old"))]);
    }

    #[test]
    fn finds_every_track_of_the_newest_matching_instance_once() {
        let root = scratch("find");
        launcher(&root);
        let found = discover(&Place { launcher: root.to_string_lossy().into_owned(), instance: String::new() }, &rules());
        assert_eq!(found.instance, Some(root.join("instances/GT_New_Horizons_2.9.0-beta-3_Java_17-26")));
        let locations: Vec<&str> = found.tracks.iter().map(|t| t.location.as_str()).collect();
        assert_eq!(
            locations,
            [
                // The store's music and records, not its block sounds or files it lacks.
                "minecraft:music/game/calm1",
                "minecraft:music/menu/menu1",
                "minecraft:records/cat",
                // The soundtrack, less calm1, found already; both `theme`s, in different folders.
                "melodies:statech/caves/deep_hum",
                "melodies:statech/end/theme",
                "melodies:statech/nether/theme",
                "melodies:statech/overworld/forest_dawn",
                // Packs in name order: the folder pack, then the zip's new track only.
                "loose:music/rain_walk",
                "st:music/overworld/meadow",
                // The mod's disc, by its event; not its block sound or a disabled mod.
                "discs:disc/sky",
            ]
        );
        let counts: Vec<(&str, usize)> = found.counts.iter().map(|(label, n)| (label.as_str(), *n)).collect();
        assert_eq!(
            counts,
            [
                ("assets 1.7.10", 3),
                ("soundtrack/Soundtrack.zip", 4),
                ("resourcepacks/Loose", 1),
                ("resourcepacks/StaTech-Music.zip", 1),
                ("mods/discs.jar", 1)
            ]
        );
        // Each reads back as its own bytes, from the store, a zip or a folder.
        let read = |location: &str| found.tracks.iter().find(|t| t.location == location).unwrap().file.read().unwrap();
        assert_eq!(read("minecraft:records/cat"), ogg("cat"));
        assert_eq!(read("melodies:statech/end/theme"), ogg("end theme"));
        assert_eq!(read("loose:music/rain_walk"), ogg("rain_walk"));
        assert_eq!(read("discs:disc/sky"), ogg("sky"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_named_instance_wins_and_nothing_is_found_without_one() {
        let root = scratch("named");
        launcher(&root);
        let launcher = root.to_string_lossy().into_owned();
        let named = discover(&Place { launcher: launcher.clone(), instance: "GT_New_Horizons_2.8.0".into() }, &rules());
        assert_eq!(named.instance, Some(root.join("instances/GT_New_Horizons_2.8.0")));
        // That instance has only the store's tracks.
        assert_eq!(named.tracks.len(), 3);
        assert!(discover(&Place { launcher: launcher.clone(), instance: "Nope".into() }, &rules()).tracks.is_empty());
        let no_match = Discovery { instances: vec!["skyblock".into()], ..rules() };
        assert_eq!(discover(&Place { launcher, instance: String::new() }, &no_match).instance, None);
        let missing = discover(&Place { launcher: root.join("nowhere").to_string_lossy().into_owned(), instance: String::new() }, &rules());
        assert!(missing.instance.is_none() && missing.tracks.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
