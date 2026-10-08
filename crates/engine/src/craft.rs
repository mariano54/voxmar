//! Crafting: recipes are content (each package's `recipes.ron`), matched against the
//! inventory's crafting grid (see [`crate::inventory`]): two by two anywhere, three by
//! three at a crafting table.
//!
//! A recipe is shaped (a pattern of up to three rows of three, which may sit anywhere in
//! the grid) or shapeless (ingredients in any arrangement). One bigger than two by two, or
//! with more than four ingredients, needs a table. A name with `{name}` in it makes a
//! family of recipes, one for every name that fits all of the recipe's blocks: one line
//! turns every wood's log into its planks. A cell or ingredient may also accept several
//! items: `tag:TAG` takes anything with that tag, and `a|b` either of two. A recipe trains a
//! skill (Crafting unless it says), may need a level in it, and gives experience for every
//! one made.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::skill::{SkillId, SkillRegistry};

/// The crafting grid's side and cells, row by row from the top left. Away from a table
/// only the top left two by two is used (cells 0, 1, 3 and 4).
pub const SIDE: usize = 3;
pub const GRID: usize = SIDE * SIDE;

/// The cells of the two-by-two grid the inventory has away from a table.
pub const SMALL: [usize; 4] = [0, 1, SIDE, SIDE + 1];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeDef {
    /// Rows of item names, top first; `""` for an empty cell.
    #[serde(default)]
    pub pattern: Vec<Vec<String>>,
    /// Items in any arrangement, one name per item.
    #[serde(default)]
    pub ingredients: Vec<String>,
    pub output: String,
    #[serde(default = "one")]
    pub count: u8,
    /// The skill it trains (Crafting unless given), the level it needs, and the
    /// experience each one made gives.
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default = "one")]
    pub level: u8,
    #[serde(default)]
    pub xp: f32,
    /// Boosts and schematics cannot lower its level (the Singularity's Engineering 90).
    #[serde(default)]
    pub firm: bool,
}

fn one() -> u8 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFile {
    recipes: Vec<RecipeDef>,
}

/// What a cell or ingredient accepts: one item, or any of several (sorted).
pub type Accepts = Vec<BlockId>;

/// A recipe with its names resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    /// The pattern's rows, for a shaped recipe.
    pattern: Option<Vec<Vec<Option<Accepts>>>>,
    /// The ingredients, for a shapeless one.
    ingredients: Vec<Accepts>,
    pub output: BlockId,
    pub count: u8,
    pub skill: Option<SkillId>,
    pub level: u8,
    pub xp: f32,
    pub firm: bool,
}

impl Recipe {
    /// The grid laid out as the recipe asks, with the first item each cell accepts: a
    /// pattern from the top left corner, or the ingredients cell by cell.
    pub fn cells(&self) -> [Option<BlockId>; GRID] {
        self.alternatives().map(|cell| cell.and_then(|accepts| accepts.first().copied()))
    }

    /// The grid laid out as the recipe asks, with everything each cell accepts.
    pub fn alternatives(&self) -> [Option<&[BlockId]>; GRID] {
        let mut cells = [None; GRID];
        match &self.pattern {
            Some(rows) => {
                for (r, row) in rows.iter().enumerate() {
                    for (c, accepts) in row.iter().enumerate() {
                        cells[r * SIDE + c] = accepts.as_deref();
                    }
                }
            }
            None => self.ingredients.iter().zip(&mut cells).for_each(|(accepts, cell)| *cell = Some(accepts.as_slice())),
        }
        cells
    }

    /// Whether its ingredients go in any arrangement.
    pub fn shapeless(&self) -> bool {
        self.pattern.is_none()
    }

    /// Whether it takes a crafting table: bigger than two by two, or more than four
    /// ingredients.
    pub fn needs_table(&self) -> bool {
        match &self.pattern {
            Some(rows) => rows.len() > 2 || rows.iter().any(|row| row.len() > 2),
            None => self.ingredients.len() > 4,
        }
    }

    /// How many rows and columns it takes laid out (see `cells`).
    pub fn size(&self) -> (usize, usize) {
        match &self.pattern {
            Some(rows) => (rows.len(), rows.first().map_or(0, Vec::len)),
            None => (self.ingredients.len().div_ceil(SIDE).max(1), self.ingredients.len().min(SIDE)),
        }
    }

    fn matches(&self, grid: &[Option<BlockId>; GRID]) -> bool {
        match &self.pattern {
            Some(pattern) => shape_of(grid).is_some_and(|shape| {
                shape.len() == pattern.len()
                    && shape.iter().zip(pattern).all(|(row, want)| {
                        row.len() == want.len()
                            && row.iter().zip(want).all(|(item, accepts)| match (item, accepts) {
                                (None, None) => true,
                                (Some(item), Some(accepts)) => accepts.binary_search(item).is_ok(),
                                _ => false,
                            })
                    })
            }),
            None => {
                let items: Vec<BlockId> = grid.iter().flatten().copied().collect();
                items.len() == self.ingredients.len() && assign(&items, &self.ingredients, &mut vec![false; items.len()])
            }
        }
    }
}

/// Whether every ingredient can take a different one of `items` (few enough to search).
fn assign(items: &[BlockId], ingredients: &[Accepts], used: &mut Vec<bool>) -> bool {
    let Some((first, rest)) = ingredients.split_first() else { return true };
    for i in 0..items.len() {
        if !used[i] && first.binary_search(&items[i]).is_ok() {
            used[i] = true;
            if assign(items, rest, used) {
                return true;
            }
            used[i] = false;
        }
    }
    false
}

/// The grid's filled cells cut down to their bounding rows and columns, so a pattern
/// matches wherever it sits.
fn shape_of<T: Clone>(grid: &[Option<T>; GRID]) -> Option<Vec<Vec<Option<T>>>> {
    let cell = |row: usize, column: usize| grid[row * SIDE + column].clone();
    let filled = |row: usize, column: usize| grid[row * SIDE + column].is_some();
    let rows: Vec<usize> = (0..SIDE).filter(|&r| (0..SIDE).any(|c| filled(r, c))).collect();
    let columns: Vec<usize> = (0..SIDE).filter(|&c| (0..SIDE).any(|r| filled(r, c))).collect();
    // A gap row or column inside the shape is part of it.
    let rows: Vec<usize> = match (rows.first(), rows.last()) {
        (Some(&first), Some(&last)) => (first..=last).collect(),
        _ => rows,
    };
    let columns: Vec<usize> = match (columns.first(), columns.last()) {
        (Some(&first), Some(&last)) => (first..=last).collect(),
        _ => columns,
    };
    if rows.is_empty() {
        return None;
    }
    Some(rows.iter().map(|&r| columns.iter().map(|&c| cell(r, c)).collect()).collect())
}

/// Every recipe, in the order packages list them.
#[derive(Debug, Default)]
pub struct RecipeRegistry {
    recipes: Vec<Recipe>,
}

impl RecipeRegistry {
    /// Reads one package's `recipes.ron`; unqualified names go into `namespace`.
    pub fn read(namespace: &str, source: &str) -> Result<Vec<RecipeDef>, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: RecipeFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/recipes.ron: {e}")))?;
        // Each of `a|b`'s names; `tag:` names a tag, not an item.
        let qualify = |name: &mut String| {
            if !name.is_empty() && !name.starts_with("tag:") {
                *name = name.split('|').map(|n| if n.contains(':') { n.to_owned() } else { format!("{namespace}:{n}") }).collect::<Vec<_>>().join("|");
            }
        };
        let mut defs = file.recipes;
        for def in &mut defs {
            def.pattern.iter_mut().flatten().for_each(qualify);
            def.ingredients.iter_mut().for_each(qualify);
            qualify(&mut def.output);
        }
        Ok(defs)
    }

    pub fn new(defs: Vec<RecipeDef>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let mut recipes = Vec::new();
        let crafting = skills.id("core:crafting");
        for def in defs {
            // The single item names, for families (`tag:` and `a|b` cells stand apart).
            let names: Vec<&String> = def.pattern.iter().flatten().chain(&def.ingredients).chain([&def.output]).filter(|n| !n.starts_with("tag:") && !n.contains('|')).collect();
            let bad = |what: &str| RegistryError(format!("recipe for {}: {what}", def.output));
            if def.pattern.len() > SIDE || def.pattern.iter().any(|row| row.len() > SIDE || row.len() != def.pattern[0].len()) {
                return Err(bad("a pattern is up to three rows of three"));
            }
            if def.pattern.is_empty() == def.ingredients.is_empty() || def.ingredients.len() > GRID {
                return Err(bad("give either a pattern or up to nine ingredients"));
            }
            let skill = match &def.skill {
                Some(name) => Some(skills.id(name).ok_or_else(|| bad(&format!("no skill `{name}`")))?),
                None => crafting,
            };
            // A family: one recipe for every name that fits all of the recipe's blocks.
            let fills: Vec<Option<String>> = match names.iter().find(|n| n.contains("{name}")) {
                None => vec![None],
                Some(template) => {
                    let (prefix, suffix) = template.split_once("{name}").expect("it holds the placeholder");
                    (0..blocks.len())
                        .map(|i| blocks.def(BlockId(i as u16)).name.as_str())
                        .filter_map(|name| name.strip_prefix(prefix)?.strip_suffix(suffix).map(str::to_owned))
                        .filter(|fill| names.iter().all(|n| blocks.id(&n.replace("{name}", fill)).is_some()))
                        .map(Some)
                        .collect()
                }
            };
            for fill in fills {
                let item = |name: &str| -> Result<BlockId, RegistryError> {
                    let name = fill.as_ref().map_or_else(|| name.to_owned(), |fill| name.replace("{name}", fill));
                    blocks.id(&name).ok_or_else(|| bad(&format!("no block `{name}`")))
                };
                let resolve = |name: &String| -> Result<Option<Accepts>, RegistryError> {
                    if name.is_empty() {
                        return Ok(None);
                    }
                    let mut accepts: Accepts = match name.strip_prefix("tag:") {
                        Some(tag) => (0..blocks.len() as u16).map(BlockId).filter(|&id| blocks.def(id).has_tag(tag)).collect(),
                        None => name.split('|').map(item).collect::<Result<_, _>>()?,
                    };
                    if accepts.is_empty() {
                        return Err(bad(&format!("nothing has the tag in `{name}`")));
                    }
                    accepts.sort();
                    accepts.dedup();
                    Ok(Some(accepts))
                };
                let pattern = if def.pattern.is_empty() {
                    None
                } else {
                    let rows: Vec<Vec<Option<Accepts>>> =
                        def.pattern.iter().map(|row| row.iter().map(resolve).collect()).collect::<Result<_, _>>()?;
                    // Patterns are compared trimmed, like the grid.
                    let mut grid: [Option<Accepts>; GRID] = Default::default();
                    for (r, row) in rows.into_iter().enumerate() {
                        for (c, accepts) in row.into_iter().enumerate() {
                            grid[r * SIDE + c] = accepts;
                        }
                    }
                    shape_of(&grid)
                };
                let ingredients: Vec<Accepts> = def.ingredients.iter().map(resolve).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
                let output = item(&def.output)?;
                recipes.push(Recipe { pattern, ingredients, output, count: def.count.max(1), skill, level: def.level, xp: def.xp, firm: def.firm });
            }
        }
        Ok(Self { recipes })
    }

    /// The recipe the grid holds, if any.
    pub fn find(&self, grid: &[Option<BlockId>; GRID]) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.matches(grid))
    }

    /// What each craft of the grid makes while it still holds `first`, the recipe it held
    /// when the player crafted: crafting all stops once what is left makes another recipe.
    /// The server judges a craft by this, and the client predicts it the same way.
    pub fn while_making<'a>(&'a self, first: Option<&'a Recipe>) -> impl Fn(&[Option<BlockId>; GRID]) -> Option<(BlockId, u8)> + 'a {
        move |grid| self.find(grid).filter(|r| first.is_some_and(|f| std::ptr::eq(*r, f))).map(|r| (r.output, r.count))
    }

    /// Every recipe, in the order packages list them.
    pub fn iter(&self) -> impl Iterator<Item = &Recipe> {
        self.recipes.iter()
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

/// How many of each item a recipe takes, for showing it.
pub fn inputs(grid: &[Option<BlockId>; GRID]) -> FxHashMap<BlockId, u8> {
    let mut counts = FxHashMap::default();
    for item in grid.iter().flatten() {
        *counts.entry(*item).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn blocks() -> BlockRegistry {
        let names = ["test:oak_log", "test:oak_planks", "test:birch_log", "test:birch_planks", "test:stone", "test:stone_bricks", "test:wheat", "test:bread", "test:fir_log"];
        BlockRegistry::new(names.iter().map(|n| BlockDef { name: n.to_string(), ..Default::default() }).collect()).unwrap()
    }

    /// A grid from rows of up to three cells, as the player sees it.
    fn grid(rows: &[&[Option<BlockId>]]) -> [Option<BlockId>; GRID] {
        let mut grid = [None; GRID];
        for (r, row) in rows.iter().enumerate() {
            for (c, item) in row.iter().enumerate() {
                grid[r * SIDE + c] = *item;
            }
        }
        grid
    }

    fn registry(blocks: &BlockRegistry, source: &str) -> RecipeRegistry {
        let mut skills = SkillRegistry::default();
        skills.extend("core", r#"(curve: RuneScape(max: 99), skills: [(name: "crafting", title: "", description: ""), (name: "engineering", title: "", description: "")])"#).unwrap();
        RecipeRegistry::new(RecipeRegistry::read("test", source).unwrap(), blocks, &skills).unwrap()
    }

    #[test]
    fn families_shapes_and_shapeless() {
        let blocks = blocks();
        let source = r#"(recipes: [
            (ingredients: ["{name}_log"], output: "{name}_planks", count: 4),
            (pattern: [["stone", "stone"], ["stone", "stone"]], output: "stone_bricks", count: 4),
            (pattern: [["wheat", "wheat"]], output: "bread"),
        ])"#;
        let registry = registry(&blocks, source);
        // Oak and birch have planks; fir has none, so it gets no recipe.
        assert_eq!(registry.len(), 4);
        let id = |name: &str| blocks.id(&format!("test:{name}")).unwrap();
        let birch = grid(&[&[], &[Some(id("birch_log"))]]);
        assert_eq!(registry.find(&birch).map(|r| (r.output, r.count)), Some((id("birch_planks"), 4)));
        assert!(registry.find(&grid(&[&[Some(id("fir_log"))]])).is_none());
        // A shaped recipe matches in any row, but not turned on its side.
        let wheat = Some(id("wheat"));
        assert!(registry.find(&grid(&[&[], &[], &[None, wheat, wheat]])).is_some_and(|r| r.output == id("bread")));
        assert!(registry.find(&grid(&[&[wheat], &[wheat]])).is_none());
        let stone = Some(id("stone"));
        assert!(registry.find(&grid(&[&[stone, stone], &[stone, stone]])).is_some_and(|r| r.output == id("stone_bricks")));
        assert!(registry.find(&grid(&[&[stone, stone], &[stone]])).is_none());
        assert!(registry.find(&[None; GRID]).is_none());
        // Planks train Crafting, and no recipe so far needs a table.
        assert!(registry.iter().all(|r| r.skill.is_some() && !r.needs_table()));
    }

    #[test]
    fn three_by_three_recipes_need_a_table_and_keep_their_gaps() {
        let blocks = blocks();
        let source = r#"(recipes: [
            (pattern: [["stone", "stone", "stone"], ["", "oak_planks", ""], ["", "oak_planks", ""]], output: "bread", skill: "core:engineering", level: 5, xp: 12.5),
            (pattern: [["wheat", ""], ["", "wheat"]], output: "stone"),
        ])"#;
        let registry = registry(&blocks, source);
        let id = |name: &str| Some(blocks.id(&format!("test:{name}")).unwrap());
        let pick = registry.find(&grid(&[&[id("stone"), id("stone"), id("stone")], &[None, id("oak_planks")], &[None, id("oak_planks")]])).unwrap();
        assert!(pick.needs_table());
        assert_eq!((pick.level, pick.xp, pick.size()), (5, 12.5, (3, 3)));
        // A diagonal keeps its gap, anywhere in the grid.
        let wheat = id("wheat");
        assert!(registry.find(&grid(&[&[], &[None, wheat], &[None, None, wheat]])).is_some());
        assert!(registry.find(&grid(&[&[wheat], &[None, None, wheat]])).is_none(), "a wider gap is another shape");
    }

    #[test]
    fn cells_may_take_any_item_with_a_tag_or_either_of_two() {
        let tagged = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![
            tagged("test:oak_log", &["log"]),
            tagged("test:birch_log", &["log"]),
            tagged("test:coal", &[]),
            tagged("test:charcoal", &[]),
            tagged("test:torch", &[]),
            tagged("test:stick", &[]),
            tagged("test:campfire", &[]),
        ])
        .unwrap();
        let source = r#"(recipes: [
            (pattern: [["coal|charcoal"], ["stick"]], output: "torch", count: 4),
            (ingredients: ["tag:log", "tag:log", "stick"], output: "campfire"),
        ])"#;
        let registry = registry(&blocks, source);
        let id = |name: &str| Some(blocks.id(&format!("test:{name}")).unwrap());
        for fuel in ["coal", "charcoal"] {
            assert!(registry.find(&grid(&[&[id(fuel)], &[id("stick")]])).is_some_and(|r| r.output == id("torch").unwrap()), "{fuel}");
        }
        assert!(registry.find(&grid(&[&[id("stick")], &[id("stick")]])).is_none());
        // Two logs of different woods, in any arrangement.
        let fire = registry.find(&grid(&[&[id("stick"), id("oak_log")], &[id("birch_log")]])).unwrap();
        assert_eq!(fire.output, id("campfire").unwrap());
        assert!(registry.find(&grid(&[&[id("stick"), id("oak_log")], &[id("coal")]])).is_none());
        // The viewer shows the first of each cell's choices.
        assert_eq!(fire.cells()[..3], [id("oak_log"), id("oak_log"), id("stick")]);
        assert_eq!(fire.alternatives()[0].map(<[BlockId]>::len), Some(2));
    }

    #[test]
    fn crafting_all_stops_when_what_is_left_makes_another_recipe() {
        use crate::inventory::{Inventory, ItemStack};
        let blocks = blocks();
        let source = r#"(recipes: [
            (ingredients: ["oak_log", "stone"], output: "bread"),
            (ingredients: ["oak_log"], output: "oak_planks", count: 4),
        ])"#;
        let registry = registry(&blocks, source);
        let id = |name: &str| blocks.id(&format!("test:{name}")).unwrap();
        // Four logs and one stone: one loaf, and then the logs alone would make planks.
        let mut inventory = Inventory::default();
        inventory.craft[0] = Some(ItemStack { item: id("oak_log"), count: 4 });
        inventory.craft[1] = Some(ItemStack { item: id("stone"), count: 1 });
        let first = registry.find(&inventory.grid());
        assert_eq!(first.map(|r| r.output), Some(id("bread")));
        assert_eq!(inventory.craft(registry.while_making(first), true), 1);
        assert_eq!(inventory.craft[0].map(|s| s.count), Some(3), "the logs left are not made into planks");
        let carried = |inventory: &Inventory, item| inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
        assert_eq!((carried(&inventory, id("bread")), carried(&inventory, id("oak_planks"))), (1, 0));
        // Nothing held, nothing made.
        assert_eq!(inventory.craft(registry.while_making(None), true), 0);
        // The planks recipe, held from the start, crafts all three.
        let first = registry.find(&inventory.grid());
        assert_eq!(inventory.craft(registry.while_making(first), true), 3);
        assert_eq!(carried(&inventory, id("oak_planks")), 12);
    }
}
