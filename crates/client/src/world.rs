//! Client copy of the world: applies server updates, relights and remeshes columns on
//! worker threads, and keeps one entity per non-empty chunk mesh.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ChunkPos, ColumnPos};
use voxmar_engine::light::{LIGHT_PAD, LitRegion, Neighborhood};
use voxmar_engine::mesh::{ChunkMesh, MeshBuffers, mesh_chunk};
use voxmar_engine::visibility::{Connectivity, visible_chunks};
use voxmar_engine::world::{Column, VoxelWorld};

use crate::render::{VoxelMaterials, build_mesh};

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (dispatch_jobs, apply_meshes, cull_chunks).chain());
    }
}

#[derive(Default)]
struct ChunkRender {
    entities: [Option<Entity>; 2],
    hashes: [u64; 2],
    quads: [usize; 2],
    connectivity: Connectivity,
}

struct MeshResult {
    pos: ColumnPos,
    meshes: Vec<ChunkMesh>,
}

#[derive(Resource)]
pub struct ClientWorld {
    pub voxels: VoxelWorld,
    pub registry: Arc<BlockRegistry>,
    /// Columns whose meshes are out of date.
    dirty: FxHashSet<ColumnPos>,
    in_flight: FxHashSet<ColumnPos>,
    results_tx: Sender<MeshResult>,
    results_rx: Receiver<MeshResult>,
    chunks: FxHashMap<ChunkPos, ChunkRender>,
    /// Set when chunk meshes or connectivity changed, so culling must rerun.
    visibility_dirty: bool,
    pub view_distance: i32,
    pub stats: WorldStats,
}

#[derive(Default, Clone, Copy)]
pub struct WorldStats {
    pub chunk_entities: usize,
    pub quads: usize,
    pub visible_quads: usize,
    pub visible_chunks: usize,
    pub jobs_in_flight: usize,
    pub pending_columns: usize,
}

impl ClientWorld {
    pub fn new(registry: Arc<BlockRegistry>, view_distance: i32) -> Self {
        let (results_tx, results_rx) = unbounded();
        Self {
            voxels: VoxelWorld::default(),
            registry,
            dirty: FxHashSet::default(),
            in_flight: FxHashSet::default(),
            results_tx,
            results_rx,
            chunks: FxHashMap::default(),
            visibility_dirty: true,
            view_distance,
            stats: WorldStats::default(),
        }
    }

    pub fn insert_column(&mut self, pos: ColumnPos, column: Arc<Column>) {
        self.voxels.insert(pos, column);
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.dirty.insert(pos.offset(dx, dz));
            }
        }
    }

    pub fn remove_column(&mut self, pos: ColumnPos, commands: &mut Commands) {
        self.voxels.remove(pos);
        self.dirty.remove(&pos);
        for cy in 0..COLUMN_CHUNKS {
            if let Some(render) = self.chunks.remove(&pos.chunk(cy)) {
                for entity in render.entities.into_iter().flatten() {
                    commands.entity(entity).despawn();
                }
            }
        }
        self.visibility_dirty = true;
    }

    pub fn set_block(&mut self, pos: IVec3, block: BlockId) {
        if self.voxels.set_block(pos, block).is_none() {
            return;
        }
        // Remesh every column whose light region (column ± LIGHT_PAD) contains the block.
        let column = ColumnPos::of_block(pos);
        let local = pos & (CHUNK_SIZE - 1);
        let reach = |l: i32| {
            let low = if l < LIGHT_PAD { -1 } else { 0 };
            let high = if l >= CHUNK_SIZE - LIGHT_PAD { 1 } else { 0 };
            low..=high
        };
        for dz in reach(local.z) {
            for dx in reach(local.x) {
                self.dirty.insert(column.offset(dx, dz));
            }
        }
    }

    pub fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.voxels.block(pos)
    }

    pub fn is_solid(&self, pos: IVec3) -> bool {
        // Unloaded space is solid so the player cannot fall out of the loaded world.
        self.block(pos).is_none_or(|b| self.registry.info(b).solid)
    }

    fn neighborhood(&self, pos: ColumnPos) -> Option<Neighborhood> {
        let mut columns = Vec::with_capacity(9);
        for dz in -1..=1 {
            for dx in -1..=1 {
                columns.push(self.voxels.column(pos.offset(dx, dz))?.clone());
            }
        }
        columns.try_into().ok()
    }
}

/// Starts relight+remesh jobs for dirty columns, nearest to the camera first.
fn dispatch_jobs(mut world: ResMut<ClientWorld>, camera: Query<&GlobalTransform, With<Camera3d>>) {
    let max_in_flight = rayon::current_num_threads() * 2;
    let center = camera
        .single()
        .map(|t| ColumnPos::of_block(t.translation().floor().as_ivec3()))
        .unwrap_or(ColumnPos::new(0, 0));

    let world = &mut *world;
    let voxels = &world.voxels;
    world.dirty.retain(|pos| voxels.contains(*pos));
    let mut ready: Vec<ColumnPos> = world
        .dirty
        .iter()
        .copied()
        .filter(|pos| !world.in_flight.contains(pos) && world.voxels.contains(*pos))
        .collect();
    ready.sort_unstable_by_key(|pos| pos.distance(center));

    for pos in ready {
        if world.in_flight.len() >= max_in_flight {
            break;
        }
        let Some(columns) = world.neighborhood(pos) else { continue };
        world.dirty.remove(&pos);
        world.in_flight.insert(pos);
        let registry = world.registry.clone();
        let tx = world.results_tx.clone();
        rayon::spawn(move || {
            let region = LitRegion::compute(&registry, &columns);
            let meshes = (0..COLUMN_CHUNKS).map(|cy| mesh_chunk(&registry, &region, &columns[4], cy)).collect();
            let _ = tx.send(MeshResult { pos, meshes });
        });
    }
    world.stats.jobs_in_flight = world.in_flight.len();
    // Columns at the edge of the loaded area wait for neighbours; only count meshable ones.
    world.stats.pending_columns = world.dirty.iter().filter(|p| world.neighborhood(**p).is_some()).count();
}

fn mesh_hash(buffers: &MeshBuffers) -> u64 {
    let mut hasher = FxHasher::default();
    buffers.vertices.hash(&mut hasher);
    buffers.indices.hash(&mut hasher);
    hasher.finish()
}

/// Turns finished jobs into chunk entities, skipping meshes that did not change.
fn apply_meshes(
    mut commands: Commands,
    mut world: ResMut<ClientWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    materials: Res<VoxelMaterials>,
) {
    let world = &mut *world;
    while let Ok(result) = world.results_rx.try_recv() {
        world.in_flight.remove(&result.pos);
        if !world.voxels.contains(result.pos) {
            continue;
        }
        for (cy, mesh) in result.meshes.into_iter().enumerate() {
            let chunk = result.pos.chunk(cy as i32);
            let render = world.chunks.entry(chunk).or_default();
            if render.connectivity != mesh.connectivity {
                render.connectivity = mesh.connectivity;
                world.visibility_dirty = true;
            }
            let passes = [(&mesh.opaque, &materials.opaque), (&mesh.translucent, &materials.translucent)];
            for (pass, (buffers, material)) in passes.into_iter().enumerate() {
                let hash = mesh_hash(buffers);
                if render.hashes[pass] == hash {
                    continue;
                }
                render.hashes[pass] = hash;
                render.quads[pass] = buffers.vertices.len() / 4;
                match (buffers.is_empty(), render.entities[pass]) {
                    (true, Some(entity)) => {
                        commands.entity(entity).despawn();
                        render.entities[pass] = None;
                    }
                    (true, None) => {}
                    (false, Some(entity)) => {
                        commands.entity(entity).insert(Mesh3d(meshes.add(build_mesh(buffers))));
                    }
                    (false, None) => {
                        let entity = commands
                            .spawn((
                                Mesh3d(meshes.add(build_mesh(buffers))),
                                MeshMaterial3d(material.clone()),
                                Transform::from_translation(chunk.origin().as_vec3()),
                                Aabb::from_min_max(Vec3::ZERO, Vec3::splat(CHUNK_SIZE as f32)),
                            ))
                            .id();
                        render.entities[pass] = Some(entity);
                        world.visibility_dirty = true;
                    }
                }
            }
        }
    }
    world.stats.chunk_entities = world.chunks.values().flat_map(|c| c.entities.iter().flatten()).count();
    world.stats.quads = world.chunks.values().flat_map(|c| c.quads).sum();
}

/// Hides chunks the camera cannot see through the terrain (cave culling).
fn cull_chunks(
    mut world: ResMut<ClientWorld>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
    mut last_chunk: Local<Option<IVec3>>,
) {
    let Ok(camera) = camera.single() else { return };
    let eye = camera.translation().floor().as_ivec3();
    let camera_chunk = eye.div_euclid(IVec3::splat(CHUNK_SIZE));
    if !world.visibility_dirty && *last_chunk == Some(camera_chunk) {
        return;
    }
    *last_chunk = Some(camera_chunk);
    world.visibility_dirty = false;

    let _span = info_span!("cull_chunks").entered();
    let visible = visible_chunks(eye, world.view_distance, |pos| world.chunks.get(&pos).map(|c| c.connectivity));
    let (mut count, mut quads) = (0, 0);
    for (pos, render) in &world.chunks {
        let shown = visible.contains(pos);
        count += shown as usize;
        quads += if shown { render.quads.iter().sum() } else { 0 };
        for entity in render.entities.iter().flatten() {
            if let Ok(mut v) = visibility.get_mut(*entity) {
                v.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
            }
        }
    }
    world.stats.visible_chunks = count;
    world.stats.visible_quads = quads;
}
