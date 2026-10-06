//! The journal: every kind of thing a player can discover, and how far they have come
//! with each (Mariano's draft 21, 2026-10-06). The server records discoveries as they
//! happen and saves them with the player; the interface draws the journal screen from them,
//! an outline for anything not yet found.
//!
//! An entry is seen when first found (a creature in sight, a plant picked, a biome stood
//! in, a fish caught, an ore mined); doing more of the same (kills, catches, picks) counts
//! up to studied and mastered, at thresholds each page sets in `journal.ron`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::block::RegistryError;

/// A page of the journal: a kind of thing to discover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Page {
    /// Every animal, wild, tamed or bred (creature names).
    Animals,
    /// Monsters and spirits (creature names).
    Monsters,
    Bosses,
    /// Named breeds from genes.
    Breeds,
    /// Flowers, herbs, fungi, crops and wonders (block names).
    Plants,
    /// Fish caught (item names).
    Fish,
    /// Biomes stood in (biome names).
    Biomes,
    /// Caves, ruins, villages and wonders entered.
    Places,
    /// Rare weather, moon phases and constellations witnessed.
    Sky,
    /// Ores, gems and rocks mined (block names).
    Ores,
    /// Every drop of every creature and boss (item names).
    Collection,
    /// Music heard in the world.
    Music,
    Books,
    /// Trades, deeds and sequence breaks.
    Discoveries,
}

impl Page {
    pub const ALL: [Page; 14] = [
        Page::Animals,
        Page::Monsters,
        Page::Bosses,
        Page::Breeds,
        Page::Plants,
        Page::Fish,
        Page::Biomes,
        Page::Places,
        Page::Sky,
        Page::Ores,
        Page::Collection,
        Page::Music,
        Page::Books,
        Page::Discoveries,
    ];
}

/// How much of an entry the journal shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Stage {
    /// The picture and the name.
    #[default]
    Seen,
    /// Where and when, and how it behaves.
    Studied,
    /// Everything: every drop with its rate, every property.
    Mastered,
}

/// One discovered thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub page: Page,
    /// The content name: `base:deer`, `base:poppy`, `base:forest`.
    pub name: String,
    pub stage: Stage,
    /// Times done since: killed, caught, picked, mined.
    pub count: u32,
    /// The world tick it was first found.
    pub found: u64,
}

/// What it takes to study and master an entry on each page (`journal.ron`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRules {
    pub pages: Vec<PageRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRule {
    pub page: Page,
    /// Count at which an entry is studied, and mastered; none for a page whose entries
    /// are studied some other way (a codex instrument, a book).
    #[serde(default)]
    pub studied: Option<u32>,
    #[serde(default)]
    pub mastered: Option<u32>,
}

impl JournalRules {
    pub fn read(namespace: &str, source: &str) -> Result<JournalRules, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/journal.ron: {e}")))
    }

    /// Several packages' rules: a later package's rule for a page replaces an earlier one.
    pub fn merge(files: Vec<JournalRules>) -> JournalRules {
        let mut pages: Vec<PageRule> = Vec::new();
        for rule in files.into_iter().flat_map(|f| f.pages) {
            pages.retain(|r| r.page != rule.page);
            pages.push(rule);
        }
        JournalRules { pages }
    }

    /// The stage `count` reaches on `page`.
    pub fn stage(&self, page: Page, count: u32) -> Stage {
        let Some(rule) = self.pages.iter().find(|r| r.page == page) else { return Stage::Seen };
        if rule.mastered.is_some_and(|n| count >= n) {
            Stage::Mastered
        } else if rule.studied.is_some_and(|n| count >= n) {
            Stage::Studied
        } else {
            Stage::Seen
        }
    }
}

/// A player's journal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    entries: BTreeMap<(Page, String), Entry>,
}

impl Journal {
    pub fn from_entries(entries: impl IntoIterator<Item = Entry>) -> Self {
        Self { entries: entries.into_iter().map(|e| ((e.page, e.name.clone()), e)).collect() }
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    pub fn get(&self, page: Page, name: &str) -> Option<&Entry> {
        self.entries.get(&(page, name.to_owned()))
    }

    /// How many entries a page has.
    pub fn found(&self, page: Page) -> usize {
        self.entries.range((page, String::new())..).take_while(|((p, _), _)| *p == page).count()
    }

    /// Records finding `name` on `page` and doing it `times` more (0 for only seeing it).
    /// Returns the entry when it is new or reached a further stage, for the player to hear
    /// of; a count going up alone returns nothing.
    pub fn record(&mut self, rules: &JournalRules, page: Page, name: &str, times: u32, tick: u64) -> Option<&Entry> {
        let key = (page, name.to_owned());
        let new = !self.entries.contains_key(&key);
        let entry = self.entries.entry(key).or_insert_with(|| Entry { page, name: name.to_owned(), stage: Stage::Seen, count: 0, found: tick });
        entry.count = entry.count.saturating_add(times);
        let stage = rules.stage(page, entry.count).max(entry.stage);
        let raised = stage > entry.stage;
        entry.stage = stage;
        (new || raised).then_some(&*entry)
    }

    /// Raises an entry to at least `stage` (a codex instrument, a book), finding it if new.
    pub fn study(&mut self, page: Page, name: &str, stage: Stage, tick: u64) -> Option<&Entry> {
        let key = (page, name.to_owned());
        let new = !self.entries.contains_key(&key);
        let entry = self.entries.entry(key).or_insert_with(|| Entry { page, name: name.to_owned(), stage: Stage::Seen, count: 0, found: tick });
        let raised = stage > entry.stage;
        entry.stage = entry.stage.max(stage);
        (new || raised).then_some(&*entry)
    }

    /// Takes in entries the server sent, replacing those it had.
    pub fn merge(&mut self, entries: impl IntoIterator<Item = Entry>) {
        for entry in entries {
            self.entries.insert((entry.page, entry.name.clone()), entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> JournalRules {
        JournalRules::read("core", "(pages: [(page: Fish, studied: 10, mastered: 100), (page: Biomes)])").unwrap()
    }

    #[test]
    fn entries_are_seen_then_studied_and_mastered_by_count() {
        let rules = rules();
        let mut journal = Journal::default();
        let first = journal.record(&rules, Page::Fish, "base:trout", 1, 5).cloned().unwrap();
        assert_eq!((first.stage, first.count, first.found), (Stage::Seen, 1, 5));
        for _ in 0..8 {
            assert!(journal.record(&rules, Page::Fish, "base:trout", 1, 6).is_none(), "a count going up says nothing");
        }
        assert_eq!(journal.record(&rules, Page::Fish, "base:trout", 1, 7).map(|e| e.stage), Some(Stage::Studied));
        journal.record(&rules, Page::Fish, "base:trout", 90, 8);
        assert_eq!(journal.get(Page::Fish, "base:trout").unwrap().stage, Stage::Mastered);
        // Biomes have no count thresholds: standing in one over and over keeps it seen.
        assert!(journal.record(&rules, Page::Biomes, "base:forest", 0, 9).is_some());
        assert!(journal.record(&rules, Page::Biomes, "base:forest", 0, 10).is_none());
        assert_eq!(journal.found(Page::Biomes), 1);
        assert_eq!(journal.found(Page::Fish), 1);
        assert_eq!(journal.found(Page::Animals), 0);
    }

    #[test]
    fn studying_raises_but_never_lowers() {
        let mut journal = Journal::default();
        assert!(journal.study(Page::Animals, "base:deer", Stage::Studied, 1).is_some());
        assert!(journal.study(Page::Animals, "base:deer", Stage::Seen, 2).is_none());
        assert_eq!(journal.get(Page::Animals, "base:deer").unwrap().stage, Stage::Studied);
    }
}
