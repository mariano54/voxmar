//! Processes: continuous rules over traits and fields, each running at its own tier.
//! Processes never decide; they change values and raise events (here: death).

use voxmar_engine::coords::ColumnPos;
use voxmar_engine::effect::Situation;
use voxmar_engine::fluid::FluidId;
use voxmar_engine::glam::Vec3;
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits::{self, Replication};
use voxmar_engine::vitals;
use voxmar_protocol::{PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::player::MAX_HEALTH;
use crate::weather::Weather;

/// Falls shorter than this many blocks do no damage.
const SAFE_FALL: f32 = 3.0;
/// Lava burns this much health every `BURN_TICKS`.
const BURN: f32 = 4.0;
const BURN_TICKS: u64 = TICKS_PER_SECOND as u64 / 2;

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
    ]
}

/// Fall damage, burning in lava and the void, from the server's own view of where
/// players stand.
struct Falling;

impl Process for Falling {
    fn tier(&self) -> Tier {
        Tier::High
    }

    fn run(&mut self, server: &mut Server) {
        let blocks = &server.content.blocks;
        let fluid = |name: &str| blocks.id(name).and_then(|b| blocks.fluid(b)).map(|f| f.fluid);
        let (water, lava) = (fluid("base:water"), fluid("base:lava"));
        let burns = server.tick.is_multiple_of(BURN_TICKS);
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
            // Any state of the fluid counts: flowing water breaks a fall, flowing lava burns.
            let touching = |fluid: Option<FluidId>| {
                fluid.is_some() && aabb.intersects(|p| server.block(p).and_then(|b| blocks.fluid(b)).is_some_and(|f| Some(f.fluid) == fluid))
            };
            let (in_water, in_lava) = (touching(water), touching(lava));

            let player = server.players.get_mut(&id).unwrap();
            if on_ground && !player.flying {
                player.grounded = tick;
            }
            let mut damage = 0.0;
            if in_lava && burns {
                damage += BURN;
            }
            if player.flying || in_water || in_lava || !column_loaded {
                player.fall_start = None;
            } else if on_ground {
                if let Some(start) = player.fall_start.take() {
                    damage += (start - player.position.y - SAFE_FALL).max(0.0).ceil();
                }
            } else {
                let y = player.position.y;
                player.fall_start = Some(player.fall_start.map_or(y, |s| s.max(y)));
            }
            if player.position.y < -16.0 {
                damage += MAX_HEALTH as f32;
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
struct Survival;

/// How far a fire or torch warms, in blocks, and how much at its heart (°C).
const HEAT_REACH: i32 = 4;
const HEAT: f32 = 14.0;
/// How much colder water feels (°C).
const WATER_CHILL: f32 = 8.0;
/// How far up a roof is looked for, to tell whether a player is out in the weather.
const ROOF_SCAN: i32 = 48;

impl Process for Survival {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let seconds = Tier::Low.period() as f32 / TICKS_PER_SECOND as f32;
        let effects = server.content.effects.clone();
        let hitpoints = server.content.skills.id("core:hitpoints");
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
            let cold_air = air - if wet { WATER_CHILL } else { 0.0 };
            let warmed = heat > 0.5 && cold_air < vitals::CHILLY;
            let max_level = hitpoints.map(|skill| server.content.skills.level(server.players[&id].skills.xp(skill)));

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
            let mut situation = Situation { felt: cold_air + heat, warmed, outdoors, weather: &weather, biome: &biome.name, biome_tags: &biome.tags };
            let first = effects.active(&situation, &player.timed_effects);
            situation.felt = cold_air + heat + effects.modifiers(&first).warmth;
            let active = effects.active(&situation, &player.timed_effects);
            let modifiers = effects.modifiers(&active);
            player.modifiers = modifiers;
            let rates = vitals::rates(&modifiers);
            // Twice the Hitpoints level, and what effects add: what a raised most brings is
            // health to fill it, what a lowered one takes goes.
            let max = (max_level.map_or(MAX_HEALTH as i32, |level| 2 * level as i32) + modifiers.max_health as i32).clamp(1, u8::MAX as i32) as u8;
            if max != player.health.max {
                let gained = max.saturating_sub(player.health.max);
                player.health.max = max;
                player.health.current = player.health.current.saturating_add(gained).min(max);
            }
            // Healing, while anything is left to heal.
            match rates.heal_every.filter(|_| player.health.current < player.health.max) {
                Some(every) => {
                    player.heal_clock += seconds;
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
            if player.vitals != before.0 {
                player.changed(traits::VITALS);
            }
            if player.health != before.1 {
                player.changed(traits::HEALTH);
            }
        }
    }
}

impl Server {
    /// Whether nothing solid hangs over `at` (within `ROOF_SCAN` blocks): out in the rain.
    fn open_sky(&self, at: voxmar_engine::glam::IVec3) -> bool {
        (0..ROOF_SCAN).map(|dy| at + voxmar_engine::glam::IVec3::Y * dy).all(|p| !self.is_solid(p))
    }

    /// How much fires, torches and other glowing blocks near `at` warm it (°C): the
    /// brightest within reach, fading with distance.
    fn heat_near(&self, at: voxmar_engine::glam::IVec3) -> f32 {
        let blocks = &self.content.blocks;
        let mut heat = 0.0f32;
        for dy in -1..=2 {
            for dz in -HEAT_REACH..=HEAT_REACH {
                for dx in -HEAT_REACH..=HEAT_REACH {
                    let pos = at + voxmar_engine::glam::IVec3::new(dx, dy, dz);
                    let Some(block) = self.block(pos) else { continue };
                    let emission = blocks.info(block).emission;
                    if emission < 12 {
                        continue;
                    }
                    let distance = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                    let falloff = (1.0 - distance / (HEAT_REACH as f32 + 1.0)).max(0.0);
                    heat = heat.max(HEAT * falloff * emission as f32 / 15.0);
                }
            }
        }
        heat
    }
}

impl Server {
    pub(crate) fn run_processes(&mut self) {
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
