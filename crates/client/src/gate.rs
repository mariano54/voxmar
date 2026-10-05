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
//! It runs headless like `--capture`, builds its own test ground high above spawn with
//! commands (so terrain changes don't move it), and exits with an error if any check
//! failed. Each check is logged as `gate: ok …` or `gate: FAILED …`, and the shots are
//! saved in DIR for a look.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::world::VoxelWorld;
use voxmar_protocol::{ClientConnection, ClientMessage, PROTOCOL_VERSION, PlayerId, PlayerInput, ServerMessage, local_pair};

use crate::menu::{Notice, Screen};
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::render::lod::Lod;
use crate::session::Launch;
use crate::world::ClientWorld;

pub struct GatePlugin {
    pub dir: PathBuf,
}

/// How much longer each tick of the integrated server takes in a gate run: more than a
/// tick's 50 ms, so the server keeps running behind, as on a busy machine, and its word on
/// every action comes late (about 15 ticks a second instead of 20). Mariano's play builds
/// lost blocks he broke to exactly that (fixed in 17bf62f).
pub const SERVER_LAG: Duration = Duration::from_millis(60);

/// Size of the gate's shots: enough to judge, quick to read back.
const SHOT: UVec2 = UVec2::new(960, 540);
/// Longest the whole session may take, and the world to load, in seconds of game time.
const TIME_LIMIT: f32 = 300.0;
const LOAD_LIMIT: f32 = 180.0;
/// Longest a single step may take.
const STEP_LIMIT: f32 = 30.0;
/// How long an edit is given at least, and at most, to reach the server, come back and be
/// meshed; the checks run as soon as both sides show it.
const SETTLE: f32 = 0.3;
const SETTLE_LIMIT: f32 = 4.0;
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
    }
}

fn render_offscreen(
    mut commands: Commands,
    mut gate: ResMut<Gate>,
    mut images: ResMut<Assets<Image>>,
    camera: Query<Entity, With<Camera3d>>,
) {
    gate.target = crate::capture::offscreen(&mut commands, &mut images, &camera, SHOT);
}

/// The steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Swimming against the pool's bank and holding jump climbs out onto the block on it.
    Swim,
    /// Esc opens and closes the menu, E the inventory.
    Screens,
    /// A storm and a snowfall come, drawn with their particles, then clear again.
    Weather,
    /// The sun by day, the moon and stars by night.
    Sky,
    /// A small tree with an apple among its leaves, drawn with its leaf cards and fruit.
    Tree,
    /// `/locate gorge` finds a river deep between walls, and the server's world there has
    /// them.
    Gorge,
    /// Leaving the world for the title screen.
    Leave,
    Done,
}

#[derive(Resource)]
struct Gate {
    dir: PathBuf,
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
    /// The highest the player's feet reached while climbing out of the pool.
    highest: f32,
    /// Where `/locate gorge` sent the player: the column, one block above its water.
    gorge: IVec3,
    failures: Vec<String>,
    passes: usize,
}

impl Gate {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
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
            highest: f32::MIN,
            gorge: IVec3::ZERO,
            failures: Vec::new(),
            passes: 0,
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

    fn next(&mut self, step: Step, now: f32) {
        info!("gate: {step:?} at {now:.1}s");
        self.step = step;
        self.since = now;
        self.frames = 0;
        self.phase = 0;
        self.phase_since = now;
    }

    /// On to the next stage of a step.
    fn advance(&mut self, now: f32) {
        self.phase += 1;
        self.phase_since = now;
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
    replies: Vec<String>,
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

/// What ends the session early: leaving the game, the observer cut off, or time running
/// out.
fn guard(world: &mut World, gate: &mut Gate, now: f32, screen: Screen) -> Result<(), String> {
    if !matches!(gate.step, Step::Load | Step::Leave) && screen != Screen::InGame {
        let notice = world.resource::<Notice>().0.clone().unwrap_or_default();
        return Err(format!("left the game for {screen:?}: {notice}"));
    }
    if now > TIME_LIMIT {
        return Err(format!("still at {:?} after {TIME_LIMIT}s", gate.step));
    }
    let limit = if gate.step == Step::Load { LOAD_LIMIT } else { STEP_LIMIT };
    if now - gate.since > limit {
        return Err(format!("{:?} took longer than {limit}s", gate.step));
    }
    if let Some(observer) = &mut gate.observer
        && gate.step != Step::Leave
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
            if loaded(world) {
                gate.check("load", Ok(format!("the world loaded in {now:.1}s")));
                land(world, gate);
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
            if count >= wanted || since > 8.0 {
                let picked = count - gate.count_before;
                let result = if count >= wanted {
                    Ok(format!("walking into the hole picked up {picked} items"))
                } else {
                    Err(format!("{picked} of {} dropped items picked up", gate.dug.len()))
                };
                gate.check("drops", result);
                key(world, KeyCode::KeyW, false);
                let line = format!("tp {} {} {}", feet.x as f32 + 4.5, feet.y - 2, feet.z as f32 + 0.5);
                world.resource::<Net>().command(&line);
                gate.next(Step::Swim, now);
            }
        }
        Step::Swim => swim(world, gate, now, in_phase),
        Step::Screens => screens(world, gate, now, in_phase),
        Step::Weather => weather(world, gate, now, in_phase),
        Step::Tree => tree(world, gate, now, in_phase),
        Step::Sky => sky(world, gate, now, in_phase),
        Step::Gorge => gorge(world, gate, now, in_phase),
        Step::Leave => {
            if gate.phase == 0 {
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
                gate.next(Step::Done, now);
            }
        }
        Step::Done => {}
    }
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

/// Esc opens the menu and closes it again; E the inventory.
fn screens(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let ui = world.resource::<UiState>();
    let (menu, inventory, locked) = (ui.menu_open, ui.inventory_open, ui.cursor_locked);
    match gate.phase {
        0 => {
            look(world, 0.0, 5.0);
            if gate.shot(world, "game").is_some() {
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
                key(world, KeyCode::Escape, true);
                gate.advance(now);
            } else if in_phase > 3.0 {
                gate.check("menu", Err("Esc did not open the menu".into()));
                gate.next(Step::Weather, now);
            }
        }
        2 => {
            key(world, KeyCode::Escape, false);
            if !menu && locked {
                gate.check("menu: close", Ok("Esc closed it and gave the game the mouse back".into()));
                key(world, KeyCode::KeyE, true);
                gate.advance(now);
            } else if in_phase > 3.0 {
                gate.check("menu: close", Err(format!("after Esc the menu is {} and the mouse {}", if menu { "open" } else { "closed" }, if locked { "grabbed" } else { "free" })));
                gate.next(Step::Weather, now);
            }
        }
        3 => {
            key(world, KeyCode::KeyE, false);
            if inventory && in_phase > 0.2 && let Some(shot) = gate.shot(world, "inventory") {
                let change = gate.earlier_shot("game").map_or(0.0, |game| change(&game, &shot, 0.0));
                let shown = if change >= SCREEN_CHANGE { Ok(format!("drawn, changing the screen by {change:.1} levels")) } else { Err(format!("changed the screen by only {change:.1} levels")) };
                gate.check("inventory", shown);
                key(world, KeyCode::KeyE, true);
                gate.advance(now);
            } else if in_phase > 3.0 {
                gate.check("inventory", Err("E did not open the inventory".into()));
                gate.next(Step::Weather, now);
            }
        }
        4 => {
            key(world, KeyCode::KeyE, false);
            if !inventory && locked {
                gate.check("inventory: close", Ok("E closed it".into()));
                gate.next(Step::Weather, now);
            } else if in_phase > 3.0 {
                gate.check("inventory: close", Err("E did not close the inventory".into()));
                gate.next(Step::Weather, now);
            }
        }
        _ => {}
    }
}

/// A storm, then snow, each drawn (rain, snow and their particles change the view), then
/// clear skies again.
fn weather(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let shown = world.resource::<crate::weather::ClientWeather>().current.clone();
    let kinds: [(&str, &'static str, fn(&voxmar_engine::weather::Weather) -> bool); 2] =
        [("storm", "storm", |w| w.precipitation > 0.3 && w.snow < 0.5), ("snow", "snow", |w| w.precipitation > 0.3 && w.snow > 0.5)];
    match gate.phase {
        0 => {
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
                if in_phase > 8.0 {
                    gate.check(&format!("weather: {kind}"), Err(format!("it did not come: {}", shown.summary())));
                    world.resource::<Net>().command("weather clear 600");
                    gate.phase = 3;
                }
                return;
            }
            // Let it fill the air before the shot.
            if in_phase < 1.0 {
                return;
            }
            let Some(shot) = gate.shot(world, name) else { return };
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
        3 if shown.precipitation < 0.05 && shown.cover < 0.2 => gate.next(Step::Sky, now),
        3 if in_phase > 8.0 => {
            gate.check("weather: clear", Err(format!("the sky did not clear: {}", shown.summary())));
            gate.next(Step::Sky, now);
        }
        _ => {}
    }
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
            gate.next(Step::Gorge, now);
        }
        _ => {}
    }
}

/// The sun low in the morning sky, a bright disc in a sky that darkens upwards; at night
/// the moon, opposite the sun, and stars around it.
fn sky(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    // Morning (ticks after sunrise), sunset with the sun a few degrees up, and midnight.
    const MORNING: u64 = 1920;
    const SUNSET: u64 = 11_830;
    const MIDNIGHT: u64 = 18_000;
    let ticks = world.resource::<crate::render::DayCycle>().ticks.rem_euclid(voxmar_protocol::TICKS_PER_DAY as f64);
    let at = |time: u64| (ticks - time as f64).abs() < 100.0;
    match gate.phase {
        0 => {
            world.resource::<Net>().command(&format!("time set {MORNING}"));
            gate.advance(now);
        }
        1 if at(MORNING) && in_phase > 0.5 => {
            aim_at_light(world);
            gate.advance(now);
        }
        2 if in_phase > 0.3 => {
            let Some(shot) = gate.shot(world, "sun") else { return };
            let at = light_on_screen(world);
            gate.check("sky: sun", disc(&shot, at, "sun"));
            gate.check("sky: day", gradient(&shot));
            // Up above the top of the world, as a flying player can go.
            let mut player = world.resource_mut::<LocalPlayer>();
            player.flying = true;
            player.position.y = ABOVE_THE_WORLD;
            player.pitch = 0.2;
            gate.advance(now);
        }
        3 if in_phase > 0.5 => {
            let Some(shot) = gate.shot(world, "above the world") else { return };
            gate.check("sky: above the world", sky_lit(&shot));
            let mut player = world.resource_mut::<LocalPlayer>();
            player.position.y = gate.feet.y as f32;
            player.flying = false;
            world.resource::<Net>().command(&format!("time set {SUNSET}"));
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
            gate.next(Step::Tree, now);
        }
        1 | 4 | 7 if in_phase > 5.0 => {
            gate.check("sky", Err(format!("the time of day did not change: {ticks:.0} ticks")));
            gate.next(Step::Tree, now);
        }
        _ => {}
    }
}

/// How far `/locate gorge` looks from the test ground, in blocks.
const GORGE_RADIUS: i32 = 3000;
/// How high a gorge's walls must stand over its water on both sides, within a few blocks
/// of it.
const GORGE_WALLS: i32 = 6;

/// `/locate gorge` (asked by the observer, which reads the answer) names the nearest river
/// running deep in a slot gorge; the player goes there by day, and the server's world has
/// water there with ground standing at least `GORGE_WALLS` over it on both sides, 3 to 8
/// blocks off across it, straight or diagonally.
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
            let Some(reply) = gate.observer.as_ref().and_then(|o| o.replies.iter().find(|r| r.contains("gorge")).cloned()) else {
                if in_phase > 20.0 {
                    gate.check("gorge", Err("no answer to /locate gorge".into()));
                    gate.next(Step::Leave, now);
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
                gate.next(Step::Leave, now);
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
            let there = world.resource::<LocalPlayer>().position.xz().distance(at.xz().as_vec2()) < 4.0;
            let client = world.resource::<ClientWorld>();
            let shown = columns_around(at).into_iter().all(|c| client.voxels.contains(c)) && settled(world, at);
            if !(held && there && shown && in_phase > 1.0) {
                if in_phase > 25.0 {
                    gate.check("gorge", Err(format!("the world around the gorge at {at} did not load")));
                    gate.next(Step::Leave, now);
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
            gate.next(Step::Leave, now);
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
    (server && shown(&|p| client.block(p)) && settled(world, gate.feet)) || since > SETTLE_LIMIT
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
        let mut differ = Vec::new();
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    let p = gate.feet + IVec3::new(dx, dy, dz);
                    if let (Some(ours), Some(theirs)) = (client.block(p), observer.world.block(p))
                        && ours != theirs
                    {
                        differ.push(format!("{p}: client {}, server {}", named(Some(ours)), named(Some(theirs))));
                    }
                }
            }
        }
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
