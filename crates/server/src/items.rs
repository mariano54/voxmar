//! Dropped items, as Minecraft's item entities: what breaking a block gives pops out into
//! the world, a thrown stack flies from the hand, and both lie there until a player comes
//! close, when they are drawn in and picked up, or until they have lain for five minutes.
//!
//! Each tick moves the items that are moving (resting ones sleep, see
//! [`Loose::step`]), draws in and picks up those near a player with room for them,
//! merges neighbouring stacks of the same item now and then, and tells each player about
//! the items within sight that changed. Items in columns that are not loaded wait, as
//! Minecraft's do, timer and all.

use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::dropped::{self, LIFETIME, Loose, MAGNET};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::inventory::{ItemStack, MAX_STACK, max_stack};
use voxmar_engine::movement::{self, Surroundings};
use voxmar_engine::physics::{Aabb, player as dims};
use voxmar_engine::traits;
use voxmar_protocol::{ItemGone, ItemView, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::save::SavedItem;

/// Players see dropped items this many blocks away (Minecraft draws them to 64 at most,
/// and ours are tinier still).
const SEEN_WITHIN: f32 = 48.0;
/// How fast a drawn-in item flies at the player, in blocks per second.
const MAGNET_SPEED: f32 = 7.0;
/// How far a player's box reaches for items, across and up and down (Minecraft's 1 and
/// 0.5).
const REACH: Vec3 = Vec3::new(1.0, 0.5, 1.0);
/// Stacks of the same item this close together merge.
const MERGE_WITHIN: f32 = 0.6;
/// Ticks between merges of resting stacks; moving ones try every few ticks.
const MERGE_EVERY: u64 = 10;
/// Dropped items alive at once at most; beyond it the oldest go first.
pub const MAX_ITEMS: usize = 4096;

/// A stack lying loose in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dropped {
    pub stack: ItemStack,
    pub body: Loose,
    /// Ticks it has lain in a loaded column.
    pub age: u32,
    /// Ticks until it can be picked up.
    pub pickup_in: u32,
    /// Whose it is, by a hash of their name, if only they may pick it up (what a player
    /// left where they died), and the ticks it lasts.
    pub owner: Option<u64>,
    pub lifetime: u32,
}

/// The owner hash of a player name (see `Dropped::owner`).
pub(crate) fn owner_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Ticks a dropped item lasts unless it says otherwise.
fn lifetime() -> u32 {
    (LIFETIME * TICKS_PER_SECOND as f32) as u32
}

impl Dropped {
    fn view(&self, id: u32) -> ItemView {
        ItemView { id, stack: self.stack, position: self.body.position, velocity: self.body.velocity }
    }
}

/// Every dropped item, and what players were last told.
#[derive(Default)]
pub struct Items {
    all: FxHashMap<u32, Dropped>,
    next_id: u32,
    /// Changed since players were last told.
    changed: FxHashSet<u32>,
    /// Gone since players were last told.
    gone: Vec<ItemGone>,
}

impl Items {
    /// Every drop lying in the world, taken out of it, for tests to look over.
    #[cfg(test)]
    pub(crate) fn take_all(&mut self) -> impl Iterator<Item = Dropped> + '_ {
        self.all.drain().map(|(_, d)| d)
    }

    /// Older saves may record several spellings of one keeper. Only hashes of known names
    /// are migrated; a hash with no surviving spelling must remain protected.
    pub(crate) fn canonical_owner(&mut self, aliases: &[u64], canonical: u64) {
        for (&id, item) in &mut self.all {
            if item.owner.is_some_and(|owner| owner != canonical && aliases.binary_search(&owner).is_ok()) {
                item.owner = Some(canonical);
                self.changed.insert(id);
            }
        }
    }

    pub fn spawn(&mut self, stack: ItemStack, body: Loose, pickup_delay: f32) -> u32 {
        self.spawn_for(stack, body, pickup_delay, None, lifetime())
    }

    /// Spawns a stack only `owner` may pick up (see `Dropped::owner`), lasting `lifetime`
    /// ticks.
    pub fn spawn_for(&mut self, stack: ItemStack, body: Loose, pickup_delay: f32, owner: Option<u64>, lifetime: u32) -> u32 {
        // Over the limit, the oldest goes first, a player's own things last.
        if self.all.len() >= MAX_ITEMS
            && let Some((&oldest, _)) = self.all.iter().max_by_key(|(_, item)| (item.owner.is_none(), item.age))
        {
            self.remove(oldest, None);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let pickup_in = (pickup_delay * TICKS_PER_SECOND as f32).round() as u32;
        self.all.insert(id, Dropped { stack, body, age: 0, pickup_in, owner, lifetime });
        self.changed.insert(id);
        id
    }

    fn remove(&mut self, id: u32, collector: Option<PlayerId>) {
        if self.all.remove(&id).is_some() {
            self.changed.remove(&id);
            self.gone.push(ItemGone { id, collector });
        }
    }

}

/// The world as a dropped item feels it.
struct Around<'a>(&'a Server);

impl Surroundings for Around<'_> {
    fn solid(&self, cell: IVec3) -> bool {
        self.0.is_solid(cell)
    }

    fn water(&self, aabb: &Aabb) -> (f32, Vec2) {
        let blocks = &self.0.content.blocks;
        movement::water_around(blocks, |p| self.0.block(p), aabb)
    }
}

/// Whether a crop nobody sowed gives anything for a draw of `roll` (see `unsown_gives`).
pub(crate) fn unsown_roll(def: &voxmar_engine::crop::CropDef, roll: f32) -> bool {
    def.fruit || roll < def.unsown.unwrap_or(1.0)
}

/// A small, fast hash for the spread of popped drops and for chances: deterministic for a
/// given key (a count-derived one, `WorldMeta::next_pop`, or a tick) and place, like
/// everything else the server does. Each value goes through
/// the whole of MurmurHash3's finaliser: with half of it, ticks in a row gave values in a
/// slow drift, so a one-in-eight chance tried on sixty ticks running could miss them all.
pub(crate) fn random3(key: u64, cell: IVec3, salt: u32) -> [f32; 3] {
    let mut h = key.wrapping_mul(0xd6e8_feb8_6659_fd93)
        ^ (cell.x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (cell.y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ (cell.z as u64).wrapping_mul(0x1656_67b1_9e37_79f9)
        ^ (salt as u64).wrapping_mul(0x27d4_eb2f_1656_67c5);
    [0, 1, 2].map(|_| {
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        (h >> 40) as f32 / (1u64 << 24) as f32
    })
}

impl Server {
    /// Pops what breaking the blocks at `cells` (a block and the plants it held up) drops
    /// out into the world: a ripe crop its harvest, earning `agent` its experience.
    /// `placed` lists cells a player put their block in: those give their plain drop, never
    /// a harvest or a forage.
    pub(crate) fn drop_loot(&mut self, agent: PlayerId, cells: &[IVec3], placed: &[IVec3]) {
        for (i, &cell) in cells.iter().enumerate() {
            let Some(block) = self.block(cell) else { continue };
            let wild = !placed.contains(&cell);
            // Sown or not, it is gone from the farm.
            let sown = self.farm.take_sown(cell);
            // A ripe crop above the player's level (one standing on broken ground) gives
            // only its plain drop.
            let ripe = self.ripe_crop(block).filter(|&crop| {
                let crop = self.content.crops.get(crop);
                wild && self.check_level(agent, crop.skill) >= crop.def.level
            });
            if let Some(crop) = ripe {
                if sown {
                    self.harvest(agent, crop, cell, i as u32, true);
                } else {
                    self.unsown_ripe(agent, crop, cell, i as u32);
                }
                continue;
            }
            if let Some((crop, plants)) = self.content.crops.wild_form(block).filter(|_| wild) {
                for plant in 0..plants as u32 {
                    self.forage(agent, crop, cell, i as u32 * 8 + plant);
                }
                continue;
            }
            let (drops, drop_count) = (self.content.blocks.info(block).drops, self.content.blocks.info(block).drop_count);
            if drops.is_air() || drop_count == 0 {
                continue;
            }
            // A furnishing the world placed (a village's altar) gives nothing.
            if wild && self.content.blocks.def(block).has_tag("furnishing") {
                continue;
            }
            // A crop nobody sowed (a village's field), broken before it is harvested,
            // gives its seed or produce only now and then: enough to start the crop, not a
            // store of free food.
            if wild
                && !sown
                && let Some((crop, _)) = self.content.crops.stage_of(block)
                && !self.unsown_gives(agent, crop)
            {
                continue;
            }
            let stack = ItemStack { item: drops, count: drop_count };
            // Scattered by the world's count of drops popped, not the tick it breaks on.
            let body = dropped::popped(cell, self.meta.next_pop(cell, i as u32));
            self.items.spawn(stack, body, dropped::PICKUP_DELAY);
        }
    }

    /// Whether a crop nobody sowed gives anything this time (`unsown` in crops.ron), by
    /// the player's count of wild crops taken; fruit on trees always does.
    pub(crate) fn unsown_gives(&mut self, agent: PlayerId, crop: voxmar_engine::crop::CropId) -> bool {
        // Tree fruit always gives: only a crop that may give nothing spends a draw.
        if self.content.crops.get(crop).def.fruit {
            return true;
        }
        let [roll, _] = self.draw_for(agent, crate::save::Draw::Forage);
        unsown_roll(&self.content.crops.get(crop).def, roll)
    }

    /// The next two draws of `kind` by the player's own count of it (`Player::draw`); for
    /// a player not here, the world's own rolls of that kind.
    pub(crate) fn draw_for(&mut self, agent: PlayerId, kind: crate::save::Draw) -> [f32; 2] {
        match self.players.get_mut(&agent) {
            Some(player) => player.draw(kind),
            None => self.meta.next_roll(kind.salt()),
        }
    }

    /// A ripe crop nobody sowed: one the world grew wild forages like its wild form (its
    /// `forage`, and forage experience at its level); one with no forage list (a village's)
    /// now and then gives its harvest, never its experience.
    pub(crate) fn unsown_ripe(&mut self, agent: PlayerId, crop: voxmar_engine::crop::CropId, cell: IVec3, salt: u32) {
        if !self.content.crops.get(crop).forage.is_empty() {
            self.forage(agent, crop, cell, salt);
        } else if self.unsown_gives(agent, crop) {
            let loot = self.content.crops.get(crop).harvest.clone();
            self.yield_loot(agent, crop, &loot, cell, salt);
        }
    }

    /// Throws up to `count` items of `slot` from the player's hand.
    pub(crate) fn throw(&mut self, agent: PlayerId, slot: usize, count: u8) {
        let Some(player) = self.players.get_mut(&agent) else { return };
        let Some(stack) = player.inventory.take(slot, count) else { return };
        player.changed(traits::INVENTORY);
        let body = dropped::thrown(player.eye(), player.look());
        self.items.spawn(stack, body, dropped::THROWN_PICKUP_DELAY);
    }

    /// Moves, picks up, merges and ages dropped items, then tells players what changed.
    pub(crate) fn tick_items(&mut self) {
        let dt = 1.0 / TICKS_PER_SECOND as f32;
        let mut items = std::mem::take(&mut self.items);
        let ids: Vec<u32> = items.all.keys().copied().collect();
        for &id in &ids {
            let item = items.all.get_mut(&id).expect("listed above");
            // Items age by the world's clock, loaded or not.
            item.age += 1;
            if item.age >= item.lifetime || item.body.position.y < -64.0 {
                items.remove(id, None);
                continue;
            }
            if !self.world.contains(ColumnPos::of_block(item.body.position.floor().as_ivec3())) {
                continue;
            }
            item.pickup_in = item.pickup_in.saturating_sub(1);
            if item.body.step(&Around(self), dt) {
                items.changed.insert(id);
            }
        }
        self.collect_items(&mut items, dt);
        merge(&mut items, self.tick);
        self.items = items;
        self.replicate_items();
    }

    /// Draws items towards players near them who have room, and picks up those in reach.
    fn collect_items(&mut self, items: &mut Items, dt: f32) {
        let collectors: Vec<(PlayerId, Vec3, Aabb, u64)> = self
            .players
            .values()
            .filter(|p| p.joined && p.health.alive())
            .map(|p| {
                let mut reach = p.aabb();
                reach.min -= REACH;
                reach.max += REACH;
                (p.id, p.position + Vec3::Y * dims::HEIGHT * 0.5, reach, owner_of(&p.name))
            })
            .collect();
        if collectors.is_empty() {
            return;
        }
        let ids: Vec<u32> = items.all.keys().copied().collect();
        for id in ids {
            let item = items.all[&id];
            if item.pickup_in > 0 {
                continue;
            }
            let centre = item.body.position + Vec3::Y * dropped::SIZE / 2.0;
            let nearest = collectors
                .iter()
                .filter(|(pid, middle, _, name)| {
                    // What a player left where they died is theirs alone.
                    item.owner.is_none_or(|owner| owner == *name)
                        && middle.distance(centre) < MAGNET + 1.0
                        && self.players[pid].inventory.room_for(item.stack.item, 1) > 0
                })
                .min_by(|a, b| a.1.distance(centre).total_cmp(&b.1.distance(centre)));
            let Some(&(collector, middle, reach, _)) = nearest else { continue };
            let inside = item.body.aabb().min.cmple(reach.max).all() && item.body.aabb().max.cmpge(reach.min).all();
            if inside {
                let player = self.players.get_mut(&collector).expect("a collector");
                let left = player.inventory.add(item.stack.item, item.stack.count);
                if left == item.stack.count {
                    continue;
                }
                player.changed(traits::INVENTORY);
                if left == 0 {
                    items.remove(id, Some(collector));
                } else {
                    items.all.get_mut(&id).expect("still there").stack.count = left;
                    items.changed.insert(id);
                }
            } else if middle.distance(centre) < MAGNET {
                // Drawn in, faster the closer it gets, still stopped by walls.
                let item = items.all.get_mut(&id).expect("still there");
                let pull = (middle - centre).normalize_or_zero() * MAGNET_SPEED;
                item.body.velocity = item.body.velocity.lerp(pull, (12.0 * dt).min(1.0));
                item.body.on_ground = false;
                items.changed.insert(id);
            }
        }
    }

    /// Sends each player the changed items within sight, and the ones gone from it.
    fn replicate_items(&mut self) {
        let changed = std::mem::take(&mut self.items.changed);
        let gone = std::mem::take(&mut self.items.gone);
        for player in self.players.values_mut().filter(|p| p.joined) {
            let mut message_changed = Vec::new();
            let mut message_gone: Vec<ItemGone> = gone.iter().filter(|g| player.items_seen.remove(&g.id)).copied().collect();
            let eye = player.position;
            for (&id, item) in &self.items.all {
                let near = item.body.position.distance(eye) < SEEN_WITHIN;
                let seen = player.items_seen.contains(&id);
                if near && (!seen || changed.contains(&id)) {
                    player.items_seen.insert(id);
                    message_changed.push(item.view(id));
                } else if !near && seen {
                    player.items_seen.remove(&id);
                    message_gone.push(ItemGone { id, collector: None });
                }
            }
            if !message_changed.is_empty() || !message_gone.is_empty() {
                player.connection.send(ServerMessage::Items { changed: message_changed, gone: message_gone });
            }
        }
    }
}

/// Merges neighbouring stacks of the same item into the larger, as Minecraft does, so a
/// heap of drops becomes a few stacks. Resting items try now and then, moving ones more
/// often.
fn merge(items: &mut Items, tick: u64) {
    let every_few = tick.is_multiple_of(2);
    let resting_too = tick.is_multiple_of(MERGE_EVERY);
    if !every_few {
        return;
    }
    // Bucketed by block, so each item only meets its neighbours.
    let mut cells: FxHashMap<IVec3, Vec<u32>> = FxHashMap::default();
    for (&id, item) in &items.all {
        if item.stack.count < max_stack(item.stack.item) {
            cells.entry(item.body.position.floor().as_ivec3()).or_default().push(id);
        }
    }
    let mut ids: Vec<u32> = cells.values().flatten().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(&item) = items.all.get(&id) else { continue };
        if item.body.resting() && !resting_too {
            continue;
        }
        let home = item.body.position.floor().as_ivec3();
        let mut partner = None;
        'search: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    for &other in cells.get(&(home + IVec3::new(dx, dy, dz))).into_iter().flatten() {
                        let Some(o) = items.all.get(&other) else { continue };
                        if other != id
                            && o.stack.item == item.stack.item
                            && o.stack.count < max_stack(o.stack.item)
                            && o.owner == item.owner
                            && o.body.position.distance(item.body.position) < MERGE_WITHIN
                        {
                            partner = Some(other);
                            break 'search;
                        }
                    }
                }
            }
        }
        let Some(other) = partner else { continue };
        let o = items.all[&other];
        // The smaller stack goes into the larger; ties keep the older item.
        let (into, from) = if (o.stack.count, o.age) >= (item.stack.count, item.age) { (other, id) } else { (id, other) };
        let (into_item, from_item) = (items.all[&into], items.all[&from]);
        let moved = from_item.stack.count.min(max_stack(into_item.stack.item).saturating_sub(into_item.stack.count));
        let target = items.all.get_mut(&into).expect("listed");
        target.stack.count += moved;
        // The merged stack lasts as long as the younger of the two would have, and can be
        // picked up when both could.
        target.age = target.age.min(from_item.age);
        target.pickup_in = target.pickup_in.max(from_item.pickup_in);
        items.changed.insert(into);
        if moved == from_item.stack.count {
            items.remove(from, None);
        } else {
            items.all.get_mut(&from).expect("listed").stack.count -= moved;
            items.changed.insert(from);
        }
    }
}

impl Items {
    pub fn saved(&self, name: impl Fn(BlockId) -> String) -> Vec<SavedItem> {
        let mut saved: Vec<(u32, SavedItem)> = self
            .all
            .iter()
            .map(|(&id, item)| {
                let age = item.age as f32 / TICKS_PER_SECOND as f32;
                let lifetime = (item.lifetime != self::lifetime()).then(|| item.lifetime as f32 / TICKS_PER_SECOND as f32);
                (id, SavedItem { item: name(item.stack.item), count: item.stack.count, position: item.body.position.to_array(), age, owner: item.owner, lifetime })
            })
            .collect();
        saved.sort_by_key(|(id, _)| *id);
        saved.into_iter().map(|(_, item)| item).collect()
    }

    /// Items from a save, minus those whose item no longer exists.
    pub fn restore(saved: Vec<SavedItem>, id: impl Fn(&str) -> Option<BlockId>) -> Self {
        let mut items = Items::default();
        for item in saved {
            let (Some(block), position) = (id(&item.item), Vec3::from(item.position)) else { continue };
            if item.count == 0 || !position.is_finite() {
                continue;
            }
            let stack = ItemStack { item: block, count: item.count.min(MAX_STACK) };
            let life = item.lifetime.map_or(lifetime(), |seconds| (seconds * TICKS_PER_SECOND as f32) as u32);
            let new = items.spawn_for(stack, Loose::new(position, Vec3::ZERO), 0.0, item.owner, life);
            items.all.get_mut(&new).expect("just spawned").age = (item.age * TICKS_PER_SECOND as f32) as u32;
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clear and a move sent together are handled before the tick, and the tick picks up
    /// whatever lies where the player now stands: so the one state the player is sent that
    /// tick already carries what was picked up, and the pack is never seen empty. The gate's
    /// dry step once cleared, then moved onto a rabbit's loot from an earlier step's kill,
    /// and waited forever for the empty pack it would clear again on; it now clears only once
    /// there.
    #[test]
    fn a_clear_then_a_move_onto_loot_is_never_seen_empty() {
        use voxmar_protocol::{ClientMessage, ServerMessage};
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
        (player.joined, player.admin, player.name) = (true, true, "test".into());
        let raw = server.content.blocks.expect("base:raw_small_game");
        player.inventory.add(raw, 2);
        server.players.insert(1, player);
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at - IVec3::new(12, 0, 12), at + IVec3::new(12, 0, 12));
        // The rabbit's loot, lying well out of reach of where the player stands now.
        let to = server.players[&1].position + Vec3::new(8.0, 0.0, 0.0);
        server.items.spawn(ItemStack { item: raw, count: 1 }, Loose::new(to + Vec3::new(0.4, 0.0, 0.0), Vec3::ZERO), 0.0);
        while let Ok(Some(_)) = client.try_recv() {}
        client.send(ClientMessage::Command { line: "clear raw_small_game".into() });
        client.send(ClientMessage::Command { line: format!("tp {} {} {}", to.x, to.y, to.z) });
        server.handle_messages();
        server.tick();
        let carried: Vec<u32> = std::iter::from_fn(|| client.try_recv().ok().flatten())
            .filter_map(|m| match m {
                ServerMessage::PlayerState { inventory, .. } => Some(inventory.count(raw)),
                _ => None,
            })
            .collect();
        assert_eq!(carried, vec![1], "the states sent after a clear and a move onto loot");
    }

    #[test]
    fn a_crop_nobody_sowed_gives_at_its_unsown_chance_and_tree_fruit_always() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_engine::content::Content::load(&dir).unwrap();
        let def = |name: &str| content.crops.iter().find(|(_, c)| c.def.name == name).unwrap().1.def.clone();
        let tomato = def("base:tomato");
        let chance = tomato.unsown.unwrap();
        // A player's first 10,000 forage draws, as `unsown_gives` draws them: the share's
        // spread is 0.0036, so this is about three spreads either side.
        use crate::save::{Draw, numbered_draws};
        let key = crate::player::player_draw_key(0x0d1a, "ada");
        let forage = |n: u64| numbered_draws(key, Draw::Forage.salt(), n)[0];
        let gave = (0..10_000).filter(|&n| unsown_roll(&tomato, forage(n))).count() as f32 / 10_000.0;
        assert!((chance - 0.01..=chance + 0.01).contains(&gave), "{gave} of tomatoes nobody sowed gave, for a chance of {chance}");
        let apple = def("base:apple");
        assert!((0..1000).all(|n| unsown_roll(&apple, forage(n))), "tree fruit held back");
    }

    /// Where a broken block's drop pops to is drawn by the world's count of drops popped,
    /// not the tick it breaks on, and each drop counts on.
    #[test]
    fn a_drop_pops_by_the_count_of_drops_not_the_tick() {
        let popped = |tick: u64| {
            let mut server = crate::test_server();
            server.meta.draw_key = 0x0d1a;
            let cells = [2, 3].map(|x| server.spawn.floor().as_ivec3() + IVec3::new(x, 0, 0));
            let stone = server.content.blocks.expect("base:cobblestone");
            for cell in cells {
                server.set_block(cell, stone);
            }
            server.tick = tick;
            server.drop_loot(1, &cells, &cells);
            let mut bodies: Vec<_> = server.items.all.values().map(|d| (d.body.position.to_array(), d.body.velocity.to_array())).collect();
            bodies.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (bodies, server.meta.drops)
        };
        let (first, count) = popped(1_000);
        assert_eq!((first.len(), count), (2, 2), "two drops, counted");
        assert_eq!(popped(1_377).0, first, "the same drops broken later popped otherwise");
        assert_ne!(first[0].1, first[1].1, "two drops popped alike");
    }

    fn item(count: u8, x: f32, age: u32) -> Dropped {
        let body = Loose { position: Vec3::new(x, 0.0, 0.5), velocity: Vec3::ZERO, on_ground: true };
        Dropped { stack: ItemStack { item: BlockId(5), count }, body, age, pickup_in: 0, owner: None, lifetime: lifetime() }
    }

    #[test]
    fn neighbouring_stacks_merge_into_the_larger() {
        let mut items = Items::default();
        items.all.insert(0, item(10, 0.5, 30));
        items.all.insert(1, item(60, 0.8, 5));
        items.all.insert(2, item(3, 4.0, 5));
        merge(&mut items, MERGE_EVERY);
        // 4 of the 10 top the 60 up to a full stack; the far one stays alone.
        assert_eq!(items.all[&1].stack.count, 64);
        assert_eq!(items.all[&0].stack.count, 6);
        assert_eq!(items.all[&2].stack.count, 3);
        // Full stacks take no more.
        merge(&mut items, MERGE_EVERY * 2);
        assert_eq!((items.all[&0].stack.count, items.all[&1].stack.count), (6, 64));
        // A small heap ends as one stack, which lives as long as its youngest part.
        let mut heap = Items::default();
        for (i, x) in [0.5, 0.6, 0.7].into_iter().enumerate() {
            heap.all.insert(i as u32, item(2, x, 100 - i as u32 * 10));
        }
        merge(&mut heap, MERGE_EVERY);
        merge(&mut heap, MERGE_EVERY * 2);
        assert_eq!(heap.all.len(), 1);
        let only = heap.all.values().next().unwrap();
        assert_eq!((only.stack.count, only.age), (6, 80));
        assert_eq!(heap.gone.len(), 2);
    }

    #[test]
    fn saved_items_round_trip_by_name() {
        let mut items = Items::default();
        items.all.insert(0, item(7, 2.5, 200));
        let saved = items.saved(|_| "base:dirt".into());
        let back = Items::restore(saved, |name| (name == "base:dirt").then_some(BlockId(5)));
        let only = back.all.values().next().unwrap();
        assert_eq!((only.stack, only.body.position, only.age), (items.all[&0].stack, Vec3::new(2.5, 0.0, 0.5), 200));
        // Items that no longer exist are left behind.
        assert!(Items::restore(items.saved(|_| "gone:thing".into()), |_| None).all.is_empty());
    }

    #[test]
    fn spreads_differ_by_place_and_salt() {
        let a = random3(1, IVec3::ZERO, 0);
        assert!(a.iter().all(|v| (0.0..1.0).contains(v)));
        assert_ne!(a, random3(1, IVec3::X, 0));
        assert_ne!(a, random3(1, IVec3::ZERO, 1));
    }

    /// Whether a crop nobody sowed gives anything is drawn by the player's count of wild
    /// crops taken, not the tick it is taken on, and each counts on.
    #[test]
    fn an_unsown_crop_gives_by_the_players_count_not_the_tick() {
        let taken = |tick: u64| {
            let mut server = crate::test_server();
            let (client, connection) = voxmar_protocol::local_pair();
            std::mem::forget(client);
            let mut player = crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits);
            (player.joined, player.draw_key) = (true, crate::player::player_draw_key(0x0d1a, "ada"));
            server.players.insert(1, player);
            let (tomato, _) = server.content.crops.iter().find(|(_, c)| c.def.name == "base:tomato").unwrap();
            server.tick = tick;
            let gave: Vec<bool> = (0..40).map(|_| server.unsown_gives(1, tomato)).collect();
            // Tree fruit always gives, and spends no draw.
            let (apple, _) = server.content.crops.iter().find(|(_, c)| c.def.name == "base:apple").unwrap();
            assert!((0..5).all(|_| server.unsown_gives(1, apple)), "tree fruit held back");
            (gave, server.players[&1].draws.forages)
        };
        let (first, count) = taken(1_000);
        assert_eq!(count, 40, "each crop that may give nothing taken counted on, and no fruit");
        assert!(first.contains(&true) && first.contains(&false), "forty tomatoes nobody sowed all gave alike: {first:?}");
        assert_eq!(taken(1_377), (first, count), "the same crops taken later gave otherwise");
    }
}
