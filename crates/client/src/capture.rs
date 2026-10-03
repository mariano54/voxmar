//! `--capture PATH`: once the world around the player has loaded and settled, logs frame
//! statistics, saves a screenshot and exits. Used for visual checks and benchmarking.
//! Runs without a window and renders into an offscreen image, so frame times measure the
//! engine rather than the display, and it works with the display asleep.

use std::path::PathBuf;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::player::LocalPlayer;
use crate::world::ClientWorld;

pub struct CapturePlugin {
    pub path: PathBuf,
    /// Optional camera override: x, y, z, yaw, pitch (degrees).
    pub pose: Option<[f32; 5]>,
}

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Capture {
            path: self.path.clone(),
            pose: self.pose,
            target: Handle::default(),
            settled_at: None,
            taken_at: None,
        })
        .add_systems(PostStartup, render_offscreen)
        .add_systems(Update, (target_ui, capture))
        .add_systems(PostUpdate, apply_pose.before(TransformSystems::Propagate));
    }
}

#[derive(Resource)]
struct Capture {
    path: PathBuf,
    pose: Option<[f32; 5]>,
    target: Handle<Image>,
    settled_at: Option<f32>,
    taken_at: Option<f32>,
}

fn render_offscreen(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    camera: Query<Entity, With<Camera3d>>,
) {
    let mut image = Image::new_target_texture(1920, 1080, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    state.target = images.add(image);
    for entity in &camera {
        commands.entity(entity).insert(RenderTarget::Image(state.target.clone().into()));
    }
}

type UntargetedUiRoot = (With<Node>, Without<ChildOf>, Without<UiTargetCamera>);

/// UI follows the window by default; point it at the offscreen camera instead.
fn target_ui(
    mut commands: Commands,
    camera: Query<Entity, With<Camera3d>>,
    roots: Query<Entity, UntargetedUiRoot>,
) {
    let Ok(camera) = camera.single() else { return };
    for root in &roots {
        commands.entity(root).insert(UiTargetCamera(camera));
    }
}

fn apply_pose(state: Res<Capture>, mut camera: Query<&mut Transform, With<Camera3d>>) {
    let (Some([x, y, z, yaw, pitch]), Ok(mut transform)) = (state.pose, camera.single_mut()) else { return };
    transform.translation = Vec3::new(x, y, z);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw.to_radians(), pitch.to_radians(), 0.0);
}

fn capture(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<Capture>,
    world: Res<ClientWorld>,
    player: Res<LocalPlayer>,
    diagnostics: Res<DiagnosticsStore>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    if let Some(taken) = state.taken_at {
        if now - taken > 1.0 {
            exit.write(AppExit::Success);
        }
        return;
    }
    let stats = world.stats;
    let busy = !player.joined || stats.jobs_in_flight > 0 || stats.pending_columns > 0 || world.voxels.len() < 9;
    if busy && now < 60.0 {
        state.settled_at = None;
        return;
    }
    // Measure a few seconds of steady-state rendering before capturing.
    let settled = *state.settled_at.get_or_insert(now);
    if now - settled < 4.0 {
        return;
    }
    let fps = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.average()).unwrap_or(0.0);
    let frame = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.average()).unwrap_or(0.0);
    info!(
        "capture: {fps:.0} fps ({frame:.2} ms), {} columns, {} chunk meshes, {} quads ({} after cave culling), {} visible, {} jobs, {} pending, loaded in {settled:.1}s",
        world.voxels.len(),
        stats.chunk_entities,
        stats.quads,
        stats.visible_quads,
        stats.visible_chunks,
        stats.jobs_in_flight,
        stats.pending_columns,
    );
    commands.spawn(Screenshot::image(state.target.clone())).observe(save_to_disk(state.path.clone()));
    state.taken_at = Some(now);
}
