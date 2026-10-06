//! Crafting: recipes are content (each package's `recipes.ron`), matched against the
//! inventory's crafting grid (see [`crate::inventory`]): two by two anywhere, three by
//! three at a crafting table.
//!
//! A recipe is shaped (a pattern of up to three rows of three, which may sit anywhere in
//! the grid) or shapeless (ingredients in any arrangement). One bigger than two by two, or
//! with more than four ingredients, needs a table. A name with `{name}` in it makes a
//! family of recipes, one for every name that fits all of the recipe's blocks: one line
//! turns every wood's log into its planks. A recipe trains a skill (Crafting unless it
//! says), may need a level in it, and gives experience for every one made.

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
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    /// The pattern's rows, for a shaped recipe.
    pattern: Option<Vec<Vec<Option<BlockId>>>>,
    /// The ingredients, sorted, for a shapeless one.
    ingredients: Vec<BlockId>,
    pub output: BlockId,
    pub count: u8,
    pub skill: Option<SkillId>,
    pub level: u8,
    pub xp: f32,
}

impl Recipe {
    /// The grid laid out as the recipe asks: a pattern from the top left corner, or the
    /// ingredients cell by cell.
    pub fn cells(&self) -> [Option<BlockId>; GRID] {
        let mut cells = [None; GRID];
        match &self.pattern {
            Some(rows) => {
                for (r, row) in rows.iter().enumerate() {
                    for (c, item) in row.iter().enumerate() {
                        cells[r * SIDE + c] = *item;
                    }
                }
            }
            None => self.ingredients.iter().zip(&mut cells).for_each(|(item, cell)| *cell = Some(*item)),
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
    let cell = |row: usize, column: usize| grid[row * SIDE + column];
    let rows: Vec<usize> = (0..SIDE).filter(|&r| (0..SIDE).any(|c| cell(r, c).is_some())).collect();
    let columns: Vec<usize> = (0..SIDE).filter(|&c| (0..SIDE).any(|r| cell(r, c).is_some())).collect();
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

    pub fn new(defs: Vec<RecipeDef>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let mut recipes = Vec::new();
        let crafting = skills.id("core:crafting");
        for def in defs {
            let names: Vec<&String> = def.pattern.iter().flatten().chain(&def.ingredients).chain([&def.output]).collect();
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
                            grid[r * SIDE + c] = *item;
                        }
                    }
                    shape_of(&grid)
                };
                let mut ingredients: Vec<BlockId> = def.ingredients.iter().map(resolve).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
                ingredients.sort();
                let output = resolve(&def.output)?.ok_or_else(|| bad("no output"))?;
                recipes.push(Recipe { pattern, ingredients, output, count: def.count.max(1), skill, level: def.level, xp: def.xp });
            }
        }
        Ok(Self { recipes })
    }

    /// The recipe the grid holds, if any.
    pub fn find(&self, grid: &[Option<BlockId>; GRID]) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.matches(grid))
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
}
