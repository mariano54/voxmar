//! What dying costs (Mariano, 2026-10-06): a player keeps their three most valuable items,
//! and everything else they carried stays where they fell, theirs alone to pick up, for
//! 30 minutes; then it is gone. A grave-ward amulet keeps a fourth item and doubles the
//! time; the Keepsake prayer keeps a fourth too, but never a fifth with the amulet (design
//! d86). They wake at home (`Player::home`, set by a bed or hearth), or at world spawn.
//! Worth is gold on the P51 curve (`voxmar_engine::value::worth`), counted a unit at a
//! time; the numbers are content (`value.ron`'s `death`).

use std::collections::HashMap;

use voxmar_engine::block::BlockId;
use voxmar_engine::dropped;
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::{Inventory, ItemStack};
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::items::owner_of;

impl Server {
    /// Leaves what each player who has just died carried where they fell.
    pub(crate) fn bury_the_dead(&mut self) {
        let dead: Vec<PlayerId> = self.players.values().filter(|p| p.joined && !p.health.alive() && !p.buried).map(|p| p.id).collect();
        for id in dead {
            self.bury(id);
        }
    }

    /// Buries `id` if they are dead and not yet buried. Whatever revives, saves or drops a
    /// player calls this first, so no order of messages and ticks skips the grave: a
    /// respawn sent the moment `Died` arrives, before the tick that would bury, still
    /// leaves everything but the kept items behind.
    pub(crate) fn bury_if_dead(&mut self, id: PlayerId) {
        if self.players.get(&id).is_some_and(|p| p.joined && !p.health.alive() && !p.buried) {
            self.bury(id);
        }
    }

    fn bury(&mut self, id: PlayerId) {
        let at = self.grave_site(id);
        let content = self.content.clone();
        let death = content.value.death;
        let player = self.players.get_mut(&id).expect("listed");
        player.buried = true;
        let ward = |item: BlockId| content.blocks.def(item).has_tag("grave_ward");
        let warded = player.inventory.slots.iter().flatten().any(|s| ward(s.item));
        let mut worth = HashMap::new();
        let value = |item: BlockId| {
            if ward(item) {
                return u32::MAX;
            }
            voxmar_engine::value::worth_in(&content, item, &mut worth)
        };
        // Keepsake keeps one more, never past `death.most` (design d86).
        let keep = death.keeps(warded, content.prayers.keeps(&player.praying.on));
        let (kept, left) = split(&player.inventory, usize::from(keep), warded, value);
        player.inventory = kept;
        // Death puts out every prayer.
        player.praying.on.clear();
        player.changed(traits::INVENTORY);
        let owner = owner_of(&player.name);
        let seconds = if warded { death.warded_grave } else { death.grave };
        let lifetime = (seconds * TICKS_PER_SECOND as f32) as u32;
        let cell = at.floor().as_ivec3();
        for (i, stack) in left.into_iter().enumerate() {
            // A pile at the feet, barely spread, so nothing rolls off a ledge or into water
            // out of reach of the one who comes back for it.
            // By the world's count of drops, never the tick of the death.
            let [a, b, c] = self.meta.next_pop(cell, i as u32 ^ 0x9a7e);
            let nudge = Vec3::new(a - 0.5, 0.0, b - 0.5) * 0.4;
            let angle = c * std::f32::consts::TAU;
            let body = dropped::Loose::new(at + nudge + Vec3::Y * 0.25, Vec3::new(angle.cos() * 0.3, 1.5, angle.sin() * 0.3));
            self.items.spawn_for(stack, body, 1.0, Some(owner), lifetime);
        }
        let words = Words::key("death.left")
            .with("x", Words::text(cell.x))
            .with("y", Words::text(cell.y))
            .with("z", Words::text(cell.z))
            .with("minutes", Words::text((seconds / 60.0).round() as u32));
        self.players[&id].connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
    }
}

/// Splits an inventory into what is kept (the `keep` most valuable single items, and a
/// grave-ward amulet besides when `warded`) and the stacks that are left behind.
pub(crate) fn split(inventory: &Inventory, keep: usize, warded: bool, mut value: impl FnMut(BlockId) -> u32) -> (Inventory, Vec<ItemStack>) {
    use voxmar_engine::inventory::{CURSOR, LAST, WORN};
    // Everything carried: the inventory, the pointer, the crafting grid and what is worn.
    let mut carried: Vec<ItemStack> = (0..=CURSOR + voxmar_engine::craft::GRID).chain(WORN..=LAST).filter_map(|slot| inventory.get(slot)).collect();
    carried.extend(inventory.pouched.iter().flat_map(|&(item, count)| {
        (0..count.div_ceil(64)).map(move |i| ItemStack::new(item, (count - i * 64).min(64) as u8))
    }));
    // The amulet is kept besides the others.
    let mut kept = Inventory::default();
    let mut budget = keep + usize::from(warded);
    while budget > 0 {
        // Of tools worth the same, the least worn.
        let Some((index, _)) = carried.iter().enumerate().filter(|(_, s)| s.count > 0).max_by_key(|(_, s)| (value(s.item), std::cmp::Reverse(s.wear))) else { break };
        // A worn tool is kept as worn as it was.
        let one = ItemStack { count: 1, ..carried[index] };
        carried[index].count -= 1;
        kept.add_stack(one);
        budget -= 1;
    }
    (kept, carried.into_iter().filter(|s| s.count > 0).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_valuable_three_are_kept_one_at_a_time() {
        let mut inventory = Inventory::default();
        inventory.add(BlockId(1), 10); // value 1
        inventory.add(BlockId(2), 2); // value 50
        inventory.add(BlockId(3), 1); // value 20
        let value = |item: BlockId| [0, 1, 50, 20][item.0 as usize];
        let (kept, left) = split(&inventory, 3, false, value);
        let count = |inv: &Inventory, id: u16| inv.slots.iter().flatten().filter(|s| s.item == BlockId(id)).map(|s| s.count).sum::<u8>();
        assert_eq!((count(&kept, 2), count(&kept, 3), count(&kept, 1)), (2, 1, 0));
        assert_eq!(left, [ItemStack::new(BlockId(1), 10)]);
    }

    #[test]
    fn worth_in_gold_decides_and_a_worn_piece_or_a_pouch_counts_unit_by_unit() {
        let content = crate::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("the game's content loads");
        let id = |name: &str| content.blocks.id(&format!("base:{name}")).unwrap_or_else(|| panic!("{name}"));
        let mut known = HashMap::new();
        let mut value = |item| voxmar_engine::value::worth_in(&content, item, &mut known);
        let count = |inv: &Inventory, item| inv.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum::<u32>() + inv.pouched.iter().filter(|p| p.0 == item).map(|p| p.1 as u32).sum::<u32>();
        // Six kinds carried; the chainmail shirt worn is worth most, then the pickaxe and
        // the sword, and five ingots or a loaf are each worth less than any of them.
        let mut inventory = Inventory::default();
        for (name, n) in [("iron_ingot", 5), ("bread", 3), ("iron_pickaxe", 1), ("iron_sword", 1), ("dirt", 40)] {
            inventory.add(id(name), n);
        }
        inventory.worn[1] = Some(ItemStack::new(id("chainmail_shirt"), 1));
        let (kept, left) = split(&inventory, 3, false, &mut value);
        for name in ["chainmail_shirt", "iron_pickaxe", "iron_sword"] {
            assert_eq!(count(&kept, id(name)), 1, "{name} is kept: {kept:?}");
        }
        assert_eq!(left.iter().map(|s| s.count as u32).sum::<u32>(), 5 + 3 + 40);
        // A pouch is counted a unit at a time: three of what is in it are kept, not all.
        let mut pouch = Inventory::default();
        pouch.pouched.push((id("iron_ingot"), 200));
        pouch.add(id("dirt"), 1);
        let (kept, left) = split(&pouch, 3, false, &mut value);
        assert_eq!(count(&kept, id("iron_ingot")), 3);
        assert_eq!(left.iter().filter(|s| s.item == id("iron_ingot")).map(|s| s.count as u32).sum::<u32>(), 197);
    }
}
