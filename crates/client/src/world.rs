//! Client copy of the world: applies server updates, relights and remeshes columns on
//! worker threads, and hands the meshes to the terrain renderer.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, COLUMN_CHUNKS, ChunkPos, ColumnPos};
use voxmar_engine::light::{LIGHT_PAD, LitRegion, Neighborhood};
use voxmar_engine::mesh::{ChunkMesh, MeshBuffers, mesh_chunk};
use voxmar_engine::visibility::{Connectivity, visible_among};
use voxmar_engine::world::{Column, VoxelWorld};

use crate::render::TerrainPass;
use crate::render::terrain::Terrain;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (dispatch_jobs, apply_meshes, cull_chunks).chain());
    }
}

#[derive(Default)]
struct ChunkRender {
    /// Which `TerrainPass`es have a mesh.
    passes: [bool; 5],
    hashes: [u64; 5],
    quads: [usize; 5],
    blades_hash: u64,
    connectivity: Connectivity,
}

/// A finished relight+remesh job.
struct MeshResult {
    pos: ColumnPos,
    /// The level of detail it was meshed at (see `mesh_chunk`).
    detail: i32,
    chunks: Vec<ChunkResult>,
}

struct ChunkResult {
    connectivity: Connectivity,
    passes: [PassResult; 5],
    blades: Vec<u32>,
    blades_hash: u64,
}

struct PassResult {
    hash: u64,
    quads: usize,
    mesh: MeshBuffers,
}

impl PassResult {
    fn new(mesh: MeshBuffers) -> Self {
        let mut hasher = FxHasher::default();
        mesh.vertices.hash(&mut hasher);
        mesh.indices.hash(&mut hasher);
        Self { hash: hasher.finish(), quads: mesh.vertices.len() / 4, mesh }
    }
}

impl ChunkResult {
    fn new(mesh: ChunkMesh) -> Self {
        let passes = [mesh.opaque, mesh.cutout, mesh.inner, mesh.translucent, mesh.fringe].map(PassResult::new);
        let mut hasher = FxHasher::default();
        mesh.blades.hash(&mut hasher);
        Self { connectivity: mesh.connectivity, passes, blades: mesh.blades, blades_hash: hasher.finish() }
    }
}

/// Quads handed to the renderer per frame (about 1.7 MB of vertex and index data), so
/// streaming and edits never cause a long frame.
const UPLOAD_BUDGET: usize = 32_000;

/// The level of detail a column `distance` columns from the camera is meshed at: full
/// detail out to `range[0]` columns, two blocks merged into one along x and z out to
/// `range[1]`, four beyond (see `mesh_chunk` and `settings::FarDetail`). A column keeps its
/// `current` detail until it is a column past the boundary, so walking back and forth over
/// one does not remesh.
fn detail_at(distance: i32, current: Option<i32>, range: [i32; 2]) -> i32 {
    let wanted = |distance: i32| match distance {
        d if d <= range[0] => 1,
        d if d <= range[1] => 2,
        _ => 4,
    };
    match current {
        Some(current) if (wanted(distance - 1)..=wanted(distance + 1)).contains(&current) => current,
        _ => wanted(distance),
    }
}

const PASSES: [TerrainPass; 5] =
    [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Inner, TerrainPass::Translucent, TerrainPass::Fringe];

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
    /// The level of detail each meshed column was meshed at, and how far out each level
    /// reaches (see `detail_at`).
    details: FxHashMap<ColumnPos, i32>,
    detail_range: [i32; 2],
    /// Set when the detail range changed, so every column's detail is checked again.
    details_dirty: bool,
    /// The column the camera was over when details were last checked.
    detail_center: ColumnPos,
    terrain: Terrain,
    /// Set when chunk meshes or connectivity changed, so culling must rerun.
    visibility_dirty: bool,
    culler: Culler,
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
    pub fn new(registry: Arc<BlockRegistry>, view_distance: i32, terrain: Terrain) -> Self {
        let (results_tx, results_rx) = unbounded();
        Self {
            voxels: VoxelWorld::default(),
            registry,
            dirty: FxHashSet::default(),
            in_flight: FxHashSet::default(),
            results_tx,
            results_rx,
            chunks: FxHashMap::default(),
            details: FxHashMap::default(),
            detail_range: [i32::MAX; 2],
            details_dirty: false,
            detail_center: ColumnPos::new(0, 0),
            terrain,
            visibility_dirty: true,
            culler: Culler::spawn(),
            view_distance,
            stats: WorldStats::default(),
        }
    }

    /// Forgets every column and its meshes, for leaving a world. Jobs still running
    /// deliver to a channel nobody reads any more.
    pub fn reset(&mut self, view_distance: i32) {
        self.terrain.clear();
        let detail_range = self.detail_range;
        *self = Self::new(self.registry.clone(), view_distance, self.terrain.clone());
        self.detail_range = detail_range;
    }

    /// How far out chunks keep full and half detail, in columns.
    pub fn set_far_detail(&mut self, range: [i32; 2]) {
        if self.detail_range != range {
            self.detail_range = range;
            self.details_dirty = true;
        }
    }

    pub fn set_view_distance(&mut self, view_distance: i32) {
        self.view_distance = view_distance;
        self.visibility_dirty = true;
    }

    pub fn insert_column(&mut self, pos: ColumnPos, column: Arc<Column>) {
        self.terrain.upload_column(pos, column.colors.clone());
        self.voxels.insert(pos, column);
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.dirty.insert(pos.offset(dx, dz));
            }
        }
    }

    pub fn remove_column(&mut self, pos: ColumnPos) {
        self.voxels.remove(pos);
        self.dirty.remove(&pos);
        for cy in 0..COLUMN_CHUNKS {
            let chunk = pos.chunk(cy);
            if let Some(render) = self.chunks.remove(&chunk) {
                for (pass, _) in PASSES.iter().zip(render.passes).filter(|(_, present)| *present) {
                    self.terrain.remove(chunk, *pass);
                }
                self.terrain.upload_blades(chunk, Vec::new());
            }
        }
        self.terrain.remove_column(pos);
        self.details.remove(&pos);
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

    /// Whether `pos` waits to be meshed, or is being meshed.
    pub fn meshing(&self, pos: ColumnPos) -> bool {
        self.dirty.contains(&pos) || self.in_flight.contains(&pos)
    }

    /// Checks that the renderer has `pos` as its blocks are now: no remesh is waiting or
    /// running, and meshing it again gives exactly what was handed over. `--gate` runs this
    /// after edits, to catch an edit the client knows of that never reaches the screen.
    pub fn check_meshed(&self, pos: ColumnPos) -> Result<(), String> {
        if self.meshing(pos) {
            return Err(format!("column {:?} is still waiting to be meshed", pos.0));
        }
        let columns = self.neighborhood(pos).ok_or_else(|| format!("column {:?} lacks neighbours to mesh with", pos.0))?;
        let detail = *self.details.get(&pos).ok_or_else(|| format!("column {:?} was never meshed", pos.0))?;
        let region = LitRegion::compute(&self.registry, &columns);
        for cy in 0..COLUMN_CHUNKS {
            let fresh = ChunkResult::new(mesh_chunk(&self.registry, &region, &columns[4], cy, detail));
            let shown = self.chunks.get(&pos.chunk(cy)).ok_or_else(|| format!("chunk {cy} of column {:?} has no mesh", pos.0))?;
            let stale = PASSES.iter().zip(&fresh.passes).zip(shown.hashes).find(|((_, fresh), shown)| fresh.hash != *shown);
            if let Some(((pass, _), _)) = stale {
                return Err(format!("chunk {cy} of column {:?} shows an old {pass:?} mesh", pos.0));
            }
            if fresh.blades_hash != shown.blades_hash {
                return Err(format!("chunk {cy} of column {:?} shows old grass blades", pos.0));
            }
        }
        Ok(())
    }

    /// Whether the column and its eight neighbours are all loaded, so it can be meshed.
    fn meshable(&self, pos: ColumnPos) -> bool {
        (-1..=1).all(|dz| (-1..=1).all(|dx| self.voxels.contains(pos.offset(dx, dz))))
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

/// Starts relight+remesh jobs for dirty columns, nearest to the camera first, and remeshes
/// columns whose level of detail changed as the camera moved.
fn dispatch_jobs(
    mut world: ResMut<ClientWorld>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut last_center: Local<Option<ColumnPos>>,
) {
    let max_in_flight = rayon::current_num_threads() * 2;
    let center = camera
        .single()
        .map(|t| ColumnPos::of_block(t.translation().floor().as_ivec3()))
        .unwrap_or(ColumnPos::new(0, 0));

    let world = &mut *world;
    world.detail_center = center;
    if *last_center != Some(center) || world.details_dirty {
        *last_center = Some(center);
        world.details_dirty = false;
        for (&pos, &detail) in &world.details {
            if detail_at(pos.distance(center), Some(detail), world.detail_range) != detail {
                world.dirty.insert(pos);
            }
        }
    }
    let voxels = &world.voxels;
    world.dirty.retain(|pos| voxels.contains(*pos));
    // Columns at the edge of the loaded area wait for neighbours. While the world streams
    // in, thousands wait, so this pass runs every frame: it only looks columns up.
    let mut meshable = 0;
    let mut ready = Vec::new();
    for &pos in &world.dirty {
        if world.meshable(pos) {
            meshable += 1;
            if !world.in_flight.contains(&pos) {
                ready.push(pos);
            }
        }
    }
    // The nearest first, sorting only as many as there are free jobs.
    let free = max_in_flight.saturating_sub(world.in_flight.len()).min(ready.len());
    if free < ready.len() {
        ready.select_nth_unstable_by_key(free, |pos| pos.distance(center));
        ready.truncate(free);
    }
    ready.sort_unstable_by_key(|pos| pos.distance(center));

    let mut dispatched = 0;
    for pos in ready {
        let Some(columns) = world.neighborhood(pos) else { continue };
        dispatched += 1;
        world.dirty.remove(&pos);
        world.in_flight.insert(pos);
        let registry = world.registry.clone();
        let tx = world.results_tx.clone();
        let detail = detail_at(pos.distance(center), world.details.get(&pos).copied(), world.detail_range);
        rayon::spawn(move || {
            let region = LitRegion::compute(&registry, &columns);
            let chunks = (0..COLUMN_CHUNKS)
                .map(|cy| ChunkResult::new(mesh_chunk(&registry, &region, &columns[4], cy, detail)))
                .collect();
            let _ = tx.send(MeshResult { pos, detail, chunks });
        });
    }
    world.stats.jobs_in_flight = world.in_flight.len();
    world.stats.pending_columns = meshable - dispatched;
}

/// Hands finished jobs to the renderer, skipping meshes that did not change.
fn apply_meshes(mut world: ResMut<ClientWorld>) {
    let world = &mut *world;
    let mut uploaded = 0;
    while uploaded < UPLOAD_BUDGET
        && let Ok(result) = world.results_rx.try_recv()
    {
        world.in_flight.remove(&result.pos);
        if !world.voxels.contains(result.pos) {
            continue;
        }
        world.details.insert(result.pos, result.detail);
        // Meshed for where the camera was when the job started: if it has moved since
        // (say, the player arriving at their spawn), the column may want another level,
        // which no later check would notice once the camera stops.
        if detail_at(result.pos.distance(world.detail_center), Some(result.detail), world.detail_range) != result.detail {
            world.dirty.insert(result.pos);
        }
        for (cy, chunk_result) in result.chunks.into_iter().enumerate() {
            let chunk = result.pos.chunk(cy as i32);
            let render = world.chunks.entry(chunk).or_default();
            if render.connectivity != chunk_result.connectivity {
                render.connectivity = chunk_result.connectivity;
                world.visibility_dirty = true;
            }
            if render.blades_hash != chunk_result.blades_hash {
                render.blades_hash = chunk_result.blades_hash;
                world.terrain.upload_blades(chunk, chunk_result.blades);
            }
            for (pass, result) in PASSES.into_iter().zip(chunk_result.passes) {
                let i = pass as usize;
                if render.hashes[i] == result.hash {
                    continue;
                }
                render.hashes[i] = result.hash;
                render.quads[i] = result.quads;
                uploaded += result.quads;
                let present = !result.mesh.is_empty();
                if present || render.passes[i] {
                    world.terrain.upload(chunk, pass, result.mesh);
                }
                world.visibility_dirty |= present != render.passes[i];
                render.passes[i] = present;
            }
        }
    }
    if uploaded > 0 {
        world.stats.chunk_entities = world.chunks.values().flat_map(|c| c.passes).filter(|&p| p).count();
        world.stats.quads = world.chunks.values().flat_map(|c| c.quads).sum();
    }
}

/// How long chunks that arrived wait at most before cave culling takes them in.
const CULL_AGAIN: f32 = 0.1;

/// Cave culling's walk, on a thread of its own: at view distance 32 it took 3-5 ms, which
/// on the main thread lengthened every frame it ran in (every `CULL_AGAIN` while the world
/// streams in) and set flying's p99 frame. The main thread hands it the chunks' connectivity
/// and takes the visible set back a frame or so later. Not a pool worker: those are busy
/// with streaming, and the walk would queue behind it. The thread ends with its world (a
/// reset makes a new one), so a walk of a world left behind is never applied.
struct Culler {
    jobs: Sender<CullJob>,
    results: Receiver<CullResult>,
    busy: bool,
}

struct CullJob {
    eye: IVec3,
    radius: i32,
    /// Every chunk's position, connectivity and quads.
    chunks: Vec<(ChunkPos, Connectivity, usize)>,
}

struct CullResult {
    visible: voxmar_engine::visibility::VisibleChunks,
    count: usize,
    quads: usize,
}

impl Culler {
    fn spawn() -> Self {
        let (jobs, job_rx) = unbounded::<CullJob>();
        let (result_tx, results) = unbounded();
        let walk = move || {
            while let Ok(job) = job_rx.recv() {
                let visible = visible_among(job.eye, job.radius, job.chunks.iter().map(|&(pos, connectivity, _)| (pos, connectivity)));
                let (mut count, mut quads) = (0, 0);
                for &(pos, _, chunk_quads) in &job.chunks {
                    if visible.contains(pos) {
                        count += 1;
                        quads += chunk_quads;
                    }
                }
                if result_tx.send(CullResult { visible, count, quads }).is_err() {
                    return;
                }
            }
        };
        std::thread::Builder::new().name("cave-culling".into()).spawn(walk).expect("the cave culling thread starts");
        Self { jobs, results, busy: false }
    }
}

/// Hides chunks the camera cannot see through the terrain (cave culling).
fn cull_chunks(
    time: Res<Time<Real>>,
    mut world: ResMut<ClientWorld>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut last: Local<Option<(IVec3, f32)>>,
) {
    let world = &mut *world;
    if let Ok(result) = world.culler.results.try_recv() {
        world.culler.busy = false;
        world.stats.visible_chunks = result.count;
        world.stats.visible_quads = result.quads;
        world.terrain.set_visible(result.visible);
    }
    let Ok(camera) = camera.single() else { return };
    let eye = camera.translation().floor().as_ivec3();
    let camera_chunk = eye.div_euclid(IVec3::splat(CHUNK_SIZE));
    let now = time.elapsed_secs();
    // A new camera chunk walks again at once; chunks arriving (which mark visibility dirty
    // nearly every frame while the world streams in) at most every `CULL_AGAIN` seconds.
    // One walk at a time: one asked for meanwhile goes as soon as it is back.
    let moved = last.is_none_or(|(chunk, _)| chunk != camera_chunk);
    let due = world.visibility_dirty && last.is_none_or(|(_, at)| now - at >= CULL_AGAIN);
    if (!moved && !due) || world.culler.busy {
        return;
    }
    *last = Some((camera_chunk, now));
    world.visibility_dirty = false;

    let _span = info_span!("cull_chunks").entered();
    // Search as far as columns are loaded. The camera can be away from where they were
    // loaded around (a capture's pose, the drifting menu camera, a player who has just
    // moved); a meshed column the search left out would be drawn neither by itself nor by
    // distant terrain, which gives way to every meshed column, and the hole showed as a
    // dark slab of sea floor at the edge of the loaded chunks.
    let camera_column = ColumnPos::of_block(eye);
    let radius = world.voxels.columns().map(|(pos, _)| (pos.0 - camera_column.0).abs().max_element()).max().unwrap_or(0);
    let chunks = world.chunks.iter().map(|(&pos, c)| (pos, c.connectivity, c.quads.iter().sum())).collect();
    let job = CullJob { eye, radius: radius.max(world.view_distance), chunks };
    world.culler.busy = world.culler.jobs.send(job).is_ok();
}
