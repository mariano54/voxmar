//! When the world is saved: `saves.ron` in any content package.

use serde::Deserialize;
use voxmar_engine::content::ContentFile;
use voxmar_protocol::TICKS_PER_SECOND;

/// The file the save timing is defined in, in any content package.
pub const FILE: &str = "saves.ron";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavesFile {
    every: Option<f32>,
    after_leaving: Option<f32>,
    slow_ms: Option<f32>,
}

/// When the world is saved, in ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SaveRules {
    /// Between saves while anyone plays.
    pub every: u64,
    /// The least time after one save before a player leaving brings another.
    pub after_leaving: u64,
    /// A save taking longer than this is logged as slow.
    pub slow_ms: f32,
}

impl Default for SaveRules {
    fn default() -> Self {
        Self { every: 30 * TICKS_PER_SECOND as u64, after_leaving: 5 * TICKS_PER_SECOND as u64, slow_ms: 50.0 }
    }
}

impl SaveRules {
    /// Reads every package's `saves.ron`, in package order; a later package's numbers
    /// replace an earlier one's.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut rules = Self::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let at = |e: &dyn std::fmt::Display| format!("{}/{FILE}: {e}", file.package);
            let def: SavesFile = ron::Options::default()
                .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
                .from_str(&file.source)
                .map_err(|e| at(&e))?;
            // Seconds to ticks: at least one, and no more than a day, so a package can't make
            // the server save every step or never.
            let ticks = |name: &str, seconds: f32| -> Result<u64, String> {
                if !(seconds.is_finite() && seconds * TICKS_PER_SECOND as f32 >= 1.0 && seconds <= 86_400.0) {
                    return Err(at(&format!("`{name}` must be from one tick to a day, not {seconds} seconds")));
                }
                Ok((seconds * TICKS_PER_SECOND as f32).round() as u64)
            };
            if let Some(every) = def.every {
                rules.every = ticks("every", every)?;
            }
            if let Some(after) = def.after_leaving {
                rules.after_leaving = ticks("after_leaving", after)?;
            }
            if let Some(slow) = def.slow_ms {
                if !(slow.is_finite() && slow > 0.0) {
                    return Err(at(&format!("`slow_ms` must be positive, not {slow}")));
                }
                rules.slow_ms = slow;
            }
        }
        Ok(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    #[test]
    fn a_later_package_replaces_the_numbers_and_nonsense_is_refused() {
        let rules = SaveRules::load(&[file("core", "(every: 30.0, after_leaving: 5.0, slow_ms: 50.0)"), file("mod", "(after_leaving: 2.0)")]).unwrap();
        assert_eq!(rules, SaveRules { every: 30 * TICKS_PER_SECOND as u64, after_leaving: 2 * TICKS_PER_SECOND as u64, slow_ms: 50.0 });
        for bad in ["(every: 0.0)", "(every: -1.0)", "(after_leaving: 1e9)", "(slow_ms: 0.0)", "(every: inf)", "(often: 1.0)"] {
            assert!(SaveRules::load(&[file("mod", bad)]).is_err(), "{bad}");
        }
    }
}
