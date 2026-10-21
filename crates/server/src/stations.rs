//! Home stations on the server (`stations.ron`, docs/stations.md): a load put on a drying
//! rack or a compost heap, done at a tick on the world's clock whether anyone is near or
//! not, and taken off again; and fertilisers spread on growing crops. A look every few
//! seconds turns a done station in a loaded column to its ready block, so it shows; one in a
//! column nobody has loaded shows it when it is next used.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::dropped;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::Words;
use voxmar_engine::schedule::Tier;
use voxmar_engine::station::{Phase, Spot, Station};
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::process::Process;

/// What is on a station: `count` of `item`, done at tick `due`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Load {
    pub item: BlockId,
    pub count: u8,
    pub due: u64,
}

/// Every loaded station, by where it stands.
#[derive(Default)]
pub struct Loads {
    loads: FxHashMap<IVec3, Load>,
}

/// A load as `world.ron` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLoad {
    pub position: [i32; 3],
    pub item: String,
    pub count: u8,
    pub due: u64,
}

impl Loads {
    pub fn saved(&self, name: impl Fn(BlockId) -> String) -> Vec<SavedLoad> {
        let mut saved: Vec<SavedLoad> = self.loads.iter().map(|(pos, l)| SavedLoad { position: pos.to_array(), item: name(l.item), count: l.count, due: l.due }).collect();
        saved.sort_by_key(|l| l.position);
        saved
    }

    /// The saved loads whose item is still known.
    pub fn restore(saved: &[SavedLoad], id: impl Fn(&str) -> Option<BlockId>) -> Self {
        Self { loads: saved.iter().filter_map(|l| Some((IVec3::from_array(l.position), Load { item: id(&l.item)?, count: l.count, due: l.due }))).collect() }
    }

    pub fn get(&self, pos: IVec3) -> Option<Load> {
        self.loads.get(&pos).copied()
    }
}

/// How far from a player's eye a station they ask about may stand, in blocks: a little past
/// reach, so the readout does not flicker at its edge.
const SEEN_FROM: f32 = 8.0;

/// How often the look at loaded stations comes, in seconds: often enough that a pit kiln's
/// twelve-second load shows fired within a second of being done.
const LOOK_SECONDS: u64 = 1;

/// `base:stations`: turns done stations in loaded columns to their ready blocks.
pub(crate) struct Stations;

impl Process for Stations {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        if server.tick % (LOOK_SECONDS * TICKS_PER_SECOND as u64) != 0 {
            return;
        }
        server.show_done_stations();
    }
}

impl Server {
    fn tell_station(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
    }

    /// Whether `spot` holds at the station at `pos`.
    fn spot_holds(&self, pos: IVec3, spot: &Spot) -> bool {
        match spot {
            Spot::OpenSky => self.open_sky(pos + IVec3::Y),
            Spot::Near(tag, reach) => self.tagged_within(pos, tag, *reach),
            Spot::Water(reach) => {
                let blocks = &self.content.blocks;
                // A block that waters (a well) is water too.
                let water = |b: BlockId| (blocks.info(b).fluid.is_some() && !blocks.def(b).has_tag("hot")) || blocks.def(b).has_tag("waters");
                let r = *reach;
                (-r..=r).any(|dy| (-r..=r).any(|dz| (-r..=r).any(|dx| self.block(pos + IVec3::new(dx, dy, dz)).is_some_and(water))))
            }
        }
    }

    /// The first of a station's needs the spot at `pos` lacks, as words to say.
    fn lacks(&self, pos: IVec3, station: &Station) -> Option<Words> {
        station.needs.iter().find(|spot| !self.spot_holds(pos, spot)).map(|spot| match spot {
            Spot::OpenSky => Words::key("station.needs.sky"),
            Spot::Near(..) => Words::key("station.needs.spot"),
            Spot::Water(reach) => Words::key("station.needs.water").with("reach", Words::text(*reach)),
        })
    }

    /// The words for the time left on a load due at `due`: `minutes`, rounded up, or under
    /// a minute `seconds`, so a pit kiln's twelve seconds is not "about 1 min".
    fn time_left(&self, [minutes, seconds]: [&str; 2], due: u64) -> Words {
        let ticks = due.saturating_sub(self.tick);
        if ticks < 60 * TICKS_PER_SECOND as u64 {
            Words::key(seconds).with("seconds", Words::text(ticks.div_ceil(TICKS_PER_SECOND as u64).max(1)))
        } else {
            Words::key(minutes).with("minutes", Words::text(ticks.div_ceil(60 * TICKS_PER_SECOND as u64)))
        }
    }

    /// What is on the station at `pos` as `agent` sees it (`ClientMessage::LookAtStation`):
    /// None, not answered, if it is no station or too far off to see; else its load, if any,
    /// and, for a fuelled structure's core (a kiln), the seconds its fire burns on.
    pub(crate) fn station_seen(&self, agent: PlayerId, pos: IVec3) -> Option<(Option<voxmar_protocol::StationLoad>, Option<u32>)> {
        let player = self.players.get(&agent)?;
        if player.eye().distance(pos.as_vec3() + Vec3::splat(0.5)) > SEEN_FROM {
            return None;
        }
        let block = self.block(pos)?;
        let seconds = |due: u64| due.saturating_sub(self.tick).div_ceil(TICKS_PER_SECOND as u64).min(u32::MAX as u64) as u32;
        if let Some((station, _)) = self.content.stations.of(block) {
            let load = self.stations.get(pos).and_then(|load| {
                let (_, work) = station.work(load.item)?;
                Some(voxmar_protocol::StationLoad { item: load.item, count: load.count, output: work.output, makes: load.count / work.takes * work.gives, seconds: seconds(load.due) })
            });
            return Some((load, None));
        }
        if let Some(load) = self.smoulder_seen(pos) {
            return Some((Some(load), None));
        }
        let fuelled = self.content.multiblocks.with_core(block).any(|m| !m.fuels.is_empty());
        fuelled.then(|| (None, Some(if self.station_lit(pos) { self.lit.get(&pos).map_or(0, |fire| seconds(fire.until)) } else { 0 })))
    }

    /// Answers `agent`'s look at the station at `pos`, when it is one they can see.
    pub(crate) fn look_at_station(&self, agent: PlayerId, pos: IVec3) {
        if let (Some((load, fire)), Some(player)) = (self.station_seen(agent, pos), self.players.get(&agent)) {
            player.connection.send(ServerMessage::Station { block: pos, load, fire });
        }
    }

    /// `agent` uses the station at `pos` (`base:use_station`): loads it with what is in
    /// hand, takes what it made, or is told how it goes.
    pub(crate) fn use_station(&mut self, agent: PlayerId, pos: IVec3) {
        let stations = self.content.stations.clone();
        let Some(block) = self.block(pos) else { return };
        let Some((station, phase)) = stations.of(block) else { return };
        match phase {
            Phase::Empty => self.load_station(agent, pos, station),
            Phase::Working => match self.stations.get(pos) {
                Some(load) if self.tick >= load.due => self.take_from_station(agent, pos, station, load),
                Some(load) => {
                    let words = self.time_left(["station.working", "station.working_seconds"], load.due).with("item", Words::Block(self.content.blocks.def(load.item).name.clone()));
                    self.tell_station(agent, words);
                }
                // Working with nothing on it (a command set it so): it is empty.
                None => self.set_block(pos, station.empty),
            },
            Phase::Ready => match self.stations.get(pos) {
                Some(load) => self.take_from_station(agent, pos, station, load),
                None => self.set_block(pos, station.empty),
            },
        }
    }

    fn load_station(&mut self, agent: PlayerId, pos: IVec3, station: &Station) {
        let Some(player) = self.players.get(&agent) else { return };
        let selected = player.selected;
        let Some(stack) = player.inventory.get(selected) else { return };
        let Some((_, work)) = station.work(stack.item) else { return };
        if self.check_level(agent, work.skill) < work.level {
            let skill = Words::key(&voxmar_engine::lang::def_key("skill", &self.content.skills.get(work.skill).name));
            self.refuse(agent, Words::key("station.level").with("skill", skill).with("level", Words::text(work.level)));
            return;
        }
        // Its needs and what speeds it are judged on the land around it, all of it loaded.
        let reach = station.needs.iter().chain(station.faster.iter().map(|(spot, _)| spot)).map(|spot| match spot {
            Spot::OpenSky => 0,
            Spot::Near(_, reach) | Spot::Water(reach) => *reach,
        });
        if !self.land_loaded(pos, reach.max().unwrap_or(0)) {
            self.refuse(agent, Words::key("station.unloaded"));
            return;
        }
        if let Some(lacks) = self.lacks(pos, station) {
            self.refuse(agent, lacks);
            return;
        }
        // Loaded in the loader's home, the rest of the pack and then the home's stores top
        // the load up with more of what is in hand (docs/home.md).
        let topped = station.from_stores && self.in_home_with(agent, pos);
        let stores = if topped { self.home_stores(agent) } else { Vec::new() };
        let player = &self.players[&agent];
        let packed: u32 = if !topped { 0 } else { player.inventory.slots.iter().enumerate().filter(|&(slot, _)| slot != selected).filter_map(|(_, s)| s.filter(|s| s.item == stack.item)).map(|s| s.count as u32).sum() };
        let stored: u32 = stores.iter().filter_map(|&at| self.factory.get(at)).flat_map(|(_, state)| state.bin.iter().flatten()).filter(|s| s.item == stack.item).map(|s| s.count as u32).sum();
        let within = (stack.count as u32 + packed + stored).min(station.holds as u32) as u8;
        // Whole batches only: the heap rots four greens to one compost, so a load is a
        // multiple of four.
        let count = within / work.takes * work.takes;
        if count == 0 {
            self.refuse(agent, Words::key("station.too_few").with("count", Words::text(work.takes)).with("item", Words::Block(self.content.blocks.def(stack.item).name.clone())));
            return;
        }
        let speed: f32 = station.faster.iter().filter(|(spot, _)| self.spot_holds(pos, spot)).map(|(_, times)| times).product();
        let ticks = (work.minutes * 60.0 * TICKS_PER_SECOND as f32 / speed).round() as u64;
        let player = self.players.get_mut(&agent).expect("checked above");
        let mut left = count - player.inventory.take(selected, count).map_or(0, |taken| taken.count);
        if topped {
            for slot in (0..player.inventory.slots.len()).filter(|&slot| slot != selected) {
                if left > 0 && player.inventory.slots[slot].is_some_and(|s| s.item == stack.item) {
                    left -= player.inventory.take(slot, left).map_or(0, |taken| taken.count);
                }
            }
        }
        player.changed(traits::INVENTORY);
        let from_stores = self.take_from_stores(&stores, stack.item, left as u32);
        debug_assert_eq!(from_stores, left as u32, "counted as there");
        // A store the loader has open shows what was taken with the same update.
        if from_stores > 0 {
            self.pull_store(agent);
        }
        self.stations.loads.insert(pos, Load { item: stack.item, count, due: self.tick + ticks.max(1) });
        self.set_block(pos, station.working);
        let mut words = self.time_left(["station.loaded", "station.loaded_seconds"], self.tick + ticks.max(1)).with("count", Words::text(count)).with("item", Words::Block(self.content.blocks.def(stack.item).name.clone()));
        if speed > 1.0 {
            words = Words::key("station.loaded_faster").with("words", words);
        }
        if from_stores > 0 {
            words = Words::key("station.loaded_from_stores").with("words", words).with("count", Words::text(from_stores));
        }
        self.tell_station(agent, words);
    }

    fn take_from_station(&mut self, agent: PlayerId, pos: IVec3, station: &Station, load: Load) {
        if let Some(lacks) = self.lacks(pos, station) {
            self.tell_station(agent, Words::key("station.spoilt").with("why", lacks));
            return;
        }
        let Some((_, work)) = station.work(load.item) else { return };
        let made = load.count / work.takes * work.gives;
        self.stations.loads.remove(&pos);
        self.set_block(pos, station.empty);
        let Some(player) = self.players.get_mut(&agent) else { return };
        let left = player.inventory.add(work.output, made);
        player.changed(traits::INVENTORY);
        if left > 0 {
            let body = dropped::popped(pos, self.meta.next_pop(pos, 0x57a7));
            self.items.spawn(ItemStack::new(work.output, left), body, dropped::PICKUP_DELAY);
        }
        self.grant_xp(agent, work.skill, work.xp * made as f32);
        if let Some(kind) = &work.counts {
            let output = self.content.blocks.def(work.output).name.clone();
            self.tally(agent, kind, &output, made as u32);
            // Cooked at this station too, by its empty block's name, for a quest's `CookAt`.
            if kind == "cooked" {
                let at = self.content.blocks.def(station.empty).name.rsplit(':').next().unwrap_or_default().to_owned();
                self.tally(agent, &format!("cooked_at:{at}"), &output, made as u32);
            }
        }
        let words = Words::key("station.took").with("count", Words::text(made)).with("item", Words::Block(self.content.blocks.def(work.output).name.clone()));
        self.tell_station(agent, words);
    }

    /// A block changed: a loaded station that is no longer one of its own station's blocks
    /// (broken, burnt, replaced) lets what was on it fall, done or not.
    pub(crate) fn station_changed(&mut self, pos: IVec3, old: BlockId, block: BlockId) {
        if old == block {
            return;
        }
        let Some(load) = self.stations.get(pos) else { return };
        let stations = self.content.stations.clone();
        let Some((station, _)) = stations.of(old) else { return };
        if [station.empty, station.working, station.ready].contains(&block) && block != station.empty {
            return;
        }
        self.stations.loads.remove(&pos);
        if block == station.empty {
            return;
        }
        let stack = match station.work(load.item) {
            Some((_, work)) if self.tick >= load.due => ItemStack::new(work.output, load.count / work.takes * work.gives),
            _ => ItemStack::new(load.item, load.count),
        };
        let body = dropped::popped(pos, self.meta.next_pop(pos, 0x57a8));
        self.items.spawn(stack, body, dropped::PICKUP_DELAY);
    }

    /// Done stations in loaded columns show it. A load on what is no station now (an older
    /// world's, on a station since retired: the earth clamp) falls out as it went in.
    pub(crate) fn show_done_stations(&mut self) {
        let stations = self.content.stations.clone();
        let mut done: Vec<(IVec3, BlockId)> = Vec::new();
        let mut stationless: Vec<IVec3> = Vec::new();
        for (&pos, load) in &self.stations.loads {
            let Some(block) = self.block(pos) else { continue };
            match stations.of(block) {
                None => stationless.push(pos),
                Some((station, Phase::Working)) if self.tick >= load.due => done.push((pos, station.ready)),
                Some(_) => {}
            }
        }
        done.sort_by_key(|(pos, _)| pos.to_array());
        for (pos, ready) in done {
            self.set_block(pos, ready);
        }
        stationless.sort_by_key(|pos| pos.to_array());
        for pos in stationless {
            let Some(load) = self.stations.loads.remove(&pos) else { continue };
            let body = dropped::popped(pos, self.meta.next_pop(pos, 0x57a9));
            self.items.spawn(ItemStack::new(load.item, load.count), body, dropped::PICKUP_DELAY);
        }
    }

    /// `agent` spreads one fertiliser in hand on the crop at `pos` (`base:fertilise`): the
    /// sown crops within its reach grow; with none to grow, it stays in hand.
    pub(crate) fn fertilise(&mut self, agent: PlayerId, pos: IVec3) {
        let stations = self.content.stations.clone();
        let Some(player) = self.players.get(&agent) else { return };
        let selected = player.selected;
        let Some(fertiliser) = player.held().and_then(|item| stations.fertiliser(item)) else { return };
        let grown = self.grow_around(pos.as_vec3() + Vec3::splat(0.5), fertiliser.reach, fertiliser.stages);
        if grown == 0 {
            self.refuse(agent, Words::key("station.nothing_grows"));
            return;
        }
        let player = self.players.get_mut(&agent).expect("checked above");
        player.inventory.take(selected, 1);
        player.changed(traits::INVENTORY);
        self.grant_xp(agent, fertiliser.skill, fertiliser.xp * grown as f32);
        self.tally(agent, "fertilised", &self.content.blocks.def(fertiliser.item).name.clone(), grown as u32);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::world::Column;

    use super::*;

    const AT: IVec3 = IVec3::new(40, 200, 40);
    const MINUTE: u64 = 60 * TICKS_PER_SECOND as u64;

    /// A flat stone floor at y 199 under open sky, and player 1 beside `AT` with
    /// Farming `farming`.
    fn fixture(farming: u8) -> Server {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = server.content.blocks.expect("base:stone");
        for cx in 0..3 {
            for cz in 0..3 {
                let mut column = Column::default();
                for x in 0..32 {
                    for z in 0..32 {
                        column.set_block(IVec3::new(x, 199, z), stone);
                    }
                }
                server.world.insert(ColumnPos::new(cx, cz), Arc::new(column));
            }
        }
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = AT.as_vec3() + Vec3::new(2.5, 0.0, 0.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        let skill = server.content.skills.id("core:farming").unwrap();
        player.skills.grant(skill, server.content.skills.xp_for(farming));
        server.players.insert(1, player);
        server
    }

    fn block(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    fn hold(server: &mut Server, name: &str, count: u8) {
        let item = block(server, name);
        let player = server.players.get_mut(&1).unwrap();
        player.inventory = Default::default();
        player.inventory.add(item, count);
        player.selected = 0;
    }

    /// How many of `name` player 1 carries.
    fn carried(server: &Server, name: &str) -> u32 {
        let item = block(server, name);
        server.players[&1].inventory.slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum()
    }

    /// How many of `name` lie loose in the world.
    fn loose(server: &Server, name: &str) -> u32 {
        server.items.saved(|b| server.content.blocks.def(b).name.clone()).iter().filter(|i| i.item == name).map(|i| i.count as u32).sum()
    }

    fn at(server: &Server, pos: IVec3) -> String {
        server.content.blocks.def(server.block(pos).unwrap()).name.clone()
    }

    fn xp(server: &Server, skill: &str) -> u32 {
        server.players[&1].skills.xp(server.content.skills.id(skill).unwrap())
    }

    /// A station is judged on the land around it: while a column within its needs' reach is
    /// still loading, using it is refused with a line saying so and takes nothing; once
    /// loaded, it works as before.
    #[test]
    fn a_station_by_land_still_loading_says_so_and_takes_nothing() {
        let mut server = fixture(1);
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        // Three blocks from the column to the west: in reach of a fire that speeds it.
        let rack = IVec3::new(34, 200, 40);
        server.set_block(rack, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_beef", 8);
        let west = ColumnPos::new(0, 1);
        let held = server.world.remove(west).expect("loaded");
        server.use_station(1, rack);
        let refused: Vec<Words> = std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Refused { why } => Some(why), _ => None }).collect();
        assert_eq!(refused, [Words::key("station.unloaded")]);
        assert_eq!((at(&server, rack).as_str(), carried(&server, "base:raw_beef")), ("base:drying_rack", 8), "loaded on land still loading");
        server.world.insert(west, held);
        server.use_station(1, rack);
        assert_eq!(at(&server, rack), "base:drying_rack_hung");
    }

    /// The rack: raw meat in hand loads it (up to eight), it hangs while the clock runs,
    /// the look turns it to the dry rack when due, and using it then gives the jerky and
    /// Cooking experience.
    #[test]
    fn meat_hung_on_a_rack_is_jerky_ten_minutes_later() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_beef", 10);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:drying_rack_hung");
        assert_eq!(carried(&server, "base:raw_beef"), 2, "eight hang, two stay in hand");
        server.tick += 10 * MINUTE - 1;
        server.show_done_stations();
        assert_eq!(at(&server, AT), "base:drying_rack_hung", "not yet");
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:jerky"), 0, "nothing to take before it is done");
        server.tick += 1;
        server.show_done_stations();
        assert_eq!(at(&server, AT), "base:drying_rack_dry");
        let cooking = xp(&server, "core:cooking");
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:jerky"), 8);
        assert_eq!(at(&server, AT), "base:drying_rack");
        assert!(xp(&server, "core:cooking") > cooking, "taking pays Cooking");
        assert_eq!(server.stations.get(AT), None);
    }

    /// The pit kiln: clay in hand loads it (up to twelve) and it fires with nobody by it,
    /// twelve seconds; taken, all twelve are fired clay (none cracks), pay Crafting and
    /// count as cooked. Cobblestone fires to stone the same way, and a held log loads nothing.
    #[test]
    fn clay_left_in_a_pit_kiln_is_fired_twelve_seconds_later() {
        let mut server = fixture(1);
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        server.set_block(AT, block(&server, "base:pit_kiln"));
        hold(&mut server, "base:clay", 16);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:pit_kiln_firing");
        // Said in seconds, not "about 1 min".
        let told: Vec<String> = std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Chat { text, .. } => Some(format!("{text:?}")), _ => None }).collect();
        assert!(told.iter().any(|t| t.contains("station.loaded_seconds") && t.contains("(\"seconds\", Text(\"12\"))")), "{told:?}");
        assert_eq!(carried(&server, "base:clay"), 4, "twelve go in, four stay in hand");
        server.tick += MINUTE / 5 - 1;
        server.show_done_stations();
        assert_eq!(at(&server, AT), "base:pit_kiln_firing", "not yet");
        server.tick += 1;
        server.show_done_stations();
        assert_eq!(at(&server, AT), "base:pit_kiln_fired");
        let crafting = xp(&server, "core:crafting");
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:fired_clay"), 12, "none cracked");
        assert_eq!(at(&server, AT), "base:pit_kiln");
        assert_eq!(xp(&server, "core:crafting") - crafting, 720, "taking pays Crafting, 6 a clay (in tenths)");
        assert_eq!(server.players[&1].tallies.get("cooked:base:fired_clay"), Some(&12), "Fire the Clay counts it cooked");
        hold(&mut server, "base:cobblestone", 3);
        server.use_station(1, AT);
        server.tick += MINUTE / 5;
        server.show_done_stations();
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:stone"), 3);
        hold(&mut server, "base:oak_log", 1);
        server.use_station(1, AT);
        assert_eq!((at(&server, AT).as_str(), carried(&server, "base:oak_log")), ("base:pit_kiln", 1), "a log is no kiln load");
    }

    /// A raw hide off a beast cures on the rack in ten minutes, and taking it pays Crafting.
    #[test]
    fn a_raw_hide_hung_on_a_rack_is_hide_ten_minutes_later() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_hide", 3);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:drying_rack_hung");
        server.tick += 10 * MINUTE - 1;
        server.show_done_stations();
        assert_eq!(at(&server, AT), "base:drying_rack_hung", "not yet");
        server.tick += 1;
        server.show_done_stations();
        let crafting = xp(&server, "core:crafting");
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:hide"), 3);
        assert_eq!(xp(&server, "core:crafting") - crafting, 60, "taking pays Crafting, 2 a hide (in tenths)");
    }

    /// Looked at, a rack at work says what is on it and the seconds left, an empty one
    /// nothing; one too far off, or a block that is no station, is not answered.
    #[test]
    fn a_station_looked_at_says_what_is_on_it_and_how_long() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        assert_eq!(server.station_seen(1, AT), Some((None, None)), "empty");
        hold(&mut server, "base:raw_beef", 6);
        server.use_station(1, AT);
        server.tick += 4 * MINUTE;
        let load = server.station_seen(1, AT).and_then(|(load, _)| load).expect("loaded");
        assert_eq!((load.item, load.count, load.output, load.makes), (block(&server, "base:raw_beef"), 6, block(&server, "base:jerky"), 6));
        assert_eq!(load.seconds, 6 * 60, "ten minutes, four gone");
        server.tick += 6 * MINUTE;
        assert_eq!(server.station_seen(1, AT).and_then(|(load, _)| load).map(|l| l.seconds), Some(0), "done");
        assert_eq!(server.station_seen(1, AT + IVec3::new(0, 0, 40)), None, "too far");
        assert_eq!(server.station_seen(1, AT + IVec3::new(1, 0, 0)), None, "no station");
    }

    /// A rack works while the column is not even loaded: a done rack still shows its old
    /// block, and using it takes the jerky all the same.
    #[test]
    fn a_rack_left_alone_is_done_when_its_owner_comes_back() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_small_game", 3);
        server.use_station(1, AT);
        server.tick += 60 * MINUTE;
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:jerky"), 3);
        assert_eq!(at(&server, AT), "base:drying_rack");
    }

    /// The rack needs open sky: under a roof it will not load, and a roof put over a
    /// loaded rack spoils the drying until it comes off again.
    #[test]
    fn a_rack_dries_only_under_open_sky() {
        let mut server = fixture(1);
        let stone = block(&server, "base:stone");
        server.set_block(AT, block(&server, "base:drying_rack"));
        server.set_block(AT + IVec3::new(0, 3, 0), stone);
        hold(&mut server, "base:raw_beef", 4);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:drying_rack", "roofed: refused");
        assert_eq!(carried(&server, "base:raw_beef"), 4);
        server.set_block(AT + IVec3::new(0, 3, 0), BlockId::AIR);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:drying_rack_hung");
        server.set_block(AT + IVec3::new(0, 3, 0), stone);
        server.tick += 10 * MINUTE;
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:jerky"), 0, "roofed: nothing to take");
        server.set_block(AT + IVec3::new(0, 3, 0), BlockId::AIR);
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:jerky"), 4);
    }

    /// A fire within three makes the rack twice as quick.
    #[test]
    fn a_fire_by_the_rack_dries_twice_as_fast() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        server.set_block(AT + IVec3::new(2, 0, 0), block(&server, "base:campfire"));
        hold(&mut server, "base:raw_beef", 1);
        server.use_station(1, AT);
        assert_eq!(server.stations.get(AT).unwrap().due, server.tick + 5 * MINUTE);
    }

    /// The heap: greens go on four to a compost, a load is whole fours, fewer than four is
    /// refused, it asks Farming 5, and water within two makes it half again as quick.
    #[test]
    fn a_heap_rots_greens_four_to_one() {
        let mut server = fixture(4);
        server.set_block(AT, block(&server, "base:compost_heap"));
        hold(&mut server, "base:wheat", 10);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:compost_heap", "Farming 4: refused");
        let farming = server.content.skills.id("core:farming").unwrap();
        let more = server.content.skills.xp_for(5) - xp(&server, "core:farming");
        server.players.get_mut(&1).unwrap().skills.grant(farming, more);
        hold(&mut server, "base:wheat", 3);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:compost_heap", "three: too few");
        assert_eq!(carried(&server, "base:wheat"), 3);
        hold(&mut server, "base:wheat", 10);
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:wheat"), 2, "eight go on, whole fours only");
        assert_eq!(server.stations.get(AT).unwrap().due, server.tick + 15 * MINUTE);
        server.tick += 15 * MINUTE;
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:compost"), 2);

        let wet = AT + IVec3::new(6, 0, 0);
        server.set_block(wet, block(&server, "base:compost_heap"));
        server.set_block(wet + IVec3::new(2, -1, 0), block(&server, "base:water"));
        hold(&mut server, "base:carrot", 4);
        server.use_station(1, wet);
        assert_eq!(server.stations.get(wet).unwrap().due, server.tick + 10 * MINUTE);

        // A well beside a heap keeps it as damp as water does.
        let by_well = AT + IVec3::new(6, 0, 6);
        server.set_block(by_well, block(&server, "base:compost_heap"));
        server.set_block(by_well + IVec3::new(2, 0, 0), block(&server, "base:well"));
        hold(&mut server, "base:carrot", 4);
        server.use_station(1, by_well);
        assert_eq!(server.stations.get(by_well).unwrap().due, server.tick + 10 * MINUTE);
    }

    /// The pot: it simmers only over a hob (a campfire, or on the hearthstone) with water
    /// near; two grain become porridge at Cooking 3, three meat or vegetables a stew at 14.
    #[test]
    fn a_pot_simmers_on_a_hob_by_water() {
        let mut server = fixture(1);
        let cooking = server.content.skills.id("core:cooking").unwrap();
        let grant = |server: &mut Server, level: u8| {
            let more = server.content.skills.xp_for(level) - xp(server, "core:cooking");
            server.players.get_mut(&1).unwrap().skills.grant(cooking, more);
        };
        grant(&mut server, 3);
        server.set_block(AT, block(&server, "base:cooking_pot"));
        hold(&mut server, "base:wheat", 4);
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:cooking_pot", "no fire under it");
        server.set_block(AT + IVec3::new(1, 0, 0), block(&server, "base:campfire"));
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:cooking_pot", "no water near");
        server.set_block(AT + IVec3::new(-3, -1, 0), block(&server, "base:water"));
        server.use_station(1, AT);
        assert_eq!(at(&server, AT), "base:cooking_pot_simmering");
        assert_eq!(carried(&server, "base:wheat"), 0, "two sets of two");
        server.tick += 2 * MINUTE;
        server.use_station(1, AT);
        assert_eq!(carried(&server, "base:porridge"), 2);

        // A stew asks Cooking 14; on the hearthstone, the pot needs no campfire.
        let hearth = AT + IVec3::new(0, 0, 4);
        server.set_block(hearth, block(&server, "base:hearthstone"));
        server.set_block(hearth + IVec3::Y, block(&server, "base:cooking_pot"));
        hold(&mut server, "base:carrot", 3);
        server.use_station(1, hearth + IVec3::Y);
        assert_eq!(at(&server, hearth + IVec3::Y), "base:cooking_pot", "Cooking 3: no stew");
        grant(&mut server, 14);
        hold(&mut server, "base:carrot", 3);
        server.use_station(1, hearth + IVec3::Y);
        assert_eq!(at(&server, hearth + IVec3::Y), "base:cooking_pot_simmering");
        server.tick += 3 * MINUTE;
        server.use_station(1, hearth + IVec3::Y);
        assert_eq!(carried(&server, "base:stew"), 1);
    }

    /// Breaking a loaded station lets its load fall: the raw input before it is done, the
    /// output after.
    #[test]
    fn a_broken_station_drops_what_was_on_it() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_beef", 5);
        server.use_station(1, AT);
        server.set_block(AT, BlockId::AIR);
        assert_eq!(loose(&server, "base:raw_beef"), 5);
        assert_eq!(server.stations.get(AT), None);

        server.set_block(AT, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_beef", 3);
        server.use_station(1, AT);
        server.tick += 10 * MINUTE;
        server.show_done_stations();
        server.set_block(AT, BlockId::AIR);
        assert_eq!(loose(&server, "base:jerky"), 3);
    }

    /// Loads save with the world and come back as they were.
    #[test]
    fn loads_keep_across_a_save() {
        let mut server = fixture(1);
        server.set_block(AT, block(&server, "base:drying_rack"));
        hold(&mut server, "base:raw_beef", 6);
        server.use_station(1, AT);
        let blocks = &server.content.blocks;
        let saved = server.stations.saved(|b| blocks.def(b).name.clone());
        assert_eq!(saved.len(), 1);
        let back = Loads::restore(&saved, |name| blocks.id(name));
        assert_eq!(back.get(AT), server.stations.get(AT));
    }

    /// The earth clamp went (blocks_stations.ron): one an older world holds is the log it was
    /// heaped over, and what was loaded in it falls out whole at the next look.
    #[test]
    fn a_retired_earth_clamp_is_a_log_and_gives_back_its_load() {
        let mut server = fixture(1);
        let log = server.content.blocks.saved_id("base:earth_clamp_lit").expect("renamed");
        assert_eq!(log, block(&server, "base:oak_log"));
        server.set_block(AT, log);
        let saved = [SavedLoad { position: AT.to_array(), item: "base:clay".into(), count: 16, due: server.tick + MINUTE }];
        server.stations = Loads::restore(&saved, |name| server.content.blocks.item_named(name));
        server.show_done_stations();
        assert_eq!(server.stations.get(AT), None, "the load is gone from the log");
        assert_eq!(loose(&server, "base:clay"), 16, "and lies beside it, all sixteen");
        assert_eq!(at(&server, AT), "base:oak_log");
    }

    /// Wheat sprouts on farmland at y 200 for each x of `xs` along z 40.
    fn field(server: &mut Server, xs: std::ops::Range<i32>) -> voxmar_engine::crop::CropId {
        let seed = block(server, "base:wheat_seeds");
        let wheat = server.content.crops.planted_by(seed).unwrap();
        let sprout = server.content.crops.get(wheat).stages[0];
        let farmland = block(server, "base:farmland");
        for x in xs {
            server.set_block(IVec3::new(x, 199, 40), farmland);
            server.world.set_block(IVec3::new(x, 200, 40), sprout);
            server.farm.schedule(IVec3::new(x, 200, 40), crate::farm::Planted { crop: wheat, stage: 0, due: u64::MAX - 1, sown: true });
        }
        wheat
    }

    fn stage(server: &Server, x: i32) -> usize {
        let pos = IVec3::new(x, 200, 40);
        let seed = block(server, "base:wheat_seeds");
        let wheat = server.content.crops.planted_by(seed).unwrap();
        let here = server.block(pos).unwrap();
        server.content.crops.get(wheat).stages.iter().position(|&b| b == here).unwrap()
    }

    /// Compost on a growing crop grows it and its neighbours a stage; bone meal only the
    /// one; with nothing to grow the fertiliser stays in hand.
    #[test]
    fn compost_grows_a_crop_and_its_neighbours() {
        let mut server = fixture(5);
        field(&mut server, 30..35);
        hold(&mut server, "base:compost", 2);
        server.fertilise(1, IVec3::new(32, 200, 40));
        assert_eq!((30..35).map(|x| stage(&server, x)).collect::<Vec<_>>(), [0, 1, 1, 1, 0]);
        assert_eq!(carried(&server, "base:compost"), 1);
        assert_eq!(server.players[&1].tallies.get("fertilised:base:compost"), Some(&3), "quests count the crops grown");

        hold(&mut server, "base:bone", 1);
        server.fertilise(1, IVec3::new(30, 200, 40));
        assert_eq!((30..35).map(|x| stage(&server, x)).collect::<Vec<_>>(), [1, 1, 1, 1, 0]);
        assert_eq!(carried(&server, "base:bone"), 0);

        hold(&mut server, "base:compost", 1);
        server.fertilise(1, IVec3::new(20, 200, 40));
        assert_eq!(carried(&server, "base:compost"), 1, "nothing to grow: kept");
    }
}
