//! The building pieces (blocks_building.ron) load as their own things: cut down from a block,
//! a piece never stands in for that block where a recipe takes any of a kind.

use voxmar_engine::content::Content;

fn content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

/// Slabs give six for three blocks: if a slab answered a recipe's `tag:` cell (a hearthstone's
/// cobbles), cutting blocks into pieces would double them. A cell naming a piece outright (a
/// shingle made of slabs) is the recipe's own choice.
#[test]
fn no_piece_answers_a_recipes_tag_cell() {
    let content = content();
    let blocks = &content.blocks;
    let piece = |id| blocks.def(id).has_tag("building") && blocks.info(id).shape.is_some();
    let mut wrong = Vec::new();
    for recipe in content.recipes.iter() {
        for cell in recipe.alternatives().into_iter().flatten().filter(|accepts| accepts.len() > 1) {
            for &id in cell.iter().filter(|&&id| piece(id)) {
                wrong.push(format!("{} in a recipe for {}", blocks.def(id).name, blocks.def(recipe.output).name));
            }
        }
    }
    assert!(wrong.is_empty(), "pieces standing in for a kind of block:\n{}", wrong.join("\n"));
    let cobble_slab = blocks.expect("base:cobblestone_slab");
    assert!(!blocks.def(cobble_slab).has_tag("cobbled"), "a cobblestone slab is sling ammo and a cobble");
}
