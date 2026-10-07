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
use crate::creature::CreatureRegistry;
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
                    "skills.ron" => skills.extend(package, &source)?,
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
                        creatures.extend(CreatureRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    // `recipes.ron`, and `recipes_<topic>.ron` for what a system makes (weapons, machines).
                    topic if topic.starts_with("recipes") => {
                        recipes.extend(RecipeRegistry::read(package, &source)?);
                        // Kept with the other files too, so the fingerprint covers it.
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "fishing.ron" => {
                        fishing.push(FishingRegistry::read(package, &source)?);
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
                        treasure.push(crate::treasure::Treasure::read(package, &source)?);
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
        let blocks = BlockRegistry::new(blocks)?;
        let crops = CropRegistry::new(crops, &blocks, &skills)?;
        let creatures = CreatureRegistry::new(creatures, &blocks, &skills)?;
        let cooking = CookingRegistry::new(cooking, &blocks, &skills)?;
        let recipes = RecipeRegistry::new(recipes, &blocks, &skills)?;
        let effects = EffectRegistry::new(effects)?;
        let tools = ToolRegistry::new(tools, &blocks, &skills)?;
        let combat = crate::combat::CombatRegistry::new(combat, &blocks, &skills)?;
        let fishing = FishingRegistry::new(fishing, &blocks, &skills)?;
        let regrowth = crate::regrowth::Regrowth::new(regrowth, &blocks)?;
        let multiblocks = crate::multiblock::MultiblockRegistry::new(multiblocks, &blocks)?;
        let machines = crate::machine::MachineRegistry::new(machines, processes, &blocks)?;
        if let Some(unknown) = machines.walkers().iter().find(|w| creatures.id(w).is_none()) {
            return Err(RegistryError(format!("machines.ron: no creature `{unknown}` to walk a treadwheel")));
        }
        let treasure = crate::treasure::Treasure::new(treasure, &blocks)?;
        // An opened chest becomes a bin with room for all its table can give.
        for (block, chest) in treasure.chests() {
            let room = machines.get(chest.opens_to).map_or(0, |m| m.bin_slots());
            let most = treasure.tables[chest.table].most_stacks();
            if room < most {
                return Err(RegistryError(format!("treasure.ron: `{}` opens to a bin of {room} stacks, but its table gives up to {most}", blocks.def(block).name)));
            }
        }
        crate::inventory::set_stack_limits(stack_limits(&blocks, &stacking)?);
        crate::inventory::set_pouches(pouches(&blocks));
        crate::inventory::set_worn_slots(combat.worn_slots());
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
            value: Arc::new(crate::value::Value::merge(value)),
            multiblocks: Arc::new(multiblocks),
            machines: Arc::new(machines),
            quests: Arc::new(crate::quest::QuestRegistry::new(quests)?),
            journal: Arc::new(crate::journal::JournalRules::merge(journal)),
            files: Arc::new(files),
            english: Arc::new(Lang::load(dir, ENGLISH)),
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

/// Each item's pouch: the first item tagged `pouch` and `holds:TAG` for a tag it has.
fn pouches(blocks: &BlockRegistry) -> Vec<Option<crate::block::BlockId>> {
    let holders: Vec<(crate::block::BlockId, String)> = blocks
        .ids()
        .filter(|&id| blocks.def(id).has_tag("pouch"))
        .flat_map(|id| blocks.def(id).tags.iter().filter_map(move |t| Some((id, t.strip_prefix("holds:")?.to_owned()))))
        .collect();
    blocks.ids().map(|id| holders.iter().find(|(_, tag)| blocks.def(id).has_tag(tag)).map(|&(pouch, _)| pouch)).collect()
}
