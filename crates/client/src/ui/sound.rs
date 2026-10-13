//! The interface's sounds: a click when a button is pressed and a tap when an item is
//! picked up or put down in the inventory, in one of a few sets the player chooses
//! (`UiSounds` in the settings). Every set is synthesized at startup, like the block
//! sounds, and follows the sound effects volume.
//!
//! Each sound is a few decaying sine partials, which may glide in pitch, be frequency
//! modulated or blip twice, and may carry a breath of high noise.
//!
//! A quest finished chimes, and its rewards claimed ring softer. These do not follow the
//! set chosen: they are events, not clicks. Synthesized unless a package's `sounds.ron`
//! lists recordings for them (`quest_done`, `quest_claim`), which replace the synthesis.

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::Volume;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::widgets::Disabled;
use crate::settings::Settings;
use crate::session::Launch;
use crate::sfx::{Audible, MASTER, RATE, Recordings, wav};

pub struct UiSoundPlugin;

impl Plugin for UiSoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiSound>()
            .add_systems(Startup, (synthesize, quest_clips))
            .add_systems(Update, (click_buttons, play.in_set(Audible)).chain())
            .add_systems(Update, play_quest.in_set(Audible));
    }
}

/// A sound the interface makes.
#[derive(Message, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiSound {
    /// A button pressed.
    Click,
    /// An item picked up or put down.
    Item,
    /// A quest finished: a rising two-note chime.
    QuestDone,
    /// A finished quest's rewards claimed: a softer, brighter ring of coins.
    QuestClaim,
}

/// Buttons that make no click of their own, such as inventory slots, which tap instead.
#[derive(Component)]
pub struct Silent;

/// The sets of interface sounds a player can choose from: clean, short and a little
/// futuristic. Micro switch is recorded (Kenney's UI Audio, CC0, in `assets/ui/sounds`);
/// the rest are synthesized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiSounds {
    /// Very short, bright switches, almost a tick. Mariano's pick.
    #[default]
    MicroSwitch,
    /// A short low thump under a tiny bright tick, like a phone's haptic click.
    Haptic,
    /// A high, pure tick with its octave, gone almost at once.
    Glass,
    /// A minimal digital blip; items blip twice, rising.
    Blip,
    /// A soft synth click that drops in pitch.
    Pulse,
    /// A sci-fi chirp: frequency modulation settling into a clean tone.
    Signal,
    /// Almost only air: a short breath of high noise. The quietest.
    Tap,
    Off,
}

impl UiSounds {
    pub const ALL: [UiSounds; 8] = [
        UiSounds::MicroSwitch,
        UiSounds::Blip,
        UiSounds::Haptic,
        UiSounds::Glass,
        UiSounds::Pulse,
        UiSounds::Signal,
        UiSounds::Tap,
        UiSounds::Off,
    ];

    /// The key of its name in the settings (see `lang`).
    pub fn key(self) -> &'static str {
        match self {
            UiSounds::MicroSwitch => "settings.ui_sounds.micro_switch",
            UiSounds::Haptic => "settings.ui_sounds.haptic",
            UiSounds::Glass => "settings.ui_sounds.glass",
            UiSounds::Blip => "settings.ui_sounds.blip",
            UiSounds::Pulse => "settings.ui_sounds.pulse",
            UiSounds::Signal => "settings.ui_sounds.signal",
            UiSounds::Tap => "settings.ui_sounds.tap",
            UiSounds::Off => "settings.value.off",
        }
    }

    /// The recorded click and item sound, in `assets/ui/sounds`.
    fn files(self) -> Option<[&'static str; 2]> {
        match self {
            UiSounds::MicroSwitch => Some(["micro_switch_click.ogg", "micro_switch_item.ogg"]),
            _ => None,
        }
    }

    /// The synthesized click and item tap, or `None` for silence or a recorded set.
    fn recipe(self) -> Option<[Sound; 2]> {
        Some(match self {
            UiSounds::Haptic => [
                Sound { partials: &[(170.0, 150.0, 1.0, 0.012), (3800.0, 3800.0, 0.18, 0.002)], attack: 0.0008, glide: 0.02, ..PLAIN },
                Sound { partials: &[(130.0, 115.0, 1.0, 0.016), (2900.0, 2900.0, 0.14, 0.002)], seconds: 0.06, attack: 0.0008, glide: 0.02, ..PLAIN },
            ],
            UiSounds::Glass => [
                Sound { partials: &[(3150.0, 3150.0, 0.7, 0.012), (6300.0, 6300.0, 0.25, 0.006)], seconds: 0.06, attack: 0.0006, ..PLAIN },
                Sound { partials: &[(2360.0, 2360.0, 0.7, 0.014), (4720.0, 4720.0, 0.22, 0.007)], seconds: 0.07, attack: 0.0006, ..PLAIN },
            ],
            UiSounds::Blip => [
                Sound { partials: &[(1320.0, 1320.0, 0.8, 0.02), (3960.0, 3960.0, 0.18, 0.012)], seconds: 0.06, ..PLAIN },
                Sound { partials: &[(880.0, 880.0, 0.8, 0.018), (2640.0, 2640.0, 0.16, 0.01)], seconds: 0.11, echo: Some((0.045, 1.5)), ..PLAIN },
            ],
            UiSounds::Pulse => [
                Sound { partials: &[(2200.0, 700.0, 1.0, 0.022)], seconds: 0.07, attack: 0.0008, glide: 0.018, ..PLAIN },
                Sound { partials: &[(1500.0, 450.0, 1.0, 0.028)], seconds: 0.08, attack: 0.0008, glide: 0.022, ..PLAIN },
            ],
            UiSounds::Signal => [
                Sound { partials: &[(1500.0, 1800.0, 0.8, 0.035)], seconds: 0.1, glide: 0.03, fm: Some((0.5, 2.5, 0.015)), ..PLAIN },
                Sound { partials: &[(1100.0, 900.0, 0.8, 0.04)], seconds: 0.1, glide: 0.03, fm: Some((0.5, 2.0, 0.015)), ..PLAIN },
            ],
            UiSounds::Tap => [
                Sound { partials: &[(2600.0, 2600.0, 0.12, 0.004)], seconds: 0.03, noise: Some((1.0, 0.0028, 0.5)), ..PLAIN },
                Sound { partials: &[(1700.0, 1700.0, 0.12, 0.005)], seconds: 0.04, noise: Some((1.0, 0.0038, 0.35)), ..PLAIN },
            ],
            UiSounds::Off | UiSounds::MicroSwitch => return None,
        })
    }
}

const PLAIN: Sound = Sound { partials: &[], seconds: 0.05, attack: 0.001, glide: 0.0, fm: None, noise: None, echo: None };

/// The quest chime: a bell (its partials not whole multiples, as a struck bell's are) and the
/// same bell a fifth above a moment later.
const QUEST_DONE: Sound = Sound {
    partials: &[(784.0, 784.0, 1.0, 0.45), (1568.0, 1568.0, 0.35, 0.26), (2163.0, 2163.0, 0.16, 0.15), (3190.0, 3190.0, 0.07, 0.08)],
    seconds: 1.1,
    attack: 0.003,
    echo: Some((0.14, 1.5)),
    ..PLAIN
};

/// Rewards claimed: two quick, high, small rings.
const QUEST_CLAIM: Sound = Sound {
    partials: &[(1568.0, 1568.0, 1.0, 0.1), (2349.0, 2349.0, 0.4, 0.07), (4698.0, 4698.0, 0.1, 0.03)],
    seconds: 0.45,
    attack: 0.002,
    echo: Some((0.07, 1.335)),
    ..PLAIN
};

/// A short synthesized sound, as `UiSounds::recipe` describes.
struct Sound {
    /// Sine partials: starting and ending frequency (Hz), level and decay time (s).
    partials: &'static [(f32, f32, f32, f32)],
    seconds: f32,
    attack: f32,
    /// Seconds over which each partial glides, exponentially, to its ending frequency.
    glide: f32,
    /// Frequency modulation of every partial: the modulator's frequency as a ratio of the
    /// partial's, the modulation index and how fast it dies away (s).
    fm: Option<(f32, f32, f32)>,
    /// A strike of high-passed noise: level, decay (s) and the low-pass coefficient taken
    /// away from it (higher leaves a thinner, brighter noise).
    noise: Option<(f32, f32, f32)>,
    /// The whole sound again, this many seconds later at this pitch.
    echo: Option<(f32, f32)>,
}

impl Sound {
    /// The samples, peaking at 0.8.
    fn render(&self, seed: u32) -> Vec<f32> {
        let frames = (self.seconds * RATE as f32) as usize;
        let mut out = vec![0.0f32; frames];
        self.add(&mut out, 0, 1.0, seed);
        if let Some((delay, pitch)) = self.echo {
            self.add(&mut out, (delay * RATE as f32) as usize, pitch, seed);
        }
        // A 3 ms fade at the end, so nothing clicks.
        let fade = (0.003 * RATE as f32) as usize;
        let n = out.len();
        for i in 0..fade.min(n) {
            out[n - 1 - i] *= i as f32 / fade as f32;
        }
        let peak = out.iter().fold(1e-9f32, |m, s| m.max(s.abs()));
        out.iter().map(|s| s / peak * 0.8).collect()
    }

    fn add(&self, out: &mut [f32], offset: usize, pitch: f32, seed: u32) {
        let mut rng = seed;
        let mut low = 0.0;
        let mut phases = vec![0.0f32; self.partials.len()];
        let mut modulators = vec![0.0f32; self.partials.len()];
        for (i, sample) in out.iter_mut().skip(offset).enumerate() {
            let t = i as f32 / RATE as f32;
            let k = if self.glide > 0.0 { (t / self.glide).min(1.0) } else { 1.0 };
            let mut v = 0.0;
            for (j, &(from, to, level, decay)) in self.partials.iter().enumerate() {
                let f = from * (to / from).powf(k) * pitch;
                let modulation = self.fm.map_or(0.0, |(ratio, index, index_decay)| {
                    modulators[j] += TAU * f * ratio / RATE as f32;
                    index * (-t / index_decay).exp() * modulators[j].sin()
                });
                phases[j] += TAU * f / RATE as f32;
                v += (phases[j] + modulation).sin() * level * (t / self.attack).min(1.0) * (-t / decay).exp();
            }
            if let Some((level, decay, tone)) = self.noise {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let white = rng as f32 / u32::MAX as f32 * 2.0 - 1.0;
                low += tone * (white - low);
                v += (white - low) * level * (t / 0.0003).min(1.0) * (-t / decay).exp();
            }
            *sample += v;
        }
    }
}

/// Every set's click and tap, synthesized once.
#[derive(Resource)]
struct Clips(Vec<(UiSounds, [Handle<AudioSource>; 2])>);

fn synthesize(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let clips = UiSounds::ALL
        .iter()
        .filter_map(|&set| {
            if let Some(files) = set.files() {
                let dir = voxmar_server::assets_dir().join("ui/sounds");
                let read = |name: &str| std::fs::read(dir.join(name)).map_err(|e| warn!("cannot read {}: {e}", dir.join(name).display())).ok();
                let [click, item] = files.map(read);
                return Some((set, [sources.add(AudioSource { bytes: Arc::from(click?) }), sources.add(AudioSource { bytes: Arc::from(item?) })]));
            }
            let [click, item] = set.recipe()?;
            let mut add = |sound: &Sound, seed| sources.add(AudioSource { bytes: Arc::from(wav(&sound.render(seed))) });
            Some((set, [add(&click, 7), add(&item, 11)]))
        })
        .collect();
    commands.insert_resource(Clips(clips));
}

#[allow(clippy::type_complexity)]
/// The quest sounds: a package's recordings where it lists any, else the synthesized chimes.
#[derive(Resource)]
struct QuestClips {
    done: Vec<Handle<AudioSource>>,
    claim: Vec<Handle<AudioSource>>,
}

fn quest_clips(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>, assets: Res<AssetServer>, launch: Res<Launch>) {
    let recorded = Recordings::load(&launch.content.files).unwrap_or_else(|e| {
        error!("no recorded sounds: {e}");
        Recordings::default()
    });
    let mut pick = |paths: &[String], synthesized: &Sound, seed| -> Vec<Handle<AudioSource>> {
        if paths.is_empty() {
            vec![sources.add(AudioSource { bytes: Arc::from(wav(&synthesized.render(seed))) })]
        } else {
            paths.iter().map(|path| assets.load::<AudioSource>(path.clone())).collect()
        }
    };
    commands.insert_resource(QuestClips { done: pick(&recorded.quest_done, &QUEST_DONE, 3), claim: pick(&recorded.quest_claim, &QUEST_CLAIM, 5) });
}

/// Plays the quest chimes, taking turns among a package's recordings if it lists several.
fn play_quest(mut commands: Commands, mut sounds: MessageReader<UiSound>, clips: Option<Res<QuestClips>>, settings: Res<Settings>, mut turn: Local<usize>) {
    let volume = MASTER * settings.sound_volume as f32 / 100.0;
    for &sound in sounds.read() {
        let (Some(clips), true) = (&clips, volume > 0.0) else { continue };
        let (list, gain) = match sound {
            UiSound::QuestDone => (&clips.done, 0.5),
            UiSound::QuestClaim => (&clips.claim, 0.3),
            UiSound::Click | UiSound::Item => continue,
        };
        *turn += 1;
        if let Some(clip) = list.get(*turn % list.len().max(1)) {
            commands.spawn((AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume))));
        }
    }
}

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
            UiSound::QuestDone | UiSound::QuestClaim => continue,
        };
        commands.spawn((AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_sets_are_shipped() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/sounds");
        for set in UiSounds::ALL {
            for file in set.files().into_iter().flatten() {
                assert!(dir.join(file).is_file(), "{set:?}: no {file}");
            }
        }
    }

    /// The quest chimes sound, fade out, and the claim is the shorter and softer of the two.
    #[test]
    fn the_quest_chimes_sound_and_end_quietly() {
        let (done, claim) = (QUEST_DONE.render(3), QUEST_CLAIM.render(5));
        for samples in [&done, &claim] {
            assert!(samples.iter().any(|s| s.abs() > 0.7), "silent");
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 0.81));
            assert!(samples.last().unwrap().abs() < 1e-3, "ends with a click");
        }
        assert!(claim.len() < done.len());
    }

    #[test]
    fn every_set_but_off_sounds_and_ends_quietly() {
        for set in UiSounds::ALL {
            let Some(strikes) = set.recipe() else {
                assert!(set == UiSounds::Off || set.files().is_some(), "{set:?} has no sound");
                continue;
            };
            for sound in strikes {
                let samples = sound.render(1);
                assert!(samples.iter().any(|s| s.abs() > 0.7), "{set:?} is silent");
                assert!(samples.last().unwrap().abs() < 1e-3, "{set:?} ends with a click");
            }
        }
    }
}
