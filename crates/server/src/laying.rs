//! Laying a recipe (`core:lay`, from the recipe viewer, as NEI's "+" does): the crafting
//! grid is filled with what a recipe for an item takes, `count` sets of it, from the pack
//! (its pouches too) and, while the player stands in their home (`home_reach`), from the
//! stores in it: a home's barrels and cupboards are a larder and a woodshed to craft from.
//! What the grid held goes back into the pack first. A recipe that needs a table, a level
//! the player lacks, or a block they are not near, is passed over; with nothing to lay
//! from, the player is told what is short.

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::craft::{GRID, Recipe};
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::{ItemStack, max_stack};
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;

impl Server {
    /// Lays the grid with `count` sets (at least one) of one of the recipes from `first`
    /// for `ways` on (a recipe book entry's variants, all making one thing).
    pub(crate) fn lay_recipe(&mut self, agent: PlayerId, first: usize, ways: usize, count: u8) {
        let recipes = self.content.recipes.clone();
        let making: Vec<&Recipe> = recipes.iter().skip(first).take(ways.max(1)).collect();
        if making.is_empty() || making.iter().any(|r| r.output != making[0].output) {
            return;
        }
        let allowed = self.allowed_recipes(agent);
        let near = self.near_tags(agent);
        let near = |tag: &str| near.iter().any(|t| t == tag);
        let stores = self.home_stores(agent);
        let Some(player) = self.players.get(&agent) else { return };
        // What the grid and the pointer hold goes back into the pack first.
        let mut trial = player.inventory.clone();
        if let Some(stack) = trial.cursor.take() {
            if trial.add_stack(stack) > 0 {
                self.say(agent, Words::key("craft.lay.room"));
                return;
            }
        }
        for cell in 0..GRID {
            if let Some(stack) = trial.craft[cell].take()
                && trial.add_stack(stack) > 0
            {
                self.say(agent, Words::key("craft.lay.room"));
                return;
            }
        }
        let mut have: FxHashMap<BlockId, u32> = FxHashMap::default();
        for stack in trial.slots.iter().flatten() {
            *have.entry(stack.item).or_default() += stack.count as u32;
        }
        // What the pouches carry (herbs and seeds, and what the grid gave back into them).
        for &(item, n) in &trial.pouched {
            *have.entry(item).or_default() += n as u32;
        }
        for &pos in &stores {
            for stack in self.factory.get(pos).map(|(_, state)| state.bin.as_slice()).unwrap_or_default().iter().flatten() {
                *have.entry(stack.item).or_default() += stack.count as u32;
            }
        }
        let usable: Vec<&Recipe> = making.iter().copied().filter(|r| allowed(r)).collect();
        let laid = (1..=count.max(1)).rev().find_map(|times| {
            usable.iter().find_map(|recipe| {
                let grid = recipe.lay(times, &have, max_stack)?;
                // The grid it lays must make it, or, by a block it is made near, the same thing
                // better (a plain way laid by a chopping block makes its six planks).
                let items = grid.map(|cell| cell.map(|(item, _)| item));
                let makes = |found: &Recipe| std::ptr::eq(found, *recipe) || (recipe.near.is_none() && found.near.is_some() && found.output == recipe.output && allowed(found));
                recipes.find_near(&items, near).is_some_and(makes).then_some(grid)
            })
        });
        let Some(grid) = laid else {
            let words = match usable.first() {
                Some(recipe) => short_words(recipe, &have, &self.content.blocks),
                None => Words::key("craft.lay.cannot"),
            };
            self.say(agent, words);
            return;
        };
        // Taken from the pack first, then from the stores.
        let mut stored = false;
        for (cell, laid) in grid.iter().enumerate() {
            let Some((item, n)) = *laid else { continue };
            let mut left = n;
            for slot in trial.slots.iter_mut() {
                if let Some(stack) = slot.as_mut().filter(|s| s.item == item) {
                    let took = left.min(stack.count);
                    stack.count -= took;
                    left -= took;
                    if stack.count == 0 {
                        *slot = None;
                    }
                }
                if left == 0 {
                    break;
                }
            }
            for (_, n) in trial.pouched.iter_mut().filter(|(pouched, _)| *pouched == item) {
                let took = (left as u16).min(*n);
                *n -= took;
                left -= took as u8;
            }
            trial.pouched.retain(|&(_, n)| n > 0);
            for &pos in &stores {
                if left == 0 {
                    break;
                }
                let Some(state) = self.factory.state_mut(pos) else { continue };
                for slot in state.bin.iter_mut() {
                    if let Some(stack) = slot.as_mut().filter(|s| s.item == item) {
                        let took = left.min(stack.count);
                        stack.count -= took;
                        left -= took;
                        stored |= took > 0;
                        if stack.count == 0 {
                            *slot = None;
                        }
                    }
                    if left == 0 {
                        break;
                    }
                }
            }
            debug_assert_eq!(left, 0, "counted as there");
            trial.craft[cell] = Some(ItemStack::new(item, n));
        }
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.inventory = trial;
        // Laid, the grid shows: an open store's screen, in its place, closes.
        player.store = None;
        player.inventory.close_store();
        player.changed(traits::INVENTORY);
        // A lay that drew on the home's stores counts for the quests that teach it.
        if stored {
            self.tally(agent, "laid", &self.content.blocks.def(making[0].output).name.clone(), 1);
        }
    }

    /// Which recipes `agent` may craft where they are: a table's within reach of one, one
    /// made near a block within reach of such a block, and those whose level they have.
    pub(crate) fn allowed_recipes(&self, agent: PlayerId) -> impl Fn(&Recipe) -> bool + use<> {
        let at_table = self.at_table(agent);
        let near = self.near_tags(agent);
        let (skills, effects, blocks) = (self.content.skills.clone(), self.content.effects.clone(), self.content.blocks.clone());
        let player = self.players.get(&agent);
        let levels: FxHashMap<_, (u8, u8)> = player.map(|player| {
            skills.ids().map(|skill| (skill, (skills.level(player.skills.xp(skill)), voxmar_engine::level::check_level(&skills, &effects, &player.skills, &player.vitals.effects, skill)))).collect()
        }).unwrap_or_default();
        let inventory = player.map(|p| p.inventory.clone()).unwrap_or_default();
        move |r: &Recipe| {
            (at_table || !r.needs_table())
                && r.near.as_deref().is_none_or(|tag| near.iter().any(|t| t == tag))
                && r.skill.is_none_or(|s| levels.get(&s).is_some_and(|&(own, boosted)| voxmar_engine::level::meets_recipe(r, own, boosted, &inventory, &blocks)))
        }
    }

    /// The stores in `agent`'s home, while they stand in it, nearest first.
    pub(crate) fn home_stores(&self, agent: PlayerId) -> Vec<IVec3> {
        self.at_home(agent).map_or_else(Vec::new, |anchor| self.stores_around(anchor))
    }

    /// The stores in the home box around `anchor`, nearest first.
    pub(crate) fn stores_around(&self, anchor: IVec3) -> Vec<IVec3> {
        let rules = &self.survival;
        let mut found = Vec::new();
        for dy in -rules.home_down..=rules.home_up {
            for dz in -rules.home_reach..=rules.home_reach {
                for dx in -rules.home_reach..=rules.home_reach {
                    let pos = anchor + IVec3::new(dx, dy, dz);
                    if let Some((block, _)) = self.factory.get(pos)
                        && self.content.machines.get(block).is_some_and(|m| m.is_store())
                    {
                        found.push(pos);
                    }
                }
            }
        }
        found.sort_by_key(|pos| (*pos - anchor).length_squared());
        found
    }

    fn say(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
    }
}

/// What a recipe is short of, for one set: "Not enough to lay it: 2 Oak Planks".
fn short_words(recipe: &Recipe, have: &FxHashMap<BlockId, u32>, blocks: &voxmar_engine::block::BlockRegistry) -> Words {
    let items = recipe.short(have).iter().map(|&(item, n)| Words::key("multiblock.fuel_stack").with("count", n.to_string()).with("item", Words::Block(blocks.def(item).name.clone()))).collect();
    Words::key("craft.lay.short").with("items", Words::List(items))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use voxmar_engine::action::ActionTarget;
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::glam::Vec3;
    use voxmar_engine::world::Column;

    use super::*;

    const FLOOR: i32 = 199;
    const BARREL: IVec3 = IVec3::new(10, FLOOR + 1, 10);

    /// A stone floor, player 1 standing on it, and a barrel beside them.
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
        let barrel = server.content.blocks.expect("base:barrel");
        server.set_block(BARREL, barrel);
        let machines = server.content.machines.clone();
        if server.factory.get(BARREL).is_none() {
            server.factory.insert(BARREL, barrel, &machines);
        }
        server
    }

    fn id(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    /// Lays the first crafting way the recipe book shows for `output`, as its Lay button.
    fn lay(server: &mut Server, output: &str, count: u8) {
        let action = server.content.actions.expect("core:lay");
        let item = id(server, output);
        let book = voxmar_engine::recipe_book::RecipeBook::new(&server.content);
        let (first, ways) = book
            .making(item)
            .iter()
            .find_map(|entry| match &entry.method {
                voxmar_engine::recipe_book::Method::Craft { variants, first, .. } => Some((*first as u16, variants.len() as u8)),
                _ => None,
            })
            .expect("a crafting way");
        server.propose(1, action, ActionTarget::Recipe { first, ways, count }, Instant::now());
    }

    fn stock(server: &mut Server, name: &str, count: u8) {
        let machines = server.content.machines.clone();
        let item = id(server, name);
        assert_eq!(server.factory.put(BARREL, ItemStack::new(item, count), &machines), None);
    }

    fn grid(server: &Server) -> Vec<(BlockId, u8)> {
        server.players[&1].inventory.craft.iter().flatten().map(|s| (s.item, s.count)).collect()
    }

    fn barrel(server: &Server) -> u32 {
        server.factory.get(BARREL).map_or(0, |(_, state)| state.bin.iter().flatten().map(|s| s.count as u32).sum())
    }

    /// From the pack: what the grid held goes back first, and the logs are laid.
    #[test]
    fn a_recipe_lays_from_the_pack() {
        let mut server = fixture();
        let (log, dirt) = (id(&server, "base:oak_log"), id(&server, "base:dirt"));
        let inventory = &mut server.players.get_mut(&1).unwrap().inventory;
        inventory.slots[0] = Some(ItemStack::new(log, 5));
        inventory.craft[4] = Some(ItemStack::new(dirt, 2));
        lay(&mut server, "base:oak_planks", 3);
        assert_eq!(grid(&server), vec![(log, 3)]);
        let pack = &server.players[&1].inventory.slots;
        assert!(pack.iter().flatten().any(|s| s.item == dirt && s.count == 2) && pack.iter().flatten().any(|s| s.item == log && s.count == 2));
    }

    /// Herbs carried in a pouch are laid from too, and taken out of it.
    #[test]
    fn a_recipe_lays_from_the_pouches() {
        let mut server = fixture();
        let (twine, lavender) = (id(&server, "base:twine"), id(&server, "base:lavender"));
        let crafting = server.content.skills.id("core:crafting").unwrap();
        let xp = server.content.skills.xp_for(5);
        let player = server.players.get_mut(&1).unwrap();
        player.skills.grant(crafting, xp);
        player.inventory.slots[0] = Some(ItemStack::new(twine, 2));
        player.inventory.pouched.push((lavender, 3));
        lay(&mut server, "base:hanging_herbs", 1);
        let mut laid = grid(&server);
        laid.sort();
        let mut want = vec![(twine, 1), (twine, 1), (lavender, 1), (lavender, 1)];
        want.sort();
        assert_eq!(laid, want);
        assert_eq!(server.players[&1].inventory.pouched, vec![(lavender, 1)]);
    }

    /// At home, the home's stores are laid from too; away from it, they are not, and the
    /// player is told what is short.
    #[test]
    fn at_home_a_recipe_lays_from_its_stores() {
        let mut server = fixture();
        stock(&mut server, "base:oak_planks", 6);
        let planks = id(&server, "base:oak_planks");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(planks, 1));
        // No home yet: one plank is not enough for a table.
        lay(&mut server, "base:crafting_table", 1);
        assert!(grid(&server).is_empty());
        // Slept here, it is home.
        server.players.get_mut(&1).unwrap().bed = Some(Vec3::new(8.5, (FLOOR + 1) as f32, 8.5));
        // Laid with the barrel open, its screen closes onto the grid.
        server.open_store(1, BARREL);
        assert!(server.players[&1].inventory.store.is_some());
        lay(&mut server, "base:crafting_table", 1);
        assert_eq!(grid(&server), vec![(planks, 1); 4]);
        assert_eq!(server.players[&1].inventory.slots[0], None, "the pack's plank first");
        assert_eq!(barrel(&server), 3);
        let laid = |server: &Server| server.players[&1].tallies.get("laid:base:crafting_table").copied().unwrap_or(0);
        assert_eq!(laid(&server), 1, "laid from the stores counts for the quest that teaches it");
        assert!(server.players[&1].inventory.store.is_none() && server.players[&1].store.is_none(), "the barrel's screen hid the grid");
        // Walked off past its reach, the barrel is out of reach again.
        server.players.get_mut(&1).unwrap().inventory.craft = [None; GRID];
        server.players.get_mut(&1).unwrap().position = Vec3::new(28.5, (FLOOR + 1) as f32, 28.5);
        lay(&mut server, "base:crafting_table", 1);
        assert!(grid(&server).is_empty());
        assert_eq!(barrel(&server), 3);
        // From the pack alone, it is no lay from the stores.
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(planks, 4));
        lay(&mut server, "base:crafting_table", 1);
        assert_eq!(grid(&server), vec![(planks, 1); 4]);
        assert_eq!(laid(&server), 1, "only a lay that took from a store counts");
    }
    /// A log split near a chopping block gives six planks, away from one the grid's four;
    /// the way made near one lays only near one.
    #[test]
    fn a_chopping_block_splits_a_log_into_two_more_planks() {
        let mut server = fixture();
        let (log, planks) = (id(&server, "base:oak_log"), id(&server, "base:oak_planks"));
        let craft = server.content.actions.expect("core:craft");
        let carried = |server: &Server| server.players[&1].inventory.slots.iter().flatten().filter(|s| s.item == planks).map(|s| s.count as u32).sum::<u32>();
        let split = |server: &mut Server| {
            server.players.get_mut(&1).unwrap().inventory.craft[0] = Some(ItemStack::new(log, 1));
            server.propose(1, craft, ActionTarget::Slot(0), Instant::now());
        };
        split(&mut server);
        assert_eq!(carried(&server), 4, "the grid's own, away from a chopping block");
        let block = IVec3::new(10, FLOOR + 1, 8);
        server.set_block(block, id(&server, "base:chopping_block"));
        split(&mut server);
        assert_eq!(carried(&server), 10, "two more by one");
        // Its way in the recipe book lays only near one.
        let book = voxmar_engine::recipe_book::RecipeBook::new(&server.content);
        let first = book
            .making(planks)
            .iter()
            .find_map(|entry| match &entry.method {
                voxmar_engine::recipe_book::Method::Craft { first, near: Some(_), .. } => Some(*first as u16),
                _ => None,
            })
            .expect("a way near a chopping block");
        server.players.get_mut(&1).unwrap().inventory.slots[1] = Some(ItemStack::new(log, 2));
        let action = server.content.actions.expect("core:lay");
        server.propose(1, action, ActionTarget::Recipe { first, ways: 1, count: 1 }, Instant::now());
        assert_eq!(grid(&server), vec![(log, 1)]);
        // The plain way laid by one lays too (and makes the six).
        server.players.get_mut(&1).unwrap().inventory.craft = [None; GRID];
        lay(&mut server, "base:oak_planks", 1);
        assert_eq!(grid(&server), vec![(log, 1)], "the plain way, laid by a chopping block");
        server.players.get_mut(&1).unwrap().inventory.craft = [None; GRID];
        server.players.get_mut(&1).unwrap().inventory.slots[1] = Some(ItemStack::new(log, 2));
        server.set_block(block, BlockId::AIR);
        server.propose(1, action, ActionTarget::Recipe { first, ways: 1, count: 1 }, Instant::now());
        assert!(grid(&server).is_empty(), "not near one");
    }
}
