//! Shader packs: named looks for the lighting, haze and colour grading, read from
//! `assets/shaderpacks/*.ron`. A pack only scales and tints what the renderer already
//! computes, so every pack costs the same to draw; the player picks one in settings.

use std::path::Path;

use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use serde::Deserialize;

use super::DisplayTransform;

/// One look. Every field has a neutral default, so a pack lists only what it changes.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct ShaderPack {
    pub name: String,
    /// Multiplies the sun and moon.
    pub sun: f32,
    /// Multiplies the sky's ambient light, which is all that lights the shade.
    pub ambient: f32,
    /// Tints the ambient light, usually towards blue so shade reads cool against warm sun.
    pub shade_tint: Vec3,
    /// Multiplies how quickly distant terrain fades into haze.
    pub haze: f32,
    /// Tints the horizon and haze.
    pub haze_tint: Vec3,
    pub exposure: f32,
    /// Spreads tones around mid grey (1 leaves them).
    pub contrast: f32,
    pub saturation: f32,
    /// Raises the saturation of dull colours more than vivid ones.
    pub vibrance: f32,
    /// Shifts the image towards warm (positive) or cool (negative).
    pub warmth: f32,
    pub bloom: f32,
}

impl Default for ShaderPack {
    fn default() -> Self {
        Self {
            name: "Classic".into(),
            sun: 1.0,
            ambient: 1.0,
            shade_tint: Vec3::ONE,
            haze: 1.0,
            haze_tint: Vec3::ONE,
            exposure: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            vibrance: 0.0,
            warmth: 0.0,
            bloom: 0.1,
        }
    }
}

/// Every pack found, the built-in Classic look first.
#[derive(Resource)]
pub struct ShaderPacks(pub Vec<ShaderPack>);

impl ShaderPacks {
    pub fn load(assets: &Path) -> Self {
        let mut packs = vec![ShaderPack::default()];
        let dir = assets.join("shaderpacks");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "ron"))
            .collect();
        files.sort();
        for path in files {
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| ron::from_str::<ShaderPack>(&s).map_err(|e| e.to_string())) {
                Ok(pack) if packs.iter().all(|p| p.name != pack.name) => packs.push(pack),
                Ok(pack) => warn!("{}: a shader pack called {} already exists", path.display(), pack.name),
                Err(e) => warn!("could not read shader pack {}: {e}", path.display()),
            }
        }
        Self(packs)
    }

    /// The pack with this name, or Classic.
    pub fn get(&self, name: &str) -> &ShaderPack {
        self.0.iter().find(|p| p.name == name).unwrap_or(&self.0[0])
    }

    /// The name of the pack after `name`, wrapping around.
    pub fn next(&self, name: &str) -> &str {
        let i = self.0.iter().position(|p| p.name == name).map_or(0, |i| i + 1);
        &self.0[i % self.0.len()].name
    }
}

/// The pack in use.
#[derive(Resource, Clone, Default, Deref)]
pub struct ActivePack(pub ShaderPack);

/// Applies the active pack's grading and bloom to the camera when it changes.
pub fn apply_pack(pack: Res<ActivePack>, mut camera: Query<(&mut DisplayTransform, &mut Bloom), With<Camera3d>>) {
    if !pack.is_changed() {
        return;
    }
    for (mut display, mut bloom) in &mut camera {
        *display = DisplayTransform::from(&pack.0);
        bloom.intensity = pack.bloom;
    }
}
