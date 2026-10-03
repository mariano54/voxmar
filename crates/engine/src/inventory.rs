//! Player inventory rules, shared by the server (authority) and client (display).

use serde::{Deserialize, Serialize};

use crate::block::BlockId;

pub const HOTBAR_SLOTS: usize = 9;
pub const SLOTS: usize = 36;
pub const MAX_STACK: u8 = 64;

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
}

impl Default for Inventory {
    fn default() -> Self {
        Self { slots: vec![None; SLOTS] }
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
        self.slots.get(slot).copied().flatten()
    }

    /// Moves the stack in `from` onto `to`, merging equal items and swapping otherwise.
    pub fn move_stack(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= SLOTS || to >= SLOTS {
            return false;
        }
        match (self.slots[from], self.slots[to]) {
            (Some(a), Some(mut b)) if a.item == b.item => {
                let moved = a.count.min(MAX_STACK - b.count);
                b.count += moved;
                self.slots[to] = Some(b);
                self.slots[from] = (a.count > moved).then_some(ItemStack { count: a.count - moved, ..a });
            }
            _ => self.slots.swap(from, to),
        }
        true
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
        assert!(inv.move_stack(1, 0));
        assert_eq!(inv.get(0).unwrap().count, 64);
        assert_eq!(inv.get(1).unwrap().count, 35);
    }
}
