//! The home ladder (effects.ron): each step asks for everything the one below does, replaces
//! it, and sleeping at a bed in a better home gives a better rest.

use voxmar_engine::content::Content;
use voxmar_engine::effect::{Condition, HomeSurvey, Situation};
use voxmar_engine::weather::Weather;

fn content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

/// What a step asks of a home, as its `HomeHas`es say (a step is an `All` of them).
fn asks(when: &Condition) -> Vec<(String, u32)> {
    match when {
        Condition::All(list) => list.iter().flat_map(asks).collect(),
        Condition::HomeHas(tag, n) => vec![(tag.clone(), *n)],
        _ => Vec::new(),
    }
}

const LADDER: [&str; 6] = ["camp", "hearthside", "snug", "homely", "cosy", "heart_of_the_home"];

#[test]
fn each_step_of_the_home_ladder_asks_for_the_one_below_and_hides_it() {
    let content = content();
    let effects = &content.effects;
    let id = |name: &str| effects.id(&format!("base:{name}")).unwrap_or_else(|| panic!("a `{name}` step"));
    // Snug and up are a climb: each asks for all the last asked, and for more.
    let furnished = &LADDER[2..];
    for pair in furnished.windows(2) {
        let (below, step) = (asks(effects.get(id(pair[0])).when.as_ref().unwrap()), asks(effects.get(id(pair[1])).when.as_ref().unwrap()));
        for (tag, n) in &below {
            assert!(step.iter().any(|(t, m)| t == tag && m >= n), "{} asks for {n} {tag}, which {} must too", pair[0], pair[1]);
        }
        assert!(step.len() > below.len() || step.iter().zip(&below).any(|(a, b)| a.1 > b.1), "{} asks for more than {}", pair[1], pair[0]);
    }
    for (i, name) in LADDER.iter().enumerate() {
        let def = effects.get(id(name));
        assert!(def.announce, "{name} tells the player when they reach it");
        for lower in &LADDER[..i] {
            assert!(effects.hides(id(name), id(lower)), "{name} hides {lower}");
        }
    }
}

#[test]
fn a_home_shows_the_highest_step_it_has_and_a_rest_to_match() {
    let content = content();
    let effects = &content.effects;
    let calm = Weather::NONE;
    let survey_of = |step: &str| {
        let wanted = asks(effects.get(effects.id(&format!("base:{step}")).unwrap()).when.as_ref().unwrap());
        HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), wanted.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect() }
    };
    let top = |survey: &HomeSurvey, outdoors: bool| {
        let situation = Situation { felt: 15.0, warmed: false, outdoors, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(survey) };
        effects.active(&situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).collect::<Vec<_>>()
    };
    let rest = |survey: &HomeSurvey, outdoors: bool| {
        let situation = Situation { felt: 15.0, warmed: false, outdoors, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(survey) };
        effects.given_by_sleep(&situation).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect::<Vec<_>>()
    };
    for step in LADDER {
        assert_eq!(top(&survey_of(step), false), [format!("base:{step}")], "a home furnished for {step} shows {step} alone");
    }
    // Out in the open nothing past the hearth holds.
    assert_eq!(top(&survey_of("heart_of_the_home"), true), ["base:hearthside"]);
    let names = |list: Vec<(String, f32)>| list.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    assert_eq!(names(rest(&survey_of("camp"), false)), ["base:rested"]);
    assert_eq!(names(rest(&survey_of("snug"), false)), ["base:rested", "base:well_rested"]);
    assert_eq!(names(rest(&survey_of("cosy"), false)), ["base:rested", "base:well_rested", "base:deep_rest"]);
    assert_eq!(names(rest(&survey_of("cosy"), true)), ["base:rested"], "a bed under the sky is only a plain rest");
}

#[test]
fn the_next_step_is_above_the_highest_reached_not_one_skipped_below() {
    // A hearth lit with no torch by the bed: Hearthside holds though Camp does not, and
    // `/home` names Snug next, not Camp.
    let content = content();
    let effects = &content.effects;
    let calm = Weather::NONE;
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), u32::from(tag == "hearth"))).collect() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors: true, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&survey) };
    let id = |name: &str| effects.id(&format!("base:{name}")).unwrap();
    let ladder = effects.home_progress(&situation).into_iter().find(|chain| chain.reached == Some(id("hearthside"))).expect("the ladder stands at Hearthside");
    assert_eq!(ladder.next.map(|(next, _)| next), Some(id("snug")));
}
