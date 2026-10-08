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
use crate::creature::condition::ConditionRules;
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
    /// Machines, what they burn and what they make (`machines.ron`, `processing.ron`).
    pub machines: Arc<crate::machine::MachineRegistry>,
    /// The quest book.
    pub quests: Arc<crate::quest::QuestRegistry>,
    /// What it takes to study and master the journal's entries.
    pub journal: Arc<crate::journal::JournalRules>,
    /// Which wild plants come back where they were picked, and how soon.
    pub regrowth: Arc<crate::regrowth::Regrowth>,
    /// Locked chests, the lockpicks that open them and what they hold.
    pub treasure: Arc<crate::treasure::Treasure>,
    /// What things are worth (`value`).
    pub value: Arc<crate::value::Value>,
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
/// mod tunes one number without forking base. [`settable!`] declares one. Worldgen's
/// `layered!` (crates/worldgen/src/content.rs) is the same rule for climate, land and trail
/// data, with nested structs and named lists; the two should become one.
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
pub(crate) struct Setting<T> {
    pub(crate) value: T,
    by: Option<String>,
}

impl<T: Settable> Setting<T> {
    /// Lays what `package`'s `file` sets of `what` over the value, noting when that
    /// replaces an earlier package's.
    pub(crate) fn set(&mut self, package: &str, file: &str, what: &str, set: Option<T::Set>, notes: &mut Vec<String>) {
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
        let mut condition = Setting::<ConditionRules>::default();
        let mut fishing_rules = Setting::<crate::fishing::FishingRules>::default();
        let mut notes = Vec::new();
        let mut cooking = Vec::new();
        let mut recipes = Vec::new();
        let mut effects = Vec::new();
        let mut tools = Vec::new();
        let mut combat = Vec::new();
        let mut value = Vec::new();
        let mut stacking = Vec::new();
        let mut fishing = Vec::new();
        let mut regrowth = Vec::new();
        let mut treasure = Vec::new();
        let mut journal = Vec::new();
        let mut multiblocks = Vec::new();
        let mut machines = Vec::new();
        let mut processes = Vec::new();
        let mut quests = Vec::new();
        let mut shapes = Vec::new();
        let mut shape_limits = Setting::<crate::shape::ShapeLimits>::default();
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
                    "shapes.ron" => {
                        let file = crate::shape::ShapeFile::read(package, &source)?;
                        shape_limits.set(package, &name, "shape limits", file.limits, &mut notes);
                        shapes.push((package.clone(), file));
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
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
                    // `cooking.ron`, and `cooking_<topic>.ron` for what a system smelts (machines).
                    topic if topic.starts_with("cooking") => {
                        cooking.extend(CookingRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "creatures.ron" => {
                        let read = CreatureRegistry::read(package, &source)?;
                        creatures.extend(read.defs);
                        creature_limits.set(package, &name, "creature limits", read.limits, &mut notes);
                        defence.set(package, &name, "Defence training", read.defence, &mut notes);
                        condition.set(package, &name, "condition rules", read.condition, &mut notes);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    // `recipes.ron`, and `recipes_<topic>.ron` for what a system makes (weapons, machines).
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
                    // `multiblocks.ron`, and `multiblocks_<topic>.ron` for a system's own (machines).
                    topic if topic.starts_with("multiblocks") => {
                        multiblocks.push(crate::multiblock::MultiblockRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "machines.ron" => {
                        machines.push(crate::machine::MachineRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "processing.ron" => {
                        processes.push(crate::machine::MachineRegistry::read_processes(package, &source)?);
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
                    "treasure.ron" => {
                        treasure.push((package.clone(), crate::treasure::Treasure::read(package, &source)?));
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
                    "value.ron" => {
                        value.push(crate::value::Value::read(package, &source)?);
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
        let blocks = BlockRegistry::with_shapes(blocks, &crate::shape::Shapes::gather(shapes, shape_limits.value)?)?;
        let crops = CropRegistry::new(crops, &blocks, &skills)?;
        let rules = Rules { limits: creature_limits.value, defence: defence.value, condition: condition.value };
        let creatures = CreatureRegistry::new(creatures, rules, &blocks, &skills)?;
        let cooking = CookingRegistry::new(cooking, &blocks, &skills)?;
        let recipes = RecipeRegistry::new(recipes, &blocks, &skills)?;
        let effects = EffectRegistry::new(effects)?;
        let tools = ToolRegistry::new(tools, &blocks, &skills)?;
        let combat = crate::combat::CombatRegistry::new(combat, &blocks, &skills)?;
        let fishing = FishingRegistry::new(fishing, fishing_rules.value, &blocks, &skills)?;
        let regrowth = crate::regrowth::Regrowth::new(regrowth, &blocks)?;
        let multiblocks = crate::multiblock::MultiblockRegistry::new(multiblocks, &blocks)?;
        let machines = crate::machine::MachineRegistry::new(machines, processes, &blocks)?;
        if let Some(unknown) = machines.walkers().iter().find(|w| creatures.id(w).is_none()) {
            return Err(RegistryError(format!("machines.ron: no creature `{unknown}` to walk a treadwheel")));
        }
        let value = crate::value::Value::merge(value);
        let limits = stack_limits(&blocks, &stacking)?;
        let treasure = crate::treasure::Treasure::new(treasure, &blocks, &value, &limits, &mut notes)?;
        // An opened chest becomes a bin with room for all it can give.
        for (block, chest) in treasure.chests() {
            let room = machines.get(chest.opens_to).map_or(0, |m| m.bin_slots());
            let most = treasure.most_stacks(chest);
            if room < most {
                return Err(RegistryError(format!("treasure.ron: `{}` opens to a bin of {room} stacks, but its table gives up to {most}", blocks.def(block).name)));
            }
        }
        crate::inventory::set_stack_limits(limits);
        crate::inventory::set_pouches(pouches(&blocks)?);
        crate::inventory::set_worn_slots(combat.worn_slots());
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
            treasure: Arc::new(treasure),
            value: Arc::new(value),
            multiblocks: Arc::new(multiblocks),
            machines: Arc::new(machines),
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
    use crate::block::BlockId;
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

    /// How a thing on the main line is made: at the crafting grid (a quest's Craft), by
    /// cooking (its Cook), or either (what goes into something else, a multiblock's pieces,
    /// what a quest asks to Have).
    #[derive(Clone, Copy, PartialEq)]
    enum Way {
        Grid,
        Cooked,
        Either,
    }

    /// A recipe as the walk uses it: its skill, level and experience a batch, how many a
    /// batch makes, what each of its cells takes, and the station it needs (the crafting
    /// table for a big grid, a cooking station's tag).
    struct Batch {
        skill: Option<SkillId>,
        level: u8,
        xp: f32,
        count: u32,
        cells: Vec<Vec<BlockId>>,
        station: Option<String>,
    }

    /// One route through the main line so far: the experience it has earned, what it has
    /// made in all (what Craft and Cook goals count) and has in hand (what gets used up),
    /// the ores it has mined and used up, the stations it has put down and the multiblocks
    /// it has built, and the walls it met.
    #[derive(Clone)]
    struct Walk<'a> {
        content: &'a Content,
        quest: String,
        xp: crate::skill::Skills,
        made: FxHashMap<BlockId, u32>,
        stock: FxHashMap<BlockId, u32>,
        mined: FxHashMap<BlockId, u32>,
        used: FxHashMap<BlockId, u32>,
        stations: Vec<String>,
        built: Vec<String>,
        walls: Vec<String>,
        depth: u32,
    }

    impl Walk<'_> {
        fn item(&self, name: &str) -> BlockId {
            let blocks = &self.content.blocks;
            blocks.id(name).or_else(|| blocks.id(&format!("base:{name}"))).unwrap_or_else(|| panic!("{}: no item `{name}`", self.quest))
        }

        fn name(&self, id: BlockId) -> String {
            self.content.blocks.def(id).name.clone()
        }

        /// A wall unless the line has reached `level` in `skill` by now.
        fn need(&mut self, skill: SkillId, level: u8, what: &str) {
            let skills = &self.content.skills;
            if skills.level(self.xp.xp(skill)) < level {
                self.walls.push(format!("{}: {what} needs {} {level}, the line gives {} xp", self.quest, skills.get(skill).name, self.xp.xp(skill) / 10));
            }
        }

        /// Mined, in all, at least `total` of `ore`: what more that takes is mined now.
        fn mine(&mut self, ore: BlockId, total: u32, what: &str) {
            let had = self.mined.get(&ore).copied().unwrap_or(0);
            let Some(rule) = self.content.tools.rule(ore).filter(|_| total > had) else { return };
            self.mined.insert(ore, total);
            let Some(skill) = rule.skill else { return };
            self.need(skill, rule.level, what);
            self.xp.grant(skill, (rule.xp * 10.0).round() as u32 * (total - had));
        }

        /// The easiest recipe making `id` the given way, if any.
        fn recipe(&self, id: BlockId, way: Way) -> Option<Batch> {
            let content = self.content;
            let grid = || {
                content.recipes.iter().filter(|r| r.output == id).min_by_key(|r| r.level).map(|r| Batch {
                    skill: r.skill,
                    level: r.level,
                    xp: r.xp,
                    count: r.count as u32,
                    cells: r.alternatives().into_iter().flatten().map(|a| a.to_vec()).collect(),
                    station: r.needs_table().then(|| "crafting_table".to_owned()),
                })
            };
            let cooked = || {
                let r = content.cooking.iter().map(|(_, r)| r).filter(|r| r.output == id).min_by_key(|r| r.def.level)?;
                let inputs = content.cooking.iter().filter(|(_, other)| std::ptr::eq(*other, r)).map(|(input, _)| input).collect();
                Some(Batch { skill: r.skill, level: r.def.level, xp: r.def.xp, count: 1, cells: vec![inputs], station: Some(r.def.station.clone()) })
            };
            match way {
                Way::Grid => grid(),
                Way::Cooked => cooked(),
                Way::Either => grid().or_else(cooked),
            }
        }

        /// Makes `k` more of `id`, `way`: the easiest recipe, at its station, from what its
        /// cells take (used up from what is in hand, or made, mined or gathered first).
        fn produce(&mut self, id: BlockId, k: u32, way: Way) {
            let name = self.name(id);
            let Some(batch) = self.recipe(id, way) else { panic!("{}: no recipe makes `{name}` that way", self.quest) };
            let batches = k.div_ceil(batch.count);
            if batches == 0 {
                return;
            }
            self.depth += 1;
            assert!(self.depth < 24, "{}: making `{name}` goes round in a loop of recipes", self.quest);
            if let Some(station) = &batch.station {
                self.station(station);
            }
            for accepts in &batch.cells {
                self.consume(accepts, batches, &name);
            }
            if let Some(skill) = batch.skill {
                self.need(skill, batch.level, &name);
                self.xp.grant(skill, (batch.xp * 10.0).round() as u32 * batches);
            }
            *self.made.entry(id).or_default() += batches * batch.count;
            *self.stock.entry(id).or_default() += batches * batch.count;
            self.depth -= 1;
        }

        /// Uses up `k` of what a cell takes, for `what`: a block only mining gives is mined
        /// (the easiest of them); anything else a player gathers is gathered; otherwise the
        /// easiest made one comes from what is in hand, the rest made now.
        fn consume(&mut self, accepts: &[BlockId], k: u32, what: &str) {
            let content = self.content;
            let mined: Option<Vec<(BlockId, u8)>> = accepts.iter().map(|&b| content.tools.rule(b).filter(|r| r.skill.is_some()).map(|r| (b, r.level))).collect();
            if let Some((ore, _)) = mined.and_then(|ores| ores.into_iter().min_by_key(|&(_, level)| level)) {
                let used = self.used.entry(ore).or_default();
                *used += k;
                let total = *used;
                self.mine(ore, total, &format!("{what} using {}", self.name(ore)));
                return;
            }
            let made: Vec<(BlockId, u8)> = accepts.iter().filter_map(|&b| self.recipe(b, Way::Either).map(|r| (b, r.level))).collect();
            if made.len() < accepts.len() {
                return;
            }
            let Some(&(item, _)) = made.iter().min_by_key(|&&(_, level)| level) else { return };
            let have = self.stock.get(&item).copied().unwrap_or(0);
            let short = k.saturating_sub(have);
            self.produce(item, short, Way::Either);
            *self.stock.entry(item).or_default() -= k;
        }

        /// Puts down a station with `tag` once (the easiest block with it that is made), if
        /// any block with it is made at all.
        fn station(&mut self, tag: &str) {
            if self.stations.iter().any(|s| s == tag) {
                return;
            }
            self.stations.push(tag.to_owned());
            let blocks = &self.content.blocks;
            let made: Vec<BlockId> = blocks.tagged(tag).filter(|&b| self.recipe(b, Way::Either).is_some()).collect();
            if !made.is_empty() {
                self.consume(&made, 1, tag);
            }
        }

        /// Builds the multiblock `name` once: every piece its cells take.
        fn build(&mut self, name: &str) {
            if self.built.iter().any(|b| b == name) {
                return;
            }
            self.built.push(name.to_owned());
            let content = self.content;
            let multiblock = content.multiblocks.get(name).unwrap_or_else(|| panic!("{}: no multiblock `{name}`", self.quest));
            for (accepts, n) in multiblock.pieces() {
                self.consume(accepts, n, name);
            }
        }

        /// The least of two routes: what both are sure to have earned, made, held, mined
        /// and put down.
        fn least(self, other: Self) -> Self {
            let both = |a: FxHashMap<BlockId, u32>, b: &FxHashMap<BlockId, u32>| -> FxHashMap<_, _> {
                a.into_iter().map(|(k, v)| (k, v.min(b.get(&k).copied().unwrap_or(0)))).collect()
            };
            let mut xp = crate::skill::Skills::default();
            for id in self.content.skills.ids() {
                xp.grant(id, self.xp.xp(id).min(other.xp.xp(id)));
            }
            let common = |a: Vec<String>, b: &[String]| a.into_iter().filter(|x| b.contains(x)).collect();
            Walk {
                xp,
                made: both(self.made, &other.made),
                stock: both(self.stock, &other.stock),
                mined: both(self.mined, &other.mined),
                used: both(self.used, &other.used),
                stations: common(self.stations, &other.stations),
                built: common(self.built, &other.built),
                ..self
            }
        }
    }

    /// The main quest line, walked in order, reaches every level it asks for from what it
    /// guarantees: quest rewards and the things it has the player make, all read from the
    /// content (each quest's crafts, cooks and ores; the pieces of the multiblocks it
    /// builds, its home among them; what each recipe uses up, made, mined or gathered
    /// first; and the stations they need, the crafting table and kilns among them). Where
    /// a quest has several routes, only what every route gives counts. A level asked for before the walk has earned it is a wall: grinding
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
        let mut walk = Walk {
            content: &content,
            quest: String::new(),
            xp: Default::default(),
            made: Default::default(),
            stock: Default::default(),
            mined: Default::default(),
            used: Default::default(),
            stations: Vec::new(),
            built: Vec::new(),
            walls: Vec::new(),
            depth: 0,
        };
        skills.start(&mut walk.xp);
        let mut done: Vec<String> = Vec::new();
        while done.len() < wanted.len() {
            let ready = |q: &&&crate::quest::QuestDef| !done.contains(&q.name) && wanted.contains(&q.name) && q.after.iter().all(|a| done.contains(a));
            let Some(quest) = quests.iter().find(ready) else { panic!("the main line can't go on after {done:?}") };
            walk.quest = quest.name.clone();
            // Each route on its own; then only what every open one is sure to give.
            let mut open: Option<Walk> = None;
            let mut first_walls = None;
            for task in &quest.tasks {
                let mut route = walk.clone();
                for goal in task {
                    match goal {
                        Goal::Craft(item, _) | Goal::Cook(item, _) if item == "any" => {}
                        Goal::Craft(item, _) | Goal::Cook(item, _) | Goal::Have(item, _) if item.starts_with("tag:") => panic!("{}: the walk doesn't model {goal:?} yet", quest.name),
                        Goal::Craft(item, n) | Goal::Cook(item, n) => {
                            let id = route.item(item);
                            let short = n.saturating_sub(route.made.get(&id).copied().unwrap_or(0));
                            route.produce(id, short, if matches!(goal, Goal::Craft(..)) { Way::Grid } else { Way::Cooked });
                        }
                        Goal::Have(item, n) => {
                            let id = route.item(item);
                            let short = n.saturating_sub(route.stock.get(&id).copied().unwrap_or(0));
                            route.produce(id, short, Way::Either);
                        }
                        Goal::Mine(ore, n) => {
                            let ore = route.item(ore);
                            let what = format!("mining {}", content.blocks.def(ore).name);
                            route.mine(ore, (*n).max(route.mined.get(&ore).copied().unwrap_or(0)), &what);
                        }
                        Goal::Level(skill, n) => {
                            let skill = skill_named(skill).unwrap_or_else(|| panic!("{}: no skill `{skill}`", quest.name));
                            route.need(skill, *n, "its own goal");
                        }
                        Goal::Home => {
                            let home = content.multiblocks.iter().find(|m| m.home).map(|m| m.name.clone());
                            route.build(&home.unwrap_or_else(|| panic!("{}: no multiblock makes a home", quest.name)));
                        }
                        Goal::Built(name, _) => {
                            let name = if name.contains(':') { name.clone() } else { format!("base:{name}") };
                            route.build(&name);
                        }
                        Goal::Visit(_) => {}
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
                match reward {
                    Reward::Xp(name, amount) => {
                        let skill = skill_named(name).unwrap_or_else(|| panic!("{}: no skill `{name}`", quest.name));
                        walk.xp.grant(skill, (amount * 10.0).round() as u32);
                    }
                    Reward::Item(name, count) => *walk.stock.entry(walk.item(name)).or_default() += *count as u32,
                }
            }
            done.push(quest.name.clone());
        }
        assert!(walk.walls.is_empty(), "the main quest line asks for levels it hasn't earned: {:#?}", walk.walls);
    }
}
