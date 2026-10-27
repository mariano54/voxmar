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
use crate::creature::{Climbing, CreatureRegistry, DefenceTraining, Limits, Rules};
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
    /// Placed lights that burn out and are lit again (`flame`).
    pub flames: Arc<crate::flame::Flames>,
    /// Weapons, their classes and styles (`combat`).
    pub combat: Arc<crate::combat::CombatRegistry>,
    /// The prayer book (`prayer`): Magic's prayers, their points, and what altars take.
    pub prayers: Arc<crate::prayer::PrayerRegistry>,
    /// What bites where.
    pub fishing: Arc<FishingRegistry>,
    /// Structures recognised from patterns of blocks (the Homestead Hearth).
    pub multiblocks: Arc<crate::multiblock::MultiblockRegistry>,
    /// Machines, what they burn and what they make (`machines.ron`, `processing.ron`).
    pub machines: Arc<crate::machine::MachineRegistry>,
    /// Where each item is worn (`combat::Slot` index), if it is: armour, shields, charms.
    pub worn_slots: Arc<Vec<Option<u8>>>,
    /// The quest book.
    pub quests: Arc<crate::quest::QuestRegistry>,
    /// The quest book's lanes, one per age and branch (`quest_paths.ron`).
    pub quest_paths: Arc<crate::quest_path::QuestPaths>,
    /// What it takes to study and master the journal's entries.
    pub journal: Arc<crate::journal::JournalRules>,
    /// Which wild plants come back where they were picked, and how soon.
    pub regrowth: Arc<crate::regrowth::Regrowth>,
    /// What catches fire from a flame beside it, and how it burns (`fire.ron`).
    pub fire: Arc<crate::fire::Fire>,
    /// Wild nests, skeps and what bees do (`bees.ron`).
    pub bees: Arc<crate::bees::Bees>,
    /// Home stations left to work, and fertilisers (`stations.ron`).
    pub stations: Arc<crate::station::Stations>,
    /// Traps set for small animals (`traps.ron`).
    pub traps: Arc<crate::trap::Traps>,
    /// Locked chests, the lockpicks that open them and what they hold.
    pub treasure: Arc<crate::treasure::Treasure>,
    /// What things are worth (`value`).
    pub value: Arc<crate::value::Value>,
    /// Parties that come by on their own errands (`wandering.ron`).
    pub wandering: Arc<crate::wander::WanderRegistry>,
    /// Why refused actions failed, in words (`refusals.ron`).
    pub refusals: Arc<crate::refusal::Refusals>,
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
pub trait Settable: Clone {
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

/// Who set each piece of single-valued data last (Design draft 91): every place a later
/// package's number or list replaces an earlier one's goes through [`Laid::set`], so the
/// load log names both packages. A piece is named by the field (or path, or key) it sits at.
#[derive(Clone, Debug, Default)]
pub struct Laid {
    /// The package that set everything not named in `by`: a first file that sets it all.
    whole: Option<String>,
    by: std::collections::BTreeMap<String, String>,
}

impl Laid {
    /// Everything set by `package`, so far.
    pub fn whole(package: &str) -> Self {
        Self { whole: Some(package.to_owned()), ..Self::default() }
    }

    /// Records that `package`'s `file` sets `keys` of `what`. Where that replaces what
    /// another package set, pushes one note per earlier package (and logs it): "mod/x.ron
    /// replaces the what base set (key, key)".
    pub fn set<S: AsRef<str>>(&mut self, package: &str, file: &str, what: &str, keys: impl IntoIterator<Item = S>, notes: &mut Vec<String>) {
        let mut replaced: Vec<(String, Vec<String>)> = Vec::new();
        for key in keys {
            let key = key.as_ref();
            let earlier = self.by.insert(key.to_owned(), package.to_owned()).or_else(|| self.whole.clone());
            let Some(earlier) = earlier.filter(|earlier| earlier != package) else { continue };
            match replaced.iter_mut().find(|(name, _)| *name == earlier) {
                Some((_, keys)) => keys.push(key.to_owned()),
                None => replaced.push((earlier, vec![key.to_owned()])),
            }
        }
        for (earlier, keys) in replaced {
            let note = format!("{package}/{file} replaces the {what} {earlier} set ({})", keys.join(", "));
            tracing::info!("{note}");
            notes.push(note);
        }
    }
}

/// Single-valued data as the packages so far set it.
#[derive(Default)]
pub(crate) struct Setting<T> {
    pub(crate) value: T,
    laid: Laid,
}

impl<T: Settable> Setting<T> {
    /// A value one package's file set whole.
    pub(crate) fn whole(value: T, package: &str) -> Self {
        Self { value, laid: Laid::whole(package) }
    }

    /// Lays what `package`'s `file` sets of `what` over the value, noting what that
    /// replaces of an earlier package's.
    pub(crate) fn set(&mut self, package: &str, file: &str, what: &str, set: Option<T::Set>, notes: &mut Vec<String>) {
        let Some(set) = set else { return };
        let changed;
        (self.value, changed) = self.value.clone().with(&set);
        self.laid.set(package, file, what, changed, notes);
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

/// The files of a package read from `Content::files`, by name, once the load has passed
/// them by: worldgen's (the land, its biomes, settlements and ores), the server's (survival,
/// the spawn kit, wear), the client's (sounds, music, the look of the HUD) and the engine's
/// own `refusals.ron`. With those the load reads itself, these are every name a package's
/// `.ron` may have; any other is read by nothing, which the load warns of (a misspelt
/// `quest.ron` would otherwise be dropped without a word).
pub const READ_ELSEWHERE: &[&str] = &[
    "action_fx.ron", "ambience.ron", "ambient.ron", "biomes.ron", "cave_biomes.ron", "celebrations.ron", "climate.ron", "combat_fx.ron",
    "combat_hud.ron", "creature_motion.ron", "creature_sounds.ron", "cultures.ron", "decorations.ron", "hoards.ron",
    "interface.ron", "lairs.ron", "land.ron", "look.ron", "menu_bar.ron", "music.ron", "night_light.ron", "ores.ron", "plants.ron",
    "player_look.ron", "quest_book.ron", "refusals.ron", "rocks.ron", "settlements.ron", "sounds.ron", "spawn_kit.ron", "streaming.ron", "surfaces.ron",
    "survival.ron", "terrain.ron", "trails.ron", "trees.ron", "wear.ron",
];

/// How many single letters `a` and `b` differ by (added, dropped or changed).
fn letters_apart(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, &y) in b.iter().enumerate() {
            let next = (last + usize::from(x != y)).min(row[j] + 1).min(row[j + 1] + 1);
            last = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

impl Content {
    /// The cooking recipes a player can work: those whose station some block carries. A
    /// recipe at a station no block has (the potters' `village_kiln`, worked for the player by
    /// a wandering party) is left out of the recipe book, the makeable list and the guides.
    pub fn player_cooking(&self) -> impl Iterator<Item = (crate::block::BlockId, &crate::cooking::Recipe)> {
        self.cooking.iter().filter(|(_, recipe)| self.blocks.tagged(&recipe.def.station).next().is_some())
    }

    /// The tags of blocks crafting looks for within reach: what recipes are made near (a
    /// chopping block) and what tools are mended at, each once.
    pub fn near_tags(&self) -> Vec<&str> {
        let mut tags = self.recipes.near_tags();
        tags.extend(self.tools.mend_tags());
        tags.sort_unstable();
        tags.dedup();
        tags
    }

    /// The least tool that is a `kind` of at least `tier`, by property: the lowest tier, then
    /// the lowest worth (value.rs), then the first in registry order. What a refusal names
    /// ("Needs a Flint Pickaxe or better").
    pub fn least_tool(&self, kind: usize, tier: u8) -> Option<crate::block::BlockId> {
        let mut known = std::collections::HashMap::new();
        self.tools.all_of(kind, tier).map(|(t, id)| (t, crate::value::worth_in(self, id, &mut known), id)).min().map(|(_, _, id)| id)
    }

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
        let mut climbing = Setting::<Climbing>::default();
        let mut genetics = Setting::<crate::creature::Genetics>::default();
        let mut fishing_rules = Setting::<crate::fishing::FishingRules>::default();
        let mut notes = Vec::new();
        let mut cooking = Vec::new();
        let mut recipes = Vec::new();
        let mut effects = Vec::new();
        let mut tools = Vec::new();
        let mut combat = Vec::new();
        let mut prayers = Vec::new();
        let mut value = Vec::new();
        let mut stacking = Vec::new();
        let mut fishing = Vec::new();
        let mut regrowth = Vec::new();
        let mut fire = Vec::new();
        let mut bees = Vec::new();
        let mut stations = Vec::new();
        let mut traps = Vec::new();
        let mut treasure = Vec::new();
        let mut journal = Vec::new();
        let mut multiblocks = Vec::new();
        let mut machines = Vec::new();
        let mut processes = Vec::new();
        let mut quests = Vec::new();
        let mut quest_paths = Vec::new();
        let mut wandering = Vec::new();
        let mut wander_rules = Setting::<crate::wander::Rules>::default();
        let mut shapes = Vec::new();
        let mut shape_limits = Setting::<crate::shape::ShapeLimits>::default();
        let mut files = Vec::new();
        // The files nothing reads, and the names of those something did, to name the
        // likeliest meant.
        let (mut unread, mut read) = (Vec::new(), std::collections::BTreeSet::new());
        for package in &packages {
            let path = dir.join(package);
            let mut names: Vec<String> = std::fs::read_dir(&path)
                .map_err(|e| read_error(&path, e))?
                .flatten()
                // No folders, through links too: a folder named `x.ron` is no definition file
                // (a dangling link still fails the load, read below).
                .filter(|entry| !entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.ends_with(".ron"))
                .collect();
            names.sort();
            for name in names {
                let file = path.join(&name);
                let source = std::fs::read_to_string(&file).map_err(|e| read_error(&file, e))?;
                let (named, unread_before) = (name.clone(), unread.len());
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
                    // `creatures.ron`, and `creatures_<topic>.ron` for the people a system
                    // places (villagers).
                    topic if topic == "creatures.ron" || topic.starts_with("creatures_") && topic.ends_with(".ron") => {
                        let read = CreatureRegistry::read(package, &source)?;
                        creatures.extend(read.defs);
                        creature_limits.set(package, &name, "creature limits", read.limits, &mut notes);
                        defence.set(package, &name, "Defence training", read.defence, &mut notes);
                        condition.set(package, &name, "condition rules", read.condition, &mut notes);
                        climbing.set(package, &name, "climbing", read.climbing, &mut notes);
                        genetics.set(package, &name, "genetics", read.genetics, &mut notes);
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
                    // `quests.ron`, and `quests_<topic>.ron` for a book of its own (the Nomad's,
                    // the Farm's), read in name order like every file of a package.
                    topic if topic == "quests.ron" || topic.strip_prefix("quests_").is_some_and(|t| t != ".ron") => {
                        quests.push(crate::quest::QuestRegistry::read_named(package, &name, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    crate::quest_path::QuestPaths::FILE => {
                        quest_paths.push((package.clone(), crate::quest_path::QuestPaths::read(package, &source)?));
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
                        journal.push((package.clone(), crate::journal::JournalRules::read(package, &source)?));
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "regrowth.ron" => {
                        regrowth.push(crate::regrowth::Regrowth::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "fire.ron" => {
                        fire.push(crate::fire::Fire::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "bees.ron" => {
                        bees.push(crate::bees::Bees::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "stations.ron" => {
                        stations.push(crate::station::Stations::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "traps.ron" => {
                        traps.push(crate::trap::Traps::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "treasure.ron" => {
                        treasure.push((package.clone(), crate::treasure::Treasure::read(package, &source)?));
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "wandering.ron" => {
                        let package_file = crate::wander::WanderRegistry::read(package, &source)?;
                        wander_rules.set(package, &name, "wandering rules", package_file.rules, &mut notes);
                        wandering.push(package_file);
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
                        value.push((package.clone(), crate::value::Value::read(package, &source)?));
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "combat.ron" => {
                        combat.push(crate::combat::CombatRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "prayers.ron" => {
                        prayers.push(crate::prayer::PrayerRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "effects.ron" => {
                        effects.push(EffectRegistry::read(package, &source)?);
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    "movement.ron" => {
                        // The first package sets every number; each later one only what it
                        // changes, which replaces the earlier packages' (Design draft 91).
                        match movement.as_mut() {
                            None => movement = Some(Setting::whole(Tuning::from_ron(&source)?, package)),
                            Some(setting) => {
                                setting.set(package, &name, "movement", Some(Tuning::set_from_ron(&source)?), &mut notes);
                                setting.value.validate().map_err(|e| RegistryError(format!("{package}/{e}")))?;
                            }
                        }
                        // Kept with the other files too, so the fingerprint covers it.
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                    _ => {
                        if !READ_ELSEWHERE.contains(&name.as_str()) {
                            unread.push(format!("{package}/{name}"));
                        }
                        files.push(ContentFile { package: package.clone(), name, source });
                    }
                }
                if unread.len() == unread_before {
                    read.insert(named);
                }
            }
        }
        for file in unread {
            // Names compared without `.ron`, a short one (`fish`) only a letter from another.
            let stem = |name: &str| name.strip_suffix(".ron").unwrap_or(name).to_owned();
            let name = stem(file.rsplit('/').next().unwrap_or_default());
            let near = if name.chars().count() <= 5 { 1 } else { 2 };
            let meant = read.iter().map(|known| (letters_apart(&name, &stem(known)), known)).filter(|&(apart, _)| apart <= near).min();
            // A topic family the game reads (`creature_motion`, `creature_sounds`) names its
            // members when the topic itself is far from all of them (`creature_horses`).
            let family: Vec<&str> = match name.split_once('_') {
                Some((head, _)) => read.iter().filter(|known| known.split_once('_').is_some_and(|(h, _)| h == head)).map(String::as_str).collect(),
                None => Vec::new(),
            };
            let note = match meant {
                Some((_, known)) => format!("{file} is read by nothing (meant {known}?)"),
                None if !family.is_empty() => format!("{file} is read by nothing: of its family the game reads only {}", family.join(", ")),
                None => format!("{file} is read by nothing: no part of the game reads a file of that name"),
            };
            tracing::warn!("{note}");
            notes.push(note);
        }
        let renames = BlockRegistry::renames(&blocks);
        let blocks = BlockRegistry::expand(blocks)?;
        weather.validate()?;
        notes.extend(weather.notes().iter().cloned());
        if weather.try_tuning().is_none() {
            return Err(RegistryError("no package's weather.ron sets its `tuning`".to_owned()));
        }
        let movement = movement.ok_or_else(|| RegistryError("no package sets movement (movement.ron)".to_owned()))?.value;
        skills.validate()?;
        let mut blocks = BlockRegistry::with_shapes(blocks, &crate::shape::Shapes::gather(shapes, shape_limits.value)?)?;
        blocks.rename(renames)?;
        let crops = CropRegistry::new(crops, &blocks, &skills)?;
        let rules = Rules { limits: creature_limits.value, defence: defence.value, condition: condition.value, climbing: climbing.value, genetics: genetics.value };
        let creatures = CreatureRegistry::new(creatures, rules, &blocks, &skills)?;
        let cooking = CookingRegistry::new(cooking, &blocks, &skills)?;
        let recipes = RecipeRegistry::new(recipes, &blocks, &skills)?;
        let effects = EffectRegistry::new(effects)?;
        // A home asks for tags blocks carry: a misspelt one would be a step nobody can climb.
        // (`kept` counts the animals the player keeps around it.)
        if let Some(tag) = effects.home_tags().iter().find(|tag| *tag != "kept" && !blocks.ids().any(|id| blocks.def(id).has_tag(tag))) {
            return Err(RegistryError(format!("effects.ron: a home is asked for blocks tagged `{tag}`, which no block carries")));
        }
        // So are the tags of blocks near, and of the item in hand.
        if let Some(tag) = effects.near_asks().iter().map(|(tag, _)| tag).chain(effects.held_tags()).find(|tag| !blocks.ids().any(|id| blocks.def(id).has_tag(tag))) {
            return Err(RegistryError(format!("effects.ron: an effect asks for a block or held item tagged `{tag}`, which no block carries")));
        }
        // And those of the seat sat on: a seat must carry each.
        if let Some(tag) = effects.seat_tags().iter().find(|tag| !blocks.ids().any(|id| blocks.def(id).has_tag(tag) && blocks.def(id).has_tag(crate::action::SEAT_TAG))) {
            return Err(RegistryError(format!("effects.ron: an effect asks for a seat tagged `{tag}`, which no seat carries")));
        }
        // Every kept animal needs water, met by a block tagged `drink` near its home.
        crate::creature::condition::check_drink(&blocks, !creatures.is_empty())?;
        let tools = ToolRegistry::new(tools, &blocks, &skills)?;
        let flames = crate::flame::Flames::new(&blocks)?;
        let combat = crate::combat::CombatRegistry::new(combat, &blocks, &skills)?;
        let prayers = crate::prayer::PrayerRegistry::new(prayers, &blocks, &skills)?;
        let fishing = FishingRegistry::new(fishing, fishing_rules.value, &blocks, &skills)?;
        let regrowth = crate::regrowth::Regrowth::new(regrowth, &blocks)?;
        let fire = crate::fire::Fire::new(fire, &blocks)?;
        let multiblocks = crate::multiblock::MultiblockRegistry::new(multiblocks, &blocks)?;
        let machines = crate::machine::MachineRegistry::new(machines, processes, &blocks)?;
        for structure in multiblocks.iter() {
            for &(fuel, count) in &structure.fuels {
                if !machines.burn_seconds(fuel).is_some_and(|seconds| seconds.is_finite() && seconds > 0.0 && (seconds * count as f32).is_finite()) {
                    return Err(crate::block::RegistryError(format!("multiblock {}: fuel {} needs a positive finite burn time in machines.ron", structure.name, blocks.def(fuel).name)));
                }
                if machines.fuel_heat(fuel).is_some_and(|heat| heat < structure.heat) {
                    return Err(crate::block::RegistryError(format!("multiblock {}: fuel {} burns cooler than the heat {} it asks", structure.name, blocks.def(fuel).name, structure.heat)));
                }
            }
        }
        // A seat that rocks the view is sat on.
        if let Some(id) = blocks.ids().find(|&id| blocks.def(id).has_tag(crate::vitals::ROCKS) && !blocks.def(id).has_tag(crate::action::SEAT_TAG)) {
            return Err(RegistryError(format!("blocks: `{}` rocks (`{}`) but is no seat", blocks.def(id).name, crate::vitals::ROCKS)));
        }
        // What keeps spare lights for a home keeps them in its store.
        if let Some(id) = blocks.ids().find(|&id| blocks.def(id).has_tag(crate::flame::SPARE_LIGHTS) && !machines.get(id).is_some_and(|m| m.is_store())) {
            return Err(RegistryError(format!("blocks: `{}` keeps spare lights (`{}`) but is no store (machines.ron)", blocks.def(id).name, crate::flame::SPARE_LIGHTS)));
        }
        if let Some(unknown) = machines.walkers().iter().find(|w| creatures.id(w).is_none()) {
            return Err(RegistryError(format!("machines.ron: no creature `{unknown}` to walk a treadwheel")));
        }
        let value = crate::value::Value::merge(value, &mut notes);
        value.check()?;
        // A price or level set for a thing no package has would price nothing.
        if let Some(name) = value.fixed.keys().chain(value.levels.keys()).find(|name| blocks.id(name).is_none()) {
            return Err(RegistryError(format!("value.ron: a worth is set for `{name}`, which no package has")));
        }
        let limits = stack_limits(&blocks, &stacking)?;
        for structure in multiblocks.iter() {
            for &(fuel, count) in &structure.fuels {
                if count > limits[fuel.0 as usize] {
                    return Err(RegistryError(format!("multiblock {}: its {}-item fuel charge exceeds {}'s stack limit", structure.name, count, blocks.def(fuel).name)));
                }
            }
        }
        let treasure = crate::treasure::Treasure::new(treasure, &blocks, &|name| creatures.find(name), &value, &limits, &mut notes)?;
        // An opened chest becomes a bin with room for all it can give.
        for (block, chest) in treasure.chests() {
            let room = machines.get(chest.opens_to).map_or(0, |m| m.bin_slots());
            let most = treasure.most_stacks(chest);
            if room < most {
                return Err(RegistryError(format!("treasure.ron: `{}` opens to a bin of {room} stacks, but its table gives up to {most}", blocks.def(block).name)));
            }
        }
        let bees = crate::bees::Bees::new(bees, &blocks, &skills, &effects, &creatures)?;
        let stations = crate::station::Stations::new(stations, &blocks, &skills)?;
        let traps = crate::trap::Traps::new(traps, &blocks, &creatures, &skills)?;
        let wandering = crate::wander::WanderRegistry::new(wandering, wander_rules.value, &creatures, &blocks, &skills, &effects, &cooking, &weather)?;
        let parties_cook: Vec<&str> = wandering
            .iter()
            .flat_map(|(_, event)| &event.meetings)
            .flat_map(|meeting| &meeting.lines)
            .flat_map(|line| &line.gives)
            .filter_map(|give| match give {
                crate::wander::ResolvedReward::Cook(station, ..) => Some(station.as_str()),
                _ => None,
            })
            .collect();
        cooking.check_stations(&blocks, &parties_cook)?;
        // What picking a block gives besides its drops: a ripe crop's harvest, a wild plant's
        // forage (the quest load check counts a Pick goal as teaching them).
        let harvests: Vec<(crate::block::BlockId, crate::block::BlockId)> = crops
            .iter()
            .flat_map(|(_, crop)| {
                let ripe = crop.harvest.iter().map(|(item, _)| (crop.ripe(), *item));
                let wild = crop.def.wild.iter().filter_map(|name| blocks.item_named(name)).flat_map(|wild| crop.forage.iter().map(move |(item, _)| (wild, *item)));
                ripe.chain(wild).collect::<Vec<_>>()
            })
            .collect();
        let mut quests = crate::quest::QuestRegistry::new(quests)?;
        quests.resolve(&crate::quest::Named { blocks: &blocks, skills: &skills, creatures: &creatures, fishing: &fishing, multiblocks: &multiblocks, steps: &effects.steps(), events: &wandering.iter().map(|(_, e)| e.def.name.clone()).collect::<Vec<_>>(), lines: &wandering.iter().flat_map(|(_, e)| e.meetings.iter().flat_map(|m| m.lines.iter()).map(|l| format!("{}/{}", e.def.name, l.name))).collect::<Vec<_>>(), prayers: &prayers.all().map(|(_, p)| p.name.clone()).collect::<Vec<_>>(), effects: &effects.ids().map(|id| effects.get(id).name.clone()).collect::<Vec<_>>(), near: &recipes.iter().filter_map(|r| r.near.clone()).collect::<Vec<_>>(), stores: &machines.iter().filter(|m| m.is_store()).map(|m| blocks.def(m.block).name.clone()).collect::<Vec<_>>(), trapped: &traps.traps.iter().flat_map(|t| &t.catches).map(|&c| creatures.get(c).def.name.clone()).collect::<Vec<_>>(), harvests: &harvests })?;
        // A recipe of a later age needs that age's rite, or it could never be made.
        if let Some(recipe) = recipes.iter().find(|r| r.age.as_ref().is_some_and(|age| quests.rite(age).is_none())) {
            let age = recipe.age.as_deref().unwrap_or_default();
            return Err(RegistryError(format!("recipes.ron: {} opens in the age `{age}`, which no quest opens (`opens`)", blocks.def(recipe.output).name)));
        }
        let quest_paths = crate::quest_path::QuestPaths::new(quest_paths, &quests, &blocks)?;
        // An effect that waits on a quest nobody can finish would never come on.
        if let Some(quest) = effects.quests().iter().find(|quest| quests.get(quest).is_none()) {
            return Err(RegistryError(format!("effects.ron: an effect waits for the quest `{quest}`, which no package has")));
        }
        crate::inventory::set_stack_limits(limits);
        crate::inventory::set_pouches(pouches(&blocks)?);
        // A charm is worn in the charm slot, and is nothing else that is worn.
        let mut worn = combat.worn_slots();
        for id in blocks.ids().filter(|&id| blocks.def(id).has_tag("charm")) {
            let at = &mut worn[id.0 as usize];
            if at.is_some() {
                return Err(RegistryError(format!("blocks: `{}` is a charm and armour or a shield too: it is worn in one slot", blocks.def(id).name)));
            }
            *at = Some(crate::combat::Slot::Charm.index() as u8);
        }
        // `Wearing` asks of the charm worn: a tag no charm carries (a torch's `fire`, a
        // helmet's) would be an effect that can never hold, or two at once.
        let charm = Some(crate::combat::Slot::Charm.index() as u8);
        let charms: Vec<_> = blocks.ids().filter(|&id| worn[id.0 as usize] == charm).collect();
        if let Some(tag) = effects.worn_tags().iter().find(|tag| !charms.iter().any(|&id| blocks.def(id).has_tag(tag))) {
            return Err(RegistryError(format!("effects.ron: an effect asks for a charm tagged `{tag}`, which nothing worn carries")));
        }
        // And a charm that no effect asks for would be worn for nothing.
        if let Some(&id) = charms.iter().find(|&&id| !effects.worn_tags().iter().any(|tag| blocks.def(id).has_tag(tag))) {
            return Err(RegistryError(format!("blocks: `{}` is a charm, but no effect asks for it worn (effects.ron's `Wearing`)", blocks.def(id).name)));
        }
        crate::inventory::set_worn_slots(worn.clone());
        // A store's slots follow the player's own in a slot number of a byte.
        let mut holds = vec![None; blocks.len()];
        for machine in machines.iter().filter(|m| m.is_store()) {
            if machine.bin_slots() > crate::inventory::STORE_SLOTS {
                return Err(RegistryError(format!("machines.ron: the store `{}` has {} slots, more than {}", blocks.def(machine.block).name, machine.bin_slots(), crate::inventory::STORE_SLOTS)));
            }
            holds[machine.block.0 as usize] = machine.holds.clone();
        }
        crate::inventory::set_store_holds(holds);
        // A block that shows what it holds is a store with a slot for each spot it shows.
        for id in blocks.ids().filter(|&id| !blocks.def(id).shows.is_empty()) {
            let spots = blocks.def(id).shows.len();
            if !machines.get(id).is_some_and(|m| m.is_store() && m.bin_slots() >= spots) {
                return Err(RegistryError(format!("block `{}` shows {spots} things, but is no store (machines.ron) with a slot for each", blocks.def(id).name)));
            }
        }
        // A furnishing gives itself back only where a player placed it: one that drops
        // nothing anyway has no business with the tag.
        if let Some(id) = blocks.ids().find(|&id| blocks.def(id).has_tag("furnishing") && (blocks.info(id).drops.is_air() || blocks.info(id).drop_count == 0)) {
            return Err(RegistryError(format!(
                "block `{}` is a `furnishing` but drops nothing: give it a drop, or remove the tag (village props that drop nothing don't need it)",
                blocks.def(id).name
            )));
        }
        // A sleep rule's `bed` tag names a kind of bed: some bed must carry it, or the rest it
        // gives could never be had.
        if let Some(tag) = effects.sleep_beds().find(|tag| !blocks.ids().any(|id| blocks.def(id).has_tag("bed") && blocks.def(id).has_tag(tag))) {
            return Err(RegistryError(format!("effects.ron: a sleep rule asks for a bed tagged `{tag}`, and no block tagged `bed` carries it")));
        }
        // Every refusal is said in every language the game has.
        let refusals = crate::refusal::Refusals::load(&files)?;
        let languages: Vec<Lang> = crate::lang::languages(dir).into_iter().map(|language| Lang::load(dir, &language.code)).collect();
        let codes: Vec<&str> = languages.iter().map(Lang::code).collect();
        refusals.check_words(&codes, |code, key| languages.iter().any(|lang| lang.code() == code && lang.defines(key)))?;
        // Every age a rite opens has its name in every language: the greyed recipes and the
        // refusal say it.
        for age in quests.iter().filter_map(|q| q.opens.as_deref()) {
            let key = format!("quest.age.{age}");
            if let Some(lang) = languages.iter().find(|lang| !lang.defines(&key)) {
                return Err(RegistryError(format!("lang/{}.ron: a quest opens the age `{age}`, and `{key}` has no words", lang.code())));
            }
        }
        // How a worn tool is mended, by a tag and where, has words in every language: its
        // tooltip says it, and so does the line when it is nearly worn out; and a break, with
        // or without a spare brought up, says so.
        let mend_words: Vec<String> = tools.mend_by_tags().iter().map(|t| format!("item.mend.tag.{t}")).chain(tools.mend_tags().iter().map(|t| format!("item.mend.near.{t}"))).collect();
        for key in mend_words.into_iter().chain(["item.mend", "tool.nearly_worn", "tool.broke", "tool.broke_spare"].map(str::to_owned)) {
            if let Some(lang) = languages.iter().find(|lang| !lang.defines(&key)) {
                return Err(RegistryError(format!("lang/{}.ron: tools are mended, and `{key}` has no words", lang.code())));
            }
        }
        let mut content = Self {
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
            flames: Arc::new(flames),
            combat: Arc::new(combat),
            prayers: Arc::new(prayers),
            fishing: Arc::new(fishing),
            regrowth: Arc::new(regrowth),
            fire: Arc::new(fire),
            bees: Arc::new(bees),
            stations: Arc::new(stations),
            traps: Arc::new(traps),
            treasure: Arc::new(treasure),
            value: Arc::new(value),
            wandering: Arc::new(wandering),
            multiblocks: Arc::new(multiblocks),
            machines: Arc::new(machines),
            quests: Arc::new(quests),
            quest_paths: Arc::new(quest_paths),
            worn_slots: Arc::new(worn),
            journal: Arc::new(crate::journal::JournalRules::merge(journal, &mut notes)),
            refusals: Arc::new(refusals),
            files: Arc::new(files),
            english: Arc::new(Lang::load(dir, ENGLISH)),
            notes: Arc::new(notes),
        };
        // Mending follows the recipes, which load after the tools.
        let terms = ToolRegistry::link(&content)?;
        Arc::get_mut(&mut content.tools).expect("only this has it").set_mend_terms(terms);
        Ok(content)
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

/// Each item's most per stack: an item's own `stack:N` tag, else one for an item that wears
/// (`uses:N`, so each one wears on its own) or with a tag some package's `stacking.ron`
/// lists as single, else a full stack.
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
            let wears = tags.iter().any(|t| t.starts_with("uses:") || t.starts_with("burns:"));
            own.filter(|_| !wears).unwrap_or(if wears || tags.iter().any(|t| single.contains(t)) { 1 } else { crate::inventory::MAX_STACK }).min(crate::inventory::MAX_STACK)
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

    /// Loading checks every cooking station (`CookingRegistry::check_stations`): the game's
    /// content with one recipe at a misspelt station is refused, naming it.
    #[test]
    fn content_with_a_station_nothing_has_is_refused() {
        fn copy(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    copy(&path, &to.join(entry.file_name()));
                } else {
                    std::fs::copy(&path, to.join(entry.file_name())).unwrap();
                }
            }
        }
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let at = std::env::temp_dir().join(format!("voxmar-stations-{}", std::process::id()));
        copy(&assets, &at);
        let cooking = at.join("base/cooking.ron");
        let source = std::fs::read_to_string(&cooking).unwrap();
        let source = source.replacen("    recipes: [\n", "    recipes: [\n        (input: \"clay\", output: \"fired_clay\", station: \"pit_klin\", xp: 1.0),\n", 1);
        std::fs::write(&cooking, source).unwrap();
        let refused = Content::load(&at).err().map(|e| e.0);
        std::fs::remove_dir_all(&at).unwrap();
        assert!(refused.as_deref().is_some_and(|e| e.contains("`pit_klin`")), "{refused:?}");
    }

    /// The game's flues, hood and pot carry `CHIMNEY_PIECE_TAG`, so a renamed tag can't
    /// quietly leave every chimney smoking from under its roof.
    #[test]
    fn the_chimney_pieces_carry_the_chimney_piece_tag() {
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        for name in ["base:chimney_stack", "base:stone_flue", "base:chimney_hood", "base:chimney_pot"] {
            let id = content.blocks.id(name).unwrap_or_else(|| panic!("no block {name}"));
            assert!(content.blocks.def(id).has_tag(crate::block::CHIMNEY_PIECE_TAG), "{name} is not a chimney piece");
        }
    }

    /// Each timber's hatch opens and shuts as a pair, in every facing: shut it keeps the
    /// light out as a roof does and is no ladder; open it lets light by and is climbed.
    #[test]
    fn a_hatch_opens_to_be_climbed_and_shuts_to_keep_the_light_out() {
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let blocks = &content.blocks;
        for timber in ["oak", "birch", "dark_oak"] {
            for facing in ["", "_east", "_south", "_west"] {
                let shut = blocks.id(&format!("base:{timber}_hatch{facing}")).unwrap();
                let open = blocks.id(&format!("base:{timber}_hatch_open{facing}")).unwrap();
                assert_eq!((blocks.swapped(shut), blocks.swapped(open)), (Some(open), Some(shut)), "the {timber} hatch{facing} opens and shuts as a pair");
                let (shut, open) = (blocks.info(shut), blocks.info(open));
                assert!(shut.solid && !shut.climbable && shut.attenuation == 15, "a shut {timber} hatch{facing} lets light by or is climbed");
                assert!(open.solid && open.climbable && open.attenuation < 15, "an open {timber} hatch{facing} keeps light out or is not climbed");
            }
        }
    }

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
    enum Way<'a> {
        Grid,
        Cooked,
        CookedAt(&'a str),
        Either,
    }

    /// A recipe as the walk uses it: its skill, level and experience a batch, how many a
    /// batch makes, what each of its cells takes, and the station it needs (the crafting
    /// table for a big grid, a cooking station's tag). A smoulder counts as one too.
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
        fn recipe(&self, id: BlockId, way: Way<'_>) -> Option<Batch> {
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
                let r = content.cooking.iter().map(|(_, r)| r).filter(|r| r.output == id && match way { Way::CookedAt(station) => r.def.station == station, _ => true }).min_by_key(|r| r.def.level)?;
                let inputs: Vec<BlockId> = content.cooking.iter().filter(|(_, other)| std::ptr::eq(*other, r)).map(|(input, _)| input).collect();
                // `uses` of the held input, then each item it takes besides, one cell an item.
                let held = std::iter::repeat_n(inputs, r.def.uses as usize);
                let with = r.with.iter().flat_map(|&(item, n)| std::iter::repeat_n(vec![item], n as usize));
                Some(Batch { skill: r.skill, level: r.def.level, xp: r.def.xp, count: r.def.count as u32, cells: held.chain(with).collect(), station: Some(r.def.station.clone()) })
            };
            // A smoulder (the charcoal clamp): a batch is one structure, every piece it takes,
            // making what each of its charred cells breaks into.
            let smouldered = || {
                content.multiblocks.iter().find_map(|m| {
                    let s = m.smoulder.as_ref().filter(|s| content.blocks.info(s.done).drops == id)?;
                    let count = s.wood.len() as u32 * content.blocks.info(s.done).drop_count as u32;
                    let cells = m.pieces().into_iter().flat_map(|(accepts, n)| std::iter::repeat_n(accepts.to_vec(), n as usize)).collect();
                    Some(Batch { skill: None, level: 0, xp: 0.0, count, cells, station: None })
                })
            };
            match way {
                Way::Grid => grid(),
                Way::Cooked | Way::CookedAt(_) => cooked(),
                Way::Either => grid().or_else(cooked).or_else(smouldered),
            }
        }

        /// Makes `k` more of `id`, `way`: the easiest recipe, at its station, from what its
        /// cells take (used up from what is in hand, or made, mined or gathered first).
        fn produce(&mut self, id: BlockId, k: u32, way: Way<'_>) {
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

        /// Whether the line can bring `effect` on by now, by the server's rule
        /// (`EffectRegistry::can_feel`), with what it has made and holds, its home if built,
        /// and the quests `done`.
        fn can_feel(&self, effect: crate::effect::EffectId, done: &[String]) -> bool {
            // A thing made and kept counts once, the more of made and held: what went into
            // something else counts too, as Placed does.
            let mut items: Vec<(BlockId, u32)> = Vec::new();
            for (&id, &n) in self.made.iter().chain(&self.stock) {
                match items.iter_mut().find(|(i, _)| *i == id) {
                    Some((_, most)) => *most = (*most).max(n),
                    None if n > 0 => items.push((id, n)),
                    None => {}
                }
            }
            let home = self.built.iter().any(|name| self.content.multiblocks.get(name).is_some_and(|m| m.home));
            self.content.effects.can_feel(effect, &crate::effect::Means { items: &items, home, done, worn: &self.content.worn_slots }, &self.content.blocks)
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

    /// A Fence and a Gate asks for six plain fences beside the gate: gates and corners carry
    /// `fence` too, so seven gates set down (or one fence and six gates) are not the fences.
    #[test]
    fn a_fence_and_a_gate_wants_fences_not_gates() {
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let quest = content.quests.get("base:a_fence_and_a_gate").expect("A Fence and a Gate");
        let done = |tallies: &[(&str, u32)]| {
            let tallies: FxHashMap<String, u32> = tallies.iter().map(|(k, n)| (k.to_string(), *n)).collect();
            let journal = crate::journal::Journal::default();
            let has_tag = |name: &str, tag: &str| content.blocks.item_named(name).is_some_and(|id| content.blocks.def(id).has_tag(tag));
            let progress = crate::quest::Progress { journal: &journal, tallies: &tallies, level: &|_| 1, carried: &|_| 0, has_tag: &has_tag, drops: &|_| None, thriving: &|_| 0, home: false };
            quest.tasks.iter().any(|task| content.quests.task_done(task, &progress, &|_| true))
        };
        let (fence, gate) = ("base:oak_fence", "base:oak_fence_gate");
        let both = |fences: u32, gates: u32| {
            let keys = [format!("crafted:{fence}"), format!("crafted:{gate}"), format!("placed:{fence}"), format!("placed:{gate}")];
            done(&[(keys[0].as_str(), fences), (keys[1].as_str(), gates), (keys[2].as_str(), fences), (keys[3].as_str(), gates)])
        };
        assert!(both(6, 1), "six fences and a gate made and set down");
        assert!(!both(0, 7), "seven gates are no fences");
        assert!(!both(1, 6), "one fence and six gates are not six fences");
        assert!(!both(5, 1), "five fences are not six");
    }

    /// Lanterns of Cloth asks for paper lanterns: the metal and stone lanterns carry
    /// `lantern` too, so two copper lanterns made and set down are not the quest.
    #[test]
    fn lanterns_of_cloth_wants_paper_lanterns() {
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let quest = content.quests.get("base:lanterns_of_cloth").expect("Lanterns of Cloth");
        let done = |lantern: &str| {
            let keys = [format!("crafted:{lantern}"), format!("placed:{lantern}")];
            let tallies: FxHashMap<String, u32> = keys.into_iter().map(|k| (k, 2)).collect();
            let journal = crate::journal::Journal::default();
            let has_tag = |name: &str, tag: &str| content.blocks.item_named(name).is_some_and(|id| content.blocks.def(id).has_tag(tag));
            let progress = crate::quest::Progress { journal: &journal, tallies: &tallies, level: &|_| 1, carried: &|_| 0, has_tag: &has_tag, drops: &|_| None, thriving: &|_| 0, home: false };
            quest.tasks.iter().any(|task| content.quests.task_done(task, &progress, &|_| true))
        };
        let paper = content.blocks.tagged("paper_lantern").next().map(|b| content.blocks.def(b).name.clone()).expect("a paper lantern");
        assert!(done(&paper), "two paper lanterns made and set down");
        for other in ["base:copper_hanging_lantern", "base:iron_hanging_lantern", "base:stone_lantern"] {
            assert!(content.blocks.id(other).is_some(), "no `{other}`");
            assert!(!done(other), "two of `{other}` are no paper lanterns");
        }
    }

    /// Each Farm path's capstone comes after every other quest of its path, directly or
    /// through what comes before it, but for side quests and the age's rite. An "a|b" edge
    /// is followed down neither side, as it makes sure of neither. (The Nomad book is checked
    /// once it is laid out so.)
    #[test]
    fn a_farm_capstone_comes_after_all_of_its_path() {
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let farm: Vec<&crate::quest::QuestDef> = content.quests.iter().filter(|q| q.age == "farm").collect();
        let capstones: Vec<_> = farm.iter().filter(|q| q.capstone).collect();
        assert!(capstones.len() >= 4, "the Farm book has {} capstones", capstones.len());
        for capstone in capstones {
            let mut before: Vec<&str> = Vec::new();
            let mut next: Vec<&str> = capstone.after.iter().filter(|a| !a.contains('|')).map(String::as_str).collect();
            while let Some(name) = next.pop() {
                if before.contains(&name) {
                    continue;
                }
                before.push(name);
                let quest = content.quests.get(name).unwrap_or_else(|| panic!("{} comes after {name}, which is no quest", capstone.name));
                next.extend(quest.after.iter().filter(|a| !a.contains('|')).map(String::as_str));
            }
            assert!(!before.contains(&capstone.name.as_str()), "{} comes after itself", capstone.name);
            let skipped: Vec<&str> = farm
                .iter()
                .filter(|q| q.branch == capstone.branch && q.name != capstone.name && !q.side && q.opens.is_none() && !before.contains(&q.name.as_str()))
                .map(|q| q.name.as_str())
                .collect();
            assert!(skipped.is_empty(), "{} (the {} path's capstone) does not come after {skipped:?}: chain them in, or mark them `side`", capstone.name, capstone.branch);
        }
    }

    /// The Farm book's kiln quests ask for what the Crucible Kiln takes: A Full Charge for
    /// one of its fuels in full, The Kiln Ring for every kiln brick of it but the crucible's.
    #[test]
    fn the_kiln_quests_ask_for_the_kiln_as_it_stands() {
        use crate::quest::Goal;
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let blocks = &content.blocks;
        let kiln = content.multiblocks.get("base:crucible_kiln").expect("the Crucible Kiln");
        let fuels: Vec<(String, u32)> = kiln.fuels.iter().map(|&(item, n)| (blocks.def(item).name.clone(), u32::from(n))).collect();
        let charge = content.quests.get("base:the_kiln_charge").expect("A Full Charge");
        let asked: Vec<(String, u32)> = charge
            .tasks
            .iter()
            .map(|task| match task.as_slice() {
                [Goal::Have(item, n)] => (item.clone(), *n),
                other => panic!("A Full Charge asks for one fuel held a way, not {other:?}"),
            })
            .collect();
        assert_eq!(asked, fuels, "A Full Charge asks for the kiln's fuels");
        let bricks = blocks.expect("base:tempered_kiln_bricks");
        let ring: u32 = kiln.pieces().iter().filter(|(takes, _)| *takes == [bricks]).map(|(_, n)| n).sum();
        assert!(ring > 0, "the kiln's ring takes tempered kiln bricks");
        let placed: Vec<u32> = content.quests.get("base:the_kiln_ring").expect("The Kiln Ring").tasks.iter().flatten().filter_map(|g| match g {
            Goal::Placed(item, n) if item == "base:tempered_kiln_bricks" => Some(*n),
            _ => None,
        }).collect();
        assert_eq!(placed, [ring], "The Kiln Ring sets down the kiln's {ring} bricks");
    }

    /// The walk's `Felt` goals are judged by the server's own rule (`can_feel`): each
    /// effect, by its name, with what brings it on and with one thing short.
    #[test]
    fn a_feeling_is_had_only_with_what_brings_it_on() {
        use crate::effect::Means;
        let content = Content::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let blocks = &content.blocks;
        // An item by name, or the first and second carrying a tag (`tag:light`, `tag:light#2`).
        let item = |name: &str| match name.strip_prefix("tag:") {
            Some(tag) => {
                let (tag, nth) = tag.split_once('#').map_or((tag, 1), |(t, n)| (t, n.parse().unwrap()));
                blocks.tagged(tag).nth(nth - 1).unwrap_or_else(|| panic!("no block tagged {tag}"))
            }
            None => blocks.expect(&format!("base:{name}")),
        };
        let feel = |effect: &str, have: &[(&str, u32)], home: bool| {
            let items: Vec<(BlockId, u32)> = have.iter().map(|&(name, n)| (item(name), n)).collect();
            let id = content.effects.find(effect).unwrap_or_else(|| panic!("no effect {effect}"));
            content.effects.can_feel(id, &Means { items: &items, home, done: &[], worn: &content.worn_slots }, blocks)
        };
        let fire = ("campfire", 1);
        assert!(feel("fireside", &[("tag:seat", 1), fire], false), "fireside: a stool at a campfire");
        assert!(!feel("fireside", &[fire], false), "fireside: no seat");
        assert!(!feel("fireside", &[("tag:seat", 1)], false), "fireside: no fire");
        assert!(feel("hearty", &[("cooked_small_game", 1)], false), "hearty: cooked meat eaten");
        assert!(!feel("hearty", &[("raw_small_game", 1)], false), "hearty: raw meat");
        assert!(feel("smoky", &[("torch", 1)], false), "smoky: a torch in hand");
        assert!(feel("tusk_stride", &[("charge_charm", 1)], false), "tusk_stride: the charm worn");
        let unworn = vec![None; content.worn_slots.len()];
        let charm = [(item("charge_charm"), 1)];
        let id = content.effects.find("tusk_stride").unwrap();
        assert!(!content.effects.can_feel(id, &Means { items: &charm, home: false, done: &[], worn: &unworn }, blocks), "tusk_stride: a charm the content wears nowhere");
        assert!(feel("evening_in", &[fire], true), "evening_in: home, with its campfire");
        assert!(!feel("evening_in", &[fire], false), "evening_in: no home");
        // Snug wants the room walled: nine faces of a two-cell room, and two lights (of
        // two kinds, counted together).
        let snug = |walls: u32| [("tag:hearth", 1), ("tag:bed", 1), ("tag:light", 1), ("tag:light#2", 1), ("cobblestone", walls)];
        assert!(feel("snug", &snug(9), true), "snug: walled and roofed in cobblestone");
        assert!(!feel("snug", &snug(3), true), "snug: three cobblestone leave the room open");
        assert!(!feel("snug", &[("tag:hearth", 1), ("tag:bed", 1), ("tag:light", 1), ("cobblestone", 9)], true), "snug: one light");
    }

    /// The main quest line, walked in order, reaches every level it asks for from what it
    /// guarantees: quest rewards and the things it has the player make, all read from the
    /// content (each quest's crafts, cooks and ores; the pieces of the multiblocks it
    /// builds, its home among them; what each recipe uses up, made, mined or gathered
    /// first; and the stations they need, the crafting table and kilns among them; and the
    /// effects it asks to be felt, by the server's rule, with what it has made by then). Where
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
                // What is to be felt, once all else the task has made is to hand.
                for goal in task.iter().filter(|g| !matches!(g, Goal::Felt(_))).chain(task.iter().filter(|g| matches!(g, Goal::Felt(_)))) {
                    match goal {
                        Goal::CookAt(item, station, n) => {
                            // Model an actual successful food recipe at the promised station,
                            // including its ingredients, skill and crafted station. Availability
                            // in the world and burn losses still belong to fresh-survival proof.
                            let matches = |id: BlockId| item == "any" || item.strip_prefix("tag:").map_or_else(
                                || content.blocks.def(id).name.rsplit(':').next() == Some(item.as_str()) || content.blocks.def(id).name == *item,
                                |tag| content.blocks.def(id).has_tag(tag));
                            let id = content.cooking.iter().map(|(_, r)| r).filter(|r| r.def.station == *station && matches(r.output))
                                .min_by_key(|r| r.def.level).unwrap_or_else(|| panic!("{}: no recipe for {goal:?}", quest.name)).output;
                            route.produce(id, *n, Way::CookedAt(station));
                        }
                        Goal::Lit(name, _) => {
                            let name = if name.contains(':') { name.clone() } else { format!("base:{name}") };
                            route.build(&name);
                            let fuels = &content.multiblocks.get(&name).expect("built").fuels;
                            if !fuels.is_empty() {
                                let mut fuel_routes: Option<Walk> = None;
                                for &(fuel, count) in fuels {
                                    let mut firing = route.clone();
                                    firing.consume(&[fuel], count as u32, &name);
                                    assert_eq!(firing.walls.len(), route.walls.len(), "{}: fuel route {} is blocked: {:?}", quest.name, content.blocks.def(fuel).name, firing.walls);
                                    fuel_routes = Some(match fuel_routes { Some(before) => before.least(firing), None => firing });
                                }
                                route = fuel_routes.expect("fuel routes");
                            }
                        }
                        Goal::Craft(item, _) | Goal::Cook(item, _) if item == "any" => {}
                        // Held, of a tag: gathered if any of it is (logs), else the easiest made.
                        Goal::Have(item, n) if let Some(tag) = item.strip_prefix("tag:") => {
                            let accepts: Vec<BlockId> = content.blocks.tagged(tag).collect();
                            assert!(!accepts.is_empty(), "{}: nothing is tagged `{tag}`", quest.name);
                            let short = n.saturating_sub(accepts.iter().map(|b| route.stock.get(b).copied().unwrap_or(0)).sum());
                            route.consume(&accepts, short, &format!("holding {item}"));
                        }
                        Goal::Craft(item, _) | Goal::Cook(item, _) | Goal::Have(item, _) | Goal::Placed(item, _) if item.starts_with("tag:") => panic!("{}: the walk doesn't model {goal:?} yet", quest.name),
                        // Set down: made, in all, at least that many (what is set down is not used
                        // up, so a ring of bricks set down still builds the kiln after).
                        Goal::Placed(item, n) => {
                            let id = route.item(item);
                            let short = n.saturating_sub(route.made.get(&id).copied().unwrap_or(0));
                            route.produce(id, short, Way::Either);
                        }
                        Goal::Craft(item, n) | Goal::Cook(item, n) => {
                            let id = route.item(item);
                            let short = n.saturating_sub(route.made.get(&id).copied().unwrap_or(0));
                            route.produce(id, short, if matches!(goal, Goal::Craft(..)) { Way::Grid } else { Way::Cooked });
                        }
                        Goal::Have(item, n) => {
                            let id = route.item(item);
                            let short = n.saturating_sub(route.stock.get(&id).copied().unwrap_or(0));
                            if route.recipe(id, Way::Either).is_some() {
                                route.produce(id, short, Way::Either);
                            } else {
                                // Mined (an ore held whole) or gathered: what more it takes now.
                                route.consume(&[id], short, &format!("holding {item}"));
                            }
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
                            // A ladder step (a home's) is earned by what stands around it, not built.
                            if content.multiblocks.get(&name).is_some() {
                                route.build(&name);
                            }
                        }
                        // Put by in a store: the store made, and what goes in it used up.
                        Goal::Stocked(item, store, n) if !item.starts_with("tag:") => {
                            // A store nothing makes is no way through (a barrel is found, not made).
                            let store = route.item(store);
                            if route.recipe(store, Way::Either).is_none() {
                                route.walls.push(format!("{}: nothing makes a {}", quest.name, content.blocks.def(store).name));
                            } else if route.made.get(&store).copied().unwrap_or(0) == 0 {
                                route.produce(store, 1, Way::Either);
                            }
                            let id = route.item(item);
                            route.consume(&[id], *n, &format!("stocking {item}"));
                        }
                        Goal::Felt(name) => {
                            let effect = content.effects.find(name).unwrap_or_else(|| panic!("{}: no effect `{name}`", quest.name));
                            if !route.can_feel(effect, &done) {
                                route.walls.push(format!("{}: nothing the line has made brings on {name}", quest.name));
                            }
                        }
                        Goal::Visit(_) | Goal::LookUp(_) => {}
                        // A rite asks for quests done, on whatever paths: nothing made.
                        Goal::QuestsDone(..) | Goal::Capstones(..) => {}
                        Goal::Catch(..) | Goal::Pick(..) | Goal::Tame(..) | Goal::Lured(..) | Goal::Kill(..) | Goal::Discover(..) | Goal::Kinds(..) | Goal::Thriving(..) | Goal::Met(..) | Goal::Dealt(..) | Goal::Prayed(..) | Goal::Offered(..) | Goal::Opened(..) | Goal::Ate(..) | Goal::Fertilised(..) | Goal::Laid(..) | Goal::PutAway(..) | Goal::Mended(..) | Goal::Watering(..) | Goal::Stocked(..) | Goal::CraftNear(..) | Goal::Trapped(..) => panic!("{}: the walk doesn't model {goal:?} yet", quest.name),
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
