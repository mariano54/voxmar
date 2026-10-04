//! The air in front of the view, drawn over the opaque scene and the water before Bevy's
//! transparent pass (rain and snow then fall in front of it) (volumetric.wgsl): the low
//! cloud, a layer in the world that passes over, around or below the eye; ground fog near
//! the eye, uneven and drifting; and sunlight the air scatters where the sun reaches it
//! (light shafts), marched through Bevy's cascaded shadow maps. Rays are marched at half
//! resolution into a cached texture, the light they gather and how much of the scene shows
//! through, softened there among rays that ended about as far away, then composited at full
//! resolution with a depth-aware upsample, so the pass costs a quarter of what full
//! resolution would and keeps crisp edges against what is in front. Softening before the
//! upsample rather than in it reads 12.5 texels per full-resolution pixel instead of 32
//! (0.52 ms down to 0.31 at 2268×1473), with the same blur.
//!
//! The pipelines are built from the mesh pipeline's descriptor for the view, like the
//! terrain's, so the view bind group and its shader defs (multisampling among them) match
//! exactly; only the shaders, targets and our own group 2 differ. Under MSAA the composite
//! draws into the multisampled image, which the transparent pass carries on in. The pass is
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
    RenderPipelineDescriptor, SamplerBindingType, ShaderStages, SpecializedMeshPipeline, StoreOp, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, VertexAttribute, VertexStepMode,
    binding_types::{sampler, texture_2d, texture_3d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;
use rustc_hash::FxHashMap;

use super::ATTRIBUTE_VOXEL;
use super::atmosphere::{Atmosphere, AtmosphereBuffer};
use super::cloud_map::CloudMap;
use super::water::WaterPass;

const SHADER: &str = "embedded://voxmar/render/volumetric.wgsl";
/// The half-resolution light, with how much of the scene shows through in alpha.
const MARCH_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// The view depth each half-resolution ray ended at, which the composite compares its own
/// pixel's against for every ray it blends: read from this small texture rather than as
/// sixteen loads from the full-resolution depth per pixel, it halves the composite's cost.
const RAY_DEPTH_FORMAT: TextureFormat = TextureFormat::R16Float;

pub struct VolumetricPlugin;

impl Plugin for VolumetricPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "volumetric.wgsl");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(Core3d, volumetric_light.after(WaterPass).before(main_transparent_pass_3d).in_set(Core3dSystems::MainPass));
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
    composite: Layouts,
    /// A vertex layout to hand the mesh pipeline, which our full-screen triangle ignores.
    vertex_layout: MeshVertexBufferLayout,
    /// The march and composite pipelines for each view key.
    pipelines: FxHashMap<MeshPipelineKey, Passes>,
}

/// The pipelines of a view's three passes.
#[derive(Clone, Copy)]
struct Passes {
    march: CachedRenderPipelineId,
    soften: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
}

/// This frame's pipelines, bind groups and half-resolution target for a camera that draws
/// volumetric light.
#[derive(Component)]
struct VolumetricView {
    passes: Passes,
    march_group: BindGroup,
    soften_group: BindGroup,
    composite_group: BindGroup,
    target: CachedTexture,
    ray_depths: CachedTexture,
    softened: CachedTexture,
}

fn init(mut commands: Commands, asset_server: Res<AssetServer>) {
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
            let noise = TextureSampleType::Float { filterable: true };
            BindGroupLayoutDescriptor::new(
                "volumetric_march_layout",
                &BindGroupLayoutEntries::with_indices(
                    ShaderStages::FRAGMENT,
                    (
                        (0, depth),
                        (1, uniform_buffer::<Atmosphere>(false)),
                        // The cloud maps and noise (cloud_map.rs).
                        (8, texture_2d(noise)),
                        (9, sampler(SamplerBindingType::Filtering)),
                        (10, texture_2d(noise)),
                        (11, texture_2d(noise)),
                        (12, texture_3d(noise)),
                        (13, sampler(SamplerBindingType::Filtering)),
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
        vertex_layout,
        pipelines: FxHashMap::default(),
    });
}

impl VolumetricPipelines {
    /// The march, softening and composite pipelines for a view.
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
        // Softening reads what the composite reads, the march's in place of its own.
        let soften = self.descriptor(&base, fullscreen, self.composite.get(multisampled), "soften", vec![target(MARCH_FORMAT)], 1);
        let composite = vec![Some(ColorTargetState { format, blend: Some(BlendState { color: over, alpha: keep }), write_mask: ColorWrites::COLOR })];
        let composite =
            self.descriptor(&base, fullscreen, self.composite.get(multisampled), "composite", composite, key.msaa_samples());
        let passes = Passes {
            march: cache.queue_render_pipeline(march),
            soften: cache.queue_render_pipeline(soften),
            composite: cache.queue_render_pipeline(composite),
        };
        self.pipelines.insert(key, passes);
        Some(passes)
    }

    /// The mesh pipeline's descriptor with our full-screen shaders, group 2 and target of
    /// `samples` samples; `entry_point` is the pass, `march`, `soften` or `composite`.
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
            if entry_point != "march" {
                fragment.shader_defs.push(ShaderDefVal::from("COMPOSITE"));
            } else {
                fragment.shader_defs.extend(["NOISE_TEXTURES", "CLOUD_MAP"].map(ShaderDefVal::from));
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
    mut textures: ResMut<TextureCache>,
    atmosphere: Res<Atmosphere>,
    buffer: Res<AtmosphereBuffer>,
    cloud_map: Option<Res<CloudMap>>,
    views: Query<(Entity, &ExtractedView, &ViewDepthTexture), (With<ExtractedCamera>, With<Camera3d>)>,
) {
    let Some(cloud_map) = cloud_map else { return };
    for (entity, view, depth) in &views {
        let drawn = atmosphere.rays.x > 0.0 || atmosphere.mist.w > 0.0 || atmosphere.cloud_look.z > 0.0;
        let key = view_keys.get(&view.retained_view_entity).copied().filter(|_| drawn);
        let Some((key, passes)) =
            key.and_then(|key| Some((key, pipelines.pipelines(key, &mesh_pipeline, &mut layouts, &fullscreen, &cache)?)))
        else {
            commands.entity(entity).remove::<VolumetricView>();
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
        let softened = half("volumetric_softened", MARCH_FORMAT);
        let multisampled = key.msaa_samples() > 1;
        let march_group = device.create_bind_group(
            "volumetric_march",
            &cache.get_bind_group_layout(pipelines.march.get(multisampled)),
            &BindGroupEntries::with_indices((
                (0, depth.view()),
                (1, buffer.0.as_entire_binding()),
                (8, &cloud_map.view),
                (9, &cloud_map.sampler),
                (10, &cloud_map.shade),
                (11, &cloud_map.noise.plane),
                (12, &cloud_map.noise.volume),
                (13, &cloud_map.noise.sampler),
            )),
        );
        let light_group = |label, light: &CachedTexture| {
            device.create_bind_group(
                label,
                &cache.get_bind_group_layout(pipelines.composite.get(multisampled)),
                &BindGroupEntries::sequential((depth.view(), &light.default_view, &ray_depths.default_view)),
            )
        };
        let (soften_group, composite_group) = (light_group("volumetric_soften", &target), light_group("volumetric_composite", &softened));
        commands.entity(entity).insert(VolumetricView {
            passes,
            march_group,
            soften_group,
            composite_group,
            target,
            ray_depths,
            softened,
        });
    }
}

fn volumetric_light(
    view: ViewQuery<(&ViewTarget, &MeshViewBindGroup, &VolumetricView)>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, mesh_view, volumetric) = view.into_inner();
    let passes = volumetric.passes;
    let [Some(march), Some(soften), Some(composite)] =
        [passes.march, passes.soften, passes.composite].map(|id| pipeline_cache.get_render_pipeline(id))
    else {
        return;
    };
    // March into the half-resolution target, soften it there, then blend it over the image
    // (the multisampled one under MSAA, resolved at the end of the pass).
    let half = |view| {
        Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Clear(default()), store: StoreOp::Store },
        })
    };
    let march_targets = [half(&volumetric.target.default_view), half(&volumetric.ray_depths.default_view)];
    let soften_target = [half(&volumetric.softened.default_view)];
    let composite_target = [Some(target.get_color_attachment())];
    let passes = [
        ("volumetric_march", &march_targets[..], march, &volumetric.march_group),
        ("volumetric_soften", &soften_target[..], soften, &volumetric.soften_group),
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
