//! Stores (`machine::Role::Bin` that does not feed: a chest, a cupboard, a jar): opened,
//! one shows beside the player's inventory as slots of theirs (`inventory::STORE`). The bin
//! in the factory is what the store holds; the player's slots are a copy, brought up to
//! date before each move they make and written back after, and kept in step every tick
//! with what chutes bring and other players take. It closes with the screen, when the
//! player walks out of reach or dies, or when the store is broken.

use voxmar_engine::block::BlockId;
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::{ItemStack, Store};
use voxmar_engine::traits;
use voxmar_protocol::PlayerId;

use crate::Server;

/// How far past a player's reach a store stays open, as a crafting table does.
const SLACK: f32 = 1.5;

impl Server {
    /// `agent` opens the store at `pos`. One the world placed (a village's barrel) is made
    /// a store the first time it is opened.
    pub(crate) fn open_store(&mut self, agent: PlayerId, pos: IVec3) {
        let Some(block) = self.block(pos) else { return };
        let machines = self.content.machines.clone();
        if !machines.get(block).is_some_and(|m| m.is_store()) {
            return;
        }
        if self.factory.get(pos).is_none() {
            self.factory.insert(pos, block, &machines);
        }
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.store = Some(pos);
        player.inventory.store = Some(Store { block, slots: Vec::new() });
        self.pull_store(agent);
    }

    /// Brings `agent`'s copy of their open store up to date with what it holds, or closes
    /// it if the store is gone (broken this tick, before `watch_stores` saw it), so no move
    /// goes into a copy nothing would keep.
    pub(crate) fn pull_store(&mut self, agent: PlayerId) {
        let Some(player) = self.players.get_mut(&agent) else { return };
        let Some(pos) = player.store else { return };
        let Some((block, state)) = self.factory.get(pos) else {
            self.close_store(agent);
            return;
        };
        let fresh = Store { block, slots: state.bin.clone() };
        if player.inventory.store.as_ref() != Some(&fresh) {
            player.inventory.store = Some(fresh);
            player.changed(traits::INVENTORY);
        }
    }

    /// Writes `agent`'s copy of their open store back into it, after a move. What it gained
    /// and lost of each item is tallied by the store's block, for a quest's `Stocked`: an
    /// item put in and taken out again counts nothing.
    pub(crate) fn push_store(&mut self, agent: PlayerId) {
        let Some(player) = self.players.get(&agent) else { return };
        let (Some(pos), Some(store)) = (player.store, player.inventory.store.as_ref()) else { return };
        let mut moved: Vec<(BlockId, i64)> = Vec::new();
        let mut count = |slots: &[Option<ItemStack>], sign: i64| {
            for stack in slots.iter().flatten() {
                match moved.iter_mut().find(|(item, _)| *item == stack.item) {
                    Some((_, n)) => *n += sign * i64::from(stack.count),
                    None => moved.push((stack.item, sign * i64::from(stack.count))),
                }
            }
        };
        let block = store.block;
        let Some(state) = self.factory.state_mut(pos) else { return };
        if state.bin.len() != store.slots.len() {
            return;
        }
        count(&store.slots, 1);
        count(&state.bin, -1);
        state.bin.clone_from(&store.slots);
        let blocks = &self.content.blocks;
        let kind = format!("stocked:{}", blocks.def(block).name);
        let moved: Vec<(String, i64)> = moved.into_iter().filter(|&(_, n)| n != 0).map(|(item, n)| (blocks.def(item).name.clone(), n)).collect();
        for (item, n) in moved {
            if n > 0 {
                self.tally(agent, &kind, &item, n as u32);
            } else {
                self.untally(agent, &kind, &item, n.unsigned_abs() as u32);
            }
        }
    }

    /// Closes `agent`'s open store, if one is.
    pub(crate) fn close_store(&mut self, agent: PlayerId) {
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.store = None;
        if player.inventory.close_store() {
            player.changed(traits::INVENTORY);
        }
    }

    /// Every tick: closes the stores of players who walked away from them, died, or saw
    /// them broken, and brings every open one up to date.
    pub(crate) fn watch_stores(&mut self) {
        let open: Vec<(PlayerId, IVec3)> = self.players.iter().filter_map(|(&id, p)| p.store.map(|pos| (id, pos))).collect();
        for (agent, pos) in open {
            let player = &self.players[&agent];
            let near = player.eye().distance(pos.as_vec3() + 0.5) <= voxmar_engine::physics::player::REACH + SLACK;
            let standing = self.block(pos).is_some_and(|b| self.content.machines.get(b).is_some_and(|m| m.is_store())) && self.factory.get(pos).is_some();
            if near && standing && player.health.alive() {
                self.pull_store(agent);
            } else {
                self.close_store(agent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use voxmar_engine::action::ActionTarget;
    use voxmar_engine::block::BlockId;
    use voxmar_engine::coords::Face;
    use voxmar_engine::glam::Vec3;
    use voxmar_engine::inventory::{ItemStack, STORE};
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::world::Column;

    use super::*;

    const FLOOR: i32 = 199;
    const AT: IVec3 = IVec3::new(10, FLOOR + 1, 10);

    /// A stone floor, and player 1 standing on it beside `AT`.
    fn fixture() -> Server {
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
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = Vec3::new(8.5, (FLOOR + 1) as f32, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        server
    }

    fn id(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    fn act(server: &mut Server, action: &str, target: ActionTarget) {
        let action = server.content.actions.expect(action);
        server.propose(1, action, target, Instant::now());
    }

    fn open(server: &mut Server) {
        act(server, "base:open_store", ActionTarget::Block { block: AT, face: Face::NegX });
    }

    fn give(server: &mut Server, slot: usize, name: &str, count: u8) {
        let item = id(server, name);
        server.players.get_mut(&1).unwrap().inventory.slots[slot] = Some(ItemStack { item, count });
    }

    fn held(server: &Server) -> Vec<(BlockId, u8)> {
        server.factory.get(AT).map(|(_, state)| state.bin.iter().flatten().map(|s| (s.item, s.count)).collect()).unwrap_or_default()
    }

    /// A chest opens into its 27 slots beside the inventory; what moves into them is in the
    /// chest, what a chute brings shows at once, and it closes when the player walks off.
    #[test]
    fn a_chest_opens_into_slots_kept_in_step_with_it() {
        let mut server = fixture();
        let chest = id(&server, "base:chest");
        server.set_block(AT, chest);
        give(&mut server, 0, "base:dirt", 10);
        open(&mut server);
        let store = server.players[&1].inventory.store.clone().expect("open");
        assert_eq!((store.block, store.slots.len(), server.players[&1].store), (chest, 27, Some(AT)));
        act(&mut server, "core:transfer", ActionTarget::Slots { from: 0, to: STORE as u8 + 4, count: 10 });
        let dirt = id(&server, "base:dirt");
        assert_eq!(held(&server), vec![(dirt, 10)]);
        assert_eq!(server.players[&1].inventory.slots[0], None);
        let stocked = |server: &Server| server.players[&1].tallies.get("stocked:base:chest:base:dirt").copied();
        assert_eq!(stocked(&server), Some(10), "what went in is tallied for quests, by the store and the item");
        // Taken out and put back, it is still ten in the chest, not twenty.
        act(&mut server, "core:transfer", ActionTarget::Slots { from: STORE as u8 + 4, to: 0, count: 10 });
        assert_eq!(stocked(&server), None, "taken out, it counts nothing");
        act(&mut server, "core:transfer", ActionTarget::Slots { from: 0, to: STORE as u8 + 4, count: 10 });
        assert_eq!(stocked(&server), Some(10));
        // A chute brings sand.
        let sand = id(&server, "base:sand");
        let machines = server.content.machines.clone();
        server.factory.put(AT, ItemStack { item: sand, count: 5 }, &machines);
        server.watch_stores();
        assert_eq!(server.players[&1].inventory.get(STORE), Some(ItemStack { item: sand, count: 5 }));
        // Shift click takes it into the pack.
        act(&mut server, "core:quick_move", ActionTarget::Slot(STORE as u8));
        assert_eq!(held(&server), vec![(dirt, 10)]);
        assert_eq!(stocked(&server), Some(10), "what a chute brought, taken out, takes nothing off what was put in");
        assert!(server.players[&1].inventory.slots.iter().flatten().any(|s| s.item == sand && s.count == 5));
        server.players.get_mut(&1).unwrap().position = Vec3::new(28.5, (FLOOR + 1) as f32, 28.5);
        server.watch_stores();
        assert_eq!((server.players[&1].store, server.players[&1].inventory.store.is_some()), (None, false), "closed out of reach");
        assert_eq!(held(&server), vec![(dirt, 10)], "and what it held stays in it");
    }

    /// A jar keeps only what is edible (`holds`), from a player's hand or a chute; a
    /// basket only what is foraged.
    #[test]
    fn a_jar_keeps_only_what_is_edible() {
        let mut server = fixture();
        server.set_block(AT, id(&server, "base:earthen_jar"));
        give(&mut server, 0, "base:cobblestone", 3);
        give(&mut server, 1, "base:honey", 2);
        open(&mut server);
        assert_eq!(server.players[&1].inventory.store.as_ref().map(|s| s.slots.len()), Some(4));
        act(&mut server, "core:transfer", ActionTarget::Slots { from: 0, to: STORE as u8, count: 3 });
        act(&mut server, "core:quick_move", ActionTarget::Slot(0));
        assert!(held(&server).is_empty(), "no stone in a jar");
        act(&mut server, "core:quick_move", ActionTarget::Slot(1));
        assert_eq!(held(&server), vec![(id(&server, "base:honey"), 2)]);
        let machines = server.content.machines.clone();
        let stone = ItemStack { item: id(&server, "base:cobblestone"), count: 1 };
        assert_eq!(server.factory.put(AT, stone, &machines), Some(stone), "nor from a chute");
        let basket = server.content.machines.get(id(&server, "base:earthen_basket")).expect("a store");
        assert!(basket.bin_takes(id(&server, "base:windroot")) && !basket.bin_takes(id(&server, "base:honey")));
    }

    /// Furniture holds what it is given until it is broken, when it spills it, and the
    /// screen on it closes.
    #[test]
    fn a_broken_cupboard_spills_what_it_held() {
        let mut server = fixture();
        server.set_block(AT, id(&server, "base:oak_cupboard"));
        give(&mut server, 0, "base:dirt", 7);
        open(&mut server);
        act(&mut server, "core:quick_move", ActionTarget::Slot(0));
        let before = server.items.saved(|_| String::new()).len();
        server.set_block(AT, BlockId::AIR);
        assert_eq!(server.items.saved(|_| String::new()).len(), before + 1, "the dirt lies where it stood");
        server.watch_stores();
        assert!(server.players[&1].inventory.store.is_none());
    }

    /// A move into a store broken earlier the same tick finds it closed: what was moved
    /// stays in the pack rather than going into a copy nothing keeps.
    #[test]
    fn a_move_into_a_store_just_broken_stays_in_the_pack() {
        let mut server = fixture();
        server.set_block(AT, id(&server, "base:oak_cupboard"));
        open(&mut server);
        give(&mut server, 0, "base:dirt", 5);
        server.set_block(AT, BlockId::AIR);
        act(&mut server, "core:transfer", ActionTarget::Slots { from: 0, to: STORE as u8, count: 5 });
        let player = &server.players[&1];
        assert_eq!((player.store, player.inventory.slots[0].map(|s| s.count)), (None, Some(5)));
    }

    /// A barrel the world placed (in a village) was never a machine: opened, it becomes a
    /// store.
    #[test]
    fn a_village_barrel_becomes_a_store_when_opened() {
        let mut server = fixture();
        let barrel = id(&server, "base:barrel");
        let mut column = (**server.world.column(ColumnPos::new(0, 0)).unwrap()).clone();
        column.set_block(AT, barrel);
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        assert!(server.factory.get(AT).is_none());
        open(&mut server);
        assert_eq!(server.players[&1].inventory.store.as_ref().map(|s| s.slots.len()), Some(9));
    }

    /// A treasure chest's lock picked, it opens straight into its slots, holding its find.
    #[test]
    fn a_picked_lock_opens_the_chest_into_its_slots() {
        let mut server = fixture();
        let block = id(&server, "base:nomad_treasure_chest");
        let chest = server.content.treasure.chest(block).unwrap().clone();
        server.set_block(AT, block);
        let pick = server.content.treasure.opening_pick(chest.age).expect("a pick opens it");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack { item: pick, count: 1 });
        act(&mut server, "base:pick_lock", ActionTarget::Block { block: AT, face: Face::NegX });
        let player = &server.players[&1];
        assert_eq!(player.store, Some(AT));
        let shown: Vec<(BlockId, u8)> = player.inventory.store.as_ref().unwrap().slots.iter().flatten().map(|s| (s.item, s.count)).collect();
        assert!(!shown.is_empty() && shown == held(&server), "its find, as the chest holds it");
    }
}
