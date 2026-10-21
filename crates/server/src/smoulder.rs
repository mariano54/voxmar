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
    /// Who lit it, by key (`player_key`), told when it is done; None for one lit before
    /// this was kept.
    #[serde(default)]
    pub by: Option<String>,
    /// Whether whoever lit it has been told it is done (or it waits for them to be on,
    /// `Waiting`): when its time is up out of loaded land, before it is turned into what it
    /// made there.
    #[serde(default)]
    pub told: bool,
    /// Whether a block of it changed while part of it was not loaded: it may have been
    /// opened, which is judged only once all of it is loaded, so it is not told done before.
    #[serde(default)]
    pub doubt: bool,
}

/// News of a firing for a player away when it came: whose (`player_key`) and what it says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Waiting {
    pub owner: String,
    pub words: Words,
}

/// How much news waits for one player away: the newest, so a long absence brings back what
/// is true now rather than every firing since, and fewer than their screen shows at once, so
/// a line stays free for whatever else they are told as they come on.
const NEWS_WAITING: usize = voxmar_protocol::NEWS_LINES - 1;
const _: () = assert!(NEWS_WAITING > 0 && NEWS_WAITING < voxmar_protocol::NEWS_LINES);

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
            let left = self.smoulders[&core].done.saturating_sub(self.day as u64) as f64;
            let words = Words::key("multiblock.smouldering").with("structure", title).with("hours", format!("{:.0}", (left / HOUR).ceil())).with("minutes", Words::text(self.real_minutes(left)));
            self.tell_smoulder(agent, words);
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
        let by = self.players.get(&agent).map(|p| crate::player_key(&p.name));
        self.smoulders.insert(core, Smouldering { position: core.to_array(), structure: structure.name.clone(), turn, lit: now, done, seen: now, wet: 0, roofed, by, told: false, doubt: false });
        self.set_block(core, smoulder.burning_at(0.0));
        self.tally(agent, "lit", &structure.name, 1);
        let minutes = self.real_minutes(done.saturating_sub(now) as f64);
        self.tell_smoulder(agent, Words::key("multiblock.smoulders").with("structure", title).with("hours", format!("{:.0}", smoulder.hours.ceil())).with("minutes", Words::text(minutes)));
    }

    /// Day ticks as whole real minutes at the world's pace, rounded up, at least one: a
    /// player reads "two hours" of the day as their own unless told.
    fn real_minutes(&self, day_ticks: f64) -> u64 {
        let pace = (self.day_speed * self.content.commands.day_pace()) as f64 * voxmar_protocol::TICKS_PER_SECOND as f64 * 60.0;
        if pace > 0.0 { (day_ticks / pace).ceil().max(1.0) as u64 } else { 1 }
    }

    fn tell_smoulder(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(voxmar_protocol::ServerMessage::Chat { kind: voxmar_protocol::ChatKind::Info, text: words });
        }
    }

    /// A block changed at `changed`: a smouldering structure it was part of, no longer
    /// standing, has been opened, and what it holds is part-burnt. One partly in land not
    /// loaded is never counted opened for the land missing: [`Server::tend_smoulders`]
    /// judges it once all of it is there again.
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
        let (world, structures) = (&self.world, &self.content.multiblocks);
        for (&core, s) in self.smoulders.iter_mut() {
            if (changed - core).abs().max_element() <= reach
                && structures.get(&s.structure).is_some_and(|m| m.includes(core, changed) && !m.cells_at(core, s.turn).all(|p| world.block(p).is_some()))
            {
                s.doubt = true;
            }
        }
    }

    /// Looks over each smouldering structure: rain on its open flue is counted as it falls,
    /// loaded or not (one not loaded is open as it was when last seen); with all of it
    /// loaded, one no longer standing was opened (broken while part of it was not loaded),
    /// and one whose time is up is done. Until then it waits.
    pub(crate) fn tend_smoulders(&mut self) {
        if self.smoulders.is_empty() {
            return;
        }
        let (now, tick, day, wet) = (self.day.floor() as u64, self.tick, self.day, self.survival.douse_rain);
        let cores: Vec<IVec3> = self.smoulders.keys().copied().collect();
        let (mut done, mut opened, mut stage, mut told) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for core in cores {
            let s = &self.smoulders[&core];
            if let Some(m) = self.content.multiblocks.get(&s.structure)
                && m.cells_at(core, s.turn).all(|p| self.world.block(p).is_some())
                && !m.stands(core, |p| self.world.block(p))
            {
                opened.push(core);
                continue;
            }
            // All of it, not only its wood: a piece broken where the land is not loaded is
            // judged (opened, above) before its time can be up.
            let cells: Vec<IVec3> = self.content.multiblocks.get(&s.structure).map_or_else(Vec::new, |m| m.cells_at(core, s.turn).collect());
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
            } else if now >= s.done && !s.told && !s.doubt {
                told.push(core);
            } else if loaded {
                stage.push((core, now.saturating_sub(s.lit) as f32 / s.done.saturating_sub(s.lit).max(1) as f32));
            }
        }
        // Its flue shows how far along the burn is: catching, steady, dying down.
        for (core, share) in stage {
            let Some(smoulder) = self.smoulders.get(&core).and_then(|s| self.content.multiblocks.get(&s.structure)).and_then(|m| m.smoulder.as_ref()) else { continue };
            let now_shows = smoulder.burning_at(share);
            if self.block(core).is_some_and(|b| smoulder.is_burning(b) && b != now_shows) {
                self.set_block(core, now_shows);
            }
        }
        // Done out of loaded land: told now, wherever they are, of all it was lit with (none
        // of it can change unloaded), and turned into what it made once loaded.
        for core in told {
            let Some(s) = self.smoulders.get_mut(&core) else { continue };
            s.told = true;
            let s = s.clone();
            let heaps = self.content.multiblocks.get(&s.structure).and_then(|m| m.smoulder.as_ref()).map_or(0, |smoulder| smoulder.wood.len());
            self.tell_done(&s, heaps);
        }
        for core in done {
            self.end_smoulder(core, End::Done);
        }
        for core in opened {
            self.end_smoulder(core, End::Opened);
        }
    }

    /// Tells the one who lit a fuelled fire that is no home (a kiln) that it has gone cold,
    /// wherever they are, once, as its fuel runs out.
    /// Fires gone out are taken out of `lit` here, or by a block changed meanwhile
    /// (`fire_changed`), which keeps them for this to tell.
    pub(crate) fn tell_fires_out(&mut self) {
        let tick = self.tick;
        let mut cold = std::mem::take(&mut self.fires_out);
        let before = self.lit.len();
        self.lit.retain(|_, fire| {
            let keep = fire.until > tick;
            if !keep {
                cold.push(fire.clone());
            }
            keep
        });
        if self.lit.len() != before {
            self.send_fires();
        }
        // Even the first look after a start tells: a fire saved lit comes back only if it
        // still burns (`until` past the world's tick), so whatever goes out went out in play.
        let out: Vec<(String, String)> = cold
            .into_iter()
            .filter(|fire| self.content.multiblocks.get(&fire.structure).is_some_and(|m| !m.home && !m.fuels.is_empty()))
            .filter_map(|fire| Some((fire.owner?, fire.structure)))
            .collect();
        for (owner, structure) in out {
            let Some(m) = self.content.multiblocks.get(&structure) else { continue };
            let title = Words::Own(def_key("multiblock", &m.name), m.title.clone());
            self.tell_fired(&owner, Words::key("multiblock.fire_out").with("structure", title));
        }
    }

    /// What a smouldering structure's core looked at says (`Server::station_seen`): the wood
    /// it chars, what it will make of it (less what rain has spoiled so far) and the real
    /// seconds left at the world's pace. None for a core not smouldering.
    pub(crate) fn smoulder_seen(&self, core: IVec3) -> Option<voxmar_protocol::StationLoad> {
        let s = self.smoulders.get(&core)?;
        let structure = self.content.multiblocks.get(&s.structure)?;
        let smoulder = structure.smoulder.as_ref()?;
        let item = smoulder.wood.first().and_then(|&o| structure.takes(o).first().copied())?;
        let heaps = smoulder.wood.iter().filter(|&&o| self.block(core + turned(o, s.turn)).is_some_and(|b| structure.takes(o).contains(&b))).count();
        let share = s.wet as f64 / s.done.saturating_sub(s.lit).max(1) as f64;
        let good = heaps - (heaps as f64 * share.min(1.0)).round() as usize;
        let info = self.content.blocks.info(smoulder.done);
        let pace = (self.day_speed * self.content.commands.day_pace()) as f64 * voxmar_protocol::TICKS_PER_SECOND as f64;
        let seconds = if pace > 0.0 { (s.done as f64 - self.day).max(0.0) / pace } else { 0.0 };
        Some(voxmar_protocol::StationLoad {
            item,
            count: heaps.min(u8::MAX as usize) as u8,
            output: info.drops,
            makes: (good * info.drop_count as usize).min(u8::MAX as usize) as u8,
            seconds: seconds.ceil().min(u32::MAX as f64) as u32,
        })
    }

    /// Sends `words` to the player keyed `owner` (`player_key`); one away, or whose
    /// connection has gone, hears it when on again (`tell_waiting`), the newest
    /// `NEWS_WAITING` kept for each.
    fn tell_fired(&mut self, owner: &str, words: Words) {
        if let Some(player) = self.players.values().find(|p| crate::player_key(&p.name) == owner)
            && player.connection.send(voxmar_protocol::ServerMessage::Fired { words: words.clone() })
        {
            return;
        }
        self.news_waiting.push(Waiting { owner: owner.to_owned(), words });
        let theirs = self.news_waiting.iter().filter(|w| w.owner == owner).count();
        if theirs > NEWS_WAITING
            && let Some(oldest) = self.news_waiting.iter().position(|w| w.owner == owner)
        {
            self.news_waiting.remove(oldest);
        }
    }

    /// Tells player `id`, just on, the news that came while they were away, oldest first;
    /// what their connection would not take (gone again, or full) stays kept for them.
    pub(crate) fn tell_waiting(&mut self, id: voxmar_protocol::PlayerId) {
        let Some(player) = self.players.get(&id) else { return };
        let key = crate::player_key(&player.name);
        let (theirs, mut rest): (Vec<Waiting>, Vec<Waiting>) = std::mem::take(&mut self.news_waiting).into_iter().partition(|w| w.owner == key);
        let mut theirs = theirs.into_iter();
        for waiting in theirs.by_ref() {
            if !player.connection.send(voxmar_protocol::ServerMessage::Fired { words: waiting.words.clone() }) {
                rest.push(waiting);
                break;
            }
        }
        rest.extend(theirs);
        self.news_waiting = rest;
    }

    /// Turns what `core`'s structure chars into what it made: done, its share of the burn
    /// rained on spoiled (from the top down, where the rain came in) and the rest charred
    /// well; opened, all part-burnt. A cell emptied by hand, or with something else set in
    /// it, is left as it is. The core burns out.
    fn end_smoulder(&mut self, core: IVec3, end: End) {
        let Some(s) = self.smoulders.remove(&core) else { return };
        let Some(structure) = self.content.multiblocks.get(&s.structure).cloned() else { return };
        let Some(smoulder) = structure.smoulder.clone() else { return };
        let wood = |o: IVec3, p: IVec3| self.block(p).is_some_and(|b| structure.takes(o).contains(&b));
        let mut cells: Vec<IVec3> = smoulder.wood.iter().map(|&o| (o, core + turned(o, s.turn))).filter(|&(o, p)| wood(o, p)).map(|(_, p)| p).collect();
        cells.sort_by_key(|p| std::cmp::Reverse(p.y));
        let spoiled = match end {
            End::Opened => 0,
            End::Done => {
                let share = s.wet as f64 / s.done.saturating_sub(s.lit).max(1) as f64;
                (cells.len() as f64 * share.min(1.0)).round() as usize
            }
        };
        let heaps = cells.len();
        for (i, pos) in cells.into_iter().enumerate() {
            let into = match end {
                End::Opened => smoulder.early,
                End::Done if i < spoiled => smoulder.spoiled,
                End::Done => smoulder.done,
            };
            self.set_block(pos, into);
        }
        if self.block(core).is_some_and(|b| smoulder.is_burning(b)) {
            self.set_block(core, smoulder.spent);
        }
        if end == End::Done && !s.told {
            self.tell_done(&s, heaps);
        }
        // Told it was done, then opened after all: what waits there now is part-burnt.
        if end == End::Opened
            && s.told
            && let Some(by) = &s.by
        {
            let title = Words::Own(def_key("multiblock", &structure.name), structure.title.clone());
            // In what is taken from it, as the done note says: part-burnt wood, not its heaps.
            let info = self.content.blocks.info(smoulder.early);
            let count = heaps * info.drop_count as usize;
            let words = Words::key("multiblock.smoulder_opened").with("structure", title).with("count", Words::text(count)).with("item", Words::Block(self.content.blocks.def(info.drops).name.clone()));
            self.tell_fired(by, words);
        }
    }

    /// Whoever lit `s` hears it is done, wherever they are, and what waits there of the
    /// `heaps` of wood it charred: its share of the burn rained on spoiled.
    fn tell_done(&mut self, s: &Smouldering, heaps: usize) {
        let Some(by) = &s.by else { return };
        let Some(structure) = self.content.multiblocks.get(&s.structure) else { return };
        let Some(smoulder) = structure.smoulder.as_ref() else { return };
        let share = s.wet as f64 / s.done.saturating_sub(s.lit).max(1) as f64;
        let spoiled = (heaps as f64 * share.min(1.0)).round() as usize;
        let title = Words::Own(def_key("multiblock", &structure.name), structure.title.clone());
        let info = self.content.blocks.info(smoulder.done);
        let count = (heaps - spoiled) * info.drop_count as usize;
        let words = if spoiled > 0 { Words::key("multiblock.smoulder_done_wet").with("spoiled", Words::text(spoiled)) } else { Words::key("multiblock.smoulder_done") };
        let words = words.with("structure", title).with("count", Words::text(count)).with("item", Words::Block(self.content.blocks.def(info.drops).name.clone()));
        self.tell_fired(by, words);
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
        server.tell_fires_out();
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

    /// A stone floor under open sky, two columns wide, and player 1 on it holding `item`.
    fn fixture(item: &str) -> Server {
        let (server, client) = heard(item);
        std::mem::forget(client);
        server
    }

    /// The fixture, with player 1's end of the connection, to hear what they are told.
    fn heard(item: &str) -> (Server, voxmar_protocol::ClientConnection) {
        let mut server = crate::test_server();
        server.world = Default::default();
        let stone = id(&server, "base:stone");
        for cx in 0..2 {
            let mut column = Column::default();
            for x in 0..32 {
                for z in 0..32 {
                    column.set_block(IVec3::new(x, FLOOR, z), stone);
                }
            }
            server.world.insert(ColumnPos::new(cx, 0), Arc::new(column));
        }
        let (client, connection) = voxmar_protocol::local_pair();
        let at = Vec3::new(10.5, (FLOOR + 1) as f32, 6.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        player.inventory.slots[0] = Some(stack(id(&server, item), 1));
        player.selected = 0;
        server.players.insert(1, player);
        (server, client)
    }

    fn id(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    fn stack(item: BlockId, count: u8) -> ItemStack {
        ItemStack::new(item, count)
    }

    /// Nine firewood under earth, kindling on the roof: the clamp as multiblocks.ron draws it.
    fn build(server: &mut Server) {
        build_at(server, CORNER);
    }

    /// The clamp with its north-west corner at `corner`.
    fn build_at(server: &mut Server, corner: IVec3) {
        let (wood, earth, kindling) = (id(server, "base:firewood"), id(server, "base:dirt"), id(server, "base:kindling"));
        for x in 0..5 {
            for z in 0..5 {
                let inner = (1..4).contains(&x) && (1..4).contains(&z);
                server.set_block(corner + IVec3::new(x, 0, z), if inner { wood } else { earth });
                if inner {
                    server.set_block(corner + IVec3::new(x, 1, z), if (x, z) == (2, 2) { kindling } else { earth });
                }
            }
        }
    }

    fn light(server: &mut Server) {
        light_at(server, FLUE);
    }

    fn light_at(server: &mut Server, flue: IVec3) {
        let action = server.content.actions.expect("base:light");
        server.propose(1, action, ActionTarget::Block { block: flue, face: Face::PosY }, Instant::now());
    }

    /// What lies where the firewood was, by name and how many of each.
    fn charred(server: &Server) -> Vec<(String, usize)> {
        charred_at(server, CORNER)
    }

    fn charred_at(server: &Server, corner: IVec3) -> Vec<(String, usize)> {
        let mut found: Vec<(String, usize)> = Vec::new();
        for x in 1..4 {
            for z in 1..4 {
                let name = server.block(corner + IVec3::new(x, 0, z)).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone());
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
        assert_eq!(server.block(FLUE), Some(id(&server, "base:clamp_flue_catching")), "lit");
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

    /// A lit clamp looked at says what it chars, what it will make and the seconds left at
    /// the world's pace; its flue catches, burns steady and dies down as the burn goes on;
    /// and whoever lit it is told, wherever they are, when it is done and what waits there.
    #[test]
    fn a_clamp_shows_its_burn_and_tells_its_lighter_when_done() {
        let (mut server, client) = heard("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        let flue = |server: &Server| server.block(FLUE).map(|b| server.content.blocks.def(b).name.clone()).unwrap_or_default();
        assert_eq!(flue(&server), "base:clamp_flue_catching", "just lit, it catches");
        let burn = burn_ticks(&server);
        let pace = server.day_speed as f64 * server.content.commands.day_pace() as f64 * TICKS_PER_SECOND as f64;
        let seen = server.smoulder_seen(FLUE).expect("a lit clamp says what it does");
        assert_eq!(
            (seen.item, seen.count, seen.output, seen.makes, seen.seconds),
            (id(&server, "base:firewood"), 9, id(&server, "base:charcoal"), 36, (burn / pace).ceil() as u32),
            "nine firewood charring into thirty-six charcoal, its whole burn left"
        );
        assert!(server.station_seen(1, FLUE).is_some_and(|(load, fire)| load == Some(seen) && fire.is_none()), "answered when looked at");
        assert!(server.smoulder_seen(CORNER).is_none(), "only its core answers");
        server.day += burn / 2.0;
        server.tend_smoulders();
        assert_eq!(flue(&server), "base:clamp_flue", "halfway, steady");
        assert_eq!(server.smoulder_seen(FLUE).map(|s| s.seconds), Some((burn / 2.0 / pace).ceil() as u32), "half its time left");
        server.day += burn * 0.4;
        server.tend_smoulders();
        assert_eq!(flue(&server), "base:clamp_flue_dying", "near the end, dying down");
        while client.try_recv().is_ok_and(|m| m.is_some()) {}
        server.day += burn;
        server.tend_smoulders();
        let mut fired = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, args) } = message {
                let count = args.iter().find(|(name, _)| name == "count").map(|(_, w)| format!("{w:?}"));
                fired.push((key, count));
            }
        }
        assert!(matches!(fired.as_slice(), [(key, Some(count))] if key == "multiblock.smoulder_done" && count.contains("36")), "{fired:?}");
    }

    /// A kiln someone fired (a fuelled structure, no home) going cold tells them once,
    /// wherever they are, even when a block changed anywhere took it out first (as one
    /// nearly always does before the once-a-second look); a home hearth's fire never does.
    #[test]
    fn a_kiln_gone_cold_tells_whoever_fired_it_once() {
        let (mut server, client) = heard("base:torch");
        server.players.get_mut(&1).unwrap().name = "Ada".into();
        let kiln = server.content.multiblocks.iter().find(|m| !m.home && !m.fuels.is_empty()).expect("a fuelled kiln").name.clone();
        let hearth = server.content.multiblocks.iter().find(|m| m.home).expect("a home hearth").name.clone();
        let (here, there) = (IVec3::new(4, FLOOR + 1, 4), IVec3::new(20, FLOOR + 1, 20));
        server.tell_fires_out();
        let until = server.tick + 40;
        server.lit.insert(here, crate::hearth::Lit { position: here.to_array(), structure: kiln, until, owner: Some(crate::player_key("Ada")) });
        server.lit.insert(there, crate::hearth::Lit { position: there.to_array(), structure: hearth, until, owner: Some(crate::player_key("Ada")) });
        let told = |client: &voxmar_protocol::ClientConnection| {
            let mut keys = Vec::new();
            while let Ok(Some(message)) = client.try_recv() {
                if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, _) } = message {
                    keys.push(key);
                }
            }
            keys
        };
        told(&client);
        server.tick += 20;
        server.tell_fires_out();
        assert!(told(&client).is_empty(), "still burning");
        server.tick += 40;
        server.set_block(IVec3::new(8, FLOOR + 1, 8), server.content.blocks.expect("base:stone"));
        assert!(!server.lit.contains_key(&here), "a block changed takes the fire gone out away");
        server.tell_fires_out();
        assert_eq!(told(&client), ["multiblock.fire_out"], "gone cold, told once for the kiln and never for the hearth");
        server.tick += 40;
        server.tell_fires_out();
        assert!(told(&client).is_empty(), "told once");
    }

    /// Its earth clicked from the floor beside it, as a player there sees it: lit from a
    /// torch in hand as from its flue, the earth and the torch kept; from a stick, not.
    #[test]
    fn a_clamp_is_lit_from_its_earth() {
        let mut server = fixture("base:stick");
        sky(&mut server, "clear");
        build(&mut server);
        let action = server.content.actions.expect("base:light");
        let side = ActionTarget::Block { block: FLUE - IVec3::Z, face: Face::NegZ };
        server.propose(1, action, side, Instant::now());
        assert!(server.smoulders.is_empty(), "a stick does not light it");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(stack(id(&server, "base:torch"), 1));
        server.propose(1, action, side, Instant::now());
        assert_eq!(server.block(FLUE), Some(id(&server, "base:clamp_flue_catching")), "lit");
        assert!(server.smoulders.contains_key(&FLUE), "its core smoulders");
        assert_eq!(server.block(FLUE - IVec3::Z), Some(id(&server, "base:dirt")), "its earth kept");
        assert_eq!(server.players[&1].inventory.slots[0].map(|s| s.count), Some(1), "the torch kept");
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

    /// A clamp whose time is up with its land not loaded tells whoever lit it then, wherever
    /// they are, and once loaded again chars to what it said, telling no more; one changed
    /// while part of it was not loaded is not told done until it is judged loaded.
    #[test]
    fn a_clamp_done_out_of_loaded_land_tells_its_lighter_then() {
        let (mut server, client) = heard("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        let done = |client: &voxmar_protocol::ClientConnection| {
            let mut keys = Vec::new();
            while let Ok(Some(message)) = client.try_recv() {
                if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, args) } = message {
                    keys.push((key, args.iter().find(|(name, _)| name == "count").map(|(_, w)| format!("{w:?}")).unwrap_or_default()));
                }
            }
            keys
        };
        done(&client);
        let column = server.world.remove(ColumnPos::new(0, 0)).unwrap();
        server.day += burn_ticks(&server) + 1.0;
        server.tend_smoulders();
        let told = done(&client);
        assert!(matches!(told.as_slice(), [(key, count)] if key == "multiblock.smoulder_done" && count.contains("36")), "told while away: {told:?}");
        assert_eq!(server.smoulders.len(), 1, "charred only once loaded");
        server.tend_smoulders();
        assert!(done(&client).is_empty(), "told once");
        server.world.insert(ColumnPos::new(0, 0), column);
        server.tend_smoulders();
        assert_eq!(charred(&server), [("base:charcoal_heap".to_owned(), 9)]);
        assert!(done(&client).is_empty(), "not told again once charred");

        // Changed while part of it was not loaded: it may have been opened, so not told.
        let (mut server, client) = heard("base:torch");
        let (corner, flue) = (IVec3::new(28, FLOOR + 1, 8), IVec3::new(30, FLOOR + 2, 10));
        server.players.get_mut(&1).unwrap().position = Vec3::new(30.5, (FLOOR + 1) as f32, 6.5);
        sky(&mut server, "clear");
        build_at(&mut server, corner);
        light_at(&mut server, flue);
        done(&client);
        let east = corner + IVec3::new(4, 0, 2);
        let mut column = server.world.remove(ColumnPos::new(1, 0)).unwrap();
        std::sync::Arc::make_mut(&mut column).set_block(east, BlockId::AIR);
        server.smoulder_changed(east);
        let core = server.world.remove(ColumnPos::new(0, 0)).unwrap();
        server.day += burn_ticks(&server) + 1.0;
        server.tend_smoulders();
        assert!(done(&client).is_empty(), "not told done while it may have been opened");
        server.world.insert(ColumnPos::new(0, 0), core);
        server.world.insert(ColumnPos::new(1, 0), column);
        server.tend_smoulders();
        assert!(server.smoulders.is_empty() && done(&client).is_empty(), "opened once loaded, never told done");
    }

    /// A clamp done out of loaded land while whoever lit it is away tells them when they are
    /// on again, once (the news kept, and saved with the world, meanwhile); told done and
    /// then found opened once loaded, it tells them so.
    #[test]
    fn a_clamp_done_while_its_lighter_is_away_tells_them_when_back() {
        let (mut server, client) = heard("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        let keys = |client: &voxmar_protocol::ClientConnection| {
            let mut keys = Vec::new();
            while let Ok(Some(message)) = client.try_recv() {
                if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, _) } = message {
                    keys.push(key);
                }
            }
            keys
        };
        keys(&client);
        let mut column = server.world.remove(ColumnPos::new(0, 0)).unwrap();
        let away = server.players.remove(&1).expect("the lighter");
        server.day += burn_ticks(&server) + 1.0;
        server.tend_smoulders();
        assert_eq!(server.news_waiting.len(), 1, "kept for them while they are away");
        // Saved with the world, and back with it.
        let saved: Vec<super::Waiting> = ron::from_str(&ron::to_string(&server.news_waiting).unwrap()).unwrap();
        assert_eq!(saved, server.news_waiting);
        server.players.insert(1, away);
        server.tend_smoulders();
        assert!(keys(&client).is_empty(), "told as they come on, not before");
        server.tell_waiting(1);
        assert_eq!(keys(&client), ["multiblock.smoulder_done"], "told once back");
        assert!(server.news_waiting.is_empty());
        server.tend_smoulders();
        server.tell_waiting(1);
        assert!(keys(&client).is_empty(), "told once");
        // Its corner earth gone while no one saw: opened, once loaded, and they hear so.
        std::sync::Arc::make_mut(&mut column).set_block(CORNER, BlockId::AIR);
        server.world.insert(ColumnPos::new(0, 0), column);
        server.tend_smoulders();
        assert!(server.smoulders.is_empty(), "opened once loaded");
        // In what is taken from it, as the done note was: the part-burnt wood its heaps give.
        let early = server.content.multiblocks.get("base:charcoal_clamp").and_then(|m| m.smoulder.as_ref()).map(|s| s.early).unwrap();
        let info = server.content.blocks.info(early);
        let (wood, count) = (server.content.blocks.def(info.drops).name.clone(), 9 * info.drop_count as usize);
        let mut opened = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, args) } = message {
                let arg = |name: &str| args.iter().find(|(n, _)| n == name).map(|(_, w)| w.clone());
                opened.push((key, arg("count"), arg("item")));
            }
        }
        assert_eq!(opened, [("multiblock.smoulder_opened".to_owned(), Some(voxmar_engine::lang::Words::text(count)), Some(voxmar_engine::lang::Words::Block(wood)))], "told it was opened after all");
    }

    /// A kiln gone cold and a clamp done in loaded land while whoever lit them is away are
    /// kept for them, and told as they come on.
    #[test]
    fn news_of_a_kiln_or_clamp_loaded_waits_for_its_lighter_away() {
        let (mut server, client) = heard("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        let name = server.players[&1].name.clone();
        let kiln = server.content.multiblocks.iter().find(|m| !m.home && !m.fuels.is_empty()).expect("a fuelled kiln").name.clone();
        let here = IVec3::new(4, FLOOR + 1, 20);
        server.tell_fires_out();
        let until = server.tick + 40;
        server.lit.insert(here, crate::hearth::Lit { position: here.to_array(), structure: kiln, until, owner: Some(crate::player_key(&name)) });
        while client.try_recv().is_ok_and(|m| m.is_some()) {}
        let away = server.players.remove(&1).expect("the lighter");
        server.tick += 60;
        server.tell_fires_out();
        server.day += burn_ticks(&server) + 1.0;
        server.tend_smoulders();
        assert!(server.smoulders.is_empty(), "charred where it stands, loaded");
        assert_eq!(server.news_waiting.len(), 2, "both kept: {:?}", server.news_waiting);
        server.players.insert(1, away);
        server.tell_waiting(1);
        let mut keys = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, _) } = message {
                keys.push(key);
            }
        }
        assert_eq!(keys, ["multiblock.fire_out", "multiblock.smoulder_done"], "told in the order they came");
    }

    /// News waits for a player away, the newest few, and only theirs is told them.
    #[test]
    fn news_for_a_player_away_keeps_the_newest_and_is_theirs() {
        use super::NEWS_WAITING;
        use voxmar_engine::lang::Words;
        let (mut server, client) = heard("base:torch");
        let ada = crate::player_key(&server.players[&1].name);
        for i in 0..NEWS_WAITING + 3 {
            server.tell_fired("someone_away", Words::text(i));
        }
        server.tell_fired("other", Words::text("not theirs"));
        assert_eq!(server.news_waiting.iter().filter(|w| w.owner == "someone_away").count(), NEWS_WAITING);
        assert_eq!(server.news_waiting.first().map(|w| w.words.clone()), Some(Words::text(3)), "the oldest went first");
        server.tell_waiting(1);
        while let Ok(Some(message)) = client.try_recv() {
            assert!(!matches!(message, voxmar_protocol::ServerMessage::Fired { .. }), "none was {ada}'s: {message:?}");
        }
        assert_eq!(server.news_waiting.len(), NEWS_WAITING + 1, "theirs kept for them");
    }

    /// News for a player whose connection is gone (dropped, not yet noticed, or about to
    /// be replaced by their own rejoin) is kept for them, not lost on the dead socket.
    #[test]
    fn news_for_a_player_whose_connection_went_is_kept() {
        use voxmar_engine::lang::Words;
        let (mut server, client) = heard("base:torch");
        let ada = crate::player_key(&server.players[&1].name);
        drop(client);
        server.tell_fired(&ada, Words::text("your kiln is cold"));
        server.tell_fired(&ada, Words::text("your clamp is done"));
        let kept = [super::Waiting { owner: ada.clone(), words: Words::text("your kiln is cold") }, super::Waiting { owner: ada, words: Words::text("your clamp is done") }];
        assert_eq!(server.news_waiting, kept);
        // Told on a hello whose connection is gone again: still kept, in order.
        server.tell_waiting(1);
        assert_eq!(server.news_waiting, kept, "not lost on a send that failed");
    }

    /// A kiln gone cold in the first look after the server starts is told: what was lit
    /// when the world was saved comes back only if it still burns, so whatever goes out now
    /// went out in play.
    #[test]
    fn a_kiln_gone_cold_in_the_first_look_after_a_start_is_told() {
        let (mut server, client) = heard("base:torch");
        let name = server.players[&1].name.clone();
        let kiln = server.content.multiblocks.iter().find(|m| !m.home && !m.fuels.is_empty()).expect("a fuelled kiln").name.clone();
        let here = IVec3::new(4, FLOOR + 1, 20);
        server.lit.insert(here, crate::hearth::Lit { position: here.to_array(), structure: kiln, until: server.tick + 1, owner: Some(crate::player_key(&name)) });
        while client.try_recv().is_ok_and(|m| m.is_some()) {}
        server.tick += 2;
        server.tell_fires_out();
        let mut keys = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Fired { words: voxmar_engine::lang::Words::Key(key, _) } = message {
                keys.push(key);
            }
        }
        assert_eq!(keys, ["multiblock.fire_out"], "told the first time it looked");
    }

    /// A player coming on (their hello answered) hears the news kept for them, and only
    /// theirs.
    #[test]
    fn a_player_saying_hello_hears_the_news_kept_for_them() {
        use voxmar_engine::lang::Words;
        use voxmar_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, local_pair};
        let mut server = crate::test_server();
        let (connections, incoming) = crossbeam_channel::unbounded();
        server.incoming = incoming;
        server.tell_fired(&crate::player_key("bob"), Words::text("your clamp is done"));
        server.tell_fired("someone_else", Words::text("not bob's"));
        let (client, server_side) = local_pair();
        connections.send(server_side).unwrap();
        client.send(ClientMessage::Hello { name: "bob".into(), protocol: PROTOCOL_VERSION, content: server.content.fingerprint(), admin_token: None });
        server.accept_connections();
        server.handle_messages();
        let (mut welcomed, mut fired) = (false, Vec::new());
        while let Ok(Some(message)) = client.try_recv() {
            match message {
                ServerMessage::Welcome { .. } => welcomed = true,
                ServerMessage::Fired { words } => fired.push(words),
                _ => {}
            }
        }
        assert!(welcomed, "let in");
        assert_eq!(fired, [Words::text("your clamp is done")], "told theirs as they came on");
        assert_eq!(server.news_waiting.len(), 1, "the other's still kept");
    }

    /// Its wood swapped for stone while it smoulders opens it, and the stone is left as it
    /// is: only wood chars.
    #[test]
    fn what_was_set_in_its_woods_place_is_not_charred() {
        let mut server = fixture("base:torch");
        sky(&mut server, "clear");
        build(&mut server);
        light(&mut server);
        server.set_block(CORNER + IVec3::new(1, 0, 1), id(&server, "base:stone"));
        assert!(server.smoulders.is_empty(), "opened");
        assert_eq!(charred(&server), [("base:charred_wood_heap".to_owned(), 8), ("base:stone".to_owned(), 1)]);
    }

    /// A piece broken while its land is not loaded opens it once all of it is loaded again,
    /// never waiting out its time to char well: a clamp across two columns, its flue and
    /// wood loaded, its east earth broken in the column that is not, is not done when its
    /// time is up.
    #[test]
    fn a_clamp_broken_while_unloaded_is_opened_once_loaded() {
        let mut server = fixture("base:torch");
        // Its east earth ring (x 32) alone in the next column.
        let (corner, flue) = (IVec3::new(28, FLOOR + 1, 8), IVec3::new(30, FLOOR + 2, 10));
        server.players.get_mut(&1).unwrap().position = Vec3::new(30.5, (FLOOR + 1) as f32, 6.5);
        sky(&mut server, "clear");
        build_at(&mut server, corner);
        light_at(&mut server, flue);
        assert_eq!(server.smoulders.len(), 1, "lit");
        server.day += burn_ticks(&server) * 3.0 / 8.0;
        server.tend_smoulders();
        let east = corner + IVec3::new(4, 0, 2);
        let mut column = server.world.remove(ColumnPos::new(1, 0)).unwrap();
        assert!(server.block(flue).is_some() && server.block(corner + IVec3::new(3, 0, 3)).is_some(), "only the east earth is unloaded");
        std::sync::Arc::make_mut(&mut column).set_block(east, BlockId::AIR);
        server.smoulder_changed(east);
        server.day += burn_ticks(&server);
        server.tend_smoulders();
        assert_eq!(server.smoulders.len(), 1, "not judged with part of it unloaded, its time up");
        server.world.insert(ColumnPos::new(1, 0), column);
        server.tend_smoulders();
        assert!(server.smoulders.is_empty(), "opened once loaded");
        assert_eq!(charred_at(&server, corner), [("base:charred_wood_heap".to_owned(), 9)]);
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
