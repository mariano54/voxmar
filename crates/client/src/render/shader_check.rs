//! The terrain shader composed with its imports and validated as the renderer would, for
//! every pass it is built for, without a GPU: a shader that would not compile fails here,
//! not only on the first frame. Bevy's own shader modules are read from its sources in
//! Cargo's registry (fetched, as this crate builds with Bevy), at the versions Cargo.lock
//! names. Here in the client, a shader edit runs it (`tools/gate` by changed crate).

use naga_oil::compose::{ComposableModuleDescriptor, Composer, NagaModuleDescriptor, ShaderDefValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WORKSPACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const RENDER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/render");

/// Each `.wgsl` under `dir`, with its text.
fn wgsl_files(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            wgsl_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "wgsl") {
            out.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
        }
    }
}

/// The sources of every Bevy crate Cargo.lock names, from Cargo's registry.
fn bevy_sources() -> Vec<PathBuf> {
    let lock = std::fs::read_to_string(format!("{WORKSPACE}/Cargo.lock")).unwrap();
    let crates: Vec<String> = lock
        .split("[[package]]")
        .filter_map(|package| {
            let field = |key: &str| package.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(" = \"")?.strip_suffix('"'));
            let name = field("name")?;
            let version = field("version")?;
            name.starts_with("bevy_").then(|| format!("{name}-{version}"))
        })
        .collect();
    let home = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| {
        let user = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).expect("CARGO_HOME, HOME or USERPROFILE names where Cargo keeps its registry");
        PathBuf::from(user).join(".cargo")
    });
    let sources = home.join("registry/src");
    let registries: Vec<PathBuf> = std::fs::read_dir(&sources)
        .unwrap_or_else(|e| panic!("Bevy's shader sources are read from Cargo's registry, {}: {e} (set CARGO_HOME; `cargo fetch` fills it)", sources.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    // Crates for other targets (bevy_android) need not be fetched; the two the terrain
    // shader imports from must be.
    let needed = |c: &&String| c.starts_with("bevy_pbr-") || c.starts_with("bevy_render-");
    let missing: Vec<&String> = crates.iter().filter(needed).filter(|c| !registries.iter().any(|r| r.join(c).is_dir())).collect();
    assert!(missing.is_empty(), "not in Cargo's registry at {} (set CARGO_HOME; `cargo fetch` fills it): {missing:?}", sources.display());
    crates.iter().filter_map(|c| registries.iter().map(|r| r.join(c)).find(|p| p.is_dir())).collect()
}

/// The defs Bevy loads its view types with (`MeshRenderPlugin`), as its `light.rs` limits.
fn module_defs() -> HashMap<String, ShaderDefValue> {
    [("MAX_DIRECTIONAL_LIGHTS", 10), ("MAX_CASCADES_PER_LIGHT", 4), ("MAX_RECT_LIGHTS", 8)]
        .map(|(def, value)| (def.to_string(), ShaderDefValue::UInt(value)))
        .into()
}

/// A composer holding every importable module: Bevy's and the client's own.
fn composer() -> Composer {
    let mut modules = Vec::new();
    for dir in bevy_sources() {
        wgsl_files(&dir.join("src"), &mut modules);
    }
    wgsl_files(Path::new(RENDER), &mut modules);
    modules.retain(|(_, source)| source.contains("#define_import_path"));
    let mut composer = Composer::default();
    // A module goes in once every module it imports has: added in rounds until none is left
    // or a round adds none (a module importing what nothing defines, left out).
    loop {
        let before = modules.len();
        modules.retain(|(path, source)| {
            composer
                .add_composable_module(ComposableModuleDescriptor {
                    source,
                    file_path: &path.to_string_lossy(),
                    shader_defs: module_defs(),
                    ..Default::default()
                })
                .is_err()
        });
        if modules.is_empty() || modules.len() == before {
            break;
        }
    }
    composer
}

#[test]
fn the_terrain_shader_composes_and_validates_for_every_pass() {
    let mut composer = composer();
    let source = std::fs::read_to_string(format!("{RENDER}/voxel.wgsl")).unwrap();
    // As `terrain.rs` `pass_setup`, for the main pass and the shadow (prepass) one.
    let passes: [&[&str]; 5] = [&[], &["CUTOUT"], &["CUTOUT", "INNER"], &["TRANSLUCENT"], &["CUTOUT", "FRINGE"]];
    let mut failures = Vec::new();
    for pass in passes {
        // The camera has distance fog (`render/mod.rs`); with several samples a pixel, the
        // view is multisampled and cut-outs go by alpha to coverage.
        let main = ["DISTANCE_FOG"];
        let multisampled = ["DISTANCE_FOG", "MULTISAMPLED", "ALPHA_TO_COVERAGE"];
        for variant in [&main[..], &multisampled, &["PREPASS_PIPELINE"], &["PREPASS_PIPELINE", "UNCLIPPED_DEPTH_ORTHO_EMULATION"]] {
            let mut defs: HashMap<String, ShaderDefValue> = pass.iter().chain(variant).map(|d| (d.to_string(), ShaderDefValue::Bool(true))).collect();
            // The pipeline cache's, for wgpu's least storage buffers per stage.
            defs.insert("AVAILABLE_STORAGE_BUFFER_BINDINGS".into(), ShaderDefValue::UInt(8));
            if let Err(error) = composer.make_naga_module(NagaModuleDescriptor { source: &source, file_path: "voxel.wgsl", shader_defs: defs, ..Default::default() }) {
                failures.push(format!("{pass:?} {variant:?}: {}", error.emit_to_string(&composer)));
            }
        }
    }
    assert!(failures.is_empty(), "voxel.wgsl does not compile:\n{}", failures.join("\n"));
}
