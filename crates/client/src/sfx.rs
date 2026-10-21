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
use rustc_hash::{FxHashMap, FxHashSet};
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
            .add_message::<Catches>()
            .add_message::<Done>()
            .init_resource::<MaterialsAsked>()
            .configure_sets(Update, Audible.run_if(sound_on))
            .add_systems(Startup, synthesize)
            .add_systems(Update, ((footsteps, (play_cues.in_set(Audible), ask_materials)).chain(), thunder.in_set(Audible), play_catches.in_set(Audible), action_sounds()));
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
    /// The block it is the sound of, if any: a block of a building material
    /// (`sounds.ron`'s `materials`) sounds as that material, not as its family.
    pub block: Option<BlockId>,
}

impl SoundCue {
    pub fn new(sound: Sound, cue: Cue) -> Self {
        Self { sound, cue, at: None, gain: 1.0, pitch: 1.0, block: None }
    }

    pub fn for_block(world: &ClientWorld, block: BlockId, cue: Cue) -> Self {
        Self::of(&world.registry, block, cue)
    }

    pub fn of(registry: &BlockRegistry, block: BlockId, cue: Cue) -> Self {
        Self { block: Some(block), ..Self::new(registry.info(block).sound, cue) }
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

/// An action the client sees done ([`SEEN_DONE`]): the server changed the block it works,
/// `with` (a full skep's honey taken). Heard by the way done `with`, else the action's own,
/// from the block, at `at`.
#[derive(Message, Clone, Copy, Debug)]
pub struct Done {
    pub at: IVec3,
    pub action: voxmar_engine::action::ActionId,
    pub with: BlockId,
}

/// Whether a block changing from `old` to `new` is bees taken (`bees.ron`'s takes): the
/// block a take is from becoming what it becomes, when that is a block (a full skep's honey
/// cut). A nest gone to air may as well be broken: a swarm taken is heard by the skep of
/// bees it brings instead.
pub fn bees_taken(bees: &voxmar_engine::bees::Bees, old: BlockId, new: BlockId) -> bool {
    !new.is_air() && bees.takes.iter().any(|take| take.block == old && take.becomes == new)
}

/// Something nearby caught fire (fire.ron): the server turned it into the burning block.
/// Heard as `sounds.ron`'s `fire` `catch`, from the block.
#[derive(Message, Clone, Copy, Debug)]
pub struct Catches {
    pub at: Vec3,
}

/// Whether a block changing from `old` to `new` is something catching fire: becoming
/// fire.ron's burning block, from anything else that was loaded. Said by the server's block
/// change, never timed here.
pub fn catches(fire: &voxmar_engine::fire::Fire, old: Option<BlockId>, new: BlockId) -> bool {
    fire.burning == Some(new) && old.is_some_and(|old| old != new)
}

/// One clip a cue may play, and how loud and fast.
#[derive(Clone)]
struct Clip {
    source: Handle<AudioSource>,
    gain: f32,
    speed: f32,
}

/// Result sounds by action: each with the tags its items must all carry.
type Brings = FxHashMap<voxmar_engine::action::ActionId, Vec<(Vec<String>, Vec<Clip>)>>;

#[derive(Resource)]
struct Sfx {
    clips: FxHashMap<(Sound, Cue), Vec<Clip>>,
    /// Each building material's clips by cue (`sounds.ron`'s `materials`), and which
    /// material each block is of, by block id.
    materials: Vec<FxHashMap<Cue, Vec<Clip>>>,
    material: Vec<Option<usize>>,
    /// Thunder from near, middling and far lightning, a few variants each.
    thunder: [Vec<Handle<AudioSource>>; 3],
    /// What actions sound like as the player does them.
    actions: FxHashMap<voxmar_engine::action::ActionId, Vec<Clip>>,
    /// Something catching fire.
    catch: Vec<Clip>,
    /// What their results sound like landing in the pack, by their tags.
    brings: Brings,
    /// Actions done whose results may yet land, and until when (seconds).
    awaiting: Vec<(voxmar_engine::action::ActionId, f32)>,
    /// The actions heard as the server does them ([`ActionSound::when_done`]), not as
    /// proposed.
    when_done: FxHashSet<voxmar_engine::action::ActionId>,
    /// How long each action that brings something waits for it, if not [`BROUGHT_WITHIN`].
    brought_within: FxHashMap<voxmar_engine::action::ActionId, f32>,
    /// Clicks of actions heard when done, with what they were done with, until when: a
    /// [`Done`] is heard only for the block the player clicked.
    clicked: Vec<(voxmar_engine::action::ActionId, IVec3, f32)>,
    /// The ways of doing them that sound their own, in order, by what each is done with.
    ways: FxHashMap<voxmar_engine::action::ActionId, Vec<(FxHashSet<BlockId>, Vec<Clip>)>>,
    counter: u64,
}

impl Sfx {
    fn random(&mut self) -> f32 {
        self.counter += 1;
        (mix(self.counter) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// The clips `cue` plays from: its block's material's, when it has some for the cue,
    /// else its family's.
    fn clips(&self, cue: &SoundCue) -> Option<&Vec<Clip>> {
        let material = cue.block.and_then(|block| *self.material.get(block.0 as usize)?);
        material.and_then(|m| self.materials[m].get(&cue.cue)).or_else(|| self.clips.get(&(cue.sound, cue.cue)))
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
    // Building materials sound about as loud as the family cues they stand in for.
    let materials = recorded
        .materials
        .iter()
        .map(|material| {
            Cue::ALL
                .into_iter()
                .filter_map(|cue| {
                    let (paths, speed) = material.clips(cue)?;
                    let gain = cue.gain() * level(&target, cue) / ACTION_LOUDNESS;
                    Some((cue, load(paths).into_iter().map(|source| Clip { source, gain, speed }).collect()))
                })
                .collect()
        })
        .collect();
    let blocks = &launch.content.blocks;
    let material = blocks.ids().map(|id| recorded.material_of(blocks, id)).collect();
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
    let ways = recorded
        .actions
        .iter()
        .filter_map(|(name, sound)| {
            let ways = sound
                .ways
                .iter()
                .filter_map(|way| {
                    let items = way.items(blocks).inspect_err(|e| error!("{RECORDINGS}: `{name}`'s way `{}`: {e}", way.name)).ok()?;
                    let gain = way.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
                    Some((items, load(&way.sounds).into_iter().map(|source| Clip { source, gain, speed: 1.0 }).collect()))
                })
                .collect();
            Some((launch.content.actions.id(name)?, ways))
        })
        .collect();
    // Catching fire as loud as placing a block, times its volume.
    let gain = recorded.fire.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
    let catch = load(&recorded.fire.catch).into_iter().map(|source| Clip { source, gain, speed: 1.0 }).collect();
    let brings = recorded
        .actions
        .iter()
        .filter(|(_, sound)| !sound.brings.is_empty())
        .filter_map(|(name, sound)| {
            let rules = sound.brings.iter().map(|brought| {
                let gain = brought.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
                (brought.tags.clone(), load(&brought.sounds).into_iter().map(|source| Clip { source, gain, speed: 1.0 }).collect())
            });
            Some((launch.content.actions.id(name)?, rules.collect()))
        })
        .collect();
    let when_done = recorded.actions.iter().filter(|(_, sound)| sound.when_done).filter_map(|(name, _)| launch.content.actions.id(name)).collect();
    let brought_within = recorded.actions.iter().filter_map(|(name, sound)| Some((launch.content.actions.id(name)?, sound.brought_within?))).collect();
    commands.insert_resource(Sfx { clips, materials, material, thunder, actions, brings, awaiting: Vec::new(), when_done, brought_within, clicked: Vec::new(), catch, ways, counter: 0 });
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
    /// What building materials sound like underfoot, set down and broken, each for the
    /// blocks carrying all its tags. A later package that names a material replaces it.
    pub(crate) materials: Vec<MaterialSound>,
    /// What fire sounds like: thatch or wattle catching. A later package that lists any
    /// replaces these.
    pub(crate) fire: FireSounds,
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
    /// The ways of doing it that sound their own, the first that fits heard instead of
    /// `sounds`: crafting from flint, lighting a hearthstone (see [`WaySound`]).
    pub(crate) ways: Vec<WaySound>,
    /// What it sounds like when what it brings lands in the pack (a fish landed, food off
    /// the fire done or burnt): the first whose `tags` the item all has.
    pub(crate) brings: Vec<Brought>,
    /// Heard as the server does it, from where it is done, never on the click: a refused one
    /// is silent. Only for an action the client sees done ([`SEEN_DONE`]).
    pub(crate) when_done: bool,
    /// Seconds after the click that what it brings may still land and be heard (if not
    /// said, [`BROUGHT_WITHIN`], for a line's five seconds in the water): an action the
    /// server answers at once waits less, so a later pickup is not taken for it.
    pub(crate) brought_within: Option<f32>,
}

/// The actions the client sees done, by the block they work changing (see [`Done`]): bees
/// taken (`bees.ron`'s takes, the block becoming what it becomes).
const SEEN_DONE: &[&str] = &["base:tend_bees"];

/// The sound of an action's result arriving in the pack.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Brought {
    /// The tags an item brought has, all of them (`burnt`; `fish` and `raw`), so that only
    /// what the action brings answers it, never anything else picked up meanwhile.
    pub(crate) tags: Vec<String>,
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

/// The sounds of fire in the thatch (fire.ron): its `catch`, a soft whoomph from a block
/// taking fire, made at the loudness of [`ACTION_LOUDNESS`] and played that much louder or
/// softer than a block set down. The smoulder before it and the burning after are the
/// ambience's (ambience.ron's `smoulder` and `blaze` groups).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FireSounds {
    pub(crate) catch: Vec<String>,
    pub(crate) volume: f32,
}

impl Default for FireSounds {
    fn default() -> Self {
        Self { catch: Vec::new(), volume: 1.0 }
    }
}

/// The sound of one building material (timber, thatch, cobblestone): the clips of a step on
/// it, of a piece set down and of one broken, made at the loudness of [`ACTION_LOUDNESS`].
/// Every block carrying all of `tags` sounds as it; of several that fit, the one naming
/// the most tags (the later, if as many). Mining taps and landings use the steps, the
/// landing lower; a cue it lists no clips for is its block's family's (`sound: Stone`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MaterialSound {
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
    pub(crate) step: Vec<String>,
    pub(crate) place: Vec<String>,
    #[serde(rename = "break")]
    pub(crate) breaks: Vec<String>,
}

/// How much lower a landing on a material sounds than a step on it.
const LANDING_PITCH: f32 = 0.8;

impl MaterialSound {
    /// The clips `cue` plays on this material, and at what speed; none for a cue it leaves
    /// to the block's family.
    pub(crate) fn clips(&self, cue: Cue) -> Option<(&[String], f32)> {
        let (paths, speed) = match cue {
            Cue::Step | Cue::Hit => (&self.step, 1.0),
            Cue::Land => (&self.step, LANDING_PITCH),
            Cue::Place => (&self.place, 1.0),
            Cue::Break => (&self.breaks, 1.0),
            Cue::Pickup => return None,
        };
        (!paths.is_empty()).then_some((paths.as_slice(), speed))
    }

    fn fits(&self, def: &voxmar_engine::block::BlockDef) -> bool {
        self.tags.iter().all(|tag| def.has_tag(tag))
    }
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

/// One way of doing an action that sounds its own: done with any of `with` (items or blocks
/// by name, or `tag:TAG`), it plays these clips instead of the action's. What an action is
/// done with is what it is made from (crafting: the grid's items) or the block it works and
/// what is in hand (lighting, loading a station, cooking).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WaySound {
    pub(crate) name: String,
    pub(crate) with: Vec<String>,
    pub(crate) sounds: Vec<String>,
    #[serde(default = "full")]
    pub(crate) volume: f32,
    /// Makes no sound of its own: heard only by what the action brings (shears, a pail).
    pub(crate) silent: bool,
}

impl WaySound {
    /// Every item and block the way is done with, or why one of `with` names none.
    pub(crate) fn items(&self, blocks: &BlockRegistry) -> Result<FxHashSet<BlockId>, String> {
        let mut items = FxHashSet::default();
        for name in &self.with {
            let found: Vec<BlockId> = match name.strip_prefix("tag:") {
                Some(tag) => blocks.tagged(tag).collect(),
                None => blocks.id(name).into_iter().collect(),
            };
            if found.is_empty() {
                return Err(format!("`{name}` is no item, block or tag any has"));
            }
            items.extend(found);
        }
        Ok(items)
    }
}

/// Whether an action done with `with` awaits what it brings: always, unless some of its
/// `ways` are heard only by what they bring (silent: no clips), and then only when done
/// one of those (shears, not food; a swarm, not honey).
pub(crate) fn awaits<T>(ways: &[(FxHashSet<BlockId>, Vec<T>)], with: &[BlockId]) -> bool {
    let by_brings = ways.iter().any(|(_, clips)| clips.is_empty());
    !by_brings || way_of(ways, with).is_some_and(|clips| clips.is_empty())
}

/// The first of `ways` done with any of `with`.
pub(crate) fn way_of<'a, T>(ways: &'a [(FxHashSet<BlockId>, T)], with: &[BlockId]) -> Option<&'a T> {
    ways.iter().find(|(items, _)| with.iter().any(|item| items.contains(item))).map(|(_, way)| way)
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
                // A bare name is the package's own item, as in its other files.
                let named = |with: Vec<String>| with.into_iter().map(|name| if name.contains(':') { name } else { format!("{}:{name}", file.package) }).collect();
                let ways = sound.ways.into_iter().map(|way| WaySound { with: named(way.with), sounds: path(way.sounds).collect(), ..way }).collect();
                all.actions.insert(name, ActionSound { sounds: path(sound.sounds).collect(), brings, ways, ..sound });
            }
            for (name, sound) in def.interface {
                all.interface.insert(name, InterfaceSound { sounds: path(sound.sounds).collect(), ..sound });
            }
            for material in def.materials {
                let material = MaterialSound {
                    step: path(material.step).collect(),
                    place: path(material.place).collect(),
                    breaks: path(material.breaks).collect(),
                    ..material
                };
                match all.materials.iter_mut().find(|m| m.name == material.name) {
                    Some(earlier) => *earlier = material,
                    None => all.materials.push(material),
                }
            }
            if !def.fire.catch.is_empty() {
                all.fire = FireSounds { catch: path(def.fire.catch).collect(), ..def.fire };
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
            if sound.when_done && (sound.sounds.is_empty() || !SEEN_DONE.contains(&name.as_str())) {
                return Err(format!("{RECORDINGS}: `{name}` is heard when done, which needs its own sounds and an action the client sees done ({})", SEEN_DONE.join(", ")));
            }
            // One the client sees done is heard then, or its sounds would play on the click too.
            if SEEN_DONE.contains(&name.as_str()) && !sound.sounds.is_empty() && !sound.when_done {
                return Err(format!("{RECORDINGS}: `{name}` has sounds but is not heard when done (`when_done: true`), as the client sees it done"));
            }
            if let Some(within) = sound.brought_within
                && (sound.brings.is_empty() || !(within > 0.0 && within <= 30.0))
            {
                return Err(format!("{RECORDINGS}: `{name}` waits {within} s for what it brings: it must bring something, and wait above 0 and up to 30 s"));
            }
            for brought in &sound.brings {
                if brought.sounds.is_empty() || !(brought.volume > 0.0 && brought.volume <= 2.0) {
                    return Err(format!("{RECORDINGS}: what `{name}` brings needs sounds, at a volume above 0 and up to 2"));
                }
                if brought.tags.is_empty() || brought.tags.iter().any(String::is_empty) {
                    return Err(format!("{RECORDINGS}: what `{name}` brings needs the tags its items have, none empty"));
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

    /// Whether a fire that can set blocks burning (fire.ron) is heard catching: a `catch`
    /// clip, at a volume from above nothing to twice the usual.
    pub(crate) fn check_fire(&self, fire: &voxmar_engine::fire::Fire) -> Result<(), String> {
        if fire.on() && fire.burning.is_some() && self.fire.catch.is_empty() {
            return Err(format!("{RECORDINGS}: fire.ron sets blocks burning, but `fire` has no `catch` sound"));
        }
        if !(self.fire.volume > 0.0 && self.fire.volume <= 2.0) {
            return Err(format!("{RECORDINGS}: `fire` has volume {}, not above 0 and up to 2", self.fire.volume));
        }
        Ok(())
    }

    /// Whether every way of an action is named once, refines an action that has clips, is
    /// done with items, blocks or tags the content has, and has clips at a volume from
    /// above nothing to twice the usual.
    pub(crate) fn check_ways(&self, blocks: &BlockRegistry) -> Result<(), String> {
        for (action, sound) in &self.actions {
            if !sound.ways.is_empty() && sound.sounds.is_empty() {
                return Err(format!("{RECORDINGS}: `{action}` has ways but no sounds of its own; ways stand in for its sounds"));
            }
            for (i, way) in sound.ways.iter().enumerate() {
                let name = &way.name;
                let at = format!("{RECORDINGS}: `{action}`'s way `{name}`");
                if name.is_empty() || sound.ways[..i].iter().any(|w| &w.name == name) {
                    return Err(format!("{at}: each way has a name of its own"));
                }
                if way.with.is_empty() {
                    return Err(format!("{at} is done with nothing, so never heard"));
                }
                way.items(blocks).map_err(|e| format!("{at}: {e}"))?;
                match (way.silent, way.sounds.is_empty()) {
                    (false, true) => return Err(format!("{at} has no sounds")),
                    (true, false) => return Err(format!("{at} is silent and has sounds; it says one")),
                    (true, true) if sound.brings.is_empty() => return Err(format!("{at} is silent, and `{action}` brings nothing to be heard by")),
                    _ => {}
                }
                if !(way.volume > 0.0 && way.volume <= 2.0) {
                    return Err(format!("{at} has volume {}, not above 0 and up to 2", way.volume));
                }
            }
        }
        Ok(())
    }

    /// Whether every clip named is there under `assets`: a mistyped path fails at launch,
    /// not silently the first time it would play.
    pub(crate) fn check_files(&self, assets: &std::path::Path) -> Result<(), String> {
        for (what, clips) in self.clip_lists() {
            if let Some(missing) = clips.iter().find(|path| !assets.join(path).is_file()) {
                return Err(format!("{RECORDINGS}: {what} sounds `{missing}`, which is not there"));
            }
        }
        Ok(())
    }

    /// Every list of clips in the file, with what it is the sound of. Each struct is taken
    /// apart field by field, never with `..`, so a list added to one cannot be left out.
    fn clip_lists(&self) -> Vec<(String, &[String])> {
        let Recordings { thunder, swim, splash, quest_done, quest_claim, level_up, level_milestone, pickup, actions, interface, materials, fire, mix: _ } = self;
        let ThunderRecordings { near, middle, far } = thunder;
        let FireSounds { catch, volume: _ } = fire;
        let mut lists: Vec<(String, &[String])> = vec![
            ("thunder".into(), near), ("thunder".into(), middle), ("thunder".into(), far),
            ("swim".into(), swim), ("splash".into(), splash), ("pickup".into(), pickup),
            ("quest_done".into(), quest_done), ("quest_claim".into(), quest_claim),
            ("level_up".into(), level_up), ("level_milestone".into(), level_milestone),
            ("fire".into(), catch),
        ];
        for (action, sound) in actions {
            let ActionSound { sounds, volume: _, silent: _, heard: _, ways, brings, when_done: _, brought_within: _ } = sound;
            lists.push((format!("`{action}`"), sounds));
            for WaySound { name, with: _, sounds, volume: _, silent: _ } in ways {
                lists.push((format!("`{action}`'s way `{name}`"), sounds));
            }
            for Brought { tags, sounds, volume: _ } in brings {
                lists.push((format!("`{action}`'s brings {tags:?}"), sounds));
            }
        }
        for (event, InterfaceSound { tap: _, sounds, jingle: _, silent: _, volume: _, pitch: _ }) in interface {
            lists.push((format!("interface `{event}`"), sounds));
        }
        for MaterialSound { name, tags: _, step, place, breaks } in materials {
            lists.extend([step, place, breaks].map(|clips| (format!("material `{name}`"), clips.as_slice())));
        }
        lists
    }

    /// The name of the way `action` done with `with` sounds as, if any.
    #[cfg(test)]
    pub(crate) fn way_named(&self, blocks: &BlockRegistry, action: &str, with: &[BlockId]) -> Option<&str> {
        let ways: Vec<_> = self.actions.get(action)?.ways.iter().map(|way| (way.items(blocks).unwrap(), way.name.as_str())).collect();
        way_of(&ways, with).copied()
    }

    /// Which material `block` sounds as, if any (see [`MaterialSound`]): the one its def
    /// names (`material`), else the closest fit by tags.
    pub(crate) fn material_of(&self, blocks: &BlockRegistry, block: BlockId) -> Option<usize> {
        let def = blocks.def(block);
        if let Some(named) = &def.material {
            return self.materials.iter().position(|m| &m.name == named);
        }
        self.materials.iter().enumerate().filter(|(_, m)| m.fits(def)).max_by_key(|(_, m)| m.tags.len()).map(|(i, _)| i)
    }

    /// Whether every material is named, says which blocks it is by, has clips for some cue,
    /// and is the sound of some block: one that fits none is a mistyped tag. And whether
    /// every block naming a material (`material`) names one there is.
    pub(crate) fn check_materials(&self, blocks: &BlockRegistry) -> Result<(), String> {
        for id in blocks.ids() {
            let def = blocks.def(id);
            if let Some(named) = &def.material
                && !self.materials.iter().any(|m| &m.name == named)
            {
                return Err(format!("block `{}` sounds as material `{named}`, which {RECORDINGS} does not have", def.name));
            }
        }
        for material in &self.materials {
            let name = &material.name;
            if name.is_empty() {
                return Err(format!("{RECORDINGS}: a material without a name"));
            }
            if material.tags.is_empty() {
                return Err(format!("{RECORDINGS}: material `{name}` has no tags, so it is no block's"));
            }
            if Cue::ALL.into_iter().all(|cue| material.clips(cue).is_none()) {
                return Err(format!("{RECORDINGS}: material `{name}` has no step, place or break clips"));
            }
            if !blocks.ids().map(|id| blocks.def(id)).any(|def| material.fits(def) || def.material.as_ref() == Some(name)) {
                return Err(format!("{RECORDINGS}: no block has all of material `{name}`'s tags {:?}", material.tags));
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
        let Some(clip) = sfx.clips(cue).and_then(|clips| clips.get((pick * clips.len() as f32) as usize)) else {
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

/// Plays something catching fire, from the block, as the server says it caught.
fn play_catches(
    mut commands: Commands,
    mut caught: MessageReader<Catches>,
    sfx: Option<ResMut<Sfx>>,
    settings: Res<Settings>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
) {
    let Some(mut sfx) = sfx else { return };
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    let listener = listener.single().map(|t| t.translation()).ok();
    for &Catches { at } in caught.read() {
        let gain = falloff(listener.map_or(0.0, |ear| at.distance(ear)), HEARD_WITHIN);
        let (pick, jitter) = (sfx.random(), sfx.random());
        let Some(clip) = sfx.catch.get((pick * sfx.catch.len() as f32) as usize) else { continue };
        if gain <= 0.0 {
            continue;
        }
        let speed = clip.speed * (0.94 + 0.12 * jitter);
        let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(clip.gain * gain * master)) };
        commands.spawn((
            AudioPlayer(clip.source.clone()),
            PlaybackSettings { spatial: true, spatial_scale: Some(SpatialScale::new(PAN_SCALE)), ..playback },
            Transform::from_translation(at),
            Voice::at(Category::Everyday, at),
        ));
    }
}

/// The player's own actions heard: on the click, as what they bring lands, and as the
/// server does them; the click first, so an integrated server's answer in the same frame
/// finds it.
fn action_sounds() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (play_actions, play_brought, play_done.in_set(Audible)).chain().into_configs()
}

/// Plays an action the client sees done, from where it is done, as the server does it for
/// a click of the player's.
fn play_done(
    mut commands: Commands,
    time: Option<Res<Time>>,
    mut done: MessageReader<Done>,
    sfx: Option<ResMut<Sfx>>,
    settings: Res<Settings>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
) {
    let Some(mut sfx) = sfx else { return };
    let now = time.map_or(0.0, |time| time.elapsed_secs());
    sfx.clicked.retain(|&(_, _, until)| until >= now);
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    let listener = listener.single().map(|t| t.translation()).ok();
    for &Done { at: cell, action, with } in done.read() {
        // Only the block the player clicked, and only once: a bear's raid on a skep, or
        // another's cut of the next one, is not the player's.
        let Some(click) = sfx.clicked.iter().position(|&(a, clicked, _)| a == action && clicked == cell) else { continue };
        sfx.clicked.remove(click);
        let at = cell.as_vec3() + Vec3::splat(0.5);
        let gain = falloff(listener.map_or(0.0, |ear| at.distance(ear)), HEARD_WITHIN);
        let (pick, jitter) = (sfx.random(), sfx.random());
        let way = sfx.ways.get(&action).and_then(|ways| way_of(ways, &[with]));
        let Some(clip) = way.or_else(|| sfx.actions.get(&action)).and_then(|clips| clips.get((pick * clips.len() as f32) as usize)) else { continue };
        if gain <= 0.0 {
            continue;
        }
        let speed = clip.speed * (0.96 + 0.08 * jitter);
        let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(clip.gain * gain * master)) };
        commands.spawn((
            AudioPlayer(clip.source.clone()),
            PlaybackSettings { spatial: true, spatial_scale: Some(SpatialScale::new(PAN_SCALE)), ..playback },
            Transform::from_translation(at),
            Voice::at(Category::Everyday, at),
        ));
    }
}

/// Which clips each building material's cues were asked for, as asset paths: once for each
/// block and cue, kept whether or not sound is on, so a silent run (the gate) can tell a
/// block of a material sounded as that material.
#[derive(Resource, Default, Debug)]
pub struct MaterialsAsked(pub Vec<(BlockId, Cue, Vec<String>)>);

fn ask_materials(mut cues: MessageReader<SoundCue>, sfx: Option<Res<Sfx>>, assets: Res<AssetServer>, mut asked: ResMut<MaterialsAsked>) {
    let Some(sfx) = sfx else {
        cues.clear();
        return;
    };
    for cue in cues.read() {
        let Some(block) = cue.block else { continue };
        let Some(m) = sfx.material.get(block.0 as usize).copied().flatten() else { continue };
        if asked.0.iter().any(|&(b, c, _)| b == block && c == cue.cue) {
            continue;
        }
        let paths = sfx.materials[m].get(&cue.cue).into_iter().flatten().filter_map(|clip| assets.get_path(&clip.source)).map(|path| path.to_string()).collect();
        asked.0.push((block, cue.cue, paths));
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
    for proposed in proposed {
        let action = proposed.action;
        if sfx.brings.contains_key(&action) && awaits(sfx.ways.get(&action).map_or(&[], Vec::as_slice), &proposed.with) {
            let within = sfx.brought_within.get(&action).copied().unwrap_or(BROUGHT_WITHIN);
            sfx.awaiting.push((action, now + within));
        }
        // Heard when the server does it (`play_done`), never on the click.
        if sfx.when_done.contains(&action) {
            if let voxmar_engine::action::ActionTarget::Block { block, .. } = proposed.target {
                sfx.clicked.push((action, block, now + DONE_WITHIN));
            }
            continue;
        }
        let (pick, jitter) = (sfx.random(), sfx.random());
        // The way it is done (flint knapped, a hearth kindled), else the action's own.
        let way = sfx.ways.get(&action).and_then(|ways| way_of(ways, &proposed.with));
        let Some(clip) = way.or_else(|| sfx.actions.get(&action)).and_then(|clips| clips.get((pick * clips.len() as f32) as usize)) else { continue };
        play_clip(&mut commands, clip, master, jitter);
    }
}

/// Seconds after an action is done that what it brings may still land and be heard: a
/// line's five seconds in the water, and the server's word coming back.
const BROUGHT_WITHIN: f32 = 10.0;

/// Seconds after the click on an action heard when done that the server's doing it is
/// taken for that click.
const DONE_WITHIN: f32 = 3.0;

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
/// first awaited whose rules take it, by its first rule whose tags the item all has.
fn answered<'a>(
    awaiting: &[(voxmar_engine::action::ActionId, f32)],
    brings: &'a Brings,
    has: impl Fn(&str) -> bool,
) -> Option<(usize, &'a [Clip])> {
    awaiting.iter().enumerate().find_map(|(i, (action, _))| {
        let rules = brings.get(action)?;
        rules.iter().find(|(tags, _)| tags.iter().all(|tag| has(tag))).map(|(_, clips)| (i, clips.as_slice()))
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
                .insert_resource(Sfx { clips, materials: Vec::new(), material: Vec::new(), thunder: rolls, actions: FxHashMap::default(), brings: FxHashMap::default(), ways: FxHashMap::default(), awaiting: Vec::new(), when_done: FxHashSet::default(), brought_within: FxHashMap::default(), clicked: Vec::new(), catch: Vec::new(), counter: 0 })
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
        for wrong in [r#"(actions: {"core:juggle": (sounds: ["a.ogg"])})"#, r#"(actions: {"core:eat": (sounds: [])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], volume: 0.0)})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], silent: true)})"#, r#"(actions: {"core:eat": (silent: true, heard: Some(Block))})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(tags: ["x"], sounds: [])])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(tags: [""], sounds: ["b.ogg"])])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(sounds: ["b.ogg"])])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brings: [(tags: ["x"], sounds: ["b.ogg"], volume: 3.0)])})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], when_done: true)})"#, r#"(actions: {"base:tend_bees": (silent: true, when_done: true)})"#, r#"(actions: {"base:tend_bees": (sounds: ["a.ogg"])})"#, r#"(actions: {"base:fish": (sounds: ["a.ogg"], brings: [(tags: ["x"], sounds: ["b.ogg"])], brought_within: Some(0.0))})"#, r#"(actions: {"core:eat": (sounds: ["a.ogg"], brought_within: Some(2.0))})"#] {
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

    /// The work of the first two ages is heard by what it is done with: a craft by its
    /// hardest ingredient, a light, a station loaded or a kiln fed by the block worked, an
    /// animal tended by what is in hand, bees by the hive, a door or gate by what swings. Every
    /// home station loads with a sound of its own, and both age gates kindle. A way that is
    /// unnamed, named twice, done with nothing or with what the content lacks, without clips
    /// or volume, or on an action with no clips of its own fails the launch check.
    #[test]
    fn work_is_heard_by_what_it_is_done_with() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let recorded = Recordings::load(&content.files).unwrap();
        assert_eq!(recorded.check_ways(blocks), Ok(()));
        let id = |name: &str| blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("{name}"));
        let way = |action: &str, with: &[&str]| recorded.way_named(blocks, action, &with.iter().map(|name| id(name)).collect::<Vec<_>>()).map(str::to_owned);
        for (with, heard) in [
            (&["flint", "stick"][..], Some("knap")),
            (&["cobblestone", "stick", "twine"], Some("dress_stone")),
            (&["hide"], Some("scrape_hide")),
            (&["clay"], Some("shape_clay")),
            (&["fired_clay"], Some("clink")),
            (&["wheat"], Some("twist")),
            (&["oak_planks"], Some("whittle")),
            (&["oak_log"], Some("whittle")),
            (&["iron_ingot"], None),
        ] {
            assert_eq!(way("core:craft", with), heard.map(str::to_owned), "crafting with {with:?}");
        }
        assert_eq!(way("base:light", &["hearthstone"]), Some("kindle_hearth".into()));
        assert_eq!(way("base:light", &["crucible", "charcoal"]), Some("kindle_kiln".into()));
        assert_eq!(way("base:cook", &["pit_kiln", "clay"]), Some("fire_in_kiln".into()));
        assert_eq!(way("base:cook", &["crucible", "copper_ore"]), Some("smelt".into()));
        assert_eq!(way("base:cook", &["campfire", "raw_beef"]), None);
        // The Farm age's hand stations are heard at their work, never as food on a fire,
        // and what each gives as it comes off them.
        assert_eq!(way("base:cook", &["quern_stones", "wheat"]), Some("grind".into()));
        assert_eq!(way("base:cook", &["spindle_stand", "wool"]), Some("spin".into()));
        assert_eq!(way("base:cook", &["loom_frame", "wool"]), Some("weave".into()));
        assert_eq!(way("base:cook", &["old_quern_stones", "wheat"]), None, "a village's quern works nothing");
        let gives = |item: &str| recorded.actions["base:cook"].brings.iter().find(|b| b.tags.iter().all(|t| blocks.def(id(item)).has_tag(t))).map(|b| b.tags.join("+"));
        for (item, heard) in [("flour", "flour"), ("twine", "twine"), ("red_cloth", "cloth"), ("cooked_beef", "cooked"), ("burnt_food", "burnt")] {
            assert_eq!(gives(item).as_deref(), Some(heard), "{item}");
        }
        // An animal tended with shears or a pail is silent on the click, and heard by the
        // wool or milk the server gives; fed, as ever. A swarm is heard by the skep of bees
        // it brings, a full skep's honey as the server cuts it (`Done`), never on the click.
        assert_eq!(way("base:tend", &["shears"]), Some("shear".into()));
        assert_eq!(way("base:tend", &["pail"]), Some("milk".into()));
        assert_eq!(way("base:tend", &["wheat"]), None);
        assert!(recorded.actions["base:tend"].ways.iter().all(|w| w.silent && w.sounds.is_empty()));
        let brings = |name: &str| recorded.actions[name].brings.iter().map(|b| b.tags.join("+")).collect::<Vec<_>>();
        assert_eq!(brings("base:tend"), ["wool", "dairy"]);
        assert_eq!(brings("base:tend_bees"), ["bees"]);
        for (item, tag) in [("wool", "wool"), ("milk", "dairy"), ("bee_skep", "bees")] {
            assert!(blocks.def(id(item)).has_tag(tag), "{item}");
        }
        assert!(recorded.actions["base:tend_bees"].when_done);
        assert!(bees_taken(&content.bees, id("honey_skep"), id("bee_skep")), "honey cut");
        assert!(!bees_taken(&content.bees, id("bee_nest"), BlockId::AIR), "a nest gone may be broken");
        assert!(!bees_taken(&content.bees, id("bee_skep"), id("honey_skep")), "a skep filling");
        // What is swung: a door as ever, a gate, shutters, a hatch, a curtain each its own.
        for (block, heard) in [("wooden_door", None), ("balcony_door", None), ("wooden_door_open", Some("shut")), ("wooden_door_open_top", Some("shut")), ("balcony_door_open", Some("shut")), ("oak_fence_gate", Some("gate")), ("oak_fence_gate_open", Some("gate")), ("wooden_shutters", Some("shutters")), ("oak_shuttered_window", Some("shutters")), ("oak_hatch", Some("hatch")), ("red_curtain", Some("curtain"))] {
            assert_eq!(way("base:open", &[block]), heard.map(str::to_owned), "{block}");
        }
        // A store by what is opened; a bed and a seat by what they are made of.
        for (action, block, heard) in [
            ("base:open_store", "chest", None),
            ("base:open_store", "crate", None),
            ("base:open_store", "barrel", Some("barrel")),
            ("base:open_store", "oak_barrel_rack", Some("barrel")),
            ("base:open_store", "oak_cupboard", Some("cupboard")),
            ("base:open_store", "dark_oak_wardrobe", Some("cupboard")),
            ("base:open_store", "earthen_jar", Some("jar")),
            ("base:open_store", "cobalt_basket", Some("basket")),
            ("base:sleep", "sleeping_bag", None),
            ("base:sleep", "oak_bed", Some("bed_frame")),
            ("base:sit", "birch_stool", Some("wooden_seat")),
            ("base:sit", "oak_settle", Some("wooden_seat")),
            ("base:sit", "red_cushion", None),
        ] {
            assert_eq!(way(action, &[block]), heard.map(str::to_owned), "{action} at {block}");
        }
        // Each station loaded at its empty block sounds of its own; emptied, as before.
        for station in &content.stations.stations {
            let name = &blocks.def(station.empty).name;
            assert!(recorded.way_named(blocks, "base:use_station", &[station.empty]).is_some(), "loading {name} has no sound of its own");
            assert_eq!(recorded.way_named(blocks, "base:use_station", &[station.ready]), None, "{name}");
        }
        // Both age gates kindle: every core a multiblock of the first two ages stands round.
        for core in ["hearthstone", "crucible"] {
            assert!(way("base:light", &[core]).is_some(), "{core}");
        }
        let file = |source: &str| ContentFile { package: "mod".into(), name: RECORDINGS.into(), source: source.into() };
        let fine = r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "knap", with: ["base:flint", "tag:log"], sounds: ["k.ogg"], volume: 1.5)])})"#;
        let fine = Recordings::load(&[file(fine)]).unwrap();
        assert_eq!(fine.actions["core:craft"].ways[0].sounds, ["content/mod/k.ogg"]);
        assert_eq!(fine.check_ways(blocks), Ok(()));
        // A bare name is the package's own: the base package's flint, but no flint of a mod's.
        let base = ContentFile { package: "base".into(), name: RECORDINGS.into(), source: r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "knap", with: ["flint"], sounds: ["k.ogg"])])})"#.into() };
        assert_eq!(Recordings::load(&[base]).unwrap().check_ways(blocks), Ok(()));
        let bare = r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "knap", with: ["flint"], sounds: ["k.ogg"])])})"#;
        assert!(Recordings::load(&[file(bare)]).unwrap().check_ways(blocks).is_err());
        // Every shipped way's clips are there; one a mod mistyped is not, and fails the check.
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        assert_eq!(recorded.check_files(&assets), Ok(()));
        assert!(fine.check_files(&assets).is_err(), "content/mod/k.ogg is not there");
        // So are an action's own clips, what it brings and a material's.
        for missing in [
            r#"(actions: {"core:craft": (sounds: ["c.ogg"])})"#,
            r#"(actions: {"core:craft": (silent: true, brings: [(tags: ["fish"], sounds: ["f.ogg"])])})"#,
            r#"(materials: [(name: "rough", tags: ["stone"], step: ["s.ogg"])])"#,
            r#"(materials: [(name: "rough", tags: ["stone"], place: ["p.ogg"])])"#,
            r#"(materials: [(name: "rough", tags: ["stone"], break: ["b.ogg"])])"#,
            r#"(interface: {"click": (sounds: ["c.ogg"])})"#,
            r#"(thunder: (near: ["n.ogg"]))"#,
            r#"(thunder: (middle: ["m.ogg"]))"#,
            r#"(thunder: (far: ["f.ogg"]))"#,
            r#"(swim: ["s.ogg"])"#,
            r#"(splash: ["s.ogg"])"#,
            r#"(pickup: ["p.ogg"])"#,
            r#"(quest_done: ["q.ogg"])"#,
            r#"(quest_claim: ["q.ogg"])"#,
            r#"(level_up: ["l.ogg"])"#,
            r#"(level_milestone: ["l.ogg"])"#,
            r#"(fire: (catch: ["c.ogg"]))"#,
        ] {
            assert!(Recordings::load(&[file(missing)]).unwrap().check_files(&assets).is_err(), "{missing}");
        }
        for wrong in [
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(with: ["base:flint"], sounds: ["k.ogg"])])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flint"], sounds: ["k.ogg"]), (name: "a", with: ["base:clay"], sounds: ["k.ogg"])])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", sounds: ["k.ogg"])])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flimt"], sounds: ["k.ogg"])])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["tag:nothing_has_this"], sounds: ["k.ogg"])])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flint"])])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flint"], sounds: ["k.ogg"], volume: 0.0)])})"#,
            r#"(actions: {"core:craft": (silent: true, ways: [(name: "a", with: ["base:flint"], sounds: ["k.ogg"])])})"#,
            // A silent way says it once, and needs what the action brings to be heard by.
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flint"], sounds: ["k.ogg"], silent: true)])})"#,
            r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flint"], silent: true)])})"#,
        ] {
            assert!(Recordings::load(&[file(wrong)]).unwrap().check_ways(blocks).is_err(), "{wrong}");
        }
        let heard_by_brings = r#"(actions: {"core:craft": (sounds: ["c.ogg"], ways: [(name: "a", with: ["base:flint"], silent: true)], brings: [(tags: ["x"], sounds: ["b.ogg"])])})"#;
        assert_eq!(Recordings::load(&[file(heard_by_brings)]).unwrap().check_ways(blocks), Ok(()));
    }

    /// A later package replaces a material by its name and adds others; a block sounds as
    /// the material its def names, else the one whose tags it all carries, the one naming
    /// the most of them if several fit; taps and landings take the steps. A material that is
    /// unnamed, has no tags or clips, or fits no block fails the launch check, and so does a
    /// block naming a material there is not.
    #[test]
    fn materials_are_checked_and_the_closest_fit_sounds() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let file = |package: &str, source: &str| ContentFile { package: package.into(), name: RECORDINGS.into(), source: source.into() };
        let base = file("base", r#"(materials: [(name: "rough", tags: ["stone"], step: ["s.ogg"], break: ["b.ogg"]), (name: "cobblestone", tags: ["cobbled"], place: ["p.ogg"]), (name: "stone_brick", tags: ["bricks"], step: ["t.ogg"])])"#);
        let recorded = Recordings::load(&[base.clone(), file("mod", r#"(materials: [(name: "cobblestone", tags: ["stone", "cobbled"], step: ["x.ogg"]), (name: "mossy", tags: ["stone", "cobbled", "mossy"], break: ["m.ogg"])])"#)]).unwrap();
        assert_eq!(recorded.materials.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["rough", "cobblestone", "stone_brick", "mossy"]);
        let cobble = &recorded.materials[1];
        assert_eq!((cobble.step.as_slice(), cobble.place.is_empty()), (&["content/mod/x.ogg".to_owned()][..], true));
        assert_eq!(recorded.check_materials(blocks), Ok(()));
        let named = |name: &str| recorded.material_of(blocks, blocks.id(name).unwrap()).map(|m| recorded.materials[m].name.as_str());
        assert_eq!(named("base:stone"), Some("rough"));
        assert_eq!(named("base:cobblestone"), Some("cobblestone"));
        assert_eq!(named("base:mossy_cobblestone"), Some("mossy"));
        assert_eq!(named("base:oak_planks"), None);
        // Cut cobblestone carries only `stone` (it fits "rough"), but its def names cobblestone.
        let slab = blocks.def(blocks.id("base:cobblestone_slab").unwrap());
        assert!(recorded.materials[0].fits(slab) && !cobble.fits(slab));
        assert_eq!(named("base:cobblestone_slab"), Some("cobblestone"));
        assert_eq!(cobble.clips(Cue::Hit), Some((&cobble.step[..], 1.0)));
        assert_eq!(cobble.clips(Cue::Land), Some((&cobble.step[..], LANDING_PITCH)));
        assert_eq!((cobble.clips(Cue::Place), cobble.clips(Cue::Break), cobble.clips(Cue::Pickup)), (None, None, None));
        for wrong in [
            r#"(materials: [(tags: ["stone"], step: ["s.ogg"])])"#,
            r#"(materials: [(name: "any", step: ["s.ogg"])])"#,
            r#"(materials: [(name: "mute", tags: ["stone"])])"#,
            r#"(materials: [(name: "typo", tags: ["stoen"], step: ["s.ogg"])])"#,
        ] {
            let result = Recordings::load(&[base.clone(), file("mod", wrong)]).unwrap().check_materials(blocks);
            assert!(result.is_err(), "{wrong}");
        }
        // Without the stone_brick material, the cut stone bricks name one there is not.
        let unnamed = Recordings::load(&[file("base", r#"(materials: [(name: "cobblestone", tags: ["cobbled"], place: ["p.ogg"])])"#)]).unwrap().check_materials(blocks);
        assert!(unnamed.as_ref().is_err_and(|e| e.contains("stone_brick")), "{unnamed:?}");
    }

    /// Catching fire is the server's block change to fire.ron's burning block, from
    /// anything else loaded; a fire that can burn must have a catch sound, at a sane volume,
    /// and a later package replaces it. The one we ship decodes, at the loudness of the rest.
    #[test]
    fn catching_fire_is_heard_from_the_server_and_checked() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (fire, blocks) = (&content.fire, &content.blocks);
        let (burning, thatch) = (fire.burning.unwrap(), blocks.id("base:thatch_roof").unwrap());
        assert!(catches(fire, Some(thatch), burning));
        assert!(!catches(fire, Some(burning), burning), "already burning");
        assert!(!catches(fire, None, burning), "not loaded: not seen to catch");
        assert!(!catches(fire, Some(burning), BlockId::AIR), "burning out is no catch");
        assert!(!catches(fire, Some(thatch), BlockId::AIR), "broken is no catch");
        let recorded = Recordings::load(&content.files).unwrap();
        assert_eq!(recorded.check_fire(fire), Ok(()));
        let silent = Recordings::default().check_fire(fire);
        assert!(silent.as_ref().is_err_and(|e| e.contains("catch")), "{silent:?}");
        let file = |package: &str, source: &str| ContentFile { package: package.into(), name: RECORDINGS.into(), source: source.into() };
        let loud = Recordings::load(&[file("base", r#"(fire: (catch: ["c.ogg"], volume: 3.0))"#)]).unwrap().check_fire(fire);
        assert!(loud.is_err(), "{loud:?}");
        let replaced = Recordings::load(&[file("base", r#"(fire: (catch: ["c.ogg"]))"#), file("mod", r#"(fire: (catch: ["d.ogg"], volume: 0.5))"#)]).unwrap();
        assert_eq!(replaced.fire, FireSounds { catch: vec!["content/mod/d.ogg".into()], volume: 0.5 });
        for path in &recorded.fire.catch {
            let [left, right] = crate::ambience::tests::decode(path);
            let stereo: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
            let heard = loudness(&stereo, 2) / ACTION_LOUDNESS;
            assert!((0.5..=1.25).contains(&heard), "{path} is heard at {heard:.2} of the loudness of the rest");
        }
    }

    /// The building materials we ship pass the launch check, sound for the blocks they are
    /// meant for, and are made at [`ACTION_LOUDNESS`] (beside the family cues they stand
    /// in for) and short: a step shorter than a stride, a break under a second.
    #[test]
    fn shipped_materials_are_heard_at_one_loudness() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let recorded = Recordings::load(&content.files).unwrap();
        recorded.check_materials(blocks).unwrap();
        for (block, material) in [("base:cobblestone", "cobblestone"), ("base:mossy_cobblestone", "cobblestone"), ("base:stone_bricks", "stone_brick"), ("base:cracked_stone_bricks", "stone_brick"),
            ("base:cobblestone_slab", "cobblestone"),
            ("base:cobblestone_stairs_top_east", "cobblestone"),
            ("base:stone_brick_stairs", "stone_brick"),
            ("base:oak_post", "timber"),
            ("base:birch_beam", "timber"),
            ("base:dark_oak_lattice_window", "timber"),
            ("base:thatch_roof", "thatch"),
            ("base:thatch_ridge", "thatch"),
            ("base:oak_shingles", "shingle"),
            ("base:dark_oak_shingle_ridge", "shingle"),
            ("base:wattle_panel", "wattle"),
            ("base:daub", "daub"),
            ("base:hide_lean_to", "hide"),
            ("base:half_timber", "timber"),
            ("base:chimney_stack", "stone_brick"),
            ("base:chimney_pot", "terracotta"),
        ] {
            let found = recorded.material_of(blocks, blocks.id(block).unwrap()).map(|m| recorded.materials[m].name.as_str());
            assert_eq!(found, Some(material), "{block}");
        }
        for material in &recorded.materials {
            for (paths, longest) in [(&material.step, 0.3), (&material.place, 0.6), (&material.breaks, 1.0)] {
                assert!(!paths.is_empty(), "{} lacks a cue", material.name);
                for path in paths {
                    let [left, right] = crate::ambience::tests::decode(path);
                    let stereo: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
                    let heard = loudness(&stereo, 2) / ACTION_LOUDNESS;
                    assert!((0.5..=1.25).contains(&heard), "{path} is heard at {heard:.2} of the loudness of the rest");
                    assert!(left.len() as f32 / (RATE as f32) < longest, "{path} is longer than {longest} s");
                }
            }
        }
    }

    /// A building piece cut or shaped from a block that sounds as a building material
    /// sounds as it too: every `building` block drawn with the side texture of a material's
    /// block sounds as that material, so a slab never sounds unlike the wall it was cut from.
    /// (Stations made of the stuff, a quern of cobbles, keep their own sound.)
    #[test]
    fn pieces_sound_as_what_they_look_like() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let blocks = &content.blocks;
        let recorded = Recordings::load(&content.files).unwrap();
        let side = |id: BlockId| blocks.def(id).textures.face(voxmar_engine::coords::Face::PosX).map(str::to_owned);
        let mut by_look: FxHashMap<String, (usize, &str)> = FxHashMap::default();
        for id in blocks.ids().filter(|&id| blocks.def(id).render == voxmar_engine::block::RenderKind::Opaque) {
            if let (Some(look), Some(material)) = (side(id), recorded.material_of(blocks, id)) {
                by_look.entry(look).or_insert((material, &blocks.def(id).name));
            }
        }
        assert!(by_look.len() >= 4, "{by_look:?}");
        let mut checked = 0;
        let mut unlike = Vec::new();
        for id in blocks.ids().filter(|&id| blocks.def(id).has_tag("building")) {
            let Some((material, whole)) = side(id).and_then(|look| by_look.get(&look)) else { continue };
            let found = recorded.material_of(blocks, id).map(|m| recorded.materials[m].name.as_str());
            if found != Some(recorded.materials[*material].name.as_str()) {
                unlike.push(format!("{} sounds as {found:?} but looks like {whole}", blocks.def(id).name));
            }
            checked += 1;
        }
        assert!(unlike.is_empty(), "{unlike:#?}");
        assert!(checked > by_look.len(), "no pieces share a material block's look");
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
            "base:tend_bees",
            "base:empty_pouch",
            "base:use_machine",
            "base:pick_lock",
            "base:pray",
            "base:fertilise",
        ] {
            assert!(recorded.actions.contains_key(name), "no sound for {name}");
        }
        assert!(!recorded.pickup.is_empty(), "no pickup pop");
        // What fishing and cooking bring is heard as it lands: a catch, food done or burnt,
        // and the flour, twine and cloth of the Farm age's hand stations.
        let brings = |name: &str| recorded.actions[name].brings.iter().map(|b| b.tags.join("+")).collect::<Vec<_>>();
        assert_eq!(brings("base:fish"), ["fish+raw"]);
        assert_eq!(brings("base:cook"), ["burnt", "cooked", "flour", "twine", "cloth"]);
        // Every tag a result is told by is one some item has.
        for brought in recorded.actions.values().flat_map(|sound| &sound.brings) {
            for tag in &brought.tags {
                assert!((0..content.blocks.len()).any(|id| content.blocks.def(BlockId(id as u16)).has_tag(tag)), "no item is tagged {tag}");
            }
        }
        let brought = recorded.actions.values().flat_map(|sound| sound.brings.iter().flat_map(|b| b.sounds.iter().map(move |path| (path, b.volume))));
        let ways = recorded.actions.values().flat_map(|sound| &sound.ways).flat_map(|way| way.sounds.iter().map(|path| (path, way.volume)));
        let clips = recorded.actions.values().flat_map(|sound| sound.sounds.iter().map(|path| (path, sound.volume))).chain(brought).chain(ways).chain(recorded.pickup.iter().map(|path| (path, 1.0)));
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
            for way in &sound.ways {
                let gain = way.volume * Cue::Place.gain() * level(&target, Cue::Place) / ACTION_LOUDNESS;
                heard.extend(way.sounds.iter().map(|path| (path.clone(), measure(path)[0] * gain)));
            }
        }
        let everyday = within(crate::mix::Category::Everyday, heard);
        let thunder = [(&recorded.thunder.near, 0.0), (&recorded.thunder.middle, NEAR_LIGHTNING), (&recorded.thunder.far, FAR_LIGHTNING)]
            .into_iter()
            .flat_map(|(paths, distance)| paths.iter().map(move |path| (path.clone(), measure(path)[0] * thunder_gain(distance))));
        let weather = within(crate::mix::Category::Weather, thunder);
        crate::mix::tests::all([everyday, weather]);
    }

    /// An action with ways heard only by what they bring awaits it only when done one of
    /// those; one without (fishing) always does.
    #[test]
    fn only_a_way_heard_by_its_result_awaits_it() {
        let set = |ids: &[u16]| ids.iter().map(|&id| BlockId(id)).collect::<FxHashSet<_>>();
        let tend = [(set(&[1]), Vec::<u8>::new()), (set(&[2]), vec![7])];
        assert!(awaits(&tend, &[BlockId(1)]), "shears");
        assert!(!awaits(&tend, &[BlockId(2)]), "a way with its own clips");
        assert!(!awaits(&tend, &[BlockId(3)]), "food");
        assert!(awaits::<u8>(&[], &[BlockId(3)]), "no ways: fishing");
        assert!(awaits(&[(set(&[2]), vec![7u8])], &[BlockId(3)]), "ways all with clips");
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
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        brings.insert(cook, vec![(tags(&["burnt"]), vec![clip(1.0)]), (tags(&["cooked"]), vec![clip(2.0)])]);
        brings.insert(fish, vec![(tags(&["fish", "raw"]), vec![clip(3.0)])]);
        let gain = |found: Option<(usize, &[Clip])>| found.map(|(at, clips)| (at, clips[0].gain));
        let item = |has: &'static [&'static str]| move |tag: &str| has.contains(&tag);
        assert_eq!(gain(answered(&[(cook, 1.0)], &brings, item(&["burnt"]))), Some((0, 1.0)));
        assert_eq!(gain(answered(&[(cook, 1.0)], &brings, item(&["cooked", "meat"]))), Some((0, 2.0)));
        assert_eq!(gain(answered(&[(fish, 1.0), (cook, 1.0)], &brings, item(&["fish", "raw"]))), Some((0, 3.0)));
        // A cooked fish answers the meal, not a line still out; a stick picked up meanwhile
        // answers neither.
        assert_eq!(gain(answered(&[(fish, 1.0), (cook, 1.0)], &brings, item(&["fish", "cooked"]))), Some((1, 2.0)));
        assert_eq!(gain(answered(&[(fish, 1.0), (cook, 1.0)], &brings, item(&["stick"]))), None);
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
                .insert_resource(Sfx { clips: FxHashMap::default(), materials: Vec::new(), material: Vec::new(), thunder: Default::default(), actions, brings: FxHashMap::default(), ways: FxHashMap::default(), awaiting: Vec::new(), when_done: FxHashSet::default(), brought_within: FxHashMap::default(), clicked: Vec::new(), catch: Vec::new(), counter: 0 })
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

    /// A full skep's honey is silent on the click and heard as the server cuts it, once;
    /// a skep emptied with no click of the player's on it (a bear's raid, another's cut of
    /// the one beside it) is not heard.
    #[test]
    fn honey_is_heard_as_cut_never_on_the_click() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let tend_bees = content.actions.expect("base:tend_bees");
        let skep = content.blocks.id("base:honey_skep").unwrap();
        let (connection, _server_side) = voxmar_protocol::local_pair();
        let net = crate::net::Net::new(connection, None, "tester".into(), content.fingerprint(), None);
        let mut actions = FxHashMap::default();
        actions.insert(tend_bees, vec![Clip { source: Handle::default(), gain: 1.0, speed: 1.0 }]);
        let mut app = App::new();
        app.insert_resource(GlobalVolume::new(Volume::Linear(1.0)))
            .insert_resource(Settings::default())
            .insert_resource(Sfx { clips: FxHashMap::default(), materials: Vec::new(), material: Vec::new(), thunder: Default::default(), actions, brings: FxHashMap::default(), ways: FxHashMap::default(), awaiting: Vec::new(), when_done: [tend_bees].into_iter().collect(), brought_within: FxHashMap::default(), clicked: Vec::new(), catch: Vec::new(), counter: 0 })
            .insert_resource(net)
            .insert_resource(Time::<()>::default())
            .insert_resource(LocalPlayer::default())
            .add_message::<Done>()
            .add_systems(Update, action_sounds());
        let playing = |app: &mut App| app.world_mut().query::<&AudioPlayer>().iter(app.world()).count();
        let cut = Done { at: IVec3::new(4, 64, 2), action: tend_bees, with: skep };
        let next_skep = Done { at: IVec3::new(5, 64, 2), ..cut };
        app.world_mut().write_message(cut);
        app.update();
        assert_eq!(playing(&mut app), 0, "no click: a bear's raid");
        let target = voxmar_engine::action::ActionTarget::Block { block: cut.at, face: voxmar_engine::coords::Face::PosY };
        app.world().resource::<crate::net::Net>().propose_with(tend_bees, target, vec![skep]);
        app.update();
        assert_eq!(playing(&mut app), 0, "the click is silent");
        app.world_mut().write_message(next_skep);
        app.update();
        assert_eq!(playing(&mut app), 0, "the skep beside it, cut by someone else");
        app.world_mut().write_message(cut);
        app.update();
        assert_eq!(playing(&mut app), 1, "the cut is heard");
        app.world_mut().write_message(cut);
        app.update();
        assert_eq!(playing(&mut app), 1, "once for the click");
        // The click and the server's cut in one frame (an integrated server answering at
        // once) are heard: the click is taken first.
        app.world().resource::<crate::net::Net>().propose_with(tend_bees, target, vec![skep]);
        app.world_mut().write_message(cut);
        app.update();
        assert_eq!(playing(&mut app), 2, "the click and its cut in one frame");
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
