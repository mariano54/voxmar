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
                    "skills.ron" => skills.extend(package, &source)?,
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
                        creatures.extend(CreatureRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    // `recipes.ron`, and `recipes_<topic>.ron` for what a system makes (weapons).
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
        let creatures = CreatureRegistry::new(creatures, &blocks, &skills)?;
        let cooking = CookingRegistry::new(cooking, &blocks, &skills)?;
        let recipes = RecipeRegistry::new(recipes, &blocks, &skills)?;
        let effects = EffectRegistry::new(effects)?;
        let tools = ToolRegistry::new(tools, &blocks, &skills)?;
        let combat = crate::combat::CombatRegistry::new(combat, &blocks, &skills)?;
        let fishing = FishingRegistry::new(fishing, &blocks, &skills)?;
        let regrowth = crate::regrowth::Regrowth::new(regrowth, &blocks)?;
        let multiblocks = crate::multiblock::MultiblockRegistry::new(multiblocks, &blocks)?;
        crate::inventory::set_stack_limits(stack_limits(&blocks, &stacking)?);
        crate::inventory::set_pouches(pouches(&blocks)?);
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
    use super::*;
    use crate::skill::SkillId;

    /// Every skill something trains has a way in at level 1: one whose only source asks
    /// for level 5 never leaves level 1 (Construction, with only the altar, did: design
    /// draft 94). Skills nothing trains yet (Ranged, Magic, Alchemy) are waiting for their
    /// systems, and the combat skills are trained by any fight.
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
    }
}
