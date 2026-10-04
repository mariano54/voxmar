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
//! the air again; a downpour in strong updrafts is a thunderstorm and raises lightning
//! events. A cell coming into range first catches up on the last ten minutes, so the
//! weather at a place does not depend on when anyone arrived, and cells left behind return
//! to their climate baseline.
//!
//! Only exact floating point operations are used (no `sin`, `exp` or `powf`), so every
//! machine computes the same weather.

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::field::Field;
use voxmar_engine::glam::{DVec2, IVec2, Vec2};
use voxmar_engine::schedule::Tier;
use voxmar_engine::weather::Preset;
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
/// Forced weather covers this many blocks around where it was asked for, then fades out
/// over `FORCE_EDGE` more; when its time is up it fades over `FORCE_FADE` seconds.
const FORCE_RADIUS: f32 = 1024.0;
const FORCE_EDGE: f32 = 384.0;
const FORCE_FADE: u64 = 60;
/// For this many seconds after a command changes the weather, players there are told at
/// once and change over quickly.
const QUICK: u64 = 3;

/// Random streams, each mixed into the world seed.
const AIR_MASS: u64 = 1;
const UPDRAFT: u64 = 2;
const GUSTS: u64 = 3;
const CIRRUS: u64 = 4;
const STRIKES: u64 = 5;

/// The weather's state: `core:humidity` (relative), `core:clouds` (cloud cover) and
/// `core:precipitation` (rain or snow reaching the ground), all 0..1, what drives them,
/// and the weather admins have forced.
pub(crate) struct WeatherState {
    pub humidity: Field,
    pub clouds: Field,
    pub precipitation: Field,
    sky: Sky,
    /// Cells simulated in the last update.
    active: FxHashSet<IVec2>,
    forced: Vec<Forcing>,
    /// Where commands changed the weather lately.
    changes: Vec<Change>,
}

/// A preset held over the land around `center` until `until` (a simulation tick).
struct Forcing {
    preset: Preset,
    center: Vec2,
    until: u64,
}

impl Forcing {
    /// How strongly the preset holds at `at`, 0..1.
    fn weight(&self, at: Vec2, tick: u64) -> f32 {
        let space = 1.0 - smoothstep(FORCE_RADIUS, FORCE_RADIUS + FORCE_EDGE, self.center.distance(at));
        let over = tick.saturating_sub(self.until) as f32 / (FORCE_FADE * TICKS_PER_SECOND as u64) as f32;
        space * (1.0 - over).max(0.0)
    }
}

/// A command changed the weather around `center` at `tick`; `release` gave it back to
/// nature at once.
struct Change {
    center: Vec2,
    tick: u64,
    release: bool,
}

impl Change {
    fn covers(&self, at: Vec2) -> bool {
        self.center.distance(at) < FORCE_RADIUS + FORCE_EDGE
    }
}

impl WeatherState {
    /// `wind` is the direction (x, z) the world's prevailing wind blows towards.
    /// `rainfall(x, z)` is the climate's annual precipitation in mm; humidity's baseline,
    /// the air's usual humidity, follows it, so wet climates rain often and dry ones rarely.
    pub fn new(world_seed: u64, wind: Vec2, rainfall: impl Fn(i32, i32) -> f32 + Send + Sync + 'static) -> Self {
        Self {
            humidity: Field::with_cell_size("core:humidity", CELL, move |x, z| usual_humidity(rainfall(x, z))),
            clouds: Field::with_cell_size("core:clouds", CELL, |_, _| 0.0),
            precipitation: Field::with_cell_size("core:precipitation", CELL, |_, _| 0.0),
            sky: Sky {
                seed: mix(world_seed ^ 0x0077_6561_7468_6572), // "weather"
                prevailing: wind.as_dvec2().normalize_or(DVec2::X),
            },
            active: FxHashSet::default(),
            forced: Vec::new(),
            changes: Vec::new(),
        }
    }

    /// Holds `preset` over the land around `center` for `seconds`, replacing weather forced
    /// nearby before.
    pub fn force(&mut self, preset: Preset, center: Vec2, seconds: u32, tick: u64) {
        self.forced.retain(|f| f.center.distance(center) > FORCE_RADIUS);
        let until = tick + seconds as u64 * TICKS_PER_SECOND as u64;
        self.forced.push(Forcing { preset, center, until });
        self.changes.push(Change { center, tick, release: false });
    }

    /// Ends the weather forced around `center`, bringing back the natural weather at once.
    /// Returns whether any was.
    pub fn release(&mut self, center: Vec2, tick: u64) -> bool {
        let before = self.forced.len();
        self.forced.retain(|f| f.weight(center, tick) == 0.0);
        self.changes.push(Change { center, tick, release: true });
        self.forced.len() < before
    }

    /// The preset forced at `at`, and the seconds it has left, if one holds there.
    pub fn forced_at(&self, at: Vec2, tick: u64) -> Option<(&Preset, u64)> {
        let forcing = self.forced.iter().rev().find(|f| f.weight(at, tick) > 0.0)?;
        Some((&forcing.preset, forcing.until.saturating_sub(tick) / TICKS_PER_SECOND as u64))
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

    /// The strongest forced preset at `at`, and how strongly it holds.
    fn forcing(&self, at: Vec2, tick: u64) -> Option<(&Preset, f32)> {
        self.forced
            .iter()
            .map(|f| (&f.preset, f.weight(at, tick)))
            .filter(|&(_, weight)| weight > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Whether a command changed the weather at `at` in the last few seconds.
    fn recently_changed(&self, at: Vec2) -> bool {
        self.changes.iter().any(|change| change.covers(at))
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

/// What drives the weather: pure functions of the world seed, place and time.
struct Sky {
    seed: u64,
    /// Direction of the prevailing wind, fixed per world.
    prevailing: DVec2,
}

/// The weather process.
#[derive(Default)]
pub(crate) struct Weather {
    /// The weather each player was last sent, and when.
    sent: FxHashMap<PlayerId, (LocalWeather, u64)>,
}

impl Process for Weather {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let (tick, day) = (server.tick, server.day);
        let positions: Vec<(PlayerId, Vec2)> =
            server.players.values().filter(|p| p.joined).map(|p| (p.id, Vec2::new(p.position.x, p.position.z))).collect();
        let around: Vec<IVec2> = positions.iter().map(|(_, at)| at.floor().as_ivec2()).collect();
        let strikes = server.weather.update(&mut server.temperature, &around, tick, day);
        for at in strikes {
            for (id, player) in &positions {
                if player.distance(at) < LIGHTNING_REACH {
                    server.players[id].connection.send(ServerMessage::Lightning { at });
                }
            }
        }

        self.sent.retain(|id, _| server.players.contains_key(id));
        for (id, at) in positions {
            let weather = server.weather.local(&mut server.temperature, at, tick, day);
            let quick = server.weather.recently_changed(at);
            let due = quick
                || self.sent.get(&id).is_none_or(|(last, sent_at)| tick >= sent_at + RESEND || changed(last, &weather));
            if due {
                self.sent.insert(id, (weather, tick));
                server.players[&id].connection.send(ServerMessage::Weather { weather, quick });
            }
        }
    }
}

impl WeatherState {
    /// Advances the cells within range of each block position in `around` to `tick`, and
    /// returns cells no longer in range to their baseline. `day` is the time of day, in
    /// ticks. Returns where lightning flashed.
    fn update(&mut self, temperature: &mut Field, around: &[IVec2], tick: u64, day: f64) -> Vec<Vec2> {
        // Cells a command released go back to what nature would have made of them.
        let quick = QUICK * TICKS_PER_SECOND as u64;
        let released: Vec<Vec2> = self.changes.iter().filter(|c| c.release && c.tick + quick > tick).map(|c| c.center).collect();
        self.changes.retain(|c| c.tick + quick > tick && !c.release);
        self.active.retain(|cell| released.iter().all(|&center| !Change { center, tick, release: true }.covers(cell_center(*cell).as_vec2())));
        self.forced.retain(|f| tick < f.until + FORCE_FADE * TICKS_PER_SECOND as u64);

        let mut cells = FxHashSet::default();
        for at in around {
            let center = at.div_euclid(IVec2::splat(CELL));
            for dz in -RANGE..=RANGE {
                for dx in -RANGE..=RANGE {
                    cells.insert(center + IVec2::new(dx, dz));
                }
            }
        }
        let gone: Vec<IVec2> = self.active.difference(&cells).copied().collect();
        for cell in gone {
            self.reset(cell * CELL);
        }
        let dt = Tier::Low.period() as f32 / TICKS_PER_SECOND as f32;
        let day_at = |t: i64| day_fraction(day - (tick as i64 - t) as f64);
        let mut strikes = Vec::new();
        for &cell in &cells {
            let at = cell_center(cell);
            let climate = self.humidity.baseline(at.x, at.y);
            let warmth = warmth(temperature.sample(at.x, at.y));
            let mut state = if self.active.contains(&cell) {
                let drivers = self.sky.drivers(at.as_dvec2(), tick as i64, day_at(tick as i64), warmth);
                let state = step(self.get(at), &drivers, climate, warmth, dt);
                strikes.extend(self.sky.lightning(cell, state.precipitation, drivers.lift, tick));
                state
            } else {
                // Catch up from the climate's average state.
                let mut state = Cell { humidity: climate, clouds: 0.0, precipitation: 0.0 };
                let start = tick as i64 - CATCH_UP * TICKS_PER_SECOND as i64;
                for i in 1..=CATCH_UP / CATCH_UP_STEP {
                    let t = start + i * CATCH_UP_STEP * TICKS_PER_SECOND as i64;
                    let drivers = self.sky.drivers(at.as_dvec2(), t, day_at(t), warmth);
                    state = step(state, &drivers, climate, warmth, CATCH_UP_STEP as f32);
                }
                state
            };
            // Forced weather holds the cell at the preset.
            if let Some((preset, weight)) = self.forcing(at.as_vec2(), tick) {
                let mix = |value: f32, forced: Option<f32>| forced.map_or(value, |f| value + (f - value) * weight);
                state = Cell {
                    humidity: mix(state.humidity, preset.humidity),
                    clouds: mix(state.clouds, preset.cover),
                    precipitation: mix(state.precipitation, preset.precipitation),
                };
            }
            self.set(at, state);
        }
        self.active = cells;
        strikes
    }

    /// The weather at a block position at `tick` and time of day `day` (in ticks): the
    /// fields interpolated between the four nearest cell centres, the kind of cloud from
    /// the drivers, snow where it is freezing, and any forced weather over that.
    pub fn local(&mut self, temperature: &mut Field, at: Vec2, tick: u64, day: f64) -> LocalWeather {
        let corner = ((at - CELL as f32 / 2.0) / CELL as f32).floor();
        let f = (at - CELL as f32 / 2.0) / CELL as f32 - corner;
        let corner = corner.as_ivec2() * CELL + CELL / 2;
        let mut sum = |dx: i32, dz: i32, weight: f32| {
            let cell = self.get(corner + IVec2::new(dx, dz) * CELL);
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
        let warm = warmth(celsius);
        let sky = &self.sky;
        let (tick, day) = (tick as i64, day_fraction(day));
        let drivers = sky.drivers(at.as_dvec2(), tick, day, warm);
        // Updrafts heap cloud into towers; without them it spreads into flat layers.
        let convection = smoothstep(0.05, 0.5, drivers.lift + 0.25 * smoothstep(0.45, 0.8, warm) * (1.0 - cover));
        // High ice cloud runs ahead of moist air, and drifts across fair skies now and then.
        let seconds = tick as f64 / TICKS_PER_SECOND as f64;
        let upwind = sky.drivers(at.as_dvec2() - sky.prevailing * 2500.0, tick, day, warm);
        let fair = sky.noise(CIRRUS, at.as_dvec2() / 2000.0, seconds / 1200.0) as f32;
        let ahead = 0.8 * smoothstep(0.15, 0.7, upwind.air - drivers.air);
        let cirrus = ahead.max(0.5 * smoothstep(0.6, 0.85, fair)) * (1.0 - cover);

        // Wind: the prevailing direction, veering and strengthening over hours, gusting in
        // showers.
        let gusts = sky.noise(GUSTS, DVec2::ZERO, seconds / 1500.0);
        let veer = sky.noise(GUSTS + 16, DVec2::ZERO, seconds / 900.0) - 0.5;
        let direction = (sky.prevailing + sky.prevailing.perp() * veer * 1.2).normalize();
        let speed = 1.5 + 7.0 * gusts as f32 + 6.0 * precipitation * convection;

        let natural = LocalWeather {
            cover,
            convection,
            cirrus,
            humidity,
            precipitation,
            snow: 1.0 - smoothstep(-1.5, 1.5, celsius),
            wind: direction.as_vec2() * speed,
        };
        match self.forcing(at, tick as u64) {
            Some((preset, weight)) => preset.blend(&natural, weight),
            None => natural,
        }
    }
}

impl Sky {
    /// Where lightning flashes in a cell this second, if it does: a downpour under strong
    /// updrafts is a thunderstorm, which has a chance each second to flash somewhere in
    /// the cell, drawn from the cell's own random stream.
    fn lightning(&self, cell: IVec2, rain: f32, lift: f32, tick: u64) -> Option<Vec2> {
        let chance = LIGHTNING * smoothstep(0.25, 0.8, rain * smoothstep(0.1, 0.6, lift));
        let h = mix(mix(mix(self.seed ^ STRIKES ^ tick) ^ cell.x as u64) ^ cell.y as u64);
        let place = Vec2::new(unit(mix(h)) as f32, unit(mix(mix(h))) as f32);
        (unit(h) < chance as f64).then(|| (cell.as_vec2() + place) * CELL as f32)
    }

    /// The air mass and lift at a block position and tick. `day` is the time of day as a
    /// fraction from sunrise, and `temperature` the warmth (see `warmth`); together they
    /// set how readily the ground heats air into updrafts.
    fn drivers(&self, at: DVec2, tick: i64, day: f64, temperature: f32) -> Drivers {
        let seconds = tick as f64 / TICKS_PER_SECOND as f64;
        let p = at - self.prevailing * (MEAN_WIND * seconds);
        let air = 0.65 * self.noise(AIR_MASS, p / 3000.0, seconds / 2400.0)
            + 0.35 * self.noise(AIR_MASS + 16, p / 1100.0, seconds / 1000.0);
        let updraft = smoothstep(0.5, 0.75, self.noise(UPDRAFT, p / 700.0, seconds / 600.0) as f32);
        // Sun-warmed ground lifts air most in the afternoon, and hardly at all when cold.
        let from_peak = (day - 0.32) / 0.24;
        let afternoon = (1.0 - from_peak * from_peak).max(0.0) as f32;
        let heating = smoothstep(0.2, 0.7, temperature) * (0.35 + 0.65 * afternoon);
        Drivers { air: ((air - 0.5) * 3.2) as f32, lift: updraft * heating }
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
        let corner =
            |dx: i64, dz: i64, dt: i64| unit(mix(mix(mix(seed ^ (ix + dx) as u64) ^ (iz + dz) as u64) ^ (it + dt) as u64));
        let plane = |dt| {
            let near = corner(0, 0, dt) + (corner(1, 0, dt) - corner(0, 0, dt)) * wx;
            let far = corner(0, 1, dt) + (corner(1, 1, dt) - corner(0, 1, dt)) * wx;
            near + (far - near) * wz
        };
        plane(0) + (plane(1) - plane(0)) * wt
    }
}

/// The centre of a weather cell, in blocks.
fn cell_center(cell: IVec2) -> IVec2 {
    cell * CELL + CELL / 2
}

/// A time of day in ticks as a fraction of the day from sunrise.
fn day_fraction(day: f64) -> f64 {
    (day / TICKS_PER_DAY as f64).rem_euclid(1.0)
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

/// Whether a player told `a` should now be told `b` (wind counts in tens of blocks per
/// second).
fn changed(a: &LocalWeather, b: &LocalWeather) -> bool {
    let values = |w: &LocalWeather| {
        [w.cover, w.convection, w.cirrus, w.humidity, w.precipitation, w.snow, w.wind.x / 10.0, w.wind.y / 10.0]
    };
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

    fn setup(seed: u64, wind: Vec2, rainfall: f32, celsius: f32) -> (WeatherState, Field) {
        (WeatherState::new(seed, wind, move |_, _| rainfall), Field::new("core:temperature", move |_, _| celsius))
    }

    /// Runs the weather for a player walking east from `from`, returning what they were
    /// told each second.
    fn walk(seed: u64, rainfall: f32, celsius: f32, from: Vec2, seconds: u64) -> Vec<LocalWeather> {
        let (mut weather, mut temperature) = setup(seed, Vec2::new(0.6, 0.8), rainfall, celsius);
        (0..seconds)
            .map(|s| {
                let at = from + Vec2::X * 4.3 * s as f32;
                let tick = 50_000 + s * SECOND;
                weather.update(&mut temperature, &[at.floor().as_ivec2()], tick, tick as f64);
                weather.local(&mut temperature, at, tick, tick as f64)
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
        let (weather, _) = setup(11, Vec2::new(-1.0, 0.0), rainfall, celsius);
        let climate = usual_humidity(rainfall);
        let (mut rainy, mut steps) = (0, 0);
        for place in 0..6 {
            let at = IVec2::new(place * 20_000, place * -13_000);
            let mut cell = Cell { humidity: climate, clouds: 0.0, precipitation: 0.0 };
            for step in 0..2 * 24 * 3600 / 5 {
                let tick = step * 5 * SECOND as i64;
                let drivers = weather.sky.drivers(at.as_dvec2(), tick, day_fraction(tick as f64), warmth(celsius));
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
        let (mut weather, mut temperature) = setup(5, wind, 2000.0, 20.0);
        let (here, elsewhere) = (IVec2::new(100, 100), IVec2::new(90_000, 0));
        let start = 80_000;
        let at = |s: u64| (start + s * SECOND, (start + s * SECOND) as f64);
        for s in 0..1200 {
            weather.update(&mut temperature, &[here], at(s).0, at(s).1);
        }
        let stayed = weather.local(&mut temperature, here.as_vec2(), at(1199).0, at(1199).1);

        let (mut weather, mut temperature) = setup(5, wind, 2000.0, 20.0);
        weather.update(&mut temperature, &[elsewhere], at(1198).0, at(1198).1);
        weather.update(&mut temperature, &[here], at(1199).0, at(1199).1);
        let arrived = weather.local(&mut temperature, here.as_vec2(), at(1199).0, at(1199).1);
        assert!(!changed(&stayed, &arrived), "{stayed:?} != {arrived:?}");
        // The cells left behind are back to their climate.
        assert_eq!(weather.clouds.deltas().filter(|&(cell, _)| cell.x > 100).count(), 0);
    }

    #[test]
    fn lightning_only_in_thunderstorms() {
        let (weather, _) = setup(9, Vec2::X, 1000.0, 10.0);
        let flashes = |rain: f32, lift: f32| {
            (0..20_000u64).filter_map(|s| weather.sky.lightning(IVec2::new(3, -4), rain, lift, s * SECOND)).collect::<Vec<_>>()
        };
        assert!(flashes(0.0, 1.0).is_empty() && flashes(1.0, 0.0).is_empty() && flashes(0.2, 0.5).is_empty());
        let storm = flashes(1.0, 1.0);
        assert!((100..500).contains(&storm.len()), "{} flashes", storm.len());
        assert!(storm.iter().all(|at| (384.0..512.0).contains(&at.x) && (-512.0..-384.0).contains(&at.y)));
        assert_eq!(storm, flashes(1.0, 1.0));
    }

    #[test]
    fn forced_weather_holds_then_eases_back() {
        let storm = Preset {
            name: "base:storm".into(),
            cover: Some(1.0),
            convection: Some(1.0),
            humidity: Some(0.9),
            precipitation: Some(1.0),
            wind: Some(10.0),
            ..Default::default()
        };
        // A desert, where it would never rain.
        let (mut weather, mut temperature) = setup(4, Vec2::X, 100.0, 25.0);
        let (here, far) = (Vec2::new(10.0, 20.0), Vec2::new(10.0, 3000.0));
        let report = |weather: &mut WeatherState, temperature: &mut Field, s: u64| {
            let tick = 10_000 + s * SECOND;
            weather.update(temperature, &[here.as_ivec2()], tick, tick as f64);
            weather.local(temperature, here, tick, tick as f64)
        };
        assert_eq!(report(&mut weather, &mut temperature, 0).precipitation, 0.0);
        weather.force(storm, here, 60, 10_000 + SECOND);
        assert!(weather.recently_changed(here) && !weather.recently_changed(far));
        let held = report(&mut weather, &mut temperature, 2);
        assert_eq!((held.cover, held.precipitation, held.convection), (1.0, 1.0, 1.0), "{held:?}");
        assert!((held.wind.length() - 10.0).abs() < 1e-3);
        assert_eq!(weather.forced_at(here, 10_000 + 2 * SECOND).map(|(p, left)| (p.short_name(), left)), Some(("storm", 59)));
        assert!(weather.forced_at(far, 10_000).is_none());
        // Still forced a minute in, then fading, then released to slowly dry out.
        let reports: Vec<LocalWeather> = (3..400).map(|s| report(&mut weather, &mut temperature, s)).collect();
        assert_eq!(reports[55].precipitation, 1.0);
        assert!(reports[70].precipitation < 1.0 && reports[70].precipitation > 0.3, "{:?}", reports[70]);
        assert!(reports.last().unwrap().precipitation < 0.05, "{:?}", reports.last());
        assert!(weather.forced.is_empty());

        // Released at once by `/weather natural`.
        weather.force(Preset { name: "base:rain".into(), cover: Some(1.0), precipitation: Some(0.5), ..Default::default() }, here, 600, 10_000 + 400 * SECOND);
        assert_eq!(report(&mut weather, &mut temperature, 401).precipitation, 0.5);
        assert!(weather.release(here, 10_000 + 402 * SECOND));
        assert!(report(&mut weather, &mut temperature, 403).precipitation < 0.01);
        assert!(!weather.release(here, 10_000 + 404 * SECOND));
    }
}
