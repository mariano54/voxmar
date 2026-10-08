//! Picking the lock of a treasure chest (`treasure.ron`): a lockpick of the age before the
//! chest's, or later, opens it and is used up; one two ages short opens it only on a lucky
//! try and breaks either way. Opened, the chest becomes a plain one holding what its table
//! rolled, by the world's seed and where it stands, so the same chest in the same world
//! always held the same.

use voxmar_engine::dropped;
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_engine::treasure::{Chest, Treasure, Try};
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;
use crate::items::random3;

impl Server {
    /// `agent` tries the lock of the chest at `pos` with what they hold.
    pub(crate) fn pick_lock(&mut self, agent: PlayerId, pos: IVec3) {
        let treasure = self.content.treasure.clone();
        let Some(block) = self.block(pos) else { return };
        let Some(chest) = treasure.chest(block).cloned() else { return };
        let lucky = random3(self.tick, pos, 0x1ac7)[0] < treasure.lucky;
        let Some(player) = self.players.get_mut(&agent) else { return };
        let selected = player.selected;
        let pick = player.inventory.get(selected).and_then(|s| treasure.pick_age(s.item));
        let tell = |player: &crate::player::Player, words: Words| player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        let Some(pick) = pick else {
            let words = match treasure.opening_pick(chest.age) {
                Some(pick) => Words::key("treasure.locked").with("pick", Words::Block(self.content.blocks.def(pick).name.clone())),
                None => Words::key("treasure.no_pick"),
            };
            tell(player, words);
            return;
        };
        let opens = match Treasure::try_pick(pick, chest.age) {
            Try::TooCrude => {
                tell(player, Words::key("treasure.too_crude"));
                return;
            }
            Try::Opens => true,
            Try::Lucky => lucky,
        };
        player.inventory.take(selected, 1);
        player.changed(traits::INVENTORY);
        if !opens {
            tell(player, Words::key("treasure.pick_broke"));
            return;
        }
        let key = if Treasure::try_pick(pick, chest.age) == Try::Lucky { "treasure.lucky" } else { "treasure.opened" };
        tell(player, Words::key(key));
        tracing::debug!("player {agent} opens the {} at {pos}", self.content.blocks.def(block).name);
        self.open_chest(pos, &chest);
    }

    /// Turns the locked chest at `pos` into the block it opens to, filled from its table.
    fn open_chest(&mut self, pos: IVec3, chest: &Chest) {
        let treasure = self.content.treasure.clone();
        let machines = self.content.machines.clone();
        self.set_block(pos, chest.opens_to);
        let seed = self.meta.seed;
        let mut draw = 0u32;
        let found = treasure.roll(chest, || {
            draw += 1;
            random3(seed, pos, 0x7e45_0000 ^ draw)[0]
        });
        for (i, (item, count)) in found.into_iter().enumerate() {
            // What the chest has no room for (a mod's table) lies beside it.
            if let Some(left) = self.factory.put(pos, ItemStack { item, count }, &machines) {
                let random = random3(self.tick, pos, 0x7e46 ^ i as u32);
                self.items.spawn(left, dropped::popped(pos, random), dropped::PICKUP_DELAY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chest_finds_follow_the_worlds_saved_seed_even_when_the_launch_seed_differs() {
        let mut server = crate::test_server();
        // Saved metadata wins over the launch config, as it does when opening a world.
        server.meta.seed = 731;
        let pos = server.spawn.floor().as_ivec3();
        let block = server.content.blocks.expect("base:steam_treasure_chest");
        let treasure = server.content.treasure.clone();
        let chest = treasure.chest(block).unwrap().clone();
        let roll = |seed| {
            let mut draw = 0u32;
            treasure.roll(&chest, || { draw += 1; random3(seed, pos, 0x7e45_0000 ^ draw)[0] })
        };
        let expected = roll(server.meta.seed);
        // Positive evidence the two paths differ: this test would catch config.seed.
        assert_ne!(expected, roll(0x5eed));
        for launch in [0x5eed, 182] {
            server.config.seed = launch;
            server.set_block(pos, block);
            server.open_chest(pos, &chest);
            let found: Vec<_> = server.factory.get(pos).unwrap().1.bin.iter().flatten().map(|s| (s.item, s.count)).collect();
            assert_eq!(found, expected, "launch seed {launch} changed a saved world's chest");
        }
    }
}
