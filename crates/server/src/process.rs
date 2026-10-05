//! Processes: continuous rules over traits and fields, each running at its own tier.
//! Processes never decide; they change values and raise events (here: death).

use voxmar_engine::coords::ColumnPos;
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
}

pub(crate) fn core_processes() -> Vec<Box<dyn Process>> {
    vec![
        Box::new(Falling),
        Box::new(Survival),
        Box::new(Weather::default()),
        Box::new(crate::wear::Footsteps::default()),
        Box::new(crate::farm::Growth),
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

/// Survival: hunger empties, warmth follows the air, fires and water, and together they
/// decide each player's effects (see `voxmar_engine::vitals`), which heal or hurt them
/// over time. Once a second; creative players neither hunger nor hurt.
#[derive(Default)]
struct Survival;

/// How far a fire or torch warms, in blocks, and how much at its heart (°C).
const HEAT_REACH: i32 = 4;
const HEAT: f32 = 14.0;
/// How much colder water feels (°C).
const WATER_CHILL: f32 = 8.0;

impl Process for Survival {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        let seconds = Tier::Low.period() as f32 / TICKS_PER_SECOND as f32;
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
            let cold_air = air - if wet { WATER_CHILL } else { 0.0 };
            let felt = cold_air + heat;
            let warmed = heat > 0.5 && cold_air < vitals::CHILLY;

            let player = server.players.get_mut(&id).unwrap();
            let creative = player.allow_flight;
            let before = player.vitals.clone();
            if creative {
                player.vitals.hunger = vitals::MAX_HUNGER;
            }
            let effects = vitals::effects(player.vitals.hunger, felt, warmed);
            let rates = vitals::rates(&effects, false);
            if !creative {
                player.exhaustion += rates.hunger_per_minute * seconds / 60.0;
            }
            // Healing, while anything is left to heal.
            match rates.heal_every.filter(|_| player.health.current < player.health.max) {
                Some(every) => {
                    player.heal_clock += seconds;
                    if player.heal_clock >= every {
                        player.heal_clock -= every;
                        player.health.heal(1);
                        player.exhaustion += vitals::HEAL_COST;
                        player.changed(traits::HEALTH);
                    }
                }
                None => player.heal_clock = 0.0,
            }
            match rates.hurt_every.filter(|_| !creative) {
                Some(every) => {
                    player.hurt_clock += seconds;
                    if player.hurt_clock >= every {
                        player.hurt_clock -= every;
                        let died = player.health.damage(1);
                        player.changed(traits::HEALTH);
                        if died {
                            player.connection.send(ServerMessage::Died);
                        }
                    }
                }
                None => player.hurt_clock = 0.0,
            }
            while player.exhaustion >= 1.0 {
                player.exhaustion -= 1.0;
                player.vitals.hunger = player.vitals.hunger.saturating_sub(1);
            }
            // The felt temperature is sent to the degree, so it does not resend every tick.
            player.vitals.felt = felt.round();
            player.vitals.effects = vitals::effects(player.vitals.hunger, felt, warmed);
            if player.vitals != before {
                player.changed(traits::VITALS);
            }
        }
    }
}

impl Server {
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
            if process.tier().due(self.tick) {
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
