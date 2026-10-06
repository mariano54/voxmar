//! The air in front of the view, drawn over the opaque scene and the water before Bevy's
//! transparent pass (rain and snow then fall in front of it) (volumetric.wgsl): the low
//! cloud, a layer in the world that passes over, around or below the eye; ground fog near
//! the eye, uneven and drifting; and sunlight the air scatters where the sun reaches it
//! (light shafts), marched through Bevy's cascaded shadow maps. Rays are marched at half
//! resolution into a cached texture, the light they gather and how much of the scene shows
//! through; blended into the frames before, found where the same air was by last frame's
//! view, so the grain of each frame's steps (which start at new noise every frame) averages
//! away; then composited at full resolution with a depth-aware upsample, so the pass costs a
//! quarter of what full resolution would and keeps crisp edges against what is in front.
//! The accumulation took the place of a spatial blur among neighbouring rays, which left the
//! clouds grainy and soft (Mariano's playtest, 2026-10-05).
//!
//! The pipelines are built from the mesh pipeline's descriptor for the view, like the
//! terrain's, so the view bind group and its shader defs (multisampling among them) match
//! exactly; only the shaders, targets and our own group 2 differ. Under MSAA the composite
//! draws into the multisampled image, which the transparent pass carries on in and which is
//! resolved once, at the end (`resolve`). The pass is
//! skipped when there is no cloud, ground fog or light shafts to draw, and under water.

use bevy::asset::embedded_asset;
use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::mesh::{MeshVertexBufferLayout, MeshVertexBufferLayouts, PrimitiveTopology, VertexBufferLayout};
use bevy::pbr::{MeshPipeline, MeshPipelineKey, MeshViewBindGroup, ViewKeyCache};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BindGroupLayoutEntryBuilder,
    BlendComponent, BlendFactor, BlendOperation, BlendState, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d,
    LoadOp, MultisampleState, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType,
    SpecializedMeshPipeline, StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    UniformBuffer, VertexAttribute, VertexStepMode,
    binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
};
use bevy::diagnostic::FrameCount;
use bevy::image::ImageFilterMode;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;
use rustc_hash::FxHashMap;

use super::ATTRIBUTE_VOXEL;
use super::atmosphere::{Atmosphere, AtmosphereBuffer};
use super::cloud_map::{self, CloudMap};
use super::water::WaterPass;

const SHADER: &str = "embedded://voxmar/render/volumetric.wgsl";
/// The half-resolution light, with how much of the scene shows through in alpha.
const MARCH_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// The view depth each half-resolution ray ended at, which the composite compares its own
/// pixel's against for every ray it blends: read from this small texture rather than as
/// sixteen loads from the full-resolution depth per pixel, it halves the composite's cost.
/// And how far away the ray's light comes from, where the next frame looks for it again.
const RAY_DEPTH_FORMAT: TextureFormat = TextureFormat::Rg16Float;
/// The depths the accumulated rays ended at, for the next frame to tell an edge that moved.
const HISTORY_DEPTH_FORMAT: TextureFormat = TextureFormat::R16Float;
/// How much of each frame's march goes into the accumulated air: about the last eight
/// frames make it up.
const BLEND: f32 = 0.12;

pub struct VolumetricPlugin;

impl Plugin for VolumetricPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "volumetric.wgsl");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(
                Core3d,
                volumetric_light.in_set(VolumetricPass).after(WaterPass).before(main_transparent_pass_3d).in_set(Core3dSystems::MainPass),
            );
    }
}

/// Our group 2 for one pass, with a single-sampled and a multisampled depth texture.
struct Layouts {
    single: BindGroupLayoutDescriptor,
    multisampled: BindGroupLayoutDescriptor,
}

impl Layouts {
    /// `layout` makes the group from the depth texture's entry.
    fn new(layout: impl Fn(BindGroupLayoutEntryBuilder) -> BindGroupLayoutDescriptor) -> Self {
        Self { single: layout(texture_depth_2d()), multisampled: layout(texture_depth_2d_multisampled()) }
    }

    fn get(&self, multisampled: bool) -> &BindGroupLayoutDescriptor {
        if multisampled { &self.multisampled } else { &self.single }
    }
}

#[derive(Resource)]
struct VolumetricPipelines {
    shader: Handle<Shader>,
    /// The march reads depth, the atmosphere and the cloud maps and noise; the composite
    /// depth and the march.
    march: Layouts,
    accumulate: Layouts,
    composite: Layouts,
    /// Reads the history between its texels, where last frame's view put it.
    linear: Sampler,
    /// A vertex layout to hand the mesh pipeline, which our full-screen triangle ignores.
    vertex_layout: MeshVertexBufferLayout,
    /// The march and composite pipelines for each view key.
    pipelines: FxHashMap<MeshPipelineKey, Passes>,
}

/// The pipelines of a view's three passes.
#[derive(Clone, Copy)]
struct Passes {
    march: CachedRenderPipelineId,
    accumulate: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
}

/// The volumetric pass in the `Core3d` schedule, which particles that blend draw over.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumetricPass;

/// This frame's pipelines, bind groups and half-resolution target for a camera that draws
/// volumetric light.
#[derive(Component)]
struct VolumetricView {
    passes: Passes,
    march_group: BindGroup,
    accumulate_group: BindGroup,
    composite_group: BindGroup,
    target: CachedTexture,
    ray_depths: CachedTexture,
    /// This frame's accumulated air and its rays' depths (last frame's are read).
    accumulated: CachedTexture,
    accumulated_depths: CachedTexture,
}

/// Last frame's view of a camera that drew volumetric light, and its size, so the next
/// frame finds where the air it accumulated lies now.
#[derive(Component)]
struct PreviousView {
    clip_from_world: Mat4,
    size: UVec2,
}

#[derive(ShaderType)]
struct PreviousUniform {
    clip_from_world: Mat4,
    blend: Vec4,
}

fn init(mut commands: Commands, asset_server: Res<AssetServer>, device: Res<RenderDevice>) {
    let vertex_layout = MeshVertexBufferLayout::new(
        vec![ATTRIBUTE_VOXEL.id],
        VertexBufferLayout {
            array_stride: 8,
            step_mode: VertexStepMode::Vertex,
            attributes: vec![VertexAttribute { format: ATTRIBUTE_VOXEL.format, offset: 0, shader_location: 0 }],
        },
    );
    commands.insert_resource(VolumetricPipelines {
        shader: asset_server.load(SHADER),
        march: Layouts::new(|depth| {
            let mut layout = BindGroupLayoutDescriptor::new(
                "volumetric_march_layout",
                &BindGroupLayoutEntries::with_indices(ShaderStages::FRAGMENT, ((0, depth), (1, uniform_buffer::<Atmosphere>(false)))),
            );
            layout.entries.extend(cloud_map::layout_entries(ShaderStages::FRAGMENT));
            layout
        }),
        accumulate: Layouts::new(|depth| {
            let exact = || texture_2d(TextureSampleType::Float { filterable: false });
            BindGroupLayoutDescriptor::new(
                "volumetric_accumulate_layout",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (
                        depth,
                        exact(),
                        exact(),
                        texture_2d(TextureSampleType::Float { filterable: true }),
                        exact(),
                        sampler(SamplerBindingType::Filtering),
                        uniform_buffer::<PreviousUniform>(false),
                    ),
                ),
            )
        }),
        composite: Layouts::new(|depth| {
            BindGroupLayoutDescriptor::new(
                "volumetric_composite_layout",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (depth, texture_2d(TextureSampleType::Float { filterable: false }), texture_2d(TextureSampleType::Float { filterable: false })),
                ),
            )
        }),
        linear: device.create_sampler(&SamplerDescriptor {
            label: Some("volumetric_history"),
            mag_filter: ImageFilterMode::Linear.into(),
            min_filter: ImageFilterMode::Linear.into(),
            ..default()
        }),
        vertex_layout,
        pipelines: FxHashMap::default(),
    });
}

impl VolumetricPipelines {
    /// The march, accumulation and composite pipelines for a view.
    fn pipelines(
        &mut self,
        key: MeshPipelineKey,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        fullscreen: &FullscreenShader,
        cache: &PipelineCache,
    ) -> Option<Passes> {
        if let Some(&ids) = self.pipelines.get(&key) {
            return Some(ids);
        }
        let mesh_key = key | MeshPipelineKey::from_primitive_topology_and_strip_index(PrimitiveTopology::TriangleList, None);
        let base = match mesh_pipeline.specialize(mesh_key, &layouts.insert(self.vertex_layout.clone())) {
            Ok(descriptor) => descriptor,
            Err(e) => {
                error!("volumetric light pipeline: {e}");
                return None;
            }
        };
        let multisampled = key.msaa_samples() > 1;
        let format = base.fragment.as_ref()?.targets.first().cloned().flatten()?.format;
        // The air's light, plus the scene times how much of it shows through.
        let over = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::SrcAlpha, operation: BlendOperation::Add };
        let keep = BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        let target = |format| Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL });
        let march = vec![target(MARCH_FORMAT), target(RAY_DEPTH_FORMAT)];
        let march = self.descriptor(&base, fullscreen, self.march.get(multisampled), "march", march, 1);
        let accumulated = vec![target(MARCH_FORMAT), target(HISTORY_DEPTH_FORMAT)];
        let accumulate = self.descriptor(&base, fullscreen, self.accumulate.get(multisampled), "accumulate", accumulated, 1);
        let composite = vec![Some(ColorTargetState { format, blend: Some(BlendState { color: over, alpha: keep }), write_mask: ColorWrites::COLOR })];
        let composite =
            self.descriptor(&base, fullscreen, self.composite.get(multisampled), "composite", composite, key.msaa_samples());
        let passes = Passes {
            march: cache.queue_render_pipeline(march),
            accumulate: cache.queue_render_pipeline(accumulate),
            composite: cache.queue_render_pipeline(composite),
        };
        self.pipelines.insert(key, passes);
        Some(passes)
    }

    /// The mesh pipeline's descriptor with our full-screen shaders, group 2 and target of
    /// `samples` samples; `entry_point` is the pass, `march`, `accumulate` or `composite`.
    fn descriptor(
        &self,
        base: &RenderPipelineDescriptor,
        fullscreen: &FullscreenShader,
        group: &BindGroupLayoutDescriptor,
        entry_point: &'static str,
        targets: Vec<Option<ColorTargetState>>,
        samples: u32,
    ) -> RenderPipelineDescriptor {
        let mut descriptor = base.clone();
        descriptor.label = Some(format!("volumetric_{entry_point}").into());
        descriptor.layout.truncate(2);
        descriptor.layout.push(group.clone());
        descriptor.vertex = fullscreen.to_vertex_state();
        descriptor.primitive = default();
        descriptor.depth_stencil = None;
        descriptor.multisample = MultisampleState { count: samples, ..default() };
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
            fragment.entry_point = Some(entry_point.into());
            match entry_point {
                "march" => fragment.shader_defs.extend(["MARCH", "NOISE_TEXTURES", "CLOUD_MAP"].map(ShaderDefVal::from)),
                "accumulate" => fragment.shader_defs.push(ShaderDefVal::from("ACCUMULATE")),
                _ => fragment.shader_defs.push(ShaderDefVal::from("COMPOSITE")),
            }
            fragment.targets = targets;
        }
        descriptor
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    mut commands: Commands,
    mut pipelines: ResMut<VolumetricPipelines>,
    mesh_pipeline: Res<MeshPipeline>,
    mut layouts: ResMut<MeshVertexBufferLayouts>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
    view_keys: Res<ViewKeyCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    frames: Res<FrameCount>,
    mut textures: ResMut<TextureCache>,
    atmosphere: Res<Atmosphere>,
    buffer: Res<AtmosphereBuffer>,
    cloud_map: Option<Res<CloudMap>>,
    views: Query<(Entity, &ExtractedView, &ViewDepthTexture, Option<&PreviousView>), (With<ExtractedCamera>, With<Camera3d>)>,
) {
    let Some(cloud_map) = cloud_map else { return };
    for (entity, view, depth, previous) in &views {
        let drawn = atmosphere.rays.x > 0.0 || atmosphere.mist.w > 0.0 || atmosphere.cloud_look.z > 0.0;
        let key = view_keys.get(&view.retained_view_entity).copied().filter(|_| drawn);
        let Some((key, passes)) =
            key.and_then(|key| Some((key, pipelines.pipelines(key, &mesh_pipeline, &mut layouts, &fullscreen, &cache)?)))
        else {
            commands.entity(entity).remove::<(VolumetricView, PreviousView)>();
            continue;
        };
        let size = depth.texture.size();
        let mut half = |label, format| {
            textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d { width: size.width.div_ceil(2), height: size.height.div_ceil(2), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let (target, ray_depths) = (half("volumetric_light", MARCH_FORMAT), half("volumetric_ray_depths", RAY_DEPTH_FORMAT));
        // Two of each, written in turn: this frame's accumulation reads last frame's.
        let histories = [half("volumetric_history_a", MARCH_FORMAT), half("volumetric_history_b", MARCH_FORMAT)];
        let history_depths =
            [half("volumetric_history_depths_a", HISTORY_DEPTH_FORMAT), half("volumetric_history_depths_b", HISTORY_DEPTH_FORMAT)];
        let (write, read) = if frames.0 % 2 == 0 { (0, 1) } else { (1, 0) };
        let clip_from_world = view.clip_from_world.unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let size = UVec2::new(size.width, size.height);
        // The first frame, and one after a resize, starts afresh.
        let (before, blend) = match previous {
            Some(previous) if previous.size == size => (previous.clip_from_world, BLEND),
            _ => (clip_from_world, 1.0),
        };
        let mut uniform = UniformBuffer::from(PreviousUniform { clip_from_world: before, blend: Vec4::new(blend, 0.0, 0.0, 0.0) });
        uniform.write_buffer(&device, &queue);
        let Some(uniform) = uniform.binding() else { continue };
        let multisampled = key.msaa_samples() > 1;
        let march_group = device.create_bind_group(
            "volumetric_march",
            &cache.get_bind_group_layout(pipelines.march.get(multisampled)),
            &[&BindGroupEntries::with_indices(((0, depth.view()), (1, buffer.0.as_entire_binding())))[..], &cloud_map.entries()]
                .concat(),
        );
        let light_group = |label, light: &CachedTexture| {
            device.create_bind_group(
                label,
                &cache.get_bind_group_layout(pipelines.composite.get(multisampled)),
                &BindGroupEntries::sequential((depth.view(), &light.default_view, &ray_depths.default_view)),
            )
        };
        let accumulate_group = device.create_bind_group(
            "volumetric_accumulate",
            &cache.get_bind_group_layout(pipelines.accumulate.get(multisampled)),
            &BindGroupEntries::sequential((
                depth.view(),
                &target.default_view,
                &ray_depths.default_view,
                &histories[read].default_view,
                &history_depths[read].default_view,
                &pipelines.linear,
                uniform,
            )),
        );
        let [accumulated, _] = if write == 0 { histories } else { let [a, b] = histories; [b, a] };
        let [accumulated_depths, _] = if write == 0 { history_depths } else { let [a, b] = history_depths; [b, a] };
        let composite_group = light_group("volumetric_composite", &accumulated);
        commands.entity(entity).insert((
            VolumetricView { passes, march_group, accumulate_group, composite_group, target, ray_depths, accumulated, accumulated_depths },
            PreviousView { clip_from_world, size },
        ));
    }
}

fn volumetric_light(
    view: ViewQuery<(&ViewTarget, &MeshViewBindGroup, &VolumetricView)>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, mesh_view, volumetric) = view.into_inner();
    let passes = volumetric.passes;
    let [Some(march), Some(accumulate), Some(composite)] =
        [passes.march, passes.accumulate, passes.composite].map(|id| pipeline_cache.get_render_pipeline(id))
    else {
        return;
    };
    // March into the half-resolution target, accumulate it there, then blend it over the
    // image (the multisampled one under MSAA, resolved at the end of the main passes; see
    // `resolve`).
    let half = |view| {
        Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Clear(default()), store: StoreOp::Store },
        })
    };
    let march_targets = [half(&volumetric.target.default_view), half(&volumetric.ray_depths.default_view)];
    let accumulate_targets = [half(&volumetric.accumulated.default_view), half(&volumetric.accumulated_depths.default_view)];
    let composite_target = [Some(super::resolve::unresolved_color_attachment(target))];
    let passes = [
        ("volumetric_march", &march_targets[..], march, &volumetric.march_group),
        ("volumetric_accumulate", &accumulate_targets[..], accumulate, &volumetric.accumulate_group),
        ("volumetric_composite", &composite_target[..], composite, &volumetric.composite_group),
    ];
    for (label, attachments, pipeline, group) in passes {
        let descriptor = RenderPassDescriptor {
            label: Some(label),
            color_attachments: attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        };
        let mut pass = ctx.command_encoder().begin_render_pass(&descriptor);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &mesh_view.main, &mesh_view.main_offsets);
        pass.set_bind_group(1, &mesh_view.binding_array, &[]);
        pass.set_bind_group(2, group, &[]);
        pass.draw(0..3, 0..1);
    }
}
