//! Stores (`machine::Role::Bin` that does not feed: a chest, a cupboard, a jar): opened,
//! one shows beside the player's inventory as slots of theirs (`inventory::STORE`). The bin
//! in the factory is what the store holds; the player's slots are a copy, brought up to
//! date before each move they make and written back after, and kept in step every tick
//! with what chutes bring and other players take. It closes with the screen, when the
//! player walks out of reach or dies, or when the store is broken.

use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::{HOTBAR_SLOTS, ItemStack, SLOTS, Store, max_stack};
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;

/// How far past a player's reach a store stays open, as a crafting table does.
const SLACK: f32 = 1.5;

impl Server {
    /// What stands on the block at `pos`, if it shows what it holds (`BlockDef::shows`: a
    /// mantel shelf's things): the item in each of its first slots, a spot apiece. None for
    /// one with nothing on it, or no such block.
    fn shown_at(&self, pos: IVec3) -> Option<Vec<Option<BlockId>>> {
        let (block, state) = self.factory.get(pos)?;
        let spots = self.content.blocks.def(block).shows.len();
        let items: Vec<Option<BlockId>> = state.bin.iter().take(spots).map(|slot| slot.as_ref().map(|stack| stack.item)).collect();
        items.iter().any(Option::is_some).then_some(items)
    }

    /// Tells the players who have a column what stands on its shelves, when that changed
    /// since they were last told: only the bins the factory says changed are looked at. A
    /// player is told of a column's shelves as the column is sent (`stream_columns`).
    pub(crate) fn send_shown(&mut self) {
        let mut columns: Vec<ColumnPos> = Vec::new();
        for pos in self.factory.take_bins_changed() {
            let now = self.shown_at(pos);
            let column = ColumnPos::of_block(pos);
            let told = self.shown.get(&column).and_then(|at| at.iter().find(|(p, _)| *p == pos)).map(|(_, items)| items);
            if told == now.as_ref() {
                continue;
            }
            let at = self.shown.entry(column).or_default();
            at.retain(|(p, _)| *p != pos);
            if let Some(items) = now {
                at.push((pos, items));
                at.sort_by_key(|(p, _)| p.to_array());
            }
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
        for column in columns {
            let at = self.shown.get(&column).cloned().unwrap_or_default();
            if at.is_empty() {
                self.shown.remove(&column);
            }
            for player in self.players.values().filter(|p| p.joined && p.sent.contains(&column)) {
                player.connection.send(ServerMessage::Shown { column, at: at.clone() });
            }
        }
    }

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
        // A store in its keeper's home holds more as the home climbs (effects' store_room).
        if self.in_home_with(agent, pos)
            && let Some(machine) = machines.get(block)
        {
            // Judged afresh, at their own home: not what a visit or the last home left on
            // their modifiers (they refresh only once a second).
            crate::process::Survival::refresh(self, agent);
            let want = machine.grown_bin(self.players[&agent].modifiers.store_room);
            if let Some(state) = self.factory.state_mut(pos)
                && state.bin.len() < want
            {
                state.bin.resize(want, None);
                state.grown = want;
            }
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
        // A store turned another way is tallied as the store itself.
        let kind = format!("stocked:{}", blocks.def(block).form_of().unwrap_or(&blocks.def(block).name));
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

impl Server {
    /// `agent` puts their pack away at home (`base:put_away`): each stack off the hotbar
    /// whose item a store in the home already holds goes into the stores holding it,
    /// nearest first, onto that item's stacks and then into empty slots, as far as it fits.
    /// The hotbar, the pouches, the grid and the pointer stay as they are. What goes in is
    /// tallied by each store's block, for a quest's `Stocked`, as a move into it is.
    pub(crate) fn put_away(&mut self, agent: PlayerId) {
        if self.at_home(agent).is_none() {
            return self.tell_store(agent, Words::key("store.put_away.away"));
        }
        let stores = self.home_stores(agent);
        let machines = self.content.machines.clone();
        let Some(player) = self.players.get(&agent) else { return };
        let mut pack = player.inventory.clone();
        let mut moved = 0u32;
        let mut stocked: Vec<(String, String, u32)> = Vec::new();
        // A stack some store holds and takes, whether or not it had room.
        let mut matched = false;
        for slot in HOTBAR_SLOTS..SLOTS {
            let Some(mut stack) = pack.slots[slot] else { continue };
            let most = max_stack(stack.item);
            for &pos in &stores {
                let Some((block, state)) = self.factory.get(pos) else { continue };
                let holds = state.bin.iter().flatten().any(|s| s.item == stack.item);
                if !holds || !machines.get(block).is_some_and(|m| m.bin_takes(stack.item)) {
                    continue;
                }
                matched = true;
                let blocks = &self.content.blocks;
                // A turned store counts as its base form, as a move into it does.
                let (kind, item) = (format!("stocked:{}", blocks.def(block).form_of().unwrap_or(&blocks.def(block).name)), blocks.def(stack.item).name.clone());
                let before = stack.count;
                let state = self.factory.state_mut(pos).expect("got above");
                // Onto its stacks first, then into empty slots.
                for onto in [true, false] {
                    for cell in state.bin.iter_mut() {
                        let room = match cell {
                            // Worn stacks never merge.
                            Some(s) if onto && s.item == stack.item && s.wear == 0 && stack.wear == 0 => most.saturating_sub(s.count),
                            None if !onto => most,
                            _ => 0,
                        };
                        let put = room.min(stack.count);
                        if put == 0 {
                            continue;
                        }
                        match cell {
                            Some(s) => s.count += put,
                            None => *cell = Some(ItemStack { count: put, ..stack }),
                        }
                        stack.count -= put;
                    }
                }
                if stack.count < before {
                    stocked.push((kind, item, u32::from(before - stack.count)));
                }
                if stack.count == 0 {
                    break;
                }
            }
            if Some(stack) != pack.slots[slot] {
                moved += 1;
                pack.slots[slot] = (stack.count > 0).then_some(stack);
            }
        }
        if moved == 0 {
            return self.tell_store(agent, Words::key(if matched { "store.put_away.full" } else { "store.put_away.none" }));
        }
        let player = self.players.get_mut(&agent).expect("got above");
        player.inventory = pack;
        player.changed(traits::INVENTORY);
        for (kind, item, n) in stocked {
            self.tally(agent, &kind, &item, n);
            // Put away with the key, for a quest's `PutAway` (a move by hand is not).
            self.tally(agent, "put_away", &item, n);
        }
        self.pull_store(agent);
        self.tell_store(agent, Words::key("store.put_away").with("count", Words::text(moved)));
    }

    fn tell_store(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
    }
}

/// Tells players what stands on the blocks that show what they hold, five times a second:
/// a thing put on a shelf by hand, a chute or another player shows soon after. Only the
/// bins that changed are looked at, and only the players who have their columns told.
pub(crate) struct Shelves;

impl crate::process::Process for Shelves {
    fn tier(&self) -> voxmar_engine::schedule::Tier {
        voxmar_engine::schedule::Tier::Medium
    }

    fn run(&mut self, server: &mut Server) {
        server.send_shown();
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
        server.players.get_mut(&1).unwrap().inventory.slots[slot] = Some(ItemStack::new(item, count));
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
        server.factory.put(AT, ItemStack::new(sand, 5), &machines);
        server.watch_stores();
        assert_eq!(server.players[&1].inventory.get(STORE), Some(ItemStack::new(sand, 5)));
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

    /// A cupboard turned east opens and keeps things as one facing north does, and what goes
    /// in is tallied by the cupboard itself, whichever way it faces.
    #[test]
    fn a_turned_cupboard_is_a_store_tallied_as_the_cupboard() {
        let mut server = fixture();
        server.set_block(AT, id(&server, "base:oak_cupboard_east"));
        give(&mut server, 0, "base:dirt", 4);
        open(&mut server);
        assert_eq!(server.players[&1].store, Some(AT), "it opens");
        act(&mut server, "core:transfer", ActionTarget::Slots { from: 0, to: STORE as u8, count: 4 });
        assert_eq!(held(&server), vec![(id(&server, "base:dirt"), 4)]);
        assert_eq!(server.players[&1].tallies.get("stocked:base:oak_cupboard:base:dirt").copied(), Some(4), "tallied as the cupboard");
    }

    /// Put away with H into a turned cupboard, what goes in is tallied as the cupboard too.
    #[test]
    fn putting_away_into_a_turned_cupboard_tallies_the_cupboard() {
        let mut server = fixture();
        let cupboard = id(&server, "base:oak_cupboard_east");
        server.set_block(AT, cupboard);
        let machines = server.content.machines.clone();
        if server.factory.get(AT).is_none() {
            server.factory.insert(AT, cupboard, &machines);
        }
        assert_eq!(server.factory.put(AT, ItemStack::new(id(&server, "base:dirt"), 1), &machines), None);
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(8.5, (FLOOR + 1) as f32, 8.5));
        give(&mut server, 12, "base:dirt", 5);
        act(&mut server, "base:put_away", ActionTarget::None);
        assert_eq!(held(&server), vec![(id(&server, "base:dirt"), 6)]);
        assert_eq!(server.players[&1].tallies.get("stocked:base:oak_cupboard:base:dirt").copied(), Some(5), "tallied as the cupboard");
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
        let stone = ItemStack::new(id(&server, "base:cobblestone"), 1);
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
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(pick, 1));
        act(&mut server, "base:pick_lock", ActionTarget::Block { block: AT, face: Face::NegX });
        let player = &server.players[&1];
        assert_eq!(player.store, Some(AT));
        let shown: Vec<(BlockId, u8)> = player.inventory.store.as_ref().unwrap().slots.iter().flatten().map(|s| (s.item, s.count)).collect();
        assert!(!shown.is_empty() && shown == held(&server), "its find, as the chest holds it");
    }

    fn slots(server: &Server) -> usize {
        server.factory.get(AT).map_or(0, |(_, state)| state.bin.len())
    }

    /// Player 1 at home by `AT` (their bed beside it), and Homely for a while, as the
    /// survival pass judges it.
    fn homely(server: &mut Server) {
        let homely = server.content.effects.id("base:homely").expect("base's homely");
        let player = server.players.get_mut(&1).unwrap();
        player.bed = Some(Vec3::new(9.5, (FLOOR + 1) as f32, 8.5));
        player.give_effects(&[(homely, 60.0)]);
        crate::process::Process::run(&mut crate::process::Survival, server);
    }

    /// A barrel of 9 in a Homely home, opened by its keeper, holds 18; in no home it holds
    /// its 9.
    #[test]
    fn a_homely_home_doubles_the_stores_in_it() {
        let mut server = fixture();
        let barrel = id(&server, "base:barrel");
        server.set_block(AT, barrel);
        open(&mut server);
        assert_eq!(slots(&server), 9, "a barrel away from any home");
        server.close_store(1);
        homely(&mut server);
        assert_eq!(server.players[&1].modifiers.store_room, 2.0);
        open(&mut server);
        assert_eq!(slots(&server), 18, "a barrel in a Homely home");
        assert_eq!(server.players[&1].inventory.store.as_ref().map(|s| s.slots.len()), Some(18), "the open store's slots, as the client is sent them");
    }

    /// What a finer home elsewhere (a friend's, or the one just left) last left on the
    /// player's modifiers grows nothing: at their own plain home the barrel is judged afresh
    /// and holds its 9.
    #[test]
    fn a_store_grows_by_the_home_it_is_in_not_the_last_one_felt() {
        let mut server = fixture();
        let barrel = id(&server, "base:barrel");
        server.set_block(AT, barrel);
        let player = server.players.get_mut(&1).unwrap();
        player.bed = Some(Vec3::new(9.5, (FLOOR + 1) as f32, 8.5));
        player.modifiers.store_room = 3.0;
        open(&mut server);
        assert_eq!(slots(&server), 9, "a barrel at a plain home grew by a finer home's store_room");
    }

    /// A store grown at home keeps its size: the home no longer Homely, opened again, and
    /// taken out of the factory and put back as a save does, it still holds 18.
    #[test]
    fn a_grown_store_never_shrinks() {
        let mut server = fixture();
        let barrel = id(&server, "base:barrel");
        server.set_block(AT, barrel);
        homely(&mut server);
        open(&mut server);
        server.close_store(1);
        server.players.get_mut(&1).unwrap().timed_effects.clear();
        crate::process::Process::run(&mut crate::process::Survival, &mut server);
        assert_eq!(server.players[&1].modifiers.store_room, 1.0);
        open(&mut server);
        assert_eq!(slots(&server), 18, "shrank when the home slipped");
        server.close_store(1);
        // Filled to the brim and saved, the world loads it with every stack in place.
        let machines = server.content.machines.clone();
        let stone = id(&server, "base:cobblestone");
        let full = voxmar_engine::inventory::max_stack(stone);
        for _ in 0..18 {
            assert_eq!(server.factory.put(AT, ItemStack::new(stone, full), &machines), None);
        }
        let saved = server.saved_machines();
        server.factory.remove(AT).unwrap();
        server.restore_machines(saved);
        assert_eq!(slots(&server), 18, "shrank in a save");
        let kept: u32 = server.factory.get(AT).unwrap().1.bin.iter().flatten().map(|s| s.count as u32).sum();
        assert_eq!(kept, 18 * full as u32, "nothing fell out on loading");
        assert!(server.items.saved(|b| server.content.blocks.def(b).name.clone()).is_empty(), "nothing lies loose");
    }

    /// A barrel at `AT` holding `count` of `name`, and player 1's home where they stand.
    fn home_barrel(server: &mut Server, name: &str, count: u8) {
        let barrel = id(server, "base:barrel");
        server.set_block(AT, barrel);
        let machines = server.content.machines.clone();
        if server.factory.get(AT).is_none() {
            server.factory.insert(AT, barrel, &machines);
        }
        let item = id(server, name);
        assert_eq!(server.factory.put(AT, ItemStack::new(item, count), &machines), None);
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(8.5, (FLOOR + 1) as f32, 8.5));
    }

    fn slot(server: &Server, slot: usize) -> Option<(BlockId, u8)> {
        server.players[&1].inventory.slots[slot].map(|s| (s.item, s.count))
    }

    /// At home, H puts each stack off the hotbar whose item the barrel already holds into
    /// it, onto the barrel's own stack first; the hotbar and what no store holds stay.
    #[test]
    fn putting_away_moves_what_the_home_stores_already_hold() {
        let mut server = fixture();
        home_barrel(&mut server, "base:oak_planks", 10);
        give(&mut server, 0, "base:oak_planks", 5);
        give(&mut server, 12, "base:oak_planks", 20);
        give(&mut server, 20, "base:oak_planks", 64);
        give(&mut server, 13, "base:cobblestone", 3);
        act(&mut server, "base:put_away", ActionTarget::None);
        let (planks, stone) = (id(&server, "base:oak_planks"), id(&server, "base:cobblestone"));
        assert_eq!(held(&server), vec![(planks, 64), (planks, 30)], "onto the barrel's stack, then a new one");
        assert_eq!(slot(&server, 0), Some((planks, 5)), "the hotbar stays");
        assert_eq!((slot(&server, 12), slot(&server, 20)), (None, None));
        assert_eq!(slot(&server, 13), Some((stone, 3)), "the barrel holds no cobblestone");
        let stocked = server.players[&1].tallies.get("stocked:base:barrel:base:oak_planks").copied();
        assert_eq!(stocked, Some(84), "what went in counts for a quest's Stocked, as a move in does");
        let put_away = server.players[&1].tallies.get("put_away:base:oak_planks").copied();
        assert_eq!(put_away, Some(84), "and for a quest's PutAway, which only H counts");
    }

    /// Planks put away with H into a home's store count for Put It Away (`PutAway`);
    /// planks moved into the store by hand do not, though they are stocked.
    #[test]
    fn planks_put_away_finish_put_it_away() {
        let mut server = fixture();
        home_barrel(&mut server, "base:oak_planks", 1);
        // Open: From the Shelves, the quest before it, is done.
        server.players.get_mut(&1).unwrap().quests_done.push("base:from_the_shelves".into());
        // Planks moved in by hand count for Stocked, not for the key the quest teaches.
        give(&mut server, 0, "base:oak_planks", 40);
        open(&mut server);
        act(&mut server, "core:transfer", ActionTarget::Slots { from: 0, to: STORE as u8, count: 40 });
        server.close_store(1);
        server.update_quests(1);
        assert!(!server.players[&1].quests_done.iter().any(|q| q == "base:put_it_away"), "done by moving planks in by hand");
        give(&mut server, 12, "base:oak_planks", 20);
        act(&mut server, "base:put_away", ActionTarget::None);
        server.update_quests(1);
        let done = |server: &Server| server.players[&1].quests_done.iter().any(|q| q == "base:put_it_away");
        assert!(!done(&server), "done with twenty planks put away of thirty-two");
        give(&mut server, 13, "base:oak_planks", 12);
        act(&mut server, "base:put_away", ActionTarget::None);
        server.update_quests(1);
        assert!(done(&server), "thirty-two planks put away and Put It Away not done");
    }

    /// A store with no room keeps what does not fit in the pack.
    #[test]
    fn putting_away_into_a_full_store_leaves_the_rest_in_the_pack() {
        let mut server = fixture();
        home_barrel(&mut server, "base:oak_planks", 60);
        let machines = server.content.machines.clone();
        let stone = id(&server, "base:cobblestone");
        let room = server.factory.get(AT).unwrap().1.bin.len();
        for _ in 1..room {
            assert_eq!(server.factory.put(AT, ItemStack::new(stone, 64), &machines), None);
        }
        give(&mut server, 12, "base:oak_planks", 20);
        act(&mut server, "base:put_away", ActionTarget::None);
        let planks = id(&server, "base:oak_planks");
        assert_eq!(slot(&server, 12), Some((planks, 16)), "four fit");
        assert!(held(&server).contains(&(planks, 64)));
        // With the store full, the player is told it has no room, not that nothing matched.
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        act(&mut server, "base:put_away", ActionTarget::None);
        assert_eq!(slot(&server, 12), Some((planks, 16)), "nothing more fits");
        let mut told = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Chat { text: Words::Key(key, _), .. } = message {
                told.push(key);
            }
        }
        assert_eq!(told, ["store.put_away.full"]);
    }

    /// Away from home, nothing moves.
    #[test]
    fn putting_away_away_from_home_moves_nothing() {
        let mut server = fixture();
        home_barrel(&mut server, "base:oak_planks", 10);
        give(&mut server, 12, "base:oak_planks", 20);
        server.players.get_mut(&1).unwrap().position = Vec3::new(28.5, (FLOOR + 1) as f32, 28.5);
        act(&mut server, "base:put_away", ActionTarget::None);
        let planks = id(&server, "base:oak_planks");
        assert_eq!(slot(&server, 12), Some((planks, 20)));
        assert_eq!(held(&server), vec![(planks, 10)]);
    }
}
