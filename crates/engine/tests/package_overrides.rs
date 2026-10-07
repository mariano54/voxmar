//! A mod changes single-valued data (Design draft 91: a later package replaces it, and a
//! load note names both packages): the game's own packages load with a later one that sets
//! one creature limit and one Defence training number, or the level curve, and the later
//! package's numbers win, the rest keeping base's.

use std::path::Path;

use voxmar_engine::content::Content;

#[test]
fn a_later_package_replaces_one_creature_limit_and_one_defence_number_with_notes() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-limits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_herds")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_herds/creatures.ron"), "(creatures: [], limits: (animals_near: 4), defence: (least_level: 0.75))").unwrap();

    let game = Content::load(&assets).unwrap();
    let base = game.creatures.limits();
    let content = Content::load(&dir).unwrap();
    let limits = content.creatures.limits();
    assert_eq!(limits.animals_near, 4);
    assert_eq!((limits.most, limits.wild_most, limits.kept_each, limits.monsters_near), (base.most, base.wild_most, base.kept_each, base.monsters_near));
    let notes: Vec<_> = content.notes.iter().filter(|n| n.contains("creature limits")).collect();
    assert_eq!(notes.len(), 1, "{:?}", content.notes);
    assert!(notes[0].contains("zz_herds") && notes[0].contains("base") && notes[0].contains("animals_near"), "{}", notes[0]);
    let defence = content.creatures.defence();
    assert_eq!((defence.least_level, defence.fought_within), (0.75, game.creatures.defence().fought_within));
    let notes: Vec<_> = content.notes.iter().filter(|n| n.contains("Defence training")).collect();
    assert_eq!(notes.len(), 1, "{:?}", content.notes);
    assert!(notes[0].contains("zz_herds") && notes[0].contains("base") && notes[0].contains("least_level"), "{}", notes[0]);
    // The game alone has nothing to note.
    assert!(game.notes.is_empty(), "{:?}", game.notes);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_later_package_replaces_the_level_curve_with_a_note() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-curve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_long")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_long/skills.ron"), "(curve: RuneScape(max: 120))").unwrap();

    assert_eq!(Content::load(&assets).unwrap().skills.max_level(), 99);
    let content = Content::load(&dir).unwrap();
    assert_eq!(content.skills.max_level(), 120);
    let notes: Vec<_> = content.notes.iter().filter(|n| n.contains("level curve")).collect();
    assert_eq!(notes.len(), 1, "{:?}", content.notes);
    assert!(notes[0].contains("zz_long") && notes[0].contains("core"), "{}", notes[0]);
    std::fs::remove_dir_all(&dir).unwrap();
}
