//! What a catalogue lists (`BlockRegistry::holdable`): the items a player could hold, not
//! the states a fluid takes, the stages of a growing crop, or a plant whose item merged.

use std::path::Path;

use voxmar_engine::content::Content;

#[test]
fn only_what_a_player_could_hold_is_holdable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
    // Held: a merged item, the water source, seeds, a plain block.
    for name in ["base:kitchen_herbs", "base:water", "base:wheat_seeds", "base:stone"] {
        assert!(blocks.holdable(id(name)), "{name} is not holdable");
    }
    // Not held: a merged-away plant, a crop's stages, a fluid's other states.
    let mut stages = 0;
    let mut states = 0;
    for b in blocks.ids() {
        let name = &blocks.def(b).name;
        if blocks.def(b).has_tag("crop") {
            stages += 1;
            assert!(!blocks.holdable(b), "{name} (a crop stage) is holdable");
        }
        if blocks.fluid(b).is_some_and(|state| blocks.fluids()[state.fluid.0 as usize].source != b) {
            states += 1;
            assert!(!blocks.holdable(b), "{name} (a fluid state) is holdable");
        }
        if blocks.merged_away(b) {
            assert!(!blocks.holdable(b), "{name} (merged away) is holdable");
        }
    }
    assert!(stages >= 20 && states > 0, "{stages} crop stages and {states} fluid states were checked");
    assert!(!blocks.holdable(id("base:oregano")) && !blocks.holdable(id("base:wheat_crop_1")));
    // Air is nothing to hold.
    assert!(!blocks.holdable(voxmar_engine::block::BlockId::AIR));
}
