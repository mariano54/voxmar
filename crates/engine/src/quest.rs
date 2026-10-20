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
//!
//! Finishing a quest does not pay it: the rewards wait for the player to claim them in the
//! quest screen (`QuestState::claimed`), unless the quest pays nothing.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::creature::CreatureRegistry;
use crate::fishing::FishingRegistry;
use crate::journal::{Journal, Page};
use crate::multiblock::MultiblockRegistry;
use crate::skill::SkillRegistry;

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
    /// Drew this many wild creatures (a creature, or `any`) up close by holding their
    /// favourite food: each creature counts once.
    Lured(String, u32),
    /// Picked or harvested this many of a plant (or `any`).
    Pick(String, u32),
    /// Mined this many of an ore or rock (or `any`).
    Mine(String, u32),
    /// Stood in this many different biomes.
    Visit(u32),
    /// Found this many entries on a journal page.
    Discover(Page, u32),
    /// Done this many different kinds of a journal page's entries at least once each (killed,
    /// caught, picked, mined; only seen does not count), among `any`, a `tag:TAG` or one
    /// name: `Kinds(Plants, "any", 5)` is five different plants picked.
    Kinds(Page, String, u32),
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
    /// Had dealings with this many parties of a wandering kind (or `any`), or with this many
    /// different kinds (`Kinds`): a line said that gave or took something, a show watched, a
    /// share of a hunt, a captive's or a guided one's thanks, a lure followed to its end. A
    /// greeting alone is no dealing.
    Met(String, u32),
    /// Turned a prayer (or `any`) on this many times.
    Prayed(String, u32),
    /// Offered this many of an item (or `any`, or a `tag:TAG`) at an altar.
    Offered(String, u32),
    /// Opened this many locked chests (a chest block, `any`, or a `tag:TAG`) by picking the lock.
    Opened(String, u32),
    /// Grew this many crops by spreading a fertiliser on them (an item, `any`, or a
    /// `tag:TAG`): one compost on three crops is three.
    Fertilised(String, u32),
    /// Ate this many of a food (an item, `any`, or a `tag:TAG`).
    Ate(String, u32),
    /// Was under an effect (by name) at least once, whatever gave it: a food, smoke, a
    /// home's comfort.
    Felt(String),
    /// Laid a recipe for this item (or `any`, or a `tag:TAG`) into the crafting grid this
    /// many times with the recipe viewer's Lay, taking from a store in the home.
    Laid(String, u32),
    /// Set down this many blocks that water crops (a block, `any` or a `tag:TAG`, such as a
    /// well) by tilled ground, near enough to keep it moist.
    Watering(String, u32),
}

impl Goal {
    /// The item (a name, `tag:TAG` or `any`) a goal asks the player to make, carry, get or
    /// use, if it asks for one.
    pub fn item(&self) -> Option<&str> {
        match self {
            Goal::Catch(item, _) if item == "Kinds" => None,
            Goal::Craft(item, _)
            | Goal::Cook(item, _)
            | Goal::CookAt(item, _, _)
            | Goal::Catch(item, _)
            | Goal::Pick(item, _)
            | Goal::Mine(item, _)
            | Goal::Have(item, _)
            | Goal::LookUp(item)
            | Goal::Offered(item, _)
            | Goal::Ate(item, _)
            | Goal::Laid(item, _)
            | Goal::Watering(item, _)
            | Goal::Fertilised(item, _) => Some(item),
            _ => None,
        }
    }
}

/// The quests with a goal that names `item` (by name, short name or one of its tags; a
/// goal for `any` names nothing), in the book's order: what the recipe viewer lists as
/// asking for it.
pub fn asking<'a>(quests: &'a QuestRegistry, blocks: &crate::block::BlockRegistry, item: crate::block::BlockId) -> Vec<&'a QuestDef> {
    let def = blocks.def(item);
    let names = |want: &str| match want.strip_prefix("tag:") {
        Some(tag) => def.has_tag(tag),
        None => def.name == want || def.name.rsplit(':').next() == Some(want),
    };
    quests.iter().filter(|quest| quest.tasks.iter().flatten().filter_map(Goal::item).any(names)).collect()
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
    /// Where it was read from, `package/file`, for the load checks to name.
    #[serde(skip)]
    pub source: String,
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
    #[serde(default)]
    pub follow_on: Option<bool>,
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
    /// Whether a pinned quest that drops off done leaves the quests it opens pinned in its
    /// place, so what to do next is always on screen.
    pub follow_on: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Self { pins: 3, done_stays: 5.0, backing_alpha: 0.5, follow_on: true }
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
    /// Tallies by key: `crafted:<item>`, `cooked:<item>`, `tamed:<creature>`, `met:<event>`,
    /// `prayed:<prayer>`, `offered:<item>`.
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
            Goal::Kill(creature, n) => (journal(Page::Animals, creature) + journal(Page::Monsters, creature) + journal(Page::Bosses, creature), *n),
            Goal::Tame(creature, n) => (tally("tamed", creature), *n),
            Goal::Lured(creature, n) => (tally("lured", creature), *n),
            Goal::Pick(plant, n) => (journal(Page::Plants, plant), *n),
            Goal::Mine(ore, n) => {
                let mined = self.journal.entries().filter(|e| e.page == Page::Ores && (matches(ore, &e.name) || (self.drops)(&e.name).is_some_and(|d| matches(ore, &d))));
                (mined.map(|e| e.count).sum(), *n)
            }
            Goal::Visit(n) => (self.journal.found(Page::Biomes) as u32, *n),
            Goal::Discover(page, n) => (self.journal.found(*page) as u32, *n),
            Goal::Kinds(page, want, n) => (self.journal.entries().filter(|e| e.page == *page && e.count > 0 && matches(want, &e.name)).count() as u32, *n),
            Goal::Level(skill, n) => ((self.level)(skill) as u32, *n as u32),
            Goal::Have(item, n) => ((self.carried)(item), *n),
            Goal::Home => (u32::from(self.home), 1),
            Goal::Built(structure, n) => (tally("built", structure), *n),
            Goal::Lit(structure, n) => (tally("lit", structure), *n),
            Goal::LookUp(item) => (tally("lookedup", item).min(1), 1),
            Goal::Thriving(n, percent) => ((self.thriving)(*percent), *n),
            Goal::Met(kind, n) if kind == "Kinds" => (self.tallies.iter().filter(|&(key, &times)| times > 0 && key.starts_with("met:")).count() as u32, *n),
            Goal::Met(kind, n) => (tally("met", kind), *n),
            Goal::Prayed(prayer, n) => (tally("prayed", prayer), *n),
            Goal::Offered(item, n) => (tally("offered", item), *n),
            Goal::Opened(chest, n) => (tally("opened", chest), *n),
            Goal::Fertilised(item, n) => (tally("fertilised", item), *n),
            Goal::Ate(food, n) => (tally("ate", food), *n),
            Goal::Felt(effect) => (tally("felt", effect).min(1), 1),
            Goal::Laid(item, n) => (tally("laid", item), *n),
            Goal::Watering(block, n) => (tally("watering", block), *n),
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
    /// Done and paid out. A done quest with rewards waits to be claimed in the quest screen
    /// (`ClientMessage::ClaimQuest`); one with none is claimed the moment it is done.
    pub claimed: bool,
    /// For each task, each goal's progress: (have, need).
    pub tasks: Vec<Vec<(u32, u32)>>,
}

impl QuestDef {
    /// Whether finishing it pays anything out, to be claimed.
    pub fn pays(&self) -> bool {
        !self.rewards.is_empty()
    }
}

impl QuestRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<QuestFile, RegistryError> {
        Self::read_named(namespace, "quests.ron", source)
    }

    /// One of a package's quest files, `quests.ron` or a `quests_<topic>.ron`, named in its
    /// errors.
    pub fn read_named(namespace: &str, file: &str, source: &str) -> Result<QuestFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let from = format!("{namespace}/{file}");
        let mut file: QuestFile = options.from_str(source).map_err(|e| RegistryError(format!("{from}: {e}")))?;
        file.source = from;
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
            // A reward item's full name is written when the content loads (`resolve`), which also
            // lets a mod's quest name a base item without its package.
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
        // The files that last changed the tracker and set the start pins, for their errors.
        let (mut tracker_from, mut pins_from) = ("quests.ron", "quests.ron");
        for (patch, source) in files.iter().filter_map(|f| Some((f.tracker.as_ref()?, f.source.as_str()))) {
            tracker_from = source;
            if let Some(names) = &patch.start_pins {
                start_pins = names.clone();
                pins_from = source;
            }
            tracker.pins = patch.pins.unwrap_or(tracker.pins);
            tracker.done_stays = patch.done_stays.unwrap_or(tracker.done_stays);
            tracker.backing_alpha = patch.backing_alpha.unwrap_or(tracker.backing_alpha);
            tracker.follow_on = patch.follow_on.unwrap_or(tracker.follow_on);
        }
        if tracker.pins == 0 || !(tracker.done_stays >= 0.0) || !(0.0..=1.0).contains(&tracker.backing_alpha) {
            return Err(RegistryError(format!("{tracker_from}: the tracker needs a pin or more, a stay of 0 s or more and a backing from 0 to 1, not {tracker:?}")));
        }
        let (tracker_from, pins_from) = (tracker_from.to_owned(), pins_from.to_owned());
        // Each quest with the file it was written in.
        let written: Vec<(QuestDef, String)> = files.into_iter().flat_map(|f| f.quests.into_iter().map(move |q| (q, f.source.clone()))).collect();
        // One book or several files, a quest is written once: a second of a name, in the
        // same file or another (another package's too, so a mod cannot replace a quest by
        // writing its name), is refused rather than shown twice.
        let mut first = FxHashMap::default();
        for (quest, source) in &written {
            if let Some(earlier) = first.insert(quest.name.as_str(), source.as_str()) {
                return Err(RegistryError(format!("quest {} is written in {earlier} and again in {source}", quest.name)));
            }
        }
        for (quest, source) in &written {
            if quest.tasks.is_empty() {
                return Err(RegistryError(format!("{source}: quest {} has no task", quest.name)));
            }
            // A task is its parts, all to be done; one with none would be done at once.
            if quest.tasks.iter().any(Vec::is_empty) {
                return Err(RegistryError(format!("{source}: quest {} has a task with no part", quest.name)));
            }
            for name in quest.after.iter().flat_map(|a| a.split('|')) {
                if !first.contains_key(name) {
                    return Err(RegistryError(format!("{source}: quest {} comes after `{name}`, which does not exist", quest.name)));
                }
            }
        }
        let quests: Vec<QuestDef> = written.into_iter().map(|(q, _)| q).collect();
        if start_pins.len() > tracker.pins {
            return Err(RegistryError(format!("{pins_from}: {} start pins, but the tracker follows {} ({tracker_from})", start_pins.len(), tracker.pins)));
        }
        if let Some(name) = start_pins.iter().find(|name| !quests.iter().any(|q| &q.name == *name)) {
            return Err(RegistryError(format!("{pins_from}: the start pin `{name}` is not a quest")));
        }
        Ok(Self { quests, tracker, start_pins })
    }

    /// The quests a new player's book starts with pinned.
    pub fn start_pins(&self) -> &[String] {
        &self.start_pins
    }

    /// Checks every name the quests' goals and rewards hold (skills, items, tags, fish,
    /// creatures, plants, ores, multiblocks) against the loaded content, and writes each as
    /// the full name it found, so the server looks names up exactly. A name nothing has (a
    /// typo in a mod's quests.ron) is refused with the quest it is in, rather than leaving a
    /// quest that never finishes or a reward that never comes. Journal pages are checked as
    /// the file is read.
    pub fn resolve(&mut self, named: &Named) -> Result<(), RegistryError> {
        let Named { blocks, skills, creatures, fishing, multiblocks, steps, events, prayers, effects, harvests } = *named;
        let block_ids: Vec<BlockId> = blocks.ids().collect();
        let fish_ids: Vec<BlockId> = fishing.iter().map(|c| c.item).collect();
        let names = |ids: &[BlockId]| ids.iter().map(|&id| blocks.def(id).name.as_str()).collect::<Vec<_>>();
        let (block_names, fish_names) = (names(&block_ids), names(&fish_ids));
        let creature_names: Vec<&str> = creatures.iter().map(|(_, c)| c.def.name.as_str()).collect();
        let tameable: Vec<&str> = creatures.iter().filter(|(_, c)| c.def.taming.is_some()).map(|(_, c)| c.def.name.as_str()).collect();
        let multiblock_names: Vec<&str> = multiblocks.iter().map(|m| m.name.as_str()).collect();
        let built_names: Vec<&str> = multiblock_names.iter().copied().chain(steps.iter().map(String::as_str)).collect();
        // What a goal's name may be: one of these, `any` where the goal counts anything, and
        // `tag:TAG` where one of these blocks has the tag.
        struct Kind<'a> {
            what: &'a str,
            names: &'a [&'a str],
            any: bool,
            tagged: &'a [BlockId],
        }
        let item = Kind { what: "item", names: &block_names, any: false, tagged: &block_ids };
        let made = Kind { any: true, ..item };
        let plant = Kind { what: "plant", ..made };
        let rock = Kind { what: "block", ..made };
        let fish = Kind { what: "fish", names: &fish_names, any: true, tagged: &fish_ids };
        let creature = Kind { what: "creature", names: &creature_names, any: true, tagged: &[] };
        let tame = Kind { what: "tameable creature", names: &tameable, ..creature };
        let multiblock = Kind { what: "multiblock", names: &multiblock_names, ..creature };
        let built = Kind { what: "multiblock or ladder step", names: &built_names, ..creature };
        let reward = Kind { tagged: &[], ..item };
        let event_names: Vec<&str> = events.iter().map(String::as_str).collect();
        let prayer_names: Vec<&str> = prayers.iter().map(String::as_str).collect();
        let event = Kind { what: "wandering kind", names: &event_names, ..creature };
        let prayer = Kind { what: "prayer", names: &prayer_names, ..creature };
        let effect_names: Vec<&str> = effects.iter().map(String::as_str).collect();
        let effect = Kind { what: "effect", names: &effect_names, any: false, tagged: &[] };
        for quest in &mut self.quests {
            let namespace = quest.name.split(':').next().unwrap_or("").to_owned();
            let refuse = |what: &str, name: &str| RegistryError(format!("quest {} names the {what} `{name}`, which no package has", quest.name));
            let skill = |name: &mut String| -> Result<(), RegistryError> {
                *name = skills.named(&namespace, name).map(|id| skills.get(id).name.clone()).ok_or_else(|| refuse("skill", name))?;
                Ok(())
            };
            let find = |name: &mut String, kind: &Kind| -> Result<(), RegistryError> {
                if kind.any && name == "any" {
                    return Ok(());
                }
                if let Some(tag) = name.strip_prefix("tag:")
                    && !kind.tagged.is_empty()
                {
                    return if kind.tagged.iter().any(|&id| blocks.def(id).has_tag(tag)) { Ok(()) } else { Err(refuse("tag", tag)) };
                }
                *name = full_name(kind.names, &namespace, name).ok_or_else(|| refuse(kind.what, name))?;
                Ok(())
            };
            for goal in quest.tasks.iter_mut().flatten() {
                match goal {
                    Goal::Level(name, _) => skill(name)?,
                    Goal::Have(name, _) => find(name, &item)?,
                    Goal::Craft(name, _) | Goal::Cook(name, _) => find(name, &made)?,
                    Goal::Pick(name, _) => find(name, &plant)?,
                    Goal::Mine(name, _) => find(name, &rock)?,
                    Goal::Catch(name, _) if name == "Kinds" => {}
                    Goal::Catch(name, _) => find(name, &fish)?,
                    Goal::Kill(name, _) | Goal::Lured(name, _) => find(name, &creature)?,
                    Goal::Tame(name, _) => find(name, &tame)?,
                    Goal::Built(name, _) => find(name, &built)?,
                    Goal::Lit(name, _) => find(name, &multiblock)?,
                    Goal::CookAt(name, _, _) => find(name, &made)?,
                    Goal::Thriving(..) => {}
                    Goal::LookUp(name) => find(name, &made)?,
                    Goal::Kinds(page, name, _) => match page {
                        Page::Plants => find(name, &plant)?,
                        Page::Ores => find(name, &rock)?,
                        Page::Fish => find(name, &fish)?,
                        Page::Animals | Page::Monsters => find(name, &creature)?,
                        _ if name == "any" => {}
                        _ => return Err(refuse("kind on that page (only `any` there)", name)),
                    },
                    Goal::Visit(_) | Goal::Discover(..) | Goal::Home => {}
                    Goal::Met(name, _) if name == "Kinds" => {}
                    Goal::Met(name, _) => find(name, &event)?,
                    Goal::Prayed(name, _) => find(name, &prayer)?,
                    Goal::Offered(name, _) => find(name, &made)?,
                    Goal::Opened(name, _) => find(name, &rock)?,
                    Goal::Fertilised(name, _) => find(name, &made)?,
                    Goal::Ate(name, _) => find(name, &made)?,
                    Goal::Felt(name) => find(name, &effect)?,
                    Goal::Laid(name, _) => find(name, &made)?,
                    Goal::Watering(name, _) => find(name, &made)?,
                }
            }
            for given in &mut quest.rewards {
                match given {
                    Reward::Xp(name, _) => skill(name)?,
                    Reward::Item(name, _) => find(name, &reward)?,
                }
            }
        }
        self.rewards_skip_no_lesson(blocks, harvests)
    }

    /// A reward is a gift, never the lesson: refuses rewards that alone hold another quest's
    /// `Have` goal while that quest may still be open when they are paid, so a quest that asks
    /// the player to gather or make a stack is never finished by a gift (the_potter's firewood
    /// under a clamp, Oct 20). What a giver pays of every item the goal takes is added up. A
    /// quest whose `after` makes it done first is passed, and so is one of an earlier age,
    /// whose lesson is learnt by the time a later age pays, and a task that also asks to pick,
    /// mine, make or catch the item (or a block that drops it), which still teaches it. An age is earlier than another when a
    /// quest of the other comes after one of it. A package answers for its own book: a mod's
    /// quest asking for sticks is not refused for a base reward of them.
    fn rewards_skip_no_lesson(&self, blocks: &BlockRegistry, harvests: &[(BlockId, BlockId)]) -> Result<(), RegistryError> {
        let firsts: FxHashMap<&str, rustc_hash::FxHashSet<&str>> = self.quests.iter().map(|q| (q.name.as_str(), self.done_before(q))).collect();
        let mut earlier: rustc_hash::FxHashSet<(&str, &str)> = rustc_hash::FxHashSet::default();
        for quest in &self.quests {
            for before in firsts[quest.name.as_str()].iter().filter_map(|name| self.get(name)) {
                if before.age != quest.age {
                    earlier.insert((before.age.as_str(), quest.age.as_str()));
                }
            }
        }
        if let Some((a, b)) = earlier.iter().find(|(a, b)| earlier.contains(&(*b, *a))) {
            return Err(RegistryError(format!("the {a} and {b} ages each have a quest that comes after one of the other's: an age's quests come after its own or an earlier age's")));
        }
        let package = |quest: &QuestDef| quest.name.split(':').next().unwrap_or("").to_owned();
        let holds = |want: &str, item: &str| match want.strip_prefix("tag:") {
            Some(tag) => blocks.item_named(item).is_some_and(|id| blocks.def(id).has_tag(tag)),
            None => want == item,
        };
        // The goals that have the player pick, mine, make or catch the item, which teach it as
        // a stack in hand does not; a block picked or mined teaches what it drops too, and one
        // picked what its harvest gives (a ripe crop's grain, a wild plant's forage).
        fn teaches(goal: &Goal) -> Option<&str> {
            match goal {
                Goal::Pick(w, _) | Goal::Mine(w, _) | Goal::Craft(w, _) | Goal::Cook(w, _) | Goal::CookAt(w, _, _) | Goal::Catch(w, _) => Some(w),
                _ => None,
            }
        }
        let dropped = |goal: &Goal, want: &str, item: &str| {
            let picked = matches!(goal, Goal::Pick(..));
            let gives = |b: BlockId, item: BlockId| blocks.info(b).drops == item || (picked && harvests.contains(&(b, item)));
            matches!(goal, Goal::Pick(..) | Goal::Mine(..))
                && blocks.item_named(item).is_some_and(|item| match want.strip_prefix("tag:") {
                    Some(tag) => blocks.tagged(tag).any(|b| gives(b, item)),
                    None => blocks.item_named(want).is_some_and(|b| gives(b, item)),
                })
        };
        for giver in &self.quests {
            let first = &firsts[giver.name.as_str()];
            let mut given: Vec<(&str, u32)> = Vec::new();
            for reward in &giver.rewards {
                if let Reward::Item(item, count) = reward {
                    match given.iter_mut().find(|(i, _)| i == item) {
                        Some((_, total)) => *total += u32::from(*count),
                        None => given.push((item, u32::from(*count))),
                    }
                }
            }
            let open = self.quests.iter().filter(|q| q.name != giver.name && package(q) == package(giver) && !first.contains(q.name.as_str()) && !earlier.contains(&(q.age.as_str(), giver.age.as_str())));
            for quest in open {
                for task in &quest.tasks {
                    for goal in task {
                        let Goal::Have(want, need) = goal else { continue };
                        let paid: Vec<&(&str, u32)> = given.iter().filter(|(item, _)| holds(want, item)).collect();
                        if paid.iter().map(|(_, n)| n).sum::<u32>() < *need {
                            continue;
                        }
                        let taught = task.iter().any(|g| teaches(g).is_some_and(|w| w == want || paid.iter().any(|(item, _)| holds(w, item) || dropped(g, w, item))));
                        if !taught {
                            let gifts = paid.iter().map(|(item, n)| format!("{n} {item}")).collect::<Vec<_>>().join(" and ");
                            return Err(RegistryError(format!(
                                "quest {} gives {gifts}, which alone holds {}'s Have({want}, {need}) while that quest may still be open: give fewer, or put it after {}",
                                giver.name, quest.name, quest.name
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The quests surely done before `quest` opens: those its `after` names, and theirs; of
    /// `"a|b"`, only what both ways have done first.
    fn done_before<'a>(&'a self, quest: &'a QuestDef) -> rustc_hash::FxHashSet<&'a str> {
        fn walk<'a>(quests: &'a QuestRegistry, quest: &'a QuestDef, seen: &mut Vec<&'a str>) -> rustc_hash::FxHashSet<&'a str> {
            let mut done = rustc_hash::FxHashSet::default();
            if seen.contains(&quest.name.as_str()) {
                return done;
            }
            seen.push(&quest.name);
            for after in &quest.after {
                let ways = after.split('|').filter_map(|name| quests.get(name)).map(|q| {
                    let mut first = walk(quests, q, seen);
                    first.insert(q.name.as_str());
                    first
                });
                if let Some(both) = ways.reduce(|a, b| a.intersection(&b).copied().collect()) {
                    done.extend(both);
                }
            }
            seen.pop();
            done
        }
        walk(self, quest, &mut Vec::new())
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
    pub fn states(&self, done: &[String], claimed: &[String], progress: &Progress) -> Vec<QuestState> {
        self.quests
            .iter()
            .map(|q| QuestState {
                name: q.name.clone(),
                done: done.contains(&q.name),
                claimed: claimed.contains(&q.name),
                tasks: q.tasks.iter().map(|t| t.iter().map(|g| progress.goal(g)).collect()).collect(),
            })
            .collect()
    }
}

/// What quests' names are checked against (`QuestRegistry::resolve`).
#[derive(Clone, Copy)]
pub struct Named<'a> {
    pub blocks: &'a BlockRegistry,
    pub skills: &'a SkillRegistry,
    pub creatures: &'a CreatureRegistry,
    pub fishing: &'a FishingRegistry,
    pub multiblocks: &'a MultiblockRegistry,
    /// The announced steps of effect ladders (`base:home_snug`), which `Built` may name too.
    pub steps: &'a [String],
    /// The wandering kinds' full names, which `Met` names.
    pub events: &'a [String],
    /// The prayers' full names, which `Prayed` names.
    pub prayers: &'a [String],
    /// The effects' full names, which `Felt` names.
    pub effects: &'a [String],
    /// What picking a block gives besides its drops, as (block, item): a ripe crop's harvest
    /// and a wild plant's forage, which a `Pick` goal teaches.
    pub harvests: &'a [(BlockId, BlockId)],
}

/// The one of `names` a quest in package `namespace` means by `name`: a full name as
/// written, else the package's own of that name, else the one of that name in any package
/// (two packages' are refused rather than guessed between).
fn full_name(names: &[&str], namespace: &str, name: &str) -> Option<String> {
    if name.contains(':') {
        return names.contains(&name).then(|| name.to_owned());
    }
    let own = format!("{namespace}:{name}");
    if names.contains(&own.as_str()) {
        return Some(own);
    }
    let mut named = names.iter().filter(|full| full.rsplit(':').next() == Some(name));
    match (named.next(), named.next()) {
        (Some(only), None) => Some((*only).to_owned()),
        _ => None,
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
    fn kinds_counts_different_entries_done_at_least_once_not_only_seen() {
        let rules = crate::journal::JournalRules::default();
        let mut journal = Journal::default();
        for (name, times) in [("base:poppy", 3), ("base:dandelion", 1), ("base:blueberry_bush", 1), ("base:fern", 0)] {
            journal.record(&rules, Page::Plants, name, times, 0);
        }
        journal.record(&rules, Page::Animals, "base:rabbit", 2, 0);
        let tallies = FxHashMap::default();
        let (level, carried, drops, thriving) = (|_: &str| 1, |_: &str| 0, |_: &str| None, |_: u8| 0);
        let has_tag = |name: &str, tag: &str| tag == "flower" && (name.ends_with("poppy") || name.ends_with("dandelion"));
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &thriving, home: false };
        assert_eq!(progress.goal(&Goal::Kinds(Page::Plants, "any".into(), 5)), (3, 5), "three picked; the fern was only seen, and a rabbit is no plant");
        assert_eq!(progress.goal(&Goal::Kinds(Page::Plants, "tag:flower".into(), 2)), (2, 2));
        assert_eq!(progress.goal(&Goal::Pick("any".into(), 5)), (5, 5), "a count of picks is not a count of kinds");
    }

    #[test]
    fn reward_items_keep_their_short_names_for_the_load_check_to_resolve() {
        let source = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "A", why: "", tasks: [[Home]], rewards: [Xp("cooking", 1.0), Item("torch", 4), Item("core:coin", 1)])])"#;
        let file = QuestRegistry::read("t", source).unwrap();
        assert_eq!(file.quests[0].rewards, vec![Reward::Xp("cooking".into(), 1.0), Reward::Item("torch".into(), 4), Reward::Item("core:coin".into(), 1)]);
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

    /// A book may be split over `quests.ron` and `quests_<topic>.ron` files: a quest in a
    /// later file may follow one in an earlier, the book reads in file order, and a quest
    /// written in two files is refused rather than shown twice.
    #[test]
    fn quests_split_over_files_join_in_order_and_are_written_once() {
        let first = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "A", why: "", tasks: [[Home]])])"#;
        let topic = r#"(quests: [(name: "b", age: "nomad", branch: "main", title: "B", why: "", tasks: [[Home]], after: ["a"])])"#;
        let read = |file: &str, source: &str| QuestRegistry::read_named("t", file, source).unwrap();
        let book = QuestRegistry::new(vec![read("quests.ron", first), read("quests_nomad.ron", topic)]).unwrap();
        assert_eq!(book.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(), ["t:a", "t:b"]);
        assert_eq!(book.get("t:b").unwrap().after, ["t:a"]);

        let again = r#"(quests: [(name: "a", age: "farm", branch: "main", title: "A Again", why: "", tasks: [[Home]])])"#;
        let twice = QuestRegistry::new(vec![read("quests.ron", first), read("quests_farm.ron", again)]).err().expect("a quest written in two files is refused");
        assert!(twice.0.contains("t:a is written in t/quests.ron and again in t/quests_farm.ron"), "{}", twice.0);
        let one_file = format!("(quests: [{0}, {0}])", r#"(name: "a", age: "nomad", branch: "main", title: "A", why: "", tasks: [[Home]])"#);
        let twice = QuestRegistry::new(vec![read("quests.ron", &one_file)]).err().expect("and so is one written twice in one file");
        assert!(twice.0.contains("in t/quests.ron and again in t/quests.ron"), "{}", twice.0);
        // Another package's quest of the same full name does not replace it: refused too.
        let theirs = r#"(quests: [(name: "t:a", age: "nomad", branch: "main", title: "Mine Now", why: "", tasks: [[Home]])])"#;
        let replaced = QuestRegistry::new(vec![read("quests.ron", first), QuestRegistry::read_named("zz", "quests.ron", theirs).unwrap()]).err().expect("a mod writing a base quest's name is refused");
        assert!(replaced.0.contains("t:a is written in t/quests.ron and again in zz/quests.ron"), "{}", replaced.0);
        // The other checks name the file the quest is in.
        let lost = r#"(quests: [(name: "c", age: "nomad", branch: "main", title: "C", why: "", tasks: [[Home]], after: ["nowhere"])])"#;
        let lost = QuestRegistry::new(vec![read("quests.ron", first), read("quests_c.ron", lost)]).err().expect("an `after` that names no quest");
        assert!(lost.0.starts_with("t/quests_c.ron: quest t:c comes after `t:nowhere`"), "{}", lost.0);

        let broken = QuestRegistry::read_named("t", "quests_farm.ron", "(quests: [").err().expect("a broken file is refused");
        assert!(broken.0.starts_with("t/quests_farm.ron:"), "the error names its file: {}", broken.0);
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
        let states = quests.states(&done, &done, &progress);
        // Done is not claimed: the states say which of the finished still wait.
        let waiting = quests.states(&done, &["base:tools".to_owned()], &progress);
        assert_eq!(waiting.iter().filter(|s| s.done && !s.claimed).map(|s| s.name.as_str()).collect::<Vec<_>>(), ["base:settle", "base:hearth"]);
        assert!(states.iter().filter(|s| s.done).all(|s| s.claimed));
        let fish = states.iter().find(|s| s.name == "base:fish").unwrap();
        assert_eq!((fish.done, fish.tasks.clone()), (false, vec![vec![(3, 5)], vec![(1, 10)]]));
        // Two more trout finish it.
        journal.record(&rules, Page::Fish, "base:raw_trout", 2, 3);
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: true };
        assert_eq!(quests.newly_done(&done, &progress), ["base:fish"]);
    }

    /// Dealings with wanderers count by kind or all together, and `Kinds` counts the kinds
    /// dealt with at least once; prayers and offerings count by name, offerings by tag too;
    /// opened chests count by chest or all together; food eaten by name or tag; an effect
    /// felt counts once, however often.
    #[test]
    fn wanderers_prayers_and_offerings_count_from_their_tallies() {
        let journal = Journal::default();
        let mut tallies = FxHashMap::default();
        for (key, n) in [("met:base:pedlar", 2), ("met:base:smith", 1), ("met:base:hermit", 0), ("prayed:base:bark_skin", 3), ("offered:base:bone", 4), ("crafted:base:bone", 9), ("opened:base:nomad_treasure_chest", 2), ("ate:base:honey", 3), ("felt:base:smoky", 5), ("fertilised:base:compost", 5), ("laid:base:oak_planks", 2), ("lured:base:rabbit", 2), ("watering:base:well", 1)] {
            tallies.insert(key.to_owned(), n);
        }
        let level = |_: &str| 1u8;
        let carried = |_: &str| 0u32;
        let has_tag = |name: &str, tag: &str| (name == "base:bone" && tag == "offering") || (name == "base:honey" && tag == "sweet");
        let drops = |_: &str| None;
        let progress = Progress { journal: &journal, tallies: &tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &|_| 0, home: false };
        let goal = |goal: Goal| progress.goal(&goal);
        assert_eq!(goal(Goal::Met("base:pedlar".into(), 1)), (2, 1));
        assert_eq!(goal(Goal::Met("pedlar".into(), 3)), (2, 3), "a short name is the kind's own");
        assert_eq!(goal(Goal::Met("any".into(), 4)), (3, 4));
        assert_eq!(goal(Goal::Met("Kinds".into(), 3)), (2, 3), "a kind never dealt with is no kind");
        assert_eq!(goal(Goal::Prayed("base:bark_skin".into(), 1)), (3, 1));
        assert_eq!(goal(Goal::Prayed("base:ember_arm".into(), 1)), (0, 1));
        assert_eq!(goal(Goal::Offered("base:bone".into(), 3)), (4, 3), "only what was offered, not what was made");
        assert_eq!(goal(Goal::Offered("tag:offering".into(), 3)), (4, 3));
        assert_eq!(goal(Goal::Opened("any".into(), 1)), (2, 1), "locked chests opened count by chest or all together");
        assert_eq!(goal(Goal::Opened("base:farm_treasure_chest".into(), 1)), (0, 1));
        assert_eq!(goal(Goal::Fertilised("base:compost".into(), 4)), (5, 4), "crops grown by compost spread");
        assert_eq!(goal(Goal::Fertilised("base:bone".into(), 1)), (0, 1));
        assert_eq!(goal(Goal::Ate("base:honey".into(), 2)), (3, 2));
        assert_eq!(goal(Goal::Ate("tag:sweet".into(), 4)), (3, 4), "food eaten counts by tag");
        assert_eq!(goal(Goal::Ate("base:bone".into(), 1)), (0, 1), "what was made is not what was eaten");
        assert_eq!(goal(Goal::Felt("base:smoky".into())), (1, 1), "felt once is enough");
        assert_eq!(goal(Goal::Laid("any".into(), 3)), (2, 3), "recipes laid from a home's stores");
        assert_eq!(goal(Goal::Laid("base:bone".into(), 1)), (0, 1), "what was made is not what was laid");
        assert_eq!(goal(Goal::Lured("any".into(), 1)), (2, 1), "creatures drawn up close by their favourite food");
        assert_eq!(goal(Goal::Lured("base:junglefowl".into(), 1)), (0, 1), "a rabbit lured is no junglefowl");
        assert_eq!(goal(Goal::Watering("base:well".into(), 1)), (1, 1), "a well set by the field");
        assert_eq!(goal(Goal::Watering("base:bone".into(), 1)), (0, 1));
        assert_eq!(goal(Goal::Felt("base:heartwood".into())), (0, 1));
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
        let mod_ = QuestRegistry::read("mod", "(tracker: (pins: 5, follow_on: false))").unwrap();
        assert_eq!(QuestRegistry::new(vec![core, mod_]).unwrap().tracker(), Tracker { pins: 5, done_stays: 3.0, backing_alpha: 0.4, follow_on: false });
        assert_eq!(registry().tracker(), Tracker::default());
        let none = QuestRegistry::read("core", "(tracker: (pins: 0))").unwrap();
        assert!(QuestRegistry::new(vec![none]).is_err());
        let clear = QuestRegistry::read("core", "(tracker: (backing_alpha: 1.5))").unwrap();
        assert!(QuestRegistry::new(vec![clear]).is_err());
    }

    /// Content to check quests against: blocks (a log tagged, fish, a kiln's pieces), core
    /// skills and sky's own, a tameable rabbit and a wild wolf, trout to catch and a kiln.
    fn resolved(namespace: &str, quest: &str) -> Result<QuestRegistry, RegistryError> {
        resolved_with(namespace, quest, &[])
    }

    /// `resolved`, with `more` blocks (name and tags) beside the usual ones.
    fn resolved_with(namespace: &str, quest: &str, more: &[(&str, &[&str])]) -> Result<QuestRegistry, RegistryError> {
        let blocks: &[(&str, &[&str])] = &[
            ("base:torch", &[]),
            ("base:oak_log", &["log"]),
            ("base:stone_pickaxe", &[]),
            ("base:stone_axe", &[]),
            ("base:copper_ore", &["ore"]),
            ("base:poppy", &["flower"]),
            ("base:raw_trout", &["fish"]),
            ("base:brick", &["multiblock_core"]),
            ("sky:torch", &[]),
            ("sky:cloud", &[]),
        ];
        let defs = blocks.iter().chain(more).map(|(name, tags)| crate::block::BlockDef { name: name.to_string(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() }).collect();
        let blocks = BlockRegistry::new(defs).unwrap();
        let mut skills = SkillRegistry::default();
        let core = r#"(curve: RuneScape(max: 99), skills: [(name: "mining", title: "", description: ""), (name: "cooking", title: "", description: ""), (name: "fishing", title: "", description: ""), (name: "hunter", title: "", description: "")])"#;
        skills.extend("core", core).unwrap();
        skills.extend("sky", r#"(skills: [(name: "gliding", title: "", description: ""), (name: "fishing", title: "", description: "")])"#).unwrap();
        let body = "body: (size: (0.6, 0.6), model: Blob(body: (0.5, 0.5, 0.5)), colours: [(1, 2, 3)])";
        let creatures = format!(r#"(creatures: [(name: "rabbit", health: 3, {body}, taming: (xp: 10.0)), (name: "wolf", health: 8, {body})])"#);
        let creatures = CreatureRegistry::new(CreatureRegistry::read("base", &creatures).unwrap().defs, Default::default(), &blocks, &skills).unwrap();
        let fishing = FishingRegistry::new(vec![FishingRegistry::read("base", r#"(catches: [(item: "raw_trout", level: 1, xp: 10.0)])"#).unwrap()], Default::default(), &blocks, &skills).unwrap();
        let kiln = r#"(multiblocks: [(name: "kiln", layers: [["bbb", "bBb", "bbb"]], key: {'b': "brick", 'B': "brick"}, core: 'B')])"#;
        let multiblocks = MultiblockRegistry::new(vec![MultiblockRegistry::read("base", kiln).unwrap()], &blocks).unwrap();
        let source = format!("(quests: [{quest}])");
        let mut quests = QuestRegistry::new(vec![QuestRegistry::read(namespace, &source).unwrap()])?;
        quests.resolve(&Named { blocks: &blocks, skills: &skills, creatures: &creatures, fishing: &fishing, multiblocks: &multiblocks, steps: &[], events: &["base:pedlar".to_owned()], prayers: &["base:bark_skin".to_owned()], effects: &["base:smoky".to_owned()], harvests: &[] })?;
        Ok(quests)
    }

    #[test]
    fn a_reward_never_holds_a_goal_of_a_quest_that_may_still_be_open() {
        let quest = |name: &str, age: &str, after: &str, tasks: &str, rewards: &str| format!(r#"(name: "{name}", age: "{age}", branch: "main", title: "", why: "", after: [{after}], tasks: [[{tasks}]], rewards: [{rewards}])"#);
        let stack = quest("stack", "nomad", "", r#"Have("torch", 8)"#, r#"Xp("mining", 1.0)"#);
        let book = |giver: &str| resolved("base", &format!("{stack}, {giver}")).err().map(|e| e.0);
        assert_eq!(
            book(&quest("gift", "nomad", "", "Home", r#"Item("torch", 8)"#)),
            Some("quest base:gift gives 8 base:torch, which alone holds base:stack's Have(base:torch, 8) while that quest may still be open: give fewer, or put it after base:stack".to_owned())
        );
        assert_eq!(book(&quest("gift", "nomad", "", "Home", r#"Item("torch", 4), Item("torch", 4)"#)).is_some(), true, "two rewards of one item add up");
        assert_eq!(book(&quest("gift", "nomad", "", "Home", r#"Item("torch", 7)"#)), None, "a share of the stack is a help, not the lesson");
        assert_eq!(book(&quest("gift", "nomad", r#""base:stack""#, "Home", r#"Item("torch", 8)"#)), None, "paid only once the stack quest is done");
        let wood = quest("wood", "nomad", "", r#"Have("tag:log", 4)"#, r#"Xp("mining", 1.0)"#);
        assert!(resolved("base", &format!("{wood}, {}", quest("gift", "nomad", "", "Home", r#"Item("oak_log", 4)"#))).is_err(), "a tagged goal is held by any item with the tag");
        let either = quest("either", "nomad", r#""base:stack|base:wood""#, "Home", r#"Item("torch", 8)"#);
        assert!(resolved("base", &format!("{stack}, {wood}, {either}")).is_err(), "of two ways in, the stack quest is done on only one");
        let (stacked, wooded) = (quest("stacked", "nomad", r#""base:stack""#, "Home", ""), quest("wooded", "nomad", r#""base:stack""#, "Home", ""));
        let both = quest("both", "nomad", r#""base:stacked|base:wooded""#, "Home", r#"Item("torch", 8)"#);
        assert!(resolved("base", &format!("{stack}, {stacked}, {wooded}, {both}")).is_ok(), "both ways in come after the stack quest");
        let logs = resolved_with("base", &format!("{wood}, {}", quest("gift", "nomad", "", "Home", r#"Item("oak_log", 2), Item("birch_log", 2)"#)), &[("base:birch_log", &["log"])]);
        assert_eq!(
            logs.err().map(|e| e.0),
            Some("quest base:gift gives 2 base:oak_log and 2 base:birch_log, which alone holds base:wood's Have(tag:log, 4) while that quest may still be open: give fewer, or put it after base:wood".to_owned()),
            "what a giver pays of every item a tag takes adds up"
        );
        let picked = quest("picked", "nomad", "", r#"Pick("poppy", 4), Have("poppy", 4)"#, r#"Xp("mining", 1.0)"#);
        assert!(resolved("base", &format!("{picked}, {}", quest("gift", "nomad", "", "Home", r#"Item("poppy", 4)"#))).is_ok(), "a task that asks to pick the item still teaches it");
        let patch = quest("patch", "nomad", "", r#"Pick("carrot_crop", 4), Have("carrot", 4)"#, r#"Xp("mining", 1.0)"#);
        let garden = quest("garden", "nomad", "", "Home", r#"Item("carrot", 4)"#);
        let with_drop = |drops: Option<&str>, harvested: bool| {
            let mut defs: Vec<crate::block::BlockDef> = ["base:carrot", "base:carrot_crop"].iter().map(|name| crate::block::BlockDef { name: name.to_string(), ..Default::default() }).collect();
            defs[1].drops = drops.map(str::to_owned);
            let blocks = BlockRegistry::new(defs).unwrap();
            let mut book = QuestRegistry::new(vec![QuestRegistry::read("base", &format!("(quests: [{patch}, {garden}])")).unwrap()]).unwrap();
            // Full names, as `resolve` writes them.
            for goal in book.quests.iter_mut().flat_map(|q| q.tasks.iter_mut().flatten()) {
                if let Goal::Pick(name, _) | Goal::Have(name, _) = goal {
                    *name = format!("base:{name}");
                }
            }
            for reward in book.quests.iter_mut().flat_map(|q| q.rewards.iter_mut()) {
                if let Reward::Item(name, _) = reward && !name.contains(':') {
                    *name = format!("base:{name}");
                }
            }
            let harvests = [(blocks.expect("base:carrot_crop"), blocks.expect("base:carrot"))];
            book.rewards_skip_no_lesson(&blocks, if harvested { &harvests } else { &[] })
        };
        assert!(with_drop(Some("base:carrot"), false).is_ok(), "picking the crop that drops the carrots teaches them");
        assert!(with_drop(None, true).is_ok(), "picking the crop whose harvest gives the carrots teaches them");
        assert!(with_drop(None, false).is_err(), "a crop that drops and harvests itself teaches no carrots");
        let looked = quest("looked", "nomad", "", r#"LookUp("poppy"), Have("poppy", 4)"#, r#"Xp("mining", 1.0)"#);
        assert!(resolved("base", &format!("{looked}, {}", quest("gift", "nomad", "", "Home", r#"Item("poppy", 4)"#))).is_err(), "looking the item up teaches no stack");
        // A later age read first: the book's file order is no age order.
        let (hearth, farm) = (quest("hearth", "nomad", "", "Home", ""), quest("farm", "farm", r#""base:hearth""#, r#"Have("torch", 8)"#, ""));
        let farm_gift = quest("farm_gift", "farm", r#""base:hearth""#, "Home", r#"Item("torch", 8)"#);
        let farm_first = quest("farm", "farm", r#""base:hearth""#, "Home", "");
        assert!(resolved("base", &format!("{farm_first}, {hearth}, {stack}, {farm_gift}")).is_ok(), "a Farm reward meets a Nomad stack whose lesson is learnt");
        let nomad_gift = quest("nomad_gift", "nomad", "", "Home", r#"Item("torch", 8)"#);
        assert!(resolved("base", &format!("{farm}, {hearth}, {nomad_gift}")).is_err(), "a Nomad reward still meets a Farm stack, wherever its file sorts");
        let back = quest("back", "nomad", r#""base:farm""#, "Home", "");
        assert!(resolved("base", &format!("{farm}, {hearth}, {back}")).unwrap_err().0.contains("each have a quest that comes after one of the other's"), "two ages after each other have no order");
        let gift = quest("gift", "nomad", "", "Home", r#"Item("base:torch", 8)"#);
        let quests = QuestRegistry::new(vec![QuestRegistry::read("base", &format!("(quests: [{stack}])")).unwrap(), QuestRegistry::read("sky", &format!("(quests: [{gift}])")).unwrap()]).unwrap();
        assert!(quests.rewards_skip_no_lesson(&BlockRegistry::new(vec![crate::block::BlockDef { name: "base:torch".into(), ..Default::default() }]).unwrap(), &[]).is_ok(), "another package's book is its own");
    }

    #[test]
    fn a_name_no_package_has_is_refused_naming_the_quest() {
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
        assert!(refused(r#"Kinds(Plants, "any", 5)"#, "").is_none() && refused(r#"Kinds(Animals, "rabbit", 2)"#, "").is_none());
        assert!(refused(r#"Kinds(Animals, "rabit", 2)"#, "").is_some(), "a kind is checked against the page's own things");
        assert!(refused(r#"Kinds(Biomes, "forest", 2)"#, "").is_some(), "a page with no named things takes `any` only");
        assert!(refused(r#"Have("any", 1)"#, "").is_some());
        assert_eq!(refused("Home", r#"Item("tag:log", 1)"#), Some("quest base:dig names the item `tag:log`, which no package has".to_owned()), "a reward is an item, never a tag");
        // Every other kind of goal, one typo each.
        assert_eq!(refused(r#"Catch("raw_trou", 1)"#, ""), Some("quest base:dig names the fish `raw_trou`, which no package has".to_owned()));
        assert_eq!(refused(r#"Catch("torch", 1)"#, ""), Some("quest base:dig names the fish `torch`, which no package has".to_owned()), "nothing catches torches");
        assert_eq!(refused(r#"Catch("tag:ore", 1)"#, ""), Some("quest base:dig names the tag `ore`, which no package has".to_owned()), "no fish is an ore");
        assert_eq!(refused(r#"Kill("wolff", 1)"#, ""), Some("quest base:dig names the creature `wolff`, which no package has".to_owned()));
        assert_eq!(refused(r#"Tame("wolf", 1)"#, ""), Some("quest base:dig names the tameable creature `wolf`, which no package has".to_owned()), "a wolf cannot be tamed");
        assert_eq!(refused(r#"Pick("popy", 1)"#, ""), Some("quest base:dig names the plant `popy`, which no package has".to_owned()));
        assert_eq!(refused(r#"Mine("coper_ore", 1)"#, ""), Some("quest base:dig names the block `coper_ore`, which no package has".to_owned()));
        assert_eq!(refused(r#"Mine("tag:ores", 1)"#, ""), Some("quest base:dig names the tag `ores`, which no package has".to_owned()));
        assert_eq!(refused(r#"Built("kil", 1)"#, ""), Some("quest base:dig names the multiblock or ladder step `kil`, which no package has".to_owned()));
        assert_eq!(refused(r#"Met("pedler", 1)"#, ""), Some("quest base:dig names the wandering kind `pedler`, which no package has".to_owned()));
        assert_eq!(refused(r#"Prayed("bark_skn", 1)"#, ""), Some("quest base:dig names the prayer `bark_skn`, which no package has".to_owned()));
        assert_eq!(refused(r#"Offered("bon", 1)"#, ""), Some("quest base:dig names the item `bon`, which no package has".to_owned()));
        assert_eq!(refused(r#"Opened("nomad_chest", 1)"#, ""), Some("quest base:dig names the block `nomad_chest`, which no package has".to_owned()));
        assert_eq!(refused(r#"Ate("hony", 1)"#, ""), Some("quest base:dig names the item `hony`, which no package has".to_owned()));
        assert_eq!(refused(r#"Felt("smokey")"#, ""), Some("quest base:dig names the effect `smokey`, which no package has".to_owned()));
        assert!(refused(r#"Felt("smoky"), Ate("torch", 1)"#, "").is_none(), "an effect by its short name");
        assert!(refused(r#"Met("pedlar", 1), Met("Kinds", 3), Met("any", 2), Prayed("bark_skin", 1), Offered("torch", 2)"#, "").is_none());
        // A page no journal has is refused as the file is read.
        let page = quest(r#"Discover(Spells, 1)"#, "");
        assert!(QuestRegistry::read("base", &format!("(quests: [{page}])")).is_err());
        // What they ask for spelled right, `any`, kinds of fish, visits and pages, pass.
        let fine = r#"Catch("Kinds", 3), Catch("raw_trout", 1), Catch("tag:fish", 1), Kill("wolf", 1), Kill("any", 2), Tame("rabbit", 1), Pick("tag:flower", 1), Mine("copper_ore", 1), Built("kiln", 1), Visit(2), Discover(Plants, 1)"#;
        assert_eq!(refused(fine, ""), None);
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
        let caught = resolved("base", r#"(name: "hunt", age: "nomad", branch: "main", title: "", why: "", tasks: [[Catch("raw_trout", 1), Kill("wolf", 1), Tame("rabbit", 1), Pick("poppy", 1), Mine("copper_ore", 1), Built("kiln", 1)]])"#).unwrap();
        let full = ["base:raw_trout", "base:wolf", "base:rabbit", "base:poppy", "base:copper_ore", "base:kiln"];
        let written: Vec<&str> = caught.get("base:hunt").unwrap().tasks[0]
            .iter()
            .map(|goal| match goal {
                Goal::Catch(n, _) | Goal::Kill(n, _) | Goal::Tame(n, _) | Goal::Pick(n, _) | Goal::Mine(n, _) | Goal::Built(n, _) => n.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(written, full);
        let elsewhere = r#"(name: "far", age: "nomad", branch: "main", title: "", why: "", tasks: [[Have("torch", 1)]])"#;
        assert!(resolved("deep", elsewhere).is_err());
    }

    #[test]
    fn a_quest_after_one_that_does_not_exist_is_refused() {
        let source = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "", why: "", tasks: [[Home]], after: ["nope"])])"#;
        assert!(QuestRegistry::new(vec![QuestRegistry::read("base", source).unwrap()]).is_err());
    }

    #[test]
    fn a_quest_with_an_empty_task_is_refused() {
        let quest = |tasks: &str| QuestRegistry::new(vec![QuestRegistry::read("base", &format!(r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "", why: "", tasks: {tasks})])"#)).unwrap()]).err().map(|e| e.0);
        assert_eq!(quest("[]"), Some("base/quests.ron: quest base:a has no task".into()));
        assert_eq!(quest("[[Home], []]"), Some("base/quests.ron: quest base:a has a task with no part".into()));
        assert_eq!(quest("[[Home, Craft(\"torch\", 1)]]"), None, "one task of two parts");
        assert!(QuestRegistry::read("base", r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "", why: "", tasks: [[Juggle(3)]])])"#).is_err(), "an unknown part");
    }
}
