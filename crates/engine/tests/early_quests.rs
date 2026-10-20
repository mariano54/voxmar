//! The first two ages hold enough to do, and everything they name exists. Thirty quests an
//! age is the floor (Mariano, Oct 13); a goal, reward or `after` that names nothing real is
//! a quest nobody can finish, and nothing else says so before a player gets stuck.

use voxmar_engine::content::Content;
use voxmar_engine::quest::{Goal, Reward};

const FLOOR: usize = 30;

fn content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

fn package(name: &str) -> &str {
    name.split_once(':').map_or("base", |(package, _)| package)
}

fn qualified(package: &str, name: &str) -> [String; 2] {
    [if name.contains(':') { name.to_owned() } else { format!("{package}:{name}") }, name.to_owned()]
}

#[test]
fn nomad_and_farm_each_have_thirty_quests_with_words() {
    let content = content();
    for age in ["nomad", "farm"] {
        let quests: Vec<_> = content.quests.iter().filter(|q| q.age == age).collect();
        assert!(quests.len() >= FLOOR, "{age} has {} quests, the floor is {FLOOR}", quests.len());
        for quest in quests {
            assert!(!quest.title.is_empty() && !quest.why.is_empty(), "{} has no words", quest.name);
            assert!(!quest.tasks.is_empty() && quest.tasks.iter().all(|t| !t.is_empty()), "{} has no goal", quest.name);
            assert!(!quest.rewards.is_empty(), "{} gives nothing", quest.name);
        }
    }
}

/// The Farm chapter opens with its rite, after the lit hearth: a Farm quest comes after Farm
/// quests or the rite, never after a Nomad side quest a player may have passed by.
#[test]
fn farm_quests_come_after_farm_quests_or_the_rite() {
    let content = content();
    let rite = content.quests.rite("farm").expect("a rite opens the Farm age");
    assert!(rite.after.iter().any(|a| a == "base:homestead_hearth"), "the Farm age's rite comes after the lit hearth");
    let farm: Vec<&str> = content.quests.iter().filter(|q| q.age == "farm").map(|q| q.name.as_str()).collect();
    let wrong: Vec<String> = content
        .quests
        .iter()
        .filter(|q| q.age == "farm")
        .flat_map(|q| q.after.iter().flat_map(|a| a.split('|')).filter(|a| *a != rite.name && !farm.contains(a)).map(move |a| format!("{} after {a}", q.name)))
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A Farm quest that could be finished before the Farm age's rite would grant the rite
/// to a new player (an old save's way in), so the content refuses to load one: Clay Banks
/// with its `after` dropped is refused by name.
#[test]
fn a_farm_quest_that_skips_the_rite_is_refused() {
    use voxmar_engine::quest::QuestRegistry;
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base");
    let source = std::fs::read_to_string(base.join("quests.ron")).unwrap();
    // The Nomad book, where the rite is.
    let nomad = || QuestRegistry::read_named("base", "quests_nomad.ron", &std::fs::read_to_string(base.join("quests_nomad.ron")).unwrap()).unwrap();
    assert!(QuestRegistry::new(vec![QuestRegistry::read("base", &source).unwrap(), nomad()]).is_ok(), "the quests as they ship load");
    let at = source.find(r#"name: "clay_banks""#).expect("Clay Banks");
    let after = at + source[at..].find(r#"after: ["the_farm_age"],"#).expect("Clay Banks follows the rite");
    let skipped = format!("{}after: [],{}", &source[..after], &source[after + r#"after: ["the_farm_age"],"#.len()..]);
    let refused = QuestRegistry::new(vec![QuestRegistry::read("base", &skipped).unwrap(), nomad()]).err().expect("Clay Banks without its rite loads");
    assert!(refused.0.contains("base:clay_banks") && refused.0.contains("base:the_farm_age"), "{}", refused.0);
}

#[test]
fn every_early_quest_names_things_the_content_has() {
    let content = content();
    let mut wrong = Vec::new();
    for quest in content.quests.iter() {
        let package = package(&quest.name);
        let item = |name: &str| {
            name == "any"
                || name.strip_prefix("tag:").is_some_and(|tag| content.blocks.tagged(tag).next().is_some())
                || qualified(package, name).iter().any(|name| content.blocks.item_named(name).is_some())
        };
        let skill = |name: &str| {
            let local = name.rsplit(':').next().unwrap_or(name);
            content.skills.ids().any(|id| content.skills.get(id).name.rsplit(':').next() == Some(local))
        };
        for goal in quest.tasks.iter().flatten() {
            let known = match goal {
                Goal::Craft(name, _) | Goal::Cook(name, _) | Goal::CookAt(name, _, _) | Goal::Have(name, _) | Goal::Pick(name, _) | Goal::Mine(name, _) | Goal::LookUp(name) => item(name),
                Goal::Catch(name, _) => name == "Kinds" || item(name),
                Goal::Kinds(_, name, _) => item(name) || qualified(package, name).iter().any(|name| content.creatures.find(name).is_some()),
                Goal::Kill(name, _) | Goal::Tame(name, _) => name == "any" || qualified(package, name).iter().any(|name| content.creatures.find(name).is_some()),
                Goal::Built(name, _) => qualified(package, name).iter().any(|name| content.multiblocks.get(name).is_some() || content.effects.steps().contains(name)),
                Goal::Lit(name, _) => qualified(package, name).iter().any(|name| content.multiblocks.get(name).is_some()),
                Goal::Level(name, _) => skill(name),
                Goal::Met(name, _) => name == "Kinds" || name == "any" || content.wandering.find(name).is_some(),
                Goal::Prayed(name, _) => qualified(package, name).iter().any(|name| content.prayers.id(name).is_some()),
                Goal::Offered(name, _) | Goal::Opened(name, _) | Goal::Ate(name, _) | Goal::Fertilised(name, _) => item(name),
                Goal::Felt(name) => qualified(package, name).iter().any(|name| content.effects.find(name).is_some()),
                _ => true,
            };
            if !known {
                wrong.push(format!("{}: {goal:?}", quest.name));
            }
        }
        for reward in &quest.rewards {
            let known = match reward {
                Reward::Xp(name, _) => skill(name),
                Reward::Item(name, _) => item(name),
            };
            if !known {
                wrong.push(format!("{}: {reward:?}", quest.name));
            }
        }
        for after in quest.after.iter().flat_map(|a| a.split('|')) {
            if content.quests.get(after).is_none() && content.quests.get(&format!("{package}:{after}")).is_none() {
                wrong.push(format!("{}: after {after}", quest.name));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Where an item comes from, for the reach check below: the most any one skill must be
/// at along the cheapest way there, and that skill.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Way {
    level: u8,
    skill: Option<voxmar_engine::skill::SkillId>,
}

/// Every item a player can come by, and the cheapest way: what breaking the land gives
/// (any block no recipe makes), what crops, catches, animals, bosses and bees give, and what crafting
/// and cooking make from those at a station that can itself be come by.
fn reachable(content: &Content) -> rustc_hash::FxHashMap<voxmar_engine::block::BlockId, Way> {
    use voxmar_engine::block::BlockId;
    let blocks = &content.blocks;
    let mut made = rustc_hash::FxHashSet::default();
    for recipe in content.recipes.iter() {
        made.insert(blocks.item_of(recipe.output));
    }
    for (_, recipe) in content.cooking.iter() {
        made.insert(blocks.item_of(recipe.output));
    }
    let mut ways: rustc_hash::FxHashMap<BlockId, Way> = rustc_hash::FxHashMap::default();
    let free = Way { level: 0, skill: None };
    let give = |ways: &mut rustc_hash::FxHashMap<BlockId, Way>, item: BlockId, way: Way| -> bool {
        let item = blocks.item_of(item);
        match ways.get(&item) {
            Some(had) if had.level <= way.level => false,
            _ => {
                ways.insert(item, way);
                true
            }
        }
    };
    // A home station's loaded blocks come only from loading the station, never from the land.
    let loaded = |block: BlockId| content.stations.of(block).is_some_and(|(_, phase)| phase != voxmar_engine::station::Phase::Empty);
    // Blocks the land gives though something also makes them, by their `natural` tag alone.
    let mut natural_and_made = Vec::new();
    for block in blocks.ids() {
        let def = blocks.def(block);
        // Natural ground is the land's even where something also makes it (stone, which a
        // kiln fires from cobblestone): breaking it is a way to what it drops.
        let made_too = made.contains(&blocks.item_of(block));
        if !def.item && (!made_too || def.has_tag("natural")) && blocks.info(block).drops != BlockId(0) && !loaded(block) {
            if made_too {
                natural_and_made.push(def.name.clone());
            }
            give(&mut ways, blocks.info(block).drops, free);
        }
    }
    natural_and_made.sort();
    assert_eq!(
        natural_and_made,
        ["base:red_sandstone", "base:sandstone", "base:stone"],
        "a block a recipe makes counts as the land's by its `natural` tag: add one here only if worldgen places it"
    );
    for (_, crop) in content.crops.iter() {
        for (item, _) in crop.harvest.iter().chain(&crop.forage) {
            give(&mut ways, *item, free);
        }
    }
    for catch in content.fishing.iter() {
        give(&mut ways, catch.item, free);
    }
    for (_, creature) in content.creatures.iter() {
        // Party creatures come only with their wandering party; a boss waits in its lairs
        // (lairs.ron), so its loot is the fight's.
        if creature.def.habitat.party_only && !creature.def.boss {
            continue;
        }
        for drop in &creature.loot {
            give(&mut ways, drop.item, free);
        }
        for &item in &creature.products {
            give(&mut ways, item, free);
        }
    }
    // What bees give (bees.ron): a swarm into the skep in hand, honey and wax from a full
    // one, at the level each take asks.
    for take in &content.bees.takes {
        let way = Way { level: take.level, skill: Some(take.skill) };
        for item in take.gives.iter().map(|(item, _)| *item).chain(take.held_becomes) {
            give(&mut ways, item, way);
        }
    }
    let worst = |a: Way, b: Way| if b.level > a.level { b } else { a };
    loop {
        let mut grew = false;
        for recipe in content.recipes.iter() {
            let mut way = Some(Way { level: recipe.level, skill: recipe.skill });
            for cell in recipe.alternatives().into_iter().flatten() {
                let best = cell.iter().filter_map(|b| ways.get(&blocks.item_of(*b)).copied()).min_by_key(|w| w.level);
                way = match (way, best) {
                    (Some(way), Some(best)) => Some(worst(way, best)),
                    _ => None,
                };
            }
            if let Some(way) = way {
                grew |= give(&mut ways, recipe.output, way);
            }
        }
        for (input, recipe) in content.cooking.iter() {
            let station = blocks.tagged(&recipe.def.station).filter_map(|b| ways.get(&blocks.item_of(b)).copied()).min_by_key(|w| w.level);
            // Whatever it takes besides what is held must be come by too.
            let with: Option<Vec<Way>> = recipe.with.iter().map(|&(item, _)| ways.get(&blocks.item_of(item)).copied()).collect();
            if let (Some(input), Some(station), Some(with)) = (ways.get(&blocks.item_of(input)).copied(), station, with) {
                let way = with.into_iter().fold(worst(worst(input, station), Way { level: recipe.def.level, skill: recipe.skill }), worst);
                grew |= give(&mut ways, recipe.output, way);
            }
        }
        // What home stations make (stations.ron): from any input they work, once the
        // station itself can be come by.
        for station in &content.stations.stations {
            let Some(&built) = ways.get(&blocks.item_of(station.empty)) else { continue };
            for work in &station.works {
                if let Some(input) = work.accepts.iter().filter_map(|b| ways.get(&blocks.item_of(*b)).copied()).min_by_key(|w| w.level) {
                    let way = worst(worst(input, built), Way { level: work.level, skill: Some(work.skill) });
                    grew |= give(&mut ways, work.output, way);
                }
            }
        }
        if !grew {
            return ways;
        }
    }
}

/// Everything a Nomad or Farm quest asks to be made, cooked, held or offered can be come
/// by from the land and the recipes: a quest for something nothing gives is a dead end.
#[test]
fn every_early_quest_asks_for_what_the_land_and_recipes_give() {
    let content = content();
    let ways = reachable(&content);
    let blocks = &content.blocks;
    let mut wrong = Vec::new();
    let mut table = Vec::new();
    for quest in content.quests.iter().filter(|q| q.age == "nomad" || q.age == "farm") {
        let package = package(&quest.name);
        for goal in quest.tasks.iter().flatten() {
            let name = match goal {
                Goal::Craft(name, _) | Goal::Cook(name, _) | Goal::CookAt(name, _, _) | Goal::Have(name, _) | Goal::Offered(name, _) | Goal::Fertilised(name, _) => name,
                _ => continue,
            };
            let found: Vec<_> = match name.strip_prefix("tag:") {
                Some(tag) => blocks.tagged(tag).filter_map(|b| ways.get(&blocks.item_of(b))).collect(),
                None if name == "any" => continue,
                None => qualified(package, name).iter().filter_map(|n| blocks.item_named(n)).filter_map(|b| ways.get(&b)).collect(),
            };
            match found.into_iter().min_by_key(|w| w.level) {
                Some(way) => table.push(format!("{} {}: {name} at {:?} {}", quest.age, quest.name, way.skill.map(|s| content.skills.get(s).name.clone()), way.level)),
                None => wrong.push(format!("{}: {name}", quest.name)),
            }
        }
    }
    if std::env::var_os("VOXMAR_QUEST_REACH").is_some() {
        println!("{}", table.join("\n"));
    }
    assert!(wrong.is_empty(), "nothing gives {wrong:#?}");
}

/// A quest's words point only at quests open beside it: a title it names in brackets (to
/// spend a wait on, or as the next step) is done already or opens with what this quest
/// needs. Naming a side quest the player may never have opened sends them looking for
/// nothing.
#[test]
fn a_quest_names_only_quests_open_beside_it() {
    use rustc_hash::{FxHashMap, FxHashSet};
    let content = content();
    let quests: FxHashMap<&str, &voxmar_engine::quest::QuestDef> = content.quests.iter().map(|q| (q.name.as_str(), q)).collect();
    let titled: FxHashMap<&str, &str> = content.quests.iter().map(|q| (q.title.as_str(), q.name.as_str())).collect();
    // The quests surely done when `name` opens: all its `after`, and for `a|b` only what
    // both ways share.
    fn done<'a>(name: &'a str, quests: &FxHashMap<&'a str, &'a voxmar_engine::quest::QuestDef>, memo: &mut FxHashMap<&'a str, FxHashSet<&'a str>>) -> FxHashSet<&'a str> {
        if let Some(set) = memo.get(name) {
            return set.clone();
        }
        memo.insert(name, FxHashSet::default());
        let mut set = FxHashSet::default();
        for entry in quests.get(name).map_or(&[][..], |q| &q.after[..]) {
            let mut ways = entry.split('|').filter_map(|a| quests.get_key_value(a).map(|(&a, _)| a)).map(|a| {
                let mut way = done(a, quests, memo);
                way.insert(a);
                way
            });
            if let Some(first) = ways.next() {
                set.extend(ways.fold(first, |shared, way| shared.intersection(&way).copied().collect()));
            }
        }
        memo.insert(name, set.clone());
        set
    }
    let mut memo = FxHashMap::default();
    let mut wrong = Vec::new();
    for quest in content.quests.iter() {
        let before = done(&quest.name, &quests, &mut memo);
        // A bracket holds one title, or several joined by commas, "or" and "and" (which a
        // title such as Coal and Cinders may hold itself).
        let brackets = quest.why.split('(').skip(1).filter_map(|rest| rest.split_once(')')).map(|(inside, _)| inside);
        let mut names = FxHashSet::default();
        for inside in brackets {
            for part in inside.split([',', ';']).flat_map(|part| part.split(" or ")) {
                let part = part.trim().trim_start_matches("and ");
                names.insert(part);
                names.extend(part.split(" and ").map(str::trim));
            }
        }
        for named in names {
            let Some(&other) = titled.get(named) else { continue };
            if other == quest.name || before.contains(other) {
                continue;
            }
            let opens = quests[other].after.iter().all(|entry| entry.split('|').any(|a| a == quest.name || before.contains(a)));
            if !opens {
                wrong.push(format!("{} names {named} ({other}), which needs {:?}", quest.name, quests[other].after));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Hunting gets a felt step before copper: the short bow (twenty blocks of reach, no chase)
/// is wielded at a Ranged level the Nomad quests already ask for, and it and its arrows are
/// made at a Crafting level no higher than the Nomad's own makes ask (the gathering-upgrades
/// audit, Oct 14: the stone sword's hunt ran ten minutes and more before copper arms).
#[test]
fn the_short_bow_comes_within_the_nomad_age() {
    let content = content();
    let skill = |name: &str| content.skills.id(name).unwrap_or_else(|| panic!("no skill {name}"));
    let (ranged, crafting) = (skill("core:ranged"), skill("core:crafting"));
    let nomad: Vec<_> = content.quests.iter().filter(|q| q.age == "nomad").collect();
    let asked = nomad.iter().flat_map(|q| q.tasks.iter().flatten()).filter_map(|goal| match goal {
        Goal::Level(name, n) if content.skills.id(&qualified("core", name)[0]) == Some(ranged) => Some(*n),
        _ => None,
    });
    let ranged_asked = asked.max().expect("a Nomad quest asks for a Ranged level");
    let made: Vec<_> = nomad
        .iter()
        .flat_map(|q| q.tasks.iter().flatten())
        .filter_map(|goal| match goal {
            Goal::Craft(item, _) => content.blocks.id(&qualified("base", item)[0]),
            _ => None,
        })
        .collect();
    let crafting_asked = content.recipes.iter().filter(|r| made.contains(&r.output) && r.skill == Some(crafting)).map(|r| r.level).max().expect("Nomad quests make things");
    let bow = content.blocks.expect("base:short_bow");
    let weapon = content.combat.weapon(bow).expect("the short bow is a weapon");
    assert!(weapon.level <= ranged_asked, "the short bow needs Ranged {}, the Nomad quests reach {ranged_asked}", weapon.level);
    for item in ["base:short_bow", "base:arrow"] {
        let item = content.blocks.expect(item);
        for recipe in content.recipes.iter().filter(|r| r.output == item) {
            assert!(recipe.level <= crafting_asked, "{} takes Crafting {}, the Nomad's makes ask {crafting_asked}", content.blocks.def(item).name, recipe.level);
        }
    }
}
