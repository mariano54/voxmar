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
    assert!(content.wandering.len() >= 100, "the first sixteen batches load");
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
                Script::Lure { .. } | Script::Journey { .. } => keys.push(event.key("arrived")),
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
        WanderRegistry::new(vec![package], Rules::default(), &content.creatures, &content.blocks, &content.skills, &content.effects, &content.cooking, &content.weather)
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

#[test]
fn journeys_weather_prayer_and_the_new_needs_are_checked_as_they_load() {
    use voxmar_engine::wander::{Rules, WanderRegistry};
    let content = Content::load(&dir()).unwrap();
    let load = |event: &str| {
        let package = WanderRegistry::read("base", &format!("(events: [(name: \"x\", cast: [(species: \"trader\")], {event})])")).expect("parses");
        WanderRegistry::new(vec![package], Rules::default(), &content.creatures, &content.blocks, &content.skills, &content.effects, &content.cooking, &content.weather)
    };
    let journey = "script: Journey(reach: 400.0, pace: 0.9, near: 12.0)";
    assert!(load(journey).is_ok(), "a journey with no gift at its end");
    assert!(load(&format!("{journey}, reveal: [Item(\"bread\", (1, 1))]")).is_ok());
    assert!(load("script: Journey(reach: 40.0, pace: 0.9, near: 12.0)").is_err(), "a town 40 blocks off is no journey");
    assert!(load(&format!("{journey}, guard: (near: 5.0, warn: 2.0, calm: 9.0)")).is_err());
    let roam = "script: Roam(radius: 4.0, pause: (2.0, 3.0), rest: false, seconds: 60.0)";
    let line = |gives: &str| load(&format!("{roam}, meetings: [(lines: [(name: \"l\", gives: [{gives}])])]"));
    assert!(line("Weather(\"rain\", 600.0)").is_ok());
    assert!(line("Weather(\"no_such_sky\", 600.0)").is_err(), "weather the sky has");
    assert!(line("Weather(\"rain\", 10.0)").is_err(), "for a while");
    assert!(load(&format!("{roam}, show: (near: 5.0, seconds: 5.0, gives: [Heal(2)]), reveal: [Weather(\"rain\", 600.0)]")).is_err(), "weather only from a line");
    assert!(line("Prayer(30.0)").is_ok());
    assert!(line("Prayer(0.0)").is_err());
    assert!(line("Grow(32.0, 1)").is_ok());
    assert!(line("Grow(32.0, 0)").is_err(), "grows at least a stage");
    assert!(line("Grow(80.0, 1)").is_err(), "a field's worth, not a county");
    assert!(load(&format!("{roam}, needs: (wet: Some(true), near_home: Some(48.0))")).is_ok());
    assert!(load(&format!("{roam}, needs: (near_home: Some(48.0), home: Some(false))")).is_err(), "near a home it has not");
    assert!(load(&format!("{roam}, needs: (near_home: Some(4.0))")).is_err());
}

#[test]
fn bouts_are_checked_as_they_load() {
    use voxmar_engine::wander::{Rules, WanderRegistry};
    let content = Content::load(&dir()).unwrap();
    let load = |species: &str, rest: &str| {
        let event = format!("(name: \"x\", cast: [(species: \"{species}\")], script: Roam(radius: 4.0, pause: (2.0, 3.0), rest: false, seconds: 60.0), {rest})");
        let package = WanderRegistry::read("base", &format!("(events: [{event}])")).expect("parses");
        WanderRegistry::new(vec![package], Rules::default(), &content.creatures, &content.blocks, &content.skills, &content.effects, &content.cooking, &content.weather)
    };
    let lines = |lines: &str| format!("meetings: [(lines: [{lines}])]");
    let spar = r#"(name: "spar", gives: [Spar(0.5, 60.0)])"#;
    assert!(load("guard", &lines(spar)).is_ok());
    assert!(load("trader", &lines(spar)).is_err(), "a speaker who does not fight spars with nobody");
    assert!(load("guard", &lines(r#"(name: "spar", gives: [Spar(0.05, 60.0)])"#)).is_err(), "not to the last breath");
    assert!(load("guard", &lines(r#"(name: "spar", gives: [Spar(0.5, 5.0)])"#)).is_err(), "a bout lasts a while");
    let won = format!(r#"{spar}, (name: "won", needs: [When(Won)]), (name: "lost", needs: [When(Lost)])"#);
    assert!(load("guard", &lines(&won)).is_ok(), "a bout's ends are its stages");
    assert!(load("guard", &lines(r#"(name: "lost", needs: [When(Lost)])"#)).is_err(), "no bout, no end of one");
    assert!(load("guard", &format!("show: (near: 5.0, seconds: 5.0, gives: [Spar(0.5, 60.0)])")).is_err(), "a bout only from a line");
}

#[test]
fn parties_that_answer_other_parties_are_checked_as_they_load() {
    use voxmar_engine::wander::{Rules, WanderRegistry};
    let content = Content::load(&dir()).unwrap();
    let load = |rest: &str| {
        let roam = "cast: [(species: \"villager\")], script: Roam(radius: 4.0, pause: (2.0, 3.0), rest: false, seconds: 60.0)";
        let events = format!("(name: \"poacher\", {roam}), (name: \"x\", {roam}, {rest})");
        let package = WanderRegistry::read("base", &format!("(events: [{events}])")).expect("parses");
        WanderRegistry::new(vec![package], Rules::default(), &content.creatures, &content.blocks, &content.skills, &content.effects, &content.cooking, &content.weather)
    };
    assert!(load("needs: (after: Some((\"poacher\", 160.0)))").is_ok());
    assert!(load("needs: (after: Some((\"nobody\", 160.0)))").is_err(), "after a kind there is");
    assert!(load("needs: (after: Some((\"x\", 160.0)))").is_err(), "not after itself");
    assert!(load("needs: (after: Some((\"poacher\", 900.0)))").is_err(), "within sight of it");
    assert!(load("needs: (met: Some(\"poacher\"))").is_ok());
    assert!(load("needs: (met: Some(\"nobody\"))").is_err(), "a kind there is to have met");
    let line = |gives: &str| load(&format!("meetings: [(lines: [(name: \"l\", gives: [{gives}])])]"));
    assert!(line("Toward(\"poacher\", 160.0)").is_ok());
    assert!(line("OneOf([Toward(\"poacher\", 160.0), Item(\"bread\", (1, 1))])").is_ok());
    assert!(line("Toward(\"nobody\", 160.0)").is_err(), "tells of a kind there is");
    assert!(line("Toward(\"poacher\", 4.0)").is_err());
    assert!(load("needs: (home_is: Some(\"homely\"))").is_ok(), "a home step");
    assert!(load("needs: (home_is: Some(\"nowhere\"))").is_err(), "a step there is");
    assert!(load("needs: (home_is: Some(\"hearty\"))").is_err(), "a step, not a food's buff that holds anywhere");
    let camp = "cast: [(species: \"villager\")], script: Camp(near: 16.0, stay: (60.0, 90.0), mill: 1.0, away: 60.0)";
    let show = |gives: &str| {
        let events = format!("(name: \"poacher\", {camp}), (name: \"x\", {camp}, show: (near: 5.0, seconds: 10.0, gives: [{gives}]))");
        let package = WanderRegistry::read("base", &format!("(events: [{events}])")).expect("parses");
        WanderRegistry::new(vec![package], Rules::default(), &content.creatures, &content.blocks, &content.skills, &content.effects, &content.cooking, &content.weather)
    };
    assert!(show("Item(\"bread\", (1, 1))").is_ok());
    assert!(show("Toward(\"poacher\", 160.0)").is_err(), "a show tells of nobody");
}
