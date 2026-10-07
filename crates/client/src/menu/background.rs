//! The menus' backdrop: a demo world under the real sky, with real chunks nearby and
//! distant terrain out to the horizon, seen from a camera that drifts and turns slowly.
//! The columns are generated right here on worker threads; no server runs.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, unbounded};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos};
use voxmar_engine::world::Column;
use voxmar_worldgen::{Generator, SEA_LEVEL};

use super::InMenu;
use crate::render::DayCycle;
use crate::render::lod::{self, WorldGenerator};
use crate::settings::Settings;
use crate::world::ClientWorld;

/// The demo world. The camera hovers over its spawn point, which the generator always
/// puts in green country.
const SEED: u64 = 0x5eed;
/// Radius, in columns, of real terrain around the camera; distant terrain does the rest.
const RADIUS: i32 = 6;
/// Height of the camera above the highest ground near it.
const ALTITUDE: f32 = 28.0;
const PITCH: f32 = -0.18;
/// Seconds for one full turn.
const TURN_SECONDS: f32 = 300.0;
/// Morning, with long soft shadows.
const START_TICKS: f64 = 1500.0;

pub struct BackgroundPlugin;

impl Plugin for BackgroundPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(InMenu), start)
            .add_systems(OnExit(InMenu), stop)
            .add_systems(Update, (receive_columns, move_camera).run_if(in_state(InMenu)));
    }
}

#[derive(Resource)]
struct Backdrop {
    columns: Receiver<(ColumnPos, Column)>,
    /// Tells generation jobs that have not started yet not to bother.
    cancelled: Arc<AtomicBool>,
    eye: Vec3,
    started: f32,
}

fn start(
    mut commands: Commands,
    time: Res<Time>,
    mut world: ResMut<ClientWorld>,
    mut world_generator: ResMut<WorldGenerator>,
    launch: Res<crate::session::Launch>,
    mut cycle: ResMut<DayCycle>,
    mut spawn: Local<Option<ColumnPos>>,
) {
    world.reset(RADIUS);
    // The same generator, with the loaded content packages, makes the real and distant
    // terrain.
    let generator = world_generator.start(SEED, &launch.content);
    // Searching for the spawn point takes a moment; it never changes.
    let center = *spawn.get_or_insert_with(|| ColumnPos::of_block(generator.spawn_point()));
    cycle.ticks = START_TICKS;
    cycle.view_distance = lod::view_distance(RADIUS);

    let (tx, columns) = unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut positions: Vec<ColumnPos> =
        (-RADIUS..=RADIUS).flat_map(|dz| (-RADIUS..=RADIUS).map(move |dx| center.offset(dx, dz))).collect();
    positions.sort_by_key(|pos| pos.distance(center));
    for pos in positions {
        let (generator, tx, cancelled) = (generator.clone(), tx.clone(), cancelled.clone());
        rayon::spawn(move || {
            if !cancelled.load(Ordering::Relaxed) {
                let _ = tx.send((pos, generator.generate(pos)));
            }
        });
    }

    // Hover over the middle of the centre column, clear of the ground the camera circles.
    let middle = center.origin() + IVec3::new(CHUNK_SIZE / 2, 0, CHUNK_SIZE / 2);
    let ground = (-2..=2)
        .flat_map(|dz| (-2..=2).map(move |dx| (dx * 8, dz * 8)))
        .map(|(dx, dz)| generator.height(middle.x + dx, middle.z + dz))
        .max()
        .unwrap_or(SEA_LEVEL)
        .max(SEA_LEVEL);
    let eye = Vec3::new(middle.x as f32, ground as f32 + ALTITUDE, middle.z as f32);
    commands.insert_resource(Backdrop { columns, cancelled, eye, started: time.elapsed_secs() });
}

fn stop(
    mut commands: Commands,
    backdrop: Option<Res<Backdrop>>,
    settings: Res<Settings>,
    mut world: ResMut<ClientWorld>,
    mut generator: ResMut<WorldGenerator>,
) {
    if let Some(backdrop) = backdrop {
        backdrop.cancelled.store(true, Ordering::Relaxed);
    }
    commands.remove_resource::<Backdrop>();
    world.reset(settings.view_distance);
    generator.clear();
}

fn receive_columns(backdrop: Res<Backdrop>, mut world: ResMut<ClientWorld>) {
    while let Ok((pos, column)) = backdrop.columns.try_recv() {
        world.insert_column(pos, Arc::new(column));
    }
}

/// Turns the camera slowly while it drifts around a small circle and bobs gently.
fn move_camera(time: Res<Time>, backdrop: Res<Backdrop>, mut camera: Query<&mut Transform, With<Camera3d>>) {
    let Ok(mut transform) = camera.single_mut() else { return };
    let t = time.elapsed_secs() - backdrop.started;
    let angle = t / TURN_SECONDS * TAU;
    let drift = Vec3::new((angle * 3.0).cos() * 6.0, (t * 0.25).sin() * 0.8, (angle * 3.0).sin() * 6.0);
    transform.translation = backdrop.eye + drift;
    transform.rotation = Quat::from_euler(EulerRot::YXZ, -angle, PITCH, 0.0);
}
