//! The interface's sounds: a click when a button is pressed and a tap when an item is
//! picked up or put down in the inventory, in one of a few sets the player chooses
//! (`UiSounds` in the settings). Every set is synthesized at startup, like the block
//! sounds, and follows the sound effects volume.
//!
//! Each sound is a few decaying sine partials, which may glide in pitch, be frequency
//! modulated or blip twice, and may carry a breath of high noise.
//!
//! Events play jingles instead, whatever the set chosen: a quest finished, its rewards
//! claimed (coins), a skill's level reached, and a grander one every tenth level. They are
//! recordings each package's `sounds.ron` lists (the base package's are our own, from
//! `tools/generate_jingles.py`). Two tunes never start over each other: one arriving while
//! another opens waits its turn, so a quest finished by reaching a level chimes, then rings.

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
            .add_systems(Startup, (synthesize, jingle_clips))
            .add_systems(Update, (click_buttons, play.in_set(Audible)).chain())
            .add_systems(Update, play_jingles.in_set(Audible));
    }
}

/// A sound the interface makes.
#[derive(Message, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiSound {
    /// A button pressed.
    Click,
    /// An item picked up or put down.
    Item,
    /// A quest finished.
    QuestDone,
    /// A finished quest's rewards claimed: coins.
    QuestClaim,
    /// A skill reached this level.
    LevelUp(u8),
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

/// The jingles, as the packages list them.
#[derive(Resource)]
struct Jingles {
    done: Vec<Handle<AudioSource>>,
    claim: Vec<Handle<AudioSource>>,
    level: Vec<Handle<AudioSource>>,
    milestone: Vec<Handle<AudioSource>>,
    /// Levels this many apart (a multiple of it) play the milestone instead; 0 for never.
    every: u8,
}

impl Jingles {
    /// The clips `sound` takes turns among and how loud it plays, or `None` for no jingle.
    fn of(&self, sound: UiSound) -> Option<(&[Handle<AudioSource>], f32)> {
        let (list, gain) = match sound {
            UiSound::QuestDone => (&self.done, 0.5),
            UiSound::QuestClaim => (&self.claim, 0.3),
            UiSound::LevelUp(level) if self.milestone_at(level) => (&self.milestone, 0.5),
            UiSound::LevelUp(_) => (&self.level, 0.5),
            UiSound::Click | UiSound::Item => return None,
        };
        (!list.is_empty()).then_some((list.as_slice(), gain))
    }

    fn milestone_at(&self, level: u8) -> bool {
        self.every > 0 && level.is_multiple_of(self.every) && !self.milestone.is_empty()
    }

    /// Seconds `sound` holds the stage before another tune may start: its opening, not its
    /// ringing tail. Coins never hold it.
    fn holds(&self, sound: UiSound) -> f32 {
        match sound {
            UiSound::LevelUp(level) if self.milestone_at(level) => 1.9,
            UiSound::QuestDone | UiSound::LevelUp(_) => 1.2,
            _ => 0.0,
        }
    }
}

fn jingle_clips(mut commands: Commands, assets: Res<AssetServer>, launch: Res<Launch>) {
    let recorded = Recordings::load(&launch.content.files).unwrap_or_else(|e| {
        error!("no jingles: {e}");
        Recordings::default()
    });
    let load = |paths: &[String]| paths.iter().map(|path| assets.load::<AudioSource>(path.clone())).collect();
    commands.insert_resource(Jingles {
        done: load(&recorded.quest_done),
        claim: load(&recorded.quest_claim),
        level: load(&recorded.level_up),
        milestone: load(&recorded.level_milestone.sounds),
        every: recorded.level_milestone.every,
    });
}

/// Tunes waiting for the stage, and when it is free (real seconds).
#[derive(Default)]
struct Stage {
    free_at: f32,
    waiting: Vec<UiSound>,
}

impl Stage {
    /// `sound` asked for. Coins play at once (`true`); a tune waits its turn, once: two
    /// quests finished together chime once, and levels reached together ring once, as the
    /// grandest of them.
    fn ask(&mut self, sound: UiSound, jingles: &Jingles) -> bool {
        match sound {
            UiSound::QuestClaim => return true,
            UiSound::LevelUp(level) => match self.waiting.iter_mut().find(|w| matches!(w, UiSound::LevelUp(_))) {
                Some(UiSound::LevelUp(was)) if jingles.milestone_at(*was) || (!jingles.milestone_at(level) && *was >= level) => {}
                Some(waiting) => *waiting = sound,
                None => self.waiting.push(sound),
            },
            UiSound::QuestDone if !self.waiting.contains(&sound) => self.waiting.push(sound),
            _ => {}
        }
        false
    }

    /// The tune to start `now`, if the stage is free and one waits.
    fn next(&mut self, now: f32, jingles: &Jingles) -> Option<UiSound> {
        if now < self.free_at || self.waiting.is_empty() {
            return None;
        }
        let sound = self.waiting.remove(0);
        self.free_at = now + jingles.holds(sound);
        Some(sound)
    }
}

/// Plays the jingles, taking turns among a package's clips where it lists several.
fn play_jingles(
    mut commands: Commands,
    mut sounds: MessageReader<UiSound>,
    jingles: Option<Res<Jingles>>,
    settings: Res<Settings>,
    time: Res<Time<Real>>,
    mut stage: Local<Stage>,
    mut turn: Local<usize>,
) {
    let Some(jingles) = jingles else { return };
    let volume = MASTER * settings.sound_volume as f32 / 100.0;
    let mut start = Vec::new();
    for &sound in sounds.read() {
        if stage.ask(sound, &jingles) {
            start.push(sound);
        }
    }
    start.extend(stage.next(time.elapsed_secs(), &jingles));
    for sound in start {
        let Some((list, gain)) = jingles.of(sound).filter(|_| volume > 0.0) else { continue };
        *turn += 1;
        commands.spawn((AudioPlayer(list[*turn % list.len()].clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume))));
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
            UiSound::QuestDone | UiSound::QuestClaim | UiSound::LevelUp(_) => continue,
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

    fn jingles(every: u8) -> Jingles {
        let clip = || vec![Handle::<AudioSource>::default()];
        Jingles { done: clip(), claim: clip(), level: clip(), milestone: clip(), every }
    }

    /// Every tenth level rings the milestone, the rest the level-up, and none without a
    /// spacing.
    #[test]
    fn every_tenth_level_is_a_milestone() {
        let tenth = jingles(10);
        assert!(tenth.milestone_at(10) && tenth.milestone_at(40));
        assert!(!tenth.milestone_at(9) && !tenth.milestone_at(11));
        assert!(!jingles(0).milestone_at(10));
        assert_eq!(tenth.holds(UiSound::LevelUp(20)), 1.9);
        assert_eq!(tenth.holds(UiSound::LevelUp(21)), 1.2);
    }

    /// A quest finished by reaching a level chimes, then the level rings once the chime has
    /// opened: never both at once. Coins play straight away.
    #[test]
    fn tunes_take_turns_and_coins_do_not_wait() {
        let jingles = jingles(10);
        let mut stage = Stage::default();
        assert!(!stage.ask(UiSound::QuestDone, &jingles));
        assert!(!stage.ask(UiSound::LevelUp(5), &jingles));
        assert!(stage.ask(UiSound::QuestClaim, &jingles));
        assert_eq!(stage.next(0.0, &jingles), Some(UiSound::QuestDone));
        assert_eq!(stage.next(1.0, &jingles), None);
        assert_eq!(stage.next(1.2, &jingles), Some(UiSound::LevelUp(5)));
        assert_eq!(stage.next(9.0, &jingles), None);
    }

    /// Levels reached together ring once, as the grandest: a milestone over a higher level.
    #[test]
    fn levels_together_ring_once_as_the_grandest() {
        let jingles = jingles(10);
        let mut stage = Stage::default();
        for level in [4, 7, 10, 12] {
            stage.ask(UiSound::LevelUp(level), &jingles);
        }
        stage.ask(UiSound::QuestDone, &jingles);
        stage.ask(UiSound::QuestDone, &jingles);
        assert_eq!(stage.waiting, [UiSound::LevelUp(10), UiSound::QuestDone]);
        let mut stage = Stage::default();
        for level in [3, 8, 5] {
            stage.ask(UiSound::LevelUp(level), &jingles);
        }
        assert_eq!(stage.waiting, [UiSound::LevelUp(8)]);
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
