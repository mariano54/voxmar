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
/// placed in, nor the state one swaps into and comes back as (a curtain drawn back), and not
/// the singles and the metal lanterns (see [`the_singles_load_and_can_be_made`]).
fn made(content: &Content) -> Vec<BlockId> {
    content
        .blocks
        .ids()
        .filter(|&id| {
            let def = content.blocks.def(id);
            let state = def.swaps.is_some() && def.drops == def.swaps;
            def.has_tag("decor") && def.family.is_some() && def.form_of().is_none() && !state && !def.has_tag("metal") && matches!(def.tint, BlockTint::Fixed(..))
        })
        .collect()
}

/// A state one swaps into and comes back as (a curtain drawn back) is the same piece: its
/// partner's colour, family and furnishing tags (all but the one naming the state), and it
/// gives the partner back.
#[test]
fn a_swap_state_keeps_its_partners_colour_and_tags() {
    let content = content();
    let mut states = 0;
    for id in content.blocks.ids() {
        let def = content.blocks.def(id);
        if !(def.has_tag("decor") && def.form_of().is_none() && def.swaps.is_some() && def.drops == def.swaps) {
            continue;
        }
        let partner = content.blocks.def(content.blocks.id(def.swaps.as_deref().unwrap()).expect("its partner loads"));
        assert_eq!(partner.swaps.as_deref(), Some(def.name.as_str()), "{} and {} swap both ways", def.name, partner.name);
        assert_eq!(def.tint, partner.tint, "{}'s colour", def.name);
        assert_eq!(def.family, partner.family, "{}'s family", def.name);
        // Apart from the tag naming the state (a curtain drawn across hushes a draught).
        let plain = |tags: &[String]| tags.iter().filter(|t| !t.ends_with("_drawn") && !t.ends_with("_open")).cloned().collect::<Vec<_>>();
        assert_eq!(plain(&def.tags), plain(&partner.tags), "{}'s tags", def.name);
        states += 1;
    }
    assert!(states >= 12, "the twelve dyes' drawn-back curtains, at least: {states}");
}

#[test]
fn the_templates_make_every_family_in_full() {
    let content = content();
    let names: BTreeSet<String> = made(&content).into_iter().map(|id| content.blocks.def(id).name.clone()).collect();
    let furniture = ["stool", "chair", "bench", "table", "bed", "shelf", "bookshelf", "cupboard", "wardrobe", "desk", "counter", "barrel_rack", "crate_stack", "pot_shelf", "window_seat", "sill", "pantry_shelf", "settle", "woodpile", "peg_rail", "blanket_chest", "washstand", "rushlight", "dresser", "pot_crane", "rocking_chair", "cradle", "mantel_shelf", "fireside_bench", "plate_rack", "bedside_table", "footstool", "spice_rack", "candle_box", "bread_bin", "kindling_box"];
    let cloth = ["cloth", "rug", "banner", "wall_hanging", "curtain", "paper_lantern", "cushion", "bunting", "awning"];
    let crockery = ["plate", "bowl", "jar", "jug", "basket"];
    let woods = ["birch", "oak", "dark_oak"];
    let dyes = ["white", "cream", "ochre", "amber", "red", "rust", "rose", "mulberry", "indigo", "sky", "teal", "green"];
    let glazes = ["earthen", "ivory", "cobalt"];
    let mut expected = BTreeSet::new();
    for (families, members) in [(&woods[..], &furniture[..]), (&dyes[..], &cloth[..]), (&glazes[..], &crockery[..])] {
        for family in families {
            for member in members {
                expected.insert(format!("base:{family}_{member}"));
            }
        }
    }
    assert_eq!(expected.len(), 3 * 36 + 12 * 9 + 3 * 5);
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
fn a_placed_piece_gives_itself_back_and_a_village_one_nothing() {
    // Whatever a home is furnished with comes back whole when broken, so a room can be
    // rearranged or a home moved; the `furnishing` tag keeps a village's pieces from
    // giving anything (the server gives a furnishing back only from a cell a player set).
    let content = content();
    let candle = content.blocks.expect("base:candle");
    for id in made(&content).into_iter().chain([candle]) {
        let def: &BlockDef = content.blocks.def(id);
        let info = content.blocks.info(id);
        assert!(def.has_tag("furnishing"), "{} would give itself away where a village placed it", def.name);
        assert_eq!((info.drops, info.drop_count), (id, 1), "{} does not give itself back", def.name);
    }
}

#[test]
fn every_recipe_for_decor_can_be_crafted_as_written() {
    let content = content();
    let decor: BTreeSet<BlockId> = content.blocks.ids().filter(|&id| content.blocks.def(id).has_tag("decor")).collect();
    let mut crafted = 0;
    let mut shadowed = Vec::new();
    for recipe in content.recipes.iter().filter(|r| decor.contains(&r.output)) {
        // The recipe's own ingredients, laid out as it asks, are matched by a recipe with the
        // same output: none is shadowed by another that takes the same things.
        let grid = recipe.cells();
        let found = content.recipes.find(&grid).unwrap_or_else(|| panic!("{} cannot be crafted", content.blocks.def(recipe.output).name));
        if found.output != recipe.output {
            shadowed.push(format!("{} is shadowed by {}", content.blocks.def(recipe.output).name, content.blocks.def(found.output).name));
        }
        crafted += 1;
    }
    assert!(crafted >= 172, "{crafted} recipes");
    assert!(shadowed.is_empty(), "{}", shadowed.join("\n"));
}

/// The singles and the lanterns and columns of metal and stone: each a block with a way to
/// make it, a light where it is a light, and a drop that is nothing or the metal it is.
#[test]
fn the_singles_load_and_can_be_made() {
    let content = content();
    let obtained: BTreeSet<BlockId> = content.recipes.iter().map(|r| r.output).collect();
    let singles = [
        "candle", "brazier", "cauldron", "cottage_stove", "loom_frame", "spindle_stand", "quern_stones", "wind_flags", "stone_grave_slab", "mossy_grave_slab", "stone_headstone",
        "mossy_headstone", "iron_hanging_lantern", "copper_hanging_lantern", "stone_lantern", "sandstone_lantern", "calcite_column", "sandstone_column", "stone_column", "window_box", "rush_mat",
    ];
    for name in singles {
        let id = content.blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("no {name}"));
        assert!(obtained.contains(&id), "{name} has no recipe");
    }
    let light = |name: &str| content.blocks.info(content.blocks.id(&format!("base:{name}")).unwrap()).emission;
    assert_eq!((light("candle"), light("brazier"), light("iron_hanging_lantern"), light("stone_lantern"), light("red_paper_lantern"), light("oak_rushlight")), (8, 14, 14, 10, 12, 7));
    // A rushlight burns out sooner than a candle, down to its bare holder, which a fresh
    // rush lights again; a settlement's are tended.
    let (blocks, flames) = (&content.blocks, &content.flames);
    let (rushlight, candle) = (blocks.expect("base:oak_rushlight"), blocks.expect("base:candle"));
    let (days, spent) = flames.burns_out(blocks, rushlight).expect("a rushlight burns out");
    assert!(days < flames.burns_out(blocks, candle).unwrap().0, "a rush burns out before a candle");
    assert_eq!((blocks.def(spent).name.as_str(), blocks.info(spent).emission), ("base:oak_rushlight_spent", 0));
    assert_eq!((flames.relit(blocks, spent), flames.relit_with(spent)), (Some(rushlight), Some("reed")));
    assert!(flames.tended(rushlight) && !blocks.def(rushlight).has_tag("fire"), "tended, and no fire to spread");
    // A metal lantern breaks to its metal; baked in its colour.
    let copper = content.blocks.id("base:copper_hanging_lantern").unwrap();
    assert_eq!(content.blocks.info(copper).drops, content.blocks.id("base:copper_ingot").unwrap());
    assert_eq!(content.blocks.def(copper).tint, BlockTint::Fixed(214, 128, 86));
}

/// A mantel shelf shows the three things put on it whichever way it faces: a store with a
/// slot for each spot it shows, as every store turned another way keeps things as it does.
#[test]
fn a_mantel_shelf_shows_what_is_put_on_it_whichever_way_it_faces() {
    let content = content();
    let blocks = &content.blocks;
    for name in ["base:oak_mantel_shelf", "base:oak_mantel_shelf_east", "base:oak_mantel_shelf_south", "base:oak_mantel_shelf_west"] {
        let id = blocks.expect(name);
        assert_eq!(blocks.def(id).shows.len(), 3, "{name} shows three things");
        assert!(content.machines.get(id).is_some_and(|m| m.is_store() && m.bin_slots() == 3), "{name} keeps what it shows");
    }
    // Every store turned another way keeps things as the store itself does.
    let mut turned = 0;
    for id in blocks.ids() {
        let Some(base) = blocks.def(id).facing_of().map(|name| blocks.expect(name)) else { continue };
        let Some(store) = content.machines.get(base).filter(|m| m.is_store()) else { continue };
        let kept = content.machines.get(id).is_some_and(|m| m.is_store() && m.bin_slots() == store.bin_slots() && m.holds == store.holds);
        assert!(kept, "{} keeps things as {} does", blocks.def(id).name, blocks.def(base).name);
        turned += 1;
    }
    assert!(turned >= 3 * 3 * 10, "the cupboards, chests, dressers and the shelves, racks, tables and boxes that show things, turned three ways: {turned}");
}

/// By the hearth: a candle box on the wall keeps candles, nothing else, and shows the two
/// standing in it whichever way it faces; a bread bin keeps the loaves under its lid.
#[test]
fn a_candle_box_keeps_candles_and_a_bread_bin_bread() {
    let content = content();
    let blocks = &content.blocks;
    let (candle, bread, stick) = (blocks.expect("base:candle"), blocks.expect("base:bread"), blocks.expect("base:stick"));
    let keeps = |id, thing| content.machines.get(id).and_then(|m| m.holds.as_ref()).is_some_and(|holds| holds.contains(&thing));
    for name in ["base:oak_candle_box", "base:oak_candle_box_east", "base:oak_candle_box_south", "base:oak_candle_box_west"] {
        let id = blocks.expect(name);
        assert_eq!(blocks.def(id).shows.len(), 2, "{name} shows two candles");
        assert!(content.machines.get(id).is_some_and(|m| m.is_store() && m.bin_slots() == 2), "{name} keeps what it shows");
        assert!(keeps(id, candle) && !keeps(id, bread) && !keeps(id, stick), "{name} keeps candles alone");
    }
    let bin = blocks.expect("base:birch_bread_bin");
    assert!(blocks.def(bin).shows.is_empty(), "a bread bin's loaves are under its lid");
    assert!(content.machines.get(bin).is_some_and(|m| m.is_store() && m.bin_slots() == 4), "a bread bin keeps four");
    assert!(keeps(bin, bread) && !keeps(bin, candle) && !keeps(bin, stick), "a bread bin keeps what is baked alone");
}

/// A kindling box by the fire keeps what burns, nothing else, and shows the two things on
/// top of what is in it whichever way it faces.
#[test]
fn a_kindling_box_keeps_fuel_to_hand() {
    let content = content();
    let blocks = &content.blocks;
    let (stick, charcoal, bread) = (blocks.expect("base:stick"), blocks.expect("base:charcoal"), blocks.expect("base:bread"));
    let keeps = |id, thing| content.machines.get(id).and_then(|m| m.holds.as_ref()).is_some_and(|holds| holds.contains(&thing));
    for name in ["base:dark_oak_kindling_box", "base:dark_oak_kindling_box_east", "base:dark_oak_kindling_box_south", "base:dark_oak_kindling_box_west"] {
        let id = blocks.expect(name);
        assert_eq!(blocks.def(id).shows.len(), 2, "{name} shows two");
        assert!(content.machines.get(id).is_some_and(|m| m.is_store() && m.bin_slots() == 4), "{name} keeps four");
        assert!(keeps(id, stick) && keeps(id, charcoal) && !keeps(id, bread), "{name} keeps what burns alone");
    }
}
