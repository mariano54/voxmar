//! The numbers survival runs on, from the content's `survival.ron`: a new player's health
//! and kit, falls, lava and the void, and how fires warm and water chills. A package sets
//! any of them; where two set the same one, the later package's wins, so a mod changes
//! what it names and keeps the rest. Checked at startup (`load_content`), so a broken file
//! fails there.

use serde::Deserialize;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::content::{ContentFile, Laid};
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory, ItemStack, SLOTS, max_stack};

const FILE: &str = "survival.ron";
/// The tag of a wall a draught comes through (a lattice window, an arrow slit): always a
/// draughty one, and what a draughty home's whistle can listen for.
pub(crate) const DRAUGHT: &str = "draught";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SurvivalRules {
    pub max_health: u8,
    /// What a new player starts with, and what goes in the pack's last slots (last first,
    /// out of the hotbar's way) before it.
    pub starter_kit: Vec<(BlockId, u8)>,
    pub pack_end: Vec<(BlockId, u8)>,
    /// How many new players from one address get the starting kit in any `kit_window`
    /// seconds; the rest start with `pack_end` only.
    pub kits_per_address: u32,
    pub kit_window: f32,
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
    /// Rain at least this heavy (precipitation 0..1) puts out a light that `rain_douses`
    /// with open sky over it (a torch).
    pub douse_rain: f32,
    /// A light that `gutters_to:` something does so in this last share of its burn (0..1).
    pub gutter_share: f32,
    /// A light that is `tended` (flame.rs) set down in a settlement, no more than this many
    /// blocks above or below the settlement's height, is tended there and never burns out.
    pub tend_height: i32,
    /// A home reaches this many blocks out on each side, this many up and this many down:
    /// the box its furnishings are counted in and a player must stand in to feel it.
    pub home_reach: i32,
    pub home_up: i32,
    pub home_down: i32,
    /// Tags of what stands in a room rather than walls it in (a chest, a barrel, a table, a
    /// station): the survey of a home's walls looks past such a block to the wall behind it, so furnishing a room never makes it draughty.
    pub home_furniture: Vec<String>,
    /// The most the home's meal effects together (effects.ron's `meal_heal`, `meal_time`,
    /// which multiply) make of a meal's heal and of how long its good lasts.
    pub meal_most: f32,
    /// No sleeping with a creature that hunts players this many blocks off, or this many
    /// seconds after being hurt.
    pub sleep_threat_reach: f32,
    pub sleep_after_hurt: f32,
    /// The most health a second brings back by itself, however many heal buffs and prayers
    /// are on: eating stays the way to heal in a hurry.
    pub heal_most: f32,
}

/// One package's `survival.ron`: whichever numbers it sets.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SurvivalFile {
    max_health: Option<u8>,
    starter_kit: Option<Vec<(String, u8)>>,
    pack_end: Option<Vec<(String, u8)>>,
    kits_per_address: Option<u32>,
    kit_window: Option<f32>,
    safe_fall: Option<f32>,
    burn: Option<u8>,
    burn_every: Option<f32>,
    void_below: Option<f32>,
    heat_emission: Option<u8>,
    heat: Option<f32>,
    heat_reach: Option<i32>,
    water_chill: Option<f32>,
    roof_scan: Option<i32>,
    douse_rain: Option<f32>,
    gutter_share: Option<f32>,
    tend_height: Option<i32>,
    home_reach: Option<i32>,
    home_up: Option<i32>,
    home_down: Option<i32>,
    home_furniture: Option<Vec<String>>,
    meal_most: Option<f32>,
    sleep_threat_reach: Option<f32>,
    sleep_after_hurt: Option<f32>,
    heal_most: Option<f32>,
}

impl SurvivalRules {
    /// The rules the packages' files set, a later package's number replacing an earlier
    /// one's; `notes` gets one line for each package that does, naming both.
    pub(crate) fn load(files: &[ContentFile], blocks: &BlockRegistry, notes: &mut Vec<String>) -> Result<Self, String> {
        let mut merged = SurvivalFile::default();
        let mut laid = Laid::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
            let next: SurvivalFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            let mut set = Vec::new();
            macro_rules! later_wins {
                ($($field:ident),*) => { $(if next.$field.is_some() { merged.$field = next.$field; set.push(stringify!($field)); })* };
            }
            later_wins!(max_health, starter_kit, pack_end, kits_per_address, kit_window, safe_fall, burn, burn_every, void_below, heat_emission, heat, heat_reach, water_chill, roof_scan, douse_rain, gutter_share, tend_height, home_reach, home_up, home_down, home_furniture, meal_most, sleep_threat_reach, sleep_after_hurt, heal_most);
            laid.set(&file.package, FILE, "survival rules", set, notes);
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
            kits_per_address: set!(kits_per_address),
            kit_window: set!(kit_window),
            safe_fall: set!(safe_fall),
            burn: set!(burn),
            burn_every: set!(burn_every),
            void_below: set!(void_below),
            heat_emission: set!(heat_emission),
            heat: set!(heat),
            heat_reach: set!(heat_reach),
            water_chill: set!(water_chill),
            roof_scan: set!(roof_scan),
            douse_rain: set!(douse_rain),
            gutter_share: set!(gutter_share),
            tend_height: set!(tend_height),
            home_reach: set!(home_reach),
            home_up: set!(home_up),
            home_down: set!(home_down),
            home_furniture: set!(home_furniture),
            meal_most: set!(meal_most),
            sleep_threat_reach: set!(sleep_threat_reach),
            sleep_after_hurt: set!(sleep_after_hurt),
            heal_most: set!(heal_most),
        };
        let checks = [
            (rules.max_health > 0, "max_health must be above 0"),
            (rules.safe_fall.is_finite() && rules.safe_fall >= 0.0, "safe_fall must be 0 or more"),
            (rules.burn_every.is_finite() && rules.burn_every > 0.0, "burn_every must be above 0"),
            (rules.kit_window.is_finite() && (0.0..=31_536_000.0).contains(&rules.kit_window), "kit_window must be 0 to a year of seconds"),
            (rules.void_below.is_finite(), "void_below must be a number"),
            (rules.heat_emission <= 15, "heat_emission is a light level, 0 to 15"),
            (rules.heat.is_finite() && rules.heat >= 0.0, "heat must be 0 or more"),
            ((0..=16).contains(&rules.heat_reach), "heat_reach must be 0 to 16 blocks"),
            (rules.water_chill.is_finite(), "water_chill must be a number"),
            ((1..=256).contains(&rules.roof_scan), "roof_scan must be 1 to 256 blocks"),
            (rules.douse_rain.is_finite() && rules.douse_rain > 0.0 && rules.douse_rain <= 1.0, "douse_rain must be above 0, up to 1"),
            (rules.gutter_share.is_finite() && (0.0..1.0).contains(&rules.gutter_share), "gutter_share must be 0 or more, below 1"),
            ((0..=64).contains(&rules.tend_height), "tend_height must be 0 to 64 blocks"),
            ((1..=16).contains(&rules.home_reach), "home_reach must be 1 to 16 blocks"),
            ((0..=16).contains(&rules.home_up) && (0..=16).contains(&rules.home_down), "home_up and home_down must be 0 to 16 blocks"),
            (!rules.home_furniture.iter().any(|tag| tag.starts_with("shelter:")), "home_furniture names a shelter tag: a wall's, not furniture's"),
            (rules.meal_most.is_finite() && (1.0..=10.0).contains(&rules.meal_most), "meal_most must be 1 to 10"),
            (rules.sleep_threat_reach.is_finite() && (0.0..=64.0).contains(&rules.sleep_threat_reach), "sleep_threat_reach must be 0 to 64 blocks"),
            (rules.sleep_after_hurt.is_finite() && (0.0..=600.0).contains(&rules.sleep_after_hurt), "sleep_after_hurt must be 0 to 600 seconds"),
            (rules.heal_most.is_finite() && rules.heal_most > 0.0, "heal_most must be above 0"),
            (rules.pack_end.len() <= SLOTS - HOTBAR_SLOTS, "pack_end holds at most as many items as the pack has slots besides the hotbar"),
            (rules.new_inventory(true).is_some(), "pack_end and starter_kit together do not fit in an inventory"),
        ];
        match checks.into_iter().find(|(ok, _)| !ok) {
            Some((_, problem)) => Err(format!("{FILE}: {problem}")),
            None => Ok(rules),
        }
    }

    /// Whether a block of `tags` is furniture to a home's walls: one of `home_furniture`'s
    /// tags and no shelter tag. A shelter tag always wins: cloth hung for a wall, a hay bale
    /// or a stone column walls a room whatever else it is.
    pub(crate) fn furniture(&self, tags: &[String]) -> bool {
        tags.iter().any(|tag| self.home_furniture.contains(tag)) && !tags.iter().any(|tag| tag.starts_with("shelter:"))
    }

    /// Every block that walls a room says how well it shelters it (`HomeWalls`), once: each
    /// solid block tagged `building` or `roof`, full or shaped (roofs, slabs, stairs), but
    /// furniture (`home_furniture`), which the survey looks past. No block says it twice.
    pub(crate) fn check_shelters(&self, blocks: &BlockRegistry) -> Result<(), String> {
        use voxmar_engine::effect::Shelter;
        let unjudged: Vec<String> = blocks
            .ids()
            .map(|id| blocks.def(id))
            .filter(|def| {
                let shelters = Shelter::ALL.iter().filter(|s| def.has_tag(s.tag())).count();
                let furniture = self.furniture(&def.tags);
                let walls = def.solid && (def.has_tag("building") || def.has_tag("roof")) && !furniture;
                shelters > 1 || (shelters == 0 && walls)
            })
            .map(|def| def.name.clone())
            .collect();
        if !unjudged.is_empty() {
            return Err(format!("blocks: each solid `building` or `roof` block that is not furniture ({FILE}'s home_furniture) carries exactly one of `shelter:draughty`, `shelter:sound` or `shelter:fine`, and no block more than one; these do not: {}", unjudged.join(", ")));
        }
        // A misspelt shelter tag judges nothing; a draught comes only through a draughty wall.
        let misjudged: Vec<String> = blocks
            .ids()
            .map(|id| blocks.def(id))
            .filter_map(|def| {
                let unknown = def.tags.iter().find(|tag| tag.starts_with("shelter:") && !Shelter::ALL.iter().any(|s| s.tag() == tag.as_str()));
                match unknown {
                    Some(tag) => Some(format!("{} (`{tag}` is no shelter)", def.name)),
                    None => (def.has_tag(DRAUGHT) && !def.has_tag(Shelter::Draughty.tag())).then(|| format!("{} (`{DRAUGHT}` but not `shelter:draughty`)", def.name)),
                }
            })
            .collect();
        match misjudged.is_empty() {
            true => Ok(()),
            false => Err(format!("blocks: a shelter tag is `shelter:draughty`, `shelter:sound` or `shelter:fine`, and a wall a draught comes through (`{DRAUGHT}`) is draughty; these are not: {}", misjudged.join(", "))),
        }
    }

    /// A new player's inventory: `pack_end` in the pack's last slots, then the starter kit
    /// if `kit`. `None` if they do not fit (refused at load, so never once the server runs).
    pub(crate) fn new_inventory(&self, kit: bool) -> Option<Inventory> {
        let mut inventory = Inventory::default();
        for (slot, &(item, count)) in (0..SLOTS).rev().zip(&self.pack_end) {
            inventory.slots[slot] = Some(ItemStack::new(item, count));
        }
        for &(item, count) in self.starter_kit.iter().filter(|_| kit) {
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
        let load = |source: &str| SurvivalRules::load(&[file("core", source)], &blocks, &mut Vec::new());
        let rules = load(SHIPPED).unwrap();
        assert_eq!((rules.max_health, rules.burn_ticks()), (20, 10));
        assert!(load(&SHIPPED.replace("burn_every: 0.5", "burn_every: 0.0")).unwrap_err().contains("burn_every"));
        assert!(load(&SHIPPED.replace("max_health: 20", "max_health: 0")).is_err());
        assert!(SurvivalRules::load(&[], &blocks, &mut Vec::new()).unwrap_err().contains("max_health"), "some package must set survival");
        assert!(load(&SHIPPED.replace("safe_fall: 3.0,", "")).unwrap_err().contains("safe_fall"));
        assert!(load(&SHIPPED.replace("safe_fall", "safe_falls")).is_err(), "a misspelt field is refused");
        assert_eq!(rules.meal_most, 2.0);
        assert!(load(&SHIPPED.replace("meal_most: 2.0", "meal_most: 0.5")).unwrap_err().contains("meal_most"), "a ceiling under 1 would make meals worse at home");
    }

    #[test]
    fn a_new_player_starts_with_the_purse_alone_and_a_kit_a_mod_gives_fills_the_hotbar_first() {
        let blocks = blocks();
        let rules = SurvivalRules::load(&[file("core", SHIPPED)], &blocks, &mut Vec::new()).unwrap();
        let id = |name: &str| blocks.id(name).unwrap();
        let inventory = rules.new_inventory(true).unwrap();
        assert_eq!(inventory.slots.iter().flatten().collect::<Vec<_>>(), [&ItemStack::new(id("base:coin_purse"), 1)], "nothing but the purse");
        assert_eq!(inventory.slots[SLOTS - 1].map(|s| s.item), Some(id("base:coin_purse")), "the purse is in the last slot");
        let kit = file("mod", r#"(starter_kit: [("base:wooden_pickaxe", 1), ("base:wooden_axe", 1), ("base:bread", 4), ("base:torch", 8), ("base:oak_planks", 16)])"#);
        let rules = SurvivalRules::load(&[file("core", SHIPPED), kit], &blocks, &mut Vec::new()).unwrap();
        let inventory = rules.new_inventory(true).unwrap();
        let carried = |item| inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>();
        assert_eq!([carried(id("base:bread")), carried(id("base:torch")), carried(id("base:oak_planks"))], [4, 8, 16]);
        assert_eq!(inventory.slots[SLOTS - 1], Some(ItemStack::new(id("base:coin_purse"), 1)), "the purse is in the last slot");
        assert_eq!(inventory.slots[..2].iter().flatten().map(|s| s.item).collect::<Vec<_>>(), [id("base:wooden_pickaxe"), id("base:wooden_axe")], "the tools come first, in the hotbar");
        assert_eq!(inventory.slots.iter().flatten().count(), 9, "a slot for each tool and loaf, the torches, the planks and the purse");
        assert_eq!(rules.new_inventory(false).unwrap().slots.iter().flatten().count(), 1, "no kit: the purse alone");
    }

    #[test]
    fn a_later_package_changes_only_what_it_names_and_unknown_items_are_refused() {
        let blocks = blocks();
        let later = file("mod", r#"(max_health: 30, starter_kit: [("base:torch", 64)])"#);
        let mut notes = Vec::new();
        let rules = SurvivalRules::load(&[file("core", SHIPPED), later], &blocks, &mut notes).unwrap();
        assert_eq!((rules.max_health, rules.safe_fall), (30, 3.0));
        assert_eq!(notes, ["mod/survival.ron replaces the survival rules core set (max_health, starter_kit)"]);
        assert_eq!(rules.starter_kit, [(blocks.id("base:torch").unwrap(), 64)]);
        assert_eq!(rules.pack_end.len(), 1, "the purse is kept");
        let unknown = file("mod", r#"(starter_kit: [("base:diamond", 1)])"#);
        assert!(SurvivalRules::load(&[file("core", SHIPPED), unknown], &blocks, &mut Vec::new()).unwrap_err().contains("base:diamond"));
        let too_many = file("mod", r#"(pack_end: [("base:bread", 2)])"#);
        assert!(SurvivalRules::load(&[file("core", SHIPPED), too_many], &blocks, &mut Vec::new()).unwrap_err().contains("1 to 1"), "bread takes a slot each");
        // Thirty-six full stacks and the purse do not fit.
        let crowded = file("mod", &format!("(starter_kit: [{}])", vec![r#"("base:torch", 64)"#; 36].join(", ")));
        assert!(SurvivalRules::load(&[file("core", SHIPPED), crowded], &blocks, &mut Vec::new()).unwrap_err().contains("fit"));
    }
}
