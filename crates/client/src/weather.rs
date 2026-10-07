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
use voxmar_engine::weather::{Ease as EaseData, GUST_WRAP, ShownTuning, WeatherTable, advance_gusts, gust, parse_intensity, pcg};
use voxmar_protocol::{Nearby, THUNDER_HEARD, TICKS_PER_SECOND, WEATHER_CELL, Weather};

use crate::menu::Screen;

pub mod atmosphere;

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(atmosphere::AtmospherePlugin)
            .init_resource::<ClientWeather>()
            .init_resource::<Wind>()
            .add_message::<Thunder>()
            .init_resource::<WeatherReady>()
            .add_systems(Update, (ease, ready.after(ease).after(atmosphere::AtmosphereSystems).run_if(in_state(Screen::InGame))))
            .add_systems(OnExit(Screen::InGame), |mut weather: ResMut<ClientWeather>, mut ready: ResMut<WeatherReady>| {
                weather.leave();
                ready.0 = false;
            });
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
    /// How fast each eased value is changing.
    speeds: Speeds,
    /// When (seconds since startup) the real weather first showed in this game.
    shown_since: Option<f32>,
    /// Whether `WeatherReady` has been said in this game: until then every report is shown
    /// at once, so the first frame shown is the latest report, not a change towards it.
    settled: bool,
    /// How far gust fronts have travelled, advanced locally, and how far it still has to
    /// move to agree with the server's last report.
    gust_offset: Vec2,
    gust_error: Vec2,
    /// Effects showing, by name, strongest first, as the server last said.
    pub effects: Vec<String>,
    /// How hard it rains or snows in the weather cells around, as the server last said.
    nearby: Nearby,
    /// How far the wind has carried the clouds, in blocks, wrapped every `DRIFT_WRAP`.
    pub drift: Vec2,
    /// What the gusts are seeded with, and the server's simulation tick when last told,
    /// with when that was (seconds since startup).
    seed: u64,
    tick: u64,
    told_at: f64,
    strikes: Vec<Strike>,
    /// How the shown weather follows the reports, from content.
    easing: std::sync::Arc<Easing>,
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
/// How the shown weather follows the server's, from `tuning: (shown: …)` in weather.ron
/// (`voxmar_engine::weather::ShownTuning`): cloud builds and clears over a minute, rain
/// and hail set in and stop over twenty seconds or so, fog gathers and lifts over half a
/// minute, and for `command_for` seconds after a command everything changes over within
/// several seconds, never at once.
#[derive(Clone, Debug, PartialEq)]
pub struct Easing {
    /// Each fraction of `Weather::fractions`.
    fractions: [Ease; 24],
    /// The least quick a fraction is after a command, and for how long after it.
    command: Ease,
    command_for: f32,
    /// The temperature (°C), the top of a layer of ground fog (blocks) and each component
    /// of the mean wind (blocks per second), and the wind after a command.
    temperature: Ease,
    fog_top: Ease,
    wind: Ease,
    command_wind: Ease,
    /// Seconds after the weather's first report that readiness waits at most for the
    /// land's air: its generator is built off the main thread, and a slow build must not
    /// hold the game up.
    ready_wait: f32,
    /// Seconds a bolt stays visible.
    bolt_life: f32,
}

impl Easing {
    pub fn new(t: &ShownTuning) -> Self {
        let ease = |e: EaseData| Ease { settle: e.settle, most: e.most, ramp: e.ramp };
        Self {
            fractions: t.swings().map(|s| Ease { settle: s.settle, most: 1.0 / s.full, ramp: t.fraction_ramp }),
            command: Ease { settle: t.command.settle, most: 1.0 / t.command.full, ramp: t.command_ramp },
            command_for: t.command_for,
            temperature: ease(t.temperature),
            fog_top: ease(t.fog_top),
            wind: ease(t.wind),
            command_wind: ease(t.command_wind),
            ready_wait: t.ready_wait,
            bolt_life: t.bolt_life,
        }
    }

    /// The base package's, until content says otherwise: the menus run before it loads.
    fn base() -> Self {
        static BASE: std::sync::LazyLock<Easing> = std::sync::LazyLock::new(|| {
            let table = WeatherTable::from_ron("base", include_str!("../../../assets/content/base/weather.ron")).expect("base weather.ron");
            Easing::new(&table.tuning().shown)
        });
        BASE.clone()
    }

    /// The most the shown fraction `i` (in the order of `Weather::fractions`) changes per
    /// second, even just after a command.
    pub fn fastest(&self, i: usize) -> f32 {
        self.fractions[i].quicker(self.command).most
    }
}

/// Longest step easing is worked out in: a longer frame is split into steps this long,
/// so the shown weather is the same at any frame rate.
const EASE_STEP: f32 = 1.0 / 30.0;

/// How one value of the shown weather follows the server's: it gathers speed over `ramp`
/// seconds, moves no faster than `most` per second, and settles into the goal over about
/// `settle` seconds (exponentially, without overshooting it). Fog, rain and their sounds
/// so always come and go gradually, whatever the server's values do.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ease {
    settle: f32,
    most: f32,
    ramp: f32,
}

impl Ease {
    /// The quicker of this and `other`, for changes a command made.
    fn quicker(self, other: Ease) -> Self {
        Self { settle: self.settle.min(other.settle), most: self.most.max(other.most), ramp: self.ramp.min(other.ramp) }
    }

    /// Moves `value`, changing at `speed` per second, towards `goal` over `dt` seconds (at
    /// most `EASE_STEP`).
    fn step(&self, value: &mut f32, speed: &mut f32, goal: f32, dt: f32) {
        let gap = goal - *value;
        let wanted = (gap / self.settle).clamp(-self.most, self.most);
        let change = self.most / self.ramp * dt;
        *speed += (wanted - *speed).clamp(-change, change);
        let moved = *speed * dt;
        // Never past the goal: on reaching it, it stops there.
        if gap != 0.0 && moved * gap > 0.0 && moved.abs() >= gap.abs() {
            *value = goal;
            *speed = 0.0;
        } else {
            *value += moved;
        }
    }
}

/// How fast each eased value of the shown weather is changing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Speeds {
    fractions: [f32; 24],
    temperature: f32,
    fog_top: f32,
    wind: Vec2,
}

/// Moves `shown` (and `mean_wind`, the mean wind shown) towards `target` over `dt`
/// seconds, quickly after a command. Pure, and the same however `dt` is split up.
fn approach(easing: &Easing, shown: &mut Weather, mean_wind: &mut Vec2, speeds: &mut Speeds, target: &Weather, mut dt: f32, quick: bool) {
    let quicken = |ease: Ease| if quick { ease.quicker(easing.command) } else { ease };
    let goals = target.fractions();
    while dt > 0.0 {
        let step = dt.min(EASE_STEP);
        dt -= step;
        for (i, value) in shown.fractions_mut().into_iter().enumerate() {
            quicken(easing.fractions[i]).step(value, &mut speeds.fractions[i], goals[i].1, step);
        }
        quicken(easing.temperature).step(&mut shown.temperature, &mut speeds.temperature, target.temperature, step);
        // A layer of fog appearing forms at its own height rather than sinking from the last.
        if shown.ground_fog < 0.01 {
            shown.fog_top = target.fog_top;
            speeds.fog_top = 0.0;
        }
        quicken(easing.fog_top).step(&mut shown.fog_top, &mut speeds.fog_top, target.fog_top, step);
        let wind = if quick { easing.wind.quicker(easing.command_wind) } else { easing.wind };
        wind.step(&mut mean_wind.x, &mut speeds.wind.x, target.wind.x, step);
        wind.step(&mut mean_wind.y, &mut speeds.wind.y, target.wind.y, step);
    }
    shown.dust_color = target.dust_color;
}

/// Whether the client shows the weather it is really in: true once the server's first
/// report of it (or a fixed weather, for captures) and the first sample of the land's air
/// there have been applied, from which moment it is shown without any jump. Until then
/// (and in the menus) the fair sky of the menus shows. A loading screen waits for it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeatherReady(pub bool);

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

/// A lightning strike: seen at once, heard later.
struct Strike {
    /// Seconds since the flash.
    age: f32,
    /// Blocks away.
    distance: f32,
    heard: bool,
    /// Where it struck the ground (or flashed, in the cloud), and the seed of its shape.
    at: Vec3,
    seed: u32,
    cloud: bool,
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
            speeds: Speeds::default(),
            shown_since: None,
            settled: false,
            gust_offset: Vec2::ZERO,
            gust_error: Vec2::ZERO,
            effects: Vec::new(),
            nearby: Nearby::default(),
            drift: Vec2::ZERO,
            seed: 0,
            tick: 0,
            told_at: 0.0,
            strikes: Vec::new(),
            easing: std::sync::Arc::new(Easing::base()),
        }
    }
}

impl ClientWeather {
    /// The server's report for where the player is, and the effects showing there.
    /// `quick` (after a command changed the weather) changes over within several seconds
    /// rather than over tens of them; `now` is seconds since startup.
    pub fn receive(&mut self, weather: Weather, effects: Vec<String>, nearby: Nearby, quick: bool, now: f32) {
        if self.forced {
            return;
        }
        // Until the weather is ready to show (see `WeatherReady`), each report is shown at
        // once, rather than eased into from the menus' sky or from an earlier report: on
        // joining, the server's report on arrival and its next scheduled one can come a
        // tick apart.
        if !self.settled {
            self.reported = true;
            self.current = weather;
            self.mean_wind = weather.wind;
            self.speeds = Speeds::default();
        }
        if quick {
            self.quick_until = now + self.easing.command_for;
        }
        // Bring the gusts round to the server's over a second or so, the short way round.
        let half = Vec2::splat(GUST_WRAP / 2.0);
        self.gust_error = (weather.gust_offset - self.gust_offset + half).rem_euclid(Vec2::splat(GUST_WRAP)) - half;
        self.target = Some(weather);
        self.effects = effects;
        self.nearby = nearby;
    }

    /// Whether the weather shown is the real one: the server's (or a fixed one), not the
    /// menus' fair sky. See `WeatherReady`, which also waits for the land's air.
    pub fn ready(&self) -> bool {
        self.reported || self.forced
    }

    /// What the server last reported (or the fixed weather), which `current` eases towards.
    pub fn target(&self) -> Option<&Weather> {
        self.target.as_ref()
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

    /// Lightning struck the ground at `at`, or with `cloud` flashed inside the cloud there,
    /// `distance` blocks away; `seed` shapes the bolt.
    pub fn lightning(&mut self, at: Vec3, seed: u32, distance: f32, cloud: bool) {
        self.strikes.push(Strike { age: 0.0, distance, heard: false, at, seed, cloud });
    }

    /// How brightly lightning lights the sky and land now, 0..1: a few quick pulses per
    /// flash, fainter the farther it is and when it stays within the cloud.
    pub fn flash(&self) -> f32 {
        let flashes = self.strikes.iter().map(|s| {
            let veiled = if s.cloud { 0.5 } else { 1.0 };
            veiled * pulses(s.age) / (1.0 + (s.distance / 400.0).powi(2))
        });
        flashes.sum::<f32>().min(1.0)
    }

    /// Flashes within clouds now, for lighting the clouds from inside: where (in the cloud,
    /// about 160 blocks over the ground) and how bright, 0..1. For the renderer; see
    /// docs/weather-effects.md.
    pub fn cloud_flashes(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.strikes.iter().filter(|s| s.cloud && s.age < self.easing.bolt_life * 2.0).map(|s| (s.at, pulses(s.age).min(1.0)))
    }

    /// How hard it rains or snows in each weather cell around (`WEATHER_CELL` blocks
    /// across), as the cell's corner (x, z) and 0..1: for drawing distant rain shafts and
    /// a storm front coming in.
    pub fn nearby_rain(&self) -> impl Iterator<Item = (Vec2, f32)> + '_ {
        let first = self.nearby.first;
        self.nearby.rain.iter().enumerate().map(move |(i, &rain)| {
            let cell = first + IVec2::new(i as i32 % 5, i as i32 / 5) * WEATHER_CELL;
            (cell.as_vec2(), rain as f32 / 255.0)
        })
    }

    /// The lightning bolts to draw now.
    pub fn bolts(&self) -> impl Iterator<Item = Bolt> + '_ {
        self.strikes.iter().filter(|s| !s.cloud && s.age < self.easing.bolt_life).map(|s| {
            let (channel, branches) = bolt_shape(s.at, s.seed);
            Bolt { channel, branches, brightness: pulses(s.age).min(1.0) }
        })
    }

    /// Back to the menus' fair sky, unless a fixed weather was asked for.
    fn leave(&mut self) {
        if !self.forced {
            *self = Self { easing: self.easing.clone(), ..Self::default() };
        }
        self.shown_since = None;
        self.settled = false;
    }

    /// Eases the shown weather as content says.
    pub fn tune(&mut self, easing: Easing) {
        self.easing = std::sync::Arc::new(easing);
    }

    /// How the shown weather follows the reports.
    pub fn easing(&self) -> &Easing {
        &self.easing
    }

    /// Shows `weather` from now on, ignoring the server.
    pub fn force(&mut self, weather: Weather) {
        self.forced = true;
        self.target = Some(weather);
        self.current = weather;
        self.mean_wind = weather.wind;
        self.speeds = Speeds::default();
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
            // Lightning too far off is seen but not heard (heat lightning).
            if strike.distance < THUNDER_HEARD {
                thunder.write(Thunder { distance: strike.distance });
            }
        }
    }
    weather.strikes.retain(|s| !s.heard || s.age < 1.0);
    if let Some(target) = weather.target {
        approach(&weather.easing, &mut weather.current, &mut weather.mean_wind, &mut weather.speeds, &target, dt, quick);
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

/// Says the weather is ready once the real weather and the land's air show (or the air
/// has been waited for long enough).
fn ready(time: Res<Time>, mut weather: ResMut<ClientWeather>, air: Res<atmosphere::ClientAtmosphere>, mut ready: ResMut<WeatherReady>) {
    if ready.0 || !weather.ready() {
        return;
    }
    let now = time.elapsed_secs();
    let since = *weather.shown_since.get_or_insert(now);
    if air.sampled() || now - since >= weather.easing.ready_wait {
        ready.0 = true;
        weather.settled = true;
        info!("weather: ready, showing {}", weather.current.summary());
    }
}

/// A kind of weather by name (`clear`, `storm`, `aurora`, …), as the content packages in
/// `table` define it, optionally at an intensity (`rain:0.3`, `snow:heavy`, or
/// `rain 0.3`), for `--weather`: its values and effects over a fair day, with any fog lying
/// on the sea. Names ignore case, and `-` stands for `_` (`sea-fog`). An error names the
/// kinds there are.
pub fn preset(table: &WeatherTable, asked: &str) -> Result<Weather, String> {
    forced(table, asked).ok_or_else(|| {
        let kinds: Vec<&str> = table.presets().iter().map(|p| p.short_name()).collect();
        format!("--weather: no weather `{asked}`; kinds are {}, optionally with :INTENSITY (0..1, light, moderate, heavy or max)", kinds.join(", "))
    })
}

/// `name`'s values and effects from `table` over a fair day; see `preset`.
fn forced(table: &WeatherTable, name: &str) -> Option<Weather> {
    let name = name.trim().to_ascii_lowercase().replace('-', "_");
    let (name, intensity) = match name.split_once([':', ' ']) {
        Some((name, intensity)) => (name, Some(parse_intensity(intensity.trim())?)),
        None => (name.as_str(), None),
    };
    let preset = table.preset(name)?;
    let preset = preset.at(intensity.unwrap_or(preset.intensity));
    Some(table.force(&preset, &FAIR, voxmar_worldgen::SEA_LEVEL as f32))
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

    /// Runs easing from `from` towards `to` for `seconds` in frames of `frame(n)` seconds,
    /// checking no frame changes any fraction faster than it may; returns the shown weather
    /// at each whole second.
    fn ease_over(from: Weather, to: Weather, seconds: f32, quick: bool, frame: impl Fn(usize) -> f32) -> Vec<Weather> {
        let easing = Easing::base();
        let (mut shown, mut wind, mut speeds) = (from, from.wind, Speeds::default());
        let (mut t, mut n, mut seconds_out) = (0.0, 0, vec![shown]);
        while t < seconds - 1e-4 {
            let mut dt = frame(n).min(seconds - t);
            // Stop on each whole second so frame rates can be compared there.
            let next_second = (t + 1e-4).floor() + 1.0;
            dt = dt.min(next_second - t);
            let before = shown.fractions();
            approach(&easing, &mut shown, &mut wind, &mut speeds, &to, dt, quick);
            for (i, ((name, was), (_, now))) in before.iter().zip(shown.fractions()).enumerate() {
                let most = if quick { easing.fastest(i) } else { easing.fractions[i].most };
                assert!((now - was).abs() <= most * dt + 1e-5, "{name} jumped by {} in {dt}s", now - was);
            }
            t += dt;
            n += 1;
            if (t - t.round()).abs() < 1e-4 {
                seconds_out.push(shown);
            }
        }
        seconds_out
    }

    fn foggy_downpour() -> Weather {
        Weather { fog: 0.9, ground_fog: 0.8, fog_top: 90.0, precipitation: 1.0, cover: 1.0, lightning: 0.8, wind: Vec2::new(12.0, 0.0), ..FAIR }
    }

    #[test]
    fn weather_eases_without_jumps_at_any_frame_rate() {
        let (from, to) = (foggy_downpour(), FAIR);
        // 30 and 144 frames a second, and an uneven rate with a two-second hitch.
        let slow = ease_over(from, to, 90.0, false, |_| 1.0 / 30.0);
        let fast = ease_over(from, to, 90.0, false, |_| 1.0 / 144.0);
        let uneven = ease_over(from, to, 90.0, false, |n| if n == 40 { 2.0 } else { [0.004, 0.02, 0.011][n % 3] });
        for ((a, b), c) in slow.iter().zip(&fast).zip(&uneven) {
            for ((name, a), ((_, b), (_, c))) in a.fractions().iter().zip(b.fractions().iter().zip(c.fractions())) {
                assert!((a - b).abs() < 0.01 && (a - c).abs() < 0.02, "{name}: {a} {b} {c}");
            }
        }
        // Fog and rain fade over tens of seconds: most of them is still there after five,
        // little after a minute; the sound follows the same values.
        let at = |second: usize| slow[second];
        assert!(at(5).fog > 0.7 && at(5).precipitation > 0.75, "{}", at(5).summary());
        assert!(at(15).fog < at(10).fog && at(15).fog > 0.25, "{}", at(15).fog);
        assert!(at(60).fog < 0.02 && at(60).precipitation < 0.02, "{}", at(60).summary());
        // Smoothly: it gathers speed rather than starting at full pace.
        assert!(from.fog - at(1).fog < 0.5 * (at(2).fog - at(3).fog).abs() + 0.01);
        // And back: fog gathers as gradually.
        let back = ease_over(to, from, 90.0, false, |_| 1.0 / 60.0);
        assert!(back[5].fog < 0.25 && back[60].fog > 0.85, "{} {}", back[5].fog, back[60].fog);
    }

    #[test]
    fn commands_change_the_weather_quickly_but_not_at_once() {
        let shown = ease_over(foggy_downpour(), FAIR, 15.0, true, |_| 1.0 / 60.0);
        assert!(shown[1].fog > 0.75 && shown[1].precipitation > 0.85, "{}", shown[1].summary());
        assert!(shown[3].fog < shown[1].fog && shown[3].fog > 0.3, "{}", shown[3].fog);
        assert!(shown[10].fog < 0.01 && shown[10].precipitation < 0.01 && shown[10].cover < 0.4, "{}", shown[10].summary());
    }

    #[test]
    fn joining_shows_the_servers_weather_at_once() {
        let mut weather = ClientWeather::default();
        assert!(!weather.ready());
        let storm = foggy_downpour();
        weather.receive(storm, Vec::new(), Nearby::default(), false, 3.0);
        assert!(weather.ready());
        assert_eq!(weather.current.fractions(), storm.fractions());
        assert_eq!(weather.mean_wind, storm.wind);
        // Until the weather is ready, a later report (the server's next, a tick after the
        // one sent on joining) is shown at once too: the first frame is the latest report.
        let later = Weather { fog: 0.5, ..storm };
        weather.receive(later, Vec::new(), Nearby::default(), false, 3.05);
        assert_eq!(weather.current.fractions(), later.fractions());
        assert_eq!(weather.target().map(|t| t.fractions()), Some(later.fractions()));
        // Once ready, later reports are eased into.
        weather.settled = true;
        weather.receive(FAIR, Vec::new(), Nearby::default(), false, 4.0);
        assert_eq!(weather.current.fractions(), later.fractions());
        // Leaving forgets it, so the next game starts at its own weather again.
        weather.leave();
        assert!(!weather.ready() && weather.current.fractions() == FAIR.fractions());
    }

    #[test]
    fn presets_come_from_content() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let table = voxmar_server::load_content(&assets).unwrap().weather;
        let fog = forced(&table, "fog:max").unwrap();
        assert!(fog.fog > 0.3 && fog.ground_fog > 0.9 && fog.fog_top > voxmar_worldgen::SEA_LEVEL as f32);
        assert!(forced(&table, "aurora").unwrap().aurora == 1.0);
        assert!(forced(&table, "nonsense").is_none());
        assert_eq!(forced(&table, "rain:0.3").unwrap().precipitation, 0.3);
        assert_eq!(forced(&table, "rain:max").unwrap().precipitation, 1.0);
        assert!(forced(&table, "rain:loud").is_none());
        // Every name captures and the README have used still works, in any case.
        for name in ["clear", "cirrus", "cumulus", "cloudy", "overcast", "rain", "storm", "snow", "Cumulus", "sea-fog", "rain 0.3"] {
            assert!(forced(&table, name).is_some(), "{name}");
        }
        let error = preset(&table, "hurricane").unwrap_err();
        assert!(error.contains("cumulus") && error.contains("INTENSITY"), "{error}");
    }
}
