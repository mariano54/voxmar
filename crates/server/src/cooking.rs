//! Cooking at stations (`engine::cooking`): a player holding something a station cooks
//! uses it on the station (`base:cook`) and gets one cooked, or burnt, with Cooking
//! experience for what does not burn. Successful output is counted by station as well
//! as item, so early food quests cannot be completed by smelting or charcoal.

use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::ItemStack;
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
        // taken.
        let blocks = &self.content.blocks;
        if let Some(tag) = recipe.def.tool.as_deref()
            && !player.inventory.slots.iter().flatten().any(|s| blocks.def(s.item).has_tag(tag))
        {
            let named = blocks.tagged(tag).next().map(|item| Words::Block(blocks.def(item).name.clone())).unwrap_or_else(|| Words::Text(tag.to_owned()));
            self.refuse(agent, Words::key("cooking.needs_tool").with("tool", named));
            return;
        }
        let level = recipe.skill.map_or(1, |skill| skills.level(player.skills.xp(skill)));
        // Everything one go takes, from the held stack first, or nothing is cooked: a
        // crucible's bronze wants three copper ingots and a tin ore.
        let wants = recipe.ingredients(item);
        let mut after = player.inventory.clone();
        if !after.take_all(&wants, selected) {
            let names = wants.iter().map(|&(item, n)| Words::key("multiblock.fuel_stack").with("count", n.to_string()).with("item", Words::Block(self.content.blocks.def(item).name.clone()))).collect();
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("cooking.needs").with("items", Words::List(names)) });
            return;
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
            // The last one held comes back cooked into the emptied hand (a lit firebrand
            // is held up at once), unless it tops up a stack already carried; the rest go
            // where they fit.
            let most = voxmar_engine::inventory::max_stack(made);
            let tops_up = player.inventory.slots.iter().flatten().any(|s| s.item == made && s.wear == 0 && s.count < most);
            if player.inventory.get(selected).is_none() && !tops_up {
                let here = n.min(most);
                player.inventory.slots[selected] = Some(ItemStack::new(made, here));
                player.inventory.add(made, n - here);
            } else {
                player.inventory.add(made, n);
            }
        }
        player.changed(traits::INVENTORY);
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
