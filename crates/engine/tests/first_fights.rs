//! The first-age fights fit a new player (combat lane's balance pass, Oct 14, 2026):
//! starting health, Attack 1 and Defence 1, a stone sword swung on its cooldown, never
//! guarding. On average, from the content's own rolls (the server's `land_blow` and
//! `creature_strikes`): a wolf is beaten with a fifth to a half of the player's health
//! left, a badger and a fox easily, an aurochs fairly; a wild boar that flees is caught by
//! a sprint; and a brown bear, which no new player can beat, lives nowhere they start.
//! `first_age_fights_balanced` in the server's gameplay tests measures the same fights on
//! the clock (docs/combat.md, "First-age fight balance").

use voxmar_engine::combat::{defence_against, hit_chance, hit_chance_with};
use voxmar_engine::content::Content;
use voxmar_engine::creature::fit;

/// The temperatures a new player starts in: the world's spawn is open, temperate
/// grassland at 8 to 20 degrees (worldgen's `spawn_search`).
const START_TEMPERATURES: (f32, f32) = (8.0, 20.0);

struct Fight {
    seconds: f32,
    health_left: f32,
}

/// A new player's fight with `name` on average: how long the kill takes, swinging on the
/// beat, and the share of their health left (below 0: they fall first).
fn fight(content: &Content, name: &str) -> Fight {
    let creature = &content.creatures.get(content.creatures.find(name).unwrap_or_else(|| panic!("no {name}"))).def;
    let combat = &content.combat;
    let feel = combat.feel;
    let sword = content.blocks.id("base:stone_sword").expect("a stone sword");
    let weapon = combat.wielded(Some(sword), |_| 1);
    let hitpoints = content.skills.get(content.skills.id("core:hitpoints").expect("hitpoints"));
    let health = 2.0 * f32::from(hitpoints.start);
    let ours = hit_chance_with(1, weapon.accuracy, defence_against(creature.defence, creature.weak, weapon.style), 0, feel.beat_accuracy);
    let per_second = ours * (1.0 + f32::from(weapon.max_hit(1))) / 2.0 / weapon.cooldown;
    let seconds = f32::from(creature.health) / per_second;
    let Some(attack) = creature.attack else { return Fight { seconds, health_left: 1.0 } };
    let windup = attack.windup.unwrap_or(feel.creature_windup);
    let cycle = attack.every.max(windup + attack.recovery.unwrap_or(feel.creature_recovery));
    let accuracy = attack.accuracy.unwrap_or((feel.creature_accuracy * f32::from(creature.level)).round() as u16);
    let theirs = hit_chance(creature.level, accuracy, 1, 0);
    let blows = if seconds > windup { (seconds - windup) / cycle + 1.0 } else { 0.0 };
    let taken = blows * theirs * f32::from(u16::from(attack.damage.0) + u16::from(attack.damage.1)) / 2.0;
    Fight { seconds, health_left: 1.0 - taken / health }
}

fn content() -> Content {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    Content::load(&dir).unwrap()
}

#[test]
fn a_new_player_beats_the_first_age_creatures_as_designed() {
    let content = content();
    let mut wrong = Vec::new();
    let mut check = |name: &str, (least, most): (f32, f32), why: &str| {
        let f = fight(&content, name);
        eprintln!("{name}: {:.1} s, {:.0}% health left", f.seconds, f.health_left * 100.0);
        if !(least..=most).contains(&f.health_left) {
            wrong.push(format!("{name}: a new player keeps {:.0}% of their health ({:.1} s), not {:.0}% to {:.0}%: {why}", f.health_left * 100.0, f.seconds, least * 100.0, most * 100.0));
        }
    };
    check("wolf", (0.15, 0.45), "a wolf is a real fight a new player wins without guarding");
    check("badger", (0.5, 1.0), "a badger is easy");
    check("fox", (0.6, 1.0), "a fox is easy");
    check("aurochs", (0.3, 0.85), "an aurochs is real but fair");
    check("wild_boar", (0.6, 1.0), "a wild boar answers a blow but is easy");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_new_player_catches_a_fleeing_boar_at_a_sprint() {
    let content = content();
    let boar = &content.creatures.get(content.creatures.find("wild_boar").unwrap()).def;
    let sprint = content.movement.sprint_speed;
    // Half a block a second or more: a chase closes in a few seconds, not never.
    assert!(boar.movement.run + 0.5 <= sprint, "a wild boar runs {} blocks a second, too near a sprint's {sprint} to catch", boar.movement.run);
    // The liveliest of them too (`Personality::pace`, its energy rolled as far up as it goes).
    let t = &boar.temperament;
    let liveliest = voxmar_engine::creature::brain::Personality { energy: (t.energy + t.spread).min(1.0), ..voxmar_engine::creature::brain::Personality::roll(t, 0) }.pace();
    let fastest = boar.movement.run * liveliest;
    assert!(fastest <= 0.95 * sprint, "the liveliest wild boar runs {fastest:.2} blocks a second, above 0.95 of a sprint's {sprint}");
}

/// The wild boar never starts a fight, but struck while it is hale it turns and gores
/// back; badly hurt it runs instead, and a piglet always runs. Whatever its own temper.
#[test]
fn a_wild_boar_struck_gores_back_until_badly_hurt() {
    use voxmar_engine::creature::brain::{Mind, Seen, Situation, think};
    use voxmar_engine::glam;
    let content = content();
    let boar = content.creatures.get(content.creatures.find("wild_boar").unwrap());
    let doing = |mind: &Mind| mind.doing.map_or("nothing", |i| boar.modules[i].0.name());
    let near = [Seen { id: 1, position: glam::Vec3::new(1.2, 0.0, 0.0), tempting: false, combat_level: 3, creative: false, tolerated: false, friend: false, open: true }];
    let at = |health| Situation { position: glam::Vec3::ZERO, home: glam::Vec3::ZERO, health, phase: 0, scale: 1.0, daylight: 1.0, players: &near, kin: &[], food: None, mate: None, kept: false, errand: None, dt: 0.2, seed: 7 };
    // Thinking a blow's worth of time (`every` 1.6 s of the player's swings, near enough).
    let between = |mind: &mut Mind, health: f32, thinks: u32| {
        let mut attacked = false;
        for _ in 0..thinks {
            attacked |= think(boar, mind, &at(health)).attack.is_some();
        }
        attacked
    };
    for seed in 0..40 {
        // One mind played through a fight: it never goes first, gores back each blow while
        // above 40% of its health, then drops the fight and runs, and keeps running with
        // no more blows.
        let mut mind = Mind::new(&boar.def.temperament, seed);
        assert!(!between(&mut mind, 1.0, 8) && doing(&mind) != "defend", "seed {seed}: a boar went for a player who never struck it ({})", doing(&mind));
        for health in [0.9, 0.7, 0.5] {
            mind.hurt(boar, Some(1), health);
            let attacked = between(&mut mind, health, 8);
            assert!(attacked && doing(&mind) == "defend", "seed {seed}: a boar struck down to {health} did not gore back ({}, attacked {attacked})", doing(&mind));
        }
        mind.hurt(boar, Some(1), 0.3);
        let _ = between(&mut mind, 0.3, 1);
        assert_eq!(doing(&mind), "flee", "seed {seed}: a boar struck down to 0.3 did not run");
        let attacked = between(&mut mind, 0.3, 25);
        assert!(!attacked && doing(&mind) == "flee", "seed {seed}: 5 s on with no blows a badly hurt boar stopped running ({}, attacked {attacked})", doing(&mind));
        // A piglet runs, hale or not.
        let mut mind = Mind::new(&boar.def.temperament, seed);
        mind.hurt_young(boar, Some(1), 0.8, true);
        let _ = think(boar, &mut mind, &at(0.8));
        assert_eq!(doing(&mind), "flee", "seed {seed}: a piglet struck did not run");
    }
    // Every kind that fights and runs (a fox, a frost stag, a cave bat): struck while hale it
    // fights, badly hurt it drops the fight and runs, and keeps running with no more blows.
    let mut running = Vec::new();
    for (_, creature) in content.creatures.iter().filter(|(_, c)| c.fights() && c.flees() && !c.def.boss) {
        let name = creature.def.name.rsplit(':').next().unwrap_or(&creature.def.name);
        let doing = |mind: &Mind| mind.doing.map_or("nothing", |i| creature.modules[i].0.name());
        for seed in 0..40 {
            let mut mind = Mind::new(&creature.def.temperament, seed);
            mind.hurt(creature, Some(1), 0.9);
            for _ in 0..4 {
                let _ = think(creature, &mut mind, &at(0.9));
            }
            assert!(matches!(doing(&mind), "defend" | "hunt"), "seed {seed}: a {name} struck while hale did not fight ({})", doing(&mind));
            mind.hurt(creature, Some(1), 0.05);
            for _ in 0..25 {
                let _ = think(creature, &mut mind, &at(0.05));
            }
            assert_eq!(doing(&mind), "flee", "seed {seed}: a badly hurt {name} was not running 5 s on");
        }
        running.push(name.to_owned());
    }
    assert!(running.iter().any(|n| n == "wild_boar"), "the kinds that fight and run: {running:?}");
    eprintln!("fight, then run: {running:?}");
    // A kind that never runs fights on badly hurt, its anger kept.
    for (_, creature) in content.creatures.iter().filter(|(_, c)| c.fights() && !c.flees() && !c.def.boss) {
        let mut mind = Mind::new(&creature.def.temperament, 3);
        mind.hurt(creature, Some(1), 0.9);
        mind.hurt(creature, Some(1), 0.05);
        assert!(mind.anger > 0.5, "{} gave up its fight badly hurt though it never runs", creature.def.name);
    }
}

#[test]
fn a_brown_bear_is_a_danger_far_from_where_a_new_player_starts() {
    let content = content();
    let bear = &content.creatures.get(content.creatures.find("brown_bear").unwrap()).def;
    // No new player can beat one.
    assert!(fight(&content, "brown_bear").health_left < 0.0, "a new player could beat a brown bear");
    // So none lives where they start, at any moisture.
    let (low, high) = START_TEMPERATURES;
    for tenth in 0..=((high - low) * 10.0) as u32 {
        let temperature = low + tenth as f32 / 10.0;
        for moisture in [0.6, 1.0, 1.6] {
            let comfort = fit(&bear.habitat.comfort, temperature, moisture, 0.0);
            assert_eq!(comfort, 0.0, "a brown bear lives at {temperature} degrees, where a new player starts");
        }
    }
}
