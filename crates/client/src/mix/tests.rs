//! The loudness bands, as the tests of each kind of sound check them, and the mix's own
//! workings.

use super::*;
use voxmar_engine::content::ContentFile;

/// The base package's mix.
pub(crate) fn base() -> MixDef {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base/sounds.ron");
    let source = std::fs::read_to_string(path).unwrap();
    Recordings::load(&[ContentFile { package: "base".into(), name: "sounds.ron".into(), source }]).unwrap().mix
}

/// How loud a shipped clip is, `[at its loudest 50 ms, over all of it]`, both channels
/// together, from its asset path.
pub(crate) fn measure(path: &str) -> [f32; 2] {
    let [left, right] = crate::ambience::tests::decode(path);
    let stereo: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
    [crate::sfx::loudness(&stereo, 2), crate::sfx::mean_loudness(&stereo)]
}

/// Whether every sound `heard` (a name, and its loudness as played) is inside `category`'s
/// band, naming each that is not, and the quietest and loudest of them all.
pub(crate) fn within(category: Category, heard: impl IntoIterator<Item = (String, f32)>) -> Result<(), String> {
    let mix = base();
    let (low, high) = mix.bands[&category];
    let heard: Vec<(String, f32)> = heard.into_iter().map(|(name, level)| (name, db(level))).collect();
    assert!(!heard.is_empty(), "nothing {category:?} measured");
    let outside: Vec<String> =
        heard.iter().filter(|(_, d)| !(low..=high).contains(d)).map(|(name, d)| format!("  {name}: {d:.1} dB")).collect();
    // VOXMAR_MIX_LIST=1 lists every sound measured, for tuning.
    if std::env::var_os("VOXMAR_MIX_LIST").is_some() {
        for (name, d) in &heard {
            println!("mix {category:?} {d:.1} {name}");
        }
    }
    let quietest = heard.iter().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    let loudest = heard.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    println!("{category:?}: {} sounds from {:.1} dB ({}) to {:.1} dB ({}), band {low} to {high}", heard.len(), quietest.1, quietest.0, loudest.1, loudest.0);
    if outside.is_empty() {
        Ok(())
    } else {
        Err(format!("{category:?} sounds outside their band ({low} to {high} dB):\n{}", outside.join("\n")))
    }
}

/// Fails with every band's complaint at once.
pub(crate) fn all(results: impl IntoIterator<Item = Result<(), String>>) {
    let errors: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// The base mix has a band and a cap for every kind, and a later package replaces those
/// it names.
#[test]
fn the_mix_is_whole_and_a_later_package_replaces_its_numbers() {
    let mix = base();
    mix.check().unwrap();
    let file = |package: &str, source: &str| ContentFile { package: package.into(), name: "sounds.ron".into(), source: source.into() };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base/sounds.ron");
    let base_file = file("base", &std::fs::read_to_string(path).unwrap());
    let both = Recordings::load(&[base_file.clone(), file("mod", "(mix: (bands: {Ui: (-40, -30)}, voices: {Ui: 1}))")]).unwrap().mix;
    assert_eq!((both.bands[&Category::Ui], both.cap(Category::Ui)), ((-40.0, -30.0), 1));
    assert_eq!(both.bands[&Category::Music], mix.bands[&Category::Music]);
    assert_eq!(both.duck.as_ref().map(|d| d.to), mix.duck.as_ref().map(|d| d.to));
    for wrong in ["(mix: (bands: {Ui: (-10, -20)}))", "(mix: (voices: {Ui: 0}))", "(mix: (duck: Some((under: [], to: 1.5, fall: 0.1, rise: 1))))"] {
        let mix = Recordings::load(&[base_file.clone(), file("mod", wrong)]).unwrap().mix;
        assert!(mix.check().is_err(), "{wrong}");
    }
    let mut missing = mix.clone();
    missing.bands.remove(&Category::Combat);
    assert!(missing.check().is_err());
}

/// Things the player can walk up to fade alike: whole up close, half as loud about a
/// third of the way out, ever more gently to nothing at their reach and beyond.
#[test]
fn sounds_in_the_world_fade_along_one_curve() {
    assert_eq!(falloff(0.0, 16.0), 1.0);
    assert!((falloff(16.0 * (1.0 - 0.5f32.sqrt()), 16.0) - 0.5).abs() < 1e-5);
    assert_eq!(falloff(16.0, 16.0), 0.0);
    assert_eq!(falloff(40.0, 16.0), 0.0);
    // The same distance, as a share of the reach, is as loud whatever the reach.
    assert!((falloff(7.0, 14.0) - falloff(28.0, 56.0)).abs() < 1e-6);
    let mut last = 1.0;
    for step in 1..=32 {
        let now = falloff(step as f32 * 0.5, 16.0);
        assert!(now < last, "louder farther off at {}", step as f32 * 0.5);
        last = now;
    }
}

/// The music dips quickly under a jingle, holds there while it plays, and comes back more
/// slowly after.
#[test]
fn the_music_dips_under_a_jingle_and_comes_back() {
    let duck = base().duck.expect("the music makes room");
    assert!(duck.under.contains(&Category::Jingles) && duck.bosses);
    let mut level = 1.0;
    let mut seconds = 0.0;
    while level > duck.to {
        level = toward(level, true, &duck, 1.0 / 60.0);
        seconds += 1.0 / 60.0;
    }
    assert!((seconds - duck.fall).abs() < 0.05, "dipped in {seconds} s");
    assert_eq!(toward(level, true, &duck, 1.0), duck.to);
    seconds = 0.0;
    while level < 1.0 {
        level = toward(level, false, &duck, 1.0 / 60.0);
        seconds += 1.0 / 60.0;
    }
    assert!((seconds - duck.rise).abs() < 0.05, "back in {seconds} s");
    assert!(duck.rise > duck.fall);
}

/// An app with the mix and a listener at the origin.
fn mixing() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Mix(base()))
        .init_resource::<Duck>()
        .add_systems(PostUpdate, (number, make_room, fade_out, duck).chain());
    app.world_mut().spawn((Camera3d::default(), GlobalTransform::default()));
    app
}

/// Past a kind's cap, the farthest voice makes room, then the oldest; other kinds are let
/// be.
#[test]
fn past_its_cap_the_farthest_then_oldest_voice_makes_room() {
    let mut app = mixing();
    let cap = base().cap(Category::Creatures);
    let far = app.world_mut().spawn(Voice::at(Category::Creatures, Vec3::X * 30.0)).id();
    let near: Vec<Entity> = (0..cap).map(|i| app.world_mut().spawn(Voice::at(Category::Creatures, Vec3::X * (1.0 + i as f32 * 0.01))).id()).collect();
    let ui = app.world_mut().spawn(Voice::of(Category::Ui)).id();
    app.update();
    app.update();
    assert!(app.world().get_entity(far).is_err(), "the farthest creature still plays");
    assert!(near.iter().all(|&e| app.world().get_entity(e).is_ok()) && app.world().get_entity(ui).is_ok());

    // At one distance, the oldest goes first.
    let ui_cap = base().cap(Category::Ui);
    let mut clicks = mixing();
    let all: Vec<Entity> = (0..=ui_cap)
        .map(|_| {
            let e = clicks.world_mut().spawn(Voice::of(Category::Ui)).id();
            clicks.update();
            e
        })
        .collect();
    clicks.update();
    assert!(clicks.world().get_entity(all[0]).is_err(), "the oldest click still plays");
    assert!(all[1..].iter().all(|&e| clicks.world().get_entity(e).is_ok()));
}

/// The music dips while a jingle or a boss's voice plays, not for a click.
#[test]
fn the_music_makes_room_for_jingles_and_bosses_only() {
    for (voice, dips) in [(Voice::of(Category::Ui), false), (Voice::of(Category::Jingles), true), (Voice { ducks: true, ..Voice::at(Category::Creatures, Vec3::X) }, true), (Voice::at(Category::Creatures, Vec3::X), false)] {
        let mut app = mixing();
        app.world_mut().spawn(voice);
        for _ in 0..3 {
            app.update();
        }
        let level = app.world().resource::<Duck>().0;
        assert_eq!(level < 1.0, dips, "{voice:?}: {level}");
    }
}

/// The shipped mix passes its check; a category given no voices, or more than
/// [`VOICES_MOST`], fails it.
#[test]
fn voices_are_bounded() {
    let mix = base();
    assert_eq!(mix.check(), Ok(()));
    for (n, bad) in [(0, true), (VOICES_MOST, false), (VOICES_MOST + 1, true)] {
        let mut changed = mix.clone();
        changed.voices.insert(Category::Creatures, n);
        assert_eq!(changed.check().is_err(), bad, "{n} voices");
    }
}
