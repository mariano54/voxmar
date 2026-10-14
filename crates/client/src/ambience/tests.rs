//! The base package's ambience in typical places, and an offline preview of how they sound.

use bevy::math::{IVec2, IVec3, Vec3};
use voxmar_engine::content::ContentFile;

use super::defs::{Ambience, BedSound, Environment, FILE, Fixed};
use super::mixer::{Cue, Mixer, place};
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
    use bevy::audio::{AudioSource, Decodable, Source};
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets").join(path);
    let source = AudioSource { bytes: std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())).into() };
    let decoder = source.decoder();
    assert_eq!(decoder.sample_rate().get(), RATE, "{path}");
    let channels = decoder.channels().get() as usize;
    let samples: Vec<f32> = decoder.collect();
    let channel = |c: usize| samples.iter().skip(c.min(channels - 1)).step_by(channels).copied().collect();
    [channel(0), channel(1)]
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
}

fn scene(ambience: &Ambience, name: &'static str, tags: &[&str], set: &[(&str, f32)]) -> Scene {
    let mut env = Environment::new(ambience.groups.len());
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
    Scene { name, env, samples }
}

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
        scene(ambience, "porch_in_rain", &["forest", "temperate"], &[("air", 0.6), ("sky", 0.0), ("rain", 0.2), ("raining", 0.7), ("cover", 1.0)]),
        scene(ambience, "forest_after_rain", &forest_tags, &[&forest[..], &[("wetness", 0.9), ("cover", 0.6)]].concat()),
        scene(ambience, "lava_cave", &["mountain"], &[("rock", 1.0), ("lava", 0.8), ("sky", 0.0), ("air", 0.3), ("reverb", 0.7), ("depth", 30.0)]),
        scene(ambience, "waterfall", &["river", "water"], &[("water", 0.6), ("falls", 0.9), ("flowing", 1.0), ("rock", 0.4)]),
        scene(ambience, "campfire_night", &["grassland", "temperate", "open"], &[("grass", 1.0), ("fire", 1.0), ("sun", -0.7), ("temperature", 18.0)]),
        scene(ambience, "by_the_hearth", &["forest", "temperate"], &[("fire", 1.0), ("air", 0.5), ("sky", 0.0), ("reverb", 0.1)]),
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
        mixer.update(ambience, &scene.env, &scene.samples, Vec3::ZERO, 0.01, |_, _| 2.0, (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS), &mut cues);
        timeline.extend(cues.drain(..).map(|c| (step as f32 * 0.01, c)));
    }
    let levels = (0..ambience.beds.len()).map(|b| mixer.level(b)).collect();
    (timeline, levels)
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
            "porch_in_rain" => (&["rain_on_roof"], &["downpour"]),
            "forest_after_rain" => (&["dripping_trees"], &["rain", "downpour"]),
            "lava_cave" => (&["lava", "lava_pop", "cave", "far_drip"], &["stream", "lake", "waterfall", "fire", "fire_snap"]),
            // A fire is heard by the fire, crackling and snapping, under the night outdoors.
            "campfire_night" => (&["fire", "fire_snap", "crickets"], &["lava", "lava_pop", "warbler"]),
            "by_the_hearth" => (&["fire", "fire_snap"], &["lava", "breeze", "rain_on_roof"]),
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
    let beds: Vec<Vec<[Vec<f32>; 2]>> =
        ambience.beds.iter().map(|b| (0..synth::BED_VARIANTS).map(|v| segment(&b.sound, v)).collect()).collect();
    type Variants = Vec<(Vec<f32>, [Vec<f32>; 2])>;
    let sounds: Vec<Variants> =
        ambience.sounds.iter().map(|s| (0..synth::ONE_SHOT_VARIANTS).map(|v| synth::one_shot(s.synth, v)).collect()).collect();
    let block = RATE as usize / 100;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for scene in scenes(&ambience) {
        println!("{} starts at {:.0} s", scene.name, left.len() as f32 / RATE as f32);
        let seconds = 20.0;
        let total = (seconds * RATE as f32) as usize;
        let start = left.len();
        left.resize(start + total + RATE as usize * 4, 0.0);
        right.resize(start + total + RATE as usize * 4, 0.0);
        let mut mixer = Mixer::new(&ambience, 9);
        // Skip the warm-up and the beds' fade-in, as if the player had been there a while.
        let mut cues = Vec::new();
        for _ in 0..1000 {
            mixer.update(&ambience, &scene.env, &scene.samples, Vec3::ZERO, 0.01, |_, _| 2.0, (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS), &mut cues);
        }
        cues.clear();
        let mut playing: Vec<(usize, usize, usize)> = (0..ambience.beds.len())
            .filter(|&b| mixer.level(b) > 0.0)
            .map(|b| (b, 0, block * (OVERLAP * 100.0) as usize))
            .collect();
        for step in 0..total / block {
            let at = start + step * block;
            mixer.update(&ambience, &scene.env, &scene.samples, Vec3::ZERO, 0.01, |s, v| sounds[s][v].0.len() as f32 / RATE as f32, (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS), &mut cues);
            for cue in cues.drain(..) {
                match cue {
                    Cue::Segment { bed, variant } => playing.push((bed, variant, 0)),
                    Cue::Stop { bed } => playing.retain(|p| p.0 != bed),
                    Cue::Sound { sound, variant, at: position, volume, .. } => {
                        let (dry, wet) = &sounds[sound][variant];
                        let reverb = scene.env.get(Fixed::Reverb);
                        let [l, r] = place(dry, wet, position, Vec3::ZERO, Vec3::X, Vec3::NEG_Z, reverb);
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
                for i in 0..block {
                    if let Some(s) = l.get(*offset + i) {
                        left[at + i] += s * level;
                        right[at + i] += r[*offset + i] * level;
                    }
                }
                *offset += block;
            }
            playing.retain(|&(bed, variant, offset)| offset < beds[bed][variant][0].len());
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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ambience_preview.wav");
    std::fs::write(&path, sfx::wav_stereo(&left, &right)).unwrap();
    println!("wrote {}", path.display());
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
    let kinds = super::kinds(&ambience, &content.blocks);
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
        let found = super::enclosure::scan(eye, |p| world.block(p), &kinds, 1);
        let place = super::place(&generator, eye);
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
    world.insert_resource(super::Surroundings { flowers: 0.6, depth: 40.0, sky: 0.0, biome: Vec::new() });
    world.run_system_once(super::leave).unwrap();
    let left = world.resource::<super::Surroundings>();
    let fresh = super::Surroundings::default();
    assert_eq!((left.flowers, left.depth, left.sky), (fresh.flowers, fresh.depth, fresh.sky));
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
            Placement::Block(_) | Placement::Listener => 0.0,
        };
        for variant in 0..synth::ONE_SHOT_VARIANTS {
            let (dry, _) = synth::one_shot(sound.synth, variant);
            let near = sound.reach / (sound.reach + distance);
            heard.push((format!("{} {variant}", sound.name), sfx::loudness(&dry, 1) * sound.volume * near * super::AMBIENCE_GAIN));
        }
    }
    all([within(Category::Ambience, heard)]);
}
