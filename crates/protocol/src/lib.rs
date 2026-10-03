//! Client/server messages and the transports that carry them.
//!
//! The server is authoritative. Clients send intent (input and proposed actions);
//! the server validates, commits and replicates the result. The same message types travel
//! over an in-process channel (singleplayer) or TCP (multiplayer).

mod transport;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use voxmar_engine::action::{ActionId, ActionTarget};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::inventory::Inventory;
use voxmar_engine::traits::Health;
use voxmar_engine::world::Column;

pub use transport::{ClientConnection, Connection, Disconnected, ServerConnection, local_pair, tcp};

pub const PROTOCOL_VERSION: u32 = 4;
pub const DEFAULT_PORT: u16 = 25_665;
/// Server simulation rate.
pub const TICKS_PER_SECOND: u32 = 20;
/// In-game day length.
pub const TICKS_PER_DAY: u64 = 24_000;
/// Highest flight speed multiplier an admin may choose.
pub const MAX_FLY_SPEED: f32 = 4.0;

pub type PlayerId = u32;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello { name: String, protocol: u32, content: u64 },
    Input(PlayerInput),
    /// Propose an action; it replaces any action still in progress.
    Propose { action: ActionId, target: ActionTarget },
    /// Abandon the action in progress (for example, the mouse button was released).
    Cancel,
    /// The player's preferences: streaming radius in columns (capped by the server) and
    /// flight speed multiplier (honoured for admins only).
    Settings { view_distance: u8, fly_speed: f32 },
}

/// The client's predicted movement state, sent every tick. The server checks it against
/// its own copy of the world and corrects the client if it is implausible.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PlayerInput {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub selected_slot: u8,
    pub flying: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Where the player is (as they left it, in a saved world), and what they may do.
    Welcome { player: PlayerId, seed: u64, position: Vec3, yaw: f32, flying: bool, allow_flight: bool, admin: bool },
    Rejected { reason: String },
    Column { pos: ColumnPos, column: Arc<Column> },
    Unload { pos: ColumnPos },
    BlockChanged { pos: IVec3, block: BlockId },
    PlayerState { health: Health, inventory: Inventory },
    /// Authoritative position, after a rejected move or a respawn.
    Teleport { position: Vec3 },
    Players { players: Vec<PlayerView> },
    Time { ticks: u64 },
    /// The weather where the player is, sent when it changes and every few seconds.
    Weather(Weather),
    /// Lightning over (x, z): a flash, and thunder once the sound has travelled.
    Lightning { at: Vec2 },
    Died,
}

/// How other players see a player.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: PlayerId,
    pub name: String,
    pub position: Vec3,
    pub yaw: f32,
}

/// The weather at one place. Clients ease towards each update, so it may change in steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    /// Fraction of the sky covered by low cloud, 0..1.
    pub cover: f32,
    /// How that cloud is built: 0 flat layers (stratus) to 1 heaped towers (cumulus, and
    /// cumulonimbus when it rains).
    pub convection: f32,
    /// High, thin ice cloud (cirrus), 0..1. It often runs ahead of rain.
    pub cirrus: f32,
    /// Relative humidity of the air, 0..1.
    pub humidity: f32,
    /// Rain or snow reaching the ground, 0 (none) to 1 (downpour).
    pub precipitation: f32,
    /// How much of it falls as snow, 0..1.
    pub snow: f32,
    /// Wind along x and z, in blocks per second.
    pub wind: Vec2,
}
