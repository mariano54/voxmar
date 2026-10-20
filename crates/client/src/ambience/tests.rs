//! The base package's ambience in typical places, and an offline preview of how they sound.

use bevy::math::{IVec2, IVec3, Vec3};
use voxmar_engine::content::ContentFile;

use super::defs::{Ambience, BedSound, Environment, FILE, Fixed};
use super::mixer::{Cue, Mixer, muffle, place};
use super::synth::{self, OVERLAP};
use crate::sfx::{self, RATE, mix};

/// A bed's segment as `[left, right]`: synthesized, or decoded from its recording.
pub(crate) fn segment(sound: &BedSound, variant: usize) -> [Vec<f32>; 2] {
    match sound {
        BedSound::Synth(synth) => synth::bed(*synth, variant),
        BedSound::Recorded(paths) => decode(&paths[variant]),
    }
}

/// A recording under `assets/` as `[left, right]` at the game's sample rate.
pub(crate) fn decode(path: &str) -> [Vec<f32>; 2] {
    super::decode(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets").join(path)).unwrap()
}

fn base() -> Ambience {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base").join(FILE);
    let source = std::fs::read_to_string(path).unwrap();
    Ambience::load(&[ContentFile { package: "base".into(), name: FILE.into(), source }]).unwrap()
}

/// A place: its environment and blocks to sound from, each group's around the player.
struct Scene {
    name: &'static str,
    env: Environment,
    samples: Vec<Vec<IVec3>>,
    /// Each set of buildings one-shots sound from: one, a dozen blocks off.
    buildings: Vec<Vec<Vec3>>,
}

fn scene(ambience: &Ambience, name: &'static str, tags: &[&str], set: &[(&str, f32)]) -> Scene {
    let mut env = Environment::new(ambience.axes());
    // A mild, still, cloudy-bright day on open land unless the scene says otherwise.
    for (axis, value) in [(Fixed::Air, 1.0), (Fixed::Sky, 1.0), (Fixed::Sun, 0.8), (Fixed::Temperature, 15.0), (Fixed::Moisture, 1.0)] {
        env.set(axis, value);
    }
    env.set(Fixed::Continentalness, 0.5);
    env.set(Fixed::Depth, -2.0);
    env.set(Fixed::Wind, 3.0);
    for &(name, value) in set {
        if let Some(axis) = Fixed::ALL.iter().find(|a| format!("{a:?}").to_lowercase() == name) {
            env.set(*axis, value);
        } else if let Some(patter) = ambience.roofs.patters.iter().position(|p| p == name) {
            env.values[ambience.patter_axis(patter)] = value;
        } else {
            let group = ambience.groups.iter().position(|g| g.name == name).unwrap_or_else(|| panic!("no axis {name}"));
            env.set_group(group, value);
        }
    }
    env.home_tags = tags.iter().map(|t| t.to_string()).collect();
    let mut random = name.len() as u64;
    let samples = (0..ambience.groups.len())
        .map(|group| {
            (0..8)
                .map(|_| {
                    let mut next = || {
                        random = mix(random);
                        (random >> 40) as f32 / (1u64 << 24) as f32
                    };
                    let (angle, distance) = (next() * std::f32::consts::TAU, 4.0 + 16.0 * next());
                    let height = if ambience.groups[group].name == "leaves" { 4.0 + 6.0 * next() } else { -1.0 };
                    IVec3::new((angle.cos() * distance) as i32, height as i32, (angle.sin() * distance) as i32)
                })
                .collect()
        })
        .collect();
    let buildings = vec![vec![Vec3::new(12.0, 0.0, 0.0)]; ambience.buildings.len()];
    Scene { name, env, samples, buildings }
}

/// A thunderstorm in the woods, heard out in it.
const STORM: &[(&str, f32)] =
    &[("rain", 1.0), ("raining", 1.0), ("storm", 0.9), ("wind", 11.0), ("gusts", 1.0), ("cover", 1.0), ("wetness", 1.0)];
/// The same from inside a home with its door open: the fill leaks out of the door, so sky
/// and air say half outdoors, and rain is heard falling outside it.
// A plank roof over it, so the rain taps on it (`roof_tap`) and is muffled (`sheltered`).
const INDOORS: &[(&str, f32)] = &[("shelter", 1.0), ("roof", 1.0), ("roof_tap", 1.0), ("sheltered", 0.6), ("sky", 0.25), ("air", 0.35), ("rain", 0.3), ("reverb", 0.1)];

fn scenes(ambience: &Ambience) -> Vec<Scene> {
    let forest = [("leaves", 1.0), ("wood", 0.7), ("grass", 0.5), ("canopy", 0.8)];
    let forest_tags = ["forest", "temperate", "broadleaf"];
    vec![
        scene(ambience, "forest_dawn", &forest_tags, &[&forest[..], &[("sun", 0.02)]].concat()),
        scene(ambience, "forest_noon", &forest_tags, &forest),
        scene(ambience, "forest_rain", &forest_tags, &[&forest[..], &[("rain", 1.0), ("cover", 1.0)]].concat()),
        scene(ambience, "meadow_noon", &["grassland", "temperate", "open"], &[("grass", 1.0), ("temperature", 20.0)]),
        scene(ambience, "meadow_night", &["grassland", "temperate", "open"], &[("grass", 1.0), ("sun", -0.7), ("temperature", 18.0)]),
        scene(ambience, "river", &["river", "water"], &[("water", 0.7), ("grass", 0.4), ("river", 1.0), ("flowing", 1.0), ("leaves", 0.3)]),
        scene(ambience, "coast", &["ocean"], &[("water", 1.0), ("sand", 0.6), ("continentalness", 0.0), ("height", 3.0), ("wind", 6.0)]),
        scene(ambience, "summit", &["mountain"], &[("rock", 0.8), ("snowpack", 0.5), ("relief", 70.0), ("height", 160.0), ("wind", 12.0), ("temperature", -6.0)]),
        scene(ambience, "cave", &["mountain"], &[("rock", 1.0), ("sky", 0.0), ("air", 0.3), ("reverb", 0.8), ("depth", 40.0)]),
        scene(ambience, "hailstorm", &["grassland", "temperate", "open"], &[("grass", 1.0), ("hail", 0.6), ("rain", 0.5), ("storm", 0.8), ("wind", 9.0), ("gusts", 1.0), ("cover", 1.0)]),
        scene(ambience, "sandstorm", &["desert", "dry", "open"], &[("sand", 1.0), ("dust", 1.0), ("wind", 13.0), ("gusts", 1.0), ("temperature", 30.0), ("moisture", 0.1)]),
        scene(ambience, "blizzard", &["frozen", "cold", "open"], &[("snowpack", 1.0), ("snow", 0.9), ("wind", 13.0), ("gusts", 1.0), ("temperature", -10.0), ("cover", 1.0)]),
        scene(ambience, "sleet", &["grassland", "temperate", "open"], &[("grass", 1.0), ("sleet", 0.5), ("rain", 0.3), ("temperature", 1.0), ("cover", 1.0)]),
        scene(ambience, "lake", &["lake", "water"], &[("water", 0.8), ("grass", 0.4), ("river", 1.0), ("still", 1.0)]),
        scene(ambience, "light_rain", &["grassland", "temperate", "open"], &[("grass", 1.0), ("rain", 0.3), ("raining", 0.3), ("cover", 0.9)]),
        scene(ambience, "porch_in_rain", &["forest", "temperate"], &[("air", 0.6), ("sky", 0.0), ("roof", 1.0), ("rain", 0.2), ("raining", 0.7), ("roof_tap", 0.7), ("sheltered", 0.45), ("cover", 1.0)]),
        // In a thatched hut with its door open in a downpour: the straw's hush, the open
        // rain muffled under the sound roof.
        scene(ambience, "thatch_in_rain", &["grassland", "temperate"], &[("air", 0.4), ("sky", 0.3), ("rain", 0.9), ("raining", 0.9), ("roof_soft", 0.9), ("sheltered", 1.0), ("cover", 1.0)]),
        scene(ambience, "forest_after_rain", &forest_tags, &[&forest[..], &[("wetness", 0.9), ("cover", 0.6)]].concat()),
        scene(ambience, "lava_cave", &["mountain"], &[("rock", 1.0), ("lava", 0.8), ("sky", 0.0), ("air", 0.3), ("reverb", 0.7), ("depth", 30.0)]),
        scene(ambience, "waterfall", &["river", "water"], &[("water", 0.6), ("falls", 0.9), ("flowing", 1.0), ("rock", 0.4)]),
        scene(ambience, "campfire_night", &["grassland", "temperate", "open"], &[("grass", 1.0), ("fire", 1.0), ("sun", -0.7), ("temperature", 18.0)]),
        scene(ambience, "by_the_hearth", &["forest", "temperate"], &[("fire", 1.0), ("air", 0.5), ("sky", 0.0), ("reverb", 0.1), ("shelter", 1.0), ("roof", 1.0)]),
        scene(ambience, "storm_outside", &forest_tags, &[&forest[..], STORM].concat()),
        scene(ambience, "storm_by_the_hearth", &forest_tags, &[&forest[..], STORM, INDOORS, &[("fire", 1.0)]].concat()),
        scene(ambience, "quiet_peak", &["mountain"], &[("rock", 0.8), ("relief", 60.0), ("height", 150.0), ("wind", 4.0), ("temperature", 2.0)]),
    ]
}

/// Runs a scene for `seconds`, returning every cue with its time and the beds' levels at
/// the end.
fn run(ambience: &Ambience, scene: &Scene, seconds: f32) -> (Vec<(f32, Cue)>, Vec<f32>) {
    let mut mixer = Mixer::new(ambience, 5);
    let mut timeline = Vec::new();
    let mut cues = Vec::new();
    let steps = (seconds * 100.0) as usize;
    for step in 0..steps {
        mixer.update(ambience, &scene.env, &scene.samples, &scene.buildings, Vec3::ZERO, 0.01, |_, _| 2.0, (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS), &mut cues);
        timeline.extend(cues.drain(..).map(|c| (step as f32 * 0.01, c)));
    }
    let levels = (0..ambience.beds.len()).map(|b| mixer.level(b)).collect();
    (timeline, levels)
}

/// A hearth or a Crucible Kiln is heard as a fire only while it burns; a campfire or a pit
/// kiln, always a fire, always.
#[test]
fn structures_are_heard_as_fires_only_while_lit() {
    let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
    let ambience = base();
    let kinds = super::kinds(&ambience, &content.blocks, &content.multiblocks, &voxmar_server::wall_shelters(&content));
    let fire = ambience.groups.iter().position(|g| g.name == "fire").unwrap() as u8;
    for (name, lit_only) in [("base:hearthstone", true), ("base:crucible", true), ("base:campfire", false), ("base:pit_kiln", false)] {
        let id = content.blocks.expect(name).0 as usize;
        assert_eq!(kinds.group[id], Some(fire), "{name} is not heard as a fire");
        assert_eq!(kinds.lit_only[id], lit_only, "{name}");
    }
}

#[test]
fn each_place_has_its_own_sound() {
    let ambience = base();
    let bed = |name: &str| ambience.beds.iter().position(|b| b.name == format!("base:{name}")).unwrap();
    let sound = |name: &str| ambience.sounds.iter().position(|s| s.name == format!("base:{name}")).unwrap();
    for scene in scenes(&ambience) {
        let (timeline, levels) = run(&ambience, &scene, 300.0);
        let audible = |name: &str| levels[bed(name)] > 0.3 * ambience.beds[bed(name)].volume;
        let played = |name: &str| timeline.iter().filter(|(_, c)| matches!(c, Cue::Sound { sound: s, .. } if *s == sound(name))).count();
        let (beds, sounds): (Vec<_>, Vec<_>) = (
            ambience.beds.iter().filter(|b| audible(b.name.trim_start_matches("base:"))).map(|b| &b.name[5..]).collect(),
            ambience.sounds.iter().filter(|s| played(&s.name[5..]) > 0).map(|s| &s.name[5..]).collect(),
        );
        println!("{}: beds {beds:?}, sounds {sounds:?}", scene.name);
        let (must, must_not): (&[&str], &[&str]) = match scene.name {
            "forest_dawn" => (&["leaves", "dawn_chorus", "warbler"], &["crickets", "owl"]),
            "forest_noon" => (&["leaves", "breeze", "warbler", "thrush"], &["meadow", "crickets", "owl", "dawn_chorus", "cave", "drip", "fire", "fire_snap"]),
            "forest_rain" => (&["downpour"], &["rain_on_roof"]),
            "meadow_noon" => (&["meadow", "breeze"], &["leaves", "crickets", "warbler"]),
            "meadow_night" => (&["crickets", "cricket"], &["meadow", "warbler", "thrush"]),
            "river" => (&["stream"], &["surf", "lake"]),
            "coast" => (&["surf"], &["stream", "lake", "frog"]),
            "lake" => (&["lake"], &["stream", "surf"]),
            "light_rain" => (&["rain"], &["downpour", "rain_on_roof"]),
            "porch_in_rain" => (&["rain_on_roof"], &["downpour", "rain_on_thatch"]),
            "thatch_in_rain" => (&["rain_on_thatch"], &["rain_on_roof", "rain_on_stone", "rain_on_glass", "downpour"]),
            "forest_after_rain" => (&["dripping_trees"], &["rain", "downpour"]),
            "lava_cave" => (&["lava", "lava_pop", "cave", "far_drip"], &["stream", "lake", "waterfall", "fire", "fire_snap"]),
            // A fire is heard by the fire, crackling and snapping, under the night outdoors.
            "campfire_night" => (&["fire", "fire_snap", "crickets"], &["lava", "lava_pop", "warbler"]),
            "by_the_hearth" => (&["fire", "fire_snap"], &["lava", "breeze", "rain_on_roof"]),
            "storm_outside" => (&["downpour", "storm_wind"], &["rain_on_roof", "fire"]),
            // Indoors in a storm: the rain on the roof and the hearth, the storm a murmur.
            "storm_by_the_hearth" => (&["rain_on_roof", "fire", "fire_snap"], &["downpour", "storm_wind", "dripping_trees", "leaves", "breeze"]),
            "waterfall" => (&["waterfall"], &["lava", "cave"]),
            // Wind on heights even on a quiet day.
            "quiet_peak" => (&["high_wind"], &["leaves", "warbler"]),
            "summit" => (&["high_wind", "gust"], &["leaves", "warbler", "crickets", "meadow"]),
            "cave" => (&["cave", "drip"], &["breeze", "high_wind", "leaves", "warbler"]),
            "hailstorm" => (&["hail", "gusty"], &["sandstorm", "sleet", "meadow"]),
            "sandstorm" => (&["sandstorm", "gusty"], &["hail", "blizzard", "meadow", "crickets"]),
            "blizzard" => (&["blizzard"], &["sandstorm", "hail", "meadow"]),
            "sleet" => (&["sleet"], &["hail", "sandstorm"]),
            other => panic!("{other}"),
        };
        for name in must {
            assert!(beds.contains(name) || sounds.contains(name), "{}: no {name}", scene.name);
        }
        for name in must_not {
            assert!(!beds.contains(name) && !sounds.contains(name), "{}: {name}", scene.name);
        }
        if scene.name == "summit" {
            assert!(levels[bed("breeze")] < 0.4 * ambience.beds[bed("breeze")].volume, "the gale drowns the breeze");
        }
    }
    // Rain turns the forest down without silencing it.
    let all = scenes(&ambience);
    let (dry, wet) = (run(&ambience, &all[1], 30.0).1, run(&ambience, &all[2], 30.0).1);
    let leaves = bed("leaves");
    assert!(wet[leaves] > 0.0 && wet[leaves] < 0.6 * dry[leaves], "{} vs {}", wet[leaves], dry[leaves]);
}

/// Indoors the storm and the wild outside fall to a murmur, and their one-shots are
/// muffled, while the hearth by the player sounds as it does anywhere and the rain drums on
/// the roof.
#[test]
fn indoors_the_outside_is_muffled_and_the_hearth_is_clear() {
    let ambience = base();
    let all = scenes(&ambience);
    let find = |name: &str| all.iter().find(|s| s.name == name).unwrap();
    let bed = |name: &str| ambience.beds.iter().position(|b| b.name == format!("base:{name}")).unwrap();
    let (outside, inside) = (run(&ambience, find("storm_outside"), 60.0), run(&ambience, find("storm_by_the_hearth"), 60.0));
    for name in ["downpour", "storm_wind", "breeze", "leaves"] {
        let (out, home) = (outside.1[bed(name)], inside.1[bed(name)]);
        assert!(out > 0.0 && home <= 0.3 * out, "{name}: {home} indoors against {out} out");
    }
    assert_eq!(outside.1[bed("rain_on_roof")], 0.0);
    assert!(inside.1[bed("rain_on_roof")] > 0.9 * ambience.beds[bed("rain_on_roof")].volume);
    // The hearth is as loud indoors in a storm as by a fire on a still night.
    let night = run(&ambience, find("campfire_night"), 60.0).1;
    assert_eq!(inside.1[bed("fire")], night[bed("fire")]);
    // Birds through the walls: still there, muffled; the fire's snaps never.
    let sounds = |timeline: &[(f32, Cue)], name: &str| -> Vec<(f32, f32)> {
        let i = ambience.sounds.iter().position(|s| s.name == format!("base:{name}")).unwrap();
        timeline.iter().filter_map(|(_, c)| match *c { Cue::Sound { sound, volume, muffle, .. } if sound == i => Some((volume, muffle)), _ => None }).collect()
    };
    let dry = run(&ambience, &scene(&ambience, "forest_noon_inside", &["forest", "temperate", "broadleaf"], &[("leaves", 1.0), ("wood", 0.7), ("shelter", 1.0), ("roof", 1.0)]), 300.0);
    let birds = sounds(&dry.0, "warbler");
    assert!(!birds.is_empty() && birds.iter().all(|&(_, muffle)| muffle >= 0.75), "{birds:?}");
    let snaps = sounds(&inside.0, "fire_snap");
    assert!(!snaps.is_empty() && snaps.iter().all(|&(_, muffle)| muffle == 0.0), "{snaps:?}");
    assert!(sounds(&outside.0, "gusty").iter().all(|&(_, muffle)| muffle == 0.0), "outdoors nothing is muffled");
}

/// Outdoor beds have a muffled twin, darker and as long, which plays as far as the place
/// is indoors; the fire, the rain on the roof and the cave's own air have none.
#[test]
fn outdoor_beds_are_heard_muffled_from_indoors() {
    let ambience = base();
    let bed = |name: &str| ambience.beds.iter().find(|b| b.name == format!("base:{name}")).unwrap();
    for name in ["rain", "downpour", "breeze", "storm_wind", "leaves", "stream", "surf"] {
        assert!(bed(name).outdoor(), "{name}");
    }
    for name in ["fire", "rain_on_roof", "cave", "lava"] {
        assert!(!bed(name).outdoor(), "{name}");
    }
    let mixer = Mixer::new(&ambience, 1);
    let mut env = Environment::new(ambience.groups.len());
    assert_eq!(mixer.muffling(&env, &bed("breeze").duck), 0.0);
    env.set(Fixed::Shelter, 1.0);
    assert!(mixer.muffling(&env, &bed("breeze").duck) > 0.8);
    assert_eq!(mixer.muffling(&env, &bed("fire").duck), 0.0);
    // A recorded bed and a synthesized one, muffled: as long, a quarter of the samples, and
    // far darker than the same kept at that rate unfiltered.
    let brightness = |c: &[f32]| {
        let rms = |x: &mut dyn Iterator<Item = f32>| (x.map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt();
        rms(&mut c.windows(2).map(|w| w[1] - w[0])) / rms(&mut c.iter().copied()).max(1e-9)
    };
    for sound in [&bed("rain").sound, &bed("breeze").sound] {
        let dry = segment(sound, 0);
        let kept: Vec<f32> = dry[0].iter().step_by(super::mixer::MUFFLED_STEP).copied().collect();
        let [left, right] = super::mixer::muffled(dry.clone(), ambience.shelter.muffle);
        assert_eq!((left.len(), right.len()), (kept.len(), kept.len()));
        assert!(brightness(&left) < 0.5 * brightness(&kept), "{sound:?}: {} against {}", brightness(&left), brightness(&kept));
        assert!(left.iter().any(|s| s.abs() > 1e-3), "{sound:?} muffled to nothing");
    }
}

/// Recorded beds are what the mixer expects: segments of `SEGMENT` seconds that fade in
/// and out, at the game's sample rate.
#[test]
fn recorded_beds_fit_the_mixer() {
    let ambience = base();
    let mut recorded = 0;
    for bed in &ambience.beds {
        let BedSound::Recorded(paths) = &bed.sound else { continue };
        for path in paths {
            let [left, right] = decode(path);
            let seconds = left.len() as f32 / RATE as f32;
            assert!((seconds - synth::SEGMENT).abs() < 0.1, "{path}: {seconds} s");
            let loudest = |s: &[f32]| s.iter().fold(0f32, |m, x| m.max(x.abs()));
            let edge = RATE as usize / 200;
            for channel in [&left, &right] {
                assert!(loudest(&channel[..edge]) < 0.03 && loudest(&channel[channel.len() - edge..]) < 0.03, "{path} starts or ends abruptly");
                assert!(loudest(channel) > 0.05 && loudest(channel) < 1.0, "{path}");
            }
            recorded += 1;
        }
    }
    assert!(recorded >= 18);
}

/// `cargo test --release -p voxmar_client ambience_preview -- --ignored` writes about
/// twenty seconds of each scene above, one after another, to
/// `target/ambience_preview.wav` for listening, mixed as the game mixes them.
#[test]
#[ignore]
fn ambience_preview() {
    let ambience = base();
    let [left, right] = render(&ambience, scenes(&ambience), &[]);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ambience_preview.wav");
    std::fs::write(&path, sfx::wav_stereo(&left, &right)).unwrap();
    println!("wrote {}", path.display());
}

/// `cargo test --release -p voxmar_client indoor_preview -- --ignored` writes a woodland
/// noon and then a thunderstorm, each heard out in it and then from inside a home by its
/// hearth, with a thunderclap in each storm, to `target/indoor_preview.wav`.
#[test]
#[ignore]
fn indoor_preview() {
    let ambience = base();
    let forest = [("leaves", 1.0), ("wood", 0.7), ("grass", 0.5), ("canopy", 0.8)];
    let tags = ["forest", "temperate", "broadleaf"];
    let picked = vec![
        scene(&ambience, "forest_noon", &tags, &forest),
        scene(&ambience, "forest_noon_inside", &tags, &[&forest[..], INDOORS, &[("rain", 0.0), ("fire", 1.0)]].concat()),
        scene(&ambience, "storm_outside", &tags, &[&forest[..], STORM].concat()),
        scene(&ambience, "storm_by_the_hearth", &tags, &[&forest[..], STORM, INDOORS, &[("fire", 1.0)]].concat()),
    ];
    // A strike 300 blocks off, as sfx plays it, eight seconds into each storm.
    let clap = decode("content/base/sounds/thunder_middle_1.mp3");
    let gain = 0.9 / (1.0 + 300.0 / 400.0) * sfx::MASTER;
    let thunder = [(2, 8.0, gain), (3, 8.0, gain * ambience.shelter.thunder)];
    let [left, right] = render(&ambience, picked, &thunder.map(|(scene, at, gain)| (scene, at, gain, &clap)));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/indoor_preview.wav");
    std::fs::write(&path, sfx::wav_stereo(&left, &right)).unwrap();
    println!("wrote {}", path.display());
}

/// About twenty seconds of each scene, one after another, mixed as the game mixes them,
/// with `extra` clips laid over: (scene, seconds into it, gain, clip).
fn render(ambience: &Ambience, scenes: Vec<Scene>, extra: &[(usize, f32, f32, &[Vec<f32>; 2])]) -> [Vec<f32>; 2] {
    let beds: Vec<Vec<[Vec<f32>; 2]>> =
        ambience.beds.iter().map(|b| (0..synth::BED_VARIANTS).map(|v| segment(&b.sound, v)).collect()).collect();
    // Outdoor beds' muffled twins, held back up to the full rate, as the game plays them.
    let step = super::mixer::MUFFLED_STEP;
    let muffled: Vec<Vec<[Vec<f32>; 2]>> = ambience
        .beds
        .iter()
        .zip(&beds)
        .map(|(bed, segments)| {
            let up = |c: Vec<f32>| c.into_iter().flat_map(|s| std::iter::repeat_n(s, step)).collect::<Vec<f32>>();
            if !bed.outdoor() {
                return Vec::new();
            }
            segments.iter().map(|segment| super::mixer::muffled(segment.clone(), ambience.shelter.muffle).map(up)).collect()
        })
        .collect();
    type Variants = Vec<(Vec<f32>, [Vec<f32>; 2])>;
    let sounds: Vec<Variants> =
        ambience.sounds.iter().map(|s| (0..synth::ONE_SHOT_VARIANTS).map(|v| synth::one_shot(s.synth, v)).collect()).collect();
    let block = RATE as usize / 100;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for (index, scene) in scenes.into_iter().enumerate() {
        println!("{} starts at {:.0} s", scene.name, left.len() as f32 / RATE as f32);
        let seconds = 20.0;
        let total = (seconds * RATE as f32) as usize;
        let start = left.len();
        left.resize(start + total + RATE as usize * 4, 0.0);
        right.resize(start + total + RATE as usize * 4, 0.0);
        let mut mixer = Mixer::new(ambience, 9);
        // Skip the warm-up and the beds' fade-in, as if the player had been there a while.
        let mut cues = Vec::new();
        for _ in 0..1000 {
            mixer.update(ambience, &scene.env, &scene.samples, &scene.buildings, Vec3::ZERO, 0.01, |_, _| 2.0, (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS), &mut cues);
        }
        cues.clear();
        let mut playing: Vec<(usize, usize, usize)> = (0..ambience.beds.len())
            .filter(|&b| mixer.level(b) > 0.0)
            .map(|b| (b, 0, block * (OVERLAP * 100.0) as usize))
            .collect();
        for step in 0..total / block {
            let at = start + step * block;
            mixer.update(ambience, &scene.env, &scene.samples, &scene.buildings, Vec3::ZERO, 0.01, |s, v| sounds[s][v].0.len() as f32 / RATE as f32, (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS), &mut cues);
            for cue in cues.drain(..) {
                match cue {
                    Cue::Segment { bed, variant } => playing.push((bed, variant, 0)),
                    Cue::Stop { bed } => playing.retain(|p| p.0 != bed),
                    Cue::Sound { sound, variant, at: position, volume, muffle: muffled, .. } => {
                        let (dry, wet) = &sounds[sound][variant];
                        let reverb = scene.env.get(Fixed::Reverb);
                        let mut placed = place(dry, wet, position, Vec3::ZERO, Vec3::X, Vec3::NEG_Z, reverb);
                        muffle(&mut placed, muffled, ambience.shelter.muffle);
                        let [l, r] = placed;
                        for (i, (l, r)) in l.iter().zip(&r).enumerate() {
                            if let (Some(a), Some(b)) = (left.get_mut(at + i), right.get_mut(at + i)) {
                                *a += l * volume * sfx::MASTER * super::AMBIENCE_GAIN;
                                *b += r * volume * sfx::MASTER * super::AMBIENCE_GAIN;
                            }
                        }
                    }
                }
            }
            for (bed, variant, offset) in &mut playing {
                let [l, r] = &beds[*bed][*variant];
                let level = mixer.level(*bed) * sfx::MASTER * super::AMBIENCE_GAIN;
                let wet = if muffled[*bed].is_empty() { 0.0 } else { mixer.muffling(&scene.env, &ambience.beds[*bed].duck) };
                for i in 0..block {
                    if let Some(s) = l.get(*offset + i) {
                        left[at + i] += s * level * (1.0 - wet);
                        right[at + i] += r[*offset + i] * level * (1.0 - wet);
                    }
                    if let Some([ml, mr]) = muffled[*bed].get(*variant)
                        && let (Some(a), Some(b)) = (ml.get(*offset + i), mr.get(*offset + i))
                    {
                        left[at + i] += a * level * wet;
                        right[at + i] += b * level * wet;
                    }
                }
                *offset += block;
            }
            playing.retain(|&(bed, variant, offset)| offset < beds[bed][variant][0].len());
        }
        for &(_, at, gain, clip) in extra.iter().filter(|e| e.0 == index) {
            let from = start + (at * RATE as f32) as usize;
            for (i, (l, r)) in clip[0].iter().zip(&clip[1]).enumerate() {
                if let (Some(a), Some(b)) = (left.get_mut(from + i), right.get_mut(from + i)) {
                    *a += l * gain;
                    *b += r * gain;
                }
            }
        }
        // A short gap between scenes, and fade out whatever rings on.
        let end = start + total;
        for i in 0..RATE as usize {
            let fade = 1.0 - i as f32 / RATE as f32;
            left[end + i] *= fade;
            right[end + i] *= fade;
        }
        left.truncate(end + RATE as usize * 2);
        right.truncate(end + RATE as usize * 2);
        for s in left[end + RATE as usize..].iter_mut().chain(right[end + RATE as usize..].iter_mut()) {
            *s = 0.0;
        }
        let peak = left[start..end].iter().chain(&right[start..end]).fold(0.0f32, |p, s| p.max(s.abs()));
        let rms = (left[start..end].iter().map(|s| s * s).sum::<f32>() / total as f32).sqrt();
        println!("  peak {peak:.2}, rms {rms:.3}");
        assert!(peak < 1.0, "{} clips", scene.name);
    }
    [left, right]
}

/// `cargo test --profile iter -p voxmar_client ambience_survey -- --ignored --nocapture`
/// generates real places of a world (`SEED`, default 7) and prints what the enclosure
/// scan and the generator make of them, for tuning the groups' ranges and the layers.
#[test]
#[ignore]
fn ambience_survey() {
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::world::VoxelWorld;
    use voxmar_worldgen::{Generator, Overworld, WorldContent};

    let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let generator = Overworld::with_content(seed, content.blocks.clone(), &WorldContent::load(&content).unwrap());
    let ambience = Ambience::load(&content.files).unwrap();
    let kinds = super::kinds(&ambience, &content.blocks, &content.multiblocks, &voxmar_server::wall_shelters(&content));
    let around = |x: i32, z: i32| {
        let center = ColumnPos::of_block(IVec3::new(x, 0, z));
        let mut world = VoxelWorld::default();
        for dz in -1..=1 {
            for dx in -1..=1 {
                world.insert(center.offset(dx, dz), std::sync::Arc::new(generator.generate(center.offset(dx, dz))));
            }
        }
        world
    };
    let report = |name: &str, world: &VoxelWorld, eye: IVec3| {
        let found = super::enclosure::scan(eye, |p| world.block(p), |_| true, &kinds, 1);
        let place = super::place(&generator, eye, &[]);
        let groups: Vec<String> = ambience
            .groups
            .iter()
            .enumerate()
            .map(|(g, group)| format!("{} {:.0}→{:.2}", group.name, found.groups[g], ambience.presence(g, found.groups[g])))
            .collect();
        println!(
            "{name} at {eye}: air {:.2} sky {:.2} canopy {:.2} | {} | {:.1}°C moisture {:.2} cont {:.2} river {:.2} relief {:.0} depth {:.0} tags {:?}",
            found.air,
            found.sky,
            found.canopy,
            groups.join(", "),
            place.temperature,
            place.moisture,
            place.continentalness,
            place.river,
            eye.y as f32 - place.land,
            place.ground - eye.y as f32,
            place.home_tags,
        );
    };
    for biome in ["plains", "forest", "dark_forest", "taiga", "swamp", "jungle", "desert", "mountains", "river", "ocean"] {
        let Some(at) = generator.locate_biome(biome, IVec2::ZERO, 6000) else {
            println!("{biome}: none within 6 km");
            continue;
        };
        let (x, z) = (at.x + 7, at.y + 7);
        let world = around(x, z);
        // Stand on the highest solid block, or float at the water surface.
        let top = (0..voxmar_engine::coords::WORLD_HEIGHT)
            .rev()
            .find(|&y| world.block(IVec3::new(x, y, z)).is_some_and(|b| !b.is_air() && content.blocks.info(b).solid && content.blocks.info(b).render != voxmar_engine::block::RenderKind::Leaves || b == content.blocks.id("base:water").unwrap()))
            .unwrap_or(128);
        report(biome, &world, IVec3::new(x, top + 2, z));
        // A cave under it, if the column has one: open air well below the surface.
        if let Some(y) = (20..top - 15).rev().find(|&y| {
            [0, 1, 2].iter().all(|&d| world.block(IVec3::new(x, y + d, z)).is_some_and(|b| b.is_air()))
        }) {
            report(&format!("{biome} cave"), &world, IVec3::new(x, y + 1, z));
        }
    }
}

/// Leaving a world forgets where the player was in it, so the next world's music does not
/// start out underground.
#[test]
fn leaving_a_world_forgets_the_surroundings() {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::World;
    let mut world = World::new();
    world.insert_resource(super::Surroundings { flowers: 0.6, depth: 40.0, sky: 0.0, biome: Vec::new(), thunder: 0.5, settlement: Some((3, 0.0)), started: 9, surveyed: 9 });
    world.run_system_once(super::leave).unwrap();
    let left = world.resource::<super::Surroundings>();
    let fresh = super::Surroundings::default();
    assert_eq!((left.flowers, left.depth, left.sky, left.thunder, left.settlement, left.surveyed), (fresh.flowers, fresh.depth, fresh.sky, fresh.thunder, None, 0));
    // The count of surveys runs on, so one still out from this world is never fresh in the
    // next.
    assert_eq!(left.started, 9);
}

/// Every bed, over all of a segment, and every one-shot, at its loudest from where it
/// sounds nearest, is heard within the ambience's band at full weight.
#[test]
fn the_ambience_is_heard_within_its_band() {
    use super::defs::Placement;
    use crate::mix::{Category, tests::{all, within}};
    let ambience = base();
    let mut heard = Vec::new();
    for bed in &ambience.beds {
        for variant in 0..synth::BED_VARIANTS {
            let [left, right] = segment(&bed.sound, variant);
            // The middle of the segment, past its fades.
            let fade = (OVERLAP * RATE as f32) as usize;
            let stereo: Vec<f32> = left[fade..left.len() - fade].iter().zip(&right[fade..right.len() - fade]).flat_map(|(l, r)| [*l, *r]).collect();
            heard.push((format!("bed {} {variant}", bed.name), sfx::mean_loudness(&stereo) * bed.volume * super::AMBIENCE_GAIN));
        }
    }
    for sound in &ambience.sounds {
        let distance = match sound.at {
            Placement::Around { distance, .. } => distance.0,
            // A building may be close by.
            Placement::Block(_) | Placement::Listener | Placement::Building(_) => 0.0,
        };
        for variant in 0..synth::ONE_SHOT_VARIANTS {
            let (dry, _) = synth::one_shot(sound.synth, variant);
            let near = sound.reach / (sound.reach + distance);
            heard.push((format!("{} {variant}", sound.name), sfx::loudness(&dry, 1) * sound.volume * near * super::AMBIENCE_GAIN));
        }
    }
    all([within(Category::Ambience, heard)]);
}

/// The roof look sees a roof over any part of the body, not just the middle (standing
/// half under an eave), and over the way walked since the last look (under a beam and out
/// again between two looks); the lowest roof wins, and nothing above `height` counts.
#[test]
fn the_roof_look_misses_no_edge_and_no_pass() {
    use voxmar_engine::block::BlockId;
    let (roof, beam, high) = (BlockId(5), BlockId(6), BlockId(7));
    let world = |p: IVec3| {
        Some(match (p.x, p.y, p.z) {
            // An eave over x 0..=3 at y 3, a beam over x 10 at y 4, a high ledge over x 20.
            (0..=3, 3, _) => roof,
            (10, 4, _) => beam,
            (20, 30, _) => high,
            _ => BlockId::AIR,
        })
    };
    let look = |from: Vec3, to: Vec3| super::roof_over(world, from, to, 12, |b| b != BlockId::AIR);
    let feet = |x: f32| Vec3::new(x, 1.0, 0.5);
    assert_eq!(look(feet(1.5), feet(1.5)), Some(roof));
    // The body's edge just under the eave, its middle out from under it.
    assert_eq!(look(feet(4.2), feet(4.2)), Some(roof));
    assert_eq!(look(feet(4.4), feet(4.4)), None);
    // Under the beam and out again between two looks.
    assert_eq!(look(feet(8.0), feet(12.5)), Some(beam));
    // The lowest roof over the way.
    assert_eq!(look(feet(2.0), feet(10.5)), Some(roof));
    assert_eq!(look(feet(20.5), feet(20.5)), None, "a ledge 29 above the feet is no roof");
}

/// Every roof in the base game patters or is silent, by its building material or else its
/// family sound, and the home's roofs patter as they are made: thatch soft, shingles and
/// timber a tap, stone and brick dull, glass bright.
#[test]
fn shipped_roofs_patter_by_their_material() {
    let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
    let blocks = &content.blocks;
    let ambience = Ambience::load(&content.files).unwrap();
    let recorded = sfx::Recordings::load(&content.files).unwrap();
    let materials: Vec<&str> = recorded.materials.iter().map(|m| m.name.as_str()).collect();
    let material = |id| recorded.material_of(blocks, id).map_or_else(|| super::defs::family(blocks.info(id).sound), |m| recorded.materials[m].name.clone());
    let patters = ambience.roof_patters(blocks, &materials, material).unwrap();
    let of = |name: &str| patters[blocks.id(name).unwrap_or_else(|| panic!("{name}")).0 as usize].map(|p| ambience.roofs.patters[p as usize].as_str());
    for (block, patter) in [
        ("base:thatch_roof", Some("roof_soft")),
        ("base:hide_lean_to", Some("roof_soft")),
        ("base:oak_shingles", Some("roof_tap")),
        ("base:oak_beam", Some("roof_tap")),
        ("base:oak_planks", Some("roof_tap")),
        ("base:cobblestone_slab", Some("roof_dull")),
        ("base:stone_bricks", Some("roof_dull")),
        ("base:stone", Some("roof_dull")),
        ("base:glass", Some("roof_bright")),
        ("base:oak_leaves", None),
        ("core:air", None),
    ] {
        assert_eq!(of(block), patter, "{block}");
    }
}

/// Inside a home the weather outside is muffled: in a doorway under a thatched roof in a gale
/// and a blizzard, the wind and the snow are turned well down from out in the open, by the
/// `sheltered` axis the roof overhead raises.
#[test]
fn a_home_muffles_the_weather_outside() {
    let ambience = base();
    let weather = [("air", 0.5), ("sky", 0.4), ("wind", 16.0), ("snow", 0.8), ("storm", 0.8), ("temperature", -5.0), ("cover", 1.0)];
    let open = scene(&ambience, "open", &["taiga", "cold"], &weather);
    // Under thatch, the commonest roof of a home, which carries no shelter tag of its own:
    // what it is made of muffles it, as a lean-to of hide, open at its sides, does not.
    let soft = ambience.roofs.patters.iter().position(|p| p == "roof_soft");
    let thatch = ambience.roofs.sheltered(|_| false, soft);
    assert!(thatch > ambience.roofs.sheltered(|t| t == "shelter:draughty", soft) + 0.1, "thatch muffles no more than a hide lean-to");
    let inside = scene(&ambience, "doorway", &["taiga", "cold"], &[weather.as_slice(), &[("sheltered", thatch)]].concat());
    let (open, inside) = (run(&ambience, &open, 30.0).1, run(&ambience, &inside, 30.0).1);
    for name in ["storm_wind", "blizzard"] {
        let bed = ambience.beds.iter().position(|b| b.name == format!("base:{name}")).unwrap();
        assert!(open[bed] > 0.3 * ambience.beds[bed].volume, "{name} is not heard in the open: {}", open[bed]);
        assert!(inside[bed] < 0.5 * open[bed], "{name} is {} inside, {} in the open", inside[bed], open[bed]);
    }
}

/// The `settlement` axis is how far it is to the nearest settlement's middle, across the
/// ground, and [`NO_SETTLEMENT`](super::defs::NO_SETTLEMENT) where none is in hearing. Its
/// square is within its bounds (`settlement_edge` 0) and a walk out past its outermost
/// building is not; `settlement_people` is how many live there; and a building sound is
/// heard from the buildings of its name, at their doors when it asks.
#[test]
fn the_settlement_axis_is_the_way_to_the_nearest_middle() {
    use bevy::math::Vec3Swizzles;
    use voxmar_worldgen::{Generator, Overworld, WorldContent};

    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let generator = Overworld::with_content(7, content.blocks.clone(), &WorldContent::load(&content).unwrap());
    let spawn = generator.spawn_point();
    let found = generator.settlements_near(spawn.xz(), 1500);
    let near = found.iter().min_by_key(|s| (s.centre - spawn).xz().length_squared()).expect("a settlement near spawn");
    let sets = [super::defs::BuildingSet { names: vec!["well".into()], door: false }, super::defs::BuildingSet { names: vec!["house".into()], door: true }];
    let place = super::place(&generator, near.centre + IVec3::new(3, 2, 4), &sets);
    let expected = bevy::math::Vec2::new(3.5, 4.5).length();
    assert!((place.settlement - expected).abs() < 1e-3, "{} blocks off, not {expected}", place.settlement);
    assert_eq!(place.edge, 0.0, "in its square, within its bounds");
    assert_eq!((place.people, place.id), (near.people.len() as f32, Some(near.id)));
    assert!(place.people > 0.0, "someone lives there");
    let well = near.buildings.iter().find(|b| b.kind.name() == "well").expect("a well");
    let middle = bevy::math::Vec2::new((well.min.x + well.max.x + 1) as f32 / 2.0, (well.min.y + well.max.y + 1) as f32 / 2.0);
    assert!(place.buildings[0].iter().any(|p| (p.xz() - middle).length() < 1e-3), "the well's sound from its middle {middle}: {:?}", place.buildings[0]);
    let doors: Vec<_> = near.buildings.iter().filter(|b| b.answers("house")).filter_map(|b| b.door).filter(|d| (d.xz() - near.centre.xz()).as_vec2().length() < super::BUILDINGS_HEARD - 6.0).collect();
    assert!(!doors.is_empty(), "houses near the square");
    for door in doors {
        assert!(place.buildings[1].iter().any(|p| (*p - (door.as_vec3() + 0.5)).length() < 1e-3), "a house's door at {door} is not heard: {:?}", place.buildings[1]);
    }
    // Past its outermost building, east, by twenty blocks and the margin.
    let east = near.buildings.iter().map(|b| b.max.x).max().unwrap().max(near.centre.x);
    let out = super::place(&generator, IVec3::new(east + 20 + super::SETTLEMENT_MARGIN, near.centre.y, near.centre.z), &sets);
    assert!((19.0..=21.0).contains(&out.edge), "twenty blocks past its bounds is {} outside", out.edge);
    let alone = [IVec2::new(1, 0), IVec2::new(0, 1), IVec2::new(-1, 0), IVec2::new(0, -1)]
        .into_iter()
        .map(|way| near.centre.xz() + way * 700)
        .find(|&at| generator.settlements_near(at, super::SETTLEMENTS_HEARD).is_empty())
        .expect("somewhere 700 blocks off with no settlement in hearing");
    let place = super::place(&generator, IVec3::new(alone.x, near.centre.y, alone.y), &sets);
    assert_eq!((place.settlement, place.edge, place.people), (super::defs::NO_SETTLEMENT, super::defs::NO_SETTLEMENT, 0.0));
    assert!(place.buildings.iter().all(Vec::is_empty));
    // Standing at its east edge, by a second settlement whose middle is nearer but whose
    // bounds the player is outside: the one stood in is the one heard.
    let at = bevy::math::Vec2::new(east as f32 + 0.5, near.centre.z as f32 + 0.5);
    let mut other = near.clone();
    other.id = near.id + 1;
    other.buildings.clear();
    other.centre = IVec3::new(east + super::SETTLEMENT_MARGIN + 3, near.centre.y, near.centre.z);
    let ways = [near, &other].map(|s| s.centre.xz().as_vec2().distance(at));
    assert!(ways[1] < ways[0], "the other's middle is nearer: {ways:?}");
    let both = [near.clone(), other];
    let (chosen, _, (edge, _)) = super::nearest_settlement(&both, at, &sets).unwrap();
    assert_eq!((chosen.id, edge), (near.id, 0.0), "the settlement stood in, not the nearer middle");
    // Past hearing of every settlement, though the search still gives some from the regions
    // it touches: none, not the nearest of those.
    let heard = super::SETTLEMENTS_HEARD as f32;
    let beyond = [IVec2::new(1, 0), IVec2::new(0, 1), IVec2::new(-1, 0), IVec2::new(0, -1)]
        .into_iter()
        .map(|way| near.centre.xz() + way * 260)
        .find(|&at| {
            let found = generator.settlements_near(at, super::SETTLEMENTS_HEARD);
            !found.is_empty() && found.iter().all(|s| s.centre.xz().as_vec2().distance(at.as_vec2() + 0.5) > heard)
        })
        .expect("somewhere 260 blocks off where the search gives only settlements out of hearing");
    let place = super::place(&generator, IVec3::new(beyond.x, near.centre.y, beyond.y), &sets);
    assert_eq!(place.settlement, super::defs::NO_SETTLEMENT, "a settlement out of hearing counted");
}

/// A draughty home whistles on a windy night, louder the more draughty wall is near: in a
/// room walled with wattle under thatch the draught plays, and walled with bare earth too
/// (untagged, so draughty as the home judges it); the same room daubed is silent, though it
/// stands on earth (a floor is no wall); and outside, by the same wattle walls but under
/// the open sky, it is silent too. The walls are counted by the enclosure scan, as in the
/// game.
#[test]
fn a_draughty_home_whistles_until_daubed() {
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::world::{Column, VoxelWorld};

    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let blocks = &content.blocks;
    let ambience = Ambience::load(&content.files).unwrap();
    let kinds = super::kinds(&ambience, blocks, &content.multiblocks, &voxmar_server::wall_shelters(&content));
    let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("{name}"));
    let group = ambience.groups.iter().position(|g| g.name == "draughty").expect("a draughty group");
    let draught = ambience.beds.iter().position(|b| b.name == "base:draught").expect("a draught bed");
    // A room five across inside, walls three high on an earth floor, a thatched roof over it.
    let room = |wall: &str, roofed: bool| {
        let mut world = VoxelWorld::default();
        for dz in -1..=1 {
            for dx in -1..=1 {
                world.insert(ColumnPos::of_block(IVec3::ZERO).offset(dx, dz), std::sync::Arc::new(Column::default()));
            }
        }
        for x in -3..=3 {
            for z in -3..=3 {
                world.set_block(IVec3::new(x, 9, z), id("base:dirt"));
                for y in 10..13 {
                    if x.abs() == 3 || z.abs() == 3 {
                        world.set_block(IVec3::new(x, y, z), id(wall));
                    }
                }
                if roofed {
                    world.set_block(IVec3::new(x, 13, z), id("base:thatch_roof"));
                }
            }
        }
        world
    };
    let soft = ambience.roofs.patters.iter().position(|p| p == "roof_soft");
    let night = [("wind", 12.0), ("sun", -0.6), ("raining", 0.0), ("air", 0.1), ("sky", 0.0)];
    let heard = |world: &VoxelWorld, eye: IVec3, sheltered: f32| {
        let found = super::enclosure::scan(eye, |p| world.block(p), |_| true, &kinds, 1);
        let draughty = ambience.presence(group, found.groups[group]);
        let place = scene(&ambience, "home", &["forest"], &[night.as_slice(), &[("draughty", draughty), ("sheltered", sheltered)]].concat());
        (draughty, run(&ambience, &place, 20.0).1[draught])
    };
    let under_thatch = ambience.roofs.sheltered(|_| false, soft);
    let full = ambience.beds[draught].volume;
    let (walls, wattle) = heard(&room("base:wattle_panel", true), IVec3::new(0, 11, 0), under_thatch);
    assert!(walls > 0.5 && wattle > 0.5 * full, "in a wattle room the draughty walls count {walls:.2} and the draught plays at {wattle:.3} of {full}");
    let (walls, earth) = heard(&room("base:dirt", true), IVec3::new(0, 11, 0), under_thatch);
    assert!(walls > 0.5 && earth > 0.5 * full, "in an earth-walled room the draughty walls count {walls:.2} and the draught plays at {earth:.3}");
    let (walls, daubed) = heard(&room("base:daub", true), IVec3::new(0, 11, 0), under_thatch);
    assert!(walls == 0.0 && daubed < 1e-3, "daubed, the walls still count {walls:.2} as draughty and the draught plays at {daubed:.3}");
    // Outside, a step from the wattle wall, no roof overhead.
    let (walls, outside) = heard(&room("base:wattle_panel", false), IVec3::new(5, 11, 0), 0.0);
    assert!(walls > 0.0 && outside < 1e-3, "outside by the wattle (counting {walls:.2}) the draught plays at {outside:.3}");
    // Fewer draughty walls, a fainter whistle: one wall of wattle, the rest daubed.
    let mut patched = room("base:daub", true);
    for x in -3..=3 {
        for y in 10..13 {
            patched.set_block(IVec3::new(x, y, -3), id("base:wattle_panel"));
        }
    }
    let (some, fainter) = heard(&patched, IVec3::new(0, 11, 0), under_thatch);
    assert!(some > 0.0 && fainter > 0.0 && fainter < wattle, "one wattle wall left plays at {fainter:.3}, not under the {wattle:.3} of four");
    // Windows whistle too: a daubed room with a wall of open-shuttered lattice windows plays
    // the draught, and the same windows shuttered are quiet.
    let windowed = |window: &str| {
        let mut room = room("base:daub", true);
        for x in -2..=2 {
            room.set_block(IVec3::new(x, 11, -3), id(window));
        }
        room
    };
    let (gaps, open) = heard(&windowed("base:oak_shuttered_window"), IVec3::new(0, 11, 0), under_thatch);
    assert!(gaps > 0.0 && open > 0.0, "open shutters (counting {gaps:.2}) play the draught at {open:.3}");
    let (gaps, shut) = heard(&windowed("base:oak_shuttered_window_shut"), IVec3::new(0, 11, 0), under_thatch);
    assert!(gaps == 0.0 && shut < 1e-3, "shuttered, the windows count {gaps:.2} and the draught plays at {shut:.3}");
}

/// A roof block muffles the rain once: thatch, tagged `shelter:sound` for the home's walls,
/// is no quieter overhead than its soft patter alone makes it; a roof of no muffling patter
/// is muffled by its shelter tags instead.
#[test]
fn a_thatch_roof_muffles_the_rain_once() {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let ambience = Ambience::load(&content.files).unwrap();
    let roofs = &ambience.roofs;
    let thatch = content.blocks.def(content.blocks.id("base:thatch_roof").unwrap());
    assert!(thatch.has_tag("shelter:sound"), "thatch is a sound wall now; this test checks it is not counted twice overhead");
    let soft = roofs.patters.iter().position(|p| p == "roof_soft").unwrap();
    let once = 1.0 - (1.0 - roofs.muffle) * (1.0 - roofs.patter_shelter[soft]);
    let heard = roofs.sheltered(|tag| thatch.has_tag(tag), Some(soft));
    assert!(heard <= once + 1e-6, "thatch muffles {heard:.3}, more than its patter alone ({once:.3})");
    assert!((heard - once).abs() < 1e-6, "thatch muffles {heard:.3}, not its patter's {once:.3}");
    let sound = roofs.shelter.iter().find(|(tag, _)| tag == "shelter:sound").unwrap().1;
    assert!((roofs.sheltered(|tag| tag == "shelter:sound", None) - (1.0 - (1.0 - roofs.muffle) * (1.0 - sound))).abs() < 1e-6);
}

/// Fire in the thatch as the server tells it: thatch the server says smoulders hisses, and
/// the same thatch not smouldering is silent; burning cells crackle, fuller with more of
/// them near and thinner as they burn out, silent with none. Counted by the enclosure scan
/// from the blocks and the server's smoulders, as in the game.
#[test]
fn thatch_hisses_as_it_smoulders_and_crackles_thinner_as_it_burns_out() {
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::world::{Column, VoxelWorld};

    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let blocks = &content.blocks;
    let ambience = Ambience::load(&content.files).unwrap();
    let kinds = super::kinds(&ambience, blocks, &content.multiblocks, &voxmar_server::wall_shelters(&content));
    let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("{name}"));
    let group = |name: &str| ambience.groups.iter().position(|g| g.name == name).unwrap_or_else(|| panic!("a {name} group"));
    let bed = |name: &str| ambience.beds.iter().position(|b| b.name == format!("base:{name}")).unwrap_or_else(|| panic!("a {name} bed"));
    let (smoulder, blaze) = (group("smoulder"), group("blaze"));
    let burning = content.fire.burning.expect("fire.ron burns");
    // Open ground, and `cells` of a row three blocks off set to `block`.
    let world = |block: voxmar_engine::block::BlockId, cells: i32| {
        let mut world = VoxelWorld::default();
        for dz in -1..=1 {
            for dx in -1..=1 {
                world.insert(ColumnPos::of_block(IVec3::ZERO).offset(dx, dz), std::sync::Arc::new(Column::default()));
            }
        }
        for x in -6..=6 {
            for z in -6..=6 {
                world.set_block(IVec3::new(x, 9, z), id("base:dirt"));
            }
        }
        for i in 0..cells {
            world.set_block(IVec3::new(3, 10, i - 3), block);
        }
        world
    };
    let night = [("sun", -0.6), ("air", 0.8), ("sky", 0.9), ("wind", 1.0), ("raining", 0.0)];
    let heard = |world: &VoxelWorld, smoulders: bool, listen: usize| {
        let found = super::enclosure::scan(IVec3::new(0, 10, 0), |p| world.block(p), |_| smoulders, &kinds, 1);
        let axes = [("smoulder", ambience.presence(smoulder, found.groups[smoulder])), ("blaze", ambience.presence(blaze, found.groups[blaze]))];
        let place = scene(&ambience, "yard", &["plains"], &[night.as_slice(), &axes].concat());
        run(&ambience, &place, 20.0).1[listen]
    };
    let (hiss, crackle) = (bed("smoulder_hiss"), bed("blaze_crackle"));
    let thatch = world(id("base:thatch_roof"), 1);
    let smouldering = heard(&thatch, true, hiss);
    assert!(smouldering > 0.5 * ambience.beds[hiss].volume, "smouldering thatch hisses at {smouldering:.3} of {}", ambience.beds[hiss].volume);
    let still = heard(&thatch, false, hiss);
    assert!(still < 1e-3, "thatch the server does not say smoulders hisses at {still:.3}");
    let [six, two, none] = [6, 2, 0].map(|cells| heard(&world(burning, cells), false, crackle));
    assert!(six > 0.8 * ambience.beds[crackle].volume, "six burning cells near crackle at {six:.3} of {}", ambience.beds[crackle].volume);
    assert!(two > 0.05 && two < 0.75 * six, "two burning cells left crackle at {two:.3}, against {six:.3} for six: it thins as they burn out");
    assert!(none < 1e-3, "with every cell burned out the crackle plays at {none:.3}");
}

/// A settlement through its day, as the world and the server's clock and weather tell it:
/// by day its people talk across it, fuller in a village than a hamlet and fewer out in
/// the rain, with the smith at the anvil and sheep in the pens heard from those buildings;
/// at dusk the talk drops low, doors open and shut at the houses and supper things knock;
/// at night it sleeps but for a dog now and then and an owl; and away from it, none of it.
#[test]
fn a_settlement_talks_by_day_comes_home_at_dusk_and_sleeps_at_night() {
    let ambience = base();
    let bed = |name: &str| ambience.beds.iter().position(|b| b.name == format!("base:{name}")).unwrap_or_else(|| panic!("no bed {name}"));
    let sound = |name: &str| ambience.sounds.iter().position(|s| s.name == format!("base:{name}")).unwrap_or_else(|| panic!("no sound {name}"));
    let (murmur, evening) = (bed("murmur"), bed("murmur_evening"));
    let place = |people: f32, edge: f32, set: &[(&str, f32)]| {
        let within = [("settlement", 10.0 + edge), ("settlementedge", edge), ("settlementpeople", people)];
        scene(&ambience, "village", &["plains", "temperate"], &[within.as_slice(), set].concat())
    };
    let heard = |scene: &Scene, seconds: f32| {
        let (timeline, levels) = run(&ambience, scene, seconds);
        let played = |name: &str| timeline.iter().filter(|(_, c)| matches!(c, Cue::Sound { sound: s, .. } if *s == sound(name))).count();
        let from = |name: &str| timeline.iter().filter_map(|(_, c)| match c { Cue::Sound { sound: s, at, .. } if *s == sound(name) => Some(*at), _ => None }).collect::<Vec<_>>();
        let named = ["smithy", "pen_bleat", "doors", "supper", "night_dog", "night_owl"].map(|n| (n, played(n)));
        (levels, named, from("smithy"))
    };
    let count = |named: &[(&str, usize); 6], name: &str| named.iter().find(|(n, _)| *n == name).unwrap().1;
    // Noon in a village of thirty.
    let (day, named, smithy_at) = heard(&place(30.0, 0.0, &[("sun", 0.8)]), 180.0);
    let full = ambience.beds[murmur].volume;
    assert!(day[murmur] > 0.8 * full && day[evening] < 1e-3, "a village by day talks at {:.3} of {full} (evening {:.3})", day[murmur], day[evening]);
    assert!(count(&named, "smithy") > 0 && count(&named, "pen_bleat") > 0, "work by day: {named:?}");
    assert!(smithy_at.iter().all(|at| *at == Some(Vec3::new(12.0, 0.0, 0.0))), "the anvil rings from the smithy: {smithy_at:?}");
    for quiet in ["doors", "supper", "night_dog", "night_owl"] {
        assert_eq!(count(&named, quiet), 0, "{quiet} by day: {named:?}");
    }
    // A hamlet of five murmurs; the rain sends people in.
    let (hamlet, ..) = heard(&place(5.0, 0.0, &[("sun", 0.8)]), 30.0);
    assert!(hamlet[murmur] < 0.4 * day[murmur], "a hamlet talks at {:.3}, a village at {:.3}", hamlet[murmur], day[murmur]);
    let (wet, ..) = heard(&place(30.0, 0.0, &[("sun", 0.8), ("rain", 1.0), ("raining", 1.0)]), 30.0);
    assert!(wet[murmur] < 0.75 * day[murmur], "in the rain the village talks at {:.3}, dry {:.3}", wet[murmur], day[murmur]);
    // Dusk: the talk drops low, doors and supper.
    let (dusk, named, _) = heard(&place(30.0, 0.0, &[("sun", -0.1)]), 180.0);
    assert!(dusk[murmur] < 1e-3 && dusk[evening] > 0.5 * ambience.beds[evening].volume, "at dusk the day's talk {:.3}, the evening's {:.3}", dusk[murmur], dusk[evening]);
    assert!(count(&named, "doors") > 0 && count(&named, "supper") > 0, "doors and supper at dusk: {named:?}");
    assert_eq!(count(&named, "smithy"), 0, "the anvil at dusk: {named:?}");
    // Night: asleep, a dog and an owl now and then.
    let (night, named, _) = heard(&place(30.0, 0.0, &[("sun", -0.8)]), 600.0);
    assert!(night[murmur] < 1e-3 && night[evening] < 1e-3, "talk at night: {:.3}, {:.3}", night[murmur], night[evening]);
    assert!(count(&named, "night_dog") > 0 && count(&named, "night_owl") > 0, "a dog and an owl at night: {named:?}");
    for quiet in ["smithy", "pen_bleat", "doors", "supper"] {
        assert_eq!(count(&named, quiet), 0, "{quiet} at night: {named:?}");
    }
    // A hundred blocks out of it, none of it, day or night.
    for sun in [0.8, -0.1, -0.8] {
        let (away, named, _) = heard(&place(30.0, 100.0, &[("sun", sun)]), 180.0);
        assert!(away[murmur] < 1e-3 && away[evening] < 1e-3 && named.iter().all(|(_, n)| *n == 0), "a hundred blocks off at sun {sun}: {:.3} {:.3} {named:?}", away[murmur], away[evening]);
    }
}

/// Caves by their rock and their depth: wet rock (under a moist climate) has its dripping
/// air and drips, oftener deep and shut in, where a second tier joins the first; dry rock
/// (under a desert) has still air and grit ticking down instead, nothing dripping; water
/// seeping into a pool is heard in either.
/// A timber home creaks as it settles at night, indoors only; after the rain the eaves drip
/// under a roof until the ground dries, never while it still rains.
#[test]
fn a_home_settles_at_night_and_its_eaves_drip_after_rain() {
    let ambience = base();
    let sound = |name: &str| ambience.sounds.iter().position(|s| s.name == format!("base:{name}")).unwrap();
    let played = |name: &str, set: &[(&str, f32)]| {
        let (timeline, _) = run(&ambience, &scene(&ambience, "home", &["grassland", "temperate", "open"], set), 300.0);
        timeline.iter().filter(|(_, c)| matches!(c, Cue::Sound { sound: s, .. } if *s == sound(name))).count()
    };
    let home = [&INDOORS[..], &[("timber", 1.0), ("rain", 0.0)]].concat();
    let at_night = [&home[..], &[("sun", -0.5)]].concat();
    assert!(played("rafters", &at_night) > 0, "a timber home at night");
    assert_eq!(played("rafters", &home), 0, "by day");
    assert_eq!(played("rafters", &[("timber", 1.0), ("sun", -0.5)]), 0, "outdoors by a timber wall");
    let after_rain = [("roof", 1.0), ("wetness", 0.9), ("raining", 0.0)];
    assert!(played("eaves_drip", &after_rain) > 0, "under a roof after the rain");
    assert_eq!(played("eaves_drip", &[("roof", 1.0), ("wetness", 0.9), ("raining", 1.0)]), 0, "while it rains");
    assert_eq!(played("eaves_drip", &[("roof", 1.0), ("wetness", 0.0)]), 0, "the ground dry");
    assert_eq!(played("eaves_drip", &[("wetness", 0.9)]), 0, "out in the open");
}

/// Bees drone by their hive on a warm day, and keep in at night, in the cold and the rain.
#[test]
fn bees_hum_by_their_hive_on_a_warm_day() {
    let ambience = base();
    let bees = ambience.beds.iter().position(|b| b.name == "base:bees").unwrap();
    let level = |set: &[(&str, f32)]| run(&ambience, &scene(&ambience, "garden", &["grassland", "temperate", "open"], set), 20.0).1[bees];
    let by_the_hive = [("grass", 0.6), ("hive", 1.0), ("temperature", 18.0)];
    assert!(level(&by_the_hive) > 0.8 * ambience.beds[bees].volume, "by a skep on a warm day");
    assert!(level(&[("grass", 0.6), ("temperature", 18.0)]) < 1e-3, "no hive near");
    for (weather, value) in [("sun", -0.5), ("temperature", 3.0), ("rain", 1.0)] {
        let set: Vec<_> = by_the_hive.iter().copied().filter(|(axis, _)| *axis != weather).chain([(weather, value)]).collect();
        assert!(level(&set) < 0.2 * ambience.beds[bees].volume, "{weather} {value}: the bees keep in");
    }
}

#[test]
fn caves_drip_by_their_rock_and_depth() {
    let ambience = base();
    let bed = |name: &str| ambience.beds.iter().position(|b| b.name == format!("base:{name}")).unwrap_or_else(|| panic!("no bed {name}"));
    let sound = |name: &str| ambience.sounds.iter().position(|s| s.name == format!("base:{name}")).unwrap_or_else(|| panic!("no sound {name}"));
    let (wet, dry, seep) = (bed("cave"), bed("dry_cave"), bed("cave_seep"));
    let cave = |moisture: f32, depth: f32, reverb: f32, water: f32| {
        let set = [("rock", 1.0), ("sky", 0.0), ("air", 0.3), ("reverb", reverb), ("depth", depth), ("moisture", moisture), ("water", water)];
        scene(&ambience, "cave", &["mountain"], &set)
    };
    let heard = |scene: &Scene| {
        let (timeline, levels) = run(&ambience, scene, 300.0);
        let played = ["drip", "deep_drip", "far_drip", "grit"].map(|name| timeline.iter().filter(|(_, c)| matches!(c, Cue::Sound { sound: s, .. } if *s == sound(name))).count());
        (levels, played)
    };
    // Wet rock a dozen blocks down: its air and the first drips, not yet the deep ones.
    let (levels, [drips, deep, _, grit]) = heard(&cave(1.4, 12.0, 0.3, 0.0));
    assert!(levels[wet] > 0.8 * ambience.beds[wet].volume && levels[dry] < 1e-3, "wet rock's air {:.3}, dry rock's {:.3}", levels[wet], levels[dry]);
    assert!(drips > 0 && deep == 0 && grit == 0, "shallow wet cave: {drips} drips, {deep} deep, {grit} grit");
    // Deep and shut in: the deep tier joins, more drips in all.
    let (_, [deeper, deep, far, _]) = heard(&cave(1.4, 40.0, 0.8, 0.0));
    assert!(deep > 0 && far > 0 && deeper + deep > 2 * drips, "deep: {deeper} + {deep} drips against {drips} shallow ({far} far)");
    // Dry rock, as deep: still air and grit, nothing dripping.
    let (levels, [drips, deep, far, grit]) = heard(&cave(0.2, 40.0, 0.8, 0.0));
    assert!(levels[dry] > 0.8 * ambience.beds[dry].volume && levels[wet] < 1e-3, "dry rock's air {:.3}, wet rock's {:.3}", levels[dry], levels[wet]);
    assert!(drips + deep + far == 0 && grit > 0, "dry cave: {drips} + {deep} + {far} drips, {grit} grit");
    // A pool seeping in, dry rock or wet; none without water.
    for moisture in [0.2, 1.4] {
        let (levels, _) = heard(&cave(moisture, 20.0, 0.5, 0.3));
        assert!(levels[seep] > 0.5 * ambience.beds[seep].volume, "seep at moisture {moisture}: {:.3}", levels[seep]);
    }
    assert!(heard(&cave(1.4, 20.0, 0.5, 0.0)).0[seep] < 1e-3);
}

/// A survey still out when the player leaves a game is of the world left: its answer,
/// coming back after, is dropped, not heard in the next world (a village's murmur where
/// there is none), and the next game's first survey does not wait on it.
#[test]
fn a_survey_of_the_world_left_is_not_heard_in_the_next() {
    let answer = |number: u64, temperature: f32| super::Survey { number, enclosure: Default::default(), place: Some(super::Place { temperature, ..Default::default() }) };
    let mut surveys = super::Surveys::new();
    // Survey 3 goes out in the first world, and the player leaves before it comes back.
    surveys.out = true;
    surveys.leave(3);
    assert!(!surveys.out, "the next game's first survey waits on one of the world left");
    // In the next world survey 4 goes out; 3's answer comes back first, then 4's.
    surveys.out = true;
    surveys.tx.send(answer(3, 0.9)).unwrap();
    assert!(surveys.answers().is_empty(), "the answer of a survey of the world left is taken");
    assert!(surveys.out, "an old answer is no answer to the survey out now");
    surveys.tx.send(answer(4, 0.2)).unwrap();
    let answers = surveys.answers();
    assert_eq!(answers.iter().map(|s| (s.number, s.place.as_ref().map(|p| p.temperature))).collect::<Vec<_>>(), [(4, Some(0.2))]);
    assert!(!surveys.out);
}

/// The beds wait for every recorded clip to load (or fail) before any plays, so a muffled
/// twin, made at once from the same recording, never starts ahead of its dry clip.
#[test]
fn recorded_beds_and_their_muffled_twins_start_together() {
    // Bed 0 synthesized (there at once), bed 1 recorded in two clips.
    let beds = vec![vec![0], vec![1, 2]];
    assert!(!super::beds_ready(&beds, |&clip| clip != 2), "a recording still loading");
    assert!(super::beds_ready(&beds, |_| true));
    assert!(super::beds_ready(&Vec::<Vec<u8>>::new(), |_| false), "no beds, nothing to wait on");
}

/// Leaving a game drops the surveys still out (the `leave` system, not only `Surveys`): an
/// answer of the world left that comes back after is not taken in the next, and the next
/// game's first survey goes out without waiting on it.
#[test]
fn leaving_drops_the_surveys_still_out() {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::World;
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let ambience = Ambience::load(&content.files).unwrap();
    let kinds = std::sync::Arc::new(super::kinds(&ambience, &content.blocks, &content.multiblocks, &voxmar_server::wall_shelters(&content)));
    let mut ambient = super::Ambient::new(ambience, Vec::new(), kinds, content, crossbeam_channel::unbounded().1);
    ambient.surveys.out = true;
    let mut world = World::new();
    world.insert_resource(ambient);
    world.insert_resource(super::Surroundings { started: 7, surveyed: 7, ..Default::default() });
    world.run_system_once(super::leave).unwrap();
    let mut ambient = world.resource_mut::<super::Ambient>();
    assert!(!ambient.surveys.out, "the next game's first survey waits on one of the world left");
    let old = super::Survey { number: 7, enclosure: Default::default(), place: Some(Default::default()) };
    ambient.surveys.tx.send(old).unwrap();
    assert!(ambient.surveys.answers().is_empty(), "survey 7, of the world left, is taken in the next");
}

/// The clips go to the mixer only once the recordings have loaded (`receive_clips`): with
/// one still loading the beds wait, muffled twins and all.
#[test]
fn the_beds_wait_for_their_recordings() {
    use bevy::asset::{AssetApp, AssetPlugin, Assets, Handle, uuid::Uuid};
    use bevy::audio::AudioSource;
    use bevy::ecs::system::RunSystemOnce;
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let content = voxmar_server::load_content(&assets).unwrap();
    let ambience = Ambience::load(&content.files).unwrap();
    let kinds = std::sync::Arc::new(super::kinds(&ambience, &content.blocks, &content.multiblocks, &voxmar_server::wall_shelters(&content)));
    let mut ambient = super::Ambient::new(ambience, Vec::new(), kinds, content, crossbeam_channel::unbounded().1);
    let mut app = bevy::app::App::new();
    app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default())).init_asset::<AudioSource>();
    let clip = || AudioSource { bytes: std::sync::Arc::from(Vec::new()) };
    let made = app.world_mut().resource_mut::<Assets<AudioSource>>().add(clip());
    let recorded = Handle::<AudioSource>::Uuid(Uuid::from_u128(2), Default::default());
    ambient.loading = Some(super::Clips { beds: vec![vec![made.clone()], vec![recorded.clone()]], muffled: vec![vec![], vec![made]], sounds: Vec::new() });
    app.world_mut().insert_resource(ambient);
    app.world_mut().run_system_once(super::receive_clips).unwrap();
    assert!(app.world().resource::<super::Ambient>().clips.is_none(), "the beds play while a recording still loads");
    app.world_mut().resource_mut::<Assets<AudioSource>>().insert(recorded.id(), clip()).unwrap();
    app.world_mut().run_system_once(super::receive_clips).unwrap();
    let ambient = app.world().resource::<super::Ambient>();
    assert!(ambient.clips.is_some() && ambient.loading.is_none());
}
