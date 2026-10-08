//! The decor blocks (design draft 108, P58a) load as the templates say: furniture in three
//! woods, cloth things in twelve dyes, crockery in three glazes, each with a colour baked
//! into its textures, a way to get it, and a drop that keeps decor from being a mine.

use std::collections::BTreeSet;

use voxmar_engine::block::{BlockDef, BlockId, BlockTint, Tint};
use voxmar_engine::content::Content;

fn content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

/// The decor blocks the templates made with a baked colour, not the facings they are
/// placed in, and not the singles and the metal lanterns (see [`the_singles_load_and_can_be_made`]).
fn made(content: &Content) -> Vec<BlockId> {
    content
        .blocks
        .ids()
        .filter(|&id| {
            let def = content.blocks.def(id);
            def.has_tag("decor") && def.family.is_some() && def.form_of().is_none() && !def.has_tag("metal") && matches!(def.tint, BlockTint::Fixed(..))
        })
        .collect()
}

#[test]
fn the_templates_make_every_family_in_full() {
    let content = content();
    let names: BTreeSet<String> = made(&content).into_iter().map(|id| content.blocks.def(id).name.clone()).collect();
    let furniture = ["stool", "chair", "bench", "table", "bed", "shelf", "bookshelf", "cupboard", "wardrobe", "desk", "counter", "barrel_rack", "crate_stack", "pot_shelf"];
    let cloth = ["cloth", "rug", "banner", "curtain", "paper_lantern", "cushion", "bunting", "awning"];
    let crockery = ["plate", "bowl", "jar", "jug", "basket"];
    let woods = ["birch", "oak", "dark_oak"];
    let dyes = ["white", "cream", "ochre", "saffron", "red", "rust", "rose", "plum", "indigo", "sky", "teal", "green"];
    let glazes = ["earthen", "ivory", "cobalt"];
    let mut expected = BTreeSet::new();
    for (families, members) in [(&woods[..], &furniture[..]), (&dyes[..], &cloth[..]), (&glazes[..], &crockery[..])] {
        for family in families {
            for member in members {
                expected.insert(format!("base:{family}_{member}"));
            }
        }
    }
    assert_eq!(expected.len(), 3 * 14 + 12 * 8 + 3 * 5);
    let missing: Vec<_> = expected.difference(&names).collect();
    let extra: Vec<_> = names.difference(&expected).collect();
    assert!(missing.is_empty() && extra.is_empty(), "missing {missing:?}, unexpected {extra:?}");
}

#[test]
fn every_decor_block_has_a_colour_baked_into_its_textures_and_a_way_to_get_it() {
    let content = content();
    let obtained: BTreeSet<BlockId> = content.recipes.iter().map(|r| r.output).collect();
    let mut without = Vec::new();
    for id in made(&content) {
        let def = content.blocks.def(id);
        let BlockTint::Fixed(r, g, b) = def.tint else {
            panic!("{} is not tinted a fixed colour", def.name);
        };
        // Every layer its faces and boxes draw is that colour.
        let info = content.blocks.info(id);
        let textures = content.blocks.textures();
        for &layer in info.layers.iter().chain(content.blocks.shape_layers(id)) {
            let tint = textures[layer as usize].tint;
            assert!(tint.is_none() || tint == Some([r, g, b]), "{}: layer {layer}", def.name);
        }
        let drawn = info.layers.iter().chain(content.blocks.shape_layers(id)).filter(|&&l| textures[l as usize].tint == Some([r, g, b])).count();
        assert!(drawn > 0, "{} draws nothing in its colour", def.name);
        // The renderer is told of no tint: the colour is in the texture.
        assert_eq!(info.tints, [Tint::None; 6], "{}", def.name);
        if !obtained.contains(&id) {
            without.push(def.name.clone());
        }
    }
    assert!(without.is_empty(), "decor with no recipe: {without:?}");
}

#[test]
fn furniture_breaks_to_planks_and_everything_else_to_nothing() {
    let content = content();
    let air = BlockId::AIR;
    for id in made(&content) {
        let def: &BlockDef = content.blocks.def(id);
        let drops = content.blocks.info(id).drops;
        if def.has_tag("furniture") {
            let planks = format!("{}_planks", def.family.as_deref().unwrap());
            assert_eq!(drops, content.blocks.id(&planks).unwrap(), "{}", def.name);
            // Half what it cost, at most: breaking furniture is no way to make planks.
            assert!(def.drop_count >= 1 && def.drop_count <= 5, "{}", def.name);
        } else {
            assert_eq!(drops, air, "{} drops something", def.name);
        }
    }
}

#[test]
fn every_recipe_for_decor_can_be_crafted_as_written() {
    let content = content();
    let decor: BTreeSet<BlockId> = made(&content).into_iter().collect();
    let mut crafted = 0;
    for recipe in content.recipes.iter().filter(|r| decor.contains(&r.output)) {
        // The recipe's own ingredients, laid out as it asks, are matched by a recipe with the
        // same output: none is shadowed by another that takes the same things.
        let grid = recipe.cells();
        let found = content.recipes.find(&grid).unwrap_or_else(|| panic!("{} cannot be crafted", content.blocks.def(recipe.output).name));
        assert_eq!(found.output, recipe.output, "{} is shadowed by {}", content.blocks.def(recipe.output).name, content.blocks.def(found.output).name);
        crafted += 1;
    }
    assert!(crafted >= 100, "{crafted} recipes");
}

/// The singles and the lanterns and columns of metal and stone: each a block with a way to
/// make it, a light where it is a light, and a drop that is nothing or the metal it is.
#[test]
fn the_singles_load_and_can_be_made() {
    let content = content();
    let obtained: BTreeSet<BlockId> = content.recipes.iter().map(|r| r.output).collect();
    let singles = [
        "candle", "brazier", "cauldron", "cottage_stove", "loom_frame", "spindle_stand", "quern_stones", "wind_flags", "stone_grave_slab", "mossy_grave_slab", "stone_headstone",
        "mossy_headstone", "iron_hanging_lantern", "copper_hanging_lantern", "stone_lantern", "sandstone_lantern", "calcite_column", "sandstone_column", "stone_column",
    ];
    for name in singles {
        let id = content.blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("no {name}"));
        assert!(obtained.contains(&id), "{name} has no recipe");
    }
    let light = |name: &str| content.blocks.info(content.blocks.id(&format!("base:{name}")).unwrap()).emission;
    assert_eq!((light("candle"), light("brazier"), light("iron_hanging_lantern"), light("stone_lantern"), light("red_paper_lantern")), (8, 14, 14, 10, 12));
    // A metal lantern breaks to its metal; baked in its colour.
    let copper = content.blocks.id("base:copper_hanging_lantern").unwrap();
    assert_eq!(content.blocks.info(copper).drops, content.blocks.id("base:copper_ingot").unwrap());
    assert_eq!(content.blocks.def(copper).tint, BlockTint::Fixed(214, 128, 86));
}
