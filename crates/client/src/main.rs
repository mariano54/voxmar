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
//! screenshot), `--menu` (show the settings menu), `--inventory` (show the inventory), `--character` (the character screen), `--tab NAME` (open that settings tab), `--guide SKILL` (open that skill's guide),
//! `--size WIDTHxHEIGHT` (the shot's size, 1920x1080 unless given), `--clip SECONDS` (save
//! every frame of that much game time at 30 fps instead, to check motion), `--windowed`
//! (draw to a real window and shoot that, for what only a window has, such as the render
//! scale; with the saved settings when `VOXMAR_CONFIG` is set), `--flicker` or
//! `--flicker-still` (measure shimmer; see capture.rs) or
//! `--screen title|worlds|create|join|settings` (capture a menu screen instead). `--gate DIR`
//! plays a scripted session headless and checks it (see gate.rs), saving its shots and
//! report in DIR. `--server-lag MS` makes every tick of the integrated server take that
//! much longer, as on a busy machine (gate runs default to `gate::SERVER_LAG`).
//! Other options: `--host` also open singleplayer worlds to the network, `--seed N` (for new
//! worlds made by `--world` or `--capture`), `--name NAME`, `--view-distance N` (columns),
//! `--fov DEGREES`, `--weather KIND[:INTENSITY]` (show that instead of the server's
//! weather; the kinds are in `weather.ron`, intensities 0..1 or light|moderate|heavy|max), `--command LINE` (run a console command once in game, such as
//! `"/time set noon"`; repeatable), `--no-vsync`, `--no-music`. `--view-distance`,
//! `--fov` and `--no-vsync` override the saved settings. Captures are silent, and so is
//! any run with the environment variable `VOXMAR_MUTE` set.

mod ambience;
mod breaking;
mod capture;
mod drops;
mod gate;
mod hud;
mod interact;
mod keys;
mod menu;
mod music;
mod net;
mod player;
mod render;
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
    /// `--tab NAME`: the settings tab to open; `--guide SKILL`: the skill guide to open.
    tab: Option<String>,
    guide: Option<String>,
    flicker: Option<capture::FlickerKind>,
    clip: Option<f32>,
    /// `--press left|right`: hold a mouse button at the start of the clip.
    press: Option<MouseButton>,
    size: UVec2,
    windowed: bool,
    screen: Option<Screen>,
    music: bool,
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
        tab: None,
        guide: None,
        flicker: None,
        clip: None,
        press: None,
        size: UVec2::new(1920, 1080),
        windowed: false,
        screen: None,
        music: true,
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
            "--tab" => options.tab = Some(value()?),
            "--guide" => options.guide = Some(value()?),
            "--flicker" => options.flicker = Some(capture::FlickerKind::Step),
            "--flicker-still" => options.flicker = Some(capture::FlickerKind::Still),
            "--clip" => options.clip = Some(value()?.parse().map_err(|e| format!("--clip: {e}"))?),
            "--press" => {
                options.press = Some(match value()?.as_str() {
                    "left" => MouseButton::Left,
                    "right" => MouseButton::Right,
                    other => return Err(format!("--press: `{other}` is not left or right")),
                })
            }
            "--windowed" => options.windowed = true,
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
    // A gate run is a capture that plays and checks a session rather than taking a shot.
    let headless = options.capture.is_some() || options.gate.is_some();
    // Leave a core for the main and render threads; worldgen, lighting and meshing share
    // the rest, at a lower priority so bursts of streaming never delay a frame.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(2).max(2));
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|i| format!("worker-{i}"))
        .start_handler(|_| lower_thread_priority())
        .build_global()?;

    let assets = voxmar_server::assets_dir();
    let content = voxmar_server::load_content(&assets)?;
    let content_weather = content.weather.clone();
    let shader_packs = render::pack::ShaderPacks::load(&assets);
    let registry = content.blocks.clone();
    let world_content = Arc::new(WorldContent::load(&content)?);
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
        .set(bevy::pbr::PbrPlugin { use_gpu_instance_buffer_builder: false, ..default() });
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
    ));
    let terrain = app.world().resource::<render::terrain::Terrain>().clone();
    app.insert_resource(ClientWorld::new(registry.clone(), settings.view_distance, terrain))
    .insert_resource(Actions(content.actions))
    .insert_resource(render::DayCycle { view_distance: render::lod::view_distance(settings.view_distance), ..default() })
    .insert_resource(shader_packs)
    .insert_resource(settings)
    .insert_resource(Registry { blocks: registry, world: world_content })
    .add_systems(Startup, setup);
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
            None => None,
        };
        let gate_lag = options.gate.as_ref().map(|_| gate::SERVER_LAG);
        let asked = options.server_lag.map(|ms| voxmar_server::Lag { per_tick: Duration::from_millis(ms), ..default() });
        let lag = asked.or(gate_lag).unwrap_or_default();
        let config = ServerConfig { seed: options.seed, world, lag, ..ServerConfig::default() };
        app.insert_resource(launch.singleplayer(config)?);
        Screen::InGame
    } else {
        Screen::Title
    };
    app.insert_resource(FirstScreen(first)).insert_resource(launch);
    if let Some(tab) = options.tab {
        app.insert_resource(settings::StartTab(tab));
    }
    if let Some(skill) = options.guide {
        app.insert_resource(hud::character::StartGuide(skill));
    }
    // Slow frames of real play are recorded for later (see slow_frames.rs); captures and
    // gate runs, which run on a busy machine by design, are not.
    // The renderer's part of a slow frame's record is always there to be set.
    app.init_resource::<slow_frames::SlowFrameExtra>();
    if !headless {
        app.add_plugins(slow_frames::SlowFramesPlugin);
    }
    if options.music && !headless {
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
            press: options.press,
            size: options.size,
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

/// Lowers the calling thread's scheduling priority below the game's frame threads.
fn lower_thread_priority() {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_UTILITY: u32 = 0x11;
        // SAFETY: only changes the scheduling class of the current thread.
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
    }
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn nice(increment: i32) -> i32;
        }
        // SAFETY: on Linux `nice` affects only the calling thread.
        unsafe { nice(5) };
    }
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
    let mut source = Source::discover(settings.textures);
    let (size, mut block_textures) = source.block_textures(registry.textures());
    // Opaque blocks must not show see-through texels; cut-out and translucent ones keep them.
    let mut see_through = vec![false; block_textures.len()];
    for id in 0..registry.len() {
        let info = registry.info(voxmar_engine::block::BlockId(id as u16));
        if !matches!(info.render, RenderKind::Opaque | RenderKind::Invisible) {
            for layer in info.layers {
                see_through[layer as usize] = true;
            }
        }
    }
    for (texture, keep) in block_textures.iter_mut().zip(see_through) {
        if !keep {
            texture.fill_transparent();
        }
    }
    // Blocks that give off light glow by themselves, as brightly as the light they give.
    let mut layers = vec![LayerInfo::default(); block_textures.len()];
    for id in 0..registry.len() {
        let info = registry.info(voxmar_engine::block::BlockId(id as u16));
        for layer in info.layers {
            let glow = &mut layers[layer as usize].glow;
            *glow = glow.max(info.emission as f32 / 15.0);
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
    ui::skin::load(&mut source, &mut images);
    ui::skin::set_font(settings.font, &mut fonts);
    let colormaps = source.colormaps();
    render::setup(&mut commands, &mut images, &mut meshes, &mut sky_materials, &block_textures, &layers);
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
        let icon = own.unwrap_or_else(|| match info.render {
            RenderKind::Plant | RenderKind::Invisible => face_texture(info, Face::PosZ),
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
    commands.insert_resource(HudImages { icons, crosshair, skills: skill_icons });
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
