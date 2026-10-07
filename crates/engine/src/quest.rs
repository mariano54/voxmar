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

use crate::block::{BlockRegistry, RegistryError};
use crate::skill::SkillRegistry;
use crate::journal::{Journal, Page};

/// Something a player must have done.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Goal {
    /// Made this many of an item (or of anything with a tag, `tag:TAG`) at a crafting grid.
    Craft(String, u32),
    /// Cooked this many of an item, or of anything with a tag.
    Cook(String, u32),
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
    /// Lit a multiblock (a crucible kiln) this many times.
    Built(String, u32),
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerPatch {
    #[serde(default)]
    pub pins: Option<usize>,
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
            Goal::Catch(fish, n) if fish == "Kinds" => (self.journal.found(Page::Fish) as u32, *n),
            Goal::Catch(fish, n) => (journal(Page::Fish, fish), *n),
            Goal::Kill(creature, n) => (journal(Page::Animals, creature) + journal(Page::Monsters, creature), *n),
            Goal::Tame(creature, n) => (tally("tamed", creature), *n),
            Goal::Pick(plant, n) => (journal(Page::Plants, plant), *n),
            Goal::Mine(ore, n) => (journal(Page::Ores, ore), *n),
            Goal::Visit(n) => (self.journal.found(Page::Biomes) as u32, *n),
            Goal::Discover(page, n) => (self.journal.found(*page) as u32, *n),
            Goal::Level(skill, n) => ((self.level)(skill) as u32, *n as u32),
            Goal::Have(item, n) => ((self.carried)(item), *n),
            Goal::Home => (u32::from(self.home), 1),
            Goal::Built(structure, n) => (tally("built", structure), *n),
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
        for patch in files.iter().filter_map(|f| f.tracker) {
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
        Ok(Self { quests, tracker })
    }

    /// Checks every skill, item and tag the quests' goals and rewards name against the
    /// loaded skills and blocks, and writes each as the full name it found, so the server
    /// looks names up exactly. A name nothing has (a typo in a mod's quests.ron) is refused
    /// with the quest it is in, rather than leaving a quest that never finishes or a reward
    /// that never comes.
    pub fn resolve(&mut self, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<(), RegistryError> {
        for quest in &mut self.quests {
            let namespace = quest.name.split(':').next().unwrap_or("").to_owned();
            let refuse = |what: &str, name: &str| RegistryError(format!("quest {} names the {what} `{name}`, which no package has", quest.name));
            let skill = |name: &mut String| -> Result<(), RegistryError> {
                *name = skills.named(&namespace, name).map(|id| skills.get(id).name.clone()).ok_or_else(|| refuse("skill", name))?;
                Ok(())
            };
            // An item, `tag:TAG` (some block has it), or `any` where the goal counts anything.
            let item = |name: &mut String, any: bool| -> Result<(), RegistryError> {
                if any && name == "any" {
                    return Ok(());
                }
                if let Some(tag) = name.strip_prefix("tag:") {
                    return if blocks.ids().any(|id| blocks.def(id).has_tag(tag)) { Ok(()) } else { Err(refuse("tag", tag)) };
                }
                *name = item_name(blocks, &namespace, name).ok_or_else(|| refuse("item", name))?;
                Ok(())
            };
            for goal in quest.tasks.iter_mut().flatten() {
                match goal {
                    Goal::Level(name, _) => skill(name)?,
                    Goal::Have(name, _) => item(name, false)?,
                    Goal::Craft(name, _) | Goal::Cook(name, _) => item(name, true)?,
                    _ => {}
                }
            }
            for reward in &mut quest.rewards {
                match reward {
                    Reward::Xp(name, _) => skill(name)?,
                    Reward::Item(name, _) => item(name, false)?,
                }
            }
        }
        Ok(())
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

/// The item a quest in package `namespace` means by `name`: a full name as written, else
/// the package's own block of that name, else the one block of that name in any package.
fn item_name(blocks: &BlockRegistry, namespace: &str, name: &str) -> Option<String> {
    if name.contains(':') {
        return blocks.id(name).map(|_| name.to_owned());
    }
    let own = format!("{namespace}:{name}");
    if blocks.id(&own).is_some() {
        return Some(own);
    }
    let mut named = blocks.ids().map(|id| &blocks.def(id).name).filter(|full| full.rsplit(':').next() == Some(name));
    match (named.next(), named.next()) {
        (Some(only), None) => Some(only.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, home: true };
        // The tools were made before the quest was ever seen; the biomes visited too. With
        // a home already, the hearth follows the same moment.
        let done = quests.newly_done(&[], &progress);
        assert_eq!(done, ["base:tools", "base:settle", "base:hearth"]);
        let states = quests.states(&done, &progress);
        let fish = states.iter().find(|s| s.name == "base:fish").unwrap();
        assert_eq!((fish.done, fish.tasks.clone()), (false, vec![vec![(3, 5)], vec![(1, 10)]]));
        // Two more trout finish it.
        journal.record(&rules, Page::Fish, "base:raw_trout", 2, 3);
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, home: true };
        assert_eq!(quests.newly_done(&done, &progress), ["base:fish"]);
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

    fn named(blocks: &[(&str, &[&str])]) -> (BlockRegistry, SkillRegistry) {
        let defs = blocks.iter().map(|(name, tags)| crate::block::BlockDef { name: name.to_string(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() }).collect();
        let mut skills = SkillRegistry::default();
        let core = r#"(curve: RuneScape(max: 99), skills: [(name: "mining", title: "", description: ""), (name: "cooking", title: "", description: ""), (name: "fishing", title: "", description: "")])"#;
        skills.extend("core", core).unwrap();
        skills.extend("sky", r#"(skills: [(name: "gliding", title: "", description: ""), (name: "fishing", title: "", description: "")])"#).unwrap();
        (crate::block::BlockRegistry::new(defs).unwrap(), skills)
    }

    fn resolved(namespace: &str, quest: &str) -> Result<QuestRegistry, RegistryError> {
        let (blocks, skills) = named(&[("base:torch", &[]), ("base:oak_log", &["log"]), ("base:stone_pickaxe", &[]), ("base:stone_axe", &[]), ("sky:torch", &[]), ("sky:cloud", &[])]);
        let source = format!("(quests: [{quest}])");
        let mut quests = QuestRegistry::new(vec![QuestRegistry::read(namespace, &source).unwrap()])?;
        quests.resolve(&blocks, &skills)?;
        Ok(quests)
    }

    #[test]
    fn a_skill_item_or_tag_no_package_has_is_refused_naming_the_quest() {
        let quest = |tasks: &str, rewards: &str| format!(r#"(name: "dig", age: "nomad", branch: "main", title: "", why: "", tasks: [[{tasks}]], rewards: [{rewards}])"#);
        let refused = |tasks: &str, rewards: &str| resolved("base", &quest(tasks, rewards)).err().map(|e| e.0);
        assert_eq!(refused(r#"Level("minning", 10)"#, ""), Some("quest base:dig names the skill `minning`, which no package has".to_owned()));
        assert_eq!(refused("Home", r#"Xp("cookin", 5.0)"#), Some("quest base:dig names the skill `cookin`, which no package has".to_owned()));
        assert_eq!(refused(r#"Have("tourch", 1)"#, ""), Some("quest base:dig names the item `tourch`, which no package has".to_owned()));
        assert_eq!(refused("Home", r#"Item("base:tourch", 1)"#), Some("quest base:dig names the item `base:tourch`, which no package has".to_owned()));
        assert_eq!(refused(r#"Have("tag:logs", 1)"#, ""), Some("quest base:dig names the tag `logs`, which no package has".to_owned()));
        assert_eq!(refused(r#"Craft("stone_shovel", 1)"#, ""), Some("quest base:dig names the item `stone_shovel`, which no package has".to_owned()));
        // `any` counts anything only where a goal counts things made; nobody carries "any".
        assert!(refused(r#"Cook("any", 1)"#, "").is_none());
        assert!(refused(r#"Have("any", 1)"#, "").is_some());
        // Goals the loaded blocks and skills cannot check (catches, kills, visits) pass as written.
        assert!(refused(r#"Catch("Kinds", 3), Visit(2)"#, "").is_none());
    }

    #[test]
    fn short_names_become_the_full_names_the_server_looks_up() {
        let base = resolved("base", r#"(name: "dig", age: "nomad", branch: "main", title: "", why: "", tasks: [[Level("mining", 5), Have("tag:log", 2), Craft("stone_pickaxe", 1)]], rewards: [Xp("cooking", 9.0), Item("stone_axe", 1), Item("torch", 2)])"#).unwrap();
        let dig = base.get("base:dig").unwrap();
        assert_eq!(dig.tasks, vec![vec![Goal::Level("core:mining".into(), 5), Goal::Have("tag:log".into(), 2), Goal::Craft("base:stone_pickaxe".into(), 1)]]);
        assert_eq!(dig.rewards, vec![Reward::Xp("core:cooking".into(), 9.0), Reward::Item("base:stone_axe".into(), 1), Reward::Item("base:torch".into(), 2)]);
        // A package's own skill and block of a name come before core's and others'; a block
        // only one package has is that one; a name two others share is refused, not guessed.
        let sky = resolved("sky", r#"(name: "fly", age: "nomad", branch: "main", title: "", why: "", tasks: [[Level("fishing", 2), Level("gliding", 2), Have("torch", 1), Have("stone_axe", 1)]])"#).unwrap();
        assert_eq!(sky.get("sky:fly").unwrap().tasks[0], vec![Goal::Level("sky:fishing".into(), 2), Goal::Level("sky:gliding".into(), 2), Goal::Have("sky:torch".into(), 1), Goal::Have("base:stone_axe".into(), 1)]);
        let elsewhere = r#"(name: "far", age: "nomad", branch: "main", title: "", why: "", tasks: [[Have("torch", 1)]])"#;
        assert!(resolved("deep", elsewhere).is_err());
    }

    #[test]
    fn a_quest_after_one_that_does_not_exist_is_refused() {
        let source = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "", why: "", tasks: [[Home]], after: ["nope"])])"#;
        assert!(QuestRegistry::new(vec![QuestRegistry::read("base", source).unwrap()]).is_err());
    }
}
