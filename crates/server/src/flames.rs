//! Placed lights burning out and lit again (`voxmar_engine::flame`, progression piece 8b).
//! A torch set down burns `burns_days:` game days by the world's clock, then goes out to
//! its stub; rain on it under open sky puts it out sooner. A right click on the stub with
//! anything that kindles (a firebrand, a lit torch) lights it again for nothing: the cost
//! is coming back, never coal. Only what is set down after this counts: torches already in
//! the world, and those a settlement was built with, burn on.

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::glam::{IVec3, Vec2};
use voxmar_protocol::{PlayerId, TICKS_PER_DAY};

use crate::Server;
use crate::save::SavedFlame;

/// A light set down and burning: what it is, and the world's day tick it goes out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flame {
    pub block: BlockId,
    pub out: u64,
}

impl Server {
    /// `block` was set at `pos`: a light that burns out starts burning there, anything
    /// else forgets the one that was. One lookup, so every block set can ask.
    pub(crate) fn flame_changed(&mut self, pos: IVec3, block: BlockId) {
        match self.content.flames.burns_out(&self.content.blocks, block) {
            Some((days, _)) => {
                let out = self.day.floor() as u64 + (days * TICKS_PER_DAY as f32).round() as u64;
                self.flames.insert(pos, Flame { block, out });
            }
            None => {
                self.flames.remove(&pos);
            }
        }
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

    /// Puts out the lights whose time is up, and those rain falls on. Those in columns
    /// not loaded wait; one no longer there is forgotten.
    pub(crate) fn burn_flames(&mut self) {
        if self.flames.is_empty() {
            return;
        }
        let (now, tick, day, wet) = (self.day.floor() as u64, self.tick, self.day, self.survival.douse_rain);
        let mut out = Vec::new();
        let mut gone = Vec::new();
        // Rain is the same over a column of blocks: sampled once for each.
        let mut raining: FxHashMap<(i32, i32), bool> = FxHashMap::default();
        let flames: Vec<(IVec3, Flame)> = self.flames.iter().map(|(&p, &f)| (p, f)).collect();
        for (pos, flame) in flames {
            let Some(here) = self.block(pos) else { continue };
            if here != flame.block {
                gone.push(pos);
                continue;
            }
            if now >= flame.out {
                out.push(pos);
                continue;
            }
            if !self.content.flames.douses(here) || !self.open_sky(pos + IVec3::Y) {
                continue;
            }
            let column = (pos.x.div_euclid(16), pos.z.div_euclid(16));
            let rain = *raining.entry(column).or_insert_with(|| {
                let report = self.weather.local(&mut self.temperature, Vec2::new(pos.x as f32, pos.z as f32), tick, day);
                report.weather.precipitation * (1.0 - report.weather.snow) >= wet
            });
            if rain {
                out.push(pos);
            }
        }
        for pos in gone {
            self.flames.remove(&pos);
        }
        for pos in out {
            let Some(here) = self.block(pos) else { continue };
            if let Some((_, stub)) = self.content.flames.burns_out(&self.content.blocks, here) {
                self.set_block(pos, stub);
            }
        }
    }

    pub(crate) fn saved_flames(&self) -> Vec<SavedFlame> {
        let mut saved: Vec<SavedFlame> = self.flames.iter().map(|(pos, f)| SavedFlame { pos: pos.to_array(), block: self.content.blocks.def(f.block).name.clone(), out: f.out }).collect();
        saved.sort_by_key(|s| s.pos);
        saved
    }
}

/// Saved burning lights back by id, dropping those whose blocks no longer exist.
pub(crate) fn restore(saved: &[SavedFlame], id: impl Fn(&str) -> Option<BlockId>) -> FxHashMap<IVec3, Flame> {
    saved.iter().filter_map(|s| Some((IVec3::from_array(s.pos), Flame { block: id(&s.block)?, out: s.out }))).collect()
}

/// Puts out burnt-out and rained-on lights once a second.
#[derive(Default)]
pub(crate) struct Flames;

impl crate::process::Process for Flames {
    fn tier(&self) -> voxmar_engine::schedule::Tier {
        voxmar_engine::schedule::Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
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
        assert_eq!(at(&server, TORCH), "base:torch", "a tick short of a day");
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
        assert_eq!(super::restore(&back, |name| blocks.id(name)), server.flames);
        assert!(super::restore(&back, |_| None).is_empty(), "a light whose block is gone is dropped");
    }
}
