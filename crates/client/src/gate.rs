//! `--gate DIR`: plays a scripted session through the real client and its integrated
//! server, as a player would, and checks that every step reaches the server, the client's
//! world, its meshes and the screen. It is the game half of `tools/gate`, which every
//! change runs before it lands on main (see docs/quality.md).
//!
//! Unit tests and the server's own tests each check one side; what reached Mariano's play
//! builds broke between them: blocks the client broke that the server never did, pipelines
//! that failed validation the first time a particle was drawn, a sky that vanished. So the
//! gate drives the game only through its inputs (mouse buttons, keys, console commands,
//! where the player looks) and judges it only by what it shows:
//!
//! - the server's world, as a second player connected to the same server sees it (the
//!   observer, floating out of the way), must agree block for block with the client's
//!   around the player once an edit has settled;
//! - the renderer must have every column there as its blocks are now (see
//!   `ClientWorld::check_meshed`);
//! - the screen must change where an edit happened, judged from shots taken before and
//!   after from the same place.
//!
//! Before it builds, it looks over the generated land around spawn as the player arrives
//! and checks what Mariano asked of it: no hiking-trail wear while there are no
//! settlements, no cobbled or crafted stone lying about at the surface, and grass that
//! covers some of the ground but not all of it (the tall grass share is reported; each
//! biome's is held by worldgen's `grass_is_kept_in_check`, which does not move when the
//! world reseeds and spawn lands elsewhere).
//!
//! Breaking is played under load besides: a row of blocks of every hardness broken with
//! the bare hand, a held block (throwing one halfway through) and every tool, while busy
//! threads contend for the cores, the game's own frames stall now and then and the server runs behind
//! (`SERVER_LAG`). Mariano kept losing blocks he broke on a busy machine; client and server
//! must still agree on every one.
//!
//! It runs headless like `--capture`, builds its own test ground high above spawn with
//! commands (so terrain changes don't move it), and exits with an error if any check
//! failed. Each check is logged as `gate: ok …` or `gate: FAILED …`, and the shots are
//! saved in DIR for a look.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::world::VoxelWorld;
use voxmar_server::Lag;
use voxmar_protocol::{ClientConnection, ClientMessage, PROTOCOL_VERSION, PlayerId, PlayerInput, ServerMessage, local_pair};

use crate::lang::{Locale, Localized};
use crate::menu::{Notice, Screen};
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::render::lod::Lod;
use crate::session::Launch;
use crate::settings::Settings;
use crate::world::ClientWorld;

pub struct GatePlugin {
    pub dir: PathBuf,
}

/// How the integrated server runs behind in a gate run, as on a busy machine: every tick
/// takes 60 ms more than its 50 ms (about 15 ticks a second instead of 20), every ninth
/// stalls for 400 ms besides, with players' messages waiting, and it stalls 600 ms as every
/// other action is proposed. Mariano's play builds lost blocks he broke to both: a server whose
/// word came late (fixed in 17bf62f), and one stalled when the break was proposed, whose
/// own clock then started late (fixed by the client's `Finished`).
pub const SERVER_LAG: Lag = Lag {
    per_tick: Duration::from_millis(60),
    stall: Duration::from_millis(400),
    stall_every: 9,
    proposal_stall: Duration::from_millis(600),
};

/// Size of the gate's shots: enough to judge, quick to read back.
const SHOT: UVec2 = UVec2::new(960, 540);
/// Longest the whole session may take, and the world to load, in seconds of game time.
/// The whole session has room for the gorge's longest wait on an overloaded machine, and
/// the ore's on a quiet one.
const TIME_LIMIT: f32 = 300.0 + GORGE_OVERLOADED_LIMIT + GORGE_LOAD_LIMIT;
const LOAD_LIMIT: f32 = 180.0;
/// Longest a single step may take.
const STEP_LIMIT: f32 = 30.0;
/// Longest the strike step may take: a server running behind takes blows slowly, and the
/// step fails sooner on its own if blows stop being answered.
const STRIKE_LIMIT: f32 = 90.0;
/// Seconds without the server answering a blow, and blows answered without a kill, that
/// fail the strike step: a creature with 4 health dies within a few hits at any level.
const UNANSWERED: f32 = 12.0;
const MOST_BLOWS: u32 = 40;
/// How long an edit is given at least, and at most, to reach the server, come back and be
/// meshed; the checks run as soon as both sides show it.
const SETTLE: f32 = 0.3;
const SETTLE_LIMIT: f32 = 8.0;
/// Blocks around the player in which client and server must agree.
const AGREE_WITHIN: i32 = 7;
/// How far above spawn the test ground is built, clear of terrain and water.
const GROUND_HEIGHT: i32 = 24;
/// Blocks the gate digs down, holding the button as a player would.
const DIG: usize = 3;
/// Least mean change (levels of 255) in the middle of the screen for an edit to count
/// as shown.
const SHOWN_CHANGE: f32 = 6.0;
/// Blocks around spawn over which the land is looked over, each way.
const LAND_REACH: i32 = 48;
/// Least grass-topped columns there for the ground cover to be judged.
const LAND_GRASS: usize = 300;

impl Plugin for GatePlugin {
    fn build(&self, app: &mut App) {
        let dir = self.dir.clone();
        app.insert_resource(Gate::new(dir))
            .add_systems(PostStartup, render_offscreen)
            .add_systems(Update, crate::capture::target_ui)
            .add_systems(Last, run);
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        order.insert_after(Update, BeforePostUpdate);
        order.insert_after(PostUpdate, AfterPostUpdate);
        app.init_resource::<PostUpdateTime>()
            .add_systems(BeforePostUpdate, |mut time: ResMut<PostUpdateTime>| time.start = Some(Instant::now()))
            .add_systems(AfterPostUpdate, |mut time: ResMut<PostUpdateTime>| {
                time.ms = time.start.take().map_or(0.0, |start| start.elapsed().as_secs_f32() * 1000.0);
            });
    }
}

/// How long this frame's PostUpdate took (UI layout, transforms, visibility), for the
/// overlay step.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct BeforePostUpdate;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterPostUpdate;
#[derive(Resource, Default)]
struct PostUpdateTime {
    start: Option<Instant>,
    ms: f32,
}

fn render_offscreen(
    mut commands: Commands,
    mut gate: ResMut<Gate>,
    mut images: ResMut<Assets<Image>>,
    camera: Query<Entity, With<Camera3d>>,
) {
    gate.target = crate::capture::offscreen(&mut commands, &mut images, &camera, SHOT, 1.0);
}

/// The steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Step {
    /// The world around the player loads.
    Load,
    /// The test ground is built, the clock and weather held, and the observer joins.
    Build,
    /// The player stands on the test ground, the observer floats out of the way.
    Arrive,
    /// Three planks placed on the ground behind, a click each.
    Place,
    PlaceSettle,
    /// Holding the button over the ground ahead digs a hole three deep.
    Dig,
    DigSettle,
    /// Walking into the hole picks up what the dug blocks dropped.
    Collect,
    /// A row of blocks of every hardness, broken with the bare hand, a held block and every
    /// tool, under load: client and server agree on every one, and none comes back.
    Breaks,
    /// Sticks and a crafting table from planks; the table placed and right clicked opens a
    /// three-by-three grid, where cobblestone and sticks make a stone pickaxe. An iron ore
    /// refuses the hand and a low Mining level, and breaks for the pickaxe at level 15,
    /// teaching Mining; a hoe tills dirt into farmland; cooked food takes a slot each; a rod
    /// cast into the pool catches a fish. Then the player dies: three items stay in hand,
    /// the rest lie where they fell, and after waking they are picked up again.
    Workshop,
    /// Swimming against the pool's bank and holding jump climbs out onto the block on it.
    Swim,
    /// Esc opens and closes the menu, E the inventory.
    Screens,
    /// F3 opens the debug overlay, and PostUpdate (UI layout among it) takes no longer with
    /// it open than closed.
    Overlay,
    /// A storm and a snowfall come, drawn with their particles, then clear again.
    Weather,
    /// The sun by day, the moon and stars by night.
    Sky,
    /// A small tree with an apple among its leaves, drawn with its leaf cards and fruit.
    Tree,
    /// Creatures summoned on a pad beside the test ground reach the client and draw.
    Creatures,
    /// Struck with left clicks until it dies, a lumen snail drops its slime, which walking
    /// over picks up.
    Strike,
    /// An iron sword in hand at Attack 20, a brown bear struck until it dies takes harder
    /// blows than a fist could deal.
    Weapon,
    /// Raw rabbit used on a campfire comes back cooked (or burnt) into the inventory, and
    /// smoke and embers rise from the fire.
    Cook,
    /// A junglefowl fed wheat by right clicks comes to trust the player and is theirs.
    Tame,
    /// `/locate ore` finds a sample of ore on the ground, and the server's world there has
    /// it, with its vein's ore below it.
    Ore,
    /// `/locate gorge` finds a river deep between walls, and the server's world there has
    /// them.
    Gorge,
    /// `/locate settlement` finds a hamlet or village, and the server's world there has
    /// what its plan says: a well on its square, doors open onto a floor, and houses dry.
    Settlements,
    /// Over a log in the inventory, the uses key shows what it goes into (planks, a
    /// campfire); clicking planks shows their recipe, and Back returns.
    Recipes,
    /// A timed effect's chip counts its time down, and its hover plate says how long is left.
    Effects,
    /// An effect that comes on pops up with its name, and the first look at the inventory
    /// brings its tip.
    Tips,
    /// The Farming guide sorts its lines into kinds down the left; Planting shows only
    /// planting, and a line opens its page.
    Guides,
    /// The journal key opens the journal, whose Plants page lists what there is to find.
    Journal,
    /// Ten twine crafted finish a quest; the quest key opens the book on a quest worth
    /// reading, W held with it open does not walk, Esc shuts it and W walks again; a quest pinned counts
    /// torches crafted for it as the server counts them, and a quest done reads so, then
    /// drops off.
    Quests,
    /// In creative mode the inventory's catalogue finds an item by its English name and by
    /// its Japanese reading, and a click on it gives a stack; in survival it gives nothing.
    Catalogue,
    /// The screenshot key saves the frame as a PNG (in the gate's folder, leaving the
    /// player's folder and clipboard alone) and says so in the chat.
    Screenshot,
    /// Every in-game screen and state, in English and Japanese: no text reaches past the
    /// element it sits in.
    Fit,
    /// Every journal page, in English and Japanese: no text reaches past the element it
    /// sits in. Each page has the time a step has, to be drawn and read.
    FitJournal,
    /// Leaving the world for the title screen.
    Leave,
    /// The title screen and every menu before a game, in English and Japanese: no text
    /// reaches past the element it sits in.
    FitMenus,
    Done,
}

#[derive(Resource)]
struct Gate {
    dir: PathBuf,
    /// When the run began, to tell whether files changed during it.
    started_at: Option<std::time::SystemTime>,
    target: Handle<Image>,
    step: Step,
    /// When the step started (seconds of game time), and frames since.
    since: f32,
    frames: u32,
    /// Progress through a step's own stages, and when the current one began.
    phase: u32,
    phase_since: f32,
    /// Where the player stands on the test ground.
    feet: IVec3,
    observer: Option<Observer>,
    /// Shots asked for, and those that arrived.
    requested: Vec<&'static str>,
    shots: Arc<Mutex<FxHashMap<&'static str, Image>>>,
    placed: Vec<IVec3>,
    /// The block being dug, and those dug with what they were.
    digging: Option<(IVec3, BlockId)>,
    dug: Vec<(IVec3, BlockId)>,
    /// How many of an item the inventory held before a step that changes it.
    count_before: u32,
    /// Where the feet stood when W went down with the quest book open.
    quest_feet: Vec3,
    /// The quests' revision when torches were crafted for the tracker.
    quests_rev: u32,
    /// When a quest already done was pinned, to time how long it stays.
    quest_pinned: f32,
    /// Where the player died in the workshop, the most items carried since waking, and when
    /// (in server seconds) that last went up.
    fell: Vec3,
    gathered: (u32, f32),
    /// The crucible of the kiln the workshop builds and lights, and when (server seconds)
    /// it was first lit.
    kiln: IVec3,
    kiln_since: f32,
    /// When (server seconds) the kiln was last asked to light.
    kiln_asked: f32,
    /// The machines' stage (coal in the boiler, ore in the crusher, dust out), and when
    /// (server seconds) it began and the machine was last used.
    machines_stage: u8,
    machines_since: f32,
    machines_asked: f32,
    /// Whether the player has been seen holding what this machine stage puts in.
    machines_seen: bool,
    /// When the aurochs at the treadwheel was last told to hold still, by the server's clock.
    machines_held: f32,
    /// The highest the player's feet reached while climbing out of the pool.
    highest: f32,
    /// Where `/locate gorge` sent the player: the column, one block above its water.
    gorge: IVec3,
    /// Whether the player has arrived there: the river's current may carry them off while
    /// the world around loads, which says nothing about the world.
    gorge_reached: bool,
    /// Where `/locate ore` found a sample of ore.
    ore: IVec3,
    /// Where `/locate settlement` sent the player: the middle of its square.
    settlement: IVec3,
    /// Where the struck creature was last seen, for walking over its loot.
    quarry: Vec3,
    /// Blows the server has answered, as last counted.
    blows_seen: u32,
    /// Whether the target readout has shown the struck creature's health.
    readout_seen: bool,
    /// Whether the struck creature's fall was judged.
    fall_judged: bool,
    /// Whether the loading cover was checked.
    cover_checked: bool,
    /// Texts found not to fit, each once per language: what, and where it was first seen.
    spills: Vec<(String, String)>,
    /// Views in a language with line rules where no text had its line breaks checked.
    unbroken: Vec<String>,
    /// The interface font before the gate tried another, to go back to.
    font: Option<crate::ui::skin::InterfaceFont>,
    /// Whether the taming food was moved into the hotbar, which earlier steps may have filled.
    food_moved: bool,
    /// While taming, frames the junglefowl was under the crosshair, another creature was,
    /// or none was, and the nearest the player came to it.
    tame_aim: [u32; 3],
    tame_nearest: f32,
    breaks: Breaks,
    /// Mining experience before the workshop's ore was broken.
    mining_before: Option<u32>,
    /// The lowest and highest the struck creature stood while struck: no knockback lifts it.
    struck_heights: (f32, f32),
    failures: Vec<String>,
    passes: usize,
    /// The weather's fractions as shown last frame, and the biggest change from one frame
    /// to the next relative to the most it may change in a frame (over 1 is a jump), with
    /// which and when.
    weather_last: Option<[f32; 24]>,
    weather_jump: (f32, &'static str, f32),
    /// The catalogue step: whether the player was in creative before it, and how many oak
    /// logs they had before a click.
    was_creative: bool,
    counted: u32,
    /// Real frame times so far (milliseconds), and the longest in each step.
    frame_ms: Vec<f32>,
    longest: FxHashMap<Step, f32>,
    /// The server's clock (its ticks, in seconds) this frame, and when the phase began by it.
    server_now: f32,
    phase_server: f32,
    /// The step last frame, and the failures by then: a step that fails lets go of every key
    /// and button (a step that passes may hand one on, as digging hands walking on).
    last_step: Step,
    failures_then: usize,
    /// The player's health last frame, and whether a death was reported: the session must
    /// never kill its player, and a dead one fails every step after.
    health_last: u8,
    death_seen: bool,
    /// Set while a step kills the player on purpose (the workshop's /kill), until the step ends.
    killed_on_purpose: bool,
    /// PostUpdate's milliseconds each frame with the debug overlay closed, then open.
    overlay_ms: [Vec<f32>; 2],
    /// Frames on which the open overlay's text changed (each one lays the UI out again),
    /// and when counting began and last went on.
    overlay_changes: u32,
    overlay_counted: (f32, f32),
}

impl Gate {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            started_at: Some(std::time::SystemTime::now()),
            target: Handle::default(),
            step: Step::Load,
            since: 0.0,
            frames: 0,
            phase: 0,
            phase_since: 0.0,
            feet: IVec3::ZERO,
            observer: None,
            requested: Vec::new(),
            shots: default(),
            placed: Vec::new(),
            digging: None,
            dug: Vec::new(),
            count_before: 0,
            quest_feet: Vec3::ZERO,
            quests_rev: 0,
            quest_pinned: 0.0,
            fell: Vec3::ZERO,
            gathered: (0, 0.0),
            kiln: IVec3::ZERO,
            kiln_since: 0.0,
            kiln_asked: f32::MIN,
            machines_stage: 0,
            machines_since: 0.0,
            machines_asked: f32::MIN,
            machines_seen: false,
            machines_held: f32::MIN,
            highest: f32::MIN,
            gorge: IVec3::ZERO,
            gorge_reached: false,
            settlement: IVec3::ZERO,
            ore: IVec3::ZERO,
            quarry: Vec3::ZERO,
            blows_seen: 0,
            readout_seen: false,
            fall_judged: false,
            cover_checked: false,
            spills: Vec::new(),
            unbroken: Vec::new(),
            font: None,
            food_moved: false,
            tame_aim: [0; 3],
            tame_nearest: f32::MAX,
            breaks: Breaks::default(),
            mining_before: None,
            struck_heights: (f32::MAX, f32::MIN),
            failures: Vec::new(),
            passes: 0,
            weather_last: None,
            weather_jump: (0.0, "", 0.0),
            was_creative: false,
            counted: 0,
            frame_ms: Vec::new(),
            longest: default(),
            overlay_ms: default(),
            server_now: 0.0,
            phase_server: 0.0,
            last_step: Step::Load,
            failures_then: 0,
            health_last: 0,
            death_seen: false,
            killed_on_purpose: false,
            overlay_changes: 0,
            overlay_counted: (0.0, 0.0),
        }
    }

    fn check(&mut self, name: &str, result: Result<String, String>) {
        match result {
            Ok(detail) => {
                info!("gate: ok {name}: {detail}");
                self.passes += 1;
            }
            Err(detail) => {
                error!("gate: FAILED {name}: {detail}");
                self.failures.push(format!("{name}: {detail}"));
            }
        }
    }

    /// A check whose failure is a known problem someone is fixing: reported, not failed.
    /// Remove the call (back to `check`) with the fix.
    fn known(&mut self, name: &str, result: Result<String, String>, why: &str) {
        match result {
            Ok(_) => self.check(name, result),
            Err(detail) => info!("gate: KNOWN {name}: {detail} ({why})"),
        }
    }

    fn next(&mut self, step: Step, now: f32) {
        info!("gate: {step:?} at {now:.1}s");
        self.step = step;
        self.since = now;
        self.frames = 0;
        self.phase = 0;
        self.phase_since = now;
        self.phase_server = self.server_now;
        self.food_moved = false;
    }

    /// On to the next stage of a step.
    fn advance(&mut self, now: f32) {
        self.phase += 1;
        self.phase_since = now;
        self.phase_server = self.server_now;
    }

    /// Seconds of the server's own clock since the phase began: what a wait on the server
    /// (an effect it gives, a blow it answers) is measured in, for a server running behind
    /// on a busy machine takes longer by the wall clock and the game's.
    fn server_in_phase(&self) -> f32 {
        self.server_now - self.phase_server
    }

    /// The shot called `name` of what is drawn now, asking for it the first time; it
    /// arrives a few frames later.
    fn shot(&mut self, world: &mut World, name: &'static str) -> Option<Image> {
        if !self.requested.contains(&name) {
            self.requested.push(name);
            let shots = self.shots.clone();
            world
                .spawn(Screenshot::image(self.target.clone()))
                .observe(save_to_disk(self.dir.join(format!("{name}.png"))))
                .observe(move |captured: On<ScreenshotCaptured>| {
                    shots.lock().expect("gate shots").insert(name, captured.image.clone());
                });
        }
        self.shots.lock().expect("gate shots").get(name).cloned()
    }

    fn earlier_shot(&self, name: &str) -> Option<Image> {
        self.shots.lock().expect("gate shots").get(name).cloned()
    }
}

/// A second player on the same server, out of the way above the test ground, whose copy
/// of the world is what the server says it is.
struct Observer {
    connection: ClientConnection,
    world: VoxelWorld,
    id: Option<PlayerId>,
    position: Option<Vec3>,
    moved: bool,
    /// Where the server says the other players are.
    others: FxHashMap<PlayerId, Vec3>,
    /// What the server answered the observer's commands.
    replies: Vec<voxmar_engine::lang::Words>,
}

impl Observer {
    fn join(net: &Net, content: u64) -> Option<Self> {
        let (connection, server_side) = local_pair();
        net.server()?.connections().send(server_side).ok()?;
        connection.send(ClientMessage::Hello { name: "gate-observer".into(), protocol: PROTOCOL_VERSION, content });
        connection.send(ClientMessage::Settings { view_distance: 2, fly_speed: 1.0 });
        Some(Self { connection, world: VoxelWorld::default(), id: None, position: None, moved: false, others: default(), replies: Vec::new() })
    }

    /// Applies what the server sent, and keeps the observer floating where it was put.
    fn pump(&mut self) -> Result<(), String> {
        loop {
            match self.connection.try_recv() {
                Ok(Some(message)) => match message {
                    ServerMessage::Welcome { player, position, .. } => {
                        self.id = Some(player);
                        self.position = Some(position);
                    }
                    ServerMessage::Teleport { position } => self.position = Some(position),
                    ServerMessage::Column { pos, column } => self.world.insert(pos, column),
                    ServerMessage::Unload { pos } => {
                        self.world.remove(pos);
                    }
                    ServerMessage::BlockChanged { pos, block } => {
                        self.world.set_block(pos, block);
                    }
                    ServerMessage::Blocks { changes } => {
                        for (pos, block) in changes {
                            self.world.set_block(pos, block);
                        }
                    }
                    ServerMessage::Chat { text, .. } => self.replies.push(text),
                    ServerMessage::Players { players } => {
                        self.others = players.iter().filter(|p| Some(p.id) != self.id).map(|p| (p.id, p.position)).collect();
                    }
                    _ => {}
                },
                Ok(None) => break,
                Err(_) => return Err("the server dropped the observer".into()),
            }
        }
        if let Some(position) = self.position {
            let input = PlayerInput { position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: true };
            self.connection.send(ClientMessage::Input(input));
        }
        Ok(())
    }
}

fn run(world: &mut World) {
    let now = world.resource::<Time>().elapsed_secs();
    let screen = *world.resource::<State<Screen>>().get();
    let mut gate = world.remove_resource::<Gate>().expect("gate state");
    gate.frames += 1;
    gate.breaks.hitch(gate.frames);
    stall_under_test(gate.frames);
    // (No server once the world is left: the clock stands still then.)
    if let Some(net) = world.get_resource::<Net>() {
        gate.server_now = net.server().map_or(now, |server| server.ticks() as f32 / voxmar_protocol::TICKS_PER_SECOND as f32);
    }
    // A step that failed lets go of everything it held: one once left the player walking
    // off the test ground, failing every step after it.
    if gate.step != gate.last_step && gate.failures.len() > gate.failures_then {
        for code in [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD, KeyCode::Space, KeyCode::ShiftLeft, KeyCode::ControlLeft] {
            key(world, code, false);
        }
        for button in [MouseButton::Left, MouseButton::Right] {
            mouse(world, button, false);
        }
    }
    if gate.step != gate.last_step {
        (gate.last_step, gate.failures_then) = (gate.step, gate.failures.len());
        gate.killed_on_purpose = false;
    }
    // (The text-fit steps put the player through the death screen on purpose.)
    if screen == Screen::InGame && !matches!(gate.step, Step::Fit | Step::FitJournal | Step::FitMenus | Step::Leave | Step::Done) {
        watch_health(world, &mut gate);
    }
    let frame_ms = world.resource::<Time<Real>>().delta_secs() * 1000.0;
    gate.frame_ms.push(frame_ms);
    let longest = gate.longest.entry(gate.step).or_default();
    *longest = longest.max(frame_ms);
    if screen == Screen::InGame && world.resource::<crate::weather::WeatherReady>().0 {
        watch_weather(world, &mut gate, now);
    } else {
        gate.weather_last = None;
    }
    if gate.step != Step::Done {
        match guard(world, &mut gate, now, screen) {
            Ok(()) => step(world, &mut gate, now),
            Err(problem) => {
                gate.check("session", Err(problem));
                gate.next(Step::Done, now);
            }
        }
    }
    if gate.step == Step::Done && gate.frames <= 1 {
        finish(world, &gate, now);
    }
    world.insert_resource(gate);
}

/// With `VOXMAR_GATE_STALL` set, every frame of the session stalls as a machine loaded far
/// past its cores makes it: 80 to 320 ms every fifth frame or so. Steps that pass only on an
/// idle machine (a fixed number of frames, a wait in seconds of game time that a stall
/// stretches) fail this way every time rather than now and then.
fn stall_under_test(frame: u32) {
    static STALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *STALL.get_or_init(|| std::env::var_os("VOXMAR_GATE_STALL").is_some()) {
        let roll = crate::sfx::mix(frame as u64 ^ 0x5eed);
        if roll % 5 == 0 {
            std::thread::sleep(Duration::from_millis(80 + (roll >> 8) % 240));
        }
    }
}

/// Logs every loss of health with what was around the player, and fails the session the
/// moment the player dies, then respawns them on the test ground: three sessions on a
/// loaded machine lost their player during the swim and failed every step after with no
/// word of why.
fn watch_health(world: &mut World, gate: &mut Gate) {
    let player = world.resource::<LocalPlayer>();
    let (health, dead, position, in_water) = (player.health, player.dead, player.position, player.in_water);
    if health < gate.health_last {
        let client = world.resource::<ClientWorld>();
        let name = |at: Vec3| client.block(at.floor().as_ivec3()).map_or("unloaded".to_owned(), |b| client.registry.def(b).name.clone());
        let nearest = world
            .resource::<crate::creatures::Creatures>()
            .views()
            .map(|v| (v.species, v.position.distance(position)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or("none".to_owned(), |(species, d)| format!("species {species:?} {d:.1} blocks off"));
        info!(
            "gate: health {} -> {health} during {:?} at {position:.1}: feet in {}, head in {}, in water {in_water}, nearest creature {nearest}",
            gate.health_last,
            gate.step,
            name(position + Vec3::Y * 0.1),
            name(position + Vec3::Y * 1.6)
        );
    }
    gate.health_last = health;
    if dead && !gate.death_seen && !gate.killed_on_purpose {
        gate.death_seen = true;
        gate.check("alive", Err(format!("the player died during {:?} at {position:.1} (the health losses before are logged)", gate.step)));
        key(world, KeyCode::KeyR, true);
    } else if gate.death_seen && !dead && gate.health_last > 0 {
        key(world, KeyCode::KeyR, false);
    }
}

/// What ends the session early: leaving the game, the observer cut off, or time running
/// out.
fn guard(world: &mut World, gate: &mut Gate, now: f32, screen: Screen) -> Result<(), String> {
    if !matches!(gate.step, Step::Load | Step::Leave | Step::FitMenus) && screen != Screen::InGame {
        let notice = world.resource::<Notice>().0.clone().unwrap_or_default();
        return Err(format!("left the game for {screen:?}: {notice}"));
    }
    if now > TIME_LIMIT {
        return Err(format!("still at {:?} after {TIME_LIMIT}s", gate.step));
    }
    let limit = match gate.step {
        Step::Load => LOAD_LIMIT,
        // A fresh area of the world to generate and mesh.
        Step::Gorge | Step::Ore => GORGE_OVERLOADED_LIMIT + STEP_LIMIT,
        // Striking judges itself by the blows the server answers (see `strike`).
        Step::Strike | Step::Weapon => STRIKE_LIMIT,
        Step::Breaks => BREAKS_LIMIT + 15.0,
        // Crafting, mining, tilling and fishing until something bites.
        Step::Workshop => 250.0,
        // Torches crafted for a pinned quest, waited on in the server's seconds, and a quest
        // done left to drop off.
        Step::Quests => 60.0,
        _ => STEP_LIMIT,
    };
    // The text-fit tours are judged view by view: sixty views at a settle each take most of
    // a step's time on their own, and a lagging machine draws each later, not wrongly.
    let start = if matches!(gate.step, Step::Fit | Step::FitJournal | Step::FitMenus) { gate.phase_since } else { gate.since };
    if now - start > limit {
        return Err(format!("{:?} took longer than {limit}s", gate.step));
    }
    if let Some(observer) = &mut gate.observer
        && !matches!(gate.step, Step::Leave | Step::FitMenus)
    {
        observer.pump()?;
    }
    Ok(())
}

fn step(world: &mut World, gate: &mut Gate, now: f32) {
    let since = now - gate.since;
    let in_phase = now - gate.phase_since;
    let feet = gate.feet;
    match gate.step {
        Step::Load => {
            // As a loading screen would, waiting for the weather to show too.
            if loaded(world) && world.resource::<crate::weather::WeatherReady>().0 {
                gate.check("load", Ok(format!("the world loaded in {now:.1}s")));
                land(world, gate);
                // From here on, anything first drawn compiles its pipeline in that frame
                // unless it was warmed at load (render::warm); `pipelines` lists any.
                world.resource::<crate::render::warm::PipelineWatch>().start();
                gate.next(Step::Build, now);
            }
        }
        Step::Build => {
            let feet = world.resource::<LocalPlayer>().position.floor().as_ivec3() + IVec3::Y * GROUND_HEIGHT;
            gate.feet = feet;
            let fingerprint = world.resource::<Launch>().content.fingerprint();
            let net = world.resource::<Net>();
            gate.observer = Observer::join(net, fingerprint);
            if gate.observer.is_none() {
                gate.check("observer", Err("could not join the integrated server".into()));
            }
            // The ground, with a pool three deep east of where the player stands, a step
            // up on its far bank.
            let (x, y, z) = (feet.x, feet.y, feet.z);
            for line in [
                "time set noon".to_owned(),
                "time freeze".into(),
                "weather clear 600".into(),
                format!("fill {} {} {} {} {} {} air", x - 7, y, z - 7, x + 7, y + 8, z + 7),
                format!("fill {} {} {} {} {} {} dirt", x - 7, y - 4, z - 7, x + 7, y - 1, z + 7),
                format!("fill {} {} {} {} {} {} water", x + 3, y - 3, z - 1, x + 5, y - 1, z + 1),
                format!("fill {} {} {} {} {} {} dirt", x + 6, y, z - 1, x + 6, y, z + 1),
                format!("tp {} {} {}", x as f32 + 0.5, y, z as f32 + 0.5),
            ] {
                net.command(&line);
            }
            gate.next(Step::Arrive, now);
        }
        Step::Arrive => {
            let player = world.resource::<LocalPlayer>();
            let standing = player.on_ground && player.position.floor().as_ivec3() == feet;
            let built = world.resource::<ClientWorld>().block(feet - IVec3::Y).is_some_and(|b| !b.is_air());
            let Some(observer) = gate.observer.as_mut().filter(|_| standing && built) else { return };
            let Some(above) = observer.position else { return };
            if !observer.moved {
                // High above, where it neither picks items up nor stands in a placed
                // block's way, and still has the same columns streamed to it.
                let line = format!("tp {} {} {}", above.x, feet.y + 40, above.z);
                observer.connection.send(ClientMessage::Command { line });
                observer.moved = true;
                // Click into the game, as into its window, which grabs the mouse: looking
                // at the sky, so the click mines nothing.
                look(world, 0.0, 80.0);
                mouse(world, MouseButton::Left, true);
            } else {
                // Let go the next frame, however late the observer was ready: a button left
                // held would mine whatever the next steps aim at.
                mouse(world, MouseButton::Left, false);
            }
            if since > 0.5 && settled(world, feet) {
                let locked = world.resource::<UiState>().cursor_locked;
                gate.check("arrive", if locked { Ok(format!("standing on the test ground at {feet}")) } else { Err("a click did not grab the mouse".into()) });
                aim(world, feet + IVec3::new(0, -1, 2));
                gate.count_before = held_count(world);
                gate.next(Step::Place, now);
            }
        }
        Step::Place => {
            if gate.frames < 3 || gate.shot(world, "place-before").is_none() {
                return;
            }
            // A click a cell: press, release the next frame, and on to the next cell.
            let cells = [0, 1, -1].map(|dx| feet + IVec3::new(dx, -1, 2));
            let Some(&cell) = cells.get(gate.placed.len()) else {
                aim(world, cells[0]);
                gate.next(Step::PlaceSettle, now);
                return;
            };
            if gate.frames % 2 == 0 {
                aim(world, cell);
                mouse(world, MouseButton::Right, true);
            } else {
                mouse(world, MouseButton::Right, false);
                if world.resource::<ClientWorld>().block(cell + IVec3::Y).is_some_and(|b| !b.is_air()) {
                    gate.placed.push(cell + IVec3::Y);
                }
            }
            if since > 5.0 {
                gate.check("place", Err(format!("only {} of 3 clicks placed a block", gate.placed.len())));
                mouse(world, MouseButton::Right, false);
                aim(world, feet + IVec3::new(0, -1, -2));
                gate.next(Step::Dig, now);
            }
        }
        Step::PlaceSettle => {
            if !edit_settled(world, gate, since, &gate.placed.clone(), |b| !b.is_air()) {
                return;
            }
            let Some(after) = gate.shot(world, "place-after") else { return };
            let cells = gate.placed.clone();
            verify_edit(world, gate, "place", &cells, |b| !b.is_air(), "place-before", &after);
            smooth(gate, "place", Step::Place, None);
            let (count, wanted) = (held_count(world), gate.count_before.saturating_sub(3));
            let result = if count == wanted { Ok(format!("the hand holds {count}")) } else { Err(format!("the hand holds {count}, not {wanted}")) };
            gate.check("place: inventory", result);
            aim(world, feet + IVec3::new(0, -1, -2));
            gate.next(Step::Dig, now);
        }
        Step::Dig => {
            if gate.frames < 3 || gate.shot(world, "dig-before").is_none() {
                return;
            }
            // Hold the button over the bottom of the hole as it deepens.
            let bottom = feet + IVec3::new(0, -1 - gate.dug.len() as i32, -2);
            match (world.resource::<ClientWorld>().block(bottom), gate.digging) {
                (Some(block), _) if !block.is_air() => gate.digging = Some((bottom, block)),
                (Some(_), Some((pos, was))) if pos == bottom => {
                    gate.dug.push((pos, was));
                    gate.digging = None;
                }
                _ => {}
            }
            if gate.dug.len() < DIG {
                aim(world, feet + IVec3::new(0, -1 - gate.dug.len() as i32, -2));
                mouse(world, MouseButton::Left, true);
            } else {
                mouse(world, MouseButton::Left, false);
                aim(world, feet + IVec3::new(0, -1, -2));
                gate.next(Step::DigSettle, now);
            }
        }
        Step::DigSettle => {
            let dug: Vec<IVec3> = gate.dug.iter().map(|&(pos, _)| pos).collect();
            if !edit_settled(world, gate, since, &dug, |b| b.is_air()) {
                return;
            }
            let Some(after) = gate.shot(world, "dig-after") else { return };
            let cells: Vec<IVec3> = gate.dug.iter().map(|&(pos, _)| pos).collect();
            verify_edit(world, gate, "dig", &cells, |b| b.is_air(), "dig-before", &after);
            smooth(gate, "dig", Step::Dig, None);
            gate.count_before = dropped_count(world, gate);
            look(world, 0.0, 0.0);
            key(world, KeyCode::KeyW, true);
            gate.next(Step::Collect, now);
        }
        Step::Collect => {
            if since > 1.0 {
                key(world, KeyCode::KeyW, false);
            }
            let wanted = gate.count_before + gate.dug.len() as u32;
            let count = dropped_count(world, gate);
            // Breaking pops items out with a random kick, and one from the top block can
            // land on the ground beside the hole, out of reach from its bottom, as in
            // Minecraft. Then the player climbs back out beside the hole to gather it.
            if count < wanted && since > 4.0 && gate.phase == 0 {
                let line = format!("tp {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5);
                world.resource::<Net>().command(&line);
                gate.advance(now);
            }
            if count >= wanted || since > 10.0 {
                let picked = count - gate.count_before;
                let result = if count >= wanted {
                    Ok(format!("walking into the hole picked up {picked} items"))
                } else {
                    // Where everything was, to tell a lagging pickup from an item that
                    // landed out of reach.
                    let player = world.resource::<LocalPlayer>();
                    let server = gate.observer.as_ref().and_then(|o| o.others.values().next().copied());
                    let lying: Vec<String> =
                        world.resource::<crate::drops::Drops>().lying().map(|(id, stack, at)| format!("#{id} {}x at {at:.1}", stack.count)).collect();
                    let dug: Vec<String> = gate.dug.iter().map(|(p, _)| p.to_string()).collect();
                    Err(format!(
                        "{picked} of {} dropped items picked up; the player at {:.1} (server {}), on the ground {}; items lying: [{}]; dug {}",
                        gate.dug.len(),
                        player.position,
                        server.map_or("unknown".into(), |p| format!("{p:.1}")),
                        player.on_ground,
                        lying.join(", "),
                        dug.join(" "),
                    ))
                };
                gate.check("drops", result);
                key(world, KeyCode::KeyW, false);
                gate.next(Step::Breaks, now);
            }
        }
        Step::Breaks => breaks(world, gate, now, in_phase),
        Step::Workshop => workshop(world, gate, now, in_phase),
        Step::Swim => swim(world, gate, now, in_phase),
        Step::Screens => screens(world, gate, now, in_phase),
        Step::Overlay => overlay(world, gate, now, in_phase),
        Step::Weather => weather(world, gate, now, in_phase),
        Step::Tree => tree(world, gate, now, in_phase),
        Step::Sky => sky(world, gate, now, in_phase),
        Step::Creatures => creatures(world, gate, now, in_phase),
        Step::Strike => strike(world, gate, now, in_phase),
        Step::Weapon => weapon(world, gate, now, in_phase),
        Step::Cook => cook(world, gate, now, in_phase),
        Step::Tame => tame(world, gate, now, in_phase),
        Step::Gorge => gorge(world, gate, now, in_phase),
        Step::Settlements => settlements(world, gate, now, in_phase),
        Step::Ore => ore(world, gate, now, in_phase),
        Step::Recipes => recipes(world, gate, now, in_phase),
        Step::Effects => effects(world, gate, now, in_phase),
        Step::Tips => tips(world, gate, now, in_phase),
        Step::Guides => guides(world, gate, now, in_phase),
        Step::Journal => journal(world, gate, now, in_phase),
        Step::Quests => quests(world, gate, now, in_phase),
        Step::Catalogue => catalogue(world, gate, now, in_phase),
        Step::Screenshot => screenshot(world, gate, now, in_phase),
        Step::Fit | Step::FitJournal => fit(world, gate, now, in_phase),
        Step::FitMenus => fit(world, gate, now, in_phase),
        Step::Leave => {
            if gate.phase == 0 {
                let late: Vec<String> = world.resource::<crate::render::warm::PipelineWatch>().stop().into_iter().filter(|l| !GATE_PIPELINES.contains(&l.as_str())).collect();
                let result = if late.is_empty() {
                    Ok("none compiled while playing".to_owned())
                } else {
                    Err(format!("compiled while playing, each a hitch the frame it was first needed: {}", late.join(", ")))
                };
                gate.check("pipelines", result);
                world.resource_mut::<NextState<Screen>>().set(Screen::Title);
                gate.advance(now);
            } else {
                let screen = *world.resource::<State<Screen>>().get();
                if screen != Screen::Title && in_phase < 3.0 {
                    return;
                }
                let result = if screen == Screen::Title {
                    Ok("back at the title screen, the world closed".to_owned())
                } else {
                    Err(format!("still at {screen:?} a second after leaving"))
                };
                gate.check("leave", result);
                // The session changed settings and closed the menu, which saves them in a
                // player's game; a gate run must leave the player's file as it found it.
                let modified = crate::settings::file().and_then(|file| std::fs::metadata(file).and_then(|m| m.modified()).ok());
                let untouched = if !crate::settings::file_untouched() {
                    Err("settings would be saved to the player's file".to_owned())
                } else if modified.is_some_and(|at| gate.started_at.is_some_and(|start| at > start)) {
                    Err("the settings file changed during the run".to_owned())
                } else {
                    Ok("the player's settings file was left alone".to_owned())
                };
                gate.check("settings: untouched", untouched);
                gate.next(Step::FitMenus, now);
            }
        }
        Step::Done => {}
    }
}

/// The player's effects that the weather brings on, by name in English.
fn weather_effects(world: &World) -> Vec<String> {
    let (player, launch) = (world.resource::<LocalPlayer>(), world.resource::<Launch>());
    let weathery = |c: &voxmar_engine::effect::Condition| {
        use voxmar_engine::effect::Condition::*;
        matches!(c, Rain(_) | Snow(_) | Hail(_) | Wind(_) | Dust(_) | Storm(_))
    };
    player
        .vitals
        .effects
        .iter()
        .map(|a| launch.content.effects.get(a.effect))
        .filter(|def| def.when.as_ref().is_some_and(weathery))
        .map(|def| def.title.to_lowercase())
        .collect()
}

/// Blocks of every hardness the breaks step lays out: glass and snow break in a blink,
/// stone and cobblestone take seconds.
const BREAK_BLOCKS: [&str; 6] = ["base:glass", "base:dirt", "base:stone", "base:snow_block", "base:gravel", "base:cobblestone"];
/// Where the player's feet go for the breaks step, from the test ground's middle, and the
/// cells within reach west of there where the blocks go: three columns of seven, and three
/// more north and south.
const BREAK_STAND: IVec3 = IVec3::new(-1, 0, 0);
fn break_cells() -> Vec<IVec3> {
    let west = (-4..=-2).flat_map(|x| (-3..=3).map(move |z| IVec3::new(x, 0, z)));
    west.chain([IVec3::new(-1, 0, -3), IVec3::new(0, 0, -3), IVec3::new(-1, 0, 3)]).collect()
}
/// The hotbar slot tools are swapped into.
const TOOL_SLOT: usize = voxmar_engine::inventory::HOTBAR_SLOTS - 1;
/// Longest the row may take to break, in seconds of game time.
const BREAKS_LIMIT: f32 = 45.0;

/// What a block is broken with in the breaks step.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Holder {
    Hand,
    /// The starting planks, one thrown with Q halfway through: an instant action while
    /// the break goes on, which must not cancel it.
    Planks,
    Tool(BlockId),
}

/// The breaks step's state, and the load it puts the machine under.
#[derive(Default)]
struct Breaks {
    /// The row: each cell, what it was, and what it is broken with.
    row: Vec<(IVec3, BlockId, Holder)>,
    /// Blocks done so far; when the current one began; whether its Q was pressed.
    done: usize,
    began: f32,
    thrown: bool,
    /// The tools given so far, each for its block.
    given: Vec<BlockId>,
    /// Cells the client has shown broken, and any it has seen come back since.
    shown: Vec<IVec3>,
    came_back: Vec<IVec3>,
    load: Option<Load>,
}

impl Breaks {
    /// Stalls the game's frame now and then while under load: 80 to 320 ms every fifth
    /// frame or so, as Mariano's slow frames did.
    fn hitch(&self, frame: u32) {
        if self.load.is_some() {
            let roll = crate::sfx::mix(frame as u64);
            if roll % 5 == 0 {
                std::thread::sleep(Duration::from_millis(80 + (roll >> 8) % 240));
            }
        }
    }
}

/// Threads spinning until dropped: a few, enough to contend with the game's own threads
/// on a machine that is already busy (gate runs share it with builds and other gates, and
/// with Mariano's game) without taking every core from everyone else.
const LOAD_THREADS: usize = 4;

/// Threads spinning until dropped.
struct Load {
    stop: Arc<std::sync::atomic::AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Load {
    fn start() -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let threads = (0..LOAD_THREADS)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        for _ in 0..10_000 {
                            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                        }
                    }
                })
            })
            .collect();
        Self { stop, threads }
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// A row of blocks of every hardness, broken one after another with the button held, as a
/// player sweeps along a wall: with the bare hand, the starting planks (throwing one with Q
/// halfway through) and every tool, swapped into the hotbar as needed. Busy threads
/// contend for the cores, the game's frames stall now and then, and the server runs behind. Every
/// block must end broken on the client and the server alike, and none may come back.
fn breaks(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let feet = gate.feet;
    match gate.phase {
        0 => {
            let row = {
                let registry = &world.resource::<ClientWorld>().registry;
                let tools: Vec<BlockId> = registry.ids().filter(|&id| registry.def(id).has_tag("tool")).collect();
                let mut holders = vec![Holder::Hand, Holder::Planks];
                holders.extend(tools.iter().map(|&t| Holder::Tool(t)));
                let blocks: Vec<BlockId> = BREAK_BLOCKS.iter().filter_map(|name| registry.id(name)).collect();
                // Every holder once (as many as there are cells), every hardness at least once.
                let cells = break_cells();
                let count = blocks.len().max(holders.len()).min(cells.len());
                (0..count).map(|i| (feet + cells[i], blocks[i % blocks.len()], holders[i % holders.len()])).collect::<Vec<_>>()
            };
            let net = world.resource::<Net>();
            let stand = feet + BREAK_STAND;
            net.command(&format!("tp {} {} {}", stand.x as f32 + 0.5, stand.y, stand.z as f32 + 0.5));
            let registry = &world.resource::<ClientWorld>().registry;
            for &(cell, block, _) in &row {
                net.command(&format!("setblock {} {} {} {}", cell.x, cell.y, cell.z, registry.def(block).name));
            }
            gate.breaks = Breaks { row, ..default() };
            gate.advance(now);
        }
        1 => {
            // The row in place on both sides, the player there.
            let row = gate.breaks.row.clone();
            let client = world.resource::<ClientWorld>();
            let built = |side: &dyn Fn(IVec3) -> Option<BlockId>| row.iter().all(|&(cell, block, _)| side(cell) == Some(block));
            let server = gate.observer.as_ref().is_none_or(|o| built(&|p| o.world.block(p)));
            let player = world.resource::<LocalPlayer>();
            let standing = player.on_ground && player.position.floor().as_ivec3() == feet + BREAK_STAND;
            if built(&|p| client.block(p)) && server && standing {
                gate.breaks.load = Some(Load::start());
                gate.breaks.began = now;
                gate.advance(now);
            } else if in_phase > 10.0 {
                gate.check("breaks", Err(format!("the row was never built (client {}, server {server}) or the player never stood there", built(&|p| client.block(p)))));
                end_breaks(world, gate, now);
            }
        }
        2 => {
            let breaks = &mut gate.breaks;
            let client = world.resource::<ClientWorld>();
            for &(cell, _, _) in &breaks.row {
                let air = client.block(cell).is_some_and(|b| b.is_air());
                if air && !breaks.shown.contains(&cell) {
                    breaks.shown.push(cell);
                } else if !air && breaks.shown.contains(&cell) && !breaks.came_back.contains(&cell) {
                    breaks.came_back.push(cell);
                }
            }
            let mut finished = Vec::new();
            while breaks.done < breaks.row.len() && client.block(breaks.row[breaks.done].0).is_some_and(|b| b.is_air()) {
                finished.push(breaks.row[breaks.done].2);
                breaks.done += 1;
                breaks.began = now;
                breaks.thrown = false;
            }
            // Each tool is given for its block and put away after it, so the hotbar keeps
            // room for a bare hand.
            for holder in finished {
                if let Holder::Tool(tool) = holder {
                    world.resource::<Net>().command(&format!("clear {}", client.registry.def(tool).name));
                }
            }
            let breaks = &mut gate.breaks;
            let client = world.resource::<ClientWorld>();
            let Some(&(cell, _, holder)) = breaks.row.get(breaks.done) else {
                mouse(world, MouseButton::Left, false);
                gate.advance(now);
                return;
            };
            if in_phase > BREAKS_LIMIT {
                mouse(world, MouseButton::Left, false);
                let stuck = format!("{} of {} blocks broke in {BREAKS_LIMIT}s; stuck on {cell} with {holder:?}", breaks.done, breaks.row.len());
                gate.check("breaks", Err(stuck));
                end_breaks(world, gate, now);
                return;
            }
            // This block's tool, and the next one's while this one is mined, so the next
            // is in hand without waiting on the server.
            let next = gate.breaks.row.get(gate.breaks.done + 1).map(|&(.., holder)| holder);
            for (i, wanted) in [Some(holder), next].into_iter().enumerate() {
                let Some(Holder::Tool(tool)) = wanted else { continue };
                let had = world.resource::<LocalPlayer>().inventory.slots.iter().flatten().any(|s| s.item == tool);
                if !had && !gate.breaks.given.contains(&tool) {
                    gate.breaks.given.push(tool);
                    world.resource::<Net>().command(&format!("give {} 1", client.registry.def(tool).name));
                }
                if !had && i == 0 {
                    return;
                }
            }
            hold(world, holder);
            // Q halfway through the planks' block: thrown while the break goes on.
            let progress = world.resource::<crate::interact::Target>().breaking.filter(|(at, _)| *at == cell).map_or(0.0, |(_, p)| p);
            let throw = holder == Holder::Planks && !gate.breaks.thrown && progress >= 0.5;
            if throw {
                gate.breaks.thrown = true;
            }
            key(world, KeyCode::KeyQ, throw);
            aim(world, cell);
            mouse(world, MouseButton::Left, true);
        }
        _ => {
            // Both sides settle with every block broken; then nothing may have come back.
            gate.breaks.load = None;
            let cells: Vec<IVec3> = gate.breaks.row.iter().map(|&(cell, ..)| cell).collect();
            if !edit_settled(world, gate, in_phase, &cells, |b| b.is_air()) {
                return;
            }
            let client = world.resource::<ClientWorld>();
            let named = |block: Option<BlockId>| block.map_or_else(|| "not loaded".to_owned(), |b| client.registry.def(b).name.clone());
            let mut wrong: Vec<String> = Vec::new();
            for &(cell, block, holder) in &gate.breaks.row {
                let server = gate.observer.as_ref().and_then(|o| o.world.block(cell));
                if !client.block(cell).is_some_and(|b| b.is_air()) || !server.is_some_and(|b| b.is_air()) {
                    wrong.push(format!("{} with {holder:?} at {cell}: client {}, server {}", named(Some(block)), named(client.block(cell)), named(server)));
                }
            }
            wrong.extend(gate.breaks.came_back.iter().map(|cell| format!("{cell} came back after it broke")));
            let differ = gate.observer.as_ref().map_or(0, |o| disagreements(client, &o.world, feet).len());
            if differ > 0 {
                wrong.push(format!("{differ} blocks around differ between client and server"));
            }
            let tools = gate.breaks.row.iter().filter(|(.., holder)| matches!(holder, Holder::Tool(_))).count();
            let longest = gate.longest.get(&Step::Breaks).copied().unwrap_or(0.0);
            let result = if wrong.is_empty() {
                Ok(format!("{} blocks of {} hardnesses broke with the bare hand, planks and {tools} tools under load (frames up to {longest:.0} ms), and stayed broken on both sides", gate.breaks.row.len(), BREAK_BLOCKS.len()))
            } else {
                Err(wrong.join("; "))
            };
            gate.check("breaks", result);
            end_breaks(world, gate, now);
        }
    }
}

/// Puts `holder` in the player's hand: an empty hotbar slot, the planks, or the tool,
/// swapped into the tool slot first if it is elsewhere (predicted, as a click would).
fn hold(world: &mut World, holder: Holder) {
    let actions = world.resource::<crate::Actions>().0.clone();
    let transfer = actions.expect("core:transfer");
    let planks = world.resource::<ClientWorld>().registry.id("base:oak_planks");
    let mut swap = None;
    {
        let mut player = world.resource_mut::<LocalPlayer>();
        let find = |player: &LocalPlayer, wanted: &dyn Fn(Option<BlockId>) -> bool, slots: std::ops::Range<usize>| {
            slots.into_iter().find(|&s| wanted(player.inventory.get(s).map(|stack| stack.item)))
        };
        let hotbar = 0..voxmar_engine::inventory::HOTBAR_SLOTS;
        let slot = match holder {
            Holder::Hand => find(&player, &|item| item.is_none(), hotbar),
            Holder::Planks => find(&player, &|item| item.is_some() && item == planks, hotbar),
            Holder::Tool(tool) => match find(&player, &|item| item == Some(tool), hotbar) {
                Some(slot) => Some(slot),
                None => {
                    let from = find(&player, &|item| item == Some(tool), voxmar_engine::inventory::HOTBAR_SLOTS..voxmar_engine::inventory::SLOTS);
                    if let Some(from) = from
                        && player.inventory.move_items(from, TOOL_SLOT, u8::MAX)
                    {
                        swap = Some(from);
                    }
                    swap.map(|_| TOOL_SLOT)
                }
            },
        };
        if let Some(slot) = slot {
            player.selected = slot;
        }
    }
    if let Some(from) = swap {
        let target = voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: TOOL_SLOT as u8, count: u8::MAX };
        world.resource::<Net>().propose(transfer, target);
    }
}

/// Lifts the load, puts the last tool away (later steps want room in the hotbar) and goes
/// on to the workshop.
fn end_breaks(world: &mut World, gate: &mut Gate, now: f32) {
    gate.breaks.load = None;
    mouse(world, MouseButton::Left, false);
    key(world, KeyCode::KeyQ, false);
    let registry = world.resource::<ClientWorld>().registry.clone();
    let net = world.resource::<Net>();
    let held = &world.resource::<LocalPlayer>().inventory;
    for &tool in gate.breaks.given.iter().filter(|&&tool| held.slots.iter().flatten().any(|s| s.item == tool)) {
        net.command(&format!("clear {}", registry.def(tool).name));
    }
    gate.next(Step::Workshop, now);
}

/// Where the workshop puts the crafting table, the ore, and the soil it tills, from the
/// test ground's middle (west of the breaks' stand, where the row was).
const TABLE_AT: IVec3 = IVec3::new(-2, 0, 1);
const ORE_AT: IVec3 = IVec3::new(-3, 0, 0);
const TILL_AT: IVec3 = IVec3::new(-2, -1, -1);
/// Longest a workshop stage may wait for the server.
const WORKSHOP_WAIT: f32 = 8.0;

/// How many of item `name` the inventory holds, and the first slot holding one.
fn holding(world: &World, name: &str) -> (u32, Option<usize>) {
    let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
    let Some(item) = client.registry.id(name) else { return (0, None) };
    let slots = &player.inventory.slots;
    let count = slots.iter().flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum();
    (count, slots.iter().position(|s| s.is_some_and(|s| s.item == item)))
}

/// Puts one of item `name` into each crafting cell in `cells` and crafts as many as it
/// can, as the inventory's clicks would: the server moves and crafts.
fn lay_and_craft(world: &mut World, cells: &[(usize, &str)]) -> bool {
    let actions = world.resource::<crate::Actions>().0.clone();
    let mut slots = Vec::new();
    for &(cell, item) in cells {
        let Some(slot) = holding(world, item).1 else { return false };
        slots.push((slot, cell));
    }
    let net = world.resource::<Net>();
    for (slot, cell) in slots {
        let target = voxmar_engine::action::ActionTarget::Slots { from: slot as u8, to: (voxmar_engine::inventory::CRAFT + cell) as u8, count: 1 };
        net.propose(actions.expect("core:transfer"), target);
    }
    net.propose(actions.expect("core:craft"), voxmar_engine::action::ActionTarget::Slot(0));
    true
}

/// Selects the hotbar slot holding `name`, swapping it in from the pack first if need be.
fn select(world: &mut World, name: &str) -> bool {
    let Some(slot) = holding(world, name).1 else { return false };
    if slot >= voxmar_engine::inventory::HOTBAR_SLOTS {
        let actions = world.resource::<crate::Actions>().0.clone();
        let target = voxmar_engine::action::ActionTarget::Slots { from: slot as u8, to: TOOL_SLOT as u8, count: u8::MAX };
        world.resource::<Net>().propose(actions.expect("core:transfer"), target);
        world.resource_mut::<LocalPlayer>().inventory.move_items(slot, TOOL_SLOT, u8::MAX);
        world.resource_mut::<LocalPlayer>().selected = TOOL_SLOT;
    } else {
        world.resource_mut::<LocalPlayer>().selected = slot;
    }
    true
}

/// The level of a skill, as the client was last told.
fn skill_level(world: &World, name: &str) -> (u8, u32) {
    let (player, launch) = (world.resource::<LocalPlayer>(), world.resource::<Launch>());
    let skills = &launch.content.skills;
    skills.id(name).map_or((1, 0), |id| (skills.level(player.skills.xp(id)), player.skills.xp(id)))
}

/// Crafting, a crafting table, tool tiers and Mining levels, and tilling: see `Step::Workshop`.
fn workshop(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let feet = gate.feet;
    let (table, ore, till) = (feet + TABLE_AT, feet + ORE_AT, feet + TILL_AT);
    let fail = |world: &mut World, gate: &mut Gate, what: &str, why: String| {
        gate.check(&format!("workshop: {what}"), Err(why));
        end_workshop(world, gate, now);
    };
    let block_is = |world: &World, gate: &Gate, at: IVec3, name: &str| {
        let client = world.resource::<ClientWorld>();
        let want = client.registry.id(name);
        client.block(at) == want && gate.observer.as_ref().is_none_or(|o| o.world.block(at) == want)
    };
    match gate.phase {
        // Two planks, one over the other in the two-by-two grid, make four sticks.
        0 => {
            mouse(world, MouseButton::Left, false);
            gate.count_before = holding(world, "base:stick").0;
            if lay_and_craft(world, &[(0, "base:oak_planks"), (3, "base:oak_planks")]) {
                gate.advance(now);
            } else {
                fail(world, gate, "sticks", "no planks to make sticks of".into());
            }
        }
        1 => {
            let sticks = holding(world, "base:stick").0;
            if sticks >= gate.count_before + 4 {
                gate.check("workshop: sticks", Ok(format!("two planks made {} sticks", sticks - gate.count_before)));
                let (_, xp) = skill_level(world, "core:engineering");
                gate.count_before = xp;
                lay_and_craft(world, &[(0, "base:oak_planks"), (1, "base:oak_planks"), (3, "base:oak_planks"), (4, "base:oak_planks")]);
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "sticks", format!("two planks in the grid made {} sticks", sticks - gate.count_before));
            }
        }
        // Four planks make a crafting table, teaching Engineering; placed with a right click.
        2 => {
            if holding(world, "base:crafting_table").0 > 0 {
                let (_, xp) = skill_level(world, "core:engineering");
                let learnt = xp.saturating_sub(gate.count_before);
                let result = if learnt > 0 { Ok(format!("four planks made a crafting table, {:.1} Engineering experience", learnt as f32 / 10.0)) } else { Err("a crafting table was made but taught no Engineering".into()) };
                gate.check("workshop: table", result);
                select(world, "base:crafting_table");
                aim(world, table - IVec3::Y);
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "table", "four planks in the grid made no crafting table".into());
            }
        }
        3 => {
            let pressed = gate.frames % 2 == 0;
            mouse(world, MouseButton::Right, pressed && !block_is(world, gate, table, "base:crafting_table"));
            if block_is(world, gate, table, "base:crafting_table") {
                mouse(world, MouseButton::Right, false);
                aim(world, table);
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                mouse(world, MouseButton::Right, false);
                fail(world, gate, "table", "a right click did not place the crafting table on both sides".into());
            }
        }
        // Right clicking the table opens the inventory with a three-by-three grid, where
        // three cobblestone over two sticks make a stone pickaxe.
        4 => {
            let open = world.resource::<UiState>().inventory_open && world.resource::<LocalPlayer>().station == Some(table);
            if !open {
                mouse(world, MouseButton::Right, gate.frames % 2 == 0);
                if in_phase > WORKSHOP_WAIT {
                    mouse(world, MouseButton::Right, false);
                    fail(world, gate, "table: open", "right clicking the crafting table did not open its grid".into());
                }
                return;
            }
            mouse(world, MouseButton::Right, false);
            let mut cells = world.query::<(&crate::hud::inventory::CraftCell, &Node)>();
            let shown = cells.iter(world).filter(|(_, node)| node.display != Display::None).count();
            if shown < voxmar_engine::craft::GRID {
                // The grid is laid out the next frame.
                if in_phase > 2.0 {
                    fail(world, gate, "table: open", format!("the table's grid shows {shown} cells, not {}", voxmar_engine::craft::GRID));
                }
                return;
            }
            gate.check("workshop: table: open", Ok(format!("a right click opened the table's grid of {shown} cells")));
            gate.count_before = skill_level(world, "core:engineering").1;
            let laid = lay_and_craft(world, &[(0, "base:cobblestone"), (1, "base:cobblestone"), (2, "base:cobblestone"), (4, "base:stick"), (7, "base:stick")]);
            if laid {
                gate.advance(now);
            } else {
                fail(world, gate, "pickaxe", "no cobblestone or sticks for a pickaxe".into());
            }
        }
        5 => {
            if holding(world, "base:stone_pickaxe").0 > 0 {
                // A shot of the table's grid, for a look.
                if gate.shot(world, "table-grid").is_none() {
                    return;
                }
                gate.check("workshop: pickaxe", Ok("three cobblestone over two sticks made a stone pickaxe at the table".into()));
                // Leaving the screen leaves the table.
                world.resource_mut::<UiState>().inventory_open = false;
                let net = world.resource::<Net>();
                net.command(&format!("setblock {} {} {} iron_ore", ore.x, ore.y, ore.z));
                net.command("skill mining 1");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "pickaxe", "the table's grid made no stone pickaxe".into());
            }
        }
        // An iron ore asks for Mining 15 and a stone pickaxe: refused with the level shown,
        // then broken at level 15 with the pickaxe, teaching Mining.
        6 => {
            if !block_is(world, gate, ore, "base:iron_ore") || world.resource::<LocalPlayer>().station.is_some() {
                if in_phase > WORKSHOP_WAIT {
                    fail(world, gate, "ore", "the iron ore never came, or the table's grid stayed open".into());
                }
                return;
            }
            select(world, "base:stone_pickaxe");
            aim(world, ore);
            mouse(world, MouseButton::Left, gate.frames % 2 == 0);
            let locale = world.resource::<Locale>();
            let launch = world.resource::<Launch>();
            let mining = launch.content.skills.id("core:mining").map(|id| locale.skill(launch.content.skills.get(id))).unwrap_or_default();
            let words = locale.format("skills.needs", &[("skill", &mining), ("level", &15)]);
            if world.resource::<crate::hud::skills::SkillToasts>().shows_refusal(&words) {
                mouse(world, MouseButton::Left, false);
                gate.check("workshop: ore: refused", Ok(format!("at Mining 1 the iron ore refused the pickaxe: \"{words}\"")));
                world.resource::<Net>().command("skill mining 15");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                mouse(world, MouseButton::Left, false);
                fail(world, gate, "ore: refused", format!("clicking the iron ore at Mining 1 never said \"{words}\""));
            }
        }
        7 => {
            if skill_level(world, "core:mining").0 < 15 {
                return;
            }
            let xp = skill_level(world, "core:mining").1;
            let before = *gate.mining_before.get_or_insert(xp);
            aim(world, ore);
            mouse(world, MouseButton::Left, true);
            let gone = world.resource::<ClientWorld>().block(ore).is_some_and(|b| b.is_air()) && gate.observer.as_ref().is_none_or(|o| o.world.block(ore).is_some_and(|b| b.is_air()));
            let learnt = xp.saturating_sub(before);
            if gone && learnt > 0 {
                mouse(world, MouseButton::Left, false);
                gate.check("workshop: ore", Ok(format!("at Mining 15 the stone pickaxe broke the iron ore on both sides, {:.1} Mining experience", learnt as f32 / 10.0)));
                world.resource::<Net>().command("give wooden_hoe 1");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                mouse(world, MouseButton::Left, false);
                fail(world, gate, "ore", format!("at Mining 15 with the pickaxe the ore {} and taught {learnt} tenths", if gone { "broke" } else { "stayed" }));
            }
        }
        // A hoe tills dirt into farmland with a right click.
        8 => {
            if !select(world, "base:wooden_hoe") {
                if in_phase > WORKSHOP_WAIT {
                    fail(world, gate, "till", "the hoe never came".into());
                }
                return;
            }
            aim(world, till);
            mouse(world, MouseButton::Right, gate.frames % 2 == 0);
            if block_is(world, gate, till, "base:farmland") {
                mouse(world, MouseButton::Right, false);
                if gate.shot(world, "workshop").is_none() {
                    return;
                }
                gate.check("workshop: till", Ok("a right click with a hoe tilled the dirt into farmland on both sides".into()));
                world.resource::<Net>().command("give bread 3");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                mouse(world, MouseButton::Right, false);
                fail(world, gate, "till", "right clicking dirt with a hoe made no farmland".into());
            }
        }
        // Cooked food takes a slot each, as in RuneScape.
        9 => {
            let player = world.resource::<LocalPlayer>();
            let bread = world.resource::<ClientWorld>().registry.id("base:bread");
            let stacks: Vec<u8> = player.inventory.slots.iter().flatten().filter(|s| Some(s.item) == bread).map(|s| s.count).collect();
            if stacks.len() >= 3 {
                let result = if stacks.iter().all(|&n| n == 1) { Ok("three loaves took three slots".to_owned()) } else { Err(format!("bread stacked: {stacks:?}")) };
                gate.check("workshop: food takes a slot each", result);
                let net = world.resource::<Net>();
                net.command("give seed_pouch 1");
                net.command("give wheat_seeds 50");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "food takes a slot each", format!("three loaves given, the inventory shows {stacks:?}"));
            }
        }
        // With a seed pouch carried, seeds go into it rather than taking slots.
        10 => {
            let seeds = world.resource::<ClientWorld>().registry.id("base:wheat_seeds");
            let pouched = seeds.map_or(0, |s| world.resource::<LocalPlayer>().inventory.pouched(s));
            if pouched >= 50 {
                gate.check("workshop: seed pouch", Ok(format!("50 seeds given went into the pouch ({pouched} there)")));
                let net = world.resource::<Net>();
                net.command("clear seed_pouch");
                net.command("give fishing_rod 1");
                net.command("skill fishing 80");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "seed pouch", format!("50 seeds given with a seed pouch carried, the pouch holds {pouched}"));
            }
        }
        // A rod held over the pool, right button down, casts until something bites.
        11 => {
            let fish = |world: &World| {
                let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
                player.inventory.slots.iter().flatten().find(|s| client.registry.def(s.item).has_tag("fish")).map(|s| client.registry.def(s.item).name.clone())
            };
            if !select(world, "base:fishing_rod") {
                if in_phase > WORKSHOP_WAIT {
                    fail(world, gate, "fishing", "the rod never came".into());
                }
                return;
            }
            aim(world, feet + IVec3::new(4, -1, 0));
            mouse(world, MouseButton::Right, true);
            if let Some(caught) = fish(world) {
                mouse(world, MouseButton::Right, false);
                gate.check("workshop: fishing", Ok(format!("casting into the pool caught {caught} in {in_phase:.1}s")));
                // The crafting table made earlier finished the quest that asks for one.
                let quests = &world.resource::<LocalPlayer>().quests;
                let table = quests.iter().find(|q| q.name == "base:crafting_table").map(|q| q.done);
                gate.check("workshop: quests", match table {
                    Some(true) => Ok(format!("the crafting table quest is done ({} of {} quests)", quests.iter().filter(|q| q.done).count(), quests.len())),
                    Some(false) => Err("a crafting table was made, but its quest is not done".into()),
                    None => Err("the server sent no quests".into()),
                });
                // A small pack to die with, so what is left lies in a few stacks by the feet.
                let net = world.resource::<Net>();
                for line in ["clear", "give stone_pickaxe 1", "give bread 1", "give stick 10", "give dirt 20", "give oak_planks 12"] {
                    net.command(line);
                }
                gate.advance(now);
            } else if in_phase > 40.0 {
                mouse(world, MouseButton::Right, false);
                fail(world, gate, "fishing", "40 seconds of casting into the pool at Fishing 80 caught nothing".into());
            }
        }
        12 => {
            if carried(world) == 44 {
                gate.count_before = 44;
                gate.fell = world.resource::<LocalPlayer>().position;
                gate.killed_on_purpose = true;
                world.resource::<Net>().command("kill");
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "death keeps three", format!("the pack to die with never came: carrying {}", carried(world)));
            }
        }
        // Dead: the three most valuable items stay, the rest lie where the player fell.
        13 => {
            let (dead, kept) = (world.resource::<LocalPlayer>().dead, carried(world));
            if dead && kept == 3 {
                let lying = world.resource::<crate::drops::Drops>().lying().filter(|(_, _, at)| at.distance(gate.fell) < 4.0).count();
                let result = if lying > 0 { Ok(format!("dying kept 3 of {} items; {lying} stacks lie where the player fell", gate.count_before)) } else { Err("dying kept 3 items, but nothing lies where the player fell".into()) };
                gate.check("workshop: death keeps three", result);
                let actions = world.resource::<crate::Actions>().0.clone();
                world.resource::<Net>().propose(actions.expect("core:respawn"), voxmar_engine::action::ActionTarget::None);
                gate.advance(now);
            } else if in_phase > WORKSHOP_WAIT {
                fail(world, gate, "death keeps three", format!("after /kill: dead {dead}, carrying {kept} of {} items", gate.count_before));
            }
        }
        // Awake again, back where they fell, the player picks everything up.
        14 => {
            if world.resource::<LocalPlayer>().dead {
                if in_phase > WORKSHOP_WAIT {
                    fail(world, gate, "grave", "the player never woke after asking to respawn".into());
                }
                return;
            }
            // Walk to each stack still lying near where they fell, as a player gathers
            // them: one that rolled into the pool or down a step is out of reach from the
            // spot itself.
            let fell = gate.fell;
            let lying: Vec<Vec3> = world.resource::<crate::drops::Drops>().lying().map(|(_, _, at)| at).filter(|at| at.distance(fell) < 10.0).collect();
            if gate.frames % 30 == 0 {
                let to = lying.iter().copied().min_by(|a, b| a.distance(fell).total_cmp(&b.distance(fell))).unwrap_or(fell);
                let line = format!("tp {} {} {}", to.x, to.y, to.z);
                world.resource::<Net>().command(&line);
            }
            let back = carried(world);
            // Picking up is the server's work: under load it gathers slowly, so the wait
            // runs while the count keeps rising, by the server's clock.
            if back > gate.gathered.0 || gate.gathered.1 == 0.0 {
                gate.gathered = (back.max(gate.gathered.0), gate.server_now);
            }
            if back >= gate.count_before {
                gate.check("workshop: grave", Ok(format!("woke and picked all {back} items back up where the player fell")));
                // The kit a new player starts with, for the steps after.
                let net = world.resource::<Net>();
                for line in ["clear", "give oak_planks 64", "give cobblestone 64", "give glass 32", "give glowstone 16"] {
                    net.command(line);
                }
                // A Crucible Kiln built in the air over the player's head, clear of the
                // test ground and the pool, to light.
                let kiln = world.resource::<LocalPlayer>().position.floor().as_ivec3() + IVec3::new(0, 4, 0);
                let at = |dx: i32, dy: i32, dz: i32| format!("{} {} {}", kiln.x + dx, kiln.y + dy, kiln.z + dz);
                let net = world.resource::<Net>();
                for line in [
                    format!("fill {} {} air", at(-1, 0, -1), at(1, 1, 1)),
                    format!("fill {} {} kiln_bricks", at(-1, 0, -1), at(1, 0, 1)),
                    format!("setblock {} crucible", at(0, 0, 0)),
                    format!("setblock {} kiln_bricks", at(-1, 1, -1)),
                    format!("setblock {} kiln_bricks", at(1, 1, -1)),
                    format!("setblock {} kiln_bricks", at(-1, 1, 1)),
                    format!("setblock {} kiln_bricks", at(1, 1, 1)),
                ] {
                    net.command(&line);
                }
                gate.kiln = kiln;
                gate.advance(now);
            } else if gate.server_now - gate.gathered.1 > 15.0 {
                let left: Vec<String> = lying.iter().map(|at| format!("{:+.1},{:+.1},{:+.1}", at.x - fell.x, at.y - fell.y, at.z - fell.z)).collect();
                fail(world, gate, "grave", format!("nothing more was picked up for 15 server seconds, carrying {back} of {} items; stacks lying near, from where they fell: [{}]", gate.count_before, left.join(" ")));
            }
        }
        // Right clicking the crucible lights the kiln: the quest that asks for it counts it.
        15 => {
            let kiln = gate.kiln;
            let lit = |world: &World| {
                world.resource::<LocalPlayer>().quests.iter().find(|q| q.name == "base:crucible_kiln").and_then(|q| q.tasks.first()?.first().copied()).is_some_and(|(have, _)| have > 0)
            };
            let crucible = world.resource::<ClientWorld>().registry.id("base:crucible");
            if world.resource::<ClientWorld>().block(kiln) != crucible {
                if in_phase > WORKSHOP_WAIT {
                    fail(world, gate, "kiln", "the crucible never came".into());
                }
                return;
            }
            if gate.kiln_since == 0.0 {
                gate.kiln_since = gate.server_now;
            }
            // Asked again only every few server seconds: each ask may stall the lagging
            // server (`SERVER_LAG`), and asked every few frames it never got to a tick.
            if gate.server_now - gate.kiln_asked >= 3.0 {
                gate.kiln_asked = gate.server_now;
                let actions = world.resource::<crate::Actions>().0.clone();
                world.resource::<Net>().propose(actions.expect("base:light"), voxmar_engine::action::ActionTarget::Block { block: kiln, face: voxmar_engine::coords::Face::NegY });
            }
            // Quests are checked once a second of the server's time, which a loaded machine
            // stretches: the wait is by its clock.
            let done = lit(world);
            let late = gate.server_now - gate.kiln_since > 20.0 || in_phase > 40.0;
            if done || late {
                // Taken down again either way, so the pool's bank is clear for the swim.
                let (a, b) = (kiln - IVec3::new(1, 0, 1), kiln + IVec3::new(1, 1, 1));
                world.resource::<Net>().command(&format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z));
            }
            if done {
                gate.check("workshop: kiln", Ok("lighting the crucible lit the Crucible Kiln round it".into()));
                // Where the kiln stood, a boiler with a steam crusher beside it, and nothing in
                // hand but coal to fire it.
                let at = |d: IVec3| { let p = kiln + d; format!("{} {} {}", p.x, p.y, p.z) };
                let net = world.resource::<Net>();
                for line in ["clear".to_owned(), format!("setblock {} boiler", at(IVec3::ZERO)), format!("setblock {} steam_crusher", at(IVec3::X)), "give coal_ore 2".to_owned()] {
                    net.command(&line);
                }
                gate.machines_stage = 0;
                gate.machines_since = gate.server_now;
                gate.machines_asked = f32::MIN;
                gate.machines_seen = false;
                gate.advance(now);
            } else if late {
                let quests = &world.resource::<LocalPlayer>().quests;
                let state = quests.iter().find(|q| q.name == "base:crucible_kiln").cloned();
                let waited = (gate.server_now - gate.kiln_since, in_phase);
                fail(world, gate, "kiln", format!("lighting the crucible in its kiln never counted ({:.1} server s, {:.1} game s): {} quests, the kiln's {state:?}", waited.0, waited.1, quests.len()));
            }
        }
        // Coal in hand fires the boiler, ore in hand goes into the crusher beside it, and
        // with an empty hand the crusher gives back two dust for the one ore, made on the
        // boiler's steam.
        16 => {
            let (boiler, crusher) = (gate.kiln, gate.kiln + IVec3::X);
            let client = world.resource::<ClientWorld>();
            if client.block(crusher) != client.registry.id("base:steam_crusher") || client.block(boiler) != client.registry.id("base:boiler") {
                if in_phase > WORKSHOP_WAIT {
                    fail(world, gate, "machines", "the boiler and crusher never came".into());
                }
                return;
            }
            let (coal, ore, dust) = (holding(world, "coal_ore").0, holding(world, "copper_ore").0, holding(world, "copper_dust").0);
            // Each stage is done by what the player holds, as the server says it: what goes in
            // must first be seen in hand (it comes a few frames after it is given), then gone
            // after the machine was used.
            let held = match gate.machines_stage {
                0 => coal,
                1 => ore,
                _ => 0,
            };
            gate.machines_seen |= held > 0;
            let put_in = gate.machines_seen && held == 0 && gate.machines_asked > f32::MIN;
            let (done, target, holds) = match gate.machines_stage {
                0 => (put_in, boiler, Some("coal_ore")),
                1 => (put_in, crusher, Some("copper_ore")),
                _ => (dust >= 2, crusher, None),
            };
            if done {
                let net = world.resource::<Net>();
                match gate.machines_stage {
                    0 => net.command("give copper_ore 1"),
                    1 => net.command("clear"),
                    _ => {}
                }
                if gate.machines_stage == 2 {
                    gate.check("workshop: machines", Ok(format!("coal fired the boiler, its steam crushed one copper ore, and an empty hand took out {dust} dust")));
                    // In their place, MV: a boiler and turbine over a transformer, an ore
                    // washer beside it, and a dry glass basin beside that for its water.
                    let k = gate.kiln;
                    let at = |d: IVec3| { let p = k + d; format!("{} {} {}", p.x, p.y, p.z) };
                    let net = world.resource::<Net>();
                    for line in [
                        format!("fill {} {} air", at(IVec3::new(0, 0, -1)), at(IVec3::new(3, 1, 1))),
                        "clear".to_owned(),
                        format!("setblock {} lv_transformer", at(IVec3::ZERO)),
                        format!("setblock {} steam_turbine", at(IVec3::Y)),
                        format!("setblock {} boiler", at(IVec3::new(1, 1, 0))),
                        format!("setblock {} ore_washer", at(IVec3::X)),
                        format!("setblock {} glass", at(IVec3::new(2, -1, 0))),
                        format!("setblock {} glass", at(IVec3::new(3, 0, 0))),
                        format!("setblock {} glass", at(IVec3::new(2, 0, -1))),
                        format!("setblock {} glass", at(IVec3::new(2, 0, 1))),
                        "give coal_ore 4".to_owned(),
                    ] {
                        net.command(&line);
                    }
                    gate.machines_stage = 0;
                    gate.machines_since = gate.server_now;
                    gate.machines_asked = f32::MIN;
                    gate.machines_seen = false;
                    gate.advance(now);
                    return;
                }
                gate.machines_stage += 1;
                gate.machines_since = gate.server_now;
                gate.machines_asked = f32::MIN;
                gate.machines_seen = false;
                return;
            }
            // Waits for what it holds to arrive before using the machine with it.
            let ready = match holds {
                Some(name) => select(world, name),
                None => world.resource::<LocalPlayer>().inventory.slots.iter().all(Option::is_none),
            };
            // Used only every few server seconds (see the kiln above); the crusher takes a
            // few of them to work the ore.
            if ready && gate.server_now - gate.machines_asked >= 3.0 {
                gate.machines_asked = gate.server_now;
                let actions = world.resource::<crate::Actions>().0.clone();
                world.resource::<Net>().propose(actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
            }
            // No progress for 30 seconds of the server's time (a loaded machine stretches it);
            // the game's clock only catches a server that stopped.
            if gate.server_now - gate.machines_since > 30.0 || in_phase > 300.0 {
                let what = ["the boiler never took the coal", "the crusher never took the ore", "the crusher never gave back two dust"][gate.machines_stage.min(2) as usize];
                let waited = (gate.server_now - gate.machines_since, in_phase);
                fail(world, gate, "machines", format!("{what} ({:.1} server s, {:.1} game s): holding coal {coal}, ore {ore}, dust {dust}", waited.0, waited.1));
                let (a, b) = (boiler, crusher);
                world.resource::<Net>().command(&format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z));
            }
        }
        // MV: the turbine's power comes down through the transformer to the washer, which
        // takes the ore but waits, saying it needs water, until water lies beside it; then
        // an empty hand takes out three dust for the one ore.
        17 => {
            let k = gate.kiln;
            let (boiler, washer, basin) = (k + IVec3::new(1, 1, 0), k + IVec3::X, k + IVec3::new(2, 0, 0));
            let teardown = move |world: &mut World| {
                let (a, b) = (k + IVec3::new(0, -1, -1), k + IVec3::new(3, 1, 1));
                let net = world.resource::<Net>();
                for line in [format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z), "clear".to_owned()] {
                    net.command(&line);
                }
                for line in ["give oak_planks 64", "give cobblestone 64", "give glass 32", "give glowstone 16"] {
                    net.command(line);
                }
            };
            let client = world.resource::<ClientWorld>();
            if client.block(washer) != client.registry.id("base:ore_washer") || client.block(boiler) != client.registry.id("base:boiler") {
                if in_phase > WORKSHOP_WAIT {
                    teardown(world);
                    fail(world, gate, "MV machines", "the turbine, transformer and washer never came".into());
                }
                return;
            }
            let (coal, ore, dust) = (holding(world, "coal_ore").0, holding(world, "copper_ore").0, holding(world, "copper_dust").0);
            let content = world.resource::<Launch>().content.clone();
            let english = |words: &voxmar_engine::lang::Words| content.english.render(words, &content.blocks);
            let dry = gate.observer.as_ref().is_some_and(|o| o.replies.iter().map(english).any(|r| r.contains("needs water")));
            // Each stage is done by what the player holds (first seen in hand, then gone after
            // the machine was used, as in the step before), or what the server says of the
            // washer (asked by the observer: `/machine`).
            let held = match gate.machines_stage {
                0 => coal,
                1 => ore,
                _ => 0,
            };
            gate.machines_seen |= held > 0;
            let put_in = gate.machines_seen && held == 0 && gate.machines_asked > f32::MIN;
            let done = match gate.machines_stage {
                0 | 1 => put_in,
                2 => dry,
                _ => dust >= 3,
            };
            if done {
                let net = world.resource::<Net>();
                match gate.machines_stage {
                    0 => net.command("give copper_ore 1"),
                    1 => net.command("clear"),
                    2 => net.command(&format!("setblock {} {} {} water", basin.x, basin.y, basin.z)),
                    _ => {
                        gate.check("workshop: MV machines", Ok(format!("a turbine on steam powered an ore washer through a transformer; dry it said it needs water, wet it gave {dust} dust for one ore")));
                        teardown(world);
                        // In their place, the Bellows Bloomery: kiln bricks round a bloomery,
                        // bellows in their ring, a shaft out to a treadwheel, and a tamed
                        // aurochs held still on a floor beside the wheel.
                        let at = |d: IVec3| { let p = k + d; format!("{} {} {}", p.x, p.y, p.z) };
                        let net = world.resource::<Net>();
                        for line in [
                            format!("fill {} {} cobblestone", at(IVec3::new(2, -1, 1)), at(IVec3::new(4, -1, 3))),
                            format!("fill {} {} kiln_bricks", at(IVec3::new(-1, 0, -1)), at(IVec3::new(1, 2, 1))),
                            format!("fill {} {} air", at(IVec3::Y), at(IVec3::new(0, 2, 0))),
                            format!("setblock {} bloomery", at(IVec3::ZERO)),
                            format!("setblock {} bellows", at(IVec3::X)),
                            format!("setblock {} shaft", at(IVec3::new(2, 0, 0))),
                            format!("setblock {} treadwheel", at(IVec3::new(3, 0, 0))),
                            format!("summon aurochs 1 {}", at(IVec3::new(3, 0, 2))),
                            "creatures tame aurochs".to_owned(),
                            "creatures hold aurochs".to_owned(),
                            "clear".to_owned(),
                            "give iron_ore 2".to_owned(),
                        ] {
                            net.command(&line);
                        }
                        gate.machines_stage = 0;
                        gate.machines_since = gate.server_now;
                        gate.machines_asked = f32::MIN;
                        gate.machines_held = gate.server_now;
                        gate.machines_seen = false;
                        gate.advance(now);
                        return;
                    }
                }
                gate.machines_stage += 1;
                gate.machines_since = gate.server_now;
                gate.machines_asked = f32::MIN;
                gate.machines_seen = false;
                return;
            }
            // Used (or asked about) only every few server seconds, as the kiln is.
            if gate.server_now - gate.machines_asked >= 3.0 {
                let ready = match gate.machines_stage {
                    0 => select(world, "coal_ore"),
                    1 => select(world, "copper_ore"),
                    _ => world.resource::<LocalPlayer>().inventory.slots.iter().all(Option::is_none),
                };
                if ready {
                    gate.machines_asked = gate.server_now;
                    if gate.machines_stage == 2 {
                        if let Some(observer) = gate.observer.as_mut() {
                            observer.replies.clear();
                            observer.connection.send(ClientMessage::Command { line: format!("machine {} {} {}", washer.x, washer.y, washer.z) });
                        }
                    } else {
                        let target = if gate.machines_stage == 0 { boiler } else { washer };
                        let actions = world.resource::<crate::Actions>().0.clone();
                        world.resource::<Net>().propose(actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
                    }
                }
            }
            // By the server's clock, as the step before.
            if gate.server_now - gate.machines_since > 40.0 || in_phase > 300.0 {
                let what = ["the boiler never took the coal", "the washer never took the ore", "the dry washer never said it needs water", "the wet washer never gave back three dust"][gate.machines_stage.min(3) as usize];
                let waited = (gate.server_now - gate.machines_since, in_phase);
                let said: Vec<String> = gate.observer.as_ref().map(|o| o.replies.iter().map(english).collect()).unwrap_or_default();
                teardown(world);
                fail(world, gate, "MV machines", format!("{what} ({:.1} server s, {:.1} game s): holding coal {coal}, ore {ore}, dust {dust}; /machine said {said:?}", waited.0, waited.1));
            }
        }
        // The Bellows Bloomery and item logistics. Two iron ore and a coal go into the
        // bloomery; an aurochs walking the treadwheel blows its bellows, and an empty hand
        // takes out the bloom. Then a line in its place: coal fires a boiler, ore put into a
        // hopper rides a conveyor into the steam crusher, and its dust rises up a chute into
        // a chest, which the observer asks after (`/machine`).
        18 => {
            let k = gate.kiln;
            let (core, wheel) = (k, k + IVec3::new(3, 0, 0));
            let (hopper, boiler, chest) = (k, k + IVec3::new(2, 0, -1), k + IVec3::new(2, 2, 0));
            let teardown = move |world: &mut World| {
                let (a, b) = (k + IVec3::new(-1, -1, -1), k + IVec3::new(4, 2, 3));
                let net = world.resource::<Net>();
                for line in ["creatures remove aurochs".to_owned(), format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z), "clear".to_owned()] {
                    net.command(&line);
                }
                for line in ["give oak_planks 64", "give cobblestone 64", "give glass 32", "give glowstone 16"] {
                    net.command(line);
                }
            };
            let content = world.resource::<Launch>().content.clone();
            let english = |words: &voxmar_engine::lang::Words| content.english.render(words, &content.blocks);
            let said: Vec<String> = gate.observer.as_ref().map(|o| o.replies.iter().map(english).collect()).unwrap_or_default();
            let stage = gate.machines_stage;
            // The machine each stage uses, and the block that must have come before it.
            let (target, needs) = match stage {
                0..=2 => (core, "base:treadwheel"),
                _ => (if stage == 3 { boiler } else { hopper }, "base:chest"),
            };
            let there = if stage < 3 { wheel } else { chest };
            let client = world.resource::<ClientWorld>();
            if client.block(there) != client.registry.id(needs) {
                if in_phase > WORKSHOP_WAIT && gate.server_now - gate.machines_since > 10.0 {
                    teardown(world);
                    fail(world, gate, "bloomery and logistics", format!("the {} never came", if stage < 3 { "bloomery and treadwheel" } else { "hopper line" }));
                }
                return;
            }
            let (ore, coal, copper, bloom) = (holding(world, "iron_ore").0, holding(world, "coal_ore").0, holding(world, "copper_ore").0, holding(world, "iron_bloom").0);
            // Each stage is done by what the player holds, first seen in hand and then gone
            // after the machine was used (as in the steps before), or what the server says.
            let held = match stage {
                0 => ore,
                1 | 3 => coal,
                4 => copper,
                _ => 0,
            };
            gate.machines_seen |= held > 0;
            let put_in = gate.machines_seen && held == 0 && gate.machines_asked > f32::MIN;
            let done = match stage {
                0 | 1 | 3 | 4 => put_in,
                2 => bloom >= 1,
                _ => said.iter().any(|r| r.contains("holds 4 items")),
            };
            if done {
                let net = world.resource::<Net>();
                match stage {
                    0 => net.command("give coal_ore 1"),
                    1 => net.command("clear"),
                    2 => {
                        // The bloomery's work is shown; its place goes to the hopper line.
                        teardown(world);
                        let at = |d: IVec3| { let p = k + d; format!("{} {} {}", p.x, p.y, p.z) };
                        let net = world.resource::<Net>();
                        for line in [
                            format!("setblock {} hopper", at(IVec3::ZERO)),
                            format!("setblock {} conveyor", at(IVec3::X)),
                            format!("setblock {} steam_crusher", at(IVec3::new(2, 0, 0))),
                            format!("setblock {} boiler", at(IVec3::new(2, 0, -1))),
                            format!("setblock {} chute", at(IVec3::new(2, 1, 0))),
                            format!("setblock {} chest", at(IVec3::new(2, 2, 0))),
                            "clear".to_owned(),
                            "give coal_ore 2".to_owned(),
                        ] {
                            net.command(&line);
                        }
                    }
                    3 => net.command("give copper_ore 2"),
                    4 => {}
                    _ => {
                        gate.check(
                            "workshop: bloomery and logistics",
                            Ok("an aurochs on the treadwheel blew the bellows and the Bellows Bloomery made an iron bloom; ore put in a hopper rode a conveyor into the crusher and 4 dust came up a chute into the chest".into()),
                        );
                        teardown(world);
                        // In their place, the Iron age: a Coke Oven (kiln bricks three on a
                        // side round a hollow middle, the oven in its south face) west of a
                        // Blast Furnace (the bloomery's ring four high, bellows in it) blown by
                        // an aurochs on a treadwheel.
                        let at = |d: IVec3| { let p = k + d; format!("{} {} {}", p.x, p.y, p.z) };
                        let net = world.resource::<Net>();
                        for line in [
                            format!("fill {} {} cobblestone", at(IVec3::new(2, -1, 1)), at(IVec3::new(4, -1, 3))),
                            format!("fill {} {} kiln_bricks", at(IVec3::new(-1, 0, -1)), at(IVec3::new(1, 3, 1))),
                            format!("fill {} {} air", at(IVec3::Y), at(IVec3::new(0, 3, 0))),
                            format!("setblock {} blast_furnace", at(IVec3::ZERO)),
                            format!("setblock {} bellows", at(IVec3::X)),
                            format!("setblock {} shaft", at(IVec3::new(2, 0, 0))),
                            format!("setblock {} treadwheel", at(IVec3::new(3, 0, 0))),
                            format!("fill {} {} kiln_bricks", at(IVec3::new(-4, 0, -1)), at(IVec3::new(-2, 2, 1))),
                            format!("setblock {} air", at(IVec3::new(-3, 1, 0))),
                            format!("setblock {} coke_oven", at(IVec3::new(-3, 1, 1))),
                            format!("summon aurochs 1 {}", at(IVec3::new(3, 0, 2))),
                            "creatures tame aurochs".to_owned(),
                            "creatures hold aurochs".to_owned(),
                            "clear".to_owned(),
                            "give coal_ore 1".to_owned(),
                        ] {
                            net.command(&line);
                        }
                        gate.machines_stage = 0;
                        gate.machines_since = gate.server_now;
                        gate.machines_asked = f32::MIN;
                        gate.machines_held = gate.server_now;
                        gate.machines_seen = false;
                        gate.advance(now);
                        return;
                    }
                }
                gate.machines_stage += 1;
                gate.machines_since = gate.server_now;
                gate.machines_asked = f32::MIN;
                gate.machines_seen = false;
                return;
            }
            // The aurochs is held still again before its minute runs out.
            if stage <= 2 && gate.server_now - gate.machines_held >= 20.0 {
                gate.machines_held = gate.server_now;
                world.resource::<Net>().command("creatures hold aurochs");
            }
            // Used (or asked about) only every few server seconds, as the kiln is.
            if gate.server_now - gate.machines_asked >= 3.0 {
                let ready = match stage {
                    0 => select(world, "iron_ore"),
                    1 | 3 => select(world, "coal_ore"),
                    4 => select(world, "copper_ore"),
                    _ => world.resource::<LocalPlayer>().inventory.slots.iter().all(Option::is_none),
                };
                if ready {
                    gate.machines_asked = gate.server_now;
                    // While the bloom is worked, and while the dust travels, the observer asks
                    // how the wheel or the chest is doing.
                    if stage >= 2
                        && let Some(observer) = gate.observer.as_mut()
                    {
                        let asked = if stage == 2 { wheel } else { chest };
                        observer.replies.clear();
                        observer.connection.send(ClientMessage::Command { line: format!("machine {} {} {}", asked.x, asked.y, asked.z) });
                    }
                    if stage != 5 {
                        let actions = world.resource::<crate::Actions>().0.clone();
                        world.resource::<Net>().propose(actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
                    }
                }
            }
            // No progress for 40 seconds of the server's time; the game's clock only catches a
            // server that stopped.
            if gate.server_now - gate.machines_since > 40.0 || in_phase > 300.0 {
                let what = [
                    "the bloomery never took the iron ore",
                    "the bloomery never took the coal",
                    "the bloomery never gave an iron bloom",
                    "the boiler never took the coal",
                    "the hopper never took the copper ore",
                    "the chest never held 4 dust",
                ][stage.min(5) as usize];
                let waited = (gate.server_now - gate.machines_since, in_phase);
                teardown(world);
                fail(world, gate, "bloomery and logistics", format!("{what} ({:.1} server s, {:.1} game s): holding iron ore {ore}, coal {coal}, copper ore {copper}, blooms {bloom}; /machine said {said:?}", waited.0, waited.1));
            }
        }
        // The Iron age. A coal goes into the Coke Oven, which says it is making coke (by its
        // own heat: the structure stands); two iron ore and a coke go into the Blast Furnace,
        // which an aurochs on the treadwheel blows, and it says it is making iron. The coke
        // and the four ingots themselves take half a minute each: the server tests wait for
        // them (`a_coke_oven_bakes_coal_into_coke_...`).
        19 => {
            let k = gate.kiln;
            let (furnace, wheel, oven) = (k, k + IVec3::new(3, 0, 0), k + IVec3::new(-3, 1, 1));
            let teardown = move |world: &mut World| {
                let (a, b) = (k + IVec3::new(-4, -1, -1), k + IVec3::new(4, 3, 3));
                let net = world.resource::<Net>();
                for line in ["creatures remove aurochs".to_owned(), format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z), "clear".to_owned()] {
                    net.command(&line);
                }
                for line in ["give oak_planks 64", "give cobblestone 64", "give glass 32", "give glowstone 16"] {
                    net.command(line);
                }
            };
            let content = world.resource::<Launch>().content.clone();
            let english = |words: &voxmar_engine::lang::Words| content.english.render(words, &content.blocks);
            let said: Vec<String> = gate.observer.as_ref().map(|o| o.replies.iter().map(english).collect()).unwrap_or_default();
            let stage = gate.machines_stage;
            let client = world.resource::<ClientWorld>();
            if client.block(oven) != client.registry.id("base:coke_oven") || client.block(wheel) != client.registry.id("base:treadwheel") {
                if in_phase > WORKSHOP_WAIT && gate.server_now - gate.machines_since > 10.0 {
                    teardown(world);
                    fail(world, gate, "iron age", "the coke oven and blast furnace never came".into());
                }
                return;
            }
            let (coal, ore, coke) = (holding(world, "coal_ore").0, holding(world, "iron_ore").0, holding(world, "coke").0);
            // Put in: first seen in hand, then gone after the machine was used. Working: what
            // the server says when asked.
            let held = match stage {
                0 => coal,
                2 => ore,
                3 => coke,
                _ => 0,
            };
            gate.machines_seen |= held > 0;
            let put_in = gate.machines_seen && held == 0 && gate.machines_asked > f32::MIN;
            let done = match stage {
                0 | 2 | 3 => put_in,
                // Names compared without case, so a renamed capital doesn't stall the step.
                1 => said.iter().any(|r| r.to_lowercase().contains("making coke")),
                _ => said.iter().any(|r| r.to_lowercase().contains("making iron ingot")),
            };
            if done {
                let net = world.resource::<Net>();
                match stage {
                    0 => {}
                    1 => {
                        net.command("give iron_ore 2");
                        net.command("give coke 1");
                    }
                    2 | 3 => {}
                    _ => {
                        // Both structures in view, between them.
                        aim(world, k + IVec3::new(-1, 1, 0));
                        if gate.shot(world, "iron_age").is_none() {
                            return;
                        }
                        gate.check(
                            "workshop: iron age",
                            Ok("a coal in the Coke Oven set it baking coke; two iron ore and a coke in the Blast Furnace, blown by an aurochs on the treadwheel, set it making iron".into()),
                        );
                        teardown(world);
                        end_workshop(world, gate, now);
                        return;
                    }
                }
                gate.machines_stage += 1;
                gate.machines_since = gate.server_now;
                gate.machines_asked = f32::MIN;
                gate.machines_seen = false;
                return;
            }
            // The aurochs is held still again before its minute runs out.
            if gate.server_now - gate.machines_held >= 20.0 {
                gate.machines_held = gate.server_now;
                world.resource::<Net>().command("creatures hold aurochs");
            }
            // Used (or asked about) only every few server seconds, as the kiln is.
            if gate.server_now - gate.machines_asked >= 3.0 {
                let ready = match stage {
                    0 => select(world, "coal_ore"),
                    2 => select(world, "iron_ore"),
                    3 => select(world, "coke"),
                    _ => true,
                };
                if ready {
                    gate.machines_asked = gate.server_now;
                    if stage == 1 || stage >= 4 {
                        let asked = if stage == 1 { oven } else { furnace };
                        if let Some(observer) = gate.observer.as_mut() {
                            observer.replies.clear();
                            observer.connection.send(ClientMessage::Command { line: format!("machine {} {} {}", asked.x, asked.y, asked.z) });
                        }
                    } else {
                        let target = if stage == 0 { oven } else { furnace };
                        let actions = world.resource::<crate::Actions>().0.clone();
                        world.resource::<Net>().propose(actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
                    }
                }
            }
            // No progress for 40 seconds of the server's time; the game's clock only catches a
            // server that stopped.
            if gate.server_now - gate.machines_since > 40.0 || in_phase > 300.0 {
                let what = [
                    "the coke oven never took the coal",
                    "the coke oven never said it was making coke",
                    "the blast furnace never took the iron ore",
                    "the blast furnace never took the coke",
                    "the blast furnace never said it was making iron",
                ][stage.min(4) as usize];
                let waited = (gate.server_now - gate.machines_since, in_phase);
                teardown(world);
                fail(world, gate, "iron age", format!("{what} ({:.1} server s, {:.1} game s): holding coal {coal}, iron ore {ore}, coke {coke}; /machine said {said:?}", waited.0, waited.1));
            }
        }
        _ => {}
    }
}

/// Every item the player carries, pouches and all.
fn carried(world: &World) -> u32 {
    let inventory = &world.resource::<LocalPlayer>().inventory;
    inventory.slots.iter().flatten().map(|s| s.count as u32).sum::<u32>() + inventory.pouched.iter().map(|&(_, n)| n as u32).sum::<u32>()
}

/// Puts the workshop's tools away and goes on to the pool.
fn end_workshop(world: &mut World, gate: &mut Gate, now: f32) {
    mouse(world, MouseButton::Left, false);
    mouse(world, MouseButton::Right, false);
    world.resource_mut::<UiState>().inventory_open = false;
    let net = world.resource::<Net>();
    for tool in ["stone_pickaxe", "wooden_hoe", "crafting_table", "stick", "bread", "fishing_rod"] {
        net.command(&format!("clear {tool}"));
    }
    let feet = gate.feet;
    let line = format!("tp {} {} {}", feet.x as f32 + 4.5, feet.y - 2, feet.z as f32 + 0.5);
    net.command(&line);
    gate.next(Step::Swim, now);
}

/// In the pool, facing the bank with the step on it, holding forward and jump climbs out
/// onto the step, and the server agrees where the player ends up.
fn swim(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let feet = gate.feet;
    let step_top = (feet.y + 1) as f32;
    let player = world.resource::<LocalPlayer>();
    let (position, in_water, on_ground) = (player.position, player.in_water, player.on_ground);
    match gate.phase {
        // Teleported into the water (in `Collect`); wait until in it.
        0 if in_water && position.x > (feet.x + 3) as f32 && position.y < feet.y as f32 => {
            look(world, -90.0, 0.0);
            key(world, KeyCode::KeyW, true);
            key(world, KeyCode::Space, true);
            gate.highest = position.y;
            gate.advance(now);
        }
        1 => {
            gate.highest = gate.highest.max(position.y);
            // Out of the water at the step's height, jump is let go so the player lands on
            // the step and stays: held, a frame on the ground is easy to miss on a loaded
            // machine, and the player hopped on and off the test ground.
            if !in_water && position.y >= step_top - 0.01 {
                key(world, KeyCode::Space, false);
            }
            if on_ground && position.y >= step_top - 0.01 {
                key(world, KeyCode::KeyW, false);
                key(world, KeyCode::Space, false);
                gate.check("swim", Ok(format!("climbed out of the water onto the step in {in_phase:.1}s")));
                gate.advance(now);
            } else if in_phase > 8.0 {
                key(world, KeyCode::KeyW, false);
                key(world, KeyCode::Space, false);
                let reached = gate.highest - feet.y as f32;
                gate.check("swim", Err(format!("did not climb out onto the step: feet rose to {reached:+.2} against the step's +1")));
                gate.next(Step::Screens, now);
            }
        }
        2 => {
            let server = gate.observer.as_ref().and_then(|o| o.others.values().next().copied());
            if server.is_none_or(|at| at.distance(position) >= 1.0) && in_phase < 3.0 {
                return;
            }
            let result = match server {
                Some(at) if at.distance(position) < 1.0 => Ok(format!("the server has the player within {:.2} blocks", at.distance(position))),
                Some(at) => Err(format!("the server has the player at {at}, the client at {position}")),
                None => Err("the server told the observer of no player".into()),
            };
            gate.check("swim: server", result);
            gate.next(Step::Screens, now);
        }
        _ => {}
    }
}

/// Esc opens the menu and closes it again; E the inventory, C from there the character
/// screen in its place, and C closes that.
/// How long a screen is given to open, close or be shot: on a machine loaded far past its
/// cores a shot alone can take seconds.
const SCREEN_WAIT: f32 = 10.0;

/// How long PostUpdate is timed with the debug overlay closed and then open, in seconds
/// (after a moment to settle), and the least number of frames each.
const OVERLAY_TIMED: f32 = 2.0;
const OVERLAY_FRAMES: usize = 60;

/// F3 opens the debug overlay and closes it again, and while it is open its text changes
/// only a few times a second: an overlay that rewrote its text every frame had UI layout
/// redone every frame, 10 to 25 ms of Mariano's slow frames. That is judged by counting
/// the changes, which a loaded machine cannot blur; PostUpdate's time with the overlay
/// closed and open is reported beside it (before the fix it rose 0.77 ms in the median,
/// within the noise of a loaded machine).
fn overlay(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let shown = world.resource::<Settings>().debug_overlay;
    let ms = world.resource::<PostUpdateTime>().ms;
    let timed = |gate: &Gate, i: usize| in_phase > OVERLAY_TIMED + 0.5 && gate.overlay_ms[i].len() >= OVERLAY_FRAMES;
    match gate.phase {
        0 => {
            if shown {
                key(world, KeyCode::F3, true);
                key(world, KeyCode::F3, false);
                return;
            }
            if in_phase > 0.5 {
                gate.overlay_ms[0].push(ms);
            }
            if timed(gate, 0) {
                key(world, KeyCode::F3, true);
                gate.advance(now);
            }
        }
        1 => {
            key(world, KeyCode::F3, false);
            if !shown {
                if in_phase > SCREEN_WAIT {
                    gate.check("overlay", Err("F3 did not open the debug overlay".into()));
                    gate.next(Step::Weather, now);
                }
                return;
            }
            if in_phase > 0.5 {
                gate.overlay_ms[1].push(ms);
                // An exclusive system sees changes made since its last run: this frame's.
                let mut text = world.query_filtered::<Ref<Text>, With<crate::hud::DebugText>>();
                if text.iter(world).any(|text| text.is_changed()) {
                    gate.overlay_changes += 1;
                }
                if gate.overlay_counted.0 == 0.0 {
                    gate.overlay_counted.0 = now;
                }
                gate.overlay_counted.1 = now;
            }
            if timed(gate, 1) {
                key(world, KeyCode::F3, true);
                gate.advance(now);
            }
        }
        2 => {
            key(world, KeyCode::F3, false);
            if shown {
                if in_phase > SCREEN_WAIT {
                    gate.check("overlay", Err("F3 did not close the debug overlay".into()));
                    gate.next(Step::Weather, now);
                }
                return;
            }
            let stats = |times: &[f32]| {
                let mut sorted = times.to_vec();
                sorted.sort_by(f32::total_cmp);
                let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
                (at(0.5), at(0.9))
            };
            let ((closed, closed_p90), (open, open_p90)) = (stats(&gate.overlay_ms[0]), stats(&gate.overlay_ms[1]));
            let seconds = (gate.overlay_counted.1 - gate.overlay_counted.0).max(0.1);
            let rate = gate.overlay_changes as f32 / seconds;
            let most = 1.0 / crate::hud::DEBUG_REFRESH + 1.0;
            let detail = format!(
                "its text changed on {} of {} frames in {seconds:.1}s, {rate:.1} a second (at most {most:.0}); \
                 PostUpdate median {open:.2} ms open against {closed:.2} ms closed, p90 {open_p90:.2} against {closed_p90:.2} ms",
                gate.overlay_changes,
                gate.overlay_ms[1].len(),
            );
            let result = if gate.overlay_changes > 0 && rate <= most { Ok(detail) } else { Err(detail) };
            gate.check("overlay: no layout every frame", result);
            gate.next(Step::Weather, now);
        }
        _ => {}
    }
}

fn screens(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let ui = world.resource::<UiState>();
    let (menu, inventory, character, locked) = (ui.menu_open, ui.inventory_open, ui.character_open, ui.cursor_locked);
    match gate.phase {
        0 => {
            look(world, 0.0, 5.0);
            // The loading cover lifted only once the world's weather showed.
            if !gate.cover_checked {
                gate.cover_checked = true;
                let covered = world.query_filtered::<(), With<crate::menu::loading::Cover>>().iter(world).count() > 0;
                let weather = world.resource::<crate::weather::WeatherReady>().0;
                let result = match (covered, weather) {
                    (false, true) => Ok("lifted, with the world's weather showing".to_owned()),
                    (true, _) => Err("still up a whole step into the game".to_owned()),
                    (false, false) => Err("lifted before the weather was ready".to_owned()),
                };
                gate.check("loading cover", result);
            }
            if gate.shot(world, "game").is_some() {
                // The HUD reads health off the server; there is no hunger to show.
                let (health, max_health) = {
                    let player = world.resource::<LocalPlayer>();
                    (player.health, player.max_health)
                };
                let reading = format!("{health}/{max_health}");
                let mut texts = world.query::<&Text>();
                let meters = texts.iter(world).filter(|text| text.0 == reading).count();
                gate.check(
                    "vitals",
                    if health < max_health {
                        Err(format!("the player has lost health: {reading}"))
                    } else if meters >= 1 {
                        Ok(format!("health {reading}, shown on {meters} meters"))
                    } else {
                        Err(format!("no meter reads the health {reading}"))
                    },
                );
                key(world, KeyCode::Escape, true);
                gate.advance(now);
            }
        }
        1 => {
            key(world, KeyCode::Escape, false);
            if menu && in_phase > 0.2 && let Some(shot) = gate.shot(world, "menu") {
                let change = gate.earlier_shot("game").map_or(0.0, |game| change(&game, &shot, 0.0));
                let shown = if change >= SCREEN_CHANGE { Ok(format!("drawn, changing the screen by {change:.1} levels")) } else { Err(format!("changed the screen by only {change:.1} levels")) };
                gate.check("menu", shown.and_then(|s| if locked { Err("the mouse stayed grabbed".into()) } else { Ok(s) }));
                set_language(world, "ja");
                gate.advance(now);
            } else if !menu && in_phase > SCREEN_WAIT {
                gate.check("menu", Err("Esc did not open the menu".into()));
                gate.next(Step::Weather, now);
            }
        }
        2 => {
            // The open menu, rewritten in Japanese and back again.
            if in_phase > 0.2 && let Some(shot) = gate.shot(world, "menu-ja") {
                let words = localized_text(world, "menu.quit_game");
                let japanese = world.resource::<Locale>().get("menu.quit_game").to_owned();
                let change = gate.earlier_shot("menu").map_or(0.0, |menu| change(&menu, &shot, 0.0));
                gate.check(
                    "language",
                    if words.as_deref() != Some(japanese.as_str()) || japanese == "Quit game" {
                        Err(format!("the menu still reads {words:?} in Japanese"))
                    } else if !crate::lang::smooth_fonts_ready(world) {
                        Err("kana and kanji have no font to fall back to".into())
                    } else if change < 1.0 {
                        Err(format!("Japanese changed the screen by only {change:.1} levels"))
                    } else {
                        Ok(format!("the menu reads {japanese}, changing the screen by {change:.1} levels"))
                    },
                );
                set_language(world, "en");
                // And another interface font, as the setting changes it.
                let mut settings = world.resource_mut::<crate::settings::Settings>();
                gate.font = Some(settings.font);
                settings.font = crate::ui::skin::InterfaceFont::ALL.into_iter().find(|f| *f != settings.font).unwrap_or_default();
                gate.advance(now);
            }
        }
        3 => {
            if in_phase > 0.2 {
                let words = localized_text(world, "menu.quit_game");
                let back = if words.as_deref() == Some("Quit game") { Ok("back in English".into()) } else { Err(format!("reads {words:?} after switching back")) };
                gate.check("language: English", back);
                let font = world.resource::<crate::settings::Settings>().font;
                gate.check(
                    "font change",
                    match crate::lang::texts_without_a_font(world) {
                        None => Ok(format!("every text has {}", font.label())),
                        Some(lost) => Err(format!("in {}, {lost} have no font: every letter a box", font.label())),
                    },
                );
                if let Some(font) = gate.font.take() {
                    world.resource_mut::<crate::settings::Settings>().font = font;
                }
                key(world, KeyCode::Escape, true);
                gate.advance(now);
            }
        }
        4 => {
            key(world, KeyCode::Escape, false);
            if !menu && locked {
                gate.check("menu: close", Ok("Esc closed it and gave the game the mouse back".into()));
                key(world, KeyCode::KeyE, true);
                gate.advance(now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("menu: close", Err(format!("after Esc the menu is {} and the mouse {}", if menu { "open" } else { "closed" }, if locked { "grabbed" } else { "free" })));
                gate.next(Step::Weather, now);
            }
        }
        5 => {
            key(world, KeyCode::KeyE, false);
            if inventory && in_phase > 0.2 && let Some(shot) = gate.shot(world, "inventory") {
                let change = gate.earlier_shot("game").map_or(0.0, |game| change(&game, &shot, 0.0));
                let shown = if change >= SCREEN_CHANGE { Ok(format!("drawn, changing the screen by {change:.1} levels")) } else { Err(format!("changed the screen by only {change:.1} levels")) };
                gate.check("inventory", shown);
                key(world, KeyCode::KeyC, true);
                gate.advance(now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("inventory", Err("E did not open the inventory".into()));
                gate.next(Step::Weather, now);
            }
        }
        6 => {
            key(world, KeyCode::KeyE, false);
            key(world, KeyCode::KeyC, false);
            if character && !inventory && in_phase > 0.2 && let Some(shot) = gate.shot(world, "character") {
                let change = gate.earlier_shot("inventory").map_or(0.0, |inventory| change(&inventory, &shot, 0.0));
                let shown = if change >= SCREEN_CHANGE {
                    Ok(format!("C in the inventory switched to it, changing the screen by {change:.1} levels"))
                } else {
                    Err(format!("changed the screen from the inventory's by only {change:.1} levels"))
                };
                gate.check("character", shown);
                // Every skill has its tile, all of them on screen, three across.
                let (skills, tiles, inside) = crate::hud::character::skill_tiles(world);
                gate.check(
                    "character: skills",
                    if skills > 0 && tiles == skills && inside == skills { Ok(format!("all {skills} skills on screen")) } else { Err(format!("{skills} skills, {tiles} tiles shown, {inside} on screen")) },
                );
                key(world, KeyCode::KeyC, true);
                gate.advance(now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("character", Err(format!("after C the inventory is {} and the character screen {}", if inventory { "open" } else { "closed" }, if character { "open" } else { "closed" })));
                gate.next(Step::Weather, now);
            }
        }
        7 => {
            key(world, KeyCode::KeyC, false);
            if !character && !inventory && locked {
                gate.check("character: close", Ok("C closed it and gave the game the mouse back".into()));
                // A log to craft with.
                world.resource::<Net>().command("give oak_log 1");
                gate.advance(now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("character: close", Err("C did not close the character screen".into()));
                gate.next(Step::Weather, now);
            }
        }
        // The log goes into the crafting grid and comes out as planks, as the inventory's
        // clicks would do it; the server does the crafting.
        8 => {
            let log = world.resource::<crate::session::Launch>().content.blocks.id("base:oak_log");
            let held = world.resource::<LocalPlayer>().inventory.slots.iter().position(|s| s.is_some_and(|s| Some(s.item) == log));
            if let Some(slot) = held {
                let actions = world.resource::<crate::Actions>().0.clone();
                let net = world.resource::<Net>();
                let target = voxmar_engine::action::ActionTarget::Slots { from: slot as u8, to: voxmar_engine::inventory::CRAFT as u8, count: 1 };
                net.propose(actions.expect("core:transfer"), target);
                net.propose(actions.expect("core:craft"), voxmar_engine::action::ActionTarget::Slot(0));
                gate.advance(now);
            } else if in_phase > 5.0 {
                gate.check("craft", Err("the log to craft with never arrived".into()));
                gate.next(Step::Weather, now);
            }
        }
        9 => {
            let planks = world.resource::<crate::session::Launch>().content.blocks.id("base:oak_planks");
            let inventory = &world.resource::<LocalPlayer>().inventory;
            let count: u32 = inventory.slots.iter().flatten().filter(|s| Some(s.item) == planks).map(|s| s.count as u32).sum();
            if count >= 4 {
                gate.check("craft", Ok(format!("the crafting grid turned a log into {count} planks")));
                gate.next(Step::Overlay, now);
            } else if in_phase > 5.0 {
                gate.check("craft", Err(format!("after crafting a log there are {count} planks and the grid holds {:?}", inventory.grid())));
                gate.next(Step::Overlay, now);
            }
        }
        _ => {}
    }
}

/// The recipe viewer: U over an oak log in the inventory lists what it goes into, then an
/// item clicked in the viewer shows its own recipes, and Back returns.
fn recipes(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::recipes::{HoveredItem, RecipeViewer};
    let blocks = world.resource::<Launch>().content.blocks.clone();
    let (Some(log), Some(planks), Some(campfire)) = (blocks.id("base:oak_log"), blocks.id("base:oak_planks"), blocks.id("base:campfire")) else {
        gate.check("recipes", Err("no oak log, planks or campfire in the content".into()));
        gate.next(Step::Effects, now);
        return;
    };
    let uses_key = world.resource::<crate::settings::Settings>().uses_key.0;
    match gate.phase {
        0 => {
            let mut ui = world.resource_mut::<UiState>();
            (ui.menu_open, ui.character_open, ui.console_open, ui.inventory_open) = (false, false, false, true);
            // As if the pointer were over a log, the uses key pressed.
            world.resource_mut::<HoveredItem>().0 = Some(log);
            key(world, uses_key, true);
            gate.advance(now);
        }
        1 => {
            key(world, uses_key, false);
            let viewer = world.resource::<RecipeViewer>();
            let outputs: Vec<BlockId> = viewer.found().iter().map(|entry| entry.output).collect();
            let open = world.resource::<UiState>().recipes_open;
            if open && outputs.contains(&planks) && outputs.contains(&campfire) {
                gate.check("recipes: uses", Ok(format!("U over an oak log lists {} uses, planks and the campfire among them", outputs.len())));
                // As if planks were clicked in the viewer.
                world.resource_mut::<RecipeViewer>().open(planks, false);
                gate.advance(now);
            } else if in_phase > 2.0 {
                gate.check("recipes: uses", Err(format!("U over an oak log: viewer open {open}, listing {outputs:?}")));
                gate.next(Step::Effects, now);
            }
        }
        2 => {
            let made: Vec<String> = world.resource::<RecipeViewer>().found().iter().map(|entry| format!("{:?}", entry.method)).collect();
            let title = world.query_filtered::<&Text, With<Localized>>().iter(world).count();
            let crafted = made.iter().any(|m| m.starts_with("Craft") && m.contains(&format!("{log:?}")));
            if crafted || in_phase > 2.0 {
                gate.check("recipes: click", if crafted { Ok(format!("planks show {} recipes, the log in the grid ({title} texts drawn)", made.len())) } else { Err(format!("planks show {made:?}")) });
                world.resource_mut::<RecipeViewer>().trail.pop();
                gate.advance(now);
            }
        }
        _ => {
            let viewer = world.resource::<RecipeViewer>();
            let back = viewer.trail.last().is_some_and(|lookup| lookup.item == log && lookup.uses);
            gate.check("recipes: back", if back { Ok("Back returns to the log's uses".into()) } else { Err(format!("after Back the viewer shows {:?}", viewer.trail.last())) });
            // A log's tooltip says what it goes into, planks among them.
            let lines = {
                let (locale, launch) = (world.resource::<Locale>(), world.resource::<Launch>());
                let planks_name = locale.block(launch.content.blocks.def(planks));
                world.resource::<RecipeViewer>().book().map(|book| (crate::hud::inventory::origins_and_uses(locale, &launch.content, book, log), planks_name))
            };
            gate.check(
                "tooltip: used in",
                match lines {
                    Some((lines, planks)) if lines.iter().any(|line| line.contains(&planks)) => Ok(format!("an oak log's tooltip: {}", lines.join(" / "))),
                    Some((lines, _)) => Err(format!("an oak log's tooltip says {lines:?}")),
                    None => Err("no recipe book to read".into()),
                },
            );
            world.resource_mut::<RecipeViewer>().trail.clear();
            let mut ui = world.resource_mut::<UiState>();
            (ui.recipes_open, ui.inventory_open) = (false, false);
            world.resource_mut::<HoveredItem>().0 = None;
            gate.next(Step::Effects, now);
        }
    }
}

/// A timed effect on the player (as eating would give, the server not told): its chip
/// counts down and its plate, as if the pointer were over the chip, says the same. Judged
/// by two readings a moment apart rather than by where the count starts: the session's own
/// cooking gives the same effect earlier, and on a loaded machine that one is further along.
fn effects(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::effects::{PinnedTip, ShowEffects};
    let reading = |world: &mut World| {
        let chip = world.query_filtered::<&Text, With<crate::hud::EffectTimer>>().iter(world).map(|t| t.0.clone()).find(|t| !t.is_empty());
        let left = crate::hud::effects::shown_left(world);
        (chip, left)
    };
    // Seconds a reading such as "1:28" or "1:28 left" stands for.
    let seconds = |text: &str| {
        let clock = text.split_whitespace().next()?;
        let (m, s) = clock.split_once(':')?;
        Some(m.parse::<u32>().ok()? * 60 + s.parse::<u32>().ok()?)
    };
    match gate.phase {
        0 => {
            // Food eaten earlier in the run (eating heals the hurt) may have given a real
            // one; clear it so the chip shows the effect set here.
            world.resource::<Net>().command("effect clear");
            world.insert_resource(ShowEffects(vec![("hearty".into(), 90)]));
            world.insert_resource(PinnedTip("hearty".into()));
            gate.advance(now);
        }
        1 => {
            let (chip, left) = reading(world);
            let agree = chip.as_deref().and_then(seconds).filter(|&c| left.as_deref().and_then(seconds) == Some(c));
            match agree {
                Some(first) if in_phase > 0.2 => {
                    gate.count_before = first;
                    gate.advance(now);
                }
                _ if in_phase > 3.0 => {
                    gate.check("effects: countdown", Err(format!("chip {chip:?}, plate {left:?}")));
                    world.remove_resource::<ShowEffects>();
                    world.remove_resource::<PinnedTip>();
                    gate.next(Step::Screenshot, now);
                }
                _ => {}
            }
        }
        _ => {
            let (chip, left) = reading(world);
            let (chip_s, left_s) = (chip.as_deref().and_then(seconds), left.as_deref().and_then(seconds));
            let counted = chip_s.is_some_and(|c| c < gate.count_before) && chip_s == left_s;
            // While the server holds the effect itself (the session's cooking gives it), the
            // count moves on the server's clock, which a loaded machine slows: given five of
            // the server's seconds, however long they take.
            if !counted && gate.server_in_phase() < 5.0 {
                return;
            }
            let first = gate.count_before;
            let result = if counted {
                Ok(format!("the chip went from {}:{:02} to {}, the plate with it ({})", first / 60, first % 60, chip.unwrap_or_default(), left.unwrap_or_default()))
            } else {
                Err(format!("from {}:{:02}, chip {chip:?}, plate {left:?}", first / 60, first % 60))
            };
            gate.check("effects: countdown", result);
            world.remove_resource::<ShowEffects>();
            world.remove_resource::<PinnedTip>();
            gate.next(Step::Tips, now);
        }
    }
}

fn tips(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::tips::Tips;
    let showing = |world: &World| world.resource::<Tips>().showing.map(|(name, _)| name);
    match gate.phase {
        // As for a player who has seen every tip but the inventory's, the inventory shut.
        0 => {
            let others = crate::hud::tips::TIPS.iter().filter(|name| **name != "inventory").map(|name| name.to_string()).collect();
            world.resource_mut::<crate::settings::Settings>().tips_seen = others;
            *world.resource_mut::<Tips>() = Tips::default();
            world.resource_mut::<UiState>().inventory_open = false;
            gate.advance(now);
        }
        1 if in_phase > 0.2 => {
            world.resource_mut::<UiState>().inventory_open = true;
            gate.advance(now);
        }
        2 => {
            let tip = showing(world);
            if tip.is_none() && in_phase < 3.0 {
                return;
            }
            gate.check("tips: first look", match tip { Some(name) => Ok(format!("opening the inventory brought the {name} tip")), None => Err("opening the inventory brought no tip".into()) });
            // Gone, and the inventory shut and opened again: it does not come back.
            world.resource_mut::<Tips>().dismiss();
            world.resource_mut::<UiState>().inventory_open = false;
            gate.advance(now);
        }
        3 if in_phase > 0.2 => {
            world.resource_mut::<UiState>().inventory_open = true;
            gate.advance(now);
        }
        4 if in_phase > 1.0 => {
            let again = showing(world).filter(|name| *name == "inventory");
            gate.check("tips: once", if again.is_none() { Ok("seen once, it does not come back".into()) } else { Err("the inventory tip came back".into()) });
            world.resource_mut::<UiState>().inventory_open = false;
            gate.next(Step::Guides, now);
        }
        _ => {}
    }
}

fn guides(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::character::{Guide, GuideView};
    let skills = &world.resource::<Launch>().content.skills;
    let Some(farming) = skills.ids().find(|&id| skills.get(id).name.ends_with("farming")) else {
        gate.check("guides", Err("no Farming skill in the content".into()));
        gate.next(Step::Journal, now);
        return;
    };
    match gate.phase {
        0 => {
            world.resource_mut::<UiState>().character_open = true;
            world.resource_mut::<Guide>().0 = Some(farming);
            world.resource_mut::<GuideView>().category = Some("planting".into());
            gate.advance(now);
        }
        1 => {
            let (lines, kinds) = crate::hud::character::guide_buttons(world);
            if lines == 0 && in_phase < 2.0 {
                return;
            }
            gate.check("guides: kinds", if kinds >= 3 && lines > 0 { Ok(format!("{kinds} kinds down the left, {lines} planting lines")) } else { Err(format!("{kinds} kinds and {lines} lines")) });
            let first = {
                let locale = world.resource::<Locale>().clone();
                let launch = world.resource::<Launch>();
                crate::hud::guide::entries(&locale, launch, farming).iter().position(|e| e.category == "planting")
            };
            world.resource_mut::<GuideView>().entry = first;
            gate.advance(now);
        }
        _ => {
            let needs = world.resource::<Locale>().get("guide.needs").to_owned();
            let mut texts = world.query::<&Text>();
            let shown = texts.iter(world).any(|t| t.0 == needs);
            if !shown && in_phase < 2.0 {
                return;
            }
            gate.check("guides: page", if shown { Ok("a planting line opens its page".into()) } else { Err("a line's page never showed what it needs".into()) });
            *world.resource_mut::<GuideView>() = GuideView::default();
            world.resource_mut::<Guide>().0 = None;
            world.resource_mut::<UiState>().character_open = false;
            gate.next(Step::Journal, now);
        }
    }
}

fn journal(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use voxmar_engine::journal::Page;
    let key_code = world.resource::<crate::settings::Settings>().journal_key.0;
    match gate.phase {
        0 => {
            let plants = Page::ALL.iter().position(|page| *page == Page::Plants).unwrap_or(0);
            world.resource_mut::<crate::hud::journal::JournalView>().page = plants;
            key(world, key_code, true);
            gate.advance(now);
        }
        _ => {
            key(world, key_code, false);
            let open = world.resource::<UiState>().journal_open;
            let cells = world.query_filtered::<(), With<Button>>().iter(world).count();
            if !open && in_phase < 2.0 {
                return;
            }
            let found = world.resource::<LocalPlayer>().journal.entries().count();
            gate.check("journal", if open { Ok(format!("J opened the journal ({found} things found so far, {cells} buttons drawn)")) } else { Err("J did not open the journal".into()) });
            world.resource_mut::<UiState>().journal_open = false;
            gate.next(Step::Quests, now);
        }
    }
}

/// The torch quest the tracker follows, and the quest it sees done.
const TRACKED: &str = "base:torchlight";
const DONE_QUEST: &str = "base:crafting_table";
/// A quest the session finishes before the book opens, for the line out of it.
const DONE_FIRST: &str = "base:twine_and_thong";
/// Longest the quest stages wait for the server, in its seconds.
const QUEST_WAIT: f32 = 10.0;

/// The torch quest's first goal as the server last told it: (have, need).
fn tracked(world: &World) -> (u32, u32) {
    let player = world.resource::<LocalPlayer>();
    player.quests.iter().find(|q| q.name == TRACKED).and_then(|q| q.tasks.first()?.first().copied()).unwrap_or((0, 1))
}

/// The words the tracker shows for the torch quest at `have` torches, or done.
fn tracker_words(world: &World, have: u32) -> String {
    let (locale, launch) = (world.resource::<Locale>(), world.resource::<Launch>());
    let Some(quest) = launch.content.quests.get(TRACKED) else { return String::new() };
    let Some(goal) = quest.tasks.first().and_then(|t| t.first()) else { return String::new() };
    let need = if let voxmar_engine::quest::Goal::Craft(_, need) = goal { *need } else { 1 };
    if have >= need {
        return locale.get("quest.tracker.done").to_owned();
    }
    let met = locale.get("quest.mark.todo");
    format!("{met} {}", crate::hud::quests::goal_words(locale, &launch.content, quest, goal, (have, need)))
}

/// The tracker's lines, when it is shown.
fn tracker_lines(world: &mut World) -> Option<Vec<String>> {
    use crate::hud::quests::{Tracker, TrackerLine};
    let shown = world.query_filtered::<&Visibility, With<Tracker>>().iter(world).any(|v| *v != Visibility::Hidden);
    shown.then(|| world.query_filtered::<&Text, With<TrackerLine>>().iter(world).map(|t| t.0.clone()).collect())
}

/// The quest book and the tracker, played: L opens the book on a quest worth reading, W
/// held with it open does not walk, Esc shuts it and grabs the mouse again; a quest
/// pinned stands at the right and counts torches crafted for real, as the server counts
/// them; a quest done reads so, then drops off and the tracker goes.
fn quests(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::quests::{QuestView, toggle_pin};
    let key_code = world.resource::<crate::settings::Settings>().quests_key.0;
    let waited = gate.server_in_phase();
    let known = {
        let quests = &world.resource::<Launch>().content.quests;
        [TRACKED, DONE_QUEST, DONE_FIRST].iter().all(|name| quests.get(name).is_some())
    };
    if !known {
        gate.check("quests", Err(format!("the content lacks {TRACKED}, {DONE_QUEST} or {DONE_FIRST}")));
        gate.next(Step::Catalogue, now);
        return;
    }
    let quit = |world: &mut World, gate: &mut Gate, what: &str, why: String| {
        gate.check(what, Err(why));
        for code in [key_code, KeyCode::KeyW, KeyCode::Escape] {
            key(world, code, false);
        }
        world.resource_mut::<UiState>().quests_open = false;
        world.resource_mut::<crate::settings::Settings>().pinned_quests.clear();
        gate.next(Step::Catalogue, now);
    };
    match gate.phase {
        // Ten twine, from fifteen wheat in three stacks of five, to finish Twine and Thong, so
        // the book shows a quest done and the line out of it gold.
        0 => {
            world.resource_mut::<crate::settings::Settings>().pinned_quests.clear();
            let net = world.resource::<Net>();
            net.command("clear wheat");
            net.command("give wheat 15");
            gate.advance(now);
        }
        1 => {
            let (wheat, slot) = holding(world, "base:wheat");
            match slot {
                Some(slot) if wheat >= 15 => {
                    let actions = world.resource::<crate::Actions>().0.clone();
                    let net = world.resource::<Net>();
                    for cell in 0..3 {
                        let target = voxmar_engine::action::ActionTarget::Slots { from: slot as u8, to: (voxmar_engine::inventory::CRAFT + cell) as u8, count: 5 };
                        net.propose(actions.expect("core:transfer"), target);
                    }
                    net.propose(actions.expect("core:craft"), voxmar_engine::action::ActionTarget::Slot(0));
                    gate.advance(now);
                }
                _ if waited > QUEST_WAIT || in_phase > 3.0 * QUEST_WAIT => quit(world, gate, "quests: twine", format!("{waited:.1} server s on, {wheat} wheat came of 15")),
                _ => {}
            }
        }
        2 => {
            let done = world.resource::<LocalPlayer>().quests.iter().any(|q| q.name == DONE_FIRST && q.done);
            if !done {
                if waited > QUEST_WAIT || in_phase > 3.0 * QUEST_WAIT {
                    let state = world.resource::<LocalPlayer>().quests.iter().find(|q| q.name == DONE_FIRST).cloned();
                    quit(world, gate, "quests: twine", format!("fifteen wheat crafted, {waited:.1} server s on the quest stands {state:?} and {} twine are held", holding(world, "base:twine").0));
                }
                return;
            }
            gate.check("quests: twine", Ok(format!("ten twine crafted finished {DONE_FIRST}")));
            *world.resource_mut::<QuestView>() = QuestView::default();
            key(world, key_code, true);
            gate.advance(now);
        }
        3 => {
            key(world, key_code, false);
            let open = world.resource::<UiState>().quests_open;
            let picked = world.resource::<QuestView>().picked.clone();
            if !(open && picked.is_some()) && in_phase < 2.0 {
                return;
            }
            let Some(picked) = picked.filter(|_| open) else {
                return quit(world, gate, "quests: book", if open { "the book opened on no quest".into() } else { "L did not open the quest book".into() });
            };
            let nodes = world.query_filtered::<(), With<Button>>().iter(world).count();
            gate.check("quests: book", Ok(format!("L opened the quest book on {picked} ({nodes} buttons drawn)")));
            gate.advance(now);
        }
        // The book as it stands (what the earlier steps did shows done), in English, then
        // Japanese.
        4 => {
            if in_phase > 0.5 && gate.shot(world, "quests-en").is_some() {
                set_language(world, "ja");
                gate.advance(now);
            }
        }
        5 => {
            if in_phase < 0.5 || gate.shot(world, "quests-ja").is_none() {
                return;
            }
            set_language(world, "en");
            // W held with the book open.
            gate.quest_feet = world.resource::<LocalPlayer>().position;
            key(world, KeyCode::KeyW, true);
            gate.advance(now);
        }
        6 => {
            if in_phase < 1.0 {
                return;
            }
            key(world, KeyCode::KeyW, false);
            let moved = (world.resource::<LocalPlayer>().position - gate.quest_feet).xz().length();
            gate.check("quests: still", if moved < 0.05 { Ok(format!("W held a second with the book open moved the feet {moved:.2} blocks")) } else { Err(format!("W held with the book open walked {moved:.2} blocks")) });
            key(world, KeyCode::Escape, true);
            gate.advance(now);
        }
        7 => {
            key(world, KeyCode::Escape, false);
            let ui = world.resource::<UiState>();
            let (open, locked, menu) = (ui.quests_open, ui.cursor_locked, ui.menu_open);
            if (open || !locked) && in_phase < 2.0 {
                return;
            }
            if open || !locked || menu {
                return quit(world, gate, "quests: Esc", format!("Esc left the book open: {open}, the mouse grabbed: {locked}, the menu open: {menu}"));
            }
            gate.check("quests: Esc", Ok("Esc shut the book and grabbed the mouse again".into()));
            // W again with the book shut walks, or standing still above proved nothing.
            gate.quest_feet = world.resource::<LocalPlayer>().position;
            key(world, KeyCode::KeyW, true);
            gate.advance(now);
        }
        8 => {
            let moved = (world.resource::<LocalPlayer>().position - gate.quest_feet).xz().length();
            if moved <= 1.0 {
                if waited > 2.0 {
                    quit(world, gate, "quests: walks", format!("W held {waited:.1} server s with the book shut moved the feet only {moved:.2} blocks"));
                }
                return;
            }
            key(world, KeyCode::KeyW, false);
            gate.check("quests: walks", Ok(format!("W with the book shut walked {moved:.2} blocks in {in_phase:.2} s ({waited:.1} server s)")));
            // Pinned, the torch quest stands at the right; then torches made for it.
            let content = world.resource::<Launch>().content.clone();
            toggle_pin(&mut world.resource_mut::<crate::settings::Settings>(), &content, TRACKED);
            gate.count_before = tracked(world).0;
            let net = world.resource::<Net>();
            net.command("give charcoal 1");
            net.command("give stick 1");
            gate.advance(now);
        }
        9 => {
            let ready = holding(world, "base:charcoal").0 >= 1 && holding(world, "base:stick").0 >= 1;
            let words = tracker_words(world, gate.count_before);
            let lines = tracker_lines(world);
            let reads = lines.as_ref().is_some_and(|lines| lines.contains(&words));
            if !(ready && reads) {
                if waited > QUEST_WAIT || in_phase > 3.0 * QUEST_WAIT {
                    quit(world, gate, "quests: pinned", format!("{QUEST_WAIT} server s on, charcoal and a stick in hand: {ready}; the tracker reads {lines:?}, not {words:?}"));
                }
                return;
            }
            gate.check("quests: pinned", Ok(format!("pinned, the tracker reads {words:?}")));
            if !lay_and_craft(world, &[(0, "base:charcoal"), (3, "base:stick")]) {
                return quit(world, gate, "quests: torches", "the charcoal or the stick went missing".into());
            }
            gate.quests_rev = world.resource::<LocalPlayer>().quests_rev;
            gate.advance(now);
        }
        10 => {
            let made = gate.count_before + 4;
            let rev = world.resource::<LocalPlayer>().quests_rev;
            let lines = tracker_lines(world).unwrap_or_default();
            // Nothing counts on screen before the server says so.
            if rev == gate.quests_rev && !lines.contains(&tracker_words(world, gate.count_before)) {
                return quit(world, gate, "quests: torches", format!("before the server counted the torches the tracker read {lines:?}"));
            }
            let words = tracker_words(world, made);
            if !(tracked(world).0 == made && lines.contains(&words)) {
                if waited > QUEST_WAIT || in_phase > 3.0 * QUEST_WAIT {
                    quit(world, gate, "quests: torches", format!("4 torches crafted, {waited:.1} server s ({in_phase:.1} s) on the server counts {:?}, {} torches held, and the tracker reads {lines:?}, not {words:?}", tracked(world), holding(world, "base:torch").0));
                }
                return;
            }
            gate.check("quests: torches", Ok(format!("4 torches crafted, the server counted {made} and the tracker reads {words:?}")));
            // A quest done reads so, then goes, and the tracker with it.
            let content = world.resource::<Launch>().content.clone();
            let mut settings = world.resource_mut::<crate::settings::Settings>();
            settings.pinned_quests.clear();
            toggle_pin(&mut settings, &content, DONE_QUEST);
            gate.quest_pinned = now;
            gate.advance(now);
        }
        11 => {
            let done = world.resource::<Locale>().get("quest.tracker.done").to_owned();
            let lines = tracker_lines(world);
            if !lines.as_ref().is_some_and(|lines| lines.contains(&done)) {
                if waited > 2.0 {
                    quit(world, gate, "quests: done", format!("{DONE_QUEST} pinned, done; 2 server s on the tracker reads {lines:?}"));
                }
                return;
            }
            gate.check("quests: done", Ok(format!("{DONE_QUEST} pinned reads {done:?}")));
            gate.advance(now);
        }
        _ => {
            // It stays its while, counted from when it was pinned done, then goes.
            let stays = world.resource::<Launch>().content.quests.tracker().done_stays;
            let since = now - gate.quest_pinned;
            let pinned = !world.resource::<crate::settings::Settings>().pinned_quests.is_empty();
            let lines = tracker_lines(world);
            if lines.is_none() && since < stays - 1.0 {
                return quit(world, gate, "quests: retired", format!("done, it dropped off after only {since:.1} s of {stays} s"));
            }
            if (pinned || lines.is_some()) && since < stays + 3.0 {
                return;
            }
            gate.check(
                "quests: retired",
                if pinned || lines.is_some() { Err(format!("{since:.1} s after it was pinned done, still pinned: {pinned}; the tracker reads {lines:?}")) } else { Ok(format!("done, it stayed and dropped off by {since:.1} s ({stays} s asked), and the tracker went")) },
            );
            world.resource_mut::<crate::settings::Settings>().pinned_quests.clear();
            gate.next(Step::Catalogue, now);
        }
    }
}

/// How many of `item` the player has, held or in the inventory.
fn count_of(world: &World, item: BlockId) -> u32 {
    let inventory = &world.resource::<LocalPlayer>().inventory;
    inventory.slots.iter().chain(std::iter::once(&inventory.cursor)).flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

fn catalogue(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::creative::Catalogue;
    let Some(log) = world.resource::<Launch>().content.blocks.id("base:oak_log") else {
        gate.check("catalogue", Err("no oak log in the content".into()));
        gate.next(Step::Screenshot, now);
        return;
    };
    // The search, as typed: one of every item it finds, or why not.
    let search = |world: &mut World, query: &str| {
        let mut catalogue = world.resource_mut::<Catalogue>();
        catalogue.query = query.to_owned();
        catalogue.refresh();
        catalogue.found.contains(&log)
    };
    match gate.phase {
        0 => {
            gate.was_creative = world.resource::<LocalPlayer>().allow_flight;
            world.resource::<Net>().command("gamemode creative");
            world.resource_mut::<UiState>().inventory_open = true;
            gate.advance(now);
        }
        1 => {
            if !world.resource::<LocalPlayer>().allow_flight && in_phase < 3.0 {
                return;
            }
            let english = search(world, "oak log");
            let japanese = search(world, "げんぼく");
            let katakana = search(world, "オーク");
            gate.check(
                "catalogue: search",
                if english && japanese && katakana { Ok("\"oak log\", \"げんぼく\" and \"オーク\" all find the oak log".into()) } else { Err(format!("finds the oak log: by \"oak log\" {english}, \"げんぼく\" {japanese}, \"オーク\" {katakana}")) },
            );
            gate.counted = count_of(world, log);
            crate::hud::creative::take(world.resource::<Net>(), log, false, true);
            gate.advance(now);
        }
        2 => {
            let gained = count_of(world, log).saturating_sub(gate.counted);
            if gained == 0 && in_phase < 3.0 {
                return;
            }
            gate.check("catalogue: creative", if gained > 0 { Ok(format!("Shift-click gave {gained} oak logs")) } else { Err("a Shift-click gave nothing in creative".into()) });
            world.resource::<Net>().command("gamemode survival");
            gate.advance(now);
        }
        3 => {
            if world.resource::<LocalPlayer>().allow_flight && in_phase < 3.0 {
                return;
            }
            gate.counted = count_of(world, log);
            crate::hud::creative::take(world.resource::<Net>(), log, false, true);
            gate.advance(now);
        }
        _ => {
            // Long enough for a stack to arrive, were it given.
            if in_phase < 1.0 {
                return;
            }
            let gained = count_of(world, log).saturating_sub(gate.counted);
            gate.check("catalogue: survival", if gained == 0 { Ok("a click in survival gives nothing".into()) } else { Err(format!("survival was given {gained} oak logs")) });
            if gate.was_creative {
                world.resource::<Net>().command("gamemode creative");
            }
            world.resource_mut::<Catalogue>().query.clear();
            world.resource_mut::<UiState>().inventory_open = false;
            gate.next(Step::Screenshot, now);
        }
    }
}

/// The screenshot key, as a player presses it: a PNG of the frame lands in the folder.
fn screenshot(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::screenshot::{Elsewhere, Kept, LastShot};
    let key_code = world.resource::<crate::settings::Settings>().screenshot_key.0;
    match gate.phase {
        0 => {
            world.insert_resource(Elsewhere(gate.dir.join("screenshots")));
            world.resource_mut::<LastShot>().0 = None;
            key(world, key_code, true);
            gate.advance(now);
        }
        _ => {
            key(world, key_code, false);
            let kept = world.resource::<LastShot>().0.clone();
            let result = match kept {
                Some(Kept::Saved { path, .. }) if path.is_file() => {
                    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                    Ok(format!("{} saved, {size} bytes", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())))
                }
                Some(Kept::Saved { path, .. }) => Err(format!("said it saved {} but there is no such file", path.display())),
                Some(Kept::Failed(why)) => Err(why),
                None if in_phase < 10.0 => return,
                None => Err("pressing the key saved nothing in 10 s".into()),
            };
            gate.check("screenshot", result);
            world.remove_resource::<Elsewhere>();
            gate.next(Step::Fit, now);
        }
    }
}

/// A screen or state the text-fit tour opens.

#[derive(Clone, Copy, Debug)]
enum View {
    /// The HUD alone, with experience, a refusal and a level-up showing.
    Hud,
    /// The HUD with the debug overlay on.
    Debug,
    Inventory,
    Character,
    /// The character screen with one skill's guide open.
    Guide(usize),
    /// The journal on one page.
    Journal(usize),
    /// The quest book on its first chapter, a quest still waiting picked.
    Quests,
    /// The menu on one settings tab.
    Menu(usize),
    /// Dead, waiting to respawn.
    Dead,
    /// The chat and command line, open.
    Console,
    /// The recipe viewer over the inventory, on one of `FIT_LOOKUPS`.
    Recipes(usize),
    /// The menus before a game: the title screen, the worlds, a new world, joining one,
    /// and the settings on one tab.
    Title,
    Worlds,
    Create,
    Join,
    Settings(usize),
}

/// What the text-fit tour looks up in the recipe viewer, and whether its uses: a crafted
/// item, an ingredient, and a creature's product with a range of amounts.
const FIT_LOOKUPS: [(&str, bool); 3] = [("base:campfire", false), ("base:oak_log", true), ("base:wool", false)];

/// The languages the text-fit tour checks, and how long it leaves each view to lay out.
const FIT_LANGUAGES: [&str; 2] = ["en", "ja"];
const FIT_SETTLE: f32 = 0.4;

/// Opens every view in turn, in each language, and lists the texts that do not fit (see
/// `ui::fit`): one check for the lot, naming each.
fn fit(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let skills = world.resource::<Launch>().content.skills.len();
    let views: Vec<View> = if gate.step == Step::FitJournal {
        (0..voxmar_engine::journal::Page::ALL.len()).map(View::Journal).collect()
    } else if gate.step == Step::FitMenus {
        [View::Title, View::Worlds, View::Create, View::Join].into_iter().chain((0..crate::settings::TABS).map(View::Settings)).collect()
    } else {
        [View::Hud, View::Debug, View::Dead, View::Console, View::Inventory, View::Recipes(0), View::Recipes(1), View::Recipes(2), View::Character]
            .into_iter()
            .chain((0..skills).map(View::Guide))
            .chain([View::Quests])
            .chain((0..crate::settings::TABS).map(View::Menu))
            .collect()
    };
    let menus = gate.step == Step::FitMenus;
    let index = gate.phase as usize;
    let Some((language, view)) = FIT_LANGUAGES.get(index / views.len()).map(|language| (*language, views[index % views.len()])) else {
        let spills: Vec<String> = std::mem::take(&mut gate.spills).into_iter().map(|(_, line)| line).collect();
        // Line breaks are checked on every view in the languages with rules (Japanese), or
        // the pass fails.
        let breaks = world.remove_resource::<crate::ui::fit::LineBreaksChecked>().unwrap_or_default();
        let unbroken = std::mem::take(&mut gate.unbroken);
        let result = if !unbroken.is_empty() {
            Err(format!("no line breaks checked on {} views in a language with line rules: {}", unbroken.len(), unbroken.join("; ")))
        } else if spills.is_empty() {
            Ok(format!("every text fits on {} views in {}; line breaks checked {}", views.len(), FIT_LANGUAGES.join(" and "), breaks.checked))
        } else {
            Err(format!("{} texts do not fit: {}", spills.len(), spills.join("; ")))
        };
        let journal = gate.step == Step::FitJournal;
        gate.check(if menus { "text fits: menus" } else if journal { "text fits: journal" } else { "text fits" }, result);
        show_view(world, None, now);
        set_language(world, "en");
        gate.next(if menus { Step::Done } else if journal { Step::Leave } else { Step::FitJournal }, now);
        return;
    };
    if world.resource::<Locale>().code() != language {
        set_language(world, language);
    }
    show_view(world, Some(view), now);
    if in_phase < FIT_SETTLE {
        return;
    }
    // A journal page is read once it is drawn, in this language, however long that takes.
    if let View::Journal(page) = view
        && world.resource::<crate::hud::journal::JournalDrawn>().0.as_ref() != Some(&(page, language.to_owned()))
    {
        return;
    }
    let checked = |world: &World| world.get_resource::<crate::ui::fit::LineBreaksChecked>().copied().unwrap_or_default();
    let before = checked(world);
    for spill in crate::ui::fit::find(world) {
        let what = format!("{language}: {spill}");
        if !gate.spills.iter().any(|(known, _)| *known == what) {
            gate.spills.push((what, format!("{language}, {view:?}: {spill}")));
        }
    }
    if checked(world).none_since(&before) {
        gate.unbroken.push(format!("{language}, {view:?}"));
    }
    gate.advance(now);
}

/// Opens one view of [`fit`]'s tour, or closes every screen.
fn show_view(world: &mut World, view: Option<View>, now: f32) {
    let screen = match view {
        Some(View::Title) => Some(Screen::Title),
        Some(View::Worlds) => Some(Screen::Worlds),
        Some(View::Create) => Some(Screen::CreateWorld),
        Some(View::Join) => Some(Screen::Join),
        Some(View::Settings(_)) => Some(Screen::Settings),
        _ => None,
    };
    if let Some(screen) = screen
        && *world.resource::<State<Screen>>().get() != screen
    {
        world.resource_mut::<NextState<Screen>>().set(screen);
    }
    world.resource_mut::<LocalPlayer>().dead = matches!(view, Some(View::Dead));
    let debug = matches!(view, Some(View::Debug));
    if world.resource::<crate::settings::Settings>().debug_overlay != debug {
        world.resource_mut::<crate::settings::Settings>().debug_overlay = debug;
    }
    let mut ui = world.resource_mut::<UiState>();
    (ui.menu_open, ui.inventory_open, ui.character_open, ui.console_open, ui.journal_open, ui.quests_open) = (false, false, false, false, false, false);
    match view {
        Some(View::Inventory) => ui.inventory_open = true,
        Some(View::Character | View::Guide(_)) => ui.character_open = true,
        Some(View::Menu(_)) => ui.menu_open = true,
        Some(View::Journal(_)) => ui.journal_open = true,
        Some(View::Quests) => ui.quests_open = true,
        Some(View::Console) => ui.console_open = true,
        Some(View::Recipes(_)) => (ui.inventory_open, ui.recipes_open) = (true, true),
        _ => {}
    }
    if !matches!(view, Some(View::Recipes(_))) {
        ui.recipes_open = false;
    }
    if let Some(View::Quests) = view {
        // A quest still waiting on another, so its page reads so, beside those done and open.
        use crate::hud::quests::{QuestView, Standing, standing};
        let picked = {
            let (launch, player) = (world.resource::<Launch>(), world.resource::<LocalPlayer>());
            let content = &launch.content;
            let first_age = content.quests.iter().next().map(|q| q.age.clone());
            let chapter: Vec<_> = content.quests.iter().filter(|q| Some(&q.age) == first_age.as_ref()).collect();
            chapter.iter().find(|q| standing(content, player, q) == Standing::Waiting).or(chapter.first()).map(|q| q.name.clone())
        };
        *world.resource_mut::<QuestView>() = QuestView { chapter: 0, picked };
    }
    if let Some(View::Journal(page)) = view {
        world.resource_mut::<crate::hud::journal::JournalView>().page = page;
    }
    let lookup = match view {
        Some(View::Recipes(index)) => {
            let (name, uses) = FIT_LOOKUPS[index];
            let blocks = &world.resource::<Launch>().content.blocks;
            blocks.id(name).map(|item| crate::hud::recipes::Lookup { item, uses, page: 0 })
        }
        _ => None,
    };
    world.resource_mut::<crate::hud::recipes::RecipeViewer>().trail = lookup.into_iter().collect();
    let guide = match view {
        Some(View::Guide(index)) => world.resource::<Launch>().content.skills.ids().nth(index),
        _ => None,
    };
    world.resource_mut::<crate::hud::character::Guide>().0 = guide;
    if let Some(View::Menu(tab) | View::Settings(tab)) = view {
        world.resource_mut::<crate::settings::OpenSection>().0 = tab;
    }
    if let Some(View::Hud) = view {
        // The HUD's passing notes, as long as they come: a gain, a refusal, a new level.
        let launch = world.resource::<Launch>().clone();
        let locale = world.resource::<Locale>().clone();
        let skills = &launch.content.skills;
        if let Some(skill) = skills.ids().max_by_key(|&id| locale.skill(skills.get(id)).chars().count()) {
            let name = locale.skill(skills.get(skill));
            let needs = locale.format("skills.needs", &[("skill", &name), ("level", &99)]);
            let mut toasts = world.resource_mut::<crate::hud::skills::SkillToasts>();
            toasts.gained(skill, 12_345, now);
            toasts.refused(needs, now);
            toasts.level_up(skill, 99, now);
        }
    }
}

/// Switches the interface to language `code`, as the setting does, without writing the
/// player's settings.
fn set_language(world: &mut World, code: &str) {
    let content = voxmar_server::assets_dir().join("content");
    world.insert_resource(Locale::load(&content, code));
}

/// What the text written from language key `key` reads now.
fn localized_text(world: &mut World, key: &str) -> Option<String> {
    let mut texts = world.query::<(&Localized, &Text)>();
    texts.iter(world).find(|(localized, _)| localized.0 == key).map(|(_, text)| text.0.clone())
}

/// Follows the weather shown from frame to frame once it is ready. On the first frame it
/// must already be the server's (joining shows the real weather, not a change from the
/// menus' sky into it); after that no fraction may change faster than easing lets it
/// (`weather::Easing::fastest`), so fog, rain and the rest come and go gradually.
fn watch_weather(world: &mut World, gate: &mut Gate, now: f32) {
    let weather = world.resource::<crate::weather::ClientWeather>();
    let shown = weather.current.fractions();
    let values = shown.map(|(_, v)| v);
    match gate.weather_last {
        None if gate.step == Step::Load => {
            let target = weather.target().map(|t| t.fractions());
            let off = target.map_or(f32::INFINITY, |t| t.iter().zip(&shown).map(|(t, s)| (t.1 - s.1).abs()).fold(0.0, f32::max));
            // Exactly: until ready every report is shown at once, and easing leaves a value
            // at its goal where it is.
            let result = if off == 0.0 {
                Ok(format!("ready at {now:.1}s, showing the server's weather at once: {}", weather.current.summary()))
            } else {
                Err(format!("first shown {} away from the server's report: {}", off, weather.current.summary()))
            };
            gate.check("weather: joined", result);
        }
        None => {}
        Some(last) => {
            let dt = world.resource::<Time>().delta_secs();
            for (i, (&(name, value), was)) in shown.iter().zip(last).enumerate() {
                let ratio = (value - was).abs() / (weather.easing().fastest(i) * dt + 1e-4);
                if ratio > gate.weather_jump.0 {
                    gate.weather_jump = (ratio, name, now);
                }
            }
        }
    }
    gate.weather_last = Some(values);
}

/// A storm, then snow, each drawn (rain, snow and their particles change the view), then
/// clear skies again.
fn weather(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let shown = world.resource::<crate::weather::ClientWeather>().current.clone();
    let kinds: [(&str, &'static str, fn(&voxmar_engine::weather::Weather) -> bool); 2] =
        [("storm", "storm", |w| w.precipitation > 0.3 && w.snow < 0.5), ("snow", "snow", |w| w.precipitation > 0.3 && w.snow > 0.5)];
    match gate.phase {
        0 => {
            // A screen left open by a failed step before would cover every view after it,
            // failing all of them: close them all (as Esc would, one by one).
            let mut ui = world.resource_mut::<UiState>();
            (ui.menu_open, ui.inventory_open, ui.character_open, ui.console_open) = (false, false, false, false);
            look(world, 0.0, 5.0);
            if gate.shot(world, "clear").is_some() {
                world.resource::<Net>().command(&format!("weather {} 120", kinds[0].0));
                gate.advance(now);
            }
        }
        // Waiting for each kind to come, then a shot of it.
        phase @ (1 | 2) => {
            let (kind, name, arrived) = kinds[phase as usize - 1];
            if !arrived(&shown) {
                if in_phase > 12.0 {
                    gate.check(&format!("weather: {kind}"), Err(format!("it did not come: {}", shown.summary())));
                    world.resource::<Net>().command("weather clear 600");
                    gate.phase = 3;
                }
                return;
            }
            // Let it fill the air before the shot, and the server tell the player they are
            // out in it (effects are judged once a second).
            let weathered = weather_effects(world);
            if in_phase < 1.0 || (weathered.is_empty() && gate.server_in_phase() < 5.0) {
                return;
            }
            let Some(shot) = gate.shot(world, name) else { return };
            let felt = if weathered.is_empty() {
                Err(format!("out in it, the player has no weather effect (felt {:.0} °C)", world.resource::<LocalPlayer>().vitals.felt))
            } else {
                Ok(format!("out in it, the player is {}", weathered.join(", ")))
            };
            gate.check(&format!("weather: {kind}: effects"), felt);
            let change = gate.earlier_shot("clear").map_or(0.0, |clear| change(&clear, &shot, 0.0));
            let result = if change >= SCREEN_CHANGE {
                Ok(format!("{}, changing the view by {change:.1} levels", shown.summary()))
            } else {
                Err(format!("it barely shows: the view changed by {change:.1} levels"))
            };
            gate.check(&format!("weather: {kind}"), result);
            let next = kinds.get(phase as usize).map_or_else(|| "weather clear 600".to_owned(), |(kind, ..)| format!("weather {kind} 120"));
            world.resource::<Net>().command(&next);
            gate.advance(now);
        }
        3 if shown.precipitation < 0.05 && shown.cover < 0.2 => {
            eased(gate);
            gate.next(Step::Sky, now);
        }
        3 if in_phase > 12.0 => {
            gate.check("weather: clear", Err(format!("the sky did not clear: {}", shown.summary())));
            eased(gate);
            gate.next(Step::Sky, now);
        }
        _ => {}
    }
}

/// Whether every change of weather so far (the commands' storm, snow and clearing among
/// them) eased in without a jump.
fn eased(gate: &mut Gate) {
    let (ratio, name, at) = gate.weather_jump;
    let result = if ratio <= 1.01 {
        Ok(format!("no jumps, at most {:.0}% of the fastest easing allows ({name})", ratio * 100.0))
    } else {
        Err(format!("{name} jumped at {at:.1}s, {ratio:.1} times what easing allows in a frame"))
    };
    gate.check("weather: eases", result);
}

/// A small tree grows on the test ground: a trunk under a crown of leaves with an apple
/// among them. It must reach the client, and its leaves, their cards and the fruit must
/// draw (any pipeline that fails validation fails the gate) and change the view.
fn tree(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let trunk = gate.feet + IVec3::new(-4, 0, -4);
    let apple = trunk + IVec3::new(1, 3, 0);
    match gate.phase {
        0 => {
            world.resource::<Net>().command("time set noon");
            gate.advance(now);
        }
        1 if in_phase > 0.5 => {
            aim(world, trunk + IVec3::Y * 2);
            if gate.shot(world, "tree-before").is_some() {
                let (x, y, z) = (trunk.x, trunk.y, trunk.z);
                for line in [
                    format!("fill {} {} {} {} {} {} oak_leaves", x - 2, y + 3, z - 2, x + 2, y + 5, z + 2),
                    format!("fill {x} {y} {z} {x} {} {z} oak_log", y + 4),
                    format!("fill {} {} {} {} {} {} apple_leaves", apple.x, apple.y, apple.z, apple.x, apple.y, apple.z),
                ] {
                    world.resource::<Net>().command(&line);
                }
                gate.advance(now);
            }
        }
        2 => {
            let grown = world.resource::<ClientWorld>().block(apple).is_some_and(|b| !b.is_air());
            if !(grown && settled(world, trunk) && in_phase > SETTLE) && in_phase < STEP_LIMIT - 1.0 {
                return;
            }
            let Some(shot) = gate.shot(world, "tree") else { return };
            let change = gate.earlier_shot("tree-before").map_or(0.0, |before| change(&before, &shot, 0.25));
            let result = match (grown, change >= SCREEN_CHANGE) {
                (true, true) => Ok(format!("a tree with an apple drawn, changing the view by {change:.1} levels")),
                (false, _) => Err("the tree's fruit never reached the client".into()),
                (true, false) => Err(format!("the tree barely shows: the view changed by {change:.1} levels")),
            };
            gate.check("tree", result);
            gate.next(Step::Creatures, now);
        }
        _ => {}
    }
}

/// The species the gate summons and strikes: slow and fearless, so it stays to be struck.
const CREATURE: &str = "base:lumen_snail";
/// What it always drops.
const CREATURE_DROP: &str = "base:lumen_slime";

/// A lumen snail and a few grazers are summoned on open test ground: they must reach the
/// client and draw, and the snail, struck with left clicks as a player would, must die
/// and drop its slime, which walking over it picks up.
fn creatures(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let species = world.resource::<Launch>().content.creatures.find(CREATURE);
    let Some(species) = species else {
        gate.check("creatures", Err(format!("no `{CREATURE}` in the content")));
        gate.next(Step::Ore, now);
        return;
    };
    let snail = |world: &World| {
        let creatures = world.resource::<crate::creatures::Creatures>();
        creatures.views().find(|v| v.species == species).copied()
    };
    match gate.phase {
        0 => {
            // A clear pad of its own beside the test ground: stone underfoot, air above, so
            // nothing the earlier steps built stands between the player and the creatures.
            let pad = gate.feet + IVec3::new(0, 0, 14);
            let (x, y, z) = (pad.x, pad.y, pad.z);
            let net = world.resource::<Net>();
            // Walled two high, so nothing wanders off the edge.
            net.command(&format!("fill {} {} {} {} {} {} stone", x - 6, y - 1, z - 6, x + 6, y + 1, z + 6));
            net.command(&format!("fill {} {y} {} {} {} {} air", x - 5, z - 5, x + 5, y + 3, z + 5));
            net.command(&format!("tp {} {y} {}", x as f32 + 0.5, z as f32 + 4.5));
            net.command("time set noon");
            gate.advance(now);
        }
        1 if in_phase > 1.0 => {
            // Facing the pad's middle, which is where /summon puts them (four blocks ahead).
            look(world, 0.0, -25.0);
            gate.count_before = item_count(world, CREATURE_DROP);
            let net = world.resource::<Net>();
            let pad = gate.feet + IVec3::new(0, 0, 14);
            net.command(&format!("summon {CREATURE} 1 {} {} {}", pad.x, pad.y, pad.z + 1));
            net.command(&format!("summon mouflon 2 {} {} {}", pad.x - 3, pad.y, pad.z - 4));
            net.command(&format!("summon rabbit 1 {} {} {}", pad.x + 3, pad.y, pad.z - 4));
            gate.advance(now);
        }
        2 => {
            let Some(view) = snail(world) else {
                if in_phase > 10.0 {
                    gate.check("creatures", Err("a summoned creature never reached the client".into()));
                    gate.next(Step::Ore, now);
                }
                return;
            };
            aim_at(world, view.position + Vec3::Y * 0.2);
            if in_phase < 1.0 {
                return;
            }
            if gate.shot(world, "creatures").is_some() {
                let shown = world.resource::<crate::creatures::Creatures>().alive();
                gate.check("creatures shown", Ok(format!("{shown} creatures drawn on the test ground")));
                gate.check("creature voices", crate::creature_sounds::loaded(world));
                // The one pointed at shows its level and health over it.
                let shown = world.query_filtered::<&Visibility, With<crate::hud::levels::LevelLabel>>().iter(world).any(|v| *v != Visibility::Hidden);
                gate.check("creature level", if shown { Ok("the creature pointed at shows its level and health over it".into()) } else { Err("the creature pointed at shows no level over it".into()) });
                gate.next(Step::Strike, now);
            }
        }
        _ => {}
    }
}

/// The lumen snail summoned in [`creatures`], struck until it dies; its slime must drop
/// and be picked up.
fn strike(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let Some(species) = world.resource::<Launch>().content.creatures.find(CREATURE) else { return };
    let snail = |world: &World| {
        let creatures = world.resource::<crate::creatures::Creatures>();
        creatures.views().find(|v| v.species == species).copied()
    };
    match gate.phase {
        0 => {
            // Strike it as a player would, a click every few frames, until it dies.
            // Judged by the blows the server answers, not the clock: it must keep taking
            // them, and the snail must die within a few dozen.
            let (blows, landed) = {
                let creatures = world.resource::<crate::creatures::Creatures>();
                (creatures.blows, creatures.landed)
            };
            // Once a blow lands, the readout names it, with its level and health.
            if landed > 0 && !gate.readout_seen {
                let readout = world.resource::<crate::hud::target::Readout>();
                if let Some(health) = readout.health.filter(|_| !readout.name.is_empty() && !readout.level.is_empty()) {
                    let detail = format!("{} {}, {:.0}% health", readout.name, readout.level, health * 100.0);
                    gate.readout_seen = true;
                    gate.check("target readout", Ok(detail));
                }
            }
            if blows > gate.blows_seen {
                gate.blows_seen = blows;
                gate.phase_since = now;
                gate.phase_server = gate.server_now;
            }
            let stalled = gate.server_in_phase() > UNANSWERED;
            match snail(world) {
                Some(view) if !stalled && blows < MOST_BLOWS => {
                    gate.quarry = view.position;
                    if landed > 0 {
                        gate.struck_heights = (gate.struck_heights.0.min(view.position.y), gate.struck_heights.1.max(view.position.y));
                    }
                    // Close in to striking reach, then strike.
                    let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 2.2;
                    key(world, KeyCode::KeyW, far);
                    aim_at(world, view.position + Vec3::Y * 0.2);
                    // A well-timed click, a full charge apart, as a player swings (spamming
                    // lands lighter blows: `combat`).
                    let swing = ((now - gate.since) / (voxmar_engine::combat::FULL_CHARGE + 0.05)).fract() < 0.1;
                    mouse(world, MouseButton::Left, swing);
                }
                Some(view) => {
                    mouse(world, MouseButton::Left, false);
                    key(world, KeyCode::KeyW, false);
                    let aimed = world.resource::<crate::interact::Target>().creature.is_some();
                    let distance = view.position.distance(world.resource::<LocalPlayer>().eye());
                    let why = if stalled { format!("the server answered no blow for {UNANSWERED}s") } else { format!("{blows} blows ({landed} landed) did not kill it") };
                    gate.check("creature struck", Err(format!("{why} (aimed at it: {aimed}, {distance:.1} blocks from the eye)")));
                    gate.next(Step::Ore, now);
                }
                None => {
                    mouse(world, MouseButton::Left, false);
                    let deaths = world.resource::<crate::creatures::Creatures>().deaths;
                    let (blows, landed) = {
                        let creatures = world.resource::<crate::creatures::Creatures>();
                        (creatures.blows, creatures.landed)
                    };
                    let result = if deaths > 0 { Ok(format!("killed with {landed} of {blows} blows in {:.1}s", now - gate.since)) } else { Err("it went without dying".into()) };
                    gate.check("creature struck", result);
                    // Blows land without knockback: the snail never left the pad's floor.
                    let (low, high) = gate.struck_heights;
                    let rise = (high - low).max(0.0);
                    let result = if rise < 0.2 { Ok(format!("struck, it rose at most {rise:.2} blocks")) } else { Err(format!("struck, it was thrown {rise:.2} blocks up")) };
                    gate.check("creature struck: no knockback", result);
                    // Its cries reached the sound system (a silent run plays none).
                    let heard = &world.resource::<crate::creature_sounds::Heard>().asked;
                    let cries = [crate::creature_sounds::Kind::Hurt, crate::creature_sounds::Kind::Death].map(|k| heard.get(&k).copied().unwrap_or(0));
                    let result = if cries.iter().all(|&n| n > 0) { Ok(format!("{} hurt and {} death sounds asked for", cries[0], cries[1])) } else { Err(format!("hurt and death sounds asked for: {cries:?}")) };
                    gate.check("creature cries", result);
                    if !gate.readout_seen {
                        gate.check("target readout", Err("it never showed the struck creature's name, level and health".into()));
                    }
                    gate.advance(now);
                }
            }
        }
        1 => {
            // How its body fell, once it has: over onto its side close to where it stood,
            // and never drawn around the eye (from close up, its inside showed).
            if !gate.fall_judged {
                let fall = world.resource::<crate::creatures::Creatures>().falls.last().copied();
                if let Some(fall) = fall {
                    gate.fall_judged = true;
                    // Judged by what was drawn: every joint of the body, every frame of its fall.
                    // Watched over several frames, not one: three samples of every joint.
                    let result = if fall.samples < 3 * fall.joints.max(1) {
                        Err(format!("its fall was hardly seen: {} joint samples for {} joints", fall.samples, fall.joints))
                    } else if fall.spread > 1.5 {
                        Err(format!("a joint of its body was drawn {:.2} of its heights from where it died", fall.spread))
                    } else if fall.scale > 0.05 {
                        Err(format!("a joint of its body was drawn {:.0}% off its standing size", fall.scale * 100.0))
                    } else {
                        let eye = if fall.around_eye { "the eye was inside it, and it was hidden" } else { "away from the eye" };
                        Ok(format!("drawn within {:.2} of its heights, joints at most {:.1}% off size; {eye}", fall.spread, fall.scale * 100.0))
                    };
                    gate.check("creature falls", result);
                } else if in_phase > 4.0 {
                    gate.fall_judged = true;
                    gate.check("creature falls", Err("its body never finished falling".into()));
                }
            }
            // Walk over where it died, as a player picks up loot.
            let feet = world.resource::<LocalPlayer>().position;
            let close = feet.xz().distance(gate.quarry.xz()) < 0.4;
            key(world, KeyCode::KeyW, !close);
            if !close {
                aim_at(world, gate.quarry + Vec3::Y * 1.6);
            }
            let count = item_count(world, CREATURE_DROP);
            if count > gate.count_before || in_phase > 8.0 {
                key(world, KeyCode::KeyW, false);
                let result = if count > gate.count_before {
                    Ok(format!("picked up {} {CREATURE_DROP}", count - gate.count_before))
                } else {
                    Err(format!("no {CREATURE_DROP} picked up after the kill"))
                };
                gate.check("creature drops", result);
                gate.next(Step::Weapon, now);
            }
        }
        _ => {}
    }
}

/// The weapon the gate strikes with, and the level that wields it.
const WEAPON: &str = "base:iron_sword";
const WEAPON_LEVEL: u8 = 20;
/// What it strikes: big enough to take several blows, in creative so its own pass by.
const BEAR: &str = "base:brown_bear";

/// An iron sword at Attack 20 against a brown bear on the pad: struck until it dies, some
/// blow must do more than a bare fist's most at that level (1 + 20 / 8 = 3).
fn weapon(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let Some(species) = world.resource::<Launch>().content.creatures.find(BEAR) else {
        gate.check("weapon", Err(format!("no `{BEAR}` in the content")));
        gate.next(Step::Cook, now);
        return;
    };
    let bear = |world: &World| world.resource::<crate::creatures::Creatures>().views().find(|v| v.species == species).copied();
    let fist = voxmar_engine::combat::Weapon::FISTS.max_hit(WEAPON_LEVEL);
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("gamemode creative");
            net.command(&format!("skill attack {WEAPON_LEVEL}"));
            net.command(&format!("give {} 1", WEAPON.trim_start_matches("base:")));
            let pad = gate.feet + IVec3::new(0, 0, 14);
            net.command(&format!("summon {} 1 {} {} {}", BEAR.trim_start_matches("base:"), pad.x, pad.y, pad.z + 1));
            gate.blows_seen = world.resource::<crate::creatures::Creatures>().blows;
            gate.advance(now);
        }
        1 => {
            // The sword in hand, at the level that wields it, and the bear in sight.
            let ready = skill_level(world, "core:attack").0 >= WEAPON_LEVEL && bear(world).is_some();
            if ready && select(world, WEAPON) {
                gate.advance(now);
            } else if in_phase > 10.0 {
                gate.check("weapon", Err(format!("the sword, Attack {WEAPON_LEVEL} or the bear never came (bear seen: {})", bear(world).is_some())));
                end_weapon(world, gate, now);
            }
        }
        _ => {
            let (blows, hardest) = {
                let creatures = world.resource::<crate::creatures::Creatures>();
                (creatures.blows, creatures.hardest)
            };
            if blows > gate.blows_seen {
                gate.blows_seen = blows;
                gate.phase_since = now;
                gate.phase_server = gate.server_now;
            }
            let stalled = gate.server_in_phase() > UNANSWERED;
            match bear(world) {
                Some(view) if !stalled => {
                    // Close in and strike a full charge apart, as a player swings.
                    let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 2.4;
                    key(world, KeyCode::KeyW, far);
                    aim_at(world, view.position + Vec3::Y * 0.6);
                    let swing = ((now - gate.since) / (voxmar_engine::combat::FULL_CHARGE + 0.05)).fract() < 0.1;
                    mouse(world, MouseButton::Left, swing);
                }
                Some(_) => {
                    gate.check("weapon strikes", Err(format!("the server answered no blow for {UNANSWERED}s")));
                    end_weapon(world, gate, now);
                }
                None => {
                    let result = if hardest > fist {
                        Ok(format!("the bear died; the hardest blow did {hardest}, a fist's most is {fist}"))
                    } else {
                        Err(format!("no blow did more than a fist's {fist} (the hardest did {hardest})"))
                    };
                    gate.check("weapon strikes", result);
                    end_weapon(world, gate, now);
                }
            }
        }
    }
}

/// Back to survival with the hands free, for the steps after.
fn end_weapon(world: &mut World, gate: &mut Gate, now: f32) {
    mouse(world, MouseButton::Left, false);
    key(world, KeyCode::KeyW, false);
    let net = world.resource::<Net>();
    net.command("gamemode survival");
    net.command("skill attack 1");
    gate.next(Step::Cook, now);
}

/// A campfire on the pad, raw rabbit in hand: a right click on the fire cooks one, which
/// comes back as cooked rabbit or, at a low Cooking level, sometimes burnt.
fn cook(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let fire = gate.feet + IVec3::new(0, 0, 14);
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command(&format!("setblock {} {} {} campfire", fire.x, fire.y, fire.z));
            net.command("give raw_rabbit 3");
            gate.advance(now);
        }
        1 => {
            let client = world.resource::<ClientWorld>();
            let (Some(raw), Some(campfire)) = (client.registry.id("base:raw_rabbit"), client.registry.id("base:campfire")) else {
                gate.check("cook", Err("no raw rabbit or campfire in the content".into()));
                gate.next(Step::Ore, now);
                return;
            };
            let lit = client.block(fire) == Some(campfire);
            // Earlier steps fill the hotbar with what they pick up, more of it on some runs
            // than others: the rabbit then lands in the pack, and is moved down, once.
            let slot = in_hotbar(world, &mut gate.food_moved, raw);
            match (lit, slot) {
                (true, Some(slot)) => {
                    world.resource_mut::<LocalPlayer>().selected = slot;
                    gate.count_before = item_count(world, "base:cooked_rabbit") + item_count(world, "base:burnt_food");
                    gate.advance(now);
                }
                _ if in_phase > 10.0 => {
                    gate.check("cook", Err(format!("the campfire ({lit}) or raw rabbit in the hotbar ({slot:?}) never came")));
                    gate.next(Step::Ore, now);
                }
                _ => {}
            }
        }
        2 => {
            aim_at(world, fire.as_vec3() + Vec3::new(0.5, 0.3, 0.5));
            let cooked = item_count(world, "base:cooked_rabbit") + item_count(world, "base:burnt_food");
            if cooked > gate.count_before {
                mouse(world, MouseButton::Right, false);
                gate.check("cook", Ok(format!("{} came back from the fire", cooked - gate.count_before)));
                gate.advance(now);
            } else if in_phase > 15.0 {
                mouse(world, MouseButton::Right, false);
                gate.check("cook", Err("right clicks on the campfire cooked nothing".into()));
                gate.next(Step::Ore, now);
            } else {
                mouse(world, MouseButton::Right, ((now - gate.since) * 2.0).fract() < 0.2);
            }
        }
        // The fire's particles, counted for a while: smoke, embers and sparks rising from
        // it. Counting is the same at any frame rate; a picture of the air over it, taken
        // twice, is kept for the record (a few faint puffs sit near any threshold).
        3 => {
            aim_at(world, fire.as_vec3() + Vec3::new(0.5, 1.6, 0.5));
            if in_phase > 0.5 && gate.shot(world, "fire").is_some() {
                world.resource_mut::<crate::emitters::Emitters>().watch(fire);
                gate.advance(now);
            }
        }
        4 if in_phase > FIRE_WATCH => {
            let Some(later) = gate.shot(world, "fire_later") else { return };
            let change = gate.earlier_shot("fire").map_or(0.0, |earlier| change(&earlier, &later, 0.4));
            let amount = world.resource::<crate::render::particles::Particles>().amount;
            let is_fire = {
                let client = world.resource::<ClientWorld>();
                client.registry.id("base:campfire").is_some_and(|c| client.block(fire) == Some(c))
            };
            let mut emitters = world.resource_mut::<crate::emitters::Emitters>();
            let count = emitters.watched;
            let why = emitters.why_quiet(fire, amount);
            emitters.watch = None;
            let result = if count >= FIRE_PARTICLES {
                Ok(format!("{count} particles rose from it in {FIRE_WATCH} s (the view over it changed by {change:.2} levels)"))
            } else {
                Err(format!("only {count} particles rose from it in {FIRE_WATCH} s, under {FIRE_PARTICLES} (still a campfire: {is_fire}; {why})"))
            };
            gate.check("campfire smoke", result);
            gate.next(Step::Tame, now);
        }
        _ => {}
    }
}

/// Seconds the gate watches a campfire's particles for.
const FIRE_WATCH: f32 = 1.5;

/// Particles a campfire must give off in `FIRE_WATCH` seconds: its smoke, embers and
/// sparks come at about ten a second, about fifteen in that time at any frame rate, so
/// fewer than this means the fire gives off (almost) nothing.
const FIRE_PARTICLES: u32 = 4;

/// A junglefowl on the pad, wheat in hand: right clicks feed it until it trusts the
/// player and is theirs (kept, with them as its owner).
fn tame(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let pad = gate.feet + IVec3::new(0, 0, 14);
    let Some(species) = world.resource::<Launch>().content.creatures.find("base:junglefowl") else { return };
    // The junglefowl summoned on the pad: the newest one there. A wild one that wandered
    // near (they spawn about the test ground too) is often older, outside the pad's walls,
    // and was the one aimed at when this picked the oldest: "nearest 20 blocks".
    let centre = (pad.as_vec3() + Vec3::new(-1.5, 0.0, 0.5)).xz();
    let hen = |world: &World| {
        let creatures = world.resource::<crate::creatures::Creatures>();
        creatures.views().filter(|v| v.species == species && v.position.xz().distance(centre) < 7.0).max_by_key(|v| v.id).copied()
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command(&format!("summon junglefowl 1 {} {} {}", pad.x - 2, pad.y, pad.z));
            net.command("give wheat 16");
            gate.advance(now);
        }
        1 => {
            let wheat = world.resource::<ClientWorld>().registry.id("base:wheat");
            let slot = {
                let player = world.resource::<LocalPlayer>();
                (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| player.inventory.get(s).is_some_and(|stack| Some(stack.item) == wheat))
            };
            // Earlier steps can fill the hotbar (planks, slime, cooked or burnt food), so the
            // wheat may land in the pack: swap it into the last hotbar slot, once.
            let packed = {
                let player = world.resource::<LocalPlayer>();
                (voxmar_engine::inventory::HOTBAR_SLOTS..voxmar_engine::inventory::SLOTS).find(|&s| player.inventory.get(s).is_some_and(|stack| Some(stack.item) == wheat))
            };
            if slot.is_none()
                && !gate.food_moved
                && let Some(from) = packed
            {
                let actions = world.resource::<crate::Actions>().0.clone();
                let to = voxmar_engine::inventory::HOTBAR_SLOTS - 1;
                let target = voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: to as u8, count: u8::MAX };
                world.resource::<Net>().propose(actions.expect("core:transfer"), target);
                gate.food_moved = true;
            }
            match (hen(world), slot) {
                (Some(_), Some(slot)) => {
                    world.resource_mut::<LocalPlayer>().selected = slot;
                    gate.advance(now);
                }
                _ if in_phase > 10.0 => {
                    gate.check("tame", Err("the junglefowl or the wheat never came".into()));
                    gate.next(Step::Ore, now);
                }
                _ => {}
            }
        }
        2 => {
            let Some(view) = hen(world) else {
                gate.check("tame", Err("the junglefowl went".into()));
                gate.next(Step::Ore, now);
                return;
            };
            let me = world.resource::<crate::creatures::Creatures>().me;
            if view.kept && view.owner == me {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::KeyW, false);
                gate.check("tame", Ok(format!("a junglefowl is ours after {in_phase:.1}s of feeding")));
                gate.advance(now);
            } else if in_phase > 25.0 {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::KeyW, false);
                let (wheat, trust) = (item_count(world, "base:wheat"), view.trust);
                let [on, other, none] = gate.tame_aim;
                gate.check("tame", Err(format!(
                    "25 seconds of feeding wheat did not tame it: trust {trust}%, {wheat} wheat left; the crosshair was on it {on} frames, on another creature {other}, on none {none}; nearest {:.1} blocks",
                    gate.tame_nearest
                )));
                gate.next(Step::Ore, now);
            } else {
                let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 1.8;
                key(world, KeyCode::KeyW, far);
                aim_at(world, view.position + Vec3::Y * 0.3);
                // Only with the junglefowl itself under the crosshair: a mouflon wandering in
                // front would be offered the wheat instead (they love it too).
                let under = world.resource::<crate::interact::Target>().creature;
                let aimed = under == Some(view.id);
                gate.tame_aim[if aimed { 0 } else if under.is_some() { 1 } else { 2 }] += 1;
                gate.tame_nearest = gate.tame_nearest.min(world.resource::<LocalPlayer>().eye().distance(view.position));
                mouse(world, MouseButton::Right, aimed && ((now - gate.since) * 2.0).fract() < 0.2);
            }
        }
        3 => {
            // Looking at our new junglefowl, the target readout says it is kept.
            if let Some(view) = hen(world) {
                aim_at(world, view.position + Vec3::Y * 0.3);
            }
            let kept = world.resource::<crate::lang::Locale>().get("target.kept").to_owned();
            let readout = world.resource::<crate::hud::target::Readout>();
            let tags: Vec<&str> = readout.tags.iter().map(|(words, _)| words.as_str()).collect();
            if tags.contains(&kept.as_str()) {
                let detail = format!("{} {}: {}", readout.name, readout.level, tags.join(", "));
                gate.check("readout: kept", Ok(detail));
                gate.next(Step::Ore, now);
            } else if in_phase > 3.0 {
                let detail = format!("aimed at the tamed junglefowl, it read {:?} {:?} with tags {tags:?}", readout.name, readout.level);
                gate.check("readout: kept", Err(detail));
                gate.next(Step::Ore, now);
            }
        }
        _ => {}
    }
}

/// Turns the player to look at a point.
fn aim_at(world: &mut World, at: Vec3) {
    let mut player = world.resource_mut::<LocalPlayer>();
    let to = at - player.eye();
    player.yaw = (-to.x).atan2(-to.z);
    player.pitch = to.y.atan2(to.xz().length());
}

/// How many of an item the inventory holds.
fn item_count(world: &World, name: &str) -> u32 {
    let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
    let Some(item) = client.registry.id(name) else { return 0 };
    (0..voxmar_engine::inventory::SLOTS).filter_map(|slot| player.inventory.get(slot)).filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

/// The sun low in the morning sky, a bright disc in a sky that darkens upwards; at night
/// the moon, opposite the sun, and stars around it.
fn sky(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    // Morning (ticks after sunrise), sunset with the sun a few degrees up, and midnight.
    const MORNING: u64 = 1920;
    const SUNSET: u64 = 15_830;
    const MIDNIGHT: u64 = 20_000;
    let ticks = world.resource::<crate::render::DayCycle>().ticks.rem_euclid(voxmar_protocol::TICKS_PER_DAY as f64);
    let at = |time: u64| (ticks - time as f64).abs() < 100.0;
    // The sky's checks judge the sun, moon and stars against the sky around them, so they
    // wait for the clear weather the weather step left (cloud cover 0) to have cleared every
    // cloud, whatever the seed's weather was. (No second command: another line on the
    // console would cover the moon.)
    let cloudless = world.resource::<crate::weather::ClientWeather>().current.cover <= 0.01;
    match gate.phase {
        0 => {
            world.resource::<Net>().command(&format!("time set {MORNING}"));
            gate.advance(now);
        }
        1 if at(MORNING) && cloudless && in_phase > 0.5 => {
            aim_at_light(world);
            gate.advance(now);
        }
        2 if in_phase > 0.3 => {
            let Some(shot) = gate.shot(world, "sun") else { return };
            let at = light_on_screen(world);
            gate.check("sky: sun", disc(&shot, at, "sun"));
            gate.check("sky: day", gradient(&shot));
            // Up above the top of the world, as a flying player can go: by teleport, for the
            // server refuses a move that jumps hundreds of blocks in one step.
            let feet = gate.feet;
            let mut player = world.resource_mut::<LocalPlayer>();
            player.flying = true;
            player.pitch = 0.2;
            world.resource::<Net>().command(&format!("tp {} {ABOVE_THE_WORLD} {}", feet.x as f32 + 0.5, feet.z as f32 + 0.5));
            gate.advance(now);
        }
        3 if in_phase > 0.5 && world.resource::<LocalPlayer>().position.y > ABOVE_THE_WORLD - 1.0 => {
            let Some(shot) = gate.shot(world, "above the world") else { return };
            gate.check("sky: above the world", sky_lit(&shot));
            let feet = gate.feet;
            world.resource_mut::<LocalPlayer>().flying = false;
            let net = world.resource::<Net>();
            net.command(&format!("tp {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5));
            net.command(&format!("time set {SUNSET}"));
            gate.advance(now);
        }
        // At sunset, the sky above the horizon towards the sun, then away from it.
        4 if at(SUNSET) && in_phase > 0.5 => {
            aim_at_light(world);
            world.resource_mut::<LocalPlayer>().pitch = 0.15;
            gate.advance(now);
        }
        5 if in_phase > 0.3 => {
            let Some(_) = gate.shot(world, "sunset towards") else { return };
            world.resource_mut::<LocalPlayer>().yaw += std::f32::consts::PI;
            gate.advance(now);
        }
        6 if in_phase > 0.3 => {
            let Some(away) = gate.shot(world, "sunset away") else { return };
            let towards = gate.earlier_shot("sunset towards");
            let result = towards.ok_or_else(|| "no shot towards the sunset".to_owned()).and_then(|towards| sunset(&towards, &away));
            gate.check("sky: sunset", result);
            world.resource::<Net>().command(&format!("time set {MIDNIGHT}"));
            gate.advance(now);
        }
        7 if at(MIDNIGHT) && in_phase > 0.5 => {
            aim_at_light(world);
            gate.advance(now);
        }
        8 if in_phase > 0.3 => {
            let Some(shot) = gate.shot(world, "moon") else { return };
            let at = light_on_screen(world);
            gate.check("sky: moon", disc(&shot, at, "moon"));
            // Away from the moon, high up.
            let mut player = world.resource_mut::<LocalPlayer>();
            player.yaw += 1.6;
            player.pitch = 1.0;
            gate.advance(now);
        }
        9 if in_phase > 0.3 => {
            let Some(shot) = gate.shot(world, "stars") else { return };
            gate.check("sky: stars", stars(&shot));
            // Down at the land around, lit by the night sky alone.
            world.resource_mut::<LocalPlayer>().pitch = -0.6;
            gate.advance(now);
        }
        10 if in_phase > 0.3 => {
            let Some(shot) = gate.shot(world, "night ground") else { return };
            gate.check("sky: night ground", night_ground(&shot));
            gate.next(Step::Tree, now);
        }
        1 if in_phase > 15.0 => {
            let why = if cloudless { format!("the time of day did not change: {ticks:.0} ticks") } else { "the sky did not clear of cloud".to_owned() };
            gate.check("sky", Err(why));
            gate.next(Step::Tree, now);
        }
        4 | 7 if in_phase > 5.0 => {
            gate.check("sky", Err(format!("the time of day did not change: {ticks:.0} ticks")));
            gate.next(Step::Tree, now);
        }
        _ => {}
    }
}

/// Pipelines only the gate's own work compiles (its shots), which `pipelines` allows.
const GATE_PIPELINES: [&str; 1] = ["screenshot-to-screen"];

/// How far `/locate gorge` looks from the test ground, in blocks.
const GORGE_RADIUS: i32 = 3000;
/// How high a gorge's walls must stand over its water on both sides, within a few blocks
/// of it.
const GORGE_WALLS: i32 = 6;

/// `/locate gorge` (asked by the observer, which reads the answer) names the nearest river
/// running deep in a slot gorge; the player goes there by day, and the server's world has
/// water there with ground standing at least `GORGE_WALLS` over it on both sides, 3 to 8
/// blocks off across it, straight or diagonally.
/// How long the world around the gorge, a fresh area far away, is given to load: it
/// takes 2 to 5 s, so a minute on a machine with cores to spare is plenty.
const GORGE_LOAD_LIMIT: f32 = 60.0;
/// The same on a machine asked for more than all its cores (other threads' builds), where
/// generating a fresh area can take ten times as long or more. The step waits rather than
/// being skipped: a skipped check would let a landing in untested.
const GORGE_OVERLOADED_LIMIT: f32 = 240.0;

/// How far `/locate settlement` looks from the gorge, in blocks: about one 1024-block
/// square in six holds one.
const SETTLEMENT_RADIUS: i32 = 3000;
/// How far around its middle the step needs the world loaded and judges buildings.
const SETTLEMENT_REACH: i32 = 40;

/// `/locate settlement` names the nearest hamlet or village; the player and the observer go
/// there, and once the server's world around it has loaded, its blocks are judged against
/// the plan worldgen gives for it (`Overworld::settlements_near`): its well stands on the
/// square, every door within reach is open with a floor under it and space behind it, and
/// no house holds water. Then the player goes back to the test ground.
fn settlements(world: &mut World, gate: &mut Gate, now: f32, _in_phase: f32) {
    match gate.phase {
        0 => {
            if let Some(observer) = gate.observer.as_mut() {
                observer.replies.clear();
                observer.connection.send(ClientMessage::Command { line: format!("locate settlement {SETTLEMENT_RADIUS}") });
            }
            gate.advance(now);
        }
        1 => {
            let content = world.resource::<Launch>().content.clone();
            let english = |words| content.english.render(words, &content.blocks);
            let Some(reply) = gate.observer.as_ref().and_then(|o| o.replies.iter().map(english).find(|r| r.contains("settlement") || r.contains("hamlet or village"))) else {
                if gate.server_in_phase() > 20.0 {
                    gate.check("settlements", Err("no answer to /locate settlement".into()));
                    gate.next(Step::Recipes, now);
                }
                return;
            };
            let at = reply.split("/tp ").nth(1).and_then(|rest| {
                let n: Vec<i32> = rest.split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
                (n.len() == 3).then(|| IVec3::new(n[0], n[1], n[2]))
            });
            let Some(at) = at else {
                gate.check("settlements", Err(format!("/locate settlement: {reply}")));
                gate.next(Step::Recipes, now);
                return;
            };
            gate.settlement = at;
            world.resource::<Net>().command(&format!("tp {} {} {}", at.x as f32 + 0.5, at.y, at.z as f32 + 0.5));
            if let Some(observer) = gate.observer.as_mut() {
                observer.connection.send(ClientMessage::Command { line: format!("tp {} {} {}", at.x, at.y + 60, at.z) });
            }
            gate.advance(now);
        }
        2 => {
            let at = gate.settlement;
            let (low, high) = (at - IVec3::splat(SETTLEMENT_REACH), at + IVec3::splat(SETTLEMENT_REACH));
            let (low, high) = (ColumnPos::of_block(low).0, ColumnPos::of_block(high).0);
            let columns: Vec<ColumnPos> = (low.x..=high.x).flat_map(|x| (low.y..=high.y).map(move |z| ColumnPos::new(x, z))).collect();
            let held = gate.observer.as_ref().map_or(0, |o| columns.iter().filter(|&&c| o.world.contains(c)).count());
            if held < columns.len() {
                // Waits on the server's clock: generating a fresh area takes the cores.
                if gate.server_in_phase() > 120.0 {
                    let n = columns.len();
                    gate.check("settlements", Err(format!("the world around the settlement at {at} did not load: the server sent {held} of its {n} columns in 120 server seconds")));
                    gate.next(Step::Recipes, now);
                }
                return;
            }
            let result = judge_settlement(world, gate, at);
            gate.check("settlements", result);
            let feet = gate.feet;
            world.resource::<Net>().command(&format!("tp {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5));
            gate.advance(now);
        }
        // Back on the test ground.
        _ => {
            let home = world.resource::<LocalPlayer>().position.xz().distance(gate.feet.xz().as_vec2() + 0.5) < 2.0;
            if home || gate.server_in_phase() > 20.0 {
                if !home {
                    gate.check("settlements: back", Err("the player never came back to the test ground".into()));
                }
                gate.next(Step::Recipes, now);
            }
        }
    }
}

/// The settlement whose middle is `at`, as worldgen plans it, against the server's blocks.
fn judge_settlement(world: &World, gate: &Gate, at: IVec3) -> Result<String, String> {
    let Some(generator) = world.resource::<crate::render::lod::WorldGenerator>().get().cloned() else {
        return Err("no world generator on the client to read the settlement's plan from".into());
    };
    let Some(observer) = gate.observer.as_ref() else { return Err("no observer".into()) };
    let client = world.resource::<ClientWorld>();
    let plans = generator.settlements_near(at.xz(), 64);
    let Some(plan) = plans.iter().find(|s| s.centre.xz() == at.xz()) else {
        return Err(format!("worldgen plans no settlement with its middle at {at} ({} near)", plans.len()));
    };
    let def = |p: IVec3| observer.world.block(p).map(|b| client.registry.def(b));
    let solid = |p: IVec3| def(p).is_some_and(|d| d.solid);
    let wet = |p: IVec3| observer.world.block(p).is_some_and(|b| client.registry.fluid(b).is_some());
    let within = |p: IVec2| (p - at.xz()).abs().max_element() <= SETTLEMENT_REACH;
    let mut problems = Vec::new();
    // The well: something built on its ground above its floor.
    let well = plan.buildings.iter().find(|b| b.kind.name() == "well");
    let well_built = well.is_some_and(|b| {
        (b.min.x..=b.max.x).any(|x| (b.min.y..=b.max.y).any(|z| (b.floor..b.floor + 3).any(|y| def(IVec3::new(x, y, z)).is_some_and(|d| d.name != "core:air"))))
    });
    if !well_built {
        problems.push(format!("no well built on the square ({})", if well.is_some() { "planned, but nothing stands there" } else { "none planned" }));
    }
    let (mut doors, mut houses) = (0, 0);
    for b in plan.buildings.iter().filter(|b| within(b.min) && within(b.max)) {
        let middle = (b.min + b.max) / 2;
        if b.door.is_some() {
            houses += 1;
            if wet(IVec3::new(middle.x, b.floor + 1, middle.y)) {
                problems.push(format!("water inside the {} at {middle}", b.kind.name()));
            }
        }
        let Some(door) = b.door else { continue };
        let step = b.facing.step();
        let (out, inside) = (door + IVec3::new(step.x, 0, step.y), door - IVec3::new(step.x, 0, step.y));
        let blocked: Vec<IVec3> = [door, door + IVec3::Y, out, out + IVec3::Y, inside, inside + IVec3::Y].into_iter().filter(|&p| solid(p)).collect();
        if !blocked.is_empty() {
            problems.push(format!("the {}'s door at {door} is blocked at {blocked:?}", b.kind.name()));
        } else if !solid(door - IVec3::Y) || !solid(inside - IVec3::Y) {
            problems.push(format!("the {}'s door at {door} has no floor under it or behind it", b.kind.name()));
        } else {
            doors += 1;
        }
    }
    if doors == 0 && problems.is_empty() {
        problems.push("no door within reach of the square to judge".into());
    }
    let name = &plan.name.0;
    if problems.is_empty() {
        Ok(format!("{name}, a {} at {at}: a well on the square, {doors} doors open onto floors, {houses} buildings dry", plan.size))
    } else {
        Err(format!("{name}, a {} at {at}: {}", plan.size, problems.join("; ")))
    }
}

fn gorge(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    match gate.phase {
        0 => {
            if let Some(observer) = gate.observer.as_mut() {
                observer.replies.clear();
                observer.connection.send(ClientMessage::Command { line: format!("locate gorge {GORGE_RADIUS}") });
            }
            gate.advance(now);
        }
        1 => {
            let content = world.resource::<Launch>().content.clone();
            let english = |words| content.english.render(words, &content.blocks);
            let Some(reply) = gate.observer.as_ref().and_then(|o| o.replies.iter().map(english).find(|r| r.contains("gorge"))) else {
                if in_phase > 20.0 {
                    gate.check("gorge", Err("no answer to /locate gorge".into()));
                    gate.next(Step::Settlements, now);
                }
                return;
            };
            // "… /tp X Y Z goes there."
            let at = reply.split("/tp ").nth(1).and_then(|rest| {
                let n: Vec<i32> = rest.split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
                (n.len() == 3).then(|| IVec3::new(n[0], n[1], n[2]))
            });
            let Some(at) = at else {
                gate.check("gorge", Err(format!("/locate gorge: {reply}")));
                gate.next(Step::Settlements, now);
                return;
            };
            gate.gorge = at;
            let net = world.resource::<Net>();
            net.command("time set noon");
            net.command(&format!("tp {} {} {}", at.x as f32 + 0.5, at.y, at.z as f32 + 0.5));
            if let Some(observer) = gate.observer.as_mut() {
                observer.connection.send(ClientMessage::Command { line: format!("tp {} {} {}", at.x, at.y + 40, at.z) });
            }
            gate.advance(now);
        }
        2 => {
            let at = gate.gorge;
            // The columns around it on the server's side and the player's, meshed (the
            // distance may still be loading; the check needs only these).
            let held = gate.observer.as_ref().is_some_and(|o| columns_around(at).into_iter().all(|c| o.world.contains(c)));
            gate.gorge_reached |= world.resource::<LocalPlayer>().position.xz().distance(at.xz().as_vec2()) < 4.0;
            let there = gate.gorge_reached;
            let client = world.resource::<ClientWorld>();
            let shown = columns_around(at).into_iter().all(|c| client.voxels.contains(c)) && settled(world, at);
            if !(held && there && shown && in_phase > 1.0) {
                let (load, cores) = (load_average(), std::thread::available_parallelism().map_or(1, |n| n.get()) as f64);
                let limit = if load > cores { GORGE_OVERLOADED_LIMIT } else { GORGE_LOAD_LIMIT };
                if in_phase > limit {
                    // What was missing, so a slow load can be told from one that never comes.
                    let columns = columns_around(at);
                    let on_server = gate.observer.as_ref().map_or(0, |o| columns.iter().filter(|&&c| o.world.contains(c)).count());
                    let on_client = columns.iter().filter(|&&c| client.voxels.contains(c)).count();
                    let meshing = columns.iter().filter(|&&c| client.meshing(c)).count();
                    let away = world.resource::<LocalPlayer>().position.xz().distance(at.xz().as_vec2());
                    let n = columns.len();
                    let reached = if gate.gorge_reached { "got there and is now" } else { "never got within 4 blocks and is" };
                    gate.check(
                        "gorge",
                        Err(format!(
                            "the world around the gorge at {at} did not load in {limit:.0}s (load {load:.0} on {cores} cores): \
                             the server has {on_server} of its {n} columns, the client {on_client} with {meshing} still meshing, \
                             and the player {reached} {away:.0} blocks away"
                        )),
                    );
                    gate.next(Step::Settlements, now);
                }
                return;
            }
            look(world, 0.0, 25.0);
            gate.advance(now);
        }
        3 if in_phase > 0.5 => {
            if gate.shot(world, "gorge").is_none() {
                return;
            }
            let client = world.resource::<ClientWorld>();
            let Some(observer) = gate.observer.as_ref() else { return };
            let def = |p: IVec3| observer.world.block(p).map(|b| client.registry.def(b));
            // Water in any of its states (a river's is mostly flowing).
            let wet = |p: IVec3| observer.world.block(p).is_some_and(|b| client.registry.fluid(b).is_some());
            // The river's water, within a couple of blocks of the place named (which lies on
            // its course; the water there may stand a block or two lower after a fall).
            let named = gate.gorge - IVec3::Y;
            let found = (-2..=2)
                .flat_map(|dz| (-2..=2).flat_map(move |dx| (-3..=0).map(move |dy| named + IVec3::new(dx, dy, dz))))
                .find(|&p| wet(p) && !wet(p + IVec3::Y));
            let is_water = found.is_some();
            let (at, water) = found.map_or((named, named.y), |p| (p, p.y));
            // The ground's top in a column: its highest solid block, trees aside.
            let ground = |x: i32, z: i32| {
                (water..=water + 40).rev().find(|&y| {
                    def(IVec3::new(x, y, z)).is_some_and(|d| d.solid && d.render != voxmar_engine::block::RenderKind::Leaves && !d.name.contains("log") && !d.name.contains("wood"))
                })
            };
            let side = |dx: i32, dz: i32| (3..=8).filter_map(|k| ground(at.x + dx * k, at.z + dz * k)).max().unwrap_or(water) - water;
            let walls = [(1, 0), (0, 1), (1, 1), (1, -1)].map(|(dx, dz)| side(dx, dz).min(side(-dx, -dz))).into_iter().max().unwrap_or(0);
            let result = match (is_water, walls >= GORGE_WALLS) {
                (true, true) => Ok(format!("water at {} {} {} between walls {walls} blocks high", at.x, water, at.z)),
                (false, _) => Err(format!("no water within two blocks of {named}, where /locate gorge said")),
                (true, false) => Err(format!("the walls by the water at {} {} {} stand only {walls} blocks high", at.x, water, at.z)),
            };
            gate.check("gorge", result);
            gate.next(Step::Settlements, now);
        }
        _ => {}
    }
}

/// How far from the test ground `/locate ore` looks for a sample of ore: samples lie over
/// every vein, which lie a hundred or so blocks apart.
const ORE_RADIUS: i32 = 600;

/// `/locate ore` finds a sample of ore on the ground near the test ground (before the
/// gorge, whose own long load then settles the server before the steps after). Going there, the
/// server's world has it on solid ground with ore of the same kind below it, its vein,
/// the client has it too, and the ore around lies in veins, not alone.
fn ore(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    match gate.phase {
        0 => {
            if let Some(observer) = gate.observer.as_mut() {
                observer.replies.clear();
                observer.connection.send(ClientMessage::Command { line: format!("locate ore {ORE_RADIUS}") });
            }
            gate.advance(now);
        }
        1 => {
            let content = world.resource::<Launch>().content.clone();
            let english = |words| content.english.render(words, &content.blocks);
            let Some(reply) = gate.observer.as_ref().and_then(|o| o.replies.iter().map(english).find(|r| r.contains("vein") || r.contains("ore"))) else {
                if in_phase > 20.0 {
                    gate.check("ore", Err("no answer to /locate ore".into()));
                    gate.next(Step::Gorge, now);
                }
                return;
            };
            // "… /tp X Y Z goes there."
            let at = reply.split("/tp ").nth(1).and_then(|rest| {
                let n: Vec<i32> = rest.split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
                (n.len() == 3).then(|| IVec3::new(n[0], n[1], n[2]))
            });
            let Some(at) = at else {
                gate.check("ore", Err(format!("/locate ore: {reply}")));
                gate.next(Step::Gorge, now);
                return;
            };
            gate.ore = at;
            let net = world.resource::<Net>();
            net.command(&format!("tp {} {} {}", at.x as f32 + 0.5, at.y, at.z as f32 + 0.5));
            if let Some(observer) = gate.observer.as_mut() {
                observer.connection.send(ClientMessage::Command { line: format!("tp {} {} {}", at.x, at.y + 40, at.z) });
            }
            gate.advance(now);
        }
        2 => {
            let at = gate.ore;
            let held = gate.observer.as_ref().is_some_and(|o| columns_around(at).into_iter().all(|c| o.world.contains(c)));
            let there = world.resource::<LocalPlayer>().position.xz().distance(at.xz().as_vec2() + 0.5) < 4.0;
            let client = world.resource::<ClientWorld>();
            let shown = columns_around(at).into_iter().all(|c| client.voxels.contains(c)) && settled(world, at);
            if !(held && there && shown && in_phase > 1.0) {
                let (load, cores) = (load_average(), std::thread::available_parallelism().map_or(1, |n| n.get()) as f64);
                let limit = if load > cores { GORGE_OVERLOADED_LIMIT } else { GORGE_LOAD_LIMIT };
                if in_phase > limit {
                    gate.check("ore", Err(format!("the world around the sample of ore at {at} did not load in {limit:.0}s (load {load:.0} on {cores} cores)")));
                    gate.next(Step::Gorge, now);
                }
                return;
            }
            // Standing over it, looking down at it a little from the side.
            look(world, 0.0, -70.0);
            gate.advance(now);
        }
        3 if in_phase > 0.5 => {
            if gate.shot(world, "ore").is_none() {
                return;
            }
            let client = world.resource::<ClientWorld>();
            let registry = client.registry.clone();
            let Some(observer) = gate.observer.as_ref() else { return };
            let at = gate.ore;
            let ore_of = |block: BlockId| {
                let def = registry.def(block);
                (def.has_tag("ore") && !def.has_tag("ore_sample")).then(|| def.drops.clone().unwrap_or_else(|| def.name.clone()))
            };
            let sample = observer.world.block(at).filter(|&b| registry.def(b).has_tag("ore_sample"));
            let result = match sample {
                None => Err(format!("no sample of ore at {at}, where /locate ore said")),
                Some(sample) => {
                    let def = registry.def(sample);
                    let wanted = def.drops.clone().unwrap_or_else(|| def.name.clone());
                    let ground = observer.world.block(at - IVec3::Y).is_some_and(|b| registry.def(b).solid);
                    // The ore around, from the bottom of the world up to the sample: its vein
                    // below it, and none of it lying alone.
                    let mut ores = Vec::new();
                    for z in -AGREE_WITHIN..=AGREE_WITHIN {
                        for x in -AGREE_WITHIN..=AGREE_WITHIN {
                            for y in 1..at.y {
                                let p = IVec3::new(at.x + x, y, at.z + z);
                                if let Some(ore) = observer.world.block(p).and_then(ore_of) {
                                    ores.push((p, ore));
                                }
                            }
                        }
                    }
                    let below = ores.iter().filter(|(_, o)| *o == wanted).count();
                    let alone = ores
                        .iter()
                        .filter(|(p, _)| !ores.iter().any(|(q, _)| q != p && (*q - *p).abs().max_element() <= 2))
                        .count();
                    let drawn = client.block(at) == Some(sample);
                    match (ground, below > 0, alone * 30 <= ores.len(), drawn) {
                        (false, ..) => Err(format!("the {} at {at} lies on no solid ground", def.name)),
                        (_, false, ..) => Err(format!("no {wanted} below the {} at {at}: it points to no vein", def.name)),
                        (.., false, _) => Err(format!("{alone} of the {} ore blocks around {at} lie alone, not in veins", ores.len())),
                        (.., false) => Err(format!("the client does not have the {} at {at}", def.name)),
                        _ => Ok(format!("a {} at {at} over {below} blocks of {wanted} ({alone} of {} ore blocks around alone)", def.name, ores.len())),
                    }
                }
            };
            gate.check("ore", result);
            gate.next(Step::Gorge, now);
        }
        _ => {}
    }
}

/// The direction to the light of the moment: the sun by day, the moon by night.
fn to_light(world: &mut World) -> Option<Vec3> {
    let mut query = world.query_filtered::<&Transform, With<crate::render::Sun>>();
    query.single(world).ok().map(|light| light.back().as_vec3())
}

/// Turns the player to have the light of the moment high in the view, clear of the
/// crosshair and the console's lines.
fn aim_at_light(world: &mut World) {
    let Some(to_light) = to_light(world) else { return };
    let mut player = world.resource_mut::<LocalPlayer>();
    player.yaw = (-to_light.x).atan2(-to_light.z);
    player.pitch = to_light.y.atan2(to_light.xz().length()) - 12f32.to_radians();
}

/// Where the light of the moment is in the shot, in pixels.
fn light_on_screen(world: &mut World) -> Option<Vec2> {
    let to_light = to_light(world)?;
    let mut cameras = world.query_filtered::<(&Camera, &GlobalTransform), With<Camera3d>>();
    let (camera, transform) = cameras.single(world).ok()?;
    camera.world_to_viewport(transform, transform.translation() + to_light * 1000.0).ok()
}

/// Least mean change (levels of 255) across the screen for a menu or weather to count as
/// shown.
const SCREEN_CHANGE: f32 = 6.0;

/// Least levels the sun or moon's disc stands out from the sky a little way off.
const DISC_CONTRAST: u32 = 20;

/// A bright disc in a shot where the sun or moon should be (`at`): near white, and
/// brighter than the sky to either side, beyond the sun's glow.
fn disc(shot: &Image, at: Option<Vec2>, what: &str) -> Result<String, String> {
    let (w, h, data) = pixels(shot)?;
    let at = at.ok_or_else(|| format!("the {what} is not in view"))?;
    let (cx, cy) = (at.x.round() as i32, at.y.round() as i32);
    let luminance = |(x, y): (i32, i32)| {
        let (x, y) = (x.clamp(0, w as i32 - 1) as usize, y.clamp(0, h as i32 - 1) as usize);
        let i = (y * w + x) * 4;
        (data[i] as u32 * 54 + data[i + 1] as u32 * 183 + data[i + 2] as u32 * 19) / 256
    };
    let ring = |r: i32| [(cx - r, cy), (cx + r, cy), (cx, cy - r), (cx, cy + r), (cx - r, cy - r), (cx + r, cy + r), (cx - r, cy + r), (cx + r, cy - r)];
    let dimmest = std::iter::once((cx, cy)).chain(ring(2)).map(luminance).min().unwrap_or(0);
    let around = luminance((cx - 150, cy)).max(luminance((cx + 150, cy)));
    if dimmest >= 220 && dimmest >= around + DISC_CONTRAST {
        Ok(format!("a {what} disc at {dimmest} levels against the sky's {around} nearby"))
    } else {
        Err(format!("no {what} disc: {dimmest} levels where it should be, the sky {around} nearby"))
    }
}

/// A day sky darkens from the horizon upwards: the top of a shot aimed at the morning
/// sun differs from its bottom.
fn gradient(shot: &Image) -> Result<String, String> {
    let (w, h, data) = pixels(shot)?;
    let band = |y0: usize, y1: usize| {
        let mut sum = [0u64; 3];
        for y in y0..y1 {
            for x in 0..w {
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += data[(y * w + x) * 4 + c] as u64;
                }
            }
        }
        sum.map(|s| s as f32 / ((y1 - y0) * w) as f32)
    };
    let (top, low) = (band(0, h / 10), band(h * 6 / 10, h * 7 / 10));
    let difference = (0..3).map(|c| (top[c] - low[c]).abs()).fold(0.0, f32::max);
    if difference >= 8.0 {
        Ok(format!("the sky's top differs from lower down by {difference:.0} levels"))
    } else {
        Err(format!("the sky is flat: top and lower down differ by {difference:.0} levels"))
    }
}

/// Where the gate flies to look at the sky from above the world's top (384).
const ABOVE_THE_WORLD: f32 = 450.0;

/// A day sky seen from high above the world is lit: the top fifth of the shot is not dark.
fn sky_lit(shot: &Image) -> Result<String, String> {
    let (w, h, data) = pixels(shot)?;
    let mut sum = 0u64;
    for y in 0..h / 5 {
        for x in 0..w {
            let i = (y * w + x) * 4;
            sum += (data[i] as u64 * 54 + data[i + 1] as u64 * 183 + data[i + 2] as u64 * 19) / 256;
        }
    }
    let mean = sum as f32 / (w * (h / 5)) as f32;
    if mean >= 80.0 {
        Ok(format!("the sky from above the world is lit, {mean:.0} levels"))
    } else {
        Err(format!("the sky from above the world is dark, {mean:.0} levels"))
    }
}

/// Least levels by which the sky above the horizon is redder (red less blue) towards the
/// setting sun than away from it.
const SUNSET_WARMTH: f32 = 15.0;

/// At sunset the sky low down is warm towards the sun and cool away from it: in a band
/// above the middle of shots aimed a little above the horizon either way.
fn sunset(towards: &Image, away: &Image) -> Result<String, String> {
    let warmth = |shot: &Image| -> Result<f32, String> {
        let (w, h, data) = pixels(shot)?;
        let (mut red, mut blue) = (0u64, 0u64);
        for y in h / 5..h * 2 / 5 {
            for x in 0..w {
                red += data[(y * w + x) * 4] as u64;
                blue += data[(y * w + x) * 4 + 2] as u64;
            }
        }
        Ok((red as f32 - blue as f32) / (w * (h / 5)) as f32)
    };
    let (warm, cool) = (warmth(towards)?, warmth(away)?);
    if warm - cool >= SUNSET_WARMTH {
        Ok(format!("towards the setting sun the sky is {warm:+.0} levels red over blue, away from it {cool:+.0}"))
    } else {
        Err(format!("the sunset sky is as warm away from the sun ({cool:+.0} red over blue) as towards it ({warm:+.0})"))
    }
}

/// A night sky away from the moon shows stars: small points much brighter than the dark
/// around them, in the top two fifths of the shot, above the console's lines.
fn stars(shot: &Image) -> Result<String, String> {
    let (w, h, data) = pixels(shot)?;
    let luminance = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        (data[i] as i32 * 54 + data[i + 1] as i32 * 183 + data[i + 2] as i32 * 19) / 256
    };
    let mut count = 0;
    for y in 2..h * 2 / 5 {
        for x in 2..w - 2 {
            let here = luminance(x, y);
            let dark = [luminance(x - 2, y), luminance(x + 2, y), luminance(x, y - 2), luminance(x, y + 2)].into_iter().max().unwrap_or(0);
            count += (here >= 60 && here >= dark + 30) as usize;
        }
    }
    if count >= 40 {
        Ok(format!("{count} star pixels"))
    } else {
        Err(format!("only {count} star pixels"))
    }
}

/// Open ground at midnight, in the lower half of a shot: dark, but not black. Before nights
/// were lifted (2026-10-06) a moonless night's ground averaged 2 or 3 of 255 and Mariano
/// could not see where he walked; nights averaging 8 (moonless) to 17 (full moon) he found
/// far too bright; they now average about 5 to 9.
fn night_ground(shot: &Image) -> Result<String, String> {
    const DARKEST: f32 = 3.5;
    const BRIGHTEST: f32 = 40.0;
    let (w, h, data) = pixels(shot)?;
    let (mut sum, mut count) = (0.0f32, 0.0f32);
    // The lower half, above the hotbar.
    for y in h / 2..h * 4 / 5 {
        for x in 0..w {
            let i = (y * w + x) * 4;
            sum += (data[i] as f32 * 54.0 + data[i + 1] as f32 * 183.0 + data[i + 2] as f32 * 19.0) / 256.0;
            count += 1.0;
        }
    }
    let mean = sum / count.max(1.0);
    match mean {
        m if m < DARKEST => Err(format!("the ground at night averages {m:.1} of 255: black")),
        m if m > BRIGHTEST => Err(format!("the ground at night averages {m:.1} of 255: not night")),
        m => Ok(format!("the ground at night averages {m:.1} of 255")),
    }
}

/// A shot's size and RGBA8 pixels.
fn pixels(shot: &Image) -> Result<(usize, usize, &[u8]), String> {
    let (w, h) = (shot.width() as usize, shot.height() as usize);
    match &shot.data {
        Some(data) if data.len() == w * h * 4 => Ok((w, h, data)),
        _ => Err("the shot has no pixels".into()),
    }
}

/// Looks over the land around spawn as generated: its surface, block by block, and what
/// grows on it.
fn land(world: &World, gate: &mut Gate) {
    let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
    let registry = &client.registry;
    let centre = player.position.floor().as_ivec3();
    let (mut columns, mut grass, mut covered, mut tall) = (0usize, 0usize, 0usize, 0usize);
    let (mut worn, mut crafted) = (Vec::new(), Vec::new());
    for z in centre.z - LAND_REACH..=centre.z + LAND_REACH {
        for x in centre.x - LAND_REACH..=centre.x + LAND_REACH {
            // The ground: the highest solid block that is not part of a tree.
            let Some((y, top)) = (0..voxmar_engine::coords::WORLD_HEIGHT).rev().find_map(|y| {
                let block = client.block(IVec3::new(x, y, z))?;
                let def = registry.def(block);
                let tree = def.has_tag("leaves") || def.has_tag("log") || def.name.ends_with("_log") || def.name.ends_with("_wood");
                (registry.info(block).solid && !tree).then_some((y, block))
            }) else {
                continue;
            };
            columns += 1;
            let def = registry.def(top);
            let at = IVec3::new(x, y, z);
            if def.name.ends_with("worn_grass_block") || def.name.ends_with("_planks") {
                worn.push(format!("{} at {at}", def.name));
            }
            if ["cobbled", "polished", "bricks"].iter().any(|t| def.has_tag(t)) {
                crafted.push(format!("{} at {at}", def.name));
            }
            if def.name.ends_with(":grass_block") {
                grass += 1;
                if let Some(above) = client.block(at + IVec3::Y).filter(|&b| registry.def(b).render == voxmar_engine::block::RenderKind::Plant) {
                    covered += 1;
                    tall += usize::from(registry.def(above).name.contains("tall_grass"));
                }
            }
        }
    }
    let first = |list: &[String]| list.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    gate.check(
        "land: no trails",
        if worn.is_empty() { Ok(format!("no worn tread or decks in {columns} columns around spawn")) } else { Err(format!("{} columns: {}", worn.len(), first(&worn))) },
    );
    gate.check(
        "land: natural stone",
        if crafted.is_empty() { Ok("no cobbled or crafted stone at the surface".into()) } else { Err(format!("{} columns: {}", crafted.len(), first(&crafted))) },
    );
    let cover = covered as f32 / grass.max(1) as f32;
    let tall_share = tall as f32 / covered.max(1) as f32;
    gate.check(
        "land: ground cover",
        if grass < LAND_GRASS {
            Ok(format!("only {grass} grass columns around spawn, too few to judge"))
        } else if !(0.1..=0.95).contains(&cover) {
            // Only the extremes fail here, whatever biome spawn lands in when the world
            // reseeds; each biome's own share of grass and tall grass is held by the
            // worldgen test `grass_is_kept_in_check`.
            Err(format!("{:.0}% of {grass} grass blocks carry a plant (wanted 10-95%)", 100.0 * cover))
        } else {
            Ok(format!("{:.0}% of {grass} grass blocks carry a plant, {:.0}% of them tall grass", 100.0 * cover, 100.0 * tall_share))
        },
    );
}

/// Whether the world around the player has loaded and been meshed, distant terrain
/// included.
fn loaded(world: &World) -> bool {
    let (player, client, lod) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>(), world.resource::<Lod>());
    let (tiles, wanted) = lod.progress();
    player.joined && client.stats.jobs_in_flight == 0 && client.stats.pending_columns == 0 && client.voxels.len() >= 9 && tiles >= wanted
}

/// Columns within `AGREE_WITHIN` of `feet`.
fn columns_around(feet: IVec3) -> Vec<ColumnPos> {
    let (low, high) = (feet - IVec3::splat(AGREE_WITHIN), feet + IVec3::splat(AGREE_WITHIN));
    let (low, high) = (ColumnPos::of_block(low).0, ColumnPos::of_block(high).0);
    (low.x..=high.x).flat_map(|x| (low.y..=high.y).map(move |z| ColumnPos::new(x, z))).collect()
}

/// Whether no column around `feet` waits to be meshed.
fn settled(world: &World, feet: IVec3) -> bool {
    let client = world.resource::<ClientWorld>();
    columns_around(feet).into_iter().all(|pos| !client.meshing(pos))
}

/// The longest frame a step may take: the first break or placement once compiled the
/// pipelines for cracks, particles and dropped items on the spot, a hitch Mariano felt as
/// block breaking lag. Gate runs share a busy machine, so the bar is high and relative.
const HITCH_MS: f32 = 150.0;
const HITCH_FACTOR: f32 = 6.0;

/// Checks that `step` (placing or digging) ran without a hitch: no frame over
/// max(`HITCH_MS`, `HITCH_FACTOR` × the session's median frame).
fn smooth(gate: &mut Gate, name: &str, step: Step, known: Option<&str>) {
    let mut sorted = gate.frame_ms.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let longest = gate.longest.get(&step).copied().unwrap_or(0.0);
    // A machine asked for more than all its cores (other threads' builds) stretches every
    // frame's waits by about how far over it is: the bar rises with it rather than the
    // check being skipped, for the machine is rarely calm enough to wait for. Pipelines
    // compiled on the spot, what this check first caught, the pipelines check catches at
    // any load.
    let (load, cores) = (load_average(), std::thread::available_parallelism().map_or(1, |n| n.get()) as f64);
    let crowded = (load / cores).max(1.0) as f32;
    let limit = HITCH_MS.max(HITCH_FACTOR * median) * crowded;
    let on = if crowded > 1.0 { format!(", the bar {crowded:.1}× higher at load {load:.0} on {cores} cores") } else { String::new() };
    let result = if longest <= limit {
        Ok(format!("its longest frame took {longest:.0} ms (median {median:.0} ms{on})"))
    } else {
        Err(format!("a frame took {longest:.0} ms, over {limit:.0} ms (median {median:.0} ms{on})"))
    };
    let known = if std::env::var_os("VOXMAR_GATE_STALL").is_some() { Some("frames stalled on purpose (VOXMAR_GATE_STALL)") } else { known };
    match known {
        Some(why) => gate.known(&format!("{name}: smooth"), result, why),
        None => gate.check(&format!("{name}: smooth"), result),
    }
}

/// The machine's load average over the last minute (runnable threads), or 0 if unknown.
fn load_average() -> f64 {
    unsafe extern "C" {
        fn getloadavg(loadavg: *mut f64, nelem: i32) -> i32;
    }
    let mut load = [0.0f64; 1];
    // SAFETY: writes at most one value into a one-element buffer.
    let got = unsafe { getloadavg(load.as_mut_ptr(), 1) };
    if got == 1 { load[0] } else { 0.0 }
}

/// Whether an edit of `cells` has settled, `since` seconds after it was made: both client
/// and server show each as `wanted` and nothing around waits to be meshed, or the limit
/// has passed (and the checks will say what is missing).
fn edit_settled(world: &World, gate: &Gate, since: f32, cells: &[IVec3], wanted: fn(BlockId) -> bool) -> bool {
    if since < SETTLE {
        return false;
    }
    let client = world.resource::<ClientWorld>();
    let shown = |side: &dyn Fn(IVec3) -> Option<BlockId>| cells.iter().all(|&p| side(p).is_some_and(wanted));
    let server = gate.observer.as_ref().is_none_or(|o| shown(&|p| o.world.block(p)));
    // A break the client finished early completes on the server in its own time, so the
    // whole area must agree, not only the cells aimed at.
    let agree = gate.observer.as_ref().is_none_or(|o| disagreements(client, &o.world, gate.feet).is_empty());
    (server && agree && shown(&|p| client.block(p)) && settled(world, gate.feet)) || since > SETTLE_LIMIT
}

/// Blocks within `AGREE_WITHIN` of `feet` that client and server both have and see
/// differently.
fn disagreements(client: &ClientWorld, server: &VoxelWorld, feet: IVec3) -> Vec<(IVec3, BlockId, BlockId)> {
    let r = AGREE_WITHIN;
    let mut differ = Vec::new();
    for dy in -r..=r {
        for dz in -r..=r {
            for dx in -r..=r {
                let p = feet + IVec3::new(dx, dy, dz);
                if let (Some(ours), Some(theirs)) = (client.block(p), server.block(p))
                    && ours != theirs
                {
                    differ.push((p, ours, theirs));
                }
            }
        }
    }
    differ
}

/// Checks an edit of `cells` once it has settled: each is as `wanted` on the client and
/// the server, the two agree everywhere around, the renderer has it, and the middle of
/// the screen changed between the shots before and after.
fn verify_edit(world: &World, gate: &mut Gate, name: &str, cells: &[IVec3], wanted: fn(BlockId) -> bool, before: &'static str, after: &Image) {
    let client = world.resource::<ClientWorld>();
    let named = |block: Option<BlockId>| block.map_or_else(|| "not loaded".to_owned(), |b| client.registry.def(b).name.clone());
    let expect = |side: &dyn Fn(IVec3) -> Option<BlockId>| {
        let wrong: Vec<String> = cells.iter().filter(|&&p| !side(p).is_some_and(wanted)).map(|&p| format!("{p} is {}", named(side(p)))).collect();
        if wrong.is_empty() { Ok(format!("{} blocks as expected", cells.len())) } else { Err(wrong.join(", ")) }
    };
    let mut checks = vec![("client", expect(&|p| client.block(p)))];
    if let Some(observer) = &gate.observer {
        checks.push(("server", expect(&|p| observer.world.block(p))));
        let r = AGREE_WITHIN;
        let differ: Vec<String> = disagreements(client, &observer.world, gate.feet)
            .into_iter()
            .map(|(p, ours, theirs)| format!("{p}: client {}, server {}", named(Some(ours)), named(Some(theirs))))
            .collect();
        checks.push((
            "agree",
            if differ.is_empty() {
                Ok(format!("client and server agree within {r} blocks"))
            } else {
                Err(format!("{} blocks differ: {}", differ.len(), differ.into_iter().take(6).collect::<Vec<_>>().join("; ")))
            },
        ));
    }
    let stale: Vec<String> = columns_around(gate.feet).into_iter().filter_map(|pos| client.check_meshed(pos).err()).collect();
    checks.push(("meshed", if stale.is_empty() { Ok("the renderer has every column as it is".into()) } else { Err(stale.join("; ")) }));
    let change = gate.shots.lock().expect("gate shots").get(before).map(|before| change(before, after, 7.0 / 16.0));
    checks.push((
        "shown",
        match change {
            Some(change) if change >= SHOWN_CHANGE => Ok(format!("the middle of the screen changed by {change:.1} levels")),
            Some(change) => Err(format!("the middle of the screen changed by only {change:.1} levels")),
            None => Err(format!("no shot {before}")),
        },
    ));
    for (check, result) in checks {
        gate.check(&format!("{name}: {check}"), result);
    }
}

/// Mean change between two shots of the same size, in levels of 255, over the frame less
/// `inset` (a fraction of its width and height) on every side: 0 for all of it, 7/16 for
/// the middle eighth.
fn change(a: &Image, b: &Image, inset: f32) -> f32 {
    let (Ok((w, h, da)), Ok((wb, hb, db))) = (pixels(a), pixels(b)) else { return 0.0 };
    if (w, h) != (wb, hb) {
        return 0.0;
    }
    let (x0, y0) = ((w as f32 * inset) as usize, (h as f32 * inset) as usize);
    let (mut sum, mut n) = (0u64, 0u64);
    for y in y0..h - y0 {
        for x in x0..w - x0 {
            let i = (y * w + x) * 4;
            sum += (0..3).map(|c| da[i + c].abs_diff(db[i + c]) as u64).sum::<u64>();
            n += 3;
        }
    }
    sum as f32 / n.max(1) as f32
}

/// Turns the player to look at the middle of the top of `block`, as moving the mouse
/// would.
fn aim(world: &mut World, block: IVec3) {
    let mut player = world.resource_mut::<LocalPlayer>();
    let to = block.as_vec3() + Vec3::new(0.5, 0.999, 0.5) - player.eye();
    player.yaw = (-to.x).atan2(-to.z);
    player.pitch = to.y.atan2(to.xz().length());
}

/// Turns the player to `yaw` and `pitch` (degrees), as moving the mouse would.
fn look(world: &mut World, yaw: f32, pitch: f32) {
    let mut player = world.resource_mut::<LocalPlayer>();
    player.yaw = yaw.to_radians();
    player.pitch = pitch.to_radians();
}

fn mouse(world: &mut World, button: MouseButton, down: bool) {
    let state = if down { ButtonState::Pressed } else { ButtonState::Released };
    world.write_message(MouseButtonInput { button, state, window: Entity::PLACEHOLDER });
}

fn key(world: &mut World, key_code: KeyCode, down: bool) {
    let state = if down { ButtonState::Pressed } else { ButtonState::Released };
    let logical_key = Key::Unidentified(NativeKey::Unidentified);
    world.write_message(KeyboardInput { key_code, logical_key, state, text: None, repeat: false, window: Entity::PLACEHOLDER });
}

/// The hotbar slot holding `item`; if it is only in the pack, asks once (`moved`) for it to
/// be moved into the last hotbar slot, as dragging it there would.
fn in_hotbar(world: &mut World, moved: &mut bool, item: BlockId) -> Option<usize> {
    use voxmar_engine::inventory::{HOTBAR_SLOTS, SLOTS};
    let player = world.resource::<LocalPlayer>();
    let holds = |s: &usize| player.inventory.get(*s).is_some_and(|stack| stack.item == item);
    let (slot, packed) = ((0..HOTBAR_SLOTS).find(holds), (HOTBAR_SLOTS..SLOTS).find(holds));
    if slot.is_none()
        && !*moved
        && let Some(from) = packed
    {
        let actions = world.resource::<crate::Actions>().0.clone();
        let target = voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: (HOTBAR_SLOTS - 1) as u8, count: u8::MAX };
        world.resource::<Net>().propose(actions.expect("core:transfer"), target);
        *moved = true;
    }
    slot
}

/// How many items the selected hotbar slot holds.
fn held_count(world: &World) -> u32 {
    let player = world.resource::<LocalPlayer>();
    player.inventory.get(player.selected).map_or(0, |s| s.count as u32)
}

/// How many of what the dug blocks drop the inventory holds.
fn dropped_count(world: &World, gate: &Gate) -> u32 {
    let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
    let Some(&(_, block)) = gate.dug.first() else { return 0 };
    let item = client.registry.info(block).drops;
    (0..voxmar_engine::inventory::SLOTS).filter_map(|slot| player.inventory.get(slot)).filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

/// Writes the report and exits, with an error if any check failed.
fn finish(world: &mut World, gate: &Gate, now: f32) {
    let verdict = if gate.failures.is_empty() { "passed" } else { "FAILED" };
    let mut report = format!("gate {verdict}: {} checks passed, {} failed, in {now:.1}s\n", gate.passes, gate.failures.len());
    for failure in &gate.failures {
        report += &format!("FAILED {failure}\n");
    }
    if let Err(e) = std::fs::write(gate.dir.join("report.txt"), &report) {
        error!("cannot write the gate report: {e}");
    }
    if gate.failures.is_empty() {
        info!("{}", report.trim_end());
        world.write_message(AppExit::Success);
    } else {
        error!("{}", report.trim_end());
        world.write_message(AppExit::error());
    }
}
