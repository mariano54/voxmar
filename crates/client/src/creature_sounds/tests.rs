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

/// Every species has a voice, one that warns with a warning of its own; every clip is
/// there, decodes, is short and clean, and is small on disk.
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
        // One that warns players off (`creature::Warn`) has a warning of its own: a growl, a
        // roar, a hiss, not just its alarm.
        assert!(creature.def.warn.is_none() || voice.event(Kind::Warn).is_some(), "{} warns with no `warn` sound", creature.def.name);
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
    let sound = CreatureSound { kind: Kind::Hurt, id: 1, species: rabbit, position: Vec3::ZERO, speed: 0.0, scale: 1.0, young: false, answering: false };
    app.world_mut().resource_mut::<CreatureSounds>().push(sound);
    app.update();
    assert!(app.world().resource::<CreatureSounds>().0.is_empty());
    assert_eq!(app.world().resource::<Heard>().asked[&Kind::Hurt], 1);
    assert_eq!(app.world().resource::<Heard>().played, 0);
    assert_eq!(app.world_mut().query::<&AudioPlayer>().iter(app.world()).count(), 0);
}

/// A munch heard by chance when a ewe grazes unbidden is always heard when she eats from
/// the player's hand.
#[test]
fn an_answer_to_the_hand_is_never_thinned() {
    assert!(super::thinned(0.7, false, || 0.9));
    assert!(!super::thinned(0.7, false, || 0.5));
    assert!(!super::thinned(0.7, true, || 0.9));
    assert!(!super::thinned(1.0, false, || unreachable!("a sure sound draws nothing")));
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

/// Every clip, at each volume a species plays it (a sheep's call, a lamb's the same), is
/// heard within the creatures' band: a far quieter one is lost in the rest, a far louder
/// one startles.
#[test]
fn creature_voices_are_heard_within_their_band() {
    let content = content();
    let bank = Bank::load(&content.files, &content.creatures).unwrap();
    let mut uses: std::collections::BTreeSet<(usize, u32)> = Default::default();
    for voice in bank.species.iter().flatten() {
        for event in voice.events.iter().flatten() {
            for &clip in &event.clips {
                uses.insert((clip, (event.gain * voice.gain * 1000.0).round() as u32));
            }
        }
    }
    let mut measured: FxHashMap<&str, f32> = FxHashMap::default();
    let heard: Vec<(String, f32)> = uses
        .into_iter()
        .map(|(clip, gain)| {
            let path = bank.clips[clip].path.as_str();
            let level = *measured.entry(path).or_insert_with(|| crate::mix::tests::measure(path)[0]);
            (format!("{path} at {}", gain as f32 / 1000.0), level * gain as f32 / 1000.0)
        })
        .collect();
    crate::mix::tests::all([crate::mix::tests::within(Category::Creatures, heard)]);
}

/// Every one of a settlement's people barks in their own voice (a greeting, an alarm, a
/// content sigh), as the launch requires; a package that revoices them without barks fails.
#[test]
fn settlement_people_bark_in_their_own_voice() {
    let content = content();
    let bank = Bank::load(&content.files, &content.creatures).unwrap();
    assert_eq!(bank.unbarked(&content.creatures), Vec::<String>::new());
    let villager = content.creatures.id("base:villager").unwrap();
    let voice = bank.voice(villager).unwrap();
    for kind in [Kind::Greet, Kind::Alarm, Kind::Content] {
        let own = voice.events[kind as usize].as_ref().unwrap_or_else(|| panic!("no {kind:?}"));
        if let Some(fallback) = voice.event(kind.fallback().unwrap()) {
            assert_ne!(own.clips, fallback.clips, "{kind:?} is the sound it falls back to");
        }
    }
    // An alarm carries farther than a greeting, and a sigh only close by.
    assert!(voice.event(Kind::Alarm).unwrap().range > voice.event(Kind::Greet).unwrap().range);
    assert!(voice.event(Kind::Content).unwrap().range < voice.event(Kind::Greet).unwrap().range);
    let mut files = content.files.to_vec();
    files.push(voxmar_engine::content::ContentFile {
        package: "zmod".into(),
        name: defs::FILE.into(),
        source: r#"(voices: {"mute": (hurt: (clips: ["h.ogg"]))}, species: {"base:villager": (voice: "mute")})"#.into(),
    });
    let mute = Bank::load(&files, &content.creatures).unwrap();
    assert_eq!(mute.unbarked(&content.creatures), ["base:villager"]);
}
