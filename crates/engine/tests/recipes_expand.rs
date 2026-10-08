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
    for tool in ["wooden_pickaxe", "wooden_axe", "wooden_shovel", "wooden_hoe", "wooden_knife"] {
        let id = content.blocks.id(&format!("base:{tool}")).unwrap();
        assert!(content.recipes.iter().any(|r| r.output == id), "{tool} cannot be crafted");
    }
}
