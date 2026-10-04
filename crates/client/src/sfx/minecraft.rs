//! Block sounds from the player's own Minecraft install, when they have one. Nothing from
//! Minecraft is shipped or needed: the launcher keeps every sound in a content-addressed
//! store (`assets/objects/<hash prefix>/<hash>`), and an asset index names them. Sound
//! events (`block.grass.break`) come from that version's `sounds.json`, so the clips and
//! their volumes and pitches are exactly the game's.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use serde::Deserialize;

/// One clip an event may play, with the volume and pitch the event gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    /// Name in the store, `dig/grass1`.
    pub name: String,
    pub volume: f32,
    pub pitch: f32,
}

/// The newest asset store found in a standard launcher location.
pub struct SoundStore {
    objects: PathBuf,
    /// Hash of each sound file, by name (`dig/grass1`).
    files: FxHashMap<String, String>,
    events: FxHashMap<String, Vec<Clip>>,
}

#[derive(Deserialize)]
struct Index {
    objects: FxHashMap<String, Object>,
}

#[derive(Deserialize)]
struct Object {
    hash: String,
}

#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    sounds: Vec<Entry>,
}

/// A clip in `sounds.json`: a bare name, or a name with settings. `type: "event"` makes it
/// a reference to another event.
#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Name(String),
    Full {
        name: String,
        #[serde(default = "one")]
        volume: f32,
        #[serde(default = "one")]
        pitch: f32,
        #[serde(default = "one_u32")]
        weight: u32,
        #[serde(default, rename = "type")]
        kind: Option<String>,
    },
}

fn one() -> f32 {
    1.0
}

fn one_u32() -> u32 {
    1
}

impl SoundStore {
    /// The store of the newest asset index that has sounds, in the official launcher's or
    /// Prism Launcher's folder.
    pub fn discover() -> Option<Self> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)?;
        let mut roots = vec![
            home.join("Library/Application Support/minecraft/assets"),
            home.join(".minecraft/assets"),
            home.join("Library/Application Support/PrismLauncher/assets"),
            home.join(".local/share/PrismLauncher/assets"),
        ];
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            roots.push(appdata.join(".minecraft/assets"));
            roots.push(appdata.join("PrismLauncher/assets"));
        }
        let mut indexes: Vec<(IndexAge, PathBuf, PathBuf)> = Vec::new();
        for root in &roots {
            let Ok(entries) = std::fs::read_dir(root.join("indexes")) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(age) = name.strip_suffix(".json").and_then(IndexAge::parse) {
                    indexes.push((age, entry.path(), root.join("objects")));
                }
            }
        }
        indexes.sort();
        // The newest index whose store actually holds the sound events.
        indexes.into_iter().rev().find_map(|(_, index, objects)| Self::open(&index, objects))
    }

    fn open(index: &Path, objects: PathBuf) -> Option<Self> {
        let index: Index = serde_json::from_slice(&std::fs::read(index).ok()?).ok()?;
        let events = object(&objects, &index.objects.get("minecraft/sounds.json")?.hash)?;
        let events: FxHashMap<String, Event> = serde_json::from_slice(&std::fs::read(events).ok()?).ok()?;
        let files = index
            .objects
            .into_iter()
            .filter_map(|(name, o)| Some((name.strip_prefix("minecraft/sounds/")?.strip_suffix(".ogg")?.to_owned(), o.hash)))
            .collect();
        let mut store = Self { objects, files, events: FxHashMap::default() };
        store.events = events.keys().map(|name| (name.clone(), resolve(&events, name, 0))).collect();
        Some(store)
    }

    /// The clips of an event (`block.grass.break`), each listed as often as its weight;
    /// none if the event is unknown or its files are missing.
    pub fn event(&self, name: &str) -> Vec<Clip> {
        let mut clips = self.events.get(name).cloned().unwrap_or_default();
        clips.retain(|clip| self.files.contains_key(&clip.name));
        clips
    }

    /// The bytes of a clip's Ogg Vorbis file.
    pub fn read(&self, clip: &Clip) -> Option<Vec<u8>> {
        std::fs::read(object(&self.objects, self.files.get(&clip.name)?)?).ok()
    }
}

/// Where the store keeps a file: under the first two characters of its hash.
fn object(objects: &Path, hash: &str) -> Option<PathBuf> {
    Some(objects.join(hash.get(..2)?).join(hash))
}

/// An event's clips, following references to other events (a few levels deep at most).
fn resolve(events: &FxHashMap<String, Event>, name: &str, depth: u32) -> Vec<Clip> {
    let Some(event) = events.get(name.trim_start_matches("minecraft:")) else { return Vec::new() };
    let mut clips = Vec::new();
    for entry in &event.sounds {
        match entry {
            Entry::Name(name) => clips.push(Clip { name: clip_name(name), volume: 1.0, pitch: 1.0 }),
            Entry::Full { name, kind: Some(kind), .. } if kind == "event" => {
                if depth < 4 {
                    clips.extend(resolve(events, name, depth + 1));
                }
            }
            Entry::Full { name, volume, pitch, weight, .. } => {
                let clip = Clip { name: clip_name(name), volume: *volume, pitch: *pitch };
                clips.extend(std::iter::repeat_n(clip, (*weight).clamp(1, 8) as usize));
            }
        }
    }
    clips
}

fn clip_name(name: &str) -> String {
    name.trim_start_matches("minecraft:").to_owned()
}

/// How new an asset index is. Modern indexes are numbered (`17` for 1.21.1); older ones are
/// named by version (`1.7.10`) and come before every numbered one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum IndexAge {
    Version(Vec<u32>),
    Numbered(u32),
}

impl IndexAge {
    fn parse(name: &str) -> Option<Self> {
        if let Ok(n) = name.parse() {
            return Some(IndexAge::Numbered(n));
        }
        name.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<u32>>>().map(IndexAge::Version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_indexes_are_newest() {
        let mut ages: Vec<IndexAge> = ["5", "1.7.10", "17", "1.12"].iter().filter_map(|n| IndexAge::parse(n)).collect();
        ages.sort();
        assert_eq!(ages.last(), Some(&IndexAge::Numbered(17)));
        assert_eq!(ages.first(), Some(&IndexAge::Version(vec![1, 7, 10])));
        assert_eq!(IndexAge::parse("legacy"), None);
    }

    #[test]
    fn events_resolve_names_settings_weights_and_references() {
        let json = r#"{
            "block.grass.break": {"sounds": ["dig/grass1", {"name": "minecraft:dig/grass2", "volume": 0.5, "pitch": 0.8, "weight": 2}]},
            "block.moss.step": {"sounds": [{"name": "block.grass.break", "type": "event"}], "subtitle": "x"},
            "loop.a": {"sounds": [{"name": "loop.a", "type": "event"}]}
        }"#;
        let events: FxHashMap<String, Event> = serde_json::from_str(json).unwrap();
        let grass = resolve(&events, "block.grass.break", 0);
        assert_eq!(grass.len(), 3);
        assert_eq!(grass[0], Clip { name: "dig/grass1".into(), volume: 1.0, pitch: 1.0 });
        assert_eq!(grass[1], Clip { name: "dig/grass2".into(), volume: 0.5, pitch: 0.8 });
        assert_eq!(resolve(&events, "block.moss.step", 0), grass);
        assert!(resolve(&events, "loop.a", 0).is_empty());
    }
}
