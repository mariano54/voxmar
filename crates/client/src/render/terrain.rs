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
    ExtractedDirectionalLight, LightEntity, MeshPipeline, MeshPipelineKey, PrepassPipeline, SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup,
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
    BindGroup, BindGroupEntries, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, CommandEncoderDescriptor, CompareFunction, DepthBiasState, DepthStencilState, Face, FragmentState,
    IndexFormat, MultisampleState, PipelineCache, PrimitiveState, RenderPipelineDescriptor, SamplerBindingType,
    ShaderStages, ShaderType, SpecializedMeshPipeline, StencilState, TextureSampleType, VertexState, encase,
    binding_types::{sampler, storage_buffer_read_only_sized, texture_2d, texture_2d_array, texture_3d, uniform_buffer},
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
use voxmar_engine::coords::{CHUNK_SIZE, ChunkPos, ColumnPos};
use voxmar_engine::mesh::{MeshBuffers, Vertex};
use voxmar_engine::world::{COLUMN_AREA, DEFAULT_COLORS};

mod compact;

use compact::Compaction;

use super::cloud_map::CloudMap;
use super::atmosphere::{Atmosphere, AtmosphereBuffer};
use super::lod::{LodVertex, TileKey, TileMesh};
use super::water::{self, WaterParams};
use super::wind::Breeze;
use super::{ATTRIBUTE_LOD, ATTRIBUTE_VOXEL, DISTANT_SHADER, GRASS_SHADER, TerrainPass, VOXEL_SHADER};

/// Chunk vertices are handed out in blocks of this many, each owned by one chunk (see
/// `PagePool`).
const VERTEX_BLOCK: u32 = 64;
/// Vertices per distant terrain page.
const TILE_PAGE_VERTICES: u32 = 1 << 20;
const PASSES: [TerrainPass; 5] =
    [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Inner, TerrainPass::Translucent, TerrainPass::Fringe];
/// Only these passes cast shadows: leaf crowns' shells already shade, and water would
/// shadow itself.
const SHADOW_PASSES: [TerrainPass; 3] = [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Fringe];
/// Leaf fringes are drawn for chunks this close to the camera, in blocks; further out they
/// would be sub-pixel.
const FRINGE_DISTANCE: f32 = 96.0;
/// Plant faces' face index (voxel.wgsl's `FACE_PLANT`).
const FACE_PLANT: usize = 6;
/// The most grass blades a block grows, and the distances in blocks over which they thin
/// out and shrink away (grass.wgsl's `BLADES` and `FADE`); further chunks draw none.
const BLADES: f32 = 16.0;
const BLADE_FADE: [f32; 2] = [16.0, 34.0];
/// Points and indices of one blade (two segments of a strip; see grass.wgsl).
const BLADE_POINTS: u32 = 5;
const BLADE_INDICES: [u32; 9] = [0, 1, 2, 2, 1, 3, 2, 3, 4];
/// Blades in the blade index buffer; longer draws are split.
const PATTERN_BLADES: u32 = 1 << 14;

/// How many blades each block of a chunk this far away grows: as many as grass.wgsl's
/// `kept` leaves at its nearest point.
fn blades_at(distance: f32) -> u32 {
    let t = ((distance - BLADE_FADE[0]) / (BLADE_FADE[1] - BLADE_FADE[0])).clamp(0.0, 1.0);
    let fade = 1.0 - t * t * (3.0 - 2.0 * t);
    if fade <= 0.0 { 0 } else { (BLADES * (0.25 + 0.75 * fade)).ceil() as u32 }
}

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
            .add_systems(RenderStartup, (init_pipelines, compact::init_pipeline))
            .add_systems(
                Render,
                (
                    // Before culling, which runs in `Queue`: draws must never refer to
                    // pages a world change has just dropped.
                    receive.in_set(RenderSystems::PrepareAssets),
                    prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
                    (cull, queue).chain().in_set(RenderSystems::Queue),
                    compact::dispatch.in_set(RenderSystems::PrepareResources),
                ),
            );
    }
}

/// The block texture array, and each texture layer's overlay and glow (`LayerInfo`).
#[derive(Resource, Clone, ExtractResource)]
pub struct TerrainTextures {
    pub textures: Handle<Image>,
    pub layers: Handle<Image>,
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
    ColumnColours { column: IVec2, colours: Vec<[[u8; 3]; 3]> },
    RemoveColumn(IVec2),
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

    /// Sets the biome colours of a column's blocks (grass, foliage and water, sRGB, indexed
    /// z * 32 + x), which its chunks are tinted with. Send them before the chunks.
    pub fn upload_column(&self, column: ColumnPos, colours: Vec<[[u8; 3]; 3]>) {
        let _ = self.tx.send(Message::ColumnColours { column: column.0, colours });
    }

    /// Forgets a column's colours; its chunks must be gone too.
    pub fn remove_column(&self, column: ColumnPos) {
        let _ = self.tx.send(Message::RemoveColumn(column.0));
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
    vertices: RangeAllocator,
    /// Distant tile pages only: the tiles' indices. Chunk pages keep none, since a
    /// chunk's quads say how to split themselves into triangles (see `compact`).
    indices: Option<(Buffer, RangeAllocator)>,
    /// Chunk pages only: the chunk slot of each block of `VERTEX_BLOCK` vertices, and the
    /// bind group (group 3) that holds it.
    chunks: Option<(Buffer, BindGroup)>,
}

/// Pages of one vertex format.
///
/// Chunk pages hold only vertices, four to a quad, handed out in whole blocks of
/// `VERTEX_BLOCK` and recording which chunk owns each block, so the shader finds a
/// vertex's chunk from its index. Each view builds the indices of the quads it draws
/// (see `compact`), so a page stores no index data at all, 24 bytes a quad saved.
struct PagePool {
    label: &'static str,
    vertex_size: u64,
    /// Vertices per page; distant tile pages get one and a half times as many indices.
    page_vertices: u32,
    pages: Vec<Page>,
    chunk_pages: bool,
}

/// What a chunk page's draw needs of a mesh being added: its owner, and the layout of
/// the page's bind group.
struct Owner<'a> {
    slot: u32,
    layout: &'a BindGroupLayout,
}

impl PagePool {
    fn new(label: &'static str, vertex_size: usize, page_vertices: u32, chunk_pages: bool) -> Self {
        Self { label, vertex_size: vertex_size as u64, page_vertices, pages: Vec::new(), chunk_pages }
    }

    /// Copies a mesh into the first page with room, adding a page if none has. Chunk
    /// meshes' indices are not stored.
    fn allocate<V: bytemuck::Pod>(
        &mut self,
        device: &RenderDevice,
        queue: &RenderQueue,
        vertices: &[V],
        indices: &[u32],
        owner: Option<Owner>,
    ) -> Allocation {
        let len = vertices.len() as u32;
        let reserved = if self.chunk_pages { len.next_multiple_of(VERTEX_BLOCK) } else { len };
        let index_count = indices.len() as u32;
        let fit = |page: &mut Page| -> Option<(Range<u32>, Range<u32>)> {
            let vertices = page.vertices.allocate(reserved)?;
            let Some((_, free)) = &mut page.indices else { return Some((vertices, 0..0)) };
            match free.allocate(index_count) {
                Some(indices) => Some((vertices, indices)),
                None => {
                    page.vertices.release(vertices);
                    None
                }
            }
        };
        let found = self.pages.iter_mut().enumerate().find_map(|(i, page)| fit(page).map(|ranges| (i, ranges)));
        let (page, (vertex_range, index_range)) = found.unwrap_or_else(|| {
            let mut page = self.new_page(device, owner.as_ref().map(|owner| owner.layout));
            let ranges = fit(&mut page).expect("a mesh fits a page");
            self.pages.push(page);
            (self.pages.len() - 1, ranges)
        });
        let target = &self.pages[page];
        queue.write_buffer(&target.vertex_buffer, vertex_range.start as u64 * self.vertex_size, bytemuck::cast_slice(vertices));
        if let (Some((chunks, _)), Some(owner)) = (&target.chunks, owner) {
            let blocks = vec![owner.slot; (reserved / VERTEX_BLOCK) as usize];
            queue.write_buffer(chunks, (vertex_range.start / VERTEX_BLOCK) as u64 * 4, bytemuck::cast_slice(&blocks));
        }
        if let Some((buffer, _)) = &target.indices {
            queue.write_buffer(buffer, index_range.start as u64 * 4, bytemuck::cast_slice(indices));
        }
        Allocation { page: page as u32, vertices: vertex_range.start..vertex_range.start + len, reserved, indices: index_range }
    }

    fn free(&mut self, allocation: Allocation) {
        let page = &mut self.pages[allocation.page as usize];
        page.vertices.release(allocation.vertices.start..allocation.vertices.start + allocation.reserved);
        if let Some((_, free)) = &mut page.indices
            && !allocation.indices.is_empty()
        {
            free.release(allocation.indices);
        }
    }

    fn new_page(&self, device: &RenderDevice, layout: Option<&BindGroupLayout>) -> Page {
        let index = self.pages.len();
        let buffer = |kind: &str, size: u64, usage: BufferUsages| {
            device.create_buffer(&BufferDescriptor {
                label: Some(&format!("{}_{kind}_{index}", self.label)),
                size,
                usage: usage | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let chunks = layout.filter(|_| self.chunk_pages).map(|layout| {
            let chunks = buffer("chunks", (self.page_vertices / VERTEX_BLOCK) as u64 * 4, BufferUsages::STORAGE);
            let bind_group = device.create_bind_group(
                format!("{}_chunks_{index}", self.label).as_str(),
                layout,
                &BindGroupEntries::single(chunks.as_entire_binding()),
            );
            (chunks, bind_group)
        });
        let page_indices = self.page_vertices / 2 * 3;
        let indices = (!self.chunk_pages)
            .then(|| (buffer("indices", page_indices as u64 * 4, BufferUsages::INDEX), RangeAllocator::new(page_indices)));
        // Chunk vertices are also read by the pass that builds each view's indices.
        let usage = if self.chunk_pages { BufferUsages::VERTEX | BufferUsages::STORAGE } else { BufferUsages::VERTEX };
        Page {
            vertex_buffer: buffer("vertices", self.page_vertices as u64 * self.vertex_size, usage),
            vertices: RangeAllocator::new(self.page_vertices),
            indices,
            chunks,
        }
    }
}

/// Where a mesh lives: its vertices, the share of the page they hold (whole blocks, for
/// chunk pages) and, on distant tile pages, its indices.
struct Allocation {
    page: u32,
    vertices: Range<u32>,
    reserved: u32,
    indices: Range<u32>,
}

/// A chunk's meshes are drawn in streams, each with its own pages: one per pass, with
/// opaque faces split by direction, so a view skips the faces turned away from it (half
/// of all opaque faces) before the GPU ever sees them, and plants apart from the other
/// cut-out faces, so they are skipped where they have sunk away. Streams 0-5 are opaque
/// faces by `Face`, then cut-out faces, plants, inner, translucent and fringe.
const STREAMS: usize = 11;
/// The cut-out stream: leaves, glass and other blocks drawn with holes.
const CUTOUT: usize = 6;
const PLANTS: usize = 7;
const FRINGE: usize = 10;
/// Plants have sunk into the ground by this distance, in blocks (`PLANT_FADE` in
/// voxel.wgsl), so further chunks skip them.
const PLANT_DISTANCE: f32 = 320.0;

/// The streams a pass is drawn from.
fn streams(pass: TerrainPass) -> Range<usize> {
    match pass {
        TerrainPass::Opaque => 0..6,
        TerrainPass::Cutout => 6..8,
        pass => 6 + pass as usize..7 + pass as usize,
    }
}

/// Indices each quad of a stream draws with: plants and leaf fringes are drawn from both
/// sides, with both windings.
fn indices_per_quad(stream: usize) -> u32 {
    match stream {
        PLANTS | FRINGE => 12,
        _ => 6,
    }
}

/// Vertices per page of each stream: the opaque directions and cut-outs hold most quads.
fn stream_page_vertices(stream: usize) -> u32 {
    match stream {
        0..=6 => 1 << 21,
        PLANTS => 1 << 20,
        FRINGE => 1 << 18,
        _ => 1 << 19,
    }
}

/// Which opaque face directions might face a view, by `Face` index: a camera sees a face
/// pointing +x only from its +x side, a sun only faces pointing back at it.
fn facing(min: Vec3, max: Vec3, eye: Vec3, light: Option<Vec3>) -> [bool; 6] {
    match light {
        Some(towards) => [towards.x < 0.0, towards.x > 0.0, towards.y < 0.0, towards.y > 0.0, towards.z < 0.0, towards.z > 0.0],
        None => [eye.x > min.x, eye.x < max.x, eye.y > min.y, eye.y < max.y, eye.z > min.z, eye.z < max.z],
    }
}

/// The quads of a pass's mesh that go to each of its streams. The mesher emits opaque
/// faces grouped by face, in `Face` order, and plants after the other cut-out faces.
fn split(pass: TerrainPass, mesh: &MeshBuffers) -> Vec<Range<usize>> {
    let face = |quad: &[Vertex]| ((quad[0][0] >> 18) & 7) as usize;
    let group = |face: usize| match pass {
        TerrainPass::Opaque => face,
        TerrainPass::Cutout => (face >= FACE_PLANT) as usize,
        _ => 0,
    };
    let groups: Vec<usize> = mesh.vertices.chunks(4).map(|quad| group(face(quad))).collect();
    debug_assert!(groups.is_sorted(), "a pass's faces come grouped by stream");
    streams(pass)
        .enumerate()
        .map(|(g, _)| groups.partition_point(|&q| q < g)..groups.partition_point(|&q| q <= g))
        .collect()
}

#[derive(Default)]
struct ChunkRecord {
    /// Index into the origin table, which the shader finds from the vertex's block.
    slot: u32,
    streams: [Option<Allocation>; STREAMS],
    /// Where the chunk's grass blade seeds are in the seed buffer.
    blades: Option<Range<u32>>,
}

impl ChunkRecord {
    fn is_empty(&self) -> bool {
        self.streams.iter().all(Option::is_none) && self.blades.is_none()
    }
}

/// Packs a column's biome colours for the shader: grass, foliage and water for each
/// block, a word (RGBA8, sRGB) each.
fn pack_colours(colours: &[[[u8; 3]; 3]]) -> Vec<u32> {
    colours.iter().flat_map(|kinds| kinds.map(|[r, g, b]| u32::from_le_bytes([r, g, b, 255]))).collect()
}

/// A storage buffer of words handed out in ranges, which grows (keeping its contents) as
/// needed: grass blade seeds, and the biome colours of columns.
struct Arena {
    label: &'static str,
    buffer: Option<Buffer>,
    ranges: RangeAllocator,
    capacity: u32,
}

impl Arena {
    fn new(label: &'static str) -> Self {
        Self { label, buffer: None, ranges: RangeAllocator::default(), capacity: 0 }
    }

    fn allocate(&mut self, device: &RenderDevice, queue: &RenderQueue, words: &[u32]) -> Range<u32> {
        let len = words.len() as u32;
        let range = self.ranges.allocate(len).unwrap_or_else(|| {
            self.grow(device, queue, len);
            self.ranges.allocate(len).expect("the arena has just grown")
        });
        self.write(queue, range.start, words);
        range
    }

    fn write(&self, queue: &RenderQueue, start: u32, words: &[u32]) {
        let buffer = self.buffer.as_ref().expect("the arena exists once it has room");
        queue.write_buffer(buffer, start as u64 * 4, bytemuck::cast_slice(words));
    }

    /// Forgets every range, keeping the buffer.
    fn clear(&mut self) {
        self.ranges = RangeAllocator::new(self.capacity);
    }

    /// Replaces the buffer with one at least twice as large, holding the same words.
    fn grow(&mut self, device: &RenderDevice, queue: &RenderQueue, at_least: u32) {
        let capacity = (self.capacity * 2).max(self.capacity + at_least).max(1 << 18);
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(self.label),
            size: capacity as u64 * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        if let Some(old) = &self.buffer {
            // Words written to the old buffer this frame land before the copy.
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some(self.label) });
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
    /// The flutter's phase, radians.
    flutter: f32,
}

#[derive(ShaderType, Clone, Copy, Default)]
struct DistantView {
    /// xy: column of the coverage mask's first bit, z: its width in columns, w: the real
    /// chunks' radius in columns.
    coverage: IVec4,
    /// The sky's zenith and horizon colours, which distant water reflects as real water
    /// does where its reflections leave the screen.
    zenith: Vec4,
    horizon: Vec4,
}

/// Render world state: pages, chunk records and the bind group shared by every draw.
#[derive(Resource)]
pub(super) struct TerrainGpu {
    /// Chunk pages of each stream.
    streams: [PagePool; STREAMS],
    tile_pages: PagePool,
    /// Each chunk's record, in `records`.
    chunks: FxHashMap<ChunkPos, u32>,
    /// Chunk records, and the places of forgotten ones for new chunks to take.
    records: Vec<ChunkRecord>,
    free_records: Vec<u32>,
    /// Each record's chunk's minimum corner, and whether it is drawn at all (a chunk cave
    /// culling leaves visible, not a forgotten record): all that culling a view reads for
    /// each chunk until one is in view, 16 bytes beside the records' few hundred, so a
    /// view's scan of every chunk stays in cache and no list of candidates is gathered
    /// first.
    bounds: Vec<(Vec3, bool)>,
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
    seeds: Arena,
    /// One blade's triangles, repeated `PATTERN_BLADES` times.
    blade_indices: Option<Buffer>,
    /// Biome colours of every column with real chunks: `COLUMN_COLOUR_WORDS` words each,
    /// found by the offset chunks keep in their origin's w.
    colours: Arena,
    column_colours: FxHashMap<IVec2, Range<u32>>,
    /// Colours for chunks whose column sent none.
    default_colours: Option<Range<u32>>,
    wind: Option<Buffer>,
    bind_group: Option<BindGroup>,
    distant_bind_group: Option<BindGroup>,
    /// Whether the bind groups must be rebuilt (the origin buffer was replaced).
    bind_group_dirty: bool,
    /// Counts clears, which replace every page.
    generation: u32,
    /// Counts changes to the chunks' index data, which views' gathered indices copy.
    revision: u64,
}

impl Default for TerrainGpu {
    fn default() -> Self {
        Self {
            streams: std::array::from_fn(|stream| {
                PagePool::new("terrain", size_of::<Vertex>(), stream_page_vertices(stream), true)
            }),
            tile_pages: PagePool::new("distant", size_of::<LodVertex>(), TILE_PAGE_VERTICES, false),
            chunks: FxHashMap::default(),
            records: Vec::new(),
            free_records: Vec::new(),
            bounds: Vec::new(),
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
            seeds: Arena::new("blade_seeds"),
            blade_indices: None,
            colours: Arena::new("column_colours"),
            column_colours: FxHashMap::default(),
            default_colours: None,
            wind: None,
            bind_group: None,
            distant_bind_group: None,
            bind_group_dirty: false,
            generation: 0,
            revision: 0,
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
    /// Takes cave culling's new set of visible chunks, marking each record.
    fn set_visible(&mut self, visible: FxHashSet<ChunkPos>) {
        for (pos, &index) in &self.chunks {
            self.bounds[index as usize].1 = visible.contains(pos);
        }
        self.visible = visible;
    }

    fn record(&mut self, chunk: ChunkPos) -> &mut ChunkRecord {
        let index = match self.chunks.get(&chunk) {
            Some(&index) => index,
            None => {
                let column = IVec2::new(chunk.0.x, chunk.0.z);
                let colours = self.column_colours.get(&column).or(self.default_colours.as_ref()).map_or(0, |range| range.start);
                let slot = self.new_slot(chunk.origin().extend(colours as i32));
                let bounds = (chunk.origin().as_vec3(), self.visible.contains(&chunk));
                let record = ChunkRecord { slot, ..default() };
                let index = match self.free_records.pop() {
                    Some(index) => {
                        self.records[index as usize] = record;
                        self.bounds[index as usize] = bounds;
                        index
                    }
                    None => {
                        self.records.push(record);
                        self.bounds.push(bounds);
                        self.records.len() as u32 - 1
                    }
                };
                self.chunks.insert(chunk, index);
                *self.columns.entry(column).or_default() += 1;
                index
            }
        };
        &mut self.records[index as usize]
    }

    /// The chunk's record, if it has one.
    fn record_mut(&mut self, chunk: ChunkPos) -> Option<&mut ChunkRecord> {
        let &index = self.chunks.get(&chunk)?;
        Some(&mut self.records[index as usize])
    }

    /// Forgets a chunk with nothing left to draw.
    fn forget_if_empty(&mut self, chunk: ChunkPos) {
        let Some(&index) = self.chunks.get(&chunk) else { return };
        if !self.records[index as usize].is_empty() {
            return;
        }
        self.chunks.remove(&chunk);
        self.free_records.push(index);
        self.bounds[index as usize].1 = false;
        let record = std::mem::take(&mut self.records[index as usize]);
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
        let Some(&index) = self.chunks.get(&chunk) else { return };
        let record = &mut self.records[index as usize];
        for stream in streams(pass) {
            if let Some(allocation) = record.streams[stream].take() {
                self.streams[stream].free(allocation);
                self.revision += 1;
            }
        }
        self.forget_if_empty(chunk);
    }

    fn upload(
        &mut self,
        device: &RenderDevice,
        queue: &RenderQueue,
        layout: &BindGroupLayout,
        chunk: ChunkPos,
        pass: TerrainPass,
        mesh: &MeshBuffers,
    ) {
        self.release(chunk, pass);
        let slot = self.record(chunk).slot;
        for (stream, quads) in streams(pass).zip(split(pass, mesh)) {
            if quads.is_empty() {
                continue;
            }
            let vertices = &mesh.vertices[quads.start * 4..quads.end * 4];
            let indices = &mesh.indices[quads.start * 6..quads.end * 6];
            let owner = Owner { slot, layout };
            let allocation = self.streams[stream].allocate(device, queue, vertices, indices, Some(owner));
            self.record(chunk).streams[stream] = Some(allocation);
            self.revision += 1;
        }
    }

    fn upload_blades(&mut self, device: &RenderDevice, queue: &RenderQueue, chunk: ChunkPos, seeds: &[u32]) {
        if let Some(range) = self.record_mut(chunk).and_then(|record| record.blades.take()) {
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

    fn upload_column_colours(&mut self, device: &RenderDevice, queue: &RenderQueue, column: IVec2, colours: &[[[u8; 3]; 3]]) {
        let words = pack_colours(colours);
        if let Some(range) = self.column_colours.get(&column) {
            self.colours.write(queue, range.start, &words);
            return;
        }
        let capacity = self.colours.capacity;
        let range = self.colours.allocate(device, queue, &words);
        self.bind_group_dirty |= self.colours.capacity != capacity;
        self.column_colours.insert(column, range);
    }

    fn remove_column_colours(&mut self, column: IVec2) {
        if let Some(range) = self.column_colours.remove(&column) {
            self.colours.ranges.release(range);
        }
    }

    fn remove_tile(&mut self, key: TileKey) {
        if let Some(tile) = self.tiles.remove(&key) {
            self.tile_pages.free(tile.mesh);
            self.free_slots.push(tile.slot);
        }
    }

    fn upload_tile(&mut self, device: &RenderDevice, queue: &RenderQueue, tile: TileMesh) {
        self.remove_tile(tile.key);
        let mesh = self.tile_pages.allocate(device, queue, &tile.vertices, &tile.indices, None);
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
        let mut seeds = std::mem::replace(&mut self.seeds, Arena::new("blade_seeds"));
        let mut colours = std::mem::replace(&mut self.colours, Arena::new("column_colours"));
        seeds.clear();
        colours.clear();
        *self = Self {
            origins_buffer: self.origins_buffer.take(),
            distant_view: self.distant_view.take(),
            coverage: self.coverage.take(),
            seeds,
            colours,
            blade_indices: self.blade_indices.take(),
            wind: self.wind.take(),
            bind_group_dirty: true,
            generation: self.generation + 1,
            revision: self.revision + 1,
            ..Self::default()
        };
    }

    /// Makes the arenas' buffers the first time, and the default colours after a clear.
    fn ensure_arenas(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        if self.seeds.buffer.is_none() {
            self.seeds.grow(device, queue, 0);
        }
        if self.colours.buffer.is_none() {
            self.colours.grow(device, queue, 0);
        }
        if self.blade_indices.is_none() {
            let indices: Vec<u32> =
                (0..PATTERN_BLADES).flat_map(|blade| BLADE_INDICES.map(|i| blade * BLADE_POINTS + i)).collect();
            let buffer = device.create_buffer(&BufferDescriptor {
                label: Some("blade_indices"),
                size: (indices.len() * 4) as u64,
                usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&indices));
            self.blade_indices = Some(buffer);
        }
        if self.default_colours.is_none() {
            let words = pack_colours(&[DEFAULT_COLORS; COLUMN_AREA]);
            self.default_colours = Some(self.colours.allocate(device, queue, &words));
        }
    }

    /// Writes this frame's wind.
    fn write_wind(&mut self, device: &RenderDevice, queue: &RenderQueue, breeze: Breeze) {
        let wind = WindUniform {
            flow: breeze.velocity.extend(breeze.offset.x).extend(breeze.offset.y),
            player: breeze.player.map_or(Vec4::ZERO, |feet| feet.extend(1.0)),
            gustiness: breeze.gustiness,
            flutter: breeze.flutter,
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
    fn write_coverage(&mut self, device: &RenderDevice, queue: &RenderQueue, camera: Vec3, sky: [Vec4; 2]) {
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
        let [zenith, horizon] = sky;
        let view = DistantView { coverage: IVec4::new(first.x, first.y, COVERAGE_SIZE, self.real_columns), zenith, horizon };
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
#[allow(clippy::too_many_arguments)]
fn receive(
    mut gpu: ResMut<TerrainGpu>,
    inbox: Res<TerrainInbox>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cameras: Query<&ExtractedView, (With<ExtractedCamera>, With<Camera3d>)>,
    breeze: Option<Res<Breeze>>,
    pipelines: Res<TerrainPipelines>,
    pipeline_cache: Res<PipelineCache>,
    water: Option<Res<WaterParams>>,
) {
    let layout = pipeline_cache.get_bind_group_layout(&pipelines.chunks_layout);
    gpu.ensure_arenas(&device, &queue);
    for message in inbox.0.try_iter() {
        match message {
            Message::Upload { chunk, pass, mesh } => gpu.upload(&device, &queue, &layout, chunk, pass, &mesh),
            Message::Remove { chunk, pass } => gpu.release(chunk, pass),
            Message::Blades { chunk, seeds } => gpu.upload_blades(&device, &queue, chunk, &seeds),
            Message::ColumnColours { column, colours } => gpu.upload_column_colours(&device, &queue, column, &colours),
            Message::RemoveColumn(column) => gpu.remove_column_colours(column),
            Message::Visible(visible) => gpu.set_visible(visible),
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
        gpu.write_coverage(&device, &queue, position, water.map_or([Vec4::ZERO; 2], |w| w.sky()));
    }
}

fn prepare_bind_group(
    mut gpu: ResMut<TerrainGpu>,
    pipelines: Res<TerrainPipelines>,
    textures: Option<Res<TerrainTextures>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    atmosphere: Res<AtmosphereBuffer>,
    cloud_map: Option<Res<CloudMap>>,
) {
    if gpu.bind_group.is_some() && gpu.distant_bind_group.is_some() && !gpu.bind_group_dirty {
        return;
    }
    let Some(cloud_map) = cloud_map else { return };
    let (Some(origins), Some(view), Some(coverage)) = (&gpu.origins_buffer, &gpu.distant_view, &gpu.coverage) else { return };
    let distant = device.create_bind_group(
        "distant_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.distant_layout),
        &BindGroupEntries::with_indices((
            (0, origins.as_entire_binding()),
            (1, view.as_entire_binding()),
            (2, coverage.as_entire_binding()),
            (6, atmosphere.0.as_entire_binding()),
            (8, &cloud_map.view),
            (9, &cloud_map.sampler),
            (10, &cloud_map.shade),
            (11, &cloud_map.noise.plane),
            (12, &cloud_map.noise.volume),
            (13, &cloud_map.noise.sampler),
        )),
    );
    let textures = textures.as_ref().and_then(|t| Some((images.get(&t.textures)?, images.get(&t.layers)?)));
    let (Some((blocks, layers)), Some(seeds), Some(wind), Some(colours)) =
        (textures, &gpu.seeds.buffer, &gpu.wind, &gpu.colours.buffer)
    else {
        gpu.distant_bind_group = Some(distant);
        return;
    };
    let bind_group = device.create_bind_group(
        "terrain_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::sequential((
            &blocks.texture_view,
            &blocks.sampler,
            origins.as_entire_binding(),
            seeds.as_entire_binding(),
            wind.as_entire_binding(),
            colours.as_entire_binding(),
            atmosphere.0.as_entire_binding(),
            &layers.texture_view,
            &cloud_map.view,
            &cloud_map.sampler,
            &cloud_map.shade,
            &cloud_map.noise.plane,
            &cloud_map.noise.volume,
            &cloud_map.noise.sampler,
        )),
    );
    gpu.distant_bind_group = Some(distant);
    gpu.bind_group = Some(bind_group);
    gpu.bind_group_dirty = false;
}

/// One draw of chunk meshes from one stream's page: a range of the view's gathered
/// indices, usually many chunks long.
struct ChunkDraw {
    stream: u8,
    page: u32,
    indices: Range<u32>,
}

/// One distant terrain tile.
struct TileDraw {
    page: u32,
    indices: Range<u32>,
    base_vertex: i32,
    slot: u32,
}

/// One chunk's grass blades: a tuft of `blades` per seed.
struct BladeDraw {
    seeds: Range<u32>,
    blades: u32,
    slot: u32,
}

/// The draws of each pass that survive a view's frustum, in drawing order.
#[derive(Component, Default)]
pub(super) struct TerrainDraws {
    passes: [Vec<ChunkDraw>; 5],
    /// Where the passes' indices are gathered from the pages.
    compaction: Compaction,
    /// What the water pass draws the translucent terrain with, when there is any; cameras
    /// only.
    water: Option<WaterDraw>,
    /// Distant terrain tiles, cameras only.
    distant: Vec<TileDraw>,
    /// Grass blades near the camera, nearest first; cameras only.
    blades: Vec<BladeDraw>,
    /// Each stream's surviving ranges with their page, kept between frames so culling
    /// reuses their memory.
    ranges: [Vec<(u32, Range<u32>)>; STREAMS],
    /// Room to group a stream's ranges by page.
    grouped: Vec<(u32, Range<u32>)>,
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
type CulledView = (Entity, &'static ExtractedView, Option<&'static LightEntity>, Option<&'static mut TerrainDraws>);
// The world's camera, not the one that shows it scaled on the window (`render::scale`).
type CulledViewFilter = Or<(With<LightEntity>, (With<ExtractedCamera>, With<Camera3d>))>;

/// Culls the visible chunks against each view's frustum, in parallel over views, and
/// skips opaque faces turned away from the view. What survives is gathered into the
/// view's own index buffer (see `compact`), one draw per stream and page; translucent
/// chunks are gathered back to front.
fn cull(
    mut commands: Commands,
    gpu: Res<TerrainGpu>,
    lights: Query<&ExtractedDirectionalLight>,
    mut views: Query<CulledView, CulledViewFilter>,
) {
    for (entity, _, _, draws) in &views {
        if draws.is_none() {
            commands.entity(entity).insert(TerrainDraws::default());
        }
    }
    // The camera the sun's cascades serve.
    let viewer = views.iter().find(|(_, _, light, _)| light.is_none()).map(|(_, view, _, _)| Viewer::new(view));
    views.par_iter_mut().for_each(|(_, view, light_entity, draws)| {
        let Some(mut draws) = draws else { return };
        let clip_from_world = view
            .clip_from_world
            .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let is_light = light_entity.is_some();
        let frustum = Frustum::new(clip_from_world, is_light);
        let eye = view.world_from_view.translation();
        let light = is_light.then(|| view.world_from_view.forward().as_vec3());
        let casters = light.zip(light_entity.and_then(|light| slice(light, &lights))).zip(viewer).map(|((towards, slice), viewer)| Casters::new(towards, slice, viewer));
        let farthest = light_entity.is_some_and(|light| last_of_many(light, &lights));
        let passes: &[TerrainPass] = if is_light { &SHADOW_PASSES } else { &PASSES };
        // Each stream's surviving ranges, and the translucent ones with their distance.
        let mut ranges = std::mem::take(&mut draws.ranges);
        let mut translucent = Vec::new();
        let mut blades = Vec::new();
        for (record, &(min, drawn)) in gpu.records.iter().zip(&gpu.bounds) {
            let max = min + CHUNK_SIZE as f32;
            if !drawn || !frustum.intersects(min, max) || casters.as_ref().is_some_and(|c| !c.matter(min, max)) {
                continue;
            }
            // Distance from the camera (not a shadow view's light) to the chunk's bounds.
            let distance = (gpu.camera.clamp(min, max) - gpu.camera).length();
            if let Some(seeds) = &record.blades
                && !is_light
                && blades_at(distance) > 0
            {
                blades.push((distance, BladeDraw { seeds: seeds.clone(), blades: blades_at(distance), slot: record.slot }));
            }
            let facing = facing(min, max, eye, light);
            for &pass in passes {
                if pass == TerrainPass::Fringe && distance >= FRINGE_DISTANCE {
                    continue;
                }
                for stream in streams(pass) {
                    let Some(allocation) = &record.streams[stream] else { continue };
                    if (pass == TerrainPass::Opaque && !facing[stream]) || (stream == PLANTS && (distance >= PLANT_DISTANCE || farthest)) {
                        continue;
                    }
                    let range = (allocation.page, allocation.vertices.clone());
                    if pass == TerrainPass::Translucent {
                        let centre = min + CHUNK_SIZE as f32 / 2.0;
                        translucent.push((centre.distance_squared(eye), range));
                    } else {
                        ranges[stream].push(range);
                    }
                }
            }
        }
        let draws = &mut *draws;
        draws.compaction.clear();
        for pass in &mut draws.passes {
            pass.clear();
        }
        // Each stream's ranges are gathered page by page, in page order.
        let mut grouped = std::mem::take(&mut draws.grouped);
        for pass in PASSES {
            for stream in streams(pass) {
                let ranges = &mut ranges[stream];
                group_by_page(ranges, &mut grouped);
                for run in grouped.chunk_by(|a, b| a.0 == b.0) {
                    let page = run[0].0;
                    let quads = run.iter().map(|(_, vertices)| vertices.clone());
                    let indices = draws.compaction.gather(stream, page, quads, indices_per_quad(stream));
                    let pass = if farthest && stream == CUTOUT { TerrainPass::Opaque } else { pass };
                    draws.passes[pass as usize].push(ChunkDraw { stream: stream as u8, page, indices });
                }
                ranges.clear();
            }
        }
        draws.ranges = ranges;
        draws.grouped = grouped;
        // Translucent chunks keep their back-to-front order across pages.
        translucent.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        let stream = streams(TerrainPass::Translucent).start;
        for run in translucent.chunk_by(|a, b| a.1.0 == b.1.0) {
            let page = run[0].1.0;
            let quads = run.iter().map(|(_, (_, vertices))| vertices.clone());
            let indices = draws.compaction.gather(stream, page, quads, indices_per_quad(stream));
            draws.passes[TerrainPass::Translucent as usize].push(ChunkDraw { stream: stream as u8, page, indices });
        }
        draws.distant.clear();
        if !is_light {
            for (&key, tile) in &gpu.tiles {
                if frustum.intersects(tile.min, tile.max) && !(key.level == 0 && gpu.covered_by_real(key)) {
                    draws.distant.push(TileDraw {
                        page: tile.mesh.page,
                        indices: tile.mesh.indices.clone(),
                        base_vertex: tile.mesh.vertices.start as i32,
                        slot: tile.slot,
                    });
                }
            }
            draws.distant.sort_unstable_by_key(|draw| draw.page);
        }
        // Nearest first, so nearer blades hide the farther ones' fragments.
        blades.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        draws.blades.clear();
        draws.blades.extend(blades.into_iter().map(|(_, draw)| draw));
    });
}

/// How much higher than what it shades terrain may stand and still cast a shadow, in
/// blocks (see `Casters`).
const CASTER_RISE: f32 = 160.0;

/// The camera, as the sun's cascades see it: where it is, which way it looks, and the
/// four side planes of its view, normalised (pointing inwards).
#[derive(Clone, Copy)]
struct Viewer {
    eye: Vec3,
    forward: Vec3,
    sides: [Vec4; 4],
}

impl Viewer {
    fn new(view: &ExtractedView) -> Self {
        let clip_from_world = view.clip_from_world.unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let sides = Frustum::new(clip_from_world, false).planes;
        Self {
            eye: view.world_from_view.translation(),
            forward: view.world_from_view.forward().as_vec3(),
            sides: std::array::from_fn(|i| sides[i] / sides[i].truncate().length()),
        }
    }
}

/// Which chunks can shade what a sun cascade covers. A cascade's box is much larger than
/// the stretch of view it serves (it bounds the slice of the view frustum at any angle,
/// so it is as wide as the slice's diagonal), and reaches up the sun's rays without end.
/// A chunk only matters if a shadow it casts can land within the cascade's range of view
/// depth, and within the view itself: its shadow falls along the sun's direction, and no
/// further than ground `CASTER_RISE` blocks above what it shades would throw it at the
/// sun's height. The view's sides take a fifth off the two farthest cascades' work, the
/// chunks their boxes take in off to either side of the view.
struct Casters {
    towards: Vec3,
    reach: f32,
    viewer: Viewer,
    /// The cascade's range of view depth, in blocks.
    depths: (f32, f32),
}

impl Casters {
    fn new(towards: Vec3, depths: (f32, f32), viewer: Viewer) -> Self {
        Self { towards, reach: (CASTER_RISE / (-towards.y).max(0.05)).min(4096.0), viewer, depths }
    }

    fn matter(&self, min: Vec3, max: Vec3) -> bool {
        let centre = (min + max) * 0.5;
        let radius = (max - min).length() * 0.5;
        // Where its shadow can fall: from the chunk itself down the sun's rays.
        let far = centre + self.towards * self.reach;
        let Viewer { eye, forward, sides } = &self.viewer;
        let (start, end) = ((centre - *eye).dot(*forward), (far - *eye).dot(*forward));
        let within_depths = start.max(end) + radius >= self.depths.0 && start.min(end) - radius <= self.depths.1;
        let in_view = |plane: &Vec4| plane.truncate().dot(centre).max(plane.truncate().dot(far)) + plane.w >= -radius;
        within_depths && sides.iter().all(in_view)
    }
}

/// Whether a view is the farthest of three or more sun cascades. Its texels are larger
/// than a leaf's holes, so it draws leaves and other cut-out blocks as solid casters
/// without the alpha test, which on Apple GPUs is what makes them expensive (no hidden
/// surface removal); the nearer cascades keep the dappled shade under trees. It leaves
/// plants out: their shadows there are specks no one can tell from none, and drawing them
/// took a third of the cascade's time in a savanna (0.27 ms at 2268×1473).
fn last_of_many(light: &LightEntity, lights: &Query<&ExtractedDirectionalLight>) -> bool {
    let LightEntity::Directional { light_entity, cascade_index } = *light else { return false };
    let Ok(light) = lights.get(light_entity) else { return false };
    let cascades = light.cascade_shadow_config.bounds.len();
    cascades >= 3 && cascade_index + 1 == cascades
}

/// The range of view depth a sun cascade serves, if the view is one.
fn slice(light: &LightEntity, lights: &Query<&ExtractedDirectionalLight>) -> Option<(f32, f32)> {
    let LightEntity::Directional { light_entity, cascade_index } = *light else { return None };
    let config = &lights.get(light_entity).ok()?.cascade_shadow_config;
    let far = *config.bounds.get(cascade_index)?;
    let near = match cascade_index {
        0 => config.minimum_distance,
        i => config.bounds[i - 1] * (1.0 - config.overlap_proportion),
    };
    Some((near, far))
}

/// Puts `ranges` into `grouped` ordered by page, each page's in the order they came, with a
/// counting sort: gathering needs them grouped by page and nothing more, and this is
/// linear in their number where sorting them was most of culling's time.
fn group_by_page(ranges: &[(u32, Range<u32>)], grouped: &mut Vec<(u32, Range<u32>)>) {
    grouped.clear();
    let Some(pages) = ranges.iter().map(|(page, _)| *page as usize + 1).max() else { return };
    let mut starts = vec![0usize; pages + 1];
    for (page, _) in ranges {
        starts[*page as usize + 1] += 1;
    }
    for page in 0..pages {
        starts[page + 1] += starts[page];
    }
    grouped.resize(ranges.len(), (0, 0..0));
    for (page, vertices) in ranges {
        let at = &mut starts[*page as usize];
        grouped[*at] = (*page, vertices.clone());
        *at += 1;
    }
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
    /// A chunk page's table of which chunk owns each block of vertices.
    chunks_layout: BindGroupLayoutDescriptor,
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
                storage_buffer_read_only_sized(false, None),
                // Grass blade seeds and the wind, for grass.wgsl and wind.wgsl.
                storage_buffer_read_only_sized(false, None),
                uniform_buffer::<WindUniform>(false),
                // Biome colours of columns, by block.
                storage_buffer_read_only_sized(false, None),
                // The air and light (lighting.wgsl), and each texture layer's overlay and glow.
                uniform_buffer::<Atmosphere>(false),
                texture_2d(TextureSampleType::Float { filterable: false }),
                // The cloud maps and noise (cloud_map.rs).
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let chunks_layout = BindGroupLayoutDescriptor::new(
        "terrain_chunks_layout",
        &BindGroupLayoutEntries::single(ShaderStages::VERTEX, storage_buffer_read_only_sized(false, None)),
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
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::VERTEX_FRAGMENT,
            (
                (0, storage_buffer_read_only_sized(false, None)),
                (1, uniform_buffer::<DistantView>(false)),
                (2, storage_buffer_read_only_sized(false, None)),
                // The air and light, at the binding lighting.wgsl expects, and the cloud maps
                // and noise.
                (6, uniform_buffer::<Atmosphere>(false)),
                (8, texture_2d(TextureSampleType::Float { filterable: true })),
                (9, sampler(SamplerBindingType::Filtering)),
                (10, texture_2d(TextureSampleType::Float { filterable: true })),
                (11, texture_2d(TextureSampleType::Float { filterable: true })),
                (12, texture_3d(TextureSampleType::Float { filterable: true })),
                (13, sampler(SamplerBindingType::Filtering)),
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
        chunks_layout,
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
    // Every such pipeline binds the cloud maps and noise (bindings 8 to 13).
    let defs = ["CLOUD_MAP", "NOISE_TEXTURES"].map(ShaderDefVal::from);
    descriptor.vertex.shader_defs.extend(defs.clone());
    if let Some(fragment) = descriptor.fragment.as_mut() {
        fragment.shader = shader.clone();
        fragment.shader_defs.extend(defs);
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
        descriptor.layout.push(self.chunks_layout.clone());
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
            layout: vec![
                prepass.view_layout_no_motion_vectors.clone(),
                prepass.empty_layout.clone(),
                self.layout.clone(),
                self.chunks_layout.clone(),
            ],
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
    mut views: Query<(&ExtractedView, Has<LightEntity>, Option<&mut TerrainDraws>), Or<(With<LightEntity>, With<Camera3d>)>>,
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
        let gpu = gpu.into_inner();
        let (Some(bind_group), Some(indices)) = (&gpu.bind_group, &gpu.blade_indices) else {
            return RenderCommandResult::Skip;
        };
        render_pass.set_bind_group(2, bind_group, &[]);
        render_pass.set_index_buffer(indices.slice(..), IndexFormat::Uint32);
        for draw in &draws.blades {
            let instance = draw.slot | draw.blades << 24;
            let (first, total) = (draw.seeds.start * draw.blades, draw.seeds.len() as u32 * draw.blades);
            for start in (0..total).step_by(PATTERN_BLADES as usize) {
                let count = (total - start).min(PATTERN_BLADES);
                let base = ((first + start) * BLADE_POINTS) as i32;
                render_pass.draw_indexed(0..count * BLADE_INDICES.len() as u32, base, instance..instance + 1);
            }
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
                let page = gpu.tile_pages.pages.get(draw.page as usize);
                let Some((page, (indices, _))) = page.and_then(|page| Some((page, page.indices.as_ref()?))) else {
                    return RenderCommandResult::Skip;
                };
                render_pass.set_vertex_buffer(0, page.vertex_buffer.slice(..));
                render_pass.set_index_buffer(indices.slice(..), IndexFormat::Uint32);
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
        let (true, Some(indices)) = (draws.compaction.ready, draws.compaction.indices()) else {
            return RenderCommandResult::Skip;
        };
        render_pass.set_bind_group(2, bind_group, &[]);
        render_pass.set_index_buffer(indices.slice(..), IndexFormat::Uint32);
        let mut current = None;
        for draw in &draws.passes[*pass as usize] {
            if current != Some((draw.stream, draw.page)) {
                let page = gpu.streams[draw.stream as usize].pages.get(draw.page as usize);
                let Some((page, (_, chunks))) = page.and_then(|page| Some((page, page.chunks.as_ref()?))) else {
                    return RenderCommandResult::Skip;
                };
                render_pass.set_vertex_buffer(0, page.vertex_buffer.slice(..));
                render_pass.set_bind_group(3, chunks, &[]);
                current = Some((draw.stream, draw.page));
            }
            render_pass.draw_indexed(draw.indices.clone(), 0, 0..1);
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
