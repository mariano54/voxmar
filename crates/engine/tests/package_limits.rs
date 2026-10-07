//! A mod changes one creature limit: the game's own packages and a later one that sets
//! `animals_near` load together, the later package's number wins, the rest keep base's,
//! and a note names both packages (Design, 2026-10-07: a later package replaces
//! single-valued data).

use std::path::Path;

use voxmar_engine::content::Content;

#[test]
fn a_later_package_replaces_one_creature_limit_with_a_note() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-limits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_herds")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_herds/creatures.ron"), "(creatures: [], limits: (animals_near: 4))").unwrap();

    let base = Content::load(&assets).unwrap().creatures.limits();
    let content = Content::load(&dir).unwrap();
    let limits = content.creatures.limits();
    assert_eq!(limits.animals_near, 4);
    assert_eq!((limits.most, limits.wild_most, limits.kept_each, limits.monsters_near), (base.most, base.wild_most, base.kept_each, base.monsters_near));
    let notes: Vec<_> = content.notes.iter().filter(|n| n.contains("creature limits")).collect();
    assert_eq!(notes.len(), 1, "{:?}", content.notes);
    assert!(notes[0].contains("zz_herds") && notes[0].contains("base") && notes[0].contains("animals_near"), "{}", notes[0]);
    // The game alone has nothing to note.
    assert!(Content::load(&assets).unwrap().notes.iter().all(|n| !n.contains("creature limits")));
    std::fs::remove_dir_all(&dir).unwrap();
}
