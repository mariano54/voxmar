//! Bosses in their lairs (`lairs.ron`, docs/bosses.md): worldgen digs the rooms; here a
//! lair's boss is put on its floor when a player comes near, a lair stands empty for its
//! `rest` once its boss falls (saved), no wild creature is born in its `quiet`, and a
//! creature that mends (`moves::Mend`) heals between blows unless fire or the sun stops it.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::moves::MendStoppers;
use voxmar_engine::glam::{IVec2, IVec3, Vec3};
use voxmar_engine::lang::Words;
use voxmar_engine::schedule::Tier;
use voxmar_protocol::{ChatKind, ServerMessage, TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::Server;
use crate::process::Process;

/// How far around a player lairs are looked for, in blocks: past every lair's `wakes`.
const LOOK: i32 = 96;
/// Blocks within which players hear a boss come to a new phase of its fight.
const HEARD: f32 = 32.0;

/// The bosses in their lairs, by the middle of each lair's floor.
#[derive(Default)]
pub(crate) struct Lairs {
    bosses: FxHashMap<IVec3, u32>,
    /// The day clock until which a lair whose boss fell stands empty.
    resting: FxHashMap<IVec3, f64>,
}

/// A lair resting, as saved.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLair {
    pub heart: IVec3,
    pub until: f64,
}

impl Lairs {
    pub(crate) fn saved(&self) -> Vec<SavedLair> {
        let mut saved: Vec<SavedLair> = self.resting.iter().map(|(&heart, &until)| SavedLair { heart, until }).collect();
        saved.sort_by_key(|l| (l.heart.x, l.heart.y, l.heart.z));
        saved
    }

    pub(crate) fn restore(saved: &[SavedLair]) -> Self {
        Self { resting: saved.iter().map(|l| (l.heart, l.until)).collect(), ..Self::default() }
    }
}

/// Wakes lairs' bosses and mends what mends, every second.
#[derive(Default)]
pub(crate) struct Bosses;

impl Process for Bosses {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        server.wake_lairs();
        server.mend_creatures();
    }
}

impl Server {
    /// Puts each lair's boss on its floor once a player comes within its `wakes`, unless
    /// it is there already or the lair rests; forgets bosses gone without dying (out of
    /// every player's reach), who come back whole.
    pub(crate) fn wake_lairs(&mut self) {
        let live = &self.creatures.live;
        self.lairs.bosses.retain(|_, id| live.contains_key(id));
        let day = self.day;
        self.lairs.resting.retain(|_, until| *until > day);
        let players: Vec<Vec3> = self.players.values().filter(|p| p.joined && p.health.alive()).map(|p| p.position).collect();
        let defs = self.overworld.lair_defs();
        let mut wake = Vec::new();
        for at in players {
            for site in self.overworld.lairs_near(IVec2::new(at.x as i32, at.z as i32), LOOK) {
                let def = &defs[site.lair];
                let floor = site.heart.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
                if floor.distance(at) > def.wakes || self.lairs.bosses.contains_key(&site.heart) || self.lairs.resting.contains_key(&site.heart) {
                    continue;
                }
                // Only in a hollow that was dug: land made before lairs were (an old save's)
                // is solid where her heart would be, and no boss wakes inside the ground.
                let open = |pos: IVec3| self.block(pos).is_some_and(|b| !self.content.blocks.info(b).solid);
                let dug = open(site.heart) && open(site.heart + IVec3::Y);
                if dug && self.world.contains(ColumnPos::of_block(site.heart)) && !wake.iter().any(|(h, _)| *h == site.heart) {
                    wake.push((site.heart, def.boss.clone()));
                }
            }
        }
        for (heart, boss) in wake {
            let Some(species) = self.content.creatures.id(&boss) else { continue };
            let floor = heart.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
            let seed = voxmar_engine::creature::mix(self.meta.seed, (heart.x as u64) << 32 ^ heart.z as u32 as u64);
            if let Some(id) = self.place_creature(species, floor, seed, 0.0) {
                self.start_rolls(id);
                self.lairs.bosses.insert(heart, id);
            }
        }
    }

    /// A lair's boss fell: its lair rests.
    pub(crate) fn lair_boss_died(&mut self, id: u32) {
        let Some((&heart, _)) = self.lairs.bosses.iter().find(|&(_, &b)| b == id) else { return };
        self.lairs.bosses.remove(&heart);
        let site = IVec2::new(heart.x, heart.z);
        let rest = self.overworld.lairs_near(site, 1).first().map_or(1.0, |s| self.overworld.lair_defs()[s.lair].rest);
        self.lairs.resting.insert(heart, self.day + rest as f64 * TICKS_PER_DAY as f64);
    }

    /// Whether `spot` lies in a lair's `quiet`, where no wild creature is born.
    pub(crate) fn in_lair_quiet(&self, spot: IVec3) -> bool {
        let defs = self.overworld.lair_defs();
        let most = defs.iter().map(|d| d.quiet).fold(0.0f32, f32::max);
        most > 0.0
            && self.overworld.lairs_near(IVec2::new(spot.x, spot.z), most.ceil() as i32).iter().any(|s| s.heart.as_vec3().distance(spot.as_vec3()) <= defs[s.lair].quiet)
    }

    /// Heals each creature that mends (`moves::Mend`) by its `health` every `every`
    /// seconds, once `after` seconds have passed without a blow landing on it, unless a
    /// fire burns within its `fire_reach` or, under open sky, the day is bright enough.
    pub(crate) fn mend_creatures(&mut self) {
        let tick = self.tick;
        let daylight = self.daylight();
        let mut mended = Vec::new();
        for (&id, live) in &self.creatures.live {
            let species = self.content.creatures.get(live.species);
            let Some(mend) = &species.def.mend else { continue };
            let every = (mend.every * TICKS_PER_SECOND as f32) as u64;
            let after = (mend.after * TICKS_PER_SECOND as f32) as u64;
            if live.health >= species.def.health || tick < live.mended + every || live.fought.is_some_and(|(_, at)| tick < at + after) {
                continue;
            }
            let cell = live.body.position.floor().as_ivec3();
            let now = MendStoppers { fire: self.fire_within(cell, &mend.fire, mend.fire_reach), daylight, open_sky: self.open_sky(cell + IVec3::Y) };
            if mend.mends(now) {
                mended.push((id, mend.health, species.def.health));
            }
        }
        for (id, health, most) in mended {
            if let Some(live) = self.creatures.live.get_mut(&id) {
                live.health = (live.health + health).min(most);
                live.mended = tick;
                live.changed = true;
            }
        }
    }

    /// Whether a block with any of `tags` lies within `reach` blocks of `at`.
    pub(crate) fn fire_within(&self, at: IVec3, tags: &[String], reach: f32) -> bool {
        let r = reach.ceil() as i32;
        let blocks = &self.content.blocks;
        (-r..=r).any(|dy| {
            (-r..=r).any(|dz| {
                (-r..=r).any(|dx| {
                    let d = IVec3::new(dx, dy, dz);
                    d.as_vec3().length() <= reach && self.block(at + d).is_some_and(|b| !b.is_air() && tags.iter().any(|t| blocks.def(b).has_tag(t)))
                })
            })
        })
    }

    /// A boss that came to a new phase of its fight (`Creature::phase`) rears for the
    /// phase's `hold`, its wind-up undone, and players near are told.
    pub(crate) fn phase_check(&mut self, id: u32) {
        let Some(live) = self.creatures.live.get(&id) else { return };
        let species = self.content.creatures.get(live.species);
        let phase = species.phase(live.health as f32 / species.def.health as f32);
        if phase <= live.phase {
            return;
        }
        let hold = species.def.phases[phase as usize - 1].hold;
        let (name, at) = (species.def.name.clone(), live.body.position);
        self.fights_forget(id);
        let tick = self.tick;
        let live = self.creatures.live.get_mut(&id).expect("checked above");
        live.phase = phase;
        live.rearing = true;
        live.winding = false;
        live.tracking = None;
        live.held_until = tick + (hold * TICKS_PER_SECOND as f32).round() as u64;
        live.changed = true;
        let (ns, short) = name.split_once(':').unwrap_or(("base", &name));
        let key = format!("boss.{ns}.{short}.phase.{phase}");
        for player in self.players.values().filter(|p| p.joined && p.position.distance(at) <= HEARD) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key(&key) });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::block::BlockId;
    use voxmar_engine::creature::CreatureId;
    use voxmar_engine::world::Column;

    use super::*;

    const FLOOR: i32 = 199;

    /// A flat stone floor at y 199 over a few columns from the origin, and player 1
    /// standing on it at `at`.
    fn fixture(at: Vec3) -> Server {
        fixture_on(crate::test_content(), at)
    }

    /// As `fixture`, on content already loaded.
    fn fixture_on(content: voxmar_engine::content::Content, at: Vec3) -> Server {
        let mut server = crate::test_server_on(content);
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        for cx in 0..3 {
            for cz in 0..3 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        column.set_block(IVec3::new(x, FLOOR, z), stone);
                    }
                }
                server.world.insert(ColumnPos::new(cx, cz), Arc::new(column));
            }
        }
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        server
    }

    fn rootmother(server: &Server) -> CreatureId {
        server.content.creatures.id("base:rootmother").expect("the Rootmother")
    }

    /// The Rootmother on the floor at (x, z), wounded to `health`.
    fn summon(server: &mut Server, x: f32, z: f32, health: u16) -> u32 {
        let id = server.place_creature(rootmother(server), Vec3::new(x, FLOOR as f32 + 1.0, z), 9, 0.0).expect("placed");
        server.creatures.live.get_mut(&id).unwrap().health = health;
        id
    }

    fn day_at(light: impl Fn(f32) -> bool) -> f64 {
        (0..200).map(|i| TICKS_PER_DAY as f64 * (4.0 + i as f64 / 200.0)).find(|&d| light(voxmar_engine::creature::daylight(d as u64))).expect("such an hour")
    }

    fn health(server: &Server, id: u32) -> u16 {
        server.creatures.live[&id].health
    }

    /// Waits out her `after` and `every`, then mends once.
    fn mend_later(server: &mut Server) {
        server.tick += 10 * TICKS_PER_SECOND as u64;
        server.mend_creatures();
    }

    /// Cut her and she grows back: in the dark she mends between blows, not while they
    /// still land; a campfire within four blocks stops it, and so does the sun on her
    /// back, never the day over a roof.
    #[test]
    fn the_rootmother_mends_unless_fire_or_the_sun_stops_her() {
        let mut server = fixture(Vec3::new(20.5, 200.0, 30.5));
        server.day = day_at(|l| l <= 0.0);
        let id = summon(&mut server, 20.5, 20.5, 60);
        let mend = server.content.creatures.get(rootmother(&server)).def.mend.clone().unwrap();
        mend_later(&mut server);
        assert_eq!(health(&server, id), 60 + mend.health, "mends in the dark");
        // A blow just landed: not yet.
        server.creatures.live.get_mut(&id).unwrap().fought = Some((1, server.tick + 10 * TICKS_PER_SECOND as u64));
        mend_later(&mut server);
        assert_eq!(health(&server, id), 60 + mend.health, "not while blows land");
        server.creatures.live.get_mut(&id).unwrap().fought = None;
        // A campfire three blocks off.
        let campfire = server.content.blocks.expect("base:campfire");
        server.set_block(IVec3::new(23, FLOOR + 1, 20), campfire);
        mend_later(&mut server);
        assert_eq!(health(&server, id), 60 + mend.health, "a fire near her stops it");
        server.set_block(IVec3::new(23, FLOOR + 1, 20), BlockId::AIR);
        // Broad day, open sky: the sun on her.
        server.day = day_at(|l| l >= 1.0);
        mend_later(&mut server);
        assert_eq!(health(&server, id), 60 + mend.health, "the sun on her stops it");
        // Under a roof the day is nothing to her.
        let stone = server.content.blocks.expect("base:stone");
        server.set_block(IVec3::new(20, FLOOR + 6, 20), stone);
        mend_later(&mut server);
        assert_eq!(health(&server, id), 60 + 2 * mend.health, "under a roof she mends by day");
    }

    /// Below half she rears into her second phase, held still, her wind-up undone, and
    /// players near are told; then a player who keeps away draws her volley, one beside
    /// her the thrash or the grasp.
    #[test]
    fn the_rootmother_rears_into_her_second_phase_and_volleys_at_range() {
        let mut server = fixture(Vec3::new(20.5, 200.0, 28.5));
        let id = summon(&mut server, 20.5, 20.5, 64);
        server.wind_up(id, 1);
        assert_eq!(server.creatures.live[&id].attack, 0, "out of her first phase's reach, no move fits");
        assert!(!server.creatures.live[&id].winding);
        server.hurt_creature(1, id, 6, None, voxmar_engine::combat::Blow::Plain);
        let live = &server.creatures.live[&id];
        assert_eq!((live.phase, live.rearing), (1, true));
        assert!(live.held_until > server.tick && live.view(id).tell == voxmar_protocol::REARING);
        let her = server.content.creatures.get(rootmother(&server));
        let index = |name: &str| her.def.moves.iter().position(|m| m.name == name).unwrap() as u8 + 1;
        let (volley, thrash, grasp) = (index("volley"), index("thrash"), index("grasp"));
        server.creatures.live.get_mut(&id).unwrap().rearing = false;
        server.wind_up(id, 1);
        assert_eq!(server.creatures.live[&id].attack, volley, "eight blocks off draws the volley");
        assert_eq!(server.creatures.live[&id].view(id).tell, volley, "and shows it");
        server.fights_forget(id);
        server.players.get_mut(&1).unwrap().position = Vec3::new(20.5, 200.0, 22.4);
        server.creatures.live.get_mut(&id).unwrap().winding = false;
        server.wind_up(id, 1);
        assert!([thrash, grasp].contains(&server.creatures.live[&id].attack), "beside her, the thrash or the grasp");
    }

    /// She always comes to blows with a player who stands still: never stops where her
    /// brain judges her close but no move of hers fits. A small Rootmother just past her
    /// grasp (her brain once measured her at her species' size, her wind-up at her own), and
    /// in her second phase a player between her thrash and her volley, both draw a wind-up.
    #[test]
    fn the_rootmother_never_stands_where_no_move_of_hers_fits() {
        let her = |server: &Server| server.content.creatures.get(rootmother(server)).def.clone();
        // The content loaded once for all the cases' servers.
        let content = crate::test_content();
        let edge_of_reach = {
            let server = fixture_on(content.clone(), Vec3::ZERO);
            let def = her(&server);
            // Her grasp's reach from her edge, at her smallest, and a little past it.
            def.moves.iter().map(|m| m.attack.reach).filter(|&r| r < 3.0).fold(0.0, f32::max) + def.body.size.0 * 0.88 * 0.5 + 0.3 + 0.05
        };
        let mut failed = Vec::new();
        for scale in [0.88, 1.0, 1.12] {
        for (health, gap) in [(120, edge_of_reach), (40, 3.0 + 0.9 + 0.3), (120, 3.0), (120, 3.5), (120, 4.0), (120, 4.5), (120, 5.0), (120, 5.5), (120, 6.0), (40, 5.0), (40, 6.0)] {
            let mut server = fixture_on(content.clone(), Vec3::new(20.5, 200.0, 20.5 + gap));
            server.day = day_at(|l| l <= 0.0);
            let id = summon(&mut server, 20.5, 20.5, health);
            let live = server.creatures.live.get_mut(&id).unwrap();
            (live.scale, live.phase) = (scale, if health < 60 { 1 } else { 0 });
            let wound_up = (0..10 * TICKS_PER_SECOND).any(|_| {
                server.tick += 1;
                server.tick_creatures();
                server.creatures.live.get(&id).is_some_and(|live| live.winding)
            });
            let at = server.creatures.live[&id].body.position;
            if !wound_up { failed.push(format!("scale {scale} health {health} gap {gap:.2}: at {at:.2}")); }
        }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }

    /// A boss always fights whoever comes to her: never leaves be a player who has stood
    /// about one spot past `TOLERANCE` (the gate's session, long on a busy machine, once
    /// met her so and she stood idle), whatever their combat level.
    #[test]
    fn a_boss_never_tolerates_a_player() {
        let mut server = fixture(Vec3::new(20.5, 200.0, 24.5));
        server.day = day_at(|l| l <= 0.0);
        // Ten minutes and more within a few blocks of here.
        server.creatures.settled.insert(1, (Vec3::new(20.5, 200.0, 28.5), server.tick));
        server.tick += crate::creatures::TOLERANCE + 1;
        let id = summon(&mut server, 20.5, 20.5, 120);
        let wound_up = (0..10 * TICKS_PER_SECOND).any(|_| {
            server.tick += 1;
            server.tick_creatures();
            server.creatures.live.get(&id).is_some_and(|live| live.winding)
        });
        assert!(wound_up, "she left a settled player be: {:?}", server.creatures.live[&id].mind);
    }

    /// A ward stops only `boss_ward` of a boss's blow (half), where it stops `ward` of
    /// another's (four fifths).
    #[test]
    fn a_ward_stops_half_a_bosss_blow() {
        let mut server = fixture(Vec3::new(20.5, 200.0, 22.4));
        let id = summon(&mut server, 20.5, 20.5, 120);
        let ward = server.content.prayers.id("base:ward_slash").unwrap();
        let taken = |server: &mut Server, creature: u32, warded: bool| -> u32 {
            let species = server.creatures.live[&creature].species;
            let player = server.players.get_mut(&1).unwrap();
            player.praying.on = if warded { vec![ward] } else { Vec::new() };
            // Both runs draw the same 400 blows, so they differ only by the ward.
            player.draws.blows = 0;
            (0..400u64)
                .map(|_| {
                    let player = server.players.get_mut(&1).unwrap();
                    player.health.current = player.health.max;
                    server.creature_strikes(creature, species, 1, 1.0).unwrap_or(0) as u32
                })
                .sum()
        };
        let (boss_bare, boss_warded) = (taken(&mut server, id, false), taken(&mut server, id, true));
        let share = boss_warded as f32 / boss_bare as f32;
        let rules = server.content.prayers.rules;
        assert!((share - (1.0 - rules.boss_ward)).abs() < 0.12, "a ward lets {share} of her lash through");
        assert!(rules.boss_ward < rules.ward);
    }

    /// A lair's boss is there when a player comes near, and not twice; once it falls the
    /// lair rests a day (kept through a save), then she is back. No wild creature is born
    /// within its quiet.
    #[test]
    fn a_lair_wakes_its_boss_and_rests_once_she_falls() {
        let mut server = crate::test_server();
        // A world with old woods near its spawn (the test world's seed has none for miles).
        let land = voxmar_worldgen::WorldContent::load(&server.content).unwrap();
        server.overworld = voxmar_worldgen::Overworld::shared(1, &server.content, &land);
        let site = server.overworld.lairs_near(IVec2::ZERO, 2000).into_iter().next().expect("a lair within 2 km");
        let def = server.overworld.lair_defs()[site.lair].clone();
        server.world = Default::default();
        server.world.insert(ColumnPos::of_block(site.heart), Arc::new(Column::default()));
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let far = site.heart.as_vec3() + Vec3::new(def.wakes + 10.0, 0.0, 0.0);
        let mut player = crate::player::Player::new(1, connection, far, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        server.wake_lairs();
        assert!(server.creatures.live.is_empty(), "too far to wake her");
        server.players.get_mut(&1).unwrap().position = site.heart.as_vec3() + Vec3::new(def.wakes - 8.0, 0.0, 0.0);
        // Land generated before lairs were has no hollow: she does not wake in the stone.
        let stone = server.content.blocks.expect("base:stone");
        server.world.set_block(site.heart, stone);
        server.wake_lairs();
        assert!(server.creatures.live.is_empty(), "no hollow dug, no boss");
        server.world.set_block(site.heart, BlockId::AIR);
        server.wake_lairs();
        let bosses: Vec<u32> = server.creatures.live.keys().copied().collect();
        assert_eq!(bosses.len(), 1, "one boss in her lair");
        let id = bosses[0];
        assert_eq!(server.content.creatures.get(server.creatures.live[&id].species).def.name, def.boss);
        assert!(server.creatures.live[&id].body.position.distance(site.heart.as_vec3() + Vec3::new(0.5, 0.0, 0.5)) < 0.01);
        assert!(server.in_lair_quiet(site.heart) && !server.in_lair_quiet(site.heart + IVec3::new(def.quiet as i32 + 400, 0, 0)));
        server.hurt_creature(1, id, 999, None, voxmar_engine::combat::Blow::Plain);
        assert!(server.creatures.live.is_empty());
        server.wake_lairs();
        assert!(server.creatures.live.is_empty(), "her lair rests");
        let saved = server.lairs.saved();
        assert_eq!(saved.len(), 1);
        server.lairs = Lairs::restore(&saved);
        server.day += def.rest as f64 * TICKS_PER_DAY as f64 * 0.5;
        server.wake_lairs();
        assert!(server.creatures.live.is_empty(), "still resting after a save, half a day on");
        server.day += def.rest as f64 * TICKS_PER_DAY as f64 * 0.6;
        server.wake_lairs();
        assert_eq!(server.creatures.live.len(), 1, "back after her rest");
        assert!(server.lairs.saved().is_empty());
    }
}
