//! Ambient sound: layered beds and placed one-shots that follow where the player is.
//!
//! - **Where the player is** is an environment vector ([`defs::Environment`]): how enclosed
//!   and how open to the sky (a flood fill from the eye, [`enclosure`]), what is near
//!   (leaves, water, grass, rock), the climate and biome from the world generator, height
//!   above sea and above the land around, depth underground, the weather and the time of
//!   day.
//! - **What plays** is data: every package's `ambience.ron` ([`defs`]) lists beds and
//!   one-shots with soft-range conditions on that vector, so a mod adds ambience like the
//!   base game.
//! - **How it sounds** is synthesized at startup ([`synth`]); the [`mixer`] decides
//!   volumes and timing. Beds are chained segments that crossfade and never repeat a
//!   variant back to back; one-shots sound from real blocks (birds in trees, frogs in
//!   water), panned by direction, and ring in caves with a reverb as wet as the place is
//!   enclosed.
//!
//! Nothing here runs per voxel per frame: the scan runs on a worker about once a second
//! and the rest is a few dozen numbers per frame.

pub mod defs;
pub mod enclosure;
pub mod mixer;
pub mod synth;
#[cfg(test)]
pub(crate) mod tests;

use std::sync::{Arc, OnceLock};

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, GlobalVolume, PlaybackSettings, Volume};
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rayon::prelude::*;
use voxmar_engine::block::{BlockRegistry, RenderKind};
use voxmar_engine::content::Content;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::TICKS_PER_DAY;
use voxmar_worldgen::{Body, Overworld, SEA_LEVEL, WorldContent};

use self::defs::{Ambience, BedSound, Environment, Fixed};
use self::enclosure::{Enclosure, Kinds};
use self::mixer::{Cue, Mixer};
use self::synth::Synth;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::session::Launch;
use crate::settings::Settings;
use crate::sfx::{self, mix};
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

/// Seconds between scans of the surroundings.
const SURVEY_INTERVAL: f32 = 1.0;
/// Seconds over which the environment follows a new scan.
const SETTLE: f32 = 2.0;
/// Ambience sits under everything else: every bed and one-shot is this much quieter than
/// its `volume` in the data says, relative to the sound effects.
const AMBIENCE_GAIN: f32 = 0.65;

pub struct AmbiencePlugin;

/// What the ambience's scan finds around the player, for the rest of the client: how many
/// flowers are near, 0..1 (the `flowers` group of `ambience.ron`).
#[derive(Resource, Default)]
pub struct Surroundings {
    pub flowers: f32,
}

impl Plugin for AmbiencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Surroundings>()
            .add_systems(Startup, start)
            // Playing runs muted too (it starts nothing then, as its level is zero), so
            // captures still log which beds would play.
            .add_systems(Update, (receive_clips, (survey, listen, play).chain().run_if(in_state(Screen::InGame))))
            .add_systems(OnExit(Screen::InGame), leave);
    }
}

/// The synthesized sounds: bed segments ready to play, and one-shots kept as samples to
/// place in stereo each time they play.
struct Clips {
    beds: Vec<Vec<Handle<AudioSource>>>,
    sounds: Vec<Arc<Vec<OneShotClip>>>,
}

struct OneShotClip {
    dry: Vec<f32>,
    wet: [Vec<f32>; 2],
}

/// What the synthesis thread hands back: bed segments as WAV files, and one-shots.
struct Synthesized {
    beds: Vec<(Synth, Vec<Vec<u8>>)>,
    sounds: Vec<(Synth, Arc<Vec<OneShotClip>>)>,
}

/// Climate and terrain at the player, from the world generator.
#[derive(Clone, Default)]
struct Place {
    temperature: f32,
    moisture: f32,
    continentalness: f32,
    river: f32,
    /// `river` by running water, and by still water.
    flowing: f32,
    still: f32,
    forest: f32,
    /// Ground height here, and on average over the land around.
    ground: f32,
    land: f32,
    home_tags: Vec<String>,
    neighbour_tags: Vec<String>,
    home: f32,
}

struct Survey {
    enclosure: Enclosure,
    place: Option<Place>,
}

#[derive(Resource)]
struct Ambient {
    ambience: Arc<Ambience>,
    kinds: Arc<Kinds>,
    content: Content,
    clips: Option<Clips>,
    clips_rx: Receiver<Synthesized>,
    surveys: (Sender<Survey>, Receiver<Survey>),
    surveying: bool,
    since_survey: f32,
    /// The generator of the world being played, made on first use.
    generator: Option<(u64, Arc<OnceLock<Option<Arc<Overworld>>>>)>,
    /// The latest scan (whose sample blocks one-shots sound from), and its measures eased
    /// towards over a couple of seconds.
    enclosure: Enclosure,
    place: Option<Place>,
    smooth: Enclosure,
    env: Environment,
    mixer: Mixer,
    cues: Vec<Cue>,
    /// The audible beds, as last logged.
    logged: String,
}

#[derive(Component)]
struct BedSegment(usize);

/// Reads the ambience definitions and starts synthesizing what they use.
fn start(mut commands: Commands, launch: Res<Launch>, world: Res<ClientWorld>) {
    let ambience = match Ambience::load(&launch.content.files) {
        Ok(ambience) => ambience,
        Err(e) => {
            error!("no ambience: {e}");
            Ambience::default()
        }
    };
    let kinds = Arc::new(kinds(&ambience, &world.registry));
    let bed_synths = ambience.beds.iter().filter_map(|b| match b.sound {
        BedSound::Synth(synth) => Some(synth),
        BedSound::Recorded(_) => None,
    });
    let mut synths: Vec<Synth> = bed_synths.chain(ambience.sounds.iter().map(|s| s.synth)).collect();
    synths.sort_by_key(|s| *s as u8);
    synths.dedup();
    let (tx, clips_rx) = unbounded();
    rayon::spawn(move || {
        let beds = synths
            .par_iter()
            .filter(|s| s.is_bed())
            .map(|&s| (s, (0..synth::BED_VARIANTS).map(|v| synth::bed(s, v)).map(|[l, r]| sfx::wav_stereo(&l, &r)).collect()))
            .collect();
        let sounds = synths
            .par_iter()
            .filter(|s| !s.is_bed())
            .map(|&s| {
                let variants = (0..synth::ONE_SHOT_VARIANTS).map(|v| synth::one_shot(s, v)).map(|(dry, wet)| OneShotClip { dry, wet }).collect();
                (s, Arc::new(variants))
            })
            .collect();
        let _ = tx.send(Synthesized { beds, sounds });
    });
    let groups = ambience.groups.len();
    commands.insert_resource(Ambient {
        mixer: Mixer::new(&ambience, 0x616d_6269),
        ambience: Arc::new(ambience),
        kinds,
        content: launch.content.clone(),
        clips: None,
        clips_rx,
        surveys: unbounded(),
        surveying: false,
        since_survey: SURVEY_INTERVAL,
        generator: None,
        enclosure: Enclosure::default(),
        place: None,
        smooth: Enclosure::default(),
        env: Environment::new(groups),
        cues: Vec::new(),
        logged: String::new(),
    });
}

/// What the enclosure scan needs of each block.
fn kinds(ambience: &Ambience, registry: &BlockRegistry) -> Kinds {
    let ids = (0..registry.len()).map(|id| registry.info(voxmar_engine::block::BlockId(id as u16)));
    Kinds {
        open: ids.clone().map(|info| !info.solid || info.render == RenderKind::Leaves).collect(),
        leaves: ids.map(|info| info.render == RenderKind::Leaves).collect(),
        group: ambience.block_groups(registry),
        groups: ambience.groups.len(),
    }
}

fn receive_clips(ambient: Option<ResMut<Ambient>>, mut sources: ResMut<Assets<AudioSource>>, assets: Res<AssetServer>) {
    let Some(mut ambient) = ambient else { return };
    let Ok(synthesized) = ambient.clips_rx.try_recv() else { return };
    let bed_handles: Vec<(Synth, Vec<Handle<AudioSource>>)> = synthesized
        .beds
        .into_iter()
        .map(|(s, files)| (s, files.into_iter().map(|bytes| sources.add(AudioSource { bytes: Arc::from(bytes) })).collect()))
        .collect();
    let find_bed = |synth: Synth| bed_handles.iter().find(|(s, _)| *s == synth).map(|(_, h)| h.clone()).unwrap_or_default();
    let find_sound = |synth: Synth| synthesized.sounds.iter().find(|(s, _)| *s == synth).map(|(_, c)| c.clone()).unwrap_or_default();
    // Recorded beds load from their package like any other asset.
    let beds = ambient
        .ambience
        .beds
        .iter()
        .map(|b| match &b.sound {
            BedSound::Synth(synth) => find_bed(*synth),
            BedSound::Recorded(paths) => paths.iter().map(|path| assets.load(path.clone())).collect(),
        })
        .collect();
    let sounds = ambient.ambience.sounds.iter().map(|s| find_sound(s.synth)).collect();
    ambient.clips = Some(Clips { beds, sounds });
}

/// Scans the surroundings on a worker about once a second, and eases the environment
/// towards what the last scan found.
fn survey(
    time: Res<Time>,
    world: Res<ClientWorld>,
    player: Res<LocalPlayer>,
    mut ambient: ResMut<Ambient>,
) {
    let ambient = &mut *ambient;
    while let Ok(survey) = ambient.surveys.1.try_recv() {
        ambient.surveying = false;
        ambient.enclosure = survey.enclosure;
        if survey.place.is_some() {
            ambient.place = survey.place;
        }
    }
    let dt = time.delta_secs();
    let ease = (dt / SETTLE).min(1.0);
    let (smooth, latest) = (&mut ambient.smooth, &ambient.enclosure);
    if smooth.groups.len() != latest.groups.len() {
        *smooth = latest.clone();
    } else {
        for (value, target) in [(&mut smooth.air, latest.air), (&mut smooth.sky, latest.sky), (&mut smooth.canopy, latest.canopy)] {
            *value += (target - *value) * ease;
        }
        for (value, target) in smooth.groups.iter_mut().zip(&latest.groups) {
            *value += (target - *value) * ease;
        }
    }

    ambient.since_survey += dt;
    if !player.joined || ambient.surveying || ambient.since_survey < SURVEY_INTERVAL {
        return;
    }
    ambient.since_survey = 0.0;
    let eye = player.eye().floor().as_ivec3();
    let center = ColumnPos::of_block(eye);
    let mut snapshot = VoxelWorld::default();
    for dz in -1..=1 {
        for dx in -1..=1 {
            if let Some(column) = world.voxels.column(center.offset(dx, dz)) {
                snapshot.insert(center.offset(dx, dz), column.clone());
            }
        }
    }
    if ambient.generator.as_ref().is_none_or(|(seed, _)| *seed != player.seed) {
        ambient.generator = Some((player.seed, Arc::new(OnceLock::new())));
    }
    let (seed, generator) = ambient.generator.clone().expect("set above");
    let (kinds, content, tx) = (ambient.kinds.clone(), ambient.content.clone(), ambient.surveys.0.clone());
    let random = mix(time.elapsed().as_nanos() as u64);
    ambient.surveying = true;
    rayon::spawn(move || {
        let enclosure = enclosure::scan(eye, |p| snapshot.block(p), &kinds, random);
        let generator = generator.get_or_init(|| match WorldContent::load(&content) {
            Ok(world) => Some(Overworld::shared(seed, &content, &world)),
            Err(e) => {
                warn!("ambience has no climate: {e}");
                None
            }
        });
        let place = generator.as_ref().map(|generator| place(generator, eye));
        let _ = tx.send(Survey { enclosure, place });
    });
}

/// The climate, biome and lie of the land at `eye`.
fn place(generator: &Overworld, eye: IVec3) -> Place {
    let site = generator.site(eye.x, eye.z);
    let biomes = generator.biomes();
    let mut land = 0.0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            land += generator.site(eye.x + dx * 24, eye.z + dz * 24).height as f32 / 9.0;
        }
    }
    let edge = (site.edge / 48.0).clamp(0.0, 1.0);
    Place {
        temperature: site.temperature,
        moisture: site.moisture,
        continentalness: site.continentalness,
        river: site.river,
        flowing: if matches!(site.body, Body::Creek | Body::River) { site.river } else { 0.0 },
        still: if matches!(site.body, Body::Lake | Body::Spring) { site.river } else { 0.0 },
        forest: site.forest,
        ground: site.height as f32,
        land,
        home_tags: biomes.get(site.home).tags.clone(),
        neighbour_tags: biomes.get(site.neighbour).tags.clone(),
        home: 0.5 + 0.5 * edge * edge * (3.0 - 2.0 * edge),
    }
}

/// Fills in the environment vector from the scan, the place, the weather and the time.
fn listen(
    player: Res<LocalPlayer>,
    weather: Res<ClientWeather>,
    cycle: Res<DayCycle>,
    mut ambient: ResMut<Ambient>,
    mut surroundings: ResMut<Surroundings>,
) {
    let ambient = &mut *ambient;
    let (env, smooth) = (&mut ambient.env, &ambient.smooth);
    env.set(Fixed::Air, smooth.air);
    env.set(Fixed::Sky, smooth.sky);
    env.set(Fixed::Canopy, smooth.canopy);
    let enclosed = (1.0 - smooth.sky) * ((smooth.air - 0.02) / 0.28).clamp(0.0, 1.0);
    env.set(Fixed::Reverb, enclosed);
    for (group, &count) in smooth.groups.iter().enumerate() {
        let presence = ambient.ambience.presence(group, count);
        env.set_group(group, presence);
        if short(&ambient.ambience.groups[group].name) == "flowers" {
            surroundings.flowers = presence;
        }
    }
    let eye = player.eye().y;
    env.set(Fixed::Height, eye - SEA_LEVEL as f32);
    if let Some(place) = &ambient.place {
        env.set(Fixed::Temperature, place.temperature);
        env.set(Fixed::Moisture, place.moisture);
        env.set(Fixed::Continentalness, place.continentalness);
        env.set(Fixed::River, place.river);
        env.set(Fixed::Flowing, place.flowing);
        env.set(Fixed::Still, place.still);
        env.set(Fixed::Forest, place.forest);
        env.set(Fixed::Relief, eye - place.land);
        env.set(Fixed::Depth, place.ground - eye);
        env.home_tags.clone_from(&place.home_tags);
        env.neighbour_tags.clone_from(&place.neighbour_tags);
        env.home = place.home;
    }
    let shown = &weather.current;
    let falling = shown.precipitation * (1.0 - shown.snow);
    env.set(Fixed::Rain, falling * smooth.sky.max(0.3 * smooth.air));
    env.set(Fixed::Raining, falling);
    env.set(Fixed::Wetness, shown.wetness);
    env.set(Fixed::Snow, shown.precipitation * shown.snow);
    env.set(Fixed::Storm, shown.precipitation * shown.convection);
    env.set(Fixed::Wind, shown.wind.length());
    env.set(Fixed::Gusts, shown.gusts);
    // Hail and sleet are heard where they land: out in the open, or on the roof overhead.
    let heard = smooth.sky.max(0.3 * smooth.air);
    env.set(Fixed::Hail, shown.precipitation * shown.hail * heard);
    env.set(Fixed::Sleet, shown.precipitation * shown.sleet * heard);
    env.set(Fixed::Dust, shown.dust * smooth.sky.max(0.15 * smooth.air));
    env.set(Fixed::Cover, shown.cover);
    env.set(Fixed::Sun, voxmar_engine::calendar::sun_height((cycle.ticks / TICKS_PER_DAY as f64).rem_euclid(1.0)) as f32);
    env.set(Fixed::Underwater, cycle.underwater as u8 as f32);
}

/// Runs the mixer and carries out what it asks for.
#[allow(clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    settings: Res<Settings>,
    global: Res<GlobalVolume>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut segments: Query<(Entity, &BedSegment, &mut AudioSink)>,
    mut sources: ResMut<Assets<AudioSource>>,
    mut ambient: ResMut<Ambient>,
) {
    let ambient = &mut *ambient;
    let (Some(clips), Ok(camera), true) = (&ambient.clips, camera.single(), player.joined) else { return };
    // Bevy applies the global volume only as a sound starts, and beds play on (see main.rs).
    let master = AMBIENCE_GAIN * sfx::MASTER * settings.sound_volume as f32 / 100.0 * settings.ambience_volume as f32 / 100.0
        * global.volume.to_linear();
    let ears = camera.translation();
    let mut cues = std::mem::take(&mut ambient.cues);
    let length = |sound: usize, variant: usize| clips.sounds[sound].get(variant).map_or(1.0, |c| c.dry.len() as f32 / sfx::RATE as f32);
    let variants = (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS);
    ambient.mixer.update(&ambient.ambience, &ambient.env, &ambient.enclosure.samples, ears, time.delta_secs(), length, variants, &mut cues);
    let audible = master > 0.0;
    for cue in cues.drain(..) {
        if let Cue::Sound { sound, .. } = cue {
            debug!("ambience: {}", short(&ambient.ambience.sounds[sound].name));
        }
        match cue {
            Cue::Segment { .. } | Cue::Sound { .. } if !audible => {}
            Cue::Segment { bed, variant } => {
                let Some(clip) = clips.beds[bed].get(variant) else { continue };
                let volume = Volume::Linear(ambient.mixer.level(bed) * master);
                commands.spawn((BedSegment(bed), AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(volume)));
            }
            Cue::Stop { bed } => {
                for (entity, segment, _) in &segments {
                    if segment.0 == bed {
                        commands.entity(entity).despawn();
                    }
                }
            }
            Cue::Sound { sound, variant, at, volume, speed } => {
                let Some(clip) = clips.sounds[sound].get(variant) else { continue };
                let reverb = ambient.env.get(Fixed::Reverb);
                let [left, right] = mixer::place(&clip.dry, &clip.wet, at, ears, camera.right().into(), camera.forward().into(), reverb);
                let source = sources.add(AudioSource { bytes: Arc::from(sfx::wav_stereo(&left, &right)) });
                let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume * master)) };
                commands.spawn((AudioPlayer(source), playback));
            }
        }
    }
    ambient.cues = cues;
    for (_, segment, mut sink) in &mut segments {
        sink.set_volume(Volume::Linear(ambient.mixer.level(segment.0) * master));
    }
    // Say which beds are playing whenever that changes, for tuning.
    let beds: Vec<&str> = (ambient.ambience.beds.iter().enumerate())
        .filter(|&(i, bed)| ambient.mixer.level(i) > 0.25 * bed.volume)
        .map(|(_, bed)| short(&bed.name))
        .collect();
    let beds = beds.join(", ");
    if beds != ambient.logged {
        let env = &ambient.env;
        let [air, sky, reverb, relief, depth] = [Fixed::Air, Fixed::Sky, Fixed::Reverb, Fixed::Relief, Fixed::Depth].map(|a| env.get(a));
        info!("ambience: [{beds}] air {air:.2} sky {sky:.2} reverb {reverb:.2} relief {relief:.0} depth {depth:.0}");
        ambient.logged = beds;
    }
}

/// A layer's name without its package, for logs.
fn short(name: &str) -> &str {
    name.split_once(':').map_or(name, |(_, name)| name)
}

/// Leaving a game silences the beds and forgets the place.
fn leave(mut commands: Commands, segments: Query<Entity, With<BedSegment>>, ambient: Option<ResMut<Ambient>>) {
    for entity in &segments {
        commands.entity(entity).despawn();
    }
    let Some(mut ambient) = ambient else { return };
    let groups = ambient.ambience.groups.len();
    ambient.mixer = Mixer::new(&ambient.ambience, 0x616d_6269);
    ambient.enclosure = Enclosure::default();
    ambient.smooth = Enclosure::default();
    ambient.place = None;
    ambient.env = Environment::new(groups);
    ambient.since_survey = SURVEY_INTERVAL;
}
