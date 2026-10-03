//! Sound effects, synthesized at startup from block materials.
//!
//! Sound philosophy:
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

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, Sound};
use voxmar_engine::physics::player as dims;

use crate::player::LocalPlayer;
use crate::world::ClientWorld;

const RATE: u32 = 44_100;
const VARIANTS: u64 = 4;
const MASTER: f32 = 0.6;

pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SoundCue>()
            .add_systems(Startup, synthesize)
            .add_systems(Update, (footsteps, play_cues).chain());
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
            Cue::Step => 0.35,
            Cue::Land => 0.6,
            Cue::Place => 0.7,
            Cue::Break => 0.8,
            Cue::Hit => 0.3,
        }
    }
}

/// Request to play a cue for a material.
#[derive(Message, Clone, Copy, Debug)]
pub struct SoundCue {
    pub sound: Sound,
    pub cue: Cue,
}

impl SoundCue {
    pub fn for_block(world: &ClientWorld, block: BlockId, cue: Cue) -> Self {
        Self { sound: world.registry.info(block).sound, cue }
    }
}

#[derive(Resource)]
struct Sfx {
    clips: FxHashMap<(Sound, Cue), Vec<Handle<AudioSource>>>,
    counter: u64,
}

impl Sfx {
    fn random(&mut self) -> f32 {
        self.counter += 1;
        (mix(self.counter) >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn synthesize(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let mut clips = FxHashMap::default();
    for sound in Sound::ALL {
        for cue in Cue::ALL {
            let variants = (0..VARIANTS)
                .map(|v| {
                    let samples = render(sound, cue, mix(sound as u64 * 131 + cue as u64 * 17 + v));
                    sources.add(AudioSource { bytes: Arc::from(wav(&samples)) })
                })
                .collect();
            clips.insert((sound, cue), variants);
        }
    }
    commands.insert_resource(Sfx { clips, counter: 0 });
}

fn play_cues(mut commands: Commands, mut cues: MessageReader<SoundCue>, sfx: Option<ResMut<Sfx>>) {
    let Some(mut sfx) = sfx else { return };
    for cue in cues.read() {
        let variant = (sfx.random() * VARIANTS as f32) as usize % VARIANTS as usize;
        let speed = 0.92 + sfx.random() * 0.16;
        let Some(clip) = sfx.clips.get(&(cue.sound, cue.cue)).map(|v| v[variant].clone()) else { continue };
        let settings = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(cue.cue.gain() * MASTER)) };
        commands.spawn((AudioPlayer(clip), settings));
    }
}

/// Footsteps every stride on the ground, and a thud when landing from a fall.
fn footsteps(
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    mut cues: MessageWriter<SoundCue>,
    mut state: Local<(Option<Vec3>, f32, bool, f32)>,
) {
    let (last, travelled, was_grounded, peak) = &mut *state;
    let position = player.position;
    let previous = last.replace(position).unwrap_or(position);
    if !player.joined || player.flying {
        *travelled = 0.0;
        *peak = position.y;
        return;
    }
    let below = world.block((position - Vec3::Y * 0.1).floor().as_ivec3());
    if player.on_ground {
        if !*was_grounded && *peak - position.y > 1.2 && let Some(block) = below {
            cues.write(SoundCue::for_block(&world, block, Cue::Land));
        }
        *travelled += (position - previous).with_y(0.0).length();
        let stride = if player.velocity.with_y(0.0).length() > dims::WALK_SPEED + 0.5 { 2.1 } else { 1.7 };
        if *travelled >= stride {
            *travelled = 0.0;
            if let Some(block) = below.filter(|b| !b.is_air()) {
                cues.write(SoundCue::for_block(&world, block, Cue::Step));
            }
        }
        *peak = position.y;
    } else {
        *peak = peak.max(position.y);
        if player.in_water {
            *travelled += (position - previous).length();
            if *travelled >= 2.4 {
                *travelled = 0.0;
                cues.write(SoundCue { sound: Sound::Water, cue: Cue::Step });
            }
        }
    }
    *was_grounded = player.on_ground;
}

/// SplitMix64 finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = mix(self.0);
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }

    fn unit(&mut self) -> f32 {
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
        Sound::Stone => Recipe { length: 0.09, center: 2100.0, q: 1.3, tones: &[(150.0, 30.0)], tone_gain: 0.35, ..base },
        Sound::Wood => Recipe { length: 0.15, center: 900.0, q: 2.2, tones: &[(240.0, 16.0), (590.0, 24.0)], tone_gain: 0.6, ..base },
        Sound::Soil => Recipe { length: 0.11, center: 650.0, q: 0.8, grains: 320.0, grain_length: 0.006, tones: &[(95.0, 30.0)], tone_gain: 0.3, ..base },
        Sound::Grass => Recipe { length: 0.13, center: 3000.0, q: 0.9, grains: 520.0, grain_length: 0.004, attack: 0.008, ..base },
        Sound::Sand => Recipe { length: 0.16, center: 2400.0, q: 0.6, attack: 0.02, ..base },
        Sound::Gravel => Recipe { length: 0.14, center: 1500.0, q: 1.4, grains: 260.0, grain_length: 0.008, ..base },
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

    // Normalise, then fade the last few milliseconds so clips never click.
    let peak = samples.iter().fold(1e-6f32, |p, s| p.max(s.abs()));
    let fade = (0.005 * RATE as f32) as usize;
    let len = samples.len();
    for (i, sample) in samples.iter_mut().enumerate() {
        let tail = ((len - i) as f32 / fade as f32).min(1.0);
        *sample = *sample / peak * 0.8 * tail;
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
fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
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
