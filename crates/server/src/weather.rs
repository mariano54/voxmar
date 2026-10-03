//! Weather: a Low-tier process that evolves humidity, cloud and precipitation fields near
//! players and tells each player what the sky is doing where they stand.
//!
//! What drives the weather is a pure function of the world seed, place and time: moist and
//! dry air masses a few kilometres across that drift with the prevailing wind and slowly
//! change shape, and smaller, shorter-lived updrafts that are strongest on warm
//! afternoons. Drivers need no state, so the weather exists everywhere without being
//! simulated everywhere. Near players the process integrates how each cell responds:
//! humidity follows the passing air mass from the climate's baseline, cloud condenses as
//! the air nears saturation and evaporates as it dries, and thick cloud rains, which dries
//! the air again. A cell coming into range first catches up on the last ten minutes, so
//! the weather at a place does not depend on when anyone arrived, and cells left behind
//! return to their climate baseline.
//!
//! Only exact floating point operations are used (no `sin`, `exp` or `powf`), so every
//! machine computes the same weather.

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::field::Field;
use voxmar_engine::glam::{DVec2, IVec2, Vec2};
use voxmar_engine::schedule::Tier;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND, Weather as LocalWeather};

use crate::Server;
use crate::process::Process;

/// Blocks per weather cell.
const CELL: i32 = 128;
/// Cells simulated around each player in every direction, enough to interpolate between.
const RANGE: i32 = 2;
/// Seconds a cell catches up on when it comes into range, in steps of `CATCH_UP_STEP`.
const CATCH_UP: i64 = 600;
const CATCH_UP_STEP: i64 = 5;
/// Mean speed of the prevailing wind that carries air masses, in blocks per second.
const MEAN_WIND: f64 = 3.0;
/// A player's weather is sent when any value moves this much, or after `RESEND` ticks.
const CHANGE: f32 = 0.02;
const RESEND: u64 = 5 * TICKS_PER_SECOND as u64;
/// Chance per second that a cell under the fiercest storm flashes, and how far away, in
/// blocks, players see and hear lightning.
const LIGHTNING: f32 = 0.015;
const LIGHTNING_REACH: f32 = 1500.0;

/// Random streams, each mixed into the world seed.
const AIR_MASS: u64 = 1;
const UPDRAFT: u64 = 2;
const GUSTS: u64 = 3;
const CIRRUS: u64 = 4;
const STRIKES: u64 = 5;

/// `core:humidity` (relative), `core:clouds` (cloud cover) and `core:precipitation` (rain
/// or snow reaching the ground), all 0..1.
pub(crate) struct WeatherFields {
    pub humidity: Field,
    pub clouds: Field,
    pub precipitation: Field,
}

impl WeatherFields {
    /// `rainfall(x, z)` is the climate's annual precipitation in mm; humidity's baseline,
    /// the air's usual humidity, follows it, so wet climates rain often and dry ones rarely.
    pub fn new(rainfall: impl Fn(i32, i32) -> f32 + Send + Sync + 'static) -> Self {
        Self {
            humidity: Field::with_cell_size("core:humidity", CELL, move |x, z| usual_humidity(rainfall(x, z))),
            clouds: Field::with_cell_size("core:clouds", CELL, |_, _| 0.0),
            precipitation: Field::with_cell_size("core:precipitation", CELL, |_, _| 0.0),
        }
    }

    fn get(&mut self, at: IVec2) -> Cell {
        Cell {
            humidity: self.humidity.sample(at.x, at.y),
            clouds: self.clouds.sample(at.x, at.y),
            precipitation: self.precipitation.sample(at.x, at.y),
        }
    }

    fn set(&mut self, at: IVec2, cell: Cell) {
        self.humidity.set(at.x, at.y, cell.humidity);
        self.clouds.set(at.x, at.y, cell.clouds);
        self.precipitation.set(at.x, at.y, cell.precipitation);
    }

    fn reset(&mut self, at: IVec2) {
        self.humidity.reset(at.x, at.y);
        self.clouds.reset(at.x, at.y);
        self.precipitation.reset(at.x, at.y);
    }
}

/// The evolving state of one weather cell.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    humidity: f32,
    clouds: f32,
    precipitation: f32,
}

/// What drives the weather at a place and time.
struct Drivers {
    /// Moisture of the passing air mass relative to the climate, about -1..1.
    air: f32,
    /// Rising air, 0..1. It condenses cloud sooner and builds showers and storms.
    lift: f32,
}

pub(crate) struct Weather {
    seed: u64,
    /// Direction of the prevailing wind, fixed per world.
    prevailing: DVec2,
    /// Cells simulated in the last run.
    active: FxHashSet<IVec2>,
    /// The weather each player was last sent, and when.
    sent: FxHashMap<PlayerId, (LocalWeather, u64)>,
}

impl Process for Weather {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let tick = server.tick;
        let positions: Vec<(PlayerId, Vec2)> =
            server.players.values().filter(|p| p.joined).map(|p| (p.id, Vec2::new(p.position.x, p.position.z))).collect();
        let around: Vec<IVec2> = positions.iter().map(|(_, at)| at.floor().as_ivec2()).collect();
        let strikes = self.update(&mut server.weather, &mut server.temperature, &around, tick);
        for at in strikes {
            for (id, player) in &positions {
                if player.distance(at) < LIGHTNING_REACH {
                    server.players[id].connection.send(ServerMessage::Lightning { at });
                }
            }
        }

        self.sent.retain(|id, _| server.players.contains_key(id));
        for (id, at) in positions {
            let weather = self.local(&mut server.weather, &mut server.temperature, at, tick);
            let due = self.sent.get(&id).is_none_or(|(last, sent_at)| tick >= sent_at + RESEND || changed(last, &weather));
            if due {
                self.sent.insert(id, (weather, tick));
                server.players[&id].connection.send(ServerMessage::Weather(weather));
            }
        }
    }
}

impl Weather {
    /// `wind` is the direction (x, z) the world's prevailing wind blows towards.
    pub fn new(world_seed: u64, wind: Vec2) -> Self {
        Self {
            seed: mix(world_seed ^ 0x0077_6561_7468_6572), // "weather"
            prevailing: wind.as_dvec2().normalize_or(DVec2::X),
            active: FxHashSet::default(),
            sent: FxHashMap::default(),
        }
    }

    /// Advances the cells within range of each block position in `around` to `tick`, and
    /// returns cells no longer in range to their baseline. Returns where lightning flashed.
    fn update(&mut self, fields: &mut WeatherFields, temperature: &mut Field, around: &[IVec2], tick: u64) -> Vec<Vec2> {
        let mut cells = FxHashSet::default();
        for at in around {
            let center = at.div_euclid(IVec2::splat(CELL));
            for dz in -RANGE..=RANGE {
                for dx in -RANGE..=RANGE {
                    cells.insert(center + IVec2::new(dx, dz));
                }
            }
        }
        for &cell in self.active.difference(&cells) {
            fields.reset(cell * CELL);
        }
        let dt = Tier::Low.period() as f32 / TICKS_PER_SECOND as f32;
        let mut strikes = Vec::new();
        for &cell in &cells {
            let at = cell * CELL + CELL / 2;
            let climate = fields.humidity.baseline(at.x, at.y);
            let warmth = warmth(temperature.sample(at.x, at.y));
            let state = if self.active.contains(&cell) {
                let drivers = self.drivers(at.as_dvec2(), tick as i64, warmth);
                let state = step(fields.get(at), &drivers, climate, warmth, dt);
                strikes.extend(self.lightning(cell, state.precipitation, drivers.lift, tick));
                state
            } else {
                // Catch up from the climate's average state.
                let mut state = Cell { humidity: climate, clouds: 0.0, precipitation: 0.0 };
                let start = tick as i64 - CATCH_UP * TICKS_PER_SECOND as i64;
                for i in 1..=CATCH_UP / CATCH_UP_STEP {
                    let t = start + i * CATCH_UP_STEP * TICKS_PER_SECOND as i64;
                    state = step(state, &self.drivers(at.as_dvec2(), t, warmth), climate, warmth, CATCH_UP_STEP as f32);
                }
                state
            };
            fields.set(at, state);
        }
        self.active = cells;
        strikes
    }

    /// Where lightning flashes in a cell this second, if it does: a downpour under strong
    /// updrafts is a thunderstorm, which has a chance each second to flash somewhere in
    /// the cell, drawn from the cell's own random stream.
    fn lightning(&self, cell: IVec2, rain: f32, lift: f32, tick: u64) -> Option<Vec2> {
        let chance = LIGHTNING * smoothstep(0.25, 0.8, rain * smoothstep(0.1, 0.6, lift));
        let h = mix(mix(mix(self.seed ^ STRIKES ^ tick) ^ cell.x as u64) ^ cell.y as u64);
        let place = Vec2::new(unit(mix(h)) as f32, unit(mix(mix(h))) as f32);
        (unit(h) < chance as f64).then(|| (cell.as_vec2() + place) * CELL as f32)
    }

    /// The air mass and lift at a block position and tick. `temperature`, the warmth (see
    /// `warmth`), sets how readily the ground heats air into updrafts.
    fn drivers(&self, at: DVec2, tick: i64, temperature: f32) -> Drivers {
        let seconds = tick as f64 / TICKS_PER_SECOND as f64;
        let p = at - self.prevailing * (MEAN_WIND * seconds);
        let air = 0.65 * self.noise(AIR_MASS, p / 3000.0, seconds / 2400.0)
            + 0.35 * self.noise(AIR_MASS + 16, p / 1100.0, seconds / 1000.0);
        let updraft = smoothstep(0.5, 0.75, self.noise(UPDRAFT, p / 700.0, seconds / 600.0) as f32);
        // Sun-warmed ground lifts air most in the afternoon, and hardly at all when cold.
        let day = tick.rem_euclid(TICKS_PER_DAY as i64) as f64 / TICKS_PER_DAY as f64;
        let from_peak = (day - 0.32) / 0.24;
        let afternoon = (1.0 - from_peak * from_peak).max(0.0) as f32;
        let heating = smoothstep(0.2, 0.7, temperature) * (0.35 + 0.65 * afternoon);
        Drivers { air: ((air - 0.5) * 3.2) as f32, lift: updraft * heating }
    }

    /// The weather at a block position: the fields interpolated between the four nearest
    /// cell centres, the kind of cloud from the drivers, and snow where it is freezing.
    fn local(&self, fields: &mut WeatherFields, temperature: &mut Field, at: Vec2, tick: u64) -> LocalWeather {
        let corner = ((at - CELL as f32 / 2.0) / CELL as f32).floor();
        let f = (at - CELL as f32 / 2.0) / CELL as f32 - corner;
        let corner = corner.as_ivec2() * CELL + CELL / 2;
        let mut sum = |dx: i32, dz: i32, weight: f32| {
            let cell = fields.get(corner + IVec2::new(dx, dz) * CELL);
            [cell.humidity * weight, cell.clouds * weight, cell.precipitation * weight]
        };
        let corners = [
            sum(0, 0, (1.0 - f.x) * (1.0 - f.y)),
            sum(1, 0, f.x * (1.0 - f.y)),
            sum(0, 1, (1.0 - f.x) * f.y),
            sum(1, 1, f.x * f.y),
        ];
        let [humidity, cover, precipitation] =
            corners.iter().fold([0.0; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);

        let block = at.floor().as_ivec2();
        let celsius = temperature.sample(block.x, block.y);
        let temperature = warmth(celsius);
        let tick = tick as i64;
        let drivers = self.drivers(at.as_dvec2(), tick, temperature);
        // Updrafts heap cloud into towers; without them it spreads into flat layers.
        let convection = smoothstep(0.05, 0.5, drivers.lift + 0.25 * smoothstep(0.45, 0.8, temperature) * (1.0 - cover));
        // High ice cloud runs ahead of moist air, and drifts across fair skies now and then.
        let seconds = tick as f64 / TICKS_PER_SECOND as f64;
        let upwind = self.drivers(at.as_dvec2() - self.prevailing * 2500.0, tick, temperature);
        let fair = self.noise(CIRRUS, at.as_dvec2() / 2000.0, seconds / 1200.0) as f32;
        let cirrus = (0.8 * smoothstep(0.15, 0.7, upwind.air - drivers.air)).max(0.5 * smoothstep(0.6, 0.85, fair)) * (1.0 - cover);

        // Wind: the prevailing direction, veering and strengthening over hours, gusting in
        // showers.
        let gusts = self.noise(GUSTS, DVec2::ZERO, seconds / 1500.0);
        let veer = self.noise(GUSTS + 16, DVec2::ZERO, seconds / 900.0) - 0.5;
        let direction = (self.prevailing + self.prevailing.perp() * veer * 1.2).normalize();
        let speed = 1.5 + 7.0 * gusts as f32 + 6.0 * precipitation * convection;

        LocalWeather {
            cover,
            convection,
            cirrus,
            humidity,
            precipitation,
            snow: 1.0 - smoothstep(-1.5, 1.5, celsius),
            wind: direction.as_vec2() * speed,
        }
    }

    /// Smooth random values in 0..1 over a position and time, from hashed values on a
    /// lattice of spacing 1, blended with smoothstep weights.
    fn noise(&self, stream: u64, p: DVec2, t: f64) -> f64 {
        let seed = mix(self.seed ^ stream);
        let (i, f) = (p.floor(), p - p.floor());
        let (it, ft) = (t.floor(), t - t.floor());
        let (ix, iz, it) = (i.x as i64, i.y as i64, it as i64);
        let fade = |t: f64| t * t * (3.0 - 2.0 * t);
        let (wx, wz, wt) = (fade(f.x), fade(f.y), fade(ft));
        let corner = |dx: i64, dz: i64, dt: i64| unit(mix(mix(mix(seed ^ (ix + dx) as u64) ^ (iz + dz) as u64) ^ (it + dt) as u64));
        let plane = |dt| {
            let near = corner(0, 0, dt) + (corner(1, 0, dt) - corner(0, 0, dt)) * wx;
            let far = corner(0, 1, dt) + (corner(1, 1, dt) - corner(0, 1, dt)) * wx;
            near + (far - near) * wz
        };
        plane(0) + (plane(1) - plane(0)) * wt
    }
}

/// The air's usual relative humidity where `rainfall` mm of rain or snow fall a year.
fn usual_humidity(rainfall: f32) -> f32 {
    rainfall.max(0.0) / (rainfall.max(0.0) + 1200.0)
}

/// How warm a mean temperature is for the weather, 0 (−10 °C and colder) to 1 (30 °C and
/// hotter).
fn warmth(celsius: f32) -> f32 {
    ((celsius + 10.0) / 40.0).clamp(0.0, 1.0)
}

/// One step of `dt` seconds of a cell's response to its drivers. `climate` is the usual
/// humidity and `temperature` the warmth (see `warmth`).
fn step(cell: Cell, drivers: &Drivers, climate: f32, temperature: f32, dt: f32) -> Cell {
    let rate = |seconds: f32| (dt / seconds).min(1.0);
    // Humidity follows the air mass, away from the climate's usual humidity.
    let target = (0.18 + 0.75 * climate + 0.42 * drivers.air).clamp(0.0, 1.0);
    let mut humidity = cell.humidity + (target - cell.humidity) * rate(150.0);
    // Cloud forms as the air nears saturation, sooner in rising air and in cold air,
    // which holds less water, and evaporates again as it dries.
    let saturation = 0.78 - 0.08 * (1.0 - temperature) - 0.2 * drivers.lift;
    let cloud_target = smoothstep(saturation - 0.4, saturation + 0.08, humidity);
    let clouds = cell.clouds + (cloud_target - cell.clouds) * rate(90.0);
    // Thick cloud rains, harder from strong updrafts or very moist air; rain dries the
    // air, which thins the cloud until the air mass brings more moisture.
    let wetness = smoothstep(saturation, saturation + 0.25, humidity).max(drivers.lift);
    let rain_target = smoothstep(0.8, 0.97, clouds) * (0.3 + 0.7 * wetness);
    let precipitation = cell.precipitation + (rain_target - cell.precipitation) * rate(30.0);
    humidity -= precipitation * 0.0012 * dt;
    Cell {
        humidity: humidity.clamp(0.0, 1.0),
        clouds: clouds.clamp(0.0, 1.0),
        precipitation: precipitation.clamp(0.0, 1.0),
    }
}

fn changed(a: &LocalWeather, b: &LocalWeather) -> bool {
    let values = |w: &LocalWeather| [w.cover, w.convection, w.cirrus, w.humidity, w.precipitation, w.snow, w.wind.x / 10.0, w.wind.y / 10.0];
    values(a).iter().zip(values(b)).any(|(x, y)| (x - y).abs() > CHANGE)
}

/// SplitMix64 finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Uniform in 0..1 from a hash.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = TICKS_PER_SECOND as u64;

    fn setup(rainfall: f32, celsius: f32) -> (WeatherFields, Field) {
        (WeatherFields::new(move |_, _| rainfall), Field::new("core:temperature", move |_, _| celsius))
    }

    /// Runs the process for a player walking east from `from`, returning what they were
    /// told each second.
    fn walk(seed: u64, rainfall: f32, celsius: f32, from: Vec2, seconds: u64) -> Vec<LocalWeather> {
        let mut weather = Weather::new(seed, Vec2::new(0.6, 0.8));
        let (mut fields, mut temperature) = setup(rainfall, celsius);
        (0..seconds)
            .map(|s| {
                let at = from + Vec2::X * 4.3 * s as f32;
                let tick = 50_000 + s * SECOND;
                weather.update(&mut fields, &mut temperature, &[at.floor().as_ivec2()], tick);
                weather.local(&mut fields, &mut temperature, at, tick)
            })
            .collect()
    }

    #[test]
    fn deterministic_per_seed() {
        let from = Vec2::new(-300.0, 1234.0);
        assert_eq!(walk(7, 1500.0, 15.0, from, 600), walk(7, 1500.0, 15.0, from, 600));
        assert_ne!(walk(7, 1500.0, 15.0, from, 600), walk(8, 1500.0, 15.0, from, 600));
    }

    #[test]
    fn values_stay_in_bounds() {
        for (rainfall, celsius) in [(0.0, -30.0), (6000.0, 40.0), (6000.0, -30.0), (0.0, 40.0), (900.0, 10.0)] {
            for weather in walk(3, rainfall, celsius, Vec2::new(5000.0, -5000.0), 1800) {
                let values = [weather.cover, weather.convection, weather.cirrus, weather.humidity, weather.precipitation, weather.snow];
                assert!(values.iter().all(|v| (0.0..=1.0).contains(v)), "{weather:?}");
                assert!(weather.wind.is_finite() && weather.wind.length() < 20.0, "{weather:?}");
            }
        }
    }

    #[test]
    fn snow_only_where_freezing() {
        assert!(walk(1, 800.0, -8.0, Vec2::ZERO, 10).iter().all(|w| w.snow == 1.0));
        assert!(walk(1, 800.0, 12.0, Vec2::ZERO, 10).iter().all(|w| w.snow == 0.0));
    }

    /// Fraction of two days that it rains, over several places, for a climate.
    fn rainy_fraction(rainfall: f32, celsius: f32) -> f32 {
        let weather = Weather::new(11, Vec2::new(-1.0, 0.0));
        let climate = usual_humidity(rainfall);
        let (mut rainy, mut steps) = (0, 0);
        for place in 0..6 {
            let at = IVec2::new(place * 20_000, place * -13_000);
            let mut cell = Cell { humidity: climate, clouds: 0.0, precipitation: 0.0 };
            for step in 0..2 * 24 * 3600 / 5 {
                let drivers = weather.drivers(at.as_dvec2(), step * 5 * SECOND as i64, warmth(celsius));
                cell = super::step(cell, &drivers, climate, warmth(celsius), 5.0);
                rainy += (cell.precipitation > 0.1) as u32;
                steps += 1;
            }
        }
        rainy as f32 / steps as f32
    }

    #[test]
    fn rain_follows_the_climate() {
        let desert = rainy_fraction(150.0, 28.0);
        let temperate = rainy_fraction(900.0, 12.0);
        let rainforest = rainy_fraction(3000.0, 26.0);
        assert!(desert < 0.01, "desert rains {desert}");
        assert!((0.03..0.25).contains(&temperate), "temperate rains {temperate}");
        assert!(rainforest > 0.25, "rainforest rains {rainforest}");
    }

    #[test]
    fn weather_does_not_depend_on_when_players_arrived() {
        let wind = Vec2::new(0.0, 1.0);
        let mut weather = Weather::new(5, wind);
        let (mut fields, mut temperature) = setup(2000.0, 20.0);
        let (here, elsewhere) = (IVec2::new(100, 100), IVec2::new(90_000, 0));
        let start = 80_000;
        for s in 0..1200 {
            weather.update(&mut fields, &mut temperature, &[here], start + s * SECOND);
        }
        let stayed = weather.local(&mut fields, &mut temperature, here.as_vec2(), start + 1199 * SECOND);

        let mut weather = Weather::new(5, wind);
        let (mut fields, mut temperature) = setup(2000.0, 20.0);
        weather.update(&mut fields, &mut temperature, &[elsewhere], start + 1198 * SECOND);
        weather.update(&mut fields, &mut temperature, &[here], start + 1199 * SECOND);
        let arrived = weather.local(&mut fields, &mut temperature, here.as_vec2(), start + 1199 * SECOND);
        assert!(!changed(&stayed, &arrived), "{stayed:?} != {arrived:?}");
        // The cells left behind are back to their climate.
        assert_eq!(fields.clouds.deltas().filter(|&(cell, _)| cell.x > 100).count(), 0);
    }

    #[test]
    fn lightning_only_in_thunderstorms() {
        let weather = Weather::new(9, Vec2::X);
        let flashes = |rain: f32, lift: f32| {
            (0..20_000u64).filter_map(|s| weather.lightning(IVec2::new(3, -4), rain, lift, s * SECOND)).collect::<Vec<_>>()
        };
        assert!(flashes(0.0, 1.0).is_empty() && flashes(1.0, 0.0).is_empty() && flashes(0.2, 0.5).is_empty());
        let storm = flashes(1.0, 1.0);
        assert!((100..500).contains(&storm.len()), "{} flashes", storm.len());
        assert!(storm.iter().all(|at| (384.0..512.0).contains(&at.x) && (-512.0..-384.0).contains(&at.y)));
        assert_eq!(storm, flashes(1.0, 1.0));
    }
}
