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
//! Nothing here runs per voxel per frame: the scan runs on a worker about once a second,
//! the roof look a few times a second over the handful of columns the player passed under,
//! and the rest is a few dozen numbers per frame.

pub mod defs;
pub mod enclosure;
pub mod mixer;
pub mod synth;
#[cfg(test)]
pub(crate) mod tests;

use std::sync::{Arc, OnceLock};

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, GlobalVolume, PlaybackSettings, Volume};
use bevy::math::Vec3Swizzles;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rayon::prelude::*;
use voxmar_engine::block::{BlockRegistry, RenderKind};
use voxmar_engine::content::Content;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::TICKS_PER_DAY;
use voxmar_worldgen::{Body, Overworld, SEA_LEVEL, WorldContent};

use self::defs::{Ambience, BedSound, Environment, Fixed, NO_SETTLEMENT};
use self::enclosure::{Enclosure, Kinds};
use self::mixer::{Cue, Mixer};
use self::synth::Synth;
use crate::menu::Screen;
use crate::mix::{Category, Voice};
use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::session::Launch;
use crate::settings::Settings;
use crate::sfx::{self, mix};
use crate::weather::ClientWeather;
use crate::world::ClientWorld;

/// Seconds between scans of the surroundings.
const SURVEY_INTERVAL: f32 = 1.0;
/// Seconds between looks for a roof overhead.
pub const ROOF_LOOK: f32 = 0.25;
/// Seconds over which the environment follows a new scan.
const SETTLE: f32 = 2.0;
/// How far from the player settlements are looked for, in blocks: past this, none is heard.
const SETTLEMENTS_HEARD: i32 = 192;
/// Ambience sits under everything else: every bed and one-shot is this much quieter than
/// its `volume` in the data says, relative to the sound effects.
const AMBIENCE_GAIN: f32 = 0.65;

pub struct AmbiencePlugin;

/// What the ambience's scan finds around the player, for the rest of the client: how many
/// flowers are near, 0..1 (the `flowers` group of `ambience.ron`), how far below the
/// ground surface the player is, in blocks, and how much of the air around is open to the
/// sky, 0..1 (for the music underground).
#[derive(Resource, Default)]
pub struct Surroundings {
    pub flowers: f32,
    pub depth: f32,
    pub sky: f32,
    /// The tags of the biome the player is in (`biomes.ron`), for the music of a place.
    pub biome: Vec<String>,
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
    /// Blocks to the middle of the nearest settlement, across the ground.
    settlement: f32,
}

struct Survey {
    enclosure: Enclosure,
    place: Option<Place>,
}

/// The ambience as the launch checks loaded it, and each block's roof patter they worked
/// out (see [`Ambience::roof_patters`]), handed on so neither is made twice.
#[derive(Resource)]
pub struct Checked(pub Ambience, pub Vec<Option<u8>>);

/// The roof the player is under, looked for every [`ROOF_LOOK`] seconds.
#[derive(Default)]
struct RoofLook {
    since: f32,
    /// Where the player's feet were at the last look.
    from: Option<Vec3>,
    /// The roof found then: the block, under any column the player passed beneath since the
    /// look before.
    under: Option<voxmar_engine::block::BlockId>,
}

#[derive(Resource)]
struct Ambient {
    ambience: Arc<Ambience>,
    /// Each block's roof patter (see [`Ambience::roof_patters`]).
    roof_patters: Vec<Option<u8>>,
    roof: RoofLook,
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
fn start(mut commands: Commands, launch: Res<Launch>, world: Res<ClientWorld>, checked: Option<ResMut<Checked>>) {
    let (ambience, roof_patters) = match checked {
        Some(mut checked) => {
            commands.remove_resource::<Checked>();
            (std::mem::take(&mut checked.0), std::mem::take(&mut checked.1))
        }
        None => {
            let ambience = Ambience::load(&launch.content.files).unwrap_or_else(|e| {
                error!("no ambience: {e}");
                Ambience::default()
            });
            // Without the launch's check (and the sound effects' building materials), roofs
            // patter by their family sound, or not at all.
            let patters = ambience.roof_patters(&world.registry, &[], |id| defs::family(world.registry.info(id).sound)).unwrap_or_default();
            (ambience, patters)
        }
    };
    let kinds = Arc::new(kinds(&ambience, &world.registry, &launch.content.multiblocks));
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
    let axes = ambience.axes();
    commands.insert_resource(Ambient {
        mixer: Mixer::new(&ambience, 0x616d_6269),
        roof_patters,
        roof: RoofLook::default(),
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
        env: Environment::new(axes),
        cues: Vec::new(),
        logged: String::new(),
    });
}

/// What the enclosure scan needs of each block.
fn kinds(ambience: &Ambience, registry: &BlockRegistry, multiblocks: &voxmar_engine::multiblock::MultiblockRegistry) -> Kinds {
    let ids = (0..registry.len()).map(|id| registry.info(voxmar_engine::block::BlockId(id as u16)));
    let group = ambience.block_groups(registry);
    // A structure's core in a group heard only while lit.
    let lit_only = group
        .iter()
        .enumerate()
        .map(|(id, g)| g.is_some_and(|g| ambience.groups[g as usize].lit) && multiblocks.with_core(voxmar_engine::block::BlockId(id as u16)).next().is_some())
        .collect();
    Kinds {
        open: ids.clone().map(|info| !info.solid || info.render == RenderKind::Leaves).collect(),
        leaves: ids.map(|info| info.render == RenderKind::Leaves).collect(),
        group,
        lit_only,
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

    ambient.roof.since += dt;
    if player.joined && ambient.roof.since >= ROOF_LOOK {
        ambient.roof.since = 0.0;
        let to = player.position;
        let from = ambient.roof.from.replace(to).unwrap_or(to);
        ambient.roof.under = roof_over(|p| world.voxels.block(p), from, to, ambient.ambience.roofs.height, |b| defs::is_roof(&world.registry, b));
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
    let fires = world.fires.clone();
    let random = mix(time.elapsed().as_nanos() as u64);
    ambient.surveying = true;
    rayon::spawn(move || {
        let enclosure = enclosure::scan(eye, |p| snapshot.block(p), |p| fires.contains(&p), &kinds, random);
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

/// The roof over a player who walked from `from` to `to` (feet) since the last look: the
/// lowest roof block (`is_roof`) within `height` blocks above the head over any column the
/// body passed beneath, so stepping under an eave, or under a beam and out again between
/// two looks, is not missed. A few columns, a dozen blocks each.
pub fn roof_over(block: impl Fn(IVec3) -> Option<voxmar_engine::block::BlockId>, from: Vec3, to: Vec3, height: i32, is_roof: impl Fn(voxmar_engine::block::BlockId) -> bool) -> Option<voxmar_engine::block::BlockId> {
    use voxmar_engine::physics::player::{HEIGHT, WIDTH};
    let half = WIDTH / 2.0;
    // Every column the body's footprint covered, sampled along the way a little closer
    // than its width.
    let steps = (from.xz().distance(to.xz()) / (WIDTH * 0.9)).ceil().clamp(0.0, 64.0) as usize;
    let mut columns: Vec<(i32, i32)> = Vec::new();
    for i in 0..=steps {
        let at = from.lerp(to, if steps == 0 { 1.0 } else { i as f32 / steps as f32 });
        for (dx, dz) in [(-half, -half), (half, -half), (-half, half), (half, half)] {
            let column = ((at.x + dx).floor() as i32, (at.z + dz).floor() as i32);
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
    }
    let head = (from.y.min(to.y) + HEIGHT).floor() as i32;
    let mut found: Option<(i32, voxmar_engine::block::BlockId)> = None;
    for (x, z) in columns {
        for y in head..=head + height {
            if found.is_some_and(|(lowest, _)| y >= lowest) {
                break;
            }
            match block(IVec3::new(x, y, z)) {
                Some(b) if is_roof(b) => {
                    found = Some((y, b));
                    break;
                }
                _ => {}
            }
        }
    }
    found.map(|(_, b)| b)
}

/// What the gate hears of the roof overhead: the patter chosen (its name, from the roof
/// block's material), how much the open rain is muffled (`sheltered`), and how fully each
/// bed playing on a patter would play here now. Nothing before the ambience has started.
pub fn roof_heard(world: &World) -> Option<RoofHeard> {
    let ambient = world.get_resource::<Ambient>()?;
    let roofs = &ambient.ambience.roofs;
    let patter = (0..roofs.patters.len()).find(|&p| ambient.env.values[ambient.ambience.patter_axis(p)] > 0.0).map(|p| roofs.patters[p].clone());
    let beds = ambient
        .ambience
        .beds
        .iter()
        .filter(|bed| (0..roofs.patters.len()).any(|p| bed.when.uses_axis(ambient.ambience.patter_axis(p))))
        .map(|bed| (short(&bed.name).to_owned(), bed.when.weight(&ambient.env)))
        .collect();
    // The beds outside that a roof turns down (`duck` by `sheltered`), and how far now.
    let sheltered = ambient.env.get(Fixed::Sheltered);
    let muffled = ambient
        .ambience
        .beds
        .iter()
        .filter_map(|bed| {
            let amount = bed.duck.iter().find(|(by, _)| *by == defs::DuckBy::Axis(Fixed::Sheltered as usize))?.1;
            Some((short(&bed.name).to_owned(), 1.0 - amount * sheltered.clamp(0.0, 1.0)))
        })
        .collect();
    Some(RoofHeard { under: ambient.roof.under, patter, sheltered, beds, muffled })
}

/// See [`roof_heard`].
#[derive(Debug)]
pub struct RoofHeard {
    pub under: Option<voxmar_engine::block::BlockId>,
    pub patter: Option<String>,
    pub sheltered: f32,
    pub beds: Vec<(String, f32)>,
    /// Each bed a roof turns down, and the share of it left.
    pub muffled: Vec<(String, f32)>,
}

/// The patter a roof of `block` makes in rain, by name, as the ambience data says.
pub fn patter_of(world: &World, block: voxmar_engine::block::BlockId) -> Option<String> {
    let ambient = world.get_resource::<Ambient>()?;
    let p = (*ambient.roof_patters.get(block.0 as usize)?)?;
    Some(ambient.ambience.roofs.patters[p as usize].clone())
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
    let settlement = generator
        .settlements_near(eye.xz(), SETTLEMENTS_HEARD)
        .iter()
        .map(|s| s.centre.xz().as_vec2().distance(eye.xz().as_vec2() + 0.5))
        // The search gives every settlement planned in the regions it touches, some far
        // past hearing.
        .filter(|&distance| distance <= SETTLEMENTS_HEARD as f32)
        .fold(NO_SETTLEMENT, f32::min);
    Place {
        settlement,
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
    world: Res<ClientWorld>,
    weather: Res<ClientWeather>,
    cycle: Res<DayCycle>,
    mut ambient: ResMut<Ambient>,
    mut surroundings: ResMut<Surroundings>,
) {
    let ambient = &mut *ambient;
    let registry = &world.registry;
    let (env, smooth) = (&mut ambient.env, &ambient.smooth);
    env.set(Fixed::Air, smooth.air);
    env.set(Fixed::Sky, smooth.sky);
    surroundings.sky = smooth.sky;
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
        surroundings.depth = place.ground - eye;
        if surroundings.biome != place.home_tags {
            surroundings.biome.clone_from(&place.home_tags);
        }
        env.home_tags.clone_from(&place.home_tags);
        env.neighbour_tags.clone_from(&place.neighbour_tags);
        env.home = place.home;
    }
    env.set(Fixed::Settlement, ambient.place.as_ref().map_or(NO_SETTLEMENT, |place| place.settlement));
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
    // Rain on the roof overhead, by what the roof is made of; and the open rain muffled.
    let roofs = &ambient.ambience.roofs;
    let patter = ambient.roof.under.and_then(|b| ambient.roof_patters.get(b.0 as usize).copied().flatten());
    for p in 0..roofs.patters.len() {
        let axis = ambient.ambience.patter_axis(p);
        ambient.env.values[axis] = if patter == Some(p as u8) { falling } else { 0.0 };
    }
    let sheltered = ambient.roof.under.map_or(0.0, |b| roofs.sheltered(|tag| registry.def(b).has_tag(tag), patter.map(usize::from)));
    ambient.env.set(Fixed::Sheltered, sheltered);
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
                // One-shots are voices of the ambience, capped with the rest of it (`mix`);
                // beds are the mixer's own, crossfading, never cut off.
                let voice = at.map_or(Voice::of(Category::Ambience), |at| Voice::at(Category::Ambience, at));
                commands.spawn((AudioPlayer(source), playback, voice));
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

/// What the settlement around is heard as, for the gate: the `settlement` axis now (blocks
/// to the nearest one's middle), and each one-shot that hangs on it with how fully it would
/// play here at noon, at midnight, and here with no settlement near, all at the surface
/// (the player may have dropped into the well on the square). Nothing before the ambience
/// has started.
pub fn settlement_heard(world: &World) -> Option<(f32, Vec<(String, [f32; 3])>)> {
    let ambient = world.get_resource::<Ambient>()?;
    let env = &ambient.env;
    let with = |axis: Fixed, value: f32| {
        let mut env = env.clone();
        env.set(Fixed::Depth, 0.0);
        env.set(axis, value);
        env
    };
    let (noon, midnight, alone) = (with(Fixed::Sun, 1.0), with(Fixed::Sun, -1.0), with(Fixed::Settlement, NO_SETTLEMENT));
    let sounds = ambient
        .ambience
        .sounds
        .iter()
        .filter(|s| s.when.uses(Fixed::Settlement))
        .map(|s| (short(&s.name).to_owned(), [&noon, &midnight, &alone].map(|env| s.when.weight(env))))
        .collect();
    Some((env.get(Fixed::Settlement), sounds))
}

/// A layer's name without its package, for logs.
fn short(name: &str) -> &str {
    name.split_once(':').map_or(name, |(_, name)| name)
}

/// Leaving a game silences the beds and forgets the place.
fn leave(
    mut commands: Commands,
    segments: Query<Entity, With<BedSegment>>,
    ambient: Option<ResMut<Ambient>>,
    mut surroundings: ResMut<Surroundings>,
) {
    for entity in &segments {
        commands.entity(entity).despawn();
    }
    // The next world starts from nothing found, not deep under the last one.
    *surroundings = Surroundings::default();
    let Some(mut ambient) = ambient else { return };
    let axes = ambient.ambience.axes();
    ambient.mixer = Mixer::new(&ambient.ambience, 0x616d_6269);
    ambient.roof = RoofLook::default();
    ambient.enclosure = Enclosure::default();
    ambient.smooth = Enclosure::default();
    ambient.place = None;
    ambient.env = Environment::new(axes);
    ambient.since_survey = SURVEY_INTERVAL;
}
