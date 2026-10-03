//! Player sessions: handshake, movement validation, actions and health.

use rustc_hash::FxHashSet;
use voxmar_engine::block::{BlockId, RenderKind};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory};
use voxmar_engine::physics::{Aabb, player as dims};
use voxmar_protocol::{
    Action, ClientMessage, PROTOCOL_VERSION, PlayerId, PlayerInput, PlayerView, ServerConnection, ServerMessage,
    TICKS_PER_SECOND,
};

use crate::Server;

pub const MAX_HEALTH: u8 = 20;
const REGEN_INTERVAL: u64 = 4 * TICKS_PER_SECOND as u64;
/// Falls shorter than this many blocks do no damage.
const SAFE_FALL: f32 = 3.0;
/// Fraction of the nominal break time accepted, to absorb latency and clock skew.
const BREAK_TOLERANCE: f32 = 0.7;

pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub connection: ServerConnection,
    pub joined: bool,
    pub position: Vec3,
    yaw: f32,
    selected: usize,
    flying: bool,
    health: u8,
    inventory: Inventory,
    /// Highest point of the current fall, if airborne.
    fall_start: Option<f32>,
    breaking: Option<(IVec3, u64)>,
    last_input_tick: u64,
    state_dirty: bool,
    pub sent: FxHashSet<ColumnPos>,
}

impl Player {
    pub fn new(id: PlayerId, connection: ServerConnection, spawn: Vec3) -> Self {
        Self {
            id,
            name: String::new(),
            connection,
            joined: false,
            position: spawn,
            yaw: 0.0,
            selected: 0,
            flying: false,
            health: MAX_HEALTH,
            inventory: Inventory::default(),
            fall_start: None,
            breaking: None,
            last_input_tick: 0,
            state_dirty: true,
            sent: FxHashSet::default(),
        }
    }

    pub fn view(&self) -> PlayerView {
        PlayerView { id: self.id, name: self.name.clone(), position: self.position, yaw: self.yaw }
    }

    fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, dims::WIDTH, dims::HEIGHT)
    }

    fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * dims::EYE_HEIGHT
    }

    fn alive(&self) -> bool {
        self.health > 0
    }
}

impl Server {
    pub(crate) fn handle_message(&mut self, id: PlayerId, message: ClientMessage) {
        match message {
            ClientMessage::Hello { name, protocol, content } => self.hello(id, name, protocol, content),
            _ if !self.players[&id].joined => {}
            ClientMessage::Input(input) => self.input(id, input),
            ClientMessage::Action(action) => self.action(id, action),
        }
    }

    fn hello(&mut self, id: PlayerId, name: String, protocol: u32, content: u64) {
        let fingerprint = self.registry.fingerprint();
        let start_kit: Vec<(BlockId, u8)> = [("base:oak_planks", 64), ("base:cobblestone", 64), ("base:glass", 32), ("base:glowstone", 16)]
            .into_iter()
            .filter_map(|(name, count)| self.registry.id(name).map(|id| (id, count)))
            .collect();
        let (seed, allow_flight) = (self.config.seed, self.config.allow_flight);
        let player = self.players.get_mut(&id).expect("message from unknown player");
        if player.joined {
            return;
        }
        let reason = if protocol != PROTOCOL_VERSION {
            Some(format!("protocol {protocol} is not supported (server speaks {PROTOCOL_VERSION})"))
        } else if content != fingerprint {
            Some("client and server content differ".to_owned())
        } else {
            None
        };
        if let Some(reason) = reason {
            player.connection.send(ServerMessage::Rejected { reason });
            return;
        }

        player.name = name.chars().filter(|c| !c.is_control()).take(32).collect();
        player.joined = true;
        for (block, count) in start_kit {
            player.inventory.add(block, count);
        }
        player.connection.send(ServerMessage::Welcome { player: id, seed, position: player.position, allow_flight });
        player.last_input_tick = self.tick;
        tracing::info!("{} joined", player.name);
    }

    fn input(&mut self, id: PlayerId, input: PlayerInput) {
        let tick = self.tick;
        let allow_flight = self.config.allow_flight;
        let player = &self.players[&id];
        if !player.alive() || !input.position.is_finite() {
            return;
        }
        let elapsed = ((tick - player.last_input_tick) as f32 / TICKS_PER_SECOND as f32).clamp(0.05, 1.0);
        let delta = input.position - player.position;
        let flying = input.flying && allow_flight;
        let max_horizontal = if flying { dims::FLY_SPRINT_SPEED } else { dims::SPRINT_SPEED } * 1.5 * elapsed + 0.5;
        let max_vertical = if flying { dims::FLY_SPRINT_SPEED } else { dims::TERMINAL_VELOCITY } * 1.5 * elapsed + 1.5;
        let aabb = Aabb::standing(input.position, dims::WIDTH * 0.9, dims::HEIGHT * 0.95);
        let inside_wall = aabb.intersects(|p| self.is_solid(p));
        let plausible = delta.with_y(0.0).length() <= max_horizontal && delta.y.abs() <= max_vertical && !inside_wall;

        let player = self.players.get_mut(&id).unwrap();
        player.yaw = input.yaw;
        player.flying = flying;
        player.selected = (input.selected_slot as usize).min(HOTBAR_SLOTS - 1);
        if plausible {
            player.position = input.position;
            player.last_input_tick = tick;
        } else {
            player.connection.send(ServerMessage::Teleport { position: player.position });
        }
    }

    fn action(&mut self, id: PlayerId, action: Action) {
        if !self.players[&id].alive() {
            if matches!(action, Action::Respawn) {
                self.respawn(id);
            }
            return;
        }
        match action {
            Action::StartBreaking { block } => {
                let tick = self.tick;
                if self.in_reach(id, block) {
                    self.players.get_mut(&id).unwrap().breaking = Some((block, tick));
                }
            }
            Action::FinishBreaking { block } => self.finish_breaking(id, block),
            Action::Place { block, face } => self.place(id, block + face.normal()),
            Action::MoveItem { from, to } => {
                let player = self.players.get_mut(&id).unwrap();
                player.state_dirty |= player.inventory.move_stack(from as usize, to as usize);
            }
            Action::Respawn => {}
        }
    }

    fn finish_breaking(&mut self, id: PlayerId, pos: IVec3) {
        let Some(current) = self.block(pos) else { return };
        let info = *self.registry.info(current);
        let started = self.players[&id].breaking.filter(|(p, _)| *p == pos).map(|(_, t)| t);
        let required = (info.hardness * TICKS_PER_SECOND as f32 * BREAK_TOLERANCE) as u64;
        let valid = info.hardness >= 0.0
            && !current.is_air()
            && self.in_reach(id, pos)
            && started.is_some_and(|t| self.tick - t >= required);
        if !valid {
            self.resync_block(id, pos);
            return;
        }
        self.set_block(pos, BlockId::AIR);
        let player = self.players.get_mut(&id).unwrap();
        player.breaking = None;
        player.inventory.add(info.drops, 1);
        player.state_dirty = true;
    }

    fn place(&mut self, id: PlayerId, pos: IVec3) {
        let player = &self.players[&id];
        let item = player.inventory.get(player.selected).map(|s| s.item);
        let replaceable = self
            .block(pos)
            .is_some_and(|b| b.is_air() || self.registry.info(b).render == RenderKind::Translucent && !self.registry.info(b).solid);
        let blocks_player = item.is_some_and(|b| self.registry.info(b).solid)
            && self.players.values().any(|p| p.alive() && p.aabb().intersects(|c| c == pos));
        let Some(item) = item.filter(|_| replaceable && !blocks_player && self.in_reach(id, pos)) else {
            self.resync_block(id, pos);
            return;
        };
        self.set_block(pos, item);
        let player = self.players.get_mut(&id).unwrap();
        player.inventory.take_one(player.selected);
        player.state_dirty = true;
    }

    fn resync_block(&self, id: PlayerId, pos: IVec3) {
        if let Some(block) = self.block(pos) {
            self.players[&id].connection.send(ServerMessage::BlockChanged { pos, block });
        }
    }

    fn in_reach(&self, id: PlayerId, pos: IVec3) -> bool {
        self.players[&id].eye().distance(pos.as_vec3() + 0.5) <= dims::REACH + 1.5
    }

    fn is_solid(&self, pos: IVec3) -> bool {
        self.block(pos).is_some_and(|b| self.registry.info(b).solid)
    }

    fn respawn(&mut self, id: PlayerId) {
        let spawn = self.spawn;
        let player = self.players.get_mut(&id).unwrap();
        player.health = MAX_HEALTH;
        player.position = spawn;
        player.fall_start = None;
        player.state_dirty = true;
        player.connection.send(ServerMessage::Teleport { position: spawn });
    }

    /// Per-tick player processes: fall damage, the void, regeneration, state sync.
    pub(crate) fn tick_player(&mut self, id: PlayerId) {
        let tick = self.tick;
        let player = &self.players[&id];
        if !player.joined {
            return;
        }
        let aabb = player.aabb();
        let column_loaded = self.world.contains(ColumnPos::of_block(player.position.floor().as_ivec3()));
        let mut below = aabb;
        below.translate(Vec3::new(0.0, -0.05, 0.0));
        let on_ground = below.intersects(|p| self.is_solid(p));
        let water = self.registry.id("base:water");
        let in_water = aabb.intersects(|p| self.block(p).is_some() && self.block(p) == water);

        let player = self.players.get_mut(&id).unwrap();
        if player.alive() {
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
                player.health = player.health.saturating_sub(damage as u8);
                player.state_dirty = true;
                if player.health == 0 {
                    player.connection.send(ServerMessage::Died);
                }
            } else if player.health < MAX_HEALTH && tick.is_multiple_of(REGEN_INTERVAL) {
                player.health += 1;
                player.state_dirty = true;
            }
        }
        if player.state_dirty {
            player.state_dirty = false;
            player
                .connection
                .send(ServerMessage::PlayerState { health: player.health, inventory: player.inventory.clone() });
        }
    }
}
