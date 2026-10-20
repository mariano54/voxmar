//! Traps on the server (`traps.ron`, docs/traps.md): a wild creature of a kind a set trap
//! catches that steps into its cell is held by the trap's chance for it, else slips free and
//! bolts. Held, it is gone from the world, the trap shows sprung, and what it was is kept
//! with the trap until a player takes it: its loot, rolled for them as a kill of theirs, and
//! the trap's share of its kill experience. The trap is set again, unless that wore it out.
//! A wild creature too big for it, or that fights, may tear it as it steps in. Every roll is
//! the trap's own: by the world's draw key, the standing trap's serial and its count of
//! rolls. A better hunter does better (`Skilled`): the setter's Hunter level when it was
//! set decides its hold and tear, the taker's its wear.

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
    /// Each standing trap, set or sprung, by its cell (`Standing`), saved; gone with the
    /// trap. Its serial is the next of `serials`, so one set again in the same cell is a new
    /// trap that never draws the old one's rolls.
    standing: FxHashMap<IVec3, Standing>,
    serials: u64,
    /// Each wild creature that has stepped into a set trap and was neither held nor tore
    /// it, with that trap's cell: rolled for once, not again at every look while it stays.
    stepped: FxHashMap<u32, IVec3>,
}

/// What tells a trap's rolls from the world's other draws by the same key.
const TRAP_SALT: u64 = 0x5a4e_7a9b;

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

/// A standing trap: its serial, its rolls drawn so far (`trap_draw`), and the level of its
/// skill its setter had when they set it (0 for one no player set: the trap's own).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Standing {
    pub serial: u64,
    pub rolls: u64,
    pub level: u8,
}

/// A standing trap as `world.ron` keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SavedTrap {
    pub position: [i32; 3],
    pub standing: Standing,
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

    /// The standing traps, in order, and the serials given out, as `world.ron` keeps them.
    pub fn saved_traps(&self) -> (Vec<SavedTrap>, u64) {
        let mut saved: Vec<SavedTrap> = self.standing.iter().map(|(pos, &standing)| SavedTrap { position: pos.to_array(), standing }).collect();
        saved.sort();
        (saved, self.serials)
    }

    /// The saved catches whose species is still known, and the standing traps.
    pub fn restore(saved: &[SavedCatch], traps: &[SavedTrap], serials: u64, id: impl Fn(&str) -> Option<CreatureId>) -> Self {
        Self {
            caught: saved.iter().filter_map(|c| Some((IVec3::from_array(c.position), Catch { species: id(&c.species)?, seed: c.seed }))).collect(),
            standing: traps.iter().map(|t| (IVec3::from_array(t.position), t.standing)).collect(),
            serials,
            ..Self::default()
        }
    }

    /// The standing trap at `cell`, a new one if none stood there.
    fn standing(&mut self, cell: IVec3) -> &mut Standing {
        let serials = &mut self.serials;
        self.standing.entry(cell).or_insert_with(|| {
            *serials += 1;
            Standing { serial: *serials, ..Standing::default() }
        })
    }

    /// Rolls drawn by every standing trap.
    #[cfg(test)]
    pub fn rolls_drawn(&self) -> u64 {
        self.standing.values().map(|s| s.rolls).sum()
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
    /// The next roll of the trap at `cell`: by the world's draw key, the standing trap's
    /// serial and its count of rolls, never the tick or what else rolled first.
    pub(crate) fn trap_draw(&mut self, cell: IVec3) -> f32 {
        use voxmar_engine::creature::mix;
        let key = self.meta.draw_key ^ TRAP_SALT;
        let standing = self.catches.standing(cell);
        let draw = crate::save::numbered_draws(mix(key, standing.serial), TRAP_SALT, standing.rolls)[0];
        standing.rolls += 1;
        draw
    }

    /// The level of its skill the setter of the trap at `cell` had (0 for none).
    fn setter_level(&self, cell: IVec3) -> u8 {
        self.catches.standing.get(&cell).map_or(0, |s| s.level)
    }

    /// `agent` set a block at `cell`: a trap set there is theirs, as good as their level of
    /// its skill makes it.
    pub(crate) fn placed_trap(&mut self, agent: PlayerId, cell: IVec3) {
        let Some(trap) = self.block(cell).and_then(|b| self.content.traps.set(b)) else { return };
        let Some(player) = self.players.get(&agent) else { return };
        let level = self.content.skills.level(player.skills.xp(trap.skill));
        self.catches.standing(cell).level = level;
    }

    /// Each wild creature that stood in a set trap's cell, or crossed it since the last
    /// look, steps into it once: one the trap catches is held by its chance (none within
    /// `shy_reach` of what game keeps off), else slips free and bolts; one too big for it or
    /// that fights tears it by `tear`; any other passes over.
    pub(crate) fn spring_traps(&mut self) {
        let (traps, creatures) = (self.content.traps.clone(), self.content.creatures.clone());
        let seen = std::mem::take(&mut self.catches.seen);
        // Wild as `despawn_far` has it: no owner, no companion, no villager, and none a
        // village or a wanderer keeps (a herd, a hound, a flock).
        let (villagers, wandering) = (&self.villagers, &self.wandering);
        let wild = self.creatures.live.iter().filter(|&(id, c)| {
            c.owner.is_none() && !c.companion && c.health > 0 && creatures.get(c.species).def.kind != voxmar_engine::creature::Kind::Villager && !villagers.manages(*id) && !wandering.manages(*id)
        });
        // Every set trap each crossed that it would step into, in the order it crossed them.
        let crossings: Vec<(u32, Vec<IVec3>)> = wild
            .clone()
            .map(|(&id, c)| {
                let now = c.body.position;
                let set = crossed(seen.get(&id).copied().unwrap_or(now), now).into_iter().filter(|&cell| self.world.block(cell).and_then(|b| traps.set(b)).is_some()).collect();
                (id, set)
            })
            .filter(|(_, set): &(u32, Vec<_>)| !set.is_empty())
            .collect();
        self.catches.seen = wild.map(|(&id, c)| (id, c.body.position)).collect();
        for (id, set) in crossings {
            for cell in set {
                // Sprung or torn by one before it this look: on to the next it crossed.
                let Some(trap) = self.world.block(cell).and_then(|b| traps.set(b)) else { continue };
                if self.catches.stepped.get(&id) == Some(&cell) {
                    continue;
                }
                let Some((kind, seed)) = self.creatures.live.get(&id).map(|c| (c.species, c.seed)) else { break };
                let species = creatures.get(kind);
                let (torn, held) = (trap.torn_by(species), trap.catches(kind));
                if !torn && !held {
                    continue;
                }
                // Game keeps off fire: none steps into a trap near one, nor tears it. Looked
                // for once while it stays (`stepped`).
                self.catches.stepped.insert(id, cell);
                if self.fire_within(cell, &trap.shy_of, trap.shy_reach) {
                    continue;
                }
                let level = self.setter_level(cell);
                if torn {
                    if self.trap_draw(cell) < trap.tear_at(level) {
                        self.trap_gone(cell, trap);
                    }
                    continue;
                }
                if self.trap_draw(cell) >= trap.chance_at(kind, level) {
                    // Slipped it: off it bolts, frightened, though no player is near.
                    let live = self.creatures.live.get_mut(&id).expect("checked above");
                    let health = live.health as f32 / species.def.health.max(1) as f32;
                    live.mind.hurt(species, None, health);
                    live.mind.startled(cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
                    continue;
                }
                self.catches.stepped.remove(&id);
                let catch = Catch { species: kind, seed };
                self.creature_felled(id, None);
                self.set_block(cell, trap.sprung);
                self.catches.caught.insert(cell, catch);
                break;
            }
        }
        // One that stepped into a trap is rolled for again only once it has left that cell,
        // and not on its way out of it: forgotten only after the look that saw it leave.
        let live = &self.creatures.live;
        self.catches.stepped.retain(|id, cell| live.get(id).is_some_and(|c| c.body.position.floor().as_ivec3() == *cell));
    }

    /// The trap at `cell` is torn or worn out: gone, leaving what it leaves.
    fn trap_gone(&mut self, cell: IVec3, trap: &voxmar_engine::trap::Trap) {
        self.set_block(cell, BlockId::AIR);
        for (i, &item) in trap.leaves.iter().enumerate() {
            let body = dropped::popped(cell, self.meta.next_pop(cell, 0x7ea2 + i as u32));
            self.items.spawn(ItemStack { item, count: 1 }, body, dropped::PICKUP_DELAY);
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
        // Taken, it is set again, or wears out by `wear` (less for a better hunter).
        if self.trap_draw(pos) < trap.wear_at(level) {
            self.trap_gone(pos, trap);
        } else {
            self.set_block(pos, trap.set);
        }
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
        // The trap's share of the kill's experience.
        let kill = species.xp_skill.map(|skill| (skill, species.def.xp.kill * trap.xp));
        let mut journal: Vec<(voxmar_engine::journal::Page, String, u32)> = page.map(|page| (page, name, 1)).into_iter().collect();
        for (i, (item, count)) in found.into_iter().enumerate() {
            journal.push((voxmar_engine::journal::Page::Collection, self.content.blocks.def(item).name.clone(), count as u32));
            let Some(player) = self.players.get_mut(&agent) else { break };
            let left = player.inventory.add(item, count);
            player.changed(traits::INVENTORY);
            if left > 0 {
                let body = dropped::popped(pos, self.meta.next_pop(pos, 0x5a4e + i as u32));
                self.items.spawn(ItemStack { item, count: left }, body, dropped::PICKUP_DELAY);
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

    /// A block changed: a sprung trap broken or replaced loses its catch; a trap set where
    /// none stood is a new standing trap, kept while it is set or sprung and gone with it.
    pub(crate) fn trap_changed(&mut self, pos: IVec3, block: BlockId) {
        let traps = &self.content.traps;
        if traps.sprung(block).is_none() {
            self.catches.caught.remove(&pos);
        }
        if traps.set(block).is_some() || traps.sprung(block).is_some() {
            self.catches.standing(pos);
        } else {
            self.catches.standing.remove(&pos);
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
    /// The snare is a sure one (every chance 1, never torn or worn out), so the tests on it
    /// are of where, when and what it catches; how sure the game's own is, `snare_as_it_is`.
    fn field(hunter: u8) -> (Server, IVec3) {
        let (mut server, snare) = snare_as_it_is(hunter);
        let mut traps = (*server.content.traps).clone();
        for trap in &mut traps.traps {
            trap.chances.iter_mut().for_each(|c| *c = 1.0);
            (trap.tear, trap.wear) = (0.0, 0.0);
        }
        server.content.traps = Arc::new(traps);
        (server, snare)
    }

    /// The field, with the game's own snare, and the world's draw key fixed.
    fn snare_as_it_is(hunter: u8) -> (Server, IVec3) {
        let mut server = crate::test_server();
        server.meta.draw_key = 0x0d1a;
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

    fn snare_trap(server: &Server) -> &voxmar_engine::trap::Trap {
        server.content.traps.set(server.content.blocks.expect("base:snare")).expect("the snare is a trap")
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
        let (traps, serials) = server.catches.saved_traps();
        let restored = Catches::restore(&saved, &traps, serials, |name| server.content.creatures.id(name));
        assert_eq!(restored.saved_traps(), (traps, serials), "the standing traps and their rolls kept");
        assert!(restored.rolls_drawn() > 0, "the catch drew a roll");
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
        assert!(Catches::restore(&[], &[], 0, |_| None).seen.is_empty(), "a loaded world remembers no last look");
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

    fn gone(server: &Server, pos: IVec3) -> bool {
        server.block(pos).is_none_or(|b| b.is_air())
    }

    fn sticks_lying(server: &Server) -> u32 {
        server.items.saved(|item| server.content.blocks.def(item).name.clone()).iter().filter(|i| i.item == "base:stick").map(|i| i.count as u32).sum()
    }

    /// The game's snare holds a rabbit by its chance: of many stepping in, about that share
    /// are caught. One that slips it bolts, frightened, and is not rolled for again while it
    /// stays in the snare's cell.
    #[test]
    fn a_snare_holds_a_rabbit_by_its_chance_and_one_that_slips_it_bolts() {
        let (mut server, snare) = snare_as_it_is(1);
        let chance = snare_trap(&server).chance(server.content.creatures.id("base:rabbit").unwrap());
        let (mut caught, tries) = (0, 200);
        for _ in 0..tries {
            // Set again where taking wore it out.
            if gone(&server, snare) {
                server.set_block(snare, server.content.blocks.expect("base:snare"));
            }
            let rabbit = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            server.spring_traps();
            if server.catches.get(snare).is_some() {
                caught += 1;
                server.take_from_trap(1, snare);
                continue;
            }
            assert!(server.creatures.live[&rabbit].mind.fear > 0.9, "a rabbit that slipped the snare is not frightened");
            let rolls = server.catches.rolls_drawn();
            server.spring_traps();
            assert_eq!(server.catches.rolls_drawn(), rolls, "a rabbit still in the snare was rolled for again");
            server.creatures.live.get_mut(&rabbit).unwrap().body.position = snare.as_vec3() + Vec3::new(2.5, 0.0, 0.5);
            server.spring_traps();
            assert_eq!(server.catches.rolls_drawn(), rolls, "a rabbit walking out of the snare it slipped was rolled for again");
            assert!(server.catches.get(snare).is_none());
            server.creature_felled(rabbit, None);
        }
        let share = caught as f32 / tries as f32;
        assert!((share - chance).abs() < 0.1, "{caught} of {tries} rabbits held by a snare that holds {chance}");
    }

    /// Anything too big for the snare, or that fights, tears it by its `tear` as it steps in
    /// (a deer, a fox), leaving a stick, and is rolled for once while it stays; a rabbit
    /// never tears it.
    #[test]
    fn a_deer_or_a_fox_tears_a_snare_by_its_chance_and_a_rabbit_never() {
        for name in ["base:red_deer", "base:fox"] {
            let (mut server, snare) = snare_as_it_is(1);
            let tear = snare_trap(&server).tear;
            let (mut torn, tries) = (0, 120);
            for _ in 0..tries {
                server.set_block(snare, server.content.blocks.expect("base:snare"));
                let id = put(&mut server, name, snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
                server.spring_traps();
                server.spring_traps();
                if named(&server, snare) != "base:snare" {
                    assert!(gone(&server, snare), "a {name} left the snare as {}", named(&server, snare));
                    torn += 1;
                }
                assert!(server.creatures.live.contains_key(&id), "a {name} was caught");
                server.creature_felled(id, None);
            }
            assert_eq!(sticks_lying(&server), torn, "each torn snare leaves a stick");
            let share = torn as f32 / tries as f32;
            assert!((share - tear).abs() < 0.12, "a {name} tore {torn} of {tries} snares, the snare's tear is {tear}");
        }
    }

    /// Only wild game steps into a snare: a companion, a villager, one a player owns, and a
    /// deer a village or a wanderer keeps never spring or tear one, nor are rolled for.
    #[test]
    fn a_companion_or_a_villager_never_springs_or_tears_a_snare() {
        let (mut server, snare) = snare_as_it_is(1);
        let at = snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        let companion = put(&mut server, "base:red_deer", at);
        server.creatures.live.get_mut(&companion).unwrap().companion = true;
        let villager = put(&mut server, "base:villager", at);
        let owned = put(&mut server, "base:red_deer", at);
        server.creatures.live.get_mut(&owned).unwrap().owner = Some("keeper".into());
        let herded = put(&mut server, "base:red_deer", at);
        server.villagers.keep_for_test(herded);
        let wandering = put(&mut server, "base:red_deer", at);
        server.wandering.keep_for_test(wandering);
        let all = [companion, villager, owned, herded, wandering];
        for _ in 0..20 {
            server.spring_traps();
            for id in all {
                server.creatures.live.get_mut(&id).unwrap().body.position = snare.as_vec3() + Vec3::new(-1.5, 0.0, 0.5);
            }
            server.spring_traps();
            for id in all {
                server.creatures.live.get_mut(&id).unwrap().body.position = at;
            }
        }
        assert_eq!(named(&server, snare), "base:snare", "a kept creature sprang or tore the snare");
        assert_eq!(server.catches.rolls_drawn(), 0, "a kept creature was rolled for");
        // A wild deer walked in the same way is rolled for: it tore the snare, or a roll
        // stands drawn.
        put(&mut server, "base:red_deer", at);
        server.spring_traps();
        assert!(gone(&server, snare) || server.catches.rolls_drawn() == 1, "a wild deer was not rolled for");
    }

    /// A rabbit that slips a snare runs from it with no player near, as kin raise the alarm,
    /// rather than graze on and walk back into it.
    #[test]
    fn a_rabbit_that_slips_a_snare_runs_from_it_with_no_one_near() {
        let off = slipped_runs(false);
        assert!(off > 4.0, "a rabbit that slipped the snare is at most {off:.1} blocks from it in 3 s");
    }

    /// Kin it alarmed as it bolted alarm it back as they run: it still runs from the snare,
    /// not toward its home by the snare.
    #[test]
    fn a_rabbit_that_slips_a_snare_runs_from_it_while_kin_raise_the_alarm() {
        let off = slipped_runs(true);
        assert!(off > 4.0, "a slipped rabbit its kin alarmed is at most {off:.1} blocks from the snare in 3 s");
    }

    /// The furthest a rabbit that slipped the snare gets from it in 3 s, with no player near;
    /// `kin` alarms it again every tick, as kin running beside it would.
    fn slipped_runs(kin: bool) -> f32 {
        let (mut server, snare) = snare_as_it_is(1);
        server.players.get_mut(&1).unwrap().position = Vec3::new(31.5, FLOOR as f32 + 1.0, 31.5);
        let at = snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        let rabbit = loop {
            if gone(&server, snare) {
                server.set_block(snare, server.content.blocks.expect("base:snare"));
            }
            let rabbit = put(&mut server, "base:rabbit", at);
            server.spring_traps();
            if server.catches.get(snare).is_none() {
                break rabbit;
            }
            server.take_from_trap(1, snare);
        };
        // The test world is one column, so a rabbit that runs off its edge is gone: count
        // the furthest it got while it was here.
        let mut off = 0.0_f32;
        for _ in 0..3 * TICKS_PER_SECOND {
            if kin && let Some(c) = server.creatures.live.get_mut(&rabbit) {
                c.mind.alarmed();
            }
            server.tick();
            if let Some(c) = server.creatures.live.get(&rabbit) {
                off = off.max(c.body.position.with_y(0.0).distance(at.with_y(0.0)));
            }
        }
        off
    }

    /// A better hunter's snare holds more and is torn less, by its setter's Hunter level when
    /// it was set (`skilled`); a snare no player set does as its trap says.
    #[test]
    fn a_better_hunters_snare_holds_more_and_is_torn_less() {
        let share = |hunter: u8, name: &str| {
            let (mut server, snare) = snare_as_it_is(hunter);
            server.placed_trap(1, snare);
            let (mut hits, tries) = (0, 240);
            for _ in 0..tries {
                if named(&server, snare) != "base:snare" {
                    server.set_block(snare, server.content.blocks.expect("base:snare"));
                    server.placed_trap(1, snare);
                }
                let id = put(&mut server, name, snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
                server.spring_traps();
                if server.catches.get(snare).is_some() || gone(&server, snare) {
                    hits += 1;
                }
                if server.catches.get(snare).is_some() {
                    server.take_from_trap(1, snare);
                } else {
                    server.creature_felled(id, None);
                }
            }
            hits as f32 / tries as f32
        };
        let (server, _) = snare_as_it_is(1);
        let trap = snare_trap(&server).clone();
        let rabbit = server.content.creatures.id("base:rabbit").unwrap();
        let best = trap.level + trap.skilled.levels;
        for (level, held, torn) in [(1, trap.chance_at(rabbit, 1), trap.tear_at(1)), (best, trap.chance_at(rabbit, best), trap.tear_at(best))] {
            let (h, t) = (share(level, "base:rabbit"), share(level, "base:red_deer"));
            assert!((h - held).abs() < 0.1, "at Hunter {level} {h:.2} of rabbits held, the snare says {held:.2}");
            assert!((t - torn).abs() < 0.1, "at Hunter {level} {t:.2} of snares torn by a deer, the snare says {torn:.2}");
        }
    }

    /// Taking a catch wears the snare out by its `wear`: over many snares, it lasts about
    /// as many catches as one in `wear`, and worn out it is gone, leaving a stick. A catch
    /// gives the snare's share of the kill's experience.
    #[test]
    fn a_snare_wears_out_by_its_wear_and_a_catch_gives_its_share_of_a_kill() {
        let (mut server, snare) = snare_as_it_is(1);
        let trap = snare_trap(&server).clone();
        let rabbit = server.content.creatures.get(server.content.creatures.id("base:rabbit").unwrap());
        let kill = rabbit.def.xp.kill;
        let (mut taken, mut worn, mut xp) = (0u32, 0u32, 0u32);
        while worn < 40 {
            assert!(taken < 1000, "{taken} catches taken and {worn} snares worn out");
            if named(&server, snare) != "base:snare" {
                server.set_block(snare, server.content.blocks.expect("base:snare"));
            }
            let id = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            server.spring_traps();
            if server.catches.get(snare).is_none() {
                server.creature_felled(id, None);
                continue;
            }
            let before = hunter_xp(&server);
            server.take_from_trap(1, snare);
            xp += hunter_xp(&server) - before;
            taken += 1;
            if gone(&server, snare) {
                worn += 1;
            }
        }
        assert_eq!(sticks_lying(&server), worn, "each worn-out snare leaves a stick");
        let lasts = taken as f32 / worn as f32;
        assert!((lasts * trap.wear - 1.0).abs() < 0.35, "a snare lasted {lasts:.1} catches, its wear {} says {:.1}", trap.wear, 1.0 / trap.wear);
        // Experience in tenths: the share of a kill, a tenth either way for rounding.
        let per = xp as f32 / taken as f32 / 10.0;
        assert!((per - kill * trap.xp).abs() <= 0.1, "a catch gave {per:.2} experience, the snare's share {} of a kill's {kill}", trap.xp);
    }

    /// No game is caught by a fire: within the snare's `shy_reach` of a campfire a rabbit
    /// stepping in is neither held nor rolled for; further off, it is.
    #[test]
    fn no_game_is_caught_by_a_fire() {
        for (off, near) in [(3, true), (8, false)] {
            let (mut server, snare) = field(1);
            server.set_block(snare + IVec3::new(off, 0, 0), server.content.blocks.expect("base:campfire"));
            put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            server.spring_traps();
            assert_eq!(server.catches.get(snare).is_none(), near, "a campfire {off} blocks off");
            assert_eq!(server.catches.rolls_drawn() == 0, near, "a campfire {off} blocks off: {} rolls drawn", server.catches.rolls_drawn());
        }
    }

    /// A snare's rolls are its own: by the world's draw key, its cell and its count there,
    /// the same whatever tick they fall on and whatever the world rolled first; a snare set
    /// again in the same cell goes on from its count.
    #[test]
    fn a_snares_rolls_go_by_its_cell_and_count_not_the_tick() {
        let run = |tick: u64, others: usize| {
            let (mut server, snare) = snare_as_it_is(1);
            server.tick = tick;
            let mut outcomes = Vec::new();
            for _ in 0..24 {
                for _ in 0..others {
                    server.meta.next_roll(crate::save::ROLL_WANDER);
                }
                if named(&server, snare) != "base:snare" {
                    server.set_block(snare, server.content.blocks.expect("base:snare"));
                }
                let id = put(&mut server, "base:rabbit", snare.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
                server.spring_traps();
                let caught = server.catches.get(snare).is_some();
                server.take_from_trap(1, snare);
                server.creature_felled(id, None);
                outcomes.push((caught, named(&server, snare)));
            }
            (outcomes, server.catches.saved_traps())
        };
        let first = run(1_000, 0);
        assert!(first.0.iter().any(|o| o.0) && first.0.iter().any(|o| !o.0), "24 rabbits were all held or all slipped: {:?}", first.0);
        assert_eq!(run(1_377, 3), first, "the same rabbits later, after other rolls, came out otherwise");
        // Set again, it is a new snare: its own serial and no rolls drawn, and none is kept
        // for a cell no trap stands in.
        let (mut server, snare) = snare_as_it_is(1);
        server.trap_draw(snare);
        let (before, _) = server.catches.saved_traps();
        server.set_block(snare, BlockId::AIR);
        assert!(server.catches.saved_traps().0.is_empty(), "a snare gone keeps no standing");
        server.set_block(snare, server.content.blocks.expect("base:snare"));
        let (after, _) = server.catches.saved_traps();
        assert_eq!((after.len(), after[0].standing.rolls), (1, 0));
        assert_ne!(after[0].standing.serial, before[0].standing.serial, "set again, the snare drew the old one's rolls");
    }
}
