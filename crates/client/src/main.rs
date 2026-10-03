//! The voxmar client.
//!
//! `voxmar` starts singleplayer with an integrated server. Options:
//! `--connect HOST:PORT` join a server, `--host` also open singleplayer to the network,
//! `--seed N`, `--name NAME`, `--view-distance N` (columns), `--vsync`,
//! `--capture PATH` (screenshot and exit once loaded).

mod capture;
mod hud;
mod interact;
mod net;
mod player;
mod render;
mod textures;
mod world;

use std::sync::Arc;

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::camera::Hdr;
use bevy::window::PresentMode;
use voxmar_engine::block::{BlockRegistry, Tint};
use voxmar_engine::coords::{CHUNK_SIZE, Face};
use voxmar_protocol::{DEFAULT_PORT, local_pair, tcp};
use voxmar_server::{ServerConfig, ServerHandle};

use crate::hud::HudImages;
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
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        connect: None,
        host: false,
        seed: ServerConfig::default().seed,
        name: std::env::var("USER").unwrap_or_else(|_| "player".into()),
        view_distance: 16,
        vsync: false,
        capture: None,
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
            "--vsync" => options.vsync = true,
            "--capture" => options.capture = Some(value()?.into()),
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
    let registry = voxmar_server::load_content(&assets)?;
    let (connection, server) = connect(&options, registry.clone())?;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "voxmar".into(),
                    present_mode: if options.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin { file_path: assets.to_string_lossy().into_owned(), ..default() }),
    )
    .add_plugins((
        FrameTimeDiagnosticsPlugin::default(),
        render::RenderPlugin,
        world::WorldPlugin,
        player::PlayerPlugin,
        interact::InteractPlugin,
        net::NetPlugin,
        hud::HudPlugin,
    ))
    .insert_resource(ClientWorld::new(registry.clone(), options.view_distance))
    .insert_resource(net::Net::new(connection, server, options.name.clone(), registry.fingerprint()))
    .insert_resource(render::DayCycle { view_distance: (options.view_distance * CHUNK_SIZE) as f32, ..default() })
    .insert_resource(Registry(registry))
    .add_systems(Startup, setup);
    if let Some(path) = options.capture {
        app.add_plugins(capture::CapturePlugin(path));
    }
    app.run();
    Ok(())
}

#[derive(Resource)]
struct Registry(Arc<BlockRegistry>);

fn connect(
    options: &Options,
    registry: Arc<BlockRegistry>,
) -> Result<(voxmar_protocol::ClientConnection, Option<ServerHandle>), Box<dyn std::error::Error>> {
    if let Some(addr) = &options.connect {
        let addr = if addr.contains(':') { addr.clone() } else { format!("{addr}:{DEFAULT_PORT}") };
        return Ok((tcp::connect(addr)?, None));
    }
    let config = ServerConfig { seed: options.seed, view_distance: options.view_distance, allow_flight: true };
    let server = voxmar_server::spawn(config, registry)?;
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
) {
    let registry = &registry.0;
    let mut source = Source::discover();
    let (_, block_textures) = source.block_textures(registry.textures());
    let colormaps = source.colormaps();
    render::setup(&mut commands, &mut images, &mut meshes, &mut voxel_materials, &mut sky_materials, &block_textures, &colormaps);

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

    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 75f32.to_radians(), near: 0.05, ..default() }),
        Hdr,
        Tonemapping::AcesFitted,
        Bloom { intensity: 0.08, ..Bloom::NATURAL },
        Transform::from_xyz(0.0, 100.0, 0.0),
    ));
}

fn tint(texture: &mut Rgba, color: [f32; 3]) {
    for pixel in texture.pixels.chunks_exact_mut(4) {
        for c in 0..3 {
            pixel[c] = (pixel[c] as f32 * color[c]) as u8;
        }
    }
}
