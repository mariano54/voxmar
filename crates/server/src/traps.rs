//! Traps on the server (`traps.ron`, docs/traps.md): a wild creature of a kind a set trap
//! catches that steps into its cell is caught. It is gone from the world, the trap shows
//! sprung, and what it was is kept with the trap until a player takes it: its loot, rolled
//! for them as a kill of theirs, and its kill experience. The trap is set again.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::creature::CreatureId;
use voxmar_engine::crop::level_bonus;
use voxmar_engine::dropped;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::loot::{self, Moment};
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits;
use voxmar_protocol::PlayerId;

use crate::Server;
use crate::process::Process;

/// What a sprung trap holds: the species caught, and the caught creature's own seed, kept
/// with it in saves (its loot rolls by the emptier's count of catches, `Draw::Catch`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Catch {
    pub species: CreatureId,
    pub seed: u64,
}

/// Every sprung trap's catch, by where it stands; and where each wild creature stood at the
/// traps' last look (or where it arrived since), so one that ran through a set trap's cell
/// between two looks is caught there.
#[derive(Default)]
pub struct Catches {
    caught: FxHashMap<IVec3, Catch>,
    seen: FxHashMap<u32, Vec3>,
}

/// The furthest a creature moves between two looks (4 ticks) for the cells it crossed to
/// count: a sprint is under 2 blocks; a longer move is a blink or a teleport, which crosses
/// nothing on the way.
const CROSSED_MOST: f32 = 4.0;

/// The cells a creature's feet crossed going from `from` to `to`, in order, ending with the
/// one it stands in: the straight path sampled a quarter of a block apart.
fn crossed(from: Vec3, to: Vec3) -> Vec<IVec3> {
    let length = from.distance(to);
    if length > CROSSED_MOST {
        return vec![to.floor().as_ivec3()];
    }
    let steps = (length / 0.25).ceil() as u32;
    let mut cells: Vec<IVec3> = Vec::new();
    for i in 0..=steps {
        let cell = from.lerp(to, if steps == 0 { 1.0 } else { i as f32 / steps as f32 }).floor().as_ivec3();
        if cells.last() != Some(&cell) {
            cells.push(cell);
        }
    }
    cells
}

/// A catch as `world.ron` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCatch {
    pub position: [i32; 3],
    pub species: String,
    pub seed: u64,
}

impl Catches {
    pub fn saved(&self, name: impl Fn(CreatureId) -> String) -> Vec<SavedCatch> {
        let mut saved: Vec<SavedCatch> = self.caught.iter().map(|(pos, c)| SavedCatch { position: pos.to_array(), species: name(c.species), seed: c.seed }).collect();
        saved.sort_by_key(|c| c.position);
        saved
    }

    /// The saved catches whose species is still known.
    pub fn restore(saved: &[SavedCatch], id: impl Fn(&str) -> Option<CreatureId>) -> Self {
        Self { caught: saved.iter().filter_map(|c| Some((IVec3::from_array(c.position), Catch { species: id(&c.species)?, seed: c.seed }))).collect(), seen: FxHashMap::default() }
    }

    /// Creature `id` arrived at `at` since the traps last looked.
    pub(crate) fn arrived(&mut self, id: u32, at: Vec3) {
        self.seen.insert(id, at);
    }

    #[cfg(test)]
    pub fn get(&self, pos: IVec3) -> Option<Catch> {
        self.caught.get(&pos).copied()
    }
}

/// `base:traps`: springs set traps on the creatures that stepped into them, every 4 ticks
/// (`Tier::Medium`), so a rabbit running through a set cell is still in it at a look.
pub(crate) struct Traps;

impl Process for Traps {
    fn tier(&self) -> Tier {
        Tier::Medium
    }

    fn run(&mut self, server: &mut Server) {
        if server.content.traps.traps.is_empty() {
            return;
        }
        server.spring_traps();
    }
}

impl Server {
    /// Each wild creature a trap catches, standing in a set one's cell or having crossed it
    /// since the last look, is caught there.
    pub(crate) fn spring_traps(&mut self) {
        let traps = self.content.traps.clone();
        let seen = std::mem::take(&mut self.catches.seen);
        let wild = self.creatures.live.iter().filter(|(_, c)| c.owner.is_none() && c.health > 0);
        // Every set trap each crossed that catches its kind, in the order it crossed them.
        let crossings: Vec<(u32, Vec<(IVec3, BlockId)>)> = wild
            .clone()
            .map(|(&id, c)| {
                let now = c.body.position;
                let set = crossed(seen.get(&id).copied().unwrap_or(now), now)
                    .into_iter()
                    .filter_map(|cell| {
                        let trap = self.world.block(cell).and_then(|b| traps.set(b)).filter(|t| t.catches(c.species))?;
                        Some((cell, trap.sprung))
                    })
                    .collect();
                (id, set)
            })
            .filter(|(_, set): &(u32, Vec<_>)| !set.is_empty())
            .collect();
        self.catches.seen = wild.map(|(&id, c)| (id, c.body.position)).collect();
        for (id, set) in crossings {
            // One creature to a trap: the first it crossed that is still set when its turn
            // comes, so a second through a trap another sprang on this look goes on to the
            // next trap it crossed (or, standing in it, waits for the trap set again).
            let Some((cell, sprung)) = set.into_iter().find(|&(cell, _)| self.world.block(cell).and_then(|b| traps.set(b)).is_some()) else { continue };
            let Some(creature) = self.creatures.live.get(&id) else { continue };
            let catch = Catch { species: creature.species, seed: creature.seed };
            self.creature_felled(id, None);
            self.set_block(cell, sprung);
            self.catches.caught.insert(cell, catch);
        }
    }

    /// `agent` takes what the sprung trap at `pos` caught (`base:take_from_trap`): its loot
    /// as a kill of theirs, and its kill experience; the trap is set again.
    pub(crate) fn take_from_trap(&mut self, agent: PlayerId, pos: IVec3) {
        let traps = self.content.traps.clone();
        let Some(trap) = self.block(pos).and_then(|b| traps.sprung(b)) else { return };
        let Some(player) = self.players.get(&agent) else { return };
        let level = self.content.skills.level(player.skills.xp(trap.skill));
        if level < trap.level {
            let skill = Words::Own(voxmar_engine::lang::def_key("skill", &self.content.skills.get(trap.skill).name), self.content.skills.get(trap.skill).title.clone());
            self.refuse(agent, Words::key("trap.needs").with("skill", skill).with("level", Words::text(trap.level)));
            return;
        }
        let catch = self.catches.caught.remove(&pos);
        self.set_block(pos, trap.set);
        let Some(catch) = catch else { return };
        let species = self.content.creatures.get(catch.species);
        let player = &self.players[&agent];
        let tool: Vec<String> = player.held().map_or_else(Vec::new, |item| self.content.blocks.def(item).tags.clone());
        let bonus = species.xp_skill.map_or(0.0, |skill| level_bonus(self.content.skills.level(player.skills.xp(skill)), 1, self.content.skills.max_level()));
        let moment = Moment { bonus, tool: &tool, night: self.daylight() < 0.3, by_player: true };
        // Each roll numbered by the emptier's own count of catches, never by the tick it is
        // emptied on or by what else rolled first.
        let player = self.players.get_mut(&agent).expect("checked above");
        let found = loot::roll(&species.loot, &moment, || player.draw(crate::save::Draw::Catch)[0]);
        let name = species.def.name.clone();
        let trapped = name.clone();
        let page = Self::page_of(&species.def);
        let kill = species.xp_skill.map(|skill| (skill, species.def.xp.kill));
        let mut journal: Vec<(voxmar_engine::journal::Page, String, u32)> = page.map(|page| (page, name, 1)).into_iter().collect();
        for (i, (item, count)) in found.into_iter().enumerate() {
            journal.push((voxmar_engine::journal::Page::Collection, self.content.blocks.def(item).name.clone(), count as u32));
            let Some(player) = self.players.get_mut(&agent) else { break };
            let left = player.inventory.add(item, count);
            player.changed(traits::INVENTORY);
            if left > 0 {
                let body = dropped::popped(pos, self.meta.next_pop(pos, 0x5a4e + i as u32));
                self.items.spawn(ItemStack::new(item, left), body, dropped::PICKUP_DELAY);
            }
        }
        if let Some((skill, xp)) = kill {
            self.grant_xp(agent, skill, xp);
        }
        for (page, name, times) in journal {
            self.discover(agent, page, &name, times);
        }
        // For a quest's `Trapped`.
        self.tally(agent, "trapped", &trapped, 1);
    }

    /// A block changed: a sprung trap broken or replaced loses its catch.
    pub(crate) fn trap_changed(&mut self, pos: IVec3, block: BlockId) {
        if self.content.traps.sprung(block).is_none() {
            self.catches.caught.remove(&pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::glam::Vec3;
    use voxmar_engine::world::Column;
    use voxmar_protocol::{ServerMessage, TICKS_PER_SECOND};

    use super::*;

    const FLOOR: i32 = 199;

    /// A stone floor far from the world, player 1 on it and a snare set at (10, 200, 10).
    fn field(hunter: u8) -> (Server, IVec3) {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        let mut column = Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, FLOOR, z), stone);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        let (_client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, Vec3::new(4.5, FLOOR as f32 + 1.0, 4.5), 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        let skill = server.content.skills.id("core:hunter").unwrap();
        let xp = server.content.skills.xp_for(hunter);
        player.skills.grant(skill, xp);
        server.players.insert(1, player);
        let snare = IVec3::new(10, FLOOR + 1, 10);
        server.set_block(snare, server.content.blocks.expect("base:snare"));
        (server, snare)
    }

    fn put(server: &mut Server, name: &str, at: Vec3) -> u32 {
        let species = server.content.creatures.id(name).unwrap();
        server.place_creature(species, at, 7, 0.0).expect("placed")
    }

    fn named(server: &Server, pos: IVec3) -> String {
        server.block(pos).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone())
    }

    fn hunter_xp(server: &Server) -> u32 {
        server.players[&1].skills.xp(server.content.skills.id("core:hunter").unwrap())
    }

    /// A rabbit that steps into a set snare is caught: gone from the world, the snare
    /// sprung; taken, its loot comes to hand with its kill's experience, and the snare is
    /// set again.
    #[test]
    fn a_rabbit_steps_into_a_snare_and_is_taken_from_it() {
        let (mut server, snare) = field(1);
        let rabbit = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.spring_traps();
        assert!(!server.creatures.live.contains_key(&rabbit), "caught, gone from the world");
        assert_eq!(named(&server, snare), "base:snare_sprung");
        let before = hunter_xp(&server);
        server.take_from_trap(1, snare);
        assert_eq!(named(&server, snare), "base:snare", "set again");
        assert!(hunter_xp(&server) > before, "the kill's experience");
        assert_eq!(server.players[&1].tallies.get("trapped:base:rabbit"), Some(&1), "tallied for quests");
        let carried: u32 = server.players[&1].inventory.slots.iter().flatten().map(|s| s.count as u32).sum();
        assert!(carried > 0, "its loot came to hand");
        // Taken once: nothing more.
        server.take_from_trap(1, snare);
        assert_eq!(named(&server, snare), "base:snare");
    }

    /// Below the trap's level a catch stays where it is, and the player is told why as a
    /// refusal (under the crosshair), not a chat line. The base snare asks for Hunter 1,
    /// which every player has, so the test asks for more.
    #[test]
    fn a_catch_below_the_traps_level_is_refused_with_why() {
        let (mut server, snare) = field(1);
        let mut traps = (*server.content.traps).clone();
        traps.traps.iter_mut().for_each(|trap| trap.level = 5);
        server.content.traps = Arc::new(traps);
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.spring_traps();
        server.take_from_trap(1, snare);
        assert_eq!(named(&server, snare), "base:snare_sprung", "still holding its catch");
        assert!(server.catches.caught.contains_key(&snare));
        let mut refused = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            match message {
                ServerMessage::Refused { why: Words::Key(key, _) } => refused.push(key),
                ServerMessage::Chat { text: Words::Key(key, _), .. } => assert_ne!(key, "trap.needs", "said in chat"),
                _ => {}
            }
        }
        assert_eq!(refused, ["trap.needs"]);
    }

    /// What the gate's Snare step does: a rabbit summoned (by command, with the player two
    /// blocks off) into a set snare's cell is caught by the server's own ticks.
    #[test]
    fn a_rabbit_summoned_into_a_set_snare_springs_it() {
        let (mut server, snare) = field(1);
        server.players.get_mut(&1).unwrap().position = snare.as_vec3() + Vec3::new(0.5, 0.0, -2.5);
        server.console(&format!("summon rabbit 1 {} {} {}", snare.x + 2, snare.y, snare.z));
        let rabbit = server.creatures.live.iter().find(|(_, c)| c.owner.is_none()).map(|(&id, c)| (id, c.body.position));
        let (_, at) = rabbit.expect("a rabbit was summoned");
        assert_eq!(at.floor().as_ivec3(), snare, "summoned into the snare's cell, not {at}");
        for _ in 0..TICKS_PER_SECOND * 2 {
            server.tick();
            if named(&server, snare) == "base:snare_sprung" {
                return;
            }
        }
        panic!("the snare never sprang: the rabbit is at {:?}", server.creatures.live.values().find(|c| c.owner.is_none()).map(|c| c.body.position));
    }

    /// A snare catches only its kinds, only wild ones, and only in its own cell.
    #[test]
    fn a_snare_holds_only_small_wild_game_in_its_cell() {
        let (mut server, snare) = field(1);
        let wolf = put(&mut server, "base:wolf", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        let beside = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(2.5, 0.0, 0.5));
        let kept = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.creatures.live.get_mut(&kept).unwrap().owner = Some("someone".into());
        server.spring_traps();
        assert!([wolf, beside, kept].iter().all(|id| server.creatures.live.contains_key(id)));
        assert_eq!(named(&server, snare), "base:snare");
    }

    /// Breaking a sprung snare loses its catch; a catch keeps across a save.
    #[test]
    fn a_catch_keeps_across_a_save_and_is_lost_with_the_snare() {
        let (mut server, snare) = field(1);
        put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        server.spring_traps();
        let saved = server.catches.saved(|id| server.content.creatures.get(id).def.name.clone());
        let restored = Catches::restore(&saved, |name| server.content.creatures.id(name));
        assert_eq!(restored.get(snare), server.catches.get(snare));
        assert!(restored.get(snare).is_some());
        server.set_block(snare, BlockId::AIR);
        assert!(server.catches.get(snare).is_none(), "broken, the catch is lost");
    }

    /// A catch's loot is rolled by the emptier's own count of catches, not the tick it is
    /// taken on or what else the world rolled first, and each roll counts on.
    #[test]
    fn a_catch_rolls_by_the_emptiers_count_not_the_tick_or_other_rolls() {
        // Later, and with the world's other loot rolls (kills elsewhere) drawn in between.
        let taken = |tick: u64, others: usize| {
            let (mut server, snare) = field(1);
            server.meta.draw_key = 0x0d1a;
            server.tick = tick;
            let mut found = Vec::new();
            for _ in 0..12 {
                for _ in 0..others {
                    server.meta.next_loot();
                }
                put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
                server.spring_traps();
                server.players.get_mut(&1).unwrap().inventory = Default::default();
                server.take_from_trap(1, snare);
                found.push(server.players[&1].inventory.slots.iter().flatten().map(|s| (s.item, s.count)).collect::<Vec<_>>());
            }
            (found, server.players[&1].draws.catches)
        };
        let (first, rolls) = taken(1_000, 0);
        assert!(rolls > 0 && first.iter().all(|f| !f.is_empty()), "twelve catches rolled {rolls} times for {first:?}");
        assert_eq!(taken(1_377, 3), (first, rolls), "the same catches taken later, after other rolls, gave otherwise");
    }

    /// A rabbit summoned into a set snare, with a player by it as the gate's snare step
    /// stands, held or loose, is caught whatever tick it arrives on. Held, it is there at the
    /// traps' next look; loose, arriving just after a look, it is out of the cell by the next,
    /// but the traps know where it arrived and the cells it crossed since.
    #[test]
    fn a_rabbit_summoned_into_a_snare_held_or_loose_is_caught() {
        for held in [true, false] {
            for phase in 0..Tier::Medium.period() {
                let (mut server, snare) = field(1);
                server.players.get_mut(&1).unwrap().position = Vec3::new(snare.x as f32 + 0.5, snare.y as f32, snare.z as f32 - 2.5);
                server.tick = 1000 + phase;
                let at = [snare.x + 2, snare.y, snare.z].map(|v| v.to_string());
                let args: Vec<String> = ["rabbit", "1"].into_iter().map(str::to_owned).chain(at).chain(held.then(|| "held".to_owned())).collect();
                server.summon(crate::commands::Source::Player(1), &args).expect("summoned");
                let caught = (0..Tier::Medium.period()).position(|_| {
                    server.tick();
                    server.catches.get(snare).is_some()
                });
                assert!(caught.is_some(), "arriving {phase} ticks past a look, a rabbit (held {held}) in the snare was not caught at the next");
            }
        }
    }

    /// A rabbit that ran across a set snare's cell between two looks, standing either side of
    /// it at each, is caught in it; one that went round it is not, nor one that blinked
    /// further than any run.
    #[test]
    fn a_rabbit_that_crossed_a_snare_between_looks_is_caught_there() {
        let ran = |from: Vec3, to: Vec3| {
            let (mut server, snare) = field(1);
            let id = put(&mut server, "base:rabbit", snare.as_vec3() + from);
            server.spring_traps();
            assert!(server.catches.get(snare).is_none(), "caught before it ran");
            server.creatures.live.get_mut(&id).unwrap().body.position = snare.as_vec3() + to;
            server.spring_traps();
            server.catches.get(snare).is_some()
        };
        assert!(ran(Vec3::new(-1.0, 0.0, 0.5), Vec3::new(2.0, 0.0, 0.5)), "ran straight through");
        assert!(!ran(Vec3::new(-1.0, 0.0, -0.5), Vec3::new(2.0, 0.0, -0.5)), "ran beside it");
        assert!(!ran(Vec3::new(-3.0, 0.0, 0.5), Vec3::new(3.0, 0.0, 0.5)), "a blink of six blocks");
    }

    /// Where each wild creature stood at the last look is kept for those still wild and alive
    /// only: one felled, taken away or taken in is dropped at the next look, so the record
    /// never outgrows the wild about; and none of it is saved, so a loaded world's first look
    /// starts from where each creature stands.
    #[test]
    fn the_last_look_keeps_only_the_wild_and_starts_fresh_after_a_load() {
        let (mut server, snare) = field(1);
        let [stays, felled, gone, kept] = [-6.0, -4.0, 4.0, 6.0].map(|x| put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(x, 0.0, 0.5)));
        let seen = |server: &Server| {
            let mut ids: Vec<u32> = server.catches.seen.keys().copied().collect();
            ids.sort_unstable();
            ids
        };
        server.spring_traps();
        let mut all = vec![stays, felled, gone, kept];
        all.sort_unstable();
        assert_eq!(seen(&server), all);
        server.creature_felled(felled, None);
        server.creatures.take_away(gone);
        server.creatures.live.get_mut(&kept).unwrap().owner = Some("keeper".to_owned());
        server.spring_traps();
        assert_eq!(seen(&server), [stays], "only the one still wild and alive is remembered");
        assert!(Catches::restore(&[], |_| None).seen.is_empty(), "a loaded world remembers no last look");
    }

    /// Two rabbits that ran through the same two snares in a line between two looks are both
    /// caught, one in each: the second, finding the first snare sprung, is caught in the next.
    #[test]
    fn two_rabbits_through_a_line_of_snares_are_caught_one_in_each() {
        let (mut server, snare) = field(1);
        let next = snare + IVec3::X;
        server.set_block(next, server.content.blocks.expect("base:snare"));
        let ids = [0.3, 0.7].map(|z| put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(-1.0, 0.0, z)));
        server.spring_traps();
        for (&id, z) in ids.iter().zip([0.3, 0.7]) {
            server.creatures.live.get_mut(&id).unwrap().body.position = snare.as_vec3() + Vec3::new(3.0, 0.0, z);
        }
        server.spring_traps();
        assert!(server.catches.get(snare).is_some() && server.catches.get(next).is_some(), "one snare of the two stood empty");
        assert!(ids.iter().all(|id| !server.creatures.live.contains_key(id)), "a rabbit ran through both snares");
    }

    /// A kept rabbit let go is followed from where it was let go: running through a snare
    /// before the traps' next look, it is caught there.
    #[test]
    fn a_rabbit_let_go_that_runs_through_a_snare_is_caught() {
        let (mut server, snare) = field(1);
        let id = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(-1.0, 0.0, 0.5));
        let keeper = server.players[&1].name.clone();
        let kept = server.creatures.live.get_mut(&id).unwrap();
        (kept.owner, kept.owner_id) = (Some(keeper), Some(1));
        server.spring_traps();
        server.release(1, id);
        server.release(1, id);
        assert!(server.creatures.live[&id].owner.is_none(), "let go");
        server.creatures.live.get_mut(&id).unwrap().body.position = snare.as_vec3() + Vec3::new(2.0, 0.0, 0.5);
        server.spring_traps();
        assert!(server.catches.get(snare).is_some(), "the rabbit let go ran through the snare uncaught");
    }
}
