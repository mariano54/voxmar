//! Quests on the server (see `voxmar_engine::quest`): each second every player's quests are
//! checked against what they have done, finished ones wait to be claimed, and the client
//! hears of any change. The rewards (items and experience) are paid when the player claims
//! them in the quest screen (`ClientMessage::ClaimQuest`); a quest that pays nothing is
//! claimed as it finishes.

use voxmar_engine::lang::Words;
use voxmar_engine::quest::{Progress, QuestState, Reward};
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

    /// Once a second: finishes quests whose tasks hold, marks them done, and sends each
    /// player their quests' states when they changed.
    pub(crate) fn check_quests(&mut self) {
        if self.tick % TICKS_PER_SECOND as u64 != 0 || self.content.quests.iter().next().is_none() {
            return;
        }
        let ids: Vec<PlayerId> = self.players.values().filter(|p| p.joined).map(|p| p.id).collect();
        for id in ids {
            self.update_quests(id);
        }
    }

    /// Finishes `id`'s quests whose tasks hold and tells them their quests' states if they
    /// changed.
    fn update_quests(&mut self, id: PlayerId) {
        let (finished, states) = self.quest_check(id);
        for name in &finished {
            self.finish_quest(id, name);
        }
        self.send_quests(id, states);
    }

    /// The quests `id` has newly finished, and every quest's state as it will stand once
    /// they are marked done.
    fn quest_check(&self, id: PlayerId) -> (Vec<String>, Vec<QuestState>) {
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
        let mut claimed = player.quests_claimed.clone();
        claimed.extend(finished.iter().filter(|name| self.content.quests.get(name).is_some_and(|q| !q.pays())).cloned());
        (finished, self.content.quests.states(&done, &claimed, &progress))
    }

    /// Tells `id` their quests' states if they changed since last told.
    fn send_quests(&mut self, id: PlayerId, states: Vec<QuestState>) {
        let player = self.players.get_mut(&id).expect("listed");
        if player.quests_sent != states {
            player.connection.send(ServerMessage::Quests { states: states.clone() });
            player.quests_sent = states;
        }
    }

    /// Marks a quest done for `agent`. What it pays waits to be claimed (`claim_quest`), and
    /// the chat line says so; a quest that pays nothing is claimed at once.
    fn finish_quest(&mut self, agent: PlayerId, name: &str) {
        let Some(quest) = self.content.quests.get(name).cloned() else { return };
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.quests_done.push(name.to_owned());
        if !quest.pays() {
            player.quests_claimed.push(name.to_owned());
        }
        let title = Words::Own(voxmar_engine::lang::def_key("quest", &quest.name), quest.title.clone());
        let key = if quest.pays() { "quest.ready" } else { "quest.done" };
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key(key).with("quest", title) });
    }

    /// A player claims a finished quest's rewards from the quest screen: its items go into
    /// the pack and its experience to the skills. If the pack cannot hold every item, nothing
    /// is given, the player is told, and the quest stays claimable. A quest that is not
    /// finished, was claimed already, or does not exist is ignored.
    pub(crate) fn claim_quest(&mut self, agent: PlayerId, name: &str) {
        let Some(quest) = self.content.quests.get(name).cloned() else { return };
        let Some(player) = self.players.get_mut(&agent) else { return };
        if !player.quests_done.iter().any(|d| d == name) || player.quests_claimed.iter().any(|c| c == name) {
            return;
        }
        let items: Vec<_> = quest.rewards.iter().filter_map(|r| if let Reward::Item(item, count) = r { Some((self.content.blocks.id(item)?, *count)) } else { None }).collect();
        let mut probe = player.inventory.clone();
        if items.iter().any(|&(item, count)| probe.add(item, count) > 0) {
            let title = Words::Own(voxmar_engine::lang::def_key("quest", &quest.name), quest.title.clone());
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Error, text: Words::key("quest.claim.full").with("quest", title) });
            return;
        }
        player.quests_claimed.push(name.to_owned());
        for (item, count) in items {
            player.inventory.add(item, count);
        }
        player.changed(traits::INVENTORY);
        for reward in &quest.rewards {
            if let Reward::Xp(skill, xp) = reward
                && let Some(id) = self.content.skills.id(skill)
            {
                self.grant_xp(agent, id, *xp);
            }
        }
        // The quest screen hears at once, not at the next second's check; what the reward
        // itself finishes (items carried, a level reached) is finished now too.
        self.update_quests(agent);
    }
}
