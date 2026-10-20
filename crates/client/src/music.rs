//! Background music, played now and then like Minecraft's: a track starts soon on the title
//! screen, then quiet gaps separate tracks, far longer in a world (`gap` in `music.ron`)
//! than a track, so a world is mostly quiet. Starting a game fades the menu's music out,
//! so a world opens in quiet and its first track comes after a gap like any other;
//! returning to the menus fades the game's music out and the menu's starts again soon. `M`
//! mutes or unmutes. As a track starts, its name and category show small at the top right
//! for a moment (`NowPlayingLine`).
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
//!
//! The home motif (`home` in `music.ron`) is a short cue apart from the rotation: the first
//! time each night the player steps inside their own lit home, a `Home` track plays, the
//! track playing fading for it, and the quiet gap follows it as it follows any track. A
//! boss fight holds it back, and one starting fades it.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, GlobalVolume, PlaybackSettings, Volume};
use bevy::prelude::*;
use crossbeam_channel::{Receiver, TryRecvError, bounded};

use crate::ambience::Surroundings;
use crate::menu::{InMenu, Screen};
use crate::creatures::Creatures;
use crate::lang::Locale;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::mix::Duck;
use crate::settings::Settings;
use crate::sfx::sound_on;
use crate::world::ClientWorld;
use voxmar_engine::block::BlockId;
use voxmar_engine::creature::CreatureId;
use crate::ui::widgets::{BODY, SHADOW, SOFT, UiFont, shadowed, space};

use self::prism::{Found, Place, TrackFile};
use self::rules::{Category, Context, MusicRules, NowPlaying};

pub mod prism;
pub mod rules;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_tracks)
            .add_systems(PostStartup, spawn_now_playing)
            .add_systems(OnEnter(Screen::InGame), start_game)
            .add_systems(OnExit(Screen::InGame), leave_game)
            .add_systems(
                Update,
                (
                    toggle_music,
                    (receive_found, see_fight, home_motif, settlement_motif, schedule_music, start_loaded).chain().run_if(sound_on.or_else(resource_exists::<Rehearse>)),
                    show_now_playing,
                    fade_out,
                    set_volume,
                    stop_switched_off.run_if(resource_changed::<Settings>),
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
    /// Biome tags where it plays, one enough; none for anywhere.
    biomes: Vec<String>,
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

    /// Whether it plays in a biome with these tags.
    fn plays_in(&self, biome: &[String]) -> bool {
        self.biomes.is_empty() || self.biomes.iter().any(|tag| biome.contains(tag))
    }
}

/// Music picked and started in a silent run as in a heard one, for the gate to see a track
/// named as it starts.
#[derive(Resource)]
pub struct Rehearse;

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
    /// The last track started and when, for the line naming it.
    started: Option<(usize, Duration)>,
    /// The track playing (or being read).
    current: Option<usize>,
    /// The species that are bosses, and the blocks of bosses' rooms, each with its boss's
    /// music tags (the creature's `music`). A boss that roams the land rather than keeping
    /// to a room (no `party_only`, Old Tusk) carries its most health: its music waits until
    /// it is in a fight (`Creatures::stirred`), not merely grazing near.
    bosses: Vec<(CreatureId, Vec<String>, Option<u16>)>,
    room_blocks: Vec<(BlockId, Vec<String>)>,
    /// The music tags of the boss whose room was near when last looked, and when that was.
    room: (Option<Vec<String>>, Option<Duration>),
    /// Whether the player was in a boss fight when last scheduled, and that boss's music
    /// tags.
    fighting: bool,
    fight_music: Vec<String>,
    /// The boss fight seen this frame (its music tags), looked for before the motifs and
    /// the rotation, so neither starts on the frame a fight begins.
    fight: Option<Vec<String>>,
    home: AtHome,
    settlement: AtSettlement,
}

/// What the home motif knows: whether the player was inside their lit home when last
/// looked (none before the first look in a world), the night it last played, how many
/// times it has started, and the night of a step in not yet greeted (held back by another
/// motif or a fight, see [`due`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AtHome {
    pub inside: Option<bool>,
    pub night: Option<u64>,
    pub starts: u32,
    pub pending: Option<u64>,
}

/// What the settlement motif knows, as [`AtHome`]: whether the player was within a
/// settlement's bounds when last looked, the game day it last played, how many times it
/// has started and which track (by index) it last started, the day of a step in not yet
/// greeted (as the home's); and the surveys of the surroundings started before the player last
/// joined or came back to life (`Surroundings::started`), which tell nothing of where they
/// are now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AtSettlement {
    pub inside: Option<bool>,
    pub day: Option<u64>,
    pub starts: u32,
    pub track: Option<usize>,
    pub pending: Option<u64>,
    pub stale: u64,
}

impl AtSettlement {
    /// Looks at the settlement as survey `surveyed` found it: whether this is a step into
    /// its bounds. Nothing is looked at until a survey taken since the player joined or
    /// came back to life, and the first look finds them where they are, not stepping in.
    fn step_in(&mut self, surveyed: u64, settlement: Option<(u64, f32)>) -> bool {
        if surveyed <= self.stale {
            return false;
        }
        let inside = within_settlement(settlement);
        self.inside.replace(inside) == Some(false) && inside
    }
}

impl Music {
    /// Fades the playing track out, if there is one, and forgets one being read.
    fn fade_out(&mut self, commands: &mut Commands) {
        self.loading = None;
        self.started = None;
        self.current = None;
        if let Some(entity) = self.playing.take() {
            commands.entity(entity).try_insert(FadingOut(self.rules.fade_out));
        }
    }

    /// Gain of a track playing in full, brought to the others' loudness by `trim`.
    fn level(&self, settings: &Settings, own: bool, trim: f32) -> f32 {
        let borrowed = if own { 1.0 } else { self.rules.borrowed_volume };
        self.rules.volume * borrowed * trim * settings.music_volume as f32 / 100.0
    }

    /// Seconds of quiet before the next track, in the menus or in a world.
    fn gap(&self, time: &Time, in_menu: bool) -> Duration {
        let gap = if in_menu { self.rules.menu_gap } else { self.rules.gap };
        Duration::from_secs_f32(random_in(time, 0, gap))
    }

    /// Plays track `index` from `source`, and names it.
    fn start(&mut self, commands: &mut Commands, time: &Time, settings: &Settings, index: usize, source: Handle<AudioSource>) {
        let track = &self.tracks[index];
        let trim = if track.own() { self.rules.trim(&track.name) } else { 1.0 };
        let level = self.level(settings, track.own(), trim);
        let entity = commands
            .spawn((MusicTrack { own: track.own(), trim, category: track.category.clone() }, AudioPlayer(source), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(level))))
            .id();
        self.playing = Some(entity);
        self.current = Some(index);
        self.started = Some((index, time.elapsed()));
        info!("music: {} ({})", track.name, track.category);
    }
}

/// What a track is called: one of Voxmar's own as the languages say (`music.track.<its
/// file name>`), a borrowed one by the last part of its location, `calm1` as Calm1.
fn title(track: &Track, locale: &Locale) -> String {
    let file = track.name.rsplit(['/', ':']).next().unwrap_or(&track.name);
    let stem = file.rsplit_once('.').filter(|_| track.own()).map_or(file, |(stem, _)| stem);
    let words: Vec<String> = stem
        .split(['_', '-', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
        })
        .collect();
    let words = words.join(" ");
    if track.own() { locale.get_or(&format!("music.track.{stem}"), &words).to_owned() } else { words }
}

/// A track playing, whether it is one of Voxmar's own, and its category.
#[derive(Component)]
struct MusicTrack {
    own: bool,
    /// The gain bringing it to the loudness of the rest (`MusicRules::trim`).
    trim: f32,
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
        let names = own_tracks(&directory, folder);
        counts.push(format!("{} {}", category.name, names.len()));
        tracks.extend(names.into_iter().map(|name| {
            let rule = category.track_rule(&name);
            Track {
                source: Source::Own(assets.load(format!("music/{name}"))),
                contexts: rule.map_or_else(|| category.contexts.clone(), |rule| rule.contexts.clone()),
                weight: rule.map_or(1.0, |rule| rule.weight),
                biomes: rule.map(|rule| rule.biomes.clone()).unwrap_or_default(),
                name,
                category: category.name.clone(),
            }
        }));
    }
    let content = launch.as_ref().map(|launch| &launch.content);
    let bosses = content
        .map(|content| content.creatures.iter().filter(|(_, c)| c.def.boss).map(|(id, c)| (id, c.def.music.clone(), (!c.def.habitat.party_only).then_some(c.def.health))).collect())
        .unwrap_or_default();
    let room_blocks = content
        .map(|content| {
            rules
                .boss
                .rooms
                .iter()
                .filter_map(|room| {
                    let block = content.blocks.id(&room.block).or_else(|| {
                        warn!("music: no block {} for a boss's room", room.block);
                        None
                    })?;
                    let boss = content.creatures.find(&room.boss).or_else(|| {
                        warn!("music: no creature {} for the room of {}", room.boss, room.block);
                        None
                    })?;
                    Some((block, content.creatures.get(boss).def.music.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    info!("{} music tracks ({})", tracks.len(), counts.join(", "));
    commands.insert_resource(MusicCategories(rules.categories.clone()));
    // Looking through a modpack's jars takes a moment; Voxmar's own tracks play meanwhile.
    let found = settings.minecraft_assets.then(|| {
        let (tx, rx) = bounded(1);
        let place = Place { launcher: settings.prism_folder.clone(), instance: settings.prism_instance.clone() };
        let discovery = rules.prism.clone();
        crate::priority::spawn_background("music-search", move || {
            let _ = tx.send(prism::discover(&place, &discovery));
        });
        rx
    });
    commands.insert_resource(Music {
        rules,
        tracks,
        next_at: None,
        playing: None,
        recent: VecDeque::new(),
        enabled: true,
        found,
        loading: None,
        started: None,
        current: None,
        bosses,
        room_blocks,
        room: (None, None),
        fighting: false,
        fight_music: Vec::new(),
        fight: None,
        home: AtHome::default(),
        settlement: AtSettlement::default(),
    });
}

/// The tracks in `folder` of `directory` and the folders in it, by their path from
/// `directory` (`elevenlabs/round5/forest/boss-01.mp3`), in order.
fn own_tracks(directory: &std::path::Path, folder: &str) -> Vec<String> {
    fn walk(directory: &std::path::Path, path: String, names: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory.join(&path)).into_iter().flatten().flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            let inner = format!("{path}/{name}");
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                walk(directory, inner, names);
            } else if name.ends_with(".mp3") || name.ends_with(".ogg") {
                names.push(inner);
            }
        }
    }
    let mut names = Vec::new();
    walk(directory, folder.to_owned(), &mut names);
    names.sort();
    names
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
            biomes: rule.biomes.clone(),
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

/// The track to play next where the player is (`biome`: the tags of where they stand, or in
/// a boss fight the boss's music tags), among those whose category is on: Voxmar's
/// own `own_share` of the time when borrowed tracks suit too, then by weight, passing over
/// the ones played lately while others suit. `rolls` are two numbers in 0..1.
fn pick(tracks: &[Track], context: Context, biome: &[String], settings: &Settings, recent: &VecDeque<usize>, own_share: f32, rolls: [f32; 2]) -> Option<usize> {
    let suits = |context: Context| -> Vec<usize> {
        (0..tracks.len()).filter(|&i| tracks[i].contexts.contains(&context) && tracks[i].on(settings) && tracks[i].plays_in(biome)).collect()
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

/// What the music makes of a boss fight, for the gate: whether it counts the player in
/// one, and the track playing (or being read), if it is a boss's, with whether it is one
/// of this boss's (its tags among the boss's `music`, or none); none when no music runs.
pub(crate) fn boss_music(world: &World) -> Option<(bool, Option<(String, bool)>)> {
    let music = world.get_resource::<Music>()?;
    let track = music.current.map(|index| &music.tracks[index]).filter(|track| track.contexts.contains(&Context::Boss));
    Some((music.fighting, track.map(|track| (track.name.clone(), track.plays_in(&music.fight_music)))))
}

/// The music tags of the boss the player is fighting, if they are in a boss fight: the
/// nearest boss alive near, else the boss whose room's block is near (looked for once a
/// second).
fn in_boss_fight(music: &mut Music, time: &Time, feet: Vec3, creatures: Option<&Creatures>, world: Option<&ClientWorld>) -> Option<Vec<String>> {
    let boss = music.rules.boss.clone();
    let now = time.elapsed_secs();
    let near = creatures.and_then(|creatures| {
        creatures
            .living()
            .filter_map(|(id, species, at)| {
                let (_, tags, roams) = music.bosses.iter().find(|(boss, ..)| *boss == species)?;
                roams.is_none_or(|_| creatures.stirred(id, now, boss.calm_after)).then_some((at.distance(feet), tags))
            })
            .filter(|(distance, _)| *distance <= boss.within)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, tags)| tags.clone())
    });
    if near.is_some() {
        return near;
    }
    let (Some(world), false) = (world, music.room_blocks.is_empty()) else { return None };
    let due = music.room.1.is_none_or(|at| time.elapsed() >= at + Duration::from_secs(1));
    if due {
        let centre = feet.floor().as_ivec3();
        let reach = boss.room_reach.max(0);
        let mut found = None;
        'look: for dy in -reach..=reach {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    if let Some((_, tags)) = world.block(centre + IVec3::new(dx, dy, dz)).and_then(|block| music.room_blocks.iter().find(|(b, _)| *b == block)) {
                        found = Some(tags.clone());
                        break 'look;
                    }
                }
            }
        }
        music.room = (found, Some(time.elapsed()));
    }
    music.room.0.clone()
}

/// Looks for a boss fight (see [`in_boss_fight`]) before the motifs and the rotation.
fn see_fight(time: Res<Time>, player: Res<LocalPlayer>, menu: Option<Res<State<InMenu>>>, creatures: Option<Res<Creatures>>, world: Option<Res<ClientWorld>>, mut music: ResMut<Music>) {
    let fight = if menu.is_some() || !player.joined { None } else { in_boss_fight(&mut music, &time, player.position, creatures.as_deref(), world.as_deref()) };
    music.fight = fight;
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
    let fight = if in_menu { None } else { music.fight.clone() };
    let boss = fight.is_some();
    // A boss's music is its own wherever it is met: its tags choose the track, not the
    // biome's.
    if let Some(tags) = fight {
        music.fight_music = tags;
    }
    let underground = &music.rules.underground;
    let context = match &surroundings {
        _ if in_menu => Context::Menu,
        _ if boss => Context::Boss,
        Some(around) if around.depth >= underground.depth && around.sky <= underground.max_sky => Context::Underground,
        _ => Context::Game,
    };
    let soon = time.elapsed() + Duration::from_secs_f32(music.rules.resume_after);
    // A finished track despawns itself; schedule the next one after a quiet gap, or soon
    // in a boss fight.
    if let Some(entity) = music.playing
        && playing.get(entity).is_err()
    {
        music.playing = None;
        music.current = None;
        music.next_at = Some(if boss { soon } else { time.elapsed() + music.gap(&time, in_menu) });
    }
    // A boss fight starting ends the calm track playing, and its music comes once that has
    // faded, even in a quiet gap; one ending ends the boss's track, and quiet follows.
    if boss != music.fighting {
        music.fighting = boss;
        if let Some(index) = music.current
            && boss != music.tracks[index].contexts.contains(&Context::Boss)
        {
            music.fade_out(&mut commands);
        }
        music.next_at = Some(if boss { soon } else { time.elapsed() + music.gap(&time, in_menu) });
    }
    if music.playing.is_some() || music.loading.is_some() {
        return;
    }
    let first = music.rules.first_menu_delay;
    let next_at = *music.next_at.get_or_insert(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, first)));
    if time.elapsed() < next_at {
        return;
    }
    let biome = if boss { music.fight_music.as_slice() } else { surroundings.as_ref().map_or(&[][..], |around| around.biome.as_slice()) };
    let rolls = [random_in(&time, 1, (0.0, 1.0)), random_in(&time, 2, (0.0, 1.0))];
    let Some(index) = pick(&music.tracks, context, biome, &settings, &music.recent, music.rules.own_share, rolls) else {
        // Nothing suits here; look again after a gap.
        music.next_at = Some(time.elapsed() + music.gap(&time, in_menu));
        return;
    };
    music.recent.push_back(index);
    while music.recent.len() > music.rules.recent.max(1) {
        music.recent.pop_front();
    }
    match music.tracks[index].source {
        Source::Own(ref handle) => {
            let handle = handle.clone();
            music.start(&mut commands, &time, &settings, index, handle);
        }
        Source::Borrowed(ref file) => {
            // Read off the main thread: a track is several megabytes, perhaps out of a big zip.
            let (tx, rx) = bounded(1);
            let (file, max_bytes) = (file.clone(), music.rules.prism.max_bytes);
            crate::priority::spawn_background("music-read", move || {
                let _ = tx.send(read_track(&file, max_bytes));
            });
            music.loading = Some((index, rx));
            music.current = Some(index);
        }
    }
}

/// Whether a player standing at `feet` is inside their lit home: `home` (`Vitals::home`) is a
/// burning hearth (`fires`), within `reach` blocks each way, with a roof block within
/// `roof` blocks over the head.
fn inside_home(rules: &rules::HomeMotif, feet: Vec3, home: Option<IVec3>, fires: &rustc_hash::FxHashSet<IVec3>, block: impl Fn(IVec3) -> Option<BlockId>, is_roof: impl Fn(BlockId) -> bool) -> bool {
    let Some(home) = home.filter(|home| fires.contains(home)) else { return false };
    let off = (feet.floor().as_ivec3() - home).abs();
    off.max_element() <= rules.reach && crate::ambience::roof_over(block, feet, feet, rules.roof, is_roof).is_some()
}

/// Whether the home motif may look yet: not before the server has said the player's state
/// (the home comes with it, after the welcome), nor before the ground under the player and
/// the hearth have loaded (`loaded`). A home or a roof not yet known is not a step out.
fn home_known(player: &LocalPlayer, loaded: impl Fn(Vec3) -> bool) -> bool {
    let half = voxmar_engine::physics::player::WIDTH / 2.0;
    let corners = [Vec3::new(-half, 0.0, -half), Vec3::new(half, 0.0, -half), Vec3::new(-half, 0.0, half), Vec3::new(half, 0.0, half)];
    player.states > 0 && corners.iter().all(|&c| loaded(player.position + c)) && player.vitals.home.is_none_or(|home| loaded(home.as_vec3()))
}

/// The home motif: on stepping inside the lit home at night, the first time that night and
/// outside a boss fight, a `Home` track starts, the one playing fading for it. A step in
/// held back (by a fight, or the settlement motif still playing) is greeted once it clears,
/// while the player is still in and it is still that night ([`due`]).
#[allow(clippy::too_many_arguments)]
fn home_motif(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    menu: Option<Res<State<InMenu>>>,
    settings: Res<Settings>,
    cycle: Option<Res<crate::render::DayCycle>>,
    world: Option<Res<ClientWorld>>,
    mut music: ResMut<Music>,
) {
    let (Some(world), Some(cycle)) = (world, cycle) else { return };
    if menu.is_some() || !player.joined || player.dead {
        music.home.inside = None;
        music.home.pending = None;
        return;
    }
    let feet = player.position;
    if !home_known(&player, |at| world.voxels.contains(voxmar_engine::coords::ColumnPos::of_block(at.floor().as_ivec3()))) {
        return;
    }
    let inside = inside_home(&music.rules.home, feet, player.vitals.home, &world.fires, |p| world.voxels.block(p), |b| crate::ambience::defs::is_roof(&world.registry, b));
    // Only a step in: the first look in a world, or after dying, finds the player where
    // they are.
    let stepped = music.home.inside.replace(inside) == Some(false) && inside;
    let night = music.rules.home.night(cycle.ticks.max(0.0) as u64);
    let blocked = music.fighting || music.fight.is_some() || motif_playing(&music) || !music.enabled || settings.music_volume == 0;
    let last = music.home.night;
    let Some(night) = due(&mut music.home.pending, last, stepped, inside, night, blocked) else { return };
    let rolls = [random_in(&time, 3, (0.0, 1.0)), random_in(&time, 4, (0.0, 1.0))];
    // Picked as others are, but kept out of the rotation's memory of tracks played lately.
    let Some(index) = pick(&music.tracks, Context::Home, &[], &settings, &VecDeque::new(), music.rules.own_share, rolls) else { return };
    // Only own tracks are `Home`'s (`MusicRules::load`).
    let Source::Own(ref handle) = music.tracks[index].source else { return };
    let handle = handle.clone();
    music.home.night = Some(night);
    music.home.starts += 1;
    music.fade_out(&mut commands);
    music.start(&mut commands, &time, &settings, index, handle);
}

/// Which night (or day) a motif greets a step in now, if it does: `now` is the night it is
/// (none by day; for a settlement, the day), `last` the one it last played. A step in
/// is greeted once that night; one held back (`blocked`: another motif playing, a fight,
/// music off) waits while the player stays in, and is greeted once nothing holds it back,
/// the same night. Stepping out, or the night ending, forgets it. `pending` holds the night
/// of a step in not yet greeted.
fn due(pending: &mut Option<u64>, last: Option<u64>, stepped: bool, inside: bool, now: Option<u64>, blocked: bool) -> Option<u64> {
    if stepped {
        *pending = now;
    }
    if !inside || now.is_none() || *pending != now || last == now {
        *pending = None;
        return None;
    }
    if blocked {
        return None;
    }
    pending.take()
}

/// Whether a motif (the home's or a settlement's) is the track playing: another holds
/// back for it, so they never play over each other.
fn motif_playing(music: &Music) -> bool {
    music.current.is_some_and(|index| music.tracks[index].contexts.iter().any(|c| Context::MOTIFS.contains(c)))
}

/// The settlement motif: on stepping within a settlement's bounds by day (the home
/// motif's day), the first time that game day, outside a boss fight and not over the home
/// motif, a `Settlement` track starts, the one playing fading for it. Where the bounds are
/// is the ambience's survey of the world (`Surroundings::settlement`).
#[allow(clippy::too_many_arguments)]
fn settlement_motif(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    menu: Option<Res<State<InMenu>>>,
    settings: Res<Settings>,
    cycle: Option<Res<crate::render::DayCycle>>,
    surroundings: Option<Res<crate::ambience::Surroundings>>,
    mut music: ResMut<Music>,
) {
    let (Some(cycle), Some(surroundings)) = (cycle, surroundings) else { return };
    if menu.is_some() || !player.joined || player.dead {
        music.settlement.inside = None;
        music.settlement.pending = None;
        music.settlement.stale = surroundings.started;
        return;
    }
    // Only a step in: the first look in a world, or after dying, finds the player where
    // they are, once a survey of there has come.
    let stepped = music.settlement.step_in(surroundings.surveyed, surroundings.settlement);
    let inside = music.settlement.inside == Some(true);
    let day = music.rules.home.day(cycle.ticks.max(0.0) as u64);
    let blocked = music.fighting || music.fight.is_some() || motif_playing(&music) || !music.enabled || settings.music_volume == 0;
    let last = music.settlement.day;
    let Some(day) = due(&mut music.settlement.pending, last, stepped, inside, day, blocked) else { return };
    let rolls = [random_in(&time, 5, (0.0, 1.0)), random_in(&time, 6, (0.0, 1.0))];
    let Some(index) = pick(&music.tracks, Context::Settlement, &[], &settings, &VecDeque::new(), music.rules.own_share, rolls) else { return };
    // Only own tracks are `Settlement`'s (`MusicRules::load`).
    let Source::Own(ref handle) = music.tracks[index].source else { return };
    let handle = handle.clone();
    music.settlement.day = Some(day);
    music.settlement.starts += 1;
    music.settlement.track = Some(index);
    music.fade_out(&mut commands);
    music.start(&mut commands, &time, &settings, index, handle);
}

/// Whether the player is within the nearest settlement's bounds (`settlement`: its id and
/// how far outside them, from the ambience's survey).
fn within_settlement(settlement: Option<(u64, f32)>) -> bool {
    settlement.is_some_and(|(_, edge)| edge <= 0.0)
}

/// What the gate sees of the home motif: what it knows, the night it is now by its rules
/// (none by day), and the track playing if it is the motif's.
pub(crate) struct HomeHeard {
    pub at: AtHome,
    pub tonight: Option<u64>,
    pub playing: Option<String>,
}

pub(crate) fn home_heard(world: &World) -> Option<HomeHeard> {
    let music = world.get_resource::<Music>()?;
    let ticks = world.get_resource::<crate::render::DayCycle>()?.ticks.max(0.0) as u64;
    let track = music.current.map(|index| &music.tracks[index]).filter(|track| track.contexts.contains(&Context::Home));
    Some(HomeHeard { at: music.home, tonight: music.rules.home.night(ticks), playing: track.map(|track| track.name.clone()) })
}

/// What the gate sees of the settlement motif: what it knows, and the name of the track it
/// last started (kept, as a muted run drops a track the frame it starts).
pub(crate) struct SettlementHeard {
    pub at: AtSettlement,
    pub started: Option<String>,
}

pub(crate) fn settlement_motif_heard(world: &World) -> Option<SettlementHeard> {
    let music = world.get_resource::<Music>()?;
    let started = music.settlement.track.map(|index| music.tracks[index].name.clone());
    Some(SettlementHeard { at: music.settlement, started })
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
    music.current = None;
    match read {
        Ok(source) => {
            let source = sources.add(source);
            music.start(&mut commands, &time, &settings, index, source);
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
    music.next_at = Some(time.elapsed() + music.gap(&time, false));
    music.fighting = false;
    // Another world counts its nights afresh.
    (music.home.inside, music.home.night) = (None, None);
    (music.settlement.inside, music.settlement.day) = (None, None);
}

/// Back in the menus, the game's music fades out and the menu's starts again soon.
fn leave_game(mut commands: Commands, time: Res<Time>, music: Option<ResMut<Music>>) {
    let Some(mut music) = music else { return };
    music.fade_out(&mut commands);
    music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, 0, music.rules.first_menu_delay)));
    music.fighting = false;
}

fn fade_out(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    global: Res<GlobalVolume>,
    music: Option<Res<Music>>,
    mut fading: Query<(Entity, &MusicTrack, &mut FadingOut, Option<&mut AudioSink>)>,
    duck: Option<Res<Duck>>,
) {
    let duck = duck.map_or(1.0, |d| d.0);
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
        sink.set_volume(Volume::Linear(music.level(&settings, track.own, track.trim) * left * duck) * global.volume);
    }
}

/// Keeps the track playing at the volume the settings ask, dipped while the mix makes room
/// for a jingle or a boss (`mix::Duck`).
fn set_volume(
    settings: Res<Settings>,
    global: Res<GlobalVolume>,
    music: Option<Res<Music>>,
    duck: Option<Res<Duck>>,
    mut playing: Query<(&MusicTrack, &mut AudioSink), Without<FadingOut>>,
) {
    let Some(music) = music else { return };
    let duck = duck.map_or(1.0, |d| d.0);
    for (track, mut sink) in &mut playing {
        sink.set_volume(Volume::Linear(music.level(&settings, track.own, track.trim) * duck) * global.volume);
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

/// The line at the top right naming the track that has just started, and its category.
#[derive(Component)]
pub struct NowPlayingLine;

fn spawn_now_playing(mut commands: Commands, font: Res<UiFont>) {
    // Above the pinned quests, clear of the look-at plate at the top left.
    let corner = Node { position_type: PositionType::Absolute, right: Val::Px(space::SECTION), top: Val::Px(space::SECTION), ..default() };
    commands.spawn((corner, GlobalZIndex(1))).with_child((NowPlayingLine, shadowed(&font.0, "", BODY, SOFT), Visibility::Hidden));
}

/// Shows the line for `now_playing.shown` seconds after a track starts, fading in and out,
/// in the player's language.
fn show_now_playing(
    time: Res<Time>,
    locale: Res<Locale>,
    music: Option<Res<Music>>,
    mut line: Query<(&mut Text, &mut TextColor, &mut TextShadow, &mut Visibility), With<NowPlayingLine>>,
) {
    let Some(music) = music else { return };
    let Ok((mut text, mut color, mut shadow, mut visibility)) = line.single_mut() else { return };
    let NowPlaying { shown, fade } = music.rules.now_playing;
    let showing = music.started.map(|(index, at)| (index, (time.elapsed() - at).as_secs_f32())).filter(|(_, age)| *age < shown);
    let Some((index, age)) = showing else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let track = &music.tracks[index];
    let words = locale.format("music.now_playing", &[("title", &title(track, &locale)), ("category", &locale.get(&format!("music.{}", track.category)))]);
    if text.0 != words {
        text.0 = words;
    }
    let alpha = (age / fade.max(f32::EPSILON)).min((shown - age) / fade.max(f32::EPSILON)).clamp(0.0, 1.0);
    color.set_if_neq(TextColor(SOFT.with_alpha(alpha)));
    shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
    visibility.set_if_neq(Visibility::Inherited);
}

/// For the gate: the line naming the track playing, while it shows.
pub fn now_playing_shown(world: &mut World) -> Option<String> {
    let mut line = world.query_filtered::<(&Text, &Visibility), With<NowPlayingLine>>();
    line.iter(world).find(|(_, visibility)| **visibility != Visibility::Hidden).map(|(text, _)| text.0.clone())
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
    music.started = None;
    music.current = None;
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
        Track { source, contexts: contexts.to_vec(), weight, name: String::new(), category, biomes: Vec::new() }
    }

    /// Inside a home is by its burning hearth, within reach and under a roof: not by a cold
    /// one, nor out in the yard, nor in the open beside it.
    #[test]
    fn inside_a_home_is_by_its_lit_hearth_under_a_roof() {
        let rules = rules::HomeMotif { reach: 7, roof: 12, dusk: 16500, dawn: 23500 };
        let (hearth, roof) = (IVec3::new(0, 10, 0), BlockId(5));
        // A roof four blocks over the floor, over the room's nine by nine.
        let block = |p: IVec3| Some(if p.y == 14 && p.x.abs() <= 4 && p.z.abs() <= 4 { roof } else { BlockId::AIR });
        let is_roof = |b: BlockId| b == roof;
        let lit: rustc_hash::FxHashSet<IVec3> = [hearth].into_iter().collect();
        let cold = rustc_hash::FxHashSet::default();
        let at = |x: f32, z: f32| Vec3::new(x + 0.5, 10.0, z + 0.5);
        assert!(inside_home(&rules, at(2.0, 0.0), Some(hearth), &lit, block, is_roof));
        assert!(!inside_home(&rules, at(2.0, 0.0), Some(hearth), &cold, block, is_roof), "a cold hearth is no lit home");
        assert!(!inside_home(&rules, at(2.0, 0.0), None, &lit, block, is_roof), "no home");
        assert!(!inside_home(&rules, at(6.0, 0.0), Some(hearth), &lit, block, is_roof), "in the open beside it");
        let wide = |p: IVec3| Some(if p.y == 14 { roof } else { BlockId::AIR });
        assert!(inside_home(&rules, at(7.0, -7.0), Some(hearth), &lit, wide, is_roof));
        assert!(!inside_home(&rules, at(8.0, 0.0), Some(hearth), &lit, wide, is_roof), "past its reach, under another roof");
    }

    /// The settlement motif's step in is into a settlement's bounds as the ambience last
    /// surveyed them: none near, or outside them, is not in.
    #[test]
    fn the_settlement_motif_is_within_the_bounds() {
        assert!(!within_settlement(None));
        assert!(!within_settlement(Some((7, 3.0))), "three blocks outside");
        assert!(within_settlement(Some((7, 0.0))));
    }

    /// A step in held back by a motif still playing (or a fight) is greeted once that
    /// clears, while the player stays in and only the same night (or day); stepping out,
    /// or the night ending, forgets it, and a night greeted is not greeted again.
    #[test]
    fn a_step_in_held_back_is_greeted_once_it_clears() {
        let mut pending = None;
        // Walking in at dusk while the market day's motif still plays: held, not lost.
        assert_eq!(due(&mut pending, None, true, true, Some(3), true), None);
        assert_eq!(due(&mut pending, None, false, true, Some(3), true), None, "still playing");
        assert_eq!(due(&mut pending, None, false, true, Some(3), false), Some(3), "the motif ended: greeted");
        assert_eq!(pending, None);
        assert_eq!(due(&mut pending, Some(3), false, true, Some(3), false), None, "greeted once");
        // A step in unblocked is greeted at once; one into a night already greeted is not.
        assert_eq!(due(&mut pending, Some(2), true, true, Some(3), false), Some(3));
        assert_eq!(due(&mut pending, Some(3), true, true, Some(3), false), None);
        // Held, then stepping out forgets it.
        assert_eq!(due(&mut pending, None, true, true, Some(4), true), None);
        assert_eq!(due(&mut pending, None, false, false, Some(4), false), None);
        assert_eq!(due(&mut pending, None, false, true, Some(4), false), None, "back in without a step in");
        // Held until the night ended: not greeted by day, nor the next night.
        assert_eq!(due(&mut pending, None, true, true, Some(5), true), None);
        assert_eq!(due(&mut pending, None, false, true, None, false), None, "day came");
        assert_eq!(due(&mut pending, None, false, true, Some(6), false), None, "the next night");
        // A step in by day is no night's.
        assert_eq!(due(&mut pending, None, true, true, None, false), None);
        assert_eq!(due(&mut pending, None, false, true, Some(7), false), None);
        // Staying in without stepping in never greets.
        assert_eq!(due(&mut pending, None, false, true, Some(8), false), None);
    }

    /// Joining, or coming back to life, inside a settlement is no step in: before a survey
    /// taken since then nothing is looked at (the last one was of somewhere else, or none
    /// is yet), and the first that comes finds the player where they are. Walking out and
    /// back in is one.
    #[test]
    fn joining_inside_a_settlement_is_no_step_in() {
        let (inside, outside) = (Some((7, 0.0)), Some((7, 12.0)));
        // Joined: surveys up to 4 were started before.
        let mut at = AtSettlement { stale: 4, ..Default::default() };
        assert!(!at.step_in(0, None), "no survey yet");
        assert!(!at.step_in(4, outside), "a survey from before joining");
        assert_eq!(at.inside, None, "nothing looked at yet");
        assert!(!at.step_in(5, inside), "the first survey since finds the player inside");
        assert!(!at.step_in(6, outside));
        assert!(at.step_in(7, inside), "walking back in");
        // Died outside, came back to life inside: the survey from before dying is stale.
        (at.inside, at.stale) = (None, 7);
        assert!(!at.step_in(7, outside));
        assert!(!at.step_in(8, inside), "coming back to life inside");
    }

    /// The motif waits for the server's word on the player (which brings the home) and for
    /// the ground under them and under the hearth.
    #[test]
    fn the_home_motif_waits_for_the_home_and_its_ground() {
        let mut player = LocalPlayer::default();
        player.position = Vec3::new(0.5, 10.0, 0.5);
        let everywhere = |_: Vec3| true;
        assert!(!home_known(&player, everywhere), "before the first PlayerState the home is not known");
        player.states = 1;
        assert!(home_known(&player, everywhere));
        player.vitals.home = Some(IVec3::new(40, 10, 0));
        assert!(!home_known(&player, |at: Vec3| at.x < 20.0), "the hearth's column has not loaded");
        assert!(!home_known(&player, |at: Vec3| at.x > 0.5), "part of the ground underfoot has not loaded");
        assert!(home_known(&player, everywhere));
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
        assert_eq!(pick(&tracks, Menu, &[], &all, &none, 0.25, [0.1, 0.5]), Some(0));
        assert_eq!(pick(&tracks, Menu, &[], &all, &none, 0.25, [0.9, 0.5]), Some(1));
        // Underground, by weight: one in four the world's track, else the deep one.
        assert_eq!(pick(&tracks, Underground, &[], &all, &none, 0.25, [0.9, 0.2]), Some(2));
        assert_eq!(pick(&tracks, Underground, &[], &all, &none, 0.25, [0.9, 0.3]), Some(3));
        // Never the deep track above ground.
        for roll in [0.0, 0.5, 0.99] {
            assert_eq!(pick(&tracks, Game, &[], &all, &none, 0.25, [0.9, roll]), Some(2));
        }
        // Played lately is passed over while others suit, but a lone track still plays,
        // and only the very last is avoided when every one was played lately.
        let lately = VecDeque::from([3]);
        assert_eq!(pick(&tracks, Underground, &[], &all, &lately, 0.25, [0.9, 0.9]), Some(2));
        assert_eq!(pick(&tracks, Game, &[], &all, &VecDeque::from([2]), 0.25, [0.9, 0.5]), Some(2));
        assert_eq!(pick(&tracks, Underground, &[], &all, &VecDeque::from([3, 2]), 0.25, [0.9, 0.0]), Some(3));
        // Nothing borrowed: always Voxmar's own; nothing at all: none.
        assert_eq!(pick(&tracks[..1], Menu, &[], &all, &none, 0.0, [0.9, 0.9]), Some(0));
        assert_eq!(pick(&[], Game, &[], &all, &none, 0.25, [0.5, 0.5]), None);
        // Underground with nothing for it falls back to the world's tracks.
        assert_eq!(pick(&[track(false, &[Game], 1.0)], Underground, &[], &all, &none, 0.25, [0.5, 0.5]), Some(0));
        // A category switched off is never picked; with Voxmar's own off, the borrowed play
        // whatever the share, and with everything off, nothing does.
        let ours_off = Settings { music_off: vec!["ours".into()], ..Settings::default() };
        for roll in [0.0, 0.5, 0.99] {
            assert_eq!(pick(&tracks, Menu, &[], &ours_off, &none, 0.25, [roll, roll]), Some(1));
        }
        let all_off = Settings { music_off: vec!["ours".into(), "borrowed".into()], ..Settings::default() };
        assert_eq!(pick(&tracks, Game, &[], &all_off, &none, 0.25, [0.5, 0.5]), None);
    }

    /// A track for some biomes plays only in them; a boss's only in a boss fight, and only
    /// for a boss whose music names its tags (or, untagged, for every boss).
    #[test]
    fn biomes_and_boss_fights_choose_among_tracks() {
        use Context::*;
        let all = Settings::default();
        let none = VecDeque::new();
        let place = |contexts: &[Context], biomes: &[&str]| Track { biomes: biomes.iter().map(|b| b.to_string()).collect(), ..track(true, contexts, 1.0) };
        let tracks = [
            place(&[Menu, Game], &[]),
            place(&[Game], &["forest"]),
            place(&[Boss], &["forest"]),
            place(&[Boss], &["frozen", "cold"]),
        ];
        let forest = ["forest".to_owned(), "temperate".to_owned()];
        let desert = ["desert".to_owned(), "hot".to_owned()];
        // In the forest both calm tracks may play; in the desert only the general one.
        assert_eq!(pick(&tracks, Game, &forest, &all, &none, 0.0, [0.5, 0.9]), Some(1));
        for roll in [0.0, 0.5, 0.99] {
            assert_eq!(pick(&tracks, Game, &desert, &all, &none, 0.0, [0.5, roll]), Some(0));
            // Boss tracks never play outside a fight, and calm ones never in one.
            assert!(pick(&tracks, Game, &forest, &all, &none, 0.0, [0.5, roll]).is_some_and(|i| i < 2));
            // In a fight the boss's music tags choose: a forest boss's track for a forest
            // boss, wherever the fight is.
            assert_eq!(pick(&tracks, Boss, &["forest".to_owned()], &all, &none, 0.0, [0.5, roll]), Some(2));
            // A desert boss has no track of its own: never another's, only the untagged
            // boss tracks, every boss's, if there are any.
            assert_eq!(pick(&tracks, Boss, &desert, &all, &none, 0.0, [0.5, roll]), None);
            let with_any = [place(&[Menu, Game], &[]), place(&[Game], &["forest"]), place(&[Boss], &["forest"]), place(&[Boss], &["frozen", "cold"]), place(&[Boss], &[])];
            assert_eq!(pick(&with_any, Boss, &desert, &all, &none, 0.0, [0.5, roll]), Some(4));
        }
        assert_eq!(pick(&tracks[..2], Boss, &forest, &all, &none, 0.0, [0.5, 0.5]), None);
    }

    /// Every one of Voxmar's own tracks is measured in `music.ron` as it is (a test that
    /// finds one far off says what to write), and brought by it to within the music's
    /// loudness band.
    #[test]
    fn own_tracks_are_measured_and_heard_within_their_band() {
        use crate::mix::{Category, db, tests::{all, within}};
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let rules = MusicRules::load(&content.files).unwrap();
        let own: Vec<String> = rules.categories.iter().filter_map(|c| c.folder.as_ref()).flat_map(|folder| own_tracks(&assets.join("music"), folder)).collect();
        let measured: Vec<(String, f32)> = std::thread::scope(|scope| {
            let jobs: Vec<_> = own
                .iter()
                .map(|name| {
                    scope.spawn(move || {
                        use bevy::audio::{AudioSource, Decodable};
                        let bytes = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/music").join(name)).unwrap();
                        let samples: Vec<f32> = AudioSource { bytes: bytes.into() }.decoder().collect();
                        (name.clone(), crate::sfx::mean_loudness(&samples))
                    })
                })
                .collect();
            jobs.into_iter().map(|job| job.join().unwrap()).collect()
        });
        let stale: Vec<String> = measured
            .iter()
            .filter(|(name, level)| rules.levels.get(name).is_none_or(|known| (known - db(*level)).abs() > 1.0))
            .map(|(name, level)| format!("        \"{name}\": {:.1},", db(*level)))
            .collect();
        let heard = measured.iter().map(|(name, level)| (name.clone(), level * rules.volume * rules.trim(name)));
        let band = within(Category::Music, heard);
        let levels = if stale.is_empty() { Ok(()) } else { Err(format!("music.ron's levels miss or are off for:\n{}", stale.join("\n"))) };
        all([levels, band]);
    }

    /// Every boss has a fight track of its own among Voxmar's tracks: one whose rule is a
    /// boss track's with a tag of the boss's `music`, or an untagged boss track.
    #[test]
    fn every_boss_has_a_fight_track_of_its_own() {
        fn files(dir: &std::path::Path, under: &std::path::Path, into: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files(&path, under, into);
                } else if path.extension().is_some_and(|e| e == "mp3" || e == "ogg") {
                    into.push(path.strip_prefix(under).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let rules = MusicRules::load(&content.files).unwrap();
        let music = assets.join("music");
        let mut own = Vec::new();
        files(&music, &music, &mut own);
        let bosses: Vec<_> = content.creatures.iter().filter(|(_, c)| c.def.boss).collect();
        assert!(!bosses.is_empty());
        for (_, boss) in bosses {
            let theirs = own.iter().find(|path| {
                rules.categories.iter().filter(|c| c.folder.as_ref().is_some_and(|f| path.starts_with(&format!("{f}/")))).any(|c| {
                    c.track_rule(path).is_some_and(|rule| rule.contexts.contains(&Context::Boss) && (rule.biomes.is_empty() || rule.biomes.iter().any(|t| boss.def.music.contains(t))))
                })
            });
            assert!(theirs.is_some(), "no fight track for {} (music {:?})", boss.def.name, boss.def.music);
        }
    }

    /// Voxmar's own tracks are named by the languages, each of them, and a borrowed one by
    /// the last part of its location.
    #[test]
    fn tracks_are_named_by_the_languages_or_their_location() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let rules = MusicRules::load(&content.files).unwrap();
        let english = Locale::load(&assets.join("content"), "en");
        let named = |name: &str, own: bool| title(&Track { name: name.into(), ..track(own, &[], 1.0) }, &english);
        assert_eq!(named("suno/moonlit_quest_2.mp3", true), "Moonlit Quest 2");
        assert_eq!(named("lyria/rain_on_stone.mp3", true), "Rain on Stone");
        assert_eq!(named("zmod/a_new-song.ogg", true), "A New Song");
        assert_eq!(named("minecraft:music/game/calm1", false), "Calm1");
        assert_eq!(named("horizonmelodies:music_and_melody/music/overworld/wild_hills", false), "Wild Hills");
        for folder in rules.categories.iter().filter_map(|c| c.folder.as_ref()) {
            for name in own_tracks(&assets.join("music"), folder) {
                let stem = name.rsplit('/').next().unwrap().rsplit_once('.').unwrap().0;
                assert!(english.find(&format!("music.track.{stem}")).is_some(), "{name} has no music.track.{stem}");
            }
        }
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
