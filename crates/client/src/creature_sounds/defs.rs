//! The creature sound bank as packages write it (`creature_sounds.ron`), and what it
//! resolves to: for every species, the clips of each sound it makes and how to play them.

use rustc_hash::FxHashMap;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;
use voxmar_engine::creature::{CreatureId, CreatureRegistry};

use super::Kind;

/// The file listing a package's creature voices.
pub const FILE: &str = "creature_sounds.ron";

/// One package's `creature_sounds.ron`.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BankFile {
    pub voices: FxHashMap<String, VoiceDef>,
    pub species: FxHashMap<String, SpeciesDef>,
}

/// A voice: the sounds an animal makes, shared by every species that uses it. Each
/// species pitches it to its own size.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VoiceDef {
    /// Another voice whose sounds this one takes for any it does not give itself.
    pub like: Option<String>,
    /// Height in blocks of the animal recorded, against which each species' own height
    /// pitches the voice: a smaller one higher, a larger lower.
    pub size: Option<f32>,
    /// Whether its steps sound the ground they fall on (false for things that fly, float
    /// or glide on slime). Unset: as `like`'s, else true.
    pub ground: Option<bool>,
    pub step: Option<EventDef>,
    pub idle: Option<EventDef>,
    pub graze: Option<EventDef>,
    pub alert: Option<EventDef>,
    pub rest: Option<EventDef>,
    pub call: Option<EventDef>,
    pub ready: Option<EventDef>,
    pub attack: Option<EventDef>,
    /// Unset, its `alert`.
    pub warn: Option<EventDef>,
    pub hurt: Option<EventDef>,
    pub death: Option<EventDef>,
    /// Barks, for a settlement's people: greeting, alarm and contentment.
    pub greet: Option<EventDef>,
    pub alarm: Option<EventDef>,
    pub content: Option<EventDef>,
}

impl VoiceDef {
    fn event(&self, kind: Kind) -> Option<&EventDef> {
        match kind {
            Kind::Step => self.step.as_ref(),
            Kind::Idle => self.idle.as_ref(),
            Kind::Graze => self.graze.as_ref(),
            Kind::Alert => self.alert.as_ref(),
            Kind::Rest => self.rest.as_ref(),
            Kind::Call => self.call.as_ref(),
            Kind::Ready => self.ready.as_ref(),
            Kind::Attack => self.attack.as_ref(),
            Kind::Warn => self.warn.as_ref(),
            Kind::Hurt => self.hurt.as_ref(),
            Kind::Death => self.death.as_ref(),
            Kind::Greet => self.greet.as_ref(),
            Kind::Alarm => self.alarm.as_ref(),
            Kind::Content => self.content.as_ref(),
        }
    }
}

/// One sound a voice makes: several recordings, one picked at random each time and
/// played a little higher or lower, so that no two in a row sound the same.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    /// Paths within the package.
    pub clips: Vec<String>,
    /// Loudness. Unset: the kind's usual loudness ([`Kind::gain`]), as clips are
    /// levelled alike and a munch is quieter than a bellow.
    pub gain: Option<f32>,
    /// The range a playback's pitch is picked from, around the species' own.
    #[serde(default = "pitch_spread")]
    pub pitch: (f32, f32),
    /// Blocks away at which it fades to nothing. Unset: the kind's usual reach.
    pub range: Option<f32>,
    /// The chance it sounds at all when the animal does this, for things it does often.
    #[serde(default = "one")]
    pub chance: f32,
}

fn one() -> f32 {
    1.0
}

fn pitch_spread() -> (f32, f32) {
    (0.93, 1.07)
}

/// Which voice a species speaks with, and how.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeciesDef {
    pub voice: String,
    /// Pitch against the voice's. Unset: from the species' height against the voice's
    /// `size`.
    #[serde(default)]
    pub pitch: Option<f32>,
    #[serde(default = "one")]
    pub gain: f32,
    /// How otherworldly it sounds, 0..1: aether creatures shimmer, echo and ring. Unset:
    /// [`AETHER`] for a supernatural species, else none.
    #[serde(default)]
    pub aether: Option<f32>,
}

/// How otherworldly a supernatural species sounds when its entry does not say.
pub const AETHER: f32 = 0.5;

/// A recording as the game plays it: a path within its package (`content/base/…`), at
/// some level of aether shimmer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClipKey {
    pub path: String,
    /// Tenths of shimmer; 0 plays the file as it is.
    pub aether: u8,
}

/// One kind of sound for one species, resolved.
#[derive(Clone, Debug)]
pub struct Event {
    /// Indices into [`Bank::clips`].
    pub clips: Vec<usize>,
    pub gain: f32,
    pub pitch: (f32, f32),
    pub range: f32,
    pub chance: f32,
}

/// How one species sounds.
#[derive(Clone, Debug)]
pub struct Voice {
    pub events: [Option<Event>; Kind::ALL.len()],
    /// Pitch for its size, before each individual's own.
    pub pitch: f32,
    pub gain: f32,
    /// Whether its steps sound the ground underfoot.
    pub ground: bool,
}

impl Voice {
    /// The event for `kind`, or the nearest one it has: a call for a missing alert, an
    /// idle sound for a missing call, and so on (see [`Kind::fallback`]).
    pub fn event(&self, kind: Kind) -> Option<&Event> {
        let mut kind = Some(kind);
        while let Some(k) = kind {
            if let Some(event) = &self.events[k as usize] {
                return Some(event);
            }
            kind = k.fallback();
        }
        None
    }
}

/// Every species' voice, and every clip they play.
#[derive(Debug, Default)]
pub struct Bank {
    /// By species id; `None` for a species no package gave a voice.
    pub species: Vec<Option<Voice>>,
    pub clips: Vec<ClipKey>,
}

impl Bank {
    /// The bank every package's `creature_sounds.ron` makes together: later packages
    /// replace earlier ones' voices and species by name, so a mod or sound pack can
    /// revoice anything. Species names without a package are the declaring package's.
    pub fn load(files: &[ContentFile], creatures: &CreatureRegistry) -> Result<Self, String> {
        let mut voices: FxHashMap<String, (String, VoiceDef)> = FxHashMap::default();
        let mut species: FxHashMap<String, SpeciesDef> = FxHashMap::default();
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        for file in files.iter().filter(|f| f.name == FILE) {
            let def: BankFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            for (name, voice) in def.voices {
                voices.insert(name, (file.package.clone(), voice));
            }
            for (name, entry) in def.species {
                let name = if name.contains(':') { name } else { format!("{}:{name}", file.package) };
                species.insert(name, entry);
            }
        }
        let mut bank = Bank { species: vec![None; creatures.len()], clips: Vec::new() };
        let mut index: FxHashMap<ClipKey, usize> = FxHashMap::default();
        for (name, entry) in &species {
            let Some(id) = creatures.id(name) else { continue };
            let creature = &creatures.get(id).def;
            let chain = chain(&voices, &entry.voice).map_err(|e| format!("{FILE}: species `{name}`: {e}"))?;
            let aether = entry.aether.unwrap_or(if creature.supernatural { AETHER } else { 0.0 }).clamp(0.0, 1.0);
            let aether = (aether * 10.0).round() as u8;
            let mut events: [Option<Event>; Kind::ALL.len()] = Default::default();
            for kind in Kind::ALL {
                let Some((package, def)) = chain.iter().find_map(|(package, voice)| voice.event(kind).map(|e| (package, e))) else { continue };
                let clips = def
                    .clips
                    .iter()
                    .map(|path| {
                        let key = ClipKey { path: format!("content/{package}/{path}"), aether };
                        *index.entry(key.clone()).or_insert_with(|| {
                            bank.clips.push(key);
                            bank.clips.len() - 1
                        })
                    })
                    .collect::<Vec<_>>();
                if clips.is_empty() {
                    continue;
                }
                let (low, high) = def.pitch;
                events[kind as usize] = Some(Event {
                    clips,
                    gain: def.gain.unwrap_or(kind.gain()).max(0.0),
                    pitch: (low.min(high), low.max(high)),
                    range: def.range.unwrap_or(kind.range()).max(1.0),
                    chance: def.chance.clamp(0.0, 1.0),
                });
            }
            let height = creature.body.size.1;
            let size = chain.iter().find_map(|(_, v)| v.size);
            let pitch = entry.pitch.unwrap_or_else(|| size.map_or(1.0, |size| pitch_for_size(size, height)));
            let ground = chain.iter().find_map(|(_, v)| v.ground).unwrap_or(true);
            bank.species[id.0 as usize] = Some(Voice { events, pitch, gain: entry.gain.max(0.0), ground });
        }
        Ok(bank)
    }

    /// Whether every clip a voice plays is there under `assets`, so a mistyped path fails
    /// at launch rather than leaving the animal silent.
    pub fn check_files(&self, assets: &std::path::Path) -> Result<(), String> {
        match self.clips.iter().find(|key| !assets.join(&key.path).is_file()) {
            Some(missing) => Err(format!("{FILE}: `{}` is not there", missing.path)),
            None => Ok(()),
        }
    }

    pub fn voice(&self, species: CreatureId) -> Option<&Voice> {
        self.species.get(species.0 as usize).and_then(Option::as_ref)
    }

    /// Settlements' people (kind `Villager`) whose voice lacks clips of its own for a bark:
    /// each would be heard as some other sound, a hum for an alarm. The launch fails on any.
    pub fn unbarked(&self, creatures: &CreatureRegistry) -> Vec<String> {
        let barks = [Kind::Greet, Kind::Alarm, Kind::Content];
        creatures
            .iter()
            .filter(|(_, c)| c.def.kind == voxmar_engine::creature::Kind::Villager)
            .filter(|(id, _)| self.voice(*id).is_none_or(|voice| barks.iter().any(|&k| voice.events[k as usize].is_none())))
            .map(|(_, c)| c.def.name.clone())
            .collect()
    }
}

/// A voice and those it is `like`, nearest first, each with its package.
fn chain<'a>(voices: &'a FxHashMap<String, (String, VoiceDef)>, name: &str) -> Result<Vec<(&'a str, &'a VoiceDef)>, String> {
    let mut chain = Vec::new();
    let mut next = Some(name);
    while let Some(name) = next {
        let (package, voice) = voices.get(name).ok_or_else(|| format!("no voice `{name}`"))?;
        if chain.len() > 8 {
            return Err(format!("voice `{name}` is `like` itself"));
        }
        chain.push((package.as_str(), voice));
        next = voice.like.as_deref();
    }
    Ok(chain)
}

/// How much higher (above 1) or lower a voice recorded from an animal `recorded` blocks
/// tall sounds from one `height` tall. A body's resonances scale with its size; the square
/// root keeps a mouse-sized wolf from squeaking like a mouse.
pub fn pitch_for_size(recorded: f32, height: f32) -> f32 {
    (recorded / height.max(0.05)).sqrt().clamp(0.5, 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(sources: &[(&str, &str)]) -> Vec<ContentFile> {
        sources.iter().map(|(package, source)| ContentFile { package: (*package).into(), name: FILE.into(), source: (*source).into() }).collect()
    }

    fn content() -> voxmar_engine::content::Content {
        voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap()
    }

    #[test]
    fn voices_inherit_and_fall_back() {
        let content = content();
        let creatures = &content.creatures;
        let bank = Bank::load(
            &files(&[(
                "base",
                r#"(
                    voices: {
                        "beast": (size: 1.0, idle: (clips: ["a.ogg", "b.ogg"]), hurt: (clips: ["h.ogg"], gain: 0.5), rest: (clips: ["r.ogg"])),
                        "small": (like: "beast", call: (clips: ["c.ogg"], range: 60.0)),
                    },
                    species: { "rabbit": (voice: "small") },
                )"#,
            )]),
            creatures,
        )
        .unwrap();
        let rabbit = bank.voice(creatures.find("rabbit").unwrap()).unwrap();
        let path = |kind| bank.clips[rabbit.event(kind).unwrap().clips[0]].path.clone();
        // Its own call, and the voice it is like's sounds.
        assert_eq!(path(Kind::Call), "content/base/c.ogg");
        assert_eq!(rabbit.event(Kind::Call).unwrap().range, 60.0);
        assert_eq!(path(Kind::Idle), "content/base/a.ogg");
        assert_eq!(path(Kind::Rest), "content/base/r.ogg");
        // An alert it lacks falls back to its call; a death to its hurt.
        assert_eq!(path(Kind::Alert), "content/base/c.ogg");
        assert_eq!(path(Kind::Death), "content/base/h.ogg");
        // Nothing for grazing: grazing is not heard rather than a stand-in.
        assert!(rabbit.event(Kind::Graze).is_none());
        // A rabbit is smaller than the voice's one-block animal, so higher.
        assert!(rabbit.pitch > 1.2, "{}", rabbit.pitch);
        // Species without an entry have no voice.
        assert!(bank.voice(creatures.find("mouflon").unwrap()).is_none());
    }

    #[test]
    fn later_packages_revoice() {
        let content = content();
        let creatures = &content.creatures;
        let bank = Bank::load(
            &files(&[
                ("base", r#"(voices: {"v": (idle: (clips: ["a.ogg"]))}, species: {"rabbit": (voice: "v")})"#),
                ("zmod", r#"(voices: {"w": (idle: (clips: ["b.ogg"]))}, species: {"base:rabbit": (voice: "w", aether: 1.0)})"#),
            ]),
            creatures,
        )
        .unwrap();
        let rabbit = bank.voice(creatures.find("rabbit").unwrap()).unwrap();
        let clip = &bank.clips[rabbit.event(Kind::Idle).unwrap().clips[0]];
        assert_eq!(clip, &ClipKey { path: "content/zmod/b.ogg".into(), aether: 10 });
        // Every shipped voice's clips are there; the mod's b.ogg is not, and fails the launch.
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        assert_eq!(Bank::load(&content.files, creatures).unwrap().check_files(&assets), Ok(()));
        assert!(bank.check_files(&assets).is_err());
    }

    #[test]
    fn unknown_voices_and_fields_are_errors() {
        let content = content();
        assert!(Bank::load(&files(&[("base", r#"(species: {"rabbit": (voice: "nobody")})"#)]), &content.creatures).is_err());
        assert!(Bank::load(&files(&[("base", r#"(voices: {"v": (idel: (clips: ["a.ogg"]))})"#)]), &content.creatures).is_err());
    }

    #[test]
    fn bigger_is_lower() {
        assert!(pitch_for_size(1.0, 2.0) < 1.0);
        assert!(pitch_for_size(1.0, 0.5) > 1.0);
        assert_eq!(pitch_for_size(1.0, 1.0), 1.0);
        assert_eq!(pitch_for_size(1.0, 0.0), 2.0);
    }
}
