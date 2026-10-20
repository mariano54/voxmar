//! Every recipe the base content writes makes at least one recipe: a family line (`{name}`)
//! that fits no name is a recipe nobody can ever craft, and nothing else says so. A
//! pattern with an empty cell and a `{name}` once fitted nothing, which left the wooden
//! pickaxe, axe, shovel, hoe and knife uncraftable.

use voxmar_engine::content::Content;
use voxmar_engine::craft::RecipeRegistry;

#[test]
fn every_recipe_in_the_base_content_expands_to_at_least_one() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let mut dead = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(dir.join("base")).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with("recipes") && name.ends_with(".ron")) {
            continue;
        }
        let defs = RecipeRegistry::read("base", &std::fs::read_to_string(&path).unwrap()).unwrap();
        for def in defs {
            let label = format!("{name}: {} <- {:?}{:?}", def.output, def.pattern, def.ingredients);
            let made = RecipeRegistry::new(vec![def], &content.blocks, &content.skills).unwrap();
            if made.is_empty() {
                dead.push(label);
            }
        }
    }
    assert!(dead.is_empty(), "recipes that expand to nothing:\n{}", dead.join("\n"));
    // The first tools a player makes, by name.
    for tool in ["flint_pickaxe", "flint_axe", "flint_knife", "digging_stick"] {
        let id = content.blocks.id(&format!("base:{tool}")).unwrap();
        assert!(content.recipes.iter().any(|r| r.output == id), "{tool} cannot be crafted");
    }
}

#[test]
fn campfire_accepts_the_barked_wood_grid_from_survival_inventory() {
    let content = Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap();
    let id = |name: &str| content.blocks.id(&format!("base:{name}")).unwrap();
    let grid = |wood: &str, stone: &str| {
        let mut cells = [None; voxmar_engine::craft::GRID];
        for (cell, item) in voxmar_engine::craft::SMALL.into_iter().zip([wood, wood, stone, stone]) {
            cells[cell] = Some(id(item));
        }
        cells
    };
    for wood in ["oak_wood", "oak_log", "stripped_oak_log", "birch_wood", "spruce_log"] {
        let recipe = content.recipes.find(&grid(wood, "cobblestone")).unwrap_or_else(|| panic!("{wood} cannot make a campfire"));
        assert_eq!((recipe.output, recipe.count), (id("campfire"), 1));
        assert!(!recipe.needs_table(), "a first-night fire needs no crafting table");
    }
    assert!(content.recipes.find(&grid("oak_planks", "cobblestone")).is_none(), "planks are not raw timber");
    assert!(content.recipes.find(&grid("oak_wood", "stone")).is_none(), "the recipe still requires cobblestone");
    let book = voxmar_engine::recipe_book::RecipeBook::new(&content);
    assert!(book.using(id("oak_wood")).iter().any(|entry| entry.output == id("campfire")), "the recipe search must describe the same accepted wood as crafting");
}
