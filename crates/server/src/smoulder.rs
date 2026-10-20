//! Structures that smoulder (multiblocks.ron's `smoulder`): a charcoal clamp, lit from a
//! torch in hand, chars its stacked firewood by the world's clock, unwatched. Done in its
//! time it leaves charcoal; opened before then (any piece of it broken) what it holds is
//! part-burnt; rain falling on its open flue spoils the share of it that the rain fell for.
//! What smoulders is saved with the world, by game time, so leaving changes nothing.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::glam::{IVec3, Vec2};
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::multiblock::{Multiblock, turned};
use voxmar_protocol::{PlayerId, TICKS_PER_DAY};

use crate::Server;

/// Game ticks in an hour.
const HOUR: f64 = TICKS_PER_DAY as f64 / 24.0;

/// A structure smouldering: which, turned how, and by the world's day ticks when it was lit,
/// when it is done, when it was last looked at, and for how long rain fell on its flue;
/// and whether its flue was roofed over when last seen loaded, for while it is not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Smouldering {
    pub position: [i32; 3],
    pub structure: String,
    pub turn: u8,
    pub lit: u64,
    pub done: u64,
    pub seen: u64,
    #[serde(default)]
    pub wet: u64,
    #[serde(default)]
    pub roofed: bool,
}

/// How a smoulder ended.
#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Done,
    Opened,
}

impl Server {
    /// `agent` lights `structure`, standing round `core`, with what is in hand: anything
    /// tagged its `lit_by` (`firelighter`: a torch, a firebrand or a campfire), used up never. Lit, its core burns, and it chars for its hours.
    pub(crate) fn light_smoulder(&mut self, agent: PlayerId, core: IVec3, structure: &Multiblock) {
        let Some(smoulder) = &structure.smoulder else { return };
        let title = Words::Own(def_key("multiblock", &structure.name), structure.title.clone());
        if self.smoulders.contains_key(&core) {
            let left = self.smoulders[&core].done.saturating_sub(self.day as u64) as f64 / HOUR;
            self.tell_smoulder(agent, Words::key("multiblock.smouldering").with("structure", title).with("hours", format!("{:.0}", left.ceil())));
            return;
        }
        let blocks = &self.content.blocks;
        let lit_by = structure.lit_by.as_deref().unwrap_or_default();
        if !self.players.get(&agent).and_then(|p| p.held()).is_some_and(|item| blocks.def(item).has_tag(lit_by)) {
            // Every item that lights it, by name: not a block it only places as (a wall torch).
            let named = Words::List(blocks.tagged(lit_by).filter(|&item| blocks.item_of(item) == item).map(|item| Words::Block(blocks.def(item).name.clone())).collect());
            self.refuse(agent, Words::key("multiblock.lit_by").with("structure", title).with("item", named));
            return;
        }
        let Some(turn) = structure.turn(core, |p| self.block(p)) else { return };
        let now = self.day.floor() as u64;
        let done = now + (smoulder.hours as f64 * HOUR).round() as u64;
        // Its flue is loaded now, so a roof already over it is known before it is left.
        let roofed = !self.open_sky(core + IVec3::Y);
        self.smoulders.insert(core, Smouldering { position: core.to_array(), structure: structure.name.clone(), turn, lit: now, done, seen: now, wet: 0, roofed });
        self.set_block(core, smoulder.burning);
        self.tally(agent, "lit", &structure.name, 1);
        self.tell_smoulder(agent, Words::key("multiblock.smoulders").with("structure", title).with("hours", format!("{:.0}", smoulder.hours.ceil())));
    }

    fn tell_smoulder(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(voxmar_protocol::ServerMessage::Chat { kind: voxmar_protocol::ChatKind::Info, text: words });
        }
    }

    /// A block changed at `changed`: a smouldering structure it was part of, no longer
    /// standing, has been opened, and what it holds is part-burnt. One partly in land not
    /// loaded is judged once all of it is there again, never as opened for the land missing.
    pub(crate) fn smoulder_changed(&mut self, changed: IVec3) {
        if self.smoulders.is_empty() {
            return;
        }
        let structures = &self.content.multiblocks;
        let reach = structures.reach();
        let opened: Vec<IVec3> = self
            .smoulders
            .iter()
            .filter(|&(&core, s)| {
                (changed - core).abs().max_element() <= reach
                    && structures.get(&s.structure).is_some_and(|m| {
                        m.includes(core, changed) && m.cells_at(core, s.turn).all(|p| self.world.block(p).is_some()) && !m.stands(core, |p| self.world.block(p))
                    })
            })
            .map(|(&core, _)| core)
            .collect();
        for core in opened {
            self.end_smoulder(core, End::Opened);
        }
    }

    /// Looks over each smouldering structure: rain on its open flue is counted as it falls,
    /// loaded or not (one not loaded is open as it was when last seen), and one whose time
    /// is up is done once all of it is loaded; until then it waits.
    pub(crate) fn tend_smoulders(&mut self) {
        if self.smoulders.is_empty() {
            return;
        }
        let (now, tick, day, wet) = (self.day.floor() as u64, self.tick, self.day, self.survival.douse_rain);
        let cores: Vec<IVec3> = self.smoulders.keys().copied().collect();
        let mut done = Vec::new();
        for core in cores {
            let s = &self.smoulders[&core];
            let cells: Vec<IVec3> = self.content.multiblocks.get(&s.structure).and_then(|m| m.smoulder.as_ref()).map_or_else(Vec::new, |sm| sm.wood.iter().map(|&o| core + turned(o, s.turn)).collect());
            let loaded = self.block(core).is_some() && cells.iter().all(|&p| self.block(p).is_some());
            if self.block(core).is_some() {
                let roofed = !self.open_sky(core + IVec3::Y);
                self.smoulders.get_mut(&core).expect("listed above").roofed = roofed;
            }
            let open = !self.smoulders[&core].roofed;
            let raining = open && {
                let report = self.weather.local(&mut self.temperature, Vec2::new(core.x as f32, core.z as f32), tick, day);
                report.weather.precipitation * (1.0 - report.weather.snow) >= wet
            };
            let s = self.smoulders.get_mut(&core).expect("listed above");
            let until = now.min(s.done);
            if raining {
                s.wet += until.saturating_sub(s.seen);
            }
            s.seen = s.seen.max(until);
            if now >= s.done && loaded {
                done.push(core);
            }
        }
        for core in done {
            self.end_smoulder(core, End::Done);
        }
    }

    /// Turns what `core`'s structure chars into what it made: done, its share of the burn
    /// rained on spoiled (from the top down, where the rain came in) and the rest charred
    /// well; opened, all part-burnt. A cell emptied by hand stays empty. The core burns out.
    fn end_smoulder(&mut self, core: IVec3, end: End) {
        let Some(s) = self.smoulders.remove(&core) else { return };
        let Some(smoulder) = self.content.multiblocks.get(&s.structure).and_then(|m| m.smoulder.clone()) else { return };
        let mut cells: Vec<IVec3> = smoulder.wood.iter().map(|&o| core + turned(o, s.turn)).filter(|&p| self.block(p).is_some_and(|b| !b.is_air())).collect();
        cells.sort_by_key(|p| std::cmp::Reverse(p.y));
        let spoiled = match end {
            End::Opened => 0,
            End::Done => {
                let share = s.wet as f64 / s.done.saturating_sub(s.lit).max(1) as f64;
                (cells.len() as f64 * share.min(1.0)).round() as usize
            }
        };
        for (i, pos) in cells.into_iter().enumerate() {
            let into = match end {
                End::Opened => smoulder.early,
                End::Done if i < spoiled => smoulder.spoiled,
                End::Done => smoulder.done,
            };
            self.set_block(pos, into);
        }
        if self.block(core) == Some(smoulder.burning) {
            self.set_block(core, smoulder.spent);
        }
    }

    pub(crate) fn saved_smoulders(&self) -> Vec<Smouldering> {
        let mut saved: Vec<Smouldering> = self.smoulders.values().cloned().collect();
        saved.sort_by_key(|s| s.position);
        saved
    }
}

/// Saved smoulders back, dropping those whose structure no longer smoulders.
pub(crate) fn restore(saved: &[Smouldering], smoulders: impl Fn(&str) -> bool) -> FxHashMap<IVec3, Smouldering> {
    saved.iter().filter(|s| smoulders(&s.structure)).map(|s| (IVec3::from_array(s.position), s.clone())).collect()
}

/// Looks over what smoulders once a second.
#[derive(Default)]
pub(crate) struct Smoulders;

impl crate::process::Process for Smoulders {
    fn tier(&self) -> voxmar_engine::schedule::Tier {
        voxmar_engine::schedule::Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        server.tend_smoulders();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use voxmar_engine::action::ActionTarget;
    use voxmar_engine::block::BlockId;
    use voxmar_engine::coords::{ColumnPos, Face};
    use voxmar_engine::glam::{IVec2, IVec3, Vec2, Vec3};
    use voxmar_engine::inventory::ItemStack;
    use voxmar_engine::world::Column;
    use voxmar_protocol::{TICKS_PER_DAY, TICKS_PER_SECOND};

    use crate::Server;

    const FLOOR: i32 = 40;
    /// The clamp's north-west corner on the ground; its flue is two in and one up.
    const CORNER: IVec3 = IVec3::new(8, FLOOR + 1, 8);
    const FLUE: IVec3 = IVec3::new(10, FLOOR + 2, 10);

    /// A stone floor under open sky, and player 1 on it holding `item`.
    fn fixture(item: &str) -> Server {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = id(&server, "base:stone");
        let mut column = Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, FLOOR, z), stone);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = Vec3::new(10.5, (FLOOR + 1) as f32, 6.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        player.inventory.slots[0] = Some(stack(id(&server, item), 1));
        player.selected = 0;
        server.players.insert(1, player);
        server
    }

    fn id(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    fn stack(item: BlockId, count: u8) -> ItemStack {
        ItemStack::new(item, count)
    }

    /// Nine firewood under earth, kindling on the roof: the clamp as multiblocks.ron draws it.
    fn build(server: &mut Server) {
        let (wood, earth, kindling) = (id(server, "base:firewood"), id(server, "base:dirt"), id(server, "base:kindling"));
        for x in 0..5 {
            for z in 0..5 {
                let inner = (1..4).contains(&x) && (1..4).contains(&z);
                server.set_block(CORNER + IVec3::new(x, 0, z), if inner { wood } else { earth });
                if inner {
                    server.set_block(CORNER + IVec3::new(x, 1, z), if (x, z) == (2, 2) { kindling } else { earth });
                }
            }
        }
    }

    fn light(server: &mut Server) {
        let action = server.content.actions.expect("base:light");
        server.propose(1, action, ActionTarget::Block { block: FLUE, face: Face::PosY }, Instant::now());
    }

    /// What lies where the firewood was, by name and how many of each.
    fn charred(server: &Server) -> Vec<(String, usize)> {
        let mut found: Vec<(String, usize)> = Vec::new();
        for x in 1..4 {
            for z in 1..4 {
                let name = server.block(CORNER + IVec3::new(x, 0, z)).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone());
                match found.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, count)) => *count += 1,
                    None => found.push((name, 1)),
                }
            }
        }
        found.sort();
        found
    }

    /// The clamp's burn, in day ticks, as multiblocks.ron gives it.
    fn burn_ticks(server: &Server) -> f64 {
        let clamp = server.content.multiblocks.get("base:charcoal_clamp").expect("the base clamp");
        clamp.smoulder.as_ref().expect("it smoulders").hours as f64 * TICKS_PER_DAY as f64 / 24.0
    }

    /// Clear skies held over the clamp, so rain falls only when a test brings it.
    fn sky(server: &mut Server, preset: &str) {
        let here = Vec2::new(FLUE.x as f32, FLUE.z as f32);
        let preset = server.content.weather.preset(preset).unwrap().at(1.0);
        server.weather.force(preset, here, 6000, server.tick);
        for _ in 0..3 {
            server.tick += TICKS_PER_SECOND as u64;
            let (tick, day) = (server.tick, server.day);
            server.weather.advance(&mut server.temperature, &[IVec2::new(FLUE.x, FLUE.z)], tick, day);
        }
    }

    /// A clamp is lit only from a flame in hand, used up never; lit, its flue glows, and
    /// its game hours on, nobody by it, its firewood is charcoal, four a heap.
    #[test]
    fn a_clamp_lit_from_a_torch_chars_its_firewood_in_its_hours() {
        let mut server = fixture("base:stick");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        assert!(server.smoulders.is_empty(), "a stick does not light it");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(stack(id(&server, "base:torch"), 1));
        light(&mut server);
        assert_eq!(server.block(FLUE), Some(id(&server, "base:clamp_flue")), "lit");
        assert_eq!(server.players[&1].inventory.slots[0].map(|s| s.count), Some(1), "the torch kept");
        server.day += burn_ticks(&server) - 1.0;
        server.tend_smoulders();
        assert_eq!(charred(&server), [("base:firewood".to_owned(), 9)], "a tick short");
        server.day += 1.0;
        server.tend_smoulders();
        assert_eq!(charred(&server), [("base:charcoal_heap".to_owned(), 9)]);
        assert_eq!(server.block(FLUE), Some(BlockId::AIR), "the flue burnt out");
        let heap = server.content.blocks.info(id(&server, "base:charcoal_heap"));
        assert_eq!((heap.drops, heap.drop_count), (id(&server, "base:charcoal"), 4), "thirty-six charcoal, a kiln's charge and more");
        assert!(server.smoulders.is_empty());
    }

    /// A piece of it broken before its time opens it: what it holds is part-burnt.
    #[test]
    fn a_clamp_opened_early_leaves_part_burnt_wood() {
        let mut server = fixture("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        server.day += burn_ticks(&server) * 3.0 / 8.0;
        server.tend_smoulders();
        // A change told while its land is not loaded opens nothing: missing land is no gap.
        let column = server.world.remove(ColumnPos::new(0, 0)).unwrap();
        server.smoulder_changed(CORNER);
        server.world.insert(ColumnPos::new(0, 0), column);
        assert_eq!(server.smoulders.len(), 1, "still smouldering with its land unloaded");
        server.set_block(CORNER, BlockId::AIR);
        assert_eq!(charred(&server), [("base:charred_wood_heap".to_owned(), 9)]);
        assert!(server.smoulders.is_empty() && server.block(FLUE) == Some(BlockId::AIR));
    }

    /// Rain on the open flue for half the burn spoils half the heaps to ash, from the top;
    /// a roof over the flue keeps it dry.
    #[test]
    fn rain_on_the_open_flue_spoils_its_share_to_ash() {
        let burn = |roofed: bool| {
            let mut server = fixture("base:torch");
            sky(&mut server, "clear");
            build(&mut server);
            if roofed {
                server.set_block(FLUE + IVec3::new(0, 3, 0), id(&server, "base:stone"));
            }
            light(&mut server);
            server.day += burn_ticks(&server) / 2.0;
            server.tend_smoulders();
            sky(&mut server, "rain");
            server.day += burn_ticks(&server) / 2.0;
            server.tend_smoulders();
            charred(&server)
        };
        assert_eq!(burn(false), [("base:ash_heap".to_owned(), 5), ("base:charcoal_heap".to_owned(), 4)], "rained on for half of it (rounded up)");
        assert_eq!(burn(true), [("base:charcoal_heap".to_owned(), 9)]);
    }

    /// Rain is counted while nobody has the clamp loaded too, open as it was last seen: a
    /// player away through the rain comes back to the same ash as one who stood by, and a
    /// roofed one stays dry. Done while not loaded, it waits to char until all of it is.
    #[test]
    fn rain_while_the_clamp_is_not_loaded_counts_as_it_fell() {
        let burn = |roofed: bool| {
            let mut server = fixture("base:torch");
            sky(&mut server, "clear");
            build(&mut server);
            if roofed {
                server.set_block(FLUE + IVec3::new(0, 3, 0), id(&server, "base:stone"));
            }
            light(&mut server);
            server.day += burn_ticks(&server) / 2.0;
            server.tend_smoulders();
            let column = server.world.remove(ColumnPos::new(0, 0)).unwrap();
            sky(&mut server, "rain");
            server.day += burn_ticks(&server) / 2.0;
            server.tend_smoulders();
            assert_eq!(server.smoulders.len(), 1, "done, but waiting to be loaded");
            sky(&mut server, "clear");
            server.world.insert(ColumnPos::new(0, 0), column);
            server.tend_smoulders();
            assert!(server.smoulders.is_empty());
            charred(&server)
        };
        assert_eq!(burn(false), [("base:ash_heap".to_owned(), 5), ("base:charcoal_heap".to_owned(), 4)], "the rain while away counted, the clear sky on return not");
        assert_eq!(burn(true), [("base:charcoal_heap".to_owned(), 9)], "roofed when last seen");
        // Roofed, lit and left at once, never tended while loaded: the roof seen as it was lit.
        let mut server = fixture("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        server.set_block(FLUE + IVec3::new(0, 3, 0), id(&server, "base:stone"));
        light(&mut server);
        let column = server.world.remove(ColumnPos::new(0, 0)).unwrap();
        sky(&mut server, "rain");
        server.day += burn_ticks(&server);
        server.tend_smoulders();
        sky(&mut server, "clear");
        server.world.insert(ColumnPos::new(0, 0), column);
        server.tend_smoulders();
        assert_eq!(charred(&server), [("base:charcoal_heap".to_owned(), 9)], "roofed as it was lit, left at once");
    }

    /// What smoulders is saved with the world, and comes back while its structure does.
    #[test]
    fn a_smouldering_clamp_is_saved_and_restored() {
        let mut server = fixture("base:torch");
        build(&mut server);
        light(&mut server);
        let saved = server.saved_smoulders();
        let back: Vec<super::Smouldering> = ron::from_str(&ron::to_string(&saved).unwrap()).unwrap();
        assert_eq!(super::restore(&back, |name| name == "base:charcoal_clamp"), server.smoulders);
        assert!(super::restore(&back, |_| false).is_empty(), "a clamp no longer in the content is dropped");
    }

    /// A log held to a chopping block splits into two firewood only with an axe about
    /// you, which is kept.
    #[test]
    fn a_log_splits_into_firewood_on_a_chopping_block_with_an_axe() {
        let mut server = fixture("base:oak_log");
        let block = CORNER + IVec3::new(4, 0, -2);
        server.set_block(block, id(&server, "base:chopping_block"));
        let cook = server.content.actions.expect("base:cook");
        let split = |server: &mut Server| {
            server.propose(1, cook, ActionTarget::Block { block, face: Face::PosY }, Instant::now());
            server.cook(1, block);
        };
        // The click itself is refused without an axe carried (`Requirement::CookingTool`).
        let def = server.content.actions.get(cook).unwrap().clone();
        let target = ActionTarget::Block { block, face: Face::PosY };
        let refused = |server: &Server| def.validate(&crate::actions::context(server, 1), target);
        assert_eq!(refused(&server), Err(voxmar_engine::action::Rejection::Unmet(voxmar_engine::action::Requirement::CookingTool)));
        split(&mut server);
        let count = |server: &Server, name: &str| server.players[&1].inventory.count(server.content.blocks.expect(name));
        assert_eq!((count(&server, "base:oak_log"), count(&server, "base:firewood")), (1, 0), "no axe, no split");
        server.players.get_mut(&1).unwrap().inventory.slots[4] = Some(stack(id(&server, "base:stone_axe"), 1));
        assert_eq!(refused(&server), Ok(()), "an axe in the pack, not in hand");
        split(&mut server);
        assert_eq!((count(&server, "base:oak_log"), count(&server, "base:firewood"), count(&server, "base:stone_axe")), (0, 2, 1), "split, the axe kept");
        // A full pack loses nothing: the split comes back into the emptied hand.
        let (dirt, log) = (id(&server, "base:dirt"), id(&server, "base:oak_log"));
        let player = server.players.get_mut(&1).unwrap();
        for slot in player.inventory.slots.iter_mut().filter(|s| s.is_none()) {
            *slot = Some(stack(dirt, 64));
        }
        let selected = player.selected;
        player.inventory.slots[selected] = Some(stack(log, 1));
        let wood_before = count(&server, "base:firewood");
        split(&mut server);
        assert_eq!((count(&server, "base:oak_log"), count(&server, "base:firewood") - wood_before), (0, 2), "a full pack: split into the hand");
        let machines = &server.content.machines;
        let (log, wood, coal) = (id(&server, "base:oak_log"), id(&server, "base:firewood"), id(&server, "base:charcoal"));
        assert_eq!([log, wood, coal].map(|item| machines.fuel_heat(item)), [Some(1), Some(2), Some(3)], "each burns hotter");
        assert!(2.0 * machines.burn_seconds(wood).unwrap() > machines.burn_seconds(log).unwrap(), "split, a log burns longer");
    }
}
