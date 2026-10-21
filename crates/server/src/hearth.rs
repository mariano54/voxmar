//! Lighting the early age gates. A hearth becomes a respawn home; a kiln consumes a
//! content-defined fuel charge and smelts only while that fire and its structure hold.
//! The fire uses server ticks and is saved with the world, so reconnecting cannot refuel it.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::multiblock::Multiblock;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

/// A paid fuel charge, by structure name rather than transient block ids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lit {
    pub position: [i32; 3],
    pub structure: String,
    pub until: u64,
    /// Whose it is, by key (`player_key`): a home hearth's kept owner (`hearth_owners`, the one
    /// who first lit it, whoever relit it), whose home it is for fire (fire.ron's
    /// `homes_catch_only_from_owner`); for any other fire, who lit it. None for a fire lit
    /// before this was kept.
    #[serde(default, deserialize_with = "crate::blaze::keyed")]
    pub owner: Option<String>,
}

/// Whose a home hearth is, saved: its core and its owner's key, None for one lit before
/// fires were kept by owner (which keeps every fire out, and stays no one's).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthOwner {
    pub position: [i32; 3],
    #[serde(default, deserialize_with = "crate::blaze::keyed")]
    pub owner: Option<String>,
}

impl HearthOwner {
    /// The saved owners, and for a world from before they were kept, those of the home
    /// hearths still lit (each fire kept its owner).
    pub(crate) fn restore(saved: &[HearthOwner], lit: &FxHashMap<IVec3, Lit>, home: impl Fn(&str) -> bool) -> FxHashMap<IVec3, Option<String>> {
        let mut owners: FxHashMap<IVec3, Option<String>> = lit.iter().filter(|(_, fire)| home(&fire.structure)).map(|(&core, fire)| (core, fire.owner.clone())).collect();
        owners.extend(saved.iter().map(|h| (IVec3::from_array(h.position), h.owner.clone())));
        owners
    }

    pub(crate) fn saved(owners: &FxHashMap<IVec3, Option<String>>) -> Vec<HearthOwner> {
        let mut saved: Vec<HearthOwner> = owners.iter().map(|(core, owner)| HearthOwner { position: core.to_array(), owner: owner.clone() }).collect();
        saved.sort_by_key(|h| h.position);
        saved
    }
}

fn structure_words(structure: &Multiblock) -> Words {
    Words::Own(def_key("multiblock", &structure.name), structure.title.clone())
}

fn fuel_words(structure: &Multiblock, blocks: &voxmar_engine::block::BlockRegistry) -> Words {
    let fuels = structure.fuels.iter().map(|&(item, count)| {
        Words::key("multiblock.fuel_stack").with("count", count.to_string()).with("item", Words::Block(blocks.def(item).name.clone()))
    }).collect();
    Words::key("multiblock.fuel").with("structure", structure_words(structure)).with("fuels", Words::List(fuels))
}

impl Server {
    /// Light a standing core. A cold kiln takes a complete fuel charge from the hand;
    /// using an already burning kiln consumes nothing and earns no further lighting tally.
    pub(crate) fn light(&mut self, agent: PlayerId, at: IVec3) {
        let Some(block) = self.block(at) else { return };
        let multiblocks = self.content.multiblocks.clone();
        // Its core, or any cell of a clamp standing round it (`Requirement::MultiblockCore`).
        let found = multiblocks.with_core(block).find(|m| m.stands(at, |p| self.block(p))).map(|m| (at, m));
        let Some((core, structure)) = found.or_else(|| multiblocks.smoulder_round(at, |p| self.block(p))) else {
            self.station_hint(agent, at);
            return;
        };
        self.tally(agent, "built", &structure.name, 1);
        if structure.smoulder.is_some() {
            return self.light_smoulder(agent, core, structure);
        }
        // What catches by it as it is lit is the lighter's fire, whoever's home it is: lit by a
        // stranger, it never catches their thatch in its owner's home.
        let lighter = self.players.get(&agent).map(|p| crate::player_key(&p.name));
        if !structure.fuels.is_empty() && !self.station_lit(core) {
            let fuel = self.players.get(&agent).and_then(|p| p.inventory.get(p.selected))
                .and_then(|stack| structure.fuels.iter().find(|&&(item, count)| item == stack.item && stack.count >= count).copied());
            let Some((item, count)) = fuel else {
                self.refuse(agent, fuel_words(structure, &self.content.blocks));
                return;
            };
            let seconds = self.content.machines.burn_seconds(item).expect("checked fuel") * count as f32;
            let Some(player) = self.players.get_mut(&agent) else { return };
            player.inventory.take(player.selected, count);
            player.changed(traits::INVENTORY);
            let until = self.tick.saturating_add((seconds * TICKS_PER_SECOND as f32).ceil() as u64);
            let owner = self.hearth_owner(agent, core, structure);
            self.lit.insert(core, Lit { position: core.to_array(), structure: structure.name.clone(), until, owner });
            self.hearth_blaze(core, lighter.clone());
            self.tally(agent, "lit", &structure.name, 1);
        } else if structure.fuels.is_empty() && !self.lit.contains_key(&core) {
            // A fire that needs no fuel burns until the structure breaks; lit again while it
            // burns, it is the same fire (and the same lighting, for quests).
            let owner = self.hearth_owner(agent, core, structure);
            self.lit.insert(core, Lit { position: core.to_array(), structure: structure.name.clone(), until: u64::MAX, owner });
            self.hearth_blaze(core, lighter);
            self.tally(agent, "lit", &structure.name, 1);
        }
        self.send_fires();
        if structure.home && let Some(player) = self.players.get_mut(&agent) {
            player.home = Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        }
        let key = if structure.home { "multiblock.home" } else { "multiblock.lit" };
        self.structure_reply(agent, Words::key(key).with("structure", structure_words(structure)));
    }

    /// Whose a fire `agent` lights at `core` is: a home hearth's is the one it was first lit
    /// by, kept while it is out, broken by others and relit by anyone, until its owner breaks
    /// its hearthstone (`home_changed`), so a stranger who breaks and mends it to relight it
    /// never takes it.
    fn hearth_owner(&mut self, agent: PlayerId, core: IVec3, structure: &Multiblock) -> Option<String> {
        let lighter = self.players.get(&agent).map(|p| crate::player_key(&p.name));
        if !structure.home {
            return lighter;
        }
        if let Some(owner) = self.hearth_owners.get(&core) {
            return owner.clone();
        }
        // One home hearth each: a player's hearth elsewhere is theirs no more, so the kept
        // owners stay one a player (and the old ones no one owns).
        self.hearth_owners.retain(|_, owner| owner.is_none() || *owner != lighter);
        self.hearth_owners.insert(core, lighter.clone());
        lighter
    }

    fn structure_reply(&self, agent: PlayerId, words: Words) {
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
    }

    /// Explain a broken or cold station without activating it as a side effect of cooking.
    pub(crate) fn station_hint(&self, agent: PlayerId, core: IVec3) {
        let Some(block) = self.block(core) else { return };
        let structures = &self.content.multiblocks;
        let standing = structures.with_core(block).find(|m| m.stands(core, |p| self.block(p)));
        // A piece in land not yet loaded never matches: that is no reason to call it unbuilt.
        if standing.is_none() && !self.structure_loaded(core) {
            self.refuse(agent, Words::key("station.unloaded"));
            return;
        }
        let words = match standing {
            // Cold: said where refusals are, with the soft no.
            Some(standing) => {
                let words = Words::key("multiblock.cold").with("structure", structure_words(standing)).with("fuel", fuel_words(standing, &self.content.blocks));
                self.refuse(agent, words);
                return;
            }
            // Not built yet: how to build it is a guide, for the console.
            None => {
                let Some(first) = structures.with_core(block).next() else { return };
                Words::Own(format!("{}.hint", def_key("multiblock", &first.name)), first.hint.clone())
            }
        };
        self.structure_reply(agent, words);
    }

    /// Whether every column a structure with its core at `at` reaches into is loaded.
    pub(crate) fn structure_loaded(&self, at: IVec3) -> bool {
        let reach = self.block(at).and_then(|block| self.content.multiblocks.with_core(block).map(|m| m.reach()).max()).unwrap_or(0);
        self.land_loaded(at, reach)
    }

    /// Whether every column within `reach` blocks of `at` (across, not up) is loaded.
    pub(crate) fn land_loaded(&self, at: IVec3, reach: i32) -> bool {
        (-reach..=reach).all(|x| (-reach..=reach).all(|z| self.world.contains(ColumnPos::of_block(at + IVec3::new(x, 0, z)))))
    }

    /// A station core must stand in its structure; ordinary stations work on their own.
    pub(crate) fn station_stands(&self, at: IVec3) -> bool {
        let Some(block) = self.block(at) else { return false };
        if !self.content.blocks.def(block).has_tag("multiblock_core") {
            return true;
        }
        self.content.multiblocks.with_core(block).any(|m| m.stands(at, |p| self.block(p)))
    }

    pub(crate) fn station_lit(&self, at: IVec3) -> bool {
        let Some(block) = self.block(at) else { return false };
        let structures = &self.content.multiblocks;
        let mut candidates = structures.with_core(block).peekable();
        if candidates.peek().is_none() {
            return true;
        }
        candidates.any(|m| m.stands(at, |p| self.block(p)) && (m.fuels.is_empty()
            || self.lit.get(&at).is_some_and(|fire| fire.structure == m.name && fire.until > self.tick)))
    }

    /// Dismantling the core or a required piece quenches its fire immediately. Putting
    /// the piece back cannot resurrect a paid charge, even between simulation ticks.
    pub(crate) fn fire_changed(&mut self, changed: IVec3) {
        let (structures, world, tick) = (&self.content.multiblocks, &self.world, self.tick);
        // Every block change comes here (snow settling too): a fire whose core is beyond any
        // structure's reach of the change is kept without looking its structure up.
        let reach = structures.reach();
        let before = self.lit.len();
        // A fire whose fuel has run out goes from here as from anywhere; it is told gone cold
        // (`tell_fires_out`) for it all the same.
        let cold = &mut self.fires_out;
        self.lit.retain(|&core, fire| {
            let keep = fire.until > tick && ((changed - core).abs().max_element() > reach || structures.get(&fire.structure)
                .is_some_and(|m| !m.includes(core, changed) || m.stands(core, |p| world.block(p))));
            if !keep && fire.until <= tick {
                cold.push(fire.clone());
            }
            keep
        });
        if self.lit.len() != before {
            self.send_fires();
        }
    }

    /// A bed or home hearth broken (or built over) is no home: each player whose bed or hearth
    /// stood at `pos` forgets it, as the new block there is no bed (no home's core), and is
    /// told. They wake at what is left (the bed, the hearth, else the world's spawn), and the
    /// home mark goes with it.
    pub(crate) fn home_changed(&mut self, pos: IVec3, block: voxmar_engine::block::BlockId) {
        // The hearthstone broken by its owner, the home hearth is no one's any more: whoever
        // builds one there next lights it as theirs. Broken by anyone else (a stranger, a
        // flood, a command) it stays its owner's, and their home stays theirs.
        if let Some(owner) = self.hearth_owners.get(&pos)
            && owner.is_some()
            && *owner == self.changed_by
            && !self.content.multiblocks.with_core(block).any(|m| m.home)
        {
            self.hearth_owners.remove(&pos);
        }
        let at = |spot: Option<Vec3>| spot.is_some_and(|spot| (spot - Vec3::new(0.5, 1.0, 0.5)).round().as_ivec3() == pos);
        // A flood changes thousands of cells a tick: one that is no online player's bed or
        // hearth goes no further than these compares (two a player, no lookups).
        if !self.players.values().any(|p| at(p.bed) || at(p.home)) {
            return;
        }
        let bed = self.content.blocks.def(block).has_tag("bed");
        let hearth = self.content.multiblocks.with_core(block).any(|m| m.home);
        for player in self.players.values_mut() {
            let (bed_lost, hearth_lost) = (!bed && at(player.bed), !hearth && at(player.home));
            if bed_lost {
                player.bed = None;
                player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("bed.lost") });
            }
            if hearth_lost {
                player.home = None;
                player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("home.lost") });
            }
            if bed_lost || hearth_lost {
                player.vitals.home = player.home_block(&self.lit);
                player.changed(traits::VITALS);
                // What the old home met is nothing to the new one: noted afresh there.
                (player.home_steps, player.home_anchor) = (Vec::new(), None);
            }
        }
    }

    /// The cores of every fire burning, in order.
    pub(crate) fn fires(&self) -> Vec<IVec3> {
        let mut fires: Vec<IVec3> = self.lit.iter().filter(|(_, fire)| fire.until > self.tick).map(|(&core, _)| core).collect();
        fires.sort_by_key(|core| core.to_array());
        fires
    }

    /// Tells every player which fires burn, when that changed since they were last told.
    pub(crate) fn send_fires(&mut self) {
        let fires = self.fires();
        if fires != self.fires_sent {
            self.broadcast(ServerMessage::Fires { lit: fires.clone() });
            self.fires_sent = fires;
        }
    }

    /// A home hearth lit before hearths kept their fire (a player saved earlier) is lit again
    /// once its player is back and the land round it is loaded, if it still stands; never
    /// again after, so a hearth broken, rebuilt and left cold stays cold. Tried as they
    /// join and every second after until the hearth's land is there to look at.
    pub(crate) fn kindle_home(&mut self, id: PlayerId) {
        let Some(player) = self.players.get(&id).filter(|p| !p.kindled) else { return };
        let Some(home) = player.home else {
            self.players.get_mut(&id).unwrap().kindled = true;
            return;
        };
        let core = (home - Vec3::new(0.5, 1.0, 0.5)).round().as_ivec3();
        let Some(block) = self.block(core) else { return };
        let unloaded = std::cell::Cell::new(false);
        let look = |p: IVec3| {
            let block = self.block(p);
            unloaded.set(unloaded.get() || block.is_none());
            block
        };
        let multiblocks = self.content.multiblocks.clone();
        let standing = multiblocks.with_core(block).find(|m| m.home && m.fuels.is_empty() && m.stands(core, look)).map(|m| m.name.clone());
        if standing.is_none() && unloaded.get() {
            return;
        }
        self.players.get_mut(&id).unwrap().kindled = true;
        if let Some(structure) = standing.filter(|_| !self.lit.contains_key(&core)) {
            // Relit, it is whose it was, or no one's: whoever's home is at it now need not be
            // who built it, and an unowned home hearth keeps every fire out.
            let owner = self.hearth_owners.entry(core).or_insert(None).clone();
            let owned = owner.is_some();
            self.lit.insert(core, Lit { position: core.to_array(), structure, until: u64::MAX, owner });
            // Relit for them, not by them: nothing they changed starts a fire, but they are
            // told, once, what by it would catch if it did. One no one owns catches nothing.
            if owned {
                self.hearth_hint(id, core);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::block::BlockId;
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::inventory::ItemStack;
    use crate::player::Player;
    use crate::save::Draw;

    fn player(server: &mut Server) {
        let (_, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
    }

    #[test]
    fn kiln_fire_expires_at_its_paid_tick_and_relighting_takes_another_charge() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let coal = server.content.blocks.expect("base:coal_ore");
        server.set_block(core, server.content.blocks.expect("base:crucible"));
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(coal, 24));
        server.light(1, core);
        assert!(!server.lit.contains_key(&core), "a loose core was lit");
        assert_eq!(server.players[&1].inventory.get(0).unwrap().count, 24, "a loose core consumed fuel");
        let brick = server.content.blocks.expect("base:kiln_bricks");
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), brick);
        }}
        for x in [-1, 1] { for z in [-1, 1] { server.set_block(core + IVec3::new(x, 1, z), brick); }}
        server.set_block(core, server.content.blocks.expect("base:crucible"));
        server.light(1, core);
        let until = server.lit[&core].until;
        assert_eq!(until - server.tick, (24.0 * server.content.machines.burn_seconds(coal).unwrap() * TICKS_PER_SECOND as f32) as u64);
        server.players.get_mut(&1).unwrap().position = core.as_vec3() + Vec3::new(0.5, 0.0, 3.5);
        let burns = (until - server.tick).div_ceil(TICKS_PER_SECOND as u64) as u32;
        assert_eq!(server.station_seen(1, core), Some((None, Some(burns))), "a look says how long the fire burns");
        server.tick = until - 1;
        assert!(server.station_lit(core));
        server.tick = until;
        assert!(!server.station_lit(core), "the fire outlived its charge");
        assert_eq!(server.station_seen(1, core), Some((None, Some(0))), "a look says the kiln is cold");
        server.light(1, core);
        assert!(!server.station_lit(core), "an empty hand relit an expired fire");
        assert_eq!(server.players[&1].tallies["lit:base:crucible_kiln"], 1);
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(coal, 24));
        server.light(1, core);
        assert!(server.station_lit(core));
        assert_eq!(server.players[&1].tallies["lit:base:crucible_kiln"], 2);
        server.set_block(core, BlockId::AIR);
        assert!(!server.lit.contains_key(&core));
    }

    /// A bed or home hearth broken leaves no ghost home: the player forgets it, is told, and
    /// wakes at what is left. Another bed set down where the bed was keeps it.
    #[test]
    fn a_broken_bed_or_hearth_is_forgotten() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let (stone, chimney) = (server.content.blocks.expect("base:cobblestone"), server.content.blocks.expect("base:chimney"));
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), stone);
        }}
        server.set_block(core, server.content.blocks.expect("base:hearthstone"));
        for y in 1..=2 { server.set_block(core + IVec3::new(-1, y, -1), chimney); }
        let (bag, oak_bed) = (server.content.blocks.expect("base:sleeping_bag"), server.content.blocks.expect("base:oak_bed"));
        let bed = core + IVec3::new(5, 1, 0);
        server.set_block(bed, bag);
        server.players.get_mut(&1).unwrap().bed = Some(bed.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        server.light(1, core);
        let home = |server: &Server| server.players[&1].home_block(&server.lit);
        assert_eq!(home(&server), Some(core), "the lit hearth is home");
        // A bed swapped for another bed is still the bed.
        server.set_block(bed, oak_bed);
        assert!(server.players[&1].bed.is_some(), "a bed set where the bag was is still slept in");
        // The hearthstone broken: no home there, the bed is.
        server.set_block(core, BlockId::AIR);
        assert_eq!(server.players[&1].home, None, "a broken hearth is forgotten");
        assert_eq!(home(&server), Some(bed), "home is the bed");
        assert_eq!(server.players[&1].vitals.home, Some(bed), "and the mark says so at once");
        // The bed broken: nothing left, they wake at the world's spawn.
        server.set_block(bed, BlockId::AIR);
        assert_eq!(server.players[&1].bed, None, "a broken bed is forgotten");
        assert_eq!(home(&server), None, "no home left");
        assert_eq!(server.players[&1].vitals.home, None);
    }

    /// A flood that washes the bed away leaves no ghost home either.
    #[test]
    fn a_bed_washed_away_is_forgotten() {
        let mut server = crate::test_server();
        player(&mut server);
        let floor = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let stone = server.content.blocks.expect("base:cobblestone");
        for x in -3..=3 { for z in -3..=3 {
            server.set_block(floor + IVec3::new(x, 0, z), stone);
        }}
        // The bag between the water and the floor's nearer edge, so it lies on the water's
        // way down (water runs only toward the nearest drop).
        let bed = floor + IVec3::new(2, 1, 0);
        server.set_block(bed, server.content.blocks.expect("base:sleeping_bag"));
        server.players.get_mut(&1).unwrap().bed = Some(bed.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        server.set_block(bed - IVec3::X, server.content.blocks.expect("base:water"));
        for _ in 0..200 {
            server.tick += 1;
            server.flow();
            if server.players[&1].bed.is_none() {
                break;
            }
        }
        assert_ne!(server.block(bed), server.content.blocks.id("base:sleeping_bag"), "the water took the bag");
        assert_eq!(server.players[&1].bed, None, "and the bed is forgotten");
    }

    /// Home is the hearth lit as home while it burns, else the last bed: a hearth broken
    /// sends its player home to their bed, and lit again it is home again.
    #[test]
    fn home_is_the_burning_hearth_else_the_last_bed() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let (stone, chimney) = (server.content.blocks.expect("base:cobblestone"), server.content.blocks.expect("base:chimney"));
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), stone);
        }}
        server.set_block(core, server.content.blocks.expect("base:hearthstone"));
        for y in 1..=2 { server.set_block(core + IVec3::new(-1, y, -1), chimney); }
        let bed = core + IVec3::new(5, 0, 0);
        server.players.get_mut(&1).unwrap().bed = Some(bed.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
        let home = |server: &Server| server.players[&1].home_block(&server.lit);
        assert_eq!(home(&server), Some(bed), "with no hearth lit, home is the bed");
        server.light(1, core);
        assert_eq!(home(&server), Some(core), "the lit hearth is home");
        server.set_block(core + IVec3::new(1, 0, 1), BlockId::AIR);
        assert_eq!(home(&server), Some(bed), "a broken hearth is no home");
        // Respawn, home effects and stores and wanderers all agree: the bed.
        let waking = server.players[&1].home_spot(&server.lit).unwrap();
        server.players.get_mut(&1).unwrap().position = waking;
        assert_eq!(server.at_home(1), Some(bed + IVec3::Y), "home effects stay at the broken hearth");
        assert_eq!(server.home_holding(1, waking), Some(bed + IVec3::Y), "home stores stay at the broken hearth");
        server.set_block(core + IVec3::new(1, 0, 1), stone);
        server.light(1, core);
        assert_eq!(home(&server), Some(core), "the hearth lit again is home again");
        // A save from before hearths kept their fire: its hearth is home until it is lit
        // again for them, even while its land is not loaded yet.
        server.set_block(core + IVec3::new(1, 0, 1), BlockId::AIR);
        server.set_block(core + IVec3::new(1, 0, 1), stone);
        server.players.get_mut(&1).unwrap().kindled = false;
        assert_eq!(home(&server), Some(core), "an old save's hearth is not home before it is kindled");
    }

    /// A hearth is cold until lit: then it burns, and players are told, until a piece of it
    /// is taken away, and players are told it went out. A home hearth from a world saved
    /// before hearths kept their fire is lit again when its player joins and its land is loaded, once.
    #[test]
    fn a_hearth_burns_from_lighting_until_it_breaks_and_players_are_told() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
        server.players.get_mut(&1).unwrap().joined = true;
        let told = |client: &voxmar_protocol::ClientConnection| -> Vec<Vec<IVec3>> {
            std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Fires { lit } => Some(lit), _ => None }).collect()
        };
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let (stone, chimney) = (server.content.blocks.expect("base:cobblestone"), server.content.blocks.expect("base:chimney"));
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), stone);
        }}
        server.set_block(core, server.content.blocks.expect("base:hearthstone"));
        for y in 1..=2 { server.set_block(core + IVec3::new(-1, y, -1), chimney); }
        server.send_fires();
        assert!(server.fires().is_empty() && told(&client).is_empty(), "a hearth burned before it was lit");
        server.light(1, core);
        assert_eq!(server.fires(), [core], "the hearth was not lit");
        assert_eq!(told(&client), [vec![core]]);
        server.light(1, core);
        let lightings: u32 = server.players[&1].tallies.iter().filter(|(name, _)| name.starts_with("lit:")).map(|(_, &n)| n).sum();
        assert_eq!(lightings, 1, "lighting a burning hearth again counted as another lighting");
        assert!(told(&client).is_empty());
        server.tick += 1_000_000;
        server.send_fires();
        assert_eq!(server.fires(), [core], "a hearth burned out");
        assert!(told(&client).is_empty(), "players were told again of what had not changed");
        server.set_block(core + IVec3::new(1, 0, 1), BlockId::AIR);
        assert!(server.fires().is_empty(), "a broken hearth still burns");
        assert_eq!(told(&client), [Vec::<IVec3>::new()]);
        // An older world's home hearth: standing, the player's home, but never kept lit.
        server.set_block(core + IVec3::new(1, 0, 1), stone);
        assert!(server.fires().is_empty(), "mending a hearth lit it");
        let player = server.players.get_mut(&1).unwrap();
        (player.home, player.kindled) = (Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5)), false);
        // Its land not loaded yet (as when they join far from spawn): tried again later.
        let column = ColumnPos::of_block(core);
        let held = server.world.remove(column).expect("the hearth's column is loaded");
        server.kindle_home(1);
        assert!(server.fires().is_empty() && !server.players[&1].kindled, "gave up on a hearth not yet loaded");
        server.world.insert(column, held);
        server.kindle_home(1);
        assert_eq!(server.fires(), [core], "the old home hearth was not lit again");
        // Once only: broken, mended and left cold, it stays cold when they join again.
        server.set_block(core + IVec3::new(1, 0, 1), BlockId::AIR);
        server.set_block(core + IVec3::new(1, 0, 1), stone);
        server.kindle_home(1);
        assert!(server.fires().is_empty(), "joining again lit a hearth left cold");
    }

    /// A kiln standing across a column border, the far column still loading: lighting it or
    /// cooking at it is refused with a line saying the land is loading, not a guide to
    /// building it, and takes nothing; loaded, it lights.
    #[test]
    fn a_kiln_on_land_still_loading_says_so_and_takes_nothing() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
        // On the column's east edge: its east bricks stand in the next column.
        let column = ColumnPos::of_block(server.spawn.floor().as_ivec3());
        let core = column.origin() + IVec3::new(31, server.spawn.y as i32 + 4, 8);
        server.load_area(core - IVec3::splat(2), core + IVec3::splat(2));
        let (brick, crucible, coal) = (server.content.blocks.expect("base:kiln_bricks"), server.content.blocks.expect("base:crucible"), server.content.blocks.expect("base:coal_ore"));
        for x in -1..=1 { for z in -1..=1 {
            server.set_block(core + IVec3::new(x, 0, z), brick);
        }}
        for x in [-1, 1] { for z in [-1, 1] { server.set_block(core + IVec3::new(x, 1, z), brick); }}
        server.set_block(core, crucible);
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(coal, 24));
        let east = column.offset(1, 0);
        let held = server.world.remove(east).expect("loaded");
        server.light(1, core);
        server.cook(1, core);
        // Every refusal and chat line, a refusal's words first.
        let told: Vec<(bool, Words)> = std::iter::from_fn(|| client.try_recv().ok().flatten())
            .filter_map(|m| match m { ServerMessage::Refused { why } => Some((true, why)), ServerMessage::Chat { text, .. } => Some((false, text)), _ => None })
            .collect();
        let unloaded = (true, Words::key("station.unloaded"));
        assert_eq!(told, [unloaded.clone(), unloaded], "not told the land is loading");
        assert!(!server.lit.contains_key(&core) && server.players[&1].inventory.get(0).unwrap().count == 24, "lit or took fuel on land still loading");
        server.world.insert(east, held);
        server.light(1, core);
        assert!(server.lit.contains_key(&core), "not lit once loaded");
    }

    /// A structure that stands is told as standing, though land within its reach on a side
    /// it does not use is still loading.
    #[test]
    fn a_standing_structure_by_land_still_loading_is_told_cold_not_loading() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
        // A mod's hearth one block wide, built along the column's east edge.
        let source = r#"(multiblocks: [(name: "edge_hearth", title: "Edge Hearth", hint: "A stone each side.", layers: [["s", "H", "s"]], key: {'s': "tag:stone", 'H': "hearthstone"}, core: 'H')])"#;
        let file = voxmar_engine::multiblock::MultiblockRegistry::read("base", source).expect("reads");
        server.content.multiblocks = std::sync::Arc::new(voxmar_engine::multiblock::MultiblockRegistry::new(vec![file], &server.content.blocks).expect("resolves"));
        let column = ColumnPos::of_block(server.spawn.floor().as_ivec3());
        let core = column.origin() + IVec3::new(31, server.spawn.y as i32 + 4, 8);
        server.load_area(core - IVec3::splat(2), core + IVec3::splat(2));
        let stone = server.content.blocks.expect("base:cobblestone");
        server.set_block(core + IVec3::Z, stone);
        server.set_block(core - IVec3::Z, stone);
        server.set_block(core, server.content.blocks.expect("base:hearthstone"));
        server.world.remove(column.offset(1, 0)).expect("loaded");
        assert!(server.station_stands(core) && !server.structure_loaded(core), "the case is a standing structure short of loaded land");
        server.station_hint(1, core);
        let told: Vec<Words> = std::iter::from_fn(|| client.try_recv().ok().flatten()).filter_map(|m| match m { ServerMessage::Refused { why } => Some(why), _ => None }).collect();
        assert!(matches!(told.as_slice(), [Words::Key(key, _)] if key == "multiblock.cold"), "told {told:?}");
    }

    /// A standing Homestead Hearth and a Cottage Stove cook what a campfire cooks, and the
    /// hearth lights a firebrand or a torch's stub as any fire does; a hearthstone with no
    /// ring round it does not, and says what it lacks.
    #[test]
    fn the_hearth_and_the_stove_cook_as_a_campfire_does() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let stove = core + IVec3::new(4, 0, 0);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (raw, cooked) = (block(&server, "base:raw_small_game"), block(&server, "base:cooked_small_game"));
        server.set_block(core, block(&server, "base:hearthstone"));
        server.set_block(stove, block(&server, "base:cottage_stove"));
        // A count of goes cooked from which the next two never burn at level 1.
        let burn = server.content.cooking.for_item(raw, &["campfire".into()]).unwrap().burn_chance(1);
        let key = server.players[&1].draw_key;
        let cooks = (0..1000).find(|&n| (n..n + 2).all(|n| crate::save::numbered_draws(key, Draw::Cook.salt(), n)[0] >= burn)).unwrap();
        server.players.get_mut(&1).unwrap().draws.cooks = cooks;
        let cook = |server: &mut Server, at: IVec3| {
            server.players.get_mut(&1).unwrap().inventory.slots.iter_mut().for_each(|slot| *slot = None);
            server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(raw, 1));
            server.cook(1, at);
            server.players[&1].inventory.get(0).map(|s| s.item)
        };
        assert_eq!(cook(&mut server, core), Some(raw), "a loose hearthstone cooked");
        let stone = block(&server, "base:cobblestone");
        for dz in -1..=1 {
            for dx in -1..=1 {
                if (dx, dz) != (0, 0) {
                    server.set_block(core + IVec3::new(dx, 0, dz), stone);
                }
            }
        }
        for dy in 1..=2 {
            server.set_block(core + IVec3::new(-1, dy, -1), block(&server, "base:chimney"));
        }
        assert!(server.station_stands(core));
        assert_eq!(cook(&mut server, core), Some(cooked), "the standing hearth cooked nothing");
        assert_eq!(cook(&mut server, stove), Some(cooked), "the stove cooked nothing");
        assert_eq!(server.players[&1].tallies["cooked_at:campfire:base:cooked_small_game"], 2, "counted as campfire cooking, for First Night");
        // A stick held in it catches, and a torch's stub lights again, as at any fire.
        for (held, lit) in [("base:stick", "base:firebrand"), ("base:torch_stub", "base:torch")] {
            server.players.get_mut(&1).unwrap().inventory.slots.iter_mut().for_each(|slot| *slot = None);
            server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(block(&server, held), 1));
            server.cook(1, core);
            assert_eq!(server.players[&1].inventory.get(0).map(|s| s.item), Some(block(&server, lit)), "{held} at the hearth");
        }
        // The stove is a fire: it warms, and holds a pot.
        let def = server.content.blocks.def(block(&server, "base:cottage_stove"));
        assert!(def.has_tag("hob") && def.has_tag("heat"));
        assert!(server.content.blocks.info(block(&server, "base:cottage_stove")).emission >= server.survival.heat_emission);
    }

    /// Bronze is three copper ingots held and a tin ore carried, four at a time, in one
    /// step: short of the tin, nothing is taken and the player is told what it takes.
    #[test]
    fn the_crucible_makes_four_bronze_from_three_copper_ingots_and_a_tin_ore() {
        let mut server = crate::test_server();
        player(&mut server);
        let core = ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 4, 8);
        let blocks = server.content.blocks.clone();
        let brick = blocks.expect("base:kiln_bricks");
        for x in -1..=1 { for z in -1..=1 { server.set_block(core + IVec3::new(x, 0, z), brick); }}
        for x in [-1, 1] { for z in [-1, 1] { server.set_block(core + IVec3::new(x, 1, z), brick); }}
        server.set_block(core, blocks.expect("base:crucible"));
        let (coal, copper, tin, bronze) = (blocks.expect("base:coal_ore"), blocks.expect("base:copper_ingot"), blocks.expect("base:tin_ore"), blocks.expect("base:bronze_ingot"));
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(coal, 24));
        server.light(1, core);
        assert!(server.station_lit(core));
        let engineering = server.content.skills.id("core:engineering").unwrap();
        let player = server.players.get_mut(&1).unwrap();
        player.skills.grant(engineering, server.content.skills.xp_for(12));
        player.inventory.slots[0] = Some(ItemStack::new(copper, 4));
        player.selected = 0;
        server.cook(1, core);
        assert_eq!(server.players[&1].inventory.count(copper), 4, "no tin, nothing taken");
        server.players.get_mut(&1).unwrap().inventory.slots[5] = Some(ItemStack::new(tin, 1));
        let before = server.players[&1].skills.xp(engineering);
        server.cook(1, core);
        let player = &server.players[&1];
        assert_eq!((player.inventory.count(copper), player.inventory.count(tin), player.inventory.count(bronze)), (1, 0, 4));
        assert_eq!(player.tallies["cooked:base:bronze_ingot"], 4, "First Bronze counts all four");
        assert!(player.skills.xp(engineering) > before);
    }

    #[test]
    fn a_burnt_meal_does_not_count_as_successful_campfire_food() {
        let mut server = crate::test_server();
        player(&mut server);
        let fire = server.spawn.floor().as_ivec3() + IVec3::Y * 4;
        server.set_block(fire, server.content.blocks.expect("base:campfire"));
        let raw = server.content.blocks.expect("base:raw_minnow");
        let recipe = server.content.cooking.for_item(raw, &["campfire".into()]).unwrap();
        let burn = recipe.burn_chance(1);
        let burnt = recipe.burnt.unwrap();
        let key = server.players[&1].draw_key;
        let cooks = (0..1000).find(|&n| crate::save::numbered_draws(key, Draw::Cook.salt(), n)[0] < burn).expect("a real burnt roll");
        server.players.get_mut(&1).unwrap().draws.cooks = cooks;
        server.players.get_mut(&1).unwrap().inventory.slots[0] = Some(ItemStack::new(raw, 1));
        server.cook(1, fire);
        assert_eq!(server.players[&1].inventory.get(0).unwrap().item, burnt);
        assert!(!server.players[&1].tallies.keys().any(|key| key.starts_with("cooked")), "a burnt output counted");
    }

    /// A full pack fires its clay as long as the fired clay has room: a crack gives back
    /// the clay into the stack it came from.
    #[test]
    fn a_full_pack_fires_clay_whether_it_cracks_or_not() {
        let mut server = crate::test_server();
        player(&mut server);
        let kiln = server.spawn.floor().as_ivec3() + IVec3::Y * 4;
        server.set_block(kiln, server.content.blocks.expect("base:pit_kiln"));
        let (clay, fired, stone) = (server.content.blocks.expect("base:clay"), server.content.blocks.expect("base:fired_clay"), server.content.blocks.expect("base:cobblestone"));
        let recipe = server.content.cooking.for_item(clay, &server.content.blocks.def(server.content.blocks.expect("base:pit_kiln")).tags).unwrap();
        let burn = recipe.burn_chance(1);
        assert!(burn > 0.0 && recipe.burnt == Some(clay), "clay cracks back to clay at level 1");
        for cracks in [true, false] {
            let key = server.players[&1].draw_key;
            let cooks = (0..1000).find(|&n| (crate::save::numbered_draws(key, Draw::Cook.salt(), n)[0] < burn) == cracks).unwrap();
            server.players.get_mut(&1).unwrap().draws.cooks = cooks;
            let inventory = &mut server.players.get_mut(&1).unwrap().inventory;
            for slot in inventory.slots.iter_mut() {
                *slot = Some(ItemStack::new(stone, 64));
            }
            inventory.slots[0] = Some(ItemStack::new(clay, 64));
            inventory.slots[1] = Some(ItemStack::new(fired, 10));
            server.players.get_mut(&1).unwrap().selected = 0;
            server.cook(1, kiln);
            let inventory = &server.players[&1].inventory;
            let (clay_left, fired_now) = (inventory.get(0).unwrap().count, inventory.get(1).unwrap().count);
            assert_eq!((clay_left, fired_now), if cracks { (64, 10) } else { (63, 11) }, "cracks: {cracks}");
        }
    }
}
