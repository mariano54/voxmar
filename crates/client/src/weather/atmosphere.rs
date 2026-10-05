//! The air around the player: the haze, sky tint, colour grading and ambient particles of
//! the land (`Overworld::atmosphere_at`, which follows the climate and lets biomes set
//! their own), eased as the player walks so borders pass gradually, then combined with
//! the weather, the wind and the time of day. Fog, dust and overcast skies thicken the
//! haze and drain colour; rain grounds pollen and fireflies; wind stills hovering spores,
//! and dust is only blown about by a real wind; pollen comes out only on some days, mostly
//! in sunshine and where flowers are near (as the ambience's scan finds them), and
//! fireflies only on some nights.
//!
//! `ClientAtmosphere` is what the renderer reads: the colour parts are applied within the
//! active shader pack (see docs/rendering.md), and the particle sets are drawn by the
//! precipitation particles.

use std::sync::Arc;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, bounded};
use voxmar_engine::weather::pcg;
use voxmar_protocol::TICKS_PER_DAY;
use voxmar_worldgen::{DayTime, Motion, Overworld, ParticleDef, WeatherWhen, WorldContent};

use super::ClientWeather;
use crate::ambience::Surroundings;
use crate::menu::Screen;
use crate::player::LocalPlayer;
use crate::render::DayCycle;
use crate::session::Launch;

pub struct AtmospherePlugin;

impl Plugin for AtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientAtmosphere>()
            .add_systems(Update, (prepare, sample, combine).chain())
            .add_systems(OnExit(Screen::InGame), |mut air: ResMut<ClientAtmosphere>| *air = ClientAtmosphere::default());
    }
}

/// Seconds between samples of the land's air, and to ease most of the way to each.
const SAMPLE_EVERY: f32 = 0.5;
const EASE: f32 = 4.0;
/// Share of nights each night-time particle set (fireflies) comes out on, and of days
/// each daytime one (pollen, dust) does.
const NIGHTS_OUT: f32 = 0.6;
const DAYS_OUT: f32 = 0.35;

/// The air around the player, for the renderer.
#[derive(Resource)]
pub struct ClientAtmosphere {
    /// Haze colour, linear RGB.
    pub haze: Vec3,
    /// Haze density relative to the shader pack's usual, around 1: more in humid air,
    /// fog, dust and rain.
    pub haze_density: f32,
    /// Tint multiplied into the sky, linear RGB; white leaves it.
    pub sky: Vec3,
    /// Colour grading nudges: −1 cooler ..= 1 warmer, and saturation around 1.
    pub warmth: f32,
    pub saturation: f32,
    /// Ambient particle sets showing now, densest first, with how many (0..1 of the most
    /// the particle renderer shows).
    pub particles: Vec<AmbientParticles>,
    /// The land's air as last sampled, and as eased towards.
    land: Air,
    target: Air,
    /// The world's generator, once made, for the world seeded with `seed`.
    generator: Option<Arc<Overworld>>,
    seed: Option<u64>,
    building: Option<Receiver<Arc<Overworld>>>,
    since_sample: f32,
    /// A sample being taken off the main thread.
    sampling: Option<Receiver<Air>>,
}

/// One set of ambient particles: how many, and how they look and move.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientParticles {
    pub name: String,
    pub density: f32,
    /// Linear RGB.
    pub color: Vec3,
    /// Size in blocks.
    pub size: f32,
    pub motion: Motion,
    /// Gives off its own light.
    pub glow: bool,
}

/// The land's air, eased: the colour parts and each particle set's density by name.
#[derive(Clone, Debug, PartialEq)]
struct Air {
    haze: Vec3,
    haze_density: f32,
    sky: Vec3,
    warmth: f32,
    saturation: f32,
    particles: Vec<(ParticleDef, f32)>,
}

impl Default for Air {
    fn default() -> Self {
        Self { haze: Vec3::new(0.76, 0.83, 0.92), haze_density: 1.0, sky: Vec3::ONE, warmth: 0.0, saturation: 1.0, particles: Vec::new() }
    }
}

impl Default for ClientAtmosphere {
    fn default() -> Self {
        let air = Air::default();
        Self {
            haze: srgb(air.haze),
            haze_density: 1.0,
            sky: Vec3::ONE,
            warmth: 0.0,
            saturation: 1.0,
            particles: Vec::new(),
            land: air.clone(),
            target: air,
            generator: None,
            seed: None,
            building: None,
            since_sample: SAMPLE_EVERY,
            sampling: None,
        }
    }
}

impl ClientAtmosphere {
    /// Samples the air of the world with this seed from now on.
    pub fn start(&mut self, seed: u64) {
        if self.seed != Some(seed) {
            self.seed = Some(seed);
            self.generator = None;
            self.building = None;
        }
    }
}

/// Builds the world's generator off the main thread once the server has said which world
/// it is.
fn prepare(mut air: ResMut<ClientAtmosphere>, launch: Res<Launch>) {
    if let Some(building) = &air.building {
        if let Ok(generator) = building.try_recv() {
            air.generator = Some(generator);
            air.building = None;
        }
        return;
    }
    let (Some(seed), None) = (air.seed, &air.generator) else { return };
    let content = launch.content.clone();
    let (sender, receiver) = bounded(1);
    rayon::spawn(move || match WorldContent::load(&content) {
        Ok(world) => {
            let _ = sender.send(Arc::new(Overworld::with_content(seed, content.blocks.clone(), &world)));
        }
        Err(e) => tracing::error!("cannot sample the air: {e}"),
    });
    air.building = Some(receiver);
}

/// Samples the land's air where the player is now and then, and eases towards it. The
/// sample is taken off the main thread: it looks up the land's climate, which took 20-36 ms
/// at times while the world streamed in around a flight, a frame each time.
fn sample(time: Res<Time>, mut air: ResMut<ClientAtmosphere>, player: Res<LocalPlayer>) {
    let air = &mut *air;
    air.since_sample += time.delta_secs();
    if let Some(sampling) = &air.sampling
        && let Ok(target) = sampling.try_recv()
    {
        air.target = target;
        air.sampling = None;
    }
    if let Some(generator) = &air.generator
        && player.joined
        && air.sampling.is_none()
        && air.since_sample >= SAMPLE_EVERY
    {
        air.since_sample = 0.0;
        let at = player.position.floor().as_ivec3();
        let generator = generator.clone();
        let (sender, receiver) = bounded(1);
        rayon::spawn(move || {
            let sampled = generator.atmosphere_at(at.x, at.z);
            let sets = generator.particle_sets();
            let _ = sender.send(Air {
                haze: Vec3::from(sampled.haze),
                haze_density: sampled.haze_density,
                sky: Vec3::from(sampled.sky),
                warmth: sampled.warmth,
                saturation: sampled.saturation,
                particles: sampled.particles.iter().map(|&(id, density)| (sets[id.0 as usize].clone(), density)).collect(),
            });
        });
        air.sampling = Some(receiver);
    }
    let t = (time.delta_secs() / EASE).min(1.0);
    let (land, target) = (&mut air.land, &air.target);
    land.haze = land.haze.lerp(target.haze, t);
    land.haze_density += (target.haze_density - land.haze_density) * t;
    land.sky = land.sky.lerp(target.sky, t);
    land.warmth += (target.warmth - land.warmth) * t;
    land.saturation += (target.saturation - land.saturation) * t;
    // Sets ease in and out by density; ones gone from the target fade away.
    for (set, _) in &target.particles {
        if !land.particles.iter().any(|(s, _)| s.name == set.name) {
            land.particles.push((set.clone(), 0.0));
        }
    }
    for (set, density) in &mut land.particles {
        let goal = target.particles.iter().find(|(s, _)| s.name == set.name).map_or(0.0, |&(_, d)| d);
        *density += (goal - *density) * t;
    }
    land.particles.retain(|&(_, density)| density > 0.001);
}

/// The land's air with the weather, wind and time of day applied.
fn combine(
    mut air: ResMut<ClientAtmosphere>,
    weather: Res<ClientWeather>,
    cycle: Res<DayCycle>,
    surroundings: Option<Res<Surroundings>>,
) {
    let shown = &weather.current;
    let day = (cycle.ticks / TICKS_PER_DAY as f64).rem_euclid(1.0) as f32;
    // Days are numbered from midnight to midnight and nights from noon to noon (0 is
    // sunrise), so neither changes while it is out.
    let days = cycle.ticks / TICKS_PER_DAY as f64;
    let (day_index, night_index) = ((days + 0.25).floor() as i64, (days - 0.25).floor() as i64);
    let flowers = surroundings.map_or(0.0, |s| s.flowers);
    // Sunshine: pollen keeps to bright days.
    let sunny = 1.0 - 0.8 * smoothstep(0.5, 0.95, shown.cover);
    let land = air.land.clone();

    // Fog, dust and humid air thicken the haze and tint it; overcast and rain drain colour.
    let falling = smoothstep(0.02, 0.2, shown.precipitation);
    let dust = Vec3::from(shown.dust_color.map(|c| c as f32 / 255.0));
    let mut haze = land.haze.lerp(dust, shown.dust * 0.8);
    haze = haze.lerp(Vec3::splat(0.82), (shown.fog + 0.5 * falling).min(1.0) * 0.6);
    air.haze = srgb(haze);
    air.haze_density = land.haze_density
        * (1.0 + 2.0 * shown.fog + 1.0 * shown.ground_fog + 1.5 * shown.dust + 0.5 * falling + 0.4 * smoothstep(0.7, 1.0, shown.humidity));
    air.sky = srgb(land.sky);
    air.warmth = (land.warmth + 0.1 * shown.dust - 0.1 * shown.snow * falling).clamp(-1.0, 1.0);
    air.saturation = land.saturation * (1.0 - 0.2 * shown.cover * (0.5 + 0.5 * falling)) * (1.0 - 0.3 * shown.fog);

    let wind = shown.wind.length();
    let seed = air.seed.unwrap_or(0);
    let mut particles: Vec<AmbientParticles> = land
        .particles
        .iter()
        .filter_map(|(set, density)| {
            let when = time_of_day(set.time, day);
            let weather = match set.weather {
                WeatherWhen::Always => 1.0,
                WeatherWhen::Dry => 1.0 - falling,
                WeatherWhen::Wet => falling,
            };
            // Hovering things settle out of a strong wind; drifting ones thin in it; dust
            // lies still until the wind picks it up.
            let calm = match set.motion {
                Motion::Hover => 1.0 - smoothstep(4.0, 9.0, wind),
                Motion::Drift => 1.0 - 0.6 * smoothstep(6.0, 14.0, wind),
                Motion::Blow => smoothstep(3.5, 8.0, wind),
                _ => 1.0,
            };
            // Night creatures come out only on some nights, pollen on some days and then
            // mostly in sunshine; dust follows the wind instead.
            let out = match set.time {
                DayTime::Night => out_on(seed, night_index, &set.name, NIGHTS_OUT),
                DayTime::Day if set.motion != Motion::Blow => out_on(seed, day_index, &set.name, DAYS_OUT) * sunny,
                _ => 1.0,
            };
            // Sets tagged `flowers` (pollen) keep mostly to where flowers are near.
            let near = if set.tags.iter().any(|t| t == "flowers") { 0.15 + 0.85 * flowers } else { 1.0 };
            let density = density * when * weather * calm * out * near;
            (density > 0.005).then(|| AmbientParticles {
                name: set.name.clone(),
                density,
                color: srgb(Vec3::from([set.color.0, set.color.1, set.color.2].map(|c| c as f32 / 255.0))),
                size: set.size,
                motion: set.motion,
                glow: set.glow,
            })
        })
        .collect();
    particles.sort_by(|a, b| b.density.total_cmp(&a.density));
    air.particles = particles;
}

/// How much a set showing at `time` shows at a fraction of the day from sunrise.
fn time_of_day(time: DayTime, day: f32) -> f32 {
    // As the sky: the sun rises at 0 and sets at 0.5.
    let sun = (day * std::f32::consts::TAU).sin();
    let daylight = smoothstep(-0.2, 0.15, sun);
    match time {
        DayTime::Always => 1.0,
        DayTime::Day => daylight,
        DayTime::Night => 1.0 - smoothstep(-0.3, -0.05, sun),
        DayTime::Twilight => 1.0 - smoothstep(0.0, 0.25, sun.abs()),
    }
}

/// Whether a set comes out on the day or night numbered `period`: on `share` of them, the
/// same for every player of the world.
fn out_on(seed: u64, period: i64, name: &str, share: f32) -> f32 {
    let name = name.bytes().fold(0u32, |h, b| pcg(h ^ b as u32));
    let h = pcg(pcg((seed ^ (seed >> 32)) as u32 ^ name) ^ period as u32);
    if ((h >> 8) as f32 / 16_777_216.0) < share { 1.0 } else { 0.0 }
}

/// Linear RGB from sRGB components.
fn srgb(color: Vec3) -> Vec3 {
    let linear = Color::srgb(color.x, color.y, color.z).to_linear();
    Vec3::new(linear.red, linear.green, linear.blue)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fireflies_keep_to_some_nights() {
        let nights: Vec<f32> = (0..400).map(|n| out_on(9, n, "base:fireflies", NIGHTS_OUT)).collect();
        let out = nights.iter().sum::<f32>() / nights.len() as f32;
        assert!((0.5..0.7).contains(&out), "{out}");
        assert_eq!(out_on(9, 12, "base:fireflies", NIGHTS_OUT), out_on(9, 12, "base:fireflies", NIGHTS_OUT));
        let days = (0..400).map(|n| out_on(9, n, "base:pollen", DAYS_OUT)).sum::<f32>() / 400.0;
        assert!((0.27..0.43).contains(&days), "{days}");
        assert!(time_of_day(DayTime::Night, 0.75) == 1.0 && time_of_day(DayTime::Night, 0.25) == 0.0);
        assert!(time_of_day(DayTime::Day, 0.25) == 1.0 && time_of_day(DayTime::Twilight, 0.0) > 0.9);
    }
}
