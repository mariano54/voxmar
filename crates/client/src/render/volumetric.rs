//! Volumetric light (light shafts): the sunlight the air scatters towards the eye where the
//! sun reaches it (volumetric.wgsl), added to the HDR image after the main passes and
//! before bloom. Rays are marched through Bevy's cascaded shadow maps at half resolution
//! into a cached texture, then composited at full resolution with a depth-aware upsample,
//! so the pass costs a quarter of what full resolution would and shafts keep crisp edges
//! against what is in front of them.
//!
//! Both pipelines are built from the mesh pipeline's descriptor for the view, like the
//! terrain's, so the view bind group and its shader defs (multisampling among them) match
//! exactly; only the shaders, targets and our own group 2 differ. The pass is skipped when
//! the setting or the shader pack switches it off, or under water.

use bevy::asset::embedded_asset;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::mesh::{MeshVertexBufferLayout, MeshVertexBufferLayouts, PrimitiveTopology, VertexBufferLayout};
use bevy::pbr::{MeshPipeline, MeshPipelineKey, MeshViewBindGroup, ViewKeyCache};
use bevy::post_process::bloom::bloom;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BindGroupLayoutEntryBuilder,
    BlendComponent, BlendFactor, BlendOperation, BlendState, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d,
    LoadOp, MultisampleState, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, ShaderStages, SpecializedMeshPipeline, StoreOp, TextureDescriptor, TextureDimension,
    TextureFormat, TextureSampleType, TextureUsages, VertexAttribute, VertexStepMode,
    binding_types::{texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderDefVal;
use rustc_hash::FxHashMap;

use super::ATTRIBUTE_VOXEL;
use super::atmosphere::{Atmosphere, AtmosphereBuffer};

const SHADER: &str = "embedded://voxmar/render/volumetric.wgsl";
/// The half-resolution light, with each ray's length in alpha.
const MARCH_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

pub struct VolumetricPlugin;

impl Plugin for VolumetricPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "volumetric.wgsl");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(Core3d, volumetric_light.before(bloom).in_set(Core3dSystems::PostProcess));
    }
}

/// Our group 2 for one pass, with a single-sampled and a multisampled depth texture.
struct Layouts {
    single: BindGroupLayoutDescriptor,
    multisampled: BindGroupLayoutDescriptor,
}

impl Layouts {
    fn new(name: &'static str, second: BindGroupLayoutEntryBuilder) -> Self {
        let layout = |depth| BindGroupLayoutDescriptor::new(name, &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (depth, second)));
        Self { single: layout(texture_depth_2d()), multisampled: layout(texture_depth_2d_multisampled()) }
    }

    fn get(&self, multisampled: bool) -> &BindGroupLayoutDescriptor {
        if multisampled { &self.multisampled } else { &self.single }
    }
}

#[derive(Resource)]
struct VolumetricPipelines {
    shader: Handle<Shader>,
    /// The march reads depth and the atmosphere; the composite depth and the march.
    march: Layouts,
    composite: Layouts,
    /// A vertex layout to hand the mesh pipeline, which our full-screen triangle ignores.
    vertex_layout: MeshVertexBufferLayout,
    /// The march and composite pipelines for each view key.
    pipelines: FxHashMap<MeshPipelineKey, (CachedRenderPipelineId, CachedRenderPipelineId)>,
}

/// This frame's pipelines, bind groups and half-resolution target for a camera that draws
/// volumetric light.
#[derive(Component)]
struct VolumetricView {
    march: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
    march_group: BindGroup,
    composite_group: BindGroup,
    target: CachedTexture,
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
        march: Layouts::new("volumetric_march_layout", uniform_buffer::<Atmosphere>(false)),
        composite: Layouts::new("volumetric_composite_layout", texture_2d(TextureSampleType::Float { filterable: false })),
        vertex_layout,
        pipelines: FxHashMap::default(),
    });
}

impl VolumetricPipelines {
    /// The march and composite pipelines for a view.
    fn pipelines(
        &mut self,
        key: MeshPipelineKey,
        mesh_pipeline: &MeshPipeline,
        layouts: &mut MeshVertexBufferLayouts,
        fullscreen: &FullscreenShader,
        cache: &PipelineCache,
    ) -> Option<(CachedRenderPipelineId, CachedRenderPipelineId)> {
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
        let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        let march = ColorTargetState { format: MARCH_FORMAT, blend: None, write_mask: ColorWrites::ALL };
        let march = self.descriptor(&base, fullscreen, self.march.get(multisampled), "march", march);
        let composite =
            ColorTargetState { format, blend: Some(BlendState { color: add, alpha: BlendComponent::OVER }), write_mask: ColorWrites::COLOR };
        let composite = self.descriptor(&base, fullscreen, self.composite.get(multisampled), "composite", composite);
        let ids = (cache.queue_render_pipeline(march), cache.queue_render_pipeline(composite));
        self.pipelines.insert(key, ids);
        Some(ids)
    }

    /// The mesh pipeline's descriptor with our full-screen shaders, group 2 and target;
    /// `entry_point` is the pass, `march` or `composite`.
    fn descriptor(
        &self,
        base: &RenderPipelineDescriptor,
        fullscreen: &FullscreenShader,
        group: &BindGroupLayoutDescriptor,
        entry_point: &'static str,
        target: ColorTargetState,
    ) -> RenderPipelineDescriptor {
        let mut descriptor = base.clone();
        descriptor.label = Some(format!("volumetric_{entry_point}").into());
        descriptor.layout.truncate(2);
        descriptor.layout.push(group.clone());
        descriptor.vertex = fullscreen.to_vertex_state();
        descriptor.primitive = default();
        descriptor.depth_stencil = None;
        // Both draw after multisampling is resolved.
        descriptor.multisample = MultisampleState::default();
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
            fragment.entry_point = Some(entry_point.into());
            if entry_point == "composite" {
                fragment.shader_defs.push(ShaderDefVal::from("COMPOSITE"));
            }
            fragment.targets = vec![Some(target)];
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
    views: Query<(Entity, &ExtractedView, &ViewDepthTexture), With<ExtractedCamera>>,
) {
    for (entity, view, depth) in &views {
        let key = view_keys.get(&view.retained_view_entity).copied().filter(|_| atmosphere.rays.x > 0.0);
        let Some((key, (march, composite))) =
            key.and_then(|key| Some((key, pipelines.pipelines(key, &mesh_pipeline, &mut layouts, &fullscreen, &cache)?)))
        else {
            commands.entity(entity).remove::<VolumetricView>();
            continue;
        };
        let size = depth.texture.size();
        let target = textures.get(
            &device,
            TextureDescriptor {
                label: Some("volumetric_light"),
                size: Extent3d { width: size.width.div_ceil(2), height: size.height.div_ceil(2), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: MARCH_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        let multisampled = key.msaa_samples() > 1;
        let march_group = device.create_bind_group(
            "volumetric_march",
            &cache.get_bind_group_layout(pipelines.march.get(multisampled)),
            &BindGroupEntries::sequential((depth.view(), buffer.0.as_entire_binding())),
        );
        let composite_group = device.create_bind_group(
            "volumetric_composite",
            &cache.get_bind_group_layout(pipelines.composite.get(multisampled)),
            &BindGroupEntries::sequential((depth.view(), &target.default_view)),
        );
        commands.entity(entity).insert(VolumetricView { march, composite, march_group, composite_group, target });
    }
}

fn volumetric_light(
    view: ViewQuery<(&ViewTarget, &MeshViewBindGroup, &VolumetricView)>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, mesh_view, volumetric) = view.into_inner();
    let (Some(march), Some(composite)) =
        (pipeline_cache.get_render_pipeline(volumetric.march), pipeline_cache.get_render_pipeline(volumetric.composite))
    else {
        return;
    };
    // March into the half-resolution target, then add it to the image.
    let passes = [
        (&volumetric.target.default_view, LoadOp::Clear(default()), march, &volumetric.march_group),
        (target.main_texture_view(), LoadOp::Load, composite, &volumetric.composite_group),
    ];
    for (view, load, pipeline, group) in passes {
        let descriptor = RenderPassDescriptor {
            label: Some("volumetric_light"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load, store: StoreOp::Store },
            })],
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
