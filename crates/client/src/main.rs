//! The voxmar client.
//!
//! `voxmar` opens the title screen. Options that go straight into a game instead:
//! `--world NAME` play (or create) the saved world NAME, `--connect HOST[:PORT]` join a
//! server, `--capture PATH` (screenshot and exit once loaded; plays an unsaved world unless
//! `--world` or `--connect` is given) with `--pose dx,dy,dz,yaw,pitch` (camera offset from
//! the player) or `--flight SECONDS` (fly forward first and report frame pacing), `--time
//! FRACTION` (hold the time of day, 0 is sunrise), `--menu` (show the settings menu) or
//! `--screen title|worlds|create|join|settings` (capture a menu screen instead).
//! Other options: `--host` also open singleplayer worlds to the network, `--seed N` (for new
//! worlds made by `--world` or `--capture`), `--name NAME`, `--view-distance N` (columns),
//! `--fov DEGREES`, `--weather clear|cirrus|cumulus|cloudy|overcast|rain|storm|snow` (show
//! that instead of the server's weather), `--no-vsync`, `--no-music`. `--view-distance`,
//! `--fov` and `--no-vsync` override the saved settings.

mod capture;
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
mod textures;
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

use crate::hud::HudImages;
use crate::menu::{FirstScreen, Screen};
use crate::render::lod::{Lod, LodPalette};
use crate::render::{SkyMaterial, ui_image};
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
    weather: Option<voxmar_protocol::Weather>,
    capture: Option<std::path::PathBuf>,
    pose: Option<[f32; 5]>,
    flight: Option<f32>,
    time: Option<f32>,
    menu: bool,
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
        capture: None,
        pose: None,
        flight: None,
        time: None,
        menu: false,
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
                let name = value()?;
                options.weather = Some(weather::preset(&name).ok_or_else(|| format!("--weather: unknown weather {name}"))?);
            }
            "--no-music" => options.music = false,
            "--capture" => options.capture = Some(value()?.into()),
            "--flight" => options.flight = Some(value()?.parse().map_err(|e| format!("--flight: {e}"))?),
            "--time" => options.time = Some(value()?.parse().map_err(|e| format!("--time: {e}"))?),
            "--menu" => options.menu = true,
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
    let registry = content.blocks.clone();
    let launch = Launch { name: options.name.clone(), host: options.host, content: content.clone() };
    // Captures are benchmarks and visual checks, so they ignore the player's saved settings.
    let mut settings = if options.capture.is_some() { Settings::default() } else { Settings::load() };
    settings.view_distance = options.view_distance.unwrap_or(settings.view_distance);
    settings.fov = options.fov.unwrap_or(settings.fov);
    settings.vsync = options.vsync.unwrap_or(settings.vsync);
    let settings = settings.clamped();

    let mut app = App::new();
    let window = Window {
        title: "voxmar".into(),
        present_mode: if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
        ..default()
    };
    let plugins = DefaultPlugins.set(AssetPlugin { file_path: assets.to_string_lossy().into_owned(), ..default() });
    if options.capture.is_some() {
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
    app
    .add_plugins((
        FrameTimeDiagnosticsPlugin::default(),
        render::RenderPlugin,
        world::WorldPlugin,
        player::PlayerPlugin,
        interact::InteractPlugin,
        net::NetPlugin,
        hud::HudPlugin,
        sfx::SfxPlugin,
        settings::SettingsPlugin,
        menu::MenuPlugin,
        session::SessionPlugin,
        weather::WeatherPlugin,
    ));
    let terrain = app.world().resource::<render::terrain::Terrain>().clone();
    app.insert_resource(ClientWorld::new(registry.clone(), settings.view_distance, terrain))
    .insert_resource(Actions(content.actions))
    .insert_resource(render::DayCycle { view_distance: render::lod::view_distance(settings.view_distance), ..default() })
    .insert_resource(settings)
    .insert_resource(Registry(registry))
    .add_systems(Startup, setup);
    if let Some(forced) = options.weather {
        app.world_mut().resource_mut::<weather::ClientWeather>().force(forced);
    }

    // Go straight into a game when asked to, otherwise to the title screen.
    let first = if let Some(screen) = options.screen {
        screen
    } else if let Some(address) = &options.connect {
        app.insert_resource(menu::connect(address));
        Screen::Join
    } else if options.world.is_some() || options.capture.is_some() {
        let world = match &options.world {
            Some(name) => Some(menu::worlds_dir().ok_or("no configuration directory to keep worlds in")?.join(name)),
            None => None,
        };
        let config = ServerConfig { seed: options.seed, world, ..ServerConfig::default() };
        app.insert_resource(launch.singleplayer(config)?);
        Screen::InGame
    } else {
        Screen::Title
    };
    app.insert_resource(FirstScreen(first)).insert_resource(launch);
    if options.music && options.capture.is_none() {
        app.add_plugins(music::MusicPlugin);
    }
    if let Some(path) = options.capture {
        app.add_plugins(capture::CapturePlugin {
            path,
            pose: options.pose,
            flight: options.flight,
            time: options.time,
            menu: options.menu,
        });
    }
    app.run();
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

#[derive(Resource)]
struct Registry(Arc<BlockRegistry>);

/// Action definitions, shared with the server, for proposing and predicting actions.
#[derive(Resource)]
pub struct Actions(pub Arc<ActionRegistry>);

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    registry: Res<Registry>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    world: Res<ClientWorld>,
    terrain: Res<render::terrain::Terrain>,
) {
    let registry = &registry.0;
    let mut source = Source::discover();
    let (_, mut block_textures) = source.block_textures(registry.textures());
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
    let colormaps = source.colormaps();
    render::setup(&mut commands, &mut images, &mut meshes, &mut sky_materials, &block_textures, &colormaps);
    let palette = LodPalette::new(registry, &block_textures, &colormaps);
    commands.insert_resource(Lod::new(palette, terrain.clone(), world.view_distance));

    let icons = (0..registry.len())
        .map(|id| {
            let info = registry.info(voxmar_engine::block::BlockId(id as u16));
            let face = if info.tints[Face::PosZ as usize] != Tint::None { Face::PosY } else { Face::PosZ };
            let mut icon = block_textures[info.layers[face as usize] as usize].clone();
            if info.tints[face as usize] != Tint::None {
                tint(&mut icon, [0.45, 0.72, 0.3]);
            }
            images.add(ui_image(&icon))
        })
        .collect();
    let mut sprite = |name: &str, size: (u32, u32), color: [u8; 4]| images.add(ui_image(&source.sprite(name, (size.0, size.1, color))));
    commands.insert_resource(HudImages {
        icons,
        hotbar: sprite("hotbar", (182, 22), [0, 0, 0, 140]),
        selection: sprite("hotbar_selection", (24, 23), [255, 255, 255, 90]),
        crosshair: sprite("crosshair", (15, 15), [255, 255, 255, 0]),
        heart_full: sprite("heart/full", (9, 9), [220, 30, 30, 255]),
        heart_half: sprite("heart/half", (9, 9), [150, 20, 20, 255]),
        heart_container: sprite("heart/container", (9, 9), [40, 0, 0, 255]),
    });
}

fn tint(texture: &mut Rgba, color: [f32; 3]) {
    for pixel in texture.pixels.as_chunks_mut::<4>().0 {
        for c in 0..3 {
            pixel[c] = (pixel[c] as f32 * color[c]) as u8;
        }
    }
}
