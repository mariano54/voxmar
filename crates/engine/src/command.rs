//! Commands: lines typed into the console, such as `/time set noon`.
//!
//! A command is a definition like any other: a name, who may use it, a usage line, help
//! text and what it runs. What it runs is either one of the engine's built-in commands, a
//! small fixed vocabulary in the way actions compile down to a fixed set of effects, or an
//! alias: other command lines with this command's arguments filled in. Aliases are how a
//! package adds commands without native code (`/day` is `/time set day`), and new
//! built-ins come with the engine, or later with WASM.
//!
//! Commands follow the same path as actions: the client proposes a line, and the server
//! parses it, checks the player's permission and the arguments, then applies the effect
//! and replies.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::RegistryError;

/// Who may run a command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    #[default]
    Anyone,
    /// Server admins: the singleplayer host, and players named with `--admin`.
    Admin,
}

/// What a command runs: one of the engine's built-in commands, or other command lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Run {
    /// Lists the commands the player may use, or explains one.
    Help,
    /// Lists who is online.
    List,
    /// Tells the world seed.
    Seed,
    /// Shows, sets, advances, speeds up or freezes the time of day.
    Time,
    /// Forces the weather around the player for a while, or tells what it is.
    Weather,
    /// Forces the wind around the player for a while, or tells what it is.
    Wind,
    /// Moves a player to coordinates or to another player.
    Teleport,
    /// Switches a player between survival and creative.
    GameMode,
    /// Puts a block at a position.
    SetBlock,
    /// Fills a box with a block.
    Fill,
    /// Gives the player blocks.
    Give,
    /// Empties the player's inventory, or takes an item out of it.
    Clear,
    /// Gives the player a timed effect, or clears them.
    Effect,
    /// Kills the player.
    Kill,
    /// Heals the player to full health.
    Heal,
    /// Makes an online player an admin, until `Deop`.
    Op,
    /// Takes an online player's admin rights away.
    Deop,
    /// Drops a stack of items into the world.
    Drop,
    /// Grows the crops around the player.
    Grow,
    /// Names the biome the player stands in.
    Biome,
    /// Finds the nearest place of a kind, such as a biome.
    Locate,
    /// Puts creatures of a species beside the player.
    Summon,
    /// Lists the creatures about the player, or (admins) tames, readies or hastens them.
    Creatures,
    /// Sets a player's level in a skill.
    Skill,
    /// Marks a quest done for the player, its rewards waiting to be claimed.
    Quest,
    /// Tells how the machine at a block is doing.
    Machine,
    /// Lists, starts and ends wandering parties (`wandering.ron`).
    Wander,
    /// Tells where the player's home is, how far its furnishing has come and what the next
    /// step lacks.
    Home,
    /// Runs these command lines in order. `$1`, `$2`, … stand for this command's
    /// arguments and `$*` for all of them.
    Alias(Vec<String>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDef {
    /// Namespaced once loaded (`core:time`); typed without the namespace (`/time`).
    pub name: String,
    /// The arguments, as shown after the name in help (`set <day|noon|…>`).
    #[serde(default)]
    pub usage: String,
    pub help: String,
    #[serde(default)]
    pub permission: Permission,
    pub run: Run,
}

impl CommandDef {
    /// The name as typed, without the namespace.
    pub fn short_name(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// `/name usage`, for help and error messages.
    pub fn usage_line(&self) -> String {
        if self.usage.is_empty() { format!("/{}", self.short_name()) } else { format!("/{} {}", self.short_name(), self.usage) }
    }

    /// Its help in words: `command.core.time.help`, or the help as written.
    pub fn help_words(&self) -> crate::lang::Words {
        crate::lang::Words::Own(self.help_key(), self.help.clone())
    }

    /// Its help's first sentence in words, for the list `/help` gives:
    /// `command.core.time.help.summary`, or that sentence as written.
    pub fn summary_words(&self) -> crate::lang::Words {
        let summary = self.help.split_inclusive(". ").next().unwrap_or(&self.help).trim();
        crate::lang::Words::Own(format!("{}.summary", self.help_key()), summary.to_owned())
    }

    fn help_key(&self) -> String {
        format!("{}.help", crate::lang::def_key("command", &self.name))
    }
}

/// `commands.ron`: a package's commands, and names for times of day.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandFile {
    #[serde(default)]
    commands: Vec<CommandDef>,
    /// Times of day `/time set` understands by name, in ticks after sunrise.
    #[serde(default)]
    times: Vec<(String, u64)>,
    /// Real minutes a whole day takes at normal speed (the last package to say wins; 0,
    /// the default, says nothing).
    #[serde(default)]
    day_minutes: f32,
}

/// Why a typed name finds no single command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Unknown,
    /// Several packages define it; their namespaced names tell them apart.
    Ambiguous(Vec<String>),
}

#[derive(Debug, Default)]
pub struct CommandRegistry {
    defs: Vec<CommandDef>,
    /// Every namespaced name, and each short name defined by exactly one package.
    by_name: FxHashMap<String, usize>,
    /// Short names more than one package defines.
    ambiguous: FxHashMap<String, Vec<usize>>,
    times: Vec<(String, u64)>,
    day_minutes: Option<f32>,
}

/// Real minutes a day takes when no package says: its 24,000 ticks at 20 a second.
const DEFAULT_DAY_MINUTES: f32 = 20.0;

impl CommandRegistry {
    pub fn from_ron(namespace: &str, source: &str) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        registry.extend(namespace, source)?;
        Ok(registry)
    }

    /// Adds a package's commands and time names. Unqualified names are placed in
    /// `namespace`. A short name two packages share stays usable through the namespaced
    /// names, whatever order the packages load in.
    pub fn extend(&mut self, namespace: &str, source: &str) -> Result<(), RegistryError> {
        let file: CommandFile =
            ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/commands.ron: {e}")))?;
        for mut def in file.commands {
            if !def.name.contains(':') {
                def.name = format!("{namespace}:{}", def.name);
            }
            let short = def.short_name().to_owned();
            if short.is_empty() || short.chars().any(char::is_whitespace) {
                return Err(RegistryError(format!("command `{}` needs a name without spaces", def.name)));
            }
            let index = self.defs.len();
            if self.by_name.insert(def.name.clone(), index).is_some() {
                return Err(RegistryError(format!("duplicate command `{}`", def.name)));
            }
            if let Some(others) = self.ambiguous.get_mut(&short) {
                others.push(index);
            } else if let Some(other) = self.by_name.remove(&short) {
                self.ambiguous.insert(short, vec![other, index]);
            } else {
                self.by_name.insert(short, index);
            }
            self.defs.push(def);
        }
        for (name, ticks) in file.times {
            self.times.retain(|(existing, _)| *existing != name);
            self.times.push((name, ticks));
        }
        match file.day_minutes {
            0.0 => {}
            minutes if minutes > 0.0 && minutes.is_finite() => self.day_minutes = Some(minutes),
            _ => return Err(RegistryError(format!("{namespace}/commands.ron: day_minutes must be above 0"))),
        }
        Ok(())
    }

    /// Real minutes a whole day takes at normal speed.
    pub fn day_minutes(&self) -> f32 {
        self.day_minutes.unwrap_or(DEFAULT_DAY_MINUTES)
    }

    /// Ticks of the time of day per simulation tick at normal speed: below 1 when days are
    /// longer than their 24,000 ticks at 20 a second.
    pub fn day_pace(&self) -> f32 {
        DEFAULT_DAY_MINUTES / self.day_minutes()
    }

    /// The command a typed name (with or without its namespace) refers to.
    pub fn find(&self, name: &str) -> Result<&CommandDef, Lookup> {
        if let Some(&index) = self.by_name.get(name) {
            return Ok(&self.defs[index]);
        }
        match self.ambiguous.get(name) {
            Some(indices) => Err(Lookup::Ambiguous(indices.iter().map(|&i| self.defs[i].name.clone()).collect())),
            None => Err(Lookup::Unknown),
        }
    }

    /// Every command, in load order.
    pub fn iter(&self) -> impl Iterator<Item = &CommandDef> {
        self.defs.iter()
    }

    /// Names a player can type that start with `prefix`, sorted, for completion.
    pub fn complete(&self, prefix: &str, admin: bool) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .by_name
            .iter()
            .filter(|(name, _)| name.starts_with(prefix) && (prefix.contains(':') || !name.contains(':')))
            .filter(|&(_, &i)| admin || self.defs[i].permission == Permission::Anyone)
            .map(|(name, _)| name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    /// A time of day by name (`noon`), in ticks after sunrise.
    pub fn time_of_day(&self, name: &str) -> Option<u64> {
        self.times.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, ticks)| ticks)
    }

    /// The names of the times of day, in the order they were defined.
    pub fn times(&self) -> impl Iterator<Item = &str> {
        self.times.iter().map(|(name, _)| name.as_str())
    }
}

/// Splits a command line into its name and arguments at spaces. Double quotes keep
/// spaces in an argument (`/tp "Ada Lovelace"`). A leading `/` is optional.
pub fn split(line: &str) -> Vec<String> {
    let line = line.trim().strip_prefix('/').unwrap_or(line.trim());
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// The lines an alias runs, with `$1`, `$2`, … replaced by `args` (missing ones by
/// nothing) and `$*` by all of them. Arguments with spaces are quoted again.
pub fn expand(lines: &[String], args: &[String]) -> Vec<String> {
    let quote = |arg: &String| if arg.contains(char::is_whitespace) { format!("\"{arg}\"") } else { arg.clone() };
    let all = args.iter().map(quote).collect::<Vec<_>>().join(" ");
    lines
        .iter()
        .map(|line| {
            let mut out = String::new();
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                match (c, chars.peek()) {
                    ('$', Some('*')) => {
                        chars.next();
                        out.push_str(&all);
                    }
                    ('$', Some(d)) if d.is_ascii_digit() && *d != '0' => {
                        let n = d.to_digit(10).unwrap() as usize;
                        chars.next();
                        out.extend(args.get(n - 1).map(quote));
                    }
                    _ => out.push(c),
                }
            }
            out.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE: &str = r#"(
        commands: [
            (name: "time", usage: "set <time>", help: "Time.", permission: Admin, run: Time),
            (name: "help", help: "Help.", run: Help),
        ],
        times: [("noon", 6000), ("night", 13000)],
    )"#;

    #[test]
    fn short_names_unless_shared() {
        let mut registry = CommandRegistry::from_ron("core", CORE).unwrap();
        assert_eq!(registry.find("time").unwrap().name, "core:time");
        assert_eq!(registry.find("core:time").unwrap().run, Run::Time);
        assert_eq!(registry.find("nope").unwrap_err(), Lookup::Unknown);

        let weather = r#"(commands: [(name: "time", help: "Other.", run: Alias(["help"])), (name: "day", help: "Day.", run: Alias(["time set day"]))], times: [("noon", 5000)])"#;
        registry.extend("mod", weather).unwrap();
        assert_eq!(registry.find("time").unwrap_err(), Lookup::Ambiguous(vec!["core:time".into(), "mod:time".into()]));
        assert_eq!(registry.find("mod:time").unwrap().help, "Other.");
        assert_eq!(registry.find("day").unwrap().short_name(), "day");
        // Later packages may rename a time of day.
        assert_eq!(registry.time_of_day("NOON"), Some(5000));
        assert!(CommandRegistry::from_ron("core", r#"(commands: [(name: "a b", help: "", run: Help)])"#).is_err());
        assert!(registry.extend("core", CORE).is_err(), "duplicate namespaced names are an error");
    }

    #[test]
    fn completion_respects_permission() {
        let registry = CommandRegistry::from_ron("core", CORE).unwrap();
        assert_eq!(registry.complete("", false), ["help"]);
        assert_eq!(registry.complete("", true), ["help", "time"]);
        assert_eq!(registry.complete("core:t", true), ["core:time"]);
    }

    #[test]
    fn splitting_and_aliases() {
        assert_eq!(split("/tp  \"Ada Lovelace\" 3"), ["tp", "Ada Lovelace", "3"]);
        assert_eq!(split("weather rain 30"), ["weather", "rain", "30"]);
        assert_eq!(split("say \"\""), ["say", ""]);
        assert!(split("  ").is_empty());
        let lines = ["weather $1 $2".to_owned(), "say $*".to_owned()];
        let args = ["storm".to_owned(), "a b".to_owned()];
        assert_eq!(expand(&lines, &args), ["weather storm \"a b\"", "say storm \"a b\""]);
        assert_eq!(expand(&lines, &args[..1]), ["weather storm", "say storm"]);
    }
}
