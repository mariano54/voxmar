//! Crafting: recipes are content (each package's `recipes.ron`), matched against the
//! inventory's two-by-two crafting grid (see [`crate::inventory`]).
//!
//! A recipe is shaped (a pattern of up to two rows of two, which may sit anywhere in the
//! grid) or shapeless (ingredients in any arrangement). A name with `{name}` in it makes
//! a family of recipes, one for every name that fits all of the recipe's blocks: one line
//! turns every wood's log into its planks.

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};

/// The crafting grid's cells, row by row: top left, top right, bottom left, bottom right.
pub const GRID: usize = 4;

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
}

fn one() -> u8 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFile {
    recipes: Vec<RecipeDef>,
}

/// A recipe with its names resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    /// The pattern's rows, for a shaped recipe.
    pattern: Option<Vec<Vec<Option<BlockId>>>>,
    /// The ingredients, sorted, for a shapeless one.
    ingredients: Vec<BlockId>,
    pub output: BlockId,
    pub count: u8,
}

impl Recipe {
    fn matches(&self, grid: &[Option<BlockId>; GRID]) -> bool {
        match &self.pattern {
            Some(pattern) => shape_of(grid).is_some_and(|shape| shape == *pattern),
            None => {
                let mut items: Vec<BlockId> = grid.iter().flatten().copied().collect();
                items.sort();
                items == self.ingredients
            }
        }
    }
}

/// The grid's filled cells cut down to their bounding rows and columns, so a pattern
/// matches wherever it sits.
fn shape_of(grid: &[Option<BlockId>; GRID]) -> Option<Vec<Vec<Option<BlockId>>>> {
    let cell = |row: usize, column: usize| grid[row * 2 + column];
    let rows: Vec<usize> = (0..2).filter(|&r| (0..2).any(|c| cell(r, c).is_some())).collect();
    let columns: Vec<usize> = (0..2).filter(|&c| (0..2).any(|r| cell(r, c).is_some())).collect();
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
        let file: RecipeFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/recipes.ron: {e}")))?;
        let qualify = |name: &mut String| {
            if !name.is_empty() && !name.contains(':') {
                *name = format!("{namespace}:{name}");
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

    pub fn new(defs: Vec<RecipeDef>, blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut recipes = Vec::new();
        for def in defs {
            let names: Vec<&String> = def.pattern.iter().flatten().chain(&def.ingredients).chain([&def.output]).collect();
            let bad = |what: &str| RegistryError(format!("recipe for {}: {what}", def.output));
            if def.pattern.len() > 2 || def.pattern.iter().any(|row| row.len() > 2 || row.len() != def.pattern[0].len()) {
                return Err(bad("a pattern is up to two rows of two"));
            }
            if def.pattern.is_empty() == def.ingredients.is_empty() || def.ingredients.len() > GRID {
                return Err(bad("give either a pattern or up to four ingredients"));
            }
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
                let resolve = |name: &String| -> Result<Option<BlockId>, RegistryError> {
                    if name.is_empty() {
                        return Ok(None);
                    }
                    let name = fill.as_ref().map_or_else(|| name.clone(), |fill| name.replace("{name}", fill));
                    blocks.id(&name).map(Some).ok_or_else(|| bad(&format!("no block `{name}`")))
                };
                let pattern = if def.pattern.is_empty() {
                    None
                } else {
                    let rows: Vec<Vec<Option<BlockId>>> =
                        def.pattern.iter().map(|row| row.iter().map(resolve).collect()).collect::<Result<_, _>>()?;
                    // Patterns are compared trimmed, like the grid.
                    let mut grid = [None; GRID];
                    for (r, row) in rows.iter().enumerate() {
                        for (c, item) in row.iter().enumerate() {
                            grid[r * 2 + c] = *item;
                        }
                    }
                    shape_of(&grid)
                };
                let mut ingredients: Vec<BlockId> = def.ingredients.iter().map(resolve).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
                ingredients.sort();
                let output = resolve(&def.output)?.ok_or_else(|| bad("no output"))?;
                recipes.push(Recipe { pattern, ingredients, output, count: def.count.max(1) });
            }
        }
        Ok(Self { recipes })
    }

    /// The recipe the grid holds, if any.
    pub fn find(&self, grid: &[Option<BlockId>; GRID]) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.matches(grid))
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

    #[test]
    fn families_shapes_and_shapeless() {
        let blocks = blocks();
        let source = r#"(recipes: [
            (ingredients: ["{name}_log"], output: "{name}_planks", count: 4),
            (pattern: [["stone", "stone"], ["stone", "stone"]], output: "stone_bricks", count: 4),
            (pattern: [["wheat", "wheat"]], output: "bread"),
        ])"#;
        let registry = RecipeRegistry::new(RecipeRegistry::read("test", source).unwrap(), &blocks).unwrap();
        // Oak and birch have planks; fir has none, so it gets no recipe.
        assert_eq!(registry.len(), 4);
        let id = |name: &str| blocks.id(&format!("test:{name}")).unwrap();
        let birch = [None, None, Some(id("birch_log")), None];
        assert_eq!(registry.find(&birch).map(|r| (r.output, r.count)), Some((id("birch_planks"), 4)));
        assert!(registry.find(&[Some(id("fir_log")), None, None, None]).is_none());
        // A shaped recipe matches in any row, but not turned on its side.
        let wheat = Some(id("wheat"));
        assert!(registry.find(&[None, None, wheat, wheat]).is_some_and(|r| r.output == id("bread")));
        assert!(registry.find(&[wheat, None, wheat, None]).is_none());
        let stone = Some(id("stone"));
        assert!(registry.find(&[stone; GRID]).is_some_and(|r| r.output == id("stone_bricks")));
        assert!(registry.find(&[stone, stone, stone, None]).is_none());
        assert!(registry.find(&[None; GRID]).is_none());
    }
}
