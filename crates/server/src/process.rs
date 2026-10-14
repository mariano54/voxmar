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
        Box::new(Weather::default()),
        Box::new(crate::wear::Footsteps::default()),
        Box::new(crate::farm::Growth),
        Box::new(crate::machines::Machines),
        Box::new(crate::apiary::Bees::default()),
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
            let on_ground = below.intersects(|p| server.is_solid(p));
            // Any state of the fluid counts: flowing water (any fluid swum in) breaks a fall,
            // flowing lava burns.
            let touching = |kind: &dyn Fn(FluidId) -> bool| aabb.intersects(|p| server.block(p).and_then(|b| blocks.fluid(b)).is_some_and(|f| kind(f.fluid)));
            let (in_water, in_lava) = (touching(&|f| blocks.fluid_def(f).def.swim), touching(&|f| is_lava(blocks, f)));

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
            if player.flying || in_water || in_lava || !column_loaded {
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
                let died = player.health.damage(damage as u8);
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
        let effects = server.content.effects.clone();
        let hitpoints = server.content.skills.id("core:hitpoints");
        // Steps of a home reached this second, counted for quests (`Built("home_snug", 1)`, qualified by the effect's package).
        let mut reached: Vec<(PlayerId, String)> = Vec::new();
        let ids: Vec<PlayerId> = server.players.keys().copied().collect();
        for id in ids {
            let player = &server.players[&id];
            if !player.joined || !player.health.alive() {
                continue;
            }
            let feet = player.position.floor().as_ivec3();
            let air = server.temperature.sample(feet.x, feet.z);
            let heat = server.heat_near(feet);
            let wet = server.block(feet).and_then(|b| server.content.blocks.fluid(b)).is_some();
            let outdoors = server.open_sky(feet + voxmar_engine::glam::IVec3::Y * 2);
            let at = voxmar_engine::glam::Vec2::new(player.position.x, player.position.z);
            let (tick, day) = (server.tick, server.day);
            let weather = server.weather.local(&mut server.temperature, at, tick, day).weather;
            let cold_air = air - if wet { server.survival.water_chill } else { 0.0 };
            let warmed = heat > 0.5 && cold_air < vitals::CHILLY;
            let home = server.home_survey(id);
            // What effects ask of the item in hand (`Holding`) and of the blocks near
            // (`Near`).
            let held: Vec<String> = player.held().map_or_else(Vec::new, |item| {
                let def = server.content.blocks.def(item);
                effects.held_tags().iter().filter(|tag| def.has_tag(tag)).cloned().collect()
            });
            let near: Vec<(String, i32)> = effects.near_asks().iter().filter(|(tag, reach)| server.tagged_within(feet, tag, *reach)).cloned().collect();
            let max_level = hitpoints.map(|skill| server.content.skills.level(server.players[&id].skills.xp(skill)));
            // Mending (a prayer) brings health back faster.
            let mending = server.content.prayers.mending(&server.players[&id].praying.on);

            let player = server.players.get_mut(&id).unwrap();
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
            let mut situation = Situation { felt: cold_air + heat, warmed, outdoors, weather: &weather, biome: &biome.name, biome_tags: &biome.tags, home: home.as_ref(), done: &player.quests_done, held: &held, near: &near };
            // What clings (smoke) is topped up while it holds, and lasts after.
            effects.linger(&situation, &mut player.timed_effects);
            let first = effects.active(&situation, &player.timed_effects);
            situation.felt = cold_air + heat + effects.modifiers(&first).warmth;
            let active = effects.active(&situation, &player.timed_effects);
            let modifiers = effects.modifiers(&active);
            player.modifiers = modifiers;
            let rates = vitals::rates(&modifiers);
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
            // Healing, while anything is left to heal.
            match rates.heal_every.filter(|_| player.health.current < player.health.max) {
                Some(every) => {
                    player.heal_clock += seconds * mending;
                    if player.heal_clock >= every {
                        player.heal_clock -= every;
                        player.health.heal(1);
                    }
                }
                None => player.heal_clock = 0.0,
            }
            match rates.hurt_every.filter(|_| !creative) {
                Some(every) => {
                    player.hurt_clock += seconds;
                    if player.hurt_clock >= every {
                        player.hurt_clock -= every;
                        if player.health.damage(1) {
                            player.connection.send(ServerMessage::Died);
                        }
                    }
                }
                None => player.hurt_clock = 0.0,
            }
            // The felt temperature is sent to the degree, so it does not resend every tick.
            player.vitals.felt = situation.felt.round();
            player.vitals.effects = effects.active(&situation, &player.timed_effects);
            player.vitals.home = player.home.or(player.bed).map(|at| at.floor().as_ivec3());
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
        let player = self.players.get(&id)?;
        let anchor = player.home.or(player.bed)?.floor().as_ivec3();
        let rules = &self.survival;
        let (up, down) = (rules.home_up, rules.home_down);
        let feet = player.position.floor().as_ivec3();
        let near = (feet - anchor).abs();
        if near.x > rules.home_reach || near.z > rules.home_reach || feet.y > anchor.y + up || feet.y < anchor.y - down {
            return None;
        }
        Some(self.survey_around(anchor, Some(id)))
    }

    /// What lies around a home at `anchor`, whoever stands near it: its blocks by tag and the
    /// animals `keeper` keeps within its reach (`kept`).
    pub(crate) fn survey_around(&self, anchor: IVec3, keeper: Option<PlayerId>) -> HomeSurvey {
        let rules = &self.survival;
        let (up, down) = (rules.home_up, rules.home_down);
        let wanted = self.content.effects.home_tags();
        let mut counts: Vec<(String, u32)> = wanted.iter().map(|tag| (tag.clone(), 0)).collect();
        if wanted.is_empty() {
            return HomeSurvey { counts };
        }
        let blocks = &self.content.blocks;
        let reach = rules.home_reach;
        for dy in -down..=up {
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    let Some(block) = self.block(anchor + IVec3::new(dx, dy, dz)) else { continue };
                    let tags = &blocks.def(block).tags;
                    if tags.is_empty() {
                        continue;
                    }
                    for (tag, n) in &mut counts {
                        if tags.contains(tag) {
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
        HomeSurvey { counts }
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
