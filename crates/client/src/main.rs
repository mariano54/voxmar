//! The voxmar client.
//!
//! `voxmar` starts singleplayer with an integrated server. Options:
//! `--connect HOST:PORT` join a server, `--host` also open singleplayer to the network,
//! `--seed N`, `--name NAME`, `--view-distance N` (columns), `--no-vsync`, `--no-music`,
//! `--capture PATH` (screenshot and exit once loaded), with `--pose dx,dy,dz,yaw,pitch` (camera offset from the player)
//! or `--flight SECONDS` (fly forward first and report frame pacing).

mod capture;
mod hud;
mod interact;
mod music;
mod net;
mod player;
mod render;
mod sfx;
mod textures;
mod world;

use std::sync::Arc;
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use voxmar_engine::action::ActionRegistry;
use voxmar_engine::block::{BlockRegistry, RenderKind, Tint};
use voxmar_engine::content::Content;
use voxmar_engine::coords::Face;
use voxmar_protocol::{DEFAULT_PORT, local_pair, tcp};
use voxmar_server::{ServerConfig, ServerHandle};

use crate::hud::HudImages;
use crate::render::lod::{Lod, LodMaterial, LodPalette};
use crate::render::{SkyMaterial, VoxelMaterial, ui_image};
use crate::textures::{Rgba, Source};
use crate::world::ClientWorld;

struct Options {
    connect: Option<String>,
    host: bool,
    seed: u64,
    name: String,
    view_distance: i32,
    vsync: bool,
    capture: Option<std::path::PathBuf>,
    pose: Option<[f32; 5]>,
    flight: Option<f32>,
    music: bool,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        connect: None,
        host: false,
        seed: ServerConfig::default().seed,
        name: std::env::var("USER").unwrap_or_else(|_| "player".into()),
        view_distance: 16,
        vsync: true,
        capture: None,
        pose: None,
        flight: None,
        music: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--connect" => options.connect = Some(value()?),
            "--host" => options.host = true,
            "--seed" => options.seed = value()?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--name" => options.name = value()?,
            "--view-distance" => options.view_distance = value()?.parse().map_err(|e| format!("--view-distance: {e}"))?,
            "--no-vsync" => options.vsync = false,
            "--no-music" => options.music = false,
            "--capture" => options.capture = Some(value()?.into()),
            "--flight" => options.flight = Some(value()?.parse().map_err(|e| format!("--flight: {e}"))?),
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
    // the rest.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(2).max(2));
    rayon::ThreadPoolBuilder::new().num_threads(workers).thread_name(|i| format!("worker-{i}")).build_global()?;

    let assets = voxmar_server::assets_dir();
    let content = voxmar_server::load_content(&assets)?;
    let registry = content.blocks.clone();
    let fingerprint = content.fingerprint();
    let (connection, server) = connect(&options, content.clone())?;

    let mut app = App::new();
    let window = Window {
        title: "voxmar".into(),
        present_mode: if options.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
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
    ))
    .insert_resource(ClientWorld::new(registry.clone(), options.view_distance))
    .insert_resource(net::Net::new(connection, server, options.name.clone(), fingerprint))
    .insert_resource(Actions(content.actions))
    .insert_resource(render::DayCycle { view_distance: render::lod::view_distance(options.view_distance), ..default() })
    .insert_resource(Registry(registry))
    .add_systems(Startup, setup);
    if options.music && options.capture.is_none() {
        app.add_plugins(music::MusicPlugin);
    }
    if let Some(path) = options.capture {
        app.add_plugins(capture::CapturePlugin { path, pose: options.pose, flight: options.flight });
    }
    app.run();
    Ok(())
}

#[derive(Resource)]
struct Registry(Arc<BlockRegistry>);

/// Action definitions, shared with the server, for proposing and predicting actions.
#[derive(Resource)]
pub struct Actions(pub Arc<ActionRegistry>);

fn connect(
    options: &Options,
    content: Content,
) -> Result<(voxmar_protocol::ClientConnection, Option<ServerHandle>), Box<dyn std::error::Error>> {
    if let Some(addr) = &options.connect {
        let addr = if addr.contains(':') { addr.clone() } else { format!("{addr}:{DEFAULT_PORT}") };
        return Ok((tcp::connect(addr)?, None));
    }
    let config = ServerConfig { seed: options.seed, view_distance: options.view_distance, allow_flight: true };
    let server = voxmar_server::spawn(config, content)?;
    if options.host {
        let addr = tcp::listen(("0.0.0.0", DEFAULT_PORT), server.connections())?;
        info!("open to the network on {addr}");
    }
    let (client, server_side) = local_pair();
    server.connections().send(server_side)?;
    Ok((client, Some(server)))
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    registry: Res<Registry>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut voxel_materials: ResMut<Assets<VoxelMaterial>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut lod_materials: ResMut<Assets<LodMaterial>>,
    world: Res<ClientWorld>,
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
    render::setup(&mut commands, &mut images, &mut meshes, &mut voxel_materials, &mut sky_materials, &block_textures, &colormaps);
    let palette = LodPalette::new(registry, &block_textures, &colormaps);
    commands.insert_resource(Lod::new(palette, world.view_distance, &mut lod_materials));

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
