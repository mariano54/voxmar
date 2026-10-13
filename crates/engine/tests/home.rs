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
        // The ladder's own step alone: rooms (below) come on beside it.
        effects.active(&situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).filter(|name| LADDER.iter().any(|step| *name == format!("base:{step}"))).collect::<Vec<_>>()
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

/// What a survey with these counts shows of the rooms (the side branches) in this weather.
fn rooms_in(counts: &[(&str, u32)], weather: &Weather, outdoors: bool) -> Vec<String> {
    let content = content();
    let effects = &content.effects;
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors, weather, biome: "base:plains", biome_tags: &[], home: Some(&survey) };
    effects.active(&situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).filter(|name| !LADDER.iter().any(|step| *name == format!("base:{step}"))).collect()
}

/// The same, indoors in fair weather.
fn rooms(counts: &[(&str, u32)]) -> Vec<String> {
    rooms_in(counts, &Weather::NONE, false)
}

#[test]
fn rooms_come_on_beside_the_ladder_and_the_better_one_hides_the_other() {
    assert!(rooms(&[("bed", 1)]).is_empty());
    assert_eq!(rooms(&[("crafting_table", 1), ("seat", 1), ("light", 1)]), ["base:workbench"]);
    let workshop = [("crafting_table", 1), ("seat", 1), ("light", 2), ("workshop", 1), ("storage", 2)];
    assert_eq!(rooms(&workshop), ["base:workshop"], "the workshop hides the workbench; no fire, no pantry");
    assert_eq!(rooms(&[("storage", 2), ("heat", 1)]), ["base:pantry"]);
    assert_eq!(rooms(&[("storage", 2), ("hearth", 1)]), ["base:pantry"], "a hearth is a fire too");
    let larder = [("storage", 4), ("cooking", 1), ("table", 1), ("hearth", 1)];
    assert_eq!(rooms(&larder), ["base:larder"], "the larder hides the pantry");
    // Rooms stack: a workshop with a larder shows both.
    let both = [("crafting_table", 1), ("seat", 1), ("light", 2), ("workshop", 1), ("storage", 4), ("cooking", 1), ("table", 1), ("hearth", 1)];
    assert_eq!(rooms(&both), ["base:workshop", "base:larder"]);
}

#[test]
fn rooms_lift_levels_and_meals_by_what_content_says() {
    let content = content();
    let effects = &content.effects;
    let get = |name: &str| effects.get(effects.id(&format!("base:{name}")).unwrap());
    let skills = |name: &str| get(name).boosts.iter().map(|(s, n)| (s.as_str(), *n)).collect::<Vec<_>>();
    assert_eq!(skills("workbench"), [("core:crafting", 1), ("core:construction", 1)]);
    assert_eq!(skills("workshop"), [("core:crafting", 2), ("core:construction", 2), ("core:engineering", 1)]);
    assert_eq!((get("pantry").modifiers.meal_time, get("pantry").modifiers.meal_heal), (1.25, 1.0));
    assert_eq!((get("larder").modifiers.meal_time, get("larder").modifiers.meal_heal), (1.5, 1.2));
}

#[test]
fn a_workshop_boosts_the_levels_recipes_are_checked_against() {
    let content = content();
    let effects = &content.effects;
    let calm = Weather::NONE;
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), 4)).collect() };
    let at_home = Situation { felt: 15.0, warmed: false, outdoors: false, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&survey) };
    let active = effects.active(&at_home, &[]);
    assert_eq!((effects.boost(&active, "core:crafting"), effects.boost(&active, "core:engineering")), (2, 1));
    let away = Situation { home: None, ..at_home };
    assert_eq!(effects.boost(&effects.active(&away, &[]), "core:crafting"), 0, "away from home there is no boost");
}

#[test]
fn a_home_with_a_fire_shelters_from_the_weather_outside() {
    let fire = [("hearth", 1)];
    let sheltered = "base:sheltered".to_string();
    let storm = Weather { precipitation: 0.9, ..Weather::NONE };
    let snow = Weather { precipitation: 0.9, snow: 1.0, ..Weather::NONE };
    let dust = Weather { dust: 0.8, ..Weather::NONE };
    let lone = |weather: &Weather, outdoors, counts: &[(&str, u32)]| rooms_in(counts, weather, outdoors);
    for weather in [&storm, &snow, &dust] {
        assert!(lone(weather, false, &fire).contains(&sheltered), "rain, snow and dust all count from indoors");
        assert!(!lone(weather, true, &fire).contains(&sheltered), "out in it you are not sheltered");
        assert!(!lone(weather, false, &[]).contains(&sheltered), "a roof with no fire is just a roof");
    }
    assert!(!lone(&Weather::NONE, false, &fire).contains(&sheltered), "fair weather asks for no shelter");
    assert!(lone(&storm, false, &[("heat", 1)]).contains(&sheltered), "any heat block is a fire");
}

#[test]
fn animals_kept_about_the_house_are_company_and_more_a_full_house() {
    assert!(rooms(&[("kept", 0)]).is_empty());
    assert_eq!(rooms(&[("kept", 1)]), ["base:company"]);
    assert_eq!(rooms(&[("kept", 3)]), ["base:company"]);
    assert_eq!(rooms(&[("kept", 4)]), ["base:full_house"], "a full house hides company");
    // Company needs no roof: a yard is a yard.
    assert_eq!(rooms_in(&[("kept", 1)], &Weather::NONE, true), ["base:company"]);
}

#[test]
fn crops_by_the_door_are_a_kitchen_garden_that_makes_meals_heal_more() {
    assert!(rooms(&[("crop", 3)]).is_empty());
    assert_eq!(rooms(&[("crop", 4)]), ["base:kitchen_garden"]);
    let content = content();
    let garden = content.effects.get(content.effects.id("base:kitchen_garden").unwrap());
    assert_eq!(garden.modifiers.meal_heal, 1.1);
    // It stacks with a larder: both lift a meal's healing.
    let larder = [("storage", 4), ("cooking", 1), ("table", 1), ("hearth", 1), ("crop", 4)];
    assert_eq!(rooms(&larder), ["base:larder", "base:kitchen_garden"]);

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
