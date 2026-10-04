//! Shader packs: named looks for the lighting, haze and colour grading, read from
//! `assets/shaderpacks/*.ron`. A pack only scales and tints what the renderer already
//! computes, so every pack costs the same to draw; the player picks one in settings.

use std::path::Path;

use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use serde::Deserialize;

use super::DisplayTransform;
use crate::weather::atmosphere::ClientAtmosphere;

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
    /// Multiplies the sunlight the ground bounces onto walls and undersides (0 turns it
    /// off), which keeps shaded sides from going black beside sunlit ground.
    pub bounce: f32,
    /// Multiplies how quickly distant terrain fades into haze.
    pub haze: f32,
    /// Tints the horizon and haze.
    pub haze_tint: Vec3,
    /// How thick ground fog (valley fog, dawn mist, sea fog) and the weather's fog (mist,
    /// whiteout, rain, dust) are, relative to the weather's own.
    pub mist: f32,
    pub fog: f32,
    /// How much the air of the place shows (`weather::atmosphere::ClientAtmosphere`: a
    /// biome's or climate's dusty, damp or cold haze, its sky tint and grading nudges), as
    /// a nudge on top of the pack's own look; 0 ignores it.
    pub local_air: f32,
    /// Shadow penumbra width per block between a caster and the shade it casts (the sun's
    /// apparent size); 0 keeps every shadow edge as sharp as the shadow map allows.
    pub shadow_softness: f32,
    pub exposure: f32,
    /// Spreads tones around mid grey (1 leaves them).
    pub contrast: f32,
    pub saturation: f32,
    /// Raises the saturation of dull colours more than vivid ones.
    pub vibrance: f32,
    /// Shifts the image towards warm (positive) or cool (negative).
    pub warmth: f32,
    pub bloom: f32,
    /// Multiplies how many particles effects spawn (0 turns them off).
    pub particles: f32,
    /// How strongly cracks show over a block being mined, 0..1.
    pub cracks: f32,
    pub water: WaterLook,
}

/// How water looks (see `render::water`). Each strength scales the renderer's own, so 1
/// leaves it and 0 switches that part off.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct WaterLook {
    /// How quickly light fades through water: higher is murkier, lower clearer.
    pub absorption: f32,
    /// How much light the water itself scatters back: the brightness of deep water.
    pub scattering: f32,
    /// Reflections of the terrain and sky.
    pub reflections: f32,
    pub waves: f32,
    /// How far waves bend the view of the bed.
    pub refraction: f32,
    /// Foam along shores.
    pub foam: f32,
    /// One wave normal per texel of the block textures, so water glitters in little
    /// squares up close, as in Complementary's Reimagined style.
    pub pixelated: bool,
}

impl Default for WaterLook {
    fn default() -> Self {
        Self { absorption: 1.0, scattering: 1.0, reflections: 1.0, waves: 1.0, refraction: 1.0, foam: 1.0, pixelated: false }
    }
}

impl Default for ShaderPack {
    fn default() -> Self {
        Self {
            name: "Classic".into(),
            sun: 1.0,
            ambient: 1.0,
            shade_tint: Vec3::ONE,
            bounce: 1.0,
            haze: 1.0,
            haze_tint: Vec3::ONE,
            mist: 1.0,
            fog: 1.0,
            local_air: 0.5,
            shadow_softness: 0.015,
            exposure: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            vibrance: 0.0,
            warmth: 0.0,
            bloom: 0.1,
            particles: 1.0,
            cracks: 1.0,
            water: WaterLook::default(),
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

/// Applies the active pack's grading and bloom to the camera, with the place's grading
/// nudges (warmth and saturation of the local air) as far as the pack lets them show.
pub fn apply_pack(pack: Res<ActivePack>, air: Res<ClientAtmosphere>, mut camera: Query<(&mut DisplayTransform, &mut Bloom), With<Camera3d>>) {
    let share = pack.local_air.clamp(0.0, 1.0);
    let mut grading = DisplayTransform::from(&pack.0);
    grading.warmth += air.warmth * LOCAL_WARMTH * share;
    grading.saturation *= 1.0 + (air.saturation - 1.0) * share;
    for (mut display, mut bloom) in &mut camera {
        display.set_if_neq(grading);
        if bloom.intensity != pack.bloom {
            bloom.intensity = pack.bloom;
        }
    }
}

/// How far the local air's warmth (−1..1) shifts the grading's.
const LOCAL_WARMTH: f32 = 0.08;
