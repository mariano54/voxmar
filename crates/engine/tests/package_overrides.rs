//! A mod changes single-valued data (Design draft 91: a later package replaces it, and a
//! load note names both packages): the game's own packages load with a later one that sets
//! one creature limit, one Defence training number and one condition rule, or the level
//! curve, and the later package's numbers win, the rest keeping base's.

use std::path::Path;

use voxmar_engine::content::Content;

#[test]
fn a_later_package_replaces_single_numbers_with_notes() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-limits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_herds")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_herds/creatures.ron"), "(creatures: [], limits: (animals_near: 4), defence: (least_level: 0.75), condition: (up: 0.3))").unwrap();

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
    let (rules, base_rules) = (content.creatures.condition(), game.creatures.condition());
    assert_eq!((rules.up, rules.down, rules.tamed), (0.3, base_rules.down, base_rules.tamed));
    let notes: Vec<_> = content.notes.iter().filter(|n| n.contains("condition rules")).collect();
    assert_eq!(notes.len(), 1, "{:?}", content.notes);
    assert!(notes[0].contains("zz_herds") && notes[0].contains("base") && notes[0].contains("(up)"), "{}", notes[0]);
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

#[test]
fn a_later_package_replaces_treasure_settings_with_both_package_names() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-treasure-settings-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("zz_treasure")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_treasure/treasure.ron"), r#"(lucky: 0.1, gold: (item: "base:gold_coin", centre: 2.5, spread: 0.5))"#).unwrap();
    let content = Content::load(&dir).unwrap();
    assert_eq!(content.treasure.lucky, 0.1);
    let chest = content.treasure.chest(content.blocks.expect("base:nomad_treasure_chest")).unwrap();
    assert_eq!(chest.gold, (6, 9));
    for what in ["treasure lucky chance", "treasure gold"] {
        let notes: Vec<_> = content.notes.iter().filter(|n| n.contains(what)).collect();
        assert_eq!(notes.len(), 1, "{:?}", content.notes);
        assert!(notes[0].contains("zz_treasure/treasure.ron") && notes[0].contains("base"), "{}", notes[0]);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The same rule for the other single-valued files: a second package that changes one
/// number of how players move, what things are worth, the weather's tuning or a journal
/// page replaces just that number, and one load note names both packages.
#[test]
fn a_second_package_changes_one_number_of_each_file_and_each_note_names_both_packages() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_tuned")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    let files = [
        ("movement.ron", "(walk_speed: 5.0, checks: (burst: 3.0))"),
        ("value.ron", "(base: 3.0, death: (kept: 2))"),
        ("weather.ron", "(tuning: (simulation: (mean_wind: 4.5)))"),
        ("journal.ron", "(pages: [(page: Fish, studied: 5, mastered: 50)])"),
    ];
    for (name, source) in files {
        std::fs::write(dir.join("zz_tuned").join(name), source).unwrap();
    }

    let game = Content::load(&assets).unwrap();
    let content = Content::load(&dir).unwrap();
    // Only what the second package names changed.
    let (walk, was) = (&content.movement, &game.movement);
    assert_eq!((walk.walk_speed, walk.checks.burst), (5.0, 3.0));
    assert_eq!((walk.sprint_speed, walk.jump_velocity, walk.checks.speed_margin), (was.sprint_speed, was.jump_velocity, was.checks.speed_margin));
    assert_eq!((content.value.base, content.value.death.kept), (3.0, 2));
    assert_eq!((content.value.growth, content.value.death.grave), (game.value.growth, game.value.death.grave));
    assert_eq!(content.weather.tuning().simulation.mean_wind, 4.5);
    assert_eq!(content.weather.tuning().simulation.range, game.weather.tuning().simulation.range);
    // Each note names the file's package and the one it replaces, and the numbers.
    let note = |what: &str| -> String {
        let found: Vec<_> = content.notes.iter().filter(|n| n.contains(what)).collect();
        assert_eq!(found.len(), 1, "{what}: {:?}", content.notes);
        found[0].clone()
    };
    assert_eq!(note("movement"), "zz_tuned/movement.ron replaces the movement core set (walk_speed, checks.burst)");
    assert_eq!(note("worth"), "zz_tuned/value.ron replaces the worth base set (base, death.kept)");
    assert_eq!(note("weather tuning"), "zz_tuned/weather.ron replaces the weather tuning base set (simulation.mean_wind)");
    assert_eq!(note("journal"), "zz_tuned/journal.ron replaces the journal rules core set (Fish)");
    assert!(game.notes.is_empty(), "{:?}", game.notes);

    // A number that would hang or stop the server is refused with the package named,
    // though no one file is out of range alone.
    std::fs::write(dir.join("zz_tuned/movement.ron"), "(checks: (sweep_step: 0.0))").unwrap();
    let error = Content::load(&dir).err().expect("refused").to_string();
    assert!(error.contains("zz_tuned/movement.ron") && error.contains("sweep_step"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}
