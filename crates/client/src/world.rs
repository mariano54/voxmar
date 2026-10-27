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
    /// How many times the server has said which fires burn, so a change is looked for
    /// only when it has.
    pub fires_told: u32,
    /// Blocks smouldering, about to catch fire, as the server last said
    /// (`ServerMessage::Smoulders`): they smoke and crackle until they catch or are saved.
    pub smoulders: FxHashSet<IVec3>,
    /// What stands on the blocks that show what they hold, a spot apiece, as the server
    /// last said (`ServerMessage::Shown`): a mantel shelf's things (`drops::shelf_things`).
    pub shown: FxHashMap<IVec3, Vec<Option<BlockId>>>,
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
            fires_told: 0,
            smoulders: FxHashSet::default(),
            shown: FxHashMap::default(),
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

    /// Whether every column received that can be meshed (its neighbours received too) is
    /// meshed, and nothing is being meshed: the land in has all been built (workers.rs).
    pub fn caught_up(&self) -> bool {
        self.in_flight.is_empty() && self.dirty.iter().all(|&pos| !self.voxels.contains(pos) || !self.meshable(pos))
    }

    /// Whether every column within `radius` of `center` has come.
    pub fn all_in(&self, center: ColumnPos, radius: i32) -> bool {
        (-radius..=radius).all(|dz| (-radius..=radius).all(|dx| self.voxels.contains(center.offset(dx, dz))))
    }

    /// Whether the land in view of `center` is all built: every column within the view
    /// distance come, and every one inside its edge (whose neighbours have come too) meshed
    /// and up to date. An edge waiting for neighbours is no hole; one inside it is.
    pub fn built_in_view(&self, center: ColumnPos) -> bool {
        let inner = self.view_distance - 1;
        let built = |pos: ColumnPos| self.details.contains_key(&pos) && !self.dirty.contains(&pos);
        self.in_flight.is_empty()
            && self.all_in(center, self.view_distance)
            && (-inner..=inner).all(|dz| (-inner..=inner).all(|dx| built(center.offset(dx, dz))))
    }

    /// How many columns straight ahead of `eye` along `forward` (on the ground plane) are
    /// meshed before the first that is not, a strip three columns wide, up to the view
    /// distance: the land a flight has in front of it, which falls short when streaming
    /// falls behind (the capture's `ahead:` line).
    pub fn meshed_ahead(&self, eye: Vec3, forward: Vec2) -> i32 {
        self.ahead(eye, forward, |c| self.details.contains_key(&c))
    }

    /// As `meshed_ahead`, of the columns come from the server, meshed or not: with the two
    /// alike, a flight outruns generation; with this one further, meshing.
    pub fn received_ahead(&self, eye: Vec3, forward: Vec2) -> i32 {
        self.ahead(eye, forward, |c| self.voxels.contains(c))
    }

    fn ahead(&self, eye: Vec3, forward: Vec2, has: impl Fn(ColumnPos) -> bool) -> i32 {
        let forward = forward.normalize_or_zero();
        let side = forward.perp();
        let column = |at: Vec2| ColumnPos((at / CHUNK_SIZE as f32).floor().as_ivec2());
        let start = eye.xz();
        (1..=self.view_distance)
            .find(|&d| {
                let at = start + forward * (d * CHUNK_SIZE) as f32;
                [-1.0, 0.0, 1.0].iter().any(|&s| !has(column(at + side * (s * CHUNK_SIZE as f32))))
            })
            .map_or(self.view_distance, |d| d - 1)
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

    /// What the server says stands on the shelves in `column` now (`ServerMessage::Shown`),
    /// in place of what it said of it before.
    pub fn show_in(&mut self, column: ColumnPos, at: Vec<(IVec3, Vec<Option<BlockId>>)>) {
        self.shown.retain(|&pos, _| ColumnPos::of_block(pos) != column);
        self.shown.extend(at);
    }

    pub fn remove_column(&mut self, pos: ColumnPos) {
        self.shown.retain(|&at, _| ColumnPos::of_block(at) != pos);
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

/// Which dirty column to mesh first, the least first: the nearest, but those the camera
/// faces before those beside it and those behind, as if each were twice as far beside and
/// three times as far behind as ahead. A flight meets the land ahead and passes what is
/// behind; standing, what is looked at fills in first. With no facing, the nearest.
fn mesh_order(pos: ColumnPos, center: ColumnPos, facing: Vec2) -> i32 {
    let distance = pos.distance(center) as f32;
    let along = (pos.0 - center.0).as_vec2().dot(facing).clamp(-distance, distance);
    ((BESIDE * distance - along) * ORDER_STEPS).round() as i32
}

/// How much farther a column beside the camera counts than one as far ahead (`mesh_order`);
/// one behind counts one more.
const BESIDE: f32 = 2.0;
/// Steps in `mesh_order` to a column of distance, so columns a fraction apart differ.
const ORDER_STEPS: f32 = 4.0;

/// Starts relight+remesh jobs for dirty columns, nearest to the camera first and those it
/// faces before the rest (`mesh_order`), and remeshes columns whose level of detail
/// changed as the camera moved.
fn dispatch_jobs(
    mut world: ResMut<ClientWorld>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut last_center: Local<Option<ColumnPos>>,
) {
    let max_in_flight = rayon::current_num_threads() * 2;
    let camera = camera.single().ok();
    let center = camera.map(|t| ColumnPos::of_block(t.translation().floor().as_ivec3())).unwrap_or(ColumnPos::new(0, 0));
    let facing = camera.map_or(Vec2::ZERO, |t| t.forward().xz().normalize_or_zero());

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
                    start_job(world, pos, center, columns, Some(&*EDITS), None);
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
    // The first in `mesh_order`, sorting only as many as there are free jobs.
    let free = max_in_flight.saturating_sub(world.in_flight.len()).min(ready.len());
    let order = |pos: &ColumnPos| mesh_order(*pos, center, facing);
    if free < ready.len() {
        ready.select_nth_unstable_by_key(free, order);
        ready.truncate(free);
    }
    ready.sort_unstable_by_key(order);

    let mut dispatched = 0;
    for pos in ready {
        let Some(columns) = world.neighborhood(pos) else { continue };
        // Streaming on fewer workers than there are, once playing (workers.rs).
        let Some(slot) = voxmar_engine::jobs::start(voxmar_engine::jobs::Kind::Meshes) else { break };
        dispatched += 1;
        world.dirty.remove(&pos);
        start_job(world, pos, center, columns, None, Some(slot));
    }
    world.stats.jobs_in_flight = world.in_flight.len();
    world.stats.pending_columns = meshable - dispatched;
}

/// Relights and remeshes `pos` from `columns` (it and its neighbours) on `pool`, or on the
/// shared workers, holding `slot` (a streaming job's, `voxmar_engine::jobs`) while it runs.
/// It is the column's newest job: any older one's result is dropped.
fn start_job(world: &mut ClientWorld, pos: ColumnPos, center: ColumnPos, columns: Neighborhood, pool: Option<&rayon::ThreadPool>, slot: Option<voxmar_engine::jobs::Slot>) {
    world.jobs += 1;
    let job = world.jobs;
    world.in_flight.insert(pos, job);
    let registry = world.registry.clone();
    let tx = world.results_tx.clone();
    let detail = detail_at(pos.distance(center), world.details.get(&pos).copied(), world.detail_range);
    let work = move || {
        let _slot = slot;
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

    /// Caught up only with every column whose neighbours have come meshed: one at the edge
    /// of what has come waits for its neighbours and holds nothing up.
    #[test]
    fn caught_up_once_every_meshable_column_is_meshed() {
        let mut world = test_world();
        assert!(world.caught_up());
        let column = Arc::new(Column::default());
        for dz in -1..=1 {
            for dx in -1..=1 {
                world.insert_column(ColumnPos::new(dx, dz), column.clone());
            }
        }
        assert!(!world.caught_up(), "the middle one can be meshed");
        world.dirty.remove(&ColumnPos::new(0, 0));
        assert!(world.caught_up(), "the rest wait for neighbours");
        world.in_flight.insert(ColumnPos::new(0, 0), 1);
        assert!(!world.caught_up(), "one being meshed");
        world.in_flight.clear();
        world.view_distance = 1;
        assert!(world.all_in(ColumnPos::new(0, 0), 1));
        assert!(!world.all_in(ColumnPos::new(1, 0), 1), "the next ring not come");
        assert!(!world.built_in_view(ColumnPos::new(0, 0)), "the middle not meshed: a hole");
        world.details.insert(ColumnPos::new(0, 0), 1);
        assert!(world.built_in_view(ColumnPos::new(0, 0)), "the edge waits for neighbours");
    }

    #[test]
    fn meshed_ahead_stops_at_the_first_hole() {
        let mut world = test_world();
        world.view_distance = 8;
        // Meshed out to five columns east of the origin, three wide; a hole at the sixth.
        for x in 0..=5 {
            for z in -1..=1 {
                world.details.insert(ColumnPos::new(x, z), 1);
            }
        }
        world.details.remove(&ColumnPos::new(4, 1));
        let eye = Vec3::new(8.0, 100.0, 8.0);
        assert_eq!(world.meshed_ahead(eye, Vec2::X), 3, "the hole beside the path at four");
        world.details.insert(ColumnPos::new(4, 1), 1);
        assert_eq!(world.meshed_ahead(eye, Vec2::X), 5);
        assert_eq!(world.meshed_ahead(eye, -Vec2::X), 0, "nothing west");
        // Come out to seven, meshed or not.
        for x in 0..=7 {
            for z in -1..=1 {
                world.voxels.insert(ColumnPos::new(x, z), Arc::new(Column::default()));
            }
        }
        assert_eq!(world.received_ahead(eye, Vec2::X), 7);
        assert_eq!(world.meshed_ahead(eye, Vec2::X), 5, "of what came, meshed");
    }

    /// The land the camera faces is meshed first: of columns as near, ahead before beside
    /// before behind, and ahead as soon as beside at half the distance or behind at a third.
    #[test]
    fn the_land_ahead_is_meshed_first() {
        let order = |x, z, facing| mesh_order(ColumnPos::new(x, z), ColumnPos::new(0, 0), facing);
        let east = Vec2::X;
        assert!(order(3, 0, east) < order(0, 3, east) && order(0, 3, east) < order(-3, 0, east));
        assert_eq!(order(6, 0, east), order(0, 3, east));
        assert_eq!(order(9, 0, east), order(-3, 0, east));
        assert_eq!(order(3, 0, Vec2::ZERO), order(-3, 0, Vec2::ZERO), "no facing: the nearest");
        // Dispatched so: with columns all round and room for a few jobs, those east first.
        // The rest of the jobs are taken, far off, however many threads there are.
        let max = rayon::current_num_threads() * 2;
        let free = max.min(8);
        let taken = max - free;
        let mut world = test_world();
        world.in_flight.extend((0..taken).map(|x| (ColumnPos::new(1000 + x as i32, 1000), 0)));
        for x in -6..=6 {
            for z in -6..=6 {
                world.voxels.insert(ColumnPos::new(x, z), Arc::new(Column::default()));
                if x.abs() <= 5 && z.abs() <= 5 {
                    world.dirty.insert(ColumnPos::new(x, z));
                }
            }
        }
        let mut app = App::new();
        app.insert_resource(world).add_systems(Update, dispatch_jobs);
        app.world_mut().spawn((Camera3d::default(), GlobalTransform::from(Transform::from_xyz(8.0, 100.0, 8.0).looking_to(Vec3::X, Vec3::Y))));
        app.update();
        let world = app.world().resource::<ClientWorld>();
        let started: Vec<ColumnPos> = world.in_flight.keys().copied().filter(|p| p.0.x < 1000).collect();
        assert_eq!(started.len(), free, "{started:?}");
        // None behind, and half at least ahead: those beside tie with those twice as far ahead.
        let ahead = started.iter().filter(|p| p.0.x > 0).count();
        let behind = started.iter().filter(|p| p.0.x < 0).count();
        assert!(behind == 0 && 2 * ahead >= free, "{ahead} east, {behind} west of {started:?}");
        // The player's own column first of all.
        assert!(started.contains(&ColumnPos::new(0, 0)), "{started:?}");
        for x in -5..=5 {
            for z in -5..=5 {
                assert!((x, z) == (0, 0) || order(x, z, east) > order(0, 0, east), "({x}, {z})");
            }
        }
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

    /// The main thread's share of streaming in a fast flight, frame by frame, as the game
    /// does it on a machine with no GPU: an in-process server at view 32 on seed 1, a
    /// player flying along +x at four times flight speed for `FLIGHT` seconds, and a frame
    /// every 4 ms (the w4 sweep's median) that takes in the server's columns and unloads,
    /// starts meshing jobs, applies their results and walks cave culling. Prints the time
    /// of each step and of the whole frame (median, p99, worst), and for the slowest frames
    /// what each step took and how much it handled.
    ///
    /// Ignored by default: `cargo test --release -p voxmar_client --bin voxmar
    /// streaming_main_thread -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn streaming_main_thread() {
        use bevy::ecs::schedule::Schedule;
        use std::time::{Duration, Instant};
        use voxmar_protocol::{ClientMessage, PROTOCOL_VERSION, PlayerInput, ServerMessage, local_pair};
        const FLIGHT: f32 = 30.0;
        const FRAME: Duration = Duration::from_millis(4);
        const ALTITUDE: f32 = 240.0;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let server = voxmar_server::spawn(voxmar_server::ServerConfig { seed: 1, view_distance: 32, ..Default::default() }, content.clone()).unwrap();
        let (connection, server_side) = local_pair();
        server.connections().send(server_side).unwrap();
        connection.send(ClientMessage::Hello { name: "bench".into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: None });
        connection.send(ClientMessage::Settings { view_distance: 32, fly_speed: 4.0 });
        let mut app = App::new();
        app.insert_resource(ClientWorld::new(content.blocks.clone(), 32, Terrain::detached()));
        app.insert_resource(Time::<Real>::default());
        let camera = app.world_mut().spawn((Camera3d::default(), GlobalTransform::default())).id();
        let mut steps: Vec<(&str, Schedule)> = Vec::new();
        for (name, system) in [("dispatch", dispatch_jobs.into_configs()), ("apply", apply_meshes.into_configs()), ("cull", cull_chunks.into_configs())] {
            let mut schedule = Schedule::default();
            schedule.add_systems(system);
            steps.push((name, schedule));
        }
        // Each frame: how long the server's messages took to take in, and each step.
        struct Frame { total: f32, parts: Vec<f32>, columns: usize, unloads: usize, jobs: usize, applied: usize, chunks: usize }
        let mut frames: Vec<Frame> = Vec::new();
        let speed = content.movement.fly_sprint_speed * 4.0 * 0.9;
        let (mut position, mut started, mut last_input, mut teleports) = (None::<Vec3>, None::<Instant>, Instant::now(), 0);
        loop {
            let frame = Instant::now();
            let cpu = || crate::render::work::cpu_clock().map_or(0.0, |ns| ns as f32 / 1e6);
            let frame_cpu = cpu();
            let (mut columns, mut unloads) = (0, 0);
            {
                let mut world = app.world_mut().resource_mut::<ClientWorld>();
                while let Ok(Some(message)) = connection.try_recv() {
                    match message {
                        ServerMessage::Welcome { position: at, .. } => position = Some(at.with_y(ALTITUDE)),
                        ServerMessage::Column { pos, column } => {
                            columns += 1;
                            world.insert_column(pos, column);
                        }
                        ServerMessage::Unload { pos } => {
                            unloads += 1;
                            world.remove_column(pos);
                        }
                        ServerMessage::Teleport { seq, .. } => teleports = seq,
                        _ => {}
                    }
                }
            }
            let mut parts = vec![cpu() - frame_cpu];
            let Some(at) = position.as_mut() else { std::thread::sleep(FRAME); continue };
            app.world_mut().entity_mut(camera).insert(GlobalTransform::from(Transform::from_translation(*at).looking_to(Vec3::X, Vec3::Y)));
            app.world_mut().resource_mut::<Time<Real>>().update();
            let results_before = app.world().resource::<ClientWorld>().results_rx.len();
            let chunks = app.world().resource::<ClientWorld>().chunks.len();
            for (_, schedule) in &mut steps {
                let step = cpu();
                schedule.run(app.world_mut());
                parts.push(cpu() - step);
            }
            let world = app.world().resource::<ClientWorld>();
            let jobs = world.stats.jobs_in_flight;
            let applied = results_before.saturating_sub(world.results_rx.len());
            let flying = *started.get_or_insert_with(Instant::now);
            if flying.elapsed().as_secs_f32() > FLIGHT {
                break;
            }
            let dt = last_input.elapsed().as_secs_f32();
            if dt >= 0.05 {
                *at += Vec3::X * speed * dt;
                last_input = Instant::now();
                connection.send(ClientMessage::Input(PlayerInput { position: *at, yaw: -std::f32::consts::FRAC_PI_2, pitch: 0.0, selected_slot: 0, flying: true, teleport: teleports }));
            }
            frames.push(Frame { total: cpu() - frame_cpu, parts, columns, unloads, jobs, applied, chunks });
            std::thread::sleep(FRAME.saturating_sub(frame.elapsed()));
        }
        let names = ["messages", "dispatch", "apply", "cull"];
        let stats = |mut values: Vec<f32>| {
            values.sort_by(f32::total_cmp);
            let at = |q: f32| values[((values.len() - 1) as f32 * q) as usize];
            format!("median {:.2}, p99 {:.2}, worst {:.2} ms", at(0.5), at(0.99), at(1.0))
        };
        eprintln!("{} frames over {FLIGHT} s; main thread's streaming work a frame, in thread CPU time: {}", frames.len(), stats(frames.iter().map(|f| f.total).collect()));
        for (i, name) in names.iter().enumerate() {
            eprintln!("  {name}: {}", stats(frames.iter().map(|f| f.parts[i]).collect()));
        }
        let mut slowest: Vec<&Frame> = frames.iter().collect();
        slowest.sort_by(|a, b| b.total.total_cmp(&a.total));
        for f in slowest.iter().take(12) {
            let parts: Vec<String> = names.iter().zip(&f.parts).map(|(n, t)| format!("{n} {t:.2}")).collect();
            eprintln!("  {:.2} ms: {}; {} columns in, {} unloaded, {} jobs, {} results waiting before, {} chunks", f.total, parts.join(", "), f.columns, f.unloads, f.jobs, f.applied, f.chunks);
        }
    }

    /// The main thread's streaming steps at the full view of 32, which a cloud server
    /// cannot stream in fast enough for `streaming_main_thread` to reach: 65×65 columns of
    /// 16 chunks each, all meshed. Times, in thread CPU, a row of 65 columns unloading and
    /// another arriving (as the camera crosses a column), the detail-range scan and the
    /// dirty-column scan that follow, and building cave culling's job.
    ///
    /// Ignored by default: `cargo test --release -p voxmar_client --bin voxmar
    /// streaming_steps_at_full_view -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn streaming_steps_at_full_view() {
        use bevy::ecs::schedule::Schedule;
        const VIEW: i32 = 32;
        let cpu = || crate::render::work::cpu_clock().map_or(0.0, |ns| ns as f64 / 1e6);
        let mut app = App::new();
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let mut world = ClientWorld::new(content.blocks.clone(), VIEW, Terrain::detached());
        let column = Arc::new(Column::default());
        for x in -VIEW..=VIEW {
            for z in -VIEW..=VIEW {
                let pos = ColumnPos::new(x, z);
                world.voxels.insert(pos, column.clone());
                world.details.insert(pos, 1);
                for cy in 0..COLUMN_CHUNKS {
                    world.chunks.insert(pos.chunk(cy), ChunkRender { passes: [true, true, false, false, false], quads: [400, 50, 0, 0, 0], ..Default::default() });
                }
            }
        }
        app.insert_resource(world);
        app.insert_resource(Time::<Real>::default());
        let camera = app.world_mut().spawn((Camera3d::default(), GlobalTransform::default())).id();
        let mut dispatch = Schedule::default();
        dispatch.add_systems(dispatch_jobs);
        let mut cull = Schedule::default();
        cull.add_systems(cull_chunks);
        let mut times: [Vec<f64>; 5] = Default::default();
        for step in 0..40 {
            // The camera crosses into the next column east: the row VIEW behind goes, the
            // row VIEW ahead comes.
            let (gone, come) = (step - VIEW, step + 1 + VIEW);
            let t = cpu();
            {
                let mut world = app.world_mut().resource_mut::<ClientWorld>();
                for z in -VIEW..=VIEW {
                    world.remove_column(ColumnPos::new(gone, z));
                }
            }
            times[0].push(cpu() - t);
            let t = cpu();
            {
                let mut world = app.world_mut().resource_mut::<ClientWorld>();
                for z in -VIEW..=VIEW {
                    world.insert_column(ColumnPos::new(come, z), column.clone());
                }
            }
            times[1].push(cpu() - t);
            let at = Vec3::new((step + 1) as f32 * CHUNK_SIZE as f32 + 8.0, 100.0, 8.0);
            app.world_mut().entity_mut(camera).insert(GlobalTransform::from(Transform::from_translation(at).looking_to(Vec3::X, Vec3::Y)));
            // No room for jobs: the detail scan alone, then with room, the dirty scan.
            let t = cpu();
            {
                let mut world = app.world_mut().resource_mut::<ClientWorld>();
                let max = rayon::current_num_threads() * 2;
                world.in_flight.extend((0..max).map(|i| (ColumnPos::new(1000 + i as i32, 0), 0)));
            }
            dispatch.run(app.world_mut());
            times[2].push(cpu() - t);
            app.world_mut().resource_mut::<ClientWorld>().in_flight.clear();
            let t = cpu();
            dispatch.run(app.world_mut());
            times[3].push(cpu() - t);
            {
                // Its jobs dropped, as if done: only the scans are timed.
                let mut world = app.world_mut().resource_mut::<ClientWorld>();
                world.in_flight.clear();
                world.culler.busy = false;
                world.visibility_dirty = true;
            }
            app.world_mut().resource_mut::<Time<Real>>().update();
            let t = cpu();
            cull.run(app.world_mut());
            times[4].push(cpu() - t);
            let _ = app.world().resource::<ClientWorld>().culler.results.recv();
            app.world_mut().resource_mut::<ClientWorld>().culler.busy = false;
        }
        for (name, mut t) in ["unload a row of 65", "a row of 65 arriving", "detail scan", "dirty scan and dispatch", "cave culling's job"].into_iter().zip(times) {
            t.sort_by(f64::total_cmp);
            eprintln!("{name}: median {:.3}, worst {:.3} ms", t[t.len() / 2], t[t.len() - 1]);
        }
    }
}
