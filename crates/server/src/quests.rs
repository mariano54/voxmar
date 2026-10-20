//! Quests on the server (see `voxmar_engine::quest`): each second every player's quests are
//! checked against what they have done, finished ones wait to be claimed, and the client
//! hears of any change. The rewards (items and experience) are paid when the player claims
//! them in the quest screen (`ClientMessage::ClaimQuest`); a quest that pays nothing is
//! claimed as it finishes.

use rustc_hash::FxHashMap;
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::glam::IVec3;
use voxmar_engine::lang::Words;
use voxmar_engine::quest::{Progress, QuestRegistry, QuestState, Reward};
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// Whether a quest that `done` does not hold counts `item` set down (`Goal::Placed`).
fn counts_placed(blocks: &BlockRegistry, quests: &QuestRegistry, item: &str, done: &dyn Fn(&str) -> bool) -> bool {
    blocks.id(item).is_some_and(|id| quests.places(item, &|tag| blocks.def(id).has_tag(tag), done))
}

/// The blocks set down for quests a save recorded (`WorldMeta::quest_placed`), by cell: a
/// renamed item's records go on under its new name, as tallies do, and records no quest
/// counts any more (a mod took it away) are dropped.
pub(crate) fn restore_placed(saved: &[([i32; 3], String, String)], blocks: &BlockRegistry, quests: &QuestRegistry) -> FxHashMap<IVec3, (String, String)> {
    saved
        .iter()
        .filter_map(|(cell, owner, item)| {
            let item = blocks.def(blocks.saved_id(item)?).name.clone();
            counts_placed(blocks, quests, &item, &|_| false).then(|| (IVec3::from_array(*cell), (crate::player_key(owner), item)))
        })
        .collect()
}

impl Server {
    /// Counts one more of something a player did that quests may ask for: `crafted`,
    /// `cooked` or `tamed`, and what.
    pub(crate) fn tally(&mut self, agent: PlayerId, kind: &str, name: &str, times: u32) {
        if let Some(player) = self.players.get_mut(&agent) {
            *player.tallies.entry(format!("{kind}:{name}")).or_default() += times;
        }
    }

    /// Takes `times` back off a tally (never below nothing): what was undone, such as an
    /// item taken back out of a store.
    pub(crate) fn untally(&mut self, agent: PlayerId, kind: &str, name: &str, times: u32) {
        let Some(player) = self.players.get_mut(&agent) else { return };
        let key = format!("{kind}:{name}");
        if let Some(n) = player.tallies.get_mut(&key) {
            *n = n.saturating_sub(times);
            if *n == 0 {
                player.tallies.remove(&key);
            }
        }
    }

    /// A block a player set down: recorded at its cell (`Server::quest_placed`) and counted
    /// on their tally where an unfinished quest of theirs counts it (`Placed`). A record the
    /// cell held before (a snare set over a snare) comes off its owner's count, now if they
    /// are here, else as they next join (`recount_placed`).
    pub(crate) fn placed_for_quests(&mut self, agent: PlayerId, item: BlockId, cell: IVec3) {
        let Some(player) = self.players.get(&agent) else { return };
        let def = self.content.blocks.def(item);
        let done = |quest: &str| player.quests_done.iter().any(|d| d == quest);
        if !self.content.quests.places(&def.name, &|tag| def.has_tag(tag), &done) {
            return;
        }
        let (owner, name) = (crate::player_key(&player.name), def.name.clone());
        if let Some((before, item)) = self.quest_placed.insert(cell, (owner, name.clone()))
            && let Some(id) = self.here(&before)
        {
            self.untally(id, "placed", &item, 1);
        }
        self.tally(agent, "placed", &name, 1);
    }

    /// The cell at `pos` now holds `block`: a block recorded there for a quest stands while
    /// this one still drops its item (a sprung snare still drops a snare). Gone (broken,
    /// built over, washed away), its record goes and it comes off its owner's count, now if
    /// they are here, else as they next join.
    pub(crate) fn unplaced(&mut self, pos: IVec3, block: BlockId) {
        let Some((_, item)) = self.quest_placed.get(&pos) else { return };
        let blocks = &self.content.blocks;
        let drops = |id: BlockId| Some(blocks.info(id).drops).filter(|d| !d.is_air());
        if blocks.id(item).and_then(drops).is_some_and(|wanted| drops(block) == Some(wanted)) {
            return;
        }
        let (owner, item) = self.quest_placed.remove(&pos).expect("found above");
        if let Some(id) = self.here(&owner) {
            self.untally(id, "placed", &item, 1);
        }
    }

    /// A player's `placed` tallies, counted again from the cells recorded for them: what
    /// was built over or broken while they were away comes off, and so do records no quest
    /// still open to them counts (a mod took the quest away).
    pub(crate) fn recount_placed(&mut self, agent: PlayerId) {
        self.prune_placed(agent);
        let Some(player) = self.players.get_mut(&agent) else { return };
        let owner = crate::player_key(&player.name);
        player.tallies.retain(|key, _| !key.starts_with("placed:"));
        for (_, item) in self.quest_placed.values().filter(|(who, _)| *who == owner) {
            *player.tallies.entry(format!("placed:{item}")).or_default() += 1;
        }
    }

    /// Drops `agent`'s records of blocks set down that no quest still open to them counts,
    /// and takes them off their tally.
    fn prune_placed(&mut self, agent: PlayerId) {
        let Some(player) = self.players.get(&agent) else { return };
        let (owner, done) = (crate::player_key(&player.name), player.quests_done.clone());
        let (blocks, quests) = (&self.content.blocks, &self.content.quests);
        let mut dropped = Vec::new();
        self.quest_placed.retain(|_, (who, item)| {
            let keep = *who != owner || counts_placed(blocks, quests, item, &|quest| done.iter().any(|d| d == quest));
            if !keep {
                dropped.push(item.clone());
            }
            keep
        });
        for item in dropped {
            self.untally(agent, "placed", &item, 1);
        }
    }

    /// The joined player whose `player_key` is `owner`.
    fn here(&self, owner: &str) -> Option<PlayerId> {
        self.players.iter().find(|(_, p)| p.joined && crate::player_key(&p.name) == owner).map(|(&id, _)| id)
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
    pub(crate) fn update_quests(&mut self, id: PlayerId) {
        let (finished, states) = self.quest_check(id);
        for name in &finished {
            self.finish_quest(id, name, &finished);
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
    /// the chat line says so, and a second names what finishing it opened (none of `batch`,
    /// the quests finishing with it, which say so themselves); a quest that pays nothing is
    /// claimed at once.
    pub(crate) fn finish_quest(&mut self, agent: PlayerId, name: &str, batch: &[String]) {
        let Some(quest) = self.content.quests.get(name).cloned() else { return };
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.quests_done.push(name.to_owned());
        if !quest.pays() {
            player.quests_claimed.push(name.to_owned());
        }
        // Blocks set down that no quest still open to them counts are no longer kept.
        self.prune_placed(agent);
        let Some(player) = self.players.get_mut(&agent) else { return };
        let title = Words::Own(voxmar_engine::lang::def_key("quest", &quest.name), quest.title.clone());
        let key = if quest.pays() { "quest.ready" } else { "quest.done" };
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key(key).with("quest", title) });
        // And what it opened, so the next step is named as this one ends: open now, and
        // not before it was done (one coming after it or another quest already done).
        let done = |n: &str| player.quests_done.iter().any(|d| d == n);
        let without = |n: &str| n != name && done(n);
        let opened: Vec<Words> = self
            .content
            .quests
            .iter()
            .filter(|q| !done(&q.name) && !batch.contains(&q.name) && q.after.iter().flat_map(|a| a.split('|')).any(|a| a == name))
            .filter(|q| self.content.quests.open(q, &done) && !self.content.quests.open(q, &without))
            .map(|q| Words::Own(voxmar_engine::lang::def_key("quest", &q.name), q.title.clone()))
            .collect();
        if !opened.is_empty() {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("quest.opens").with("list", Words::List(opened)) });
        }
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

#[cfg(test)]
mod tests {
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::glam::{IVec3, Vec3};
    use voxmar_engine::lang::Words;
    use voxmar_engine::world::Column;
    use voxmar_protocol::{ServerMessage, TICKS_PER_DAY};

    use crate::Server;

    /// A stone floor at y 199 under a morning sky, and one player in the middle: a party can
    /// stand and talk, an altar can be prayed at, with no generated land.
    fn fixture() -> Server {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        for cx in 0..8 {
            for cz in 0..8 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        column.set_block(IVec3::new(x, 199, z), stone);
                    }
                }
                server.world.insert(ColumnPos::new(cx, cz), std::sync::Arc::new(column));
            }
        }
        server.day = TICKS_PER_DAY as f64 * 0.3;
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let mut player = crate::player::Player::new(1, connection, Vec3::new(128.5, 200.0, 128.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.name) = (true, "Tëster".into());
        server.players.insert(1, player);
        server
    }

    fn hold(server: &mut Server, item: &str, count: u8) {
        let item = server.content.blocks.expect(item);
        let p = server.players.get_mut(&1).unwrap();
        p.selected = 0;
        p.inventory.clear(|_| true);
        assert_eq!(p.inventory.add(item, count), 0);
    }

    /// A block set down counts for a quest's `Placed` only while the block in its cell still
    /// drops it: broken, built over or washed away, it comes off; a snare sprung (its block
    /// changed, not gone) or set again over itself counts once; broken while its player is
    /// away, it comes off as they join; a block no open quest asks to be set down is never
    /// tallied.
    #[test]
    fn a_placed_block_counts_while_it_stands() {
        use voxmar_engine::action::ActionTarget;
        use voxmar_engine::block::BlockId;
        use voxmar_engine::coords::Face;
        let mut server = fixture();
        let place = server.content.actions.expect("core:place");
        let set = |server: &mut Server, item: &str, x: i32| {
            hold(server, item, 1);
            server.propose(1, place, ActionTarget::Block { block: IVec3::new(x, 199, 130), face: Face::PosY }, std::time::Instant::now());
            assert!(server.block(IVec3::new(x, 200, 130)) != Some(BlockId::AIR), "{item} set down");
            IVec3::new(x, 200, 130)
        };
        let trough = set(&mut server, "base:feed_trough", 127);
        assert_eq!(tally(&server, "placed:base:feed_trough"), 1);
        server.set_block(trough, BlockId::AIR);
        assert_eq!(tally(&server, "placed:base:feed_trough"), 0, "broken, it no longer counts");
        set(&mut server, "base:feed_trough", 127);
        assert_eq!(tally(&server, "placed:base:feed_trough"), 1, "set down again, once");
        let snare = set(&mut server, "base:snare", 129);
        let trap = server.content.traps.traps.iter().find(|t| server.block(snare) == Some(t.set)).expect("a set snare");
        let (armed, sprung) = (trap.set, trap.sprung);
        server.set_block(snare, sprung);
        assert_eq!(tally(&server, "placed:base:snare"), 1, "sprung is still standing");
        set(&mut server, "base:snare", 129);
        assert_eq!(server.block(snare), Some(armed), "the cell holds the set snare again");
        assert_eq!(tally(&server, "placed:base:snare"), 1, "a snare set over its own cell counts once");
        set(&mut server, "base:cobblestone", 129);
        assert_eq!(tally(&server, "placed:base:snare"), 0, "built over, the snare no longer counts");
        assert!(!server.quest_placed.contains_key(&snare));
        set(&mut server, "base:cobblestone", 131);
        assert_eq!(tally(&server, "placed:base:cobblestone"), 0, "no quest asks for cobblestone set down");
        // Water washes a snare away, in the fluids' own changes.
        let washed = set(&mut server, "base:snare", 133);
        assert_eq!(tally(&server, "placed:base:snare"), 1);
        // Marked as a player's own, as a planted flower is: washed away, it is forgotten.
        server.placed.insert(washed);
        server.set_block(washed + IVec3::X, server.content.blocks.expect("base:water"));
        for _ in 0..200 {
            server.tick += 1;
            server.flow();
        }
        assert!(server.block(washed).is_some_and(|b| server.content.blocks.info(b).fluid.is_some()), "the water took the snare's cell");
        assert_eq!(tally(&server, "placed:base:snare"), 0, "washed away, the snare no longer counts");
        assert!(!server.placed.contains(&washed), "nor is its cell still the player's");
        // Broken while its player is away, it comes off as they join again.
        let away = server.players.remove(&1).expect("the player");
        server.set_block(trough, BlockId::AIR);
        server.players.insert(1, away);
        assert_eq!(tally(&server, "placed:base:feed_trough"), 1, "not yet told");
        server.recount_placed(1);
        assert_eq!(tally(&server, "placed:base:feed_trough"), 0, "broken while they were away");
        // Kept only while a quest still open to them counts it.
        let trough = set(&mut server, "base:feed_trough", 127);
        server.finish_quest(1, "base:a_trough_for_the_pen", &["base:a_trough_for_the_pen".into()]);
        assert!(!server.quest_placed.contains_key(&trough), "no open quest counts it any more");
        set(&mut server, "base:feed_trough", 125);
        assert_eq!(tally(&server, "placed:base:feed_trough"), 0, "nor is one set down after");
    }

    /// A save's records come back by cell: a renamed item's under its new name, and one no
    /// quest counts any more (a mod took its quest away) not at all, nor on its owner's tally
    /// as they join.
    #[test]
    fn placed_records_load_renamed_and_without_quests_gone() {
        // Its own load, not the shared one: the rename changes the blocks in place.
        let mut content = crate::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
        std::sync::Arc::get_mut(&mut content.blocks).expect("its own").rename(vec![("base:old_snare".into(), "base:snare".into())]).unwrap();
        let saved = vec![([1, 2, 3], "Ada".to_owned(), "base:old_snare".to_owned()), ([4, 5, 6], "ada".to_owned(), "base:cobblestone".to_owned()), ([7, 8, 9], "ada".to_owned(), "base:gone".to_owned())];
        let restored = super::restore_placed(&saved, &content.blocks, &content.quests);
        let restored: Vec<_> = restored.into_iter().collect();
        assert_eq!(restored, [(IVec3::new(1, 2, 3), ("ada".to_owned(), "base:snare".to_owned()))], "renamed loads as the snare; cobblestone no quest counts; an unknown item goes");
        // At join: one whose quest a mod took away goes, and its tally with it.
        let mut server = fixture();
        let name = server.players[&1].name.clone();
        let owner = crate::player_key(&name);
        server.quest_placed.insert(IVec3::new(1, 2, 3), (owner.clone(), "base:cobblestone".into()));
        server.quest_placed.insert(IVec3::new(4, 5, 6), (owner, "base:snare".into()));
        server.recount_placed(1);
        assert_eq!(server.quest_placed.len(), 1, "only the snare a quest counts is kept");
        assert_eq!((tally(&server, "placed:base:snare"), tally(&server, "placed:base:cobblestone")), (1, 0));
    }

    /// Starts a party of `kind` beside the player and gives back its first member.
    fn party(server: &mut Server, kind: &str) -> u32 {
        let event = server.content.wandering.find(kind).unwrap_or_else(|| panic!("no wandering kind `{kind}`"));
        let spot = server.standing_spot(130, 128, 200).expect("floor");
        let before: Vec<u32> = server.creatures.live.keys().copied().collect();
        server.begin_event_at(Some(1), event, spot, 7, true).expect("the party fits");
        let leads = server.content.wandering.get(event).groups[0].0;
        let mut found = server.creatures.live.iter().filter(|&(id, c)| c.species == leads && !before.contains(id) && server.wandering.manages(*id)).map(|(&id, _)| id);
        found.next().expect("the party's leader")
    }

    fn tally(server: &Server, key: &str) -> u32 {
        server.players[&1].tallies.get(key).copied().unwrap_or(0)
    }

    fn done(server: &Server, quest: &str) -> bool {
        server.players[&1].quests_done.iter().any(|q| q == quest)
    }

    /// Finishing a quest names what it opened, after the line saying it is done; not one
    /// still waiting on another quest, one open already (after either of two, the other
    /// done), or one finishing with it, which says so itself.
    #[test]
    fn a_finished_quest_names_what_it_opened() {
        let mut server = fixture();
        let quest = |name: &str, after: &str| format!(r#"(name: "{name}", age: "nomad", branch: "main", title: "{name}", why: "", tasks: [[Home]], after: [{after}])"#);
        let source = format!(
            "(quests: [{}, {}, {}, {}, {}])",
            quest("a", ""),
            quest("x", ""),
            quest("next", r#""a""#),
            quest("waiting", r#""a", "x""#),
            quest("either", r#""x|a""#)
        );
        let file = voxmar_engine::quest::QuestRegistry::read("t", &source).unwrap();
        server.content.quests = std::sync::Arc::new(voxmar_engine::quest::QuestRegistry::new(vec![file]).unwrap());
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        // What the chat names as opened when `name` finishes with `batch`, `done` done before.
        let mut opened_by = |done: &[&str], name: &str, batch: &[&str]| -> Vec<String> {
            server.players.get_mut(&1).unwrap().quests_done = done.iter().map(|n| format!("t:{n}")).collect();
            let batch: Vec<String> = batch.iter().map(|n| format!("t:{n}")).collect();
            server.finish_quest(1, &format!("t:{name}"), &batch);
            let mut lines = Vec::new();
            while let Ok(Some(message)) = client.try_recv() {
                if let ServerMessage::Chat { text, .. } = message {
                    lines.push(text);
                }
            }
            match lines.get(1) {
                Some(Words::Key(key, args)) if key == "quest.opens" => match args.iter().find(|(arg, _)| arg == "list") {
                    Some((_, Words::List(opened))) => opened.iter().map(|w| if let Words::Own(_, title) = w { title.clone() } else { panic!("{w:?}") }).collect(),
                    _ => panic!("{args:?}"),
                },
                None => Vec::new(),
                _ => panic!("{lines:?}"),
            }
        };
        assert_eq!(opened_by(&[], "a", &["a"]), ["next", "either"], "not one waiting on x");
        assert_eq!(opened_by(&["x"], "a", &["a"]), ["next", "waiting"], "either was open with x done");
        assert_eq!(opened_by(&[], "a", &["a", "next"]), ["either"], "next says it is done itself");
    }

    #[test]
    fn a_dealing_with_a_wanderer_counts_and_a_greeting_does_not() {
        let mut server = fixture();
        let pedlar = party(&mut server, "pedlar_and_hound");
        hold(&mut server, "base:hide", 1);
        assert!(server.meet(1, pedlar));
        assert_eq!(tally(&server, "met:base:pedlar_and_hound"), 0, "one hide buys nothing: only a greeting");
        hold(&mut server, "base:hide", 2);
        assert!(server.meet(1, pedlar));
        assert_eq!(tally(&server, "met:base:pedlar_and_hound"), 1, "two hides for twine is a dealing");
        // A line said for its words alone (the hurt hunter has nothing to teach the skilled)
        // is no dealing either.
        let hunter = party(&mut server, "wounded_hunter");
        let skill = server.content.skills.id("core:hunter").expect("hunter");
        let xp = server.content.skills.xp_for(15);
        server.players.get_mut(&1).unwrap().skills.grant(skill, xp);
        hold(&mut server, "base:bread", 4);
        assert!(server.meet(1, hunter));
        assert_eq!(tally(&server, "met:base:wounded_hunter"), 0, "the skilled are only told so");
    }

    #[test]
    fn dealing_with_travellers_finishes_the_road_quests() {
        let mut server = fixture();
        for quest in ["base:look_it_up", "base:timber", "base:stone_in_hand", "base:where_to_settle"] {
            server.players.get_mut(&1).unwrap().quests_done.push(quest.into());
        }
        let pedlar = party(&mut server, "pedlar_and_hound");
        hold(&mut server, "base:hide", 2);
        assert!(server.meet(1, pedlar));
        server.update_quests(1);
        assert!(done(&server, "base:folk_of_the_road") && done(&server, "base:something_for_something"), "a pedlar's swap is a dealing, and a trade");
        assert!(!done(&server, "base:a_kind_turn"), "a trade is no kindness");
        let hunter = party(&mut server, "wounded_hunter");
        hold(&mut server, "base:bread", 4);
        assert!(server.meet(1, hunter));
        server.update_quests(1);
        assert!(done(&server, "base:a_kind_turn"), "food for a hurt hunter is");
        assert!(!done(&server, "base:many_travellers"), "two kinds of five, and no home yet");
        let progress = server.quest_check(1).1.into_iter().find(|s| s.name == "base:many_travellers").unwrap();
        assert_eq!(progress.tasks[0][0], (2, 5), "kinds dealt with, out of five");
    }

    /// What is eaten counts by food (`Ate`), and an effect counts once it has been felt at
    /// all (`Felt`), whatever gave it: Sweet Relief and Smoke That Clings finish from them.
    #[test]
    fn food_eaten_and_smoke_felt_finish_the_honey_quests() {
        use crate::process::Process;
        let mut server = fixture();
        for quest in ["base:humming_in_the_meadow", "base:a_bee_garden"] {
            server.players.get_mut(&1).unwrap().quests_done.push(quest.into());
        }
        let eat = server.content.actions.expect("core:eat");
        hold(&mut server, "base:honey", 6);
        // Food is eaten only by the hurt.
        server.players.get_mut(&1).unwrap().health.current = 2;
        let mut at = std::time::Instant::now();
        for _ in 0..3 {
            server.propose(1, eat, voxmar_engine::action::ActionTarget::None, at);
            at += std::time::Duration::from_secs(2);
            server.finished_action(1, voxmar_engine::action::ActionTarget::None, at);
        }
        assert_eq!(tally(&server, "ate:base:honey"), 3, "each honey eaten counts");
        server.update_quests(1);
        assert!(done(&server, "base:sweet_relief"), "three eaten, three kept");
        assert!(!done(&server, "base:smoke_that_clings"), "the honey, but no smoke yet");
        // A fire within two blocks leaves the player Smoky: felt once, it counts once.
        let feet = server.players[&1].position.floor().as_ivec3();
        server.set_block(feet + IVec3::new(0, 0, -2), server.content.blocks.expect("base:campfire"));
        hold(&mut server, "base:honey", 4);
        crate::process::Survival.run(&mut server);
        crate::process::Survival.run(&mut server);
        assert_eq!(tally(&server, "felt:base:smoky"), 1, "an effect counts once, however long it lasts");
        server.update_quests(1);
        assert!(done(&server, "base:smoke_that_clings"));
    }

    #[test]
    fn prayers_turned_on_and_bones_offered_finish_the_altar_rites() {
        let mut server = fixture();
        server.players.get_mut(&1).unwrap().quests_done.push("base:a_place_to_pray".into());
        let bark = server.content.prayers.id("base:bark_skin").expect("bark skin");
        hold(&mut server, "base:bone", 3);
        for _ in 0..3 {
            server.pray_at_altar(1);
        }
        assert_eq!(tally(&server, "offered:base:bone"), 3);
        server.update_quests(1);
        assert!(!done(&server, "base:altar_rites"), "the bones, but no prayer yet");
        server.pray(1, bark, true);
        server.pray(1, bark, true);
        assert_eq!(tally(&server, "prayed:base:bark_skin"), 1, "a prayer already on is not turned on again");
        server.update_quests(1);
        assert!(done(&server, "base:altar_rites"));
        // Nothing to offer is nothing offered.
        server.pray_at_altar(1);
        assert_eq!(tally(&server, "offered:base:bone"), 3);
    }

    #[test]
    fn the_home_room_quests_read_the_steps_the_home_reaches() {
        let mut server = fixture();
        for quest in ["base:look_it_up", "base:timber", "base:first_night", "base:a_bed_of_your_own", "base:torchlight", "base:make_camp", "base:a_friend_in_the_wild"] {
            server.players.get_mut(&1).unwrap().quests_done.push(quest.into());
        }
        // Reaching a room's step is tallied as the home is surveyed (process.rs); the quests
        // name those steps.
        server.tally(1, "built", "base:home_company", 1);
        server.update_quests(1);
        assert!(done(&server, "base:company_at_camp"));
        server.tally(1, "built", "base:home_pantry", 1);
        server.update_quests(1);
        assert!(done(&server, "base:pantry_shelves"));
    }
}
