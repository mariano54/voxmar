//! Players: agents whose controller is a remote input device. A player's session handles
//! the handshake and validates movement; everything else they do goes through actions.

use rustc_hash::FxHashSet;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::Vec3;
use voxmar_engine::inventory::{HOTBAR_SLOTS, Inventory};
use voxmar_engine::physics::{Aabb, player as dims};
use voxmar_engine::traits::{self, Health, TraitDecl};
use voxmar_protocol::{
    ClientMessage, PROTOCOL_VERSION, PlayerId, PlayerInput, PlayerView, ServerConnection, ServerMessage, TICKS_PER_SECOND,
};

use crate::Server;

pub const MAX_HEALTH: u8 = 20;

pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub connection: ServerConnection,
    pub joined: bool,
    pub position: Vec3,
    pub yaw: f32,
    pub selected: usize,
    pub flying: bool,
    pub health: Health,
    pub inventory: Inventory,
    /// Highest point of the current fall, if airborne.
    pub fall_start: Option<f32>,
    pub last_input_tick: u64,
    /// Owner-replicated traits changed since they were last sent.
    pub dirty: Vec<TraitDecl>,
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
            health: Health::full(MAX_HEALTH),
            inventory: Inventory::default(),
            fall_start: None,
            last_input_tick: 0,
            dirty: vec![traits::HEALTH, traits::INVENTORY],
            sent: FxHashSet::default(),
        }
    }

    pub fn view(&self) -> PlayerView {
        PlayerView { id: self.id, name: self.name.clone(), position: self.position, yaw: self.yaw }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, dims::WIDTH, dims::HEIGHT)
    }

    pub fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * dims::EYE_HEIGHT
    }

    pub fn held(&self) -> Option<BlockId> {
        self.inventory.get(self.selected).map(|stack| stack.item)
    }

    pub fn changed(&mut self, decl: TraitDecl) {
        if !self.dirty.iter().any(|d| d.name == decl.name) {
            self.dirty.push(decl);
        }
    }
}

impl Server {
    pub(crate) fn handle_message(&mut self, id: PlayerId, message: ClientMessage) {
        match message {
            ClientMessage::Hello { name, protocol, content } => self.hello(id, name, protocol, content),
            _ if !self.players[&id].joined => {}
            ClientMessage::Input(input) => self.input(id, input),
            ClientMessage::Propose { action, target } => self.propose(id, action, target),
            ClientMessage::Cancel => self.cancel_action(id),
        }
    }

    fn hello(&mut self, id: PlayerId, name: String, protocol: u32, content: u64) {
        let fingerprint = self.content.fingerprint();
        let blocks = &self.content.blocks;
        let start_kit: Vec<(BlockId, u8)> =
            [("base:oak_planks", 64), ("base:cobblestone", 64), ("base:glass", 32), ("base:glowstone", 16)]
                .into_iter()
                .filter_map(|(name, count)| blocks.id(name).map(|id| (id, count)))
                .collect();
        let (seed, allow_flight, tick) = (self.config.seed, self.config.allow_flight, self.tick);
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
        player.changed(traits::INVENTORY);
        player.connection.send(ServerMessage::Welcome { player: id, seed, position: player.position, allow_flight });
        player.last_input_tick = tick;
        tracing::info!("{} joined", player.name);
    }

    /// Accepts the client's predicted movement if it is physically plausible, otherwise
    /// snaps the client back.
    fn input(&mut self, id: PlayerId, input: PlayerInput) {
        let tick = self.tick;
        let allow_flight = self.config.allow_flight;
        let player = &self.players[&id];
        if !player.health.alive() || !input.position.is_finite() {
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
}
