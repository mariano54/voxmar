//! What dying costs (Mariano, 2026-10-06): a player keeps their three most valuable items,
//! and everything else they carried stays where they fell, theirs alone to pick up, for
//! 30 minutes; then it is gone. A grave-ward amulet keeps a fourth item and doubles the
//! time. They wake at home (`Player::home`, set by a bed or hearth), or at world spawn.

use voxmar_engine::block::{BlockId, Rarity};
use voxmar_engine::dropped::{self, LIFETIME};
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::{Inventory, ItemStack};
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::items::{owner_of, random3};

/// Items a player keeps through death, and with a grave-ward amulet.
pub const KEPT: usize = 3;
pub const KEPT_WARDED: usize = 4;

impl Server {
    /// Leaves what each player who has just died carried where they fell.
    pub(crate) fn bury_the_dead(&mut self) {
        let dead: Vec<PlayerId> = self.players.values().filter(|p| p.joined && !p.health.alive() && !p.buried).map(|p| p.id).collect();
        for id in dead {
            self.bury(id);
        }
    }

    fn bury(&mut self, id: PlayerId) {
        let blocks = self.content.blocks.clone();
        let tools = self.content.tools.clone();
        let tick = self.tick;
        let player = self.players.get_mut(&id).expect("listed");
        player.buried = true;
        let warded = player.inventory.slots.iter().flatten().any(|s| blocks.def(s.item).has_tag("grave_ward"));
        let value = |item: BlockId| {
            let def = blocks.def(item);
            if def.has_tag("grave_ward") {
                return u32::MAX;
            }
            let rarity = match def.rarity {
                Rarity::Common => 1,
                Rarity::Uncommon => 4,
                Rarity::Rare => 16,
                Rarity::VeryRare => 64,
                Rarity::Legendary => 256,
            };
            rarity * (1 + tools.tool(Some(item)).tier as u32) * if voxmar_engine::inventory::max_stack(item) == 1 { 2 } else { 1 }
        };
        let (kept, left) = split(&player.inventory, if warded { KEPT_WARDED } else { KEPT }, warded, value);
        player.inventory = kept;
        player.changed(traits::INVENTORY);
        let at = player.position;
        let owner = owner_of(&player.name);
        let minutes = if warded { 2.0 } else { 1.0 };
        let lifetime = (LIFETIME * minutes * TICKS_PER_SECOND as f32) as u32;
        let cell = at.floor().as_ivec3();
        for (i, stack) in left.into_iter().enumerate() {
            // A pile at the feet, barely spread, so nothing rolls off a ledge or into water
            // out of reach of the one who comes back for it.
            let [a, b, c] = random3(tick, cell, i as u32 ^ 0x9a7e);
            let nudge = Vec3::new(a - 0.5, 0.0, b - 0.5) * 0.4;
            let angle = c * std::f32::consts::TAU;
            let body = dropped::Loose::new(at + nudge + Vec3::Y * 0.25, Vec3::new(angle.cos() * 0.3, 1.5, angle.sin() * 0.3));
            self.items.spawn_for(stack, body, 1.0, Some(owner), lifetime);
        }
        let words = Words::key("death.left")
            .with("x", Words::text(cell.x))
            .with("y", Words::text(cell.y))
            .with("z", Words::text(cell.z))
            .with("minutes", Words::text((30.0 * minutes) as u32));
        self.players[&id].connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
    }
}

/// Splits an inventory into what is kept (the `keep` most valuable single items, and a
/// grave-ward amulet besides when `warded`) and the stacks that are left behind.
fn split(inventory: &Inventory, keep: usize, warded: bool, value: impl Fn(BlockId) -> u32) -> (Inventory, Vec<ItemStack>) {
    use voxmar_engine::inventory::{CURSOR, LAST, WORN};
    // Everything carried: the inventory, the pointer, the crafting grid and what is worn.
    let mut carried: Vec<ItemStack> = (0..=CURSOR + voxmar_engine::craft::GRID).chain(WORN..=LAST).filter_map(|slot| inventory.get(slot)).collect();
    carried.extend(inventory.pouched.iter().flat_map(|&(item, count)| {
        (0..count.div_ceil(64)).map(move |i| ItemStack { item, count: (count - i * 64).min(64) as u8 })
    }));
    // The amulet is kept besides the others.
    let mut kept = Inventory::default();
    let mut budget = keep + usize::from(warded);
    while budget > 0 {
        let Some((index, _)) = carried.iter().enumerate().filter(|(_, s)| s.count > 0).max_by_key(|(_, s)| value(s.item)) else { break };
        let item = carried[index].item;
        carried[index].count -= 1;
        kept.add(item, 1);
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
        assert_eq!(left, [ItemStack { item: BlockId(1), count: 10 }]);
    }
}
