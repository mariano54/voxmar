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
use voxmar_engine::creature::CreatureId;
use voxmar_engine::creature::brain::Pose;
use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::inventory::{Inventory, ItemStack};
use voxmar_engine::lang::Words;
use voxmar_engine::skill::{SkillId, Skills};
use voxmar_engine::vitals::Vitals;
use voxmar_engine::traits::Health;
use voxmar_engine::world::Column;

pub use transport::{ClientConnection, Connection, Disconnected, ServerConnection, local_pair, tcp};
pub use voxmar_engine::weather::Weather;

pub const PROTOCOL_VERSION: u32 = 14;
pub const DEFAULT_PORT: u16 = 25_665;
/// Server simulation rate.
pub const TICKS_PER_SECOND: u32 = 20;
/// In-game day length.
pub const TICKS_PER_DAY: u64 = 24_000;
/// Blocks across a cell of the weather's simulation.
pub const WEATHER_CELL: i32 = 128;
/// Thunder is heard from lightning closer than this many blocks; beyond, a flash is silent.
pub const THUNDER_HEARD: f32 = 1300.0;
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
    /// The action in progress is done by the player's own clock (a block shows broken):
    /// the next proposal must not cancel it, however late the server's own clock runs.
    Finished,
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
    Rejected { reason: Words },
    Column { pos: ColumnPos, column: Arc<Column> },
    Unload { pos: ColumnPos },
    BlockChanged { pos: IVec3, block: BlockId },
    /// Blocks changed together in one tick (water and lava flowing), in the order they
    /// changed, in columns the player has.
    Blocks { changes: Vec<(IVec3, BlockId)> },
    PlayerState { health: Health, inventory: Inventory, skills: Skills, vitals: Vitals },
    /// The player earned `tenths` of experience in `skill`, for the interface to show.
    Xp { skill: SkillId, tenths: u32 },
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
    /// A line for the player's console: a reply to their command, or chat, in words the
    /// client puts into the player's language.
    Chat { kind: ChatKind, text: Words },
    /// The weather where the player is, sent when it changes and every few seconds, with
    /// the names of the effects showing (`valley_fog`, `hail`), strongest first, and the
    /// rain around, to show storms coming. `quick` asks the client to change to it within
    /// a second or two rather than ease in slowly, after a command changed it.
    Weather { weather: Weather, effects: Vec<String>, nearby: Nearby, quick: bool },
    /// Lightning at `at`: a flash, and thunder once the sound has travelled. It strikes
    /// the ground there, or with `cloud` flashes inside the cloud. `seed` shapes the bolt,
    /// so every client draws the same one.
    Lightning { at: Vec3, seed: u32, cloud: bool },
    Died,
    /// Dropped items near the player that appeared or changed, and those that went: picked
    /// up (by `collector`), merged into another, despawned, or out of range.
    Items { changed: Vec<ItemView>, gone: Vec<ItemGone> },
    /// Creatures near the player that appeared or changed, and those that went: died, or
    /// out of range.
    Creatures { changed: Vec<CreatureView>, gone: Vec<CreatureGone> },
}

/// A creature as players see it. Its velocity lets clients carry it on smoothly between
/// updates; its seed picks the small differences between individuals (size, shade).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreatureView {
    pub id: u32,
    pub species: CreatureId,
    /// Feet, centred.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pose: Pose,
    pub health: u16,
    /// Its size against its species', from its genes.
    pub scale: f32,
    pub seed: u32,
}

/// A creature that is no longer there, and whether it died.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatureGone {
    pub id: u32,
    pub died: bool,
}

/// A dropped item as players see it. Its velocity lets clients carry it on smoothly
/// between updates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemView {
    pub id: u32,
    pub stack: ItemStack,
    /// Bottom centre.
    pub position: Vec3,
    pub velocity: Vec3,
}

/// A dropped item that is no longer there, and who picked it up, if anyone did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemGone {
    pub id: u32,
    pub collector: Option<PlayerId>,
}

/// How hard it rains or snows around the player: in each of the 5×5 weather cells
/// (`WEATHER_CELL` blocks across) centred on the player's, row by row along +x then +z,
/// 0..255.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nearby {
    /// The (x, z) block corner of the first cell.
    pub first: voxmar_engine::glam::IVec2,
    pub rain: [u8; 25],
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
