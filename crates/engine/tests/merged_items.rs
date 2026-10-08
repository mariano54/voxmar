//! One item per material (design plan P27, second round): the wild plants that were
//! several items each (five sweet berries, eight kitchen herbs, six mushrooms...) stay in
//! the world as blocks but drop one merged item, and nothing hands out, asks for or makes
//! a merged-away plant as an item.

use std::path::{Path, PathBuf};

use voxmar_engine::content::Content;

fn files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, found);
        } else if path.extension().is_some_and(|e| e == "ron") {
            found.push(path);
        }
    }
}

/// What the plants merge into, as the plan wrote it.
const MERGES: &[(&str, &[&str])] = &[
    ("sweet_berry", &["sweet_berry_bush", "blueberry_bush", "raspberry_bush", "blackberry_bush", "cloudberry"]),
    ("tart_berry", &["cranberry_bush", "lingonberry_bush", "gooseberry_bush"]),
    ("kitchen_herbs", &["oregano", "parsley", "dill", "fennel", "coriander", "basil", "rosemary", "cumin"]),
    ("wild_roots", &["wild_turnip", "wild_beet", "wild_radish", "wild_parsnip"]),
    ("wild_onion", &["wild_leek", "wild_garlic"]),
    ("wild_grain", &["wild_barley", "wild_oats", "wild_millet", "wild_quinoa"]),
    ("mushroom", &["porcini", "shiitake", "blewit", "oyster_mushroom", "chanterelle", "brown_mushroom"]),
    ("pumpkin", &["wild_pumpkin", "wild_squash"]),
];

#[test]
fn merged_away_plants_drop_the_merged_item_and_are_named_nowhere_as_items() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let blocks = &content.blocks;
    let mut wrong = Vec::new();
    for (into, plants) in MERGES {
        let Some(target) = blocks.id(&format!("base:{into}")) else {
            wrong.push(format!("no merged item {into}"));
            continue;
        };
        if blocks.def(target).has_tag("merged") {
            wrong.push(format!("{into} is itself merged away"));
        }
        for plant in *plants {
            let Some(id) = blocks.id(&format!("base:{plant}")) else {
                wrong.push(format!("no plant {plant}"));
                continue;
            };
            if blocks.item_named(&format!("base:{plant}")) != Some(target) {
                wrong.push(format!("{plant} is not held as {into} by an old save"));
            }
            if blocks.info(id).drops != target {
                wrong.push(format!("{plant} drops {} not {into}", blocks.def(blocks.info(id).drops).name));
            }
            if !blocks.def(id).has_tag("merged") {
                wrong.push(format!("{plant} lacks the `merged` tag"));
            }
        }
    }
    // Every block that says it merged is in the list above.
    let listed: Vec<String> = MERGES.iter().flat_map(|(_, plants)| plants.iter().map(|p| format!("base:{p}"))).collect();
    for id in blocks.ids() {
        let def = blocks.def(id);
        if def.has_tag("merged") && !listed.contains(&def.name) && blocks.info(id).drops != id && !blocks.placement_form(id) {
            wrong.push(format!("{} is tagged merged but not in the plan's list", def.name));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // Where items are made, asked for or handed out, the merged-away plants are not named.
    // Left alone: the files that place plants in the world and their text.
    let world = ["blocks", "plants.ron", "biomes.ron", "cave_biomes.ron", "decorations.ron", "land.ron", "terrain.ron", "trees.ron"];
    let mut all = Vec::new();
    files(&dir, &mut all);
    let mut named = Vec::new();
    for path in all {
        let rel = path.strip_prefix(&dir).unwrap().to_string_lossy().into_owned();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        if rel.contains("/lang/") || world.iter().any(|w| file.starts_with(w)) {
            continue;
        }
        for (n, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
            // What a creature eats in the world is a plant block, not an item.
            if line.contains("diet:") || line.trim_start().starts_with("//") {
                continue;
            }
            for (_, plants) in MERGES {
                for plant in *plants {
                    if line.contains(&format!("\"{plant}\"")) || line.contains(&format!("\"base:{plant}\"")) {
                        named.push(format!("{rel}:{}: {plant}", n + 1));
                    }
                }
            }
        }
    }
    // A merged-into item may share a name with one of its plants (wild_onion).
    named.retain(|n| !n.ends_with(": wild_onion"));
    assert!(named.is_empty(), "merged-away plants named as items:\n{}", named.join("\n"));
}
