//! Cooking at stations (`engine::cooking`): a player holding something a station cooks
//! uses it on the station (`base:cook`) and gets one cooked, or burnt, with Cooking
//! experience for what does not burn.

use voxmar_engine::glam::IVec3;
use voxmar_engine::traits;
use voxmar_protocol::PlayerId;

use crate::Server;
use crate::items::random3;

impl Server {
    /// Cooks one of what `agent` holds at the station at `block`.
    pub(crate) fn cook(&mut self, agent: PlayerId, block: IVec3) {
        let Some(station) = self.block(block) else { return };
        let tags = self.content.blocks.def(station).tags.clone();
        let cooking = self.content.cooking.clone();
        let skills = self.content.skills.clone();
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&agent) else { return };
        let selected = player.selected;
        let Some(item) = player.held() else { return };
        let Some(recipe) = cooking.for_item(item, &tags) else { return };
        let level = cooking.skill.map_or(1, |skill| skills.level(player.skills.xp(skill)));
        // Room for what comes out, or nothing is cooked.
        let room = [Some(recipe.output), recipe.burnt].into_iter().flatten().all(|food| player.inventory.room_for(food, 1) > 0);
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
        if let (false, Some(skill)) = (burnt, cooking.skill) {
            let xp = recipe.def.xp;
            self.grant_xp(agent, skill, xp);
        }
    }
}
