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
    let recovery = attack.recovery.unwrap_or(feel.creature_recovery);
    // One that circles between blows comes in again only once it has (`Attack::circle`).
    let circled = attack.circle.map_or(0.0, |c| windup + recovery + c.seconds);
    let cycle = attack.every.max(windup + recovery).max(circled);
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
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_new_player_catches_a_fleeing_boar_at_a_sprint() {
    let content = content();
    let boar = &content.creatures.get(content.creatures.find("wild_boar").unwrap()).def;
    let sprint = content.movement.sprint_speed;
    // Half a block a second or more: a chase closes in a few seconds, not never.
    assert!(boar.movement.run + 0.5 <= sprint, "a wild boar runs {} blocks a second, too near a sprint's {sprint} to catch", boar.movement.run);
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
