//! Terrain rendering without an entity per chunk.
//!
//! Chunk meshes live in a few large GPU pages (one vertex and one index buffer each) and a
//! table of chunk origins. Every frame each view culls the chunks against its frustum and
//! draws the survivors straight from the pages: one phase item per pass and view stands
//! in for thousands of mesh entities, so no ECS work, extraction, specialisation or
//! batching scales with the number of chunks. The main world talks to this module only
//! through [`Terrain`], a channel of uploads, removals and the set of chunks that cave
//! culling leaves visible.
//!
//! Grass blades (grass.wgsl) are drawn from the same records: every chunk's blade seeds
//! live in one storage buffer, and chunks near the camera draw a tuft per seed.

use std::ops::Range;

use bevy::core_pipeline::core_3d::{AlphaMask3d, CORE_3D_DEPTH_FORMAT, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey};
use bevy::core_pipeline::prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey};
use bevy::ecs::system::{SystemParam, SystemParamItem};
use bevy::ecs::system::lifetimeless::{Read, SRes};
use bevy::mesh::{MeshVertexBufferLayout, MeshVertexBufferLayouts, PrimitiveTopology, VertexBufferLayout};
use bevy::pbr::{
    LightEntity, MeshPipeline, MeshPipelineKey, PrepassPipeline, SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup,
    SetPrepassViewBindGroup, SetPrepassViewEmptyBindGroup, Shadow, ShadowBatchSetKey, ShadowBinKey, ViewKeyCache,
};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
    RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, CommandEncoderDescriptor, CompareFunction, DepthBiasState, DepthStencilState, Face, FragmentState,
    IndexFormat, MultisampleState, PipelineCache, PrimitiveState, RenderPipelineDescriptor, SamplerBindingType,
    ShaderStages, ShaderType, SpecializedMeshPipeline, StencilState, TextureSampleType, VertexState, encase,
    binding_types::{sampler, storage_buffer_read_only_sized, texture_2d_array, uniform_buffer},
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::camera::ExtractedCamera;
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::coords::{CHUNK_SIZE, ChunkPos};
use voxmar_engine::mesh::{MeshBuffers, Vertex};

use super::lod::{LodVertex, TileKey, TileMesh};
use super::water;
use super::wind::Breeze;
use super::{ATTRIBUTE_LOD, ATTRIBUTE_VOXEL, DISTANT_SHADER, GRASS_SHADER, TerrainPass, VOXEL_SHADER};

/// Vertices per chunk page (32 MB, with 24 MB of indices): room for about a million quads.
const PAGE_VERTICES: u32 = 4 << 20;
const PASSES: [TerrainPass; 5] =
    [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Inner, TerrainPass::Translucent, TerrainPass::Fringe];
/// Only these passes cast shadows: leaf crowns' shells already shade, and water would
/// shadow itself.
const SHADOW_PASSES: [TerrainPass; 3] = [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Fringe];
/// Leaf fringes are drawn for chunks this close to the camera, in blocks; further out they
/// would be sub-pixel.
const FRINGE_DISTANCE: f32 = 96.0;
/// Blades grown per grass block, and vertices per blade (see grass.wgsl).
const BLADES: u32 = 16;
const BLADE_VERTICES: u32 = 9;
/// Grass blades are drawn for chunks this close to the camera, in blocks; they have shrunk
/// away just before (see grass.wgsl).
const BLADE_DISTANCE: f32 = 46.0;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = unbounded();
        app.insert_resource(Terrain { tx }).add_plugins(ExtractResourcePlugin::<TerrainTextures>::default());
        // Phase items are keyed by main world entity; these stand for the passes.
        let mut key = || MainEntity::from(app.world_mut().spawn_empty().id());
        let keys = TerrainKeys { passes: PASSES.map(|_| key()), distant: key(), blades: key() };
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .insert_resource(TerrainInbox(rx))
            .insert_resource(keys)
            .init_resource::<TerrainGpu>()
            .add_render_command::<Opaque3d, DrawTerrainMain>()
            .add_render_command::<Opaque3d, DrawDistantTerrain>()
            .add_render_command::<Opaque3d, DrawGrassBlades>()
            .add_render_command::<AlphaMask3d, DrawTerrainMain>()
            .add_render_command::<Shadow, DrawTerrainShadow>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                (
                    // Before culling, which runs in `Queue`: draws must never refer to
                    // pages a world change has just dropped.
                    receive.in_set(RenderSystems::PrepareAssets),
                    prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
                    (cull, queue).chain().in_set(RenderSystems::Queue),
                ),
            );
    }
}

/// The block texture array and the grass/foliage colour maps.
#[derive(Resource, Clone, ExtractResource)]
pub struct TerrainTextures {
    pub textures: Handle<Image>,
    pub colormaps: Handle<Image>,
}

/// The main world's handle on terrain rendering.
#[derive(Resource, Clone)]
pub struct Terrain {
    tx: Sender<Message>,
}

enum Message {
    Upload { chunk: ChunkPos, pass: TerrainPass, mesh: MeshBuffers },
    Remove { chunk: ChunkPos, pass: TerrainPass },
    Blades { chunk: ChunkPos, seeds: Vec<u32> },
    /// The chunks cave culling leaves visible; any other chunk is skipped.
    Visible(FxHashSet<ChunkPos>),
    UploadTile(Box<TileMesh>),
    RemoveTile(TileKey),
    RealColumns(i32),
    Clear,
}

impl Terrain {
    /// Replaces one pass of a chunk's mesh; an empty mesh removes it.
    pub fn upload(&self, chunk: ChunkPos, pass: TerrainPass, mesh: MeshBuffers) {
        let message = if mesh.is_empty() { Message::Remove { chunk, pass } } else { Message::Upload { chunk, pass, mesh } };
        let _ = self.tx.send(message);
    }

    pub fn remove(&self, chunk: ChunkPos, pass: TerrainPass) {
        let _ = self.tx.send(Message::Remove { chunk, pass });
    }

    /// Replaces a chunk's grass blade seeds (see `voxmar_engine::mesh::ChunkMesh`); none
    /// removes them.
    pub fn upload_blades(&self, chunk: ChunkPos, seeds: Vec<u32>) {
        let _ = self.tx.send(Message::Blades { chunk, seeds });
    }

    pub fn set_visible(&self, visible: FxHashSet<ChunkPos>) {
        let _ = self.tx.send(Message::Visible(visible));
    }

    /// Adds or replaces a distant terrain tile.
    pub fn upload_tile(&self, tile: TileMesh) {
        let _ = self.tx.send(Message::UploadTile(Box::new(tile)));
    }

    pub fn remove_tile(&self, key: TileKey) {
        let _ = self.tx.send(Message::RemoveTile(key));
    }

    /// The radius of real chunks, which water ripples on distant terrain fade out from.
    pub fn set_real_columns(&self, real_columns: i32) {
        let _ = self.tx.send(Message::RealColumns(real_columns));
    }

    /// Forgets every chunk, for leaving a world.
    pub fn clear(&self) {
        let _ = self.tx.send(Message::Clear);
    }
}

#[derive(Resource)]
struct TerrainInbox(Receiver<Message>);

#[derive(Resource)]
struct TerrainKeys {
    passes: [MainEntity; 5],
    distant: MainEntity,
    blades: MainEntity,
}

/// Ranges of a buffer handed out first-fit; freed ranges merge with their neighbours.
#[derive(Default)]
struct RangeAllocator {
    /// Free ranges, sorted and never adjacent.
    free: Vec<Range<u32>>,
}

impl RangeAllocator {
    fn new(size: u32) -> Self {
        Self { free: (size > 0).then_some(0..size).into_iter().collect() }
    }

    fn allocate(&mut self, len: u32) -> Option<Range<u32>> {
        let i = self.free.iter().position(|r| r.len() as u32 >= len)?;
        let start = self.free[i].start;
        self.free[i].start += len;
        if self.free[i].is_empty() {
            self.free.remove(i);
        }
        Some(start..start + len)
    }

    fn release(&mut self, range: Range<u32>) {
        let i = self.free.partition_point(|r| r.start < range.start);
        let joins_previous = i > 0 && self.free[i - 1].end == range.start;
        let joins_next = i < self.free.len() && self.free[i].start == range.end;
        match (joins_previous, joins_next) {
            (true, true) => {
                self.free[i - 1].end = self.free[i].end;
                self.free.remove(i);
            }
            (true, false) => self.free[i - 1].end = range.end,
            (false, true) => self.free[i].start = range.start,
            (false, false) => self.free.insert(i, range),
        }
    }
}

struct Page {
    vertex_buffer: Buffer,
    index_buffer: Buffer,
    vertices: RangeAllocator,
    indices: RangeAllocator,
}

/// Pages of one vertex format.
struct PagePool {
    label: &'static str,
    vertex_size: u64,
    /// Vertices per page; indices get one and a half times as many.
    page_vertices: u32,
    pages: Vec<Page>,
}

impl PagePool {
    fn new(label: &'static str, vertex_size: usize, page_vertices: u32) -> Self {
        Self { label, vertex_size: vertex_size as u64, page_vertices, pages: Vec::new() }
    }

    /// Copies a mesh into the first page with room, adding a page if none has.
    fn allocate<V: bytemuck::Pod>(&mut self, device: &RenderDevice, queue: &RenderQueue, vertices: &[V], indices: &[u32]) -> Allocation {
        let (vertex_count, index_count) = (vertices.len() as u32, indices.len() as u32);
        let found = self.pages.iter_mut().enumerate().find_map(|(i, page)| {
            let vertices = page.vertices.allocate(vertex_count)?;
            match page.indices.allocate(index_count) {
                Some(indices) => Some((i, vertices, indices)),
                None => {
                    page.vertices.release(vertices);
                    None
                }
            }
        });
        let (page, vertex_range, index_range) = found.unwrap_or_else(|| {
            let mut page = self.new_page(device);
            let vertices = page.vertices.allocate(vertex_count).expect("a mesh fits a page");
            let indices = page.indices.allocate(index_count).expect("a mesh fits a page");
            self.pages.push(page);
            (self.pages.len() - 1, vertices, indices)
        });
        let target = &self.pages[page];
        queue.write_buffer(&target.vertex_buffer, vertex_range.start as u64 * self.vertex_size, bytemuck::cast_slice(vertices));
        queue.write_buffer(&target.index_buffer, index_range.start as u64 * 4, bytemuck::cast_slice(indices));
        Allocation { page: page as u32, vertices: vertex_range, indices: index_range }
    }

    fn free(&mut self, allocation: Allocation) {
        let page = &mut self.pages[allocation.page as usize];
        page.vertices.release(allocation.vertices);
        page.indices.release(allocation.indices);
    }

    fn new_page(&self, device: &RenderDevice) -> Page {
        let index = self.pages.len();
        let buffer = |kind: &str, size: u64, usage: BufferUsages| {
            device.create_buffer(&BufferDescriptor {
                label: Some(&format!("{}_{kind}_{index}", self.label)),
                size,
                usage: usage | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let page_indices = self.page_vertices / 2 * 3;
        Page {
            vertex_buffer: buffer("vertices", self.page_vertices as u64 * self.vertex_size, BufferUsages::VERTEX),
            index_buffer: buffer("indices", page_indices as u64 * 4, BufferUsages::INDEX),
            vertices: RangeAllocator::new(self.page_vertices),
            indices: RangeAllocator::new(page_indices),
        }
    }
}

/// Where a mesh lives.
struct Allocation {
    page: u32,
    vertices: Range<u32>,
    indices: Range<u32>,
}

struct ChunkRecord {
    /// Index into the origin table, which the shader reads as the instance index.
    slot: u32,
    passes: [Option<Allocation>; 5],
    /// Where the chunk's grass blade seeds are in the seed buffer.
    blades: Option<Range<u32>>,
}

impl ChunkRecord {
    fn is_empty(&self) -> bool {
        self.passes.iter().all(Option::is_none) && self.blades.is_none()
    }
}

/// Grass blade seeds of every chunk, in one storage buffer that grows as needed.
#[derive(Default)]
struct SeedBuffer {
    buffer: Option<Buffer>,
    ranges: RangeAllocator,
    capacity: u32,
}

impl SeedBuffer {
    fn allocate(&mut self, device: &RenderDevice, queue: &RenderQueue, seeds: &[u32]) -> Range<u32> {
        let len = seeds.len() as u32;
        let range = self.ranges.allocate(len).unwrap_or_else(|| {
            self.grow(device, queue, len);
            self.ranges.allocate(len).expect("the seed buffer has just grown")
        });
        let buffer = self.buffer.as_ref().expect("the seed buffer exists once it has room");
        queue.write_buffer(buffer, range.start as u64 * 4, bytemuck::cast_slice(seeds));
        range
    }

    /// Replaces the buffer with one at least twice as large, holding the same seeds.
    fn grow(&mut self, device: &RenderDevice, queue: &RenderQueue, at_least: u32) {
        let capacity = (self.capacity * 2).max(self.capacity + at_least).max(1 << 18);
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("blade_seeds"),
            size: capacity as u64 * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        if let Some(old) = &self.buffer {
            // Seeds written to the old buffer this frame land before the copy.
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("grow_blade_seeds") });
            encoder.copy_buffer_to_buffer(old, 0, &buffer, 0, self.capacity as u64 * 4);
            queue.submit([encoder.finish()]);
        }
        self.buffer = Some(buffer);
        self.ranges.release(self.capacity..capacity);
        self.capacity = capacity;
    }
}

/// A distant terrain tile (see `lod`).
struct TileRecord {
    slot: u32,
    mesh: Allocation,
    min: Vec3,
    max: Vec3,
}

/// Columns around the camera that have real chunks, one bit each, for distant terrain to
/// give way to exactly where real terrain is drawn.
const COVERAGE_SIZE: i32 = 2 * 48 + 1;
const COVERAGE_WORDS: usize = (COVERAGE_SIZE * COVERAGE_SIZE) as usize / 32 + 1;

/// The wind foliage bends in (see wind.wgsl).
#[derive(ShaderType)]
struct WindUniform {
    /// xy: mean wind along x and z in blocks per second, zw: how far gusts have travelled.
    flow: Vec4,
    /// xyz: the player's feet, w: whether there is a player.
    player: Vec4,
    gustiness: f32,
}

#[derive(ShaderType, Clone, Copy, Default)]
struct DistantView {
    /// xy: column of the coverage mask's first bit, z: its width in columns, w: the real
    /// chunks' radius in columns.
    coverage: IVec4,
}

/// Render world state: pages, chunk records and the bind group shared by every draw.
#[derive(Resource)]
pub(super) struct TerrainGpu {
    chunk_pages: PagePool,
    tile_pages: PagePool,
    chunks: FxHashMap<ChunkPos, ChunkRecord>,
    tiles: FxHashMap<TileKey, TileRecord>,
    /// Chunks with a mesh, per column.
    columns: FxHashMap<IVec2, u32>,
    visible: FxHashSet<ChunkPos>,
    real_columns: i32,
    /// Where the camera is this frame.
    camera: Vec3,
    free_slots: Vec<u32>,
    /// Origin of each slot's mesh; for tiles, w is the cell size.
    origins: Vec<IVec4>,
    origins_buffer: Option<Buffer>,
    origins_dirty: bool,
    distant_view: Option<Buffer>,
    coverage: Option<Buffer>,
    seeds: SeedBuffer,
    wind: Option<Buffer>,
    bind_group: Option<BindGroup>,
    distant_bind_group: Option<BindGroup>,
    /// Whether the bind groups must be rebuilt (the origin buffer was replaced).
    bind_group_dirty: bool,
}

impl Default for TerrainGpu {
    fn default() -> Self {
        Self {
            chunk_pages: PagePool::new("terrain", size_of::<Vertex>(), PAGE_VERTICES),
            tile_pages: PagePool::new("distant", size_of::<LodVertex>(), PAGE_VERTICES / 4),
            chunks: FxHashMap::default(),
            tiles: FxHashMap::default(),
            columns: FxHashMap::default(),
            visible: FxHashSet::default(),
            real_columns: 0,
            camera: Vec3::ZERO,
            free_slots: Vec::new(),
            origins: Vec::new(),
            origins_buffer: None,
            origins_dirty: false,
            distant_view: None,
            coverage: None,
            seeds: SeedBuffer::default(),
            wind: None,
            bind_group: None,
            distant_bind_group: None,
            bind_group_dirty: false,
        }
    }
}

impl TerrainGpu {
    fn new_slot(&mut self, origin: IVec4) -> u32 {
        let slot = self.free_slots.pop().unwrap_or(self.origins.len() as u32);
        if slot as usize == self.origins.len() {
            self.origins.push(origin);
        } else {
            self.origins[slot as usize] = origin;
        }
        self.origins_dirty = true;
        slot
    }

    /// The chunk's record, made if it has none.
    fn record(&mut self, chunk: ChunkPos) -> &mut ChunkRecord {
        if !self.chunks.contains_key(&chunk) {
            let slot = self.new_slot(chunk.origin().extend(1));
            self.chunks.insert(chunk, ChunkRecord { slot, passes: Default::default(), blades: None });
            *self.columns.entry(IVec2::new(chunk.0.x, chunk.0.z)).or_default() += 1;
        }
        self.chunks.get_mut(&chunk).expect("record was just ensured")
    }

    /// Forgets a chunk with nothing left to draw.
    fn forget_if_empty(&mut self, chunk: ChunkPos) {
        if !self.chunks.get(&chunk).is_some_and(ChunkRecord::is_empty) {
            return;
        }
        let record = self.chunks.remove(&chunk).expect("record exists");
        self.free_slots.push(record.slot);
        let column = IVec2::new(chunk.0.x, chunk.0.z);
        if let Some(count) = self.columns.get_mut(&column) {
            *count -= 1;
            if *count == 0 {
                self.columns.remove(&column);
            }
        }
    }

    fn release(&mut self, chunk: ChunkPos, pass: TerrainPass) {
        let Some(record) = self.chunks.get_mut(&chunk) else { return };
        if let Some(allocation) = record.passes[pass as usize].take() {
            self.chunk_pages.free(allocation);
        }
        self.forget_if_empty(chunk);
    }

    fn upload(&mut self, device: &RenderDevice, queue: &RenderQueue, chunk: ChunkPos, pass: TerrainPass, mesh: &MeshBuffers) {
        self.release(chunk, pass);
        let allocation = self.chunk_pages.allocate(device, queue, &mesh.vertices, &mesh.indices);
        self.record(chunk).passes[pass as usize] = Some(allocation);
    }

    fn upload_blades(&mut self, device: &RenderDevice, queue: &RenderQueue, chunk: ChunkPos, seeds: &[u32]) {
        if let Some(range) = self.chunks.get_mut(&chunk).and_then(|record| record.blades.take()) {
            self.seeds.ranges.release(range);
        }
        if seeds.is_empty() {
            self.forget_if_empty(chunk);
            return;
        }
        let capacity = self.seeds.capacity;
        let range = self.seeds.allocate(device, queue, seeds);
        self.bind_group_dirty |= self.seeds.capacity != capacity;
        self.record(chunk).blades = Some(range);
    }

    fn remove_tile(&mut self, key: TileKey) {
        if let Some(tile) = self.tiles.remove(&key) {
            self.tile_pages.free(tile.mesh);
            self.free_slots.push(tile.slot);
        }
    }

    fn upload_tile(&mut self, device: &RenderDevice, queue: &RenderQueue, tile: TileMesh) {
        self.remove_tile(tile.key);
        let mesh = self.tile_pages.allocate(device, queue, &tile.vertices, &tile.indices);
        let corner = tile.key.origin();
        let slot = self.new_slot(IVec4::new(corner.x, 0, corner.y, tile.key.cell()));
        let size = tile.key.size() as f32;
        let corner = corner.as_vec2();
        let min = Vec3::new(corner.x, tile.heights.start as f32, corner.y);
        let max = Vec3::new(corner.x + size, tile.heights.end as f32, corner.y + size);
        self.tiles.insert(tile.key, TileRecord { slot, mesh, min, max });
    }

    /// Whether every column under a tile has real chunks, so it need not be drawn.
    fn covered_by_real(&self, key: TileKey) -> bool {
        if key.size() > CHUNK_SIZE * COVERAGE_SIZE {
            return false;
        }
        let first = key.origin().div_euclid(IVec2::splat(CHUNK_SIZE));
        let side = (key.size() / CHUNK_SIZE).max(1);
        (0..side * side).all(|i| self.columns.contains_key(&(first + IVec2::new(i % side, i / side))))
    }

    fn clear(&mut self) {
        let mut seeds = std::mem::take(&mut self.seeds);
        seeds.ranges = RangeAllocator::new(seeds.capacity);
        *self = Self {
            origins_buffer: self.origins_buffer.take(),
            distant_view: self.distant_view.take(),
            coverage: self.coverage.take(),
            seeds,
            wind: self.wind.take(),
            bind_group_dirty: true,
            ..Self::default()
        };
    }

    /// Writes this frame's wind, making the wind and seed buffers the first time.
    fn write_wind(&mut self, device: &RenderDevice, queue: &RenderQueue, breeze: Breeze) {
        if self.seeds.buffer.is_none() {
            self.seeds.grow(device, queue, 0);
        }
        let wind = WindUniform {
            flow: breeze.velocity.extend(breeze.offset.x).extend(breeze.offset.y),
            player: breeze.player.map_or(Vec4::ZERO, |feet| feet.extend(1.0)),
            gustiness: breeze.gustiness,
        };
        let mut uniform = encase::UniformBuffer::new(Vec::<u8>::new());
        uniform.write(&wind).expect("the wind fits a uniform");
        let buffer = self.wind.get_or_insert_with(|| {
            device.create_buffer(&BufferDescriptor {
                label: Some("terrain_wind"),
                size: WindUniform::min_size().get(),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        queue.write_buffer(buffer, 0, uniform.as_ref());
    }

    /// Writes the origin table, replacing the buffer when it outgrew it.
    fn write_origins(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        if !self.origins_dirty {
            return;
        }
        self.origins_dirty = false;
        let needed = (self.origins.len().max(1) * size_of::<IVec4>()) as u64;
        if self.origins_buffer.as_ref().is_none_or(|b| b.size() < needed) {
            self.origins_buffer = Some(device.create_buffer(&BufferDescriptor {
                label: Some("terrain_chunk_origins"),
                size: needed.next_power_of_two().max(64 << 10),
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.bind_group_dirty = true;
        }
        let buffer = self.origins_buffer.as_ref().expect("origin buffer was just ensured");
        if !self.origins.is_empty() {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.origins));
        }
    }

    /// Writes which columns around `camera` have real chunks, and the real radius.
    fn write_coverage(&mut self, device: &RenderDevice, queue: &RenderQueue, camera: Vec3) {
        let first = IVec2::new(camera.x.floor() as i32, camera.z.floor() as i32).div_euclid(IVec2::splat(CHUNK_SIZE))
            - COVERAGE_SIZE / 2;
        let mut bits = [0u32; COVERAGE_WORDS];
        for column in self.columns.keys() {
            let at = *column - first;
            if at.cmpge(IVec2::ZERO).all() && at.cmplt(IVec2::splat(COVERAGE_SIZE)).all() {
                let i = (at.y * COVERAGE_SIZE + at.x) as usize;
                bits[i / 32] |= 1 << (i % 32);
            }
        }
        let coverage = self.coverage.get_or_insert_with(|| {
            device.create_buffer(&BufferDescriptor {
                label: Some("distant_coverage"),
                size: (COVERAGE_WORDS * 4) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        queue.write_buffer(coverage, 0, bytemuck::cast_slice(&bits));
        let view = DistantView { coverage: IVec4::new(first.x, first.y, COVERAGE_SIZE, self.real_columns) };
        let mut uniform = encase::UniformBuffer::new(Vec::<u8>::new());
        uniform.write(&view).expect("distant view fits a uniform");
        let buffer = self.distant_view.get_or_insert_with(|| {
            device.create_buffer(&BufferDescriptor {
                label: Some("distant_view"),
                size: DistantView::min_size().get(),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        queue.write_buffer(buffer, 0, uniform.as_ref());
    }
}

/// Applies the main world's messages: writes and frees mesh data, updates the origin table.
fn receive(
    mut gpu: ResMut<TerrainGpu>,
    inbox: Res<TerrainInbox>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cameras: Query<&ExtractedView, With<ExtractedCamera>>,
    breeze: Option<Res<Breeze>>,
) {
    for message in inbox.0.try_iter() {
        match message {
            Message::Upload { chunk, pass, mesh } => gpu.upload(&device, &queue, chunk, pass, &mesh),
            Message::Remove { chunk, pass } => gpu.release(chunk, pass),
            Message::Blades { chunk, seeds } => gpu.upload_blades(&device, &queue, chunk, &seeds),
            Message::Visible(visible) => gpu.visible = visible,
            Message::UploadTile(tile) => gpu.upload_tile(&device, &queue, *tile),
            Message::RemoveTile(key) => gpu.remove_tile(key),
            Message::RealColumns(columns) => gpu.real_columns = columns,
            Message::Clear => gpu.clear(),
        }
    }
    gpu.write_origins(&device, &queue);
    gpu.write_wind(&device, &queue, breeze.as_deref().copied().unwrap_or_default());
    if let Some(camera) = cameras.iter().next() {
        let position = camera.world_from_view.translation();
        gpu.camera = position;
        gpu.write_coverage(&device, &queue, position);
    }
}

fn prepare_bind_group(
    mut gpu: ResMut<TerrainGpu>,
    pipelines: Res<TerrainPipelines>,
    textures: Option<Res<TerrainTextures>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
) {
    if gpu.bind_group.is_some() && gpu.distant_bind_group.is_some() && !gpu.bind_group_dirty {
        return;
    }
    let (Some(origins), Some(view), Some(coverage)) = (&gpu.origins_buffer, &gpu.distant_view, &gpu.coverage) else { return };
    let distant = device.create_bind_group(
        "distant_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.distant_layout),
        &BindGroupEntries::sequential((origins.as_entire_binding(), view.as_entire_binding(), coverage.as_entire_binding())),
    );
    let textures = textures.as_ref().and_then(|t| Some((images.get(&t.textures)?, images.get(&t.colormaps)?)));
    let (Some((blocks, colormaps)), Some(seeds), Some(wind)) = (textures, &gpu.seeds.buffer, &gpu.wind) else {
        gpu.distant_bind_group = Some(distant);
        return;
    };
    let bind_group = device.create_bind_group(
        "terrain_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::sequential((
            &blocks.texture_view,
            &blocks.sampler,
            &colormaps.texture_view,
            &colormaps.sampler,
            origins.as_entire_binding(),
            seeds.as_entire_binding(),
            wind.as_entire_binding(),
        )),
    );
    gpu.distant_bind_group = Some(distant);
    gpu.bind_group = Some(bind_group);
    gpu.bind_group_dirty = false;
}

/// One draw: a chunk's pass from a page.
struct Draw {
    page: u32,
    indices: Range<u32>,
    base_vertex: i32,
    slot: u32,
}

/// One chunk's grass blades: a range of vertices, a tuft per seed.
struct BladeDraw {
    vertices: Range<u32>,
    slot: u32,
}

/// The draws of each pass that survive a view's frustum, in drawing order.
#[derive(Component, Default)]
pub(super) struct TerrainDraws {
    passes: [Vec<Draw>; 5],
    /// What the water pass draws the translucent terrain with, when there is any; cameras
    /// only.
    water: Option<WaterDraw>,
    /// Distant terrain tiles, cameras only.
    distant: Vec<Draw>,
    /// Grass blades near the camera, nearest first; cameras only.
    blades: Vec<BladeDraw>,
}

/// The translucent terrain's pipeline and item entity, for the water pass (see `water`).
#[derive(Clone, Copy)]
pub(super) struct WaterDraw {
    pub pipeline: CachedRenderPipelineId,
    pub entity: (Entity, MainEntity),
}

impl TerrainDraws {
    /// Whether the view has water or ice in sight.
    pub(super) fn has_water(&self) -> bool {
        !self.passes[TerrainPass::Translucent as usize].is_empty()
    }

    pub(super) fn water(&self) -> Option<WaterDraw> {
        self.water.filter(|_| self.has_water())
    }
}

/// A view whose draws `cull` fills: a camera or a shadow cascade.
type CulledView = (Entity, &'static ExtractedView, Has<LightEntity>, Option<&'static mut TerrainDraws>);
type CulledViewFilter = Or<(With<LightEntity>, With<ExtractedCamera>)>;

/// Culls the visible chunks against each view's frustum, in parallel over views.
/// Translucent chunks are sorted back to front; the rest keep page order, so pages rarely
/// switch.
fn cull(
    mut commands: Commands,
    gpu: Res<TerrainGpu>,
    mut views: Query<CulledView, CulledViewFilter>,
) {
    // Chunks cave culling leaves visible, with their bounds' minimum corner.
    let candidates: Vec<(Vec3, &ChunkRecord)> = gpu
        .chunks
        .iter()
        .filter(|(pos, _)| gpu.visible.contains(*pos))
        .map(|(pos, record)| (pos.origin().as_vec3(), record))
        .collect();
    for (entity, _, _, draws) in &views {
        if draws.is_none() {
            commands.entity(entity).insert(TerrainDraws::default());
        }
    }
    views.par_iter_mut().for_each(|(_, view, is_light, draws)| {
        let Some(mut draws) = draws else { return };
        let clip_from_world = view
            .clip_from_world
            .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let frustum = Frustum::new(clip_from_world, is_light);
        let eye = view.world_from_view.translation();
        let passes: &[TerrainPass] = if is_light { &SHADOW_PASSES } else { &PASSES };
        for pass in &mut draws.passes {
            pass.clear();
        }
        let mut translucent = Vec::new();
        let mut blades = Vec::new();
        for &(min, record) in &candidates {
            if !frustum.intersects(min, min + CHUNK_SIZE as f32) {
                continue;
            }
            // Distance from the camera (not a shadow view's light) to the chunk's bounds.
            let distance = (gpu.camera.clamp(min, min + CHUNK_SIZE as f32) - gpu.camera).length();
            let near = distance < FRINGE_DISTANCE;
            if let Some(seeds) = &record.blades
                && !is_light
                && distance < BLADE_DISTANCE
            {
                let per_seed = BLADES * BLADE_VERTICES;
                let vertices = seeds.start * per_seed..seeds.end * per_seed;
                blades.push((distance, BladeDraw { vertices, slot: record.slot }));
            }
            for &pass in passes {
                if pass == TerrainPass::Fringe && !near {
                    continue;
                }
                let Some(allocation) = &record.passes[pass as usize] else { continue };
                let draw = Draw {
                    page: allocation.page,
                    indices: allocation.indices.clone(),
                    base_vertex: allocation.vertices.start as i32,
                    slot: record.slot,
                };
                if pass == TerrainPass::Translucent {
                    let centre = min + CHUNK_SIZE as f32 / 2.0;
                    translucent.push((centre.distance_squared(eye), draw));
                } else {
                    draws.passes[pass as usize].push(draw);
                }
            }
        }
        draws.distant.clear();
        if !is_light {
            for (&key, tile) in &gpu.tiles {
                if frustum.intersects(tile.min, tile.max) && !(key.level == 0 && gpu.covered_by_real(key)) {
                    draws.distant.push(Draw {
                        page: tile.mesh.page,
                        indices: tile.mesh.indices.clone(),
                        base_vertex: tile.mesh.vertices.start as i32,
                        slot: tile.slot,
                    });
                }
            }
            draws.distant.sort_unstable_by_key(|draw| draw.page);
        }
        for pass in &mut draws.passes {
            pass.sort_unstable_by_key(|draw| draw.page);
        }
        translucent.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        draws.passes[TerrainPass::Translucent as usize].extend(translucent.into_iter().map(|(_, draw)| draw));
        // Nearest first, so nearer blades hide the farther ones' fragments.
        blades.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        draws.blades.clear();
        draws.blades.extend(blades.into_iter().map(|(_, draw)| draw));
    });
}

/// The planes of a view frustum, as `normal · p + d >= 0` inside.
struct Frustum {
    planes: Vec<Vec4>,
}

impl Frustum {
    /// The four side planes, plus for shadow views the far plane. Shadow views keep
    /// casters nearer the light than their near plane (their depth is unclipped), and
    /// cameras have no far plane (infinite reverse-Z).
    fn new(clip_from_world: Mat4, is_light: bool) -> Self {
        let rows = [clip_from_world.row(0), clip_from_world.row(1), clip_from_world.row(2), clip_from_world.row(3)];
        let mut planes = vec![rows[3] + rows[0], rows[3] - rows[0], rows[3] + rows[1], rows[3] - rows[1]];
        if is_light {
            planes.push(rows[2]);
        }
        Self { planes }
    }

    fn intersects(&self, min: Vec3, max: Vec3) -> bool {
        let centre = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        self.planes.iter().all(|plane| {
            let normal = plane.truncate();
            normal.dot(centre) + normal.abs().dot(half) + plane.w >= 0.0
        })
    }
}

/// Marks the render entity that stands for one terrain pass in the phases.
#[derive(Component, Clone, Copy)]
pub(super) struct TerrainPassItem(TerrainPass);

#[derive(Resource)]
struct TerrainPipelines {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    vertex_layout: MeshVertexBufferLayout,
    /// One render entity per pass.
    items: [Entity; 5],
    main: FxHashMap<(MeshPipelineKey, TerrainPass), CachedRenderPipelineId>,
    shadow: FxHashMap<TerrainPass, CachedRenderPipelineId>,
    distant_layout: BindGroupLayoutDescriptor,
    distant_shader: Handle<Shader>,
    distant_vertex_layout: MeshVertexBufferLayout,
    distant_item: Entity,
    distant: FxHashMap<MeshPipelineKey, CachedRenderPipelineId>,
    blade_shader: Handle<Shader>,
    blade_item: Entity,
    blades: FxHashMap<MeshPipelineKey, CachedRenderPipelineId>,
}

fn init_pipelines(mut commands: Commands, asset_server: Res<AssetServer>) {
    let layout = BindGroupLayoutDescriptor::new(
        "terrain_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                texture_2d_array(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d_array(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only_sized(false, None),
                // Grass blade seeds and the wind, for grass.wgsl and wind.wgsl.
                storage_buffer_read_only_sized(false, None),
                uniform_buffer::<WindUniform>(false),
            ),
        ),
    );
    let attribute = ATTRIBUTE_VOXEL.at_shader_location(0);
    let vertex_layout = MeshVertexBufferLayout::new(
        vec![ATTRIBUTE_VOXEL.id],
        VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: bevy::render::render_resource::VertexStepMode::Vertex,
            attributes: vec![bevy::render::render_resource::VertexAttribute {
                format: ATTRIBUTE_VOXEL.format,
                offset: 0,
                shader_location: attribute.shader_location,
            }],
        },
    );
    let items = PASSES.map(|pass| commands.spawn(TerrainPassItem(pass)).id());
    let distant_layout = BindGroupLayoutDescriptor::new(
        "distant_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                storage_buffer_read_only_sized(false, None),
                uniform_buffer::<DistantView>(false),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    );
    let distant_vertex_layout = MeshVertexBufferLayout::new(
        vec![ATTRIBUTE_LOD.id],
        VertexBufferLayout {
            array_stride: size_of::<LodVertex>() as u64,
            step_mode: bevy::render::render_resource::VertexStepMode::Vertex,
            attributes: vec![bevy::render::render_resource::VertexAttribute {
                format: ATTRIBUTE_LOD.format,
                offset: 0,
                shader_location: 0,
            }],
        },
    );
    let distant_item = commands.spawn_empty().id();
    let blade_item = commands.spawn_empty().id();
    commands.insert_resource(TerrainPipelines {
        layout,
        shader: asset_server.load(VOXEL_SHADER),
        vertex_layout,
        items,
        main: FxHashMap::default(),
        shadow: FxHashMap::default(),
        distant_layout,
        distant_shader: asset_server.load(DISTANT_SHADER),
        distant_vertex_layout,
        distant_item,
        distant: FxHashMap::default(),
        blade_shader: asset_server.load(GRASS_SHADER),
        blade_item,
        blades: FxHashMap::default(),
    });
}

/// Shader defs and face culling of each pass.
fn pass_setup(pass: TerrainPass) -> (&'static [&'static str], Option<Face>) {
    match pass {
        TerrainPass::Opaque => (&[], Some(Face::Back)),
        TerrainPass::Cutout => (&["CUTOUT"], Some(Face::Back)),
        // One face serves the two leaves on either side of it.
        TerrainPass::Inner => (&["CUTOUT", "INNER"], None),
        // Water is visible from below too.
        TerrainPass::Translucent => (&["TRANSLUCENT"], None),
        // Both windings are meshed.
        TerrainPass::Fringe => (&["CUTOUT"], Some(Face::Back)),
    }
}

/// The mesh pipeline's descriptor for a view, with our own shader, vertex buffer and bind
/// group 2 in place of a mesh's: the view setup, shader defs and targets stay Bevy's.
pub(super) fn mesh_like(
    mesh_pipeline: &MeshPipeline,
    layouts: &mut MeshVertexBufferLayouts,
    key: MeshPipelineKey,
    vertex_layout: &MeshVertexBufferLayout,
    group: &BindGroupLayoutDescriptor,
    shader: &Handle<Shader>,
    label: String,
) -> Option<RenderPipelineDescriptor> {
    let key = key | MeshPipelineKey::from_primitive_topology_and_strip_index(PrimitiveTopology::TriangleList, None);
    let mut descriptor = match mesh_pipeline.specialize(key, &layouts.insert(vertex_layout.clone())) {
        Ok(descriptor) => descriptor,
        Err(e) => {
            error!("{label} pipeline: {e}");
            return None;
        }
    };
    descriptor.label = Some(label.into());
    descriptor.layout.truncate(2);
    descriptor.layout.push(group.clone());
    descriptor.vertex.shader = shader.clone();
    descriptor.vertex.buffers = vec![vertex_layout.layout().clone()];
    if let Some(fragment) = descriptor.fragment.as_mut() {
        fragment.shader = shader.clone();
    }
    Some(descriptor)
}

impl TerrainPipelines {
    /// The main pass pipeline of a terrain pass.
    fn main(
        &mut self,
        view_key: MeshPipelineKey,
        pass: TerrainPass,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        cache: &PipelineCache,
    ) -> Option<CachedRenderPipelineId> {
        if let Some(&id) = self.main.get(&(view_key, pass)) {
            return Some(id);
        }
        let alpha = match pass {
            TerrainPass::Opaque => MeshPipelineKey::BLEND_OPAQUE,
            TerrainPass::Cutout | TerrainPass::Inner | TerrainPass::Fringe => MeshPipelineKey::MAY_DISCARD,
            TerrainPass::Translucent => MeshPipelineKey::BLEND_ALPHA,
        };
        let key = view_key | alpha;
        let label = format!("terrain_{pass:?}");
        let mut descriptor = mesh_like(mesh_pipeline, layouts, key, &self.vertex_layout, &self.layout, &self.shader, label)?;
        if pass == TerrainPass::Translucent {
            // The scene so far, which the water pass binds (see `water`).
            descriptor.layout.push(water::layout(key.msaa_samples() > 1));
        }
        let (defs, cull_mode) = pass_setup(pass);
        descriptor.primitive.cull_mode = cull_mode;
        // With several samples per pixel, cut-outs cover samples by their alpha instead of
        // passing or failing whole: leaves and plants stay smooth in the distance.
        let alpha_to_coverage =
            key.msaa_samples() > 1 && matches!(pass, TerrainPass::Cutout | TerrainPass::Inner | TerrainPass::Fringe);
        descriptor.multisample.alpha_to_coverage_enabled = alpha_to_coverage;
        // The vertex stage too: what it passes on differs by pass (water's flow).
        descriptor.vertex.shader_defs.extend(defs.iter().map(|&def| ShaderDefVal::from(def)));
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.extend(defs.iter().map(|&def| ShaderDefVal::from(def)));
            if alpha_to_coverage {
                fragment.shader_defs.push("ALPHA_TO_COVERAGE".into());
            }
        }
        let id = cache.queue_render_pipeline(descriptor);
        self.main.insert((view_key, pass), id);
        Some(id)
    }

    /// The distant terrain pipeline, with `lod.wgsl`.
    fn distant(
        &mut self,
        view_key: MeshPipelineKey,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        cache: &PipelineCache,
    ) -> Option<CachedRenderPipelineId> {
        if let Some(&id) = self.distant.get(&view_key) {
            return Some(id);
        }
        let descriptor = mesh_like(
            mesh_pipeline,
            layouts,
            view_key,
            &self.distant_vertex_layout,
            &self.distant_layout,
            &self.distant_shader,
            "distant_terrain".into(),
        )?;
        let id = cache.queue_render_pipeline(descriptor);
        self.distant.insert(view_key, id);
        Some(id)
    }

    /// The grass blade pipeline, with `grass.wgsl`: no vertex buffer, both sides drawn.
    fn blades(
        &mut self,
        view_key: MeshPipelineKey,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        cache: &PipelineCache,
    ) -> Option<CachedRenderPipelineId> {
        if let Some(&id) = self.blades.get(&view_key) {
            return Some(id);
        }
        let mut descriptor =
            mesh_like(mesh_pipeline, layouts, view_key, &self.vertex_layout, &self.layout, &self.blade_shader, "grass_blades".into())?;
        descriptor.vertex.buffers.clear();
        descriptor.primitive.cull_mode = None;
        let id = cache.queue_render_pipeline(descriptor);
        self.blades.insert(view_key, id);
        Some(id)
    }

    /// The shadow pass pipeline: depth only, with the alpha test for cut-out faces.
    fn shadow(&mut self, pass: TerrainPass, prepass: &PrepassPipeline, cache: &PipelineCache) -> CachedRenderPipelineId {
        if let Some(&id) = self.shadow.get(&pass) {
            return id;
        }
        let emulate_unclipped = !prepass.depth_clip_control_supported;
        let mut defs: Vec<ShaderDefVal> = vec!["PREPASS_PIPELINE".into()];
        if emulate_unclipped {
            defs.push("UNCLIPPED_DEPTH_ORTHO_EMULATION".into());
        }
        let (pass_defs, cull_mode) = pass_setup(pass);
        defs.extend(pass_defs.iter().map(|&def| ShaderDefVal::from(def)));
        let fragment = (pass != TerrainPass::Opaque || emulate_unclipped).then(|| FragmentState {
            shader: self.shader.clone(),
            shader_defs: defs.clone(),
            targets: Vec::new(),
            ..default()
        });
        let descriptor = RenderPipelineDescriptor {
            label: Some(format!("terrain_shadow_{pass:?}").into()),
            layout: vec![prepass.view_layout_no_motion_vectors.clone(), prepass.empty_layout.clone(), self.layout.clone()],
            vertex: VertexState { shader: self.shader.clone(), shader_defs: defs, buffers: vec![self.vertex_layout.layout().clone()], ..default() },
            fragment,
            primitive: PrimitiveState { cull_mode, unclipped_depth: !emulate_unclipped, ..default() },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            multisample: MultisampleState::default(),
            ..default()
        };
        let id = cache.queue_render_pipeline(descriptor);
        self.shadow.insert(pass, id);
        id
    }
}

/// The phases terrain draws in and their draw functions.
#[derive(SystemParam)]
struct Phases<'w> {
    opaque: ResMut<'w, ViewBinnedRenderPhases<Opaque3d>>,
    mask: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3d>>,
    shadow: ResMut<'w, ViewBinnedRenderPhases<Shadow>>,
    opaque_draw: Res<'w, DrawFunctions<Opaque3d>>,
    mask_draw: Res<'w, DrawFunctions<AlphaMask3d>>,
    shadow_draw: Res<'w, DrawFunctions<Shadow>>,
}

/// Puts one item per pass into every view's phases. Binned phases keep items between
/// frames, so each is replaced rather than added again.
#[allow(clippy::too_many_arguments)]
fn queue(
    mut pipelines: ResMut<TerrainPipelines>,
    mesh_pipeline: Res<MeshPipeline>,
    prepass: Res<PrepassPipeline>,
    mut layouts: ResMut<MeshVertexBufferLayouts>,
    cache: Res<PipelineCache>,
    view_keys: Res<ViewKeyCache>,
    mut phases: Phases,
    keys: Res<TerrainKeys>,
    mut views: Query<(&ExtractedView, Has<LightEntity>, Option<&mut TerrainDraws>)>,
) {
    let items = PASSES.map(|pass| (pipelines.items[pass as usize], keys.passes[pass as usize]));
    let distant_item = (pipelines.distant_item, keys.distant);
    let blade_item = (pipelines.blade_item, keys.blades);
    let registered = "terrain draw is registered";
    let opaque_draw = phases.opaque_draw.read().get_id::<DrawTerrainMain>().expect(registered);
    let mask_draw = phases.mask_draw.read().get_id::<DrawTerrainMain>().expect(registered);
    let shadow_draw = phases.shadow_draw.read().get_id::<DrawTerrainShadow>().expect(registered);
    let distant_draw = phases.opaque_draw.read().get_id::<DrawDistantTerrain>().expect(registered);
    let blade_draw = phases.opaque_draw.read().get_id::<DrawGrassBlades>().expect(registered);
    let bin = || Opaque3dBinKey { asset_id: AssetId::<Mesh>::default().untyped() };

    for (view, is_light, draws) in &mut views {
        let retained = &view.retained_view_entity;
        if is_light {
            let Some(phase) = phases.shadow.get_mut(retained) else { continue };
            for pass in SHADOW_PASSES {
                let pipeline = pipelines.shadow(pass, &prepass, &cache);
                let item = items[pass as usize];
                phase.remove(item.1);
                let key = ShadowBatchSetKey { pipeline, draw_function: shadow_draw, material_bind_group_index: None, slabs: default() };
                let bin = ShadowBinKey { asset_id: bin().asset_id };
                phase.add(key, bin, item, InputUniformIndex::default(), BinnedRenderPhaseType::NonMesh);
            }
            continue;
        }
        let Some(&view_key) = view_keys.get(retained) else { continue };
        let distant = pipelines.distant(view_key, &mesh_pipeline, &mut layouts, &cache);
        let blades = pipelines.blades(view_key, &mesh_pipeline, &mut layouts, &cache);
        let mut pipeline = |pass| pipelines.main(view_key, pass, &mesh_pipeline, &mut layouts, &cache);
        if let Some(phase) = phases.opaque.get_mut(retained) {
            let opaque = (pipeline(TerrainPass::Opaque), opaque_draw, items[TerrainPass::Opaque as usize]);
            for (id, draw_function, item) in [opaque, (distant, distant_draw, distant_item), (blades, blade_draw, blade_item)] {
                phase.remove(item.1);
                let Some(id) = id else { continue };
                let key = Opaque3dBatchSetKey {
                    pipeline: id,
                    draw_function,
                    material_bind_group_index: None,
                    slabs: default(),
                    lightmap_slab: None,
                };
                phase.add(key, bin(), item, InputUniformIndex::default(), BinnedRenderPhaseType::NonMesh);
            }
        }
        if let Some(phase) = phases.mask.get_mut(retained) {
            for pass in [TerrainPass::Cutout, TerrainPass::Inner, TerrainPass::Fringe] {
                let Some(id) = pipeline(pass) else { continue };
                let item = items[pass as usize];
                phase.remove(item.1);
                let key = OpaqueNoLightmap3dBatchSetKey {
                    pipeline: id,
                    draw_function: mask_draw,
                    material_bind_group_index: None,
                    slabs: default(),
                };
                let bin = OpaqueNoLightmap3dBinKey { asset_id: bin().asset_id };
                phase.add(key, bin, item, InputUniformIndex::default(), BinnedRenderPhaseType::NonMesh);
            }
        }
        // Water and ice are drawn by the water pass, which reads the scene behind them.
        let translucent = pipeline(TerrainPass::Translucent);
        if let Some(mut draws) = draws {
            let entity = items[TerrainPass::Translucent as usize];
            draws.water = translucent.map(|pipeline| WaterDraw { pipeline, entity });
        }
    }
}

type DrawTerrainMain = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawTerrain);
type DrawTerrainShadow = (SetItemPipeline, SetPrepassViewBindGroup<0>, SetPrepassViewEmptyBindGroup<1>, DrawTerrain);
type DrawDistantTerrain = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawDistant);
type DrawGrassBlades = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawBlades);

/// Draws a camera's culled grass blades, pulled from the seed buffer by vertex index.
struct DrawBlades;

impl<P: PhaseItem> RenderCommand<P> for DrawBlades {
    type Param = SRes<TerrainGpu>;
    type ViewQuery = Read<TerrainDraws>;
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        draws: &'w TerrainDraws,
        _: Option<()>,
        gpu: SystemParamItem<'w, '_, Self::Param>,
        render_pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bind_group) = &gpu.into_inner().bind_group else { return RenderCommandResult::Skip };
        render_pass.set_bind_group(2, bind_group, &[]);
        for draw in &draws.blades {
            render_pass.draw(draw.vertices.clone(), draw.slot..draw.slot + 1);
        }
        RenderCommandResult::Success
    }
}

/// Draws a camera's culled distant terrain tiles.
struct DrawDistant;

impl<P: PhaseItem> RenderCommand<P> for DrawDistant {
    type Param = SRes<TerrainGpu>;
    type ViewQuery = Read<TerrainDraws>;
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        draws: &'w TerrainDraws,
        _: Option<()>,
        gpu: SystemParamItem<'w, '_, Self::Param>,
        render_pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let gpu = gpu.into_inner();
        let Some(bind_group) = &gpu.distant_bind_group else { return RenderCommandResult::Skip };
        render_pass.set_bind_group(2, bind_group, &[]);
        let mut current = None;
        for draw in &draws.distant {
            if current != Some(draw.page) {
                let Some(page) = gpu.tile_pages.pages.get(draw.page as usize) else { return RenderCommandResult::Skip };
                render_pass.set_vertex_buffer(0, page.vertex_buffer.slice(..));
                render_pass.set_index_buffer(page.index_buffer.slice(..), IndexFormat::Uint32);
                current = Some(draw.page);
            }
            render_pass.draw_indexed(draw.indices.clone(), draw.base_vertex, draw.slot..draw.slot + 1);
        }
        RenderCommandResult::Success
    }
}

/// Draws a view's culled chunks of the item's pass.
pub(super) struct DrawTerrain;

impl<P: PhaseItem> RenderCommand<P> for DrawTerrain {
    type Param = SRes<TerrainGpu>;
    type ViewQuery = Read<TerrainDraws>;
    type ItemQuery = Read<TerrainPassItem>;

    fn render<'w>(
        _item: &P,
        draws: &'w TerrainDraws,
        pass_item: Option<&'w TerrainPassItem>,
        gpu: SystemParamItem<'w, '_, Self::Param>,
        render_pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let gpu = gpu.into_inner();
        let (Some(TerrainPassItem(pass)), Some(bind_group)) = (pass_item, &gpu.bind_group) else {
            return RenderCommandResult::Skip;
        };
        render_pass.set_bind_group(2, bind_group, &[]);
        let mut current = None;
        for draw in &draws.passes[*pass as usize] {
            if current != Some(draw.page) {
                let Some(page) = gpu.chunk_pages.pages.get(draw.page as usize) else { return RenderCommandResult::Skip };
                render_pass.set_vertex_buffer(0, page.vertex_buffer.slice(..));
                render_pass.set_index_buffer(page.index_buffer.slice(..), IndexFormat::Uint32);
                current = Some(draw.page);
            }
            render_pass.draw_indexed(draw.indices.clone(), draw.base_vertex, draw.slot..draw.slot + 1);
        }
        RenderCommandResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocator_reuses_and_merges_freed_ranges() {
        let mut alloc = RangeAllocator::new(100);
        let a = alloc.allocate(30).unwrap();
        let b = alloc.allocate(30).unwrap();
        let c = alloc.allocate(30).unwrap();
        assert_eq!((a.start, b.start, c.start), (0, 30, 60));
        assert!(alloc.allocate(20).is_none());
        alloc.release(a);
        alloc.release(c);
        assert_eq!(alloc.free, vec![0..30, 60..100]);
        alloc.release(b);
        assert_eq!(alloc.free, vec![0..100]);
        assert_eq!(alloc.allocate(100), Some(0..100));
    }

    #[test]
    fn frustum_keeps_boxes_in_view() {
        let projection = Mat4::perspective_infinite_reverse_rh(1.0, 1.0, 0.1);
        let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let frustum = Frustum::new(projection * view, false);
        assert!(frustum.intersects(Vec3::new(-1.0, -1.0, -20.0), Vec3::new(1.0, 1.0, -18.0)));
        assert!(!frustum.intersects(Vec3::new(-1.0, -1.0, 18.0), Vec3::new(1.0, 1.0, 20.0)));
        assert!(!frustum.intersects(Vec3::new(50.0, -1.0, -20.0), Vec3::new(52.0, 1.0, -18.0)));
    }
}
