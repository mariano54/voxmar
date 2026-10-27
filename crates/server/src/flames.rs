//! Placed lights burning out and lit again (`voxmar_engine::flame`, progression piece 8b).
//! A torch set down burns `burns_days:` game days by the world's clock, then goes out to
//! its stub; rain on it under open sky puts it out sooner, and in the last share of its burn
//! (`gutter_share`) it gutters, low and sputtering. A right click on the stub with anything
//! that kindles (a firebrand, a lit torch) lights it again for nothing: the cost is coming
//! back, never coal.
//!
//! Picked up burning, a light keeps what it has burnt as its item's wear, and set down
//! again burns on from there. Inside a settlement, near its height, its people tend what is
//! `tended` (`tend_height`): it never goes out there, and keeps what it had burnt. Lights from before lights burnt
//! out, and those the world placed, are given a full burn the first time their column
//! loads (`flame_columns`, once a column). When one burning out leaves a home short of a
//! light a step of it asks for, the home's owner is told.

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, local_index};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::effect::HomeSurvey;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_DAY};

use crate::Server;
use crate::save::SavedFlame;

/// A light set down and burning: what it is, and the world's day tick it goes out; or,
/// `tended` in a settlement, the ticks of burn it has left, kept while it is tended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flame {
    pub block: BlockId,
    pub out: u64,
    pub tended: bool,
}

impl Server {
    /// `block` was set at `pos`: a light that burns out starts burning there, a light
    /// guttering burns on to the same end, anything else forgets the one that was. One
    /// lookup, so every block set can ask.
    pub(crate) fn flame_changed(&mut self, pos: IVec3, block: BlockId) {
        let Some(full) = self.full_burn(block) else {
            self.flames.remove(&pos);
            return;
        };
        if let Some(flame) = self.flames.get_mut(&pos)
            && self.content.flames.gutters(&self.content.blocks, flame.block) == Some(block)
        {
            flame.block = block;
            return;
        }
        self.burn_from(pos, block, full);
    }

    /// Ticks `block` burns set down, if it burns out.
    fn full_burn(&self, block: BlockId) -> Option<u64> {
        self.content.flames.burns_out(&self.content.blocks, block).map(|(days, _)| (days * TICKS_PER_DAY as f32).round() as u64)
    }

    /// Whether `block` at `pos` is tended: a light its people keep, inside a settlement and
    /// near its height (`tend_height`), not in a mine under it.
    fn tended_at(&self, pos: IVec3, block: BlockId) -> bool {
        self.content.flames.tended(block) && self.overworld.settlement_height(pos.x, pos.z).is_some_and(|height| (pos.y - height).abs() <= self.survival.tend_height)
    }

    /// `block` at `pos` burns on with `left` ticks of its burn. Tended, it is kept: as no
    /// entry when its burn is whole, so a settlement's lights cost nothing.
    fn burn_from(&mut self, pos: IVec3, block: BlockId, left: u64) {
        if self.tended_at(pos, block) {
            if self.full_burn(block).is_some_and(|full| left >= full) {
                self.flames.remove(&pos);
            } else {
                self.flames.insert(pos, Flame { block, out: left, tended: true });
            }
            return;
        }
        self.flames.insert(pos, Flame { block, out: self.day.floor() as u64 + left, tended: false });
    }

    /// The wear `item`, dropped by the light burning at `pos`, keeps of what it has burnt
    /// (its share of the burn, of all `item` lasts); 0 for what is not burning there.
    pub(crate) fn burnt_wear(&self, pos: IVec3, item: BlockId) -> u16 {
        let life = self.content.flames.burn_life(item) as u64;
        let Some(flame) = self.flames.get(&pos).filter(|f| self.block(pos) == Some(f.block)) else { return 0 };
        let Some(full) = self.full_burn(flame.block).filter(|&full| life > 0 && full > 0) else { return 0 };
        let left = if flame.tended { flame.out } else { flame.out.saturating_sub(self.day.floor() as u64) }.min(full);
        ((full - left) * life).div_ceil(full).min(life) as u16
    }

    /// The light just set down at `pos` from `item` worn by `wear` burns on from there.
    pub(crate) fn set_down_burnt(&mut self, pos: IVec3, item: BlockId, wear: u16) {
        let life = self.content.flames.burn_life(item) as u64;
        let Some(block) = self.block(pos) else { return };
        let Some(full) = self.full_burn(block) else { return };
        if wear == 0 || life == 0 {
            return;
        }
        let left = full - full * (wear as u64).min(life) / life;
        self.burn_from(pos, block, left);
    }

    /// `agent` lit the light gone out at `pos` again (`Effect::Relight`), using up one of
    /// what is in hand if it asks that (an oil lamp's fat).
    pub(crate) fn relight(&mut self, agent: PlayerId, pos: IVec3) {
        let Some(here) = self.block(pos) else { return };
        let Some(lit) = self.content.flames.relit(&self.content.blocks, here) else { return };
        if self.content.flames.uses_up(here) {
            let Some(player) = self.players.get_mut(&agent) else { return };
            let selected = player.selected;
            player.inventory.take(selected, 1);
            player.changed(voxmar_engine::traits::INVENTORY);
        }
        self.set_block(pos, lit);
    }

    /// A column just loaded: the lights in it that are not burning (set down before lights
    /// burnt out, or placed by the world) are given a full burn, once a column; tended ones
    /// stay as they are.
    pub(crate) fn flames_loaded(&mut self, column: ColumnPos) {
        let key = column.0.to_array();
        let Err(at) = self.meta.flame_columns.binary_search(&key) else { return };
        let Some(loaded) = self.world.column(column) else { return };
        let flames = &self.content.flames;
        let origin = column.origin();
        let (mut found, mut any) = (Vec::new(), false);
        for (index, chunk) in loaded.chunks.iter().enumerate() {
            if !chunk.palette().iter().any(|&b| flames.burn_life(b) > 0) {
                continue;
            }
            any = true;
            let base = origin + IVec3::new(0, index as i32 * CHUNK_SIZE, 0);
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let block = chunk.get(local_index(IVec3::new(x, y, z)));
                        let pos = base + IVec3::new(x, y, z);
                        if flames.burn_life(block) > 0 && !self.flames.contains_key(&pos) {
                            found.push((pos, block));
                        }
                    }
                }
            }
        }
        // A column with no lights is not marked: one set down later burns as it is set.
        if !any {
            return;
        }
        self.meta.flame_columns.insert(at, key);
        for (pos, block) in found {
            if let Some(full) = self.full_burn(block) {
                self.burn_from(pos, block, full);
            }
        }
    }

    /// Gutters the lights near their end, and puts out those whose time is up and those
    /// rain falls on. Those in columns not loaded wait; tended ones are kept; one no longer
    /// there, or no longer burning out (a mod changed it), is forgotten.
    pub(crate) fn burn_flames(&mut self) {
        if self.flames.is_empty() {
            return;
        }
        let (now, tick, day, wet, share) = (self.day.floor() as u64, self.tick, self.day, self.survival.douse_rain, self.survival.gutter_share);
        let (mut out, mut gone, mut gutter) = (Vec::new(), Vec::new(), Vec::new());
        // Of those going out, the ones burnt down (not rained out): only these are made
        // good from home's stores.
        let mut burnt: Vec<IVec3> = Vec::new();
        // Rain is the same over a column of blocks: sampled once for each, and a roof looked
        // for only where it rains.
        let mut raining: FxHashMap<(i32, i32), bool> = FxHashMap::default();
        let flames: Vec<(IVec3, Flame)> = self.flames.iter().filter(|(_, f)| !f.tended).map(|(&p, &f)| (p, f)).collect();
        for (pos, flame) in flames {
            let Some(here) = self.block(pos) else { continue };
            let Some(full) = self.full_burn(here).filter(|_| here == flame.block) else {
                gone.push(pos);
                continue;
            };
            if now >= flame.out {
                out.push(pos);
                burnt.push(pos);
                continue;
            }
            if let Some(low) = self.content.flames.gutters(&self.content.blocks, here)
                && (flame.out - now) as f32 <= full as f32 * share
            {
                gutter.push((pos, low));
            }
            if !self.content.flames.douses(here) {
                continue;
            }
            let column = (pos.x.div_euclid(16), pos.z.div_euclid(16));
            let rain = *raining.entry(column).or_insert_with(|| {
                let report = self.weather.local(&mut self.temperature, Vec2::new(pos.x as f32, pos.z as f32), tick, day);
                report.weather.precipitation * (1.0 - report.weather.snow) >= wet
            });
            if rain && self.open_sky(pos + IVec3::Y) {
                out.push(pos);
            }
        }
        for pos in gone {
            self.flames.remove(&pos);
        }
        for (pos, low) in gutter {
            if !out.contains(&pos) {
                self.set_block(pos, low);
            }
        }
        if out.is_empty() {
            return;
        }
        // The homes these lights stand in, surveyed once before and once after the whole
        // pass: each owner is told once, of the highest step lost.
        let mut homes: Vec<(PlayerId, IVec3, BlockId)> = Vec::new();
        for &pos in &out {
            let Some(here) = self.block(pos) else { continue };
            for (id, anchor) in self.homes_holding(pos) {
                if !homes.iter().any(|&(had, ..)| had == id) {
                    homes.push((id, anchor, here));
                }
            }
        }
        let before: Vec<HomeSurvey> = homes.iter().map(|&(id, anchor, _)| self.survey_around(anchor, Some(id))).collect();
        for pos in out {
            let Some(here) = self.block(pos) else { continue };
            // A spare kept at home for it (a candle box's candles) is set out in its place.
            let burnt = burnt.contains(&pos);
            if let Some(full) = self.full_burn(here).filter(|_| burnt)
                && self.take_spare(pos, here)
            {
                self.burn_from(pos, here, full);
                continue;
            }
            // Else one going out to what is lit again is lit again from a kindling box there.
            if let Some(lit) = burnt.then(|| self.rekindle(pos, here)).flatten() {
                match self.full_burn(here).filter(|_| lit == here) {
                    Some(full) => self.burn_from(pos, here, full),
                    None => self.set_block(pos, lit),
                }
                continue;
            }
            if let Some((_, stub)) = self.content.flames.burns_out(&self.content.blocks, here) {
                self.set_block(pos, stub);
            }
        }
        let tags = self.light_tags();
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        for (&(id, anchor, light), before) in homes.iter().zip(&before) {
            let after = self.survey_around(anchor, Some(id));
            let lost = self.content.effects.falls_short(&tags, before, &after);
            let steps = self.content.effects.home_steps(&after);
            let Some(player) = self.players.get_mut(&id) else { continue };
            // One back whose return is yet to be checked is told there, of this and what
            // went before it together, and not twice.
            if player.home_unchecked {
                continue;
            }
            (player.home_steps, player.home_anchor) = (steps, Some(anchor));
            let Some((effect, _, need)) = lost else { continue };
            let words = Words::key("flame.home_short").with("light", Words::Block(self.content.blocks.def(light).name.clone()));
            self.tell_short(id, words, effect, need);
        }
    }

    /// The tags home steps ask for (`HomeHas`) that lights burning out take away: those of
    /// what burns out and not of what it leaves.
    fn light_tags(&self) -> Vec<String> {
        let (blocks, flames) = (&self.content.blocks, &self.content.flames);
        let lights: Vec<BlockId> = blocks.ids().filter(|&b| flames.burn_life(b) > 0).collect();
        self.content
            .effects
            .home_tags()
            .iter()
            .filter(|tag| lights.iter().any(|&b| blocks.def(b).has_tag(tag) && flames.burns_out(blocks, b).is_some_and(|(_, stub)| !blocks.def(stub).has_tag(tag))))
            .cloned()
            .collect()
    }

    /// Takes one of `light` (in its own form, not its facing) out of a store tagged
    /// `spare_lights` in a home holding `pos`, its owner's or nobody's: whether one was
    /// there to set out in its place.
    fn take_spare(&mut self, pos: IVec3, light: BlockId) -> bool {
        let blocks = &self.content.blocks;
        let item = blocks.def(light).form_of().and_then(|base| blocks.id(base)).unwrap_or(light);
        self.take_at_home(pos, voxmar_engine::flame::SPARE_LIGHTS, |it| it == item)
    }

    /// What `light` at `pos`, going out to something lit again into it (or into the light it
    /// gutters from), is lit again into from a store tagged `relights_lights` in a home holding
    /// it: one of what its `relight_with:` asks, else one `kindling`, taken out.
    fn rekindle(&mut self, pos: IVec3, light: BlockId) -> Option<BlockId> {
        let (blocks, flames) = (&self.content.blocks, &self.content.flames);
        let (_, stub) = flames.burns_out(blocks, light)?;
        let lit = flames.relit(blocks, stub)?;
        let tag = flames.relit_with(stub).unwrap_or(voxmar_engine::flame::KINDLING).to_owned();
        let blocks = self.content.blocks.clone();
        self.take_at_home(pos, voxmar_engine::flame::RELIGHTS_LIGHTS, |it| blocks.def(it).has_tag(&tag)).then_some(lit)
    }

    /// Takes one item that `wanted` out of a store tagged `store_tag` in a home holding
    /// `pos`, its owner's or nobody's, nearest first: whether there was one.
    fn take_at_home(&mut self, pos: IVec3, store_tag: &str, wanted: impl Fn(BlockId) -> bool) -> bool {
        for (id, anchor) in self.homes_holding(pos) {
            let name = self.players[&id].name.clone();
            for at in self.stores_around(anchor) {
                let Some((block, state)) = self.factory.get(at) else { continue };
                if !self.content.blocks.def(block).has_tag(store_tag) || state.owner.as_deref().is_some_and(|owner| owner != name) {
                    continue;
                }
                let Some(slot) = state.bin.iter().position(|s| s.is_some_and(|s| wanted(s.item))) else { continue };
                let state = self.factory.state_mut(at).unwrap();
                let stack = state.bin[slot].as_mut().unwrap();
                stack.count -= 1;
                if stack.count == 0 {
                    state.bin[slot] = None;
                }
                return true;
            }
        }
        false
    }

    /// The players here whose homes hold `pos`, and where each home is.
    fn homes_holding(&self, pos: IVec3) -> Vec<(PlayerId, IVec3)> {
        let at = pos.as_vec3() + Vec3::splat(0.5);
        self.players.iter().filter(|(_, p)| p.joined).filter_map(|(&id, _)| Some((id, self.home_holding(id, at)?))).collect()
    }

    /// Tells player `id` that `effect`, a step of their home, now wants `need` lights:
    /// `words` with the step's name and the count filled in.
    fn tell_short(&self, id: PlayerId, words: Words, effect: voxmar_engine::effect::EffectId, need: u32) {
        let Some(player) = self.players.get(&id) else { return };
        let def = self.content.effects.get(effect);
        let key = voxmar_engine::lang::def_key("effect", &def.name);
        let words = words.with("name", Words::Own(key, def.title.clone())).with("need", need.to_string());
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
    }

    /// Player `id` back since they joined: once the land around their home is loaded, they
    /// are told the highest home step that lights burning out while they were away cost
    /// them, and what their home meets is noted from then on.
    pub(crate) fn check_home_return(&mut self, id: PlayerId) {
        let Some(player) = self.players.get(&id).filter(|p| p.joined && p.home_unchecked) else { return };
        let Some(anchor) = player.home_spot(&self.lit).map(|at| at.floor().as_ivec3()) else {
            self.players.get_mut(&id).unwrap().home_unchecked = false;
            return;
        };
        let reach = IVec3::new(self.survival.home_reach, 0, self.survival.home_reach);
        let corners = [anchor - reach, anchor + reach, anchor + IVec3::new(reach.x, 0, -reach.z), anchor + IVec3::new(-reach.x, 0, reach.z)];
        if !corners.iter().all(|&c| self.world.column(ColumnPos::of_block(c)).is_some()) {
            return;
        }
        let now = self.survey_around(anchor, Some(id));
        let tags = self.light_tags();
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        // Steps noted of a home elsewhere (a new bed since, a hearth gone out so home is the
        // bed) say nothing of what this one lost.
        let lost = if player.home_anchor == Some(anchor) { self.content.effects.lost(&player.home_steps, &tags, &now) } else { None };
        let steps = self.content.effects.home_steps(&now);
        let player = self.players.get_mut(&id).unwrap();
        (player.home_steps, player.home_anchor, player.home_unchecked) = (steps, Some(anchor), false);
        if let Some((effect, _, need)) = lost {
            self.tell_short(id, Words::key("flame.home_short_away"), effect, need);
        }
    }

    pub(crate) fn saved_flames(&self) -> Vec<SavedFlame> {
        let mut saved: Vec<SavedFlame> =
            self.flames.iter().map(|(pos, f)| SavedFlame { pos: pos.to_array(), block: self.content.blocks.def(f.block).name.clone(), out: f.out, tended: f.tended }).collect();
        saved.sort_by_key(|s| s.pos);
        saved
    }
}

/// Saved burning lights back by id, dropping those whose blocks no longer exist or no
/// longer burn out.
pub(crate) fn restore(saved: &[SavedFlame], id: impl Fn(&str) -> Option<BlockId>, burns_out: impl Fn(BlockId) -> bool) -> FxHashMap<IVec3, Flame> {
    saved.iter().filter_map(|s| Some((IVec3::from_array(s.pos), Flame { block: id(&s.block).filter(|&b| burns_out(b))?, out: s.out, tended: s.tended }))).collect()
}

/// Puts out burnt-out and rained-on lights once a second, and tells players back what
/// lights burnt out while they were away cost their homes.
#[derive(Default)]
pub(crate) struct Flames;

impl crate::process::Process for Flames {
    fn tier(&self) -> voxmar_engine::schedule::Tier {
        voxmar_engine::schedule::Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let back: Vec<PlayerId> = server.players.iter().filter(|(_, p)| p.home_unchecked).map(|(&id, _)| id).collect();
        for id in back {
            server.check_home_return(id);
        }
        server.burn_flames();
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
    const TORCH: IVec3 = IVec3::new(10, FLOOR + 1, 8);

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
        let at = Vec3::new(7.5, (FLOOR + 1) as f32, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        player.inventory.slots[0] = Some(ItemStack::new(id(&server, item), 4));
        player.selected = 0;
        server.players.insert(1, player);
        server
    }

    fn id(server: &Server, name: &str) -> BlockId {
        server.content.blocks.expect(name)
    }

    fn act(server: &mut Server, action: &str, block: IVec3, face: Face) {
        let action = server.content.actions.expect(action);
        server.propose(1, action, ActionTarget::Block { block, face }, Instant::now());
    }

    fn at(server: &Server, pos: IVec3) -> String {
        server.block(pos).map_or_else(String::new, |b| server.content.blocks.def(b).name.clone())
    }

    /// A torch set down burns a game day by the world's clock, then goes out to its stub;
    /// a firebrand lights the stub again, for a day more; one taken away is forgotten.
    #[test]
    fn a_torch_set_down_burns_a_day_and_its_stub_lights_again() {
        let mut server = fixture("base:torch");
        server.day = 1000.0;
        act(&mut server, "core:place", TORCH - IVec3::Y, Face::PosY);
        assert_eq!(at(&server, TORCH), "base:torch");
        let day = TICKS_PER_DAY as u64;
        assert_eq!(server.flames.get(&TORCH).map(|f| f.out), Some(1000 + day), "a day from when it was set");
        server.day = (1000 + day - 1) as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch_guttering", "a tick short of a day, guttering");
        server.day = (1000 + day) as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch_stub", "a day on");
        assert!(!server.flames.contains_key(&TORCH), "a stub does not burn");
        assert_eq!(server.content.blocks.def(id(&server, "base:torch_stub")).light, 0);

        // Nothing that kindles in hand: refused, the stub as it was.
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(id(&server, "base:stick"), 1));
        act(&mut server, "base:relight", TORCH, Face::NegX);
        assert_eq!(at(&server, TORCH), "base:torch_stub", "a stick does not light it");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(id(&server, "base:firebrand"), 1));
        act(&mut server, "base:relight", TORCH, Face::NegX);
        assert_eq!(at(&server, TORCH), "base:torch", "a firebrand lights it");
        assert_eq!(server.players[&1].inventory.slots[0].map(|s| s.count), Some(1), "for nothing");
        assert_eq!(server.flames.get(&TORCH).map(|f| f.out), Some(1000 + 2 * day), "a day from when it was lit");

        // Broken, it is no longer counted.
        server.set_block(TORCH, BlockId::AIR);
        assert!(server.flames.is_empty());
    }

    /// A wall torch goes out to its stub in its own facing and lights again into itself.
    #[test]
    fn a_wall_torch_goes_out_and_lights_in_its_facing() {
        let mut server = fixture("base:firebrand");
        let pos = TORCH + IVec3::new(0, 1, 0);
        server.set_block(TORCH, id(&server, "base:stone"));
        server.set_block(pos, id(&server, "base:wall_torch_east"));
        server.day += TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, pos), "base:wall_torch_stub_east");
        act(&mut server, "base:relight", pos, Face::NegX);
        assert_eq!(at(&server, pos), "base:wall_torch_east");
    }

    /// `preset` at full brought over the torch, held a few seconds: the weather there then.
    fn bring(server: &mut Server, preset: &str) -> voxmar_protocol::Weather {
        let here = Vec2::new(TORCH.x as f32, TORCH.z as f32);
        let preset = server.content.weather.preset(preset).unwrap().at(1.0);
        server.weather.force(preset, here, 600, server.tick);
        for _ in 0..3 {
            server.tick += TICKS_PER_SECOND as u64;
            server.day += TICKS_PER_SECOND as f64;
            let (tick, day) = (server.tick, server.day);
            server.weather.advance(&mut server.temperature, &[IVec2::new(TORCH.x, TORCH.z)], tick, day);
        }
        server.weather.local(&mut server.temperature, here, server.tick, server.day).weather
    }

    /// Rain on a torch under open sky puts it out at once; one under a roof burns on.
    #[test]
    fn rain_puts_out_a_torch_under_open_sky() {
        let mut server = fixture("base:torch");
        let roofed = TORCH + IVec3::new(4, 0, 0);
        let torch = id(&server, "base:torch");
        server.set_block(TORCH, torch);
        server.set_block(roofed, torch);
        server.set_block(roofed + IVec3::new(0, 3, 0), id(&server, "base:stone"));
        bring(&mut server, "clear");
        server.burn_flames();
        assert_eq!((at(&server, TORCH), at(&server, roofed)), ("base:torch".into(), "base:torch".into()), "a clear sky");
        let rain = bring(&mut server, "rain");
        assert!(rain.precipitation * (1.0 - rain.snow) >= server.survival.douse_rain, "raining: {rain:?}");
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch_stub", "rained out under open sky");
        assert_eq!(at(&server, roofed), "base:torch", "kept dry under a roof");
    }

    /// A candle burns three game days to its end, never lit again; an oil lamp burns five
    /// to empty, and fat right clicked on it fills and lights it, one used up.
    #[test]
    fn a_candle_burns_to_its_end_and_a_lamp_is_filled_with_fat() {
        let mut server = fixture("base:lard");
        let lamp = TORCH + IVec3::X;
        server.set_block(TORCH, id(&server, "base:candle"));
        server.set_block(lamp, id(&server, "base:clay_oil_lamp"));
        let start = server.day;
        server.day = start + 3.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!((at(&server, TORCH), at(&server, lamp)), ("base:candle_end".into(), "base:clay_oil_lamp".into()), "three days on");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(id(&server, "base:firebrand"), 1));
        act(&mut server, "base:relight", TORCH, Face::NegX);
        assert_eq!(at(&server, TORCH), "base:candle_end", "a candle's end is never lit again");
        server.day = start + 5.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, lamp), "base:clay_oil_lamp_empty", "five days on, burnt dry");
        act(&mut server, "base:relight", lamp, Face::NegX);
        assert_eq!(at(&server, lamp), "base:clay_oil_lamp_empty", "a firebrand does not fill it");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(id(&server, "base:lard"), 2));
        act(&mut server, "base:relight", lamp, Face::NegX);
        assert_eq!(at(&server, lamp), "base:clay_oil_lamp", "filled with fat and lit");
        assert_eq!(server.players[&1].inventory.slots[0].map(|s| s.count), Some(1), "one lard used up");
    }

    /// The burning lights are saved with the world, by name, with when each goes out.
    #[test]
    fn burning_lights_are_saved_and_restored() {
        let mut server = fixture("base:torch");
        server.set_block(TORCH, id(&server, "base:torch"));
        let saved = server.saved_flames();
        assert_eq!(saved.len(), 1);
        assert_eq!((saved[0].pos, saved[0].block.as_str()), (TORCH.to_array(), "base:torch"));
        let ron = ron::to_string(&saved).unwrap();
        let back: Vec<crate::save::SavedFlame> = ron::from_str(&ron).unwrap();
        let blocks = &server.content.blocks;
        let flames = &server.content.flames;
        assert_eq!(super::restore(&back, |name| blocks.id(name), |b| flames.burn_life(b) > 0), server.flames);
        assert!(super::restore(&back, |_| None, |_| true).is_empty(), "a light whose block is gone is dropped");
        assert!(super::restore(&back, |name| blocks.id(name), |_| false).is_empty(), "a light that no longer burns out is dropped");
    }

    /// What breaking the block at `pos` drops (it is the player's own), and the block gone.
    fn pick_up(server: &mut Server, pos: IVec3) -> ItemStack {
        server.items.take_all().for_each(drop);
        server.drop_loot(1, &[pos], &[pos]);
        server.set_block(pos, BlockId::AIR);
        let dropped: Vec<ItemStack> = server.items.take_all().map(|d| d.stack).collect();
        assert_eq!(dropped.len(), 1, "one drop: {dropped:?}");
        dropped[0]
    }

    /// A candle picked up a day into its three keeps that day burnt as its wear, and set
    /// down again burns the two it has left, not three fresh; a lamp picked up lit is the
    /// lamp with its fat, never the empty one.
    #[test]
    fn a_candle_picked_up_burning_burns_on_from_there_set_down() {
        let mut server = fixture("base:candle");
        let day = TICKS_PER_DAY;
        server.day = 1000.0;
        act(&mut server, "core:place", TORCH - IVec3::Y, Face::PosY);
        assert_eq!(server.flames.get(&TORCH).map(|f| f.out), Some(1000 + 3 * day), "three days from when it was set");
        server.placed.insert(TORCH);
        server.day = (1000 + day) as f64;
        let candle = pick_up(&mut server, TORCH);
        assert_eq!((at_item(&server, candle), candle.wear), ("base:candle".into(), 1200), "a day burnt, of 3600");
        assert!(server.flames.is_empty(), "gone with it");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(candle);
        act(&mut server, "core:place", TORCH - IVec3::Y, Face::PosY);
        assert_eq!(at(&server, TORCH), "base:candle");
        assert_eq!(server.flames.get(&TORCH).map(|f| f.out), Some(1000 + 3 * day), "the two days it had left");
        server.day = (1000 + 3 * day) as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:candle_end", "burnt to its end on time");

        let lamp = TORCH + IVec3::X;
        server.set_block(lamp, id(&server, "base:clay_oil_lamp"));
        server.day += 4.0 * day as f64;
        let picked = pick_up(&mut server, lamp);
        assert_eq!((at_item(&server, picked), picked.wear), ("base:clay_oil_lamp".into(), 4800), "the lamp with a day of fat left");
    }

    fn at_item(server: &Server, stack: ItemStack) -> String {
        server.content.blocks.def(stack.item).name.clone()
    }

    /// In the last tenth of its burn a torch gutters, dimmer, burning on to the same end;
    /// picked up then it is a torch nearly burnt. Burnt out, its stub breaks into an unlit
    /// stub, which sets down as a stub and catches again held in a fire.
    #[test]
    fn a_torch_gutters_near_its_end_and_its_stub_is_picked_up_unlit() {
        let mut server = fixture("base:torch");
        let day = TICKS_PER_DAY;
        server.day = 1000.0;
        act(&mut server, "core:place", TORCH - IVec3::Y, Face::PosY);
        let out = 1000 + day;
        server.day = (out - day / 10 - 1) as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch", "more than a tenth left");
        server.day = (out - day / 10) as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch_guttering", "a tenth left");
        let (bright, low) = (server.content.blocks.def(id(&server, "base:torch")).light, server.content.blocks.def(id(&server, "base:torch_guttering")).light);
        assert!(low > 0 && low < bright, "guttering, dimmer: {low} against {bright}");
        assert_eq!(server.flames.get(&TORCH).map(|f| (f.out, f.block)), Some((out, id(&server, "base:torch_guttering"))), "burning on to the same end");
        let wall = TORCH + IVec3::new(2, 1, 0);
        server.set_block(wall - IVec3::Y, id(&server, "base:stone"));
        server.set_block(wall, id(&server, "base:wall_torch_east"));
        server.flames.get_mut(&wall).unwrap().out = out;
        server.burn_flames();
        assert_eq!(at(&server, wall), "base:wall_torch_guttering_east", "in its facing");

        let picked = pick_up(&mut server, TORCH);
        assert_eq!((at_item(&server, picked), picked.wear), ("base:torch".into(), 1080), "a torch with a tenth left");
        let picked = pick_up(&mut server, wall);
        assert_eq!((at_item(&server, picked), picked.wear), ("base:torch".into(), 1080), "a wall torch guttering gives a torch");
        server.set_block(TORCH, id(&server, "base:torch"));
        server.day = (out + day) as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch_stub");
        let stub = pick_up(&mut server, TORCH);
        assert_eq!((at_item(&server, stub), stub.wear), ("base:torch_stub".into(), 0), "an unlit stub");
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(stub);
        act(&mut server, "core:place", TORCH - IVec3::Y, Face::PosY);
        assert_eq!(at(&server, TORCH), "base:torch_stub", "set down unlit");
        assert!(!server.flames.contains_key(&TORCH));
        let fire = server.content.cooking.for_item(stub.item, &["fire".to_owned()]).map(|r| at_item(&server, ItemStack::new(r.output, 1)));
        assert_eq!(fire.as_deref(), Some("base:torch"), "held in a fire it catches");
    }

    /// A light set down inside a settlement is tended and never goes out there, keeping
    /// what it had burnt; one set down whole leaves no entry at all.
    #[test]
    fn a_light_in_a_settlement_is_tended_and_never_burns_out() {
        let mut server = fixture("base:torch");
        let town = server.overworld.settlements_near(IVec2::ZERO, 6000)[0].centre;
        let pos = IVec3::new(town.x, town.y + 1, town.z);
        server.world.insert(ColumnPos::of_block(pos), Arc::new(Column::default()));
        server.set_block(pos - IVec3::Y, id(&server, "base:stone"));
        server.set_block(pos, id(&server, "base:torch"));
        assert!(!server.flames.contains_key(&pos), "a whole light tended is no entry");
        // A mine under the settlement is no part of it: a torch there burns.
        let mine = pos - IVec3::Y * (server.survival.tend_height + 2);
        server.set_block(mine, id(&server, "base:torch"));
        assert_eq!(server.flames.get(&mine).map(|f| f.tended), Some(false), "a torch in a mine under it burns");
        server.set_block(mine, BlockId::AIR);
        let worn = TORCH + IVec3::X;
        server.set_block(worn, id(&server, "base:torch"));
        server.set_down_burnt(pos, id(&server, "base:torch"), 600);
        assert_eq!(server.flames.get(&pos).map(|f| (f.tended, f.out)), Some((true, TICKS_PER_DAY / 2)), "half a day left, kept");
        server.day += 30.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, pos), "base:torch", "a month on, tended");
        server.placed.insert(pos);
        assert_eq!(pick_up(&mut server, pos).wear, 600, "what it had burnt, no more");
        let saved = server.saved_flames();
        assert!(saved.iter().all(|s| s.tended == (s.pos == pos.to_array())));
    }

    /// A light with no burn when its column loads (set down before lights burnt out, or
    /// by the world) is given a full one, once a column: one forgotten after is not given
    /// another. In a settlement it is left tended.
    #[test]
    fn lights_from_before_are_given_a_burn_once_when_their_column_loads() {
        let mut server = fixture("base:torch");
        let torch = id(&server, "base:torch");
        let mut column = (**server.world.column(ColumnPos::new(0, 0)).unwrap()).clone();
        column.set_block(TORCH, torch);
        server.world.insert(ColumnPos::new(0, 0), Arc::new(column));
        assert!(server.flames.is_empty(), "set in the column, never seen burning");
        server.flames_loaded(ColumnPos::new(0, 0));
        assert_eq!(server.flames.get(&TORCH).map(|f| f.out), Some(server.day.floor() as u64 + TICKS_PER_DAY), "a full burn");
        assert_eq!(server.meta.flame_columns, vec![[0, 0]]);
        server.flames.clear();
        server.flames_loaded(ColumnPos::new(0, 0));
        assert!(server.flames.is_empty(), "once a column");

        let town = server.overworld.settlements_near(IVec2::ZERO, 6000)[0].centre;
        let pos = IVec3::new(town.x, town.y + 1, town.z);
        let mut column = Column::default();
        column.set_block(pos - ColumnPos::of_block(pos).origin(), torch);
        server.world.insert(ColumnPos::of_block(pos), Arc::new(column));
        server.flames_loaded(ColumnPos::of_block(pos));
        assert!(server.flames.is_empty(), "in a settlement, tended");
        assert_eq!(server.meta.flame_columns.len(), 2);
    }

    /// An entry whose block no longer burns out (a mod took its tags away) is forgotten,
    /// not put out into a stub.
    #[test]
    fn a_light_that_no_longer_burns_out_is_forgotten() {
        let mut server = fixture("base:torch");
        let stone = id(&server, "base:stone");
        server.set_block(TORCH, stone);
        server.flames.insert(TORCH, super::Flame { block: stone, out: 0, tended: false });
        server.burn_flames();
        assert!(server.flames.is_empty());
        assert_eq!(at(&server, TORCH), "base:stone");
    }

    /// When a light burning out leaves a home short of a light a step of it asks for, its
    /// owner is told once, naming the step.
    #[test]
    fn a_home_short_a_light_tells_its_owner() {
        let (mut server, client) = told_fixture();
        let bed = TORCH + IVec3::new(-2, 0, 0);
        let blocks = &server.content.blocks;
        let sleeping = blocks.ids().find(|&b| blocks.def(b).has_tag("bed") && blocks.upper(b).is_none()).unwrap();
        server.set_block(bed, sleeping);
        server.players.get_mut(&1).unwrap().bed = Some(bed.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        // Two lights, burning out in the same second.
        server.set_block(TORCH, id(&server, "base:torch"));
        server.set_block(TORCH + IVec3::Z, id(&server, "base:torch"));
        let had = server.content.effects.home_steps(&server.survey_around(bed, Some(1)));
        assert!(!had.is_empty(), "the home meets a step with its lights");
        while client.try_recv().ok().flatten().is_some() {}
        server.day += TICKS_PER_DAY as f64;
        server.burn_flames();
        let said = heard(&client);
        assert_eq!(said.iter().filter(|k| *k == "flame.home_short").count(), 1, "told once for both: {said:?}");
        assert!(server.players[&1].home_steps.len() < had.len(), "what the home meets noted after");
        assert_eq!(server.players[&1].home_anchor, Some(bed), "and where");

        // One back whose return is yet to be checked is told there, not by the burn pass too.
        server.set_block(TORCH, id(&server, "base:torch"));
        server.set_block(TORCH + IVec3::Z, id(&server, "base:torch"));
        let player = server.players.get_mut(&1).unwrap();
        (player.home_steps, player.home_unchecked) = (had.clone(), true);
        server.day += TICKS_PER_DAY as f64;
        server.burn_flames();
        assert!(!heard(&client).iter().any(|k| k == "flame.home_short"), "told by the burn pass while unchecked");
        assert_eq!(server.players[&1].home_steps, had, "left for the return check");
        server.check_home_return(1);
        assert_eq!(heard(&client), vec!["flame.home_short_away".to_owned()], "told once, on the check");
    }

    /// A candle burning out at home is replaced from a candle box there (`spare_lights`): one
    /// candle comes out of the box and the light burns on, in its facing, and the home is
    /// short of nothing. With the box empty, or the spares in a plain barrel, it burns out.
    #[test]
    fn a_candle_box_at_home_keeps_its_candles_lit() {
        let (mut server, client) = told_fixture();
        let bed = TORCH + IVec3::new(-2, 0, 0);
        let blocks = &server.content.blocks;
        let sleeping = blocks.ids().find(|&b| blocks.def(b).has_tag("bed") && blocks.upper(b).is_none()).unwrap();
        server.set_block(bed, sleeping);
        server.players.get_mut(&1).unwrap().bed = Some(bed.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        let candle = id(&server, "base:candle");
        let machines = server.content.machines.clone();
        let store = |server: &mut Server, pos: IVec3, name: &str, count: u8| {
            let block = id(server, name);
            server.set_block(pos, block);
            if server.factory.get(pos).is_none() {
                server.factory.insert(pos, block, &machines);
            }
            server.factory.state_mut(pos).unwrap().bin[0] = (count > 0).then(|| ItemStack::new(candle, count));
        };
        let held = |server: &Server, pos: IVec3| server.factory.get(pos).and_then(|(_, state)| state.bin[0]).map_or(0, |s| s.count);
        // Spares in a plain barrel are only kept, not set out.
        let barrel = TORCH + IVec3::new(-1, 0, 2);
        store(&mut server, barrel, "base:barrel", 2);
        server.set_block(TORCH, candle);
        server.day += 3.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:candle_end", "a barrel sets no candle out");
        assert_eq!(held(&server, barrel), 2);
        // A candle box sets one out in its place.
        let candle_box = TORCH + IVec3::new(-1, 1, 1);
        store(&mut server, candle_box, "base:oak_candle_box", 1);
        server.set_block(TORCH, candle);
        while client.try_recv().ok().flatten().is_some() {}
        server.day += 3.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:candle", "a candle from the box set out");
        assert_eq!(held(&server, candle_box), 0, "taken from the box");
        assert!(server.flames.get(&TORCH).is_some_and(|f| f.out > server.day.floor() as u64), "burning afresh");
        assert!(!heard(&client).iter().any(|k| k == "flame.home_short"), "the home was short of nothing");
        // The box empty, it burns out.
        server.day += 3.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:candle_end", "no spare left");
    }

    /// A light at home that burns out to something lit again is lit again on the spot from
    /// a kindling box there (`relights_lights`): a torch by one bundle of kindling, an oil
    /// lamp filled by one lard (what its `relight_with:` asks), in its facing; the lard is
    /// never spent on the torch. The box empty, it goes out.
    #[test]
    fn a_kindling_box_at_home_keeps_its_torches_and_lamps_lit() {
        let mut server = fixture("base:torch");
        let bed = TORCH + IVec3::new(-2, 0, 0);
        let blocks = &server.content.blocks;
        let sleeping = blocks.ids().find(|&b| blocks.def(b).has_tag("bed") && blocks.upper(b).is_none()).unwrap();
        server.set_block(bed, sleeping);
        server.players.get_mut(&1).unwrap().bed = Some(bed.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
        let (kindling_item, lard) = (id(&server, "base:kindling"), id(&server, "base:lard"));
        let kindling = TORCH + IVec3::new(-1, 0, 2);
        let block = id(&server, "base:oak_kindling_box");
        server.set_block(kindling, block);
        if server.factory.get(kindling).is_none() {
            let machines = server.content.machines.clone();
            server.factory.insert(kindling, block, &machines);
        }
        let bin = &mut server.factory.state_mut(kindling).unwrap().bin;
        (bin[0], bin[1]) = (Some(ItemStack::new(kindling_item, 1)), Some(ItemStack::new(lard, 1)));
        let held = |server: &Server| server.factory.get(kindling).unwrap().1.bin.iter().flatten().map(|s| (s.item, s.count)).collect::<Vec<_>>();
        let lamp = TORCH + IVec3::Z;
        server.set_block(TORCH, id(&server, "base:torch"));
        server.set_block(lamp, id(&server, "base:clay_oil_lamp"));
        // A day on the torch is out, and lit again by the kindling.
        server.day += TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch", "the torch lit again from the box");
        assert!(server.flames.get(&TORCH).is_some_and(|f| f.out > server.day.floor() as u64), "burning afresh");
        assert_eq!(held(&server), vec![(lard, 1)], "the kindling used, not the lard");
        // Five days on the lamp is dry, and filled by the lard; the torch has nothing left.
        server.day += 4.0 * TICKS_PER_DAY as f64;
        server.burn_flames();
        assert_eq!(at(&server, lamp), "base:clay_oil_lamp", "the lamp filled from the box");
        assert_eq!(at(&server, TORCH), "base:torch_stub", "nothing left to light the torch");
        assert!(held(&server).is_empty(), "the lard used");
        // Rain putting a torch out under open sky is no burning down: the box keeps its
        // kindling and the torch stays out.
        let bin = &mut server.factory.state_mut(kindling).unwrap().bin;
        bin[0] = Some(ItemStack::new(kindling_item, 2));
        server.set_block(TORCH, id(&server, "base:torch"));
        let rain = bring(&mut server, "rain");
        assert!(rain.precipitation * (1.0 - rain.snow) >= server.survival.douse_rain, "raining: {rain:?}");
        server.burn_flames();
        assert_eq!(at(&server, TORCH), "base:torch_stub", "rained out");
        assert_eq!(held(&server), vec![(kindling_item, 2)], "no kindling spent on the rain");
    }

    /// The keys of the lines player 1 was told since last asked.
    fn heard(client: &voxmar_protocol::ClientConnection) -> Vec<String> {
        let mut said = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Chat { text: voxmar_engine::lang::Words::Key(key, _), .. } = message {
                said.push(key);
            }
        }
        said
    }

    /// A player back after their home's lights burnt out while they were away is told,
    /// once the land around their home has loaded, what step it cost.
    #[test]
    fn a_player_back_is_told_what_lights_burnt_out_away_cost() {
        let (mut server, client) = told_fixture();
        let bed = TORCH + IVec3::new(-2, 0, 0);
        let blocks = &server.content.blocks;
        let sleeping = blocks.ids().find(|&b| blocks.def(b).has_tag("bed") && blocks.upper(b).is_none()).unwrap();
        server.set_block(bed, sleeping);
        server.set_block(TORCH, id(&server, "base:torch"));
        server.set_block(TORCH + IVec3::Z, id(&server, "base:torch"));
        let had = server.content.effects.home_steps(&server.survey_around(bed, Some(1)));
        // Away, the lights burnt out; back, with what the home met then.
        server.set_block(TORCH, id(&server, "base:torch_stub"));
        server.set_block(TORCH + IVec3::Z, id(&server, "base:torch_stub"));
        // What the home met, and where, come back through a save.
        let player = server.players.get_mut(&1).unwrap();
        (player.home_steps, player.home_anchor) = (had.clone(), Some(bed));
        let blocks = server.content.blocks.clone();
        let dir = std::env::temp_dir().join(format!("voxmar-home-steps-{}", std::process::id()));
        let save = crate::save::WorldSave::new(&dir);
        save.write_player("ada", &server.players[&1].saved(&server.content.skills, &server.content.effects), &blocks).unwrap();
        let read = save.read_player("ada", &blocks).unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let names: Vec<String> = had.iter().map(|&e| server.content.effects.get(e).name.clone()).collect();
        assert_eq!((read.home_steps, read.home_anchor), (names, Some(bed.to_array())));

        // Back with the land round a home far off not yet loaded: nothing said, still to check.
        let far = Vec3::new(5000.5, FLOOR as f32 + 1.0, 5000.5);
        let player = server.players.get_mut(&1).unwrap();
        (player.bed, player.home_steps, player.home_anchor, player.home_unchecked) = (Some(far), had.clone(), Some(far.floor().as_ivec3()), true);
        while client.try_recv().ok().flatten().is_some() {}
        server.check_home_return(1);
        assert!(heard(&client).is_empty() && server.players[&1].home_unchecked, "checked before its land loaded");

        // Steps noted of a home elsewhere (a new bed since) say nothing of this one's loss.
        let player = server.players.get_mut(&1).unwrap();
        (player.bed, player.home_anchor) = (Some(bed.as_vec3() + Vec3::new(0.5, 0.0, 0.5)), Some(bed + IVec3::new(40, 0, 0)));
        server.check_home_return(1);
        assert!(heard(&client).is_empty(), "told of another home's lights");
        assert_eq!((server.players[&1].home_unchecked, server.players[&1].home_anchor), (false, Some(bed)), "noted afresh here");

        let player = server.players.get_mut(&1).unwrap();
        (player.home_steps, player.home_anchor, player.home_unchecked) = (had, Some(bed), true);
        server.check_home_return(1);
        assert_eq!(heard(&client), vec!["flame.home_short_away".to_owned()]);
        assert!(!server.players[&1].home_unchecked);
        server.check_home_return(1);
        assert!(heard(&client).is_empty(), "told once");
    }

    /// A guttering torch is lit again whole by what kindles, burning a fresh day.
    #[test]
    fn a_guttering_torch_is_lit_again_whole() {
        let mut server = fixture("base:firebrand");
        server.set_block(TORCH, id(&server, "base:torch_guttering"));
        act(&mut server, "base:relight", TORCH, Face::NegX);
        assert_eq!(at(&server, TORCH), "base:torch");
        assert_eq!(server.flames.get(&TORCH).map(|f| f.out), Some(server.day.floor() as u64 + TICKS_PER_DAY));
    }

    /// `fixture`, with the client end of player 1's connection kept to hear what they are told.
    fn told_fixture() -> (Server, voxmar_protocol::ClientConnection) {
        let mut server = fixture("base:torch");
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.get_mut(&1).unwrap().connection = connection;
        (server, client)
    }
}
