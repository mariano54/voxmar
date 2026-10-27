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

/// A hip is the lower of the two slopes it joins and a valley their union: at every point
/// of the cell, a corner piece is solid where the roof eaved north and the roof eaved west
/// (its two sides) are both solid (a hip) or either is (a valley), as the blocks load them.
#[test]
fn a_hip_is_where_both_slopes_are_and_a_valley_where_either_is() {
    let content = content();
    let blocks = &content.blocks;
    let inside = |name: &str, at: glam::Vec3| {
        let shape = blocks.shape(blocks.expect(name)).unwrap_or_else(|| panic!("{name} has no shape"));
        shape.bounds.iter().any(|&(min, max)| at.cmpgt(min).all() && at.cmplt(max).all())
    };
    for (roof, corner, valley) in [
        ("base:thatch_roof", "base:thatch_corner", "base:thatch_inner_corner"),
        ("base:oak_shingles", "base:oak_shingles_corner", "base:oak_shingles_inner_corner"),
    ] {
        let west = format!("{roof}_west");
        let mut wrong = Vec::new();
        for i in 0..32 * 32 * 32 {
            let at = (glam::Vec3::new((i % 32) as f32, (i / 32 % 32) as f32, (i / 1024) as f32) + 0.5) / 32.0;
            let (north, west) = (inside(roof, at), inside(&west, at));
            if inside(corner, at) != (north && west) || inside(valley, at) != (north || west) {
                wrong.push(at);
            }
        }
        assert!(wrong.is_empty(), "{corner} or {valley} is not made of {roof}'s two slopes at {} points, first {:?}", wrong.len(), wrong.first());
    }
}

/// A hipped roof's top closes as its sides do: a ridge's end is the ridge where the ridge
/// turned across it is solid too, over the half towards its end (eaved north, as a roof
/// is), so the end slopes down as its sides do; and a peak, where four hips meet over a
/// square, is the ridge and the ridge turned across it both, sloping on every side.
#[test]
fn a_ridge_ends_and_peaks_in_the_slopes_of_its_sides() {
    let content = content();
    let blocks = &content.blocks;
    let inside = |name: &str, at: glam::Vec3| {
        let shape = blocks.shape(blocks.expect(name)).unwrap_or_else(|| panic!("{name} has no shape"));
        shape.bounds.iter().any(|&(min, max)| at.cmpgt(min).all() && at.cmplt(max).all())
    };
    for (ridge, end, peak) in [
        ("base:thatch_ridge", "base:thatch_ridge_end", "base:thatch_peak"),
        ("base:oak_shingle_ridge", "base:oak_shingle_ridge_end", "base:oak_shingle_peak"),
    ] {
        // The bare ridge runs east to west; turned east, north to south as its end does.
        let across = format!("{ridge}_east");
        let mut wrong = Vec::new();
        for i in 0..32 * 32 * 32 {
            let at = (glam::Vec3::new((i % 32) as f32, (i / 32 % 32) as f32, (i / 1024) as f32) + 0.5) / 32.0;
            let (along, over) = (inside(&across, at), inside(ridge, at));
            if inside(end, at) != (along && (over || at.z > 0.5)) || inside(peak, at) != (along && over) {
                wrong.push(at);
            }
        }
        assert!(wrong.is_empty(), "{end} or {peak} is not made of {ridge}'s slopes at {} points, first {:?}", wrong.len(), wrong.first());
        for name in [end, peak] {
            let id = blocks.expect(name);
            let shelter: Vec<_> = blocks.def(id).tags.iter().filter(|t| t.starts_with("shelter:")).collect();
            assert_eq!(shelter, ["shelter:sound"], "{name} closes a room as its ridge does");
            assert!(blocks.def(id).has_tag("roof") && content.recipes.iter().any(|r| r.output == id), "{name} is a roof a player makes");
        }
    }
}

/// A dry-stone wall for a yard or a pen: courses of cobbles narrowing to a cap, a block
/// high (a railing: nothing steps over it), running east to west; its corner is the run's
/// west part and the run turned east's north part, so a corner joins a wall on either side
/// with no step. Laid without mortar, it shelters a room as a draughty wall, once; it is
/// made of cobbles and is none (no `cobbled`, so it is no sling's stone).
#[test]
fn a_dry_stone_wall_turns_its_corner_in_its_own_courses() {
    let content = content();
    let blocks = &content.blocks;
    let inside = |name: &str, at: glam::Vec3| {
        let shape = blocks.shape(blocks.expect(name)).unwrap_or_else(|| panic!("{name} has no shape"));
        shape.bounds.iter().any(|&(min, max)| at.cmpgt(min).all() && at.cmplt(max).all())
    };
    let (wall, corner) = ("base:dry_stone_wall", "base:dry_stone_wall_corner");
    let across = format!("{wall}_east");
    let mut wrong = Vec::new();
    for i in 0..32 * 32 * 32 {
        let at = (glam::Vec3::new((i % 32) as f32, (i / 32 % 32) as f32, (i / 1024) as f32) + 0.5) / 32.0;
        let want = (inside(wall, at) && at.x < 13.0 / 16.0) || (inside(&across, at) && at.z < 13.0 / 16.0);
        if inside(corner, at) != want {
            wrong.push(at);
        }
    }
    assert!(wrong.is_empty(), "{corner} is not {wall}'s west part and its turned run's north part at {} points, first {:?}", wrong.len(), wrong.first());
    let cobbles = blocks.expect("base:cobblestone");
    for name in [wall, corner, "base:dry_stone_stile"] {
        let id = blocks.expect(name);
        let def = blocks.def(id);
        let shelter: Vec<_> = def.tags.iter().filter(|t| t.starts_with("shelter:")).collect();
        assert_eq!(shelter, ["shelter:draughty"], "{name} walls a room as a gappy wall does, once");
        assert!(def.solid && def.has_tag("railing") && !def.has_tag("cobbled"), "{name} is a block-high wall and no cobble");
        assert!(content.recipes.iter().any(|r| r.output == id), "a player cannot make {name}");
    }
    let recipe = content.recipes.iter().find(|r| r.output == blocks.expect(wall)).unwrap();
    assert!(recipe.cells().iter().flatten().all(|&cell| cell == cobbles), "a dry-stone wall is laid of cobbles");
}

/// A block set down takes a cell from air, a grass tuft or a flower, but not from water or a
/// solid block (`BlockInfo::gives_room`, which planting, tilling and the test bot read).
#[test]
fn a_block_set_down_takes_the_room_of_air_and_plants_only() {
    let content = content();
    let blocks = &content.blocks;
    let gives = |name: &str| blocks.info(blocks.expect(name)).gives_room();
    assert!(gives("core:air") && gives("base:short_grass") && gives("base:poppy"));
    assert!(!gives("base:water") && !gives("base:stone"));
}

/// A knee brace runs corner to corner between a post and the beam it carries: timber as
/// they are, in every wood and facing, and turned over to brace a beam from below.
#[test]
fn a_brace_comes_in_every_wood_and_facing_and_turned_over() {
    let content = content();
    let blocks = &content.blocks;
    for wood in ["oak", "birch", "dark_oak"] {
        for turn in ["", "_east", "_south", "_west"] {
            let up = blocks.expect(&format!("base:{wood}_brace{turn}"));
            let down = blocks.expect(&format!("base:{wood}_brace_top{turn}"));
            assert_eq!(blocks.flipped(up), Some(down), "{wood}_brace{turn} turns over");
            for id in [up, down] {
                let def = blocks.def(id);
                assert!(def.solid && def.has_tag("timber") && def.has_tag("brace") && blocks.is_piece(id), "{} is solid timber, a piece", def.name);
                assert!(def.has_tag("shelter:sound"), "{} is a sound face of a room as the post and beam it joins are", def.name);
            }
        }
        let brace = blocks.expect(&format!("base:{wood}_brace"));
        assert!(content.recipes.iter().any(|recipe| recipe.output == brace), "a {wood} brace is made");
    }
}
