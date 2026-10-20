//! Crafting: recipes are content (each package's `recipes.ron`), matched against the
//! inventory's crafting grid (see [`crate::inventory`]): two by two anywhere, three by
//! three at a crafting table.
//!
//! A recipe is shaped (a pattern of up to three rows of three, which may sit anywhere in
//! the grid, as written or mirrored left to right) or shapeless (ingredients in any arrangement). One bigger than two by two, or
//! with more than four ingredients, needs a table. A name with `{name}` in it makes a
//! family of recipes, one for every name that fits all of the recipe's blocks: one line
//! turns every wood's log into its planks. A cell or ingredient may also accept several
//! items: `tag:TAG` takes anything with that tag (a family's own, with `{name}` in it, as
//! `tag:dye:{name}`), and `a|b` either of two. A recipe trains a skill (Crafting unless it
//! says), may need a level in it, and gives experience for every one made.
//!
//! A recipe may also be made only `near` a block with some tag, within a player's reach (a
//! chopping block's planks): where the grid holds both a recipe made near something the
//! player stands by and one made anywhere, the one near it is made.

use glam::{IVec3, Vec3};
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

/// Where a shapeless recipe's ingredients are laid, in order: the two by two first.
const SHAPELESS_CELLS: [usize; GRID] = [0, 1, SIDE, SIDE + 1, 2, SIDE + 2, 2 * SIDE, 2 * SIDE + 1, 2 * SIDE + 2];

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
    /// Made only within reach of a block with this tag.
    #[serde(default)]
    pub near: Option<String>,
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
    /// The tag of a block it is made near, if any.
    pub near: Option<String>,
    /// What its cells accept that no player holds (sorted): natural blocks that break into
    /// something else, as deepslate breaks to its cobbles. Shown and asked for last.
    unheld: Vec<BlockId>,
}

impl Recipe {
    /// The grid laid out as the recipe asks, with the first item each cell accepts that a
    /// player can hold: a pattern from the top left corner, or the ingredients cell by cell.
    pub fn cells(&self) -> [Option<BlockId>; GRID] {
        self.alternatives().map(|cell| cell.map(|accepts| self.shown(accepts)))
    }

    /// The first of `accepts` a player can hold, else the first.
    fn shown(&self, accepts: &[BlockId]) -> BlockId {
        *accepts.iter().find(|item| self.unheld.binary_search(item).is_err()).unwrap_or(&accepts[0])
    }

    /// The grid as the recipe book shows it, a layout for each item a cell may take: the
    /// `k`th layout puts each cell's `k`th item a player can hold (going round a cell that
    /// holds fewer). One layout when every cell takes one item.
    pub fn layouts(&self) -> Vec<[Option<BlockId>; GRID]> {
        let held: Vec<Option<Vec<BlockId>>> = self
            .alternatives()
            .iter()
            .map(|cell| {
                cell.map(|accepts| {
                    let held: Vec<BlockId> = accepts.iter().copied().filter(|item| self.unheld.binary_search(item).is_err()).collect();
                    if held.is_empty() { accepts.to_vec() } else { held }
                })
            })
            .collect();
        let ways = held.iter().flatten().map(Vec::len).max().unwrap_or(1);
        (0..ways).map(|k| std::array::from_fn(|cell| held[cell].as_ref().map(|items| items[k % items.len()]))).collect()
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

    /// The grid laid with `times` sets of this recipe from what `have` holds (counts by
    /// item), each cell with its item and how many: a cell takes the first of its
    /// alternatives there is enough of, preferring what an earlier cell with the same
    /// alternatives took (a family's logs all oak), and never more than one stack of it
    /// (`stack`). A shaped recipe sits in the top left corner; a shapeless one fills the two
    /// by two first, so one of up to four ingredients fits the inventory's own grid. None
    /// if some cell finds not enough.
    pub fn lay(&self, times: u8, have: &FxHashMap<BlockId, u32>, stack: impl Fn(BlockId) -> u8) -> Option<[Option<(BlockId, u8)>; GRID]> {
        if times == 0 {
            return None;
        }
        let mut used: FxHashMap<BlockId, u32> = FxHashMap::default();
        let mut chosen: Vec<(&[BlockId], BlockId)> = Vec::new();
        let mut grid = [None; GRID];
        let cells = self.alternatives();
        for (i, accepts) in cells.into_iter().enumerate() {
            let Some(accepts) = accepts else { continue };
            let at = if self.shapeless() { SHAPELESS_CELLS[i] } else { i };
            // Enough for `cells` more cells' sets.
            let enough = |item: &BlockId, cells: u32| stack(*item) >= times && have.get(item).copied().unwrap_or(0) >= used.get(item).copied().unwrap_or(0) + cells * times as u32;
            let alike = cells[i..].iter().filter(|c| **c == Some(accepts)).count() as u32;
            let same = chosen.iter().find(|(earlier, _)| *earlier == accepts).map(|&(_, item)| item).filter(|item| enough(item, 1));
            let item = same
                .or_else(|| accepts.iter().copied().find(|item| enough(item, alike)))
                .or_else(|| accepts.iter().copied().find(|item| enough(item, 1)))?;
            *used.entry(item).or_default() += times as u32;
            chosen.push((accepts, item));
            grid[at] = Some((item, times));
        }
        Some(grid)
    }

    /// What `have` (counts by item) lacks for one set, by item and how many: each cell takes
    /// one of the first of its alternatives there is any left of, and a cell that finds
    /// none counts its first alternative a player can hold short. Empty when one set is there to lay.
    pub fn short(&self, have: &FxHashMap<BlockId, u32>) -> Vec<(BlockId, u32)> {
        let mut needed: Vec<(BlockId, u32)> = Vec::new();
        let mut left = have.clone();
        for accepts in self.alternatives().into_iter().flatten() {
            match accepts.iter().find(|item| left.get(item).copied().unwrap_or(0) > 0) {
                Some(item) => *left.get_mut(item).expect("found with some left") -= 1,
                None => match needed.iter_mut().find(|(item, _)| *item == self.shown(accepts)) {
                    Some((_, n)) => *n += 1,
                    None => needed.push((self.shown(accepts), 1)),
                },
            }
        }
        needed
    }

    /// Whether the grid holds it: a pattern as written or mirrored left to right, wherever it
    /// sits; ingredients in any arrangement.
    fn matches(&self, grid: &[Option<BlockId>; GRID]) -> bool {
        match &self.pattern {
            Some(pattern) => shape_of(grid).is_some_and(|shape| {
                let fits = |shape: &[Vec<Option<BlockId>>]| {
                    shape.len() == pattern.len()
                        && shape.iter().zip(pattern).all(|(row, want)| {
                            row.len() == want.len()
                                && row.iter().zip(want).all(|(item, accepts)| match (item, accepts) {
                                    (None, None) => true,
                                    (Some(item), Some(accepts)) => accepts.binary_search(item).is_ok(),
                                    _ => false,
                                })
                        })
                };
                fits(&shape) || fits(&mirrored(&shape))
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

/// Rows mirrored left to right.
fn mirrored<T: Clone>(rows: &[Vec<T>]) -> Vec<Vec<T>> {
    rows.iter().map(|row| row.iter().rev().cloned().collect()).collect()
}

/// Whether some grid would hold both patterns: the same size, and every cell empty in both
/// or with an item both accept.
fn overlap(a: &[Vec<Option<Accepts>>], b: &[Vec<Option<Accepts>>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.iter().any(|item| b.binary_search(item).is_ok()),
                    _ => false,
                })
        })
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
            // The single item names, for families (`tag:` and `a|b` cells stand apart, and an
            // empty cell is no item: with it in, no name fits and the family has no recipes).
            let names: Vec<&String> =
                def.pattern.iter().flatten().chain(&def.ingredients).chain([&def.output]).filter(|n| !n.is_empty() && !n.starts_with("tag:") && !n.contains('|')).collect();
            // The tags a family fills in (`tag:dye:{name}`).
            let tags: Vec<&str> = def.pattern.iter().flatten().chain(&def.ingredients).filter_map(|n| n.strip_prefix("tag:")).filter(|t| t.contains("{name}")).collect();
            let bad = |what: &str| RegistryError(format!("recipe for {}: {what}", def.output));
            if def.pattern.len() > SIDE || def.pattern.iter().any(|row| row.len() > SIDE || row.len() != def.pattern[0].len()) {
                return Err(bad("a pattern is up to three rows of three"));
            }
            if def.pattern.is_empty() == def.ingredients.is_empty() || def.ingredients.len() > GRID {
                return Err(bad("give either a pattern or up to nine ingredients"));
            }
            if let Some(tag) = &def.near
                && !blocks.ids().any(|id| blocks.def(id).has_tag(tag))
            {
                return Err(bad(&format!("nothing has the tag `{tag}` it is made near")));
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
                        // A fill whose own tag nothing has is no family member, not an error.
                        .filter(|fill| tags.iter().all(|tag| blocks.ids().any(|id| blocks.def(id).has_tag(&tag.replace("{name}", fill)))))
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
                        Some(tag) => {
                            // A family's recipe may take its own tag (`tag:dye:{name}`).
                            let tag = fill.as_ref().map_or_else(|| tag.to_owned(), |fill| tag.replace("{name}", fill));
                            (0..blocks.len() as u16).map(BlockId).filter(|&id| blocks.def(id).has_tag(&tag)).collect()
                        }
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
                // A natural block that breaks into something else is never in hand.
                let mut unheld: Vec<BlockId> = pattern
                    .iter()
                    .flatten()
                    .flatten()
                    .flatten()
                    .chain(&ingredients)
                    .flatten()
                    .copied()
                    .filter(|&id| blocks.def(id).has_tag("natural") && blocks.info(id).drops != id)
                    .collect();
                unheld.sort();
                unheld.dedup();
                recipes.push(Recipe { pattern, ingredients, output, count: def.count.max(1), skill, level: def.level, xp: def.xp, firm: def.firm, near: def.near.clone(), unheld });
            }
        }
        Self::mirrors_collide(&recipes, blocks)?;
        Ok(Self { recipes })
    }

    /// A pattern matches mirrored too, so its mirror image must not be some other recipe's
    /// pattern: the grid would make whichever is listed first. Refused where the two do not
    /// already overlap as written (that is the order of `find`, as before) and make
    /// something different, made near the same thing.
    fn mirrors_collide(recipes: &[Recipe], blocks: &BlockRegistry) -> Result<(), RegistryError> {
        let shaped: Vec<(&Recipe, &Vec<Vec<Option<Accepts>>>)> = recipes.iter().filter_map(|r| Some((r, r.pattern.as_ref()?))).collect();
        for (i, &(a, pa)) in shaped.iter().enumerate() {
            let flipped = mirrored(pa);
            if flipped == *pa {
                continue;
            }
            for &(b, pb) in &shaped[i + 1..] {
                if a.near != b.near || (a.output, a.count) == (b.output, b.count) || !overlap(&flipped, pb) || overlap(pa, pb) {
                    continue;
                }
                let name = |r: &Recipe| blocks.def(r.output).name.clone();
                return Err(RegistryError(format!(
                    "recipe for {}: mirrored, its pattern is the recipe for {}'s; recipes match their mirror image, so change one",
                    name(a),
                    name(b)
                )));
            }
        }
        Ok(())
    }

    /// The recipe the grid holds away from anything: none made only near a block.
    pub fn find(&self, grid: &[Option<BlockId>; GRID]) -> Option<&Recipe> {
        self.find_near(grid, |_| false)
    }

    /// The recipe the grid holds where `near` says which tags' blocks are within reach: one
    /// made near such a block first, else one made anywhere.
    pub fn find_near(&self, grid: &[Option<BlockId>; GRID], near: impl Fn(&str) -> bool) -> Option<&Recipe> {
        let nearby = self.recipes.iter().filter(|r| r.near.as_deref().is_some_and(&near)).find(|r| r.matches(grid));
        nearby.or_else(|| self.recipes.iter().filter(|r| r.near.is_none()).find(|r| r.matches(grid)))
    }

    /// What each craft of the grid makes while it still holds `first`, the recipe it held
    /// when the player crafted (where `near` says what is within reach): crafting all stops
    /// once what is left makes another recipe. The server judges a craft by this, and the
    /// client predicts it the same way.
    pub fn while_making<'a>(&'a self, first: Option<&'a Recipe>, near: impl Fn(&str) -> bool + 'a) -> impl Fn(&[Option<BlockId>; GRID]) -> Option<(BlockId, u8)> + 'a {
        move |grid| self.find_near(grid, &near).filter(|r| first.is_some_and(|f| std::ptr::eq(*r, f))).map(|r| (r.output, r.count))
    }

    /// The tags recipes are made near, each once.
    pub fn near_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self.recipes.iter().filter_map(|r| r.near.as_deref()).collect();
        tags.sort_unstable();
        tags.dedup();
        tags
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

/// How far a block a recipe is made near may be from the player's eye, to its middle: their
/// reach.
pub const NEAR_REACH: f32 = crate::physics::player::REACH;

/// Which of `tags` (see [`RecipeRegistry::near_tags`]) some block within reach of `eye` has;
/// `block` reads the world. The server judges crafts by this, and the client the same way.
pub fn tags_near(tags: &[&str], blocks: &BlockRegistry, eye: Vec3, block: impl Fn(IVec3) -> Option<BlockId>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    if tags.is_empty() {
        return found;
    }
    let r = NEAR_REACH.ceil() as i32;
    let at = eye.floor().as_ivec3();
    for dy in -r..=r {
        for dz in -r..=r {
            for dx in -r..=r {
                let pos = at + IVec3::new(dx, dy, dz);
                if eye.distance(pos.as_vec3() + 0.5) > NEAR_REACH {
                    continue;
                }
                let Some(b) = block(pos).filter(|b| !b.is_air()) else { continue };
                let def = blocks.def(b);
                for tag in tags {
                    if def.has_tag(tag) && !found.iter().any(|f| f == tag) {
                        found.push((*tag).to_owned());
                    }
                }
                if found.len() == tags.len() {
                    return found;
                }
            }
        }
    }
    found
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

    /// A recipe laid from what there is: the first alternative there is enough of, the
    /// same one in every cell that takes it, as many sets as there is enough for, and the
    /// grid it makes is the recipe's own.
    #[test]
    fn a_recipe_lays_from_what_there_is() {
        let source = r#"(recipes: [
            (pattern: [["stone", "stone"], ["stone", "stone"]], output: "stone_bricks", count: 4),
            (pattern: [["tag:log", "tag:log"]], output: "bread"),
            (ingredients: ["wheat", "oak_planks", "wheat"], output: "stone"),
        ])"#;
        let names = ["test:oak_log", "test:oak_planks", "test:birch_log", "test:stone", "test:stone_bricks", "test:wheat", "test:bread"];
        let tags = |n: &str| if n.ends_with("_log") { vec!["log".to_string()] } else { Vec::new() };
        let blocks = BlockRegistry::new(names.iter().map(|n| BlockDef { name: n.to_string(), tags: tags(n), ..Default::default() }).collect()).unwrap();
        let registry = registry(&blocks, source);
        let id = |name: &str| blocks.id(&format!("test:{name}")).unwrap();
        let have = |items: &[(&str, u32)]| items.iter().map(|&(n, c)| (id(n), c)).collect::<FxHashMap<_, _>>();
        let recipe = |output: &str| registry.iter().find(|r| r.output == id(output)).unwrap();
        let stack = |_| 64;
        let bricks = recipe("stone_bricks");
        assert!(bricks.lay(2, &have(&[("stone", 7)]), stack).is_none(), "two sets take eight");
        let laid = bricks.lay(1, &have(&[("stone", 7)]), stack).unwrap();
        assert_eq!(laid.iter().flatten().count(), 4);
        let items = |laid: [Option<(BlockId, u8)>; GRID]| laid.map(|c| c.map(|(item, _)| item));
        assert_eq!(registry.find(&items(laid)).map(|r| r.output), Some(id("stone_bricks")));
        // Both logs from one wood, the one there is enough of.
        let logs = recipe("bread").lay(2, &have(&[("oak_log", 3), ("birch_log", 4)]), stack).unwrap();
        assert_eq!((logs[0], logs[1]), (Some((id("birch_log"), 2)), Some((id("birch_log"), 2))));
        // Shapeless: into the two by two, so the inventory's grid takes it.
        let laid = recipe("stone").lay(1, &have(&[("wheat", 2), ("oak_planks", 1)]), stack).unwrap();
        assert!(SMALL.iter().filter(|&&i| laid[i].is_some()).count() == 3 && laid.iter().flatten().count() == 3);
        assert_eq!(registry.find(&items(laid)).map(|r| r.output), Some(id("stone")));
        // No more than a stack in a cell.
        assert!(recipe("stone").lay(2, &have(&[("wheat", 4), ("oak_planks", 2)]), |_| 1).is_none());
    }

    #[test]
    fn a_pattern_matches_its_mirror_image() {
        let blocks = blocks();
        let id = |n: &str| blocks.id(&format!("test:{n}"));
        let recipes = registry(&blocks, r#"(recipes: [(pattern: [["stone", "stone"], ["", "oak_log"]], output: "bread")])"#);
        let (stone, log) = (id("stone"), id("oak_log"));
        assert_eq!(recipes.find(&grid(&[&[stone, stone], &[None, log]])).map(|r| r.output), id("bread"));
        assert_eq!(recipes.find(&grid(&[&[None, stone, stone], &[None, log, None]])).map(|r| r.output), id("bread"));
        // Turned over is not mirrored.
        assert!(recipes.find(&grid(&[&[None, log], &[stone, stone]])).is_none());
    }

    #[test]
    fn a_mirror_image_may_not_be_another_recipe() {
        let blocks = blocks();
        let mut skills = SkillRegistry::default();
        skills.extend("core", r#"(curve: RuneScape(max: 99), skills: [(name: "crafting", title: "", description: "")])"#).unwrap();
        let load = |source: &str| RecipeRegistry::new(RecipeRegistry::read("test", source).unwrap(), &blocks, &skills).map(|r| r.len());
        let error = load(r#"(recipes: [
            (pattern: [["stone", ""], ["stone", "stone"]], output: "bread"),
            (pattern: [["", "stone"], ["stone", "stone"]], output: "stone_bricks"),
        ])"#)
        .unwrap_err();
        assert!(error.0.contains("test:bread") && error.0.contains("test:stone_bricks"), "{}", error.0);
        // Through a tag or `a|b` as well.
        assert!(load(r#"(recipes: [
            (pattern: [["oak_log", ""], ["stone", "stone"]], output: "bread"),
            (pattern: [["", "birch_log|oak_log"], ["stone", "stone"]], output: "stone_bricks"),
        ])"#)
        .is_err());
        // The same thing both ways, a pattern that is its own mirror, recipes that already
        // overlap as written all load.
        assert_eq!(
            load(r#"(recipes: [
            (pattern: [["stone", ""], ["stone", "stone"]], output: "bread"),
            (pattern: [["", "stone"], ["stone", "stone"]], output: "bread"),
            (pattern: [["wheat", "wheat"]], output: "bread"),
            (pattern: [["oak_log", "fir_log"]], output: "stone_bricks"),
            (pattern: [["oak_log|fir_log", "fir_log|oak_log"]], output: "oak_planks"),
        ])"#)
            .ok(),
            Some(5)
        );
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
    fn a_family_pattern_may_have_empty_cells() {
        let item = |name: &str| BlockDef { name: name.into(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![item("test:oak_planks"), item("test:birch_planks"), item("test:stick"), item("test:oak_axe"), item("test:birch_axe")]).unwrap();
        let registry = registry(&blocks, r#"(recipes: [(pattern: [["{name}_planks", "{name}_planks"], ["", "stick"]], output: "{name}_axe")])"#);
        assert_eq!(registry.len(), 2, "an empty cell is no name to fit");
    }

    #[test]
    fn a_family_may_take_the_tag_of_its_own_name() {
        let tagged = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![
            tagged("test:wool", &[]),
            tagged("test:poppy", &["dye:red"]),
            tagged("test:cornflower", &["dye:blue"]),
            tagged("test:red_cloth", &[]),
            tagged("test:blue_cloth", &[]),
            // Nothing is dyed wraith: not a member of the family, and no error.
            tagged("test:wraith_cloth", &[]),
        ])
        .unwrap();
        let registry = registry(&blocks, r#"(recipes: [(ingredients: ["wool", "tag:dye:{name}"], output: "{name}_cloth")])"#);
        let id = |name: &str| Some(blocks.id(&format!("test:{name}")).unwrap());
        assert_eq!(registry.len(), 2);
        let made = |flower: &str| registry.find(&grid(&[&[id("wool"), id(flower)]])).map(|r| r.output);
        assert_eq!(made("poppy"), id("red_cloth"));
        assert_eq!(made("cornflower"), id("blue_cloth"));
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
        assert_eq!(inventory.craft(registry.while_making(first, |_| false), true), 1);
        assert_eq!(inventory.craft[0].map(|s| s.count), Some(3), "the logs left are not made into planks");
        let carried = |inventory: &Inventory, item| inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
        assert_eq!((carried(&inventory, id("bread")), carried(&inventory, id("oak_planks"))), (1, 0));
        // Nothing held, nothing made.
        assert_eq!(inventory.craft(registry.while_making(None, |_| false), true), 0);
        // The planks recipe, held from the start, crafts all three.
        let first = registry.find(&inventory.grid());
        assert_eq!(inventory.craft(registry.while_making(first, |_| false), true), 3);
        assert_eq!(carried(&inventory, id("oak_planks")), 12);
    }
    /// A recipe made near a tagged block wins where the player stands by one, and is not
    /// made away from it; crafting all goes on only while still near it.
    #[test]
    fn a_recipe_made_near_a_block_wins_only_near_it() {
        use crate::inventory::{Inventory, ItemStack};
        let tagged = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![tagged("test:oak_log", &[]), tagged("test:oak_planks", &[]), tagged("test:stump", &["chopping_block"])]).unwrap();
        let source = r#"(recipes: [
            (ingredients: ["oak_log"], output: "oak_planks", count: 4),
            (ingredients: ["oak_log"], output: "oak_planks", count: 6, near: "chopping_block"),
        ])"#;
        let registry = registry(&blocks, source);
        let log = blocks.id("test:oak_log");
        let held = grid(&[&[log]]);
        let near = |tag: &str| tag == "chopping_block";
        assert_eq!(registry.find(&held).map(|r| r.count), Some(4), "away from it, the grid's own");
        assert_eq!(registry.find_near(&held, near).map(|r| r.count), Some(6), "by it, two more");
        assert_eq!(registry.near_tags(), ["chopping_block"]);
        let mut inventory = Inventory::default();
        inventory.craft[0] = Some(ItemStack { item: log.unwrap(), count: 2 });
        let first = registry.find_near(&inventory.grid(), near);
        assert_eq!(inventory.craft(registry.while_making(first, near), true), 2);
        // Held near it, the recipe makes nothing once away.
        inventory.craft[0] = Some(ItemStack { item: log.unwrap(), count: 2 });
        assert_eq!(inventory.craft(registry.while_making(first, |_| false), true), 0);
        // A tag no block has is refused.
        let bad = RecipeRegistry::read("test", r#"(recipes: [(ingredients: ["oak_log"], output: "oak_planks", near: "anvil")])"#).unwrap();
        assert!(RecipeRegistry::new(bad, &blocks, &SkillRegistry::default()).is_err());
    }

    /// Only blocks within reach of the eye count, and only the tags asked about.
    #[test]
    fn tags_near_look_within_reach() {
        let tagged = |name: &str, tags: &[&str]| BlockDef { name: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let blocks = BlockRegistry::new(vec![tagged("test:stump", &["chopping_block", "wood"])]).unwrap();
        let stump = blocks.id("test:stump").unwrap();
        let at = IVec3::new(3, 0, 0);
        let world = |pos: IVec3| (pos == at).then_some(stump);
        let eye = Vec3::new(0.5, 0.5, 0.5);
        assert_eq!(tags_near(&["chopping_block", "anvil"], &blocks, eye, world), ["chopping_block"]);
        assert!(tags_near(&["chopping_block"], &blocks, eye - Vec3::X * 3.0, world).is_empty(), "out of reach");
        assert!(tags_near(&[], &blocks, eye, world).is_empty());
    }
}
