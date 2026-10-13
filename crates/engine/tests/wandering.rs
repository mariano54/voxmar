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
    assert!(content.wandering.len() >= 52, "the first eight batches load");
    for code in ["en", "ja"] {
        let lang = Lang::load(&dir(), code);
        for (_, event) in content.wandering.iter() {
            let mut keys = vec![event.title_key()];
            for meeting in &event.meetings {
                keys.extend((0..meeting.greets).map(|n| event.greeting(meeting, n)));
                keys.extend(meeting.lines.iter().map(|line| event.key(&line.name)));
                keys.extend(meeting.lines.iter().filter(|line| !line.asks.is_empty()).map(|line| event.key(&format!("{}_ask", line.name))));
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
            if event.def.show.is_some() {
                keys.push(event.key("show"));
            }
            if !event.share.is_empty() {
                keys.push(event.key("share"));
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

#[test]
fn hunts_and_fortunes_are_checked_as_they_load() {
    use voxmar_engine::wander::{Rules, WanderRegistry};
    let content = Content::load(&dir()).unwrap();
    let load = |event: &str| {
        let package = WanderRegistry::read("base", &format!("(events: [(name: \"x\", cast: [(species: \"hunter\")], {event})])")).expect("parses");
        WanderRegistry::new(vec![package], Rules::default(), &content.creatures, &content.blocks, &content.skills, &content.effects, &content.cooking)
    };
    let hunt = "script: Hunt(reach: 30.0, strike: 2.0, takes: 1, seconds: 120.0)";
    assert!(load(&format!("{hunt}, quarry: [\"wild_boar\"]")).is_ok());
    assert!(load(hunt).is_err(), "a hunt with no quarry");
    assert!(load(&format!("{hunt}, quarry: [\"villager\"]")).is_err(), "people are no quarry");
    assert!(load(&format!("{hunt}, quarry: [\"wild_boar\"], halt: (near: 5.0, seconds: (2.0, 3.0))")).is_err(), "a hunt does not stop for a player");
    assert!(load("script: Roam(radius: 4.0, pause: (2.0, 3.0), rest: false, seconds: 60.0), quarry: [\"wild_boar\"]").is_err(), "quarry without a hunt");
    assert!(load("script: Roam(radius: 4.0, pause: (2.0, 3.0), rest: false, seconds: 60.0), share: [Heal(2)]").is_err(), "a share without a hunt");
    assert!(load(&format!("{hunt}, quarry: [\"wild_boar\"], share: [Leave]")).is_err(), "a share gives things");
    assert!(load("script: Patrol(radius: 8.0, stops: 3, pause: (1.0, 2.0), laps: 1), needs: (town: Some(5.0))").is_err(), "a town within 5 blocks is no town");
    let fortune = |gives: &str| load(&format!("script: Roam(radius: 4.0, pause: (2.0, 3.0), rest: false, seconds: 60.0), meetings: [(lines: [(name: \"f\", gives: [{gives}])])]"));
    assert!(fortune("OneOf([Point(\"tag:wonder\", 64.0), Town(300.0)])").is_ok());
    assert!(fortune("OneOf([Town(300.0)])").is_err(), "one of one is no choice");
    assert!(fortune("OneOf([Town(300.0), Leave])").is_err(), "a fortune tells or gives, it does not send a party off");
    assert!(fortune("OneOf([Point(\"no_such_block\", 64.0), Town(300.0)])").is_err(), "its choices are checked too");
}
