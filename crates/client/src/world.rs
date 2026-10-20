//! Client copy of the world: applies server updates, relights and remeshes columns on
//! worker threads, and hands the meshes to the terrain renderer.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

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
    /// Which job it was (`ClientWorld::in_flight`).
    job: u64,
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

/// Columns an edit changed within this many columns of the camera are remeshed at once,
/// on threads of their own (`EDITS`): what the player builds or digs, and what changes
/// around them.
const NEAR_EDITS: i32 = 2;

/// The threads near edits are relit and remeshed on, at the default class (priority.rs). On the
/// shared workers such a job waited behind every column being generated (up to 64 at a
/// time, by the server in this process) and meshed while the land streamed in, at a lower
/// priority: early in a new world, and on a busy machine, a block broken at spawn stood
/// there for many seconds after it broke. They are idle but for edits.
static EDITS: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .thread_name(|i| format!("edit-mesh-{i}"))
        .start_handler(|_| crate::priority::default_thread())
        .build()
        .expect("the edits' meshing threads start")
});

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
    /// Of those, the ones an edit changed (`set_block`), to be remeshed at once if near.
    edited: FxHashSet<ColumnPos>,
    /// Columns being meshed, with their newest job: only its result is shown. An edit's
    /// job can start while an older one waits on the shared workers, and finish first.
    in_flight: FxHashMap<ColumnPos, u64>,
    /// Of those, the ones whose newest job is on the edits' threads: one at a time each,
    /// or a column edited every tick (flowing water, fire) would start a job every frame,
    /// each superseding the last, and none would ever be shown.
    editing: FxHashSet<ColumnPos>,
    /// Jobs started so far.
    jobs: u64,
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
    /// The cores of lit structures whose fire burns, as the server last said
    /// (`ServerMessage::Fires`): a hearth crackles and smokes only while lit.
    pub fires: FxHashSet<IVec3>,
    /// Blocks smouldering, about to catch fire, as the server last said
    /// (`ServerMessage::Smoulders`): they smoke and crackle until they catch or are saved.
    pub smoulders: FxHashSet<IVec3>,
}

#[derive(Default, Clone, Copy)]
pub struct WorldStats {
    pub chunk_entities: usize,
    pub quads: usize,
    pub visible_quads: usize,
    pub visible_chunks: usize,
    pub jobs_in_flight: usize,
    /// Meshable columns waiting at the last dispatch scan. While all workers are busy,
    /// this count stays cached; the next free slot refreshes it before idle is reported.
    pub pending_columns: usize,
}

impl ClientWorld {
    pub fn new(registry: Arc<BlockRegistry>, view_distance: i32, terrain: Terrain) -> Self {
        let (results_tx, results_rx) = unbounded();
        Self {
            voxels: VoxelWorld::default(),
            registry,
            dirty: FxHashSet::default(),
            edited: FxHashSet::default(),
            in_flight: FxHashMap::default(),
            editing: FxHashSet::default(),
            jobs: 0,
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
            fires: FxHashSet::default(),
            smoulders: FxHashSet::default(),
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
        self.edited.remove(&pos);
        // A job still running was of what is gone.
        self.in_flight.remove(&pos);
        self.editing.remove(&pos);
        for cy in 0..COLUMN_CHUNKS {
            let chunk = pos.chunk(cy);
            if let Some(render) = self.chunks.remove(&chunk) {
                self.stats.chunk_entities -= render.passes.into_iter().filter(|&present| present).count();
                self.stats.quads -= render.quads.into_iter().sum::<usize>();
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
                self.edited.insert(column.offset(dx, dz));
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
        self.dirty.contains(&pos) || self.in_flight.contains_key(&pos)
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
    // Near edits first, at once, whatever the shared workers are busy with. Far ones (a
    // neighbour's, the server's) wait their turn with the rest of the dirty columns.
    // One whose last edit is still being meshed there waits for it, edited still, and
    // goes as soon as it is shown; so does one whose neighbours have not all arrived.
    let edited: Vec<ColumnPos> = world.edited.drain().filter(|&pos| pos.distance(center) <= NEAR_EDITS).collect();
    for pos in edited {
        if world.editing.contains(&pos) {
            world.edited.insert(pos);
        } else if world.dirty.contains(&pos) {
            // One still waiting for a neighbour to arrive stays edited until it can go.
            match world.neighborhood(pos) {
                Some(columns) => {
                    world.dirty.remove(&pos);
                    world.editing.insert(pos);
                    start_job(world, pos, center, columns, Some(&*EDITS));
                }
                None => {
                    world.edited.insert(pos);
                }
            }
        }
    }
    world.stats.jobs_in_flight = world.in_flight.len();
    // No job can start until a result frees a slot. Keep the detail checks above, but
    // avoid thousands of nine-neighbour lookups on every busy streaming frame.
    if world.in_flight.len() >= max_in_flight {
        return;
    }
    let voxels = &world.voxels;
    world.dirty.retain(|pos| voxels.contains(*pos));
    // Columns at the edge of the loaded area wait for neighbours. Scan them when a
    // worker slot is available, so newly arrived neighbours can make them ready.
    let mut meshable = 0;
    let mut ready = Vec::new();
    for &pos in &world.dirty {
        if world.meshable(pos) {
            meshable += 1;
            if !world.in_flight.contains_key(&pos) {
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
        start_job(world, pos, center, columns, None);
    }
    world.stats.jobs_in_flight = world.in_flight.len();
    world.stats.pending_columns = meshable - dispatched;
}

/// Relights and remeshes `pos` from `columns` (it and its neighbours) on `pool`, or on the
/// shared workers. It is the column's newest job: any older one's result is dropped.
fn start_job(world: &mut ClientWorld, pos: ColumnPos, center: ColumnPos, columns: Neighborhood, pool: Option<&rayon::ThreadPool>) {
    world.jobs += 1;
    let job = world.jobs;
    world.in_flight.insert(pos, job);
    let registry = world.registry.clone();
    let tx = world.results_tx.clone();
    let detail = detail_at(pos.distance(center), world.details.get(&pos).copied(), world.detail_range);
    let work = move || {
        let region = LitRegion::compute(&registry, &columns);
        let chunks = (0..COLUMN_CHUNKS)
            .map(|cy| ChunkResult::new(mesh_chunk(&registry, &region, &columns[4], cy, detail)))
            .collect();
        let _ = tx.send(MeshResult { pos, job, detail, chunks });
    };
    match pool {
        Some(pool) => pool.spawn(work),
        None => rayon::spawn(work),
    }
}

/// Hands finished jobs to the renderer, skipping meshes that did not change.
fn apply_meshes(mut world: ResMut<ClientWorld>) {
    let world = &mut *world;
    let mut uploaded = 0;
    while uploaded < UPLOAD_BUDGET
        && let Ok(result) = world.results_rx.try_recv()
    {
        // Only the column's newest job is shown: an older one finishing after an edit's
        // (which started later but on threads of its own) has the blocks before the edit.
        if world.in_flight.get(&result.pos) != Some(&result.job) {
            continue;
        }
        world.in_flight.remove(&result.pos);
        world.editing.remove(&result.pos);
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
                world.stats.quads = world.stats.quads - render.quads[i] + result.quads;
                render.quads[i] = result.quads;
                uploaded += result.quads;
                let present = !result.mesh.is_empty();
                if present || render.passes[i] {
                    world.terrain.upload(chunk, pass, result.mesh);
                }
                world.visibility_dirty |= present != render.passes[i];
                world.stats.chunk_entities = world.stats.chunk_entities - usize::from(render.passes[i]) + usize::from(present);
                render.passes[i] = present;
            }
        }
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
        // At the default class, not lowered with the streaming threads (priority.rs): chunks
        // that arrive show only once a walk has taken them in.
        let walk = move || {
            crate::priority::default_thread();
            walk();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_world() -> ClientWorld {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        ClientWorld::new(content.blocks, 2, Terrain::detached())
    }

    fn chunk(quads: [usize; 5]) -> ChunkResult {
        let passes = quads.map(|count| {
            let vertices = vec![[0, 0]; count * 4];
            let indices = (0..count as u32).flat_map(|quad| {
                let first = quad * 4;
                [first, first + 1, first + 2, first, first + 2, first + 3]
            }).collect();
            PassResult::new(MeshBuffers { vertices, indices })
        });
        ChunkResult { connectivity: Connectivity::default(), passes, blades: Vec::new(), blades_hash: 0 }
    }

    fn queue(app: &mut App, pos: ColumnPos, quads: [usize; 5]) {
        let mut world = app.world_mut().resource_mut::<ClientWorld>();
        world.jobs += 1;
        let job = world.jobs;
        world.in_flight.insert(pos, job);
        world.results_tx.send(MeshResult { pos, job, detail: 1, chunks: vec![chunk(quads)] }).unwrap();
        drop(world);
        app.update();
    }

    fn assert_stats(app: &App, quads: usize, passes: usize) {
        let world = app.world().resource::<ClientWorld>();
        assert_eq!(world.stats.quads, quads);
        assert_eq!(world.stats.chunk_entities, passes);
        assert_eq!(world.stats.quads, world.chunks.values().flat_map(|c| c.quads).sum::<usize>());
        assert_eq!(world.stats.chunk_entities, world.chunks.values().flat_map(|c| c.passes).filter(|&p| p).count());
    }

    #[test]
    fn mesh_totals_follow_replacements_empty_results_unloads_and_reset() {
        let mut world = test_world();
        let a = ColumnPos::new(0, 0);
        let b = ColumnPos::new(1, 0);
        world.insert_column(a, Arc::new(Column::default()));
        world.insert_column(b, Arc::new(Column::default()));
        let mut app = App::new();
        app.insert_resource(world).add_systems(Update, apply_meshes);
        queue(&mut app, a, [2, 1, 0, 1, 0]);
        queue(&mut app, b, [0, 0, 3, 0, 2]);
        assert_stats(&app, 9, 5);
        queue(&mut app, a, [1, 0, 0, 0, 4]);
        assert_stats(&app, 10, 4);
        queue(&mut app, a, [1, 0, 0, 0, 4]);
        assert_stats(&app, 10, 4);
        // An all-empty result uploads zero quads, but must still subtract old geometry.
        queue(&mut app, b, [0; 5]);
        assert_stats(&app, 5, 2);
        app.world_mut().resource_mut::<ClientWorld>().remove_column(a);
        assert_stats(&app, 0, 0);
        queue(&mut app, a, [9, 0, 0, 0, 0]);
        assert_stats(&app, 0, 0);
        app.world_mut().resource_mut::<ClientWorld>().insert_column(a, Arc::new(Column::default()));
        queue(&mut app, a, [2; 5]);
        assert_stats(&app, 10, 5);
        app.world_mut().resource_mut::<ClientWorld>().reset(3);
        assert_stats(&app, 0, 0);
    }

    #[test]
    fn full_workers_defer_the_queue_scan_but_keep_detail_checks() {
        let mut world = test_world();
        let far = ColumnPos::new(20, 0);
        world.details.insert(far, 1);
        world.set_far_detail([2, 4]);
        let max = rayon::current_num_threads() * 2;
        world.in_flight.extend((0..max).map(|x| (ColumnPos::new(x as i32, 0), 0)));
        world.stats.pending_columns = 42;
        let mut app = App::new();
        app.insert_resource(world).add_systems(Update, dispatch_jobs);
        app.update();
        let world = app.world().resource::<ClientWorld>();
        assert!(!world.details_dirty);
        assert!(world.dirty.contains(&far), "detail changes must queue even while workers are full");
        assert_eq!(world.stats.jobs_in_flight, max);
        assert_eq!(world.stats.pending_columns, 42, "the busy-frame count comes from the last dispatch scan");
        app.world_mut().resource_mut::<ClientWorld>().in_flight.clear();
        app.update();
        let world = app.world().resource::<ClientWorld>();
        assert!(world.dirty.is_empty(), "the next free slot must discard unloaded columns");
        assert_eq!(world.stats.jobs_in_flight, 0);
        assert_eq!(world.stats.pending_columns, 0, "cached pending work must never prevent idle after a fresh scan");
    }

    /// A block broken by the camera is shown broken at once, though every shared worker is
    /// busy with the land streaming in (as early in a new world): its column is meshed on
    /// threads of its own. A job for it begun before the break, finishing after, does not
    /// bring the block back.
    #[test]
    fn a_block_broken_nearby_shows_broken_while_the_shared_workers_are_full() {
        let mut world = test_world();
        let stone = world.registry.id("base:stone").expect("stone");
        let mut column = Column::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                column.set_block(IVec3::new(x, 60, z), stone);
            }
        }
        let column = Arc::new(column);
        for dz in -2..=2 {
            for dx in -2..=2 {
                world.insert_column(ColumnPos::new(dx, dz), column.clone());
            }
        }
        let mut app = App::new();
        app.insert_resource(world).add_systems(Update, (dispatch_jobs, apply_meshes).chain());
        let centre = ColumnPos::new(0, 0);
        let settle = |app: &mut App, what: &str| {
            let start = std::time::Instant::now();
            while app.world().resource::<ClientWorld>().meshing(centre) {
                assert!(start.elapsed() < std::time::Duration::from_secs(20), "{what}: the column at the camera was never meshed");
                app.update();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        settle(&mut app, "the land as it came");
        let mut world = app.world_mut().resource_mut::<ClientWorld>();
        // Every shared worker taken, and among their jobs one for the camera's column.
        let max = rayon::current_num_threads() * 2;
        world.in_flight.extend((0..max).map(|x| (ColumnPos::new(100 + x as i32, 0), 0)));
        world.jobs += 1;
        let before = world.jobs;
        world.in_flight.insert(centre, before);
        world.set_block(IVec3::new(5, 60, 5), BlockId::AIR);
        drop(world);
        settle(&mut app, "the broken block");
        let world = app.world().resource::<ClientWorld>();
        world.results_tx.send(MeshResult { pos: centre, job: before, detail: 1, chunks: (0..COLUMN_CHUNKS).map(|_| chunk([1, 0, 0, 0, 0])).collect() }).unwrap();
        app.update();
        let world = app.world().resource::<ClientWorld>();
        assert_eq!(world.check_meshed(centre), Ok(()), "the job begun before the break showed the block again");
    }

    /// A block broken where a neighbouring column has not arrived yet (early in a new
    /// world) waits for it, and is then meshed at once all the same, the shared workers
    /// still full.
    #[test]
    fn a_block_broken_before_its_neighbours_arrive_shows_once_they_do() {
        let mut world = test_world();
        let stone = world.registry.id("base:stone").expect("stone");
        let mut column = Column::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                column.set_block(IVec3::new(x, 60, z), stone);
            }
        }
        let column = Arc::new(column);
        let late = ColumnPos::new(1, 1);
        for dz in -1..=1 {
            for dx in -1..=1 {
                if ColumnPos::new(dx, dz) != late {
                    world.insert_column(ColumnPos::new(dx, dz), column.clone());
                }
            }
        }
        let max = rayon::current_num_threads() * 2;
        world.in_flight.extend((0..max).map(|x| (ColumnPos::new(100 + x as i32, 0), 0)));
        let centre = ColumnPos::new(0, 0);
        world.set_block(IVec3::new(5, 60, 5), BlockId::AIR);
        let mut app = App::new();
        app.insert_resource(world).add_systems(Update, (dispatch_jobs, apply_meshes).chain());
        for _ in 0..10 {
            app.update();
        }
        assert!(app.world().resource::<ClientWorld>().meshing(centre), "meshed without all its neighbours");
        app.world_mut().resource_mut::<ClientWorld>().insert_column(late, column);
        let start = std::time::Instant::now();
        while app.world().resource::<ClientWorld>().meshing(centre) {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the broken block waited behind the full workers once its neighbour came");
            app.update();
        }
        assert_eq!(app.world().resource::<ClientWorld>().check_meshed(centre), Ok(()));
    }

    /// A column edited every frame (water flowing past, fire spreading) is still shown as
    /// it changes, one edit's mesh at a time, and settles once the edits stop: a job started
    /// for each edit, each superseding the last, would never show any.
    #[test]
    fn a_column_edited_every_frame_still_shows_its_changes() {
        let mut world = test_world();
        let stone = world.registry.id("base:stone").expect("stone");
        let mut column = Column::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                column.set_block(IVec3::new(x, 60, z), stone);
            }
        }
        let column = Arc::new(column);
        for dz in -2..=2 {
            for dx in -2..=2 {
                world.insert_column(ColumnPos::new(dx, dz), column.clone());
            }
        }
        let mut app = App::new();
        app.insert_resource(world).add_systems(Update, (dispatch_jobs, apply_meshes).chain());
        let centre = ColumnPos::new(0, 0);
        let start = std::time::Instant::now();
        while app.world().resource::<ClientWorld>().meshing(centre) {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the land as it came was never meshed");
            app.update();
        }
        // Another block broken every frame, frames coming faster than a column meshes, so
        // every mesh shown is a change. A column's edit job runs until it is shown: the next
        // one never takes its place while it is still on its way.
        let chunk = centre.chunk(60 / CHUNK_SIZE);
        let shown = |app: &App| app.world().resource::<ClientWorld>().chunks.get(&chunk).map(|c| c.hashes);
        let (mut last, mut changes, mut frames) = (shown(&app), 0, 0);
        let mut job = None;
        let start = std::time::Instant::now();
        while changes < 3 {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the edited chunk changed on screen {changes} times in {frames} frames of edits");
            let cell = frames % (CHUNK_SIZE * CHUNK_SIZE);
            let block = if frames / (CHUNK_SIZE * CHUNK_SIZE) % 2 == 0 { BlockId::AIR } else { stone };
            frames += 1;
            app.world_mut().resource_mut::<ClientWorld>().set_block(IVec3::new(cell % CHUNK_SIZE, 60, cell / CHUNK_SIZE), block);
            app.update();
            let now = shown(&app);
            changes += usize::from(now != last);
            last = now;
            let newest = app.world().resource::<ClientWorld>().in_flight.get(&centre).copied();
            if let (Some(was), Some(newest)) = (job, newest) {
                assert_eq!(was, newest, "an edit's job was replaced before it was shown, after {frames} frames of edits");
            }
            job = newest;
        }
        let start = std::time::Instant::now();
        while app.world().resource::<ClientWorld>().meshing(centre) {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "the column never settled after the edits stopped");
            app.update();
        }
        assert_eq!(app.world().resource::<ClientWorld>().check_meshed(centre), Ok(()));
    }
}
