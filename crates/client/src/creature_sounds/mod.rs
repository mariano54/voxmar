//! Creature sounds: footsteps, bleats, grunts, calls, snarls, cries of pain and the rest,
//! from a moddable sound bank (`creature_sounds.ron` in each package).
//!
//! The creatures module says what each creature just did ([`CreatureSound`]: a step, a
//! call, a hurt) into [`CreatureSounds`]; this module decides what that sounds like and
//! plays it. A package names *voices* (the sounds of a sheep, a deer, a wolf, each event
//! several recordings) and gives each species one; a voice can be `like` another for the
//! sounds it lacks, and a missing event falls back to the nearest one it has (an alert to
//! a call, a call to an idle sound). Later packages replace voices and species by name, so
//! a sound pack revoices the world like a texture pack repaints it.
//!
//! How a sound is played:
//! - Pitched for size. A voice says how tall the animal recorded was; a species of
//!   another height plays it higher or lower ([`defs::pitch_for_size`]), each individual a
//!   little more for its own genes, and the young higher still.
//! - Never the same twice. A clip is picked at random from several and played within a
//!   small random range of pitch and loudness.
//! - Placed in the world. Each sound comes from where the creature stands, panned to the
//!   ear it is nearer and fading to nothing at its kind's reach (a call carries far, a
//!   munch barely at all).
//! - Otherworldly where aether runs. A supernatural species' clips are shimmered once,
//!   as they load ([`aether::shimmer`]).
//! - Steps sound the ground. A creature's footfalls play the block it walks on, as the
//!   player's do, softer and higher for small ones; a voice's own step clips (a crab's
//!   claws) are layered on top.
//! - Light. A creature never talks over itself, the quietest requests are dropped first,
//!   and at most the mix's cap of creature sounds play at once (`sounds.ron`), the
//!   farthest, then the oldest, making room for a new one (`mix`).
//! - A boss's own voice (its calls, roars and cries, not its steps) is one the music dips
//!   under.

use bevy::audio::{AudioPlayer, AudioSource, GlobalVolume, PlaybackSettings, SpatialListener, SpatialScale, Volume};
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_engine::creature::{Bark, CreatureId};

use crate::session::Launch;
use crate::settings::Settings;
use crate::mix::{Category, Mix, PAN_SCALE, Voice, falloff};
use crate::sfx::{self, Cue, MASTER, SoundCue};
use crate::world::ClientWorld;

pub mod aether;
pub mod defs;

pub use defs::Bank;

/// Seconds a creature waits after one vocal sound before another (hurt and death excepted).
const BREATH: f32 = 0.35;
/// Sounds quieter than this are not played at all.
const FAINTEST: f32 = 0.004;
/// Shimmered clips made per frame, so a supernatural species arriving costs no hitch.
const SHIMMERS_PER_FRAME: usize = 3;

/// What a creature did that makes a sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A footfall.
    Step,
    /// Content noises while it wanders.
    Idle,
    /// Munching.
    Graze,
    /// It saw or heard something: a bark, a snort, a hiss.
    Alert,
    /// Settling down: a sigh, soft breathing, a purr.
    Rest,
    /// Calling to its kind across a distance.
    Call,
    /// Ready to breed: the mating call.
    Ready,
    /// Lunging or striking.
    Attack,
    Hurt,
    Death,
    /// Barks of a settlement's people (`Server::bark`): a hum of greeting, a cry of alarm, a
    /// content sigh.
    Greet,
    Alarm,
    Content,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Step,
        Kind::Idle,
        Kind::Graze,
        Kind::Alert,
        Kind::Rest,
        Kind::Call,
        Kind::Ready,
        Kind::Attack,
        Kind::Hurt,
        Kind::Death,
        Kind::Greet,
        Kind::Alarm,
        Kind::Content,
    ];

    /// The sound a bark is heard as.
    pub fn of_bark(bark: Bark) -> Self {
        match bark {
            Bark::Greet => Kind::Greet,
            Bark::Alarm => Kind::Alarm,
            Bark::Content => Kind::Content,
        }
    }

    /// What to play when a voice has nothing for this.
    pub fn fallback(self) -> Option<Kind> {
        match self {
            Kind::Ready | Kind::Alert => Some(Kind::Call),
            Kind::Call => Some(Kind::Idle),
            Kind::Attack => Some(Kind::Alert),
            Kind::Hurt => Some(Kind::Alert),
            Kind::Death => Some(Kind::Hurt),
            Kind::Greet => Some(Kind::Idle),
            Kind::Alarm => Some(Kind::Alert),
            Kind::Content => Some(Kind::Rest),
            Kind::Step | Kind::Idle | Kind::Graze | Kind::Rest => None,
        }
    }

    /// Blocks within which it is heard, unless the voice says otherwise.
    pub fn range(self) -> f32 {
        match self {
            Kind::Step => 14.0,
            Kind::Graze | Kind::Rest | Kind::Content => 10.0,
            Kind::Idle | Kind::Greet => 24.0,
            Kind::Alarm => 48.0,
            Kind::Alert | Kind::Attack => 32.0,
            Kind::Hurt | Kind::Death | Kind::Ready => 36.0,
            Kind::Call => 56.0,
        }
    }

    /// How loud it is when the voice does not say, its clips being levelled alike.
    pub fn gain(self) -> f32 {
        match self {
            Kind::Step => 0.4,
            Kind::Rest | Kind::Content => 0.3,
            Kind::Greet => 0.5,
            Kind::Alarm => 0.75,
            Kind::Graze => 0.35,
            Kind::Idle => 0.45,
            Kind::Alert | Kind::Ready => 0.6,
            Kind::Attack | Kind::Hurt | Kind::Death => 0.7,
            Kind::Call => 0.8,
        }
    }

    /// Whether it comes from the throat, so a creature cannot make two at once.
    fn vocal(self) -> bool {
        !matches!(self, Kind::Step | Kind::Graze)
    }

    /// Whether it cuts through a creature's last sound rather than waiting for it.
    fn urgent(self) -> bool {
        matches!(self, Kind::Hurt | Kind::Death | Kind::Alarm)
    }
}

/// Something a creature did that may be heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatureSound {
    pub kind: Kind,
    /// The creature's id, so that it never talks over itself.
    pub id: u32,
    pub species: CreatureId,
    /// Its feet.
    pub position: Vec3,
    /// How fast it moves across the ground, blocks a second: a running step is louder.
    pub speed: f32,
    /// Its size against its species', from its genes.
    pub scale: f32,
    pub young: bool,
    /// An answer to the player's own hand (a fed ewe's munch): always heard, as an event's
    /// `chance` thins only what a creature does unbidden.
    pub answering: bool,
}

/// What creatures did this frame that may be heard; drained each frame.
#[derive(Resource, Default)]
pub struct CreatureSounds(pub Vec<CreatureSound>);

impl CreatureSounds {
    pub fn push(&mut self, sound: CreatureSound) {
        self.0.push(sound);
    }
}

pub struct CreatureSoundsPlugin;

impl Plugin for CreatureSoundsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CreatureSounds>()
            .init_resource::<Heard>()
            .add_systems(Startup, load)
            // After the creatures are shown, so what they did this frame is asked for (and
            // heard) this frame, not the next: a death is counted and cried in one frame.
            .add_systems(Update, (shimmer_loaded, listen, play).chain().after(crate::creatures::ShowCreatures));
    }
}

/// The bank and the clips it plays.
#[derive(Resource)]
pub struct Voices {
    pub bank: Bank,
    /// Each of the bank's clips as it plays: the file, or its shimmered copy once made.
    sources: Vec<Option<Handle<AudioSource>>>,
    /// Files loading, for each clip still to be shimmered.
    raw: Vec<Handle<AudioSource>>,
    unshimmered: Vec<usize>,
    /// When each creature last made a vocal sound, in seconds since startup.
    last: FxHashMap<u32, f32>,
    counter: u64,
}

/// A uniform number in 0..1 from a counter.
fn random(counter: &mut u64) -> f32 {
    *counter += 1;
    (sfx::mix(*counter ^ 0x5eed_c0de) >> 40) as f32 / (1u64 << 24) as f32
}

impl Voices {
    /// Whether every clip the bank names has loaded (and been shimmered where it should
    /// be), or the first that failed.
    pub fn ready(&self, assets: &AssetServer) -> Result<bool, String> {
        for (key, handle) in self.bank.clips.iter().zip(&self.raw) {
            if let Some(bevy::asset::LoadState::Failed(e)) = assets.get_load_state(handle.id()) {
                return Err(format!("{}: {e}", key.path));
            }
        }
        Ok(self.unshimmered.is_empty())
    }
}

/// How many sounds of each kind were asked for and how many played, since startup.
/// How many of the latest barks [`Heard`] keeps: enough for any check, bounded in a long
/// game with villagers barking all day.
pub const BARKS_KEPT: usize = 64;

#[derive(Resource, Default, Debug)]
pub struct Heard {
    pub asked: FxHashMap<Kind, u32>,
    pub played: u32,
    /// The latest barks asked for (up to [`BARKS_KEPT`]), played or not (the gate is
    /// silent): which creature, which bark, and the clips its voice gives that bark.
    pub barks: std::collections::VecDeque<(u32, Kind, Vec<String>)>,
}

/// For the gate: every species has a voice, and every clip has loaded (and been
/// shimmered where it should be).
pub fn loaded(world: &World) -> Result<String, String> {
    let voices = world.get_resource::<Voices>().ok_or("no creature sound bank")?;
    let content = &world.resource::<Launch>().content;
    let silent: Vec<_> = content.creatures.iter().filter(|(id, _)| voices.bank.voice(*id).is_none()).map(|(_, c)| c.def.name.clone()).collect();
    if !silent.is_empty() {
        return Err(format!("no voice for {}", silent.join(", ")));
    }
    let assets = world.resource::<AssetServer>();
    let loaded = voices.raw.iter().filter(|h| assets.is_loaded(h.id())).count();
    match voices.ready(assets)? {
        true => Ok(format!("{} species voiced, {loaded} of {} clips loaded", content.creatures.len(), voices.bank.clips.len())),
        false => Err(format!("{} clips not yet shimmered, {loaded} of {} loaded", voices.unshimmered.len(), voices.bank.clips.len())),
    }
}

/// The bank the launch checks loaded (and found every settlement person barking in), handed
/// on so it is not read twice.
#[derive(Resource)]
pub struct Checked(pub Bank);

fn load(mut commands: Commands, assets: Res<AssetServer>, launch: Res<Launch>, checked: Option<ResMut<Checked>>) {
    let content = &launch.content;
    let bank = match checked {
        Some(mut checked) => {
            commands.remove_resource::<Checked>();
            std::mem::take(&mut checked.0)
        }
        None => Bank::load(&content.files, &content.creatures).unwrap_or_else(|e| {
            error!("no creature sounds: {e}");
            Bank::default()
        }),
    };
    let voiced = bank.species.iter().flatten().count();
    let silent: Vec<_> =
        content.creatures.iter().filter(|(id, _)| bank.voice(*id).is_none()).map(|(_, c)| c.def.name.as_str()).collect();
    if !silent.is_empty() {
        warn!("creatures with no voice in any {}: {}", defs::FILE, silent.join(", "));
    }
    let raw: Vec<Handle<AudioSource>> = bank.clips.iter().map(|key| assets.load(key.path.clone())).collect();
    let sources = bank.clips.iter().zip(&raw).map(|(key, handle)| (key.aether == 0).then(|| handle.clone())).collect();
    let unshimmered = bank.clips.iter().enumerate().filter(|(_, key)| key.aether > 0).map(|(i, _)| i).collect();
    info!("creature sounds: {voiced} species, {} clips", bank.clips.len());
    commands.insert_resource(Voices { bank, sources, raw, unshimmered, last: FxHashMap::default(), counter: 0 });
}

/// Shimmers supernatural species' clips as their files arrive, a few a frame.
fn shimmer_loaded(voices: Option<ResMut<Voices>>, mut sources: ResMut<Assets<AudioSource>>) {
    let Some(mut voices) = voices else { return };
    if voices.unshimmered.is_empty() {
        return;
    }
    let voices = &mut *voices;
    let mut made = 0;
    voices.unshimmered.retain(|&i| {
        if made >= SHIMMERS_PER_FRAME {
            return true;
        }
        let Some(source) = sources.get(&voices.raw[i]) else { return true };
        made += 1;
        let key = &voices.bank.clips[i];
        let shimmered = match decode_mono(source) {
            Some((samples, rate)) => {
                let seed = key.path.bytes().fold(0u64, |h, b| sfx::mix(h ^ b as u64));
                let out = aether::shimmer(&samples, rate, key.aether as f32 / 10.0, seed);
                sources.add(AudioSource { bytes: sfx::wav_at(rate, &out).into() })
            }
            None => voices.raw[i].clone(),
        };
        voices.sources[i] = Some(shimmered);
        false
    });
}

/// A clip as mono samples, and its sample rate.
fn decode_mono(source: &AudioSource) -> Option<(Vec<f32>, u32)> {
    use bevy::audio::{Decodable, Source};
    let decoder = source.decoder();
    let (channels, rate) = (decoder.channels().get() as usize, decoder.sample_rate().get());
    let samples: Vec<f32> = decoder.collect();
    let mono: Vec<f32> = samples.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect();
    (!mono.is_empty()).then_some((mono, rate))
}

/// Gives the camera ears for panning, unless something else already did.
///
/// Rodio (0.22) pans a source towards the ear *farther* from it, so the ears are given
/// swapped: a creature on the left then sounds on the left.
fn listen(mut commands: Commands, cameras: Query<Entity, With<Camera3d>>, listeners: Query<(), With<SpatialListener>>) {
    if !listeners.is_empty() {
        return;
    }
    if let Some(camera) = cameras.iter().next() {
        let ears = SpatialListener::new(0.3);
        commands.entity(camera).insert(SpatialListener { left_ear_offset: ears.right_ear_offset, right_ear_offset: ears.left_ear_offset });
    }
}

/// Whether a sound is dropped by its event's `chance` (`roll` drawn only if it may be): an
/// answer to the player's hand never is.
fn thinned(chance: f32, answering: bool, roll: impl FnOnce() -> f32) -> bool {
    !answering && chance < 1.0 && roll() > chance
}

/// A sound about to be played.
struct Voiced {
    clip: Handle<AudioSource>,
    at: Vec3,
    volume: f32,
    speed: f32,
    id: u32,
    vocal: bool,
    /// A boss's voice, which the music dips under.
    boss: bool,
}

#[allow(clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    mut asked: ResMut<CreatureSounds>,
    mut heard: ResMut<Heard>,
    voices: Option<ResMut<Voices>>,
    launch: Res<Launch>,
    world: Option<Res<ClientWorld>>,
    settings: Res<Settings>,
    global: Option<Res<GlobalVolume>>,
    time: Res<Time>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    mix: Option<Res<Mix>>,
    mut cues: MessageWriter<SoundCue>,
) {
    let asked = std::mem::take(&mut asked.0);
    for sound in &asked {
        *heard.asked.entry(sound.kind).or_default() += 1;
    }
    let Some(mut voices) = voices else { return };
    let Voices { bank, sources, last, counter, .. } = &mut *voices;
    for sound in asked.iter().filter(|s| matches!(s.kind, Kind::Greet | Kind::Alarm | Kind::Content)) {
        let clips = bank.voice(sound.species).and_then(|v| v.event(sound.kind)).map(|e| e.clips.iter().map(|&i| bank.clips[i].path.clone()).collect());
        if heard.barks.len() == BARKS_KEPT {
            heard.barks.pop_front();
        }
        heard.barks.push_back((sound.id, sound.kind, clips.unwrap_or_default()));
    }
    // A silent run (a capture, VOXMAR_MUTE, the gate) decides nothing and plays nothing.
    if asked.is_empty() || !sfx::sound_on(global) {
        return;
    }
    let Ok(ear) = listener.single().map(|t| t.translation()) else { return };
    let now = time.elapsed_secs();
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    let mut wanted: Vec<Voiced> = Vec::new();
    for sound in asked {
        let creature = &launch.content.creatures.get(sound.species).def;
        let distance = sound.position.distance(ear);
        let Some(voice) = bank.voice(sound.species) else { continue };
        // How heavy it is, by the volume of its body, against a sheep's.
        let (width, height) = creature.body.size;
        let scale = sound.scale.max(0.2) * if sound.young { 0.6 } else { 1.0 };
        let heft = (width * width * height * scale.powi(3) / 0.45).powf(0.3).clamp(0.12, 1.4);
        let size_pitch = voice.pitch / scale.sqrt();
        if let (Kind::Step, true, Some(world)) = (sound.kind, voice.ground, &world) {
            // The ground's own step, as the player's are heard, softer for light feet and
            // quicker for running ones.
            let pace = (0.75 + sound.speed / 8.0).min(1.3);
            if let Some(ground) = ground_under(world, sound.position) {
                let pitch = (1.0 / heft.sqrt()).clamp(0.8, 1.5);
                cues.write(SoundCue::for_block(world, ground, Cue::Step).at(sound.position).scaled(0.7 * heft * pace).pitched(pitch));
            }
        }
        let Some(event) = voice.event(sound.kind) else { continue };
        if thinned(event.chance, sound.answering, || random(counter)) {
            continue;
        }
        let fall = falloff(distance, event.range);
        // Big animals are louder than small ones, their steps most of all.
        let presence = if sound.kind == Kind::Step { heft } else { 0.55 + 0.45 * heft.min(1.2) };
        let volume = event.gain * voice.gain * presence * fall;
        if volume < FAINTEST {
            continue;
        }
        let pick = (random(counter) * event.clips.len() as f32) as usize;
        let Some(clip) = sources[event.clips[pick.min(event.clips.len() - 1)]].clone() else { continue };
        let jitter = random(counter);
        let speed = (size_pitch * (event.pitch.0 + (event.pitch.1 - event.pitch.0) * jitter)).clamp(0.35, 2.6);
        let loudness = 0.9 + 0.2 * random(counter);
        // From about its mouth, or its feet for a step.
        let at = sound.position + Vec3::Y * if sound.kind == Kind::Step { 0.1 } else { height * scale * 0.7 };
        wanted.push(Voiced { clip, at, volume: volume * loudness, speed, id: sound.id, vocal: sound.kind.vocal() && !sound.kind.urgent(), boss: creature.boss && sound.kind.vocal() });
        if sound.kind.urgent() {
            last.insert(sound.id, now);
        }
    }
    // The loudest first, no more at once than may play; older and farther ones make room.
    wanted.sort_by(|a, b| b.volume.total_cmp(&a.volume));
    let mut room = mix.map_or(usize::MAX, |mix| mix.0.cap(Category::Creatures));
    for voiced in wanted {
        if room == 0 {
            break;
        }
        if voiced.vocal {
            let before = last.get(&voiced.id).copied().unwrap_or(f32::MIN);
            if now - before < BREATH {
                continue;
            }
            last.insert(voiced.id, now);
        }
        room -= 1;
        heard.played += 1;
        let settings = PlaybackSettings {
            speed: voiced.speed,
            spatial: true,
            spatial_scale: Some(SpatialScale::new(PAN_SCALE)),
            ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(voiced.volume * master))
        };
        let voice = Voice { ducks: voiced.boss, ..Voice::at(Category::Creatures, voiced.at) };
        commands.spawn((AudioPlayer(voiced.clip), settings, Transform::from_translation(voiced.at), voice));
    }
    // Forget creatures long quiet, so the map stays small.
    if last.len() > 512 {
        last.retain(|_, &mut t| now - t < 10.0);
    }
}

/// The solid block under a creature's feet.
fn ground_under(world: &ClientWorld, feet: Vec3) -> Option<voxmar_engine::block::BlockId> {
    world.block((feet - Vec3::Y * 0.1).floor().as_ivec3()).filter(|&b| world.registry.info(b).solid)
}

#[cfg(test)]
mod tests;
