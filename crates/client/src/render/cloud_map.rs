//! The cloud map: the low cloud's 2D body (clouds.wgsl, `cloud_body`) around the eye, drawn
//! into a texture once a frame (cloud_map.wgsl) before the main passes. Every fragment of
//! terrain, water, grass and particles looks up its cloud shadow there, and the volumetric
//! pass its cloud, with one filtered read instead of several octaves of hashed noise: under
//! cloud, the noise in each terrain fragment's shadow cost more than the rest of its shading
//! (about 5 ms a frame at 1920×1080 in a cumulus sky), the map costs a tenth of a
//! millisecond. Its texels are fixed in the world (the map moves with the eye in whole
//! texels), so shadows stay still as the eye moves; beyond the map, shaders work the body
//! out themselves.
//!
//! The map's place goes to the shaders in the atmosphere uniform (`cloud_map`); pipelines
//! that read it set `CLOUD_MAP` and bind it at group 2, bindings 8 and 9.

use bevy::asset::embedded_asset;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{
    AddressMode, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState, LoadOp, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerDescriptor,
    ShaderStages, StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
    binding_types::uniform_buffer,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::{RenderApp, RenderStartup};

use super::atmosphere::{Atmosphere, AtmosphereBuffer};

const SHADER: &str = "embedded://voxmar/render/cloud_map.wgsl";
/// Texels across the map, and blocks it spans: 12.5 blocks a texel, finer than the body's
/// smallest octave (about 60 blocks), out past the farthest cloud drawn (6000 blocks).
pub const SIZE: u32 = 1024;
pub const EXTENT: f32 = 12_800.0;
const FORMAT: TextureFormat = TextureFormat::R16Float;

/// Where the map's corner lies from the eye at `eye` (x and z), in whole texels of the
/// world, so the texels stay put as the eye moves.
pub fn corner(eye: DVec2) -> Vec2 {
    let texel = EXTENT as f64 / SIZE as f64;
    let corner = ((eye - EXTENT as f64 / 2.0) / texel).floor() * texel;
    (corner - eye).as_vec2()
}

pub struct CloudMapPlugin;

impl Plugin for CloudMapPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "cloud_map.wgsl");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app.add_systems(RenderStartup, init).add_systems(Core3d, draw.in_set(Core3dSystems::Prepass));
    }
}

/// The map, its sampler and the pipeline that draws it.
#[derive(Resource)]
pub struct CloudMap {
    pub view: TextureView,
    pub sampler: Sampler,
    pipeline: CachedRenderPipelineId,
    layout: BindGroupLayoutDescriptor,
    group: Option<BindGroup>,
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("cloud_map"),
        size: Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("cloud_map"),
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    let layout = BindGroupLayoutDescriptor::new(
        "cloud_map_layout",
        &BindGroupLayoutEntries::single(ShaderStages::FRAGMENT, uniform_buffer::<Atmosphere>(false)),
    );
    let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("cloud_map".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: asset_server.load(SHADER),
            targets: vec![Some(ColorTargetState { format: FORMAT, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    commands.insert_resource(CloudMap {
        view: texture.create_view(&TextureViewDescriptor::default()),
        sampler,
        pipeline,
        layout,
        group: None,
    });
}

/// Draws the map while there is cloud (`Atmosphere::cloud_map.w`).
fn draw(
    _view: ViewQuery<&ExtractedCamera>,
    mut map: ResMut<CloudMap>,
    atmosphere: Res<Atmosphere>,
    buffer: Res<AtmosphereBuffer>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    if atmosphere.cloud_map.w <= 0.0 {
        return;
    }
    let Some(pipeline) = cache.get_render_pipeline(map.pipeline) else { return };
    if map.group.is_none() {
        let group = ctx.render_device().create_bind_group(
            "cloud_map",
            &cache.get_bind_group_layout(&map.layout),
            &BindGroupEntries::single(buffer.0.as_entire_binding()),
        );
        map.group = Some(group);
    }
    let Some(group) = map.group.as_ref() else { return };
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("cloud_map"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: &map.view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Clear(default()), store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}
