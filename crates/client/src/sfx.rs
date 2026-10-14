//! Sound effects for block materials, thunder, swimming and splashing, and what the player
//! does (eating, crafting, lighting a fire, opening a door: `sounds.ron`'s `actions`).
//!
//! A block declares what it is made of (`sound: Wood`); every cue for that material (a
//! step, landing, placing, a tap while mining, breaking) comes from the player's own
//! Minecraft install when they have one and allow it (the `minecraft_assets` setting; see
//! `minecraft`), played with the volume and pitch Minecraft gives that use of it, so grass,
//! flowers, dirt and stone sound exactly as they do there. Without an install, without the
//! setting, or for a cue the install lacks, the cue is synthesized at startup from the
//! material.
//!
//! Sound philosophy, for the synthesized cues:
//! - Sounds come from materials, not from individual blocks. A block only declares what it
//!   is made of (`sound: Wood`), so a mod's new block sounds right without new audio.
//! - Quiet, short and warm. The world is calm; effects confirm what the player did and never
//!   compete with the music. Nothing is harsh or high-pitched for long.
//! - Never identical twice. Every cue has several synthesized variants and is played with a
//!   small random pitch change, so repetition (footsteps above all) does not grate.
//! - One recipe per material, varied per event: a step is the material's short signature,
//!   placing adds a soft low thump, breaking is longer and lower, mining taps are quieter.
//! - Deterministic generation, like textures: a material always sounds the same on every
//!   machine, and nothing is downloaded or shipped.
//!
//! Thunder, swimming strokes and splashes are field recordings (CC0, credited in
//! THIRD_PARTY_NOTICES.md), listed in each package's `sounds.ron` and loaded from the
//! package like any asset: close strikes crack and roll, farther ones are muffled, far ones
//! only roll, each arriving as late as sound takes to travel. Water falls back to a
//! synthesized swash and splash (after Andy Farnell's *Designing Sound*) for a package
//! that lists none. Rain is the ambience's (`ambience.ron`), layered by strength and cover.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use bevy::audio::{AudioPlayer, AudioSource, GlobalVolume, PlaybackSettings, SpatialScale, Volume};
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, Sound};
use voxmar_engine::station::{Phase, Stations};
use voxmar_engine::physics::player as dims;

use self::dsp::{OnePole, Pink, Svf};
use self::minecraft::SoundStore;
use crate::mix::{Category, PAN_SCALE, Voice, falloff};
use crate::player::LocalPlayer;
use crate::session::Launch;
use crate::settings::Settings;
use crate::weather::Thunder;
use crate::world::ClientWorld;

pub(crate) mod dsp;
mod minecraft;

/// Sample rate of every synthesized sound, shared with `ambience`.
pub(crate) const RATE: u32 = 44_100;
const VARIANTS: u64 = 4;
/// Volume at 100% in the settings.
pub(crate) const MASTER: f32 = 0.6;
/// Thunder's volume from lightning right overhead.
const THUNDER_GAIN: f32 = 0.9;
/// Lightning closer than the first of these many blocks cracks before it rumbles; closer
/// than the second, it tears muffled before it rolls; beyond, only the roll is heard.
const NEAR_LIGHTNING: f32 = 250.0;
const FAR_LIGHTNING: f32 = 900.0;
/// The file listing a package's recorded sounds.
const RECORDINGS: &str = "sounds.ron";
/// Cues placed in the world fade out over this many blocks from the listener, as
/// Minecraft's block sounds do, along the curve every sound in the world fades by
/// ([`falloff`]).
const HEARD_WITHIN: f32 = 16.0;
/// Seconds of the loudest stretch of a clip, by which clips are levelled.
const LOUDNESS_WINDOW: f32 = 0.05;

pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SoundCue>()
            .configure_sets(Update, Audible.run_if(sound_on))
            .add_systems(Startup, synthesize)
            .add_systems(Update, ((footsteps, play_cues.in_set(Audible)).chain(), thunder.in_set(Audible), (play_actions, play_brought).chain()));
    }
}

/// Systems that start sounds (here, in the music and in the interface). They run only while sound is on: a capture, or any run with VOXMAR_MUTE set,
/// has a global volume of zero (see main.rs) and so never starts a sound at all, whatever
/// volume it would have been given. The global volume still scales every sound otherwise.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Audible;

/// Whether sound is on: the global volume is above zero.
pub fn sound_on(global: Option<Res<GlobalVolume>>) -> bool {
    global.is_some_and(|g| g.volume.to_linear() > 0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    Step,
    /// Landing from a fall.
    Land,
    Place,
    Break,
    /// One tap while mining.
    Hit,
    /// A dropped item picked up: the same pop whatever it is made of.
    Pickup,
}

impl Cue {
    const ALL: [Cue; 6] = [Cue::Step, Cue::Land, Cue::Place, Cue::Break, Cue::Hit, Cue::Pickup];

    fn gain(self) -> f32 {
        match self {
            Cue::Step => 0.3,
            Cue::Land => 0.4,
            Cue::Place => 0.55,
            Cue::Break => 0.6,
            Cue::Hit => 0.22,
            Cue::Pickup => 0.3,
        }
    }
}

/// Request to play a cue for a material.
#[derive(Message, Clone, Copy, Debug)]
pub struct SoundCue {
    pub sound: Sound,
    pub cue: Cue,
    /// Where it sounds from, if not at the listener; it fades out with distance.
    pub at: Option<Vec3>,
    /// Multiplies the cue's usual volume.
    pub gain: f32,
    /// Multiplies its pitch (and speed): a small creature's steps are quicker and higher.
    pub pitch: f32,
}

impl SoundCue {
    pub fn new(sound: Sound, cue: Cue) -> Self {
        Self { sound, cue, at: None, gain: 1.0, pitch: 1.0 }
    }

    pub fn for_block(world: &ClientWorld, block: BlockId, cue: Cue) -> Self {
        Self::of(&world.registry, block, cue)
    }

    pub fn of(registry: &BlockRegistry, block: BlockId, cue: Cue) -> Self {
        Self::new(registry.info(block).sound, cue)
    }

    /// Sounding from `at` in the world.
    pub fn at(self, at: Vec3) -> Self {
        Self { at: Some(at), ..self }
    }

    /// At `gain` times the usual volume.
    pub fn scaled(self, gain: f32) -> Self {
        Self { gain: self.gain * gain, ..self }
    }

    /// At `pitch` times the usual pitch.
    pub fn pitched(self, pitch: f32) -> Self {
        Self { pitch: self.pitch * pitch, ..self }
    }
}

/// What a station changing from `old` to `new` sounds like, at the block: loaded (empty to
/// working), its own sound or its working block's placing; emptied (ready or working back to
/// empty), the pickup's pop. Played when the server says so, never predicted, so a refused
/// load is silent.
pub fn station_cue(stations: &Stations, registry: &BlockRegistry, old: BlockId, new: BlockId) -> Option<SoundCue> {
    let (station, was) = stations.of(old)?;
    let (_, now) = stations.of(new).filter(|(s, _)| std::ptr::eq(*s, station))?;
    match (was, now) {
        (Phase::Empty, Phase::Working) => Some(match station.sound {
            Some(sound) => SoundCue::new(sound, Cue::Step),
            None => SoundCue::of(registry, station.working, Cue::Place),
        }),
        (Phase::Working | Phase::Ready, Phase::Empty) => Some(SoundCue::new(Sound::Stone, Cue::Pickup)),
        _ => None,
    }
}

/// One clip a cue may play, and how loud and fast.
#[derive(Clone)]
struct Clip {
    source: Handle<AudioSource>,
    gain: f32,
    speed: f32,
}

/// Result sounds by action: each with the tag its items must carry (none: any item).
type Brings = FxHashMap<voxmar_engine::action::ActionId, Vec<(Option<String>, Vec<Clip>)>>;

#[derive(Resource)]
struct Sfx {
    clips: FxHashMap<(Sound, Cue), Vec<Clip>>,
    /// Thunder from near, middling and far lightning, a few variants each.
    thunder: [Vec<Handle<AudioSource>>; 3],
    /// What actions sound like as the player does them.
    actions: FxHashMap<voxmar_engine::action::ActionId, Vec<Clip>>,
    /// What their results sound like landing in the pack, by tag (none: anything).
    brings: Brings,
    /// Actions done whose results may yet land, and until when (seconds).
    awaiting: Vec<(voxmar_engine::action::ActionId, f32)>,
    counter: u64,
}

impl Sfx {
    fn random(&mut self) -> f32 {
        self.counter += 1;
        (mix(self.counter) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The Minecraft sound event a cue plays for a material, with the volume and pitch
/// Minecraft gives that use of a block's sounds.
fn minecraft_event(sound: Sound, cue: Cue) -> Option<(String, f32, f32)> {
    // Minecraft pops at 0.2 and about twice the clip's pitch, whatever was picked up.
    if cue == Cue::Pickup {
        return Some(("entity.item.pickup".to_owned(), 0.2, 2.0));
    }
    let kind = match sound {
        Sound::Stone => "stone",
        Sound::Wood => "wood",
        // Minecraft's dirt, clay and podzol sound like its gravel.
        Sound::Soil | Sound::Gravel => "gravel",
        // Its leaves, plants and flowers like its grass block.
        Sound::Grass | Sound::Leaves => "grass",
        Sound::Sand => "sand",
        Sound::Snow => "snow",
        Sound::Glass => "glass",
        Sound::Mud => "mud",
        Sound::Moss => "moss",
        Sound::Water => {
            return match cue {
                Cue::Step => Some(("entity.player.swim".to_owned(), 0.2, 1.0)),
                Cue::Land => Some(("entity.player.splash".to_owned(), 0.3, 1.0)),
                _ => None,
            };
        }
    };
    let (event, volume, pitch) = match cue {
        Cue::Step => ("step", 0.15, 1.0),
        Cue::Land => ("fall", 0.5, 0.75),
        Cue::Place => ("place", 1.0, 0.8),
        Cue::Break => ("break", 1.0, 0.8),
        Cue::Hit => ("hit", 0.25, 0.5),
        Cue::Pickup => unreachable!("answered above"),
    };
    Some((format!("block.{kind}.{event}"), volume, pitch))
}

/// Loads each cue's clips from the player's Minecraft install where it has them (with the
/// `minecraft_assets` setting on), and synthesizes the rest.
///
/// Minecraft's clips are levelled to the loudness of our synthesized ones for the same
/// cue: Minecraft plays a step at 0.15 of a clip already quieter than its break, which
/// under music and ambience was lost altogether. Within a cue's clips their own volumes
/// and Minecraft's variation stay; across materials each cue sounds about as loud.
fn synthesize(
    mut commands: Commands,
    mut sources: ResMut<Assets<AudioSource>>,
    assets: Res<AssetServer>,
    launch: Res<Launch>,
    settings: Res<crate::settings::Settings>,
) {
    let started = std::time::Instant::now();
    let recorded = match Recordings::load(&launch.content.files) {
        Ok(recorded) => recorded,
        Err(e) => {
            error!("no recorded sounds: {e}");
            Recordings::default()
        }
    };
    let load = |paths: &[String]| paths.iter().map(|path| assets.load::<AudioSource>(path.clone())).collect::<Vec<_>>();
    // The player's own Minecraft sounds only when they opted in.
    let store = if settings.minecraft_assets { SoundStore::discover() } else { None };
    // Hits, steps and falls share files; each is read and measured once.
    let mut loaded: FxHashMap<String, Option<(Handle<AudioSource>, f32)>> = FxHashMap::default();
    let mut clips = FxHashMap::default();
    let mut borrowed = 0;
    // How loud each cue's synthesized clips are, to level borrowed ones to.
    let mut target: FxHashMap<Cue, (f32, f32)> = FxHashMap::default();
    let mut synthesized: FxHashMap<(Sound, Cue), Vec<Vec<f32>>> = FxHashMap::default();
    for sound in Sound::ALL {
        for cue in Cue::ALL {
            let variants: Vec<Vec<f32>> = (0..VARIANTS).map(|v| render(sound, cue, mix(sound as u64 * 131 + cue as u64 * 17 + v))).collect();
            let entry = target.entry(cue).or_default();
            for samples in &variants {
                *entry = (entry.0 + loudness(samples, 1), entry.1 + 1.0);
            }
            synthesized.insert((sound, cue), variants);
        }
    }
    for sound in Sound::ALL {
        for cue in Cue::ALL {
            // Water's recordings come first, then Minecraft's clips, then synthesis.
            let mut variants: Vec<Clip> = match (sound, cue) {
                (Sound::Water, Cue::Step) => load(&recorded.swim),
                (Sound::Water, Cue::Land) => load(&recorded.splash),
                _ => Vec::new(),
            }
            .into_iter()
            .map(|source| Clip { source, gain: cue.gain() * RECORDED_GAIN, speed: 1.0 })
            .collect();
            let recorded_here = !variants.is_empty();
            if let (false, Some(store), Some((event, volume, pitch))) = (recorded_here, &store, minecraft_event(sound, cue)) {
                let event = store.event(&event);
                let measured: Vec<_> = event
                    .iter()
                    .filter_map(|clip| {
                        let entry = loaded.entry(clip.name.clone()).or_insert_with(|| {
                            let source = AudioSource { bytes: Arc::from(store.read(clip)?) };
                            let level = measure(&source)?;
                            Some((sources.add(source), level))
                        });
                        entry.clone().map(|(source, level)| (clip, source, level))
                    })
                    .collect();
                // The event's clips as Minecraft plays them, levelled together.
                let level = measured.iter().map(|(clip, _, level)| level * clip.volume * volume).sum::<f32>() / measured.len().max(1) as f32;
                let (sum, count) = target[&cue];
                let gain = cue.gain() * (sum / count) / level.max(1e-4);
                for (clip, source, _) in measured {
                    variants.push(Clip { source, gain: gain * clip.volume * volume, speed: pitch * clip.pitch });
                }
            }
            // Our own pop for a pickup Minecraft's sounds do not give, as loud as the one
            // synthesized.
            if variants.is_empty() && cue == Cue::Pickup && !recorded.pickup.is_empty() {
                let gain = cue.gain() * level(&target, cue) / ACTION_LOUDNESS;
                variants = load(&recorded.pickup).into_iter().map(|source| Clip { source, gain, speed: 1.0 }).collect();
            } else if recorded_here {
            } else if variants.is_empty() {
                variants = synthesized[&(sound, cue)]
                    .iter()
                    .map(|samples| Clip { source: sources.add(AudioSource { bytes: Arc::from(wav(samples)) }), gain: cue.gain(), speed: 1.0 })
                    .collect();
            } else {
                borrowed += 1;
            }
            clips.insert((sound, cue), variants);
        }
    }
    info!(
        "block sounds: {borrowed} of {} cues from Minecraft's assets, the rest synthesized ({} ms)",
        clips.len(),
        started.elapsed().as_millis()
    );
    let thunder = [&recorded.thunder.near, &recorded.thunder.middle, &recorded.thunder.far].map(|paths| load(paths));
    // What the player does sounds about as loud as placing a block, times its volume.
    let actions = recorded
        .actions
        .iter()
        .filter_map(|(name, sound)| {
            let gain = sound.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
            Some((launch.content.actions.id(name)?, load(&sound.sounds).into_iter().map(|source| Clip { source, gain, speed: 1.0 }).collect()))
        })
        .collect();
    let brings = recorded
        .actions
        .iter()
        .filter(|(_, sound)| !sound.brings.is_empty())
        .filter_map(|(name, sound)| {
            let rules = sound.brings.iter().map(|brought| {
                let gain = brought.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
                (brought.tag.clone(), load(&brought.sounds).into_iter().map(|source| Clip { source, gain, speed: 1.0 }).collect())
            });
            Some((launch.content.actions.id(name)?, rules.collect()))
        })
        .collect();
    commands.insert_resource(Sfx { clips, thunder, actions, brings, awaiting: Vec::new(), counter: 0 });
}

/// How loud a synthesized cue's clips are on average.
fn level(target: &FxHashMap<Cue, (f32, f32)>, cue: Cue) -> f32 {
    let (sum, count) = target[&cue];
    sum / count.max(1.0)
}

/// How loud our own action and pickup clips are made (tools/generate_sfx.py), by
/// [`loudness`]; a package's own should be made as loud.
pub(crate) const ACTION_LOUDNESS: f32 = 0.22;

/// Recorded swimming strokes and splashes are levelled to a peak of -3 dBFS; this brings
/// them to the loudness of the synthesized cues.
const RECORDED_GAIN: f32 = 0.6;

/// Every package's `sounds.ron`: recorded sounds for events, as paths within the package.
/// Later packages add to the lists.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Recordings {
    thunder: ThunderRecordings,
    /// Strokes while swimming.
    swim: Vec<String>,
    /// Jumping or falling into water.
    splash: Vec<String>,
    /// The jingles of events (`ui::sound`): a quest finished, a quest's rewards claimed, a
    /// skill's level reached, and a milestone level reached (which levels are is
    /// `celebrations.ron`'s). A later package that lists any for an event replaces the
    /// earlier ones: a jingle is one tune, not a pool to add to.
    pub(crate) quest_done: Vec<String>,
    pub(crate) quest_claim: Vec<String>,
    pub(crate) level_up: Vec<String>,
    pub(crate) level_milestone: Vec<String>,
    /// An item picked up off the ground, played when the player's Minecraft sounds are not.
    /// A later package that lists any replaces these.
    pickup: Vec<String>,
    /// What the player does, heard as they do it, by the action's name (`core:eat`): the
    /// clips take turns at random. A later package that names an action replaces its sound.
    pub(crate) actions: std::collections::BTreeMap<String, ActionSound>,
    /// What the interface sounds like, by what happens in it (`open`, `refuse`; see
    /// `ui::sound::UiSound`). A later package that names one replaces its sound. Every one
    /// says how it sounds, or the game does not start.
    pub(crate) interface: std::collections::BTreeMap<String, InterfaceSound>,
    /// How loud each kind of sound is, how many play at once and what the music makes
    /// room for (`mix`).
    pub(crate) mix: crate::mix::MixDef,
}

/// The sound of one action: its clips, made at the loudness of [`ACTION_LOUDNESS`], and how
/// loud to play them beside it; or that it is meant to be silent, or is heard from another
/// part of the game. Every action says one of the three.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ActionSound {
    pub(crate) sounds: Vec<String>,
    #[serde(default = "full")]
    pub(crate) volume: f32,
    /// Makes no sound, on purpose.
    pub(crate) silent: bool,
    /// Heard from elsewhere instead.
    pub(crate) heard: Option<HeardBy>,
    /// What it sounds like when what it brings lands in the pack (a fish landed, food off
    /// the fire done or burnt): the first whose `tag` the item has, or with none, any.
    pub(crate) brings: Vec<Brought>,
}

/// The sound of an action's result arriving in the pack.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Brought {
    /// A tag the item brought has (`burnt`); none: anything it brings.
    pub(crate) tag: Option<String>,
    pub(crate) sounds: Vec<String>,
    #[serde(default = "full")]
    pub(crate) volume: f32,
}

/// The sound of something happening in the interface: the player's chosen set's click or
/// item tap (`tap`), clips of its own, or a jingle (the events' tunes above); or that it is
/// meant to be silent. Every one says one of the four. Its own clips, like the set's, are
/// not heard with the interface's sounds turned off.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct InterfaceSound {
    pub(crate) tap: Option<Tap>,
    pub(crate) sounds: Vec<String>,
    /// Played by its event's jingle (`quest_done`, `quest_claim`, `level_up`,
    /// `level_milestone`).
    pub(crate) jingle: bool,
    pub(crate) silent: bool,
    #[serde(default = "full")]
    pub(crate) volume: f32,
    /// How fast it is played: higher is higher and shorter.
    #[serde(default = "full")]
    pub(crate) pitch: f32,
}

/// Which of the player's set's two sounds an interface sound plays.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum Tap {
    Click,
    Item,
}

/// Where an action's sound comes from when not from `sounds.ron`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum HeardBy {
    /// The block it touches, by its material's cues (mining, placing, sowing, picking).
    Block,
    /// The interface's clicks and taps (moving items about the screens).
    Interface,
    /// The fight's own sounds (swings, blows, dodges).
    Combat,
}

fn full() -> f32 {
    1.0
}

/// Thunder by how far off the lightning struck (see [`Reach`]).
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThunderRecordings {
    near: Vec<String>,
    middle: Vec<String>,
    far: Vec<String>,
}

impl Recordings {
    /// The lists of every package, each path made an asset path (`content/base/sounds/…`).
    pub(crate) fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut all = Recordings::default();
        for file in files.iter().filter(|f| f.name == RECORDINGS) {
            let def: Recordings = ron::from_str(&file.source).map_err(|e| format!("{}/{RECORDINGS}: {e}", file.package))?;
            let path = |list: Vec<String>| list.into_iter().map(|p| format!("content/{}/{p}", file.package));
            all.thunder.near.extend(path(def.thunder.near));
            all.thunder.middle.extend(path(def.thunder.middle));
            all.thunder.far.extend(path(def.thunder.far));
            all.swim.extend(path(def.swim));
            all.splash.extend(path(def.splash));
            let replace = |all: &mut Vec<String>, list: Vec<String>| {
                if !list.is_empty() {
                    *all = path(list).collect();
                }
            };
            replace(&mut all.quest_done, def.quest_done);
            replace(&mut all.quest_claim, def.quest_claim);
            replace(&mut all.level_up, def.level_up);
            replace(&mut all.level_milestone, def.level_milestone);
            replace(&mut all.pickup, def.pickup);
            for (name, sound) in def.actions {
                let brings = sound.brings.into_iter().map(|b| Brought { sounds: path(b.sounds).collect(), ..b }).collect();
                all.actions.insert(name, ActionSound { sounds: path(sound.sounds).collect(), brings, ..sound });
            }
            for (name, sound) in def.interface {
                all.interface.insert(name, InterfaceSound { sounds: path(sound.sounds).collect(), ..sound });
            }
            all.mix.merge(def.mix);
        }
        Ok(all)
    }

    /// Whether every action given a sound is one the content has, with clips, at a volume
    /// from above nothing to twice the usual.
    pub(crate) fn check(&self, actions: &voxmar_engine::action::ActionRegistry) -> Result<(), String> {
        for (name, sound) in &self.actions {
            if actions.id(name).is_none() {
                return Err(format!("{RECORDINGS}: a sound for `{name}`, which is no action"));
            }
            let says = [!sound.sounds.is_empty(), sound.silent, sound.heard.is_some()].into_iter().filter(|&s| s).count();
            if says != 1 {
                return Err(format!("{RECORDINGS}: `{name}` says {says} of sounds, silent and heard; it says one"));
            }
            if !(sound.volume > 0.0 && sound.volume <= 2.0) {
                return Err(format!("{RECORDINGS}: `{name}` has volume {}, not above 0 and up to 2", sound.volume));
            }
            for brought in &sound.brings {
                if brought.sounds.is_empty() || !(brought.volume > 0.0 && brought.volume <= 2.0) {
                    return Err(format!("{RECORDINGS}: what `{name}` brings needs sounds, at a volume above 0 and up to 2"));
                }
                if brought.tag.as_deref() == Some("") {
                    return Err(format!("{RECORDINGS}: what `{name}` brings has an empty tag; leave it out for anything"));
                }
            }
        }
        for (name, sound) in &self.interface {
            let Some(event) = crate::ui::sound::UiSound::named(name) else {
                return Err(format!("{RECORDINGS}: an interface sound for `{name}`, which the interface never makes"));
            };
            let says = [sound.tap.is_some(), !sound.sounds.is_empty(), sound.jingle, sound.silent].into_iter().filter(|&s| s).count();
            if says != 1 {
                return Err(format!("{RECORDINGS}: interface `{name}` says {says} of tap, sounds, jingle and silent; it says one"));
            }
            if sound.jingle != event.is_jingle() {
                let jingles = crate::ui::sound::UiSound::ALL.iter().filter(|e| e.is_jingle()).map(|e| e.name()).collect::<Vec<_>>().join(", ");
                return Err(format!("{RECORDINGS}: interface `{name}`: only {jingles} are jingles, and they are"));
            }
            if !(sound.volume > 0.0 && sound.volume <= 2.0) {
                return Err(format!("{RECORDINGS}: interface `{name}` has volume {}, not above 0 and up to 2", sound.volume));
            }
            if !(0.5..=2.0).contains(&sound.pitch) {
                return Err(format!("{RECORDINGS}: interface `{name}` has pitch {}, not from 0.5 to 2", sound.pitch));
            }
        }
        Ok(())
    }

    /// Whether every action the content has says how it sounds (`actions`): its clips, or
    /// that it is silent or heard from elsewhere, so a new action is never mute by
    /// oversight; and so does everything the interface does (`interface`).
    pub(crate) fn covers(&self, actions: &voxmar_engine::action::ActionRegistry) -> Result<(), String> {
        let missing: Vec<&str> = actions.names().filter(|name| !self.actions.contains_key(*name)).collect();
        if !missing.is_empty() {
            return Err(format!("{RECORDINGS}: no sound for {}; give each `sounds`, `silent: true` or `heard`", missing.join(", ")));
        }
        // And everything the interface does, likewise.
        let missing: Vec<&str> = crate::ui::sound::UiSound::ALL.iter().map(|e| e.name()).filter(|name| !self.interface.contains_key(*name)).collect();
        if !missing.is_empty() {
            return Err(format!("{RECORDINGS}: no interface sound for {}; give each `tap`, `sounds`, `jingle: true` or `silent: true`", missing.join(", ")));
        }
        Ok(())
    }
}

/// How loud a clip is: the RMS of its loudest stretch of [`LOUDNESS_WINDOW`], so short
/// clicks and long tails compare fairly. `samples` interleave `channels`.
pub(crate) fn loudness(samples: &[f32], channels: usize) -> f32 {
    let window = ((LOUDNESS_WINDOW * RATE as f32) as usize * channels).max(1);
    let hop = (window / 4).max(1);
    let mut loudest = 0.0f32;
    let mut start = 0;
    loop {
        let stretch = &samples[start.min(samples.len())..(start + window).min(samples.len())];
        if !stretch.is_empty() {
            loudest = loudest.max(stretch.iter().map(|s| s * s).sum::<f32>() / stretch.len() as f32);
        }
        if start + window >= samples.len() {
            break;
        }
        start += hop;
    }
    loudest.sqrt()
}

/// How loud a sound that goes on is (music, a bed): the RMS of all of it.
#[cfg(test)]
pub(crate) fn mean_loudness(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| (s * s) as f64).sum::<f64>() / samples.len().max(1) as f64).sqrt() as f32
}

/// The loudness of an encoded clip, decoding it once.
fn measure(source: &AudioSource) -> Option<f32> {
    use bevy::audio::{Decodable, Source};
    let decoder = source.decoder();
    let channels = decoder.channels().get() as usize;
    let samples: Vec<f32> = decoder.collect();
    (!samples.is_empty()).then(|| loudness(&samples, channels))
}

/// How far off lightning is, for which thunder it makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Near,
    Middle,
    Far,
}

impl Reach {
    fn of(distance: f32) -> Self {
        if distance < NEAR_LIGHTNING {
            Reach::Near
        } else if distance < FAR_LIGHTNING {
            Reach::Middle
        } else {
            Reach::Far
        }
    }
}


/// How loud thunder is from lightning `distance` blocks off: half as loud 400 blocks away,
/// and on, never quite gone.
fn thunder_gain(distance: f32) -> f32 {
    THUNDER_GAIN / (1.0 + distance / 400.0)
}

/// Plays thunder as it reaches the player, quieter the farther the lightning, and indoors.
fn thunder(
    mut commands: Commands,
    mut heard: MessageReader<Thunder>,
    sfx: Option<ResMut<Sfx>>,
    settings: Res<Settings>,
    surroundings: Option<Res<crate::ambience::Surroundings>>,
) {
    let Some(mut sfx) = sfx else { return };
    let master = MASTER * settings.sound_volume as f32 / 100.0 * surroundings.map_or(1.0, |s| s.thunder);
    for thunder in heard.read() {
        let (pick, jitter) = (sfx.random(), sfx.random());
        let clips = &sfx.thunder[Reach::of(thunder.distance) as usize];
        let Some(clip) = clips.get((pick * clips.len() as f32) as usize).cloned() else { continue };
        let speed = 0.95 + jitter * 0.1;
        let volume = Volume::Linear(thunder_gain(thunder.distance) * master);
        commands.spawn((AudioPlayer(clip), PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(volume) }, Voice::of(Category::Weather)));
    }
}

fn play_cues(
    mut commands: Commands,
    mut cues: MessageReader<SoundCue>,
    sfx: Option<ResMut<Sfx>>,
    settings: Res<Settings>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
) {
    let Some(mut sfx) = sfx else { return };
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    let listener = listener.single().map(|t| t.translation()).ok();
    for cue in cues.read() {
        let distance = cue.at.zip(listener).map_or(0.0, |(at, ear)| at.distance(ear));
        let gain = cue.gain * falloff(distance, HEARD_WITHIN);
        if gain <= 0.0 {
            continue;
        }
        let (pick, jitter) = (sfx.random(), sfx.random());
        let Some(clip) = sfx.clips.get(&(cue.sound, cue.cue)).and_then(|clips| clips.get((pick * clips.len() as f32) as usize))
        else {
            continue;
        };
        let speed = clip.speed * cue.pitch * (0.94 + 0.12 * jitter);
        let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(clip.gain * gain * master)) };
        match cue.at {
            // Placed in the world: panned to the nearer ear, as a creature's call is.
            Some(at) => commands.spawn((
                AudioPlayer(clip.source.clone()),
                PlaybackSettings { spatial: true, spatial_scale: Some(SpatialScale::new(PAN_SCALE)), ..playback },
                Transform::from_translation(at),
                Voice::at(Category::Everyday, at),
            )),
            None => commands.spawn((AudioPlayer(clip.source.clone()), playback, Voice::of(Category::Everyday))),
        };
    }
}

/// Plays what the player's proposed actions sound like (`sounds.ron`'s `actions`): eating,
/// crafting, lighting a fire, opening a chest or a door. Proposals are taken every frame,
/// so none pile up while sound is off; muted, none is heard. An action whose results have
/// a sound waits [`BROUGHT_WITHIN`] for them (`play_brought`).
fn play_actions(
    mut commands: Commands,
    time: Option<Res<Time>>,
    net: Option<Res<crate::net::Net>>,
    sfx: Option<ResMut<Sfx>>,
    settings: Res<Settings>,
    global: Option<Res<GlobalVolume>>,
) {
    let Some(net) = net else { return };
    let proposed = net.take_proposed();
    let (Some(mut sfx), true) = (sfx, sound_on(global)) else { return };
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    let now = time.map_or(0.0, |time| time.elapsed_secs());
    for action in proposed {
        if sfx.brings.contains_key(&action) {
            sfx.awaiting.push((action, now + BROUGHT_WITHIN));
        }
        let (pick, jitter) = (sfx.random(), sfx.random());
        let Some(clip) = sfx.actions.get(&action).and_then(|clips| clips.get((pick * clips.len() as f32) as usize)) else { continue };
        play_clip(&mut commands, clip, master, jitter);
    }
}

/// Seconds after an action is done that what it brings may still land and be heard: a
/// line's five seconds in the water, and the server's word coming back.
const BROUGHT_WITHIN: f32 = 10.0;

fn play_clip(commands: &mut Commands, clip: &Clip, master: f32, jitter: f32) {
    let speed = clip.speed * (0.96 + 0.08 * jitter);
    let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(clip.gain * master)) };
    commands.spawn((AudioPlayer(clip.source.clone()), playback, Voice::of(Category::Everyday)));
}

/// How many of each item the player carries: the pack, the pointer and the pouches.
fn carried(inventory: &voxmar_engine::inventory::Inventory) -> FxHashMap<BlockId, u32> {
    let mut counts = FxHashMap::default();
    for stack in inventory.slots.iter().chain([&inventory.cursor]).flatten() {
        *counts.entry(stack.item).or_default() += stack.count as u32;
    }
    for &(item, count) in &inventory.pouched {
        *counts.entry(item).or_default() += count as u32;
    }
    counts
}

/// The items carried more of `now` than `before`.
fn gained(before: &FxHashMap<BlockId, u32>, now: &FxHashMap<BlockId, u32>) -> Vec<BlockId> {
    let mut more: Vec<BlockId> = now.iter().filter(|(item, n)| before.get(item).is_none_or(|b| b < n)).map(|(item, _)| *item).collect();
    more.sort_by_key(|item| item.0);
    more
}

/// Which awaited action an item landing in the pack answers, and with which clips: the
/// first awaited whose rules take it, by its first rule whose tag the item has.
fn answered<'a>(
    awaiting: &[(voxmar_engine::action::ActionId, f32)],
    brings: &'a Brings,
    has: impl Fn(&str) -> bool,
) -> Option<(usize, &'a [Clip])> {
    awaiting.iter().enumerate().find_map(|(i, (action, _))| {
        let rules = brings.get(action)?;
        rules.iter().find(|(tag, _)| tag.as_deref().is_none_or(&has)).map(|(_, clips)| (i, clips.as_slice()))
    })
}

/// Plays what an action brings as it lands in the pack (a fish landed, food off the fire
/// done or burnt), once, for the action awaiting it.
#[allow(clippy::too_many_arguments)]
fn play_brought(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    launch: Option<Res<Launch>>,
    sfx: Option<ResMut<Sfx>>,
    settings: Res<Settings>,
    global: Option<Res<GlobalVolume>>,
    mut last: Local<Option<FxHashMap<BlockId, u32>>>,
) {
    let Some(mut sfx) = sfx else { return };
    let now = time.elapsed_secs();
    sfx.awaiting.retain(|&(_, until)| until > now);
    if !player.is_changed() {
        return;
    }
    let counts = carried(&player.inventory);
    let (Some(before), Some(launch)) = (last.replace(counts.clone()), launch) else { return };
    if sfx.awaiting.is_empty() || !sound_on(global) {
        return;
    }
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    for item in gained(&before, &counts) {
        let def = launch.content.blocks.def(item);
        let Some((at, clips)) = answered(&sfx.awaiting, &sfx.brings, |tag| def.has_tag(tag)) else { continue };
        let clips = clips.to_vec();
        sfx.awaiting.remove(at);
        let (pick, jitter) = (sfx.random(), sfx.random());
        if let Some(clip) = clips.get((pick * clips.len() as f32) as usize) {
            play_clip(&mut commands, clip, master, jitter);
        }
    }
}

/// What `footsteps` keeps between frames.
#[derive(Default)]
struct Strides {
    last: Option<Vec3>,
    travelled: f32,
    was_grounded: bool,
    /// Highest point since last on the ground.
    peak: f32,
    was_in_water: bool,
}

/// Footsteps every stride, from whatever is underfoot, in the manner of Minecraft and of
/// footstep mods such as Presence Footsteps: the ground's own step; the rustle of plants,
/// moss or petals the feet pass through, with the ground under them softer; a splash in
/// shallow water; strokes while swimming; and a thud when landing from a fall.
fn footsteps(player: Res<LocalPlayer>, world: Res<ClientWorld>, mut cues: MessageWriter<SoundCue>, mut state: Local<Strides>) {
    let state = &mut *state;
    // Falling or jumping into water splashes.
    if player.joined && player.in_water && !state.was_in_water && player.velocity.y < -3.0 {
        cues.write(SoundCue::new(Sound::Water, Cue::Land));
    }
    state.was_in_water = player.in_water;
    let position = player.position;
    let previous = state.last.replace(position).unwrap_or(position);
    if !player.joined || player.flying {
        state.travelled = 0.0;
        state.peak = position.y;
        return;
    }
    let feet = position.floor().as_ivec3();
    let ground = ground_under(&world, position);
    // A plant, carpet or flower the feet stand in.
    let cover = world.block(feet).filter(|&b| world.registry.info(b).render == RenderKind::Plant);
    let wading = player.in_water;
    if player.on_ground {
        if !state.was_grounded && state.peak - position.y > 1.2 {
            if let Some(block) = ground {
                cues.write(SoundCue::for_block(&world, block, Cue::Land));
            }
            if wading {
                cues.write(SoundCue::new(Sound::Water, Cue::Step));
            }
        }
        state.travelled += (position - previous).with_y(0.0).length();
        let stride = if player.sprinting { 2.1 } else { 1.7 };
        if state.travelled >= stride {
            state.travelled = 0.0;
            for cue in step_cues(&world.registry, ground, cover, wading) {
                cues.write(cue);
            }
        }
        state.peak = position.y;
    } else {
        state.peak = state.peak.max(position.y);
        if player.in_water {
            state.travelled += (position - previous).length();
            if state.travelled >= 2.4 {
                state.travelled = 0.0;
                cues.write(SoundCue::new(Sound::Water, Cue::Step));
            }
        }
    }
    state.was_grounded = player.on_ground;
}

/// The block a body standing at `feet` stands on: the one under its middle, or when that
/// is open (standing on an edge), the one under a corner of its footprint.
fn ground_under(world: &ClientWorld, feet: Vec3) -> Option<BlockId> {
    let half = dims::WIDTH / 2.0 - 0.01;
    [Vec2::ZERO, Vec2::new(-half, -half), Vec2::new(half, -half), Vec2::new(-half, half), Vec2::new(half, half)]
        .into_iter()
        .filter_map(|corner| world.block((feet + Vec3::new(corner.x, -0.1, corner.y)).floor().as_ivec3()))
        .find(|&b| world.registry.info(b).solid)
}

/// The sounds of one step: the ground's, unless something covers it (a plant, moss
/// carpet, petals), whose rustle then leads with the ground under it softer; and a splash
/// when wading.
fn step_cues(registry: &BlockRegistry, ground: Option<BlockId>, cover: Option<BlockId>, wading: bool) -> Vec<SoundCue> {
    let mut cues = Vec::new();
    if let Some(cover) = cover {
        cues.push(SoundCue::of(registry, cover, Cue::Step));
    }
    if let Some(ground) = ground {
        let softer = if cover.is_some() || wading { 0.5 } else { 1.0 };
        cues.push(SoundCue::of(registry, ground, Cue::Step).scaled(softer));
    }
    if wading {
        cues.push(SoundCue::new(Sound::Water, Cue::Step).scaled(0.7));
    }
    cues
}

/// SplitMix64 finaliser.
pub(crate) fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// White noise and uniform numbers from a seed.
pub(crate) struct Noise(pub(crate) u64);

impl Noise {
    /// -1..1.
    pub(crate) fn next(&mut self) -> f32 {
        self.0 = mix(self.0);
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }

    /// 0..1.
    pub(crate) fn unit(&mut self) -> f32 {
        (self.next() + 1.0) * 0.5
    }
}

/// How a material sounds, as parameters of one small synthesizer.
struct Recipe {
    /// Seconds for a step; other cues scale it.
    length: f32,
    /// Centre and sharpness of the band-passed noise.
    center: f32,
    q: f32,
    /// Grains per second; 0 means continuous noise.
    grains: f32,
    grain_length: f32,
    /// Seconds for the attack (soft materials swell in).
    attack: f32,
    /// Pitched resonances (frequency, decay per second): wood bodies, glass partials.
    tones: &'static [(f32, f32)],
    tone_gain: f32,
}

fn recipe(sound: Sound) -> Recipe {
    let base = Recipe { length: 0.12, center: 1500.0, q: 1.0, grains: 0.0, grain_length: 0.004, attack: 0.002, tones: &[], tone_gain: 0.0 };
    match sound {
        Sound::Stone => Recipe { length: 0.09, center: 1500.0, q: 1.3, tones: &[(150.0, 30.0)], tone_gain: 0.35, ..base },
        Sound::Wood => Recipe { length: 0.15, center: 900.0, q: 2.2, tones: &[(240.0, 16.0), (590.0, 24.0)], tone_gain: 0.6, ..base },
        Sound::Soil => Recipe {
            length: 0.12,
            center: 450.0,
            q: 0.8,
            grains: 200.0,
            grain_length: 0.007,
            attack: 0.01,
            tones: &[(90.0, 30.0)],
            tone_gain: 0.25,
        },
        Sound::Grass => Recipe { length: 0.14, center: 1800.0, q: 0.9, grains: 350.0, grain_length: 0.005, attack: 0.015, ..base },
        Sound::Sand => Recipe { length: 0.2, center: 1300.0, q: 0.5, grains: 600.0, grain_length: 0.004, attack: 0.035, ..base },
        Sound::Gravel => Recipe { length: 0.14, center: 1100.0, q: 1.4, grains: 260.0, grain_length: 0.008, ..base },
        Sound::Snow => Recipe { length: 0.17, center: 950.0, q: 0.9, grains: 420.0, grain_length: 0.007, attack: 0.012, ..base },
        Sound::Glass => Recipe {
            length: 0.22,
            center: 4200.0,
            q: 2.0,
            tones: &[(2110.0, 22.0), (3370.0, 30.0), (5180.0, 42.0)],
            tone_gain: 0.5,
            ..base
        },
        Sound::Leaves => Recipe { length: 0.2, center: 4200.0, q: 0.7, grains: 900.0, grain_length: 0.002, attack: 0.03, ..base },
        Sound::Water => Recipe { length: 0.32, center: 800.0, q: 0.7, attack: 0.01, tones: &[], ..base },
        Sound::Mud => Recipe {
            length: 0.16,
            center: 420.0,
            q: 1.6,
            grains: 120.0,
            grain_length: 0.012,
            attack: 0.012,
            tones: &[(110.0, 22.0)],
            tone_gain: 0.3,
        },
        Sound::Moss => Recipe { length: 0.15, center: 950.0, q: 0.7, grains: 280.0, grain_length: 0.006, attack: 0.025, ..base },
    }
}

/// Synthesizes one variant of a cue as mono samples in -1..1.
fn render(sound: Sound, cue: Cue, seed: u64) -> Vec<f32> {
    if cue == Cue::Pickup {
        return pop(seed);
    }
    if sound == Sound::Water {
        return water(cue, seed);
    }
    let recipe = recipe(sound);
    let (length_scale, pitch, grain_scale) = match cue {
        Cue::Step => (1.0, 1.0, 1.0),
        Cue::Land => (1.3, 0.8, 1.3),
        Cue::Place => (1.1, 0.9, 1.2),
        Cue::Break => (1.8, 0.82, 1.6),
        Cue::Hit => (0.6, 1.1, 0.7),
        Cue::Pickup => unreachable!("rendered by `pop`"),
    };
    let mut noise = Noise(seed);
    let length = recipe.length * length_scale * (0.9 + 0.2 * noise.unit());
    let count = (length * RATE as f32) as usize;

    // Grain mask: short Hann windows at random times with random strength.
    let mut mask = vec![if recipe.grains > 0.0 { 0.0 } else { 1.0 }; count];
    if recipe.grains > 0.0 {
        let grain = (recipe.grain_length * RATE as f32) as usize;
        let grains = (recipe.grains * grain_scale * length) as usize;
        for _ in 0..grains {
            let start = (noise.unit() * count as f32) as usize;
            let strength = 0.3 + 0.7 * noise.unit();
            for j in 0..grain.min(count.saturating_sub(start)) {
                mask[start + j] += strength * (0.5 - 0.5 * (TAU * j as f32 / grain as f32).cos());
            }
        }
    }

    // Chamberlin state-variable band-pass.
    let center = recipe.center * pitch;
    let f = 2.0 * (std::f32::consts::PI * center / RATE as f32).sin();
    let damping = 1.0 / recipe.q;
    let (mut low, mut band) = (0.0f32, 0.0f32);

    let decay = 4.5 / length;
    let mut samples = Vec::with_capacity(count);
    for (i, &m) in mask.iter().enumerate() {
        let t = i as f32 / RATE as f32;
        let envelope = (t / recipe.attack).min(1.0) * (-t * decay).exp();
        let input = noise.next() * m;
        low += f * band;
        let high = input - low - damping * band;
        band += f * high;
        let mut sample = band * 1.5;
        for &(frequency, tone_decay) in recipe.tones {
            sample += recipe.tone_gain * (TAU * frequency * pitch * t).sin() * (-t * tone_decay).exp();
        }
        // Placing gets a soft low thump under the material.
        if cue == Cue::Place || cue == Cue::Land {
            sample += 0.5 * (TAU * 70.0 * t).sin() * (-t * 28.0).exp();
        }
        samples.push(sample * envelope);
    }

    // Warm the top end with a gentle low-pass, match loudness (RMS, so continuous hiss is
    // not louder than grainy crunches), keep peaks in range, and fade the last
    // milliseconds so clips never click.
    let mut warm = 0.0f32;
    for sample in &mut samples {
        warm += 0.35 * (*sample - warm);
        *sample = warm;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt().max(1e-6);
    let peak = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    let scale = (0.16 / rms).min(0.9 / peak);
    let fade = (0.005 * RATE as f32) as usize;
    let len = samples.len();
    for (i, sample) in samples.iter_mut().enumerate() {
        let tail = ((len - i) as f32 / fade as f32).min(1.0);
        *sample *= scale * tail;
    }
    samples
}

/// Picking an item up: a short, soft pop whose pitch springs upward, like a bubble
/// bursting, a little different each time.
fn pop(seed: u64) -> Vec<f32> {
    let mut noise = Noise(seed);
    let base = dsp::between(&mut noise, 520.0, 760.0);
    let length = 0.09;
    let count = (length * RATE as f32) as usize;
    let mut phase = 0.0f32;
    let mut samples: Vec<f32> = (0..count)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let frequency = base * (1.0 + 1.3 * (1.0 - (-t * 55.0).exp()));
            phase += TAU * frequency / RATE as f32;
            let envelope = (t / 0.002).min(1.0) * (-t * 48.0).exp();
            (phase.sin() + 0.25 * (2.0 * phase).sin()) * envelope
        })
        .collect();
    dsp::normalise(&mut samples, 0.16, 0.9);
    samples
}

/// Water: a stroke while swimming (a step in water) is a soft swash of moving water with
/// a few fine bubbles in its wake; landing in water is a splash; anything else a small
/// swash.
fn water(cue: Cue, seed: u64) -> Vec<f32> {
    let mut noise = Noise(seed);
    let mut samples = match cue {
        Cue::Land => splash(&mut noise),
        Cue::Step => swash(&mut noise, 1.0),
        _ => swash(&mut noise, 0.6),
    };
    dsp::normalise(&mut samples, 0.16, 0.9);
    samples
}

/// Water pushed aside: a band of soft noise that swells and ebbs, sloshing a little, its
/// tone rising with the push, with a hint of fizz and a few small bubbles left behind.
fn swash(noise: &mut Noise, size: f32) -> Vec<f32> {
    let length = dsp::between(noise, 0.45, 0.6) * (0.6 + 0.4 * size);
    let count = dsp::seconds(length);
    let slosh = (dsp::between(noise, 7.0, 12.0), TAU * noise.unit());
    let mut pink = Pink::default();
    let mut body = Svf::new(500.0, 0.8);
    let mut dark = OnePole::new(2000.0);
    let mut fizz = OnePole::new(3000.0);
    let mut samples: Vec<f32> = (0..count)
        .map(|i| {
            let x = i as f32 / count as f32;
            let t = i as f32 / RATE as f32;
            let swell = if x < 0.3 { (x / 0.3 * PI / 2.0).sin().powi(2) } else { ((1.0 - x) / 0.7).powf(1.5) };
            body.tune(420.0 + 520.0 * swell);
            let wobble = 1.0 + 0.3 * (TAU * slosh.0 * t + slosh.1).sin();
            (dark.low(body.band(pink.next(noise.next()))) * wobble + 0.02 * fizz.high(noise.next())) * swell
        })
        .collect();
    for _ in 0..6 + (noise.unit() * 8.0) as usize {
        let at = dsp::between(noise, 0.1, 0.9) * length;
        let frequency = 600.0 * 3.5f32.powf(noise.unit());
        dsp::bubble(&mut samples, at, frequency, dsp::between(noise, 0.05, 0.15), dsp::between(noise, 0.05, 0.3));
    }
    samples
}

/// Falling into water: the slap of the surface, the low whump of water thrown aside, the
/// spray pattering back down and the air dragged under bubbling up.
fn splash(noise: &mut Noise) -> Vec<f32> {
    let mut samples = vec![0.0f32; dsp::seconds(1.1)];
    let (mut slap, mut soft) = (OnePole::new(500.0), OnePole::new(4000.0));
    for (i, sample) in samples.iter_mut().enumerate().take(dsp::seconds(0.3)) {
        let t = i as f32 / RATE as f32;
        *sample += 0.8 * soft.low(slap.high(noise.next())) * (t / 0.002).min(1.0) * (-t / 0.035).exp();
    }
    let mut whump = [Svf::new(260.0, 0.7), Svf::new(260.0, 0.7)];
    for (i, sample) in samples.iter_mut().enumerate().take(dsp::seconds(0.6)) {
        let t = i as f32 / RATE as f32;
        let x = whump.iter_mut().fold(noise.next(), |x, f| f.low(x));
        *sample += 3.0 * x * (t / 0.006).min(1.0) * (-t / 0.09).exp();
    }
    let mut t = 0.03f32;
    while t < 0.8 {
        let strength = 0.35 * (-(t - 0.03) / 0.22).exp() * dsp::between(noise, 0.2, 1.0);
        let (decay, center) = (dsp::between(noise, 0.0006, 0.002), dsp::between(noise, 1500.0, 4500.0));
        dsp::tap(&mut samples, noise, t, decay, center, strength);
        t += dsp::gap(noise, 500.0);
    }
    for _ in 0..14 {
        let at = dsp::between(noise, 0.04, 0.6);
        let frequency = 450.0 * 4.5f32.powf(noise.unit().powf(0.8));
        let gain = dsp::between(noise, 0.06, 0.2) * (1.0 - at);
        dsp::bubble(&mut samples, at, frequency, gain, dsp::between(noise, 0.1, 0.4));
    }
    samples
}

/// Encodes mono samples as a 16-bit PCM WAV file.
pub(crate) fn wav(samples: &[f32]) -> Vec<u8> {
    wav_at(RATE, samples)
}

/// Encodes mono samples at `rate` as a 16-bit PCM WAV file.
pub(crate) fn wav_at(rate: u32, samples: &[f32]) -> Vec<u8> {
    encode(1, rate, samples.len(), samples.iter().copied())
}

/// Encodes two channels of equal length as a 16-bit PCM WAV file.
pub(crate) fn wav_stereo(left: &[f32], right: &[f32]) -> Vec<u8> {
    wav_stereo_at(RATE, left, right)
}

/// Encodes two channels of equal length at `rate` as a 16-bit PCM WAV file.
pub(crate) fn wav_stereo_at(rate: u32, left: &[f32], right: &[f32]) -> Vec<u8> {
    debug_assert_eq!(left.len(), right.len());
    encode(2, rate, left.len(), left.iter().zip(right).flat_map(|(&l, &r)| [l, r]))
}

/// A WAV file of `frames` frames of `channels` interleaved samples at `rate`.
fn encode(channels: u16, rate: u32, frames: usize, samples: impl Iterator<Item = f32>) -> Vec<u8> {
    let block = channels as u32 * 2;
    let data_len = frames as u32 * block;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_renders_clean_audio() {
        for sound in Sound::ALL {
            for cue in Cue::ALL {
                let samples = render(sound, cue, 7);
                assert!(samples.len() > RATE as usize / 50, "{sound:?} {cue:?} too short");
                assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
                assert!(samples.last().unwrap().abs() < 0.05, "{sound:?} {cue:?} ends abruptly");
            }
        }
    }

    /// A muted run (a capture, VOXMAR_MUTE) starts no sound at all, and an unmuted one
    /// does: block sounds and thunder here, the rest through the same `Audible` set.
    #[test]
    fn muted_runs_start_no_sound() {
        for (global, started) in [(Volume::SILENT, 0), (Volume::Linear(1.0), 2)] {
            let mut app = App::new();
            let mut clips = FxHashMap::default();
            clips.insert((Sound::Stone, Cue::Step), vec![Clip { source: Handle::default(), gain: 1.0, speed: 1.0 }]);
            let rolls = [vec![Handle::default()], vec![Handle::default()], vec![Handle::default()]];
            app.insert_resource(GlobalVolume::new(global))
                .insert_resource(Settings::default())
                .insert_resource(Sfx { clips, thunder: rolls, actions: FxHashMap::default(), brings: FxHashMap::default(), awaiting: Vec::new(), counter: 0 })
                .add_message::<SoundCue>()
                .add_message::<Thunder>()
                .configure_sets(Update, Audible.run_if(sound_on))
                .add_systems(Update, (play_cues, thunder).in_set(Audible));
            app.world_mut().write_message(SoundCue::new(Sound::Stone, Cue::Step));
            app.world_mut().write_message(Thunder { distance: 100.0 });
            app.update();
            assert_eq!(app.world_mut().query::<&AudioPlayer>().iter(app.world()).count(), started, "at {global:?}");
        }
        // No audio at all (no global volume) is muted too.
        assert!(!App::new().world_mut().run_system_cached(sound_on).unwrap());
    }

    #[test]
    fn every_surface_has_a_step() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let id = |name: &str| blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("{name}"));
        let heard = |cues: Vec<SoundCue>| cues.iter().map(|c| (c.sound, c.gain)).collect::<Vec<_>>();
        // Bare ground: its own step.
        assert_eq!(heard(step_cues(blocks, Some(id("stone")), None, false)), [(Sound::Stone, 1.0)]);
        // Through grass and flowers, or over moss carpet: their rustle, the ground softer.
        assert_eq!(heard(step_cues(blocks, Some(id("grass_block")), Some(id("poppy")), false)), [(Sound::Grass, 1.0), (Sound::Grass, 0.5)]);
        assert_eq!(heard(step_cues(blocks, Some(id("stone")), Some(id("moss_carpet")), false))[0], (Sound::Moss, 1.0));
        // Wading: a splash over the ground.
        assert_eq!(heard(step_cues(blocks, Some(id("sand")), None, true)), [(Sound::Sand, 0.5), (Sound::Water, 0.7)]);
    }

    /// A pot loaded stirs, a rack loaded thumps like its wood, either emptied pops; a load
    /// coming done, or a block that is no station, is silent.
    #[test]
    fn stations_stir_and_pop() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let id = |name: &str| blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("{name}"));
        let cue = |old: &str, new: &str| station_cue(&content.stations, blocks, id(old), id(new)).map(|c| (c.sound, c.cue));
        assert_eq!(cue("cooking_pot", "cooking_pot_simmering"), Some((Sound::Water, Cue::Step)));
        assert_eq!(cue("drying_rack", "drying_rack_hung"), Some((blocks.info(id("drying_rack_hung")).sound, Cue::Place)));
        assert_eq!(cue("cooking_pot_ready", "cooking_pot"), Some((Sound::Stone, Cue::Pickup)));
        assert_eq!(cue("drying_rack_hung", "drying_rack"), Some((Sound::Stone, Cue::Pickup)));
        assert_eq!(cue("cooking_pot_simmering", "cooking_pot_ready"), None);
        assert_eq!(cue("stone", "cooking_pot"), None);
        assert_eq!(cue("drying_rack", "cooking_pot_simmering"), None);
    }

    #[test]
    fn loudness_compares_the_loudest_stretch() {
        let tone = |seconds: f32| (0..(seconds * RATE as f32) as usize).map(|i| (i as f32 * 0.1).sin() * 0.5).collect::<Vec<f32>>();
        let short = loudness(&tone(0.1), 1);
        // Silence after a sound, or a longer sound, does not change how loud it is.
        let mut padded = tone(0.1);
        padded.extend(std::iter::repeat_n(0.0, RATE as usize));
        assert!((loudness(&padded, 1) - short).abs() < short * 0.05);
        assert!((loudness(&tone(1.0), 1) - short).abs() < short * 0.05);
        // A sine of amplitude 0.5 has an RMS of about 0.35.
        assert!((short - 0.354).abs() < 0.02, "{short}");
        assert_eq!(loudness(&[], 1), 0.0);
    }

    /// The base package's recordings: listed, there, and in the game's format.
    #[test]
    fn recordings_load() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        let thunder = &recorded.thunder;
        for list in [&thunder.near, &thunder.middle, &thunder.far, &recorded.swim, &recorded.splash] {
            assert!(!list.is_empty());
            for path in list {
                let [left, _] = crate::ambience::tests::decode(path);
                assert!(left.len() > RATE as usize / 2 && left.iter().all(|s| s.is_finite() && s.abs() <= 1.0), "{path}");
            }
        }
        // The jingles: each there, as loud as one another at their loudest (within 3 dB, so
        // none jumps out), fading out rather than stopping, and over within five seconds.
        for list in [&recorded.quest_done, &recorded.quest_claim, &recorded.level_up, &recorded.level_milestone] {
            assert!(!list.is_empty());
            for path in list {
                let [left, right] = crate::ambience::tests::decode(path);
                let both: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
                let level = loudness(&both, 2);
                assert!((0.21..0.42).contains(&level), "{path}: loudest {level}");
                assert!(both.iter().all(|s| s.is_finite() && s.abs() <= 0.95), "{path} clips");
                assert!(left.len() < 5 * RATE as usize, "{path} runs on");
                let tail = &both[both.len() - 400..];
                assert!(tail.iter().all(|s| s.abs() < 0.01), "{path} ends with a click");
            }
        }
    }

    /// A later package's jingle replaces the earlier one; one it leaves out stays.
    #[test]
    fn a_later_jingle_replaces_the_earlier() {
        let file = |package: &str, source: &str| ContentFile { package: package.into(), name: RECORDINGS.into(), source: source.into() };
        let base = file("base", r#"(quest_done: ["a.ogg"], level_up: ["b.ogg"], level_milestone: ["c.ogg"])"#);
        let recorded = Recordings::load(&[base, file("mod", r#"(quest_done: ["x.ogg", "y.ogg"])"#)]).unwrap();
        assert_eq!(recorded.quest_done, ["content/mod/x.ogg", "content/mod/y.ogg"]);
        assert_eq!(recorded.level_up, ["content/base/b.ogg"]);
        assert_eq!(recorded.level_milestone, ["content/base/c.ogg"]);
    }

    /// A later package replaces an action's sound and the pickup pop, and adds actions; a
    /// sound for no action, with no clips, or at no volume fails the launch check.
    #[test]
    fn action_sounds_are_checked_and_a_later_package_replaces_them() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let file = |package: &str, source: &str| ContentFile { package: package.into(), name: RECORDINGS.into(), source: source.into() };
        let base = file("base", r#"(pickup: ["p.ogg"], actions: {"core:eat": (sounds: ["a.ogg", "b.ogg"]), "core:craft": (sounds: ["c.ogg"], volume: 0.5)})"#);
        let recorded = Recordings::load(&[base.clone(), file("mod", r#"(pickup: ["q.ogg"], actions: {"core:eat": (sounds: ["x.ogg"], volume: 1.5)})"#)]).unwrap();
        assert_eq!(recorded.pickup, ["content/mod/q.ogg"]);
        assert_eq!(recorded.actions["core:eat"], ActionSound { sounds: vec!["content/mod/x.ogg".into()], volume: 1.5, ..Default::default() });
        assert_eq!(recorded.actions["core:craft"], ActionSound { sounds: vec!["content/base/c.ogg".into()], volume: 0.5, ..Default::default() });
        assert_eq!(recorded.check(&content.actions), Ok(()));
        for wrong in [r#"(actions: {"core:juggle": (sounds: ["a.ogg"])})"#, r#"(actions: {"core:eat": (sounds: [])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], volume: 0.0)})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], silent: true)})"#, r#"(actions: {"core:eat": (silent: true, heard: Some(Block))})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(sounds: [])])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(tag: Some(""), sounds: ["b.ogg"])])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(sounds: ["b.ogg"], volume: 3.0)])})"#] {
            assert!(Recordings::load(&[file("mod", wrong)]).unwrap().check(&content.actions).is_err(), "{wrong}");
        }
        // Every action must say how it sounds: the base package's do; a package that adds
        // an action and says nothing of it fails, until it does.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base/sounds.ron");
        let base = file("base", &std::fs::read_to_string(path).unwrap());
        assert_eq!(Recordings::load(&[base.clone()]).unwrap().covers(&content.actions), Ok(()));
        let mut more = voxmar_engine::action::ActionRegistry::default();
        for (namespace, source) in [("core", "core/actions.ron"), ("base", "base/actions.ron")] {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
            more.extend(namespace, &std::fs::read_to_string(dir.join(source)).unwrap()).unwrap();
        }
        more.extend("zmod", r#"(actions: [(name: "juggle", duration: Instant, effects: [])])"#).unwrap();
        let mute = Recordings::load(&[base.clone()]).unwrap().covers(&more);
        assert!(mute.as_ref().is_err_and(|e| e.contains("zmod:juggle")), "{mute:?}");
        let quiet = Recordings::load(&[base, file("zmod", r#"(actions: {"zmod:juggle": (silent: true)})"#)]).unwrap();
        assert_eq!((quiet.check(&more), quiet.covers(&more)), (Ok(()), Ok(())));
    }

    /// The everyday actions of the first days are heard, from clips we ship, each made at
    /// [`ACTION_LOUDNESS`] (a clip far quieter or louder would sound wrong beside the
    /// rest), and so is an item picked up.
    #[test]
    fn everyday_actions_are_heard_at_one_loudness() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        recorded.check(&content.actions).unwrap();
        recorded.covers(&content.actions).unwrap();
        for name in [
            "core:eat",
            "core:craft",
            "core:drop",
            "base:light",
            "base:open_store",
            "base:open",
            "base:cook",
            "base:sleep",
            "base:tend",
            "base:empty_pouch",
            "base:use_machine",
            "base:pick_lock",
            "base:pray",
            "base:fertilise",
        ] {
            assert!(recorded.actions.contains_key(name), "no sound for {name}");
        }
        assert!(!recorded.pickup.is_empty(), "no pickup pop");
        // What fishing and cooking bring is heard as it lands: a catch, food done or burnt.
        let brings = |name: &str| recorded.actions[name].brings.iter().map(|b| b.tag.clone()).collect::<Vec<_>>();
        assert_eq!(brings("base:fish"), [None]);
        assert_eq!(brings("base:cook"), [Some("burnt".to_owned()), None]);
        // Every tag a result is told by is one some item has.
        for brought in recorded.actions.values().flat_map(|sound| &sound.brings) {
            if let Some(tag) = &brought.tag {
                assert!((0..content.blocks.len()).any(|id| content.blocks.def(BlockId(id as u16)).has_tag(tag)), "no item is tagged {tag}");
            }
        }
        let brought = recorded.actions.values().flat_map(|sound| sound.brings.iter().flat_map(|b| b.sounds.iter().map(move |path| (path, b.volume))));
        let clips = recorded.actions.values().flat_map(|sound| sound.sounds.iter().map(|path| (path, sound.volume))).chain(brought).chain(recorded.pickup.iter().map(|path| (path, 1.0)));
        for (path, volume) in clips {
            let [left, right] = crate::ambience::tests::decode(path);
            let stereo: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
            let heard = loudness(&stereo, 2) * volume / ACTION_LOUDNESS;
            assert!((0.5..=1.25).contains(&heard), "{path} is heard at {heard:.2} of the loudness of the rest");
            assert!(left.len() as f32 / (RATE as f32) < 1.5, "{path} is longer than an action");
        }
    }

    /// Everything the player does and touches is heard within the everyday band (blocks'
    /// cues as synthesized, the water's and our own recordings as levelled to them), and
    /// thunder within the weather's, from overhead and from each reach's nearest.
    #[test]
    fn everyday_sounds_and_thunder_are_heard_within_their_bands() {
        use crate::mix::tests::{measure, within};
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        let mut heard = Vec::new();
        let mut target: FxHashMap<Cue, (f32, f32)> = FxHashMap::default();
        for sound in Sound::ALL {
            for cue in Cue::ALL {
                for v in 0..VARIANTS {
                    let samples = render(sound, cue, mix(sound as u64 * 131 + cue as u64 * 17 + v));
                    let level = loudness(&samples, 1);
                    let entry = target.entry(cue).or_default();
                    *entry = (entry.0 + level, entry.1 + 1.0);
                    heard.push((format!("{sound:?} {cue:?} {v}"), level * cue.gain()));
                }
            }
        }
        for (cue, paths) in [(Cue::Step, &recorded.swim), (Cue::Land, &recorded.splash)] {
            heard.extend(paths.iter().map(|path| (path.clone(), measure(path)[0] * cue.gain() * RECORDED_GAIN)));
        }
        let pickup = Cue::Pickup.gain() * level(&target, Cue::Pickup) / ACTION_LOUDNESS;
        heard.extend(recorded.pickup.iter().map(|path| (path.clone(), measure(path)[0] * pickup)));
        for sound in recorded.actions.values() {
            let gain = sound.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
            heard.extend(sound.sounds.iter().map(|path| (path.clone(), measure(path)[0] * gain)));
            for brought in &sound.brings {
                let gain = brought.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
                heard.extend(brought.sounds.iter().map(|path| (path.clone(), measure(path)[0] * gain)));
            }
        }
        let everyday = within(crate::mix::Category::Everyday, heard);
        let thunder = [(&recorded.thunder.near, 0.0), (&recorded.thunder.middle, NEAR_LIGHTNING), (&recorded.thunder.far, FAR_LIGHTNING)]
            .into_iter()
            .flat_map(|(paths, distance)| paths.iter().map(move |path| (path.clone(), measure(path)[0] * thunder_gain(distance))));
        let weather = within(crate::mix::Category::Weather, thunder);
        crate::mix::tests::all([everyday, weather]);
    }

    /// What lands in the pack answers the action awaiting it, by the first rule whose tag it
    /// has: a burnt meal its burn, anything else cooked its finish; nothing answers an
    /// action not awaited, and only what is carried more of counts as landed.
    #[test]
    fn results_answer_the_action_awaiting_them() {
        use voxmar_engine::action::ActionId;
        let clip = |gain: f32| Clip { source: Handle::default(), gain, speed: 1.0 };
        let (cook, fish) = (ActionId(1), ActionId(2));
        let mut brings = FxHashMap::default();
        brings.insert(cook, vec![(Some("burnt".to_owned()), vec![clip(1.0)]), (None, vec![clip(2.0)])]);
        brings.insert(fish, vec![(None, vec![clip(3.0)])]);
        let gain = |found: Option<(usize, &[Clip])>| found.map(|(at, clips)| (at, clips[0].gain));
        assert_eq!(gain(answered(&[(cook, 1.0)], &brings, |tag| tag == "burnt")), Some((0, 1.0)));
        assert_eq!(gain(answered(&[(cook, 1.0)], &brings, |_| false)), Some((0, 2.0)));
        assert_eq!(gain(answered(&[(fish, 1.0), (cook, 1.0)], &brings, |_| false)), Some((0, 3.0)));
        assert_eq!(gain(answered(&[(ActionId(9), 1.0)], &brings, |_| true)), None);
        let counts = |pairs: &[(u16, u32)]| pairs.iter().map(|&(i, n)| (BlockId(i), n)).collect::<FxHashMap<_, _>>();
        assert_eq!(gained(&counts(&[(5, 2), (6, 1)]), &counts(&[(5, 3), (6, 1), (7, 1)])), [BlockId(5), BlockId(7)]);
        assert!(gained(&counts(&[(5, 2)]), &counts(&[(5, 1)])).is_empty());
        let mut inventory = voxmar_engine::inventory::Inventory::default();
        inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack { item: BlockId(5), count: 2 });
        inventory.cursor = Some(voxmar_engine::inventory::ItemStack { item: BlockId(5), count: 1 });
        inventory.pouched.push((BlockId(8), 40));
        assert_eq!(carried(&inventory), counts(&[(5, 3), (8, 40)]));
    }

    /// Each action proposed is heard once, and only with sound on; proposals made while it
    /// is off are let go, not saved up.
    #[test]
    fn proposed_actions_are_heard_once_and_never_muted() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let eat = content.actions.expect("core:eat");
        let craft = content.actions.expect("core:craft");
        for (global, started) in [(Volume::SILENT, 0), (Volume::Linear(1.0), 1)] {
            let (connection, _server_side) = voxmar_protocol::local_pair();
            let net = crate::net::Net::new(connection, None, "tester".into(), content.fingerprint(), None);
            let mut actions = FxHashMap::default();
            actions.insert(eat, vec![Clip { source: Handle::default(), gain: 1.0, speed: 1.0 }]);
            let mut app = App::new();
            app.insert_resource(GlobalVolume::new(global))
                .insert_resource(Settings::default())
                .insert_resource(Sfx { clips: FxHashMap::default(), thunder: Default::default(), actions, brings: FxHashMap::default(), awaiting: Vec::new(), counter: 0 })
                .insert_resource(net)
                .add_systems(Update, play_actions);
            // Crafting has no sound here: nothing plays for it.
            app.world().resource::<crate::net::Net>().propose(eat, voxmar_engine::action::ActionTarget::None);
            app.world().resource::<crate::net::Net>().propose(craft, voxmar_engine::action::ActionTarget::Slot(1));
            app.update();
            app.update();
            let playing = app.world_mut().query::<&AudioPlayer>().iter(app.world()).count();
            assert_eq!(playing, started, "volume {global:?}");
            assert!(app.world().resource::<crate::net::Net>().take_proposed().is_empty());
        }
    }

    /// `cargo test -p voxmar_client minecraft_sounds_decode -- --ignored` checks that every
    /// cue borrowed from the Minecraft install on this machine decodes.
    #[test]
    #[ignore]
    fn minecraft_sounds_decode() {
        use bevy::audio::Decodable;
        let store = SoundStore::discover().expect("a Minecraft install with an asset index");
        let mut decoded = 0;
        for sound in Sound::ALL {
            for cue in Cue::ALL {
                let Some((event, ..)) = minecraft_event(sound, cue) else { continue };
                let clips = store.event(&event);
                assert!(!clips.is_empty() || matches!(sound, Sound::Mud | Sound::Moss), "{event} has no clips");
                for clip in clips {
                    let source = AudioSource { bytes: Arc::from(store.read(&clip).expect("clip file")) };
                    assert!(source.decoder().take(1000).count() == 1000, "{} does not decode", clip.name);
                    decoded += 1;
                }
            }
        }
        println!("{decoded} clips decoded");
    }

    /// `cargo test --profile iter -p voxmar_client weather_sound_preview -- --ignored`
    /// writes the recorded thunder (at the volume of a strike 100, 500 and 1500 blocks
    /// away), swimming strokes and splashes to `target/sound_preview/`, as the game plays
    /// them; `ambience_preview` has the rain and water beds.
    #[test]
    #[ignore]
    fn weather_sound_preview() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sound_preview");
        std::fs::create_dir_all(&dir).unwrap();
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        let gap = |seconds: f32| vec![0.0f32; (seconds * RATE as f32) as usize];
        let mut thunder = Vec::new();
        for (list, distance) in [(&recorded.thunder.near, 100.0), (&recorded.thunder.middle, 500.0), (&recorded.thunder.far, 1500.0)] {
            for path in list {
                let [left, right] = crate::ambience::tests::decode(path);
                let gain = THUNDER_GAIN / (1.0 + distance / 400.0) * MASTER;
                thunder.extend(left.iter().zip(&right).map(|(l, r)| 0.5 * (l + r) * gain));
                thunder.extend(gap(1.0));
            }
        }
        std::fs::write(dir.join("thunder.wav"), wav(&thunder)).unwrap();
        let mut water = Vec::new();
        for (list, cue, every) in [(&recorded.swim, Cue::Step, 0.9), (&recorded.splash, Cue::Land, 3.6)] {
            for path in list {
                let [left, right] = crate::ambience::tests::decode(path);
                let gain = cue.gain() * RECORDED_GAIN * MASTER;
                water.extend(left.iter().zip(&right).map(|(l, r)| 0.5 * (l + r) * gain));
                water.extend(gap(every - left.len() as f32 / RATE as f32));
            }
        }
        std::fs::write(dir.join("water.wav"), wav(&water)).unwrap();
        println!("wrote {}", dir.display());
    }

    /// `cargo test -p voxmar_client sfx_preview -- --ignored` writes every cue, one after
    /// another, to `target/sfx_preview.wav` for listening.
    #[test]
    #[ignore]
    fn sfx_preview() {
        let mut all = Vec::new();
        for sound in Sound::ALL {
            for cue in Cue::ALL {
                for v in 0..2 {
                    all.extend(render(sound, cue, v).iter().map(|s| s * cue.gain()));
                    all.extend(std::iter::repeat_n(0.0, RATE as usize / 4));
                }
            }
            all.extend(std::iter::repeat_n(0.0, RATE as usize / 2));
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sfx_preview.wav");
        std::fs::write(path, wav(&all)).unwrap();
    }
}
