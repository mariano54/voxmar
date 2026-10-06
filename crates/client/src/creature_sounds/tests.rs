//! The base package's creature voices, and an offline preview of how they sound.

use std::path::Path;

use super::*;
use crate::sfx::RATE;

fn content() -> voxmar_engine::content::Content {
    voxmar_server::load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap()
}

/// A clip under `assets/` as mono samples at the game's rate.
fn decode(path: &str) -> Vec<f32> {
    let [left, right] = crate::ambience::tests::decode(path);
    left.iter().zip(&right).map(|(l, r)| 0.5 * (l + r)).collect()
}

/// Every species has a voice; every clip is there, decodes, is short and clean, and is
/// small on disk.
#[test]
fn every_species_has_a_voice() {
    let content = content();
    let bank = Bank::load(&content.files, &content.creatures).unwrap();
    for (id, creature) in content.creatures.iter() {
        let voice = bank.voice(id).unwrap_or_else(|| panic!("{} has no voice", creature.def.name));
        // Everything that lives can be heard getting hurt and dying.
        assert!(voice.event(Kind::Hurt).is_some() && voice.event(Kind::Death).is_some(), "{} is mute when struck", creature.def.name);
        assert!(voice.event(Kind::Idle).is_some() || voice.event(Kind::Alert).is_some(), "{} never makes a sound", creature.def.name);
        assert!((0.5..=2.0).contains(&voice.pitch), "{}: pitch {}", creature.def.name, voice.pitch);
    }
    let mut bytes = 0;
    for key in &bank.clips {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets").join(&key.path);
        bytes += std::fs::metadata(&file).unwrap_or_else(|e| panic!("{}: {e}", key.path)).len();
        let samples = decode(&key.path);
        let seconds = samples.len() as f32 / RATE as f32;
        assert!((0.03..=6.0).contains(&seconds), "{}: {seconds} s", key.path);
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0), "{}", key.path);
        assert!(samples.iter().any(|s| s.abs() > 0.05), "{} is near silent", key.path);
        assert!(samples.last().unwrap().abs() < 0.05, "{} ends abruptly", key.path);
    }
    // Compact: a few megabytes for every creature in the game.
    assert!(bytes < 8 << 20, "creature sounds take {} MB", bytes >> 20);
}

/// The supernatural sound otherworldly, and nothing else does unless it says.
#[test]
fn supernatural_species_shimmer() {
    let content = content();
    let bank = Bank::load(&content.files, &content.creatures).unwrap();
    for (id, creature) in content.creatures.iter() {
        let voice = bank.voice(id).unwrap();
        let shimmer = bank.clips[voice.event(Kind::Hurt).unwrap().clips[0]].aether;
        assert_eq!(shimmer > 0, creature.def.supernatural, "{}", creature.def.name);
    }
}

/// A sound asked for while the game is silent is drained and counted, and plays nothing.
#[test]
fn silent_runs_drain_and_play_nothing() {
    let content = content();
    let rabbit = content.creatures.find("rabbit").unwrap();
    let mut app = App::new();
    app.add_plugins(bevy::time::TimePlugin)
        .insert_resource(GlobalVolume::new(Volume::SILENT))
        .insert_resource(Settings::default())
        .insert_resource(Launch { name: String::new(), host: false, content: content.clone() })
        .insert_resource(Voices {
            bank: Bank::load(&content.files, &content.creatures).unwrap(),
            sources: Vec::new(),
            raw: Vec::new(),
            unshimmered: Vec::new(),
            last: FxHashMap::default(),
            counter: 0,
        })
        .init_resource::<CreatureSounds>()
        .init_resource::<Heard>()
        .add_message::<SoundCue>()
        .add_systems(Update, play);
    let sound = CreatureSound { kind: Kind::Hurt, id: 1, species: rabbit, position: Vec3::ZERO, speed: 0.0, scale: 1.0, young: false };
    app.world_mut().resource_mut::<CreatureSounds>().push(sound);
    app.update();
    assert!(app.world().resource::<CreatureSounds>().0.is_empty());
    assert_eq!(app.world().resource::<Heard>().asked[&Kind::Hurt], 1);
    assert_eq!(app.world().resource::<Heard>().played, 0);
    assert_eq!(app.world_mut().query::<&AudioPlayer>().iter(app.world()).count(), 0);
}

/// `cargo test --profile iter -p voxmar_client creature_sound_preview -- --ignored` writes
/// each species' sounds, every event in turn at the species' pitch (aether shimmer
/// included), to `target/creature_sounds/<species>.wav`, and all of them to `all.wav`.
#[test]
#[ignore]
fn creature_sound_preview() {
    let content = content();
    let bank = Bank::load(&content.files, &content.creatures).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/creature_sounds");
    std::fs::create_dir_all(&dir).unwrap();
    let gap = |seconds: f32| vec![0.0f32; (seconds * RATE as f32) as usize];
    let mut all = Vec::new();
    for (id, creature) in content.creatures.iter() {
        let voice = bank.voice(id).unwrap();
        let mut out = Vec::new();
        for kind in Kind::ALL {
            let Some(event) = &voice.events[kind as usize] else { continue };
            for &clip in event.clips.iter().take(3) {
                let key = &bank.clips[clip];
                let mut samples = decode(&key.path);
                if key.aether > 0 {
                    samples = aether::shimmer(&samples, RATE, key.aether as f32 / 10.0, 7);
                }
                // Played at the species' pitch: resampled.
                let step = voice.pitch;
                let played: Vec<f32> = (0..(samples.len() as f32 / step) as usize)
                    .map(|i| samples.get((i as f32 * step) as usize).copied().unwrap_or(0.0) * event.gain * MASTER)
                    .collect();
                out.extend(played);
                out.extend(gap(0.25));
            }
            out.extend(gap(0.5));
        }
        let short = creature.def.name.rsplit(':').next().unwrap();
        std::fs::write(dir.join(format!("{short}.wav")), sfx::wav(&out)).unwrap();
        all.extend(out);
        all.extend(gap(1.0));
    }
    std::fs::write(dir.join("all.wav"), sfx::wav(&all)).unwrap();
    println!("wrote {}", dir.display());
}
