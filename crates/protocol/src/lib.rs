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
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::Inventory;
use voxmar_engine::traits::Health;
use voxmar_engine::world::Column;

pub use transport::{ClientConnection, Connection, Disconnected, ServerConnection, local_pair, tcp};
pub use voxmar_engine::weather::Weather;

pub const PROTOCOL_VERSION: u32 = 7;
pub const DEFAULT_PORT: u16 = 25_665;
/// Server simulation rate.
pub const TICKS_PER_SECOND: u32 = 20;
/// In-game day length.
pub const TICKS_PER_DAY: u64 = 24_000;
/// Highest flight speed multiplier an admin may choose.
pub const MAX_FLY_SPEED: f32 = 4.0;
/// Longest console line, in characters, a server accepts.
pub const MAX_LINE: usize = 256;

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
    /// A command typed into the console (`time set noon`, with or without the `/`). The
    /// server checks the player may run it, runs it and replies with `Chat` lines.
    Command { line: String },
    /// Something said to everyone in the console.
    Chat { text: String },
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
    /// `weather_seed` seeds the gusts of wind (`voxmar_engine::weather::gust`), which
    /// clients work out themselves.
    Welcome {
        player: PlayerId,
        seed: u64,
        weather_seed: u64,
        position: Vec3,
        yaw: f32,
        flying: bool,
        allow_flight: bool,
        admin: bool,
    },
    Rejected { reason: String },
    Column { pos: ColumnPos, column: Arc<Column> },
    Unload { pos: ColumnPos },
    BlockChanged { pos: IVec3, block: BlockId },
    /// Blocks changed together in one tick (water and lava flowing), in the order they
    /// changed, in columns the player has.
    Blocks { changes: Vec<(IVec3, BlockId)> },
    PlayerState { health: Health, inventory: Inventory },
    /// Authoritative position, after a rejected move or a respawn.
    Teleport { position: Vec3 },
    Players { players: Vec<PlayerView> },
    /// The clocks, every second and whenever a command changes them: `tick` counts
    /// simulation ticks since the world began, at `TICKS_PER_SECOND`, and `day` is the
    /// time of day in ticks (0 at sunrise of the first day), which runs `day_speed` times
    /// as fast and may be set or frozen.
    Time { tick: u64, day: u64, day_speed: f32 },
    /// What the player may now do, after an admin changed it (`/gamemode`).
    Abilities { allow_flight: bool, admin: bool },
    /// A line for the player's console: a reply to their command, or chat.
    Chat { kind: ChatKind, text: String },
    /// The weather where the player is, sent when it changes and every few seconds, with
    /// the names of the effects showing (`valley_fog`, `hail`), strongest first. `quick`
    /// asks the client to change to it within a second or two rather than ease in slowly,
    /// after a command changed it.
    Weather { weather: Weather, effects: Vec<String>, quick: bool },
    /// Lightning striking the ground at `at`: a flash, and thunder once the sound has
    /// travelled. `seed` shapes the bolt, so every client draws the same one.
    Lightning { at: Vec3, seed: u32 },
    Died,
}

/// What a console line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatKind {
    /// Said by a player; the text starts with their name.
    Chat,
    /// A command's answer, or news from the server.
    Info,
    /// A command that could not run, and why.
    Error,
}

/// How other players see a player.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: PlayerId,
    pub name: String,
    pub position: Vec3,
    pub yaw: f32,
}
