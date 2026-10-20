//! Dropped items as Minecraft draws them: a tiny block for a block, and for a plant a flat
//! sprite given thickness, one texel deep with its edges walled in, turning slowly and
//! bobbing where it lies. The shader turns and bobs them from the time, so items lying
//! still cost no upload: the buffer is written only when what is shown changes.
//!
//! Every item shown this frame is one instance of one draw: 36 vertices of a box, a cube
//! for blocks and a slab a sixteenth thick for sprites. A sprite's slab is marched through
//! texel by texel in the fragment shader along the view ray, so its outline and the walls
//! of its edges come out of the texture's alpha, as Minecraft's extruded item models do,
//! with no mesh per item. Items are lit like the terrain around them (`voxmar::lighting`):
//! sun and its shadows, sky and block light, and the air between them and the eye.
//!
//! The caller fills [`ItemModels`] every frame (see `drops`).

use bevy::asset::embedded_asset;
use bevy::core_pipeline::core_3d::AlphaMask3d;
use bevy::core_pipeline::prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey};
use bevy::ecs::system::SystemParamItem;
use bevy::ecs::system::lifetimeless::SRes;
use bevy::mesh::{MeshVertexBufferLayout, MeshVertexBufferLayouts, VertexBufferLayout};
use bevy::pbr::{LightEntity, MeshPipeline, MeshPipelineKey, SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, ViewKeyCache};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem, RenderCommand, RenderCommandResult,
    SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor, BufferUsages,
    CachedRenderPipelineId, PipelineCache, SamplerBindingType, ShaderStages, TextureSampleType, VertexStepMode,
    binding_types::{sampler, storage_buffer_read_only_sized, texture_2d, texture_2d_array, uniform_buffer},
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bytemuck::{Pod, Zeroable};
use rustc_hash::FxHashMap;

use super::atmosphere::{Atmosphere, AtmosphereBuffer};
use super::cloud_map::{self, CloudMap};
use super::terrain::{TerrainTextures, mesh_like};

/// Items drawn at once at most; beyond it the furthest are left out by the caller.
pub const CAPACITY: usize = 2048;
const SHADER: &str = "embedded://voxmar/render/items.wgsl";
/// Vertices of one item's box.
const BOX_VERTICES: u32 = 36;
/// The flag of the item in hand (`FLAG_HELD` in items.wgsl).
const HELD: u32 = 1 << 10;
/// The flag of an item held still in the world (`FLAG_POSED` in items.wgsl).
const POSED: u32 = 1 << 11;

pub struct ItemPlugin;

impl Plugin for ItemPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "items.wgsl");
        app.init_resource::<ItemModels>();
        let key = MainEntity::from(app.world_mut().spawn_empty().id());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .insert_resource(ItemKey(key))
            .init_resource::<ItemInbox>()
            .add_render_command::<AlphaMask3d, DrawItemBoxes>()
            .add_systems(RenderStartup, init_gpu)
            .add_systems(ExtractSchedule, extract)
            .add_systems(
                Render,
                (
                    upload.in_set(RenderSystems::PrepareResources),
                    prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
                    queue.in_set(RenderSystems::Queue),
                ),
            );
    }
}

/// How an item is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A cube with the block's six faces.
    Cube,
    /// The texture of the first face as a flat sprite a texel thick.
    Sprite,
}

/// One item to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct ItemModel {
    /// Centre of the model, before it bobs.
    pub center: Vec3,
    /// Seconds into its turning and bobbing at time 0, so neighbours don't move in step.
    pub phase: f32,
    /// Where this copy of a stack sits from the centre, in the item's turning frame.
    pub offset: Vec3,
    /// Edge length in blocks.
    pub size: f32,
    pub shape: Shape,
    /// Texture layer per face, in `Face` order (+x, -x, +y, -y, +z, -z).
    pub layers: [u16; 6],
    /// Which faces (bit per face) take `tint`, on the texture or its overlay.
    pub tinted: u8,
    /// sRGB colour of tinted faces.
    pub tint: [u8; 3],
    /// Sky and block light where it is, 0..=15.
    pub light: [u8; 2],
    /// Set for the item in the player's hand: it is placed in the view, not the world.
    pub held: Option<Held>,
    /// Set for an item held still in the world, turned by this rotation vector (axis times
    /// angle) about its `center`: one in another player's hand. It neither turns nor bobs.
    pub posed: Option<Vec3>,
}

/// Where the item in the player's hand sits, in the camera's frame. It stands in for the
/// model's `center` (and `offset` and `phase`, which it does not use): the shader finds the
/// camera's axes for itself, so the hand moves with the very view the frame is drawn with
/// and trails nothing behind, and draws it in front of everything, as no wall comes
/// between a hand and the eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Held {
    /// Where its middle is, in blocks: right, up and forward of the eye.
    pub at: Vec3,
    /// How it is turned from facing the eye, as a rotation vector (axis times angle).
    pub turn: Vec3,
}

/// The items to draw this frame, which `drops` fills.
#[derive(Resource, Default)]
pub struct ItemModels(pub Vec<ItemModel>);

/// An item as the shader reads it (`Item` in items.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
struct GpuItem {
    /// Where it is, or for the item in hand, right, up and forward of the eye.
    center: [f32; 3],
    phase: f32,
    /// Where this copy of a stack sits, or for the item in hand, its rotation vector.
    offset: [f32; 3],
    size: f32,
    /// shape:4 | tinted faces:6 | held:1 | posed:1 | free:12 | block light:4 | sky light:4.
    flags: u32,
    /// Two faces' layers each, the lower face in the low half.
    layers: [u32; 3],
    /// sRGB tint, 8 bits each.
    tint: u32,
    /// To the 16-byte multiple WGSL rounds the struct to (its `vec3` aligns at 16).
    _pad: [u32; 3],
}

impl GpuItem {
    fn new(model: &ItemModel) -> Self {
        let [sky, block] = model.light.map(|l| l.min(15) as u32);
        let shape = match model.shape {
            Shape::Cube => 0,
            Shape::Sprite => 1,
        };
        let pair = |i: usize| model.layers[i] as u32 | (model.layers[i + 1] as u32) << 16;
        let [r, g, b] = model.tint;
        let (center, offset, held) = match (model.held, model.posed) {
            (Some(held), _) => (held.at, held.turn, HELD),
            (None, Some(turn)) => (model.center, turn, POSED),
            (None, None) => (model.center, model.offset, 0),
        };
        Self {
            center: center.to_array(),
            phase: model.phase,
            offset: offset.to_array(),
            size: model.size,
            flags: shape | ((model.tinted as u32) & 63) << 4 | held | block << 24 | sky << 28,
            layers: [pair(0), pair(2), pair(4)],
            tint: u32::from_le_bytes([r, g, b, 255]),
            _pad: [0; 3],
        }
    }
}

#[derive(Resource)]
struct ItemKey(MainEntity);

/// What extraction hands the render world: the items shown, and whether they changed
/// since they were last uploaded.
#[derive(Resource, Default)]
struct ItemInbox {
    items: Vec<GpuItem>,
    changed: bool,
}

fn extract(models: Extract<Res<ItemModels>>, mut inbox: ResMut<ItemInbox>, mut next: Local<Vec<GpuItem>>) {
    next.clear();
    next.extend(models.0.iter().take(CAPACITY).map(GpuItem::new));
    if *next != inbox.items {
        std::mem::swap(&mut *next, &mut inbox.items);
        inbox.changed = true;
    }
}

/// The items' buffer and the pipeline that draws them.
#[derive(Resource)]
struct ItemGpu {
    buffer: Buffer,
    /// Items in the buffer this frame.
    count: u32,
    layout: BindGroupLayoutDescriptor,
    bind_group: Option<BindGroup>,
    shader: Handle<Shader>,
    vertex_layout: MeshVertexBufferLayout,
    pipelines: FxHashMap<MeshPipelineKey, CachedRenderPipelineId>,
    /// Render entity standing for the items in the phases.
    item: Entity,
}

fn init_gpu(mut commands: Commands, device: Res<RenderDevice>, asset_server: Res<AssetServer>) {
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("items"),
        size: (CAPACITY * size_of::<GpuItem>()) as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut layout = BindGroupLayoutDescriptor::new(
        "item_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::VERTEX_FRAGMENT,
            (
                (0, texture_2d_array(TextureSampleType::Float { filterable: true })),
                (1, sampler(SamplerBindingType::Filtering)),
                (2, storage_buffer_read_only_sized(false, None)),
                // The air and light, at the binding lighting.wgsl expects.
                (6, uniform_buffer::<Atmosphere>(false)),
                // Each texture layer's overlay, at the terrain's binding.
                (7, texture_2d(TextureSampleType::Float { filterable: false })),
            ),
        ),
    );
    // Built as the terrain's pipeline (`mesh_like`), with its shader defs, so lighting.wgsl
    // finds the cloud maps and noise it reads (bindings 8 to 13).
    layout.entries.extend(cloud_map::layout_entries(ShaderStages::VERTEX_FRAGMENT));
    // No vertex buffer: the vertex and instance indices say everything.
    let vertex_layout = MeshVertexBufferLayout::new(
        Vec::new(),
        VertexBufferLayout { array_stride: 0, step_mode: VertexStepMode::Vertex, attributes: Vec::new() },
    );
    let item = commands.spawn_empty().id();
    commands.insert_resource(ItemGpu {
        buffer,
        count: 0,
        layout,
        bind_group: None,
        shader: asset_server.load(SHADER),
        vertex_layout,
        pipelines: FxHashMap::default(),
        item,
    });
}

fn upload(mut gpu: ResMut<ItemGpu>, mut inbox: ResMut<ItemInbox>, queue: Res<RenderQueue>) {
    if !std::mem::take(&mut inbox.changed) {
        return;
    }
    gpu.count = inbox.items.len() as u32;
    if !inbox.items.is_empty() {
        queue.write_buffer(&gpu.buffer, 0, bytemuck::cast_slice(&inbox.items));
    }
}

fn prepare_bind_group(
    mut gpu: ResMut<ItemGpu>,
    textures: Option<Res<TerrainTextures>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    atmosphere: Res<AtmosphereBuffer>,
    cloud_map: Option<Res<CloudMap>>,
) {
    if gpu.bind_group.is_some() {
        return;
    }
    let Some(cloud_map) = cloud_map else { return };
    let Some((blocks, layers)) = textures.as_ref().and_then(|t| Some((images.get(&t.textures)?, images.get(&t.layers)?))) else {
        return;
    };
    let bind_group = device.create_bind_group(
        "item_bind_group",
        &pipeline_cache.get_bind_group_layout(&gpu.layout),
        &[
            &BindGroupEntries::with_indices((
                (0, &blocks.texture_view),
                (1, &blocks.sampler),
                (2, gpu.buffer.as_entire_binding()),
                (6, atmosphere.0.as_entire_binding()),
                (7, &layers.texture_view),
            ))[..],
            &cloud_map.entries(),
        ]
        .concat(),
    );
    gpu.bind_group = Some(bind_group);
}

impl ItemGpu {
    /// The pipeline for a view: the alpha-tested mesh pipeline with our shader.
    fn pipeline(
        &mut self,
        view_key: MeshPipelineKey,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        cache: &PipelineCache,
    ) -> Option<CachedRenderPipelineId> {
        if let Some(&id) = self.pipelines.get(&view_key) {
            return Some(id);
        }
        let key = view_key | MeshPipelineKey::MAY_DISCARD;
        let mut descriptor = mesh_like(mesh_pipeline, layouts, key, &self.vertex_layout, &self.layout, &self.shader, "items".into())?;
        descriptor.vertex.buffers.clear();
        // The shader drops faces turned away itself, whatever their winding.
        descriptor.primitive.cull_mode = None;
        let id = cache.queue_render_pipeline(descriptor);
        self.pipelines.insert(view_key, id);
        Some(id)
    }
}

/// Puts the items into every camera's alpha-tested phase while there are any.
#[allow(clippy::too_many_arguments)]
fn queue(
    mut gpu: ResMut<ItemGpu>,
    key: Res<ItemKey>,
    mesh_pipeline: Res<MeshPipeline>,
    mut layouts: ResMut<MeshVertexBufferLayouts>,
    cache: Res<PipelineCache>,
    view_keys: Res<ViewKeyCache>,
    mut phases: ResMut<ViewBinnedRenderPhases<AlphaMask3d>>,
    draw_functions: Res<DrawFunctions<AlphaMask3d>>,
    views: Query<&ExtractedView, (Without<LightEntity>, With<Camera3d>)>,
) {
    let draw_function = draw_functions.read().get_id::<DrawItemBoxes>().expect("item draw is registered");
    let drawing = gpu.count > 0 && gpu.bind_group.is_some();
    for view in &views {
        let retained = &view.retained_view_entity;
        let (Some(phase), Some(&view_key)) = (phases.get_mut(retained), view_keys.get(retained)) else { continue };
        phase.remove(key.0);
        // Asked for while nothing is drawn too, so it is compiled as the world loads: on
        // Metal, Bevy compiles on the render thread, which held the frame of the first
        // block broken.
        let Some(pipeline) = gpu.pipeline(view_key, &mesh_pipeline, &mut layouts, &cache) else { continue };
        if !drawing {
            continue;
        }
        let batch = OpaqueNoLightmap3dBatchSetKey { pipeline, draw_function, material_bind_group_index: None, slabs: default() };
        let bin = OpaqueNoLightmap3dBinKey { asset_id: AssetId::<Mesh>::default().untyped() };
        phase.add(batch, bin, (gpu.item, key.0), InputUniformIndex::default(), BinnedRenderPhaseType::NonMesh);
    }
}

type DrawItemBoxes = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawItems);

/// Draws one box per item.
struct DrawItems;

impl<P: PhaseItem> RenderCommand<P> for DrawItems {
    type Param = SRes<ItemGpu>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: (),
        _: Option<()>,
        gpu: SystemParamItem<'w, '_, Self::Param>,
        render_pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let gpu = gpu.into_inner();
        let Some(bind_group) = &gpu.bind_group else { return RenderCommandResult::Skip };
        render_pass.set_bind_group(2, bind_group, &[]);
        render_pass.draw(0..BOX_VERTICES, 0..gpu.count);
        RenderCommandResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_pack_like_the_shader_reads_them() {
        assert_eq!(size_of::<GpuItem>(), 64);
        let model = ItemModel {
            center: Vec3::new(1.0, 2.0, 3.0),
            phase: 0.5,
            offset: Vec3::X,
            size: 0.25,
            shape: Shape::Sprite,
            layers: [1, 2, 3, 4, 5, 600],
            tinted: 0b000100,
            tint: [10, 20, 30],
            light: [14, 3],
            held: None,
            posed: None,
        };
        let gpu = GpuItem::new(&model);
        assert_eq!(gpu.flags & 15, 1);
        assert_eq!((gpu.flags >> 4) & 63, 0b000100);
        assert_eq!((gpu.flags >> 24 & 15, gpu.flags >> 28), (3, 14));
        assert_eq!(gpu.layers, [1 | 2 << 16, 3 | 4 << 16, 5 | 600 << 16]);
        assert_eq!(gpu.tint.to_le_bytes(), [10, 20, 30, 255]);
        assert_eq!(gpu.flags & HELD, 0);
        // The item in hand: its place in the view and its turn take the world place's fields.
        let held = Held { at: Vec3::new(0.3, -0.2, 0.6), turn: Vec3::new(0.0, 0.5, 0.25) };
        let gpu = GpuItem::new(&ItemModel { held: Some(held), ..model });
        assert_eq!((gpu.center, gpu.offset), ([0.3, -0.2, 0.6], [0.0, 0.5, 0.25]));
        assert_eq!(gpu.flags & HELD, HELD);
        // And its flag leaves the rest of the word as it was.
        assert_eq!((gpu.flags >> 4) & 63, 0b000100);
        assert_eq!((gpu.flags >> 24 & 15, gpu.flags >> 28), (3, 14));
        // An item held still in the world (another player's): its place, and its turn in
        // the offset's field.
        let turn = Vec3::new(0.1, 0.2, 0.3);
        let gpu = GpuItem::new(&ItemModel { posed: Some(turn), ..model });
        assert_eq!((gpu.center, gpu.offset), ([1.0, 2.0, 3.0], [0.1, 0.2, 0.3]));
        assert_eq!(gpu.flags & (HELD | POSED), POSED);
        assert_eq!((gpu.flags >> 4) & 63, 0b000100);
    }
}
