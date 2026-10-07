//! Rarity labels follow the rate (design wiki, draft 94): Common at 1 in 5 or better,
//! Uncommon to 1 in 30, Rare to 1 in 150, VeryRare to 1 in 400, Legendary rarer. A find
//! labelled Rare or rarer, and an item labelled so, never drops more often per kill than its
//! label allows, with the best tool. (Uncommon against Common is left to taste: a wolf's
//! fang at 1 in 3 is still a trophy.)

use std::path::Path;

use voxmar_engine::block::Rarity;
use voxmar_engine::content::Content;

/// The most often, per kill, a find of `rarity` may drop.
fn most_often(rarity: Rarity) -> f32 {
    match rarity {
        Rarity::Common | Rarity::Uncommon => 1.0,
        Rarity::Rare => 1.0 / 30.0,
        Rarity::VeryRare => 1.0 / 150.0,
        Rarity::Legendary => 1.0 / 400.0,
    }
}

#[test]
fn rare_drops_are_as_rare_as_their_labels_say() {
    let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap();
    let mut wrong = Vec::new();
    for (_, creature) in content.creatures.iter() {
        for entry in &creature.loot {
            let loot = &entry.loot;
            let tool = loot.tools.iter().map(|t| t.chance).fold(1.0, f32::max);
            let rate = (loot.chance * tool).min(1.0);
            let item = content.blocks.def(entry.item);
            for (what, rarity) in [("the find", loot.rarity), ("the item", item.rarity)] {
                if rate > most_often(rarity) + 1e-6 {
                    wrong.push(format!("{}: {} drops 1 in {:.1}, but {what} is labelled {rarity:?}", creature.def.name, item.name, 1.0 / rate));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
