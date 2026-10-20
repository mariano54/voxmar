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
    fn finish_quest(&mut self, agent: PlayerId, name: &str, batch: &[String]) {
        let Some(quest) = self.content.quests.get(name).cloned() else { return };
        let Some(player) = self.players.get_mut(&agent) else { return };
        player.quests_done.push(name.to_owned());
        if !quest.pays() {
            player.quests_claimed.push(name.to_owned());
        }
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
