//! Terrain rendering without an entity per chunk.
//!
//! Chunk meshes live in a few large GPU pages (one vertex and one index buffer each) and a
//! table of chunk origins. Every frame each view culls the chunks against its frustum and
//! draws the survivors straight from the pages: one phase item per pass and view stands
//! in for thousands of mesh entities, so no ECS work, extraction, specialisation or
//! batching scales with the number of chunks. The main world talks to this module only
//! through [`Terrain`], a channel of uploads, removals and the set of chunks that cave
//! culling leaves visible.

use std::ops::Range;

use bevy::core_pipeline::core_3d::{
    AlphaMask3d, CORE_3D_DEPTH_FORMAT, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey, Transparent3d,
    TransparentSortingInfo3d,
};
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
    AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem, PhaseItemExtraIndex,
    RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
    ViewSortedRenderPhases,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, CompareFunction, DepthBiasState, DepthStencilState, Face, FragmentState,
    IndexFormat, MultisampleState, PipelineCache, PrimitiveState, RenderPipelineDescriptor, SamplerBindingType,
    ShaderStages, SpecializedMeshPipeline, StencilState, TextureSampleType, VertexState,
    binding_types::{sampler, storage_buffer_read_only_sized, texture_2d_array},
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use voxmar_engine::coords::{CHUNK_SIZE, ChunkPos};
use voxmar_engine::mesh::{MeshBuffers, Vertex};

use super::{ATTRIBUTE_VOXEL, TerrainPass, VOXEL_SHADER};

/// Vertices and indices per page: 32 MB and 24 MB, room for about a million quads.
const PAGE_VERTICES: u32 = 4 << 20;
const PAGE_INDICES: u32 = 6 << 20;
const PASSES: [TerrainPass; 4] = [TerrainPass::Opaque, TerrainPass::Cutout, TerrainPass::Inner, TerrainPass::Translucent];
/// Only these passes cast shadows: leaf crowns' shells already shade, and water would
/// shadow itself.
const SHADOW_PASSES: [TerrainPass; 2] = [TerrainPass::Opaque, TerrainPass::Cutout];

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = unbounded();
        app.insert_resource(Terrain { tx }).add_plugins(ExtractResourcePlugin::<TerrainTextures>::default());
        // Phase items are keyed by main world entity; these stand for the four passes.
        let keys = TerrainKeys(PASSES.map(|_| MainEntity::from(app.world_mut().spawn_empty().id())));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .insert_resource(TerrainInbox(rx))
            .insert_resource(keys)
            .init_resource::<TerrainGpu>()
            .add_render_command::<Opaque3d, DrawTerrainMain>()
            .add_render_command::<AlphaMask3d, DrawTerrainMain>()
            .add_render_command::<Transparent3d, DrawTerrainMain>()
            .add_render_command::<Shadow, DrawTerrainShadow>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                (
                    receive.in_set(RenderSystems::PrepareResources),
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
    /// The chunks cave culling leaves visible; any other chunk is skipped.
    Visible(FxHashSet<ChunkPos>),
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

    pub fn set_visible(&self, visible: FxHashSet<ChunkPos>) {
        let _ = self.tx.send(Message::Visible(visible));
    }

    /// Forgets every chunk, for leaving a world.
    pub fn clear(&self) {
        let _ = self.tx.send(Message::Clear);
    }
}

#[derive(Resource)]
struct TerrainInbox(Receiver<Message>);

#[derive(Resource)]
struct TerrainKeys([MainEntity; 4]);

/// Ranges of a page handed out first-fit; freed ranges merge with their neighbours.
struct RangeAllocator {
    /// Free ranges, sorted and never adjacent.
    free: Vec<Range<u32>>,
}

impl RangeAllocator {
    fn new(size: u32) -> Self {
        Self { free: std::iter::once(0..size).collect() }
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

/// Where one pass of a chunk's mesh lives.
struct Allocation {
    page: u32,
    vertices: Range<u32>,
    indices: Range<u32>,
}

struct ChunkRecord {
    /// Index into the origin table, which the shader reads as the instance index.
    slot: u32,
    passes: [Option<Allocation>; 4],
}

/// Render world state: pages, chunk records and the bind group shared by every draw.
#[derive(Resource, Default)]
struct TerrainGpu {
    pages: Vec<Page>,
    chunks: FxHashMap<ChunkPos, ChunkRecord>,
    visible: FxHashSet<ChunkPos>,
    free_slots: Vec<u32>,
    /// Chunk origins by slot, as the shader reads them.
    origins: Vec<IVec4>,
    origins_buffer: Option<Buffer>,
    origins_dirty: bool,
    bind_group: Option<BindGroup>,
    /// Whether the bind group must be rebuilt (the origin buffer was replaced).
    bind_group_dirty: bool,
}

impl TerrainGpu {
    fn release(&mut self, chunk: ChunkPos, pass: TerrainPass) {
        let Some(record) = self.chunks.get_mut(&chunk) else { return };
        if let Some(allocation) = record.passes[pass as usize].take() {
            let page = &mut self.pages[allocation.page as usize];
            page.vertices.release(allocation.vertices);
            page.indices.release(allocation.indices);
        }
        if record.passes.iter().all(Option::is_none) {
            self.free_slots.push(record.slot);
            self.chunks.remove(&chunk);
        }
    }

    fn upload(&mut self, device: &RenderDevice, queue: &RenderQueue, chunk: ChunkPos, pass: TerrainPass, mesh: &MeshBuffers) {
        self.release(chunk, pass);
        let (vertex_count, index_count) = (mesh.vertices.len() as u32, mesh.indices.len() as u32);
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
        let (page, vertices, indices) = found.unwrap_or_else(|| {
            let mut page = Page::new(device, self.pages.len());
            let vertices = page.vertices.allocate(vertex_count).expect("a chunk mesh fits a page");
            let indices = page.indices.allocate(index_count).expect("a chunk mesh fits a page");
            self.pages.push(page);
            (self.pages.len() - 1, vertices, indices)
        });
        let target = &self.pages[page];
        let vertex_bytes = size_of::<Vertex>() as u64;
        queue.write_buffer(&target.vertex_buffer, vertices.start as u64 * vertex_bytes, bytemuck::cast_slice(&mesh.vertices));
        queue.write_buffer(&target.index_buffer, indices.start as u64 * 4, bytemuck::cast_slice(&mesh.indices));

        let slot = match self.chunks.get(&chunk) {
            Some(record) => record.slot,
            None => {
                let slot = self.free_slots.pop().unwrap_or(self.origins.len() as u32);
                if slot as usize == self.origins.len() {
                    self.origins.push(IVec4::ZERO);
                }
                self.origins[slot as usize] = chunk.origin().extend(0);
                self.origins_dirty = true;
                self.chunks.insert(chunk, ChunkRecord { slot, passes: Default::default() });
                slot
            }
        };
        let record = self.chunks.get_mut(&chunk).expect("record was just ensured");
        debug_assert_eq!(record.slot, slot);
        record.passes[pass as usize] = Some(Allocation { page: page as u32, vertices, indices });
    }

    fn clear(&mut self) {
        let origins_buffer = self.origins_buffer.take();
        *self = Self { origins_buffer, bind_group_dirty: true, ..Self::default() };
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
}

impl Page {
    fn new(device: &RenderDevice, index: usize) -> Self {
        let buffer = |label: &str, size: u64, usage: BufferUsages| {
            device.create_buffer(&BufferDescriptor {
                label: Some(&format!("terrain_{label}_{index}")),
                size,
                usage: usage | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            vertex_buffer: buffer("vertices", PAGE_VERTICES as u64 * size_of::<Vertex>() as u64, BufferUsages::VERTEX),
            index_buffer: buffer("indices", PAGE_INDICES as u64 * 4, BufferUsages::INDEX),
            vertices: RangeAllocator::new(PAGE_VERTICES),
            indices: RangeAllocator::new(PAGE_INDICES),
        }
    }
}

/// Applies the main world's messages: writes and frees mesh data, updates the origin table.
fn receive(mut gpu: ResMut<TerrainGpu>, inbox: Res<TerrainInbox>, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    for message in inbox.0.try_iter() {
        match message {
            Message::Upload { chunk, pass, mesh } => gpu.upload(&device, &queue, chunk, pass, &mesh),
            Message::Remove { chunk, pass } => gpu.release(chunk, pass),
            Message::Visible(visible) => gpu.visible = visible,
            Message::Clear => gpu.clear(),
        }
    }
    gpu.write_origins(&device, &queue);
}

fn prepare_bind_group(
    mut gpu: ResMut<TerrainGpu>,
    pipelines: Res<TerrainPipelines>,
    textures: Option<Res<TerrainTextures>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
) {
    if gpu.bind_group.is_some() && !gpu.bind_group_dirty {
        return;
    }
    let (Some(textures), Some(origins)) = (textures, gpu.origins_buffer.as_ref()) else { return };
    let (Some(blocks), Some(colormaps)) = (images.get(&textures.textures), images.get(&textures.colormaps)) else { return };
    let bind_group = device.create_bind_group(
        "terrain_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::sequential((
            &blocks.texture_view,
            &blocks.sampler,
            &colormaps.texture_view,
            &colormaps.sampler,
            origins.as_entire_binding(),
        )),
    );
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

/// The draws of each pass that survive a view's frustum, in drawing order.
#[derive(Component, Default)]
struct TerrainDraws {
    passes: [Vec<Draw>; 4],
}

/// Culls the visible chunks against each view's frustum, in parallel over views.
/// Translucent chunks are sorted back to front; the rest keep page order, so pages rarely
/// switch.
fn cull(
    mut commands: Commands,
    gpu: Res<TerrainGpu>,
    mut views: Query<(Entity, &ExtractedView, Has<LightEntity>, Option<&mut TerrainDraws>)>,
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
        for &(min, record) in &candidates {
            if !frustum.intersects(min, min + CHUNK_SIZE as f32) {
                continue;
            }
            for &pass in passes {
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
        for pass in &mut draws.passes {
            pass.sort_unstable_by_key(|draw| draw.page);
        }
        translucent.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        draws.passes[TerrainPass::Translucent as usize].extend(translucent.into_iter().map(|(_, draw)| draw));
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
struct TerrainPassItem(TerrainPass);

#[derive(Resource)]
struct TerrainPipelines {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    vertex_layout: MeshVertexBufferLayout,
    /// One render entity per pass.
    items: [Entity; 4],
    main: FxHashMap<(MeshPipelineKey, TerrainPass), CachedRenderPipelineId>,
    shadow: FxHashMap<TerrainPass, CachedRenderPipelineId>,
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
    commands.insert_resource(TerrainPipelines {
        layout,
        shader: asset_server.load(VOXEL_SHADER),
        vertex_layout,
        items,
        main: FxHashMap::default(),
        shadow: FxHashMap::default(),
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
    }
}

impl TerrainPipelines {
    /// The main pass pipeline: the mesh pipeline's view setup and shader defs, with the
    /// terrain's shader, vertex layout and bind group.
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
            TerrainPass::Cutout | TerrainPass::Inner => MeshPipelineKey::MAY_DISCARD,
            TerrainPass::Translucent => MeshPipelineKey::BLEND_ALPHA,
        };
        let key = view_key | alpha | MeshPipelineKey::from_primitive_topology_and_strip_index(PrimitiveTopology::TriangleList, None);
        let vertex_layout = layouts.insert(self.vertex_layout.clone());
        let mut descriptor = match mesh_pipeline.specialize(key, &vertex_layout) {
            Ok(descriptor) => descriptor,
            Err(e) => {
                error!("terrain pipeline: {e}");
                return None;
            }
        };
        let (defs, cull_mode) = pass_setup(pass);
        descriptor.label = Some(format!("terrain_{pass:?}").into());
        descriptor.layout.truncate(2);
        descriptor.layout.push(self.layout.clone());
        descriptor.vertex.shader = self.shader.clone();
        descriptor.vertex.buffers = vec![self.vertex_layout.layout().clone()];
        descriptor.primitive.cull_mode = cull_mode;
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
            fragment.shader_defs.extend(defs.iter().map(|&def| ShaderDefVal::from(def)));
        }
        let id = cache.queue_render_pipeline(descriptor);
        self.main.insert((view_key, pass), id);
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
    transparent: ResMut<'w, ViewSortedRenderPhases<Transparent3d>>,
    shadow: ResMut<'w, ViewBinnedRenderPhases<Shadow>>,
    opaque_draw: Res<'w, DrawFunctions<Opaque3d>>,
    mask_draw: Res<'w, DrawFunctions<AlphaMask3d>>,
    transparent_draw: Res<'w, DrawFunctions<Transparent3d>>,
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
    views: Query<(&ExtractedView, Has<LightEntity>)>,
) {
    let items = PASSES.map(|pass| (pipelines.items[pass as usize], keys.0[pass as usize]));
    let registered = "terrain draw is registered";
    let opaque_draw = phases.opaque_draw.read().get_id::<DrawTerrainMain>().expect(registered);
    let mask_draw = phases.mask_draw.read().get_id::<DrawTerrainMain>().expect(registered);
    let transparent_draw = phases.transparent_draw.read().get_id::<DrawTerrainMain>().expect(registered);
    let shadow_draw = phases.shadow_draw.read().get_id::<DrawTerrainShadow>().expect(registered);
    let bin = || Opaque3dBinKey { asset_id: AssetId::<Mesh>::default().untyped() };

    for (view, is_light) in &views {
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
        let mut pipeline = |pass| pipelines.main(view_key, pass, &mesh_pipeline, &mut layouts, &cache);
        if let (Some(phase), Some(id)) = (phases.opaque.get_mut(retained), pipeline(TerrainPass::Opaque)) {
            let item = items[TerrainPass::Opaque as usize];
            phase.remove(item.1);
            let key = Opaque3dBatchSetKey {
                pipeline: id,
                draw_function: opaque_draw,
                material_bind_group_index: None,
                slabs: default(),
                lightmap_slab: None,
            };
            phase.add(key, bin(), item, InputUniformIndex::default(), BinnedRenderPhaseType::NonMesh);
        }
        if let Some(phase) = phases.mask.get_mut(retained) {
            for pass in [TerrainPass::Cutout, TerrainPass::Inner] {
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
        if let (Some(phase), Some(id)) = (phases.transparent.get_mut(retained), pipeline(TerrainPass::Translucent)) {
            // Water and ice sort among themselves; as a whole they go before other
            // see-through things (precipitation, which always falls in front).
            phase.add_transient(Transparent3d {
                sorting_info: TransparentSortingInfo3d::Sorted { mesh_center: view.world_from_view.translation(), depth_bias: -1.0e8 },
                distance: -1.0e8,
                pipeline: id,
                entity: items[TerrainPass::Translucent as usize],
                draw_function: transparent_draw,
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                indexed: true,
            });
        }
    }
}

type DrawTerrainMain = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawTerrain);
type DrawTerrainShadow = (SetItemPipeline, SetPrepassViewBindGroup<0>, SetPrepassViewEmptyBindGroup<1>, DrawTerrain);

/// Draws a view's culled chunks of the item's pass.
struct DrawTerrain;

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
                let page = &gpu.pages[draw.page as usize];
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
