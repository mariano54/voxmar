//! Processes: continuous rules over traits and fields, each running at its own tier.
//! Processes never decide; they change values and raise events (here: death).

use voxmar_engine::coords::ColumnPos;
use voxmar_engine::effect::{HomeSurvey, Situation};
use voxmar_engine::fluid::FluidId;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits::{self, Replication};
use voxmar_engine::vitals;
use voxmar_engine::effect::EffectId;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::weather::Weather;

pub(crate) trait Process: Send {
    fn tier(&self) -> Tier;
    fn run(&mut self, server: &mut Server);
    /// Whether to run this tick even though its tier is not due: for news a player who
    /// just joined must not wait for, such as the weather where they are.
    fn urgent(&self, _server: &Server) -> bool {
        false
    }
}

pub(crate) fn core_processes() -> Vec<Box<dyn Process>> {
    vec![
        Box::new(Falling),
        Box::new(Survival),
        Box::new(crate::tool_wear::Burning::default()),
        Box::new(crate::flames::Flames),
        Box::new(Weather::default()),
        Box::new(crate::wear::Footsteps::default()),
        Box::new(crate::farm::Growth),
        Box::new(crate::smoulder::Smoulders),
        Box::new(crate::stores::Shelves),
        Box::new(crate::machines::Machines),
        Box::new(crate::apiary::Bees::default()),
        Box::new(crate::stations::Stations),
        Box::new(crate::traps::Traps),
        Box::new(crate::lairs::Bosses),
    ]
}

/// Fall damage, burning in lava and the void, from the server's own view of where
/// players stand.
struct Falling;

/// Whether `fluid` burns like lava: its source block is tagged `lava`.
pub(crate) fn is_lava(blocks: &voxmar_engine::block::BlockRegistry, fluid: FluidId) -> bool {
    blocks.def(blocks.fluid_def(fluid).source).has_tag("lava")
}

impl Process for Falling {
    fn tier(&self) -> Tier {
        Tier::High
    }

    fn run(&mut self, server: &mut Server) {
        let blocks = &server.content.blocks;
        let rules = server.survival.clone();
        let burns = server.tick.is_multiple_of(rules.burn_ticks());
        let tick = server.tick;
        let ids: Vec<PlayerId> = server.players.keys().copied().collect();
        for id in ids {
            let player = &server.players[&id];
            if !player.joined || !player.health.alive() {
                continue;
            }
            let aabb = player.aabb();
            let column_loaded = server.world.contains(ColumnPos::of_block(player.position.floor().as_ivec3()));
            let mut below = aabb;
            below.translate(Vec3::new(0.0, -0.05, 0.0));
            // Ground is what the feet rest on as drawn: a shaft lined with posts or stair corners
            // is still a fall.
            let on_ground = below.overlaps(|p| server.boxes(p));
            // Any state of the fluid counts: flowing water (any fluid swum in) breaks a fall,
            // flowing lava burns.
            let touching = |kind: &dyn Fn(FluidId) -> bool| aabb.intersects(|p| server.block(p).and_then(|b| blocks.fluid(b)).is_some_and(|f| kind(f.fluid)));
            let (in_water, in_lava) = (touching(&|f| blocks.fluid_def(f).def.swim), touching(&|f| is_lava(blocks, f)));
            // Feet on a ladder, nobody is falling: sliding down it ends no fall.
            let on_ladder = server.on_ladder(player.position);

            let player = server.players.get_mut(&id).unwrap();
            // Where a grave would go: never in lava, nor on anything below the void.
            if on_ground && !in_lava && player.position.y >= rules.void_below {
                player.last_ground = Some(player.position);
            }
            if on_ground && !player.flying {
                player.grounded = tick;
            }
            let mut damage = 0.0;
            if in_lava && burns {
                damage += rules.burn as f32;
            }
            if player.flying || in_water || in_lava || on_ladder || !column_loaded {
                player.fall_start = None;
            } else if on_ground {
                if let Some(start) = player.fall_start.take() {
                    damage += (start - player.position.y - rules.safe_fall).max(0.0).ceil();
                }
            } else {
                let y = player.position.y;
                player.fall_start = Some(player.fall_start.map_or(y, |s| s.max(y)));
            }
            if player.position.y < rules.void_below {
                damage += player.health.max as f32;
            }
            if damage > 0.0 {
                let died = player.wound(damage as u8, tick);
                player.changed(traits::HEALTH);
                if died {
                    player.connection.send(ServerMessage::Died);
                }
            }
        }
    }
}

/// Survival: warmth follows the air, fires, water and the weather, and together with what
/// the player ate they decide each player's effects (content in `effects.ron`, see
/// `voxmar_engine::effect`), whose modifiers heal or hurt them over time, set their most
/// health and how fast they mine and learn. There is no hunger: food heals (`actions`).
/// Once a second; creative players are never hurt.
#[derive(Default)]
pub(crate) struct Survival;

impl Process for Survival {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let seconds = Tier::Low.period() as f32 / TICKS_PER_SECOND as f32;
        let ids: Vec<PlayerId> = server.players.keys().copied().collect();
        for id in ids {
            Survival::step(server, id, seconds);
        }
    }
}

impl Survival {
    /// Judges player `id`'s effects at once, as the once-a-second pass does but with no time
    /// gone by: for a change that should tell at once (sat down at a table, then a bite).
    pub(crate) fn refresh(server: &mut Server, id: PlayerId) {
        Survival::step(server, id, 0.0);
    }

    /// One player's pass `seconds` on from the last: warmth, effects, healing and hurt.
    fn step(server: &mut Server, id: PlayerId, seconds: f32) {
        let effects = server.content.effects.clone();
        let hitpoints = server.content.skills.id("core:hitpoints");
        // Steps of a home reached, counted for quests (`Built("home_snug", 1)`, qualified by the effect's package).
        let mut reached: Vec<(PlayerId, String)> = Vec::new();
        {
        let player = &server.players[&id];
        if !player.joined || !player.health.alive() {
            return;
        }
        let feet = player.position.floor().as_ivec3();
        let air = server.temperature.sample(feet.x, feet.z);
        let heat = server.heat_near(feet);
        let wet = server.block(feet).and_then(|b| server.content.blocks.fluid(b)).is_some();
        let outdoors = server.open_sky(feet + voxmar_engine::glam::IVec3::Y * 2);
        let at = voxmar_engine::glam::Vec2::new(player.position.x, player.position.z);
        let (tick, day, night) = (server.tick, server.day, server.night());
        let weather = server.weather.local(&mut server.temperature, at, tick, day).weather;
        let cold_air = air - if wet { server.survival.water_chill } else { 0.0 };
        let warmed = vitals::warmed(heat, cold_air);
        let home = server.home_survey(id);
        let home_stores = server.home_store_tally(id);
        // What effects ask of the item in hand (`Holding`) and of the blocks near
        // (`Near`).
        let held: Vec<String> = player.held().map_or_else(Vec::new, |item| effects.held_by(server.content.blocks.def(item)));
        // And of the charm worn (`Wearing`): only the charm slot's, so one charm's effect
        // at a time, whatever else a pack tags.
        let charm = player.inventory.worn[voxmar_engine::combat::Slot::Charm.index()];
        let worn: Vec<String> = charm.map_or_else(Vec::new, |s| effects.worn_by(Some(voxmar_engine::combat::Slot::Charm.index()), server.content.blocks.def(s.item)));
        let near: Vec<(String, i32)> = effects.near_asks().iter().filter(|(tag, reach)| server.tagged_within(feet, tag, *reach)).cloned().collect();
        let max_level = hitpoints.map(|skill| server.content.skills.level(server.players[&id].skills.xp(skill)));
        // Mending (a prayer) brings health back faster.
        let mending = server.content.prayers.mending(&server.players[&id].praying.on);
        // Sat on a seat until they step off it, or it goes.
        let seated = server.players[&id].vitals.seated.filter(|&seat| {
            vitals::still_seated(server.players[&id].position, seat)
                && server.block(seat).is_some_and(|b| server.content.blocks.def(b).has_tag(voxmar_engine::action::SEAT_TAG))
        });

        let anchor = server.at_home(id);
        let player = server.players.get_mut(&id).unwrap();
        // What their home meets now, and where it stands, to tell on their return what
        // lights burnt out while they were away cost; not before that is told
        // (`flames::check_home_return`).
        if let Some(home) = home.as_ref().filter(|_| !player.home_unchecked) {
            (player.home_steps, player.home_anchor) = (effects.home_steps(home), anchor);
        }
        let creative = player.allow_flight;
        let before = (player.vitals.clone(), player.health);
        // A minute of other things brings back what eating the same food over and over
        // took off it.
        player.food_clock += seconds;
        if player.food_clock >= 60.0 {
            player.food_clock -= 60.0;
            player.recent_food.recover();
        }
        player.timed_effects.retain_mut(|(_, left)| {
            *left -= seconds;
            *left > 0.0
        });
        // Warmth from what was eaten and the weather counts before the bands of warmth
        // are judged: soaked through, a mild day feels chilly.
        let biome = server.overworld.biome_at(feet.x, feet.z);
        let mut situation = Situation { felt: vitals::felt_air(cold_air, heat), warmed, outdoors, weather: &weather, biome: &biome.name, biome_tags: &biome.tags, home: home.as_ref(), done: &player.quests_done, held: &held, near: &near, worn: &worn, seated: seated.is_some(), night };
        // What clings (smoke) is topped up while it holds, and lasts after.
        effects.linger(&situation, &mut player.timed_effects);
        let first = effects.active(&situation, &player.timed_effects);
        situation.felt = effects.modifiers(&first).felt(vitals::felt_air(cold_air, heat), server.survival.comfort_top);
        let active = effects.active(&situation, &player.timed_effects);
        let modifiers = effects.modifiers(&active);
        // Quests ask whether an effect was ever felt (`Felt`), not how often.
        for a in &active {
            player.tallies.entry(format!("felt:{}", effects.get(a.effect).name)).or_insert(1);
        }
        player.modifiers = modifiers;
        let rates = vitals::rates(&modifiers, mending, server.survival.heal_most);
        // Twice the Hitpoints level, and what effects add: a level gained brings health
        // to fill it; what effects add comes empty (Hearty's tenth, design draft 63), and
        // what a lowered most takes goes.
        let base = max_level.map_or(server.survival.max_health as i32, |level| 2 * level as i32).clamp(1, u8::MAX as i32) as u8;
        let max = (((base as i32 + modifiers.max_health as i32) as f32 * modifiers.max_health_share).round() as i32).clamp(1, u8::MAX as i32) as u8;
        if base != player.health_base || max != player.health.max {
            // Just joined: as much as the most went up.
            let gained = if player.health_base == 0 { max.saturating_sub(player.health.max) } else { base.saturating_sub(player.health_base) };
            player.health_base = base;
            player.health.max = max;
            player.health.current = player.health.current.saturating_add(gained).min(max);
        }
        // Healing and hurt run on time gone by: a refresh (no time) leaves them be, or each
        // would pay out a point the pass before had already counted.
        if seconds > 0.0 {
            // Healing, while anything is left to heal.
            match rates.heal_every.filter(|_| player.health.current < player.health.max) {
                Some(every) => {
                    player.heal_clock += seconds;
                    if player.heal_clock >= every {
                        player.heal_clock -= every;
                        player.health.heal(1);
                    }
                    // Time past a point is not banked to pay out after the buffs end: at
                    // most one point's worth waits.
                    player.heal_clock = player.heal_clock.min(every);
                }
                None => player.heal_clock = 0.0,
            }
            match rates.hurt_every.filter(|_| !creative) {
                Some(every) => {
                    player.hurt_clock += seconds;
                    if player.hurt_clock >= every {
                        player.hurt_clock -= every;
                        // Wearing at them (cold, heat, hail) is no wound: it never stops a sleep.
                        if player.health.damage(1) {
                            player.connection.send(ServerMessage::Died);
                        }
                    }
                }
                None => player.hurt_clock = 0.0,
            }
        }
        // The felt temperature is sent to the degree, so it does not resend every tick.
        player.vitals.felt = situation.felt.round();
        player.vitals.effects = effects.active(&situation, &player.timed_effects);
        player.vitals.home = player.home_block(&server.lit);
        player.vitals.seated = seated;
        player.vitals.home_stores = home_stores;
        // What kept animals laid into the home's stores is told once the keeper is home.
        if home.is_some() && !player.nested.is_empty() {
            let items = std::mem::take(&mut player.nested).into_iter().map(|(item, n)| Words::key("multiblock.fuel_stack").with("count", n.to_string()).with("item", Words::Block(item))).collect();
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("home.nested").with("items", Words::List(items)) });
        }
        // The steps of a ladder say so when first reached.
        let gained: Vec<EffectId> = player.vitals.effects.iter().map(|a| a.effect).filter(|&e| effects.get(e).announce && !player.announced.contains(&e)).collect();
        player.announced.extend(&gained);
        for effect in gained {
            let def = effects.get(effect);
            let (package, own) = def.name.split_once(':').unwrap_or(("", &def.name));
            reached.push((id, format!("{package}:{}_{own}", def.ladder)));
            let key = voxmar_engine::lang::def_key("effect", &def.name);
            let mut words = Words::key("effect.gained").with("name", Words::Own(key.clone(), def.title.clone())).with("what", Words::Own(format!("{key}.description"), def.description.clone()));
            if !def.next.is_empty() {
                words = Words::key("effect.gained_next").with("name", Words::Own(key.clone(), def.title.clone())).with("what", Words::Own(format!("{key}.description"), def.description.clone())).with("next", Words::Own(format!("{key}.next"), def.next.clone()));
            }
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
        if player.vitals != before.0 {
            player.changed(traits::VITALS);
        }
        if player.health != before.1 {
            player.changed(traits::HEALTH);
        }
        }
        for (id, step) in reached {
            server.tally(id, "built", &step, 1);
        }
    }
}

impl Server {
    /// Whether a block tagged `tag` lies within `reach` blocks of `at`, every way.
    pub(crate) fn tagged_within(&self, at: IVec3, tag: &str, reach: i32) -> bool {
        let blocks = &self.content.blocks;
        (-reach..=reach).any(|dy| (-reach..=reach).any(|dz| (-reach..=reach).any(|dx| self.block(at + IVec3::new(dx, dy, dz)).is_some_and(|b| blocks.def(b).has_tag(tag)))))
    }

    /// What lies around player `id`'s home (the lit hearth, else the bed last slept in),
    /// counted by the tags effects ask of it, if they stand within its reach.
    pub(crate) fn home_survey(&self, id: PlayerId) -> Option<HomeSurvey> {
        self.at_home(id).map(|anchor| self.survey_around(anchor, Some(id)))
    }

    /// Where player `id`'s home is, if they stand within its reach.
    pub(crate) fn at_home(&self, id: PlayerId) -> Option<IVec3> {
        let player = self.players.get(&id)?;
        let anchor = player.home_spot(&self.lit)?.floor().as_ivec3();
        self.in_home_box(anchor, player.position).then_some(anchor)
    }

    /// Where player `id`'s home is, if `at` lies within its reach.
    pub(crate) fn home_holding(&self, id: PlayerId, at: Vec3) -> Option<IVec3> {
        let player = self.players.get(&id)?;
        let anchor = player.home_spot(&self.lit)?.floor().as_ivec3();
        self.in_home_box(anchor, at).then_some(anchor)
    }

    /// Whether `at` lies in the home box around `anchor`.
    fn in_home_box(&self, anchor: IVec3, at: Vec3) -> bool {
        let rules = &self.survival;
        let feet = at.floor().as_ivec3();
        let near = (feet - anchor).abs();
        near.x <= rules.home_reach && near.z <= rules.home_reach && feet.y <= anchor.y + rules.home_up && feet.y >= anchor.y - rules.home_down
    }

    /// What lies around a home at `anchor`, whoever stands near it: its blocks by tag and the
    /// animals `keeper` keeps within its reach (`kept`).
    pub(crate) fn survey_around(&self, anchor: IVec3, keeper: Option<PlayerId>) -> HomeSurvey {
        let rules = &self.survival;
        let (up, down) = (rules.home_up, rules.home_down);
        let wanted = self.content.effects.home_tags();
        let mut counts: Vec<(String, u32)> = wanted.iter().map(|tag| (tag.clone(), 0)).collect();
        let walls = if self.content.effects.home_walls() { self.room_walls(anchor) } else { Default::default() };
        if wanted.is_empty() {
            return HomeSurvey { counts, walls };
        }
        let blocks = &self.content.blocks;
        let reach = rules.home_reach;
        // Tags counted only under the open sky (`outdoors`: things set out in the yard).
        let outdoor: Vec<bool> = counts.iter().map(|(tag, _)| self.content.effects.outdoor(tag)).collect();
        for dy in -down..=up {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    let at = anchor + IVec3::new(dx, dy, dz);
                    let Some(block) = self.block(at) else { continue };
                    let tags = &blocks.def(block).tags;
                    if tags.is_empty() {
                        continue;
                    }
                    for ((tag, n), &outdoor) in counts.iter_mut().zip(&outdoor) {
                        if tags.contains(tag) && (!outdoor || self.open_sky(at + IVec3::Y)) {
                            *n += 1;
                        }
                    }
                }
            }
        }
        if let Some((_, kept)) = counts.iter_mut().find(|(tag, _)| tag == "kept") {
            let (reach, centre) = (reach as f32 + 0.5, anchor.as_vec3() + Vec3::splat(0.5));
            for creature in self.creatures.live.values().filter(|c| keeper.is_some() && c.owner_id == keeper) {
                let off = creature.body.position - centre;
                if off.x.abs() <= reach && off.z.abs() <= reach && off.y <= up as f32 + 1.0 && off.y >= -(down as f32) - 1.0 {
                    *kept += 1;
                }
            }
        }
        HomeSurvey { counts, walls }
    }

    /// The walls and roof about the room a home at `anchor` stands in: from the cell at
    /// `anchor` (or the first open cell over it: a hearth's hearthstone) out over every open
    /// cell within the home's reach, each face met on the way
    /// (sideways or up) is a face of the room, of the shelter its block's tag says (a solid
    /// block with none is draughty), or an opening where the way leads out of the reach or
    /// into the unloaded. A wall is a solid block or any with a shelter tag (a closed door, a
    /// window); a door left open lets the scan out, and furniture (survival.ron's
    /// `home_furniture`, with no shelter tag: a shelter tag always wins) is looked past to the
    /// wall behind it, though never stepped down into: a hearthstone set in its ring is floor. Bounded by the home box, so a home in
    /// the open costs no more than the box.
    pub(crate) fn room_walls(&self, anchor: IVec3) -> voxmar_engine::effect::Walls {
        use voxmar_engine::effect::{RoomCell, Shelter, Walls};
        let (rules, blocks) = (&self.survival, &self.content.blocks);
        // What stands at a cell: open, or a wall of a shelter. Furniture (a furniture tag
        // with no shelter tag: a shelter tag always wins) is looked past, as open, whatever
        // it is made of.
        Walls::of_room(anchor, rules.home_reach, rules.home_up, rules.home_down, |p| {
            let block = self.block(p)?;
            let (tags, solid) = (&blocks.def(block).tags, blocks.info(block).solid);
            let furniture = rules.furniture(tags);
            Some(RoomCell { wall: Shelter::of_wall(tags, solid, furniture), solid, furniture })
        })
    }

    /// Whether nothing solid hangs over `at` (within `survival.roof_scan` blocks): out in
    /// the rain.
    pub(crate) fn open_sky(&self, at: voxmar_engine::glam::IVec3) -> bool {
        (0..self.survival.roof_scan).map(|dy| at + voxmar_engine::glam::IVec3::Y * dy).all(|p| !self.is_solid(p))
    }

    /// How much fires, torches and other glowing blocks near `at` warm it (°C): the
    /// brightest within reach, fading with distance.
    fn heat_near(&self, at: voxmar_engine::glam::IVec3) -> f32 {
        let blocks = &self.content.blocks;
        let rules = &self.survival;
        let reach = rules.heat_reach;
        let mut heat = 0.0f32;
        for dy in -1..=2 {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    let pos = at + voxmar_engine::glam::IVec3::new(dx, dy, dz);
                    let Some(block) = self.block(pos) else { continue };
                    let emission = blocks.info(block).emission;
                    if emission < rules.heat_emission {
                        continue;
                    }
                    let distance = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                    let falloff = (1.0 - distance / (reach as f32 + 1.0)).max(0.0);
                    heat = heat.max(rules.heat * falloff * emission as f32 / 15.0);
                }
            }
        }
        heat
    }
}

impl Server {
    pub(crate) fn run_processes(&mut self) {
        let _span = tracing::info_span!("processes").entered();
        let mut processes = std::mem::take(&mut self.processes);
        for process in &mut processes {
            if process.tier().due(self.tick) || process.urgent(self) {
                process.run(self);
            }
        }
        self.processes = processes;
    }

    /// Sends owner-replicated traits that changed this tick.
    pub(crate) fn replicate_owned_traits(&mut self) {
        for player in self.players.values_mut().filter(|p| p.joined && !p.dirty.is_empty()) {
            debug_assert!(player.dirty.iter().all(|d| d.replication == Replication::Owner));
            player.dirty.clear();
            // Joining and sleeping send vitals before the next survival pass would.
            player.vitals.home = player.home_block(&self.lit);
            player
                .connection
                .send(ServerMessage::PlayerState {
                    health: player.health,
                    inventory: player.inventory.clone(),
                    skills: player.skills.clone(),
                    vitals: player.vitals.clone(),
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sliding down a ladder at its own pace, from ten blocks up or from six after jumping
    /// onto it, ends no fall: no harm at the bottom, and never falling on the way (so no
    /// falling blow, fight.rs). The same descent with no ladder hurts.
    #[test]
    fn sliding_down_a_ladder_is_no_fall() {
        use voxmar_engine::block::BlockId;
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let (stone, planks, ladder) = (server.content.blocks.expect("base:stone"), server.content.blocks.expect("base:oak_planks"), server.content.blocks.expect("base:oak_ladder"));
        server.set_block(at, stone);
        for y in 1..=12 {
            server.set_block(at + IVec3::new(0, y, 1), planks);
        }
        let id = crate::creatures::cornered_tests::join(&mut server, at.as_vec3() + Vec3::new(0.5, 11.0, 0.5));
        let slide = |server: &mut Server, from: f32, rungs: bool| -> (u8, bool) {
            for y in 1..=10 {
                server.set_block(at + IVec3::Y * y, if rungs { ladder } else { BlockId::AIR });
            }
            let player = server.players.get_mut(&id).unwrap();
            player.health.current = player.health.max;
            player.position = at.as_vec3() + Vec3::new(0.5, 1.0 + from, 0.5);
            // Jumped onto it from a little above.
            player.fall_start = Some(player.position.y + 1.0);
            let mut falling = false;
            // Down at climb_speed, 3 blocks a second: 0.15 a tick.
            for _ in 0..=(from / 0.15).ceil() as usize {
                Falling.run(server);
                let player = server.players.get_mut(&id).unwrap();
                falling |= player.fall_start.is_some_and(|top| player.position.y < top - 0.3);
                player.position.y = (player.position.y - 0.15).max(at.y as f32 + 1.0);
            }
            Falling.run(server);
            (server.players[&id].health.current, falling)
        };
        let full = server.players[&id].health.max;
        assert_eq!(slide(&mut server, 9.99, true), (full, false), "a slide down a ten-block ladder hurt or fell");
        assert_eq!(slide(&mut server, 6.0, true), (full, false), "a slide down from six blocks up hurt or fell");
        let (bare, falling) = slide(&mut server, 9.99, false);
        assert!(bare < full && falling, "the same descent with no ladder did not hurt");
    }

    /// A home's room is judged by its walls and roof: a closed plank room is sound all about,
    /// a brick in it is a fine face, a hole or an open door lets the outside in, and a closed
    /// door is a wall.
    #[test]
    fn a_room_is_judged_by_its_walls_and_roof() {
        use voxmar_engine::{block::BlockId, effect::Shelter};
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (stone, planks) = (block(&server, "base:stone"), block(&server, "base:oak_planks"));
        // A floor, plank walls three high about a five by five room, and a plank roof.
        for x in -3..=3 {
            for z in -3..=3 {
                server.set_block(at + IVec3::new(x, 0, z), stone);
                server.set_block(at + IVec3::new(x, 4, z), planks);
                for y in 1..=3 {
                    let edge = x.abs() == 3 || z.abs() == 3;
                    server.set_block(at + IVec3::new(x, y, z), if edge { planks } else { BlockId::AIR });
                }
            }
        }
        let home = at + IVec3::Y;
        let walls = server.room_walls(home);
        assert_eq!(walls.faces, 5 * 3 * 4 + 5 * 5, "the walls and the roof, the floor not counted");
        assert_eq!((walls.share(Shelter::Draughty), walls.share(Shelter::Sound), walls.share(Shelter::Fine)), (1.0, 1.0, 0.0), "planks are sound");
        let brick = at + IVec3::new(3, 2, 0);
        server.set_block(brick, block(&server, "base:stone_bricks"));
        assert_eq!(server.room_walls(home).at_least, [85, 85, 1], "a brick is a fine face, and sound too");
        // A hole lets the scan out to the home's reach: open faces, not walled in.
        let hole = at + IVec3::new(-3, 1, 0);
        server.set_block(hole, BlockId::AIR);
        let open = server.room_walls(home);
        assert!(open.share(Shelter::Draughty) < 0.9, "a hole is an opening: {open:?}");
        server.set_block(hole, block(&server, "base:wooden_door"));
        assert_eq!(server.room_walls(home).at_least, [85, 85, 1], "a closed door is a sound wall");
        server.set_block(hole, block(&server, "base:wooden_door_open"));
        assert!(server.room_walls(home).share(Shelter::Draughty) < 0.9, "an open one lets the outside in");
        server.set_block(hole, block(&server, "base:hide_window"));
        assert_eq!(server.room_walls(home).at_least, [85, 84, 1], "a hide window is draughty");
        // Furniture against the wall and in the room is looked past: the room is as it was.
        server.set_block(hole, planks);
        let before = server.room_walls(home);
        for (dx, name) in [(-2, "base:barrel"), (-1, "base:chest"), (2, "base:crafting_table")] {
            server.set_block(at + IVec3::new(dx, 1, 2), block(&server, name));
        }
        assert_eq!(server.room_walls(home), before, "furniture made the room draughtier");
        // A ladder up the wall to a loft is looked past as well.
        for y in 1..=3 {
            server.set_block(at + IVec3::new(0, y, 2), block(&server, "base:oak_ladder"));
        }
        assert_eq!(server.room_walls(home), before, "a ladder made the room draughtier");
        // A solid block where home stands (a hearthstone): the room is the open cell over it.
        assert_eq!(server.room_walls(at), server.room_walls(home));
    }

    /// A window walls a room in: a lattice window, a shuttered one left open or an arrow
    /// slit in a plank room leaves it as closed as it was (Snug asks nine tenths walled),
    /// though a draughty face, one short of sound all round; shut, the shutters are a sound
    /// wall again. An empty window frame is an opening: the room is open there.
    #[test]
    fn a_window_keeps_a_room_walled_and_an_empty_frame_opens_it() {
        use voxmar_engine::{block::BlockId, effect::Shelter};
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (stone, planks) = (block(&server, "base:stone"), block(&server, "base:oak_planks"));
        for x in -3..=3 {
            for z in -3..=3 {
                server.set_block(at + IVec3::new(x, 0, z), stone);
                server.set_block(at + IVec3::new(x, 4, z), planks);
                for y in 1..=3 {
                    let edge = x.abs() == 3 || z.abs() == 3;
                    server.set_block(at + IVec3::new(x, y, z), if edge { planks } else { BlockId::AIR });
                }
            }
        }
        let home = at + IVec3::Y;
        assert_eq!(server.room_walls(home).at_least, [85, 85, 0], "a closed plank room");
        let window = at + IVec3::new(3, 2, 0);
        for name in ["base:oak_lattice_window", "base:birch_shuttered_window", "base:stone_arrow_slit"] {
            server.set_block(window, block(&server, name));
            let walls = server.room_walls(home);
            assert_eq!(walls.at_least, [85, 84, 0], "{name} in the wall");
            assert_eq!(walls.share(Shelter::Draughty), 1.0, "{name} let the room out");
            let def = server.content.blocks.def(block(&server, name));
            assert!(def.has_tag(crate::rules::DRAUGHT), "{name} carries no draught for the whistle");
        }
        let shuttered = block(&server, "base:birch_shuttered_window");
        let shut = server.content.blocks.swapped(shuttered).expect("the shutters shut");
        server.set_block(window, shut);
        assert_eq!(server.room_walls(home).at_least, [85, 85, 0], "shut shutters are a sound wall");
        assert_eq!(server.content.blocks.swapped(shut), Some(shuttered), "and open again");
        server.set_block(window, block(&server, "base:oak_window_frame"));
        let open = server.room_walls(home);
        assert!(open.share(Shelter::Draughty) < 0.9, "an empty frame walled the room: {open:?}");
    }

    /// A balcony door shut in a plank room's wall walls it in: its lower half a sound face,
    /// its lattice a draughty one. Opened, it is a way out as an empty frame is, and a
    /// railing across the doorway walls nothing.
    #[test]
    fn a_balcony_door_walls_a_room_shut_and_opens_it_open() {
        use voxmar_engine::{block::BlockId, effect::Shelter};
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (stone, planks) = (block(&server, "base:stone"), block(&server, "base:oak_planks"));
        for x in -3..=3 {
            for z in -3..=3 {
                server.set_block(at + IVec3::new(x, 0, z), stone);
                server.set_block(at + IVec3::new(x, 4, z), planks);
                for y in 1..=3 {
                    let edge = x.abs() == 3 || z.abs() == 3;
                    server.set_block(at + IVec3::new(x, y, z), if edge { planks } else { BlockId::AIR });
                }
            }
        }
        let home = at + IVec3::Y;
        let door = at + IVec3::new(3, 1, 0);
        server.set_block(door, block(&server, "base:balcony_door"));
        server.set_block(door + IVec3::Y, block(&server, "base:balcony_door_top"));
        assert_eq!(server.room_walls(home).at_least, [85, 84, 0], "a shut balcony door: a sound half and a draughty lattice");
        server.set_block(door, block(&server, "base:balcony_door_open"));
        server.set_block(door + IVec3::Y, block(&server, "base:balcony_door_open_top"));
        let open = server.room_walls(home);
        assert!(open.share(Shelter::Draughty) < 0.9, "an open balcony door walled the room: {open:?}");
        // A railing across the doorway walls nothing: the survey looks past it, out.
        server.set_block(door, block(&server, "base:oak_railing"));
        server.set_block(door + IVec3::Y, planks);
        let railed = server.room_walls(home);
        assert!(railed.share(Shelter::Draughty) < 0.9, "a railing walled the room: {railed:?}");
    }

    /// A five by five room roofed as a hip: shingles round its edge facing out, a hip at
    /// each corner, and planks over the middle. Every face is sound, nothing lets the sky
    /// in, and a valley set in place of a hip leaves it sound too.
    #[test]
    fn a_hipped_roof_of_corners_shelters_a_room() {
        use voxmar_engine::{block::BlockId, effect::Shelter};
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (stone, planks) = (block(&server, "base:stone"), block(&server, "base:oak_planks"));
        for x in -3..=3 {
            for z in -3..=3 {
                server.set_block(at + IVec3::new(x, 0, z), stone);
                for y in 1..=3 {
                    let edge = x.abs() == 3 || z.abs() == 3;
                    server.set_block(at + IVec3::new(x, y, z), if edge { planks } else { BlockId::AIR });
                }
                // Planks over the walls, and the roof's edge over the room's outer cells,
                // eaves out (north, east, south, west), so the room sees every corner.
                let name = match (x, z) {
                    _ if x.abs() == 3 || z.abs() == 3 => "base:oak_planks",
                    (-2, -2) => "base:oak_shingles_corner",
                    (2, -2) => "base:oak_shingles_corner_east",
                    (2, 2) => "base:oak_shingles_corner_south",
                    (-2, 2) => "base:oak_shingles_corner_west",
                    (_, -2) => "base:oak_shingles",
                    (2, _) => "base:oak_shingles_east",
                    (_, 2) => "base:oak_shingles_south",
                    (-2, _) => "base:oak_shingles_west",
                    _ => "base:oak_planks",
                };
                server.set_block(at + IVec3::new(x, 4, z), block(&server, name));
            }
        }
        let home = at + IVec3::Y;
        let walls = server.room_walls(home);
        assert_eq!(walls.share(Shelter::Sound), 1.0, "a hipped roof let the weather in: {walls:?}");
        for (x, z) in [(-2, -2), (2, -2), (2, 2), (-2, 2), (0, -2), (2, 0), (0, 0)] {
            assert!(!server.open_sky(at + IVec3::new(x, 1, z)), "the sky shows through at {x} {z}");
        }
        server.set_block(at + IVec3::new(-2, 4, -2), block(&server, "base:oak_shingles_inner_corner"));
        assert_eq!(server.room_walls(home).share(Shelter::Sound), 1.0, "a valley let the weather in");
        assert!(!server.open_sky(at + IVec3::new(-2, 1, -2)), "the sky shows through the valley");
    }

    /// The home ladder's own furniture in a five by five stone-brick room under a plank roof
    /// leaves it as walled as it was empty: sound enough for Cosy and fine enough for Heart
    /// of the Home. Thatch and shingles roof it as soundly as planks.
    #[test]
    fn a_furnished_room_under_any_roof_is_as_walled_as_it_was_empty() {
        use voxmar_engine::{block::BlockId, effect::Shelter};
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (stone, bricks) = (block(&server, "base:stone"), block(&server, "base:stone_bricks"));
        let build = |server: &mut Server, roof: BlockId| {
            for x in -3..=3 {
                for z in -3..=3 {
                    server.set_block(at + IVec3::new(x, 0, z), stone);
                    server.set_block(at + IVec3::new(x, 4, z), roof);
                    for y in 1..=3 {
                        let edge = x.abs() == 3 || z.abs() == 3;
                        server.set_block(at + IVec3::new(x, y, z), if edge { bricks } else { BlockId::AIR });
                    }
                }
            }
        };
        let home = at + IVec3::Y;
        let planks = block(&server, "base:oak_planks");
        build(&mut server, planks);
        let empty = server.room_walls(home);
        let furniture = ["base:oak_table", "base:oak_bed", "base:barrel", "base:chest", "base:oak_bookshelf", "base:cottage_stove", "base:loom_frame", "base:oak_shelf", "base:oak_chair", "base:crafting_table"];
        for (i, name) in furniture.iter().enumerate() {
            let (x, z) = ((i % 5) as i32 - 2, if i < 5 { 2 } else { -2 });
            server.set_block(at + IVec3::new(x, 1, z), block(&server, name));
        }
        let furnished = server.room_walls(home);
        assert_eq!(furnished, empty, "furniture changed the room's walls");
        assert!(furnished.share(Shelter::Sound) >= 0.9 && furnished.share(Shelter::Fine) >= 0.5, "not walled for Cosy and Heart: {furnished:?}");
        for roof in ["base:thatch_roof", "base:thatch_ridge", "base:oak_shingles", "base:oak_shingle_ridge", "base:oak_slab"] {
            let roof_block = block(&server, roof);
            build(&mut server, roof_block);
            let walls = server.room_walls(home);
            assert!(walls.share(Shelter::Sound) >= 0.9, "a {roof} ceiling is not sound: {walls:?}");
        }
    }

    /// A hearthstone set in its floor ring is floor, not wall: the room about it judged from
    /// the hearthstone and from the cell over it is the room of a plain stone floor. Cloth
    /// hung for walls and hay bales stacked wall a room in (draughty), though both are
    /// furnishings: a cloth room is closed, not open to the sky.
    #[test]
    fn a_hearth_in_its_floor_and_cloth_walls_wall_the_room_as_they_should() {
        use voxmar_engine::{block::BlockId, effect::Shelter};
        let mut server = crate::test_server();
        let at = voxmar_engine::coords::ColumnPos::of_block(server.spawn.floor().as_ivec3()).origin() + IVec3::new(8, server.spawn.y as i32 + 10, 8);
        let block = |server: &Server, name: &str| server.content.blocks.expect(name);
        let (stone, cobble, planks) = (block(&server, "base:stone"), block(&server, "base:cobblestone"), block(&server, "base:oak_planks"));
        let build = |server: &mut Server, wall: BlockId| {
            for x in -3..=3 {
                for z in -3..=3 {
                    server.set_block(at + IVec3::new(x, 0, z), stone);
                    server.set_block(at + IVec3::new(x, 4, z), planks);
                    for y in 1..=3 {
                        let edge = x.abs() == 3 || z.abs() == 3;
                        server.set_block(at + IVec3::new(x, y, z), if edge { wall } else { BlockId::AIR });
                    }
                }
            }
        };
        let bricks = block(&server, "base:stone_bricks");
        build(&mut server, bricks);
        let plain = server.room_walls(at + IVec3::Y);
        for x in -1..=1 {
            for z in -1..=1 {
                server.set_block(at + IVec3::new(x, 0, z), cobble);
            }
        }
        server.set_block(at, block(&server, "base:hearthstone"));
        assert_eq!(server.room_walls(at), plain, "the hearth's floor ring counted as walls");
        assert_eq!(server.room_walls(at + IVec3::Y), plain);
        // A barrel and a well set into the floor (over open air: the floor is one block
        // thick) are floor too: the scan does not step down through them and out.
        for (dx, name) in [(-2, "base:barrel"), (2, "base:well")] {
            server.set_block(at + IVec3::new(dx, 0, 2), block(&server, name));
        }
        assert_eq!(server.room_walls(at + IVec3::Y), plain, "furniture in the floor let the scan under the room");
        // Cloth hung and hay bales stacked are walls (their shelter tag wins over their
        // furnishing ones), and so is a stone column mid-wall in a plank room.
        for wall in ["base:white_cloth", "base:hay_bale"] {
            let wall_block = block(&server, wall);
            build(&mut server, wall_block);
            let walls = server.room_walls(at + IVec3::Y);
            assert!(walls.share(Shelter::Draughty) >= 0.99, "a {wall} room is open: {walls:?}");
        }
        build(&mut server, planks);
        for y in 1..=3 {
            server.set_block(at + IVec3::new(3, y, 0), block(&server, "base:stone_column"));
        }
        let walls = server.room_walls(at + IVec3::Y);
        assert!(walls.share(Shelter::Draughty) >= 0.99, "a column mid-wall is a hole: {walls:?}");
        // A crafting table (a station: furniture, no shelter tag) set in a gap in the wall
        // plugs nothing: the room is open there.
        server.set_block(at + IVec3::new(3, 1, 1), block(&server, "base:crafting_table"));
        let gapped = server.room_walls(at + IVec3::Y);
        assert!(gapped.share(Shelter::Draughty) < walls.share(Shelter::Draughty), "a station plugged a wall: {gapped:?}");
    }

    /// The load check on shelter tags: a solid `building` block with no shelter tag, or one
    /// with two, is refused by name; furniture (a furniture tag, no shelter tag) is let
    /// through, as is every shelter-tagged block.
    #[test]
    fn the_shelter_check_refuses_an_unjudged_wall_and_lets_furniture_through() {
        let server = crate::test_server();
        let rules = server.survival.clone();
        let parse = |source: &str| voxmar_engine::block::BlockRegistry::from_ron("test", source).unwrap();
        let ok = parse(r#"(blocks: [
            (name: "wall", textures: All("stone"), tags: ["building", "shelter:sound"]),
            (name: "chest", textures: All("stone"), tags: ["storage", "building"]),
            (name: "cloth", textures: All("stone"), tags: ["decor", "building", "shelter:draughty"]),
            (name: "dirt", textures: All("stone"), tags: ["natural"]),
        ])"#);
        assert_eq!(rules.check_shelters(&ok), Ok(()));
        let unjudged = parse(r#"(blocks: [(name: "wall", textures: All("stone"), tags: ["building"])])"#);
        let why = rules.check_shelters(&unjudged).unwrap_err();
        assert!(why.contains("test:wall"), "{why}");
        let twice = parse(r#"(blocks: [(name: "wall", textures: All("stone"), tags: ["roof", "shelter:sound", "shelter:fine"])])"#);
        assert!(rules.check_shelters(&twice).unwrap_err().contains("test:wall"));
        // A misspelt shelter tag, and a draught through anything but a draughty wall.
        let misspelt = parse(r#"(blocks: [(name: "rail", textures: All("stone"), solid: false, tags: ["building", "shelter:snug"])])"#);
        assert!(rules.check_shelters(&misspelt).unwrap_err().contains("test:rail (`shelter:snug` is no shelter)"));
        let sound = parse(r#"(blocks: [(name: "slit", textures: All("stone"), render: Cutout, tags: ["building", "shelter:sound", "draught"])])"#);
        assert!(rules.check_shelters(&sound).unwrap_err().contains("test:slit (`draught` but not `shelter:draughty`)"));
    }

    /// Time past a point of healing is not banked: however long the clock ran (buffs and a
    /// prayer stacked, then ended), a pass leaves at most one point's worth waiting.
    #[test]
    fn healing_past_a_point_is_not_banked_for_after() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = voxmar_engine::glam::Vec3::new(8.5, 300.0, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        Survival.run(&mut server);
        let player = server.players.get_mut(&1).unwrap();
        (player.health.current, player.heal_clock) = (1, 1000.0);
        Survival.run(&mut server);
        let player = &server.players[&1];
        let every = vitals::rates(&player.modifiers, 1.0, server.survival.heal_most).heal_every.expect("healing");
        assert_eq!(player.health.current, 2, "one point a pass");
        assert!(player.heal_clock <= every, "{} s banked against a point each {every} s", player.heal_clock);
    }

    /// A refresh (no time gone by, as sitting down asks for) heals nothing: the pass before
    /// left a point's worth waiting on the clock, and only time pays it out.
    #[test]
    fn a_refresh_after_a_pass_heals_nothing_more() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = voxmar_engine::glam::Vec3::new(8.5, 300.0, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        // Hitpoints 5: room to heal past one point.
        let hitpoints = server.content.skills.id("core:hitpoints").expect("hitpoints");
        player.skills.grant(hitpoints, server.content.skills.xp_for(5));
        server.players.insert(1, player);
        Survival.run(&mut server);
        let player = server.players.get_mut(&1).unwrap();
        assert!(player.health.max >= 4, "room to heal: most {}", player.health.max);
        (player.health.current, player.heal_clock) = (1, 1000.0);
        Survival.run(&mut server);
        assert_eq!(server.players[&1].health.current, 2, "one point a pass");
        Survival::refresh(&mut server, 1);
        assert_eq!(server.players[&1].health.current, 2, "a refresh pays out nothing");
    }

    /// A wound keeps the player from sleeping even if it heals back at once; the most coming
    /// down (a buff to it wearing off) is no wound.
    #[test]
    fn a_wound_counts_and_a_lowered_most_does_not() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        std::mem::forget(client);
        let at = voxmar_engine::glam::Vec3::new(8.5, 300.0, 8.5);
        let mut player = crate::player::Player::new(1, connection, at, 2, server.tick, server.survival.max_health, &server.config.limits);
        player.joined = true;
        server.players.insert(1, player);
        Survival.run(&mut server);
        let player = server.players.get_mut(&1).unwrap();
        let full = player.health.current;
        (player.health.max, player.health.current) = (full - 1, full - 1);
        Survival.run(&mut server);
        assert_eq!(server.players[&1].hurt_at, None, "the most came down: no wound");
        let tick = server.tick;
        let player = server.players.get_mut(&1).unwrap();
        let _ = player.wound(1, tick);
        player.health.heal(1);
        assert_eq!(player.hurt_at, Some(tick), "a wound healed back at once is a wound");
    }

    /// Every roof piece (thatch, shingles, their ridges) keeps the sky off whoever stands
    /// under it, as a solid block does: no rain soaks them, a rack under it does not dry, and
    /// they are indoors for a home.
    #[test]
    fn every_roof_piece_keeps_the_sky_off() {
        use voxmar_engine::glam::IVec3;
        let mut server = crate::test_server();
        let cell = server.spawn.floor().as_ivec3();
        server.load_area(cell - IVec3::new(4, 0, 4), cell + IVec3::new(4, 0, 4));
        for dy in 0..server.survival.roof_scan + 2 {
            server.set_block(cell + IVec3::Y * dy, voxmar_engine::block::BlockId::AIR);
        }
        assert!(server.open_sky(cell), "the test spot is open to the sky");
        let blocks = server.content.blocks.clone();
        let roofs: Vec<_> = blocks.ids().filter(|&id| blocks.def(id).has_tag("roof")).collect();
        assert!(roofs.len() >= 8, "thatch, shingles in three woods and their ridges: {}", roofs.len());
        for roof in roofs {
            server.set_block(cell + IVec3::Y * 3, roof);
            assert!(!server.open_sky(cell), "{} lets the sky through", blocks.def(roof).name);
        }
    }
}
