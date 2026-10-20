//! A package's quests may be split over `quests.ron` and `quests_<topic>.ron`: the game's own
//! packages load with a later one whose book is in two files, read in name order, and a
//! broken topic file, or one repeating a quest, fails the load naming its file.

use std::path::{Path, PathBuf};

use voxmar_engine::content::Content;

/// The game's packages, and a `zz_book` package holding `files`, in a fresh directory.
fn with_book(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_book")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    for (file, source) in files {
        std::fs::write(dir.join("zz_book").join(file), source).unwrap();
    }
    dir
}

const FIRST: &str = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "A", why: "", tasks: [[Home]], after: ["b"])])"#;
const TOPIC: &str = r#"(quests: [(name: "b", age: "nomad", branch: "main", title: "B", why: "", tasks: [[Home]])])"#;
const PATHS: &str = "(paths: [])";

#[test]
fn a_book_in_two_files_loads_in_name_order() {
    let dir = with_book("quest-files", &[("quests.ron", FIRST), ("quests_b.ron", TOPIC), ("quest_paths.ron", PATHS)]);
    let content = Content::load(&dir).unwrap();
    let book: Vec<&str> = content.quests.iter().map(|q| q.name.as_str()).filter(|n| n.starts_with("zz_book:")).collect();
    assert_eq!(book, ["zz_book:a", "zz_book:b"], "quests.ron first, then the topic, and an `after` from one file to the next");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_broken_or_repeating_topic_file_fails_the_load() {
    let dir = with_book("quest-files-broken", &[("quests.ron", FIRST), ("quests_b.ron", TOPIC), ("quest_paths.ron", PATHS), ("quests_c.ron", "(quests: [")]);
    let error = Content::load(&dir).err().expect("a broken quests_c.ron is refused");
    assert!(error.0.starts_with("zz_book/quests_c.ron:"), "{}", error.0);
    std::fs::remove_dir_all(&dir).unwrap();

    let again = r#"(quests: [(name: "a", age: "nomad", branch: "main", title: "A Again", why: "", tasks: [[Home]])])"#;
    let dir = with_book("quest-files-again", &[("quests.ron", FIRST), ("quests_b.ron", TOPIC), ("quest_paths.ron", PATHS), ("quests_c.ron", again)]);
    let error = Content::load(&dir).err().expect("a quests_c.ron repeating a quest is refused");
    assert!(error.0.contains("zz_book:a is written in zz_book/quests.ron and again in zz_book/quests_c.ron"), "{}", error.0);
    std::fs::remove_dir_all(&dir).unwrap();
}
