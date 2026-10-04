//! Console commands and chat.
//!
//! A command line follows the same path as an action: the client proposes it, and the
//! server parses it, checks the player may run it and that its arguments make sense, then
//! applies it and replies in the player's console. Which commands exist is data
//! (`commands.ron` in each package); each runs one of the built-ins here, or other lines.

use voxmar_engine::command::{self, CommandDef, Lookup, Permission, Run};
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3};
use voxmar_protocol::{ChatKind, MAX_LINE, PlayerId, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::save::GameMode;
use crate::{Server, load_column};

/// How deeply aliases may run other aliases.
const MAX_DEPTH: usize = 8;
/// The fastest the time of day may run.
const MAX_DAY_SPEED: f32 = 1000.0;
/// How far `/locate` looks unless told, and at most, in blocks.
const LOCATE_RADIUS: i32 = 4000;
const MAX_LOCATE_RADIUS: i32 = 10_000;
/// Seconds forced weather lasts unless the command says otherwise, and at most.
const WEATHER_SECONDS: u32 = 300;
const MAX_WEATHER_SECONDS: u32 = 24 * 3600;
/// Farthest from the origin a teleport may go, in blocks: beyond this, positions lose
/// precision.
const MAX_COORDINATE: f32 = 1_000_000.0;
const TICKS_PER_HOUR: u64 = TICKS_PER_DAY / 24;

/// What a command says back: a line for the console, or why it could not run.
type Reply = Result<String, String>;

impl Server {
    /// Runs a command line a player typed (with or without the leading `/`).
    pub(crate) fn command(&mut self, id: PlayerId, line: &str) {
        let line: String = line.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect();
        tracing::info!("{}: /{}", self.players[&id].name, line.trim().trim_start_matches('/'));
        self.run_line(id, &line, 0);
    }

    /// Says `text` to everyone, as the player.
    pub(crate) fn chat(&mut self, id: PlayerId, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let line = format!("<{}> {text}", self.players[&id].name);
        tracing::info!("{line}");
        self.broadcast(ServerMessage::Chat { kind: ChatKind::Chat, text: line });
    }

    pub(crate) fn tell(&self, id: PlayerId, kind: ChatKind, text: impl Into<String>) {
        if let Some(player) = self.players.get(&id) {
            player.connection.send(ServerMessage::Chat { kind, text: text.into() });
        }
    }

    fn run_line(&mut self, id: PlayerId, line: &str, depth: usize) {
        let words = command::split(line);
        let Some((name, args)) = words.split_first() else { return };
        let def = match self.content.commands.find(name) {
            Ok(def) => def.clone(),
            Err(Lookup::Unknown) => return self.tell(id, ChatKind::Error, format!("There is no /{name}. /help lists the commands.")),
            Err(Lookup::Ambiguous(names)) => {
                let names = names.iter().map(|n| format!("/{n}")).collect::<Vec<_>>().join(", ");
                return self.tell(id, ChatKind::Error, format!("Several packages add /{name}: use one of {names}."));
            }
        };
        if def.permission == Permission::Admin && !self.players.get(&id).is_some_and(|p| p.admin) {
            return self.tell(id, ChatKind::Error, format!("Only admins may use /{}.", def.short_name()));
        }
        let reply = match &def.run {
            Run::Alias(lines) => {
                if depth >= MAX_DEPTH {
                    Err(format!("/{} runs too many commands inside each other.", def.short_name()))
                } else {
                    for line in command::expand(lines, args) {
                        self.run_line(id, &line, depth + 1);
                    }
                    return;
                }
            }
            Run::Help => self.help(id, args),
            Run::List => Ok(self.list()),
            Run::Seed => Ok(format!("Seed: {}", self.meta.seed)),
            Run::Time => self.time(args),
            Run::Weather => self.weather_command(id, args),
            Run::Wind => self.wind_command(id, args),
            Run::Teleport => self.teleport(id, args),
            Run::GameMode => self.game_mode(id, args),
            Run::Biome => Ok(self.biome(id)),
            Run::Locate => self.locate(id, args),
        };
        let (Ok(text) | Err(text)) = &reply;
        tracing::debug!("/{}: {text}", def.short_name());
        match reply {
            Ok(text) if !text.is_empty() => self.tell(id, ChatKind::Info, text),
            Ok(_) => {}
            Err(why) => self.tell(id, ChatKind::Error, format!("{why}\nUsage: {}", def.usage_line())),
        }
    }

    fn help(&self, id: PlayerId, args: &[String]) -> Reply {
        let admin = self.players[&id].admin;
        let allowed = |def: &CommandDef| admin || def.permission == Permission::Anyone;
        if let Some(name) = args.first() {
            let name = name.trim_start_matches('/');
            return match self.content.commands.find(name) {
                Ok(def) if allowed(def) => Ok(format!("{}\n{}", def.usage_line(), def.help)),
                Ok(def) => Ok(format!("{}\n{} (admins only)", def.usage_line(), def.help)),
                Err(_) => Err(format!("There is no /{name}.")),
            };
        }
        let mut lines = vec!["Commands (/help NAME explains one):".to_owned()];
        for def in self.content.commands.iter().filter(|def| allowed(def)) {
            let summary = def.help.split_inclusive(". ").next().unwrap_or(&def.help).trim();
            lines.push(format!("/{} — {summary}", def.short_name()));
        }
        Ok(lines.join("\n"))
    }

    fn list(&self) -> String {
        let mut names: Vec<&str> = self.players.values().filter(|p| p.joined).map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        format!("{} online: {}", names.len(), names.join(", "))
    }

    fn time(&mut self, args: &[String]) -> Reply {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            [] => return Ok(self.describe_time()),
            ["set", time] => {
                let target = match self.content.commands.time_of_day(time) {
                    Some(ticks) => ticks,
                    None => time.parse::<u64>().map_err(|_| format!("`{time}` is not a time of day."))?,
                } % TICKS_PER_DAY;
                let now = self.day.floor() as u64;
                self.day = (now + (target + TICKS_PER_DAY - now % TICKS_PER_DAY) % TICKS_PER_DAY) as f64;
            }
            ["add", amount] => self.day += parse_duration(amount).ok_or_else(|| format!("`{amount}` is not an amount of time."))? as f64,
            ["speed", speed] => {
                let speed: f32 = speed.parse().map_err(|_| format!("`{speed}` is not a number."))?;
                if !(0.0..=MAX_DAY_SPEED).contains(&speed) {
                    return Err(format!("The speed must be between 0 and {MAX_DAY_SPEED}."));
                }
                self.day_speed = speed;
            }
            ["freeze"] => self.day_speed = 0.0,
            ["resume"] => self.day_speed = 1.0,
            _ => return Err("That is not something /time does.".into()),
        }
        self.broadcast(self.time_message());
        Ok(self.describe_time())
    }

    /// `Day 3, 14:30, running normally`.
    fn describe_time(&self) -> String {
        let ticks = self.day.floor() as u64;
        let of_day = ticks % TICKS_PER_DAY;
        // Tick 0 is sunrise, at six in the morning.
        let hours = (of_day / TICKS_PER_HOUR + 6) % 24;
        let minutes = of_day % TICKS_PER_HOUR * 60 / TICKS_PER_HOUR;
        let pace = match self.day_speed {
            0.0 => "frozen".to_owned(),
            1.0 => "running normally".to_owned(),
            speed => {
                let minutes = TICKS_PER_DAY as f32 / TICKS_PER_SECOND as f32 / 60.0 / speed;
                format!("running {speed}× as fast ({} per day)", if minutes >= 1.0 { format!("{minutes:.1} minutes") } else { format!("{:.0} seconds", minutes * 60.0) })
            }
        };
        format!("Day {}, {hours:02}:{minutes:02} (tick {of_day} of {TICKS_PER_DAY}), {pace}.", ticks / TICKS_PER_DAY + 1)
    }

    fn teleport(&mut self, id: PlayerId, args: &[String]) -> Reply {
        let coordinate = |word: &String| word.starts_with('~') || word.parse::<f32>().is_ok();
        let (who, place) = match args {
            [name] if !coordinate(name) => (id, Place::Player(name.clone())),
            [a, b] if coordinate(a) && coordinate(b) => (id, Place::Ground(a.clone(), b.clone())),
            [name, other] => (self.find_player(name)?, Place::Player(other.clone())),
            [x, y, z] if coordinate(x) => (id, Place::At(x.clone(), y.clone(), z.clone())),
            [name, x, z] => (self.find_player(name)?, Place::Ground(x.clone(), z.clone())),
            [name, x, y, z] => (self.find_player(name)?, Place::At(x.clone(), y.clone(), z.clone())),
            _ => return Err("Where to?".into()),
        };
        let from = self.players[&who].position;
        let position = match place {
            Place::Player(name) => {
                let other = self.find_player(&name)?;
                self.players[&other].position
            }
            Place::At(x, y, z) => Vec3::new(axis(&x, from.x, true)?, axis(&y, from.y, false)?, axis(&z, from.z, true)?),
            Place::Ground(x, z) => {
                let (x, z) = (axis(&x, from.x, true)?, axis(&z, from.z, true)?);
                Vec3::new(x, self.ground_height(Vec2::new(x, z)) as f32, z)
            }
        };
        if position.x.abs().max(position.z.abs()) > MAX_COORDINATE || !(-64.0..WORLD_HEIGHT as f32 + 512.0).contains(&position.y) {
            return Err(format!("{:.1} {:.1} {:.1} is too far out.", position.x, position.y, position.z));
        }
        let tick = self.tick;
        let player = self.players.get_mut(&who).unwrap();
        player.position = position;
        player.fall_start = None;
        player.last_input_tick = tick;
        player.connection.send(ServerMessage::Teleport { position });
        let name = player.name.clone();
        let place = format!("{:.1} {:.1} {:.1}", position.x, position.y, position.z);
        if who == id {
            Ok(format!("Teleported to {place}."))
        } else {
            self.tell(who, ChatKind::Info, format!("{} teleported you to {place}.", self.players[&id].name));
            Ok(format!("Teleported {name} to {place}."))
        }
    }

    /// The height to stand at on the ground at `at`: on the highest solid block, loading
    /// the column first if need be.
    fn ground_height(&mut self, at: Vec2) -> i32 {
        let block = at.floor().as_ivec2();
        let pos = ColumnPos::of_block(IVec3::new(block.x, 0, block.y));
        if !self.world.contains(pos) {
            let column = load_column(self.save.as_ref(), &*self.generator, &self.content.blocks, pos);
            self.world.insert(pos, std::sync::Arc::new(column));
        }
        (0..WORLD_HEIGHT).rev().find(|&y| self.is_solid(IVec3::new(block.x, y, block.y))).map_or(WORLD_HEIGHT, |y| y + 1)
    }

    fn game_mode(&mut self, id: PlayerId, args: &[String]) -> Reply {
        let (mode, who) = match args {
            [mode] => (mode, id),
            [mode, name] => (mode, self.find_player(name)?),
            _ => return Err("Which mode?".into()),
        };
        let mode = match mode.to_ascii_lowercase().as_str() {
            "survival" | "s" | "0" => GameMode::Survival,
            "creative" | "c" | "1" => GameMode::Creative,
            other => return Err(format!("There is no `{other}` mode.")),
        };
        let player = self.players.get_mut(&who).unwrap();
        player.allow_flight = mode == GameMode::Creative;
        player.flying &= player.allow_flight;
        player.fall_start = None;
        player.connection.send(ServerMessage::Abilities { allow_flight: player.allow_flight, admin: player.admin });
        let (name, mode) = (player.name.clone(), if player.allow_flight { "creative" } else { "survival" });
        if who == id {
            Ok(format!("You are now in {mode} mode."))
        } else {
            self.tell(who, ChatKind::Info, format!("You are now in {mode} mode."));
            Ok(format!("{name} is now in {mode} mode."))
        }
    }

    fn weather_command(&mut self, id: PlayerId, args: &[String]) -> Reply {
        let position = self.players[&id].position;
        let at = Vec2::new(position.x, position.z);
        let (tick, day) = (self.tick, self.day);
        match args.first().map(String::as_str) {
            None => {
                let report = self.weather.local(&mut self.temperature, at, tick, day);
                let weather = report.weather;
                let effects: Vec<String> = report
                    .effects
                    .iter()
                    .filter(|&&(_, strength)| strength >= 0.05)
                    .map(|&(i, strength)| format!("{} {:.0}%", self.weather.table().effects()[i].short_name(), strength * 100.0))
                    .collect();
                let effects = if effects.is_empty() { String::new() } else { format!("\nEffects: {}.", effects.join(", ")) };
                let forced = match self.weather.forced_at(at, tick) {
                    Some((preset, 0)) => format!(", easing back from {}", preset.short_name()),
                    Some((preset, left)) => format!(", {} for {}", preset.short_name(), minutes(left)),
                    None => String::new(),
                };
                Ok(format!("{}{forced}: {}.{effects}", weather.describe(), weather.summary()))
            }
            Some("list") => {
                let names: Vec<&str> = self.content.weather.presets().iter().map(|p| p.short_name()).collect();
                Ok(format!("Kinds of weather: {}, and natural.", names.join(", ")))
            }
            Some("natural") => {
                if args.len() > 1 {
                    return Err("`natural` takes no time.".into());
                }
                Ok(if self.weather.release(at, tick) { "The natural weather is back." } else { "The weather here is already natural." }.into())
            }
            Some(name) => {
                let preset = self.content.weather.preset(name).ok_or_else(|| format!("There is no weather called `{name}`; /weather list names them."))?.clone();
                if args.len() > 2 {
                    return Err("Too many words.".into());
                }
                let seconds = weather_seconds(args.get(1))?;
                let reply = format!("Bringing {} to the land around you for {}.", preset.short_name(), minutes(seconds as u64));
                self.weather.force(preset, at, seconds, tick);
                Ok(reply)
            }
        }
    }

    fn wind_command(&mut self, id: PlayerId, args: &[String]) -> Reply {
        let position = self.players[&id].position;
        let at = Vec2::new(position.x, position.z);
        let (tick, day) = (self.tick, self.day);
        match args.first().map(String::as_str) {
            None => {
                let wind = self.weather.local(&mut self.temperature, at, tick, day).weather;
                let from = if wind.wind.length() < 0.05 { "nowhere".to_owned() } else { format!("the {}", compass(-wind.wind)) };
                Ok(format!("Wind {:.1} blocks/s from {from}, gusting {:.0}%.", wind.wind.length(), wind.gusts * 100.0))
            }
            Some("list") => {
                let names: Vec<&str> = self.content.weather.winds().iter().map(|w| w.short_name()).collect();
                Ok(format!("Winds: {}.", names.join(", ")))
            }
            Some(name) => {
                let mut wind = self.content.weather.wind(name).ok_or_else(|| format!("There is no wind called `{name}`; /wind list names them."))?.clone();
                let mut rest = &args[1..];
                if let Some(from) = rest.first().and_then(|word| direction_from(word)) {
                    wind.direction = Some(-from);
                    rest = &rest[1..];
                }
                if rest.len() > 1 {
                    return Err("Too many words.".into());
                }
                let seconds = weather_seconds(rest.first())?;
                let from = wind.direction.map_or(String::new(), |d| format!(" from the {}", compass(-d)));
                let reply = format!("Bringing a {}{from} to the land around you for {}.", wind.short_name(), minutes(seconds as u64));
                self.weather.force_wind(&wind, at, seconds, tick);
                Ok(reply)
            }
        }
    }

    /// The biome the player stands in, and what makes it.
    fn biome(&self, id: PlayerId) -> String {
        let at = self.players[&id].position.floor().as_ivec3();
        let site = self.overworld.site(at.x, at.z);
        let biome = self.overworld.biome_at(at.x, at.z);
        let tags = if biome.tags.is_empty() { String::new() } else { format!(" ({})", biome.tags.join(", ")) };
        format!(
            "{}{tags}, {:?}. {:.1} °C mean, {:.0} mm of rain a year, aether {:.2}.",
            biome.name, biome.rarity, site.temperature, site.precipitation, site.aether
        )
    }

    /// `/locate biome NAME [RADIUS]`: the nearest place of a biome.
    fn locate(&self, id: PlayerId, args: &[String]) -> Reply {
        let (name, radius) = match args {
            [kind, name] if kind == "biome" => (name, LOCATE_RADIUS),
            [kind, name, radius] if kind == "biome" => {
                let radius: i32 = radius.parse().ok().filter(|r| (1..=MAX_LOCATE_RADIUS).contains(r))
                    .ok_or_else(|| format!("The radius must be a number of blocks up to {MAX_LOCATE_RADIUS}."))?;
                (name, radius)
            }
            _ => return Err("Locate what?".into()),
        };
        if self.overworld.biomes().id(name).or_else(|| self.overworld.biomes().id(&format!("base:{name}"))).is_none() {
            return Err(format!("There is no biome called `{name}`."));
        }
        let position = self.players[&id].position;
        let from = IVec2::new(position.x.floor() as i32, position.z.floor() as i32);
        match self.overworld.locate_biome(name, from, radius) {
            Some(at) => {
                let offset = (at - from).as_vec2();
                let way = if offset.length() < 1.0 { String::new() } else { format!(" to the {}", compass(offset)) };
                Ok(format!("The nearest {name} is at {} {}, {:.0} blocks away{way}. /tp {} {} goes there.", at.x, at.y, offset.length(), at.x, at.y))
            }
            None => Err(format!("No {name} within {radius} blocks.")),
        }
    }

    /// The online player called `name`, ignoring case if no name matches exactly.
    fn find_player(&self, name: &str) -> Result<PlayerId, String> {
        let joined = || self.players.values().filter(|p| p.joined);
        if let Some(player) = joined().find(|p| p.name == name) {
            return Ok(player.id);
        }
        let mut similar = joined().filter(|p| p.name.eq_ignore_ascii_case(name));
        match (similar.next(), similar.next()) {
            (Some(player), None) => Ok(player.id),
            _ => Err(format!("Nobody called {name} is online.")),
        }
    }
}

/// Where `/tp` sends a player.
enum Place {
    Player(String),
    At(String, String, String),
    /// On the ground at x and z.
    Ground(String, String),
}

/// One teleport coordinate: a number, or `~` (the player's) plus an optional offset.
/// Whole numbers along x and z mean the middle of that block.
fn axis(word: &str, current: f32, horizontal: bool) -> Result<f32, String> {
    let error = || format!("`{word}` is not a coordinate.");
    let value = match word.strip_prefix('~') {
        Some("") => current,
        Some(offset) => current + offset.parse::<f32>().map_err(|_| error())?,
        None => {
            let value: f32 = word.parse().map_err(|_| error())?;
            if horizontal && !word.contains('.') { value + 0.5 } else { value }
        }
    };
    if value.is_finite() { Ok(value) } else { Err(error()) }
}

/// The direction a wind named as coming from `word` (a compass point or degrees clockwise
/// from north) comes from, as a unit vector (x, z). North is −z.
fn direction_from(word: &str) -> Option<Vec2> {
    let degrees: f32 = match word.to_ascii_lowercase().as_str() {
        "n" | "north" => 0.0,
        "ne" | "northeast" => 45.0,
        "e" | "east" => 90.0,
        "se" | "southeast" => 135.0,
        "s" | "south" => 180.0,
        "sw" | "southwest" => 225.0,
        "w" | "west" => 270.0,
        "nw" | "northwest" => 315.0,
        other => other.parse().ok().filter(|d: &f32| d.is_finite())?,
    };
    let (sin, cos) = degrees.to_radians().sin_cos();
    Some(Vec2::new(sin, -cos))
}

/// The compass point nearest a direction (x, z): `north-east`.
fn compass(direction: Vec2) -> &'static str {
    const POINTS: [&str; 8] = ["north", "north-east", "east", "south-east", "south", "south-west", "west", "north-west"];
    let degrees = direction.x.atan2(-direction.y).to_degrees().rem_euclid(360.0);
    POINTS[((degrees + 22.5) / 45.0) as usize % 8]
}

/// `5 minutes`, `90 seconds`.
fn minutes(seconds: u64) -> String {
    match seconds {
        1 => "a second".into(),
        s if s < 120 => format!("{s} seconds"),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{:.1} minutes", s as f32 / 60.0),
    }
}

/// An amount of time in ticks: `100` or `100t` ticks, `30s` seconds, `2h` in-game hours,
/// `1d` days.
fn parse_duration(word: &str) -> Option<u64> {
    let unit = match word.chars().last()? {
        't' => 1,
        's' => TICKS_PER_SECOND as u64,
        'h' => TICKS_PER_HOUR,
        'd' => TICKS_PER_DAY,
        c if c.is_ascii_digit() => return word.parse().ok(),
        _ => return None,
    };
    let amount: f64 = word[..word.len() - 1].parse().ok()?;
    (amount.is_finite() && amount >= 0.0).then(|| (amount * unit as f64).round() as u64)
}

/// Seconds forced weather should last: `seconds` if given and sensible.
fn weather_seconds(seconds: Option<&String>) -> Result<u32, String> {
    let Some(word) = seconds else { return Ok(WEATHER_SECONDS) };
    match word.parse::<u32>() {
        Ok(seconds) if (1..=MAX_WEATHER_SECONDS).contains(&seconds) => Ok(seconds),
        _ => Err(format!("`{word}` is not a number of seconds from 1 to {MAX_WEATHER_SECONDS}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("100"), Some(100));
        assert_eq!(parse_duration("100t"), Some(100));
        assert_eq!(parse_duration("30s"), Some(600));
        assert_eq!(parse_duration("1.5h"), Some(1500));
        assert_eq!(parse_duration("2d"), Some(48_000));
        assert_eq!(parse_duration("-1d"), None);
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn compass_points() {
        let north = direction_from("n").unwrap();
        assert!((north - Vec2::new(0.0, -1.0)).length() < 1e-6);
        assert!((direction_from("90").unwrap() - Vec2::X).length() < 1e-6);
        assert_eq!(compass(north), "north");
        assert_eq!(compass(direction_from("sw").unwrap()), "south-west");
        assert_eq!(compass(direction_from("100").unwrap()), "east");
        assert!(direction_from("up").is_none());
    }

    #[test]
    fn coordinates() {
        assert_eq!(axis("~", 10.25, true), Ok(10.25));
        assert_eq!(axis("~-3", 10.0, false), Ok(7.0));
        assert_eq!(axis("12", 0.0, true), Ok(12.5));
        assert_eq!(axis("-12", 0.0, true), Ok(-11.5));
        assert_eq!(axis("12.25", 0.0, true), Ok(12.25));
        assert_eq!(axis("64", 0.0, false), Ok(64.0));
        assert!(axis("x", 0.0, true).is_err() && axis("~y", 0.0, true).is_err() && axis("inf", 0.0, true).is_err());
    }
}
