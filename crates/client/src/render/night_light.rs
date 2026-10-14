//! How bright a night is (`night_light.ron`): the moon's light, the sky's light and colour
//! under a full moon, and how much of that a moonless night keeps. Every package may give
//! the file, a later one changing what an earlier gave; the game refuses to start on a
//! night outside the bounds Mariano's playtests set, so no pack can make nights black or
//! bright as dusk.

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;

pub const FILE: &str = "night_light.ron";

/// The night's light under a full moon at midnight, linear RGB before the shader pack's
/// scales and tints, and how much of the sky's a moonless night keeps.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct NightLight {
    /// The full moon's direct light on a face turned to it: it gives moonlit faces their
    /// edge over the shade, so silhouettes and walls read.
    pub moon: Vec3,
    /// The sky's light on open ground (all that lights the shade), the zenith's and the
    /// horizon's colour.
    pub ambient: Vec3,
    pub zenith: Vec3,
    pub horizon: Vec3,
    /// How much of the sky's light, zenith and horizon a night without a moon keeps.
    pub moonless: f32,
}

/// The sky's light at night (luminance) at and under which a moonless night's ground went
/// black, and at and over which nights had far too much light (Mariano, 2026-10-06/07: a
/// sixth and twice the 2026-10-07 ambient of 0.05, 0.066, 0.115).
const BLACK: f32 = 0.011;
const TOO_BRIGHT: f32 = 0.132;
/// Night may not outshine the day's light: each colour stays under these.
const DAY_AMBIENT: f32 = 0.40;
const DAY_SUN: f32 = 0.62;
const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

impl Default for NightLight {
    /// The base package's nights, for tests and anything drawn before content loads.
    fn default() -> Self {
        Self {
            moon: Vec3::new(0.025, 0.034, 0.060),
            ambient: Vec3::new(0.0425, 0.056, 0.098),
            zenith: Vec3::new(0.0012, 0.0018, 0.0045),
            horizon: Vec3::new(0.0062, 0.0088, 0.0175),
            moonless: 0.5,
        }
    }
}

impl NightLight {
    /// Reads every package's file in order, the last one giving the night, and refuses a
    /// night that is missing or out of bounds.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut night = None;
        for file in files.iter().filter(|f| f.name == FILE) {
            night = Some(ron::from_str::<NightLight>(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?);
        }
        let night = night.ok_or(format!("no package gives {FILE}"))?;
        night.check().map_err(|e| format!("{FILE}: {e}"))?;
        Ok(night)
    }

    /// Colours finite and not below nothing, under the day's; a moonless share of 0..=1;
    /// and the sky's light under every moon between black and too bright.
    pub fn check(&self) -> Result<(), String> {
        let colours = [("moon", self.moon, DAY_SUN), ("ambient", self.ambient, DAY_AMBIENT), ("zenith", self.zenith, DAY_AMBIENT), ("horizon", self.horizon, DAY_AMBIENT)];
        for (name, colour, day) in colours {
            if !colour.is_finite() || colour.min_element() < 0.0 {
                return Err(format!("{name} {colour} must be finite and not below 0"));
            }
            if colour.max_element() >= day {
                return Err(format!("{name} {colour} must stay under the day's {day}"));
            }
        }
        if !(0.0..=1.0).contains(&self.moonless) {
            return Err(format!("moonless {} must be within 0..=1", self.moonless));
        }
        let (full, new) = (self.ambient.dot(LUMINANCE), self.ambient.dot(LUMINANCE) * self.moonless);
        if new <= BLACK {
            return Err(format!("a moonless night's sky light {new:.4} (ambient {full:.4} x moonless {}) is black: it must be over {BLACK}", self.moonless));
        }
        if full >= TOO_BRIGHT {
            return Err(format!("a full moon's sky light {full:.4} is far too much light: it must be under {TOO_BRIGHT}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> NightLight {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/base/", "night_light.ron")).unwrap();
        NightLight::load(&[ContentFile { package: "base".into(), name: FILE.into(), source }]).unwrap()
    }

    #[test]
    fn the_base_night_is_the_default() {
        assert_eq!(base(), NightLight::default());
    }

    #[test]
    fn nights_out_of_bounds_are_refused() {
        let night = NightLight::default();
        let refused = |n: NightLight| n.check().unwrap_err();
        assert!(refused(NightLight { moonless: 0.1, ..night }).contains("black"));
        assert!(refused(NightLight { ambient: night.ambient * 3.0, ..night }).contains("too much"));
        assert!(refused(NightLight { moonless: 1.5, ..night }).contains("moonless"));
        assert!(refused(NightLight { moon: Vec3::new(-0.1, 0.0, 0.0), ..night }).contains("moon"));
        assert!(refused(NightLight { horizon: Vec3::splat(f32::NAN), ..night }).contains("horizon"));
        assert!(refused(NightLight { zenith: Vec3::splat(0.5), ..night }).contains("day"));
    }

    #[test]
    fn a_later_package_changes_the_night_and_none_is_refused() {
        let given = |package: &str, moonless: f32| ContentFile {
            package: package.into(),
            name: FILE.into(),
            source: format!("(moon: (0.02, 0.03, 0.05), ambient: (0.04, 0.05, 0.09), zenith: (0.001, 0.002, 0.004), horizon: (0.005, 0.007, 0.014), moonless: {moonless})"),
        };
        assert_eq!(NightLight::load(&[given("base", 0.5), given("dark", 0.4)]).unwrap().moonless, 0.4);
        assert!(NightLight::load(&[]).unwrap_err().contains("no package"));
    }
}
