//! The weather as the client sees it: the server's latest report for where the player is,
//! eased into over seconds so clouds form and rain sets in gradually, plus how far the
//! wind has carried the clouds.

use bevy::prelude::*;
use voxmar_protocol::Weather;

use crate::menu::Screen;

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientWeather>()
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
    /// What is shown now.
    pub current: Weather,
    /// How far the wind has carried the clouds, in blocks, wrapped every `DRIFT_WRAP`.
    pub drift: Vec2,
}

/// Cloud drift wraps around after this many blocks, keeping shader coordinates small.
pub const DRIFT_WRAP: f32 = 1_048_576.0;

/// Until a server reports the weather, and in the menus: a fair day.
impl Default for ClientWeather {
    fn default() -> Self {
        let fair = preset("cumulus").expect("cumulus is a preset");
        Self { target: Some(fair), reported: false, forced: false, current: fair, drift: Vec2::ZERO }
    }
}

impl ClientWeather {
    pub fn receive(&mut self, weather: Weather) {
        if self.forced {
            return;
        }
        // The first report is shown at once, rather than eased into from the menus' sky.
        if !self.reported {
            self.reported = true;
            self.current = weather;
        }
        self.target = Some(weather);
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
/// seconds, rain sets in and stops over a few.
fn ease(time: Res<Time>, mut weather: ResMut<ClientWeather>) {
    let dt = time.delta_secs();
    let weather = &mut *weather;
    if let Some(target) = weather.target {
        let current = &mut weather.current;
        let approach = |value: &mut f32, target: f32, seconds: f32| *value += (target - *value) * (dt / seconds).min(1.0);
        approach(&mut current.cover, target.cover, 25.0);
        approach(&mut current.convection, target.convection, 25.0);
        approach(&mut current.cirrus, target.cirrus, 30.0);
        approach(&mut current.humidity, target.humidity, 20.0);
        approach(&mut current.precipitation, target.precipitation, 6.0);
        approach(&mut current.snow, target.snow, 10.0);
        current.wind += (target.wind - current.wind) * (dt / 10.0).min(1.0);
    }
    weather.drift = (weather.drift + weather.current.wind * dt).rem_euclid(Vec2::splat(DRIFT_WRAP));
}

/// A short description of the weather, for the debug overlay.
pub fn describe(weather: &Weather) -> &'static str {
    let falling = weather.precipitation > 0.05;
    match () {
        _ if falling && weather.snow > 0.5 => if weather.precipitation > 0.5 { "heavy snow" } else { "snow" },
        _ if falling && weather.convection > 0.5 && weather.precipitation > 0.4 => "thunderstorm",
        _ if falling => if weather.precipitation > 0.5 { "heavy rain" } else { "rain" },
        _ if weather.cover > 0.8 => "overcast",
        _ if weather.cover > 0.2 => if weather.convection > 0.5 { "cumulus" } else { "cloudy" },
        _ if weather.cirrus > 0.3 => "cirrus",
        _ => "clear",
    }
}

/// Named weather for captures and checks.
pub fn preset(name: &str) -> Option<Weather> {
    let weather = |cover, convection, cirrus, humidity, precipitation, snow, wind: [f32; 2]| Weather {
        cover,
        convection,
        cirrus,
        humidity,
        precipitation,
        snow,
        wind: Vec2::from(wind),
    };
    Some(match name {
        "clear" => weather(0.0, 0.5, 0.0, 0.3, 0.0, 0.0, [2.0, 1.0]),
        "cirrus" => weather(0.05, 0.3, 0.9, 0.35, 0.0, 0.0, [4.0, 1.0]),
        "cumulus" => weather(0.35, 1.0, 0.1, 0.5, 0.0, 0.0, [3.0, 1.5]),
        "cloudy" => weather(0.6, 0.3, 0.0, 0.6, 0.0, 0.0, [4.0, 2.0]),
        "overcast" => weather(0.95, 0.0, 0.0, 0.75, 0.0, 0.0, [5.0, 2.0]),
        "rain" => weather(0.98, 0.1, 0.0, 0.85, 0.5, 0.0, [5.0, 2.0]),
        "storm" => weather(1.0, 1.0, 0.0, 0.9, 1.0, 0.0, [10.0, 4.0]),
        "snow" => weather(0.95, 0.0, 0.0, 0.8, 0.6, 1.0, [3.0, 1.0]),
        _ => return None,
    })
}
