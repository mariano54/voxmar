//! Weather: what the weather at a place is described by, the effects that make it more
//! than cloud and rain, the kinds of weather commands can ask for, and the gusting wind
//! server and clients share.
//!
//! The server simulates the weather and tells each player the `Weather` where they are;
//! clients only draw it. Everything a client draws (fog, hail, dust, rainbows, aurora, …)
//! is a channel of `Weather`, a fixed vocabulary the renderer knows. What raises those
//! channels is data: each package's `weather.ron` lists effects, each a product of smooth
//! conditions on a fixed set of inputs (the simulated weather, the local climate and land,
//! the time of day) with a rarity, and the channels it raises. So a mod adds effects (a
//! pollen haze in spring meadows, ash falling near volcanoes) by combining what exists,
//! and kinds of weather (`/weather storm`) by naming values and effects to force.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{Rarity, RegistryError};
use crate::glam::{DVec2, Vec2};

/// The weather at one place. Clients ease towards each update, so it may change in steps.
/// Fractions are 0..1. Gameplay reads the same values: how cold (`temperature`, `wind`),
/// wet (`precipitation`, `wetness`) and hard to see through (`visibility`) it is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    /// Fraction of the sky covered by low cloud.
    pub cover: f32,
    /// How that cloud is built: 0 flat layers (stratus) to 1 heaped towers (cumulus, and
    /// cumulonimbus when it rains).
    pub convection: f32,
    /// High, thin ice cloud (cirrus). It often runs ahead of rain.
    pub cirrus: f32,
    /// Relative humidity of the air.
    pub humidity: f32,
    /// Air temperature now, °C: the climate's mean, warmer by day and colder at night.
    pub temperature: f32,
    /// Mean wind along x and z, in blocks per second; `gust` adds the gusts.
    pub wind: Vec2,
    /// How much the wind gusts around its mean.
    pub gusts: f32,
    /// How far the gust fronts have travelled with the wind where the player is (x, z),
    /// wrapped every `GUST_WRAP` blocks; see `gust`.
    pub gust_offset: Vec2,
    /// Rain or snow reaching the ground, 0 (none) to 1 (downpour).
    pub precipitation: f32,
    /// How much of it falls as snow.
    pub snow: f32,
    /// How much of the rain is fine drizzle rather than drops.
    pub drizzle: f32,
    /// How much of what falls is hail, and how much is sleet (ice pellets).
    pub hail: f32,
    pub sleet: f32,
    /// Fog or mist filling the air (whiteout and sea fog too), 0 clear to 1 about 30
    /// blocks of visibility.
    pub fog: f32,
    /// Fog lying in a layer, thick below `fog_top` (a world height) and thinning above.
    pub ground_fog: f32,
    pub fog_top: f32,
    /// Sand or dust carried by the wind, and its colour (sRGB).
    pub dust: f32,
    pub dust_color: [u8; 3],
    /// How wet the ground is (puddles, dark wet surfaces), rising in rain and drying
    /// over minutes.
    pub wetness: f32,
    /// Snow lying on the ground, 0 bare to 1 deep.
    pub snow_cover: f32,
    /// Hoar frost on the ground and plants.
    pub frost: f32,
    /// How often lightning strikes nearby beyond what thunderstorm cells make
    /// (thundersnow, fantastical storms), 1 about once a minute.
    pub lightning: f32,
    /// Light shows the renderer draws where they belong: a rainbow opposite the sun, a
    /// halo and sun dogs around it, the aurora on the night sky, shimmering heat haze
    /// over hot ground, drifting balls of light, glowing motes in the air and shooting
    /// stars.
    pub rainbow: f32,
    pub halo: f32,
    pub aurora: f32,
    pub heat_haze: f32,
    pub wisps: f32,
    pub motes: f32,
    pub falling_stars: f32,
}

impl Weather {
    /// No weather at all: a still, clear, dry day at 15 °C.
    pub const NONE: Weather = Weather {
        cover: 0.0,
        convection: 0.0,
        cirrus: 0.0,
        humidity: 0.0,
        temperature: 15.0,
        wind: Vec2::ZERO,
        gusts: 0.0,
        gust_offset: Vec2::ZERO,
        precipitation: 0.0,
        snow: 0.0,
        drizzle: 0.0,
        hail: 0.0,
        sleet: 0.0,
        fog: 0.0,
        ground_fog: 0.0,
        fog_top: 0.0,
        dust: 0.0,
        dust_color: [210, 180, 130],
        wetness: 0.0,
        snow_cover: 0.0,
        frost: 0.0,
        lightning: 0.0,
        rainbow: 0.0,
        halo: 0.0,
        aurora: 0.0,
        heat_haze: 0.0,
        wisps: 0.0,
        motes: 0.0,
        falling_stars: 0.0,
    };

    /// Every fraction by name (all but `temperature`, `wind`, `fog_top` and `dust_color`),
    /// for checks and the debug overlay.
    pub fn fractions(&self) -> [(&'static str, f32); 24] {
        let mut copy = *self;
        let values = copy.fractions_mut().map(|v| *v);
        std::array::from_fn(|i| (FRACTIONS[i], values[i]))
    }

    /// The effects' channels by name, from drizzle on: what `/weather` and the F3 screen
    /// list after the numbers.
    pub fn channels(&self) -> impl Iterator<Item = (&'static str, f32)> + use<> {
        self.fractions().into_iter().skip(FIRST_CHANNEL)
    }

    /// Every fraction, in the order of `fractions`, for easing all of them alike.
    pub fn fractions_mut(&mut self) -> [&mut f32; 24] {
        [
            &mut self.cover,
            &mut self.convection,
            &mut self.cirrus,
            &mut self.humidity,
            &mut self.gusts,
            &mut self.precipitation,
            &mut self.snow,
            &mut self.drizzle,
            &mut self.hail,
            &mut self.sleet,
            &mut self.fog,
            &mut self.ground_fog,
            &mut self.dust,
            &mut self.wetness,
            &mut self.snow_cover,
            &mut self.frost,
            &mut self.lightning,
            &mut self.rainbow,
            &mut self.halo,
            &mut self.aurora,
            &mut self.heat_haze,
            &mut self.wisps,
            &mut self.motes,
            &mut self.falling_stars,
        ]
    }

    /// How far one can see, in blocks, near the ground: fog, dust, falling snow and
    /// heavy rain close it in. For gameplay, and for a renderer without its own fog model.
    pub fn visibility(&self) -> f32 {
        let falling = self.precipitation * (0.35 + 0.65 * self.snow);
        let murk = (self.fog + 0.8 * self.dust + 0.6 * falling + 0.5 * self.ground_fog).min(1.0);
        // From about 3 km in clear air to about 30 blocks in the thickest fog.
        3000.0 * (1.0 - murk) * (1.0 - murk) + 30.0
    }

    /// How cold it feels, °C, for comfort and later warmth buffs: wind chills cool air,
    /// rain and wet ground chill further, and heat is felt more in humid air.
    pub fn feels_like(&self) -> f32 {
        let t = self.temperature;
        let wind = self.wind.length();
        // About 1 °C colder per block/s of wind at 0 °C, less as it warms, none above 20 °C.
        let chill = wind.sqrt() * 2.5 * ramp(20.0, 0.0, t).max(0.0);
        let wet = 3.0 * self.precipitation * (1.0 - self.snow) + 1.5 * self.wetness;
        let muggy = 6.0 * ramp(24.0, 34.0, t) * ramp(0.5, 0.9, self.humidity);
        t - chill - wet * ramp(25.0, 10.0, t) + muggy
    }

    /// A short name for the weather: `heavy rain`, `cumulus`.
    pub fn describe(&self) -> &'static str {
        let falling = self.precipitation > 0.05;
        match () {
            _ if self.dust > 0.5 => "dust storm",
            _ if falling && self.snow > 0.5 && self.fog > 0.5 => "blizzard",
            _ if falling && self.snow > 0.5 => if self.precipitation > 0.5 { "heavy snow" } else { "snow" },
            _ if falling && self.hail > 0.3 => "hail",
            _ if falling && self.sleet > 0.3 => "sleet",
            _ if falling && self.convection > 0.5 && self.precipitation > 0.4 => "thunderstorm",
            _ if falling && self.drizzle > 0.5 => "drizzle",
            _ if falling => if self.precipitation > 0.5 { "heavy rain" } else { "rain" },
            _ if self.fog.max(self.ground_fog) > 0.4 => "fog",
            _ if self.fog.max(self.ground_fog) > 0.15 => "mist",
            _ if self.cover > 0.8 => "overcast",
            _ if self.cover > 0.2 => if self.convection > 0.5 { "cumulus" } else { "cloudy" },
            _ if self.cirrus > 0.3 => "cirrus",
            _ => "clear",
        }
    }

    /// The numbers behind it, for the debug overlay and `/weather`.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "{:.1} °C, cloud {:.0}% ({:.0}% heaped), cirrus {:.0}%, humidity {:.0}%, {} {:.0}%, wind {:.1} blocks/s, visibility {:.0} blocks",
            self.temperature,
            self.cover * 100.0,
            self.convection * 100.0,
            self.cirrus * 100.0,
            self.humidity * 100.0,
            if self.snow > 0.5 { "snow" } else { "rain" },
            self.precipitation * 100.0,
            self.wind.length(),
            self.visibility(),
        );
        for (name, value) in self.channels() {
            if value >= 0.05 {
                text.push_str(&format!(", {} {:.0}%", name.replace('_', " "), value * 100.0));
            }
        }
        text
    }
}

/// Where the effects' channels start in `FRACTIONS`: drizzle, the first after the numbers.
const FIRST_CHANNEL: usize = 7;

/// Names of `Weather::fractions`.
const FRACTIONS: [&str; 24] = [
    "cover",
    "convection",
    "cirrus",
    "humidity",
    "gusts",
    "precipitation",
    "snow",
    "drizzle",
    "hail",
    "sleet",
    "fog",
    "ground_fog",
    "dust",
    "wetness",
    "snow_cover",
    "frost",
    "lightning",
    "rainbow",
    "halo",
    "aurora",
    "heat_haze",
    "wisps",
    "motes",
    "falling_stars",
];

impl Default for Weather {
    fn default() -> Self {
        Self::NONE
    }
}

/// What effects' conditions read: the simulated weather where the effect would show, the
/// local climate and land, and the time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Input {
    /// Low cloud cover, 0..1.
    Cover,
    /// How heaped the cloud is, 0..1.
    Convection,
    Cirrus,
    /// Relative humidity, 0..1.
    Humidity,
    /// Rain or snow falling, 0..1.
    Precipitation,
    /// How much of it is snow, 0..1.
    Snow,
    /// Mean wind speed, blocks per second.
    Wind,
    /// Rising air in showers and storms, 0..1.
    Lift,
    /// Air temperature now, °C.
    Temperature,
    /// The climate's annual rain and snow, mm.
    Rainfall,
    /// Rain over evaporation: about 0.1 in deserts, 1 balanced, 2 in rainforests.
    Moisture,
    /// 1 at and off the coast, falling to 0 inland.
    Coast,
    /// 1 over the sea, a lake or a river.
    Water,
    /// Blocks above sea level.
    Altitude,
    /// Blocks below the land around: positive in valleys and hollows.
    Valley,
    /// 0 at the equator, 1 at a pole.
    Latitude,
    /// Ground cover and tree cover the climate supports, 0..1.
    Vegetation,
    Forest,
    /// How fantastical the land is, 0..1.
    Aether,
    /// 1 by day, 0 at night, with twilight between.
    Daylight,
    /// 1 around sunrise and the hours after, 0 otherwise; `Dusk` the same at sunset.
    Dawn,
    Dusk,
    /// 1 at night.
    Night,
    /// Height of the sun: 1 overhead, 0 on the horizon, negative below it.
    SunHeight,
    /// How wet the ground is, 0..1: rain wets it and it dries over minutes.
    Wetness,
    /// Snow lying on the ground, 0..1.
    SnowCover,
}

impl Input {
    pub const COUNT: usize = Input::SnowCover as usize + 1;
}

/// Every input's value at one place and time, indexed by `Input`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs(pub [f32; Input::COUNT]);

impl Inputs {
    pub fn get(&self, input: Input) -> f32 {
        self.0[input as usize]
    }

    pub fn set(&mut self, input: Input, value: f32) {
        self.0[input as usize] = value;
    }
}

impl Default for Inputs {
    fn default() -> Self {
        Self([0.0; Input::COUNT])
    }
}

/// What an effect raises: a channel of `Weather`, by an amount at full strength.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Show {
    Fog(f32),
    /// A layer of fog `depth` blocks deep over the lowest land nearby (or the sea).
    GroundFog { density: f32, depth: f32 },
    Gusts(f32),
    Drizzle(f32),
    Hail(f32),
    Sleet(f32),
    Dust(f32),
    Frost(f32),
    Lightning(f32),
    Rainbow(f32),
    Halo(f32),
    Aurora(f32),
    HeatHaze(f32),
    Wisps(f32),
    Motes(f32),
    FallingStars(f32),
}

impl Show {
    /// Raises the channel to this show at `strength`, if that is more than it has. The
    /// strongest effect wins each channel, so the order effects load in never matters.
    /// `floor` is the height ground fog lies on.
    pub fn apply(&self, weather: &mut Weather, strength: f32, floor: f32) {
        let raise = |channel: &mut f32, amount: f32| *channel = channel.max(amount * strength);
        match *self {
            Show::Fog(a) => raise(&mut weather.fog, a),
            Show::GroundFog { density, depth } => {
                if density * strength > weather.ground_fog {
                    weather.ground_fog = density * strength;
                    weather.fog_top = floor + depth;
                }
            }
            Show::Gusts(a) => raise(&mut weather.gusts, a),
            Show::Drizzle(a) => raise(&mut weather.drizzle, a),
            Show::Hail(a) => raise(&mut weather.hail, a),
            Show::Sleet(a) => raise(&mut weather.sleet, a),
            Show::Dust(a) => raise(&mut weather.dust, a),
            Show::Frost(a) => raise(&mut weather.frost, a),
            Show::Lightning(a) => raise(&mut weather.lightning, a),
            Show::Rainbow(a) => raise(&mut weather.rainbow, a),
            Show::Halo(a) => raise(&mut weather.halo, a),
            Show::Aurora(a) => raise(&mut weather.aurora, a),
            Show::HeatHaze(a) => raise(&mut weather.heat_haze, a),
            Show::Wisps(a) => raise(&mut weather.wisps, a),
            Show::Motes(a) => raise(&mut weather.motes, a),
            Show::FallingStars(a) => raise(&mut weather.falling_stars, a),
        }
    }
}

/// A weather effect: valley fog, hail, a sandstorm, the aurora.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    /// Namespaced once loaded (`base:valley_fog`).
    pub name: String,
    /// How often it may occur where its conditions hold: an uncommon effect in about a
    /// third of the stretches of land and time, a legendary one in about one in five
    /// hundred (the world generator's tiers). Each world also features a few rare effects
    /// and suppresses a common one, from its seed.
    #[serde(default)]
    pub rarity: Rarity,
    /// For gameplay and other systems to find effects by kind (`wet`, `cold`, `visibility`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Smooth ramps on one input each, `(input, from, to)`: 0 at `from`, 1 at `to` (which
    /// may be below `from`, for a falling ramp). The effect's strength is their product.
    #[serde(default)]
    pub when: Vec<(Input, f32, f32)>,
    pub shows: Vec<Show>,
    /// Typical minutes an occurrence lasts, and so how long the stretches of time its
    /// rarity is drawn for are.
    #[serde(default = "default_minutes")]
    pub minutes: f32,
}

fn default_minutes() -> f32 {
    20.0
}

impl Effect {
    /// The short name, without the namespace.
    pub fn short_name(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// How strongly the conditions hold, 0..1, before rarity.
    pub fn strength(&self, inputs: &Inputs) -> f32 {
        self.when.iter().map(|&(input, from, to)| ramp(from, to, inputs.get(input))).product()
    }
}

/// A kind of weather, as `/weather` and `--weather` ask for it. Values left out are the
/// natural ones where it is forced (rain falls as snow where it is freezing).
///
/// A preset describes its weather at its fullest; `intensity` (0..1) scales it, so
/// `/weather rain 0.2` is a light shower and `/weather rain 1` the heaviest downpour. What
/// falls and the effects held scale with it, the wind partly, and the sky not at all (any
/// rain comes from a grey sky).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preset {
    /// Namespaced once loaded (`base:rain`); asked for without the namespace.
    pub name: String,
    pub cover: Option<f32>,
    pub convection: Option<f32>,
    pub cirrus: Option<f32>,
    pub humidity: Option<f32>,
    pub precipitation: Option<f32>,
    pub snow: Option<f32>,
    /// Wind speed in blocks per second; it keeps blowing from where it does unless
    /// `direction` says otherwise.
    pub wind: Option<f32>,
    /// How much the wind gusts, 0..1.
    pub gusts: Option<f32>,
    /// The direction (x, z) the wind blows towards.
    pub direction: Option<Vec2>,
    /// Air temperature, °C: a cold snap lets snow lie, a heat wave dries the land.
    pub temperature: Option<f32>,
    /// Effects held, by name, whatever their conditions and rarity, as strong as the
    /// intensity.
    pub effects: Vec<String>,
    /// How intense it is when asked for without saying, 0..1; once scaled (`at`), how
    /// intense it is.
    pub intensity: f32,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: String::new(),
            cover: None,
            convection: None,
            cirrus: None,
            humidity: None,
            precipitation: None,
            snow: None,
            wind: None,
            gusts: None,
            direction: None,
            temperature: None,
            effects: Vec::new(),
            intensity: 1.0,
        }
    }
}

/// Names for intensities, as `/weather` accepts them.
pub const INTENSITIES: [(&str, f32); 5] = [("light", 0.25), ("moderate", 0.5), ("heavy", 0.75), ("max", 1.0), ("full", 1.0)];

/// An intensity typed as a number from 0 to 1 or a name (`light`, `moderate`, `heavy`,
/// `max`).
pub fn parse_intensity(word: &str) -> Option<f32> {
    let named = INTENSITIES.iter().find(|(name, _)| name.eq_ignore_ascii_case(word)).map(|&(_, v)| v);
    named.or_else(|| word.parse::<f32>().ok().filter(|v| (0.0..=1.0).contains(v)))
}

impl Preset {
    /// The short name, without the namespace.
    pub fn short_name(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// This preset at `intensity` (0..1): what falls and how hard the wind blows scaled,
    /// and its effects held that strongly.
    pub fn at(&self, intensity: f32) -> Preset {
        let intensity = intensity.clamp(0.0, 1.0);
        Preset {
            precipitation: self.precipitation.map(|p| p * intensity),
            wind: self.wind.map(|w| w * (0.35 + 0.65 * intensity)),
            gusts: self.gusts.map(|g| g * (0.5 + 0.5 * intensity)),
            intensity,
            ..self.clone()
        }
    }

    /// `natural` with this preset's values blended in by `weight` (0..1). Its effects are
    /// applied separately, by `WeatherTable::apply`.
    pub fn blend(&self, natural: &Weather, weight: f32) -> Weather {
        let mix = |natural: f32, forced: Option<f32>| forced.map_or(natural, |f| natural + (f - natural) * weight);
        let speed = natural.wind.length();
        let mut direction = natural.wind.normalize_or(Vec2::X);
        if let Some(forced) = self.direction.map(|d| d.normalize_or(direction)) {
            direction = direction.lerp(forced, weight).normalize_or(forced);
        }
        Weather {
            cover: mix(natural.cover, self.cover),
            convection: mix(natural.convection, self.convection),
            cirrus: mix(natural.cirrus, self.cirrus),
            humidity: mix(natural.humidity, self.humidity),
            precipitation: mix(natural.precipitation, self.precipitation),
            snow: mix(natural.snow, self.snow),
            temperature: mix(natural.temperature, self.temperature),
            wind: direction * mix(speed, self.wind),
            gusts: mix(natural.gusts, self.gusts),
            ..*natural
        }
    }
}

/// `weather.ron`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeatherFile {
    #[serde(default)]
    effects: Vec<Effect>,
    #[serde(default)]
    presets: Vec<Preset>,
    /// Winds `/wind` can bring, as presets that set only the wind.
    #[serde(default)]
    winds: Vec<Preset>,
    /// Ground blocks and the snowy blocks snow lying on them turns them into.
    #[serde(default)]
    snow_cover: Vec<(String, String)>,
    /// How the weather is simulated, sent and shown (see `WeatherTuning`), in full or only
    /// the values a package changes.
    #[serde(default)]
    tuning: Option<ron::Value>,
}

/// How the weather is simulated, told to players and shown: the `tuning` section of
/// `weather.ron`. The base package sets every value; a later package may set any of them
/// alone (`tuning: (shown: (fractions: {"fog": (settle: 30.0, full: 90.0)}))`) and the
/// rest stay as they were.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherTuning {
    pub simulation: SimulationTuning,
    pub shown: ShownTuning,
    pub air: AirTuning,
}

/// The server's simulation and what it tells players. Times are in seconds, distances in
/// blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationTuning {
    /// Weather cells simulated around each player in every direction.
    pub range: i32,
    /// Seconds a cell catches up on when it comes into range, in steps of `catch_up_step`.
    pub catch_up: i64,
    pub catch_up_step: i64,
    /// Mean speed of the prevailing wind that carries air masses, blocks per second.
    pub mean_wind: f64,
    /// A player's weather is sent when any value moves this much, or every `resend` seconds.
    pub report_change: f32,
    pub resend: u64,
    /// Chance per second that a cell under the fiercest storm strikes the ground (it
    /// flashes within the cloud twice as often), and how far players see and hear it.
    pub lightning: f32,
    pub lightning_reach: f32,
    /// Blocks above the ground (or the sea) a flash within a cloud lights it.
    pub cloud_flash_height: i32,
    /// Strikes per second near a player at full `lightning` from effects.
    pub effect_lightning: f32,
    /// Forced weather covers this far around where it was asked for, fades out over
    /// `force_edge` more, and when its time is up fades over `force_fade` seconds.
    pub force_radius: f32,
    pub force_edge: f32,
    pub force_fade: u64,
    /// Seconds after a command changes the weather during which players there are told at
    /// once and change over quickly.
    pub command_quick: u64,
    /// Blocks across the stretches of land an effect's rarity is drawn for.
    pub effect_region: f64,
    /// Effects at least this strong are named to players and in `/weather`.
    pub named: f32,
    /// Seconds for wet ground to lose half its water, in mild, cloudy weather.
    pub drying: f32,
    /// Columns whose lying snow is brought up to date per player each second, and blocks
    /// changed in one column at most.
    pub snow_columns: usize,
    pub snow_changes: usize,
    /// How much less snow cover snow in shade still lies at.
    pub shade_lingers: f32,
    /// Seconds wetness and snow left lying in cells out of range are remembered, and the
    /// most a returning cell catches up on.
    pub lying_kept: i64,
    pub lying_catch_up: i64,
    /// Seconds for a cell's count of recent lightning to halve.
    pub lightning_memory: f32,
    /// Hail's impact for each unit of `hail` squared.
    pub hail_energy: f32,
}

/// How clients show the weather they are told: each value gathers speed over `ramp`
/// seconds, changes at most `most` per second and settles into the report over about
/// `settle` seconds, so nothing seen or heard jumps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShownTuning {
    /// Each fraction of `Weather::fractions`, by name.
    pub fractions: FxHashMap<String, Swing>,
    /// Seconds every fraction takes to gather speed.
    pub fraction_ramp: f32,
    /// After a command, every fraction at least this quick, gathering speed this fast, for
    /// `command_for` seconds.
    pub command: Swing,
    pub command_ramp: f32,
    pub command_for: f32,
    /// The temperature (°C), the top of ground fog (blocks) and each component of the mean
    /// wind (blocks per second), and the wind after a command.
    pub temperature: Ease,
    pub fog_top: Ease,
    pub wind: Ease,
    pub command_wind: Ease,
    /// Seconds after the first report a game waits at most for the land's air before it
    /// says the weather is ready.
    pub ready_wait: f32,
    /// Seconds a lightning bolt stays visible.
    pub bolt_life: f32,
}

/// How a fraction follows the report: it settles over about `settle` seconds and swings
/// fully (0 to 1) over at least `full`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Swing {
    pub settle: f32,
    pub full: f32,
}

/// How a value follows the report: see `ShownTuning`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ease {
    pub settle: f32,
    pub most: f32,
    pub ramp: f32,
}

/// The land's air (haze, sky tint, ambient particles) on clients.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AirTuning {
    /// Seconds between samples of the land's air, and to ease most of the way to each.
    pub sample_every: f32,
    pub ease: f32,
    /// Share of nights each night-time particle set (fireflies) comes out on, and of days
    /// each daytime one (pollen) does.
    pub nights_out: f32,
    pub days_out: f32,
}

impl ShownTuning {
    /// Each fraction's swing, in the order of `Weather::fractions`.
    pub fn swings(&self) -> [Swing; 24] {
        FRACTIONS.map(|name| self.fractions[name])
    }
}

/// Lays `over` onto `base`: maps (structs) key by key, anything else replaced.
fn overlay(base: &mut ron::Value, over: ron::Value) {
    match (base, over) {
        (ron::Value::Map(base), ron::Value::Map(over)) => {
            for (key, value) in over {
                match base.get_mut(&key) {
                    Some(old) => overlay(old, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, over) => *base = over,
    }
}

/// Every package's weather effects and kinds of weather.
#[derive(Debug, Default)]
pub struct WeatherTable {
    effects: Vec<Effect>,
    presets: Vec<Preset>,
    /// Effects and presets by namespaced name, and by short name for the first package
    /// (in package order) to use it.
    effect_names: FxHashMap<String, usize>,
    preset_names: FxHashMap<String, usize>,
    winds: Vec<Preset>,
    snow_cover: Vec<(String, String)>,
    /// The tuning as every package so far has set it, and read, once complete.
    tuning_source: Option<ron::Value>,
    tuning: Option<WeatherTuning>,
}

impl WeatherTable {
    pub fn from_ron(namespace: &str, source: &str) -> Result<Self, RegistryError> {
        let mut table = Self::default();
        table.extend(namespace, source)?;
        Ok(table)
    }

    /// Adds a package's weather. Unqualified names are placed in `namespace`.
    pub fn extend(&mut self, namespace: &str, source: &str) -> Result<(), RegistryError> {
        let error = |message: String| RegistryError(format!("{namespace}/weather.ron: {message}"));
        // Optional values are written plainly (`snow: 1.0`), not as `Some(1.0)`.
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: WeatherFile = options.from_str(source).map_err(|e| error(e.to_string()))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        for mut effect in file.effects {
            qualify(&mut effect.name);
            let flat = effect.when.iter().any(|&(_, from, to)| from == to || !from.is_finite() || !to.is_finite());
            if flat || effect.minutes.is_nan() || effect.minutes <= 0.0 {
                return Err(error(format!("effect `{}` has a ramp that goes nowhere or no duration", effect.name)));
            }
            let index = self.effects.len();
            if self.effect_names.insert(effect.name.clone(), index).is_some() {
                return Err(error(format!("duplicate effect `{}`", effect.name)));
            }
            self.effect_names.entry(effect.short_name().to_owned()).or_insert(index);
            self.effects.push(effect);
        }
        for mut wind in file.winds {
            qualify(&mut wind.name);
            if wind.wind.is_none_or(|w| !(0.0..=40.0).contains(&w)) || wind.gusts.is_some_and(|g| !(0.0..=1.0).contains(&g)) {
                return Err(error(format!("wind `{}` needs a speed from 0 to 40 and gusts from 0 to 1", wind.name)));
            }
            self.winds.push(wind);
        }
        for mut preset in file.presets {
            qualify(&mut preset.name);
            let values = [preset.cover, preset.convection, preset.cirrus, preset.humidity, preset.precipitation, preset.snow];
            if values.into_iter().flatten().any(|v| !(0.0..=1.0).contains(&v))
                || preset.wind.is_some_and(|w| !(0.0..=40.0).contains(&w))
                || preset.gusts.is_some_and(|g| !(0.0..=1.0).contains(&g))
                || preset.temperature.is_some_and(|t| !(-60.0..=60.0).contains(&t))
                || !(0.0..=1.0).contains(&preset.intensity)
            {
                return Err(error(format!("weather `{}` has a value out of range", preset.name)));
            }
            let index = self.presets.len();
            if self.preset_names.insert(preset.name.clone(), index).is_some() {
                return Err(error(format!("duplicate weather `{}`", preset.name)));
            }
            self.preset_names.entry(preset.short_name().to_owned()).or_insert(index);
            self.presets.push(preset);
        }
        self.snow_cover.extend(file.snow_cover);
        if let Some(tuning) = file.tuning {
            match &mut self.tuning_source {
                Some(source) => overlay(source, tuning),
                None => self.tuning_source = Some(tuning),
            }
            // Complete once a package has set every value (checked in `validate`).
            self.tuning = self.read_tuning().ok();
        }
        Ok(())
    }

    fn read_tuning(&self) -> Result<WeatherTuning, RegistryError> {
        let source = self.tuning_source.clone().ok_or_else(|| RegistryError("no package's weather.ron sets its `tuning`".into()))?;
        let tuning: WeatherTuning = source.into_rust().map_err(|e| RegistryError(format!("weather.ron tuning: {e}")))?;
        let names = &tuning.shown.fractions;
        if let Some(missing) = FRACTIONS.iter().find(|name| !names.contains_key(**name)) {
            return Err(RegistryError(format!("weather.ron tuning: no easing for the fraction `{missing}`")));
        }
        if let Some(unknown) = names.keys().find(|name| !FRACTIONS.contains(&name.as_str())) {
            return Err(RegistryError(format!("weather.ron tuning: `{unknown}` is not a fraction of the weather")));
        }
        let swings = tuning.shown.swings().into_iter().chain([tuning.shown.command]);
        let eases = [tuning.shown.temperature, tuning.shown.fog_top, tuning.shown.wind, tuning.shown.command_wind];
        let positive = swings.flat_map(|s| [s.settle, s.full]).chain(eases.into_iter().flat_map(|e| [e.settle, e.most, e.ramp]));
        if positive.chain([tuning.shown.fraction_ramp, tuning.shown.command_ramp]).any(|v| v.is_nan() || v <= 0.0) {
            return Err(RegistryError("weather.ron tuning: easing times and rates must be above 0".into()));
        }
        Ok(tuning)
    }

    /// How the weather is simulated, sent and shown. Content that loaded has it (see
    /// `validate`).
    pub fn tuning(&self) -> &WeatherTuning {
        self.tuning.as_ref().expect("weather.ron sets no complete tuning")
    }

    /// The tuning, if the packages so far set all of it.
    pub fn try_tuning(&self) -> Option<&WeatherTuning> {
        self.tuning.as_ref()
    }

    /// Checks that every effect a preset forces exists, once all packages are loaded,
    /// and that the packages' tuning, if any set it, is complete.
    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.tuning_source.is_some() {
            self.read_tuning()?;
        }
        for preset in &self.presets {
            if let Some(missing) = preset.effects.iter().find(|name| self.effect(name).is_none()) {
                return Err(RegistryError(format!("weather `{}` forces `{missing}`, which no package defines", preset.name)));
            }
        }
        Ok(())
    }

    /// A preset by name, with or without its namespace.
    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.preset_names.get(name).map(|&i| &self.presets[i])
    }

    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// A wind `/wind` can bring, by short name.
    pub fn wind(&self, name: &str) -> Option<&Preset> {
        self.winds.iter().find(|w| w.short_name() == name || w.name == name)
    }

    pub fn winds(&self) -> &[Preset] {
        &self.winds
    }

    /// An effect's index by name, with or without its namespace.
    pub fn effect(&self, name: &str) -> Option<usize> {
        self.effect_names.get(name).copied()
    }

    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    /// Ground blocks, by name, and the snowy blocks snow lying on them turns them into.
    pub fn snow_cover(&self) -> &[(String, String)] {
        &self.snow_cover
    }

    /// Applies effects at their strengths (indexed like `effects`) to `weather`. `floor`
    /// is the height ground fog lies on.
    pub fn apply(&self, weather: &mut Weather, strengths: &[f32], floor: f32) {
        for (effect, &strength) in self.effects.iter().zip(strengths) {
            if strength > 0.0 {
                for show in &effect.shows {
                    show.apply(weather, strength, floor);
                }
            }
        }
    }

    /// `preset` forced over `natural` completely, effects and all, where nothing about
    /// the place is known: what `--weather` shows. Ground fog lies on `floor`.
    pub fn force(&self, preset: &Preset, natural: &Weather, floor: f32) -> Weather {
        let mut weather = preset.blend(natural, 1.0);
        let mut strengths = vec![0.0; self.effects.len()];
        for index in preset.effects.iter().filter_map(|name| self.effect(name)) {
            strengths[index] = preset.intensity;
        }
        self.apply(&mut weather, &strengths, floor);
        weather
    }
}

/// Smoothstep from `from` to `to`, falling if `to` is below `from`.
pub fn ramp(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Gust fronts repeat every this many blocks, so how far they have travelled can wrap.
pub const GUST_WRAP: f32 = 40.0 * GUST_CELLS as f32;
/// Lattice cells of the gust noise before it repeats.
const GUST_CELLS: u32 = 1024;

/// The wind at a place and moment: `mean` (blocks per second) with gusts as strong as
/// `gusts` (0..1) asks. Gust fronts some 40 blocks across change over a few seconds and
/// travel with the wind: `offset` is how far they have travelled, the mean wind integrated
/// over time (wrapped every `GUST_WRAP` blocks), which the server keeps for each player
/// and sends with the weather, and clients advance between reports. Integrating rather
/// than taking `mean * seconds` keeps the fronts from leaping when the mean wind changes.
/// Uses only exact arithmetic on 32-bit hashes, so it is the same on every machine and
/// easy to port to a shader (see docs/rendering.md).
pub fn gust(seed: u64, at: Vec2, seconds: f64, offset: Vec2, mean: Vec2, gusts: f32) -> Vec2 {
    if gusts <= 0.0 || mean == Vec2::ZERO {
        return mean;
    }
    let p = (at.as_dvec2() - offset.as_dvec2()) / 40.0;
    let t = seconds / 5.0;
    let seed = (seed ^ (seed >> 32)) as u32 ^ 0x6775_7374; // "gust"
    let strength = value_noise(seed, p, t);
    let veer = value_noise(seed ^ 0x9e37_79b9, p, t * 0.7) - 0.5;
    // Lulls a little below the mean, gusts up to about half again above it at the gustiest.
    let speed = 1.0 + gusts * (strength * strength - 0.25);
    mean * speed + mean.perp() * (veer * 0.4 * gusts)
}

/// `offset` carried `dt` seconds further by `mean`, wrapped every `GUST_WRAP` blocks.
pub fn advance_gusts(offset: Vec2, mean: Vec2, dt: f32) -> Vec2 {
    (offset + mean * dt).rem_euclid(Vec2::splat(GUST_WRAP))
}

/// Smooth noise in 0..1 over a plane and time, from 32-bit hashes on an integer lattice
/// that repeats every `GUST_CELLS` cells across.
fn value_noise(seed: u32, p: DVec2, t: f64) -> f32 {
    let (cell, cell_t) = (p.floor(), t.floor());
    let fade = |x: f32| x * x * (3.0 - 2.0 * x);
    let (fx, fz, ft) = (fade((p.x - cell.x) as f32), fade((p.y - cell.y) as f32), fade((t - cell_t) as f32));
    let wrap = |v: f64| (v as i64).rem_euclid(GUST_CELLS as i64) as u32;
    let (ix, iz, it) = (wrap(cell.x), wrap(cell.y), cell_t as i64 as u32);
    let corner = |dx: u32, dz: u32, dt: u32| {
        let (x, z) = ((ix + dx) % GUST_CELLS, (iz + dz) % GUST_CELLS);
        let h = pcg(seed ^ pcg(x ^ pcg(z ^ pcg(it.wrapping_add(dt)))));
        (h >> 8) as f32 / 16_777_216.0
    };
    let plane = |dt| {
        let near = corner(0, 0, dt) + (corner(1, 0, dt) - corner(0, 0, dt)) * fx;
        let far = corner(0, 1, dt) + (corner(1, 1, dt) - corner(0, 1, dt)) * fx;
        near + (far - near) * fz
    };
    plane(0) + (plane(1) - plane(0)) * ft
}

/// PCG hash (Jarzynski and Olano), the same in WGSL.
pub fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The channels start at drizzle, after the numbers `/weather` shows on their own.
    #[test]
    fn channels_start_at_drizzle() {
        assert_eq!(Weather::NONE.channels().next().map(|(name, _)| name), Some("drizzle"));
        assert!(Weather::NONE.channels().all(|(name, _)| !["gusts", "precipitation", "snow"].contains(&name)));
    }

    const TABLE: &str = r#"(
        effects: [
            (name: "valley_fog", rarity: Uncommon, when: [(Humidity, 0.6, 0.9), (Wind, 3.0, 1.0)], shows: [GroundFog(density: 0.9, depth: 12.0)]),
            (name: "mist", when: [(Wetness, 0.3, 0.8)], shows: [Fog(0.4)], minutes: 10.0),
        ],
        presets: [
            (name: "rain", cover: 1.0, precipitation: 0.5, wind: 6.0),
            (name: "fog", humidity: 1.0, effects: ["valley_fog", "mist"]),
        ],
        snow_cover: [("base:grass_block", "base:snowy_grass_block")],
    )"#;

    #[test]
    fn presets_blend_over_the_natural_weather() {
        let table = WeatherTable::from_ron("base", TABLE).unwrap();
        table.validate().unwrap();
        let rain = table.preset("rain").unwrap();
        assert_eq!(table.preset("base:rain"), Some(rain));
        let natural = Weather { cover: 0.2, snow: 1.0, wind: Vec2::new(0.0, -2.0), ..Weather::NONE };
        assert_eq!(rain.blend(&natural, 0.0), natural);
        let forced = rain.blend(&natural, 1.0);
        assert_eq!((forced.cover, forced.precipitation, forced.snow), (1.0, 0.5, 1.0), "snow stays natural");
        assert_eq!(forced.wind, Vec2::new(0.0, -6.0));
        assert!((rain.blend(&natural, 0.5).cover - 0.6).abs() < 1e-6);

        let light = rain.at(0.2);
        assert_eq!((light.precipitation, light.cover, light.intensity), (Some(0.1), Some(1.0), 0.2));
        assert!((light.wind.unwrap() - 6.0 * 0.48).abs() < 1e-5);
        assert_eq!(parse_intensity("heavy"), Some(0.75));
        assert_eq!(parse_intensity("0.3"), Some(0.3));
        assert_eq!(parse_intensity("3"), None);

        let fog = table.force(table.preset("fog").unwrap(), &natural, 70.0);
        assert_eq!((fog.ground_fog, fog.fog_top, fog.fog, fog.humidity), (0.9, 82.0, 0.4, 1.0));
        let thin = table.force(&table.preset("fog").unwrap().at(0.5), &natural, 70.0);
        assert_eq!((thin.ground_fog, thin.fog), (0.45, 0.2));
        assert_eq!(table.snow_cover().len(), 1);

        assert!(WeatherTable::from_ron("base", r#"(presets: [(name: "bad", cover: 2.0)])"#).is_err());
        let unknown = WeatherTable::from_ron("base", r#"(presets: [(name: "odd", effects: ["nothing"])])"#).unwrap();
        assert!(unknown.validate().is_err());
        assert!(WeatherTable::from_ron("base", r#"(effects: [(name: "flat", when: [(Wind, 1.0, 1.0)], shows: [])])"#).is_err());
    }

    /// The base package's tuning is what the values were when they were constants in the
    /// code, and a later package changes only what it sets.
    #[test]
    fn tuning_loads_from_data_and_packages_override_parts_of_it() {
        let base = include_str!("../../../assets/content/base/weather.ron");
        let mut table = WeatherTable::from_ron("base", base).unwrap();
        table.validate().unwrap();
        let t = table.tuning().clone();
        let sim = &t.simulation;
        assert_eq!((sim.range, sim.catch_up, sim.catch_up_step, sim.mean_wind), (2, 600, 5, 3.0));
        assert_eq!((sim.report_change, sim.resend), (0.02, 5));
        assert_eq!((sim.lightning, sim.lightning_reach, sim.cloud_flash_height, sim.effect_lightning), (0.0009, 1500.0, 160, 1.0 / 15.0));
        assert_eq!((sim.force_radius, sim.force_edge, sim.force_fade, sim.command_quick), (1024.0, 384.0, 60, 3));
        assert_eq!((sim.effect_region, sim.named, sim.drying), (1500.0, 0.3, 300.0));
        assert_eq!((sim.snow_columns, sim.snow_changes, sim.shade_lingers), (2, 256, 0.25));
        assert_eq!((sim.lying_kept, sim.lying_catch_up, sim.lightning_memory, sim.hail_energy), (3 * 3600, 3600, 600.0, 4.0));
        let shown = &t.shown;
        let old: [(f32, f32); 24] = [
            (20.0, 60.0), (20.0, 40.0), (25.0, 60.0), (15.0, 40.0), (4.0, 10.0), (8.0, 25.0), (8.0, 20.0), (8.0, 20.0),
            (4.0, 12.0), (6.0, 20.0), (12.0, 35.0), (15.0, 40.0), (8.0, 20.0), (10.0, 40.0), (20.0, 80.0), (25.0, 80.0),
            (3.0, 8.0), (6.0, 15.0), (10.0, 25.0), (15.0, 40.0), (8.0, 20.0), (5.0, 15.0), (6.0, 15.0), (8.0, 20.0),
        ];
        assert_eq!(shown.swings().map(|s| (s.settle, s.full)), old);
        assert_eq!((shown.fraction_ramp, shown.command, shown.command_ramp, shown.command_for), (2.0, Swing { settle: 1.0, full: 6.0 }, 0.5, 10.0));
        assert_eq!(shown.temperature, Ease { settle: 20.0, most: 0.5, ramp: 2.0 });
        assert_eq!(shown.fog_top, Ease { settle: 10.0, most: 3.0, ramp: 2.0 });
        assert_eq!(shown.wind, Ease { settle: 8.0, most: 1.5, ramp: 2.0 });
        assert_eq!(shown.command_wind, Ease { settle: 1.0, most: 4.0, ramp: 0.5 });
        assert_eq!((shown.ready_wait, shown.bolt_life), (5.0, 0.45));
        assert_eq!(t.air, AirTuning { sample_every: 0.5, ease: 10.0, nights_out: 0.6, days_out: 0.35 });

        // A mod slows the fog and nothing else.
        table.extend("slow_fog", r#"(tuning: (shown: (fractions: {"fog": (settle: 30.0, full: 90.0)}), simulation: (named: 0.5)))"#).unwrap();
        table.validate().unwrap();
        let changed = table.tuning();
        assert_eq!(changed.shown.fractions["fog"], Swing { settle: 30.0, full: 90.0 });
        assert_eq!(changed.simulation.named, 0.5);
        assert_eq!(changed.shown.fractions["cover"], t.shown.fractions["cover"]);
        assert_eq!((&changed.air, changed.simulation.drying), (&t.air, t.simulation.drying));
        // Partial tuning alone is not enough, and nonsense is refused.
        let partial = WeatherTable::from_ron("odd", r#"(tuning: (simulation: (named: 0.5)))"#).unwrap();
        assert!(partial.try_tuning().is_none() && partial.validate().is_err());
        table.extend("bad", r#"(tuning: (shown: (fractions: {"fog": (settle: 0.0, full: 90.0)})))"#).unwrap();
        assert!(table.validate().is_err());
        assert!(WeatherTable::from_ron("bad", r#"(tuning: (shown: (fractions: {"smog": (settle: 1.0, full: 1.0)})))"#).unwrap().validate().is_err());
    }

    #[test]
    fn effects_follow_their_conditions_and_the_strongest_wins() {
        let table = WeatherTable::from_ron("base", TABLE).unwrap();
        let fog = &table.effects()[table.effect("valley_fog").unwrap()];
        let mut inputs = Inputs::default();
        inputs.set(Input::Humidity, 0.95);
        inputs.set(Input::Wind, 0.5);
        assert_eq!(fog.strength(&inputs), 1.0);
        inputs.set(Input::Wind, 2.0);
        assert!((fog.strength(&inputs) - 0.5).abs() < 1e-6, "halfway down a falling ramp");
        inputs.set(Input::Humidity, 0.5);
        assert_eq!(fog.strength(&inputs), 0.0);

        let mut weather = Weather::NONE;
        table.apply(&mut weather, &[0.5, 1.0], 64.0);
        table.apply(&mut weather, &[0.2, 0.5], 90.0);
        assert_eq!((weather.ground_fog, weather.fog_top, weather.fog), (0.45, 76.0, 0.4));
        assert_eq!(weather.fractions()[10], ("fog", 0.4));
    }

    #[test]
    fn wind_and_rain_feel_colder_and_humid_heat_hotter() {
        let calm = Weather { temperature: 2.0, ..Weather::NONE };
        assert_eq!(calm.feels_like(), 2.0);
        let windy = Weather { wind: Vec2::new(9.0, 0.0), ..calm };
        assert!(windy.feels_like() < -4.0, "{}", windy.feels_like());
        let wet = Weather { precipitation: 1.0, wetness: 1.0, ..calm };
        assert!(wet.feels_like() < -1.5);
        let muggy = Weather { temperature: 33.0, humidity: 0.9, ..Weather::NONE };
        assert!(muggy.feels_like() > 37.0);
    }

    #[test]
    fn gusts_are_deterministic_and_bounded() {
        let mean = Vec2::new(6.0, -2.0);
        let at = Vec2::new(1234.5, -987.25);
        assert_eq!(gust(7, at, 1e7, Vec2::ZERO, mean, 0.0), mean);
        let mut offset = Vec2::new(40_000.0, 3.0);
        let mut speeds = Vec::new();
        for i in 0..2000 {
            speeds.push(gust(7, at, 1e7 + i as f64 * 0.25, offset, mean, 1.0).length());
            offset = advance_gusts(offset, mean, 0.25);
        }
        assert_eq!(gust(7, at, 1e7 + 3.0, offset, mean, 1.0), gust(7, at, 1e7 + 3.0, offset, mean, 1.0));
        assert_ne!(gust(7, at, 1e7 + 3.0, offset, mean, 1.0), gust(8, at, 1e7 + 3.0, offset, mean, 1.0));
        let (low, high) = speeds.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        let base = mean.length();
        assert!(low > base * 0.7 && high < base * 1.85 && high > base * 1.4, "{low}..{high} around {base}");
        // Gusts change within seconds, not every frame, even as the offset wraps round.
        let step = speeds.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(step < base * 0.12, "jumps {step}");
        // The pattern repeats exactly where the offset wraps.
        let (a, b) = (Vec2::new(5.0, 7.0), Vec2::new(5.0 + GUST_WRAP, 7.0 - GUST_WRAP));
        assert_eq!(gust(3, a, 100.0, Vec2::ZERO, mean, 1.0), gust(3, b, 100.0, Vec2::ZERO, mean, 1.0));
    }
}
