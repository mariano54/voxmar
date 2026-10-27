//! A window lets a lit room's light out (docs/home.md, "Light in the windows"): every block
//! tagged `window` passes block light as air does and glows by the light through it
//! (`lit_through`), so a candle behind a hide window lights the ground outside and the pane
//! shows warm at night, where a stone wall keeps it all in. A lattice is a window too, open
//! or with its shutters open, drawn with a grain of its own so only it glows.

use std::sync::Arc;

use glam::IVec3;
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::coords::Face;
use voxmar_engine::light::LitRegion;
use voxmar_engine::world::Column;

#[test]
fn a_window_lets_a_lit_rooms_light_out() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    let windows: Vec<BlockId> = blocks.ids().filter(|&id| blocks.def(id).has_tag("window")).collect();
    assert!(windows.len() >= 3, "glass, old glass and the hide window are windows");
    for &id in &windows {
        let (def, info) = (blocks.def(id), blocks.info(id));
        assert!(def.lit_through, "{} glows by the light through it", def.name);
        assert_eq!(info.attenuation, 1, "{} passes light as air does", def.name);
    }

    // A stone room round a candle, its wall towards +z a hide window or stone.
    let stone = blocks.id("base:stone").unwrap();
    let candle = blocks.id("base:candle").unwrap();
    let room = IVec3::new(16, 100, 16);
    let outside = |wall: BlockId| {
        let mut column = Column::default();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    column.set_block(room + IVec3::new(x, y, z), stone);
                }
            }
        }
        column.set_block(room, candle);
        column.set_block(room + IVec3::Z, wall);
        let column = Arc::new(column);
        let region = LitRegion::compute(blocks, &std::array::from_fn(|_| column.clone()));
        region.light(room + IVec3::Z * 2) & 0xF
    };
    let light = blocks.info(candle).emission;
    assert_eq!(outside(blocks.id("base:hide_window").unwrap()), light - 2, "out through the window");
    assert_eq!(outside(stone), 0, "a stone wall keeps it in");
}

/// A lattice window is a window, and so is one hung with shutters while they stand open:
/// tagged `window` (so a home's rooms count it), it glows by the light through it and lets
/// a candle's light out as a hide window does. Shut, the shutters keep the light in and are
/// no window; nor are a bare frame (an opening, not a window) or an arrow slit.
#[test]
fn a_lattice_window_lets_the_light_out_and_counts_as_a_window() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("{name}"));
    for name in ["base:oak_lattice_window", "base:oak_shuttered_window", "base:birch_lattice_window_east", "base:stone_mullion_window", "base:stone_mullion_window_west"] {
        let def = blocks.def(id(name));
        assert!(def.has_tag("window") && def.lit_through, "{name} is a window, lit through");
    }
    // Shut, a bare frame, a slit, or plain shutters folded back over an opening (no lattice
    // in it) are no windows.
    for name in ["base:oak_shuttered_window_shut", "base:oak_window_frame", "base:stone_arrow_slit", "base:wooden_shutters_open"] {
        let def = blocks.def(id(name));
        assert!(!def.has_tag("window") && !def.lit_through, "{name} is no window");
    }
    // Opened and shut, the shuttered window is one pair, each the other's swap.
    for facing in ["", "_east", "_south", "_west"] {
        let (open, shut) = (id(&format!("base:oak_shuttered_window{facing}")), id(&format!("base:oak_shuttered_window_shut{facing}")));
        assert_eq!((blocks.swapped(open), blocks.swapped(shut)), (Some(shut), Some(open)), "the shutters at {facing:?} open and shut as a pair");
    }
    let stone = id("base:stone");
    let candle = id("base:candle");
    let room = IVec3::new(16, 100, 16);
    let outside = |wall: BlockId| {
        let mut column = Column::default();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    column.set_block(room + IVec3::new(x, y, z), stone);
                }
            }
        }
        column.set_block(room, candle);
        column.set_block(room + IVec3::Z, wall);
        let column = Arc::new(column);
        let region = LitRegion::compute(blocks, &std::array::from_fn(|_| column.clone()));
        region.light(room + IVec3::Z * 2) & 0xF
    };
    let light = blocks.info(candle).emission;
    assert_eq!(outside(id("base:oak_lattice_window")), light - 2, "out through the lattice");
    assert_eq!(outside(id("base:oak_shuttered_window")), light - 2, "out through the open shutters");
    assert_eq!(outside(id("base:oak_shuttered_window_shut")), 0, "the shut shutters keep it in");
    assert_eq!(outside(id("base:stone_mullion_window")), light - 2, "out through the mullioned window's lights");
}

/// A stone house has a window of its own: a mullioned window of dressed stone, made from
/// stone bricks and their slabs, a draughty wall to a home's rooms, as a lattice is.
#[test]
fn a_stone_house_has_a_mullioned_window() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    let window = blocks.def(blocks.id("base:stone_mullion_window").unwrap());
    assert!(window.has_tag("masonry") && window.has_tag("shelter:draughty") && window.has_tag("draught"), "{:?}", window.tags);
    let id = blocks.id("base:stone_mullion_window").unwrap();
    let recipe = content.recipes.iter().find(|r| r.output == id).expect("a recipe for the mullioned window");
    let inputs: Vec<BlockId> = recipe.alternatives().into_iter().flatten().flatten().copied().collect();
    let count = |name: &str| inputs.iter().filter(|&&b| b == blocks.id(name).unwrap()).count();
    assert_eq!((count("base:stone_bricks"), count("base:stone_brick_slab"), inputs.len()), (4, 2, 6), "stone bricks either side of the slabs");
    assert_eq!((recipe.count, recipe.level), (2, 6), "made two at a time at the arrow slit's level");
}

/// A window's drawing glows by the light through it, so only windows draw with it: the
/// lattice's timber is its own (`lattice_grain`), not the grain every post, beam, frame and
/// chair leg shares, which would light them all in a candle-lit room.
#[test]
fn only_windows_glow_by_the_light_through_them() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    // A face with no drawing of its own takes the first layer, which is no sharing.
    let drawn = |id: BlockId| {
        let (def, info) = (blocks.def(id), blocks.info(id));
        Face::ALL.into_iter().filter(|&f| def.textures.face(f).is_some()).map(|f| info.layers[f as usize]).collect::<Vec<u16>>()
    };
    let glowing: std::collections::BTreeSet<u16> = blocks.ids().filter(|&id| blocks.def(id).lit_through).flat_map(drawn).collect();
    let lit: Vec<&str> = blocks
        .ids()
        .filter(|&id| !blocks.def(id).lit_through && drawn(id).iter().chain(blocks.shape_layers(id)).any(|layer| glowing.contains(layer)))
        .map(|id| blocks.def(id).name.as_str())
        .collect();
    assert!(lit.is_empty(), "{} blocks glow as a window does, as {}", lit.len(), lit.iter().take(6).copied().collect::<Vec<_>>().join(", "));
}

/// A curtain drawn across a window keeps most of a lit room's light in, so the pane glows
/// low; drawn back, the light goes through as before. Both forms swap into each other with
/// a right click, are drapes and count as curtains, and the drawn-back one gives the curtain.
#[test]
fn a_drawn_curtain_keeps_the_light_in() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    let (stone, candle, pane) = (blocks.id("base:stone").unwrap(), blocks.id("base:candle").unwrap(), blocks.id("base:hide_window").unwrap());
    let (drawn, back) = (blocks.id("base:red_curtain").unwrap(), blocks.id("base:red_curtain_open").unwrap());
    // A stone passage: a candle, the curtain's cell, the window, and the light outside it.
    let room = IVec3::new(16, 100, 16);
    let outside = |curtain: BlockId| {
        let mut column = Column::default();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=2 {
                    column.set_block(room + IVec3::new(x, y, z), stone);
                }
            }
        }
        column.set_block(room, candle);
        column.set_block(room + IVec3::Z, curtain);
        column.set_block(room + IVec3::Z * 2, pane);
        let column = Arc::new(column);
        let region = LitRegion::compute(blocks, &std::array::from_fn(|_| column.clone()));
        (region.light(room + IVec3::Z * 2) & 0xF, region.light(room + IVec3::Z * 3) & 0xF)
    };
    let (open_pane, open_out) = outside(back);
    let (shut_pane, shut_out) = outside(drawn);
    assert_eq!(outside(BlockId::AIR), (open_pane, open_out), "drawn back, it passes light as air");
    assert!(shut_pane + 6 <= open_pane, "drawn, the pane is lit {shut_pane} against {open_pane}");
    assert!(shut_out < open_out, "drawn, less light gets out: {shut_out} against {open_out}");
    let (d, b) = (blocks.def(drawn), blocks.def(back));
    assert_eq!((d.swaps.as_deref(), b.swaps.as_deref()), (Some("base:red_curtain_open"), Some("base:red_curtain")));
    assert_eq!(b.drops.as_deref(), Some("base:red_curtain"));
    for def in [d, b] {
        assert!(def.has_tag("drape") && def.has_tag("curtain"), "{} is a curtain and a drape", def.name);
    }
}
