//! The interface's sounds: a click when a button is pressed and a tap when an item is
//! picked up or put down in the inventory, in one of a few sets the player chooses
//! (`UiSounds` in the settings). Every set is synthesized at startup, like the block
//! sounds, and follows the sound effects volume.
//!
//! Each sound is a few decaying sine partials, which may glide in pitch, be frequency
//! modulated or blip twice, and may carry a breath of high noise.
//!
//! Events play jingles instead, whatever the set chosen: a quest finished, its rewards
//! claimed (coins), a skill's level reached, and a grander one at a milestone level. They
//! are recordings each package's `sounds.ron` lists (the base package's are our own, from
//! `tools/generate_jingles.py`). The HUD's moments (`hud::celebrate`) ask for the tunes as
//! each one shows, so two never start over each other.

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::Volume;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::widgets::Disabled;
use crate::mix::{Category, Voice};
use crate::settings::Settings;
use crate::session::Launch;
use crate::sfx::{Audible, InterfaceSound, MASTER, RATE, Recordings, Tap, wav};

pub struct UiSoundPlugin;

impl Plugin for UiSoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiSound>()
            .init_resource::<JinglesAsked>()
            .add_systems(Startup, (synthesize, jingle_clips))
            .add_systems(Update, ask_jingles)
            .add_systems(Update, ((click_buttons, hover_buttons, hear_screens, hear_refusals, hear_news), play.in_set(Audible)).chain())
            .add_systems(Update, play_jingles.in_set(Audible));
    }
}

/// A sound the interface makes. What each sounds like is `sounds.ron`'s (`interface`, by
/// [`UiSound::name`]): the chosen set's click or tap, clips of its own, a jingle, or
/// nothing.
#[derive(Message, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiSound {
    /// A button pressed.
    Click,
    /// The pointer over a button.
    Hover,
    /// An item picked up or put down.
    Item,
    /// Half a stack picked up (a right click).
    Split,
    /// A screen opened (the inventory, the quest book, the character screen, the journal,
    /// the recipes, the menu).
    Open,
    /// A screen closed, with none opening.
    Close,
    /// A quest picked in the book.
    QuestPick,
    /// Something the player tried was refused, and the HUD says why.
    Refuse,
    /// A quest finished.
    QuestDone,
    /// A finished quest's rewards claimed: coins.
    QuestClaim,
    /// A skill reached a new level.
    LevelUp,
    /// A skill reached a milestone level (`hud::celebrate`).
    Milestone,
    /// Something the player set going finished while they were away: a clamp charred
    /// through, a kiln gone cold (`ServerMessage::Fired`).
    Fired,
}

impl UiSound {
    pub const ALL: [UiSound; 13] = [
        UiSound::Click,
        UiSound::Hover,
        UiSound::Item,
        UiSound::Split,
        UiSound::Open,
        UiSound::Close,
        UiSound::QuestPick,
        UiSound::Refuse,
        UiSound::QuestDone,
        UiSound::QuestClaim,
        UiSound::LevelUp,
        UiSound::Milestone,
        UiSound::Fired,
    ];

    /// Its name in `sounds.ron`'s `interface`.
    pub fn name(self) -> &'static str {
        match self {
            UiSound::Click => "click",
            UiSound::Hover => "hover",
            UiSound::Item => "item",
            UiSound::Split => "split",
            UiSound::Open => "open",
            UiSound::Close => "close",
            UiSound::QuestPick => "quest_pick",
            UiSound::Refuse => "refuse",
            UiSound::QuestDone => "quest_done",
            UiSound::QuestClaim => "quest_claim",
            UiSound::LevelUp => "level_up",
            UiSound::Milestone => "level_milestone",
            UiSound::Fired => "fired",
        }
    }

    pub fn named(name: &str) -> Option<UiSound> {
        UiSound::ALL.into_iter().find(|sound| sound.name() == name)
    }

    /// Whether its tune is one of the events' jingles, which sound whatever the set chosen.
    pub fn is_jingle(self) -> bool {
        matches!(self, UiSound::QuestDone | UiSound::QuestClaim | UiSound::LevelUp | UiSound::Milestone)
    }
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
}

impl Jingles {
    /// The clips `sound` takes turns among and how loud it plays, or `None` for no jingle. A
    /// milestone without clips of its own rings as a level.
    fn of(&self, sound: UiSound) -> Option<(&[Handle<AudioSource>], f32)> {
        let list = match sound {
            UiSound::QuestDone => &self.done,
            UiSound::QuestClaim => &self.claim,
            UiSound::Milestone if !self.milestone.is_empty() => &self.milestone,
            UiSound::LevelUp | UiSound::Milestone => &self.level,
            _ => return None,
        };
        (!list.is_empty()).then_some((list.as_slice(), jingle_gain(sound)))
    }
}

/// How loud a jingle plays: the coins of a claim softer than the tunes.
pub(crate) fn jingle_gain(sound: UiSound) -> f32 {
    if sound == UiSound::QuestClaim { 0.3 } else { 0.5 }
}

/// How an interface sound is heard, from `sounds.ron`'s `interface`.
#[derive(Clone, Debug, PartialEq)]
enum Heard {
    /// The chosen set's click (0) or item tap (1), at a volume and a speed.
    Tap(usize, f32, f32),
    /// Clips of its own, taking turns, at a volume and a speed.
    Clips(Vec<Handle<AudioSource>>, f32, f32),
    /// A jingle, or nothing.
    Not,
}

/// What every interface sound plays.
#[derive(Resource, Default)]
struct Interface(Vec<(UiSound, Heard)>);

impl Interface {
    fn of(&self, sound: UiSound) -> &Heard {
        self.0.iter().find(|(s, _)| *s == sound).map_or(&Heard::Not, |(_, heard)| heard)
    }
}

/// How each interface sound is heard, as `recorded` says, loading its own clips with `load`.
fn heard(recorded: &Recordings, mut load: impl FnMut(&str) -> Handle<AudioSource>) -> Interface {
    Interface(
        UiSound::ALL
            .into_iter()
            .map(|sound| {
                let heard = match recorded.interface.get(sound.name()) {
                    Some(InterfaceSound { tap: Some(tap), volume, pitch, .. }) => Heard::Tap(if *tap == Tap::Click { 0 } else { 1 }, *volume, *pitch),
                    Some(def) if !def.sounds.is_empty() => Heard::Clips(def.sounds.iter().map(|path| load(path)).collect(), def.volume, def.pitch),
                    _ => Heard::Not,
                };
                (sound, heard)
            })
            .collect(),
    )
}

fn jingle_clips(mut commands: Commands, assets: Res<AssetServer>, launch: Res<Launch>) {
    let recorded = Recordings::load(&launch.content.files).unwrap_or_else(|e| {
        error!("no jingles: {e}");
        Recordings::default()
    });
    commands.insert_resource(heard(&recorded, |path| assets.load(path.to_string())));
    let load = |paths: &[String]| paths.iter().map(|path| assets.load::<AudioSource>(path.clone())).collect();
    commands.insert_resource(Jingles {
        done: load(&recorded.quest_done),
        claim: load(&recorded.quest_claim),
        level: load(&recorded.level_up),
        milestone: load(&recorded.level_milestone),
    });
}

/// The jingles asked for since the game started, in order, each with the clips it takes
/// turns among (their asset paths): for the gate, which hears nothing in a silent run.
#[derive(Resource, Default, Debug)]
pub struct JinglesAsked(pub Vec<(UiSound, Vec<String>)>);

fn ask_jingles(mut sounds: MessageReader<UiSound>, jingles: Option<Res<Jingles>>, assets: Res<AssetServer>, mut asked: ResMut<JinglesAsked>) {
    let Some(jingles) = jingles else { return };
    for &sound in sounds.read() {
        if let Some((list, _)) = jingles.of(sound) {
            asked.0.push((sound, list.iter().filter_map(|clip| assets.get_path(clip)).map(|path| path.to_string()).collect()));
        }
    }
}

/// Plays the jingles asked for, each once a frame, taking turns among a package's clips
/// where it lists several.
fn play_jingles(mut commands: Commands, mut sounds: MessageReader<UiSound>, jingles: Option<Res<Jingles>>, settings: Res<Settings>, mut turn: Local<usize>) {
    let Some(jingles) = jingles else { return };
    let volume = MASTER * settings.sound_volume as f32 / 100.0;
    let mut played = Vec::new();
    for &sound in sounds.read() {
        let Some((list, gain)) = jingles.of(sound).filter(|_| volume > 0.0 && !played.contains(&sound)) else { continue };
        played.push(sound);
        *turn += 1;
        let playback = PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume));
        commands.spawn((AudioPlayer(list[*turn % list.len()].clone()), playback, Voice::of(Category::Jingles)));
    }
}

#[allow(clippy::type_complexity)]
fn click_buttons(
    buttons: Query<(&Interaction, Has<crate::hud::quests::QuestNode>), (Changed<Interaction>, With<Button>, Without<Disabled>, Without<Silent>)>,
    mut sounds: MessageWriter<UiSound>,
) {
    for (interaction, quest) in &buttons {
        if *interaction == Interaction::Pressed {
            sounds.write(if quest { UiSound::QuestPick } else { UiSound::Click });
        }
    }
}

#[allow(clippy::type_complexity)]
fn hover_buttons(buttons: Query<&Interaction, (Changed<Interaction>, With<Button>, Without<Disabled>)>, mut sounds: MessageWriter<UiSound>) {
    if buttons.iter().any(|interaction| *interaction == Interaction::Hovered) {
        sounds.write(UiSound::Hover);
    }
}

fn play(mut commands: Commands, mut sounds: MessageReader<UiSound>, clips: Option<Res<Clips>>, interface: Option<Res<Interface>>, settings: Res<Settings>, mut turn: Local<usize>) {
    let (Some(clips), Some(interface)) = (clips, interface) else { return };
    let volume = MASTER * settings.sound_volume as f32 / 100.0;
    let set = clips.0.iter().find(|(set, _)| *set == settings.ui_sounds).map(|(_, clips)| clips);
    let mut played = Vec::new();
    for &sound in sounds.read() {
        // One of each a frame, however many buttons or moves asked; none with the
        // interface's sounds off.
        let Some(set) = set.filter(|_| !played.contains(&sound) && volume > 0.0) else { continue };
        played.push(sound);
        let (clip, gain, speed) = match interface.of(sound) {
            Heard::Tap(clip, gain, speed) => (set[*clip].clone(), *gain, *speed),
            Heard::Clips(list, gain, speed) if !list.is_empty() => {
                *turn += 1;
                (list[*turn % list.len()].clone(), *gain, *speed)
            }
            _ => continue,
        };
        let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume)) };
        commands.spawn((AudioPlayer(clip), playback, Voice::of(Category::Ui)));
    }
}

/// A refusal newly shown, or shown again, by the HUD (`hud::skills`) sounds `Refuse`, at most
/// every [`REFUSE_EVERY`] seconds however fast the player keeps trying.
fn hear_refusals(toasts: Option<Res<crate::hud::skills::SkillToasts>>, time: Res<Time>, mut last: Local<(f32, f32)>, mut sounds: MessageWriter<UiSound>) {
    let Some(newest) = toasts.and_then(|t| t.newest_refusal()) else { return };
    let now = time.elapsed_secs();
    let (seen, heard) = *last;
    if newest > seen {
        if now - heard >= REFUSE_EVERY || heard == 0.0 {
            sounds.write(UiSound::Refuse);
            last.1 = now;
        }
        last.0 = newest;
    }
}

const REFUSE_EVERY: f32 = 0.35;

/// News of something finished elsewhere (`hud::skills`) sounds `Fired`, once however much
/// came in the same frame.
fn hear_news(toasts: Option<ResMut<crate::hud::skills::SkillToasts>>, mut sounds: MessageWriter<UiSound>) {
    if let Some(mut toasts) = toasts
        && !toasts.take_fired().is_empty()
    {
        sounds.write(UiSound::Fired);
    }
}

/// A screen opening sounds `Open`; one closing with none opening, `Close` (the recipes
/// closing back onto the inventory, the last screen shut). Switching from one screen to
/// another in a frame is an opening.
fn hear_screens(ui: Option<Res<crate::player::UiState>>, mut was: Local<[bool; 6]>, mut sounds: MessageWriter<UiSound>) {
    let Some(ui) = ui else { return };
    let now = [ui.inventory_open, ui.quests_open, ui.character_open, ui.journal_open, ui.recipes_open, ui.menu_open];
    if now.iter().zip(was.iter()).any(|(now, was)| *now && !was) {
        sounds.write(UiSound::Open);
    } else if now.iter().zip(was.iter()).any(|(now, was)| !now && *was) {
        sounds.write(UiSound::Close);
    }
    *was = now;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every set's clicks and taps and the interface's own clips, as `sounds.ron` plays them,
    /// are heard within the interface's band, and every jingle within the jingles'.
    #[test]
    fn interface_sounds_and_jingles_are_heard_within_their_bands() {
        use crate::mix::tests::{all, measure, within};
        use crate::sfx::loudness;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        let mut taps = Vec::new();
        for set in UiSounds::ALL {
            let levels: [f32; 2] = match (set.files(), set.recipe()) {
                (Some(files), _) => files.map(|file| measure(&format!("ui/sounds/{file}"))[0]),
                (None, Some([click, item])) => [loudness(&click.render(7), 1), loudness(&item.render(11), 1)],
                (None, None) => continue,
            };
            for sound in UiSound::ALL {
                let def = &recorded.interface[sound.name()];
                if let Some(tap) = def.tap {
                    taps.push((format!("{set:?} {sound:?}"), levels[if tap == Tap::Click { 0 } else { 1 }] * def.volume));
                }
            }
        }
        for sound in UiSound::ALL {
            let def = &recorded.interface[sound.name()];
            taps.extend(def.sounds.iter().map(|path| (path.clone(), measure(path)[0] * def.volume)));
        }
        assert!(taps.iter().any(|(name, _)| name.contains("refuse")), "the refusal's clips are measured");
        let events = [
            (UiSound::QuestDone, &recorded.quest_done),
            (UiSound::QuestClaim, &recorded.quest_claim),
            (UiSound::LevelUp, &recorded.level_up),
            (UiSound::Milestone, &recorded.level_milestone),
        ];
        let jingles = events.into_iter().flat_map(|(sound, paths)| paths.iter().map(move |path| (path.clone(), measure(path)[0] * jingle_gain(sound))));
        all([within(Category::Ui, taps), within(Category::Jingles, jingles)]);
    }

    /// Opening a screen sounds once, switching straight to another opens again, and only
    /// the last one closing closes.
    #[test]
    fn screens_open_and_close_aloud() {
        let mut app = App::new();
        app.add_message::<UiSound>().init_resource::<crate::player::UiState>().add_systems(Update, hear_screens);
        let step = |app: &mut App, change: fn(&mut crate::player::UiState)| -> Vec<UiSound> {
            change(&mut app.world_mut().resource_mut::<crate::player::UiState>());
            app.update();
            app.world_mut().resource_mut::<Messages<UiSound>>().drain().collect()
        };
        assert_eq!(step(&mut app, |_| {}), []);
        assert_eq!(step(&mut app, |ui| ui.inventory_open = true), [UiSound::Open]);
        assert_eq!(step(&mut app, |_| {}), []);
        assert_eq!(step(&mut app, |ui| (ui.inventory_open, ui.quests_open) = (false, true)), [UiSound::Open]);
        assert_eq!(step(&mut app, |ui| ui.quests_open = false), [UiSound::Close]);
        assert_eq!(step(&mut app, |_| {}), []);
        // The recipes over the inventory open, and close back onto it; so does the menu.
        assert_eq!(step(&mut app, |ui| ui.inventory_open = true), [UiSound::Open]);
        assert_eq!(step(&mut app, |ui| ui.recipes_open = true), [UiSound::Open]);
        assert_eq!(step(&mut app, |ui| ui.recipes_open = false), [UiSound::Close]);
        assert_eq!(step(&mut app, |ui| ui.inventory_open = false), [UiSound::Close]);
        assert_eq!(step(&mut app, |ui| ui.menu_open = true), [UiSound::Open]);
        assert_eq!(step(&mut app, |ui| ui.menu_open = false), [UiSound::Close]);
    }

    /// A refusal shown sounds once; the same one again soon after does not chatter, but
    /// later it is heard again.
    #[test]
    fn refusals_are_heard_without_chattering() {
        use crate::hud::skills::SkillToasts;
        let mut app = App::new();
        app.add_message::<UiSound>().init_resource::<SkillToasts>().init_resource::<Time>().add_systems(Update, hear_refusals);
        let mut step = |at: f32, refuse: Option<&str>| -> usize {
            app.world_mut().resource_mut::<Time>().advance_to(std::time::Duration::from_secs_f32(at));
            if let Some(why) = refuse {
                app.world_mut().resource_mut::<SkillToasts>().refused(why.into(), at);
            }
            app.update();
            app.world_mut().resource_mut::<Messages<UiSound>>().drain().filter(|s| *s == UiSound::Refuse).count()
        };
        assert_eq!(step(0.5, None), 0);
        assert_eq!(step(1.0, Some("Needs Mining 5")), 1);
        assert_eq!(step(1.1, None), 0);
        assert_eq!(step(1.2, Some("Needs Mining 5")), 0, "kept trying, it does not chatter");
        assert_eq!(step(2.0, Some("Needs Mining 5")), 1, "tried again later, it is heard again");
        assert_eq!(step(2.1, Some("Cannot make that here")), 0);
        assert_eq!(step(3.0, Some("Cannot make that here")), 1);
    }

    /// Everything the interface does says how it sounds in the base package, and a package
    /// naming one the interface never makes, or saying two things of one, fails to load;
    /// one leaving any out fails too.
    #[test]
    fn every_interface_sound_is_named_both_ways() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        assert_eq!((recorded.check(&content.actions), recorded.covers(&content.actions)), (Ok(()), Ok(())));
        let file = |source: &str| voxmar_engine::content::ContentFile { package: "zmod".into(), name: "sounds.ron".into(), source: source.into() };
        let base = content.files.iter().find(|f| f.package == "base" && f.name == "sounds.ron").unwrap().clone();
        for wrong in [
            r#"(interface: {"whistle": (silent: true)})"#,
            r#"(interface: {"click": (silent: true, tap: Some(Click))})"#,
            r#"(interface: {"click": ()})"#,
            r#"(interface: {"click": (jingle: true)})"#,
            r#"(interface: {"quest_done": (silent: true)})"#,
            r#"(interface: {"refuse": (sounds: ["a.ogg"], volume: 0.0)})"#,
            r#"(interface: {"refuse": (sounds: ["a.ogg"], pitch: 3.0)})"#,
        ] {
            let more = Recordings::load(&[base.clone(), file(wrong)]).unwrap();
            assert!(more.check(&content.actions).is_err(), "{wrong}");
        }
        // A package may make a sound silent, or give the hover a tick.
        let quiet = Recordings::load(&[base.clone(), file(r#"(interface: {"refuse": (silent: true), "hover": (tap: Some(Click), volume: 0.1)})"#)]).unwrap();
        assert_eq!(quiet.check(&content.actions), Ok(()));
        // Leaving one out, as a package with only its own sounds.ron would, fails at launch.
        let alone = Recordings::load(&[file(r#"(interface: {"click": (tap: Some(Click))})"#)]).unwrap();
        assert!(alone.covers(&voxmar_engine::action::ActionRegistry::default()).is_err_and(|e| e.contains("refuse") && e.contains("quest_pick")));
    }

    /// Each sound is heard as `sounds.ron` says: a tap of the set, its own clips, or not.
    #[test]
    fn interface_sounds_are_heard_as_named() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let recorded = Recordings::load(&content.files).unwrap();
        let mut loaded = Vec::new();
        let interface = heard(&recorded, |path| {
            loaded.push(path.to_string());
            Handle::default()
        });
        assert_eq!(interface.of(UiSound::Click), &Heard::Tap(0, 0.4, 1.0));
        assert!(matches!(interface.of(UiSound::Split), Heard::Tap(1, _, pitch) if *pitch > 1.0));
        assert!(matches!(interface.of(UiSound::Refuse), Heard::Clips(list, ..) if list.len() == 2));
        assert_eq!([interface.of(UiSound::Hover), interface.of(UiSound::QuestDone)], [&Heard::Not, &Heard::Not]);
        assert!(loaded.iter().any(|p| p == "content/base/sounds/page_1.ogg"), "{loaded:?}");
    }

    #[test]
    fn recorded_sets_are_shipped() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/sounds");
        for set in UiSounds::ALL {
            for file in set.files().into_iter().flatten() {
                assert!(dir.join(file).is_file(), "{set:?}: no {file}");
            }
        }
    }

    /// Each event takes its own clips, a milestone without any rings as a level, and clicks
    /// are no jingle.
    #[test]
    fn events_take_their_jingles() {
        let clip = |n: usize| vec![Handle::<AudioSource>::default(); n];
        let jingles = Jingles { done: clip(1), claim: clip(3), level: clip(1), milestone: clip(2) };
        let count = |sound| jingles.of(sound).map(|(list, _)| list.len());
        assert_eq!([count(UiSound::QuestDone), count(UiSound::QuestClaim), count(UiSound::LevelUp), count(UiSound::Milestone)], [Some(1), Some(3), Some(1), Some(2)]);
        assert_eq!([count(UiSound::Click), count(UiSound::Refuse), count(UiSound::QuestPick)], [None, None, None]);
        let plain = Jingles { milestone: Vec::new(), ..jingles };
        assert_eq!(plain.of(UiSound::Milestone).map(|(list, _)| list.len()), Some(1));
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
