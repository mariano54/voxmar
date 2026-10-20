//! The building pieces (blocks_building.ron) load as their own things: cut down from a block,
//! a piece never stands in for that block where a recipe or a multiblock takes any of a kind.

use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;

fn content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

/// Slabs give six for three blocks: if a slab answered a recipe's or a multiblock's `tag:`
/// cell that whole blocks answer too (a hearthstone's cobbles, the hearth's ring of stone),
/// cutting blocks into pieces would double them. A cell only pieces answer (`tag:beam`), or
/// one naming a piece outright (a shingle made of slabs), is the recipe's own choice.
#[test]
fn no_piece_answers_a_recipes_tag_cell() {
    let content = content();
    let blocks = &content.blocks;
    let mut wrong = Vec::new();
    let mut judge = |cell: &[BlockId], what: String| {
        if cell.iter().any(|&id| !blocks.is_piece(id)) {
            for &id in cell.iter().filter(|&&id| blocks.is_piece(id)) {
                wrong.push(format!("{} in {what}", blocks.def(id).name));
            }
        }
    };
    for recipe in content.recipes.iter() {
        for cell in recipe.alternatives().into_iter().flatten().filter(|accepts| accepts.len() > 1) {
            judge(cell, format!("a recipe for {}", blocks.def(recipe.output).name));
        }
    }
    for multiblock in content.multiblocks.iter() {
        for (cell, _) in multiblock.pieces() {
            judge(cell, format!("the {}", multiblock.name));
        }
    }
    assert!(wrong.is_empty(), "pieces standing in for a kind of block:\n{}", wrong.join("\n"));
    let cobble_slab = blocks.expect("base:cobblestone_slab");
    assert!(!blocks.def(cobble_slab).has_tag("cobbled"), "a cobblestone slab is sling ammo and a cobble");
    // The test sees what it names: cut stone is stone, and the hearth's ring takes stone.
    assert!(blocks.def(cobble_slab).has_tag("stone") && blocks.is_piece(cobble_slab));
    let hearth = content.multiblocks.get("base:homestead_hearth").expect("the Homestead Hearth");
    assert!(hearth.pieces().iter().any(|(cell, _)| cell.contains(&blocks.expect("base:cobblestone"))), "the hearth's ring takes cobblestone");
    // A piece is any block drawn as a shape, building or not: a headstone of stone is no
    // block of stone in the ring.
    for name in ["base:cobblestone_slab", "base:stone_headstone", "base:stone_grave_slab", "base:stone_column"] {
        let piece = blocks.expect(name);
        assert!(blocks.def(piece).has_tag("stone") && !hearth.pieces().iter().any(|(cell, _)| cell.contains(&piece)), "{name} stands in for stone in the hearth's ring");
    }
}
