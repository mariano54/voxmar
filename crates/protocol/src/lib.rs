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

pub const PROTOCOL_VERSION: u32 = 25;
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
    /// Propose an action. A timed one (mining) replaces any timed action still in
    /// progress; an instant one (placing, throwing) leaves it be.
    Propose { action: ActionId, target: ActionTarget },
    /// Abandon the timed action in progress (for example, the mouse button was released).
    Cancel,
    /// The timed action at `target` is done by the player's own clock (a block shows
    /// broken): it completes, now or in its own time, and nothing the player does next
    /// cancels it. If the server has no such action (it refused it), it answers with the
    /// truth about the target, so the client never keeps a break the server did not make.
    Finished { target: ActionTarget },
    /// The player's preferences: streaming radius in columns (capped by the server) and
    /// flight speed multiplier (honoured for admins only).
    Settings { view_distance: u8, fly_speed: f32 },
    /// A command typed into the console (`time set noon`, with or without the `/`). The
    /// server checks the player may run it, runs it and replies with `Chat` lines.
    Command { line: String },
    /// Something said to everyone in the console.
    Chat { text: String },
    /// Creative mode's catalogue of every item (the inventory's item panel): `count` of
    /// `item`, onto the pointer (`held`) or into the inventory. Refused in survival.
    Take { item: BlockId, count: u8, held: bool },
    /// The player raised (`up`) or lowered their guard (`voxmar_engine::combat`).
    Guard { up: bool },
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
    /// Journal entries found or changed: all of them on joining, then those that change.
    Journal { entries: Vec<voxmar_engine::journal::Entry> },
    /// Every quest's state: all on joining, then whenever any changes.
    Quests { states: Vec<voxmar_engine::quest::QuestState> },
    /// Dropped items near the player that appeared or changed, and those that went: picked
    /// up (by `collector`), merged into another, despawned, or out of range.
    Items { changed: Vec<ItemView>, gone: Vec<ItemGone> },
    /// Creatures near the player that appeared or changed, and those that went: died, or
    /// out of range.
    Creatures { changed: Vec<CreatureView>, gone: Vec<CreatureGone> },
    /// The player's blow at a creature landed for `damage` (0: it missed), whether it
    /// killed, what kind of blow it was (on the beat, a flourish, a counter: its hitsplat)
    /// and the rhythm pips the player has after it. Every swing the server takes is
    /// answered, so the client can show it.
    Struck { creature: u32, damage: u16, killed: bool, blow: voxmar_engine::combat::Blow, pips: u8 },
    /// A creature's blow at the player came down: for `damage`, or turned aside as
    /// `defended` says. Every blow a creature winds up at the player is answered.
    Hurt { creature: u32, damage: u16, defended: voxmar_engine::combat::Defended },
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
    /// Kept by a player (tamed or bred), and that player while they are here.
    pub kept: bool,
    pub owner: Option<PlayerId>,
    /// Still growing up.
    pub young: bool,
    /// Ready to breed: fed, and looking for a mate.
    pub ready: bool,
    /// How far a wild one has come to trust the player feeding it, in hundredths.
    pub trust: u8,
    /// Its genes, packed (`engine::genes::pack`), for the client to paint what shows.
    pub genes: [u16; voxmar_engine::genes::MOST],
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
