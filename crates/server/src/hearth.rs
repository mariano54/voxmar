//! Lighting the early age gates. A hearth becomes a respawn home; a kiln consumes a
//! content-defined fuel charge and smelts only while that fire and its structure hold.
//! The fire uses server ticks and is saved with the world, so reconnecting cannot refuel it.

use serde::{Deserialize, Serialize};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::multiblock::Multiblock;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// A paid fuel charge, by structure name rather than transient block ids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lit {
    pub position: [i32; 3],
    pub structure: String,
    pub until: u64,
}

fn structure_words(structure: &Multiblock) -> Words {
    Words::Own(def_key("multiblock", &structure.name), structure.title.clone())
}

fn fuel_words(structure: &Multiblock, blocks: &voxmar_engine::block::BlockRegistry) -> Words {
    let fuels = structure.fuels.iter().map(|&(item, count)| {
        Words::key("multiblock.fuel_stack").with("count", count.to_string()).with("item", Words::Block(blocks.def(item).name.clone()))
    }).collect();
    Words::key("multiblock.fuel").with("structure", structure_words(structure)).with("fuels", Words::List(fuels))
}

impl Server {
    /// Light a standing core. A cold kiln takes a complete fuel charge from the hand;
    /// using an already burning kiln consumes nothing and earns no further lighting tally.
    pub(crate) fn light(&mut self, agent: PlayerId, core: IVec3) {
        let Some(block) = self.block(core) else { return };
        let multiblocks = self.content.multiblocks.clone();
        let Some(structure) = multiblocks.with_core(block).find(|m| m.stands(core, |p| self.block(p))) else {
            self.station_hint(agent, core);
            return;
        };
        self.tally(agent, "built", &structure.name, 1);
        if !structure.fuels.is_empty() && !self.station_lit(core) {
            let fuel = self.players.get(&agent).and_then(|p| p.inventory.get(p.selected))
                .and_then(|stack| structure.fuels.iter().find(|&&(item, count)| item == stack.item && stack.count >= count).copied());
            let Some((item, count)) = fuel else {
                self.refuse(agent, fuel_words(structure, &self.content.blocks));
                return;
            };
            let seconds = self.content.machines.burn_seconds(item).expect("checked fuel") * count as f32;
            let Some(player) = self.players.get_mut(&agent) else { return };
            player.inventory.take(player.selected, count);
            player.changed(traits::INVENTORY);
            let until = self.tick.saturating_add((seconds * TICKS_PER_SECOND as f32).ceil() as u64);
            self.lit.insert(core, Lit { position: core.to_array(), structure: structure.name.clone(), until });
            self.tally(agent, "lit", &structure.name, 1);
        } else if structure.fuels.is_empty() {
            self.tally(agent, "lit", &structure.name, 1);
        }
        if structure.home && let Some(player) = self.players.get_mut(&agent) {
            player.home = Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        }
        let key = if structure.home { "multiblock.home" } else { "multiblock.lit" };
        self.structure_reply(agent, Words::key(key).with("structure", structure_words(structure)));
    }

    fn structure_reply(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
    }

    /// Explain a broken or cold station without activating it as a side effect of cooking.
    pub(crate) fn station_hint(&self, agent: PlayerId, core: IVec3) {
        let Some(block) = self.block(core) else { return };
        let structures = &self.content.multiblocks;
        let words = match structures.with_core(block).find(|m| m.stands(core, |p| self.block(p))) {
            // Cold: said where refusals are, with the soft no.
            Some(standing) => {
                let words = Words::key("multiblock.cold").with("structure", structure_words(standing)).with("fuel", fuel_words(standing, &self.content.blocks));
                self.refuse(agent, words);
                return;
            }
            // Not built yet: how to build it is a guide, for the console.
            None => {
                let Some(first) = structures.with_core(block).next() else { return };
                Words::Own(format!("{}.hint", def_key("multiblock", &first.name)), first.hint.clone())
            }
        };
        self.structure_reply(agent, words);
    }

    /// A station core must stand in its structure; ordinary stations work on their own.
    pub(crate) fn station_stands(&self, at: IVec3) -> bool {
        let Some(block) = self.block(at) else { return false };
        if !self.content.blocks.def(block).has_tag("multiblock_core") {
            return true;
        }
        self.content.multiblocks.with_core(block).any(|m| m.stands(at, |p| self.block(p)))
    }

    pub(crate) fn station_lit(&self, at: IVec3) -> bool {
        let Some(block) = self.block(at) else { return false };
        let structures = &self.content.multiblocks;
        let mut candidates = structures.with_core(block).peekable();
        if candidates.peek().is_none() {
            return true;
        }
        candidates.any(|m| m.stands(at, |p| self.block(p)) && (m.fuels.is_empty()
            || self.lit.get(&at).is_some_and(|fire| fire.structure == m.name && fire.until > self.tick)))
    }

    /// Dismantling the core or a required piece quenches its fire immediately. Putting
    /// the piece back cannot resurrect a paid charge, even between simulation ticks.
    pub(crate) fn fire_changed(&mut self, changed: IVec3) {
        let (structures, world, tick) = (&self.content.multiblocks, &self.world, self.tick);
        self.lit.retain(|&core, fire| fire.until > tick && structures.get(&fire.structure)
            .is_some_and(|m| !m.includes(core, changed) || m.stands(core, |p| world.block(p))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::block::BlockId;
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::inventory::ItemStack;
    use crate::player::Player;

    fn player(server: &mut Server) {
        let (_, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
    }

    #[test]
    fn kiln_fire_expires_at_its_paid_tick_and_relighting_takes_another_charge() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let coal = server.content.blocks.expect("base:coal_ore");
        server.set_block(core, server.content.blocks.expect("base:crucible"));
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: coal, count: 24 });
        server.light(1, core);
        assert!(!server.lit.contains_key(&core), "a loose core was lit");
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 24, "a loose core consumed fuel");
        let brick = server.content.blocks.expect("base:kiln_bricks");
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), brick);
        }}
        for x in [-1, 1] { for z in [-1, 1] { server.set_block(core + IVec3::new(x, 1, z), brick); }}
        server.set_block(core, server.content.blocks.expect("base:crucible"));
        server.light(1, core);
        let until = server.lit[&core].until;
        assert_eq!(until - server.tick, (24.0 * server.content.machines.burn_seconds(coal).unwrap() * TICKS_PER_SECOND as f32) as u64);
        server.tick = until - 1;
        assert!(server.station_lit(core));
        server.tick = until;
        assert!(!server.station_lit(core), "the fire outlived its charge");
        server.light(1, core);
        assert!(!server.station_lit(core), "an empty hand relit an expired fire");
        assert_eq!(server.players[&1].tallies["lit:base:crucible_kiln"], 1);
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: coal, count: 24 });
        server.light(1, core);
        assert!(server.station_lit(core));
        assert_eq!(server.players[&1].tallies["lit:base:crucible_kiln"], 2);
        server.set_block(core, BlockId::AIR);
        assert!(!server.lit.contains_key(&core));
    }

    /// A standing Homestead Hearth and a Cottage Stove cook what a campfire cooks; a
    /// hearthstone with no ring round it does not, and says what it lacks.
    #[test]
    fn the_hearth_and_the_stove_cook_as_a_campfire_does() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let stove = core + IVec3::new(4, 0, 0);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (raw, cooked) = (block(&server, "base:raw_small_game"), block(&server, "base:cooked_small_game"));
        server.set_block(core, block(&server, "base:hearthstone"));
        server.set_block(stove, block(&server, "base:cottage_stove"));
        // A tick whose roll never burns at level 1, at either fire.
        let burn = server.content.cooking.for_item(raw, &["campfire".into()]).unwrap().burn_chance(1);
        let tick = (server.tick..server.tick + 1000).find(|&tick| [core, stove].iter().all(|&at| crate::items::random3(tick, at, 1 ^ 0xc00c)[0] >= burn)).unwrap();
        server.tick = tick;
        let cook = |server: &mut Server, at: IVec3| {
            server.players.get_mut(&1).unwrap().inventory.slots.iter_mut().for_each(|slot| *slot = None);
            server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: raw, count: 1 });
            server.cook(1, at);
            server.players[&1].inventory.get(0).map(|s| s.item)
        };
        assert_eq!(cook(&mut server, core), Some(raw), "a loose hearthstone cooked");
        let stone = block(&server, "base:cobblestone");
        for dz in -1..=1 {
            for dx in -1..=1 {
                if (dx, dz) != (0, 0) {
                    server.set_block(core + IVec3::new(dx, 0, dz), stone);
                }
            }
        }
        for dy in 1..=2 {
            server.set_block(core + IVec3::new(-1, dy, -1), block(&server, "base:chimney"));
        }
        assert!(server.station_stands(core));
        assert_eq!(cook(&mut server, core), Some(cooked), "the standing hearth cooked nothing");
        assert_eq!(cook(&mut server, stove), Some(cooked), "the stove cooked nothing");
        assert_eq!(server.players[&1].tallies["cooked_at:campfire:base:cooked_small_game"], 2, "counted as campfire cooking, for First Night");
        // The stove is a fire: it warms, and holds a pot.
        let def = server.content.blocks.def(block(&server, "base:cottage_stove"));
        assert!(def.has_tag("hob") && def.has_tag("heat"));
        assert!(server.content.blocks.info(block(&server, "base:cottage_stove")).emission >= server.survival.heat_emission);
    }

    /// Bronze is three copper ingots held and a tin ore carried, four at a time, in one
    /// step: short of the tin, nothing is taken and the player is told what it takes.
    #[test]
    fn the_crucible_makes_four_bronze_from_three_copper_ingots_and_a_tin_ore() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let blocks = server.content.blocks.clone();
        let brick = blocks.expect("base:kiln_bricks");
        for x in -1..=1 { for z in -1..=1 { server.set_block(core + IVec3::new(x, 0, z), brick); }}
        for x in [-1, 1] { for z in [-1, 1] { server.set_block(core + IVec3::new(x, 1, z), brick); }}
        server.set_block(core, blocks.expect("base:crucible"));
        let (coal, copper, tin, bronze) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ingot"), blocks.expect("base:tin_ore"), blocks.expect("base:bronze_ingot"));
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: coal, count: 24 });
        server.light(1, core);
        assert!(server.station_lit(core));
        let engineering = server.content.skills.id("core:engineering").unwrap();
        let player = server.players.get_mut(&1).unwrap();
        player.skills.grant(engineering, server.content.skills.xp_for(12));
        player.inventory.slots[0] = Some(ItemStack { item: copper, count: 4 });
        player.selected = 0;
        server.cook(1, core);
        assert_eq!(server.players[&1].inventory.count(copper), 4, "no tin, nothing taken");
        server.players.get_mut(&1).unwrap().inventory.slots[5] = Some(ItemStack { item: tin, count: 1 });
        let before = server.players[&1].skills.xp(engineering);
        server.cook(1, core);
        let player = &server.players[&1];
        assert_eq!((player.inventory.count(copper), player.inventory.count(tin), player.inventory.count(bronze)), (1, 0, 4));
        assert_eq!(player.tallies["cooked:base:bronze_ingot"], 4, "First Bronze counts all four");
        assert!(player.skills.xp(engineering) > before);
    }

    #[test]
    fn a_burnt_meal_does_not_count_as_successful_campfire_food() {
        let mut server = crate::test_server();
        player(&mut server);
        let fire = server.spawn.floor().as_ivec3() + IVec3::Y * 4;
        server.set_block(fire, server.content.blocks.expect("base:campfire"));
        let raw = server.content.blocks.expect("base:raw_minnow");
        let recipe = server.content.cooking.for_item(raw, &["campfire".into()]).unwrap();
        let burn = recipe.burn_chance(1);
        let burnt = recipe.burnt.unwrap();
        let tick = (server.tick..server.tick + 1000).find(|&tick| crate::items::random3(tick, fire, 1 ^ 0xc00c)[0] < burn).expect("a real burnt roll");
        server.tick = tick;
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: raw, count: 1 });
        server.cook(1, fire);
        assert_eq!(server.players[&1].inventory.get(0).unwrap().item, burnt);
        assert!(!server.players[&1].tallies.keys().any(|key| key.starts_with("cooked")), "a burnt output counted");
    }

    /// A full pack fires its clay as long as the fired clay has room: a crack gives back
    /// the clay into the stack it came from.
    #[test]
    fn a_full_pack_fires_clay_whether_it_cracks_or_not() {
        let mut server = crate::test_server();
        player(&mut server);
        let kiln = server.spawn.floor().as_ivec3() + IVec3::Y * 4;
        server.set_block(kiln, server.content.blocks.expect("base:pit_kiln"));
        let (clay, fired, stone) = (server.content.blocks.expect("base:clay"), server.content.blocks.expect("base:fired_clay"), server.content.blocks.expect("base:cobblestone"));
        let recipe = server.content.cooking.for_item(clay, &server.content.blocks.def(server.content.blocks.expect("base:pit_kiln")).tags).unwrap();
        let burn = recipe.burn_chance(1);
        assert!(burn > 0.0 && recipe.burnt == Some(clay), "clay cracks back to clay at level 1");
        for cracks in [true, false] {
            let tick = (server.tick..server.tick + 1000).find(|&tick| (crate::items::random3(tick, kiln, 1 ^ 0xc00c)[0] < burn) == cracks).unwrap();
            server.tick = tick;
            let inventory = &mut server.players.get_mut(&1).unwrap().inventory;
            for slot in inventory.slots.iter_mut() {
                *slot = Some(ItemStack { item: stone, count: 64 });
            }
            inventory.slots[0] = Some(ItemStack { item: clay, count: 64 });
            inventory.slots[1] = Some(ItemStack { item: fired, count: 10 });
            server.players.get_mut(&1).unwrap().selected = 0;
            server.cook(1, kiln);
            let inventory = &server.players[&1].inventory;
            let (clay_left, fired_now) = (inventory.get(0).unwrap().count, inventory.get(1).unwrap().count);
            assert_eq!((clay_left, fired_now), if cracks { (64, 10) } else { (63, 11) }, "cracks: {cracks}");
        }
    }
}
