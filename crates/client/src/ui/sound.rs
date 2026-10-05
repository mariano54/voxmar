//! The interface's sounds: a click when a button is pressed and a tap when an item is
//! picked up or put down in the inventory, in one of a few sets the player chooses
//! (`UiSounds` in the settings). Every set is synthesized at startup, like the block
//! sounds, and follows the sound effects volume.
//!
//! Each sound is a struck object: a few decaying sine modes (a wood block's inharmonic
//! ones, a glass's bright pair, a slate's low ones) over a short low-passed noise strike.

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::Volume;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::widgets::Disabled;
use crate::settings::Settings;
use crate::sfx::{Audible, MASTER, RATE, wav};

pub struct UiSoundPlugin;

impl Plugin for UiSoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiSound>().add_systems(Startup, synthesize).add_systems(Update, (click_buttons, play.in_set(Audible)).chain());
    }
}

/// A sound the interface makes.
#[derive(Message, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiSound {
    /// A button pressed.
    Click,
    /// An item picked up or put down.
    Item,
}

/// Buttons that make no click of their own, such as inventory slots, which tap instead.
#[derive(Component)]
pub struct Silent;

/// The sets of interface sounds a player can choose from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiSounds {
    /// A small wooden knock.
    #[default]
    Wood,
    /// Glass ringing softly, a fifth apart.
    Chime,
    /// Almost all strike, like a pen tapped on paper; the quietest.
    Tick,
    /// A low tap on slate, with weight.
    Stone,
    /// A short rising shimmer, the teal glow of the interface as sound.
    Arcane,
    Off,
}

impl UiSounds {
    pub const ALL: [UiSounds; 6] = [UiSounds::Wood, UiSounds::Chime, UiSounds::Tick, UiSounds::Stone, UiSounds::Arcane, UiSounds::Off];

    pub fn label(self) -> &'static str {
        match self {
            UiSounds::Wood => "Wood",
            UiSounds::Chime => "Chime",
            UiSounds::Tick => "Tick",
            UiSounds::Stone => "Stone",
            UiSounds::Arcane => "Arcane",
            UiSounds::Off => "Off",
        }
    }

    /// The click and the item tap, each as decaying modes (frequency in Hz, level, decay in
    /// seconds), with its length, its strike (level, decay, tone: the low-pass's
    /// coefficient, lower is darker) and pitch sweep. `None` for silence.
    fn recipe(self) -> Option<[Strike; 2]> {
        Some(match self {
            UiSounds::Wood => [
                Strike { modes: &[(940.0, 1.0, 0.030), (2420.0, 0.45, 0.014), (3950.0, 0.2, 0.008)], seconds: 0.12, noise: (0.8, 0.004, 0.35), sweep: 0.0 },
                Strike { modes: &[(520.0, 1.0, 0.040), (1340.0, 0.35, 0.018)], seconds: 0.14, noise: (0.7, 0.005, 0.25), sweep: 0.0 },
            ],
            UiSounds::Chime => [
                Strike { modes: &[(2093.0, 0.6, 0.11), (3136.0, 0.35, 0.07), (6270.0, 0.08, 0.03)], seconds: 0.35, noise: (0.15, 0.002, 0.9), sweep: 0.0 },
                Strike { modes: &[(1568.0, 0.6, 0.09), (2349.0, 0.3, 0.06)], seconds: 0.3, noise: (0.1, 0.002, 0.9), sweep: 0.0 },
            ],
            UiSounds::Tick => [
                Strike { modes: &[(1800.0, 0.25, 0.006)], seconds: 0.05, noise: (1.0, 0.0025, 0.6), sweep: 0.0 },
                Strike { modes: &[(700.0, 0.3, 0.012)], seconds: 0.07, noise: (1.0, 0.004, 0.3), sweep: 0.0 },
            ],
            UiSounds::Stone => [
                Strike { modes: &[(310.0, 1.0, 0.035), (870.0, 0.3, 0.015), (1630.0, 0.12, 0.008)], seconds: 0.12, noise: (1.0, 0.006, 0.15), sweep: 0.0 },
                Strike { modes: &[(220.0, 1.0, 0.045), (610.0, 0.25, 0.02)], seconds: 0.14, noise: (0.9, 0.007, 0.12), sweep: 0.0 },
            ],
            UiSounds::Arcane => [
                Strike { modes: &[(880.0, 0.7, 0.05), (1320.0, 0.45, 0.04), (2640.0, 0.15, 0.03)], seconds: 0.18, noise: (0.1, 0.002, 0.9), sweep: 0.35 },
                Strike { modes: &[(660.0, 0.7, 0.05), (990.0, 0.35, 0.035)], seconds: 0.16, noise: (0.1, 0.002, 0.9), sweep: -0.15 },
            ],
            UiSounds::Off => return None,
        })
    }
}

/// A struck object, as `UiSounds::recipe` describes.
struct Strike {
    modes: &'static [(f32, f32, f32)],
    seconds: f32,
    noise: (f32, f32, f32),
    /// How far every mode's pitch bends (as a ratio) over the first 40 ms.
    sweep: f32,
}

impl Strike {
    /// The samples, peaking at 0.8.
    fn render(&self, seed: u32) -> Vec<f32> {
        let envelope = |t: f32, attack: f32, decay: f32| (t / attack).min(1.0) * (-t / decay).exp();
        let (noise, noise_decay, tone) = self.noise;
        let mut rng = seed;
        let mut low = 0.0;
        let mut phases = vec![0.0f32; self.modes.len()];
        let frames = (self.seconds * RATE as f32) as usize;
        let mut out: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let bend = 1.0 + self.sweep * (t / 0.04).min(1.0);
                let mut v = 0.0;
                for (phase, &(f, level, decay)) in phases.iter_mut().zip(self.modes) {
                    *phase += TAU * f * bend / RATE as f32;
                    v += phase.sin() * level * envelope(t, 0.0015, decay);
                }
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                low += tone * (rng as f32 / u32::MAX as f32 * 2.0 - 1.0 - low);
                v + low * noise * envelope(t, 0.0005, noise_decay)
            })
            .collect();
        // A 3 ms fade at the end, so nothing clicks.
        let fade = (0.003 * RATE as f32) as usize;
        let n = out.len();
        for i in 0..fade.min(n) {
            out[n - 1 - i] *= i as f32 / fade as f32;
        }
        let peak = out.iter().fold(1e-9f32, |m, s| m.max(s.abs()));
        out.iter().map(|s| s / peak * 0.8).collect()
    }
}

/// Every set's click and tap, synthesized once.
#[derive(Resource)]
struct Clips(Vec<(UiSounds, [Handle<AudioSource>; 2])>);

fn synthesize(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let clips = UiSounds::ALL
        .iter()
        .filter_map(|&set| {
            let [click, item] = set.recipe()?;
            let mut add = |strike: &Strike, seed| sources.add(AudioSource { bytes: Arc::from(wav(&strike.render(seed))) });
            Some((set, [add(&click, 3), add(&item, 5)]))
        })
        .collect();
    commands.insert_resource(Clips(clips));
}

#[allow(clippy::type_complexity)]
fn click_buttons(
    buttons: Query<&Interaction, (Changed<Interaction>, With<Button>, Without<Disabled>, Without<Silent>)>,
    mut sounds: MessageWriter<UiSound>,
) {
    if buttons.iter().any(|interaction| *interaction == Interaction::Pressed) {
        sounds.write(UiSound::Click);
    }
}

fn play(mut commands: Commands, mut sounds: MessageReader<UiSound>, clips: Option<Res<Clips>>, settings: Res<Settings>) {
    let Some(clips) = clips else { return };
    let volume = MASTER * settings.sound_volume as f32 / 100.0;
    let set = clips.0.iter().find(|(set, _)| *set == settings.ui_sounds).map(|(_, clips)| clips);
    let mut played = Vec::new();
    for &sound in sounds.read() {
        // One of each a frame, however many buttons or moves asked.
        let Some(set) = set.filter(|_| !played.contains(&sound) && volume > 0.0) else { continue };
        played.push(sound);
        let (clip, gain) = match sound {
            UiSound::Click => (&set[0], 0.4),
            UiSound::Item => (&set[1], 0.32),
        };
        commands.spawn((AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_set_but_off_sounds_and_ends_quietly() {
        for set in UiSounds::ALL {
            let Some(strikes) = set.recipe() else {
                assert_eq!(set, UiSounds::Off);
                continue;
            };
            for strike in strikes {
                let samples = strike.render(1);
                assert!(samples.iter().any(|s| s.abs() > 0.7), "{set:?} is silent");
                assert!(samples.last().unwrap().abs() < 1e-3, "{set:?} ends with a click");
            }
        }
    }
}
