//! Processes: continuous rules over traits and fields, each running at its own tier.
//! Processes never decide; they change values and raise events (here: death).

use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{Vec2, Vec3};
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits::{self, Replication};
use voxmar_protocol::{PlayerId, ServerMessage};

use crate::Server;
use crate::player::MAX_HEALTH;
use crate::weather::Weather;

/// Falls shorter than this many blocks do no damage.
const SAFE_FALL: f32 = 3.0;
/// Below this temperature (°C), healing slows down.
const COLD: f32 = 0.0;

pub(crate) trait Process: Send {
    fn tier(&self) -> Tier;
    fn run(&mut self, server: &mut Server);
}

/// `wind` is the direction (x, z) the world's prevailing wind blows towards.
pub(crate) fn core_processes(seed: u64, wind: Vec2) -> Vec<Box<dyn Process>> {
    vec![Box::new(Falling), Box::new(Regeneration::default()), Box::new(Weather::new(seed, wind))]
}

/// Fall damage and the void, from the server's own view of where players stand.
struct Falling;

impl Process for Falling {
    fn tier(&self) -> Tier {
        Tier::High
    }

    fn run(&mut self, server: &mut Server) {
        let water = server.content.blocks.id("base:water");
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
            let in_water = aabb.intersects(|p| server.block(p).is_some() && server.block(p) == water);

            let player = server.players.get_mut(&id).unwrap();
            let mut damage = 0.0;
            if player.flying || in_water || !column_loaded {
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

/// Natural healing: one point every few seconds, slower where it is freezing. Reads the
/// temperature field, so anything that warms or cools a place affects it.
#[derive(Default)]
struct Regeneration {
    runs: u64,
}

impl Process for Regeneration {
    fn tier(&self) -> Tier {
        Tier::Low
    }

    fn run(&mut self, server: &mut Server) {
        self.runs += 1;
        let ids: Vec<PlayerId> = server.players.keys().copied().collect();
        for id in ids {
            let player = &server.players[&id];
            if !player.joined || !player.health.alive() || player.health.current >= player.health.max {
                continue;
            }
            let feet = player.position.floor().as_ivec3();
            let cold = server.temperature.sample(feet.x, feet.z) < COLD;
            let every = if cold { 8 } else { 4 };
            if self.runs.is_multiple_of(every) {
                let player = server.players.get_mut(&id).unwrap();
                player.health.heal(1);
                player.changed(traits::HEALTH);
            }
        }
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
                .send(ServerMessage::PlayerState { health: player.health, inventory: player.inventory.clone() });
        }
    }
}
