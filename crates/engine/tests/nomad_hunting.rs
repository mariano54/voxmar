//! Hunting in the Nomad age with a short bow against a spear or a club (combat lane, Oct
//! 15, 2026; Mariano: the progression is too easy, so the bow must not make Nomad hunting
//! free kills). A seeded model in game time, from the content's own numbers: the rolls of
//! the server's `land_blow` and `creature_strikes`, each creature's run and fear (a
//! creature hurt flees while its fear fades, `brain::FEAR_FADE`, unless it holds its
//! ground and fights back, `Creature::holds` as `Mind::hurt` asks it), and each weapon's rhythm, reach and ammunition.
//!
//! The bow is meant to be safer than melee but slower and costlier, and the better way
//! to take skittish prey that no sprint catches.

use voxmar_engine::combat::{Weapon, defence_against, hit_chance, hit_chance_with};
use voxmar_engine::content::Content;
use voxmar_engine::creature::{Behaviour, Creature};

/// A late-Nomad player: Attack, Ranged and Defence 3 (the short bow wields at Ranged 3).
const LEVEL: u8 = 3;
/// The model's step, seconds (a server tick).
const STEP: f32 = 0.05;
/// A hunt not done in this long is given up (the prey got away).
const GIVE_UP: f32 = 180.0;
/// How far off the hunt starts: stalked up to the edge of sight.
const START: f32 = 24.0;
/// A creature's fear fades this much a second (`brain::FEAR_FADE`), and below this it
/// stops running.
const FEAR_FADE: f32 = 0.12;
const FEAR_RUNS: f32 = 0.1;
/// Runs per weapon and creature: enough that a mean kill time is known to about half a
/// percent (the aurochs' bow-to-melee ratio, 0.961, to a standard error of about 0.005).
const RUNS: u64 = 6400;

/// What came of the hunts of one creature with one weapon, on average over the kills.
#[derive(Debug, Default, Clone, Copy)]
struct Hunt {
    /// The share of hunts that killed it.
    killed: f32,
    seconds: f32,
    /// The share of the player's health lost.
    hurt: f32,
    /// Shots taken, and ammunition used up (shots not picked up again), a kill.
    shots: f32,
    spent: f32,
    /// The share of hunts in which the player fell.
    died: f32,
}

fn content() -> Content {
    Content::load(&content_dir()).unwrap()
}

fn content_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")
}

/// A run's own numbers in 0..1, from its seed (splitmix64).
struct Rolls(u64);

impl Rolls {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// 1 to `most`, evenly.
    fn upto(&mut self, most: u16) -> u16 {
        1 + (self.next() * f32::from(most)).floor().min(f32::from(most) - 1.0) as u16
    }
}

/// How far off a calm creature runs from a player coming near (`Flee`'s radius at its
/// species' fear), if it flees at all.
fn shy(creature: &Creature) -> Option<f32> {
    creature.behaviours.iter().find_map(|b| match b.behaviour {
        Behaviour::Flee { radius } => Some(radius * (0.5 + creature.def.temperament.fear)),
        _ => None,
    })
}

/// The key of an item's rolls: its name hashed (FNV-1a), which stays put however many
/// blocks the content holds, where its block id moves with every block loaded before it.
fn key(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// One hunt of `creature` with `item`, from `seed`.
fn hunt(content: &Content, creature: &Creature, item: &str, seed: u64) -> (bool, f32, f32, u32, u32, bool) {
    let combat = &content.combat;
    let feel = combat.feel;
    let name = item;
    let item = content.blocks.id(item).unwrap_or_else(|| panic!("no {item}"));
    let weapon: Weapon = combat.wielded(Some(item), |_| LEVEL);
    let shoots = !combat.ammo(&weapon).is_empty();
    let recover = combat.recover(&weapon);
    let recover_miss = combat.recover_miss(&weapon);
    let spoiled = combat.spoiled_by_hits(&weapon);
    let def = &creature.def;
    let movement = &content.movement;
    let hitpoints = content.skills.get(content.skills.id("core:hitpoints").expect("hitpoints"));
    let health = 2.0 * f32::from(hitpoints.start);
    let ours = hit_chance_with(LEVEL, weapon.accuracy, defence_against(def.defence, def.weak, weapon.style), 0, feel.beat_accuracy);
    let mut rolls = Rolls(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ key(name));
    let (mut gap, mut life, mut taken) = (START, f32::from(def.health), 0.0f32);
    let (mut fear, mut angry) = (0.0f32, false);
    let (mut ready, mut lands, mut blow_at) = (0.0f32, None::<f32>, 0.0f32);
    let (mut shots, mut kept) = (0u32, 0u32);
    let shy = shy(creature);
    let reach = weapon.reach;
    let mut t = 0.0;
    while t < GIVE_UP {
        t += STEP;
        // The creature: angry, it closes and strikes; afraid, it runs while its fear lasts;
        // calm and shy, it keeps the player off; else it stands (grazing).
        fear = (fear - FEAR_FADE * STEP).max(0.0);
        let attack = def.attack.filter(|_| angry);
        let creature_speed = if let Some(attack) = attack {
            let strikes = attack.reach + def.body.size.0 * 0.5 + 0.3;
            if gap > strikes {
                -def.movement.run
            } else {
                if t >= blow_at {
                    let windup = attack.windup.unwrap_or(feel.creature_windup);
                    let cycle = attack.every.max(windup + attack.recovery.unwrap_or(feel.creature_recovery));
                    blow_at = t + cycle;
                    let accuracy = attack.accuracy.unwrap_or((feel.creature_accuracy * f32::from(def.level)).round() as u16);
                    if rolls.next() < hit_chance(def.level, accuracy, LEVEL, 0) {
                        taken += f32::from(rolls.upto(u16::from(attack.damage.1).max(u16::from(attack.damage.0))));
                        // A hit taken spoils a shot being drawn, as a guard raised does
                        // (`Rhythm::cancel`): the meter starts again from `cancel_to`.
                        if spoiled && lands.is_some() {
                            lands = None;
                            ready = t + weapon.cooldown * (1.0 - feel.cancel_to);
                        }
                    }
                }
                0.0
            }
        } else if fear > FEAR_RUNS || shy.is_some_and(|r| gap < r * 1.5 && fear > 0.0) {
            def.movement.run
        } else if shy.is_some_and(|r| gap < r) {
            fear = fear.max(FEAR_RUNS + 0.01);
            def.movement.run
        } else {
            0.0
        };
        if taken >= health {
            return (false, t, 1.0, shots, shots - kept, true);
        }
        // The player: in reach, strikes on the beat; and moves: sprinting after what runs
        // (as fast as a shot being drawn lets them), walking up on what stands (never into a
        // shy one's ground with a bow), closing on what charges only to meet it.
        if gap <= reach && lands.is_none() && t >= ready {
            lands = Some(t + combat.timing(&weapon).windup);
            ready = t + weapon.cooldown;
        }
        let drawing = if lands.is_some() { combat.drawing_speed(&weapon) } else { 1.0 };
        let player_speed = if creature_speed > 0.0 {
            movement.sprint_speed * drawing
        } else if gap <= reach {
            0.0
        } else if angry {
            movement.sprint_speed * drawing
        } else if shoots && shy.is_some_and(|r| gap - movement.walk_speed * STEP < r) {
            0.0
        } else {
            movement.walk_speed * drawing
        };
        gap = (gap + (creature_speed - player_speed) * STEP).max(0.0);
        if shoots && shy.is_some_and(|r| gap >= r) && player_speed == 0.0 && gap > reach && creature_speed == 0.0 {
            // Waiting it out at the edge of its ground with nothing in reach: it never comes.
            return (false, GIVE_UP, taken / health, shots, shots - kept, false);
        }
        if lands.is_some_and(|at| t >= at) {
            lands = None;
            let in_reach = gap <= reach;
            if shoots {
                shots += 1;
            }
            let hit = in_reach && rolls.next() < ours;
            if hit {
                let damage = f32::from(rolls.upto(weapon.max_hit(LEVEL)));
                life -= damage;
                // The killing shot too, by `recover`, where the creature fell.
                if shoots && rolls.next() < recover {
                    kept += 1;
                }
                if life <= 0.0 {
                    return (true, t, taken / health, shots, shots - kept, false);
                }
            } else if shoots && rolls.next() < recover_miss {
                kept += 1;
            }
            // A blow that reached it, hit or miss, rouses it as `land_blow` does
            // (`Mind::hurt`): it fights back, or runs while its fear lasts. A shot at the
            // air, out of reach, does not.
            if in_reach {
                let fights = creature.holds(false, life / f32::from(def.health), 1.0 - def.temperament.fear);
                if fights {
                    angry = true;
                } else {
                    angry = false;
                    fear = 1.0;
                }
            }
        }
    }
    (false, GIVE_UP, taken / health, shots, shots - kept, false)
}

/// The hunts of `name` with `item`, over `RUNS` seeds.
fn hunts(content: &Content, name: &str, item: &str) -> Hunt {
    let creature = content.creatures.get(content.creatures.find(name).unwrap_or_else(|| panic!("no {name}")));
    let mut sum = Hunt::default();
    let mut kills = 0.0;
    for seed in 0..RUNS {
        let (killed, seconds, hurt, shots, spent, died) = hunt(content, creature, item, seed);
        sum.died += f32::from(died);
        sum.hurt += hurt;
        if killed {
            kills += 1.0;
            sum.seconds += seconds;
            sum.shots += shots as f32;
            sum.spent += spent as f32;
        }
    }
    let n = RUNS as f32;
    let per = |x: f32| if kills > 0.0 { x / kills } else { f32::NAN };
    Hunt { killed: kills / n, seconds: per(sum.seconds), hurt: sum.hurt / n, shots: per(sum.shots), spent: per(sum.spent), died: sum.died / n }
}

/// The Nomad age's game: what a late-Nomad player hunts.
const ROSTER: [&str; 5] = ["rabbit", "junglefowl", "red_deer", "wild_boar", "aurochs"];
const MELEE: [&str; 2] = ["base:flint_spear", "base:bone_club"];
const BOW: &str = "base:short_bow";
const SLING: &str = "base:sling";

fn table(content: &Content) -> Vec<(&'static str, &'static str, Hunt)> {
    let mut rows = Vec::new();
    for name in ROSTER {
        for item in MELEE.iter().chain([&BOW, &SLING]) {
            let h = hunts(content, name, item);
            eprintln!(
                "{name:<11} {:<12} killed {:>4.0}%  {:>5.1} s  hurt {:>3.0}%  died {:>3.0}%  shots {:>4.1}  spent {:>4.1}",
                item.trim_start_matches("base:"),
                h.killed * 100.0,
                h.seconds,
                h.hurt * 100.0,
                h.died * 100.0,
                h.shots,
                h.spent
            );
            rows.push((name, *item, h));
        }
    }
    rows
}

/// The bow is not strictly better than melee on the Nomad roster: it is safer, but on
/// every kind of game melee takes it is no quicker, it uses up arrows on every kind, and
/// only on skittish prey (what no sprint catches) does it win outright. The sling, a Nomad's
/// first ranged weapon, is no quicker than melee either.
#[test]
fn the_short_bow_is_safer_but_slower_and_costlier_than_melee_in_the_nomad_age() {
    let content = content();
    let rows = table(&content);
    let get = |name: &str, item: &str| rows.iter().find(|(n, i, _)| *n == name && *i == item).map(|r| r.2).expect("a row");
    let best_melee = |name: &str| MELEE.iter().map(|m| get(name, m)).max_by(|a, b| a.killed.total_cmp(&b.killed).then(b.seconds.total_cmp(&a.seconds))).expect("melee");
    let mut wrong = Vec::new();
    let (mut bow_time, mut melee_time) = (0.0, 0.0);
    for name in ROSTER {
        let melee = best_melee(name);
        for ranged in [BOW, SLING] {
            let shot = get(name, ranged);
            let short = ranged.trim_start_matches("base:");
            if melee.killed >= 0.5 && shot.killed > 0.0 && shot.seconds < 0.95 * melee.seconds {
                wrong.push(format!("{name}: the {short} kills in {:.1} s, melee in {:.1} s: on game melee takes, a shot should be no quicker", shot.seconds, melee.seconds));
            }
        }
        let bow = get(name, BOW);
        if melee.killed >= 0.5 {
            bow_time += bow.seconds;
            melee_time += melee.seconds;
            if bow.hurt > melee.hurt + 0.02 {
                wrong.push(format!("{name}: the bow costs {:.0}% of health, melee {:.0}%: the bow should be the safer", bow.hurt * 100.0, melee.hurt * 100.0));
            }
        }
        if bow.killed > 0.0 && bow.spent < 0.5 {
            wrong.push(format!("{name}: a kill uses up {:.2} arrows: arrows should be a cost", bow.spent));
        }
    }
    eprintln!("over the game melee takes, the bow kills in {bow_time:.1} s and melee in {melee_time:.1} s");
    let deer = (get("red_deer", BOW), best_melee("red_deer"));
    if deer.0.killed < deer.1.killed + 0.3 {
        wrong.push(format!("a red deer is taken {:.0}% of the time with a bow and {:.0}% in melee: skittish prey is the bow's", deer.0.killed * 100.0, deer.1.killed * 100.0));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A hunt's rolls are its own, keyed by the weapon's name: a package of blocks loaded ahead
/// of the base game's (which moves every weapon's block id along) leaves every hunt as it
/// was. Before: the rolls were keyed by the block id, so a building piece added before the
/// weapons' file drew another sample, and the aurochs' bow, 4% quicker than the spear,
/// came out 6% quicker and failed the bow test.
#[test]
fn blocks_loaded_ahead_of_the_weapons_leave_the_hunts_as_they_were() {
    let dir = std::env::temp_dir().join(format!("voxmar-hunt-ids-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("aaa")).unwrap();
    for package in ["base", "core"] {
        std::os::unix::fs::symlink(content_dir().join(package).canonicalize().unwrap(), dir.join(package)).unwrap();
    }
    let extra: String = (0..40).map(|i| format!("(name: \"filler_{i}\", textures: All(\"stone\"), hardness: 1.0, color: (120, 120, 120)),")).collect();
    std::fs::write(dir.join("aaa/blocks.ron"), format!("(blocks: [{extra}])")).unwrap();
    let more = Content::load(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    let more = more.unwrap();
    let base = content();
    assert_ne!(more.blocks.id(BOW), base.blocks.id(BOW), "the fixture must move the weapons' ids");
    let (before, after) = (table(&base), table(&more));
    for ((name, item, a), (_, _, b)) in before.iter().zip(&after) {
        let same = |x: f32, y: f32| x == y || x.is_nan() && y.is_nan();
        assert!(same(a.killed, b.killed) && same(a.seconds, b.seconds) && same(a.hurt, b.hurt) && same(a.spent, b.spent), "{name} with {item}: {a:?} before, {b:?} with blocks ahead");
    }
}
