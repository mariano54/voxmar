//! What `music.ron` says: the categories of music (each a folder of Voxmar's own tracks,
//! or borrowed ones, or both), when tracks play and how loud, where to find borrowed ones
//! (`prism`), and where each borrowed track plays, by rules matching its location. Every
//! content package may have one; a later package's numbers replace an earlier one's, a
//! category it names again replaces that one, and its rules come first, so a mod can move
//! any track or add a category of its own.

use serde::Deserialize;
use voxmar_engine::content::ContentFile;

use super::prism::Discovery;

/// The file music is defined in, in any content package.
pub const FILE: &str = "music.ron";

/// Where the player is, for which tracks suit the moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Context {
    /// The title screen and the other menus.
    Menu,
    /// In a world, above ground.
    Game,
    /// In a world, deep under the surface and out of the sky's sight.
    Underground,
    /// Near a boss that is alive (a creature `boss: true`), or in a boss's room (`boss` in
    /// `music.ron`). Only tracks for it play then, and the others never do.
    Boss,
    /// Stepping inside the player's own lit home at night (`home` in `music.ron`), the
    /// first time that night: a short cue, never in the rotation of the others.
    Home,
    /// Stepping inside a settlement's bounds by day, the first time that game day (the
    /// home motif's day, `home` in `music.ron`): a short cue as `Home`'s is, and never over
    /// it.
    Settlement,
}

impl Context {
    /// The cues that play alone, once, as the player arrives: never in the rotation.
    pub const MOTIFS: [Context; 2] = [Context::Home, Context::Settlement];
}

/// The rules, merged across packages.
#[derive(Clone, Debug)]
pub struct MusicRules {
    /// Linear gain of a track playing in full, at 100% in the settings.
    pub volume: f32,
    /// Borrowed tracks' gain relative to Voxmar's own.
    pub borrowed_volume: f32,
    /// The loudness each of Voxmar's own tracks is brought to, in dB below full scale (the
    /// RMS of all of it), by its measured `levels`; `None` plays them as they are.
    pub loudness: Option<f32>,
    /// How loud each own track is as made, by its path under `assets/music`, in dB below
    /// full scale: the RMS of all of it, both channels. A test measures them again.
    pub levels: std::collections::BTreeMap<String, f32>,
    /// Seconds before the title screen's first track, as a range picked from.
    pub first_menu_delay: (f32, f32),
    /// Seconds of quiet between tracks in a world, and before its first.
    pub gap: (f32, f32),
    /// Seconds of quiet between tracks in the menus.
    pub menu_gap: (f32, f32),
    /// How the name of a track starting shows.
    pub now_playing: NowPlaying,
    /// When the player is in a boss fight.
    pub boss: BossMusic,
    /// Seconds a track takes to fade out when the player starts or leaves a game.
    pub fade_out: f32,
    /// Seconds before another pick when a borrowed track cannot be read or decoded.
    pub retry_after: f32,
    /// Seconds before a track when music is turned back on with `M`.
    pub resume_after: f32,
    /// The chance a pick is one of Voxmar's own tracks while borrowed ones suit too.
    pub own_share: f32,
    /// The categories, which the audio settings turn on and off, in the settings' order.
    pub categories: Vec<Category>,
    /// How many tracks played lately are passed over while others suit.
    pub recent: usize,
    pub underground: Underground,
    /// When the home motif plays.
    pub home: HomeMotif,
    pub prism: Discovery,
    pub rules: Vec<Rule>,
}

/// The home motif: a track for `Home` (one of that context's, picked as others are) plays
/// as the player steps inside their own lit home (the hearth `Vitals::home` names, burning)
/// between `dusk` and `dawn`, once a night. Inside is within `reach` blocks of the hearth
/// each way, with a roof (a solid block) within `roof` blocks over the head. A boss fight
/// holds it back, and one starting fades it as it fades any calm track.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeMotif {
    pub reach: i32,
    pub roof: i32,
    /// Ticks after sunrise (as `/time` counts) the night starts and ends; one running past
    /// midnight wraps.
    pub dusk: u64,
    pub dawn: u64,
}

impl HomeMotif {
    /// Which night `ticks` (since the world began) falls in, counted from the first dusk;
    /// none by day.
    pub fn night(&self, ticks: u64) -> Option<u64> {
        let day = voxmar_protocol::TICKS_PER_DAY;
        let since_dusk = (ticks + day - self.dusk) % day;
        let long = (self.dawn + day - self.dusk) % day;
        (since_dusk < long).then(|| (ticks + day - self.dusk) / day)
    }

    /// Which game day `ticks` falls in, counted from the first dawn; none by night. The
    /// settlement motif plays once in each.
    pub fn day(&self, ticks: u64) -> Option<u64> {
        let day = voxmar_protocol::TICKS_PER_DAY;
        self.night(ticks).is_none().then(|| (ticks + day - self.dawn) / day)
    }
}

/// The night is part of a day and has some length; inside is a box and a roof of a few
/// blocks, as a home's are.
fn check_home(home: &HomeMotif) -> Result<(), String> {
    let day = voxmar_protocol::TICKS_PER_DAY;
    if home.dusk >= day || home.dawn >= day || home.dusk == home.dawn {
        return Err(format!("home: dusk {} and dawn {} must differ, each under {day} ticks", home.dusk, home.dawn));
    }
    if !(1..=32).contains(&home.reach) || !(1..=32).contains(&home.roof) {
        return Err(format!("home: reach {} and roof {} must be 1 to 32 blocks", home.reach, home.roof));
    }
    Ok(())
}

/// The line naming a track as it starts, and its category, at the top right.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NowPlaying {
    /// Seconds it shows, fading in and out included.
    pub shown: f32,
    /// Seconds it takes to fade in, and to fade out.
    pub fade: f32,
}

/// When the player counts as in a boss fight: a boss alive within `within` blocks (one that
/// roams, in a fight within the last `calm_after` seconds), or a block of a boss's room (one
/// of `rooms`) within `room_reach` blocks.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossMusic {
    pub within: f32,
    #[serde(default)]
    pub rooms: Vec<BossRoom>,
    #[serde(default)]
    pub room_reach: i32,
    /// Seconds after a roaming boss last attacked, showed a tell or was hurt that its fight
    /// is over and its music ends (`Creatures::stirred`).
    #[serde(default = "calm_after")]
    pub calm_after: f32,
}

fn calm_after() -> f32 {
    12.0
}

impl Default for BossMusic {
    fn default() -> Self {
        Self { within: 0.0, rooms: Vec::new(), room_reach: 0, calm_after: calm_after() }
    }
}

/// A boss's room: a block of it (by name) and the boss whose room it is, whose fight music
/// (its `music` tags) plays there.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossRoom {
    pub block: String,
    pub boss: String,
}

/// When the player counts as underground.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Underground {
    /// Blocks below the ground surface here, at least.
    pub depth: f32,
    /// Share of the nearby air open to the sky, at most.
    pub max_sky: f32,
}

/// A category of music, switched on and off in the audio settings. Voxmar's own tracks are
/// the files in its folder; borrowed tracks join it by the rules naming it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// What the rules and the settings file call it: lowercase letters, digits and `_`. The
    /// languages call it `music.<name>`, and say what it holds in `music.<name>.hint`.
    pub name: String,
    /// The folder of `assets/music` its own tracks are in, if it has any.
    #[serde(default)]
    pub folder: Option<String>,
    /// Where its own tracks play.
    #[serde(default = "everywhere")]
    pub contexts: Vec<Context>,
    /// Where some of its own tracks play instead, by their path under `assets/music` (with
    /// the folder: `elevenlabs/round5/forest/boss-01.mp3`): the first rule a track matches
    /// decides; one matching none plays as `contexts` says, in any biome.
    #[serde(default)]
    pub tracks: Vec<Rule>,
}

impl Category {
    /// Whether its tracks all come from a Prism Launcher instance, so it matters only with
    /// Minecraft assets on.
    pub fn borrowed_only(&self) -> bool {
        self.folder.is_none()
    }

    /// The rule of `tracks` an own track at `path` (from `assets/music`) matches first.
    pub fn track_rule(&self, path: &str) -> Option<&Rule> {
        self.tracks.iter().find(|rule| rule.paths.iter().any(|pattern| glob(pattern, path)))
    }
}

fn everywhere() -> Vec<Context> {
    vec![Context::Menu, Context::Game, Context::Underground]
}

/// Where tracks at matching locations play. The first rule a location matches decides.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Patterns of paths, the location less its namespace, `*` standing for anything:
    /// `*music/menu/*`.
    pub paths: Vec<String>,
    /// The category they are in (`categories`); left out in a category's own `tracks`.
    #[serde(default)]
    pub category: String,
    /// Where they play; none leaves them out.
    pub contexts: Vec<Context>,
    /// How often one is picked against the others that suit the moment.
    #[serde(default = "one")]
    pub weight: f32,
    /// Biome tags (`biomes.ron`) where they play, one enough; none for anywhere. For boss
    /// tracks, the tags a boss's `music` names (any one): tagged ones play only for those
    /// bosses, untagged ones for every boss.
    #[serde(default)]
    pub biomes: Vec<String>,
}

fn one() -> f32 {
    1.0
}

/// One package's file: what it leaves out, an earlier package (or the default) decides.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    volume: Option<f32>,
    borrowed_volume: Option<f32>,
    loudness: Option<f32>,
    levels: std::collections::BTreeMap<String, f32>,
    first_menu_delay: Option<(f32, f32)>,
    gap: Option<(f32, f32)>,
    menu_gap: Option<(f32, f32)>,
    now_playing: Option<NowPlaying>,
    boss: Option<BossMusic>,
    fade_out: Option<f32>,
    retry_after: Option<f32>,
    resume_after: Option<f32>,
    own_share: Option<f32>,
    categories: Vec<Category>,
    recent: Option<usize>,
    underground: Option<Underground>,
    home: Option<HomeMotif>,
    prism: Option<Discovery>,
    rules: Vec<Rule>,
}

impl Default for MusicRules {
    /// Before any `music.ron`: no categories, so no tracks.
    fn default() -> Self {
        Self {
            volume: 0.35,
            borrowed_volume: 1.0,
            loudness: None,
            levels: Default::default(),
            first_menu_delay: (2.0, 6.0),
            gap: (180.0, 420.0),
            menu_gap: (180.0, 420.0),
            now_playing: NowPlaying { shown: 6.0, fade: 1.0 },
            boss: BossMusic::default(),
            fade_out: 1.5,
            retry_after: 2.0,
            resume_after: 2.0,
            own_share: 1.0,
            categories: Vec::new(),
            recent: 0,
            underground: Underground { depth: f32::INFINITY, max_sky: 0.0 },
            home: HomeMotif { reach: 7, roof: 12, dusk: 16500, dawn: 23500 },
            prism: Discovery::default(),
            rules: Vec::new(),
        }
    }
}

impl MusicRules {
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut rules = Self::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
            let read: File = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            macro_rules! replace {
                ($($field:ident),*) => {$(if let Some(value) = read.$field { rules.$field = value; })*};
            }
            rules.levels.extend(read.levels);
            if read.loudness.is_some() {
                rules.loudness = read.loudness;
            }
            if let Some(home) = &read.home {
                check_home(home).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            }
            replace!(volume, borrowed_volume, first_menu_delay, gap, menu_gap, now_playing, boss, fade_out, retry_after, resume_after, own_share, recent, underground, home, prism);
            for category in read.categories {
                check_category(&category).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
                match rules.categories.iter_mut().find(|known| known.name == category.name) {
                    Some(known) => *known = category,
                    None => rules.categories.push(category),
                }
            }
            rules.rules.splice(0..0, read.rules);
        }
        if let Some(rule) = rules.categories.iter().flat_map(|c| &c.tracks).find(|rule| !rule.category.is_empty()) {
            return Err(format!("{FILE}: the category's track rule for {:?} names a category; it is the one it is in", rule.paths));
        }
        // The home and settlement motifs are cues of their own, never also in the rotation
        // nor each other.
        for motif in Context::MOTIFS {
            let lists = rules.categories.iter().map(|c| (c.name.as_str(), &c.contexts)).chain(rules.categories.iter().flat_map(|c| c.tracks.iter().map(move |r| (c.name.as_str(), &r.contexts))));
            if let Some((category, _)) = lists.chain(rules.rules.iter().map(|r| (r.category.as_str(), &r.contexts))).find(|(_, contexts)| contexts.contains(&motif) && contexts.len() > 1) {
                return Err(format!("{FILE}: category {category:?} plays tracks both as the {motif:?} motif and elsewhere; {motif:?} plays alone"));
            }
            // A cue starts at once, and a borrowed track is read first.
            if let Some(rule) = rules.rules.iter().find(|rule| rule.contexts.contains(&motif)) {
                return Err(format!("{FILE}: the rule for {:?} makes borrowed tracks the {motif:?} motif; it is a category's own tracks", rule.paths));
            }
        }
        if let Some(rule) = rules.rules.iter().find(|rule| rules.category(&rule.category).is_none()) {
            return Err(format!("{FILE}: the rule for {:?} puts tracks in category {:?}, which no categories list has", rule.paths, rule.category));
        }
        if !(rules.boss.calm_after > 0.0 && rules.boss.calm_after.is_finite()) {
            return Err(format!("{FILE}: boss `calm_after` is seconds, more than 0 (not {})", rules.boss.calm_after));
        }
        Ok(rules)
    }

    /// The gain that brings an own track (its path under `assets/music`) to `loudness`: at
    /// most four times up or down, and none for a track not measured.
    pub fn trim(&self, track: &str) -> f32 {
        match (self.loudness, self.levels.get(track)) {
            (Some(target), Some(level)) => 10f32.powf((target - level) / 20.0).clamp(0.25, 4.0),
            _ => 1.0,
        }
    }

    /// The category called `name`.
    pub fn category(&self, name: &str) -> Option<&Category> {
        self.categories.iter().find(|category| category.name == name)
    }

    /// The rule a borrowed track's location matches by its path, if any. The namespace is
    /// left out, so a mod named `nethertools` is not taken for the Nether.
    pub fn rule(&self, location: &str) -> Option<&Rule> {
        let path = location.split_once(':').map_or(location, |(_, path)| path);
        self.rules.iter().find(|rule| rule.paths.iter().any(|pattern| glob(pattern, path)))
    }
}

/// A category's name is part of language keys and the settings file, and its folder lies
/// inside `assets/music`.
fn check_category(category: &Category) -> Result<(), String> {
    let name = &category.name;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(format!("category {name:?}: a name is lowercase letters, digits and _"));
    }
    if let Some(folder) = &category.folder
        && (folder.is_empty() || folder.starts_with('/') || folder.split(['/', '\\']).any(|part| part == ".."))
    {
        return Err(format!("category {name}: folder {folder:?} is not inside assets/music"));
    }
    Ok(())
}

/// Whether `text` matches `pattern`, where `*` matches any run of characters (`/` too).
pub fn glob(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else { return false };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else { return rest.is_empty() };
    for part in middle {
        let Some(at) = rest.find(part) else { return false };
        rest = &rest[at + part.len()..];
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_anything_in_place_of_a_star() {
        assert!(glob("*music/menu/*", "minecraft:music/menu/menu1"));
        assert!(glob("*music/menu/*", "melodies:minecraft/music/menu/menu1"));
        assert!(!glob("*music/menu/*", "minecraft:music/game/calm1"));
        assert!(glob("*", ""));
        assert!(glob("record*", "record.sky"));
        assert!(!glob("record*", "block.record"));
        assert!(glob("*records/11", "minecraft:records/11"));
        assert!(!glob("*records/11", "minecraft:records/11b"));
        assert!(glob("a*b*a", "aba"));
        assert!(!glob("a*b*a", "ab"));
        assert!(glob("exact", "exact") && !glob("exact", "exactly"));
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() { files.extend(walk(&path)) } else { files.push(path) }
        }
        files
    }

    fn package(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    #[test]
    fn later_packages_replace_numbers_and_their_rules_come_first() {
        let base = package(
            "base",
            r#"(volume: 0.3, recent: 4, categories: [(name: "ours", folder: "ours"), (name: "game")], rules: [(paths: ["*music/*"], category: "game", contexts: [Game])])"#,
        );
        let mod_ = package(
            "zmod",
            r#"(recent: 9, categories: [(name: "menu"), (name: "ours", folder: "theirs", contexts: [Menu])], rules: [(paths: ["*music/menu/*"], category: "menu", contexts: [Menu], weight: 2.0)])"#,
        );
        let rules = MusicRules::load(&[base, mod_]).unwrap();
        assert_eq!((rules.volume, rules.recent, rules.gap), (0.3, 9, (180.0, 420.0)));
        // A category named again is replaced where it was; a new one comes after.
        let names: Vec<&str> = rules.categories.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["ours", "game", "menu"]);
        let ours = rules.category("ours").unwrap();
        assert_eq!((ours.folder.as_deref(), ours.contexts.as_slice()), (Some("theirs"), [Context::Menu].as_slice()));
        assert_eq!(rules.category("game").unwrap().contexts, [Context::Menu, Context::Game, Context::Underground]);
        assert!(rules.category("game").unwrap().borrowed_only() && !ours.borrowed_only());
        let menu = rules.rule("minecraft:music/menu/menu1").unwrap();
        assert_eq!((menu.category.as_str(), menu.contexts.as_slice(), menu.weight), ("menu", [Context::Menu].as_slice(), 2.0));
        assert_eq!(rules.rule("minecraft:music/game/calm1").unwrap().weight, 1.0);
        assert!(rules.rule("minecraft:dig/grass1").is_none());
        // Patterns see the path, not the namespace.
        assert!(rules.rule("musicmod:dig/grass1").is_none());
        assert!(MusicRules::load(&[package("bad", "(loudest: 2.0)")]).unwrap_err().contains("bad/music.ron"));
    }

    /// A rule's category must be listed, and a category's name and folder must be safe to
    /// use as a language key, a settings entry and a folder of `assets/music`.
    #[test]
    fn categories_are_checked_as_they_load() {
        let unlisted = package("base", r#"(rules: [(paths: ["*"], category: "world", contexts: [Game])])"#);
        assert!(MusicRules::load(&[unlisted]).unwrap_err().contains("\"world\", which no categories list has"));
        for (bad, says) in [
            (r#"(categories: [(name: "Suno Tracks")])"#, "lowercase"),
            (r#"(categories: [(name: "")])"#, "lowercase"),
            (r#"(categories: [(name: "up", folder: "../sounds")])"#, "not inside"),
            (r#"(categories: [(name: "root", folder: "/music")])"#, "not inside"),
        ] {
            let error = MusicRules::load(&[package("zmod", bad)]).unwrap_err();
            assert!(error.contains("zmod/music.ron") && error.contains(says), "{bad}: {error}");
        }
        // A mod may put its rules' tracks in a category an earlier package lists.
        let base = package("base", r#"(categories: [(name: "world")])"#);
        let mod_ = package("zmod", r#"(rules: [(paths: ["*"], category: "world", contexts: [Game])])"#);
        assert_eq!(MusicRules::load(&[base, mod_]).unwrap().rule("x:y").unwrap().category, "world");
    }

    /// The home motif's nights are counted from dusk, one wrapping past midnight as one, and
    /// its numbers and contexts are checked as they load: a night of some length within a
    /// day, a home-sized box and roof, and `Home` alone, a category's own tracks.
    #[test]
    fn the_home_motif_counts_nights_and_is_checked_as_it_loads() {
        let home = HomeMotif { reach: 7, roof: 12, dusk: 16500, dawn: 23500 };
        let day = voxmar_protocol::TICKS_PER_DAY;
        assert_eq!(home.night(6000), None, "noon");
        assert_eq!(home.night(20000), home.night(23000));
        assert_ne!(home.night(20000), home.night(day + 20000), "the next night is another");
        assert_eq!(home.night(23600), None, "after dawn");
        let wraps = HomeMotif { dusk: 22000, dawn: 2000, ..home };
        assert_eq!(wraps.night(23000), wraps.night(day + 1000), "past midnight is the same night");
        assert_eq!(wraps.night(day + 3000), None);
        // Its days, the settlement motif's: one from dawn to dusk, none by night, the next
        // dawn another.
        assert_eq!(home.day(20000), None, "night");
        assert_eq!(home.day(23600), home.day(day + 6000), "dawn and the noon after are one day");
        assert_ne!(home.day(6000), home.day(day + 6000), "the next noon is another");
        assert_eq!(wraps.day(day + 3000), wraps.day(day + 12000));
        for (bad, says) in [
            ("(home: (reach: 7, roof: 12, dusk: 16500, dawn: 16500))", "must differ"),
            ("(home: (reach: 7, roof: 12, dusk: 24000, dawn: 100))", "must differ"),
            ("(home: (reach: 0, roof: 12, dusk: 16500, dawn: 23500))", "1 to 32"),
            ("(home: (reach: 7, roof: 99, dusk: 16500, dawn: 23500))", "1 to 32"),
            (r#"(categories: [(name: "home", folder: "home", contexts: [Home, Game])])"#, "Home plays alone"),
            (r#"(categories: [(name: "x", folder: "x", tracks: [(paths: ["*"], contexts: [Game, Home])])])"#, "Home plays alone"),
            (r#"(categories: [(name: "x")], rules: [(paths: ["*"], category: "x", contexts: [Home])])"#, "a category's own tracks"),
            (r#"(categories: [(name: "x", folder: "x", contexts: [Settlement, Home])])"#, "plays alone"),
            (r#"(categories: [(name: "x", folder: "x", contexts: [Settlement, Game])])"#, "Settlement plays alone"),
            (r#"(categories: [(name: "x")], rules: [(paths: ["*"], category: "x", contexts: [Settlement])])"#, "a category's own tracks"),
        ] {
            let error = MusicRules::load(&[package("zmod", bad)]).map(|_| ()).unwrap_err();
            assert!(error.contains(says), "{bad}: {error}");
        }
        let fine = MusicRules::load(&[package("zmod", r#"(home: (reach: 5, roof: 8, dusk: 22000, dawn: 2000), categories: [(name: "home", folder: "home", contexts: [Home])])"#)]).unwrap();
        assert_eq!(fine.home, HomeMotif { reach: 5, roof: 8, ..wraps });
    }

    /// The shipped rules place each kind of track the GT New Horizons soundtrack and the
    /// asset store hold.
    #[test]
    fn shipped_rules_place_each_kind_of_track() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let rules = MusicRules::load(&content.files).unwrap();
        use Context::*;
        for (location, contexts) in [
            ("minecraft:music/menu/menu1", &[Menu][..]),
            ("minecraft:music/game/end/credits", &[Menu]),
            ("minecraft:music/game/calm1", &[Game, Underground]),
            ("minecraft:music/game/creative/creative1", &[Game]),
            ("minecraft:music/game/nether/nether1", &[Underground]),
            ("minecraft:music/game/end/boss", &[Underground]),
            ("minecraft:records/cat", &[Game, Underground]),
            ("minecraft:records/11", &[Underground]),
            ("horizonmelodies:minecraft/music/menu/menu3", &[Menu]),
            ("horizonmelodies:minecraft/music/game/swamp/bayou", &[Game, Underground]),
            ("horizonmelodies:minecraft/music/game/water/axolotl", &[Game]),
            ("horizonmelodies:minecraft/music/game/crimson_forest/chrysopoeia", &[Underground]),
            ("horizonmelodies:minecraft/records/pigstep", &[Game, Underground]),
            ("horizonmelodies:music_and_melody/music/overworld/a", &[Game, Underground]),
            ("horizonmelodies:music_and_melody/music/caves/a", &[Underground]),
            ("horizonmelodies:music_and_melody/music/under_water/a", &[Game]),
            ("horizonmelodies:music_and_melody/music/structure/stronghold/a", &[Underground]),
            ("horizonmelodies:music_and_melody/music/ancient_city/a", &[Underground]),
            ("horizonmelodies:music_and_melody/music/menu/a", &[Menu]),
            ("horizonmelodies:music_and_melody/music/misc/a", &[Game, Underground]),
            ("horizonmelodies:music_and_melody/loose_track", &[Game, Underground]),
            // A mod's namespace says nothing about where its tracks play.
            ("nethercreativeboss:music/calm", &[Game, Underground]),
            ("endermenu:music/overworld/a", &[Game, Underground]),
        ] {
            let rule = rules.rule(location).unwrap_or_else(|| panic!("{location} has no rule"));
            assert_eq!(rule.contexts, contexts, "{location} ({})", rule.category);
            assert!(rule.weight > 0.0);
        }
        assert!(rules.own_share > 0.0 && rules.own_share < 1.0);
        // A world is mostly quiet: gaps far longer than a track (two to three minutes), at
        // least five minutes, and the name of each shows a moment, fading in and out.
        assert!(rules.gap.0 >= 300.0 && rules.gap.1 >= rules.gap.0, "{:?}", rules.gap);
        assert!(rules.now_playing.shown > 2.0 * rules.now_playing.fade && rules.now_playing.fade > 0.0);
        // Voxmar's own tracks, a folder for each source, play in the menus and the world.
        for name in ["suno", "lyria"] {
            let category = rules.category(name).unwrap_or_else(|| panic!("no category {name}"));
            assert_eq!(category.folder.as_deref(), Some(name));
            assert!(category.contexts.contains(&Menu) && category.contexts.contains(&Game), "{name}");
        }
        // ElevenLabs' tracks: a boss's only in a boss fight, in its biome first; a biome's
        // only there, above ground; the general ones anywhere.
        let eleven = rules.category("elevenlabs").expect("no category elevenlabs");
        for (path, contexts, biome) in [
            ("elevenlabs/round5/forest/boss-01-a-corrupted-beast.mp3", &[Boss][..], Some("forest")),
            ("elevenlabs/round5/snow/boss-01-a-frost-giant.mp3", &[Boss], Some("frozen")),
            ("elevenlabs/round5/desert/boss-01-the-guardian.mp3", &[Boss], Some("desert")),
            ("elevenlabs/round5/fantasy/boss-01-a-duel.mp3", &[Boss], Some("fantastical")),
            ("elevenlabs/round9/swamp/boss-01-a-hag.mp3", &[Boss], None),
            ("elevenlabs/round4/forest/03-under-the-oaks.mp3", &[Game], Some("forest")),
            ("elevenlabs/round5/snow/02-quiet-drifts.mp3", &[Game], Some("frozen")),
        ] {
            let rule = eleven.track_rule(path).unwrap_or_else(|| panic!("{path} has no rule"));
            assert_eq!(rule.contexts, contexts, "{path}");
            assert_eq!(rule.biomes.first().map(String::as_str), biome, "{path}");
        }
        assert!(eleven.track_rule("elevenlabs/round5/general/01-a-peaceful-village-square.mp3").is_none());
        assert_eq!(eleven.contexts, [Menu, Game, Underground]);
        assert!(rules.boss.within > 0.0);
        // Every category is named and explained (every language has the same keys, as
        // `lang::every_language_has_the_same_keys` checks), every folder holds tracks, and
        // every track of `assets/music` is in a category's folder.
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let english = voxmar_engine::lang::Lang::load(&assets.join("content"), "en");
        for category in &rules.categories {
            for key in [format!("music.{}", category.name), format!("music.{}.hint", category.name)] {
                assert!(english.find(&key).is_some(), "no {key}");
            }
        }
        let audio = |path: &std::path::PathBuf| path.extension().is_some_and(|e| e == "mp3" || e == "ogg");
        let mut placed = 0;
        for folder in rules.categories.iter().filter_map(|c| c.folder.as_ref()) {
            let tracks = walk(&assets.join("music").join(folder)).iter().filter(|p| audio(p)).count();
            assert!(tracks > 0, "assets/music/{folder} has no tracks");
            placed += tracks;
        }
        let all: Vec<_> = walk(&assets.join("music")).into_iter().filter(audio).collect();
        assert_eq!(placed, all.len(), "tracks outside every category's folder: {all:?}");
        assert!(!rules.prism.instances.is_empty() && !rules.prism.sources.is_empty());
        // The home motif: its own category, its tracks for `Home` alone, at night by the
        // way-home mark's clock, inside the home's reach (survival.ron's).
        let home = rules.category("home").expect("no category home");
        assert_eq!((home.folder.as_deref(), home.contexts.as_slice()), (Some("home"), [Home].as_slice()));
        assert!(rules.home.night(20000).is_some() && rules.home.night(6000).is_none(), "{:?}", rules.home);
        let home_reach = content.files.iter().filter(|f| f.name == "survival.ron").filter_map(|f| f.source.split("home_reach:").nth(1)?.split(',').next()?.trim().parse::<i32>().ok()).last();
        assert_eq!(Some(rules.home.reach), home_reach, "the server's home and the motif's");
        // The settlement motif: its own category too, for `Settlement` alone.
        let settlement = rules.category("settlement").expect("no category settlement");
        assert_eq!((settlement.folder.as_deref(), settlement.contexts.as_slice()), (Some("settlement"), [Context::Settlement].as_slice()));
    }
}
