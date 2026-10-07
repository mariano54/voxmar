//! Shader packs: named looks for the lighting, haze and colour grading, read from
//! `assets/shaderpacks/*.ron`. A pack only scales and tints what the renderer already
//! computes, so every pack costs the same to draw; the player picks one in settings.
//! The packs the game ships (listed in `shaderpacks/shipped.ron`) load first, so a pack
//! dropped in beside them can't take a shipped pack's name.

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
    /// Where it was read from (`shaderpacks/verdant.ron`); none for the built-in Classic.
    #[serde(skip)]
    pub source: Option<String>,
    /// Whether the game ships it.
    #[serde(skip)]
    pub shipped: bool,
    /// Whether lang may name it: not when a pack loaded before it has the same key.
    #[serde(skip)]
    pub keyed: bool,
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
            source: None,
            shipped: false,
            keyed: true,
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
    /// The key lang names it by: its name in lower case, with anything but a letter or a
    /// digit made `_` (`shader_pack.verdant`, `shader_pack.新緑`, `shader_pack.r_d_v1_2` for
    /// "R&D v1.2").
    pub fn key(&self) -> String {
        let id: String = self.name.chars().flat_map(char::to_lowercase).map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        format!("shader_pack.{id}")
    }

    /// Its name in `locale`: lang's, if lang names it and its key is its own, or its own.
    pub fn named<'a>(&'a self, locale: &'a voxmar_engine::lang::Lang) -> &'a str {
        if self.keyed { locale.get_or(&self.key(), &self.name) } else { &self.name }
    }

    fn from(&self) -> &str {
        self.source.as_deref().unwrap_or("the game itself")
    }
}

/// Every pack found, the built-in Classic look first, then the shipped ones in the order
/// `shipped.ron` lists them, then any others by file name.
#[derive(Resource)]
pub struct ShaderPacks(pub Vec<ShaderPack>);

/// The list of the packs the game ships, in `shaderpacks`.
const SHIPPED: &str = "shipped.ron";

impl ShaderPacks {
    pub fn load(assets: &Path) -> Self {
        let mut packs = vec![ShaderPack { shipped: true, ..default() }];
        let dir = assets.join("shaderpacks");
        let shipped: Vec<String> = match std::fs::read_to_string(dir.join(SHIPPED)) {
            Ok(text) => ron::from_str(&text).unwrap_or_else(|e| {
                warn!("could not read shaderpacks/{SHIPPED}: {e}");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let file_name = |path: &Path| path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "ron") && file_name(path) != SHIPPED)
            .collect();
        files.sort_by_cached_key(|path| (shipped.iter().position(|name| *name == file_name(path)).unwrap_or(usize::MAX), file_name(path)));
        for path in files {
            let source = format!("shaderpacks/{}", file_name(&path));
            let mut pack = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| ron::from_str::<ShaderPack>(&s).map_err(|e| e.to_string())) {
                Ok(pack) => pack,
                Err(e) => {
                    warn!("could not read shader pack {source}: {e}");
                    continue;
                }
            };
            if let Some(first) = packs.iter().find(|p| p.name == pack.name) {
                warn!("{source}: refused, a shader pack named {} is already loaded from {}", pack.name, first.from());
                continue;
            }
            // Named alike ("Soft Light", "soft_light"), it would share a name in every
            // language with the one before it: it keeps its own.
            if let Some(first) = packs.iter().find(|p| p.keyed && p.key() == pack.key()) {
                warn!("{source}: {} keeps its own name in every language, as {} from {} is named by {}", pack.name, first.name, first.from(), pack.key());
                pack.keyed = false;
            }
            pack.shipped = shipped.contains(&file_name(&path));
            pack.source = Some(source);
            packs.push(pack);
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

    /// The packs a folder of `(file, name)` loads, by name, file and whether lang may name
    /// them, with `shipped` listing the files the game ships.
    fn load(files: &[(&str, &str)], shipped: &[&str]) -> Vec<(String, String, bool)> {
        let dir = std::env::temp_dir().join(format!("voxmar-packs-{}-{}", std::process::id(), files.iter().map(|(file, _)| *file).collect::<Vec<_>>().join("-")));
        std::fs::create_dir_all(dir.join("shaderpacks")).unwrap();
        for (file, name) in files {
            std::fs::write(dir.join("shaderpacks").join(file), format!("(name: {name:?})")).unwrap();
        }
        std::fs::write(dir.join("shaderpacks").join(SHIPPED), format!("{shipped:?}")).unwrap();
        let packs = ShaderPacks::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        packs.0.into_iter().skip(1).map(|p| (p.name, p.source.unwrap(), p.keyed)).collect()
    }

    /// A pack's key keeps its name's letters and digits, in any script, in lower case, and
    /// makes the rest `_`.
    #[test]
    fn a_pack_is_keyed_by_its_name_in_letters_and_digits() {
        assert_eq!(named("Verdant").key(), "shader_pack.verdant");
        assert_eq!(named("Classic").key(), "shader_pack.classic");
        assert_eq!(named("R&D v1.2").key(), "shader_pack.r_d_v1_2");
        assert_eq!(named("Café").key(), "shader_pack.café");
        assert_eq!(named("新緑").key(), "shader_pack.新緑");
        assert_ne!(named("新緑").key(), named("深海").key());
        assert_eq!(named("Soft Light").key(), named("soft_light").key());
    }

    /// Packs with names of their own all load, however they are written; one named alike
    /// to one before it loads too, but keeps its own name; one with a name already taken is
    /// refused; and a shipped pack loads before a dropped-in one, so it keeps its name.
    #[test]
    fn packs_load_once_by_name_and_shipped_first() {
        let packs = load(&[("a.ron", "新緑"), ("b.ron", "深海"), ("c.ron", "Café")], &[]);
        assert_eq!(packs.iter().map(|(name, ..)| name.as_str()).collect::<Vec<_>>(), ["新緑", "深海", "Café"]);
        assert!(packs.iter().all(|(.., keyed)| *keyed));
        let packs = load(&[("a.ron", "Soft Light"), ("b.ron", "soft_light"), ("c.ron", "Soft Light")], &[]);
        assert_eq!(packs, [("Soft Light".into(), "shaderpacks/a.ron".into(), true), ("soft_light".into(), "shaderpacks/b.ron".into(), false)]);
        let (lang, _) = voxmar_engine::lang::Lang::from_files("en", [("core", "en", r#"{ "shader_pack.soft_light": "Lang's Soft Light" }"#)]);
        let second = ShaderPack { keyed: false, ..named("soft_light") };
        assert_eq!((named("Soft Light").named(&lang), second.named(&lang)), ("Lang's Soft Light", "soft_light"));
        let packs = load(&[("aaa.ron", "Verdant"), ("verdant.ron", "Verdant")], &["verdant.ron"]);
        assert_eq!(packs, [("Verdant".into(), "shaderpacks/verdant.ron".into(), true)]);
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        // The game's own: Classic, built in, then what shipped.ron lists.
        let real = ShaderPacks::load(&assets);
        let shipped: Vec<&str> = real.0.iter().filter(|p| p.shipped && p.keyed).map(|p| p.name.as_str()).collect();
        assert_eq!(shipped, ["Classic", "Verdant"]);
    }
}
