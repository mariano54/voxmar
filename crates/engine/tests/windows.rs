//! A window lets a lit room's light out (docs/home.md, "Light in the windows"): every block
//! tagged `window` passes block light as air does and glows by the light through it
//! (`lit_through`), so a candle behind a hide window lights the ground outside and the pane
//! shows warm at night, where a stone wall keeps it all in.

use std::sync::Arc;

use glam::IVec3;
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
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
