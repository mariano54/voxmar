//! Background music, played now and then like Minecraft's: a track starts soon on the title
//! screen, then long quiet gaps separate tracks. Starting a game fades the menu's music out,
//! so a world opens in quiet and its first track comes after a gap like any other;
//! returning to the menus fades the game's music out and the menu's starts again soon. `M`
//! mutes or unmutes.
//!
//! Music comes in categories (`music.ron`), which the audio settings turn on and off.
//! Voxmar's own tracks are the files in their category's folder of `assets/music`
//! (`suno`, made with Suno; `lyria`, generated offline by `tools/generate_music.py`). With
//! the `minecraft_assets` setting on, the tracks of the player's own Prism Launcher
//! instance join them (`prism`), each read from where it is as it is about to play. Each
//! track suits some moments (the menus, the world above, deep underground), as
//! `music.ron` says (`rules`); a pick is among those suiting the moment and in a category
//! that is on, passing over the ones played lately. A borrowed track is checked to decode
//! before it plays; one that does not (empty, cut short, another codec) leaves the
//! rotation.

use std::collections::VecDeque;
use std::io::Cursor;
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
use self::rules::{Category, Context, MusicRules};

pub mod prism;
pub mod rules;

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
                    (set_volume, stop_switched_off).run_if(resource_changed::<Settings>),
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
    /// Its category's name (`music.ron`).
    category: String,
}

enum Source {
    Own(Handle<AudioSource>),
    Borrowed(TrackFile),
}

impl Track {
    fn own(&self) -> bool {
        matches!(self.source, Source::Own(_))
    }

    /// Whether its category is on in the settings.
    fn on(&self, settings: &Settings) -> bool {
        !settings.music_off.contains(&self.category)
    }
}

/// The categories of music, in the settings' order, for the audio settings' switches.
#[derive(Resource, Clone, Default)]
pub struct MusicCategories(pub Vec<Category>);

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
    loading: Option<(usize, Receiver<Result<AudioSource, String>>)>,
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

/// A track playing, whether it is one of Voxmar's own, and its category.
#[derive(Component)]
struct MusicTrack {
    own: bool,
    category: String,
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
    let mut tracks = Vec::new();
    let mut counts = Vec::new();
    for category in &rules.categories {
        let Some(folder) = &category.folder else { continue };
        let mut names: Vec<String> = std::fs::read_dir(directory.join(folder))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".mp3") || name.ends_with(".ogg"))
            .collect();
        names.sort();
        counts.push(format!("{} {}", category.name, names.len()));
        tracks.extend(names.into_iter().map(|name| Track {
            source: Source::Own(assets.load(format!("music/{folder}/{name}"))),
            contexts: category.contexts.clone(),
            weight: 1.0,
            name: format!("{folder}/{name}"),
            category: category.name.clone(),
        }));
    }
    info!("{} music tracks ({})", tracks.len(), counts.join(", "));
    commands.insert_resource(MusicCategories(rules.categories.clone()));
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
    for problem in &found.problems {
        warn!("music: {problem}");
    }
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
        added.push(Track {
            source: Source::Borrowed(track.file),
            contexts: rule.contexts.clone(),
            weight: rule.weight,
            name: track.location,
            category: rule.category.clone(),
        });
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
    // Every copy skipped, so a place adding none is explained.
    let mut places: Vec<&str> = found.repeats.iter().map(|r| r.track.place.as_str()).collect();
    places.dedup();
    for place in places {
        let repeats: Vec<String> =
            found.repeats.iter().filter(|r| r.track.place == place).map(|r| format!("{} = {}", r.track.location, r.of)).collect();
        info!("music: {place}: {} copies of tracks found earlier: {}", repeats.len(), repeats.join(", "));
    }
    music.tracks.extend(added);
}

/// Reads a borrowed track and checks it decodes, as Bevy will decode it (`AudioSource`'s
/// decoder panics on a file it cannot open), off the main thread.
fn read_track(file: &TrackFile, max_bytes: u64) -> Result<AudioSource, String> {
    let source = AudioSource { bytes: Arc::from(file.read(max_bytes).map_err(|e| e.to_string())?) };
    let mut decoder = rodio::Decoder::builder()
        .with_byte_len(source.bytes.len() as u64)
        .with_data(Cursor::new(source.clone()))
        .build()
        .map_err(|e| format!("does not decode: {e}"))?;
    decoder.next().ok_or("has no sound")?;
    Ok(source)
}

/// Cheap varied number in `range` from the clock; music timing needs no determinism.
/// `salt` gives a different number for each draw in one frame.
fn random_in(time: &Time, salt: u64, (low, high): (f32, f32)) -> f32 {
    let nanos = time.elapsed().as_nanos() as u64 ^ salt.wrapping_mul(0xa076_1d64_78bd_642f);
    let mixed = nanos.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
    low + (high - low) * (mixed as f32 / (1u64 << 24) as f32)
}

/// The track to play next where the player is, among those whose category is on: Voxmar's
/// own `own_share` of the time when borrowed tracks suit too, then by weight, passing over
/// the ones played lately while others suit. `rolls` are two numbers in 0..1.
fn pick(tracks: &[Track], context: Context, settings: &Settings, recent: &VecDeque<usize>, own_share: f32, rolls: [f32; 2]) -> Option<usize> {
    let suits = |context: Context| -> Vec<usize> {
        (0..tracks.len()).filter(|&i| tracks[i].contexts.contains(&context) && tracks[i].on(settings)).collect()
    };
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
    let Some(index) = pick(&music.tracks, context, &settings, &music.recent, music.rules.own_share, rolls) else {
        // Nothing suits here; look again after a gap.
        music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, music.rules.gap)));
        return;
    };
    music.recent.push_back(index);
    while music.recent.len() > music.rules.recent.max(1) {
        music.recent.pop_front();
    }
    let category = music.tracks[index].category.clone();
    match music.tracks[index].source {
        Source::Own(ref handle) => {
            let handle = handle.clone();
            let entity = spawn_track(&mut commands, handle, true, category, music.level(&settings, true));
            music.playing = Some(entity);
        }
        Source::Borrowed(ref file) => {
            // Read off the main thread: a track is several megabytes, perhaps out of a big zip.
            let (tx, rx) = bounded(1);
            let (file, max_bytes) = (file.clone(), music.rules.prism.max_bytes);
            std::thread::spawn(move || {
                let _ = tx.send(read_track(&file, max_bytes));
            });
            music.loading = Some((index, rx));
        }
    }
}

fn spawn_track(commands: &mut Commands, source: Handle<AudioSource>, own: bool, category: String, level: f32) -> Entity {
    commands.spawn((MusicTrack { own, category }, AudioPlayer(source), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(level)))).id()
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
        Err(TryRecvError::Disconnected) => Err("its reader stopped".to_owned()),
    };
    music.loading = None;
    match read {
        Ok(source) => {
            let source = sources.add(source);
            let category = music.tracks[index].category.clone();
            let entity = spawn_track(&mut commands, source, false, category, music.level(&settings, false));
            music.playing = Some(entity);
        }
        Err(e) => {
            // It leaves the rotation, so the next pick is another track.
            let track = &mut music.tracks[index];
            warn!("music: {} cannot play, left out: {e}", track.name);
            track.contexts.clear();
            music.next_at = Some(time.elapsed() + Duration::from_secs_f32(music.rules.retry_after));
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

/// A category switched off in the settings fades out its track playing (or forgets one
/// being read), and another, from a category still on, comes soon.
fn stop_switched_off(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    music: Option<ResMut<Music>>,
    playing: Query<&MusicTrack, Without<FadingOut>>,
) {
    let Some(mut music) = music else { return };
    let loading_off = music.loading.as_ref().is_some_and(|(index, _)| !music.tracks[*index].on(&settings));
    let playing_off = music.playing.and_then(|entity| playing.get(entity).ok()).is_some_and(|track| settings.music_off.contains(&track.category));
    if loading_off || playing_off {
        music.fade_out(&mut commands);
        music.next_at = Some(time.elapsed() + Duration::from_secs_f32(music.rules.resume_after));
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
    music.next_at = Some(time.elapsed() + Duration::from_secs_f32(music.rules.resume_after));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(own: bool, contexts: &[Context], weight: f32) -> Track {
        let source = if own { Source::Own(Handle::default()) } else { Source::Borrowed(TrackFile::Loose("x.ogg".into())) };
        let category = if own { "ours" } else { "borrowed" }.to_owned();
        Track { source, contexts: contexts.to_vec(), weight, name: String::new(), category }
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
        let all = Settings::default();
        // In the menus: own below the share, else the one borrowed menu track.
        assert_eq!(pick(&tracks, Menu, &all, &none, 0.25, [0.1, 0.5]), Some(0));
        assert_eq!(pick(&tracks, Menu, &all, &none, 0.25, [0.9, 0.5]), Some(1));
        // Underground, by weight: one in four the world's track, else the deep one.
        assert_eq!(pick(&tracks, Underground, &all, &none, 0.25, [0.9, 0.2]), Some(2));
        assert_eq!(pick(&tracks, Underground, &all, &none, 0.25, [0.9, 0.3]), Some(3));
        // Never the deep track above ground.
        for roll in [0.0, 0.5, 0.99] {
            assert_eq!(pick(&tracks, Game, &all, &none, 0.25, [0.9, roll]), Some(2));
        }
        // Played lately is passed over while others suit, but a lone track still plays,
        // and only the very last is avoided when every one was played lately.
        let lately = VecDeque::from([3]);
        assert_eq!(pick(&tracks, Underground, &all, &lately, 0.25, [0.9, 0.9]), Some(2));
        assert_eq!(pick(&tracks, Game, &all, &VecDeque::from([2]), 0.25, [0.9, 0.5]), Some(2));
        assert_eq!(pick(&tracks, Underground, &all, &VecDeque::from([3, 2]), 0.25, [0.9, 0.0]), Some(3));
        // Nothing borrowed: always Voxmar's own; nothing at all: none.
        assert_eq!(pick(&tracks[..1], Menu, &all, &none, 0.0, [0.9, 0.9]), Some(0));
        assert_eq!(pick(&[], Game, &all, &none, 0.25, [0.5, 0.5]), None);
        // Underground with nothing for it falls back to the world's tracks.
        assert_eq!(pick(&[track(false, &[Game], 1.0)], Underground, &all, &none, 0.25, [0.5, 0.5]), Some(0));
        // A category switched off is never picked; with Voxmar's own off, the borrowed play
        // whatever the share, and with everything off, nothing does.
        let ours_off = Settings { music_off: vec!["ours".into()], ..Settings::default() };
        for roll in [0.0, 0.5, 0.99] {
            assert_eq!(pick(&tracks, Menu, &ours_off, &none, 0.25, [roll, roll]), Some(1));
        }
        let all_off = Settings { music_off: vec!["ours".into(), "borrowed".into()], ..Settings::default() };
        assert_eq!(pick(&tracks, Game, &all_off, &none, 0.25, [0.5, 0.5]), None);
    }

    /// A borrowed file that is empty or not audio is refused before it reaches Bevy's
    /// decoder (which would panic), and one that decodes is handed on.
    #[test]
    fn only_tracks_that_decode_are_played() {
        let (root, found) = prism::tests::found("decode");
        for location in ["st:music/broken/empty", "st:music/broken/garbled"] {
            let track = found.tracks.iter().find(|t| t.location == location).unwrap();
            let refused = read_track(&track.file, 1 << 20).map(|_| ()).unwrap_err();
            assert!(refused.contains("decode") || refused.contains("sound"), "{location}: {refused}");
        }
        let wav = root.join("tone.wav");
        let tone: Vec<f32> = (0..4410).map(|i| (i as f32 * 0.06).sin() * 0.3).collect();
        std::fs::write(&wav, crate::sfx::wav(&tone)).unwrap();
        let played = read_track(&TrackFile::Loose(wav.clone()), 1 << 20).unwrap();
        assert_eq!(played.bytes.len() as u64, std::fs::metadata(&wav).unwrap().len());
        assert!(read_track(&TrackFile::Loose(wav), 16).unwrap_err().contains("more than"));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// `cargo test --profile iter -p voxmar_client prism_music_decodes -- --ignored
    /// --nocapture` finds the music of the Prism Launcher instance on this machine as the
    /// game does and proves every track of it plays: it lists each place's count, and for
    /// the 1.21 and StaTech resource packs every candidate (location, size, CRC-32) with the
    /// track it repeats; it checks each candidate is kept or a copy of a kept track, that
    /// HorizonMelodies adds tracks, and that every kept track has a rule placing it and
    /// decodes.
    #[test]
    #[ignore]
    fn prism_music_decodes() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let rules = MusicRules::load(&content.files).unwrap();
        let found = prism::discover(&Place::default(), &rules.prism);
        let crc = |crc: Option<u32>| crc.map_or("(unique size)".to_owned(), |crc| format!("{crc:08x}"));
        assert!(found.problems.is_empty(), "{:?}", found.problems);
        let instance = found.instance.clone().expect("a Prism Launcher instance");
        println!("instance {}", instance.display());
        for (place, count) in &found.counts {
            println!("  {place}: {count} kept");
        }
        let kept: std::collections::HashSet<&str> = found.tracks.iter().map(|t| t.location.as_str()).collect();
        let places: std::collections::BTreeSet<&str> =
            found.tracks.iter().chain(found.repeats.iter().map(|r| &r.track)).map(|t| t.place.as_str()).collect();
        for place in places.iter().filter(|p| p.starts_with("resourcepacks/") && (p.contains("1.21") || p.to_lowercase().contains("statech"))) {
            println!("{place}:");
            for track in found.tracks.iter().filter(|t| t.place == *place) {
                println!("  kept   {} {} {}", track.location, track.size, crc(track.crc));
            }
            for repeat in found.repeats.iter().filter(|r| r.track.place == *place) {
                let track = &repeat.track;
                println!("  copy   {} {} {} = {}", track.location, track.size, crc(track.crc), repeat.of);
                assert!(kept.contains(repeat.of.as_str()), "{} repeats {}, which was not kept", track.location, repeat.of);
            }
        }
        let melodies = found.counts.iter().filter(|(place, _)| place.starts_with("horizonmelodies/")).map(|(_, n)| n).sum::<usize>();
        assert!(melodies > 0, "HorizonMelodies adds no tracks");
        let mut categories: std::collections::BTreeMap<String, usize> = Default::default();
        let mut failed = Vec::new();
        for track in &found.tracks {
            let rule = rules.rule(&track.location).unwrap_or_else(|| panic!("{} has no rule", track.location));
            assert!(!rule.contexts.is_empty(), "{} ({}) plays nowhere", track.location, rule.category);
            *categories.entry(rule.category.clone()).or_default() += 1;
            if let Err(e) = read_track(&track.file, rules.prism.max_bytes) {
                failed.push(format!("{}: {e}", track.location));
            }
        }
        println!("{categories:?}");
        println!("{} tracks kept, {} copies skipped", found.tracks.len(), found.repeats.len());
        assert!(failed.is_empty(), "tracks that do not play:\n{}", failed.join("\n"));
    }
}
