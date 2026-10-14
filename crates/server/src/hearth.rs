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
                self.structure_reply(agent, fuel_words(structure, &self.content.blocks));
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
            // A fire that needs no fuel burns until the structure breaks.
            self.lit.insert(core, Lit { position: core.to_array(), structure: structure.name.clone(), until: u64::MAX });
            self.tally(agent, "lit", &structure.name, 1);
        }
        self.send_fires();
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
            Some(standing) => Words::key("multiblock.cold").with("structure", structure_words(standing)).with("fuel", fuel_words(standing, &self.content.blocks)),
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
        let before = self.lit.len();
        self.lit.retain(|&core, fire| fire.until > tick && structures.get(&fire.structure)
            .is_some_and(|m| !m.includes(core, changed) || m.stands(core, |p| world.block(p))));
        if self.lit.len() != before {
            self.send_fires();
        }
    }

    /// The cores of every fire burning, in order.
    pub(crate) fn fires(&self) -> Vec<IVec3> {
        let mut fires: Vec<IVec3> = self.lit.iter().filter(|(_, fire)| fire.until > self.tick).map(|(&core, _)| core).collect();
        fires.sort_by_key(|core| core.to_array());
        fires
    }

    /// Tells every player which fires burn, when that changed since they were last told.
    pub(crate) fn send_fires(&mut self) {
        let fires = self.fires();
        if fires != self.fires_sent {
            self.broadcast(ServerMessage::Fires { lit: fires.clone() });
            self.fires_sent = fires;
        }
    }

    /// A home hearth lit before hearths kept their fire (a world saved earlier) is lit again
    /// when its player joins, if it still stands.
    pub(crate) fn kindle_home(&mut self, id: PlayerId) {
        let Some(home) = self.players.get(&id).and_then(|p| p.home) else { return };
        let core = (home - Vec3::new(0.5, 1.0, 0.5)).round().as_ivec3();
        if self.lit.contains_key(&core) {
            return;
        }
        let Some(block) = self.block(core) else { return };
        let multiblocks = self.content.multiblocks.clone();
        if let Some(structure) = multiblocks.with_core(block).find(|m| m.home && m.fuels.is_empty() && m.stands(core, |p| self.block(p))) {
            self.lit.insert(core, Lit { position: core.to_array(), structure: structure.name.clone(), until: u64::MAX });
        }
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
        server.players.get_mut(&1).unwrap().position = core.as_vec3() + Vec3::new(0.5, 0.0, 3.5);
        let burns = (until - server.tick).div_ceil(TICKS_PER_SECOND as u64) as u32;
        assert_eq!(server.station_seen(1, core), Some((None, Some(burns))), "a look says how long the fire burns");
        server.tick = until - 1;
        assert!(server.station_lit(core));
        server.tick = until;
        assert!(!server.station_lit(core), "the fire outlived its charge");
        assert_eq!(server.station_seen(1, core), Some((None, Some(0))), "a look says the kiln is cold");
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

    /// A hearth is cold until lit: then it burns, and players are told, until a piece of it
    /// is taken away, and players are told it went out. A home hearth from a world saved
    /// before hearths kept their fire is lit again when its player joins.
    #[test]
    fn a_hearth_burns_from_lighting_until_it_breaks_and_players_are_told() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
        server.players.get_mut(&1).unwrap().joined = true;
        let told = |client: &voxmar_protocol::ClientConnection| -> Vec<Vec<IVec3>> {
            std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Fires { lit } => Some(lit), _ => None }).collect()
        };
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let (stone, chimney) = (server.content.blocks.expect("base:cobblestone"), server.content.blocks.expect("base:chimney"));
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), stone);
        }}
        server.set_block(core, server.content.blocks.expect("base:hearthstone"));
        for y in 1..=2 { server.set_block(core + IVec3::new(-1, y, -1), chimney); }
        server.send_fires();
        assert!(server.fires().is_empty() && told(&client).is_empty(), "a hearth burned before it was lit");
        server.light(1, core);
        assert_eq!(server.fires(), [core], "the hearth was not lit");
        assert_eq!(told(&client), [vec![core]]);
        server.tick += 1_000_000;
        server.send_fires();
        assert_eq!(server.fires(), [core], "a hearth burned out");
        assert!(told(&client).is_empty(), "players were told again of what had not changed");
        server.set_block(core + IVec3::new(1, 0, 1), BlockId::AIR);
        assert!(server.fires().is_empty(), "a broken hearth still burns");
        assert_eq!(told(&client), [Vec::<IVec3>::new()]);
        // An older world's home hearth: standing, the player's home, but never kept lit.
        server.set_block(core + IVec3::new(1, 0, 1), stone);
        assert!(server.fires().is_empty(), "mending a hearth lit it");
        server.players.get_mut(&1).unwrap().home = Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        server.kindle_home(1);
        assert_eq!(server.fires(), [core], "the old home hearth was not lit again");
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
