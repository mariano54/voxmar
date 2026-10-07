//! The numbers survival runs on, from the content's `survival.ron` (one package sets it):
//! a new player's health and kit, falls, lava and the void, and how fires warm and water
//! chills. Checked at startup (`load_content`), so a broken file fails there.

use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::content::ContentFile;
use voxmar_engine::inventory::MAX_STACK;

const FILE: &str = "survival.ron";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SurvivalRules {
    pub max_health: u8,
    pub starter_kit: Vec<(String, u8)>,
    /// What a new player starts with in the pack's last slots, last first.
    #[serde(default)]
    pub pack_end: Vec<(String, u8)>,
    pub safe_fall: f32,
    pub burn: u8,
    /// Seconds.
    pub burn_every: f32,
    pub void_below: f32,
    pub heat_emission: u8,
    pub heat: f32,
    pub heat_reach: i32,
    pub water_chill: f32,
    pub roof_scan: i32,
}

impl SurvivalRules {
    pub(crate) fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut found = files.iter().filter(|f| f.name == FILE);
        let file = found.next().ok_or_else(|| format!("no package sets survival ({FILE})"))?;
        if let Some(other) = found.next() {
            return Err(format!("{}/{FILE}: {} already sets survival", other.package, file.package));
        }
        let at = |e: &dyn std::fmt::Display| format!("{}/{FILE}: {e}", file.package);
        let rules: SurvivalRules = ron::from_str(&file.source).map_err(|e| at(&e))?;
        let checks = [
            (rules.max_health > 0, "max_health must be above 0"),
            (rules.safe_fall.is_finite() && rules.safe_fall >= 0.0, "safe_fall must be 0 or more"),
            (rules.burn_every.is_finite() && rules.burn_every > 0.0, "burn_every must be above 0"),
            (rules.void_below.is_finite(), "void_below must be a number"),
            (rules.heat_emission <= 15, "heat_emission is a light level, 0 to 15"),
            (rules.heat.is_finite() && rules.heat >= 0.0, "heat must be 0 or more"),
            ((0..=16).contains(&rules.heat_reach), "heat_reach must be 0 to 16 blocks"),
            (rules.water_chill.is_finite(), "water_chill must be a number"),
            ((1..=256).contains(&rules.roof_scan), "roof_scan must be 1 to 256 blocks"),
            (rules.starter_kit.iter().chain(&rules.pack_end).all(|&(_, count)| (1..=MAX_STACK).contains(&count)), "a starter kit count must be 1 to 64"),
            (rules.pack_end.len() <= 9, "pack_end holds at most 9 items"),
        ];
        match checks.into_iter().find(|(ok, _)| !ok) {
            Some((_, problem)) => Err(at(&problem)),
            None => Ok(rules),
        }
    }

    /// The starter kit's items that exist.
    pub(crate) fn kit(&self, blocks: &BlockRegistry) -> Vec<(BlockId, u8)> {
        existing(&self.starter_kit, blocks)
    }

    /// The items for the pack's last slots that exist, last slot first.
    pub(crate) fn pack_end(&self, blocks: &BlockRegistry) -> Vec<(BlockId, u8)> {
        existing(&self.pack_end, blocks)
    }

    /// Ticks between lava's burns.
    pub(crate) fn burn_ticks(&self) -> u64 {
        ((self.burn_every * voxmar_protocol::TICKS_PER_SECOND as f32).round() as u64).max(1)
    }
}

fn existing(items: &[(String, u8)], blocks: &BlockRegistry) -> Vec<(BlockId, u8)> {
    items.iter().filter_map(|(name, count)| blocks.id(name).map(|id| (id, *count))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(source: &str) -> Vec<ContentFile> {
        vec![ContentFile { package: "core".into(), name: FILE.into(), source: source.into() }]
    }

    #[test]
    fn the_shipped_rules_load_and_bad_ones_are_refused() {
        let shipped = include_str!("../../../assets/content/core/survival.ron");
        let rules = SurvivalRules::load(&file(shipped)).unwrap();
        assert_eq!((rules.max_health, rules.burn_ticks()), (20, 10));
        assert!(SurvivalRules::load(&file(&shipped.replace("burn_every: 0.5", "burn_every: 0.0"))).unwrap_err().contains("burn_every"));
        assert!(SurvivalRules::load(&file(&shipped.replace("max_health: 20", "max_health: 0"))).is_err());
        assert!(SurvivalRules::load(&[]).is_err(), "some package must set survival");
        let twice = [file(shipped), vec![ContentFile { package: "mod".into(), name: FILE.into(), source: shipped.into() }]].concat();
        assert!(SurvivalRules::load(&twice).is_err());
    }
}
