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
use voxmar_engine::coords::{ColumnPos, Face, WORLD_HEIGHT};
use voxmar_engine::crop::{self, Conditions, CropId};
use voxmar_engine::dropped;
use voxmar_engine::glam::IVec3;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::schedule::Tier;
use voxmar_engine::skill::SkillId;
use voxmar_engine::traits;
use voxmar_engine::action::ActionTarget;
use voxmar_engine::lang::Words;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::items::random3;
use crate::process::Process;
use crate::save::{Draw, SavedCrop};

/// Ticks a crop that cannot grow waits before looking again.
const RETRY: u64 = 60 * TICKS_PER_SECOND as u64;
/// Blocks across within which water keeps a crop's ground moist (Minecraft's farmland), as
/// does a block that waters (`waters`: a well) standing on the ground or sunk into it.
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
    /// A player sowed it: its harvest pays the sowing's experience.
    pub sown: bool,
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
    pub(crate) fn schedule(&mut self, pos: IVec3, planted: Planted) {
        self.planted.insert(pos, planted);
        if planted.due != u64::MAX {
            self.queue.push(Reverse((planted.due, pos.to_array())));
        }
    }

    pub fn saved(&self, name: impl Fn(CropId) -> String) -> Vec<SavedCrop> {
        let mut saved: Vec<SavedCrop> = self
            .planted
            .iter()
            .map(|(pos, p)| SavedCrop { position: pos.to_array(), crop: name(p.crop), stage: p.stage, due: p.due, sown: p.sown })
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
            farm.schedule(pos, Planted { crop: id, stage: crop.stage.min(stages - 1), due: crop.due, sown: crop.sown });
            farm.unseen.entry(ColumnPos::of_block(pos)).or_default().push(pos);
        }
        farm
    }

    /// The crop at `pos` is gone (broken): whether a player had sown it.
    pub(crate) fn take_sown(&mut self, pos: IVec3) -> bool {
        self.planted.remove(&pos).is_some_and(|p| p.sown)
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

    /// How fast the crop grows at `pos` (see `crop::rate`), faster near a hive's bees
    /// (`pollination`).
    fn crop_rate(&self, crop: CropId, pos: IVec3) -> f32 {
        crop::rate(&self.content.crops.get(crop).def, self.crop_conditions(pos)) * self.pollination(pos)
    }

    /// A block that waters crops (`waters`: a well) set down by `agent` from `item` where it
    /// keeps some tilled ground moist counts for the quests that ask a field watered
    /// (`Watering`), by the item: a well set facing east is still a well.
    pub(crate) fn placed_by_field(&mut self, agent: PlayerId, item: BlockId, cell: IVec3) {
        let blocks = &self.content.blocks;
        if !self.block(cell).is_some_and(|b| blocks.def(b).has_tag("waters")) {
            return;
        }
        // The ground a crop is sown on, a level below to a level above it (crop_conditions).
        let tilled = (-1..=1).any(|dy| {
            (-WATER_REACH..=WATER_REACH).any(|dz| (-WATER_REACH..=WATER_REACH).any(|dx| self.block(cell + IVec3::new(dx, dy, dz)).is_some_and(|b| blocks.def(b).has_tag("tilled"))))
        });
        if tilled {
            let name = blocks.def(item).name.clone();
            self.tally(agent, "watering", &name, 1);
        }
    }

    /// The climate, light, ground and water a crop at `pos` grows in.
    fn crop_conditions(&self, pos: IVec3) -> Conditions {
        let blocks = &self.content.blocks;
        let site = self.overworld.site(pos.x, pos.z);
        let fertile = self.block(pos - IVec3::Y).is_some_and(|b| blocks.def(b).has_tag("fertile"));
        let water = blocks.id("base:water").and_then(|b| blocks.fluid(b)).map(|f| f.fluid);
        let waters: Vec<BlockId> = blocks.tagged("waters").collect();
        // Water level with the ground or below it; a well on the ground too.
        let watered = (-1..=1).any(|dy| {
            (-WATER_REACH..=WATER_REACH).any(|dz| {
                (-WATER_REACH..=WATER_REACH).any(|dx| {
                    let cell = pos + IVec3::new(dx, dy - 1, dz);
                    self.block(cell).is_some_and(|b| waters.contains(&b) || (dy < 1 && blocks.fluid(b).is_some_and(|f| Some(f.fluid) == water)))
                })
            })
        });
        Conditions { temperature: site.temperature, moisture: site.soil_moisture(), light: self.light_at(pos), fertile, watered }
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

    /// Whether a crop planted within `radius` of `at` has a stage left to grow (what
    /// `grow_around` would move on).
    pub(crate) fn unripe_near(&self, at: voxmar_engine::glam::Vec3, radius: f32) -> bool {
        self.farm.planted.iter().any(|(&pos, planted)| {
            let crop = self.content.crops.get(planted.crop);
            (pos.as_vec3() + 0.5).distance(at) <= radius && planted.stage < crop.ripe_stage() && self.block(pos) == Some(crop.stages[planted.stage as usize])
        })
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
        let Some(id) = self.content.crops.planted_by(seed) else { return };
        // Where it could never grow, the seed stays in hand and the player hears why
        // (the client showed the sprout and gets the ground back), naming the crop by what
        // it bears ("Rice", "Wheat"), or else by its seed, never by a growing stage.
        let crop = self.content.crops.get(id);
        if let Some(why) = crop::unsuited(&crop.def, self.crop_conditions(pos)) {
            let named = self.content.blocks.id(&crop.def.name).unwrap_or(seed);
            self.refuse(agent, Words::key(why.key()).with("crop", Words::Block(self.content.blocks.def(named).name.clone())));
            self.reject(agent, ActionTarget::Block { block: ground, face: Face::PosY });
            return;
        }
        let player = self.players.get_mut(&agent).expect("an actor");
        let selected = player.selected;
        let taken = if player.held() == Some(seed) { player.inventory.take_one(selected).is_some() } else { player.inventory.take_pouched(seed) };
        if !taken {
            return;
        }
        player.changed(traits::INVENTORY);
        let crop = self.content.crops.get(id);
        let sprout = crop.stages[0];
        self.set_block(pos, sprout);
        let rate = self.crop_rate(id, pos);
        let due = if rate > 0.0 { self.stage_due(id, pos, 0, rate) } else { self.tick + RETRY };
        self.farm.schedule(pos, Planted { crop: id, stage: 0, due, sown: true });
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
        let sown = self.farm.planted.get(&pos).is_some_and(|p| p.sown);
        // Tomatoes nobody sowed are picked as they would be broken (see `unsown_ripe`).
        // Fruit on trees is anyone's.
        if sown || crop.def.fruit {
            self.harvest(agent, id, pos, 0, false);
        } else {
            self.unsown_ripe(agent, id, pos, 0);
        }
        self.set_block(pos, block);
        let rate = self.crop_rate(id, pos);
        let due = if rate > 0.0 { self.stage_due(id, pos, regrow, rate) } else { self.tick + RETRY };
        self.farm.schedule(pos, Planted { crop: id, stage: regrow, due, sown });
    }

    /// What a ripe crop at `pos` yields for `agent`, popped out into the world, and the
    /// experience it earns. `salt` is mixed into where its drops pop.
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
    pub(crate) fn yield_loot(&mut self, agent: PlayerId, id: CropId, loot: &[(BlockId, crop::Loot)], pos: IVec3, salt: u32) {
        let crop = self.content.crops.get(id);
        let level = self.players.get(&agent).map_or(1, |p| self.content.skills.level(p.skills.xp(crop.skill)));
        let bonus = crop::level_bonus(level, crop.def.level, self.content.skills.max_level());
        // Each draw numbered by the player's count of harvest draws, and where a drop pops
        // by the world's count of drops, never by the tick it is taken on.
        let (players, meta) = (&mut self.players, &mut self.meta);
        let found = crop::roll(loot, bonus, || match players.get_mut(&agent) {
            Some(player) => player.draw(Draw::Harvest)[0],
            None => meta.next_roll(Draw::Harvest.salt())[0],
        });
        for (i, (item, count)) in found.into_iter().enumerate() {
            let body = dropped::popped(pos, self.meta.next_pop(pos, salt.wrapping_mul(131).wrapping_add(i as u32 + 1000)));
            self.items.spawn(ItemStack::new(item, count), body, dropped::PICKUP_DELAY);
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

    /// Crops whose seed nothing supplies yet, waiting on another thread's change.
    const AWAITING_SETTLEMENTS: [&str; 0] = [];

    #[test]
    fn every_sown_crop_is_obtainable() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_engine::content::Content::load(&dir).unwrap();
        let world = voxmar_worldgen::WorldContent::load(&content).unwrap();
        let crops = &content.crops;
        let fields: Vec<&str> = world.settlements.styles.iter().flat_map(|s| s.crops.iter().map(|(c, _)| c.as_str())).collect();
        let mut nowhere = Vec::new();
        for (id, crop) in crops.iter() {
            let Some(seed) = crop.seed else { continue };
            let gives = |loot: &[(BlockId, crop::Loot)]| loot.iter().any(|(item, _)| *item == seed);
            // Foraged from its wild form, given by another crop, or harvested from a
            // settlement's field.
            // A wild form the world grows (plants.ron), or the crop itself grown wild and
            // ripe, or it is found nowhere.
            let ripe = &crop.def.stages[crop.ripe_stage() as usize];
            let generated = crop.def.wild.iter().chain([ripe]).any(|wild| world.plants.iter().any(|p| p.block == *wild || format!("base:{}", p.block) == *wild));
            let foraged = generated && gives(&crop.forage);
            let other = crops.iter().any(|(other, c)| other != id && (gives(&c.harvest) || gives(&c.forage)));
            // Village fields grow unripe, so what an unripe stage drops.
            let field = fields.iter().any(|f| *f == crop.def.name || format!("base:{f}") == crop.def.name) && content.blocks.info(crop.stages[0]).drops == seed;
            let name = crop.def.name.as_str();
            if !(foraged || other || field || AWAITING_SETTLEMENTS.contains(&name)) {
                nowhere.push(name.to_owned());
            }
        }
        assert!(nowhere.is_empty(), "nothing in survival gives these crops' seeds: {nowhere:?}");
    }

    /// A well waters crops as water does, four blocks across, standing on the ground by the
    /// field or sunk level with it; no further.
    #[test]
    fn a_well_waters_the_field_about_it() {
        let mut server = crate::test_server();
        server.world = Default::default();
        const FLOOR: i32 = 199;
        let farmland = server.content.blocks.expect("base:farmland");
        let well = server.content.blocks.expect("base:well");
        let mut column = voxmar_engine::world::Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, FLOOR, z), farmland);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), std::sync::Arc::new(column));
        let crop = IVec3::new(8, FLOOR + 1, 8);
        assert!(!server.crop_conditions(crop).watered, "dry ground");
        for (at, watered) in [(IVec3::new(12, FLOOR + 1, 8), true), (IVec3::new(12, FLOOR, 12), true), (IVec3::new(13, FLOOR + 1, 8), false), (IVec3::new(8, FLOOR + 2, 8), false)] {
            server.set_block(at, well);
            assert_eq!(server.crop_conditions(crop).watered, watered, "a well at {at}");
            server.set_block(at, if at.y == FLOOR { farmland } else { BlockId::AIR });
        }
    }

    /// Set down by tilled ground, a well counts for the quests that ask a field watered;
    /// set down where it keeps nothing moist, it does not.
    #[test]
    fn a_well_set_by_tilled_ground_counts_as_watering_it() {
        let mut server = crate::test_server();
        server.world = Default::default();
        const FLOOR: i32 = 199;
        let (stone, farmland, well) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:farmland"), server.content.blocks.expect("base:well"));
        let mut column = voxmar_engine::world::Column::default();
        for x in 0..32 {
            for z in 0..32 {
                column.set_block(IVec3::new(x, FLOOR, z), stone);
            }
        }
        server.world.insert(ColumnPos::new(0, 0), std::sync::Arc::new(column));
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = voxmar_engine::glam::Vec3::new(8.5, (FLOOR + 1) as f32, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        player.inventory.slots[0] = Some(ItemStack::new(well, 2));
        player.selected = 0;
        server.players.insert(1, player);
        let place = server.content.actions.expect("core:place");
        let watering = |server: &Server| server.players[&1].tallies.get("watering:base:well").copied().unwrap_or(0);
        let waters = |server: &Server, at: IVec3| server.block(at).map(|b| server.content.blocks.def(b).name.clone());
        server.propose(1, place, voxmar_engine::action::ActionTarget::Block { block: IVec3::new(10, FLOOR, 8), face: Face::PosY }, std::time::Instant::now());
        assert_eq!(waters(&server, IVec3::new(10, FLOOR + 1, 8)).as_deref(), Some("base:well_west"), "set facing the player");
        assert_eq!(watering(&server), 0, "no tilled ground near");
        // Farmland four blocks off, level with the floor the next well stands on.
        server.set_block(IVec3::new(10, FLOOR, 14), farmland);
        server.propose(1, place, voxmar_engine::action::ActionTarget::Block { block: IVec3::new(10, FLOOR, 10), face: Face::PosY }, std::time::Instant::now());
        assert!(server.block(IVec3::new(10, FLOOR + 1, 10)).is_some_and(|b| server.content.blocks.def(b).has_tag("waters")));
        assert_eq!(watering(&server), 1, "a well by the field, whichever way it faces");
        assert_eq!(server.players[&1].tallies.get("placed:base:well"), None, "only a block some quest's `Placed` asks for is tallied set down");
    }

    #[test]
    fn saved_crops_round_trip_by_name_and_show_when_their_column_loads() {
        let mut farm = Farm::default();
        farm.schedule(IVec3::new(5, 70, -3), Planted { crop: CropId(1), stage: 2, due: 900, sown: true });
        farm.schedule(IVec3::new(1, 70, 1), Planted { crop: CropId(0), stage: 3, due: u64::MAX, sown: false });
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

    /// A harvest is rolled by the player's count of harvest draws, and pops by the world's
    /// count of drops, not the tick it is taken on, and each counts on.
    #[test]
    fn a_harvest_rolls_by_the_players_count_not_the_tick() {
        let harvested = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let (client, connection) = voxmar_protocol::local_pair();
            std::mem::forget(client);
            let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
            (player.joined, player.draw_key) = (true, crate::player::player_draw_key(0x0d1a, "ada"));
            server.players.insert(1, player);
            let (wheat, crop) = server.content.crops.iter().find(|(_, c)| c.def.name == "base:wheat").unwrap();
            let loot = crop.harvest.clone();
            let pos = server.spawn.floor().as_ivec3();
            server.tick = tick;
            let mut found = Vec::new();
            for salt in 0..12 {
                server.yield_loot(1, wheat, &loot, pos, salt);
                let mut lying: Vec<_> = server.items.take_all().map(|d| (d.stack.item, d.stack.count, d.body.position.to_array(), d.body.velocity.to_array())).collect();
                lying.sort_by(|a, b| a.partial_cmp(b).unwrap());
                found.push(lying);
            }
            (found, server.players[&1].draws.harvests, server.meta.drops)
        };
        let first = harvested(1_000);
        assert!(first.1 > 0 && first.0.iter().all(|f| !f.is_empty()), "twelve harvests drew {} times for {:?}", first.1, first.0);
        assert!(first.0.windows(2).any(|w| w[0].iter().map(|f| f.1).ne(w[1].iter().map(|f| f.1))), "twelve harvests all gave alike: {:?}", first.0);
        assert_eq!(harvested(1_377), first, "the same harvests later gave otherwise");
    }
}
