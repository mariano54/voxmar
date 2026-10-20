//! Feed troughs (docs/home.md, "A trough for the pen"): a store (`Bin`, machines.ron) tagged
//! `trough`. A kept animal that grazes (`Graze` behaviour), gone hungry (unfed past its `fed`
//! days, `creatures.ron`'s condition) with nothing to graze near, walks to a trough within its
//! leash of home that holds one of its foods and eats one, head down, as it grazes: what it
//! grazes, its favourites, what tames it or readies it, or any `breeding_feed`. So a keeper
//! fills the trough and the pen is fed while they are away, an item an animal a day, after the
//! grass it strips is gone. Hunters (a kept fox) still take their meals by hand.

use voxmar_engine::block::BlockId;
use voxmar_engine::creature::condition::Met;
use voxmar_engine::creature::husbandry::KEPT_LEASH;
use voxmar_engine::glam::IVec3;

use crate::Server;

/// The tag that makes a store a trough.
const TROUGH: &str = "trough";

impl Server {
    /// The trough a kept, hungry creature `id` would eat from: the nearest to it within its
    /// leash of home (as the walk home reckons it) holding one of its foods.
    pub(crate) fn trough_for(&self, id: u32) -> Option<IVec3> {
        let creature = self.creatures.live.get(&id)?;
        let rules = self.content.creatures.condition();
        if creature.owner.is_none() || rules.fed(self.days_since(creature.fed)) == Met::Yes {
            return None;
        }
        let (home, at) = (creature.home.floor().as_ivec3(), creature.body.position);
        // Within its leash as the walk home reckons it (across, as the crow flies), a block
        // short of it, where it stands to eat: a trough in a square pen's far corner is past
        // its leash, and it would turn home before reaching it.
        let leash = self.content.creatures.get(creature.species).def.leash.min(KEPT_LEASH);
        let within = |pos: IVec3| (pos.as_vec3() + 0.5 - creature.home).with_y(0.0).length() <= leash - 1.0;
        let reach = leash as i32;
        let mut best: Option<(f32, IVec3)> = None;
        for dy in -3..=1 {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    let pos = home + IVec3::new(dx, dy, dz);
                    if within(pos) && self.trough_food(id, pos).is_some() {
                        let distance = (pos.as_vec3() + 0.5).distance(at);
                        if best.is_none_or(|(d, _)| distance < d) {
                            best = Some((distance, pos));
                        }
                    }
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// One of creature `id`'s foods in the trough at `pos`, if it is a trough and has one.
    fn trough_food(&self, id: u32, pos: IVec3) -> Option<(usize, BlockId)> {
        let (block, state) = self.factory.get(pos)?;
        if !self.content.blocks.def(block).has_tag(TROUGH) {
            return None;
        }
        let species = self.content.creatures.get(self.creatures.live.get(&id)?.species);
        let eats = |item: BlockId| {
            [&species.eats, &species.favourite, &species.tame_food, &species.breed_food].iter().any(|foods| foods.contains(&item))
                || self.content.blocks.def(item).has_tag("breeding_feed")
        };
        state.bin.iter().enumerate().find_map(|(slot, stack)| stack.as_ref().filter(|s| s.count > 0 && eats(s.item)).map(|s| (slot, s.item)))
    }

    /// Creature `id` eats one of its foods from the trough at `pos`: fed, if there was one.
    pub(crate) fn eat_from_trough(&mut self, id: u32, pos: IVec3) -> bool {
        let Some((slot, _)) = self.trough_food(id, pos) else { return false };
        let Some(state) = self.factory.state_mut(pos) else { return false };
        let stack = state.bin[slot].as_mut().expect("found there");
        stack.count -= 1;
        if stack.count == 0 {
            state.bin[slot] = None;
        }
        let day = self.day;
        if let Some(creature) = self.creatures.live.get_mut(&id) {
            creature.fed = day;
            creature.needs.food = Met::Yes;
        }
        true
    }

    /// Whether the block at `pos` is a trough.
    pub(crate) fn is_trough(&self, pos: IVec3) -> bool {
        self.block(pos).is_some_and(|b| self.content.blocks.def(b).has_tag(TROUGH))
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::creature::condition::Met;
    use voxmar_engine::glam::{IVec3, Vec3};
    use voxmar_engine::inventory::ItemStack;
    use voxmar_protocol::TICKS_PER_DAY;

    use crate::Server;

    /// A grown fox (it never grazes) kept by "test" at spawn, three days unfed, and a trough
    /// two blocks off with what is given.
    fn pen(items: &[(&str, u8)]) -> (Server, u32, IVec3) {
        let mut server = crate::test_server();
        let at = server.spawn.floor().as_ivec3();
        server.load_area(at, at);
        let species = server.content.creatures.id("base:fox").expect("foxes");
        let id = server.spawn_creature(species, server.spawn, 1).expect("room");
        let day = server.day;
        let creature = server.creatures.live.get_mut(&id).unwrap();
        creature.owner = Some("test".into());
        creature.home = server.spawn;
        creature.fed = day - 3.0 * TICKS_PER_DAY as f64;
        let trough = at + IVec3::new(2, 0, 0);
        let block = server.content.blocks.expect("base:feed_trough");
        server.set_block(trough, block);
        let machines = server.content.machines.clone();
        if server.factory.get(trough).is_none() {
            server.factory.insert(trough, block, &machines);
        }
        for &(name, count) in items {
            let item = server.content.blocks.expect(name);
            assert_eq!(server.factory.put(trough, ItemStack::new(item, count), &machines), None);
        }
        (server, id, trough)
    }

    fn left(server: &Server, trough: IVec3) -> u32 {
        server.factory.get(trough).map_or(0, |(_, state)| state.bin.iter().flatten().map(|s| s.count as u32).sum())
    }

    #[test]
    fn a_hungry_kept_animal_eats_its_food_from_a_trough() {
        // Wheat is no fox's food: the trough is passed by.
        let (server, fox, _) = pen(&[("base:wheat", 4)]);
        assert_eq!(server.trough_for(fox), None, "a fox went to wheat");
        // Its meat it eats, one, and is fed.
        let (mut server, fox, trough) = pen(&[("base:wheat", 4), ("base:raw_small_game", 2)]);
        assert_eq!(server.needs_of(fox).food, Met::No);
        assert_eq!(server.trough_for(fox), Some(trough));
        assert!(server.eat_from_trough(fox, trough));
        assert_eq!(left(&server, trough), 5, "one eaten");
        assert_eq!(server.needs_of(fox).food, Met::Yes);
        // Fed, it leaves the trough alone for a day.
        assert_eq!(server.trough_for(fox), None);
        // A wild one never eats from it, nor one whose home is beyond its leash of it.
        let creature = server.creatures.live.get_mut(&fox).unwrap();
        creature.fed -= 3.0 * TICKS_PER_DAY as f64;
        creature.owner = None;
        assert_eq!(server.trough_for(fox), None, "a wild fox ate from a trough");
        let creature = server.creatures.live.get_mut(&fox).unwrap();
        creature.owner = Some("test".into());
        creature.home += Vec3::new(20.0, 0.0, 0.0);
        assert_eq!(server.trough_for(fox), None, "a fox ate from a trough far from its home");
    }

    /// Through the brain: a hungry kept rabbit with nothing to graze near walks over to its
    /// trough and eats a carrot from it, head down, as it would graze.
    /// A trough counts within the leash as the walk home reckons it, across as the crow
    /// flies: one in a square pen's far corner (six and six off) is past it, one five
    /// straight off is not.
    #[test]
    fn a_trough_in_the_pens_far_corner_is_past_the_leash() {
        let (mut server, fox, trough) = pen(&[("base:raw_small_game", 2)]);
        let middle = trough.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        server.creatures.live.get_mut(&fox).unwrap().home = middle - Vec3::new(6.0, 0.0, 6.0);
        assert_eq!(server.trough_for(fox), None, "a trough past the leash's reach was chosen");
        server.creatures.live.get_mut(&fox).unwrap().home = middle - Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(server.trough_for(fox), Some(trough));
    }

    #[test]
    fn a_kept_grazer_left_alone_feeds_itself_at_the_trough() {
        let (mut server, _, trough) = pen(&[]);
        let species = server.content.creatures.id("base:rabbit").expect("rabbits");
        let eats = server.content.creatures.get(species).eats.clone();
        let rabbit = server.spawn_creature(species, server.spawn, 2).expect("room");
        let day = server.day;
        let creature = server.creatures.live.get_mut(&rabbit).unwrap();
        creature.owner = Some("test".into());
        creature.home = server.spawn;
        creature.fed = day - 3.0 * TICKS_PER_DAY as f64;
        // Its grass grazed off all about.
        let at = server.spawn.floor().as_ivec3();
        for dy in -2..=2 {
            for dz in -12..=12 {
                for dx in -12..=12 {
                    let cell = at + IVec3::new(dx, dy, dz);
                    if server.block(cell).is_some_and(|b| eats.contains(&b)) {
                        server.set_block(cell, voxmar_engine::block::BlockId::AIR);
                    }
                }
            }
        }
        let carrot = server.content.blocks.expect("base:carrot");
        let machines = server.content.machines.clone();
        assert_eq!(server.factory.put(trough, ItemStack::new(carrot, 3), &machines), None);
        for _ in 0..TICKS_PER_DAY / 4 {
            server.tick_creatures();
            server.tick += 1;
            if left(&server, trough) < 3 {
                break;
            }
        }
        assert_eq!(left(&server, trough), 2, "the rabbit did not eat at its trough");
        assert_eq!(server.needs_of(rabbit).food, Met::Yes);
    }
}
