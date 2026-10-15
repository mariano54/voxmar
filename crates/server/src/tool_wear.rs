//! Tools and weapons wearing out in the hand (`voxmar_engine::tool::Wear`, each package's
//! `tools.ron`): a block broken, soil tilled or a blow that lands takes uses from what the
//! player holds, the skill its kind trains saves one now and then, and worn to nothing it
//! breaks, leaving its head in the hand. The player hears it warn once it is nearly worn
//! out, and hears and sees it break (`ServerMessage::ToolWore`). Creative play wears nothing.

use voxmar_engine::inventory::ItemStack;
use voxmar_engine::tool::{Worn, wear_down};
use voxmar_protocol::{PlayerId, ServerMessage};

use crate::Server;
use crate::save::Draw;

impl Server {
    /// Wears what `agent` holds by `uses` (it says how many a use takes).
    pub(crate) fn wear_held(&mut self, agent: PlayerId, uses: u16) {
        let (tools, blocks, skills) = (self.content.tools.clone(), self.content.blocks.clone(), self.content.skills.clone());
        let Some(player) = self.players.get_mut(&agent) else { return };
        let slot = player.selected;
        let Some(mut stack) = player.inventory.get(slot) else { return };
        let total = tools.uses(stack.item);
        if total == 0 || uses == 0 || player.mode() == crate::save::GameMode::Creative {
            return;
        }
        // By the player's own count of uses that might have worn, never the tick.
        if let Some(skill) = tools.wear_skill(stack.item, blocks.def(stack.item).has_tag("weapon")) {
            let level = skills.level(player.skills.xp(skill));
            let [draw, _] = player.draw(Draw::Wear);
            if tools.saves_wear(level, draw) {
                return;
            }
        }
        let item = stack.item;
        let worn = wear_down(&mut stack, uses, total, tools.wear().warn);
        player.inventory.slots[slot] = match worn {
            Worn::Broke => tools.head(item).map(|head| ItemStack::new(head, 1)),
            _ => Some(stack),
        };
        player.changed(voxmar_engine::traits::INVENTORY);
        if worn != Worn::Fine {
            player.connection.send(ServerMessage::ToolWore { item, broke: worn == Worn::Broke });
        }
    }

    /// Uses breaking or tilling the block at `pos` takes from what `agent` holds.
    pub(crate) fn wear_on_block(&mut self, agent: PlayerId, pos: voxmar_engine::glam::IVec3, tilling: bool) {
        let tools = &self.content.tools;
        let uses = if tilling {
            let held = self.players.get(&agent).and_then(|p| p.held());
            if held.is_some_and(|item| tools.uses(item) > 0) { tools.wear().till } else { 0 }
        } else {
            let Some(block) = self.block(pos) else { return };
            let held = self.players.get(&agent).and_then(|p| p.held());
            tools.break_wear(held, block, self.content.blocks.info(block).hardness)
        };
        self.wear_held(agent, uses);
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::glam::IVec3;
    use voxmar_engine::inventory::ItemStack;
    use voxmar_protocol::{ClientConnection, ServerMessage};

    use crate::Server;

    /// A server with one joined survival player holding `item` (worn by `wear`) in slot 0,
    /// and the client end of their connection.
    fn holding(item: &str, wear: u16) -> (Server, ClientConnection) {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.allow_flight, player.draw_key) = (true, false, crate::player::player_draw_key(0x0d1a, "ada"));
        let item = server.content.blocks.expect(item);
        player.inventory.slots[0] = Some(ItemStack { wear, ..ItemStack::new(item, 1) });
        server.players.insert(1, player);
        (server, client)
    }

    /// What the player was told of their tool: (warned, broke).
    fn told(client: &ClientConnection) -> (u32, u32) {
        let mut told = (0, 0);
        while let Ok(Some(message)) = client.try_recv() {
            if let ServerMessage::ToolWore { broke, .. } = message {
                if broke { told.1 += 1 } else { told.0 += 1 }
            }
        }
        told
    }

    fn put(server: &mut Server, block: &str) -> IVec3 {
        let pos = server.spawn.floor().as_ivec3() + IVec3::new(2, 0, 0);
        let block = server.content.blocks.expect(block);
        server.set_block(pos, block);
        pos
    }

    /// A pickaxe wears a use for each stone it breaks, two for a block it is not made for
    /// (planks), none for one that breaks at a touch; it warns once at a tenth left, and worn
    /// out it breaks to its head and the player hears it.
    #[test]
    fn a_tool_wears_by_use_and_breaks_to_its_head() {
        let (mut server, client) = holding("base:wooden_pickaxe", 0);
        let uses = server.content.tools.uses(server.content.blocks.expect("base:wooden_pickaxe"));
        assert!(uses >= 10, "a wooden pickaxe lasts {uses} uses");
        let wear = |server: &Server| server.players[&1].inventory.slots[0].map(|s| s.wear);
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false);
        assert_eq!(wear(&server), Some(1), "a stone");
        let planks = put(&mut server, "base:oak_planks");
        server.wear_on_block(1, planks, false);
        assert_eq!(wear(&server), Some(3), "planks, which a pickaxe is not made for");
        let grass = put(&mut server, "base:short_grass");
        assert_eq!(server.content.blocks.info(server.content.blocks.expect("base:short_grass")).hardness, 0.0);
        server.wear_on_block(1, grass, false);
        assert_eq!(wear(&server), Some(3), "grass breaks at a touch");
        assert_eq!(told(&client), (0, 0));
        // Down to a tenth left, it warns once.
        let warn_at = (uses as f32 * server.content.tools.wear().warn).ceil() as u16;
        server.players.get_mut(&1).unwrap().inventory.slots[0].as_mut().unwrap().wear = uses - warn_at - 1;
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false);
        server.wear_on_block(1, stone, false);
        assert_eq!(told(&client), (1, 0), "warned once, crossing a tenth left");
        server.players.get_mut(&1).unwrap().inventory.slots[0].as_mut().unwrap().wear = uses - 1;
        server.wear_on_block(1, stone, false);
        let stick = server.content.blocks.expect("base:stick");
        assert_eq!(server.players[&1].inventory.slots[0], Some(ItemStack::new(stick, 1)), "broken, its head is left in the hand");
        assert_eq!(told(&client), (0, 1));
    }

    /// Mining saves a pickaxe's wear by the level: never at level 1, about one use in two
    /// at 99, drawn by the player's count of uses, never the tick.
    #[test]
    fn skill_saves_wear_by_the_count_of_uses_not_the_tick() {
        let worn = |tick: u64, level: u8| {
            let (mut server, _client) = holding("base:iron_pickaxe", 0);
            let mining = server.content.skills.id("core:mining").unwrap();
            let xp = server.content.skills.xp_for(level);
            server.players.get_mut(&1).unwrap().skills.grant(mining, xp);
            server.tick = tick;
            let stone = put(&mut server, "base:stone");
            for _ in 0..200 {
                server.wear_on_block(1, stone, false);
            }
            (server.players[&1].inventory.slots[0].unwrap().wear, server.players[&1].draws.wears)
        };
        assert_eq!(worn(1_000, 1).0, 200, "level 1 saves nothing");
        let (wear, draws) = worn(1_000, 99);
        assert_eq!(draws, 200, "each use counted on");
        assert!((70..130).contains(&wear), "at 99 about half the uses wear: {wear} of 200");
        assert_eq!(worn(1_377, 99), (wear, draws), "the same uses later wore otherwise");
    }

    /// A hoe wears as it tills; in creative nothing wears; a block a player holds that never
    /// wears (a stick) and a bare hand are untouched.
    #[test]
    fn tilling_wears_and_creative_does_not() {
        let (mut server, _client) = holding("base:wooden_hoe", 0);
        let soil = put(&mut server, "base:dirt");
        server.wear_on_block(1, soil, true);
        assert_eq!(server.players[&1].inventory.slots[0].unwrap().wear, server.content.tools.wear().till);
        server.players.get_mut(&1).unwrap().allow_flight = true;
        server.wear_on_block(1, soil, true);
        assert_eq!(server.players[&1].inventory.slots[0].unwrap().wear, server.content.tools.wear().till, "creative");
        let (mut server, _client) = holding("base:stick", 0);
        let stone = put(&mut server, "base:stone");
        server.wear_on_block(1, stone, false);
        assert_eq!(server.players[&1].inventory.slots[0].unwrap().wear, 0, "a stick never wears");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = None;
        server.wear_on_block(1, stone, false);
        assert_eq!(server.players[&1].inventory.slots[0], None);
    }

    /// A worn tool keeps its wear when dropped, saved with the world's items and picked up
    /// again, and when it is kept through a death.
    #[test]
    fn a_dropped_tool_keeps_its_wear() {
        let (server, _client) = holding("base:stone_axe", 37);
        let axe = server.players[&1].inventory.slots[0].unwrap();
        let mut items = crate::items::Items::default();
        items.spawn(axe, voxmar_engine::dropped::Loose::new(server.spawn, voxmar_engine::glam::Vec3::ZERO), 0.0);
        let blocks = server.content.blocks.clone();
        let saved = items.saved(|item| blocks.def(item).name.clone());
        assert_eq!(saved[0].wear, 37);
        let restored = crate::items::Items::restore(saved, |name| blocks.id(name));
        let again = restored.saved(|item| blocks.def(item).name.clone());
        assert_eq!((again[0].item.as_str(), again[0].wear), ("base:stone_axe", 37), "restored as worn");
        let stack = axe;
        let mut inventory = voxmar_engine::inventory::Inventory::default();
        inventory.add(axe.item, 1);
        assert_eq!(inventory.add_stack(stack), 0);
        assert_eq!(inventory.slots[1], Some(axe), "a worn one takes a slot of its own");
    }
}
