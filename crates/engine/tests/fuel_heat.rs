//! A multiblock that asks a heat takes only fuel that burns at least that hot: a later
//! package's structure burning logs (heat 1) loads asking heat 1, and is refused asking 3.

use std::path::Path;

use voxmar_engine::content::Content;

#[test]
fn a_multiblock_fuel_cooler_than_its_heat_is_refused_at_load() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let load = |heat: u8| {
        let dir = std::env::temp_dir().join(format!("voxmar-fuel-heat-{}-{heat}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zz_oven")).unwrap();
        for package in ["base", "core"] {
            std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
        }
        let oven = format!(r#"(multiblocks: [(name: "log_oven", title: "Log Oven", fuels: [("base:oak_log", 4)], heat: {heat}, hint: "Planks all round.", layers: [["ppp", "pOp", "ppp"]], key: {{'p': "base:oak_planks", 'O': "base:crucible"}}, core: 'O')])"#);
        std::fs::write(dir.join("zz_oven/multiblocks.ron"), oven).unwrap();
        let loaded = Content::load(&dir).map(|_| ()).map_err(|e| e.to_string());
        let _ = std::fs::remove_dir_all(&dir);
        loaded
    };
    assert_eq!(load(1), Ok(()));
    let refused = load(3).unwrap_err();
    assert!(refused.contains("log_oven") && refused.contains("burns cooler"), "{refused}");
}
