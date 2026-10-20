//! Fire in the thatch (see `voxmar_engine::fire`): a block that catches, set beside a flame,
//! catches after a while, burns, and is gone, lighting what catches beside it as it burns.
//!
//! Only block changes come here (a player's, a command's, a fire's own), never a column as
//! the world made it, so a village's thatch by its torches stands until someone sets a
//! flame to it. What waits is timed in ticks and saved with the world, so leaving and coming
//! back neither puts a fire out nor lets it burn twice.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::glam::IVec3;
use voxmar_protocol::TICKS_PER_SECOND;

use crate::Server;

/// Something that will happen to a block by fire at tick `due`: it catches (`burning`
/// false), or what burns is gone (`burning` true). `origin` is whose fire it is (a player's
/// key, `player_key`; see `Server::fire_origin`), carried on as it spreads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blaze {
    pub pos: [i32; 3],
    pub due: u64,
    pub burning: bool,
    /// None in a save from before fires were kept by owner.
    #[serde(default, deserialize_with = "keyed")]
    pub origin: Option<String>,
}

/// A saved fire's or hearth's player as their key (`player_key`): saves from before it was
/// kept so held the name as typed.
pub(crate) fn keyed<'de, D: serde::Deserializer<'de>>(from: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(from)?.as_deref().map(crate::player_key))
}

/// Seconds between looks at what is due.
const EVERY: u64 = TICKS_PER_SECOND as u64;

fn ticks(seconds: f32) -> u64 {
    (seconds * TICKS_PER_SECOND as f32).ceil() as u64
}

impl Server {
    /// Every cell a flame at `from` reaches within `steps`, with how many steps: out a face
    /// at a time, on only through cells that are not solid, so stone between a flame and
    /// thatch shields it, and a chimney about a hearth's flue keeps it from the roof. A cell
    /// reached is visited whether solid or not (the thatch itself). Bounded by `steps`, at
    /// most 4: a few hundred cells at worst.
    fn reached(&self, from: IVec3, steps: i32, mut visit: impl FnMut(IVec3, i32)) {
        let mut seen = rustc_hash::FxHashSet::default();
        seen.insert(from);
        let mut ring = vec![from];
        for step in 1..=steps {
            let mut next = Vec::new();
            for cell in ring {
                for side in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                    let pos = cell + side;
                    if !seen.insert(pos) {
                        continue;
                    }
                    let Some(block) = self.block(pos) else { continue };
                    visit(pos, step);
                    if !self.content.blocks.info(block).solid {
                        next.push(pos);
                    }
                }
            }
            ring = next;
        }
    }

    /// Whether a flame reaches `pos`: a block that is one (a burning one too) within the
    /// fire's reach, or a lit hearth or kiln within its own, through open cells.
    fn exposed(&self, pos: IVec3) -> bool {
        let fire = &self.content.fire;
        let (reach, hearth) = (fire.rules.reach, fire.rules.hearth_reach);
        let mut found = false;
        self.reached(pos, reach.max(hearth), |cell, step| {
            found |= (step <= reach && self.block(cell).is_some_and(|b| fire.flame(b)))
                || (step <= hearth && self.lit.get(&cell).is_some_and(|lit| lit.until > self.tick));
        });
        found
    }

    /// Whether a fire of `origin`'s is kept from `pos`: with fire.ron's
    /// `homes_catch_only_from_owner`, a block in someone's home box (a home hearth lit by
    /// them, or an online player's home) catches only from its owner's fire. A stranger's
    /// torch fizzles at your walls, a neighbour's burning shed stops at your home's edge,
    /// and your own careless torch still burns your own roof. Where homes overlap the block
    /// is kept from every fire but one all of them own, so lighting a hearth next door (or
    /// making yours at someone's hearth) never lets your fire into their home; a hearth
    /// with no owner (lit before fires were kept by owner) keeps out every fire.
    fn kept_out(&self, pos: IVec3, origin: &Option<String>) -> bool {
        if !self.content.fire.rules.homes_catch_only_from_owner {
            return false;
        }
        let (reach, up, down) = (self.survival.home_reach, self.survival.home_up, self.survival.home_down);
        let within = |home: IVec3| {
            let d = pos - home;
            d.x.abs() <= reach && d.z.abs() <= reach && d.y <= up && d.y >= -down
        };
        let multiblocks = &self.content.multiblocks;
        // Each home's box is the one its owner feels at home in (`in_home_box`): about the
        // cell home is at, the one over a hearthstone. A home hearth is its owner's lit or
        // cold, also with a piece of it broken (`hearth_owners`), so breaking a stone of
        // someone's hearth never opens their home to your fire.
        let lit = self.lit.iter().filter(|(_, lit)| lit.until > self.tick && multiblocks.get(&lit.structure).is_some_and(|m| m.home)).map(|(&core, lit)| (core, lit.owner.as_deref()));
        let hearths = lit.chain(self.hearth_owners.iter().map(|(&core, owner)| (core, owner.as_deref()))).map(|(core, owner)| (core + IVec3::Y, owner));
        let beds = self.players.values().filter_map(|p| Some((p.home_spot(&self.lit)?.floor().as_ivec3(), Some(p.name.as_str()))));
        // Where homes overlap (housemates, a neighbour's hearth close by), the block is in
        // each of them, and a fire reaches it only if every one of them is its owner's.
        // Players are told apart by key, so a name typed in another case is the same player.
        let origin = origin.as_deref().map(crate::player_key);
        hearths.chain(beds).filter(|&(home, _)| within(home)).any(|(_, owner)| owner.is_none() || owner.map(crate::player_key) != origin)
    }

    /// Starts `pos` waiting to catch, unless something already waits there or the fire
    /// (`fire_origin`'s) is kept from it.
    fn smoulder(&mut self, pos: IVec3) {
        let due = self.tick + ticks(self.content.fire.rules.catch_seconds);
        if !self.blazes.contains_key(&pos) && !self.kept_out(pos, &self.fire_origin) {
            self.blazes.insert(pos, Blaze { pos: pos.to_array(), due, burning: false, origin: self.fire_origin.clone() });
            self.blaze_risked.push(pos);
        }
    }

    /// The block at `pos` changed to `block`: what catches beside a flame starts waiting to
    /// catch; a flame set down sets what catches about it waiting; the burning block starts
    /// burning down; anything else ends what waited there. Every change comes here, a flood's
    /// too, so a block that neither catches nor burns goes no further than one removal.
    pub(crate) fn blaze_changed(&mut self, pos: IVec3, block: BlockId) {
        let fire = self.content.fire.clone();
        if !fire.on() {
            return;
        }
        let (catches, flame) = (fire.catches(block), fire.flame(block));
        if !catches && Some(block) != fire.burning {
            self.blazes.remove(&pos);
        }
        if catches && self.exposed(pos) {
            self.smoulder(pos);
        }
        if Some(block) == fire.burning {
            let due = self.tick + ticks(fire.rules.burn_seconds);
            // Whose fire caught here, else whoever set the burning block down.
            let origin = self.blazes.get(&pos).map_or(self.fire_origin.clone(), |b| b.origin.clone());
            self.blazes.insert(pos, Blaze { pos: pos.to_array(), due, burning: true, origin: origin.clone() });
            if flame {
                let set_by = std::mem::replace(&mut self.fire_origin, origin);
                self.light_about(pos, fire.rules.reach);
                self.fire_origin = set_by;
            }
        } else if flame {
            self.light_about(pos, fire.rules.reach);
        }
    }

    /// Sets what catches that a flame at `centre` reaches within `reach` waiting to catch.
    fn light_about(&mut self, centre: IVec3, reach: i32) {
        let fire = self.content.fire.clone();
        let mut catching = Vec::new();
        self.reached(centre, reach, |pos, _| {
            if self.block(pos).is_some_and(|b| fire.catches(b)) {
                catching.push(pos);
            }
        });
        for pos in catching {
            self.smoulder(pos);
        }
    }

    /// Tells `agent`, the first time this session, why what they just did set something
    /// smouldering: it was their change, so they are there to see it and put it right.
    pub(crate) fn blaze_hint(&mut self, agent: voxmar_protocol::PlayerId) {
        if self.blaze_risked.is_empty() {
            return;
        }
        self.blaze_risked.clear();
        let Some(player) = self.players.get_mut(&agent) else { return };
        if !player.told_fire {
            player.told_fire = true;
            player.connection.send(voxmar_protocol::ServerMessage::Chat { kind: voxmar_protocol::ChatKind::Info, text: voxmar_engine::lang::Words::key("fire.smoulders") });
        }
    }

    /// The blocks that smoulder (wait to catch), in order of place.
    pub(crate) fn smouldering(&self) -> Vec<IVec3> {
        let mut at: Vec<IVec3> = self.blazes.values().filter(|b| !b.burning).map(|b| IVec3::from_array(b.pos)).collect();
        at.sort_by_key(|p| p.to_array());
        at
    }

    /// Tells every player which blocks smoulder, when that changed since they were last told
    /// (a player joining is told on joining).
    pub(crate) fn send_smoulders(&mut self) {
        let at = self.smouldering();
        if at != self.smoulders_sent {
            self.broadcast(voxmar_protocol::ServerMessage::Smoulders { at: at.clone() });
            self.smoulders_sent = at;
        }
    }

    /// A hearth or kiln was lit at `core` by `lighter`: what catches near it starts waiting
    /// to catch, as the lighter's fire (not its owner's, so a stranger relighting someone's
    /// hearth never sets their home alight).
    pub(crate) fn hearth_blaze(&mut self, core: IVec3, lighter: Option<String>) {
        if self.content.fire.on() && self.content.fire.rules.hearth_reach > 0 {
            let set_by = std::mem::replace(&mut self.fire_origin, lighter);
            self.light_about(core, self.content.fire.rules.hearth_reach);
            self.fire_origin = set_by;
        }
    }

    /// A home hearth relit for `agent` as they join (an older world's): nothing starts to
    /// burn, since they changed nothing, but they are told once of the nearest thatch or
    /// wattle its fire reaches, and what to do about it.
    pub(crate) fn hearth_hint(&mut self, agent: voxmar_protocol::PlayerId, core: IVec3) {
        let fire = self.content.fire.clone();
        if !fire.on() || self.players.get(&agent).is_none_or(|p| p.told_fire) {
            return;
        }
        let mut nearest: Option<(i32, IVec3)> = None;
        self.reached(core, fire.rules.hearth_reach, |pos, step| {
            if self.block(pos).is_some_and(|b| fire.catches(b)) && nearest.is_none_or(|(s, p)| (step, pos.to_array()) < (s, p.to_array())) {
                nearest = Some((step, pos));
            }
        });
        let Some((steps, pos)) = nearest else { return };
        let block = self.content.blocks.def(self.block(pos).unwrap()).name.clone();
        let player = self.players.get_mut(&agent).unwrap();
        player.told_fire = true;
        let text = voxmar_engine::lang::Words::key("fire.near_hearth")
            .with("block", voxmar_engine::lang::Words::key(&voxmar_engine::lang::def_key("block", &block)))
            .with("steps", voxmar_engine::lang::Words::text(steps));
        player.connection.send(voxmar_protocol::ServerMessage::Chat { kind: voxmar_protocol::ChatKind::Info, text });
    }

    /// What is due: a block still beside a flame catches, one no longer is let be; what has
    /// burned is gone. In order of place, catching before burning out, so the same world
    /// burns the same way every time.
    pub(crate) fn blaze(&mut self) {
        // What a change set at risk is told to whoever made it in the same tick, or no one.
        self.blaze_risked.clear();
        if !self.tick.is_multiple_of(EVERY) {
            return;
        }
        self.send_smoulders();
        if self.blazes.is_empty() {
            return;
        }
        let tick = self.tick;
        let mut due: Vec<Blaze> = self.blazes.values().filter(|b| b.due <= tick).cloned().collect();
        due.sort_by_key(|b| (b.burning, b.pos));
        let set_by = self.fire_origin.take();
        let fire = self.content.fire.clone();
        let (Some(burning), Some(ash)) = (fire.burning, fire.ash) else { return };
        for blaze in due {
            let pos = IVec3::from_array(blaze.pos);
            // A column away is left to wait until it is back.
            let Some(here) = self.block(pos) else { continue };
            self.blazes.remove(&pos);
            if !blaze.burning && fire.catches(here) && self.exposed(pos) && !self.kept_out(pos, &blaze.origin) {
                // It burns as the fire that caught it, and spreads as that fire.
                self.blazes.insert(pos, blaze.clone());
                self.fire_origin = blaze.origin.clone();
                self.set_block(pos, burning);
                self.fire_origin = None;
            } else if blaze.burning && here == burning {
                self.set_block(pos, ash);
            }
        }
        self.fire_origin = set_by;
        self.send_smoulders();
        // A fire's own spreading is no one's to be told of.
        self.blaze_risked.clear();
    }

    /// What waits on fire, to save, in order of place.
    pub(crate) fn saved_blazes(&self) -> Vec<Blaze> {
        let mut saved: Vec<Blaze> = self.blazes.values().cloned().collect();
        saved.sort_by_key(|b| b.pos);
        saved
    }
}

/// Saved fires back by place.
pub(crate) fn restore(saved: &[Blaze]) -> FxHashMap<IVec3, Blaze> {
    saved.iter().map(|b| (IVec3::from_array(b.pos), b.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::coords::ColumnPos;

    /// A floor in the spawn column, high over the ground, to build on.
    fn floor(server: &mut Server) -> IVec3 {
        // On a second, so what is due a whole number of seconds on is looked at that tick.
        server.tick = server.tick.next_multiple_of(EVERY);
        let at = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let stone = server.content.blocks.expect("base:stone");
        for x in -4..=4 {
            for z in -4..=4 {
                server.set_block(at + IVec3::new(x, 0, z), stone);
            }
        }
        at + IVec3::Y
    }

    /// Runs the server's fire for `seconds`.
    fn run(server: &mut Server, seconds: f32) {
        for _ in 0..ticks(seconds) {
            server.tick += 1;
            server.blaze();
        }
    }

    fn at(server: &Server, pos: IVec3) -> String {
        server.content.blocks.def(server.block(pos).unwrap()).name.clone()
    }

    /// A torch set beside a wattle panel sets it alight once it has stood there the catching
    /// time, and the fire runs on to the panel beside it before the first is gone; then both
    /// burn away to nothing. The same world burns the same way, tick for tick.
    #[test]
    fn a_torch_sets_wattle_alight_and_the_fire_runs_on() {
        let burn = |server: &mut Server| {
            let base = floor(server);
            let (wattle, torch) = (server.content.blocks.expect("base:wattle_panel"), server.content.blocks.expect("base:torch"));
            let (first, second, far) = (base, base + IVec3::X, base + IVec3::new(3, 0, 0));
            for pos in [first, second, far] {
                server.set_block(pos, wattle);
            }
            assert!(server.blazes.is_empty(), "nothing burns with no flame");
            server.set_block(base - IVec3::X, torch);
            let rules = server.content.fire.rules.clone();
            run(server, rules.catch_seconds - 1.0);
            assert_eq!(at(server, first), "base:wattle_panel", "not yet");
            run(server, 1.0);
            assert_eq!(at(server, first), "base:blaze", "caught");
            assert_eq!(at(server, second), "base:wattle_panel", "the second was out of the torch's reach");
            run(server, rules.catch_seconds);
            assert_eq!(at(server, second), "base:blaze", "the fire ran on");
            // Midway, what stands about the torch and what waits on fire, to compare.
            let midway: Vec<String> = (-1..=3).map(|x| at(server, base + IVec3::X * x)).collect();
            let waiting = server.saved_blazes();
            run(server, rules.burn_seconds);
            assert_eq!(at(server, first), "core:air", "burned away");
            assert_eq!(at(server, far), "base:wattle_panel", "three blocks off, never in reach of a flame");
            run(server, rules.burn_seconds);
            assert_eq!(at(server, second), "core:air");
            assert!(server.blazes.is_empty(), "nothing left waiting");
            (midway, waiting, server.tick)
        };
        let (mut a, mut b) = (crate::test_server(), crate::test_server());
        assert_eq!(burn(&mut a), burn(&mut b));
    }

    /// Taken away in time, the torch lights nothing; broken, a blaze is put out; a lit
    /// hearth near enough sets thatch alight too.
    #[test]
    fn a_flame_taken_away_or_put_out_burns_nothing() {
        let mut server = crate::test_server();
        let base = floor(&mut server);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (wattle, torch, air) = (block(&server, "base:wattle_panel"), block(&server, "base:torch"), BlockId::AIR);
        let rules = server.content.fire.rules.clone();
        server.set_block(base, wattle);
        server.set_block(base + IVec3::X, torch);
        run(&mut server, rules.catch_seconds - 1.0);
        server.set_block(base + IVec3::X, air);
        run(&mut server, 2.0);
        assert_eq!(at(&server, base), "base:wattle_panel", "the torch went before it caught");
        assert!(server.blazes.is_empty());
        // Burning, broken by hand: out, and the panel beside it never catches.
        server.set_block(base + IVec3::X, wattle);
        server.set_block(base, block(&server, "base:blaze"));
        run(&mut server, 1.0);
        server.set_block(base, air);
        run(&mut server, rules.catch_seconds + rules.burn_seconds);
        assert_eq!(at(&server, base + IVec3::X), "base:wattle_panel", "put out in time");
        // A hearth lit two blocks off sets it alight (Ada's, in her own home: one no one
        // owns keeps every fire out, its own too).
        let core = base + IVec3::new(rules.hearth_reach + 1, 0, 0);
        server.lit.insert(core, crate::hearth::Lit { position: core.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: Some("Ada".into()) });
        server.hearth_blaze(core, Some("ada".into()));
        run(&mut server, rules.catch_seconds);
        assert_eq!(at(&server, base + IVec3::X), "base:blaze", "the hearth's fire reached it");
    }

    /// A Homestead Hearth on the floor, with a player 1 who sees what they are sent;
    /// returns its core and the player's end of the connection.
    fn hearth(server: &mut Server) -> (IVec3, voxmar_protocol::ClientConnection) {
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        let core = floor(server);
        build_hearth(server, core);
        (core, client)
    }

    /// A Homestead Hearth's blocks about `core`.
    fn build_hearth(server: &mut Server, core: IVec3) {
        let (stone, chimney) = (server.content.blocks.expect("base:cobblestone"), server.content.blocks.expect("base:chimney"));
        for x in -1..=1 {
            for z in -1..=1 {
                server.set_block(core + IVec3::new(x, 0, z), stone);
            }
        }
        server.set_block(core, server.content.blocks.expect("base:hearthstone"));
        for y in 1..=2 {
            server.set_block(core + IVec3::new(-1, y, -1), chimney);
        }
    }

    /// Joins player `id` as `name` the way a client does (`hello`), and keeps their end.
    fn join(server: &mut Server, id: crate::PlayerId, name: &str) -> voxmar_protocol::ClientConnection {
        let (client, connection) = voxmar_protocol::local_pair();
        let player = crate::player::Player::new(id, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        server.players.insert(id, player);
        server.hello(id, name.into(), voxmar_protocol::PROTOCOL_VERSION, server.content.fingerprint(), None);
        client
    }

    /// What `who`'s own message sets before their change at `at` lands (`handle_message`).
    fn by_player(server: &mut Server, who: &str, at: IVec3, block: BlockId) {
        server.fire_origin = Some(crate::player_key(who));
        server.changed_by = server.fire_origin.clone();
        server.set_block(at, block);
        server.fire_origin = None;
        server.changed_by = None;
    }

    /// Whether `who`'s torch set at `beside` sets `wall` smouldering.
    fn torch_smoulders(server: &mut Server, who: &str, beside: IVec3, wall: IVec3) -> bool {
        server.fire_origin = Some(crate::player_key(who));
        server.set_block(beside, server.content.blocks.expect("base:torch"));
        let caught = server.blazes.remove(&wall).is_some();
        server.set_block(beside, BlockId::AIR);
        server.fire_origin = None;
        caught
    }

    /// A thatch roof two blocks over the hearth, round its chimney: over the fire itself
    /// thatch, or stone lining the flue where the roof is open over it.
    fn roof(server: &mut Server, core: IVec3, flue: bool) {
        let (thatch, stone) = (server.content.blocks.expect("base:thatch_roof"), server.content.blocks.expect("base:cobblestone"));
        for x in -2..=2 {
            for z in -2..=2 {
                if (x, z) != (-1, -1) {
                    let lined = flue && (x, z) == (0, 0);
                    server.set_block(core + IVec3::new(x, 2, z), if lined { stone } else { thatch });
                }
            }
        }
    }

    fn told(client: &voxmar_protocol::ClientConnection) -> Vec<voxmar_protocol::ServerMessage> {
        std::iter::from_fn(|| client.try_recv().ok().flatten()).collect()
    }

    /// Lighting a hearth with thatch two blocks over it sets the thatch smouldering: the
    /// player who lit it is told why, once, every player is told where, and only once the
    /// catching time has passed (time enough to break it by hand) does it catch.
    #[test]
    fn a_hearth_lit_under_thatch_smoulders_then_catches() {
        let mut server = crate::test_server();
        let (core, client) = hearth(&mut server);
        let above = core + IVec3::Y * 2;
        server.set_block(above, server.content.blocks.expect("base:thatch_roof"));
        assert!(server.blazes.is_empty(), "a cold hearth puts nothing at risk");
        told(&client);
        server.light(1, core);
        assert!(server.lit.contains_key(&core), "the hearth was not lit");
        assert_eq!(server.blazes.get(&above).map(|b| b.burning), Some(false), "the thatch over the fire was not smouldering");
        server.blaze_hint(1);
        let hints = |client: &voxmar_protocol::ClientConnection| {
            told(client).into_iter().filter(|m| matches!(m, voxmar_protocol::ServerMessage::Chat { text, .. } if matches!(text, voxmar_engine::lang::Words::Key(k, _) if k == "fire.smoulders"))).count()
        };
        assert_eq!(hints(&client), 1, "the lighter was told why the thatch smoulders");
        let rules = server.content.fire.rules.clone();
        run(&mut server, 1.0);
        let smoulders: Vec<Vec<IVec3>> = told(&client).into_iter().filter_map(|m| match m { voxmar_protocol::ServerMessage::Smoulders { at } => Some(at), _ => None }).collect();
        assert_eq!(smoulders, [vec![above]], "players were told where it smoulders");
        run(&mut server, rules.catch_seconds - 2.0);
        assert_eq!(at(&server, above), "base:thatch_roof", "caught before its time");
        run(&mut server, 1.0);
        assert_eq!(at(&server, above), "base:blaze", "caught");
        // A second risk the same session: not told again.
        server.set_block(core + IVec3::new(1, 1, 0), server.content.blocks.expect("base:thatch_roof"));
        server.blaze_hint(1);
        assert_eq!(hints(&client), 0, "told twice");
    }

    /// An older world's home hearth, relit for its player as they join under a thatch roof:
    /// nothing starts to smoulder (they changed nothing). One no one is known to own is relit
    /// as no one's, says nothing, and stays no one's when lit by hand, so it never catches.
    /// One known to be Ada's is relit as hers: she is told once which block its fire reaches
    /// and how far off, and her next lighting by hand does set it smouldering.
    #[test]
    fn a_hearth_relit_on_joining_names_the_thatch_and_burns_nothing() {
        let mut server = crate::test_server();
        let (core, client) = hearth(&mut server);
        let above = core + IVec3::Y * 2;
        server.set_block(above, server.content.blocks.expect("base:thatch_roof"));
        let hints = |client: &voxmar_protocol::ClientConnection| -> Vec<_> {
            told(client).into_iter().filter_map(|m| match m {
                voxmar_protocol::ServerMessage::Chat { text: voxmar_engine::lang::Words::Key(k, args), .. } if k == "fire.near_hearth" => Some(args),
                _ => None,
            }).collect()
        };
        for known in [false, true] {
            server.lit.remove(&core);
            server.hearth_owners.clear();
            if known {
                server.hearth_owners.insert(core, Some("ada".into()));
            }
            let player = server.players.get_mut(&1).unwrap();
            player.name = "Ada".into();
            (player.home, player.kindled) = (Some(core.as_vec3() + voxmar_engine::glam::Vec3::new(0.5, 1.0, 0.5)), false);
            told(&client);
            server.kindle_home(1);
            assert!(server.lit.contains_key(&core), "the old home hearth was not lit again");
            assert_eq!(server.lit[&core].owner.as_deref(), known.then_some("ada"), "relit on joining as whose");
            assert!(server.blazes.is_empty(), "relit on joining set the thatch smouldering");
            let said = hints(&client);
            if !known {
                assert!(said.is_empty(), "told of a fire that keeps out its own: {said:?}");
                server.lit.remove(&core);
                server.light(1, core);
                assert_eq!(server.lit[&core].owner, None, "lighting it by hand took the old hearth");
                assert!(server.blazes.is_empty(), "the hearth no one owns set its thatch smouldering");
                continue;
            }
            assert_eq!(said.len(), 1, "told once of what the fire reaches");
            let said = format!("{:?}", said[0]);
            assert!(said.contains("block.base.thatch_roof") && said.contains("\"2\""), "names the thatch two steps off: {said}");
            server.lit.remove(&core);
            server.light(1, core);
            assert_eq!(server.blazes.get(&above).map(|b| b.burning), Some(false), "lit by hand, the thatch smoulders");
        }
    }

    /// Ada lights her home hearth and leaves; Bo breaks a stone of its ring (its fire goes
    /// out) and his torch at her wall still fizzles; he puts the stone back and lights it:
    /// it is still Ada's. Breaking and laying the hearthstone himself changes nothing either;
    /// only Ada breaking it makes it no one's, for whoever builds there next.
    #[test]
    fn mending_and_relighting_someone_elses_hearth_never_takes_it() {
        let mut server = crate::test_server();
        let (core, _client) = hearth(&mut server);
        server.players.remove(&1);
        let _ada = join(&mut server, 1, "Ada");
        server.light(1, core);
        server.leave(1);
        let _bo = join(&mut server, 2, "Bo");
        let wall = core + IVec3::new(4, 0, 0);
        server.set_block(wall, server.content.blocks.expect("base:wattle_panel"));
        // What Bo's message sets before his change lands (`handle_message`).
        let by = by_player;
        let (ring, stone) = (core + IVec3::new(1, 0, 1), server.content.blocks.expect("base:cobblestone"));
        by(&mut server, "Bo", ring, BlockId::AIR);
        assert!(!server.lit.contains_key(&core), "the broken ring left the fire burning");
        assert!(!torch_smoulders(&mut server, "Bo", wall + IVec3::Z, wall), "Bo's torch at Ada's wall, her hearth broken");
        by(&mut server, "Bo", ring, stone);
        server.light(2, core);
        assert_eq!(server.lit[&core].owner.as_deref(), Some("ada"), "Bo's relighting took Ada's hearth");
        assert!(!torch_smoulders(&mut server, "Bo", wall + IVec3::Z, wall), "Bo's torch at Ada's wall");
        assert_eq!(crate::hearth::HearthOwner::saved(&server.hearth_owners), [crate::hearth::HearthOwner { position: core.to_array(), owner: Some("ada".into()) }]);
        // Bo breaking the hearthstone and laying it again: still Ada's.
        let hearthstone = server.content.blocks.expect("base:hearthstone");
        by(&mut server, "Bo", core, BlockId::AIR);
        assert!(!torch_smoulders(&mut server, "Bo", wall + IVec3::Z, wall), "Bo's torch at Ada's wall, her hearthstone gone");
        by(&mut server, "Bo", core, hearthstone);
        server.light(2, core);
        assert_eq!(server.lit[&core].owner.as_deref(), Some("ada"), "Bo's new hearthstone took Ada's hearth");
        // Ada breaking it herself: it is whoever lights it next.
        by(&mut server, "Ada", core, BlockId::AIR);
        by(&mut server, "Bo", core, hearthstone);
        server.light(2, core);
        assert_eq!(server.lit[&core].owner.as_deref(), Some("bo"), "a new hearth on the spot is its builder's");
        // Bo lighting a home hearth elsewhere: this one is no longer kept as his.
        let elsewhere = core + IVec3::new(0, 0, 12);
        build_hearth(&mut server, elsewhere);
        server.light(2, elsewhere);
        assert_eq!(crate::hearth::HearthOwner::saved(&server.hearth_owners), [crate::hearth::HearthOwner { position: elsewhere.to_array(), owner: Some("bo".into()) }], "one home hearth a player");
    }

    /// Ada lights her home hearth and leaves; Bo lays thatch by it, breaks and mends a stone
    /// of its ring and lights it again: the fire is his as it is lit, so nothing of hers
    /// smoulders, however close.
    #[test]
    fn relighting_someone_elses_hearth_never_sets_their_home_alight() {
        let mut server = crate::test_server();
        let (core, _client) = hearth(&mut server);
        server.players.remove(&1);
        let _ada = join(&mut server, 1, "Ada");
        server.light(1, core);
        server.leave(1);
        let _bo = join(&mut server, 2, "Bo");
        let (thatch, stone) = (server.content.blocks.expect("base:thatch_roof"), server.content.blocks.expect("base:cobblestone"));
        let laid = core + IVec3::Y * 2;
        by_player(&mut server, "Bo", laid, thatch);
        assert!(server.blazes.is_empty(), "Bo's thatch by Ada's burning hearth smoulders");
        let ring = core + IVec3::new(1, 0, 1);
        by_player(&mut server, "Bo", ring, BlockId::AIR);
        by_player(&mut server, "Bo", ring, stone);
        server.fire_origin = Some("bo".into());
        server.light(2, core);
        server.fire_origin = None;
        assert_eq!(server.lit[&core].owner.as_deref(), Some("ada"));
        assert!(server.blazes.is_empty(), "Bo's relighting set Ada's home smouldering: {:?}", server.blazes.keys().collect::<Vec<_>>());
        let catch = server.content.fire.rules.catch_seconds;
        run(&mut server, catch + 1.0);
        assert_eq!(at(&server, laid), "base:thatch_roof", "the thatch caught");
    }

    /// Ada's own fire spreading over where her hearthstone was (Bo broke it and laid thatch
    /// there) burns it, but her hearth stays hers: only her own hand breaking the hearthstone
    /// forgets whose it is, not her fire's.
    #[test]
    fn a_fire_spreading_never_forgets_whose_a_hearth_is() {
        let mut server = crate::test_server();
        let (core, _client) = hearth(&mut server);
        server.players.remove(&1);
        let _ada = join(&mut server, 1, "Ada");
        server.light(1, core);
        server.leave(1);
        let _bo = join(&mut server, 2, "Bo");
        by_player(&mut server, "Bo", core, BlockId::AIR);
        let (thatch, torch) = (server.content.blocks.expect("base:thatch_roof"), server.content.blocks.expect("base:torch"));
        by_player(&mut server, "Bo", core, thatch);
        // Ada's torch by it: her fire, in her own home.
        let beside = core + IVec3::Y;
        by_player(&mut server, "Ada", beside, torch);
        assert!(server.blazes.contains_key(&core), "Ada's own torch kept from the thatch in her home");
        let catch = server.content.fire.rules.catch_seconds;
        run(&mut server, catch + 1.0);
        assert_eq!(at(&server, core), "base:blaze", "the thatch caught");
        assert_eq!(server.hearth_owners.get(&core), Some(&Some("ada".to_owned())), "her fire burning there forgot whose the hearth is");
    }

    /// Ada mining her own hearthstone forgets whose the hearth is, also when the mining
    /// falls due in the run loop rather than as a message of hers arrives; Bo mining it the
    /// same way does not.
    #[test]
    fn mining_your_own_hearthstone_forgets_it_however_late_it_completes() {
        use voxmar_engine::action::ActionTarget;
        use voxmar_engine::coords::Face;
        for (who, forgets) in [(1, true), (2, false)] {
            let mut server = crate::test_server();
            let (core, _client) = hearth(&mut server);
            server.players.remove(&1);
            let _ada = join(&mut server, 1, "Ada");
            server.light(1, core);
            let _bo = join(&mut server, 2, "Bo");
            server.players.get_mut(&who).unwrap().position = core.as_vec3() + voxmar_engine::glam::Vec3::new(2.5, 0.0, 0.5);
            let (now, mine) = (std::time::Instant::now(), server.content.actions.expect("core:mine"));
            server.propose(who, mine, ActionTarget::Block { block: core, face: Face::PosX }, now);
            server.complete_actions(now + std::time::Duration::from_secs(30));
            assert_eq!(at(&server, core), "core:air", "the hearthstone was not mined");
            assert_eq!(server.hearth_owners.contains_key(&core), !forgets, "mined by player {who}");
        }
    }

    /// Ada, an admin, setting her own lit hearth's hearthstone to air by command: a command
    /// is no one's doing, so the hearth is still hers.
    #[test]
    fn a_command_over_your_hearthstone_forgets_nothing() {
        let mut server = crate::test_server();
        let (core, _client) = hearth(&mut server);
        server.players.remove(&1);
        let _ada = join(&mut server, 1, "Ada");
        server.players.get_mut(&1).unwrap().admin = true;
        server.light(1, core);
        let line = format!("setblock {} {} {} core:air", core.x, core.y, core.z);
        server.handle_message(1, voxmar_protocol::ClientMessage::Command { line }, std::time::Instant::now());
        assert_eq!(at(&server, core), "core:air", "the command set nothing");
        assert_eq!(server.hearth_owners.get(&core), Some(&Some("ada".to_owned())), "a command forgot Ada's hearth");
    }

    /// Whose each home hearth is comes back with the world: an unlit one kept through the
    /// world's own save, and from a world saved before owners were kept (no
    /// `hearth_owners`), each lit home hearth's owner. (An older world's cold home hearth
    /// has no owner kept anywhere, so it comes back as no one's: rare, and safe.)
    #[test]
    fn hearth_owners_come_back_with_the_world() {
        use crate::save::{WorldMeta, WorldSave};
        let content = crate::test_content();
        let dir = std::env::temp_dir().join(format!("voxmar-hearth-owners-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = WorldMeta::new("hearth-owners", 1, crate::GameMode::Survival);
        WorldSave::new(&dir).create(&meta).unwrap();
        let open = |meta: WorldMeta| {
            let ((_, incoming), (_, console)) = (crossbeam_channel::unbounded(), crossbeam_channel::unbounded());
            let config = crate::ServerConfig { view_distance: 2, ..Default::default() };
            Server::new(config, meta, Some(WorldSave::new(&dir)), crate::save::WorldState::default(), content.clone(), incoming, console)
        };
        let mut server = open(meta);
        let core = IVec3::new(3, 70, -4);
        server.hearth_owners.insert(core, Some("ada".into()));
        server.save();
        let meta = WorldSave::new(&dir).read_meta().unwrap();
        let loaded = open(meta.clone());
        assert_eq!(loaded.hearth_owners.get(&core), Some(&Some("ada".to_owned())), "the cold hearth's owner came back");
        // The same world as one saved before owners were kept, with Ada's home hearth lit.
        let mut old = meta;
        old.hearth_owners.clear();
        old.lit = vec![crate::hearth::Lit { position: core.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: Some("Ada".into()) }];
        let text = ron::to_string(&old).unwrap();
        let without = text.replace("hearth_owners:[],", "");
        assert_ne!(text, without, "the field was not written as expected: {text}");
        let old: WorldMeta = ron::from_str(&without).unwrap();
        assert!(old.hearth_owners.is_empty());
        let loaded = open(old);
        assert_eq!(loaded.hearth_owners.get(&core), Some(&Some("ada".to_owned())), "the lit hearth's owner was not kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same hearth under the same thatch roof: with the roof's opening over the fire
    /// lined in stone, the fire reaches no thatch (the nearest is two blocks off as the crow
    /// flies, but three round the stone); with thatch over the fire, it catches.
    #[test]
    fn a_stone_lined_flue_keeps_a_hearths_fire_from_the_roof() {
        for flue in [true, false] {
            let mut server = crate::test_server();
            let (core, _client) = hearth(&mut server);
            roof(&mut server, core, flue);
            server.light(1, core);
            let rules = server.content.fire.rules.clone();
            run(&mut server, rules.catch_seconds);
            let burning: Vec<IVec3> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| core + IVec3::new(x, 2, z))).filter(|&p| at(&server, p) == "base:blaze").collect();
            if flue {
                assert!(server.blazes.is_empty() && burning.is_empty(), "the lined flue let the fire at the roof: {burning:?}");
            } else {
                assert_eq!(burning, [core + IVec3::Y * 2], "the thatch over the fire, and only it, caught first");
            }
        }
    }

    /// A chimney piece lines the flue as stone does: set on the hearthstone with thatch all
    /// round it a block up, a stone flue or a brick hood keeps the fire from the thatch,
    /// where an open flue (air) lets it catch.
    #[test]
    fn a_chimney_piece_lines_the_flue_as_stone_does() {
        for piece in ["base:stone_flue", "base:chimney_hood", "air"] {
            let mut server = crate::test_server();
            let (core, _client) = hearth(&mut server);
            let thatch = server.content.blocks.expect("base:thatch_roof");
            for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1)] {
                server.set_block(core + IVec3::new(x, 1, z), thatch);
            }
            let flue = if piece == "air" { BlockId::AIR } else { server.content.blocks.expect(piece) };
            server.set_block(core + IVec3::Y, flue);
            server.light(1, core);
            let rules = server.content.fire.rules.clone();
            run(&mut server, rules.catch_seconds);
            let caught = !server.blazes.is_empty() || (-1..=1).any(|x| (-1..=1).any(|z| at(&server, core + IVec3::new(x, 1, z)) == "base:blaze"));
            assert_eq!(caught, piece == "air", "with {piece} over the fire the thatch caught: {caught}");
        }
    }

    /// Fire.ron's `homes_catch_only_from_owner`: player 2's torch set beside the wattle in
    /// player 1's home (the box round the home hearth 1 lit) never sets it smouldering, nor
    /// does it catch; player 1's own torch in the same place does. Turned off, anyone's does.
    #[test]
    fn a_strangers_torch_never_burns_your_home_but_your_own_does() {
        for only_owner in [true, false] {
            let mut server = crate::test_server();
            if !only_owner {
                server.content.fire = std::sync::Arc::new({
                    let mut fire = (*server.content.fire).clone();
                    fire.rules.homes_catch_only_from_owner = false;
                    fire
                });
            }
            let (core, _client) = hearth(&mut server);
            server.players.get_mut(&1).unwrap().name = "Ada".into();
            server.light(1, core);
            assert_eq!(server.lit.get(&core).and_then(|l| l.owner.clone()).as_deref(), Some("ada"), "the hearth is Ada's");
            // Four off the hearth, out of its own fire's reach, well inside the home box.
            let (wall, beside) = (core + IVec3::new(4, 0, 0), core + IVec3::new(4, 0, 1));
            let (wattle, torch) = (server.content.blocks.expect("base:wattle_panel"), server.content.blocks.expect("base:torch"));
            server.set_block(wall, wattle);
            let rules = server.content.fire.rules.clone();
            // What a player's message sets before their change lands (`handle_message`).
            let place = |server: &mut Server, who: &str, block: BlockId| {
                server.fire_origin = Some(crate::player_key(who));
                server.set_block(beside, block);
                server.fire_origin = None;
            };
            place(&mut server, "Bo", torch);
            assert_eq!(server.blazes.contains_key(&wall), !only_owner, "Bo's torch at Ada's wall");
            run(&mut server, rules.catch_seconds + 1.0);
            assert_eq!(at(&server, wall), if only_owner { "base:wattle_panel" } else { "base:blaze" });
            if !only_owner {
                continue;
            }
            place(&mut server, "Bo", BlockId::AIR);
            place(&mut server, "Ada", torch);
            assert_eq!(server.blazes.get(&wall).and_then(|b| b.origin.clone()).as_deref(), Some("ada"));
            run(&mut server, rules.catch_seconds);
            assert_eq!(at(&server, wall), "base:blaze", "Ada's own torch burns her own wall");
            // Bo's home hearth lit next door, its box over Ada's wall too: the wall is in both
            // homes, so neither's torch sets it smouldering.
            let next_door = core + IVec3::new(8, 0, 0);
            server.lit.insert(next_door, crate::hearth::Lit { position: next_door.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: Some("Bo".into()) });
            let other = beside - IVec3::X;
            server.set_block(other, wattle);
            place(&mut server, "Ada", BlockId::AIR);
            place(&mut server, "Ada", torch);
            assert!(!server.blazes.contains_key(&other), "Ada's torch set her wall smouldering where Bo's home overlaps");
            place(&mut server, "Ada", BlockId::AIR);
            place(&mut server, "Bo", torch);
            assert!(!server.blazes.contains_key(&other), "Bo's torch set Ada's wall smouldering where his home overlaps");
        }
    }

    /// Bo right-clicking Ada's lit home hearth makes his home there, but her hearth stays
    /// hers and his torch at her wall still fizzles, also once she has left.
    #[test]
    fn making_your_home_at_someone_elses_hearth_never_lets_your_fire_in() {
        let mut server = crate::test_server();
        let (core, _ada) = hearth(&mut server);
        server.players.get_mut(&1).unwrap().name = "Ada".into();
        server.light(1, core);
        let _bo = join(&mut server, 2, "Bo");
        server.light(2, core);
        let home = core.as_vec3() + voxmar_engine::glam::Vec3::new(0.5, 1.0, 0.5);
        assert_eq!(server.players[&2].home, Some(home), "Bo's home is at Ada's hearth");
        assert_eq!(server.lit[&core].owner.as_deref(), Some("ada"), "Bo's click took Ada's hearth");
        let wall = core + IVec3::new(4, 0, 0);
        server.set_block(wall, server.content.blocks.expect("base:wattle_panel"));
        assert!(!torch_smoulders(&mut server, "Bo", wall + IVec3::Z, wall), "Bo's torch at Ada's wall, Ada here");
        server.leave(1);
        assert!(!torch_smoulders(&mut server, "Bo", wall + IVec3::Z, wall), "Bo's torch at Ada's wall, Ada away");
    }

    /// Bo lighting a home hearth of his own close by, its box over Ada's walls: his torch
    /// still fizzles at her wall while she is away, and burns his own wall past her box.
    #[test]
    fn a_hearth_lit_next_door_never_lets_its_owners_fire_in() {
        let mut server = crate::test_server();
        let (core, _ada) = hearth(&mut server);
        server.players.get_mut(&1).unwrap().name = "Ada".into();
        server.light(1, core);
        let _bo = join(&mut server, 2, "Bo");
        let next_door = core + IVec3::new(7, 0, 0);
        build_hearth(&mut server, next_door);
        server.light(2, next_door);
        assert_eq!(server.lit.get(&next_door).and_then(|l| l.owner.as_deref()), Some("bo"), "Bo lit his own hearth");
        server.leave(1);
        let wattle = server.content.blocks.expect("base:wattle_panel");
        let (hers, his) = (core + IVec3::new(4, 0, 0), next_door + IVec3::new(4, 0, 0));
        server.set_block(hers, wattle);
        server.set_block(his, wattle);
        assert!(!torch_smoulders(&mut server, "Bo", hers + IVec3::Z, hers), "Bo's torch at Ada's wall inside his own box");
        assert!(torch_smoulders(&mut server, "Bo", his + IVec3::Z, his), "Bo's torch at his own wall");
    }

    /// Ada lights her home hearth, leaves, and joins again typing her name as "ADA": she is
    /// the same player, so her own torch still burns her own wall and Bo's still fizzles.
    /// A hearth and a fire saved with the owner's name as typed open as her key.
    #[test]
    fn a_name_typed_in_another_case_is_the_same_home_owner() {
        let mut server = crate::test_server();
        let (core, _client) = hearth(&mut server);
        server.players.remove(&1);
        let _ada = join(&mut server, 1, "Ada");
        server.light(1, core);
        assert_eq!(server.lit[&core].owner.as_deref(), Some("ada"), "the hearth is kept by Ada's key");
        server.leave(1);
        let _ada = join(&mut server, 1, "ADA");
        let wall = core + IVec3::new(4, 0, 0);
        server.set_block(wall, server.content.blocks.expect("base:wattle_panel"));
        let name = server.players[&1].name.clone();
        assert!(torch_smoulders(&mut server, &name, wall + IVec3::Z, wall), "{name}'s own torch fizzled at her own wall");
        assert!(!torch_smoulders(&mut server, "Bo", wall + IVec3::Z, wall), "Bo's torch at Ada's wall");
        let lit: crate::hearth::Lit = ron::from_str(r#"(position: (0, 0, 0), structure: "base:homestead_hearth", until: 9, owner: Some("Ada"))"#).unwrap();
        let blaze: Blaze = ron::from_str(r#"(pos: (0, 0, 0), due: 9, burning: false, origin: Some("Ada"))"#).unwrap();
        assert_eq!((lit.owner.as_deref(), blaze.origin.as_deref()), (Some("ada"), Some("ada")), "an older save's names open as keys");
        let old: Blaze = ron::from_str("(pos: (0, 0, 0), due: 9, burning: false)").unwrap();
        assert_eq!(old.origin, None, "a save from before fires were kept by owner");
    }

    /// A home hearth lit before fires were kept by owner (no owner saved) belongs to no one
    /// and keeps every fire out: Bo right-clicking it, leaving and joining again never makes
    /// it his, and his torch by its wall and by its roof's top layer (the home box's top,
    /// `home_up` over the cell above the hearthstone) sets nothing smouldering.
    #[test]
    fn an_old_unowned_hearth_keeps_every_fire_out_and_is_never_taken() {
        let mut server = crate::test_server();
        let (core, _client) = hearth(&mut server);
        server.players.remove(&1);
        server.lit.insert(core, crate::hearth::Lit { position: core.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: None });
        let _bo = join(&mut server, 2, "Bo");
        server.light(2, core);
        assert!(server.players[&2].home.is_some(), "Bo's home is at the old hearth");
        assert_eq!(server.lit[&core].owner, None, "Bo's click took the old hearth");
        server.leave(2);
        let _bo = join(&mut server, 2, "Bo");
        assert_eq!(server.lit[&core].owner, None, "Bo's rejoin took the old hearth");
        let (wattle, thatch) = (server.content.blocks.expect("base:wattle_panel"), server.content.blocks.expect("base:thatch_roof"));
        let top = core + IVec3::Y * (1 + server.survival.home_up);
        let wall = core + IVec3::new(4, 0, 0);
        server.set_block(wall, wattle);
        server.set_block(top, thatch);
        for who in ["Bo", "Ada"] {
            assert!(!torch_smoulders(&mut server, who, wall + IVec3::Z, wall), "{who}'s torch at the old home's wall");
            assert!(!torch_smoulders(&mut server, who, top + IVec3::X, top), "{who}'s torch at the old home's roof top");
        }
        // One above the box is no longer the home's.
        let above = top + IVec3::Y;
        server.set_block(above, thatch);
        assert!(torch_smoulders(&mut server, "Bo", above + IVec3::X, above), "the home box reaches past home_up");
    }

    /// The same fire, run twice, stands the same halfway through: what waits and burns, with
    /// its due tick and whose it is, and every block it touches.
    #[test]
    fn a_fire_halfway_through_is_the_same_every_time() {
        let snapshot = |server: &mut Server| {
            let base = floor(server);
            let (wattle, thatch, torch) = (server.content.blocks.expect("base:wattle_panel"), server.content.blocks.expect("base:thatch_roof"), server.content.blocks.expect("base:torch"));
            for x in 0..4 {
                for z in -1..=1 {
                    server.set_block(base + IVec3::new(x, 0, z), if z == 0 { wattle } else { thatch });
                }
            }
            server.fire_origin = Some("ada".into());
            server.set_block(base - IVec3::X, torch);
            server.fire_origin = None;
            let rules = server.content.fire.rules.clone();
            run(server, rules.catch_seconds * 2.0 + rules.burn_seconds * 0.5);
            let blocks: Vec<String> = (-1..4).flat_map(|x| (-1..=1).map(move |z| base + IVec3::new(x, 0, z))).map(|p| at(server, p)).collect();
            let saved = server.saved_blazes();
            assert!(saved.iter().any(|b| b.burning) && blocks.iter().any(|b| b == "base:blaze"), "nothing burning halfway: {blocks:?}");
            assert!(saved.iter().all(|b| b.origin.as_deref() == Some("ada")), "the fire stays Ada's as it runs: {saved:?}");
            (saved, blocks)
        };
        let (mut a, mut b) = (crate::test_server(), crate::test_server());
        assert_eq!(snapshot(&mut a), snapshot(&mut b));
    }

    /// Saved halfway to catching, through the world's own save, and opened by a new server:
    /// the panel catches at the game tick it would have, not a tick sooner.
    #[test]
    fn a_smoulder_saved_and_loaded_catches_on_its_tick() {
        use crate::save::{WorldMeta, WorldSave};
        let content = crate::test_content();
        let dir = std::env::temp_dir().join(format!("voxmar-smoulder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = WorldMeta::new("smoulder", 1, crate::GameMode::Survival);
        WorldSave::new(&dir).create(&meta).unwrap();
        let open = |meta: WorldMeta| {
            let ((_, incoming), (_, console)) = (crossbeam_channel::unbounded(), crossbeam_channel::unbounded());
            let config = crate::ServerConfig { view_distance: 2, ..Default::default() };
            Server::new(config, meta, Some(WorldSave::new(&dir)), crate::save::WorldState::default(), content.clone(), incoming, console)
        };
        let mut server = open(meta);
        let base = floor(&mut server);
        let (wattle, torch) = (server.content.blocks.expect("base:wattle_panel"), server.content.blocks.expect("base:torch"));
        server.set_block(base, wattle);
        server.set_block(base + IVec3::X, torch);
        let due = server.blazes[&base].due;
        let half = server.content.fire.rules.catch_seconds / 2.0;
        run(&mut server, half);
        server.save();
        let mut loaded = open(WorldSave::new(&dir).read_meta().unwrap());
        assert_eq!(loaded.tick, server.tick, "the clock came back");
        assert_eq!(loaded.saved_blazes(), server.saved_blazes(), "what smoulders came back");
        // The world's blocks come back with their columns; set them as they were (each is
        // already waiting, so its due tick stands).
        loaded.set_block(base, wattle);
        loaded.set_block(base + IVec3::X, torch);
        assert_eq!(loaded.blazes[&base].due, due);
        while loaded.tick + 1 < due {
            loaded.tick += 1;
            loaded.blaze();
        }
        assert_eq!(at(&loaded, base), "base:wattle_panel", "caught early");
        loaded.tick += 1;
        loaded.blaze();
        assert_eq!(at(&loaded, base), "base:blaze", "caught on its tick");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The light a hint sends a player to: a candle lights a home (snug counts a `light`)
    /// and no flame stands in it, and it is made by hand at Crafting 3, in the Nomad age.
    #[test]
    fn a_candle_lights_a_home_without_setting_it_alight() {
        let server = crate::test_server();
        let blocks = &server.content.blocks;
        let candle = blocks.expect("base:candle");
        assert!(blocks.def(candle).has_tag("light") && !server.content.fire.flame(candle) && !server.content.fire.catches(candle));
        assert!(blocks.def(candle).light > 0);
        let recipes: Vec<_> = server.content.recipes.iter().filter(|r| r.output == candle).collect();
        assert!(recipes.iter().any(|r| r.near.is_none() && r.level <= 3), "a candle is made by hand, early: {recipes:?}");
    }

    /// What waits on fire is saved and comes back as it was.
    #[test]
    fn fires_are_saved_with_the_world() {
        let mut server = crate::test_server();
        let base = floor(&mut server);
        server.set_block(base, server.content.blocks.expect("base:wattle_panel"));
        server.set_block(base + IVec3::X, server.content.blocks.expect("base:blaze"));
        let saved = server.saved_blazes();
        assert_eq!(saved.len(), 2, "one waiting to catch, one burning: {saved:?}");
        assert_eq!(restore(&saved), server.blazes);
    }
}
