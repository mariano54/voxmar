//! Traps on the server (`traps.ron`, docs/traps.md): a wild creature of a kind a set trap
//! catches that steps into its cell is caught. It is gone from the world, the trap shows
//! sprung, and what it was is kept with the trap until a player takes it: its loot, rolled
//! for them as a kill of theirs, and its kill experience. The trap is set again.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::creature::{CreatureId, mix, unit_from};
use voxmar_engine::crop::level_bonus;
use voxmar_engine::dropped;
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::loot::{self, Moment};
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;
use crate::items::random3;
use crate::process::Process;

/// What a sprung trap holds: the species caught, and the seed its loot is rolled from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Catch {
    pub species: CreatureId,
    pub seed: u64,
}

/// Every sprung trap's catch, by where it stands.
#[derive(Default)]
pub struct Catches {
    caught: FxHashMap<IVec3, Catch>,
}

/// A catch as `world.ron` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCatch {
    pub position: [i32; 3],
    pub species: String,
    pub seed: u64,
}

impl Catches {
    pub fn saved(&self, name: impl Fn(CreatureId) -> String) -> Vec<SavedCatch> {
        let mut saved: Vec<SavedCatch> = self.caught.iter().map(|(pos, c)| SavedCatch { position: pos.to_array(), species: name(c.species), seed: c.seed }).collect();
        saved.sort_by_key(|c| c.position);
        saved
    }

    /// The saved catches whose species is still known.
    pub fn restore(saved: &[SavedCatch], id: impl Fn(&str) -> Option<CreatureId>) -> Self {
        Self { caught: saved.iter().filter_map(|c| Some((IVec3::from_array(c.position), Catch { species: id(&c.species)?, seed: c.seed }))).collect() }
    }

    #[cfg(test)]
    pub fn get(&self, pos: IVec3) -> Option<Catch> {
        self.caught.get(&pos).copied()
    }
}

/// `base:traps`: springs set traps on the creatures that stepped into them, every 4 ticks
/// (`Tier::Medium`), so a rabbit running through a set cell is still in it at a look.
pub(crate) struct Traps;

impl Process for Traps {
    fn tier(&self) -> Tier {
        Tier::Medium
    }

    fn run(&mut self, server: &mut Server) {
        if server.content.traps.traps.is_empty() {
            return;
        }
        server.spring_traps();
    }
}

impl Server {
    /// Each wild creature a trap catches, standing in a set one's cell, is caught there.
    pub(crate) fn spring_traps(&mut self) {
        let traps = self.content.traps.clone();
        let caught: Vec<(u32, IVec3, BlockId)> = self
            .creatures
            .live
            .iter()
            .filter(|(_, c)| c.owner.is_none() && c.health > 0)
            .filter_map(|(&id, c)| {
                let cell = c.body.position.floor().as_ivec3();
                let trap = self.world.block(cell).and_then(|b| traps.set(b)).filter(|t| t.catches(c.species))?;
                Some((id, cell, trap.sprung))
            })
            .collect();
        for (id, cell, sprung) in caught {
            // One creature to a trap: a second in the same cell on the same look waits.
            if self.world.block(cell) == Some(sprung) {
                continue;
            }
            let Some(creature) = self.creatures.live.get(&id) else { continue };
            let catch = Catch { species: creature.species, seed: creature.seed };
            self.creature_felled(id, None);
            self.set_block(cell, sprung);
            self.catches.caught.insert(cell, catch);
        }
    }

    /// `agent` takes what the sprung trap at `pos` caught (`base:take_from_trap`): its loot
    /// as a kill of theirs, and its kill experience; the trap is set again.
    pub(crate) fn take_from_trap(&mut self, agent: PlayerId, pos: IVec3) {
        let traps = self.content.traps.clone();
        let Some(trap) = self.block(pos).and_then(|b| traps.sprung(b)) else { return };
        let Some(player) = self.players.get(&agent) else { return };
        let level = self.content.skills.level(player.skills.xp(trap.skill));
        if level < trap.level {
            let skill = Words::Own(voxmar_engine::lang::def_key("skill", &self.content.skills.get(trap.skill).name), self.content.skills.get(trap.skill).title.clone());
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("trap.needs").with("skill", skill).with("level", Words::text(trap.level)) });
            return;
        }
        let catch = self.catches.caught.remove(&pos);
        self.set_block(pos, trap.set);
        let Some(catch) = catch else { return };
        let species = self.content.creatures.get(catch.species);
        let player = &self.players[&agent];
        let tool: Vec<String> = player.held().map_or_else(Vec::new, |item| self.content.blocks.def(item).tags.clone());
        let bonus = species.xp_skill.map_or(0.0, |skill| level_bonus(self.content.skills.level(player.skills.xp(skill)), 1, self.content.skills.max_level()));
        let moment = Moment { bonus, tool: &tool, night: self.daylight() < 0.3, by_player: true };
        let mut draws = 0u64;
        let tick = self.tick;
        let found = loot::roll(&species.loot, &moment, || {
            draws += 1;
            unit_from(mix(catch.seed, draws ^ tick << 8))
        });
        let name = species.def.name.clone();
        let page = Self::page_of(&species.def);
        let kill = species.xp_skill.map(|skill| (skill, species.def.xp.kill));
        let mut journal: Vec<(voxmar_engine::journal::Page, String, u32)> = page.map(|page| (page, name, 1)).into_iter().collect();
        for (i, (item, count)) in found.into_iter().enumerate() {
            journal.push((voxmar_engine::journal::Page::Collection, self.content.blocks.def(item).name.clone(), count as u32));
            let Some(player) = self.players.get_mut(&agent) else { break };
            let left = player.inventory.add(item, count);
            player.changed(traits::INVENTORY);
            if left > 0 {
                let body = dropped::popped(pos, random3(tick, pos, 0x5a4e + i as u32));
                self.items.spawn(ItemStack { item, count: left }, body, dropped::PICKUP_DELAY);
            }
        }
        if let Some((skill, xp)) = kill {
            self.grant_xp(agent, skill, xp);
        }
        for (page, name, times) in journal {
            self.discover(agent, page, &name, times);
        }
    }

    /// A block changed: a sprung trap broken or replaced loses its catch.
    pub(crate) fn trap_changed(&mut self, pos: IVec3, block: BlockId) {
        if self.content.traps.sprung(block).is_none() {
            self.catches.caught.remove(&pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::glam::Vec3;
    use voxmar_engine::world::Column;
    use voxmar_protocol::TICKS_PER_SECOND;

    use super::*;

    const FLOOR: i32 = 199;

    /// A stone floor far from the world, player 1 on it and a snare set at (10, 200, 10).
    fn field(hunter: u8) -> (Server, IVec3) {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        let mut column = Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, FLOOR, z), stone);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, Vec3::new(4.5, FLOOR as f32 + 1.0, 4.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        let skill = server.content.skills.id("core:hunter").unwrap();
        let xp = server.content.skills.xp_for(hunter);
        player.skills.grant(skill, xp);
        server.players.insert(1, player);
        let snare = IVec3::new(10, FLOOR + 1, 10);
        server.set_block(snare, server.content.blocks.expect("base:snare"));
        (server, snare)
    }

    fn put(server: &mut Server, name: &str, at: Vec3) -> u32 {
        let species = server.content.creatures.id(name).unwrap();
        server.place_creature(species, at, 7, 0.0).expect("placed")
    }

    fn named(server: &Server, pos: IVec3) -> String {
        server.block(pos).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone())
    }

    fn hunter_xp(server: &Server) -> u32 {
        server.players[&1].skills.xp(server.content.skills.id("core:hunter").unwrap())
    }

    /// A rabbit that steps into a set snare is caught: gone from the world, the snare
    /// sprung; taken, its loot comes to hand with its kill's experience, and the snare is
    /// set again.
    #[test]
    fn a_rabbit_steps_into_a_snare_and_is_taken_from_it() {
        let (mut server, snare) = field(1);
        let rabbit = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.spring_traps();
        assert!(!server.creatures.live.contains_key(&rabbit), "caught, gone from the world");
        assert_eq!(named(&server, snare), "base:snare_sprung");
        let before = hunter_xp(&server);
        server.take_from_trap(1, snare);
        assert_eq!(named(&server, snare), "base:snare", "set again");
        assert!(hunter_xp(&server) > before, "the kill's experience");
        let carried: u32 = server.players[&1].inventory.slots.iter().flatten().map(|s| s.count as u32).sum();
        assert!(carried > 0, "its loot came to hand");
        // Taken once: nothing more.
        server.take_from_trap(1, snare);
        assert_eq!(named(&server, snare), "base:snare");
    }

    /// What the gate's Snare step does: a rabbit summoned (by command, with the player two
    /// blocks off) into a set snare's cell is caught by the server's own ticks.
    #[test]
    fn a_rabbit_summoned_into_a_set_snare_springs_it() {
        let (mut server, snare) = field(1);
        server.players.get_mut(&1).unwrap().position = snare.as_vec3() + Vec3::new(0.5, 0.0, -2.5);
        server.console(&format!("summon rabbit 1 {} {} {}", snare.x, snare.y, snare.z));
        let rabbit = server.creatures.live.iter().find(|(_, c)| c.owner.is_none()).map(|(&id, c)| (id, c.body.position));
        let (_, at) = rabbit.expect("a rabbit was summoned");
        assert_eq!(at.floor().as_ivec3(), snare, "summoned into the snare's cell, not {at}");
        for _ in 0..TICKS_PER_SECOND * 2 {
            server.tick();
            if named(&server, snare) == "base:snare_sprung" {
                return;
            }
        }
        panic!("the snare never sprang: the rabbit is at {:?}", server.creatures.live.values().find(|c| c.owner.is_none()).map(|c| c.body.position));
    }

    /// A snare catches only its kinds, only wild ones, and only in its own cell.
    #[test]
    fn a_snare_holds_only_small_wild_game_in_its_cell() {
        let (mut server, snare) = field(1);
        let wolf = put(&mut server, "base:wolf", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        let beside = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(2.5, 0.0, 0.5));
        let kept = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.creatures.live.get_mut(&kept).unwrap().owner = Some("someone".into());
        server.spring_traps();
        assert!([wolf, beside, kept].iter().all(|id| server.creatures.live.contains_key(id)));
        assert_eq!(named(&server, snare), "base:snare");
    }

    /// Breaking a sprung snare loses its catch; a catch keeps across a save.
    #[test]
    fn a_catch_keeps_across_a_save_and_is_lost_with_the_snare() {
        let (mut server, snare) = field(1);
        put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.spring_traps();
        let saved = server.catches.saved(|id| server.content.creatures.get(id).def.name.clone());
        let restored = Catches::restore(&saved, |name| server.content.creatures.id(name));
        assert_eq!(restored.get(snare), server.catches.get(snare));
        assert!(restored.get(snare).is_some());
        server.set_block(snare, BlockId::AIR);
        assert!(server.catches.get(snare).is_none(), "broken, the catch is lost");
    }
}
