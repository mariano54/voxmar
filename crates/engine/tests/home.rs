//! The home ladder (effects.ron): each step asks for everything the one below does, replaces
//! it, and sleeping at a bed in a better home gives a better rest.

use voxmar_engine::content::Content;
use voxmar_engine::effect::{Condition, EffectRegistry, HomeSurvey, Lack, Shelter, Situation, Walls};
use voxmar_engine::weather::Weather;

/// A sleeping bag's tags: the plainest bed.
fn bag() -> Vec<String> {
    vec!["bed".to_owned(), "sleeping_bag".to_owned()]
}

fn content() -> Content {
    Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).unwrap()
}

/// What a step asks of a home's walls, as its `HomeWalls` say.
fn walls_asked(when: &Condition) -> Vec<(Shelter, f32)> {
    match when {
        Condition::All(list) => list.iter().flat_map(walls_asked).collect(),
        Condition::HomeWalls(shelter, share) => vec![(*shelter, *share)],
        _ => Vec::new(),
    }
}

/// The walls a step asks for, met all about: the best shelter it asks, every face of it.
fn walls_for(when: &Condition) -> Walls {
    walls_asked(when).into_iter().map(|(shelter, _)| shelter).max().map_or_else(Walls::default, |best| Walls::all(best, 60))
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
        // And walls at least as good: each the step below asks, this one asks as well or better.
        let (below, step) = (walls_asked(effects.get(id(pair[0])).when.as_ref().unwrap()), walls_asked(effects.get(id(pair[1])).when.as_ref().unwrap()));
        for (shelter, share) in &below {
            assert!(step.iter().any(|(s, n)| s >= shelter && n >= share), "{} asks {share} of {shelter:?} walls, which {} must too", pair[0], pair[1]);
        }
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
        let walls = walls_for(effects.get(effects.id(&format!("base:{step}")).unwrap()).when.as_ref().unwrap());
        HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), wanted.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect(), walls }
    };
    let top = |survey: &HomeSurvey, outdoors: bool| {
        let situation = Situation { felt: 15.0, warmed: false, outdoors, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
        // The ladder's own step alone: rooms (below) come on beside it.
        effects.active(&situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).filter(|name| LADDER.iter().any(|step| *name == format!("base:{step}"))).collect::<Vec<_>>()
    };
    let rest = |survey: &HomeSurvey, outdoors: bool| {
        let situation = Situation { felt: 15.0, warmed: false, outdoors, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: true };
        effects.given_by_sleep(&situation, &bag()).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect::<Vec<_>>()
    };
    for step in LADDER {
        assert_eq!(top(&survey_of(step), false), [format!("base:{step}")], "a home furnished for {step} shows {step} alone");
    }
    // Furnished for a step but walled for the one below, it is the one below: each better
    // wall is a step up, and a warmer one.
    let walled = |step: &str, walls: Walls| top(&HomeSurvey { walls, ..survey_of(step) }, false);
    let mostly_sound = {
        let mut walls = Walls::all(Shelter::Sound, 48);
        (0..12).for_each(|_| walls.add(Some(Shelter::Draughty)));
        walls
    };
    let mut gappy = Walls::all(Shelter::Fine, 50);
    (0..10).for_each(|_| gappy.add(None));
    assert_eq!(walled("snug", Walls::default()), ["base:hearthside"], "no walls, no snug");
    assert_eq!(walled("snug", gappy), ["base:hearthside"], "a sixth of the room open to the outside is not walled in");
    assert_eq!(walled("homely", Walls::all(Shelter::Draughty, 60)), ["base:snug"], "hide and wattle walls are snug, not homely");
    assert_eq!(walled("cosy", mostly_sound), ["base:homely"], "mostly sound walls are homely, not cosy");
    assert_eq!(walled("heart_of_the_home", Walls::all(Shelter::Sound, 60)), ["base:cosy"], "sound walls are cosy; the heart wants fine ones");
    let warmth = |step: &str| effects.get(effects.id(&format!("base:{step}")).unwrap()).modifiers.home_warmth;
    for pair in LADDER[2..].windows(2) {
        assert!(warmth(pair[1]) > warmth(pair[0]), "{} is warmer than {}", pair[1], pair[0]);
    }
    // Out in the open nothing past the hearth holds.
    assert_eq!(top(&survey_of("heart_of_the_home"), true), ["base:hearthside"]);
    let names = |list: Vec<(String, f32)>| list.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    assert_eq!(names(rest(&survey_of("camp"), false)), ["base:healing_sleep", "base:rested"]);
    assert_eq!(names(rest(&survey_of("snug"), false)), ["base:healing_sleep", "base:rested", "base:well_rested"]);
    assert_eq!(names(rest(&survey_of("cosy"), false)), ["base:healing_sleep", "base:rested", "base:well_rested", "base:deep_rest"]);
    assert_eq!(names(rest(&survey_of("camp"), true)), ["base:healing_sleep", "base:rested"], "a bed under the sky is a plain rest");
    assert_eq!(names(rest(&survey_of("cosy"), true)), ["base:healing_sleep", "base:rested", "base:well_rested"], "under the sky, its seats and lights are only a lit corner");
    // By day the same cosy home is only a nap: the better rests are a night's.
    let nap = Situation { felt: 15.0, warmed: false, outdoors: false, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&survey_of("cosy")), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
    assert_eq!(names(effects.given_by_sleep(&nap, &bag()).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect()), ["base:healing_sleep", "base:rested"]);
}

#[test]
fn a_lit_corner_by_the_bed_rests_well_before_any_roof() {
    let content = content();
    let effects = &content.effects;
    let calm = Weather::NONE;
    let survey = |counts: &[(&str, u32)]| HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect(), ..Default::default() };
    let rest = |survey: &HomeSurvey| {
        let situation = Situation { felt: 15.0, warmed: false, outdoors: true, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: true };
        effects.given_by_sleep(&situation, &bag()).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect::<Vec<_>>()
    };
    // A camp out under the sky: a plain rest. A seat and a second light by the bag: a well rested one.
    assert_eq!(rest(&survey(&[("bed", 1), ("light", 1)])), [("base:healing_sleep".to_owned(), 60.0), ("base:rested".to_owned(), 600.0)]);
    let corner = survey(&[("bed", 1), ("seat", 1), ("light", 2)]);
    assert_eq!(rest(&corner), [("base:healing_sleep".to_owned(), 60.0), ("base:rested".to_owned(), 600.0), ("base:well_rested".to_owned(), 900.0)]);
    let noon = Situation { felt: 15.0, warmed: false, outdoors: true, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&corner), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
    let nap = effects.given_by_sleep(&noon, &bag()).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect::<Vec<_>>();
    assert_eq!(nap, [("base:healing_sleep".to_owned(), 60.0), ("base:rested".to_owned(), 600.0)], "a lit corner by day is a nap: the mend and Rested, not Well rested");
    assert_eq!(rooms_in(&[("bed", 1), ("seat", 1), ("light", 2)], &calm, true), ["base:bedside"]);
    assert_eq!(effects.get(effects.id("base:bedside").unwrap()).modifiers.home_warmth, 2.0);
    // Snug and a lit corner too: the one rest, once.
    let snug_corner = HomeSurvey { walls: Walls::all(Shelter::Draughty, 60), ..survey(&[("bed", 1), ("seat", 1), ("light", 2), ("hearth", 1)]) };
    let situation = Situation { felt: 15.0, warmed: false, outdoors: false, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&snug_corner), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: true };
    let given = effects.given_by_sleep(&situation, &bag());
    assert_eq!(given.iter().filter(|(id, _)| effects.get(*id).name == "base:well_rested").count(), 1, "two ways to one rest give it once");
}

#[test]
fn herbs_drying_at_home_make_a_nook_for_the_garden_and_the_brew() {
    let content = content();
    let effects = &content.effects;
    assert!(rooms(&[("drying", 2)]).is_empty(), "nowhere to keep them");
    assert!(rooms(&[("drying", 1), ("storage", 1)]).is_empty(), "one bunch is not a nook");
    assert_eq!(rooms(&[("drying", 2), ("storage", 1)]), ["base:herb_nook"]);
    let nook = effects.get(effects.id("base:herb_nook").unwrap());
    assert_eq!(nook.boosts.iter().map(|(s, n)| (s.as_str(), *n)).collect::<Vec<_>>(), [("core:farming", 1), ("core:alchemy", 1)]);
    assert_eq!(nook.modifiers.meal_time, 1.1);
    // The bunches are craftable, and break to nothing (furnishings drop nothing, d80).
    let herbs = content.blocks.expect("base:hanging_herbs");
    assert!(content.recipes.iter().any(|r| r.output == herbs), "hanging herbs are made");
    assert_eq!(content.blocks.def(herbs).drops.as_deref(), Some("core:air"));
}

/// Every tag a home is surveyed for has its words in every language, so `/home`'s "lacks"
/// line names it in the player's own words, not the tag's English.
#[test]
fn every_home_tag_is_named_in_every_language() {
    let content = content();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base/lang");
    for code in ["en", "ja"] {
        let lang: std::collections::HashMap<String, String> = ron::from_str(&std::fs::read_to_string(dir.join(format!("{code}.ron"))).unwrap()).unwrap();
        let missing: Vec<_> = content.effects.home_tags().iter().map(|tag| format!("home.tag.{tag}")).filter(|key| !lang.contains_key(key)).collect();
        assert!(missing.is_empty(), "{code}: {missing:?}");
    }
}

/// The hearth corner: the homestead hearth under the roof with a settle at it and a woodpile
/// is a hearth corner; any of the three missing, or out under the sky, it is not. Every wood makes
/// the settle and the woodpile, and the settle is sat in as any seat.
#[test]
fn a_settle_and_a_woodpile_by_the_hearth_make_a_hearth_corner() {
    let content = content();
    let effects = &content.effects;
    let corner = [("hearth", 1), ("settle", 1), ("woodpile", 1)];
    assert_eq!(rooms(&corner), ["base:hearth_corner"]);
    for missing in 0..corner.len() {
        let short: Vec<_> = corner.iter().enumerate().filter(|&(i, _)| i != missing).map(|(_, &c)| c).collect();
        assert!(!rooms(&short).contains(&"base:hearth_corner".to_owned()), "no {}, no hearth corner", corner[missing].0);
    }
    assert!(rooms(&[("hearth", 1), ("seat", 1), ("woodpile", 1)]).is_empty(), "a stool is no settle");
    assert!(rooms_in(&corner, &Weather::NONE, true).is_empty(), "no roof, no hearth corner");
    let corner_room = effects.get(effects.id("base:hearth_corner").unwrap());
    assert_eq!((corner_room.modifiers.home_warmth, corner_room.modifiers.heal), (1.0, 1.05));
    for wood in ["birch", "oak", "dark_oak"] {
        let settle = content.blocks.def(content.blocks.expect(&format!("base:{wood}_settle")));
        assert!(settle.has_tag("seat") && settle.has_tag("settle") && !settle.solid, "{wood} settle");
        let pile = content.blocks.expect(&format!("base:{wood}_woodpile"));
        assert!(content.blocks.def(pile).has_tag("woodpile") && content.recipes.iter().any(|r| r.output == pile), "{wood} woodpile");
    }
}

/// The lived-in hearth: under a roof, the hearth with a mantel shelf over it and a fireside
/// bench before it; any of them missing, or out under the sky, it is not one. Every wood
/// makes both pieces, the shelf keeps things and the bench is sat on.
#[test]
fn a_mantel_shelf_and_a_fireside_bench_make_a_lived_in_hearth() {
    let content = content();
    let effects = &content.effects;
    let room = [("hearth", 1), ("mantel_shelf", 1), ("fireside_bench", 1)];
    assert_eq!(rooms(&room), ["base:lived_in_hearth"]);
    for missing in 0..room.len() {
        let short: Vec<_> = room.iter().enumerate().filter(|&(i, _)| i != missing).map(|(_, &c)| c).collect();
        assert!(!rooms(&short).contains(&"base:lived_in_hearth".to_owned()), "no {}, no lived-in hearth", room[missing].0);
    }
    assert!(rooms(&[("hearth", 1), ("mantel_shelf", 1), ("seat", 1)]).is_empty(), "a stool is no fireside bench");
    assert!(rooms_in(&room, &Weather::NONE, true).is_empty(), "no roof, no lived-in hearth");
    let hearth = effects.get(effects.id("base:lived_in_hearth").unwrap());
    assert_eq!((hearth.modifiers.home_warmth, hearth.modifiers.xp), (1.0, 1.05));
    for wood in ["birch", "oak", "dark_oak"] {
        let shelf = content.blocks.expect(&format!("base:{wood}_mantel_shelf"));
        assert!(content.blocks.def(shelf).has_tag("mantel_shelf") && content.recipes.iter().any(|r| r.output == shelf), "{wood} mantel shelf");
        assert!(content.machines.iter().any(|m| m.block == shelf && m.is_store()), "the {wood} mantel shelf keeps things");
        let bench = content.blocks.def(content.blocks.expect(&format!("base:{wood}_fireside_bench")));
        assert!(bench.has_tag("fireside_bench") && bench.has_tag("seat"), "{wood} fireside bench");
    }
}

/// The bedchamber: under a roof, the bed with a blanket chest, a washstand and a rug; any
/// of them missing, or out under the sky, it is not one. Every wood makes both pieces, and
/// the chest is storage.
#[test]
fn a_blanket_chest_and_a_washstand_by_the_bed_make_a_bedchamber() {
    let content = content();
    let effects = &content.effects;
    let room = [("bed", 1), ("blanket_chest", 1), ("washstand", 1), ("rug", 1)];
    assert_eq!(rooms(&room), ["base:bedchamber"]);
    for missing in 0..room.len() {
        let short: Vec<_> = room.iter().enumerate().filter(|&(i, _)| i != missing).map(|(_, &c)| c).collect();
        assert!(!rooms(&short).contains(&"base:bedchamber".to_owned()), "no {}, no bedchamber", room[missing].0);
    }
    assert!(rooms_in(&room, &Weather::NONE, true).is_empty(), "no roof, no bedchamber");
    let chamber = effects.get(effects.id("base:bedchamber").unwrap());
    assert_eq!((chamber.modifiers.home_warmth, chamber.modifiers.heal), (1.0, 1.1));
    for wood in ["birch", "oak", "dark_oak"] {
        let chest = content.blocks.expect(&format!("base:{wood}_blanket_chest"));
        assert!(content.blocks.def(chest).has_tag("storage") && content.recipes.iter().any(|r| r.output == chest), "{wood} blanket chest");
        assert!(content.machines.iter().any(|m| m.block == chest && m.is_store()), "the {wood} blanket chest opens to keep things in");
        let stand = content.blocks.expect(&format!("base:{wood}_washstand"));
        assert!(content.blocks.def(stand).has_tag("washstand") && content.recipes.iter().any(|r| r.output == stand), "{wood} washstand");
    }
}

/// Windows count at home: two in its walls under a roof make a bright room; one does not,
/// nor two out under the sky. Every window the content has is one (`window`).
#[test]
fn two_windows_at_home_make_a_bright_room() {
    let content = content();
    let effects = &content.effects;
    assert!(rooms(&[("window", 1)]).is_empty(), "one window is not a bright room");
    assert_eq!(rooms(&[("window", 2)]), ["base:bright_room"]);
    assert!(rooms_in(&[("window", 2)], &Weather::NONE, true).is_empty(), "no roof, no room");
    let bright = effects.get(effects.id("base:bright_room").unwrap());
    assert_eq!((bright.modifiers.xp, bright.modifiers.heal), (1.05, 1.05));
    for name in ["base:hide_window", "base:glass"] {
        assert!(content.blocks.def(content.blocks.expect(name)).has_tag("window"), "{name} is a window");
    }
}

/// Things set out in the yard count at home: an awning and two yard pieces out under the
/// sky make a dooryard; fewer do not, nor no awning, nor the same under the roof. The yard
/// pieces are a bench, bunting and a hay bale (`yard`), counted only in the open
/// (`outdoors`); an awning is not one, nor anything sat on indoors only.
#[test]
fn an_awning_and_yard_pieces_out_of_doors_make_a_dooryard() {
    let content = content();
    let effects = &content.effects;
    let outside = |counts: &[(&str, u32)]| rooms_in(counts, &Weather::NONE, true);
    assert_eq!(outside(&[("awning", 1), ("yard", 2)]), ["base:dooryard"]);
    assert!(outside(&[("awning", 1), ("yard", 1)]).is_empty(), "an awning and one more is not a dooryard");
    assert!(outside(&[("yard", 3)]).is_empty(), "no awning, no dooryard");
    assert!(rooms(&[("awning", 1), ("yard", 2)]).is_empty(), "under the roof is not the yard");
    let yard = effects.get(effects.id("base:dooryard").unwrap());
    assert_eq!((yard.modifiers.heal, yard.modifiers.xp), (1.05, 1.0), "one small multiplier");
    assert!(effects.outdoor("yard") && !effects.outdoor("awning"), "yard pieces count only in the open");
    for name in ["base:oak_bench", "base:red_bunting", "base:hay_bale"] {
        assert!(content.blocks.def(content.blocks.expect(name)).has_tag("yard"), "{name} is a yard piece");
    }
    for name in ["base:red_awning", "base:oak_chair", "base:red_rug"] {
        assert!(!content.blocks.def(content.blocks.expect(name)).has_tag("yard"), "{name} is not a yard piece");
    }
    // Indoors, `/home` says to step out.
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), u32::from(tag == "awning") + 2 * u32::from(tag == "yard"))).collect(), ..Default::default() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors: false, weather: &Weather::NONE, biome: "base:plains", biome_tags: &[], home: Some(&survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
    assert_eq!(yard.when.as_ref().unwrap().lacks(&situation), [Lack::Sky]);
    // `/home` judges it from the roofed home as if the player stood in the yard: reached.
    let dooryard = effects.id("base:dooryard").unwrap();
    assert!(effects.home_progress(&situation).iter().any(|chain| chain.reached == Some(dooryard)), "a finished dooryard is never reached from indoors");
}

/// A tag counted only outdoors must be one a home is asked for.
#[test]
fn an_outdoor_tag_no_home_asks_is_refused() {
    let file = EffectRegistry::read("zz", r#"(outdoors: ["porch"])"#).unwrap();
    let error = EffectRegistry::new(vec![file]).err().expect("refused").to_string();
    assert!(error.contains("`porch`"), "{error}");
}

/// What a survey with these counts shows of the rooms (the side branches) in this weather.
fn rooms_in(counts: &[(&str, u32)], weather: &Weather, outdoors: bool) -> Vec<String> {
    let content = content();
    let effects = &content.effects;
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect(), ..Default::default() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors, weather, biome: "base:plains", biome_tags: &[], home: Some(&survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
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
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), 4)).collect(), ..Default::default() };
    let at_home = Situation { felt: 15.0, warmed: false, outdoors: false, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
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
}

/// The home perks showing for a player who has finished `done`, at a home with these counts,
/// in this weather.
fn perks(counts: &[(&str, u32)], weather: &Weather, outdoors: bool, done: &[String]) -> Vec<String> {
    let content = content();
    let effects = &content.effects;
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect(), ..Default::default() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors, weather, biome: "base:plains", biome_tags: &[], home: Some(&survey), done, held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
    effects.active(&situation, &[]).iter().map(|a| effects.get(a.effect).name.clone()).filter(|name| effects.get(effects.id(name).unwrap()).ladder == "homecraft").collect()
}

fn finished(quests: &[&str]) -> Vec<String> {
    quests.iter().map(|q| format!("base:{q}")).collect()
}

#[test]
fn a_finished_home_quest_teaches_a_perk_that_works_at_home_where_it_is_used() {
    let calm = Weather::NONE;
    let snow = Weather { precipitation: 0.9, snow: 1.0, ..Weather::NONE };
    // (quest, the home it works in, the weather, indoors?, the perk it gives)
    let cases: [(&str, &[(&str, u32)], &Weather, bool, &str); 6] = [
        ("make_camp", &[("hearth", 1)], &calm, true, "base:banked_fire"),
        ("snug_for_winter", &[], &snow, false, "base:snowed_in"),
        ("a_homely_home", &[("storage", 2)], &calm, true, "base:everything_in_its_place"),
        ("workshop_and_stove", &[("cooking", 1)], &calm, true, "base:home_cooking"),
        ("books_and_banners", &[("books", 1)], &calm, false, "base:studious"),
        ("heart_of_the_home", &[], &calm, true, "base:hearthkeeper"),
    ];
    for (quest, home, weather, outdoors, perk) in cases {
        let known = finished(&[quest]);
        assert_eq!(perks(home, weather, outdoors, &known), [perk], "{quest} teaches {perk} where it is used");
        assert!(perks(home, weather, outdoors, &[]).is_empty(), "{perk} needs {quest} done first");
    }
    // Where it is used: a fire to bank, a store to sort, a stove, a shelf of books, snow outside
    // from indoors, and indoors to read.
    assert!(perks(&[], &calm, false, &finished(&["make_camp"])).is_empty(), "no fire, nothing to bank");
    assert!(perks(&[("storage", 1)], &calm, false, &finished(&["a_homely_home"])).is_empty(), "one crate is not a store");
    assert!(perks(&[("cooking", 0)], &calm, false, &finished(&["workshop_and_stove"])).is_empty(), "no stove, no home cooking");
    assert!(perks(&[("books", 1)], &calm, true, &finished(&["books_and_banners"])).is_empty(), "no reading out in the open");
    assert!(perks(&[], &calm, false, &finished(&["snug_for_winter"])).is_empty(), "snowed in needs snow");
    assert!(perks(&[], &snow, true, &finished(&["snug_for_winter"])).is_empty(), "out in the snow you are not snowed in, you are snowed on");
}

#[test]
fn home_perks_are_countable_by_quests_and_stay_out_of_the_homes_steps() {
    let content = content();
    let effects = &content.effects;
    let steps = effects.steps();
    for name in ["homecraft_banked_fire", "homecraft_hearthkeeper", "home_workbench", "home_workshop", "home_pantry", "home_larder", "home_company", "home_full_house", "home_kitchen_garden", "home_camp", "home_heart_of_the_home"] {
        assert!(steps.contains(&format!("base:{name}")), "a quest can ask for base:{name}");
    }
    // `/home` tells the ladder and the rooms, never the perks.
    let calm = Weather::NONE;
    let done = finished(&["make_camp", "heart_of_the_home"]);
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), 0)).collect(), ..Default::default() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors: false, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&survey), done: &done, held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
    for chain in effects.home_progress(&situation) {
        for id in chain.reached.into_iter().chain(chain.next.map(|(id, _)| id)) {
            assert_eq!(effects.get(id).ladder, "home", "{} is not a step of the home", effects.get(id).name);
        }
    }
    // Nor an evening in, a time of day and no step to build: at midnight by the fire `/home`
    // never names it as next, with nothing it could ask for.
    let evening = effects.id("base:evening_in").unwrap();
    let night = Situation { night: true, ..situation };
    assert!(effects.home_progress(&night).iter().all(|chain| chain.reached != Some(evening) && chain.next.as_ref().is_none_or(|(id, _)| *id != evening)));
    // Every step `/home` names next says what it needs, by day and by night.
    for at in [&situation, &night] {
        for (id, lacks) in effects.home_progress(at).into_iter().filter_map(|chain| chain.next) {
            assert!(!lacks.is_empty(), "/home names {} next with nothing it needs", effects.get(id).name);
        }
    }
}

#[test]
fn the_next_step_is_above_the_highest_reached_not_one_skipped_below() {
    // A hearth lit with no torch by the bed: Hearthside holds though Camp does not, and
    // `/home` names Snug next, not Camp.
    let content = content();
    let effects = &content.effects;
    let calm = Weather::NONE;
    let survey = HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), u32::from(tag == "hearth"))).collect(), ..Default::default() };
    let situation = Situation { felt: 15.0, warmed: false, outdoors: true, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&survey), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
    let id = |name: &str| effects.id(&format!("base:{name}")).unwrap();
    let ladder = effects.home_progress(&situation).into_iter().find(|chain| chain.reached == Some(id("hearthside"))).expect("the ladder stands at Hearthside");
    assert_eq!(ladder.next.map(|(next, _)| next), Some(id("snug")));
}

/// A proper bed rests better than a bag on the ground (A Proper Bed: "something better than
/// the ground"): well rested anywhere, and a longer mend. A sleep mends, yet slower than food
/// (design draft 63: food is the heal).
#[test]
fn a_proper_bed_rests_well_and_every_sleep_mends_slower_than_food() {
    let content = content();
    let effects = &content.effects;
    let calm = Weather::NONE;
    let survey = |counts: &[(&str, u32)]| HomeSurvey { counts: effects.home_tags().iter().map(|tag| (tag.clone(), counts.iter().find(|(t, _)| t == tag).map_or(0, |&(_, n)| n))).collect(), ..Default::default() };
    let camp = survey(&[("bed", 1), ("light", 1)]);
    let situation = Situation { felt: 15.0, warmed: false, outdoors: true, weather: &calm, biome: "base:plains", biome_tags: &[], home: Some(&camp), done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: true };
    let bed = content.blocks.def(content.blocks.expect("base:oak_bed")).tags.clone();
    let rest = |tags: &[String]| effects.given_by_sleep(&situation, tags).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect::<Vec<_>>();
    assert_eq!(rest(&bag()), [("base:healing_sleep".to_owned(), 60.0), ("base:rested".to_owned(), 600.0)], "a bag under the sky");
    assert_eq!(rest(&bed), [("base:healing_sleep".to_owned(), 120.0), ("base:rested".to_owned(), 600.0), ("base:well_rested".to_owned(), 900.0)], "a proper bed under the sky");
    // By day a proper bed is a nap: Rested and the longer mend, never Well rested.
    let noon = Situation { night: false, ..situation };
    let nap = effects.given_by_sleep(&noon, &bed).iter().map(|&(id, seconds)| (effects.get(id).name.clone(), seconds)).collect::<Vec<_>>();
    assert_eq!(nap, [("base:healing_sleep".to_owned(), 120.0), ("base:rested".to_owned(), 600.0)], "a proper bed by day");
    // The night's rest after a nap: not while the nap's mend runs, then though its Rested is
    // still on. And a bed after a bag in one night mends no more.
    let id = |name: &str| effects.id(&format!("base:{name}")).unwrap();
    let night = effects.given_by_sleep(&situation, &bed);
    assert!(effects.rested(&[(id("healing_sleep"), 30.0), (id("rested"), 500.0)], &night), "the nap's mend still runs");
    assert!(!effects.rested(&[(id("rested"), 400.0)], &night), "the nap's Rested keeps no better rest away");
    let bagged = effects.given_by_sleep(&situation, &bag());
    assert!(effects.rested(&bagged, &night), "a bed straight after a bag gives no fresh mend");
    assert!(effects.rested(&[(id("well_rested"), 400.0)], &night), "rested well already");
    // Every heal buff that can be on at once (the strongest of each chain of replaces, the
    // others hidden by it) and the strongest prayer, together, are held at survival.ron's
    // `heal_most`, which is below what the slowest food heals a second, a bite at a time:
    // food stays the heal (design draft 63).
    let mut chain: Vec<usize> = (0..effects.len()).collect();
    fn root(chain: &mut [usize], i: usize) -> usize {
        if chain[i] == i { i } else { let r = root(chain, chain[i]); chain[i] = r; r }
    }
    let ids: Vec<_> = effects.ids().collect();
    for (i, &a) in ids.iter().enumerate() {
        for (j, &b) in ids.iter().enumerate() {
            if effects.hides(a, b) {
                let (ra, rb) = (root(&mut chain, i), root(&mut chain, j));
                chain[ra] = rb;
            }
        }
    }
    let mut strongest = std::collections::HashMap::new();
    for (i, &id) in ids.iter().enumerate() {
        let heal = effects.get(id).modifiers.heal;
        let best = strongest.entry(root(&mut chain, i)).or_insert(1.0f32);
        *best = best.max(heal);
    }
    let buffs: f32 = strongest.values().product();
    let prayer = content.prayers.all().flat_map(|(_, p)| &p.effects).filter_map(|e| if let voxmar_engine::prayer::Effect::Mending(times) = e { Some(*times) } else { None }).fold(1.0, f32::max);
    let survival = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core/survival.ron")).unwrap();
    let heal_most: f32 = survival.lines().find_map(|line| line.trim().strip_prefix("heal_most:")).and_then(|n| n.trim().trim_end_matches(',').parse().ok()).expect("survival.ron sets heal_most");
    let all_on = voxmar_engine::vitals::rates(&voxmar_engine::effect::Modifiers { heal: buffs, ..Default::default() }, prayer, heal_most);
    let per_second = 1.0 / all_on.heal_every.unwrap();
    // `rates` holds it at `heal_most` whatever stacks: the check that matters is that even
    // that stays under eating.
    let eat = content.actions.get(content.actions.id("core:eat").unwrap()).unwrap();
    let voxmar_engine::action::Duration::Seconds(bite) = eat.duration else { panic!("eating takes a set time") };
    let slowest = content.blocks.ids().map(|id| voxmar_engine::vitals::food_value(&content.blocks.def(id).tags)).filter(|&h| h > 0).min().unwrap();
    let eating = slowest as f32 / bite;
    assert!(per_second < eating, "every heal buff and prayer heal {per_second:.2} a second, as fast as eating the slowest food ({eating:.2})");
}

/// Whatever a home keeps things in, or on, counts as its storage: every store a player opens
/// (a chest, a cupboard, a crate stack, a jar), and every shelf and cupboard Shelves and
/// Cupboards asks for. A feed trough is the pen's, not the home's.
#[test]
fn every_store_shelf_and_cupboard_counts_as_storage() {
    let content = content();
    let blocks = &content.blocks;
    let stores = content.machines.iter().filter(|m| m.is_store()).map(|m| m.block);
    let furniture = blocks.ids().filter(|&id| blocks.def(id).has_tag("shelf") || blocks.def(id).has_tag("cupboard"));
    let missing: Vec<&str> = stores
        .chain(furniture)
        .filter(|&id| !blocks.def(id).has_tag("trough") && !blocks.def(id).has_tag("storage"))
        .map(|id| blocks.def(id).name.as_str())
        .collect();
    assert!(missing.is_empty(), "kept things in, yet not storage: {missing:?}");
    assert!(blocks.ids().filter(|&id| blocks.def(id).has_tag("trough")).all(|id| !blocks.def(id).has_tag("storage")), "a trough counted as a home's storage");
}

/// A package whose sleep rule asks for a kind of bed no bed is (a feather bed nobody made) is
/// refused at load, naming the tag: that rest could never be had.
#[test]
fn a_sleep_rule_for_a_bed_nobody_made_is_refused_at_load() {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content").canonicalize().unwrap();
    let dir = std::env::temp_dir().join(format!("voxmar-feather-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zz_feather")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(assets.join(package), dir.join(package)).unwrap();
    }
    std::fs::write(dir.join("zz_feather/effects.ron"), r#"(sleeps: [(effect: "base:well_rested", seconds: 1200.0, bed: "feather")])"#).unwrap();
    let error = Content::load(&dir).err().expect("refused").to_string();
    assert!(error.contains("`feather`"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Soft things count at home: a rug and a drape under a roof make soft furnishings, 1°
/// warmer; a rug alone or two do not, nor a drape alone, nor the pair out under the sky.
/// Every wall hanging is a drape, made at the grid from cloth and sticks.
#[test]
fn a_rug_and_a_hanging_under_a_roof_are_soft_furnishings() {
    let content = content();
    let effects = &content.effects;
    assert!(rooms(&[("rug", 2)]).is_empty(), "two rugs are not soft furnishings");
    assert!(rooms(&[("drape", 1)]).is_empty(), "a drape alone is not soft furnishings");
    assert_eq!(rooms(&[("rug", 1), ("drape", 1)]), ["base:soft_furnishings"]);
    assert!(rooms_in(&[("rug", 1), ("drape", 1)], &Weather::NONE, true).is_empty(), "no roof, no room");
    let soft = effects.get(effects.id("base:soft_furnishings").unwrap());
    assert_eq!((soft.modifiers.home_warmth, soft.modifiers.heal, soft.modifiers.xp), (1.0, 1.0, 1.0), "one small modifier");
    for name in ["base:red_wall_hanging", "base:white_wall_hanging"] {
        assert!(content.blocks.def(content.blocks.expect(name)).has_tag("drape"), "{name} is a drape");
    }
    let hanging = content.blocks.expect("base:red_wall_hanging");
    assert!(content.recipes.iter().any(|r| r.output == hanging), "no recipe makes a wall hanging");
}

/// Come in wet, hang your things on the peg rail by the door: within two blocks of one you
/// dry off as by a fire (`Near("pegs", 2)` ends Soaked and Drenched), so the rail by the door
/// does something. Out in the rain, it does not.
#[test]
fn wet_things_hung_on_a_peg_rail_dry_you_off() {
    let content = content();
    let effects = &content.effects;
    let pegs = [("pegs".to_owned(), 2)];
    assert!(effects.near_asks().contains(&pegs[0]), "the pegs are asked for");
    let rail = content.blocks.expect("base:oak_peg_rail");
    assert!(content.blocks.def(rail).has_tag("pegs"), "a peg rail has pegs");
    for precipitation in [0.5, 0.9] {
        let rain = Weather { precipitation, ..Weather::NONE };
        let out = Situation { felt: 15.0, warmed: false, outdoors: true, weather: &rain, biome: "base:plains", biome_tags: &[], home: None, done: &[], held: &[], near: &[], worn: &[], seated: false, seat: &[], night: false };
        let mut timed = Vec::new();
        effects.linger(&out, &mut timed);
        let wet = timed.len();
        assert!(wet > 0, "wet through in rain {precipitation}");
        effects.linger(&Situation { near: &pegs, ..out }, &mut timed);
        assert_eq!(timed.len(), wet, "out in the rain {precipitation}, the pegs do not dry you");
        effects.linger(&Situation { outdoors: false, ..out }, &mut timed);
        assert_eq!(timed.len(), wet, "indoors, still wet");
        effects.linger(&Situation { outdoors: false, near: &pegs, ..out }, &mut timed);
        assert!(timed.is_empty(), "indoors by the peg rail, dry, after rain {precipitation}");
    }
}
