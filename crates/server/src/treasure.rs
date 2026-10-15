//! Picking the lock of a treasure chest (`treasure.ron`): a lockpick of the age before the
//! chest's, or later, opens it and is used up; one two ages short opens it only on a lucky
//! try and breaks either way. Opened, the chest becomes a plain one holding what its table
//! rolled, by the world's seed and where it stands, so the same chest in the same world
//! always held the same.

use voxmar_engine::dropped;
use voxmar_engine::glam::{IVec3, Vec3};
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
            // By the player's count of locks tried on luck, never the tick it is tried on.
            Try::Lucky => player.draw(crate::save::Draw::Lock)[0] < treasure.lucky,
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
        // Quests count the chests opened (`Opened`).
        let name = self.content.blocks.def(block).name.clone();
        self.tally(agent, "opened", &name, 1);
        self.open_chest(pos, &chest);
        // Opened, it shows what it held, as any chest does.
        self.open_store(agent, pos);
        self.wake_beside(agent, pos, &chest);
    }

    /// Wakes what slept beside the chest at `pos` as it opened (`wakes`), each on its chance,
    /// over the chest, where the floor it was left on has room.
    fn wake_beside(&mut self, agent: PlayerId, pos: IVec3, chest: &Chest) {
        for (i, &(species, (low, high), chance)) in chest.wakes.iter().enumerate() {
            // By the opener's own count of wakes, never the tick it is opened on or what
            // else the world rolled first.
            let roll = self.draw_for(agent, crate::save::Draw::Wake);
            if roll[0] >= chance {
                continue;
            }
            let count = low + ((roll[1] * f32::from(high - low + 1)) as u8).min(high - low);
            for n in 0..count {
                let at = pos.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
                if self.spawn_creature(species, at, 0x3a4e_0000 ^ ((i as u64) << 8) ^ u64::from(n)).is_some() {
                    tracing::debug!("a {} wakes beside the chest at {pos}", self.content.creatures.get(species).def.name);
                }
            }
        }
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
            if let Some(left) = self.factory.put(pos, ItemStack::new(item, count), &machines) {
                let random = self.meta.next_pop(pos, 0x7e46 ^ i as u32);
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

    /// Opening a Nomad chest left under rock sometimes wakes a flight of cave bats over it:
    /// on the chest's chance, as many as its count, never when the lock holds.
    #[test]
    fn a_chest_may_wake_what_slept_beside_it_as_its_lock_gives() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = server.spawn;
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let blocks = server.content.blocks.clone();
        let (chest, pick) = (blocks.expect("base:nomad_treasure_chest"), blocks.expect("base:bone_lockpick"));
        let bat = server.content.creatures.find("cave_bat").unwrap();
        let (low, high) = server.content.treasure.chest(chest).unwrap().wakes[0].1;
        let (mut woke, mut quiet) = (0, 0);
        for n in 0..40 {
            server.creatures.live.clear();
            let pos = at.floor().as_ivec3() + IVec3::new(3 + n % 8, 0, 3 + n / 8);
            server.set_block(pos, chest);
            server.tick += 7;
            let player = server.players.get_mut(&1).unwrap();
            player.inventory.clear(|_| true);
            player.inventory.add(pick, 1);
            player.selected = 0;
            server.pick_lock(1, pos);
            assert_ne!(server.block(pos), Some(chest), "a bone pick opens a Nomad chest");
            let near: Vec<_> = server.creatures.live.values().filter(|c| c.species == bat).map(|c| c.body.position).collect();
            assert_eq!(near.len(), server.creatures.live.len(), "only what the chest wakes");
            assert!(near.iter().all(|p| p.distance(pos.as_vec3() + Vec3::new(0.5, 1.0, 0.5)) < 1.0), "over the chest: {near:?}");
            match near.len() as u8 {
                0 => quiet += 1,
                n => {
                    assert!((low..=high).contains(&n), "{n} bats, not {low}..={high}");
                    woke += 1;
                }
            }
        }
        assert!(woke > 4 && quiet > 4, "on its chance: {woke} woke, {quiet} quiet");
        // A pick too crude to try opens nothing and wakes nothing.
        server.creatures.live.clear();
        let iron = at.floor().as_ivec3() + IVec3::new(-3, 0, -3);
        server.set_block(iron, blocks.expect("base:iron_treasure_chest"));
        let player = server.players.get_mut(&1).unwrap();
        player.inventory.clear(|_| true);
        player.inventory.add(pick, 1);
        player.selected = 0;
        server.pick_lock(1, iron);
        assert!(server.creatures.live.is_empty());
    }

    /// A lucky pick holds or gives by the player's count of locks tried on luck, and what
    /// wakes by the opener's count of wakes, not the tick the lock is tried on.
    #[test]
    fn a_lucky_lock_and_what_wakes_draw_by_count_not_the_tick() {
        let tried = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let (client, connection) = voxmar_protocol::local_pair();
            std::mem::forget(client);
            let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
            (player.joined, player.draw_key) = (true, crate::player::player_draw_key(0x0d1a, "ada"));
            server.players.insert(1, player);
            let blocks = server.content.blocks.clone();
            let pick = blocks.expect("base:bone_lockpick");
            server.tick = tick;
            let at = server.spawn.floor().as_ivec3();
            let mut opened = Vec::new();
            for (n, chest) in ["base:bronze_treasure_chest", "base:nomad_treasure_chest"].into_iter().flat_map(|c| [c; 24]).enumerate() {
                server.creatures.live.clear();
                let pos = at + IVec3::new(3 + n as i32 % 8, 0, 3 + n as i32 / 8);
                server.set_block(pos, blocks.expect(chest));
                let player = server.players.get_mut(&1).unwrap();
                player.inventory.clear(|_| true);
                player.inventory.add(pick, 1);
                player.selected = 0;
                server.pick_lock(1, pos);
                let open = server.block(pos) != Some(blocks.expect(chest));
                let wakes = if open { server.content.treasure.chest(blocks.expect(chest)).map_or(0, |c| c.wakes.len()) } else { 0 };
                opened.push((open, server.creatures.live.len(), wakes));
            }
            (opened, server.players[&1].draws.locks, server.players[&1].draws.wakes)
        };
        let first = tried(1_000);
        let (lucky, woke) = first.0.split_at(24);
        assert_eq!(first.1, 24, "each lock tried on luck counted on");
        let wakes: usize = first.0.iter().map(|t| t.2).sum();
        assert!(wakes > 0, "no opened chest had anything to wake: {:?}", first.0);
        assert_eq!(first.2, wakes as u64, "each of an opened chest's wakes counted on");
        assert!(lucky.iter().any(|t| t.0) && lucky.iter().any(|t| !t.0), "24 lucky tries all went alike: {lucky:?}");
        assert!(woke.iter().any(|t| t.1 > 0) && woke.iter().any(|t| t.1 == 0), "24 opened chests all woke alike: {woke:?}");
        assert_eq!(tried(1_377), first, "the same locks tried later went otherwise");
    }
}
