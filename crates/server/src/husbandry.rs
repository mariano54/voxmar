//! Taming, keeping and breeding on the server (`engine::creature::husbandry`,
//! docs/breeding.md): players tend creatures with what they hold (`base:tend`), kept
//! creatures give their products on timers and breed when two ready adults meet, and kept
//! creatures are saved with the world.

use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::husbandry::{self, IN_LOVE_SECONDS, MATE_RANGE, YOUNG_SCALE};
use voxmar_engine::creature::brain::Mind;
use voxmar_engine::creature::condition::{self, Met};
use voxmar_engine::creature::CreatureId;
use voxmar_engine::crop::level_bonus;
use voxmar_engine::dropped;
use voxmar_engine::genes;
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::skill::SkillId;
use voxmar_engine::traits;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::save::Draw;
use crate::commands::Source;
use crate::Server;
use crate::condition::RECKON_EVERY;
use crate::creatures::product_ticks;

/// How close two ready adults come before they breed, in blocks.
const MEET: f32 = 1.6;
/// How soon a second ask to let an animal go must follow the first, in ticks.
const RELEASE_CONFIRM: u64 = 5 * TICKS_PER_SECOND as u64;
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
    /// How well it was kept, and in-game days since it last ate; none in saves from before
    /// condition, which restore as newly tamed and just fed.
    #[serde(default)]
    pub condition: Option<f32>,
    #[serde(default)]
    pub fed_days_ago: Option<f32>,
    /// Its own rolls drawn so far (`Live::rolls`); none in saves from before they were
    /// counted.
    #[serde(default)]
    pub rolls: u64,
}

impl Server {
    pub(crate) fn player_name(&self, player: PlayerId) -> Option<String> {
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

    /// Whether creature `id` takes `item` from `player`: food that tames it (if wild), its
    /// food when it is hungry or the food could ready it to breed (if theirs), or a tool for
    /// a product of its.
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
        // A kept one eats its food when hungry, and takes it otherwise only if it could
        // ready it (whether it is kept well enough, and has room for young, `tend` says).
        let its_food = feed || species.tame_food.contains(&item) || species.breed_food.contains(&item);
        let eats = mine && its_food && (self.hungry(id) || self.readies(player, id, item));
        let tool = mine && species.def.products.iter().any(|p| p.tool.as_ref().is_some_and(|t| tags.contains(t)));
        tames || eats || tool
    }

    /// Whether kept creature `id` has gone a day without eating.
    fn hungry(&self, id: u32) -> bool {
        let rules = self.content.creatures.condition();
        self.creatures.live.get(&id).is_some_and(|c| rules.fed(self.days_since(c.fed)) != Met::Yes)
    }

    /// Whether `item` could ready creature `id` to breed for `player`, its condition and room
    /// for young aside: its breeding food, a grown one of a kind that breeds, the player's
    /// Farming level enough, and rested since it last bred.
    fn readies(&self, player: PlayerId, id: u32, item: BlockId) -> bool {
        let Some(creature) = self.creatures.live.get(&id) else { return false };
        let species = self.content.creatures.get(creature.species);
        let feed = self.content.blocks.def(item).tags.iter().any(|t| t == "breeding_feed");
        let level = species.def.breeding.as_ref().is_some_and(|b| self.skill_level(player, "core:farming") >= b.level);
        level && creature.grows_up.is_none() && self.tick >= creature.breeds_again && (feed || species.breed_food.contains(&item))
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
            // By the player's count of products taken, never by the tick.
            // One draw for how many, the other for the yield gene's fraction.
            let Some([roll, fraction]) = self.players.get_mut(&player).map(|p| p.draw(crate::save::Draw::Yield)) else { return };
            let creature = &self.creatures.live[&id];
            let count = yielded(low + ((roll * (high - low + 1) as f32) as u8).min(high - low), creature.traits.yield_, fraction);
            // Condition sets how soon the next is due (0.5 to 1.2 as often), its yield gene
            // how many come.
            let (seed, often) = (creature.seed, self.content.creatures.condition().output(creature.condition).max(0.05));
            let creature = self.creatures.live.get_mut(&id).expect("checked");
            creature.products_due[index] = self.tick + product_ticks(every / often, seed, index);
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
            self.refuse(player, Words::key("husbandry.kept_full").with("count", Words::text(kept_each)));
            return;
        }
        // A kept one eats a meal when hungry, and its breeding food readies it if it is kept
        // well enough and has room for young; what it cannot use stays in hand, with why.
        if let Some(keeper) = creature.owner.clone() {
            let rules = *self.content.creatures.condition();
            let (hungry, readies) = (self.hungry(id), self.readies(player, id, item));
            let well = creature.condition > rules.breed;
            let room = self.birth_room(&keeper) > 0;
            let ready = readies && well && room;
            if hungry || ready {
                if let Some(p) = self.players.get_mut(&player) {
                    let selected = p.selected;
                    if p.inventory.take_one(selected).is_none() {
                        return;
                    }
                    p.changed(traits::INVENTORY);
                }
                let (day, tick) = (self.day, self.tick);
                let creature = self.creatures.live.get_mut(&id).expect("checked");
                creature.fed = day;
                creature.needs.food = Met::Yes;
                if ready {
                    creature.ready_until = tick + (IN_LOVE_SECONDS * TICKS_PER_SECOND as f32) as u64;
                    creature.changed = true;
                }
            }
            // Why the food did not ready it, if it would have: said after any meal, so a fed
            // one is not told it wants food.
            let why = match (readies, well, room) {
                (true, false, _) => Some(self.too_poorly_kept(id)),
                (true, true, false) => Some(self.no_room_for_young(&keeper)),
                _ => None,
            };
            if let Some(why) = why {
                self.refuse(player, why);
            }
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
        let (day, tamed) = (self.day, self.content.creatures.condition().tamed);
        self.creatures.live.get_mut(&id).expect("checked").fed = day;
        let Some(taming) = species.def.taming.clone() else { return };
        let kind = species.def.name.clone();
        // By the count of feeds the player has given, not the tick it is eaten on.
        let Some([u, _]) = self.players.get_mut(&player).map(|p| p.draw(Draw::Feed)) else { return };
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
            let was_kept = creature.was_kept;
            // One let go keeps the condition it had: taking it in again does not mend it.
            creature.condition = if was_kept { creature.condition.min(tamed) } else { tamed };
            creature.reckoned = day;
            self.settle(id);
            // One let go and taken in again pays nothing more (design draft 99).
            if was_kept {
                return;
            }
            // For quests that ask for tamed animals.
            let hunter = self.content.skills.id("core:hunter");
            self.grant(player, hunter, taming.xp);
            self.tally(player, "tamed", &kind, 1);
        }
    }

    /// Why kept creature `id` won't breed yet: its condition in a word, and what it wants.
    fn too_poorly_kept(&self, id: u32) -> Words {
        let creature = &self.creatures.live[&id];
        let what = crate::commands::creature_name(&self.content.creatures.get(creature.species).def);
        let state = Words::key(&format!("condition.{}", condition::word(creature.condition, self.content.creatures.condition().breed)));
        match self.wants(id) {
            Some(wants) => Words::key("husbandry.too_poorly_kept_wants").with("creature", what).with("state", state).with("needs", wants),
            None => Words::key("husbandry.too_poorly_kept").with("creature", what).with("state", state),
        }
    }

    /// What kept creature `id` wants, as its needs were last looked at, if anything.
    pub(crate) fn wants(&self, id: u32) -> Option<Words> {
        let creature = self.creatures.live.get(&id)?;
        let wants: Vec<Words> = creature.needs.all().into_iter().filter(|(_, met)| *met != Met::Yes).map(|(need, _)| Words::key(&format!("need.{need}"))).collect();
        (!wants.is_empty()).then_some(Words::List(wants))
    }

    /// Whether `player` keeps creature `id`.
    pub(crate) fn keeps(&self, player: PlayerId, id: u32) -> bool {
        let owner = self.creatures.live.get(&id).and_then(|c| c.owner.as_deref());
        owner.is_some_and(|owner| self.players.get(&player).is_some_and(|p| p.name == owner))
    }

    /// `base:release`: a keeper lets one of their animals go wild again (design draft 99).
    /// The first ask only says how to confirm; the same ask within `RELEASE_CONFIRM` lets
    /// it go. It keeps its genes, breed and condition and loses its keeper, trust and
    /// readiness: from then on it is wild like any other (not saved, gone when nobody is
    /// near, no products or breeding), only marked as once kept, so taming it again pays
    /// no Hunter xp and counts for no quest. Release gives nothing.
    pub(crate) fn release(&mut self, player: PlayerId, id: u32) {
        if !self.keeps(player, id) {
            return;
        }
        let species = self.creatures.live[&id].species;
        let what = crate::commands::creature_name(&self.content.creatures.get(species).def);
        let tick = self.tick;
        let confirmed = self.creatures.release_asked.get(&player).is_some_and(|&(asked, at)| asked == id && tick.saturating_sub(at) <= RELEASE_CONFIRM);
        if !confirmed {
            self.creatures.release_asked.insert(player, (id, tick));
            self.tell(Source::Player(player), ChatKind::Info, Words::key("husbandry.release_ask").with("creature", what));
            return;
        }
        self.creatures.release_asked.remove(&player);
        let def = &self.content.creatures.get(species).def;
        let creature = self.creatures.live.get_mut(&id).expect("kept");
        // A wild mind again: its own temper, wary of people as any wild one is.
        let mut mind = Mind::new(&def.temperament, creature.seed);
        mind.personality.aggression = (mind.personality.aggression + creature.traits.temper).clamp(0.0, 1.0);
        creature.mind = mind;
        (creature.owner, creature.owner_id, creature.trusting, creature.trust) = (None, None, None, 0.0);
        (creature.ready_until, creature.needs) = (0, Default::default());
        creature.home = creature.body.position;
        creature.was_kept = true;
        creature.changed = true;
        // Wild from here: the traps' next look follows it from where it was let go.
        let at = creature.body.position;
        self.catches.arrived(id, at);
        self.tell(Source::Player(player), ChatKind::Info, Words::key("husbandry.released").with("creature", what));
    }

    /// Where a ready creature's mate is: the nearest other of its kind, ready too, within
    /// range.
    pub(crate) fn mate_for(&self, id: u32, neighbours: &crate::creatures::Neighbours) -> Option<Vec3> {
        let creature = self.creatures.live.get(&id)?;
        if creature.ready_until == 0 {
            return None;
        }
        let at = creature.body.position;
        neighbours
            .kind_around(at, creature.species, id)
            .filter_map(|other| self.creatures.live.get(&other))
            .filter(|c| c.species == creature.species && c.owner.is_some() && c.ready_until > 0 && c.body.position.distance(at) < MATE_RANGE)
            .map(|c| c.body.position)
            .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
    }

    /// Products laid on the ground, young growing up, readiness running out, and ready
    /// pairs that have met breeding.
    pub(crate) fn tick_husbandry(&mut self) {
        if !self.tick.is_multiple_of(EVERY) {
            return;
        }
        let tick = self.tick;
        // A share of the kept animals each pass, each one every `RECKON_EVERY` ticks.
        let shares = RECKON_EVERY / EVERY;
        self.reckon_share(shares, tick / EVERY % shares);
        // Owners' ids, for the players here now (names that differ only in case are one
        // player's).
        let ids: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| c.owner.is_some()).map(|(&id, _)| id).collect();
        for &id in &ids {
            let owner = self.creatures.live[&id].owner.as_deref().map(crate::player_key);
            let present = self.players.values().find(|p| p.joined && Some(crate::player_key(&p.name)) == owner).map(|p| p.id);
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
            // In land that is not loaded its products wait, as its condition does: what is due
            // moves on with the clock, and nothing piles up where nobody can see it.
            if !self.world.contains(ColumnPos::of_block(creature.body.position.floor().as_ivec3())) {
                creature.products_due.iter_mut().for_each(|due| *due += EVERY);
                continue;
            }
            let species = self.content.creatures.get(creature.species);
            // Condition sets how soon the next is due (0.5 to 1.2 as often), its yield gene
            // how many come.
            let often = self.content.creatures.condition().output(creature.condition).max(0.05);
            for (index, product) in species.def.products.iter().enumerate() {
                if product.tool.is_none() && tick >= creature.products_due[index] {
                    // By its own count of rolls, never the tick it is laid on.
                    let roll = crate::creatures::own_roll(creature.seed, creature.rolls, crate::creatures::PRODUCT);
                    creature.rolls += 1;
                    creature.products_due[index] = tick + product_ticks(product.every / often, roll, index);
                    let keeper = creature.owner_id.filter(|_| product.nests);
                    laid.push((creature.body.position, species.products[index], product.count, creature.traits.yield_, id, keeper));
                }
            }
        }
        for (position, item, (low, high), yield_, id, keeper) in laid {
            let cell = position.floor().as_ivec3();
            // Numbered by the world's count of rolls, never by the tick it is laid on.
            let [a, c] = self.meta.next_roll(crate::save::ROLL_PRODUCT);
            let count = yielded(low + ((a * (high - low + 1) as f32) as u8).min(high - low), yield_, c);
            // Laid in its keeper's home, it goes into the stores there that take it, nearest
            // first; what finds no room falls as ever.
            let mut left = Some(ItemStack { item, count });
            if let Some(keeper) = keeper
                && let Some(anchor) = self.home_holding(keeper, position)
            {
                for store in self.stores_around(anchor) {
                    let Some(stack) = left else { break };
                    left = self.factory.put(store, stack, &self.content.machines);
                }
                let nested = count as u32 - left.map_or(0, |s| s.count as u32);
                if nested > 0
                    && let Some(player) = self.players.get_mut(&keeper)
                {
                    let name = self.content.blocks.def(item).name.clone();
                    match player.nested.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, n)) => *n += nested,
                        None => player.nested.push((name, nested)),
                    }
                }
            }
            if let Some(stack) = left {
                self.items.spawn(stack, dropped::popped(cell, self.meta.next_pop(cell, id)), dropped::PICKUP_DELAY);
            }
        }
        // Ready pairs that have met.
        let ready: Vec<(u32, CreatureId, Vec3)> =
            self.creatures.live.iter().filter(|(_, c)| c.owner.is_some() && c.ready_until > 0).map(|(&id, c)| (id, c.species, c.body.position)).collect();
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
                self.refuse(player, self.no_room_for_young(keeper));
            }
            return;
        }
        let (low, high) = breeding.litter;
        // Their young live by the climate of the place they are born.
        let fit = self.climate_fit(species, at);
        // Parents in better condition have bigger litters, never more than there is room for.
        let rules = *self.content.creatures.condition();
        let fitness = (self.creatures.live[&a].condition + self.creatures.live[&b].condition) / 2.0;
        let odds = (self.meta.next_roll(crate::save::ROLL_LITTER)[0] * rules.output(fitness)).min(0.9999);
        let count = (low + ((odds * (high - low + 1) as f32) as u8).min(high - low)).max(1).min(room.min(u8::MAX as usize) as u8);
        let grows = self.tick + (breeding.growth * 60.0 * TICKS_PER_SECOND as f32) as u64;
        let (mother, father) = (self.creatures.live[&a].genome.clone(), self.creatures.live[&b].genome.clone());
        // Mutations are likelier where aether is high (docs/breeding.md, "Mutation").
        let aether = self.overworld.site(at.x.floor() as i32, at.z.floor() as i32).aether;
        let mutation = genes::MUTATION * (1.0 + 4.0 * aether.clamp(0.0, 1.0));
        let mut born = 0;
        for i in 0..count {
            // Each young's genes, looks and mind by the world's count of rolls, never by the
            // tick it is born on.
            let [g, h] = self.meta.next_cast(crate::save::ROLL_YOUNG);
            let salt = seed ^ (u64::from(g.to_bits()) << 32 | u64::from(h.to_bits()));
            let Some(id) = self.spawn_creature(species, at + Vec3::new(0.3 * i as f32, 0.0, 0.3), salt) else { continue };
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
            young.condition = fitness;
            young.climate_fit = fit;
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
                    condition: Some(c.condition),
                    fed_days_ago: Some(self.days_since(c.fed)),
                    rolls: c.rolls,
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
            let fit = self.climate_fit(species, Vec3::from_array(s.home));
            let def = &self.content.creatures.get(species).def;
            let (tick, day) = (self.tick, self.day);
            let creature = self.creatures.live.get_mut(&id).expect("just made");
            creature.seed = s.seed;
            creature.rolls = s.rolls;
            creature.home = Vec3::from_array(s.home);
            creature.climate_fit = fit;
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
            if let Some(condition) = s.condition {
                creature.condition = condition.clamp(0.0, 1.0);
            }
            creature.fed = day - s.fed_days_ago.unwrap_or(0.0).max(0.0) as f64 * TICKS_PER_DAY as f64;
            creature.reckoned = day;
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
    use crate::{Server, test_server};
    use voxmar_engine::creature::condition::Met;
    use voxmar_engine::glam::IVec3;

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
            condition: None,
            fed_days_ago: None,
            rolls: 0,
        }
    }

    #[test]
    fn a_waiting_animal_comes_back_once_its_place_is_loaded() {
        let mut server = test_server();
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
        let mut server = test_server();
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
        let mut server = test_server();
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
    fn a_settlements_people_leave_the_wild_budget_alone() {
        let mut server = test_server();
        let wild = server.wild_room();
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at, at);
        let villager = server.content.creatures.id("base:villager").expect("villagers");
        for salt in 0..5 {
            server.spawn_creature(villager, server.spawn, salt).expect("room");
        }
        assert_eq!(server.wild_room(), wild, "people took the wild animals' places");
        let kept = server.kept_by("test");
        server.console("summon villager 1");
        server.console("creatures tame");
        assert_eq!(server.kept_by("test"), kept, "a villager was taken in as a kept animal");
    }

    #[test]
    fn a_birth_needs_room_and_costs_only_when_young_come() {
        let mut server = test_server();
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
        let mut player = crate::player::Player::new(id, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.admin, player.name) = (true, true, "test".into());
        server.players.insert(id, player);
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at - IVec3::new(64, 0, 64), at + IVec3::new(64, 0, 64));
        (id, client)
    }

    /// The keys of what a player has been told since last asked; for a command refused,
    /// the key of why.
    /// The keys of what the player was told, in the console or as a refusal (with "refused:"
    /// before it).
    fn told(client: &voxmar_protocol::ClientConnection) -> Vec<String> {
        let mut keys = Vec::new();
        while let Ok(Some(message)) = client.try_recv() {
            if let voxmar_protocol::ServerMessage::Refused { why: Words::Key(key, _) } = &message {
                keys.push(format!("refused: {key}"));
            }
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

    /// How much a kept animal yields is drawn by the keeper's count of products taken, not
    /// the tick it is taken on, and each counts on.
    #[test]
    fn a_yield_draws_by_the_keepers_count_not_the_tick() {
        let taken = |tick: u64| {
            let mut server = test_server();
            let (player, _client) = keeper(&mut server);
            server.players.get_mut(&player).unwrap().draw_key = crate::player::player_draw_key(0x0d1a, "test");
            let (species, index) = server.content.creatures.iter().find_map(|(id, c)| c.def.products.iter().position(|p| p.tool.as_deref() == Some("shears")).map(|i| (id, i))).expect("one sheared");
            let shears = server.content.blocks.expect("base:shears");
            server.players.get_mut(&player).unwrap().inventory.add(shears, 1);
            let id = server.spawn_creature(species, server.spawn, 2).expect("room");
            let kept = server.creatures.live.get_mut(&id).unwrap();
            (kept.owner, kept.owner_id) = (Some("test".into()), Some(player));
            let product = server.content.creatures.get(species).products[index];
            server.tick = tick;
            (0..12)
                .map(|_| {
                    server.creatures.live.get_mut(&id).unwrap().products_due[index] = 0;
                    let before = server.players[&player].inventory.count(product);
                    server.tend(player, id);
                    server.players[&player].inventory.count(product) - before
                })
                .collect::<Vec<_>>()
                .into_iter()
                .chain([server.players[&player].draws.yields as u32])
                .collect::<Vec<_>>()
        };
        let first = taken(1_000);
        assert_eq!(first.last(), Some(&12), "each product taken counted on");
        assert!(first[..12].iter().all(|&n| n > 0), "a product taken gave nothing: {first:?}");
        assert!(first[..12].windows(2).any(|w| w[0] != w[1]), "twelve products taken all yielded alike: {first:?}");
        assert_eq!(taken(1_377), first, "the same products taken later yielded otherwise");
    }

    /// How many a product yields and how its yield gene's share rounds are two draws, not
    /// one: with one for both, a low count never rounded the same way a high one did.
    #[test]
    fn a_yields_rounding_is_its_own_draw() {
        let mut server = test_server();
        let (player, _client) = keeper(&mut server);
        server.players.get_mut(&player).unwrap().draw_key = crate::player::player_draw_key(0x0d1a, "test");
        let (species, index) = server.content.creatures.iter().find_map(|(id, c)| c.def.products.iter().position(|p| p.tool.as_deref() == Some("shears")).map(|i| (id, i))).expect("one sheared");
        let shears = server.content.blocks.expect("base:shears");
        server.players.get_mut(&player).unwrap().inventory.add(shears, 1);
        let id = server.spawn_creature(species, server.spawn, 2).expect("room");
        let kept = server.creatures.live.get_mut(&id).unwrap();
        (kept.owner, kept.owner_id) = (Some("test".into()), Some(player));
        // A share past a whole one, so every count rounds one way or the other.
        kept.traits.yield_ = 1.3;
        let (product, (low, high)) = (server.content.creatures.get(species).products[index], server.content.creatures.get(species).def.products[index].count);
        let mut apart = 0;
        for _ in 0..24 {
            let p = &server.players[&player];
            let [roll, fraction] = crate::save::numbered_draws(p.draw_key, crate::save::Draw::Yield.salt(), p.draws.yields);
            let count = low + ((roll * (high - low + 1) as f32) as u8).min(high - low);
            server.creatures.live.get_mut(&id).unwrap().products_due[index] = 0;
            let before = server.players[&player].inventory.count(product);
            server.tend(player, id);
            let got = server.players[&player].inventory.count(product) - before;
            assert_eq!(got, u32::from(yielded(count, 1.3, fraction)), "{count} at a share of 1.3, rounded by the second draw {fraction:.2}");
            apart += usize::from(yielded(count, 1.3, fraction) != yielded(count, 1.3, roll));
        }
        assert!(apart > 0, "in 24 takes the second draw never rounded otherwise than the first would have");
    }

    #[test]
    fn taming_and_breeding_food_wait_for_room_under_the_keepers_cap() {
        let mut server = test_server();
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
        // Refusals, for the interface to show as such, not console lines.
        assert_eq!(told(&client), ["refused: husbandry.kept_full", "refused: husbandry.no_room_to_breed"]);
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
        assert!(told(&client).iter().all(|key| !key.contains("husbandry.")), "refused with room");
        // Another wild one: an admin tames as many as there is room for, and no more.
        server.creatures.live.get_mut(&wild).unwrap().owner = None;
        let other = server.spawn_creature(species, server.spawn, 3).expect("room");
        server.command(player, "creatures tame");
        let tamed = [wild, other].iter().filter(|id| server.creatures.live[id].owner.is_some()).count();
        assert_eq!(tamed, 1);
        assert_eq!(server.kept_by("test"), kept_each);
    }

    #[test]
    fn a_treadwheel_animal_eats_at_the_keepers_cap_but_is_not_readied() {
        let mut server = test_server();
        let (player, client) = keeper(&mut server);
        server.command(player, "skill farming 15");
        let species = server.content.creatures.id("base:aurochs").expect("aurochs");
        assert!(server.content.machines.walks("base:aurochs"));
        let food = server.content.creatures.get(species).breed_food[0];
        server.players.get_mut(&player).unwrap().inventory.add(food, 10);
        assert_eq!(server.players[&player].held(), Some(food));
        let in_hand = |server: &Server| server.players[&player].inventory.get(0).map_or(0, |stack| stack.count);
        // A kept aurochs, well kept but long unfed (hungry), and its keeper at their cap: one
        // kept here, the rest waiting. A hungry one eats whatever the room, so a treadwheel's
        // walker can always be kept fed.
        let aurochs = server.spawn_creature(species, server.spawn, 2).expect("room");
        let day = server.day;
        let kept = server.creatures.live.get_mut(&aurochs).unwrap();
        (kept.owner, kept.owner_id, kept.fed, kept.condition) = (Some("test".into()), Some(player), day - 1.0e6, 1.0);
        let kept_each = server.content.creatures.limits().kept_each;
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        server.creatures.unplaced = (0..kept_each - 1).map(|_| rabbit(far)).collect();
        told(&client);
        // It eats one, and its fed clock starts again, but no young could come of it.
        server.tend(player, aurochs);
        assert_eq!(in_hand(&server), 9, "a treadwheel animal at the keeper's cap was not fed");
        assert_eq!(server.creatures.live[&aurochs].fed, server.day);
        assert_eq!(server.creatures.live[&aurochs].ready_until, 0, "readied to breed with no room for young");
        assert_eq!(told(&client), ["refused: husbandry.no_room_to_breed"]);
    }

    #[test]
    fn natural_spawning_brings_a_herd_no_bigger_than_the_room_left() {
        let mut server = test_server();
        let (player, _client) = keeper(&mut server);
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        let full: Vec<_> = (0..server.room_left() - 1).map(|_| rabbit(far)).collect();
        // Spots that bring two or more with room to spare bring one when there is room for
        // only one (kept animals waiting in the save fill the rest).
        let mut trimmed = 0;
        for salt in 0..4000 {
            server.creatures.unplaced.clear();
            // The same spot again: the second try is drawn by the same count of rolls.
            let rolls = server.meta.rolls;
            server.spawn_near(player, salt);
            let free = server.creatures.live.len();
            server.creatures.live.clear();
            if free < 2 {
                continue;
            }
            server.creatures.unplaced = full.clone();
            assert_eq!(server.wild_room(), 1);
            server.meta.rolls = rolls;
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

    #[test]
    fn condition_walks_a_day_at_a_time_toward_what_its_needs_allow() {
        let mut server = test_server();
        let id = kept_rabbit(&mut server, 1);
        let rules = *server.content.creatures.condition();
        let day = voxmar_protocol::TICKS_PER_DAY as f64;
        // A day since it was last reckoned, three since it ate.
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.condition, creature.reckoned, creature.fed) = (0.6, server.day - day, server.day - 3.0 * day);
        let needs = server.needs_of(id);
        assert_eq!(needs.food, Met::No, "three days unfed is hungry");
        server.reckon_condition();
        let creature = &server.creatures.live[&id];
        assert_eq!(creature.needs, needs);
        let expected = rules.step(0.6, rules.target(&needs, false), 1.0);
        assert!((creature.condition - expected).abs() < 1e-4, "{} against {expected}", creature.condition);
        assert!(creature.condition < 0.6, "a hungry animal lost no condition");
        // Fed again, its food need is met.
        server.creatures.live.get_mut(&id).unwrap().fed = server.day;
        assert_eq!(server.needs_of(id).food, Met::Yes);
        // Below half health it is held to half however well kept.
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.condition, creature.health, creature.reckoned) = (0.9, 1, server.day - 10.0 * day);
        server.reckon_condition();
        assert!(server.creatures.live[&id].condition <= rules.hurt + 1e-6);
    }

    #[test]
    fn every_kept_animal_is_looked_at_once_a_reckoning_a_share_at_a_time() {
        let mut server = test_server();
        let ids: Vec<u32> = (0..30).map(|salt| kept_rabbit(&mut server, salt)).collect();
        let mut looked: std::collections::BTreeMap<u32, u32> = Default::default();
        let mut most_at_once = 0;
        server.tick = server.tick.next_multiple_of(RECKON_EVERY);
        for _ in 0..RECKON_EVERY {
            server.day += 1.0;
            let before: Vec<f64> = ids.iter().map(|id| server.creatures.live[id].reckoned).collect();
            server.tick_husbandry();
            let now: Vec<u32> = ids.iter().zip(before).filter(|(id, was)| server.creatures.live[*id].reckoned != *was).map(|(id, _)| *id).collect();
            most_at_once = most_at_once.max(now.len());
            now.into_iter().for_each(|id| *looked.entry(id).or_default() += 1);
            server.tick += 1;
        }
        assert!(ids.iter().all(|id| looked.get(id) == Some(&1)), "{looked:?}");
        // Thirty animals over twenty shares: never more than two in one pass.
        assert!(most_at_once <= 2, "{most_at_once} looked at in one pass");
    }

    /// A grown `species` kept by "test" at spawn, with its land loaded and its products due.
    fn kept(server: &mut Server, species: &str, salt: u64) -> u32 {
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at, at);
        let species = server.content.creatures.id(species).expect("species");
        let id = server.spawn_creature(species, server.spawn, salt).expect("room");
        let creature = server.creatures.live.get_mut(&id).unwrap();
        creature.owner = Some("test".into());
        creature.products_due.iter_mut().for_each(|due| *due = 0);
        id
    }

    /// Lets one of "test"'s animals go: asked, then asked again a second later.
    fn let_go(server: &mut Server, player: PlayerId, id: u32) {
        server.release(player, id);
        server.tick += TICKS_PER_SECOND as u64;
        server.release(player, id);
    }

    #[test]
    fn letting_an_animal_go_takes_two_asks_and_frees_its_place() {
        let mut server = test_server();
        let (player, client) = keeper(&mut server);
        let species = server.content.creatures.id("base:rabbit").expect("rabbits");
        let food = server.content.creatures.get(species).tame_food[0];
        server.players.get_mut(&player).unwrap().inventory.add(food, 10);
        // The keeper at their cap: two here, the rest waiting in the save; a wild one by them.
        let kept_each = server.content.creatures.limits().kept_each;
        let (mine, other) = (kept_rabbit(&mut server, 1), kept_rabbit(&mut server, 2));
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        server.creatures.unplaced = (0..kept_each - 2).map(|_| rabbit(far)).collect();
        let wild = server.spawn_creature(species, server.spawn, 3).expect("room");
        server.tend(player, wild);
        assert_eq!(server.creatures.live[&wild].trust, 0.0, "a full keeper took a wild one in");
        told(&client);
        // One ask only asks; a second too late asks again.
        server.release(player, mine);
        server.tick += 6 * TICKS_PER_SECOND as u64;
        server.release(player, mine);
        assert!(server.creatures.live[&mine].owner.is_some(), "a lone ask let it go");
        // Asking about another animal in between starts over.
        server.release(player, other);
        server.release(player, mine);
        assert!(server.creatures.live[&mine].owner.is_some() && server.creatures.live[&other].owner.is_some());
        assert_eq!(told(&client), ["husbandry.release_ask"; 4]);
        // Two asks within a few seconds let it go: wild, marked, and its place is free.
        server.tick += 6 * TICKS_PER_SECOND as u64;
        let_go(&mut server, player, mine);
        let freed = &server.creatures.live[&mine];
        assert!(freed.owner.is_none() && freed.owner_id.is_none() && freed.was_kept && freed.trust == 0.0);
        assert_eq!(told(&client), ["husbandry.release_ask", "husbandry.released"]);
        assert_eq!(server.kept_by("test"), kept_each - 1);
        server.tend(player, wild);
        assert!(server.creatures.live[&wild].trust > 0.0 || server.creatures.live[&wild].owner.is_some(), "the freed place took no wild one");
        // Nobody lets go of another keeper's animal, nor of a wild one.
        server.creatures.live.get_mut(&other).unwrap().owner = Some("someone".into());
        let stranger = server.spawn_creature(species, server.spawn, 4).expect("room");
        let_go(&mut server, player, other);
        let_go(&mut server, player, stranger);
        assert_eq!(server.creatures.live[&other].owner.as_deref(), Some("someone"));
        assert!(told(&client).iter().all(|key| !key.starts_with("husbandry.release")));
    }

    #[test]
    fn taming_a_let_go_animal_again_pays_no_xp_and_counts_for_no_quest() {
        let mut server = test_server();
        let (player, _client) = keeper(&mut server);
        let species = server.content.creatures.id("base:rabbit").expect("rabbits");
        let food = server.content.creatures.get(species).tame_food[0];
        let hunter = server.content.skills.id("core:hunter").expect("Hunter");
        let paid = |server: &Server| (server.players[&player].skills.xp(hunter), server.players[&player].tallies.get("tamed:base:rabbit").copied().unwrap_or(0));
        let tame = |server: &mut Server, id: u32| {
            for _ in 0..60 {
                if server.creatures.live[&id].owner.is_some() {
                    return;
                }
                server.players.get_mut(&player).unwrap().inventory.add(food, 1);
                server.tick += 1;
                server.tend(player, id);
            }
            panic!("60 feeds did not tame it");
        };
        let wild = server.spawn_creature(species, server.spawn, 1).expect("room");
        let (xp, tally) = paid(&server);
        tame(&mut server, wild);
        let (xp_tamed, tally_tamed) = paid(&server);
        assert!(xp_tamed > xp && tally_tamed == tally + 1, "taming paid {xp} -> {xp_tamed}, tally {tally} -> {tally_tamed}");
        let tamed = server.content.creatures.condition().tamed;
        assert_eq!(server.creatures.live[&wild].condition, tamed);
        // Poorly kept, let go and taken in again, it comes back as poorly kept; one in better
        // condition than a newly tamed animal comes back as newly tamed.
        for (before, after) in [(0.2, 0.2), (0.9, tamed)] {
            server.creatures.live.get_mut(&wild).unwrap().condition = before;
            let_go(&mut server, player, wild);
            assert!(server.creatures.live[&wild].owner.is_none());
            assert_eq!(server.creatures.live[&wild].condition, before, "letting go mended or spoiled it");
            tame(&mut server, wild);
            assert_eq!(server.creatures.live[&wild].condition, after, "taking it in again from {before}");
        }
        assert_eq!(paid(&server), (xp_tamed, tally_tamed), "taming a let-go animal again paid");
    }

    /// A kept hen laying in its keeper's home lays into the nearest store there that takes
    /// an egg (a basket, which keeps only what is foraged, does not), and the keeper is told
    /// once home; with no room in the stores, or away from home, the egg falls as ever.
    #[test]
    fn eggs_laid_at_home_go_into_its_stores() {
        use crate::process::Process;
        let mut server = test_server();
        let (player, client) = keeper(&mut server);
        let at = server.spawn.floor().as_ivec3();
        let machines = server.content.machines.clone();
        let (basket, barrel) = (at + IVec3::X, at + IVec3::new(3, 0, 0));
        for (pos, name) in [(basket, "base:earthen_basket"), (barrel, "base:barrel")] {
            let block = server.content.blocks.expect(name);
            server.set_block(pos, block);
            if server.factory.get(pos).is_none() {
                server.factory.insert(pos, block, &machines);
            }
        }
        let egg = server.content.blocks.expect("base:egg");
        let on_ground = |server: &Server| server.items.saved(|b| server.content.blocks.def(b).name.clone()).iter().filter(|i| i.item == "base:egg").count();
        let held = |server: &Server, pos: IVec3| server.factory.get(pos).map_or(0, |(_, state)| state.bin.iter().flatten().filter(|s| s.item == egg).map(|s| s.count as u32).sum::<u32>());
        let pass = |server: &mut Server| {
            server.tick = server.tick.next_multiple_of(EVERY);
            server.tick_husbandry();
            server.tick += 1;
        };
        // Base's every egg nests, whichever bird lays it.
        let unnested: Vec<String> = server.content.creatures.iter().flat_map(|(_, c)| c.def.products.iter().filter(|p| p.item == "base:egg" && !p.nests).map(|_| c.def.name.clone())).collect();
        assert!(unnested.is_empty(), "eggs that do not nest: {unnested:?}");
        let hen = kept(&mut server, "base:junglefowl", 1);
        let due = |server: &mut Server| server.creatures.live.get_mut(&hen).unwrap().products_due.iter_mut().for_each(|due| *due = 0);
        // No home yet: it falls.
        pass(&mut server);
        assert_eq!((on_ground(&server), held(&server, barrel)), (1, 0), "laid with no home");
        // Slept here, it is home: the barrel takes it, the nearer basket does not.
        server.players.get_mut(&player).unwrap().bed = Some(server.spawn);
        due(&mut server);
        pass(&mut server);
        let laid = held(&server, barrel);
        assert!(laid > 0 && on_ground(&server) == 1 && held(&server, basket) == 0, "{laid} in the barrel, {} on the ground", on_ground(&server));
        assert_eq!(server.players[&player].nested, vec![("base:egg".to_owned(), laid)]);
        // Home, they are told, once.
        while client.try_recv().is_ok_and(|m| m.is_some()) {}
        crate::process::Survival.run(&mut server);
        assert!(told(&client).contains(&"home.nested".to_owned()));
        assert!(server.players[&player].nested.is_empty());
        crate::process::Survival.run(&mut server);
        assert!(!told(&client).contains(&"home.nested".to_owned()), "told twice");
        // The barrel full: it falls.
        let stone = server.content.blocks.expect("base:cobblestone");
        server.factory.state_mut(barrel).unwrap().bin.iter_mut().for_each(|slot| *slot = Some(ItemStack { item: stone, count: 64 }));
        due(&mut server);
        pass(&mut server);
        assert_eq!(on_ground(&server), 2, "laid into a full store");
        // Emptied, but home is elsewhere now: it falls.
        server.factory.state_mut(barrel).unwrap().bin.iter_mut().for_each(|slot| *slot = None);
        server.players.get_mut(&player).unwrap().bed = Some(server.spawn + Vec3::new(40.0, 0.0, 0.0));
        due(&mut server);
        pass(&mut server);
        assert_eq!((on_ground(&server), held(&server, barrel)), (3, 0), "laid away from home");
    }

    #[test]
    fn a_let_go_animal_in_a_pen_gives_nothing_and_never_breeds() {
        let mut server = test_server();
        let (player, _client) = keeper(&mut server);
        let eggs = |server: &Server| server.items.saved(|b| server.content.blocks.def(b).name.clone()).iter().filter(|i| i.item == "base:egg").count();
        // Two kept hens, ready to breed and with an egg due: a pass lays.
        let hens = [kept(&mut server, "base:junglefowl", 1), kept(&mut server, "base:junglefowl", 2)];
        for &hen in &hens {
            server.creatures.live.get_mut(&hen).unwrap().ready_until = server.tick + 1000;
        }
        let pass = |server: &mut Server| {
            server.tick = server.tick.next_multiple_of(EVERY);
            server.tick_husbandry();
            server.tick += 1;
        };
        pass(&mut server);
        assert!(eggs(&server) > 0, "kept hens laid nothing");
        assert!(server.creatures.live.len() > 2, "kept hens, ready side by side, bred nothing");
        // Let go, they lay nothing and breed no more, though due and side by side.
        for &hen in &hens {
            let_go(&mut server, player, hen);
            let creature = server.creatures.live.get_mut(&hen).unwrap();
            assert_eq!((creature.owner.as_deref(), creature.ready_until), (None, 0));
            creature.products_due = vec![0; creature.products_due.len()];
        }
        let (laid, born) = (eggs(&server), server.creatures.live.len());
        for _ in 0..20 {
            pass(&mut server);
        }
        assert_eq!((eggs(&server), server.creatures.live.len()), (laid, born), "let-go hens laid or bred");
        // Their old keeper's grain only starts taming them again; it readies neither.
        let wheat = server.content.blocks.id("base:wheat").expect("wheat");
        server.players.get_mut(&player).unwrap().inventory.add(wheat, 1);
        server.tend(player, hens[0]);
        assert_eq!(server.creatures.live[&hens[0]].ready_until, 0);
        // Even ready, a let-go hen beside a ready kept one is no mate: no young, and the kept
        // hen is left ready.
        let keeper_hen = kept(&mut server, "base:junglefowl", 3);
        server.creatures.live.get_mut(&keeper_hen).unwrap().ready_until = server.tick + 1000;
        server.creatures.live.get_mut(&hens[0]).unwrap().ready_until = server.tick + 1000;
        let born = server.creatures.live.len();
        pass(&mut server);
        assert_eq!(server.creatures.live.len(), born, "a let-go hen bred");
        assert!(server.creatures.live[&keeper_hen].ready_until > 0, "a let-go hen took the kept hen's readiness");
        assert!(server.mate_for(keeper_hen, &crate::creatures::Neighbours::of(&server.creatures.live)).is_none(), "a let-go hen is a mate");
    }

    #[test]
    fn a_hungry_kept_animal_eats_by_hand_and_a_fed_one_keeps_the_food() {
        let mut server = test_server();
        let (player, client) = keeper(&mut server);
        // A kept fox, which never grazes and does not breed, poorly kept and three days unfed.
        let fox = kept(&mut server, "base:fox", 1);
        let meat = server.content.creatures.get(server.creatures.live[&fox].species).tame_food[0];
        let day = voxmar_protocol::TICKS_PER_DAY as f64;
        let creature = server.creatures.live.get_mut(&fox).unwrap();
        (creature.condition, creature.fed) = (0.5, server.day - 3.0 * day);
        assert_eq!(server.needs_of(fox).food, Met::No);
        server.players.get_mut(&player).unwrap().inventory.add(meat, 2);
        let in_hand = |server: &Server| server.players[&player].inventory.get(0).map_or(0, |stack| stack.count);
        assert!(server.creature_accepts(player, fox, meat), "a hungry kept fox turned its food down");
        server.tend(player, fox);
        assert_eq!(in_hand(&server), 1, "the fox ate nothing");
        assert_eq!(server.needs_of(fox).food, Met::Yes, "a meal did not feed it");
        // Fed, and nothing to breed for, it leaves the rest in hand, without a word.
        assert!(!server.creature_accepts(player, fox, meat));
        server.tend(player, fox);
        assert_eq!(in_hand(&server), 1);
        assert!(told(&client).iter().all(|key| !key.contains("husbandry.")));
    }

    #[test]
    fn a_poorly_kept_animal_eats_but_says_why_it_will_not_breed() {
        let mut server = test_server();
        let (player, client) = keeper(&mut server);
        let rabbit = kept(&mut server, "base:rabbit", 1);
        let food = server.content.creatures.get(server.creatures.live[&rabbit].species).breed_food[0];
        server.players.get_mut(&player).unwrap().inventory.add(food, 3);
        let in_hand = |server: &Server| server.players[&player].inventory.get(0).map_or(0, |stack| stack.count);
        let day = voxmar_protocol::TICKS_PER_DAY as f64;
        // Hungry and at 0.4: it eats as a meal, is not readied, and says why.
        let creature = server.creatures.live.get_mut(&rabbit).unwrap();
        (creature.condition, creature.fed) = (0.4, server.day - 2.0 * day);
        creature.needs.water = Met::No;
        told(&client);
        server.tend(player, rabbit);
        assert_eq!((in_hand(&server), server.creatures.live[&rabbit].ready_until), (2, 0));
        assert_eq!(told(&client), ["refused: husbandry.too_poorly_kept_wants"]);
        // Fed now, it keeps the food in hand and says why again.
        server.tend(player, rabbit);
        assert_eq!((in_hand(&server), server.creatures.live[&rabbit].ready_until), (2, 0));
        assert_eq!(told(&client), ["refused: husbandry.too_poorly_kept_wants"]);
        // Kept well, the same food readies it.
        server.creatures.live.get_mut(&rabbit).unwrap().condition = 0.7;
        server.tend(player, rabbit);
        assert_eq!(in_hand(&server), 1);
        assert!(server.creatures.live[&rabbit].ready_until > server.tick);
        assert!(told(&client).is_empty());
    }

    #[test]
    fn condition_sets_how_often_products_come_not_how_many() {
        let mut server = test_server();
        // Two hens, one kept at condition 0 and one at 1.
        let (poor, fine) = (kept(&mut server, "base:junglefowl", 1), kept(&mut server, "base:junglefowl", 2));
        let rules = *server.content.creatures.condition();
        let mut laid = [0u32; 2];
        server.tick = server.tick.next_multiple_of(EVERY);
        // Six hours of husbandry passes, counting each time a hen lays (its next egg due later).
        for _ in 0..(6 * 3600 * TICKS_PER_SECOND as u64 / EVERY) {
            for (id, condition) in [(poor, 0.0), (fine, 1.0)] {
                server.creatures.live.get_mut(&id).unwrap().condition = condition;
            }
            let due = [poor, fine].map(|id| server.creatures.live[&id].products_due[0]);
            server.tick_husbandry();
            for (i, id) in [poor, fine].into_iter().enumerate() {
                laid[i] += u32::from(server.creatures.live[&id].products_due[0] != due[i]);
            }
            server.tick += EVERY;
        }
        let ratio = laid[0] as f32 / laid[1] as f32;
        let expected = rules.output(0.0) / rules.output(1.0);
        assert!(laid[1] >= 50 && (ratio - expected).abs() < 0.12, "{laid:?} eggs, {ratio:.2} against {expected:.2}");
    }

    #[test]
    fn a_kept_animal_in_land_not_loaded_lays_nothing_until_it_loads() {
        let mut server = test_server();
        let hen = kept(&mut server, "base:junglefowl", 1);
        let eggs = |server: &Server| server.items.saved(|b| server.content.blocks.def(b).name.clone()).iter().filter(|i| i.item == "base:egg").count();
        // Far off in land nobody has loaded, with an egg due a minute from now: an hour of
        // passes lays nothing, and the egg is still a minute off.
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        server.tick = server.tick.next_multiple_of(EVERY);
        let minute = 60 * TICKS_PER_SECOND as u64;
        let creature = server.creatures.live.get_mut(&hen).unwrap();
        (creature.body.position, creature.home) = (far, far);
        creature.products_due[0] = server.tick + minute;
        for _ in 0..(3600 * TICKS_PER_SECOND as u64 / EVERY) {
            server.tick_husbandry();
            server.tick += EVERY;
        }
        assert_eq!(eggs(&server), 0, "a hen in unloaded land laid");
        assert_eq!(server.creatures.live[&hen].products_due[0], server.tick + minute);
        // Back where the land is loaded, it lays when its minute is up.
        let creature = server.creatures.live.get_mut(&hen).unwrap();
        (creature.body.position, creature.home) = (server.spawn, server.spawn);
        for _ in 0..(2 * minute / EVERY) {
            server.tick_husbandry();
            server.tick += EVERY;
        }
        assert_eq!(eggs(&server), 1);
    }

    #[test]
    fn an_animal_in_land_not_loaded_is_paused_as_its_land_is() {
        let mut server = test_server();
        let id = kept_rabbit(&mut server, 1);
        let day = voxmar_protocol::TICKS_PER_DAY as f64;
        // Its home is far off, where nothing is loaded. Last looked at a day ago, when it had
        // eaten half a day before: a day passes, and nothing changes.
        let far = server.spawn + Vec3::new(4000.0, 0.0, 0.0);
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.condition, creature.reckoned, creature.fed, creature.home) = (0.6, server.day - day, server.day - 1.5 * day, far);
        server.reckon_condition();
        let creature = &server.creatures.live[&id];
        assert_eq!((creature.condition, creature.reckoned), (0.6, server.day));
        assert!((server.days_since(creature.fed) - 0.5).abs() < 1e-4, "it hungered while its land slept");
        // Home again, the same day apart moves it.
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.reckoned, creature.home, creature.fed) = (server.day - day, server.spawn, server.day - 3.0 * day);
        server.reckon_condition();
        assert!(server.creatures.live[&id].condition < 0.6, "a hungry animal at home held its condition");
    }

    #[test]
    fn a_roof_within_reach_shelters() {
        let mut server = test_server();
        let at = server.spawn;
        let cell = at.floor().as_ivec3();
        server.load_area(cell - IVec3::new(8, 0, 8), cell + IVec3::new(8, 0, 8));
        let stone = server.content.blocks.id("base:stone").expect("stone");
        // Open sky all round: no roof. A block two off and a little above its head: a roof.
        let clear = |server: &mut Server| {
            for dx in -4..=4 {
                for dz in -4..=4 {
                    for dy in 0..50 {
                        server.set_block(cell + IVec3::new(dx, dy, dz), voxmar_engine::block::BlockId::AIR);
                    }
                }
            }
        };
        clear(&mut server);
        assert!(!server.roof_near(at, 0.5, 3.0));
        server.set_block(cell + IVec3::new(2, 3, 0), stone);
        assert!(server.roof_near(at, 0.5, 3.0));
        // A wall or step two off, at head height with open sky above it, is no roof.
        clear(&mut server);
        server.set_block(cell + IVec3::new(2, 0, 0), stone);
        assert!(!server.roof_near(at, 0.5, 3.0));
        // Further off than reach shelters nothing.
        clear(&mut server);
        server.set_block(cell + IVec3::new(4, 3, 0), stone);
        assert!(!server.roof_near(at, 0.5, 3.0));
    }

    #[test]
    fn work_and_the_save_follow_condition() {
        let mut server = test_server();
        let id = kept_rabbit(&mut server, 1);
        let rules = *server.content.creatures.condition();
        let day = voxmar_protocol::TICKS_PER_DAY as f64;
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.condition, creature.fed) = (1.0, server.day - 0.5 * day);
        assert!((server.working_share(id) - rules.output(1.0)).abs() < 1e-6);
        // Unfed past two days it stops walking; a wild one never works.
        server.creatures.live.get_mut(&id).unwrap().fed = server.day - 2.5 * day;
        assert_eq!(server.working_share(id), 0.0);
        server.creatures.live.get_mut(&id).unwrap().owner = None;
        assert_eq!(server.working_share(id), 0.0);
        server.creatures.live.get_mut(&id).unwrap().owner = Some("test".into());
        // The save keeps its condition, when it ate and its count of rolls, and brings
        // them back, through the file's own text.
        let creature = server.creatures.live.get_mut(&id).unwrap();
        (creature.condition, creature.fed, creature.rolls) = (0.42, server.day - 1.5 * day, 600);
        let saved = server.saved_creatures();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].condition, Some(0.42));
        assert!((saved[0].fed_days_ago.unwrap() - 1.5).abs() < 1e-3);
        let text = ron::ser::to_string(&saved).unwrap();
        let read: Vec<SavedCreature> = ron::from_str(&text).unwrap();
        assert_eq!(read, saved);
        server.creatures.live.clear();
        server.restore_creatures(read);
        let back = server.creatures.live.values().next().expect("restored");
        assert_eq!(back.condition, 0.42);
        assert!((server.days_since(back.fed) - 1.5).abs() < 1e-3);
        assert_eq!(back.rolls, 600, "it thinks on from where it was, not over again");
        // One saved before condition comes back newly tamed and just fed.
        let old = rabbit(server.spawn);
        server.creatures.live.clear();
        server.restore_creatures(vec![old]);
        let back = server.creatures.live.values().next().expect("restored");
        assert_eq!(back.condition, rules.tamed);
        assert!(server.days_since(back.fed).abs() < 1e-3);
        // A file from before rolls were counted reads, and starts its count at none.
        let older = text.replace(",rolls:600", "");
        assert_ne!(older, text, "the field is in the text: {text}");
        let read: Vec<SavedCreature> = ron::from_str(&older).unwrap();
        assert_eq!(read[0].rolls, 0);
    }

    /// When a kept animal's next product is due is drawn by its own count of rolls, never
    /// the tick it lays on.
    #[test]
    fn a_products_timing_draws_by_its_own_count_not_the_tick() {
        let due_at = |tick: u64| {
            let mut server = test_server();
            let (_player, client) = keeper(&mut server);
            std::mem::forget(client);
            let hen = kept(&mut server, "base:junglefowl", 1);
            server.tick = tick.next_multiple_of(EVERY);
            let mut waits = Vec::new();
            for _ in 0..12 {
                server.creatures.live.get_mut(&hen).unwrap().products_due[0] = 0;
                server.tick_husbandry();
                waits.push(server.creatures.live[&hen].products_due[0] - server.tick);
                server.tick += EVERY;
            }
            (waits, server.creatures.live[&hen].rolls)
        };
        let first = due_at(2_000);
        assert_eq!(first.1, 12, "each lay counted on");
        assert!(first.0.windows(2).any(|w| w[0] != w[1]), "every wait alike: {:?}", first.0);
        assert_eq!(due_at(9_337), first, "the same lays later came due otherwise");
    }

    /// What a kept animal lays by itself, and how many young a pair has, are drawn by the
    /// world's count of rolls, and what is laid pops by its count of drops, not the tick
    /// they fall on, and each counts on.
    #[test]
    fn a_lay_and_a_litter_draw_by_the_worlds_count_not_the_tick() {
        let kept_at = |tick: u64| {
            let mut server = test_server();
            server.meta.draw_key = 0x0d1a;
            let (_player, client) = keeper(&mut server);
            std::mem::forget(client);
            let hen = kept(&mut server, "base:junglefowl", 1);
            server.tick = tick.next_multiple_of(EVERY);
            let mut laid = Vec::new();
            for _ in 0..12 {
                server.creatures.live.get_mut(&hen).unwrap().products_due[0] = 0;
                server.tick_husbandry();
                server.tick += EVERY;
                let mut lying: Vec<_> = server.items.take_all().map(|d| (d.stack.item, d.stack.count, d.body.position.to_array(), d.body.velocity.to_array())).collect();
                lying.sort_by(|a, b| a.partial_cmp(b).unwrap());
                laid.push(lying);
            }
            server.creatures.live.clear();
            let (a, b) = (kept_rabbit(&mut server, 1), kept_rabbit(&mut server, 2));
            let mut litters = Vec::new();
            for _ in 0..16 {
                for id in [a, b] {
                    let parent = server.creatures.live.get_mut(&id).unwrap();
                    (parent.ready_until, parent.breeds_again) = (server.tick + 100, 0);
                }
                server.breed(a, b);
                let mut young: Vec<_> = server.creatures.live.iter().filter(|&(&id, _)| id != a && id != b).map(|(&id, c)| (id, c.genome.clone(), c.seed)).collect();
                young.sort_by_key(|y| y.0);
                litters.push(young);
                server.creatures.live.retain(|&id, _| id == a || id == b);
            }
            (laid, litters, server.meta.rolls)
        };
        let first = kept_at(1_000);
        assert!(first.0.iter().all(|l| !l.is_empty()), "a hen due to lay laid nothing: {:?}", first.0);
        let born: usize = first.1.iter().map(Vec::len).sum();
        assert_eq!(first.2, (12 + 16 + born) as u64, "each lay, litter and young counted on");
        assert!(first.1.windows(2).any(|w| w[0].len() != w[1].len()), "sixteen litters all one size: {:?}", first.1.iter().map(Vec::len).collect::<Vec<_>>());
        let genomes: Vec<_> = first.1.iter().flatten().map(|y| &y.1).collect();
        assert!(genomes.windows(2).any(|w| w[0] != w[1]), "every young born with the same genes");
        assert_eq!(kept_at(1_377), first, "the same lays and litters later went otherwise");
    }
}
