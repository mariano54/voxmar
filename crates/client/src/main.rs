//! The voxmar client.
//!
//! `voxmar` opens the title screen. Options that go straight into a game instead:
//! `--world NAME` play (or create) the saved world NAME, `--connect HOST[:PORT]` join a
//! server, `--capture PATH` (screenshot and exit once loaded; plays an unsaved world unless
//! `--world` or `--connect` is given) with `--pose dx,dy,dz,yaw,pitch` (camera offset from
//! the player) or `--flight SECONDS` (fly forward first and report frame pacing) or `--dig
//! SECONDS` (dig straight down holding the button, reporting frame pacing), `--wait
//! SECONDS` (let the world run that much longer first, for weather to build), `--time
//! FRACTION` (hold the time of day, 0 is sunrise), `--mine PROGRESS` (mine the block in
//! the middle of the view that far, cracks and chips showing; 1 breaks it just before the
//! screenshot), `--menu` (show the settings menu), `--inventory` (show the inventory), `--character` (the character screen), `--journal PAGE` (the journal on that page), `--tip NAME` (hold that first-time tip), `--quests QUEST` (the quest book on that quest), `--pin QUEST` (pin a quest), `--hover-slot N` (the tooltip of inventory slot N), `--loading` (keep the loading cover up), `--tab NAME` (open that settings tab), `--guide SKILL` (open that skill's guide), `--strike CREATURE` (summon one and strike it before the shot), `--level-up SKILL` (note that skill's next level before the shot), `--tend CREATURE` (summon one and feed it until the shot), `--grid A,B,C,D` (lay items into the crafting grid; up to nine, row by row, show a crafting table's three by three), `--recipes NAME` / `--uses NAME` (open the recipe viewer on an item), `--effect NAME:SECONDS` (show the player that timed effect, the server not told), `--effect-tip NAME` (show that effect's hover plate over its chip), `--catalogue SEARCH` (open creative mode's catalogue on a search, once in creative),
//! `--size WIDTHxHEIGHT` (the shot's size, 1920x1080 unless given), `--scale-factor N` (screen
//! pixels per interface pixel, 2 to shoot as a Retina display draws: use twice the size), `--clip SECONDS` (save
//! every frame of that much game time at 30 fps instead, to check motion; `--clip-join` starts
//! it the moment the player joins rather than once the world has settled, `--clip-command LINE`
//! runs a console command as it starts), `--windowed`
//! (draw to a real window and shoot that, for what only a window has, such as the render
//! scale; with the saved settings when `VOXMAR_CONFIG` is set), `--flicker` or
//! `--flicker-still` (measure shimmer; see capture.rs) or
//! `--screen title|worlds|create|join|settings` (capture a menu screen instead). `--gate DIR`
//! plays a scripted session headless and checks it (see gate.rs), saving its shots and
//! report in DIR. `--server-lag MS` makes every tick of the integrated server take that
//! much longer, as on a busy machine (gate runs default to `gate::SERVER_LAG`).
//! Other options: `--host` also open singleplayer worlds to the network, `--seed N` (for new
//! worlds made by `--world` or `--capture`), `--name NAME`, `--view-distance N` (columns),
//! `--fov DEGREES`, `--language CODE` (`en`, `ja`; overrides the setting, and captures are in
//! English without it), `--weather KIND[:INTENSITY]` (show that instead of the server's
//! weather; the kinds are in `weather.ron`, intensities 0..1 or light|moderate|heavy|max), `--command LINE` (run a console command once in game, such as
//! `"/time set noon"`, the `/` optional; repeatable), `--no-vsync`, `--no-music`. `--view-distance`,
//! `--fov` and `--no-vsync` override the saved settings. Captures are silent, and so is
//! any run with the environment variable `VOXMAR_MUTE` set.

mod ambience;
mod breaking;
mod fx;
mod butterflies;
mod capture;
mod creature_sounds;
mod creatures;
mod drops;
mod figures;
mod emitters;
mod fight;
mod creature_motion;
mod fight_fx;
mod gate;
mod held;
mod hud;
mod interact;
mod keys;
mod lang;
mod menu;
mod mix;
mod music;
mod net;
mod player;
mod priority;
mod render;
mod screenshot;
mod session;
mod settings;
mod sfx;
mod slow_frames;
mod textures;
mod ui;
mod weather;
mod world;

use std::sync::Arc;
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use voxmar_engine::action::ActionRegistry;
use voxmar_engine::block::{BlockRegistry, RenderKind, Tint};
use voxmar_engine::coords::Face;
use voxmar_server::ServerConfig;
use voxmar_worldgen::WorldContent;

use crate::hud::HudImages;
use crate::menu::{FirstScreen, Screen};
use crate::render::lod::{Lod, LodPalette, WorldGenerator};
use crate::render::{LayerInfo, SkyMaterial, ui_image};
use crate::session::Launch;
use crate::settings::Settings;
use crate::textures::{Rgba, Source};
use crate::world::ClientWorld;

struct Options {
    connect: Option<String>,
    world: Option<String>,
    host: bool,
    seed: u64,
    name: String,
    view_distance: Option<i32>,
    fov: Option<i32>,
    vsync: Option<bool>,
    /// The kind of weather `--weather` asked for, resolved once content has loaded.
    weather: Option<String>,
    commands: Vec<String>,
    capture: Option<std::path::PathBuf>,
    gate: Option<std::path::PathBuf>,
    server_lag: Option<u64>,
    pose: Option<[f32; 5]>,
    flight: Option<f32>,
    dig: Option<f32>,
    wait: f32,
    mine: Option<f32>,
    time: Option<f32>,
    menu: bool,
    inventory: bool,
    character: bool,
    /// `--journal PAGE`: open the journal on that page.
    journal: Option<String>,
    /// `--tip NAME`: hold that first-time tip on screen; `--hover-slot N`: show the
    /// tooltip of inventory slot N.
    tip: Option<String>,
    /// `--quests QUEST`: the quest book with that quest picked; `--pin QUEST`: a quest
    /// pinned on screen.
    quests: Option<String>,
    pins: Vec<String>,
    hover_slot: Option<usize>,
    /// `--loading`: keep the loading cover up, to capture it.
    loading: bool,
    /// `--strike CREATURE`, `--level-up SKILL`: show the target readout and a new level in the shot.
    strike: Option<String>,
    level_up: Option<String>,
    /// `--tend CREATURE`: summon one and feed it until the shot.
    tend: Option<String>,
    /// `--recipes NAME`, `--uses NAME`: open the recipe viewer on an item.
    lookup: Option<(String, bool)>,
    /// `--grid A,B,C,D`: items to lay into the crafting grid (more than four: a crafting
    /// table's, row by row).
    grid: Option<Vec<String>>,
    /// `--tab NAME`: the settings tab to open; `--guide SKILL`: the skill guide to open.
    tab: Option<String>,
    guide: Option<String>,
    /// `--effect NAME:SECONDS`: timed effects to show the player; `--effect-tip NAME`: the
    /// effect whose hover plate to show over its chip.
    effects: Vec<(String, u16)>,
    effect_tip: Option<String>,
    /// `--catalogue SEARCH`: the inventory's catalogue or survival recipe search.
    catalogue: Option<String>,
    flicker: Option<capture::FlickerKind>,
    clip: Option<f32>,
    /// `--clip-join`: start the clip on joining; `--clip-command LINE`: run it as the clip
    /// starts.
    clip_join: bool,
    clip_commands: Vec<String>,
    /// `--press left|right`: hold a mouse button at the start of the clip.
    press: Option<MouseButton>,
    /// `--hold ITEM`: select the hotbar slot holding an item, once it is there.
    hold: Option<String>,
    /// `--wear ITEM` (again for each piece): put armour on, once it is in the inventory.
    wear: Vec<String>,
    /// `--repeat`: click `--press`'s button over and over through the clip.
    repeat: bool,
    /// `--face-creature`: keep looking at the nearest creature.
    face_creature: bool,
    size: UVec2,
    scale_factor: f32,
    windowed: bool,
    screen: Option<Screen>,
    music: bool,
    language: Option<String>,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        connect: None,
        world: None,
        host: false,
        seed: ServerConfig::default().seed,
        name: std::env::var("USER").unwrap_or_else(|_| "player".into()),
        view_distance: None,
        fov: None,
        vsync: None,
        weather: None,
        commands: Vec::new(),
        capture: None,
        gate: None,
        server_lag: None,
        pose: None,
        flight: None,
        dig: None,
        wait: 0.0,
        mine: None,
        time: None,
        menu: false,
        inventory: false,
        character: false,
        journal: None,
        tip: None,
        quests: None,
        pins: Vec::new(),
        hover_slot: None,
        loading: false,
        strike: None,
        tend: None,
        lookup: None,
        grid: None,
        level_up: None,
        tab: None,
        guide: None,
        effects: Vec::new(),
        effect_tip: None,
        catalogue: None,
        flicker: None,
        clip: None,
        clip_join: false,
        clip_commands: Vec::new(),
        press: None,
        hold: None,
        wear: Vec::new(),
        repeat: false,
        face_creature: false,
        size: UVec2::new(1920, 1080),
        scale_factor: 1.0,
        windowed: false,
        screen: None,
        music: true,
        language: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--connect" => options.connect = Some(value()?),
            "--world" => options.world = Some(value()?),
            "--host" => options.host = true,
            "--seed" => options.seed = value()?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--name" => options.name = value()?,
            "--view-distance" => options.view_distance = Some(value()?.parse().map_err(|e| format!("--view-distance: {e}"))?),
            "--fov" => options.fov = Some(value()?.parse().map_err(|e| format!("--fov: {e}"))?),
            "--no-vsync" => options.vsync = Some(false),
            "--weather" => {
                options.weather = Some(value()?);
            }
            "--command" => options.commands.push(value()?),
            "--no-music" => options.music = false,
            "--language" => options.language = Some(value()?),
            "--capture" => options.capture = Some(value()?.into()),
            "--gate" => options.gate = Some(value()?.into()),
            "--server-lag" => options.server_lag = Some(value()?.parse().map_err(|e| format!("--server-lag: {e}"))?),
            "--flight" => options.flight = Some(value()?.parse().map_err(|e| format!("--flight: {e}"))?),
            "--dig" => options.dig = Some(value()?.parse().map_err(|e| format!("--dig: {e}"))?),
            "--wait" => options.wait = value()?.parse().map_err(|e| format!("--wait: {e}"))?,
            "--mine" => options.mine = Some(value()?.parse().map_err(|e| format!("--mine: {e}"))?),
            "--time" => options.time = Some(value()?.parse().map_err(|e| format!("--time: {e}"))?),
            "--menu" => options.menu = true,
            "--inventory" => options.inventory = true,
            "--character" => options.character = true,
            "--journal" => options.journal = Some(value()?),
            "--tip" => options.tip = Some(value()?),
            "--quests" => options.quests = Some(value()?),
            "--pin" => options.pins.push(value()?),
            "--hover-slot" => options.hover_slot = Some(value()?.parse().map_err(|e| format!("--hover-slot: {e}"))?),
            "--loading" => options.loading = true,
            "--strike" => options.strike = Some(value()?),
            "--level-up" => options.level_up = Some(value()?),
            "--tend" => options.tend = Some(value()?),
            "--recipes" => options.lookup = Some((value()?, false)),
            "--uses" => options.lookup = Some((value()?, true)),
            "--grid" => options.grid = Some(value()?.split(',').map(str::to_owned).collect()),
            "--tab" => options.tab = Some(value()?),
            "--guide" => options.guide = Some(value()?),
            "--effect" => {
                let effect = value()?;
                let (name, seconds) = effect.rsplit_once(':').ok_or("--effect needs NAME:SECONDS")?;
                options.effects.push((name.to_owned(), seconds.parse().map_err(|e| format!("--effect: {e}"))?));
            }
            "--effect-tip" => options.effect_tip = Some(value()?),
            "--catalogue" => options.catalogue = Some(value()?),
            "--flicker" => options.flicker = Some(capture::FlickerKind::Step),
            "--flicker-still" => options.flicker = Some(capture::FlickerKind::Still),
            "--hold" => options.hold = Some(value()?),
            "--wear" => options.wear.push(value()?),
            "--repeat" => options.repeat = true,
            "--face-creature" => options.face_creature = true,
            "--clip" => options.clip = Some(value()?.parse().map_err(|e| format!("--clip: {e}"))?),
            "--clip-join" => options.clip_join = true,
            "--clip-command" => options.clip_commands.push(value()?),
            "--press" => {
                options.press = Some(match value()?.as_str() {
                    "left" => MouseButton::Left,
                    "right" => MouseButton::Right,
                    other => return Err(format!("--press: `{other}` is not left or right")),
                })
            }
            "--windowed" => options.windowed = true,
            "--scale-factor" => options.scale_factor = value()?.parse().map_err(|e| format!("--scale-factor: {e}"))?,
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').ok_or("--size needs WIDTHxHEIGHT")?;
                options.size = UVec2::new(w.parse().map_err(|e| format!("--size: {e}"))?, h.parse().map_err(|e| format!("--size: {e}"))?);
            }
            "--screen" => {
                let name = value()?;
                options.screen = Some(Screen::from_name(&name).ok_or_else(|| format!("--screen: no screen called {name}"))?);
            }
            "--pose" => {
                let parts: Vec<f32> = value()?.split(',').map(str::parse).collect::<Result<_, _>>().map_err(|e| format!("--pose: {e}"))?;
                options.pose = Some(parts.try_into().map_err(|_| "--pose needs x,y,z,yaw,pitch")?);
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(options)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options()?;
    priority::frame_thread();
    // A gate run is a capture that plays and checks a session rather than taking a shot.
    let headless = options.capture.is_some() || options.gate.is_some();
    // Leave two cores for the main and render threads; worldgen, lighting and meshing share
    // the rest, at background priority so bursts of streaming never delay a frame
    // (priority.rs). Cores, not hardware threads: on a machine with two threads a core (the
    // Linux box's 16-core EPYC), workers on every thread slowed every frame system about
    // threefold while the world streamed in, whatever their priority, and took flightfast's
    // p99 frame from 11 to 25 ms. `VOXMAR_WORKERS=n` sets their number instead, for timing how
    // many a busy machine's frames can take (tools/perf/play_frames).
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut workers = physical_cores().unwrap_or(threads).min(threads).saturating_sub(2).max(2);
    if let Ok(given) = std::env::var("VOXMAR_WORKERS")
        && !given.is_empty()
    {
        match given.parse::<usize>() {
            Ok(n) if (1..=threads).contains(&n) => workers = n,
            // Before logging starts, so on stderr, where tools/perf/play_frames shows it beside
            // the numbers, which are then the default's.
            _ => eprintln!("VOXMAR_WORKERS={given} is not from 1 to {threads}; {workers} workers"),
        }
    }
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|i| format!("worker-{i}"))
        .start_handler(|_| priority::streaming_worker())
        .build_global()?;
    // The generator's region builds borrow helper threads besides: no more than there are
    // workers.
    voxmar_worldgen::set_helper_limit(workers);

    let assets = voxmar_server::assets_dir();
    let content = voxmar_server::load_content(&assets)?;
    let interface = hud::inventory::InterfaceStyle::load(&content.files)?;
    let combat_marks = hud::combat::CombatStyle::load(&content.files)?;
    let quest_look = hud::quest_look::QuestLook::load(&content.files)?;
    let menu_bar = hud::screens::MenuBar::load(&content.files)?;
    if let Some(name) = quest_look.unknown_quests(|name| content.quests.get(name).is_some()).next() {
        return Err(format!("{}: a shape for `{name}`, which is no quest", hud::quest_look::FILE).into());
    }
    let fight_look = fight_fx::FightLook::load(&content.files)?;
    if let Some(blow) = fight_look.untold(&content.creatures).first() {
        return Err(format!("{}: no tell for the blow `{blow}`", fight_fx::style::FILE).into());
    }
    if let Some(name) = fight_look.unlain(&content.creatures).first() {
        return Err(format!("{}: `{name}` dies lying fallen less than {} s, past the killing blow's freeze and its going", fight_fx::style::FILE, voxmar_engine::creature::LIES).into());
    }
    if let Some(class) = fight_look.unshot(content.combat.classes()).first() {
        return Err(format!("{}: no shot for the {class} class, which shoots", fight_fx::style::FILE).into());
    }
    let creature_motion = creature_motion::Motion::load(&content.files)?;
    if let Some(name) = creature_motion.unknown_species(|name| content.creatures.find(name).is_some()).next() {
        return Err(format!("{}: gaits for `{name}`, which is no creature", creature_motion::FILE).into());
    }
    let look = hud::target::LookStyle::load(&content.files)?;
    let night_light = render::night_light::NightLight::load(&content.files)?;
    let player_look = figures::PlayerLook::load(&content.files)?;
    let celebrations = hud::celebrate::Celebrations::load(&content.files)?;
    let recorded = sfx::Recordings::load(&content.files)?;
    recorded.check(&content.actions)?;
    recorded.check_wear()?;
    recorded.covers(&content.actions)?;
    let creature_bank = creature_sounds::Bank::load(&content.files, &content.creatures)?;
    creature_bank.check_files(&assets)?;
    let unbarked = creature_bank.unbarked(&content.creatures);
    if !unbarked.is_empty() {
        return Err(format!("{}: settlement people with no greet, alarm or content of their own: {}", creature_sounds::defs::FILE, unbarked.join(", ")).into());
    }
    recorded.check_materials(&content.blocks)?;
    recorded.check_fire(&content.fire)?;
    // Every roof's rain patters or is silent (ambience.ron's `roofs`), by its building
    // material or else its family sound.
    let ambience = ambience::defs::Ambience::load(&content.files)?;
    let materials: Vec<&str> = recorded.materials.iter().map(|m| m.name.as_str()).collect();
    let roof_patters = ambience.roof_patters(&content.blocks, &materials, |id| {
        recorded.material_of(&content.blocks, id).map_or_else(|| ambience::defs::family(content.blocks.info(id).sound), |m| recorded.materials[m].name.clone())
    })?;
    // Every shelter tag on a block says how it sounds: muffling the rain, or whistling.
    ambience.check_shelter_tags(&content.blocks)?;
    recorded.check_ways(&content.blocks)?;
    recorded.check_files(&assets)?;
    ambience.check_recordings(&assets)?;
    recorded.mix.check().map_err(|e| format!("sounds.ron: {e}"))?;
    fx::ActionFx::load(&content.files)?.check(&content.actions)?;
    if let Some(effect) = textures::Source::bundled(&assets.join("packs")).unpictured(&content.effects).first() {
        return Err(format!("effects.ron: the effect {effect} has no icon in any bundled texture pack").into());
    }
    let content_weather = content.weather.clone();
    let shader_packs = render::pack::ShaderPacks::load(&assets);
    let registry = content.blocks.clone();
    let world_content = Arc::new(WorldContent::load(&content)?);
    // Every building an ambient sound comes from is one settlements have (ambience.ron).
    ambience.check_buildings(&world_content.settlements.building_names())?;
    let launch = Launch { name: options.name.clone(), host: options.host, content: content.clone() };
    // Captures are benchmarks and visual checks, so they ignore the player's saved settings,
    // unless a configuration directory is given explicitly.
    let reproducible = headless && std::env::var_os("VOXMAR_CONFIG").is_none();
    // Captures and gate runs change settings as they go (the gate's short view distance,
    // silent ambience); none of it may reach the player's settings file.
    if headless {
        settings::leave_file_untouched();
    }
    let mut settings = if reproducible { Settings::default() } else { Settings::load() };
    settings.view_distance = options.view_distance.unwrap_or(settings.view_distance);
    settings.fov = options.fov.unwrap_or(settings.fov);
    settings.vsync = options.vsync.unwrap_or(settings.vsync);
    // Captures are in English unless asked otherwise, whatever the machine's language.
    let default_language = if reproducible { voxmar_engine::lang::ENGLISH.to_owned() } else { settings.language.clone() };
    settings.language = options.language.clone().unwrap_or(default_language);
    // Captures run the ambience without playing it, so its log shows what would play.
    if headless {
        settings.ambience_volume = 0;
    }
    let settings = settings.clamped();

    let mut app = App::new();
    let window = Window {
        title: "voxmar".into(),
        present_mode: if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
        ..default()
    };
    // Captures make no sound, nor does any run with VOXMAR_MUTE set: games launched while
    // developing would otherwise play over whoever is at the machine. Such runs never open
    // an audio device at all (see `silence`).
    let muted = headless || std::env::var_os("VOXMAR_MUTE").is_some();
    // Our few Bevy meshes (the sky, rain, cracks, other players) are batched on the CPU:
    // building their instances on the GPU takes four compute passes and two checks of
    // indirect draws a frame, and every pass costs about 0.2 ms on Metal however little it
    // does (see docs/rendering.md).
    let mut plugins = DefaultPlugins
        .set(AssetPlugin { file_path: assets.to_string_lossy().into_owned(), ..default() })
        .set(bevy::pbr::PbrPlugin { use_gpu_instance_buffer_builder: false, ..default() })
        .set({
            // Bevy's systems run on a pool of their own, by default every thread left over (24
            // on the 32-thread Linux box), while streaming does its work on our workers. Six
            // served better: waking two dozen threads for each parallel system cost more than
            // they did, and the frame-bound scenes there gained 5-13% (dig 2.95 to 2.62 ms a
            // frame, mountains 2.98 to 2.65).
            // Frames wait for the systems' threads, never for the io and async pools'.
            let mut options = bevy::app::TaskPoolOptions::default();
            options.compute.max_threads = SYSTEM_THREADS;
            options.compute.on_thread_spawn = Some(Arc::new(priority::frame_thread));
            options.io.on_thread_spawn = Some(Arc::new(priority::background_thread));
            options.async_compute.on_thread_spawn = Some(Arc::new(priority::background_thread));
            bevy::app::TaskPoolPlugin { task_pool_options: options }
        });
    if muted {
        plugins = plugins.disable::<bevy::audio::AudioPlugin>();
    }
    if headless && !options.windowed {
        // Headless: render offscreen as fast as possible, independent of any display.
        app.add_plugins((
            plugins
                .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
                .disable::<bevy::winit::WinitPlugin>(),
            ScheduleRunnerPlugin::run_loop(Duration::ZERO),
        ));
    } else {
        app.add_plugins(plugins.set(WindowPlugin { primary_window: Some(window), ..default() }));
    }
    if muted {
        silence(&mut app);
    }
    app
    .add_plugins(screenshot::ScreenshotPlugin)
    .add_plugins((
        FrameTimeDiagnosticsPlugin::default(),
        render::RenderPlugin,
        world::WorldPlugin,
        player::PlayerPlugin,
        interact::InteractPlugin,
        breaking::BreakingPlugin,
        drops::DropsPlugin,
        net::NetPlugin,
        hud::HudPlugin,
        sfx::SfxPlugin,
        settings::SettingsPlugin,
        menu::MenuPlugin,
        session::SessionPlugin,
        weather::WeatherPlugin,
        ui::UiPlugin,
    ))
    .add_plugins((lang::LangPlugin, mix::MixPlugin));
    app.add_plugins(priority::PriorityPlugin);
    app.add_plugins((creatures::CreaturesPlugin, creature_sounds::CreatureSoundsPlugin, butterflies::ButterfliesPlugin));
    app.add_plugins((emitters::EmittersPlugin, fx::FxPlugin));
    app.add_plugins(held::HeldPlugin);
    app.add_plugins(figures::FiguresPlugin);
    app.add_plugins(fight_fx::FightFxPlugin);
    // GPU time per render pass (timestamp queries, Vulkan and DX12), which captures print
    // beside their frame statistics.
    if std::env::var_os("VOXMAR_GPU_TIMES").is_some() {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
    }
    let terrain = app.world().resource::<render::terrain::Terrain>().clone();
    app.insert_resource(ClientWorld::new(registry.clone(), settings.view_distance, terrain))
    .insert_resource(Actions::new(content.actions))
    .insert_resource(render::DayCycle { view_distance: render::lod::view_distance(settings.view_distance), ..default() })
    .insert_resource(shader_packs)
    .insert_resource(lang::Locale::load(&assets.join("content"), &settings.language))
    .insert_resource(settings)
    .insert_resource(Registry { blocks: registry, world: world_content })
    .add_systems(Startup, setup);
    // The weather's easing and the land's air as content tunes them (weather.ron).
    let tuning = content_weather.tuning();
    app.world_mut().resource_mut::<weather::ClientWeather>().tune(weather::Easing::new(&tuning.shown));
    app.world_mut().resource_mut::<weather::atmosphere::ClientAtmosphere>().tune(tuning.air.clone());
    if let Some(forced) = options.weather.as_deref().map(|name| weather::preset(&content_weather, name)).transpose()? {
        app.world_mut().resource_mut::<weather::ClientWeather>().force(forced);
    }
    app.world_mut().resource_mut::<hud::console::Console>().run_when_joined(options.commands);

    // Go straight into a game when asked to, otherwise to the title screen.
    let first = if let Some(screen) = options.screen {
        screen
    } else if let Some(address) = &options.connect {
        app.insert_resource(menu::connect(address));
        Screen::Join
    } else if options.world.is_some() || headless {
        let world = match &options.world {
            Some(name) => Some(menu::worlds_dir().ok_or("no configuration directory to keep worlds in")?.join(name)),
            // Gates exercise saved reconnects too; their disposable world stays beside
            // the gate's own logs and is cleared by the next run.
            None => options.gate.as_ref().map(|dir| dir.join("world")),
        };
        let gate_lag = options.gate.as_ref().map(|_| gate::SERVER_LAG);
        let asked = options.server_lag.map(|ms| voxmar_server::Lag { per_tick: Duration::from_millis(ms), ..default() });
        let lag = asked.or(gate_lag).unwrap_or_default();
        // A gate's world draws (a cast's catch) by a fixed key, as it is made on a fixed
        // seed, so every run casts alike.
        let draw_key = options.gate.as_ref().map(|_| gate::DRAW_KEY);
        let config = ServerConfig { seed: options.seed, world, lag, draw_key, ..ServerConfig::default() };
        app.insert_resource(launch.singleplayer(config)?);
        Screen::InGame
    } else {
        Screen::Title
    };
    app.insert_resource(FirstScreen(first)).insert_resource(launch).insert_resource(interface).insert_resource(quest_look).insert_resource(menu_bar).insert_resource(combat_marks).insert_resource(fight_look).insert_resource(creature_motion).insert_resource(look).insert_resource(celebrations).insert_resource(night_light).insert_resource(player_look);
    app.insert_resource(creature_sounds::Checked(creature_bank));
    app.insert_resource(ambience::Checked(ambience, roof_patters));
    if options.loading {
        app.insert_resource(menu::HoldCover);
    }
    if let Some(tab) = options.tab {
        app.insert_resource(settings::StartTab(tab));
    }
    if !options.pins.is_empty() {
        app.insert_resource(hud::quests::StartPins(options.pins.clone()));
    }
    if let Some(quest) = options.quests {
        app.insert_resource(hud::quests::StartQuest(quest));
    }
    if let Some(tip) = options.tip {
        app.insert_resource(hud::tips::HeldTip(tip));
    }
    if let Some(slot) = options.hover_slot {
        app.insert_resource(hud::inventory::HoverSlot(slot));
    }
    if let Some(page) = options.journal {
        app.insert_resource(hud::journal::StartPage(page));
    }
    if let Some(skill) = options.guide {
        app.insert_resource(hud::character::StartGuide(skill));
    }
    if !options.effects.is_empty() {
        app.insert_resource(hud::effects::ShowEffects(options.effects));
    }
    if let Some(search) = options.catalogue {
        app.insert_resource(hud::creative::StartSearch(search));
    }
    if let Some(name) = options.effect_tip {
        app.insert_resource(hud::effects::PinnedTip(name));
    }
    if let Some((name, uses)) = options.lookup {
        app.insert_resource(hud::recipes::StartLookup(name, uses));
    }
    // Slow frames of real play are recorded for later (see slow_frames.rs); captures and
    // gate runs, which run on a busy machine by design, are not, unless
    // VOXMAR_RECORD_SLOW_FRAMES asks (with VOXMAR_CONFIG elsewhere, to keep them apart).
    // The renderer's part of a slow frame's record is always there to be set.
    app.init_resource::<slow_frames::SlowFrameExtra>();
    if !headless || std::env::var_os("VOXMAR_RECORD_SLOW_FRAMES").is_some() {
        app.add_plugins(slow_frames::SlowFramesPlugin);
    }
    // The gate rehearses the music, silently (`music::Rehearse`), to see its switches and
    // the line naming a track; captures have none, so no track's name shows in a shot.
    if options.music && (!headless || options.gate.is_some()) {
        app.add_plugins(music::MusicPlugin);
    }
    app.add_plugins(ambience::AmbiencePlugin);
    if let Some(dir) = options.gate {
        std::fs::create_dir_all(&dir)?;
        app.add_plugins(gate::GatePlugin { dir });
    } else if let Some(path) = options.capture {
        app.add_plugins(capture::CapturePlugin {
            path,
            pose: options.pose,
            flight: options.flight,
            dig: options.dig,
            wait: options.wait,
            mine: options.mine,
            time: options.time,
            menu: options.menu,
            inventory: options.inventory,
            character: options.character,
            flicker: options.flicker,
            clip: options.clip,
            clip_join: options.clip_join,
            clip_commands: options.clip_commands,
            press: options.press,
            hold: options.hold,
            wear: options.wear,
            repeat: options.repeat,
            face_creature: options.face_creature,
            strike: options.strike,
            level_up: options.level_up,
            tend: options.tend,
            grid: options.grid,
            size: options.size,
            scale_factor: options.scale_factor,
            windowed: options.windowed,
        });
    }
    // An error exit (a failed gate, or wgpu reporting a rendering error, which Bevy quits
    // on) must reach whoever ran the game.
    if let AppExit::Error(code) = app.run() {
        std::process::exit(code.get().into());
    }
    Ok(())
}

/// Threads Bevy runs systems on (its compute task pool), at most.
const SYSTEM_THREADS: usize = 6;

/// Physical cores, where they differ from hardware threads (Linux, from sysfs).
fn physical_cores() -> Option<usize> {
    let mut cores = std::collections::HashSet::new();
    for entry in std::fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.strip_prefix("cpu").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            && let Ok(siblings) = std::fs::read_to_string(entry.path().join("topology/thread_siblings_list"))
        {
            cores.insert(siblings.trim().to_owned());
        }
    }
    (!cores.is_empty()).then_some(cores.len())
}

/// Block definitions and the world generator's definitions from the loaded content
/// packages (biomes, trees, plants), for distant terrain to match the server's world.
#[derive(Resource)]
struct Registry {
    blocks: Arc<BlockRegistry>,
    world: Arc<WorldContent>,
}

/// Action definitions, shared with the server, for proposing and predicting actions.
#[derive(Resource)]
pub struct Actions(pub Arc<ActionRegistry>);

/// The actions the client asks for by name.
const NAMED_ACTIONS: [&str; 9] = ["core:mine", "core:place", "core:transfer", "core:quick_move", "core:stow", "core:drop", "core:craft", "core:eat", "core:respawn"];

impl Actions {
    /// The content's actions, warning once for each the client asks for that no package
    /// defines (a broken or partial mod): its gesture will do nothing.
    pub fn new(actions: Arc<voxmar_engine::action::ActionRegistry>) -> Self {
        for name in NAMED_ACTIONS.into_iter().filter(|name| actions.id(name).is_none()) {
            warn!("no package defines the action {name}, so it cannot be done");
        }
        Self(actions)
    }

    /// The action called `name`, which the game asks for by name, if the content has it.
    pub fn named(&self, name: &str) -> Option<voxmar_engine::action::ActionId> {
        self.0.id(name)
    }
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    registry: Res<Registry>,
    settings: Res<Settings>,
    mut images: ResMut<Assets<Image>>,
    mut fonts: ResMut<Assets<Font>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut crack_materials: ResMut<Assets<render::cracks::CrackMaterial>>,
    world: Res<ClientWorld>,
    terrain: Res<render::terrain::Terrain>,
    launch: Res<Launch>,
) {
    let (registry, world_content) = (&registry.blocks, registry.world.clone());
    let mut source = Source::discover(settings.textures, settings.minecraft_assets);
    let (size, mut block_textures, animations) = source.block_textures(registry.textures());
    // A pack we don't ship may give an opaque face clear texels, which are filled below; it
    // hears of it if that is most of the face.
    let opaque = textures::opaque_faces(registry);
    for warning in textures::see_through_warnings(registry.textures(), &block_textures, &opaque, &source.servers(registry.textures())) {
        warn!("{warning}");
    }
    // Opaque blocks must not show see-through texels; cut-out and translucent ones, and the
    // quads of shapes, keep them.
    let mut see_through = vec![false; block_textures.len()];
    for id in 0..registry.len() {
        let block = voxmar_engine::block::BlockId(id as u16);
        let info = registry.info(block);
        if !matches!(info.render, RenderKind::Opaque | RenderKind::Invisible) {
            for &layer in info.layers.iter().chain(registry.shape_layers(block)) {
                see_through[layer as usize] = true;
            }
        }
    }
    // But an opaque block's face is always filled, though a shape or a cut-out block shares
    // it (planks with the slabs): drawn without an alpha test, it would show clear texels
    // in their own colour.
    for (keep, block) in see_through.iter_mut().zip(&opaque) {
        *keep &= block.is_none();
    }
    for (texture, &keep) in block_textures.iter_mut().zip(&see_through) {
        if !keep {
            texture.fill_transparent();
        }
    }
    // Blocks that give off light glow by themselves, as brightly as the light they give, and
    // windows by the light through them;
    // opaque ones vary their textures from block to block as their data says.
    let mut layers = vec![LayerInfo::default(); block_textures.len()];
    for id in 0..registry.len() {
        let block = voxmar_engine::block::BlockId(id as u16);
        let info = registry.info(block);
        let def = registry.def(block);
        let variety = def.texture_variety;
        for face in Face::ALL {
            let layer = &mut layers[info.layers[face as usize] as usize];
            layer.glow = layer.glow.max(info.emission as f32 / 15.0);
            // Only a face drawn its own way glows by the light through it (one without a
            // drawing takes the first layer), as the registry's load check counts them.
            layer.through |= def.lit_through && def.textures.face(face).is_some();
            if info.render == RenderKind::Opaque {
                layer.variety.turn |= variety.turn;
                layer.variety.mirror |= variety.mirror;
            }
        }
    }
    // The untinted sides of an opaque block with a tinted top (a grass block) take the
    // pack's overlay for them, tinted as the top is, after the textures themselves. Leaves
    // take theirs untinted over the tinted leaves: the blossom and fruit of fruit leaves.
    let mut overlay_tint: Vec<Option<Tint>> = vec![None; block_textures.len()];
    for id in 0..registry.len() {
        let info = registry.info(voxmar_engine::block::BlockId(id as u16));
        let top = info.tints[Face::PosY as usize];
        if info.render == RenderKind::Opaque && top != Tint::None {
            for face in Face::ALL.into_iter().filter(|&f| f != Face::PosY && info.tints[f as usize] == Tint::None) {
                overlay_tint[info.layers[face as usize] as usize] = Some(top);
            }
        } else if info.render == RenderKind::Leaves {
            for layer in info.layers {
                overlay_tint[layer as usize] = Some(Tint::None);
            }
        }
    }
    let overlays = source.overlays(registry.textures(), size, |i| overlay_tint[i].is_some());
    for (layer, overlay) in overlays.into_iter().enumerate() {
        if let (Some(overlay), Some(tint)) = (overlay, overlay_tint[layer]) {
            layers[layer].overlay = Some((block_textures.len() as u16, tint));
            block_textures.push(overlay);
            layers.push(LayerInfo::default());
        }
    }
    // An animated texture's frames after its first go after everything else, each a layer
    // of its own, which the terrain shader steps through by the clock (`LayerInfo`).
    for (layer, animation) in animations.into_iter().enumerate() {
        let Some(animation) = animation else { continue };
        let first = block_textures.len() as u16;
        let frames = animation.frames.len() as u8 + 1;
        for mut frame in animation.frames {
            if !see_through[layer] {
                frame.fill_transparent();
            }
            block_textures.push(frame);
            layers.push(LayerInfo { animation: None, ..layers[layer].clone() });
        }
        layers[layer].animation = Some(render::Animation { first, frames, ticks: animation.ticks });
    }
    // Other drawings of a texture its pack gives (`variants.ron`) go after those, each a
    // layer of its own, which the terrain shader picks among by each block's position.
    for (layer, alternates) in source.block_variants(registry.textures(), size).into_iter().enumerate() {
        if alternates.is_empty() || layers[layer].animation.is_some() {
            continue;
        }
        let first = block_textures.len() as u16;
        let count = alternates.len().min(255) as u8;
        for mut alternate in alternates.into_iter().take(255) {
            if !see_through[layer] {
                alternate.fill_transparent();
            }
            block_textures.push(alternate);
            layers.push(LayerInfo { variants: None, ..layers[layer].clone() });
        }
        layers[layer].variants = Some(render::Variants { first, count });
    }
    // The empty hand's pieces (`held.ron`), each a layer of its own after all of those.
    let mut handled = held::load(source.held_books(), registry);
    if let Some(handling) = handled.0.as_mut()
        && let Some(bare) = &handling.bare
    {
        for texture in source.held_textures(&bare.pieces, size) {
            handling.bare_layers.push(block_textures.len() as u16);
            block_textures.push(texture);
            layers.push(LayerInfo::default());
        }
    }
    ui::skin::load(&mut source, &mut images);
    ui::skin::set_font(settings.font, &mut fonts);
    let colormaps = source.colormaps();
    render::setup(&mut commands, &mut images, &mut meshes, &mut sky_materials, &block_textures, &layers, render::terrain::shape_table(registry));
    let crack_stages = (0..render::cracks::STAGES).map(|stage| source.read(&format!("block/destroy_stage_{stage}"))).collect();
    render::cracks::setup(&mut commands, &mut images, &mut meshes, &mut crack_materials, crack_stages);
    let palette = LodPalette::new(registry, &block_textures, &layers);
    commands.insert_resource(Lod::new(palette, terrain.clone(), world.view_distance));
    commands.insert_resource(WorldGenerator::new(world_content));

    // Inventory icons: a block's own `icon` when it names one, a plant's sprite, or a cube
    // seen from above, with tinted faces in a temperate climate's colours.
    let temperate = |map: &Rgba| {
        let (x, y) = ((0.3 * 255.0) as u32, ((1.0 - 0.6 * 0.7) * 255.0) as u32);
        let i = ((y * map.width + x) * 4) as usize;
        [0, 1, 2].map(|c| map.pixels[i + c] as f32 / 255.0)
    };
    let tints = [temperate(&colormaps[0]), temperate(&colormaps[1])];
    let colour = |kind: Tint| match kind {
        Tint::None => [1.0; 3],
        Tint::Grass => tints[0],
        Tint::Foliage => tints[1],
        Tint::Water => [0.25, 0.45, 0.85],
    };
    let face_texture = |info: &voxmar_engine::block::BlockInfo, face: Face| {
        let layer = info.layers[face as usize] as usize;
        let mut texture = block_textures[layer].clone();
        tint(&mut texture, colour(info.tints[face as usize]));
        if let Some((overlay, kind)) = layers[layer].overlay {
            let mut overlay = block_textures[overlay as usize].clone();
            tint(&mut overlay, colour(kind));
            blend_over(&mut texture, &overlay);
        }
        texture
    };
    let as_image = |t: Rgba| voxmar_texgen::Image { width: t.width, height: t.height, pixels: t.pixels };
    let mut icons: Vec<Handle<Image>> = Vec::with_capacity(registry.len());
    for id in 0..registry.len() {
        let block = voxmar_engine::block::BlockId(id as u16);
        // A fluid's flowing, falling and river states (hundreds of blocks) show their
        // source's icon, which comes before them.
        if let Some(state) = registry.fluid(block) {
            let fluid_source = registry.fluid_def(state.fluid).source;
            if fluid_source != block {
                icons.push(icons[fluid_source.0 as usize].clone());
                continue;
            }
        }
        let (info, def) = (registry.info(block), registry.def(block));
        let own = def.icon.as_ref().and_then(|name| source.read(&format!("item/{name}")));
        // A shape as it stands (a barrel's staves, a stair's steps), each quad in its own
        // texture; a block in facings turned south, so its front faces the icon's left.
        let shown = registry.facing(block, voxmar_engine::block::Facing::South);
        let icon = own.unwrap_or_else(|| match (info.render, registry.shape(shown)) {
            (RenderKind::Plant | RenderKind::Invisible, _) => face_texture(info, Face::PosZ),
            (RenderKind::Model, Some(shape)) => {
                let textures: Vec<_> = registry.shape_layers(shown).iter().map(|&layer| as_image(block_textures[layer as usize].clone())).collect();
                let quads: Vec<_> = shape
                    .quads
                    .iter()
                    .zip(&textures)
                    .map(|(quad, texture)| voxmar_texgen::icon::IconQuad {
                        corners: quad.corners.map(|c| c.to_array()),
                        uvs: quad.uvs,
                        texture,
                        light: voxmar_texgen::icon::icon_light(quad.normal.to_array()),
                    })
                    .collect();
                let icon = voxmar_texgen::icon::isometric_quads(&quads, 32);
                Rgba { width: icon.width, height: icon.height, pixels: icon.pixels }
            }
            _ => {
                // Leaves would show only their holes' rims: draw them as a solid bush.
                let [top, left, right] = [Face::PosY, Face::PosZ, Face::PosX].map(|face| {
                    let mut texture = face_texture(info, face);
                    if info.render == RenderKind::Leaves {
                        texture.fill_transparent();
                    }
                    as_image(texture)
                });
                let cube = voxmar_texgen::icon::isometric(&top, &left, &right, 32);
                Rgba { width: cube.width, height: cube.height, pixels: cube.pixels }
            }
        });
        icons.push(images.add(ui_image(&icon)));
    }
    let crosshair = images.add(ui_image(&source.sprite("crosshair", (15, 15, [255, 255, 255, 0]))));
    // Each skill's icon, `ui/skill/<name>` in the packs, or the unknown skill's.
    let skills = &launch.content.skills;
    let unknown = source.read("ui/skill/unknown");
    let skill_icons = skills
        .ids()
        .map(|id| {
            let name = skills.get(id).name.rsplit(':').next().unwrap_or_default().to_owned();
            source.read(&format!("ui/skill/{name}")).or_else(|| unknown.clone()).map(|art| images.add(ui_image(&art))).unwrap_or_default()
        })
        .collect();
    // Each effect's icon, `ui/effect/<icon>` (content's effects.ron names it), or the plain
    // one for a buff or a debuff.
    let mut picture = |name: &str| source.read(name).map(|art| images.add(ui_image(&art)));
    let (buff, debuff) = (picture("ui/effect/buff").unwrap_or_default(), picture("ui/effect/debuff").unwrap_or_default());
    let effects = &launch.content.effects;
    let effects = effects
        .ids()
        .map(|id| {
            let def = effects.get(id);
            let plain = if def.kind == voxmar_engine::effect::Kind::Buff { &buff } else { &debuff };
            picture(&format!("ui/effect/{}", def.icon)).unwrap_or_else(|| plain.clone())
        })
        .collect();
    commands.insert_resource(HudImages { icons, crosshair, skills: skill_icons, effects });
    commands.insert_resource(handled);
}

fn tint(texture: &mut Rgba, color: [f32; 3]) {
    for pixel in texture.pixels.as_chunks_mut::<4>().0 {
        for c in 0..3 {
            pixel[c] = (pixel[c] as f32 * color[c]) as u8;
        }
    }
}

/// Draws `overlay` over `texture` (the same size) by its alpha.
fn blend_over(texture: &mut Rgba, overlay: &Rgba) {
    for (pixel, over) in texture.pixels.as_chunks_mut::<4>().0.iter_mut().zip(overlay.pixels.as_chunks::<4>().0) {
        let a = over[3] as f32 / 255.0;
        for c in 0..3 {
            pixel[c] = (pixel[c] as f32 * (1.0 - a) + over[c] as f32 * a).round() as u8;
        }
    }
}

/// Stands in for Bevy's audio plugin in a silent run: the game's sounds are still made and
/// spawned, but with no audio output nothing plays them. One-shot sounds are despawned as
/// they arrive, as playing would have; loops and music simply never start.
fn silence(app: &mut App) {
    use bevy::audio::{AudioLoader, AudioPlayer, AudioSource, GlobalVolume, PlaybackMode, PlaybackSettings, Volume};
    fn drop_sounds(mut commands: Commands, sounds: Query<(Entity, &PlaybackSettings), Added<AudioPlayer>>) {
        for (entity, settings) in &sounds {
            match settings.mode {
                PlaybackMode::Despawn => commands.entity(entity).despawn(),
                PlaybackMode::Remove => {
                    commands.entity(entity).remove::<(AudioPlayer, PlaybackSettings)>();
                }
                PlaybackMode::Once | PlaybackMode::Loop => {}
            }
        }
    }
    app.init_asset::<AudioSource>()
        .init_asset_loader::<AudioLoader>()
        .insert_resource(GlobalVolume::new(Volume::SILENT))
        .add_systems(PostUpdate, drop_sounds);
}
