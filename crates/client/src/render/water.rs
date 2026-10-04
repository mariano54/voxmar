//! Water, drawn in a pass of its own so it can see what lies behind and around it.
//!
//! Water and ice are the terrain's translucent faces (`TerrainPass::Translucent`). They are
//! drawn after the opaque terrain and before Bevy's transparent pass, with the scene so far
//! bound for reading (`water_surface.wgsl`): its depth, through a read-only depth
//! attachment, so nothing is copied, and its colour, the main image so far, while the pass
//! writes the other main image (Bevy's post-processing ping-pong), resolving into it under
//! MSAA and drawing over a copy of the scene otherwise. From those the water shader finds
//!
//! - how far light travels through the water to whatever is behind it, so shallows are
//!   clear and deep water takes its colour, with no bands at whole blocks;
//! - the bed seen through the surface, bent by the waves;
//! - reflections of the terrain by marching the depth buffer, and of the sky elsewhere;
//! - foam where the surface meets the shore.
//!
//! Waves are a tiling texture of swells laid over the surface in a few layers that drift
//! with the wind (see `waves`); their positions are worked out here each frame, in double
//! precision around the camera, so far from the origin they stay smooth.

use bevy::core_pipeline::core_3d::{main_opaque_pass_3d, main_transparent_pass_3d};
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::math::DVec2;
use bevy::pbr::{SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, main_transmissive_pass_3d};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_phase::{
    AddRenderCommand, CachedRenderPipelinePhaseItem, DrawFunctionId, DrawFunctions, PhaseItem, PhaseItemExtraIndex,
    SetItemPipeline,
};
use bevy::render::render_resource::{
    AddressMode, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState, LoadOp, MipmapFilterMode,
    Operations, PipelineCache, RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, StoreOp,
    TextureDataOrder, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor, encase,
    binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::{ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::asset::embedded_asset;
use bevy::core_pipeline::FullscreenShader;
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::shader::load_shader_library;
use serde::{Deserialize, Serialize};

use super::pack::ActivePack;
use super::terrain::{DrawTerrain, TerrainDraws};
use super::{DayCycle, SkyHandle, SkyMaterial};
use crate::weather::Wind;

mod waves;

/// Wave layers the shader can sum (`water_surface.wgsl`).
const LAYERS: usize = waves::LAYERS.len();
/// Wind speed (blocks per second) at which waves reach their full height, and how high they
/// are in calm air, as a fraction of that.
const WINDY: f32 = 8.0;
const CALM_WAVES: f32 = 0.2;

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "water.wgsl");
        load_shader_library!(app, "water_surface.wgsl");
        embedded_asset!(app, "water_depth.wgsl");
        app.init_resource::<WaterQuality>()
            .init_resource::<WaterParams>()
            .add_plugins(ExtractResourcePlugin::<WaterParams>::default())
            // After the day cycle has set the sky's colours for this frame.
            .add_systems(PostUpdate, update_params.after(TransformSystems::Propagate));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<DrawFunctions<WaterItem>>()
            .add_render_command::<WaterItem, DrawWater>()
            .add_systems(RenderStartup, init_gpu)
            .add_systems(
                Render,
                (write_uniform, prepare_depth_textures).in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Core3d,
                water_pass
                    .after(main_opaque_pass_3d)
                    .after(main_transmissive_pass_3d)
                    .before(main_transparent_pass_3d)
                    .in_set(Core3dSystems::MainPass),
            );
    }
}

/// How much work water does, a setting.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaterQuality {
    /// Gentle ripples and a reflection of the sky only.
    Low,
    /// Waves, and reflections of the terrain at a coarser step.
    Medium,
    /// The full set: finer waves and reflections.
    #[default]
    High,
}

impl WaterQuality {
    pub const ALL: [WaterQuality; 3] = [WaterQuality::Low, WaterQuality::Medium, WaterQuality::High];

    pub fn label(self) -> &'static str {
        match self {
            WaterQuality::Low => "Low",
            WaterQuality::Medium => "Medium",
            WaterQuality::High => "High",
        }
    }

    /// Wave layers summed, steps of the reflection march, and halvings that refine a hit.
    fn budget(self) -> (u32, u32, u32) {
        match self {
            WaterQuality::Low => (2, 0, 0),
            WaterQuality::Medium => (3, 12, 4),
            WaterQuality::High => (LAYERS as u32, 20, 5),
        }
    }
}

/// Everything the water shader is given, gathered in the main world each frame.
#[derive(Resource, Clone, ExtractResource, Default)]
pub struct WaterParams {
    uniform: WaterUniform,
    drift: waves::Drift,
}

impl WaterParams {
    /// The sky's zenith and horizon colours this frame, which water reflects.
    pub fn sky(&self) -> [Vec4; 2] {
        [self.uniform.zenith, self.uniform.horizon]
    }
}

/// See `Water` in water_surface.wgsl.
#[derive(ShaderType, Clone, Copy, Default)]
struct WaterUniform {
    layers: [Vec4; LAYERS],
    phases: [Vec4; LAYERS],
    origin: Vec4,
    flow_origin: Vec4,
    zenith: Vec4,
    horizon: Vec4,
    sun: Vec4,
    optics: Vec4,
    strengths: Vec4,
    budget: UVec4,
}

/// Wave positions are measured from the camera snapped to this many blocks, so they stay
/// small without moving every frame. Flowing water's streaks turn with the flow, so their
/// pattern cannot be carried across a snap exactly; they snap far more rarely.
const ORIGIN_SNAP: f64 = 256.0;
const FLOW_ORIGIN_SNAP: f64 = 4096.0;

#[allow(clippy::too_many_arguments)]
fn update_params(
    time: Res<Time>,
    quality: Res<WaterQuality>,
    pack: Res<ActivePack>,
    wind: Res<Wind>,
    cycle: Res<DayCycle>,
    sky_handle: Option<Res<SkyHandle>>,
    sky_materials: Res<Assets<SkyMaterial>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut params: ResMut<WaterParams>,
) {
    let Ok(camera) = camera.single() else { return };
    let look = &pack.water;
    let (layers, steps, refinements) = quality.budget();
    let at = camera.translation().as_dvec3();
    let origin = (DVec2::new(at.x, at.z) / ORIGIN_SNAP).floor() * ORIGIN_SNAP;
    params.drift.advance(wind.mean(), time.delta_secs_f64());
    let mut uniform = WaterUniform::default();
    (uniform.layers, uniform.phases) = params.drift.uniform(origin);
    // Waves grow with the wind, from a faint stir in calm air to their full height in a
    // strong breeze; how open the water is scales them further, per vertex.
    let windiness = (wind.mean().length() / WINDY).min(1.0);
    let height = look.waves * (CALM_WAVES + (1.0 - CALM_WAVES) * windiness);
    uniform.origin = Vec4::new(origin.x as f32, windiness, origin.y as f32, height);
    let flow_origin = (DVec2::new(at.x, at.z) / FLOW_ORIGIN_SNAP).floor() * FLOW_ORIGIN_SNAP;
    uniform.flow_origin = Vec4::new(flow_origin.x as f32, 0.0, flow_origin.y as f32, 0.0);
    if let Some(sky) = sky_handle.and_then(|handle| sky_materials.get(&handle.0)) {
        uniform.zenith = sky.params.zenith;
        uniform.horizon = sky.params.horizon;
        uniform.sun = sky.params.sun;
    }
    uniform.optics = Vec4::new(look.absorption, look.scattering, look.refraction, if cycle.underwater { 1.0 } else { 0.0 });
    uniform.strengths = Vec4::new(look.reflections, look.foam, if look.pixelated { 1.0 } else { 0.0 }, look.waves);
    uniform.budget = UVec4::new(layers, steps, refinements, 0);
    params.uniform = uniform;
}

/// The water bind group's layout (group 4 of the translucent terrain pipeline): the scene's
/// depth and colour, a sampler, the uniform, the half-resolution depth, and the wave
/// texture with its sampler.
pub fn layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    let entries = if multisampled {
        BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d_multisampled(),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<WaterUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        )
    } else {
        BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d(),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<WaterUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        )
    };
    BindGroupLayoutDescriptor::new(if multisampled { "water_layout_msaa" } else { "water_layout" }, &entries)
}

#[derive(Resource)]
struct WaterGpu {
    uniform: Buffer,
    sampler: Sampler,
    /// The wave texture (`waves`), and its sampler, which repeats and filters its mips.
    waves: TextureView,
    wave_sampler: Sampler,
    /// The half-resolution depth's layouts and pipelines, without and with MSAA.
    depth_layouts: [BindGroupLayoutDescriptor; 2],
    depth_pipelines: [CachedRenderPipelineId; 2],
}

/// The half-resolution depth the reflection march reads (`water_depth.wgsl`).
const MARCH_DEPTH_FORMAT: TextureFormat = TextureFormat::R32Float;

fn init_gpu(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let uniform = device.create_buffer(&BufferDescriptor {
        label: Some("water_uniform"),
        size: WaterUniform::min_size().get(),
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("water_scene_sampler"),
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    let (texels, levels) = waves::texture();
    let waves = device
        .create_texture_with_data(
            &queue,
            &TextureDescriptor {
                label: Some("water_waves"),
                size: Extent3d { width: waves::TEXELS, height: waves::TEXELS, depth_or_array_layers: 1 },
                mip_level_count: levels,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8Snorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            &texels,
        )
        .create_view(&TextureViewDescriptor::default());
    let wave_sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("water_wave_sampler"),
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    let shader = asset_server.load("embedded://voxmar/render/water_depth.wgsl");
    let depth_layouts = [false, true].map(|multisampled| {
        let entry = if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() };
        BindGroupLayoutDescriptor::new("water_depth_layout", &BindGroupLayoutEntries::single(ShaderStages::FRAGMENT, entry))
    });
    let depth_pipelines = [0, 1].map(|i| {
        pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("water_depth".into()),
            layout: vec![depth_layouts[i].clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                shader_defs: if i == 1 { vec!["MULTISAMPLED".into()] } else { Vec::new() },
                targets: vec![Some(ColorTargetState { format: MARCH_DEPTH_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    });
    commands.insert_resource(WaterGpu { uniform, sampler, waves, wave_sampler, depth_layouts, depth_pipelines });
}

/// Each camera's half-resolution depth, while it has water in sight.
#[derive(Component)]
struct MarchDepth(CachedTexture);

fn prepare_depth_textures(
    mut commands: Commands,
    mut cache: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera, Option<&TerrainDraws>), With<Camera3d>>,
) {
    for (entity, camera, draws) in &views {
        let Some(size) = camera.physical_target_size.filter(|_| draws.is_some_and(TerrainDraws::has_water)) else {
            commands.entity(entity).remove::<MarchDepth>();
            continue;
        };
        let texture = cache.get(
            &device,
            TextureDescriptor {
                label: Some("water_march_depth"),
                size: Extent3d { width: size.x.div_ceil(2), height: size.y.div_ceil(2), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: MARCH_DEPTH_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        commands.entity(entity).insert(MarchDepth(texture));
    }
}

fn write_uniform(params: Res<WaterParams>, gpu: Option<Res<WaterGpu>>, queue: Res<RenderQueue>) {
    let Some(gpu) = gpu else { return };
    let mut buffer = encase::UniformBuffer::new(Vec::<u8>::new());
    buffer.write(&params.uniform).expect("the water uniform fits");
    queue.write_buffer(&gpu.uniform, 0, buffer.as_ref());
}

/// Stands for the translucent terrain in the water pass, to run its draw commands.
pub struct WaterItem {
    pub pipeline: CachedRenderPipelineId,
    pub entity: (Entity, MainEntity),
    pub draw_function: DrawFunctionId,
    pub batch_range: std::ops::Range<u32>,
    pub extra_index: PhaseItemExtraIndex,
}

impl PhaseItem for WaterItem {
    const AUTOMATIC_BATCHING: bool = false;

    fn entity(&self) -> Entity {
        self.entity.0
    }

    fn main_entity(&self) -> MainEntity {
        self.entity.1
    }

    fn draw_function(&self) -> DrawFunctionId {
        self.draw_function
    }

    fn batch_range(&self) -> &std::ops::Range<u32> {
        &self.batch_range
    }

    fn batch_range_mut(&mut self) -> &mut std::ops::Range<u32> {
        &mut self.batch_range
    }

    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra_index.clone()
    }

    fn batch_range_and_extra_index_mut(&mut self) -> (&mut std::ops::Range<u32>, &mut PhaseItemExtraIndex) {
        (&mut self.batch_range, &mut self.extra_index)
    }
}

impl CachedRenderPipelinePhaseItem for WaterItem {
    fn cached_pipeline(&self) -> CachedRenderPipelineId {
        self.pipeline
    }
}

/// The translucent terrain's draw commands; `water_pass` binds group 4 itself.
type DrawWater = (SetItemPipeline, SetMeshViewBindGroup<0>, SetMeshViewBindingArrayBindGroup<1>, DrawTerrain);

/// What the water pass needs of a camera.
type WaterView = (
    &'static ExtractedCamera,
    &'static ViewTarget,
    &'static ViewDepthTexture,
    &'static Msaa,
    &'static TerrainDraws,
    Option<&'static MarchDepth>,
);

/// Draws the translucent terrain of a camera with the scene so far bound for reading. With
/// MSAA it draws into the multisampled image, reading the resolved one, and resolves into
/// the other main image; without, it copies the scene into the other main image and draws
/// there. Either way that image becomes the main one, and Bevy's transparent pass carries
/// on in it.
fn water_pass(
    world: &World,
    view: ViewQuery<WaterView>,
    gpu: Option<Res<WaterGpu>>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();
    let (camera, target, depth, msaa, draws, march) = view.into_inner();
    let (Some(gpu), Some(water), Some(size), Some(MarchDepth(march))) = (gpu, draws.water(), camera.physical_target_size, march) else {
        return;
    };
    let multisampled = *msaa != Msaa::Off;
    let Some(depth_pipeline) = pipeline_cache.get_render_pipeline(gpu.depth_pipelines[multisampled as usize]) else { return };

    // The half-resolution depth for the reflection march.
    let depth_bind_group = ctx.render_device().create_bind_group(
        "water_depth_bind_group",
        &pipeline_cache.get_bind_group_layout(&gpu.depth_layouts[multisampled as usize]),
        &BindGroupEntries::single(depth.view()),
    );
    {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("water_depth"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &march.default_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(depth_pipeline);
        pass.set_bind_group(0, &depth_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    // The scene so far stays the source the water reads, and the water's result goes to the
    // other main image, which then becomes the main one.
    let write = target.post_process_write();
    let color = match target.sampled_main_texture_view().filter(|_| multisampled) {
        // The multisampled image, resolved at the end of the pass.
        Some(samples) => RenderPassColorAttachment {
            view: samples,
            depth_slice: None,
            resolve_target: Some(write.destination),
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        },
        // Without samples to draw into, the destination starts as a copy of the scene.
        None => {
            ctx.command_encoder().copy_texture_to_texture(
                write.source_texture.as_image_copy(),
                write.destination_texture.as_image_copy(),
                Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
            );
            RenderPassColorAttachment {
                view: write.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
            }
        }
    };
    let bind_group = ctx.render_device().create_bind_group(
        "water_bind_group",
        &pipeline_cache.get_bind_group_layout(&layout(multisampled)),
        &BindGroupEntries::sequential((
            depth.view(),
            write.source,
            &gpu.sampler,
            gpu.uniform.as_entire_binding(),
            &march.default_view,
            &gpu.waves,
            &gpu.wave_sampler,
        )),
    );
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("water_pass"),
        color_attachments: &[Some(color)],
        // Read-only, so the same depth can be sampled while it is tested against.
        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment { view: depth.view(), depth_ops: None, stencil_ops: None }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let span = diagnostics.pass_span(&mut pass, "water_pass");
    if let Some(viewport) = camera.viewport.as_ref() {
        pass.set_camera_viewport(viewport);
    }
    pass.set_bind_group(4, &bind_group, &[]);
    let functions = world.resource::<DrawFunctions<WaterItem>>();
    let mut functions = functions.write();
    functions.prepare(world);
    if let Some(id) = functions.get_id::<DrawWater>() {
        let item = WaterItem {
            pipeline: water.pipeline,
            entity: water.entity,
            draw_function: id,
            batch_range: 0..1,
            extra_index: PhaseItemExtraIndex::None,
        };
        if let Some(draw) = functions.get_mut(id)
            && let Err(e) = draw.draw(world, &mut pass, view_entity, &item)
        {
            error!("water pass: {e:?}");
        }
    }
    span.end(&mut pass);
}
