//! The weather as the client sees it: the server's latest report for where the player is,
//! eased into over seconds so clouds form and rain sets in gradually, how far the wind has
//! carried the clouds, and lightning: a flash at once, thunder when the sound arrives.

use bevy::prelude::*;
use voxmar_protocol::Weather;

use crate::menu::Screen;

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientWeather>()
            .add_message::<Thunder>()
            .add_systems(Update, ease)
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
    /// What is shown now.
    pub current: Weather,
    /// How far the wind has carried the clouds, in blocks, wrapped every `DRIFT_WRAP`.
    pub drift: Vec2,
    strikes: Vec<Strike>,
}

/// Cloud drift wraps around after this many blocks, keeping shader coordinates small.
pub const DRIFT_WRAP: f32 = 1_048_576.0;
/// Blocks sound travels per second.
const SOUND_SPEED: f32 = 343.0;
/// Seconds a quick change takes to show, roughly.
const QUICK_EASE: f32 = 0.7;
const QUICK_FOR: f32 = 4.0;
/// The menus' sky, and what the game shows until the server reports: a fair day.
const FAIR: Weather = Weather {
    cover: 0.35,
    convection: 1.0,
    cirrus: 0.1,
    humidity: 0.5,
    precipitation: 0.0,
    snow: 0.0,
    wind: Vec2::new(3.0, 1.5),
};

/// A lightning flash, seen at once and heard later.
struct Strike {
    /// Seconds since the flash.
    age: f32,
    /// Blocks away.
    distance: f32,
    heard: bool,
}

/// Thunder reaching the player from lightning `distance` blocks away.
#[derive(Message, Clone, Copy, Debug)]
pub struct Thunder {
    pub distance: f32,
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
            drift: Vec2::ZERO,
            strikes: Vec::new(),
        }
    }
}

impl ClientWeather {
    /// The server's report for where the player is. `quick` (after a command changed the
    /// weather) changes over within a second or two rather than easing in; `now` is
    /// seconds since startup.
    pub fn receive(&mut self, weather: Weather, quick: bool, now: f32) {
        if self.forced {
            return;
        }
        // The first report is shown at once, rather than eased into from the menus' sky.
        if !self.reported {
            self.reported = true;
            self.current = weather;
        }
        if quick {
            self.quick_until = now + QUICK_FOR;
        }
        self.target = Some(weather);
    }

    /// Lightning flashed `distance` blocks away.
    pub fn lightning(&mut self, distance: f32) {
        self.strikes.push(Strike { age: 0.0, distance, heard: false });
    }

    /// How brightly lightning lights the sky and land now, 0..1: a few quick pulses per
    /// flash, fainter the farther it is.
    pub fn flash(&self) -> f32 {
        let pulses = |age: f32| {
            let pulse = |at: f32, strength: f32| strength * (-((age - at) * 30.0).powi(2)).exp();
            pulse(0.02, 1.0) + pulse(0.14, 0.5) + pulse(0.3, 0.8) + 0.15 * (-age * 8.0).exp()
        };
        let flashes = self.strikes.iter().map(|s| pulses(s.age) / (1.0 + (s.distance / 400.0).powi(2)));
        flashes.sum::<f32>().min(1.0)
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
    }
}

/// Moves the shown weather towards the reported one: cloud builds and clears over tens of
/// seconds, rain sets in and stops over a few. Thunder follows each flash.
fn ease(time: Res<Time>, mut weather: ResMut<ClientWeather>, mut thunder: MessageWriter<Thunder>) {
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
        let current = &mut weather.current;
        let approach = |value: &mut f32, target: f32, seconds: f32| {
            let seconds = if quick { seconds.min(QUICK_EASE) } else { seconds };
            *value += (target - *value) * (dt / seconds).min(1.0)
        };
        approach(&mut current.cover, target.cover, 25.0);
        approach(&mut current.convection, target.convection, 25.0);
        approach(&mut current.cirrus, target.cirrus, 30.0);
        approach(&mut current.humidity, target.humidity, 20.0);
        approach(&mut current.precipitation, target.precipitation, 6.0);
        approach(&mut current.snow, target.snow, 10.0);
        current.wind += (target.wind - current.wind) * (dt / if quick { QUICK_EASE } else { 10.0 }).min(1.0);
    }
    weather.drift = (weather.drift + weather.current.wind * dt).rem_euclid(Vec2::splat(DRIFT_WRAP));
}

/// A kind of weather by name (`clear`, `storm`, …), as the content packages define it, for
/// `--weather`.
pub fn preset(name: &str) -> Option<Weather> {
    let content = voxmar_server::load_content(&voxmar_server::assets_dir()).ok()?;
    content.weather.preset(name).map(|preset| preset.blend(&FAIR, 1.0))
}
