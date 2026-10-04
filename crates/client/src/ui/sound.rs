//! The interface's sounds: a soft click when a button is pressed and a light tap when an
//! item is picked up or put down in the inventory. Both are synthesized at startup, like
//! the block sounds, and follow the sound effects volume.

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::Volume;
use bevy::prelude::*;

use super::widgets::Disabled;
use crate::settings::Settings;
use crate::sfx::{MASTER, RATE, wav};

pub struct UiSoundPlugin;

impl Plugin for UiSoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiSound>().add_systems(Startup, synthesize).add_systems(Update, (click_buttons, play).chain());
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

#[derive(Resource)]
struct Clips {
    click: Handle<AudioSource>,
    item: Handle<AudioSource>,
}

/// A short decaying chord of sine partials over a tick of noise: `(frequency, level)`
/// pairs, a decay time in seconds and the noise's level.
fn tone(partials: &[(f32, f32)], decay: f32, noise: f32, seconds: f32) -> Vec<f32> {
    let frames = (seconds * RATE as f32) as usize;
    let mut seed = 0x9e37_79b9u32;
    (0..frames)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            // A 2 ms attack so the clip never clicks on its own.
            let envelope = (t / 0.002).min(1.0) * (-t / decay).exp();
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let hiss = (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * noise * (-t / 0.004).exp();
            envelope * partials.iter().map(|&(f, level)| (TAU * f * t).sin() * level).sum::<f32>() + hiss
        })
        .collect()
}

fn synthesize(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let mut add = |samples: Vec<f32>| sources.add(AudioSource { bytes: Arc::from(wav(&samples)) });
    // A small wooden knock with a glassy overtone.
    let click = add(tone(&[(620.0, 0.5), (1240.0, 0.25), (2790.0, 0.08)], 0.028, 0.15, 0.12));
    // Softer and lower, like setting down a pebble.
    let item = add(tone(&[(330.0, 0.45), (990.0, 0.12)], 0.035, 0.25, 0.12));
    commands.insert_resource(Clips { click, item });
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
    let mut played = Vec::new();
    for &sound in sounds.read() {
        // One of each a frame, however many buttons or moves asked.
        if played.contains(&sound) || volume <= 0.0 {
            continue;
        }
        played.push(sound);
        let (clip, gain) = match sound {
            UiSound::Click => (&clips.click, 0.45),
            UiSound::Item => (&clips.item, 0.35),
        };
        commands.spawn((AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain * volume))));
    }
}
