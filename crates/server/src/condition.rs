//! Kept animals' condition on the server (docs/breeding.md, "Keeping animals"): every few
//! seconds each kept animal in loaded land looks at its needs (food, water, shelter, room,
//! climate, company) and its condition walks toward what they allow. The rules are the
//! engine's (`creature::condition`) and the content's numbers.

use voxmar_engine::creature::condition::{self, Met, Needs};
use voxmar_engine::creature::husbandry::KEPT_LEASH;
use voxmar_engine::creature;
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_protocol::{TICKS_PER_DAY, TICKS_PER_SECOND};

use crate::Server;

/// Ticks between looks at kept animals' needs.
pub(crate) const RECKON_EVERY: u64 = 10 * TICKS_PER_SECOND as u64;
/// Within how many blocks one of its kind keeps a social animal company.
const COMPANY: f32 = 14.0;
/// Rain or snow heavier than this wants a roof.
const WET: f32 = 0.05;
/// Climate fit at or above this is met, at or above `CLIMATE_PARTLY` partly.
const CLIMATE_MET: f32 = 0.75;
const CLIMATE_PARTLY: f32 = 0.25;

impl Server {
    /// In-game days from day-clock `since` to now.
    pub(crate) fn days_since(&self, since: f64) -> f32 {
        ((self.day - since) / TICKS_PER_DAY as f64) as f32
    }

    /// Looks at each kept animal's needs and moves its condition toward them. Animals whose
    /// land is not loaded wait as they are, as their land does.
    pub(crate) fn reckon_condition(&mut self) {
        let rules = *self.content.creatures.condition();
        let ids: Vec<u32> = self.creatures.live.iter().filter(|(_, c)| c.owner.is_some()).map(|(&id, _)| id).collect();
        for id in ids {
            let creature = &self.creatures.live[&id];
            if !self.world.contains(voxmar_engine::coords::ColumnPos::of_block(creature.body.position.floor().as_ivec3())) {
                continue;
            }
            let needs = self.needs_of(id);
            let creature = &self.creatures.live[&id];
            let def = &self.content.creatures.get(creature.species).def;
            let hurt = (creature.health as u32) * 2 < def.health as u32;
            let target = rules.target(&needs, hurt);
            let days = self.days_since(creature.reckoned);
            let day = self.day;
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
        let (species, at, home) = (creature.species, creature.body.position, creature.home);
        let def = &self.content.creatures.get(species).def;
        let site = self.overworld.site(at.x.floor() as i32, at.z.floor() as i32);
        let fit = creature::fit(&def.habitat.comfort, site.temperature, site.moisture, site.aether);
        let climate = if fit >= CLIMATE_MET {
            Met::Yes
        } else if fit >= CLIMATE_PARTLY {
            Met::Partly
        } else {
            Met::No
        };
        let food = rules.fed(self.days_since(creature.fed));
        let water = if self.water_near(home, KEPT_LEASH as i32) { Met::Yes } else { Met::No };
        // Room: the ground within reach shared by every kept animal on it.
        let reach = rules.space_radius;
        let sharing = self.creatures.live.values().filter(|c| c.owner.is_some() && c.body.position.distance(at) <= reach).count().max(1);
        let room = std::f32::consts::PI * reach * reach / sharing as f32;
        let space = condition::room(room, def.needs.space(def.body.size.0));
        let company = if def.needs.company(def.temperament.sociability) {
            let kin = self.creatures.live.iter().any(|(&other, c)| other != id && c.species == species && c.body.position.distance(at) < COMPANY);
            if kin { Met::Yes } else { Met::No }
        } else {
            Met::Yes
        };
        let shelter = if def.needs.shelter() && self.out_in_the_wet(at, def.body.size.1) {
            // Rain wets it; snow chills it too.
            if site.temperature < 0.0 { Met::No } else { Met::Partly }
        } else {
            Met::Yes
        };
        Needs { food, water, shelter, space, climate, company }
    }

    /// Whether rain or snow falls on a body standing at `at`, `height` tall, with nothing
    /// over it.
    fn out_in_the_wet(&mut self, at: Vec3, height: f32) -> bool {
        let head = (at + Vec3::Y * height.max(0.5)).floor().as_ivec3();
        if !self.open_sky(head) {
            return false;
        }
        let (tick, day) = (self.tick, self.day);
        let report = self.weather.local(&mut self.temperature, Vec2::new(at.x, at.z), tick, day);
        report.weather.precipitation > WET
    }

    /// Whether water lies within `reach` blocks of `at` across and a few up and down.
    fn water_near(&self, at: Vec3, reach: i32) -> bool {
        let centre = at.floor().as_ivec3();
        (-3..=1).any(|dy| (-reach..=reach).any(|dz| (-reach..=reach).any(|dx| self.is_water(centre + IVec3::new(dx, dy, dz)))))
    }

    /// What a kept animal puts into work, a treadwheel's or gin's share (0.5 to 1.2 of a
    /// full share by default): none from a wild one, a young one, or one unfed past
    /// `fed.1` days, which stops walking.
    #[allow(dead_code, reason = "the treadwheel (Machines) reads it")]
    pub(crate) fn working_share(&self, id: u32) -> f32 {
        let rules = self.content.creatures.condition();
        let Some(creature) = self.creatures.live.get(&id) else { return 0.0 };
        if creature.owner.is_none() || creature.grows_up.is_some() || rules.fed(self.days_since(creature.fed)) == Met::No {
            return 0.0;
        }
        rules.output(creature.condition)
    }

}
