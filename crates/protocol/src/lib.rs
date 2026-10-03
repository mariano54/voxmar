//! Client/server messages and the transports that carry them.
//!
//! The server is authoritative. Clients send intent (input and proposed [`Action`]s);
//! the server validates, commits and replicates the result. The same message types travel
//! over an in-process channel (singleplayer) or TCP (multiplayer).

mod transport;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::{ColumnPos, Face};
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::Inventory;
use voxmar_engine::world::Column;

pub use transport::{ClientConnection, Connection, ServerConnection, local_pair, tcp};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 25_665;
/// Server simulation rate.
pub const TICKS_PER_SECOND: u32 = 20;
/// In-game day length.
pub const TICKS_PER_DAY: u64 = 24_000;

pub type PlayerId = u32;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello { name: String, protocol: u32, content: u64 },
    Input(PlayerInput),
    Action(Action),
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

/// A change the player proposes. The server may reject any of them.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Action {
    StartBreaking { block: IVec3 },
    FinishBreaking { block: IVec3 },
    /// Place the selected item against `face` of `block`.
    Place { block: IVec3, face: Face },
    MoveItem { from: u8, to: u8 },
    Respawn,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome { player: PlayerId, seed: u64, position: Vec3, allow_flight: bool },
    Rejected { reason: String },
    Column { pos: ColumnPos, column: Arc<Column> },
    Unload { pos: ColumnPos },
    BlockChanged { pos: IVec3, block: BlockId },
    PlayerState { health: u8, inventory: Inventory },
    /// Authoritative position, after a rejected move or a respawn.
    Teleport { position: Vec3 },
    Players { players: Vec<PlayerView> },
    Time { ticks: u64 },
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
