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
    /// What loading noticed and allowed, for the log: a later package replacing data an
    /// earlier one set.
    pub notes: Arc<Vec<String>>,
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
        let mut creature_limits = crate::creature::Limits::default();
        let mut limits_set_by: Option<String> = None;
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
                        let (defs, set) = CreatureRegistry::read(package, &source)?;
                        creatures.extend(defs);
                        // A later package's limits replace an earlier one's (Design, 2026-10-07).
                        if let Some(set) = set {
                            let (limits, changed) = creature_limits.with(&set);
                            if let Some(earlier) = limits_set_by.replace(package.clone()) {
                                let note = format!("{package}/creatures.ron replaces the creature limits {earlier} set ({})", changed.join(", "));
                                tracing::info!("{note}");
                                notes.push(note);
                            }
                            creature_limits = limits;
                        }
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
        let creatures = CreatureRegistry::new(creatures, creature_limits, &blocks, &skills)?;
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
        let (skills, blocks) = (&content.skills, &content.blocks);
        let skill_named = |name: &str| skills.id(name).or_else(|| skills.id(&format!("core:{name}")));
        let item = |name: &str| blocks.id(name).or_else(|| blocks.id(&format!("base:{name}")));
        // The block an ingredient must be mined as, if every item that can fill its place
        // is mined (the easiest of them); none when something else will do (charcoal for
        // coal in a torch).
        let mined = |accepts: &[crate::block::BlockId]| {
            let rules: Option<Vec<_>> = accepts.iter().map(|&b| content.tools.rule(b).filter(|r| r.skill.is_some()).map(|r| (b, r))).collect();
            rules?.into_iter().min_by_key(|(_, r)| r.level)
        };
        // What making one batch of `name` asks and gives: (skill, level, tenths of xp, how
        // many it makes, the ores it uses up), from the easiest grid recipe or, failing
        // that, cooking recipe.
        type Batch<'a> = (Option<SkillId>, u8, u32, u32, Vec<(crate::block::BlockId, &'a crate::tool::Rule)>);
        let recipe = |name: &str| -> Option<Batch> {
            let id = item(name)?;
            let grid = content.recipes.iter().filter(|r| r.output == id).min_by_key(|r| r.level).map(|r| {
                let ores = r.alternatives().into_iter().flatten().filter_map(|accepts| mined(accepts)).collect();
                (r.skill, r.level, r.xp, r.count as u32, ores)
            });
            let cooked = || {
                let r = content.cooking.iter().map(|(_, r)| r).filter(|r| r.output == id).min_by_key(|r| r.def.level)?;
                let inputs: Vec<crate::block::BlockId> = content.cooking.iter().filter(|(_, other)| std::ptr::eq(*other, r)).map(|(input, _)| input).collect();
                Some((r.skill, r.def.level, r.def.xp, 1, mined(&inputs).into_iter().collect()))
            };
            grid.or_else(cooked).map(|(skill, level, xp, count, ores)| (skill, level, (xp * 10.0).round() as u32, count, ores))
        };
        // The line and every quest it comes after, in file order once their `after` is done.
        let quests: Vec<&crate::quest::QuestDef> = content.quests.iter().collect();
        let mut wanted: Vec<String> = quests.iter().filter(|q| q.branch == "main").map(|q| q.name.clone()).collect();
        let mut i = 0;
        while i < wanted.len() {
            let quest = quests.iter().find(|q| q.name == wanted[i]).expect("a quest named in `after`");
            for after in &quest.after {
                let first = after.split('|').next().unwrap().to_owned();
                if !wanted.contains(&first) {
                    wanted.push(first);
                }
            }
            i += 1;
        }
        let mut xp = crate::skill::Skills::default();
        skills.start(&mut xp);
        let mut made: FxHashMap<String, u32> = FxHashMap::default();
        let mut done: Vec<String> = Vec::new();
        let mut walls = Vec::new();
        while done.len() < wanted.len() {
            let ready = |q: &&&crate::quest::QuestDef| !done.contains(&q.name) && wanted.contains(&q.name) && q.after.iter().all(|a| a.split('|').any(|a| done.iter().any(|d| d == a)));
            let Some(quest) = quests.iter().find(ready) else { panic!("the main line can't go on after {done:?}") };
            // Each route: what it makes (item, how many) and mines (ore, how many).
            let also: Vec<(String, u32)> = ALSO_MADE.iter().filter(|(q, _)| *q == quest.name).flat_map(|(_, items)| items.iter().map(|i| (i.to_string(), 1))).collect();
            let mut gains: Option<Vec<u32>> = None;
            let mut made_after: Option<FxHashMap<String, u32>> = None;
            let mut routes = Vec::new();
            for task in &quest.tasks {
                let mut route = xp.clone();
                let mut stuck = Vec::new();
                let mut made_here = made.clone();
                let makes = also.iter().cloned().chain(task.iter().filter_map(|goal| match goal {
                    Goal::Craft(item, n) | Goal::Cook(item, n) if item != "any" && !item.starts_with("tag:") => Some((item.clone(), *n)),
                    _ => None,
                }));
                for (name, n) in makes {
                    let Some((skill, level, tenths, count, ores)) = recipe(&name) else { panic!("{}: nothing makes `{name}`", quest.name) };
                    let had = made_here.entry(name.clone()).or_default();
                    let batches = n.saturating_sub(*had).div_ceil(count);
                    *had += batches * count;
                    // The ores it uses up are mined first.
                    for (ore, rule) in ores.into_iter().filter(|_| batches > 0) {
                        let skill = rule.skill.expect("mined");
                        if skills.level(route.xp(skill)) < rule.level {
                            stuck.push(format!("{}: {name} uses {} at {} {}, the line gives {} xp", quest.name, blocks.def(ore).name, skills.get(skill).name, rule.level, route.xp(skill) / 10));
                        }
                        route.grant(skill, (rule.xp * 10.0).round() as u32 * batches);
                    }
                    let Some(skill) = skill else { continue };
                    if batches > 0 && skills.level(route.xp(skill)) < level {
                        stuck.push(format!("{}: {name} needs {} {level}, the line gives {} xp", quest.name, skills.get(skill).name, route.xp(skill) / 10));
                    }
                    route.grant(skill, tenths * batches);
                }
                for goal in task {
                    if let Goal::Mine(ore, n) = goal
                        && let Some(rule) = item(ore).and_then(|id| content.tools.rule(id))
                        && let Some(skill) = rule.skill
                    {
                        if skills.level(route.xp(skill)) < rule.level {
                            stuck.push(format!("{}: mining {ore} needs {} {}, the line gives {} xp", quest.name, skills.get(skill).name, rule.level, route.xp(skill) / 10));
                        }
                        route.grant(skill, (rule.xp * 10.0).round() as u32 * n);
                    }
                }
                if stuck.is_empty() {
                    // Only what every open route gives counts.
                    let these: Vec<u32> = skills.ids().map(|id| route.xp(id)).collect();
                    gains = Some(match gains {
                        None => these,
                        Some(old) => old.into_iter().zip(these).map(|(a, b)| a.min(b)).collect(),
                    });
                    made_after = Some(match made_after {
                        None => made_here,
                        Some(old) => old.into_iter().map(|(name, n)| (name.clone(), n.min(made_here.get(&name).copied().unwrap_or(0)))).collect(),
                    });
                } else {
                    routes.push(stuck);
                }
            }
            if let Some(after) = made_after {
                made = after;
            }
            match gains {
                Some(gains) => {
                    for (id, xp_now) in skills.ids().zip(gains) {
                        xp.grant(id, xp_now - xp.xp(id));
                    }
                }
                // No route open: report the first's walls and carry on as if it were done.
                None => walls.extend(routes.into_iter().next().unwrap_or_default()),
            }
            for reward in &quest.rewards {
                if let Reward::Xp(name, amount) = reward {
                    let skill = skill_named(name).unwrap_or_else(|| panic!("{}: no skill `{name}`", quest.name));
                    xp.grant(skill, (amount * 10.0).round() as u32);
                }
            }
            done.push(quest.name.clone());
        }
        assert!(walls.is_empty(), "the main quest line asks for levels it hasn't earned: {walls:#?}");
    }
}
