//! Rarity labels follow the rate (design wiki draft 97, "Rarity labels follow the rate"):
//! an item's label comes from its easiest source, per kill. Common at 1 in 2 or better,
//! Uncommon down to 1 in 30, Rare down to 1 in 150, VeryRare down to 1 in 400, Legendary
//! rarer (bosses); and one class rarer when that source creature is itself Uncommon or
//! rarer (moon dust from glimmer moths, frost antlers from frost stags).
//!
//! Per kill counts a drop's chance with the best tool for it held, and for a count that may
//! come out nothing, `(0, n)`, the chance of at least one, n / (n + 1). A condition such as
//! `when: [Night]` counts as met: a night find is judged per kill at night. Level bonuses
//! are left out (they raise a find by at most one class).
//!
//! An item only creatures drop carries exactly its easiest source's label. One that
//! something else gives too (a recipe, cooking, a crop, fishing, a kept animal's product, a
//! quest, breaking another block) may be commoner, never rarer. A find (a drop's own rarity,
//! which decides its level bonus) is never rarer than its source's class, and may be
//! commoner: staples stay Common finds, keeping their extra-count bonus.
//!
//! A cooked food takes its raw input's label (draft 101): cooked duck is as uncommon as raw
//! duck. So does whatever cooking makes of a creature's drop or product.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use voxmar_engine::block::{BlockId, Rarity};
use voxmar_engine::quest::Reward;

const CLASSES: [Rarity; 5] = [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::VeryRare, Rarity::Legendary];

/// The class of a drop that comes `rate` per kill.
fn class(rate: f32) -> usize {
    [1.0 / 2.0, 1.0 / 30.0, 1.0 / 150.0, 1.0 / 400.0].iter().position(|&least| rate >= least).unwrap_or(4)
}

fn rank(rarity: Rarity) -> usize {
    CLASSES.iter().position(|&c| c == rarity).expect("a class")
}

#[test]
fn creature_drops_are_labelled_by_their_easiest_source() {
    let content = voxmar_server::load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
    let blocks = &content.blocks;
    // What else gives each item.
    let mut elsewhere: BTreeSet<BlockId> = BTreeSet::new();
    elsewhere.extend(content.recipes.iter().map(|r| r.output));
    elsewhere.extend(content.cooking.iter().map(|(_, r)| r.output));
    elsewhere.extend(content.crops.iter().flat_map(|(_, c)| c.harvest.iter().chain(&c.forage).map(|(item, _)| *item).chain(c.seed)));
    elsewhere.extend(content.fishing.iter().map(|c| c.item));
    elsewhere.extend(content.creatures.iter().flat_map(|(_, c)| c.products.iter().copied()));
    for quest in content.quests.iter() {
        for reward in &quest.rewards {
            if let Reward::Item(name, _) = reward {
                elsewhere.extend(blocks.id(name).or_else(|| blocks.id(&format!("base:{name}"))));
            }
        }
    }
    elsewhere.extend((0..blocks.len()).map(|i| BlockId(i as u16)).filter(|&b| blocks.info(b).drops != b).map(|b| blocks.info(b).drops));

    let mut easiest: BTreeMap<BlockId, (usize, String)> = BTreeMap::new();
    let mut wrong = Vec::new();
    for (_, creature) in content.creatures.iter() {
        let shift = usize::from(creature.def.rarity >= Rarity::Uncommon);
        for entry in &creature.loot {
            let loot = &entry.loot;
            let tool = loot.tools.iter().map(|t| t.chance).fold(1.0, f32::max);
            let extra = loot.tools.iter().map(|t| t.extra).max().unwrap_or(0);
            let mut rate = (loot.chance * tool).min(1.0);
            if loot.count.0 == 0 && extra == 0 {
                let most = loot.count.1 as f32;
                rate *= most / (most + 1.0);
            }
            let class = (class(rate) + shift).min(4);
            let source = format!("{} (1 in {:.1}{})", creature.def.name, 1.0 / rate, if shift > 0 { format!(", {:?}", creature.def.rarity) } else { String::new() });
            if rank(loot.rarity) > class {
                wrong.push(format!("{}: the find from {source} is {:?}, rarer than {:?}", blocks.def(entry.item).name, loot.rarity, CLASSES[class]));
            }
            let best = easiest.entry(entry.item).or_insert((class, source.clone()));
            if class < best.0 {
                *best = (class, source);
            }
        }
    }
    for (&item, (class, source)) in &easiest {
        let def = blocks.def(item);
        let label = rank(def.rarity);
        let other = elsewhere.contains(&item);
        if label > *class || (label < *class && !other) {
            wrong.push(format!("{}: labelled {:?}, but its easiest source, {source}, makes it {:?}{}", def.name, def.rarity, CLASSES[*class], if other { "" } else { " (only creatures drop it)" }));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn cooked_food_takes_its_raw_inputs_label() {
    let content = voxmar_server::load_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
    let blocks = &content.blocks;
    let tagged = |item: BlockId, tag: &str| blocks.def(item).tags.iter().any(|t| t == tag);
    // What creatures give: their drops and kept animals' products.
    let from_creatures: BTreeSet<BlockId> = content.creatures.iter().flat_map(|(_, c)| c.loot.iter().map(|l| l.item).chain(c.products.iter().copied())).collect();
    let mut wrong = Vec::new();
    let mut checked = 0;
    // Every recipe that cooks a creature's drop or product, or one raw item into a cooked
    // one; a recipe for a whole tag has no one input to follow.
    for (input, recipe) in content.cooking.iter().filter(|(_, r)| !r.def.input.starts_with("tag:")) {
        if !from_creatures.contains(&input) && !(tagged(input, "raw") && tagged(recipe.output, "cooked")) {
            continue;
        }
        checked += 1;
        let (raw, cooked) = (blocks.def(input), blocks.def(recipe.output));
        if raw.rarity != cooked.rarity {
            wrong.push(format!("{} is {:?} but {} is {:?}", cooked.name, cooked.rarity, raw.name, raw.rarity));
        }
    }
    assert!(checked >= 20, "only {checked} raw foods are cooked");
    assert!(wrong.is_empty(), "{wrong:#?}");
}
