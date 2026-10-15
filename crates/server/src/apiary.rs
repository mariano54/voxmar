//! Bees on the server (`bees.ron`, docs/bees.md): taking a swarm or honey, the stings of
//! bees taken from without smoke, and the kept hives, which the apiary looks at every few
//! seconds: their bees forage the flowers and crops about them by day in dry weather and
//! fill the hive with honey, fly off from a fire beside it, sting creatures that come
//! close, and lose a full hive's honey to a raiding bear. Crops near a hive with bees grow
//! faster (`farm`, `pollination`). A hive in a column nobody has loaded waits.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::dropped;
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::process::Process;

/// The kept hives with bees in them, and the seconds each has foraged toward its honey.
#[derive(Default)]
pub struct Apiary {
    hives: FxHashMap<IVec3, f32>,
}

/// A kept hive as `world.ron` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedHive {
    pub position: [i32; 3],
    pub foraged: f32,
}

impl Apiary {
    pub fn saved(&self) -> Vec<SavedHive> {
        let mut saved: Vec<SavedHive> = self.hives.iter().map(|(pos, &foraged)| SavedHive { position: pos.to_array(), foraged }).collect();
        saved.sort_by_key(|h| h.position);
        saved
    }

    pub fn restore(saved: &[SavedHive]) -> Self {
        Self { hives: saved.iter().map(|h| (IVec3::from_array(h.position), h.foraged)).collect() }
    }

    /// The seconds the hive at `pos` has foraged, if it is one.
    #[cfg(test)]
    pub fn foraged(&self, pos: IVec3) -> Option<f32> {
        self.hives.get(&pos).copied()
    }
}

/// `base:bees`: looks at the kept hives every `survey_seconds`.
#[derive(Default)]
pub(crate) struct Bees {
    last: u64,
}

impl Process for Bees {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let Some(hive) = &server.content.bees.hive else { return };
        let every = (hive.def.survey_seconds * TICKS_PER_SECOND as f32).round().max(1.0) as u64;
        if server.tick < self.last + every {
            return;
        }
        let seconds = (server.tick - self.last) as f32 / TICKS_PER_SECOND as f32;
        self.last = server.tick;
        server.survey_hives(seconds.min(hive.def.survey_seconds * 2.0));
    }
}

impl Server {
    /// A block changed: a hive with bees is known to the apiary (a new one from nothing),
    /// anything else is not.
    pub(crate) fn hive_changed(&mut self, pos: IVec3, old: voxmar_engine::block::BlockId, block: voxmar_engine::block::BlockId) {
        let Some(hive) = &self.content.bees.hive else { return };
        if block == hive.busy && old != hive.busy {
            self.apiary.hives.insert(pos, 0.0);
        } else if block == hive.full {
            self.apiary.hives.entry(pos).or_insert(0.0);
        } else if old != block {
            self.apiary.hives.remove(&pos);
        }
    }

    /// `agent` takes from the bees at `pos` with what they hold (`base:tend_bees`).
    pub(crate) fn tend_bees(&mut self, agent: PlayerId, pos: IVec3) {
        let bees = self.content.bees.clone();
        let Some(block) = self.block(pos) else { return };
        let Some(player) = self.players.get(&agent) else { return };
        let Some(take) = bees.take(block, player.held()).cloned() else { return };
        let tell = |server: &Self, words: Words| {
            if let Some(player) = server.players.get(&agent) {
                player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
            }
        };
        if self.level(agent, Some(take.skill)) < take.level {
            tell(self, Words::key("bees.level").with("level", Words::Text(take.level.to_string())));
            return;
        }
        if !self.smoked(pos, Some(agent)) {
            self.sting(agent);
            if take.needs_smoke {
                tell(self, Words::key("bees.angry"));
                return;
            }
        }
        if let (Some(_), Some(becomes)) = (take.holding, take.held_becomes) {
            let player = self.players.get_mut(&agent).expect("checked above");
            let selected = player.selected;
            let left = player.inventory.replace_one(selected, becomes);
            player.changed(traits::INVENTORY);
            if left > 0 {
                let body = dropped::popped(pos, self.meta.next_pop(pos, 0xbee0));
                self.items.spawn(ItemStack::new(becomes, left), body, dropped::PICKUP_DELAY);
            }
        }
        // Each draw numbered by the player's count of harvest draws, and where a drop pops
        // by the world's count of drops, never by the tick it is taken on.
        let player = self.players.get_mut(&agent).expect("checked above");
        let found = voxmar_engine::crop::roll(&take.gives, 0.0, || player.draw(crate::save::Draw::Harvest)[0]);
        for (i, (item, count)) in found.into_iter().enumerate() {
            let body = dropped::popped(pos, self.meta.next_pop(pos, 0xbee2 + i as u32));
            self.items.spawn(ItemStack::new(item, count), body, dropped::PICKUP_DELAY);
        }
        self.set_block(pos, take.becomes);
        self.grant_xp(agent, take.skill, take.xp);
    }

    /// Whether bees at `pos` are smoked calm: a smoking block (a campfire) within reach of
    /// them, or `agent` smoky (`bees.ron`'s `smoky`: smoke clings to them a while after a
    /// torch in hand or a fire close by, so they can wave one and then take up the skep).
    pub(crate) fn smoked(&self, pos: IVec3, agent: Option<PlayerId>) -> bool {
        let Some(hive) = &self.content.bees.hive else { return true };
        if let (Some(smoky), Some(player)) = (hive.smoky, agent.and_then(|a| self.players.get(&a)))
            && player.timed_effects.iter().any(|&(e, _)| e == smoky)
        {
            return true;
        }
        self.smoke_within(pos, hive.def.smoke_reach)
    }

    fn smoke_within(&self, pos: IVec3, reach: i32) -> bool {
        let Some(hive) = &self.content.bees.hive else { return false };
        let blocks = &self.content.blocks;
        (-reach..=reach).any(|dy| (-reach..=reach).any(|dz| (-reach..=reach).any(|dx| self.block(pos + IVec3::new(dx, dy, dz)).is_some_and(|b| hive.smokes(blocks, b)))))
    }

    /// Bees sting `agent`: a little damage, never the last of their health, and a while stung.
    fn sting(&mut self, agent: PlayerId) {
        let Some(hive) = &self.content.bees.hive else { return };
        let (damage, effect, seconds) = (hive.def.sting, hive.stung, hive.def.stung_seconds);
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&agent) else { return };
        if player.allow_flight {
            return;
        }
        let damage = damage.min(player.health.current.saturating_sub(1));
        if damage > 0 {
            player.wound(damage, tick);
            player.changed(traits::HEALTH);
        }
        player.give_effects(&[(effect, seconds)]);
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("bees.stung") });
    }

    /// A shot that landed at `at` (hit or miss) stirs bees within `shot_reach` of it: unless
    /// smoke calms all of them, they sting `shooter`.
    pub(crate) fn stir_bees(&mut self, shooter: PlayerId, at: Vec3) {
        let Some(hive) = &self.content.bees.hive else { return };
        let reach = hive.def.shot_reach;
        if reach <= 0.0 {
            return;
        }
        let (blocks, r, centre) = (&self.content.blocks, reach.ceil() as i32, at.floor().as_ivec3());
        // Any bees within reach that smoke does not calm are enough: a smoked hive beside an
        // unsmoked nest leaves the nest's bees as they are.
        let stirred = (-r..=r)
            .flat_map(|dy| (-r..=r).flat_map(move |dz| (-r..=r).map(move |dx| centre + IVec3::new(dx, dy, dz))))
            .filter(|cell| (cell.as_vec3() + Vec3::splat(0.5)).distance(at) <= reach)
            .filter(|&cell| self.block(cell).is_some_and(|b| blocks.def(b).has_tag(&hive.def.bees)))
            .any(|cell| !self.smoked(cell, Some(shooter)));
        if stirred {
            self.sting(shooter);
        }
    }

    /// How much faster a crop at `pos` grows for the bees near it: `pollinate` within
    /// reach of a hive with bees, else 1.
    pub(crate) fn pollination(&self, pos: IVec3) -> f32 {
        let Some(hive) = &self.content.bees.hive else { return 1.0 };
        let reach = hive.def.pollinate_reach;
        let near = self.apiary.hives.keys().any(|h| (h.as_vec3() - pos.as_vec3()).length() <= reach);
        if near { hive.def.pollinate } else { 1.0 }
    }

    /// One look at every kept hive, `seconds` after the last.
    pub(crate) fn survey_hives(&mut self, seconds: f32) {
        let Some(hive) = self.content.bees.hive.clone() else { return };
        let def = &hive.def;
        let daylight = self.daylight();
        let mut hives: Vec<(IVec3, f32)> = self.apiary.hives.iter().map(|(&p, &f)| (p, f)).collect();
        hives.sort_by_key(|(p, _)| p.to_array());
        for (pos, foraged) in hives {
            if !self.world.contains(ColumnPos::of_block(pos)) {
                continue;
            }
            let Some(block) = self.block(pos).filter(|&b| hive.has_bees(b)) else {
                self.apiary.hives.remove(&pos);
                continue;
            };
            // A fire beside the hive drives its bees off.
            if self.smoke_within(pos, def.scorch_reach) {
                self.set_block(pos, hive.empty);
                continue;
            }
            // Creatures that come close are stung; a raider eats a full hive's honey.
            let centre = pos.as_vec3() + 0.5;
            let near: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| c.body.position.distance(centre) <= def.creature_reach).map(|(&id, _)| id).collect();
            let mut raided = false;
            for id in near {
                let Some(creature) = self.creatures.live.get_mut(&id) else { continue };
                if hive.raiders.contains(&creature.species) {
                    raided |= block == hive.full;
                } else if !hive.unstung.contains(&creature.species) {
                    let sting = def.creature_sting.min(creature.health.saturating_sub(1));
                    if sting > 0 {
                        creature.health -= sting;
                        creature.changed = true;
                    }
                }
            }
            if raided {
                self.set_block(pos, hive.busy);
                continue;
            }
            if block != hive.busy {
                continue;
            }
            let (tick, day) = (self.tick, self.day);
            let at = Vec2::new(centre.x, centre.z);
            let precipitation = self.weather.local(&mut self.temperature, at, tick, day).weather.precipitation;
            if !hive.out(precipitation, daylight) {
                continue;
            }
            let pace = hive.pace(self.nectar_around(pos));
            let foraged = foraged + seconds * pace;
            if foraged >= def.minutes * 60.0 {
                self.set_block(pos, hive.full);
            } else {
                self.apiary.hives.insert(pos, foraged);
            }
        }
    }

    /// The nectar the flowers and crops within a hive's reach give its bees.
    pub(crate) fn nectar_around(&self, pos: IVec3) -> f32 {
        let Some(hive) = &self.content.bees.hive else { return 0.0 };
        let blocks = &self.content.blocks;
        let (reach, height) = (hive.def.forage_reach, hive.def.forage_height);
        let mut nectar = 0.0;
        for dy in -height..=height {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    if let Some(block) = self.block(pos + IVec3::new(dx, dy, dz)).filter(|b| !b.is_air()) {
                        nectar += hive.nectar(blocks, block);
                    }
                }
            }
        }
        nectar
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::block::BlockId;
    use voxmar_engine::glam::Vec3;
    use voxmar_engine::world::Column;
    use voxmar_protocol::TICKS_PER_DAY;

    use super::*;

    const HIVE: IVec3 = IVec3::new(40, 200, 40);

    /// A flat stone floor at y 199, broad day, and player 1 by the hive's spot with
    /// Farming 12.
    fn fixture() -> Server {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        for cx in 0..3 {
            for cz in 0..3 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        column.set_block(IVec3::new(x, 199, z), stone);
                    }
                }
                server.world.insert(ColumnPos::new(cx, cz), Arc::new(column));
            }
        }
        server.day = noon();
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = HIVE.as_vec3() + Vec3::new(2.5, 0.0, 0.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        let farming = server.content.skills.id("core:farming").unwrap();
        player.skills.grant(farming, server.content.skills.xp_for(12));
        server.players.insert(1, player);
        server
    }

    fn noon() -> f64 {
        (0..100).map(|i| TICKS_PER_DAY as f64 * (4.0 + i as f64 / 100.0)).find(|&d| voxmar_engine::creature::daylight(d as u64) >= 1.0).expect("a bright hour")
    }

    fn block(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    fn hold(server: &mut Server, name: &str) {
        let item = block(server, name);
        let player = server.players.get_mut(&1).unwrap();
        player.inventory = Default::default();
        player.inventory.add(item, 1);
        player.selected = 0;
    }

    fn held(server: &Server) -> Option<BlockId> {
        server.players[&1].held()
    }

    /// Flowers enough about the hive for its bees to forage at full pace.
    fn flowers(server: &mut Server) {
        let lily = block(server, "base:honeydew_lily");
        for i in 0..4 {
            server.set_block(HIVE + IVec3::new(-3, 0, i - 2), lily);
        }
    }

    fn hive(server: &Server) -> &voxmar_engine::bees::Hive {
        server.content.bees.hive.as_ref().expect("base has a hive")
    }

    /// Fire and smoke: a swarm goes into a skep only under smoke; without it the bees
    /// sting (never the last health) and stay. With a campfire near, the skep in hand
    /// becomes a skep of bees and the nest is gone.
    #[test]
    fn a_swarm_goes_into_a_skep_only_under_smoke() {
        let mut server = fixture();
        server.set_block(HIVE, block(&server, "base:bee_nest"));
        hold(&mut server, "base:skep");
        let health = server.players[&1].health.current;
        server.tend_bees(1, HIVE);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:bee_nest")), "no smoke: the swarm stays");
        assert_eq!(held(&server), Some(block(&server, "base:skep")));
        assert!(server.players[&1].health.current < health, "stung");
        assert!(server.players[&1].timed_effects.iter().any(|(e, _)| *e == hive(&server).stung));
        server.set_block(HIVE + IVec3::new(0, 0, 3), block(&server, "base:campfire"));
        server.tend_bees(1, HIVE);
        assert!(server.block(HIVE).is_some_and(|b| b.is_air()), "the swarm is taken");
        assert_eq!(held(&server), Some(block(&server, "base:bee_skep")));
        // A sting never takes the last health.
        server.players.get_mut(&1).unwrap().health.current = 1;
        server.set_block(HIVE + IVec3::new(0, 0, 3), BlockId::AIR);
        server.set_block(HIVE, block(&server, "base:bee_nest"));
        hold(&mut server, "base:skep");
        server.tend_bees(1, HIVE);
        assert_eq!(server.players[&1].health.current, 1);
    }

    /// Smoke clings: a torch waved in hand leaves the player smoky (effects.ron's `smoky`,
    /// lingering), so the swarm goes into the skep taken up after it; once the smoke has
    /// worn off, the bees sting again. A campfire close by makes them smoky too.
    #[test]
    fn smoke_from_a_torch_in_hand_clings_while_the_skep_is_taken_up() {
        use crate::process::Process;
        let mut server = fixture();
        let smoky = hive(&server).smoky.expect("base bees know smoke that clings");
        let is_smoky = |server: &Server| server.players[&1].timed_effects.iter().any(|&(e, _)| e == smoky);
        let nest = block(&server, "base:bee_nest");
        server.set_block(HIVE, nest);
        hold(&mut server, "base:torch");
        crate::process::Survival.run(&mut server);
        assert!(is_smoky(&server), "a torch in hand smokes the player");
        hold(&mut server, "base:skep");
        crate::process::Survival.run(&mut server);
        assert!(is_smoky(&server), "the smoke lingers with the skep in hand");
        let health = server.players[&1].health.current;
        server.tend_bees(1, HIVE);
        assert!(server.block(HIVE).is_some_and(|b| b.is_air()), "the swarm is taken");
        assert_eq!(held(&server), Some(block(&server, "base:bee_skep")));
        assert_eq!(server.players[&1].health.current, health, "smoked bees do not sting");
        // Worn off: the bees sting and stay.
        server.players.get_mut(&1).unwrap().timed_effects.retain(|&(e, _)| e != smoky);
        server.set_block(HIVE, nest);
        hold(&mut server, "base:skep");
        server.tend_bees(1, HIVE);
        assert_eq!(server.block(HIVE), Some(nest), "no smoke left: the swarm stays");
        // A campfire close by: smoky again, without one in hand.
        let feet = server.players[&1].position.floor().as_ivec3();
        server.set_block(feet + IVec3::new(0, 0, -2), block(&server, "base:campfire"));
        server.players.get_mut(&1).unwrap().timed_effects.clear();
        crate::process::Survival.run(&mut server);
        assert!(is_smoky(&server), "a fire close by smokes the player");
    }

    /// A full skep gives its honey whether smoked or not, stinging without smoke, and the
    /// bees stay to fill it again. Below the level bees ask, nothing.
    #[test]
    fn a_full_skep_gives_honey_and_its_bees_stay() {
        let mut server = fixture();
        server.set_block(HIVE, block(&server, "base:honey_skep"));
        hold(&mut server, "base:stick");
        let farming = server.content.skills.id("core:farming").unwrap();
        let skills = server.players[&1].skills.clone();
        server.players.get_mut(&1).unwrap().skills = Default::default();
        server.tend_bees(1, HIVE);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:honey_skep")), "too green a keeper takes nothing");
        server.players.get_mut(&1).unwrap().skills = skills;
        let xp = server.players[&1].skills.xp(farming);
        server.tend_bees(1, HIVE);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:bee_skep")));
        assert_eq!(server.apiary.foraged(HIVE), Some(0.0), "it starts filling again");
        assert!(server.players[&1].skills.xp(farming) > xp);
        let honey = block(&server, "base:honey");
        assert!(server.items.saved(|b| server.content.blocks.def(b).name.clone()).iter().any(|i| server.content.blocks.id(&i.item) == Some(honey)), "honey pops out");
        assert!(server.players[&1].timed_effects.iter().any(|(e, _)| *e == hive(&server).stung), "taken without smoke: stung");
    }

    /// Flowers, night and the hive filling: no flowers, no foraging; flowers near, the
    /// skep fills in its minutes of daylight; at night the bees stay in.
    #[test]
    fn bees_fill_a_skep_from_the_flowers_near_by_day_only() {
        let mut server = fixture();
        server.set_block(HIVE, block(&server, "base:bee_skep"));
        server.survey_hives(60.0);
        assert_eq!(server.apiary.foraged(HIVE), Some(0.0), "nothing to forage");
        flowers(&mut server);
        assert!(server.nectar_around(HIVE) >= hive(&server).def.nectar_full);
        server.day = (0..100).map(|i| TICKS_PER_DAY as f64 * (4.0 + i as f64 / 100.0)).find(|&d| voxmar_engine::creature::daylight(d as u64) <= 0.0).expect("a dark hour");
        assert!(server.daylight() < hive(&server).def.dusk);
        server.survey_hives(60.0);
        assert_eq!(server.apiary.foraged(HIVE), Some(0.0), "the bees stay in at night");
        server.day = noon();
        server.survey_hives(60.0);
        assert_eq!(server.apiary.foraged(HIVE), Some(60.0));
        let minutes = hive(&server).def.minutes;
        server.survey_hives(minutes * 60.0);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:honey_skep")), "full");
    }

    /// Fire: one right beside a kept hive drives its bees off, leaving the skep empty;
    /// one a few blocks off only smokes them.
    #[test]
    fn a_fire_beside_a_hive_drives_its_bees_off() {
        let mut server = fixture();
        server.set_block(HIVE, block(&server, "base:bee_skep"));
        server.set_block(HIVE + IVec3::new(3, 0, 0), block(&server, "base:campfire"));
        server.survey_hives(10.0);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:bee_skep")), "smoke near is no harm");
        assert!(server.smoked(HIVE, None));
        server.set_block(HIVE + IVec3::new(1, 0, 0), block(&server, "base:campfire"));
        server.survey_hives(10.0);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:skep")), "the bees fled the fire");
        assert_eq!(server.apiary.foraged(HIVE), None);
    }

    /// Crops: those near a hive with bees grow a quarter faster, others as before.
    #[test]
    fn crops_near_a_hive_with_bees_grow_faster() {
        let mut server = fixture();
        let near = HIVE + IVec3::new(4, 0, 0);
        let far = HIVE + IVec3::new(20, 0, 0);
        assert_eq!(server.pollination(near), 1.0);
        server.set_block(HIVE, block(&server, "base:bee_skep"));
        assert_eq!(server.pollination(near), hive(&server).def.pollinate);
        assert_eq!(server.pollination(far), 1.0);
        server.set_block(HIVE, block(&server, "base:skep"));
        assert_eq!(server.pollination(near), 1.0, "an empty skep pollinates nothing");
    }

    /// Pets and other creatures: those that come close are stung, never to death; people
    /// are not; a bear eats a full hive's honey and is not stung.
    #[test]
    fn creatures_by_a_hive_are_stung_and_a_bear_eats_its_honey() {
        let mut server = fixture();
        server.set_block(HIVE, block(&server, "base:honey_skep"));
        let at = HIVE.as_vec3() + Vec3::new(1.5, 0.0, 0.5);
        let species = |name: &str| server.content.creatures.id(name).unwrap();
        let (rabbit, bear) = (species("base:rabbit"), species("base:brown_bear"));
        let rabbit = server.spawn_creature(rabbit, at, 1).expect("room");
        let health = server.creatures.live[&rabbit].health;
        server.survey_hives(10.0);
        assert_eq!(server.creatures.live[&rabbit].health, health - 1, "stung");
        for _ in 0..20 {
            server.survey_hives(10.0);
        }
        assert_eq!(server.creatures.live[&rabbit].health, 1, "never to death");
        assert_eq!(server.block(HIVE), Some(block(&server, "base:honey_skep")));
        let bear = server.spawn_creature(bear, at, 2).expect("room");
        let health = server.creatures.live[&bear].health;
        server.survey_hives(10.0);
        assert_eq!(server.block(HIVE), Some(block(&server, "base:bee_skep")), "the bear ate the honey");
        assert_eq!(server.creatures.live[&bear].health, health);
    }

    /// The home: a skep of bees with flowers near makes a bee garden.
    #[test]
    fn a_skep_of_bees_among_flowers_makes_a_bee_garden_at_home() {
        let mut server = fixture();
        let home = HIVE + IVec3::new(2, 0, 2);
        let garden = server.content.effects.id("base:bee_garden").unwrap();
        let counts = |server: &Server| server.survey_around(home, Some(1));
        let has = |server: &Server, tag: &str| counts(server).counts.iter().find(|(t, _)| t == tag).map_or(0, |(_, n)| *n);
        let dandelion = block(&server, "base:dandelion");
        for i in 0..3 {
            server.set_block(HIVE + IVec3::new(-2, 0, i), dandelion);
        }
        assert_eq!(has(&server, "hive"), 0);
        server.set_block(HIVE, block(&server, "base:bee_skep"));
        assert_eq!((has(&server, "hive"), has(&server, "flower") >= 3), (1, true));
        let when = server.content.effects.get(garden).when.clone().expect("a home step");
        assert!(format!("{when:?}").contains("hive"));
    }

    /// Hives and how far each has foraged survive a save.
    #[test]
    fn hives_are_saved_with_what_they_foraged() {
        let mut server = fixture();
        server.set_block(HIVE, block(&server, "base:bee_skep"));
        flowers(&mut server);
        server.survey_hives(30.0);
        let saved = server.apiary.saved();
        assert_eq!(saved, vec![SavedHive { position: HIVE.to_array(), foraged: 30.0 }]);
        assert_eq!(Apiary::restore(&saved).foraged(HIVE), Some(30.0));
    }

    /// What a skep gives is drawn by the keeper's count of harvest draws, and pops by the
    /// world's count of drops, not the tick it is taken on.
    #[test]
    fn a_skep_gives_by_the_keepers_count_not_the_tick() {
        let taken = |tick: u64| {
            let mut server = fixture();
            server.meta.draw_key = 0x0d1a;
            server.players.get_mut(&1).unwrap().draw_key = crate::player::player_draw_key(0x0d1a, "ada");
            server.tick = tick;
            let mut found = Vec::new();
            for _ in 0..12 {
                server.set_block(HIVE, block(&server, "base:honey_skep"));
                hold(&mut server, "base:stick");
                server.tend_bees(1, HIVE);
                let mut lying: Vec<_> = server.items.take_all().map(|d| (d.stack.item, d.stack.count, d.body.position.to_array(), d.body.velocity.to_array())).collect();
                lying.sort_by(|a, b| a.partial_cmp(b).unwrap());
                found.push(lying);
            }
            (found, server.players[&1].draws.harvests, server.meta.drops)
        };
        let first = taken(1_000);
        assert!(first.1 > 0 && first.0.iter().all(|f| !f.is_empty()), "twelve skeps drew {} times for {:?}", first.1, first.0);
        let gave: Vec<Vec<_>> = first.0.iter().map(|f| f.iter().map(|d| (d.0, d.1)).collect()).collect();
        assert!(gave.windows(2).any(|w| w[0] != w[1]), "twelve skeps all gave alike: {gave:?}");
        assert_eq!(taken(1_377), first, "the same skeps taken later gave otherwise");
    }
}
