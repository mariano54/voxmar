//! Shader packs: named looks for the lighting, haze and colour grading, read from
//! `assets/shaderpacks/*.ron`. A pack only scales and tints what the renderer already
//! computes, so every pack costs the same to draw; the player picks one in settings.

use std::path::Path;

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
    /// Tints the sun's light, usually towards gold so sunlit ground reads warm against
    /// cool shade.
    pub sun_tint: Vec3,
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
    /// Strength of volumetric light: shafts of sun through canopies and the sun's glow in
    /// haze and mist. 0 switches the pass off.
    pub rays: f32,
    /// Shadow penumbra width per block between a caster and the shade it casts (the sun's
    /// apparent size); 0 keeps every shadow edge as sharp as the shadow map allows.
    pub shadow_softness: f32,
    /// How steeply sky light falls off into shade, an exponent of the sky light level:
    /// higher darkens forest floors, overhangs and the inside of crowns.
    pub sky_falloff: f32,
    /// How strongly corners and the bases of walls darken the sky light (an exponent of
    /// the voxel ambient occlusion).
    pub ambient_occlusion: f32,
    /// How brightly sunlit leaves and plants glow, most when seen against the sun.
    pub subsurface: f32,
    /// How dark the shadows of passing clouds are on the land (0 none, 1 no direct sun).
    pub cloud_shadows: f32,
    /// How brightly blocks that give off light glow by themselves.
    pub glow: f32,
    /// How much rain darkens and wets the ground and fills puddles, and how much frost and
    /// lying snow whiten it, relative to the weather's own.
    pub wet_ground: f32,
    pub frost: f32,
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
            sun_tint: Vec3::ONE,
            ambient: 1.0,
            shade_tint: Vec3::ONE,
            bounce: 1.0,
            haze: 1.0,
            haze_tint: Vec3::ONE,
            mist: 1.0,
            fog: 1.0,
            local_air: 0.5,
            rays: 0.0,
            shadow_softness: 0.015,
            sky_falloff: 2.2,
            ambient_occlusion: 1.0,
            subsurface: 0.0,
            cloud_shadows: 0.6,
            glow: 1.0,
            wet_ground: 1.0,
            frost: 1.0,
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

impl ShaderPack {
    /// The key lang names it by: its name in lower case, with anything but a letter a to z
    /// or a digit made `_` (`shader_pack.verdant`, `shader_pack.r_d_v1_2` for "R&D v1.2").
    /// A pack no language names goes by its own name.
    pub fn key(&self) -> String {
        let id: String = self.name.chars().map(|c| c.to_ascii_lowercase()).map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' }).collect();
        format!("shader_pack.{id}")
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
                // Two packs named alike would share a name in every language.
                Ok(pack) if packs.iter().all(|p| p.key() != pack.key()) => packs.push(pack),
                Ok(pack) => warn!("{}: a shader pack named like {} already exists ({})", path.display(), pack.name, pack.key()),
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
pub fn apply_pack(pack: Res<ActivePack>, air: Res<ClientAtmosphere>, mut camera: Query<(&mut DisplayTransform, &mut super::post::Glow), With<Camera3d>>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> ShaderPack {
        ShaderPack { name: name.into(), ..default() }
    }

    /// A pack's key keeps only lower-case letters and digits, so whatever its name holds the
    /// key is one lang can write, and packs named alike (by case, or a space for an
    /// underscore) are one pack: the first found.
    #[test]
    fn a_pack_is_keyed_by_its_name_in_plain_letters() {
        assert_eq!(named("Verdant").key(), "shader_pack.verdant");
        assert_eq!(named("Classic").key(), "shader_pack.classic");
        assert_eq!(named("R&D v1.2").key(), "shader_pack.r_d_v1_2");
        assert_eq!(named("Soft Light").key(), named("soft_light").key());
        assert_eq!(named("新緑").key(), "shader_pack.__");
        let dir = std::env::temp_dir().join(format!("voxmar-pack-keys-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("shaderpacks")).unwrap();
        std::fs::write(dir.join("shaderpacks/a.ron"), r#"(name: "Soft Light", sun: 2.0)"#).unwrap();
        std::fs::write(dir.join("shaderpacks/b.ron"), r#"(name: "soft_light", sun: 3.0)"#).unwrap();
        let packs = ShaderPacks::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let names: Vec<&str> = packs.0.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Classic", "Soft Light"]);
    }
}
