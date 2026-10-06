//! Player inventory rules, shared by the server (authority) and client (prediction and
//! display).
//!
//! Every way of moving items in the inventory screen (Minecraft's clicks, right clicks,
//! shift clicks, drags, double clicks and number keys) comes down to a few moves here:
//! [`Inventory::move_items`] between two slots, one of which may be the stack held on the
//! pointer ([`CURSOR`]), [`Inventory::quick_move`] between the hotbar and the rest, and
//! [`Inventory::stow_cursor`] when the screen closes. The crafting grid is nine more slots
//! ([`CRAFT`]; two by two of them away from a crafting table, see `craft::SMALL`), and [`Inventory::craft`] turns what it holds into a recipe's
//! output. The client works out which moves a gesture means; the server checks and
//! applies each the same way.

use serde::{Deserialize, Serialize};

use crate::block::BlockId;
use crate::craft::GRID;

pub const HOTBAR_SLOTS: usize = 9;
pub const SLOTS: usize = 36;
/// The most items one stack holds, unless an item holds fewer (see [`max_stack`]).
pub const MAX_STACK: u8 = 64;

/// Each item's most per stack, by block id, as content sets them (`stacking.ron`: cooked
/// food and potions take a slot each, as RuneScape's do, so what you carry into a fight is
/// a choice). Unset, every item stacks to [`MAX_STACK`].
static LIMITS: std::sync::RwLock<Vec<u8>> = std::sync::RwLock::new(Vec::new());

/// Each item's pouch, by block id: the pouch item that carries it when the player has one
/// (content's pouches: a pouch tagged `holds:seed` carries what is tagged `seed`).
static POUCHES: std::sync::RwLock<Vec<Option<BlockId>>> = std::sync::RwLock::new(Vec::new());

/// Sets each item's pouch, by block id.
pub fn set_pouches(pouches: Vec<Option<BlockId>>) {
    *POUCHES.write().unwrap_or_else(|e| e.into_inner()) = pouches;
}

/// The pouch item that carries `item`, if any.
pub fn pouch_for(item: BlockId) -> Option<BlockId> {
    POUCHES.read().unwrap_or_else(|e| e.into_inner()).get(item.0 as usize).copied().flatten()
}

/// How many of one kind a pouch holds, and how many kinds all pouches together.
pub const POUCH_EACH: u16 = 1000;
pub const POUCH_KINDS: usize = 40;

/// Sets each item's most per stack, by block id.
pub fn set_stack_limits(limits: Vec<u8>) {
    *LIMITS.write().unwrap_or_else(|e| e.into_inner()) = limits;
}

/// The most of `item` one stack holds.
pub fn max_stack(item: BlockId) -> u8 {
    LIMITS.read().unwrap_or_else(|e| e.into_inner()).get(item.0 as usize).copied().unwrap_or(MAX_STACK)
}
/// The slot number of the stack held on the pointer while the inventory screen is open.
pub const CURSOR: usize = SLOTS;
/// The crafting grid's first slot; its nine are `CRAFT..CRAFT + 9`, row by row.
pub const CRAFT: usize = CURSOR + 1;
/// The last slot number there is.
const LAST: usize = CRAFT + GRID - 1;

/// Items are blocks for now; dedicated item definitions come with tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: BlockId,
    pub count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Slots `0..HOTBAR_SLOTS` are the hotbar.
    pub slots: Vec<Option<ItemStack>>,
    /// The stack held on the pointer, slot [`CURSOR`].
    #[serde(default)]
    pub cursor: Option<ItemStack>,
    /// The crafting grid, slots [`CRAFT`] on.
    #[serde(default)]
    pub craft: [Option<ItemStack>; GRID],
    /// What the player's pouches carry: herbs and seeds, many of a kind in one slot's
    /// pouch (see [`pouch_for`]). It stays with the player; a pouch is needed to put more in.
    #[serde(default)]
    pub pouched: Vec<(BlockId, u16)>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self { slots: vec![None; SLOTS], cursor: None, craft: [None; GRID], pouched: Vec::new() }
    }
}

impl Inventory {
    /// Adds items, topping up existing stacks before using empty slots. Returns how many
    /// did not fit.
    pub fn add(&mut self, item: BlockId, mut count: u8) -> u8 {
        // Into its pouch first, if the player carries one.
        if pouch_for(item).is_some_and(|p| self.slots.iter().flatten().any(|s| s.item == p)) {
            count -= self.pouch(item, count);
        }
        for stack in self.slots.iter_mut().flatten().filter(|s| s.item == item) {
            let moved = count.min(max_stack(item).saturating_sub(stack.count));
            stack.count += moved;
            count -= moved;
        }
        for slot in self.slots.iter_mut().filter(|s| s.is_none()) {
            if count == 0 {
                break;
            }
            let moved = count.min(max_stack(item));
            *slot = Some(ItemStack { item, count: moved });
            count -= moved;
        }
        count
    }

    /// Puts up to `count` of `item` into the pouches, returning how many went in.
    fn pouch(&mut self, item: BlockId, count: u8) -> u8 {
        let index = match self.pouched.iter().position(|(i, _)| *i == item) {
            Some(index) => index,
            None if self.pouched.len() < POUCH_KINDS => {
                self.pouched.push((item, 0));
                self.pouched.len() - 1
            }
            None => return 0,
        };
        let held = &mut self.pouched[index].1;
        let moved = (count as u16).min(POUCH_EACH - *held);
        *held += moved;
        moved as u8
    }

    /// How many of `item` the pouches carry.
    pub fn pouched(&self, item: BlockId) -> u16 {
        self.pouched.iter().find(|(i, _)| *i == item).map_or(0, |&(_, n)| n)
    }

    /// Takes one `item` out of the pouches, if they carry one.
    pub fn take_pouched(&mut self, item: BlockId) -> bool {
        let Some(index) = self.pouched.iter().position(|&(i, n)| i == item && n > 0) else { return false };
        self.pouched[index].1 -= 1;
        if self.pouched[index].1 == 0 {
            self.pouched.remove(index);
        }
        true
    }

    /// Empties what `pouch` carries into the inventory's slots, as far as they have room.
    /// Returns whether anything moved.
    pub fn empty_pouch(&mut self, pouch: BlockId) -> bool {
        let mut moved = false;
        let carried: Vec<(BlockId, u16)> = self.pouched.iter().copied().filter(|&(i, _)| pouch_for(i) == Some(pouch)).collect();
        for (item, count) in carried {
            let mut left = count;
            while left > 0 {
                let batch = left.min(u8::MAX as u16) as u8;
                // Straight into slots, not back into the pouch.
                let placed = self.add_to_slots(item, batch);
                left -= placed as u16;
                moved |= placed > 0;
                if placed == 0 {
                    break;
                }
            }
            match self.pouched.iter().position(|(i, _)| *i == item) {
                Some(index) if left == 0 => {
                    self.pouched.remove(index);
                }
                Some(index) => self.pouched[index].1 = left,
                None => {}
            }
        }
        moved
    }

    /// Adds to the slots only, never a pouch; returns how many went in.
    fn add_to_slots(&mut self, item: BlockId, count: u8) -> u8 {
        let mut left = count;
        for stack in self.slots.iter_mut().flatten().filter(|s| s.item == item) {
            let moved = left.min(max_stack(item).saturating_sub(stack.count));
            stack.count += moved;
            left -= moved;
        }
        for slot in self.slots.iter_mut().filter(|s| s.is_none()) {
            if left == 0 {
                break;
            }
            let moved = left.min(max_stack(item));
            *slot = Some(ItemStack { item, count: moved });
            left -= moved;
        }
        count - left
    }

    /// Removes one item from `slot`, returning what it was.
    pub fn take_one(&mut self, slot: usize) -> Option<BlockId> {
        let stack = self.slots.get_mut(slot)?.as_mut()?;
        let item = stack.item;
        stack.count -= 1;
        if stack.count == 0 {
            self.slots[slot] = None;
        }
        Some(item)
    }

    pub fn get(&self, slot: usize) -> Option<ItemStack> {
        match slot {
            CURSOR => self.cursor,
            CRAFT..=LAST => self.craft[slot - CRAFT],
            _ => self.slots.get(slot).copied().flatten(),
        }
    }

    fn set(&mut self, slot: usize, stack: Option<ItemStack>) {
        let stack = stack.filter(|s| s.count > 0);
        match slot {
            CURSOR => self.cursor = stack,
            CRAFT..=LAST => self.craft[slot - CRAFT] = stack,
            _ => self.slots[slot] = stack,
        }
    }

    /// What the crafting grid holds, item by item.
    pub fn grid(&self) -> [Option<BlockId>; GRID] {
        self.craft.map(|stack| stack.map(|s| s.item))
    }

    /// Crafts what the grid holds into `output` (`count` of it), taking one item from each
    /// filled cell: once, onto the pointer (which must be empty or hold the same item with
    /// room), or with `all` as many times as the grid allows, into the inventory while it
    /// has room. `recipe` names what a grid makes. Returns how many times it crafted.
    pub fn craft(&mut self, recipe: impl Fn(&[Option<BlockId>; GRID]) -> Option<(BlockId, u8)>, all: bool) -> u32 {
        let mut made = 0;
        while let Some((output, count)) = recipe(&self.grid()) {
            if all {
                let mut trial = self.clone();
                if trial.add(output, count) > 0 {
                    break;
                }
                self.add(output, count);
            } else {
                match self.cursor {
                    None => self.cursor = Some(ItemStack { item: output, count }),
                    Some(held) if held.item == output && held.count as u32 + count as u32 <= max_stack(output) as u32 => {
                        self.cursor = Some(ItemStack { count: held.count + count, ..held });
                    }
                    Some(_) => break,
                }
            }
            for cell in &mut self.craft {
                if let Some(stack) = cell {
                    stack.count -= 1;
                    if stack.count == 0 {
                        *cell = None;
                    }
                }
            }
            made += 1;
            if !all {
                break;
            }
        }
        made
    }

    /// Moves up to `count` items from `from` to `to` (either may be [`CURSOR`]): onto an
    /// empty slot or the same item, as many as fit, or swapping the two stacks when all of
    /// `from` goes onto a different item. Returns whether anything moved.
    pub fn move_items(&mut self, from: usize, to: usize, count: u8) -> bool {
        if from == to || from > LAST || to > LAST || count == 0 {
            return false;
        }
        let Some(source) = self.get(from) else { return false };
        match self.get(to) {
            None => {
                let moved = count.min(source.count);
                self.set(to, Some(ItemStack { count: moved, ..source }));
                self.set(from, Some(ItemStack { count: source.count - moved, ..source }));
            }
            Some(target) if target.item == source.item => {
                let moved = count.min(source.count).min(max_stack(source.item).saturating_sub(target.count));
                if moved == 0 {
                    return false;
                }
                self.set(to, Some(ItemStack { count: target.count + moved, ..target }));
                self.set(from, Some(ItemStack { count: source.count - moved, ..source }));
            }
            Some(target) if count >= source.count => {
                self.set(to, Some(source));
                self.set(from, Some(target));
            }
            Some(_) => return false,
        }
        true
    }

    /// Shift-click: moves a stack from the hotbar into the rest of the inventory or back,
    /// topping up stacks of the same item first, then into the first empty slots. Returns
    /// whether anything moved.
    pub fn quick_move(&mut self, slot: usize) -> bool {
        if (CRAFT..=LAST).contains(&slot) {
            // Out of the crafting grid, into the inventory.
            let Some(stack) = self.get(slot) else { return false };
            let left = self.add(stack.item, stack.count);
            self.set(slot, Some(ItemStack { count: left, ..stack }));
            return left != stack.count;
        }
        if slot >= SLOTS {
            return false;
        }
        let Some(mut stack) = self.get(slot) else { return false };
        let others = if slot < HOTBAR_SLOTS { HOTBAR_SLOTS..SLOTS } else { 0..HOTBAR_SLOTS };
        let before = stack.count;
        for to in others.clone() {
            if let Some(target) = self.slots[to].as_mut().filter(|t| t.item == stack.item) {
                let moved = stack.count.min(max_stack(stack.item).saturating_sub(target.count));
                target.count += moved;
                stack.count -= moved;
            }
        }
        for to in others {
            if stack.count == 0 {
                break;
            }
            if self.slots[to].is_none() {
                self.slots[to] = Some(stack);
                stack.count = 0;
            }
        }
        self.set(slot, Some(stack));
        stack.count != before
    }

    /// Puts the stack on the pointer and what the crafting grid holds back into the
    /// inventory, as [`Inventory::add`] would. Whatever does not fit stays where it was.
    pub fn stow_cursor(&mut self) -> bool {
        let mut moved = false;
        for slot in std::iter::once(CURSOR).chain(CRAFT..=LAST) {
            let Some(stack) = self.get(slot) else { continue };
            let left = self.add(stack.item, stack.count);
            self.set(slot, Some(ItemStack { count: left, ..stack }));
            moved |= left != stack.count;
        }
        moved
    }

    /// How many of `item` [`Inventory::add`] would take, up to `count`.
    pub fn room_for(&self, item: BlockId, count: u8) -> u8 {
        let room: u32 = self
            .slots
            .iter()
            .map(|slot| match slot {
                None => max_stack(item) as u32,
                Some(stack) if stack.item == item => max_stack(item).saturating_sub(stack.count) as u32,
                Some(_) => 0,
            })
            .sum();
        room.min(count as u32) as u8
    }

    /// Takes up to `count` items from `slot` (or [`CURSOR`]), returning what it took.
    /// Takes out every stack of an item `which` picks, the pointer's and the crafting
    /// grid's too, returning how many items went.
    pub fn clear(&mut self, which: impl Fn(BlockId) -> bool) -> u32 {
        let mut count = 0;
        for slot in 0..=LAST {
            if let Some(stack) = self.get(slot).filter(|s| which(s.item)) {
                count += stack.count as u32;
                self.set(slot, None);
            }
        }
        count
    }

    pub fn take(&mut self, slot: usize, count: u8) -> Option<ItemStack> {
        let stack = self.get(slot).filter(|_| slot <= CURSOR)?;
        let taken = count.min(stack.count);
        if taken == 0 {
            return None;
        }
        self.set(slot, Some(ItemStack { count: stack.count - taken, ..stack }));
        Some(ItemStack { count: taken, ..stack })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_and_taking_part_of_a_stack() {
        let mut inv = Inventory { slots: vec![stack(SAND, 64); SLOTS], ..Default::default() };
        inv.slots[3] = stack(DIRT, 60);
        assert_eq!((inv.room_for(DIRT, 10), inv.room_for(SAND, 10)), (4, 0));
        inv.slots[7] = None;
        assert_eq!(inv.room_for(DIRT, 255), 68);
        assert_eq!(inv.take(3, 1), stack(DIRT, 1));
        assert_eq!(inv.take(3, 100), stack(DIRT, 59));
        assert_eq!((inv.get(3), inv.take(3, 1)), (None, None));
        inv.cursor = stack(DIRT, 2);
        assert_eq!((inv.take(CURSOR, 5), inv.cursor), (stack(DIRT, 2), None));
    }

    #[test]
    fn add_stacks_then_spills() {
        let mut inv = Inventory::default();
        assert_eq!(inv.add(BlockId(1), 100), 0);
        assert_eq!(inv.get(0), Some(ItemStack { item: BlockId(1), count: 64 }));
        assert_eq!(inv.get(1), Some(ItemStack { item: BlockId(1), count: 36 }));
        assert_eq!(inv.take_one(1), Some(BlockId(1)));
        assert_eq!(inv.get(1).unwrap().count, 35);
        assert!(!inv.move_items(1, 0, 35), "a full stack takes no more");
        assert_eq!(inv.get(0).unwrap().count, 64);
        assert_eq!(inv.get(1).unwrap().count, 35);
    }

    const DIRT: BlockId = BlockId(1);
    const SAND: BlockId = BlockId(2);

    fn stack(item: BlockId, count: u8) -> Option<ItemStack> {
        Some(ItemStack { item, count })
    }

    #[test]
    fn clicks_move_items_through_the_cursor() {
        let mut inv = Inventory::default();
        inv.slots[0] = stack(DIRT, 10);
        inv.slots[1] = stack(SAND, 3);
        // A right click picks up half, rounded up.
        assert!(inv.move_items(0, CURSOR, 5));
        assert_eq!((inv.get(0), inv.get(CURSOR)), (stack(DIRT, 5), stack(DIRT, 5)));
        // A right click on an empty slot puts down one.
        assert!(inv.move_items(CURSOR, 5, 1));
        assert_eq!((inv.get(5), inv.get(CURSOR)), (stack(DIRT, 1), stack(DIRT, 4)));
        // A click on a different item swaps it onto the pointer; on the same item, merges.
        assert!(inv.move_items(CURSOR, 1, 4));
        assert_eq!((inv.get(1), inv.get(CURSOR)), (stack(DIRT, 4), stack(SAND, 3)));
        // Part of a stack cannot swap.
        assert!(!inv.move_items(CURSOR, 0, 1));
        assert!(inv.move_items(CURSOR, 9, 3));
        assert!(inv.move_items(1, 0, 4));
        assert_eq!((inv.get(0), inv.get(1), inv.cursor), (stack(DIRT, 9), None, None));
        assert!(!inv.move_items(3, 4, 1), "nothing to move");
    }

    #[test]
    fn shift_clicks_cross_between_hotbar_and_storage() {
        let mut inv = Inventory::default();
        inv.slots[0] = stack(DIRT, 40);
        inv.slots[12] = stack(DIRT, 50);
        inv.slots[20] = stack(SAND, 1);
        // Tops up the stack in storage, then takes the first empty slot there.
        assert!(inv.quick_move(0));
        assert_eq!((inv.get(0), inv.get(9), inv.get(12)), (None, stack(DIRT, 26), stack(DIRT, 64)));
        // Back down to the hotbar.
        assert!(inv.quick_move(20));
        assert_eq!((inv.get(0), inv.get(20)), (stack(SAND, 1), None));
        // A full hotbar takes nothing.
        for slot in 0..HOTBAR_SLOTS {
            inv.slots[slot] = stack(SAND, 64);
        }
        assert!(!inv.quick_move(9));
    }

    #[test]
    fn the_grid_crafts_once_or_all() {
        let mut inv = Inventory::default();
        inv.craft[0] = stack(DIRT, 3);
        // One dirt makes four sand.
        let mut one_dirt = [None; GRID];
        one_dirt[0] = Some(DIRT);
        let recipe = |grid: &[Option<BlockId>; GRID]| (grid == &one_dirt).then_some((SAND, 4));
        assert_eq!(inv.craft(recipe, false), 1);
        assert_eq!((inv.cursor, inv.craft[0]), (stack(SAND, 4), stack(DIRT, 2)));
        // Shift crafts the rest into the inventory.
        assert_eq!(inv.craft(recipe, true), 2);
        assert_eq!((inv.get(0), inv.craft[0]), (stack(SAND, 8), None));
        assert_eq!(inv.craft(recipe, true), 0);
        // Grid slots move like any other, and go back on closing.
        assert!(inv.move_items(0, CRAFT + 3, 8));
        assert_eq!(inv.get(CRAFT + 3), stack(SAND, 8));
        assert!(inv.stow_cursor());
        assert_eq!((inv.get(0), inv.cursor, inv.craft), (stack(SAND, 12), None, [None; GRID]));
    }

    #[test]
    fn closing_stows_the_cursor() {
        let mut inv = Inventory::default();
        inv.slots[0] = stack(DIRT, 60);
        inv.cursor = stack(DIRT, 10);
        assert!(inv.stow_cursor());
        assert_eq!((inv.get(0), inv.get(1), inv.cursor), (stack(DIRT, 64), stack(DIRT, 6), None));
        assert!(!inv.stow_cursor());
    }

    #[test]
    fn a_pouch_carries_many_of_a_kind_in_one_slot() {
        // Items 900 and 901 go in pouch item 902 (ids no other test uses).
        let (seed, other, pouch) = (BlockId(900), BlockId(901), BlockId(902));
        let mut pouches = vec![None; 903];
        pouches[900] = Some(pouch);
        pouches[901] = Some(pouch);
        set_pouches(pouches);
        let mut inv = Inventory::default();
        // Without the pouch, seeds take slots as usual.
        assert_eq!(inv.add(seed, 10), 0);
        assert!(inv.pouched.is_empty());
        inv.add(pouch, 1);
        assert_eq!(inv.add(seed, 200), 0);
        assert_eq!(inv.add(other, 5), 0);
        assert_eq!((inv.pouched(seed), inv.pouched(other)), (200, 5));
        assert!(inv.take_pouched(other));
        assert_eq!(inv.pouched(other), 4);
        // Emptied into the slots: 200 seeds fill four stacks of 64 with the first 10.
        assert!(inv.empty_pouch(pouch));
        assert_eq!(inv.pouched(seed), 0);
        let seeds: u32 = inv.slots.iter().flatten().filter(|s| s.item == seed).map(|s| s.count as u32).sum();
        assert_eq!(seeds, 210);
    }
}
