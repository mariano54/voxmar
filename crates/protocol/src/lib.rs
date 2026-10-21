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

pub use transport::{ClientConnection, Connection, Disconnected, KEEPALIVE, ServerConnection, local_pair, tcp};
pub use voxmar_engine::weather::Weather;

// 31: the server hardening's 27/28 (numbered teleports, keepalive), after main's 29 and 30 (armour).
// 32: ClientMessage::LookedUp (the recipe viewer opened on an item, for the quest book's LookUp goals).
// 33: Ping and Pong, the round trip a fight's lag allowance reads.
// 34: ClientMessage::ClaimQuest (a finished quest's rewards are claimed in the quest screen), and
// QuestState::claimed.
// 35: Pray and Prayers, the prayer book.
// 36: a creature's tell (CreatureView::tell): the move it winds up, or rearing into a phase.
// 37: an open store's slots in the inventory (Inventory::store), after the open_store action.
// 38: where home is (Vitals::home), for the way back.
// 39: a crafting way as an action's target (ActionTarget::Recipe), for laying it.
// 40: ClientMessage::LookAtStation and ServerMessage::Station, what is on a station looked at.
// 41: ServerMessage::Fires, the lit structures whose fires burn.
// 42: a staggered creature's tell (STAGGERED).
// 43: the seat a player sits on (Vitals::seated), for the lower eye.
// 44: what the home's stores hold (Vitals::home_stores), for what can be made there.
// 47: ServerMessage::Bark, a settlement person's wordless greeting, alarm or sigh (45 and
// 46 are other lanes').
// 48: ServerMessage::Smoulders, the blocks about to catch fire, for their smoke.
// 49: a seventh worn slot, the charm (inventory::WORN_SLOTS), moving an open store's slots one
// on.
// 50: kept as a gap.
// 51: how another player looks up or down and what is in their hand (PlayerView::pitch, held).
// 54: what another player is doing in a fight (PlayerView::fight): their swing, blows, guard;
// and Players told as they change (seen, gone), names once (players who left named once more
// with no name, to be forgotten), only those near enough.
// 55 is unused.
// 56: whether a creature's blow spoiled the player's draw (ServerMessage::Hurt::spoiled).
// 57: tools wear (ItemStack::wear) and ServerMessage::ToolWore.
// 60: ServerMessage::Shown, what stands on the blocks that show what they hold (a shelf's
// things), a column at a time to the players who have it (58 and 59 are other lanes').
pub const PROTOCOL_VERSION: u32 = 60;

/// A creature's `tell` while it rears into a new phase of its fight.
pub const REARING: u8 = u8::MAX;
/// A creature's `tell` while it is staggered (a perfect block, a daze): held still, its
/// wind-up undone, so the client shows it reel rather than a blow landing.
pub const STAGGERED: u8 = u8::MAX - 1;

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
    /// `admin_token`, if the server has one and this is it, makes the player an admin.
    Hello { name: String, protocol: u32, content: u64, admin_token: Option<String> },
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
    /// The player opened the recipe viewer on `item` (to look up how it is made or what it
    /// goes into): what the quest book's "look it up" goals count.
    LookedUp { item: BlockId },
    /// The player claims the rewards (items and experience) of a finished quest from the quest
    /// screen. Refused, with a chat line and the reward still waiting, if the pack cannot
    /// hold its items; ignored if the quest is not finished or was claimed.
    ClaimQuest { name: String },
    /// The player raised (`up`) or lowered their guard (`voxmar_engine::combat`).
    Guard { up: bool },
    /// The answer to `ServerMessage::Ping` number `id`, sent as soon as it is read.
    Pong { id: u32 },
    /// The player turned a prayer on or off (`voxmar_engine::prayer`).
    Pray { prayer: voxmar_engine::prayer::PrayerId, on: bool },
    /// The player looks at the station at `block` (a drying rack at work, a kiln's core):
    /// the server answers with `ServerMessage::Station`, if it is near enough to see.
    LookAtStation { block: IVec3 },
}

/// What is on a station: `count` of `item`, making `makes` of `output`, done in `seconds`
/// (0 once done).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationLoad {
    pub item: BlockId,
    pub count: u8,
    pub output: BlockId,
    pub makes: u8,
    pub seconds: u32,
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
    /// The `seq` of the last `Teleport` the client applied: an input sent before the client
    /// saw a teleport is stale, and the server drops it rather than undo the teleport.
    pub teleport: u32,
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
    /// Authoritative position, after a rejected move, a command or a respawn; `seq` counts
    /// the player's teleports, for `PlayerInput::teleport`.
    Teleport { position: Vec3, seq: u32 },
    /// Other players near enough to see (`Limits::players_seen`), told as they change:
    /// `seen` holds only those whose view changed since it was last sent (or who came into
    /// sight), `gone` those who left or went out of sight. `names`: the names of players
    /// out of sight not told before (or renamed), so a creature they keep shows who keeps it
    /// however far off they are; an empty name for one who left the server, to forget. Sent
    /// only when there is any.
    Players { seen: Vec<PlayerView>, gone: Vec<PlayerId>, names: Vec<(PlayerId, String)> },
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
    /// Something the player tried was refused, and why: shown where the interface shows
    /// refusals (by the hotbar, as for a level the player lacks), not in the console.
    Refused { why: Words },
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
    /// `defended` says. Every blow that comes down is answered; a wind-up a daze cuts short
    /// comes to nothing and is not. `spoiled`: the blow spoiled the player's shot being
    /// drawn, so the client lets its draw go only when the server did.
    Hurt { creature: u32, damage: u16, defended: voxmar_engine::combat::Defended, spoiled: bool },
    /// Asks for a `ClientMessage::Pong` straight back: the round trip it takes is how late
    /// the player sees what happens and answers it (fights judge guards and dodges by it).
    Ping { id: u32 },
    /// The player's prayer points (whole, of `most`) and the prayers they have on, sent
    /// when either changes.
    Prayers { points: u16, most: u16, on: Vec<voxmar_engine::prayer::PrayerId> },
    /// What is on the station at `block`, answering `ClientMessage::LookAtStation`: nothing
    /// if it is empty; and, for a fuelled structure's core (a kiln), the seconds its fire
    /// burns on (0 when cold).
    Station { block: IVec3, load: Option<StationLoad>, fire: Option<u32> },
    /// The cores of every structure a player has lit whose fire still burns (a hearth, a
    /// kiln), sent on joining and whenever one is lit, breaks or burns out: they crackle
    /// and their chimneys smoke, an unlit one is cold and quiet.
    Fires { lit: Vec<IVec3> },
    /// A creature the player sees made a short, wordless sound: one of a settlement's people
    /// greeting, raising the alarm or sighing, content (`Server::bark`).
    Bark { creature: u32, bark: voxmar_engine::creature::Bark },
    /// Blocks smouldering, about to catch fire (`fire.ron`), in order: sent whenever that
    /// changes, to everyone.
    Smoulders { at: Vec<IVec3> },
    /// What the player holds wore down past its warning (`tool::Wear::warn`), or with
    /// `broke` wore out and broke: the hand hears it, and a break shows a puff of chips.
    ToolWore { item: BlockId, broke: bool },
    /// What stands on each block in `column` that shows what it holds (`BlockDef::shows`:
    /// a mantel shelf's things), by place, in order: the item in each of its first slots, a
    /// spot apiece; a block with nothing on it is left out. It replaces what was said of the
    /// column before. Sent with the column when it is sent, and whenever it changes, to the
    /// players who have the column.
    Shown { column: ColumnPos, at: Vec<(IVec3, Vec<Option<BlockId>>)> },
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
    /// What its body shows: the move it winds up (`Creature::attack_at`'s index, its
    /// `moves` from 1; 0 its plain attack or none), `REARING` into a new phase of its
    /// fight, or `STAGGERED` while it reels from a stagger, so each reads apart. Neither of
    /// the last two is a move.
    pub tell: u8,
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: PlayerId,
    /// Their name, sent once: when they come into sight not named before (in a view or in
    /// `Players::names`), or if it changes. Else `None`, the name told before standing.
    pub name: Option<String>,
    pub position: Vec3,
    pub yaw: f32,
    /// How far they look up (positive) or down, radians.
    pub pitch: f32,
    /// What is in their hand.
    pub held: Option<BlockId>,
    /// What they are doing in a fight, as the server has it.
    pub fight: FightView,
}

/// What a player is doing in a fight, for others to see it: all as the server judges it,
/// so what others are shown is what struck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FightView {
    /// The swing under way, if one is.
    pub swing: Option<SwingView>,
    /// Their swings landed so far, counted round: each change is a blow struck (or a shot
    /// loosed).
    pub struck: u8,
    /// Whether their guard is up.
    pub guard: bool,
}

/// A swing winding up (a bow drawing) or held for the beat.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SwingView {
    /// Seconds into it; below 0 while it is held for the beat.
    pub into: f32,
    /// Its wind-up, seconds: it lands as `into` reaches this.
    pub windup: f32,
    /// Whether it is a shot (a bow or sling drawn), not a stroke.
    pub shot: bool,
}
