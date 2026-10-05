//! Weather: a Low-tier process that evolves humidity, cloud, precipitation, ground
//! wetness and lying snow near players and tells each player what the weather is doing
//! where they stand.
//!
//! What drives the weather is a pure function of the world seed, place and time: moist and
//! dry air masses a few kilometres across that drift with the prevailing wind and slowly
//! change shape, and smaller, shorter-lived updrafts that are strongest on warm
//! afternoons. Drivers need no state, so the weather exists everywhere without being
//! simulated everywhere. Near players the process integrates how each cell responds:
//! humidity follows the passing air mass from the climate's baseline, cloud condenses as
//! the air nears saturation and evaporates as it dries, and thick cloud rains, which dries
//! the air again; a downpour in strong updrafts is a thunderstorm and raises lightning
//! events. Rain wets the ground, which dries over minutes, and snow lies where it is cold
//! enough and melts when it warms. A cell coming into range first catches up on the last
//! ten minutes, so the weather at a place does not depend on when anyone arrived, and
//! cells left behind return to their climate baseline.
//!
//! On top of that come the effects in each package's `weather.ron`: valley fog at dawn,
//! hail, sandstorms, rainbows, the aurora. Each is a product of smooth conditions on the
//! weather, the climate and land where the player stands and the time of day, gated by
//! its rarity: for each stretch of land and time a hash of the seed decides whether a
//! rare effect may occur at all, so rare ones are rare even where their conditions often
//! hold, and every world features a few from its seed.
//!
//! An admin can force a kind of weather (`/weather storm 0.6`) on the land around them for
//! a while, at an intensity: the cells there are held at the preset, its effects are held
//! as strongly as the intensity and its values are blended over what the drivers would
//! make. When it ends the cells are released from that state and the natural weather
//! slowly takes over again.
//!
//! The simulation runs on simulation ticks, so it keeps going when the time of day is
//! frozen or sped up; only what follows the sun (afternoon showers, dawn fog, night-time
//! cold, the aurora) reads the time of day.
//!
//! Only exact floating point operations are used (no `sin`, `exp` or `powf`), so every
//! machine computes the same weather.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, Tint};
use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ColumnPos};
use voxmar_engine::world::Column;
use voxmar_engine::field::Field;
use voxmar_engine::glam::{DVec2, IVec2, IVec3, Vec2, Vec3};
use voxmar_engine::schedule::Tier;
use voxmar_engine::weather::{Input, Inputs, Preset, WeatherTable, advance_gusts, ramp};
use voxmar_worldgen::{Featured, Overworld, SEA_LEVEL, Tier as _};
use voxmar_protocol::{Nearby, PlayerId, ServerMessage, THUNDER_HEARD, TICKS_PER_DAY, TICKS_PER_SECOND, WEATHER_CELL, Weather as LocalWeather};

use crate::Server;
use crate::process::Process;

/// Blocks per weather cell.
const CELL: i32 = WEATHER_CELL;
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
/// Chance per second that a cell under the fiercest storm strikes the ground (it flashes
/// within the cloud twice as often), and how far away, in blocks, players see and hear
/// lightning. A player in the heart of a storm has the 25 cells around them flashing a few
/// times a minute between them, a ground strike every half minute or so.
const LIGHTNING: f32 = 0.0009;
const LIGHTNING_REACH: f32 = 1500.0;
/// Blocks above the ground (or the sea) a flash within a cloud lights it.
const CLOUD_FLASH_HEIGHT: i32 = 160;
/// Strikes per second near a player at full `lightning` from effects.
const EFFECT_LIGHTNING: f32 = 1.0 / 60.0;
/// Forced weather covers this many blocks around where it was asked for, then fades out
/// over `FORCE_EDGE` more; when its time is up it fades over `FORCE_FADE` seconds.
const FORCE_RADIUS: f32 = 1024.0;
const FORCE_EDGE: f32 = 384.0;
const FORCE_FADE: u64 = 60;
/// For this many seconds after a command changes the weather, players there are told at
/// once and change over quickly.
const QUICK: u64 = 3;
/// Blocks across the stretches of land an effect's rarity is drawn for.
const REGION: f64 = 1500.0;
/// Effects at least this strong are named to players and in `/weather`.
const NAMED: f32 = 0.3;
/// Seconds for wet ground to lose half its water, in mild, cloudy weather.
const DRYING: f32 = 300.0;
/// Places whose land is remembered, at 8-block resolution.
const PLACES: usize = 4096;
/// Columns whose lying snow is brought up to date per player each second, and blocks
/// changed in one column at most.
const SNOW_COLUMNS: usize = 2;
const SNOW_CHANGES: usize = 256;

/// Random streams, each mixed into the world seed.
const AIR_MASS: u64 = 1;
const UPDRAFT: u64 = 2;
const GUSTS: u64 = 3;
const CIRRUS: u64 = 4;
const STRIKES: u64 = 5;
const EFFECT_STRIKES: u64 = 6;
const OCCURRENCE: u64 = 7;
const SNOWFLAKES: u64 = 8;

/// What the weather needs to know about the land, from the world generator.
pub(crate) trait Land: Send + Sync {
    /// The climate's mean annual temperature (°C) and precipitation (mm). Sampled once
    /// per weather cell.
    fn climate(&self, x: i32, z: i32) -> (f32, f32);
    /// Everything effects read about one place. Sampled where players stand.
    fn place(&self, x: i32, z: i32) -> Place;
    /// Height of the top ground block.
    fn height(&self, x: i32, z: i32) -> i32;
}

/// The overworld generator as the weather sees it.
pub(crate) struct OverworldLand {
    pub overworld: Arc<Overworld>,
    pub blocks: Arc<BlockRegistry>,
}

impl Land for OverworldLand {
    fn climate(&self, x: i32, z: i32) -> (f32, f32) {
        let site = self.overworld.site(x, z);
        (site.temperature, site.precipitation)
    }

    fn place(&self, x: i32, z: i32) -> Place {
        let site = self.overworld.site(x, z);
        // The land around, on rings 48 and 112 blocks out.
        const RING: [(f32, f32); 8] =
            [(1.0, 0.0), (0.71, 0.71), (0.0, 1.0), (-0.71, 0.71), (-1.0, 0.0), (-0.71, -0.71), (0.0, -1.0), (0.71, -0.71)];
        let ring = |radius: f32| RING.map(|(dx, dz)| self.height(x + (dx * radius) as i32, z + (dz * radius) as i32));
        let (near, far) = (ring(48.0), ring(112.0));
        let around = near.iter().chain(&far).sum::<i32>() as f32 / 16.0;
        let lowest = near.iter().copied().fold(site.height, i32::min);
        let underwater = site.height < SEA_LEVEL;
        let ground = self.overworld.surface_sample(x, z, site.height, site.slope).ground;
        Place {
            rainfall: site.precipitation,
            moisture: site.moisture,
            coast: 1.0 - smoothstep(0.0, 0.35, site.continentalness),
            water: if underwater { 1.0 } else { site.river },
            altitude: (site.height - SEA_LEVEL) as f32,
            valley: around - site.height as f32,
            floor: lowest.max(SEA_LEVEL) as f32,
            latitude: site.latitude,
            vegetation: site.vegetation,
            forest: site.forest,
            aether: site.aether,
            // Dust is the ground's own colour where it is bare (sand, red sand, snow), and dry
            // earth's where plants cover it.
            dust_color: if underwater || self.blocks.def(ground).tint != Tint::None {
                LocalWeather::NONE.dust_color
            } else {
                self.blocks.def(ground).color
            },
        }
    }

    fn height(&self, x: i32, z: i32) -> i32 {
        self.overworld.height(x, z)
    }
}

/// Each effect's chance to occur in a stretch of land and time: its rarity, times how much
/// this world features it.
pub(crate) fn chances(table: &WeatherTable, featured: &Featured) -> Vec<f32> {
    let items: Vec<(&str, _)> = table.effects().iter().map(|e| (e.name.as_str(), e.rarity)).collect();
    let featured = featured.multipliers("weather", &items);
    table.effects().iter().zip(featured).map(|(effect, boost)| (effect.rarity.multiplier() * boost).min(1.0)).collect()
}

/// Lightning: where it flashes (x, z), the seed of the bolt's shape, and whether it stays
/// within the cloud rather than striking the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Strike {
    at: Vec2,
    seed: u32,
    cloud: bool,
}

/// The land and climate at one place, as effects read them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub rainfall: f32,
    pub moisture: f32,
    /// 1 at and off the coast, falling to 0 inland.
    pub coast: f32,
    /// 1 over water.
    pub water: f32,
    pub altitude: f32,
    /// Blocks below the land around.
    pub valley: f32,
    /// Height ground fog lies on: the lowest land nearby, or the sea.
    pub floor: f32,
    pub latitude: f32,
    pub vegetation: f32,
    pub forest: f32,
    pub aether: f32,
    /// Colour of the ground, which dust takes.
    pub dust_color: [u8; 3],
}

/// The weather's state: `core:humidity` (relative), `core:clouds` (cloud cover),
/// `core:precipitation` (rain or snow reaching the ground), `core:wetness` (of the ground)
/// and `core:snow_cover` (snow lying on it), all 0..1; what drives them; the effects; and
/// the weather admins have forced.
pub(crate) struct WeatherState {
    pub humidity: Field,
    pub clouds: Field,
    pub precipitation: Field,
    pub wetness: Field,
    pub snow_cover: Field,
    sky: Sky,
    land: Arc<dyn Land>,
    places: FxHashMap<IVec2, Place>,
    table: Arc<WeatherTable>,
    /// The chance each effect may occur in a stretch of land and time: its rarity times
    /// how much this world features it.
    chances: Vec<f32>,
    /// Cells simulated in the last update.
    active: FxHashSet<IVec2>,
    forced: Vec<Forcing>,
    /// Where commands changed the weather lately.
    changes: Vec<Change>,
    /// The tick of the last update.
    last: Option<u64>,
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

/// The weather at a place, as reported to a player.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Report {
    pub weather: LocalWeather,
    /// Effects showing, strongest first, with their strengths.
    pub effects: Vec<(usize, f32)>,
}

impl WeatherState {
    /// `wind` is the direction (x, z) the world's prevailing wind blows towards. `chances`
    /// holds each of `table`'s effects' chance to occur (see `chances`).
    pub fn new(world_seed: u64, wind: Vec2, land: Arc<dyn Land>, table: Arc<WeatherTable>, chances: Vec<f32>) -> Self {
        let (wet, cold) = (land.clone(), land.clone());
        Self {
            // The air's usual humidity follows the climate's rainfall, so wet climates rain
            // often and dry ones rarely.
            humidity: Field::with_cell_size("core:humidity", CELL, move |x, z| usual_humidity(wet.climate(x, z).1)),
            clouds: Field::with_cell_size("core:clouds", CELL, |_, _| 0.0),
            precipitation: Field::with_cell_size("core:precipitation", CELL, |_, _| 0.0),
            wetness: Field::with_cell_size("core:wetness", CELL, |_, _| 0.0),
            // Snow lies all year in cold climates.
            snow_cover: Field::with_cell_size("core:snow_cover", CELL, move |x, z| lasting_snow(cold.climate(x, z).0)),
            sky: Sky {
                seed: mix(world_seed ^ 0x0077_6561_7468_6572), // "weather"
                prevailing: wind.as_dvec2().normalize_or(DVec2::X),
            },
            land,
            places: FxHashMap::default(),
            chances,
            table,
            active: FxHashSet::default(),
            forced: Vec::new(),
            changes: Vec::new(),
            last: None,
        }
    }

    pub fn table(&self) -> &WeatherTable {
        &self.table
    }

    /// The seed of the weather's random streams, which also seeds the gusts of wind.
    pub fn seed(&self) -> u64 {
        self.sky.seed
    }

    /// Holds `preset` over the land around `center` for `seconds`, replacing weather forced
    /// nearby before.
    pub fn force(&mut self, preset: Preset, center: Vec2, seconds: u32, tick: u64) {
        self.forced.retain(|f| f.center.distance(center) > FORCE_RADIUS);
        let until = tick + seconds as u64 * TICKS_PER_SECOND as u64;
        self.forced.push(Forcing { preset, center, until });
        self.changes.push(Change { center, tick, release: false });
    }

    /// Holds the wind of `wind` (its speed, gusts and direction, if it has one) over the
    /// land around `center` for `seconds`, on top of whatever weather is forced there.
    pub fn force_wind(&mut self, wind: &Preset, center: Vec2, seconds: u32, tick: u64) {
        let until = tick + seconds as u64 * TICKS_PER_SECOND as u64;
        let mut preset = match self.forced.iter().rev().find(|f| f.weight(center, tick) > 0.0 && tick < f.until) {
            Some(forcing) => Preset { name: forcing.preset.name.clone(), ..forcing.preset.clone() },
            None => Preset { name: wind.name.clone(), ..Preset::default() },
        };
        preset.wind = wind.wind.or(preset.wind);
        preset.gusts = wind.gusts.or(preset.gusts);
        preset.direction = wind.direction.or(preset.direction);
        let until = self.forced_at(center, tick).map_or(until, |(_, left)| until.max(tick + left * TICKS_PER_SECOND as u64));
        self.forced.retain(|f| f.center.distance(center) > FORCE_RADIUS);
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
            wetness: self.wetness.sample(at.x, at.y),
            snow_cover: self.snow_cover.sample(at.x, at.y),
        }
    }

    fn set(&mut self, at: IVec2, cell: Cell) {
        self.humidity.set(at.x, at.y, cell.humidity);
        self.clouds.set(at.x, at.y, cell.clouds);
        self.precipitation.set(at.x, at.y, cell.precipitation);
        self.wetness.set(at.x, at.y, cell.wetness);
        self.snow_cover.set(at.x, at.y, cell.snow_cover);
    }

    fn reset(&mut self, at: IVec2) {
        self.humidity.reset(at.x, at.y);
        self.clouds.reset(at.x, at.y);
        self.precipitation.reset(at.x, at.y);
        self.wetness.reset(at.x, at.y);
        self.snow_cover.reset(at.x, at.y);
    }

    /// The strongest forced preset at `at`, and how strongly it holds.
    fn forcing(&self, at: Vec2, tick: u64) -> Option<(&Preset, f32)> {
        self.forced
            .iter()
            .map(|f| (&f.preset, f.weight(at, tick)))
            .filter(|&(_, weight)| weight > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// How hard it rains or snows in the weather cells around `at`, for showing storms
    /// coming: the 5×5 cells centred on the one `at` is in.
    pub fn nearby(&mut self, at: Vec2) -> Nearby {
        let center = at.floor().as_ivec2().div_euclid(IVec2::splat(CELL));
        let mut rain = [0u8; 25];
        for (i, value) in rain.iter_mut().enumerate() {
            let cell = center + IVec2::new(i as i32 % 5 - 2, i as i32 / 5 - 2);
            let middle = cell_center(cell);
            *value = (self.precipitation.sample(middle.x, middle.y).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        Nearby { first: (center - 2) * CELL, rain }
    }

    /// Whether a command changed the weather at `at` in the last few seconds.
    fn recently_changed(&self, at: Vec2) -> bool {
        self.changes.iter().any(|change| change.covers(at))
    }

    /// The land at a block position, remembered at 8-block resolution.
    fn place(&mut self, at: IVec2) -> Place {
        if self.places.len() >= PLACES {
            self.places.clear();
        }
        let key = at >> 3;
        let land = &self.land;
        *self.places.entry(key).or_insert_with(|| land.place(key.x * 8 + 4, key.y * 8 + 4))
    }
}

/// The evolving state of one weather cell.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    humidity: f32,
    clouds: f32,
    precipitation: f32,
    wetness: f32,
    snow_cover: f32,
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
    sent: FxHashMap<PlayerId, (Report, u64)>,
    /// How far the gust fronts have travelled with the wind where each player is.
    gusts: FxHashMap<PlayerId, Vec2>,
    /// Snowy blocks by the ground blocks they cover, once resolved from the table.
    snowy: Option<Vec<(BlockId, BlockId)>>,
    runs: u64,
}

impl Process for Weather {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        self.runs += 1;
        let (tick, day) = (server.tick, server.day);
        let positions: Vec<(PlayerId, Vec2)> =
            server.players.values().filter(|p| p.joined).map(|p| (p.id, Vec2::new(p.position.x, p.position.z))).collect();
        let around: Vec<IVec2> = positions.iter().map(|(_, at)| at.floor().as_ivec2()).collect();
        let mut strikes = server.weather.update(&mut server.temperature, &around, tick, day);

        self.sent.retain(|id, _| server.players.contains_key(id));
        self.gusts.retain(|id, _| server.players.contains_key(id));
        let dt = Tier::Low.period() as f32 / TICKS_PER_SECOND as f32;
        for &(id, at) in &positions {
            let mut report = server.weather.local(&mut server.temperature, at, tick, day);
            let offset = self.gusts.entry(id).or_default();
            *offset = advance_gusts(*offset, report.weather.wind, dt);
            report.weather.gust_offset = *offset;
            let dry = report.weather.precipitation < 0.05;
            strikes.extend(server.weather.sky.effect_lightning(id, at, report.weather.lightning, dry, tick));
            let quick = server.weather.recently_changed(at);
            let due = quick
                || self.sent.get(&id).is_none_or(|(last, sent_at)| {
                    tick >= sent_at + RESEND || changed(&last.weather, &report.weather) || named(last) != named(&report)
                });
            if due {
                let names = named(&report).into_iter().map(|i| server.weather.table.effects()[i].short_name().to_owned()).collect();
                let nearby = server.weather.nearby(at);
                server.players[&id].connection.send(ServerMessage::Weather { weather: report.weather, effects: names, nearby, quick });
                self.sent.insert(id, (report, tick));
            }
        }

        for Strike { at, seed, cloud } in strikes {
            let ground = server.weather.land.height(at.x.floor() as i32, at.y.floor() as i32) + 1;
            // A flash in the cloud lights it from inside, near its base.
            let y = if cloud { ground.max(SEA_LEVEL) + CLOUD_FLASH_HEIGHT } else { ground };
            let strike = Vec3::new(at.x, y as f32, at.y);
            for (id, player) in &positions {
                if player.distance(at) < LIGHTNING_REACH {
                    server.players[id].connection.send(ServerMessage::Lightning { at: strike, seed, cloud });
                }
            }
        }
        self.settle_snow(server, &around);
    }
}

/// The updrafts where weather is forced: its convection, so forced rain stays steady rain
/// and a forced storm is a thunderstorm whatever the sky would do there.
fn forced_lift(lift: f32, forcing: Option<(&Preset, f32)>) -> f32 {
    match forcing {
        Some((preset, weight)) => preset.convection.map_or(lift, |c| lift + (c - lift) * weight),
        None => lift,
    }
}

/// The effects to name to a player.
fn named(report: &Report) -> Vec<usize> {
    report.effects.iter().filter(|&&(_, strength)| strength >= NAMED).map(|&(i, _)| i).collect()
}

impl Weather {
    /// Brings the snow lying on the ground near players up to date with the snow cover:
    /// ground blocks turn into their snowy versions (`snow_cover` in `weather.ron`) as the
    /// cover deepens, each at its own depth so the land whitens gradually, and turn back
    /// as it melts, where the land would not be snowy anyway. A couple of columns per
    /// player each second, round the 5×5 columns around them.
    fn settle_snow(&mut self, server: &mut Server, around: &[IVec2]) {
        let blocks = server.content.blocks.clone();
        let snowy = self.snowy.get_or_insert_with(|| {
            let id = |name: &str| blocks.id(name).or_else(|| {
                tracing::warn!("weather.ron: snow cover names unknown block {name}");
                None
            });
            server.weather.table.snow_cover().iter().filter_map(|(ground, snowy)| Some((id(ground)?, id(snowy)?))).collect()
        });
        if snowy.is_empty() {
            return;
        }
        let snowy = snowy.clone();
        for (n, &at) in around.iter().enumerate() {
            for k in 0..SNOW_COLUMNS {
                let turn = (self.runs as usize * SNOW_COLUMNS + k + n * 7) % 25;
                let column = ColumnPos::of_block(IVec3::new(at.x, 0, at.y)).offset(turn as i32 % 5 - 2, turn as i32 / 5 - 2);
                settle_column(server, column, &snowy);
            }
        }
    }
}

/// Brings one loaded column's lying snow up to date; see `Weather::settle_snow`.
fn settle_column(server: &mut Server, pos: ColumnPos, snowy: &[(BlockId, BlockId)]) {
    let Some(column) = server.world.column(pos).cloned() else { return };
    let blocks = &server.content.blocks;
    let origin = pos.origin();
    let mut changes = Vec::new();
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (wx, wz) = (origin.x + x, origin.z + z);
            let Some((y, top)) = top_block(&column, blocks, x, z) else { continue };
            let cover = server.weather.snow_cover.sample(wx, wz);
            // Each block whitens at its own depth of cover, a little lower than it melts.
            let depth = 0.15 + 0.7 * unit(mix(mix(server.weather.sky.seed ^ SNOWFLAKES ^ wx as u64) ^ wz as u64)) as f32;
            if let Some(&(_, white)) = snowy.iter().find(|&&(ground, _)| ground == top).filter(|_| cover > depth) {
                changes.push((IVec3::new(wx, y, wz), white));
            } else if let Some(&(ground, _)) = snowy.iter().find(|&&(_, white)| white == top).filter(|_| cover < depth - 0.1)
                // Where the climate keeps snow lying, the generator made it snowy: leave it.
                && lasting_snow(server.temperature.sample(wx, wz)) < 0.05
            {
                changes.push((IVec3::new(wx, y, wz), ground));
            }
            if changes.len() >= SNOW_CHANGES {
                break;
            }
        }
    }
    for (at, block) in changes {
        server.set_block(at, block);
    }
}

/// The highest block in a column that rain or snow lands on, and its height: plants let
/// it through to their ground. `x` and `z` are within the column.
fn top_block(column: &Column, blocks: &BlockRegistry, x: i32, z: i32) -> Option<(i32, BlockId)> {
    let catches = |block: BlockId| !block.is_air() && blocks.info(block).render != RenderKind::Plant;
    for cy in (0..COLUMN_CHUNKS).rev() {
        if column.chunks[cy as usize].as_uniform().is_some_and(|block| !catches(block)) {
            continue;
        }
        for y in (cy * CHUNK_SIZE..(cy + 1) * CHUNK_SIZE).rev() {
            let block = column.block(IVec3::new(x, y, z));
            if catches(block) {
                return Some((y, block));
            }
        }
    }
    None
}

impl WeatherState {
    /// Advances the cells within range of each block position in `around` to `tick`, and
    /// returns cells no longer in range to their baseline. `day` is the time of day, in
    /// ticks. Returns where lightning flashed.
    fn update(&mut self, temperature: &mut Field, around: &[IVec2], tick: u64, day: f64) -> Vec<Strike> {
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
        // Normally a second has passed; after a long pause every cell catches up afresh.
        let elapsed = self.last.map_or(u64::MAX, |last| tick.saturating_sub(last));
        self.last = Some(tick);
        let dt = (elapsed.max(1) as f32 / TICKS_PER_SECOND as f32).min(CATCH_UP_STEP as f32 * 6.0);
        let stale = elapsed > CATCH_UP as u64 * TICKS_PER_SECOND as u64;
        let gone: Vec<IVec2> = self.active.iter().filter(|&cell| stale || !cells.contains(cell)).copied().collect();
        for cell in gone {
            self.reset(cell * CELL);
            self.active.remove(&cell);
        }
        let day_at = |t: i64| day_fraction(day - (tick as i64 - t) as f64);
        let mut strikes = Vec::new();
        let mut cells: Vec<IVec2> = cells.into_iter().collect();
        cells.sort_unstable_by_key(|c| (c.x, c.y));
        for &cell in &cells {
            let at = cell_center(cell);
            let climate = Climate {
                humidity: self.humidity.baseline(at.x, at.y),
                temperature: temperature.sample(at.x, at.y),
                snow_cover: self.snow_cover.baseline(at.x, at.y),
            };
            let forcing = self.forcing(at.as_vec2(), tick).map(|(preset, weight)| (preset.clone(), weight));
            let forcing = forcing.as_ref().map(|(preset, weight)| (preset, *weight));
            let mut state = if self.active.contains(&cell) {
                let drivers = self.sky.drivers(at.as_dvec2(), tick as i64, day_at(tick as i64), climate.warmth());
                let state = step(self.get(at), &drivers, &climate, day_at(tick as i64), forcing, dt);
                strikes.extend(self.sky.lightning(cell, state.precipitation, forced_lift(drivers.lift, forcing), tick));
                state
            } else {
                // Catch up from the climate's average state.
                let mut state = climate.average();
                let start = tick as i64 - CATCH_UP * TICKS_PER_SECOND as i64;
                for i in 1..=CATCH_UP / CATCH_UP_STEP {
                    let t = start + i * CATCH_UP_STEP * TICKS_PER_SECOND as i64;
                    let drivers = self.sky.drivers(at.as_dvec2(), t, day_at(t), climate.warmth());
                    state = step(state, &drivers, &climate, day_at(t), None, CATCH_UP_STEP as f32);
                }
                state
            };
            // Forced weather holds the cell at the preset.
            if let Some((preset, weight)) = forcing {
                let mix = |value: f32, forced: Option<f32>| forced.map_or(value, |f| value + (f - value) * weight);
                state.humidity = mix(state.humidity, preset.humidity);
                state.clouds = mix(state.clouds, preset.cover);
                state.precipitation = mix(state.precipitation, preset.precipitation);
            }
            self.set(at, state);
        }
        self.active = cells.into_iter().collect();
        strikes
    }

    /// The weather at a block position at `tick` and time of day `day` (in ticks): the
    /// fields interpolated between the four nearest cell centres, the kind of cloud from
    /// the drivers, the temperature and snow, any forced weather, then the effects.
    pub fn local(&mut self, temperature: &mut Field, at: Vec2, tick: u64, day: f64) -> Report {
        let corner = ((at - CELL as f32 / 2.0) / CELL as f32).floor();
        let f = (at - CELL as f32 / 2.0) / CELL as f32 - corner;
        let corner = corner.as_ivec2() * CELL + CELL / 2;
        let mut sum = |dx: i32, dz: i32, weight: f32| {
            let cell = self.get(corner + IVec2::new(dx, dz) * CELL);
            [cell.humidity, cell.clouds, cell.precipitation, cell.wetness, cell.snow_cover].map(|v| v * weight)
        };
        let corners = [
            sum(0, 0, (1.0 - f.x) * (1.0 - f.y)),
            sum(1, 0, f.x * (1.0 - f.y)),
            sum(0, 1, (1.0 - f.x) * f.y),
            sum(1, 1, f.x * f.y),
        ];
        let [humidity, cover, precipitation, wetness, snow_cover] =
            corners.iter().fold([0.0; 5], |a, c| std::array::from_fn(|i| a[i] + c[i]));

        let block = at.floor().as_ivec2();
        let place = self.place(block);
        let mean = temperature.sample(block.x, block.y);
        let warm = warmth(mean);
        let sky = &self.sky;
        let (tick_i, fraction) = (tick as i64, day_fraction(day));
        let drivers = sky.drivers(at.as_dvec2(), tick_i, fraction, warm);
        // Updrafts heap cloud into towers; without them it spreads into flat layers.
        let convection = smoothstep(0.05, 0.5, drivers.lift + 0.25 * smoothstep(0.45, 0.8, warm) * (1.0 - cover));
        // High ice cloud runs ahead of moist air, and drifts across fair skies now and then.
        let seconds = tick as f64 / TICKS_PER_SECOND as f64;
        let upwind = sky.drivers(at.as_dvec2() - sky.prevailing * 2500.0, tick_i, fraction, warm);
        let fair = sky.noise(CIRRUS, at.as_dvec2() / 2000.0, seconds / 1200.0) as f32;
        let ahead = 0.8 * smoothstep(0.15, 0.7, upwind.air - drivers.air);
        let cirrus = ahead.max(0.5 * smoothstep(0.6, 0.85, fair)) * (1.0 - cover);

        // Wind: the prevailing direction, veering over hours and swinging round as a front
        // passes. Light under settled skies, freshening where air masses meet, strong in
        // storms and livelier in the afternoon; stronger on open heights and over water,
        // calmer in forests and valleys.
        let contrast = upwind.air - drivers.air;
        let front = smoothstep(0.2, 1.2, contrast.abs());
        let settled = sky.noise(GUSTS, at.as_dvec2() / 6000.0, seconds / 1500.0) as f32;
        let veer = (sky.noise(GUSTS + 16, DVec2::ZERO, seconds / 900.0) - 0.5) as f32 + 0.4 * contrast.clamp(-1.0, 1.0);
        let direction = (sky.prevailing + sky.prevailing.perp() * (veer * 1.2) as f64).normalize();
        let afternoon = 1.0 - (((fraction - 0.35) / 0.35) as f32).abs().min(1.0);
        let exposure = (1.0 + smoothstep(10.0, 90.0, place.altitude) * 0.5 + 0.25 * place.water)
            * (1.0 - 0.35 * place.forest)
            * (1.0 - 0.3 * smoothstep(2.0, 20.0, place.valley));
        let storm = precipitation * convection;
        // Rain and snow come with a freshening wind.
        let falling = smoothstep(0.02, 0.3, precipitation);
        let speed = (1.5 + 4.0 * settled * settled + 5.0 * front * front + 3.0 * falling + 10.0 * storm * storm)
            * (0.75 + 0.35 * afternoon)
            * exposure;
        let gusts = (0.05 + 0.3 * smoothstep(4.0, 12.0, speed) + 0.15 * front + 0.45 * storm).min(1.0);

        let celsius = now_temperature(mean, cover, place.moisture, fraction);
        let mut weather = LocalWeather {
            cover,
            convection,
            cirrus,
            humidity: near_ground(humidity, mean - celsius),
            temperature: celsius,
            wind: direction.as_vec2() * speed,
            gusts,
            precipitation,
            snow: snowfall(celsius),
            wetness,
            snow_cover,
            dust_color: place.dust_color,
            ..LocalWeather::NONE
        };
        let forcing = self.forcing(at, tick).map(|(preset, weight)| (preset.clone(), weight));
        if let Some((preset, weight)) = &forcing {
            weather = preset.blend(&weather, *weight);
            if preset.temperature.is_some() && preset.snow.is_none() {
                weather.snow += (snowfall(weather.temperature) - weather.snow) * weight;
            }
        }

        let lift = forced_lift(drivers.lift, forcing.as_ref().map(|(preset, weight)| (preset, *weight)));
        let inputs = self.inputs(&weather, &place, lift, fraction);
        let mut strengths: Vec<f32> = self
            .table
            .effects()
            .iter()
            .enumerate()
            .map(|(i, effect)| {
                let strength = effect.strength(&inputs);
                if strength > 0.0 { strength * self.occurs(i, at, seconds) } else { 0.0 }
            })
            .collect();
        if let Some((preset, weight)) = &forcing {
            for index in preset.effects.iter().filter_map(|name| self.table.effect(name)) {
                strengths[index] = strengths[index].max(*weight * preset.intensity);
            }
        }
        self.table.apply(&mut weather, &strengths, place.floor);
        // What blows over snow is snow: drifting snow is white, not the ground's colour.
        if weather.snow_cover > 0.1 {
            let white = smoothstep(0.1, 0.5, weather.snow_cover);
            weather.dust_color = weather.dust_color.map(|c| (c as f32 + (250.0 - c as f32) * white) as u8);
        }
        let mut effects: Vec<(usize, f32)> = strengths.into_iter().enumerate().filter(|&(_, s)| s > 0.01).collect();
        effects.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        Report { weather, effects }
    }

    /// What effects read at a place.
    fn inputs(&self, weather: &LocalWeather, place: &Place, lift: f32, day: f64) -> Inputs {
        let sun = sun_height(day);
        let daylight = smoothstep(-0.2, 0.15, sun);
        let d = if day > 0.5 { day - 1.0 } else { day } as f32;
        let e = (day - 0.5) as f32;
        let mut inputs = Inputs::default();
        for (input, value) in [
            (Input::Cover, weather.cover),
            (Input::Convection, weather.convection),
            (Input::Cirrus, weather.cirrus),
            (Input::Humidity, weather.humidity),
            (Input::Precipitation, weather.precipitation),
            (Input::Snow, weather.snow),
            (Input::Wind, weather.wind.length()),
            (Input::Lift, lift),
            (Input::Temperature, weather.temperature),
            (Input::Rainfall, place.rainfall),
            (Input::Moisture, place.moisture),
            (Input::Coast, place.coast),
            (Input::Water, place.water),
            (Input::Altitude, place.altitude),
            (Input::Valley, place.valley),
            (Input::Latitude, place.latitude),
            (Input::Vegetation, place.vegetation),
            (Input::Forest, place.forest),
            (Input::Aether, place.aether),
            (Input::Daylight, daylight),
            (Input::Dawn, ramp(-0.06, -0.01, d) * ramp(0.14, 0.04, d)),
            (Input::Dusk, ramp(-0.08, -0.02, e) * ramp(0.07, 0.02, e)),
            (Input::Night, 1.0 - daylight),
            (Input::SunHeight, sun),
            (Input::Wetness, weather.wetness),
            (Input::SnowCover, weather.snow_cover),
        ] {
            inputs.set(input, value);
        }
        inputs
    }

    /// Whether effect `index`'s rarity lets it occur at `at` and `seconds`, 0..1: for each
    /// stretch of land (`REGION` across) and time (the effect's typical duration) a hash
    /// decides whether it may, with soft edges in between.
    fn occurs(&self, index: usize, at: Vec2, seconds: f64) -> f32 {
        let chance = self.chances[index] as f64;
        if chance >= 1.0 {
            return 1.0;
        }
        let effect = &self.table.effects()[index];
        let salt = mix(self.sky.seed ^ OCCURRENCE ^ name_hash(&effect.name));
        // Each effect's lattice is offset, so stretches of different effects do not line up.
        let shift = DVec2::new(unit(salt), unit(mix(salt)));
        let p = at.as_dvec2() / REGION + shift;
        let t = seconds / (effect.minutes as f64 * 60.0) + unit(mix(mix(salt)));
        let (i, f) = (p.floor(), p - p.floor());
        let (it, ft) = (t.floor(), t - t.floor());
        let corner = |dx: i64, dz: i64, dt: i64| {
            let h = mix(mix(mix(salt ^ (i.x as i64 + dx) as u64) ^ (i.y as i64 + dz) as u64) ^ (it as i64 + dt) as u64);
            if unit(h) < chance { 1.0 } else { 0.0 }
        };
        let fade = |t: f64| t * t * (3.0 - 2.0 * t);
        let (wx, wz, wt) = (fade(f.x), fade(f.y), fade(ft));
        let plane = |dt| {
            let near = corner(0, 0, dt) + (corner(1, 0, dt) - corner(0, 0, dt)) * wx;
            let far = corner(0, 1, dt) + (corner(1, 1, dt) - corner(0, 1, dt)) * wx;
            near + (far - near) * wz
        };
        smoothstep(0.3, 0.7, (plane(0) + (plane(1) - plane(0)) * wt) as f32)
    }
}

/// The parts of a cell's climate its weather responds to.
struct Climate {
    /// The air's usual humidity.
    humidity: f32,
    /// Mean temperature, °C.
    temperature: f32,
    /// Snow lying all year, 0..1.
    snow_cover: f32,
}

impl Climate {
    fn warmth(&self) -> f32 {
        warmth(self.temperature)
    }

    /// The cell's state on an average day.
    fn average(&self) -> Cell {
        Cell { humidity: self.humidity, clouds: 0.0, precipitation: 0.0, wetness: 0.0, snow_cover: self.snow_cover }
    }
}

impl Sky {
    /// Where lightning flashes in a cell this second, if it does: only a heavy downpour
    /// under strong updrafts is a thunderstorm, which has a chance each second to strike
    /// somewhere in the cell, drawn from the cell's own random stream, and twice that
    /// chance to flash inside the cloud instead. Steady rain never flashes.
    fn lightning(&self, cell: IVec2, rain: f32, lift: f32, tick: u64) -> Option<Strike> {
        let chance = (LIGHTNING * smoothstep(0.4, 0.8, rain * smoothstep(0.3, 0.7, lift))) as f64;
        let h = mix(mix(mix(self.seed ^ STRIKES ^ tick) ^ cell.x as u64) ^ cell.y as u64);
        let roll = unit(h);
        let place = Vec2::new(unit(mix(h)) as f32, unit(mix(mix(h))) as f32);
        (roll < 3.0 * chance).then(|| Strike { at: (cell.as_vec2() + place) * CELL as f32, seed: (h >> 32) as u32, cloud: roll >= chance })
    }

    /// Lightning from effects (thundersnow, fantastical storms) near a player, at
    /// `strength` (the `lightning` channel): 150 to 900 blocks away.
    /// Dry lightning (heat lightning: no storm overhead) flashes far off, beyond where
    /// thunder is heard (`protocol::THUNDER_HEARD`), so it is silent.
    fn effect_lightning(&self, player: PlayerId, at: Vec2, strength: f32, dry: bool, tick: u64) -> Option<Strike> {
        let h = mix(mix(self.seed ^ EFFECT_STRIKES ^ tick) ^ player as u64);
        if unit(h) >= (EFFECT_LIGHTNING * strength) as f64 {
            return None;
        }
        // A direction from a point on the unit square's edge, then a distance.
        let (a, b) = (unit(mix(h)) as f32 * 2.0 - 1.0, unit(mix(mix(h))) as f32 * 2.0 - 1.0);
        let direction = Vec2::new(a, b).normalize_or(Vec2::X);
        let far = unit(mix(mix(mix(h)))) as f32;
        let distance = if dry { THUNDER_HEARD + 20.0 + (LIGHTNING_REACH - THUNDER_HEARD - 40.0) * far } else { 150.0 + 750.0 * far };
        // Mostly strikes, some flashes within the cloud.
        Some(Strike { at: at + direction * distance, seed: (h >> 32) as u32, cloud: unit(mix(h ^ 0xc10d)) < 0.4 })
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

/// Height of the sun at a fraction of the day, as the sky draws it: the sine of its angle
/// along a slightly tilted arc. Bhaskara's approximation of the sine keeps it exact
/// arithmetic.
fn sun_height(day: f64) -> f32 {
    let x = (day * std::f64::consts::TAU) as f32;
    let (x, sign) = if x <= std::f32::consts::PI { (x, 1.0) } else { (x - std::f32::consts::PI, -1.0) };
    let pi = std::f32::consts::PI;
    let sine = 16.0 * x * (pi - x) / (5.0 * pi * pi - 4.0 * x * (pi - x));
    // The arc's tilt: (cos, sin, 0.25) normalised.
    sign * sine / 1.030_776_4
}

/// The air temperature now from the climate's mean: coldest around sunrise and warmest
/// in the early afternoon, by more in dry climates and under clear skies.
fn now_temperature(mean: f32, cover: f32, moisture: f32, day: f64) -> f32 {
    let swing = (3.0 + 5.0 * (1.0 - smoothstep(0.2, 1.5, moisture))) * (1.0 - 0.6 * cover);
    const PEAK: f64 = 0.35;
    let curve = if day < PEAK {
        -1.0 + 2.0 * smoothstep(0.0, 1.0, (day / PEAK) as f32)
    } else {
        1.0 - 2.0 * smoothstep(0.0, 1.0, ((day - PEAK) / (1.0 - PEAK)) as f32)
    };
    mean + swing * curve
}

/// The relative humidity near the ground of air whose humidity is `humidity` at the day's
/// mean temperature, when it is `cooler` degrees cooler than that (warmer if negative):
/// cool air holds less water, so its relative humidity roughly doubles with every 10 °C
/// of cooling. That is why mist forms on clear, calm nights and lifts by midday.
fn near_ground(humidity: f32, cooler: f32) -> f32 {
    // 2^(cooler / 10), from a polynomial (exact arithmetic) over the day's swing.
    let c = cooler.clamp(-12.0, 12.0);
    (humidity * (1.0 + 0.0693 * c + 0.0024 * c * c)).clamp(0.0, 1.0)
}

/// How much falls as snow at an air temperature.
fn snowfall(celsius: f32) -> f32 {
    1.0 - smoothstep(-1.5, 1.5, celsius)
}

/// Snow lying all year where the mean temperature is this cold, 0..1.
fn lasting_snow(mean: f32) -> f32 {
    1.0 - smoothstep(-6.0, 0.0, mean)
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

/// One step of `dt` seconds of a cell's response to its drivers at time of day `day`,
/// with `forcing` (a preset and its weight) if weather is forced there.
fn step(cell: Cell, drivers: &Drivers, climate: &Climate, day: f64, forcing: Option<(&Preset, f32)>, dt: f32) -> Cell {
    let rate = |seconds: f32| (dt / seconds).min(1.0);
    let temperature = climate.warmth();
    // Humidity follows the air mass, away from the climate's usual humidity.
    let target = (0.18 + 0.75 * climate.humidity + 0.42 * drivers.air).clamp(0.0, 1.0);
    let mut humidity = cell.humidity + (target - cell.humidity) * rate(150.0);
    // Cloud forms as the air nears saturation, sooner in rising air and in cold air,
    // which holds less water, and evaporates again as it dries.
    let saturation = 0.78 - 0.08 * (1.0 - temperature) - 0.2 * drivers.lift;
    let cloud_target = smoothstep(saturation - 0.4, saturation + 0.08, humidity);
    let clouds = cell.clouds + (cloud_target - cell.clouds) * rate(90.0);
    // Thick cloud rains, lightly unless the air is very moist or rising fast: drizzle and
    // light rain are common, downpours need a storm's updrafts or saturated air. Rain dries
    // the air, which thins the cloud until the air mass brings more moisture.
    let wet = smoothstep(saturation, saturation + 0.25, humidity).max(drivers.lift);
    let rain_target = smoothstep(0.8, 0.97, clouds) * (0.12 + 0.88 * wet * wet);
    let precipitation = cell.precipitation + (rain_target - cell.precipitation) * rate(30.0);
    humidity -= precipitation * 0.0012 * dt;

    // Rain wets the ground, which dries faster when it is warm and sunny; snow lies where
    // it falls and melts above freezing, faster in sunshine. Forced weather brings its own
    // rain or snow and temperature.
    let mix = |value: f32, forced: Option<f32>, weight: f32| forced.map_or(value, |f| value + (f - value) * weight);
    let (falling, mut air) = (precipitation, now_temperature(climate.temperature, clouds, 1.0, day));
    let (mut falling, mut snow) = (falling, snowfall(air));
    if let Some((preset, weight)) = forcing {
        falling = mix(falling, preset.precipitation, weight);
        air = mix(air, preset.temperature, weight);
        snow = mix(snowfall(air), preset.snow, weight);
    }
    let sunny = 1.0 - clouds;
    let half_life = DRYING / (1.0 + sunny * smoothstep(5.0, 30.0, air));
    let wetness = cell.wetness * (1.0 - (dt / half_life * std::f32::consts::LN_2).min(1.0)) + falling * (1.0 - snow) * 0.01 * dt;
    let melt = (air - 0.5).max(0.0) * 0.0004 * (1.0 + sunny) * dt;
    let snow_cover = cell.snow_cover + falling * snow * 0.004 * dt - melt;
    Cell {
        humidity: humidity.clamp(0.0, 1.0),
        clouds: clouds.clamp(0.0, 1.0),
        precipitation: precipitation.clamp(0.0, 1.0),
        wetness: wetness.clamp(0.0, 1.0),
        // Never below what the climate keeps all year.
        snow_cover: snow_cover.clamp(climate.snow_cover, 1.0),
    }
}

/// Whether a player told `a` should now be told `b` (temperature counts in fives of
/// degrees and wind in tens of blocks per second).
fn changed(a: &LocalWeather, b: &LocalWeather) -> bool {
    let fractions = a.fractions().into_iter().zip(b.fractions()).any(|((_, x), (_, y))| (x - y).abs() > CHANGE);
    let others = [(a.temperature - b.temperature) / 5.0, (a.wind.x - b.wind.x) / 10.0, (a.wind.y - b.wind.y) / 10.0];
    fractions || others.iter().any(|d| d.abs() > CHANGE) || (a.fog_top - b.fog_top).abs() > 1.0
}

/// FNV-1a, for stable hashes of names.
fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
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

    fn table() -> Arc<WeatherTable> {
        let table = WeatherTable::from_ron("base", include_str!("../../../assets/content/base/weather.ron")).unwrap();
        table.validate().unwrap();
        Arc::new(table)
    }

    /// Land whose climate and lie depend on where it is: kilometre-wide regions, each one
    /// of a few kinds of country.
    struct Countries {
        /// Only this kind of country, if set.
        only: Option<usize>,
    }

    /// Mean temperature, rainfall, and the place: moisture, coast, water, altitude,
    /// valley, latitude, vegetation, forest, aether.
    const KINDS: [(&str, f32, f32, [f32; 9]); 11] = [
        ("temperate valley", 11.0, 900.0, [1.0, 0.0, 0.0, 20.0, 14.0, 0.5, 0.8, 0.5, 0.2]),
        ("temperate coast", 13.0, 1200.0, [1.2, 1.0, 0.0, 4.0, 0.0, 0.45, 0.7, 0.2, 0.3]),
        ("hot desert", 28.0, 120.0, [0.08, 0.0, 0.0, 30.0, 0.0, 0.2, 0.05, 0.0, 0.25]),
        ("dry steppe", 17.0, 400.0, [0.45, 0.0, 0.0, 40.0, 2.0, 0.35, 0.4, 0.0, 0.3]),
        ("polar", -6.0, 500.0, [1.0, 0.3, 0.0, 15.0, 3.0, 0.85, 0.2, 0.05, 0.3]),
        ("boreal hills", 1.5, 700.0, [1.1, 0.0, 0.0, 60.0, 8.0, 0.68, 0.7, 0.7, 0.35]),
        ("rainforest marsh", 26.0, 3200.0, [1.9, 0.2, 0.6, 3.0, 4.0, 0.05, 1.0, 0.9, 0.4]),
        ("aether meadow", 12.0, 1000.0, [1.0, 0.0, 0.0, 30.0, 2.0, 0.5, 0.8, 0.2, 0.85]),
        ("cold north coast", 3.0, 800.0, [1.0, 0.9, 0.2, 5.0, 0.0, 0.6, 0.5, 0.3, 0.3]),
        ("temperate lake", 10.0, 1000.0, [1.1, 0.3, 1.0, 2.0, 0.0, 0.5, 0.6, 0.2, 0.3]),
        ("deep arctic", -20.0, 250.0, [0.8, 0.2, 0.0, 20.0, 2.0, 0.95, 0.05, 0.0, 0.3]),
    ];

    impl Countries {
        fn kind(&self, x: i32, z: i32) -> usize {
            self.only.unwrap_or_else(|| (mix(mix(x.div_euclid(3000) as u64) ^ z.div_euclid(3000) as u64) % KINDS.len() as u64) as usize)
        }
    }

    impl Land for Countries {
        fn climate(&self, x: i32, z: i32) -> (f32, f32) {
            let (_, temperature, rainfall, _) = KINDS[self.kind(x, z)];
            (temperature, rainfall)
        }

        fn place(&self, x: i32, z: i32) -> Place {
            let (_, _, rainfall, [moisture, coast, water, altitude, valley, latitude, vegetation, forest, aether]) = KINDS[self.kind(x, z)];
            Place {
                rainfall,
                moisture,
                coast,
                water,
                altitude,
                valley,
                floor: 100.0,
                latitude,
                vegetation,
                forest,
                aether,
                dust_color: [200, 170, 120],
            }
        }

        fn height(&self, _: i32, _: i32) -> i32 {
            110
        }
    }

    fn state(seed: u64, wind: Vec2, land: Countries, chances: Option<Vec<f32>>) -> (WeatherState, Field) {
        let land = Arc::new(land);
        let climate = land.clone();
        let table = table();
        let chances = chances.unwrap_or_else(|| table.effects().iter().map(|e| e.rarity.multiplier()).collect());
        let temperature = Field::new("core:temperature", move |x, z| climate.climate(x, z).0);
        (WeatherState::new(seed, wind, land, table, chances), temperature)
    }

    /// Weather everywhere like one kind of country (see `KINDS`).
    fn setup(seed: u64, wind: Vec2, kind: usize) -> (WeatherState, Field) {
        state(seed, wind, Countries { only: Some(kind) }, None)
    }

    /// Runs the weather for a player walking east from `from`, returning what they were
    /// told each second.
    fn walk(seed: u64, kind: usize, from: Vec2, seconds: u64) -> Vec<Report> {
        let (mut weather, mut temperature) = setup(seed, Vec2::new(0.6, 0.8), kind);
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
        assert_eq!(walk(7, 0, from, 600), walk(7, 0, from, 600));
        assert_ne!(walk(7, 0, from, 600), walk(8, 0, from, 600));
    }

    #[test]
    fn values_stay_in_bounds() {
        for kind in 0..KINDS.len() {
            for report in walk(3, kind, Vec2::new(5000.0, -5000.0), 1800) {
                let weather = report.weather;
                assert!(weather.fractions().iter().all(|(_, v)| (0.0..=1.0).contains(v)), "{weather:?}");
                assert!(weather.wind.is_finite() && weather.wind.length() < 30.0, "{weather:?}");
                assert!((-40.0..50.0).contains(&weather.temperature), "{weather:?}");
            }
        }
    }

    #[test]
    fn snow_only_where_freezing() {
        assert!(walk(1, 4, Vec2::ZERO, 10).iter().all(|r| r.weather.snow > 0.9 && r.weather.snow_cover == 1.0));
        assert!(walk(1, 0, Vec2::ZERO, 10).iter().all(|r| r.weather.snow == 0.0 && r.weather.snow_cover == 0.0));
    }

    /// Fraction of two days that it rains, over several places, for a kind of country.
    fn rainy_fraction(kind: usize) -> f32 {
        let (weather, mut temperature) = setup(11, Vec2::new(-1.0, 0.0), kind);
        let (mut rainy, mut steps) = (0, 0);
        for place in 0..6 {
            let at = IVec2::new(place * 20_000, place * -13_000);
            let (temperature, rainfall) = (temperature.sample(at.x, at.y), KINDS[kind].2);
            let climate = Climate { humidity: usual_humidity(rainfall), temperature, snow_cover: 0.0 };
            let mut cell = climate.average();
            for step in 0..2 * 24 * 3600 / 5 {
                let tick = step * 5 * SECOND as i64;
                let day = day_fraction(tick as f64);
                let drivers = weather.sky.drivers(at.as_dvec2(), tick, day, climate.warmth());
                cell = super::step(cell, &drivers, &climate, day, None, 5.0);
                rainy += (cell.precipitation > 0.1) as u32;
                steps += 1;
            }
        }
        rainy as f32 / steps as f32
    }

    #[test]
    fn rain_follows_the_climate() {
        let desert = rainy_fraction(2);
        let temperate = rainy_fraction(0);
        let rainforest = rainy_fraction(6);
        assert!(desert < 0.01, "desert rains {desert}");
        assert!((0.03..0.25).contains(&temperate), "temperate rains {temperate}");
        assert!(rainforest > 0.25, "rainforest rains {rainforest}");
    }

    #[test]
    fn weather_does_not_depend_on_when_players_arrived() {
        let wind = Vec2::new(0.0, 1.0);
        let (mut weather, mut temperature) = setup(5, wind, 6);
        let (here, elsewhere) = (IVec2::new(100, 100), IVec2::new(90_000, 0));
        let start = 80_000;
        let at = |s: u64| (start + s * SECOND, (start + s * SECOND) as f64);
        for s in 0..1200 {
            weather.update(&mut temperature, &[here], at(s).0, at(s).1);
        }
        let stayed = weather.local(&mut temperature, here.as_vec2(), at(1199).0, at(1199).1);

        let (mut weather, mut temperature) = setup(5, wind, 6);
        weather.update(&mut temperature, &[elsewhere], at(1198).0, at(1198).1);
        weather.update(&mut temperature, &[here], at(1199).0, at(1199).1);
        let arrived = weather.local(&mut temperature, here.as_vec2(), at(1199).0, at(1199).1);
        let close = stayed.weather.fractions().iter().zip(arrived.weather.fractions()).all(|((_, a), (_, b))| (a - b).abs() < 0.05);
        assert!(close, "{stayed:?} != {arrived:?}");
        // The cells left behind are back to their climate.
        assert_eq!(weather.clouds.deltas().filter(|&(cell, _)| cell.x > 100).count(), 0);
    }

    #[test]
    fn lightning_only_in_thunderstorms() {
        let (weather, _) = setup(9, Vec2::X, 0);
        let flashes = |rain: f32, lift: f32| {
            (0..20_000u64).filter_map(|s| weather.sky.lightning(IVec2::new(3, -4), rain, lift, s * SECOND)).collect::<Vec<_>>()
        };
        assert!(flashes(0.0, 1.0).is_empty() && flashes(1.0, 0.0).is_empty() && flashes(0.2, 0.5).is_empty());
        // Steady rain, even heavy, under ordinary skies never flashes.
        assert!(flashes(0.5, 0.5).is_empty() && flashes(1.0, 0.3).is_empty());
        // The fiercest storm: a flash every few minutes in each cell, so a few a minute
        // among the 25 cells around a player.
        let storm = flashes(1.0, 1.0);
        assert!((25..100).contains(&storm.len()), "{} flashes", storm.len());
        assert!(storm.iter().all(|s| (384.0..512.0).contains(&s.at.x) && (-512.0..-384.0).contains(&s.at.y)));
        // Most flashes stay in the cloud.
        let ground = storm.iter().filter(|s| !s.cloud).count();
        assert!(ground * 2 < storm.len() && ground * 5 > storm.len(), "{ground} of {}", storm.len());
        assert_eq!(storm, flashes(1.0, 1.0));
        // Effects' lightning strikes around the player, as often as the channel says.
        let near: Vec<_> = (0..60_000u64).filter_map(|s| weather.sky.effect_lightning(1, Vec2::ZERO, 1.0, false, s * SECOND)).collect();
        assert!((700..1300).contains(&near.len()), "{} strikes", near.len());
        assert!(near.iter().all(|s| (149.0..901.0).contains(&s.at.length())));
        // Heat lightning, with no storm overhead, flashes too far off to be heard.
        let dry: Vec<_> = (0..60_000u64).filter_map(|s| weather.sky.effect_lightning(1, Vec2::ZERO, 1.0, true, s * SECOND)).collect();
        assert!(!dry.is_empty() && dry.iter().all(|s| (THUNDER_HEARD..LIGHTNING_REACH).contains(&s.at.length())));
    }

    #[test]
    fn forced_weather_holds_then_eases_back() {
        let table = table();
        let storm = table.preset("storm").unwrap().at(1.0);
        // A desert, where it would never rain.
        let (mut weather, mut temperature) = setup(4, Vec2::X, 2);
        let (here, far) = (Vec2::new(10.0, 20.0), Vec2::new(10.0, 3000.0));
        let report = |weather: &mut WeatherState, temperature: &mut Field, s: u64| {
            let tick = 10_000 + s * SECOND;
            weather.update(temperature, &[here.as_ivec2()], tick, tick as f64);
            weather.local(temperature, here, tick, tick as f64).weather
        };
        assert_eq!(report(&mut weather, &mut temperature, 0).precipitation, 0.0);
        weather.force(storm, here, 60, 10_000 + SECOND);
        assert!(weather.recently_changed(here) && !weather.recently_changed(far));
        let held = report(&mut weather, &mut temperature, 2);
        assert_eq!((held.cover, held.precipitation, held.convection), (1.0, 1.0, 1.0), "{held:?}");
        assert!((held.wind.length() - 12.0).abs() < 1e-3);
        assert_eq!(weather.forced_at(here, 10_000 + 2 * SECOND).map(|(p, left)| (p.short_name(), left)), Some(("storm", 59)));
        assert!(weather.forced_at(far, 10_000).is_none());
        // Still forced a minute in, then fading, then released to slowly dry out.
        let reports: Vec<LocalWeather> = (3..900).map(|s| report(&mut weather, &mut temperature, s)).collect();
        assert_eq!(reports[55].precipitation, 1.0);
        assert!(reports[55].wetness > 0.3, "{:?}", reports[55]);
        assert!(reports[70].precipitation < 1.0 && reports[70].precipitation > 0.3, "{:?}", reports[70]);
        assert!(reports.last().unwrap().precipitation < 0.05, "{:?}", reports.last());
        let stopped = reports.iter().position(|r| r.precipitation < 0.01).unwrap();
        assert!(reports.last().unwrap().wetness < reports[stopped].wetness * 0.5, "the ground dries");
        assert!(weather.forced.is_empty());

        // Released at once by `/weather natural`.
        let rain = table.preset("rain").unwrap().at(0.5);
        weather.force(rain, here, 600, 10_000 + 900 * SECOND);
        assert_eq!(report(&mut weather, &mut temperature, 901).precipitation, 0.5);
        assert!(weather.release(here, 10_000 + 902 * SECOND));
        assert!(report(&mut weather, &mut temperature, 903).precipitation < 0.01);
        assert!(!weather.release(here, 10_000 + 904 * SECOND));
    }

    #[test]
    fn forced_effects_show_and_snow_builds_up_then_melts() {
        let table = table();
        let (mut weather, mut temperature) = setup(4, Vec2::X, 0);
        let here = Vec2::new(10.0, 20.0);
        let mut report = |weather: &mut WeatherState, s: u64| {
            let tick = 10_000 + s * SECOND;
            weather.update(&mut temperature, &[here.as_ivec2()], tick, tick as f64);
            weather.local(&mut temperature, here, tick, tick as f64)
        };
        report(&mut weather, 0);
        weather.force(table.preset("blizzard").unwrap().at(1.0), here, 300, 10_000);
        let blizzard = report(&mut weather, 1);
        assert_eq!(blizzard.effects[0], (table.effect("blizzard").unwrap(), 1.0));
        let shown = blizzard.weather;
        assert!(shown.fog >= 0.85 && shown.snow == 1.0 && (shown.temperature + 8.0).abs() < 1e-4, "{shown:?}");
        let deep = (2..300).map(|s| report(&mut weather, s)).last().unwrap();
        assert!(deep.weather.snow_cover > 0.9, "{:?}", deep.weather);
        // It melts once the cold snap is over (12 °C), over minutes.
        let after: Vec<f32> = (300..1500).map(|s| report(&mut weather, s).weather.snow_cover).collect();
        assert!(after[200] > 0.3 && after.last().unwrap() < &0.05, "{} then {}", after[200], after.last().unwrap());

        for name in ["sandstorm", "aurora", "rainbow", "aether_storm", "sea_fog"] {
            weather.force(table.preset(name).unwrap().at(1.0), here, 60, 10_000 + 2000 * SECOND);
            let shown = report(&mut weather, 2001);
            let effect = table.preset(name).unwrap().effects[0].clone();
            assert!(shown.effects.iter().any(|&(i, s)| table.effects()[i].short_name() == effect && s == 1.0), "{name}: {shown:?}");
        }
        let fog = report(&mut weather, 2002).weather;
        assert_eq!((fog.ground_fog, fog.fog_top), (0.85, 130.0), "sea fog lies 30 blocks over the floor");
    }

    /// How often each effect shows (strength at least 0.3) over two days in every kind of
    /// country, as a fraction of samples, with and without its rarity.
    fn spread(chances: Option<Vec<f32>>) -> Vec<f32> {
        let (mut weather, mut temperature) = state(21, Vec2::new(0.8, 0.6), Countries { only: None }, chances);
        let places: Vec<IVec2> = (0..36).map(|i| IVec2::new((i % 6) * 3000 + 1500, (i / 6) * 3000 + 1500)).collect();
        let mut shown = vec![0u32; weather.table.effects().len()];
        let mut samples = 0;
        let start = 7 * 24_000;
        for step in 0..2 * 24 * 3600 / 30 {
            let tick = start + step * 30 * SECOND;
            weather.update(&mut temperature, &places, tick, tick as f64);
            if step % 7 == 0 {
                for at in &places {
                    for &(i, strength) in &weather.local(&mut temperature, at.as_vec2(), tick, tick as f64).effects {
                        shown[i] += (strength >= NAMED) as u32;
                    }
                    samples += 1;
                }
            }
        }
        shown.iter().map(|&n| n as f32 / samples as f32).collect()
    }

    /// `cargo test --release -p voxmar_server effect_spread -- --ignored --nocapture`
    /// prints how often each effect shows, by rarity.
    #[test]
    #[ignore]
    fn effect_spread() {
        let table = table();
        let (natural, conditions) = (spread(None), spread(Some(vec![1.0; table.effects().len()])));
        for (i, effect) in table.effects().iter().enumerate() {
            println!("{:18} {:10} {:7.3}% (conditions {:6.2}%)", effect.short_name(), format!("{:?}", effect.rarity), natural[i] * 100.0, conditions[i] * 100.0);
        }
    }

    /// How each kind of country's weather is spread over four days at a dozen places: the
    /// share of time it is cloudy, rains or snows at each intensity, how hard the wind blows,
    /// and its commonest effects.
    fn climate_table() -> Vec<String> {
        let table = table();
        let mut lines = Vec::new();
        for (kind, (name, ..)) in KINDS.iter().enumerate() {
            let (mut weather, mut temperature) = state(17, Vec2::new(0.8, 0.6), Countries { only: Some(kind) }, None);
            let places: Vec<IVec2> = (0..12).map(|i| IVec2::new(i * 5000 + 64, i * -3000 + 64)).collect();
            let (mut samples, mut cloudy) = (0.0f32, 0.0f32);
            let mut falling = [0.0f32; 4];
            let mut wind = [0.0f32; 5];
            let mut shown = vec![0.0f32; table.effects().len()];
            for step in 0..4 * 24 * 3600 / 30 {
                let tick = 7 * 24_000 + step * 30 * SECOND;
                weather.update(&mut temperature, &places, tick, tick as f64);
                if step % 7 != 0 {
                    continue;
                }
                for at in &places {
                    let report = weather.local(&mut temperature, at.as_vec2(), tick, tick as f64);
                    let w = report.weather;
                    samples += 1.0;
                    cloudy += (w.cover > 0.6 && w.precipitation < 0.05) as u8 as f32;
                    if w.precipitation >= 0.05 {
                        falling[[0.3, 0.6, 0.85, f32::MAX].iter().position(|&top| w.precipitation < top).unwrap()] += 1.0;
                    }
                    let speed = w.wind.length();
                    wind[[2.0, 5.0, 9.0, 14.0, f32::MAX].iter().position(|&top| speed < top).unwrap()] += 1.0;
                    for &(i, strength) in &report.effects {
                        shown[i] += (strength >= NAMED) as u8 as f32;
                    }
                }
            }
            let pct = |n: f32| 100.0 * n / samples;
            let mut effects: Vec<(usize, f32)> = shown.iter().copied().enumerate().filter(|&(_, n)| n > 0.0).collect();
            effects.sort_by(|a, b| b.1.total_cmp(&a.1));
            let effects: Vec<String> =
                effects.iter().take(14).map(|&(i, n)| format!("{} {:.1}", table.effects()[i].short_name(), pct(n))).collect();
            lines.push(format!(
                "{name:18} cloudy {:4.1}  falling light {:4.1} moderate {:4.1} heavy {:4.1} max {:4.1}  wind <2 {:4.1} <5 {:4.1} <9 {:4.1} <14 {:4.1} more {:4.1}  {}",
                pct(cloudy),
                pct(falling[0]),
                pct(falling[1]),
                pct(falling[2]),
                pct(falling[3]),
                pct(wind[0]),
                pct(wind[1]),
                pct(wind[2]),
                pct(wind[3]),
                pct(wind[4]),
                effects.join(", "),
            ));
        }
        lines
    }

    /// `cargo test --profile iter -p voxmar_server climate_spread -- --ignored --nocapture`
    /// prints the weather of each kind of country, in percent of the time.
    #[test]
    #[ignore]
    fn climate_spread() {
        for line in climate_table() {
            println!("{line}");
        }
    }

    #[test]
    fn effects_spread_from_everyday_to_rare() {
        let table = table();
        let conditions = spread(Some(vec![1.0; table.effects().len()]));
        let natural = spread(None);
        let tier = |rarity| {
            let of: Vec<usize> = (0..natural.len()).filter(|&i| table.effects()[i].rarity == rarity).collect();
            of.iter().map(|&i| natural[i]).sum::<f32>() / of.len().max(1) as f32
        };
        use voxmar_engine::block::Rarity::*;
        let (common, uncommon, rare) = (tier(Common), tier(Uncommon), tier(Rare));
        let wonders = tier(VeryRare).max(tier(Legendary));
        assert!(common > 2.0 * uncommon && uncommon > 2.0 * rare && rare > wonders, "{common} {uncommon} {rare} {wonders}");
        for (i, effect) in table.effects().iter().enumerate() {
            // Every effect's conditions hold somewhere in some kind of country.
            assert!(conditions[i] > 0.0005, "{} never has its conditions", effect.name);
            // Everyday effects are seen every day or so somewhere.
            if effect.rarity == Common {
                assert!(natural[i] > 0.001, "{} is too rare: {}", effect.name, natural[i]);
            }
        }
    }
}
