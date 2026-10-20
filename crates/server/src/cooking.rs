//! Cooking at stations (`engine::cooking`): a player holding something a station cooks
//! uses it on the station (`base:cook`) and gets one cooked, or burnt, with Cooking
//! experience for what does not burn. Successful output is counted by station as well
//! as item, so early food quests cannot be completed by smelting or charcoal.

use voxmar_engine::glam::IVec3;
use voxmar_engine::traits;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;
use crate::save::Draw;

impl Server {
    /// Cooks one of what `agent` holds at the station at `block`.
    pub(crate) fn cook(&mut self, agent: PlayerId, block: IVec3) {
        let Some(station) = self.block(block) else { return };
        // A crucible smelts only in its standing, fuelled kiln. A failed attempt only
        // explains what is missing; it must not light the kiln or consume the ingredient.
        if !self.station_stands(block) {
            if let Some(player) = self.players.get_mut(&agent) {
                player.changed(traits::INVENTORY);
            }
            self.station_hint(agent, block);
            return;
        }
        if !self.station_lit(block) {
            self.station_hint(agent, block);
            return;
        }
        let tags = self.content.blocks.def(station).tags.clone();
        let cooking = self.content.cooking.clone();
        let skills = self.content.skills.clone();
        let Some(player) = self.players.get_mut(&agent) else { return };
        let selected = player.selected;
        let Some(item) = player.held() else { return };
        let Some(recipe) = cooking.for_item(item, &tags) else { return };
        // A tool it asks for is carried (`CookingTool` refused the click without it): not
        // taken, but worn a use once the go is done.
        let blocks = &self.content.blocks;
        let tool = recipe.def.tool.as_deref().map(|tag| player.inventory.slots.iter().position(|s| s.is_some_and(|s| blocks.def(s.item).has_tag(tag))).ok_or(tag));
        if let Some(Err(tag)) = tool {
            let named = blocks.tagged(tag).next().map(|item| Words::Block(blocks.def(item).name.clone())).unwrap_or_else(|| Words::Text(tag.to_owned()));
            self.refuse(agent, Words::key("cooking.needs_tool").with("tool", named));
            return;
        }
        let level = recipe.skill.map_or(1, |skill| skills.level(player.skills.xp(skill)));
        // Everything one go takes, from the held stack first, or nothing is cooked: a
        // crucible's bronze wants three copper ingots and a tin ore (or a grain of tin).
        let wants = recipe.wants(item, |item| player.inventory.count(item));
        let mut after = player.inventory.clone();
        match wants {
            Ok(wants) if after.take_all(&wants, selected) => {}
            Ok(wants) | Err(wants) => {
                let names = wants.iter().map(|&(item, n)| Words::key("multiblock.fuel_stack").with("count", n.to_string()).with("item", Words::Block(self.content.blocks.def(item).name.clone()))).collect();
                player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("cooking.needs").with("items", Words::List(names)) });
                return;
            }
        }
        // Room for what comes out, once that is taken (a cracked clay is clay, back where it
        // was), or nothing is cooked.
        let room = [Some((recipe.output, recipe.def.count)), recipe.burnt.map(|b| (b, 1))].into_iter().flatten().all(|(made, n)| after.room_for(made, n) == n);
        if !room {
            return;
        }
        player.inventory = after;
        // By the count of goes they have cooked, not the tick it comes out on.
        let [roll, _] = player.draw(Draw::Cook);
        let burnt = roll < recipe.burn_chance(level);
        let result = if burnt { recipe.burnt.map(|b| (b, 1)) } else { Some((recipe.output, recipe.def.count)) };
        if let Some((made, n)) = result {
            player.inventory.add(made, n);
        }
        player.changed(traits::INVENTORY);
        if let Some(Ok(slot)) = tool {
            self.wear_slot(agent, slot, 1);
        }
        if let (false, Some(skill)) = (burnt, recipe.skill) {
            let xp = recipe.def.xp;
            self.grant_xp(agent, skill, xp);
        }
        if !burnt {
            let name = self.content.blocks.def(recipe.output).name.clone();
            let made = recipe.def.count as u32;
            self.tally(agent, "cooked", &name, made);
            self.tally(agent, &format!("cooked_at:{}", recipe.def.station), &name, made);
        }
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::glam::IVec3;
    use voxmar_engine::inventory::ItemStack;
    use voxmar_engine::lang::Words;
    use voxmar_protocol::ServerMessage;

    /// Ore clicked on an anvil with no hammer about is refused with words naming the
    /// hammer (refusals.ron's `cooking.needs_tool`), and nothing is taken; with one in the
    /// pack the ore is crushed, and the hammer, not taken, wears a use.
    #[test]
    fn an_anvil_refuses_ore_without_a_hammer_naming_it_and_wears_one_a_go() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.allow_flight, player.draw_key) = (true, false, crate::player::player_draw_key(0x0d1a, "ada"));
        let blocks = server.content.blocks.clone();
        let (ore, crushed, hammer, anvil) = (blocks.expect("base:copper_ore"), blocks.expect("base:crushed_copper"), blocks.expect("base:stone_hammer"), blocks.expect("base:stone_anvil"));
        player.inventory.slots[0] = Some(ItemStack::new(ore, 2));
        server.players.insert(1, player);
        let at = server.spawn.floor().as_ivec3() + IVec3::new(2, 0, 0);
        server.set_block(at, anvil);
        server.cook(1, at);
        let mut refused = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let ServerMessage::Refused { why } = message {
                refused.push(why);
            }
        }
        assert_eq!(refused, [Words::key("cooking.needs_tool").with("tool", Words::Block("base:stone_hammer".into()))]);
        assert_eq!((server.players[&1].inventory.count(ore), server.players[&1].inventory.count(crushed)), (2, 0), "nothing taken");
        server.players.get_mut(&1).unwrap().inventory.slots[5] = Some(ItemStack::new(hammer, 1));
        server.cook(1, at);
        let player = &server.players[&1];
        assert_eq!((player.inventory.count(ore), player.inventory.count(crushed)), (1, 1));
        assert_eq!(player.inventory.slots[5].map(|s| (s.item, s.wear)), Some((hammer, 1)), "the hammer stays, a use worn");
    }
}
