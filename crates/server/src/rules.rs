//! The numbers survival runs on, from the content's `survival.ron`: a new player's health
//! and kit, falls, lava and the void, and how fires warm and water chills. A package sets
//! any of them; where two set the same one, the later package's wins, so a mod changes
//! what it names and keeps the rest. Checked at startup (`load_content`), so a broken file
//! fails there.

use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::content::ContentFile;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory, ItemStack, SLOTS, max_stack};

const FILE: &str = "survival.ron";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SurvivalRules {
    pub max_health: u8,
    /// What a new player starts with, and what goes in the pack's last slots (last first,
    /// out of the hotbar's way) before it.
    pub starter_kit: Vec<(BlockId, u8)>,
    pub pack_end: Vec<(BlockId, u8)>,
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

/// One package's `survival.ron`: whichever numbers it sets.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SurvivalFile {
    max_health: Option<u8>,
    starter_kit: Option<Vec<(String, u8)>>,
    pack_end: Option<Vec<(String, u8)>>,
    safe_fall: Option<f32>,
    burn: Option<u8>,
    burn_every: Option<f32>,
    void_below: Option<f32>,
    heat_emission: Option<u8>,
    heat: Option<f32>,
    heat_reach: Option<i32>,
    water_chill: Option<f32>,
    roof_scan: Option<i32>,
}

impl SurvivalRules {
    pub(crate) fn load(files: &[ContentFile], blocks: &BlockRegistry) -> Result<Self, String> {
        let mut merged = SurvivalFile::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
            let next: SurvivalFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            macro_rules! later_wins {
                ($($field:ident),*) => { $(if next.$field.is_some() { merged.$field = next.$field; })* };
            }
            later_wins!(max_health, starter_kit, pack_end, safe_fall, burn, burn_every, void_below, heat_emission, heat, heat_reach, water_chill, roof_scan);
        }
        macro_rules! set {
            ($field:ident) => {
                merged.$field.ok_or_else(|| format!("no package sets survival's {} ({FILE})", stringify!($field)))?
            };
        }
        // The kit is added as picking items up would, a stack at a time; `pack_end` goes
        // straight into slots, so each of its counts must be one stack.
        let items = |list: Vec<(String, u8)>, what: &str| -> Result<Vec<(BlockId, u8)>, String> {
            list.into_iter()
                .map(|(name, count)| {
                    let item = blocks.id(&name).ok_or_else(|| format!("{FILE}: {what} names {name}, which no package defines"))?;
                    let most = if what == "pack_end" { max_stack(item) } else { u8::MAX };
                    if count == 0 || count > most {
                        return Err(format!("{FILE}: {what} gives {count} {name}; a stack of it holds 1 to {most}"));
                    }
                    Ok((item, count))
                })
                .collect()
        };
        let rules = SurvivalRules {
            max_health: set!(max_health),
            starter_kit: items(set!(starter_kit), "starter_kit")?,
            pack_end: items(merged.pack_end.take().unwrap_or_default(), "pack_end")?,
            safe_fall: set!(safe_fall),
            burn: set!(burn),
            burn_every: set!(burn_every),
            void_below: set!(void_below),
            heat_emission: set!(heat_emission),
            heat: set!(heat),
            heat_reach: set!(heat_reach),
            water_chill: set!(water_chill),
            roof_scan: set!(roof_scan),
        };
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
            (rules.pack_end.len() <= SLOTS - HOTBAR_SLOTS, "pack_end holds at most as many items as the pack has slots besides the hotbar"),
            (rules.new_inventory().is_some(), "pack_end and starter_kit together do not fit in an inventory"),
        ];
        match checks.into_iter().find(|(ok, _)| !ok) {
            Some((_, problem)) => Err(format!("{FILE}: {problem}")),
            None => Ok(rules),
        }
    }

    /// A new player's inventory: `pack_end` in the pack's last slots, then the starter kit.
    /// `None` if they do not fit (refused at load, so never once the server runs).
    pub(crate) fn new_inventory(&self) -> Option<Inventory> {
        let mut inventory = Inventory::default();
        for (slot, &(item, count)) in (0..SLOTS).rev().zip(&self.pack_end) {
            inventory.slots[slot] = Some(ItemStack { item, count });
        }
        for &(item, count) in &self.starter_kit {
            if inventory.add(item, count) > 0 {
                return None;
            }
        }
        Some(inventory)
    }

    /// Ticks between lava's burns.
    pub(crate) fn burn_ticks(&self) -> u64 {
        ((self.burn_every * voxmar_protocol::TICKS_PER_SECOND as f32).round() as u64).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped content's items, whose stack sizes `max_stack` knows once it loads.
    fn blocks() -> std::sync::Arc<BlockRegistry> {
        voxmar_engine::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap().blocks
    }

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    const SHIPPED: &str = include_str!("../../../assets/content/core/survival.ron");

    #[test]
    fn the_shipped_rules_load_and_bad_ones_are_refused() {
        let blocks = blocks();
        let load = |source: &str| SurvivalRules::load(&[file("core", source)], &blocks);
        let rules = load(SHIPPED).unwrap();
        assert_eq!((rules.max_health, rules.burn_ticks()), (20, 10));
        assert!(load(&SHIPPED.replace("burn_every: 0.5", "burn_every: 0.0")).unwrap_err().contains("burn_every"));
        assert!(load(&SHIPPED.replace("max_health: 20", "max_health: 0")).is_err());
        assert!(SurvivalRules::load(&[], &blocks).unwrap_err().contains("max_health"), "some package must set survival");
        assert!(load(&SHIPPED.replace("safe_fall: 3.0,", "")).unwrap_err().contains("safe_fall"));
        assert!(load(&SHIPPED.replace("safe_fall", "safe_falls")).is_err(), "a misspelt field is refused");
    }

    #[test]
    fn a_new_player_gets_tools_bread_torches_planks_and_the_purse() {
        let blocks = blocks();
        let rules = SurvivalRules::load(&[file("core", SHIPPED)], &blocks).unwrap();
        let inventory = rules.new_inventory().unwrap();
        let id = |name: &str| blocks.id(name).unwrap();
        let carried = |item| inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
        assert_eq!([carried(id("base:bread")), carried(id("base:torch")), carried(id("base:oak_planks"))], [4, 8, 16]);
        assert_eq!(inventory.slots[SLOTS - 1], Some(ItemStack { item: id("base:coin_purse"), count: 1 }), "the purse is in the last slot");
        assert_eq!(inventory.slots[..2].iter().flatten().map(|s| s.item).collect::<Vec<_>>(), [id("base:wooden_pickaxe"), id("base:wooden_axe")], "the tools come first, in the hotbar");
        assert_eq!(inventory.slots.iter().flatten().count(), 9, "a slot for each tool and loaf, the torches, the planks and the purse");
    }

    #[test]
    fn a_later_package_changes_only_what_it_names_and_unknown_items_are_refused() {
        let blocks = blocks();
        let later = file("mod", r#"(max_health: 30, starter_kit: [("base:torch", 64)])"#);
        let rules = SurvivalRules::load(&[file("core", SHIPPED), later], &blocks).unwrap();
        assert_eq!((rules.max_health, rules.safe_fall), (30, 3.0));
        assert_eq!(rules.starter_kit, [(blocks.id("base:torch").unwrap(), 64)]);
        assert_eq!(rules.pack_end.len(), 1, "the purse is kept");
        let unknown = file("mod", r#"(starter_kit: [("base:diamond", 1)])"#);
        assert!(SurvivalRules::load(&[file("core", SHIPPED), unknown], &blocks).unwrap_err().contains("base:diamond"));
        let too_many = file("mod", r#"(pack_end: [("base:bread", 2)])"#);
        assert!(SurvivalRules::load(&[file("core", SHIPPED), too_many], &blocks).unwrap_err().contains("1 to 1"), "bread takes a slot each");
        // Thirty-six full stacks and the purse do not fit.
        let crowded = file("mod", &format!("(starter_kit: [{}])", vec![r#"("base:torch", 64)"#; 36].join(", ")));
        assert!(SurvivalRules::load(&[file("core", SHIPPED), crowded], &blocks).unwrap_err().contains("fit"));
    }
}
