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
//! (`SERVER_LAG`, in the steps that play against it: `Step::lagged`). Mariano kept losing blocks he broke on a busy machine; client and server
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
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::lang::Words;
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
/// It runs behind only in the steps that play against it (`Step::lagged`).
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
/// Longest the whole session may take by the wall clock, whatever the game's and the
/// server's clocks say: the backstop for a client too slow for game time to keep up (a
/// frame counts at most 250 ms of it) and for a server that hangs.
const WALL_LIMIT: f32 = 900.0;
/// How long the server's ticks may stand still, in seconds of the wall clock, before the
/// session fails as the server stopped: waits in the world count the server's seconds and
/// would wait for it until `TIME_LIMIT`.
const SERVER_SILENT: f32 = 60.0;
/// A frame longer than this, in seconds, is the process stopped (memcap stops our runs
/// while Mariano plays), not a slow session: it counts only this much of the wall clock.
const SUSPENDED: f32 = 10.0;
/// Longest a single step may take.
const STEP_LIMIT: f32 = 30.0;
/// Longest the strike step may take: a server running behind takes blows slowly, and the
/// step fails sooner on its own if blows stop being answered.
const STRIKE_LIMIT: f32 = 90.0;
/// Seconds without the server answering a blow, and blows answered without a kill, that
/// fail the strike step: a creature with 4 health dies within a few hits at any level.
const UNANSWERED: f32 = 12.0;
/// Strikes asked with none answered, at least, before a striking step says the server
/// answered none (`Gate::unanswered`), and clicks before a phase says it waited one out
/// (`Gate::waited_out`): a client starved of frames sends few in those seconds, and a
/// server that answered no blow was not shown one.
const UNANSWERED_SWINGS: u32 = 3;
/// Game seconds between a phase's right clicks (`Gate::tap`).
const TAP_EVERY: f32 = 0.5;
const MOST_BLOWS: u32 = 40;
/// How long an edit is given at least (seconds of game time), and at most (of the
/// server's), to reach the server, come back and be meshed; the checks run as soon as both
/// sides show it.
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
            .init_resource::<UiClicks>()
            .insert_resource(crate::creatures::InspectEyes)
            .insert_resource(crate::music::Rehearse)
            .add_systems(PostStartup, render_offscreen)
            .add_systems(PreUpdate, ui_clicks.after(bevy::ui::UiSystems::Focus))
            .add_systems(PreUpdate, feel_press.after(crate::net::receive).before(bevy::input::InputSystems))
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

/// Pointer input for headless UI steps. Delivered after focus (there is no window
/// cursor), then consumed by the normal Update systems; the gate never calls a handler.
#[derive(Resource, Default)]
struct UiClicks {
    next: Option<(Entity, MouseButton)>,
    hover: Option<Entity>,
    active: Vec<Entity>,
    button: Option<MouseButton>,
}

fn ui_clicks(world: &mut World) {
    world.resource_scope(|world, mut clicks: Mut<UiClicks>| {
        if let Some(button) = clicks.button.take() {
            world.resource_mut::<ButtonInput<MouseButton>>().release(button);
        }
        for entity in clicks.active.drain(..) {
            if let Some(mut cursor) = world.get_mut::<bevy::ui::RelativeCursorPosition>(entity) {
                *cursor = default();
            }
            if let Some(mut interaction) = world.get_mut::<Interaction>(entity) {
                *interaction = Interaction::None;
            }
        }
        let Some((entity, button)) = clicks.next.take().map(|(entity, button)| (entity, Some(button))).or_else(|| clicks.hover.take().map(|entity| (entity, None))) else { return };
        if let Some(button) = button {
            world.resource_mut::<ButtonInput<MouseButton>>().press(button);
        }
        if button == Some(MouseButton::Left)
            && let Some(mut interaction) = world.get_mut::<Interaction>(entity)
        {
            *interaction = Interaction::Pressed;
        }
        // The pointer is over the cell and its parents, so clicking a search result is
        // also inside the inventory card rather than an outside click that drops items.
        let mut over = Some(entity);
        while let Some(entity) = over {
            if let Some(mut cursor) = world.get_mut::<bevy::ui::RelativeCursorPosition>(entity) {
                *cursor = bevy::ui::RelativeCursorPosition { cursor_over: true, normalized: Some(Vec2::ZERO) };
            }
            clicks.active.push(entity);
            over = world.get::<ChildOf>(entity).map(ChildOf::parent);
        }
        clicks.button = button;
    });
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
    /// The test ground is built, the clock, weather and chance wandering parties held, and
    /// the observer joins.
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
    /// The full-only HV factory, locked chests, picks and their bilingual evidence.
    HvAge,
    /// Swimming against the pool's bank and holding jump climbs out onto the block on it.
    Swim,
    /// Esc opens and closes the menu, E the inventory.
    Screens,
    /// On the menu's audio page, a switch for each category of music the player can hear;
    /// clicking one turns that category off, and again on. A track starting is named, with
    /// its category, at the top right, and the line fades.
    Music,
    /// F3 opens the debug overlay, and PostUpdate (UI layout among it) takes no longer with
    /// it open than closed.
    Overlay,
    /// A storm and a snowfall come, drawn with their particles, then clear again.
    Weather,
    /// Mist shows in the open and is gone in a shaft dug down; a crack on a block in a
    /// sealed dark room is lit by what lights that face, and by a glowstone beside it.
    Depths,
    /// The sun by day, the moon and stars by night.
    Sky,
    /// A small tree with an apple among its leaves, drawn with its leaf cards and fruit.
    Tree,
    /// Blocks drawn as shapes (a slab, stairs both ways up, a barrel, a crate and a sack)
    /// reach the client and draw.
    Shapes,
    /// The item in the hand is drawn in first person: an iron sword, a stone block and an
    /// empty hand each give a different view, only in the lower right; and sealed in a
    /// stone box with no light the sword is far darker than under the open sky.
    Held,
    /// Natural births arrive out of view; a torch-lit pad rejects monsters.
    NaturalSpawns,
    /// Creatures summoned on a pad beside the test ground reach the client and draw.
    Creatures,
    /// Struck with left clicks of the empty hand until it dies, a lumen snail drops its
    /// slime, which walking over picks up. The first punch, watched in the hand on the
    /// gate's clock, peaks as the server judges its blow (the fist's wind-up), comes back to
    /// rest, is answered and is drawn as the fist and arm; shot as it connects.
    Strike,
    /// An iron sword in hand at Attack 20, a brown bear struck until it dies takes harder
    /// blows than a fist could deal. The first swing, watched in the hand on the gate's
    /// clock, peaks as the server judges its blow (the sword's wind-up), comes back to rest
    /// and is answered; shot as it connects.
    Weapon,
    /// A sling at Ranged 70 against a held brown bear seven blocks off, out of a sword's
    /// reach: shot with left clicks until a stone hits, each shot spends one cobblestone,
    /// and the hit trains Ranged.
    Sling,
    /// The bear again, in survival, provoked into fighting back: the guard raised just as
    /// its wind-up ends is a perfect block that takes nothing, a click then is a counter,
    /// and a dodge as the next blow comes down lets it pass.
    Feel,
    /// The Rootmother summoned on the pad reaches the client and draws; the music counts it a
    /// boss fight and plays a boss's track; in survival she winds up a move of her own, its
    /// tell shown. Every exit sweeps her away.
    Boss,
    /// A torch held a moment leaves the player Smoky (smoke that clings): its chip shows,
    /// timed; then the hand is emptied.
    Smoke,
    /// A chainmail shirt and a wooden shield in hand, each right click puts one on in its
    /// own worn slot, and with the shield on the guard is the shield's; the inventory
    /// shows both in its worn wells, and their tooltips the armour lines.
    Wear,
    /// Every blow traded so far got its hitsplat, and the meter showed; then a run of
    /// blows of every kind and outcome, told to this side as the server would, draws its
    /// splats in their colours over a creature and over the hotbar, with the pips all lit
    /// and the health bar flashing for the hit (shot in English and Japanese); with damage
    /// numbers off the same run draws none; and with the guard on a toggle, one right click
    /// raises it and the next lowers it.
    CombatHud,
    /// How blows feel beyond their marks (`fight_fx`): a flourish of the player's at the
    /// creature marked, and a hit, a block and a perfect block at the player, told to this
    /// side as the server would: the creature freezes, flashes and flinches with sparks
    /// flying, the swing holds still and the hand is knocked back, the view dips and tips
    /// and the screen's edges redden, and each asks for its sounds (a silent run plays
    /// none). The fights before saw creatures coil and strike, and the fallen go up in
    /// smoke. Shot mid-flinch.
    CombatFeel,
    /// How creatures move (`creature_motion`): a deer this side alone walks across the
    /// view, its pose driven as the server's word would drive it, and its feet stay where
    /// they are put while they are down (they slide less than `MOST_SLIP` of the ground it
    /// goes).
    CreatureWalk,
    /// The deer runs straight away from the player: its feet still stay put, and it shows
    /// it is fleeing. Shot mid-run.
    CreatureRun,
    /// The deer, standing, is told to face a quarter turn round: it turns no faster than its
    /// kind may and ends facing that way.
    CreatureTurn,
    /// Raw rabbit used on a campfire comes back cooked (or burnt) into the inventory, and
    /// smoke and embers rise from the fire.
    Cook,
    /// A drying rack stood by the campfire, raw rabbit in hand: a right click hangs it, the
    /// rack the server sends back is the hung one, and chat says where it stands makes it quicker.
    /// Then an earth clamp in its place, clay in hand: a right click sets it smouldering.
    Dry,
    /// A snare set by the trail with a rabbit summoned into it springs (the server's word:
    /// the sprung block); a right click takes the catch, raw rabbit to hand, and the snare
    /// is set again.
    Snare,
    /// A junglefowl fed wheat by right clicks comes to trust the player and is theirs.
    Tame,
    /// `/wander start tinker_camp` brings a travelling party: its trader and hound reach the
    /// client and draw; a right click on the trader gets a greeting in chat; four hides held
    /// out are swapped for loaves (the party's trade, taken from the hand); `/wander end`
    /// sends them away.
    Party,
    /// `/locate ore` finds a sample of ore on the ground, and the server's world there has
    /// it, with its vein's ore below it.
    Ore,
    /// `/locate gorge` finds a river deep between walls, and the server's world there has
    /// them.
    Gorge,
    /// A pool generated by the highland tarn rule reaches both peers and draws as still water.
    Tarns,
    /// `/locate settlement` finds a hamlet or village, and the server's world there has
    /// what its plan says: a well on its square, doors open onto a floor, and houses dry.
    Settlements,
    /// Over a log in the inventory, the uses key shows what it goes into (planks, a
    /// campfire); clicking planks shows their recipe, and Back returns.
    Recipes,
    /// A chopping block set by the fire draws; with the inventory open the client knows it
    /// is within reach, so a log in the grid shows six planks, not four, and crafted, the
    /// server gives six.
    Chop,
    /// A survival player's inventory search finds the stone pickaxe by "pick"; its recipe
    /// opened in the viewer finishes the "Look It Up" quest; the quest book's page for the
    /// pickaxe shows its recipe inline, an item clicked there opens the viewer, and Back
    /// returns to the book.
    LookUp,
    /// Rice sown on the test ground, too cold for it, is refused by the server for that
    /// reason, and the refusal shows by the hotbar, as a level the player lacks does (the
    /// server sends it as a refusal, not a console line). Then a well from the hand goes
    /// down by the plot.
    Refusal,
    /// A wild plant whose item merged (oregano into kitchen herbs) is given as the merged
    /// item, and the catalogues (creative, journal, recipe book) list the item, not the plant.
    Merged,
    /// A player saved under one spelling rejoins with another case and keeps their
    /// canonical identity in the server's replicated player list.
    Identity,
    /// A timed effect's chip counts its time down, and its hover plate says how long is left.
    Effects,
    /// An effect that comes on pops up with its name, and the first look at the inventory
    /// brings its tip.
    Tips,
    /// A tenth level reached pops its banner up at the top of the screen, saying it is a
    /// milestone, the skill and the level, and throws motes around the player; a quest
    /// finished with a level shows first, the level cutting in after the quest's turn.
    Celebrate,
    /// The name of what the crosshair rests on shows at the top left: a block close by, a
    /// creature further than a blow reaches, and not through a wall.
    Look,
    /// The Farming guide sorts its lines into kinds down the left; Planting shows only
    /// planting, and a line opens its page.
    Guides,
    /// The journal key opens the journal, whose Plants page lists what there is to find.
    Journal,
    /// C opens the character screen and its Prayers button the prayer book, which has a row for
    /// every prayer; a click on Bark Skin turns it on as the server says, the strip over the
    /// hotbar names it once the screen is shut, and its quick key turns it off again.
    Prayers,
    /// A sleeping bag with a torch beside it, slept in, is a camp: the Camp chip comes on, the
    /// chat says so with what to add next, and the sleep is a rest. At home, Lay on a
    /// recipe fills the crafting grid from a barrel there.
    Home,
    /// The way home at dusk: past the home's box, the hearth mark sits within a few degrees
    /// of where home lies; back inside the home, it is gone.
    WayHome,
    /// A hen kept at that home lays into a barrel there, not onto the ground, and the chat
    /// says so.
    Nest,
    /// A wooden door placed before the player goes in whole, both halves facing them, and a
    /// right click on it opens it and another shuts it, as the server turns it.
    Door,
    /// With a log in the pack, the inventory's Can Make list shows planks ready, the ready
    /// ones first; a click on them lays the log in the grid, as the server does it; pointed
    /// at, a stone pickaxe says what it still wants.
    Makeable,
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

impl Step {
    /// Whether the server runs behind (`SERVER_LAG`) in this step: in those whose checks are
    /// about playing against a lagging server (placing, digging and breaking, where blocks
    /// were lost to it, and the fight's guard and dodge, timed against where its blows fall)
    /// and the steps before them. The rest check what lag does not change (what crafting,
    /// a kill or a campfire gives), and run against a server at full speed, which gets
    /// there three times sooner.
    fn lagged(self) -> bool {
        use Step::*;
        matches!(self, Load | Build | Arrive | Place | PlaceSettle | Dig | DigSettle | Collect | Breaks | Feel)
    }
}

impl Gate {
    /// Whether the server runs behind now: in a lagged step, but in the fight-feel step only
    /// from the bear's coming (`FEEL_LAG_SETTLE`) through its tries (the block, counter and
    /// dodge), not while it is summoned, nor while the bears are swept away after.
    fn lagging(&self) -> bool {
        self.step.lagged() && (self.step != Step::Feel || (FEEL_COMING..=FEEL_DODGE).contains(&self.phase))
    }

    /// How many times the game's pace the server ticks now: the workshop's machine stages
    /// (the boiler and crusher on to the iron age) wait on their recipes' server time, a
    /// bloom and its dust taking half a minute of it, so they run at `MACHINES_HURRIED`.
    /// Every wait in them is by the server's clock, so they prove the same sooner.
    fn hurried(&self) -> u32 {
        if self.step == Step::Workshop && (16..=19).contains(&self.phase) { MACHINES_HURRIED } else { 1 }
    }
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
    /// Frames in this stage that what a shot is about has been on screen: it shows a frame
    /// or two after the change.
    settled: u32,
    /// Where the player stands on the test ground.
    feet: IVec3,
    observer: Option<Observer>,
    identity_peer: Option<Observer>,
    identity_id: Option<PlayerId>,
    /// Shots asked for, and those that arrived.
    requested: Vec<&'static str>,
    shots: Arc<Mutex<FxHashMap<&'static str, Image>>>,
    /// Times each shot was forgotten: one asked for before its latest forgetting is
    /// dropped when it arrives, so a retake never keeps an image of the scene it replaces.
    forgotten: Arc<Mutex<FxHashMap<&'static str, u32>>>,
    /// Shots still being written to disk, each on its own thread so encoding the PNG never
    /// stalls a frame; `finish` waits for them all.
    saving: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
    placed: Vec<IVec3>,
    /// The block being dug, and those dug with what they were.
    digging: Option<(IVec3, BlockId)>,
    dug: Vec<(IVec3, BlockId)>,
    /// How many of an item the inventory held before a step that changes it.
    count_before: u32,
    /// The time of day before the way home step set dusk, set back after it.
    way_time: u64,
    /// The window step's lit pane, kept to judge against it snuffed.
    window_lit: Option<[f64; 3]>,
    natural_before: Vec<u32>,
    natural_home: Vec3,
    /// The way home's numbers, while the Home step shows the mark close up at any hour.
    home_style: Option<crate::hud::inventory::InterfaceStyle>,
    /// Puffs the watched chimney owed so far (its rate times the seconds watched).
    chimney_owed: f32,
    /// What the dry step's pot water replaced, put back after.
    pot_ground: Option<String>,
    /// The creature the combat-marks step tells its blows at, when (game time) it last told
    /// them, and how many runs it has told.
    marked: Option<u32>,
    marked_at: f32,
    marked_tries: u32,
    /// Where the feet stood when W went down with the quest book open.
    quest_feet: Vec3,
    /// The quests' revision when torches were crafted for the tracker.
    quests_rev: u32,
    /// When a quest already done was pinned, to time how long it stays.
    quest_pinned: f32,
    /// Where the quest book's web stood scrolled when a drag of it began, and where its
    /// canvas stood in the window.
    web_scroll: Vec2,
    web_canvas: Vec2,
    /// The quest a lane's header was pressed to bring into view.
    web_head: Option<String>,
    /// Where the player died in the workshop, the most items carried since waking, and when
    /// (in server seconds) that last went up.
    fell: Vec3,
    gathered: (u32, f32),
    /// The crucible of the kiln the workshop builds and lights, and when (server seconds)
    /// it was first lit.
    kiln: IVec3,
    kiln_since: f32,
    /// Where the shapes step sets its row of shaped blocks: in the air before the player.
    shapes_row: IVec3,
    /// Times the held step sent wild creatures away and began its shots of the sky again.
    held_cleared: u32,
    /// The weather step's snow left for the depths step to clear with its fog: the sky's
    /// clearing is judged once, as the fog lifts at its end (`depths`).
    weather_clear_owed: bool,
    /// When (server seconds) the kiln was last asked to light.
    kiln_asked: f32,
    /// When the lit kiln's bronze smelt began, in server and game seconds (0 before).
    bronze_since: (f32, f32),
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
    /// Teleport sequence before returning weather/sky views to the open test ground.
    scene_teleport: u32,
    /// The server's clock when `open_scene_pose` last sent its teleport.
    scene_sent: f32,
    /// Where `/locate gorge` sent the player: the column, one block above its water.
    gorge: IVec3,
    /// Whether the player has arrived there: the river's current may carry them off while
    /// the world around loads, which says nothing about the world.
    gorge_reached: bool,
    /// The tarn visit and the two peers' positions before it, for returning to the gorge
    /// before the settlement lookup uses the observer's position.
    tarn: Option<TarnVisit>,
    /// Where `/locate ore` found a sample of ore.
    ore: IVec3,
    /// Where `/locate settlement` sent the player: the middle of its square.
    settlement: IVec3,
    /// What `/locate settlement` answered, English, for the culture it names.
    settlement_reply: String,
    /// Where the struck creature was last seen, for walking over its loot.
    quarry: Vec3,
    /// The snail struck, so a fallen mark on another creature cannot end its watch.
    quarry_id: Option<u32>,
    fallen_watched: bool,
    weapon_fallen_watched: bool,
    /// Where the player was put to stand on the struck creature's loot, once it lay.
    loot_stood: Option<Vec3>,
    fallen_since: Option<f32>,
    weapon_fallen_since: Option<f32>,
    /// Blows the server has answered, as last counted.
    blows_seen: u32,
    /// The striking steps' swings (`Gate::swing`): when the last went (game time), whether
    /// it is held this frame, and how many went since the server last answered one.
    swing_at: f32,
    swung: bool,
    swings: u32,
    /// The striking steps' strikes asked of the server (`Fighting::strikes`) as counted at
    /// its last answer, `None` until the step first looks; and the server's time when the
    /// first strike since then was seen gone (`Gate::unanswered`).
    strikes_seen: Option<u32>,
    first_unanswered: Option<f32>,
    /// The right clicks of a phase (`Gate::tap`): when the last went, whether it is held
    /// this frame, and how many went in the phase.
    tap_at: f32,
    tapped: bool,
    taps: u32,
    /// The sling step's run (`sling`).
    sling: SlingRun,
    /// The Rootmothers seen before the Boss step summoned its own, and the one it summoned:
    /// another (a lair's, woken near the test ground) is never the one judged.
    boss_before: Vec<u32>,
    boss_id: Option<u32>,
    /// Whether the target readout has shown the struck creature's health.
    readout_seen: bool,
    /// Whether the creatures step could release its butterfly.
    butterfly: bool,
    /// Whether the struck creature's fall was judged.
    fall_judged: bool,
    /// Whether the loading cover was checked.
    cover_checked: bool,
    /// Texts found not to fit, each once per language: what, and where it was first seen.
    spills: Vec<(String, String)>,
    /// Views in a language with line rules where no text had its line breaks checked.
    unbroken: Vec<String>,
    /// The text-fit tour's view (its step and stage), its layout as last drawn, whether that
    /// changed since the view was shown, and the frames it has held still since (`FIT_STILL`).
    fit_layout: ((Step, u32), u64, bool, u32),
    /// The interface font before the gate tried another, to go back to.
    font: Option<crate::ui::skin::InterfaceFont>,
    /// Whether the taming food was moved into the hotbar, which earlier steps may have filled.
    food_moved: bool,
    /// While taming, frames the junglefowl was under the crosshair, another creature was,
    /// or none was, and the nearest the player came to it.
    tame_aim: [u32; 3],
    tame_nearest: f32,
    /// The newest creature id when the party step began: what is newer is the party's.
    party_after: u32,
    /// Whether this stage of the party step has sent its first command (the tp to the pad, or
    /// /wander end): sent on the stage's first frame however long that frame took.
    party_sent: bool,
    /// The game clock when the party step gave its hides: the trader's swap line is looked
    /// for from then on.
    party_given: f32,
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
    /// The machine's one-minute load average read about once a second, with the session's
    /// real seconds at each read, over the last `LOAD_SPAN` (`Gate::load_now`).
    loads: std::collections::VecDeque<(f32, f64)>,
    /// The server's clock (its ticks, in seconds) this frame, and when the step and the
    /// phase began by it.
    server_now: f32,
    step_server: f32,
    phase_server: f32,
    /// Seconds of the wall clock the session has run, each frame counted up to `SUSPENDED`;
    /// the server's ticks last seen, and the wall second they last changed; and whether the
    /// session ended because they stood still.
    wall: f32,
    ticks_seen: u64,
    ticks_changed: f32,
    server_stopped: bool,
    /// The step last frame, and the failures by then: a step that fails lets go of every key
    /// and button (a step that passes may hand one on).
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
    /// Where the fight-feel step stands (and the weapon step's bear).
    feel: FeelRun,
    /// Clearing the bears after the weapon and feel steps.
    sweep: Sweep,
    /// The weapon step's first swing, watched in the hand.
    swing_watch: SwingWatch,
    /// When the music step checked the now-playing line, while it is watched fading out
    /// beside the steps after it ([`music_fades`]).
    music_fade: Option<f32>,
    /// The player's states the server had sent when the wear step clicked.
    worn_states: u32,
    /// The Defence level before the wear step raised it, to put back after.
    worn_defence: u8,
    overlay_counted: (f32, f32),
    /// The fight sounds asked for, and the bursts, kicks and knocks, before `combat_feel`
    /// told its blows.
    feel_asked: std::collections::BTreeMap<String, u32>,
    feel_counts: (u32, u32, u32),
    /// The deer walked by the CreatureWalk, CreatureRun and CreatureTurn steps: where it
    /// is, frames into its stage, and whether its shot is taken.
    puppet: (Vec3, u32, bool),
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
            settled: 0,
            feet: IVec3::ZERO,
            observer: None,
            identity_peer: None,
            identity_id: None,
            requested: Vec::new(),
            shots: default(),
            forgotten: default(),
            saving: default(),
            placed: Vec::new(),
            digging: None,
            dug: Vec::new(),
            count_before: 0,
            way_time: 0,
            window_lit: None,
            natural_before: Vec::new(),
            natural_home: Vec3::ZERO,
            home_style: None,
            chimney_owed: 0.0,
            pot_ground: None,
            marked: None,
            marked_at: 0.0,
            marked_tries: 0,
            quest_feet: Vec3::ZERO,
            quests_rev: 0,
            quest_pinned: 0.0,
            web_scroll: Vec2::ZERO,
            web_canvas: Vec2::ZERO,
            web_head: None,
            fell: Vec3::ZERO,
            gathered: (0, 0.0),
            kiln: IVec3::ZERO,
            kiln_since: 0.0,
            shapes_row: IVec3::ZERO,
            held_cleared: 0,
            weather_clear_owed: false,
            kiln_asked: f32::MIN,
            bronze_since: (0.0, 0.0),
            machines_stage: 0,
            machines_since: 0.0,
            machines_asked: f32::MIN,
            machines_seen: false,
            machines_held: f32::MIN,
            highest: f32::MIN,
            scene_teleport: 0,
            scene_sent: 0.0,
            gorge: IVec3::ZERO,
            gorge_reached: false,
            tarn: None,
            settlement: IVec3::ZERO,
            settlement_reply: String::new(),
            ore: IVec3::ZERO,
            quarry: Vec3::ZERO,
            quarry_id: None,
            fallen_watched: false,
            weapon_fallen_watched: false,
            loot_stood: None,
            fallen_since: None,
            weapon_fallen_since: None,
            blows_seen: 0,
            swing_at: f32::MIN,
            swung: false,
            swings: 0,
            strikes_seen: None,
            first_unanswered: None,
            tap_at: f32::MIN,
            tapped: false,
            taps: 0,
            sling: SlingRun::default(),
            boss_before: Vec::new(),
            boss_id: None,
            readout_seen: false,
            butterfly: false,
            fall_judged: false,
            cover_checked: false,
            spills: Vec::new(),
            unbroken: Vec::new(),
            fit_layout: ((Step::Load, 0), 0, false, 0),
            font: None,
            food_moved: false,
            tame_aim: [0; 3],
            tame_nearest: f32::MAX,
            party_after: 0,
            party_sent: false,
            party_given: 0.0,
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
            loads: default(),
            overlay_ms: default(),
            server_now: 0.0,
            step_server: 0.0,
            phase_server: 0.0,
            wall: 0.0,
            ticks_seen: 0,
            ticks_changed: 0.0,
            server_stopped: false,
            last_step: Step::Load,
            failures_then: 0,
            health_last: 0,
            death_seen: false,
            killed_on_purpose: false,
            overlay_changes: 0,
            feel: FeelRun::default(),
            sweep: Sweep::default(),
            swing_watch: SwingWatch::default(),
            music_fade: None,
            worn_states: 0,
            worn_defence: 1,
            overlay_counted: (0.0, 0.0),
            feel_asked: Default::default(),
            feel_counts: (0, 0, 0),
            puppet: (Vec3::ZERO, 0, false),
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

    /// A well-timed click, a full charge after the last (spamming lands lighter blows:
    /// `combat`), as a player swings: pressed for one frame, let go the next, however long
    /// the frames (a swing timed as a slice of each charge was never pressed by frames
    /// longer than the slice). Counted in `swings` until the server answers one; whether it
    /// answered none is judged by the strikes the presses asked of it (`Gate::unanswered`).
    fn swing(&mut self, world: &mut World, now: f32) {
        if self.swung {
            mouse(world, MouseButton::Left, false);
            self.swung = false;
        } else if now - self.swing_at >= voxmar_engine::combat::FULL_CHARGE + 0.05 {
            mouse(world, MouseButton::Left, true);
            self.swung = true;
            self.swing_at = now;
            self.swings += 1;
        }
    }

    /// A right click held out to what is aimed at, `on` while it is wanted: pressed for one
    /// frame each half second of game time and let go the next, counted in `taps` for the
    /// phase. Pressed as a slice of each half second, it went unpressed by frames longer
    /// than the slice, while the step's server seconds ran out.
    fn tap(&mut self, world: &mut World, on: bool, now: f32) {
        if self.tapped || !on {
            mouse(world, MouseButton::Right, false);
            self.tapped = false;
        } else if now - self.tap_at >= TAP_EVERY {
            mouse(world, MouseButton::Right, true);
            self.tapped = true;
            self.tap_at = now;
            self.taps += 1;
        }
    }

    /// A left click on the screen's `cell`, held out like `tap`: given to the UI once each
    /// half second of game time and counted in `taps`, so `waited_out` gives up only on a
    /// server that was asked.
    fn click(&mut self, world: &mut World, cell: Entity, now: f32) {
        if now - self.tap_at >= TAP_EVERY {
            world.resource_mut::<UiClicks>().next = Some((cell, MouseButton::Left));
            self.tap_at = now;
            self.taps += 1;
        }
    }

    /// Whether a phase that clicks (`tap`) has waited out `seconds` of the server's clock
    /// with at least `UNANSWERED_SWINGS` clicks sent: a client starved of frames that sent
    /// fewer has not been answered no, only not asked.
    fn waited_out(&self, seconds: f32) -> bool {
        self.server_in_phase() > seconds && self.taps >= UNANSWERED_SWINGS
    }

    /// Whether the server has answered none of the strikes of a striking step: at least
    /// `UNANSWERED_SWINGS` asked of it (`Fighting::strikes`, of `strikes` so far) since its
    /// last answer, and `UNANSWERED` of its seconds since the first of them went. A press
    /// asks nothing when the client sees nothing in reach (a starved client's view of a
    /// moving creature runs behind), and a server shown no strike has answered none.
    fn unanswered(&mut self, strikes: u32) -> bool {
        let seen = *self.strikes_seen.get_or_insert(strikes);
        let asked = strikes.saturating_sub(seen);
        if asked > 0 && self.first_unanswered.is_none() {
            self.first_unanswered = Some(self.server_now);
        }
        asked >= UNANSWERED_SWINGS && self.first_unanswered.is_some_and(|first| self.server_now - first > UNANSWERED)
    }

    /// Strikes asked of the server and not yet answered, of `strikes` so far.
    fn unanswered_strikes(&self, strikes: u32) -> u32 {
        strikes.saturating_sub(self.strikes_seen.unwrap_or(strikes))
    }

    /// The server answered a strike: the next unanswered one is counted from here.
    fn strike_answered(&mut self, strikes: u32) {
        self.swings = 0;
        self.strikes_seen = Some(strikes);
        self.first_unanswered = None;
    }

    fn next(&mut self, step: Step, now: f32) {
        info!("gate: {step:?} at {now:.1}s, server {:.1}s", self.server_now);
        self.swings = 0;
        self.swung = false;
        self.strikes_seen = None;
        self.first_unanswered = None;
        self.taps = 0;
        self.step = step;
        self.since = now;
        self.frames = 0;
        self.phase = 0;
        self.phase_since = now;
        self.settled = 0;
        self.step_server = self.server_now;
        self.phase_server = self.server_now;
        self.food_moved = false;
        self.party_sent = false;
    }

    /// On to the next stage of a step. Logged with the game's and the server's clocks, so a
    /// run shows which stages its time goes to.
    fn advance(&mut self, now: f32) {
        info!("gate: {:?} stage {} at {now:.1}s, server {:.1}s", self.step, self.phase + 1, self.server_now);
        self.phase += 1;
        self.phase_since = now;
        self.settled = 0;
        self.phase_server = self.server_now;
        self.taps = 0;
    }

    /// Seconds of the server's own clock since the phase began: what a wait on the server
    /// (an effect it gives, a blow it answers, an item, a block, a reply) gives up by, for
    /// a server running behind on a busy machine takes longer by the wall clock and the
    /// game's. Given up by the game's clock, such a wait failed before the server had run
    /// the ticks it was waiting on (the cook step's "cooked nothing" on a stalled server).
    /// Waits on the client's own work (drawing, a shot, a screen opening, movement it
    /// predicts) stay on the game's clock: the server has no part in them.
    fn server_in_phase(&self) -> f32 {
        self.server_now - self.phase_server
    }

    /// Seconds of the server's own clock since the step began.
    fn server_in_step(&self) -> f32 {
        self.server_now - self.step_server
    }

    /// The shot called `name` of what is drawn now, asking for it the first time; it
    /// arrives a few frames later.
    fn shot(&mut self, world: &mut World, name: &'static str) -> Option<Image> {
        if !self.requested.contains(&name) {
            self.requested.push(name);
            let shots = self.shots.clone();
            let forgotten = self.forgotten.clone();
            let round = forgotten.lock().expect("gate shots").get(name).copied();
            let (path, saving) = (self.dir.join(format!("{name}.png")), self.saving.clone());
            world
                .spawn(Screenshot::image(self.target.clone()))
                .observe(move |captured: On<ScreenshotCaptured>| {
                    let (image, path) = (captured.image.clone(), path.clone());
                    saving.lock().expect("gate saves").push(std::thread::spawn(move || save_shot(image, &path)));
                })
                .observe(move |captured: On<ScreenshotCaptured>| {
                    if forgotten.lock().expect("gate shots").get(name).copied() == round {
                        shots.lock().expect("gate shots").insert(name, captured.image.clone());
                    }
                });
        }
        self.shots.lock().expect("gate shots").get(name).cloned()
    }

    /// Forgets the shots called `names`, those still on their way included, so the next
    /// `shot` of each takes it again.
    fn forget(&mut self, names: &[&'static str]) {
        let mut forgotten = self.forgotten.lock().expect("gate shots");
        for &name in names {
            *forgotten.entry(name).or_default() += 1;
        }
        drop(forgotten);
        self.requested.retain(|n| !names.contains(n));
        self.shots.lock().expect("gate shots").retain(|n, _| !names.contains(n));
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
    names: FxHashMap<PlayerId, String>,
    /// The last teleport applied, which its inputs echo.
    teleports: u32,
    /// What the server answered the observer's commands.
    replies: Vec<voxmar_engine::lang::Words>,
    /// Heartbeats follow the protocol's tick rate, not the renderer: inputs ahead of a
    /// command otherwise fill the deliberately lagging server's per-player message budget.
    last_input: Option<Instant>,
}

impl Observer {
    fn join(net: &Net, content: u64) -> Option<Self> {
        Self::join_named(net, content, "gate-observer")
    }

    fn join_named(net: &Net, content: u64, name: &str) -> Option<Self> {
        let (connection, server_side) = local_pair();
        net.server()?.connections().send(server_side).ok()?;
        connection.send(ClientMessage::Hello { name: name.into(), protocol: PROTOCOL_VERSION, content, admin_token: None });
        connection.send(ClientMessage::Settings { view_distance: 2, fly_speed: 1.0 });
        Some(Self { connection, world: VoxelWorld::default(), id: None, position: None, moved: false, others: default(), names: default(), teleports: 0, replies: Vec::new(), last_input: None })
    }

    /// Applies what the server sent, and keeps the observer floating where it was put.
    fn pump(&mut self) -> Result<(), String> {
        self.pump_at(Instant::now())
    }

    fn pump_at(&mut self, now: Instant) -> Result<(), String> {
        let mut moved = false;
        loop {
            match self.connection.try_recv() {
                Ok(Some(message)) => match message {
                    ServerMessage::Welcome { player, position, .. } => {
                        self.id = Some(player);
                        self.position = Some(position);
                        moved = true;
                    }
                    ServerMessage::Teleport { position, seq } => {
                        self.position = Some(position);
                        self.teleports = seq;
                        moved = true;
                    }
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
                        self.names = players.iter().map(|p| (p.id, p.name.clone())).collect();
                        self.others = players.iter().filter(|p| Some(p.id) != self.id).map(|p| (p.id, p.position)).collect();
                    }
                    _ => {}
                },
                Ok(None) => break,
                Err(_) => return Err("the server dropped the observer".into()),
            }
        }
        let interval = Duration::from_nanos(1_000_000_000 / voxmar_protocol::TICKS_PER_SECOND as u64);
        if let Some(position) = self.position
            && (moved || self.last_input.is_none_or(|sent| now.saturating_duration_since(sent) >= interval))
        {
            let input = PlayerInput { position, yaw: 0.0, pitch: 0.0, selected_slot: 0, flying: true, teleport: self.teleports };
            self.connection.send(ClientMessage::Input(input));
            self.last_input = Some(now);
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
        // In the game, the server's seconds this client has taken in (`Net::taken_ticks`):
        // what a step waits on in them can only have come in that many. The server's own
        // count runs on while a frame draws, and on a starved machine (frames of four seconds
        // at load 180) passed a step's bound with the answer sent and still unread.
        let ticks = net.taken_ticks().filter(|_| screen == Screen::InGame).or(net.server().map(|server| server.ticks()));
        gate.server_now = ticks.map_or(now, |ticks| ticks as f32 / voxmar_protocol::TICKS_PER_SECOND as f32);
        if let Some(server) = net.server() {
            server.set_lagging(gate.lagging());
            server.set_hurried(gate.hurried());
        }
    }
    // The wall clock, for what ends a session whose other clocks stopped (`guard`).
    gate.wall += world.resource::<Time<Real>>().delta_secs().min(SUSPENDED);
    match world.get_resource::<Net>().and_then(Net::server).map(|server| server.ticks()) {
        Some(ticks) if ticks == gate.ticks_seen => {}
        ticks => (gate.ticks_seen, gate.ticks_changed) = (ticks.unwrap_or(0), gate.wall),
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
    let real = world.resource::<Time<Real>>().elapsed_secs();
    if gate.loads.back().is_none_or(|&(at, _)| real - at >= 1.0) {
        gate.loads.push_back((real, load_average()));
        while gate.loads.front().is_some_and(|&(at, _)| real - at > LOAD_SPAN) {
            gate.loads.pop_front();
        }
    }
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

/// How many times the game's pace the workshop's machine stages tick (`Gate::hurried`).
const MACHINES_HURRIED: u32 = 4;

/// The workshop's machine phases that play only in the full gate, and the seconds each
/// adds to the session there (measured on a loaded Mac, with room): the session's limits,
/// `TIME_LIMIT` and `WALL_LIMIT`, grow by their sum in the full gate, so a phase dropped
/// from this list takes its time with it.
const FULL_ONLY_PHASES: [(&str, f32); 4] = [("MV washer", 120.0), ("bloomery and logistics", 160.0), ("iron age", 140.0), ("HV age and treasure", 180.0)];

/// What the full gate's session limits allow on top of the landing gate's.
fn full_allowance() -> f32 {
    if full_gate() { FULL_ONLY_PHASES.iter().map(|(_, seconds)| seconds).sum() } else { 0.0 }
}

/// Whether this is `tools/gate --full`, which sets `VOXMAR_GATE_FULL=1`: the steps too long
/// for every landing (the workshop's later machine phases) play only then.
fn full_gate() -> bool {
    static FULL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FULL.get_or_init(|| std::env::var("VOXMAR_GATE_FULL").is_ok_and(|v| v == "1"))
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
    // A step setting Hitpoints lower cuts health to the new most: no hurt.
    if health < gate.health_last && health == player.max_health {
        info!("gate: health {} -> {health} during {:?}: the most fell (Hitpoints set lower)", gate.health_last, gate.step);
    } else if health < gate.health_last {
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
    let time_limit = TIME_LIMIT + full_allowance();
    if now > time_limit {
        return Err(format!("still at {:?} after {time_limit}s", gate.step));
    }
    // Every wait in the world counts the server's seconds, so these end a session whose
    // server stopped (its ticks standing still while the game goes on) or that takes too
    // long by the wall clock whatever its clocks say. Once it has ticked: a server that
    // never starts is the load's to fail.
    if gate.ticks_seen > 0 && gate.wall - gate.ticks_changed > SERVER_SILENT {
        gate.server_stopped = true;
        return Err(format!(
            "the server stopped: its clock stood at tick {} ({:.1} server s) for {SERVER_SILENT} s of the wall clock during {:?}",
            gate.ticks_seen, gate.server_now, gate.step
        ));
    }
    let wall_limit = WALL_LIMIT + full_allowance();
    if gate.wall > wall_limit {
        return Err(format!("still at {:?} after {wall_limit} s of the wall clock", gate.step));
    }
    let limit = match gate.step {
        Step::Load => LOAD_LIMIT,
        // A fresh area of the world to generate and mesh.
        Step::Gorge | Step::Ore | Step::Tarns => GORGE_OVERLOADED_LIMIT + STEP_LIMIT,
        // Striking judges itself by the blows the server answers (see `strike`).
        Step::Strike | Step::Weapon | Step::Sling => STRIKE_LIMIT,
        // The feel step ends itself by the server's clock (`FEEL_LIMIT`, then the kill);
        // this is the backstop for a server that stopped.
        Step::Feel => FEEL_LIMIT + 2.0 * UNANSWERED + STEP_LIMIT,
        Step::Breaks => BREAKS_LIMIT + 15.0,
        // Crafting, mining, tilling and fishing until something bites.
        // In the full gate, with time for its full-only machine phases.
        Step::Workshop => 250.0 + full_allowance(),
        Step::HvAge => 180.0,
        // Torches crafted for a pinned quest, waited on in the server's seconds, and a quest
        // done left to drop off.
        Step::Quests => 60.0,
        // A party walks in, is talked to, traded with and sent away, each in server seconds.
        Step::Party => 60.0,
        // The effects are judged once a second by the server, and a rest is a sleep away.
        Step::Home => 70.0,
        // The barrel opened, then a server pass for the egg and a second for the chat.
        Step::Nest => 45.0,
        // A rack hung, then a pot set simmering, each answer waited on.
        Step::Dry => 5.0 * DRY_WAIT + STEP_LIMIT,
        // The refusal waited on, then a well set down by the plot.
        Step::Refusal => UNANSWERED + 3.0 * REFUSAL_WAIT + STEP_LIMIT,
        // A fresh area around the settlement to generate: up to `SETTLEMENT_LOAD` of the
        // server's seconds.
        Step::Settlements => SETTLEMENT_LOAD + SETTLEMENT_PEOPLE + STEP_LIMIT,
        _ => STEP_LIMIT,
    };
    // A step in the world is too long only by both the server's clock, as its own waits
    // are, and the game's, as its frames are: a server running behind takes longer, not too
    // long, as does a machine drawing slowly against a server at full speed (`Step::lagged`),
    // and one that hangs runs into TIME_LIMIT.
    // Loading and leaving have no server ticking for them. The text-fit tours are judged
    // view by view: sixty views at a settle each take most of a step's time on their own,
    // and a lagging machine draws each later, not wrongly.
    let (taken, clock) = match gate.step {
        Step::Fit | Step::FitJournal | Step::FitMenus => (now - gate.phase_since, "s"),
        Step::Load | Step::Leave => (now - gate.since, "s"),
        _ => (gate.server_in_step().min(now - gate.since), " s by the server's clock and the game's"),
    };
    if taken > limit {
        return Err(format!("{:?} took longer than {limit}{clock}", gate.step));
    }
    if let Some(observer) = &mut gate.observer
        && !matches!(gate.step, Step::Leave | Step::FitMenus)
    {
        observer.pump()?;
    }
    Ok(())
}

fn step(world: &mut World, gate: &mut Gate, now: f32) {
    music_fades(world, gate, now);
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
            // The steps from here lean on the blocks the starting kit once held in the hotbar
            // (planks, cobblestone, glass, glowstone); the kit now is tools, food and torches,
            // so the player is given those, as the steps after the grave do.
            for line in [
                "time set noon".to_owned(),
                "time freeze".into(),
                // No party sets out by chance into a step about something else (the party
                // step starts its own).
                "wander hold".into(),
                "wander end".into(),
                "clear".into(),
                "give oak_planks 64".into(),
                "give cobblestone 64".into(),
                "give glass 32".into(),
                "give glowstone 16".into(),
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
            gate.next(Step::Collect, now);
        }
        Step::Collect => {
            key(world, KeyCode::KeyW, since <= 1.0);
            let wanted = gate.count_before + gate.dug.len() as u32;
            let count = dropped_count(world, gate);
            // Breaking pops items out with a random kick, and one from the top block can
            // land on the ground beside the hole, out of reach from its bottom, as in
            // Minecraft. Then the player climbs back out beside the hole to gather it.
            // Picking up is the server's work: waited on by its clock.
            if count < wanted && gate.server_in_step() > 4.0 && gate.phase == 0 {
                let line = format!("tp {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5);
                world.resource::<Net>().command(&line);
                gate.advance(now);
            }
            if count >= wanted || gate.server_in_step() > 10.0 {
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
        Step::HvAge => hv_age(world, gate, now),
        Step::Swim => swim(world, gate, now, in_phase),
        Step::Screens => screens(world, gate, now, in_phase),
        Step::Music => music(world, gate, now, in_phase),
        Step::Overlay => overlay(world, gate, now, in_phase),
        Step::Weather => weather(world, gate, now, in_phase),
        Step::Depths => depths(world, gate, now, in_phase),
        Step::Tree => tree(world, gate, now, in_phase),
        Step::Shapes => shapes(world, gate, now, in_phase),
        Step::Held => held(world, gate, now, in_phase),
        Step::Sky => sky(world, gate, now, in_phase),
        Step::NaturalSpawns => natural_spawns(world, gate, now),
        Step::Creatures => creatures(world, gate, now, in_phase),
        Step::Strike => strike(world, gate, now, in_phase),
        Step::Weapon => weapon(world, gate, now, in_phase),
        Step::Sling => sling(world, gate, now, in_phase),
        Step::Feel => feel(world, gate, now, in_phase),
        Step::Boss => boss(world, gate, now),
        Step::Smoke => smoke(world, gate, now),
        Step::Wear => wear(world, gate, now, in_phase),
        Step::CombatHud => combat_hud(world, gate, now, in_phase),
        Step::CombatFeel => combat_feel(world, gate, now, in_phase),
        Step::CreatureWalk => creature_motion(world, gate, gate.phase.min(1)),
        Step::CreatureRun => creature_motion(world, gate, 2),
        Step::CreatureTurn => creature_motion(world, gate, 3),
        Step::Cook => cook(world, gate, now, in_phase),
        Step::Dry => dry(world, gate, now),
        Step::Snare => snare(world, gate, now),
        Step::Tame => tame(world, gate, now, in_phase),
        Step::Party => party(world, gate, now),
        Step::Gorge => gorge(world, gate, now, in_phase),
        Step::Tarns => tarns(world, gate, now, in_phase),
        Step::Settlements => settlements(world, gate, now, in_phase),
        Step::Ore => ore(world, gate, now, in_phase),
        Step::Recipes => recipes(world, gate, now, in_phase),
        Step::Chop => chop(world, gate, now),
        Step::LookUp => look_up(world, gate, now, in_phase),
        Step::Refusal => refusal(world, gate, now, in_phase),
        Step::Merged => merged(world, gate, now, in_phase),
        Step::Identity => identity(world, gate, now),
        Step::Effects => effects(world, gate, now, in_phase),
        Step::Tips => tips(world, gate, now, in_phase),
        Step::Celebrate => celebrate(world, gate, now, in_phase),
        Step::Look => look_name(world, gate, now, in_phase),
        Step::Guides => guides(world, gate, now, in_phase),
        Step::Journal => journal(world, gate, now, in_phase),
        Step::Prayers => prayers(world, gate, now, in_phase),
        Step::Home => home(world, gate, now),
        Step::WayHome => way_home(world, gate, now, in_phase),
        Step::Nest => nest(world, gate, now),
        Step::Door => door(world, gate, now, in_phase),
        Step::Makeable => makeable(world, gate, now, in_phase),
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
/// Longest the row may take to break, in seconds of the server's clock (it gives the tools
/// and has the last word on every break).
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
                holders.truncate(cells.len());
                let tools = &world.resource::<Launch>().content.tools;
                let seconds = |block: BlockId, holder: Holder| {
                    let item = match holder {
                        Holder::Hand => None,
                        Holder::Planks => registry.id("base:oak_planks"),
                        Holder::Tool(tool) => Some(tool),
                    };
                    registry.info(block).hardness.max(0.0) / tools.speed(item, block).max(0.05)
                };
                cells.iter().zip(break_row(&blocks, &holders, seconds)).map(|(&cell, (block, holder))| (feet + cell, block, holder)).collect::<Vec<_>>()
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
            } else if gate.server_in_phase() > 10.0 {
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
            let waited = gate.server_in_phase();
            let breaks = &mut gate.breaks;
            let client = world.resource::<ClientWorld>();
            let Some(&(cell, _, holder)) = breaks.row.get(breaks.done) else {
                mouse(world, MouseButton::Left, false);
                gate.advance(now);
                return;
            };
            if waited > BREAKS_LIMIT {
                mouse(world, MouseButton::Left, false);
                let stuck = format!("{} of {} blocks broke in {BREAKS_LIMIT} server s; stuck on {cell} with {holder:?}", breaks.done, breaks.row.len());
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

/// The breaks step's row: every holder once, in order, each with a block, and every block
/// (`blocks`, hardest last) at least once. The bare hand takes the slowest block it can
/// and the planks the slowest after that, so a long break runs under the load and the hitches
/// (and the planks' Q, thrown halfway, lands mid-break); each block not yet in the row goes to
/// the holder left that breaks it fastest (of those as fast, the one it costs least beside
/// the block that holder breaks fastest), and every other holder takes the block it breaks
/// fastest, so the row takes no longer than it must. Fewer holders than blocks, and the
/// blocks left over go to the holders again in turn.
fn break_row(blocks: &[BlockId], holders: &[Holder], seconds: impl Fn(BlockId, Holder) -> f32) -> Vec<(BlockId, Holder)> {
    let mut row: Vec<Option<BlockId>> = vec![None; holders.len()];
    let fastest = |holder: Holder, among: &mut dyn Iterator<Item = BlockId>| among.min_by(|&a, &b| seconds(a, holder).total_cmp(&seconds(b, holder)));
    let slowest = |holder: Holder, among: &mut dyn Iterator<Item = BlockId>| among.max_by(|&a, &b| seconds(a, holder).total_cmp(&seconds(b, holder)));
    for slow in [Holder::Hand, Holder::Planks] {
        if let Some(i) = holders.iter().position(|&h| h == slow) {
            let taken: Vec<BlockId> = row.iter().flatten().copied().collect();
            row[i] = slowest(slow, &mut blocks.iter().copied().filter(|b| !taken.contains(b)));
        }
    }
    for &block in blocks {
        if row.contains(&Some(block)) {
            continue;
        }
        // The fastest on it; of those as fast, the one it costs least beside its own fastest.
        let cost = |i: usize| {
            let own = fastest(holders[i], &mut blocks.iter().copied()).map_or(0.0, |b| seconds(b, holders[i]));
            (seconds(block, holders[i]), seconds(block, holders[i]) - own)
        };
        let free = (0..holders.len()).filter(|&i| row[i].is_none());
        let Some(i) = free.min_by(|&a, &b| {
            let (a, b) = (cost(a), cost(b));
            a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
        }) else {
            break;
        };
        row[i] = Some(block);
    }
    let mut row: Vec<(BlockId, Holder)> = row.into_iter().zip(holders).map(|(block, &holder)| (block.or_else(|| fastest(holder, &mut blocks.iter().copied())).unwrap_or(blocks[0]), holder)).collect();
    let mut left = blocks.iter().filter(|b| !row.iter().any(|(r, _)| r == *b)).copied().collect::<Vec<_>>().into_iter();
    for i in 0.. {
        let Some(block) = left.next() else { break };
        row.push((block, holders[i % holders.len()]));
    }
    row
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

/// An item `select` moved into the hand, the player states the client had been sent then,
/// and the server's tick: until a later state has it in the hotbar, it is not yet in hand
/// as the server sees it. A state already on its way when the move went out (another
/// item given) puts it back where it was, and a click then would be made empty handed.
#[derive(Resource)]
struct Moving {
    item: String,
    states: u32,
    tick: u64,
}

/// Server seconds after which a move into the hand no state confirmed is proposed again.
const MOVE_UNANSWERED: f32 = 2.0;

/// Takes item `name` in hand: selects its hotbar slot, or moves it into `TOOL_SLOT` and
/// selects that. True once it is in the hotbar as the server last said; false while there
/// is none, and while a move into the hand waits for the server's word (see `Moving`).
fn select(world: &mut World, name: &str) -> bool {
    let Some(slot) = holding(world, name).1 else { return false };
    let states = world.resource::<LocalPlayer>().states;
    let tick = world.resource::<Net>().server().map_or(0, |server| server.ticks());
    if let Some(moving) = world.get_resource::<Moving>()
        && moving.item == name
    {
        let waited = tick.saturating_sub(moving.tick) as f32 / voxmar_protocol::TICKS_PER_SECOND as f32;
        if states == moving.states && waited < MOVE_UNANSWERED {
            return false;
        }
        if slot >= voxmar_engine::inventory::HOTBAR_SLOTS && waited < MOVE_UNANSWERED {
            // A state sent before the move came first: the next one has it moved.
            return false;
        }
        world.remove_resource::<Moving>();
    }
    if slot >= voxmar_engine::inventory::HOTBAR_SLOTS {
        let actions = world.resource::<crate::Actions>().0.clone();
        let target = voxmar_engine::action::ActionTarget::Slots { from: slot as u8, to: TOOL_SLOT as u8, count: u8::MAX };
        world.resource::<Net>().propose(actions.expect("core:transfer"), target);
        world.resource_mut::<LocalPlayer>().inventory.move_items(slot, TOOL_SLOT, u8::MAX);
        world.resource_mut::<LocalPlayer>().selected = TOOL_SLOT;
        world.insert_resource(Moving { item: name.to_owned(), states, tick });
        return false;
    }
    world.resource_mut::<LocalPlayer>().selected = slot;
    true
}

/// Selects an empty hotbar slot, to use a machine with an empty hand (which takes out what
/// it made); false while none is free. Not the whole pack emptied: fuel a machine taken down
/// let fall (`Server::machine_changed`) may be picked up on the way, and must not hold the
/// step up.
fn empty_hand(world: &mut World) -> bool {
    let player = world.resource::<LocalPlayer>();
    let Some(slot) = (0..voxmar_engine::inventory::HOTBAR_SLOTS).find(|&s| player.inventory.get(s).is_none()) else { return false };
    world.resource_mut::<LocalPlayer>().selected = slot;
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
                fail(world, gate, "table", "four planks in the grid made no crafting table".into());
            }
        }
        3 => {
            // Clicked only once the table is in hand as the server has it (`select`).
            let placed = block_is(world, gate, table, "base:crafting_table");
            let in_hand = placed || select(world, "base:crafting_table");
            let pressed = gate.frames % 2 == 0;
            mouse(world, MouseButton::Right, pressed && in_hand && !placed);
            if block_is(world, gate, table, "base:crafting_table") {
                mouse(world, MouseButton::Right, false);
                aim(world, table);
                gate.advance(now);
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
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
                if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
                fail(world, gate, "pickaxe", "the table's grid made no stone pickaxe".into());
            }
        }
        // An iron ore asks for Mining 15 and a stone pickaxe: refused with the level shown,
        // then broken at level 15 with the pickaxe, teaching Mining.
        6 => {
            if !block_is(world, gate, ore, "base:iron_ore") || world.resource::<LocalPlayer>().station.is_some() {
                if gate.server_in_phase() > WORKSHOP_WAIT {
                    fail(world, gate, "ore", "the iron ore never came, or the table's grid stayed open".into());
                }
                return;
            }
            if !select(world, "base:stone_pickaxe") {
                mouse(world, MouseButton::Left, false);
                if gate.server_in_phase() > WORKSHOP_WAIT {
                    fail(world, gate, "ore", "the stone pickaxe never came to hand".into());
                }
                return;
            }
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
                mouse(world, MouseButton::Left, false);
                fail(world, gate, "ore", format!("at Mining 15 with the pickaxe the ore {} and taught {learnt} tenths", if gone { "broke" } else { "stayed" }));
            }
        }
        // A hoe tills dirt into farmland with a right click.
        8 => {
            if !select(world, "base:wooden_hoe") {
                if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
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
                // The best rod at the top level lands about 0.85 of casts (0.6 with the
                // first rod at 80), so a catch mostly takes one cast rather than two.
                net.command("give fine_rod 1");
                net.command("skill fishing 99");
                gate.advance(now);
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
                fail(world, gate, "seed pouch", format!("50 seeds given with a seed pouch carried, the pouch holds {pouched}"));
            }
        }
        // A rod held over the pool, right button down, casts until something bites.
        11 => {
            let fish = |world: &World| {
                let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
                player.inventory.slots.iter().flatten().find(|s| client.registry.def(s.item).has_tag("fish")).map(|s| client.registry.def(s.item).name.clone())
            };
            if !select(world, "base:fine_rod") {
                if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > 40.0 {
                mouse(world, MouseButton::Right, false);
                fail(world, gate, "fishing", "40 server seconds of casting into the pool at Fishing 99 with a fine rod caught nothing".into());
            }
        }
        12 => {
            if carried(world) == 44 {
                gate.count_before = 44;
                gate.fell = world.resource::<LocalPlayer>().position;
                gate.killed_on_purpose = true;
                world.resource::<Net>().command("kill");
                gate.advance(now);
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
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
            } else if gate.server_in_phase() > WORKSHOP_WAIT {
                fail(world, gate, "death keeps three", format!("after /kill: dead {dead}, carrying {kept} of {} items", gate.count_before));
            }
        }
        // Awake again, back where they fell, the player picks everything up.
        14 => {
            if world.resource::<LocalPlayer>().dead {
                if gate.server_in_phase() > WORKSHOP_WAIT {
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
                    "clear".to_owned(),
                    "give coal_ore 24".to_owned(),
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
        // Right clicking the crucible lights the kiln: the quest that asks for it counts it in its last part.
        15 if gate.bronze_since.0 > 0.0 => bronze(world, gate, now, in_phase),
        15 => {
            let kiln = gate.kiln;
            let lit = |world: &World| {
                world.resource::<LocalPlayer>().quests.iter().find(|q| q.name == "base:crucible_kiln").and_then(|q| q.tasks.first()?.last().copied()).is_some_and(|(have, _)| have > 0)
            };
            let crucible = world.resource::<ClientWorld>().registry.id("base:crucible");
            if world.resource::<ClientWorld>().block(kiln) != crucible {
                if gate.server_in_phase() > WORKSHOP_WAIT {
                    fail(world, gate, "kiln", "the crucible never came".into());
                }
                return;
            }
            let (coal, slot) = holding(world, "base:coal_ore");
            if !lit(world) {
                let Some(slot) = slot.filter(|_| coal >= 24) else {
                    if gate.server_in_phase() > WORKSHOP_WAIT {
                        fail(world, gate, "kiln", "the complete coal charge never came".into());
                    }
                    return;
                };
                world.resource_mut::<LocalPlayer>().selected = slot;
            }
            if gate.kiln_since == 0.0 {
                gate.kiln_since = gate.server_now;
            }
            // Asked again only every few server seconds: each ask may stall the lagging
            // server (`SERVER_LAG`), and asked every few frames it never got to a tick.
            if gate.server_now - gate.kiln_asked >= 3.0 {
                gate.kiln_asked = gate.server_now;
                let actions = world.resource::<crate::Actions>().0.clone();
                // With the player's input first (the coal just selected), as the machines'
                // use goes: sent alone, the first ask can reach the server with the slot held
                // before, be refused, and leave the lighting to the next, three seconds on.
                world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:light"), voxmar_engine::action::ActionTarget::Block { block: kiln, face: voxmar_engine::coords::Face::NegY });
            }
            // Quests are checked once a second of the server's time, which a loaded machine
            // stretches: the wait is by its clock.
            let done = lit(world);
            let late = gate.server_now - gate.kiln_since > 20.0;
            // Quest progress and the inventory travel as separate messages. Observe both
            // before taking the kiln down; a late inventory still fails the same deadline.
            if done && coal != 0 && !late {
                return;
            }
            if late && !done {
                // Taken down, so the pool's bank is clear for the swim. A kiln lit late
                // still smelts its bronze first (`bronze` takes it down).
                kiln_down(world, kiln);
            }
            if done {
                if coal != 0 {
                    kiln_down(world, kiln);
                    return fail(world, gate, "kiln", format!("lighting left {coal} coal of its 24-item charge"));
                }
                gate.check("workshop: kiln", Ok("a complete coal charge lit the standing Crucible Kiln and counted its lighting goal".into()));
                // The lit kiln smelts bronze before it comes down: three copper ingots in
                // hand and a tin ore in the pack make four, in one go.
                for line in ["clear", "skill engineering 12", "give copper_ingot 3", "give tin_ore 1"] {
                    world.resource::<Net>().command(line);
                }
                gate.bronze_since = (gate.server_now, in_phase);
                gate.kiln_asked = f32::MIN;
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
            let (coal, ore, dust) = (holding(world, "base:coal_ore").0, holding(world, "base:copper_ore").0, holding(world, "base:copper_dust").0);
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
                0 => (put_in, boiler, Some("base:coal_ore")),
                1 => (put_in, crusher, Some("base:copper_ore")),
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
                    // The landing gate stays quick: the boiler and crusher are its machines
                    // check, and the MV, bloomery and iron-age phases play only in the full
                    // gate. Taken down here, with the kit back for the steps after.
                    if !full_gate() {
                        let (a, b) = (gate.kiln - IVec3::Z, gate.kiln + IVec3::new(3, 1, 1));
                        let net = world.resource::<Net>();
                        for line in [format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z), "clear".to_owned()] {
                            net.command(&line);
                        }
                        for line in ["give oak_planks 64", "give cobblestone 64", "give glass 32", "give glowstone 16"] {
                            net.command(line);
                        }
                        end_workshop(world, gate, now);
                        return;
                    }
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
                None => empty_hand(world),
            };
            // Used only every few server seconds (see the kiln above); the crusher takes a
            // few of them to work the ore.
            if ready && gate.server_now - gate.machines_asked >= 3.0 {
                gate.machines_asked = gate.server_now;
                let actions = world.resource::<crate::Actions>().0.clone();
                // With the player's input first, as a click sends it: the slot just selected
                // reaches the server before the use, not with the next input. A use from the
                // slot held before (emptied by the last use) takes an idle machine's inputs
                // back out (`Factory::take_outputs_into`).
                world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
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
            let (coal, ore, dust) = (holding(world, "base:coal_ore").0, holding(world, "base:copper_ore").0, holding(world, "base:copper_dust").0);
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
                            // Asked for two blocks east of its spot: /summon sets the first of
                            // a group 1.8 blocks west, which lands it on the floor beside the
                            // wheel, not off its edge on the ground below, out of the wheel's reach.
                            format!("summon aurochs 1 {}", at(IVec3::new(5, 0, 2))),
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
                    0 => select(world, "base:coal_ore"),
                    1 => select(world, "base:copper_ore"),
                    _ => empty_hand(world),
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
                        world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
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
            let (ore, coal, copper, bloom) = (holding(world, "base:iron_ore").0, holding(world, "base:coal_ore").0, holding(world, "base:copper_ore").0, holding(world, "base:iron_bloom").0);
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
                            // Asked for two blocks east of its spot: /summon sets the first of
                            // a group 1.8 blocks west, which lands it on the floor beside the
                            // wheel, not off its edge on the ground below, out of the wheel's reach.
                            format!("summon aurochs 1 {}", at(IVec3::new(5, 0, 2))),
                            "creatures tame aurochs".to_owned(),
                            "creatures hold aurochs".to_owned(),
                            // A known floor and pose, rather than wherever gathering the
                            // death drops left the player below these raised machines.
                            format!("setblock {} cobblestone", at(IVec3::new(-2, -1, 3))),
                            format!("fill {} {} air", at(IVec3::new(-2, 0, 3)), at(IVec3::new(-2, 2, 3))),
                            format!("tp {} {} {}", k.x as f32 - 1.5, k.y, k.z as f32 + 3.5),
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
                    0 => select(world, "base:iron_ore"),
                    1 | 3 => select(world, "base:coal_ore"),
                    4 => select(world, "base:copper_ore"),
                    _ => empty_hand(world),
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
                        world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
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
            let operating = k.as_vec3() + Vec3::new(-1.5, 0.0, 3.5);
            let vantage = k.as_vec3() + Vec3::new(0.0, 4.0, 11.0);
            // "at N% power" or "N% done" above nought in a /machine answer.
            let blown = |said: &str| {
                said.trim_end_matches('.').split(", ").any(|part| {
                    let number = part.strip_prefix("at ").and_then(|p| p.strip_suffix("% power")).or_else(|| part.strip_suffix("% done"));
                    number.and_then(|n| n.parse::<f32>().ok()).is_some_and(|n| n > 0.0)
                })
            };
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
            if stage < 5 && world.resource::<LocalPlayer>().position.distance(operating) >= 1.0 {
                if gate.server_now - gate.machines_since > 10.0 {
                    let position = world.resource::<LocalPlayer>().position;
                    teardown(world);
                    fail(world, gate, "iron age", format!("the player never reached the operating floor: at {position:?}, wanted {operating:?}"));
                }
                return;
            }
            // Flying until the teleport to the vantage lands (as the sky step does), then turned
            // to both works every frame, so the camera has turned by the time of the shot.
            let arrived = world.resource::<LocalPlayer>().position.distance(vantage) < 1.0;
            if stage == 5 && !arrived {
                world.resource_mut::<LocalPlayer>().flying = true;
            } else if stage == 5 {
                aim(world, k + IVec3::new(-1, 0, 0));
            }
            let client = world.resource::<ClientWorld>();
            if client.block(oven) != client.registry.id("base:coke_oven") || client.block(wheel) != client.registry.id("base:treadwheel") {
                if in_phase > WORKSHOP_WAIT && gate.server_now - gate.machines_since > 10.0 {
                    teardown(world);
                    fail(world, gate, "iron age", "the coke oven and blast furnace never came".into());
                }
                return;
            }
            let (coal, ore, coke) = (holding(world, "base:coal_ore").0, holding(world, "base:iron_ore").0, holding(world, "base:coke").0);
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
                // Making iron with the wheel's power, not only started: power or progress
                // above none (`machine.working_powered`), asked on the server's clock.
                4 => said.iter().any(|r| r.to_lowercase().contains("making iron ingot") && blown(r)),
                // Up where both works show, for the shot, a server second after the teleport.
                _ => arrived && gate.server_now - gate.machines_since > 1.0,
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
                    // Back and up, south of the works (the oven's face, the aurochs' side),
                    // flying, for the shot.
                    4 => {
                        net.command(&format!("tp {} {} {}", vantage.x, vantage.y, vantage.z));
                        world.resource_mut::<LocalPlayer>().flying = true;
                    }
                    _ => {
                        // Both works in view: the oven west, the furnace's shaft, its wheel
                        // and the aurochs east.
                        aim(world, k + IVec3::new(-1, 0, 0));
                        if gate.shot(world, "iron_age").is_none() {
                            return;
                        }
                        gate.check(
                            "workshop: iron age",
                            Ok("a coal in the Coke Oven set it baking coke; two iron ore and a coke in the Blast Furnace, blown by an aurochs on the treadwheel, set it making iron".into()),
                        );
                        world.resource_mut::<LocalPlayer>().flying = false;
                        teardown(world);
                        gate.next(Step::HvAge, now);
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
                    0 => select(world, "base:coal_ore"),
                    2 => select(world, "base:iron_ore"),
                    3 => select(world, "base:coke"),
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
                        world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:use_machine"), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegY });
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
                    "the blast furnace never said it was making iron with power",
                    "the player never reached the place for the shot",
                ][stage.min(5) as usize];
                let waited = (gate.server_now - gate.machines_since, in_phase);
                world.resource_mut::<LocalPlayer>().flying = false;
                teardown(world);
                fail(world, gate, "iron age", format!("{what} ({:.1} server s, {:.1} game s): holding coal {coal}, iron ore {ore}, coke {coke}; /machine said {said:?}", waited.0, waited.1));
            }
        }
        _ => {}
    }
}

/// Server seconds each HV-age shot waits after its teleport, on top of the player standing
/// there with the pick in hand for three frames and the cable as the shot wants it.
const HV_SHOT_SETTLE: f32 = 0.5;

/// The HV source, processors and treasure played with the real server. Inputs must first
/// arrive in hand and then leave it; machine replies must show power/progress, not merely
/// a queued recipe. Every wait on the server uses its clock.
fn hv_age(world: &mut World, gate: &mut Gate, now: f32) {
    let content = world.resource::<Launch>().content.clone();
    let english = |words: &Words| content.english.render(words, &content.blocks);
    let h = gate.kiln + IVec3::new(1, 0, -5);
    let rel = |x, y, z| h + IVec3::new(x, y, z);
    let at = |p: IVec3| format!("{} {} {}", p.x, p.y, p.z);
    let (core, boiler, array, smelter) = (rel(0, 2, 1), rel(0, 3, 1), rel(-1, 2, 4), rel(-1, 3, 1));
    let chest = rel(-1, 0, 7);
    let use_at = |world: &World, target, action: &str| {
        let actions = world.resource::<crate::Actions>().0.clone();
        world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect(action), voxmar_engine::action::ActionTarget::Block { block: target, face: voxmar_engine::coords::Face::NegX });
    };
    let ask = |gate: &mut Gate, target| {
        if let Some(o) = gate.observer.as_mut() {
            o.replies.clear();
            o.connection.send(ClientMessage::Command { line: format!("machine {}", at(target)) });
        }
        gate.machines_asked = gate.server_now;
    };
    let powered = |text: &str| text.trim_end_matches('.').split(", ").any(|part| {
        part.strip_prefix("at ").and_then(|p| p.strip_suffix("% power")).or_else(|| part.strip_suffix("% done"))
            .and_then(|n| n.parse::<f32>().ok()).is_some_and(|n| n > 0.0)
    });
    if gate.server_in_phase() > 35.0 {
        let said: Vec<_> = gate.observer.as_ref().map(|o| o.replies.iter().map(english).collect()).unwrap_or_default();
        gate.check("HV age", Err(format!("stage {} made no progress for 35 server seconds: {said:?}", gate.phase)));
        set_language(world, "en");
        world.resource_mut::<LocalPlayer>().flying = false;
        end_workshop(world, gate, now);
        return;
    }
    match gate.phase {
        0 => {
            let mut lines = vec![
                "clear".into(),
                format!("fill {} {} air", at(rel(-5, 0, -1)), at(rel(5, 5, 9))),
                format!("fill {} {} polished_stone", at(rel(-5, -1, -1)), at(rel(5, -1, 9))),
                format!("fill {} {} hv_casing", at(h), at(rel(4, 2, 2))),
                format!("fill {} {} air", at(rel(1, 1, 1)), at(rel(3, 1, 1))),
                format!("setblock {} turbine_hall", at(core)),
                format!("setblock {} boiler", at(boiler)),
                format!("setblock {} hv_cable", at(rel(-1, 2, 1))),
                format!("setblock {} mv_transformer", at(rel(-1, 1, 1))),
                format!("setblock {} hv_battery_bank", at(rel(-2, 2, 1))),
                format!("setblock {} induction_smelter", at(smelter)),
                format!("setblock {} hv_cable", at(rel(-1, 2, 2))),
                format!("setblock {} hv_cable", at(rel(-1, 2, 3))),
                format!("fill {} {} hv_casing", at(rel(-2, 1, 4)), at(rel(0, 3, 6))),
                format!("setblock {} air", at(rel(-1, 2, 5))),
                format!("setblock {} processing_array", at(array)),
                "give coal_ore 4".into(),
                format!("tp {}", at(rel(-3, 0, 2))),
            ];
            for (i, name) in ["nomad", "farm", "bronze", "iron", "steam"].iter().enumerate() {
                lines.push(format!("setblock {} {name}_treasure_chest", at(rel(i as i32 - 3, 0, 7))));
            }
            for line in lines { world.resource::<Net>().command(&line); }
            world.resource_mut::<LocalPlayer>().flying = true;
            gate.machines_seen = false;
            gate.machines_asked = f32::MIN;
            gate.advance(now);
        }
        // Fuel, then two substantial batches so the replies cannot miss a short job.
        1 | 2 | 3 => {
            let (name, target) = match gate.phase { 1 => ("base:coal_ore", boiler), 2 => ("base:copper_ore", array), _ => ("base:copper_ore", smelter) };
            gate.machines_seen |= holding(world, name).0 > 0;
            if gate.machines_seen && holding(world, name).0 == 0 && gate.machines_asked > f32::MIN {
                if gate.phase < 3 { world.resource::<Net>().command("give copper_ore 32"); }
                gate.machines_seen = false;
                gate.machines_asked = f32::MIN;
                gate.advance(now);
            } else if select(world, name) && gate.server_now - gate.machines_asked >= 3.0 {
                use_at(world, target, "base:use_machine");
                gate.machines_asked = gate.server_now;
            }
        }
        4 | 5 | 6 => {
            let (target, want) = match gate.phase { 4 => (core, "running"), 5 => (array, "making"), _ => (smelter, "making") };
            let machine = match gate.phase { 4 => "base:turbine_hall", 5 => "base:processing_array", _ => "base:induction_smelter" };
            let prefix = format!("{}:", english(&Words::Block(machine.into())));
            let said = gate.observer.as_ref().and_then(|o| o.replies.iter().map(english).find(|r| r.starts_with(&prefix) && r.to_lowercase().contains(want) && (gate.phase == 4 || powered(r))));
            if let Some(said) = said {
                gate.check(&format!("HV age: {want}"), Ok(said));
                if let Some(o) = gate.observer.as_mut() { o.replies.clear(); }
                gate.machines_asked = f32::MIN;
                gate.advance(now);
                if gate.phase == 7 {
                    world.resource::<Net>().command("clear");
                    for material in ["bone", "copper", "bronze", "iron"] { world.resource::<Net>().command(&format!("give {material}_lockpick 2")); }
                }
            } else if gate.server_now - gate.machines_asked >= 2.0 { ask(gate, target); }
        }
        7 => {
            if ["bone", "copper", "bronze", "iron"].iter().all(|m| holding(world, &format!("base:{m}_lockpick")).0 == 2) {
                gate.advance(now);
            }
        }
        // Identical poses in both languages. The hotbar contains all four picks; selecting
        // each shows its actual translated name above it. Pack art is judged in these shots.
        8..=27 => {
            let i = (gate.phase - 8) / 2;
            let language = if i % 2 == 0 { "en" } else { "ja" };
            let (name, position, target, pick) = match i / 2 {
                0 => (if language == "en" { "hv-hall-en" } else { "hv-hall-ja" }, rel(-5, 5, -3), core, "base:bone_lockpick"),
                1 => (if language == "en" { "hv-array-en" } else { "hv-array-ja" }, rel(-3, 1, 2), array, "base:copper_lockpick"),
                2 => (if language == "en" { "hv-smelter-en" } else { "hv-smelter-ja" }, rel(-5, 5, -1), smelter, "base:bronze_lockpick"),
                3 => (if language == "en" { "hv-power-en" } else { "hv-power-ja" }, rel(-5, 2, -2), rel(-1, 2, 1), "base:iron_lockpick"),
                _ => (if language == "en" { "treasure-row-en" } else { "treasure-row-ja" }, rel(-1, 2, 12), chest, "base:copper_lockpick"),
            };
            if gate.phase % 2 == 0 {
                // The full-cube cable covers the controller's only outward face. After
                // proving the powered job, uncover it for the paired close-ups, then
                // restore the connection before the remaining factory shots.
                if gate.phase == 12 { world.resource::<Net>().command(&format!("setblock {} air", at(rel(-1, 2, 3)))); }
                if gate.phase == 16 { world.resource::<Net>().command(&format!("setblock {} hv_cable", at(rel(-1, 2, 3)))); }
                set_language(world, language);
                world.resource_mut::<Settings>().language = language.into();
                world.resource::<Net>().command(&format!("tp {}", at(position)));
                empty_hand(world);
                gate.advance(now);
            } else if (world.resource::<LocalPlayer>().position - position.as_vec3()).length() < 1.0 && gate.server_in_phase() > HV_SHOT_SETTLE
                && world.resource::<ClientWorld>().block(rel(-1, 2, 3)) == content.blocks.id(if i / 2 == 1 { "core:air" } else { "base:hv_cable" }) {
                aim(world, target);
                if select(world, pick) {
                    gate.settled += 1;
                }
                if gate.settled >= 3 && gate.shot(world, name).is_some() {
                    if i >= 9 {
                        set_language(world, "en");
                        world.resource_mut::<Settings>().language = "en".into();
                        world.resource::<Net>().command(&format!("tp {}", at(rel(-1, 0, 9))));
                        gate.phase = 40;
                        gate.phase_server = gate.server_now;
                        gate.machines_asked = f32::MIN;
                    } else { gate.advance(now); }
                }
            }
        }
        40 => {
            let client = world.resource::<ClientWorld>();
            if client.block(chest) == client.registry.id("base:chest") && holding(world, "base:copper_lockpick").0 == 1 {
                gate.check("treasure: picked", Ok("a copper pick opened the Bronze chest and exactly one pick was used".into()));
                gate.advance(now);
                gate.machines_asked = f32::MIN;
            } else if select(world, "base:copper_lockpick") && gate.server_now - gate.machines_asked >= 3.0 {
                use_at(world, chest, "base:pick_lock");
                gate.machines_asked = gate.server_now;
            }
        }
        41 => {
            let said = gate.observer.as_ref().and_then(|o| o.replies.iter().map(english).find(|r| r.contains("holds") && !r.contains("holds 0 items")));
            if let Some(said) = said {
                gate.check("treasure: contents", Ok(said));
                gate.advance(now);
                gate.machines_asked = f32::MIN;
            } else if gate.server_now - gate.machines_asked >= 2.0 { ask(gate, chest); }
        }
        42 => {
            // The opened chest opens into its slots beside the inventory, its find in them
            // (`stores`): the server's word of it, not a prediction.
            let coin = world.resource::<ClientWorld>().registry.id("base:gold_coin");
            let store = world.resource::<LocalPlayer>().inventory.store.clone();
            let found = store.as_ref().map(|s| s.slots.iter().flatten().count()).filter(|_| store.as_ref().is_some_and(|s| s.slots.iter().flatten().any(|x| Some(x.item) == coin)));
            if let Some(stacks) = found {
                gate.check("treasure: store", Ok(format!("the chest opened into its {} slots, {stacks} stacks in them with its gold", store.map_or(0, |s| s.slots.len()))));
                world.resource_mut::<UiState>().inventory_open = true;
                gate.advance(now);
            } else if empty_hand(world) && gate.server_now - gate.machines_asked >= 3.0 {
                world.resource_mut::<LocalPlayer>().store_asked = true;
                use_at(world, chest, "base:open_store");
                gate.machines_asked = gate.server_now;
            }
        }
        43 | 44 => {
            let language = if gate.phase == 43 { "en" } else { "ja" };
            set_language(world, language);
            world.resource_mut::<Settings>().language = language.into();
            if gate.server_in_phase() > 1.0 && gate.shot(world, if language == "en" { "treasure-open-en" } else { "treasure-open-ja" }).is_some() { gate.advance(now); }
        }
        45 => {
            // Shift click on the gold in the chest's slots takes it into the pack.
            if holding(world, "base:gold_coin").0 > 0 {
                gate.check("treasure: taken", Ok("shift click took the chest's gold into the pack".into()));
                gate.advance(now);
            } else if gate.server_now - gate.machines_asked >= 2.0 {
                let coin = world.resource::<ClientWorld>().registry.id("base:gold_coin");
                let player = world.resource::<LocalPlayer>();
                let slot = player.inventory.store.as_ref().and_then(|s| s.slots.iter().position(|x| x.is_some_and(|x| Some(x.item) == coin)));
                if let Some(slot) = slot {
                    let actions = world.resource::<crate::Actions>().0.clone();
                    let target = voxmar_engine::action::ActionTarget::Slot((voxmar_engine::inventory::STORE + slot) as u8);
                    world.resource::<Net>().propose(actions.expect("core:quick_move"), target);
                }
                gate.machines_asked = gate.server_now;
            } else if gate.server_in_phase() > BOSS_WAIT {
                gate.check("treasure: taken", Err(format!("the chest's gold never reached the pack in {BOSS_WAIT} server seconds")));
                gate.advance(now);
            }
        }
        _ => {
            set_language(world, "en");
            world.resource_mut::<Settings>().language = "en".into();
            world.resource_mut::<UiState>().inventory_open = false;
            world.resource_mut::<LocalPlayer>().flying = false;
            world.resource::<Net>().command(&format!("fill {} {} air", at(rel(-5, -1, -1)), at(rel(5, 5, 9))));
            world.resource::<Net>().command("clear");
            for line in ["give oak_planks 64", "give cobblestone 64", "give glass 32", "give glowstone 16"] { world.resource::<Net>().command(line); }
            end_workshop(world, gate, now);
        }
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
    for tool in ["stone_pickaxe", "wooden_hoe", "crafting_table", "stick", "bread", "fine_rod"] {
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
    let (position, in_water, on_ground, velocity, flying, teleport) = (player.position, player.in_water, player.on_ground, player.velocity, player.flying, player.teleports);
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
            // Release jump when above the bank, then wait for a real dry landing.
            let half = voxmar_engine::physics::player::WIDTH * 0.5;
            let relative = position - feet.as_vec3();
            let over_bank = relative.x > 6.0 - half && relative.x < 7.0 + half
                && relative.z > -1.0 - half && relative.z < 2.0 + half;
            if !in_water && position.y >= step_top - 0.01 {
                key(world, KeyCode::Space, false);
            }
            if on_ground && !in_water && over_bank && (position.y - step_top).abs() <= 0.01 {
                key(world, KeyCode::KeyW, false);
                key(world, KeyCode::Space, false);
                gate.check("swim", Ok(format!("climbed out of the water onto the step in {in_phase:.1}s")));
                gate.advance(now);
            } else if in_phase > 8.0 {
                key(world, KeyCode::KeyW, false);
                key(world, KeyCode::Space, false);
                let reached = gate.highest - feet.y as f32;
                let cell = position.floor().as_ivec3();
                let client = world.resource::<ClientWorld>();
                let support = (client.block(cell), client.block(cell - IVec3::Y));
                gate.check("swim", Err(format!("did not land on the step: highest {reached:+.2}, position {}, velocity {velocity}, grounded {on_ground}, water {in_water}, flying {flying}, over bank {over_bank}, support {support:?}, teleport {teleport}; {in_phase:.1} client s, {:.1} server s", position - feet.as_vec3(), gate.server_in_phase())));
                gate.next(Step::Screens, now);
            }
        }
        2 => {
            let server = gate.observer.as_ref().and_then(|o| o.others.values().next().copied());
            if server.is_none_or(|at| at.distance(position) >= 1.0) && gate.server_in_phase() < 3.0 {
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

/// The now-playing line the music step checked fades out by itself: watched every frame
/// from then on, beside the overlay step (which leaves it alone), rather than waited out
/// on its own. The weather step's first shot waits for it, so no shot has it in.
fn music_fades(world: &mut World, gate: &mut Gate, now: f32) {
    let Some(since) = gate.music_fade else { return };
    if crate::music::now_playing_shown(world).is_none() {
        gate.check("music: now playing fades", Ok(format!("gone {:.1} s after it was checked", now - since)));
        gate.music_fade = None;
    } else if now - since > SCREEN_WAIT {
        gate.check("music: now playing fades", Err("still shown 10 s after it was checked".into()));
        gate.music_fade = None;
    }
}

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
            } else if gate.server_in_phase() > 5.0 {
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
                gate.next(Step::Music, now);
            } else if gate.server_in_phase() > 5.0 {
                gate.check("craft", Err(format!("after crafting a log there are {count} planks and the grid holds {:?}", inventory.grid())));
                gate.next(Step::Music, now);
            }
        }
        _ => {}
    }
}

/// The menu's audio page shows a switch for each category of music there is to hear (its
/// own tracks, or Minecraft's with Minecraft assets on), and clicking the first turns its
/// category off in the settings, saying so beside it, and clicking again turns it back on.
/// Then M turns the music off and on, and the track that comes soon after is named at the
/// top right with its category, and the line fades away (the gate's music is picked and
/// started though silent, `music::Rehearse`).
fn music(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let ui = world.resource::<UiState>();
    let (menu, locked) = (ui.menu_open, ui.cursor_locked);
    let words = |world: &World, on: bool| world.resource::<Locale>().get(if on { "settings.value.on" } else { "settings.value.off" }).to_owned();
    let off = |world: &World| world.resource::<crate::settings::Settings>().music_off.clone();
    match gate.phase {
        0 => {
            key(world, KeyCode::Escape, true);
            gate.advance(now);
        }
        1 => {
            key(world, KeyCode::Escape, false);
            if menu && in_phase > 0.2 {
                match crate::settings::tab_button(world, "settings.tab.audio") {
                    Some(tab) => {
                        world.resource_mut::<UiClicks>().next = Some((tab, MouseButton::Left));
                        gate.advance(now);
                    }
                    None => {
                        gate.check("music: switches", Err("the menu has no audio tab".into()));
                        gate.next(Step::Overlay, now);
                    }
                }
            } else if in_phase > SCREEN_WAIT {
                gate.check("music: switches", Err("Esc did not open the menu".into()));
                gate.next(Step::Overlay, now);
            }
        }
        2 => {
            if in_phase < 0.3 {
                return;
            }
            let minecraft = world.resource::<crate::settings::Settings>().minecraft_assets;
            let wanted: Vec<String> = world
                .get_resource::<crate::music::MusicCategories>()
                .map(|c| c.0.iter().filter(|c| minecraft || !c.borrowed_only()).map(|c| c.name.clone()).collect())
                .unwrap_or_default();
            let shown = crate::settings::music_switches_shown(world);
            let names: Vec<String> = shown.iter().map(|(name, _, _)| name.clone()).collect();
            let all_on = shown.iter().all(|(_, _, said)| *said == words(world, true)) && off(world).is_empty();
            let result = if wanted.is_empty() {
                Err("the content lists no categories of music".into())
            } else if names != wanted {
                Err(format!("the audio page shows switches for {names:?}, not {wanted:?}"))
            } else if !all_on {
                Err(format!("not every category reads on: {:?}, off in the settings {:?}", shown.iter().map(|s| &s.2).collect::<Vec<_>>(), off(world)))
            } else {
                Ok(format!("a switch for each of {}, all on", names.join(", ")))
            };
            let ok = result.is_ok();
            gate.check("music: switches", result);
            match shown.first().filter(|_| ok) {
                Some((_, switch, _)) => {
                    world.resource_mut::<UiClicks>().next = Some((*switch, MouseButton::Left));
                    gate.advance(now);
                }
                None => close(world, gate, now),
            }
        }
        3 => {
            let Some((name, switch, said)) = crate::settings::music_switches_shown(world).first().cloned() else { return };
            if off(world) == [name.clone()] && said == words(world, false) {
                gate.check("music: switch off", Ok(format!("clicked, {name} is off and reads {said}")));
                world.resource_mut::<UiClicks>().next = Some((switch, MouseButton::Left));
                gate.advance(now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("music: switch off", Err(format!("after a click on {name}'s switch it reads {said:?} and the settings have {:?} off", off(world))));
                world.resource_mut::<crate::settings::Settings>().music_off.clear();
                close(world, gate, now);
            }
        }
        4 => {
            let Some((name, _, said)) = crate::settings::music_switches_shown(world).first().cloned() else { return };
            if off(world).is_empty() && said == words(world, true) {
                gate.check("music: switch on", Ok(format!("clicked again, {name} is back on")));
                close(world, gate, now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("music: switch on", Err(format!("after a second click {name} reads {said:?} and the settings have {:?} off", off(world))));
                world.resource_mut::<crate::settings::Settings>().music_off.clear();
                close(world, gate, now);
            }
        }
        5 => {
            key(world, KeyCode::Escape, false);
            if !menu && locked {
                // M turns the music off and on again, and a track comes soon after.
                key(world, KeyCode::KeyM, true);
                gate.advance(now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("music: close", Err("Esc did not close the menu".into()));
                gate.next(Step::Overlay, now);
            }
        }
        6 | 7 => {
            key(world, KeyCode::KeyM, gate.phase == 7);
            gate.advance(now);
        }
        8 => {
            key(world, KeyCode::KeyM, false);
            if let Some(line) = crate::music::now_playing_shown(world) {
                let locale = world.resource::<Locale>();
                let named = world
                    .get_resource::<crate::music::MusicCategories>()
                    .and_then(|c| c.0.iter().map(|c| locale.get(&format!("music.{}", c.name)).to_owned()).find(|name| line.contains(&format!("({name})"))));
                let result = match named {
                    Some(category) if line.starts_with("Now playing: ") && line.len() > "Now playing: ".len() + category.len() + 3 => {
                        Ok(format!("M off and on started a track, shown at the top right as {line:?} ({category})"))
                    }
                    _ => Err(format!("the line reads {line:?}, not a title and a category's name")),
                };
                gate.check("music: now playing", result);
                // The line fades out by itself, watched while the steps after go on.
                gate.music_fade = Some(now);
                gate.next(Step::Overlay, now);
            } else if in_phase > SCREEN_WAIT {
                gate.check("music: now playing", Err("no track was named within 10 s of turning the music back on with M".into()));
                gate.next(Step::Overlay, now);
            }
        }
        _ => {}
    }

    /// Esc, then on to the phase waiting for the menu to close.
    fn close(world: &mut World, gate: &mut Gate, now: f32) {
        key(world, KeyCode::Escape, true);
        gate.phase = 4;
        gate.advance(now);
    }
}

/// The recipe viewer: U over an oak log in the inventory lists what it goes into, then an
/// item clicked in the viewer shows its own recipes, and Back returns.
fn recipes(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::recipes::{HoveredItem, RecipeViewer};
    let blocks = world.resource::<Launch>().content.blocks.clone();
    let (Some(log), Some(planks), Some(campfire)) = (blocks.id("base:oak_log"), blocks.id("base:oak_planks"), blocks.id("base:campfire")) else {
        gate.check("recipes", Err("no oak log, planks or campfire in the content".into()));
        gate.next(Step::Chop, now);
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
                gate.next(Step::Chop, now);
            }
        }
        2 => {
            let made: Vec<String> = world.resource::<RecipeViewer>().found().iter().map(|entry| format!("{:?}", entry.method)).collect();
            let title = world.query_filtered::<&Text, With<Localized>>().iter(world).count();
            let crafted = made.iter().any(|m| m.starts_with("Craft") && m.contains(&format!("{log:?}")));
            if crafted || in_phase > 2.0 {
                gate.check("recipes: click", if crafted { Ok(format!("planks show {} recipes, the log in the grid ({title} texts drawn)", made.len())) } else { Err(format!("planks show {made:?}")) });
                if let Some(cell) = world.query::<(Entity, &crate::hud::recipes::ViewerItem, &InheritedVisibility)>().iter(world).find(|(_, item, visible)| item.0 == log && visible.get()).map(|(entity, ..)| entity) {
                    world.resource_mut::<UiState>().inventory_open = false;
                    world.resource_mut::<UiClicks>().hover = Some(cell);
                    gate.advance(now);
                } else if in_phase > 2.0 {
                    gate.check("recipes: ingredient tooltip", Err("the log has no visible recipe cell to hover".into()));
                    gate.next(Step::Chop, now);
                }
            }
        }
        3 => {
            let name = world.resource::<Locale>().block(blocks.def(log));
            let tooltip = world.query_filtered::<(Entity, &InheritedVisibility), With<crate::hud::inventory::TooltipLines>>().iter(world).find(|(_, visible)| visible.get()).map(|(entity, ..)| entity);
            let named = tooltip.is_some_and(|tip| world.query::<(&Text, &ChildOf)>().iter(world).any(|(text, parent)| parent.parent() == tip && text.0 == name));
            if named {
                if gate.shot(world, "recipe-ingredient-tooltip").is_none() {
                    if let Some(cell) = world.query::<(Entity, &crate::hud::recipes::ViewerItem, &InheritedVisibility)>().iter(world).find(|(_, item, visible)| item.0 == log && visible.get()).map(|(entity, ..)| entity) {
                        world.resource_mut::<UiClicks>().hover = Some(cell);
                    }
                    return;
                }
                gate.check("recipes: ingredient tooltip", Ok(format!("hovering the recipe ingredient draws {name} above the viewer with the inventory closed")));
                gate.advance(now);
            } else if in_phase > 2.0 {
                gate.check("recipes: ingredient tooltip", Err(format!("hovering the recipe ingredient drew no visible {name} tooltip")));
                gate.next(Step::Chop, now);
            } else if let Some(cell) = world.query::<(Entity, &crate::hud::recipes::ViewerItem, &InheritedVisibility)>().iter(world).find(|(_, item, visible)| item.0 == log && visible.get()).map(|(entity, ..)| entity) {
                world.resource_mut::<UiClicks>().hover = Some(cell);
            }
        }
        4 => {
            let hidden = world.query_filtered::<&Visibility, With<crate::hud::inventory::TooltipLines>>().iter(world).all(|visibility| *visibility == Visibility::Hidden);
            if hidden || in_phase > 2.0 {
                gate.check("recipes: tooltip leave", if hidden { Ok("the ingredient tooltip disappears when the pointer leaves".into()) } else { Err("the ingredient tooltip remains after leaving its cell".into()) });
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
            gate.next(Step::Chop, now);
        }
    }
}

/// Where the chop step sets its chopping block: across the campfire from the drying rack.
const CHOP_FROM_FIRE: IVec3 = IVec3::new(-2, 0, 0);

/// Server seconds the chop step waits on each answer.
const CHOP_WAIT: f32 = 10.0;

/// A chopping block by the campfire, seen from two and a half blocks (shot
/// `chopping_block`); then, the inventory open, the client finds it within reach and an oak
/// log laid in the grid makes six planks there, as the server crafts them.
fn chop(world: &mut World, gate: &mut Gate, now: f32) {
    let block = gate.feet + IVec3::new(0, 0, 14) + CHOP_FROM_FIRE;
    let waited = gate.server_in_phase() > CHOP_WAIT;
    // What the client itself does (drawing, finding the block near) is given up only once
    // its own seconds have passed too: frames starved behind a running server had no time.
    let outwaited = waited && now - gate.phase_since > CHOP_WAIT;
    let content = world.resource::<Launch>().content.clone();
    let (log, planks) = (content.blocks.id("base:oak_log"), content.blocks.id("base:oak_planks"));
    let carried = |world: &World, item: Option<BlockId>| world.resource::<LocalPlayer>().inventory.slots.iter().flatten().filter(|s| Some(s.item) == item).map(|s| s.count as u32).sum::<u32>();
    let end = |world: &mut World, gate: &mut Gate| {
        world.resource_mut::<UiState>().inventory_open = false;
        let net = world.resource::<Net>();
        net.command(&format!("setblock {} {} {} air", block.x, block.y, block.z));
        net.command("clear oak_log");
        net.command("clear oak_planks");
        gate.next(Step::LookUp, now);
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("clear oak_log");
            net.command("clear oak_planks");
            net.command("give oak_log 2");
            net.command(&format!("setblock {} {} {} chopping_block", block.x, block.y, block.z));
            net.command(&format!("tp {} {} {}", block.x as f32 + 0.5, block.y, block.z as f32 - 2.5));
            gate.advance(now);
        }
        1 => {
            aim_at(world, block.as_vec3() + Vec3::new(0.5, 0.5, 0.5));
            let client = world.resource::<ClientWorld>();
            let stood = client.registry.id("base:chopping_block").is_some_and(|b| client.block(block) == Some(b));
            let there = world.resource::<LocalPlayer>().position.distance(Vec3::new(block.x as f32 + 0.5, block.y as f32, block.z as f32 - 2.5)) < 0.5;
            if stood && there && carried(world, log) == 2 && drawn(world, block) && gate.shot(world, "chopping_block").is_some() {
                world.resource_mut::<UiState>().inventory_open = true;
                gate.advance(now);
            } else if outwaited {
                gate.check("chopping block", Err(format!("the chopping block ({stood}), the spot ({there}) or two oak logs never came, or it never drew, in {CHOP_WAIT} seconds")));
                end(world, gate);
            }
        }
        2 => {
            // Within reach, by the client's own look (`NearTags`, kept while the inventory
            // is open): a log is laid in the grid.
            let near = world.resource::<crate::hud::inventory::NearTags>().has("chopping_block");
            if near {
                let from = world.resource::<LocalPlayer>().inventory.slots.iter().position(|s| s.is_some_and(|s| Some(s.item) == log));
                if let Some(from) = from {
                    let actions = world.resource::<crate::Actions>().0.clone();
                    let target = voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: voxmar_engine::inventory::CRAFT as u8, count: 1 };
                    world.resource::<Net>().propose(actions.expect("core:transfer"), target);
                    gate.advance(now);
                }
            } else if outwaited {
                gate.check("chopping block", Err(format!("with the inventory open the client never found the chopping block within reach in {CHOP_WAIT} seconds")));
                end(world, gate);
            }
        }
        3 => {
            // The grid shows what the near recipe makes; crafted, the server gives it.
            let player = world.resource::<LocalPlayer>();
            let grid = player.inventory.grid();
            if grid.iter().flatten().count() == 1 && grid.contains(&log) {
                let near = world.resource::<crate::hud::inventory::NearTags>();
                let made = content.recipes.find_near(&grid, |tag| near.has(tag)).map(|r| (r.output, r.count));
                if made == planks.map(|p| (p, 6)) {
                    let actions = world.resource::<crate::Actions>().0.clone();
                    world.resource::<Net>().propose(actions.expect("core:craft"), voxmar_engine::action::ActionTarget::Slot(0));
                    gate.advance(now);
                } else {
                    gate.check("chopping block", Err(format!("an oak log in the grid by a chopping block shows {made:?}, not six oak planks")));
                    end(world, gate);
                }
            } else if waited {
                gate.check("chopping block", Err(format!("an oak log never reached the crafting grid in {CHOP_WAIT} server seconds")));
                end(world, gate);
            }
        }
        _ => {
            let split = carried(world, planks);
            let empty = world.resource::<LocalPlayer>().inventory.grid().iter().all(Option::is_none);
            if split == 6 && empty {
                gate.check("chopping block", Ok("set by the fire it draws; the client found it within reach, and a log split there gave six planks, as the grid showed".to_owned()));
                end(world, gate);
            } else if waited {
                gate.check("chopping block", Err(format!("a log crafted by a chopping block gave {split} oak planks (grid emptied {empty}), not six, in {CHOP_WAIT} server seconds")));
                end(world, gate);
            }
        }
    }
}

/// The recipe lookup as a survival player does it: type "pick" in the inventory's search,
/// click the stone pickaxe's recipe, see "Look It Up" finish, then open the quest book on
/// the stone tools quest, whose goal shows the pickaxe's recipe inline, look an input up
/// from there and come Back to the book.
fn look_up(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::creative::{Catalogue, Cell, SearchField};
    use crate::hud::quests::QuestView;
    use crate::hud::recipes::{RecipeViewer, ViewerButton, ViewerItem};
    use crate::ui::widgets::TextField;
    const QUEST: &str = "base:look_it_up";
    const BOOK_QUEST: &str = "base:stone_in_hand";
    let blocks = world.resource::<Launch>().content.blocks.clone();
    let Some(pickaxe) = blocks.id("base:stone_pickaxe") else {
        gate.check("recipe lookup", Err("no stone pickaxe in the content".into()));
        gate.next(Step::Refusal, now);
        return;
    };
    let fail = |world: &mut World, gate: &mut Gate, name: &str, why: String| {
        gate.check(name, Err(why));
        world.resource_mut::<RecipeViewer>().trail.clear();
        let mut ui = world.resource_mut::<UiState>();
        (ui.recipes_open, ui.inventory_open, ui.quests_open) = (false, false, false);
        gate.next(Step::Refusal, now);
    };
    match gate.phase {
        0 => {
            if world.resource::<LocalPlayer>().allow_flight && gate.server_in_phase() < 3.0 {
                return;
            }
            let mut ui = world.resource_mut::<UiState>();
            (ui.menu_open, ui.character_open, ui.console_open, ui.quests_open, ui.inventory_open) = (false, false, false, false, true);
            // As typed in the search box.
            for mut field in world.query_filtered::<&mut TextField, With<SearchField>>().iter_mut(world) {
                field.value = "pick".into();
            }
            world.resource_mut::<Catalogue>().query = "pick".into();
            gate.advance(now);
        }
        1 => {
            let found = world.resource::<Catalogue>().found.clone();
            let recipes_only = world.resource::<Catalogue>().recipes;
            if found.contains(&pickaxe) && recipes_only {
                if gate.shot(world, "lookup-search").is_none() {
                    return;
                }
                gate.check("recipe lookup: search", Ok(format!("\"pick\" in the inventory's search finds {} items, the stone pickaxe among them", found.len())));
                // Give the visible search cell the pointer on the next Update.
                let cell = world.query::<(Entity, &Cell, &InheritedVisibility)>().iter(world)
                    .find(|(_, cell, visible)| visible.get() && found.get(cell.0) == Some(&pickaxe))
                    .map(|(entity, _, _)| entity);
                let Some(cell) = cell else {
                    fail(world, gate, "recipe lookup: search", "the pickaxe has no visible search cell".into());
                    return;
                };
                world.resource_mut::<UiClicks>().next = Some((cell, MouseButton::Left));
                gate.advance(now);
            } else if in_phase > 3.0 {
                fail(world, gate, "recipe lookup: search", format!("\"pick\" found {found:?} (recipe search {recipes_only})"));
            }
        }
        2 => {
            let done = world.resource::<LocalPlayer>().quests.iter().any(|q| q.name == QUEST && q.done);
            if done {
                if gate.shot(world, "lookup-viewer").is_none() {
                    return;
                }
                gate.check("recipe lookup: quest", Ok("opening the stone pickaxe's recipe finished Look It Up".into()));
                world.resource_mut::<RecipeViewer>().trail.clear();
                let mut ui = world.resource_mut::<UiState>();
                (ui.recipes_open, ui.inventory_open, ui.quests_open) = (false, false, true);
                *world.resource_mut::<QuestView>() = QuestView { chapter: 0, picked: Some(BOOK_QUEST.into()), ..default() };
                gate.advance(now);
            } else if gate.server_in_phase() > UNANSWERED {
                fail(world, gate, "recipe lookup: quest", "the viewer opened on the pickaxe and the server never finished Look It Up".into());
            }
        }
        3 => {
            if in_phase < 0.5 {
                return;
            }
            // The first way to make a pickaxe, and the inputs it should draw.
            let inputs: Vec<BlockId> = world
                .resource::<RecipeViewer>()
                .book()
                .and_then(|book| book.making(pickaxe).first().map(|entry| entry.method.clone()))
                .map(|method| match method {
                    voxmar_engine::recipe_book::Method::Craft { variants, .. } => variants[0].iter().flatten().copied().collect(),
                    _ => Vec::new(),
                })
                .unwrap_or_default();
            let drawn: Vec<BlockId> = world.query::<(&ViewerItem, &InheritedVisibility)>().iter(world).filter(|(_, shown)| shown.get()).map(|(item, _)| item.0).collect();
            if inputs.is_empty() || !inputs.iter().all(|item| drawn.contains(item)) || !drawn.contains(&pickaxe) {
                if in_phase > 3.0 {
                    fail(world, gate, "recipe lookup: inline", format!("the pickaxe's quest page draws {drawn:?}, its recipe needs {inputs:?}"));
                }
                return;
            }
            if gate.shot(world, "lookup-quest").is_none() {
                return;
            }
            gate.check("recipe lookup: inline", Ok(format!("the quest page draws the pickaxe's recipe: {} inputs and the result, each a clickable icon", inputs.len())));
            // Press the drawn input's button through the viewer's actual click handler.
            let input = inputs[0];
            let cell = world.query::<(Entity, &ViewerItem, &InheritedVisibility)>().iter(world)
                .find(|(_, item, visible)| visible.get() && item.0 == input).map(|(entity, _, _)| entity).unwrap();
            world.resource_mut::<UiClicks>().next = Some((cell, MouseButton::Left));
            gate.counted = input.0 as u32;
            gate.advance(now);
        }
        4 => {
            if in_phase < 0.3 {
                return;
            }
            let shown = world.resource::<RecipeViewer>().trail.last().map(|look| look.item.0 as u32);
            let opened = shown == Some(gate.counted) && world.resource::<UiState>().recipes_open;
            if !opened {
                fail(world, gate, "recipe lookup: click", "the input's button did not open its recipe".into());
                return;
            }
            let back = world.query::<(Entity, &ViewerButton, &InheritedVisibility)>().iter(world)
                .find(|(_, button, visible)| visible.get() && **button == ViewerButton::Back).map(|(entity, _, _)| entity).unwrap();
            world.resource_mut::<UiClicks>().next = Some((back, MouseButton::Left));
            gate.advance(now);
        }
        5 => {
            let ui = world.resource::<UiState>();
            let back = ui.quests_open && !ui.recipes_open && world.resource::<RecipeViewer>().trail.is_empty();
            gate.check(
                "recipe lookup: back",
                if back { Ok("an input clicked in the quest book opens the viewer and its Back button returns to the book".into()) } else { Err("Back did not return to the quest book".into()) },
            );
            if !back {
                fail(world, gate, "recipe lookup: uses", "Back did not return to the quest book".into());
                return;
            }
            let cell = world.query::<(Entity, &ViewerItem, &InheritedVisibility)>().iter(world)
                .find(|(_, item, visible)| visible.get() && item.0.0 as u32 == gate.counted).map(|(entity, _, _)| entity);
            let Some(cell) = cell else { return };
            world.resource_mut::<UiClicks>().next = Some((cell, MouseButton::Right));
            gate.advance(now);
        }
        _ => {
            let viewer = world.resource::<RecipeViewer>();
            let uses = viewer.trail.last().is_some_and(|look| look.uses && look.item.0 as u32 == gate.counted);
            gate.check("recipe lookup: uses", if uses { Ok("right-clicking a quest recipe's input opens its uses".into()) } else { Err("right-click did not open the input's uses".into()) });
            world.resource_mut::<UiState>().quests_open = false;
            world.resource_mut::<UiState>().recipes_open = false;
            world.resource_mut::<RecipeViewer>().trail.clear();
            gate.next(Step::Refusal, now);
        }
    }
}

/// The language key of why rice is refused on the dry plot at `plot`: the engine's own
/// judge (`crop::unsuited`) over the climate worldgen gives its column, as the server reads
/// it, full light and unwatered.
fn refusal_reason(world: &World, plot: IVec3, rice: BlockId) -> Result<&'static str, String> {
    let generator = world.resource::<crate::render::lod::WorldGenerator>().get().cloned().ok_or("no world generator to read the plot's climate")?;
    let launch = world.resource::<Launch>();
    let crops = &launch.content.crops;
    let crop = crops.get(crops.planted_by(rice).ok_or("rice plants no crop")?);
    let site = generator.site(plot.x, plot.z);
    let at = voxmar_engine::crop::Conditions { temperature: site.temperature, moisture: site.soil_moisture(), light: 15, fertile: false, watered: false };
    let why = voxmar_engine::crop::unsuited(&crop.def, at).ok_or_else(|| format!("rice would grow on the dry plot ({:.1} °C)", site.temperature))?;
    Ok(why.key())
}

/// The plot `refusal` sows, on the test ground's dirt within reach of the player and more
/// than four blocks from its pool (x 3 to 5), so nothing waters it.
const DRY_PLOT: IVec3 = IVec3::new(-2, -1, 2);
/// Server seconds the refusal may take to come, inside the step's own limit.
const REFUSAL_WAIT: f32 = 8.0;
/// The ground beside the dry plot where the well goes, from the plot.
const WELL_BY: IVec3 = IVec3::new(1, 0, 0);

/// Rice sown on a dry plot of the test ground: the server refuses it and says why, as a
/// refusal, which shows by the hotbar with the others, naming the crop as the player holds
/// it ("Rice"), not its growing stage. The reason is judged word for word against the one
/// the plot's climate gives (`refusal_reason`): too cold on today's spawn (about 10 °C,
/// where rice wants 18 to 34), no water on warmer ground.
fn refusal(world: &mut World, gate: &mut Gate, now: f32, _in_phase: f32) {
    let plot = gate.feet + DRY_PLOT;
    let rice = world.resource::<ClientWorld>().registry.id("base:rice");
    let Some(rice) = rice else {
        gate.check("refusal", Err("no rice in the content".into()));
        gate.next(Step::Merged, now);
        return;
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            // Plain dirt round it, so no water the earlier steps left is near.
            let (low, high) = (plot - IVec3::new(4, 2, 4), plot + IVec3::new(4, 0, 4));
            for line in [
                "skill farming 10".to_owned(),
                "give rice 2".to_owned(),
                format!("fill {} {} {} {} {} {} dirt", low.x, low.y, low.z, high.x, high.y, high.z),
                format!("setblock {} {} {} farmland", plot.x, plot.y, plot.z),
                // Nothing between the eye and the plot (earlier steps place blocks here).
                format!("fill {} {} {} {} {} {} air", plot.x, plot.y + 1, gate.feet.z, gate.feet.x, plot.y + 3, plot.z),
            ] {
                net.command(&line);
            }
            gate.advance(now);
        }
        1 => {
            let farmland = world.resource::<ClientWorld>().registry.id("base:farmland");
            let tilled = farmland.is_some() && world.resource::<ClientWorld>().block(plot) == farmland;
            // Rice takes Farming 10: below it the client refuses it itself, for the level.
            let level = skill_level(world, "core:farming").0 >= 10;
            // The aim is from the feet: the player stands there first, whatever the steps
            // before left (a teleport of theirs still on its way on a server behind once
            // put the eye where the ray met the ground short of the plot).
            let posed = open_scene_pose(world, gate);
            match (tilled && level && posed, in_hotbar(world, &mut gate.food_moved, rice)) {
                (true, Some(slot)) => {
                    world.resource_mut::<LocalPlayer>().selected = slot;
                    gate.advance(now);
                }
                _ if gate.server_in_phase() > UNANSWERED => {
                    let at = if posed { "stood at the feet".to_owned() } else { unsettled(world, gate) };
                    gate.check("refusal", Err(format!("the farmland ({tilled}), Farming 10 ({level}) or rice in the hotbar never came, or {at}")));
                    gate.next(Step::Merged, now);
                }
                (ready, slot) => {
                    if gate.frames % 120 == 0 {
                        info!("gate: refusal: farmland {tilled}, Farming 10 {level}, at the feet {posed} (ready {ready}), rice in the hotbar at {slot:?}");
                    }
                }
            }
        }
        2 => {
            // Just under the farmland's top, so the ray meets its top face.
            aim_at(world, plot.as_vec3() + Vec3::new(0.5, 0.9, 0.5));
            let name = {
                let (locale, launch) = (world.resource::<Locale>(), world.resource::<Launch>());
                locale.block(launch.content.blocks.def(rice))
            };
            // Why the server should refuse it, from the plot's own climate (worldgen's site,
            // as the server reads it) and a dry plot: too cold where the ground is a hard stop
            // for rice, else no water. So the check holds wherever the world puts spawn.
            let reason = match refusal_reason(world, plot, rice) {
                Ok(reason) => reason,
                Err(why) => {
                    gate.check("refusal", Err(why));
                    gate.next(Step::Merged, now);
                    return;
                }
            };
            let cold = {
                let (locale, launch) = (world.resource::<Locale>(), world.resource::<Launch>());
                let blocks = &launch.content.blocks;
                locale.render(&voxmar_engine::lang::Words::key(reason).with("crop", voxmar_engine::lang::Words::Block(blocks.def(rice).name.clone())), blocks)
            };
            let said = world.resource::<crate::hud::skills::SkillToasts>().refusals().find(|why| why.contains(&name)).map(str::to_owned);
            if let Some(said) = said {
                mouse(world, MouseButton::Right, false);
                let result = if said == cold { Ok(format!("sowing rice was refused for its reason ({reason}): \"{said}\"")) } else { Err(format!("sowing rice was refused, but not as {reason} (\"{cold}\"): \"{said}\"")) };
                gate.check("refusal", result);
                gate.advance(now);
            } else if gate.waited_out(REFUSAL_WAIT) {
                mouse(world, MouseButton::Right, false);
                let (target, held) = {
                    let (target, player) = (world.resource::<crate::interact::Target>(), world.resource::<LocalPlayer>());
                    (target.hit.as_ref().map(|hit| (hit.block - gate.feet, hit.face)), player.inventory.get(player.selected).map(|s| s.item))
                };
                let from = world.resource::<LocalPlayer>().position - gate.feet.as_vec3();
                gate.check("refusal", Err(format!("sowing rice showed no refusal naming {name} (aimed at {target:?} from the feet, standing at {from:.2} from them, holding {held:?})")));
                gate.advance(now);
            } else {
                gate.tap(world, true, now);
                if gate.frames % 120 == 0 {
                    let (target, player, toasts, client) =
                        (world.resource::<crate::interact::Target>(), world.resource::<LocalPlayer>(), world.resource::<crate::hud::skills::SkillToasts>(), world.resource::<ClientWorld>());
                    let above = client.block(plot + IVec3::Y).map(|b| client.registry.def(b).name.clone());
                    info!(
                        "gate: refusal: aiming at {:?}, holding {:?}, above the plot {above:?}, refusals showing {:?}",
                        target.hit.as_ref().map(|hit| (hit.block - gate.feet, hit.face)),
                        player.inventory.get(player.selected).map(|s| s.item),
                        toasts.refusals().collect::<Vec<_>>()
                    );
                }
            }
        }
        // A well set down by the plot, from the hand, as a player would by a field far
        // from water (it waters it: `a_well_waters_the_field_about_it`).
        3 => {
            if gate.settled == 0 {
                world.resource::<Net>().command("give well 1");
                gate.settled = 1;
            }
            if select(world, "base:well") {
                let actions = world.resource::<crate::Actions>().0.clone();
                world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("core:place"), voxmar_engine::action::ActionTarget::Block { block: plot + WELL_BY, face: voxmar_engine::coords::Face::PosY });
                gate.settled = 0;
                gate.advance(now);
            } else if gate.server_in_phase() > REFUSAL_WAIT {
                gate.check("well", Err(format!("no well came to hand {:.1} server s after it was given (carried, first slot: {:?})", gate.server_in_phase(), holding(world, "base:well"))));
                to_phase(gate, 5, now);
            }
        }
        4 => {
            let cell = plot + WELL_BY + IVec3::Y;
            let placed = {
                let client = world.resource::<ClientWorld>();
                client.block(cell).map(|b| client.registry.def(b).name.clone())
            };
            if placed.as_deref().is_some_and(|name| name.starts_with("base:well")) {
                if gate.shot(world, "well").is_none() {
                    return;
                }
                let solid = world.resource::<ClientWorld>().is_solid(cell);
                gate.check("well", if solid { Ok("a well from the hand stands on the ground by the plot, drawn in the shot `well`".into()) } else { Err("the well stands, but not solid on the client".into()) });
                gate.advance(now);
            } else if gate.server_in_phase() > REFUSAL_WAIT {
                gate.check("well", Err(format!("{:.1} server s after placing, the well's cell holds {placed:?}", gate.server_in_phase())));
                gate.advance(now);
            }
        }
        _ => {
            let net = world.resource::<Net>();
            let well = plot + WELL_BY + IVec3::Y;
            net.command("clear rice");
            net.command("clear well");
            net.command(&format!("setblock {} {} {} air", well.x, well.y, well.z));
            net.command(&format!("setblock {} {} {} dirt", plot.x, plot.y, plot.z));
            gate.next(Step::Merged, now);
        }
    }
}

/// `give oregano` hands out kitchen herbs (oregano is a wild plant now; its item merged
/// with seven other herbs), and neither the creative catalogue's rule nor the journal lists
/// the plant while both list the item.
fn merged(world: &mut World, gate: &mut Gate, now: f32, _in_phase: f32) {
    let blocks = world.resource::<ClientWorld>().registry.clone();
    let (Some(oregano), Some(herbs)) = (blocks.id("base:oregano"), blocks.id("base:kitchen_herbs")) else {
        gate.check("merged", Err("no oregano or kitchen herbs in the content".into()));
        gate.next(Step::Identity, now);
        return;
    };
    match gate.phase {
        0 => {
            world.resource::<Net>().command("give oregano 2");
            gate.advance(now);
        }
        1 => {
            let held = |item: BlockId| world.resource::<LocalPlayer>().inventory.slots.iter().flatten().any(|s| s.item == item);
            let catalogue = world.resource::<crate::hud::creative::Catalogue>();
            if held(herbs) && let (Some(has_herbs), Some(has_oregano)) = (catalogue.lists(herbs), catalogue.lists(oregano)) {
                let verdict = if held(oregano) {
                    Err("`give oregano` also gave the oregano plant".to_owned())
                } else if has_oregano || !has_herbs || blocks.listed(oregano) || !blocks.listed(herbs) {
                    Err(format!("the creative catalogue lists oregano ({has_oregano}) or not kitchen herbs ({has_herbs})"))
                } else {
                    Ok("`give oregano` gave kitchen herbs; the creative catalogue lists the herbs, not the plant".to_owned())
                };
                gate.check("merged", verdict);
                gate.advance(now);
            } else if gate.server_in_phase() > UNANSWERED {
                gate.check("merged", Err("`give oregano` gave no kitchen herbs, or the catalogue never read the content".into()));
                gate.advance(now);
            }
        }
        _ => {
            world.resource::<Net>().command("clear kitchen_herbs");
            gate.next(Step::Identity, now);
        }
    }
}

fn identity(world: &mut World, gate: &mut Gate, now: f32) {
    let net = world.resource::<Net>();
    let content = world.resource::<Launch>().content.fingerprint();
    let failed = if let Some(peer) = &mut gate.identity_peer { peer.pump().err() } else { None };
    if let Some(reason) = failed {
        gate.check("identity", Err(reason));
        gate.identity_peer = None;
        gate.next(Step::Effects, now);
        return;
    }
    match gate.phase {
        0 => {
            gate.identity_peer = Observer::join_named(net, content, "Gate-Keeper");
            gate.advance(now);
        }
        1 => {
            if let Some(id) = gate.identity_peer.as_ref().and_then(|p| p.id)
                && gate.observer.as_ref().is_some_and(|p| p.names.get(&id).is_some_and(|n| n == "Gate-Keeper"))
            {
                gate.identity_id = Some(id);
                // Closing the channel uses the server's normal save-and-leave path.
                gate.identity_peer = None;
                gate.advance(now);
            }
        }
        2 => {
            // A player disappears only after leave saved their state and removed them.
            if gate.identity_id.is_some_and(|id| gate.observer.as_ref().is_some_and(|p| !p.names.contains_key(&id))) {
                gate.identity_peer = Observer::join_named(net, content, "GATE-KEEPER");
                gate.advance(now);
            }
        }
        _ => {
            if let Some(id) = gate.identity_peer.as_ref().and_then(|p| p.id)
                && let Some(name) = gate.observer.as_ref().and_then(|p| p.names.get(&id))
            {
                gate.check("identity", if name == "Gate-Keeper" {
                    Ok("GATE-KEEPER rejoined with the saved Gate-Keeper server identity".into())
                } else {
                    Err(format!("GATE-KEEPER rejoined as {name}, losing the saved identity"))
                });
                gate.identity_peer = None;
                gate.next(Step::Effects, now);
                return;
            }
        }
    }
    if gate.server_in_phase() > UNANSWERED {
        gate.check("identity", Err(format!("saved-name reconnect did not complete phase {}", gate.phase)));
        gate.identity_peer = None;
        gate.next(Step::Effects, now);
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
            gate.next(Step::Celebrate, now);
        }
        _ => {}
    }
}

/// A tenth Mining level, as the server would say it, pops the milestone banner (its
/// words checked) and throws motes; then a quest and a level asked together show in turn.
fn celebrate(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::celebrate::{BannerText, Celebrations, Stage};
    let launch = world.resource::<Launch>().clone();
    let skills = &launch.content.skills;
    let Some(skill) = skills.named("core", "mining").or_else(|| skills.ids().next()) else {
        gate.check("celebrate", Err("no skill in the content".into()));
        gate.next(Step::Look, now);
        return;
    };
    let banner = |world: &mut World| world.query::<(&BannerText, &Text)>().iter(world).map(|(line, text)| (*line, text.0.clone())).collect::<Vec<_>>();
    let shown = |world: &mut World| world.resource::<Stage>().showing_level();
    match gate.phase {
        0 => {
            *world.resource_mut::<Stage>() = Stage::default();
            let every = world.resource::<Celebrations>().milestone_every.max(1);
            world.resource_mut::<crate::hud::skills::SkillToasts>().level_up(skill, every);
            gate.advance(now);
        }
        // Drawn the frame after it is asked for; the motes' bursts all thrown within a second.
        1 if in_phase > 1.0 => {
            let locale = world.resource::<Locale>().clone();
            let every = world.resource::<Celebrations>().milestone_every.max(1);
            let wanted = [locale.get("celebrate.milestone").to_owned(), locale.skill(skills.get(skill)), locale.format("skills.level_up", &[("level", &every)])];
            let lines = banner(world);
            let missing: Vec<_> = wanted.iter().filter(|w| !lines.iter().any(|(_, text)| text == *w)).collect();
            let thrown = world.resource::<Stage>().thrown;
            let amount = world.resource::<crate::render::particles::Particles>().amount;
            gate.check(
                "celebrate: milestone",
                if !missing.is_empty() {
                    Err(format!("the banner lacks {missing:?} (it reads {lines:?})"))
                } else if amount > 0.0 && thrown == 0 {
                    Err("no motes were thrown".into())
                } else {
                    Ok(format!("the banner reads {wanted:?}, {thrown} motes thrown"))
                },
            );
            // A quest finished by reaching a level: the quest first.
            let quest = launch.content.quests.iter().next().map(|q| q.name.clone()).unwrap_or_default();
            *world.resource_mut::<Stage>() = Stage::default();
            world.resource_mut::<Stage>().ask_quest(quest);
            world.resource_mut::<crate::hud::skills::SkillToasts>().level_up(skill, 3);
            gate.advance(now);
        }
        2 if in_phase > 0.3 => {
            let first = shown(world);
            if first.is_some() {
                gate.check("celebrate: in turn", Err(format!("the level showed at once, over the quest ({first:?})")));
                gate.next(Step::Look, now);
                return;
            }
            gate.advance(now);
        }
        3 => {
            let hold = world.resource::<Celebrations>().quest.hold;
            let level = shown(world);
            if level.is_none() && in_phase < hold + 1.0 {
                return;
            }
            gate.check("celebrate: in turn", match level { Some(3) => Ok(format!("the quest showed first, the level after its turn ({hold} s)")), other => Err(format!("after the quest came {other:?}, not level 3")) });
            *world.resource_mut::<Stage>() = Stage::default();
            gate.next(Step::Look, now);
        }
        _ => {}
    }
}

/// Looking at a plank block beside the player names it ("Oak Planks"); looking at a lumen
/// snail seven blocks off, past any blow's reach, names it with its level; a stone wall put
/// between hides it again, and the block named is the wall's. Shots `look-block` and
/// `look-creature` show the label at the top left.
fn look_name(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let pad = gate.feet + IVec3::new(0, 0, 14);
    let planks = pad + IVec3::new(1, 0, 2);
    let species = world.resource::<Launch>().content.creatures.find(CREATURE);
    let Some(species) = species else {
        gate.check("look", Err(format!("no `{CREATURE}` in the content")));
        gate.next(Step::Guides, now);
        return;
    };
    let snail = |world: &World| world.resource::<crate::creatures::Creatures>().views().find(|v| v.species == species).copied();
    let named = |world: &World| world.resource::<crate::hud::target::Readout>().clone();
    match gate.phase {
        // On the creatures' pad (clear stone, air above): a plank block close by, once what the
        // creatures step left there (a grazer or two in the walls, the bird) is gone, since a
        // wanderer on the look ray would take the name.
        0 => {
            let net = world.resource::<Net>();
            for species in ["mouflon", "rabbit", "lumen_snail", "shambler", "goldfinch"] {
                net.command(&format!("creatures remove {species}"));
            }
            net.command(&format!("tp {} {} {}", pad.x as f32 + 0.5, pad.y, pad.z as f32 + 4.5));
            // The bird flies about its home on the pad, out of reach of the first remove.
            net.command("creatures remove goldfinch");
            net.command(&format!("setblock {} {} {} oak_planks", planks.x, planks.y, planks.z));
            net.command("time set noon");
            gate.advance(now);
        }
        1 if in_phase > 0.5 => {
            // Nothing alive on the pad before the first look (given a few server seconds to go).
            let middle = pad.as_vec3();
            let crowded = world.resource::<crate::creatures::Creatures>().views().any(|v| v.position.distance(middle) < 12.0);
            if crowded && gate.server_in_phase() < 6.0 {
                return;
            }
            let client = world.resource::<ClientWorld>();
            let Some(block) = client.block(planks).filter(|b| !b.is_air()) else {
                if gate.server_in_phase() > 10.0 {
                    gate.check("look: block", Err("the plank block never reached the client".into()));
                    gate.next(Step::Guides, now);
                }
                return;
            };
            let wanted = world.resource::<crate::lang::Locale>().block(client.registry.def(block));
            aim(world, planks);
            let readout = named(world);
            if readout.name != wanted && in_phase < 5.0 {
                return;
            }
            if gate.shot(world, "look-block").is_some() {
                let result = if readout.name == wanted && readout.creature.is_none() {
                    Ok(format!("the block looked at is named {:?} at the top left", readout.name))
                } else {
                    Err(format!("looking at the planks the label read {:?}, not {wanted:?}", readout.name))
                };
                gate.check("look: block", result);
                world.resource::<Net>().command(&format!("summon {CREATURE} 1 {} {} {}", pad.x, pad.y, pad.z - 3));
                gate.advance(now);
            }
        }
        2 => {
            let Some(view) = snail(world) else {
                if gate.server_in_phase() > 10.0 {
                    gate.check("look: creature", Err("the snail never reached the client".into()));
                    gate.next(Step::Guides, now);
                }
                return;
            };
            aim_at(world, view.position + Vec3::Y * 0.2);
            let readout = named(world);
            let distance = world.resource::<LocalPlayer>().eye().distance(view.position);
            let reach = world.resource::<Launch>().content.combat.feel.touch;
            if readout.creature != Some(view.id) && in_phase < 5.0 {
                return;
            }
            if gate.shot(world, "look-creature").is_some() {
                let wanted = {
                    let (launch, locale) = (world.resource::<Launch>(), world.resource::<crate::lang::Locale>());
                    locale.creature(&launch.content.creatures.get(species).def)
                };
                let result = if readout.creature == Some(view.id) && readout.name == wanted && !readout.level.is_empty() && distance > reach {
                    Ok(format!("a snail {distance:.1} blocks off is named {:?}, {}", readout.name, readout.level))
                } else {
                    Err(format!("looking at a snail {distance:.1} blocks off (reach {reach}) the label read {:?} {:?} for creature {:?}", readout.name, readout.level, readout.creature))
                };
                gate.check("look: creature", result);
                // A wall put between: two blocks wide, three high.
                let net = world.resource::<Net>();
                net.command(&format!("fill {} {} {} {} {} {} stone", pad.x - 1, pad.y, pad.z, pad.x + 1, pad.y + 2, pad.z));
                gate.advance(now);
            }
        }
        3 if in_phase > 1.0 => {
            let wall = pad + IVec3::new(0, 1, 0);
            if world.resource::<ClientWorld>().block(wall).is_none_or(|b| b.is_air()) && gate.server_in_phase() < 10.0 {
                return;
            }
            let Some(view) = snail(world) else { return };
            aim_at(world, view.position + Vec3::Y * 0.2);
            let readout = named(world);
            let result = if readout.creature.is_none() {
                Ok(format!("the snail behind a wall is not named; the wall is: {:?}", readout.name))
            } else {
                Err(format!("the snail behind a wall was named {:?}", readout.name))
            };
            if in_phase > 1.5 {
                gate.check("look: wall hides", result);
                world.resource::<Net>().command("creatures remove lumen_snail");
                gate.next(Step::Guides, now);
            }
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
            gate.next(Step::Prayers, now);
        }
    }
}

/// The prayer book, played: C opens the character screen and its Prayers button the book, a
/// row for every prayer; a click on Bark Skin turns it on as the server says (waited for by
/// its clock); with the screen shut the strip over the hotbar names it; and the first quick
/// key turns it off again.
fn prayers(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::prayers::{PrayerBook, book_rows, open_button, row_of, strip_names};
    let (character, quick) = {
        let settings = world.resource::<crate::settings::Settings>();
        (settings.character_key.0, settings.prayer_keys[0].0)
    };
    let Some(bark) = world.resource::<Launch>().content.prayers.id("base:bark_skin") else {
        gate.check("prayers", Err("the content lacks base:bark_skin".into()));
        gate.next(Step::Home, now);
        return;
    };
    let on = world.resource::<crate::fight::Fighting>().prayers.on.contains(&bark);
    let quit = |world: &mut World, gate: &mut Gate, what: &str, why: String| {
        gate.check(what, Err(why));
        key(world, character, false);
        key(world, quick, false);
        world.resource_mut::<UiState>().character_open = false;
        world.resource_mut::<PrayerBook>().0 = false;
        gate.next(Step::Home, now);
    };
    match gate.phase {
        0 => {
            // The server tells the points once a player is in; the book has nothing to show before.
            if world.resource::<crate::fight::Fighting>().prayers.most == 0 {
                if gate.server_in_phase() > UNANSWERED {
                    quit(world, gate, "prayers: points", "the server never told the prayer points".into());
                }
                return;
            }
            key(world, character, true);
            gate.advance(now);
        }
        1 => {
            key(world, character, false);
            let button = open_button(world);
            if !world.resource::<UiState>().character_open || button.is_none() {
                if in_phase < 2.0 {
                    return;
                }
                quit(world, gate, "prayers: screen", "the character key did not open the screen with its Prayers button".into());
                return;
            }
            world.resource_mut::<UiClicks>().next = button.map(|button| (button, MouseButton::Left));
            gate.advance(now);
        }
        2 => {
            let (rows, wanted) = (book_rows(world), world.resource::<Launch>().content.prayers.len());
            let open = world.resource::<PrayerBook>().0;
            if !open || rows != wanted {
                if in_phase < 2.0 {
                    return;
                }
                quit(world, gate, "prayers: book", format!("the Prayers button opened the book {open}, with {rows} rows of {wanted} prayers"));
                return;
            }
            gate.check("prayers: book", Ok(format!("the Prayers button opened the book, a row for each of {wanted} prayers")));
            world.resource_mut::<UiClicks>().next = row_of(world, bark).map(|row| (row, MouseButton::Left));
            gate.advance(now);
        }
        3 => {
            if !on {
                if gate.server_in_phase() < UNANSWERED {
                    return;
                }
                quit(world, gate, "prayers: click", "a click on Bark Skin's row did not turn it on".into());
                return;
            }
            gate.check("prayers: click", Ok("a click on Bark Skin's row turned it on, as the server says".into()));
            world.resource_mut::<UiState>().character_open = false;
            gate.advance(now);
        }
        4 => {
            let name = world.resource::<Locale>().prayer("base:bark_skin");
            let shown = strip_names(world);
            if !shown.as_ref().is_some_and(|names| names.contains(&name)) {
                if in_phase < 2.0 {
                    return;
                }
                quit(world, gate, "prayers: strip", format!("the strip over the hotbar reads {shown:?}, not {name}"));
                return;
            }
            gate.check("prayers: strip", Ok(format!("the strip over the hotbar names {name}")));
            key(world, quick, true);
            gate.advance(now);
        }
        _ => {
            key(world, quick, false);
            if on {
                if gate.server_in_phase() < UNANSWERED {
                    return;
                }
                let locked = world.resource::<UiState>().cursor_locked;
                quit(world, gate, "prayers: quick key", format!("the quick key did not turn Bark Skin off (cursor locked {locked})"));
                return;
            }
            gate.check("prayers: quick key", Ok("the quick key turned Bark Skin off again".into()));
            gate.next(Step::Home, now);
        }
    }
}

/// A sleeping bag and a torch beside the player, slept in: the bed is their home and it has
/// a light, so the Camp step (docs/home.md) comes on as a chip, each chip with its own icon
/// (shot `home-chips`), the chat names it and what to add next, and the sleep gives a rest. Then the way home: with the mark's hiding distance
/// set to nothing, the hearth mark shows how far the bag is. Then crafting at home: four
/// planks shift clicked into a barrel beside the bag, none left in the pack, Lay on the
/// crafting table's recipe fills the grid from the barrel. The blocks go again after, and
/// the rest with them.
fn home(world: &mut World, gate: &mut Gate, now: f32) {
    let stand = world.resource::<LocalPlayer>().position.floor().as_ivec3();
    let (bag, torch, barrel) = (stand + IVec3::new(2, 0, 0), stand + IVec3::new(3, 0, 0), stand + IVec3::new(-2, 0, 0));
    let waited = gate.server_in_phase();
    let leave = |world: &mut World, gate: &mut Gate| {
        world.resource::<Net>().command("effect clear");
        if let Some(style) = gate.home_style.take() {
            *world.resource_mut::<crate::hud::inventory::InterfaceStyle>() = style;
        }
        // The camp stays home for the way home step, which takes it down; one the server
        // never named home that step cannot find, so it goes here.
        if world.resource::<LocalPlayer>().vitals.home != Some(bag) {
            let net = world.resource::<Net>();
            for at in [bag, torch] {
                net.command(&format!("setblock {} {} {} air", at.x, at.y, at.z));
            }
        }
        gate.next(Step::WayHome, now);
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("effect clear");
            net.command(&format!("setblock {} {} {} sleeping_bag", bag.x, bag.y, bag.z));
            net.command(&format!("setblock {} {} {} torch", torch.x, torch.y, torch.z));
            gate.advance(now);
        }
        1 => {
            let placed = {
                let client = world.resource::<ClientWorld>();
                match (client.registry.id("base:sleeping_bag"), client.registry.id("base:torch")) {
                    (Some(b), Some(t)) => Some(client.block(bag) == Some(b) && client.block(torch) == Some(t)),
                    _ => None,
                }
            };
            match placed {
                None => {
                    gate.check("home", Err("no sleeping bag or torch in the content".into()));
                    leave(world, gate);
                }
                Some(true) => {
                    let actions = world.resource::<crate::Actions>().0.clone();
                    world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:sleep"), voxmar_engine::action::ActionTarget::Block { block: bag, face: voxmar_engine::coords::Face::PosY });
                    gate.advance(now);
                }
                Some(false) if waited > 10.0 => {
                    gate.check("home", Err(format!("the sleeping bag and torch were not on the client {waited:.1} server s after they were placed")));
                    leave(world, gate);
                }
                Some(false) => {}
            }
        }
        2 => {
            let has = |world: &World, name: &str| {
                let (player, launch) = (world.resource::<LocalPlayer>(), world.resource::<Launch>());
                launch.content.effects.id(name).is_some_and(|id| player.vitals.effects.iter().any(|a| a.effect == id))
            };
            let (camp, rested) = (has(world, "base:camp"), has(world, "base:rested"));
            let (announced, slept) = {
                let lines: Vec<&str> = world.resource::<crate::hud::console::Console>().since(gate.phase_since).collect();
                (lines.iter().any(|l| l.contains("Camp:") && l.contains("Next:")), lines.iter().any(|l| l.contains("You sleep soundly")))
            };
            if camp && rested && announced && slept {
                if gate.shot(world, "home-chips").is_none() {
                    return;
                }
                gate.check("home: camp", Ok("a bed and a light slept in showed the Camp chip, the chat named it with its next step, and the sleep was a rest".into()));
                // Each with its own icon, not the plain buff's both would share.
                let icons = {
                    let (images, effects) = (world.resource::<crate::hud::HudImages>(), &world.resource::<Launch>().content.effects);
                    ["base:camp", "base:rested"].map(|name| effects.id(name).map(|id| images.effect(id)))
                };
                gate.check("home: icons", if icons[0].is_some() && icons[0] != icons[1] { Ok("the Camp and Rested chips each show an icon of their own".into()) } else { Err(format!("the Camp and Rested chips share an icon: {icons:?}")) });
                // Shown however near and at any hour, for the mark's own checks.
                let mut style = world.resource_mut::<crate::hud::inventory::InterfaceStyle>();
                gate.home_style = Some(style.clone());
                (style.home_hidden_within, style.home_shown_beyond) = (0.0, 0.0);
                gate.advance(now);
            } else if waited > 30.0 {
                gate.check("home: camp", Err(format!("{waited:.1} server s after sleeping at a bag with a torch beside it: Camp chip {camp}, Rested chip {rested}, Camp line with Next {announced}, sleep line {slept}")));
                leave(world, gate);
            }
        }
        // The mark over the bag, with its distance.
        3 => {
            let told = world.resource::<LocalPlayer>().vitals.home;
            let shown = world.query_filtered::<&Visibility, With<crate::hud::home::HomeMark>>().iter(world).any(|v| *v != Visibility::Hidden);
            let words = world.query_filtered::<&Text, With<crate::hud::home::HomeWords>>().iter(world).map(|t| t.0.clone()).next().unwrap_or_default();
            if told == Some(bag) && shown && words.contains(" m") {
                gate.check("home: way", Ok(format!("the server named the bag as home and the hearth mark showed \"{words}\"")));
                // Turned away from it: home lies behind.
                let eye = world.resource::<LocalPlayer>().eye();
                aim_at(world, eye + (eye - bag.as_vec3() - Vec3::splat(0.5)).with_y(0.0));
                gate.advance(now);
            } else if waited > 10.0 {
                gate.check("home: way", Err(format!("{waited:.1} server s after the bag became home: home told {told:?} (bag {bag}), mark shown {shown}, words \"{words}\"")));
                leave(world, gate);
            }
        }
        // Home behind: the mark waits at an edge of the screen. Then, at the real hiding
        // distance, two blocks from home it is gone.
        4 => {
            let edge = world.query::<&crate::hud::home::HomeMark>().iter(world).find_map(|m| m.edge);
            if edge.is_some() {
                gate.check("home: behind", Ok(format!("turned away from home, the mark waited at the {} edge", if edge == Some(-1.0) { "left" } else { "right" })));
                if let Some(style) = gate.home_style.take() {
                    *world.resource_mut::<crate::hud::inventory::InterfaceStyle>() = style;
                }
                gate.advance(now);
            } else if waited > 5.0 {
                let yaw = world.resource::<LocalPlayer>().yaw;
                gate.check("home: behind", Err(format!("turned away from home (yaw {yaw:.2}), the mark was at no edge of the screen")));
                leave(world, gate);
            }
        }
        5 => {
            let shown = world.query_filtered::<&Visibility, With<crate::hud::home::HomeMark>>().iter(world).any(|v| *v != Visibility::Hidden);
            if !shown {
                let within = world.resource::<crate::hud::inventory::InterfaceStyle>().home_hidden_within;
                gate.check("home: near", Ok(format!("within {within} blocks of home the mark was gone")));
                // A barrel at home, four planks in it and none in the pack.
                let net = world.resource::<Net>();
                net.command("clear oak_planks");
                net.command(&format!("give oak_planks {LAY_PLANKS}"));
                net.command(&format!("setblock {} {} {} barrel", barrel.x, barrel.y, barrel.z));
                gate.advance(now);
            } else if waited > 5.0 {
                gate.check("home: near", Err("two blocks from home the mark still showed".into()));
                leave(world, gate);
            }
        }
        // The barrel opened, the planks shift clicked into it.
        6 => {
            let planks = world.resource::<ClientWorld>().registry.id("base:oak_planks");
            let player = world.resource::<LocalPlayer>();
            let stored: u32 = player.inventory.store.as_ref().map_or(0, |s| s.slots.iter().flatten().filter(|x| Some(x.item) == planks).map(|x| x.count as u32).sum());
            let (held, slot) = holding(world, "base:oak_planks");
            if stored == LAY_PLANKS && held == 0 {
                // The crafting table's recipe in the viewer, over the open inventory.
                let table = world.resource::<ClientWorld>().registry.id("base:crafting_table");
                // Opened as a click in it does: a trail with the viewer shut reads as Esc.
                if let Some(table) = table {
                    world.resource_mut::<crate::hud::recipes::RecipeViewer>().open(table, false);
                    world.resource_mut::<UiState>().recipes_open = true;
                }
                gate.advance(now);
            } else if waited > 10.0 {
                let open = player.inventory.store.is_some();
                let laid: u32 = player.inventory.craft.iter().flatten().filter(|x| Some(x.item) == planks).map(|x| x.count as u32).sum();
                let shown = world.resource::<ClientWorld>().registry.id("base:barrel").is_some_and(|b| world.resource::<ClientWorld>().block(barrel) == Some(b));
                gate.check("home: lay", Err(format!("{waited:.1} server s on, the home barrel held {stored} planks (open {open}, on the client {shown}), the pack {held} and the grid {laid}")));
                lay_done(world, barrel);
                leave(world, gate);
            } else if gate.server_now - gate.machines_asked >= 1.0 {
                if player.inventory.store.is_none() {
                    // Whatever is in hand: a store opens to anything but a machine.
                    if world.resource::<ClientWorld>().registry.id("base:barrel").is_some_and(|b| world.resource::<ClientWorld>().block(barrel) == Some(b)) {
                        world.resource_mut::<LocalPlayer>().store_asked = true;
                        let actions = world.resource::<crate::Actions>().0.clone();
                        world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:open_store"), voxmar_engine::action::ActionTarget::Block { block: barrel, face: voxmar_engine::coords::Face::PosY });
                    }
                } else if let Some(slot) = slot {
                    let actions = world.resource::<crate::Actions>().0.clone();
                    world.resource::<Net>().propose(actions.expect("core:quick_move"), voxmar_engine::action::ActionTarget::Slot(slot as u8));
                }
                gate.machines_asked = gate.server_now;
            }
        }
        // Lay pressed: the table's planks come out of the barrel into the grid.
        7 => {
            let button = world.query::<(Entity, &crate::hud::recipes::LayButton, &InheritedVisibility)>().iter(world).find(|(_, lay, visible)| visible.get() && world.resource::<ClientWorld>().registry.id("base:crafting_table") == Some(lay.output)).map(|(entity, ..)| entity);
            if let Some(button) = button {
                // The table's recipe drawn, with its Lay button, before it is pressed.
                if gate.shot(world, "home-lay-recipe").is_none() {
                    return;
                }
                world.resource_mut::<UiClicks>().next = Some((button, MouseButton::Left));
                gate.advance(now);
            } else if waited > 10.0 {
                let ui = world.resource::<UiState>();
                let (inventory, viewer) = (ui.inventory_open, ui.recipes_open);
                let shown = world.resource::<crate::hud::recipes::RecipeViewer>().trail.last().map(|lookup| lookup.item);
                let buttons = world.query::<(&crate::hud::recipes::LayButton, &InheritedVisibility)>().iter(world).map(|(lay, visible)| (lay.output, visible.get())).collect::<Vec<_>>();
                let store = world.resource::<LocalPlayer>().inventory.store.as_ref().map(|s| s.slots.iter().flatten().map(|x| x.count as u32).sum::<u32>());
                gate.check("home: lay", Err(format!("{waited:.1} server s on, no visible Lay button for the crafting table: inventory open {inventory}, viewer open {viewer} on {shown:?}, Lay buttons (item, visible) {buttons:?}, the barrel's slots on the client hold {store:?}")));
                lay_done(world, barrel);
                leave(world, gate);
            }
        }
        _ => {
            let planks = world.resource::<ClientWorld>().registry.id("base:oak_planks");
            let player = world.resource::<LocalPlayer>();
            let laid: u32 = player.inventory.craft.iter().flatten().filter(|x| Some(x.item) == planks).map(|x| x.count as u32).sum();
            let open = player.inventory.store.is_some();
            let held = holding(world, "base:oak_planks").0;
            let ui = world.resource::<UiState>();
            let (inventory, viewer) = (ui.inventory_open, ui.recipes_open);
            // Laid, the barrel's screen closes onto the grid, drawn with the planks in it;
            // the pack held none, so they came from the barrel.
            if laid == LAY_PLANKS && held == 0 && !open && inventory && !viewer {
                if gate.shot(world, "home-lay").is_none() {
                    return;
                }
                gate.check("home: lay", Ok(format!("home-lay-recipe shows the crafting table's recipe with its Lay button; pressed, home-lay shows the inventory's grid holding the {laid} planks from the home barrel (its screen closed, none in the pack)")));
                lay_done(world, barrel);
                leave(world, gate);
            } else if waited > 10.0 {
                gate.check("home: lay", Err(format!("{waited:.1} server s after Lay: {laid} planks in the grid, {held} in the pack, barrel screen open {open}, inventory open {inventory}, viewer open {viewer}")));
                lay_done(world, barrel);
                leave(world, gate);
            }
        }
    }
}

/// The Can Make list under the inventory's slots, played: a log shows planks ready, ready
/// ones first; clicking them lays the log in the grid (the server's `core:lay`); pointed at,
/// the stone pickaxe's line says what stands in its way, in the list's own words.
fn makeable(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::makeable::{Drawn, MakeCell, Reach};
    let waited = gate.server_in_phase();
    let leave = |world: &mut World, gate: &mut Gate| {
        world.resource_mut::<UiState>().inventory_open = false;
        gate.next(Step::Quests, now);
    };
    let ids = {
        let registry = &world.resource::<ClientWorld>().registry;
        ["base:oak_log", "base:oak_planks", "base:stone_pickaxe"].map(|name| registry.id(name))
    };
    let [Some(log), Some(planks), Some(pickaxe)] = ids else {
        gate.check("make: list", Err("the content lacks the oak log, its planks or the stone pickaxe".into()));
        return leave(world, gate);
    };
    let cell_of = |world: &mut World, item: BlockId| {
        let drawn = world.resource::<Drawn>().0.clone();
        world.query::<(Entity, &MakeCell)>().iter(world).find(|(_, cell)| drawn.get(cell.0).is_some_and(|m| m.output == item)).map(|(entity, _)| entity)
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("give oak_log 1");
            net.command("give cobblestone 3");
            let mut ui = world.resource_mut::<UiState>();
            (ui.recipes_open, ui.quests_open, ui.menu_open, ui.character_open) = (false, false, false, false);
            ui.inventory_open = true;
            gate.advance(now);
        }
        // Planks ready, and every ready one before any that is not. The list is drawn from
        // what the server gave, so giving up wants its seconds and the client's both.
        1 => {
            let drawn = world.resource::<Drawn>().0.clone();
            let ready = drawn.iter().find(|m| m.output == planks).is_some_and(|m| m.reach == Reach::Ready);
            let cell = cell_of(world, planks);
            match cell {
                Some(_) if ready && holding(world, "base:oak_log").0 >= 1 => {
                    let first_not = drawn.iter().position(|m| m.reach != Reach::Ready).unwrap_or(drawn.len());
                    let order = drawn[first_not..].iter().all(|m| m.reach != Reach::Ready);
                    gate.check("make: list", if order { Ok(format!("a log in the pack shows planks ready; {first_not} of {} ready, all first", drawn.len())) } else { Err(format!("a ready one comes after one not ready: {:?}", drawn.iter().map(|m| m.reach.rank()).collect::<Vec<_>>())) });
                    gate.count_before = world.resource::<LocalPlayer>().inventory.craft.iter().flatten().filter(|s| s.item == log).map(|s| s.count as u32).sum();
                    gate.advance(now);
                }
                _ if waited > QUEST_WAIT && in_phase > QUEST_WAIT => {
                    let shown: Vec<String> = drawn.iter().take(6).map(|m| format!("{:?} {:?}", m.output, m.reach)).collect();
                    gate.check("make: list", Err(format!("{waited:.1} server s on, {} logs held, planks ready {ready}, a cell for them {}; the list starts {shown:?}", holding(world, "base:oak_log").0, cell.is_some())));
                    leave(world, gate);
                }
                _ => {}
            }
        }
        // Clicked, the planks have the server lay the log in the grid.
        2 => {
            let laid: u32 = world.resource::<LocalPlayer>().inventory.craft.iter().flatten().filter(|s| s.item == log).map(|s| s.count as u32).sum();
            if laid > gate.count_before {
                gate.check("make: lay", Ok(format!("a click on the planks laid {} log in the grid", laid - gate.count_before)));
                gate.advance(now);
            } else if gate.waited_out(QUEST_WAIT) {
                gate.check("make: lay", Err(format!("{waited:.1} server s and {} clicks on the planks on, the grid holds {laid} logs, as before", gate.taps)));
                leave(world, gate);
            } else if let Some(cell) = cell_of(world, planks) {
                gate.click(world, cell, now);
            }
        }
        // Pointed at, the stone pickaxe says what it still wants.
        _ => {
            let Some(cell) = cell_of(world, pickaxe) else {
                if in_phase > 2.0 {
                    gate.check("make: wants", Err(format!("{in_phase:.1} s on, with three cobblestone the stone pickaxe is not on the list")));
                    leave(world, gate);
                }
                return;
            };
            world.resource_mut::<UiClicks>().hover = Some(cell);
            if in_phase < 0.3 {
                return;
            }
            let reach = world.resource::<Drawn>().0.iter().find(|m| m.output == pickaxe).map(|m| m.reach.clone());
            let wanted = reach.map(|reach| crate::hud::makeable::words(world.resource::<Locale>(), &world.resource::<Launch>().content, pickaxe, &reach)).unwrap_or_default();
            let line = world.query_filtered::<&Text, With<crate::hud::makeable::MakeLine>>().iter(world).next().map(|t| t.0.clone()).unwrap_or_default();
            if line != wanted {
                if in_phase > 2.0 {
                    gate.check("make: wants", Err(format!("pointed at, the stone pickaxe's line reads {line:?}, not {wanted:?}")));
                    leave(world, gate);
                }
                return;
            }
            if gate.shot(world, "makeable").is_none() {
                return;
            }
            gate.check("make: wants", Ok(format!("pointed at, the stone pickaxe reads {line:?}")));
            leave(world, gate);
        }
    }
}

/// The way home, played at dusk: from a pad `WAY_OUT` blocks east of home (past the home's
/// box, short of how far it shows by day) the hearth mark, faded in, sits within
/// `WAY_DEGREES` of where home lies with home off to one side of the view; back by the camp
/// it fades out. The camp (bag and torch, two and three blocks east of where the Home step
/// stood) comes down after, and the time of day goes back.
fn way_home(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let home = world.resource::<LocalPlayer>().vitals.home;
    let waited = gate.server_in_phase();
    let leave = |world: &mut World, gate: &mut Gate| {
        let net = world.resource::<Net>();
        // The camp stays home for the nest step, which takes it down.
        if let Some(home) = home {
            let pad = home + IVec3::new(WAY_OUT, -1, 0);
            net.command(&format!("setblock {} {} {} air", pad.x, pad.y, pad.z));
        }
        // The time it was, once it was moved.
        if gate.phase > 0 {
            net.command(&format!("time set {}", gate.way_time));
        }
        gate.next(Step::Nest, now);
    };
    let Some(home) = home else {
        gate.check("way home", Err("no home after the Home step: the server named none".into()));
        return leave(world, gate);
    };
    let mark = world.query::<&crate::hud::home::HomeMark>().iter(world).next().map(|m| (m.shown, m.x, m.fov));
    let shown = world.query_filtered::<&Visibility, With<crate::hud::home::HomeMark>>().iter(world).any(|v| *v != Visibility::Hidden);
    let feet = world.resource::<LocalPlayer>().position;
    let out = home + IVec3::new(WAY_OUT, 0, 0);
    // Back where the Home step stood, two blocks west of the bag.
    let inside = home - IVec3::new(2, 0, 0);
    let at = |cell: IVec3| (feet - cell.as_vec3() - Vec3::new(0.5, 0.0, 0.5)).with_y(0.0).length() < 1.0;
    match gate.phase {
        0 => {
            gate.way_time = world.resource::<crate::render::DayCycle>().ticks.max(0.0) as u64 % voxmar_protocol::TICKS_PER_DAY;
            let net = world.resource::<Net>();
            net.command("time set dusk");
            net.command(&format!("fill {} {} {} {} {} {} air", out.x, out.y, out.z, out.x, out.y + 1, out.z));
            net.command(&format!("setblock {} {} {} stone", out.x, out.y - 1, out.z));
            net.command(&format!("tp {} {} {}", out.x as f32 + 0.5, out.y, out.z as f32 + 0.5));
            gate.advance(now);
        }
        // Out on the pad at dusk, the mark faded in; home put `WAY_ASIDE` degrees to one side
        // of the view, so the mark sits off the middle.
        1 => {
            let dusk = {
                let style = world.resource::<crate::hud::inventory::InterfaceStyle>();
                let tick = world.resource::<crate::render::DayCycle>().ticks.max(0.0) as u64 % voxmar_protocol::TICKS_PER_DAY;
                crate::hud::home::wanted(style, Vec2::new((WAY_OUT as f32) - 0.5, 0.0), tick)
            };
            if at(out) && dusk && mark.is_some_and(|(faded, ..)| faded >= 1.0) {
                let eye = world.resource::<LocalPlayer>().eye();
                let to = home.as_vec3() + Vec3::new(0.5, 0.0, 0.5) - eye;
                let turned = Quat::from_rotation_y(WAY_ASIDE.to_radians()) * to.with_y(0.0);
                aim_at(world, eye + turned);
                gate.advance(now);
            } else if waited > QUEST_WAIT && in_phase > QUEST_WAIT {
                gate.check("way home: dusk", Err(format!("{waited:.1} server s and {in_phase:.1} game s on: on the pad {}, dusk by the client's clock {dusk}, the mark faded to {:?}", at(out), mark.map(|m| m.0))));
                leave(world, gate);
            }
        }
        // Where the mark sits says the bearing it draws; it must be home's, within a few
        // degrees. Given the frames to lay out.
        2 => {
            if in_phase < 0.3 {
                return;
            }
            let player = world.resource::<LocalPlayer>();
            let (from, yaw) = (player.position, player.yaw);
            let truth = crate::hud::home::bearing(from, yaw, home.as_vec3() + Vec3::new(0.5, 0.0, 0.5));
            let drawn = mark.map(|(_, x, fov)| (x * (fov * 0.5).tan()).atan());
            let off = drawn.map(|drawn| (drawn - truth).abs().to_degrees());
            if shown && off.is_some_and(|off| off <= WAY_DEGREES) {
                gate.check("way home: dusk", Ok(format!("at dusk {WAY_OUT} blocks out, the hearth mark showed home {:.1}° from the middle of the view, {:.1}° off its true bearing", truth.to_degrees(), off.unwrap_or_default())));
                let net = world.resource::<Net>();
                net.command(&format!("tp {} {} {}", inside.x as f32 + 0.5, inside.y, inside.z as f32 + 0.5));
                gate.advance(now);
            } else if in_phase > 2.0 {
                gate.check("way home: dusk", Err(format!("the mark (shown {shown}) drew home at {:?}°, its true bearing {:.1}°", drawn.map(f32::to_degrees), truth.to_degrees())));
                leave(world, gate);
            }
        }
        // Back in the camp, it fades out.
        _ => {
            if at(inside) && !shown {
                gate.check("way home: inside", Ok("back inside the home at dusk, the mark faded out".into()));
                leave(world, gate);
            } else if waited > QUEST_WAIT && in_phase > QUEST_WAIT {
                gate.check("way home: inside", Err(format!("{waited:.1} server s and {in_phase:.1} game s on: back in the camp {}, the mark still shown at {:?}", at(inside), mark.map(|m| m.0))));
                leave(world, gate);
            }
        }
    }
}

/// A junglefowl summoned at the Home step's bag, tamed and with an egg due, lays it into a
/// barrel in that home (`Product::nests`), seen in the barrel's open screen (shot
/// `home-nest`), and the chat tells the keeper; then the bag, torch, barrel and hen go.
fn nest(world: &mut World, gate: &mut Gate, now: f32) {
    let Some(bag) = world.resource::<LocalPlayer>().vitals.home else {
        gate.check("home: nest", Err("no home after the Home step to keep a hen at: the server named none".into()));
        gate.next(Step::Door, now);
        return;
    };
    // Where the Home step stood, two blocks west of the bag.
    let stand = bag - IVec3::new(2, 0, 0);
    let (torch, barrel, hen) = (bag + IVec3::X, stand + IVec3::new(0, 0, 2), stand + IVec3::new(0, 0, -3));
    let waited = gate.server_in_phase();
    let leave = |world: &mut World, gate: &mut Gate| {
        world.resource_mut::<UiState>().inventory_open = false;
        let net = world.resource::<Net>();
        net.command("creatures remove junglefowl");
        for at in [bag, torch, barrel] {
            net.command(&format!("setblock {} {} {} air", at.x, at.y, at.z));
        }
        net.command("clear egg");
        gate.next(Step::Door, now);
    };
    let eggs = |world: &World| {
        let egg = world.resource::<ClientWorld>().registry.id("base:egg");
        world.resource::<LocalPlayer>().inventory.store.as_ref().map_or(0, |s| s.slots.iter().flatten().filter(|x| Some(x.item) == egg).map(|x| x.count as u32).sum::<u32>())
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            // Back where the Home step stood, whatever the way home step left.
            net.command(&format!("tp {} {} {}", stand.x as f32 + 0.5, stand.y, stand.z as f32 + 0.5));
            net.command(&format!("setblock {} {} {} barrel", barrel.x, barrel.y, barrel.z));
            net.command(&format!("summon junglefowl 1 {} {} {}", hen.x, hen.y, hen.z));
            net.command("creatures tame junglefowl");
            net.command("creatures hold junglefowl");
            gate.machines_asked = f32::MIN;
            gate.advance(now);
        }
        // The barrel opened, empty, and only then an egg made due, so it is laid with the
        // screen watching.
        1 => {
            if world.resource::<LocalPlayer>().inventory.store.is_some() {
                world.resource::<Net>().command("creatures hasten junglefowl");
                gate.advance(now);
            } else if waited > 10.0 {
                gate.check("home: nest", Err(format!("{waited:.1} server s on, the nest barrel at {barrel} did not open")));
                leave(world, gate);
            } else if gate.server_now - gate.machines_asked >= 1.0 {
                if world.resource::<ClientWorld>().registry.id("base:barrel").is_some_and(|b| world.resource::<ClientWorld>().block(barrel) == Some(b)) {
                    world.resource_mut::<LocalPlayer>().store_asked = true;
                    let actions = world.resource::<crate::Actions>().0.clone();
                    world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:open_store"), voxmar_engine::action::ActionTarget::Block { block: barrel, face: voxmar_engine::coords::Face::PosY });
                }
                gate.machines_asked = gate.server_now;
            }
        }
        _ => {
            let laid = eggs(world);
            let told = world.resource::<crate::hud::console::Console>().since(gate.phase_since).any(|l| l.contains("laid into your home's stores"));
            if laid > 0 && told {
                if gate.shot(world, "home-nest").is_none() {
                    return;
                }
                gate.check("home: nest", Ok(format!("a hen kept at home laid {laid} egg into the barrel there (shot home-nest), and the chat said so")));
                leave(world, gate);
            } else if waited > 15.0 {
                let open = world.resource::<LocalPlayer>().inventory.store.is_some();
                gate.check("home: nest", Err(format!("{waited:.1} server s after the hen's egg was made due: {laid} eggs in the barrel (open {open}), told {told}")));
                leave(world, gate);
            }
        }
    }
}

/// How far east of home the way home step stands, past the home's box and short of where
/// the mark shows by day.
const WAY_OUT: i32 = 20;
/// How far to one side of the view home is put, and how near the mark must draw it.
const WAY_ASIDE: f32 = 20.0;
const WAY_DEGREES: f32 = 3.0;

/// The door step's two cells, as the client has them: (shut, open) when both halves are
/// a door's, shut or open alike.
fn door_cells(world: &World, at: IVec3) -> Option<(bool, bool)> {
    let client = world.resource::<ClientWorld>();
    let [low, high] = [at, at + IVec3::Y].map(|cell| client.block(cell).map(|b| client.registry.def(b)));
    let (low, high) = (low?, high?);
    let door = |tag: &str| low.has_tag(tag) && high.has_tag(tag);
    door("door").then(|| (door("door_shut"), door("door_open")))
}

/// A wooden door set in the ground two blocks before the player, opened and shut by right
/// clicks the server answers (it holds the collision, so the client never guesses); then a
/// hide window in a candle-lit nook, seen glowing at midnight (shot `window-night`) against
/// the same pane with the candle snuffed (`window-night-unlit`).
fn door(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let stand = world.resource::<LocalPlayer>().position.floor().as_ivec3();
    let at = stand + IVec3::new(0, 0, -2);
    // The window's nook, standing on the ground a block beyond the door, and its pane.
    let (nook, pane) = (stand + IVec3::new(0, 1, -4), stand + IVec3::new(0, 1, -3));
    let waited = gate.server_in_phase();
    let leave = |world: &mut World, gate: &mut Gate| {
        let net = world.resource::<Net>();
        net.command(&format!("fill {} {} {} {} {} {} air", at.x, at.y, at.z, at.x, at.y + 1, at.z));
        net.command("clear wooden_door");
        gate.next(Step::Makeable, now);
    };
    let use_door = |world: &mut World| {
        let actions = world.resource::<crate::Actions>().0.clone();
        world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:open"), voxmar_engine::action::ActionTarget::Block { block: at, face: voxmar_engine::coords::Face::PosZ });
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command(&format!("fill {} {} {} {} {} {} air", at.x, at.y, at.z, at.x, at.y + 1, at.z));
            net.command("clear wooden_door");
            net.command("give wooden_door 1");
            gate.advance(now);
        }
        // In hand, set on the ground before the player.
        1 => {
            if select(world, "base:wooden_door") {
                let actions = world.resource::<crate::Actions>().0.clone();
                world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("core:place"), voxmar_engine::action::ActionTarget::Block { block: at - IVec3::Y, face: voxmar_engine::coords::Face::PosY });
                gate.advance(now);
            } else if waited > 10.0 {
                let held = holding(world, "base:wooden_door");
                gate.check("door: placed", Err(format!("no wooden door came to hand {waited:.1} server s after it was given (carried, first slot: {held:?})")));
                leave(world, gate);
            }
        }
        2 => match door_cells(world, at) {
            Some((true, false)) => {
                gate.check("door: placed", Ok("a wooden door went in whole, both halves shut".into()));
                aim_at(world, at.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
                use_door(world);
                gate.advance(now);
            }
            _ if waited > 10.0 => {
                gate.check("door: placed", Err(format!("{waited:.1} server s after placing, the door's cells are {:?}", door_cells(world, at))));
                leave(world, gate);
            }
            _ => {}
        },
        3 => match door_cells(world, at) {
            Some((false, true)) => {
                if gate.shot(world, "door-open").is_none() {
                    return;
                }
                let solid = world.resource::<ClientWorld>().is_solid(at);
                gate.check("door: open", if solid { Err("the open door is still solid on the client".into()) } else { Ok("a right click opened both halves, as the server turned them".into()) });
                use_door(world);
                gate.advance(now);
            }
            _ if waited > 10.0 => {
                gate.check("door: open", Err(format!("{waited:.1} server s after a right click, the door's cells are {:?}", door_cells(world, at))));
                leave(world, gate);
            }
            _ => {}
        },
        4 => match door_cells(world, at) {
            Some((true, false)) => {
                let solid = world.resource::<ClientWorld>().is_solid(at) && world.resource::<ClientWorld>().is_solid(at + IVec3::Y);
                gate.check("door: shut", if solid { Ok("another shut it, solid again".into()) } else { Err("the shut door is not solid on the client".into()) });
                // Then a window: a stone nook lit by a candle, a hide window in its face, at
                // midnight.
                let net = world.resource::<Net>();
                net.command(&format!("fill {} {} {} {} {} {} air", at.x, at.y, at.z, at.x, at.y + 1, at.z));
                net.command("clear wooden_door");
                let (low, high) = (nook - IVec3::ONE, nook + IVec3::ONE);
                net.command(&format!("fill {} {} {} {} {} {} stone", low.x, low.y, low.z, high.x, high.y, high.z));
                net.command(&format!("fill {} {} {} {} {} {} candle", nook.x, nook.y, nook.z, nook.x, nook.y, nook.z));
                net.command(&format!("fill {} {} {} {} {} {} hide_window", pane.x, pane.y, pane.z, pane.x, pane.y, pane.z));
                net.command("time set midnight");
                gate.advance(now);
            }
            _ if waited > 10.0 => {
                gate.check("door: shut", Err(format!("{waited:.1} server s after a second right click, the door's cells are {:?}", door_cells(world, at))));
                leave(world, gate);
            }
            _ => {}
        },
        // The window seen from outside at midnight, lit by the candle: its pane is kept, then
        // the candle is snuffed and the same pane, a moment later under the same sky, is the
        // reference the lit one is judged against.
        5 => {
            aim_at(world, pane.as_vec3() + Vec3::splat(0.5));
            let client = world.resource::<ClientWorld>();
            let window = client.block(pane).map(|b| client.registry.def(b)).filter(|d| d.name == "base:hide_window").map(|d| d.lit_through);
            let lit = client.block(nook).is_some_and(|b| client.registry.def(b).name == "base:candle");
            match window {
                Some(through) if lit && waited > 2.0 => {
                    let Some(shot) = gate.shot(world, "window-night") else { return };
                    if !through {
                        gate.check("window: glows", Err("the hide window is not `lit_through`".into()));
                        return window_done(world, gate, nook, now);
                    }
                    match pane_colour(&shot) {
                        Ok(colour) => {
                            gate.window_lit = Some(colour);
                            world.resource::<Net>().command(&format!("setblock {} {} {} air", nook.x, nook.y, nook.z));
                            gate.advance(now);
                        }
                        Err(why) => {
                            gate.check("window: glows", Err(why));
                            window_done(world, gate, nook, now);
                        }
                    }
                }
                _ if waited > 10.0 => {
                    gate.check("window: glows", Err(format!("{waited:.1} server s after building the nook, its window is {window:?} and its candle is there: {lit}")));
                    window_done(world, gate, nook, now);
                }
                _ => {}
            }
        }
        // Snuffed: once the client has the nook dark and has drawn it for a while, the same
        // pane unlit.
        _ => {
            aim_at(world, pane.as_vec3() + Vec3::splat(0.5));
            let snuffed = world.resource::<ClientWorld>().block(nook).is_some_and(|b| b == BlockId::AIR);
            if snuffed && waited > 2.0 && in_phase > WINDOW_SETTLE {
                let Some(shot) = gate.shot(world, "window-night-unlit") else { return };
                let judged = match (gate.window_lit.take(), pane_colour(&shot)) {
                    (Some(lit), Ok(unlit)) => warm_window(lit, unlit),
                    (None, _) => Err("the lit pane was not kept".into()),
                    (_, Err(why)) => Err(why),
                };
                gate.check("window: glows", judged);
                window_done(world, gate, nook, now);
            } else if waited > 10.0 && in_phase > 10.0 {
                gate.check("window: glows", Err(format!("{waited:.1} server s and {in_phase:.1} game s after the candle was snuffed, the client has it gone: {snuffed}")));
                window_done(world, gate, nook, now);
            }
        }
    }
}

/// The window step's end: the nook taken down, noon again, on to the quests.
fn window_done(world: &mut World, gate: &mut Gate, nook: IVec3, now: f32) {
    let net = world.resource::<Net>();
    let (low, high) = (nook - IVec3::ONE, nook + IVec3::ONE);
    net.command(&format!("fill {} {} {} {} {} {} air", low.x, low.y, low.z, high.x, high.y, high.z));
    net.command("time set noon");
    gate.window_lit = None;
    gate.next(Step::Quests, now);
}

/// The mean colour, of 255, of the middle of a shot aimed at a window's pane.
fn pane_colour(shot: &Image) -> Result<[f64; 3], String> {
    let (w, h, data) = pixels(shot)?;
    let size = (w.min(h) / 16).max(1);
    let (x0, y0) = (w / 2 - size / 2, h / 2 - size / 2);
    let mut sum = [0.0f64; 3];
    for y in y0..y0 + size {
        for x in x0..x0 + size {
            let i = (y * w + x) * 4;
            (0..3).for_each(|c| sum[c] += data[i + c] as f64);
        }
    }
    Ok(sum.map(|v| v / (size * size) as f64))
}

/// Whether a lit room's window glows warm at midnight: what the candle adds to its pane
/// (the pane lit, less the same pane a moment later with the candle snuffed, under the
/// same sky and moon) is redder than it is blue and brightens it well. The night's own
/// light on the pane, whatever the moon, is in both and cancels; the sky (its moon, stars
/// and clouds) is not looked at.
fn warm_window(lit: [f64; 3], unlit: [f64; 3]) -> Result<String, String> {
    let added = [0, 1, 2].map(|c| lit[c] - unlit[c]);
    let brighter = added.iter().sum::<f64>() / 3.0;
    let said = format!("the pane is rgb {:.0} {:.0} {:.0} lit and {:.0} {:.0} {:.0} with the candle snuffed: the candle adds {:.0} {:.0} {:.0}", lit[0], lit[1], lit[2], unlit[0], unlit[1], unlit[2], added[0], added[1], added[2]);
    if added[0] > added[2] + WINDOW_WARMTH && brighter > WINDOW_LIFT {
        Ok(format!("{said}, warm and bright"))
    } else {
        Err(format!("{said}: not a warm, lit window"))
    }
}

/// How much redder than blue the candle's light makes a window's pane, and how much
/// brighter, of 255; and the game seconds the snuffed nook is drawn before its shot.
const WINDOW_WARMTH: f64 = 12.0;
const WINDOW_LIFT: f64 = 15.0;
const WINDOW_SETTLE: f32 = 1.5;

/// How many planks the home barrel holds for the table laid from it.
const LAY_PLANKS: u32 = 4;

/// After the lay: the viewer and inventory shut (the grid goes back to the pack), the
/// barrel gone, and the planks the session carries before it given back.
fn lay_done(world: &mut World, barrel: IVec3) {
    world.resource_mut::<crate::hud::recipes::RecipeViewer>().trail.clear();
    let mut ui = world.resource_mut::<UiState>();
    (ui.recipes_open, ui.inventory_open) = (false, false);
    let net = world.resource::<Net>();
    net.command(&format!("setblock {} {} {} air", barrel.x, barrel.y, barrel.z));
    net.command("clear oak_planks");
    net.command("give oak_planks 64");
}

/// The torch quest the tracker follows, and the quest it sees done.
const TRACKED: &str = "base:torchlight";
const DONE_QUEST: &str = "base:crafting_table";
/// A quest the session finishes before the book opens, for the line out of it.
const DONE_FIRST: &str = "base:twine_and_thong";
/// Longest the quest stages wait for the server, in its seconds.
const QUEST_WAIT: f32 = 10.0;
/// Where the quest book's web is pressed to be dragged, in the window's points, and how
/// far up and to the left it is dragged.
const WEB_GRAB: Vec2 = Vec2::new(900.0, 700.0);
const WEB_DRAG: Vec2 = Vec2::new(160.0, 120.0);

/// Where the quest book's canvas stands in the window, in its pixels.
fn canvas_at(world: &mut World) -> Vec2 {
    world.query_filtered::<&bevy::ui::UiGlobalTransform, With<crate::hud::quests::Canvas>>().iter(world).next().map_or(Vec2::ZERO, |at| at.translation)
}

/// The torch quest's first goal as the server last told it: (have, need).
fn tracked(world: &World) -> (u32, u32) {
    let player = world.resource::<LocalPlayer>();
    player.quests.iter().find(|q| q.name == TRACKED).and_then(|q| q.tasks.first()?.first().copied()).unwrap_or((0, 1))
}

/// What the tracker says of a quest finished and not yet claimed, or finished and paying
/// nothing.
fn done_words(world: &World, quest: &voxmar_engine::quest::QuestDef) -> String {
    world.resource::<Locale>().get(if quest.pays() { "quest.tracker.ready" } else { "quest.tracker.done" }).to_owned()
}

/// The words the tracker shows for the torch quest at `have` torches, or done.
fn tracker_words(world: &World, have: u32) -> String {
    let (locale, launch) = (world.resource::<Locale>(), world.resource::<Launch>());
    let Some(quest) = launch.content.quests.get(TRACKED) else { return String::new() };
    let Some(goal) = quest.tasks.first().and_then(|t| t.first()) else { return String::new() };
    let need = if let voxmar_engine::quest::Goal::Craft(_, need) = goal { *need } else { 1 };
    if have >= need {
        return done_words(world, quest);
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
/// The goals that meet wanderers and the altar, as the quest book words them in the
/// language now on: each names the kind or the prayer by its own words, never a bare key.
fn road_words(world: &World) -> Result<String, String> {
    let (locale, content) = (world.resource::<Locale>(), &world.resource::<Launch>().content);
    let mut lines = Vec::new();
    for name in ["base:farmers_on_the_road", "base:many_travellers", "base:altar_rites"] {
        let quest = content.quests.get(name).ok_or_else(|| format!("no quest {name}"))?;
        for goal in quest.tasks.iter().flatten() {
            let words = crate::hud::quests::goal_words(locale, content, quest, goal, (0, 1));
            let named = match goal {
                voxmar_engine::quest::Goal::Met(kind, _) if kind != "Kinds" && kind != "any" => Some(locale.get(&format!("{}.title", voxmar_engine::lang::def_key("wander", kind))).to_owned()),
                voxmar_engine::quest::Goal::Prayed(prayer, _) => Some(locale.get(&voxmar_engine::lang::def_key("prayer", prayer)).to_owned()),
                _ => None,
            };
            if words.contains("quest.goal") || named.as_ref().is_some_and(|named| !words.contains(named.as_str())) {
                return Err(format!("{name}: {goal:?} reads `{words}`"));
            }
            lines.push(words);
        }
    }
    Ok(lines.join("; "))
}

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
                _ if waited > QUEST_WAIT => quit(world, gate, "quests: twine", format!("{waited:.1} server s on, {wheat} wheat came of 15")),
                _ => {}
            }
        }
        2 => {
            let done = world.resource::<LocalPlayer>().quests.iter().any(|q| q.name == DONE_FIRST && q.done);
            if !done {
                if waited > QUEST_WAIT {
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
            let words = road_words(world);
            gate.check("quests: road words", words);
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
            let words = road_words(world);
            gate.check("quests: road words (ja)", words);
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
                if waited > QUEST_WAIT {
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
                if waited > QUEST_WAIT {
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
            let done = world.resource::<Launch>().content.quests.get(DONE_QUEST).map(|quest| done_words(world, quest)).unwrap_or_default();
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
        12 => {
            // It stays its while, counted from when it was pinned done, then goes, and what
            // it opens is pinned in its place (`follow_ons`), or nothing where the data says.
            let content = world.resource::<Launch>().content.clone();
            let stays = content.quests.tracker().done_stays;
            let since = now - gate.quest_pinned;
            let pins = world.resource::<crate::settings::Settings>().pinned_quests.clone();
            let still = pins.iter().any(|p| p == DONE_QUEST);
            let lines = tracker_lines(world);
            if !still && since < stays - 1.0 {
                return quit(world, gate, "quests: retired", format!("done, it dropped off after only {since:.1} s of {stays} s"));
            }
            let follows = {
                let player = world.resource::<LocalPlayer>();
                let done = |name: &str| player.quests.iter().any(|q| q.name == name && q.done);
                if content.quests.tracker().follow_on { crate::hud::quests::follow_ons(&content, &[], &[DONE_QUEST.to_owned()], &done) } else { Vec::new() }
            };
            // The tracker shows what follows once it is drawn again, or goes.
            let settled = !still && pins == follows && lines.is_some() == !follows.is_empty();
            if !settled && since < stays + 3.0 {
                return;
            }
            gate.check(
                "quests: retired",
                if settled { Ok(format!("done, it stayed and dropped off by {since:.1} s ({stays} s asked), and {follows:?} followed it")) } else { Err(format!("{since:.1} s after it was pinned done, pinned {pins:?}, not {follows:?}; the tracker reads {lines:?}")) },
            );
            world.resource_mut::<crate::settings::Settings>().pinned_quests.clear();
            gate.advance(now);
        }
        // The finished quest pays only when claimed in the book: it opens on the quest with
        // its reward waiting (Twine and Thong, finished at the start), its Claim button is
        // clicked, and the server's word comes back claimed with the experience given.
        13 => {
            let content = world.resource::<Launch>().content.clone();
            let Some(reward) = content.quests.get(DONE_FIRST).and_then(|q| q.rewards.iter().find_map(|r| if let voxmar_engine::quest::Reward::Xp(skill, xp) = r { Some((skill.clone(), *xp)) } else { None })) else {
                return quit(world, gate, "quests: claim", format!("{DONE_FIRST} has no experience to claim"));
            };
            if !crate::hud::quests::claimable(world.resource::<LocalPlayer>(), DONE_FIRST) {
                return quit(world, gate, "quests: claim", format!("{DONE_FIRST} was finished, yet its reward is not waiting to be claimed"));
            }
            let skill = content.skills.id(&reward.0);
            gate.count_before = skill.map_or(0, |skill| world.resource::<LocalPlayer>().skills.xp(skill));
            *world.resource_mut::<QuestView>() = QuestView { picked: Some(DONE_FIRST.into()), ..default() };
            world.resource_mut::<UiState>().quests_open = true;
            gate.advance(now);
        }
        14 => {
            let button = world.query_filtered::<(Entity, &InheritedVisibility), With<crate::hud::quests::ClaimButton>>().iter(world).find(|(_, v)| v.get()).map(|(entity, _)| entity);
            let Some(button) = button else {
                if in_phase > 2.0 {
                    quit(world, gate, "quests: claim", format!("the book open on {DONE_FIRST} shows no Claim button"));
                }
                return;
            };
            // The book as it stands with a reward waiting: the web filling most of the
            // window, the picked quest's page in the sidebar at the right with its Claim
            // button and its rewards.
            if in_phase < 0.5 || gate.shot(world, "quests-claim").is_none() {
                return;
            }
            world.resource_mut::<UiClicks>().next = Some((button, MouseButton::Left));
            gate.advance(now);
        }
        15 => {
            let content = world.resource::<Launch>().content.clone();
            let player = world.resource::<LocalPlayer>();
            let claimed = player.quests.iter().any(|q| q.name == DONE_FIRST && q.done && q.claimed);
            if !claimed {
                if waited > QUEST_WAIT {
                    let state = player.quests.iter().find(|q| q.name == DONE_FIRST).cloned();
                    quit(world, gate, "quests: claim", format!("Claim clicked, {waited:.1} server s on the quest stands {state:?}"));
                }
                return;
            }
            let Some((skill, xp)) = content.quests.get(DONE_FIRST).and_then(|q| q.rewards.iter().find_map(|r| if let voxmar_engine::quest::Reward::Xp(skill, xp) = r { content.skills.id(skill).map(|id| (id, *xp)) } else { None })) else { return };
            let gained = player.skills.xp(skill).saturating_sub(gate.count_before);
            // The experience comes in the same message as the state, or just after it.
            if gained < (xp * 10.0) as u32 && waited <= QUEST_WAIT {
                return;
            }
            gate.check("quests: claim", if gained >= (xp * 10.0) as u32 { Ok(format!("Claim paid {DONE_FIRST}: the quest reads claimed and {} xp came", gained as f32 / 10.0)) } else { Err(format!("claimed, yet only {} of {xp} xp came", gained as f32 / 10.0)) });
            gate.advance(now);
        }
        // The web dragged as a canvas: pressed on its empty space, moved up and to the
        // left, it follows the pointer within its edges; every line drawn stays inside the
        // well, slanted ones cut at its edge.
        16 => {
            let Some(web) = world.query_filtered::<Entity, With<crate::hud::quests::Web>>().iter(world).next() else {
                return quit(world, gate, "quests: drag", "the quest book has no web".into());
            };
            gate.web_scroll = world.get::<ScrollPosition>(web).map_or(Vec2::ZERO, |s| s.0);
            gate.web_canvas = canvas_at(world);
            world.write_message(CursorMoved { window: Entity::PLACEHOLDER, position: WEB_GRAB, delta: None });
            world.resource_mut::<UiClicks>().hover = Some(web);
            mouse(world, MouseButton::Left, true);
            gate.advance(now);
        }
        17 => {
            world.write_message(CursorMoved { window: Entity::PLACEHOLDER, position: WEB_GRAB - WEB_DRAG, delta: Some(-WEB_DRAG) });
            gate.advance(now);
        }
        18 => {
            if in_phase < 0.3 {
                return;
            }
            mouse(world, MouseButton::Left, false);
            let Some((scale, max, (x, y, w, h))) = world
                .query_filtered::<(&ComputedNode, &bevy::ui::UiGlobalTransform), With<crate::hud::quests::Web>>()
                .iter(world)
                .next()
                .map(|(computed, at)| (computed.inverse_scale_factor(), (computed.content_size() - computed.size()).max(Vec2::ZERO) * computed.inverse_scale_factor(), (at.translation.x, at.translation.y, computed.size().x, computed.size().y)))
            else {
                return quit(world, gate, "quests: drag", "the quest book has no web".into());
            };
            // The canvas as drawn moves with the pointer, pixel for pixel, as far as its edges
            // let it: the pointer's points are `UiScale` interface pixels.
            let travel = WEB_DRAG / world.resource::<UiScale>().0;
            let wanted = ((gate.web_scroll + travel).clamp(Vec2::ZERO, max) - gate.web_scroll.clamp(Vec2::ZERO, max)) / scale;
            let moved = gate.web_canvas - canvas_at(world);
            gate.check(
                "quests: drag",
                if moved.distance(wanted) < 2.0 { Ok(format!("dragged {WEB_DRAG} points on the empty web, its canvas moved {moved} pixels ({wanted} within its edges)")) } else { Err(format!("dragged {WEB_DRAG} points on the empty web, its canvas moved {moved} pixels, not {wanted}")) },
            );
            // Each line's drawn corners against the well, in the window's pixels.
            let well = Rect::from_center_size(Vec2::new(x, y), Vec2::new(w, h)).inflate(1.0);
            let mut lines = world.query_filtered::<(&Node, &ComputedNode, &bevy::ui::UiGlobalTransform), With<crate::hud::quests::WebLine>>();
            let (mut shown, mut out) = (0, Vec::new());
            for (node, computed, at) in lines.iter(world) {
                if node.display == Display::None {
                    continue;
                }
                shown += 1;
                let half = computed.size() / 2.0;
                let corners = [Vec2::new(-half.x, -half.y), Vec2::new(half.x, -half.y), half, Vec2::new(-half.x, half.y)].map(|c| at.transform_point2(c));
                if let Some(c) = corners.iter().find(|c| !well.contains(**c)) {
                    out.push(format!("{c} outside {well:?}"));
                }
            }
            let all = lines.iter(world).count();
            gate.check("quests: lines inside", if out.is_empty() && shown > 0 { Ok(format!("{shown} of {all} lines drawn, every corner inside the well")) } else { Err(format!("{shown} of {all} lines drawn; {} poke out: {:?}", out.len(), out.iter().take(3).collect::<Vec<_>>())) });
            gate.advance(now);
        }
        19 => {
            if gate.shot(world, "quests-dragged").is_some() {
                gate.advance(now);
            }
        }
        // A path's lane header pressed, the lowest one that would move the canvas, brings
        // the canvas to that path's next quest and picks it. Only the client moves, so the
        // wait is the game's seconds, not the server's.
        20 => {
            let scroll = world.query_filtered::<&ScrollPosition, With<crate::hud::quests::Web>>().iter(world).next().map_or(Vec2::ZERO, |s| s.0);
            let heads: Vec<(Entity, Option<String>, Vec2)> = world.query::<(Entity, &crate::hud::quests::LaneHead)>().iter(world).map(|(e, h)| (e, h.head.clone(), h.at)).collect();
            // A web that fits its well never moves, so there any header whose head is not
            // the quest picked will do: it must still pick its head, inside the well.
            let picked = world.resource::<crate::hud::quests::QuestView>().picked.clone();
            let moving = heads.iter().rev().find(|(_, head, at)| head.is_some() && at.distance(scroll) > 1.0);
            let Some((header, head, at)) = moving.or_else(|| heads.iter().rev().find(|(_, head, _)| head.is_some() && *head != picked)).cloned() else {
                return quit(world, gate, "quests: path", format!("{} lane headers, none with a quest away from the canvas at {scroll} or other than {picked:?}", heads.len()));
            };
            gate.web_head = head;
            gate.web_scroll = at;
            world.resource_mut::<UiClicks>().next = Some((header, MouseButton::Left));
            gate.advance(now);
        }
        21 => {
            let picked = world.resource::<crate::hud::quests::QuestView>().picked.clone();
            let scroll = world.query_filtered::<&ScrollPosition, With<crate::hud::quests::Web>>().iter(world).next().map_or(Vec2::ZERO, |s| s.0);
            let (well, max) = world
                .query_filtered::<(&ComputedNode, &bevy::ui::UiGlobalTransform), With<crate::hud::quests::Web>>()
                .iter(world)
                .next()
                .map_or((None, Vec2::ZERO), |(c, at)| (Some(Rect::from_center_size(at.translation, c.size())), (c.content_size() - c.size()).max(Vec2::ZERO) * c.inverse_scale_factor()));
            // Where the header asked for, as far as the canvas moves.
            let wanted = gate.web_scroll.clamp(Vec2::ZERO, max);
            let node = world.query::<(&crate::hud::quests::QuestNode, &bevy::ui::UiGlobalTransform)>().iter(world).find(|(n, _)| Some(&n.0) == gate.web_head.as_ref()).map(|(_, at)| at.translation);
            let shown = matches!((well, node), (Some(well), Some(node)) if well.contains(node));
            if picked == gate.web_head && scroll.distance(wanted) < 1.0 && shown {
                if gate.shot(world, "quests-path").is_none() {
                    return;
                }
                gate.check("quests: path", Ok(format!("a lane header brought the canvas to {scroll} and picked {picked:?}, inside the well")));
            } else if in_phase < 2.0 {
                return;
            } else {
                gate.check("quests: path", Err(format!("2 s after a lane header was pressed for {:?} (canvas wanted at {wanted}): picked {picked:?}, canvas at {scroll}, the quest at {node:?} in the well {well:?}", gate.web_head)));
            }
            world.resource_mut::<UiState>().quests_open = false;
            gate.next(Step::Catalogue, now);
        }
        _ => gate.next(Step::Catalogue, now),
    }
}

/// How many of `item` the player has, held or in the inventory.
fn count_of(world: &World, item: BlockId) -> u32 {
    let inventory = &world.resource::<LocalPlayer>().inventory;
    inventory.slots.iter().chain(std::iter::once(&inventory.cursor)).flatten().filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

fn catalogue(world: &mut World, gate: &mut Gate, now: f32, _in_phase: f32) {
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
            if !world.resource::<LocalPlayer>().allow_flight && gate.server_in_phase() < 3.0 {
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
            if gained == 0 && gate.server_in_phase() < 3.0 {
                return;
            }
            gate.check("catalogue: creative", if gained > 0 { Ok(format!("Shift-click gave {gained} oak logs")) } else { Err("a Shift-click gave nothing in creative".into()) });
            world.resource::<Net>().command("gamemode survival");
            gate.advance(now);
        }
        3 => {
            if world.resource::<LocalPlayer>().allow_flight && gate.server_in_phase() < 3.0 {
                return;
            }
            gate.counted = count_of(world, log);
            crate::hud::creative::take(world.resource::<Net>(), log, false, true);
            gate.advance(now);
        }
        _ => {
            // Long enough for a stack to arrive, were it given: by the server's clock, or a
            // server running behind would give it after the check.
            if gate.server_in_phase() < 1.0 {
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
    /// The prayer book over the character screen, and the strip over the hotbar naming as
    /// many prayers as can be on together.
    Prayers,
    /// The character screen with one skill's guide open.
    Guide(usize),
    /// The journal on one page.
    Journal(usize),
    /// The quest book on its first chapter, a quest still waiting picked.
    Quests,
    /// First Night's full cooking instructions, in both languages.
    FirstNight,
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
const FIT_LOOKUPS: [(&str, bool); 4] = [("base:campfire", false), ("base:oak_log", true), ("base:wool", false), ("base:cooked_small_game", false)];

/// The languages the text-fit tour checks, and how long it leaves each view to lay out.
const FIT_LANGUAGES: [&str; 2] = ["en", "ja"];
const FIT_SETTLE: f32 = 0.4;
/// Frames a view's layout must hold still, once it changed for the view, to be read before
/// `FIT_SETTLE`, and the least it is left: a view laid out sooner is read sooner.
const FIT_STILL: u32 = 4;
const FIT_SETTLE_LEAST: f32 = 0.1;

/// Where every node of the interface is and how big, and every text's lines: what laying
/// out a view changes.
fn layout_print(world: &mut World) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    let mut nodes = world.query::<(&ComputedNode, &bevy::ui::UiGlobalTransform, Option<&bevy::text::TextLayoutInfo>)>();
    for (node, at, text) in nodes.iter(world) {
        let size = node.size();
        let at = at.translation;
        [size.x, size.y, at.x, at.y].map(f32::to_bits).hash(&mut hasher);
        if let Some(text) = text {
            [text.size.x, text.size.y].map(f32::to_bits).hash(&mut hasher);
            text.glyphs.len().hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Opens every view in turn, in each language, and lists the texts that do not fit (see
/// `ui::fit`): one check for the lot, naming each.
fn fit(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let skills = world.resource::<Launch>().content.skills.len();
    let views: Vec<View> = if gate.step == Step::FitJournal {
        (0..voxmar_engine::journal::Page::ALL.len()).map(View::Journal).collect()
    } else if gate.step == Step::FitMenus {
        [View::Title, View::Worlds, View::Create, View::Join].into_iter().chain((0..crate::settings::TABS).map(View::Settings)).collect()
    } else {
        [View::Hud, View::Debug, View::Dead, View::Console, View::Inventory, View::Recipes(0), View::Recipes(1), View::Recipes(2), View::Recipes(3), View::Character, View::Prayers]
            .into_iter()
            .chain((0..skills).map(View::Guide))
            .chain([View::Quests, View::FirstNight])
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
    // Read once the view is laid out: its layout changed and then held still a few frames,
    // or `FIT_SETTLE` passed (a view that lays out like the one before it).
    let print = layout_print(world);
    let shown = (gate.step, gate.phase);
    let (on, last, changed, still) = &mut gate.fit_layout;
    if *on != shown {
        (*on, *last, *changed, *still) = (shown, print, false, 0);
    } else if print == *last {
        *still += 1;
    } else {
        (*last, *changed, *still) = (print, true, 0);
    }
    let laid_out = *changed && *still >= FIT_STILL && in_phase >= FIT_SETTLE_LEAST;
    if in_phase < FIT_SETTLE && !laid_out {
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

/// The player's own prayers while [`fit_prayers`] shows others, and those it shows.
#[derive(Resource)]
struct KeptPrayers(crate::fight::Prayers, crate::fight::Prayers);

/// With `full`, the strip over the hotbar shows as many prayers as can be on together (each
/// turned on in the book's order, as the server turns them on) and its meter half full;
/// without, the player's own come back, unless the server has told new ones since.
fn fit_prayers(world: &mut World, full: bool) {
    if !full {
        if let Some(KeptPrayers(kept, shown)) = world.remove_resource::<KeptPrayers>() {
            let mut fighting = world.resource_mut::<crate::fight::Fighting>();
            if fighting.prayers == shown {
                fighting.prayers = kept;
            }
        }
        return;
    }
    if world.contains_resource::<KeptPrayers>() {
        return;
    }
    let book = &world.resource::<Launch>().content.prayers;
    let on = book.all().fold(Vec::new(), |on, (id, _)| book.turn_on(&on, id));
    let kept = world.resource::<crate::fight::Fighting>().prayers.clone();
    let most = kept.most.max(10);
    let shown = crate::fight::Prayers { points: most / 2, most, on };
    world.resource_mut::<crate::fight::Fighting>().prayers = shown.clone();
    world.insert_resource(KeptPrayers(kept, shown));
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
        Some(View::Character | View::Prayers | View::Guide(_)) => ui.character_open = true,
        Some(View::Menu(_)) => ui.menu_open = true,
        Some(View::Journal(_)) => ui.journal_open = true,
        Some(View::Quests | View::FirstNight) => ui.quests_open = true,
        Some(View::Console) => ui.console_open = true,
        Some(View::Recipes(_)) => (ui.inventory_open, ui.recipes_open) = (true, true),
        _ => {}
    }
    if !matches!(view, Some(View::Recipes(_))) {
        ui.recipes_open = false;
    }
    let book = matches!(view, Some(View::Prayers));
    if world.resource::<crate::hud::prayers::PrayerBook>().0 != book {
        world.resource_mut::<crate::hud::prayers::PrayerBook>().0 = book;
    }
    fit_prayers(world, book);
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
        *world.resource_mut::<QuestView>() = QuestView { chapter: 0, picked, ..default() };
    }
    if let Some(View::FirstNight) = view {
        *world.resource_mut::<crate::hud::quests::QuestView>() = crate::hud::quests::QuestView { chapter: 0, picked: Some("base:first_night".into()), ..default() };
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
            toasts.level_up(skill, 90);
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

/// Returns each view test to the same open ground, including after a failed swim.
/// Wait for a new server teleport and grounded client pose before asking for a shot;
/// looking from inside the pool's walls cannot judge weather or the morning sky.
///
/// A player found off the spot once the teleport is in, on the ground and out of water, was
/// moved by something on it (a creature on the spot puts the player out of its body,
/// `player.rs`): the wild creatures around are sent away and the player back, until they
/// stay put. Twice under load the player stood 0.16 and 0.97 blocks off in +x at the start
/// of Depths, and the pose never settled.
fn open_scene_pose(world: &mut World, gate: &mut Gate) -> bool {
    let position = gate.feet.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
    let send = |world: &mut World, gate: &mut Gate, disperse: bool| {
        gate.scene_teleport = world.resource::<LocalPlayer>().teleports;
        gate.scene_sent = gate.server_now;
        let net = world.resource::<Net>();
        if disperse {
            net.command("creatures disperse");
        }
        net.command(&format!("tp {} {} {}", position.x, position.y, position.z));
    };
    if gate.settled == 0 {
        key(world, KeyCode::KeyW, false);
        key(world, KeyCode::Space, false);
        world.resource_mut::<LocalPlayer>().flying = false;
        send(world, gate, false);
        gate.settled = 1;
        return false;
    }
    let player = world.resource::<LocalPlayer>();
    let arrived = player.teleports != gate.scene_teleport && player.on_ground && !player.in_water;
    let off = player.position.distance(position) >= 0.05;
    if arrived && off && gate.server_now - gate.scene_sent > SCENE_RESEND {
        info!("gate: pose: {}; sending the wild creatures away and the player back", unsettled(world, gate));
        send(world, gate, true);
        gate.settled = 1;
        return false;
    }
    let ready = arrived && !off;
    gate.settled = if ready { gate.settled + 1 } else { 1 };
    ready && gate.settled >= 3
}

/// Server seconds `open_scene_pose` lets a player who is off the spot after its teleport
/// stand before it sends them back.
const SCENE_RESEND: f32 = 1.0;

/// Why `open_scene_pose` has not settled, for its checks' failures.
fn unsettled(world: &World, gate: &Gate) -> String {
    let player = world.resource::<LocalPlayer>();
    let position = gate.feet.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
    let launch = world.resource::<Launch>();
    let near: Vec<String> = world
        .resource::<crate::creatures::Creatures>()
        .views()
        .filter(|v| v.position.distance(position) < 4.0)
        .map(|v| format!("{} at {:.2}", launch.content.creatures.get(v.species).def.name, v.position))
        .collect();
    format!(
        "the open test-ground teleport did not settle: teleported {}, on the ground {}, in water {}, {:.2} from the spot at {:.2}; creatures within 4: {near:?}",
        player.teleports != gate.scene_teleport,
        player.on_ground,
        player.in_water,
        player.position.distance(position),
        player.position
    )
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
            if !open_scene_pose(world, gate) {
                if gate.server_in_phase() > 15.0 {
                    gate.check("weather: pose", Err(unsettled(world, gate)));
                    gate.next(Step::Depths, now);
                }
                return;
            }
            look(world, 0.0, 5.0);
            // Not with the music step's now-playing line still fading (`music_fades`).
            if gate.music_fade.is_some() {
                return;
            }
            if gate.shot(world, "clear").is_some() {
                world.resource::<Net>().command(&format!("weather {} 120", kinds[0].0));
                gate.advance(now);
            }
        }
        // Waiting for each kind to come, then a shot of it.
        phase @ (1 | 2) => {
            let (kind, name, arrived) = kinds[phase as usize - 1];
            if !arrived(&shown) {
                if gate.server_in_phase() > 12.0 {
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
            match kinds.get(phase as usize) {
                Some((kind, ..)) => {
                    world.resource::<Net>().command(&format!("weather {kind} 120"));
                    gate.advance(now);
                }
                // The depths step brings its fog straight over the snow and clears both at
                // its end: one clearing waited for, not two back to back.
                None => {
                    gate.weather_clear_owed = true;
                    gate.next(Step::Depths, now);
                }
            }
        }
        3 if sky_clear(&shown) => {
            eased(gate);
            gate.next(Step::Depths, now);
        }
        3 if gate.server_in_phase() > 12.0 => {
            gate.check("weather: clear", Err(format!("the sky did not clear: {}", shown.summary())));
            eased(gate);
            gate.next(Step::Depths, now);
        }
        _ => {}
    }
}

/// Dug down, in a cave or in a room, mist is gone; cracks are lit as the face under them is.
///
/// A fog comes over the open test ground, and the renderer's air has ground fog and the
/// weather's fog in it. The player then goes down a shaft eleven deep, open to the sky (the
/// sky light there is full all the way down, so only looking for the sky tells it from open
/// land), and the air has none of either. Then in a sealed room deep in stone, with no light
/// at all, a wall is mined just until its cracks show: they are lit by nothing and catch no
/// light (their light edges are gone); a glowstone set beside the wall and the same block
/// mined again, they are lit by it and glint. Each stint lets go of the button as soon as
/// the cracks show, and after [`CRACK_HOLD`] seconds without them: a button held on a block
/// that never cracks floods the lagging server with proposals. Both shots are kept for a look.
fn depths(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::render::atmosphere::Atmosphere;
    use crate::render::cracks::Crack;
    // The rig stands apart from everything else: stone nineteen blocks across, thirteen
    // wide and seventeen deep, level with the test ground, with a shaft down its middle and
    // a sealed room below whose walls are four blocks thick or more, so no sky light reaches
    // in beside it and mining one opens nothing.
    let middle = gate.feet + IVec3::new(-28, 0, 0);
    let top = gate.feet.y - 1;
    let shaft_foot = IVec3::new(middle.x, top - 11, middle.z);
    let room_floor = IVec3::new(middle.x - 3, top - 15, middle.z);
    let wall = IVec3::new(middle.x - 5, top - 14, middle.z);
    // The air in front of the face being mined, and the glowstone's place beside the wall.
    let face = wall + IVec3::X;
    let glow = IVec3::new(middle.x - 5, top - 13, middle.z);
    let shown = world.resource::<crate::weather::ClientWeather>().current.clone();
    let air = *world.resource::<Atmosphere>();
    let mist = |air: &Atmosphere| format!("ground fog {:.3}, the weather's fog {:.5}", air.mist.x, air.fog.w);
    let stand = |world: &World, at: IVec3| {
        let player = world.resource::<LocalPlayer>();
        player.on_ground && player.position.distance(at.as_vec3() + Vec3::new(0.5, 0.0, 0.5)) < 0.3
    };
    // The light on the face being mined as the cracks take it, and how much of their
    // light edges show in it.
    let lit = |world: &World| {
        let crack = world.resource::<Crack>();
        let scene = world.resource::<crate::render::SceneLight>().at(crack.light[0], crack.light[1]);
        let pack = world.resource::<crate::render::pack::ActivePack>();
        (crack.light, pack.crack_highlight(scene.red * 0.2126 + scene.green * 0.7152 + scene.blue * 0.0722))
    };
    let face_light = |world: &World| crate::breaking::light_at(world.resource::<ClientWorld>(), face);
    let bail = |world: &mut World, gate: &mut Gate, name: &str, why: String| {
        gate.check(name, Err(why));
        mouse(world, MouseButton::Left, false);
        world.resource::<Net>().command("weather clear 600");
        // Back up to the test ground, and the fog lifting.
        gate.phase = 9;
        gate.phase_since = now;
        gate.phase_server = gate.server_now;
        gate.settled = 0;
    };
    match gate.phase {
        // Build the rig and bring a fog over the test ground.
        0 => {
            let mut ui = world.resource_mut::<UiState>();
            (ui.menu_open, ui.inventory_open, ui.character_open, ui.console_open) = (false, false, false, false);
            if !open_scene_pose(world, gate) {
                if gate.server_in_phase() > 15.0 {
                    gate.check("depths: pose", Err(unsettled(world, gate)));
                    clear_owed(world, gate);
                    gate.next(Step::Sky, now);
                }
                return;
            }
            look(world, 0.0, 5.0);
            let net = world.resource::<Net>();
            let (x, z) = (middle.x, middle.z);
            for line in [
                format!("fill {} {} {} {} {} {} stone", x - 9, top - 16, z - 6, x + 9, top, z + 6),
                format!("fill {x} {} {z} {x} {top} {z} air", top - 11),
                format!("fill {} {} {} {} {} {} air", x - 4, top - 15, z - 1, x + 4, top - 13, z + 1),
                "time set noon".to_owned(),
                "weather fog 300".into(),
            ] {
                net.command(&line);
            }
            gate.advance(now);
        }
        // The fog is over the test ground and the rig is there: the air has mist in it.
        1 => {
            let built = world.resource::<ClientWorld>().block(shaft_foot).is_some_and(|b| b.is_air())
                && world.resource::<ClientWorld>().block(shaft_foot - IVec3::Y).is_some_and(|b| !b.is_air())
                && world.resource::<ClientWorld>().block(wall).is_some_and(|b| !b.is_air())
                && world.resource::<ClientWorld>().block(room_floor - IVec3::Y).is_some_and(|b| !b.is_air());
            let foggy = shown.ground_fog > 0.3 || shown.fog > 0.3;
            if !(built && foggy) {
                if gate.server_in_phase() > 20.0 {
                    bail(world, gate, "depths: fog", format!("it did not come, or the rig did not build: {} (built {built})", shown.summary()));
                }
                return;
            }
            // Let the air settle on it, then the shot.
            if in_phase < 1.0 {
                return;
            }
            let Some(_) = gate.shot(world, "mist-open") else { return };
            let result = if air.mist.x > 0.0 || air.fog.w > 0.0 {
                Ok(mist(&air))
            } else {
                Err(format!("in the open under {} the air has {}", shown.summary(), mist(&air)))
            };
            gate.check("depths: mist in the open", result);
            world.resource::<Net>().command(&format!("tp {} {} {}", shaft_foot.x as f32 + 0.5, shaft_foot.y, shaft_foot.z as f32 + 0.5));
            gate.advance(now);
        }
        // At the foot of the shaft: its light is full, and there is no mist.
        2 => {
            if !stand(world, shaft_foot) {
                if gate.server_in_phase() > 15.0 {
                    bail(world, gate, "depths: shaft", "the player never stood at the foot of the shaft".into());
                }
                return;
            }
            look(world, 0.0, 80.0);
            let gone = air.mist.x == 0.0 && air.fog.w == 0.0;
            if !gone {
                // The eye eases onto the new view over a second or so.
                if in_phase > 8.0 {
                    let open = world.resource::<crate::render::exposure::SkyOpen>().shown;
                    bail(world, gate, "depths: no mist in a shaft", format!("eleven deep and open to the sky, the air has {} ({open:.2} of the sky in view)", mist(&air)));
                }
                return;
            }
            let Some(_) = gate.shot(world, "mist-shaft") else { return };
            let open = world.resource::<crate::render::exposure::SkyOpen>().shown;
            gate.check("depths: no mist in a shaft", Ok(format!("{} ({open:.2} of the sky in view)", mist(&air))));
            let room = format!("tp {} {} {}", room_floor.x as f32 + 0.5, room_floor.y, room_floor.z as f32 + 0.5);
            world.resource::<Net>().command(&room);
            // The fog has been seen in the open and not in the shaft: it lifts while the
            // cracks are tried in the sealed room, where none reaches.
            world.resource::<Net>().command("weather clear 600");
            gate.advance(now);
        }
        // In the sealed room, before the wall is touched: the face is dark.
        3 => {
            let dark = face_light(world) == [0, 0];
            if !(stand(world, room_floor) && dark) {
                if gate.server_in_phase() > 15.0 {
                    let [sky, block] = face_light(world);
                    bail(world, gate, "depths: room", format!("the player never stood in the sealed room, or its wall's face was lit (sky {sky}, block {block})"));
                }
                return;
            }
            aim_at(world, wall.as_vec3() + 0.5);
            mouse(world, MouseButton::Left, true);
            gate.advance(now);
        }
        // Mining the wall in the dark: the cracks show, lit by nothing. The shot is of this
        // frame, so the button is let go at once.
        4 => {
            aim_at(world, wall.as_vec3() + 0.5);
            if world.resource::<Crack>().at.is_none() {
                if in_phase > CRACK_HOLD {
                    bail(world, gate, "depths: crack in the dark", format!("mining the wall showed no crack in {CRACK_HOLD} s"));
                } else {
                    mouse(world, MouseButton::Left, true);
                }
                return;
            }
            let (light, highlight) = lit(world);
            gate.shot(world, "crack-dark");
            mouse(world, MouseButton::Left, false);
            let result = if highlight == 0.0 && light[1] == 0 {
                Ok(format!("sky light {}, block light {}: the cracks darken and catch no light", light[0], light[1]))
            } else {
                Err(format!("in a sealed room with no light the cracks are lit: sky {}, block {}, light edges {highlight:.2}", light[0], light[1]))
            };
            gate.check("depths: crack in the dark", result);
            // The glowstone, and the wall as it was in case the stint broke it.
            let net = world.resource::<Net>();
            net.command(&format!("setblock {} {} {} stone", wall.x, wall.y, wall.z));
            net.command(&format!("setblock {} {} {} glowstone", glow.x, glow.y, glow.z));
            gate.advance(now);
        }
        // The glowstone beside the wall lights its face; the shot of the dark cracks is in.
        5 => {
            let glowstone = world.resource::<ClientWorld>().registry.id("base:glowstone");
            let ready = glowstone.is_some()
                && world.resource::<ClientWorld>().block(glow) == glowstone
                && world.resource::<ClientWorld>().block(wall).is_some_and(|b| !b.is_air())
                && world.resource::<Crack>().at.is_none()
                && face_light(world)[1] >= 10;
            if !(ready && gate.shot(world, "crack-dark").is_some()) {
                if gate.server_in_phase() > 15.0 {
                    let [sky, block] = face_light(world);
                    bail(world, gate, "depths: crack by a glowstone", format!("the glowstone never lit the wall's face (sky {sky}, block {block})"));
                }
                return;
            }
            aim_at(world, wall.as_vec3() + 0.5);
            mouse(world, MouseButton::Left, true);
            gate.advance(now);
        }
        // The same block mined again: the cracks are lit by the glowstone.
        6 => {
            aim_at(world, wall.as_vec3() + 0.5);
            if world.resource::<Crack>().at.is_none() {
                if in_phase > CRACK_HOLD {
                    bail(world, gate, "depths: crack by a glowstone", format!("mining the wall showed no crack in {CRACK_HOLD} s"));
                } else {
                    mouse(world, MouseButton::Left, true);
                }
                return;
            }
            let (light, highlight) = lit(world);
            gate.shot(world, "crack-lit");
            mouse(world, MouseButton::Left, false);
            let result = if highlight >= 0.99 && light[1] >= 10 {
                Ok(format!("sky light {}, block light {}: the cracks catch the light (edges {highlight:.2})", light[0], light[1]))
            } else {
                Err(format!("a glowstone beside the wall does not light the cracks: sky {}, block {}, light edges {highlight:.2}", light[0], light[1]))
            };
            gate.check("depths: crack by a glowstone", result);
            gate.advance(now);
        }
        // The shot of the lit cracks is in.
        7 => {
            if gate.shot(world, "crack-lit").is_some() || gate.server_in_phase() > 15.0 {
                gate.advance(now);
            }
        }
        // Back to the test ground.
        8 | 9 => {
            if !open_scene_pose(world, gate) {
                if gate.server_in_phase() > 15.0 {
                    gate.check("depths: back", Err(unsettled(world, gate)));
                    clear_owed(world, gate);
                    gate.next(Step::Sky, now);
                }
                return;
            }
            gate.phase = 9;
            gate.advance(now);
        }
        // The fog lifts before the sky is judged, and the weather step's snow has gone with it.
        _ => {
            let lifted = shown.fog < 0.02 && shown.ground_fog < 0.02;
            let cleared = !gate.weather_clear_owed || sky_clear(&shown);
            if lifted && cleared {
                if gate.weather_clear_owed {
                    gate.weather_clear_owed = false;
                    eased(gate);
                }
                gate.next(Step::Sky, now);
            } else if gate.server_in_phase() > 20.0 {
                if !lifted {
                    gate.check("depths: clear", Err(format!("the fog did not lift: {}", shown.summary())));
                }
                if !cleared {
                    gate.check("weather: clear", Err(format!("the sky did not clear: {}", shown.summary())));
                }
                if std::mem::take(&mut gate.weather_clear_owed) {
                    eased(gate);
                }
                gate.next(Step::Sky, now);
            }
        }
    }
}

/// Whether the sky has cleared: no rain or snow to speak of, and little cloud.
fn sky_clear(shown: &voxmar_engine::weather::Weather) -> bool {
    shown.precipitation < 0.05 && shown.cover < 0.2
}

/// A depths step ended before its last phase clears the sky for the weather step: cleared
/// now, its clearing unjudged (a failure, as the step's own is).
fn clear_owed(world: &mut World, gate: &mut Gate) {
    if std::mem::take(&mut gate.weather_clear_owed) {
        world.resource::<Net>().command("weather clear 600");
        gate.check("weather: clear", Err("the depths step ended before the sky could clear after the snow".into()));
        eased(gate);
    }
}

/// Seconds the depths step holds the button for a crack to show before it lets go.
const CRACK_HOLD: f32 = 3.0;

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
            if !(grown && settled(world, trunk) && in_phase > SETTLE) && gate.server_in_step() < STEP_LIMIT - 1.0 {
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
            gate.next(Step::Shapes, now);
        }
        _ => {}
    }
}

/// Blocks drawn as their shapes, set in a row in the air just in front of the player, clear
/// of whatever the earlier steps built and seen against the sky: they must reach the
/// client and change the view.
fn shapes(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    // The last four are decor (P58a): furniture and cloth in a colour baked into their
    // textures, in a facing, and a piece of crockery.
    let blocks = [
        "oak_slab", "oak_stairs", "oak_stairs_top_east", "oak_slab_top", "barrel", "crate", "sack", "oak_table", "birch_chair_east", "indigo_banner", "ivory_jug",
    ];
    if gate.phase == 0 {
        gate.shapes_row = world.resource::<LocalPlayer>().position.floor().as_ivec3() + IVec3::new(-3, 3, 4);
    }
    let row = gate.shapes_row;
    let cells: Vec<IVec3> = (0..blocks.len() as i32).map(|i| row + IVec3::X * i).collect();
    match gate.phase {
        0 => {
            aim(world, row + IVec3::X * (blocks.len() as i32 / 2));
            gate.advance(now);
        }
        // The view turned to the row first, so only the blocks change it.
        1 if in_phase > 0.5 => {
            aim(world, row + IVec3::X * (blocks.len() as i32 / 2));
            if gate.shot(world, "shapes-before").is_some() {
                for (cell, block) in cells.iter().zip(blocks) {
                    world.resource::<Net>().command(&format!("setblock {} {} {} {block}", cell.x, cell.y, cell.z));
                }
                gate.advance(now);
            }
        }
        2 => {
            let client = world.resource::<ClientWorld>();
            let arrived = cells.iter().zip(blocks).all(|(&cell, name)| {
                let wanted = client.registry.id(&format!("base:{name}"));
                wanted.is_some() && client.block(cell) == wanted && gate.observer.as_ref().is_none_or(|o| o.world.block(cell) == wanted)
            });
            if !(arrived && settled(world, row) && in_phase > SETTLE) && gate.server_in_step() < STEP_LIMIT - 1.0 {
                return;
            }
            let Some(shot) = gate.shot(world, "shapes") else { return };
            let change = gate.earlier_shot("shapes-before").map_or(0.0, |before| change(&before, &shot, 0.25));
            let result = match (arrived, change >= SCREEN_CHANGE) {
                (true, true) => Ok(format!("{} shaped blocks drawn, changing the view by {change:.1} levels", blocks.len())),
                (false, _) => Err("the shaped blocks did not agree on client and server".into()),
                (true, false) => Err(format!("the shaped blocks barely show: the view changed by {change:.1} levels")),
            };
            gate.check("shapes", result);
            gate.next(Step::Held, now);
        }
        _ => {}
    }
}

/// The lower right of the view, where the hand is (fractions of the frame: x from, x to, y
/// from, y to; y down).
const HAND_AREA: [f32; 4] = [0.5, 1.0, 0.45, 1.0];
/// The rest of the view, which holding something must leave as it was: sky above the hand,
/// clear of the quest cards, the chat and any leaves, which sway in the wind between shots.
const OUT_OF_HAND: [f32; 4] = [0.5, 0.8, 0.0, 0.35];

/// The item in the hand, seen in first person: looking up at the open sky, an empty hand,
/// an iron sword, a stone block and the empty hand again each give a shot; the sword and the
/// block each change the lower right and leave the sky alone (beyond what it drifted between
/// the two empty-hand shots), and differ from each other. Then in a
/// stone box with no light, the sword in hand is much darker (the hand takes the light where
/// the player stands).
///
/// The sky shots are of one scene: a wild creature near when one is taken (a bear once
/// filled the bottom of the first and stood at the top right of the block's) sends the
/// wild ones away and begins them again, `HELD_CLEARS` times at most.
fn held(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    const SWORD: &str = "base:iron_sword";
    const BLOCK: &str = "base:stone";
    let feet = world.resource::<LocalPlayer>().position.floor().as_ivec3();
    let hold = |world: &mut World, name: &str| select(world, name);
    // (Looked for once a shot's wait is nearly over, when the views of those sent away are gone.)
    if (2..=5).contains(&gate.phase) && in_phase > 0.5 && gate.held_cleared < HELD_CLEARS && let Some(near) = wild_near(world, HELD_CLEAR) {
        gate.held_cleared += 1;
        info!("gate: held: {near} before the sky shots; sending the wild creatures away and taking them again");
        world.resource::<Net>().command("creatures disperse");
        empty_hand(world);
        gate.forget(&HELD_SKY);
        gate.phase = 1;
        gate.advance(now);
        return;
    }
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("gamemode creative");
            net.command("time set noon");
            // Nothing wild to walk through the shots of the sky.
            net.command("creatures disperse");
            gate.held_cleared = 0;
            // The full gate's earlier steps fill the hotbar: start from an empty pack, as the
            // steps that give their own items do.
            net.command("clear");
            net.command(&format!("give {} 1", SWORD.trim_start_matches("base:")));
            net.command(&format!("give {} 1", BLOCK.trim_start_matches("base:")));
            gate.advance(now);
        }
        1 => {
            look(world, 0.0, 40.0);
            if holding(world, SWORD).1.is_some() && holding(world, BLOCK).1.is_some() && empty_hand(world) {
                gate.advance(now);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("held", Err("the sword, the stone or a free hotbar slot never came".into()));
                gate.next(Step::NaturalSpawns, now);
            }
        }
        2 if in_phase > 0.5 => {
            if gate.shot(world, "held-empty").is_some() {
                hold(world, SWORD);
                gate.advance(now);
            }
        }
        // The hand comes up from below in a quarter of a second, and its light settles.
        3 if in_phase > 0.8 => {
            if gate.shot(world, "held-sword").is_some() {
                hold(world, BLOCK);
                gate.advance(now);
            }
        }
        4 if in_phase > 0.8 => {
            if gate.shot(world, "held-block").is_some() {
                empty_hand(world);
                gate.advance(now);
            }
        }
        // The empty hand again, for how far the sky drifted (clouds go by in the wind) over
        // the shots of the sword and the block between.
        5 if in_phase > 0.8 => {
            if gate.shot(world, "held-empty-after").is_some() {
                let (x, y, z) = (feet.x, feet.y, feet.z);
                let net = world.resource::<Net>();
                net.command(&format!("fill {} {y} {} {} {} {} stone", x - 3, z - 3, x + 3, y + 5, z + 3));
                net.command(&format!("fill {} {} {} {} {} {} air", x - 2, y, z - 2, x + 2, y + 4, z + 2));
                hold(world, SWORD);
                gate.advance(now);
            }
        }
        // Sealed in: a roof over the eye and nothing alight, and the box drawn. The client
        // holding its blocks is not enough: until their columns are meshed, a shot still
        // shows the sky (a run's `held-sword-dark` did). So the box's corners and roof must
        // be there and every column under it meshed, for `HELD_DRAWN_FRAMES` frames running.
        6 => {
            let (x, y, z) = (feet.x, feet.y, feet.z);
            let cells = [IVec3::new(x, y + 5, z), IVec3::new(x - 3, y, z - 3), IVec3::new(x + 3, y, z - 3), IVec3::new(x - 3, y, z + 3), IVec3::new(x + 3, y + 5, z + 3)];
            let client = world.resource::<ClientWorld>();
            let sealed = cells.iter().all(|&cell| client.block(cell).is_some_and(|b| !b.is_air()));
            let ready = sealed && cells.iter().all(|&cell| drawn(world, cell));
            gate.settled = if ready { gate.settled + 1 } else { 0 };
            if gate.settled >= HELD_DRAWN_FRAMES && in_phase > SETTLE {
                gate.advance(now);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("held", Err(format!("{:.1} server s after the dark box was filled, it is not drawn (all its walls and roof on the client: {sealed})", gate.server_in_phase())));
                world.resource::<Net>().command(&format!("fill {} {y} {} {} {} {} air", x - 3, z - 3, x + 3, y + 5, z + 3));
                empty_hand(world);
                gate.next(Step::NaturalSpawns, now);
            }
        }
        7 if in_phase > 1.0 => {
            if gate.shot(world, "held-sword-dark").is_some() {
                empty_hand(world);
                gate.advance(now);
            }
        }
        8 if in_phase > 0.8 => {
            let Some(empty_dark) = gate.shot(world, "held-empty-dark") else { return };
            let (x, y, z) = (feet.x, feet.y, feet.z);
            world.resource::<Net>().command(&format!("fill {} {y} {} {} {} {} air", x - 3, z - 3, x + 3, y + 5, z + 3));
            let shots: Vec<Option<Image>> = [HELD_SKY[0], HELD_SKY[1], HELD_SKY[2], HELD_SKY[3], "held-sword-dark"].map(|n| gate.earlier_shot(n)).into();
            let [Some(empty), Some(sword), Some(block), Some(empty_after), Some(sword_dark)] = shots.as_slice() else {
                gate.check("held", Err("a shot of the hand was never taken".into()));
                gate.next(Step::NaturalSpawns, now);
                return;
            };
            let in_hand = |a: &Image, b: &Image| change_in(a, b, HAND_AREA);
            let (sword_seen, block_seen) = (in_hand(empty, sword), in_hand(empty, block));
            let apart = in_hand(sword, block);
            // Against whichever empty-hand shot it is nearer to, beyond the sky's own drift between
            // the two: what holding does to the sky shows against both, however it drifts.
            let drift = change_in(empty, empty_after, OUT_OF_HAND);
            let left = |held: &Image| change_in(empty, held, OUT_OF_HAND).min(change_in(empty_after, held, OUT_OF_HAND));
            let (left_sword, left_block) = (left(sword), left(block));
            let dark_seen = in_hand(&empty_dark, sword_dark);
            let result = if sword_seen < HELD_SEEN {
                Err(format!("the sword barely shows in the lower right: it changed the view there by {sword_seen:.2} levels"))
            } else if block_seen < HELD_SEEN {
                Err(format!("the block barely shows in the lower right: it changed the view there by {block_seen:.2} levels"))
            } else if apart < HELD_SEEN {
                Err(format!("the sword and the block look alike in hand: they differ by {apart:.2} levels"))
            } else if left_sword > HELD_ELSEWHERE + drift || left_block > HELD_ELSEWHERE + drift {
                Err(format!(
                    "holding something changed the rest of the view: by {left_sword:.2} (sword) and {left_block:.2} (block) levels, where the sky drifted {drift:.2} between the empty-hand shots"
                ))
            } else if dark_seen > sword_seen * 0.5 {
                Err(format!("the sword is as bright in a sealed dark box ({dark_seen:.2} levels from the empty hand) as under the sky ({sword_seen:.2})"))
            } else {
                Ok(format!(
                    "sword {sword_seen:.1} and block {block_seen:.1} levels in the lower right (differing by {apart:.1}), the rest unchanged ({left_sword:.2}, {left_block:.2}, the sky drifting {drift:.2}); in the dark box the sword shows {dark_seen:.1}"
                ))
            };
            gate.check("held", result);
            gate.next(Step::NaturalSpawns, now);
        }
        _ => {}
    }
}

/// Levels the lower right must change by for the item in hand to count as drawn, and the
/// most the rest of the view may, beyond the sky's drift between the empty-hand shots.
const HELD_SEEN: f32 = 1.0;
const HELD_ELSEWHERE: f32 = 0.5;
/// Blocks round the player within which a wild creature may walk into the held step's
/// shots, and the times the step sends them away and begins those shots again. Only one
/// before the eye counts: within the angle of the look that the server keeps births out
/// of (`spawn_unseen_angle`, wider than the view). Sending the wild away makes room near
/// the player, so births follow within seconds, 20 to 40 blocks off and out of sight:
/// counted too, they retook the shots every run.
const HELD_CLEAR: f32 = 32.0;
const HELD_CLEARS: u32 = 3;
/// The sky shots a retake takes again.
const HELD_SKY: [&str; 4] = ["held-empty", "held-sword", "held-block", "held-empty-after"];

/// The nearest wild creature shown within `reach` of the player and before their eye (none
/// kept by anyone, and no villager: what `/creatures disperse` sends away), by name and
/// distance.
fn wild_near(world: &World, reach: f32) -> Option<String> {
    let player = world.resource::<LocalPlayer>();
    let (at, eye, look) = (player.position, player.eye(), player.look_direction());
    let launch = world.resource::<Launch>();
    // An angle of 0 lets births come in sight: then any counts.
    let angle = launch.content.creatures.limits().spawn_unseen_angle;
    let cos = if angle == 0.0 { -1.0 } else { angle.to_radians().cos() };
    let villager = |v: &voxmar_protocol::CreatureView| launch.content.creatures.get(v.species).def.kind == voxmar_engine::creature::Kind::Villager;
    let before = |v: &voxmar_protocol::CreatureView| look.dot((v.position + Vec3::Y * 0.5 - eye).normalize_or_zero()) >= cos;
    world
        .resource::<crate::creatures::Creatures>()
        .views()
        .filter(|v| !v.kept && v.owner.is_none() && !villager(v) && before(v))
        .map(|v| (v.position.distance(at), v.species))
        .filter(|&(d, _)| d < reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(d, species)| format!("{} {d:.1} blocks off", launch.content.creatures.get(species).def.name))
}

/// How many births `/creatures populate` said it made, from its reply in the chat since
/// `since` (the game's clock); none until the reply has come.
fn populated(world: &World, since: f32) -> Option<usize> {
    const COUNT: &str = "\u{1}";
    let words = world.resource::<Locale>().format("command.creatures.populated", &[("count", &COUNT)]);
    let (before, after) = words.split_once(COUNT)?;
    world.resource::<crate::hud::console::Console>().since(since).find_map(|line| {
        let (_, rest) = line.split_once(before)?;
        rest.strip_suffix(after)?.trim().parse().ok()
    })
}

/// Births through ordinary spawning, driven through admin console inputs. The clear
/// pad makes view-cone rejection observable from replicated birth poses; positive dark
/// monsters and an emptied budget make the subsequent torch rejection meaningful.
///
/// The server makes a populate's births at once and says how many: each is judged once
/// that many new creatures stand on the pad on the client, or after [`NATURAL_WAIT`]
/// server seconds when some never do.
fn natural_spawns(world: &mut World, gate: &mut Gate, now: f32) {
    let pad = gate.feet + IVec3::new(130, 12, 0);
    let (x, y, z) = (pad.x, pad.y, pad.z);
    let arrived = |world: &World, gate: &Gate| {
        let content = &world.resource::<Launch>().content;
        world.resource::<crate::creatures::Creatures>().views().filter(|v| {
            !v.kept && content.creatures.get(v.species).def.kind != voxmar_engine::creature::Kind::Villager
                && !gate.natural_before.contains(&v.id) && (v.position.y - y as f32).abs() < 2.0 && v.position.distance(pad.as_vec3()) < 64.0
        }).copied().collect::<Vec<_>>()
    };
    // Births land 20 to 40 blocks off (`spawn_near`, `spawn_far`), on the pad.
    let all_in = |world: &World, gate: &Gate| {
        gate.server_in_phase() > NATURAL_WAIT || populated(world, gate.phase_since).is_some_and(|told| arrived(world, gate).len() >= told)
    };
    match gate.phase {
        0 => {
            gate.natural_home = world.resource::<LocalPlayer>().position;
            let net = world.resource::<Net>();
            net.command(&format!("fill {} {} {} {} {} {} stone", x - 44, y - 1, z - 44, x + 44, y - 1, z + 44));
            for h in [y, y + 3] {
                net.command(&format!("fill {} {h} {} {} {} {} air", x - 44, z - 44, x + 44, h + 2, z + 44));
            }
            net.command(&format!("tp {} {y} {}", x as f32 + 0.5, z as f32 + 0.5));
            net.command("time set midnight");
            look(world, 0.0, 0.0);
            gate.advance(now);
        }
        1 => {
            look(world, 0.0, 0.0);
            let player = world.resource::<LocalPlayer>();
            if gate.server_in_phase() < 2.0 || !player.on_ground || player.position.floor().as_ivec3() != pad {
                return;
            }
            gate.natural_before = world.resource::<crate::creatures::Creatures>().views().map(|v| v.id).collect();
            let net = world.resource::<Net>();
            net.command("creatures disperse");
            net.command("creatures populate");
            gate.advance(now);
        }
        2 if all_in(world, gate) => {
            let births = arrived(world, gate);
            let content = &world.resource::<Launch>().content;
            let monsters = births.iter().filter(|v| content.creatures.get(v.species).def.kind == voxmar_engine::creature::Kind::Monster).count();
            let angle = content.creatures.limits().spawn_unseen_angle;
            let eye = pad.as_vec3() + Vec3::new(0.5, voxmar_engine::physics::player::EYE_HEIGHT, 0.5);
            let visible = births.iter().any(|v| {
                let turn = Quat::from_rotation_y(v.yaw);
                content.creatures.get(v.species).parts.iter().any(|part| (0..9).any(|i| {
                    let local = if i == 8 { Vec3::ZERO } else {
                        Vec3::new(if i & 1 == 0 { -0.5 } else { 0.5 }, if i & 2 == 0 { -0.5 } else { 0.5 }, if i & 4 == 0 { -0.5 } else { 0.5 }) * part.size
                    };
                    let to = v.position + turn * (part.at_rest(local) * v.scale) - eye;
                    -Vec3::Z.dot(to.normalize_or_zero()) >= angle.to_radians().cos()
                }))
            });
            let cap = content.creatures.limits().monsters_near;
            let uncleared = world.resource::<crate::creatures::Creatures>().views().any(|v| {
                !v.kept && content.creatures.get(v.species).def.kind != voxmar_engine::creature::Kind::Villager
                    && gate.natural_before.contains(&v.id) && v.position.distance(pad.as_vec3()) < 64.0
            });
            gate.check("natural births", if monsters > 0 && monsters <= cap && !visible && !uncleared {
                Ok(format!("{} births reached the client, including {monsters} dark monsters; no model part appeared in the forward {angle} degree cone", births.len()))
            } else {
                Err(format!("{} births, {monsters} monsters (cap {cap}), visible model {visible}, prior population still present {uncleared}", births.len()))
            });
            gate.natural_before = births.iter().map(|v| v.id).collect();
            let net = world.resource::<Net>();
            net.command("creatures disperse");
            for dx in (-40..=40).step_by(10) {
                for dz in (-40..=40).step_by(10) {
                    net.command(&format!("setblock {} {y} {} torch", x + dx, z + dz));
                }
            }
            gate.advance(now);
        }
        3 => {
            let creatures = world.resource::<crate::creatures::Creatures>();
            let cleared = creatures.views().all(|v| !gate.natural_before.contains(&v.id));
            let content = &world.resource::<Launch>().content;
            let torch = content.blocks.id("base:torch");
            let client = world.resource::<ClientWorld>();
            let lit = [-40, 0, 40].into_iter().all(|dx| [-40, 0, 40].into_iter().all(|dz| client.block(pad + IVec3::new(dx, 0, dz)) == torch));
            if gate.server_in_phase() < 2.0 || !cleared || !lit {
                return;
            }
            gate.natural_before = creatures.views().map(|v| v.id).collect();
            world.resource::<Net>().command("creatures populate");
            gate.advance(now);
        }
        4 if all_in(world, gate) => {
            let content = &world.resource::<Launch>().content;
            let monsters = arrived(world, gate).iter().filter(|v| content.creatures.get(v.species).def.kind == voxmar_engine::creature::Kind::Monster).count();
            gate.check("natural torch protection", if monsters == 0 { Ok("no monsters born on the lit pad with the dark population dispersed".into()) } else { Err(format!("{monsters} monsters born among torches")) });
            let net = world.resource::<Net>();
            net.command("creatures disperse");
            let home = gate.natural_home;
            net.command(&format!("tp {} {} {}", home.x, home.y, home.z));
            net.command("time set noon");
            gate.next(Step::Creatures, now);
        }
        _ => {}
    }
}

/// Server seconds the natural spawns step waits for a populate's births at most.
const NATURAL_WAIT: f32 = 3.0;

/// Frames slower than this (milliseconds) draw a body's 0.45 s tip over in fewer than the
/// three frames the falls check asks for.
const FALL_FRAME_MS: f32 = 150.0;

/// The species the gate summons and strikes: slow and fearless, so it stays to be struck.
const CREATURE: &str = "base:lumen_snail";
/// What it always drops.
const CREATURE_DROP: &str = "base:lumen_slime";
/// A small bird summoned with them, and a butterfly released over the pad.
const BIRD: &str = "base:goldfinch";
const BUTTERFLY: &str = "base:cabbage_white";

/// A lumen snail and a few grazers are summoned on open test ground: they must reach the
/// client and draw, and the snail, struck with left clicks as a player would, must die
/// and drop its slime once its body has fallen, which walking to it picks up.
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
            net.command(&format!("summon shambler 1 {} {} {}", pad.x + 3, pad.y, pad.z));
            net.command(&format!("summon {BIRD} 1 {} {} {}", pad.x - 3, pad.y, pad.z));
            // A butterfly over the pad, in view and out of the player's way.
            let butterfly = Vec3::new(pad.x as f32 - 1.5, pad.y as f32 + 1.6, pad.z as f32 + 0.5);
            gate.butterfly = crate::butterflies::release(world, BUTTERFLY, butterfly, 30.0);
            gate.advance(now);
        }
        2 => {
            let Some(view) = snail(world) else {
                if gate.server_in_phase() > 10.0 {
                    gate.check("creatures", Err("a summoned creature never reached the client".into()));
                    gate.next(Step::Ore, now);
                }
                return;
            };
            aim_at(world, view.position + Vec3::Y * 0.2);
            if in_phase < 1.0 {
                return;
            }
            let eyes = world.resource::<crate::creatures::Creatures>().front_eyes(world.resource::<Launch>(), world.resource::<Assets<Image>>());
            let Some(eyes) = eyes else {
                if gate.server_in_phase() <= 10.0 {
                    return;
                }
                gate.check("creature front eyes", Err("the front-eyed creature's painted skin never arrived".into()));
                world.resource::<Net>().command("creatures remove shambler");
                gate.next(Step::Strike, now);
                return;
            };
            if gate.shot(world, "creatures").is_some() {
                gate.check("creature front eyes", eyes);
                world.resource::<Net>().command("creatures remove shambler");
                let shown = world.resource::<crate::creatures::Creatures>().alive();
                gate.check("creatures shown", Ok(format!("{shown} creatures drawn on the test ground")));
                // A small bird reaches the client like any creature; a butterfly is the client's
                // own, drawn as two wings.
                let bird = world.resource::<Launch>().content.creatures.find(BIRD);
                let birds = world.resource::<crate::creatures::Creatures>().views().filter(|v| Some(v.species) == bird).count();
                gate.check("birds", if birds > 0 { Ok(format!("a summoned {BIRD} reached the client")) } else { Err(format!("a summoned {BIRD} never reached the client")) });
                let wings = crate::butterflies::drawn(world);
                gate.check(
                    "butterflies",
                    if !gate.butterfly {
                        Err(format!("no winged set `{BUTTERFLY}` to release"))
                    } else if wings >= 2 {
                        Ok(format!("a released {BUTTERFLY} drawn, {wings} wing quads"))
                    } else {
                        Err(format!("a released {BUTTERFLY} drew {wings} wing quads"))
                    },
                );
                gate.check("creature voices", crate::creature_sounds::loaded(world));
                // Struck bare-handed at the weapon step's level (it sets the same next), sent
                // once the shot is taken so its reply is not in it: a fist there lands about
                // five blows in six for up to 3, so the snail's 4 health goes in two or three
                // blows. At Attack 1 it took four landed blows at even odds, 4 to 12 swings
                // from run to run.
                world.resource::<Net>().command(&format!("skill attack {WEAPON_LEVEL}"));
                gate.advance(now);
            }
        }
        3 => {
            // The one pointed at shows its level and health over it: judged on a frame the
            // crosshair rested on a creature as drawn when the label was laid out (the camera
            // as the last frame aimed it, the creatures where this frame draws them). A frame
            // that drew the snail off the crosshair, as a long one does when it has walked
            // from where the server last put it, says nothing of the label. Aimed at the snail
            // as drawn until one does, for `CREATURE_LEVEL_FRAMES` frames and as many server
            // seconds at least.
            let pointed = crate::hud::levels::pointed(world.resource(), world.resource(), world.resource(), world.resource(), world.resource()).is_some();
            if pointed {
                let shown = world.query_filtered::<&Visibility, With<crate::hud::levels::LevelLabel>>().iter(world).any(|v| *v != Visibility::Hidden);
                gate.check("creature level", if shown { Ok("the creature pointed at shows its level and health over it".into()) } else { Err("the creature pointed at shows no level over it".into()) });
                gate.next(Step::Strike, now);
                return;
            }
            gate.settled += 1;
            if gate.settled > CREATURE_LEVEL_FRAMES && gate.server_in_phase() > CREATURE_LEVEL_FRAMES as f32 {
                gate.check("creature level", Err(format!("the crosshair, aimed at the snail as drawn, rested on no creature in {} frames", gate.settled)));
                gate.next(Step::Strike, now);
                return;
            }
            let drawn = snail(world).and_then(|view| world.resource::<crate::creatures::Creatures>().centre(world.resource::<Launch>(), view.id));
            if let Some((centre, _)) = drawn {
                aim_at(world, centre);
            }
        }
        _ => {}
    }
}

/// Frames, and server seconds, the creature-level check aims at the snail as drawn for a
/// frame with the crosshair on a creature before failing.
const CREATURE_LEVEL_FRAMES: u32 = 10;

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
            let strikes = world.resource::<crate::fight::Fighting>().strikes;
            if blows > gate.blows_seen {
                gate.blows_seen = blows;
                gate.phase_since = now;
                gate.phase_server = gate.server_now;
                gate.strike_answered(strikes);
            }
            let stalled = gate.unanswered(strikes);
            // Struck with the empty hand, its first punch watched in the hand once the hand
            // is up (`watch_swing`).
            if gate.swing_watch.name.is_empty() {
                gate.swing_watch = SwingWatch::of("player punch", "player-punch-connect", "the snail");
                empty_hand(world);
            }
            match snail(world) {
                Some(view) if !stalled && blows < MOST_BLOWS => {
                    gate.quarry = view.position;
                    gate.quarry_id = Some(view.id);
                    if landed > 0 {
                        gate.struck_heights = (gate.struck_heights.0.min(view.position.y), gate.struck_heights.1.max(view.position.y));
                    }
                    // Close in to striking reach, then strike.
                    let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 1.6;
                    key(world, KeyCode::KeyW, far);
                    aim_at(world, view.position + Vec3::Y * 0.2);
                    if now - gate.since < PUNCH_AFTER || !watch_swing(world, gate, far, now) {
                        gate.swing(world, now);
                    }
                }
                Some(view) => {
                    mouse(world, MouseButton::Left, false);
                    key(world, KeyCode::KeyW, false);
                    let aimed = world.resource::<crate::interact::Target>().creature.is_some();
                    let distance = view.position.distance(world.resource::<LocalPlayer>().eye());
                    let why = if stalled { format!("the server answered none of {} strikes in {UNANSWERED}s", gate.unanswered_strikes(strikes)) } else { format!("{blows} blows ({landed} landed) did not kill it") };
                    gate.check("creature struck", Err(format!("{why} (aimed at it: {aimed}, {distance:.1} blocks from the eye)")));
                    if gate.swing_watch.stage < 2 {
                        judge_swing(world, gate, false);
                    }
                    gate.next(Step::Ore, now);
                }
                None => {
                    mouse(world, MouseButton::Left, false);
                    key(world, KeyCode::KeyW, false);
                    if gate.swing_watch.stage < 2 {
                        let answered = world.resource::<crate::creatures::Creatures>().blows > gate.swing_watch.pressed.1;
                        judge_swing(world, gate, answered);
                    }
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
                    // Why none: the deaths the creatures counted (each cries as it is counted),
                    // and the cries still waiting for the sound system.
                    let waiting = world.resource::<crate::creature_sounds::CreatureSounds>().0.iter().filter(|s| matches!(s.kind, crate::creature_sounds::Kind::Hurt | crate::creature_sounds::Kind::Death)).count();
                    let result = if cries.iter().all(|&n| n > 0) {
                        Ok(format!("{} hurt and {} death sounds asked for", cries[0], cries[1]))
                    } else {
                        Err(format!("hurt and death sounds asked for: {cries:?} ({deaths} deaths counted, {waiting} cries not yet asked for)"))
                    };
                    gate.check("creature cries", result);
                    if !gate.readout_seen {
                        gate.check("target readout", Err("it never showed the struck creature's name, level and health".into()));
                    }
                    gate.advance(now);
                }
            }
        }
        1 => {
            // The old loot walk aimed a small corpse below the viewport immediately
            // after its death. Give its real mark a view before moving over the drops.
            if !gate.fallen_watched {
                let anchor = gate.quarry_id.and_then(|id| {
                    world.resource::<crate::creatures::Creatures>().centre(world.resource::<Launch>(), id)
                }).map(|(centre, _)| centre);
                let Some(result) = watch_fallen(world, gate.quarry_id, anchor, &mut gate.fallen_since, now) else { return };
                gate.fallen_watched = true;
                gate.check("combat marks: slain snail", result.map(|seen| format!("its mark stayed visible for {seen:.2} s before the loot walk"))
                    .map_err(|seen| format!("its mark stayed visible for only {seen:.2} s during the bounded corpse watch")));
                gate.shot(world, "fallen-splat");
                return;
            }
            // How its body fell, once it has: over onto its side close to where it stood,
            // and never drawn around the eye (from close up, its inside showed).
            if !gate.fall_judged {
                let fall = world.resource::<crate::creatures::Creatures>().falls.last().copied();
                if let Some(fall) = fall {
                    gate.fall_judged = true;
                    // Judged by what was drawn: every joint of the body, every frame of its fall.
                    // Watched over several frames, not one: three samples of every joint. The
                    // tip over lasts half its death's 0.9 s, so three frames need frames under
                    // 150 ms: a machine drawing slower saw fewer, which is known, not a fault,
                    // as long as it saw the body fall at all. The frames that count are those
                    // of the tip itself (`Fall::slowest`): the last frames before this check
                    // come after the corpse watch, seconds later, and a loaded machine's slow
                    // frames during the fall once failed it with "frames up to 15 ms".
                    let joints = fall.joints.max(1);
                    let slowest = fall.slowest * 1000.0;
                    let few = fall.samples < 3 * joints;
                    let result = if fall.samples < joints || (few && slowest < FALL_FRAME_MS) {
                        Err(format!("its fall was hardly seen: {} joint samples for {} joints (frames up to {slowest:.0} ms)", fall.samples, fall.joints))
                    } else if fall.spread > 1.5 {
                        Err(format!("a joint of its body was drawn {:.2} of its heights from where it died", fall.spread))
                    } else if fall.scale > 0.05 {
                        Err(format!("a joint of its body was drawn {:.0}% off its standing size", fall.scale * 100.0))
                    } else {
                        let eye = if fall.around_eye { "the eye was inside it, and it was hidden" } else { "away from the eye" };
                        Ok(format!("drawn within {:.2} of its heights, joints at most {:.1}% off size; {eye}", fall.spread, fall.scale * 100.0))
                    };
                    if few && result.is_ok() {
                        let detail = format!("its fall was seen in {} frames, with frames up to {slowest:.0} ms", fall.samples / joints);
                        gate.known("creature falls: frames", Err(detail), "a loaded machine draws the 0.45 s tip in fewer than three frames");
                    }
                    gate.check("creature falls", result);
                } else if in_phase > 4.0 {
                    gate.fall_judged = true;
                    gate.check("creature falls", Err("its body never finished falling".into()));
                }
            }
            // Stand on the slime, as a player picks up loot. It comes out only once the body
            // has fallen (a second of the server's time after the kill, later on a server
            // running behind), popped a little way off where it died: until it shows, the
            // player walks over that spot; once it lies, the player is put where it lies (a
            // walk there could be blocked by the other creatures on the pad, and where it
            // lies depends on where the fight ended), and the server picks it up.
            let feet = world.resource::<LocalPlayer>().position;
            let slime = world.resource::<ClientWorld>().registry.id(CREATURE_DROP);
            let quarry = gate.quarry;
            let lying: Vec<Vec3> = world.resource::<crate::drops::Drops>().lying().filter(|(_, stack, at)| Some(stack.item) == slime && at.distance(quarry) < 6.0).map(|(_, _, at)| at).collect();
            let nearest = lying.iter().copied().min_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)));
            if let (Some(at), None) = (nearest, gate.loot_stood) {
                key(world, KeyCode::KeyW, false);
                world.resource::<Net>().command(&format!("tp {} {} {}", at.x, at.y.floor(), at.z));
                gate.loot_stood = Some(at);
            } else if gate.loot_stood.is_none() {
                let close = feet.xz().distance(quarry.xz()) < 0.4;
                key(world, KeyCode::KeyW, !close);
                if !close {
                    aim_at(world, quarry + Vec3::Y * 1.6);
                }
            }
            // Picking up is the server's work: waited on by its clock.
            let count = item_count(world, CREATURE_DROP);
            if count > gate.count_before || gate.server_in_phase() > 8.0 {
                key(world, KeyCode::KeyW, false);
                let result = if count > gate.count_before {
                    Ok(format!("picked up {} {CREATURE_DROP}", count - gate.count_before))
                } else {
                    let room = slime.map_or(0, |item| world.resource::<LocalPlayer>().inventory.room_for(item, 1));
                    let lying: Vec<String> = lying.iter().map(|at| format!("{:.1} blocks off", at.distance(feet))).collect();
                    let stood = gate.loot_stood.map_or("never put by it (none lay)".to_string(), |at| format!("put where it lay at {at:.1}"));
                    Err(format!(
                        "no {CREATURE_DROP} picked up in 8 server s ({in_phase:.1} s) after the kill: slime lying near where it died {lying:?}, the player {:.1} blocks from that spot, {stood}, room for {room}",
                        feet.xz().distance(quarry.xz())
                    ))
                };
                gate.check("creature drops", result);
                gate.next(Step::Weapon, now);
            }
        }
        _ => {}
    }
}

/// Game seconds into the strike step before its first punch is watched: the empty hand
/// comes up from below in a quarter of a second.
const PUNCH_AFTER: f32 = 0.5;

/// The weapon the gate strikes with, and the level that wields it.
const WEAPON: &str = "base:iron_sword";
const WEAPON_LEVEL: u8 = 20;
/// What it strikes: big enough to take several blows, in creative so its own pass by.
const BEAR: &str = "base:brown_bear";

/// An iron sword at Attack 20 against a brown bear on the pad: struck until it dies, some
/// blow must do more than a bare fist's most at that level (1 + 20 / 8 = 3).
fn weapon(world: &mut World, gate: &mut Gate, now: f32, _in_phase: f32) {
    let Some(species) = world.resource::<Launch>().content.creatures.find(BEAR) else {
        gate.check("weapon", Err(format!("no `{BEAR}` in the content")));
        gate.next(Step::Cook, now);
        return;
    };
    let fist = voxmar_engine::combat::Weapon::FISTS.max_hit(WEAPON_LEVEL);
    let pad = gate.feet + IVec3::new(0, 0, 14);
    let view = the_bear(world, gate, species);
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("gamemode creative");
            net.command(&format!("skill attack {WEAPON_LEVEL}"));
            net.command(&format!("give {} 1", WEAPON.trim_start_matches("base:")));
            net.command(&format!("summon {} 1 {} {} {}", BEAR.trim_start_matches("base:"), pad.x, pad.y, pad.z + 1));
            gate.blows_seen = world.resource::<crate::creatures::Creatures>().blows;
            gate.feel = FeelRun::default();
            gate.swing_watch = SwingWatch::of("player swing", "player-swing-connect", "the bear");
            gate.advance(now);
        }
        1 => {
            // The sword in hand, at the level that wields it, and the bear in sight.
            let ready = skill_level(world, "core:attack").0 >= WEAPON_LEVEL && view.is_some();
            if ready && select(world, WEAPON) {
                gate.advance(now);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("weapon", Err(format!("the sword, Attack {WEAPON_LEVEL} or the bear never came (bear seen: {})", view.is_some())));
                end_weapon(world, gate, now);
            }
        }
        2 => {
            let (blows, hardest) = {
                let creatures = world.resource::<crate::creatures::Creatures>();
                (creatures.blows, creatures.hardest)
            };
            let strikes = world.resource::<crate::fight::Fighting>().strikes;
            if blows > gate.blows_seen {
                gate.blows_seen = blows;
                gate.phase_since = now;
                gate.phase_server = gate.server_now;
                gate.strike_answered(strikes);
            }
            let stalled = gate.unanswered(strikes);
            match view {
                Some(view) if !stalled => {
                    // Close in and strike a full charge apart, as a player swings.
                    let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 2.0;
                    key(world, KeyCode::KeyW, far);
                    aim_at(world, view.position + Vec3::Y * 0.6);
                    if !watch_swing(world, gate, far, now) {
                        gate.swing(world, now);
                    }
                }
                Some(_) => {
                    gate.check("weapon strikes", Err(format!("the server answered none of {} strikes in {UNANSWERED}s", gate.unanswered_strikes(strikes))));
                    end_weapon(world, gate, now);
                }
                // Out of sight: dead only if it was seen dying.
                None => {
                    if gate.feel.died && !gate.weapon_fallen_watched {
                        // Cleanup teleports into the corpse and removes its view. Watch
                        // this actual bear's mark before issuing any sweep commands.
                        let anchor = gate.feel.bear.and_then(|id| {
                            world.resource::<crate::creatures::Creatures>().centre(world.resource::<Launch>(), id)
                        }).map(|(centre, _)| centre);
                        let Some(result) = watch_fallen(world, gate.feel.bear, anchor, &mut gate.weapon_fallen_since, now) else { return };
                        gate.weapon_fallen_watched = true;
                        gate.check("combat marks: slain bear", result.map(|seen| format!("its mark stayed visible for {seen:.2} s before cleanup"))
                            .map_err(|seen| format!("its mark stayed visible for only {seen:.2} s during the bounded corpse watch")));
                        return;
                    }
                    let result = if !gate.feel.died {
                        Err("the bear went out of sight alive".to_string())
                    } else if hardest > fist {
                        Ok(format!("the bear died; the hardest blow did {hardest}, a fist's most is {fist}"))
                    } else {
                        Err(format!("no blow did more than a fist's {fist} (the hardest did {hardest})"))
                    };
                    gate.check("weapon strikes", result);
                    end_weapon(world, gate, now);
                }
            }
        }
        _ => {
            if sweep_bears(world, gate, species, pad) {
                gate.next(Step::Sling, now);
            }
        }
    }
}

/// The sling, the Ranged level it is shot at, and what it shoots. The level is high enough
/// that a stone hits the bear about four times in five, so twelve misses in a row are
/// never luck (`twelve_misses_are_never_luck`); at Ranged 10 a third hit, and one gate in a
/// hundred or so missed with all twelve.
const SLING: &str = "base:sling";
const SLING_LEVEL: u8 = 70;
const SLING_STONES: u32 = 12;
/// Seconds between shots (past the sling's 0.53 s cooldown), by the game's clock.
const SLING_EVERY: f32 = 1.0;

/// The sling step's run: what stood before it began, its bear, and its shots.
#[derive(Default)]
struct SlingRun {
    /// Bears in view before the step summoned its own, which it never takes for it.
    before: Vec<u32>,
    bear: Option<u32>,
    /// Blows landed, blows answered, Ranged experience and cobblestone as shooting began.
    landed: u32,
    blows: u32,
    xp: u32,
    stones: u32,
    /// Shots clicked, frames the bear has been aimed at, and when the last shot went (the
    /// game's clock, the server's).
    shots: u32,
    aimed: u32,
    shot_at: Option<(f32, f32)>,
    /// Since when (the game's clock) a block has stood on the line while shooting.
    in_the_way: Option<f32>,
    /// The blocks that stood on the line from the eye to the bear (the look step's wall, or
    /// whatever else earlier steps or the land left there), and what each was: taken away
    /// for the shots and put back on every way out.
    cleared: Vec<(IVec3, String)>,
}

/// One cell set to `block` by name.
fn set_cell(cell: IVec3, block: &str) -> String {
    format!("fill {} {} {} {} {} {} {block}", cell.x, cell.y, cell.z, cell.x, cell.y, cell.z)
}

/// The cells between an eye and a creature standing at `feet`: on the lines to its feet,
/// its middle and its top, each once.
fn shot_lines(eye: Vec3, feet: Vec3) -> Vec<IVec3> {
    let mut cells = Vec::new();
    for height in [0.2, 0.6, 1.0, 1.4] {
        for cell in cells_between(eye, feet + Vec3::Y * height) {
            if !cells.contains(&cell) {
                cells.push(cell);
            }
        }
    }
    cells
}

/// The cells a straight line from `from` to `to` passes through, in order, each once.
fn cells_between(from: Vec3, to: Vec3) -> Vec<IVec3> {
    let steps = (from.distance(to) / 0.05).ceil().max(1.0) as u32;
    let mut cells: Vec<IVec3> = Vec::new();
    for i in 0..=steps {
        let cell = from.lerp(to, i as f32 / steps as f32).floor().as_ivec3();
        if cells.last() != Some(&cell) {
            cells.push(cell);
        }
    }
    cells
}

/// What the sling step does next.
#[derive(Debug, PartialEq)]
enum SlingNext {
    /// A stone hit and trained Ranged.
    Hit,
    /// Click once more.
    Shoot,
    /// A shot is on its way.
    Wait,
    Fail(String),
}

/// The sling step's verdict from counts alone, so its logic is tested without a session.
/// Nothing fails before the first shot: every miss verdict counts shots answered, and an
/// unanswered shot is judged by both clocks since it went, never by the stage's clock.
/// `blocked` names the block the crosshair is on instead of the bear, as a shot would go:
/// a shot that cannot reach is a failure at once, not twelve seconds of waiting.
fn sling_next(shots: u32, answered: u32, landed: u32, xp_gained: u32, stones: u32, unanswered_for: f32, blocked: Option<&str>) -> SlingNext {
    if landed > 0 && xp_gained > 0 {
        return SlingNext::Hit;
    }
    let shoot = || match blocked {
        Some(block) => SlingNext::Fail(format!("the crosshair is on {block}, not the bear, after {shots} shots")),
        None => SlingNext::Shoot,
    };
    if shots == 0 {
        return shoot();
    }
    if answered < shots {
        return if unanswered_for > UNANSWERED { SlingNext::Fail(format!("shot {shots} went unanswered for {unanswered_for:.1} s ({answered} answered)")) } else { SlingNext::Wait };
    }
    if answered >= SLING_STONES || stones == 0 {
        return SlingNext::Fail(format!("no stone hit in {answered} shots answered ({landed} landed, {xp_gained} Ranged xp, {stones} stones left)"));
    }
    shoot()
}

/// A sling at Ranged 70 against a held brown bear seven blocks off: shot a second apart
/// until a stone hits, every shot answered spends one cobblestone, and the hit trains Ranged.
/// Every way out (a hit, a failure, a bear that never came) sweeps the bears away.
fn sling(world: &mut World, gate: &mut Gate, now: f32, _in_phase: f32) {
    let Some(species) = world.resource::<Launch>().content.creatures.find(BEAR) else {
        gate.check("sling", Err(format!("no `{BEAR}` in the content")));
        gate.next(Step::Feel, now);
        return;
    };
    let pad = gate.feet + IVec3::new(0, 0, 14);
    let stones = item_count(world, "base:cobblestone");
    let (blows, landed) = {
        let creatures = world.resource::<crate::creatures::Creatures>();
        (creatures.blows, creatures.landed)
    };
    let view = {
        let creatures = world.resource::<crate::creatures::Creatures>();
        match gate.sling.bear {
            Some(id) => creatures.views().find(|v| v.id == id).copied(),
            None => creatures.views().find(|v| v.species == species && !gate.sling.before.contains(&v.id)).copied(),
        }
    };
    match gate.phase {
        0 => {
            let before = world.resource::<crate::creatures::Creatures>().views().filter(|v| v.species == species).map(|v| v.id).collect();
            gate.sling = SlingRun { before, ..default() };
            let net = world.resource::<Net>();
            net.command("gamemode creative");
            net.command(&format!("skill ranged {SLING_LEVEL}"));
            net.command(&format!("give {} 1", SLING.trim_start_matches("base:")));
            net.command(&format!("give cobblestone {SLING_STONES}"));
            // Held from the moment it comes, so it never walks in on the line before a shot.
            net.command(&format!("summon {} 1 {} {} {} held", BEAR.trim_start_matches("base:"), pad.x, pad.y, pad.z + 1));
            // Seven blocks back from it, past any blow's reach and inside the sling's.
            net.command(&format!("tp {} {} {}", pad.x as f32 + 0.5, pad.y, pad.z as f32 - 6.5));
            gate.advance(now);
        }
        1 => {
            // Its own bear in sight, the sling in hand at the level that wields it, and the
            // stones given (on top of any the player had).
            // At the spot with the bear in sight, each frame until ready: whatever stands
            // between the eye and the bear (on the lines to its feet, middle and top, which
            // hold both the crosshair's aim and the server's line to its centre) is named,
            // taken away, and remembered to put back. Looked along again each frame, so a
            // bear that moved or a cell that loaded late is cleared too.
            let eye = world.resource::<LocalPlayer>().eye();
            let at_spot = eye.xz().distance(Vec2::new(pad.x as f32 + 0.5, pad.z as f32 - 6.5)) < 0.5;
            let line = view.filter(|_| at_spot).map(|v| shot_lines(eye, v.position));
            let standing: Vec<(IVec3, BlockId)> = line.iter().flatten().filter_map(|&cell| world.resource::<ClientWorld>().block(cell).filter(|b| !b.is_air()).map(|b| (cell, b))).collect();
            let new: Vec<(IVec3, String)> = {
                let blocks = &world.resource::<Launch>().content.blocks;
                standing.iter().filter(|(cell, _)| !gate.sling.cleared.iter().any(|(c, _)| c == cell)).map(|&(cell, b)| (cell, blocks.def(b).name.clone())).collect()
            };
            if !new.is_empty() {
                info!("gate: sling line from the eye to the bear: {} blocks in the way {new:?}", new.len());
                let net = world.resource::<Net>();
                for (cell, _) in &new {
                    net.command(&set_cell(*cell, "air"));
                }
                gate.sling.cleared.extend(new);
            }
            let open = line.is_some() && standing.is_empty();
            let ready = skill_level(world, "core:ranged").0 >= SLING_LEVEL && view.is_some() && stones >= SLING_STONES && open;
            if ready && select(world, SLING) {
                let bear = view.map(|v| v.id);
                info!("gate: sling in hand, bear {bear:?} in sight, {stones} cobblestone, Ranged {}", skill_level(world, "core:ranged").0);
                world.resource::<Net>().command(&format!("creatures hold {}", BEAR.trim_start_matches("base:")));
                let xp = skill_level(world, "core:ranged").1;
                let (before, cleared) = (std::mem::take(&mut gate.sling.before), std::mem::take(&mut gate.sling.cleared));
                gate.sling = SlingRun { before, cleared, bear, landed, blows, xp, stones, ..default() };
                gate.advance(now);
            } else if gate.server_in_phase().min(now - gate.phase_since) > 10.0 {
                let held = holding(world, SLING).1;
                gate.check("sling", Err(format!("the sling, Ranged {SLING_LEVEL}, the stones, the bear or a clear pad never came (bear seen: {}, stones {stones}, sling in slot {held:?}, line clear {open}: {standing:?})", view.is_some())));
                end_weapon(world, gate, now);
            }
        }
        2 => {
            let Some(view) = view else {
                gate.check("sling shot", Err("the bear went out of sight".into()));
                end_weapon(world, gate, now);
                return;
            };
            key(world, KeyCode::KeyW, false);
            aim_at(world, view.position + Vec3::Y * 0.6);
            gate.sling.aimed += 1;
            // Before every shot the line is looked along again: a block that came under it
            // (the bear nearer, or a cell loaded late) is named, cleared and kept to put back,
            // and nothing is shot until the line is open.
            let eye = world.resource::<LocalPlayer>().eye();
            let standing: Vec<(IVec3, BlockId)> = shot_lines(eye, view.position).into_iter().filter_map(|cell| world.resource::<ClientWorld>().block(cell).filter(|b| !b.is_air()).map(|b| (cell, b))).collect();
            if !standing.is_empty() {
                let new: Vec<(IVec3, String)> = {
                    let blocks = &world.resource::<Launch>().content.blocks;
                    standing.iter().filter(|(cell, _)| !gate.sling.cleared.iter().any(|(c, _)| c == cell)).map(|&(cell, b)| (cell, blocks.def(b).name.clone())).collect()
                };
                if !new.is_empty() {
                    let distance = eye.distance(view.position + Vec3::Y * 0.6);
                    info!("gate: sling line before shot {}, bear {distance:.1} blocks off: {} blocks in the way {new:?}", gate.sling.shots + 1, new.len());
                    let net = world.resource::<Net>();
                    for (cell, _) in &new {
                        net.command(&set_cell(*cell, "air"));
                    }
                    gate.sling.cleared.extend(new);
                }
                let since = *gate.sling.in_the_way.get_or_insert(now);
                if now - since > 5.0 {
                    let blocks = &world.resource::<Launch>().content.blocks;
                    let named: Vec<_> = standing.iter().map(|&(cell, b)| (cell, blocks.def(b).name.clone())).collect();
                    gate.check("sling shot", Err(format!("the line to the bear stayed blocked for 5 s after clearing: {named:?}")));
                    end_weapon(world, gate, now);
                }
                mouse(world, MouseButton::Left, false);
                return;
            }
            gate.sling.in_the_way = None;
            let run = &gate.sling;
            let xp = skill_level(world, "core:ranged").1;
            let unanswered = run.shot_at.map_or(0.0, |(game, server)| (now - game).min(gate.server_now - server));
            // What the crosshair is on instead of the bear, once the aim has settled.
            let blocked = {
                let target = world.resource::<crate::interact::Target>();
                let blocks = &world.resource::<Launch>().content.blocks;
                let cells = world.resource::<ClientWorld>();
                target.block_hit.filter(|_| run.aimed > 2 && target.creature.is_none()).map(|hit| {
                    let name = cells.block(hit.block).map_or_else(|| "an unloaded block".to_owned(), |b| blocks.def(b).name.clone());
                    format!("{name} at {} ({:.2} blocks)", hit.block, hit.distance)
                })
            };
            let next = sling_next(run.shots, blows.saturating_sub(run.blows), landed.saturating_sub(run.landed), xp.saturating_sub(run.xp), stones, unanswered, blocked.as_deref());
            // The press of the shot before is let go first, so each shot is its own click.
            mouse(world, MouseButton::Left, false);
            match next {
                SlingNext::Hit => {
                    // Every shot the server answered spent a stone: the hit, and any misses before it.
                    let spent = run.stones.saturating_sub(stones);
                    let distance = world.resource::<LocalPlayer>().eye().distance(view.position + Vec3::Y * 0.6);
                    let result = if spent >= 1 {
                        Ok(format!("a stone hit from {distance:.1} blocks in {} shots; Ranged xp {} -> {xp}, {spent} stones spent", run.shots, run.xp))
                    } else {
                        Err(format!("a stone hit from {distance:.1} blocks, but no cobblestone was spent ({stones} left)"))
                    };
                    gate.check("sling shot", result);
                    end_weapon(world, gate, now);
                }
                SlingNext::Fail(why) => {
                    let distance = world.resource::<LocalPlayer>().eye().distance(view.position + Vec3::Y * 0.6);
                    gate.check("sling shot", Err(format!("{why}; the bear {distance:.1} blocks off")));
                    end_weapon(world, gate, now);
                }
                SlingNext::Shoot if run.aimed > 2 && run.shot_at.is_none_or(|(game, _)| now - game >= SLING_EVERY) => {
                    mouse(world, MouseButton::Left, true);
                    gate.sling.shots += 1;
                    gate.sling.shot_at = Some((now, gate.server_now));
                    // What the crosshair is on as it shoots, so a shot that never goes says why.
                    let target = world.resource::<crate::interact::Target>();
                    let distance = world.resource::<LocalPlayer>().eye().distance(view.position + Vec3::Y * 0.6);
                    info!(
                        "gate: sling shot {} at bear {} {distance:.1} blocks off: crosshair on creature {:?}, block {:?}; {} answered",
                        gate.sling.shots,
                        view.id,
                        target.creature,
                        target.block_hit.map(|hit| (hit.block, hit.distance)),
                        blows.saturating_sub(gate.sling.blows)
                    );
                }
                SlingNext::Shoot | SlingNext::Wait => {}
            }
        }
        _ => {
            // What stood on the line back where it stood, then the bears swept away.
            for (cell, block) in std::mem::take(&mut gate.sling.cleared) {
                world.resource::<Net>().command(&set_cell(cell, &block));
            }
            if sweep_bears(world, gate, species, pad) {
                gate.next(Step::Feel, now);
            }
        }
    }
}

/// Hands free, then the bears are swept away (phase 3) before the fight's feel.
fn end_weapon(world: &mut World, gate: &mut Gate, now: f32) {
    if gate.swing_watch.stage < 2 {
        judge_swing(world, gate, false);
    }
    mouse(world, MouseButton::Left, false);
    key(world, KeyCode::KeyW, false);
    gate.sweep = Sweep::default();
    to_phase(gate, 3, now);
}

/// The weapon step's first swing at the bear, watched in the hand frame by frame on the
/// gate's clock (`held::HeldClock`), so a slow machine sees the same stroke.
#[derive(Default)]
struct SwingWatch {
    /// The check's name, the shot's and what is struck.
    name: &'static str,
    shot_name: &'static str,
    quarry: &'static str,
    /// 0 before its press, 1 watching, 2 judged.
    stage: u8,
    /// Each new weighing of its stroke: seconds in, the seconds in at which it connects and
    /// how far out the hand is.
    frames: Vec<(f32, f32, f32)>,
    /// The server's seconds at its press, and the blows the server had answered then.
    pressed: (f32, u32),
    /// Its stroke has run its course, back to rest.
    ended: bool,
    /// The shot taken as it connects, and the models the hand was drawn with then.
    shot: bool,
    drawn: usize,
}

impl SwingWatch {
    fn of(name: &'static str, shot_name: &'static str, quarry: &'static str) -> Self {
        SwingWatch { name, shot_name, quarry, ..SwingWatch::default() }
    }
}

/// The gate's frame for the watched swing (game seconds).
const SWING_FRAME: f32 = 1.0 / 60.0;
/// Server seconds the watched swing has to play out and be answered.
const SWING_WAIT: f32 = 4.0;

/// Watches the weapon step's first swing: pressed once the bear is in reach and aimed at,
/// then followed each frame until its stroke is back at rest and the server has answered
/// its blow, held still for a shot as it connects. Whether it took this frame's input.
fn watch_swing(world: &mut World, gate: &mut Gate, far: bool, now: f32) -> bool {
    match gate.swing_watch.stage {
        0 => {
            let aimed = world.resource::<crate::interact::Target>().creature.is_some();
            if far || !aimed || gate.swung || now - gate.swing_at < voxmar_engine::combat::FULL_CHARGE + 0.05 {
                return false;
            }
            world.resource_mut::<crate::held::HeldClock>().step = Some(SWING_FRAME);
            gate.swing_watch.stage = 1;
            gate.swing_watch.pressed = (gate.server_now, world.resource::<crate::creatures::Creatures>().blows);
            gate.swing(world, now);
            true
        }
        1 => {
            // Let go the frame after the press, and press nothing more until it is judged.
            if gate.swung {
                gate.swing(world, now);
            }
            let seen = world.resource::<crate::held::Motion>().swing();
            let watch = &mut gate.swing_watch;
            match seen {
                Some(frame) if !watch.ended => {
                    if watch.frames.last().is_none_or(|last| last.0 != frame.0) {
                        watch.frames.push(frame);
                    }
                }
                None if !watch.frames.is_empty() => watch.ended = true,
                _ => {}
            }
            // Held where it connects (the hand's clock stopped) until the shot is taken.
            if !watch.shot && !watch.ended && watch.frames.last().is_some_and(|&(t, connect, _)| t >= connect) {
                world.resource_mut::<crate::held::HeldClock>().step = Some(0.0);
                let name = gate.swing_watch.shot_name;
                if gate.shot(world, name).is_none() {
                    return true;
                }
                gate.swing_watch.shot = true;
                gate.swing_watch.drawn = world.resource::<crate::held::Motion>().drawn();
                world.resource_mut::<crate::held::HeldClock>().step = Some(SWING_FRAME);
            }
            let answered = world.resource::<crate::creatures::Creatures>().blows > gate.swing_watch.pressed.1;
            if (gate.swing_watch.ended && answered) || gate.server_now - gate.swing_watch.pressed.0 > SWING_WAIT {
                judge_swing(world, gate, answered);
            }
            true
        }
        _ => false,
    }
}

/// Judges the watched swing: it peaked as its blow is judged (the class wind-up the server
/// times it by), went out all the way, came back to rest and was answered; the hand's clock
/// is the frame's own again.
fn judge_swing(world: &mut World, gate: &mut Gate, answered: bool) {
    world.resource_mut::<crate::held::HeldClock>().step = None;
    let judged = SwingWatch { stage: 2, ..SwingWatch::of(gate.swing_watch.name, gate.swing_watch.shot_name, gate.swing_watch.quarry) };
    let watch = std::mem::replace(&mut gate.swing_watch, judged);
    let quarry = watch.quarry;
    let (windup, held) = {
        let (launch, player) = (world.resource::<Launch>(), world.resource::<LocalPlayer>());
        let combat = &launch.content.combat;
        let held = player.inventory.get(player.selected).map(|s| s.item);
        let level = |skill| launch.content.skills.level(player.skills.xp(skill));
        (combat.timing(&combat.wielded(held, level)).windup, held)
    };
    // The hand is drawn as one model holding something, and as the empty hand's pieces
    // holding nothing.
    let models = match held {
        Some(_) => 1,
        None => world.resource::<crate::held::Handled>().0.as_ref().and_then(|h| h.bare.as_ref()).map_or(0, |bare| bare.pieces.len()),
    };
    let peak = watch.frames.iter().copied().max_by(|a, b| a.2.total_cmp(&b.2));
    let result = match peak {
        _ if watch.stage == 0 => Err(format!("{quarry} was never in reach and aimed at for the watched swing")),
        None => Err(format!("the swing at {quarry} never played in the hand")),
        Some((_, connect, _)) if (connect - windup).abs() > 1e-4 => Err(format!("the swing was timed to connect {connect:.3} s in, not at the wind-up of {windup:.3} s")),
        Some((at, _, _)) if (at - windup).abs() > SWING_FRAME + 1e-3 => Err(format!("the swing peaked {at:.3} s in, where the server judges its blow at {windup:.3} s")),
        Some((_, _, weight)) if weight < 0.9 => Err(format!("the swing went only {weight:.2} of the way out")),
        Some(_) if !watch.ended => Err(format!("the swing never came back to rest in {SWING_WAIT} server s ({} frames)", watch.frames.len())),
        Some(_) if !answered => Err(format!("the server answered no blow for the swing in {SWING_WAIT} s")),
        Some(_) if !watch.shot => Err("the swing was never shot as it connected".to_string()),
        Some(_) if watch.drawn != models || models == 0 => Err(format!("the hand was drawn with {} models as it connected, where it takes {models}", watch.drawn)),
        Some((at, _, weight)) => Ok(format!(
            "{} frames of 1/60 s: out {weight:.2} at {at:.3} s, the blow judged at {windup:.3} s; at rest {:.2} s in; its blow answered; drawn with {models} models",
            watch.frames.len(),
            watch.frames.last().map_or(0.0, |f| f.0)
        )),
    };
    gate.check(watch.name, result);
}

/// The bear a fight step is about, as shown: the one first seen since the step summoned it,
/// followed by its id, so another (one left behind) is never taken for it. Notes in
/// `gate.feel.died` when it is seen dying.
/// The fight-feel step's answer to a wind-up, pressed at the start of the frame, after the
/// server's messages are taken and before the game reads its input, so the press goes out
/// in this frame rather than the next (`FeelRun::answer`). A blow heard this frame for the
/// bear is answered by nothing; `feel` judges it in `Last`.
fn feel_press(world: &mut World) {
    let Some(mut gate) = world.remove_resource::<Gate>() else { return };
    if gate.step == Step::Feel && matches!(gate.phase, 2 | 4) {
        gate.feel.frame();
        let fighting = world.resource::<crate::fight::Fighting>();
        let new = (fighting.hurt_count - gate.feel.hurts) as usize;
        let received = gate.feel.bear.is_some_and(|bear| fighting.hurt.iter().rev().take(new).any(|h| h.0 == bear));
        let dodge_key = world.resource::<Settings>().dodge_key.0;
        gate.feel.answer(world, gate.phase == 2, dodge_key, received);
    } else {
        // Frames outside the tries (the counter, the sweep) are not theirs.
        gate.feel.frame_at = None;
    }
    world.insert_resource(gate);
}

fn the_bear(world: &World, gate: &mut Gate, species: voxmar_engine::creature::CreatureId) -> Option<voxmar_protocol::CreatureView> {
    let creatures = world.resource::<crate::creatures::Creatures>();
    if let Some(id) = gate.feel.bear {
        gate.feel.died |= creatures.sighting(id).is_some_and(|s| s.health == 0);
        return creatures.views().find(|v| v.id == id).copied();
    }
    let view = creatures.views().find(|v| v.species == species).copied();
    gate.feel.bear = view.map(|v| v.id);
    view
}

/// Clearing the bears the fight steps leave ([`sweep_bears`]).
#[derive(Default)]
struct Sweep {
    /// The server's clock at the last `/creatures kill`, how many there have been, whether
    /// the pad was swept, and since when no bear has been in view.
    killed: f32,
    kills: u32,
    pad: bool,
    quiet: Option<f32>,
}

/// Server seconds without a bear in view, after the pad is swept, that end a sweep; and
/// the most kills it tries.
const SWEEP_QUIET: f32 = 2.0;
const SWEEP_KILLS: u32 = 12;

/// Clears away every bear, so none can reach the player in a step about something else: in
/// creative (no bear can hurt), at each bear in view and then at the pad where they are
/// summoned, `/creatures kill` takes every wild creature within 8 blocks, a server second
/// apart. Done once no bear has been in view for [`SWEEP_QUIET`] server seconds after the
/// pad; failed (and done) if bears outlast [`SWEEP_KILLS`] kills.
fn sweep_bears(world: &mut World, gate: &mut Gate, species: voxmar_engine::creature::CreatureId, pad: IVec3) -> bool {
    mouse(world, MouseButton::Left, false);
    mouse(world, MouseButton::Right, false);
    key(world, KeyCode::KeyW, false);
    if gate.server_now - gate.sweep.killed < 1.0 && gate.sweep.kills > 0 {
        return false;
    }
    let bear = world.resource::<crate::creatures::Creatures>().views().find(|v| v.species == species).map(|v| v.position);
    let at = match bear {
        Some(at) => at,
        None if !gate.sweep.pad => {
            gate.sweep.pad = true;
            pad.as_vec3() + Vec3::new(0.5, 0.0, 0.5)
        }
        None => {
            let quiet = *gate.sweep.quiet.get_or_insert(gate.server_now);
            return gate.server_now - quiet > SWEEP_QUIET;
        }
    };
    if gate.sweep.kills >= SWEEP_KILLS {
        gate.check("bears swept", Err(format!("a bear was still in view after {SWEEP_KILLS} kills")));
        return true;
    }
    let net = world.resource::<Net>();
    net.command("gamemode creative");
    net.command(&format!("tp {} {} {}", at.x, at.y, at.z));
    net.command("creatures kill");
    gate.sweep.killed = gate.server_now;
    gate.sweep.kills += 1;
    gate.sweep.quiet = None;
    false
}

/// Answers to a creature's blow the feel step tries before failing: each one moves the
/// answer's moment by `FEEL_STEP` seconds, earlier or later by what came of the last, so a
/// lagging server is met where its blows fall. Perfect windows are 0.12 to 0.15 s, so a
/// step of 0.05 cannot jump one.
const FEEL_TRIES: u32 = 10;
const FEEL_STEP: f32 = 0.05;
/// Frames before the moment tried that an answer is pressed (`FeelRun::answer`): half of one,
/// so the frame that sends it is the nearest to the moment, not the first past it. Never
/// more than half the window aimed at (`FeelRun::room`): a quarter-second frame led a try
/// at 0.55 s to be queued at 0.50 s, before the perfect window, where a press that waited
/// sent it inside.
const FEEL_AIM: f32 = 0.5;
/// Seconds past the moment tried that a frame may come late with the try still saying
/// something of the moment: a frame that late answered after the blow could have landed
/// (the wind-up is 0.7 s), and said nothing, as on a machine drawing a frame a second.
const FEEL_SLOW_FRAME: f32 = 0.25;
/// Server seconds the block, counter and dodge take at most together; past them the step
/// fails what it is on and goes on to kill the bears, so none is left behind.
const FEEL_LIMIT: f32 = 120.0;

/// Pings answered under lag before the first blow is provoked. Guards and dodges are judged
/// earlier by the player's round trip (`Server::lag_allowance`), smoothed by a quarter at
/// each ping: put behind from calm, it is a tenth from where it settles eight pings on
/// (0.75^8), about 0.02 s of a 0.2 s allowance, under the 0.05 s steps the tries take; the
/// ninth, as the first may have been sent before the lag. With the lag on only from the
/// block's phase, the block was found while the allowance still grew, and every dodge after
/// it, at 0.40 to 0.65 s, was judged too late. A lagging server pings every tick, so this
/// takes ticks, where at a ping a second it took 44 s of the game's clock on the Mac.
const FEEL_LAG_SETTLE: u32 = 9;
/// Bears the step summons at most, should provoking them and the counter kill one.
const FEEL_BEARS: u32 = 3;

/// Where the fight-feel step stands: when the bear's wind-up was first seen (by the wall's
/// clock, as the server times blows), how long after that the answer comes, and what has
/// been tried.
#[derive(Default)]
struct FeelRun {
    seen: Option<Instant>,
    after: f32,
    answered: bool,
    tries: u32,
    outcomes: Vec<voxmar_engine::combat::Defended>,
    hurts: u32,
    struck: u32,
    /// The server's clock at the last provoking blow, or the bear's last blow.
    poked: f32,
    clicked: bool,
    bears: u32,
    /// The server's clock when the step began, for `FEEL_LIMIT`.
    started: f32,
    /// The server's pings answered when the step began to run behind, for `FEEL_LAG_SETTLE`.
    lagged_from: Option<u32>,
    /// The moment the answer's search is centred on, and how far round it the dodge's
    /// search has gone (`after` is the moment tried next).
    centre: f32,
    /// Half the window the answer aims at (the perfect block's, then the dodge's), and the
    /// dodge's middle and half, in seconds before the blow, from the content.
    room: f32,
    dodge: (f32, f32),
    probe: u32,
    /// The moment after the wind-up showed that the perfect block was found at: the dodge's
    /// search starts there, as the same delays (the server's, the frames') hold.
    found: Option<f32>,
    /// The bear the step is about (`the_bear`), whether it was seen dying, and whether it
    /// has been seen out of its attack pose since the last blow (a wind-up counts from the
    /// pose coming on, not from a pose still shown from the blow before).
    bear: Option<u32>,
    died: bool,
    calm: bool,
    /// The first telegraph faces the target on the server; shots keep the same camera
    /// before provoking it and during its wind-up, once the drawn body has turned.
    facing_checked: bool,
    facing_camera: Option<(f32, f32)>,
    facing_frames: u8,
    /// How late the frame that answered came past the moment tried, the worst of the tries
    /// set aside for it, and how many were.
    late: f32,
    slowest: f32,
    /// When after the wind-up showed the answer went, the frame it was pressed in (for
    /// `Last` to tell of it).
    sent: Option<f32>,
    slow: u32,
    /// Whether this wind-up's moment came with no press, already told (`answer`).
    held: bool,
    /// Frames since this wind-up showed that could have pressed before its result came
    /// (`answer`), and the tries set aside as unanswerable: the result came before any
    /// frame at or after the moment tried (`feel`).
    chances: u32,
    unanswerable: u32,
    /// When the last frame began, by the wall's clock and by how long our threads had waited
    /// for a core (`slow_frames::waited_for_cores`); the frame before it, in wall seconds and
    /// seconds waited; and the longest so of this wind-up's frames, from the one it showed in
    /// (`starved`).
    frame_at: Option<(Instant, Option<std::time::Duration>)>,
    last_frame: (f32, f32),
    longest: (f32, f32),
}

impl FeelRun {
    /// Times the frame just ended (`feel_press`, at the start of the next): its wall seconds
    /// and the seconds our threads waited for a core in them, kept as this wind-up's longest
    /// once it has been seen.
    fn frame(&mut self) {
        let now = (Instant::now(), crate::slow_frames::waited_for_cores());
        let Some((then, waited_then)) = self.frame_at.replace(now) else { return };
        let wall = now.0.saturating_duration_since(then).as_secs_f32();
        let waited = waited_then.zip(now.1).map_or(0.0, |(then, now)| now.saturating_sub(then).as_secs_f32());
        self.last_frame = (wall, waited);
        if self.seen.is_some() && wall > self.longest.0 {
            self.longest = self.last_frame;
        }
    }

    /// Whether this wind-up's longest frame was slow only for waiting for a core: longer than
    /// `FEEL_SLOW_FRAME` by the wall's clock, and no longer once the seconds our threads
    /// waited for a core in it are taken off, on a machine `crowded` with more work than it
    /// has cores. Our own work in a frame (drawing, the world, the fight) is all that is
    /// left, and it is held to the same quarter second; waiting is summed over all our
    /// threads, which on a calm machine barely wait, so there it never excuses.
    fn starved(&self, crowded: bool) -> bool {
        let (wall, waited) = self.longest;
        crowded && wall > FEEL_SLOW_FRAME && wall - waited.min(wall) <= FEEL_SLOW_FRAME
    }

    /// Whether this wind-up's result came before the client could answer it, with `answered`
    /// whether a press went: none did, and its moment passed between two frames with the
    /// result heard in the later (`held`), or no frame came at all between the wind-up
    /// showing and the result. A frame before the moment that could have pressed says a
    /// result after it came before the moment: that try counts, and the search moves earlier.
    fn could_not_answer(&self, answered: bool) -> bool {
        !answered && (self.held || self.chances == 0)
    }

    /// Observe a pose edge only after a calm pose. Hurt can arrive before the server's
    /// next Idle replication; consuming it invalidates the still-shown attack pose.
    fn observe(&mut self, attacking: bool, received: bool) -> bool {
        if !attacking {
            self.calm = true;
        } else if received {
            self.calm = false;
        } else if self.calm && self.seen.is_none() && !self.answered {
            self.seen = Some(Instant::now());
            self.calm = false;
            self.chances = 0;
            // The pose came in during the frame just timed.
            self.longest = self.last_frame;
            return true;
        }
        false
    }

    /// Queue a real input only while this observed wind-up remains unanswered by the
    /// server. A result already received cannot be defended by a next-frame input edge.
    ///
    /// Pressed at the start of a frame, before the game reads its input (`feel_press`), so
    /// the frame that presses is the one that sends it: from `FEEL_AIM` frames before the
    /// moment tried, by the frame just drawn. With frames of a few milliseconds that is the
    /// moment itself. Pressed in `Last`, the press went a frame on, and a quarter-second
    /// frame then sent a dodge tried at 0.57 s after 0.63 s, past the dodge's window.
    fn answer(&mut self, world: &mut World, blocking: bool, dodge_key: KeyCode, received: bool) -> bool {
        let Some(seen) = self.seen else { return false };
        let frame = world.get_resource::<Time>().map_or(0.0, |t| t.delta_secs());
        let due = seen.elapsed().as_secs_f32() + (FEEL_AIM * frame).min(self.room) >= self.after;
        if due && (received || self.answered) {
            // Told once a wind-up, for a try that ends with no response queued.
            if !self.held && !self.answered {
                info!("gate: combat response due {:.3}s after wind-up and not pressed: its result came first, frame {frame:.3}s", seen.elapsed().as_secs_f32());
            }
            self.held = true;
        }
        if received || self.answered || !due {
            // A frame before the moment, with the blow still to come: a press could have
            // gone, so a result after it says the moment was late.
            if !received && !self.answered {
                self.chances += 1;
            }
            return false;
        }
        self.answered = true;
        self.sent = Some(seen.elapsed().as_secs_f32());
        if blocking {
            mouse(world, MouseButton::Right, true);
        } else {
            // Forward, through the blow, rather than stepping outside its reach.
            key(world, KeyCode::KeyW, true);
            key(world, dodge_key, true);
        }
        true
    }
}

/// The bear in survival: provoked with a click every few server seconds until it winds up,
/// then the guard raised (phase 2) or a dodge (phase 4) a moment after the wind-up shows.
/// A perfect block must take nothing, and the click after it (phase 3) be a counter. The
/// dodge dashes forward, past the bear's lunge, so the blow is judged in reach: it must pass
/// through the dodge, not fall short. Only that bear's blows are judged, and only a wind-up
/// seen to begin. A failed block or counter goes on to the dodge all the same. Whatever
/// came of it, the bears are swept away (phase 5) and the player put back on the test
/// ground.
fn feel(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use voxmar_engine::combat::{Blow, Defended};
    use voxmar_engine::creature::brain::Pose;
    let (species, windup, perfect, dodge) = {
        let content = &world.resource::<Launch>().content;
        let Some(species) = content.creatures.find(BEAR) else {
            gate.check("perfect block", Err(format!("no `{BEAR}` in the content")));
            end_feel(world, gate, now);
            return;
        };
        let windup = content.creatures.get(species).def.attack.and_then(|a| a.windup).unwrap_or(content.combat.feel.creature_windup);
        // The windows the answers aim at, as the server judges them (`Guarding::meets`): a
        // raise up to `perfect` before the blow is a perfect block, and a dodge is safe from
        // `safe.0` to `safe.1` after it begins. The step wears no armour.
        let combat = &content.combat;
        let sword = combat.wielded(content.blocks.id(WEAPON), |_| WEAPON_LEVEL);
        let Some(perfect) = combat.guard(&sword).map(|g| g.perfect) else {
            gate.check("perfect block", Err(format!("`{WEAPON}` has no guard")));
            end_feel(world, gate, now);
            return;
        };
        let safe = combat.dodge_of(&[]).safe;
        (species, windup, perfect, ((safe.0 + safe.1) / 2.0, (safe.1 - safe.0) / 2.0))
    };
    let view = the_bear(world, gate, species);
    let dodge_key = world.resource::<Settings>().dodge_key.0;
    let pad = gate.feet + IVec3::new(0, 0, 14);
    let summon = |world: &mut World| world.resource::<Net>().command(&format!("summon {} 1 {} {} {}", BEAR.trim_start_matches("base:"), pad.x, pad.y, pad.z + 1));
    if gate.feel.clicked {
        mouse(world, MouseButton::Left, false);
        gate.feel.clicked = false;
    }
    if (1..=4).contains(&gate.phase) && gate.server_now - gate.feel.started > FEEL_LIMIT {
        // In a block again for a late counter (phase 2 with the perfect block found), what is
        // still owed is the counter: the perfect block passed at its first.
        let check = match gate.phase {
            3 => "counter",
            2 if gate.feel.found.is_some() => "counter",
            4 => "dodge",
            _ => "perfect block",
        };
        gate.check(check, Err(format!("not done in {FEEL_LIMIT} server seconds: {:?}, {} set aside as unanswerable", gate.feel.outcomes, gate.feel.unanswerable)));
        clear_feel(gate, now);
        return;
    }
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("gamemode survival");
            net.command(&format!("skill attack {WEAPON_LEVEL}"));
            // Room for the blows that come while the answer's moment is found, filled.
            net.command("skill hitpoints 60");
            net.command("heal");
            if item_count(world, WEAPON) == 0 {
                world.resource::<Net>().command(&format!("give {} 1", WEAPON.trim_start_matches("base:")));
            }
            summon(world);
            let fighting = world.resource::<crate::fight::Fighting>();
            // The search starts in the middle of the perfect window (0.625 s into a 0.7 s
            // wind-up). From its earliest moment (0.55 s), a press a few milliseconds early
            // was raised too soon, and on calm runs the block took two tries. It moves either
            // way from there.
            let middle = (windup - perfect / 2.0).max(0.0);
            gate.feel = FeelRun { after: middle, centre: middle, room: perfect / 2.0, dodge, hurts: fighting.hurt_count, struck: fighting.struck_count, poked: f32::MIN, bears: 1, started: gate.server_now, ..default() };
            gate.advance(now);
        }
        1 => {
            let roomy = world.resource::<LocalPlayer>().max_health >= 100;
            if gate.feel.lagged_from.is_none() {
                gate.feel.lagged_from = Some(world.resource::<crate::fight::Fighting>().pings);
            }
            if roomy && view.is_some() && select(world, WEAPON) {
                gate.advance(now);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("perfect block", Err(format!("the sword, Hitpoints 60 or the bear never came (bear seen: {}, most health {roomy})", view.is_some())));
                clear_feel(gate, now);
            }
        }
        2 | 4 => {
            let blocking = gate.phase == 2;
            let Some(view) = view else {
                // Provoking it and the counter can kill one: another takes its place. One
                // only out of sight is waited for (`FEEL_LIMIT` bounds the wait).
                if gate.feel.died && gate.feel.bears < FEEL_BEARS {
                    gate.feel.bears += 1;
                    gate.feel.seen = None;
                    gate.feel.answered = false;
                    gate.feel.bear = None;
                    gate.feel.died = false;
                    gate.feel.calm = false;
                    gate.feel.hurts = world.resource::<crate::fight::Fighting>().hurt_count;
                    gate.phase_server = gate.server_now;
                    summon(world);
                } else if gate.feel.died {
                    gate.check(if blocking { "perfect block" } else { "dodge" }, Err(format!("{FEEL_BEARS} bears died before it came")));
                    clear_feel(gate, now);
                }
                return;
            };
            // Consume this bear's newly received result before considering an answer.
            // Last can see both a crossed response threshold and Hurt in the same frame;
            // queuing a press then release would dodge only after the judged blow.
            let fighting = world.resource::<crate::fight::Fighting>();
            let new = (fighting.hurt_count - gate.feel.hurts) as usize;
            gate.feel.hurts = fighting.hurt_count;
            let ours = fighting.hurt.iter().rev().take(new).find(|h| h.0 == view.id).copied();
            // Healed after each of its blows: every try, judged or set aside, starts from
            // full health, so however many a slow machine sets aside the player never dies
            // of them. What is judged is how a blow was met, never the health left.
            if ours.is_some() {
                world.resource::<Net>().command("heal");
            }
            let distance = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz());
            let fixed_camera = gate.feel.facing_camera.filter(|_| !gate.shots.lock().expect("gate shots").contains_key("windup-facing"));
            key(world, KeyCode::KeyW, fixed_camera.is_none() && distance > 2.0 && gate.feel.seen.is_none());
            if let Some((yaw, pitch)) = fixed_camera {
                let mut player = world.resource_mut::<LocalPlayer>();
                player.yaw = yaw;
                player.pitch = pitch;
                if gate.feel.facing_frames < 3 {
                    gate.feel.facing_frames += 1;
                }
            } else {
                aim_at(world, view.position + Vec3::Y * 0.6);
            }
            // A wind-up is timed from its pose coming on: one still shown from the blow just
            // judged (its end not yet heard) is not a new one.
            if gate.feel.observe(view.pose == Pose::Attacking, ours.is_some()) {
                // The pose came in some time during the frame just drawn: on average half of
                // it before this moment, which a slow frame makes worth counting.
                let half = std::time::Duration::from_secs_f32(world.resource::<Time>().delta_secs() / 2.0);
                gate.feel.seen = gate.feel.seen.map(|seen| seen.checked_sub(half).unwrap_or(seen));
                gate.feel.held = false;
                info!("gate: combat wind-up seen phase {} try {} bear {}, frame {:.3}s, server {:.2}s", gate.phase, gate.feel.tries + 1, view.id, world.resource::<Time>().delta_secs(), gate.server_now);
                if !gate.feel.facing_checked {
                    let target = gate.observer.as_ref().and_then(|o| o.others.values().next().copied()).unwrap_or(world.resource::<LocalPlayer>().position);
                    let to = (target - view.position).with_y(0.0).normalize_or_zero();
                    let forward = Vec3::new(-view.yaw.sin(), 0.0, -view.yaw.cos());
                    let cosine = forward.dot(to);
                    let result = if cosine > 0.8 { Ok(format!("the first attacking pose faces its target (cosine {cosine:.3})")) } else { Err(format!("the attacking pose faced away from its target (cosine {cosine:.3}, yaw {})", view.yaw)) };
                    gate.check("wind-up facing", result);
                    gate.feel.facing_checked = true;
                }
            }
            // The first update checks the server heading; give the drawn body's smooth
            // turn time to show it, while it is still winding up. Keep the camera held
            // until this later image has arrived.
            if gate.feel.facing_checked && fixed_camera.is_some() && view.pose == Pose::Attacking
                && gate.feel.seen.is_some_and(|seen| seen.elapsed().as_secs_f32() >= windup * 0.35)
            {
                gate.shot(world, "windup-facing");
            }
            // Pressed at this frame's start (`feel_press`), and sent by it.
            if let Some(sent) = gate.feel.sent.take() {
                gate.feel.late = sent - gate.feel.after;
                let player = world.resource::<LocalPlayer>();
                info!("gate: combat response phase {} try {} bear {}, requested {:.3}s, sent {sent:.3}s after wind-up, grounded {}, flying {}, frame {:.3}s", gate.phase, gate.feel.tries + 1, view.id, gate.feel.after, player.on_ground, player.flying, world.resource::<Time>().delta_secs());
            } else if !blocking {
                key(world, dodge_key, false);
            }
            // Provoked at most every few server seconds while it does not fight back, and only
            // once the lag has held long enough for the server's round trip to settle on it.
            let pings = world.resource::<crate::fight::Fighting>().pings;
            let settled = gate.feel.lagged_from.is_some_and(|from| pings - from >= FEEL_LAG_SETTLE);
            if gate.feel.seen.is_none() && gate.server_now - gate.feel.poked > 3.0 && distance < 2.4 && settled {
                if !gate.feel.facing_checked {
                    if gate.feel.facing_camera.is_none() {
                        let player = world.resource::<LocalPlayer>();
                        gate.feel.facing_camera = Some((player.yaw, player.pitch));
                        key(world, KeyCode::KeyW, false);
                    }
                    // The gate runs in Last: the renderer needs later frames to see
                    // this camera direction before the first image is requested.
                    if gate.feel.facing_frames < 3 {
                        return;
                    }
                    if gate.shot(world, "windup-before").is_none() {
                        return;
                    }
                }
                gate.feel.poked = gate.server_now;
                mouse(world, MouseButton::Left, true);
                gate.feel.clicked = true;
            }
            // Only this bear's blow, from a wind-up seen to begin, answers the try; any
            // other (another creature's, or one whose wind-up began unseen) is passed over.
            let Some((_, damage, defended, _)) = ours.filter(|_| gate.feel.seen.is_some()) else {
                return;
            };
            gate.feel.poked = gate.server_now;
            mouse(world, MouseButton::Right, false);
            key(world, dodge_key, false);
            key(world, KeyCode::KeyW, false);
            let answered = std::mem::take(&mut gate.feel.answered);
            info!("gate: combat result phase {} try {} bear {}, {:?} damage {}, response queued {}, grounded {}", gate.phase, gate.feel.tries + 1, view.id, defended, damage, answered, world.resource::<LocalPlayer>().on_ground);
            gate.feel.seen = None;
            gate.feel.outcomes.push(defended);
            let passed = if blocking { defended == Defended::Perfect } else { defended == Defended::Dodged };
            // An answer sent by a frame that came this late may have gone after the blow
            // landed, whatever moment was tried: the try says nothing of the moment, so it is
            // set aside, not counted, and the search stays. Enough of them and the machine is
            // too slow to judge (as `creature falls` is when it draws too few frames).
            // A result before any frame at or after the moment tried, with none before it
            // that could have pressed (the wind-up first seen after it landed, or the moment
            // passed between two frames, as frames of a second or more do on a loaded
            // machine): no press could have answered it, so the try says nothing of the
            // moment. It is set aside and the search stays; unlike a late frame it never
            // excuses the step, which `FEEL_LIMIT` still bounds.
            // Such a try is excused as a late frame's is when its longest frame was slow only
            // for waiting on a core (`FeelRun::starved`): the machine's work, not ours.
            if !passed && gate.feel.could_not_answer(answered) {
                gate.feel.outcomes.pop();
                let why = if gate.feel.held { "the moment passed between two frames" } else { "no frame came between the wind-up showing and the blow landing" };
                let (wall, waited) = gate.feel.longest;
                let load = crate::slow_frames::load_average();
                let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
                if gate.feel.starved(load.is_some_and(|load| load > cores as f64)) {
                    info!("gate: combat try set aside phase {} try {} bear {}: {defended:?} came before it could be answered ({why}), on a {wall:.3}s frame of which our threads waited {waited:.3}s for a core, load {:.0} over {cores} cores", gate.phase, gate.feel.tries + 1, view.id, load.unwrap_or(0.0));
                    slow_try(gate, wall, blocking, windup, perfect, now);
                    return;
                }
                gate.feel.unanswerable += 1;
                info!("gate: combat try set aside phase {} try {} bear {}: {defended:?} came before it could be answered ({why}), frame {:.3}s, longest {wall:.3}s with {waited:.3}s waiting for a core, {} set aside", gate.phase, gate.feel.tries + 1, view.id, world.resource::<Time>().delta_secs(), gate.feel.unanswerable);
                return;
            }
            let late = std::mem::take(&mut gate.feel.late);
            if !passed && answered && late > FEEL_SLOW_FRAME {
                gate.feel.outcomes.pop();
                slow_try(gate, late, blocking, windup, perfect, now);
                return;
            }
            if passed {
                let detail = format!("{defended:?} for {damage} damage, requested {:.2}s after the wind-up showed, try {}", gate.feel.after, gate.feel.tries + 1);
                // A block again, for a counter that came too late (phase 3): the perfect
                // block was judged at its first, so this one only opens the counter's window.
                if blocking && damage == 0 && gate.feel.found.is_some() {
                    info!("gate: combat block again for the counter: {detail}");
                } else {
                    let result = if damage == 0 { Ok(detail) } else { Err(format!("{detail}: it still did damage")) };
                    gate.check(if blocking { "perfect block" } else { "dodge" }, result);
                }
                gate.feel.struck = world.resource::<crate::fight::Fighting>().struck_count;
                if blocking {
                    gate.feel.found = Some(gate.feel.after);
                    gate.advance(now);
                } else {
                    clear_feel(gate, now);
                }
                return;
            }
            gate.feel.tries += 1;
            // Guarded but not perfect: raised too soon. Landed (hit or turned by the roll)
            // before any answer, or after a guard: too late, so the moment (the centre of
            // the search) moves earlier. After a dodge it was too soon or too late: the next
            // tries either side of the centre in turn, a step further each pair, the earlier
            // side first (-1, +1, -2, +2 steps), as a busy machine only makes answers late.
            // Out of reach says nothing of the moment: the same again, closer.
            let landed = matches!(defended, Defended::Hit | Defended::Missed);
            match defended {
                Defended::Blocked => gate.feel.centre += FEEL_STEP,
                _ if landed && (!answered || blocking) => {
                    gate.feel.centre -= FEEL_STEP;
                    gate.feel.probe = 0;
                }
                _ if landed => gate.feel.probe += 1,
                _ => {}
            }
            let k = gate.feel.probe;
            let side = if k % 2 == 1 { -1.0 } else { 1.0 };
            gate.feel.centre = gate.feel.centre.clamp(0.0, windup);
            gate.feel.after = (gate.feel.centre + side * FEEL_STEP * k.div_ceil(2) as f32).clamp(0.0, windup);
            if gate.feel.tries >= FEEL_TRIES {
                let outcomes = std::mem::take(&mut gate.feel.outcomes);
                // After a perfect block whose counter came too late, what is still owed is
                // the counter: the perfect block passed at its first.
                if blocking && gate.feel.found.is_some() {
                    gate.check("counter", Err(format!("the click after the perfect block came too late, and the blocks after it came to {outcomes:?}, none perfect to counter from")));
                } else {
                    gate.check(if blocking { "perfect block" } else { "dodge" }, Err(format!("{FEEL_TRIES} answers to the bear's blows came to {outcomes:?}, {} more set aside as unanswerable", gate.feel.unanswerable)));
                }
                if blocking {
                    to_dodge(gate, windup, perfect, now);
                } else {
                    clear_feel(gate, now);
                }
            }
        }
        3 => {
            // The guard let down last frame; the click now must be a counter.
            mouse(world, MouseButton::Right, false);
            let Some(view) = view else {
                gate.check("counter", Err("the bear went before the counter".into()));
                to_dodge(gate, windup, perfect, now);
                return;
            };
            aim_at(world, view.position + Vec3::Y * 0.6);
            if gate.feel.poked != f32::MAX && in_phase > 0.0 {
                mouse(world, MouseButton::Left, true);
                gate.feel.clicked = true;
                gate.feel.poked = f32::MAX;
            }
            let fighting = world.resource::<crate::fight::Fighting>();
            if fighting.struck_count > gate.feel.struck {
                let (_, damage, blow, _, _) = *fighting.struck.last().expect("counted");
                gate.feel.poked = gate.server_now;
                if blow == Blow::Counter && damage > 0 {
                    gate.check("counter", Ok(format!("the click after the perfect block was a counter for {damage}")));
                    to_dodge(gate, windup, perfect, now);
                } else if gate.feel.tries + 1 >= FEEL_TRIES {
                    gate.check("counter", Err(format!("the click after the perfect block was {blow:?} for {damage}")));
                    to_dodge(gate, windup, perfect, now);
                } else {
                    // Too late for the window (a server far behind): block again.
                    gate.feel.tries += 1;
                    gate.feel.outcomes.clear();
                    to_phase(gate, FEEL_BLOCK, now);
                }
            } else if gate.server_in_phase() > UNANSWERED {
                gate.check("counter", Err(format!("the server answered no blow for {UNANSWERED}s")));
                to_dodge(gate, windup, perfect, now);
            }
        }
        5 => {
            if sweep_bears(world, gate, species, pad) {
                end_feel(world, gate, now);
            }
        }
        _ => end_feel(world, gate, now),
    }
}

/// A try set aside for a frame too slow to say anything of the moment, `late` seconds late
/// (or that long, for one no frame could answer): enough of them and the check is KNOWN,
/// the machine too slow to judge it.
fn slow_try(gate: &mut Gate, late: f32, blocking: bool, windup: f32, perfect: f32, now: f32) {
    gate.feel.slow += 1;
    gate.feel.slowest = gate.feel.slowest.max(late);
    if gate.feel.slow >= FEEL_TRIES {
        let detail = format!("{} answers came on frames up to {:.2} s late or long, past a {windup} s wind-up, so none says whether the game judges the moment", gate.feel.slow, gate.feel.slowest);
        // A block already found leaves only the counter owed, so that is what goes unjudged.
        let name = if blocking && gate.feel.found.is_some() { "counter" } else if blocking { "perfect block" } else { "dodge" };
        gate.known(name, Err(detail), "frames slower than a wind-up, as on a loaded machine");
        gate.feel.slow = 0;
        gate.feel.slowest = 0.0;
        if blocking {
            to_dodge(gate, windup, perfect, now);
        } else {
            clear_feel(gate, now);
        }
    }
}

/// After the block and counter, passed or not: on to the dodge (phase 4) with its own tries.
fn to_dodge(gate: &mut Gate, windup: f32, perfect: f32, now: f32) {
    gate.feel.tries = 0;
    gate.feel.slow = 0;
    gate.feel.unanswerable = 0;
    gate.feel.slowest = 0.0;
    gate.feel.outcomes.clear();
    gate.feel.seen = None;
    gate.feel.answered = false;
    // The dodge's search starts in the middle of its window (0.57 s into a 0.7 s wind-up,
    // for a dodge safe from 0.04 to 0.22 s in), moved as far as the perfect block was found
    // from its own window's middle: the same delays (the server's, the frames') hold for
    // both. (Started 0.05 s before the block's moment, it was hit once the smoothed round
    // trip had grown with the lag since the block's try, as the server then judged the
    // same request earlier.)
    let (middle, half) = gate.feel.dodge;
    let moved = gate.feel.found.map_or(0.0, |found| found - (windup - perfect / 2.0));
    gate.feel.after = (windup - middle + moved).clamp(0.0, windup);
    gate.feel.room = half;
    gate.feel.centre = gate.feel.after;
    gate.feel.probe = 0;
    to_phase(gate, FEEL_DODGE, now);
}

/// The feel step's phases that are jumped to, and the one the bear comes in (from which the
/// server runs behind).
const FEEL_COMING: u32 = 1;
const FEEL_BLOCK: u32 = 2;
const FEEL_DODGE: u32 = 4;
const FEEL_KILL: u32 = 5;

/// Straight to `phase` of the step.
fn to_phase(gate: &mut Gate, phase: u32, now: f32) {
    gate.phase = phase;
    gate.phase_since = now;
    gate.phase_server = gate.server_now;
}

/// Done, or a failed check: on to sweeping the bears away (phase 5), so none is left
/// provoked for the steps after.
fn clear_feel(gate: &mut Gate, now: f32) {
    gate.sweep = Sweep::default();
    to_phase(gate, FEEL_KILL, now);
}

/// Back to survival at the first levels with the hands free, on the test ground, for the
/// steps after.
fn end_feel(world: &mut World, gate: &mut Gate, now: f32) {
    mouse(world, MouseButton::Left, false);
    mouse(world, MouseButton::Right, false);
    key(world, KeyCode::KeyW, false);
    key(world, world.resource::<Settings>().dodge_key.0, false);
    let feet = gate.feet;
    let net = world.resource::<Net>();
    net.command("gamemode survival");
    net.command("skill attack 1");
    net.command("skill hitpoints 10");
    net.command(&format!("tp {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5));
    gate.next(Step::Boss, now);
}

/// The boss the boss step summons.
const BOSS: &str = "base:rootmother";
/// Server seconds each of the boss step's waits may take.
const BOSS_WAIT: f32 = 10.0;
/// The boss step's sweep, which every exit comes to.
const BOSS_SWEEP: u32 = 4;

/// The Rootmother on the pad: drawn, her music, a move's tell; then swept away.
fn boss(world: &mut World, gate: &mut Gate, now: f32) {
    let Some(species) = world.resource::<Launch>().content.creatures.find(BOSS) else {
        gate.check("boss", Err(format!("no `{BOSS}` in the content")));
        end_boss(world, gate, now);
        return;
    };
    let pad = gate.feet + IVec3::new(0, 0, 14);
    let view = {
        let creatures = world.resource::<crate::creatures::Creatures>();
        match gate.boss_id {
            Some(id) => creatures.views().find(|v| v.id == id).copied(),
            None => creatures.views().find(|v| v.species == species && !gate.boss_before.contains(&v.id)).copied(),
        }
    };
    let waited = gate.server_in_phase() > BOSS_WAIT;
    let sweep = |gate: &mut Gate, now: f32| {
        gate.sweep = Sweep::default();
        to_phase(gate, BOSS_SWEEP, now);
    };
    match gate.phase {
        0 => {
            // In creative, so she does not fight before the music is judged.
            gate.boss_before = world.resource::<crate::creatures::Creatures>().views().filter(|v| v.species == species).map(|v| v.id).collect();
            gate.boss_id = None;
            let net = world.resource::<Net>();
            net.command("gamemode creative");
            net.command(&format!("summon {} 1 {} {} {}", BOSS.trim_start_matches("base:"), pad.x, pad.y, pad.z + 1));
            gate.advance(now);
        }
        1 => {
            if let Some(view) = view {
                gate.boss_id = Some(view.id);
                let others = gate.boss_before.len();
                gate.check("boss drawn", Ok(format!("the Rootmother reached the client at {:.1} (creature {}; {others} other Rootmothers seen before)", view.position, view.id)));
                gate.advance(now);
            } else if waited {
                gate.check("boss drawn", Err(format!("a summoned {BOSS} never reached the client in {BOSS_WAIT} server seconds")));
                sweep(gate, now);
            }
        }
        2 => {
            // A boss within the rules' reach is a boss fight: the calm track fades and one
            // of this boss's own starts (her `music` tags, wherever she is met; silently,
            // `music::Rehearse`), never another biome's boss's.
            match crate::music::boss_music(world) {
                Some((true, Some((track, ours)))) => {
                    let result = if ours { Ok(format!("a boss fight, with her own track: {track}")) } else { Err(format!("a boss fight, but with another boss's track: {track}")) };
                    gate.check("boss music", result);
                    gate.advance(now);
                }
                music if waited => {
                    let said = music.map_or("no music running".to_owned(), |(fighting, _)| format!("in a boss fight: {fighting}, no boss's track"));
                    gate.check("boss music", Err(format!("not a boss fight with her track after {BOSS_WAIT} server seconds ({said})")));
                    gate.advance(now);
                }
                _ => {}
            }
        }
        3 => {
            if gate.settled == 0 {
                // Close enough for her lash, with room for a blow should one land.
                let net = world.resource::<Net>();
                net.command("skill hitpoints 60");
                net.command(&format!("tp {} {} {}", pad.x as f32 + 0.5, pad.y, pad.z as f32 - 1.0));
                net.command("gamemode survival");
                gate.settled = 1;
            }
            // What she was doing as the wait began, and when it gave up.
            let doing = |world: &World| {
                let me = world.resource::<LocalPlayer>().position;
                let (client, blocks) = (world.resource::<ClientWorld>(), &world.resource::<Launch>().content.blocks);
                let name = |cell: IVec3| client.block(cell).map_or_else(|| "unloaded".to_owned(), |b| blocks.def(b).name.clone());
                view.map_or("not seen".to_owned(), |v| {
                    let flat = (v.position - me).with_y(0.0).length();
                    let feet = v.position.floor().as_ivec3();
                    format!(
                        "creature {} at {:.1}, {flat:.2} blocks from the player across the ground, pose {:?}, tell {}, health {}, scale {:.2}, moving {:.1}, in {} over {}",
                        v.id, v.position, v.pose, v.tell, v.health, v.scale, v.velocity.with_y(0.0).length(), name(feet), name(feet - IVec3::Y)
                    )
                })
            };
            if gate.settled == 1 && gate.server_in_phase() > 1.0 {
                info!("gate: boss tell wait began: she is {}", doing(world));
                // Any other Rootmother about, for the record.
                let me = world.resource::<LocalPlayer>().position;
                for other in world.resource::<crate::creatures::Creatures>().views().filter(|v| v.species == species && Some(v.id) != gate.boss_id) {
                    info!("gate: another Rootmother {} at {:.1}, {:.1} blocks off, pose {:?}", other.id, other.position, other.position.distance(me), other.pose);
                }
                gate.settled = 2;
            }
            let told = view.filter(|v| v.tell != 0 && v.tell != voxmar_protocol::REARING);
            if let Some(view) = told {
                let content = &world.resource::<Launch>().content;
                let name = content.creatures.get(species).def.moves.get(view.tell as usize - 1).map_or("?", |m| m.name.as_str());
                gate.check("boss tell", Ok(format!("she wound up her {name}, its tell shown")));
                world.resource::<Net>().command("gamemode creative");
                sweep(gate, now);
            } else if waited || view.is_none() {
                gate.check("boss tell", Err(format!("no move of hers wound up in {BOSS_WAIT} server seconds: she is {}", doing(world))));
                sweep(gate, now);
            }
        }
        _ => {
            if sweep_bears(world, gate, species, pad) {
                end_boss(world, gate, now);
            }
        }
    }
}

/// Back to survival at the first levels on the test ground, for the steps after.
fn end_boss(world: &mut World, gate: &mut Gate, now: f32) {
    let feet = gate.feet;
    let net = world.resource::<Net>();
    net.command("gamemode survival");
    net.command("skill hitpoints 10");
    net.command(&format!("tp {} {} {}", feet.x as f32 + 0.5, feet.y, feet.z as f32 + 0.5));
    gate.next(Step::Smoke, now);
}

/// What the smoke step holds, and the effect it leaves.
const SMOKE_ITEM: &str = "base:torch";
const SMOKY: &str = "base:smoky";

/// A torch held a moment: the Smoky chip shows, with its time; then the hand is emptied
/// (the torch taken back when the hotbar has no free slot).
fn smoke(world: &mut World, gate: &mut Gate, now: f32) {
    let Some(smoky) = world.resource::<Launch>().content.effects.find(SMOKY) else {
        gate.check("smoky chip", Err(format!("no `{SMOKY}` in the content")));
        gate.next(Step::Wear, now);
        return;
    };
    let waited = gate.server_in_phase() > BOSS_WAIT;
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("effect clear");
            net.command(&format!("give {} 1", SMOKE_ITEM.trim_start_matches("base:")));
            gate.advance(now);
        }
        1 => {
            if select(world, SMOKE_ITEM) {
                gate.advance(now);
            } else if waited {
                gate.check("smoky chip", Err(format!("no {SMOKE_ITEM} came to hand in {BOSS_WAIT} server seconds")));
                gate.next(Step::Wear, now);
            }
        }
        2 => {
            // The chip is drawn for the effect, with a timer: smoke that clings is timed.
            let chip = world.query::<&crate::hud::effects::EffectHover>().iter(world).any(|hover| hover.0 == smoky);
            let timed = world.query::<&crate::hud::EffectTimer>().iter(world).any(|timer| timer.0 == smoky);
            if chip && timed {
                let left = world.resource::<LocalPlayer>().vitals.effects.iter().find(|a| a.effect == smoky).and_then(|a| a.seconds_left);
                gate.check("smoky chip", Ok(format!("a torch in hand: the Smoky chip shows, {} s left", left.unwrap_or(0))));
                gate.advance(now);
            } else if waited {
                gate.check("smoky chip", Err(format!("with a torch in hand for {BOSS_WAIT} server seconds: chip {chip}, timed {timed}")));
                gate.advance(now);
            }
        }
        // The hand emptied: onto a free hotbar slot, or, with none free, the torch taken
        // back, which frees its own (waiting out the hotbar cost the step ten seconds).
        3 => {
            if empty_hand(world) {
                world.resource::<Net>().command("effect clear");
                gate.next(Step::Wear, now);
            } else {
                world.resource::<Net>().command(&format!("clear {}", SMOKE_ITEM.trim_start_matches("base:")));
                gate.advance(now);
            }
        }
        _ => {
            if empty_hand(world) || waited {
                world.resource::<Net>().command("effect clear");
                gate.next(Step::Wear, now);
            }
        }
    }
}

/// What the wear step puts on, and where (`inventory::WORN` on).
const WORN_SHIRT: (&str, usize) = ("base:chainmail_shirt", 1);
const WORN_SHIELD: (&str, usize) = ("base:wooden_shield", 5);

/// Each piece selected and right-clicked at the sky goes on (shots `wear-shirt` and
/// `wear-shield` with each in hand, `wear-on` after): the server's word must put it in its
/// worn slot (a state it sent after the click, not what this side predicted). Then the
/// shield is what guards (a stop of 55%, not the sword's 35%); what a guard does to a blow
/// is judged by the server in the gameplay tests (`a_shield_guards_wider...`).
fn wear(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use voxmar_engine::inventory::WORN;
    let worn = |world: &World, (name, at): (&str, usize)| {
        let item = world.resource::<ClientWorld>().registry.id(name);
        world.resource::<LocalPlayer>().inventory.get(WORN + at).is_some_and(|s| Some(s.item) == item)
    };
    match gate.phase {
        0 => {
            let (player, launch) = (world.resource::<LocalPlayer>(), world.resource::<Launch>());
            let skills = &launch.content.skills;
            gate.worn_defence = skills.id("core:defence").map_or(1, |defence| skills.level(player.skills.xp(defence)));
            let net = world.resource::<Net>();
            net.command("skill defence 20");
            for (name, _) in [WORN_SHIRT, WORN_SHIELD] {
                net.command(&format!("give {} 1", name.trim_start_matches("base:")));
            }
            gate.advance(now);
        }
        1 | 3 => {
            // Selected, then one right click, once the server has given it.
            let piece = if gate.phase == 1 { WORN_SHIRT } else { WORN_SHIELD };
            if select(world, piece.0) {
                // Looking up (nothing in reach to take the click), it in hand: shot, once shown.
                look(world, 0.0, 80.0);
                gate.settled += 1;
                if gate.settled < 3 || gate.shot(world, if gate.phase == 1 { "wear-shirt" } else { "wear-shield" }).is_none() {
                    return;
                }
                mouse(world, MouseButton::Right, true);
                gate.worn_states = world.resource::<LocalPlayer>().states;
                gate.advance(now);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("wear", Err(format!("no {} came to the inventory", piece.0)));
                end_wear(world, gate, now);
            }
        }
        2 | 4 => {
            mouse(world, MouseButton::Right, false);
            let piece = if gate.phase == 2 { WORN_SHIRT } else { WORN_SHIELD };
            // A state the server sent since the click; one that left before the click came
            // shows the piece not on yet, and the wait goes on.
            let told = world.resource::<LocalPlayer>().states > gate.worn_states;
            if told && worn(world, piece) {
                if gate.phase == 4 {
                    // Both on, out of the hotbar.
                    gate.settled += 1;
                    if gate.settled < 3 || gate.shot(world, "wear-on").is_none() {
                        return;
                    }
                    let player = world.resource::<LocalPlayer>();
                    let launch = world.resource::<Launch>();
                    let held = world.resource::<ClientWorld>().registry.id(WEAPON);
                    let guard = launch.content.combat.guard_of(held, &player.inventory.worn, |skill| launch.content.skills.level(player.skills.xp(skill)));
                    let stop = guard.map_or(0.0, |g| g.stop);
                    gate.check("wear", if stop > 0.5 { Ok(format!("the server put on a chainmail shirt and a wooden shield; the guard stops {:.0}%", stop * 100.0)) } else { Err(format!("with the shield on, the guard stops {:.0}%", stop * 100.0)) });
                    // The inventory shows them worn.
                    world.resource_mut::<UiState>().inventory_open = true;
                    gate.advance(now);
                } else {
                    gate.advance(now);
                }
            } else if gate.server_in_phase() > 5.0 {
                gate.check("wear", Err(format!("a right click with {} in hand did not put it on (the server said {} states since)", piece.0, world.resource::<LocalPlayer>().states - gate.worn_states)));
                end_wear(world, gate, now);
            }
        }
        5 => {
            // The shirt's tooltip shown, in English and then Japanese, for the shots.
            world.insert_resource(crate::hud::inventory::HoverSlot(WORN + WORN_SHIRT.1));
            if in_phase < 0.5 {
                return;
            }
            if !gate.requested.contains(&"worn-en") {
                gate.check("wear: worn slots", worn_drawn(world));
            }
            if gate.shot(world, "worn-en").is_some() {
                set_language(world, "ja");
                gate.advance(now);
            }
        }
        6 => {
            if in_phase < 0.5 || gate.shot(world, "worn-ja").is_none() {
                return;
            }
            set_language(world, "en");
            world.remove_resource::<crate::hud::inventory::HoverSlot>();
            world.resource_mut::<UiState>().inventory_open = false;
            end_wear(world, gate, now);
        }
        _ => end_wear(world, gate, now),
    }
}

/// The inventory's six worn wells, the shirt and the shield drawn in theirs, and their
/// tooltips' armour lines: the set, its weight and Defence level, its armour, and the
/// shield's guard.
fn worn_drawn(world: &mut World) -> Result<String, String> {
    use crate::hud::SlotIcon;
    use voxmar_engine::inventory::{LAST, WORN};
    let icons: Vec<(usize, Handle<Image>, bool)> =
        world.query::<(&SlotIcon, &ImageNode, &InheritedVisibility)>().iter(world).filter(|(slot, ..)| (WORN..=LAST).contains(&slot.0)).map(|(slot, image, shown)| (slot.0, image.image.clone(), shown.get())).collect();
    let wells: std::collections::BTreeSet<usize> = icons.iter().map(|(slot, ..)| *slot).collect();
    if wells.len() != LAST - WORN + 1 {
        return Err(format!("{} worn wells drawn, not {}", wells.len(), LAST - WORN + 1));
    }
    let (registry, images) = (&world.resource::<ClientWorld>().registry, world.resource::<crate::hud::HudImages>());
    for (name, at) in [WORN_SHIRT, WORN_SHIELD] {
        let item = registry.id(name).ok_or(format!("no {name}"))?;
        let want = images.icons.get(item.0 as usize).cloned().unwrap_or_default();
        if !icons.iter().any(|(slot, image, shown)| *slot == WORN + at && *image == want && *shown) {
            return Err(format!("the worn well {at} does not show {name}"));
        }
    }
    let (locale, launch, player) = (world.resource::<Locale>(), world.resource::<Launch>(), world.resource::<LocalPlayer>());
    let lines = |name: &str| registry.id(name).map(|item| crate::hud::inventory::armour_lines(locale, &launch.content, player, item)).unwrap_or_default();
    let (shirt, shield) = (lines(WORN_SHIRT.0), lines(WORN_SHIELD.0));
    if shirt.len() < 2 || shield.len() < 3 {
        return Err(format!("the tooltips read {shirt:?} and {shield:?}"));
    }
    // What anyone can wear says nothing of the level it needs.
    let needs_one = locale.format("item.armour_needs", &[("level", &1)]);
    if shield.iter().any(|(line, _)| *line == needs_one) {
        return Err(format!("the shield's tooltip says \"{needs_one}\": {shield:?}"));
    }
    let words = |lines: &[(String, Color)]| lines.iter().map(|(line, _)| line.as_str()).collect::<Vec<_>>().join(" / ");
    Ok(format!("six worn wells, the shirt and shield shown in theirs; tooltips \"{}\" and \"{}\"", words(&shirt), words(&shield)))
}

/// Takes it all off again, for the steps after.
fn end_wear(world: &mut World, gate: &mut Gate, now: f32) {
    mouse(world, MouseButton::Right, false);
    let net = world.resource::<Net>();
    for (name, _) in [WORN_SHIRT, WORN_SHIELD] {
        net.command(&format!("clear {}", name.trim_start_matches("base:")));
    }
    // Back to the level before, for the steps after.
    net.command(&format!("skill defence {}", gate.worn_defence));
    // The fights before may have killed every creature about: one is summoned beside the
    // player, for `combat_hud` to tell its blows at.
    net.command(&format!("summon {} 1 ~ ~ ~2", CREATURE.trim_start_matches("base:")));
    gate.next(Step::CombatHud, now);
}

/// The blows `combat_hud` tells this side of, as the server would: the player's at a
/// creature (damage, kind) and the creature's at the player (damage, outcome).
/// One of every kind of splat (a plain blow, a beat, a miss, a counter, a flourish and the
/// green bleed), as many as `splats.most` at the most (`tell_blows` asserts it).
const STRUCK_SHOWN: [(u16, voxmar_engine::combat::Blow); 6] = {
    use voxmar_engine::combat::Blow;
    [(4, Blow::Plain), (6, Blow::Beat), (0, Blow::Plain), (7, Blow::Counter), (11, Blow::Flourish), (3, Blow::Bleed)]
};
/// At most `splats.most` (6) at once, as the splats on one target are capped: more would push
/// the oldest off the player's stack before the check reads it. Dodged shares Evaded's colour
/// and words' code path, so it is left out; the words unit test (`hurt_words`, `struck_words`)
/// covers every `Defended` kind, Dodged and the defence roll's Missed among them.
const HURT_SHOWN: [(u16, voxmar_engine::combat::Defended); 6] = {
    use voxmar_engine::combat::Defended;
    [(5, Defended::Hit), (0, Defended::Hit), (2, Defended::Blocked), (0, Defended::Blocked), (0, Defended::Perfect), (0, Defended::Evaded)]
};

/// The splats drawn now, by their words and colour (without its fade).
fn splats_drawn(world: &mut World) -> Vec<(String, Color)> {
    world
        .query::<(&crate::hud::combat::SplatWords, &Text, &TextColor, &InheritedVisibility)>()
        .iter(world)
        .filter(|(.., shown)| shown.get())
        .map(|(_, text, color, _)| (text.0.clone(), color.0.with_alpha(1.0)))
        .collect()
}

fn same_colour(a: Color, b: Color) -> bool {
    let (a, b) = (a.to_srgba(), b.to_srgba());
    (a.red - b.red).abs() < 0.01 && (a.green - b.green).abs() < 0.01 && (a.blue - b.blue).abs() < 0.01
}

/// Tells this side of a run of blows at the nearest creature and at the player.
fn tell_blows(world: &mut World, creature: u32) {
    let now = world.resource::<Time>().elapsed_secs_f64();
    let pips = world.resource::<Launch>().content.combat.feel.pips;
    let most = world.resource::<crate::hud::combat::CombatStyle>().splats.most;
    assert!(STRUCK_SHOWN.len() <= most && HURT_SHOWN.len() <= most, "the combat check tells more blows than `splats.most` ({most}) keeps on one target");
    let mut fighting = world.resource_mut::<crate::fight::Fighting>();
    for (i, (damage, blow)) in STRUCK_SHOWN.into_iter().enumerate() {
        fighting.struck(creature, damage, blow, (i as u8 + 1).min(pips), false, now);
    }
    for (damage, defended) in HURT_SHOWN {
        fighting.hurt(creature, damage, defended, now);
    }
    // All pips lit: the next blow on the beat would be a flourish.
    fighting.rhythm.set_pips(pips, now);
}

/// Whether every splat `tell_blows` told of is drawn, in its words and colour: what is
/// missing, if anything.
fn splats_missing(world: &mut World) -> Vec<String> {
    use crate::hud::combat::{CombatStyle, hurt_words, struck_words};
    let drawn = splats_drawn(world);
    let (locale, style) = (world.resource::<Locale>(), world.resource::<CombatStyle>());
    let wanted = STRUCK_SHOWN
        .iter()
        .map(|&(damage, blow)| (struck_words(locale, damage, blow), style.struck_look(damage, blow)))
        .chain(HURT_SHOWN.iter().map(|&(damage, defended)| (hurt_words(locale, damage, defended), style.hurt_look(damage, defended))));
    wanted.filter(|(words, look)| !drawn.iter().any(|(text, color)| text == words && same_colour(*color, look.color.color()))).map(|(words, _)| words).collect()
}

/// Game seconds the combat-marks step waits for a told run of blows to be drawn before it
/// tells them again (a splat shows `splats.seconds`, 0.9), and how many runs it tells at most.
const MARKS_WAIT: f32 = 0.6;
const MARKS_TELLS: u32 = 3;

const FALLEN_SECONDS: f32 = 0.2;
const FALLEN_WAIT: f32 = 6.0;

/// Hold the actual killed target in view, with native movement/attack inputs released.
/// Only its currently visible mark ends the watch; an absent/hidden mark times out.
pub(crate) fn watch_fallen(world: &mut World, id: Option<u32>, mut anchor: Option<Vec3>, since: &mut Option<f32>, now: f32) -> Option<Result<f32, f32>> {
    let began = since.is_none();
    let elapsed = now - *since.get_or_insert(now);
    key(world, KeyCode::KeyW, false);
    mouse(world, MouseButton::Left, false);
    let mut marks = world.query::<(&crate::hud::combat::HitSplat, &Visibility)>();
    if anchor.is_none() {
        anchor = marks.iter(world).find(|(mark, _)| id.is_some() && mark.creature == id && mark.killed).and_then(|(mark, _)| mark.anchor());
    }
    let killed = marks.iter(world).any(|(mark, _)| id.is_some() && mark.creature == id && mark.killed);
    let seen = marks.iter(world).filter(|(mark, visible)| id.is_some() && mark.killed && mark.creature == id && **visible != Visibility::Hidden)
        .map(|(mark, _)| mark.fallen_drawn_for()).fold(0.0, f32::max);
    if let Some(anchor) = anchor { aim_at(world, anchor); }
    if began || seen >= FALLEN_SECONDS || elapsed >= FALLEN_WAIT {
        let real = world.get_resource::<Time<Real>>().map(|time| (time.elapsed_secs_f64(), time.delta_secs_f64()));
        let game_dt = world.get_resource::<Time>().map(|time| time.delta_secs_f64());
        let player = world.resource::<LocalPlayer>();
        info!("fallen watch: target {id:?}, server killed {killed}, game {now:.3}s/dt {game_dt:?}, real {real:?}, watch {elapsed:.3}s, visible {seen:.3}s, anchor {anchor:?}, feet {:?}, yaw {:.3}, pitch {:.3}", player.position, player.yaw, player.pitch);
    }
    if elapsed >= FALLEN_WAIT { Some(Err(seen)) }
    else if seen >= FALLEN_SECONDS { Some(Ok(seen)) }
    else { None }
}

fn combat_hud(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::hud::combat::SplatsShown;
    // The snail `end_wear` summoned beside the player: not a bear the fights left standing
    // out of the view.
    let snail = world.resource::<Launch>().content.creatures.find(CREATURE);
    let creature = {
        let (creatures, player) = (world.resource::<crate::creatures::Creatures>(), world.resource::<LocalPlayer>());
        let eye = player.eye();
        creatures.living().filter(|(_, species, feet)| Some(*species) == snail && feet.distance(eye) < 14.0).min_by(|a, b| a.2.distance(eye).total_cmp(&b.2.distance(eye))).map(|(id, _, feet)| (id, feet))
    };
    // Once the blows are told at one, that one, wherever another stands.
    let creature = match gate.marked {
        Some(id) => world.resource::<crate::creatures::Creatures>().living().find(|&(at, ..)| at == id).map(|(id, _, feet)| (id, feet)),
        None => creature,
    };
    match gate.phase {
        0 => {
            // A creature to tell blows at (`end_wear` summoned one): waited for.
            if creature.is_none() {
                if gate.server_in_phase() > UNANSWERED {
                    gate.check("combat marks", Err("no creature near to mark".into()));
                    gate.next(Step::CombatFeel, now);
                }
                return;
            }
            // What the fights before drew: a splat for every blow, and the meter.
            let (struck, hurt) = {
                let fighting = world.resource::<crate::fight::Fighting>();
                (fighting.struck_count, fighting.hurt_count)
            };
            let shown = world.resource::<SplatsShown>();
            // A killing blow's splat stays over the falling body (the fights killed a snail
            // and bears), drawn for some time, not dropped with the creature: by the game's
            // clock, as a frame may take a quarter second under load.
            let result = if !shown.meter {
                Err("the meter never showed through the fights".to_owned())
            } else if shown.struck != struck || shown.hurt != hurt {
                Err(format!("{} splats drawn for the player's {struck} blows, {} for the {hurt} at the player", shown.struck, shown.hurt))
            } else if shown.over_fallen < FALLEN_SECONDS {
                Err(format!("no splat stayed over a creature killed in the fights for {FALLEN_SECONDS} s (the most: {:.2} s)", shown.over_fallen))
            } else {
                let sample: Vec<&str> = shown.recent.iter().map(String::as_str).take(8).collect();
                Ok(format!(
                    "a splat drawn for each of the player's {struck} blows and the {hurt} at them ({}), the killing blows' over the falling bodies (up to {:.2} s), and the meter, up to {} pips",
                    sample.join(", "),
                    shown.over_fallen,
                    shown.pips
                ))
            };
            gate.check("combat marks: fights", result);
            let Some((id, feet)) = creature else { return };
            aim_at(world, feet + Vec3::Y * 0.3);
            gate.count_before = world.resource::<SplatsShown>().flashes;
            gate.marked = Some(id);
            gate.marked_at = now;
            gate.marked_tries = 1;
            tell_blows(world, id);
            gate.advance(now);
        }
        1 | 3 => {
            if let Some((_, feet)) = creature {
                aim_at(world, feet + Vec3::Y * 0.3);
            }
            if in_phase < 0.25 {
                return;
            }
            // Read each frame until every splat is drawn. A busy machine can draw them late
            // (a frame of up to a quarter second); one told run lasts `splats.seconds`, so
            // past `MARKS_WAIT` the run is told again, a few times at most.
            let missing = splats_missing(world);
            let language = if gate.phase == 1 { "en" } else { "ja" };
            if !missing.is_empty() && now - gate.marked_at < MARKS_WAIT {
                return;
            }
            if !missing.is_empty() && gate.marked_tries < MARKS_TELLS && let Some((id, _)) = creature {
                gate.marked_tries += 1;
                gate.marked_at = now;
                tell_blows(world, id);
                return;
            }
            if !missing.is_empty() {
                gate.check("combat marks", Err(format!("{language}: no splat reads {missing:?} in {} runs told; drawn: {:?}", gate.marked_tries, splats_drawn(world))));
                world.resource_mut::<crate::settings::Settings>().language = "en".into();
                set_language(world, "en");
                gate.next(Step::CombatFeel, now);
                return;
            }
            let meter = world.query_filtered::<&InheritedVisibility, With<crate::hud::combat::MeterRoot>>().iter(world).any(|v| v.get());
            // The blow that hurt (5, a hit) flashed the health bar.
            let flashed = world.resource::<SplatsShown>().flashes > gate.count_before;
            let name = if gate.phase == 1 { "combat-en" } else { "combat-ja" };
            let Some(_) = gate.shot(world, name) else { return };
            let result = match (meter, flashed) {
                (true, true) => Ok(format!("{} splats in their colours, the meter and all pips lit, the health bar flashed", STRUCK_SHOWN.len() + HURT_SHOWN.len())),
                (false, _) => Err("the splats showed without the meter".into()),
                (_, false) => Err("a hit on the player did not flash the health bar".into()),
            };
            gate.check(&format!("combat marks: {language}"), result);
            if gate.phase == 1 {
                // The setting too, as a splat's words are fixed when it is drawn: a change
                // to any setting meanwhile would load the setting's language again.
                world.resource_mut::<crate::settings::Settings>().language = "ja".into();
                set_language(world, "ja");
            } else {
                // With damage numbers off, the same blows draw nothing.
                world.resource_mut::<crate::settings::Settings>().language = "en".into();
                set_language(world, "en");
                world.resource_mut::<crate::settings::Settings>().damage_numbers = false;
                let shown = world.resource::<SplatsShown>();
                gate.count_before = shown.struck + shown.hurt;
                if let Some((id, _)) = creature {
                    tell_blows(world, id);
                }
            }
            gate.advance(now);
        }
        // The English splats gone, the same run told again, to be drawn in Japanese.
        2 => {
            if let Some((_, feet)) = creature {
                aim_at(world, feet + Vec3::Y * 0.3);
            }
            let up = world.query::<&crate::hud::combat::HitSplat>().iter(world).count();
            if up == 0 && let Some((id, _)) = creature {
                gate.count_before = world.resource::<SplatsShown>().flashes;
                gate.marked_at = now;
                gate.marked_tries = 1;
                tell_blows(world, id);
                gate.advance(now);
            } else if gate.server_in_phase() > UNANSWERED {
                gate.check("combat marks", Err(format!("ja: {up} English splats still up, or no creature to mark ({})", creature.is_some())));
                world.resource_mut::<crate::settings::Settings>().language = "en".into();
                set_language(world, "en");
                gate.next(Step::CombatFeel, now);
            }
        }
        4 => {
            let told = {
                let fighting = world.resource::<crate::fight::Fighting>();
                (fighting.struck_count, fighting.hurt_count)
            };
            let shown = world.resource::<SplatsShown>();
            if in_phase < 0.25 || shown.seen() != told {
                return;
            }
            let drawn = shown.struck + shown.hurt - gate.count_before;
            gate.check("damage numbers off", if drawn == 0 { Ok(format!("with damage numbers off, {} blows drew no splat", STRUCK_SHOWN.len() + HURT_SHOWN.len())) } else { Err(format!("with damage numbers off, {drawn} splats were drawn")) });
            world.resource_mut::<crate::settings::Settings>().damage_numbers = true;
            // The sword guards only at the Attack level that wields it (the feel step left
            // Attack at 1, where it is held as fists, which do not guard).
            world.resource::<Net>().command(&format!("skill attack {WEAPON_LEVEL}"));
            gate.advance(now);
        }
        // The guard on a toggle: one click up, the next down, the sword in hand at the level
        // that guards with it and nothing under the crosshair, so the click is the guard's
        // and nothing else's (the view turns until nothing is: the workshop's kiln hangs
        // overhead).
        5 => {
            world.resource_mut::<crate::settings::Settings>().toggle_guard = true;
            let in_hand = select(world, WEAPON);
            let guards = {
                let (player, launch) = (world.resource::<LocalPlayer>(), world.resource::<Launch>());
                let held = player.inventory.get(player.selected).map(|s| s.item);
                launch.content.combat.guard_of(held, &player.inventory.worn, |skill| launch.content.skills.level(player.skills.xp(skill))).is_some()
            };
            let clear = {
                let target = world.resource::<crate::interact::Target>();
                target.hit.is_none() && target.creature.is_none()
            };
            if in_hand && guards && clear && in_phase > 0.2 {
                mouse(world, MouseButton::Right, true);
                gate.advance(now);
            } else if gate.server_in_phase() > UNANSWERED {
                gate.check("toggle guard", Err(format!("the sword in hand: {in_hand}; guarding with it: {guards}; nothing under the crosshair: {clear}")));
                world.resource_mut::<crate::settings::Settings>().toggle_guard = false;
                world.resource::<Net>().command("skill attack 1");
                gate.next(Step::CombatFeel, now);
            } else {
                look(world, (in_phase * 90.0) % 360.0, 30.0);
            }
        }
        6 => {
            mouse(world, MouseButton::Right, false);
            if in_phase < 0.3 {
                return;
            }
            gate.count_before = world.resource::<crate::fight::Fighting>().guard.is_some() as u32;
            mouse(world, MouseButton::Right, true);
            gate.advance(now);
        }
        _ => {
            mouse(world, MouseButton::Right, false);
            if in_phase < 0.3 {
                return;
            }
            let (up_after_one, up_after_two) = (gate.count_before == 1, world.resource::<crate::fight::Fighting>().guard.is_some());
            let why = {
                let (ui, player, target) = (world.resource::<UiState>(), world.resource::<LocalPlayer>(), world.resource::<crate::interact::Target>());
                format!("cursor locked {}, dead {}, the click used for something else {}", ui.cursor_locked, player.dead, target.used_right)
            };
            gate.check(
                "toggle guard",
                if up_after_one && !up_after_two {
                    Ok("with the guard on a toggle, a click raised it and stayed up let go, and the next lowered it".into())
                } else {
                    Err(format!("on a toggle, up after one click: {up_after_one}, after two: {up_after_two} ({why})"))
                },
            );
            world.resource_mut::<crate::settings::Settings>().toggle_guard = false;
            world.resource::<Net>().command("skill attack 1");
            gate.next(Step::CombatFeel, now);
        }
    }
}

/// The blows `combat_feel` tells: a flourish of the player's for this much, and what came
/// at the player (a hit that hurt, a block and a perfect block).
const FEEL_STRUCK: u16 = 4;
const FEEL_HURT: [(u16, voxmar_engine::combat::Defended); 3] = {
    use voxmar_engine::combat::Defended;
    [(3, Defended::Hit), (0, Defended::Blocked), (0, Defended::Perfect)]
};

fn combat_feel(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    use crate::fight_fx::{CameraKick, HandKick, Heard, HitStop};
    let creature = gate.marked.and_then(|id| world.resource::<crate::creatures::Creatures>().living().find(|&(at, ..)| at == id).map(|(id, _, feet)| (id, feet)));
    if let Some((_, feet)) = creature {
        aim_at(world, feet + Vec3::Y * 0.3);
    }
    let asked = |world: &World| world.resource::<Heard>().asked.clone();
    match gate.phase {
        0 => {
            let Some((id, _)) = creature else {
                if gate.server_in_phase() > UNANSWERED {
                    gate.check("combat feel", Err("the marked creature is gone".into()));
                    gate.next(Step::CreatureWalk, now);
                }
                return;
            };
            // Let the aim settle on it, so the shot shows it.
            if in_phase < 0.2 {
                return;
            }
            gate.feel_asked = asked(world);
            let heard = world.resource::<Heard>();
            gate.feel_counts = (heard.bursts, heard.kicks, heard.knocks);
            let pips = world.resource::<Launch>().content.combat.feel.pips;
            world.resource_mut::<crate::creatures::Creatures>().struck(id, FEEL_STRUCK, voxmar_engine::combat::Blow::Flourish, false, now);
            let at = world.resource::<Time>().elapsed_secs_f64();
            let mut fighting = world.resource_mut::<crate::fight::Fighting>();
            fighting.struck(id, FEEL_STRUCK, voxmar_engine::combat::Blow::Flourish, pips, false, at);
            for (damage, defended) in FEEL_HURT {
                fighting.hurt(id, damage, defended, at);
            }
            gate.advance(now);
        }
        // The frame after: everything the blows set going, before any of it fades.
        1 => {
            let Some((id, _)) = creature else {
                gate.check("combat feel", Err("the marked creature went before its blows showed".into()));
                gate.next(Step::CreatureWalk, now);
                return;
            };
            let after = asked(world);
            let grew = |name: &str| after.get(name).copied().unwrap_or(0) > gate.feel_asked.get(name).copied().unwrap_or(0);
            let hit = after.iter().any(|(name, &n)| name.starts_with("hit") && n > gate.feel_asked.get(name).copied().unwrap_or(0));
            let missing: Vec<&str> = ["flourish", "hurt", "blocked", "perfect"].into_iter().filter(|name| !grew(name)).chain((!hit).then_some("hit")).collect();
            let edges = world.query_filtered::<&Visibility, With<crate::fight_fx::Vignette>>().iter(world).any(|v| *v != Visibility::Hidden);
            let (heard, creatures) = (world.resource::<Heard>(), world.resource::<crate::creatures::Creatures>());
            let (bursts, kicks, knocks) = (heard.bursts - gate.feel_counts.0, heard.kicks - gate.feel_counts.1, heard.knocks - gate.feel_counts.2);
            let jolted = creatures.jolted(id);
            let held = world.resource::<HitStop>().holds(now);
            let hand = world.resource::<HandKick>().offset(now);
            let turned = world.resource::<CameraKick>().turn(now);
            let result = if !missing.is_empty() {
                Err(format!("no sound asked for: {missing:?}"))
            } else if !jolted {
                Err("the struck creature showed no freeze, flash or flinch".into())
            } else if bursts < 2 || knocks < 3 || kicks < 3 {
                Err(format!("{bursts} bursts of sparks (2 wanted), {knocks} knocks to the hand (3), {kicks} kicks to the view (3)"))
            } else if !held || hand.z >= 0.0 || turned == Vec3::ZERO || !edges {
                Err(format!("the swing held still: {held}; the hand knocked to {hand}; the view turned by {turned}; the screen's edges red: {edges}"))
            } else {
                Ok(format!("sounds asked for, the creature jolted, {bursts} bursts of sparks, the hand knocked back {:.3}, the view turned {:.2} degrees, the edges reddened", -hand.z, turned.length().to_degrees()))
            };
            gate.check("combat feel: blows", result);
            // What the fights before showed: creatures planting and coiling deep into their
            // strikes, standing out their recovery, and the fallen going up in smoke as they
            // vanish.
            let (strikes, deaths, poofs) = (creatures.strikes, creatures.deaths, heard.poofs);
            let reads = &creatures.reads;
            let slid = reads.iter().find(|r| r.slid > r.planted);
            let shallow = reads.iter().find(|r| r.coiled < STRIKE_COILED);
            let stood: Vec<f32> = reads.iter().filter_map(|r| r.recovered).collect();
            let result = if strikes == 0 {
                Err("no creature was seen to strike in the fights".into())
            } else if let Some(read) = slid {
                Err(format!("a creature slid {:.2} blocks as it wound up, where planted it would have moved {:.2} at most", read.slid, read.planted))
            } else if let Some(read) = shallow {
                Err(format!("a creature struck coiled only {:.2} deep ({STRIKE_COILED} wanted)", read.coiled))
            } else if let Some(most) = stood.iter().copied().find(|&most| most < STRIKE_RECOVERED) {
                Err(format!("a creature stood out its recovery showing only {most:.2} of its recover stance ({STRIKE_RECOVERED} wanted)"))
            } else if deaths > 0 && poofs == 0 {
                Err(format!("{deaths} creatures died in view and none went up in smoke"))
            } else {
                let deepest = reads.iter().map(|r| r.coiled).fold(0.0, f32::max);
                let most = reads.iter().map(|r| r.slid).fold(0.0, f32::max);
                Ok(format!(
                    "{strikes} strikes snapped out of their wind-ups ({} read: coiled up to {deepest:.2} deep, slid at most {most:.2} blocks coiling; {} stood out their recovery), {poofs} of {deaths} fallen gone in smoke",
                    reads.len(),
                    stood.len()
                ))
            };
            gate.check("combat feel: fights", result);
            gate.advance(now);
        }
        _ => {
            if gate.shot(world, "combat-feel").is_some() {
                gate.next(Step::CreatureWalk, now);
            }
        }
    }
}

/// How deep (of its coil in `combat_fx.ron`) a creature seen to strike must have coiled
/// first, short of the whole for a server's word that comes a little early; and how much of
/// its recover stance one that stood out its recovery must have shown, short of the whole
/// for a slow machine's frames.
const STRIKE_COILED: f32 = 0.6;
const STRIKE_RECOVERED: f32 = 0.5;

/// The deer the creature motion steps walk, an id the server never gives, and its species.
const PUPPET: u32 = u32::MAX - 7;
const PUPPET_SPECIES: &str = "red_deer";
/// Frames it walks (and runs, and turns) for, after `PUPPET_WARM` to get going, each frame
/// `PUPPET_STEP` seconds of its time whatever the frame took (`Creatures::watch_step`), so a
/// slow or loaded machine judges the same poses a fast one does.
const PUPPET_STEP: f32 = 1.0 / 60.0;
const PUPPET_FRAMES: u32 = 72;
const PUPPET_WARM: u32 = 12;
/// The most a foot that is down may slide, for each block the body goes.
const MOST_SLIP: f32 = 0.15;

/// The CreatureWalk, CreatureRun and CreatureTurn steps, by `stage`: 0 putting the deer
/// down, 1 walking, 2 running, 3 turning.
fn creature_motion(world: &mut World, gate: &mut Gate, stage: u32) {
    use crate::creatures::{Creatures, Watched};
    let launch = world.resource::<Launch>();
    let Some(species) = launch.content.creatures.find(PUPPET_SPECIES) else {
        gate.check("creature motion", Err(format!("no {PUPPET_SPECIES} in the content")));
        gate.next(Step::Cook, gate.since);
        return;
    };
    let def = &launch.content.creatures.get(species).def;
    let (walk, run, turn, health) = (def.movement.walk, def.movement.run, def.movement.turn.to_radians(), def.health);
    let most_turn = turn * world.resource::<crate::creature_motion::Motion>().turn.share;
    let dt = PUPPET_STEP;
    let now = world.resource::<Time>().elapsed_secs();
    let facing = |d: Vec3| (-d.x).atan2(-d.z);
    let view = |at: Vec3, velocity: Vec3, yaw: f32| voxmar_protocol::CreatureView {
        id: PUPPET,
        species,
        position: at,
        velocity,
        yaw,
        pose: voxmar_engine::creature::brain::Pose::Idle,
        tell: 0,
        health,
        scale: 1.0,
        seed: 1,
        kept: false,
        owner: None,
        young: false,
        ready: false,
        trust: 0,
        genes: [0; voxmar_engine::genes::MOST],
    };
    // Walking across the view, then running straight away from the player.
    let (across, away) = (Vec3::X, Vec3::Z);
    let (at, frames, shot) = gate.puppet;
    let moving = |world: &mut World, gate: &mut Gate, direction: Vec3, speed: f32| {
        let (at, frames) = (gate.puppet.0 + direction * speed * dt, gate.puppet.1 + 1);
        (gate.puppet.0, gate.puppet.1) = (at, frames);
        world.resource_mut::<Creatures>().receive(vec![view(at, direction * speed, facing(direction))], vec![]);
        if frames == PUPPET_WARM {
            world.resource_mut::<Creatures>().watched = Watched::default();
        }
    };
    let judged = |w: &Watched, how: &str| -> Result<String, String> {
        if w.planted < 20 || w.frames < PUPPET_FRAMES / 2 {
            Err(format!("{how}: only {} frames of feet down over {} frames drawn", w.planted, w.frames))
        } else if w.slip() > MOST_SLIP {
            Err(format!("{how}: its feet slid {:.2} of the ground it went while down ({MOST_SLIP} the most)", w.slip()))
        } else {
            Ok(format!("{how}: feet slid {:.3} of the ground it went while down, over {} foot frames", w.slip(), w.planted))
        }
    };
    match stage {
        0 => {
            let start = gate.feet.as_vec3() + Vec3::new(-5.0, 0.0, 5.0);
            gate.puppet = (start, 0, false);
            let mut creatures = world.resource_mut::<Creatures>();
            creatures.receive(vec![view(start, Vec3::ZERO, facing(across))], vec![]);
            creatures.watch = Some(PUPPET);
            creatures.watch_step = Some(PUPPET_STEP);
            gate.advance(now);
        }
        1 => {
            aim_at(world, at + Vec3::Y);
            moving(world, gate, across, walk);
            if frames >= PUPPET_WARM + PUPPET_FRAMES {
                let result = judged(&world.resource::<Creatures>().watched, "walking");
                gate.check("creature motion: walk", result);
                gate.puppet.1 = 0;
                gate.next(Step::CreatureRun, now);
            }
        }
        // From near the player again, so the shot shows it close, mid-stride.
        2 => {
            if frames == 0 && !shot {
                gate.puppet.0 = gate.feet.as_vec3() + Vec3::new(0.0, 0.0, 5.0);
            }
            aim_at(world, gate.puppet.0 + Vec3::Y);
            moving(world, gate, away, run);
            if frames >= PUPPET_WARM && !shot && gate.shot(world, "creature-motion").is_some() {
                gate.puppet.2 = true;
            }
            if frames >= PUPPET_WARM + PUPPET_FRAMES && gate.puppet.2 {
                let watched = world.resource::<Creatures>().watched;
                let result = judged(&watched, "running away").and_then(|ok| {
                    if watched.flee < 0.5 { Err(format!("running away from the player it showed fleeing only {:.2}", watched.flee)) } else { Ok(format!("{ok}; fleeing shown {:.2}", watched.flee)) }
                });
                gate.check("creature motion: run", result);
                gate.puppet.1 = 0;
                gate.next(Step::CreatureTurn, now);
            }
        }
        // Standing, told to face a quarter turn round: it turns there, no faster than its
        // kind may.
        3 => {
            let toward = facing(away) + std::f32::consts::FRAC_PI_2;
            gate.puppet.1 = frames + 1;
            let mut creatures = world.resource_mut::<Creatures>();
            if frames == 0 {
                creatures.watched = Watched::default();
            }
            creatures.receive(vec![view(at, Vec3::ZERO, toward)], vec![]);
            if frames >= PUPPET_FRAMES {
                let w = creatures.watched;
                let off = (toward - w.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                let result = if w.fastest_turn > most_turn * 1.02 {
                    Err(format!("turned at {:.0} degrees a second, {:.0} the most", w.fastest_turn.to_degrees(), most_turn.to_degrees()))
                } else if off.abs() > 0.05 {
                    Err(format!("{:.0} degrees short of the way it was told to face", off.to_degrees()))
                } else {
                    Ok(format!("turned a quarter round at up to {:.0} degrees a second ({:.0} the most)", w.fastest_turn.to_degrees(), most_turn.to_degrees()))
                };
                gate.check("creature motion: turn", result);
                creatures.receive(vec![], vec![voxmar_protocol::CreatureGone { id: PUPPET, died: false }]);
                creatures.watch = None;
                creatures.watch_step = None;
                gate.next(Step::Cook, now);
            }
        }
        _ => {}
    }
}

/// A campfire on the pad, raw rabbit in hand: a right click on the fire cooks one, which
/// comes back two seconds later as cooked rabbit or, at a low Cooking level, sometimes
/// burnt. Waited for by the server's clock.
/// Takes the workshop's Crucible Kiln down, so the pool's bank is clear for the swim.
fn kiln_down(world: &mut World, kiln: IVec3) {
    let (a, b) = (kiln - IVec3::new(1, 0, 1), kiln + IVec3::new(1, 1, 1));
    world.resource::<Net>().command(&format!("fill {} {} {} {} {} {} air", a.x, a.y, a.z, b.x, b.y, b.z));
}

/// The lit kiln's bronze: three copper ingots in hand and a tin ore in the pack, used on
/// the crucible, make four bronze in one go and leave neither. Then the kiln comes down
/// and a boiler with a steam crusher beside it goes where it stood.
fn bronze(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let kiln = gate.kiln;
    let (copper, slot) = holding(world, "base:copper_ingot");
    let (tin, bronze) = (holding(world, "base:tin_ore").0, holding(world, "base:bronze_ingot").0);
    if bronze >= 4 && copper == 0 && tin == 0 {
        gate.check("workshop: bronze", Ok(format!("three copper ingots and a tin ore made {bronze} bronze at the lit crucible in one go")));
        kiln_down(world, kiln);
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
        return;
    }
    // Both clocks: a loaded server is slow to answer, a starved client slow to see it.
    if gate.server_now - gate.bronze_since.0 > 20.0 && in_phase - gate.bronze_since.1 > 20.0 {
        kiln_down(world, kiln);
        gate.check("workshop: bronze", Err(format!("the crucible never made bronze: holding {copper} copper ingots, {tin} tin ore, {bronze} bronze")));
        return end_workshop(world, gate, now);
    }
    if let Some(slot) = slot.filter(|_| copper >= 3 && tin >= 1) {
        world.resource_mut::<LocalPlayer>().selected = slot;
        if gate.server_now - gate.kiln_asked >= 3.0 {
            gate.kiln_asked = gate.server_now;
            let actions = world.resource::<crate::Actions>().0.clone();
            // The copper just selected goes first, as for the kiln's lighting.
            world.resource::<Net>().propose_from(world.resource::<LocalPlayer>(), actions.expect("base:cook"), voxmar_engine::action::ActionTarget::Block { block: kiln, face: voxmar_engine::coords::Face::NegY });
        }
    }
}

fn cook(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    let fire = gate.feet + IVec3::new(0, 0, 14);
    let hearth = fire + IVec3::new(3, 0, 0);
    let chimney = [hearth + IVec3::new(-1, 1, -1), hearth + IVec3::new(-1, 2, -1)];
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            for line in ["clear", "give oak_log 2", "give cobblestone 2", "skill cooking 99"] { net.command(line); }
            gate.advance(now);
        }
        1 => {
            let (logs, log_slot) = holding(world, "base:oak_log");
            let (stone, stone_slot) = holding(world, "base:cobblestone");
            let cleared = world.resource::<LocalPlayer>().inventory.slots.iter().flatten().count() == 2;
            if let (Some(log_slot), Some(stone_slot)) = (log_slot, stone_slot) && logs == 2 && stone == 2 && cleared {
                let net = world.resource::<Net>();
                let transfer = world.resource::<crate::Actions>().0.expect("core:transfer");
                for (from, cells) in [(log_slot, &voxmar_engine::craft::SMALL[..2]), (stone_slot, &voxmar_engine::craft::SMALL[2..])] {
                    for &cell in cells {
                        net.propose(transfer, voxmar_engine::action::ActionTarget::Slots { from: from as u8, to: (voxmar_engine::inventory::CRAFT + cell) as u8, count: 1 });
                    }
                }
                net.propose(world.resource::<crate::Actions>().0.expect("core:craft"), voxmar_engine::action::ActionTarget::Slot(0));
                gate.advance(now);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("first night: campfire", Err("campfire ingredients never arrived".into()));
                gate.next(Step::Ore, now);
            }
        }
        2 => {
            // The grid craft is answered by the server, not at once: First Night counts a
            // campfire crafted, so the cooking waits until one is in hand.
            if item_count(world, "base:campfire") == 0 {
                if gate.server_in_phase() > 10.0 {
                    gate.check("first night: campfire", Err("the grid never made a campfire from two logs over two cobblestone".into()));
                    gate.next(Step::Ore, now);
                }
                return;
            }
            let net = world.resource::<Net>();
            // The creature the combat check summoned goes, or its level tag would still be
            // up behind the menus' text check.
            net.command("creatures kill");
            net.command(&format!("setblock {} {} {} campfire", fire.x, fire.y, fire.z));
            net.command("give raw_small_game 3");
            // In reach of it, wherever the steps before left the player.
            net.command(&format!("tp {} {} {}", fire.x as f32 + 0.5, fire.y, fire.z as f32 - 1.5));
            gate.advance(now);
        }
        3 => {
            let client = world.resource::<ClientWorld>();
            let (Some(raw), Some(campfire)) = (client.registry.id("base:raw_small_game"), client.registry.id("base:campfire")) else {
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
                    gate.count_before = item_count(world, "base:cooked_small_game") + item_count(world, "base:burnt_food");
                    gate.advance(now);
                }
                _ if gate.server_in_phase() > 10.0 => {
                    gate.check("cook", Err(format!("the campfire ({lit}) or raw rabbit in the hotbar ({slot:?}) never came")));
                    gate.next(Step::Ore, now);
                }
                _ => {}
            }
        }
        4 => {
            aim_at(world, fire.as_vec3() + Vec3::new(0.5, 0.3, 0.5));
            let cooked = item_count(world, "base:cooked_small_game") + item_count(world, "base:burnt_food");
            let first_night = world.resource::<LocalPlayer>().quests.iter().any(|q| q.name == "base:first_night" && q.done);
            if cooked > gate.count_before && first_night {
                mouse(world, MouseButton::Right, false);
                gate.check("cook", Ok(format!("{} came back from the fire", cooked - gate.count_before)));
                gate.check("first night: food", Ok("a crafted campfire and successful cooked meal completed First Night".into()));
                gate.advance(now);
            } else if gate.waited_out(15.0) {
                mouse(world, MouseButton::Right, false);
                // What the hand held, for a step before may have left something else in it.
                let held = {
                    let (player, client) = (world.resource::<LocalPlayer>(), world.resource::<ClientWorld>());
                    player.inventory.get(player.selected).map_or("nothing".to_owned(), |s| client.registry.def(s.item).name.clone())
                };
                let first_night = world.resource::<LocalPlayer>().quests.iter().find(|q| q.name == "base:first_night").map(|q| (q.done, q.tasks.clone()));
                let reason = if cooked > gate.count_before {
                    format!("food came back but First Night never finished in 15 server seconds: {first_night:?} (goals are the campfire crafted, then food cooked at it)")
                } else {
                    format!("right clicks on the campfire cooked nothing in 15 server seconds ({in_phase:.1} game s), the hand holding {held}")
                };
                gate.check("cook", Err(reason));
                gate.next(Step::Ore, now);
            } else {
                gate.tap(world, true, now);
            }
        }
        // The fire's particles, counted for a while: smoke, embers and sparks rising from
        // it. Counting is the same at any frame rate; a picture of the air over it, taken
        // twice, is kept for the record (a few faint puffs sit near any threshold).
        5 => {
            aim_at(world, fire.as_vec3() + Vec3::new(0.5, 1.6, 0.5));
            if in_phase > 0.5 && gate.shot(world, "fire").is_some() {
                world.resource_mut::<crate::emitters::Emitters>().watch(fire);
                gate.advance(now);
            }
        }
        6 if in_phase > FIRE_WATCH => {
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
            let net = world.resource::<Net>();
            for (at, block) in [(hearth, "hearthstone"), (chimney[0], "chimney"), (chimney[1], "chimney")] {
                net.command(&format!("setblock {} {} {} {block}", at.x, at.y, at.z));
            }
            gate.advance(now);
        }
        // A hearthstone beside it, with a chimney two tall at its corner: the chimney's top
        // smokes (`over` the hearth), as home does from across a valley.
        7 => {
            let placed = {
                let client = world.resource::<ClientWorld>();
                client.registry.id("base:chimney").is_some_and(|c| client.block(chimney[1]) == Some(c))
            };
            if placed {
                world.resource_mut::<crate::emitters::Emitters>().watch(chimney[1]);
                gate.advance(now);
            } else if in_phase > 5.0 {
                gate.check("chimney smoke", Err(format!("the chimney was not on the client {in_phase:.1} s after it was placed")));
                gate.phase = 8;
            }
        }
        // Watched until enough puffs rose, or until so many were owed (by the chimney's
        // rate, the particle amount and its share) that missing them is no chance.
        8 => {
            let amount = world.resource::<crate::render::particles::Particles>().amount;
            let owed = {
                let (emitters, client) = (world.resource::<crate::emitters::Emitters>(), world.resource::<ClientWorld>());
                gate.chimney_owed += emitters.rate_of(chimney[1], client, amount) * world.resource::<Time>().delta_secs().min(0.1);
                gate.chimney_owed
            };
            let count = world.resource::<crate::emitters::Emitters>().watched;
            let done = count >= CHIMNEY_PARTICLES;
            if !done && owed < CHIMNEY_OWED && in_phase < CHIMNEY_WAIT {
                return;
            }
            let mut emitters = world.resource_mut::<crate::emitters::Emitters>();
            let why = emitters.why_quiet(chimney[1], amount);
            emitters.watch = None;
            gate.chimney_owed = 0.0;
            gate.check("chimney smoke", if done {
                Ok(format!("{count} puffs rose from the chimney's top over the hearthstone in {in_phase:.1} s ({owed:.1} owed)"))
            } else {
                Err(format!("only {count} puffs rose from the chimney's top over a hearthstone in {in_phase:.1} s, under {CHIMNEY_PARTICLES} with {owed:.1} owed ({why})"))
            });
            let net = world.resource::<Net>();
            for at in [hearth, chimney[0], chimney[1]] {
                net.command(&format!("setblock {} {} {} air", at.x, at.y, at.z));
            }
            gate.next(Step::Dry, now);
        }
        _ => {}
    }
}

/// Where the dry step stands its rack: two blocks from the cook step's campfire, within
/// the three that make it dry twice as fast.
const RACK_FROM_FIRE: IVec3 = IVec3::new(2, 0, 0);

/// Server seconds the dry step waits on each answer.
const DRY_WAIT: f32 = 10.0;
/// The cooking pot, from the rack: against the campfire on its other side.
const POT_FROM_RACK: IVec3 = IVec3::new(-3, 0, 0);
/// The water the pot needs near, sunk in the ground, from the pot.
const POT_WATER: IVec3 = IVec3::new(-2, -1, 0);

/// Share of a rack shot's middle that must turn meat-coloured once the rabbit hangs, over
/// the bare rack's: the strips cover about half the middle of the hung rack.
const MEAT_SHOWS: f32 = 0.08;

/// A drying rack by the campfire, seen from three blocks at eye height (shot `rack_bare`
/// once the renderer has it), raw rabbit selected: right clicks on it hang the rabbit
/// (`base:use_station`), so the server sends the hung rack back, and once the renderer has
/// that too (shot `rack`) the meat must show in the middle of the shot. Then the rack goes,
/// letting the rabbit fall as a broken rack does, and an earth clamp loaded with clay in its
/// place smoulders (shot `clamp`, once drawn) until it goes too. Then a cooking pot against the
/// fire, water near, is set simmering with wheat (shot `pot`).
fn dry(world: &mut World, gate: &mut Gate, now: f32) {
    let rack = gate.feet + IVec3::new(0, 0, 14) + RACK_FROM_FIRE;
    let waited = gate.server_in_phase() > DRY_WAIT;
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command(&format!("setblock {} {} {} drying_rack", rack.x, rack.y, rack.z));
            net.command("give raw_small_game 2");
            net.command(&format!("tp {} {} {}", rack.x as f32 + 0.5, rack.y, rack.z as f32 - 2.5));
            gate.advance(now);
        }
        1 => {
            aim_at(world, rack.as_vec3() + RACK_MEAT);
            let client = world.resource::<ClientWorld>();
            let stood = client.registry.id("base:drying_rack").is_some_and(|r| client.block(rack) == Some(r));
            let there = world.resource::<LocalPlayer>().position.distance(Vec3::new(rack.x as f32 + 0.5, rack.y as f32, rack.z as f32 - 2.5)) < 0.5;
            if stood && there && drawn(world, rack) && gate.shot(world, "rack_bare").is_some() && select(world, "base:raw_small_game") {
                gate.advance(now);
            } else if waited {
                gate.check("drying rack", Err(format!("the rack ({stood}) or raw rabbit in hand never came in {DRY_WAIT} server seconds")));
                gate.next(Step::Snare, now);
            }
        }
        2 => {
            aim_at(world, rack.as_vec3() + RACK_MEAT);
            let client = world.resource::<ClientWorld>();
            let hung = client.registry.id("base:drying_rack_hung").is_some_and(|h| client.block(rack) == Some(h));
            if hung {
                mouse(world, MouseButton::Right, false);
                gate.advance(now);
            } else if gate.waited_out(DRY_WAIT) {
                mouse(world, MouseButton::Right, false);
                gate.check("drying rack", Err(format!("right clicks with raw rabbit in hand never hung the rack in {DRY_WAIT} server seconds")));
                gate.next(Step::Snare, now);
            } else {
                gate.tap(world, true, now);
            }
        }
        3 => {
            aim_at(world, rack.as_vec3() + RACK_MEAT);
            // The word that the fire near makes it quicker: the tail of `station.loaded_faster`,
            // which nothing before this step says.
            let faster = world.resource::<crate::lang::Locale>().get("station.loaded_faster").replace("{words}", "").trim().to_owned();
            let said = world.resource::<crate::hud::console::Console>().since(0.0).any(|line| line.contains(faster.as_str()));
            if !(said && drawn(world, rack)) && !waited {
                return;
            }
            if let Some(hung) = gate.shot(world, "rack") {
                let result = if !said {
                    Err(format!("the rack hung, but chat never said \"{faster}\" in {DRY_WAIT} server seconds, with a campfire two blocks off"))
                } else {
                    let client = world.resource::<ClientWorld>();
                    let shown = client.check_meshed(ColumnPos::of_block(rack));
                    let shares = gate.earlier_shot("rack_bare").ok_or_else(|| "the bare rack was never shot".to_owned()).and_then(|bare| Ok((meat_share(&bare)?, meat_share(&hung)?)));
                    match (shown, shares) {
                        (Err(e), _) | (_, Err(e)) => Err(format!("the rack hung, but {e}")),
                        (Ok(()), Ok((bare, meat))) if meat - bare < MEAT_SHOWS => Err(format!(
                            "the rack hung, but from three blocks the meat shows on {:.0}% of the rack's middle, against {:.0}% bare (wanted {:.0} points more)",
                            100.0 * meat,
                            100.0 * bare,
                            100.0 * MEAT_SHOWS
                        )),
                        (Ok(()), Ok((bare, meat))) => Ok(format!(
                            "raw rabbit hung on a rack by the fire: the server sent back the hung rack, quicker for the fire; from three blocks meat shows on {:.0}% of its middle, {:.0}% bare",
                            100.0 * meat,
                            100.0 * bare
                        )),
                    }
                };
                gate.check("drying rack", result);
                // The rack goes, and an earth clamp stands in its place with clay to load it.
                let net = world.resource::<Net>();
                net.command(&format!("setblock {} {} {} air", rack.x, rack.y, rack.z));
                net.command(&format!("setblock {} {} {} earth_clamp", rack.x, rack.y, rack.z));
                net.command("give clay 16");
                gate.advance(now);
            }
        }
        4 => {
            let client = world.resource::<ClientWorld>();
            let stood = client.registry.id("base:earth_clamp").is_some_and(|c| client.block(rack) == Some(c));
            if stood && select(world, "base:clay") {
                gate.advance(now);
            } else if waited {
                gate.check("earth clamp", Err(format!("the clamp ({stood}) or clay in hand never came in {DRY_WAIT} server seconds")));
                world.resource::<Net>().command(&format!("setblock {} {} {} air", rack.x, rack.y, rack.z));
                to_phase(gate, 7, now);
            }
        }
        5 => {
            aim_at(world, rack.as_vec3() + Vec3::new(0.5, 0.5, 0.5));
            let client = world.resource::<ClientWorld>();
            let lit = client.registry.id("base:earth_clamp_lit").is_some_and(|c| client.block(rack) == Some(c));
            if lit {
                mouse(world, MouseButton::Right, false);
                gate.advance(now);
            } else if waited {
                mouse(world, MouseButton::Right, false);
                gate.check("earth clamp", Err(format!("right clicks with clay in hand never lit the clamp in {DRY_WAIT} server seconds")));
                world.resource::<Net>().command(&format!("setblock {} {} {} air", rack.x, rack.y, rack.z));
                to_phase(gate, 7, now);
            } else {
                mouse(world, MouseButton::Right, ((now - gate.since) * 2.0).fract() < 0.2);
            }
        }
        6 => {
            aim_at(world, rack.as_vec3() + Vec3::new(0.5, 0.5, 0.5));
            if (drawn(world, rack) || waited) && gate.shot(world, "clamp").is_some() {
                gate.check("earth clamp", Ok("clay loaded into an earth clamp: the server sent back the smouldering clamp".into()));
                world.resource::<Net>().command(&format!("setblock {} {} {} air", rack.x, rack.y, rack.z));
                gate.advance(now);
            }
        }
        // A cooking pot beside the campfire, water sunk in the ground near it, wheat in hand.
        7 => {
            let pot = rack + POT_FROM_RACK;
            let water = pot + POT_WATER;
            let ground = {
                let client = world.resource::<ClientWorld>();
                client.block(water).map(|b| client.registry.def(b).name.clone())
            };
            gate.pot_ground = ground.filter(|name| name != "base:water").map(|name| name.trim_start_matches("base:").to_owned());
            let net = world.resource::<Net>();
            net.command(&format!("setblock {} {} {} cooking_pot", pot.x, pot.y, pot.z));
            net.command(&format!("setblock {} {} {} water", water.x, water.y, water.z));
            // Cooking stays as the Cook step left it (99), past the porridge's 3.
            net.command("give wheat 2");
            net.command(&format!("tp {} {} {}", pot.x as f32 + 0.5, pot.y, pot.z as f32 - 2.5));
            gate.advance(now);
        }
        // Right clicks put the wheat on to simmer: the server sends back the simmering pot.
        8 => {
            let pot = rack + POT_FROM_RACK;
            aim_at(world, pot.as_vec3() + Vec3::new(0.5, 0.5, 0.5));
            let (stood, simmering) = {
                let client = world.resource::<ClientWorld>();
                let is = |name: &str| client.registry.id(name).is_some_and(|b| client.block(pot) == Some(b));
                (is("base:cooking_pot"), is("base:cooking_pot_simmering"))
            };
            if simmering && drawn(world, pot) {
                mouse(world, MouseButton::Right, false);
                if gate.shot(world, "pot").is_none() {
                    return;
                }
                gate.check("cooking pot", Ok("wheat put in a cooking pot beside the campfire, water near, set it simmering (shot `pot`)".into()));
                gate.advance(now);
            } else if waited {
                mouse(world, MouseButton::Right, false);
                let held = holding(world, "base:wheat");
                gate.check("cooking pot", Err(format!("right clicks with wheat never set the pot simmering in {DRY_WAIT} server seconds (pot standing {stood}, wheat carried and first slot {held:?})")));
                gate.advance(now);
            } else if select(world, "base:wheat") {
                mouse(world, MouseButton::Right, ((now - gate.since) * 2.0).fract() < 0.2);
            }
        }
        9 => {
            let pot = rack + POT_FROM_RACK;
            let water = pot + POT_WATER;
            let ground = gate.pot_ground.take().unwrap_or_else(|| "stone".to_owned());
            let net = world.resource::<Net>();
            net.command(&format!("setblock {} {} {} air", pot.x, pot.y, pot.z));
            net.command(&format!("setblock {} {} {} {ground}", water.x, water.y, water.z));
            net.command("clear wheat");
            gate.next(Step::Snare, now);
        }
        _ => {}
    }
}

/// Where the snare step sets its snare, from where the dry step's rack stood.
const SNARE_FROM_RACK: IVec3 = IVec3::new(3, 0, 0);

fn snare(world: &mut World, gate: &mut Gate, now: f32) {
    let snare = gate.feet + IVec3::new(0, 0, 14) + RACK_FROM_FIRE + SNARE_FROM_RACK;
    let waited = gate.server_in_phase() > DRY_WAIT;
    let client = world.resource::<ClientWorld>();
    let is = |name: &str| client.registry.id(name).is_some_and(|b| client.block(snare) == Some(b));
    let (set, sprung) = (is("base:snare"), is("base:snare_sprung"));
    let rabbit = client.registry.id("base:raw_small_game");
    let carried = world.resource::<LocalPlayer>().inventory.slots.iter().flatten().filter(|s| Some(s.item) == rabbit).map(|s| s.count as u32).sum::<u32>();
    let end = |world: &mut World, gate: &mut Gate| {
        gate.tap(world, false, now);
        world.resource::<Net>().command(&format!("setblock {} {} {} air", snare.x, snare.y, snare.z));
        gate.next(Step::Tame, now);
    };
    match gate.phase {
        0 => {
            let net = world.resource::<Net>();
            net.command("clear raw_small_game");
            net.command(&format!("setblock {} {} {} snare", snare.x, snare.y, snare.z));
            net.command(&format!("tp {} {} {}", snare.x as f32 + 0.5, snare.y, snare.z as f32 - 2.5));
            gate.advance(now);
        }
        1 => {
            // Set, by the server's word: a rabbit is put in it.
            if set {
                // `/summon` takes a block cell and stands its first creature a cell and a half
                // west of that cell's middle: one cell east of the snare puts it in the snare.
                world.resource::<Net>().command(&format!("summon rabbit 1 {} {} {}", snare.x + 1, snare.y, snare.z));
                gate.advance(now);
            } else if waited {
                gate.check("snare", Err(format!("the snare never stood in {DRY_WAIT} server seconds")));
                end(world, gate);
            }
        }
        2 => {
            aim_at(world, snare.as_vec3() + Vec3::new(0.5, 0.3, 0.5));
            if sprung && drawn(world, snare) && gate.shot(world, "snare_sprung").is_some() {
                gate.check("snare", Ok("a rabbit summoned into a set snare was caught: the server sprang it".to_owned()));
                gate.advance(now);
            } else if waited {
                gate.check("snare", Err(format!("a rabbit in the snare never sprang it in {DRY_WAIT} server seconds")));
                end(world, gate);
            }
        }
        _ => {
            // Right clicks on the sprung snare, until the server answers one.
            aim_at(world, snare.as_vec3() + Vec3::new(0.5, 0.3, 0.5));
            if set && carried > 0 {
                gate.check("snare taken", Ok(format!("a right click took {carried} raw small game from the snare, set again ({} clicks)", gate.taps)));
                end(world, gate);
            } else if gate.waited_out(DRY_WAIT) {
                gate.check("snare taken", Err(format!("{} right clicks on the sprung snare: set again {set}, raw small game carried {carried}", gate.taps)));
                end(world, gate);
            } else {
                gate.tap(world, true, now);
            }
        }
    }
}

/// Where the dry step looks on its rack: the middle of the strips hung from its twine.
const RACK_MEAT: Vec3 = Vec3::new(0.5, 0.55, 0.5);

/// Frames running the held step's dark box must be drawn before the sword is shot in it:
/// a frame for the new meshes to reach the screen, and a couple more to spare.
const HELD_DRAWN_FRAMES: u32 = 3;

/// Whether the renderer has the column holding `at` as its blocks are now: none of it waits
/// to be meshed, so a shot shows what was just placed there.
fn drawn(world: &World, at: IVec3) -> bool {
    !world.resource::<ClientWorld>().meshing(ColumnPos::of_block(at))
}

/// The share of a shot's middle (a square a tenth of its height across, where the dry
/// step aims) that is meat-coloured: red well over green and blue, raw or dried.
fn meat_share(shot: &Image) -> Result<f32, String> {
    let (w, h, data) = pixels(shot)?;
    let half = h / 20;
    let (mut meat, mut all) = (0usize, 0usize);
    for y in h / 2 - half..h / 2 + half {
        for x in w / 2 - half..w / 2 + half {
            let i = (y * w + x) * 4;
            let [r, g, b] = [data[i], data[i + 1], data[i + 2]].map(f32::from);
            all += 1;
            meat += usize::from(r >= 24.0 && r >= 1.5 * g && r >= 1.5 * b);
        }
    }
    Ok(meat as f32 / all.max(1) as f32)
}

/// Seconds the gate watches a campfire's particles for.
const FIRE_WATCH: f32 = 1.5;

/// Particles a campfire must give off in `FIRE_WATCH` seconds: its smoke, embers and
/// sparks come at about ten a second, about fifteen in that time at any frame rate, so
/// fewer than this means the fire gives off (almost) nothing.
const FIRE_PARTICLES: u32 = 4;

/// Puffs a chimney's top over a hearth must give off: it smokes all the while, not once.
const CHIMNEY_PARTICLES: u32 = 3;
/// Puffs owed (its rate times the seconds watched) by when they must have risen. The count
/// varies less than a Poisson's of that mean, so three or fewer of twenty owed is under one
/// in a hundred thousand: a chimney that falls short gives off less than it says.
const CHIMNEY_OWED: f32 = 20.0;
/// Longest the chimney is watched, in the game's seconds, for one never found or never owed
/// anything.
const CHIMNEY_WAIT: f32 = 12.0;

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
                _ if gate.server_in_phase() > 10.0 => {
                    gate.check("tame", Err("the junglefowl or the wheat never came".into()));
                    gate.next(Step::Party, now);
                }
                _ => {}
            }
        }
        2 => {
            let Some(view) = hen(world) else {
                gate.check("tame", Err("the junglefowl went".into()));
                gate.next(Step::Party, now);
                return;
            };
            let me = world.resource::<crate::creatures::Creatures>().me;
            if view.kept && view.owner == me {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::KeyW, false);
                gate.check("tame", Ok(format!("a junglefowl is ours after {in_phase:.1}s of feeding")));
                gate.advance(now);
            } else if gate.waited_out(25.0) {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::KeyW, false);
                let (wheat, trust) = (item_count(world, "base:wheat"), view.trust);
                let [on, other, none] = gate.tame_aim;
                gate.check("tame", Err(format!(
                    "25 server seconds of feeding wheat did not tame it: trust {trust}%, {wheat} wheat left; the crosshair was on it {on} frames, on another creature {other}, on none {none}; nearest {:.1} blocks",
                    gate.tame_nearest
                )));
                gate.next(Step::Party, now);
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
                gate.tap(world, aimed, now);
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
                gate.advance(now);
            } else if in_phase > 3.0 {
                let detail = format!("aimed at the tamed junglefowl, it read {:?} {:?} with tags {tags:?}", readout.name, readout.level);
                gate.check("readout: kept", Err(detail));
                gate.next(Step::Party, now);
            }
        }
        4 => {
            // Looking closely at it with nothing it eats in hand: a newly tamed one reads in good
            // condition (improving, holding or getting worse).
            let lines: Vec<String> = {
                let locale = world.resource::<crate::lang::Locale>();
                let good = locale.get("condition.good").to_owned();
                ["inspect.kept", "inspect.kept_improving", "inspect.kept_worsening"].map(|key| locale.format(key, &[("state", &good)])).into()
            };
            let replied = world.resource::<crate::hud::console::Console>().since(gate.phase_since).find(|line| lines.iter().any(|l| line.contains(l.as_str()))).map(str::to_owned);
            if let Some(line) = replied {
                mouse(world, MouseButton::Right, false);
                gate.check("condition", Ok(format!("looking closely read \"{line}\" after {in_phase:.1}s")));
                gate.advance(now);
            } else if gate.waited_out(3.0) {
                mouse(world, MouseButton::Right, false);
                gate.check("condition", Err(format!("3 server seconds of looking closely at our junglefowl read none of {lines:?}")));
                gate.next(Step::Party, now);
            } else if let Some(view) = hen(world) {
                // A hotbar slot holding nothing it eats: an empty one first.
                let free = {
                    let launch = world.resource::<Launch>();
                    let species = launch.content.creatures.get(view.species);
                    let eats = |item: BlockId| species.tame_food.contains(&item) || species.breed_food.contains(&item) || launch.content.blocks.def(item).tags.iter().any(|t| t == "breeding_feed");
                    let player = world.resource::<LocalPlayer>();
                    let slots = 0..voxmar_engine::inventory::HOTBAR_SLOTS;
                    slots.clone().find(|&s| player.inventory.get(s).is_none()).or_else(|| slots.into_iter().find(|&s| player.inventory.get(s).is_some_and(|stack| !eats(stack.item))))
                };
                if let Some(slot) = free {
                    world.resource_mut::<LocalPlayer>().selected = slot;
                }
                aim_at(world, view.position + Vec3::Y * 0.3);
                let aimed = world.resource::<crate::interact::Target>().creature == Some(view.id);
                gate.tap(world, aimed, now);
            }
        }
        5 => {
            // Letting it go: Shift and right click on it, twice (the first only asks).
            let Some(view) = hen(world) else {
                key(world, KeyCode::ShiftLeft, false);
                key(world, KeyCode::KeyW, false);
                gate.check("release", Err("the junglefowl went before it was let go".into()));
                gate.next(Step::Party, now);
                return;
            };
            if !view.kept {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::ShiftLeft, false);
                key(world, KeyCode::KeyW, false);
                gate.check("release", Ok(format!("the junglefowl went wild again after {in_phase:.1}s")));
                gate.next(Step::Party, now);
            } else if gate.waited_out(8.0) {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::ShiftLeft, false);
                key(world, KeyCode::KeyW, false);
                gate.check("release", Err("8 server seconds of Shift and right clicks on our junglefowl did not let it go".into()));
                gate.next(Step::Party, now);
            } else {
                // It wanders (up to 7 blocks from home) and reach is 5: walk toward it while far.
                let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 1.8;
                key(world, KeyCode::KeyW, far);
                aim_at(world, view.position + Vec3::Y * 0.3);
                key(world, KeyCode::ShiftLeft, true);
                let aimed = world.resource::<crate::interact::Target>().creature == Some(view.id);
                gate.tap(world, aimed, now);
            }
        }
        _ => {}
    }
}

/// A tinker's camp started on the pad's far side: its trader and hound must reach the client
/// and draw, a right click on the trader gets a greeting, four hides held out are swapped
/// for loaves, and `/wander end` sends the party away.
fn party(world: &mut World, gate: &mut Gate, now: f32) {
    let content = world.resource::<Launch>().content.clone();
    let (Some(trader), Some(hound)) = (content.creatures.find("base:trader"), content.creatures.find("base:hound")) else {
        gate.check("party", Err("no `base:trader` or `base:hound` in the content".into()));
        gate.next(Step::Ore, now);
        return;
    };
    let after = gate.party_after;
    let speaker = |world: &World| {
        let creatures = world.resource::<crate::creatures::Creatures>();
        creatures.views().filter(|v| v.species == trader && v.id > after).max_by_key(|v| v.id).copied()
    };
    let pad = gate.feet + IVec3::new(0, 0, 14);
    // Back on the pad, whatever went wrong.
    let leave = |world: &mut World, gate: &mut Gate| {
        mouse(world, MouseButton::Right, false);
        let net = world.resource::<Net>();
        net.command("wander end");
        net.command(&format!("tp {} {} {}", pad.x as f32 + 0.5, pad.y, pad.z as f32 + 4.5));
        gate.next(Step::Ore, now);
    };
    match gate.phase {
        // Where the creature steps stand, at the pad's near side looking level across it,
        // whatever the steps before left: the party comes to the same place every run.
        0 => {
            look(world, 0.0, 0.0);
            if !gate.party_sent {
                gate.party_sent = true;
                world.resource::<Net>().command(&format!("tp {} {} {}", pad.x as f32 + 0.5, pad.y, pad.z as f32 + 4.5));
                return;
            }
            if gate.server_in_phase() < 1.0 {
                return;
            }
            gate.party_sent = false;
            gate.party_after = world.resource::<crate::creatures::Creatures>().views().map(|v| v.id).max().unwrap_or(0);
            gate.count_before = item_count(world, "base:bread");
            let net = world.resource::<Net>();
            net.command("time set noon");
            net.command("wander end");
            net.command("wander start tinker_camp");
            net.command("give hide 4");
            gate.party_given = now;
            gate.advance(now);
        }
        // The party reaches the client, and is drawn.
        1 => {
            let hounds = world.resource::<crate::creatures::Creatures>().views().filter(|v| v.species == hound && v.id > after).count();
            if let Some(view) = speaker(world)
                && hounds > 0
            {
                let drawn = world.resource::<crate::creatures::Creatures>().alive();
                gate.check("party: arrives", Ok(format!("a trader and {hounds} hound reached the client after {:.1}s ({drawn} creatures drawn)", gate.server_in_phase())));
                // Beside the trader, facing it: tp puts the player on the spot, a few blocks off.
                let at = view.position + Vec3::new(0.0, 0.0, 3.0);
                world.resource::<Net>().command(&format!("tp {} {} {}", at.x, at.y, at.z));
                gate.advance(now);
            } else if gate.server_in_phase() > 20.0 {
                let trader_seen = speaker(world).is_some();
                // What the server answered the commands, a refusal's reason among it.
                let said: Vec<String> = world.resource::<crate::hud::console::Console>().since(gate.phase_since).map(str::to_owned).collect();
                gate.check("party: arrives", Err(format!("/wander start tinker_camp gave the client no trader and hound in 20 server seconds (trader seen {trader_seen}, hounds {hounds}); the server said {said:?}")));
                leave(world, gate);
            }
        }
        // A right click on the trader with nothing to trade: a greeting in chat.
        2 => {
            let Some(view) = speaker(world) else {
                gate.check("party: greeting", Err("the trader went before it was spoken to".into()));
                leave(world, gate);
                return;
            };
            let greetings: Vec<String> = {
                let locale = world.resource::<crate::lang::Locale>();
                ["wander.base.tinker_camp.greet", "wander.base.tinker_camp.greet_2"].map(|key| locale.get(key).to_owned()).into()
            };
            let said = world.resource::<crate::hud::console::Console>().since(gate.phase_since).find(|line| greetings.iter().any(|g| line.contains(g.as_str()))).map(str::to_owned);
            if let Some(line) = said {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::KeyW, false);
                gate.check("party: greeting", Ok(format!("a right click on the trader read \"{line}\" after {:.1}s", gate.server_in_phase())));
                gate.advance(now);
            } else if gate.waited_out(15.0) {
                gate.check("party: greeting", Err(format!("15 server seconds of right clicks on the trader read none of {greetings:?}")));
                leave(world, gate);
            } else {
                // A slot holding no hides (they would be offered as a swap): an empty one first.
                let hide = content.blocks.id("base:hide");
                let free = {
                    let player = world.resource::<LocalPlayer>();
                    let slots = 0..voxmar_engine::inventory::HOTBAR_SLOTS;
                    slots.clone().find(|&s| player.inventory.get(s).is_none()).or_else(|| slots.into_iter().find(|&s| player.inventory.get(s).is_some_and(|stack| Some(stack.item) != hide)))
                };
                if let Some(slot) = free {
                    world.resource_mut::<LocalPlayer>().selected = slot;
                }
                // Until the given hides show in the pack, the slot that looks empty may be where
                // they land: a click then would be a swap, not a greeting.
                let hides_here = item_count(world, "base:hide") >= 4;
                let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 2.5;
                key(world, KeyCode::KeyW, far);
                aim_at(world, view.position + Vec3::Y * 0.9);
                let aimed = world.resource::<crate::interact::Target>().creature == Some(view.id);
                gate.tap(world, hides_here && aimed, now);
            }
        }
        // The same click with four hides in hand: the party's trade, loaves for them.
        3 => {
            let Some(view) = speaker(world) else {
                gate.check("party: trade", Err("the trader went before it was traded with".into()));
                leave(world, gate);
                return;
            };
            let bread = item_count(world, "base:bread");
            let swap = world.resource::<crate::lang::Locale>().get("wander.base.tinker_camp.swap_hide").to_owned();
            // Spoken since the hides were given, whichever stage's click it answered.
            let said = world.resource::<crate::hud::console::Console>().since(gate.party_given).any(|line| line.contains(swap.as_str()));
            if said && bread > gate.count_before && item_count(world, "base:hide") < 4 {
                mouse(world, MouseButton::Right, false);
                key(world, KeyCode::KeyW, false);
                gate.check("party: trade", Ok(format!("four hides held out for the trader came back as {} loaves after {:.1}s", bread - gate.count_before, gate.server_in_phase())));
                gate.advance(now);
            } else if gate.waited_out(15.0) {
                gate.check("party: trade", Err(format!("15 server seconds with four hides in hand got no swap from the trader: the line spoken {said}, loaves {} (had {}), hides held {}", bread, gate.count_before, item_count(world, "base:hide"))));
                leave(world, gate);
            } else if let Some(hide) = content.blocks.id("base:hide")
                && let Some(slot) = in_hotbar(world, &mut gate.food_moved, hide)
            {
                world.resource_mut::<LocalPlayer>().selected = slot;
                let far = world.resource::<LocalPlayer>().position.xz().distance(view.position.xz()) > 2.5;
                key(world, KeyCode::KeyW, far);
                aim_at(world, view.position + Vec3::Y * 0.9);
                let aimed = world.resource::<crate::interact::Target>().creature == Some(view.id);
                gate.tap(world, aimed, now);
            }
        }
        // Sent away: the party goes from the client.
        4 => {
            if !gate.party_sent {
                gate.party_sent = true;
                world.resource::<Net>().command("wander end");
                return;
            }
            let (party, hounds) = {
                let creatures = world.resource::<crate::creatures::Creatures>();
                (creatures.views().filter(|v| v.species == trader && v.id > after).count(), creatures.views().filter(|v| v.species == hound && v.id > after).count())
            };
            if party + hounds == 0 {
                gate.check("party: ends", Ok(format!("/wander end took the party from the client after {:.1}s", gate.server_in_phase())));
                leave(world, gate);
            } else if gate.server_in_phase() > 10.0 {
                gate.check("party: ends", Err(format!("10 server seconds after /wander end, {party} traders and {hounds} hounds of the party were still drawn")));
                leave(world, gate);
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
            if !open_scene_pose(world, gate) {
                if gate.server_in_phase() > 15.0 {
                    gate.check("sky: pose", Err(unsettled(world, gate)));
                    gate.next(Step::Tree, now);
                }
                return;
            }
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
            let horizon = horizon_on_screen(world);
            gate.check("sky: day", gradient(&shot, horizon));
            // Up above the top of the world, as a flying player can go: by teleport, for the
            // server refuses a move that jumps hundreds of blocks in one step.
            let feet = gate.feet;
            let mut player = world.resource_mut::<LocalPlayer>();
            player.flying = true;
            player.pitch = 0.2;
            world.resource::<Net>().command(&format!("tp {} {ABOVE_THE_WORLD} {}", feet.x as f32 + 0.5, feet.z as f32 + 0.5));
            gate.advance(now);
        }
        // Flying until the teleport lands: the player's own movement stops flying on a step
        // that meets the ground, which a server running behind leaves time for before it
        // moves them, and then they fall from the top of the world.
        3 if world.resource::<LocalPlayer>().position.y <= ABOVE_THE_WORLD - 1.0 => world.resource_mut::<LocalPlayer>().flying = true,
        3 if in_phase > 0.5 => {
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
        1 if gate.server_in_phase() > 15.0 => {
            let why = if cloudless { format!("the time of day did not change: {ticks:.0} ticks") } else { "the sky did not clear of cloud".to_owned() };
            gate.check("sky", Err(why));
            gate.next(Step::Tree, now);
        }
        4 | 7 if gate.server_in_phase() > 5.0 => {
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
/// How long the server is given to send the world around the settlement, in its seconds.
const SETTLEMENT_LOAD: f32 = 120.0;
/// Server seconds a settlement's people have to reach a player standing in its square.
const SETTLEMENT_PEOPLE: f32 = 20.0;

/// `/locate settlement` names the nearest hamlet or village; the player and the observer go
/// there, and once the server's world around it has loaded, its blocks are judged against
/// the plan worldgen gives for it (`Overworld::settlements_near`): its well stands on the
/// square, every door within reach is open with a floor under it and space behind it, and
/// no house holds water, and its people come to the player and draw. Then the player goes
/// back to the test ground.
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
            gate.settlement_reply = reply.clone();
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
                if gate.server_in_phase() > SETTLEMENT_LOAD {
                    let n = columns.len();
                    gate.check("settlements", Err(format!("the world around the settlement at {at} did not load: the server sent {held} of its {n} columns in {SETTLEMENT_LOAD} server seconds")));
                    gate.next(Step::Recipes, now);
                }
                return;
            }
            let result = judge_settlement(world, gate, at);
            gate.check("settlements", result);
            gate.advance(now);
        }
        // Its people: the server brings a settlement to life when a player is near, and they
        // reach the client and draw. Then the player goes back to the test ground.
        3 => {
            let content = world.resource::<Launch>().content.clone();
            let people = world.resource::<crate::creatures::Creatures>().views().filter(|v| content.creatures.get(v.species).def.kind == voxmar_engine::creature::Kind::Villager).count();
            if people == 0 && gate.server_in_phase() < SETTLEMENT_PEOPLE {
                return;
            }
            gate.check("settlement people", if people > 0 { Ok(format!("{people} people of the settlement drawn")) } else { Err("no one of the settlement came to the player standing in its square".into()) });
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
    // The server names its culture: the family's title, as worldgen plans it.
    let culture = &plan.culture;
    if !gate.settlement_reply.contains(&culture.title.0) {
        problems.push(format!("/locate settlement does not name its culture ({}): {}", culture.title.0, gate.settlement_reply));
    }
    if culture.axes.iter().any(|a| !(0.0..=1.0).contains(a)) {
        problems.push(format!("its culture's axes are out of range: {:?}", culture.axes));
    }
    if problems.is_empty() {
        Ok(format!("{name}, a {} in {} country at {at}: a well on the square, {doors} doors open onto floors, {houses} buildings dry", plan.size, culture.title.0))
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
                if gate.server_in_phase() > 20.0 {
                    gate.check("gorge", Err("no answer to /locate gorge".into()));
                    gate.next(Step::Tarns, now);
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
                gate.next(Step::Tarns, now);
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
                    gate.next(Step::Tarns, now);
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
            gate.next(Step::Tarns, now);
        }
        _ => {}
    }
}

/// Saved positions and flight state for the excursion from the gorge to a generated tarn.
struct TarnVisit {
    at: IVec3,
    player: Vec3,
    observer: Vec3,
    was_flying: bool,
    return_teleports: (u32, u32),
}

/// `/tp` centres integer-looking horizontal coordinates on a block. Retain the saved
/// f32's full decimal precision and spell integral positions with a fractional part.
fn return_teleport(at: Vec3) -> String {
    let horizontal = |value: f32| {
        let word = value.to_string();
        if word.contains('.') { word } else { format!("{word}.0") }
    };
    format!("tp {} {} {}", horizontal(at.x), at.y, horizontal(at.z))
}

/// Visits a real pool filled by the tarn rule. Both peers must receive at least twenty
/// matching source-water surface columns, then the loaded pool is captured from above.
/// The generator chooses the place; no hand-built pool can satisfy this step. Both peers
/// return to the gorge before the next lookup, including after a failed pool load.
fn tarns(world: &mut World, gate: &mut Gate, now: f32, in_phase: f32) {
    match gate.phase {
        0 => {
            let generator = world.resource::<crate::render::lod::WorldGenerator>().get().cloned();
            let at = generator.and_then(|g| {
                // The first highland can lie below the tarn floor. Try a bounded spread
                // of alpine districts, so the gate visits suitable high ground in this world.
                [IVec2::ZERO, IVec2::new(24000, 8000), IVec2::new(-8000, 24000), IVec2::new(-24000, -8000), IVec2::new(8000, -24000)].into_iter().find_map(|offset| {
                    let alpine = g.locate_biome("alpine_meadow", gate.feet.xz() + offset, 60000)?;
                    g.locate_tarn(alpine, 4096)
                })
            });
            let Some(at) = at else {
                gate.check("tarns", Err("no visible generated tarn in the sampled alpine highlands".into()));
                gate.next(Step::Settlements, now);
                return;
            };
            let Some(observer) = gate.observer.as_ref().and_then(|o| o.position) else {
                gate.check("tarns", Err("no observer position to return to after the visit".into()));
                gate.next(Step::Settlements, now);
                return;
            };
            let player = world.resource::<LocalPlayer>();
            gate.tarn = Some(TarnVisit { at, player: player.position, observer, was_flying: player.flying, return_teleports: (0, 0) });
            world.resource_mut::<LocalPlayer>().flying = true;
            world.resource::<Net>().command(&format!("tp {} {} {}", at.x, at.y + 18, at.z + 10));
            if let Some(observer) = gate.observer.as_mut() {
                observer.connection.send(ClientMessage::Command { line: format!("tp {} {} {}", at.x, at.y + 18, at.z) });
            }
            gate.advance(now);
        }
        1 => {
            let at = gate.tarn.as_ref().expect("tarn visit").at;
            let there = world.resource::<LocalPlayer>().position.xz().distance((at.xz() + IVec2::new(0, 10)).as_vec2()) < 3.0;
            let held = gate.observer.as_ref().is_some_and(|o| columns_around(at).into_iter().all(|c| o.world.contains(c)));
            let client = world.resource::<ClientWorld>();
            let shown = columns_around(at).into_iter().all(|c| client.voxels.contains(c)) && settled(world, at);
            if !(there && held && shown) {
                if gate.server_in_phase() > SETTLEMENT_LOAD {
                    gate.check("tarns", Err(format!("the pool at {at} did not load on both peers (there {there}, server {held}, drawn {shown})")));
                    // A failed visit must return too: the next lookup searches from the
                    // observer, and letting go of flight here would drop the player.
                    gate.phase = 2;
                    gate.advance(now);
                }
                return;
            }
            aim(world, at);
            gate.advance(now);
        }
        2 if in_phase > SETTLE => {
            let at = gate.tarn.as_ref().expect("tarn visit").at;
            let client = world.resource::<ClientWorld>();
            let Some(observer) = gate.observer.as_ref() else { return };
            let source = |p: IVec3| observer.world.block(p).is_some_and(|b| client.registry.fluid(b).is_some_and(|f| f.source))
                && observer.world.block(p + IVec3::Y).is_some_and(|b| client.registry.fluid(b).is_none());
            let frozen = |p: IVec3| observer.world.block(p).is_some_and(|b| client.registry.def(b).has_tag("ice"));
            let Some(generator) = world.resource::<crate::render::lod::WorldGenerator>().get() else { return };
            let mut water = 0;
            let mut disagree = 0;
            let mut seen = std::collections::HashSet::new();
            let mut todo = vec![at];
            while let Some(p) = todo.pop() {
                if (p.xz() - at.xz()).abs().max_element() > 16 || !seen.insert(p) {
                    continue;
                }
                let site = generator.site(p.x, p.z);
                if site.body != voxmar_worldgen::Body::Lake || site.height >= site.water || site.water != at.y || !(source(p) || frozen(p)) {
                    continue;
                }
                water += 1;
                disagree += usize::from(client.block(p) != observer.world.block(p));
                todo.extend([p + IVec3::X, p - IVec3::X, p + IVec3::Z, p - IVec3::Z]);
            }
            let result = if water >= 20 && disagree == 0 {
                Ok(format!("{water} connected still water or ice surface columns at {at}, matching on both peers"))
            } else {
                Err(format!("{water} source-water columns at {at}, {disagree} differ on the client"))
            };
            if gate.shot(world, "tarn").is_none() {
                return;
            }
            gate.check("tarns", result);
            gate.advance(now);
        }
        3 => {
            let visit = gate.tarn.as_mut().expect("tarn visit");
            let player = world.resource::<LocalPlayer>();
            visit.return_teleports.0 = player.teleports;
            let at = visit.player;
            world.resource::<Net>().command(&return_teleport(at));
            if let Some(observer) = gate.observer.as_mut() {
                visit.return_teleports.1 = observer.teleports;
                let at = visit.observer;
                observer.connection.send(ClientMessage::Command { line: return_teleport(at) });
            }
            gate.advance(now);
        }
        4 => {
            let visit = gate.tarn.as_ref().expect("tarn visit");
            let player = world.resource::<LocalPlayer>();
            let there = player.teleports != visit.return_teleports.0 && player.position.distance(visit.player) < 3.0;
            let observer_there = gate.observer.as_ref().is_some_and(|o| {
                o.teleports != visit.return_teleports.1 && o.position.is_some_and(|p| p.distance(visit.observer) < 3.0)
            });
            let held = gate.observer.as_ref().is_some_and(|o| columns_around(visit.observer.floor().as_ivec3()).into_iter().all(|c| o.world.contains(c)));
            let at = visit.player.floor().as_ivec3();
            let client = world.resource::<ClientWorld>();
            let shown = columns_around(at).into_iter().all(|c| client.voxels.contains(c)) && settled(world, at);
            if there && observer_there && held && shown {
                let (player_at, observer_at, was_flying) = (visit.player, visit.observer, visit.was_flying);
                gate.check("tarns: back", Ok(format!("both peers returned before the settlement search: player {player_at}, observer {observer_at}")));
                world.resource_mut::<LocalPlayer>().flying = was_flying;
                gate.tarn = None;
                gate.next(Step::Settlements, now);
            } else if gate.server_in_phase() > SETTLEMENT_LOAD {
                gate.check("tarns: back", Err(format!("the peers never returned from the tarn (player {there}, observer {observer_there}, server {held}, drawn {shown})")));
                // Do not run subsequent fixtures at the remote pool or drop an airborne
                // player: the failed restoration ends this session.
                gate.next(Step::Done, now);
            }
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
                if gate.server_in_phase() > 20.0 {
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

/// A day sky darkens from the horizon upwards: in a shot aimed at the morning sun, the clear
/// sky at the top differs from the clear sky a little above the horizon (`horizon`, how far
/// down the shot it is drawn). Only clear sky is compared, by its median colour: given the
/// whole of each band, a cloud over the top of the frame, or the sea and land below the
/// horizon, decided it (on 2026-10-08 a cloud over the top read within 6 levels of the sea
/// and the sky was called flat).
fn gradient(shot: &Image, horizon: Option<f32>) -> Result<String, String> {
    let (w, h, data) = pixels(shot)?;
    sky_gradient(w, h, data, horizon)
}

/// How much bluer than red a pixel must be to count as clear sky: the morning sky is 60 to
/// 90 levels bluer, while clouds, the sun's glow and hills in the haze are at most about 30.
const CLEAR_SKY: i32 = 40;
/// Least levels the clear sky's top and its colour just above the horizon differ by (the
/// gate's morning sky: about 31).
const SKY_GRADIENT: f32 = 15.0;

fn sky_gradient(w: usize, h: usize, data: &[u8], horizon: Option<f32>) -> Result<String, String> {
    let top = (0, h * 12 / 100);
    // From a tenth of the shot above the horizon to just above its haze; half way down the
    // shot when where the horizon is drawn is not known.
    let horizon = horizon.map_or(h / 2, |y| (y.max(0.0) as usize).min(h));
    let low = (horizon.saturating_sub(h / 10).max(top.1), horizon.saturating_sub(h * 3 / 100).max(top.1));
    let clear = |(y0, y1): (usize, usize)| {
        let mut channels: [Vec<u8>; 3] = default();
        for y in y0..y1 {
            for x in 0..w {
                let pixel = &data[(y * w + x) * 4..][..3];
                let (r, g, b) = (pixel[0] as i32, pixel[1] as i32, pixel[2] as i32);
                if b >= r + CLEAR_SKY && b >= g && b >= 120 {
                    for (channel, &value) in channels.iter_mut().zip(pixel) {
                        channel.push(value);
                    }
                }
            }
        }
        let share = channels[0].len() as f32 / ((y1 - y0) * w).max(1) as f32;
        let median = channels.map(|mut values| {
            values.sort_unstable();
            values.get(values.len() / 2).copied().unwrap_or(0) as f32
        });
        (median, share)
    };
    let ((high, high_share), (low_colour, low_share)) = (clear(top), clear(low));
    let percent = |share: f32| share * 100.0;
    if high_share < 0.02 || low_share < 0.02 {
        return Err(format!(
            "too little clear sky to judge: {:.0}% of the top, {:.0}% above the horizon (at {horizon} of {h})",
            percent(high_share),
            percent(low_share)
        ));
    }
    let difference = (0..3).map(|c| (high[c] - low_colour[c]).abs()).fold(0.0, f32::max);
    let seen = format!("clear sky in {:.0}% of the top and {:.0}% above the horizon (at {horizon} of {h})", percent(high_share), percent(low_share));
    if difference >= SKY_GRADIENT {
        Ok(format!("the clear sky's top differs from lower down by {difference:.0} levels; {seen}"))
    } else {
        Err(format!("the sky is flat: its clear top and lower down differ by {difference:.0} levels; {seen}"))
    }
}

/// How far down the shot the horizon is drawn, in pixels: where a level look straight ahead
/// meets the screen.
fn horizon_on_screen(world: &mut World) -> Option<f32> {
    let mut cameras = world.query_filtered::<(&Camera, &GlobalTransform), With<Camera3d>>();
    let (camera, transform) = cameras.single(world).ok()?;
    let ahead = (transform.forward().as_vec3() * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    camera.world_to_viewport(transform, transform.translation() + ahead * 1000.0).ok().map(|at| at.y)
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
/// max(`HITCH_MS`, `HITCH_FACTOR` × the session's median frame), times how far the load
/// now (`Gate::load_now`) asks for more than the cores.
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
    let (load, cores) = (gate.load_now(), std::thread::available_parallelism().map_or(1, |n| n.get()) as f64);
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

/// How far back `Gate::load_now` reads the load average, and the least span it judges a
/// rise over.
const LOAD_SPAN: f32 = 30.0;
const LOAD_RISE_SPAN: f32 = 10.0;

impl Gate {
    /// The machine's load now (runnable threads): the one-minute average, or more while it
    /// is rising. The average moves a part `1 - e^(-t/60)` of the way to the load over `t`
    /// seconds, so from two reads `t` apart the load between them was `(L1 - L0 e) / (1 - e)`
    /// with `e = e^(-t/60)`. At the gate's start, as its test binaries come on together, the
    /// average lags far behind: 28 read with the cores asked for about 45, and the smooth
    /// bars stood 1.6x where the frames were stretched by more.
    fn load_now(&self) -> f64 {
        let Some(&(now, latest)) = self.loads.back() else { return load_average() };
        let rising = self.loads.front().filter(|&&(at, _)| now - at >= LOAD_RISE_SPAN).map_or(0.0, |&(at, before)| load_between(before, latest, now - at));
        latest.max(rising)
    }
}

/// The load over the `seconds` between two reads of the one-minute average (see
/// `Gate::load_now`).
fn load_between(before: f64, latest: f64, seconds: f32) -> f64 {
    let e = (-f64::from(seconds) / 60.0).exp();
    (latest - before * e) / (1.0 - e)
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

/// Whether an edit of `cells` has settled, `since` seconds after it was made (the phase's
/// start): both client and server show each as `wanted` and nothing around waits to be
/// meshed, or the server has had its limit (and the checks will say what is missing).
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
    (server && agree && shown(&|p| client.block(p)) && settled(world, gate.feet)) || gate.server_in_phase() > SETTLE_LIMIT
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

/// Mean change between two shots of the same size, in levels of 255, within `area` (x from,
/// x to, y from, y to, as fractions of the frame, y down).
fn change_in(a: &Image, b: &Image, area: [f32; 4]) -> f32 {
    let (Ok((w, h, da)), Ok((wb, hb, db))) = (pixels(a), pixels(b)) else { return 0.0 };
    if (w, h) != (wb, hb) {
        return 0.0;
    }
    let [x0, x1, y0, y1] = area;
    let (x0, x1, y0, y1) = ((w as f32 * x0) as usize, (w as f32 * x1) as usize, (h as f32 * y0) as usize, (h as f32 * y1) as usize);
    let (mut sum, mut n) = (0u64, 0u64);
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
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

/// Writes a shot to `path` as a PNG, as `save_to_disk` would (without the alpha channel,
/// which holds brightness when HDR is on).
fn save_shot(image: Image, path: &std::path::Path) {
    match image.try_into_dynamic() {
        Ok(image) => match image.to_rgb8().save_with_format(path, image::ImageFormat::Png) {
            Ok(()) => info!("Screenshot saved to {}", path.display()),
            Err(e) => error!("Cannot save screenshot, IO error: {e}"),
        },
        Err(e) => error!("Cannot save screenshot, requested format not recognized: {e}"),
    }
}

/// Writes the report and exits, with an error if any check failed.
fn finish(world: &mut World, gate: &Gate, now: f32) {
    // Every shot on disk before the session ends.
    for save in std::mem::take(&mut *gate.saving.lock().expect("gate saves")) {
        if save.join().is_err() {
            error!("Cannot save screenshot: its thread panicked");
        }
    }
    let verdict = if gate.failures.is_empty() { "passed" } else { "FAILED" };
    let mut report = format!("gate {verdict}: {} checks passed, {} failed, in {now:.1}s\n", gate.passes, gate.failures.len());
    for failure in &gate.failures {
        report += &format!("FAILED {failure}\n");
    }
    if let Err(e) = std::fs::write(gate.dir.join("report.txt"), &report) {
        error!("cannot write the gate report: {e}");
    }
    // A server that stopped may never stop for good either: leaving would wait on its
    // thread for ever (`ServerHandle`'s drop joins it), so the session ends here.
    if gate.server_stopped {
        error!("{}", report.trim_end());
        std::process::exit(1);
    }
    if gate.failures.is_empty() {
        info!("{}", report.trim_end());
        world.write_message(AppExit::Success);
    } else {
        error!("{}", report.trim_end());
        world.write_message(AppExit::error());
    }
}

#[cfg(test)]
mod tests {
    use super::{sling_next, warm_window, SlingNext, BEAR, SLING, SLING_LEVEL, SLING_STONES, UNANSWERED};

    /// The window is judged by what the candle adds to its pane, so a brighter night (a
    /// full moon lighting the pane too) changes nothing; a pane the candle leaves as it was,
    /// or brightens blue, is not a lit window.
    #[test]
    fn a_lit_window_is_judged_against_itself_snuffed() {
        // The T5 pane, lit, against a dark night and a moonlit one lighting it evenly.
        let lit = [77.0, 44.0, 16.0];
        assert!(warm_window(lit, [14.0, 15.0, 19.0]).is_ok());
        let moon = 12.0;
        assert!(warm_window(lit.map(|c| c + moon), [14.0 + moon, 15.0 + moon, 19.0 + moon]).is_ok(), "a brighter moon cancels");
        assert!(warm_window(lit, lit).is_err(), "the candle added nothing");
        assert!(warm_window([30.0, 40.0, 70.0], [10.0, 10.0, 12.0]).is_err(), "bright but blue");
    }

    /// The natural spawns step reads the populate's count from its reply as the chat shows
    /// it, in English and in Japanese, and only a reply since the phase began.
    #[test]
    fn a_populate_is_counted_from_its_reply() {
        use bevy::prelude::World;
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for code in ["en", "ja"] {
            let mut world = World::new();
            let locale = crate::lang::Locale::load(&content, code);
            let reply = locale.format("command.creatures.populated", &[("count", &17)]);
            world.insert_resource(locale);
            let mut console = crate::hud::console::Console::default();
            console.push(voxmar_protocol::ChatKind::Chat, "an earlier line", 1.0);
            world.insert_resource(console);
            assert_eq!(super::populated(&world, 0.0), None, "{code}: not yet told");
            world.resource_mut::<crate::hud::console::Console>().push(voxmar_protocol::ChatKind::Chat, &reply, 2.0);
            assert_eq!(super::populated(&world, 1.5), Some(17), "{code}: {reply}");
            assert_eq!(super::populated(&world, 2.5), None, "{code}: told before the phase");
        }
    }

    /// At the sling step's Ranged level, against its bear, every stone missing is a broken
    /// shot, not a bad run of rolls: under one gate in ten million.
    #[test]
    fn twelve_misses_are_never_luck() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let bear = &content.creatures.get(content.creatures.find(BEAR).unwrap()).def;
        let sling = content.combat.weapon(content.blocks.expect(SLING)).unwrap();
        let defence = voxmar_engine::combat::defence_against(bear.defence, bear.weak, sling.style);
        let hits = voxmar_engine::combat::hit_chance(SLING_LEVEL, sling.accuracy, defence, 0);
        let all_miss = (1.0 - hits as f64).powi(SLING_STONES as i32);
        assert!(all_miss < 1e-7, "a stone hits {hits:.2} of the time: all {SLING_STONES} miss {all_miss:.1e} of the time");
    }

    #[test]
    fn the_sling_step_never_fails_before_a_shot_and_judges_misses_by_shots_answered() {
        // The first frame of shooting: however long the stage has stood, it shoots.
        assert_eq!(sling_next(0, 0, 0, 0, SLING_STONES, 0.0, None), SlingNext::Shoot);
        assert_eq!(sling_next(0, 0, 0, 0, SLING_STONES, UNANSWERED * 10.0, None), SlingNext::Shoot);
        // A shot on its way waits, then fails only once unanswered past UNANSWERED.
        assert_eq!(sling_next(1, 0, 0, 0, SLING_STONES - 1, 1.0, None), SlingNext::Wait);
        assert!(matches!(sling_next(1, 0, 0, 0, SLING_STONES - 1, UNANSWERED + 1.0, None), SlingNext::Fail(_)));
        // Answered misses shoot again, until every stone is spent.
        assert_eq!(sling_next(3, 3, 0, 0, SLING_STONES - 3, 0.5, None), SlingNext::Shoot);
        assert!(matches!(sling_next(SLING_STONES, SLING_STONES, 0, 0, 0, 0.5, None), SlingNext::Fail(_)));
        // A hit needs the landing and the experience both.
        assert_eq!(sling_next(2, 2, 1, 20, SLING_STONES - 2, 0.0, None), SlingNext::Hit);
        assert_eq!(sling_next(2, 2, 1, 0, SLING_STONES - 2, 0.0, None), SlingNext::Shoot);
        // The crosshair on a block instead of the bear: a failure naming it, before the
        // first shot or between shots, never a wait for an answer that cannot come; a shot
        // already on its way is still waited for, and a hit is still a hit.
        let stone = Some("base:stone at [-1104, 158, 1160]");
        assert!(matches!(sling_next(0, 0, 0, 0, SLING_STONES, 0.0, stone), SlingNext::Fail(why) if why.contains("base:stone")));
        assert!(matches!(sling_next(2, 2, 0, 0, SLING_STONES - 2, 0.0, stone), SlingNext::Fail(_)));
        assert_eq!(sling_next(1, 0, 0, 0, SLING_STONES - 1, 1.0, stone), SlingNext::Wait);
        assert_eq!(sling_next(2, 2, 1, 20, SLING_STONES - 2, 0.0, stone), SlingNext::Hit);
    }

    /// The line from the eye to the bear passes through each cell between once, in order:
    /// the cell right before the eye (where the Mac's sling step found stone) among them.
    #[test]
    fn the_line_to_the_bear_names_every_cell_it_crosses() {
        let eye = Vec3::new(-1103.5, 158.62, 1159.5);
        let bear = Vec3::new(-1103.5, 157.6, 1167.5);
        let cells = cells_between(eye, bear);
        assert_eq!(cells.first(), Some(&IVec3::new(-1104, 158, 1159)));
        assert_eq!(cells.last(), Some(&IVec3::new(-1104, 157, 1167)));
        assert!(cells.contains(&IVec3::new(-1104, 158, 1160)), "{cells:?}");
        assert!(cells.windows(2).all(|w| w[0] != w[1] && (w[1] - w[0]).abs().max_element() == 1), "{cells:?}");
        for z in 1159..=1167 {
            assert!(cells.iter().any(|c| c.z == z), "every cell along the way: {cells:?}");
        }
        // The lines to the bear's feet, middle and top: the look step's wall on the pad is
        // crossed at both heights the server's line to its centre or the crosshair may take.
        let lines = shot_lines(eye, bear - Vec3::Y * 0.6);
        for y in 157..=158 {
            assert!(lines.contains(&IVec3::new(-1104, y, 1166)), "the wall at height {y}: {lines:?}");
        }
        assert!(cells.iter().all(|c| lines.contains(c)), "the crosshair's line is among them");
    }

    use super::*;

    /// A shot of grey pixels, `grey(x, y)` each.
    fn grey_shot(w: u32, h: u32, grey: impl Fn(u32, u32) -> u8) -> Image {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend([grey(x, y), grey(x, y), grey(x, y), 255]);
            }
        }
        Image::new(
            bevy::render::render_resource::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            bevy::render::render_resource::TextureDimension::D2,
            data,
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            default(),
        )
    }

    /// The load a rising one-minute average comes from: the gate's start, its test binaries
    /// coming on together over a quiet machine, the average a second at a time as the
    /// kernel moves it.
    #[test]
    fn the_load_now_is_read_through_a_rising_average() {
        let (quiet, busy) = (7.5, 60.0);
        let average = |t: f32| busy - (busy - quiet) * (-f64::from(t) / 60.0).exp();
        // 32 s in, the average has come only to 29.
        assert!((average(32.0) - 29.0).abs() < 0.5);
        let estimate = load_between(average(12.0), average(32.0), 20.0);
        assert!((estimate - busy).abs() < 1e-6, "read {estimate}");
        // Steady, and falling: no more than the average says.
        assert!((load_between(20.0, 20.0, 20.0) - 20.0).abs() < 1e-9);
        assert!(load_between(30.0, 25.0, 20.0) < 25.0);
    }

    #[test]
    fn the_hand_check_sees_only_the_lower_right() {
        let empty = grey_shot(100, 100, |_, _| 100);
        // An item in the hand's corner: 40 levels brighter over 30 by 30 pixels there.
        let held = grey_shot(100, 100, |x, y| if x >= 70 && y >= 70 { 140 } else { 100 });
        let seen = change_in(&empty, &held, HAND_AREA);
        assert!((seen - 40.0 * 900.0 / 2750.0).abs() < 1e-3, "{seen}");
        assert!(seen >= HELD_SEEN, "{seen}");
        assert_eq!(change_in(&empty, &held, OUT_OF_HAND), 0.0);
        // A shot of another size is no comparison.
        assert_eq!(change_in(&empty, &grey_shot(50, 50, |_, _| 0), HAND_AREA), 0.0);
    }

    #[test]
    fn a_striking_step_waits_on_strikes_asked_not_presses() {
        let mut gate = Gate::new(std::env::temp_dir());
        // Earlier steps' strikes are not this one's.
        assert!(!gate.unanswered(40));
        // Presses that asked nothing (nothing in reach as the client saw it), however long
        // the server runs: no strike was shown it.
        gate.server_now = 100.0;
        assert!(!gate.unanswered(40));
        // Three asked: unanswered only once the server has run past `UNANSWERED` since the
        // first went.
        gate.server_now = 101.0;
        assert!(!gate.unanswered(41));
        gate.server_now = 101.0 + UNANSWERED;
        assert!(!gate.unanswered(43), "{} s since the first", UNANSWERED);
        gate.server_now = 101.5 + UNANSWERED;
        assert!(gate.unanswered(43));
        assert_eq!(gate.unanswered_strikes(43), 3);
        // An answer counts afresh from there.
        gate.strike_answered(43);
        assert!(!gate.unanswered(46));
        gate.server_now = 200.0;
        assert!(gate.unanswered(46));
    }

    #[test]
    fn swings_and_clicks_go_one_a_frame_however_long_the_frames() {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        let mut gate = Gate::new(std::env::temp_dir());
        // Frames a second apart, as a starved client draws them: every other frame presses,
        // the one after lets go, and each press counts.
        let mut pressed = 0;
        for frame in 0..6 {
            gate.swing(app.world_mut(), frame as f32);
            app.update();
            pressed += app.world().resource::<ButtonInput<MouseButton>>().just_pressed(MouseButton::Left) as u32;
        }
        assert_eq!((pressed, gate.swings), (3, 3));
        // Frames a few milliseconds apart: one a full charge.
        let mut gate = Gate::new(std::env::temp_dir());
        for frame in 0..100 {
            gate.swing(app.world_mut(), frame as f32 * 0.005);
        }
        assert_eq!(gate.swings, 1, "a swing a full charge after the last");
        // Right clicks wanted but not aimed press nothing.
        let mut gate = Gate::new(std::env::temp_dir());
        for frame in 0..6 {
            gate.tap(app.world_mut(), frame >= 2, frame as f32);
        }
        assert_eq!(gate.taps, 2, "pressed at frames 2 and 4");
        // A starved client that sent fewer than three clicks has not waited out its phase,
        // however many server seconds passed.
        gate.server_now = 100.0;
        assert!(!gate.waited_out(15.0));
        gate.taps = UNANSWERED_SWINGS;
        assert!(gate.waited_out(15.0));
    }

    #[test]
    fn an_already_received_blow_never_queues_a_late_native_defence_edge() {
        for blocking in [false, true] {
            for received in [false, true] {
                let mut app = App::new();
                app.add_plugins(bevy::input::InputPlugin);
                let mut feel = FeelRun { seen: Some(Instant::now()), after: 0.0, ..default() };
                let dodge = KeyCode::ShiftLeft;
                let queued = feel.answer(app.world_mut(), blocking, dodge, received);
                if received {
                    // Last releases the controls when judging Hurt. Press then release
                    // in that same update must never leak just_pressed into the next input frame.
                    key(app.world_mut(), KeyCode::KeyW, false);
                    key(app.world_mut(), dodge, false);
                    mouse(app.world_mut(), MouseButton::Right, false);
                }
                app.update();
                let keys = app.world().resource::<ButtonInput<KeyCode>>();
                let buttons = app.world().resource::<ButtonInput<MouseButton>>();
                assert_eq!(keys.just_pressed(dodge), !received && !blocking);
                assert_eq!(keys.pressed(KeyCode::KeyW), !received && !blocking);
                assert_eq!(buttons.just_pressed(MouseButton::Right), !received && blocking);
                assert_eq!(queued, !received);
                assert_eq!(feel.answered, !received, "a crossed threshold is not an answer to an already received blow");
                if !received {
                    assert!(!feel.answer(app.world_mut(), blocking, dodge, false), "an already queued response is sent once");
                }
            }
        }
    }

    #[test]
    fn a_blow_heard_before_any_frame_could_answer_it_is_set_aside() {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        let dodge = KeyCode::ShiftLeft;
        // A wind-up seen, and the result heard at the next frame, before the moment tried
        // (0.6 s): no frame could have pressed.
        let mut feel = FeelRun { after: 0.6, calm: true, ..default() };
        assert!(feel.observe(true, false));
        assert!(!feel.answer(app.world_mut(), true, dodge, true));
        assert!(feel.could_not_answer(false), "no frame came between the wind-up and its result");
        // A frame before the moment came first, with the blow still to come: a press could
        // have gone there, so the result after it says the moment was late, and counts.
        let mut feel = FeelRun { after: 0.6, calm: true, ..default() };
        assert!(feel.observe(true, false));
        assert!(!feel.answer(app.world_mut(), true, dodge, false));
        assert!(!feel.answer(app.world_mut(), true, dodge, true));
        assert!(!feel.could_not_answer(false), "a frame before the moment could have answered");
        // The moment passed between that frame and the next, which heard the result: the
        // press would have gone in the next, after the blow, so this one is set aside too.
        let mut feel = FeelRun { seen: Some(Instant::now() - std::time::Duration::from_secs(1)), after: 0.6, chances: 1, ..default() };
        assert!(!feel.answer(app.world_mut(), true, dodge, true));
        assert!(feel.held && feel.could_not_answer(false));
        // An answered try is never set aside for this: it said something of its moment.
        assert!(!feel.could_not_answer(true));
        // A new wind-up starts with no frames.
        let mut feel = FeelRun { chances: 3, calm: true, ..default() };
        assert!(feel.observe(true, false));
        assert_eq!(feel.chances, 0);
    }

    /// Every holder once, every block at least once; the hand and the planks on the slowest
    /// blocks, each block placed with the holder fastest on it, the rest on their fastest.
    #[test]
    fn the_break_row_has_every_holder_and_block_and_keeps_its_slow_breaks() {
        let [glass, dirt, stone, cobble] = [BlockId(1), BlockId(2), BlockId(3), BlockId(4)];
        let (pick, shovel, axe) = (Holder::Tool(BlockId(10)), Holder::Tool(BlockId(11)), Holder::Tool(BlockId(12)));
        let hardness = |b: BlockId| [0.0, 0.3, 0.5, 1.5, 2.0][b.0 as usize];
        let seconds = |b: BlockId, h: Holder| {
            let speed = match h {
                h if h == pick && (b == stone || b == cobble) => 8.0,
                h if h == shovel && b == dirt => 4.0,
                _ => 1.0,
            };
            hardness(b) / speed
        };
        let blocks = [glass, dirt, stone, cobble];
        let holders = [Holder::Hand, Holder::Planks, pick, shovel, axe, Holder::Tool(BlockId(13))];
        let row = break_row(&blocks, &holders, seconds);
        assert_eq!(row, [(cobble, Holder::Hand), (stone, Holder::Planks), (stone, pick), (dirt, shovel), (glass, axe), (glass, Holder::Tool(BlockId(13)))]);
        // More blocks than holders: the rest go round the holders again.
        let row = break_row(&blocks, &[Holder::Hand, pick], seconds);
        assert_eq!(row, [(cobble, Holder::Hand), (glass, pick), (dirt, Holder::Hand), (stone, pick)]);
    }

    #[test]
    fn a_try_no_frame_could_answer_is_excused_only_for_waiting_on_a_core() {
        // A frame of 0.9 s, past the 0.7 s wind-up: 0.8 s of it our threads waited for a
        // core, so our own part was 0.1 s, within the quarter second.
        let starved = FeelRun { longest: (0.9, 0.8), ..default() };
        assert!(starved.starved(true));
        // On a machine with cores to spare the waiting is never the machine's.
        assert!(!starved.starved(false));
        // Our own part over the quarter second: the frame was ours, and is not excused.
        assert!(!FeelRun { longest: (0.9, 0.6), ..default() }.starved(true));
        // Unknown waiting (none told) is none.
        assert!(!FeelRun { longest: (0.9, 0.0), ..default() }.starved(true));
        // A frame within the quarter second was never slow: what made it unanswerable is not
        // the frames, and it is not excused however long anything waited.
        assert!(!FeelRun { longest: (0.2, 0.2), ..default() }.starved(true));
        // Waiting summed over threads can pass the frame's length: our part is then none.
        assert!(FeelRun { longest: (0.9, 3.0), ..default() }.starved(true));
        // A frame our threads were busy for all along, held back by nothing: on macOS its
        // 0.9 s runnable (in ms) were all running, so it waited nothing and is not excused.
        let busy = crate::slow_frames::queued(900, 600, 300) as f32 / 1000.0;
        assert!(!FeelRun { longest: (0.9, busy), ..default() }.starved(true));
    }

    #[test]
    fn a_wind_ups_longest_frame_counts_from_the_one_it_showed_in() {
        let mut feel = FeelRun { calm: true, ..default() };
        feel.frame();
        // Before any wind-up, frames are timed but none kept.
        feel.frame_at = feel.frame_at.map(|(at, waited)| (at - std::time::Duration::from_millis(500), waited));
        feel.frame();
        assert!(feel.last_frame.0 >= 0.5 && feel.longest == (0.0, 0.0));
        // The frame the pose came in during is the try's first.
        assert!(feel.observe(true, false));
        assert_eq!(feel.longest, feel.last_frame);
        // A shorter frame after it leaves the longest; a longer one takes its place.
        feel.frame();
        assert!(feel.longest.0 >= 0.5);
        feel.frame_at = feel.frame_at.map(|(at, waited)| (at - std::time::Duration::from_millis(900), waited));
        feel.frame();
        assert!(feel.longest.0 >= 0.9);
    }

    #[test]
    fn an_answer_pressed_before_the_input_is_read_goes_in_that_frame() {
        // As `feel_press` runs: in PreUpdate, before the input systems. The game reads the
        // press in the same update, so it is sent by the frame that pressed it, not the next.
        #[derive(Resource)]
        struct Answer(FeelRun, bool);
        for blocking in [false, true] {
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin);
            app.insert_resource(Answer(FeelRun { seen: Some(Instant::now()), after: 0.0, ..default() }, blocking));
            app.add_systems(PreUpdate, (|world: &mut World| {
                world.resource_scope(|world, mut answer: Mut<Answer>| {
                    let blocking = answer.1;
                    answer.0.answer(world, blocking, KeyCode::ShiftLeft, false);
                });
            }).before(bevy::input::InputSystems));
            app.update();
            assert_eq!(app.world().resource::<ButtonInput<KeyCode>>().just_pressed(KeyCode::ShiftLeft), !blocking);
            assert_eq!(app.world().resource::<ButtonInput<MouseButton>>().just_pressed(MouseButton::Right), blocking);
            assert!(app.world().resource::<Answer>().0.sent.is_some(), "Last is told when it went");
        }
    }

    #[test]
    fn a_consumed_attacks_stale_pose_cannot_rearm_a_native_response() {
        for blocking in [false, true] {
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin);
            let mut feel = FeelRun { calm: true, after: 0.0, ..default() };
            let dodge = KeyCode::ShiftLeft;
            // Hurt arrives while Attacking is still replicated, then that same pose
            // survives two more input frames without another result.
            for received in [true, false, false] {
                let began = feel.observe(true, received);
                let queued = feel.answer(app.world_mut(), blocking, dodge, received);
                app.update();
                assert!(!app.world().resource::<ButtonInput<KeyCode>>().just_pressed(dodge));
                assert!(!app.world().resource::<ButtonInput<MouseButton>>().just_pressed(MouseButton::Right));
                assert!(!began && !queued && feel.seen.is_none(), "a consumed attack cannot become a new wind-up");
            }
            assert!(!feel.observe(false, false), "Idle rearms the next pose edge");
            app.update();
            assert!(feel.seen.is_none());
            assert!(feel.observe(true, false), "a fresh attack still begins a try");
            assert!(feel.answer(app.world_mut(), blocking, dodge, false));
            app.update();
            assert_eq!(app.world().resource::<ButtonInput<KeyCode>>().just_pressed(dodge), !blocking);
            assert_eq!(app.world().resource::<ButtonInput<MouseButton>>().just_pressed(MouseButton::Right), blocking);
        }
    }

    #[test]
    fn swim_controls_land_on_the_original_finite_bank_across_frame_intervals() {
        use voxmar_engine::movement::{self, Body, Controls, Surroundings, Tuning};
        use voxmar_engine::physics::Aabb;
        use voxmar_engine::world::Column;
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let tuning = Tuning::from_ron(include_str!("../../../assets/content/core/movement.ron")).unwrap();
        let feet = IVec3::new(0, 200, 0);
        struct Pool<'a> { world: VoxelWorld, blocks: &'a voxmar_engine::block::BlockRegistry }
        impl Surroundings for Pool<'_> {
            fn solid(&self, cell: IVec3) -> bool { self.world.block(cell).is_some_and(|b| self.blocks.info(b).solid) }
            fn water(&self, aabb: &Aabb) -> (f32, Vec2) { movement::water_around(self.blocks, |cell| self.world.block(cell), aabb) }
        }
        let fixture = || {
            let mut world = VoxelWorld::default();
            for z in -1..=0 {
                for x in -1..=0 { world.insert(ColumnPos::new(x, z), Arc::new(Column::default())); }
            }
            let dirt = content.blocks.expect("base:dirt");
            let water = content.blocks.expect("base:water");
            for z in -7..=7 {
                for x in -7..=7 {
                    for y in -4..0 { world.set_block(feet + IVec3::new(x, y, z), dirt); }
                }
            }
            for z in -1..=1 {
                for x in 3..=5 {
                    for y in -3..0 { world.set_block(feet + IVec3::new(x, y, z), water); }
                }
                world.set_block(feet + IVec3::new(6, 0, z), dirt);
            }
            Pool { world, blocks: &content.blocks }
        };
        let simulate = |pool: &Pool, intervals: &[f32], may_jump: bool| {
            let mut body = Body { position: feet.as_vec3() + Vec3::new(4.5, -2.0, 0.5), ..Default::default() };
            let mut controls = Controls { wish: Vec3::X, forward: true, jump: may_jump, ..Default::default() };
            let (mut elapsed, mut highest) = (0.0, body.position.y);
            for frame in 0..10_000 {
                let dt = intervals[frame % intervals.len()].min(0.25);
                elapsed += dt;
                movement::advance(&mut body, pool, &tuning, controls, dt);
                highest = highest.max(body.position.y);
                assert!(!body.aabb().intersects(|cell| pool.solid(cell)), "inside a solid block: {}", body.position);
                let high = body.position.y >= feet.y as f32 + 1.0 - 0.01;
                if !body.in_water && high { controls.jump = false; }
                if body.on_ground && !body.in_water && high {
                    assert!((body.position.y - feet.y as f32 - 1.0).abs() <= 0.01);
                    assert!(body.position.x > feet.x as f32 + 5.7 && body.position.x < feet.x as f32 + 7.3);
                    return Some(body);
                }
                if elapsed >= 8.0 {
                    eprintln!("finite swim failed: intervals {intervals:?}, jump {may_jump}, at {}, highest {}, velocity {}, grounded {}, water {}", body.position - feet.as_vec3(), highest - feet.y as f32, body.velocity, body.on_ground, body.in_water);
                    return None;
                }
            }
            panic!("unbounded frame trace");
        };
        let bank = fixture();
        for dt in [0.25, 1.0 / 6.0, 1.0 / 12.0, 1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0, 1.0 / 500.0] {
            assert!(simulate(&bank, &[dt], true).is_some(), "no landing at {dt} seconds per frame");
        }
        assert!(simulate(&bank, &[0.25, 1.0 / 60.0, 0.12, 0.25, 0.05], true).is_some(), "no landing with mixed frame intervals");
        assert!(simulate(&bank, &[0.25], false).is_none(), "without swimming up, walking cannot pass the raised bank");
        for phase in 1..=30 {
            let intervals = [phase as f32 / 120.0, 0.25, 0.25, 0.25];
            assert!(simulate(&bank, &intervals, true).is_some(), "no landing for phase {phase}");
        }
    }

    #[test]
    fn observer_heartbeats_leave_commands_and_teleports_prompt() {
        let (connection, server) = local_pair();
        let mut observer = Observer {
            connection, world: VoxelWorld::default(), id: None, position: None,
            moved: false, others: default(), names: default(), teleports: 0,
            replies: Vec::new(), last_input: None,
        };
        let now = Instant::now();
        let origin = Vec3::new(1.0, 180.0, 2.0);
        assert!(server.send(ServerMessage::Welcome { player: 7, seed: 1, weather_seed: 2, position: origin, yaw: 0.0, flying: true, allow_flight: true, admin: true }));
        observer.pump_at(now).unwrap();
        assert!(matches!(server.try_recv().unwrap(), Some(ClientMessage::Input(input)) if input.position == origin && input.flying && input.teleport == 0));
        for _ in 0..10_000 {
            observer.pump_at(now).unwrap();
        }
        assert!(observer.connection.send(ClientMessage::Command { line: "locate settlement 3000".into() }));
        assert!(matches!(server.try_recv().unwrap(), Some(ClientMessage::Command { line }) if line == "locate settlement 3000"), "render-frame inputs must not queue ahead of commands");
        assert!(server.try_recv().unwrap().is_none());

        observer.pump_at(now + Duration::from_millis(49)).unwrap();
        assert!(server.try_recv().unwrap().is_none());
        observer.pump_at(now + Duration::from_millis(50)).unwrap();
        assert!(matches!(server.try_recv().unwrap(), Some(ClientMessage::Input(_))));
        assert!(server.try_recv().unwrap().is_none());
        let later = now + Duration::from_secs(10);
        observer.pump_at(later).unwrap();
        for _ in 0..1000 {
            observer.pump_at(later).unwrap();
        }
        assert!(matches!(server.try_recv().unwrap(), Some(ClientMessage::Input(_))));
        assert!(server.try_recv().unwrap().is_none(), "a long pause sends one heartbeat, not a catch-up burst");

        let at = Vec3::new(-1043.0, 178.0, 2908.0);
        assert!(server.send(ServerMessage::Teleport { position: origin, seq: 1 }));
        assert!(server.send(ServerMessage::Teleport { position: at, seq: 2 }));
        observer.pump_at(later + Duration::from_millis(1)).unwrap();
        assert!(matches!(server.try_recv().unwrap(), Some(ClientMessage::Input(input)) if input.position == at && input.teleport == 2 && input.flying));
        assert!(server.try_recv().unwrap().is_none(), "teleports drained together echo only the latest state");

        let column = ColumnPos::new(-33, 90);
        assert!(server.send(ServerMessage::Column { pos: column, column: std::sync::Arc::new(voxmar_engine::world::Column::default()) }));
        assert!(server.send(ServerMessage::Chat { kind: voxmar_protocol::ChatKind::Chat, text: Words::text("settlement reply") }));
        observer.pump_at(later + Duration::from_millis(2)).unwrap();
        assert!(observer.world.contains(column), "incoming world updates are never throttled");
        assert_eq!(observer.replies.last(), Some(&Words::text("settlement reply")));
        assert!(server.try_recv().unwrap().is_none());
        observer.pump_at(later + Duration::from_millis(50)).unwrap();
        assert!(server.try_recv().unwrap().is_none(), "a teleport starts a fresh heartbeat interval");
        observer.pump_at(later + Duration::from_millis(51)).unwrap();
        assert!(matches!(server.try_recv().unwrap(), Some(ClientMessage::Input(input)) if input.teleport == 2));
    }

    /// A 960×540 shot with the horizon at row `horizon`: `sky(x, y)` above it, the sea below
    /// (as pale as the sky over it, as in the gate's morning shot).
    fn shot(horizon: usize, sky: impl Fn(usize, usize) -> [u8; 3]) -> (usize, usize, Vec<u8>) {
        let (w, h) = (960, 540);
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = if y < horizon { sky(x, y) } else { [183, 212, 245] };
                data.extend([r, g, b, 255]);
            }
        }
        (w, h, data)
    }

    /// The gate's morning sky as drawn: (153, 193, 241) at the top, (188, 216, 246) at the
    /// horizon.
    fn morning(y: usize, horizon: usize) -> [u8; 3] {
        let t = y as f32 / horizon as f32;
        let mix = |a: f32, b: f32| (a + (b - a) * t).round() as u8;
        [mix(153.0, 188.0), mix(193.0, 216.0), mix(241.0, 246.0)]
    }

    const HORIZON: usize = 325;

    #[test]
    fn a_sky_of_one_colour_is_flat() {
        let (w, h, flat) = shot(HORIZON, |_, _| [168, 204, 244]);
        let result = sky_gradient(w, h, &flat, Some(HORIZON as f32));
        assert!(result.as_ref().is_err_and(|e| e.starts_with("the sky is flat")), "{result:?}");
        // Nor is a sky drawn in no sky colour at all (grey fog, black) taken for one.
        let (w, h, grey) = shot(HORIZON, |_, _| [200, 200, 200]);
        let result = sky_gradient(w, h, &grey, Some(HORIZON as f32));
        assert!(result.as_ref().is_err_and(|e| e.starts_with("too little clear sky")), "{result:?}");
    }

    #[test]
    fn clouds_in_frame_leave_the_gradient_seen() {
        // A cloud over half the top, as in the shot that flaked, its grey underside, and a row
        // of small ones over the horizon.
        let clouded = |x: usize, y: usize| {
            let big = (120..640).contains(&x) && y < 150;
            let small = (x / 60) % 2 == 0 && (280..310).contains(&y);
            match (big, small) {
                (true, _) if y > 120 => [186, 183, 187],
                (true, _) | (_, true) => [240, 238, 236],
                _ => morning(y, HORIZON),
            }
        };
        let (w, h, sky) = shot(HORIZON, clouded);
        let result = sky_gradient(w, h, &sky, Some(HORIZON as f32));
        assert!(result.is_ok(), "{result:?}");
        // A flat sky with the same clouds is still flat.
        let flat = |x: usize, y: usize| if (120..640).contains(&x) && y < 150 { [240, 238, 236] } else { [168, 204, 244] };
        let (w, h, sky) = shot(HORIZON, flat);
        assert!(sky_gradient(w, h, &sky, Some(HORIZON as f32)).is_err());
    }

    /// The day check over shots the gate saved (`VOXMAR_SKY_SHOTS`, paths split by `:`),
    /// with the horizon where the gate's morning pose draws it.
    #[test]
    #[ignore = "reads shots from VOXMAR_SKY_SHOTS"]
    fn saved_morning_shots_show_the_gradient() {
        let paths = std::env::var("VOXMAR_SKY_SHOTS").expect("VOXMAR_SKY_SHOTS");
        let horizon = std::env::var("VOXMAR_SKY_HORIZON").map_or(HORIZON as f32, |h| h.parse().expect("a number"));
        for path in paths.split(':').filter(|p| !p.is_empty()) {
            let rgba = image::open(path).expect("a shot").to_rgba8();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
            let result = sky_gradient(w, h, rgba.as_raw(), Some(horizon));
            println!("{path}: {result:?}");
            assert!(result.is_ok(), "{path}: {result:?}");
        }
    }
}
