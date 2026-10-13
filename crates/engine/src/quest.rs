//! Quests (the progression plan's quest engine, P20): content (each package's
//! `quests.ron`), checked against what a player has already done, so a quest completes the
//! moment its tasks hold, however long ago they were done. Nothing is ever locked behind a
//! quest: recipes stay open, and quests only point the way and reward the walk with
//! experience, items and knowledge.
//!
//! A quest has tasks, any one of which completes it (the routes: craft it, or find it), and
//! may need other quests first (`"a|b"` for either). Each task is a few goals that must all
//! hold. Goals read the player's state: their journal (kills, catches, plants picked, ores
//! mined, biomes stood in), their tallies of things made and cooked, their skills, what they
//! carry, and whether they have a home.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::RegistryError;
use crate::journal::{Journal, Page};

/// Something a player must have done.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Goal {
    /// Made this many of an item (or of anything with a tag, `tag:TAG`) at a crafting grid.
    Craft(String, u32),
    /// Cooked this many of an item, or of anything with a tag.
    Cook(String, u32),
    /// Successfully cooked this many of an item at a station with this tag.
    CookAt(String, String, u32),
    /// Caught this many fish (of one kind, or `any`), or this many kinds (`Kinds`).
    Catch(String, u32),
    /// Killed this many of a creature (or `any`).
    Kill(String, u32),
    /// Tamed this many of a creature (or `any`).
    Tame(String, u32),
    /// Picked or harvested this many of a plant (or `any`).
    Pick(String, u32),
    /// Mined this many of an ore or rock (or `any`).
    Mine(String, u32),
    /// Stood in this many different biomes.
    Visit(u32),
    /// Found this many entries on a journal page.
    Discover(Page, u32),
    /// Reached a level in a skill.
    Level(String, u8),
    /// Carries this many of an item, or of anything with a tag.
    Have(String, u32),
    /// Has a home (a lit hearth).
    Home,
    /// Built a standing multiblock this many times.
    Built(String, u32),
    /// Lit a standing multiblock, with its required fuel, this many times.
    Lit(String, u32),
    /// Looked an item up in the recipe viewer (the item may be `any` or `tag:TAG`).
    LookUp(String),
    /// Keeps this many animals at once whose condition is at least this percent (a newly
    /// tamed one starts at 60; only food, water, room and shelter raise it). What they
    /// keep now, not a count of what was done.
    Thriving(u32, u8),
}

/// One way to finish a quest: every goal must hold.
pub type Task = Vec<Goal>;

/// What finishing a quest gives.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Reward {
    /// Experience in a skill.
    Xp(String, f32),
    /// Items.
    Item(String, u8),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestDef {
    pub name: String,
    /// The age's chapter it belongs to (`nomad`, `farm`) and its branch in it (`main`,
    /// `hunter`, `fishing`...).
    pub age: String,
    pub branch: String,
    pub title: String,
    /// Why it is worth doing: the reason and what it brings, shown first.
    pub why: String,
    /// Any one of these finishes it.
    pub tasks: Vec<Task>,
    /// Quests that come before it; `"a|b"` takes either.
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub rewards: Vec<Reward>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct QuestFile {
    #[serde(default)]
    pub quests: Vec<QuestDef>,
    /// How the interface follows pinned quests (`core/quests.ron`); a later package
    /// changes what it gives, field by field.
    #[serde(default)]
    pub tracker: Option<TrackerPatch>,
}

/// What a package changes of how pinned quests are followed.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerPatch {
    #[serde(default)]
    pub pins: Option<usize>,
    /// Quests pinned for a player whose book is new (nothing done, nothing pinned).
    #[serde(default)]
    pub start_pins: Option<Vec<String>>,
    #[serde(default)]
    pub done_stays: Option<f32>,
    #[serde(default)]
    pub backing_alpha: Option<f32>,
}

/// How the interface follows pinned quests on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracker {
    /// Quests pinned at once, at most.
    pub pins: usize,
    /// Seconds a done quest stays on screen, ticked, before it drops off.
    pub done_stays: f32,
    /// How opaque the backing behind each pinned quest is, 0 to 1.
    pub backing_alpha: f32,
}

impl Default for Tracker {
    fn default() -> Self {
        Self { pins: 3, done_stays: 5.0, backing_alpha: 0.5 }
    }
}

/// Every quest, in the order packages list them.
#[derive(Debug, Default)]
pub struct QuestRegistry {
    quests: Vec<QuestDef>,
    tracker: Tracker,
    start_pins: Vec<String>,
}

/// What a player has done, as quests read it.
pub struct Progress<'a> {
    pub journal: &'a Journal,
    /// Tallies by key: `crafted:<item>`, `cooked:<item>`, `tamed:<creature>`.
    pub tallies: &'a FxHashMap<String, u32>,
    /// Their level in a skill, by name.
    pub level: &'a dyn Fn(&str) -> u8,
    /// How many of an item (or of anything with a tag, `tag:TAG`) they carry.
    pub carried: &'a dyn Fn(&str) -> u32,
    /// Whether an item name matches a goal's `tag:TAG`.
    pub has_tag: &'a dyn Fn(&str, &str) -> bool,
    /// What breaking a block (by name) gives, if anything: a Mine goal counts every block
    /// that drops its ore (deepslate copper and copper samples for copper).
    pub drops: &'a dyn Fn(&str) -> Option<String>,
    /// How many animals they keep with a condition of at least this percent.
    pub thriving: &'a dyn Fn(u8) -> u32,
    pub home: bool,
}

impl Progress<'_> {
    /// How far a goal has got, out of what it asks.
    pub fn goal(&self, goal: &Goal) -> (u32, u32) {
        let matches = |want: &str, name: &str| match want.strip_prefix("tag:") {
            Some(tag) => (self.has_tag)(name, tag),
            None => want == "any" || want == name || name.rsplit(':').next() == Some(want),
        };
        let journal = |page: Page, want: &str| self.journal.entries().filter(|e| e.page == page && matches(want, &e.name)).map(|e| e.count).sum::<u32>();
        let tally = |kind: &str, want: &str| {
            self.tallies.iter().filter_map(|(key, &n)| Some((key.strip_prefix(kind)?.strip_prefix(':')?, n))).filter(|(name, _)| matches(want, name)).map(|(_, n)| n).sum::<u32>()
        };
        match goal {
            Goal::Craft(item, n) => (tally("crafted", item), *n),
            Goal::Cook(item, n) => (tally("cooked", item), *n),
            Goal::CookAt(item, station, n) => (tally(&format!("cooked_at:{station}"), item), *n),
            Goal::Catch(fish, n) if fish == "Kinds" => (self.journal.found(Page::Fish) as u32, *n),
            Goal::Catch(fish, n) => (journal(Page::Fish, fish), *n),
            Goal::Kill(creature, n) => (journal(Page::Animals, creature) + journal(Page::Monsters, creature), *n),
            Goal::Tame(creature, n) => (tally("tamed", creature), *n),
            Goal::Pick(plant, n) => (journal(Page::Plants, plant), *n),
            Goal::Mine(ore, n) => {
                let mined = self.journal.entries().filter(|e| e.page == Page::Ores && (matches(ore, &e.name) || (self.drops)(&e.name).is_some_and(|d| matches(ore, &d))));
                (mined.map(|e| e.count).sum(), *n)
            }
            Goal::Visit(n) => (self.journal.found(Page::Biomes) as u32, *n),
            Goal::Discover(page, n) => (self.journal.found(*page) as u32, *n),
            Goal::Level(skill, n) => ((self.level)(skill) as u32, *n as u32),
            Goal::Have(item, n) => ((self.carried)(item), *n),
            Goal::Home => (u32::from(self.home), 1),
            Goal::Built(structure, n) => (tally("built", structure), *n),
            Goal::Lit(structure, n) => (tally("lit", structure), *n),
            Goal::LookUp(item) => (tally("lookedup", item).min(1), 1),
            Goal::Thriving(n, percent) => ((self.thriving)(*percent), *n),
        }
    }

    pub fn task_done(&self, task: &Task) -> bool {
        task.iter().all(|goal| {
            let (have, need) = self.goal(goal);
            have >= need
        })
    }
}

/// A quest's state as the client is told it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestState {
    pub name: String,
    pub done: bool,
    /// For each task, each goal's progress: (have, need).
    pub tasks: Vec<Vec<(u32, u32)>>,
}

impl QuestRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<QuestFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut file: QuestFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/quests.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.contains(':') {
                *name = format!("{namespace}:{name}");
            }
        };
        for name in file.tracker.iter_mut().filter_map(|t| t.start_pins.as_mut()).flatten() {
            qualify(name);
        }
        for quest in &mut file.quests {
            qualify(&mut quest.name);
            for after in &mut quest.after {
                *after = after.split('|').map(|n| if n.contains(':') { n.to_owned() } else { format!("{namespace}:{n}") }).collect::<Vec<_>>().join("|");
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<QuestFile>) -> Result<Self, RegistryError> {
        // Packages in order, each changing what it gives.
        let mut tracker = Tracker::default();
        let mut start_pins = Vec::new();
        for patch in files.iter().filter_map(|f| f.tracker.as_ref()) {
            if let Some(names) = &patch.start_pins {
                start_pins = names.clone();
            }
            tracker.pins = patch.pins.unwrap_or(tracker.pins);
            tracker.done_stays = patch.done_stays.unwrap_or(tracker.done_stays);
            tracker.backing_alpha = patch.backing_alpha.unwrap_or(tracker.backing_alpha);
        }
        if tracker.pins == 0 || !(tracker.done_stays >= 0.0) || !(0.0..=1.0).contains(&tracker.backing_alpha) {
            return Err(RegistryError(format!("quests.ron: the tracker needs a pin or more, a stay of 0 s or more and a backing from 0 to 1, not {tracker:?}")));
        }
        let quests: Vec<QuestDef> = files.into_iter().flat_map(|f| f.quests).collect();
        for quest in &quests {
            if quest.tasks.is_empty() {
                return Err(RegistryError(format!("quest {} has no task", quest.name)));
            }
            for name in quest.after.iter().flat_map(|a| a.split('|')) {
                if !quests.iter().any(|q| q.name == name) {
                    return Err(RegistryError(format!("quest {} comes after `{name}`, which does not exist", quest.name)));
                }
            }
        }
        if start_pins.len() > tracker.pins {
            return Err(RegistryError(format!("quests.ron: {} start pins, but the tracker follows {}", start_pins.len(), tracker.pins)));
        }
        if let Some(name) = start_pins.iter().find(|name| !quests.iter().any(|q| &q.name == *name)) {
            return Err(RegistryError(format!("quests.ron: the start pin `{name}` is not a quest")));
        }
        Ok(Self { quests, tracker, start_pins })
    }

    /// The quests a new player's book starts with pinned.
    pub fn start_pins(&self) -> &[String] {
        &self.start_pins
    }

    /// How the interface follows pinned quests.
    pub fn tracker(&self) -> Tracker {
        self.tracker
    }

    pub fn iter(&self) -> impl Iterator<Item = &QuestDef> {
        self.quests.iter()
    }

    pub fn get(&self, name: &str) -> Option<&QuestDef> {
        self.quests.iter().find(|q| q.name == name)
    }

    /// Whether a quest's earlier quests are done.
    pub fn open(&self, quest: &QuestDef, done: &dyn Fn(&str) -> bool) -> bool {
        quest.after.iter().all(|after| after.split('|').any(done))
    }

    /// Quests newly finished: open, not done, with a task that holds. Finishing one can
    /// open another that is already done, so this repeats until nothing changes.
    pub fn newly_done(&self, done: &[String], progress: &Progress) -> Vec<String> {
        let mut finished: Vec<String> = Vec::new();
        loop {
            let is_done = |name: &str| done.iter().chain(&finished).any(|d| d == name);
            let next: Vec<String> = self
                .quests
                .iter()
                .filter(|q| !is_done(&q.name) && self.open(q, &is_done) && q.tasks.iter().any(|t| progress.task_done(t)))
                .map(|q| q.name.clone())
                .collect();
            if next.is_empty() {
                return finished;
            }
            finished.extend(next);
        }
    }

    /// Every quest's state for the client.
    pub fn states(&self, done: &[String], progress: &Progress) -> Vec<QuestState> {
        self.quests
            .iter()
            .map(|q| QuestState {
                name: q.name.clone(),
                done: done.contains(&q.name),
                tasks: q.tasks.iter().map(|t| t.iter().map(|g| progress.goal(g)).collect()).collect(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooking_station_and_lighting_goals_reject_legacy_broad_tallies() {
        let journal = Journal::default();
        let tallies = FxHashMap::from_iter([
            ("cooked:base:cooked_rabbit".into(), 4),
            ("cooked_at:oven:base:cooked_rabbit".into(), 2),
            ("cooked_at:campfire:base:charcoal".into(), 16),
            ("built:base:crucible_kiln".into(), 1),
        ]);
        let level = |_: &str| 1;
        let carried = |_: &str| 0;
        let has_tag = |name: &str, tag: &str| name == "base:cooked_rabbit" && tag == "edible";
        let drops = |_: &str| None;
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: false };
        assert_eq!(progress.goal(&Goal::CookAt("tag:edible".into(), "campfire".into(), 1)), (0, 1));
        assert_eq!(progress.goal(&Goal::CookAt("tag:edible".into(), "oven".into(), 2)), (2, 2));
        assert_eq!(progress.goal(&Goal::Lit("crucible_kiln".into(), 1)), (0, 1));
        assert_eq!(progress.goal(&Goal::Built("crucible_kiln".into(), 1)), (1, 1));
    }

    #[test]
    fn thriving_counts_the_animals_kept_now_at_that_condition() {
        let (journal, tallies) = (Journal::default(), FxHashMap::default());
        let level = |_: &str| 1;
        let carried = |_: &str| 0;
        let has_tag = |_: &str, _: &str| false;
        let drops = |_: &str| None;
        // Three animals at 60, 80 and 90 percent.
        let thriving = |percent: u8| [60u8, 80, 90].iter().filter(|&&c| c >= percent).count() as u32;
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &thriving, home: false };
        assert_eq!(progress.goal(&Goal::Thriving(4, 75)), (2, 4));
        assert_eq!(progress.goal(&Goal::Thriving(2, 75)), (2, 2));
        assert_eq!(progress.goal(&Goal::Thriving(3, 50)), (3, 3));
    }

    #[test]
    fn start_pins_are_qualified_and_must_be_quests_within_the_tracker() {
        let load = |source: &str| QuestRegistry::new(vec![QuestRegistry::read("t", source).unwrap()]);
        let quest = r#"(name: "a", age: "nomad", branch: "main", title: "A", why: "", tasks: [[Home]])"#;
        let ok = load(&format!("(quests: [{quest}], tracker: (start_pins: [\"a\"]))")).unwrap();
        assert_eq!(ok.start_pins(), ["t:a".to_owned()]);
        assert!(load(&format!("(quests: [{quest}], tracker: (start_pins: [\"missing\"]))")).is_err());
        assert!(load(&format!("(quests: [{quest}], tracker: (pins: 1, start_pins: [\"a\", \"a\"]))")).is_err());
    }

    const QUESTS: &str = r#"(quests: [
        (name: "fire", age: "nomad", branch: "main", title: "First Night", why: "", tasks: [[Cook("any", 1)]], rewards: [Xp("cooking", 93.0), Item("torch", 4)]),
        (name: "tools", age: "nomad", branch: "main", title: "Stone in Hand", why: "", tasks: [[Craft("stone_pickaxe", 1), Craft("stone_axe", 1)]]),
        (name: "settle", age: "nomad", branch: "main", title: "Where to Settle", why: "", tasks: [[Visit(3)]], after: ["tools"]),
        (name: "hearth", age: "nomad", branch: "main", title: "Homestead Hearth", why: "", tasks: [[Home]], after: ["settle|fire"]),
        (name: "fish", age: "nomad", branch: "fishing", title: "Bare Hands", why: "", tasks: [[Catch("any", 5)], [Level("fishing", 10)]]),
    ])"#;

    fn registry() -> QuestRegistry {
        QuestRegistry::new(vec![QuestRegistry::read("base", QUESTS).unwrap()]).unwrap()
    }

    #[test]
    fn quests_finish_from_what_was_already_done_and_open_what_follows() {
        let quests = registry();
        let rules = crate::journal::JournalRules::default();
        let mut journal = Journal::default();
        for biome in ["base:forest", "base:plains", "base:bog"] {
            journal.record(&rules, Page::Biomes, biome, 0, 1);
        }
        journal.record(&rules, Page::Fish, "base:raw_trout", 3, 2);
        let mut tallies = FxHashMap::default();
        tallies.insert("crafted:base:stone_pickaxe".to_owned(), 1);
        tallies.insert("crafted:base:stone_axe".to_owned(), 2);
        let level = |_: &str| 1u8;
        let carried = |_: &str| 0u32;
        let has_tag = |_: &str, _: &str| false;
        let drops = |_: &str| None;
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: true };
        // The tools were made before the quest was ever seen; the biomes visited too. With
        // a home already, the hearth follows the same moment.
        let done = quests.newly_done(&[], &progress);
        assert_eq!(done, ["base:tools", "base:settle", "base:hearth"]);
        let states = quests.states(&done, &progress);
        let fish = states.iter().find(|s| s.name == "base:fish").unwrap();
        assert_eq!((fish.done, fish.tasks.clone()), (false, vec![vec![(3, 5)], vec![(1, 10)]]));
        // Two more trout finish it.
        journal.record(&rules, Page::Fish, "base:raw_trout", 2, 3);
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: true };
        assert_eq!(quests.newly_done(&done, &progress), ["base:fish"]);
    }

    /// A look-up goal holds from the first time the item was looked up, and a tag or `any`
    /// takes any item that matches.
    #[test]
    fn a_look_up_goal_holds_once_the_item_was_looked_up() {
        let journal = Journal::default();
        let (level, carried, drops) = (|_: &str| 1u8, |_: &str| 0u32, |_: &str| None);
        let has_tag = |name: &str, tag: &str| tag == "tool" && name.ends_with("pickaxe");
        let mut tallies = FxHashMap::default();
        let ask = |tallies: &FxHashMap<String, u32>, goal: &Goal| {
            let progress = Progress { journal: &journal, tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: false };
            progress.goal(goal)
        };
        let pickaxe = Goal::LookUp("stone_pickaxe".into());
        assert_eq!(ask(&tallies, &pickaxe), (0, 1));
        tallies.insert("lookedup:base:oak_planks".to_owned(), 1);
        assert_eq!(ask(&tallies, &pickaxe), (0, 1), "another item does not count");
        assert_eq!(ask(&tallies, &Goal::LookUp("any".into())), (1, 1));
        assert_eq!(ask(&tallies, &Goal::LookUp("tag:tool".into())), (0, 1));
        tallies.insert("lookedup:base:stone_pickaxe".to_owned(), 3);
        assert_eq!(ask(&tallies, &pickaxe), (1, 1), "however often it was looked up, it is once");
        assert_eq!(ask(&tallies, &Goal::LookUp("tag:tool".into())), (1, 1));
    }

    /// A Mine goal counts every block that drops its ore: deepslate copper (and copper
    /// samples) count toward copper, as they give copper ore.
    #[test]
    fn mining_counts_whatever_drops_the_ore() {
        let rules = crate::journal::JournalRules::default();
        let mut journal = Journal::default();
        journal.record(&rules, Page::Ores, "base:copper_ore", 2, 1);
        journal.record(&rules, Page::Ores, "base:deepslate_copper_ore", 3, 2);
        journal.record(&rules, Page::Ores, "base:deepslate_iron_ore", 4, 3);
        let tallies = FxHashMap::default();
        let (level, carried, has_tag) = (|_: &str| 1u8, |_: &str| 0u32, |_: &str, _: &str| false);
        let drops = |name: &str| name.strip_prefix("base:deepslate_").map(|ore| format!("base:{ore}"));
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: false };
        assert_eq!(progress.goal(&Goal::Mine("copper_ore".into(), 5)), (5, 5));
        assert_eq!(progress.goal(&Goal::Mine("base:iron_ore".into(), 5)), (4, 5));
        let nothing = |_: &str| None;
        let progress = Progress { drops: &nothing, ..progress };
        assert_eq!(progress.goal(&Goal::Mine("copper_ore".into(), 5)), (2, 5), "without drops, only copper ore itself");
    }

    #[test]
    fn a_later_package_changes_the_tracker_field_by_field() {
        let core = QuestRegistry::read("core", "(tracker: (pins: 2, done_stays: 3.0, backing_alpha: 0.4))").unwrap();
        let mod_ = QuestRegistry::read("mod", "(tracker: (pins: 5))").unwrap();
        assert_eq!(QuestRegistry::new(vec![core, mod_]).unwrap().tracker(), Tracker { pins: 5, done_stays: 3.0, backing_alpha: 0.4 });
        assert_eq!(registry().tracker(), Tracker::default());
        let none = QuestRegistry::read("core", "(tracker: (pins: 0))").unwrap();
        assert!(QuestRegistry::new(vec![none]).is_err());
        let clear = QuestRegistry::read("core", "(tracker: (backing_alpha: 1.5))").unwrap();
        assert!(QuestRegistry::new(vec![clear]).is_err());
    }

    #[test]
    fn a_quest_after_one_that_does_not_exist_is_refused() {
        let source = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "", why: "", tasks: [[Home]], after: ["nope"])])"#;
        assert!(QuestRegistry::new(vec![QuestRegistry::read("base", source).unwrap()]).is_err());
    }
}
