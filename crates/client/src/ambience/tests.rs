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
        scene(ambience, "porch_in_rain", &["forest", "temperate"], &[("air", 0.6), ("sky", 0.0), ("rain", 0.2), ("raining", 0.7), ("roof_tap", 0.7), ("sheltered", 0.45), ("cover", 1.0)]),
        // In a thatched hut with its door open in a downpour: the straw's hush, the open
        // rain muffled under the sound roof.
        scene(ambience, "thatch_in_rain", &["grassland", "temperate"], &[("air", 0.4), ("sky", 0.3), ("rain", 0.9), ("raining", 0.9), ("roof_soft", 0.9), ("sheltered", 1.0), ("cover", 1.0)]),
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
/// ground, and [`NO_SETTLEMENT`](super::defs::NO_SETTLEMENT) where none is in hearing.
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
    let place = super::place(&generator, near.centre + IVec3::new(3, 2, 4));
    let expected = bevy::math::Vec2::new(3.5, 4.5).length();
    assert!((place.settlement - expected).abs() < 1e-3, "{} blocks off, not {expected}", place.settlement);
    let alone = [IVec2::new(1, 0), IVec2::new(0, 1), IVec2::new(-1, 0), IVec2::new(0, -1)]
        .into_iter()
        .map(|way| near.centre.xz() + way * 700)
        .find(|&at| generator.settlements_near(at, super::SETTLEMENTS_HEARD).is_empty())
        .expect("somewhere 700 blocks off with no settlement in hearing");
    let place = super::place(&generator, IVec3::new(alone.x, near.centre.y, alone.y));
    assert_eq!(place.settlement, super::defs::NO_SETTLEMENT);
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
}
