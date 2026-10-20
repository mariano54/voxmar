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
/// Blocks round a settlement's buildings still counted as within it (`settlement_edge`).
const SETTLEMENT_MARGIN: i32 = 6;
/// How far off a settlement's buildings are heard from, in blocks (`Building` sounds).
pub const BUILDINGS_HEARD: f32 = 48.0;
/// Ambience sits under everything else: every bed and one-shot is this much quieter than
/// its `volume` in the data says, relative to the sound effects.
const AMBIENCE_GAIN: f32 = 0.65;

pub struct AmbiencePlugin;

/// What the ambience's scan finds around the player, for the rest of the client: how many
/// flowers are near, 0..1 (the `flowers` group of `ambience.ron`), how far below the
/// ground surface the player is, in blocks, and how much of the air around is open to the
/// sky, 0..1 (for the music underground).
#[derive(Resource)]
pub struct Surroundings {
    pub flowers: f32,
    pub depth: f32,
    pub sky: f32,
    /// The tags of the biome the player is in (`biomes.ron`), for the music of a place.
    pub biome: Vec<String>,
    /// How loud thunder is here, against out in the open: less indoors (`ambience.ron`
    /// `shelter`).
    pub thunder: f32,
    /// The nearest settlement (by its id) and how far outside its bounds the player is, in
    /// blocks (0 within), for the settlement motif.
    pub settlement: Option<(u64, f32)>,
    /// How many surveys of the surroundings have been started, counting on across games,
    /// and which of them `settlement` is from (0: none since the last game was left), so
    /// the settlement motif can tell a survey taken where the player now is from one taken
    /// before they joined or respawned.
    pub started: u64,
    pub surveyed: u64,
}

impl Default for Surroundings {
    fn default() -> Self {
        Self { flowers: 0.0, depth: 0.0, sky: 0.0, biome: Vec::new(), thunder: 1.0, settlement: None, started: 0, surveyed: 0 }
    }
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
    /// Each outdoor bed's segments as heard from indoors (none for the rest).
    muffled: Vec<Vec<Handle<AudioSource>>>,
    sounds: Vec<Arc<Vec<OneShotClip>>>,
}

/// A one-shot's variant, kept as 16-bit samples: half the memory of floats, and as fine
/// as the WAV it is played from.
struct OneShotClip {
    dry: Vec<i16>,
    wet: [Vec<i16>; 2],
}

impl OneShotClip {
    fn new((dry, wet): (Vec<f32>, [Vec<f32>; 2])) -> Self {
        let pcm = |samples: Vec<f32>| samples.into_iter().map(mixer::Sample::from_f32).collect();
        let [left, right] = wet;
        Self { dry: pcm(dry), wet: [pcm(left), pcm(right)] }
    }
}

/// A one-shot placed and encoded off the main thread, ready to play.
struct Placed {
    wav: Arc<[u8]>,
    playback: PlaybackSettings,
    at: Option<Vec3>,
}

/// What the synthesis thread hands back: bed segments as WAV files, and one-shots.
struct Synthesized {
    beds: Vec<(Synth, Vec<Vec<u8>>)>,
    /// Outdoor beds' muffled segments, by bed.
    muffled: Vec<(usize, Vec<Vec<u8>>)>,
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
    /// Blocks to the middle of the nearest settlement, across the ground; how far outside
    /// its bounds (0 within); how many live there; and its id.
    settlement: f32,
    edge: f32,
    people: f32,
    id: Option<u64>,
    /// Where each of the ambience's sets of buildings (`Ambience::buildings`) is heard
    /// from, of the nearest settlement's, within [`BUILDINGS_HEARD`].
    buildings: Vec<Vec<Vec3>>,
}

struct Survey {
    /// Which survey this is (`Surroundings::started` when it began).
    number: u64,
    enclosure: Enclosure,
    place: Option<Place>,
}

/// The surveys out on workers and their answers: whether one is out, and the last begun
/// before the player left a game (`Surroundings::started` then), whose answers, still on
/// their way back, are of the world left and are dropped.
struct Surveys {
    tx: Sender<Survey>,
    rx: Receiver<Survey>,
    out: bool,
    left: u64,
}

impl Surveys {
    fn new() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx, out: false, left: 0 }
    }

    /// The answers come back since last asked, of surveys begun in this game, in order.
    fn answers(&mut self) -> Vec<Survey> {
        let mut answers = Vec::new();
        while let Ok(survey) = self.rx.try_recv() {
            if survey.number > self.left {
                self.out = false;
                answers.push(survey);
            }
        }
        answers
    }

    /// The player left the game, with `started` surveys begun: any still out are of the
    /// world left, so the next game's first is not kept waiting on them.
    fn leave(&mut self, started: u64) {
        self.left = started;
        self.out = false;
    }
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
    /// The clips made, waiting on the recorded beds to load (see [`beds_ready`]).
    loading: Option<Clips>,
    clips_rx: Receiver<Synthesized>,
    surveys: Surveys,
    since_survey: f32,
    /// The generator of the world being played, made on first use.
    generator: Option<(u64, Arc<OnceLock<Option<Arc<Overworld>>>>)>,
    /// The latest scan (whose sample blocks one-shots sound from), and its measures eased
    /// towards over a couple of seconds.
    enclosure: Enclosure,
    place: Option<Place>,
    /// Which survey `place` is from.
    place_survey: u64,
    smooth: Enclosure,
    env: Environment,
    mixer: Mixer,
    cues: Vec<Cue>,
    /// One-shots being placed and encoded on a worker, and those done (see [`Placed`]).
    placed: (Sender<Placed>, Receiver<Placed>),
    /// The audible beds, as last logged.
    logged: String,
}

#[derive(Component)]
struct BedSegment(usize);

/// A bed segment's muffled twin, playing alongside it as loud as the place is indoors.
#[derive(Component)]
struct Muffled;

/// A recording in the assets as `[left, right]` at the game's sample rate.
pub(crate) fn decode(file: &std::path::Path) -> Result<[Vec<f32>; 2], String> {
    use bevy::audio::{Decodable, Source};
    let source = AudioSource { bytes: std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?.into() };
    let decoder = source.decoder();
    if decoder.sample_rate().get() != sfx::RATE {
        return Err(format!("{}: not at {} Hz", file.display(), sfx::RATE));
    }
    let channels = decoder.channels().get() as usize;
    let samples: Vec<f32> = decoder.collect();
    let channel = |c: usize| samples.iter().skip(c.min(channels - 1)).step_by(channels).copied().collect();
    Ok([channel(0), channel(1)])
}

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
    let kinds = Arc::new(kinds(&ambience, &world.registry, &launch.content.multiblocks, &voxmar_server::wall_shelters(&launch.content), voxmar_server::roof_scan(&launch.content)));
    let bed_synths = ambience.beds.iter().filter_map(|b| match b.sound {
        BedSound::Synth(synth) => Some(synth),
        BedSound::Recorded(_) => None,
    });
    let mut synths: Vec<Synth> = bed_synths.chain(ambience.sounds.iter().map(|s| s.synth)).collect();
    synths.sort_by_key(|s| *s as u8);
    synths.dedup();
    // Outdoor beds are also made muffled, to play beside them as loud as the place is
    // indoors: a wall takes the brightness, not just the loudness.
    let outdoor: Vec<(usize, BedSound)> = ambience.beds.iter().enumerate().filter(|(_, bed)| bed.outdoor()).map(|(i, bed)| (i, bed.sound.clone())).collect();
    let cutoff = ambience.shelter.muffle;
    let assets = voxmar_server::assets_dir();
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
                let variants = (0..synth::ONE_SHOT_VARIANTS).map(|v| OneShotClip::new(synth::one_shot(s, v))).collect();
                (s, Arc::new(variants))
            })
            .collect();
        let muffled = outdoor
            .par_iter()
            .filter(|_| cutoff < defs::OPEN_AIR_HZ)
            .filter_map(|(bed, sound)| {
                let segments = (0..synth::BED_VARIANTS).map(|v| {
                    let segment = match sound {
                        BedSound::Synth(synth) => Ok(synth::bed(*synth, v)),
                        BedSound::Recorded(paths) => decode(&assets.join(&paths[v])),
                    };
                    segment.map(|segment| {
                        let [l, r] = mixer::muffled(segment, cutoff);
                        sfx::wav_stereo_at(sfx::RATE / mixer::MUFFLED_STEP as u32, &l, &r)
                    })
                });
                match segments.collect::<Result<Vec<_>, _>>() {
                    Ok(segments) => Some((*bed, segments)),
                    Err(e) => {
                        warn!("ambience: no muffled bed: {e}");
                        None
                    }
                }
            })
            .collect();
        let _ = tx.send(Synthesized { beds, muffled, sounds });
    });
    commands.insert_resource(Ambient::new(ambience, roof_patters, kinds, launch.content.clone(), clips_rx));
}

impl Ambient {
    /// The ambience before anything is heard, its clips to come from `clips_rx`.
    fn new(ambience: Ambience, roof_patters: Vec<Option<u8>>, kinds: Arc<Kinds>, content: Content, clips_rx: Receiver<Synthesized>) -> Self {
        let axes = ambience.axes();
        Self {
            mixer: Mixer::new(&ambience, 0x616d_6269),
            roof_patters,
            roof: RoofLook::default(),
            ambience: Arc::new(ambience),
            kinds,
            content,
            clips: None,
            loading: None,
            clips_rx,
            surveys: Surveys::new(),
            since_survey: SURVEY_INTERVAL,
            generator: None,
            enclosure: Enclosure::default(),
            place: None,
            place_survey: 0,
            smooth: Enclosure::default(),
            env: Environment::new(axes),
            cues: Vec::new(),
            placed: unbounded(),
            logged: String::new(),
        }
    }

    /// Hands the clips made to the mixer once every recorded bed has loaded (`loaded`), and
    /// not before (see [`beds_ready`]).
    fn take_clips(&mut self, loaded: impl Fn(&Handle<AudioSource>) -> bool) {
        if self.loading.as_ref().is_some_and(|clips| beds_ready(&clips.beds, loaded)) {
            self.clips = self.loading.take();
        }
    }
}

/// What the enclosure scan needs of each block.
fn kinds(ambience: &Ambience, registry: &BlockRegistry, multiblocks: &voxmar_engine::multiblock::MultiblockRegistry, walls: &[Option<voxmar_engine::effect::Shelter>], roof_scan: i32) -> Kinds {
    let ids = (0..registry.len()).map(|id| registry.info(voxmar_engine::block::BlockId(id as u16)));
    let group = ambience.block_groups(registry);
    // A structure's core in a group heard only while lit, and any block in a group heard only
    // while it smoulders: both only while the server says it burns (`burning` in the scan).
    let lit_only = group
        .iter()
        .enumerate()
        .map(|(id, g)| {
            g.is_some_and(|g| {
                let g = &ambience.groups[g as usize];
                g.smouldering || (g.lit && multiblocks.with_core(voxmar_engine::block::BlockId(id as u16)).next().is_some())
            })
        })
        .collect();
    Kinds {
        open: ids.clone().map(|info| !info.solid || info.render == RenderKind::Leaves).collect(),
        leaves: ids.map(|info| info.render == RenderKind::Leaves).collect(),
        wall_group: ambience.wall_groups(walls),
        group,
        lit_only,
        groups: ambience.groups.len(),
        walls: ambience.shelter.walls,
        roof: ambience.shelter.roof,
        roof_scan,
    }
}

fn receive_clips(ambient: Option<ResMut<Ambient>>, mut sources: ResMut<Assets<AudioSource>>, assets: Res<AssetServer>) {
    let Some(mut ambient) = ambient else { return };
    if ambient.clips.is_some() {
        return;
    }
    if ambient.loading.is_none() {
        let Ok(synthesized) = ambient.clips_rx.try_recv() else { return };
        ambient.loading = Some(made_clips(&ambient, synthesized, &mut sources, &assets));
    }
    ambient.take_clips(|clip| sources.contains(clip) || assets.load_state(clip).is_failed());
}

/// Whether every bed's clips are ready to play (`loaded`: there, or failed to load). A
/// recorded bed loads from its package as an asset, but its muffled twin is made from the
/// recording on a worker and is there at once: the beds wait for the recordings, so the
/// two start together and play in step.
fn beds_ready<T>(beds: &[Vec<T>], loaded: impl Fn(&T) -> bool) -> bool {
    beds.iter().flatten().all(loaded)
}

/// The clips made on the worker, as assets: the recorded beds asked of the asset server.
fn made_clips(ambient: &Ambient, synthesized: Synthesized, sources: &mut Assets<AudioSource>, assets: &AssetServer) -> Clips {
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
    let mut muffled = vec![Vec::new(); ambient.ambience.beds.len()];
    for (bed, files) in synthesized.muffled {
        muffled[bed] = files.into_iter().map(|bytes| sources.add(AudioSource { bytes: Arc::from(bytes) })).collect();
    }
    Clips { beds, muffled, sounds }
}

/// Scans the surroundings on a worker about once a second, and eases the environment
/// towards what the last scan found.
fn survey(
    time: Res<Time>,
    world: Res<ClientWorld>,
    player: Res<LocalPlayer>,
    mut ambient: ResMut<Ambient>,
    mut surroundings: ResMut<Surroundings>,
) {
    let ambient = &mut *ambient;
    for survey in ambient.surveys.answers() {
        ambient.enclosure = survey.enclosure;
        if survey.place.is_some() {
            ambient.place = survey.place;
            ambient.place_survey = survey.number;
        }
    }
    let dt = time.delta_secs();
    let ease = (dt / SETTLE).min(1.0);
    let (smooth, latest) = (&mut ambient.smooth, &ambient.enclosure);
    if smooth.groups.len() != latest.groups.len() {
        *smooth = latest.clone();
    } else {
        let eased = [
            (&mut smooth.air, latest.air),
            (&mut smooth.sky, latest.sky),
            (&mut smooth.canopy, latest.canopy),
            (&mut smooth.shelter, latest.shelter),
            (&mut smooth.roof, latest.roof),
        ];
        for (value, target) in eased {
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
    // None is taken of where a dead player lay: the next is of where they come back.
    if !player.joined || player.dead || ambient.surveys.out || ambient.since_survey < SURVEY_INTERVAL {
        return;
    }
    surroundings.started += 1;
    let number = surroundings.started;
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
    let (kinds, content, tx, ambience) = (ambient.kinds.clone(), ambient.content.clone(), ambient.surveys.tx.clone(), ambient.ambience.clone());
    let fires = world.fires.clone();
    let smoulders = world.smoulders.clone();
    let random = mix(time.elapsed().as_nanos() as u64);
    ambient.surveys.out = true;
    rayon::spawn(move || {
        let enclosure = enclosure::scan(eye, |p| snapshot.block(p), |p| fires.contains(&p) || smoulders.contains(&p), &kinds, random);
        let generator = generator.get_or_init(|| match WorldContent::load(&content) {
            Ok(world) => Some(Overworld::shared(seed, &content, &world)),
            Err(e) => {
                warn!("ambience has no climate: {e}");
                None
            }
        });
        let place = generator.as_ref().map(|generator| place(generator, eye, &ambience.buildings));
        let _ = tx.send(Survey { number, enclosure, place });
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
fn place(generator: &Overworld, eye: IVec3, sets: &[defs::BuildingSet]) -> Place {
    let site = generator.site(eye.x, eye.z);
    let biomes = generator.biomes();
    let mut land = 0.0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            land += generator.site(eye.x + dx * 24, eye.z + dz * 24).height as f32 / 9.0;
        }
    }
    let edge = (site.edge / 48.0).clamp(0.0, 1.0);
    let at = eye.xz().as_vec2() + 0.5;
    let mut near = generator.settlements_near(eye.xz(), SETTLEMENTS_HEARD);
    // The search gives every settlement planned in the regions it touches, some far past
    // hearing.
    near.retain(|s| s.centre.xz().as_vec2().distance(at) <= SETTLEMENTS_HEARD as f32);
    let nearest = nearest_settlement(&near, at, sets);
    let (settlement, outside, people, id, buildings) = match nearest {
        Some((s, settlement, (outside, buildings))) => (settlement, outside, s.people.len() as f32, Some(s.id), buildings),
        None => (NO_SETTLEMENT, NO_SETTLEMENT, 0.0, None, vec![Vec::new(); sets.len()]),
    };
    Place {
        settlement,
        edge: outside,
        people,
        id,
        buildings,
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

/// The settlement of `near` whose bounds are nearest `at` (the one the player stands in, of
/// two close together), the nearer middle of two equally near: it, the way to its middle,
/// and [`settlement_bounds`].
#[allow(clippy::type_complexity)]
fn nearest_settlement<'a>(near: &'a [voxmar_worldgen::SettlementInfo], at: Vec2, sets: &[defs::BuildingSet]) -> Option<(&'a voxmar_worldgen::SettlementInfo, f32, (f32, Vec<Vec<Vec3>>))> {
    near.iter().map(|s| (s, s.centre.xz().as_vec2().distance(at), settlement_bounds(s, at, sets))).min_by(|a, b| a.2.0.total_cmp(&b.2.0).then(a.1.total_cmp(&b.1)))
}

/// How far `at` is outside settlement `s`'s bounds (the ground its buildings and square
/// stand on, and [`SETTLEMENT_MARGIN`] round them), 0 within; and where each of `sets` of
/// its buildings is heard from (the door, or else the middle), within [`BUILDINGS_HEARD`].
fn settlement_bounds(s: &voxmar_worldgen::SettlementInfo, at: Vec2, sets: &[defs::BuildingSet]) -> (f32, Vec<Vec<Vec3>>) {
    let (low, high) = s.buildings.iter().fold((s.centre.xz(), s.centre.xz()), |(low, high), b| (low.min(b.min), high.max(b.max)));
    let (low, high) = ((low - SETTLEMENT_MARGIN).as_vec2(), (high + SETTLEMENT_MARGIN + 1).as_vec2());
    let edge = (low - at).max(at - high).max(Vec2::ZERO).length();
    let buildings = sets
        .iter()
        .map(|set| {
            s.buildings
                .iter()
                .filter(|b| set.names.iter().any(|name| b.answers(name)))
                .map(|b| match b.door.filter(|_| set.door) {
                    Some(door) => door.as_vec3() + 0.5,
                    None => Vec3::new((b.min.x + b.max.x + 1) as f32 / 2.0, b.floor as f32 + 2.0, (b.min.y + b.max.y + 1) as f32 / 2.0),
                })
                .filter(|p| p.xz().distance(at) < BUILDINGS_HEARD)
                .collect()
        })
        .collect();
    (edge, buildings)
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
    env.set(Fixed::Shelter, smooth.shelter);
    env.set(Fixed::Roof, smooth.roof);
    surroundings.thunder = 1.0 - (1.0 - ambient.ambience.shelter.thunder) * smooth.shelter;
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
    env.set(Fixed::SettlementEdge, ambient.place.as_ref().map_or(NO_SETTLEMENT, |place| place.edge));
    env.set(Fixed::SettlementPeople, ambient.place.as_ref().map_or(0.0, |place| place.people));
    surroundings.settlement = ambient.place.as_ref().and_then(|place| Some((place.id?, place.edge)));
    surroundings.surveyed = ambient.place_survey;
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
    mut segments: Query<(Entity, &BedSegment, Has<Muffled>, &mut AudioSink)>,
    mut sources: ResMut<Assets<AudioSource>>,
    mut ambient: ResMut<Ambient>,
) {
    let ambient = &mut *ambient;
    let (Some(clips), Ok(camera), true) = (&ambient.clips, camera.single(), player.joined) else { return };
    // Bevy applies the global volume only as a sound starts, and beds play on (see main.rs).
    let master = AMBIENCE_GAIN * sfx::MASTER * settings.sound_volume as f32 / 100.0 * settings.ambience_volume as f32 / 100.0
        * global.volume.to_linear();
    let ears = camera.translation();
    for Placed { wav, playback, at } in ambient.placed.1.try_iter() {
        let source = sources.add(AudioSource { bytes: wav });
        // One-shots are voices of the ambience, capped with the rest of it (`mix`);
        // beds are the mixer's own, crossfading, never cut off.
        let voice = at.map_or(Voice::of(Category::Ambience), |at| Voice::at(Category::Ambience, at));
        commands.spawn((AudioPlayer(source), playback, voice));
    }
    let mut cues = std::mem::take(&mut ambient.cues);
    let length = |sound: usize, variant: usize| clips.sounds[sound].get(variant).map_or(1.0, |c| c.dry.len() as f32 / sfx::RATE as f32);
    let variants = (synth::BED_VARIANTS, synth::ONE_SHOT_VARIANTS);
    let buildings = ambient.place.as_ref().map_or(&[][..], |place| &place.buildings[..]);
    ambient.mixer.update(&ambient.ambience, &ambient.env, &ambient.enclosure.samples, &ambient.enclosure.open_to_sky, buildings, ears, time.delta_secs(), length, variants, &mut cues);
    let audible = master > 0.0;
    for cue in cues.drain(..) {
        if let Cue::Sound { sound, .. } = cue {
            debug!("ambience: {}", short(&ambient.ambience.sounds[sound].name));
        }
        match cue {
            Cue::Segment { .. } | Cue::Sound { .. } if !audible => {}
            Cue::Segment { bed, variant } => {
                let Some(clip) = clips.beds[bed].get(variant) else { continue };
                let [dry, wet] = heard(ambient, clips, bed, master);
                commands.spawn((BedSegment(bed), AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(dry))));
                if let Some(clip) = clips.muffled[bed].get(variant) {
                    commands.spawn((BedSegment(bed), Muffled, AudioPlayer(clip.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(wet))));
                }
            }
            Cue::Stop { bed } => {
                for (entity, segment, _, _) in &segments {
                    if segment.0 == bed {
                        commands.entity(entity).despawn();
                    }
                }
            }
            Cue::Sound { sound, variant, at, volume, speed, muffle } => {
                if clips.sounds[sound].get(variant).is_none() {
                    continue;
                }
                // Placing a clip in stereo and encoding it takes about a millisecond for the
                // long ones: done on a worker, never on the frame, and played when it is back.
                let (variants, tx) = (clips.sounds[sound].clone(), ambient.placed.0.clone());
                let (reverb, cutoff) = (ambient.env.get(Fixed::Reverb), ambient.ambience.shelter.muffle);
                let (right, forward) = (camera.right().into(), camera.forward().into());
                let playback = PlaybackSettings { speed, ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume * master)) };
                rayon::spawn(move || {
                    let clip = &variants[variant];
                    let mut placed = mixer::place(&clip.dry, &clip.wet, at, ears, right, forward, reverb);
                    mixer::muffle(&mut placed, muffle, cutoff);
                    let [left, right] = placed;
                    let _ = tx.send(Placed { wav: Arc::from(sfx::wav_stereo(&left, &right)), playback, at });
                });
            }
        }
    }
    ambient.cues = cues;
    for (_, segment, muffled, mut sink) in &mut segments {
        sink.set_volume(Volume::Linear(heard(ambient, clips, segment.0, master)[muffled as usize]));
    }
    // Say which beds are playing whenever that changes, for tuning.
    let beds: Vec<&str> = (ambient.ambience.beds.iter().enumerate())
        .filter(|&(i, bed)| ambient.mixer.level(i) > 0.25 * bed.volume)
        .map(|(_, bed)| short(&bed.name))
        .collect();
    let beds = beds.join(", ");
    if beds != ambient.logged {
        let env = &ambient.env;
        let [air, sky, reverb, relief, depth, shelter] =
            [Fixed::Air, Fixed::Sky, Fixed::Reverb, Fixed::Relief, Fixed::Depth, Fixed::Shelter].map(|a| env.get(a));
        info!("ambience: [{beds}] air {air:.2} sky {sky:.2} reverb {reverb:.2} relief {relief:.0} depth {depth:.0} shelter {shelter:.2}");
        ambient.logged = beds;
    }
}

/// How loud a bed plays as it is and muffled: split by how indoors the place is, if it has
/// a muffled twin.
fn heard(ambient: &Ambient, clips: &Clips, bed: usize, master: f32) -> [f32; 2] {
    let level = ambient.mixer.level(bed) * master;
    if clips.muffled[bed].is_empty() {
        return [level, 0.0];
    }
    let muffled = ambient.mixer.muffling(&ambient.env, &ambient.ambience.beds[bed].duck);
    [level * (1.0 - muffled), level * muffled]
}

/// What the settlement around is heard as, for the gate: the `settlement` axis now (blocks
/// to the nearest one's middle), the `settlement_edge` and `settlement_people` axes (how far
/// outside its bounds, and how many live there), and each one-shot that hangs on it with how fully it would
/// play here at noon, at midnight, and here with no settlement near, all at the surface
/// (the player may have dropped into the well on the square). Nothing before the ambience
/// has started.
pub fn settlement_heard(world: &World) -> Option<(f32, [f32; 2], Vec<(String, [f32; 3])>)> {
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
    Some((env.get(Fixed::Settlement), [env.get(Fixed::SettlementEdge), env.get(Fixed::SettlementPeople)], sounds))
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
    // The next world starts from nothing found, not deep under the last one; the count of
    // surveys runs on, so one still out from this world is never taken for the next's (by
    // the music, or here: `Surveys::leave`).
    *surroundings = Surroundings { started: surroundings.started, ..default() };
    let Some(mut ambient) = ambient else { return };
    // One-shots placed for the world left are not played in the next: a new channel, so a
    // worker still placing one sends it where nobody reads.
    ambient.placed = unbounded();
    let axes = ambient.ambience.axes();
    ambient.mixer = Mixer::new(&ambient.ambience, 0x616d_6269);
    ambient.roof = RoofLook::default();
    ambient.enclosure = Enclosure::default();
    ambient.smooth = Enclosure::default();
    ambient.place = None;
    ambient.place_survey = 0;
    ambient.surveys.leave(surroundings.started);
    ambient.env = Environment::new(axes);
    ambient.since_survey = SURVEY_INTERVAL;
}
