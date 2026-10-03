//! Background music, played now and then like Minecraft's: a track starts some time after
//! joining, then long quiet gaps separate tracks. `M` mutes or unmutes.
//!
//! Tracks are the files in `assets/music` (generated offline by `tools/generate_music.py`).

use std::time::Duration;

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;

use crate::player::{LocalPlayer, UiState};

const VOLUME: f32 = 0.35;
const FIRST_TRACK_DELAY: (f32, f32) = (20.0, 60.0);
const GAP_BETWEEN_TRACKS: (f32, f32) = (180.0, 420.0);

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_tracks).add_systems(Update, (toggle_music, schedule_music));
    }
}

#[derive(Resource)]
struct Music {
    tracks: Vec<Handle<AudioSource>>,
    next_at: Option<Duration>,
    playing: Option<Entity>,
    last: Option<usize>,
    enabled: bool,
}

#[derive(Component)]
struct MusicTrack;

fn load_tracks(mut commands: Commands, assets: Res<AssetServer>) {
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
    let tracks = names.into_iter().map(|name| assets.load(format!("music/{name}"))).collect();
    commands.insert_resource(Music { tracks, next_at: None, playing: None, last: None, enabled: true });
}

/// Cheap varied number in `range` from the clock; music timing needs no determinism.
fn random_in(time: &Time, (low, high): (f32, f32)) -> f32 {
    let nanos = time.elapsed().as_nanos() as u64;
    let mixed = nanos.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
    low + (high - low) * (mixed as f32 / (1u64 << 24) as f32)
}

fn schedule_music(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    mut music: ResMut<Music>,
    tracks: Query<(), With<MusicTrack>>,
) {
    if !player.joined || music.tracks.is_empty() || !music.enabled {
        return;
    }
    // A finished track despawns itself; schedule the next one after a quiet gap.
    if let Some(entity) = music.playing
        && tracks.get(entity).is_err()
    {
        music.playing = None;
        music.next_at = Some(time.elapsed() + Duration::from_secs_f32(random_in(&time, GAP_BETWEEN_TRACKS)));
    }
    if music.playing.is_some() {
        return;
    }
    let next_at = *music.next_at.get_or_insert(time.elapsed() + Duration::from_secs_f32(random_in(&time, FIRST_TRACK_DELAY)));
    if time.elapsed() < next_at {
        return;
    }
    // Avoid repeating the previous track.
    let count = music.tracks.len();
    let mut index = (random_in(&time, (0.0, count as f32)) as usize).min(count - 1);
    if Some(index) == music.last && count > 1 {
        index = (index + 1) % count;
    }
    music.last = Some(index);
    let entity = commands
        .spawn((
            MusicTrack,
            AudioPlayer(music.tracks[index].clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(VOLUME)),
        ))
        .id();
    music.playing = Some(entity);
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
    if let Some(entity) = music.playing.take() {
        commands.entity(entity).despawn();
    }
    // When turned back on, start a track soon rather than after a long gap.
    music.next_at = Some(time.elapsed() + Duration::from_secs(2));
}
