//! What `music.ron` says: when tracks play and how loud, where to find borrowed ones
//! (`prism`), and where each borrowed track plays, by rules matching its location. Every
//! content package may have one; a later package's numbers replace an earlier one's and
//! its rules come first, so a mod can move any track.

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
}

/// The rules, merged across packages.
#[derive(Clone, Debug)]
pub struct MusicRules {
    /// Linear gain of a track playing in full, at 100% in the settings.
    pub volume: f32,
    /// Borrowed tracks' gain relative to Voxmar's own.
    pub borrowed_volume: f32,
    /// Seconds before the title screen's first track, as a range picked from.
    pub first_menu_delay: (f32, f32),
    /// Seconds of quiet between tracks.
    pub gap: (f32, f32),
    /// Seconds a track takes to fade out when the player starts or leaves a game.
    pub fade_out: f32,
    /// The chance a pick is one of Voxmar's own tracks while borrowed ones suit too.
    pub own_share: f32,
    /// Where Voxmar's own tracks play.
    pub own_contexts: Vec<Context>,
    /// How many tracks played lately are passed over while others suit.
    pub recent: usize,
    pub underground: Underground,
    pub prism: Discovery,
    pub rules: Vec<Rule>,
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

/// Where tracks at matching locations play. The first rule a location matches decides.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Patterns of locations, `*` standing for anything: `*music/menu/*`.
    pub paths: Vec<String>,
    /// A name for the log.
    pub category: String,
    /// Where they play; none leaves them out.
    pub contexts: Vec<Context>,
    /// How often one is picked against the others that suit the moment.
    #[serde(default = "one")]
    pub weight: f32,
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
    first_menu_delay: Option<(f32, f32)>,
    gap: Option<(f32, f32)>,
    fade_out: Option<f32>,
    own_share: Option<f32>,
    own_contexts: Option<Vec<Context>>,
    recent: Option<usize>,
    underground: Option<Underground>,
    prism: Option<Discovery>,
    rules: Vec<Rule>,
}

impl Default for MusicRules {
    /// Music as it was before `music.ron`: Voxmar's own tracks, everywhere.
    fn default() -> Self {
        Self {
            volume: 0.35,
            borrowed_volume: 1.0,
            first_menu_delay: (2.0, 6.0),
            gap: (180.0, 420.0),
            fade_out: 1.5,
            own_share: 1.0,
            own_contexts: vec![Context::Menu, Context::Game, Context::Underground],
            recent: 0,
            underground: Underground { depth: f32::INFINITY, max_sky: 0.0 },
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
            replace!(volume, borrowed_volume, first_menu_delay, gap, fade_out, own_share, own_contexts, recent, underground, prism);
            rules.rules.splice(0..0, read.rules);
        }
        Ok(rules)
    }

    /// The rule a borrowed track's location matches, if any.
    pub fn rule(&self, location: &str) -> Option<&Rule> {
        self.rules.iter().find(|rule| rule.paths.iter().any(|pattern| glob(pattern, location)))
    }
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

    fn package(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    #[test]
    fn later_packages_replace_numbers_and_their_rules_come_first() {
        let base = package("base", r#"(volume: 0.3, recent: 4, rules: [(paths: ["*music/*"], category: "game", contexts: [Game])])"#);
        let mod_ = package("zmod", r#"(recent: 9, rules: [(paths: ["*music/menu/*"], category: "menu", contexts: [Menu], weight: 2.0)])"#);
        let rules = MusicRules::load(&[base, mod_]).unwrap();
        assert_eq!((rules.volume, rules.recent, rules.gap), (0.3, 9, (180.0, 420.0)));
        let menu = rules.rule("minecraft:music/menu/menu1").unwrap();
        assert_eq!((menu.category.as_str(), menu.contexts.as_slice(), menu.weight), ("menu", [Context::Menu].as_slice(), 2.0));
        assert_eq!(rules.rule("minecraft:music/game/calm1").unwrap().weight, 1.0);
        assert!(rules.rule("minecraft:dig/grass1").is_none());
        assert!(MusicRules::load(&[package("bad", "(loudness: 2.0)")]).unwrap_err().contains("bad/music.ron"));
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
        ] {
            let rule = rules.rule(location).unwrap_or_else(|| panic!("{location} has no rule"));
            assert_eq!(rule.contexts, contexts, "{location} ({})", rule.category);
            assert!(rule.weight > 0.0);
        }
        assert!(rules.own_share > 0.0 && rules.own_share < 1.0);
        assert!(rules.own_contexts.contains(&Menu) && rules.own_contexts.contains(&Game));
        assert!(!rules.prism.instances.is_empty() && !rules.prism.sources.is_empty());
    }
}
