//! The charm slot is checked as content loads: a charm is worn there and is no piece of a
//! set too, no set puts a piece there, `Wearing` asks only for what a charm carries, and
//! every charm is asked for by some effect. Each tried as a later package over the game's own.

use std::path::{Path, PathBuf};

use voxmar_engine::content::Content;

/// The game's packages and one more, `zz_charm`, holding `file` with `text`.
fn with(name: &str, file: &str, text: &str) -> PathBuf {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-charm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_charm")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_charm").join(file), text).unwrap();
    dir
}

fn refused(dir: &Path) -> String {
    let error = Content::load(dir).err().map(|e| e.to_string()).unwrap_or_default();
    let _ = std::fs::remove_dir_all(dir);
    error
}

#[test]
fn a_charm_is_worn_only_in_the_charm_slot() {
    // A charm listed as a set's body piece: armour and a charm both.
    let dir = with("armour", "combat.ron", r#"(sets: [(name: "tusk_mail", armour: 0.5, pieces: [(Body, "base:charge_charm")])])"#);
    let error = refused(&dir);
    assert!(error.contains("is a charm and armour or a shield too"), "refused as armour too: {error:?}");
    // A set's piece in the charm slot.
    let dir = with("set", "combat.ron", r#"(sets: [(name: "trinkets", armour: 0.5, pieces: [(Charm, "base:hide_hood")])])"#);
    let error = refused(&dir);
    assert!(error.contains("the charm slot holds a charm"), "a set piece refused the charm slot: {error:?}");
}

#[test]
fn wearing_asks_only_for_what_something_worn_carries() {
    // Torches carry `fire`, but no torch is worn: the effect could never hold.
    let dir = with("fire", "effects.ron", r#"(effects: [(name: "aflame", kind: Buff, title: "Aflame", description: "", when: Some(Wearing("fire")), modifiers: (speed: 1.05))])"#);
    let error = refused(&dir);
    assert!(error.contains("which nothing worn carries"), "Wearing(\"fire\") refused: {error:?}");
    // The game's own charm passes.
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    assert!(Content::load(&assets).is_ok());
}

#[test]
fn every_charm_is_asked_for_by_some_effect() {
    // A charm no `Wearing` effect asks for would be worn for nothing.
    let dir = with("idle", "blocks_charms.ron", r#"(blocks: [(name: "lucky_pebble", render: Plant, solid: false, item: true, textures: All("lucky_pebble"), hardness: 0.0, tags: ["charm"])])"#);
    let error = refused(&dir);
    assert!(error.contains("`zz_charm:lucky_pebble` is a charm, but no effect asks for it worn"), "an idle charm refused: {error:?}");
}

#[test]
fn a_worth_is_set_only_for_what_some_package_has() {
    let dir = with("worth", "value.ron", r#"(levels: {"base:no_such_charm": 10})"#);
    let error = refused(&dir);
    assert!(error.contains("a worth is set for `base:no_such_charm`, which no package has"), "an unknown name refused: {error:?}");
}
