//! A `.ron` in a package that no part of the game reads loads, and the load notes it, naming
//! the file that was likely meant: the game's own packages hold none.

use std::path::{Path, PathBuf};

use voxmar_engine::content::Content;

/// The game's packages, and a `zz_mod` package holding `files`, in a fresh directory.
fn with_mod(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_mod")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    for (file, source) in files {
        std::fs::write(dir.join("zz_mod").join(file), source).unwrap();
    }
    dir
}

const UNREAD: &str = "is read by nothing";

#[test]
fn the_games_packages_hold_no_file_nothing_reads() {
    let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap();
    let unread: Vec<&String> = content.notes.iter().filter(|n| n.contains(UNREAD)).collect();
    assert!(unread.is_empty(), "{unread:?}: a new file wants its reader, and its name in READ_ELSEWHERE");
}

#[test]
fn a_file_nothing_reads_is_noted_with_what_was_meant() {
    let quests = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "A", why: "", tasks: [[Home]])])"#;
    let dir = with_mod("unread-files", &[("quest.ron", quests), ("my_notes.ron", "()"), ("fish.ron", "()"), ("creature_horses.ron", "()")]);
    std::fs::create_dir_all(dir.join("zz_mod").join("old.ron")).unwrap();
    std::os::unix::fs::symlink(dir.join("zz_mod").join("old.ron"), dir.join("zz_mod").join("linked.ron")).unwrap();
    let content = Content::load(&dir).unwrap();
    let unread: Vec<&str> = content.notes.iter().map(String::as_str).filter(|n| n.contains(UNREAD)).collect();
    assert_eq!(
        unread,
        [
            "zz_mod/creature_horses.ron is read by nothing: of its family the game reads only creature_motion.ron, creature_sounds.ron",
            "zz_mod/fish.ron is read by nothing: no part of the game reads a file of that name",
            "zz_mod/my_notes.ron is read by nothing: no part of the game reads a file of that name",
            "zz_mod/quest.ron is read by nothing (meant quests.ron?)",
        ],
        "files in name order, the misspelt one naming its likeliest, a short name (fish, not fire) only a letter off; a topic far from its family's files names them; a folder named old.ron, or a link to it, is no file"
    );
    assert!(!content.quests.iter().any(|q| q.name == "zz_mod:a"), "a misspelt quest file's quests are not loaded");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Each name other crates read is a file the game ships, so a reader renamed or dropped does
/// not leave its old name quietly accepted. (A reader's file missing from the list is caught
/// above: the shipped file would be read by nothing.)
#[test]
fn every_name_read_elsewhere_ships_in_the_games_packages() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let missing: Vec<&str> = voxmar_engine::content::READ_ELSEWHERE.iter().copied().filter(|name| !["base", "core"].iter().any(|package| assets.join(package).join(name).is_file())).collect();
    assert!(missing.is_empty(), "{missing:?}: in READ_ELSEWHERE, but no game package holds the file");
}
