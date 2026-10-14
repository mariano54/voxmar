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
    /// cycles them. `first` is the first of its recipes, in content order (its variants
    /// follow), for laying one into the grid. `near` is the tag of a block it is made
    /// near, if any.
    Craft { variants: Vec<[Option<BlockId>; GRID]>, shapeless: bool, first: usize, near: Option<String> },
    /// Cooked (or fired, or smelted) at a station (blocks with the station's tag), from
    /// `level` in `skill`: `uses` of the held `input` and the other items `with` it; what
    /// is left when it burns, if anything.
    Cook { input: BlockId, uses: u8, with: Vec<(BlockId, u8)>, station: String, skill: Option<crate::skill::SkillId>, level: u8, burnt: Option<BlockId> },
    /// Taken from a kept creature with a tool (items with the tool's tag), or left by it
    /// without one (eggs).
    Product { creature: CreatureId, tool: Option<String> },
    /// Dropped by a creature when it dies, `chance` of the time.
    Drop { creature: CreatureId, chance: f32 },
    /// Given in place of a block's own drop when it is broken (gravel's flint), `chance` of
    /// the time, maybe only with a kind of tool (tall grass's twine, with a knife): `tool`
    /// is the first of that kind, to show it by.
    Break { block: BlockId, tool: Option<BlockId>, chance: f32 },
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
        let blocks = &content.blocks;
        for (index, recipe) in content.recipes.iter().enumerate() {
            // A merged-away plant in a cell is shown as the item it merged into.
            let cells = recipe.cells().map(|cell| cell.map(|b| blocks.item_of(b)));
            let items: Vec<BlockId> = cells.iter().flatten().copied().collect();
            // The same thing from the same shape: another variant of the entry before.
            if let Some(last) = book.entries.last_mut()
                && last.output == recipe.output
                && last.count == (recipe.count, recipe.count)
                && let Method::Craft { variants, shapeless, near, .. } = &mut last.method
                && *shapeless == recipe.shapeless()
                && *near == recipe.near
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
            let method = Method::Craft { variants: vec![cells], shapeless: recipe.shapeless(), first: index, near: recipe.near.clone() };
            // The blocks it is made near are among what it uses, as a cooking station is.
            let stations: Vec<BlockId> = recipe.near.as_deref().map(|tag| blocks.tagged(tag).collect()).unwrap_or_default();
            book.add(blocks, Entry { method, output: recipe.output, count: (recipe.count, recipe.count) }, items.into_iter().chain(stations).collect());
        }
        for (input, recipe) in content.cooking.iter() {
            let station = recipe.def.station.clone();
            let stations: Vec<BlockId> = content.blocks.tagged(&station).collect();
            let input = blocks.item_of(input);
            let with: Vec<(BlockId, u8)> = recipe.with.iter().map(|&(item, n)| (blocks.item_of(item), n)).collect();
            let uses = [input].into_iter().chain(with.iter().map(|&(item, _)| item)).chain(stations).collect();
            let method = Method::Cook { input, uses: recipe.def.uses, with, station, skill: recipe.skill, level: recipe.def.level, burnt: recipe.burnt };
            book.add(blocks, Entry { method, output: recipe.output, count: (recipe.def.count, recipe.def.count) }, uses);
        }
        for (rule, block) in content.tools.givers() {
            let Some((given, chance)) = rule.instead else { continue };
            let tools: Vec<BlockId> = rule.by.map(|kind| blocks.ids().filter(|&b| content.tools.tool(Some(b)).kind == Some(kind)).collect()).unwrap_or_default();
            let tool = rule.by.and_then(|kind| content.tools.first_of(kind, 0));
            let method = Method::Break { block, tool, chance };
            book.add(blocks, Entry { method, output: given, count: (1, 1) }, [block].into_iter().chain(tools).collect());
        }
        for (id, creature) in content.creatures.iter() {
            for (product, &item) in creature.def.products.iter().zip(&creature.products) {
                let tools: Vec<BlockId> = product.tool.as_deref().map(|tag| content.blocks.tagged(tag).collect()).unwrap_or_default();
                let method = Method::Product { creature: id, tool: product.tool.clone() };
                book.add(blocks, Entry { method, output: item, count: product.count }, tools);
            }
            for drop in &creature.loot {
                let method = Method::Drop { creature: id, chance: drop.loot.chance };
                book.add(blocks, Entry { method, output: drop.item, count: drop.loot.count }, Vec::new());
            }
        }
        book
    }

    /// Adds an entry, unless it makes a plant whose item merged into another; one using such
    /// a plant lists the item it merged into (`merged` blocks are never held).
    fn add(&mut self, blocks: &crate::block::BlockRegistry, entry: Entry, mut uses: Vec<BlockId>) {
        if blocks.merged_away(entry.output) {
            return;
        }
        uses.iter_mut().for_each(|b| *b = blocks.item_of(*b));
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
    fn plants_whose_item_merged_are_not_in_the_book() {
        let content = content();
        let blocks = &content.blocks;
        let (oregano, herbs, stone) = (blocks.id("base:oregano").unwrap(), blocks.id("base:kitchen_herbs").unwrap(), blocks.id("base:stone").unwrap());
        assert!(blocks.merged_away(oregano) && !blocks.merged_away(herbs) && !blocks.listed(oregano) && blocks.listed(herbs));
        assert_eq!(blocks.item_of(oregano), herbs);
        let mut book = RecipeBook::default();
        // An entry making one is dropped, and one using it lists the item it merged into.
        book.add(blocks, Entry { method: Method::Drop { creature: CreatureId(0), chance: 1.0 }, output: oregano, count: (1, 1) }, Vec::new());
        book.add(blocks, Entry { method: Method::Drop { creature: CreatureId(0), chance: 1.0 }, output: stone, count: (1, 1) }, vec![oregano, herbs]);
        assert!(book.making(oregano).is_empty() && book.using(oregano).is_empty(), "a merged-away plant is in the book");
        assert_eq!((book.making(stone).len(), book.using(herbs).len()), (1, 1));
        // And the game's own book never mentions one.
        let book = RecipeBook::new(&content);
        assert!(blocks.ids().filter(|&id| blocks.merged_away(id)).all(|id| book.making(id).is_empty() && book.using(id).is_empty()));
    }

    /// What breaking gives in place of a block (gravel's flint, tall grass's twine with a
    /// knife) is in the book, with the block and the tool it waits on, and each knife lists
    /// twine among its uses.
    #[test]
    fn what_breaking_gives_is_in_the_book() {
        let content = content();
        let book = RecipeBook::new(&content);
        let id = |name: &str| content.blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let twine = book.making(id("base:twine"));
        assert!(
            twine.iter().any(|e| matches!(e.method, Method::Break { block, tool: Some(tool), chance } if block == id("base:tall_grass") && tool == id("base:wooden_knife") && chance > 0.0)),
            "{twine:?}"
        );
        assert!(book.making(id("base:flint")).iter().any(|e| matches!(e.method, Method::Break { block, tool: None, .. } if block == id("base:gravel"))));
        for knife in ["base:wooden_knife", "base:stone_knife", "base:iron_knife"] {
            assert!(book.using(id(knife)).iter().any(|e| e.output == id("base:twine")), "{knife} lists no twine");
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
