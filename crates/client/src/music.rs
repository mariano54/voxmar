//! Background music, played now and then like Minecraft's: a track starts soon on the title
//! screen, then long quiet gaps separate tracks. Starting a game fades the menu's music out,
//! so a world opens in quiet and its first track comes after a gap like any other;
//! returning to the menus fades the game's music out and the menu's starts again soon. `M`
//! mutes or unmutes.
//!
//! Voxmar's own tracks are the files in `assets/music` (generated offline by
//! `tools/generate_music.py`). With the `minecraft_assets` setting on, the tracks of the
//! player's own Prism Launcher instance join them (`prism`), each read from where it is
//! as it is about to play. Each track suits some moments (the menus, the world above,
//! deep underground), as `music.ron` says (`rules`); a pick is among those suiting the
//! moment, passing over the ones played lately.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, GlobalVolume, PlaybackSettings, Volume};
use bevy::prelude::*;
use crossbeam_channel::{Receiver, TryRecvError, bounded};

use crate::ambience::Surroundings;
use crate::menu::{InMenu, Screen};
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::sfx::Audible;

use self::prism::{Found, Place, TrackFile};
use self::rules::{Context, MusicRules};

pub mod prism;
pub mod rules;

/// Seconds before another pick when a borrowed track could not be read.
const RETRY_AFTER: f32 = 2.0;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_tracks)
            .add_systems(OnEnter(Screen::InGame), start_game)
            .add_systems(OnExit(Screen::InGame), leave_game)
            .add_systems(
                Update,
                (
                    toggle_music,
                    (receive_found, schedule_music, start_loaded).chain().in_set(Audible),
                    fade_out,
                    set_volume.run_if(resource_changed::<Settings>),
                ),
            );
    }
}

/// A track in the rotation.
struct Track {
    source: Source,
    contexts: Vec<Context>,
    weight: f32,
    /// What the log calls it.
    name: String,
}

enum Source {
    Own(Handle<AudioSource>),
    Borrowed(TrackFile),
}

impl Track {
    fn own(&self) -> bool {
        matches!(self.source, Source::Own(_))
    }
}

#[derive(Resource)]
struct Music {
    rules: MusicRules,
    tracks: Vec<Track>,
    next_at: Option<Duration>,
    playing: Option<Entity>,
    /// The tracks played lately, newest last.
    recent: VecDeque<usize>,
    enabled: bool,
    /// Borrowed tracks still being looked for.
    found: Option<Receiver<Found>>,
    /// A borrowed track being read, to play once it is.
    loading: Option<(usize, Receiver<std::io::Result<Vec<u8>>>)>,
}

impl Music {
    /// Fades the playing track out, if there is one, and forgets one being read.
    fn fade_out(&mut self, commands: &mut Commands) {
        self.loading = None;
        if let Some(entity) = self.playing.take() {
            commands.entity(entity).try_insert(FadingOut(self.rules.fade_out));
        }
    }

    /// Gain of a track playing in full.
    fn level(&self, settings: &Settings, own: bool) -> f32 {
        let borrowed = if own { 1.0 } else { self.rules.borrowed_volume };
        self.rules.volume * borrowed * settings.music_volume as f32 / 100.0
    }
}

/// A track playing, and whether it is one of Voxmar's own.
#[derive(Component)]
struct MusicTrack {
    own: bool,
}

/// A track on its way out, with the seconds it has left.
#[derive(Component)]
struct FadingOut(f32);

fn load_tracks(mut commands: Commands, assets: Res<AssetServer>, settings: Res<Settings>, launch: Option<Res<Launch>>) {
    let rules = match launch.as_ref().map(|launch| MusicRules::load(&launch.content.files)) {
        Some(Ok(rules)) => rules,
        Some(Err(e)) => {
            error!("music rules: {e}");
            MusicRules::default()
        }
        None => MusicRules::default(),
    };
    let directory = voxmar_server::assets_dir().join("music");
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".mp3") || name.ends_with(".ogg"))
        .collect();
    names.sort();
    info!("{} music tracks", names.len());
    let tracks = names
        .into_iter()
        .map(|name| Track {
            source: Source::Own(assets.load(format!("music/{name}"))),
            contexts: rules.own_contexts.clone(),
            weight: 1.0,
            name,
        })
        .collect();
    // Looking through a modpack's jars takes a moment; Voxmar's own tracks play meanwhile.
    let found = settings.minecraft_assets.then(|| {
        let (tx, rx) = bounded(1);
        let place = Place { launcher: settings.prism_folder.clone(), instance: settings.prism_instance.clone() };
        let discovery = rules.prism.clone();
        std::thread::spawn(move || {
            let _ = tx.send(prism::discover(&place, &discovery));
        });
        rx
    });
    commands.insert_resource(Music { rules, tracks, next_at: None, playing: None, recent: VecDeque::new(), enabled: true, found, loading: None });
}

/// Adds the borrowed tracks once found, each where its rule says it plays.
fn receive_found(mut music: ResMut<Music>) {
    let Some(rx) = &music.found else { return };
    let found = match rx.try_recv() {
        Ok(found) => found,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => {
            music.found = None;
            return;
        }
    };
    music.found = None;
    let Some(instance) = &found.instance else {
        info!("music: no Prism Launcher instance found");
        return;
    };
    let mut categories: Vec<(String, usize)> = Vec::new();
    let mut added = Vec::new();
    for track in found.tracks {
        let Some(rule) = music.rules.rule(&track.location).filter(|rule| !rule.contexts.is_empty()) else { continue };
        match categories.iter_mut().find(|(name, _)| *name == rule.category) {
            Some((_, count)) => *count += 1,
            None => categories.push((rule.category.clone(), 1)),
        }
        added.push(Track { source: Source::Borrowed(track.file), contexts: rule.contexts.clone(), weight: rule.weight, name: track.location });
    }
    let places: Vec<String> = found.counts.iter().map(|(place, count)| format!("{place} {count}")).collect();
    let categories: Vec<String> = categories.iter().map(|(category, count)| format!("{category} {count}")).collect();
    info!(
        "music: {} tracks from {} ({}; {})",
        added.len(),
        instance.display(),
        places.join(", "),
        categories.join(", ")
    );
    music.tracks.extend(added);
}

/// Cheap varied number in `range` from the clock; music timing needs no determinism.
/// `salt` gives a different number for each draw in one frame.
fn random_in(time: &Time, salt: u64, (low, high): (f32, f32)) -> f32 {
    let nanos = time.elapsed().as_nanos() as u64 ^ salt.wrapping_mul(0xa076_1d64_78bd_642f);
    let mixed = nanos.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
    low + (high - low) * (mixed as f32 / (1u64 << 24) as f32)
}

/// The track to play next where the player is: Voxmar's own `own_share` of the time when
/// borrowed tracks suit too, then by weight, passing over the ones played lately while
/// others suit. `rolls` are two numbers in 0..1.
fn pick(tracks: &[Track], context: Context, recent: &VecDeque<usize>, own_share: f32, rolls: [f32; 2]) -> Option<usize> {
    let suits = |context: Context| -> Vec<usize> { (0..tracks.len()).filter(|&i| tracks[i].contexts.contains(&context)).collect() };
    let mut suiting = suits(context);
    if suiting.is_empty() && context == Context::Underground {
        suiting = suits(Context::Game);
    }
    let (own, borrowed): (Vec<usize>, Vec<usize>) = suiting.into_iter().partition(|&i| tracks[i].own());
    let mut group = match (own.is_empty(), borrowed.is_empty()) {
        (true, true) => return None,
        (false, true) => own,
        (true, false) => borrowed,
        (false, false) => {
            if rolls[0] < own_share {
                own
            } else {
                borrowed
            }
        }
    };
    let fresh: Vec<usize> = group.iter().copied().filter(|i| !recent.contains(i)).collect();
    if !fresh.is_empty() {
        group = fresh;
    } else if let Some(&last) = recent.back()
        && group.len() > 1
    {
        group.retain(|&i| i != last);
    }
    let total: f32 = group.iter().map(|&i| tracks[i].weight.max(0.0)).sum();
    let mut left = rolls[1] * total;
    for &i in &group {
        left -= tracks[i].weight.max(0.0);
        if left < 0.0 {
            return Some(i);
        }
    }
    group.last().copied()
}

#[allow(clippy::too_many_arguments)]
fn schedule_music(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    menu: Option<Res<State<InMenu>>>,
    settings: Res<Settings>,
    surroundings: Option<Res<Surroundings>>,
    mut music: ResMut<Music>,
    playing: Query<(), With<MusicTrack>>,
) {
    let in_menu = menu.is_some();
    if !(player.joined || in_menu) || music.tracks.is_empty() || !music.enabled || settings.music_volume == 0 {
        return;
    }
    // A finished track despawns itself; schedule the next one after a quiet gap.
    if let Some(entity) = music.playing
        && playing.get(entity).is_err()
    {
        music.playing = None;
        music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, music.rules.gap)));
    }
    if music.playing.is_some() || music.loading.is_some() {
        return;
    }
    let first = music.rules.first_menu_delay;
    let next_at = *music.next_at.get_or_insert(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, first)));
    if time.elapsed() < next_at {
        return;
    }
    let underground = &music.rules.underground;
    let context = match &surroundings {
        _ if in_menu => Context::Menu,
        Some(around) if around.depth >= underground.depth && around.sky <= underground.max_sky => Context::Underground,
        _ => Context::Game,
    };
    let rolls = [random_in(&time, 1, (0.0, 1.0)), random_in(&time, 2, (0.0, 1.0))];
    let Some(index) = pick(&music.tracks, context, &music.recent, music.rules.own_share, rolls) else {
        // Nothing suits here; look again after a gap.
        music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, music.rules.gap)));
        return;
    };
    music.recent.push_back(index);
    while music.recent.len() > music.rules.recent.max(1) {
        music.recent.pop_front();
    }
    match music.tracks[index].source {
        Source::Own(ref handle) => {
            let handle = handle.clone();
            let entity = spawn_track(&mut commands, handle, true, music.level(&settings, true));
            music.playing = Some(entity);
        }
        Source::Borrowed(ref file) => {
            // Read off the main thread: a track is several megabytes, perhaps out of a big zip.
            let (tx, rx) = bounded(1);
            let file = file.clone();
            std::thread::spawn(move || {
                let _ = tx.send(file.read());
            });
            music.loading = Some((index, rx));
        }
    }
}

fn spawn_track(commands: &mut Commands, source: Handle<AudioSource>, own: bool, level: f32) -> Entity {
    commands.spawn((MusicTrack { own }, AudioPlayer(source), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(level)))).id()
}

/// Plays a borrowed track once it has been read.
fn start_loaded(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    mut sources: ResMut<Assets<AudioSource>>,
    mut music: ResMut<Music>,
) {
    let Some((index, rx)) = &music.loading else { return };
    let index = *index;
    let read = match rx.try_recv() {
        Ok(read) => read,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(std::io::Error::other("reader stopped")),
    };
    music.loading = None;
    match read {
        Ok(bytes) => {
            let source = sources.add(AudioSource { bytes: Arc::from(bytes) });
            let entity = spawn_track(&mut commands, source, false, music.level(&settings, false));
            music.playing = Some(entity);
        }
        Err(e) => {
            warn!("music: cannot read {}: {e}", music.tracks[index].name);
            music.next_at = Some(time.elapsed() + Duration::from_secs_f32(RETRY_AFTER));
        }
    }
}

/// Starting a game fades out the menu's music; the game's first track comes after a quiet
/// gap, as between tracks.
fn start_game(mut commands: Commands, time: Res<Time>, music: Option<ResMut<Music>>) {
    let Some(mut music) = music else { return };
    music.fade_out(&mut commands);
    music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, music.rules.gap)));
}

/// Back in the menus, the game's music fades out and the menu's starts again soon.
fn leave_game(mut commands: Commands, time: Res<Time>, music: Option<ResMut<Music>>) {
    let Some(mut music) = music else { return };
    music.fade_out(&mut commands);
    music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, music.rules.first_menu_delay)));
}

fn fade_out(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    global: Res<GlobalVolume>,
    music: Option<Res<Music>>,
    mut fading: Query<(Entity, &MusicTrack, &mut FadingOut, Option<&mut AudioSink>)>,
) {
    let Some(music) = music else { return };
    for (entity, track, mut fade, sink) in &mut fading {
        fade.0 -= time.delta_secs();
        // A track that has not started playing yet has nothing to fade.
        let Some(mut sink) = sink.filter(|_| fade.0 > 0.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        // Bevy applies the global volume only as a track starts (see main.rs).
        let left = fade.0 / music.rules.fade_out.max(f32::EPSILON);
        sink.set_volume(Volume::Linear(music.level(&settings, track.own) * left) * global.volume);
    }
}

fn set_volume(
    settings: Res<Settings>,
    global: Res<GlobalVolume>,
    music: Option<Res<Music>>,
    mut playing: Query<(&MusicTrack, &mut AudioSink), Without<FadingOut>>,
) {
    let Some(music) = music else { return };
    for (track, mut sink) in &mut playing {
        sink.set_volume(Volume::Linear(music.level(&settings, track.own)) * global.volume);
    }
}

fn toggle_music(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiState>,
    time: Res<Time>,
    mut music: ResMut<Music>,
) {
    if !ui.cursor_locked || !keys.just_pressed(KeyCode::KeyM) {
        return;
    }
    music.enabled = !music.enabled;
    music.loading = None;
    if let Some(entity) = music.playing.take() {
        commands.entity(entity).despawn();
    }
    // When turned back on, start a track soon rather than after a long gap.
    music.next_at = Some(time.elapsed() + Duration::from_secs(2));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(own: bool, contexts: &[Context], weight: f32) -> Track {
        let source = if own { Source::Own(Handle::default()) } else { Source::Borrowed(TrackFile::Loose("x.ogg".into())) };
        Track { source, contexts: contexts.to_vec(), weight, name: String::new() }
    }

    #[test]
    fn picks_suit_the_moment_and_share_between_own_and_borrowed() {
        use Context::*;
        let tracks = [
            track(true, &[Menu, Game, Underground], 1.0),
            track(false, &[Menu], 1.0),
            track(false, &[Game, Underground], 1.0),
            track(false, &[Underground], 3.0),
        ];
        let none = VecDeque::new();
        // In the menus: own below the share, else the one borrowed menu track.
        assert_eq!(pick(&tracks, Menu, &none, 0.25, [0.1, 0.5]), Some(0));
        assert_eq!(pick(&tracks, Menu, &none, 0.25, [0.9, 0.5]), Some(1));
        // Underground, by weight: one in four the world's track, else the deep one.
        assert_eq!(pick(&tracks, Underground, &none, 0.25, [0.9, 0.2]), Some(2));
        assert_eq!(pick(&tracks, Underground, &none, 0.25, [0.9, 0.3]), Some(3));
        // Never the deep track above ground.
        for roll in [0.0, 0.5, 0.99] {
            assert_eq!(pick(&tracks, Game, &none, 0.25, [0.9, roll]), Some(2));
        }
        // Played lately is passed over while others suit, but a lone track still plays,
        // and only the very last is avoided when every one was played lately.
        let lately = VecDeque::from([3]);
        assert_eq!(pick(&tracks, Underground, &lately, 0.25, [0.9, 0.9]), Some(2));
        assert_eq!(pick(&tracks, Game, &VecDeque::from([2]), 0.25, [0.9, 0.5]), Some(2));
        assert_eq!(pick(&tracks, Underground, &VecDeque::from([3, 2]), 0.25, [0.9, 0.0]), Some(3));
        // Nothing borrowed: always Voxmar's own; nothing at all: none.
        assert_eq!(pick(&tracks[..1], Menu, &none, 0.0, [0.9, 0.9]), Some(0));
        assert_eq!(pick(&[], Game, &none, 0.25, [0.5, 0.5]), None);
        // Underground with nothing for it falls back to the world's tracks.
        assert_eq!(pick(&[track(false, &[Game], 1.0)], Underground, &none, 0.25, [0.5, 0.5]), Some(0));
    }

    /// `cargo test --profile iter -p voxmar_client prism_music_decodes -- --ignored
    /// --nocapture` finds the music of the Prism Launcher instance on this machine as the
    /// game does, counts it by place and category, and checks every track decodes.
    #[test]
    #[ignore]
    fn prism_music_decodes() {
        use bevy::audio::Decodable;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let rules = MusicRules::load(&content.files).unwrap();
        let found = prism::discover(&Place::default(), &rules.prism);
        let instance = found.instance.clone().expect("a Prism Launcher instance");
        println!("instance {}", instance.display());
        for (place, count) in &found.counts {
            println!("  {place}: {count}");
        }
        let mut categories: std::collections::BTreeMap<String, usize> = Default::default();
        for track in &found.tracks {
            let rule = rules.rule(&track.location).unwrap_or_else(|| panic!("{} has no rule", track.location));
            *categories.entry(rule.category.clone()).or_default() += 1;
            let source = AudioSource { bytes: Arc::from(track.file.read().unwrap_or_else(|e| panic!("{}: {e}", track.location))) };
            assert!(source.decoder().take(1000).count() == 1000, "{} does not decode", track.location);
        }
        println!("{categories:?}");
        println!("{} tracks decoded", found.tracks.len());
        assert!(!found.tracks.is_empty());
    }
}
