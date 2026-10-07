//! Farming on the server: the crops players plant, growing stage by stage, and the
//! experience planting and harvesting earn.
//!
//! Growth is the cheapest update that works (docs/design-philosophy.md, principle 12, and
//! research/farming.md 12.4): each planted crop knows the tick its next stage is due, a
//! heap orders them, and the growth process, once a second, pops only the crops due. How
//! long a stage takes follows from the crop's rate where it stands (`crop::rate`: climate,
//! light, soil, water), worked out again at every stage, so a field costs nothing between
//! its few changes. A crop in a column nobody has loaded grows on in the heap and shows
//! its stage when the column comes back. A crop that cannot grow where it is waits and
//! looks again a minute later.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::{ColumnPos, WORLD_HEIGHT};
use voxmar_engine::crop::{self, Conditions, CropId};
use voxmar_engine::dropped;
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::schedule::Tier;
use voxmar_engine::skill::SkillId;
use voxmar_engine::traits;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::items::random3;
use crate::process::Process;
use crate::save::SavedCrop;

/// Ticks a crop that cannot grow waits before looking again.
const RETRY: u64 = 60 * TICKS_PER_SECOND as u64;
/// Blocks across within which water keeps a crop's ground moist (Minecraft's farmland).
const WATER_REACH: i32 = 4;
/// Blocks within which a lamp's light reaches a crop.
const LAMP_REACH: i32 = 7;

/// A crop growing where a player planted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planted {
    pub crop: CropId,
    pub stage: u8,
    /// Tick its next stage is due; `u64::MAX` once ripe.
    pub due: u64,
}

/// Every planted crop, and when each grows next.
#[derive(Default)]
pub struct Farm {
    planted: FxHashMap<IVec3, Planted>,
    /// By due tick, then place (as an array, which orders).
    queue: BinaryHeap<Reverse<(u64, [i32; 3])>>,
    /// Crops that grew while their column was not loaded, to show when it is.
    unseen: FxHashMap<ColumnPos, Vec<IVec3>>,
}

impl Farm {
    fn schedule(&mut self, pos: IVec3, planted: Planted) {
        self.planted.insert(pos, planted);
        if planted.due != u64::MAX {
            self.queue.push(Reverse((planted.due, pos.to_array())));
        }
    }

    pub fn saved(&self, name: impl Fn(CropId) -> String) -> Vec<SavedCrop> {
        let mut saved: Vec<SavedCrop> = self
            .planted
            .iter()
            .map(|(pos, p)| SavedCrop { position: pos.to_array(), crop: name(p.crop), stage: p.stage, due: p.due })
            .collect();
        saved.sort_by_key(|c| c.position);
        saved
    }

    /// Crops from a save, minus those whose crop no longer exists.
    pub fn restore(saved: Vec<SavedCrop>, id: impl Fn(&str) -> Option<(CropId, u8)>) -> Self {
        let mut farm = Farm::default();
        for crop in saved {
            let Some((id, stages)) = id(&crop.crop) else { continue };
            let pos = IVec3::from_array(crop.position);
            farm.schedule(pos, Planted { crop: id, stage: crop.stage.min(stages - 1), due: crop.due });
            farm.unseen.entry(ColumnPos::of_block(pos)).or_default().push(pos);
        }
        farm
    }
}

/// `base:growth`: grows the crops that are due.
pub(crate) struct Growth;

impl Process for Growth {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        while let Some(&Reverse((due, pos))) = server.farm.queue.peek() {
            if due > server.tick {
                break;
            }
            server.farm.queue.pop();
            server.grow(IVec3::from_array(pos), due);
        }
    }
}

impl Server {
    /// Moves the crop at `pos` on a stage, if it is still there and this is its turn.
    fn grow(&mut self, pos: IVec3, due: u64) {
        let Some(&planted) = self.farm.planted.get(&pos).filter(|p| p.due == due) else { return };
        let crop = self.content.crops.get(planted.crop);
        let column = ColumnPos::of_block(pos);
        let loaded = self.world.contains(column);
        // Gone (broken, built over): forget it.
        if loaded && self.block(pos) != Some(crop.stages[planted.stage as usize]) {
            self.farm.planted.remove(&pos);
            return;
        }
        let rate = if loaded { self.crop_rate(planted.crop, pos) } else { 1.0 };
        if rate <= 0.0 {
            self.farm.schedule(pos, Planted { due: self.tick + RETRY, ..planted });
            return;
        }
        let stage = planted.stage + 1;
        let next = Planted { stage, due: self.stage_due(planted.crop, pos, stage, rate), ..planted };
        self.farm.schedule(pos, next);
        if loaded {
            let block = self.content.crops.get(planted.crop).stages[stage as usize];
            self.set_block(pos, block);
        } else {
            self.farm.unseen.entry(column).or_default().push(pos);
        }
    }

    /// The tick a crop reaching `stage` now, growing at `rate`, reaches the next; never
    /// once ripe. Each stage takes its share of the crop's time, give or take a seventh,
    /// so a field ripens over a while rather than all at once.
    fn stage_due(&self, crop: CropId, pos: IVec3, stage: u8, rate: f32) -> u64 {
        let crop = self.content.crops.get(crop);
        if stage >= crop.ripe_stage() {
            return u64::MAX;
        }
        let ticks = crop.def.minutes * 60.0 * TICKS_PER_SECOND as f32 / crop.ripe_stage() as f32 / rate;
        let jitter = 0.85 + 0.3 * random3(stage as u64, pos, crop.stages[0].0 as u32)[0];
        self.tick + (ticks * jitter).round().max(1.0) as u64
    }

    /// How fast the crop grows at `pos` (see `crop::rate`).
    fn crop_rate(&self, crop: CropId, pos: IVec3) -> f32 {
        let blocks = &self.content.blocks;
        let site = self.overworld.site(pos.x, pos.z);
        let fertile = self.block(pos - IVec3::Y).is_some_and(|b| blocks.def(b).has_tag("fertile"));
        let water = blocks.id("base:water").and_then(|b| blocks.fluid(b)).map(|f| f.fluid);
        let watered = (-1..=0).any(|dy| {
            (-WATER_REACH..=WATER_REACH).any(|dz| {
                (-WATER_REACH..=WATER_REACH).any(|dx| {
                    let cell = pos + IVec3::new(dx, dy - 1, dz);
                    self.block(cell).and_then(|b| blocks.fluid(b)).is_some_and(|f| Some(f.fluid) == water)
                })
            })
        });
        let conditions =
            Conditions { temperature: site.temperature, moisture: site.soil_moisture(), light: self.light_at(pos), fertile, watered };
        crop::rate(&self.content.crops.get(crop).def, conditions)
    }

    /// The brighter of sky light (open sky straight above, through nothing that blocks
    /// light) and the light of the brightest lamp near `pos`, 0..=15.
    fn light_at(&self, pos: IVec3) -> u8 {
        let blocks = &self.content.blocks;
        let open = (pos.y + 1..WORLD_HEIGHT).all(|y| self.block(pos.with_y(y)).is_none_or(|b| blocks.info(b).attenuation < 15));
        if open {
            return 15;
        }
        let mut best = 0u8;
        for dy in -LAMP_REACH..=LAMP_REACH {
            for dz in -LAMP_REACH..=LAMP_REACH {
                for dx in -LAMP_REACH..=LAMP_REACH {
                    let distance = dx.abs() + dy.abs() + dz.abs();
                    if distance <= LAMP_REACH
                        && let Some(block) = self.block(pos + IVec3::new(dx, dy, dz))
                    {
                        best = best.max(blocks.info(block).emission.saturating_sub(distance as u8));
                    }
                }
            }
        }
        best
    }

    /// Moves every planted crop within `radius` of `at` on by up to `stages`, at once
    /// (the `/grow` command). Returns how many grew.
    pub(crate) fn grow_around(&mut self, at: voxmar_engine::glam::Vec3, radius: f32, stages: u8) -> usize {
        let near: Vec<(IVec3, Planted)> =
            self.farm.planted.iter().filter(|(pos, _)| (pos.as_vec3() + 0.5).distance(at) <= radius).map(|(&pos, &p)| (pos, p)).collect();
        let mut grown = 0;
        for (pos, planted) in near {
            let crop = self.content.crops.get(planted.crop);
            let stage = planted.stage.saturating_add(stages).min(crop.ripe_stage());
            if stage == planted.stage || self.block(pos) != Some(crop.stages[planted.stage as usize]) {
                continue;
            }
            let block = crop.stages[stage as usize];
            let rate = self.crop_rate(planted.crop, pos).max(0.05);
            let due = self.stage_due(planted.crop, pos, stage, rate);
            self.set_block(pos, block);
            self.farm.schedule(pos, Planted { stage, due, ..planted });
            grown += 1;
        }
        grown
    }

    /// Shows the stages crops reached while their column was away.
    pub(crate) fn show_grown_crops(&mut self, column: ColumnPos) {
        let Some(positions) = self.farm.unseen.remove(&column) else { return };
        for pos in positions {
            let Some(planted) = self.farm.planted.get(&pos).copied() else { continue };
            let crop = self.content.crops.get(planted.crop);
            let current = self.block(pos);
            // Still one of this crop's stages (not broken while away): show the stage.
            if current.is_some_and(|b| crop.stages.contains(&b)) {
                if self.world.set_block(pos, crop.stages[planted.stage as usize]).is_some() {
                    self.dirty.insert(column);
                }
            } else {
                self.farm.planted.remove(&pos);
            }
        }
    }

    /// Sows the seed `agent` holds on top of `ground`.
    pub(crate) fn plant(&mut self, agent: PlayerId, ground: IVec3) {
        let pos = ground + IVec3::Y;
        // The seed in hand, or the one a seed pouch in hand gives (see `actor_seed`).
        let Some(seed) = voxmar_engine::action::ActionContext::actor_seed(&crate::actions::context(self, agent)) else { return };
        let player = self.players.get_mut(&agent).expect("an actor");
        let selected = player.selected;
        let taken = if player.held() == Some(seed) { player.inventory.take_one(selected).is_some() } else { player.inventory.take_pouched(seed) };
        if !taken {
            return;
        }
        player.changed(traits::INVENTORY);
        let Some(id) = self.content.crops.planted_by(seed) else { return };
        let crop = self.content.crops.get(id);
        let sprout = crop.stages[0];
        self.set_block(pos, sprout);
        let rate = self.crop_rate(id, pos);
        let due = if rate > 0.0 { self.stage_due(id, pos, 0, rate) } else { self.tick + RETRY };
        self.farm.schedule(pos, Planted { crop: id, stage: 0, due });
        // Sowing's experience comes with the harvest: paid at sowing, sowing a seed and
        // breaking the sprout for it back paid without end.
    }

    /// Picks a ripe crop that fruits again: its harvest pops out and it goes back to its
    /// regrowing stage.
    pub(crate) fn pick(&mut self, agent: PlayerId, pos: IVec3) {
        let Some((id, _)) = self.block(pos).and_then(|b| self.content.crops.stage_of(b)) else { return };
        let crop = self.content.crops.get(id);
        let Some(regrow) = crop.def.regrow else { return };
        let block = crop.stages[regrow as usize];
        self.harvest(agent, id, pos, 0, false);
        self.set_block(pos, block);
        let rate = self.crop_rate(id, pos);
        let due = if rate > 0.0 { self.stage_due(id, pos, regrow, rate) } else { self.tick + RETRY };
        self.farm.schedule(pos, Planted { crop: id, stage: regrow, due });
    }

    /// What a ripe crop at `pos` yields for `agent`, popped out into the world, and the
    /// experience it earns. `salt` tells apart several harvests in one tick.
    /// `sown` adds the sowing's experience (a crop broken at harvest, not one picked to
    /// fruit again).
    pub(crate) fn harvest(&mut self, agent: PlayerId, id: CropId, pos: IVec3, salt: u32, sown: bool) {
        let crop = &self.content.crops.get(id);
        let (loot, skill) = (crop.harvest.clone(), crop.skill);
        let xp = crop.def.xp.harvest + if sown { crop.def.xp.plant } else { 0.0 };
        let ripe = self.content.blocks.def(crop.stages[crop.ripe_stage() as usize]).name.clone();
        self.yield_loot(agent, id, &loot, pos, salt);
        self.grant_xp(agent, skill, xp);
        self.discover(agent, voxmar_engine::journal::Page::Plants, &ripe, 1);
    }

    /// What foraging one wild plant of a crop at `pos` yields, and its experience.
    pub(crate) fn forage(&mut self, agent: PlayerId, id: CropId, pos: IVec3, salt: u32) {
        let crop = &self.content.crops.get(id);
        let (loot, skill, xp, level) = (crop.forage.clone(), crop.skill, crop.def.xp.forage, crop.def.level);
        self.yield_loot(agent, id, &loot, pos, salt);
        // No experience foraging what is above the forager's level.
        if self.check_level(agent, skill) >= level {
            self.grant_xp(agent, skill, xp);
        }
    }

    /// Rolls `loot` for `agent`'s level in the crop's skill and pops it out at `pos`.
    fn yield_loot(&mut self, agent: PlayerId, id: CropId, loot: &[(BlockId, crop::Loot)], pos: IVec3, salt: u32) {
        let crop = self.content.crops.get(id);
        let level = self.players.get(&agent).map_or(1, |p| self.content.skills.level(p.skills.xp(crop.skill)));
        let bonus = crop::level_bonus(level, crop.def.level, self.content.skills.max_level());
        let mut draws = (0u32..).map(|i| random3(self.tick, pos, salt.wrapping_mul(97).wrapping_add(i))[0]);
        let found = crop::roll(loot, bonus, || draws.next().expect("endless"));
        for (i, (item, count)) in found.into_iter().enumerate() {
            let body = dropped::popped(pos, random3(self.tick, pos, salt.wrapping_mul(131).wrapping_add(i as u32 + 1000)));
            self.items.spawn(ItemStack { item, count }, body, dropped::PICKUP_DELAY);
        }
    }

    /// Whether `block` is a ripe crop, and which.
    pub(crate) fn ripe_crop(&self, block: BlockId) -> Option<CropId> {
        let (id, stage) = self.content.crops.stage_of(block)?;
        (stage == self.content.crops.get(id).ripe_stage()).then_some(id)
    }

    /// Adds experience to a player's skill and tells them.
    pub(crate) fn grant_xp(&mut self, agent: PlayerId, skill: SkillId, xp: f32) {
        let Some(player) = self.players.get_mut(&agent) else { return };
        // Effects that sharpen (or dull) learning: a good breakfast, a storm overhead. All
        // boosts together stop at a quarter more (the plan's xp integrity, P26).
        let rate = player.modifiers.xp.min(voxmar_engine::skill::MAX_XP_BOOST);
        let tenths = (xp * rate * 10.0).round().max(0.0) as u32;
        if tenths == 0 {
            return;
        }
        player.skills.grant(skill, tenths);
        player.changed(traits::SKILLS);
        player.connection.send(ServerMessage::Xp { skill, tenths });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_crops_round_trip_by_name_and_show_when_their_column_loads() {
        let mut farm = Farm::default();
        farm.schedule(IVec3::new(5, 70, -3), Planted { crop: CropId(1), stage: 2, due: 900 });
        farm.schedule(IVec3::new(1, 70, 1), Planted { crop: CropId(0), stage: 3, due: u64::MAX });
        let saved = farm.saved(|id| format!("base:crop{}", id.0));
        let back = Farm::restore(saved, |name| match name {
            "base:crop0" => Some((CropId(0), 4)),
            "base:crop1" => Some((CropId(1), 4)),
            _ => None,
        });
        assert_eq!(back.planted, farm.planted);
        // Only growing crops wait in the queue; all wait to be shown.
        assert_eq!(back.queue.len(), 1);
        assert_eq!(back.unseen.values().map(Vec::len).sum::<usize>(), 2);
        // Crops that no longer exist are left out.
        assert!(Farm::restore(farm.saved(|_| "gone".into()), |_| None).planted.is_empty());
    }
}
