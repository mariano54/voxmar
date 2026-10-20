//! Console commands and chat.
//!
//! A command line follows the same path as an action: the client proposes it, and the
//! server parses it, checks the player may run it and that its arguments make sense, then
//! applies it and replies in the player's console. Which commands exist is data
//! (`commands.ron` in each package); each runs one of the built-ins here, or other lines.

use voxmar_engine::command::{self, CommandDef, Lookup, Permission, Run};
use voxmar_engine::weather::{INTENSITIES, parse_intensity};
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::inventory::{ItemStack, MAX_STACK};
use voxmar_engine::lang::Words;
use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3};
use voxmar_protocol::{ChatKind, MAX_LINE, PlayerId, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::save::GameMode;
use crate::{Server, load_column, wear};

/// What players call a species: its name in their language, or its own title.
pub(crate) fn creature_name(def: &voxmar_engine::creature::CreatureDef) -> Words {
    Words::Own(voxmar_engine::lang::def_key("creature", &def.name), def.title.clone())
}

/// How deeply aliases may run other aliases.
const MAX_DEPTH: usize = 8;
/// The fastest the time of day may run.
const MAX_DAY_SPEED: f32 = 1000.0;
/// How far `/locate` looks unless told, and at most, in blocks.
const LOCATE_RADIUS: i32 = 4000;
const MAX_LOCATE_RADIUS: i32 = 10_000;
/// How far `/locate ore` looks unless told: samples lie over every vein, a hundred or so
/// blocks apart.
const ORE_RADIUS: i32 = 1000;
/// Seconds forced weather lasts unless the command says otherwise, and at most.
const WEATHER_SECONDS: u32 = 300;
const MAX_WEATHER_SECONDS: u32 = 24 * 3600;
/// Farthest from the origin a teleport may go, in blocks: beyond this, positions lose
/// precision.
const MAX_COORDINATE: f32 = 1_000_000.0;
const TICKS_PER_HOUR: u64 = TICKS_PER_DAY / 24;
/// Most blocks one `/fill` may change, and the widest it may be (so it loads at most a
/// few columns).
const MAX_FILL: i64 = 32_768;
const MAX_FILL_SIDE: i32 = 128;

/// What a command says back: words for the console, or why it could not run.
pub(crate) type Reply = Result<Words, Words>;

/// Who runs a command: a player, or the dedicated server's own console, which is an admin
/// and stands at the world's spawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Player(PlayerId),
    Console,
}

impl Server {
    /// Runs a command line a player typed (with or without the leading `/`).
    pub(crate) fn command(&mut self, id: PlayerId, line: &str) {
        let line: String = line.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect();
        tracing::info!("{}: /{}", self.players[&id].name, line.trim().trim_start_matches('/'));
        self.run_line(Source::Player(id), &line, 0);
    }

    /// A line typed into the dedicated server's console: a command, with or without the
    /// `/`, or with a leading `say ` something to tell everyone.
    pub(crate) fn console(&mut self, line: &str) {
        let line: String = line.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect();
        if let Some(text) = line.trim().strip_prefix("say ") {
            let text = Words::key("chat.server").with("text", text.trim());
            tracing::info!("{}", self.english(&text));
            self.broadcast(ServerMessage::Chat { kind: ChatKind::Chat, text });
        } else {
            self.run_line(Source::Console, &line, 0);
        }
    }

    /// Says `text` to everyone, as the player.
    pub(crate) fn chat(&mut self, id: PlayerId, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let line = Words::key("chat.player").with("name", self.players[&id].name.as_str()).with("text", text);
        tracing::info!("{}", self.english(&line));
        self.broadcast(ServerMessage::Chat { kind: ChatKind::Chat, text: line });
    }

    /// Tells whoever ran a command something, in their console.
    pub(crate) fn tell(&self, to: Source, kind: ChatKind, text: Words) {
        match to {
            Source::Player(id) => {
                if let Some(player) = self.players.get(&id) {
                    player.connection.send(ServerMessage::Chat { kind, text });
                }
            }
            Source::Console if kind == ChatKind::Error => self.english(&text).lines().for_each(|line| tracing::warn!("{line}")),
            Source::Console => self.english(&text).lines().for_each(|line| tracing::info!("{line}")),
        }
    }

    /// Tells `player` that what they tried was refused, and why: the interface shows it
    /// with its other refusals rather than as a console line.
    pub(crate) fn refuse(&self, player: PlayerId, why: Words) {
        // Every reason the server refuses with is listed in `refusals.ron`, which checks
        // each has words in every language.
        if let Words::Key(key, _) = &why
            && !self.content.refusals.sent(key)
        {
            debug_assert!(false, "refused with `{key}`, which refusals.ron does not list under `sent`");
            tracing::warn!("refused with `{key}`, which refusals.ron does not list under `sent`");
        }
        if let Some(player) = self.players.get(&player) {
            player.connection.send(ServerMessage::Refused { why });
        }
    }

    /// Words in English, for the server's own log and console.
    pub(crate) fn english(&self, words: &Words) -> String {
        self.content.english.render(words, &self.content.blocks)
    }

    fn is_admin(&self, source: Source) -> bool {
        match source {
            Source::Player(id) => self.players.get(&id).is_some_and(|p| p.admin),
            Source::Console => true,
        }
    }

    /// The name others are told did something.
    fn name_of(&self, source: Source) -> Words {
        match source {
            Source::Player(id) => self.players.get(&id).map_or_else(|| Words::text(""), |p| Words::text(&p.name)),
            Source::Console => Words::key("command.the_server"),
        }
    }

    /// Where a command runs: the player's feet, or the world's spawn for the console.
    fn position_of(&self, source: Source) -> Vec3 {
        match source {
            Source::Player(id) => self.players.get(&id).map_or(self.spawn, |p| p.position),
            Source::Console => self.spawn,
        }
    }

    /// The player a command acts on when it names none: whoever ran it.
    fn me(source: Source) -> Result<PlayerId, Words> {
        match source {
            Source::Player(id) => Ok(id),
            Source::Console => Err(Words::key("command.error.console_is_nobody")),
        }
    }

    fn run_line(&mut self, id: Source, line: &str, depth: usize) {
        let words = command::split(line);
        let Some((name, args)) = words.split_first() else { return };
        let def = match self.content.commands.find(name) {
            Ok(def) => def.clone(),
            Err(Lookup::Unknown) => return self.tell(id, ChatKind::Error, Words::key("command.error.unknown").with("command", name.as_str())),
            Err(Lookup::Ambiguous(names)) => {
                let names = Words::List(names.iter().map(|n| Words::text(format!("/{n}"))).collect());
                return self.tell(id, ChatKind::Error, Words::key("command.error.ambiguous").with("command", name.as_str()).with("names", names));
            }
        };
        if def.permission == Permission::Admin && !self.is_admin(id) {
            return self.tell(id, ChatKind::Error, Words::key("command.error.admins_only").with("command", def.short_name()));
        }
        let reply = match &def.run {
            Run::Alias(lines) => {
                if depth >= MAX_DEPTH {
                    Err(Words::key("command.error.too_deep").with("command", def.short_name()))
                } else {
                    for line in command::expand(lines, args) {
                        self.run_line(id, &line, depth + 1);
                    }
                    return;
                }
            }
            Run::Help => self.help(id, args),
            Run::List => Ok(self.list()),
            Run::Seed => Ok(Words::key("command.seed").with("seed", Words::text(self.meta.seed))),
            Run::Time => self.time(args),
            Run::Weather => self.weather_command(id, args),
            Run::Wind => self.wind_command(id, args),
            Run::Teleport => self.teleport(id, args),
            Run::GameMode => self.game_mode(id, args),
            Run::SetBlock => self.set_block_command(id, args),
            Run::Fill => self.fill(id, args),
            Run::Give => self.give(id, args),
            Run::Wear => self.wear_command(id, args),
            Run::Clear => self.clear(id, args),
            Run::Effect => self.effect_command(id, args),
            Run::Kill => self.kill_command(id),
            Run::Heal => self.heal_command(id),
            Run::Op => self.op_command(args, true),
            Run::Deop => self.op_command(args, false),
            Run::Drop => self.drop_command(id, args),
            Run::Grow => self.grow_command(id, args),
            Run::Biome => Ok(self.biome(id)),
            Run::Locate => self.locate(id, args),
            Run::Summon => self.summon(id, args),
            Run::Creatures => match args.first().map(String::as_str) {
                None => Ok(self.creatures_about(id)),
                Some(_) if !self.is_admin(id) => Err(Words::key("command.creatures.admins_only")),
                Some(what) => self.creatures_command(id, what, args.get(1).map(String::as_str)),
            },
            Run::Skill => self.skill_command(id, args),
            Run::Quest => self.quest_command(id, args),
            Run::Machine => self.machine_command(id, args),
            Run::Wander => self.wander_command(id, args),
            Run::Home => self.home_command(id),
        };
        let (Ok(text) | Err(text)) = &reply;
        tracing::debug!("/{}: {}", def.short_name(), self.english(text));
        match reply {
            Ok(text) => self.tell(id, ChatKind::Info, text),
            Err(why) => self.tell(id, ChatKind::Error, Words::key("command.error.usage").with("why", why).with("usage", self.usage_line(&def))),
        }
    }

    /// `def`'s usage line, with the lists the content makes filled in: `{sizes}`, the
    /// settlement sizes ("hamlet|village").
    fn usage_line(&self, def: &CommandDef) -> String {
        def.usage_line().replace("{sizes}", &self.overworld.settlement_sizes().join("|"))
    }

    fn help(&self, id: Source, args: &[String]) -> Reply {
        let admin = self.is_admin(id);
        let allowed = |def: &CommandDef| admin || def.permission == Permission::Anyone;
        if let Some(name) = args.first() {
            let name = name.trim_start_matches('/');
            return match self.content.commands.find(name) {
                Ok(def) if allowed(def) => Ok(Words::Lines(vec![Words::text(self.usage_line(def)), help(def)])),
                Ok(def) => Ok(Words::Lines(vec![Words::text(self.usage_line(def)), Words::key("command.help.admins_only").with("help", help(def))])),
                Err(_) => Err(Words::key("command.error.no_such_command").with("command", name)),
            };
        }
        let mut lines = vec![Words::key("command.help.title")];
        for def in self.content.commands.iter().filter(|def| allowed(def)) {
            lines.push(Words::key("command.help.line").with("command", def.short_name()).with("summary", def.summary_words()));
        }
        Ok(Words::Lines(lines))
    }

    fn list(&self) -> Words {
        let mut names: Vec<&str> = self.players.values().filter(|p| p.joined).map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        Words::key("command.list").with("count", Words::text(names.len())).with("names", Words::List(names.into_iter().map(Words::from).collect()))
    }

    fn time(&mut self, args: &[String]) -> Reply {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            [] => return Ok(self.describe_time()),
            ["set", time] => {
                let target = match self.content.commands.time_of_day(time) {
                    Some(ticks) => ticks,
                    None => time.parse::<u64>().map_err(|_| Words::key("command.error.not_a_time").with("word", *time))?,
                } % TICKS_PER_DAY;
                let now = self.day.floor() as u64;
                self.day = (now + (target + TICKS_PER_DAY - now % TICKS_PER_DAY) % TICKS_PER_DAY) as f64;
            }
            ["add", amount] => {
                self.day += parse_duration(amount).ok_or_else(|| Words::key("command.error.not_an_amount_of_time").with("word", *amount))? as f64
            }
            ["speed", speed] => {
                let speed: f32 = speed.parse().map_err(|_| Words::key("command.error.not_a_number").with("word", *speed))?;
                if !(0.0..=MAX_DAY_SPEED).contains(&speed) {
                    return Err(Words::key("command.error.speed_range").with("max", Words::text(MAX_DAY_SPEED)));
                }
                self.day_speed = speed;
            }
            ["freeze"] => self.day_speed = 0.0,
            ["resume"] => self.day_speed = 1.0,
            _ => return Err(Words::key("command.error.time_cannot")),
        }
        self.broadcast(self.time_message());
        Ok(self.describe_time())
    }

    /// `Day 3, 14:30, running normally`.
    fn describe_time(&self) -> Words {
        let ticks = self.day.floor() as u64;
        let of_day = ticks % TICKS_PER_DAY;
        // Tick 0 is sunrise, at four in the morning: the sun is up two thirds of the day, so
        // it is highest at noon and sets at eight in the evening.
        let hours = (of_day / TICKS_PER_HOUR + 4) % 24;
        let minutes = of_day % TICKS_PER_HOUR * 60 / TICKS_PER_HOUR;
        let pace = match self.day_speed {
            0.0 => Words::key("command.time.frozen"),
            1.0 => Words::key("command.time.normal"),
            speed => {
                let minutes = self.content.commands.day_minutes() / speed;
                let day = if minutes >= 1.0 {
                    Words::key("command.time.minutes").with("minutes", Words::text(format!("{minutes:.1}")))
                } else {
                    Words::key("command.time.seconds").with("seconds", Words::text(format!("{:.0}", minutes * 60.0)))
                };
                Words::key("command.time.fast").with("speed", Words::text(speed)).with("day", day)
            }
        };
        Words::key("command.time")
            .with("day", Words::text(ticks / TICKS_PER_DAY + 1))
            .with("clock", Words::text(format!("{hours:02}:{minutes:02}")))
            .with("tick", Words::text(of_day))
            .with("ticks", Words::text(TICKS_PER_DAY))
            .with("pace", pace)
    }

    fn teleport(&mut self, id: Source, args: &[String]) -> Reply {
        let coordinate = |word: &String| word.starts_with('~') || word.parse::<f32>().is_ok();
        let (who, place) = match args {
            [name] if !coordinate(name) => (Self::me(id)?, Place::Player(name.clone())),
            [a, b] if coordinate(a) && coordinate(b) => (Self::me(id)?, Place::Ground(a.clone(), b.clone())),
            [name, other] => (self.find_player(name)?, Place::Player(other.clone())),
            [x, y, z] if coordinate(x) => (Self::me(id)?, Place::At(x.clone(), y.clone(), z.clone())),
            [name, x, z] => (self.find_player(name)?, Place::Ground(x.clone(), z.clone())),
            [name, x, y, z] => (self.find_player(name)?, Place::At(x.clone(), y.clone(), z.clone())),
            _ => return Err(Words::key("command.error.where_to")),
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
            let at = format!("{:.1} {:.1} {:.1}", position.x, position.y, position.z);
            return Err(Words::key("command.error.too_far").with("place", at));
        }
        let player = self.players.get_mut(&who).unwrap();
        player.fall_start = None;
        player.teleport(position);
        let name = player.name.clone();
        let place = format!("{:.1} {:.1} {:.1}", position.x, position.y, position.z);
        if id == Source::Player(who) {
            Ok(Words::key("command.tp.you").with("place", place))
        } else {
            self.tell(Source::Player(who), ChatKind::Info, Words::key("command.tp.by").with("who", self.name_of(id)).with("place", place.as_str()));
            Ok(Words::key("command.tp.them").with("name", name).with("place", place))
        }
    }

    /// The height to stand at on the ground at `at`: on the highest solid block, loading
    /// the column first if need be.
    fn ground_height(&mut self, at: Vec2) -> i32 {
        let block = at.floor().as_ivec2();
        let column = IVec3::new(block.x, 0, block.y);
        self.load_area(column, column);
        (0..WORLD_HEIGHT).rev().find(|&y| self.is_solid(IVec3::new(block.x, y, block.y))).map_or(WORLD_HEIGHT, |y| y + 1)
    }

    fn game_mode(&mut self, id: Source, args: &[String]) -> Reply {
        let (mode, who) = match args {
            [mode] => (mode, Self::me(id)?),
            [mode, name] => (mode, self.find_player(name)?),
            _ => return Err(Words::key("command.error.which_mode")),
        };
        let mode = match mode.to_ascii_lowercase().as_str() {
            "survival" | "s" | "0" => GameMode::Survival,
            "creative" | "c" | "1" => GameMode::Creative,
            other => return Err(Words::key("command.error.no_such_mode").with("mode", other)),
        };
        let player = self.players.get_mut(&who).unwrap();
        player.allow_flight = mode == GameMode::Creative;
        player.flying &= player.allow_flight;
        player.fall_start = None;
        player.connection.send(ServerMessage::Abilities { allow_flight: player.allow_flight, admin: player.admin });
        let (name, mode) = (player.name.clone(), Words::key(if player.allow_flight { "mode.creative" } else { "mode.survival" }));
        if id == Source::Player(who) {
            Ok(Words::key("command.gamemode.you").with("mode", mode))
        } else {
            self.tell(Source::Player(who), ChatKind::Info, Words::key("command.gamemode.you").with("mode", mode.clone()));
            Ok(Words::key("command.gamemode.them").with("name", name).with("mode", mode))
        }
    }

    fn weather_command(&mut self, id: Source, args: &[String]) -> Reply {
        let position = self.position_of(id);
        let at = Vec2::new(position.x, position.z);
        let (tick, day) = (self.tick, self.day);
        match args.first().map(String::as_str) {
            None => {
                let report = self.weather.local(&mut self.temperature, at, tick, day);
                let weather = report.weather;
                let effects: Vec<Words> = report
                    .effects
                    .iter()
                    .filter(|&&(_, strength)| strength >= 0.05)
                    .map(|&(i, strength)| share(weather_name("effect", self.weather.table().effects()[i].short_name()), strength))
                    .collect();
                let mut report = vec![{
                    let state = Words::key(&format!("weather.state.{}", weather.describe().replace(' ', "_")));
                    let state = match self.weather.forced_at(at, tick) {
                        Some((preset, 0)) => Words::key("command.weather.easing").with("state", state).with("kind", weather_name("kind", preset.short_name())),
                        Some((preset, left)) => Words::key("command.weather.forced")
                            .with("state", state)
                            .with("kind", weather_name("kind", preset.short_name()))
                            .with("time", minutes(left)),
                        None => state,
                    };
                    Words::key("command.weather.report").with("state", state).with("numbers", weather_numbers(&weather))
                }];
                if !effects.is_empty() {
                    report.push(Words::key("command.weather.effects").with("effects", Words::List(effects)));
                }
                Ok(Words::Lines(report))
            }
            Some("list") => {
                let names = self.content.weather.presets().iter().map(|p| Words::text(p.short_name())).collect();
                Ok(Words::key("command.weather.list").with("kinds", Words::List(names)))
            }
            Some("natural") => {
                if args.len() > 1 {
                    return Err(Words::key("command.error.natural_takes_no_time"));
                }
                Ok(Words::key(if self.weather.release(at, tick) { "command.weather.natural" } else { "command.weather.already_natural" }))
            }
            Some(name) => {
                let preset = self.content.weather.preset(name).ok_or_else(|| Words::key("command.error.no_such_weather").with("kind", name))?;
                // An intensity (a name, or a fraction such as 0.3), then seconds.
                let mut rest = &args[1..];
                let intensity = match rest.first().filter(|w| w.contains('.') || *w == "0" || *w == "1" || w.parse::<f32>().is_err()) {
                    Some(word) => {
                        rest = &rest[1..];
                        parse_intensity(word).ok_or_else(|| Words::key("command.error.not_an_intensity").with("word", word.as_str()))?
                    }
                    None => preset.intensity,
                };
                if rest.len() > 1 {
                    return Err(Words::key("command.error.too_many_words"));
                }
                let seconds = weather_seconds(rest.first())?;
                let how = INTENSITIES.iter().rev().find(|&&(_, v)| intensity >= v - 0.125).map_or("faint", |&(n, _)| n);
                let how = if how == "full" { "max" } else { how };
                let reply = Words::key("command.weather.bringing")
                    .with("intensity", Words::key(&format!("weather.intensity.{how}")))
                    .with("kind", weather_name("kind", preset.short_name()))
                    .with("percent", Words::text(format!("{:.0}", intensity * 100.0)))
                    .with("time", minutes(seconds as u64));
                self.weather.force(preset.at(intensity), at, seconds, tick);
                Ok(reply)
            }
        }
    }

    fn wind_command(&mut self, id: Source, args: &[String]) -> Reply {
        let position = self.position_of(id);
        let at = Vec2::new(position.x, position.z);
        let (tick, day) = (self.tick, self.day);
        match args.first().map(String::as_str) {
            None => {
                let wind = self.weather.local(&mut self.temperature, at, tick, day).weather;
                let key = if wind.wind.length() < 0.05 { "command.wind.still" } else { "command.wind.report" };
                Ok(Words::key(key)
                    .with("speed", Words::text(format!("{:.1}", wind.wind.length())))
                    .with("from", compass(-wind.wind))
                    .with("gusts", Words::text(format!("{:.0}", wind.gusts * 100.0))))
            }
            Some("list") => {
                let names = self.content.weather.winds().iter().map(|w| Words::text(w.short_name())).collect();
                Ok(Words::key("command.wind.list").with("winds", Words::List(names)))
            }
            Some(name) => {
                let mut wind = self.content.weather.wind(name).ok_or_else(|| Words::key("command.error.no_such_wind").with("wind", name))?.clone();
                let mut rest = &args[1..];
                if let Some(from) = rest.first().and_then(|word| direction_from(word)) {
                    wind.direction = Some(-from);
                    rest = &rest[1..];
                }
                if rest.len() > 1 {
                    return Err(Words::key("command.error.too_many_words"));
                }
                let seconds = weather_seconds(rest.first())?;
                let reply = match wind.direction {
                    Some(d) => Words::key("command.wind.bringing_from").with("from", compass(-d)),
                    None => Words::key("command.wind.bringing"),
                }
                .with("wind", weather_name("wind", wind.short_name()))
                .with("time", minutes(seconds as u64));
                self.weather.force_wind(&wind, at, seconds, tick);
                Ok(reply)
            }
        }
    }

    /// The biome the player stands in, and what makes it.
    /// `/summon <creature> [count]`: creatures on the ground a few blocks in front of the
    /// player.
    pub(crate) fn summon(&mut self, id: Source, args: &[String]) -> Reply {
        // `held` last stands them still for a minute from the tick they arrive, as
        // `/creatures hold` would: a hold sent after would find them already moved.
        let (args, held) = match args {
            [rest @ .., last] if last == "held" => (rest, true),
            _ => (args, false),
        };
        let (name, count, at) = match args {
            [name] => (name, "1", None),
            [name, count] => (name, count.as_str(), None),
            [name, count, x, y, z] => (name, count.as_str(), Some([x, y, z])),
            _ => return Err(Words::key("command.summon.what")),
        };
        let count = count.parse::<u32>().ok().filter(|c| (1..=16).contains(c)).ok_or_else(|| Words::key("command.summon.count"))?;
        let species = self.content.creatures.find(name).ok_or_else(|| Words::key("command.summon.unknown").with("name", name.as_str()))?;
        // People come only with their settlement, which keeps them (`villagers`).
        if self.content.creatures.get(species).def.kind == voxmar_engine::creature::Kind::Villager {
            return Err(Words::key("command.summon.settled").with("creature", creature_name(&self.content.creatures.get(species).def)));
        }
        let at = match at {
            Some(words) => self.block_position(id, words)?.as_vec3() + Vec3::new(0.5, 0.0, 0.5),
            None => {
                let (feet, look) = match id {
                    Source::Player(p) => {
                        let player = &self.players[&p];
                        (player.position, player.look().with_y(0.0).normalize_or(Vec3::NEG_Z))
                    }
                    Source::Console => (self.spawn, Vec3::NEG_Z),
                };
                feet + look * 4.0
            }
        };
        let mut placed = 0;
        for i in 0..count {
            // Summoned ones are wild: they leave room for kept animals waiting in the save.
            if self.wild_room() == 0 {
                break;
            }
            let offset = Vec3::new((i % 4) as f32 - 1.5, 0.0, (i / 4) as f32) * 1.2;
            let cell = (at + offset).floor().as_ivec3();
            // Commands may run before the land around a player has loaded (a capture's,
            // just after joining): load the column first, as /drop does.
            self.load_area(cell, cell);
            let Some(spot) = self.standing_spot(cell.x, cell.z, cell.y) else { continue };
            // Each one's looks and mind by the world's count of rolls, never the tick.
            let [a, b] = self.meta.next_cast(crate::save::ROLL_SUMMON);
            let salt = u64::from(a.to_bits()) << 32 | u64::from(b.to_bits());
            if let Some(id) = self.spawn_creature(species, spot.as_vec3() + Vec3::new(0.5, 0.0, 0.5), salt) {
                if held && let Some(creature) = self.creatures.live.get_mut(&id) {
                    creature.held_until = self.tick + 60 * TICKS_PER_SECOND as u64;
                }
                placed += 1;
            }
        }
        let creature = creature_name(&self.content.creatures.get(species).def);
        match placed {
            0 => Err(Words::key("command.summon.no_room").with("creature", creature)),
            n => Ok(Words::key("command.summon.done").with("creature", creature).with("count", Words::text(n))),
        }
    }

    /// `/creatures tame|ready|hasten`: for admins, the creatures within 8 blocks are tamed
    /// by them, readied to breed, or hastened (products due and young grown now).
    /// With a species named, only creatures of it.
    fn creatures_command(&mut self, id: Source, what: &str, species: Option<&str>) -> Reply {
        let player = Self::me(id)?;
        let at = self.position_of(id);
        let name = self.players[&player].name.clone();
        let tick = self.tick;
        let mut count = 0;
        // Bounded natural attempts for inspecting birth positions, without bypassing
        // visibility, darkness or budgets. Only the admitted creatures are held.
        if what == "populate" && species.is_none() {
            let count = self.populate_near(player);
            return Ok(Words::key("command.creatures.populated").with("count", Words::text(count)));
        }
        if what == "disperse" && species.is_none() {
            let wild: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| {
                c.owner.is_none() && self.content.creatures.get(c.species).def.kind != voxmar_engine::creature::Kind::Villager
                    && c.body.position.distance(at) < 64.0
            }).map(|(&id, _)| id).collect();
            for &id in &wild {
                self.creatures.take_away(id);
            }
            return Ok(Words::key("command.creatures.dispersed").with("count", Words::text(wild.len())));
        }
        // One of a settlement's people near makes a bark: `/creatures bark greet`.
        if what == "bark" {
            let (name, made) = species.and_then(|name| Some((name, self.bark_command(at, name)?))).ok_or_else(|| Words::key("command.creatures.bark_what"))?;
            let reach = Words::text(crate::barks::BARK_COMMAND_REACH as u32);
            return Ok(Words::key(if made > 0 { "command.creatures.barked" } else { "command.creatures.no_one_to_bark" }).with("bark", name).with("reach", reach));
        }
        let only = match species {
            Some(species) => Some(self.content.creatures.find(species).ok_or_else(|| Words::key("command.summon.unknown").with("name", species))?),
            None => None,
        };
        let near = |c: &crate::creatures::Live| c.body.position.distance(at) <= 8.0 && only.is_none_or(|s| c.species == s);
        if what == "remove" {
            // Only those the player keeps: a gate step tidies away the animals it summoned.
            let kept: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| c.owner_id == Some(player) && near(c)).map(|(&id, _)| id).collect();
            for &id in &kept {
                self.creatures.take_away(id);
            }
            return Ok(Words::key("command.creatures.done").with("count", Words::text(kept.len())));
        }
        // Killed outright, as a blow would kill them: their death, their drops.
        if what == "kill" {
            // Never another player's kept animals: only wild ones and the admin's own, by who
            // keeps them (an animal whose keeper is away is nobody's here to kill).
            let killed: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| near(c) && (c.owner.is_none() || c.owner_id == Some(player))).map(|(&id, _)| id).collect();
            for &id in &killed {
                self.creature_dies(id);
            }
            return Ok(Words::key("command.creatures.done").with("count", Words::text(killed.len())));
        }
        // Taming here keeps to the keeper's cap as feeding does.
        let kept_each = self.content.creatures.limits().kept_each;
        let mut kept_room = kept_each.saturating_sub(self.kept_by(&name));
        if what == "tame" && kept_room == 0 {
            return Err(Words::key("husbandry.kept_full").with("count", Words::text(kept_each)));
        }
        let (day, tamed_at) = (self.day, self.content.creatures.condition().tamed);
        let content = &self.content;
        let mut tamed = Vec::new();
        for (&id, creature) in self.creatures.live.iter_mut().filter(|(_, c)| near(c)) {
            match what {
                // Taken in as feeding takes one in: newly tamed, just fed, at home here.
                // Never a settlement's people: the settlement keeps them, and one tamed
                // would be saved and come back beside the one it brings out.
                "tame" if creature.owner.is_none() && kept_room > 0 && content.creatures.get(creature.species).def.kind != voxmar_engine::creature::Kind::Villager => {
                    kept_room -= 1;
                    creature.owner = Some(name.clone());
                    creature.owner_id = Some(player);
                    (creature.condition, creature.reckoned, creature.fed) = (tamed_at, day, day);
                    tamed.push(id);
                }
                "ready" if creature.owner.is_some() && creature.grows_up.is_none() => {
                    creature.ready_until = tick + 30 * voxmar_protocol::TICKS_PER_SECOND as u64;
                    creature.breeds_again = 0;
                }
                // Stands still for a minute, side on to whoever asked, for a closer look or
                // a picture.
                "hold" => {
                    creature.held_until = tick + 60 * voxmar_protocol::TICKS_PER_SECOND as u64;
                    let to = (at - creature.body.position).with_y(0.0);
                    let side = voxmar_engine::glam::Vec3::new(to.z, 0.0, -to.x);
                    if side.length_squared() > 1e-4 {
                        creature.body.yaw = voxmar_engine::creature::motion::yaw_of(side);
                    }
                    creature.body.velocity = voxmar_engine::glam::Vec3::ZERO;
                }
                // Let go of a hold now (`hold`, `/summon ... held`): the gate's Climb step lets
                // its wolf go at the foot of the step.
                "release" => creature.held_until = tick,
                "hasten" => {
                    creature.products_due.iter_mut().for_each(|due| *due = tick);
                    if creature.grows_up.is_some() {
                        creature.grows_up = Some(tick);
                    }
                }
                "tame" | "ready" => continue,
                _ => return Err(Words::key("command.creatures.what")),
            }
            creature.changed = true;
            count += 1;
        }
        for id in tamed {
            self.settle(id);
        }
        Ok(Words::key("command.creatures.done").with("count", Words::text(count)))
    }

    /// `/skill <skill> <level>`: sets the player's level in a skill.
    fn skill_command(&mut self, id: Source, args: &[String]) -> Reply {
        let player = Self::me(id)?;
        let [skill, level] = args else { return Err(Words::key("command.skill.what")) };
        let skills = self.content.skills.clone();
        let found = skills.named("core", skill).ok_or_else(|| Words::key("command.skill.unknown").with("name", skill.as_str()))?;
        let level: u8 = level.parse().ok().filter(|l| (1..=skills.max_level()).contains(l)).ok_or_else(|| Words::key("command.skill.level"))?;
        let target = skills.xp_for(level);
        let p = self.players.get_mut(&player).expect("a player");
        let have = p.skills.xp(found);
        if have < target {
            p.skills.grant(found, target - have);
        } else {
            p.skills.0[found.0 as usize] = target;
        }
        p.changed(voxmar_engine::traits::SKILLS);
        Ok(Words::key("command.skill.done").with("level", Words::text(level)))
    }

    /// `/quest <quest>`: marks a quest done for the player, by its full name or its own
    /// (`the_farm_age`); what it pays waits to be claimed.
    fn quest_command(&mut self, id: Source, args: &[String]) -> Reply {
        let player = Self::me(id)?;
        let [name] = args else { return Err(Words::key("command.quest.what")) };
        let quests = self.content.quests.clone();
        let quest = quests.iter().find(|q| q.name == *name || q.name.rsplit(':').next() == Some(name.as_str())).ok_or_else(|| Words::key("command.quest.unknown").with("name", name.as_str()))?;
        let title = Words::Own(voxmar_engine::lang::def_key("quest", &quest.name), quest.title.clone());
        if self.players[&player].quests_done.contains(&quest.name) {
            return Err(Words::key("command.quest.already").with("quest", title));
        }
        self.finish_quest(player, &quest.name);
        // Told at once, as a claim is: the stations its rite opens show open straight away.
        self.update_quests(player);
        Ok(Words::key("command.quest.done").with("quest", title))
    }

    /// `/creatures`: the creatures within 64 blocks.
    fn creatures_about(&self, id: Source) -> Words {
        let at = self.position_of(id);
        let mut counts: std::collections::BTreeMap<String, (usize, f32, &'static str, Words)> = Default::default();
        for creature in self.creatures.live.values() {
            let distance = creature.body.position.distance(at);
            if distance > 64.0 {
                continue;
            }
            let species = self.content.creatures.get(creature.species);
            let doing = creature.mind.doing.map_or("resting", |i| species.modules[i].0.name());
            let entry = counts.entry(species.def.name.clone()).or_insert((0, f32::MAX, doing, creature_name(&species.def)));
            entry.0 += 1;
            if distance < entry.1 {
                entry.1 = distance;
                entry.2 = doing;
            }
        }
        if counts.is_empty() {
            return Words::key("command.creatures.none");
        }
        let lines = counts.into_values().map(|(n, d, doing, creature)| {
            Words::key("command.creatures.line")
                .with("count", Words::text(n))
                .with("creature", creature)
                .with("distance", Words::text(format!("{d:.0}")))
                .with("doing", Words::Own(format!("creature.doing.{doing}"), doing.replace('_', " ")))
        });
        Words::Lines(std::iter::once(Words::key("command.creatures.title")).chain(lines).collect())
    }

    /// `/home`: where the player's home is (the lit hearth, else the bed last slept in), the
    /// highest step of each chain it has reached (the ladder, the rooms) and what the next
    /// one lacks, judged as if the player stood in it.
    fn home_command(&self, id: Source) -> Reply {
        let player = self.players.get(&Self::me(id)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        let Some(home) = player.home_spot(&self.lit) else { return Err(Words::key("home.none")) };
        let anchor = home.floor().as_ivec3();
        let survey = self.survey_around(anchor, Some(Self::me(id)?));
        let calm = voxmar_engine::weather::Weather::NONE;
        let situation = voxmar_engine::effect::Situation { felt: 0.0, warmed: false, outdoors: self.open_sky(anchor + IVec3::Y * 2), weather: &calm, biome: "", biome_tags: &[], home: Some(&survey), done: &player.quests_done, held: &[], near: &[], worn: &[], seated: false, night: self.night() };
        let effects = &self.content.effects;
        let name = |effect| {
            let def = effects.get(effect);
            Words::Own(voxmar_engine::lang::def_key("effect", &def.name), def.title.clone())
        };
        let away = player.position.distance(home).round();
        let mut lines = vec![Words::key("home.at").with("x", Words::text(anchor.x)).with("y", Words::text(anchor.y)).with("z", Words::text(anchor.z)).with("away", Words::text(away))];
        // Out of the loaded world its blocks read as nothing: say so rather than misjudge it.
        if self.block(anchor).is_none() {
            lines.push(Words::key("home.far"));
            return Ok(Words::Lines(lines));
        }
        for chain in effects.home_progress(&situation) {
            lines.push(match (chain.reached, chain.next) {
                (Some(reached), Some((next, lacks))) => Words::key("home.step").with("step", name(reached)).with("next", name(next)).with("needs", home_needs(&lacks)),
                (Some(reached), None) => Words::key("home.step_last").with("step", name(reached)),
                (None, Some((next, lacks))) => Words::key("home.step_first").with("next", name(next)).with("needs", home_needs(&lacks)),
                (None, None) => continue,
            });
        }
        Ok(Words::Lines(lines))
    }

    fn biome(&self, id: Source) -> Words {
        let at = self.position_of(id).floor().as_ivec3();
        let site = self.overworld.site(at.x, at.z);
        let biome = self.overworld.biome_at(at.x, at.z);
        let rarity = format!("{:?}", biome.rarity);
        let rarity = Words::Own(format!("rarity.{}", snake(&rarity)), rarity);
        Words::key("command.biome")
            .with("biome", Words::Own(voxmar_engine::lang::def_key("biome", &biome.name), biome.name.clone()))
            .with("tags", Words::List(biome.tags.iter().map(|tag| Words::Own(format!("biome.tag.{tag}"), tag.replace('_', " "))).collect()))
            .with("rarity", rarity)
            .with("temperature", Words::text(format!("{:.1}", site.temperature)))
            .with("rain", Words::text(format!("{:.0}", site.precipitation)))
            .with("aether", Words::text(format!("{:.2}", site.aether)))
    }

    /// `/locate biome NAME [RADIUS]`: the nearest place of a biome. `/locate gorge [RADIUS]`:
    /// the nearest river running deep in a slot gorge. `/locate ore [NAME] [RADIUS]`: the
    /// nearest sample of ore on the ground over a vein (of an ore or vein named so).
    fn locate(&self, id: Source, args: &[String]) -> Reply {
        let radius = |radius: &String| -> Result<i32, Words> {
            radius
                .parse()
                .ok()
                .filter(|r| (1..=MAX_LOCATE_RADIUS).contains(r))
                .ok_or_else(|| Words::key("command.error.radius").with("max", Words::text(MAX_LOCATE_RADIUS)))
        };
        let (name, radius) = match args {
            [kind] if kind == "gorge" => return self.locate_gorge(id, LOCATE_RADIUS),
            [kind, r] if kind == "gorge" => return self.locate_gorge(id, radius(r)?),
            [kind] if kind == "ore" => return self.locate_ore(id, None, ORE_RADIUS),
            [kind, r] if kind == "ore" && r.parse::<i32>().is_ok() => return self.locate_ore(id, None, radius(r)?),
            [kind, name] if kind == "ore" => return self.locate_ore(id, Some(name), ORE_RADIUS),
            [kind, name, r] if kind == "ore" => return self.locate_ore(id, Some(name), radius(r)?),
            [kind] if kind == "settlement" => return self.locate_settlement(id, None, LOCATE_RADIUS),
            [kind, r] if kind == "settlement" && r.parse::<i32>().is_ok() => return self.locate_settlement(id, None, radius(r)?),
            [kind, size] if kind == "settlement" => return self.locate_settlement(id, Some(size), LOCATE_RADIUS),
            [kind, size, r] if kind == "settlement" => return self.locate_settlement(id, Some(size), radius(r)?),
            [kind, name] if kind == "biome" => (name, LOCATE_RADIUS),
            [kind, name, r] if kind == "biome" => (name, radius(r)?),
            _ => return Err(Words::key("command.error.locate_what")),
        };
        let Some(biome) = self.overworld.biomes().id(name).or_else(|| self.overworld.biomes().id(&format!("base:{name}"))) else {
            return Err(Words::key("command.error.no_such_biome").with("biome", name.as_str()));
        };
        let full = self.overworld.biomes().get(biome).name.clone();
        let biome_name = Words::Own(voxmar_engine::lang::def_key("biome", &full), name.clone());
        let position = self.position_of(id);
        let from = IVec2::new(position.x.floor() as i32, position.z.floor() as i32);
        match self.overworld.locate_biome(name, from, radius) {
            Some(at) => {
                let offset = (at - from).as_vec2();
                Ok(Words::key(if offset.length() < 1.0 { "command.locate.here" } else { "command.locate.biome" })
                    .with("biome", biome_name)
                    .with("place", format!("{} {}", at.x, at.y))
                    .with("distance", Words::text(format!("{:.0}", offset.length())))
                    .with("way", compass(offset))
                    .with("tp", format!("/tp {} {}", at.x, at.y)))
            }
            None => Err(Words::key("command.error.biome_not_near").with("biome", biome_name).with("radius", Words::text(radius))),
        }
    }

    fn locate_gorge(&self, id: Source, radius: i32) -> Reply {
        let position = self.position_of(id);
        let from = IVec2::new(position.x.floor() as i32, position.z.floor() as i32);
        match self.overworld.locate_gorge(from, radius) {
            Some(at) => {
                let offset = (IVec2::new(at.x, at.z) - from).as_vec2();
                Ok(Words::key("command.locate.gorge")
                    .with("place", format!("{} {}", at.x, at.z))
                    .with("water", Words::text(at.y))
                    .with("distance", Words::text(format!("{:.0}", offset.length())))
                    .with("way", compass(offset))
                    .with("tp", format!("/tp {} {} {}", at.x, at.y + 1, at.z)))
            }
            None => Err(Words::key("command.error.gorge_not_near").with("radius", Words::text(radius))),
        }
    }

    fn locate_settlement(&self, id: Source, size: Option<&String>, radius: i32) -> Reply {
        let sizes = self.overworld.settlement_sizes();
        if let Some(size) = size.filter(|size| !sizes.contains(size)) {
            return Err(Words::key("command.error.no_such_size").with("size", size.as_str()).with("sizes", Words::List(sizes.iter().map(|s| Words::key(&format!("settlement.size.{s}"))).collect())));
        }
        let position = self.position_of(id);
        let from = IVec2::new(position.x.floor() as i32, position.z.floor() as i32);
        // Nearest first, planning only the squares nearer than the answer: every square
        // within a radius of thousands planned at once held the server's tick for seconds.
        let nearest = self.overworld.nearest_settlement(from, radius, |s| size.is_none_or(|size| *size == s.size)).map(|s| (IVec2::new(s.centre.x, s.centre.z) - from, s));
        match nearest {
            Some((offset, s)) => Ok(Words::key("command.locate.settlement")
                .with("name", Words::Name(vec![("en".into(), s.name.0.clone()), ("ja".into(), s.name.1.clone())]))
                .with("culture", Words::Name(vec![("en".into(), s.culture.title.0.clone()), ("ja".into(), s.culture.title.1.clone())]))
                .with("size", Words::key(&format!("settlement.size.{}", s.size)))
                .with("place", format!("{} {} {}", s.centre.x, s.centre.y, s.centre.z))
                .with("distance", Words::text(format!("{:.0}", offset.as_vec2().length())))
                .with("way", compass(offset.as_vec2()))
                .with("tp", format!("/tp {} {} {}", s.centre.x, s.centre.y + 1, s.centre.z))),
            None => Err(Words::key("command.error.settlement_not_near").with("radius", Words::text(radius))),
        }
    }

    fn locate_ore(&self, id: Source, name: Option<&str>, radius: i32) -> Reply {
        let position = self.position_of(id);
        let from = IVec2::new(position.x.floor() as i32, position.z.floor() as i32);
        match self.overworld.locate_ore(name, from, radius) {
            Some((at, block)) => {
                let offset = (IVec2::new(at.x, at.z) - from).as_vec2();
                Ok(Words::key("command.locate.ore")
                    .with("block", Words::Block(self.content.blocks.def(block).name.clone()))
                    .with("place", format!("{} {} {}", at.x, at.y, at.z))
                    .with("distance", Words::text(format!("{:.0}", offset.length())))
                    .with("way", compass(offset))
                    .with("tp", format!("/tp {} {} {}", at.x, at.y, at.z)))
            }
            None => Err(Words::key("command.error.ore_not_near").with("radius", Words::text(radius))),
        }
    }

    /// The online player called `name`, ignoring case if no name matches exactly.
    /// `/machine <x> <y> <z>`: how the machine there is doing.
    fn machine_command(&mut self, source: Source, args: &[String]) -> Reply {
        let [x, y, z] = args else { return Err(Words::key("command.error.which_block_where")) };
        let at = self.block_position(source, [x, y, z])?;
        self.machine_status(at).ok_or_else(|| Words::key("command.machine.none").with("place", format!("{} {} {}", at.x, at.y, at.z)))
    }

    fn set_block_command(&mut self, source: Source, args: &[String]) -> Reply {
        let [x, y, z, name] = args else { return Err(Words::key("command.error.which_block_where")) };
        let at = self.block_position(source, [x, y, z])?;
        let block = self.find_block(name)?;
        if !(0..WORLD_HEIGHT).contains(&at.y) {
            return Err(Words::key("command.error.outside_world").with("y", Words::text(at.y)).with("top", Words::text(WORLD_HEIGHT - 1)));
        }
        self.load_area(at, at);
        if self.would_trap_a_player(at, block) {
            return Err(Words::key("command.error.player_in_the_way").with("place", format!("{} {} {}", at.x, at.y, at.z)));
        }
        self.set_blocks(&[(at, block)]);
        let block = Words::Block(self.content.blocks.def(block).name.clone());
        Ok(Words::key("command.setblock").with("block", block).with("place", format!("{} {} {}", at.x, at.y, at.z)))
    }

    fn fill(&mut self, source: Source, args: &[String]) -> Reply {
        let [x1, y1, z1, x2, y2, z2, name] = args else { return Err(Words::key("command.error.which_box")) };
        let (a, b) = (self.block_position(source, [x1, y1, z1])?, self.block_position(source, [x2, y2, z2])?);
        let block = self.find_block(name)?;
        let (min, max) = (a.min(b).with_y(a.y.min(b.y).max(0)), a.max(b).with_y(a.y.max(b.y).min(WORLD_HEIGHT - 1)));
        let size = (max - min + 1).max(IVec3::ZERO);
        if size.x as i64 * size.y as i64 * size.z as i64 > MAX_FILL || size.x.max(size.z) > MAX_FILL_SIDE {
            return Err(Words::key("command.error.box_too_big").with("max", Words::text(MAX_FILL)).with("side", Words::text(MAX_FILL_SIDE)));
        }
        self.load_area(min, max);
        let mut changes = Vec::new();
        // A solid block never goes where a player stands (as no hand could place it there).
        let mut left = 0;
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    let at = IVec3::new(x, y, z);
                    if self.block(at).is_some_and(|b| b != block) {
                        if self.would_trap_a_player(at, block) {
                            left += 1;
                        } else {
                            changes.push((at, block));
                        }
                    }
                }
            }
        }
        self.set_blocks(&changes);
        let block = Words::Block(self.content.blocks.def(block).name.clone());
        let filled = Words::key("command.fill").with("count", Words::text(changes.len())).with("block", block);
        if left > 0 { Ok(Words::key("command.fill.around_players").with("filled", filled).with("left", Words::text(left))) } else { Ok(filled) }
    }

    fn give(&mut self, source: Source, args: &[String]) -> Reply {
        let (name, count) = match args {
            [name] => (name, 64),
            [name, count] => (
                name,
                count
                    .parse::<u16>()
                    .ok()
                    .filter(|c| (1..=2304).contains(c))
                    .ok_or_else(|| Words::key("command.error.not_a_count").with("word", count.as_str()).with("max", Words::text(2304)))?,
            ),
            _ => return Err(Words::key("command.error.give_what")),
        };
        // A wild plant whose item merged is given as that item.
        let block = self.content.blocks.item_of(self.find_block(name)?);
        if block.is_air() {
            return Err(Words::key("command.error.air_held"));
        }
        let player = self.players.get_mut(&Self::me(source)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        let mut left = count;
        while left > 0 {
            let batch = left.min(u8::MAX as u16) as u8;
            let spilled = player.inventory.add(block, batch);
            left -= (batch - spilled) as u16;
            if spilled > 0 {
                break;
            }
        }
        player.changed(voxmar_engine::traits::INVENTORY);
        // What does not fit drops at the player's feet, as Minecraft's /give does.
        let feet = player.position;
        let mut spilled = left;
        while spilled > 0 {
            let count = spilled.min(MAX_STACK as u16) as u8;
            let body = voxmar_engine::dropped::Loose::new(feet, Vec3::ZERO);
            self.items.spawn(ItemStack::new(block, count), body, 0.0);
            spilled -= count as u16;
        }
        let gave = Words::key("command.give").with("count", Words::text(count)).with("block", Words::Block(self.content.blocks.def(block).name.clone()));
        if left > 0 { Ok(Words::key("command.give.spilled").with("gave", gave).with("left", Words::text(left))) } else { Ok(gave) }
    }

    /// `/wear <uses left> [hotbar slot]`: the tool in hand (or in that hotbar slot, counted
    /// from 1) worn down (or mended) to that many uses left, or what burns there (a
    /// firebrand) to that many seconds. Naming the slot needs no wait for the server to hear
    /// which slot is in hand.
    pub(crate) fn wear_command(&mut self, source: Source, args: &[String]) -> Reply {
        let player = self.players.get_mut(&Self::me(source)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        let (left, slot) = match args {
            [left] => (left, player.selected),
            [left, slot] => {
                let most = voxmar_engine::inventory::HOTBAR_SLOTS;
                let slot = slot
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=most).contains(n))
                    .ok_or_else(|| Words::key("command.error.not_a_count").with("word", slot.as_str()).with("max", Words::text(most)))?;
                (left, slot - 1)
            }
            _ => return Err(Words::key("command.error.wear_what")),
        };
        let held = player.inventory.get(slot);
        let uses = held.map_or(0, |stack| self.content.tools.life(stack.item));
        let (Some(mut stack), true) = (held, uses > 0) else { return Err(Words::key("command.error.wear_nothing")) };
        let left = left
            .parse::<u16>()
            .ok()
            .filter(|n| (1..=uses).contains(n))
            .ok_or_else(|| Words::key("command.error.not_a_count").with("word", left.as_str()).with("max", Words::text(uses)))?;
        stack.wear = uses - left;
        player.inventory.slots[slot] = Some(stack);
        player.changed(voxmar_engine::traits::INVENTORY);
        Ok(Words::key("command.wear").with("block", Words::Block(self.content.blocks.def(stack.item).name.clone())).with("left", Words::text(left)).with("uses", Words::text(uses)))
    }

    fn clear(&mut self, source: Source, args: &[String]) -> Reply {
        let item = match args {
            [] => None,
            [name] => Some(self.content.blocks.item_of(self.find_block(name)?)),
            _ => return Err(Words::key("command.error.clear_what")),
        };
        let player = self.players.get_mut(&Self::me(source)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        let count = player.inventory.clear(|held| item.is_none_or(|item| held == item));
        player.changed(voxmar_engine::traits::INVENTORY);
        Ok(Words::key("command.clear").with("count", Words::text(count)))
    }

    fn effect_command(&mut self, source: Source, args: &[String]) -> Reply {
        let effects = self.content.effects.clone();
        let player = self.players.get_mut(&Self::me(source)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        let (name, seconds) = match args {
            [word] if word == "clear" => {
                player.timed_effects.clear();
                return Ok(Words::key("command.effect.cleared"));
            }
            [name] => (name, 60.0),
            [name, seconds] => (name, seconds.parse::<f32>().ok().filter(|s| (1.0..=86_400.0).contains(s)).ok_or_else(|| Words::key("command.error.not_seconds").with("word", seconds.as_str()).with("max", Words::text(86_400)))?),
            _ => return Err(Words::key("command.error.effect_what")),
        };
        let id = effects.find(name).ok_or_else(|| Words::key("command.error.no_such_effect").with("effect", name.as_str()))?;
        player.give_effects(&[(id, seconds)]);
        Ok(Words::key("command.effect").with("effect", Words::Own(voxmar_engine::lang::def_key("effect", &effects.get(id).name), effects.get(id).title.clone())).with("seconds", Words::text(seconds)))
    }

    /// Makes a player an admin (`/op`), or no longer one (`/deop`), until they leave. One who
    /// is an admin by right is one again when they come back.
    fn op_command(&mut self, args: &[String], admin: bool) -> Reply {
        let [name] = args else { return Err(Words::key(if admin { "command.error.op_whom" } else { "command.error.deop_whom" })) };
        let who = self.find_player(name)?;
        let player = self.players.get_mut(&who).unwrap();
        player.admin = admin;
        if !admin {
            player.fly_speed = 1.0;
        }
        player.connection.send(ServerMessage::Abilities { allow_flight: player.allow_flight, admin });
        let name = player.name.clone();
        let (theirs, yours) = match (admin, player.admin_by_right) {
            (true, _) => ("command.op", "command.op.you"),
            (false, false) => ("command.deop", "command.deop.you"),
            (false, true) => ("command.deop.until_rejoin", "command.deop.you.until_rejoin"),
        };
        self.tell(Source::Player(who), ChatKind::Info, Words::key(yours));
        Ok(Words::key(theirs).with("name", name))
    }

    fn kill_command(&mut self, source: Source) -> Reply {
        let player = self.players.get_mut(&Self::me(source)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        if player.health.damage(u8::MAX) {
            player.changed(voxmar_engine::traits::HEALTH);
            player.connection.send(ServerMessage::Died);
        }
        Ok(Words::key("command.kill"))
    }

    fn heal_command(&mut self, source: Source) -> Reply {
        let player = self.players.get_mut(&Self::me(source)?).ok_or_else(|| Words::key("command.error.nobody_to_give_to"))?;
        if !player.health.alive() {
            return Err(Words::key("command.heal.dead"));
        }
        player.health.current = player.health.max;
        player.changed(voxmar_engine::traits::HEALTH);
        Ok(Words::key("command.heal"))
    }

    fn grow_command(&mut self, source: Source, args: &[String]) -> Reply {
        let stages = match args {
            [] => 1,
            [word] if word == "ripe" => u8::MAX,
            [count] => count.parse::<u8>().ok().filter(|&c| c > 0).ok_or_else(|| Words::key("command.error.not_stages").with("word", count.as_str()))?,
            _ => return Err(Words::key("command.error.grow_how_much")),
        };
        let at = self.position_of(source);
        let grown = self.grow_around(at, 8.0, stages) + self.hurry_regrowth(at, 8.0);
        Ok(Words::key("command.grow").with("count", Words::text(grown)))
    }

    fn drop_command(&mut self, source: Source, args: &[String]) -> Reply {
        let (name, rest) = args.split_first().ok_or_else(|| Words::key("command.error.drop_what"))?;
        let (count, at) = match rest {
            [] => ("1", None),
            [count] => (count.as_str(), None),
            [count, x, y, z] => (count.as_str(), Some([x, y, z])),
            _ => return Err(Words::key("command.error.drop_count_then_place")),
        };
        let count = count
            .parse::<u8>()
            .ok()
            .filter(|c| (1..=MAX_STACK).contains(c))
            .ok_or_else(|| Words::key("command.error.not_a_count").with("word", count).with("max", Words::text(MAX_STACK)))?;
        let block = self.content.blocks.item_of(self.find_block(name)?);
        if block.is_air() {
            return Err(Words::key("command.error.air_dropped"));
        }
        let position = match at {
            Some(words) => {
                let cell = self.block_position(source, words)?;
                self.load_area(cell, cell);
                cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5)
            }
            None => self.position_of(source),
        };
        let body = voxmar_engine::dropped::Loose::new(position, Vec3::ZERO);
        self.items.spawn(ItemStack::new(block, count), body, voxmar_engine::dropped::PICKUP_DELAY);
        Ok(Words::key("command.drop")
            .with("count", Words::text(count))
            .with("block", Words::Block(self.content.blocks.def(block).name.clone()))
            .with("place", format!("{:.1} {:.1} {:.1}", position.x, position.y, position.z)))
    }

    /// Loads the columns under a box, so commands can build anywhere, not only where
    /// players have been.
    pub(crate) fn load_area(&mut self, min: IVec3, max: IVec3) {
        let (from, to) = (ColumnPos::of_block(min), ColumnPos::of_block(max));
        for z in from.0.y..=to.0.y {
            for x in from.0.x..=to.0.x {
                let pos = ColumnPos::new(x, z);
                if !self.world.contains(pos) {
                    let today = wear::day(self.day);
                    let loaded = load_column(self.save.as_ref(), &*self.generator, &self.content.blocks, &self.wear_rules, today, pos);
                    self.insert_column(pos, loaded);
                }
            }
        }
    }

    /// A block position typed as three coordinates: numbers, or `~` (where the player's
    /// feet are, or spawn for the console) plus an optional offset.
    fn block_position(&self, source: Source, words: [&String; 3]) -> Result<IVec3, Words> {
        let feet = self.position_of(source).floor().as_ivec3();
        let mut at = IVec3::ZERO;
        for (axis, word) in words.into_iter().enumerate() {
            let error = || Words::key("command.error.not_a_block_coordinate").with("word", word.as_str());
            at[axis] = match word.strip_prefix('~') {
                Some("") => feet[axis],
                Some(offset) => feet[axis] + offset.parse::<i32>().map_err(|_| error())?,
                None => word.parse::<i32>().map_err(|_| error())?,
            };
        }
        if at.x.abs().max(at.z.abs()) as f32 > MAX_COORDINATE {
            return Err(Words::key("command.error.too_far").with("place", format!("{} {} {}", at.x, at.y, at.z)));
        }
        Ok(at)
    }

    /// A block by its full name (`base:water`) or, when only one package has it, its short
    /// name (`water`); `air` is air.
    fn find_block(&self, name: &str) -> Result<voxmar_engine::block::BlockId, Words> {
        let blocks = &self.content.blocks;
        if let Some(block) = blocks.id(name) {
            return Ok(block);
        }
        let suffix = format!(":{name}");
        let mut found = (0..blocks.len()).map(|i| voxmar_engine::block::BlockId(i as u16)).filter(|&b| blocks.def(b).name.ends_with(&suffix));
        match (found.next(), found.next()) {
            (Some(block), None) => Ok(block),
            (Some(_), Some(_)) => Err(Words::key("command.error.ambiguous_block").with("block", name)),
            _ => Err(Words::key("command.error.no_such_block").with("block", name)),
        }
    }

    fn find_player(&self, name: &str) -> Result<PlayerId, Words> {
        let joined = || self.players.values().filter(|p| p.joined);
        if let Some(player) = joined().find(|p| p.name == name) {
            return Ok(player.id);
        }
        let mut similar = joined().filter(|p| p.name.eq_ignore_ascii_case(name));
        match (similar.next(), similar.next()) {
            (Some(player), None) => Ok(player.id),
            _ => Err(Words::key("command.error.no_such_player").with("name", name)),
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
fn axis(word: &str, current: f32, horizontal: bool) -> Result<f32, Words> {
    let error = || Words::key("command.error.not_a_coordinate").with("word", word);
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

/// The compass point nearest a direction (x, z): `north_east`, the end of its key.
fn compass_point(direction: Vec2) -> &'static str {
    const POINTS: [&str; 8] = ["north", "north_east", "east", "south_east", "south", "south_west", "west", "north_west"];
    let degrees = direction.x.atan2(-direction.y).to_degrees().rem_euclid(360.0);
    POINTS[((degrees + 22.5) / 45.0) as usize % 8]
}

/// The compass point nearest a direction, in words.
pub(crate) fn compass(direction: Vec2) -> Words {
    Words::key(&format!("compass.{}", compass_point(direction)))
}

/// `5 minutes`, `90 seconds`.
fn minutes(seconds: u64) -> Words {
    match seconds {
        s if s < 120 => Words::key("time.seconds").with("count", Words::text(s)),
        s if s % 60 == 0 => Words::key("time.minutes").with("count", Words::text(s / 60)),
        s => Words::key("time.minutes_part").with("minutes", Words::text(format!("{:.1}", s as f32 / 60.0))),
    }
}

/// A command's help, in the player's language where it has one.
fn help(def: &CommandDef) -> Words {
    def.help_words()
}

/// A weather name (`kind`, `effect`, `wind` or `channel`) in words, or as named.
pub fn weather_name(what: &str, name: &str) -> Words {
    Words::Own(format!("weather.{what}.{name}"), name.replace('_', " "))
}

/// `name 40%`.
fn share(name: Words, value: f32) -> Words {
    Words::key("command.weather.share").with("name", name).with("percent", Words::text(format!("{:.0}", value * 100.0)))
}

/// The numbers behind the weather: temperature, cloud, rain, wind and what else shows.
pub fn weather_numbers(weather: &voxmar_engine::weather::Weather) -> Words {
    let mut numbers = Words::key("command.weather.numbers")
        .with("temperature", Words::text(format!("{:.1}", weather.temperature)))
        .with("cloud", Words::text(format!("{:.0}", weather.cover * 100.0)))
        .with("heaped", Words::text(format!("{:.0}", weather.convection * 100.0)))
        .with("cirrus", Words::text(format!("{:.0}", weather.cirrus * 100.0)))
        .with("humidity", Words::text(format!("{:.0}", weather.humidity * 100.0)))
        .with("falling", Words::key(if weather.snow > 0.5 { "weather.channel.snow" } else { "weather.channel.rain" }))
        .with("precipitation", Words::text(format!("{:.0}", weather.precipitation * 100.0)))
        .with("wind", Words::text(format!("{:.1}", weather.wind.length())))
        .with("visibility", Words::text(format!("{:.0}", weather.visibility())));
    let channels: Vec<Words> = weather.channels().filter(|&(_, value)| value >= 0.05).map(|(name, value)| share(weather_name("channel", name), value)).collect();
    if !channels.is_empty() {
        numbers = Words::key("command.weather.numbers_and").with("numbers", numbers).with("channels", Words::List(channels));
    }
    numbers
}

/// `VeryRare` as `very_rare`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
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
fn weather_seconds(seconds: Option<&String>) -> Result<u32, Words> {
    let Some(word) = seconds else { return Ok(WEATHER_SECONDS) };
    match word.parse::<u32>() {
        Ok(seconds) if (1..=MAX_WEATHER_SECONDS).contains(&seconds) => Ok(seconds),
        _ => Err(Words::key("command.error.not_seconds").with("word", word.as_str()).with("max", Words::text(MAX_WEATHER_SECONDS))),
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
        assert_eq!(compass_point(north), "north");
        assert_eq!(compass_point(direction_from("sw").unwrap()), "south_west");
        assert_eq!(compass_point(direction_from("100").unwrap()), "east");
        assert!(direction_from("up").is_none());
    }

    /// A solid block never goes where a player stands, by `/fill` or `/setblock` (no hand
    /// could place it there); the rest of the box fills, and air still clears round them.
    #[test]
    fn admin_fills_never_shut_a_player_in() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let id = 1;
        let mut player = crate::player::Player::new(id, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.admin, player.name) = (true, true, "test".into());
        server.players.insert(id, player);
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at - IVec3::new(8, 0, 8), at + IVec3::new(8, 0, 8));
        let told = || {
            let mut keys = Vec::new();
            while let Ok(Some(message)) = client.try_recv() {
                // A refused command says why inside its usage line.
                if let voxmar_protocol::ServerMessage::Chat { text: Words::Key(key, args), .. } = message {
                    let why = args.into_iter().find_map(|(name, why)| match why {
                        Words::Key(why, _) if name == "why" => Some(why),
                        _ => None,
                    });
                    keys.push(why.unwrap_or(key));
                }
            }
            keys
        };
        // Off a cell's middle, the body takes up four columns.
        server.players.get_mut(&id).unwrap().position = at.as_vec3() + Vec3::new(0.1, 0.0, 0.1);
        let body = server.players[&id].aabb();
        let stone = server.content.blocks.id("base:stone").unwrap();
        server.command(id, "fill ~-2 ~ ~-2 ~2 ~2 ~2 air");
        told();
        server.command(id, "fill ~-2 ~ ~-2 ~2 ~2 ~2 stone");
        assert_eq!(told(), ["command.fill.around_players"]);
        let mut walled = 0;
        for y in at.y..=at.y + 2 {
            for z in at.z - 2..=at.z + 2 {
                for x in at.x - 2..=at.x + 2 {
                    let cell = IVec3::new(x, y, z);
                    let inside = body.intersects(|c| c == cell);
                    assert_eq!(server.block(cell) == Some(stone), !inside, "{cell}: stone {}, in the player {inside}", server.block(cell) == Some(stone));
                    walled += !inside as usize;
                }
            }
        }
        assert!(walled > 0 && walled < 75, "{walled} of 75 cells filled round the player");
        server.command(id, "setblock ~ ~1 ~ stone");
        assert_eq!(told(), ["command.error.player_in_the_way"]);
        assert!(!server.is_solid(at + IVec3::Y));
        // Air goes anywhere.
        server.command(id, "fill ~-2 ~ ~-2 ~2 ~2 ~2 air");
        assert_eq!(told(), ["command.fill"]);
        assert!(server.block(at + IVec3::new(2, 0, 2)).is_some_and(|b| b.is_air()));
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

    /// What a summoned creature is like (its seed, so its looks, genes and mind) is drawn by
    /// the world's count of rolls, never the tick it is summoned on.
    #[test]
    fn a_summon_draws_by_the_worlds_count_not_the_tick() {
        let summoned = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            server.tick = tick;
            server.summon(Source::Console, &["base:rabbit".to_owned(), "4".to_owned()]).expect("summoned");
            let mut made: Vec<_> = server.creatures.live.iter().map(|(&id, c)| (id, c.seed, c.genome.clone())).collect();
            made.sort_by_key(|m| m.0);
            (made, server.meta.rolls)
        };
        let first = summoned(1_000);
        assert_eq!((first.0.len(), first.1), (4, 4), "four summoned, each counted on");
        assert!(first.0.windows(2).all(|w| w[0].1 != w[1].1), "two summoned alike: {:?}", first.0);
        assert_eq!(summoned(1_377), first, "the same summons later came otherwise");
    }
}

/// What a home lacks, in words: "roof overhead, seat 1/2".
fn home_needs(lacks: &[voxmar_engine::effect::Lack]) -> Words {
    use voxmar_engine::effect::Lack;
    Words::List(
        lacks
            .iter()
            .map(|lack| match lack {
                Lack::Roof => Words::key("home.lack.roof"),
                Lack::Sky => Words::key("home.lack.sky"),
                Lack::Away => Words::key("home.lack.away"),
                Lack::Tag { tag, have, need } => Words::key("home.lack.tag").with("tag", Words::Own(format!("home.tag.{tag}"), tag.replace('_', " "))).with("have", Words::text(have)).with("need", Words::text(need)),
                Lack::Walls { shelter, have, need } => Words::key("home.lack.walls").with("shelter", Words::key(&format!("home.shelter.{}", shelter.name()))).with("have", Words::text(have)).with("need", Words::text(need)),
                Lack::OneOf(options) => Words::key("home.lack.either").with("options", Words::List(options.iter().map(|o| home_needs(o)).collect())),
            })
            .collect(),
    )
}
