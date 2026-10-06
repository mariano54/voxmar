//! Fishing (`base:fish`, engine `fishing`): a line cast into water comes up, once its time
//! is up, with something that bites there for the angler's level and rod, or nothing.

use voxmar_engine::dropped;
use voxmar_engine::fishing::Spot;
use voxmar_engine::glam::{IVec3, Vec2};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;
use crate::items::random3;

/// Rain (or snow) at least this heavy counts as rain for what bites.
const RAINING: f32 = 0.2;

impl Server {
    /// What the line `agent` cast into the water at `at` brings up.
    pub(crate) fn fish(&mut self, agent: PlayerId, at: IVec3) {
        let fishing = self.content.fishing.clone();
        let Some(player) = self.players.get(&agent) else { return };
        let level = fishing.skill.map_or(1, |skill| self.check_level(agent, skill));
        let rod = self.content.tools.tool(player.held());
        let biome = self.overworld.biome_at(at.x, at.z);
        let tags = biome.tags.clone();
        let (tick, day) = (self.tick, self.day);
        let raining = self.weather.local(&mut self.temperature, Vec2::new(at.x as f32, at.z as f32), tick, day).weather.precipitation >= RAINING;
        let spot = Spot {
            salt: tags.iter().any(|t| t == "ocean"),
            biome_tags: &tags,
            day: voxmar_engine::creature::daylight(self.day as u64) > 0.3,
            raining,
            level,
            rod_tier: rod.tier,
            rod_yield: rod.yield_,
            luck: player.modifiers.fishing,
        };
        let [land, pick, kick] = random3(self.tick, at, agent ^ 0xf15);
        let Some(catch) = fishing.cast(&spot, land, pick).cloned() else {
            if let Some(p) = self.players.get(&agent) {
                p.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("fishing.nothing") });
            }
            return;
        };
        let player = self.players.get_mut(&agent).expect("checked above");
        let left = player.inventory.add(catch.item, 1);
        player.changed(traits::INVENTORY);
        if left > 0 {
            let body = dropped::popped(at + IVec3::Y, [kick, land, pick]);
            self.items.spawn(ItemStack { item: catch.item, count: left }, body, dropped::PICKUP_DELAY);
        }
        if let Some(skill) = fishing.skill {
            self.grant_xp(agent, skill, catch.def.xp);
        }
        self.discover(agent, voxmar_engine::journal::Page::Fish, &catch.def.item, 1);
    }
}
