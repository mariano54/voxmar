//! The recipe book: every way to get an item and everything an item goes into, as a player
//! looks them up (hover an item, R for its recipes, U for its uses, as in NEI). It is read
//! from the same content the game plays by (crafting recipes, cooking, what kept
//! creatures give and what hunted ones drop), so a package's recipes appear in it with no
//! more work.

use rustc_hash::FxHashMap;

use crate::block::BlockId;
use crate::content::Content;
use crate::craft::GRID;
use crate::creature::CreatureId;

/// How an item is got.
#[derive(Clone, Debug, PartialEq)]
pub enum Method {
    /// Laid into the crafting grid, cell by cell, row by row; shapeless recipes take
    /// their ingredients in any arrangement. A family of recipes making the same thing
    /// from different woods (or stones) is one entry with each way laid out, as NEI
    /// cycles them.
    Craft { variants: Vec<[Option<BlockId>; GRID]>, shapeless: bool },
    /// Cooked at a station (blocks with the station's tag), from Cooking `level`; what is
    /// left when it burns, if anything.
    Cook { input: BlockId, station: String, level: u8, burnt: Option<BlockId> },
    /// Taken from a kept creature with a tool (items with the tool's tag), or left by it
    /// without one (eggs).
    Product { creature: CreatureId, tool: Option<String> },
    /// Dropped by a creature when it dies, `chance` of the time.
    Drop { creature: CreatureId, chance: f32 },
}

/// One way to get an item: how, what comes out, and how many (at least, at most).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub method: Method,
    pub output: BlockId,
    pub count: (u8, u8),
}

/// Every entry, found by what it makes and by what it uses.
#[derive(Debug, Default)]
pub struct RecipeBook {
    entries: Vec<Entry>,
    makes: FxHashMap<BlockId, Vec<usize>>,
    uses: FxHashMap<BlockId, Vec<usize>>,
}

impl RecipeBook {
    pub fn new(content: &Content) -> Self {
        let mut book = Self::default();
        for recipe in content.recipes.iter() {
            let cells = recipe.cells();
            let items: Vec<BlockId> = cells.iter().flatten().copied().collect();
            // The same thing from the same shape: another variant of the entry before.
            if let Some(last) = book.entries.last_mut()
                && last.output == recipe.output
                && last.count == (recipe.count, recipe.count)
                && let Method::Craft { variants, shapeless } = &mut last.method
                && *shapeless == recipe.shapeless()
                && variants[0].map(|c| c.is_some()) == cells.map(|c| c.is_some())
            {
                variants.push(cells);
                let index = book.entries.len() - 1;
                for item in items {
                    let uses = book.uses.entry(item).or_default();
                    if uses.last() != Some(&index) {
                        uses.push(index);
                    }
                }
                continue;
            }
            let method = Method::Craft { variants: vec![cells], shapeless: recipe.shapeless() };
            book.add(Entry { method, output: recipe.output, count: (recipe.count, recipe.count) }, items);
        }
        for (input, recipe) in content.cooking.iter() {
            let station = recipe.def.station.clone();
            let stations: Vec<BlockId> = content.blocks.tagged(&station).collect();
            let method = Method::Cook { input, station, level: recipe.def.level, burnt: recipe.burnt };
            book.add(Entry { method, output: recipe.output, count: (1, 1) }, [input].into_iter().chain(stations).collect());
        }
        for (id, creature) in content.creatures.iter() {
            for (product, &item) in creature.def.products.iter().zip(&creature.products) {
                let tools: Vec<BlockId> = product.tool.as_deref().map(|tag| content.blocks.tagged(tag).collect()).unwrap_or_default();
                let method = Method::Product { creature: id, tool: product.tool.clone() };
                book.add(Entry { method, output: item, count: product.count }, tools);
            }
            for drop in &creature.loot {
                let method = Method::Drop { creature: id, chance: drop.loot.chance };
                book.add(Entry { method, output: drop.item, count: drop.loot.count }, Vec::new());
            }
        }
        book
    }

    fn add(&mut self, entry: Entry, mut uses: Vec<BlockId>) {
        let index = self.entries.len();
        self.makes.entry(entry.output).or_default().push(index);
        uses.sort();
        uses.dedup();
        for item in uses {
            self.uses.entry(item).or_default().push(index);
        }
        self.entries.push(entry);
    }

    /// Every way to get `item`.
    pub fn making(&self, item: BlockId) -> Vec<&Entry> {
        self.makes.get(&item).map_or_else(Vec::new, |found| found.iter().map(|&i| &self.entries[i]).collect())
    }

    /// Everything `item` goes into: as an ingredient, a station or a tool.
    pub fn using(&self, item: BlockId) -> Vec<&Entry> {
        self.uses.get(&item).map_or_else(Vec::new, |found| found.iter().map(|&i| &self.entries[i]).collect())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads")
    }

    #[test]
    fn every_guide_icon_is_an_item() {
        let content = content();
        for id in content.skills.ids() {
            let skill = content.skills.get(id);
            for step in &skill.guide {
                if let Some(icon) = &step.icon {
                    let found = content.blocks.id(icon).or_else(|| content.blocks.id(&format!("base:{icon}")));
                    assert!(found.is_some(), "{}'s guide line {:?} names `{icon}`, which is no item", skill.name, step.text);
                }
            }
        }
    }

    #[test]
    fn the_game_content_reads_both_ways() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        // Planks are crafted from a log; the log goes into planks and the campfire.
        let planks = book.making(id("base:oak_planks"));
        assert!(planks.iter().any(|e| matches!(&e.method, Method::Craft { variants, .. } if variants[0].contains(&Some(id("base:oak_log"))))), "{planks:?}");
        // A campfire from any wood is one entry, its woods its variants.
        let campfires = book.making(id("base:campfire"));
        assert_eq!(campfires.len(), 1, "{campfires:?}");
        let log_uses: Vec<BlockId> = book.using(id("base:oak_log")).iter().map(|e| e.output).collect();
        assert!(log_uses.contains(&id("base:oak_planks")) && log_uses.contains(&id("base:campfire")), "{log_uses:?}");
        // The campfire is used as a station; cooked food is made there.
        assert!(book.using(id("base:campfire")).iter().any(|e| matches!(e.method, Method::Cook { .. })));
        // Shears are used on a creature for its product.
        assert!(book.using(id("base:shears")).iter().any(|e| matches!(&e.method, Method::Product { tool: Some(tool), .. } if tool == "shears")));
    }
}
