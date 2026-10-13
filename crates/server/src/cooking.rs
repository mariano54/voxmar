//! Cooking at stations (`engine::cooking`): a player holding something a station cooks
//! uses it on the station (`base:cook`) and gets one cooked, or burnt, with Cooking
//! experience for what does not burn. Successful output is counted by station as well
//! as item, so early food quests cannot be completed by smelting or charcoal.

use voxmar_engine::glam::IVec3;
use voxmar_engine::traits;
use voxmar_protocol::PlayerId;

use crate::Server;
use crate::items::random3;

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
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&agent) else { return };
        let selected = player.selected;
        let Some(item) = player.held() else { return };
        let Some(recipe) = cooking.for_item(item, &tags) else { return };
        let level = recipe.skill.map_or(1, |skill| skills.level(player.skills.xp(skill)));
        // Room for what comes out, or nothing is cooked. What comes out as the held item
        // itself (a cracked clay is clay) fits where the one cooked is taken from.
        let room = [Some(recipe.output), recipe.burnt].into_iter().flatten().all(|food| food == item || player.inventory.room_for(food, 1) > 0);
        if !room || player.inventory.take_one(selected).is_none() {
            return;
        }
        let [roll, ..] = random3(tick, block, agent ^ 0xc00c);
        let burnt = roll < recipe.burn_chance(level);
        let result = if burnt { recipe.burnt } else { Some(recipe.output) };
        if let Some(food) = result {
            player.inventory.add(food, 1);
        }
        player.changed(traits::INVENTORY);
        if let (false, Some(skill)) = (burnt, recipe.skill) {
            let xp = recipe.def.xp;
            self.grant_xp(agent, skill, xp);
        }
        if !burnt {
            let name = self.content.blocks.def(recipe.output).name.clone();
            self.tally(agent, "cooked", &name, 1);
            self.tally(agent, &format!("cooked_at:{}", recipe.def.station), &name, 1);
        }
    }
}
