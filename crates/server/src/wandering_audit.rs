//! How often a new player meets wandering parties (docs/wandering.md, "How often"): a
//! seeded walk through real generated land, the game's own start, spot and pick rules at
//! every scan, and a plain model of how long each script keeps its party about. Run with
//! `cargo test -p voxmar_server wandering_audit -- --ignored --nocapture`; it prints the
//! table the doc keeps.

use super::*;
use voxmar_engine::world::Column;
use voxmar_worldgen::Generator;

/// Game seconds a new player spends at one place before moving on.
const STAY: f32 = 900.0;
/// When a new player has a home (a lit hearth), in game seconds: a person, not the bot.
const HOME_AT: f32 = 2700.0;
/// The share of parties a new player deals with (a trade, a lesson, a walk).
const DEALS: f32 = 0.5;
/// Columns generated about each place, each way: enough for a spot and its gathering.
const AROUND: i32 = 5;

/// The seconds a party stays near the player it came to, by its script: those passing by
/// are soon gone, those that stop stay their time (never past their lifetime).
fn stays(event: &voxmar_engine::wander::Event) -> f32 {
    let mid = |(a, b): (f32, f32)| (a + b) / 2.0;
    let near = match event.def.script {
        Script::March { length, pace, .. } => length / pace.max(0.5) + 45.0,
        Script::Patrol { radius, stops, pause, laps } => laps as f32 * (std::f32::consts::TAU * radius / 1.4 + stops as f32 * mid(pause)),
        Script::Camp { stay, .. } => mid(stay) + 30.0,
        Script::Shadow { seconds, .. } | Script::Roam { seconds, .. } | Script::Drift { seconds, .. } | Script::Hunt { seconds, .. } => seconds,
        Script::Chase { length, pace, hold, .. } => length / pace.max(0.5) + mid(hold),
        Script::Ambush { patience, .. } => patience,
        Script::Lure { length, pace, linger, .. } => length / pace.max(0.5) + linger,
        Script::Gather { work, visits, .. } => visits as f32 * (mid(work) + 10.0),
        // Walked with, a journey is near all the way; left, it is soon out of reach.
        Script::Journey { .. } => 240.0,
    };
    near.min(event.def.lifetime)
}

/// A server on the game's content with a new world of `seed`, nothing generated yet.
fn world(seed: u64) -> Server {
    let content = crate::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
    let mut config = crate::ServerConfig { seed, ..Default::default() };
    let (save, meta) = crate::open_world(&mut config).expect("no save to open");
    let (_, incoming) = crossbeam_channel::unbounded();
    let (_, console) = crossbeam_channel::unbounded();
    let mut server = Server::new(config, meta, save, crate::save::WorldState::default(), content, incoming, console);
    server.world = Default::default();
    let (client, connection) = voxmar_protocol::local_pair();
    std::mem::forget(client);
    let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
    (player.joined, player.name) = (true, "Tëster".into());
    server.players.insert(1, player);
    server
}

/// Generates the land about `at` and stands the player on it; `None` over water.
fn go_to(server: &mut Server, at: IVec2) -> Option<Vec3> {
    let centre = ColumnPos::of_block(IVec3::new(at.x, 0, at.y));
    let want: Vec<ColumnPos> = (-AROUND..=AROUND).flat_map(|dx| (-AROUND..=AROUND).map(move |dz| centre.offset(dx, dz))).collect();
    let made: Vec<(ColumnPos, Column)> = {
        use rayon::prelude::*;
        let overworld = server.overworld.clone();
        want.par_iter().filter(|p| !server.world.contains(**p)).map(|p| (*p, overworld.generate(*p))).collect()
    };
    for (pos, column) in made {
        server.world.insert(pos, std::sync::Arc::new(column));
    }
    let top = (0..320).rev().map(|y| IVec3::new(at.x, y, at.y)).find(|&c| server.is_solid(c - IVec3::Y) && !server.is_solid(c))?;
    let stand = server.standing_spot(at.x, at.y, top.y)?;
    if server.is_wet(stand) {
        return None;
    }
    let position = stand.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
    server.players.get_mut(&1).unwrap().position = position;
    Some(position)
}

/// Combat skills a new player has after `t` game seconds: about level 10 at two hours,
/// 20 at six (a person who fights now and then, not a grinder).
fn train(server: &mut Server, t: f32) {
    let level = (1.0 + 9.0 * (t / 7200.0).powf(0.7)).min(99.0) as u8;
    for name in ["core:attack", "core:strength", "core:defence", "core:hitpoints"] {
        let Some(skill) = server.content.skills.id(name) else { continue };
        let want = server.content.skills.xp_for(level);
        let p = server.players.get_mut(&1).unwrap();
        let have = p.skills.xp(skill);
        if want > have {
            p.skills.grant(skill, want - have);
        }
    }
}

/// What one seed's walk met.
#[derive(Default)]
struct Walk {
    /// (game seconds, kind, biome) of each party that came.
    met: Vec<(f32, usize, String)>,
    tries: u32,
    unwelcome: u32,
    no_chance: u32,
    no_spot: u32,
    no_pick: u32,
    no_fit: u32,
    wet: u32,
}

fn walk(seed: u64, hours: f32) -> Walk {
    let mut server = world(seed);
    let rules = *server.content.wandering.rules();
    let home = server.spawn;
    let dawn = server.day;
    let mut out = Walk::default();
    // (instance, ends at tick)
    let mut about: Vec<(u64, u64)> = Vec::new();
    let mut place = 0u32;
    let scans = (hours * 3600.0 / rules.scan_every) as u64;
    for scan in 0..scans {
        let t = scan as f32 * rules.scan_every;
        server.tick = ticks(t);
        server.day = dawn + server.tick as f64;
        // Every STAY seconds the player is somewhere new: home every other time, once there
        // is one, else further out each time along a spiral.
        if scan == 0 || t / STAY > place as f32 {
            place = (t / STAY) as u32 + 1;
            let settled = t >= HOME_AT;
            let at = if settled && place % 2 == 0 {
                IVec2::new(home.x as i32, home.z as i32)
            } else {
                let (a, r) = (place as f32 * 2.399, 90.0 + place as f32 * 45.0);
                IVec2::new((home.x + a.cos() * r) as i32, (home.z + a.sin() * r) as i32)
            };
            let landed = (0..12).find_map(|i| go_to(&mut server, at + IVec2::new(i * 23, i * 17)));
            if landed.is_none() {
                go_to(&mut server, IVec2::new(home.x as i32, home.z as i32));
            }
        }
        train(&mut server, t);
        // The sky moves on, and the land about the player fills with its wild things as it
        // does between scans (spawn tries once a second or so: a few here are plenty).
        let (tick, day) = (server.tick, server.day);
        let here = [xz(server.players[&1].position).floor().as_ivec2()];
        server.weather.advance(&mut server.temperature, &here, tick, day);
        server.despawn_far();
        for salt in 0..6 {
            server.spawn_near(1, scan * 8 + salt);
        }
        if t >= HOME_AT && server.players[&1].home.is_none() {
            server.players.get_mut(&1).unwrap().home = Some(home);
        }
        // Those whose time is up, or left behind, go.
        let at = server.players[&1].position;
        about.retain(|&(id, ends)| {
            let away = server.wandering.active.get(&id).is_none_or(|inst| {
                inst.members.iter().filter_map(|m| server.creatures.live.get(&m.creature)).all(|c| c.body.position.distance(at) > rules.leave)
            });
            if server.tick >= ends || away {
                if let Some(inst) = server.wandering.active.remove(&id) {
                    server.end_event(inst);
                }
                return false;
            }
            true
        });
        // One scan, as `try_wandering`, counting why none came.
        out.tries += 1;
        if !server.wandering_welcome(1) {
            out.unwelcome += 1;
            // One that comes after a party about may still join it.
            let before: Vec<u64> = server.wandering.active.keys().copied().collect();
            server.try_follower(1, 0);
            let joined = server.wandering.active.iter().find(|(id, _)| !before.contains(id)).map(|(id, inst)| (*id, inst.event));
            if let Some((id, event)) = joined {
                let e = server.content.wandering.get(event);
                about.push((id, server.tick + ticks(stays(e))));
                let spot = server.creatures.live[&server.wandering.active[&id].members[0].creature].body.position.floor().as_ivec3();
                let biome = server.overworld.biomes().get(server.overworld.site(spot.x, spot.z).biome).name.clone();
                out.met.push((t, event, biome));
            }
            continue;
        }
        // As `try_wandering` casts it: by the world's count of rolls.
        let roll = mix(server.meta.roll_seed(crate::save::ROLL_PARTY), 0x3a9d);
        if unit_from(mix(roll, 1)) >= rules.chance {
            out.no_chance += 1;
            continue;
        }
        let Some(spot) = server.wandering_spot(1, roll) else {
            out.no_spot += 1;
            continue;
        };
        let (tick, day) = (server.tick, server.day);
        let wet = server.weather.local(&mut server.temperature, xz(spot.as_vec3()), tick, day).weather.precipitation > FALLING;
        out.wet += wet as u32;
        let Some(event) = server.pick_event_in(1, spot, mix(roll, 5), wet) else {
            out.no_pick += 1;
            continue;
        };
        let Some(id) = server.begin_event(Some(1), event, spot, mix(roll, 6)) else {
            out.no_fit += 1;
            continue;
        };
        let e = server.content.wandering.get(event);
        about.push((id, server.tick + ticks(stays(e))));
        let biome = server.overworld.biomes().get(server.overworld.site(spot.x, spot.z).biome).name.clone();
        out.met.push((t, event, biome));
        if unit_from(mix(roll, 7)) < DEALS {
            let name = e.def.name.clone();
            *server.players.get_mut(&1).unwrap().tallies.entry(format!("met:{name}")).or_default() += 1;
        }
    }
    out
}

#[test]
#[ignore = "an audit, run on demand: several minutes of world generation"]
fn wandering_audit() {
    let seeds = [0x5eed_u64, 1, 2, 3, 42, 777, 9, 10, 11, 12, 13, 14];
    let hours = 6.0;
    let names = |server: &Server, i: usize| server.content.wandering.get(i).def.name.clone();
    let probe = world(1);
    let mut kinds: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
    println!("| seed | first party (min) | met in 2 h | met in 6 h | kinds in 6 h | after-kinds | met-kinds | biomes |");
    println!("|---|---|---|---|---|---|---|---|");
    let mut totals = (0usize, 0usize);
    for seed in seeds {
        let w = walk(seed, hours);
        let two = w.met.iter().filter(|m| m.0 < 7200.0).count();
        let distinct: std::collections::BTreeSet<usize> = w.met.iter().map(|m| m.1).collect();
        let after = w.met.iter().filter(|m| probe.content.wandering.get(m.1).def.needs.after.is_some()).count();
        let met = w.met.iter().filter(|m| probe.content.wandering.get(m.1).def.needs.met.is_some()).count();
        let biomes: std::collections::BTreeSet<&str> = w.met.iter().map(|m| m.2.rsplit(':').next().unwrap()).collect();
        let first = w.met.first().map_or("-".into(), |m| format!("{:.0}", m.0 / 60.0));
        println!(
            "| {seed:#x} | {first} | {two} | {} | {} | {after} | {met} | {} |",
            w.met.len(),
            distinct.len(),
            biomes.into_iter().collect::<Vec<_>>().join(", ")
        );
        eprintln!(
            "seed {seed:#x}: tries {} unwelcome {} no chance {} no spot {} no pick {} no fit {} wet {}",
            w.tries, w.unwelcome, w.no_chance, w.no_spot, w.no_pick, w.no_fit, w.wet
        );
        eprintln!("  {}", w.met.iter().map(|m| format!("{:.0}m {}", m.0 / 60.0, names(&probe, m.1).rsplit(':').next().unwrap())).collect::<Vec<_>>().join(", "));
        for m in &w.met {
            let entry = kinds.entry(names(&probe, m.1)).or_default();
            entry.1 += 1;
            if m.0 < 7200.0 {
                entry.0 += 1;
            }
        }
        totals.0 += two;
        totals.1 += w.met.len();
    }
    println!();
    println!("mean met: {:.1} in 2 h, {:.1} in 6 h", totals.0 as f32 / seeds.len() as f32, totals.1 as f32 / seeds.len() as f32);
    let never: Vec<String> = probe.content.wandering.iter().map(|(_, e)| e.def.name.clone()).filter(|n| !kinds.contains_key(n)).collect();
    println!("kinds met (2 h, 6 h): {}", kinds.iter().map(|(k, (a, b))| format!("{} {a}/{b}", k.rsplit(':').next().unwrap())).collect::<Vec<_>>().join(", "));
    println!("never met in {} walks of {hours} h: {} kinds: {}", seeds.len(), never.len(), never.iter().map(|n| n.rsplit(':').next().unwrap()).collect::<Vec<_>>().join(", "));
}

#[test]
#[ignore = "an audit, run on demand"]
fn daylight_through_a_day() {
    let mut buckets = [0u32; 11];
    for t in (0..24_000).step_by(10) {
        buckets[(voxmar_engine::creature::daylight(t) * 10.0).floor() as usize] += 1;
    }
    eprintln!("daylight deciles, share of the day: {:?}", buckets.map(|b| format!("{:.2}", b as f32 / 2400.0)));
}

#[test]
#[ignore = "an audit, run on demand"]
fn wandering_gifts_by_level() {
    let server = crate::test_server();
    let content = &server.content;
    fn items(r: &ResolvedReward, out: &mut Vec<BlockId>) {
        match r {
            ResolvedReward::Item(item, _) => out.push(*item),
            ResolvedReward::OneOf(c) => c.iter().for_each(|r| items(r, out)),
            _ => {}
        }
    }
    let mut rows = Vec::new();
    for (_, e) in content.wandering.iter() {
        let mut add = |line: &str, needs: &[ResolvedNeed], gives: &[ResolvedReward]| {
            let mut got = Vec::new();
            gives.iter().for_each(|r| items(r, &mut got));
            let gate: Vec<String> = needs.iter().filter_map(|n| match n { ResolvedNeed::Skill(s, l) => Some(format!("{}>={l}", content.skills.get(*s).name)), _ => None }).collect();
            for item in got {
                rows.push((voxmar_engine::value::level_of(content, item), e.def.name.clone(), line.to_owned(), content.blocks.def(item).name.clone(), gate.join(" ")));
            }
        };
        for m in &e.meetings {
            for l in &m.lines {
                add(&l.name, &l.needs, &l.gives);
            }
        }
        add("thanks", &[], &e.thanks);
        add("guide_thanks", &[], &e.guide_thanks);
        add("reveal", &[], &e.reveal);
        add("show", &[], &e.show);
        add("share", &[], &e.share);
    }
    rows.sort();
    for r in rows.iter().rev() {
        eprintln!("{:>3} {} {} {} {}", r.0, r.1, r.2, r.3, r.4);
    }
}
