//! Quests on the server (see `voxmar_engine::quest`): each second every player's quests are
//! checked against what they have done, finished ones give their rewards, and the client
//! hears of any change.

use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::quest::{Progress, Reward};
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

impl Server {
    /// Counts one more of something a player did that quests may ask for: `crafted`,
    /// `cooked` or `tamed`, and what.
    pub(crate) fn tally(&mut self, agent: PlayerId, kind: &str, name: &str, times: u32) {
        if let Some(player) = self.players.get_mut(&agent) {
            *player.tallies.entry(format!("{kind}:{name}")).or_default() += times;
        }
    }

    /// How many animals `keeper` keeps, in the world or waiting in the save, with a condition
    /// of at least `least` (0 to 1).
    fn thriving(&self, keeper: &str, least: f32) -> u32 {
        let tamed = self.content.creatures.condition().tamed;
        let live = self.creatures.live.values().filter(|c| c.owner.as_deref() == Some(keeper) && c.condition >= least).count();
        let saved = self.creatures.unplaced.iter().filter(|s| s.owner == keeper && s.condition.unwrap_or(tamed) >= least).count();
        (live + saved) as u32
    }

    /// Once a second: finishes quests whose tasks hold, gives their rewards, and sends each
    /// player their quests' states when they changed.
    pub(crate) fn check_quests(&mut self) {
        if self.tick % TICKS_PER_SECOND as u64 != 0 || self.content.quests.iter().next().is_none() {
            return;
        }
        let ids: Vec<PlayerId> = self.players.values().filter(|p| p.joined).map(|p| p.id).collect();
        for id in ids {
            let (finished, states) = {
                let player = &self.players[&id];
                let (skills, blocks) = (&self.content.skills, &self.content.blocks);
                // Names are full ones, resolved as content loaded (`QuestRegistry::resolve`).
                let level = |name: &str| skills.level_of(&player.skills, name);
                let carried = |want: &str| {
                    let matches = |item: voxmar_engine::block::BlockId| {
                        let def = blocks.def(item);
                        match want.strip_prefix("tag:") {
                            Some(tag) => def.has_tag(tag),
                            None => def.name == want || def.name.rsplit(':').next() == Some(want),
                        }
                    };
                    let slots: u32 = player.inventory.slots.iter().flatten().filter(|s| matches(s.item)).map(|s| s.count as u32).sum();
                    slots + player.inventory.pouched.iter().filter(|(item, _)| matches(*item)).map(|&(_, n)| n as u32).sum::<u32>()
                };
                let has_tag = |name: &str, tag: &str| blocks.id(name).is_some_and(|id| blocks.def(id).has_tag(tag));
                let drops = |name: &str| blocks.id(name).map(|id| blocks.info(id).drops).filter(|d| !d.is_air()).map(|d| blocks.def(d).name.clone());
                let thriving = |percent: u8| self.thriving(&player.name, percent as f32 / 100.0);
                let progress = Progress { journal: &player.journal, tallies: &player.tallies, level: &level, carried: &carried, has_tag: &has_tag, drops: &drops, thriving: &thriving, home: player.home.is_some() };
                let finished = self.content.quests.newly_done(&player.quests_done, &progress);
                let mut done = player.quests_done.clone();
                done.extend(finished.iter().cloned());
                (finished, self.content.quests.states(&done, &progress))
            };
            for name in &finished {
                self.finish_quest(id, name);
            }
            let player = self.players.get_mut(&id).expect("listed");
            if player.quests_sent != states {
                player.connection.send(ServerMessage::Quests { states: states.clone() });
                player.quests_sent = states;
            }
        }
    }

    /// Marks a quest done for `agent` and gives its rewards.
    fn finish_quest(&mut self, agent: PlayerId, name: &str) {
        let Some(quest) = self.content.quests.get(name).cloned() else { return };
        if let Some(player) = self.players.get_mut(&agent) {
            player.quests_done.push(name.to_owned());
            let words = Words::key("quest.done").with("quest", Words::Own(voxmar_engine::lang::def_key("quest", &quest.name), quest.title.clone()));
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
        for reward in &quest.rewards {
            match reward {
                Reward::Xp(skill, xp) => {
                    if let Some(id) = self.content.skills.id(skill) {
                        self.grant_xp(agent, id, *xp);
                    }
                }
                Reward::Item(item, count) => {
                    let Some(block) = self.content.blocks.id(item) else { continue };
                    let Some(player) = self.players.get_mut(&agent) else { continue };
                    let left = player.inventory.add(block, *count);
                    player.changed(traits::INVENTORY);
                    if left > 0 {
                        let body = voxmar_engine::dropped::popped(player.position.floor().as_ivec3(), [0.5, 0.5, 0.5]);
                        self.items.spawn(ItemStack { item: block, count: left }, body, voxmar_engine::dropped::PICKUP_DELAY);
                    }
                }
            }
        }
    }
}
