//! Taming, keeping and breeding on the server (`engine::creature::husbandry`,
//! docs/breeding.md): players tend creatures with what they hold (`base:tend`), kept
//! creatures give their products on timers and breed when two ready adults meet, and kept
//! creatures are saved with the world.

use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::husbandry::{self, IN_LOVE_SECONDS, MATE_RANGE, YOUNG_SCALE};
use voxmar_engine::creature::brain::Mind;
use voxmar_engine::creature::{CreatureId, mix, unit_from};
use voxmar_engine::crop::level_bonus;
use voxmar_engine::dropped;
use voxmar_engine::genes;
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::skill::SkillId;
use voxmar_engine::traits;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, TICKS_PER_SECOND};

use crate::Server;
use crate::commands::Source;
use crate::creatures::product_ticks;
use crate::items::random3;

/// How close two ready adults come before they breed, in blocks.
const MEET: f32 = 1.6;
/// Ticks between husbandry passes (products, breeding, growing up): twice a second.
const EVERY: u64 = TICKS_PER_SECOND as u64 / 2;

/// A kept creature as a save keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCreature {
    pub species: String,
    pub position: [f32; 3],
    pub home: [f32; 3],
    pub yaw: f32,
    pub health: u16,
    pub owner: String,
    pub seed: u64,
    pub scale: f32,
    /// Ticks until it grows up, if young.
    pub grows_in: Option<u64>,
    /// Ticks until each product is due.
    pub products_in: Vec<u64>,
    /// Its genes; none in saves from before genes, which restore as wild ones of the place.
    #[serde(default)]
    pub genome: Option<genes::Genome>,
}

impl Server {
    fn player_name(&self, player: PlayerId) -> Option<String> {
        self.players.get(&player).map(|p| p.name.clone())
    }

    pub(crate) fn skill_level(&self, player: PlayerId, skill: &str) -> u8 {
        let Some(id) = self.content.skills.id(skill) else { return 99 };
        self.players.get(&player).map_or(1, |p| self.content.skills.level(p.skills.xp(id)))
    }

    fn grant(&mut self, player: PlayerId, skill: Option<SkillId>, xp: f32) {
        if let Some(skill) = skill {
            self.grant_xp(player, skill, xp);
        }
    }

    /// Whether creature `id` takes `item` from `player`: food that tames it (if wild), food
    /// that readies it to breed (if theirs and grown), or a tool for a product of its.
    pub(crate) fn creature_accepts(&self, player: PlayerId, id: u32, item: BlockId) -> bool {
        let Some(creature) = self.creatures.live.get(&id) else { return false };
        let species = self.content.creatures.get(creature.species);
        let tags = &self.content.blocks.def(item).tags;
        let mine = creature.owner.is_some() && creature.owner == self.player_name(player);
        // One too wild for the player yet turns the food down rather than eat it for nothing.
        let can_tame = species.def.taming.as_ref().is_some_and(|t| self.skill_level(player, "core:hunter") >= t.level);
        // A taming treat any tameable creature takes, and feed any kept one breeds on.
        let treat = tags.iter().any(|t| t == "taming_treat");
        let feed = tags.iter().any(|t| t == "breeding_feed");
        let tames = creature.owner.is_none() && can_tame && (treat || species.tame_food.contains(&item));
        // Nor does a kept one take breeding food it cannot use yet (the player's Farming too
        // low, or it bred too recently).
        let can_breed = species.def.breeding.as_ref().is_some_and(|b| self.skill_level(player, "core:farming") >= b.level) && self.tick >= creature.breeds_again;
        let breeds = mine && creature.grows_up.is_none() && can_breed && (feed || species.breed_food.contains(&item));
        let tool = mine && species.def.products.iter().any(|p| p.tool.as_ref().is_some_and(|t| tags.contains(t)));
        tames || breeds || tool
    }

    /// `base:tend`: feeds the creature what the player holds, or takes its product with it.
    pub(crate) fn tend(&mut self, player: PlayerId, id: u32) {
        let Some(item) = self.players.get(&player).and_then(|p| p.held()) else { return };
        if !self.creature_accepts(player, id, item) {
            return;
        }
        let creature = &self.creatures.live[&id];
        let species = self.content.creatures.get(creature.species);
        let tags = self.content.blocks.def(item).tags.clone();
        // A tool takes a product that is due.
        let product = species.def.products.iter().enumerate().find(|(_, p)| p.tool.as_ref().is_some_and(|t| tags.contains(t)));
        if let Some((index, product)) = product {
            if self.tick < creature.products_due[index] {
                return;
            }
            let (item, (low, high), every, xp) = (species.products[index], product.count, product.every, product.xp);
            let [roll, ..] = random3(self.tick, creature.body.position.floor().as_ivec3(), id);
            let count = yielded(low + ((roll * (high - low + 1) as f32) as u8).min(high - low), creature.traits.yield_, roll);
            let seed = creature.seed;
            let creature = self.creatures.live.get_mut(&id).expect("checked");
            creature.products_due[index] = self.tick + product_ticks(every, seed, index);
            let farming = self.content.skills.id("core:farming");
            if let Some(p) = self.players.get_mut(&player) {
                let left = p.inventory.add(item, count);
                p.changed(traits::INVENTORY);
                if left > 0 {
                    let body = dropped::thrown(p.eye(), p.look());
                    self.items.spawn(ItemStack { item, count: left }, body, dropped::PICKUP_DELAY);
                }
            }
            self.grant(player, farming, xp);
            return;
        }
        // A wild one past its would-be keeper's cap is not taken in: taming waits for room,
        // and the food stays in hand.
        let kept_each = self.content.creatures.limits().kept_each;
        if creature.owner.is_none() && self.player_name(player).is_some_and(|name| self.kept_by(&name) >= kept_each) {
            self.tell(Source::Player(player), ChatKind::Info, Words::key("husbandry.kept_full").with("count", Words::text(kept_each)));
            return;
        }
        // Nor is a kept one readied to breed when no young could come of it.
        if let Some(keeper) = creature.owner.as_deref()
            && self.birth_room(keeper) == 0
        {
            self.tell(Source::Player(player), ChatKind::Info, self.no_room_for_young(keeper));
            return;
        }
        // Food: one is eaten.
        if let Some(p) = self.players.get_mut(&player) {
            let selected = p.selected;
            if p.inventory.take_one(selected).is_none() {
                return;
            }
            p.changed(traits::INVENTORY);
        }
        let name = self.player_name(player);
        let creature = &self.creatures.live[&id];
        if creature.owner.is_none() {
            let Some(taming) = species.def.taming.clone() else { return };
            let u = unit_from(mix(creature.seed, self.tick ^ 0x7a3e));
            let creature = self.creatures.live.get_mut(&id).expect("checked");
            if creature.trusting != name {
                creature.trusting = name.clone();
                creature.trust = 0.0;
            }
            // A treat (`trust:2`) wins trust faster than plain food.
            let gladly = tags.iter().find_map(|t| t.strip_prefix("trust:")?.parse::<f32>().ok()).unwrap_or(1.0);
            creature.trust += husbandry::trust_gain(&taming, u) * gladly;
            creature.mind.fear = 0.0;
            creature.changed = true;
            if creature.trust >= 1.0 {
                creature.owner = name;
                creature.owner_id = Some(player);
                creature.home = creature.body.position;
                // For quests that ask for tamed animals.
                let tamed = species.def.name.clone();
                let hunter = self.content.skills.id("core:hunter");
                self.grant(player, hunter, taming.xp);
                self.tally(player, "tamed", &tamed, 1);
            }
            return;
        }
        // A kept adult fed its breeding food is ready to breed for a while.
        let Some(breeding) = species.def.breeding.clone() else { return };
        if self.skill_level(player, "core:farming") < breeding.level || self.tick < creature.breeds_again {
            return;
        }
        let creature = self.creatures.live.get_mut(&id).expect("checked");
        creature.ready_until = self.tick + (IN_LOVE_SECONDS * TICKS_PER_SECOND as f32) as u64;
        creature.changed = true;
    }

    /// Where a ready creature's mate is: another of its kind, ready too, within range.
    pub(crate) fn mate_for(&self, id: u32) -> Option<Vec3> {
        let creature = self.creatures.live.get(&id)?;
        if creature.ready_until == 0 {
            return None;
        }
        let at = creature.body.position;
        self.creatures
            .live
            .iter()
            .filter(|(other, c)| **other != id && c.species == creature.species && c.ready_until > 0 && c.body.position.distance(at) < MATE_RANGE)
            .map(|(_, c)| c.body.position)
            .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
    }

    /// Products laid on the ground, young growing up, readiness running out, and ready
    /// pairs that have met breeding.
    pub(crate) fn tick_husbandry(&mut self) {
        if !self.tick.is_multiple_of(EVERY) {
            return;
        }
        let tick = self.tick;
        // Owners' ids, for the players here now.
        let ids: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| c.owner.is_some()).map(|(&id, _)| id).collect();
        for &id in &ids {
            let owner = self.creatures.live[&id].owner.clone();
            let present = self.players.values().find(|p| p.joined && Some(&p.name) == owner.as_ref()).map(|p| p.id);
            let creature = self.creatures.live.get_mut(&id).expect("listed");
            if creature.owner_id != present {
                creature.owner_id = present;
                creature.changed = true;
            }
        }
        let mut laid = Vec::new();
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (&id, creature) in self.creatures.live.iter_mut() {
            if creature.ready_until > 0 && tick >= creature.ready_until {
                creature.ready_until = 0;
                creature.changed = true;
            }
            if let Some(grows) = creature.grows_up
                && tick >= grows
            {
                creature.grows_up = None;
                creature.scale = creature.adult_scale;
                let def = &self.content.creatures.get(creature.species).def;
                creature.body.size = (def.body.size.0 * creature.scale, def.body.size.1 * creature.scale);
                creature.changed = true;
            }
            if creature.owner.is_none() || creature.grows_up.is_some() {
                continue;
            }
            let species = self.content.creatures.get(creature.species);
            for (index, product) in species.def.products.iter().enumerate() {
                if product.tool.is_none() && tick >= creature.products_due[index] {
                    creature.products_due[index] = tick + product_ticks(product.every, creature.seed ^ tick, index);
                    laid.push((creature.body.position, species.products[index], product.count, creature.traits.yield_, id));
                }
            }
        }
        for (position, item, (low, high), yield_, id) in laid {
            let cell = position.floor().as_ivec3();
            let [a, b, c] = random3(tick, cell, id);
            let count = yielded(low + ((a * (high - low + 1) as f32) as u8).min(high - low), yield_, c);
            self.items.spawn(ItemStack { item, count }, dropped::popped(cell, [b, c, a]), dropped::PICKUP_DELAY);
        }
        // Ready pairs that have met.
        let ready: Vec<(u32, CreatureId, Vec3)> =
            self.creatures.live.iter().filter(|(_, c)| c.ready_until > 0).map(|(&id, c)| (id, c.species, c.body.position)).collect();
        for (i, &(a, species, at)) in ready.iter().enumerate() {
            if pairs.iter().any(|&(x, y)| x == a || y == a) {
                continue;
            }
            if let Some(&(b, ..)) = ready[i + 1..].iter().find(|(b, s, p)| *s == species && p.distance(at) < MEET && !pairs.iter().any(|&(x, y)| x == *b || y == *b)) {
                pairs.push((a, b));
            }
        }
        for (a, b) in pairs {
            self.breed(a, b);
        }
    }

    /// Two ready parents make young beside them; both rest a while after.
    fn breed(&mut self, a: u32, b: u32) {
        let (species, at, owner, seed) = {
            let parent = &self.creatures.live[&a];
            (parent.species, parent.body.position, parent.owner.clone(), parent.seed ^ self.creatures.live[&b].seed)
        };
        let Some(breeding) = self.content.creatures.get(species).def.breeding.clone() else { return };
        for id in [a, b] {
            let parent = self.creatures.live.get_mut(&id).expect("a parent");
            parent.ready_until = 0;
            parent.changed = true;
        }
        // Young take room under their keeper's own cap and under the world's total. Room
        // may have gone since the parents were readied (another tamed, a waiting one back).
        let room = owner.as_deref().map_or(0, |keeper| self.birth_room(keeper));
        let player = self.players.values().find(|p| Some(&p.name) == owner.as_ref()).map(|p| p.id);
        if room == 0 {
            // No birth: the parents may try again as soon as there is room, at no cost.
            if let (Some(player), Some(keeper)) = (player, owner.as_deref()) {
                self.tell(Source::Player(player), ChatKind::Info, self.no_room_for_young(keeper));
            }
            return;
        }
        let (low, high) = breeding.litter;
        let count = (low + ((unit_from(mix(seed, self.tick)) * (high - low + 1) as f32) as u8).min(high - low)).max(1).min(room.min(u8::MAX as usize) as u8);
        let grows = self.tick + (breeding.growth * 60.0 * TICKS_PER_SECOND as f32) as u64;
        let (mother, father) = (self.creatures.live[&a].genome.clone(), self.creatures.live[&b].genome.clone());
        // Mutations are likelier where aether is high (docs/breeding.md, "Mutation").
        let aether = self.overworld.site(at.x.floor() as i32, at.z.floor() as i32).aether;
        let mutation = genes::MUTATION * (1.0 + 4.0 * aether.clamp(0.0, 1.0));
        let mut born = 0;
        for i in 0..count {
            let Some(id) = self.spawn_creature(species, at + Vec3::new(0.3 * i as f32, 0.0, 0.3), seed ^ self.tick ^ i as u64) else { continue };
            born += 1;
            let def = &self.content.creatures.get(species).def;
            let young = self.creatures.live.get_mut(&id).expect("just made");
            young.genome = genes::inherit(&def.genes, &mother, &father, young.seed, mutation);
            young.traits = genes::express(&def.genes, &young.genome);
            young.mind = Mind::new(&def.temperament, young.seed);
            young.mind.personality.aggression = (young.mind.personality.aggression + young.traits.temper).clamp(0.0, 1.0);
            if !def.genes.is_empty() {
                young.adult_scale = young.traits.scale.max(0.5);
            }
            young.owner = owner.clone();
            young.grows_up = Some(grows);
            young.scale = young.adult_scale * YOUNG_SCALE;
            young.body.size = (def.body.size.0 * young.scale, def.body.size.1 * young.scale);
        }
        // A birth costs the parents their rest and pays the keeper; none costs nothing.
        if born == 0 {
            return;
        }
        let again = self.tick + (breeding.cooldown * 60.0 * TICKS_PER_SECOND as f32) as u64;
        for id in [a, b] {
            if let Some(parent) = self.creatures.live.get_mut(&id) {
                parent.breeds_again = again;
            }
        }
        // Experience to the owner, if they are here, by their Farming level over the
        // species' (more for a harder breed, as crops do).
        if let Some(player) = player {
            let farming = self.content.skills.id("core:farming");
            let level = self.skill_level(player, "core:farming");
            let bonus = 1.0 + level_bonus(level, breeding.level, self.content.skills.max_level());
            self.grant(player, farming, breeding.xp * bonus);
        }
    }

    /// Why a keeper's animals can have no young now: the keeper's own cap, or a world that
    /// holds all the creatures it can.
    fn no_room_for_young(&self, keeper: &str) -> Words {
        let kept_each = self.content.creatures.limits().kept_each;
        if self.kept_by(keeper) >= kept_each {
            Words::key("husbandry.no_room_to_breed").with("count", Words::text(kept_each))
        } else {
            Words::key("husbandry.world_full")
        }
    }

    /// The kept creatures, as the save keeps them.
    pub(crate) fn saved_creatures(&self) -> Vec<SavedCreature> {
        let tick = self.tick;
        let waiting = self.creatures.unplaced.iter().cloned();
        self.creatures
            .live
            .values()
            .filter_map(|c| {
                let owner = c.owner.clone()?;
                Some(SavedCreature {
                    species: self.content.creatures.get(c.species).def.name.clone(),
                    position: c.body.position.to_array(),
                    home: c.home.to_array(),
                    yaw: c.body.yaw,
                    health: c.health,
                    owner,
                    seed: c.seed,
                    scale: c.adult_scale,
                    grows_in: c.grows_up.map(|t| t.saturating_sub(tick)),
                    products_in: c.products_due.iter().map(|t| t.saturating_sub(tick)).collect(),
                    genome: Some(c.genome.clone()),
                })
            })
            .chain(waiting)
            .collect()
    }

    /// Tries again to put back kept creatures that could not come back yet.
    pub(crate) fn place_unplaced(&mut self) {
        if self.creatures.unplaced.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.creatures.unplaced);
        // Only those whose species is back and whose place is loaded are tried; the rest
        // wait as they are.
        let (ready, still): (Vec<_>, Vec<_>) = waiting.into_iter().partition(|s| {
            self.content.creatures.id(&s.species).is_some() && self.world.contains(ColumnPos::of_block(Vec3::from_array(s.position).floor().as_ivec3()))
        });
        self.creatures.unplaced = still;
        self.restore_creatures(ready);
    }

    /// Puts kept creatures from a save back in the world.
    pub(crate) fn restore_creatures(&mut self, saved: Vec<SavedCreature>) {
        for s in saved {
            // One that cannot come back now waits, in the save, for its species (a mod
            // turned off) or for room (the world full): a player's animal is never lost.
            let Some(species) = self.content.creatures.id(&s.species) else {
                tracing::warn!("a kept {} of {} waits: its species is not in the content", s.species, s.owner);
                self.creatures.unplaced.push(s);
                continue;
            };
            let Some(id) = self.spawn_creature(species, Vec3::from_array(s.position), s.seed) else {
                self.creatures.unplaced.push(s);
                continue;
            };
            let def = &self.content.creatures.get(species).def;
            let tick = self.tick;
            let creature = self.creatures.live.get_mut(&id).expect("just made");
            creature.seed = s.seed;
            creature.home = Vec3::from_array(s.home);
            creature.body.yaw = s.yaw;
            creature.health = s.health.min(def.health).max(1);
            creature.owner = Some(s.owner);
            creature.adult_scale = s.scale;
            creature.grows_up = s.grows_in.map(|t| tick + t);
            creature.scale = if creature.grows_up.is_some() { s.scale * YOUNG_SCALE } else { s.scale };
            creature.body.size = (def.body.size.0 * creature.scale, def.body.size.1 * creature.scale);
            for (due, left) in creature.products_due.iter_mut().zip(s.products_in) {
                *due = tick + left;
            }
            if let Some(genome) = s.genome {
                creature.traits = genes::express(&def.genes, &genome);
                creature.genome = genome;
            }
        }
    }
}

/// A product's count, more or less by the animal's yield: a woolly sheep gives more wool,
/// the fraction left over given by chance (`roll`, 0..1).
fn yielded(count: u8, yield_: f32, roll: f32) -> u8 {
    let exact = count as f32 * yield_.max(0.0);
    (exact.floor() + if roll.fract() < exact.fract() { 1.0 } else { 0.0 }).clamp(1.0, 64.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Server, ServerConfig, load_content, open_world};
    use voxmar_engine::glam::IVec3;

    fn server() -> Server {
        let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
        let mut config = ServerConfig::default();
        let (save, meta) = open_world(&mut config).expect("no save to open");
        let (_, incoming) = crossbeam_channel::unbounded();
        let (_, console) = crossbeam_channel::unbounded();
        Server::new(config, meta, save, content, incoming, console)
    }

    fn rabbit(at: Vec3) -> SavedCreature {
        let at = at.to_array();
        SavedCreature {
            species: "base:rabbit".into(),
            position: at,
            home: at,
            yaw: 0.0,
            health: 3,
            owner: "test".into(),
            seed: 7,
            scale: 1.0,
            grows_in: None,
            products_in: Vec::new(),
            genome: None,
        }
    }

    #[test]
    fn a_waiting_animal_comes_back_once_its_place_is_loaded() {
        let mut server = server();
        // One waits where the world is loaded (spawn), one far off where it is not.
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        server.creatures.unplaced.push(rabbit(server.spawn));
        server.creatures.unplaced.push(rabbit(far));
        // No player keeps columns loaded here, so step to the tick before the next try and
        // load the spawn column by hand, as a player standing there would.
        let every = 10 * TICKS_PER_SECOND as u64;
        server.tick = server.tick.next_multiple_of(every) + every - 1;
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at, at);
        server.tick();
        let kept: Vec<_> = server.creatures.live.values().filter(|c| c.owner.as_deref() == Some("test")).collect();
        assert_eq!(kept.len(), 1, "the one whose place is loaded is back");
        assert_eq!(server.content.creatures.get(kept[0].species).def.name, "base:rabbit");
        assert_eq!(server.creatures.unplaced.len(), 1, "the far one still waits");
        assert_eq!(server.creatures.unplaced[0].position, far.to_array());
        // The save holds each exactly once: the one back with its genes, the far one as it was.
        let saved = server.saved_creatures();
        assert_eq!(saved.len(), 2, "{saved:?}");
        assert_eq!(saved.iter().filter(|s| s.genome.is_some()).count(), 1);
        assert_eq!(saved.iter().filter(|s| s.position == far.to_array()).count(), 1);
    }

    #[test]
    fn wild_creatures_leave_room_for_animals_waiting_in_the_save() {
        let mut server = server();
        let most = server.content.creatures.limits().most;
        // Waiting animals far off fill all but two places.
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        let waiting = most - server.creatures.live.len() - 2;
        server.creatures.unplaced = (0..waiting).map(|_| rabbit(far)).collect();
        assert_eq!(server.wild_room(), 2);
        let wild = |server: &Server| server.creatures.live.values().filter(|c| c.owner.is_none()).count();
        server.console("summon rabbit 4");
        assert_eq!(wild(&server), 2, "wild ones took the waiting animals' room");
        assert_eq!(server.wild_room(), 0);
        server.console("summon rabbit 1");
        assert_eq!(wild(&server), 2, "a full world summoned more");
        // The room was kept: a waiting one whose place loads still comes back into it.
        server.creatures.unplaced[0] = rabbit(server.spawn);
        let every = 10 * TICKS_PER_SECOND as u64;
        server.tick = server.tick.next_multiple_of(every) + every - 1;
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at, at);
        server.tick();
        assert_eq!(server.creatures.live.values().filter(|c| c.owner.is_some()).count(), 1);
        assert_eq!(server.creatures.unplaced.len(), waiting - 1);
    }

    /// A kept rabbit of "test" at spawn, in loaded land.
    fn kept_rabbit(server: &mut Server, salt: u64) -> u32 {
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at, at);
        let species = server.content.creatures.id("base:rabbit").expect("rabbits");
        let id = server.spawn_creature(species, server.spawn, salt).expect("room");
        server.creatures.live.get_mut(&id).unwrap().owner = Some("test".into());
        id
    }

    #[test]
    fn kept_animals_leave_the_wild_budget_alone() {
        let mut server = server();
        let limits = server.content.creatures.limits();
        assert_eq!(server.wild_room(), limits.wild_most);
        // A rancher's herd, however big, takes nothing from the wild ones.
        for salt in 0..40 {
            kept_rabbit(&mut server, salt);
        }
        assert_eq!(server.wild_room(), limits.wild_most);
        assert_eq!(server.kept_by("test"), 40);
        // Waiting animals count toward their keeper too.
        server.creatures.unplaced.push(rabbit(server.spawn));
        assert_eq!(server.kept_by("test"), 41);
        // Wild ones take from it.
        server.console("summon rabbit 3");
        assert_eq!(server.wild_room(), limits.wild_most - 3);
    }

    #[test]
    fn a_birth_needs_room_and_costs_only_when_young_come() {
        let mut server = server();
        // The keeper is here, to be paid for births.
        let (player, _client) = keeper(&mut server);
        let farming = server.content.skills.id("core:farming").expect("Farming");
        let xp = |server: &Server| server.players[&player].skills.xp(farming);
        let (a, b) = (kept_rabbit(&mut server, 1), kept_rabbit(&mut server, 2));
        let kept_each = server.content.creatures.limits().kept_each;
        // The keeper is at their cap, counting animals waiting in the save.
        server.creatures.unplaced = (0..kept_each - 2).map(|_| rabbit(server.spawn + Vec3::new(4000.0, 0.0, 0.0))).collect();
        for id in [a, b] {
            server.creatures.live.get_mut(&id).unwrap().ready_until = server.tick + 100;
        }
        let before = xp(&server);
        server.breed(a, b);
        assert_eq!(server.creatures.live.len(), 2, "young were born past the cap");
        assert_eq!(xp(&server), before, "a refused birth paid Farming xp");
        for id in [a, b] {
            let parent = &server.creatures.live[&id];
            assert_eq!((parent.ready_until, parent.breeds_again), (0, 0), "no birth cost the parents their rest");
        }
        // Room for one: one is born, whatever the litter, and the parents rest.
        server.creatures.unplaced.pop();
        server.breed(a, b);
        assert_eq!(server.creatures.live.len(), 3);
        assert!(server.creatures.live[&a].breeds_again > server.tick);
        assert!(xp(&server) > before, "a birth paid no Farming xp");
        // The world full counts too, kept cap or not.
        server.creatures.unplaced.clear();
        let room = server.room_left();
        server.creatures.unplaced = (0..room).map(|_| SavedCreature { owner: "someone".into(), ..rabbit(server.spawn) }).collect();
        for id in [a, b] {
            server.creatures.live.get_mut(&id).unwrap().breeds_again = 0;
        }
        let before = xp(&server);
        server.breed(a, b);
        assert_eq!(server.creatures.live.len(), 3, "a birth took the room kept for waiting animals");
        assert_eq!(xp(&server), before, "a birth refused in a full world paid Farming xp");
    }

    /// "test", joined and an admin, standing at spawn with the land around loaded, and their
    /// end of the connection.
    fn keeper(server: &mut Server) -> (PlayerId, voxmar_protocol::ClientConnection) {
        let (client, connection) = voxmar_protocol::local_pair();
        let id = 1;
        let mut player = crate::player::Player::new(id, connection, server.spawn, 2);
        (player.joined, player.admin, player.name) = (true, true, "test".into());
        server.players.insert(id, player);
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at - IVec3::new(64, 0, 64), at + IVec3::new(64, 0, 64));
        (id, client)
    }

    /// The keys of what a player has been told since last asked; for a command refused,
    /// the key of why.
    fn told(client: &voxmar_protocol::ClientConnection) -> Vec<String> {
        let mut keys = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Chat { text: Words::Key(key, args), .. } = message {
                let why = args.into_iter().find_map(|(name, why)| match why {
                    Words::Key(why, _) if key == "command.error.usage" && name == "why" => Some(why),
                    _ => None,
                });
                keys.push(why.unwrap_or(key));
            }
        }
        keys
    }

    #[test]
    fn taming_and_breeding_food_wait_for_room_under_the_keepers_cap() {
        let mut server = server();
        let (player, client) = keeper(&mut server);
        let species = server.content.creatures.id("base:rabbit").expect("rabbits");
        let food = server.content.creatures.get(species).tame_food[0];
        assert!(server.content.creatures.get(species).breed_food.contains(&food));
        server.players.get_mut(&player).unwrap().inventory.add(food, 10);
        assert_eq!(server.players[&player].held(), Some(food));
        let in_hand = |server: &Server| server.players[&player].inventory.get(0).map_or(0, |stack| stack.count);
        // A wild rabbit, and the keeper at their cap: one kept here, the rest waiting.
        let wild = server.spawn_creature(species, server.spawn, 1).expect("room");
        let mine = kept_rabbit(&mut server, 2);
        let kept_each = server.content.creatures.limits().kept_each;
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        server.creatures.unplaced = (0..kept_each - 1).map(|_| rabbit(far)).collect();
        told(&client);
        server.tend(player, wild);
        server.tend(player, mine);
        assert_eq!(in_hand(&server), 10, "a keeper at their cap had food taken");
        assert_eq!((server.creatures.live[&wild].trust, server.creatures.live[&mine].ready_until), (0.0, 0));
        assert_eq!(told(&client), ["husbandry.kept_full", "husbandry.no_room_to_breed"]);
        // An admin's taming keeps to the cap too.
        server.command(player, "creatures tame");
        assert!(server.creatures.live[&wild].owner.is_none());
        assert_eq!(told(&client), ["husbandry.kept_full"]);
        // Room for one: the kept one takes its food and is readied, the wild one takes its
        // and comes to trust the keeper.
        server.creatures.unplaced.pop();
        server.tend(player, mine);
        server.tend(player, wild);
        assert_eq!(in_hand(&server), 8);
        assert!(server.creatures.live[&mine].ready_until > server.tick);
        let wild_now = &server.creatures.live[&wild];
        assert!(wild_now.trust > 0.0 || wild_now.owner.is_some());
        assert!(told(&client).iter().all(|key| !key.starts_with("husbandry.")), "refused with room");
        // Another wild one: an admin tames as many as there is room for, and no more.
        server.creatures.live.get_mut(&wild).unwrap().owner = None;
        let other = server.spawn_creature(species, server.spawn, 3).expect("room");
        server.command(player, "creatures tame");
        let tamed = [wild, other].iter().filter(|id| server.creatures.live[id].owner.is_some()).count();
        assert_eq!(tamed, 1);
        assert_eq!(server.kept_by("test"), kept_each);
    }

    #[test]
    fn natural_spawning_brings_a_herd_no_bigger_than_the_room_left() {
        let mut server = server();
        let (player, _client) = keeper(&mut server);
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        let full: Vec<_> = (0..server.room_left() - 1).map(|_| rabbit(far)).collect();
        // Spots that bring two or more with room to spare bring one when there is room for
        // only one (kept animals waiting in the save fill the rest).
        let mut trimmed = 0;
        for salt in 0..4000 {
            server.creatures.unplaced.clear();
            server.spawn_near(player, salt);
            let free = server.creatures.live.len();
            server.creatures.live.clear();
            if free < 2 {
                continue;
            }
            server.creatures.unplaced = full.clone();
            assert_eq!(server.wild_room(), 1);
            server.spawn_near(player, salt);
            assert_eq!(server.creatures.live.len(), 1, "{} came for one place, {free} with room", server.creatures.live.len());
            server.creatures.live.clear();
            trimmed += 1;
            if trimmed == 8 {
                break;
            }
        }
        assert_eq!(trimmed, 8, "too few groups came to judge");
    }
}
