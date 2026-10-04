//! Player inventory rules, shared by the server (authority) and client (prediction and
//! display).
//!
//! Every way of moving items in the inventory screen (Minecraft's clicks, right clicks,
//! shift clicks, drags, double clicks and number keys) comes down to a few moves here:
//! [`Inventory::move_items`] between two slots, one of which may be the stack held on the
//! pointer ([`CURSOR`]), [`Inventory::quick_move`] between the hotbar and the rest, and
//! [`Inventory::stow_cursor`] when the screen closes. The client works out which moves a
//! gesture means; the server checks and applies each the same way.

use serde::{Deserialize, Serialize};

use crate::block::BlockId;

pub const HOTBAR_SLOTS: usize = 9;
pub const SLOTS: usize = 36;
pub const MAX_STACK: u8 = 64;
/// The slot number of the stack held on the pointer while the inventory screen is open.
pub const CURSOR: usize = SLOTS;

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
}

impl Default for Inventory {
    fn default() -> Self {
        Self { slots: vec![None; SLOTS], cursor: None }
    }
}

impl Inventory {
    /// Adds items, topping up existing stacks before using empty slots. Returns how many
    /// did not fit.
    pub fn add(&mut self, item: BlockId, mut count: u8) -> u8 {
        for stack in self.slots.iter_mut().flatten().filter(|s| s.item == item) {
            let moved = count.min(MAX_STACK - stack.count);
            stack.count += moved;
            count -= moved;
        }
        for slot in self.slots.iter_mut().filter(|s| s.is_none()) {
            if count == 0 {
                break;
            }
            let moved = count.min(MAX_STACK);
            *slot = Some(ItemStack { item, count: moved });
            count -= moved;
        }
        count
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
        if slot == CURSOR { self.cursor } else { self.slots.get(slot).copied().flatten() }
    }

    fn set(&mut self, slot: usize, stack: Option<ItemStack>) {
        let stack = stack.filter(|s| s.count > 0);
        if slot == CURSOR {
            self.cursor = stack;
        } else {
            self.slots[slot] = stack;
        }
    }

    /// Moves up to `count` items from `from` to `to` (either may be [`CURSOR`]): onto an
    /// empty slot or the same item, as many as fit, or swapping the two stacks when all of
    /// `from` goes onto a different item. Returns whether anything moved.
    pub fn move_items(&mut self, from: usize, to: usize, count: u8) -> bool {
        if from == to || from > CURSOR || to > CURSOR || count == 0 {
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
                let moved = count.min(source.count).min(MAX_STACK - target.count);
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
        if slot >= SLOTS {
            return false;
        }
        let Some(mut stack) = self.get(slot) else { return false };
        let others = if slot < HOTBAR_SLOTS { HOTBAR_SLOTS..SLOTS } else { 0..HOTBAR_SLOTS };
        let before = stack.count;
        for to in others.clone() {
            if let Some(target) = self.slots[to].as_mut().filter(|t| t.item == stack.item) {
                let moved = stack.count.min(MAX_STACK - target.count);
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

    /// Puts the stack on the pointer back into the inventory, as [`Inventory::add`] would.
    /// Whatever does not fit stays on the pointer.
    pub fn stow_cursor(&mut self) -> bool {
        let Some(stack) = self.cursor.take() else { return false };
        let left = self.add(stack.item, stack.count);
        self.set(CURSOR, Some(ItemStack { count: left, ..stack }));
        left != stack.count
    }

}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn closing_stows_the_cursor() {
        let mut inv = Inventory::default();
        inv.slots[0] = stack(DIRT, 60);
        inv.cursor = stack(DIRT, 10);
        assert!(inv.stow_cursor());
        assert_eq!((inv.get(0), inv.get(1), inv.cursor), (stack(DIRT, 64), stack(DIRT, 6), None));
        assert!(!inv.stow_cursor());
    }
}
