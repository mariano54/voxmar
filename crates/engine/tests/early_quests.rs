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
                Goal::Built(name, _) | Goal::Lit(name, _) => qualified(package, name).iter().any(|name| content.multiblocks.get(name).is_some()),
                Goal::Level(name, _) => skill(name),
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
