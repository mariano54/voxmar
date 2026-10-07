//! Content packages: everything the game knows about, loaded as data.
//!
//! Each directory under `content/` is a package whose name is its namespace. First-party
//! content (`core`, `base`) loads exactly like a mod would.
//!
//! The engine reads blocks and actions itself. A package's other `.ron` files (the world
//! generator's biomes, trees and plants) are kept as text for the crates that understand
//! them, so a mod adds a biome the same way it adds a block.

use std::path::Path;
use std::sync::Arc;

use crate::action::ActionRegistry;
use crate::block::{BlockRegistry, RegistryError};
use crate::command::CommandRegistry;
use crate::cooking::CookingRegistry;
use crate::creature::{CreatureRegistry, DefenceTraining, Limits, Rules};
use crate::craft::RecipeRegistry;
use crate::crop::CropRegistry;
use crate::effect::EffectRegistry;
use crate::fishing::FishingRegistry;
use crate::lang::{ENGLISH, Lang};
use crate::movement::Tuning;
use crate::skill::SkillRegistry;
use crate::tool::ToolRegistry;
use crate::weather::WeatherTable;

#[derive(Clone)]
pub struct Content {
    pub blocks: Arc<BlockRegistry>,
    pub actions: Arc<ActionRegistry>,
    pub commands: Arc<CommandRegistry>,
    pub weather: Arc<WeatherTable>,
    /// How players move.
    pub movement: Arc<Tuning>,
    pub skills: Arc<SkillRegistry>,
    pub crops: Arc<CropRegistry>,
    pub creatures: Arc<CreatureRegistry>,
    /// What stations cook.
    pub cooking: Arc<CookingRegistry>,
    /// What the inventory's crafting grid makes.
    pub recipes: Arc<RecipeRegistry>,
    /// Buffs and debuffs.
    pub effects: Arc<EffectRegistry>,
    /// What tools do to breaking blocks, and what breaking them takes and teaches.
    pub tools: Arc<ToolRegistry>,
    /// Weapons, their classes and styles (`combat`).
    pub combat: Arc<crate::combat::CombatRegistry>,
    /// What bites where.
    pub fishing: Arc<FishingRegistry>,
    /// Structures recognised from patterns of blocks (the Homestead Hearth).
    pub multiblocks: Arc<crate::multiblock::MultiblockRegistry>,
    /// The quest book.
    pub quests: Arc<crate::quest::QuestRegistry>,
    /// What it takes to study and master the journal's entries.
    pub journal: Arc<crate::journal::JournalRules>,
    /// Which wild plants come back where they were picked, and how soon.
    pub regrowth: Arc<crate::regrowth::Regrowth>,
    /// Every other definition file, by package then file name.
    pub files: Arc<Vec<ContentFile>>,
    /// The packages' words in English (`lang/en.ron`), for the server's log and console.
    pub english: Arc<Lang>,
    /// What loading noticed and allowed, for the log: a later package replacing data an
    /// earlier one set (creature limits, Defence training, the level curve).
    pub notes: Arc<Vec<String>>,
}

/// Single-valued data a package may set field by field (Design draft 91): a later
/// package's fields replace an earlier one's, and the load log names both packages, so a
/// mod tunes one number without forking base. [`settable!`] declares one.
pub trait Settable: Copy {
    /// Every field as an option: what one package sets.
    type Set;
    /// These values with `set` over them, and the names of the fields it set.
    fn with(self, set: &Self::Set) -> (Self, Vec<&'static str>);
}

/// Declares `$set` for a [`Settable`] `$whole`: its fields as options, as a package's file
/// reads them, and `with`, which lays them over the values so far.
macro_rules! settable {
    ($(#[$meta:meta])* $set:ident for $whole:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub struct $set {
            $(pub $field: Option<$ty>,)*
        }

        impl $crate::content::Settable for $whole {
            type Set = $set;

            fn with(mut self, set: &$set) -> (Self, Vec<&'static str>) {
                let mut changed = Vec::new();
                $(
                    if let Some(value) = set.$field {
                        self.$field = value;
                        changed.push(stringify!($field));
                    }
                )*
                (self, changed)
            }
        }
    };
}
pub(crate) use settable;

/// Single-valued data as the packages so far set it, and the last package that did.
#[derive(Default)]
struct Setting<T> {
    value: T,
    by: Option<String>,
}

impl<T: Settable> Setting<T> {
    /// Lays what `package`'s `file` sets of `what` over the value, noting when that
    /// replaces an earlier package's.
    fn set(&mut self, package: &str, file: &str, what: &str, set: Option<T::Set>, notes: &mut Vec<String>) {
        let Some(set) = set else { return };
        let changed;
        (self.value, changed) = self.value.with(&set);
        if let Some(earlier) = self.by.replace(package.to_owned()) {
            let note = format!("{package}/{file} replaces the {what} {earlier} set ({})", changed.join(", "));
            tracing::info!("{note}");
            notes.push(note);
        }
    }
}

/// A definition file the engine leaves to another crate.
#[derive(Clone, Debug)]
pub struct ContentFile {
    /// The package, which is also the namespace of the names it defines.
    pub package: String,
    /// The file name, such as `biomes.ron`.
    pub name: String,
    pub source: String,
}

impl Content {
    /// Loads every package in `dir`, in name order so ids are the same everywhere.
    pub fn load(dir: &Path) -> Result<Self, RegistryError> {
        let read_error = |path: &Path, e: std::io::Error| RegistryError(format!("{}: {e}", path.display()));
        let mut packages: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| read_error(dir, e))?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        packages.sort();

        let mut blocks = Vec::new();
        let mut actions = ActionRegistry::default();
        let mut commands = CommandRegistry::default();
        let mut weather = WeatherTable::default();
        let mut movement = None;
        let mut skills = SkillRegistry::default();
        let mut crops = Vec::new();
        let mut creatures = Vec::new();
        let mut creature_limits = Setting::<Limits>::default();
        let mut defence = Setting::<DefenceTraining>::default();
        let mut fishing_rules = Setting::<crate::fishing::FishingRules>::default();
        let mut notes = Vec::new();
        let mut cooking = Vec::new();
        let mut recipes = Vec::new();
        let mut effects = Vec::new();
        let mut tools = Vec::new();
        let mut combat = Vec::new();
        let mut stacking = Vec::new();
        let mut fishing = Vec::new();
        let mut regrowth = Vec::new();
        let mut journal = Vec::new();
        let mut multiblocks = Vec::new();
        let mut quests = Vec::new();
        let mut files = Vec::new();
        for package in &packages {
            let path = dir.join(package);
            let mut names: Vec<String> = std::fs::read_dir(&path)
                .map_err(|e| read_error(&path, e))?
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.ends_with(".ron"))
                .collect();
            names.sort();
            for name in names {
                let file = path.join(&name);
                let source = std::fs::read_to_string(&file).map_err(|e| read_error(&file, e))?;
                match name.as_str() {
                    // `blocks.ron`, and `blocks_<topic>.ron` for families of blocks a system
                    // owns (what creatures drop), each read the same way.
                    name if name.starts_with("blocks") => blocks.push((package.clone(), BlockRegistry::read(package, &source)?)),
                    "actions.ron" => actions.extend(package, &source)?,
                    "commands.ron" => commands.extend(package, &source)?,
                    "weather.ron" => weather.extend(package, &source)?,
                    // A later package's curve replaces an earlier one's (Design draft 91).
                    "skills.ron" => {
                        if let Some(note) = skills.extend(package, &source)? {
                            tracing::info!("{note}");
                            notes.push(note);
                        }
                    }
                    "crops.ron" => {
                        crops.extend(CropRegistry::read(package, &source)?);
                        // Kept with the other files too, so the fingerprint covers it.
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "cooking.ron" => {
                        cooking.extend(CookingRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "creatures.ron" => {
                        let read = CreatureRegistry::read(package, &source)?;
                        creatures.extend(read.defs);
                        creature_limits.set(package, &name, "creature limits", read.limits, &mut notes);
                        defence.set(package, &name, "Defence training", read.defence, &mut notes);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    // `recipes.ron`, and `recipes_<topic>.ron` for what a system makes (weapons).
                    topic if topic.starts_with("recipes") => {
                        recipes.extend(RecipeRegistry::read(package, &source)?);
                        // Kept with the other files too, so the fingerprint covers it.
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "fishing.ron" => {
                        let file = FishingRegistry::read(package, &source)?;
                        fishing_rules.set(package, &name, "fishing rules", file.rules(), &mut notes);
                        fishing.push(file);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "quests.ron" => {
                        quests.push(crate::quest::QuestRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "multiblocks.ron" => {
                        multiblocks.push(crate::multiblock::MultiblockRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "journal.ron" => {
                        journal.push(crate::journal::JournalRules::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "regrowth.ron" => {
                        regrowth.push(crate::regrowth::Regrowth::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "stacking.ron" => {
                        stacking.push(source.clone());
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "tools.ron" => {
                        tools.push(ToolRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "combat.ron" => {
                        combat.push(crate::combat::CombatRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "effects.ron" => {
                        effects.push(EffectRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "movement.ron" => {
                        if movement.replace(Tuning::from_ron(&source)?).is_some() {
                            return Err(RegistryError(format!("{package}/movement.ron: another package already sets movement")));
                        }
                        // Kept with the other files too, so the fingerprint covers it.
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    _ => files.push(ContentFile { package: package.clone(), name, source }),
                }
            }
        }
        let blocks = BlockRegistry::expand(blocks)?;
        weather.validate()?;
        if weather.try_tuning().is_none() {
            return Err(RegistryError("no package's weather.ron sets its `tuning`".to_owned()));
        }
        let movement = movement.ok_or_else(|| RegistryError("no package sets movement (movement.ron)".to_owned()))?;
        skills.validate()?;
        let blocks = BlockRegistry::new(blocks)?;
        let crops = CropRegistry::new(crops, &blocks, &skills)?;
        let rules = Rules { limits: creature_limits.value, defence: defence.value };
        let creatures = CreatureRegistry::new(creatures, rules, &blocks, &skills)?;
        let cooking = CookingRegistry::new(cooking, &blocks, &skills)?;
        let recipes = RecipeRegistry::new(recipes, &blocks, &skills)?;
        let effects = EffectRegistry::new(effects)?;
        let tools = ToolRegistry::new(tools, &blocks, &skills)?;
        let combat = crate::combat::CombatRegistry::new(combat, &blocks, &skills)?;
        let fishing = FishingRegistry::new(fishing, fishing_rules.value, &blocks, &skills)?;
        let regrowth = crate::regrowth::Regrowth::new(regrowth, &blocks)?;
        let multiblocks = crate::multiblock::MultiblockRegistry::new(multiblocks, &blocks)?;
        crate::inventory::set_stack_limits(stack_limits(&blocks, &stacking)?);
        crate::inventory::set_pouches(pouches(&blocks)?);
        // A furnishing gives itself back only where a player placed it: one that drops
        // nothing anyway has no business with the tag.
        if let Some(id) = blocks.ids().find(|&id| blocks.def(id).has_tag("furnishing") && (blocks.info(id).drops.is_air() || blocks.info(id).drop_count == 0)) {
            return Err(RegistryError(format!(
                "block `{}` is a `furnishing` but drops nothing: give it a drop, or remove the tag (village props that drop nothing don't need it)",
                blocks.def(id).name
            )));
        }
        Ok(Self {
            blocks: Arc::new(blocks),
            actions: Arc::new(actions),
            commands: Arc::new(commands),
            weather: Arc::new(weather),
            movement: Arc::new(movement),
            skills: Arc::new(skills),
            crops: Arc::new(crops),
            creatures: Arc::new(creatures),
            cooking: Arc::new(cooking),
            recipes: Arc::new(recipes),
            effects: Arc::new(effects),
            tools: Arc::new(tools),
            combat: Arc::new(combat),
            fishing: Arc::new(fishing),
            regrowth: Arc::new(regrowth),
            multiblocks: Arc::new(multiblocks),
            quests: Arc::new(crate::quest::QuestRegistry::new(quests)?),
            journal: Arc::new(crate::journal::JournalRules::merge(journal)),
            files: Arc::new(files),
            english: Arc::new(Lang::load(dir, ENGLISH)),
            notes: Arc::new(notes),
        })
    }

    /// Fingerprint used to check that client and server load the same content.
    pub fn fingerprint(&self) -> u64 {
        let mut files = 0xcbf2_9ce4_8422_2325u64;
        for file in self.files.iter() {
            for b in file.package.bytes().chain([0]).chain(file.name.bytes()).chain([0]).chain(file.source.bytes()) {
                files = (files ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        self.blocks.fingerprint() ^ self.actions.fingerprint().rotate_left(17) ^ files.rotate_left(34) ^ self.skills.fingerprint().rotate_left(51)
    }
}

/// One package's `stacking.ron`: tags whose items take a slot each.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Stacking {
    single: Vec<String>,
}

/// Each item's most per stack: an item's own `stack:N` tag, else one for an item with a
/// tag some package's `stacking.ron` lists as single, else a full stack.
fn stack_limits(blocks: &BlockRegistry, files: &[String]) -> Result<Vec<u8>, RegistryError> {
    let mut single = Vec::new();
    for source in files {
        let file: Stacking = ron::from_str(source).map_err(|e| RegistryError(format!("stacking.ron: {e}")))?;
        single.extend(file.single);
    }
    Ok(blocks
        .ids()
        .map(|id| {
            let tags = &blocks.def(id).tags;
            let own = tags.iter().find_map(|t| t.strip_prefix("stack:")?.parse::<u8>().ok()).filter(|&n| n > 0);
            own.unwrap_or(if tags.iter().any(|t| single.contains(t)) { 1 } else { crate::inventory::MAX_STACK }).min(crate::inventory::MAX_STACK)
        })
        .collect())
}

/// Each item's pouch: the first item tagged `pouch` and `holds:TAG` for a tag it has, with
/// how many of a kind it holds (its `holds_each:N`, else `inventory::POUCH_EACH`). A
/// `holds_each` that is not a whole number above 0 is a mistake in the content.
fn pouches(blocks: &BlockRegistry) -> Result<Vec<Option<(crate::block::BlockId, u16)>>, RegistryError> {
    let mut holders: Vec<(crate::block::BlockId, String, u16)> = Vec::new();
    for id in blocks.ids().filter(|&id| blocks.def(id).has_tag("pouch")) {
        let def = blocks.def(id);
        let each = match def.tags.iter().find_map(|t| t.strip_prefix("holds_each:")) {
            None => crate::inventory::POUCH_EACH,
            Some(n) => n.parse::<u16>().ok().filter(|&n| n > 0).ok_or_else(|| RegistryError(format!("pouch `{}`: `holds_each:{n}` is not a count above 0", def.name)))?,
        };
        holders.extend(def.tags.iter().filter_map(|t| Some((id, t.strip_prefix("holds:")?.to_owned(), each))));
    }
    Ok(blocks.ids().map(|id| holders.iter().find(|(_, tag, _)| blocks.def(id).has_tag(tag)).map(|&(pouch, _, each)| (pouch, each))).collect())
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::skill::SkillId;

    /// Skills no content trains: the combat skills train by any fight, and Ranged, Magic
    /// and Alchemy wait for their systems (Combat's slices, brewing).
    const UNTRAINED: [&str; 6] = ["core:attack", "core:defence", "core:hitpoints", "core:ranged", "core:magic", "core:alchemy"];

    /// Every skill something trains has a way in at level 1: one whose only source asks
    /// for level 5 never leaves level 1 (Construction, with only the altar, did: design
    /// draft 94). And the skills nothing trains are exactly `UNTRAINED`.
    #[test]
    fn every_trained_skill_is_trained_from_level_one() {
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let skill = |name: &str| content.skills.id(name);
        let mut lowest: std::collections::BTreeMap<SkillId, (u8, String)> = Default::default();
        let mut source = |skill: Option<SkillId>, level: u8, xp: f32, what: String| {
            if let Some(skill) = skill.filter(|_| xp > 0.0) {
                let entry = lowest.entry(skill).or_insert((u8::MAX, String::new()));
                if level.max(1) < entry.0 {
                    *entry = (level.max(1), what);
                }
            }
        };
        let name = |id| content.blocks.def(id).name.clone();
        for recipe in content.recipes.iter() {
            source(recipe.skill, recipe.level, recipe.xp, format!("crafting {}", name(recipe.output)));
        }
        for (_, recipe) in content.cooking.iter() {
            source(recipe.skill, recipe.def.level, recipe.def.xp, format!("cooking {}", name(recipe.output)));
        }
        for id in content.blocks.ids() {
            if let Some(rule) = content.tools.rule(id) {
                source(rule.skill, rule.level, rule.xp, format!("breaking {}", name(id)));
            }
        }
        for (_, crop) in content.crops.iter() {
            let xp = crop.def.xp;
            source(Some(crop.skill), crop.def.level, xp.plant + xp.harvest + xp.forage, format!("growing {}", crop.def.name));
        }
        for catch in content.fishing.iter() {
            source(content.fishing.skill, catch.def.level, catch.def.xp, format!("catching {}", catch.def.item));
        }
        for (_, creature) in content.creatures.iter() {
            let def = &creature.def;
            source(creature.xp_skill, 1, def.xp.kill, format!("killing {}", def.name));
            if let Some(taming) = &def.taming {
                source(skill("core:hunter"), taming.level, taming.xp, format!("taming {}", def.name));
            }
            if let Some(breeding) = &def.breeding {
                source(skill("core:farming"), breeding.level, breeding.xp, format!("breeding {}", def.name));
            }
            for product in &def.products {
                source(skill("core:farming"), 1, product.xp, format!("{}'s {}", def.name, product.item));
            }
        }
        let planter = &content.regrowth.rules;
        source(skill("core:farming"), planter.planter_level.common as u8, planter.planter_xp.common, "a herb planter's common plants".into());
        let late: Vec<String> = lowest
            .iter()
            .filter(|(_, (level, _))| *level > 1)
            .map(|(&id, (level, what))| format!("{} (first {what}, at {level})", content.skills.get(id).name))
            .collect();
        assert!(late.is_empty(), "these skills are trained only from above level 1: {late:?}");
        let untrained: Vec<&str> = content.skills.ids().filter(|id| !lowest.contains_key(id)).map(|id| content.skills.get(id).name.as_str()).collect();
        assert_eq!(untrained, UNTRAINED, "the skills nothing trains");
    }

    /// What finishing each main-line quest makes that its tasks don't name: the hearth's
    /// pieces, the kilns' and what goes into them.
    const ALSO_MADE: &[(&str, &[&str])] = &[
        ("base:stone_in_hand", &["base:crafting_table"]),
        ("base:homestead_hearth", &["base:hearthstone", "base:chimney"]),
        ("base:the_potter", &["base:pit_kiln", "base:fired_clay"]),
        ("base:red_metal", &["base:copper_ingot"]),
        ("base:crucible_kiln", &["base:kiln_bricks", "base:crucible"]),
        ("base:first_bronze", &["base:bronze_blend"]),
    ];

    /// How a thing on the main line is made: at the crafting grid (a quest's Craft), by
    /// cooking (its Cook), or either (`ALSO_MADE`'s pieces, what a quest asks to Have).
    #[derive(Clone, Copy, PartialEq)]
    enum Way {
        Grid,
        Cooked,
        Either,
    }

    /// One route through the main line so far: the experience it has earned, what it has
    /// made, the ores it has mined and used up, and the walls it met.
    #[derive(Clone)]
    struct Walk<'a> {
        content: &'a Content,
        quest: String,
        xp: crate::skill::Skills,
        made: FxHashMap<crate::block::BlockId, u32>,
        mined: FxHashMap<crate::block::BlockId, u32>,
        used: FxHashMap<crate::block::BlockId, u32>,
        walls: Vec<String>,
    }

    impl Walk<'_> {
        fn item(&self, name: &str) -> crate::block::BlockId {
            let blocks = &self.content.blocks;
            blocks.id(name).or_else(|| blocks.id(&format!("base:{name}"))).unwrap_or_else(|| panic!("{}: no item `{name}`", self.quest))
        }

        /// A wall unless the line has reached `level` in `skill` by now.
        fn need(&mut self, skill: SkillId, level: u8, what: &str) {
            let skills = &self.content.skills;
            if skills.level(self.xp.xp(skill)) < level {
                self.walls.push(format!("{}: {what} needs {} {level}, the line gives {} xp", self.quest, skills.get(skill).name, self.xp.xp(skill) / 10));
            }
        }

        /// Mined, in all, at least `total` of `ore`: what more that takes is mined now.
        fn mine(&mut self, ore: crate::block::BlockId, total: u32, what: &str) {
            let had = self.mined.get(&ore).copied().unwrap_or(0);
            let Some(rule) = self.content.tools.rule(ore).filter(|_| total > had) else { return };
            self.mined.insert(ore, total);
            let Some(skill) = rule.skill else { return };
            self.need(skill, rule.level, what);
            self.xp.grant(skill, (rule.xp * 10.0).round() as u32 * (total - had));
        }

        /// Made, in all, at least `n` of `name`, `way`: the easiest recipe of that way, and
        /// first the ores its batches use up (where nothing but a mined block will do).
        fn make(&mut self, name: &str, n: u32, way: Way) {
            let content = self.content;
            let id = self.item(name);
            let grid = || content.recipes.iter().filter(|r| r.output == id).min_by_key(|r| r.level).map(|r| {
                let cells: Vec<Vec<crate::block::BlockId>> = r.alternatives().into_iter().flatten().map(|a| a.to_vec()).collect();
                (r.skill, r.level, r.xp, r.count as u32, cells)
            });
            let cooked = || {
                let r = content.cooking.iter().map(|(_, r)| r).filter(|r| r.output == id).min_by_key(|r| r.def.level)?;
                let inputs = content.cooking.iter().filter(|(_, other)| std::ptr::eq(*other, r)).map(|(input, _)| input).collect();
                Some((r.skill, r.def.level, r.def.xp, 1, vec![inputs]))
            };
            let found = match way {
                Way::Grid => grid(),
                Way::Cooked => cooked(),
                Way::Either => grid().or_else(cooked),
            };
            let Some((skill, level, xp, count, cells)) = found else { panic!("{}: no recipe makes `{name}` that way", self.quest) };
            let had = self.made.get(&id).copied().unwrap_or(0);
            let batches = n.saturating_sub(had).div_ceil(count);
            if batches == 0 {
                return;
            }
            self.made.insert(id, had + batches * count);
            for accepts in cells {
                let mined: Option<Vec<_>> = accepts.iter().map(|&b| content.tools.rule(b).filter(|r| r.skill.is_some()).map(|r| (b, r.level))).collect();
                if let Some((ore, _)) = mined.and_then(|ores| ores.into_iter().min_by_key(|&(_, level)| level)) {
                    let used = self.used.entry(ore).or_default();
                    *used += batches;
                    let total = *used;
                    self.mine(ore, total, &format!("{name} using {}", content.blocks.def(ore).name));
                }
            }
            if let Some(skill) = skill {
                self.need(skill, level, name);
                self.xp.grant(skill, (xp * 10.0).round() as u32 * batches);
            }
        }

        /// The least of two routes: what both are sure to have earned, made and mined.
        fn least(self, other: Self) -> Self {
            let both = |a: FxHashMap<crate::block::BlockId, u32>, b: &FxHashMap<crate::block::BlockId, u32>| -> FxHashMap<_, _> {
                a.into_iter().map(|(k, v)| (k, v.min(b.get(&k).copied().unwrap_or(0)))).collect()
            };
            let mut xp = crate::skill::Skills::default();
            for id in self.content.skills.ids() {
                xp.grant(id, self.xp.xp(id).min(other.xp.xp(id)));
            }
            Walk { xp, made: both(self.made, &other.made), mined: both(self.mined, &other.mined), used: both(self.used, &other.used), ..self }
        }
    }

    /// The main quest line, walked in order, reaches every level it asks for from what it
    /// guarantees: quest rewards and the things it has the player make (each quest's own
    /// crafts, cooks and ores, the pieces in `ALSO_MADE`, and the ores those recipes use
    /// up, mined first). Where a quest has several routes, only what every route gives
    /// counts. A level asked for before the walk has earned it is a wall: grinding
    /// throwaway crafts to go on (design drafts 100 and 102 found the chimney, Engineering 5
    /// for copper and 10 for the crucible, and the tin in bronze at Mining 10).
    #[test]
    fn the_main_quest_line_earns_every_level_it_asks_for() {
        use crate::quest::{Goal, Reward};
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let skills = &content.skills;
        let skill_named = |name: &str| skills.id(name).or_else(|| skills.id(&format!("core:{name}")));
        // The line and every quest it comes after, in file order once their `after` is done.
        let quests: Vec<&crate::quest::QuestDef> = content.quests.iter().collect();
        let mut wanted: Vec<String> = quests.iter().filter(|q| q.branch == "main").map(|q| q.name.clone()).collect();
        let mut i = 0;
        while i < wanted.len() {
            let quest = quests.iter().find(|q| q.name == wanted[i]).expect("a quest named in `after`");
            for after in &quest.after {
                assert!(!after.contains('|'), "{}: the walk doesn't take one of several quests (`{after}`) yet", quest.name);
                if !wanted.contains(after) {
                    wanted.push(after.clone());
                }
            }
            i += 1;
        }
        for (quest, _) in ALSO_MADE {
            assert!(wanted.iter().any(|w| w == quest), "ALSO_MADE names {quest}, which the main line doesn't walk");
        }
        let mut walk = Walk { content: &content, quest: String::new(), xp: Default::default(), made: Default::default(), mined: Default::default(), used: Default::default(), walls: Vec::new() };
        skills.start(&mut walk.xp);
        let mut done: Vec<String> = Vec::new();
        while done.len() < wanted.len() {
            let ready = |q: &&&crate::quest::QuestDef| !done.contains(&q.name) && wanted.contains(&q.name) && q.after.iter().all(|a| done.contains(a));
            let Some(quest) = quests.iter().find(ready) else { panic!("the main line can't go on after {done:?}") };
            walk.quest = quest.name.clone();
            let also: Vec<&str> = ALSO_MADE.iter().filter(|(q, _)| *q == quest.name).flat_map(|(_, items)| items.iter().copied()).collect();
            // Each route on its own; then only what every open one is sure to give.
            let mut open: Option<Walk> = None;
            let mut first_walls = None;
            for task in &quest.tasks {
                let mut route = walk.clone();
                for name in &also {
                    route.make(name, 1, Way::Either);
                }
                for goal in task {
                    match goal {
                        Goal::Craft(item, _) | Goal::Cook(item, _) if item == "any" => {}
                        Goal::Craft(item, _) | Goal::Cook(item, _) | Goal::Have(item, _) if item.starts_with("tag:") => panic!("{}: the walk doesn't model {goal:?} yet", quest.name),
                        Goal::Craft(item, n) => route.make(item, *n, Way::Grid),
                        Goal::Cook(item, n) => route.make(item, *n, Way::Cooked),
                        Goal::Have(item, n) => route.make(item, *n, Way::Either),
                        Goal::Mine(ore, n) => {
                            let ore = route.item(ore);
                            let what = format!("mining {}", content.blocks.def(ore).name);
                            route.mine(ore, (*n).max(route.mined.get(&ore).copied().unwrap_or(0)), &what);
                        }
                        Goal::Level(skill, n) => {
                            let skill = skill_named(skill).unwrap_or_else(|| panic!("{}: no skill `{skill}`", quest.name));
                            route.need(skill, *n, "its own goal");
                        }
                        Goal::Home | Goal::Visit(_) | Goal::Built(..) => {}
                        Goal::Catch(..) | Goal::Pick(..) | Goal::Tame(..) | Goal::Kill(..) | Goal::Discover(..) => panic!("{}: the walk doesn't model {goal:?} yet", quest.name),
                    }
                }
                if route.walls.len() == walk.walls.len() {
                    open = Some(match open {
                        None => route,
                        Some(other) => other.least(route),
                    });
                } else if first_walls.is_none() {
                    first_walls = Some(route.walls);
                }
            }
            // No route open: report the first's walls and carry on as if it were done.
            walk = match open {
                Some(route) => route,
                None => Walk { walls: first_walls.unwrap_or_default(), ..walk },
            };
            for reward in &quest.rewards {
                if let Reward::Xp(name, amount) = reward {
                    let skill = skill_named(name).unwrap_or_else(|| panic!("{}: no skill `{name}`", quest.name));
                    walk.xp.grant(skill, (amount * 10.0).round() as u32);
                }
            }
            done.push(quest.name.clone());
        }
        assert!(walk.walls.is_empty(), "the main quest line asks for levels it hasn't earned: {:#?}", walk.walls);
    }
}
