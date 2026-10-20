//! Player inventory rules, shared by the server (authority) and client (prediction and
//! display).
//!
//! Every way of moving items in the inventory screen (Minecraft's clicks, right clicks,
//! shift clicks, drags, double clicks and number keys) comes down to a few moves here:
//! [`Inventory::move_items`] between two slots, one of which may be the stack held on the
//! pointer ([`CURSOR`]), [`Inventory::quick_move`] between the hotbar and the rest, and
//! [`Inventory::stow_cursor`] when the screen closes. The crafting grid is nine more slots
//! ([`CRAFT`]; two by two of them away from a crafting table, see `craft::SMALL`), and [`Inventory::craft`] turns what it holds into a recipe's
//! output. Seven more slots ([`WORN`]) hold the armour worn, a shield and a charm, each only the
//! piece made for it (`combat::Slot`). While a store is open (a chest, a cupboard: `machine::Role::Bin`),
//! its slots follow ([`STORE`]), a copy of what it holds that the server keeps in step with it.
//! The client works out which moves a gesture means; the server checks and applies each the
//! same way.

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
static POUCHES: std::sync::RwLock<Vec<Option<(BlockId, u16)>>> = std::sync::RwLock::new(Vec::new());

/// Sets each item's pouch, by block id, and how many of the item it holds (content's
/// `holds_each:N` tag on the pouch).
pub fn set_pouches(pouches: Vec<Option<(BlockId, u16)>>) {
    *POUCHES.write().unwrap_or_else(|e| e.into_inner()) = pouches;
}

/// The pouch item that carries `item`, if any, and how many of it that holds.
pub fn pouch_holding(item: BlockId) -> Option<(BlockId, u16)> {
    POUCHES.read().unwrap_or_else(|e| e.into_inner()).get(item.0 as usize).copied().flatten()
}

/// The pouch item that carries `item`, if any.
pub fn pouch_for(item: BlockId) -> Option<BlockId> {
    pouch_holding(item).map(|(pouch, _)| pouch)
}

/// How many of one kind a pouch holds unless its `holds_each:N` says, and how many kinds
/// all pouches together.
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

/// `stack` in stacks no bigger than its item stacks to, keeping its wear.
pub fn split_stack(stack: ItemStack) -> impl Iterator<Item = ItemStack> {
    let most = max_stack(stack.item).max(1);
    let mut left = stack.count;
    std::iter::from_fn(move || {
        let count = left.min(most);
        left -= count;
        (count > 0).then_some(ItemStack { count, ..stack })
    })
}

/// The slot number of the stack held on the pointer while the inventory screen is open.
pub const CURSOR: usize = SLOTS;
/// The crafting grid's first slot; its nine are `CRAFT..CRAFT + 9`, row by row.
pub const CRAFT: usize = CURSOR + 1;
/// The first worn slot; its seven are head, body, legs, hands, feet, the off-hand's
/// shield and a charm (`combat::Slot`).
pub const WORN: usize = CRAFT + GRID;
pub const WORN_SLOTS: usize = 7;
/// The last slot number of the player's own.
pub const LAST: usize = WORN + WORN_SLOTS - 1;
/// The first slot of an open store, after the player's own; a store has at most
/// [`STORE_SLOTS`].
pub const STORE: usize = LAST + 1;
pub const STORE_SLOTS: usize = 54;

/// What each store takes, by its block id: `None` (or no entry), anything; else only these
/// items, sorted (`machines.ron`'s `holds`: a jar what is edible).
static STORE_HOLDS: std::sync::RwLock<Vec<Option<Vec<BlockId>>>> = std::sync::RwLock::new(Vec::new());

/// Sets what each store takes, by block id.
pub fn set_store_holds(holds: Vec<Option<Vec<BlockId>>>) {
    *STORE_HOLDS.write().unwrap_or_else(|e| e.into_inner()) = holds;
}

/// Whether the store `store` (a block) takes `item`.
pub fn store_takes(store: BlockId, item: BlockId) -> bool {
    STORE_HOLDS.read().unwrap_or_else(|e| e.into_inner()).get(store.0 as usize).and_then(Option::as_ref).is_none_or(|holds| holds.binary_search(&item).is_ok())
}

/// Each item's worn slot, by block id (`WORN` + it), as content's armour sets say.
static WORN_AT: std::sync::RwLock<Vec<Option<u8>>> = std::sync::RwLock::new(Vec::new());

/// Sets each item's worn slot, by block id.
pub fn set_worn_slots(slots: Vec<Option<u8>>) {
    *WORN_AT.write().unwrap_or_else(|e| e.into_inner()) = slots;
}

/// The worn slot `item` goes in, counted from [`WORN`], if it is armour.
pub fn worn_slot(item: BlockId) -> Option<usize> {
    WORN_AT.read().unwrap_or_else(|e| e.into_inner()).get(item.0 as usize).copied().flatten().map(usize::from)
}

/// Whether `stack` may be in `slot`: anything anywhere but a worn slot, which holds one of
/// the armour made for it (`worn` says where each item is worn).
fn fits(slot: usize, stack: Option<ItemStack>, worn: &impl Fn(BlockId) -> Option<usize>) -> bool {
    match (slot, stack) {
        (WORN..=LAST, Some(stack)) => stack.count == 1 && worn(stack.item) == Some(slot - WORN),
        _ => true,
    }
}

/// The most of `item` that `slot` holds.
fn holds(slot: usize, item: BlockId) -> u8 {
    if (WORN..=LAST).contains(&slot) { 1 } else { max_stack(item) }
}

/// Items are blocks for now; dedicated item definitions come with tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: BlockId,
    pub count: u8,
    /// Uses worn off it, for an item that wears out (a tool's `uses:N`, see
    /// `tool::ToolRegistry::uses`): 0 when new, and for everything that does not wear.
    /// Only a stack of one is ever worn, and worn stacks never merge.
    #[serde(default)]
    pub wear: u16,
}

impl ItemStack {
    /// `count` of `item`, new.
    pub const fn new(item: BlockId, count: u8) -> Self {
        Self { item, count, wear: 0 }
    }
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
    /// The armour worn, slots [`WORN`] on.
    #[serde(default)]
    pub worn: [Option<ItemStack>; WORN_SLOTS],
    /// What the player's pouches carry: herbs and seeds, many of a kind in one slot's
    /// pouch (see [`pouch_for`]). It stays with the player; a pouch is needed to put more in.
    #[serde(default)]
    pub pouched: Vec<(BlockId, u16)>,
    /// The store open beside the inventory, slots [`STORE`] on, if one is.
    #[serde(default)]
    pub store: Option<Store>,
}

/// An open store, as the player sees it: which block it is (what it takes) and what it
/// holds. Never the player's: it is not saved with them, and it closes with the screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub block: BlockId,
    pub slots: Vec<Option<ItemStack>>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self { slots: vec![None; SLOTS], cursor: None, craft: [None; GRID], worn: [None; WORN_SLOTS], pouched: Vec::new(), store: None }
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
            *slot = Some(ItemStack::new(item, moved));
            count -= moved;
        }
        count
    }

    /// Breaks up stacks bigger than their item stacks to (an older save's stack of tools,
    /// from before tools wore), the rest going into empty slots one stack at a time.
    /// Returns what found no room.
    pub fn split_overfull(&mut self) -> Vec<ItemStack> {
        let mut extra = Vec::new();
        for stack in self.slots.iter_mut().flatten() {
            let most = max_stack(stack.item);
            if stack.count > most {
                extra.extend(split_stack(ItemStack { count: stack.count - most, ..*stack }));
                stack.count = most;
            }
        }
        let mut extra = extra.into_iter();
        for slot in self.slots.iter_mut().filter(|s| s.is_none()) {
            let Some(stack) = extra.next() else { break };
            *slot = Some(stack);
        }
        extra.collect()
    }

    /// Adds a stack as [`Inventory::add`] does, keeping its wear: a worn one takes an empty
    /// slot of its own. Returns how many did not fit.
    pub fn add_stack(&mut self, stack: ItemStack) -> u8 {
        if stack.wear == 0 {
            return self.add(stack.item, stack.count);
        }
        match self.slots.iter_mut().find(|s| s.is_none()) {
            Some(slot) => {
                *slot = Some(stack);
                0
            }
            None => stack.count,
        }
    }

    /// Mends the tool in the crafting grid into `mended` (`tool::ToolRegistry::mended`
    /// said what the grid makes), taking one item from each filled cell: onto the empty
    /// pointer, or with `all` into the inventory. Returns whether it did.
    pub fn mend(&mut self, mended: ItemStack, all: bool) -> bool {
        if all {
            if !self.slots.iter().any(Option::is_none) {
                return false;
            }
        } else if self.cursor.is_some() {
            return false;
        }
        for cell in &mut self.craft {
            if let Some(stack) = cell {
                stack.count -= 1;
                if stack.count == 0 {
                    *cell = None;
                }
            }
        }
        if all {
            self.add_stack(mended);
        } else {
            self.cursor = Some(mended);
        }
        true
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
        let each = pouch_holding(item).map_or(POUCH_EACH, |(_, each)| each);
        let moved = (count as u16).min(each.saturating_sub(*held));
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
            *slot = Some(ItemStack::new(item, moved));
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

    /// Turns one item of `slot` into `item` (a skep that took a swarm): in the same slot
    /// if it was the last there, else wherever it fits. Returns how many did not fit.
    pub fn replace_one(&mut self, slot: usize, item: BlockId) -> u8 {
        if self.take_one(slot).is_none() {
            return 1;
        }
        if self.slots.get(slot).is_some_and(|s| s.is_none()) {
            self.slots[slot] = Some(ItemStack::new(item, 1));
            return 0;
        }
        self.add(item, 1)
    }

    pub fn get(&self, slot: usize) -> Option<ItemStack> {
        match slot {
            CURSOR => self.cursor,
            CRAFT..WORN => self.craft[slot - CRAFT],
            WORN..=LAST => self.worn[slot - WORN],
            STORE.. => self.store.as_ref()?.slots.get(slot - STORE).copied().flatten(),
            _ => self.slots.get(slot).copied().flatten(),
        }
    }

    fn set(&mut self, slot: usize, stack: Option<ItemStack>) {
        let stack = stack.filter(|s| s.count > 0);
        match slot {
            CURSOR => self.cursor = stack,
            CRAFT..WORN => self.craft[slot - CRAFT] = stack,
            WORN..=LAST => self.worn[slot - WORN] = stack,
            STORE.. => {
                if let Some(at) = self.store.as_mut().and_then(|store| store.slots.get_mut(slot - STORE)) {
                    *at = stack;
                }
            }
            _ => self.slots[slot] = stack,
        }
    }

    /// The last slot number there is now: the player's own, and an open store's.
    pub fn last(&self) -> usize {
        LAST + self.store.as_ref().map_or(0, |store| store.slots.len())
    }

    /// Whether `stack` may be in `slot`: [`fits`], and in a store only what it takes.
    fn may_hold(&self, slot: usize, stack: Option<ItemStack>, worn: &impl Fn(BlockId) -> Option<usize>) -> bool {
        let store = |item: BlockId| self.store.as_ref().is_some_and(|store| store_takes(store.block, item));
        fits(slot, stack, worn) && (slot < STORE || stack.is_none_or(|s| store(s.item)))
    }

    /// Closes the open store, if one is. Returns whether one was.
    pub fn close_store(&mut self) -> bool {
        self.store.take().is_some()
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
                    None => self.cursor = Some(ItemStack::new(output, count)),
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
        self.move_worn(from, to, count, &worn_slot)
    }

    /// [`Inventory::move_items`], with `worn` saying where each item is worn.
    fn move_worn(&mut self, from: usize, to: usize, count: u8, worn: &impl Fn(BlockId) -> Option<usize>) -> bool {
        if from == to || from > self.last() || to > self.last() || count == 0 {
            return false;
        }
        let Some(source) = self.get(from) else { return false };
        match self.get(to) {
            None => {
                let moved = count.min(source.count).min(holds(to, source.item));
                if !self.may_hold(to, Some(ItemStack { count: moved, ..source }), worn) {
                    return false;
                }
                self.set(to, Some(ItemStack { count: moved, ..source }));
                self.set(from, Some(ItemStack { count: source.count - moved, ..source }));
            }
            Some(target) if target.item == source.item && target.wear == source.wear => {
                let moved = count.min(source.count).min(holds(to, source.item).saturating_sub(target.count));
                if moved == 0 {
                    return false;
                }
                self.set(to, Some(ItemStack { count: target.count + moved, ..target }));
                self.set(from, Some(ItemStack { count: source.count - moved, ..source }));
            }
            Some(target) if count >= source.count && self.may_hold(to, Some(source), worn) && self.may_hold(from, Some(target), worn) => {
                self.set(to, Some(source));
                self.set(from, Some(target));
            }
            Some(_) => return false,
        }
        true
    }

    /// Shift-click: moves a stack from the hotbar into the rest of the inventory or back,
    /// topping up stacks of the same item first, then into the first empty slots; while a
    /// store is open, between it and the pack. Returns whether anything moved.
    pub fn quick_move(&mut self, slot: usize) -> bool {
        self.quick_move_worn(slot, &worn_slot)
    }

    /// [`Inventory::quick_move`], with `worn` saying where each item is worn.
    fn quick_move_worn(&mut self, slot: usize, worn: &impl Fn(BlockId) -> Option<usize>) -> bool {
        // An open store's stacks go into the pack, and the pack's into the store.
        if let Some(stack) = self.get(slot).filter(|_| self.store.is_some() && (slot < SLOTS || slot >= STORE)) {
            if slot >= STORE {
                let left = self.add_stack(stack);
                self.set(slot, Some(ItemStack { count: left, ..stack }));
                return left != stack.count;
            }
            let Some(store) = self.store.as_mut().filter(|store| store_takes(store.block, stack.item)) else { return false };
            let mut left = stack.count;
            for target in store.slots.iter_mut().flatten().filter(|t| t.item == stack.item && t.wear == stack.wear) {
                let moved = left.min(max_stack(stack.item).saturating_sub(target.count));
                target.count += moved;
                left -= moved;
            }
            for target in store.slots.iter_mut().filter(|t| t.is_none()) {
                if left == 0 {
                    break;
                }
                *target = Some(ItemStack { count: left, ..stack });
                left = 0;
            }
            self.set(slot, Some(ItemStack { count: left, ..stack }));
            return left != stack.count;
        }
        // Armour goes on, into its worn slot, if that is empty.
        if let Some(stack) = self.get(slot).filter(|_| slot < SLOTS) {
            if let Some(at) = worn(stack.item).filter(|&at| self.worn[at].is_none()) {
                return self.move_worn(slot, WORN + at, 1, worn);
            }
        }
        if (CRAFT..=LAST).contains(&slot) {
            // Out of the crafting grid or off, into the inventory.
            let Some(stack) = self.get(slot) else { return false };
            let left = self.add_stack(stack);
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
            if let Some(target) = self.slots[to].as_mut().filter(|t| t.item == stack.item && t.wear == stack.wear) {
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
        for slot in std::iter::once(CURSOR).chain(CRAFT..WORN) {
            let Some(stack) = self.get(slot) else { continue };
            let left = self.add_stack(stack);
            self.set(slot, Some(ItemStack { count: left, ..stack }));
            moved |= left != stack.count;
        }
        moved
    }

    /// How many of `item` the slots hold (not the pouches, the pointer or the grid).
    pub fn count(&self, item: BlockId) -> u32 {
        self.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum()
    }

    /// Takes every one of `wants` (item and how many) from the slots, from `first` before
    /// the others, or nothing if any falls short. Returns whether it took them.
    pub fn take_all(&mut self, wants: &[(BlockId, u8)], first: usize) -> bool {
        let needed = |item: BlockId| wants.iter().filter(|w| w.0 == item).map(|w| w.1 as u32).sum::<u32>();
        if wants.iter().any(|&(item, _)| self.count(item) < needed(item)) {
            return false;
        }
        for &(item, n) in wants {
            let mut left = n;
            let order = std::iter::once(first).chain((0..self.slots.len()).filter(|&s| s != first));
            for slot in order {
                if left == 0 {
                    break;
                }
                if self.slots.get(slot).copied().flatten().is_some_and(|s| s.item == item) {
                    left -= self.take(slot, left).map_or(0, |s| s.count);
                }
            }
        }
        true
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
        // What pouches hold goes too.
        self.pouched.retain(|&(item, n)| {
            let gone = which(item);
            count += if gone { n as u32 } else { 0 };
            !gone
        });
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
    fn taking_several_items_takes_all_or_none_held_slot_first() {
        let mut inv = Inventory::default();
        inv.slots[0] = stack(SAND, 2);
        inv.slots[4] = stack(SAND, 5);
        inv.slots[6] = stack(DIRT, 1);
        assert!(!inv.take_all(&[(SAND, 3), (DIRT, 2)], 4), "short of dirt");
        assert_eq!((inv.count(SAND), inv.count(DIRT)), (7, 1), "nothing taken");
        assert!(inv.take_all(&[(SAND, 6), (DIRT, 1)], 4));
        assert_eq!((inv.slots[4], inv.slots[0], inv.count(DIRT)), (None, stack(SAND, 1), 0), "the held slot went first");
    }

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
        assert_eq!(inv.get(0), Some(ItemStack::new(BlockId(1), 64)));
        assert_eq!(inv.get(1), Some(ItemStack::new(BlockId(1), 36)));
        assert_eq!(inv.take_one(1), Some(BlockId(1)));
        assert_eq!(inv.get(1).unwrap().count, 35);
        assert!(!inv.move_items(1, 0, 35), "a full stack takes no more");
        assert_eq!(inv.get(0).unwrap().count, 64);
        assert_eq!(inv.get(1).unwrap().count, 35);
    }

    const DIRT: BlockId = BlockId(1);
    const SAND: BlockId = BlockId(2);

    fn stack(item: BlockId, count: u8) -> Option<ItemStack> {
        Some(ItemStack::new(item, count))
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
        pouches[900] = Some((pouch, 1000));
        pouches[901] = Some((pouch, 1000));
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

    #[test]
    fn armour_is_worn_only_in_its_own_slot_and_one_at_a_time() {
        const HELM: BlockId = BlockId(3);
        // Helms go on the head (the content's table is global, so the test passes its own).
        let worn = |item: BlockId| (item == HELM).then_some(0);
        let mut inv = Inventory::default();
        inv.slots[0] = stack(DIRT, 10);
        inv.slots[1] = stack(HELM, 1);
        inv.slots[2] = stack(HELM, 1);
        assert!(!inv.move_worn(0, WORN, 1, &worn), "dirt is not worn");
        assert!(!inv.move_worn(1, WORN + 1, 1, &worn), "a helm goes on the head");
        // Shift-click puts it on; another goes to storage, the head being taken.
        assert!(inv.quick_move_worn(1, &worn));
        assert_eq!((inv.get(WORN), inv.get(1)), (stack(HELM, 1), None));
        assert!(inv.quick_move_worn(2, &worn));
        assert_eq!((inv.get(2), inv.get(9)), (None, stack(HELM, 1)));
        // Dirt on the pointer cannot swap with the helm worn; a helm can.
        inv.cursor = stack(DIRT, 1);
        assert!(!inv.move_worn(CURSOR, WORN, 1, &worn));
        inv.cursor = None;
        assert!(!inv.move_worn(9, WORN, 1, &worn), "one helm at a time");
        assert!(!inv.move_worn(WORN, 0, 1, &worn), "nor onto dirt, which cannot go on the head");
        assert!(inv.move_worn(WORN, 5, 1, &worn));
        assert_eq!(inv.get(WORN), None);
        // Taken off with a shift-click; cleared with the rest.
        inv.worn[0] = stack(HELM, 1);
        assert!(inv.quick_move_worn(WORN, &worn));
        assert_eq!(inv.get(WORN), None);
        inv.worn[0] = stack(HELM, 1);
        assert_eq!(inv.clear(|i| i == HELM), 4);
    }

    /// Worn tools never merge: two of one tool worn differently swap places, one dropped
    /// into the pack takes a slot of its own, and mending puts the mended tool on the
    /// pointer, taking one from each cell.
    #[test]
    fn worn_stacks_keep_apart() {
        let worn = |wear: u16| Some(ItemStack { wear, ..ItemStack::new(SAND, 1) });
        let mut inv = Inventory::default();
        inv.slots[0] = worn(5);
        inv.slots[1] = worn(9);
        assert!(inv.move_items(0, 1, 1), "swapped");
        assert_eq!((inv.slots[0], inv.slots[1]), (worn(9), worn(5)));
        inv.cursor = worn(3);
        assert!(inv.stow_cursor());
        assert_eq!(inv.slots[2], worn(3), "stowed as worn as it was");
        inv.craft[0] = worn(40);
        inv.craft[1] = stack(DIRT, 2);
        assert!(inv.mend(ItemStack { wear: 10, ..ItemStack::new(SAND, 1) }, false));
        assert_eq!((inv.cursor, inv.craft[0], inv.craft[1]), (worn(10), None, stack(DIRT, 1)));
        assert!(!inv.mend(ItemStack::new(SAND, 1), false), "the pointer is taken");
    }

    /// An open store's slots follow the player's: clicks move stacks in and out, shift
    /// click sends them between it and the pack, nothing past its last slot is there,
    /// and closing it leaves the player what they took and no more.
    #[test]
    fn an_open_store_is_slots_after_the_players_own() {
        // A block no content has, so it takes anything.
        let chest = BlockId(u16::MAX);
        let mut inv = Inventory::default();
        inv.slots[0] = stack(DIRT, 10);
        inv.slots[12] = stack(SAND, 64);
        assert!(!inv.move_items(0, STORE, 1), "no store open, no store slots");
        inv.store = Some(Store { block: chest, slots: vec![None, stack(SAND, 60), None] });
        assert_eq!(inv.last(), LAST + 3);
        assert!(inv.move_items(0, STORE, 4));
        assert_eq!((inv.get(0), inv.get(STORE)), (stack(DIRT, 6), stack(DIRT, 4)));
        assert!(!inv.move_items(0, STORE + 3, 1), "past its last slot");
        // Shift click from the pack tops up its stack of sand, then takes an empty slot.
        assert!(inv.quick_move(12));
        assert_eq!((inv.get(STORE + 1), inv.get(STORE + 2), inv.get(12)), (stack(SAND, 64), stack(SAND, 60), None));
        // And from the store, into the pack.
        assert!(inv.quick_move(STORE));
        assert_eq!((inv.get(STORE), inv.get(0)), (None, stack(DIRT, 10)));
        assert!(inv.move_items(STORE + 2, CURSOR, 60));
        assert!(inv.close_store() && !inv.close_store());
        assert_eq!((inv.get(STORE + 1), inv.last()), (None, LAST));
        assert_eq!(inv.cursor, stack(SAND, 60), "what was taken stays the player's");
    }
}
