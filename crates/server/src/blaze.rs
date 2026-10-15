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
/// name; see `Server::fire_origin`), carried on as it spreads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blaze {
    #[serde(default)]
    pub pos: [i32; 3],
    #[serde(default)]
    pub due: u64,
    #[serde(default)]
    pub burning: bool,
    #[serde(default)]
    pub origin: Option<String>,
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
    /// and your own careless torch still burns your own roof.
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
        let hearths = self.lit.iter().filter(|(_, lit)| lit.until > self.tick && multiblocks.get(&lit.structure).is_some_and(|m| m.home)).filter_map(|(&core, lit)| Some((core, lit.owner.as_ref()?)));
        let beds = self.players.values().filter_map(|p| Some((p.home_block(&self.lit)?, &p.name)));
        hearths.chain(beds).any(|(home, owner)| within(home) && origin.as_ref() != Some(owner))
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

    /// A hearth or kiln was lit at `core`: what catches near it starts waiting to catch, as
    /// its owner's fire.
    pub(crate) fn hearth_blaze(&mut self, core: IVec3) {
        if self.content.fire.on() && self.content.fire.rules.hearth_reach > 0 {
            let owner = self.lit.get(&core).and_then(|lit| lit.owner.clone());
            let set_by = std::mem::replace(&mut self.fire_origin, owner);
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
            run(server, rules.burn_seconds);
            assert_eq!(at(server, first), "core:air", "burned away");
            assert_eq!(at(server, far), "base:wattle_panel", "three blocks off, never in reach of a flame");
            run(server, rules.burn_seconds);
            assert_eq!(at(server, second), "core:air");
            assert!(server.blazes.is_empty(), "nothing left waiting");
            server.saved_blazes()
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
        // A hearth lit two blocks off sets it alight.
        let core = base + IVec3::new(rules.hearth_reach + 1, 0, 0);
        server.lit.insert(core, crate::hearth::Lit { position: core.to_array(), structure: "base:homestead_hearth".into(), until: u64::MAX, owner: None });
        server.hearth_blaze(core);
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
        (core, client)
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
    /// nothing starts to smoulder (they changed nothing), but they are told once which block
    /// its fire reaches and how far off; their next lighting by hand does set it smouldering.
    #[test]
    fn a_hearth_relit_on_joining_names_the_thatch_and_burns_nothing() {
        let mut server = crate::test_server();
        let (core, client) = hearth(&mut server);
        let above = core + IVec3::Y * 2;
        server.set_block(above, server.content.blocks.expect("base:thatch_roof"));
        let player = server.players.get_mut(&1).unwrap();
        (player.home, player.kindled) = (Some(core.as_vec3() + voxmar_engine::glam::Vec3::new(0.5, 1.0, 0.5)), false);
        told(&client);
        server.kindle_home(1);
        assert!(server.lit.contains_key(&core), "the old home hearth was not lit again");
        assert!(server.blazes.is_empty(), "relit on joining set the thatch smouldering");
        let hints: Vec<_> = told(&client).into_iter().filter_map(|m| match m {
            voxmar_protocol::ServerMessage::Chat { text: voxmar_engine::lang::Words::Key(k, args), .. } if k == "fire.near_hearth" => Some(args),
            _ => None,
        }).collect();
        assert_eq!(hints.len(), 1, "told once of what the fire reaches");
        let said = format!("{:?}", hints[0]);
        assert!(said.contains("block.base.thatch_roof") && said.contains("\"2\""), "names the thatch two steps off: {said}");
        server.lit.remove(&core);
        server.light(1, core);
        assert_eq!(server.blazes.get(&above).map(|b| b.burning), Some(false), "lit by hand, the thatch smoulders");
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
            assert_eq!(server.lit.get(&core).and_then(|l| l.owner.clone()).as_deref(), Some("Ada"), "the hearth is Ada's");
            // Four off the hearth, out of its own fire's reach, well inside the home box.
            let (wall, beside) = (core + IVec3::new(4, 0, 0), core + IVec3::new(4, 0, 1));
            let (wattle, torch) = (server.content.blocks.expect("base:wattle_panel"), server.content.blocks.expect("base:torch"));
            server.set_block(wall, wattle);
            let rules = server.content.fire.rules.clone();
            // What a player's message sets before their change lands (`handle_message`).
            let place = |server: &mut Server, who: &str, block: BlockId| {
                server.fire_origin = Some(who.to_owned());
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
            assert_eq!(server.blazes.get(&wall).and_then(|b| b.origin.clone()).as_deref(), Some("Ada"));
            run(&mut server, rules.catch_seconds);
            assert_eq!(at(&server, wall), "base:blaze", "Ada's own torch burns her own wall");
        }
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
            server.fire_origin = Some("Ada".into());
            server.set_block(base - IVec3::X, torch);
            server.fire_origin = None;
            let rules = server.content.fire.rules.clone();
            run(server, rules.catch_seconds * 2.0 + rules.burn_seconds * 0.5);
            let blocks: Vec<String> = (-1..4).flat_map(|x| (-1..=1).map(move |z| base + IVec3::new(x, 0, z))).map(|p| at(server, p)).collect();
            let saved = server.saved_blazes();
            assert!(saved.iter().any(|b| b.burning) && blocks.iter().any(|b| b == "base:blaze"), "nothing burning halfway: {blocks:?}");
            assert!(saved.iter().all(|b| b.origin.as_deref() == Some("Ada")), "the fire stays Ada's as it runs: {saved:?}");
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
