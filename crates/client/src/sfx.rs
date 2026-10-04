//! Sound effects for block materials, and rain and thunder.
//!
//! A block declares what it is made of (`sound: Wood`); every cue for that material (a
//! step, landing, placing, a tap while mining, breaking) comes from the player's own
//! Minecraft install when they have one (see `minecraft`), played with the volume and pitch
//! Minecraft gives that use of it, so grass, flowers, dirt and stone sound exactly as they
//! do there. Without an install, or for a cue it lacks, the cue is synthesized at startup
//! from the material.
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
//! Rain is a soft loop of tiny drops over a hiss, as loud as the rain is heavy and quieter
//! under cover. Thunder rumbles low and long, opening with a crack when the lightning is close.

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, Sound};
use voxmar_engine::physics::player as dims;

use self::minecraft::SoundStore;
use crate::player::LocalPlayer;
use crate::render::precipitation::SkyMap;
use crate::settings::Settings;
use crate::weather::{ClientWeather, Thunder};
use crate::world::ClientWorld;

mod minecraft;

/// Sample rate of every synthesized sound, shared with `ambience`.
pub(crate) const RATE: u32 = 44_100;
const VARIANTS: u64 = 4;
/// Volume at 100% in the settings.
pub(crate) const MASTER: f32 = 0.6;
/// Seconds in the rain loop, and its volume in the heaviest rain out in the open.
const RAIN_LOOP: f32 = 4.0;
const RAIN_GAIN: f32 = 0.45;
/// Thunder's volume from lightning right overhead.
const THUNDER_GAIN: f32 = 0.9;
/// Lightning closer than this many blocks cracks before it rumbles.
const NEAR_LIGHTNING: f32 = 300.0;
/// Cues placed in the world fade out over this many blocks from the listener, as
/// Minecraft's block sounds do.
const HEARD_WITHIN: f32 = 16.0;
/// Seconds of the loudest stretch of a clip, by which clips are levelled.
const LOUDNESS_WINDOW: f32 = 0.05;

pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SoundCue>()
            .add_systems(Startup, synthesize)
            .add_systems(Update, ((footsteps, play_cues).chain(), rain, thunder));
    }
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
}

impl Cue {
    const ALL: [Cue; 5] = [Cue::Step, Cue::Land, Cue::Place, Cue::Break, Cue::Hit];

    fn gain(self) -> f32 {
        match self {
            Cue::Step => 0.3,
            Cue::Land => 0.4,
            Cue::Place => 0.55,
            Cue::Break => 0.6,
            Cue::Hit => 0.22,
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
}

impl SoundCue {
    pub fn new(sound: Sound, cue: Cue) -> Self {
        Self { sound, cue, at: None, gain: 1.0 }
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
}

/// One clip a cue may play, and how loud and fast.
struct Clip {
    source: Handle<AudioSource>,
    gain: f32,
    speed: f32,
}

#[derive(Resource)]
struct Sfx {
    clips: FxHashMap<(Sound, Cue), Vec<Clip>>,
    /// Thunder from near and far lightning, a few variants each.
    thunder: [Vec<Handle<AudioSource>>; 2],
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
        Sound::Water => return (cue == Cue::Step).then(|| ("entity.player.swim".to_owned(), 0.2, 1.0)),
    };
    let (event, volume, pitch) = match cue {
        Cue::Step => ("step", 0.15, 1.0),
        Cue::Land => ("fall", 0.5, 0.75),
        Cue::Place => ("place", 1.0, 0.8),
        Cue::Break => ("break", 1.0, 0.8),
        Cue::Hit => ("hit", 0.25, 0.5),
    };
    Some((format!("block.{kind}.{event}"), volume, pitch))
}

/// Loads each cue's clips from the player's Minecraft install where it has them, and
/// synthesizes the rest.
///
/// Minecraft's clips are levelled to the loudness of our synthesized ones for the same
/// cue: Minecraft plays a step at 0.15 of a clip already quieter than its break, which
/// under music and ambience was lost altogether. Within a cue's clips their own volumes
/// and Minecraft's variation stay; across materials each cue sounds about as loud.
fn synthesize(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let started = std::time::Instant::now();
    let store = SoundStore::discover();
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
            let mut variants = Vec::new();
            if let (Some(store), Some((event, volume, pitch))) = (&store, minecraft_event(sound, cue)) {
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
            if variants.is_empty() {
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
    let thunder = [true, false].map(|near| {
        let clip = |v: u64| AudioSource { bytes: Arc::from(wav(&thunder_clip(mix(v * 7 + near as u64), near))) };
        (0..VARIANTS).map(|v| sources.add(clip(v))).collect()
    });
    commands.insert_resource(Sfx { clips, thunder, counter: 0 });
    let rain = sources.add(AudioSource { bytes: Arc::from(wav(&rain_loop(0x7261_696e))) });
    let playback = PlaybackSettings { paused: true, ..PlaybackSettings::LOOP.with_volume(Volume::SILENT) };
    commands.spawn((RainSound, AudioPlayer(rain), playback));
}

/// How loud a clip is: the RMS of its loudest stretch of [`LOUDNESS_WINDOW`], so short
/// clicks and long tails compare fairly. `samples` interleave `channels`.
fn loudness(samples: &[f32], channels: usize) -> f32 {
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

/// The loudness of an encoded clip, decoding it once.
fn measure(source: &AudioSource) -> Option<f32> {
    use bevy::audio::{Decodable, Source};
    let decoder = source.decoder();
    let channels = decoder.channels().get() as usize;
    let samples: Vec<f32> = decoder.collect();
    (!samples.is_empty()).then(|| loudness(&samples, channels))
}

#[derive(Component)]
struct RainSound;

/// Plays thunder as it reaches the player, quieter the farther the lightning.
fn thunder(mut commands: Commands, mut heard: MessageReader<Thunder>, sfx: Option<ResMut<Sfx>>, settings: Res<Settings>) {
    let Some(mut sfx) = sfx else { return };
    let master = MASTER * settings.sound_volume as f32 / 100.0;
    for thunder in heard.read() {
        let variant = (sfx.random() * VARIANTS as f32) as usize % VARIANTS as usize;
        let speed = 0.9 + sfx.random() * 0.2;
        let clip = sfx.thunder[(thunder.distance >= NEAR_LIGHTNING) as usize][variant].clone();
        let volume = Volume::Linear(THUNDER_GAIN / (1.0 + thunder.distance / 400.0) * master);
        commands.spawn((AudioPlayer(clip), PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(volume) }));
    }
}

/// Fades the rain loop with how hard it rains (snow is silent) and how open the sky is
/// above the player.
fn rain(
    time: Res<Time>,
    weather: Res<ClientWeather>,
    map: Res<SkyMap>,
    settings: Res<Settings>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut sink: Query<&mut AudioSink, With<RainSound>>,
    mut level: Local<f32>,
) {
    let (Ok(camera), Ok(mut sink)) = (camera.single(), sink.single_mut()) else { return };
    let shown = &weather.current;
    let rain = shown.precipitation * (1.0 - shown.snow);
    let target = if rain > 0.01 { rain.sqrt() * map.openness(camera.translation()) } else { 0.0 };
    *level += (target - *level) * (time.delta_secs() * 2.0).min(1.0);
    if *level < 0.002 {
        if !sink.is_paused() {
            sink.pause();
        }
        return;
    }
    sink.set_volume(Volume::Linear(*level * RAIN_GAIN * MASTER * settings.sound_volume as f32 / 100.0));
    if sink.is_paused() {
        sink.play();
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
        let gain = cue.gain * (1.0 - distance / HEARD_WITHIN).max(0.0);
        if gain <= 0.0 {
            continue;
        }
        let (pick, jitter) = (sfx.random(), sfx.random());
        let Some(clip) = sfx.clips.get(&(cue.sound, cue.cue)).and_then(|clips| clips.get((pick * clips.len() as f32) as usize))
        else {
            continue;
        };
        let speed = clip.speed * (0.94 + 0.12 * jitter);
        let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(clip.gain * gain * master)) };
        commands.spawn((AudioPlayer(clip.source.clone()), playback));
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
}

/// Footsteps every stride, from whatever is underfoot, in the manner of Minecraft and of
/// footstep mods such as Presence Footsteps: the ground's own step; the rustle of plants,
/// moss or petals the feet pass through, with the ground under them softer; a splash in
/// shallow water; strokes while swimming; and a thud when landing from a fall.
fn footsteps(player: Res<LocalPlayer>, world: Res<ClientWorld>, mut cues: MessageWriter<SoundCue>, mut state: Local<Strides>) {
    let state = &mut *state;
    let position = player.position;
    let previous = state.last.replace(position).unwrap_or(position);
    if !player.joined || player.flying {
        state.travelled = 0.0;
        state.peak = position.y;
        return;
    }
    let feet = position.floor().as_ivec3();
    let ground = world.block((position - Vec3::Y * 0.1).floor().as_ivec3()).filter(|b| !b.is_air());
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
        let stride = if player.velocity.with_y(0.0).length() > dims::WALK_SPEED + 0.5 { 2.1 } else { 1.7 };
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
    let recipe = recipe(sound);
    let (length_scale, pitch, grain_scale) = match cue {
        Cue::Step => (1.0, 1.0, 1.0),
        Cue::Land => (1.3, 0.8, 1.3),
        Cue::Place => (1.1, 0.9, 1.2),
        Cue::Break => (1.8, 0.82, 1.6),
        Cue::Hit => (0.6, 1.1, 0.7),
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
        if sound == Sound::Water {
            sample += water_bubbles(t, seed) * 0.6;
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

/// Rain on the ground, looping seamlessly: a dense patter of tiny drops (short decaying
/// tones, mostly faint, a few closer and louder) over a soft hiss and a low body.
fn rain_loop(seed: u64) -> Vec<f32> {
    let length = (RAIN_LOOP * RATE as f32) as usize;
    let overlap = RATE as usize / 2;
    let mut noise = Noise(seed);
    let mut samples = vec![0.0f32; length + overlap];
    let drop_length = (0.012 * RATE as f32) as usize;
    for _ in 0..((RAIN_LOOP + 0.5) * 1100.0) as usize {
        let start = (noise.unit() * samples.len() as f32) as usize;
        let step = TAU * (1800.0 + 4200.0 * noise.unit()) / RATE as f32;
        let loudness = 0.04 + 0.4 * noise.unit().powi(4);
        let decay = (-(250.0 + 500.0 * noise.unit()) / RATE as f32).exp();
        let mut amplitude = loudness;
        for (j, sample) in samples.iter_mut().skip(start).take(drop_length).enumerate() {
            *sample += amplitude * (step * j as f32).sin();
            amplitude *= decay;
        }
    }
    // Hiss and body: band-passed and low-passed noise from one state-variable filter each.
    let svf = |center: f32| 2.0 * (std::f32::consts::PI * center / RATE as f32).sin();
    let (hiss, body) = (svf(3200.0), svf(320.0));
    let (mut hiss_low, mut hiss_band, mut body_low, mut body_band) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for sample in &mut samples {
        let input = noise.next();
        hiss_low += hiss * hiss_band;
        hiss_band += hiss * (input - hiss_low - 1.4 * hiss_band);
        body_low += body * body_band;
        body_band += body * (input - body_low - 1.2 * body_band);
        *sample += 0.12 * hiss_band + 0.35 * body_low;
    }
    // Crossfade the extra tail into the start, keeping the power steady, so the loop has
    // no seam.
    for i in 0..overlap {
        let w = i as f32 / overlap as f32;
        samples[i] = samples[i] * w.sqrt() + samples[length + i] * (1.0 - w).sqrt();
    }
    samples.truncate(length);
    let mut warm = 0.0f32;
    for sample in &mut samples {
        warm += 0.5 * (*sample - warm);
        *sample = warm;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt().max(1e-6);
    let peak = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    let scale = (0.15 / rms).min(0.9 / peak);
    samples.iter_mut().for_each(|s| *s *= scale);
    samples
}

/// Thunder: a long, low rumble that swells as sound arrives from farther along the bolt, and
/// for lightning close by, a sharp crack first.
fn thunder_clip(seed: u64, near: bool) -> Vec<f32> {
    let length = (6.0 * RATE as f32) as usize;
    let mut noise = Noise(seed);
    // When, how strongly and for how long sound from each stretch of the bolt arrives.
    let swells: Vec<(f32, f32, f32)> =
        (0..5).map(|_| (0.1 + 2.4 * noise.unit(), 0.3 + 0.7 * noise.unit(), 0.25 + 0.6 * noise.unit())).collect();
    // Two cascaded low-passes for the rumble; distant thunder has lost more of its highs.
    let low = 2.0 * (std::f32::consts::PI * if near { 180.0 } else { 110.0 } / RATE as f32).sin();
    let crack = 2.0 * (std::f32::consts::PI * 1400.0 / RATE as f32).sin();
    let mut filters = [(0.0f32, 0.0f32); 3];
    let mut samples = Vec::with_capacity(length);
    for i in 0..length {
        let t = i as f32 / RATE as f32;
        let white = noise.next();
        let mut input = white;
        for (low_state, band) in &mut filters[..2] {
            *low_state += low * *band;
            *band += low * (input - *low_state - *band);
            input = *low_state;
        }
        let swell: f32 = swells.iter().map(|&(at, strength, width)| strength * (-((t - at) / width).powi(2)).exp()).sum();
        let envelope = (t / 0.03).min(1.0) * (swell + 0.3) * (-t / 2.5).exp();
        let mut sample = input * envelope;
        if near {
            let (low_state, band) = &mut filters[2];
            *low_state += crack * *band;
            *band += crack * (white - *low_state - 0.8 * *band);
            sample += *band * 1.5 * (-t * 14.0).exp();
        }
        samples.push(sample);
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt().max(1e-6);
    let peak = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    let scale = (0.2 / rms).min(0.95 / peak);
    let fade = RATE as usize / 2;
    for (i, sample) in samples.iter_mut().enumerate() {
        *sample *= scale * ((length - i) as f32 / fade as f32).min(1.0);
    }
    samples
}

/// A few rising bubble chirps for water.
fn water_bubbles(t: f32, seed: u64) -> f32 {
    (0..3)
        .map(|k| {
            let start = 0.02 + 0.07 * k as f32 + (mix(seed + k) >> 58) as f32 * 0.004;
            let local = t - start;
            if local < 0.0 {
                return 0.0;
            }
            let frequency = 380.0 * (1.0 + local * 9.0);
            (TAU * frequency * local).sin() * (-local * 45.0).exp()
        })
        .sum()
}

/// Encodes mono samples as a 16-bit PCM WAV file.
pub(crate) fn wav(samples: &[f32]) -> Vec<u8> {
    encode(1, samples.len(), samples.iter().copied())
}

/// Encodes two channels of equal length as a 16-bit PCM WAV file.
pub(crate) fn wav_stereo(left: &[f32], right: &[f32]) -> Vec<u8> {
    debug_assert_eq!(left.len(), right.len());
    encode(2, left.len(), left.iter().zip(right).flat_map(|(&l, &r)| [l, r]))
}

/// A WAV file of `frames` frames of `channels` interleaved samples.
fn encode(channels: u16, frames: usize, samples: impl Iterator<Item = f32>) -> Vec<u8> {
    let block = channels as u32 * 2;
    let data_len = frames as u32 * block;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * block).to_le_bytes());
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

    #[test]
    fn thunder_renders_clean_audio() {
        for near in [true, false] {
            let samples = thunder_clip(5, near);
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            assert!(samples.last().unwrap().abs() < 0.01);
        }
    }

    #[test]
    fn rain_loops_without_a_seam() {
        let samples = rain_loop(3);
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        let steepest = samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!((samples[0] - samples[samples.len() - 1]).abs() <= steepest);
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
        for _ in 0..2 {
            all.extend(rain_loop(1).iter().map(|s| s * RAIN_GAIN));
        }
        for near in [true, false] {
            all.extend(thunder_clip(2, near).iter().map(|s| s * THUNDER_GAIN));
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sfx_preview.wav");
        std::fs::write(path, wav(&all)).unwrap();
    }
}
