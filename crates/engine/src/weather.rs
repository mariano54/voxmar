//! Weather: what the weather at a place is described by, and the kinds of weather
//! commands and captures can ask for by name.
//!
//! The server simulates the weather and tells each player the `Weather` where they are;
//! clients only draw it. Kinds of weather (`clear`, `rain`, `storm`, …) are data in each
//! package's `weather.ron`, so a mod can add its own.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::RegistryError;
use crate::glam::Vec2;

/// The weather at one place. Clients ease towards each update, so it may change in steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    /// Fraction of the sky covered by low cloud, 0..1.
    pub cover: f32,
    /// How that cloud is built: 0 flat layers (stratus) to 1 heaped towers (cumulus, and
    /// cumulonimbus when it rains).
    pub convection: f32,
    /// High, thin ice cloud (cirrus), 0..1. It often runs ahead of rain.
    pub cirrus: f32,
    /// Relative humidity of the air, 0..1.
    pub humidity: f32,
    /// Rain or snow reaching the ground, 0 (none) to 1 (downpour).
    pub precipitation: f32,
    /// How much of it falls as snow, 0..1.
    pub snow: f32,
    /// Wind along x and z, in blocks per second.
    pub wind: Vec2,
}

impl Weather {
    /// A short name for the weather: `heavy rain`, `cumulus`.
    pub fn describe(&self) -> &'static str {
        let falling = self.precipitation > 0.05;
        match () {
            _ if falling && self.snow > 0.5 => if self.precipitation > 0.5 { "heavy snow" } else { "snow" },
            _ if falling && self.convection > 0.5 && self.precipitation > 0.4 => "thunderstorm",
            _ if falling => if self.precipitation > 0.5 { "heavy rain" } else { "rain" },
            _ if self.cover > 0.8 => "overcast",
            _ if self.cover > 0.2 => if self.convection > 0.5 { "cumulus" } else { "cloudy" },
            _ if self.cirrus > 0.3 => "cirrus",
            _ => "clear",
        }
    }

    /// The numbers behind it, for the debug overlay and `/weather`.
    pub fn summary(&self) -> String {
        format!(
            "cloud {:.0}% ({:.0}% heaped), cirrus {:.0}%, humidity {:.0}%, {} {:.0}%, wind {:.1} blocks/s",
            self.cover * 100.0,
            self.convection * 100.0,
            self.cirrus * 100.0,
            self.humidity * 100.0,
            if self.snow > 0.5 { "snow" } else { "rain" },
            self.precipitation * 100.0,
            self.wind.length(),
        )
    }
}

/// A kind of weather, as `/weather` and `--weather` ask for it. Values left out are the
/// natural ones where it is forced (rain falls as snow where it is freezing).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preset {
    /// Namespaced once loaded (`base:rain`); asked for without the namespace.
    pub name: String,
    pub cover: Option<f32>,
    pub convection: Option<f32>,
    pub cirrus: Option<f32>,
    pub humidity: Option<f32>,
    pub precipitation: Option<f32>,
    pub snow: Option<f32>,
    /// Wind speed in blocks per second; it keeps blowing from where it does.
    pub wind: Option<f32>,
}

impl Preset {
    /// The short name, without the namespace.
    pub fn short_name(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// `natural` with this preset's values blended in by `weight` (0..1).
    pub fn blend(&self, natural: &Weather, weight: f32) -> Weather {
        let mix = |natural: f32, forced: Option<f32>| forced.map_or(natural, |f| natural + (f - natural) * weight);
        let speed = natural.wind.length();
        let wind = natural.wind.normalize_or(Vec2::X) * mix(speed, self.wind);
        Weather {
            cover: mix(natural.cover, self.cover),
            convection: mix(natural.convection, self.convection),
            cirrus: mix(natural.cirrus, self.cirrus),
            humidity: mix(natural.humidity, self.humidity),
            precipitation: mix(natural.precipitation, self.precipitation),
            snow: mix(natural.snow, self.snow),
            wind,
        }
    }
}

/// `weather.ron`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeatherFile {
    #[serde(default)]
    presets: Vec<Preset>,
}

/// Every package's kinds of weather.
#[derive(Debug, Default)]
pub struct WeatherTable {
    presets: Vec<Preset>,
    by_name: FxHashMap<String, usize>,
}

impl WeatherTable {
    pub fn from_ron(namespace: &str, source: &str) -> Result<Self, RegistryError> {
        let mut table = Self::default();
        table.extend(namespace, source)?;
        Ok(table)
    }

    /// Adds a package's weather. Unqualified names are placed in `namespace`; a short
    /// name finds the first package's preset of that name, by package name order.
    pub fn extend(&mut self, namespace: &str, source: &str) -> Result<(), RegistryError> {
        // Optional values are written plainly (`snow: 1.0`), not as `Some(1.0)`.
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: WeatherFile =
            options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/weather.ron: {e}")))?;
        for mut preset in file.presets {
            if !preset.name.contains(':') {
                preset.name = format!("{namespace}:{}", preset.name);
            }
            let values = [preset.cover, preset.convection, preset.cirrus, preset.humidity, preset.precipitation, preset.snow];
            if values.into_iter().flatten().any(|v| !(0.0..=1.0).contains(&v)) || preset.wind.is_some_and(|w| !(0.0..=40.0).contains(&w)) {
                return Err(RegistryError(format!("weather `{}` has a value out of range", preset.name)));
            }
            let index = self.presets.len();
            if self.by_name.insert(preset.name.clone(), index).is_some() {
                return Err(RegistryError(format!("duplicate weather `{}`", preset.name)));
            }
            self.by_name.entry(preset.short_name().to_owned()).or_insert(index);
            self.presets.push(preset);
        }
        Ok(())
    }

    /// A preset by name, with or without its namespace.
    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.by_name.get(name).map(|&i| &self.presets[i])
    }

    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_blend_over_the_natural_weather() {
        let table = WeatherTable::from_ron("base", r#"(presets: [(name: "rain", cover: 1.0, precipitation: 0.5, wind: 6.0)])"#).unwrap();
        let rain = table.preset("rain").unwrap();
        assert_eq!(table.preset("base:rain"), Some(rain));
        let natural = Weather { cover: 0.2, snow: 1.0, wind: Vec2::new(0.0, -2.0), ..Default::default() };
        assert_eq!(rain.blend(&natural, 0.0), natural);
        let forced = rain.blend(&natural, 1.0);
        assert_eq!((forced.cover, forced.precipitation, forced.snow), (1.0, 0.5, 1.0), "snow stays natural");
        assert_eq!(forced.wind, Vec2::new(0.0, -6.0));
        assert!((rain.blend(&natural, 0.5).cover - 0.6).abs() < 1e-6);
        assert!(WeatherTable::from_ron("base", r#"(presets: [(name: "bad", cover: 2.0)])"#).is_err());
    }
}
