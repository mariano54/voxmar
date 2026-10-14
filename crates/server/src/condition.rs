//! Kept animals' condition on the server (docs/breeding.md, "Keeping animals"): every few
//! seconds each kept animal in loaded land looks at its needs (food, water, shelter, room,
//! climate, company) and its condition walks toward what they allow. The rules are the
//! engine's (`creature::condition`) and the content's numbers.

use voxmar_engine::coords::ColumnPos;
use voxmar_engine::creature::condition::{self, Met, Needs};
use voxmar_engine::creature::husbandry::KEPT_LEASH;
use voxmar_engine::creature::{self, CreatureId};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_protocol::{TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::Server;

/// Ticks between looks at each kept animal's needs. The animals are looked at a share at a
/// time, every husbandry pass, so a big ranch never costs one tick much.
pub(crate) const RECKON_EVERY: u64 = 10 * TICKS_PER_SECOND as u64;

impl Server {
    /// In-game days from day-clock `since` to now.
    pub(crate) fn days_since(&self, since: f64) -> f32 {
        ((self.day - since) / TICKS_PER_DAY as f64) as f32
    }

    /// Looks at every kept animal's needs and moves its condition toward them.
    #[cfg(test)]
    pub(crate) fn reckon_condition(&mut self) {
        self.reckon_share(1, 0);
    }

    /// Looks at the kept animals whose id falls in share `which` of `shares`, and moves each
    /// one's condition toward what its needs allow, by the days since it was last looked at.
    /// An animal standing, or kept, in land that is not loaded is paused as its land is: its
    /// clocks move on with the day, so it neither hungers nor changes while nobody is near.
    pub(crate) fn reckon_share(&mut self, shares: u64, which: u64) {
        let rules = *self.content.creatures.condition();
        let day = self.day;
        let ids: Vec<u32> = self.creatures.live.iter().filter(|&(&id, c)| c.owner.is_some() && id as u64 % shares == which).map(|(&id, _)| id).collect();
        for id in ids {
            let creature = &self.creatures.live[&id];
            let loaded = |at: Vec3| self.world.contains(ColumnPos::of_block(at.floor().as_ivec3()));
            if !loaded(creature.body.position) || !loaded(creature.home) {
                let creature = self.creatures.live.get_mut(&id).expect("listed");
                creature.fed += day - creature.reckoned;
                creature.reckoned = day;
                continue;
            }
            let needs = self.needs_of(id);
            let creature = &self.creatures.live[&id];
            let def = &self.content.creatures.get(creature.species).def;
            let hurt = (creature.health as u32) * 2 < def.health as u32;
            let target = rules.target(&needs, hurt);
            let days = self.days_since(creature.reckoned);
            let creature = self.creatures.live.get_mut(&id).expect("listed");
            creature.condition = rules.step(creature.condition, target, days);
            creature.reckoned = day;
            creature.needs = needs;
        }
    }

    /// How well each of a kept animal's needs is met where it stands now.
    pub(crate) fn needs_of(&mut self, id: u32) -> Needs {
        let rules = *self.content.creatures.condition();
        let creature = &self.creatures.live[&id];
        let (species, at, home, fit) = (creature.species, creature.body.position, creature.home, creature.climate_fit);
        let def = &self.content.creatures.get(species).def;
        let food = rules.fed(self.days_since(creature.fed));
        let water = if self.water_near(home, KEPT_LEASH as i32) { Met::Yes } else { Met::No };
        // Room: the ground within reach against what every kept animal on it wants.
        let reach = rules.space_radius;
        let wanted: f32 = self
            .creatures
            .live
            .values()
            .filter(|c| c.owner.is_some() && c.body.position.distance(at) <= reach)
            .map(|c| {
                let def = &self.content.creatures.get(c.species).def;
                def.needs.space(def.body.size.0)
            })
            .sum();
        let space = condition::room(std::f32::consts::PI * reach * reach, wanted);
        let company = if def.needs.company(def.temperament.sociability) {
            let kin = self.creatures.live.iter().any(|(&other, c)| other != id && c.species == species && c.body.position.distance(at) < rules.company);
            if kin { Met::Yes } else { Met::No }
        } else {
            Met::Yes
        };
        // Out in the rain it wants a roof near; snow is worse unless its kind is made for
        // the cold.
        let hardy = def.habitat.comfort.temperature.0 <= rules.snow_hardy;
        let (wants_roof, height) = (def.needs.shelter(), def.body.size.1);
        let wet = if wants_roof { self.out_in_the_wet(at, height, rules.wet) } else { None };
        let roof = wet.is_some() && self.roof_near(at, height, rules.roof_within);
        let shelter = condition::shelter(wants_roof, wet, roof, hardy);
        Needs { food, water, shelter, space, climate: rules.climate(fit), company }
    }

    /// How well a place suits `species`'s climate: its comfort fit there, which a kept
    /// animal keeps from its home (set when it is taken in, born or brought back).
    pub(crate) fn climate_fit(&self, species: CreatureId, at: Vec3) -> f32 {
        let def = &self.content.creatures.get(species).def;
        let site = self.overworld.site(at.x.floor() as i32, at.z.floor() as i32);
        creature::fit(&def.habitat.comfort, site.temperature, site.moisture, site.aether)
    }

    /// Makes where creature `id` stands its home, and its home's climate the one it lives by.
    pub(crate) fn settle(&mut self, id: u32) {
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let (species, at) = (creature.species, creature.body.position);
        let fit = self.climate_fit(species, at);
        let creature = self.creatures.live.get_mut(&id).expect("checked");
        (creature.home, creature.climate_fit) = (at, fit);
    }

    /// Whether rain or snow heavier than `wet` falls on a body standing at `at`, `height`
    /// tall, with nothing over it: `Some(true)` for snow, `Some(false)` for rain.
    fn out_in_the_wet(&mut self, at: Vec3, height: f32, wet: f32) -> Option<bool> {
        let head = (at + Vec3::Y * height.max(0.5)).floor().as_ivec3();
        if !self.open_sky(head) {
            return None;
        }
        let (tick, day) = (self.tick, self.day);
        let report = self.weather.local(&mut self.temperature, Vec2::new(at.x, at.z), tick, day);
        (report.weather.precipitation > wet).then_some(report.weather.snow >= 0.5)
    }

    /// Whether shelter lies within `within` blocks of a body at `at`, `height` tall: a spot
    /// it could stand in (feet and head clear) with something solid a few blocks over its
    /// head. Searched outward ring by ring, stopping at the first.
    pub(crate) fn roof_near(&self, at: Vec3, height: f32, within: f32) -> bool {
        let feet = at.floor().as_ivec3();
        let head = (at + Vec3::Y * height.max(0.5)).floor().as_ivec3();
        (1..=within.floor() as i32).any(|r| {
            (0..8 * r).any(|i| {
                let (dx, dz) = crate::creatures::ring(r, i);
                let (feet, head) = (feet + IVec3::new(dx, 0, dz), head + IVec3::new(dx, 0, dz));
                !self.is_solid(feet) && !self.is_solid(head) && (1..=6).any(|dy| self.is_solid(head + IVec3::Y * dy))
            })
        })
    }

    /// Whether something to drink (a block tagged `drink`: water, a well, a trough) lies
    /// within `reach` blocks of `at` across and a few up and down.
    fn water_near(&self, at: Vec3, reach: i32) -> bool {
        let centre = at.floor().as_ivec3();
        let blocks = &self.content.blocks;
        let drink = |cell: IVec3| self.block(cell).is_some_and(|b| blocks.def(b).has_tag(condition::DRINK));
        (-3..=1).any(|dy| (-reach..=reach).any(|dz| (-reach..=reach).any(|dx| drink(centre + IVec3::new(dx, dy, dz)))))
    }

    /// What a kept animal puts into work, a treadwheel's or gin's share (0.5 to 1.2 of a
    /// full share by default): none from a wild one, a young one, or one unfed past
    /// `fed.1` days, which stops walking.
    pub(crate) fn working_share(&self, id: u32) -> f32 {
        let rules = self.content.creatures.condition();
        let Some(creature) = self.creatures.live.get(&id) else { return 0.0 };
        if creature.owner.is_none() || creature.grows_up.is_some() || rules.fed(self.days_since(creature.fed)) == Met::No {
            return 0.0;
        }
        rules.output(creature.condition)
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::block::BlockId;
    use voxmar_engine::creature::condition::{DRINK, Met};
    use voxmar_engine::creature::husbandry::KEPT_LEASH;
    use voxmar_engine::glam::IVec3;

    /// Water, a well and a feed trough all water a pen through the `drink` tag, and a block
    /// without it does not.
    #[test]
    fn a_pen_drinks_from_water_a_well_or_a_trough() {
        let mut server = crate::test_server();
        let at = server.spawn.floor().as_ivec3();
        let reach = KEPT_LEASH as i32 + 1;
        server.load_area(at - IVec3::splat(reach), at + IVec3::splat(reach));
        let species = server.content.creatures.id("base:rabbit").expect("rabbits");
        let rabbit = server.spawn_creature(species, server.spawn, 1).expect("room");
        let creature = server.creatures.live.get_mut(&rabbit).unwrap();
        (creature.owner, creature.home) = (Some("test".into()), server.spawn);
        // Nothing to drink about its home.
        for dy in -4..=2 {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    let cell = at + IVec3::new(dx, dy, dz);
                    if server.block(cell).is_some_and(|b| server.content.blocks.def(b).has_tag(DRINK)) {
                        server.set_block(cell, BlockId::AIR);
                    }
                }
            }
        }
        assert_eq!(server.needs_of(rabbit).water, Met::No, "a dry pen watered its rabbit");
        let spot = at + IVec3::new(3, 0, 0);
        let cobblestone = server.content.blocks.expect("base:cobblestone");
        server.set_block(spot, cobblestone);
        assert_eq!(server.needs_of(rabbit).water, Met::No, "a cobblestone counted as water");
        for name in ["base:water", "base:well", "base:feed_trough"] {
            let block = server.content.blocks.expect(name);
            server.set_block(spot, block);
            assert_eq!(server.needs_of(rabbit).water, Met::Yes, "{name} by its home did not water a kept rabbit");
        }
        // Past its leash, it is too far.
        server.set_block(spot, BlockId::AIR);
        server.set_block(at + IVec3::new(KEPT_LEASH as i32 + 1, 0, 0), server.content.blocks.expect("base:well"));
        assert_eq!(server.needs_of(rabbit).water, Met::No, "a well past its leash watered it");
    }
}
