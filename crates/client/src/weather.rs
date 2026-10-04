//! The weather as the client sees it: the server's latest report for where the player is,
//! eased into over seconds so clouds form, rain sets in and fog gathers gradually; the
//! wind, gusting by the same function the server uses; how far the wind has carried the
//! clouds; and lightning: a flash and a bolt at once, thunder when the sound arrives.
//!
//! Everything the renderer draws of the weather is read from here: `current` holds every
//! channel (fog, hail, dust, rainbow, aurora, …; see `voxmar_engine::weather::Weather`),
//! `flash` how brightly lightning lights the world, and `bolts` the shape of each
//! lightning bolt. docs/weather-effects.md says how each is meant to look.

use bevy::prelude::*;
use voxmar_engine::weather::{GUST_WRAP, WeatherTable, advance_gusts, gust, pcg};
use voxmar_protocol::{TICKS_PER_SECOND, Weather};

use crate::menu::Screen;

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientWeather>()
            .init_resource::<Wind>()
            .add_message::<Thunder>()
            .add_systems(Update, (ease, draw_bolts).chain())
            .add_systems(OnExit(Screen::InGame), |mut weather: ResMut<ClientWeather>| weather.leave());
    }
}

#[derive(Resource)]
pub struct ClientWeather {
    /// What the server last reported, or a fixed state for captures.
    target: Option<Weather>,
    /// Whether the server has reported the weather yet in this game.
    reported: bool,
    forced: bool,
    /// Until when (seconds since startup) the shown weather changes over quickly, after a
    /// command changed it.
    quick_until: f32,
    /// What is shown now. Its wind includes the gusts at the camera.
    pub current: Weather,
    /// The mean wind, eased, before gusts.
    mean_wind: Vec2,
    /// How far gust fronts have travelled, advanced locally, and how far it still has to
    /// move to agree with the server's last report.
    gust_offset: Vec2,
    gust_error: Vec2,
    /// Effects showing, by name, strongest first, as the server last said.
    pub effects: Vec<String>,
    /// How far the wind has carried the clouds, in blocks, wrapped every `DRIFT_WRAP`.
    pub drift: Vec2,
    /// What the gusts are seeded with, and the server's simulation tick when last told,
    /// with when that was (seconds since startup).
    seed: u64,
    tick: u64,
    told_at: f64,
    strikes: Vec<Strike>,
}

/// The wind, for anything that sways or drifts with it: grass and leaves, particles,
/// sounds. The mean wind and how much it gusts come from the server's weather (eased like
/// the rest of it), and the gusts at any place and moment from the deterministic function
/// the server uses, so client and server agree on it. See docs/rendering.md.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct Wind {
    /// Unit vector (x, z) the mean wind blows towards.
    pub direction: Vec2,
    /// Mean speed, blocks per second: under 1 calm, about 4 a breeze, 10 strong, 18 a gale.
    pub speed: f32,
    /// How much it gusts around the mean, 0..1.
    pub gustiness: f32,
    /// Seed of the gusts, as the server's.
    pub seed: u64,
    /// Seconds of world time now: the server's simulation clock, which the gusts run on.
    pub seconds: f64,
    /// How far gust fronts have travelled with the wind (x, z), wrapped every
    /// `voxmar_engine::weather::GUST_WRAP` blocks; the server's, advanced between reports.
    pub offset: Vec2,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            direction: FAIR.wind.normalize(),
            speed: FAIR.wind.length(),
            gustiness: FAIR.gusts,
            seed: 0,
            seconds: 0.0,
            offset: Vec2::ZERO,
        }
    }
}

impl Wind {
    /// The mean wind, in blocks per second along x and z.
    pub fn mean(&self) -> Vec2 {
        self.direction * self.speed
    }

    /// The wind at `at` (x, z) and `seconds` of world time, gusts included.
    pub fn at(&self, at: Vec2, seconds: f64) -> Vec2 {
        gust(self.seed, at, seconds, self.offset, self.mean(), self.gustiness)
    }

    /// The wind at `at` now.
    pub fn now(&self, at: Vec2) -> Vec2 {
        self.at(at, self.seconds)
    }
}

/// Cloud drift wraps around after this many blocks, keeping shader coordinates small.
pub const DRIFT_WRAP: f32 = 1_048_576.0;
/// Blocks sound travels per second.
const SOUND_SPEED: f32 = 343.0;
/// Seconds a quick change takes to show, roughly.
const QUICK_EASE: f32 = 0.7;
const QUICK_FOR: f32 = 4.0;
/// Seconds each fraction of `Weather::fractions` takes to ease most of the way towards
/// the report: cloud builds over tens of seconds, rain and hail set in over a few, fog
/// gathers and lifts slowly, lightning flickers.
const EASE: [f32; 24] = [
    25.0, // cover
    25.0, // convection
    30.0, // cirrus
    20.0, // humidity
    5.0,  // gusts
    6.0,  // precipitation
    10.0, // snow
    6.0,  // drizzle
    4.0,  // hail
    6.0,  // sleet
    15.0, // fog
    20.0, // ground fog
    8.0,  // dust
    10.0, // wetness
    20.0, // snow cover
    30.0, // frost
    3.0,  // lightning
    8.0,  // rainbow
    15.0, // halo
    20.0, // aurora
    10.0, // heat haze
    5.0,  // wisps
    8.0,  // motes
    10.0, // falling stars
];
/// The menus' sky, and what the game shows until the server reports: a fair day.
const FAIR: Weather = Weather {
    cover: 0.35,
    convection: 1.0,
    cirrus: 0.1,
    humidity: 0.5,
    wind: Vec2::new(3.0, 1.5),
    gusts: 0.2,
    ..Weather::NONE
};
/// Seconds a bolt stays visible.
const BOLT_LIFE: f32 = 0.45;

/// A lightning strike: seen at once, heard later.
struct Strike {
    /// Seconds since the flash.
    age: f32,
    /// Blocks away.
    distance: f32,
    heard: bool,
    /// Where it struck the ground, and the seed of its shape.
    at: Vec3,
    seed: u32,
}

/// Thunder reaching the player from lightning `distance` blocks away.
#[derive(Message, Clone, Copy, Debug)]
pub struct Thunder {
    pub distance: f32,
}

/// A lightning bolt for the renderer: a jagged channel from the cloud base to the ground,
/// with branches that fork off it and fade before reaching the ground.
#[derive(Clone, Debug, PartialEq)]
pub struct Bolt {
    /// Points from the cloud down to where it strikes, in world space.
    pub channel: Vec<Vec3>,
    /// Shorter forks, each starting on the channel.
    pub branches: Vec<Vec<Vec3>>,
    /// How bright it is now, 0..1: it flickers with the flash and is gone within half a
    /// second.
    pub brightness: f32,
}

/// Until a server reports the weather, and in the menus: a fair day.
impl Default for ClientWeather {
    fn default() -> Self {
        Self {
            target: Some(FAIR),
            reported: false,
            forced: false,
            quick_until: 0.0,
            current: FAIR,
            mean_wind: FAIR.wind,
            gust_offset: Vec2::ZERO,
            gust_error: Vec2::ZERO,
            effects: Vec::new(),
            drift: Vec2::ZERO,
            seed: 0,
            tick: 0,
            told_at: 0.0,
            strikes: Vec::new(),
        }
    }
}

impl ClientWeather {
    /// The server's report for where the player is, and the effects showing there.
    /// `quick` (after a command changed the weather) changes over within a second or two
    /// rather than easing in; `now` is seconds since startup.
    pub fn receive(&mut self, weather: Weather, effects: Vec<String>, quick: bool, now: f32) {
        if self.forced {
            return;
        }
        // The first report is shown at once, rather than eased into from the menus' sky.
        if !self.reported {
            self.reported = true;
            self.current = weather;
            self.mean_wind = weather.wind;
        }
        if quick {
            self.quick_until = now + QUICK_FOR;
        }
        // Bring the gusts round to the server's over a second or so, the short way round.
        let half = Vec2::splat(GUST_WRAP / 2.0);
        self.gust_error = (weather.gust_offset - self.gust_offset + half).rem_euclid(Vec2::splat(GUST_WRAP)) - half;
        self.target = Some(weather);
        self.effects = effects;
    }

    /// What the gusts are seeded with, from the server's welcome.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed;
    }

    /// The server's simulation tick, told at `now` (seconds since startup).
    pub fn set_tick(&mut self, tick: u64, now: f64) {
        self.tick = tick;
        self.told_at = now;
    }

    /// Seconds the world has run, as the server counts them, at `now`.
    pub fn world_seconds(&self, now: f64) -> f64 {
        self.tick as f64 / TICKS_PER_SECOND as f64 + (now - self.told_at)
    }

    /// Lightning struck the ground at `at`, `distance` blocks away; `seed` shapes the bolt.
    pub fn lightning(&mut self, at: Vec3, seed: u32, distance: f32) {
        self.strikes.push(Strike { age: 0.0, distance, heard: false, at, seed });
    }

    /// How brightly lightning lights the sky and land now, 0..1: a few quick pulses per
    /// flash, fainter the farther it is.
    pub fn flash(&self) -> f32 {
        let flashes = self.strikes.iter().map(|s| pulses(s.age) / (1.0 + (s.distance / 400.0).powi(2)));
        flashes.sum::<f32>().min(1.0)
    }

    /// The lightning bolts to draw now.
    pub fn bolts(&self) -> impl Iterator<Item = Bolt> + '_ {
        self.strikes.iter().filter(|s| s.age < BOLT_LIFE).map(|s| {
            let (channel, branches) = bolt_shape(s.at, s.seed);
            Bolt { channel, branches, brightness: pulses(s.age).min(1.0) }
        })
    }

    /// Back to the menus' fair sky, unless a fixed weather was asked for.
    fn leave(&mut self) {
        if !self.forced {
            *self = Self::default();
        }
    }

    /// Shows `weather` from now on, ignoring the server.
    pub fn force(&mut self, weather: Weather) {
        self.forced = true;
        self.target = Some(weather);
        self.current = weather;
        self.mean_wind = weather.wind;
    }
}

/// A flash's brightness `age` seconds after it struck: a few quick pulses, then an
/// afterglow.
fn pulses(age: f32) -> f32 {
    let pulse = |at: f32, strength: f32| strength * (-((age - at) * 30.0).powi(2)).exp();
    pulse(0.02, 1.0) + pulse(0.14, 0.5) + pulse(0.3, 0.8) + 0.15 * (-age * 8.0).exp()
}

/// The shape of a bolt striking `ground`: a channel from a cloud base 150 to 220 blocks up,
/// zigzagging down in steps of about 10 blocks, and up to four branches. Pure in `seed`, so
/// every client draws the same bolt.
fn bolt_shape(ground: Vec3, seed: u32) -> (Vec<Vec3>, Vec<Vec<Vec3>>) {
    let mut h = pcg(seed ^ 0x626f_6c74); // "bolt"
    let mut random = move || {
        h = pcg(h);
        (h >> 8) as f32 / 16_777_216.0
    };
    let height = 150.0 + 70.0 * random();
    let top = ground + Vec3::new(random() - 0.5, 0.0, random() - 0.5) * 60.0 + Vec3::Y * height;
    let steps = (height / 10.0) as usize;
    let mut channel = Vec::with_capacity(steps + 1);
    let mut point = top;
    channel.push(point);
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        // Wander sideways, pulled ever harder towards the strike point.
        let straight = top.lerp(ground, t);
        let jitter = Vec3::new(random() - 0.5, 0.0, random() - 0.5) * 14.0 * (1.0 - t * 0.7);
        point = straight.lerp(point, 0.55 * (1.0 - t)) + jitter * (1.0 - t * t);
        point.y = straight.y;
        channel.push(point);
    }
    *channel.last_mut().unwrap() = ground;
    let branch_count = (random() * 5.0) as usize;
    let branches = (0..branch_count)
        .map(|_| {
            let start = 1 + (random() * (channel.len() as f32 * 0.6)) as usize;
            let mut point = channel[start.min(channel.len() - 1)];
            let heading = Vec3::new(random() - 0.5, 0.0, random() - 0.5).normalize_or(Vec3::X);
            let length = 3 + (random() * 6.0) as usize;
            (0..length)
                .map(|_| {
                    let out = point;
                    point += heading * (4.0 + 6.0 * random()) - Vec3::Y * (6.0 + 6.0 * random());
                    out
                })
                .collect()
        })
        .collect();
    (channel, branches)
}

/// Moves the shown weather towards the reported one at each channel's pace, lets the wind
/// gust at the camera and carries the clouds. Thunder follows each flash.
fn ease(
    time: Res<Time>,
    mut weather: ResMut<ClientWeather>,
    mut wind: ResMut<Wind>,
    mut thunder: MessageWriter<Thunder>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    let weather = &mut *weather;
    let quick = time.elapsed_secs() < weather.quick_until;
    for strike in &mut weather.strikes {
        strike.age += dt;
        if !strike.heard && strike.age >= strike.distance / SOUND_SPEED {
            strike.heard = true;
            thunder.write(Thunder { distance: strike.distance });
        }
    }
    weather.strikes.retain(|s| !s.heard || s.age < 1.0);
    if let Some(target) = weather.target {
        let pace = |seconds: f32| (dt / if quick { seconds.min(QUICK_EASE) } else { seconds }).min(1.0);
        let current = &mut weather.current;
        let goals = target.fractions();
        for ((value, (_, goal)), seconds) in current.fractions_mut().into_iter().zip(goals).zip(EASE) {
            *value += (goal - *value) * pace(seconds);
        }
        current.temperature += (target.temperature - current.temperature) * pace(20.0);
        // A layer of fog appearing forms at its own height rather than sinking from the last.
        if current.ground_fog < 0.01 {
            current.fog_top = target.fog_top;
        }
        current.fog_top += (target.fog_top - current.fog_top) * pace(10.0);
        current.dust_color = target.dust_color;
        weather.mean_wind += (target.wind - weather.mean_wind) * pace(10.0);
    }
    let correction = weather.gust_error * (dt / 1.2).min(1.0);
    weather.gust_error -= correction;
    weather.gust_offset = advance_gusts(weather.gust_offset + correction, weather.mean_wind, dt);
    let at = camera.single().map_or(Vec2::ZERO, |camera| camera.translation().xz());
    *wind = Wind {
        direction: weather.mean_wind.normalize_or(wind.direction),
        speed: weather.mean_wind.length(),
        gustiness: weather.current.gusts,
        seed: weather.seed,
        seconds: weather.world_seconds(time.elapsed_secs_f64()),
        offset: weather.gust_offset,
    };
    weather.current.wind = wind.now(at);
    // Clouds drift with the mean wind; gusts are too quick and too local to move them.
    weather.drift = (weather.drift + weather.mean_wind * dt).rem_euclid(Vec2::splat(DRIFT_WRAP));
}

/// Draws lightning bolts as bright lines, which bloom gives a glow, until the renderer
/// draws them properly (see docs/weather-effects.md).
fn draw_bolts(weather: Res<ClientWeather>, mut gizmos: Gizmos) {
    for bolt in weather.bolts() {
        let color = LinearRgba::rgb(6.0, 6.5, 8.0) * bolt.brightness;
        gizmos.linestrip(bolt.channel.iter().copied(), color);
        for branch in &bolt.branches {
            gizmos.linestrip(branch.iter().copied(), color * 0.5);
        }
    }
}

/// A kind of weather by name (`clear`, `storm`, `aurora`, …), as the content packages
/// define it, for `--weather`: its values and effects over a fair day, with any fog lying
/// on the sea.
pub fn preset(name: &str) -> Option<Weather> {
    let content = voxmar_server::load_content(&voxmar_server::assets_dir()).ok()?;
    forced(&content.weather, name)
}

/// `name`'s values and effects from `table` over a fair day; see `preset`.
fn forced(table: &WeatherTable, name: &str) -> Option<Weather> {
    Some(table.force(table.preset(name)?, &FAIR, voxmar_worldgen::SEA_LEVEL as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolts_reach_the_ground_and_are_the_same_everywhere() {
        let ground = Vec3::new(100.0, 140.0, -50.0);
        let (channel, branches) = bolt_shape(ground, 42);
        assert_eq!((channel.clone(), branches.clone()), bolt_shape(ground, 42));
        assert_ne!(channel, bolt_shape(ground, 43).0);
        assert_eq!(*channel.last().unwrap(), ground);
        assert!(channel[0].y > ground.y + 140.0);
        // Down all the way, never far from the line between its ends.
        assert!(channel.windows(2).all(|w| w[1].y < w[0].y));
        assert!(channel.iter().all(|p| p.xz().distance(ground.xz()) < 120.0));
        assert!(branches.len() <= 4 && branches.iter().all(|b| b.windows(2).all(|w| w[1].y < w[0].y)));
    }

    #[test]
    fn presets_come_from_content() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let table = voxmar_server::load_content(&assets).unwrap().weather;
        let fog = forced(&table, "fog").unwrap();
        assert!(fog.fog > 0.3 && fog.ground_fog > 0.9 && fog.fog_top > voxmar_worldgen::SEA_LEVEL as f32);
        assert!(forced(&table, "aurora").unwrap().aurora == 1.0);
        assert!(forced(&table, "nonsense").is_none());
    }
}
