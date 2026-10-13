//! The wandering events (`wandering.ron`) load whole: every word an event says is a lang key
//! in English and Japanese, and every species that comes only with a party is in some cast.

use std::collections::BTreeSet;
use std::path::Path;

use voxmar_engine::content::Content;
use voxmar_engine::lang::Lang;
use voxmar_engine::wander::Script;

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")
}

#[test]
fn every_event_word_is_a_key_in_both_languages() {
    let content = Content::load(&dir()).unwrap();
    assert!(content.wandering.len() >= 32, "the first five batches load");
    for code in ["en", "ja"] {
        let lang = Lang::load(&dir(), code);
        for (_, event) in content.wandering.iter() {
            let mut keys = vec![event.title_key()];
            for meeting in &event.meetings {
                keys.extend((0..meeting.greets).map(|n| event.greeting(meeting, n)));
                keys.extend(meeting.lines.iter().map(|line| event.key(&line.name)));
            }
            if event.has_captive() {
                keys.extend([event.key("thanks"), event.key("freed")]);
            }
            if event.def.guide.is_some() {
                keys.extend([event.key("arrived"), event.key("company")]);
            }
            if event.def.guard.is_some() {
                keys.push(event.key("warn"));
            }
            match event.def.script {
                Script::Ambush { .. } => keys.push(event.key("sprung")),
                Script::Lure { .. } => keys.push(event.key("arrived")),
                _ => {}
            }
            for key in keys {
                assert!(lang.find(&key).is_some(), "{code}: `{key}` is missing for {}", event.def.name);
            }
        }
    }
}

#[test]
fn party_only_species_come_with_some_party() {
    let content = Content::load(&dir()).unwrap();
    let cast: BTreeSet<_> = content.wandering.iter().flat_map(|(_, e)| e.groups.iter().map(|(species, _)| *species)).collect();
    for (id, creature) in content.creatures.iter() {
        let def = &creature.def;
        if def.habitat.party_only {
            assert!(cast.contains(&id), "`{}` only comes with a wandering party, and no event casts it", def.name);
        }
    }
}

#[test]
fn events_have_a_leader_and_every_pet_an_owner() {
    use voxmar_engine::wander::Role;
    let content = Content::load(&dir()).unwrap();
    for (_, event) in content.wandering.iter() {
        assert!(!event.groups.is_empty(), "{} has a cast", event.def.name);
        for (_, group) in &event.groups {
            if let Role::Pet(owner) = group.role {
                assert!(owner < event.groups.len(), "{}: a pet follows a group that exists", event.def.name);
            }
        }
    }
}
