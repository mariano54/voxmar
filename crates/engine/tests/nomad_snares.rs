//! A snare line never out-earns active hunting (combat lane's snare pass, Oct 15, 2026,
//! docs/traps.md "Snares as a hunt"). Rabbits wander a meadow, the field the Nomad hunts:
//! a few at a time, each taken replaced at the edge a while later, so both ways of hunting
//! draw on the same supply. Snares are set on the meadow and walked every ten minutes, as
//! the game's snare would be used, by its data (its chance for a rabbit, its wear and its
//! share of a kill's experience); a rabbit that slips one bolts at a run for as long as
//! its fear lasts. A hunter on the same meadow walks to the nearest rabbit at a sprint and
//! takes it with a short bow, in the time `nomad_hunting.rs` measures. Seeded, never on
//! the clock: experience per game hour, each way; the snare line both at the snare's own
//! level and as the best hunter its `skilled` counts sets it (the hunter kept at the bow's
//! Nomad pace, so the best snares are held to the stricter bar).

use voxmar_engine::content::Content;

/// Rabbits on the meadow at once, how long one taken is gone before another comes, and the
/// meadow's side in blocks.
const POP: usize = 6;
const REFILL: f32 = 90.0;
const FIELD: f32 = 48.0;
/// Seconds a step, the game hour measured and the runs, each from its own seed.
const STEP: f32 = 0.25;
const HOUR: f32 = 3600.0;
const RUNS: u64 = 16;
/// How often the snare line is walked, and the seconds a short bow takes a rabbit once in
/// reach (`nomad_hunting.rs`, the bow after the Nomad hunting pass).
const CHECK: f32 = 600.0;
const BOW_KILL: f32 = 8.1;
/// Fear fades at 0.12 a second (`brain::FEAR_FADE`): a frightened rabbit runs that long.
const FEAR_SECONDS: f32 = 1.0 / 0.12;

/// A seeded stream of rolls in [0, 1).
struct Rolls(u64);

impl Rolls {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

struct Rabbit {
    at: (f32, f32),
    heading: f32,
    turn_at: f32,
    afraid_until: f32,
    /// The snare it last stepped into and slipped, not rolled for again while in its cell.
    slipped: Option<(i32, i32)>,
}

/// The meadow: its rabbits, and when each one taken comes back.
struct Meadow {
    rabbits: Vec<Rabbit>,
    coming: Vec<f32>,
}

impl Meadow {
    fn new(rolls: &mut Rolls) -> Self {
        let rabbits = (0..POP).map(|_| Rabbit { at: (rolls.next() * FIELD, rolls.next() * FIELD), heading: rolls.next() * std::f32::consts::TAU, turn_at: 0.0, afraid_until: 0.0, slipped: None }).collect();
        Self { rabbits, coming: Vec::new() }
    }

    /// Each rabbit moves a step (`walk`, or `run` while afraid), turning now and then and
    /// kept on the meadow; and those due come back at its edge.
    fn step(&mut self, t: f32, walk: f32, run: f32, rolls: &mut Rolls) {
        for r in &mut self.rabbits {
            if t >= r.turn_at {
                r.heading = rolls.next() * std::f32::consts::TAU;
                r.turn_at = t + 3.0 + 5.0 * rolls.next();
            }
            let speed = if t < r.afraid_until { run } else { walk * 0.5 };
            let (mut x, mut z) = (r.at.0 + r.heading.cos() * speed * STEP, r.at.1 + r.heading.sin() * speed * STEP);
            if !(0.0..FIELD).contains(&x) || !(0.0..FIELD).contains(&z) {
                r.heading += std::f32::consts::PI;
                (x, z) = (x.clamp(0.0, FIELD - 0.01), z.clamp(0.0, FIELD - 0.01));
            }
            r.at = (x, z);
        }
        let due = self.coming.iter().filter(|&&at| at <= t).count();
        self.coming.retain(|&at| at > t);
        for _ in 0..due {
            let edge = rolls.next() * FIELD;
            self.rabbits.push(Rabbit { at: (edge, 0.0), heading: std::f32::consts::FRAC_PI_2, turn_at: t + 2.0, afraid_until: 0.0, slipped: None });
        }
    }

    fn take(&mut self, i: usize, t: f32) {
        self.rabbits.swap_remove(i);
        self.coming.push(t + REFILL);
    }
}

struct Earned {
    xp_per_hour: f32,
    snares_spent: f32,
}

fn content() -> Content {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    Content::load(&dir).unwrap()
}

/// How a snare catches: its chance for a rabbit, its share of a kill's experience, and the
/// chance taking a catch wears it out.
#[derive(Clone, Copy)]
struct Rules {
    chance: f32,
    share: f32,
    wear: f32,
}

/// An hour of a line of `snares` on the meadow, walked every `CHECK`.
fn snare_line(content: &Content, snares: usize, seed: u64, rules: Rules) -> Earned {
    let rabbit = content.creatures.find("rabbit").unwrap();
    let def = &content.creatures.get(rabbit).def;
    let Rules { chance, share, wear } = rules;
    let kill = def.xp.kill * share;
    let mut rolls = Rolls(seed);
    let mut meadow = Meadow::new(&mut rolls);
    let mut cells: Vec<((i32, i32), bool)> = (0..snares).map(|_| (((rolls.next() * FIELD) as i32, (rolls.next() * FIELD) as i32), false)).collect();
    let (mut xp, mut spent, mut t) = (0.0, snares as f32, 0.0);
    while t < HOUR {
        meadow.step(t, def.movement.walk, def.movement.run, &mut rolls);
        let mut i = 0;
        while i < meadow.rabbits.len() {
            let r = &mut meadow.rabbits[i];
            let cell = (r.at.0 as i32, r.at.1 as i32);
            if r.slipped.is_some_and(|s| s != cell) {
                r.slipped = None;
            }
            let set = cells.iter().position(|&(c, sprung)| c == cell && !sprung);
            match set {
                Some(s) if r.slipped != Some(cell) => {
                    if rolls.next() < chance {
                        cells[s].1 = true;
                        meadow.take(i, t);
                        continue;
                    }
                    (r.slipped, r.afraid_until) = (Some(cell), t + FEAR_SECONDS);
                }
                _ => {}
            }
            i += 1;
        }
        t += STEP;
        // The line walked: each catch taken, the snare set again or, worn out, made anew.
        if (t / CHECK).floor() != ((t - STEP) / CHECK).floor() {
            for (_, sprung) in &mut cells {
                if *sprung {
                    *sprung = false;
                    xp += kill;
                    if rolls.next() < wear {
                        spent += 1.0;
                    }
                }
            }
        }
    }
    Earned { xp_per_hour: xp * 3600.0 / HOUR, snares_spent: spent }
}

/// An hour of a hunter on the meadow with a short bow, after the nearest rabbit each time.
fn hunter(content: &Content, seed: u64) -> Earned {
    let rabbit = content.creatures.find("rabbit").unwrap();
    let def = &content.creatures.get(rabbit).def;
    let sprint = content.movement.sprint_speed;
    let mut rolls = Rolls(seed);
    let mut meadow = Meadow::new(&mut rolls);
    let (mut at, mut busy_until, mut xp, mut t) = ((FIELD / 2.0, FIELD / 2.0), 0.0, 0.0, 0.0);
    while t < HOUR {
        meadow.step(t, def.movement.walk, def.movement.run, &mut rolls);
        if t >= busy_until {
            let distance = |r: &Rabbit| ((r.at.0 - at.0).powi(2) + (r.at.1 - at.1).powi(2)).sqrt();
            if let Some(i) = (0..meadow.rabbits.len()).min_by(|&a, &b| distance(&meadow.rabbits[a]).total_cmp(&distance(&meadow.rabbits[b]))) {
                let r = &meadow.rabbits[i];
                // Up to the bow's reach at a sprint, then the shots.
                busy_until = t + (distance(r) - 20.0).max(0.0) / sprint + BOW_KILL;
                at = r.at;
                meadow.take(i, busy_until);
                xp += def.xp.kill;
            }
        }
        t += STEP;
    }
    Earned { xp_per_hour: xp * 3600.0 / HOUR, snares_spent: 0.0 }
}

#[test]
fn a_snare_line_never_out_earns_active_hunting() {
    let content = content();
    let mean = |f: &dyn Fn(u64) -> Earned| {
        let runs: Vec<Earned> = (0..RUNS).map(f).collect();
        let n = runs.len() as f32;
        (runs.iter().map(|e| e.xp_per_hour).sum::<f32>() / n, runs.iter().map(|e| e.snares_spent).sum::<f32>() / n)
    };
    let (hunted, _) = mean(&|seed| hunter(&content, seed));
    eprintln!("hunter, short bow: {hunted:.1} experience an hour");
    let trap = content.traps.set(content.blocks.id("base:snare").unwrap()).expect("the snare is a trap");
    let rabbit = content.creatures.find("rabbit").unwrap();
    // At the snare's own level, and set and taken by the best hunter its `skilled` counts.
    let best = trap.level + trap.skilled.levels;
    let at = |level: u8| Rules { chance: trap.chance_at(rabbit, level), share: trap.xp, wear: trap.wear_at(level) };
    // As snares were before this pass: sure, the whole kill's experience, never worn out.
    let before = Rules { chance: 1.0, share: 1.0, wear: 0.0 };
    let mut wrong = Vec::new();
    let (mut last, mut last_best) = (0.0, 0.0);
    for snares in [4, 16, 64, 256] {
        let (was, _) = mean(&|seed| snare_line(&content, snares, 0x5a4e ^ seed, before));
        let (xp, spent) = mean(&|seed| snare_line(&content, snares, 0x5a4e ^ seed, at(trap.level)));
        let (xp_best, spent_best) = mean(&|seed| snare_line(&content, snares, 0x5a4e ^ seed, at(best)));
        eprintln!("{snares:>3} snares: {xp:.1} experience an hour, {spent:.1} snares made; at Hunter {best} {xp_best:.1}, {spent_best:.1} made (before: {was:.1})");
        for (xp, last, who) in [(xp, last, "a new hunter's"), (xp_best, last_best, "the best hunter's")] {
            if xp >= hunted {
                wrong.push(format!("{snares} of {who} snares earn {xp:.1} experience an hour, a hunter {hunted:.1}: a snare line should never out-earn the hunt"));
            }
            if xp <= 0.0 || xp + 1e-3 < last {
                wrong.push(format!("{snares} of {who} snares earn {xp:.1} experience an hour, fewer earned {last:.1}: more snares should catch more"));
            }
        }
        if xp_best <= xp {
            wrong.push(format!("{snares} of the best hunter's snares earn {xp_best:.1}, no more than a new hunter's {xp:.1}"));
        }
        (last, last_best) = (xp, xp_best);
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
